use crate::{
    driver_mssql, driver_mysql, driver_sqlite, models, modules,
    window_egui::Tabular,
};
use log::debug;
use sqlx::{Column, Row, TypeInfo};
use sqlx::Connection as SqlxConnection;
use sqlx::mysql::MySqlConnection;
use std::time::Instant;

use super::pool::resolve_connection_target_async;
use super::sql::{
    infer_column_origins, infer_select_headers, is_comment_only_statement,
    is_simple_select_statement, query_contains_pagination,
    split_sql_statements, statement_returns_rows, strip_leading_sql_comments,
};
use super::types::{
    BackendPidGuard, QueryExecutionError, QueryExecutionOptions, QueryJob, QueryJobOutput,
    QueryPreparationError, QueryResultMessage,
};
use futures_util::TryStreamExt;

/// Membaca result set lewat stream dan berhenti setelah `$max` baris.
/// Menghasilkan `Result<(Vec<Row>, bool /* terpotong */), sqlx::Error>`.
macro_rules! fetch_rows_limited {
    ($query:expr, $executor:expr, $max:expr) => {
        async {
            let mut stream = $query.fetch($executor);
            let mut rows = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if rows.len() >= $max {
                    truncated = true;
                    break;
                }
                rows.push(row);
            }
            Ok::<_, sqlx::Error>((rows, truncated))
        }
    };
}

/// Menjalankan future dengan batas waktu opsional. `Err(())` berarti timeout.
async fn run_with_timeout<F: std::future::Future>(
    timeout: Option<std::time::Duration>,
    fut: F,
) -> Result<F::Output, ()> {
    match timeout {
        Some(limit) => tokio::time::timeout(limit, fut).await.map_err(|_| ()),
        None => Ok(fut.await),
    }
}

/// Pesan error timeout yang konsisten untuk semua driver.
fn timeout_message(options: &QueryExecutionOptions) -> String {
    match options.query_timeout {
        Some(limit) => format!(
            "Query timed out after {}s and was cancelled. Adjust the limit in Settings → Performance → Query timeout.",
            limit.as_secs()
        ),
        None => "Query timed out".to_string(),
    }
}

/// Memecah query job menjadi statement dengan splitter yang paham quote,
/// dollar-quote, dan komentar; statement yang hanya berisi komentar dibuang.
fn job_statements(options: &QueryExecutionOptions) -> Vec<String> {
    let hash_is_comment = matches!(
        options.connection.connection_type,
        models::enums::DatabaseType::MySQL
    );
    split_sql_statements(&options.query, hash_is_comment)
        .into_iter()
        .filter(|s| !is_comment_only_statement(s))
        .collect()
}

/// Potong teks untuk pratinjau tanpa memotong di tengah karakter multibyte.
fn preview_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        format!("{}...", text.chars().take(max_chars).collect::<String>())
    } else {
        text.to_string()
    }
}

