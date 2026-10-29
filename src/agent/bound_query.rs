//! Query baca dengan parameter terikat untuk langkah DB integration test dan
//! data check (`crate::http_test_db`).
//!
//! Nilai variabel `{{x}}` tidak pernah disambung ke teks SQL: pemanggil
//! mengubahnya menjadi placeholder (`?`, `$1`, `@P1`) lewat
//! [`crate::http_test_db::bind_vars`], lalu nilainya diikat di sini per driver.
//! Gerbangnya sama dengan `run_query`: hanya satu statement SELECT/WITH yang
//! diklasifikasi baca, dijalankan di transaksi read-only (MySQL/PostgreSQL)
//! atau `PRAGMA query_only` (SQLite), dengan batas baris dan timeout.

use std::time::{Duration, Instant};

use futures::TryStreamExt;

use super::core::{AgentError, AgentQueryResult, HeadlessSession, build_safety_report};
use crate::http_test_db::{BoundSql, SqlParam};
use crate::models::enums::{DatabasePool, DatabaseType};

/// Batas baris keras untuk query terikat (langkah DB dan data check).
pub const BOUND_MAX_ROWS: usize = 1000;

/// Kata kunci pertama statement setelah komentar dan kurung pembuka.
fn first_keyword(sql: &str) -> String {
    let mut rest = sql.trim_start();
    loop {
        if let Some(r) = rest.strip_prefix("--") {
            rest = r
                .split_once('\n')
                .map(|(_, b)| b)
                .unwrap_or("")
                .trim_start();
        } else if let Some(r) = rest.strip_prefix("/*") {
            rest = r
                .split_once("*/")
                .map(|(_, b)| b)
                .unwrap_or("")
                .trim_start();
        } else if let Some(r) = rest.strip_prefix('(') {
            rest = r.trim_start();
        } else {
            break;
        }
    }
    rest.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Tolak apa pun selain satu statement SELECT/WITH yang dibaca classifier
/// sebagai read. Berlaku di semua level akses koneksi.
pub fn ensure_select_only(db_type: &DatabaseType, sql: &str) -> Result<(), AgentError> {
    if !matches!(
        db_type,
        DatabaseType::MySQL | DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::MsSQL
    ) {
        return Err(AgentError::Refused(
            "database checks need a SQL connection (MySQL, PostgreSQL, SQLite or SQL Server)"
                .into(),
        ));
    }
    let report = build_safety_report(db_type, sql);
    if report.statements.len() != 1 {
        return Err(AgentError::Refused(format!(
            "a database check must be exactly one SELECT statement (found {})",
            report.statements.len()
        )));
    }
    let kw = first_keyword(sql);
    if !report.allowed_for_agent || !matches!(kw.as_str(), "SELECT" | "WITH") {
        return Err(AgentError::Refused(
            "only SELECT statements are allowed in database checks; writes are refused \
             whatever the connection's access level"
                .into(),
        ));
    }
    Ok(())
}

impl HeadlessSession {
    /// Jalankan satu SELECT dengan parameter terikat. `max_rows` dibatasi
    /// [`BOUND_MAX_ROWS`]; `truncated` menandai baris yang dibuang.
    pub async fn run_bound_query(
        &self,
        id: i64,
        bound: &BoundSql,
        database: Option<&str>,
        max_rows: usize,
    ) -> Result<AgentQueryResult, AgentError> {
        let (conn, pool) = self.pool_for(id).await?;
        ensure_select_only(&conn.connection_type, &bound.sql)?;
        let max_rows = max_rows.clamp(1, BOUND_MAX_ROWS);
        let database = database
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_string);
        let timeout = self.limits.query_timeout;
        let started = Instant::now();
        // Timeout server dipasang per driver; ini batas terakhir di sisi klien.
        let work = run_on_pool(&pool, bound, database.as_deref(), max_rows, timeout);
        let res = tokio::time::timeout(timeout + Duration::from_secs(5), work)
            .await
            .map_err(|_| {
                AgentError::Query(format!("query timed out after {} s", timeout.as_secs()))
            })?;
        self.record_history(&conn, &bound.display).await;
        let (columns, mut rows) = res.map_err(AgentError::Query)?;
        let truncated = rows.len() > max_rows;
        rows.truncate(max_rows);
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|r| {
                r.into_iter()
                    .map(|c| {
                        if crate::models::structs::is_null_cell(&c) {
                            crate::models::structs::NULL_DISPLAY.to_string()
                        } else {
                            c
                        }
                    })
                    .collect()
            })
            .collect();
        Ok(AgentQueryResult {
            connection_id: id,
            database,
            columns,
            row_count: rows.len(),
            rows,
            truncated,
            execution_ms: started.elapsed().as_millis(),
            warnings: Vec::new(),
            affected_rows: None,
        })
    }
}

