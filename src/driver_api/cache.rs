//! Isi cache metadata (`database_cache`, `table_cache`, `column_cache`) untuk
//! koneksi engine plugin, dengan format yang sama seperti driver builtin.

use super::{DriverResult, EngineSession, PluginPool, TableKind, run_blocking};
use crate::connection::metadata::staging::{ColumnMetaStaging, MetadataStaging, TableMetaStaging};
use sqlx::SqlitePool;
use std::sync::Arc;

/// Batas tabel yang kolomnya diambil saat refresh; sisanya dimuat saat
/// tabel dibuka, supaya refresh database besar tidak berjalan lama.
const MAX_TABLES_WITH_COLUMNS: usize = 500;

/// Nama tabel di cache: `schema.table` bila engine punya schema.
pub fn cache_table_name(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!("{s}.{table}"),
        _ => table.to_string(),
    }
}

/// Kebalikan [`cache_table_name`] untuk engine dengan schema.
pub fn split_cache_table_name(schemas: bool, name: &str) -> (Option<&str>, &str) {
    if schemas && let Some((s, t)) = name.split_once('.') {
        return (Some(s), t);
    }
    (None, name)
}

fn kind_label(kind: TableKind) -> &'static str {
    match kind {
        TableKind::Table | TableKind::Other => "table",
        TableKind::View => "view",
    }
}

/// Database yang ditampilkan untuk koneksi: daftar dari engine, atau satu
/// nama (database koneksi / `default`) bila engine tidak punya konsep itu.
pub fn databases_blocking(
    session: &dyn EngineSession,
    has_databases: bool,
    configured: &str,
) -> DriverResult<Vec<String>> {
    if has_databases {
        let list = session.list_databases()?;
        if !list.is_empty() {
            return Ok(list);
        }
    }
    Ok(vec![if configured.is_empty() {
        "default".to_string()
    } else {
        configured.to_string()
    }])
}

fn stage_blocking(
    connection_id: i64,
    pool: &PluginPool,
    configured_db: &str,
) -> DriverResult<MetadataStaging> {
    let session = pool.session.as_ref();
    let caps = &pool.capabilities;
    let mut staging = MetadataStaging::new(connection_id);
    let mut with_columns = 0usize;
    for database in databases_blocking(session, caps.databases, configured_db)? {
        let db_arg = caps.databases.then_some(database.as_str());
        let schemas: Vec<Option<String>> = if caps.schemas {
            session
                .list_schemas(db_arg)?
                .into_iter()
                .map(Some)
                .collect()
        } else {
            vec![None]
        };
        let staged_db = staging.add_database(database.clone());
        for schema in &schemas {
            let tables = match session.list_tables(db_arg, schema.as_deref()) {
                Ok(t) => t,
                Err(e) => {
                    log::warn!(
                        "[DRIVER-PLUGIN] list_tables failed for {database}/{schema:?}: {e}"
                    );
                    continue;
                }
            };
            for table in tables {
                let mut columns = Vec::new();
                if with_columns < MAX_TABLES_WITH_COLUMNS {
                    with_columns += 1;
                    match session.list_columns(db_arg, schema.as_deref(), &table.name) {
                        Ok(cols) => {
                            columns = cols
                                .into_iter()
                                .enumerate()
                                .map(|(i, c)| ColumnMetaStaging {
                                    column_name: c.name,
                                    data_type: c.data_type,
                                    ordinal_position: i as i64,
                                })
                                .collect();
                        }
                        Err(e) => log::debug!(
                            "[DRIVER-PLUGIN] list_columns failed for {}: {e}",
                            table.name
                        ),
                    }
                }
                staged_db.tables.push(TableMetaStaging {
                    table_name: cache_table_name(schema.as_deref(), &table.name),
                    table_type: kind_label(table.kind).to_string(),
                    columns,
                    indexes: Vec::new(),
                });
            }
        }
    }
    Ok(staging)
}

