//! Penyimpanan dan sinkronisasi metadata diagram (grup kustom, relasi virtual,
//! posisi tabel) ke dalam tabel `diagram_by_tabular` di database target.
//!
//! Mendukung MySQL, PostgreSQL, SQLite, dan SQL Server.

use crate::models::{enums::DatabasePool, structs::DiagramState};
use log::{debug, info, warn};
use sqlx::Row;

pub const TABLE_NAME: &str = "diagram_by_tabular";
pub const DEFAULT_DIAGRAM_ID: &str = "default";

/// Periksa apakah tabel `diagram_by_tabular` sudah ada di database target.
pub async fn check_diagram_table_exists(
    pool: &DatabasePool,
    db_name: &str,
) -> Result<bool, String> {
    match pool {
        DatabasePool::SQLite(p) => {
            let row = sqlx::query(
                "SELECT COUNT(*) AS cnt FROM sqlite_master WHERE type='table' AND name='diagram_by_tabular'",
            )
            .fetch_one(p.as_ref())
            .await
            .map_err(|e| format!("SQLite table check failed: {e}"))?;

            let count: i64 = row.try_get("cnt").unwrap_or(0);
            Ok(count > 0)
        }
        DatabasePool::PostgreSQL(p) => {
            let row = sqlx::query(
                "SELECT EXISTS (
                    SELECT 1 FROM information_schema.tables 
                    WHERE table_name = 'diagram_by_tabular'
                 ) AS table_exists",
            )
            .fetch_one(p.as_ref())
            .await
            .map_err(|e| format!("PostgreSQL table check failed: {e}"))?;

            let exists: bool = row.try_get("table_exists").unwrap_or(false);
            Ok(exists)
        }
        DatabasePool::MySQL(p) => {
            let query = if !db_name.is_empty() {
                "SELECT COUNT(*) AS cnt FROM information_schema.tables WHERE table_schema = ? AND table_name = 'diagram_by_tabular'"
            } else {
                "SELECT COUNT(*) AS cnt FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = 'diagram_by_tabular'"
            };

            let mut q = sqlx::query(query);
            if !db_name.is_empty() {
                q = q.bind(db_name);
            }

            let row = q
                .fetch_one(p.as_ref())
                .await
                .map_err(|e| format!("MySQL table check failed: {e}"))?;

            let count: i64 = row.try_get("cnt").unwrap_or(0);
            Ok(count > 0)
        }
        DatabasePool::MsSQL(p) => {
            let query = "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_NAME = 'diagram_by_tabular'";
            let (_headers, rows) = crate::driver_mssql::execute_query(p.clone(), query)
                .await
                .map_err(|e| format!("MsSQL table check failed: {e}"))?;

            let count: i64 = rows
                .first()
                .and_then(|r| r.first())
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            Ok(count > 0)
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) => {
            // NoSQL engines don't use relational diagram tables
            Ok(false)
        }
    }
}