type Rows = (Vec<String>, Vec<Vec<String>>);

async fn run_on_pool(
    pool: &DatabasePool,
    bound: &BoundSql,
    database: Option<&str>,
    max_rows: usize,
    timeout: Duration,
) -> Result<Rows, String> {
    let take = max_rows + 1;
    match pool {
        DatabasePool::MySQL(p) => run_mysql(p, bound, database, take, timeout).await,
        DatabasePool::PostgreSQL(p) => run_postgres(p, bound, take, timeout).await,
        DatabasePool::SQLite(p) => run_sqlite(p, bound, take).await,
        DatabasePool::MsSQL(p) => run_mssql(p, bound, database, take, timeout).await,
        _ => Err("this connection type does not run SQL".into()),
    }
}

fn column_names<R: sqlx::Row>(row: &R) -> Vec<String> {
    use sqlx::Column;
    row.columns().iter().map(|c| c.name().to_string()).collect()
}

/// SQL milik definisi langkah/check (sudah lolos `ensure_select_only`); semua
/// nilai variabel diikat sebagai parameter, jadi tidak ada teks luar di sini.
macro_rules! bind_all {
    ($sql:expr, $params:expr) => {{
        let mut q = sqlx::query(sqlx::AssertSqlSafe($sql.clone()));
        for p in $params {
            q = match p {
                SqlParam::Int(v) => q.bind(*v),
                SqlParam::Float(v) => q.bind(*v),
                SqlParam::Text(v) => q.bind(v.clone()),
            };
        }
        q
    }};
}

/// Kumpulkan paling banyak `n` baris lalu berhenti membaca stream.
async fn take_n<S, T>(mut stream: S, n: usize) -> Result<Vec<T>, sqlx::Error>
where
    S: futures::Stream<Item = Result<T, sqlx::Error>> + Unpin,
{
    let mut out = Vec::new();
    while out.len() < n {
        match stream.try_next().await? {
            Some(v) => out.push(v),
            None => break,
        }
    }
    Ok(out)
}

async fn run_mysql(
    pool: &sqlx::MySqlPool,
    bound: &BoundSql,
    database: Option<&str>,
    take: usize,
    timeout: Duration,
) -> Result<Rows, String> {
    let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
    if let Some(db) = database {
        let quoted = crate::schema_objects::sql::quote_ident(&DatabaseType::MySQL, db);
        // Nama database dikutip `quote_ident`.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("USE {quoted}")))
            .execute(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
    }
    // max_execution_time hanya berlaku untuk SELECT (MySQL 5.7+); engine lain
    // menolaknya tanpa akibat.
    let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET SESSION max_execution_time = {}",
        timeout.as_millis()
    )))
    .execute(&mut *conn)
    .await;
    let tx_open = sqlx::raw_sql("START TRANSACTION READ ONLY")
        .execute(&mut *conn)
        .await
        .is_ok();
    let q = bind_all!(bound.sql, &bound.params);
    let fetched = take_n(q.fetch(&mut *conn), take).await;
    if tx_open {
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
    }
    let _ = sqlx::raw_sql("SET SESSION max_execution_time = 0")
        .execute(&mut *conn)
        .await;
    let rows = fetched.map_err(|e| e.to_string())?;
    let cols = rows.first().map(column_names).unwrap_or_default();
    Ok((
        cols,
        crate::driver_mysql::convert_mysql_rows_to_table_data(rows),
    ))
}

async fn run_postgres(
    pool: &sqlx::PgPool,
    bound: &BoundSql,
    take: usize,
    timeout: Duration,
) -> Result<Rows, String> {
    let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
    let tx_open = sqlx::raw_sql("BEGIN READ ONLY")
        .execute(&mut *conn)
        .await
        .is_ok();
    if tx_open {
        let _ = sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "SET LOCAL statement_timeout = {}",
            timeout.as_millis()
        )))
        .execute(&mut *conn)
        .await;
    }
    let q = bind_all!(bound.sql, &bound.params);
    let fetched = take_n(q.fetch(&mut *conn), take).await;
    if tx_open {
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
    }
    let rows = fetched.map_err(|e| e.to_string())?;
    let cols = rows.first().map(column_names).unwrap_or_default();
    Ok((
        cols,
        crate::driver_postgres::convert_postgres_rows_to_table_data(rows),
    ))
}

