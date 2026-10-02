//! Penyimpanan diagram (grup kustom, relasi virtual, posisi tabel, note, flow)
//! di tabel `diagram_by_tabular` milik database target. Tabel ini adalah
//! penyimpanan utama diagram; file JSON lokal hanya cache.
//!
//! Setiap baris punya kolom `revision` yang naik setiap kali disimpan.
//! Penyimpanan memakai compare-and-swap: bila revision di database sudah
//! berubah sejak terakhir dibaca, simpan gagal dengan
//! [`DiagramStoreError::Conflict`] dan pemanggil melakukan merge.
//!
//! Mendukung MySQL, PostgreSQL, SQLite, dan SQL Server.

use crate::models::{enums::DatabasePool, structs::DiagramState};
use log::{debug, info, warn};
use sqlx::Row;

pub const TABLE_NAME: &str = "diagram_by_tabular";
pub const DEFAULT_DIAGRAM_ID: &str = "default";

/// Batas waktu satu operasi baca/tulis diagram ke database target.
pub const DB_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Nama file cache JSON lokal diagram `(conn_id, db_name)` di folder
/// `{data_dir}/diagrams`. Dipakai GUI dan lapisan agent supaya keduanya
/// membaca file yang sama.
pub fn local_diagram_file_name(conn_id: i64, db_name: &str) -> String {
    format!("conn_{conn_id}_{}.json", safe_db_name(db_name))
}

/// Nama file base sinkronisasi: salinan diagram di database pada revision
/// terakhir yang diketahui klien ini (lihat `diagram_sync`).
pub fn base_diagram_file_name(conn_id: i64, db_name: &str) -> String {
    format!("conn_{conn_id}_{}.base.json", safe_db_name(db_name))
}

fn safe_db_name(db_name: &str) -> String {
    db_name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

/// Satu baris `diagram_by_tabular`.
#[derive(Clone, Debug)]
pub struct DiagramRecord {
    pub state: DiagramState,
    /// 0 untuk tabel lama yang belum punya kolom `revision`.
    pub revision: i64,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum DiagramStoreError {
    #[error("this database type cannot store diagrams")]
    Unsupported,
    #[error("diagram was changed in the database (revision {})", .0.revision)]
    Conflict(Box<DiagramRecord>),
    #[error("corrupt diagram data in database: {0}")]
    Corrupt(String),
    #[error("{0}")]
    Db(String),
}

fn db_err(context: &str) -> impl Fn(sqlx::Error) -> DiagramStoreError + '_ {
    move |e| DiagramStoreError::Db(format!("{context}: {e}"))
}

/// Keadaan tabel `diagram_by_tabular` di database target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableState {
    Missing,
    /// Tabel versi lama tanpa kolom `revision`.
    Legacy,
    Ready,
}

fn mysql_table(db_name: &str) -> String {
    if db_name.is_empty() {
        "`diagram_by_tabular`".to_string()
    } else {
        format!("`{}`.`diagram_by_tabular`", db_name.replace('`', "``"))
    }
}

/// Escape literal string untuk SQL Server (driver-nya tidak punya bind).
fn mssql_lit(s: &str) -> String {
    format!("N'{}'", s.replace('\'', "''"))
}