/// Pastikan tabel `diagram_by_tabular` sudah dibuat di database target.
pub async fn ensure_diagram_table(pool: &DatabasePool, db_name: &str) -> Result<(), String> {
    match pool {
        DatabasePool::SQLite(p) => {
            let ddl = r#"
                CREATE TABLE IF NOT EXISTS diagram_by_tabular (
                    id TEXT PRIMARY KEY,
                    diagram_name TEXT,
                    data TEXT NOT NULL,
                    updated_at TEXT DEFAULT (datetime('now')),
                    updated_by TEXT
                );
            "#;
            sqlx::query(ddl)
                .execute(p.as_ref())
                .await
                .map_err(|e| format!("Failed to create SQLite diagram table: {e}"))?;
            info!("[DIAGRAM_DB] SQLite diagram_by_tabular ready");
            Ok(())
        }
        DatabasePool::PostgreSQL(p) => {
            let ddl = r#"
                CREATE TABLE IF NOT EXISTS diagram_by_tabular (
                    id VARCHAR(64) PRIMARY KEY,
                    diagram_name VARCHAR(255),
                    data TEXT NOT NULL,
                    updated_at TIMESTAMP WITH TIME ZONE DEFAULT CURRENT_TIMESTAMP,
                    updated_by VARCHAR(100)
                );
            "#;
            sqlx::query(ddl)
                .execute(p.as_ref())
                .await
                .map_err(|e| format!("Failed to create PostgreSQL diagram table: {e}"))?;
            info!("[DIAGRAM_DB] PostgreSQL diagram_by_tabular ready");
            Ok(())
        }
        DatabasePool::MySQL(p) => {
            let table_spec = if !db_name.is_empty() {
                format!("`{}`.`diagram_by_tabular`", db_name.replace('`', "``"))
            } else {
                "`diagram_by_tabular`".to_string()
            };

            let ddl = format!(
                r#"CREATE TABLE IF NOT EXISTS {} (
                    id VARCHAR(64) NOT NULL PRIMARY KEY,
                    diagram_name VARCHAR(255) NULL,
                    data LONGTEXT NOT NULL,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
                    updated_by VARCHAR(100) NULL
                ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;"#,
                table_spec
            );

            sqlx::query(sqlx::AssertSqlSafe(ddl.as_str()))
                .execute(p.as_ref())
                .await
                .map_err(|e| format!("Failed to create MySQL diagram table: {e}"))?;
            info!("[DIAGRAM_DB] MySQL diagram_by_tabular ready");
            Ok(())
        }
        DatabasePool::MsSQL(p) => {
            let ddl = r#"
                IF NOT EXISTS (SELECT * FROM sys.tables WHERE name = 'diagram_by_tabular')
                BEGIN
                    CREATE TABLE diagram_by_tabular (
                        id VARCHAR(64) PRIMARY KEY,
                        diagram_name NVARCHAR(255),
                        data NVARCHAR(MAX) NOT NULL,
                        updated_at DATETIME2 DEFAULT CURRENT_TIMESTAMP,
                        updated_by NVARCHAR(100)
                    );
                END
            "#;
            crate::driver_mssql::execute_query(p.clone(), ddl)
                .await
                .map_err(|e| format!("Failed to create MsSQL diagram table: {e}"))?;
            info!("[DIAGRAM_DB] MsSQL diagram_by_tabular ready");
            Ok(())
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) => {
            Err("Database type does not support relational diagram table storage".to_string())
        }
    }
}

