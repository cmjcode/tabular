//! Dedicated session connection for manual-commit (transaction) mode.
//!
//! See docs/adr/0001-transaction-mode-session-connection.md. A query tab
//! with manual commit enabled routes its statements to one tokio task that
//! holds a single pooled connection, so `BEGIN`/`COMMIT`, `SET @var` and
//! `USE db` persist across executions. Results flow through the regular
//! `QueryResultMessage` pipeline.

use futures_util::TryStreamExt;
use log::{debug, warn};
use sqlx::{Column, Row};
use std::time::Instant;

use super::execute::{
    SqliteInterruptHandle, SqliteStatementGuard, fetch_rows_limited, run_mysql_statement,
};
use super::types::QueryResultMessage;
use crate::models;
use crate::window_egui::Tabular;

#[derive(Debug)]
pub enum SessionCommand {
    Execute { job_id: u64, sql: String },
    Commit { job_id: u64 },
    Rollback { job_id: u64 },
    Close,
}

/// UI-side handle to a running session task. Stored on the query tab.
#[derive(Clone, Debug)]
pub struct SessionHandle {
    pub connection_id: i64,
    pub sender: tokio::sync::mpsc::UnboundedSender<SessionCommand>,
    pub abort: tokio::task::AbortHandle,
}

impl SessionHandle {
    pub fn send(&self, command: SessionCommand) -> bool {
        self.sender.send(command).is_ok()
    }

    /// Best-effort shutdown: ask the task to close (implicit rollback);
    /// if the channel is already gone, abort the task outright.
    pub fn close(&self) {
        if !self.send(SessionCommand::Close) {
            self.abort.abort();
        }
    }
}

/// Engines the session task supports.
pub fn supports_transactions(db: &models::enums::DatabaseType) -> bool {
    matches!(
        db,
        models::enums::DatabaseType::MySQL
            | models::enums::DatabaseType::PostgreSQL
            | models::enums::DatabaseType::SQLite
            | models::enums::DatabaseType::MsSQL
    )
}

/// Batas yang sama dengan job query biasa: jumlah baris maksimum per result set
/// dan batas waktu per statement.
#[derive(Clone, Copy, Debug)]
struct SessionLimits {
    max_rows: usize,
    timeout: Option<std::time::Duration>,
}

enum SessionConn {
    MySql(sqlx::pool::PoolConnection<sqlx::MySql>),
    Postgres(sqlx::pool::PoolConnection<sqlx::Postgres>),
    Sqlite(sqlx::pool::PoolConnection<sqlx::Sqlite>),
    MsSQL(Box<mssql_driver_pool::PooledConnection>),
}

/// Spawn a session task for the active tab's connection. Returns `None`
/// when the engine is unsupported, the pool is missing, or no runtime.
pub fn spawn_session(
    tabular: &mut Tabular,
    connection_id: i64,
    database_name: Option<String>,
) -> Option<SessionHandle> {
    let connection_type = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .map(|c| c.connection_type.clone())?;
    if !supports_transactions(&connection_type) {
        return None;
    }

    // Pool yang tunnel SSH-nya mati tidak bisa membuka koneksi sesi; buang
    // supaya connect berikutnya membangun ulang tunnel + pool.
    if super::pool::evict_unusable_pool(tabular, connection_id) {
        super::pool::ensure_background_pool_creation(tabular, connection_id);
        return None;
    }

    let pool = if let Some(p) = tabular.connection_pools.get(&connection_id) {
        p.clone()
    } else {
        super::pool::lock_or_recover(&tabular.shared_connection_pools)
            .get(&connection_id)
            .cloned()?
    };

    let tab_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.id);
    let runtime = tabular.runtime.clone()?;
    let result_sender = tabular.query_result_sender.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    // Sama dengan `prepare_query_job`: tanpa ini mode manual-commit membaca
    // seluruh result set ke memori dan tidak pernah timeout.
    let limits = SessionLimits {
        max_rows: tabular.max_result_rows.max(1) as usize,
        timeout: (tabular.query_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(tabular.query_timeout_secs as u64)),
    };

    let handle = runtime.spawn(run_session(
        pool,
        connection_type,
        tab_id,
        connection_id,
        database_name,
        limits,
        rx,
        result_sender,
        tabular.result_wake_hook(),
    ));

    Some(SessionHandle {
        connection_id,
        sender: tx,
        abort: handle.abort_handle(),
    })
}

