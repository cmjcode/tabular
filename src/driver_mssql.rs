// MsSQL driver module now built unconditionally (feature flag removed)
use crate::models;
use crate::window_egui; // for Tabular type
use futures_util::StreamExt;
use std::sync::Arc; // for next on tiberius QueryStream

// We'll use tiberius for MsSQL. Tiberius uses async-std or tokio with the "rustls" feature.
// For simplicity here we wrap the basic connection configuration and open connections per query.

pub struct MssqlConfigWrapper {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
}

impl MssqlConfigWrapper {
    pub fn new(
        host: String,
        port: String,
        database: String,
        username: String,
        password: String,
    ) -> Self {
        let port_num: u16 = port.parse().unwrap_or(1433);
        Self {
            host,
            port: port_num,
            database,
            username,
            password,
        }
    }
}

#[allow(dead_code)]
pub(crate) async fn fetch_mssql_data(
    _connection_id: i64,
    _cfg: Arc<MssqlConfigWrapper>,
    _cache_pool: &sqlx::SqlitePool,
) -> bool {
    // TODO: implement metadata caching (databases, tables, columns) using INFORMATION_SCHEMA
    // Placeholder returning true so UI can proceed.
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
    // Users
    let mut users_folder =
        models::structs::TreeNode::new("Users".to_string(), models::enums::NodeType::UsersFolder);
    users_folder.connection_id = Some(connection_id);
    users_folder.is_loaded = false;
    dba_children.push(users_folder);

    // Privileges
    let mut priv_folder = models::structs::TreeNode::new(
        "Privileges".to_string(),
        models::enums::NodeType::PrivilegesFolder,
    );
    priv_folder.connection_id = Some(connection_id);
    priv_folder.is_loaded = false;
    dba_children.push(priv_folder);

    // Processes
    let mut proc_folder = models::structs::TreeNode::new(
        "Processes".to_string(),
        models::enums::NodeType::ProcessesFolder,
    );
    proc_folder.connection_id = Some(connection_id);
    proc_folder.is_loaded = false;
    dba_children.push(proc_folder);

    // Status
    let mut status_folder =
        models::structs::TreeNode::new("Status".to_string(), models::enums::NodeType::StatusFolder);
    status_folder.connection_id = Some(connection_id);
    status_folder.is_loaded = false;
    dba_children.push(status_folder);

    // User Active
    let mut metrics_user_active_folder = models::structs::TreeNode::new(
        "User Active".to_string(),
        models::enums::NodeType::MetricsUserActiveFolder,
    );
    metrics_user_active_folder.connection_id = Some(connection_id);
    metrics_user_active_folder.is_loaded = false;
    dba_children.push(metrics_user_active_folder);

    dba_folder.children = dba_children;
    main_children.push(dba_folder);

    node.children = main_children;
}

