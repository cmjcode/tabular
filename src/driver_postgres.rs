use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row, SqlitePool};

use crate::{models, window_egui};

#[allow(dead_code)]
pub(crate) async fn fetch_postgres_data(
    connection_id: i64,
    pool: &PgPool,
    cache_pool: &SqlitePool,
) -> bool {
    use crate::connection::metadata::staging::{
        ColumnMetaStaging, MetadataStaging, TableMetaStaging,
    };

    let mut staging = MetadataStaging::new(connection_id);

    // 1) Cache database names
    let db_rows = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        sqlx::query("SELECT datname FROM pg_database WHERE datistemplate = false").fetch_all(pool),
    )
    .await
    .map_err(|_| sqlx::Error::PoolTimedOut)
    .and_then(|r| r)
    {
        Ok(r) => r,
        Err(_) => return false,
    };

    for row in db_rows {
        if let Ok(db_name) = row.try_get::<String, _>(0) {
            staging.add_database(&db_name);
        }
    }

    // 2) Cache tables/views for the CURRENT database only
    let current_db: Option<String> = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        sqlx::query_scalar("SELECT current_database()").fetch_one(pool),
    )
    .await
    .ok()
    .and_then(|r| r.ok());

    if let Some(db_name) = current_db {
        let staged_db = staging.add_database(&db_name);

        // Pre-fetch all columns for public schema in one batch query to eliminate N+1 latency
        let mut columns_by_table: std::collections::HashMap<String, Vec<ColumnMetaStaging>> =
            std::collections::HashMap::new();

        if let Ok(col_rows) = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            sqlx::query(
                "SELECT table_name, column_name, data_type, ordinal_position \
                 FROM information_schema.columns \
                 WHERE table_schema = 'public' \
                 ORDER BY table_name, ordinal_position",
            )
            .fetch_all(pool),
        )
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)
        .and_then(|r| r)
        {
            for col_row in col_rows {
                if let (Ok(tbl_name), Ok(col_name), Ok(col_type), Ok(ordinal_pos)) = (
                    col_row.try_get::<String, _>(0),
                    col_row.try_get::<String, _>(1),
                    col_row.try_get::<String, _>(2),
                    col_row.try_get::<i32, _>(3),
                ) {
                    columns_by_table
                        .entry(tbl_name)
                        .or_default()
                        .push(ColumnMetaStaging {
                            column_name: col_name,
                            data_type: col_type,
                            ordinal_position: ordinal_pos as i64,
                        });
                }
            }
        }

        // Tables (public)
        if let Ok(table_rows) = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sqlx::query("SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'").fetch_all(pool),
        )
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)
        .and_then(|r| r)
        {
            for table_row in table_rows {
                if let Ok(table_name) = table_row.try_get::<String, _>(0) {
                    let staged_table = TableMetaStaging {
                        columns: columns_by_table.remove(&table_name).unwrap_or_default(),
                        indexes: Vec::new(),
                        table_name,
                        table_type: "table".to_string(),
                    };

                    staged_db.tables.push(staged_table);
                }
            }
        }

        // Views (public)
        if let Ok(view_rows) = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sqlx::query(
                "SELECT table_name FROM information_schema.views WHERE table_schema = 'public'",
            )
            .fetch_all(pool),
        )
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)
        .and_then(|r| r)
        {
            for view_row in view_rows {
                if let Ok(view_name) = view_row.try_get::<String, _>(0) {
                    staged_db.tables.push(TableMetaStaging {
                        columns: columns_by_table.remove(&view_name).unwrap_or_default(),
                        indexes: Vec::new(),
                        table_name: view_name,
                        table_type: "view".to_string(),
                    });
                }
            }
        }
    }

    staging.commit_to_sqlite(cache_pool).await.is_ok()
}

