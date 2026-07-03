// MsSQL driver module now built unconditionally (feature flag removed)
use crate::models;
use crate::window_egui; // for Tabular type

use mssql_client::{Client, Config, Credentials, Ready, SqlValue};

// MsSQL connectivity is provided by mssql-client (praxiomlabs rust-mssql-driver)
// with pooling from mssql-driver-pool. The helpers below centralize config,
// connection, and dynamic value-to-string conversion for the whole app.

/// Build a Config for a direct (non-pooled) MsSQL connection.
/// Mirrors the app-wide defaults: SQL auth + trusted server certificate.
pub(crate) fn mssql_config(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
) -> Config {
    let mut config = Config::new()
        .host(host)
        .port(port)
        .credentials(Credentials::sql_server(
            username.to_string(),
            password.to_string(),
        ))
        .connect_timeout(std::time::Duration::from_secs(10))
        .trust_server_certificate(true);
    if let Some(db) = database
        && !db.is_empty()
    {
        config = config.database(db.to_string());
    }
    config
}

/// Open a one-off MsSQL connection (no pool).
pub(crate) async fn connect_mssql(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: Option<&str>,
) -> Result<Client<Ready>, String> {
    Client::connect(mssql_config(host, port, username, password, database))
        .await
        .map_err(|e| e.to_string())
}

/// Convert a dynamic SqlValue into the display string used by the data grid.
pub(crate) fn sql_value_to_string(value: &SqlValue) -> String {
    match value {
        SqlValue::Null => "NULL".to_string(),
        SqlValue::Bool(v) => v.to_string(),
        SqlValue::TinyInt(v) => v.to_string(),
        SqlValue::SmallInt(v) => v.to_string(),
        SqlValue::Int(v) => v.to_string(),
        SqlValue::BigInt(v) => v.to_string(),
        SqlValue::Float(v) => v.to_string(),
        SqlValue::Double(v) => v.to_string(),
        SqlValue::String(s) => s.clone(),
        SqlValue::Binary(b) => format!("0x{}", hex::encode(b)),
        SqlValue::Decimal(d) | SqlValue::Money(d) | SqlValue::SmallMoney(d) => d.to_string(),
        SqlValue::Uuid(u) => u.to_string(),
        SqlValue::Date(d) => d.format("%Y-%m-%d").to_string(),
        SqlValue::Time(t) => t.format("%H:%M:%S%.f").to_string(),
        SqlValue::DateTime(dt) | SqlValue::SmallDateTime(dt) => {
            dt.format("%Y-%m-%d %H:%M:%S%.f").to_string()
        }
        SqlValue::DateTimeOffset(dto) => dto.format("%Y-%m-%d %H:%M:%S%.f %:z").to_string(),
        SqlValue::Xml(x) => x.clone(),
        // Tvp is send-only and SqlValue is #[non_exhaustive]
        other => format!("{:?}", other),
    }
}

/// Convert every column of a row into display strings.
pub(crate) fn row_values_to_strings(row: &mssql_client::Row) -> Vec<String> {
    (0..row.len())
        .map(|i| match row.get_raw(i) {
            Some(v) => sql_value_to_string(&v),
            None => "NULL".to_string(),
        })
        .collect()
}

/// Run a single-statement query through the shared pool and collect all rows.
pub(crate) async fn pooled_query(
    pool: &mssql_driver_pool::Pool,
    sql: &str,
) -> Result<Vec<mssql_client::Row>, String> {
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let client = conn
        .client_mut()
        .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
    let stream = client.query(sql, &[]).await.map_err(|e| e.to_string())?;
    stream.collect_all().await.map_err(|e| e.to_string())
}

pub(crate) async fn fetch_mssql_data(
    _connection_id: i64,
    _pool: std::sync::Arc<mssql_driver_pool::Pool>,
    _cache_pool: &sqlx::SqlitePool,
) -> bool {
    // TODO: implement metadata caching
    true
}