async fn run_session(
    pool: models::enums::DatabasePool,
    connection_type: models::enums::DatabaseType,
    tab_id: Option<usize>,
    connection_id: i64,
    database_name: Option<String>,
    limits: SessionLimits,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
    result_sender: std::sync::mpsc::Sender<QueryResultMessage>,
    wake: Option<super::types::ResultWakeHook>,
) {
    // Setiap hasil juga membangunkan UI (lihat `ResultWakeHook`).
    let send = |message: QueryResultMessage| {
        super::execute::send_and_wake(&result_sender, message, wake.as_ref());
    };
    // URUTAN DEKLARASI PENTING: `conn` dideklarasikan sebelum
    // `SqliteStatementGuard` mana pun (lihat invarian di guard tersebut).
    let mut conn: Option<SessionConn> = None;
    let mut tx_open = false;

    while let Some(command) = rx.recv().await {
        match command {
            SessionCommand::Execute { job_id, sql } => {
                // Acquire lazily so connect errors land on a real job id.
                if conn.is_none() {
                    match acquire(&pool, &connection_type, database_name.as_deref()).await {
                        Ok(c) => conn = Some(c),
                        Err(e) => {
                            send(session_message(
                                job_id,
                                tab_id,
                                connection_id,
                                &sql,
                                Err(format!("Cannot open session connection: {}", e)),
                                Instant::now(),
                            ));
                            continue;
                        }
                    }
                }
                let c = conn.as_mut().expect("session connection acquired");
                let started = Instant::now();

                if !tx_open {
                    let begin = match connection_type {
                        models::enums::DatabaseType::MySQL => "START TRANSACTION",
                        models::enums::DatabaseType::MsSQL => "BEGIN TRANSACTION",
                        _ => "BEGIN",
                    };
                    if let Err(e) = run_simple(c, begin).await {
                        send(session_message(
                            job_id,
                            tab_id,
                            connection_id,
                            &sql,
                            Err(format!("BEGIN failed: {}", e)),
                            started,
                        ));
                        continue;
                    }
                    tx_open = true;
                }

                // Untuk SQLite: bila task ini di-abort atau timeout tercapai,
                // guard meng-interrupt statement yang masih berjalan.
                let interrupt = match c {
                    SessionConn::Sqlite(sqlite) => {
                        SqliteInterruptHandle::for_connection(sqlite).await
                    }
                    _ => None,
                };
                let statement_guard = SqliteStatementGuard::new(interrupt);
                let timed = match limits.timeout {
                    Some(limit) => {
                        tokio::time::timeout(limit, run_statement(c, &sql, limits.max_rows))
                            .await
                            .map_err(|_| limit)
                    }
                    None => Ok(run_statement(c, &sql, limits.max_rows).await),
                };
                let outcome = match timed {
                    Ok(result) => {
                        statement_guard.finish();
                        result
                    }
                    Err(limit) => {
                        // Guard di-drop (interrupt) selagi koneksi masih hidup,
                        // baru kemudian koneksinya dibuang.
                        drop(statement_guard);
                        // Koneksi masih punya query berjalan / respons tertunda;
                        // tidak boleh dipakai lagi atau kembali ke pool.
                        discard_session_connection(conn.take());
                        tx_open = false;
                        Err(format!(
                            "Query timed out after {}s. The session connection was closed and its open transaction was rolled back.",
                            limit.as_secs()
                        ))
                    }
                };
                send(session_message(
                    job_id,
                    tab_id,
                    connection_id,
                    &sql,
                    outcome,
                    started,
                ));
            }
            SessionCommand::Commit { job_id } => {
                let started = Instant::now();
                let outcome = finish_tx(conn.as_mut(), &mut tx_open, "COMMIT")
                    .await
                    .map(|(h, r)| (h, r, None, false));
                send(session_message(
                    job_id,
                    tab_id,
                    connection_id,
                    "COMMIT",
                    outcome,
                    started,
                ));
            }
            SessionCommand::Rollback { job_id } => {
                let started = Instant::now();
                let outcome = finish_tx(conn.as_mut(), &mut tx_open, "ROLLBACK")
                    .await
                    .map(|(h, r)| (h, r, None, false));
                send(session_message(
                    job_id,
                    tab_id,
                    connection_id,
                    "ROLLBACK",
                    outcome,
                    started,
                ));
            }
            SessionCommand::Close => {
                if tx_open
                    && let Some(c) = conn.as_mut()
                    && let Err(e) = run_simple(c, "ROLLBACK").await
                {
                    warn!("session close: implicit ROLLBACK failed: {}", e);
                }
                break;
            }
        }
    }
    debug!("session task for connection {} ended", connection_id);
}

