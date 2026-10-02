//! Eksekutor headless: jalankan SQL di database tertentu dan kembalikan semua
//! result set sebagai teks. Dipakai dialog objek skema, sidebar, dan structure
//! editor. Tidak menyentuh state GUI.

use crate::connection::pool::create_connection_pool_for_config;
use crate::connection::split_sql_statements;
use crate::connection::sql::{is_comment_only_statement, split_mssql_go_batches};
use crate::models::enums::{DatabasePool, DatabaseType};
use crate::models::structs::ConnectionConfig;
use sqlx::{Column, Row};

/// Satu result set dalam bentuk teks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultSet {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl ResultSet {
    /// Nilai sel pertama, atau `None` bila kosong / NULL.
    pub fn first_value(&self) -> Option<&str> {
        self.rows
            .first()
            .and_then(|r| r.first())
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty() && *s != "NULL")
    }

    /// Nilai kolom `header` di baris pertama (case-insensitive), fallback ke
    /// kolom pertama.
    pub fn value_by_header(&self, header: Option<&str>) -> Option<&str> {
        let row = self.rows.first()?;
        let idx = header
            .and_then(|h| {
                self.headers
                    .iter()
                    .position(|name| name.eq_ignore_ascii_case(h))
            })
            .unwrap_or(0);
        row.get(idx)
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty() && *s != "NULL")
    }

    /// Kolom pertama semua baris (untuk query daftar nama).
    pub fn first_column(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|r| r.first().cloned())
            .filter(|s| !s.is_empty() && s != "NULL")
            .collect()
    }
}

/// Pecah script menjadi unit eksekusi sesuai engine. MsSQL dikirim per batch
/// (dipisah `GO`) karena `IF ... ELSE` dan body routine tidak boleh dipecah di `;`.
pub fn split_for_engine(db: &DatabaseType, sql: &str) -> Vec<String> {
    match db {
        DatabaseType::MsSQL => split_mssql_go_batches(sql)
            .unwrap_or_else(|| vec![sql.trim().to_string()])
            .into_iter()
            .filter(|s| !s.trim().is_empty() && !is_comment_only_statement(s))
            .collect(),
        _ => split_sql_statements(sql, matches!(db, DatabaseType::MySQL))
            .into_iter()
            .filter(|s| !is_comment_only_statement(s))
            .collect(),
    }
}

/// Jalankan `sql` (boleh multi-statement) di `database`. `pool` adalah pool
/// utama koneksi bila sudah ada; untuk PostgreSQL dengan database lain dibuat
/// pool sementara (SSH/TLS tetap dipakai lewat konfigurasi koneksi).
pub async fn run_in_database(
    conn: &ConnectionConfig,
    pool: Option<DatabasePool>,
    database: Option<&str>,
    sql: &str,
) -> Result<Vec<ResultSet>, String> {
    let statements = split_for_engine(&conn.connection_type, sql);
    run_statements_in_database(conn, pool, database, &statements).await
}

/// Seperti [`run_in_database`], tetapi `statements` sudah berupa unit eksekusi
/// dan dikirim apa adanya tanpa dipecah lagi. Dipakai transfer/impor data yang
/// menyusun `INSERT` sendiri: isi sel tidak boleh ikut ditafsirkan pemecah
/// statement.
pub async fn run_statements_in_database(
    conn: &ConnectionConfig,
    pool: Option<DatabasePool>,
    database: Option<&str>,
    statements: &[String],
) -> Result<Vec<ResultSet>, String> {
    let database = database.map(str::trim).filter(|d| !d.is_empty());
    if statements.is_empty() {
        return Err("Nothing to execute".to_string());
    }

    // PostgreSQL: pool terikat ke satu database.
    if conn.connection_type == DatabaseType::PostgreSQL
        && let Some(db) = database
        && db != conn.database
    {
        let mut cfg = conn.clone();
        cfg.database = db.to_string();
        let temp = create_connection_pool_for_config(&cfg).await?;
        let result = run_statements(&temp, None, statements).await;
        if let DatabasePool::PostgreSQL(p) = &temp {
            p.close().await;
        }
        return result;
    }

    let pool = match pool {
        Some(p) => p,
        None => create_connection_pool_for_config(conn).await?,
    };
    run_statements(&pool, database, statements).await
}