pub(crate) fn load_mssql_structure(
    connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Similar to other drivers: show Databases + DBA Views folders
    let mut main_children = Vec::new();

    // Databases folder with loading marker
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(connection_id);
    let loading_node = models::structs::TreeNode::new(
        "Loading databases...".to_string(),
        models::enums::NodeType::Database,
    );
    databases_folder.children.push(loading_node);
    main_children.push(databases_folder);

    // DBA Views folder with standard children
    let mut dba_folder = models::structs::TreeNode::new(
        "DBA Views".to_string(),
        models::enums::NodeType::DBAViewsFolder,
    );
    dba_folder.connection_id = Some(connection_id);

    let mut dba_children = Vec::new();

    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::MsSQL) {
        let mut dba_node = models::structs::TreeNode::new(name.to_string(), node_type);
        dba_node.connection_id = Some(connection_id);
        dba_node.is_loaded = false;
        dba_node.query = Some(query.to_string());
        dba_children.push(dba_node);
    }

    dba_folder.children = dba_children;
    main_children.push(dba_folder);

    node.children = main_children;
}

/// Fetch MsSQL tables or views for a specific database (synchronous wrapper like other drivers)
pub(crate) fn fetch_tables_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    _database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        // Get or create pool
        let pool_enum = crate::connection::get_or_create_connection_pool(tabular, connection_id).await?;
        let pool = match pool_enum {
             crate::models::enums::DatabasePool::MsSQL(p) => p,
             _ => return None,
        };
        
        // Get a connection from the pool
        let mut conn = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                log::debug!("MsSQL pool get error: {}", e);
                return None;
            }
        };
        let client = conn.client_mut()?;

        // Choose query based on type (include schema for views)
        let query = match table_type {
            // Include schema for tables (some objects not in dbo)
            "table" => "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE='BASE TABLE' ORDER BY TABLE_NAME",
            // Include schema for views so we can build fully-qualified names
            "view" => "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS ORDER BY TABLE_NAME",
            _ => {
                log::debug!("Unsupported MsSQL table_type: {}", table_type);
                return None;
            }
        };

        let stream = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.query(query, &[]),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => { log::debug!("MsSQL list query error: {}", e); return None; }
            Err(_) => { log::debug!("MsSQL list query timeout"); return None; }
        };

        let mut items = Vec::new();
        for row in stream.collect_all().await.ok()? {
            let schema = row.get_string(0);
            let name = row.get_string(1);
            if let (Some(s), Some(n)) = (schema, name) {
                items.push(format!("[{}].[{}]", s, n));
            }
        }
        Some(items)
    })
}

/// Fetch MsSQL objects for a specific database by type: procedure | function | trigger
pub(crate) fn fetch_objects_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    _database_name: &str,
    object_type: &str,
) -> Option<Vec<String>> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        // Get or create pool
        let pool_enum = crate::connection::get_or_create_connection_pool(tabular, connection_id).await?;
        let pool = match pool_enum {
             crate::models::enums::DatabasePool::MsSQL(p) => p,
             _ => return None,
        };
        
        let mut conn = match pool.get().await {
            Ok(c) => c,
            Err(e) => {
                log::debug!("MsSQL pool get error: {}", e);
                return None;
            }
        };
        let client = conn.client_mut()?;

        let query = match object_type {
            // Stored procedures
            "procedure" => {
                // sys.procedures excludes system procedures when filtered by is_ms_shipped = 0
                "SELECT s.name AS schema_name, p.name AS object_name \
                 FROM sys.procedures p \
                 JOIN sys.schemas s ON p.schema_id = s.schema_id \
                 WHERE ISNULL(p.is_ms_shipped,0) = 0 \
                 ORDER BY p.name"
                    .to_string()
            }
            // Functions: scalar, inline table-valued, multi-statement table-valued, and CLR variants
            "function" => "SELECT s.name AS schema_name, o.name AS object_name \
                 FROM sys.objects o \
                 JOIN sys.schemas s ON o.schema_id = s.schema_id \
                 WHERE o.type IN ('FN','IF','TF','AF','FS','FT') \
                 ORDER BY o.name"
                .to_string(),
            // Triggers: list DML triggers attached to user tables
            "trigger" => {
                "SELECT ss.name AS schema_name, t.name AS table_name, tr.name AS trigger_name \
                 FROM sys.triggers tr \
                 JOIN sys.tables t ON tr.parent_id = t.object_id \
                 JOIN sys.schemas ss ON t.schema_id = ss.schema_id \
                 WHERE t.type = 'U' \
                 ORDER BY tr.name"
                    .to_string()
            }
            _ => {
                log::debug!("Unsupported MsSQL object_type: {}", object_type);
                return None;
            }
        };

        let stream = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.query(&query, &[]),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                log::debug!("MsSQL object list query error: {}", e);
                return None;
            }
            Err(_) => {
                log::debug!("MsSQL object list query timeout");
                return None;
            }
        };

        let mut items = Vec::new();
        for row in stream.collect_all().await.ok()? {
            match object_type {
                "procedure" | "function" => {
                    let schema = row.get_string(0);
                    let name = row.get_string(1);
                    if let (Some(s), Some(n)) = (schema, name) {
                        items.push(format!("[{}].[{}]", s, n));
                    }
                }
                "trigger" => {
                    let schema = row.get_string(0);
                    let table = row.get_string(1);
                    let trig = row.get_string(2);
                    if let (Some(s), Some(t), Some(tr)) = (schema, table, trig) {
                        items.push(format!("[{}].[{}].[{}]", s, t, tr));
                    }
                }
                _ => {}
            }
        }
        Some(items)
    })
}

