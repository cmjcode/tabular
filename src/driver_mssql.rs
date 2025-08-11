// MSSQL driver module now built unconditionally (feature flag removed)
use std::sync::Arc;
use crate::models;
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
                let mut row_vec = Vec::with_capacity(row.len());
                for i in 0..row.len() {
                    let val = match row.get::<&str, _>(i) { Some(v) => v.to_string(), None => {
                        // try other primitive types
                        if let Some(v) = row.get::<i32, _>(i) { v.to_string() }
                        else if let Some(v) = row.get::<i64, _>(i) { v.to_string() }
                        else if let Some(v) = row.get::<f64, _>(i) { v.to_string() }
                        else if let Some(v) = row.get::<bool, _>(i) { v.to_string() }
                        else if let Some(v) = row.get::<&[u8], _>(i) { format!("0x{}", hex::encode(v)) }
                        else { "NULL".to_string() }
                    }};
                    row_vec.push(val);
                }
                data.push(row_vec);
            }
        }
    }
    Ok((headers, data))
}