async fn table_state(pool: &DatabasePool, db_name: &str) -> Result<TableState, DiagramStoreError> {
    let columns: Vec<String> = match pool {
        DatabasePool::SQLite(p) => {
            sqlx::query("SELECT name FROM pragma_table_info('diagram_by_tabular')")
                .fetch_all(p.as_ref())
                .await
                .map_err(db_err("SQLite table check failed"))?
                .iter()
                .filter_map(|r| r.try_get::<String, _>("name").ok())
                .collect()
        }
        DatabasePool::PostgreSQL(p) => sqlx::query(
            "SELECT column_name::text AS name FROM information_schema.columns
             WHERE table_name = 'diagram_by_tabular'
               AND table_schema = ANY (current_schemas(false))",
        )
        .fetch_all(p.as_ref())
        .await
        .map_err(db_err("PostgreSQL table check failed"))?
        .iter()
        .filter_map(|r| r.try_get::<String, _>("name").ok())
        .collect(),
        DatabasePool::MySQL(p) => {
            let query = if db_name.is_empty() {
                "SELECT CAST(column_name AS CHAR) AS name FROM information_schema.columns
                 WHERE table_schema = DATABASE() AND table_name = 'diagram_by_tabular'"
            } else {
                "SELECT CAST(column_name AS CHAR) AS name FROM information_schema.columns
                 WHERE table_schema = ? AND table_name = 'diagram_by_tabular'"
            };
            let mut q = sqlx::query(query);
            if !db_name.is_empty() {
                q = q.bind(db_name);
            }
            q.fetch_all(p.as_ref())
                .await
                .map_err(db_err("MySQL table check failed"))?
                .iter()
                .filter_map(|r| r.try_get::<String, _>("name").ok())
                .collect()
        }
        DatabasePool::MsSQL(p) => {
            let query = "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'diagram_by_tabular'";
            let (_headers, rows) = crate::driver_mssql::execute_query(p.clone(), query)
                .await
                .map_err(|e| DiagramStoreError::Db(format!("MsSQL table check failed: {e}")))?;
            rows.into_iter()
                .filter_map(|r| r.into_iter().next())
                .collect()
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) | DatabasePool::Plugin(_) => {
            return Err(DiagramStoreError::Unsupported);
        }
    };
    Ok(if columns.is_empty() {
        TableState::Missing
    } else if columns.iter().any(|c| c.eq_ignore_ascii_case("revision")) {
        TableState::Ready
    } else {
        TableState::Legacy
    })
}

/// Periksa apakah tabel `diagram_by_tabular` sudah ada di database target.
pub async fn check_diagram_table_exists(
    pool: &DatabasePool,
    db_name: &str,
) -> Result<bool, String> {
    match table_state(pool, db_name).await {
        Ok(state) => Ok(state != TableState::Missing),
        Err(DiagramStoreError::Unsupported) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

/// Pastikan tabel `diagram_by_tabular` ada dan punya kolom `revision`.
/// Tabel versi lama dimigrasi dengan `ALTER TABLE`.
pub async fn ensure_diagram_table(
    pool: &DatabasePool,
    db_name: &str,
) -> Result<(), DiagramStoreError> {
    let state = table_state(pool, db_name).await?;
    if state == TableState::Ready {
        return Ok(());
    }
    let create = state == TableState::Missing;
    match pool {
        DatabasePool::SQLite(p) => {
            let ddl = if create {
                "CREATE TABLE IF NOT EXISTS diagram_by_tabular (
                    id TEXT PRIMARY KEY,
                    diagram_name TEXT,
                    data TEXT NOT NULL,
                    updated_at TEXT DEFAULT (datetime('now')),
                    updated_by TEXT,
                    revision INTEGER NOT NULL DEFAULT 0
                )"
            } else {
                "ALTER TABLE diagram_by_tabular ADD COLUMN revision INTEGER NOT NULL DEFAULT 0"
            };
            sqlx::query(ddl)
                .execute(p.as_ref())
                .await
                .map_err(db_err("Failed to prepare SQLite diagram table"))?;
        }
        DatabasePool::PostgreSQL(p) => {
            let ddl = if create {
                "CREATE TABLE IF NOT EXISTS diagram_by_tabular (
                    id VARCHAR(64) PRIMARY KEY,
                    diagram_name VARCHAR(255),
                    data TEXT NOT NULL,
                    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
                    updated_by VARCHAR(100),
                    revision BIGINT NOT NULL DEFAULT 0
                )"
            } else {
                "ALTER TABLE diagram_by_tabular ADD COLUMN IF NOT EXISTS revision BIGINT NOT NULL DEFAULT 0"
            };
            sqlx::query(ddl)
                .execute(p.as_ref())
                .await
                .map_err(db_err("Failed to prepare PostgreSQL diagram table"))?;
        }
        DatabasePool::MySQL(p) => {
            let table = mysql_table(db_name);
            let ddl = if create {
                format!(
                    "CREATE TABLE IF NOT EXISTS {table} (
                        id VARCHAR(64) NOT NULL PRIMARY KEY,
                        diagram_name VARCHAR(255) NULL,
                        data LONGTEXT NOT NULL,
                        updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
                        updated_by VARCHAR(100) NULL,
                        revision BIGINT NOT NULL DEFAULT 0
                    ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
                )
            } else {
                format!("ALTER TABLE {table} ADD COLUMN revision BIGINT NOT NULL DEFAULT 0")
            };
            sqlx::query(sqlx::AssertSqlSafe(ddl.as_str()))
                .execute(p.as_ref())
                .await
                .map_err(db_err("Failed to prepare MySQL diagram table"))?;
        }
        DatabasePool::MsSQL(p) => {
            let ddl = if create {
                "IF NOT EXISTS (SELECT * FROM sys.tables WHERE name = 'diagram_by_tabular')
                 BEGIN
                    CREATE TABLE diagram_by_tabular (
                        id VARCHAR(64) PRIMARY KEY,
                        diagram_name NVARCHAR(255),
                        data NVARCHAR(MAX) NOT NULL,
                        updated_at DATETIME2 DEFAULT CURRENT_TIMESTAMP,
                        updated_by NVARCHAR(100),
                        revision BIGINT NOT NULL DEFAULT 0
                    );
                 END"
            } else {
                "IF COL_LENGTH('diagram_by_tabular', 'revision') IS NULL
                    ALTER TABLE diagram_by_tabular ADD revision BIGINT NOT NULL DEFAULT 0"
            };
            crate::driver_mssql::execute_query(p.clone(), ddl)
                .await
                .map_err(|e| {
                    DiagramStoreError::Db(format!("Failed to prepare MsSQL diagram table: {e}"))
                })?;
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) | DatabasePool::Plugin(_) => {
            return Err(DiagramStoreError::Unsupported);
        }
    }
    info!(
        "[DIAGRAM_DB] diagram_by_tabular {} in '{db_name}'",
        if create {
            "created"
        } else {
            "migrated (revision column)"
        }
    );
    Ok(())
}

