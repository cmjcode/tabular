// MSSQL driver module now built unconditionally (feature flag removed)
use std::sync::Arc;
use crate::models;
use crate::window_egui; // for Tabular type
use futures_util::StreamExt; // for next on tiberius QueryStream

// We'll use tiberius for MSSQL. Tiberius uses async-std or tokio with the "rustls" feature.
// For simplicity here we wrap the basic connection configuration and open connections per query.

pub struct MssqlConfigWrapper {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
}

impl MssqlConfigWrapper {
    pub fn new(host: String, port: String, database: String, username: String, password: String) -> Self {
        let port_num: u16 = port.parse().unwrap_or(1433);
        Self { host, port: port_num, database, username, password}
    }
}

pub(crate) async fn fetch_mssql_data(_connection_id: i64, _cfg: Arc<MssqlConfigWrapper>, _cache_pool: &sqlx::SqlitePool) -> bool {
    // TODO: implement metadata caching (databases, tables, columns) using INFORMATION_SCHEMA
    // Placeholder returning true so UI can proceed.
    true
}

pub(crate) fn load_mssql_structure(connection_id: i64, _connection: &models::structs::ConnectionConfig, node: &mut models::structs::TreeNode) {
    // Similar to PostgreSQL: show Databases folder with loading marker.
    let mut main_children = Vec::new();
    let mut databases_folder = models::structs::TreeNode::new("Databases".to_string(), models::enums::NodeType::DatabasesFolder);
    databases_folder.connection_id = Some(connection_id);
    let loading_node = models::structs::TreeNode::new("Loading databases...".to_string(), models::enums::NodeType::Database);
    databases_folder.children.push(loading_node);
    main_children.push(databases_folder);
    node.children = main_children;
}

/// Fetch MSSQL tables or views for a specific database (synchronous wrapper like other drivers)
pub(crate) fn fetch_tables_from_mssql_connection(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, table_type: &str) -> Option<Vec<String>> {
    // Create a new runtime (pattern consistent with other drivers)
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        // Locate connection config
        let connection = tabular.connections.iter().find(|c| c.id == Some(connection_id))?.clone();

        // Open a direct MSSQL connection to the requested database (may differ from original)
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

        let tcp = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
            Ok(t) => t,
            Err(e) => {
                log::debug!("MSSQL connect error for table fetch: {}", e);
                return None;
            }
        };
        let _ = tcp.set_nodelay(true);
        let mut client = match tiberius::Client::connect(config, tcp.compat_write()).await {
            Ok(c) => c,
            Err(e) => {
                log::debug!("MSSQL client connect error: {}", e);
                return None;
            }
        };

        // Choose query based on type (include schema for views)
        let query = match table_type {
            // Include schema for tables (some objects not in dbo)
            "table" => "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE='BASE TABLE' ORDER BY TABLE_NAME".to_string(),
            // Include schema for views so we can build fully-qualified names
            "view" => "SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.VIEWS ORDER BY TABLE_NAME".to_string(),
            _ => {
                log::debug!("Unsupported MSSQL table_type: {}", table_type);
                return None;
            }
        };

        let mut stream = match client.simple_query(query).await {
            Ok(s) => s,
            Err(e) => { log::debug!("MSSQL list query error: {}", e); return None; }
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

/// Fetch MSSQL objects for a specific database by type: procedure | function | trigger
pub(crate) fn fetch_objects_from_mssql_connection(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, object_type: &str) -> Option<Vec<String>> {
    let rt = tokio::runtime::Runtime::new().ok()?;
    rt.block_on(async {
        let connection = tabular.connections.iter().find(|c| c.id == Some(connection_id))?.clone();

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

        let tcp = match tokio::net::TcpStream::connect((host.as_str(), port)).await { Ok(t) => t, Err(e) => { log::debug!("MSSQL connect error for object fetch: {}", e); return None; } };
        let _ = tcp.set_nodelay(true);
        let mut client = match tiberius::Client::connect(config, tcp.compat_write()).await { Ok(c) => c, Err(e) => { log::debug!("MSSQL client connect error: {}", e); return None; } };

        let query = match object_type {
            // Stored procedures
            "procedure" => {
                // sys.procedures excludes system procedures when filtered by is_ms_shipped = 0
                "SELECT s.name AS schema_name, p.name AS object_name \
                 FROM sys.procedures p \
                 JOIN sys.schemas s ON p.schema_id = s.schema_id \
                 WHERE ISNULL(p.is_ms_shipped,0) = 0 \
                 ORDER BY p.name".to_string()
            }
            // Functions: scalar, inline table-valued, multi-statement table-valued, and CLR variants
            "function" => {
                "SELECT s.name AS schema_name, o.name AS object_name \
                 FROM sys.objects o \
                 JOIN sys.schemas s ON o.schema_id = s.schema_id \
                 WHERE o.type IN ('FN','IF','TF','AF','FS','FT') \
                 ORDER BY o.name".to_string()
            }
            // Triggers: list DML triggers attached to user tables
            "trigger" => {
                "SELECT ss.name AS schema_name, t.name AS table_name, tr.name AS trigger_name \
                 FROM sys.triggers tr \
                 JOIN sys.tables t ON tr.parent_id = t.object_id \
                 JOIN sys.schemas ss ON t.schema_id = ss.schema_id \
                 WHERE t.type = 'U' \
                 ORDER BY tr.name".to_string()
            }
            _ => { log::debug!("Unsupported MSSQL object_type: {}", object_type); return None; }
        };

        let mut stream = match client.simple_query(query).await { Ok(s) => s, Err(e) => { log::debug!("MSSQL object list query error: {}", e); return None; } };

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
pub(crate) async fn execute_query(cfg: Arc<MssqlConfigWrapper>, query: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    use tokio_util::compat::TokioAsyncWriteCompatExt;
    use tiberius::{AuthMethod, Config};

    let mut config = Config::new();
    config.host(cfg.host.clone());
    config.port(cfg.port);
    config.authentication(AuthMethod::sql_server(cfg.username.clone(), cfg.password.clone()));
    config.trust_cert(); // for self-signed; in prod expose an option
    if !cfg.database.is_empty() { config.database(cfg.database.clone()); }

    let tcp = tokio::net::TcpStream::connect((cfg.host.as_str(), cfg.port)).await.map_err(|e| e.to_string())?;
    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
    let tls = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
    run_query(tls, query).await
}

async fn run_query(mut client: tiberius::Client<tokio_util::compat::Compat<tokio::net::TcpStream>>, query: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let mut headers: Vec<String> = Vec::new();
    let mut data: Vec<Vec<String>> = Vec::new();

    let mut stream = client.simple_query(query).await.map_err(|e| e.to_string())?;

    while let Some(item_res) = stream.next().await {
        let item = item_res.map_err(|e| e.to_string())?;
        match item {
            tiberius::QueryItem::Metadata(meta) => {
                headers = meta.columns().iter().map(|c| c.name().to_string()).collect();
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
                        | ColumnData::Xml(None) => "NULL".to_string(),
                    };
                    row_vec.push(val);
                }
                data.push(row_vec);
            }
        }
    }
    Ok((headers, data))
}