pub(crate) fn load_postgresql_structure(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Create basic structure for PostgreSQL
    let mut main_children = Vec::new();

    // Databases folder
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(connection_id);
    databases_folder.is_loaded = false;

    // Add a loading indicator
    let loading_node = models::structs::TreeNode::new(
        "Loading databases...".to_string(),
        models::enums::NodeType::Database,
    );
    databases_folder.children.push(loading_node);

    main_children.push(databases_folder);

    // DBA Views folder similar to other drivers
    let mut dba_folder = models::structs::TreeNode::new(
        "DBA Views".to_string(),
        models::enums::NodeType::DBAViewsFolder,
    );
    dba_folder.connection_id = Some(connection_id);

    let mut dba_children = Vec::new();

    for (name, node_type, query) in
        crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::PostgreSQL)
    {
        let mut dba_node = models::structs::TreeNode::new(name.to_string(), node_type);
        dba_node.connection_id = Some(connection_id);
        dba_node.is_loaded = false;
        dba_node.query = Some(query.to_string());
        dba_children.push(dba_node);
    }

    // Render Custom Views
    log::debug!(
        "Rendering custom views for connection {}: found {}",
        connection_id,
        connection.custom_views.len()
    );
    for view in connection.custom_views.iter() {
        log::debug!("Adding custom view node: {}", view.name);
        let mut view_node =
            models::structs::TreeNode::new(view.name.clone(), models::enums::NodeType::CustomView);
        view_node.connection_id = Some(connection_id);
        // Store index in generic_id or similar if needed, or just use name for query lookup
        view_node.query = Some(view.query.clone());
        view_node.is_loaded = true;
        dba_children.push(view_node);
    }

    dba_folder.children = dba_children;
    main_children.push(dba_folder);

    node.children = main_children;
}

/// Fetch all FK constraints across all non-system schemas.
pub(crate) async fn fetch_postgres_foreign_keys(
    pool: &PgPool,
) -> Result<Vec<models::structs::ForeignKey>, sqlx::Error> {
    let query = r#"
        SELECT
            tc.constraint_name,
            kcu.table_name,
            kcu.column_name,
            ccu.table_name  AS referenced_table_name,
            ccu.column_name AS referenced_column_name
        FROM information_schema.table_constraints AS tc
        JOIN information_schema.key_column_usage AS kcu
            ON tc.constraint_name = kcu.constraint_name
           AND tc.table_schema   = kcu.table_schema
        JOIN information_schema.constraint_column_usage AS ccu
            ON ccu.constraint_name = tc.constraint_name
           AND ccu.table_schema    = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
          AND tc.table_schema NOT IN ('pg_catalog','information_schema')
        ORDER BY kcu.table_name, kcu.column_name
    "#;

    let rows = sqlx::query(query).fetch_all(pool).await?;
    let mut keys = Vec::new();
    for row in rows {
        keys.push(models::structs::ForeignKey {
            constraint_name: row
                .try_get::<String, _>("constraint_name")
                .unwrap_or_default(),
            table_name: row.try_get::<String, _>("table_name").unwrap_or_default(),
            column_name: row.try_get::<String, _>("column_name").unwrap_or_default(),
            referenced_table_name: row
                .try_get::<String, _>("referenced_table_name")
                .unwrap_or_default(),
            referenced_column_name: row
                .try_get::<String, _>("referenced_column_name")
                .unwrap_or_default(),
        });
    }
    Ok(keys)
}