/// Muat baris diagram dari `diagram_by_tabular`. `None` bila tabel atau
/// barisnya belum ada.
pub async fn load_diagram_record(
    pool: &DatabasePool,
    db_name: &str,
) -> Result<Option<DiagramRecord>, DiagramStoreError> {
    let id = DEFAULT_DIAGRAM_ID;
    let rev = match table_state(pool, db_name).await? {
        TableState::Missing => return Ok(None),
        TableState::Legacy => "0",
        TableState::Ready => "revision",
    };

    type Raw = (String, i64, Option<String>, Option<String>);
    let raw: Option<Raw> = match pool {
        DatabasePool::SQLite(p) => {
            let q = format!(
                "SELECT data, {rev} AS revision, CAST(updated_at AS TEXT) AS updated_at, updated_by
                 FROM diagram_by_tabular WHERE id = ?1"
            );
            sqlx::query(sqlx::AssertSqlSafe(q.as_str()))
                .bind(id)
                .fetch_optional(p.as_ref())
                .await
                .map_err(db_err("Failed to fetch SQLite diagram"))?
                .map(|r| {
                    (
                        r.try_get("data").unwrap_or_default(),
                        r.try_get("revision").unwrap_or(0),
                        r.try_get("updated_at").ok().flatten(),
                        r.try_get("updated_by").ok().flatten(),
                    )
                })
        }
        DatabasePool::PostgreSQL(p) => {
            let q = format!(
                "SELECT data, CAST({rev} AS BIGINT) AS revision,
                        to_char(updated_at, 'YYYY-MM-DD HH24:MI:SS') AS updated_at,
                        updated_by
                 FROM diagram_by_tabular WHERE id = $1"
            );
            sqlx::query(sqlx::AssertSqlSafe(q.as_str()))
                .bind(id)
                .fetch_optional(p.as_ref())
                .await
                .map_err(db_err("Failed to fetch PostgreSQL diagram"))?
                .map(|r| {
                    (
                        r.try_get("data").unwrap_or_default(),
                        r.try_get("revision").unwrap_or(0),
                        r.try_get("updated_at").ok().flatten(),
                        r.try_get("updated_by").ok().flatten(),
                    )
                })
        }
        DatabasePool::MySQL(p) => {
            let q = format!(
                "SELECT data, CAST({rev} AS SIGNED) AS revision,
                        DATE_FORMAT(updated_at, '%Y-%m-%d %H:%i:%s') AS updated_at,
                        updated_by
                 FROM {} WHERE id = ?",
                mysql_table(db_name)
            );
            sqlx::query(sqlx::AssertSqlSafe(q.as_str()))
                .bind(id)
                .fetch_optional(p.as_ref())
                .await
                .map_err(db_err("Failed to fetch MySQL diagram"))?
                .map(|r| {
                    (
                        r.try_get("data").unwrap_or_default(),
                        r.try_get("revision").unwrap_or(0),
                        r.try_get("updated_at").ok().flatten(),
                        r.try_get("updated_by").ok().flatten(),
                    )
                })
        }
        DatabasePool::MsSQL(p) => {
            let q = format!(
                "SELECT data, {rev} AS revision, CONVERT(VARCHAR(19), updated_at, 120) AS updated_at,
                        updated_by
                 FROM diagram_by_tabular WHERE id = {}",
                mssql_lit(id)
            );
            let (_headers, rows) = crate::driver_mssql::execute_query(p.clone(), &q)
                .await
                .map_err(|e| {
                    DiagramStoreError::Db(format!("Failed to fetch MsSQL diagram: {e}"))
                })?;
            let text = |s: Option<&String>| {
                s.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("NULL"))
                    .cloned()
            };
            rows.first().map(|r| {
                (
                    r.first().cloned().unwrap_or_default(),
                    r.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
                    text(r.get(2)),
                    text(r.get(3)),
                )
            })
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) | DatabasePool::Plugin(_) => {
            return Err(DiagramStoreError::Unsupported);
        }
    };

    let Some((data, revision, updated_at, updated_by)) = raw else {
        return Ok(None);
    };
    if data.is_empty() {
        return Ok(None);
    }
    let state = serde_json::from_str::<DiagramState>(&data).map_err(|e| {
        warn!("[DIAGRAM_DB] Failed to deserialize diagram from database: {e}");
        DiagramStoreError::Corrupt(e.to_string())
    })?;
    debug!("[DIAGRAM_DB] loaded diagram of '{db_name}' at revision {revision}");
    Ok(Some(DiagramRecord {
        state,
        revision,
        updated_at,
        updated_by,
    }))
}