async fn run_statements(
    pool: &DatabasePool,
    database: Option<&str>,
    statements: &[String],
) -> Result<Vec<ResultSet>, String> {
    let mut sets = Vec::new();
    match pool {
        DatabasePool::MySQL(p) => {
            let mut c = p.acquire().await.map_err(|e| e.to_string())?;
            if let Some(db) = database {
                let use_stmt = format!("USE `{}`", db.replace('`', "``"));
                // `USE` ditolak protokol prepared statement MySQL (error 1295),
                // jadi dikirim lewat protokol teks.
                sqlx::raw_sql(sqlx::AssertSqlSafe(use_stmt))
                    .execute(&mut *c)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            for stmt in statements {
                let rows = sqlx::query(sqlx::AssertSqlSafe(stmt.as_str()))
                    .fetch_all(&mut *c)
                    .await
                    .map_err(|e| e.to_string())?;
                let headers = rows
                    .first()
                    .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                    .unwrap_or_default();
                sets.push(ResultSet {
                    headers,
                    rows: crate::driver_mysql::convert_mysql_rows_to_table_data(rows),
                });
            }
        }
        DatabasePool::PostgreSQL(p) => {
            let mut c = p.acquire().await.map_err(|e| e.to_string())?;
            for stmt in statements {
                let rows = sqlx::query(sqlx::AssertSqlSafe(stmt.as_str()))
                    .fetch_all(&mut *c)
                    .await
                    .map_err(|e| e.to_string())?;
                let headers = rows
                    .first()
                    .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                    .unwrap_or_default();
                sets.push(ResultSet {
                    headers,
                    rows: crate::driver_postgres::convert_postgres_rows_to_table_data(rows),
                });
            }
        }
        DatabasePool::SQLite(p) => {
            let mut c = p.acquire().await.map_err(|e| e.to_string())?;
            for stmt in statements {
                let rows = sqlx::query(sqlx::AssertSqlSafe(stmt.as_str()))
                    .fetch_all(&mut *c)
                    .await
                    .map_err(|e| e.to_string())?;
                let headers = rows
                    .first()
                    .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                    .unwrap_or_default();
                sets.push(ResultSet {
                    headers,
                    rows: crate::driver_sqlite::convert_sqlite_rows_to_table_data(rows),
                });
            }
        }
        DatabasePool::MsSQL(p) => {
            let mut c = p.get().await.map_err(|e| e.to_string())?;
            let client = c
                .client_mut()
                .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
            if let Some(db) = database {
                let use_sql = format!("USE [{}]", db.replace(']', "]]"));
                client
                    .simple_query(use_sql.as_str())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            for stmt in statements {
                for (headers, rows) in crate::driver_mssql::run_query_multi(client, stmt).await? {
                    sets.push(ResultSet { headers, rows });
                }
            }
        }
        _ => return Err("This connection type does not support SQL".to_string()),
    }
    Ok(sets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mssql_scripts_are_not_split_on_semicolons() {
        let sql = "IF 1 = 1\n  EXEC a;\nELSE\n  EXEC b;";
        assert_eq!(split_for_engine(&DatabaseType::MsSQL, sql).len(), 1);
        let sql = "SELECT 1;\nGO\nSELECT 2;";
        assert_eq!(split_for_engine(&DatabaseType::MsSQL, sql).len(), 2);
        assert_eq!(
            split_for_engine(&DatabaseType::PostgreSQL, "SELECT 1; SELECT 2;").len(),
            2
        );
    }

    #[test]
    fn result_set_helpers() {
        let set = ResultSet {
            headers: vec!["Trigger".into(), "SQL Original Statement".into()],
            rows: vec![vec!["t".into(), "CREATE TRIGGER t".into()]],
        };
        assert_eq!(
            set.value_by_header(Some("sql original statement")),
            Some("CREATE TRIGGER t")
        );
        assert_eq!(set.value_by_header(None), Some("t"));
        assert_eq!(set.first_column(), vec!["t".to_string()]);
        assert_eq!(ResultSet::default().first_value(), None);
    }

    #[tokio::test]
    async fn runs_statements_on_sqlite_pool() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let pool = DatabasePool::SQLite(std::sync::Arc::new(pool));
        let conn = ConnectionConfig {
            connection_type: DatabaseType::SQLite,
            ..Default::default()
        };
        let sets = run_in_database(
            &conn,
            Some(pool),
            None,
            "CREATE TABLE t (id INTEGER); INSERT INTO t VALUES (7); SELECT id FROM t;",
        )
        .await
        .unwrap();
        assert_eq!(sets.len(), 3);
        assert_eq!(sets[2].headers, vec!["id".to_string()]);
        assert_eq!(sets[2].first_value(), Some("7"));
    }
}
