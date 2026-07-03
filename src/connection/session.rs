//! Dedicated session connection for manual-commit (transaction) mode.
//!
//! See docs/adr/0001-transaction-mode-session-connection.md. A query tab
//! with manual commit enabled routes its statements to one tokio task that
//! holds a single pooled connection, so `BEGIN`/`COMMIT`, `SET @var` and
//! `USE db` persist across executions. Results flow through the regular
//! `QueryResultMessage` pipeline.

use log::{debug, warn};
use sqlx::{Column, Row};
use std::time::Instant;

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

    let pool = if let Some(p) = tabular.connection_pools.get(&connection_id) {
        p.clone()
    } else if let Ok(shared) = tabular.shared_connection_pools.lock() {
        shared.get(&connection_id).cloned()?
    } else {
        return None;
    };

    let runtime = tabular.runtime.clone()?;
    let result_sender = tabular.query_result_sender.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    let handle = runtime.spawn(run_session(
        pool,
        connection_type,
        connection_id,
        database_name,
        rx,
        result_sender,
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
    connection_id: i64,
    database_name: Option<String>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
    result_sender: std::sync::mpsc::Sender<QueryResultMessage>,
) {
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
                            let _ = result_sender.send(session_message(
                                job_id,
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
                        let _ = result_sender.send(session_message(
                            job_id,
                            connection_id,
                            &sql,
                            Err(format!("BEGIN failed: {}", e)),
                            started,
                        ));
                        continue;
                    }
                    tx_open = true;
                }

                let outcome = run_query(c, &sql).await;
                let _ = result_sender.send(session_message(
                    job_id,
                    connection_id,
                    &sql,
                    outcome,
                    started,
                ));
            }
            SessionCommand::Commit { job_id } => {
                let started = Instant::now();
                let outcome = finish_tx(conn.as_mut(), &mut tx_open, "COMMIT").await;
                let _ = result_sender.send(session_message(
                    job_id,
                    connection_id,
                    "COMMIT",
                    outcome,
                    started,
                ));
            }
            SessionCommand::Rollback { job_id } => {
                let started = Instant::now();
                let outcome = finish_tx(conn.as_mut(), &mut tx_open, "ROLLBACK").await;
                let _ = result_sender.send(session_message(
                    job_id,
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
                sqlx::query(sqlx::AssertSqlSafe(use_stmt.as_str()))
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
        SessionConn::MySql(c) => sqlx::query(sqlx::AssertSqlSafe(sql))
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

async fn run_query(
    conn: &mut SessionConn,
    sql: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    match conn {
        SessionConn::MySql(c) => {
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut **c)
                .await
                .map_err(|e| e.to_string())?;
            let headers = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            Ok((
                headers,
                crate::driver_mysql::convert_mysql_rows_to_table_data(rows),
            ))
        }
        SessionConn::Postgres(c) => {
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut **c)
                .await
                .map_err(|e| e.to_string())?;
            let headers = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            let data = rows
                .into_iter()
                .map(|row| {
                    (0..row.len())
                        .map(|idx| match row.try_get::<Option<String>, _>(idx) {
                            Ok(Some(v)) => v,
                            Ok(None) => "NULL".to_string(),
                            Err(_) => {
                                if let Ok(val) = row.try_get::<i64, _>(idx) {
                                    val.to_string()
                                } else if let Ok(val) = row.try_get::<f64, _>(idx) {
                                    val.to_string()
                                } else if let Ok(val) = row.try_get::<bool, _>(idx) {
                                    val.to_string()
                                } else {
                                    "[unsupported]".to_string()
                                }
                            }
                        })
                        .collect()
                })
                .collect();
            Ok((headers, data))
        }
        SessionConn::Sqlite(c) => {
            let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
                .fetch_all(&mut **c)
                .await
                .map_err(|e| e.to_string())?;
            let headers = rows
                .first()
                .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
                .unwrap_or_default();
            Ok((
                headers,
                crate::driver_sqlite::convert_sqlite_rows_to_table_data(rows),
            ))
        }
        SessionConn::MsSQL(c) => {
            let client = c
                .client_mut()
                .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
            crate::driver_mssql::run_query(client, sql).await
        }
    }
}

fn session_message(
    job_id: u64,
    connection_id: i64,
    query: &str,
    outcome: Result<(Vec<String>, Vec<Vec<String>>), String>,
    started: Instant,
) -> QueryResultMessage {
    match outcome {
        Ok((headers, rows)) => QueryResultMessage {
            job_id,
            connection_id,
            success: true,
            affected_rows: Some(rows.len()),
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
        },
    }
}