/// Muat diagram dari `diagram_by_tabular` tanpa metadata revision.
pub async fn load_diagram_from_database(
    pool: &DatabasePool,
    db_name: &str,
) -> Result<Option<DiagramState>, String> {
    match load_diagram_record(pool, db_name).await {
        Ok(rec) => Ok(rec.map(|r| r.state)),
        Err(DiagramStoreError::Unsupported) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Simpan diagram ke `diagram_by_tabular` dengan compare-and-swap.
///
/// `expected_revision` adalah revision database yang menjadi dasar
/// perubahan lokal (`None` = belum pernah membaca dari database). Bila
/// revision di database berbeda, hasilnya [`DiagramStoreError::Conflict`]
/// berisi versi database. Sukses mengembalikan revision baru.
pub async fn save_diagram_record(
    pool: &DatabasePool,
    db_name: &str,
    state: &DiagramState,
    expected_revision: Option<i64>,
    updated_by: &str,
) -> Result<i64, DiagramStoreError> {
    ensure_diagram_table(pool, db_name).await?;
    let json = serde_json::to_string(state)
        .map_err(|e| DiagramStoreError::Db(format!("Failed to serialize diagram: {e}")))?;
    let updated_by: String = updated_by.chars().take(100).collect();

    if let Some(rev) = expected_revision
        && update_if_revision(pool, db_name, &json, rev, &updated_by).await?
    {
        debug!(
            "[DIAGRAM_DB] saved diagram of '{db_name}' at revision {}",
            rev + 1
        );
        return Ok(rev + 1);
    }
    // Update tidak mengenai baris: baris belum ada, atau sudah diubah orang lain.
    if let Some(remote) = load_diagram_record(pool, db_name).await? {
        return Err(DiagramStoreError::Conflict(Box::new(remote)));
    }
    insert_record(pool, db_name, &json, &updated_by).await?;
    debug!("[DIAGRAM_DB] created diagram of '{db_name}' at revision 1");
    Ok(1)
}

async fn update_if_revision(
    pool: &DatabasePool,
    db_name: &str,
    json: &str,
    rev: i64,
    updated_by: &str,
) -> Result<bool, DiagramStoreError> {
    let id = DEFAULT_DIAGRAM_ID;
    let affected = match pool {
        DatabasePool::SQLite(p) => sqlx::query(
            "UPDATE diagram_by_tabular
             SET diagram_name = ?1, data = ?2, updated_at = datetime('now'),
                 updated_by = ?3, revision = revision + 1
             WHERE id = ?4 AND revision = ?5",
        )
        .bind(db_name)
        .bind(json)
        .bind(updated_by)
        .bind(id)
        .bind(rev)
        .execute(p.as_ref())
        .await
        .map_err(db_err("Failed to update SQLite diagram"))?
        .rows_affected(),
        DatabasePool::PostgreSQL(p) => sqlx::query(
            "UPDATE diagram_by_tabular
             SET diagram_name = $1, data = $2, updated_at = CURRENT_TIMESTAMP,
                 updated_by = $3, revision = revision + 1
             WHERE id = $4 AND revision = $5",
        )
        .bind(db_name)
        .bind(json)
        .bind(updated_by)
        .bind(id)
        .bind(rev)
        .execute(p.as_ref())
        .await
        .map_err(db_err("Failed to update PostgreSQL diagram"))?
        .rows_affected(),
        DatabasePool::MySQL(p) => {
            let q = format!(
                "UPDATE {}
                 SET diagram_name = ?, data = ?, updated_at = CURRENT_TIMESTAMP,
                     updated_by = ?, revision = revision + 1
                 WHERE id = ? AND revision = ?",
                mysql_table(db_name)
            );
            sqlx::query(sqlx::AssertSqlSafe(q.as_str()))
                .bind(db_name)
                .bind(json)
                .bind(updated_by)
                .bind(id)
                .bind(rev)
                .execute(p.as_ref())
                .await
                .map_err(db_err("Failed to update MySQL diagram"))?
                .rows_affected()
        }
        DatabasePool::MsSQL(p) => {
            let q = format!(
                "UPDATE diagram_by_tabular
                 SET diagram_name = {}, data = {}, updated_at = CURRENT_TIMESTAMP,
                     updated_by = {}, revision = revision + 1
                 OUTPUT inserted.revision
                 WHERE id = {} AND revision = {rev}",
                mssql_lit(db_name),
                mssql_lit(json),
                mssql_lit(updated_by),
                mssql_lit(id),
            );
            let (_headers, rows) = crate::driver_mssql::execute_query(p.clone(), &q)
                .await
                .map_err(|e| {
                    DiagramStoreError::Db(format!("Failed to update MsSQL diagram: {e}"))
                })?;
            rows.len() as u64
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) | DatabasePool::Plugin(_) => {
            return Err(DiagramStoreError::Unsupported);
        }
    };
    Ok(affected > 0)
}

async fn insert_record(
    pool: &DatabasePool,
    db_name: &str,
    json: &str,
    updated_by: &str,
) -> Result<(), DiagramStoreError> {
    let id = DEFAULT_DIAGRAM_ID;
    match pool {
        DatabasePool::SQLite(p) => {
            sqlx::query(
                "INSERT INTO diagram_by_tabular (id, diagram_name, data, updated_at, updated_by, revision)
                 VALUES (?1, ?2, ?3, datetime('now'), ?4, 1)",
            )
            .bind(id)
            .bind(db_name)
            .bind(json)
            .bind(updated_by)
            .execute(p.as_ref())
            .await
            .map_err(db_err("Failed to insert SQLite diagram"))?;
        }
        DatabasePool::PostgreSQL(p) => {
            sqlx::query(
                "INSERT INTO diagram_by_tabular (id, diagram_name, data, updated_at, updated_by, revision)
                 VALUES ($1, $2, $3, CURRENT_TIMESTAMP, $4, 1)",
            )
            .bind(id)
            .bind(db_name)
            .bind(json)
            .bind(updated_by)
            .execute(p.as_ref())
            .await
            .map_err(db_err("Failed to insert PostgreSQL diagram"))?;
        }
        DatabasePool::MySQL(p) => {
            let q = format!(
                "INSERT INTO {} (id, diagram_name, data, updated_at, updated_by, revision)
                 VALUES (?, ?, ?, CURRENT_TIMESTAMP, ?, 1)",
                mysql_table(db_name)
            );
            sqlx::query(sqlx::AssertSqlSafe(q.as_str()))
                .bind(id)
                .bind(db_name)
                .bind(json)
                .bind(updated_by)
                .execute(p.as_ref())
                .await
                .map_err(db_err("Failed to insert MySQL diagram"))?;
        }
        DatabasePool::MsSQL(p) => {
            let q = format!(
                "INSERT INTO diagram_by_tabular (id, diagram_name, data, updated_at, updated_by, revision)
                 VALUES ({}, {}, {}, CURRENT_TIMESTAMP, {}, 1)",
                mssql_lit(id),
                mssql_lit(db_name),
                mssql_lit(json),
                mssql_lit(updated_by),
            );
            crate::driver_mssql::execute_query(p.clone(), &q)
                .await
                .map_err(|e| {
                    DiagramStoreError::Db(format!("Failed to insert MsSQL diagram: {e}"))
                })?;
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) | DatabasePool::Plugin(_) => {
            return Err(DiagramStoreError::Unsupported);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramGroup, DiagramNode, RelationOrigin, VirtualRelation};
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::Arc;

    async fn memory_pool() -> DatabasePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .expect("Failed to create in-memory sqlite pool");
        DatabasePool::SQLite(Arc::new(pool))
    }

    fn titled(title: &str) -> DiagramState {
        DiagramState {
            diagram_title: Some(title.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_sqlite_diagram_storage_lifecycle() {
        let db_pool = memory_pool().await;

        // 1. Table shouldn't exist initially
        let exists = check_diagram_table_exists(&db_pool, "main").await.unwrap();
        assert!(
            !exists,
            "Table diagram_by_tabular should not exist initially"
        );

        // 2. Load returns None when table doesn't exist
        let loaded = load_diagram_record(&db_pool, "main").await.unwrap();
        assert!(loaded.is_none());

        // 3. Prepare dummy state with custom groups & virtual relations
        let mut state = DiagramState::default();
        state.groups.push(DiagramGroup {
            id: "group_auth".to_string(),
            title: "Authentication".to_string(),
            color: eframe::egui::Color32::from_rgb(100, 150, 200),
            manual_pos: None,
            repo_url: None,
        });
        state.virtual_relations.push(VirtualRelation {
            child: "audit_logs".to_string(),
            child_column: "user_uuid".to_string(),
            parent: "users".to_string(),
            parent_column: "uuid".to_string(),
            origin: RelationOrigin::Manual,
        });
        state.nodes.push(DiagramNode {
            id: "users".to_string(),
            title: "users".to_string(),
            pos: eframe::egui::pos2(120.0, 240.0),
            size: eframe::egui::vec2(200.0, 150.0),
            columns: vec!["uuid".to_string(), "email".to_string()],
            foreign_keys: vec![],
            group_ids: vec!["group_auth".to_string()],
            group_id: Some("group_auth".to_string()),
            column_meta: vec![],
            detached: false,
            database_name: Some("main".to_string()),
            connection_id: Some(1),
            connection_name: Some("Local SQLite".to_string()),
        });

        // 4. Save to database (first save creates revision 1)
        let rev = save_diagram_record(&db_pool, "main", &state, None, "tester")
            .await
            .expect("Saving diagram to SQLite should succeed");
        assert_eq!(rev, 1);

        // 5. Table should exist now
        let exists_after = check_diagram_table_exists(&db_pool, "main").await.unwrap();
        assert!(
            exists_after,
            "Table diagram_by_tabular should exist after save"
        );

        // 6. Load from database and verify state
        let record = load_diagram_record(&db_pool, "main")
            .await
            .expect("Loading diagram from SQLite should succeed")
            .expect("Diagram should be found");
        assert_eq!(record.revision, 1);
        assert_eq!(record.updated_by.as_deref(), Some("tester"));
        assert!(record.updated_at.is_some());
        let loaded_state = record.state;

        assert_eq!(loaded_state.groups.len(), 1);
        assert_eq!(loaded_state.groups[0].title, "Authentication");
        assert_eq!(loaded_state.virtual_relations.len(), 1);
        assert_eq!(loaded_state.virtual_relations[0].child, "audit_logs");
        assert_eq!(loaded_state.virtual_relations[0].child_column, "user_uuid");
        assert_eq!(loaded_state.nodes.len(), 1);
        assert_eq!(loaded_state.nodes[0].pos, eframe::egui::pos2(120.0, 240.0));
        assert_eq!(
            loaded_state.nodes[0].group_ids,
            vec!["group_auth".to_string()]
        );
    }

    #[tokio::test]
    async fn stale_revision_is_rejected_with_remote_copy() {
        let pool = memory_pool().await;
        let r1 = save_diagram_record(&pool, "main", &titled("a"), None, "alice")
            .await
            .unwrap();
        // Bob menyimpan di atas revision 1.
        let r2 = save_diagram_record(&pool, "main", &titled("b"), Some(r1), "bob")
            .await
            .unwrap();
        assert_eq!(r2, 2);

        // Alice masih memegang revision 1: harus konflik, bukan menimpa.
        let err = save_diagram_record(&pool, "main", &titled("c"), Some(r1), "alice")
            .await
            .unwrap_err();
        match err {
            DiagramStoreError::Conflict(remote) => {
                assert_eq!(remote.revision, 2);
                assert_eq!(remote.state.diagram_title.as_deref(), Some("b"));
                assert_eq!(remote.updated_by.as_deref(), Some("bob"));
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        // Tanpa base sama sekali juga konflik bila baris sudah ada.
        assert!(matches!(
            save_diagram_record(&pool, "main", &titled("d"), None, "carol").await,
            Err(DiagramStoreError::Conflict(_))
        ));
        let rec = load_diagram_record(&pool, "main").await.unwrap().unwrap();
        assert_eq!(rec.state.diagram_title.as_deref(), Some("b"));
    }

    #[tokio::test]
    async fn legacy_table_without_revision_is_migrated() {
        let pool = memory_pool().await;
        let DatabasePool::SQLite(p) = &pool else {
            unreachable!()
        };
        sqlx::query(
            "CREATE TABLE diagram_by_tabular (
                id TEXT PRIMARY KEY, diagram_name TEXT, data TEXT NOT NULL,
                updated_at TEXT DEFAULT (datetime('now')), updated_by TEXT)",
        )
        .execute(p.as_ref())
        .await
        .unwrap();
        let json = serde_json::to_string(&titled("old")).unwrap();
        sqlx::query("INSERT INTO diagram_by_tabular (id, diagram_name, data) VALUES ('default', 'main', ?1)")
            .bind(&json)
            .execute(p.as_ref())
            .await
            .unwrap();

        // Tabel lama terbaca dengan revision 0.
        let rec = load_diagram_record(&pool, "main").await.unwrap().unwrap();
        assert_eq!(rec.revision, 0);
        assert_eq!(rec.state.diagram_title.as_deref(), Some("old"));

        // Simpan di atas revision 0 memigrasi tabel lalu naik ke revision 1.
        let rev = save_diagram_record(&pool, "main", &titled("new"), Some(0), "me")
            .await
            .unwrap();
        assert_eq!(rev, 1);
        let rec = load_diagram_record(&pool, "main").await.unwrap().unwrap();
        assert_eq!(rec.revision, 1);
        assert_eq!(rec.state.diagram_title.as_deref(), Some("new"));
    }
}