/// Simpan diagram ke tabel `diagram_by_tabular` di database target.
pub async fn save_diagram_to_database(
    pool: &DatabasePool,
    db_name: &str,
    state: &DiagramState,
    diagram_id: Option<&str>,
    diagram_name: Option<&str>,
) -> Result<(), String> {
    // 1. Buat tabel bila belum ada
    ensure_diagram_table(pool, db_name).await?;

    let id = diagram_id.unwrap_or(DEFAULT_DIAGRAM_ID);
    let name = diagram_name.unwrap_or(db_name);
    let json_data = serde_json::to_string(state)
        .map_err(|e| format!("Failed to serialize diagram state: {e}"))?;
    let updated_by = "tabular-client";

    // 2. Lakukan Upsert berdasarkan tipe database
    match pool {
        DatabasePool::SQLite(p) => {
            let upsert = r#"
                INSERT INTO diagram_by_tabular (id, diagram_name, data, updated_at, updated_by)
                VALUES (?1, ?2, ?3, datetime('now'), ?4)
                ON CONFLICT(id) DO UPDATE SET
                    diagram_name = excluded.diagram_name,
                    data = excluded.data,
                    updated_at = datetime('now'),
                    updated_by = excluded.updated_by;
            "#;
            sqlx::query(upsert)
                .bind(id)
                .bind(name)
                .bind(&json_data)
                .bind(updated_by)
                .execute(p.as_ref())
                .await
                .map_err(|e| format!("Failed to upsert SQLite diagram: {e}"))?;
            debug!("[DIAGRAM_DB] Diagram saved to SQLite diagram_by_tabular (id='{id}')");
            Ok(())
        }
        DatabasePool::PostgreSQL(p) => {
            let upsert = r#"
                INSERT INTO diagram_by_tabular (id, diagram_name, data, updated_at, updated_by)
                VALUES ($1, $2, $3, CURRENT_TIMESTAMP, $4)
                ON CONFLICT (id) DO UPDATE SET
                    diagram_name = EXCLUDED.diagram_name,
                    data = EXCLUDED.data,
                    updated_at = CURRENT_TIMESTAMP,
                    updated_by = EXCLUDED.updated_by;
            "#;
            sqlx::query(upsert)
                .bind(id)
                .bind(name)
                .bind(&json_data)
                .bind(updated_by)
                .execute(p.as_ref())
                .await
                .map_err(|e| format!("Failed to upsert PostgreSQL diagram: {e}"))?;
            debug!("[DIAGRAM_DB] Diagram saved to PostgreSQL diagram_by_tabular (id='{id}')");
            Ok(())
        }
        DatabasePool::MySQL(p) => {
            let table_spec = if !db_name.is_empty() {
                format!("`{}`.`diagram_by_tabular`", db_name.replace('`', "``"))
            } else {
                "`diagram_by_tabular`".to_string()
            };

            let upsert = format!(
                r#"INSERT INTO {} (id, diagram_name, data, updated_at, updated_by)
                   VALUES (?, ?, ?, CURRENT_TIMESTAMP, ?)
                   ON DUPLICATE KEY UPDATE
                       diagram_name = VALUES(diagram_name),
                       data = VALUES(data),
                       updated_at = CURRENT_TIMESTAMP,
                       updated_by = VALUES(updated_by);"#,
                table_spec
            );

            sqlx::query(sqlx::AssertSqlSafe(upsert.as_str()))
                .bind(id)
                .bind(name)
                .bind(&json_data)
                .bind(updated_by)
                .execute(p.as_ref())
                .await
                .map_err(|e| format!("Failed to upsert MySQL diagram: {e}"))?;
            debug!("[DIAGRAM_DB] Diagram saved to MySQL diagram_by_tabular (id='{id}')");
            Ok(())
        }
        DatabasePool::MsSQL(p) => {
            let id_escaped = id.replace('\'', "''");
            let name_escaped = name.replace('\'', "''");
            let data_escaped = json_data.replace('\'', "''");
            let updated_by_escaped = updated_by.replace('\'', "''");

            let upsert = format!(
                r#"
                IF EXISTS (SELECT 1 FROM diagram_by_tabular WHERE id = '{id_escaped}')
                    UPDATE diagram_by_tabular
                    SET diagram_name = '{name_escaped}',
                        data = '{data_escaped}',
                        updated_at = CURRENT_TIMESTAMP,
                        updated_by = '{updated_by_escaped}'
                    WHERE id = '{id_escaped}';
                ELSE
                    INSERT INTO diagram_by_tabular (id, diagram_name, data, updated_at, updated_by)
                    VALUES ('{id_escaped}', '{name_escaped}', '{data_escaped}', CURRENT_TIMESTAMP, '{updated_by_escaped}');
                "#
            );

            crate::driver_mssql::execute_query(p.clone(), &upsert)
                .await
                .map_err(|e| format!("Failed to upsert MsSQL diagram: {e}"))?;
            debug!("[DIAGRAM_DB] Diagram saved to MsSQL diagram_by_tabular (id='{id}')");
            Ok(())
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) => {
            Err("Database type does not support relational diagram table storage".to_string())
        }
    }
}

