use crate::{models, modules};
use futures_util::TryStreamExt;
use log::debug;
use mongodb::{Client as MongoClient, bson::doc};
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions};

pub(crate) fn fetch_columns_from_database(
    _connection_id: i64,
    database_name: &str,
    table_name: &str,
    connection: &models::structs::ConnectionConfig,
) -> Option<Vec<(String, String)>> {
    let rt = tokio::runtime::Runtime::new().ok()?;

    let connection_clone = connection.clone();
    let database_name = database_name.to_string();
    let table_name = table_name.to_string();

    rt.block_on(async {
        match connection_clone.connection_type {
            models::enums::DatabaseType::MySQL => {
                let encoded_username = modules::url_encode(&connection_clone.username);
                let encoded_password = modules::url_encode(&connection_clone.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username,
                    encoded_password,
                    connection_clone.host,
                    connection_clone.port,
                    database_name
                );

                match MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        let query = "SELECT COLUMN_NAME, COLUMN_TYPE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION";
                        let result = sqlx::query(query)
                            .bind(&database_name)
                            .bind(&table_name)
                            .fetch_all(&pool)
                            .await;
                        match result {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut columns: Vec<(String, String)> =
                                    Vec::with_capacity(rows.len());
                                for row in rows {
                                    let col_name: Option<String> =
                                        match row.try_get::<String, _>("COLUMN_NAME") {
                                            Ok(v) => Some(v),
                                            Err(_) => row
                                                .try_get::<Vec<u8>, _>("COLUMN_NAME")
                                                .ok()
                                                .map(|b| String::from_utf8_lossy(&b).to_string()),
                                        };
                                    let data_type: Option<String> =
                                        match row.try_get::<String, _>("COLUMN_TYPE") {
                                            Ok(v) => Some(v),
                                            Err(_) => row
                                                .try_get::<Vec<u8>, _>("COLUMN_TYPE")
                                                .ok()
                                                .map(|b| String::from_utf8_lossy(&b).to_string()),
                                        };
                                    if let (Some(n), Some(t)) = (col_name, data_type) {
                                        columns.push((n, t));
                                    }
                                }
                                if columns.is_empty() {
                                    let show_q = format!(
                                        "SHOW COLUMNS FROM `{}`.`{}`",
                                        database_name.replace('`', ""),
                                        table_name.replace('`', "")
                                    );
                                    match sqlx::query(&show_q).fetch_all(&pool).await {
                                        Ok(srows) => {
                                            use sqlx::Row;
                                            for r in srows {
                                                let name: Option<String> =
                                                    r.try_get("Field").ok();
                                                let dtype: Option<String> =
                                                    r.try_get("Type").ok();
                                                if let (Some(n), Some(t)) = (name, dtype) {
                                                    columns.push((n, t));
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            debug!(
                                                "MySQL fallback SHOW COLUMNS failed for {}: {}",
                                                table_name, e
                                            );
                                        }
                                    }
                                }
                                Some(columns)
                            }
                            Err(e) => {
                                debug!(
                                    "Error querying MySQL columns for table {}: {}",
                                    table_name, e
                                );
                                let mut columns: Vec<(String, String)> = Vec::new();
                                let show_q = format!(
                                    "SHOW COLUMNS FROM `{}`.`{}`",
                                    database_name.replace('`', ""),
                                    table_name.replace('`', "")
                                );
                                if let Ok(srows) = sqlx::query(&show_q).fetch_all(&pool).await {
                                    use sqlx::Row;
                                    for r in srows {
                                        let name: Option<String> = r.try_get("Field").ok();
                                        let dtype: Option<String> = r.try_get("Type").ok();
                                        if let (Some(n), Some(t)) = (name, dtype) {
                                            columns.push((n, t));
                                        }
                                    }
                                    if !columns.is_empty() {
                                        return Some(columns);
                                    }
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
                let connection_string = format!("sqlite:{}", connection_clone.host);

                match sqlx::sqlite::SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        let escaped = table_name.replace("'", "''");
                        let query = format!("PRAGMA table_info('{}')", escaped);
                        match sqlx::query(&query).fetch_all(&pool).await {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut columns: Vec<(String, String)> = Vec::new();
                                for row in rows {
                                    let name: Option<String> = row.try_get("name").ok();
                                    let data_type: Option<String> = row.try_get("type").ok();
                                    if let (Some(n), Some(t)) = (name, data_type) {
                                        columns.push((n, t));
                                    }
                                }
                                Some(columns)
                            }
                            Err(e) => {
                                debug!(
                                    "Error querying SQLite columns for table {}: {}",
                                    table_name, e
                                );
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
                let connection_string = format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    connection_clone.username,
                    connection_clone.password,
                    connection_clone.host,
                    connection_clone.port,
                    database_name
                );

                match PgPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
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
                                debug!(
                                    "Error querying PostgreSQL columns for table {}: {}",
                                    table_name, e
                                );
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
            models::enums::DatabaseType::Redis => Some(vec![
                ("key".to_string(), "String".to_string()),
                ("value".to_string(), "Any".to_string()),
                ("type".to_string(), "String".to_string()),
                ("ttl".to_string(), "Integer".to_string()),
            ]),
            models::enums::DatabaseType::MongoDB => {
                let uri = if connection_clone.username.is_empty() {
                    format!("mongodb://{}:{}", connection_clone.host, connection_clone.port)
                } else if connection_clone.password.is_empty() {
                    format!(
                        "mongodb://{}@{}:{}",
                        connection_clone.username,
                        connection_clone.host,
                        connection_clone.port
                    )
                } else {
                    let enc_user = modules::url_encode(&connection_clone.username);
                    let enc_pass = modules::url_encode(&connection_clone.password);
                    format!(
                        "mongodb://{}:{}@{}:{}",
                        enc_user, enc_pass, connection_clone.host, connection_clone.port
                    )
                };
                match MongoClient::with_uri_str(uri).await {
                    Ok(client) => {
                        let coll = client
                            .database(&database_name)
                            .collection::<mongodb::bson::Document>(&table_name);
                        match coll.find(doc! {}).limit(1).await {
                            Ok(mut cursor) => {
                                if let Some(doc) = cursor.try_next().await.unwrap_or(None) {
                                    use mongodb::bson::Bson;
                                    let cols: Vec<(String, String)> = doc
                                        .into_iter()
                                        .map(|(k, v)| {
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
                                        })
                                        .collect();
                                    Some(cols)
                                } else {
                                    None
                                }
                            }
                            Err(_) => None,
                        }
                    }
                    Err(_) => None,
                }
            }
            models::enums::DatabaseType::MsSQL => {
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
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
                    if !db.is_empty() {
                        config.database(db.clone());
                    }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port))
                        .await
                        .map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client =
                        tiberius::Client::connect(config, tcp.compat_write())
                            .await
                            .map_err(|e| e.to_string())?;

                    let parse_qualified = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 {
                                return (Some(parts[0].to_string()), parts[1].to_string());
                            }
                        }
                        if let Some((schema, tbl)) = name.split_once('.') {
                            return (
                                Some(schema.trim_matches(|c| c == '[' || c == ']').to_string()),
                                tbl.trim_matches(|c| c == '[' || c == ']').to_string(),
                            );
                        }
                        (None, name.trim_matches(|c| c == '[' || c == ']').to_string())
                    };

                    let (schema_opt, table_only) = parse_qualified(&table);
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
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                        if let tiberius::QueryItem::Row(r) = item {
                            let name: Option<&str> = r.get(0);
                            let dt: Option<&str> = r.get(1);
                            if let (Some(n), Some(d)) = (name, dt) {
                                cols.push((n.to_string(), d.to_string()));
                            }
                        }
                    }
                    Ok::<_, String>(cols)
                }
                .await;
                match rt_res {
                    Ok(v) => Some(v),
                    Err(e) => {
                        debug!("MsSQL column fetch error: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::ApiHttp => None,
        }
    })
}