/// Fetch all columns for every user table: table_name → [kolom + tipe/PK/nullable]
pub(crate) async fn fetch_postgres_columns(
    pool: &PgPool,
) -> Result<std::collections::HashMap<String, Vec<models::structs::DiagramColumn>>, sqlx::Error> {
    let query = r#"
        SELECT c.table_name::text AS table_name,
               c.column_name::text AS column_name,
               c.udt_name::text AS type_name,
               (c.is_nullable = 'YES') AS nullable,
               EXISTS (
                   SELECT 1
                   FROM information_schema.table_constraints tc
                   JOIN information_schema.key_column_usage k
                     ON k.constraint_name = tc.constraint_name
                    AND k.table_schema = tc.table_schema
                    AND k.table_name = tc.table_name
                   WHERE tc.constraint_type = 'PRIMARY KEY'
                     AND tc.table_schema = c.table_schema
                     AND tc.table_name = c.table_name
                     AND k.column_name = c.column_name
               ) AS is_pk
        FROM information_schema.columns c
        WHERE c.table_schema NOT IN ('pg_catalog','information_schema')
        ORDER BY c.table_name, c.ordinal_position
    "#;
    let rows = sqlx::query(query).fetch_all(pool).await?;
    let mut map: std::collections::HashMap<String, Vec<models::structs::DiagramColumn>> =
        std::collections::HashMap::new();
    for row in rows {
        let tbl: String = row.try_get("table_name").unwrap_or_default();
        map.entry(tbl)
            .or_default()
            .push(models::structs::DiagramColumn {
                name: row.try_get("column_name").unwrap_or_default(),
                type_name: row.try_get("type_name").unwrap_or_default(),
                nullable: row.try_get("nullable").unwrap_or(true),
                is_pk: row.try_get("is_pk").unwrap_or(false),
            });
    }
    Ok(map)
}

// Fetch tables/views from a PostgreSQL database (schema: public)
pub(crate) fn fetch_tables_from_postgres_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    let db = database_name.to_string();

    rt.block_on(async {
              let conn = tabular.connections.iter().find(|c| c.id == Some(connection_id))?.clone();
              let conn_str = format!(
                     "postgresql://{}:{}@{}:{}/{}",
                     conn.username, conn.password, conn.host, conn.port, db
              );

        let pool = match PgPoolOptions::new()
                     .max_connections(1)
            .acquire_timeout(std::time::Duration::from_secs(10))
                     .connect(&conn_str)
                     .await
              {
                     Ok(p) => p,
                     Err(_) => return None,
              };

              let sql = match table_type {
                     "table" => "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY table_name",
                     "view" => "SELECT table_name FROM information_schema.views WHERE table_schema = 'public' ORDER BY table_name",
                     _ => return None,
              };

        match tokio::time::timeout(
              std::time::Duration::from_secs(10),
              sqlx::query_as::<_, (String,)>(sql).fetch_all(&pool),
        )
        .await
        .map_err(|_| sqlx::Error::PoolTimedOut)
        .and_then(|r| r)
        {
                     Ok(rows) => Some(rows.into_iter().map(|(n,)| n).collect()),
                     Err(_) => None,
              }
       })
}