/// Execute a query using the shared connection pool (mssql-driver-pool)
pub(crate) async fn execute_query(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    query: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    // Acquire a client from the pool and delegate to the common runner
    let mut conn = pool.get().await.map_err(|e| e.to_string())?;
    let client = conn
        .client_mut()
        .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
    run_query(client, query).await
}

pub(crate) async fn run_query(
    client: &mut Client<Ready>,
    query: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut headers: Vec<String> = Vec::new();
    let mut data: Vec<Vec<String>> = Vec::new();

    // query_multiple handles batches like "USE [db]; SELECT ..." — like the
    // previous tiberius stream, headers follow the latest result set with
    // columns while rows accumulate across result sets.
    let mut stream = client
        .query_multiple(query, &[])
        .await
        .map_err(|e| e.to_string())?;

    loop {
        if let Some(cols) = stream.columns()
            && !cols.is_empty()
        {
            headers = cols.iter().map(|c| c.name.clone()).collect();
        }
        while let Some(row) = stream.next_row().await.map_err(|e| e.to_string())? {
            data.push(row_values_to_strings(&row));
        }
        if !stream.next_result().await.map_err(|e| e.to_string())? {
            break;
        }
    }
    Ok((headers, data))
}

// Helper: Remove TOP clauses from MsSQL SELECT for pagination compatibility
pub(crate) fn sanitize_mssql_select_for_pagination(select_part: &str) -> String {
    let mut result = select_part.to_string();

    // Pattern: SELECT [whitespace] TOP [whitespace] number/expression [whitespace]
    // Use simple case-insensitive string manipulation to avoid regex dependency
    let lower = result.to_lowercase();

    // Find "select" followed by optional whitespace and "top"
    if let Some(select_pos) = lower.find("select") {
        let after_select = select_pos + "select".len();

        // Skip whitespace after SELECT
        let mut scan_pos = after_select;
        let bytes = result.as_bytes();
        while scan_pos < bytes.len() && bytes[scan_pos].is_ascii_whitespace() {
            scan_pos += 1;
        }

        // Check if "TOP" follows
        let remaining_lower = &lower[scan_pos..];
        if remaining_lower.starts_with("top") {
            let top_end = scan_pos + 3; // "top".len()

            // Skip whitespace after TOP
            let mut after_top = top_end;
            while after_top < bytes.len() && bytes[after_top].is_ascii_whitespace() {
                after_top += 1;
            }

            // Skip the number/expression after TOP
            let mut value_end = after_top;

            // Handle parenthesized expressions like TOP (100)
            if after_top < bytes.len() && bytes[after_top] == b'(' {
                value_end += 1;
                while value_end < bytes.len() && bytes[value_end] != b')' {
                    value_end += 1;
                }
                if value_end < bytes.len() {
                    value_end += 1;
                } // include closing )
            } else {
                // Handle simple numbers and PERCENT keyword
                while value_end < bytes.len()
                    && (bytes[value_end].is_ascii_digit() || bytes[value_end] == b'%')
                {
                    value_end += 1;
                }

                // Check for optional PERCENT keyword
                let mut temp_pos = value_end;
                while temp_pos < bytes.len() && bytes[temp_pos].is_ascii_whitespace() {
                    temp_pos += 1;
                }
                if temp_pos < bytes.len() {
                    let remaining = &lower[temp_pos..];
                    if remaining.starts_with("percent") {
                        value_end = temp_pos + "percent".len();
                    }
                }
            }

            // Skip trailing whitespace after the TOP value
            while value_end < bytes.len() && bytes[value_end].is_ascii_whitespace() {
                value_end += 1;
            }

            // Reconstruct: SELECT + everything after the TOP clause
            let select_part = &result[..select_pos + "select".len()];
            let remaining_part = &result[value_end..];
            result = format!("{} {}", select_part, remaining_part.trim_start());
        }
    }

    result
}