/// Muat diagram dari tabel `diagram_by_tabular` di database target.
pub async fn load_diagram_from_database(
    pool: &DatabasePool,
    db_name: &str,
    diagram_id: Option<&str>,
) -> Result<Option<DiagramState>, String> {
    let id = diagram_id.unwrap_or(DEFAULT_DIAGRAM_ID);

    // Cek dulu apakah tabelnya ada sebelum menjalankan SELECT agar tidak memicu log error SQL
    let exists = check_diagram_table_exists(pool, db_name).await?;
    if !exists {
        return Ok(None);
    }

    let json_data_opt = match pool {
        DatabasePool::SQLite(p) => {
            let row_opt = sqlx::query("SELECT data FROM diagram_by_tabular WHERE id = ?1")
                .bind(id)
                .fetch_optional(p.as_ref())
                .await
                .map_err(|e| format!("Failed to fetch SQLite diagram: {e}"))?;

            row_opt.and_then(|r| r.try_get::<String, _>("data").ok())
        }
        DatabasePool::PostgreSQL(p) => {
            let row_opt = sqlx::query("SELECT data FROM diagram_by_tabular WHERE id = $1")
                .bind(id)
                .fetch_optional(p.as_ref())
                .await
                .map_err(|e| format!("Failed to fetch PostgreSQL diagram: {e}"))?;

            row_opt.and_then(|r| r.try_get::<String, _>("data").ok())
        }
        DatabasePool::MySQL(p) => {
            let table_spec = if !db_name.is_empty() {
                format!("`{}`.`diagram_by_tabular`", db_name.replace('`', "``"))
            } else {
                "`diagram_by_tabular`".to_string()
            };

            let query = format!("SELECT data FROM {} WHERE id = ?", table_spec);
            let row_opt = sqlx::query(sqlx::AssertSqlSafe(query.as_str()))
                .bind(id)
                .fetch_optional(p.as_ref())
                .await
                .map_err(|e| format!("Failed to fetch MySQL diagram: {e}"))?;

            row_opt.and_then(|r| r.try_get::<String, _>("data").ok())
        }
        DatabasePool::MsSQL(p) => {
            let id_escaped = id.replace('\'', "''");
            let query = format!("SELECT data FROM diagram_by_tabular WHERE id = '{id_escaped}'");
            let (_headers, rows) = crate::driver_mssql::execute_query(p.clone(), &query)
                .await
                .map_err(|e| format!("Failed to fetch MsSQL diagram: {e}"))?;

            rows.first()
                .and_then(|r| r.first())
                .filter(|s| !s.is_empty())
                .cloned()
        }
        DatabasePool::Redis(_) | DatabasePool::MongoDB(_) => None,
    };

    let Some(raw_json) = json_data_opt else {
        return Ok(None);
    };

    match serde_json::from_str::<DiagramState>(&raw_json) {
        Ok(state) => {
            info!(
                "[DIAGRAM_DB] Successfully loaded diagram from database table `diagram_by_tabular`"
            );
            Ok(Some(state))
        }
        Err(e) => {
            warn!("[DIAGRAM_DB] Failed to deserialize diagram from database: {e}");
            Err(format!("Corrupt diagram data in database: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramGroup, DiagramNode, RelationOrigin, VirtualRelation};
    use sqlx::sqlite::SqlitePoolOptions;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_sqlite_diagram_storage_lifecycle() {
        let pool = SqlitePoolOptions::new()
            .connect(":memory:")
            .await
            .expect("Failed to create in-memory sqlite pool");
        let db_pool = DatabasePool::SQLite(Arc::new(pool));

        // 1. Table shouldn't exist initially
        let exists = check_diagram_table_exists(&db_pool, "main").await.unwrap();
        assert!(
            !exists,
            "Table diagram_by_tabular should not exist initially"
        );

        // 2. Load returns None when table doesn't exist
        let loaded = load_diagram_from_database(&db_pool, "main", None)
            .await
            .unwrap();
        assert!(loaded.is_none());

        // 3. Prepare dummy state with custom groups & virtual relations
        let mut state = DiagramState::default();
        state.groups.push(DiagramGroup {
            id: "group_auth".to_string(),
            title: "Authentication".to_string(),
            color: eframe::egui::Color32::from_rgb(100, 150, 200),
            manual_pos: None,
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
        });

        // 4. Save to database
        save_diagram_to_database(&db_pool, "main", &state, None, Some("Main Schema"))
            .await
            .expect("Saving diagram to SQLite should succeed");

        // 5. Table should exist now
        let exists_after = check_diagram_table_exists(&db_pool, "main").await.unwrap();
        assert!(
            exists_after,
            "Table diagram_by_tabular should exist after save"
        );

        // 6. Load from database and verify state
        let loaded_state = load_diagram_from_database(&db_pool, "main", None)
            .await
            .expect("Loading diagram from SQLite should succeed")
            .expect("Diagram should be found");

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
}