/// Minta server menghentikan statement yang sedang berjalan pada sesi `pid`.
/// Dipakai saat user menekan cancel atau saat timeout tercapai.
pub(crate) async fn cancel_backend_query(pool: models::enums::DatabasePool, pid: i64) {
    match pool {
        models::enums::DatabasePool::PostgreSQL(pg) => {
            let result = sqlx::query("SELECT pg_cancel_backend($1)")
                .bind(pid as i32)
                .execute(pg.as_ref())
                .await;
            if let Err(e) = result {
                log::warn!("[CANCEL] pg_cancel_backend({}) failed: {}", pid, e);
            }
        }
        models::enums::DatabasePool::MySQL(my) => {
            let kill = format!("KILL QUERY {}", pid);
            if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(kill.as_str()))
                .execute(my.as_ref())
                .await
            {
                log::warn!("[CANCEL] KILL QUERY {} failed: {}", pid, e);
            }
        }
        _ => {}
    }
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
        schema_name: tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.schema_name.clone())
            .filter(|s| !s.trim().is_empty()),
        use_server_pagination: tabular.use_server_pagination,
        current_page: tabular.current_page,
        page_size: tabular.page_size,
        base_query,
        dba_special_mode,
        save_to_history: true,
        ast_enabled: cfg!(feature = "query_ast"),
        job_id,
        query_timeout: (tabular.query_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(tabular.query_timeout_secs as u64)),
        max_rows: tabular.max_result_rows.max(1) as usize,
        backend_pids: tabular.jobs.backend_pids.clone(),
    };

    let tab_id = tabular.query_tabs.get(tabular.active_tab_index).map(|t| t.id);

    Ok(QueryJob {
        job_id,
        tab_id,
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

/// Run a batch of statements **sequentially** on one task so script-like
/// input (`CREATE …; INSERT …; SELECT …`) executes in order instead of
/// racing on separate pool connections. Each statement reports its own
/// `QueryResultMessage`; after the first failure the remaining statements
/// are skipped (reported as errors) rather than executed.
pub(crate) fn spawn_query_job_batch(
    tabular: &mut Tabular,
    jobs: Vec<QueryJob>,
    sender: std::sync::mpsc::Sender<QueryResultMessage>,
) -> Result<tokio::task::JoinHandle<()>, QueryPreparationError> {
    let runtime = tabular
        .runtime
        .clone()
        .ok_or(QueryPreparationError::RuntimeUnavailable)?;

    let handle = runtime.spawn(async move {
        let mut previous_failed = false;
        for mut job in jobs {
            if previous_failed {
                let _ = sender.send(skipped_statement_message(&job));
                continue;
            }
            // Reset the clock so each statement reports its own duration,
            // not the time spent waiting behind earlier statements.
            job.started_at = Instant::now();
            let result = execute_query_job(job).await;
            previous_failed = !result.success;
            let _ = sender.send(result);
        }
    });

    Ok(handle)
}

fn skipped_statement_message(job: &QueryJob) -> QueryResultMessage {
    let message = "Skipped: a previous statement in this batch failed".to_string();
    QueryResultMessage {
        job_id: job.job_id,
        tab_id: job.tab_id,
        connection_id: job.options.connection_id,
        success: false,
        headers: vec!["Error".to_string()],
        rows: vec![vec![message.clone()]],
        error: Some(message),
        duration: std::time::Duration::ZERO,
        query: job.options.query.clone(),
        dba_special_mode: job.options.dba_special_mode.clone(),
        ast_debug_sql: None,
        ast_headers: None,
        affected_rows: None,
        column_metadata: None,
        truncated: false,
        error_location: None,
    }
}

async fn execute_query_job(job: QueryJob) -> QueryResultMessage {
    let start = job.started_at;
    let tab_id = job.tab_id;
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
        models::enums::DatabaseType::ApiHttp => Err(QueryExecutionError::Message(
            "API-HTTP connections do not support SQL queries".to_string(),
        )),
    };

    match outcome {
        Ok(output) => QueryResultMessage {
            job_id: job.job_id,
            tab_id,
            connection_id,
            success: true,
            headers: output.headers.clone(),
            rows: output.rows.clone(),
            error: None,
            duration: start.elapsed(),
            query: query.clone(),
            dba_special_mode,
            ast_debug_sql: output.ast_debug_sql,
            ast_headers: output.ast_headers,
            affected_rows: output.affected_rows.map(|n| n as usize),
            column_metadata: output.column_metadata,
            truncated: output.truncated,
            error_location: None,
        },
        Err(err) => {
            let (message, error_location) = describe_execution_error(err);
            QueryResultMessage {
                job_id: job.job_id,
                tab_id,
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
                affected_rows: None,
                column_metadata: None,
                truncated: false,
                error_location,
            }
        }
    }
}

fn describe_execution_error(
    err: QueryExecutionError,
) -> (String, Option<super::types::ErrorLocation>) {
    match err {
        QueryExecutionError::Message(msg) => (msg, None),
        QueryExecutionError::Located(msg, location) => (msg, Some(location)),
    }
}

/// Posisi error dari PostgreSQL (field `position`, dalam karakter, 1-based).
fn postgres_error_location(err: &sqlx::Error, statement: &str) -> Option<super::types::ErrorLocation> {
    let sqlx::Error::Database(db_err) = err else {
        return None;
    };
    let pg = db_err.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()?;
    match pg.position()? {
        sqlx::postgres::PgErrorPosition::Original(position) => Some(super::types::ErrorLocation {
            statement: statement.to_string(),
            char_offset: Some(position.saturating_sub(1)),
            line: None,
        }),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-driver async execution helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn execute_mysql_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    debug!(
        "[async] Executing MySQL query (conn_id={})",
        options.connection_id
    );

    let (target_host, target_port) = resolve_connection_target_async(&options.connection)
        .await
        .map_err(QueryExecutionError::Message)?;

    let statements_owned = job_statements(options);
    let statements_raw: Vec<&str> = statements_owned.iter().map(|s| s.as_str()).collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = {
        let allow_ast_rewrite = options.ast_enabled
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
            && is_simple_select_statement(statements_raw[0]);

        if allow_ast_rewrite {
            let should_paginate =
                options.use_server_pagination && !query_contains_pagination(statements_raw[0]);
            let pagination_opt = if should_paginate {
                Some((options.current_page as u64, options.page_size as u64))
            } else {
                None
            };
            let inject_auto_limit = should_paginate;
            match crate::query_ast::compile_single_select(
                statements_raw[0],
                &options.connection.connection_type,
                pagination_opt,
                inject_auto_limit,
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
        }
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

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

    debug!(
        "[mysql] target={}:{}, selected_database={:?}, default_db={}",
        target_host, target_port, options.selected_database, default_db
    );

    let replication_status_mode = matches!(
        options.dba_special_mode,
        Some(models::enums::DBASpecialMode::ReplicationStatus)
    );
    let master_status_mode = matches!(
        options.dba_special_mode,
        Some(models::enums::DBASpecialMode::MasterStatus)
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
    let mut failing_stmt_preview: Option<String> = None;
    let mut error_location: Option<super::types::ErrorLocation> = None;

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

        // Catat connection id supaya cancel/timeout bisa mengirim KILL QUERY.
        let mut _pid_guard = sqlx::query_scalar::<_, u64>("SELECT CONNECTION_ID()")
            .fetch_one(&mut conn)
            .await
            .ok()
            .map(|pid| BackendPidGuard::register(&options.backend_pids, options.job_id, pid as i64));

        let mut final_headers: Vec<String> = Vec::new();
        let mut final_data: Vec<Vec<String>> = Vec::new();
        let mut final_column_metadata: Option<Vec<models::structs::ColumnMetadata>> = None;
        let mut final_affected: Option<u64> = None;
        let mut final_truncated = false;
        let mut execution_success = true;

        for (idx, statement) in statements_ref.iter().enumerate() {
            let trimmed = statement.trim();
            if is_comment_only_statement(trimmed) {
                continue;
            }

            debug!("[mysql] about to run statement[{}]: {:?}", idx + 1, trimmed);

            let upper = strip_leading_sql_comments(trimmed).to_uppercase();

            let is_admin_command = {
                upper.starts_with("PURGE BINARY LOGS")
                    || upper.starts_with("PURGE MASTER LOGS")
                    || upper.starts_with("RESET MASTER")
                    || upper.starts_with("RESET SLAVE")
                    || upper.starts_with("RESET REPLICA")
                    || upper.starts_with("CHANGE MASTER")
                    || upper.starts_with("CHANGE REPLICATION SOURCE")
                    || upper.starts_with("FLUSH")
            };

            if upper.starts_with("USE ") {
                let db_part = strip_leading_sql_comments(trimmed)[3..].trim();
                let db_name = db_part
                    .trim_matches('`')
                    .trim_matches('"')
                    .trim_matches('[')
                    .trim_matches(']')
                    .trim();

                let use_stmt = format!("USE `{}`", db_name);
                if sqlx::query(sqlx::AssertSqlSafe(use_stmt.as_str())).execute(&mut conn).await.is_err() {
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
                            _pid_guard = sqlx::query_scalar::<_, u64>("SELECT CONNECTION_ID()")
                                .fetch_one(&mut conn)
                                .await
                                .ok()
                                .map(|pid| {
                                    BackendPidGuard::register(
                                        &options.backend_pids,
                                        options.job_id,
                                        pid as i64,
                                    )
                                });
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

            let returns_rows = statement_returns_rows(trimmed);
            let query_result = run_with_timeout(options.query_timeout, async {
                if returns_rows {
                    fetch_rows_limited!(
                        sqlx::query(sqlx::AssertSqlSafe(trimmed)),
                        &mut conn,
                        options.max_rows
                    )
                    .await
                    .map(|(rows, truncated)| (rows, truncated, None))
                } else {
                    sqlx::query(sqlx::AssertSqlSafe(trimmed))
                        .execute(&mut conn)
                        .await
                        .map(|r| (Vec::new(), false, Some(r.rows_affected())))
                }
            })
            .await;

            match query_result {
                Ok(Ok((rows, truncated, affected))) => {
                    if idx == statements_ref.len() - 1 {
                        final_affected = affected;
                        final_truncated = truncated;
                        if !rows.is_empty() {
                            final_headers = rows[0]
                                .columns()
                                .iter()
                                .map(|c| c.name().to_string())
                                .collect();

                            let mut meta_vec = Vec::new();
                            let mut inferred_table_name = None;
                            if let Ok(ast) = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::MySqlDialect {}, trimmed)
                                && let Some(sqlparser::ast::Statement::Query(q)) = ast.first()
                                && let sqlparser::ast::SetExpr::Select(select) = &*q.body
                                && let Some(table_with_joins) = select.from.first()
                                && let sqlparser::ast::TableFactor::Table { name, .. } = &table_with_joins.relation
                            {
                                inferred_table_name = Some(name.to_string());
                                log::debug!("🔥 Inferred table name: {}", name);
                            } else {
                                log::warn!("🔥 Failed to infer table name from query: {}", trimmed);
                            }

                            let mut unique_tables = std::collections::HashSet::new();
                            for _col in rows[0].columns() {
                                let t_name = String::new();
                                if !t_name.is_empty() {
                                    unique_tables.insert(t_name.clone());
                                }
                            }
                            if let Some(t) = &inferred_table_name {
                                unique_tables.insert(t.clone());
                            }

                            let mut table_pks: std::collections::HashMap<String, std::collections::HashSet<String>> = std::collections::HashMap::new();

                            let data_dir = crate::directory::get_data_dir();
                            let db_path = data_dir.join("connections.db");
                            let cache_conn_str = format!("sqlite://{}?mode=ro", db_path.to_string_lossy());

                            match sqlx::sqlite::SqlitePool::connect(&cache_conn_str).await {
                                Ok(cache_pool) => {
                                    for table_full_name in &unique_tables {
                                        let parts: Vec<&str> = table_full_name.split('.').collect();
                                        let (target_db, target_table) = if parts.len() >= 2 {
                                            (parts[0], parts[1])
                                        } else {
                                            (default_db.as_str(), table_full_name.as_str())
                                        };

                                        let query = "SELECT columns_json FROM index_cache \
                                             WHERE connection_id = ? \
                                             AND database_name = ? \
                                             AND table_name LIKE ? \
                                             AND index_name = 'PRIMARY'";

                                        let result: Result<Option<(String,)>, _> = sqlx::query_as(query)
                                            .bind(options.connection.id.unwrap_or(0))
                                            .bind(target_db)
                                            .bind(target_table)
                                            .fetch_optional(&cache_pool)
                                            .await;

                                        match result {
                                            Ok(Some((json_str,))) => {
                                                if let Ok(cols) = serde_json::from_str::<Vec<String>>(&json_str)
                                                    && !cols.is_empty()
                                                {
                                                    let pks: std::collections::HashSet<String> =
                                                        cols.into_iter().map(|s| s.to_lowercase()).collect();
                                                    debug!("Found cached PKs for '{}': {:?}", table_full_name, pks);
                                                    table_pks.insert(table_full_name.to_lowercase(), pks);
                                                }
                                            }
                                            Ok(None) => {
                                                debug!("No cached PK found for '{}' (db={}, tbl={})", table_full_name, target_db, target_table);
                                            }
                                            Err(e) => {
                                                debug!("Error fetching PK from cache for '{}': {}", table_full_name, e);
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    debug!("Failed to connect to local cache at {}: {}", db_path.display(), e);
                                }
                            }

                            let (inferred_origins, involved_tables) = infer_column_origins(trimmed);

                            let mut expanded_schema: Vec<(String, String)> = Vec::new();

                            let exact_match_possible = if let Some(origins) = &inferred_origins {
                                origins.len() == rows[0].columns().len()
                                    && origins.iter().all(|o| o.is_some())
                            } else {
                                false
                            };

                            if !exact_match_possible && !involved_tables.is_empty() {
                                log::debug!("🔥 Fetching ordered schema for involved tables: {:?}", involved_tables);
                                for table in &involved_tables {
                                    let col_query = format!("SHOW COLUMNS FROM {}", table);
                                    if let Ok(col_rows) =
                                        sqlx::query(sqlx::AssertSqlSafe(col_query.as_str())).fetch_all(&mut conn).await
                                    {
                                        for row in col_rows {
                                            if let Ok(col_name) =
                                                row.try_get::<String, _>("Field")
                                            {
                                                expanded_schema
                                                    .push((col_name, table.clone()));
                                            }
                                        }
                                    }
                                }
                            }

                            let use_fine_grained = if let Some(origins) = &inferred_origins {
                                origins.len() == rows[0].columns().len()
                            } else {
                                false
                            };

                            if use_fine_grained {
                                log::debug!("🔥 Using fine-grained column table inference");
                            }

                            for (i, col) in rows[0].columns().iter().enumerate() {
                                let type_info = col.type_info();
                                let t_name = String::new();

                                log::debug!("🔥 [debug] inferring table for col '{}': t_name='{}', use_fine_grained={}, involved_tables={:?}, expanded_len={}",
                                    col.name(), t_name, use_fine_grained, involved_tables, expanded_schema.len());

                                let table_name = if !t_name.is_empty() {
                                    Some(t_name.clone())
                                } else {
                                    let ast_name = if use_fine_grained {
                                        inferred_origins
                                            .as_ref()
                                            .and_then(|o| o.get(i).cloned().flatten())
                                    } else {
                                        None
                                    };

                                    if ast_name.is_some() {
                                        ast_name
                                    } else if involved_tables.len() == 1 {
                                        Some(involved_tables[0].clone())
                                    } else if expanded_schema.len() == rows[0].columns().len() {
                                        Some(expanded_schema[i].1.clone())
                                    } else {
                                        None
                                    }
                                };

                                let is_pk = if let Some(t) = &table_name {
                                    let key = t.to_lowercase();
                                    if let Some(pks) = table_pks.get(&key) {
                                        pks.contains(&col.name().to_lowercase())
                                    } else if let Some(simple_name) = key.split('.').next_back()
                                        && let Some(pks) = table_pks.get(simple_name)
                                    {
                                        pks.contains(&col.name().to_lowercase())
                                    } else if let Some((_k, pks)) = table_pks.iter().find(|(k, _)| {
                                        k.ends_with(&format!(".{}", key))
                                    }) {
                                        pks.contains(&col.name().to_lowercase())
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if let Some(final_t) = &table_name {
                                    log::debug!("🔥 [debug] -> Resolved table: {}", final_t);
                                } else {
                                    log::debug!("🔥 [debug] -> Resolved table: NONE");
                                }

                                meta_vec.push(models::structs::ColumnMetadata {
                                    name: col.name().to_string(),
                                    type_name: type_info.name().to_string(),
                                    table_name,
                                    original_name: Some(col.name().to_string()),
                                    is_primary_key: is_pk,
                                });
                            }
                            final_column_metadata = Some(meta_vec);

                            final_data = driver_mysql::convert_mysql_rows_to_table_data(rows);

                            if replication_status_mode || master_status_mode {
                                let version_str = match sqlx::query("SELECT VERSION() AS v")
                                    .fetch_one(&mut conn)
                                    .await
                                {
                                    Ok(vrow) => {
                                        vrow.try_get::<String, _>("v").unwrap_or_default()
                                    }
                                    Err(_) => String::new(),
                                };
                                let is_mariadb =
                                    version_str.to_lowercase().contains("mariadb");

                                if replication_status_mode
                                    && final_data.is_empty()
                                    && let Ok(fallback_rows) =
                                        sqlx::query("SHOW SLAVE STATUS")
                                            .fetch_all(&mut conn)
                                            .await
                                    && !fallback_rows.is_empty()
                                {
                                    final_headers = fallback_rows[0]
                                        .columns()
                                        .iter()
                                        .map(|c| c.name().to_string())
                                        .collect();
                                    final_data =
                                        driver_mysql::convert_mysql_rows_to_table_data(
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
                                        if let Some(idx) =
                                            header_index("Replica_IO_Running")
                                                .or_else(|| header_index("Slave_IO_Running"))
                                        {
                                            summary.push(("IO Thread".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) =
                                            header_index("Replica_SQL_Running")
                                                .or_else(|| header_index("Slave_SQL_Running"))
                                        {
                                            summary.push((
                                                "SQL Thread".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) =
                                            header_index("Seconds_Behind_Source").or_else(|| {
                                                header_index("Seconds_Behind_Master")
                                            })
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
                                            summary.push((
                                                "Executed GTID".into(),
                                                first[idx].clone(),
                                            ));
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
                                            summary
                                                .push(("Position".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Binlog_Do_DB") {
                                            summary.push((
                                                "Binlog Do DB".into(),
                                                first[idx].clone(),
                                            ));
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

                    if is_admin_command
                        && (err_str.contains("1295")
                            || err_str.contains("prepared statement protocol"))
                    {
                        debug!("Admin command executed successfully (error 1295 expected for prepared statements)");
                        if idx == statements_ref.len() - 1 {
                            final_headers = vec!["Status".to_string()];
                            final_data =
                                vec![vec!["Command executed successfully".to_string()]];
                        }
                    } else {
                        if failing_stmt_preview.is_none() {
                            failing_stmt_preview = Some(preview_text(trimmed, 200));
                        }
                        if let Some(line) = super::sql::mysql_error_line(&err_str) {
                            error_location = Some(super::types::ErrorLocation {
                                statement: trimmed.to_string(),
                                char_offset: None,
                                line: Some(line),
                            });
                        }
                        if err_str.contains("1146")
                            || err_str.to_lowercase().contains("doesn't exist")
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
                }
                Err(_) => {
                    // Future yang di-drop tidak menghentikan query di server,
                    // jadi kirim KILL QUERY lewat koneksi lain dari pool.
                    let pid = options
                        .backend_pids
                        .lock()
                        .ok()
                        .and_then(|m| m.get(&options.job_id).copied());
                    if let Some(pid) = pid {
                        cancel_backend_query(pool.clone(), pid).await;
                    }
                    last_error = Some(timeout_message(options));
                    failing_stmt_preview.get_or_insert_with(|| preview_text(trimmed, 200));
                    execution_success = false;
                    break;
                }
            }
        }

        if execution_success {
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
                column_metadata: final_column_metadata,
                affected_rows: final_affected,
                truncated: final_truncated,
            });
        }

        // Koneksi sudah terbentuk tetapi statement gagal atau timeout. Jangan
        // diulang: statement sebelumnya (atau statement yang timeout itu
        // sendiri) mungkin sudah berefek, sehingga retry bisa menjalankan DML
        // dua kali. Retry hanya untuk kegagalan membuka koneksi (lihat `continue`
        // di atas).
        break;
    }

    let mut final_err = last_error.unwrap_or_else(|| "Unknown MySQL error".to_string());
    if let Some(stmt) = failing_stmt_preview {
        final_err = format!("{}\n\nFailed statement (preview): {}", final_err, stmt);
    }
    Err(match error_location {
        Some(location) => QueryExecutionError::Located(final_err, location),
        None => QueryExecutionError::Message(final_err),
    })
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

    let statements_owned = job_statements(options);
    let statements_raw: Vec<&str> = statements_owned.iter().map(|s| s.as_str()).collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    let mut ast_debug_sql: Option<String> = None;

    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = {
        let allow_ast_rewrite = options.ast_enabled
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
            && is_simple_select_statement(statements_raw[0]);

        if allow_ast_rewrite {
            let should_paginate =
                options.use_server_pagination && !query_contains_pagination(statements_raw[0]);
            let pagination_opt = if should_paginate {
                Some((options.current_page as u64, options.page_size as u64))
            } else {
                None
            };
            let inject_auto_limit = should_paginate;
            match crate::query_ast::compile_single_select(
                statements_raw[0],
                &options.connection.connection_type,
                pagination_opt,
                inject_auto_limit,
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
        }
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    // Semua statement dalam job memakai satu koneksi yang sama, sehingga SET /
    // search_path dan statement berikutnya konsisten, dan backend pid-nya
    // diketahui untuk keperluan cancel.
    let mut conn = pg_pool.acquire().await.map_err(|e| {
        QueryExecutionError::Message(format!("PostgreSQL connection error: {}", e))
    })?;
    let _pid_guard = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
        .fetch_one(&mut *conn)
        .await
        .ok()
        .map(|pid| BackendPidGuard::register(&options.backend_pids, options.job_id, pid as i64));

    // Terapkan schema aktif tab di koneksi ini juga. `SET search_path` yang
    // dijalankan terpisah bisa mendarat di koneksi pool lain dan tidak berefek.
    if let Some(schema) = options.schema_name.as_deref() {
        let set_path = format!(
            "SET search_path TO \"{}\", public",
            schema.replace('"', "\"\"")
        );
        if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(set_path.as_str()))
            .execute(&mut *conn)
            .await
        {
            return Err(QueryExecutionError::Message(format!(
                "Cannot switch to schema '{}': {}",
                schema, e
            )));
        }
    }

    let mut final_headers = Vec::new();
    let mut final_data = Vec::new();
    let mut final_affected: Option<u64> = None;
    let mut final_truncated = false;

    for (i, statement) in statements_ref.iter().enumerate() {
        let trimmed = statement.trim();
        if is_comment_only_statement(trimmed) {
            continue;
        }

        let returns_rows = statement_returns_rows(trimmed);
        let result = run_with_timeout(options.query_timeout, async {
            if returns_rows {
                fetch_rows_limited!(
                    sqlx::query(sqlx::AssertSqlSafe(trimmed)),
                    &mut *conn,
                    options.max_rows
                )
                .await
                .map(|(rows, truncated)| (rows, truncated, None))
            } else {
                sqlx::query(sqlx::AssertSqlSafe(trimmed))
                    .execute(&mut *conn)
                    .await
                    .map(|r| (Vec::new(), false, Some(r.rows_affected())))
            }
        })
        .await;

        match result {
            Ok(Ok((rows, truncated, affected))) => {
                if i == statements_ref.len() - 1 {
                    final_affected = affected;
                    final_truncated = truncated;
                    if !rows.is_empty() {
                        final_headers = rows[0]
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        final_data = crate::driver_postgres::convert_postgres_rows_to_table_data(rows);
                    } else {
                        #[cfg(feature = "query_ast")]
                        if final_headers.is_empty()
                            && let Some(hh) = inferred_headers_from_ast.clone()
                            && !hh.is_empty()
                        {
                            final_headers = hh;
                        }
                        if final_headers.is_empty()
                            && strip_leading_sql_comments(trimmed)
                                .to_uppercase()
                                .starts_with("SELECT")
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
                let message = format!("PostgreSQL error: {}", e);
                return Err(match postgres_error_location(&e, trimmed) {
                    Some(location) => QueryExecutionError::Located(message, location),
                    None => QueryExecutionError::Message(message),
                });
            }
            Err(_) => {
                // Drop future tidak menghentikan query di server; kirim
                // pg_cancel_backend lewat koneksi lain dari pool.
                let pid = options
                    .backend_pids
                    .lock()
                    .ok()
                    .and_then(|m| m.get(&options.job_id).copied());
                // Koneksi ini masih menunggu hasil query yang dibatalkan;
                // lepaskan (tutup) supaya tidak dikembalikan ke pool.
                drop(conn.detach());
                if let Some(pid) = pid {
                    cancel_backend_query(
                        models::enums::DatabasePool::PostgreSQL(pg_pool.clone()),
                        pid,
                    )
                    .await;
                }
                return Err(QueryExecutionError::Message(timeout_message(options)));
            }
        }
    }

    Ok(QueryJobOutput {
        headers: final_headers,
        rows: final_data,
        ast_debug_sql,
        ast_headers,
        column_metadata: None,
        affected_rows: final_affected,
        truncated: final_truncated,
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

    let statements_owned = job_statements(options);
    let statements_raw: Vec<&str> = statements_owned.iter().map(|s| s.as_str()).collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    let mut ast_debug_sql: Option<String> = None;

    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = {
        let allow_ast_rewrite = options.ast_enabled
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
            && is_simple_select_statement(statements_raw[0]);

        if allow_ast_rewrite {
            let should_paginate =
                options.use_server_pagination && !query_contains_pagination(statements_raw[0]);
            let pagination_opt = if should_paginate {
                Some((options.current_page as u64, options.page_size as u64))
            } else {
                None
            };
            let inject_auto_limit = should_paginate;
            match crate::query_ast::compile_single_select(
                statements_raw[0],
                &options.connection.connection_type,
                pagination_opt,
                inject_auto_limit,
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
        }
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    let mut final_headers = Vec::new();
    let mut final_data = Vec::new();
    let mut final_affected: Option<u64> = None;
    let mut final_truncated = false;

    for (i, statement) in statements_ref.iter().enumerate() {
        let trimmed = statement.trim();
        if is_comment_only_statement(trimmed) {
            continue;
        }

        let returns_rows = statement_returns_rows(trimmed);
        let result = run_with_timeout(options.query_timeout, async {
            if returns_rows {
                fetch_rows_limited!(
                    sqlx::query(sqlx::AssertSqlSafe(trimmed)),
                    sqlite_pool.as_ref(),
                    options.max_rows
                )
                .await
                .map(|(rows, truncated)| (rows, truncated, None))
            } else {
                sqlx::query(sqlx::AssertSqlSafe(trimmed))
                    .execute(sqlite_pool.as_ref())
                    .await
                    .map(|r| (Vec::new(), false, Some(r.rows_affected())))
            }
        })
        .await;

        match result {
            Ok(Ok((rows, truncated, affected))) => {
                if i == statements_ref.len() - 1 {
                    final_affected = affected;
                    final_truncated = truncated;
                    if !rows.is_empty() {
                        final_headers = rows[0]
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        final_data = driver_sqlite::convert_sqlite_rows_to_table_data(rows);
                    } else {
                        #[cfg(feature = "query_ast")]
                        if final_headers.is_empty()
                            && let Some(hh) = inferred_headers_from_ast.clone()
                            && !hh.is_empty()
                        {
                            final_headers = hh;
                        }
                        if final_headers.is_empty()
                            && strip_leading_sql_comments(trimmed)
                                .to_uppercase()
                                .starts_with("SELECT")
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
                return Err(QueryExecutionError::Message(timeout_message(options)));
            }
        }
    }

    Ok(QueryJobOutput {
        headers: final_headers,
        rows: final_data,
        ast_debug_sql,
        ast_headers,
        column_metadata: None,
        affected_rows: final_affected,
        truncated: final_truncated,
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
                    column_metadata: None,
                    affected_rows: None,
                    truncated: false,
                }),
                Ok(Ok(None)) => Ok(QueryJobOutput {
                    headers: vec!["Key".to_string(), "Value".to_string()],
                    rows: vec![vec![parts[1].to_string(), "NULL".to_string()]],
                    ast_debug_sql: None,
                    ast_headers: None,
                    column_metadata: None,
                    affected_rows: None,
                    truncated: false,
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
                    let table_data: Vec<Vec<String>> =
                        keys.into_iter().map(|k| vec![k]).collect();
                    Ok(QueryJobOutput {
                        headers: vec!["Key".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
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
                            table_data
                                .push(vec!["Sample Keys Found".to_string(), "".to_string()]);
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
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
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
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
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
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
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
            column_metadata: None,
            affected_rows: None,
            truncated: false,
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
            column_metadata: None,
            affected_rows: None,
            truncated: false,
        }),
        _ => Err(QueryExecutionError::Message(
            "Invalid pool type for MongoDB".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn sqlite_job(query: &str, max_rows: usize) -> QueryResultMessage {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        for stmt in [
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO t (name) VALUES ('a'), ('b;c'), ('d')",
        ] {
            sqlx::query(stmt).execute(&pool).await.expect("seed");
        }
        let connection = models::structs::ConnectionConfig {
            connection_type: models::enums::DatabaseType::SQLite,
            ..Default::default()
        };
        let job = QueryJob {
            job_id: 7,
            tab_id: Some(3),
            options: QueryExecutionOptions {
                connection_id: 1,
                connection,
                query: query.to_string(),
                selected_database: None,
                schema_name: None,
                use_server_pagination: false,
                current_page: 0,
                page_size: 100,
                base_query: None,
                dba_special_mode: None,
                save_to_history: false,
                ast_enabled: false,
                job_id: 7,
                query_timeout: None,
                max_rows,
                backend_pids: Default::default(),
            },
            connection_pool: models::enums::DatabasePool::SQLite(Arc::new(pool)),
            started_at: Instant::now(),
        };
        execute_query_job(job).await
    }

    #[tokio::test]
    async fn statement_with_leading_comment_is_executed() {
        let msg = sqlite_job("-- ambil semua\nSELECT name FROM t ORDER BY id", 100).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.tab_id, Some(3));
        assert_eq!(msg.rows.len(), 3);
        assert_eq!(msg.affected_rows, None);
    }

    #[tokio::test]
    async fn semicolon_inside_string_is_not_split() {
        let msg = sqlite_job("SELECT id FROM t WHERE name = 'b;c'", 100).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.rows, vec![vec!["2".to_string()]]);
    }

    #[tokio::test]
    async fn update_reports_driver_affected_rows() {
        let msg = sqlite_job("UPDATE t SET name = 'z' WHERE id >= 2", 100).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.affected_rows, Some(2));
        assert!(msg.rows.is_empty());
    }

    #[tokio::test]
    async fn result_set_is_truncated_at_row_limit() {
        let msg = sqlite_job("SELECT * FROM t", 2).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.rows.len(), 2);
        assert!(msg.truncated);
    }

    #[tokio::test]
    async fn sql_error_is_reported_as_failure() {
        let msg = sqlite_job("SELECT * FROM missing_table", 100).await;
        assert!(!msg.success);
        assert!(msg.error.unwrap_or_default().contains("missing_table"));
    }
}
