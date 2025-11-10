use crate::{
    connection, driver_mssql, driver_mysql, driver_postgres, driver_redis, driver_sqlite, models,
    modules, ssh_tunnel,
    window_egui::{self, Tabular},
};
use eframe::egui;
use futures_util::TryStreamExt; // for MsSQL try_next
use futures_util::stream::StreamExt; // for buffered concurrency
use log::debug;
use mongodb::{Client as MongoClient, bson::doc};
use redis::{Client, aio::ConnectionManager};
use sqlx::Connection; // for MySqlConnection::connect
use sqlx::mysql::MySqlConnection;
use sqlx::{
    Column, Row, SqlitePool, mysql::MySqlPoolOptions, postgres::PgPoolOptions,
    sqlite::SqlitePoolOptions,
};
use std::sync::Arc;
use std::time::Instant;

// Limit concurrent prefetch tasks to avoid overwhelming servers and local machine
const PREFETCH_CONCURRENCY: usize = 6;

fn keyword_in_sql(upper_sql: &str, keyword: &str) -> bool {
    let bytes = upper_sql.as_bytes();
    let key_bytes = keyword.as_bytes();
    let mut search_from = 0;
    while search_from + key_bytes.len() <= bytes.len() {
        if let Some(rel_pos) = upper_sql[search_from..].find(keyword) {
            let start = search_from + rel_pos;
            let end = start + key_bytes.len();
            let prev_is_ident = if start == 0 {
                false
            } else {
                let prev = bytes[start - 1];
                prev.is_ascii_alphanumeric() || prev == b'_' 
            };
            let next_is_ident = match bytes.get(end) {
                Some(next) => next.is_ascii_alphanumeric() || *next == b'_',
                None => false,
            };
            if !prev_is_ident && !next_is_ident {
                return true;
            }
            search_from = end;
        } else {
            break;
        }
    }
    false
}

pub fn query_contains_pagination(sql: &str) -> bool {
    let upper = sql.to_uppercase();
    let upper_ref = upper.as_str();
    keyword_in_sql(upper_ref, "LIMIT")
        || keyword_in_sql(upper_ref, "OFFSET")
        || keyword_in_sql(upper_ref, "FETCH")
        || keyword_in_sql(upper_ref, "TOP")
        || upper_ref.contains("FETCH NEXT")
        || upper_ref.contains("FETCH FIRST")
        || upper_ref.contains("FETCH PRIOR")
        || upper_ref.contains("FETCH ROW")
        || upper_ref.contains("FETCH ROWS")
}

#[derive(Clone, Debug)]
pub struct QueryExecutionOptions {
    pub connection_id: i64,
    pub connection: models::structs::ConnectionConfig,
    pub query: String,
    pub selected_database: Option<String>,
    pub use_server_pagination: bool,
    pub current_page: usize,
    pub page_size: usize,
    pub base_query: Option<String>,
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub save_to_history: bool,
    pub ast_enabled: bool,
}

#[derive(Clone)]
pub struct QueryJob {
    pub job_id: u64,
    pub options: QueryExecutionOptions,
    pub connection_pool: models::enums::DatabasePool,
    pub started_at: Instant,
}

#[derive(Clone, Debug)]
pub struct QueryJobStatus {
    pub job_id: u64,
    pub connection_id: i64,
    pub query_preview: String,
    pub started_at: Instant,
    pub completed: bool,
}

#[derive(Debug, Clone)]
pub struct QueryResultMessage {
    pub job_id: u64,
    pub connection_id: i64,
    pub success: bool,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub error: Option<String>,
    pub duration: std::time::Duration,
    pub query: String,
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub ast_debug_sql: Option<String>,
    pub ast_headers: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct QueryJobOutput {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub ast_debug_sql: Option<String>,
    pub ast_headers: Option<Vec<String>>,
}

#[derive(Debug)]
pub enum QueryPreparationError {
    ConnectionNotFound,
    PoolUnavailable,
    RuntimeUnavailable,
    UnsupportedDatabase,
}

#[derive(Debug)]
pub enum QueryExecutionError {
    Message(String),
}

pub(crate) fn prepare_query_job(
    tabular: &mut Tabular,
    connection_id: i64,
    query: String,
    job_id: u64,
) -> Result<QueryJob, QueryPreparationError> {
    let connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .cloned()
        .ok_or(QueryPreparationError::ConnectionNotFound)?;

    let selected_database = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .filter(|s| !s.trim().is_empty());

    let dba_special_mode = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.dba_special_mode.clone());

    let connection_pool = if let Some(pool) = tabular.connection_pools.get(&connection_id) {
        pool.clone()
    } else if let Ok(shared) = tabular.shared_connection_pools.lock() {
        shared
            .get(&connection_id)
            .cloned()
            .ok_or(QueryPreparationError::PoolUnavailable)?
    } else {
        return Err(QueryPreparationError::PoolUnavailable);
    };

    let base_query = if tabular.current_base_query.trim().is_empty() {
        None
    } else {
        Some(tabular.current_base_query.clone())
    };

    let options = QueryExecutionOptions {
        connection_id,
        connection,
        query,
        selected_database,
        use_server_pagination: tabular.use_server_pagination,
        current_page: tabular.current_page,
        page_size: tabular.page_size,
        base_query,
        dba_special_mode,
        save_to_history: true,
        ast_enabled: cfg!(feature = "query_ast"),
    };

    Ok(QueryJob {
        job_id,
        options,
        connection_pool,
        started_at: Instant::now(),
    })
}

pub(crate) fn spawn_query_job(
    tabular: &mut Tabular,
    job: QueryJob,
    sender: std::sync::mpsc::Sender<QueryResultMessage>,
) -> Result<tokio::task::JoinHandle<()>, QueryPreparationError> {
    let runtime = tabular
        .runtime
        .clone()
        .ok_or(QueryPreparationError::RuntimeUnavailable)?;

    let handle = runtime.spawn(async move {
        let result = execute_query_job(job).await;
        let _ = sender.send(result);
    });

    Ok(handle)
}

async fn execute_query_job(job: QueryJob) -> QueryResultMessage {
    let start = job.started_at;
    let connection_id = job.options.connection_id;
    let query = job.options.query.clone();
    let dba_special_mode = job.options.dba_special_mode.clone();

    let outcome = match job.options.connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            execute_mysql_query_job(&job.options, job.connection_pool.clone()).await
        }
        models::enums::DatabaseType::PostgreSQL => {
            execute_postgres_query_job(&job.options, job.connection_pool.clone()).await
        }
        models::enums::DatabaseType::SQLite => {
            execute_sqlite_query_job(&job.options, job.connection_pool.clone()).await
        }
        models::enums::DatabaseType::Redis => {
            execute_redis_query_job(&job.options, job.connection_pool.clone()).await
        }
        models::enums::DatabaseType::MsSQL => {
            execute_mssql_query_job(&job.options, job.connection_pool.clone()).await
        }
        models::enums::DatabaseType::MongoDB => {
            execute_mongodb_query_job(&job.options, job.connection_pool.clone()).await
        }
    };

    match outcome {
        Ok(output) => QueryResultMessage {
            job_id: job.job_id,
            connection_id,
            success: true,
            headers: output.headers,
            rows: output.rows,
            error: None,
            duration: start.elapsed(),
            query,
            dba_special_mode,
            ast_debug_sql: output.ast_debug_sql,
            ast_headers: output.ast_headers,
        },
        Err(err) => {
            let message = describe_execution_error(err);
            QueryResultMessage {
                job_id: job.job_id,
                connection_id,
                success: false,
                headers: vec!["Error".to_string()],
                rows: vec![vec![message.clone()]],
                error: Some(message),
                duration: start.elapsed(),
                query,
                dba_special_mode,
                ast_debug_sql: None,
                ast_headers: None,
            }
        }
    }
}

fn describe_execution_error(err: QueryExecutionError) -> String {
    match err {
        QueryExecutionError::Message(msg) => msg,
    }
}