/// Daftar database untuk sidebar (lihat [`databases_blocking`]).
pub async fn list_databases(pool: &Arc<PluginPool>, configured_db: &str) -> Option<Vec<String>> {
    let pool = pool.clone();
    let configured = configured_db.to_string();
    match run_blocking(move || {
        databases_blocking(pool.session.as_ref(), pool.capabilities.databases, &configured)
    })
    .await
    {
        Ok(list) => Some(list),
        Err(e) => {
            log::warn!("[DRIVER-PLUGIN] list_databases failed: {e}");
            None
        }
    }
}

/// Kolom satu tabel `(nama, tipe)` untuk koneksi plugin. `table` boleh
/// berbentuk `schema.table` seperti di cache.
pub async fn fetch_columns(
    connection: &crate::models::structs::ConnectionConfig,
    engine_id: &str,
    database: &str,
    table: &str,
) -> Option<Vec<(String, String)>> {
    let pool = match super::connect::pool_for(connection, engine_id).await {
        Ok(pool) => pool,
        Err(e) => {
            log::debug!("[DRIVER-PLUGIN] cannot open session for columns: {e}");
            return None;
        }
    };
    let (schema, table) = split_cache_table_name(pool.capabilities.schemas, table);
    let (schema, table) = (schema.map(str::to_string), table.to_string());
    let db = pool.capabilities.databases.then(|| database.to_string());
    run_blocking(move || {
        pool.session
            .list_columns(db.as_deref(), schema.as_deref(), &table)
    })
    .await
    .ok()
    .map(|cols| cols.into_iter().map(|c| (c.name, c.data_type)).collect())
}

/// Ambil metadata engine plugin dan tulis ke cache. Mengembalikan `true`
/// bila berhasil, sama seperti `fetch_*_data` driver builtin.
pub async fn fetch_plugin_data(
    connection_id: i64,
    pool: &Arc<PluginPool>,
    configured_db: &str,
    cache_pool: &SqlitePool,
) -> bool {
    let pool = pool.clone();
    let configured = configured_db.to_string();
    let staged = run_blocking(move || stage_blocking(connection_id, &pool, &configured)).await;
    match staged {
        Ok(staging) => staging.commit_to_sqlite(cache_pool).await.is_ok(),
        Err(e) => {
            log::warn!("[DRIVER-PLUGIN] metadata refresh failed for {connection_id}: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver_api::sqlite_adapter::SqliteSession;
    use crate::driver_api::{EngineCapabilities, ExecuteRequest};

    #[test]
    fn table_name_roundtrip() {
        assert_eq!(cache_table_name(Some("public"), "t"), "public.t");
        assert_eq!(cache_table_name(None, "t"), "t");
        assert_eq!(split_cache_table_name(true, "public.t"), (Some("public"), "t"));
        assert_eq!(split_cache_table_name(false, "a.b"), (None, "a.b"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn stages_tables_and_columns_from_session() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let session = Arc::new(SqliteSession::from_pool(
            pool,
            tokio::runtime::Handle::current(),
        ));
        let s = session.clone();
        run_blocking(move || {
            s.execute(&ExecuteRequest {
                query: "CREATE TABLE a (x INTEGER, y TEXT)".into(),
                database: None,
                schema: None,
                max_rows: 0,
                job_id: 1,
            })?;
            s.execute(&ExecuteRequest {
                query: "CREATE VIEW v AS SELECT x FROM a".into(),
                database: None,
                schema: None,
                max_rows: 0,
                job_id: 2,
            })
        })
        .await
        .unwrap();

        let plugin_pool = Arc::new(PluginPool {
            engine_id: "sqlite-adapter".into(),
            capabilities: EngineCapabilities {
                databases: false,
                ..Default::default()
            },
            session,
        });
        let p = plugin_pool.clone();
        let staging = run_blocking(move || stage_blocking(9, &p, "")).await.unwrap();
        assert_eq!(staging.databases.len(), 1);
        let db = &staging.databases[0];
        assert_eq!(db.database_name, "default");
        let a = db.tables.iter().find(|t| t.table_name == "a").unwrap();
        assert_eq!(a.table_type, "table");
        assert_eq!(a.columns.len(), 2);
        let v = db.tables.iter().find(|t| t.table_name == "v").unwrap();
        assert_eq!(v.table_type, "view");
    }
}