/// Buang koneksi sesi yang tidak boleh dipakai lagi. Koneksi jaringan di-detach
/// lalu ditutup (server me-rollback transaksinya); koneksi SQLite sudah
/// di-interrupt dan di-rollback oleh hook `after_release` pool.
fn discard_session_connection(conn: Option<SessionConn>) {
    match conn {
        Some(SessionConn::MySql(c)) => drop(c.detach()),
        Some(SessionConn::Postgres(c)) => drop(c.detach()),
        Some(SessionConn::Sqlite(c)) => drop(c),
        Some(SessionConn::MsSQL(c)) => drop(c.detach()),
        None => {}
    }
}

async fn finish_tx(
    conn: Option<&mut SessionConn>,
    tx_open: &mut bool,
    verb: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let Some(c) = conn else {
        *tx_open = false;
        return Err("No open session connection".to_string());
    };
    if !*tx_open {
        return Err(format!("{}: no transaction is open", verb));
    }
    run_simple(c, verb).await?;
    *tx_open = false;
    Ok((Vec::new(), Vec::new()))
}

async fn acquire(
    pool: &models::enums::DatabasePool,
    connection_type: &models::enums::DatabaseType,
    database_name: Option<&str>,
) -> Result<SessionConn, String> {
    match pool {
        models::enums::DatabasePool::MySQL(p) => {
            let mut conn = p.acquire().await.map_err(|e| e.to_string())?;
            if let Some(db) = database_name.filter(|d| !d.trim().is_empty()) {
                let use_stmt = format!("USE `{}`", db.replace('`', "``"));
                // Protokol teks: `USE` ditolak protokol prepared statement.
                // Hook `after_release` pool mengembalikan database default.
                sqlx::raw_sql(sqlx::AssertSqlSafe(use_stmt))
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(SessionConn::MySql(conn))
        }
        models::enums::DatabasePool::PostgreSQL(p) => {
            // The pool is already bound to the selected database.
            Ok(SessionConn::Postgres(
                p.acquire().await.map_err(|e| e.to_string())?,
            ))
        }
        models::enums::DatabasePool::SQLite(p) => Ok(SessionConn::Sqlite(
            p.acquire().await.map_err(|e| e.to_string())?,
        )),
        models::enums::DatabasePool::MsSQL(p) => {
            let mut conn = p.get().await.map_err(|e| e.to_string())?;
            if let Some(db) = database_name.filter(|d| !d.trim().is_empty()) {
                let use_sql = format!("USE [{}]", db.replace(']', "]]"));
                conn.client_mut()
                    .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?
                    .simple_query(use_sql.as_str())
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(SessionConn::MsSQL(Box::new(conn)))
        }
        _ => Err(format!(
            "Transactions are not supported for {:?}",
            connection_type
        )),
    }
}

async fn run_simple(conn: &mut SessionConn, sql: &str) -> Result<(), String> {
    match conn {
        // Protokol teks: START TRANSACTION / ROLLBACK tidak didukung protokol
        // prepared statement di semua versi server MySQL/MariaDB.
        SessionConn::MySql(c) => sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(&mut **c)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        SessionConn::Postgres(c) => sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut **c)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        SessionConn::Sqlite(c) => sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut **c)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string()),
        SessionConn::MsSQL(c) => c
            .client_mut()
            .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?
            .simple_query(sql)
            .await
            .map_err(|e| e.to_string()),
    }
}

/// Hasil satu statement di sesi: header, baris, jumlah baris terdampak (Some
/// hanya untuk statement pengubah data), dan penanda result set terpotong.
type StatementOutput = (Vec<String>, Vec<Vec<String>>, Option<u64>, bool);