/// Fetch MsSQL tables or views for a specific database (synchronous wrapper like other drivers)
pub(crate) fn fetch_tables_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    // Create a new runtime (pattern consistent with other drivers)
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        // Locate connection config
        let connection = tabular.connections.iter().find(|c| c.id == Some(connection_id))?.clone();

        // Open a direct MsSQL connection to the requested database (may differ from original)
        use tokio_util::compat::TokioAsyncWriteCompatExt;
        use tiberius::{AuthMethod, Config};

        let host = connection.host.clone();
        let port: u16 = connection.port.parse().unwrap_or(1433);
        let user = connection.username.clone();
        let pass = connection.password.clone();
        let db_name = if database_name.is_empty() { connection.database.clone() } else { database_name.to_string() };

        let mut config = Config::new();
        config.host(host.clone());
        config.port(port);
        config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
        config.trust_cert();
        if !db_name.is_empty() { config.database(db_name.clone()); }

        let tcp = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::net::TcpStream::connect((host.as_str(), port)),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => { log::debug!("MsSQL connect error for table fetch: {}", e); return None; }
            Err(_) => { log::debug!("MsSQL connect timeout for table fetch"); return None; }
        };
        let _ = tcp.set_nodelay(true);
        let mut client = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tiberius::Client::connect(config, tcp.compat_write()),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => { log::debug!("MsSQL client connect error: {}", e); return None; }
            Err(_) => { log::debug!("MsSQL client connect timeout"); return None; }
        };

        // Choose query based on type (include schema for views)
        let query = match table_type {
            // Include schema for tables (some objects not in dbo)
            "table" => "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE='BASE TABLE' ORDER BY TABLE_NAME".to_string(),
            // Include schema for views so we can build fully-qualified names
            "view" => "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS ORDER BY TABLE_NAME".to_string(),
            _ => {
                log::debug!("Unsupported MsSQL table_type: {}", table_type);
                return None;
            }
        };

        let mut stream = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.simple_query(query),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => { log::debug!("MsSQL list query error: {}", e); return None; }
            Err(_) => { log::debug!("MsSQL list query timeout"); return None; }
        };

        let mut items = Vec::new();
        use futures_util::TryStreamExt;
        while let Some(item) = stream.try_next().await.ok()? {
            if let tiberius::QueryItem::Row(r) = item {
                let schema: Option<&str> = r.get(0);
                let name: Option<&str> = r.get(1);
                if let (Some(s), Some(n)) = (schema, name) {
                    items.push(format!("[{}].[{}]", s, n));
                }
            }
        }
        Some(items)
    })
}

/// Fetch MsSQL objects for a specific database by type: procedure | function | trigger
pub(crate) fn fetch_objects_from_mssql_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    object_type: &str,
) -> Option<Vec<String>> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        let connection = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))?
            .clone();

        use tiberius::{AuthMethod, Config};
        use tokio_util::compat::TokioAsyncWriteCompatExt;

        let host = connection.host.clone();
        let port: u16 = connection.port.parse().unwrap_or(1433);
        let user = connection.username.clone();
        let pass = connection.password.clone();
        let db_name = if database_name.is_empty() {
            connection.database.clone()
        } else {
            database_name.to_string()
        };

        let mut config = Config::new();
        config.host(host.clone());
        config.port(port);
        config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
        config.trust_cert();
        if !db_name.is_empty() {
            config.database(db_name.clone());
        }

        let tcp = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::net::TcpStream::connect((host.as_str(), port)),
        )
        .await
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                log::debug!("MsSQL connect error for object fetch: {}", e);
                return None;
            }
            Err(_) => {
                log::debug!("MsSQL connect timeout for object fetch");
                return None;
            }
        };
        let _ = tcp.set_nodelay(true);
        let mut client = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tiberius::Client::connect(config, tcp.compat_write()),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                log::debug!("MsSQL client connect error: {}", e);
                return None;
            }
            Err(_) => {
                log::debug!("MsSQL client connect timeout");
                return None;
            }
        };

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

        let mut stream = match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.simple_query(query),
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
        use futures_util::TryStreamExt;
        while let Some(item) = stream.try_next().await.ok()? {
            if let tiberius::QueryItem::Row(r) = item {
                match object_type {
                    "procedure" | "function" => {
                        let schema: Option<&str> = r.get(0);
                        let name: Option<&str> = r.get(1);
                        if let (Some(s), Some(n)) = (schema, name) {
                            items.push(format!("[{}].[{}]", s, n));
                        }
                    }
                    "trigger" => {
                        let schema: Option<&str> = r.get(0);
                        let table: Option<&str> = r.get(1);
                        let trig: Option<&str> = r.get(2);
                        if let (Some(s), Some(t), Some(tr)) = (schema, table, trig) {
                            items.push(format!("[{}].[{}].[{}]", s, t, tr));
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(items)
    })
}

/// Execute a query and return (headers, rows)
pub(crate) async fn execute_query(
    cfg: Arc<MssqlConfigWrapper>,
    query: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    use tiberius::{AuthMethod, Config};
    use tokio_util::compat::TokioAsyncWriteCompatExt;

    let mut config = Config::new();
    config.host(cfg.host.clone());
    config.port(cfg.port);
    config.authentication(AuthMethod::sql_server(
        cfg.username.clone(),
        cfg.password.clone(),
    ));
    config.trust_cert(); // for self-signed; in prod expose an option
    if !cfg.database.is_empty() {
        config.database(cfg.database.clone());
    }

    let tcp = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port)),
    )
    .await
    .map_err(|_| "connect timeout".to_string())?
    .map_err(|e| e.to_string())?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    let tls = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tiberius::Client::connect(config, tcp.compat_write()),
    )
    .await
    .map_err(|_| "handshake timeout".to_string())?
    .map_err(|e| e.to_string())?;
    run_query(tls, query).await
}