/// Mengubah satu nilai PostgreSQL menjadi teks tampilan.
///
/// sqlx mengecek kompatibilitas tipe secara ketat (kolom `INT4` tidak bisa dibaca
/// sebagai `i64`, `NUMERIC` tidak bisa sebagai `String`), jadi setiap keluarga tipe
/// di-decode dengan tipe Rust yang sesuai. Tipe yang tidak dikenal memakai byte
/// mentah dari protokol.
fn pg_value_to_string(row: &sqlx::postgres::PgRow, idx: usize) -> String {
    use sqlx::{Column, TypeInfo, ValueRef};

    fn show<T: ToString>(v: Result<Option<T>, sqlx::Error>) -> Option<String> {
        v.ok().map(|o| {
            o.map(|x| x.to_string())
                .unwrap_or_else(|| "NULL".to_string())
        })
    }
    fn show_array<T: ToString>(v: Result<Option<Vec<Option<T>>>, sqlx::Error>) -> Option<String> {
        v.ok().map(|o| match o {
            None => "NULL".to_string(),
            Some(items) => format!(
                "{{{}}}",
                items
                    .iter()
                    .map(|i| i
                        .as_ref()
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "NULL".to_string()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        })
    }

    match row.try_get_raw(idx) {
        Ok(raw) if raw.is_null() => return "NULL".to_string(),
        Err(e) => return format!("[error: {}]", e),
        Ok(_) => {}
    }

    let type_name = row.columns()[idx].type_info().name().to_ascii_uppercase();
    let decoded = match type_name.as_str() {
        "BOOL" => show(row.try_get::<Option<bool>, _>(idx)),
        "INT2" | "SMALLINT" | "SMALLSERIAL" => show(row.try_get::<Option<i16>, _>(idx)),
        "INT4" | "INT" | "SERIAL" => show(row.try_get::<Option<i32>, _>(idx)),
        "INT8" | "BIGINT" | "BIGSERIAL" => show(row.try_get::<Option<i64>, _>(idx)),
        "OID" => show(
            row.try_get::<Option<sqlx::postgres::types::Oid>, _>(idx)
                .map(|o| o.map(|v| v.0)),
        ),
        "FLOAT4" | "REAL" => show(row.try_get::<Option<f32>, _>(idx)),
        "FLOAT8" | "DOUBLE PRECISION" => show(row.try_get::<Option<f64>, _>(idx)),
        "NUMERIC" => show(row.try_get::<Option<rust_decimal::Decimal>, _>(idx)),
        "TIMESTAMP" => show(row.try_get::<Option<chrono::NaiveDateTime>, _>(idx)),
        "TIMESTAMPTZ" => show(row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(idx)),
        "DATE" => show(row.try_get::<Option<chrono::NaiveDate>, _>(idx)),
        "TIME" => show(row.try_get::<Option<chrono::NaiveTime>, _>(idx)),
        "JSON" | "JSONB" => show(row.try_get::<Option<sqlx::types::JsonValue>, _>(idx)),
        "BYTEA" => row
            .try_get::<Option<Vec<u8>>, _>(idx)
            .ok()
            .map(|o| match o {
                None => "NULL".to_string(),
                Some(b) => format!("\\x{}", hex::encode(b)),
            }),
        "UUID" => row.try_get_raw(idx).ok().and_then(|raw| {
            let bytes = raw.as_bytes().ok()?;
            (bytes.len() == 16).then(|| {
                let h = hex::encode(bytes);
                format!(
                    "{}-{}-{}-{}-{}",
                    &h[0..8],
                    &h[8..12],
                    &h[12..16],
                    &h[16..20],
                    &h[20..32]
                )
            })
        }),
        "INT2[]" => show_array(row.try_get::<Option<Vec<Option<i16>>>, _>(idx)),
        "INT4[]" => show_array(row.try_get::<Option<Vec<Option<i32>>>, _>(idx)),
        "INT8[]" => show_array(row.try_get::<Option<Vec<Option<i64>>>, _>(idx)),
        "FLOAT8[]" => show_array(row.try_get::<Option<Vec<Option<f64>>>, _>(idx)),
        "BOOL[]" => show_array(row.try_get::<Option<Vec<Option<bool>>>, _>(idx)),
        "TEXT[]" | "VARCHAR[]" | "NAME[]" | "BPCHAR[]" => {
            show_array(row.try_get::<Option<Vec<Option<String>>>, _>(idx))
        }
        _ => None,
    };
    if let Some(text) = decoded {
        return text;
    }

    // Tipe mirip teks (TEXT, VARCHAR, NAME, CITEXT, enum, …) di-decode sebagai String.
    if let Ok(v) = row.try_get_unchecked::<Option<String>, _>(idx)
        && let Some(s) = v
    {
        return s;
    }
    match row
        .try_get_raw(idx)
        .ok()
        .and_then(|raw| raw.as_bytes().ok())
    {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) if s.chars().all(|c| !c.is_control() || c.is_whitespace()) => s.to_string(),
            _ => format!("\\x{}", hex::encode(bytes)),
        },
        None => format!("[unsupported {}]", type_name),
    }
}

/// Mengubah baris PostgreSQL menjadi string tampilan, dengan men-decode setiap
/// kolom memakai tipe aslinya (lihat [`pg_value_to_string`]).
pub(crate) fn convert_postgres_rows_to_table_data(
    rows: Vec<sqlx::postgres::PgRow>,
) -> Vec<Vec<String>> {
    rows.iter()
        .map(|row| {
            (0..row.len())
                .map(|idx| pg_value_to_string(row, idx))
                .collect()
        })
        .collect()
}