async fn run_sqlite(
    pool: &sqlx::SqlitePool,
    bound: &BoundSql,
    take: usize,
) -> Result<Rows, String> {
    let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
    sqlx::raw_sql("PRAGMA query_only=ON")
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;
    let q = bind_all!(bound.sql, &bound.params);
    let fetched = take_n(q.fetch(&mut *conn), take).await;
    // Wajib dimatikan lagi: koneksi kembali ke pool yang juga dipakai menulis.
    if let Err(e) = sqlx::raw_sql("PRAGMA query_only=OFF")
        .execute(&mut *conn)
        .await
    {
        log::warn!("[AGENT] cannot reset query_only; closing the connection: {e}");
        let _ = conn.detach();
    }
    let rows = fetched.map_err(|e| e.to_string())?;
    let cols = rows.first().map(column_names).unwrap_or_default();
    Ok((
        cols,
        crate::driver_sqlite::convert_sqlite_rows_to_table_data(rows),
    ))
}

async fn run_mssql(
    pool: &mssql_driver_pool::Pool,
    bound: &BoundSql,
    database: Option<&str>,
    take: usize,
    timeout: Duration,
) -> Result<Rows, String> {
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let client = conn
        .client_mut()
        .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
    if let Some(db) = database {
        let quoted = crate::schema_objects::sql::quote_ident(&DatabaseType::MsSQL, db);
        client
            .query(&format!("USE {quoted}"), &[])
            .await
            .map_err(|e| e.to_string())?
            .collect_all()
            .await
            .map_err(|e| e.to_string())?;
    }
    let owned: Vec<Box<dyn mssql_client::ToSql + Send + Sync>> = bound
        .params
        .iter()
        .map(|p| -> Box<dyn mssql_client::ToSql + Send + Sync> {
            match p {
                SqlParam::Int(v) => Box::new(*v),
                SqlParam::Float(v) => Box::new(*v),
                SqlParam::Text(v) => Box::new(v.clone()),
            }
        })
        .collect();
    let refs: Vec<&(dyn mssql_client::ToSql + Sync)> = owned
        .iter()
        .map(|b| b.as_ref() as &(dyn mssql_client::ToSql + Sync))
        .collect();
    let mut stream = tokio::time::timeout(timeout, client.query_multiple(&bound.sql, &refs))
        .await
        .map_err(|_| format!("query timed out after {} s", timeout.as_secs()))?
        .map_err(|e| e.to_string())?;
    let mut headers: Vec<String> = Vec::new();
    let mut data: Vec<Vec<String>> = Vec::new();
    if let Some(cols) = stream.columns() {
        headers = cols.iter().map(|c| c.name.clone()).collect();
    }
    while let Some(row) = stream.next_row().await.map_err(|e| e.to_string())? {
        if data.len() >= take {
            break;
        }
        data.push(crate::driver_mssql::row_values_to_strings(&row));
    }
    Ok((headers, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_only_gate_refuses_writes_and_batches() {
        let pg = DatabaseType::PostgreSQL;
        assert!(ensure_select_only(&pg, "SELECT 1").is_ok());
        assert!(ensure_select_only(&pg, "WITH a AS (SELECT 1) SELECT * FROM a").is_ok());
        assert!(ensure_select_only(&pg, "-- c\n(SELECT 1)").is_ok());
        assert!(ensure_select_only(&pg, "DELETE FROM t WHERE id = $1").is_err());
        assert!(ensure_select_only(&pg, "SELECT 1; DELETE FROM t").is_err());
        assert!(ensure_select_only(&pg, "SHOW tables").is_err());
        assert!(ensure_select_only(&pg, "UPDATE t SET a = 1").is_err());
        assert!(ensure_select_only(&DatabaseType::Redis, "GET a").is_err());
    }

    /// Jalur nyata SQLite: nilai injeksi diikat, tulisan ditolak, koneksi
    /// kembali bisa menulis (query_only dimatikan), riwayat hanya SELECT.
    #[tokio::test]
    async fn bound_query_on_sqlite_binds_values_and_stays_read_only() {
        use crate::http_test_db::{DbRequest, Placeholder, SessionDbExecutor, bind_vars};
        use std::collections::HashMap;

        let cache = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("cache");
        let db_file =
            std::env::temp_dir().join(format!("tabular-bound-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db_file);
        sqlx::raw_sql(
            "CREATE TABLE connections (id INTEGER PRIMARY KEY, name TEXT, host TEXT, port TEXT, \
             username TEXT, password TEXT, database_name TEXT, connection_type TEXT, folder TEXT, \
             ssh_enabled INTEGER, ssh_host TEXT, ssh_port TEXT, ssh_username TEXT, ssh_auth_method TEXT, \
             ssh_private_key TEXT, ssh_password TEXT, ssh_accept_unknown_host_keys INTEGER, ssh_jump_host TEXT, \
             ssl_enabled INTEGER, ssl_ca_cert TEXT, ssl_client_cert TEXT, ssl_client_key TEXT, \
             ssl_key_passphrase TEXT, ssl_verify_server INTEGER); \
             CREATE TABLE query_history (id INTEGER PRIMARY KEY AUTOINCREMENT, query_text TEXT NOT NULL, \
             connection_id INTEGER NOT NULL, connection_name TEXT NOT NULL, executed_at DATETIME DEFAULT CURRENT_TIMESTAMP);",
        )
        .execute(&cache)
        .await
        .expect("schema");
        sqlx::query(
            "INSERT INTO connections (id, name, host, port, username, password, database_name, connection_type, folder) \
             VALUES (7, 'shop staging', ?, '', '', '', '', 'SQLite', NULL)",
        )
        .bind(db_file.to_string_lossy().to_string())
        .execute(&cache)
        .await
        .expect("insert");
        {
            let target = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&format!("sqlite://{}?mode=rwc", db_file.to_string_lossy()))
                .await
                .expect("target");
            sqlx::raw_sql(
                "CREATE TABLE stocks (id INTEGER PRIMARY KEY, code TEXT, qty REAL); \
                 INSERT INTO stocks (code, qty) VALUES ('a', 4750), ('b', 9500), ('c', -1)",
            )
            .execute(&target)
            .await
            .expect("seed");
        }

        let session = HeadlessSession::new(cache.clone());
        let vars = HashMap::from([
            ("code".to_string(), "' OR 1=1 --".to_string()),
            ("min".to_string(), "5000".to_string()),
        ]);
        // Nilai injeksi diikat sebagai teks: tidak ada baris yang cocok.
        let bound = bind_vars(
            "SELECT id FROM stocks WHERE code = {{code}}",
            &vars,
            Placeholder::Question,
        )
        .unwrap();
        let r = session
            .run_bound_query(7, &bound, None, 100)
            .await
            .expect("query");
        assert_eq!(r.row_count, 0);
        // Angka diikat sebagai angka.
        let bound = bind_vars(
            "SELECT code, qty FROM stocks WHERE qty > {{min}} ORDER BY id",
            &vars,
            Placeholder::Question,
        )
        .unwrap();
        let r = session.run_bound_query(7, &bound, None, 100).await.unwrap();
        assert_eq!(r.columns, vec!["code", "qty"]);
        assert_eq!(r.rows, vec![vec!["b".to_string(), "9500".to_string()]]);

        // Tulisan ditolak walau lewat jalur terikat.
        let write = bind_vars("UPDATE stocks SET qty = 0", &vars, Placeholder::Question).unwrap();
        assert!(session.run_bound_query(7, &write, None, 10).await.is_err());

        // query_only tidak tertinggal di koneksi pool.
        let (_, pool) = session.pool_for(7).await.expect("pool");
        let DatabasePool::SQLite(p) = pool else {
            panic!("sqlite pool")
        };
        sqlx::raw_sql("INSERT INTO stocks (code, qty) VALUES ('d', 1)")
            .execute(p.as_ref())
            .await
            .expect("pool is writable again");

        // Executor: koneksi lewat nama, sama seperti variabel environment.
        let exec = SessionDbExecutor::from_session(HeadlessSession::new(cache.clone()), false)
            .await
            .expect("executor");
        let res = exec
            .query_async(&DbRequest {
                connection: "Shop Staging".into(),
                database: None,
                sql: "SELECT COUNT(*) AS n FROM stocks WHERE qty < 0".into(),
                vars: HashMap::new(),
            })
            .await
            .expect("executor query");
        assert_eq!(res.connection_name, "shop staging");
        assert_eq!(res.output.rows, vec![vec!["1".to_string()]]);

        // Riwayat query hanya berisi SELECT (yang ditolak tidak tercatat).
        let history: Vec<(String,)> = sqlx::query_as("SELECT query_text FROM query_history")
            .fetch_all(&cache)
            .await
            .expect("history");
        assert!(!history.is_empty());
        assert!(
            history
                .iter()
                .all(|(q,)| q.trim_start().to_ascii_uppercase().starts_with("SELECT")),
            "{history:?}"
        );
        let _ = std::fs::remove_file(&db_file);
    }
}