async fn run_query(
    mut client: tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>,
    query: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut headers: Vec<String> = Vec::new();
    let mut data: Vec<Vec<String>> = Vec::new();

    let mut stream = client.query(query, &[]).await.map_err(|e| e.to_string())?;

    while let Some(item_res) = stream.next().await {
        let item = item_res.map_err(|e| e.to_string())?;
        match item {
            tiberius::QueryItem::Metadata(meta) => {
                headers = meta
                    .columns()
                    .iter()
                    .map(|c| c.name().to_string())
                    .collect();
            }
            tiberius::QueryItem::Row(row) => {
                use tiberius::ColumnData;
                let mut row_vec: Vec<String> = Vec::new();
                for col in row.into_iter() {
                    let val = match col {
                        ColumnData::Bit(Some(v)) => v.to_string(),
                        ColumnData::U8(Some(v)) => v.to_string(),
                        ColumnData::I16(Some(v)) => v.to_string(),
                        ColumnData::I32(Some(v)) => v.to_string(),
                        ColumnData::I64(Some(v)) => v.to_string(),
                        ColumnData::F32(Some(v)) => v.to_string(),
                        ColumnData::F64(Some(v)) => v.to_string(),
                        ColumnData::String(Some(s)) => s.to_string(),
                        ColumnData::Binary(Some(b)) => format!("0x{}", hex::encode(b)),
                        ColumnData::Guid(Some(g)) => g.to_string(),
                        ColumnData::Numeric(Some(n)) => format!("{}", n),
                        ColumnData::DateTime(Some(dt)) => format!("{:?}", dt),
                        ColumnData::SmallDateTime(Some(dt)) => format!("{:?}", dt),
                        ColumnData::Xml(Some(x)) => x.to_string(),
                        // Newer temporal types in recent tiberius versions
                        ColumnData::Time(Some(t)) => format!("{:?}", t),
                        ColumnData::Date(Some(d)) => format!("{:?}", d),
                        ColumnData::DateTime2(Some(dt2)) => format!("{:?}", dt2),
                        ColumnData::DateTimeOffset(Some(dto)) => format!("{:?}", dto),
                        // NULL variants
                        ColumnData::Bit(None)
                        | ColumnData::U8(None)
                        | ColumnData::I16(None)
                        | ColumnData::I32(None)
                        | ColumnData::I64(None)
                        | ColumnData::F32(None)
                        | ColumnData::F64(None)
                        | ColumnData::String(None)
                        | ColumnData::Binary(None)
                        | ColumnData::Guid(None)
                        | ColumnData::Numeric(None)
                        | ColumnData::DateTime(None)
                        | ColumnData::SmallDateTime(None)
                        | ColumnData::Xml(None)
                        | ColumnData::Time(None)
                        | ColumnData::Date(None)
                        | ColumnData::DateTime2(None)
                        | ColumnData::DateTimeOffset(None) => "NULL".to_string(),
                    };
                    row_vec.push(val);
                }
                data.push(row_vec);
            }
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