async fn execute_mysql_query_job(
    options: &QueryExecutionOptions,
    _pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    debug!(
        "[async] Executing MySQL query (conn_id={})",
        options.connection_id
    );

    let (target_host, target_port) =
        resolve_connection_target(&options.connection).map_err(QueryExecutionError::Message)?;

    let statements_raw: Vec<&str> = options
        .query
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = if statements_raw.len() == 1
        && statements_raw[0]
            .trim_start()
            .to_uppercase()
            .starts_with("SELECT")
    {
        let should_paginate = options.use_server_pagination
            && !query_contains_pagination(statements_raw[0]);
        let pagination_opt = if should_paginate {
            Some((options.current_page as u64, options.page_size as u64))
        } else {
            None
        };
        match crate::query_ast::compile_single_select(
            statements_raw[0],
            &options.connection.connection_type,
            pagination_opt,
            true,
        ) {
            Ok((new_sql, hdrs)) => {
                if !hdrs.is_empty() {
                    inferred_headers_from_ast = Some(hdrs.clone());
                    ast_headers = Some(hdrs.clone());
                }
                vec![new_sql]
            }
            Err(_) => statements_raw.iter().map(|s| s.to_string()).collect(),
        }
    } else {
        statements_raw.iter().map(|s| s.to_string()).collect()
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    let replication_status_mode = matches!(
        options.dba_special_mode,
        Some(models::enums::DBASpecialMode::ReplicationStatus)
    );
    let master_status_mode = matches!(
        options.dba_special_mode,
        Some(models::enums::DBASpecialMode::MasterStatus)
    );

    let encoded_username = modules::url_encode(&options.connection.username);
    let encoded_password = modules::url_encode(&options.connection.password);

    let default_db = if let Some(db) = &options.selected_database {
        if db.trim().is_empty() {
            options.connection.database.clone()
        } else {
            db.clone()
        }
    } else {
        options.connection.database.clone()
    };

    // Log resolved target and database context up front to help diagnose schema issues
    debug!(
        "[mysql] target={}:{}, selected_database={:?}, default_db={}",
        target_host, target_port, options.selected_database, default_db
    );

    let mut ast_debug_sql: Option<String> = None;
    #[cfg(feature = "query_ast")]
    {
        if let Some(sql) = statements.first()
            && statements.len() == 1
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
        {
            ast_debug_sql = Some(sql.clone());
        }
    }

    let mut attempts = 0;
    let max_attempts = 3;
    let mut last_error: Option<String> = None;
    // Track the first failing statement across attempts for better diagnostics
    let mut failing_stmt_preview: Option<String> = None;

    while attempts < max_attempts {
        attempts += 1;

        let dsn = format!(
            "mysql://{}:{}@{}:{}/{}",
            encoded_username, encoded_password, target_host, target_port, default_db
        );

        let mut conn = match MySqlConnection::connect(&dsn).await {
            Ok(c) => c,
            Err(e) => {
                last_error = Some(e.to_string());
                continue;
            }
        };

        let _ = sqlx::query("SET SESSION wait_timeout = 600")
            .execute(&mut conn)
            .await;
        let _ = sqlx::query("SET SESSION interactive_timeout = 600")
            .execute(&mut conn)
            .await;
        let _ = sqlx::query("SET SESSION net_read_timeout = 120")
            .execute(&mut conn)
            .await;
        let _ = sqlx::query("SET SESSION net_write_timeout = 120")
            .execute(&mut conn)
            .await;
        let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824")
            .execute(&mut conn)
            .await;
        let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'")
            .execute(&mut conn)
            .await;

        let mut final_headers: Vec<String> = Vec::new();
        let mut final_data: Vec<Vec<String>> = Vec::new();
        let mut execution_success = true;

        for (idx, statement) in statements_ref.iter().enumerate() {
            let trimmed = statement.trim();
            if trimmed.is_empty()
                || trimmed.starts_with("--")
                || trimmed.starts_with('#')
                || trimmed.starts_with("/*")
            {
                continue;
            }

            let upper = trimmed.to_uppercase();
            if upper.starts_with("USE ") {
                let db_part = trimmed[3..].trim();
                let db_name = db_part
                    .trim_matches('`')
                    .trim_matches('"')
                    .trim_matches('[')
                    .trim_matches(']')
                    .trim();

                let use_stmt = format!("USE `{}`", db_name);
                if sqlx::query(&use_stmt).execute(&mut conn).await.is_err() {
                    let new_dsn = format!(
                        "mysql://{}:{}@{}:{}/{}",
                        encoded_username, encoded_password, target_host, target_port, db_name
                    );
                    match MySqlConnection::connect(&new_dsn).await {
                        Ok(new_conn) => {
                            let mut new_conn = new_conn;
                            let _ = sqlx::query("SET SESSION wait_timeout = 600")
                                .execute(&mut new_conn)
                                .await;
                            let _ = sqlx::query("SET SESSION interactive_timeout = 600")
                                .execute(&mut new_conn)
                                .await;
                            let _ = sqlx::query("SET SESSION net_read_timeout = 120")
                                .execute(&mut new_conn)
                                .await;
                            let _ = sqlx::query("SET SESSION net_write_timeout = 120")
                                .execute(&mut new_conn)
                                .await;
                            let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824")
                                .execute(&mut new_conn)
                                .await;
                            let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'")
                                .execute(&mut new_conn)
                                .await;
                            conn = new_conn;
                        }
                        Err(e) => {
                            last_error = Some(format!("USE failed (reconnect): {}", e));
                            execution_success = false;
                            break;
                        }
                    }
                }
                continue;
            }

            let query_result = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                sqlx::query(trimmed).fetch_all(&mut conn),
            )
            .await;

            match query_result {
                Ok(Ok(rows)) => {
                    if idx == statements_ref.len() - 1 {
                        if !rows.is_empty() {
                            final_headers = rows[0]
                                .columns()
                                .iter()
                                .map(|c| c.name().to_string())
                                .collect();
                            final_data = driver_mysql::convert_mysql_rows_to_table_data(rows);

                            if replication_status_mode || master_status_mode {
                                let version_str = match sqlx::query("SELECT VERSION() AS v")
                                    .fetch_one(&mut conn)
                                    .await
                                {
                                    Ok(vrow) => vrow.try_get::<String, _>("v").unwrap_or_default(),
                                    Err(_) => String::new(),
                                };
                                let is_mariadb = version_str.to_lowercase().contains("mariadb");

                                if replication_status_mode
                                    && final_data.is_empty()
                                    && let Ok(fallback_rows) =
                                        sqlx::query("SHOW SLAVE STATUS").fetch_all(&mut conn).await
                                    && !fallback_rows.is_empty()
                                {
                                    final_headers = fallback_rows[0]
                                        .columns()
                                        .iter()
                                        .map(|c| c.name().to_string())
                                        .collect();
                                    final_data = driver_mysql::convert_mysql_rows_to_table_data(
                                        fallback_rows,
                                    );
                                }

                                if !final_headers.is_empty() && !final_data.is_empty() {
                                    let header_index = |name: &str| {
                                        final_headers
                                            .iter()
                                            .position(|h| h.eq_ignore_ascii_case(name))
                                    };
                                    let first = &final_data[0];
                                    let mut summary: Vec<(String, String)> = Vec::new();

                                    if replication_status_mode {
                                        if let Some(idx) = header_index("Replica_IO_Running")
                                            .or_else(|| header_index("Slave_IO_Running"))
                                        {
                                            summary.push(("IO Thread".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Replica_SQL_Running")
                                            .or_else(|| header_index("Slave_SQL_Running"))
                                        {
                                            summary.push(("SQL Thread".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Seconds_Behind_Source")
                                            .or_else(|| header_index("Seconds_Behind_Master"))
                                        {
                                            summary.push((
                                                "Seconds Behind".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) = header_index("Channel_Name") {
                                            summary.push(("Channel".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Retrieved_Gtid_Set") {
                                            summary.push((
                                                "Retrieved GTID".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) = header_index("Executed_Gtid_Set") {
                                            summary
                                                .push(("Executed GTID".into(), first[idx].clone()));
                                        }
                                    }

                                    if master_status_mode {
                                        if let Some(idx) = header_index("File") {
                                            summary.push((
                                                "Binary Log File".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) = header_index("Position") {
                                            summary.push(("Position".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Binlog_Do_DB") {
                                            summary
                                                .push(("Binlog Do DB".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Binlog_Ignore_DB") {
                                            summary.push((
                                                "Binlog Ignore DB".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                    }

                                    if !summary.is_empty() {
                                        let mut summary_table: Vec<Vec<String>> = summary
                                            .into_iter()
                                            .map(|(metric, value)| vec![metric, value])
                                            .collect();
                                        summary_table.push(vec![
                                            "Server Version".into(),
                                            version_str.clone(),
                                        ]);
                                        summary_table.push(vec![
                                            "Engine".into(),
                                            if is_mariadb {
                                                "MariaDB".into()
                                            } else {
                                                "MySQL".into()
                                            },
                                        ]);
                                        final_headers = vec!["Metric".into(), "Value".into()];
                                        final_data = summary_table;
                                    }
                                }
                            }
                        } else {
                            #[cfg(feature = "query_ast")]
                            if final_headers.is_empty()
                                && ast_debug_sql.is_some()
                                && let Some(hh) = inferred_headers_from_ast.clone()
                                && !hh.is_empty()
                            {
                                final_headers = hh;
                            }

                            if final_headers.is_empty()
                                && trimmed.to_uppercase().starts_with("SELECT")
                            {
                                let inferred = infer_select_headers(trimmed);
                                if !inferred.is_empty() {
                                    final_headers = inferred;
                                }
                            }

                            final_data = Vec::new();
                        }
                    }
                }
                Ok(Err(e)) => {
                    let err_str = e.to_string();
                    // Capture which statement failed (preview 200 chars)
                    if failing_stmt_preview.is_none() {
                        let prev = if trimmed.len() > 200 {
                            format!("{}...", &trimmed[..200])
                        } else {
                            trimmed.to_string()
                        };
                        failing_stmt_preview = Some(prev);
                    }
                    // Attach a friendly hint for common 1146 (table doesn't exist) cases
                    if err_str.contains("1146") || err_str.to_lowercase().contains("doesn't exist")
                    {
                        let mut hint = String::new();
                        hint.push_str("Hint: Check the database/schema qualifier in your SQL. ");
                        hint.push_str(&format!(
                            "Current default database is '{}'. If your query references a different schema (e.g., 'foxlogger' vs actual '{}'), it can fail even if SELECT * FROM table works in the default DB. ",
                            default_db, default_db
                        ));
                        hint.push_str("Try removing the schema prefix or replacing it with the selected/default database, or switch the active database in the tab. ");
                        hint.push_str("Also, on case-sensitive MySQL servers (lower_case_table_names=0), using backticks requires exact table name casing. If unquoted works but \"`name`\" fails, check SHOW TABLES for the exact case and match it.");
                        last_error = Some(format!("{}\n\n{}", err_str, hint));
                    } else {
                        last_error = Some(err_str);
                    }
                    execution_success = false;
                    break;
                }
                Err(_) => {
                    last_error = Some("Query timeout after 60s".to_string());
                    execution_success = false;
                    break;
                }
            }
        }

        if execution_success {
            // Log final headers for diagnostics (helps detect unexpected projection pruning)
            if !final_headers.is_empty() {
                debug!(
                    "[mysql] final headers ({}): {:?}",
                    final_headers.len(),
                    final_headers
                );
            } else {
                debug!(
                    "[mysql] final headers are empty (rows: {})",
                    final_data.len()
                );
            }
            return Ok(QueryJobOutput {
                headers: final_headers,
                rows: final_data,
                ast_debug_sql,
                ast_headers,
            });
        }
    }

    let mut final_err = last_error.unwrap_or_else(|| "Unknown MySQL error".to_string());
    if let Some(stmt) = failing_stmt_preview {
        final_err = format!("{}\n\nFailed statement (preview): {}", final_err, stmt);
    }
    Err(QueryExecutionError::Message(final_err))
}

async fn execute_postgres_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let pg_pool = match pool {
        models::enums::DatabasePool::PostgreSQL(pg) => pg,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for PostgreSQL".to_string(),
            ));
        }
    };

    let statements_raw: Vec<&str> = options
        .query
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    let mut ast_debug_sql: Option<String> = None;

    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = if statements_raw.len() == 1
        && statements_raw[0]
            .trim_start()
            .to_uppercase()
            .starts_with("SELECT")
    {
        let should_paginate = options.use_server_pagination
            && !query_contains_pagination(statements_raw[0]);
        let pagination_opt = if should_paginate {
            Some((options.current_page as u64, options.page_size as u64))
        } else {
            None
        };
        match crate::query_ast::compile_single_select(
            statements_raw[0],
            &options.connection.connection_type,
            pagination_opt,
            true,
        ) {
            Ok((new_sql, hdrs)) => {
                if !hdrs.is_empty() {
                    inferred_headers_from_ast = Some(hdrs.clone());
                    ast_headers = Some(hdrs.clone());
                }
                ast_debug_sql = Some(new_sql.clone());
                vec![new_sql]
            }
            Err(_) => statements_raw.iter().map(|s| s.to_string()).collect(),
        }
    } else {
        statements_raw.iter().map(|s| s.to_string()).collect()
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    let mut final_headers = Vec::new();
    let mut final_data = Vec::new();

    for (i, statement) in statements_ref.iter().enumerate() {
        let trimmed = statement.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") {
            continue;
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            sqlx::query(trimmed).fetch_all(pg_pool.as_ref()),
        )
        .await;

        match result {
            Ok(Ok(rows)) => {
                if i == statements_ref.len() - 1 {
                    if !rows.is_empty() {
                        final_headers = rows[0]
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        final_data = rows
                            .into_iter()
                            .map(|row| {
                                (0..row.len())
                                    .map(|idx| match row.try_get::<Option<String>, _>(idx) {
                                        Ok(Some(v)) => v,
                                        Ok(None) => "NULL".to_string(),
                                        Err(_) => {
                                            // Try a few primitive fallbacks
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
                    } else {
                        #[cfg(feature = "query_ast")]
                        if final_headers.is_empty()
                            && let Some(hh) = inferred_headers_from_ast.clone()
                            && !hh.is_empty()
                        {
                            final_headers = hh;
                        }
                        if final_headers.is_empty() && trimmed.to_uppercase().starts_with("SELECT")
                        {
                            let inferred = infer_select_headers(trimmed);
                            if !inferred.is_empty() {
                                final_headers = inferred;
                            }
                        }
                        final_data = Vec::new();
                    }
                }
            }
            Ok(Err(e)) => {
                return Err(QueryExecutionError::Message(format!(
                    "PostgreSQL error: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(QueryExecutionError::Message(
                    "PostgreSQL query timed out".to_string(),
                ));
            }
        }
    }

    Ok(QueryJobOutput {
        headers: final_headers,
        rows: final_data,
        ast_debug_sql,
        ast_headers,
    })
}

async fn execute_sqlite_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let sqlite_pool = match pool {
        models::enums::DatabasePool::SQLite(p) => p,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for SQLite".to_string(),
            ));
        }
    };

    let statements_raw: Vec<&str> = options
        .query
        .split(';')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    let mut ast_debug_sql: Option<String> = None;

    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = if statements_raw.len() == 1
        && statements_raw[0]
            .trim_start()
            .to_uppercase()
            .starts_with("SELECT")
    {
        let should_paginate = options.use_server_pagination
            && !query_contains_pagination(statements_raw[0]);
        let pagination_opt = if should_paginate {
            Some((options.current_page as u64, options.page_size as u64))
        } else {
            None
        };
        match crate::query_ast::compile_single_select(
            statements_raw[0],
            &options.connection.connection_type,
            pagination_opt,
            true,
        ) {
            Ok((new_sql, hdrs)) => {
                if !hdrs.is_empty() {
                    inferred_headers_from_ast = Some(hdrs.clone());
                    ast_headers = Some(hdrs.clone());
                }
                ast_debug_sql = Some(new_sql.clone());
                vec![new_sql]
            }
            Err(_) => statements_raw.iter().map(|s| s.to_string()).collect(),
        }
    } else {
        statements_raw.iter().map(|s| s.to_string()).collect()
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    let mut final_headers = Vec::new();
    let mut final_data = Vec::new();

    for (i, statement) in statements_ref.iter().enumerate() {
        let trimmed = statement.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") {
            continue;
        }

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            sqlx::query(trimmed).fetch_all(sqlite_pool.as_ref()),
        )
        .await;

        match result {
            Ok(Ok(rows)) => {
                if i == statements_ref.len() - 1 {
                    if !rows.is_empty() {
                        final_headers = rows[0]
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        final_data = rows
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
                    } else {
                        #[cfg(feature = "query_ast")]
                        if final_headers.is_empty()
                            && let Some(hh) = inferred_headers_from_ast.clone()
                            && !hh.is_empty()
                        {
                            final_headers = hh;
                        }
                        if final_headers.is_empty() && trimmed.to_uppercase().starts_with("SELECT")
                        {
                            let inferred = infer_select_headers(trimmed);
                            if !inferred.is_empty() {
                                final_headers = inferred;
                            }
                        }
                        final_data = Vec::new();
                    }
                }
            }
            Ok(Err(e)) => {
                return Err(QueryExecutionError::Message(format!("SQLite error: {}", e)));
            }
            Err(_) => {
                return Err(QueryExecutionError::Message(
                    "SQLite query timed out".to_string(),
                ));
            }
        }
    }

    Ok(QueryJobOutput {
        headers: final_headers,
        rows: final_data,
        ast_debug_sql,
        ast_headers,
    })
}

async fn execute_redis_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    use redis::AsyncCommands;

    let redis_manager = match pool {
        models::enums::DatabasePool::Redis(manager) => manager,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for Redis".to_string(),
            ));
        }
    };

    let command_line = options.query.trim();
    if command_line.is_empty() {
        return Err(QueryExecutionError::Message(
            "Empty Redis command".to_string(),
        ));
    }

    let mut connection = redis_manager.as_ref().clone();

    if let Some(db_name) = options.selected_database.as_ref() {
        let db_trim = db_name.trim();
        let candidate = if let Some(rest) = db_trim.strip_prefix("db") {
            rest
        } else if let Some(rest) = db_trim.strip_prefix("DB") {
            rest
        } else {
            db_trim
        };
        if let Ok(db_index) = candidate.parse::<i32>() {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                redis::cmd("SELECT")
                    .arg(db_index)
                    .query_async::<String>(&mut connection),
            )
            .await;
        }
    }

    debug!("[async] Executing Redis command: {}", command_line);

    let parts: Vec<&str> = command_line.split_whitespace().collect();
    if parts.is_empty() {
        return Err(QueryExecutionError::Message(
            "Empty Redis command".to_string(),
        ));
    }

    let command = parts[0].to_uppercase();
    match command.as_str() {
        "GET" => {
            if parts.len() != 2 {
                return Err(QueryExecutionError::Message(
                    "GET requires exactly one key".to_string(),
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                connection.get::<&str, Option<String>>(parts[1]),
            )
            .await
            {
                Ok(Ok(Some(value))) => Ok(QueryJobOutput {
                    headers: vec!["Key".to_string(), "Value".to_string()],
                    rows: vec![vec![parts[1].to_string(), value]],
                    ast_debug_sql: None,
                    ast_headers: None,
                }),
                Ok(Ok(None)) => Ok(QueryJobOutput {
                    headers: vec!["Key".to_string(), "Value".to_string()],
                    rows: vec![vec![parts[1].to_string(), "NULL".to_string()]],
                    ast_debug_sql: None,
                    ast_headers: None,
                }),
                _ => Err(QueryExecutionError::Message(
                    "Redis GET timed out or failed".to_string(),
                )),
            }
        }
        "KEYS" => {
            if parts.len() != 2 {
                return Err(QueryExecutionError::Message(
                    "KEYS requires exactly one pattern".to_string(),
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                connection.keys::<&str, Vec<String>>(parts[1]),
            )
            .await
            {
                Ok(Ok(keys)) => {
                    let table_data: Vec<Vec<String>> = keys.into_iter().map(|k| vec![k]).collect();
                    Ok(QueryJobOutput {
                        headers: vec!["Key".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                    })
                }
                _ => Err(QueryExecutionError::Message(
                    "Redis KEYS timed out or failed".to_string(),
                )),
            }
        }
        "SCAN" => {
            if parts.len() < 2 {
                return Err(QueryExecutionError::Message(
                    "SCAN requires cursor parameter".to_string(),
                ));
            }
            let cursor = parts[1];
            let mut match_pattern = "*";
            let mut count: i64 = 10;
            let mut idx = 2;
            while idx < parts.len() {
                match parts[idx].to_uppercase().as_str() {
                    "MATCH" => {
                        if idx + 1 < parts.len() {
                            match_pattern = parts[idx + 1];
                            idx += 2;
                        } else {
                            return Err(QueryExecutionError::Message(
                                "MATCH requires a pattern".to_string(),
                            ));
                        }
                    }
                    "COUNT" => {
                        if idx + 1 < parts.len() {
                            if let Ok(parsed) = parts[idx + 1].parse::<i64>() {
                                count = parsed;
                                idx += 2;
                            } else {
                                return Err(QueryExecutionError::Message(
                                    "COUNT must be a number".to_string(),
                                ));
                            }
                        } else {
                            return Err(QueryExecutionError::Message(
                                "COUNT requires a number".to_string(),
                            ));
                        }
                    }
                    other => {
                        return Err(QueryExecutionError::Message(format!(
                            "Unknown SCAN parameter: {}",
                            other
                        )));
                    }
                }
            }

            let mut cmd = redis::cmd("SCAN");
            cmd.arg(cursor);
            if match_pattern != "*" {
                cmd.arg("MATCH").arg(match_pattern);
            }
            cmd.arg("COUNT").arg(count);

            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                cmd.query_async::<(String, Vec<String>)>(&mut connection),
            )
            .await
            {
                Ok(Ok((next_cursor, keys))) => {
                    let mut table_data = Vec::new();
                    if keys.is_empty() {
                        table_data.push(vec![
                            "Info".to_string(),
                            format!("No keys found matching pattern: {}", match_pattern),
                        ]);
                        table_data.push(vec!["Cursor".to_string(), next_cursor.clone()]);
                        table_data.push(vec![
                            "Suggestion".to_string(),
                            "Try different pattern or use 'SCAN 0 COUNT 100' to see all keys"
                                .to_string(),
                        ]);
                        if match_pattern != "*"
                            && let Ok((_, sample_keys)) = redis::cmd("SCAN")
                                .arg("0")
                                .arg("COUNT")
                                .arg("10")
                                .query_async::<(String, Vec<String>)>(&mut connection)
                                .await
                            && !sample_keys.is_empty()
                        {
                            table_data.push(vec!["Sample Keys Found".to_string(), "".to_string()]);
                            for (i, key) in sample_keys.iter().take(5).enumerate() {
                                table_data.push(vec![format!("Sample {}", i + 1), key.clone()]);
                            }
                        }
                    } else {
                        table_data.push(vec!["CURSOR".to_string(), next_cursor]);
                        for key in keys {
                            table_data.push(vec!["KEY".to_string(), key]);
                        }
                    }
                    Ok(QueryJobOutput {
                        headers: vec!["Type".to_string(), "Value".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                    })
                }
                _ => Err(QueryExecutionError::Message(
                    "Redis SCAN timed out or failed".to_string(),
                )),
            }
        }
        "INFO" => {
            let section = if parts.len() > 1 { parts[1] } else { "default" };
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                redis::cmd("INFO")
                    .arg(section)
                    .query_async::<String>(&mut connection),
            )
            .await
            {
                Ok(Ok(info_result)) => {
                    let mut table_data = Vec::new();
                    for line in info_result.lines() {
                        if line.trim().is_empty() || line.starts_with('#') {
                            continue;
                        }
                        if let Some((key, value)) = line.split_once(':') {
                            table_data.push(vec![key.to_string(), value.to_string()]);
                        }
                    }
                    Ok(QueryJobOutput {
                        headers: vec!["Property".to_string(), "Value".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                    })
                }
                _ => Err(QueryExecutionError::Message(
                    "Redis INFO timed out or failed".to_string(),
                )),
            }
        }
        "HGETALL" => {
            if parts.len() != 2 {
                return Err(QueryExecutionError::Message(
                    "HGETALL requires exactly one key".to_string(),
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                redis::cmd("HGETALL")
                    .arg(parts[1])
                    .query_async::<Vec<String>>(&mut connection),
            )
            .await
            {
                Ok(Ok(hash_data)) => {
                    let mut table_data = Vec::new();
                    for chunk in hash_data.chunks(2) {
                        if chunk.len() == 2 {
                            table_data.push(vec![chunk[0].clone(), chunk[1].clone()]);
                        }
                    }
                    if table_data.is_empty() {
                        table_data.push(vec![
                            "No data".to_string(),
                            "Hash is empty or key does not exist".to_string(),
                        ]);
                    }
                    Ok(QueryJobOutput {
                        headers: vec!["Field".to_string(), "Value".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                    })
                }
                _ => Err(QueryExecutionError::Message(
                    "Redis HGETALL timed out or failed".to_string(),
                )),
            }
        }
        _ => Err(QueryExecutionError::Message(format!(
            "Unsupported Redis command: {}",
            parts[0]
        ))),
    }
}

async fn execute_mssql_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let config = match pool {
        models::enums::DatabasePool::MsSQL(cfg) => cfg,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for MsSQL".to_string(),
            ));
        }
    };

    let mut query_str = options.query.trim().to_string();
    if query_str.is_empty() {
        return Err(QueryExecutionError::Message(
            "Empty MsSQL query".to_string(),
        ));
    }

    if query_str.contains("TOP") && query_str.contains("ROWS FETCH NEXT") {
        query_str = query_str.replace("TOP 10000", "");
    }

    match driver_mssql::execute_query(config.clone(), &query_str).await {
        Ok((headers, rows)) => Ok(QueryJobOutput {
            headers,
            rows,
            ast_debug_sql: None,
            ast_headers: None,
        }),
        Err(e) => Err(QueryExecutionError::Message(format!("Query error: {}", e))),
    }
}

async fn execute_mongodb_query_job(
    _options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    match pool {
        models::enums::DatabasePool::MongoDB(_) => Ok(QueryJobOutput {
            headers: vec!["Info".to_string()],
            rows: vec![vec![
                "MongoDB query execution is not supported. Use tree to browse collections."
                    .to_string(),
            ]],
            ast_debug_sql: None,
            ast_headers: None,
        }),
        _ => Err(QueryExecutionError::Message(
            "Invalid pool type for MongoDB".to_string(),
        )),
    }
}

fn resolve_connection_target(
    connection: &models::structs::ConnectionConfig,
) -> Result<(String, String), String> {
    if connection.ssh_enabled {
        match connection.connection_type {
            models::enums::DatabaseType::SQLite => {
                Err("SSH tunnel is not supported for SQLite connections".to_string())
            }
            _ => {
                let local_port = ssh_tunnel::ensure_tunnel(connection)?;
                Ok(("127.0.0.1".to_string(), local_port.to_string()))
            }
        }
    } else {
        Ok((connection.host.clone(), connection.port.clone()))
    }
}

// Infer column headers from a SELECT statement when no rows are returned.
// This is a best-effort parser handling simple SELECT lists (supports aliases, functions, qualified names).
fn infer_select_headers(statement: &str) -> Vec<String> {
    let lower = statement.to_lowercase();
    let select_pos = match lower.find("select") {
        Some(p) => p,
        None => return Vec::new(),
    };
    // Find the matching FROM outside parentheses
    let mut depth = 0usize;
    let mut from_pos: Option<usize> = None;
    for (i, ch) in statement.chars().enumerate().skip(select_pos + 6) {
        // after 'select'
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && i + 4 <= statement.len() && lower[i..].starts_with("from") {
            from_pos = Some(i);
            break;
        }
    }
    let from_pos = match from_pos {
        Some(p) => p,
        None => return Vec::new(),
    };
    let select_list = &statement[select_pos + 6..from_pos];
    // Split by commas at top level (ignore commas inside parentheses)
    let mut headers = Vec::new();
    let mut current = String::new();
    depth = 0;
    for ch in select_list.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let h = extract_alias_or_name(&current);
                if !h.is_empty() {
                    headers.push(h);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        let h = extract_alias_or_name(&current);
        if !h.is_empty() {
            headers.push(h);
        }
    }
    headers
}

fn extract_alias_or_name(fragment: &str) -> String {
    let frag = fragment.trim();
    if frag.is_empty() {
        return String::new();
    }
    let lower = frag.to_lowercase();
    if let Some(as_pos) = lower.rfind(" as ") {
        // alias with AS
        let alias = frag[as_pos + 4..].trim();
        return clean_identifier(alias);
    }
    // Alias without AS: take last token after space if it is not a function call
    let tokens: Vec<&str> = frag.split_whitespace().collect();
    if tokens.len() > 1 {
        let last = tokens.last().unwrap();
        // Avoid returning keywords or expressions
        if !last.contains('(') && !["distinct"].contains(&last.to_lowercase().as_str()) {
            return clean_identifier(last);
        }
    }
    // Otherwise, strip qualification
    if let Some(idx) = frag.rfind('.') {
        return clean_identifier(&frag[idx + 1..]);
    }
    clean_identifier(frag)
}

fn clean_identifier(id: &str) -> String {
    id.trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('[')
        .trim_matches(']')
        .to_string()
}

// Helper function to add auto LIMIT if not present
pub fn add_auto_limit_if_needed(query: &str, db_type: &models::enums::DatabaseType) -> String {
    let trimmed_query = query.trim();

    // Don't add LIMIT/TOP if the entire query already has LIMIT/TOP/OFFSET/FETCH
    // Detect regardless of whitespace/newlines using robust keyword boundaries
    if query_contains_pagination(trimmed_query) {
        return trimmed_query.to_string();
    }

    // Only operate on simple SELECT queries
    let upper_query = trimmed_query.to_uppercase();
    if !upper_query.starts_with("SELECT") {
        return trimmed_query.to_string();
    }

    match db_type {
        models::enums::DatabaseType::MsSQL => {
            // Insert TOP 1000 after SELECT if no TOP present
            if upper_query.starts_with("SELECT") {
                // Preserve casing after the SELECT keyword
                if let Some(rest) = trimmed_query.get(6..) {
                    return format!("SELECT TOP 5000{}", rest);
                }
            }
            trimmed_query.to_string()
        }
        _ => {
            // MySQL/PostgreSQL/SQLite/MongoDB/Redis: append LIMIT 5000
            format!("{} LIMIT 5000", trimmed_query)
        }
    }
}

pub(crate) fn execute_query_with_connection(
    tabular: &mut Tabular,
    connection_id: i64,
    query: String,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    debug!(
        "Query execution requested for connection {} with query: {}",
        connection_id, query
    );

    if let Some(connection) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .cloned()
    {
        // Determine selected database from active tab (if any)
        let selected_db = tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.database_name.clone())
            .filter(|s| !s.is_empty());

        // Auto-prepend USE for MsSQL/MySQL if not already present
        let mut final_query = query.clone();
        if let Some(db_name) = selected_db {
            match connection.connection_type {
                models::enums::DatabaseType::MsSQL => {
                    let upper = final_query.to_uppercase();
                    if !upper.starts_with("USE ") {
                        final_query = format!("USE [{}];\n{}", db_name, final_query);
                    }
                }
                models::enums::DatabaseType::MySQL => {
                    let upper = final_query.to_uppercase();
                    if !upper.starts_with("USE ") {
                        final_query = format!("USE `{}`;\n{}", db_name, final_query);
                    }
                }
                _ => {}
            }
        }

        // Server pagination fallback: if batch contains a SELECT without LIMIT/TOP/OFFSET/FETCH,
        // rewrite to paginated query and set pagination state (handles cases like "USE db; SELECT ...").
        {
            let upper = final_query.to_uppercase();
            let has_pagination_clause = query_contains_pagination(&final_query);
            let has_select_stmt = upper
                .split(';')
                .any(|s| s.trim_start().starts_with("SELECT"));

            if has_select_stmt && !has_pagination_clause {
                match connection.connection_type {
                    models::enums::DatabaseType::MySQL
                    | models::enums::DatabaseType::PostgreSQL
                    | models::enums::DatabaseType::SQLite => {
                        // Prepare base query and paginated query
                        let base = final_query.trim().trim_end_matches(';').to_string();
                        // Force-enable server pagination flags on UI state
                        tabular.use_server_pagination = true;
                        tabular.current_base_query = base.clone();
                        tabular.current_page = 0;
                        tabular.actual_total_rows = Some(10_000);
                        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                            tab.base_query = base.clone();
                            tab.current_page = tabular.current_page;
                            tab.page_size = tabular.page_size;
                        }
                        let offset = tabular.current_page * tabular.page_size;
                        final_query =
                            format!("{} LIMIT {} OFFSET {}", base, tabular.page_size, offset);
                        debug!(
                            "🛑 Auto server-pagination (connection layer) applied. Rewritten query: {}",
                            final_query
                        );
                    }
                    _ => {
                        // Non-SQL engines (Mongo/Redis/MsSQL handled elsewhere); do nothing here
                    }
                }
            } else {
                // Add auto LIMIT if still plain SELECT without clauses and not handled by pagination
                let original_query = final_query.clone();
                final_query = add_auto_limit_if_needed(&final_query, &connection.connection_type);
                if original_query != final_query {
                    debug!("Auto LIMIT applied. Original: {}", original_query);
                    debug!("Modified: {}", final_query);
                }
            }
        }

        execute_table_query_sync(tabular, connection_id, &connection, &final_query)
    } else {
        debug!("Connection not found for ID: {}", connection_id);
        None
    }
}

pub(crate) fn execute_table_query_sync(
    tabular: &mut Tabular,
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    query: &str,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    debug!("Executing query synchronously: {}", query);

    // Use the shared runtime from tabular instead of creating a new one
    let runtime = match &tabular.runtime {
        Some(rt) => rt.clone(),
        None => {
            debug!("No runtime available, creating temporary one");
            match tokio::runtime::Runtime::new() {
                Ok(rt) => Arc::new(rt),
                Err(e) => {
                    debug!("Failed to create runtime: {}", e);
                    return None;
                }
            }
        }
    };

    runtime.block_on(async {
        match try_get_connection_pool(tabular, connection_id).await {
            Some(pool) => {
                match pool {
                    models::enums::DatabasePool::MySQL(_mysql_pool) => {
                        debug!("Executing MySQL query: {}", query);

                        let (target_host, target_port) = match resolve_connection_target(connection) {
                            Ok(tuple) => tuple,
                            Err(err) => {
                                return Some((
                                    vec!["Error".to_string()],
                                    vec![vec![format!(
                                        "Failed to resolve MySQL connection target: {}",
                                        err
                                    )]],
                                ));
                            }
                        };

                        // Split into statements
                        let statements: Vec<&str> = query
                            .split(';')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .collect();
                        // Phase 1 AST integration: if exactly one statement and starts with SELECT, try compile_single_select
                        #[cfg(feature = "query_ast")]
                        let mut _inferred_headers_from_ast: Option<Vec<String>> = None;
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<String> = if statements.len() == 1 && statements[0].to_uppercase().starts_with("SELECT") {
                            let should_paginate = tabular.use_server_pagination && !query_contains_pagination(statements[0]);
                            let pagination_opt = if should_paginate { Some((tabular.current_page as u64, tabular.page_size as u64)) } else { None };
                            match crate::query_ast::compile_single_select(statements[0], &connection.connection_type, pagination_opt, true) {
                                Ok((new_sql, hdrs)) => {
                                    if !hdrs.is_empty() { _inferred_headers_from_ast = Some(hdrs.clone()); }
                                    // Store debug info for UI panel
                                    tabular.last_compiled_sql = Some(new_sql.clone());
                                    tabular.last_compiled_headers = hdrs.clone();
                                    if let Ok(plan_txt) = crate::query_ast::debug_plan(statements[0], &connection.connection_type) { tabular.last_debug_plan = Some(plan_txt); }
                                    let (h,m) = crate::query_ast::cache_stats(); tabular.last_cache_hits = h; tabular.last_cache_misses = m;
                                    vec![new_sql] },
                                Err(_e) => statements.iter().map(|s| s.to_string()).collect(),
                            }
                        } else { statements.iter().map(|s| s.to_string()).collect() };
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        debug!("Found {} SQL statements to execute", statements.len());
                        for (idx, stmt) in statements.iter().enumerate() {
                            debug!("Statement {}: '{}'", idx + 1, stmt);
                        }

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();
                        // Determine special DBA mode from active tab (no SQL comment markers)
                        let (replication_status_mode, master_status_mode) = {
                            if let Some(active_tab) = tabular.query_tabs.get(tabular.active_tab_index) {
                                match active_tab.dba_special_mode {
                                    Some(models::enums::DBASpecialMode::ReplicationStatus) => (true, false),
                                    Some(models::enums::DBASpecialMode::MasterStatus) => (false, true),
                                    _ => (false, false)
                                }
                            } else { (false,false) }
                        };

                        let mut attempts = 0;
                        let max_attempts = 3;
                        while attempts < max_attempts {
                            attempts += 1;
                            let mut execution_success = true;
                            let mut error_message = String::new();
                            // Open a dedicated connection; we'll reconnect on USE to switch DB
                            let encoded_username = modules::url_encode(&connection.username);
                            let encoded_password = modules::url_encode(&connection.password);
                            let dsn = format!(
                                "mysql://{}:{}@{}:{}/{}",
                                encoded_username, encoded_password, target_host, target_port, connection.database
                            );
                            let mut conn = match MySqlConnection::connect(&dsn).await {
                                Ok(c) => c,
                                Err(e) => {
                                    error_message = e.to_string();
                                    debug!("Failed to open MySQL connection: {}", error_message);
                                    if attempts >= max_attempts { break; } else { continue; }
                                }
                            };
                            // Align dedicated connection session settings with pool settings
                            let _ = sqlx::query("SET SESSION wait_timeout = 600").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION interactive_timeout = 600").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION net_read_timeout = 120").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION net_write_timeout = 120").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824").execute(&mut conn).await; // 1GB
                            let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'").execute(&mut conn).await;

                            for (i, statement) in statements.iter().enumerate() {
                                let trimmed = statement.trim();
                                // Skip empty or comment-only statements (MySQL supports '--', '#', and '/* ... */' comments)
                                if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with('#') || trimmed.starts_with("/*") {
                                    debug!("Skipping statement {}: '{}'", i + 1, trimmed);
                                    continue;
                                }
                                debug!("Executing statement {}: '{}'", i + 1, trimmed);
                                let upper = trimmed.to_uppercase();

                                if upper.starts_with("USE ") {
                                    // Parse target database name
                                    let db_part = trimmed[3..].trim();
                                    let db_name = db_part
                                        .trim_matches('`')
                                        .trim_matches('\"')
                                        .trim_matches('[')
                                        .trim_matches(']')
                                        .trim();

                                    // Try to execute USE statement directly first (faster)
                                    match sqlx::query(&format!("USE `{}`", db_name)).execute(&mut conn).await {
                                        Ok(_) => {
                                            debug!("✅ Switched MySQL database using USE statement to '{}'.", db_name);
                                        }
                                        Err(_) => {
                                            // Fallback: reconnect only if USE statement fails
                                            debug!("⚠️ USE statement failed, falling back to reconnection...");
                                            let new_dsn = format!(
                                                "mysql://{}:{}@{}:{}/{}",
                                                encoded_username, encoded_password, target_host, target_port, db_name
                                            );
                                            match MySqlConnection::connect(&new_dsn).await {
                                                Ok(new_conn) => {
                                                    debug!("🔄 Switched MySQL database by reconnecting to '{}'.", db_name);
                                                    let mut new_conn = new_conn;
                                                    // Re-apply session settings on reconnected session
                                                    let _ = sqlx::query("SET SESSION wait_timeout = 600").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION interactive_timeout = 600").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION net_read_timeout = 120").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION net_write_timeout = 120").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'").execute(&mut new_conn).await;
                                                    conn = new_conn;
                                                }
                                                Err(e) => {
                                                    error_message = format!("USE failed (reconnect): {}", e);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    continue;
                                }

                                // Extend timeout to 60s to avoid premature timeouts for heavy queries
                                let query_result = tokio::time::timeout(
                                    std::time::Duration::from_secs(60),
                                    sqlx::query(trimmed).fetch_all(&mut conn)
                                ).await;
                                match query_result {
                                    Ok(Ok(rows)) => {
                                        // Log query execution time and row count for performance monitoring
                                        debug!("✅ Query executed successfully: {} rows returned", rows.len());

                                        if i == statements.len() - 1 {
                                            // Get headers from metadata, even if no rows
                                            if !rows.is_empty() {
                                                final_headers = rows[0]
                                                    .columns()
                                                    .iter()
                                                    .map(|c| c.name().to_string())
                                                    .collect();
                                                final_data = driver_mysql::convert_mysql_rows_to_table_data(rows);
                                                // Post-processing for replication/master status views
                                                if replication_status_mode || master_status_mode {
                                                    // Detect version / engine
                                                    let version_str = match sqlx::query("SELECT VERSION() AS v").fetch_one(&mut conn).await {
                                                        Ok(vrow) => vrow.try_get::<String,_>("v").unwrap_or_default(),
                                                        Err(_) => String::new(),
                                                    };
                                                    let is_mariadb = version_str.to_lowercase().contains("mariadb");
                                                    // Fallback: if replication status empty, try legacy SHOW SLAVE STATUS
                                                    if replication_status_mode && final_data.is_empty()
                                                        && let Ok(fallback_rows) = sqlx::query("SHOW SLAVE STATUS").fetch_all(&mut conn).await
                                                            && !fallback_rows.is_empty() {
                                                                final_headers = fallback_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                final_data = driver_mysql::convert_mysql_rows_to_table_data(fallback_rows);
                                }

                                                    // Build summary metrics (simple overlay table)
                                                    if !final_headers.is_empty() && !final_data.is_empty() {
                                                        let header_index = |name: &str| final_headers.iter().position(|h| h.eq_ignore_ascii_case(name));
                                                        let mut summary: Vec<(String,String)> = Vec::new();
                                                        if replication_status_mode {
                                                            let first = &final_data[0];
                                                            if let Some(idx) = header_index("Replica_IO_Running").or_else(|| header_index("Slave_IO_Running")) { summary.push(("IO Thread".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Replica_SQL_Running").or_else(|| header_index("Slave_SQL_Running")) { summary.push(("SQL Thread".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Seconds_Behind_Source").or_else(|| header_index("Seconds_Behind_Master")) { summary.push(("Seconds Behind".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Channel_Name") { summary.push(("Channel".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Retrieved_Gtid_Set") { summary.push(("Retrieved GTID".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Executed_Gtid_Set") { summary.push(("Executed GTID".into(), first[idx].clone())); }
                                                        }
                                                        if master_status_mode {
                                                            let first = &final_data[0];
                                                            if let Some(idx) = header_index("File") { summary.push(("Binary Log File".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Position") { summary.push(("Position".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Binlog_Do_DB") { summary.push(("Binlog Do DB".into(), first[idx].clone())); }
                                                            if let Some(idx) = header_index("Binlog_Ignore_DB") { summary.push(("Binlog Ignore DB".into(), first[idx].clone())); }
                                                        }
                                                        if !summary.is_empty() {
                                                            let mut summary_table: Vec<Vec<String>> = summary
                                                                .into_iter()
                                                                .map(|(metric, value)| vec![metric, value])
                                                                .collect();
                                                            summary_table.push(vec!["Server Version".into(), version_str.clone()]);
                                                            summary_table.push(vec!["Engine".into(), if is_mariadb { "MariaDB".into() } else { "MySQL".into() }]);
                                                            // Replace headers/data with summary view (keep original raw query accessible by re-running manually)
                                                            final_headers = vec!["Metric".into(), "Value".into()];
                                                            final_data = summary_table;
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Zero rows: try to use AST headers first (if any)
                                                #[cfg(feature = "query_ast")]
                                                if final_headers.is_empty() && let Some(hh) = _inferred_headers_from_ast.clone() && !hh.is_empty() { final_headers = hh; }
                                                // Fallback: infer headers from SELECT list first
                                                if trimmed.to_uppercase().starts_with("SELECT") {
                                                    let inferred = infer_select_headers(trimmed);
                                                    if !inferred.is_empty() { final_headers = inferred; }
                                                }
                                                // For MySQL, try to get column info using DESCRIBE if it's a table query (fallback)
                                                if trimmed.to_uppercase().contains("FROM") {
                                                    // Extract table name for DESCRIBE
                                                    let words: Vec<&str> = trimmed.split_whitespace().collect();
                                                    if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM")
                                                        && let Some(table_name) = words.get(from_idx + 1) {
                                                        let describe_query = format!("DESCRIBE {}", table_name);
                                                        match tokio::time::timeout(
                                                            std::time::Duration::from_secs(30),
                                                            sqlx::query(&describe_query).fetch_all(&mut conn),
                                                        )
                                                        .await
                                                        {
                                                            Ok(Ok(desc_rows)) => {
                                                                if !desc_rows.is_empty() {
                                                                    // For DESCRIBE, the first column contains field names
                                                                    final_headers = desc_rows.iter().map(|row| {
                                                                        row.try_get::<String, _>(0).unwrap_or_else(|_| "Field".to_string())
                                                                    }).collect();
                                                                }
                                                            }
                                                            _ => {
                                                                // DESCRIBE failed, try LIMIT 0 as fallback
                                                                let info_query = format!("{} LIMIT 0", trimmed);
                                                                match tokio::time::timeout(
                                                                    std::time::Duration::from_secs(30),
                                                                    sqlx::query(&info_query).fetch_all(&mut conn),
                                                                )
                                                                .await
                                                                {
                                                                    Ok(Ok(info_rows)) => {
                                                                        if !info_rows.is_empty() {
                                                                            final_headers = info_rows[0]
                                                                                .columns()
                                                                                .iter()
                                                                                .map(|c| c.name().to_string())
                                                                                .collect();
                                                                        }
                                                                    }
                                                                    _ => {
                                                                        // Both methods failed
                                                                        final_headers = Vec::new();
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    // Non-table query, just return empty result
                                                    final_headers = Vec::new();
                                                }
                                                final_data = Vec::new(); // Empty data but possibly with headers
                                            }
                                        }
                                    }
                                    Ok(Err(e)) => {
                                        error_message = e.to_string();
                                        execution_success = false;
                                        break;
                                    }
                                    Err(_) => {
                                        error_message = "Query timeout after 60s".to_string();
                                        execution_success = false;
                                        break;
                                    }
                                }
                            }

                            if execution_success {
                                return Some((final_headers, final_data));
                            } else {
                                debug!("MySQL query failed on attempt {}: {}", attempts, error_message);
                                if (error_message.contains("timeout") || error_message.contains("pool")) && attempts < max_attempts {
                                    tabular.connection_pools.remove(&connection_id);
                                    continue;
                                }
                                if attempts >= max_attempts {
                                    return Some((
                                        vec!["Error".to_string()],
                                        vec![vec![format!("Query error: {}", error_message)]]
                                    ));
                                }
                            }
                        }

                        Some((
                            vec!["Error".to_string()],
                            vec![vec!["Failed to execute query after multiple attempts".to_string()]]
                        ))
                    }
                    models::enums::DatabasePool::PostgreSQL(pg_pool) => {
                        debug!("Executing PostgreSQL query: {}", query);
                        let statements: Vec<&str> = query
                            .split(';')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .collect();
                        #[cfg(feature = "query_ast")]
                        let mut _inferred_headers_from_ast: Option<Vec<String>> = None;
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<String> = if statements.len() == 1 && statements[0].to_uppercase().starts_with("SELECT") {
                            let should_paginate = tabular.use_server_pagination && !query_contains_pagination(statements[0]);
                            let pagination_opt = if should_paginate { Some((tabular.current_page as u64, tabular.page_size as u64)) } else { None };
                            match crate::query_ast::compile_single_select(statements[0], &connection.connection_type, pagination_opt, true) {
                                Ok((new_sql, hdrs)) => {
                                    if !hdrs.is_empty() { _inferred_headers_from_ast = Some(hdrs.clone()); }
                                    tabular.last_compiled_sql = Some(new_sql.clone());
                                    tabular.last_compiled_headers = hdrs.clone();
                                    if let Ok(plan_txt) = crate::query_ast::debug_plan(statements[0], &connection.connection_type) { tabular.last_debug_plan = Some(plan_txt); }
                                    let (h,m) = crate::query_ast::cache_stats(); tabular.last_cache_hits = h; tabular.last_cache_misses = m;
                                    vec![new_sql] },
                                Err(_)=> statements.iter().map(|s| s.to_string()).collect(),
                            }
                        } else { statements.iter().map(|s| s.to_string()).collect() };
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

                        for (i, statement) in statements.iter().enumerate() {
                            let trimmed = statement.trim();
                            // Skip empty or comment-only statements
                            if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") { continue; }
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                sqlx::query(trimmed).fetch_all(pg_pool.as_ref()),
                            )
                            .await
                            {
                                Ok(Ok(rows)) => {
                                    if i == statements.len() - 1 {
                                        // For the last statement, try to get headers even if no rows
                                        if !rows.is_empty() {
                                            final_headers = rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                            final_data = rows.iter().map(|row| {
                                                (0..row.len()).map(|j| match row.try_get::<Option<String>, _>(j) {
                                                    Ok(Some(v)) => v,
                                                    Ok(None) => "NULL".to_string(),
                                                    Err(_) => "Error".to_string(),
                                                }).collect()
                                            }).collect();
                                        } else {
                                            // Zero rows: attempt AST headers first
                                            #[cfg(feature = "query_ast")]
                                            if final_headers.is_empty() && let Some(hh) = _inferred_headers_from_ast.clone() && !hh.is_empty() { final_headers = hh; }
                                            // Fallback to SELECT list heuristics
                                            if statement.to_uppercase().starts_with("SELECT") {
                                                let inferred = infer_select_headers(statement);
                                                if !inferred.is_empty() { final_headers = inferred; }
                                            }
                                            // For PostgreSQL, try to get column info from information_schema
                                            if statement.to_uppercase().contains("FROM") {
                                                // Extract table name for information_schema query
                                                let words: Vec<&str> = statement.split_whitespace().collect();
                                                if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM")
                                                    && let Some(table_name) = words.get(from_idx + 1) {
                                                    let clean_table = table_name.trim_matches('"').trim_matches('`');
                                                    let info_query = format!(
                                                        "SELECT column_name FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                                                        clean_table
                                                    );
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        sqlx::query(&info_query).fetch_all(pg_pool.as_ref()),
                                                    )
                                                    .await
                                                    {
                                                        Ok(Ok(info_rows)) => {
                                                            final_headers = info_rows.iter().map(|row| {
                                                                match row.try_get::<String, _>(0) {
                                                                    Ok(col_name) => col_name,
                                                                    Err(_) => "Column".to_string(),
                                                                }
                                                            }).collect();
                                                        }
                                                        _ => {
                                                            // information_schema failed, try LIMIT 0 as fallback
                                                            let limit_query = format!("{} LIMIT 0", statement);
                                                            match tokio::time::timeout(
                                                                std::time::Duration::from_secs(10),
                                                                sqlx::query(&limit_query).fetch_all(pg_pool.as_ref()),
                                                            )
                                                            .await
                                                            {
                                                                Ok(Ok(limit_rows)) => {
                                                                    if !limit_rows.is_empty() {
                                                                        final_headers = limit_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                    }
                                                                }
                                                                _ => {
                                                                    if final_headers.is_empty() { final_headers = infer_select_headers(statement); }
                                                                    if final_headers.is_empty() { final_headers = Vec::new(); }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Non-table query, just return empty result
                                                final_headers = Vec::new();
                                            }
                                            final_data = Vec::new(); // Empty data but possibly with headers
                                        }
                                    }
                                }
                                _ => {
                                    return Some((vec!["Error".to_string()], vec![vec!["Query timed out or failed".to_string()]]));
                                }
                            }
                        }

                        Some((final_headers, final_data))
                    }
                    models::enums::DatabasePool::SQLite(sqlite_pool) => {
                        debug!("Executing SQLite query: {}", query);
                        let statements: Vec<&str> = query
                            .split(';')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .collect();
                        #[cfg(feature = "query_ast")]
                        let mut _inferred_headers_from_ast: Option<Vec<String>> = None;
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<String> = if statements.len() == 1 && statements[0].to_uppercase().starts_with("SELECT") {
                            let should_paginate = tabular.use_server_pagination && !query_contains_pagination(statements[0]);
                            let pagination_opt = if should_paginate { Some((tabular.current_page as u64, tabular.page_size as u64)) } else { None };
                            match crate::query_ast::compile_single_select(statements[0], &connection.connection_type, pagination_opt, true) {
                                Ok((new_sql, hdrs)) => { if !hdrs.is_empty() { _inferred_headers_from_ast = Some(hdrs); } vec![new_sql] },
                                Err(_)=> statements.iter().map(|s| s.to_string()).collect(),
                            }
                        } else { statements.iter().map(|s| s.to_string()).collect() };
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

                        for (i, statement) in statements.iter().enumerate() {
                            let trimmed = statement.trim();
                            // Skip empty or comment-only statements
                            if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") { continue; }
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                sqlx::query(trimmed).fetch_all(sqlite_pool.as_ref()),
                            )
                            .await
                            {
                                Ok(Ok(rows)) => {
                                    if i == statements.len() - 1 {
                                        // For the last statement, try to get headers even if no rows
                                        if !rows.is_empty() {
                                            final_headers = rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                            final_data = driver_sqlite::convert_sqlite_rows_to_table_data(rows);
                                        } else {
                                            // Zero rows: attempt AST headers first
                                            #[cfg(feature = "query_ast")]
                                            if final_headers.is_empty() && let Some(hh) = _inferred_headers_from_ast.clone() && !hh.is_empty() { final_headers = hh; }
                                            // Fallback to SELECT list heuristics
                                            if statement.to_uppercase().starts_with("SELECT") {
                                                let inferred = infer_select_headers(statement);
                                                if !inferred.is_empty() { final_headers = inferred; }
                                            }
                                            // For SQLite, try to get column info using PRAGMA table_info
                                            if statement.to_uppercase().contains("FROM") {
                                                // Extract table name for PRAGMA table_info
                                                let words: Vec<&str> = statement.split_whitespace().collect();
                                                if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM")
                                                    && let Some(table_name) = words.get(from_idx + 1) {
                                                    let clean_table = table_name.trim_matches('"').trim_matches('`').trim_matches('[').trim_matches(']');
                                                    let pragma_query = format!("PRAGMA table_info({})", clean_table);
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        sqlx::query(&pragma_query).fetch_all(sqlite_pool.as_ref()),
                                                    )
                                                    .await
                                                    {
                                                        Ok(Ok(pragma_rows)) => {
                                                            final_headers = pragma_rows.iter().map(|row| {
                                                                // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
                                                                // We want the name column (index 1)
                                                                match row.try_get::<String, _>(1) {
                                                                    Ok(col_name) => col_name,
                                                                    Err(_) => "Column".to_string(),
                                                                }
                                                            }).collect();
                                                        }
                                                        _ => {
                                                            // PRAGMA failed, try LIMIT 0 as fallback
                                                            let limit_query = format!("{} LIMIT 0", statement);
                                                            match tokio::time::timeout(
                                                                std::time::Duration::from_secs(10),
                                                                sqlx::query(&limit_query).fetch_all(sqlite_pool.as_ref()),
                                                            )
                                                            .await
                                                            {
                                                                Ok(Ok(limit_rows)) => {
                                                                    if !limit_rows.is_empty() {
                                                                        final_headers = limit_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                    }
                                                                }
                                                                _ => {
                                                                    // Both methods failed
                                                                    final_headers = Vec::new();
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                // Non-table query, just return empty result
                                                final_headers = Vec::new();
                                            }
                                            final_data = Vec::new(); // Empty data but possibly with headers
                                        }
                                    }
                                }
                                _ => {
                                    return Some((vec!["Error".to_string()], vec![vec!["Query timed out or failed".to_string()]]));
                                }
                            }
                        }

                        Some((final_headers, final_data))
                    }
                    models::enums::DatabasePool::Redis(redis_manager) => {
                        debug!("Executing Redis command: {}", query);
                        let mut connection = redis_manager.as_ref().clone();
                        use redis::AsyncCommands;

                        let parts: Vec<&str> = query.split_whitespace().collect();
                        if parts.is_empty() {
                            return Some((vec!["Error".to_string()], vec![vec!["Empty command".to_string()]]));
                        }

                        match parts[0].to_uppercase().as_str() {
                            "GET" => {
                                if parts.len() != 2 {
                                    return Some((vec!["Error".to_string()], vec![vec!["GET requires exactly one key".to_string()]]));
                                }
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    connection.get::<&str, Option<String>>(parts[1]),
                                )
                                .await
                                {
                                    Ok(Ok(Some(value))) => Some((vec!["Key".to_string(), "Value".to_string()], vec![vec![parts[1].to_string(), value]])),
                                    Ok(Ok(None)) => Some((vec!["Key".to_string(), "Value".to_string()], vec![vec![parts[1].to_string(), "NULL".to_string()]])),
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis GET timed out or failed".to_string()]])),
                                }
                            }
                            "KEYS" => {
                                if parts.len() != 2 {
                                    return Some((vec!["Error".to_string()], vec![vec!["KEYS requires exactly one pattern".to_string()]]));
                                }
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    connection.keys::<&str, Vec<String>>(parts[1]),
                                )
                                .await
                                {
                                    Ok(Ok(keys)) => {
                                        let table_data: Vec<Vec<String>> = keys.into_iter().map(|k| vec![k]).collect();
                                        Some((vec!["Key".to_string()], table_data))
                                    }
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis KEYS timed out or failed".to_string()]])),
                                }
                            }
                            "SCAN" => {
                                if parts.len() < 2 {
                                    return Some((vec!["Error".to_string()], vec![vec!["SCAN requires cursor parameter".to_string()]]));
                                }
                                let cursor = parts[1];
                                let mut match_pattern = "*";
                                let mut count: i64 = 10;
                                let mut i = 2;
                                while i < parts.len() {
                                    match parts[i].to_uppercase().as_str() {
                                        "MATCH" => {
                                            if i + 1 < parts.len() {
                                                match_pattern = parts[i + 1];
                                                i += 2;
                                            } else { return Some((vec!["Error".to_string()], vec![vec!["MATCH requires a pattern".to_string()]])); }
                                        }
                                        "COUNT" => {
                                            if i + 1 < parts.len() {
                                                if let Ok(c) = parts[i + 1].parse::<i64>() {
                                                    count = c;
                                                    i += 2;
                                                } else { return Some((vec!["Error".to_string()], vec![vec!["COUNT must be a number".to_string()]])); }
                                            } else { return Some((vec!["Error".to_string()], vec![vec!["COUNT requires a number".to_string()]])); }
                                        }
                                        _ => { return Some((vec!["Error".to_string()], vec![vec![format!("Unknown SCAN parameter: {}", parts[i])]])); }
                                    }
                                }

                                let mut cmd = redis::cmd("SCAN");
                                cmd.arg(cursor);
                                if match_pattern != "*" { cmd.arg("MATCH").arg(match_pattern); }
                                cmd.arg("COUNT").arg(count);

                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    cmd.query_async::<(String, Vec<String>)>(&mut connection),
                                )
                                .await
                                {
                                    Ok(Ok((next_cursor, keys))) => {
                                        let mut table_data = Vec::new();
                                        if keys.is_empty() {
                                            table_data.push(vec!["Info".to_string(), format!("No keys found matching pattern: {}", match_pattern)]);
                                            table_data.push(vec!["Cursor".to_string(), next_cursor.clone()]);
                                            table_data.push(vec!["Suggestion".to_string(), "Try different pattern or use 'SCAN 0 COUNT 100' to see all keys".to_string()]);
                                            if match_pattern != "*" && let Ok((_, sample_keys)) = redis::cmd("SCAN").arg("0").arg("COUNT").arg("10").query_async::<(String, Vec<String>)>(&mut connection).await
                                                && !sample_keys.is_empty() {
                                                table_data.push(vec!["Sample Keys Found".to_string(), "".to_string()]);
                                                for (i, key) in sample_keys.iter().take(5).enumerate() { table_data.push(vec![format!("Sample {}", i + 1), key.clone()]); }
                                            }
                                        } else {
                                            table_data.push(vec!["CURSOR".to_string(), next_cursor]);
                                            for key in keys { table_data.push(vec!["KEY".to_string(), key]); }
                                        }
                                        Some((vec!["Type".to_string(), "Value".to_string()], table_data))
                                    }
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis SCAN timed out or failed".to_string()]])),
                                }
                            }
                            "INFO" => {
                                let section = if parts.len() > 1 { parts[1] } else { "default" };
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    redis::cmd("INFO").arg(section).query_async::<String>(&mut connection),
                                )
                                .await
                                {
                                    Ok(Ok(info_result)) => {
                                        let mut table_data = Vec::new();
                                        for line in info_result.lines() {
                                            if line.trim().is_empty() || line.starts_with('#') { continue; }
                                            if let Some((key, value)) = line.split_once(':') { table_data.push(vec![key.to_string(), value.to_string()]); }
                                        }
                                        Some((vec!["Property".to_string(), "Value".to_string()], table_data))
                                    }
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis INFO timed out or failed".to_string()]])),
                                }
                            }
                            "HGETALL" => {
                                if parts.len() != 2 { return Some((vec!["Error".to_string()], vec![vec!["HGETALL requires exactly one key".to_string()]])); }
                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(10),
                                    redis::cmd("HGETALL").arg(parts[1]).query_async::<Vec<String>>(&mut connection),
                                )
                                .await
                                {
                                    Ok(Ok(hash_data)) => {
                                        let mut table_data = Vec::new();
                                        for chunk in hash_data.chunks(2) { if chunk.len() == 2 { table_data.push(vec![chunk[0].clone(), chunk[1].clone()]); } }
                                        if table_data.is_empty() { table_data.push(vec!["No data".to_string(), "Hash is empty or key does not exist".to_string()]); }
                                        Some((vec!["Field".to_string(), "Value".to_string()], table_data))
                                    }
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis HGETALL timed out or failed".to_string()]])),
                                }
                            }
                            _ => Some((vec!["Error".to_string()], vec![vec![format!("Unsupported Redis command: {}", parts[0])]])),
                        }
                    }
                    models::enums::DatabasePool::MsSQL(mssql_cfg) => {
                        debug!("Executing MsSQL query: {}", query);
                        let mut query_str = query.to_string();
                        if query_str.contains("TOP") && query_str.contains("ROWS FETCH NEXT") {
                            query_str = query_str.replace("TOP 10000", "");
                        }
                        match driver_mssql::execute_query(mssql_cfg.clone(), &query_str).await {
                            Ok((h, d)) => Some((h, d)),
                            Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Query error: {}", e)]])),
                        }
                    }
                    models::enums::DatabasePool::MongoDB(_client) => {
                        // For now, MongoDB queries are not supported via SQL editor. Provide hint.
                        Some((vec!["Info".to_string()], vec![vec!["MongoDB query execution is not supported. Use tree to browse collections.".to_string()]]))
                    }
                }
            }
            None => {
                debug!("Failed to get connection pool for connection_id: {}", connection_id);
                Some((vec!["Error".to_string()], vec![vec!["Failed to connect to database".to_string()]]))
            }
        }
    })
}

// Helper function to clean up completed background pools
fn cleanup_completed_background_pools(tabular: &mut Tabular) {
    if let Ok(shared_pools) = tabular.shared_connection_pools.lock() {
        for connection_id in shared_pools.keys() {
            if tabular.pending_connection_pools.contains(connection_id) {
                debug!(
                    "🧹 Cleaning up completed background pool for connection {}",
                    connection_id
                );
                tabular.pending_connection_pools.remove(connection_id);
            }
        }
    }
}

// Force cleanup of stuck pending connections (safety net)
fn cleanup_stuck_pending_connections(tabular: &mut Tabular) {
    // Remove any connection that's been pending too long to prevent permanent locks
    // This is a safety net in case background tasks fail to complete
    if !tabular.pending_connection_pools.is_empty() {
        let stuck_connections: Vec<i64> =
            tabular.pending_connection_pools.iter().copied().collect();
        for connection_id in stuck_connections {
            // Check if we have the pool in shared pools or local cache
            let has_pool = tabular.connection_pools.contains_key(&connection_id)
                || tabular
                    .shared_connection_pools
                    .lock()
                    .is_ok_and(|pools| pools.contains_key(&connection_id));

            if has_pool {
                debug!(
                    "🧹 Removing stuck pending status for connection {} (pool exists)",
                    connection_id
                );
                tabular.pending_connection_pools.remove(&connection_id);
            }
        }
    }
}

// Render a connection selector popup when the user tries to execute a query without a connection.
// Shows a simple modal listing available connections; selecting one assigns it to the active tab
// and (optionally) auto-executes the pending query captured earlier.
pub(crate) fn render_connection_selector(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_connection_selector {
        return;
    }

    // If no connections configured, show guidance with quick action
    if tabular.connections.is_empty() {
        let mut open = tabular.show_connection_selector;
        egui::Window::new("No Connections Available")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .title_bar(true)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Belum ada koneksi tersimpan. Tambahkan koneksi terlebih dahulu.");
                ui.horizontal(|ui| {
                    if ui.button("Add new connection").clicked() {
                        tabular.show_add_connection = true;
                        tabular.show_connection_selector = false;
                    }
                });
            });
        // Close when X is clicked
        if !open {
            tabular.show_connection_selector = false;
        }
        return;
    }

    // Keep a local filter text in temporary egui memory (per-session)
    let filter_id = egui::Id::new("conn_selector_filter");
    let mut filter_text = ctx
        .data(|d| d.get_temp::<String>(filter_id))
        .unwrap_or_default();

    let mut open = tabular.show_connection_selector;
    egui::Window::new("Connection Selector")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut filter_text)
                        .hint_text("type host / database / connection name...")
                        .desired_width(f32::INFINITY),
                );
                if r.changed() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(filter_id, filter_text.clone()));
                }
            });
            ui.separator();

            let mut items: Vec<_> = tabular.connections.clone();
            if !filter_text.trim().is_empty() {
                let f = filter_text.to_lowercase();
                items.retain(|c| {
                    c.name.to_lowercase().contains(&f)
                        || c.host.to_lowercase().contains(&f)
                        || c.database.to_lowercase().contains(&f)
                        || format!("{:?}", c.connection_type)
                            .to_lowercase()
                            .contains(&f)
                });
            }

            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    for conn in items.iter() {
                        // Build a friendly one-line summary
                        let title = format!(
                            "{} — {:?} @ {}:{}{}",
                            conn.name,
                            conn.connection_type,
                            conn.host,
                            conn.port,
                            if conn.database.is_empty() {
                                "".to_string()
                            } else {
                                format!(" / {}", conn.database)
                            }
                        );

                        // Row: make the connection name itself the button (single click)
                        let mut should_connect = false;
                        let lresp = ui.selectable_label(false, title);
                        if lresp.clicked() || lresp.double_clicked() {
                            should_connect = true;
                        }
                        ui.separator();

                        if should_connect {
                            if let Some(id) = conn.id {
                                // Assign connection to active tab and optionally its default database
                                if let Some(tab) =
                                    tabular.query_tabs.get_mut(tabular.active_tab_index)
                                {
                                    tab.connection_id = Some(id);
                                    if (tab.database_name.is_none()
                                        || tab.database_name.as_deref().unwrap_or("").is_empty())
                                        && !conn.database.is_empty()
                                    {
                                        tab.database_name = Some(conn.database.clone());
                                    }
                                }
                                // Also mirror to global current_connection_id for components that rely on it
                                tabular.current_connection_id = Some(id);
                                // Warm up pool in background (non-blocking)
                                ensure_background_pool_creation(tabular, id);

                                // Close the popup first to avoid re-entrancy issues
                                tabular.show_connection_selector = false;

                                // Auto execute pending query if requested
                                if tabular.auto_execute_after_connection {
                                    // Prefer executing through the editor path so pagination/history are handled consistently
                                    // Note: we do NOT need to inject pending_query; editor will re-derive from selection/cursor.
                                    // pending_query acts as a hint; we clear it afterwards.
                                    crate::editor::execute_query(tabular);
                                    tabular.auto_execute_after_connection = false;
                                    tabular.pending_query.clear();
                                }
                            }
                            // Exit early after selection to avoid processing remaining items this frame
                            break;
                        }
                    }
                });
        });
    // Close when X is clicked
    if !open {
        tabular.show_connection_selector = false;
    }
}

// Helper function untuk mendapatkan atau membuat connection pool dengan concurrency
//
// CONCURRENCY IMPROVEMENTS:
// 1. Tracks pending pool creation to avoid duplicate work (pending_connection_pools HashSet)
// 2. Uses background task spawning to prevent UI blocking
// 3. Provides immediate return for cached pools
// 4. Shared state mechanism for background-created pools
//
// BENEFITS:
// - User doesn't need to wait for slow connections when others are available
// - Prevents duplicate connection attempts for the same database
// - UI remains responsive during connection establishment
// - Background-created pools are shared and accessible
//
pub(crate) async fn get_or_create_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    // Clean up any completed background pools first
    cleanup_completed_background_pools(tabular);

    // Clean up any stuck pending connections (safety net)
    cleanup_stuck_pending_connections(tabular);

    // First check if we already have a cached connection pool for this connection
    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    // Check shared pools from background tasks
    if let Ok(shared_pools) = tabular.shared_connection_pools.lock()
        && let Some(shared_pool) = shared_pools.get(&connection_id)
    {
        debug!(
            "✅ Using background-created connection pool for connection {}",
            connection_id
        );
        let pool = shared_pool.clone();
        // Cache it locally for faster access next time
        tabular.connection_pools.insert(connection_id, pool.clone());
        // Remove from pending since we now have the pool
        tabular.pending_connection_pools.remove(&connection_id);
        return Some(pool);
    }

    // Check if we're already creating a pool for this connection to avoid duplicate work
    if tabular.pending_connection_pools.contains(&connection_id) {
        // Rate-limit log spam: only log at most once per second per connection
        let now = std::time::Instant::now();
        let should_log = match tabular.pending_pool_log_last.get(&connection_id) {
            Some(last) => now.duration_since(*last) > std::time::Duration::from_secs(1),
            None => true,
        };
        if should_log {
            debug!(
                "⏳ Connection pool creation already in progress for connection {}",
                connection_id
            );
            tabular.pending_pool_log_last.insert(connection_id, now);
        }
        return None; // Return None to indicate pool is being created
    }

    debug!(
        "🔄 Creating new connection pool for connection {}",
        connection_id
    );

    // Mark this connection as being processed
    tabular.pending_connection_pools.insert(connection_id);

    // Try quick creation first, fallback to background task if slow
    match try_quick_pool_creation(tabular, connection_id).await {
        Some(pool) => {
            // Quick success
            tabular.connection_pools.insert(connection_id, pool.clone());
            tabular.pending_connection_pools.remove(&connection_id);
            tabular.pending_pool_log_last.remove(&connection_id);
            debug!(
                "✅ Quickly created connection pool for connection {}",
                connection_id
            );
            Some(pool)
        }
        None => {
            // Start background creation and return None (non-blocking)
            start_background_pool_creation(tabular, connection_id);
            None
        }
    }
}

// Try to create pool quickly (with short timeout)
async fn try_quick_pool_creation(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    let connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    // Quick attempt with short timeout (slightly relaxed to tolerate slower DNS / TLS)
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        create_connection_pool_for_config(&connection),
    )
    .await;

    match result {
        Ok(pool) => pool,
        Err(_) => {
            debug!(
                "⚡ Quick creation timed out for connection {}, will try in background",
                connection_id
            );
            None
        }
    }
}

// Start background pool creation without blocking
fn start_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
    let connection = match tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
    {
        Some(conn) => conn.clone(),
        None => {
            debug!(
                "❌ Connection {} not found for background creation",
                connection_id
            );
            tabular.pending_connection_pools.remove(&connection_id);
            return;
        }
    };

    if let Some(runtime) = &tabular.runtime {
        let rt = runtime.clone();
        let shared_pools = tabular.shared_connection_pools.clone();

        rt.spawn(async move {
            debug!(
                "🔄 Background: Creating pool for connection {}",
                connection_id
            );

            match create_connection_pool_for_config(&connection).await {
                Some(pool) => {
                    debug!(
                        "✅ Background: Successfully created pool for connection {}",
                        connection_id
                    );

                    // Store in shared pools for main thread access
                    if let Ok(mut shared_pools) = shared_pools.lock() {
                        shared_pools.insert(connection_id, pool);
                    }
                }
                None => {
                    debug!(
                        "❌ Background: Failed to create pool for connection {}",
                        connection_id
                    );
                }
            }
        });
    }
}

// Public helper to ensure a background pool creation is in progress without blocking the UI.
// If a pool already exists or a creation is pending, this is a no-op. Otherwise, it marks
// the connection as pending and spawns a background task to create the pool.
pub(crate) fn ensure_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
    // If pool already available in local or shared caches, nothing to do
    let has_pool = tabular.connection_pools.contains_key(&connection_id)
        || tabular
            .shared_connection_pools
            .lock()
            .map(|p| p.contains_key(&connection_id))
            .unwrap_or(false);
    if has_pool {
        return;
    }
    // If pending already, nothing to do
    if tabular.pending_connection_pools.contains(&connection_id) {
        return;
    }
    // Mark pending and spawn background creation (non-blocking)
    tabular.pending_connection_pools.insert(connection_id);
    start_background_pool_creation(tabular, connection_id);
}

// Create connection pool for a specific connection config
async fn create_connection_pool_for_config(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MySQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let _encoded_username = modules::url_encode(&connection.username);
            let _encoded_password = modules::url_encode(&connection.password);
            // Reintroduce connection_string (previously removed) because it's still needed below
            // for establishing the MySQL pool. Use URL-encoded credentials to be safe with special chars.
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                _encoded_username, _encoded_password, target_host, target_port, connection.database
            );

            // Don't block on ICMP ping (often disabled on Windows firewalls). Attempt direct connect.
            // If you still want diagnostics, you can log ping result without failing the flow:
            // let _ = helpers::ping_host(&connection.host);

            // Configure MySQL pool with improved settings for stability
            // We perform up to 2 attempts with progressively relaxed constraints.
            // Common causes of the earlier "pool timed out while waiting for an open connection" error:
            //  - Slow initial handshake (network latency, DNS, TLS) exceeding acquire_timeout
            //  - test_before_acquire adds an extra round-trip under latency
            //  - min_connections > 0 forces eager creation of more than one connection
            //  - after_connect statements (session tweaks) add latency before the pool marks a conn as ready

            let mut last_err: Option<sqlx::Error> = None;

            for attempt in 1..=2u8 {
                let start = std::time::Instant::now();
                let (min_conns, test_before, acquire_secs) = match attempt {
                    1 => (0u32, false, 30u64), // Very permissive first attempt (lazy open)
                    _ => (1u32, true, 45u64),  // Second attempt: enable validation, longer acquire
                };

                let pool_result = MySqlPoolOptions::new()
                    .max_connections(10)
                    .min_connections(min_conns)
                    .acquire_timeout(std::time::Duration::from_secs(acquire_secs))
                    .idle_timeout(std::time::Duration::from_secs(600))
                    .max_lifetime(std::time::Duration::from_secs(1800))
                    .test_before_acquire(test_before)
                    .after_connect(|conn, _| {
                        Box::pin(async move {
                            // Keep after_connect lean; avoid heavyweight or server-specific settings.
                            let _ = sqlx::query("SET SESSION wait_timeout = 600")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION interactive_timeout = 600")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION net_read_timeout = 120")
                                .execute(&mut *conn)
                                .await;
                            let _ = sqlx::query("SET SESSION net_write_timeout = 120")
                                .execute(&mut *conn)
                                .await;
                            // Do NOT force sql_mode if server overrides / permissions limited; ignore errors.
                            let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'")
                                .execute(&mut *conn)
                                .await;
                            Ok(())
                        })
                    })
                    .connect(&connection_string)
                    .await;

                match pool_result {
                    Ok(pool) => {
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "✅ Created MySQL connection pool (attempt {}, {} ms) for connection {:?}",
                            attempt, elapsed, connection.id
                        );
                        return Some(models::enums::DatabasePool::MySQL(Arc::new(pool)));
                    }
                    Err(e) => {
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "❌ MySQL pool attempt {} failed after {} ms for connection {:?}: {:?}",
                            attempt, elapsed, connection.id, e
                        );
                        // If it is a timeout, we retry (if attempt 1). Otherwise break early.
                        let is_timeout = matches!(e, sqlx::Error::PoolTimedOut)
                            || e.to_string().contains("timeout");
                        last_err = Some(e);
                        if !is_timeout || attempt == 2 {
                            break;
                        }
                    }
                }
            }

            if let Some(e) = last_err {
                debug!(
                    "❌ Failed to create MySQL pool for connection {:?} after retries: {:?}",
                    connection.id, e
                );
            }
            None
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for PostgreSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                target_host,
                target_port,
                connection.database
            );

            // Configure PostgreSQL pool with improved settings
            let pool_result = PgPoolOptions::new()
                .max_connections(15) // Increase max connections
                .min_connections(1) // Start with fewer minimum connections
                .acquire_timeout(std::time::Duration::from_secs(10)) // Fail fast
                .idle_timeout(std::time::Duration::from_secs(300)) // 5 minute idle timeout
                .max_lifetime(std::time::Duration::from_secs(1800)) // 30 minute max lifetime
                .test_before_acquire(false) // Disable pre-test for better performance
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => {
                    let database_pool = models::enums::DatabasePool::PostgreSQL(Arc::new(pool));
                    Some(database_pool)
                }
                Err(e) => {
                    debug!("Failed to create PostgreSQL pool: {}", e);
                    None
                }
            }
        }
        models::enums::DatabaseType::SQLite => {
            let connection_string = format!("sqlite:{}", connection.host);

            // Configure SQLite pool with improved settings
            let pool_result = SqlitePoolOptions::new()
                .max_connections(5) // SQLite doesn't need many connections
                .min_connections(1) // Start with one connection
                .acquire_timeout(std::time::Duration::from_secs(10)) // Fail fast
                .idle_timeout(std::time::Duration::from_secs(300)) // 5 minute idle timeout
                .max_lifetime(std::time::Duration::from_secs(1800)) // 30 minute max lifetime
                .test_before_acquire(false) // Disable pre-test for better performance
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => {
                    let database_pool = models::enums::DatabasePool::SQLite(Arc::new(pool));
                    Some(database_pool)
                }
                Err(e) => {
                    debug!("Failed to create SQLite pool: {}", e);
                    None
                }
            }
        }
        models::enums::DatabaseType::Redis => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for Redis connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", target_host, target_port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, target_host, target_port
                )
            };

            debug!(
                "Creating new Redis connection manager for: {}",
                connection.name
            );
            match Client::open(connection_string) {
                Ok(client) => match ConnectionManager::new(client).await {
                    Ok(manager) => {
                        let database_pool = models::enums::DatabasePool::Redis(Arc::new(manager));
                        Some(database_pool)
                    }
                    Err(e) => {
                        debug!("Failed to create Redis connection manager: {}", e);
                        None
                    }
                },
                Err(e) => {
                    debug!("Failed to create Redis client: {}", e);
                    None
                }
            }
        }
        models::enums::DatabaseType::MongoDB => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MongoDB connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            // Build MongoDB connection string
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", target_host, target_port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, target_host, target_port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, target_host, target_port
                )
            };
            debug!("Creating MongoDB client for URI: {}", uri);
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                MongoClient::with_uri_str(uri),
            )
            .await
            {
                Ok(Ok(client)) => {
                    let pool = models::enums::DatabasePool::MongoDB(Arc::new(client));
                    Some(pool)
                }
                _ => {
                    debug!("Failed to create MongoDB client (timeout or error)");
                    None
                }
            }
        }
        models::enums::DatabaseType::MsSQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MsSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let cfg = driver_mssql::MssqlConfigWrapper::new(
                target_host,
                target_port,
                connection.database.clone(),
                connection.username.clone(),
                connection.password.clone(),
            );
            let database_pool = models::enums::DatabasePool::MsSQL(Arc::new(cfg));
            Some(database_pool)
        }
    }
}