// Helper: build MsSQL SELECT ensuring database context and proper quoting.
// db_name: selected database (can be empty -> fallback to object-provided or omit USE)
// raw_name: could be formats: table, [schema].[object], schema.object, [db].[schema].[object], db.schema.object
pub(crate) fn build_mssql_select_query(db_name: String, raw_name: String) -> String {
    // Normalize raw name: remove trailing semicolons/spaces
    let cleaned = raw_name.trim().trim_end_matches(';').to_string();

    // Split by '.' ignoring brackets segments
    // Strategy: remove outer brackets then split, re-wrap each part with []
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_bracket = false;
    for ch in cleaned.chars() {
        match ch {
            '[' => {
                in_bracket = true;
                current.push(ch);
            }
            ']' => {
                in_bracket = false;
                current.push(ch);
            }
            '.' if !in_bracket => {
                parts.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }

    // Remove surrounding brackets from each part and re-apply sanitized
    let mut plain_parts: Vec<String> = parts
        .into_iter()
        .map(|p| {
            let p2 = p.trim();
            let p2 = p2.strip_prefix('[').unwrap_or(p2);
            let p2 = p2.strip_suffix(']').unwrap_or(p2);
            p2.to_string()
        })
        .collect();

    // Decide final composition
    // Cases by length: 1=object, 2=schema.object, 3=db.schema.object
    // If db_name provided, override database part.
    let (database_part, schema_part, object_part) = match plain_parts.len() {
        3 => {
            let obj = plain_parts.pop().unwrap();
            let schema = plain_parts.pop().unwrap();
            let db = if !db_name.is_empty() {
                db_name.clone()
            } else {
                plain_parts.pop().unwrap()
            };
            (db, schema, obj)
        }
        2 => {
            let obj = plain_parts.pop().unwrap();
            let schema = plain_parts.pop().unwrap();
            let db = if !db_name.is_empty() {
                db_name.clone()
            } else {
                String::new()
            };
            (db, schema, obj)
        }
        1 => {
            let obj = plain_parts.pop().unwrap();
            let db = db_name.clone();
            (db, "dbo".to_string(), obj)
        }
        _ => (db_name.clone(), "dbo".to_string(), cleaned),
    };

    // Build fully qualified name with brackets
    let fq = if database_part.is_empty() {
        format!("[{}].[{}]", schema_part, object_part)
    } else {
        format!("[{}].[{}].[{}]", database_part, schema_part, object_part)
    };

    // If database part present, prepend USE to ensure context
    if database_part.is_empty() {
        format!("SELECT TOP 100 * FROM {};", fq)
    } else {
        format!("USE [{}];\nSELECT TOP 100 * FROM {};", database_part, fq)
    }
}
