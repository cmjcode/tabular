use crate::{
    connection, driver_mssql, driver_mysql, driver_postgres, driver_redis, driver_sqlite,
    models, modules,
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

// Type alias for complex index map structure
type IndexMap = std::collections::BTreeMap<String, (Option<String>, bool, Vec<(i64, String)>)>;

// Limit concurrent prefetch tasks to avoid overwhelming servers and local machine
const PREFETCH_CONCURRENCY: usize = 6;

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
    let upper_query = trimmed_query.to_uppercase();
    if upper_query.contains(" LIMIT ")
        || upper_query.contains(" OFFSET ")
        || upper_query.contains(" FETCH ")
        || upper_query.contains(" TOP ")
    {
        return trimmed_query.to_string();
    }

    // Only operate on simple SELECT queries
    if !upper_query.starts_with("SELECT ") {
        return trimmed_query.to_string();
    }

    match db_type {
        models::enums::DatabaseType::MsSQL => {
            // Insert TOP 1000 after SELECT if no TOP present
            if upper_query.starts_with("SELECT ") {
                // Preserve casing after the SELECT keyword
                if let Some(rest) = trimmed_query.get(6..) {
                    return format!("SELECT TOP 1000{}", rest);
                }
            }
            trimmed_query.to_string()
        }
        _ => {
            // MySQL/PostgreSQL/SQLite/MongoDB/Redis: append LIMIT 1000
            format!("{} LIMIT 1000", trimmed_query)
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
            let has_pagination_clause = upper.contains(" LIMIT")
                || upper.contains(" OFFSET")
                || upper.contains(" FETCH ")
                || upper.contains(" TOP ")
                || upper.contains("TOP(");
            let has_select_stmt = upper.split(';').any(|s| s.trim_start().starts_with("SELECT"));

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
                        final_query = format!("{} LIMIT {} OFFSET {}", base, tabular.page_size, offset);
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

                        // Split into statements
                        let statements: Vec<&str> = query
                            .split(';')
                            .map(|s| s.trim())
                            .filter(|s| !s.is_empty())
                            .collect();
                        debug!("Found {} SQL statements to execute", statements.len());
                        for (idx, stmt) in statements.iter().enumerate() {
                            debug!("Statement {}: '{}'", idx + 1, stmt);
                        }

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

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
                                encoded_username, encoded_password, connection.host, connection.port, connection.database
                            );
                            let mut conn = match MySqlConnection::connect(&dsn).await {
                                Ok(c) => c,
                                Err(e) => {
                                    error_message = e.to_string();
                                    debug!("Failed to open MySQL connection: {}", error_message);
                                    if attempts >= max_attempts { break; } else { continue; }
                                }
                            };

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
                                                encoded_username, encoded_password, connection.host, connection.port, db_name
                                            );
                                            match MySqlConnection::connect(&new_dsn).await {
                                                Ok(new_conn) => {
                                                    debug!("🔄 Switched MySQL database by reconnecting to '{}'.", db_name);
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

                                match tokio::time::timeout(
                                    std::time::Duration::from_secs(5),
                                    sqlx::query(trimmed).fetch_all(&mut conn),
                                )
                                .await
                                {
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
                                            } else {
                                                // Zero rows: try to infer headers from SELECT list first
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
                                                            std::time::Duration::from_secs(5),
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
                                                                    std::time::Duration::from_secs(5),
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
                                    _ => {
                                        error_message = "Query timed out or failed".to_string();
                                        execution_success = false;
                                        break;
                                    }
                                }
                            }

                            if execution_success {
                                return Some((final_headers, final_data));
                            } else {
                                debug!("MySQL query failed on attempt {}: {}", attempts, error_message);
                                if (error_message.contains("timed out") || error_message.contains("pool")) && attempts < max_attempts {
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
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

                        for (i, statement) in statements.iter().enumerate() {
                            let trimmed = statement.trim();
                            // Skip empty or comment-only statements
                            if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") { continue; }
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(5),
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
                                            // Zero rows: infer headers from SELECT list
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
                                                        std::time::Duration::from_secs(5),
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
                                                                std::time::Duration::from_secs(5),
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
                        debug!("Found {} SQL statements to execute", statements.len());

                        let mut final_headers = Vec::new();
                        let mut final_data = Vec::new();

                        for (i, statement) in statements.iter().enumerate() {
                            let trimmed = statement.trim();
                            // Skip empty or comment-only statements
                            if trimmed.is_empty() || trimmed.starts_with("--") || trimmed.starts_with("/*") { continue; }
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(5),
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
                                            // Zero rows: infer headers from SELECT list
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
                                                        std::time::Duration::from_secs(5),
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
                                                                std::time::Duration::from_secs(5),
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
                                    std::time::Duration::from_secs(5),
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
                                    std::time::Duration::from_secs(5),
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
                                    std::time::Duration::from_secs(5),
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
                                    std::time::Duration::from_secs(5),
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
                                    std::time::Duration::from_secs(5),
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

// Render a connection selector or related UI elements if needed by the caller.
// Currently a no-op placeholder to maintain compatibility with callers in window_egui.
// In the future, this can host quick-pick or status UI for connections.
pub(crate) fn render_connection_selector(_tabular: &mut Tabular, _ctx: &egui::Context) {
    // Intentionally left blank.
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
        debug!(
            "⏳ Connection pool creation already in progress for connection {}",
            connection_id
        );
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

    // Quick attempt with very short timeout
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
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
            let encoded_username = modules::url_encode(&connection.username);
            let encoded_password = modules::url_encode(&connection.password);
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                encoded_username,
                encoded_password,
                connection.host,
                connection.port,
                connection.database
            );

            // Don't block on ICMP ping (often disabled on Windows firewalls). Attempt direct connect.
            // If you still want diagnostics, you can log ping result without failing the flow:
            // let _ = helpers::ping_host(&connection.host);

            // Configure MySQL pool with improved settings for stability
            let pool_result = MySqlPoolOptions::new()
                .max_connections(10) // Reduced from 20 for better resource management
                .min_connections(2) // Maintain some ready connections
                .acquire_timeout(std::time::Duration::from_secs(5)) // Fail fast
                .idle_timeout(std::time::Duration::from_secs(600)) // 10 minute idle timeout (longer)
                .max_lifetime(std::time::Duration::from_secs(3600)) // 60 minute max lifetime (longer)
                .test_before_acquire(true) // Enable connection testing for reliability
                .after_connect(|conn, _| {
                    Box::pin(async move {
                        // Set connection-specific settings for better stability and performance
                        let _ = sqlx::query("SET SESSION wait_timeout = 600")
                            .execute(&mut *conn)
                            .await;
                        let _ = sqlx::query("SET SESSION interactive_timeout = 600")
                            .execute(&mut *conn)
                            .await;
                        let _ = sqlx::query("SET SESSION net_read_timeout = 60")
                            .execute(&mut *conn)
                            .await;
                        let _ = sqlx::query("SET SESSION net_write_timeout = 60")
                            .execute(&mut *conn)
                            .await;
                        // Optimize for performance
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
                    let database_pool = models::enums::DatabasePool::MySQL(Arc::new(pool));
                    debug!(
                        "✅ Created MySQL connection pool for connection {:?}",
                        connection.id
                    );
                    Some(database_pool)
                }
                Err(e) => {
                    debug!(
                        "❌ Failed to create MySQL pool for connection {:?}: {}",
                        connection.id, e
                    );
                    None
                }
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                connection.host,
                connection.port,
                connection.database
            );

            // Configure PostgreSQL pool with improved settings
            let pool_result = PgPoolOptions::new()
                .max_connections(15) // Increase max connections
                .min_connections(1) // Start with fewer minimum connections
                .acquire_timeout(std::time::Duration::from_secs(5)) // Fail fast
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
                .acquire_timeout(std::time::Duration::from_secs(5)) // Fail fast
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
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", connection.host, connection.port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, connection.host, connection.port
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
            // Build MongoDB connection string
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", connection.host, connection.port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, connection.host, connection.port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, connection.host, connection.port
                )
            };
            debug!("Creating MongoDB client for URI: {}", uri);
            match tokio::time::timeout(std::time::Duration::from_secs(5), MongoClient::with_uri_str(uri)).await {
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
            let cfg = driver_mssql::MssqlConfigWrapper::new(
                connection.host.clone(),
                connection.port.clone(),
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
}

#[allow(dead_code)]
pub(crate) async fn refresh_connection_background_async(
    connection_id: i64,
    db_pool: &Option<Arc<SqlitePool>>,
) -> bool {
    debug!("Refreshing connection with ID: {}", connection_id);

    // Get connection from database
    if let Some(cache_pool_arc) = db_pool {
        let connection_result = sqlx::query_as::<_, (i64, String, String, String, String, String, String, String)>(
            "SELECT id, name, host, port, username, password, database_name, connection_type FROM connections WHERE id = ?"
        )
            .bind(connection_id)
            .fetch_optional(cache_pool_arc.as_ref())
            .await;

        if let Ok(Some((
            id,
            name,
            host,
            port,
            username,
            password,
            database_name,
            connection_type,
        ))) = connection_result
        {
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
                    _ => models::enums::DatabaseType::SQLite,
                },
                folder: None, // Will be loaded from database later
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
                    let ok = fetch_and_cache_all_data(
                        connection_id,
                        &connection,
                        &new_pool,
                        cache_pool_arc.as_ref(),
                    )
                    .await;
                    if ok {
                        // Prefetch index metadata for tables for instant Indexes view
                        let _ = prefetch_indexes_for_all_tables(
                            connection_id,
                            &connection,
                            &new_pool,
                            cache_pool_arc.as_ref(),
                        )
                        .await;
                        // Best-effort: prefetch first 100 rows for all tables so browsing feels instant
                        let _ = prefetch_first_rows_for_all_tables(
                            connection_id,
                            &connection,
                            &new_pool,
                            cache_pool_arc.as_ref(),
                        )
                        .await;
                    }
                    ok
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
            let encoded_username = modules::url_encode(&connection.username);
            let encoded_password = modules::url_encode(&connection.password);
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                encoded_username,
                encoded_password,
                connection.host,
                connection.port,
                connection.database
            );

            // Configure MySQL pool with optimized settings for large queries
            let pool_result = MySqlPoolOptions::new()
                .max_connections(3) // Reduced from 5 to 3 - fewer but more stable connections
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(5)) // Faster timeout to fail fast
                .idle_timeout(std::time::Duration::from_secs(600)) // 10 minutes idle timeout
                .max_lifetime(std::time::Duration::from_secs(3600)) // 1 hour max lifetime
                .test_before_acquire(true) // Test connections before use
                .after_connect(|conn, _| {
                    Box::pin(async move {
                        // Optimize MySQL settings for performance with large datasets
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
                        // Increase max packet size for large result sets
                        let _ = sqlx::query("SET SESSION max_allowed_packet = 1073741824")
                            .execute(&mut *conn)
                            .await; // 1GB
                        // Optimize query cache and buffer settings
                        let _ = sqlx::query("SET SESSION query_cache_type = ON")
                            .execute(&mut *conn)
                            .await;
                        let _ = sqlx::query("SET SESSION read_buffer_size = 2097152")
                            .execute(&mut *conn)
                            .await; // 2MB
                        Ok(())
                    })
                })
                .connect(&connection_string)
                .await;

            match pool_result {
                Ok(pool) => Some(models::enums::DatabasePool::MySQL(Arc::new(pool))),
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::PostgreSQL => {
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                connection.host,
                connection.port,
                connection.database
            );

            match PgPoolOptions::new()
                .max_connections(3)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(5))
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
                .acquire_timeout(std::time::Duration::from_secs(5))
                .idle_timeout(std::time::Duration::from_secs(300))
                .connect(&connection_string)
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::SQLite(Arc::new(pool))),
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::Redis => {
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", connection.host, connection.port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, connection.host, connection.port
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
            let cfg = driver_mssql::MssqlConfigWrapper::new(
                connection.host.clone(),
                connection.port.clone(),
                connection.database.clone(),
                connection.username.clone(),
                connection.password.clone(),
            );
            Some(models::enums::DatabasePool::MsSQL(Arc::new(cfg)))
        }
        models::enums::DatabaseType::MongoDB => {
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", connection.host, connection.port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, connection.host, connection.port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, connection.host, connection.port
                )
            };
            match tokio::time::timeout(std::time::Duration::from_secs(5), MongoClient::with_uri_str(uri)).await {
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

// Helper: save index metadata directly to index_cache
async fn save_indexes_cache_direct(
    cache_pool: &SqlitePool,
    connection_id: i64,
    database_name: &str,
    table_name: &str,
    indexes: &[models::structs::IndexStructInfo],
){
    // Clear existing index rows for this table
    let _ = sqlx::query(
        "DELETE FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?",
    )
    .bind(connection_id)
    .bind(database_name)
    .bind(table_name)
    .execute(cache_pool)
    .await;

    for idx in indexes {
        let cols_json = serde_json::to_string(&idx.columns).unwrap_or_else(|_| "[]".to_string());
        let _ = sqlx::query(
            r#"INSERT OR REPLACE INTO index_cache
                (connection_id, database_name, table_name, index_name, method, is_unique, columns_json)
                VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(connection_id)
        .bind(database_name)
        .bind(table_name)
        .bind(&idx.name)
        .bind(&idx.method)
        .bind(if idx.unique { 1 } else { 0 })
        .bind(cols_json)
        .execute(cache_pool)
        .await;
    }
}

// After metadata is cached, fetch index metadata for all tables and store in index_cache
async fn prefetch_indexes_for_all_tables(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    pool: &models::enums::DatabasePool,
    cache_pool: &SqlitePool,
) -> bool {
    use sqlx::Row;
    // Load distinct (db, table) pairs for tables only
    let tables_res = sqlx::query_as::<_, (String, String)>(
        "SELECT database_name, table_name FROM table_cache WHERE connection_id = ? AND table_type = 'table' ORDER BY database_name, table_name",
    )
    .bind(connection_id)
    .fetch_all(cache_pool)
    .await;

    let pairs = match tables_res { Ok(v) => v, Err(_) => return false };

    match pool {
        models::enums::DatabasePool::MySQL(_mysql_pool) => {
            let enc_user = crate::modules::url_encode(&connection.username);
            let enc_pass = crate::modules::url_encode(&connection.password);
            futures_util::stream::iter(pairs)
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
                            let q = r#"SELECT INDEX_NAME, COLUMN_NAME, SEQ_IN_INDEX, NON_UNIQUE, INDEX_TYPE FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX"#;
                            if let Ok(rows) = sqlx::query(q).bind(&dbn).bind(&tbn).fetch_all(&mut conn).await {
                                let mut map: IndexMap = IndexMap::new();
                                for r in rows {
                                    let name: String = r.try_get("INDEX_NAME").unwrap_or_default();
                                    let col: Option<String> = r.try_get("COLUMN_NAME").ok();
                                    let seq: i64 = r.try_get("SEQ_IN_INDEX").unwrap_or(0);
                                    let non_unique: i64 = r.try_get("NON_UNIQUE").unwrap_or(1);
                                    let idx_type: Option<String> = r.try_get("INDEX_TYPE").ok();
                                    let entry = map.entry(name).or_insert((None, non_unique == 0, Vec::new()));
                                    if entry.0.is_none() { entry.0 = idx_type.clone(); }
                                    if let Some(cn) = col { entry.2.push((seq, cn)); }
                                    entry.1 = non_unique == 0;
                                }
                                let mut list = Vec::new();
                                for (name, (method, unique, mut cols)) in map {
                                    cols.sort_by_key(|(seq, _)| *seq);
                                    let columns: Vec<String> = cols.into_iter().map(|(_, c)| c).collect();
                                    list.push(models::structs::IndexStructInfo { name, method, unique, columns });
                                }
                                save_indexes_cache_direct(cache_pool, connection_id, &dbn, &tbn, &list).await;
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
            let curr_db: Option<String> = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(pg_pool.as_ref())
                .await
                .ok();
            let filtered: Vec<(String, String)> = match curr_db {
                Some(db) => pairs.into_iter().filter(|(d, _)| *d == db).collect(),
                None => pairs,
            };
            futures_util::stream::iter(filtered)
                .map(|(dbn, tbn)| {
                    let pool = pg_pool.clone();
                    async move {
                        let q = r#"SELECT idx.relname AS index_name, pg_get_indexdef(i.indexrelid) AS index_def, i.indisunique AS is_unique
                                   FROM pg_class t
                                   JOIN pg_index i ON t.oid = i.indrelid
                                   JOIN pg_class idx ON idx.oid = i.indexrelid
                                   JOIN pg_namespace n ON n.oid = t.relnamespace
                                   WHERE t.relname = $1 AND n.nspname='public' ORDER BY idx.relname"#;
                        if let Ok(rows) = sqlx::query(q).bind(&tbn).fetch_all(pool.as_ref()).await {
                            let mut list = Vec::new();
                            for r in rows {
                                let name: String = r.try_get("index_name").unwrap_or_default();
                                let def: String = r.try_get("index_def").unwrap_or_default();
                                let unique: bool = r.try_get("is_unique").unwrap_or(false);
                                let method = def.split(" USING ").nth(1).and_then(|rest| rest.split_whitespace().next()).map(|m| m.trim_matches('(').trim_matches(')').to_string());
                                let columns: Vec<String> = if let Some(start) = def.rfind('(') { if let Some(end_rel) = def[start+1..].find(')') { def[start+1..start+1+end_rel].split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect() } else { Vec::new() } } else { Vec::new() };
                                list.push(models::structs::IndexStructInfo { name, method, unique, columns });
                            }
                            save_indexes_cache_direct(cache_pool, connection_id, &dbn, &tbn, &list).await;
                        }
                    }
                })
                .buffer_unordered(PREFETCH_CONCURRENCY)
                .for_each(|_| async {})
                .await;
            true
        }
        models::enums::DatabasePool::SQLite(sqlite_pool) => {
            futures_util::stream::iter(pairs)
                .map(|(_dbn, tbn)| {
                    let pool = sqlite_pool.clone();
                    async move {
                        let q_list = format!("PRAGMA index_list(\"{}\")", tbn.replace('"', "\\\""));
                        if let Ok(list_rows) = sqlx::query(&q_list).fetch_all(pool.as_ref()).await {
                            let mut list = Vec::new();
                            for r in list_rows {
                                let name: String = r.try_get(1).unwrap_or_default();
                                let unique_i: i64 = r.try_get(2).unwrap_or(0);
                                let unique = unique_i != 0;
                                let q_cols = format!("PRAGMA index_info(\"{}\")", name.replace('"', "\\\""));
                                let mut cols: Vec<(i64, String)> = Vec::new();
                                if let Ok(col_rows) = sqlx::query(&q_cols).fetch_all(pool.as_ref()).await {
                                    for cr in col_rows {
                                        let seq: i64 = cr.try_get(0).unwrap_or(0);
                                        let cname: String = cr.try_get(2).unwrap_or_default();
                                        cols.push((seq, cname));
                                    }
                                }
                                cols.sort_by_key(|(s, _)| *s);
                                let columns: Vec<String> = cols.into_iter().map(|(_, c)| c).collect();
                                list.push(models::structs::IndexStructInfo { name, method: None, unique, columns });
                            }
                            save_indexes_cache_direct(pool.as_ref(), connection_id, "main", &tbn, &list).await;
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

// After metadata is cached, fetch first 100 rows for all tables and store in row_cache
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
                                let data = crate::driver_mysql::convert_mysql_rows_to_table_data(mysql_rows);
                                save_row_cache_direct(cache_pool, connection_id, &dbn, &tbn, &headers, &data).await;
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
                                let iq = format!("PRAGMA table_info(\"{}\")", tbn.replace('"', "\\\""));
                                match sqlx::query(&iq).fetch_all(pool.as_ref()).await {
                                    Ok(infos) => infos
                                        .iter()
                                        .filter_map(|r| r.try_get::<String, _>(1).ok())
                                        .collect(),
                                    Err(_) => Vec::new(),
                                }
                            };
                            let data = crate::driver_sqlite::convert_sqlite_rows_to_table_data(sqlite_rows);
                            save_row_cache_direct(cache_pool, connection_id, "main", &tbn, &headers, &data).await;
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

pub(crate) fn update_connection_in_database(
    tabular: &mut window_egui::Tabular,
    connection: &models::structs::ConnectionConfig,
) -> bool {
    if let Some(ref pool) = tabular.db_pool {
        if let Some(id) = connection.id {
            let pool_clone = pool.clone();
            let connection = connection.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();

            let result = rt.block_on(async {
                sqlx::query(
                    "UPDATE connections SET name = ?, host = ?, port = ?, username = ?, password = ?, database_name = ?, connection_type = ?, folder = ? WHERE id = ?"
                )
                    .bind(connection.name)
                    .bind(connection.host)
                    .bind(connection.port)
                    .bind(connection.username)
                    .bind(connection.password)
                    .bind(connection.database)
                    .bind(format!("{:?}", connection.connection_type))
                    .bind(connection.folder)
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

    // Set flag to force refresh on next update
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
                let encoded_username = modules::url_encode(&connection.username);
                let encoded_password = modules::url_encode(&connection.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username,
                    encoded_password,
                    connection.host,
                    connection.port,
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
                let connection_string = format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    connection.username,
                    connection.password,
                    connection.host,
                    connection.port,
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
                // Build URI and ping
                let uri = if connection.username.is_empty() {
                    format!("mongodb://{}:{}", connection.host, connection.port)
                } else if connection.password.is_empty() {
                    format!(
                        "mongodb://{}@{}:{}",
                        connection.username, connection.host, connection.port
                    )
                } else {
                    let enc_user = modules::url_encode(&connection.username);
                    let enc_pass = modules::url_encode(&connection.password);
                    format!(
                        "mongodb://{}:{}@{}:{}",
                        enc_user, enc_pass, connection.host, connection.port
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
                let connection_string = if connection.password.is_empty() {
                    format!("redis://{}:{}", connection.host, connection.port)
                } else {
                    format!(
                        "redis://{}:{}@{}:{}",
                        connection.username, connection.password, connection.host, connection.port
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
                let host = connection.host.clone();
                let port: u16 = connection.port.parse().unwrap_or(1433);
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