// Non-blocking version that tries to get connection pool with retry capability
#[allow(dead_code)]
pub(crate) async fn get_or_create_connection_pool_with_retry(
    tabular: &mut Tabular,
    connection_id: i64,
    max_retries: u32,
) -> Option<models::enums::DatabasePool> {
    for attempt in 0..=max_retries {
        // First check cache
        if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
            debug!(
                "✅ Using cached connection pool for connection {}",
                connection_id
            );
            return Some(cached_pool.clone());
        }

        // Try to create if not being created
        if !tabular.pending_connection_pools.contains(&connection_id) {
            return get_or_create_connection_pool(tabular, connection_id).await;
        }

        // If pool is being created, wait a bit and retry
        if attempt < max_retries {
            debug!(
                "⏳ Waiting for connection pool creation (attempt {}/{})",
                attempt + 1,
                max_retries + 1
            );
            tokio::time::sleep(std::time::Duration::from_millis(500 + attempt as u64 * 200)).await;
        } else {
            debug!(
                "⏰ Max retries reached for connection pool {}",
                connection_id
            );
            break;
        }
    }

    None
}

// Fast non-blocking version that immediately returns None if pool is being created
pub(crate) async fn try_get_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    // Clean up any completed background pools first
    cleanup_completed_background_pools(tabular);

    // Clean up any stuck pending connections (safety net)
    cleanup_stuck_pending_connections(tabular);

    // Check cache first
    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    // If currently being created, return None immediately (non-blocking)
    if tabular.pending_connection_pools.contains(&connection_id) {
        debug!(
            "⏳ Connection pool creation in progress for connection {}, skipping for now",
            connection_id
        );
        return None;
    }

    // Try to create new pool
    get_or_create_connection_pool(tabular, connection_id).await
}

