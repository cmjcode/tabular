use crate::{
    driver_mssql, driver_mysql, driver_sqlite, models, modules,
    window_egui::Tabular,
};
use log::debug;
use sqlx::{Column, Row, TypeInfo};
use sqlx::Connection as SqlxConnection;
use sqlx::mysql::MySqlConnection;
use std::sync::Arc;
use std::time::Instant;

use super::pool::{resolve_connection_target, try_get_connection_pool};
use super::sql::{
    infer_column_origins, infer_select_headers, is_simple_select_statement,
    query_contains_pagination, should_enable_auto_pagination,
};
use super::types::{
    QueryExecutionError, QueryExecutionOptions, QueryJob, QueryJobOutput, QueryPreparationError,
    QueryResultMessage,
};

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
        models::enums::DatabaseType::ApiHttp => Err(QueryExecutionError::Message(
            "API-HTTP connections do not support SQL queries".to_string(),
        )),
    };

    match outcome {
        Ok(output) => QueryResultMessage {
            job_id: job.job_id,
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
            affected_rows: Some(output.rows.len()),
            column_metadata: output.column_metadata,
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
                affected_rows: None,
                column_metadata: None,
            }
        }
    }
}

fn describe_execution_error(err: QueryExecutionError) -> String {
    match err {
        QueryExecutionError::Message(msg) => msg,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-driver async execution helpers
// ─────────────────────────────────────────────────────────────────────────────

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
        let mut final_column_metadata: Option<Vec<models::structs::ColumnMetadata>> = None;
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

            debug!("[mysql] about to run statement[{}]: {:?}", idx + 1, trimmed);

            let upper = trimmed.to_uppercase();

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

                            let mut meta_vec = Vec::new();
                            let mut inferred_table_name = None;
                            if let Ok(ast) = sqlparser::parser::Parser::parse_sql(&sqlparser::dialect::MySqlDialect {}, trimmed)
                                && let Some(sqlparser::ast::Statement::Query(q)) = ast.first()
                                && let sqlparser::ast::SetExpr::Select(select) = &*q.body
                                && let Some(table_with_joins) = select.from.first()
                                && let sqlparser::ast::TableFactor::Table { name, .. } = &table_with_joins.relation
                            {
                                inferred_table_name = Some(name.to_string());
                                log::info!("🔥 Inferred table name: {}", name);
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
                                                    std::println!("🔥 Found cached PKs for '{}': {:?}", table_full_name, pks);
                                                    table_pks.insert(table_full_name.to_lowercase(), pks);
                                                }
                                            }
                                            Ok(None) => {
                                                std::println!("🔥 No cached PK found for '{}' (db={}, tbl={})", table_full_name, target_db, target_table);
                                            }
                                            Err(e) => {
                                                std::println!("🔥 Error fetching PK from cache for '{}': {}", table_full_name, e);
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    std::println!("🔥 Failed to connect to local cache at {}: {}", db_path.display(), e);
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
                                log::info!("🔥 Fetching ordered schema for involved tables: {:?}", involved_tables);
                                for table in &involved_tables {
                                    let col_query = format!("SHOW COLUMNS FROM {}", table);
                                    if let Ok(col_rows) =
                                        sqlx::query(&col_query).fetch_all(&mut conn).await
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
                                log::info!("🔥 Using fine-grained column table inference");
                            }

                            for (i, col) in rows[0].columns().iter().enumerate() {
                                let type_info = col.type_info();
                                let t_name = String::new();

                                log::info!("🔥 [debug] inferring table for col '{}': t_name='{}', use_fine_grained={}, involved_tables={:?}, expanded_len={}",
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
                                    log::info!("🔥 [debug] -> Resolved table: {}", final_t);
                                } else {
                                    log::info!("🔥 [debug] -> Resolved table: NONE");
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
                            let prev = if trimmed.len() > 200 {
                                format!("{}...", &trimmed[..200])
                            } else {
                                trimmed.to_string()
                            };
                            failing_stmt_preview = Some(prev);
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
                    last_error = Some("Query timeout after 60s".to_string());
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
        column_metadata: None,
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
        column_metadata: None,
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
                }),
                Ok(Ok(None)) => Ok(QueryJobOutput {
                    headers: vec!["Key".to_string(), "Value".to_string()],
                    rows: vec![vec![parts[1].to_string(), "NULL".to_string()]],
                    ast_debug_sql: None,
                    ast_headers: None,
                    column_metadata: None,
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
        }),
        _ => Err(QueryExecutionError::Message(
            "Invalid pool type for MongoDB".to_string(),
        )),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Synchronous execution entry points
// ─────────────────────────────────────────────────────────────────────────────

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
        let selected_db = tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.database_name.clone())
            .filter(|s| !s.is_empty());

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

        {
            let should_auto_paginate = should_enable_auto_pagination(&final_query);

            if should_auto_paginate {
                match connection.connection_type {
                    models::enums::DatabaseType::MySQL
                    | models::enums::DatabaseType::PostgreSQL
                    | models::enums::DatabaseType::SQLite => {
                        let base = final_query.trim().trim_end_matches(';').to_string();
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
                    _ => {}
                }
            }
        }

        debug!("Final query to execute: {}", final_query);
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

                        let statements: Vec<&str> = query
                            .split(';')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .collect();
                        #[cfg(feature = "query_ast")]
                        let mut _inferred_headers_from_ast: Option<Vec<String>> = None;
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<String> = {
                            let allow_ast_rewrite = statements.len() == 1
                                && statements[0].to_uppercase().starts_with("SELECT")
                                && is_simple_select_statement(statements[0]);

                            if allow_ast_rewrite {
                                let should_paginate = tabular.use_server_pagination
                                    && !query_contains_pagination(statements[0]);
                                let pagination_opt = if should_paginate {
                                    Some((tabular.current_page as u64, tabular.page_size as u64))
                                } else {
                                    None
                                };
                                let inject_auto_limit = should_paginate;
                                match crate::query_ast::compile_single_select(
                                    statements[0],
                                    &connection.connection_type,
                                    pagination_opt,
                                    inject_auto_limit,
                                ) {
                                    Ok((new_sql, hdrs)) => {
                                        if !hdrs.is_empty() {
                                            _inferred_headers_from_ast = Some(hdrs.clone());
                                        }
                                        tabular.last_compiled_sql = Some(new_sql.clone());
                                        tabular.last_compiled_headers = hdrs.clone();
                                        if let Ok(plan_txt) =
                                            crate::query_ast::debug_plan(statements[0], &connection.connection_type)
                                        {
                                            tabular.last_debug_plan = Some(plan_txt);
                                        }
                                        let (h, m) = crate::query_ast::cache_stats();
                                        tabular.last_cache_hits = h;
                                        tabular.last_cache_misses = m;
                                        vec![new_sql]
                                    }
                                    Err(_e) => statements.iter().map(|s| s.to_string()).collect(),
                                }
                            } else {
                                statements.iter().map(|s| s.to_string()).collect()
                            }
                        };
                        #[cfg(not(feature = "query_ast"))]
                        let statements: Vec<String> = statements.iter().map(|s| s.to_string()).collect();
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        #[cfg(not(feature = "query_ast"))]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();
                        let (replication_status_mode, master_status_mode) = {
                            if let Some(active_tab) = tabular.query_tabs.get(tabular.active_tab_index) {
                                match active_tab.dba_special_mode {
                                    Some(models::enums::DBASpecialMode::ReplicationStatus) => (true, false),
                                    Some(models::enums::DBASpecialMode::MasterStatus) => (false, true),
                                    _ => (false, false),
                                }
                            } else {
                                (false, false)
                            }
                        };

                        let mut attempts = 0;
                        let max_attempts = 3;
                        while attempts < max_attempts {
                            attempts += 1;
                            let mut execution_success = true;
                            let mut error_message = String::new();
                            let encoded_username = modules::url_encode(&connection.username);
                            let encoded_password = modules::url_encode(&connection.password);
                            let dsn = format!(
                                "mysql://{}:{}@{}:{}/{}",
                                encoded_username,
                                encoded_password,
                                target_host,
                                target_port,
                                connection.database
                            );
                            let mut conn = match MySqlConnection::connect(&dsn).await {
                                Ok(c) => c,
                                Err(e) => {
                                    error_message = e.to_string();
                                    debug!("Failed to open MySQL connection: {}", error_message);
                                    if attempts >= max_attempts {
                                        break;
                                    } else {
                                        continue;
                                    }
                                }
                            };
                            let _ = sqlx::query("SET SESSION wait_timeout = 600").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION interactive_timeout = 600").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION net_read_timeout = 120").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION net_write_timeout = 120").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824").execute(&mut conn).await;
                            let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'").execute(&mut conn).await;

                            for (i, statement) in statements.iter().enumerate() {
                                let trimmed = statement.trim();
                                if trimmed.is_empty()
                                    || trimmed.starts_with("--")
                                    || trimmed.starts_with('#')
                                    || trimmed.starts_with("/*")
                                {
                                    debug!("Skipping statement {}: '{}'", i + 1, trimmed);
                                    continue;
                                }
                                debug!("Executing statement {}: '{}'", i + 1, trimmed);
                                let upper = trimmed.to_uppercase();

                                if upper.starts_with("USE ") {
                                    let db_part = trimmed[3..].trim();
                                    let db_name = db_part
                                        .trim_matches('`')
                                        .trim_matches('"')
                                        .trim_matches('[')
                                        .trim_matches(']')
                                        .trim();

                                    match sqlx::query(&format!("USE `{}`", db_name))
                                        .execute(&mut conn)
                                        .await
                                    {
                                        Ok(_) => {
                                            debug!("✅ Switched MySQL database using USE to '{}'.", db_name);
                                        }
                                        Err(_) => {
                                            debug!("⚠️ USE statement failed, falling back to reconnection...");
                                            let new_dsn = format!(
                                                "mysql://{}:{}@{}:{}/{}",
                                                encoded_username,
                                                encoded_password,
                                                target_host,
                                                target_port,
                                                db_name
                                            );
                                            match MySqlConnection::connect(&new_dsn).await {
                                                Ok(new_conn) => {
                                                    let mut new_conn = new_conn;
                                                    let _ = sqlx::query("SET SESSION wait_timeout = 600").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION interactive_timeout = 600").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION net_read_timeout = 120").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION net_write_timeout = 120").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824").execute(&mut new_conn).await;
                                                    let _ = sqlx::query("SET SESSION sql_mode = 'TRADITIONAL'").execute(&mut new_conn).await;
                                                    conn = new_conn;
                                                }
                                                Err(e) => {
                                                    error_message =
                                                        format!("USE failed (reconnect): {}", e);
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    continue;
                                }

                                let is_admin_command = {
                                    let cmd_upper = trimmed.to_uppercase();
                                    cmd_upper.starts_with("PURGE BINARY LOGS")
                                        || cmd_upper.starts_with("PURGE MASTER LOGS")
                                        || cmd_upper.starts_with("RESET MASTER")
                                        || cmd_upper.starts_with("RESET SLAVE")
                                        || cmd_upper.starts_with("RESET REPLICA")
                                        || cmd_upper.starts_with("CHANGE MASTER")
                                        || cmd_upper.starts_with("CHANGE REPLICATION SOURCE")
                                        || cmd_upper.starts_with("FLUSH")
                                };

                                let query_result = tokio::time::timeout(
                                    std::time::Duration::from_secs(60),
                                    sqlx::query(trimmed).fetch_all(&mut conn),
                                )
                                .await;

                                let handle_admin_error =
                                    |e: sqlx::Error| -> Result<Vec<sqlx::mysql::MySqlRow>, sqlx::Error> {
                                        let err_str = e.to_string();
                                        if err_str.contains("1295")
                                            || err_str.contains("prepared statement protocol")
                                        {
                                            debug!("Admin command executed via sqlx (1295 expected)");
                                            Ok(vec![])
                                        } else {
                                            Err(e)
                                        }
                                    };

                                let query_result = query_result.map(|result| {
                                    result.or_else(|e| {
                                        if is_admin_command {
                                            handle_admin_error(e)
                                        } else {
                                            Err(e)
                                        }
                                    })
                                });

                                match query_result {
                                    Ok(Ok(rows)) => {
                                        debug!("Query executed successfully: {} rows", rows.len());
                                        if i == statements.len() - 1 {
                                            if !rows.is_empty() {
                                                final_headers = rows[0]
                                                    .columns()
                                                    .iter()
                                                    .map(|c| c.name().to_string())
                                                    .collect();
                                                final_data = driver_mysql::convert_mysql_rows_to_table_data(rows);
                                                if replication_status_mode || master_status_mode {
                                                    let version_str = match sqlx::query("SELECT VERSION() AS v").fetch_one(&mut conn).await {
                                                        Ok(vrow) => vrow.try_get::<String, _>("v").unwrap_or_default(),
                                                        Err(_) => String::new(),
                                                    };
                                                    let is_mariadb = version_str.to_lowercase().contains("mariadb");
                                                    if replication_status_mode
                                                        && final_data.is_empty()
                                                        && let Ok(fallback_rows) = sqlx::query("SHOW SLAVE STATUS").fetch_all(&mut conn).await
                                                        && !fallback_rows.is_empty()
                                                    {
                                                        final_headers = fallback_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                        final_data = driver_mysql::convert_mysql_rows_to_table_data(fallback_rows);
                                                    }
                                                    if !final_headers.is_empty() && !final_data.is_empty() {
                                                        let header_index = |name: &str| final_headers.iter().position(|h| h.eq_ignore_ascii_case(name));
                                                        let mut summary: Vec<(String, String)> = Vec::new();
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
                                                            let mut summary_table: Vec<Vec<String>> = summary.into_iter().map(|(m, v)| vec![m, v]).collect();
                                                            summary_table.push(vec!["Server Version".into(), version_str.clone()]);
                                                            summary_table.push(vec!["Engine".into(), if is_mariadb { "MariaDB".into() } else { "MySQL".into() }]);
                                                            final_headers = vec!["Metric".into(), "Value".into()];
                                                            final_data = summary_table;
                                                        }
                                                    }
                                                }
                                            } else if is_admin_command {
                                                    debug!("Admin command executed successfully");
                                                    final_headers = vec!["Status".to_string()];
                                                    final_data = vec![vec!["Command executed successfully".to_string()]];
                                            } else {
                                                    #[cfg(feature = "query_ast")]
                                                    if final_headers.is_empty()
                                                        && let Some(hh) = _inferred_headers_from_ast.clone()
                                                        && !hh.is_empty()
                                                    {
                                                        final_headers = hh;
                                                    }
                                                    if trimmed.to_uppercase().starts_with("SELECT") {
                                                        let inferred = infer_select_headers(trimmed);
                                                        if !inferred.is_empty() {
                                                            final_headers = inferred;
                                                        }
                                                    }
                                                    if trimmed.to_uppercase().contains("FROM") {
                                                        let words: Vec<&str> = trimmed.split_whitespace().collect();
                                                        if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM")
                                                            && let Some(table_name) = words.get(from_idx + 1)
                                                        {
                                                            let describe_query = format!("DESCRIBE {}", table_name);
                                                            match tokio::time::timeout(
                                                                std::time::Duration::from_secs(30),
                                                                sqlx::query(&describe_query).fetch_all(&mut conn),
                                                            ).await {
                                                                Ok(Ok(desc_rows)) => {
                                                                    if !desc_rows.is_empty() {
                                                                        final_headers = desc_rows.iter().map(|row| {
                                                                            row.try_get::<String, _>(0).unwrap_or_else(|_| "Field".to_string())
                                                                        }).collect();
                                                                    }
                                                                }
                                                                _ => {
                                                                    let info_query = format!("{} LIMIT 0", trimmed);
                                                                    match tokio::time::timeout(
                                                                        std::time::Duration::from_secs(30),
                                                                        sqlx::query(&info_query).fetch_all(&mut conn),
                                                                    ).await {
                                                                        Ok(Ok(info_rows)) => {
                                                                            if !info_rows.is_empty() {
                                                                                final_headers = info_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                            }
                                                                        }
                                                                        _ => { final_headers = Vec::new(); }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    } else {
                                                        final_headers = Vec::new();
                                                    }
                                                    final_data = Vec::new();
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
                                        vec![vec![format!("Query error: {}", error_message)]],
                                    ));
                                }
                            }
                        }

                        Some((
                            vec!["Error".to_string()],
                            vec![vec!["Failed to execute query after multiple attempts".to_string()]],
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
                        let statements: Vec<String> = {
                            let allow_ast_rewrite = statements.len() == 1
                                && statements[0].to_uppercase().starts_with("SELECT")
                                && is_simple_select_statement(statements[0]);

                            if allow_ast_rewrite {
                                let should_paginate = tabular.use_server_pagination
                                    && !query_contains_pagination(statements[0]);
                                let pagination_opt = if should_paginate {
                                    Some((tabular.current_page as u64, tabular.page_size as u64))
                                } else {
                                    None
                                };
                                let inject_auto_limit = should_paginate;
                                match crate::query_ast::compile_single_select(
                                    statements[0],
                                    &connection.connection_type,
                                    pagination_opt,
                                    inject_auto_limit,
                                ) {
                                    Ok((new_sql, hdrs)) => {
                                        if !hdrs.is_empty() {
                                            _inferred_headers_from_ast = Some(hdrs.clone());
                                        }
                                        tabular.last_compiled_sql = Some(new_sql.clone());
                                        tabular.last_compiled_headers = hdrs.clone();
                                        if let Ok(plan_txt) = crate::query_ast::debug_plan(statements[0], &connection.connection_type) {
                                            tabular.last_debug_plan = Some(plan_txt);
                                        }
                                        let (h, m) = crate::query_ast::cache_stats();
                                        tabular.last_cache_hits = h;
                                        tabular.last_cache_misses = m;
                                        vec![new_sql]
                                    }
                                    Err(_) => statements.iter().map(|s| s.to_string()).collect(),
                                }
                            } else {
                                statements.iter().map(|s| s.to_string()).collect()
                            }
                        };
                        #[cfg(not(feature = "query_ast"))]
                        let statements: Vec<String> = statements.iter().map(|s| s.to_string()).collect();
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        #[cfg(not(feature = "query_ast"))]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

                        for (i, statement) in statements.iter().enumerate() {
                            let trimmed = statement.trim();
                            if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") {
                                continue;
                            }
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                sqlx::query(trimmed).fetch_all(pg_pool.as_ref()),
                            )
                            .await
                            {
                                Ok(Ok(rows)) => {
                                    if i == statements.len() - 1 {
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
                                            #[cfg(feature = "query_ast")]
                                            if final_headers.is_empty()
                                                && let Some(hh) = _inferred_headers_from_ast.clone()
                                                && !hh.is_empty()
                                            {
                                                final_headers = hh;
                                            }
                                            if statement.to_uppercase().starts_with("SELECT") {
                                                let inferred = infer_select_headers(statement);
                                                if !inferred.is_empty() { final_headers = inferred; }
                                            }
                                            if statement.to_uppercase().contains("FROM") {
                                                let words: Vec<&str> = statement.split_whitespace().collect();
                                                if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM")
                                                    && let Some(table_name) = words.get(from_idx + 1)
                                                {
                                                    let clean_table = table_name.trim_matches('"').trim_matches('`');
                                                    let info_query = format!(
                                                        "SELECT column_name FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                                                        clean_table
                                                    );
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        sqlx::query(&info_query).fetch_all(pg_pool.as_ref()),
                                                    ).await {
                                                        Ok(Ok(info_rows)) => {
                                                            final_headers = info_rows.iter().map(|row| {
                                                                match row.try_get::<String, _>(0) {
                                                                    Ok(col_name) => col_name,
                                                                    Err(_) => "Column".to_string(),
                                                                }
                                                            }).collect();
                                                        }
                                                        _ => {
                                                            let limit_query = format!("{} LIMIT 0", statement);
                                                            match tokio::time::timeout(
                                                                std::time::Duration::from_secs(10),
                                                                sqlx::query(&limit_query).fetch_all(pg_pool.as_ref()),
                                                            ).await {
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
                                                final_headers = Vec::new();
                                            }
                                            final_data = Vec::new();
                                        }
                                    }
                                }
                                _ => {
                                    return Some((
                                        vec!["Error".to_string()],
                                        vec![vec!["Query timed out or failed".to_string()]],
                                    ));
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
                        let statements: Vec<String> = {
                            let allow_ast_rewrite = statements.len() == 1
                                && statements[0].to_uppercase().starts_with("SELECT")
                                && is_simple_select_statement(statements[0]);

                            if allow_ast_rewrite {
                                let should_paginate = tabular.use_server_pagination
                                    && !query_contains_pagination(statements[0]);
                                let pagination_opt = if should_paginate {
                                    Some((tabular.current_page as u64, tabular.page_size as u64))
                                } else {
                                    None
                                };
                                let inject_auto_limit = should_paginate;
                                match crate::query_ast::compile_single_select(
                                    statements[0],
                                    &connection.connection_type,
                                    pagination_opt,
                                    inject_auto_limit,
                                ) {
                                    Ok((new_sql, hdrs)) => {
                                        if !hdrs.is_empty() {
                                            _inferred_headers_from_ast = Some(hdrs.clone());
                                        }
                                        vec![new_sql]
                                    }
                                    Err(_) => statements.iter().map(|s| s.to_string()).collect(),
                                }
                            } else {
                                statements.iter().map(|s| s.to_string()).collect()
                            }
                        };
                        #[cfg(not(feature = "query_ast"))]
                        let statements: Vec<String> = statements.iter().map(|s| s.to_string()).collect();
                        #[cfg(feature = "query_ast")]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        #[cfg(not(feature = "query_ast"))]
                        let statements: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

                        for (i, statement) in statements.iter().enumerate() {
                            let trimmed = statement.trim();
                            if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") {
                                continue;
                            }
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                sqlx::query(trimmed).fetch_all(sqlite_pool.as_ref()),
                            )
                            .await
                            {
                                Ok(Ok(rows)) => {
                                    if i == statements.len() - 1 {
                                        if !rows.is_empty() {
                                            final_headers = rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                            final_data = driver_sqlite::convert_sqlite_rows_to_table_data(rows);
                                        } else {
                                            #[cfg(feature = "query_ast")]
                                            if final_headers.is_empty()
                                                && let Some(hh) = _inferred_headers_from_ast.clone()
                                                && !hh.is_empty()
                                            {
                                                final_headers = hh;
                                            }
                                            if statement.to_uppercase().starts_with("SELECT") {
                                                let inferred = infer_select_headers(statement);
                                                if !inferred.is_empty() { final_headers = inferred; }
                                            }
                                            if statement.to_uppercase().contains("FROM") {
                                                let words: Vec<&str> = statement.split_whitespace().collect();
                                                if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM")
                                                    && let Some(table_name) = words.get(from_idx + 1)
                                                {
                                                    let clean_table = table_name.trim_matches('"').trim_matches('`').trim_matches('[').trim_matches(']');
                                                    let pragma_query = format!("PRAGMA table_info({})", clean_table);
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        sqlx::query(&pragma_query).fetch_all(sqlite_pool.as_ref()),
                                                    ).await {
                                                        Ok(Ok(pragma_rows)) => {
                                                            final_headers = pragma_rows.iter().map(|row| {
                                                                match row.try_get::<String, _>(1) {
                                                                    Ok(col_name) => col_name,
                                                                    Err(_) => "Column".to_string(),
                                                                }
                                                            }).collect();
                                                        }
                                                        _ => {
                                                            let limit_query = format!("{} LIMIT 0", statement);
                                                            match tokio::time::timeout(
                                                                std::time::Duration::from_secs(10),
                                                                sqlx::query(&limit_query).fetch_all(sqlite_pool.as_ref()),
                                                            ).await {
                                                                Ok(Ok(limit_rows)) => {
                                                                    if !limit_rows.is_empty() {
                                                                        final_headers = limit_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                    }
                                                                }
                                                                _ => { final_headers = Vec::new(); }
                                                            }
                                                        }
                                                    }
                                                }
                                            } else {
                                                final_headers = Vec::new();
                                            }
                                            final_data = Vec::new();
                                        }
                                    }
                                }
                                _ => {
                                    return Some((
                                        vec!["Error".to_string()],
                                        vec![vec!["Query timed out or failed".to_string()]],
                                    ));
                                }
                            }
                        }
                        Some((final_headers, final_data))
                    }
                    models::enums::DatabasePool::Redis(redis_manager) => {
                        debug!("Executing Redis command: {}", query);
                        let mut conn = redis_manager.as_ref().clone();
                        use redis::AsyncCommands;

                        let parts: Vec<&str> = query.split_whitespace().collect();
                        if parts.is_empty() {
                            return Some((
                                vec!["Error".to_string()],
                                vec![vec!["Empty command".to_string()]],
                            ));
                        }

                        match parts[0].to_uppercase().as_str() {
                            "GET" => {
                                if parts.len() != 2 {
                                    return Some((vec!["Error".to_string()], vec![vec!["GET requires exactly one key".to_string()]]));
                                }
                                match tokio::time::timeout(std::time::Duration::from_secs(10), conn.get::<&str, Option<String>>(parts[1])).await {
                                    Ok(Ok(Some(value))) => Some((vec!["Key".to_string(), "Value".to_string()], vec![vec![parts[1].to_string(), value]])),
                                    Ok(Ok(None)) => Some((vec!["Key".to_string(), "Value".to_string()], vec![vec![parts[1].to_string(), "NULL".to_string()]])),
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis GET timed out or failed".to_string()]])),
                                }
                            }
                            "KEYS" => {
                                if parts.len() != 2 {
                                    return Some((vec!["Error".to_string()], vec![vec!["KEYS requires exactly one pattern".to_string()]]));
                                }
                                match tokio::time::timeout(std::time::Duration::from_secs(10), conn.keys::<&str, Vec<String>>(parts[1])).await {
                                    Ok(Ok(keys)) => Some((vec!["Key".to_string()], keys.into_iter().map(|k| vec![k]).collect())),
                                    _ => Some((vec!["Error".to_string()], vec![vec!["Redis KEYS timed out or failed".to_string()]])),
                                }
                            }
                            "INFO" => {
                                let section = if parts.len() > 1 { parts[1] } else { "default" };
                                match tokio::time::timeout(std::time::Duration::from_secs(10), redis::cmd("INFO").arg(section).query_async::<String>(&mut conn)).await {
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
                                match tokio::time::timeout(std::time::Duration::from_secs(10), redis::cmd("HGETALL").arg(parts[1]).query_async::<Vec<String>>(&mut conn)).await {
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
                            Err(e) => Some((
                                vec!["Error".to_string()],
                                vec![vec![format!("Query error: {}", e)]],
                            )),
                        }
                    }
                    models::enums::DatabasePool::MongoDB(_client) => Some((
                        vec!["Info".to_string()],
                        vec![vec!["MongoDB query execution is not supported. Use tree to browse collections.".to_string()]],
                    )),
                }
            }
            None => {
                debug!(
                    "Failed to get connection pool for connection_id: {}",
                    connection_id
                );
                Some((
                    vec!["Error".to_string()],
                    vec![vec!["Failed to connect to database".to_string()]],
                ))
            }
        }
    })
}

/// Execute multiple queries concurrently (non-blocking for slow connections).
#[allow(dead_code)]
pub(crate) async fn execute_multiple_queries_concurrently(
    tabular: &mut Tabular,
    query_requests: Vec<(i64, String)>,
) -> Vec<Option<(Vec<String>, Vec<Vec<String>>)>> {
    let mut results = Vec::new();

    for (connection_id, query) in query_requests {
        match try_get_connection_pool(tabular, connection_id).await {
            Some(_pool) => {
                if let Some(connection) = tabular
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                    .cloned()
                {
                    let result =
                        execute_table_query_sync(tabular, connection_id, &connection, &query);
                    results.push(result);
                } else {
                    results.push(None);
                }
            }
            None => {
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