/// Jalankan satu statement di koneksi sesi. Statement pengubah data dijalankan
/// lewat `execute()` supaya jumlah baris terdampak dari driver bisa dilaporkan.
/// Result set dibaca lewat stream dan berhenti setelah `max_rows` baris.
async fn run_statement(
    conn: &mut SessionConn,
    sql: &str,
    max_rows: usize,
) -> Result<StatementOutput, String> {
    let returns_rows = crate::connection::sql::statement_returns_rows(sql);
    // MySQL: satu jalur untuk keduanya, dengan fallback protokol teks untuk
    // statement yang ditolak protokol prepared statement (error 1295).
    if let SessionConn::MySql(c) = conn {
        let (rows, truncated, affected) = run_mysql_statement(c, sql, returns_rows, max_rows)
            .await
            .map_err(|e| e.to_string())?;
        if let Some(n) = affected {
            return Ok((Vec::new(), Vec::new(), Some(n), false));
        }
        let headers = rows
            .first()
            .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
            .unwrap_or_default();
        return Ok((
            headers,
            crate::driver_mysql::convert_mysql_rows_to_table_data(rows),
            None,
            truncated,
        ));
    }
    if !returns_rows {
        let affected = match conn {
            // Sudah ditangani di atas.
            SessionConn::MySql(_) => None,
            SessionConn::Postgres(c) => Some(
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(&mut **c)
                    .await
                    .map_err(|e| e.to_string())?
                    .rows_affected(),
            ),
            SessionConn::Sqlite(c) => Some(
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(&mut **c)
                    .await
                    .map_err(|e| e.to_string())?
                    .rows_affected(),
            ),
            // Driver MsSQL mengembalikan hasil lewat jalur query biasa.
            SessionConn::MsSQL(_) => None,
        };
        if let Some(n) = affected {
            return Ok((Vec::new(), Vec::new(), Some(n), false));
        }
    }
    run_query(conn, sql, max_rows)
        .await
        .map(|(h, r, truncated)| (h, r, None, truncated))
}

/// Baca result set di koneksi sesi, paling banyak `max_rows` baris.
/// Mengembalikan header, baris, dan penanda terpotong.
async fn run_query(
    conn: &mut SessionConn,
    sql: &str,
    max_rows: usize,
) -> Result<(Vec<String>, Vec<Vec<String>>, bool), String> {
    match conn {
        SessionConn::MySql(c) => {
            let (rows, truncated, _affected) = run_mysql_statement(c, sql, true, max_rows)
                .await
                .map_err(|e| e.to_string())?;
            let headers = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            Ok((
                headers,
                crate::driver_mysql::convert_mysql_rows_to_table_data(rows),
                truncated,
            ))
        }
        SessionConn::Postgres(c) => {
            let (rows, truncated) =
                fetch_rows_limited!(sqlx::query(sqlx::AssertSqlSafe(sql)), &mut **c, max_rows)
                    .await
                    .map_err(|e| e.to_string())?;
            let headers = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            Ok((
                headers,
                crate::driver_postgres::convert_postgres_rows_to_table_data(rows),
                truncated,
            ))
        }
        SessionConn::Sqlite(c) => {
            let (rows, truncated) =
                fetch_rows_limited!(sqlx::query(sqlx::AssertSqlSafe(sql)), &mut **c, max_rows)
                    .await
                    .map_err(|e| e.to_string())?;
            let headers = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            Ok((
                headers,
                crate::driver_sqlite::convert_sqlite_rows_to_table_data(rows),
                truncated,
            ))
        }
        SessionConn::MsSQL(c) => {
            let client = c
                .client_mut()
                .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
            crate::driver_mssql::run_query_limited(client, sql, max_rows).await
        }
    }
}

fn session_message(
    job_id: u64,
    tab_id: Option<usize>,
    connection_id: i64,
    query: &str,
    outcome: Result<StatementOutput, String>,
    started: Instant,
) -> QueryResultMessage {
    match outcome {
        Ok((headers, rows, affected, truncated)) => QueryResultMessage {
            job_id,
            tab_id,
            connection_id,
            success: true,
            affected_rows: affected.map(|n| n as usize),
            truncated,
            error_location: None,
            timing: None,
            headers,
            rows,
            error: None,
            duration: started.elapsed(),
            query: query.to_string(),
            dba_special_mode: None,
            ast_debug_sql: None,
            ast_headers: None,
            column_metadata: None,
        },
        Err(message) => QueryResultMessage {
            job_id,
            tab_id,
            connection_id,
            success: false,
            headers: vec!["Error".to_string()],
            rows: vec![vec![message.clone()]],
            error: Some(message),
            duration: started.elapsed(),
            query: query.to_string(),
            dba_special_mode: None,
            ast_debug_sql: None,
            ast_headers: None,
            affected_rows: None,
            column_metadata: None,
            truncated: false,
            error_location: None,
            timing: None,
        },
    }
}