// Example usage function demonstrating the concurrency improvements
// This function shows how to handle multiple connection requests efficiently
#[allow(dead_code)]
pub(crate) async fn execute_multiple_queries_concurrently(
    tabular: &mut Tabular,
    query_requests: Vec<(i64, String)>, // (connection_id, query) pairs
) -> Vec<Option<(Vec<String>, Vec<Vec<String>>)>> {
    let mut results = Vec::new();

    // Process all requests concurrently without blocking on slow connections
    for (connection_id, query) in query_requests {
        // Use the non-blocking version to avoid waiting for slow connections
        match try_get_connection_pool(tabular, connection_id).await {
            Some(_pool) => {
                // Connection pool is ready, execute query
                if let Some(connection) = tabular
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                    .cloned()
                {
                    // Execute query using the existing sync function
                    let result =
                        execute_table_query_sync(tabular, connection_id, &connection, &query);
                    results.push(result);
                } else {
                    results.push(None);
                }
            }
            None => {
                // Connection pool not ready or being created, skip for now
                debug!(
                    "⏳ Skipping query for connection {} as pool is not ready",
                    connection_id
                );
                results.push(None);
            }
        }
    }

    results
}

// Function to cleanup and recreate connection pools
pub(crate) fn cleanup_connection_pool(tabular: &mut Tabular, connection_id: i64) {
    debug!(
        "🧹 Cleaning up connection pool for connection {}",
        connection_id
    );
    tabular.connection_pools.remove(&connection_id);
    tabular.pending_connection_pools.remove(&connection_id); // Also remove from pending

    // Also clean from shared pools
    if let Ok(mut shared_pools) = tabular.shared_connection_pools.lock() {
        shared_pools.remove(&connection_id);
    }

    ssh_tunnel::shutdown_by_id(connection_id);
}

#[allow(dead_code)]
pub(crate) async fn refresh_connection_background_async(
    connection_id: i64,
    db_pool: &Option<Arc<SqlitePool>>,
) -> bool {
    debug!("Refreshing connection with ID: {}", connection_id);

    // Get connection from database
    if let Some(cache_pool_arc) = db_pool {
        let connection_result = sqlx::query(
            "SELECT id, name, host, port, username, password, database_name, connection_type, folder, \
                    COALESCE(ssh_enabled, 0) AS ssh_enabled, \
                    COALESCE(ssh_host, '') AS ssh_host, \
                    COALESCE(ssh_port, '22') AS ssh_port, \
                    COALESCE(ssh_username, '') AS ssh_username, \
                    COALESCE(ssh_auth_method, 'key') AS ssh_auth_method, \
                    COALESCE(ssh_private_key, '') AS ssh_private_key, \
                    COALESCE(ssh_password, '') AS ssh_password, \
                    COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys \
             FROM connections WHERE id = ?"
        )
            .bind(connection_id)
            .fetch_optional(cache_pool_arc.as_ref())
            .await;

        if let Ok(Some(row)) = connection_result {
            let id = row.try_get::<i64, _>("id").unwrap_or(connection_id);
            let name = row.try_get::<String, _>("name").unwrap_or_default();
            let host = row.try_get::<String, _>("host").unwrap_or_default();
            let port = row
                .try_get::<String, _>("port")
                .unwrap_or_else(|_| "3306".to_string());
            let username = row.try_get::<String, _>("username").unwrap_or_default();
            let password = row.try_get::<String, _>("password").unwrap_or_default();
            let database_name = row
                .try_get::<String, _>("database_name")
                .unwrap_or_default();
            let connection_type = row
                .try_get::<String, _>("connection_type")
                .unwrap_or_else(|_| "SQLite".to_string());
            let folder = row.try_get::<Option<String>, _>("folder").unwrap_or(None);
            let ssh_enabled = row.try_get::<i64, _>("ssh_enabled").unwrap_or(0);
            let ssh_host = row.try_get::<String, _>("ssh_host").unwrap_or_default();
            let ssh_port = row
                .try_get::<String, _>("ssh_port")
                .unwrap_or_else(|_| "22".to_string());
            let ssh_username = row.try_get::<String, _>("ssh_username").unwrap_or_default();
            let ssh_auth_method = row
                .try_get::<String, _>("ssh_auth_method")
                .unwrap_or_else(|_| "key".to_string());
            let ssh_private_key = row
                .try_get::<String, _>("ssh_private_key")
                .unwrap_or_default();
            let ssh_password = row.try_get::<String, _>("ssh_password").unwrap_or_default();
            let ssh_accept_unknown_host_keys = row
                .try_get::<i64, _>("ssh_accept_unknown_host_keys")
                .unwrap_or(0);

            let connection = models::structs::ConnectionConfig {
                id: Some(id),
                name,
                host,
                port,
                username,
                password,
                database: database_name,
                connection_type: match connection_type.as_str() {
                    "MySQL" => models::enums::DatabaseType::MySQL,
                    "PostgreSQL" => models::enums::DatabaseType::PostgreSQL,
                    "Redis" => models::enums::DatabaseType::Redis,
                    "MsSQL" => models::enums::DatabaseType::MsSQL,
                    "MongoDB" => models::enums::DatabaseType::MongoDB,
                    _ => models::enums::DatabaseType::SQLite,
                },
                folder,
                ssh_enabled: ssh_enabled != 0,
                ssh_host,
                ssh_port,
                ssh_username,
                ssh_auth_method: models::enums::SshAuthMethod::from_db_value(&ssh_auth_method),
                ssh_private_key,
                ssh_password,
                ssh_accept_unknown_host_keys: ssh_accept_unknown_host_keys != 0,
            };

            // Clear cache
            let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await;

            let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await;

            let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await;

            // Also clear row and index caches to avoid stale data in case UI didn't do it
            let _ = sqlx::query("DELETE FROM row_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await;

            let _ = sqlx::query("DELETE FROM index_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await;

            // Create new connection pool
            match tokio::time::timeout(
                std::time::Duration::from_secs(30), // 30 second timeout
                create_database_pool(&connection),
            )
            .await
            {
                Ok(Some(new_pool)) => {
                    // ✨ OPTIMIZATION: No longer prefetch here to avoid blocking connection opening.
                    // Prefetch will be done on-demand or via optional background task.
                    fetch_and_cache_all_data(
                        connection_id,
                        &connection,
                        &new_pool,
                        cache_pool_arc.as_ref(),
                    )
                    .await
                }
                Ok(None) => false,
                Err(_) => false,
            }
        } else {
            false
        }
    } else {
        false
    }
}

#[allow(dead_code)]
pub(crate) async fn create_database_pool(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let _encoded_username = modules::url_encode(&connection.username);
            let _encoded_password = modules::url_encode(&connection.password);
            // connection_string not needed here; we reuse create_connection_pool_for_config which
            // builds its own connection string. Keep encoded variables in case future logic adds
            // validation or logging.

            // Configure MySQL pool with optimized settings for large queries
            // New: simplified + retried creation mirroring main path (avoid duplication by delegating)
            // Reuse the logic above by constructing a temporary ConnectionConfig clone and calling create_connection_pool_for_config
            return create_connection_pool_for_config(connection).await;
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for PostgreSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                target_host,
                target_port,
                connection.database
            );

            match PgPoolOptions::new()
                .max_connections(3)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .connect(&connection_string)
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::PostgreSQL(Arc::new(pool))),
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::SQLite => {
            let connection_string = format!("sqlite:{}", connection.host);

            match SqlitePoolOptions::new()
                .max_connections(3)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .connect(&connection_string)
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::SQLite(Arc::new(pool))),
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::Redis => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for Redis connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", target_host, target_port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, target_host, target_port
                )
            };

            match Client::open(connection_string) {
                Ok(client) => match ConnectionManager::new(client).await {
                    Ok(manager) => Some(models::enums::DatabasePool::Redis(Arc::new(manager))),
                    Err(_e) => None,
                },
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::MsSQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MsSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let cfg = driver_mssql::MssqlConfigWrapper::new(
                target_host,
                target_port,
                connection.database.clone(),
                connection.username.clone(),
                connection.password.clone(),
            );
            Some(models::enums::DatabasePool::MsSQL(Arc::new(cfg)))
        }
        models::enums::DatabaseType::MongoDB => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MongoDB connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", target_host, target_port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, target_host, target_port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, target_host, target_port
                )
            };
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                MongoClient::with_uri_str(uri),
            )
            .await
            {
                Ok(Ok(client)) => Some(models::enums::DatabasePool::MongoDB(Arc::new(client))),
                _ => None,
            }
        }
    }
}

// Fetch and cache metadata for all databases/tables/columns per connection
#[allow(dead_code)]
async fn fetch_and_cache_all_data(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    pool: &models::enums::DatabasePool,
    cache_pool: &SqlitePool,
) -> bool {
    match &connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            if let models::enums::DatabasePool::MySQL(mysql_pool) = pool {
                driver_mysql::fetch_mysql_data(connection_id, mysql_pool, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::SQLite => {
            if let models::enums::DatabasePool::SQLite(sqlite_pool) = pool {
                driver_sqlite::fetch_data(connection_id, sqlite_pool, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            if let models::enums::DatabasePool::PostgreSQL(postgres_pool) = pool {
                driver_postgres::fetch_postgres_data(connection_id, postgres_pool, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::Redis => {
            if let models::enums::DatabasePool::Redis(redis_manager) = pool {
                driver_redis::fetch_redis_data(connection_id, redis_manager, cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::MsSQL => {
            if let models::enums::DatabasePool::MsSQL(mssql_cfg) = pool {
                driver_mssql::fetch_mssql_data(connection_id, mssql_cfg.clone(), cache_pool).await
            } else {
                false
            }
        }
        models::enums::DatabaseType::MongoDB => {
            if let models::enums::DatabasePool::MongoDB(client) = pool {
                crate::driver_mongodb::fetch_mongodb_data(connection_id, client.clone(), cache_pool)
                    .await
            } else {
                false
            }
        }
    }
}

// Helper: upsert row_cache directly using cache pool
async fn save_row_cache_direct(
    cache_pool: &SqlitePool,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    headers: &[String],
    rows: &Vec<Vec<String>>,
) {
    let headers_json = serde_json::to_string(headers).unwrap_or_else(|_| "[]".to_string());
    let rows_json = serde_json::to_string(rows).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query(
        r#"INSERT INTO row_cache (connection_id, database_name, table_name, headers_json, rows_json, updated_at)
           VALUES (?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
           ON CONFLICT(connection_id, database_name, table_name)
           DO UPDATE SET headers_json=excluded.headers_json, rows_json=excluded.rows_json, updated_at=CURRENT_TIMESTAMP"#,
    )
    .bind(connection_id)
    .bind(database_name)
    .bind(table_name)
    .bind(headers_json)
    .bind(rows_json)
    .execute(cache_pool)
    .await;
}

// After metadata is cached, fetch first 100 rows for all tables and store in row_cache
#[allow(dead_code)]
async fn prefetch_first_rows_for_all_tables(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    pool: &models::enums::DatabasePool,
    cache_pool: &SqlitePool,
) -> bool {
    use sqlx::Row;
    // Load table list from cache
    let tables_res = sqlx::query_as::<_, (String, String)>(
        "SELECT database_name, table_name FROM table_cache WHERE connection_id = ? AND table_type = 'table' ORDER BY database_name, table_name",
    )
    .bind(connection_id)
    .fetch_all(cache_pool)
    .await;

    let rows = match tables_res {
        Ok(v) => v,
        Err(_) => return false,
    };

    match pool {
        models::enums::DatabasePool::MySQL(_mysql_pool) => {
            let enc_user = crate::modules::url_encode(&connection.username);
            let enc_pass = crate::modules::url_encode(&connection.password);
            futures_util::stream::iter(rows)
                .map(|(dbn, tbn)| {
                    let host = connection.host.clone();
                    let port = connection.port.clone();
                    let enc_user = enc_user.clone();
                    let enc_pass = enc_pass.clone();
                    async move {
                        let dsn = format!(
                            "mysql://{}:{}@{}:{}/{}",
                            enc_user, enc_pass, host, port, dbn
                        );
                        if let Ok(mut conn) = sqlx::mysql::MySqlConnection::connect(&dsn).await {
                            let q = format!("SELECT * FROM `{}` LIMIT 100", tbn.replace('`', "``"));
                            if let Ok(mysql_rows) = sqlx::query(&q).fetch_all(&mut conn).await {
                                let headers: Vec<String> = if let Some(r0) = mysql_rows.first() {
                                    r0.columns().iter().map(|c| c.name().to_string()).collect()
                                } else {
                                    let dq = format!("DESCRIBE `{}`", tbn.replace('`', "``"));
                                    match sqlx::query(&dq).fetch_all(&mut conn).await {
                                        Ok(desc_rows) => desc_rows
                                            .iter()
                                            .filter_map(|r| r.try_get::<String, _>(0).ok())
                                            .collect(),
                                        Err(_) => Vec::new(),
                                    }
                                };
                                let data = crate::driver_mysql::convert_mysql_rows_to_table_data(
                                    mysql_rows,
                                );
                                save_row_cache_direct(
                                    cache_pool,
                                    connection_id,
                                    &dbn,
                                    &tbn,
                                    &headers,
                                    &data,
                                )
                                .await;
                            }
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        models::enums::DatabasePool::PostgreSQL(pg_pool) => {
            futures_util::stream::iter(rows)
                .map(|(dbn, tbn)| {
                    let pool = pg_pool.clone();
                    async move {
                        let q = format!("SELECT * FROM \"public\".\"{}\" LIMIT 100", tbn.replace('"', "\\\""));
                        if let Ok(pg_rows) = sqlx::query(&q).fetch_all(pool.as_ref()).await {
                            let headers: Vec<String> = if let Some(r0) = pg_rows.first() {
                                r0.columns().iter().map(|c| c.name().to_string()).collect()
                            } else {
                                let iq = format!(
                                    "SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name='{}' ORDER BY ordinal_position",
                                    tbn.replace("'", "''")
                                );
                                match sqlx::query(&iq).fetch_all(pool.as_ref()).await {
                                    Ok(infos) => infos
                                        .iter()
                                        .filter_map(|r| r.try_get::<String, _>(0).ok())
                                        .collect(),
                                    Err(_) => Vec::new(),
                                }
                            };
                            let data: Vec<Vec<String>> = pg_rows
                                .iter()
                                .map(|row| {
                                    (0..row.len())
                                        .map(|j| match row.try_get::<Option<String>, _>(j) {
                                            Ok(Some(v)) => v,
                                            Ok(None) => "NULL".to_string(),
                                            Err(_) => {
                                                if let Ok(Some(bytes)) = row.try_get::<Option<Vec<u8>>, _>(j) {
                                                    String::from_utf8_lossy(&bytes).to_string()
                                                } else { "".to_string() }
                                            }
                                        })
                                        .collect()
                                })
                                .collect();
                            save_row_cache_direct(cache_pool, connection_id, &dbn, &tbn, &headers, &data).await;
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        models::enums::DatabasePool::SQLite(sqlite_pool) => {
            futures_util::stream::iter(rows)
                .map(|(_dbn, tbn)| {
                    let pool = sqlite_pool.clone();
                    async move {
                        let q = format!("SELECT * FROM `{}` LIMIT 100", tbn.replace('`', "``"));
                        if let Ok(sqlite_rows) = sqlx::query(&q).fetch_all(pool.as_ref()).await {
                            let headers: Vec<String> = if let Some(r0) = sqlite_rows.first() {
                                r0.columns().iter().map(|c| c.name().to_string()).collect()
                            } else {
                                let iq =
                                    format!("PRAGMA table_info(\"{}\")", tbn.replace('"', "\\\""));
                                match sqlx::query(&iq).fetch_all(pool.as_ref()).await {
                                    Ok(infos) => infos
                                        .iter()
                                        .filter_map(|r| r.try_get::<String, _>(1).ok())
                                        .collect(),
                                    Err(_) => Vec::new(),
                                }
                            };
                            let data = crate::driver_sqlite::convert_sqlite_rows_to_table_data(
                                sqlite_rows,
                            );
                            save_row_cache_direct(
                                cache_pool,
                                connection_id,
                                "main",
                                &tbn,
                                &headers,
                                &data,
                            )
                            .await;
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        _ => false,
    }
}

pub(crate) fn fetch_databases_from_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
) -> Option<Vec<String>> {
    // Find the connection configuration
    let _connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    // Create a new runtime for the database query
    let rt = tokio::runtime::Runtime::new().ok()?;

    rt.block_on(async {
        // Get or create connection pool
        let pool = connection::get_or_create_connection_pool(tabular, connection_id).await?;

        match pool {
            models::enums::DatabasePool::MySQL(mysql_pool) => {
                // Use INFORMATION_SCHEMA to avoid VARBINARY decode issues from SHOW DATABASES on some setups
                let result = sqlx::query_as::<_, (String,)>(
                    "SELECT CONVERT(SCHEMA_NAME USING utf8mb4) AS schema_name FROM INFORMATION_SCHEMA.SCHEMATA"
                )
                    .fetch_all(mysql_pool.as_ref())
                    .await;

                match result {
                    Ok(rows) => {
                        let databases: Vec<String> = rows
                            .into_iter()
                            .map(|(db_name, )| db_name)
                            .filter(|db| !["information_schema", "performance_schema", "mysql", "sys"].contains(&db.as_str()))
                            .collect();
                        Some(databases)
                    }
                    Err(e) => {
                        debug!("Error querying MySQL databases via INFORMATION_SCHEMA: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabasePool::PostgreSQL(pg_pool) => {
                let result = sqlx::query_as::<_, (String,)>(
                    "SELECT datname FROM pg_database WHERE datistemplate = false AND datname NOT IN ('postgres', 'template0', 'template1')"
                )
                    .fetch_all(pg_pool.as_ref())
                    .await;

                match result {
                    Ok(rows) => {
                        let databases: Vec<String> = rows.into_iter().map(|(db_name, )| db_name).collect();
                        Some(databases)
                    }
                    Err(e) => {
                        debug!("Error querying PostgreSQL databases: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabasePool::SQLite(sqlite_pool) => {
                // For SQLite, we'll query the actual database for table information
                let result = sqlx::query_as::<_, (String,)>("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                    .fetch_all(sqlite_pool.as_ref())
                    .await;

                match result {
                    Ok(rows) => {
                        let table_count = rows.len();
                        if table_count > 0 {
                            // Since SQLite has tables, return main database
                            Some(vec!["main".to_string()])
                        } else {
                            debug!("No tables found in SQLite database, returning 'main' database anyway");
                            Some(vec!["main".to_string()])
                        }
                    }
                    Err(e) => {
                        debug!("Error querying SQLite tables: {}", e);
                        Some(vec!["main".to_string()]) // Fallback to main
                    }
                }
            }
            models::enums::DatabasePool::Redis(redis_manager) => {
                // For Redis, get actual databases (db0, db1, etc.)
                let mut conn = redis_manager.as_ref().clone();

                // Get CONFIG GET databases to determine max database count
                let max_databases = match redis::cmd("CONFIG").arg("GET").arg("databases").query_async::<Vec<String>>(&mut conn).await {
                    Ok(config_result) if config_result.len() >= 2 => {
                        config_result[1].parse::<i32>().unwrap_or(16)
                    }
                    _ => 16 // Default fallback
                };

                debug!("Redis max databases: {}", max_databases);

                // Create list of all Redis databases (db0 to db15 by default)
                let mut databases = Vec::new();
                for db_num in 0..max_databases {
                    let db_name = format!("db{}", db_num);
                    databases.push(db_name);
                }

                debug!("Generated Redis databases: {:?}", databases);
                Some(databases)
            }
            models::enums::DatabasePool::MsSQL(ref mssql_cfg) => {
                // Fetch list of databases from MsSQL server
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                use tiberius::{AuthMethod, Config};
                {
                    let mssql_cfg = mssql_cfg.clone();
                    // Attempt connection with master database to enumerate all
                    let host = mssql_cfg.host.clone();
                    let port = mssql_cfg.port;
                    let user = mssql_cfg.username.clone();
                    let pass = mssql_cfg.password.clone();
                    let rt_res = async move {
                        let mut config = Config::new();
                        config.host(host.clone());
                        config.port(port);
                        config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                        config.trust_cert();
                        // Always use master for listing
                        config.database("master");
                        let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                        tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                        let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                        let mut dbs = Vec::new();
                        let mut stream = client.simple_query("SELECT name FROM sys.databases ORDER BY name").await.map_err(|e| e.to_string())?;
                        use futures_util::TryStreamExt;
                        while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                            if let tiberius::QueryItem::Row(r) = item {
                                let name: Option<&str> = r.get(0);
                                if let Some(n) = name {
                                    // Optionally skip system DBs? Keep them for completeness; can filter later.
                                    dbs.push(n.to_string());
                                }
                            }
                        }
                        Ok::<_, String>(dbs)
                    }.await;
                    match rt_res {
                        Ok(mut list) => {
                            if list.is_empty() {
                                debug!("MsSQL database list is empty; returning current database only");
                                Some(vec![mssql_cfg.database.clone()])
                            } else {
                                // Move system DBs (master, model, msdb, tempdb) to end for nicer UX
                                let system = ["master", "model", "msdb", "tempdb"];
                                list.sort();
                                let mut user_dbs: Vec<String> = list.iter().filter(|d| !system.contains(&d.as_str())).cloned().collect();
                                let mut sys_dbs: Vec<String> = list.into_iter().filter(|d| system.contains(&d.as_str())).collect();
                                user_dbs.append(&mut sys_dbs);
                                Some(user_dbs)
                            }
                        }
                        Err(e) => {
                            debug!("Failed to fetch MsSQL databases: {}", e);
                            // Fallback to default known system DBs so UI still shows something
                            Some(vec!["master".to_string(), "tempdb".to_string(), "model".to_string(), "msdb".to_string()])
                        }
                    }
                }
            }
            models::enums::DatabasePool::MongoDB(client) => {
                match client.list_database_names().await {
                    Ok(dbs) => Some(dbs),
                    Err(e) => {
                        debug!("MongoDB list databases error: {}", e);
                        None
                    }
                }
            }
        }
    })
}

// Async version to avoid creating a new runtime each call; preferred for internal use
pub(crate) async fn fetch_databases_from_connection_async(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
) -> Option<Vec<String>> {
    // Find the connection configuration
    let _connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    // Get or create connection pool
    let pool = connection::get_or_create_connection_pool(tabular, connection_id).await?;
    match pool {
        models::enums::DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query_as::<_, (String,)>(
                "SELECT CONVERT(SCHEMA_NAME USING utf8mb4) AS schema_name FROM INFORMATION_SCHEMA.SCHEMATA"
            )
                .fetch_all(mysql_pool.as_ref())
                .await;
            match result {
                Ok(rows) => Some(
                    rows.into_iter()
                        .map(|(db_name,)| db_name)
                        .filter(|db| {
                            !["information_schema", "performance_schema", "mysql", "sys"]
                                .contains(&db.as_str())
                        })
                        .collect(),
                ),
                Err(e) => {
                    debug!(
                        "Error querying MySQL databases via INFORMATION_SCHEMA: {}",
                        e
                    );
                    None
                }
            }
        }
        models::enums::DatabasePool::PostgreSQL(pg_pool) => {
            let result = sqlx::query_as::<_, (String,)>(
                "SELECT datname FROM pg_database WHERE datistemplate = false AND datname NOT IN ('postgres', 'template0', 'template1')"
            )
                .fetch_all(pg_pool.as_ref()).await;
            match result {
                Ok(rows) => Some(rows.into_iter().map(|(db_name,)| db_name).collect()),
                Err(e) => {
                    debug!("Error querying PostgreSQL databases: {}", e);
                    None
                }
            }
        }
        models::enums::DatabasePool::SQLite(sqlite_pool) => {
            let result = sqlx::query_as::<_, (String,)>(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .fetch_all(sqlite_pool.as_ref())
            .await;
            match result {
                Ok(_rows) => Some(vec!["main".to_string()]),
                Err(e) => {
                    debug!("Error querying SQLite tables: {}", e);
                    Some(vec!["main".to_string()])
                }
            }
        }
        models::enums::DatabasePool::Redis(redis_manager) => {
            let mut conn = redis_manager.as_ref().clone();
            let max_databases = match redis::cmd("CONFIG")
                .arg("GET")
                .arg("databases")
                .query_async::<Vec<String>>(&mut conn)
                .await
            {
                Ok(config_result) if config_result.len() >= 2 => {
                    config_result[1].parse::<i32>().unwrap_or(16)
                }
                _ => 16,
            };
            let mut databases = Vec::with_capacity(max_databases as usize);
            for db_num in 0..max_databases {
                databases.push(format!("db{}", db_num));
            }
            Some(databases)
        }
        models::enums::DatabasePool::MsSQL(ref mssql_cfg) => {
            use tiberius::{AuthMethod, Config};
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let mssql_cfg = mssql_cfg.clone();
            let host = mssql_cfg.host.clone();
            let port = mssql_cfg.port;
            let user = mssql_cfg.username.clone();
            let pass = mssql_cfg.password.clone();
            let rt_res = async move {
                let mut config = Config::new();
                config.host(host.clone());
                config.port(port);
                config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                config.trust_cert();
                config.database("master");
                let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                    .await
                    .map_err(|e| e.to_string())?;
                tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                let mut client = tiberius::Client::connect(config, tcp.compat_write())
                    .await
                    .map_err(|e| e.to_string())?;
                let mut dbs = Vec::new();
                let mut stream = client
                    .simple_query("SELECT name FROM sys.databases ORDER BY name")
                    .await
                    .map_err(|e| e.to_string())?;
                use futures_util::TryStreamExt;
                while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                    if let tiberius::QueryItem::Row(r) = item {
                        let name: Option<&str> = r.get(0);
                        if let Some(n) = name {
                            dbs.push(n.to_string());
                        }
                    }
                }
                Ok::<_, String>(dbs)
            }
            .await;
            match rt_res {
                Ok(mut list) => {
                    if list.is_empty() {
                        Some(vec![mssql_cfg.database.clone()])
                    } else {
                        let system = ["master", "model", "msdb", "tempdb"];
                        list.sort();
                        let mut user_dbs: Vec<String> = list
                            .iter()
                            .filter(|d| !system.contains(&d.as_str()))
                            .cloned()
                            .collect();
                        let mut sys_dbs: Vec<String> = list
                            .into_iter()
                            .filter(|d| system.contains(&d.as_str()))
                            .collect();
                        user_dbs.append(&mut sys_dbs);
                        Some(user_dbs)
                    }
                }
                Err(e) => {
                    debug!("Failed to fetch MsSQL databases: {}", e);
                    Some(vec![
                        "master".to_string(),
                        "tempdb".to_string(),
                        "model".to_string(),
                        "msdb".to_string(),
                    ])
                }
            }
        }
        models::enums::DatabasePool::MongoDB(client) => match client.list_database_names().await {
            Ok(dbs) => Some(dbs),
            Err(e) => {
                debug!("MongoDB list databases error: {}", e);
                None
            }
        },
    }
}

pub(crate) fn fetch_columns_from_database(
    _connection_id: i64,
    database_name: &str,
    table_name: &str,
    connection: &models::structs::ConnectionConfig,
) -> Option<Vec<(String, String)>> {
    // Create a new runtime for the database query
    let rt = tokio::runtime::Runtime::new().ok()?;

    // Clone data to move into async block
    let connection_clone = connection.clone();
    let database_name = database_name.to_string();
    let table_name = table_name.to_string();

    rt.block_on(async {
        match connection_clone.connection_type {
            models::enums::DatabaseType::MySQL => {
                // Create MySQL connection
                let encoded_username = modules::url_encode(&connection_clone.username);
                let encoded_password = modules::url_encode(&connection_clone.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username, encoded_password, connection_clone.host, connection_clone.port, database_name
                );

                match MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // Query information_schema for complete column type (COLUMN_TYPE includes length/precision)
                        let query = "SELECT COLUMN_NAME, COLUMN_TYPE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION";
                        let result = sqlx::query(query)
                            .bind(&database_name)
                            .bind(&table_name)
                            .fetch_all(&pool)
                            .await;
                        match result {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut columns: Vec<(String, String)> = Vec::with_capacity(rows.len());
                                for row in rows {
                                    // Robust extraction: try String, then bytes -> utf8_lossy
                                    let col_name: Option<String> = match row.try_get::<String, _>("COLUMN_NAME") {
                                        Ok(v) => Some(v),
                                        Err(_) => row.try_get::<Vec<u8>, _>("COLUMN_NAME").ok().map(|b| String::from_utf8_lossy(&b).to_string())
                                    };
                                    let data_type: Option<String> = match row.try_get::<String, _>("COLUMN_TYPE") {
                                        Ok(v) => Some(v),
                                        Err(_) => row.try_get::<Vec<u8>, _>("COLUMN_TYPE").ok().map(|b| String::from_utf8_lossy(&b).to_string())
                                    };
                                    if let (Some(n), Some(t)) = (col_name, data_type) { columns.push((n, t)); }
                                }
                                if columns.is_empty() {
                                    // Fallback to SHOW COLUMNS if nothing parsed (unexpected)
                                    let show_q = format!("SHOW COLUMNS FROM `{}`.`{}`", database_name.replace('`', ""), table_name.replace('`', ""));
                                    match sqlx::query(&show_q).fetch_all(&pool).await {
                                        Ok(srows) => {
                                            for r in srows {
                                                let name: Option<String> = r.try_get("Field").ok();
                                                let dtype: Option<String> = r.try_get("Type").ok();
                                                if let (Some(n), Some(t)) = (name, dtype) { columns.push((n, t)); }
                                            }
                                        }
                                        Err(e) => { debug!("MySQL fallback SHOW COLUMNS failed for {}: {}", table_name, e); }
                                    }
                                }
                                Some(columns)
                            }
                            Err(e) => {
                                debug!("Error querying MySQL columns for table {}: {}", table_name, e);
                                // Fallback directly to SHOW COLUMNS
                                let mut columns: Vec<(String, String)> = Vec::new();
                                let show_q = format!("SHOW COLUMNS FROM `{}`.`{}`", database_name.replace('`', ""), table_name.replace('`', ""));
                                if let Ok(srows) = sqlx::query(&show_q).fetch_all(&pool).await {
                                    use sqlx::Row;
                                    for r in srows {
                                        let name: Option<String> = r.try_get("Field").ok();
                                        let dtype: Option<String> = r.try_get("Type").ok();
                                        if let (Some(n), Some(t)) = (name, dtype) { columns.push((n, t)); }
                                    }
                                    if !columns.is_empty() { return Some(columns); }
                                }
                                None
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Error connecting to MySQL database: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::SQLite => {
                // Create SQLite connection
                let connection_string = format!("sqlite:{}", connection_clone.host);

                match SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // Use dynamic row extraction to avoid issues with NULL dflt_value, and quote table name safely
                        let escaped = table_name.replace("'", "''");
                        let query = format!("PRAGMA table_info('{}')", escaped);
                        match sqlx::query(&query).fetch_all(&pool).await {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut columns: Vec<(String, String)> = Vec::new();
                                for row in rows {
                                    // Columns in pragma: cid, name, type, notnull, dflt_value, pk
                                    let name: Option<String> = row.try_get("name").ok();
                                    let data_type: Option<String> = row.try_get("type").ok();
                                    if let (Some(n), Some(t)) = (name, data_type) {
                                        columns.push((n, t));
                                    }
                                }
                                Some(columns)
                            }
                            Err(e) => {
                                debug!("Error querying SQLite columns for table {}: {}", table_name, e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Error connecting to SQLite database: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::PostgreSQL => {
                // Create PostgreSQL connection
                let connection_string = format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    connection_clone.username, connection_clone.password, connection_clone.host, connection_clone.port, database_name
                );

                match PgPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // Use PostgreSQL-style positional parameters ($1, $2, ...)
                        let query = "SELECT column_name, data_type FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position";
                        match sqlx::query_as::<_, (String, String)>(query)
                            .bind(&table_name)
                            .fetch_all(&pool)
                            .await
                        {
                            Ok(rows) => {
                                let columns: Vec<(String, String)> = rows.into_iter().collect();
                                Some(columns)
                            }
                            Err(e) => {
                                debug!("Error querying PostgreSQL columns for table {}: {}", table_name, e);
                                None
                            }
                        }
                    }
                    Err(e) => {
                        debug!("Error connecting to PostgreSQL database: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::Redis => {
                // Redis doesn't have traditional tables/columns
                // Return some generic "columns" for Redis key-value structure
                Some(vec![
                    ("key".to_string(), "String".to_string()),
                    ("value".to_string(), "Any".to_string()),
                    ("type".to_string(), "String".to_string()),
                    ("ttl".to_string(), "Integer".to_string()),
                ])
            }
            models::enums::DatabaseType::MongoDB => {
                // Connect directly and sample one document to infer top-level fields
                let uri = if connection_clone.username.is_empty() {
                    format!("mongodb://{}:{}", connection_clone.host, connection_clone.port)
                } else if connection_clone.password.is_empty() {
                    format!("mongodb://{}@{}:{}", connection_clone.username, connection_clone.host, connection_clone.port)
                } else {
                    let enc_user = modules::url_encode(&connection_clone.username);
                    let enc_pass = modules::url_encode(&connection_clone.password);
                    format!("mongodb://{}:{}@{}:{}", enc_user, enc_pass, connection_clone.host, connection_clone.port)
                };
                match MongoClient::with_uri_str(uri).await {
                    Ok(client) => {
                        let coll = client.database(&database_name).collection::<mongodb::bson::Document>(&table_name);
                        match coll.find(doc! {}).limit(1).await {
                            Ok(mut cursor) => {
                                if let Some(doc) = cursor.try_next().await.unwrap_or(None) {
                                    use mongodb::bson::Bson;
                                    let cols: Vec<(String, String)> = doc.into_iter().map(|(k, v)| {
                                        let t = match v {
                                            Bson::Double(_) => "double",
                                            Bson::String(_) => "string",
                                            Bson::Array(_) => "array",
                                            Bson::Document(_) => "document",
                                            Bson::Boolean(_) => "bool",
                                            Bson::Int32(_) => "int32",
                                            Bson::Int64(_) => "int64",
                                            Bson::Decimal128(_) => "decimal128",
                                            Bson::ObjectId(_) => "objectId",
                                            Bson::DateTime(_) => "date",
                                            Bson::Null => "null",
                                            _ => "any",
                                        };
                                        (k, t.to_string())
                                    }).collect();
                                    Some(cols)
                                } else { None }
                            }
                            Err(_) => None,
                        }
                    }
                    Err(_) => None,
                }
            }
            models::enums::DatabaseType::MsSQL => {
                // Basic column metadata using INFORMATION_SCHEMA
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                use tiberius::{Config, AuthMethod};
                let host = connection_clone.host.clone();
                let port: u16 = connection_clone.port.parse().unwrap_or(1433);
                let user = connection_clone.username.clone();
                let pass = connection_clone.password.clone();
                let db = database_name.clone();
                let table = table_name.clone();
                let rt_res = async move {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    // Parse possible qualified MsSQL names like [schema].[table] or schema.table
                    let parse_qualified = |name: &str| -> (Option<String>, String) {
                        // Handle [schema].[table] or [schema].[table].[extra]
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 {
                                return (Some(parts[0].to_string()), parts[1].to_string());
                            }
                        }
                        // Handle schema.table
                        if let Some((schema, tbl)) = name.split_once('.') {
                            return (
                                Some(schema.trim_matches(|c| c == '[' || c == ']').to_string()),
                                tbl.trim_matches(|c| c == '[' || c == ']').to_string()
                            );
                        }
                        // Only table
                        (None, name.trim_matches(|c| c == '[' || c == ']').to_string())
                    };

                    let (schema_opt, table_only) = parse_qualified(&table);

                    // Build INFORMATION_SCHEMA query with optional schema filter
                    let table_escaped = table_only.replace("'", "''");
                    let mut query = format!(
                        "SELECT COLUMN_NAME, DATA_TYPE FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = '{}'",
                        table_escaped
                    );
                    if let Some(schema) = schema_opt {
                        let schema_escaped = schema.replace("'", "''");
                        query.push_str(&format!(" AND TABLE_SCHEMA = '{}'", schema_escaped));
                    }
                    query.push_str(" ORDER BY ORDINAL_POSITION");
                    let mut stream = client.simple_query(query).await.map_err(|e| e.to_string())?;
                    let mut cols = Vec::new();
                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                        if let tiberius::QueryItem::Row(r) = item {
                            let name: Option<&str> = r.get(0);
                            let dt: Option<&str> = r.get(1);
                            if let (Some(n), Some(d)) = (name, dt) { cols.push((n.to_string(), d.to_string())); }
                        }
                    }
                    Ok::<_, String>(cols)
                }.await;
                match rt_res {
                    Ok(v) => Some(v),
                    Err(e) => {
                        debug!("MsSQL column fetch error: {}", e);
                        None
                    }
                }
            }
            // MongoDB has been handled above; no additional branch here.
        }
    })
}

pub(crate) fn fetch_view_definition(
    connection: &models::structs::ConnectionConfig,
    database_name: Option<&str>,
    view_name: &str,
) -> Option<String> {
    let rt = tokio::runtime::Runtime::new().ok()?;

    let connection_clone = connection.clone();
    let db_name = database_name
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| connection_clone.database.clone());
    let view_name = view_name.to_string();

    rt.block_on(async {
        match connection_clone.connection_type {
            models::enums::DatabaseType::MySQL => {
                if db_name.is_empty() {
                    return None;
                }

                let encoded_username = modules::url_encode(&connection_clone.username);
                let encoded_password = modules::url_encode(&connection_clone.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username,
                    encoded_password,
                    connection_clone.host,
                    connection_clone.port,
                    db_name
                );

                match MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        let query = "SELECT VIEW_DEFINITION FROM information_schema.VIEWS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?";
                        match sqlx::query(query)
                            .bind(&db_name)
                            .bind(&view_name)
                            .fetch_optional(&pool)
                            .await
                        {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                let definition: Option<String> = row
                                    .try_get::<String, _>("VIEW_DEFINITION")
                                    .ok()
                                    .or_else(|| {
                                        row.try_get::<Vec<u8>, _>("VIEW_DEFINITION")
                                            .ok()
                                            .map(|b| String::from_utf8_lossy(&b).to_string())
                                    });

                                if let Some(def) = definition {
                                    let escape = |name: &str| name.replace('`', "``");
                                    let qualified = format!(
                                        "`{}`.`{}`",
                                        escape(&db_name),
                                        escape(&view_name)
                                    );
                                    let mut body = def.trim().trim_end_matches(';').to_string();
                                    if body.is_empty() {
                                        body = format!("SELECT * FROM `{}`.`{}`", db_name, view_name);
                                    }
                                    let script = format!("ALTER VIEW {} AS\n{};", qualified, body);
                                    Some(script)
                                } else {
                                    None
                                }
                            }
                            Ok(None) => None,
                            Err(e) => {
                                debug!(
                                    "Failed to fetch MySQL view definition for {}: {}",
                                    view_name, e
                                );
                                None
                            }
                        }
                    }
                    Err(e) => {
                        debug!("MySQL connection error fetching view definition: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::PostgreSQL => {
                if db_name.is_empty() {
                    return None;
                }

                let connection_string = format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    connection_clone.username,
                    connection_clone.password,
                    connection_clone.host,
                    connection_clone.port,
                    db_name
                );

                match PgPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        let query = "SELECT table_schema, pg_get_viewdef(format('%I.%I', table_schema, table_name)::regclass, true) AS definition FROM information_schema.views WHERE table_name = $1 ORDER BY CASE WHEN table_schema = 'public' THEN 0 ELSE 1 END LIMIT 1";
                        match sqlx::query(query)
                            .bind(&view_name)
                            .fetch_optional(&pool)
                            .await
                        {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                let schema: Option<String> = row
                                    .try_get::<String, _>("table_schema")
                                    .ok();
                                let definition: Option<String> = row
                                    .try_get::<String, _>("definition")
                                    .ok()
                                    .or_else(|| {
                                        row.try_get::<Vec<u8>, _>("definition")
                                            .ok()
                                            .map(|b| String::from_utf8_lossy(&b).to_string())
                                    });

                                if let Some(def) = definition {
                                    let schema = schema.unwrap_or_else(|| "public".to_string());
                                    let escape = |name: &str| name.replace('"', "\"\"");
                                    let qualified = format!(
                                        "\"{}\".\"{}\"",
                                        escape(&schema),
                                        escape(&view_name)
                                    );
                                    let mut body = def.trim().trim_end_matches(';').to_string();
                                    if body.is_empty() {
                                        body = format!("SELECT * FROM \"{}\".\"{}\"", schema, view_name);
                                    }
                                    let script = format!("ALTER VIEW {} AS\n{};", qualified, body);
                                    Some(script)
                                } else {
                                    None
                                }
                            }
                            Ok(None) => None,
                            Err(e) => {
                                debug!(
                                    "Failed to fetch PostgreSQL view definition for {}: {}",
                                    view_name, e
                                );
                                None
                            }
                        }
                    }
                    Err(e) => {
                        debug!(
                            "PostgreSQL connection error fetching view definition: {}",
                            e
                        );
                        None
                    }
                }
            }
            models::enums::DatabaseType::SQLite => {
                let connection_string = format!("sqlite:{}", connection_clone.host);

                match SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        let query = "SELECT sql FROM sqlite_master WHERE type = 'view' AND name = ?";
                        match sqlx::query(query)
                            .bind(&view_name)
                            .fetch_optional(&pool)
                            .await
                        {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                let raw_sql: Option<String> = row
                                    .try_get::<String, _>("sql")
                                    .ok();
                                if let Some(raw) = raw_sql {
                                    let upper = raw.to_uppercase();
                                    if let Some(idx) = upper.find(" AS ") {
                                        let body = raw[idx + 4..].trim().trim_end_matches(';');
                                        let escape = |name: &str| name.replace('"', "\"\"");
                                        let script = format!(
                                            "ALTER VIEW \"{}\" AS\n{};",
                                            escape(&view_name),
                                            body
                                        );
                                        Some(script)
                                    } else if let Some(idx) = upper.find("CREATE") {
                                        let mut script = raw.clone();
                                        script.replace_range(idx..idx + "CREATE".len(), "ALTER");
                                        Some(script)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            }
                            Ok(None) => None,
                            Err(e) => {
                                debug!(
                                    "Failed to fetch SQLite view definition for {}: {}",
                                    view_name, e
                                );
                                None
                            }
                        }
                    }
                    Err(e) => {
                        debug!("SQLite connection error fetching view definition: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::MsSQL => {
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                use tiberius::{AuthMethod, Config};
                let host = connection_clone.host.clone();
                let port: u16 = connection_clone.port.parse().unwrap_or(1433);
                let user = connection_clone.username.clone();
                let pass = connection_clone.password.clone();
                let db = if db_name.is_empty() {
                    connection_clone.database.clone()
                } else {
                    db_name.clone()
                };

                let rt_res: Result<Option<String>, String> = async {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() {
                        config.database(db.clone());
                    }

                    let tcp = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        tokio::net::TcpStream::connect((host.as_str(), port)),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true)
                        .map_err(|e| e.to_string())?;

                    let mut client = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        tiberius::Client::connect(config, tcp.compat_write()),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;

                    let parse_qualified = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(&['[', ']'][..]);
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 {
                                return (Some(parts[0].to_string()), parts[1].to_string());
                            }
                        }
                        if let Some((schema, tbl)) = name.split_once('.') {
                            return (
                                Some(schema.trim_matches(&['[', ']'][..]).to_string()),
                                tbl.trim_matches(&['[', ']'][..]).to_string(),
                            );
                        }
                        (None, name.trim_matches(&['[', ']'][..]).to_string())
                    };

                    let (schema_opt, view_only) = parse_qualified(&view_name);
                    let view_escaped = view_only.replace("'", "''");
                    let mut query = format!(
                        "SELECT TOP 1 TABLE_SCHEMA, VIEW_DEFINITION FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_NAME = '{}'",
                        view_escaped
                    );
                    if let Some(schema) = &schema_opt {
                        query.push_str(&format!(" AND TABLE_SCHEMA = '{}'", schema.replace("'", "''")));
                    }

                    let mut stream = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        client.simple_query(query),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;

                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                        if let tiberius::QueryItem::Row(row) = item {
                            let schema: Option<&str> = row.get(0);
                            let definition: Option<&str> = row.get(1);
                            if let Some(def) = definition {
                                let schema_name = schema
                                    .map(|s| s.to_string())
                                    .or(schema_opt.clone())
                                    .unwrap_or_else(|| "dbo".to_string());
                                let mut body = def.trim().trim_end_matches(';').to_string();
                                if body.is_empty() {
                                    body = format!(
                                        "SELECT * FROM [{}].[{}]",
                                        schema_name,
                                        view_only
                                    );
                                }
                                let qualified = format!("[{}].[{}]", schema_name, view_only);
                                let script = format!("ALTER VIEW {} AS\n{};", qualified, body);
                                return Ok(Some(script));
                            }
                        }
                    }
                    Ok::<Option<String>, String>(None)
                }
                .await;

                match rt_res {
                    Ok(result) => result,
                    Err(e) => {
                        debug!("MsSQL error fetching view definition: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::Redis | models::enums::DatabaseType::MongoDB => None,
        }
    })
}

/// Fetch stored procedure definition (raw) and return it unchanged.
/// - For MsSQL: returns the CREATE PROCEDURE text from OBJECT_DEFINITION
/// - For MySQL: returns the CREATE PROCEDURE statement from SHOW CREATE PROCEDURE
/// - Others: None
pub(crate) fn fetch_procedure_definition(
    connection: &models::structs::ConnectionConfig,
    database_name: Option<&str>,
    procedure_name: &str,
) -> Option<String> {
    let rt = tokio::runtime::Runtime::new().ok()?;

    let connection_clone = connection.clone();
    let db_name = database_name
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| connection_clone.database.clone());
    let proc_name = procedure_name.to_string();

    rt.block_on(async {
        match connection_clone.connection_type {
            models::enums::DatabaseType::MySQL => {
                if db_name.is_empty() {
                    return None;
                }

                let encoded_username = modules::url_encode(&connection_clone.username);
                let encoded_password = modules::url_encode(&connection_clone.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username,
                    encoded_password,
                    connection_clone.host,
                    connection_clone.port,
                    db_name
                );

                match MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // SHOW CREATE PROCEDURE returns `Procedure` and `Create Procedure`
                        let qualified = format!(
                            "`{}`.`{}`",
                            db_name.replace('`', "``"),
                            proc_name.replace('`', "``")
                        );
                        let query = format!("SHOW CREATE PROCEDURE {}", qualified);
                        match sqlx::query(&query).fetch_optional(&pool).await {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                // Column order (MySQL 8+):
                                // 0 = Procedure, 1 = sql_mode, 2 = Create Procedure, 3 = character_set_client, ...
                                // Prefer ordinal index 2; fall back to named column for safety
                                let def = row
                                    .try_get::<String, _>(2)
                                    .ok()
                                    .or_else(|| row.try_get::<String, _>("Create Procedure").ok());
                                if def.is_none() {
                                    debug!(
                                        "SHOW CREATE PROCEDURE returned unexpected columns; falling back to INFORMATION_SCHEMA.ROUTINES"
                                    );
                                }
                                // If SHOW CREATE failed to give us text, fall back to ROUTINES.ROUTINE_DEFINITION (body only)
                                if let Some(text) = def {
                                    Some(text)
                                } else {
                                    // fetch_optional returns Option<Option<String>>; collapse to Option<String>
                                    match sqlx::query_scalar::<_, Option<String>>(
                                        "SELECT ROUTINE_DEFINITION FROM INFORMATION_SCHEMA.ROUTINES \
                                         WHERE ROUTINE_SCHEMA = ? AND ROUTINE_NAME = ? AND ROUTINE_TYPE = 'PROCEDURE'",
                                    )
                                    .bind(&db_name)
                                    .bind(&proc_name)
                                    .fetch_optional(&pool)
                                    .await
                                    {
                                        Ok(opt) => opt.flatten(),
                                        Err(_e2) => None,
                                    }
                                }
                            }
                            Ok(None) => None,
                            Err(e) => {
                                debug!(
                                    "Failed to SHOW CREATE PROCEDURE for {}: {}",
                                    proc_name, e
                                );
                                // Fall back to ROUTINES on SHOW error
                                match sqlx::query_scalar::<_, Option<String>>(
                                    "SELECT ROUTINE_DEFINITION FROM INFORMATION_SCHEMA.ROUTINES \
                                     WHERE ROUTINE_SCHEMA = ? AND ROUTINE_NAME = ? AND ROUTINE_TYPE = 'PROCEDURE'",
                                )
                                .bind(&db_name)
                                .bind(&proc_name)
                                .fetch_optional(&pool)
                                .await
                                {
                                    Ok(v) => v.flatten(),
                                    Err(_e2) => None,
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!("MySQL connection error fetching procedure definition: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::MsSQL => {
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                use tiberius::{AuthMethod, Config};
                let host = connection_clone.host.clone();
                let port: u16 = connection_clone.port.parse().unwrap_or(1433);
                let user = connection_clone.username.clone();
                let pass = connection_clone.password.clone();
                let db = if db_name.is_empty() {
                    connection_clone.database.clone()
                } else {
                    db_name.clone()
                };

                let rt_res: Result<Option<String>, String> = async {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() {
                        config.database(db.clone());
                    }

                    let tcp = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        tokio::net::TcpStream::connect((host.as_str(), port)),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;

                    let mut client = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        tiberius::Client::connect(config, tcp.compat_write()),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;

                    // Parse schema-qualified name
                    let parse_qualified = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(&['[', ']'][..]);
                            let parts: Vec<&str> = trimmed.split("].[" ).collect();
                            if parts.len() >= 2 {
                                return (Some(parts[0].to_string()), parts[1].to_string());
                            }
                        }
                        if let Some((schema, obj)) = name.split_once('.') {
                            return (
                                Some(schema.trim_matches(&['[', ']'][..]).to_string()),
                                obj.trim_matches(&['[', ']'][..]).to_string(),
                            );
                        }
                        (None, name.trim_matches(&['[', ']'][..]).to_string())
                    };

                    let (schema_opt, proc_only) = parse_qualified(&proc_name);
                    let qualified = if let Some(s) = &schema_opt {
                        format!("[{}].[{}]", s, proc_only)
                    } else {
                        format!("[dbo].[{}]", proc_only)
                    };
                    let q = format!(
                        "SELECT OBJECT_DEFINITION(OBJECT_ID(N'{}'))",
                        qualified.replace("'", "''")
                    );

                    let mut stream = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        client.simple_query(q),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;

                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                        if let tiberius::QueryItem::Row(row) = item {
                            let def: Option<&str> = row.get(0);
                            if let Some(create_stmt) = def.map(|s| s.to_string()) {
                                return Ok(Some(create_stmt));
                            }
                        }
                    }
                    Ok::<Option<String>, String>(None)
                }
                .await;

                match rt_res {
                    Ok(result) => result,
                    Err(e) => {
                        debug!("MsSQL error fetching procedure definition: {}", e);
                        None
                    }
                }
            }
            _ => None,
        }
    })
}

pub(crate) fn update_connection_in_database(
    tabular: &mut window_egui::Tabular,
    connection: &models::structs::ConnectionConfig,
) -> bool {
    if let Some(ref pool) = tabular.db_pool {
        if let Some(id) = connection.id {
            let pool_clone = pool.clone();
            let connection = connection.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();

            // Ensure any existing tunnel for this connection is restarted with new settings
            ssh_tunnel::shutdown_for_connection(&connection);

            let result = rt.block_on(async {
                sqlx::query(
                    "UPDATE connections SET name = ?, host = ?, port = ?, username = ?, password = ?, database_name = ?, connection_type = ?, folder = ?, ssh_enabled = ?, ssh_host = ?, ssh_port = ?, ssh_username = ?, ssh_auth_method = ?, ssh_private_key = ?, ssh_password = ?, ssh_accept_unknown_host_keys = ? WHERE id = ?"
                )
                    .bind(connection.name)
                    .bind(connection.host)
                    .bind(connection.port)
                    .bind(connection.username)
                    .bind(connection.password)
                    .bind(connection.database)
                    .bind(format!("{:?}", connection.connection_type))
                    .bind(connection.folder)
                    .bind(if connection.ssh_enabled { 1 } else { 0 })
                    .bind(connection.ssh_host)
                    .bind(connection.ssh_port)
                    .bind(connection.ssh_username)
                    .bind(connection.ssh_auth_method.as_db_value())
                    .bind(connection.ssh_private_key)
                    .bind(connection.ssh_password)
                    .bind(if connection.ssh_accept_unknown_host_keys { 1 } else { 0 })
                    .bind(id)
                    .execute(pool_clone.as_ref())
                    .await
            });

            match &result {
                Ok(query_result) => {
                    debug!(
                        "Update successful: {} rows affected",
                        query_result.rows_affected()
                    );
                }
                Err(e) => {
                    debug!("Update failed: {}", e);
                }
            }

            result.is_ok()
        } else {
            debug!("Cannot update connection: no ID found");
            false
        }
    } else {
        debug!("Cannot update connection: no database pool available");
        false
    }
}

pub(crate) fn remove_connection(tabular: &mut window_egui::Tabular, connection_id: i64) {
    // Remove from database first with explicit transaction
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result: Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> = rt.block_on(async {
            // Begin transaction
            let mut tx = pool_clone.begin().await?;

            // Delete cache data first (foreign key constraints will handle this automatically due to CASCADE)
            let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await;

            let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await;

            let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await;

            // Delete the connection
            let delete_result = sqlx::query("DELETE FROM connections WHERE id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await?;

            // Commit transaction
            tx.commit().await?;

            Ok(delete_result)
        });

        match result {
            Ok(delete_result) => {
                // Only proceed if we actually deleted something
                if delete_result.rows_affected() == 0 {
                    debug!("Warning: No rows were deleted from database!");
                    return;
                }
            }
            Err(e) => {
                debug!("Failed to delete from database: {}", e);
                return; // Don't proceed if database deletion failed
            }
        }
    }

    tabular.connections.retain(|c| c.id != Some(connection_id));
    // Remove from connection pool cache
    tabular.connection_pools.remove(&connection_id);
    tabular.pending_connection_pools.remove(&connection_id);
    ssh_tunnel::shutdown_by_id(connection_id);

    // Use incremental update instead of full refresh
    crate::sidebar_database::remove_connection_from_tree(tabular, connection_id);

    // Set flag to force refresh on next update (for other components if needed)
    tabular.needs_refresh = true;
}

pub(crate) fn test_database_connection(
    connection: &models::structs::ConnectionConfig,
) -> (bool, String) {
    // Do not require ICMP ping; many environments (esp. Windows) block it. Try actual DB connect.
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                let encoded_username = modules::url_encode(&connection.username);
                let encoded_password = modules::url_encode(&connection.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username,
                    encoded_password,
                    target_host,
                    target_port,
                    connection.database
                );

                match MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // Test with a simple query
                        match sqlx::query("SELECT 1").execute(&pool).await {
                            Ok(_) => (true, "MySQL connection successful!".to_string()),
                            Err(e) => (false, format!("MySQL query failed: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("MySQL connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::PostgreSQL => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                let connection_string = format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    connection.username,
                    connection.password,
                    target_host,
                    target_port,
                    connection.database
                );

                match PgPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // Test with a simple query
                        match sqlx::query("SELECT 1").execute(&pool).await {
                            Ok(_) => (true, "PostgreSQL connection successful!".to_string()),
                            Err(e) => (false, format!("PostgreSQL query failed: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("PostgreSQL connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::SQLite => {
                let connection_string = format!("sqlite:{}", connection.host);

                match SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        // Test with a simple query
                        match sqlx::query("SELECT 1").execute(&pool).await {
                            Ok(_) => (true, "SQLite connection successful!".to_string()),
                            Err(e) => (false, format!("SQLite query failed: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("SQLite connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::MongoDB => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                // Build URI and ping
                let uri = if connection.username.is_empty() {
                    format!("mongodb://{}:{}", target_host, target_port)
                } else if connection.password.is_empty() {
                    format!(
                        "mongodb://{}@{}:{}",
                        connection.username, target_host, target_port
                    )
                } else {
                    let enc_user = modules::url_encode(&connection.username);
                    let enc_pass = modules::url_encode(&connection.password);
                    format!(
                        "mongodb://{}:{}@{}:{}",
                        enc_user, enc_pass, target_host, target_port
                    )
                };
                match MongoClient::with_uri_str(uri).await {
                    Ok(client) => {
                        let admin = client.database("admin");
                        match admin.run_command(doc!("ping": 1)).await {
                            Ok(_) => (true, "MongoDB connection successful!".to_string()),
                            Err(e) => (false, format!("MongoDB ping failed: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("MongoDB client error: {}", e)),
                }
            }
            models::enums::DatabaseType::Redis => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                let connection_string = if connection.password.is_empty() {
                    format!("redis://{}:{}", target_host, target_port)
                } else {
                    format!(
                        "redis://{}:{}@{}:{}",
                        connection.username, connection.password, target_host, target_port
                    )
                };

                match Client::open(connection_string) {
                    Ok(client) => {
                        match client.get_connection() {
                            Ok(mut conn) => {
                                // Test with a simple PING command
                                match redis::cmd("PING").query::<String>(&mut conn) {
                                    Ok(response) => {
                                        if response == "PONG" {
                                            (true, "Redis connection successful!".to_string())
                                        } else {
                                            (
                                                false,
                                                "Redis PING returned unexpected response"
                                                    .to_string(),
                                            )
                                        }
                                    }
                                    Err(e) => (false, format!("Redis PING failed: {}", e)),
                                }
                            }
                            Err(e) => (false, format!("Redis connection failed: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("Redis client creation failed: {}", e)),
                }
            }
            models::enums::DatabaseType::MsSQL => {
                // Simple test using tiberius
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                let host = target_host.clone();
                let port: u16 = target_port.parse().unwrap_or(1433);
                let db = connection.database.clone();
                let user = connection.username.clone();
                let pass = connection.password.clone();
                let res = async {
                    use tiberius::{AuthMethod, Config};
                    use tokio_util::compat::TokioAsyncWriteCompatExt;
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() {
                        config.database(db.clone());
                    }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                        .await
                        .map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write())
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut s = client
                        .simple_query("SELECT 1")
                        .await
                        .map_err(|e| e.to_string())?;
                    while let Some(item) = s.try_next().await.map_err(|e| e.to_string())? {
                        if let tiberius::QueryItem::Row(_r) = item {
                            break;
                        }
                    }
                    Ok::<_, String>(())
                }
                .await;
                match res {
                    Ok(_) => (true, "MsSQL connection successful!".to_string()),
                    Err(e) => (false, format!("MsSQL connection failed: {}", e)),
                }
            }
        }
    })
}
