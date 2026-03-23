use crate::{driver_mysql, models, modules, window_egui};
use futures_util::TryStreamExt;
use log::debug;
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions};

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
                                    let mut body =
                                        def.trim().trim_end_matches(';').to_string();
                                    if body.is_empty() {
                                        body = format!(
                                            "SELECT * FROM `{}`.`{}`",
                                            db_name, view_name
                                        );
                                    }
                                    let script =
                                        format!("ALTER VIEW {} AS\n{};", qualified, body);
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
                                let schema: Option<String> =
                                    row.try_get::<String, _>("table_schema").ok();
                                let definition: Option<String> = row
                                    .try_get::<String, _>("definition")
                                    .ok()
                                    .or_else(|| {
                                        row.try_get::<Vec<u8>, _>("definition")
                                            .ok()
                                            .map(|b| String::from_utf8_lossy(&b).to_string())
                                    });

                                if let Some(def) = definition {
                                    let schema =
                                        schema.unwrap_or_else(|| "public".to_string());
                                    let escape = |name: &str| name.replace('"', "\"\"");
                                    let qualified = format!(
                                        "\"{}\".\"{}\"",
                                        escape(&schema),
                                        escape(&view_name)
                                    );
                                    let mut body =
                                        def.trim().trim_end_matches(';').to_string();
                                    if body.is_empty() {
                                        body = format!(
                                            "SELECT * FROM \"{}\".\"{}\"",
                                            schema, view_name
                                        );
                                    }
                                    let script =
                                        format!("ALTER VIEW {} AS\n{};", qualified, body);
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

                match sqlx::sqlite::SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        let query =
                            "SELECT sql FROM sqlite_master WHERE type = 'view' AND name = ?";
                        match sqlx::query(query)
                            .bind(&view_name)
                            .fetch_optional(&pool)
                            .await
                        {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                let raw_sql: Option<String> =
                                    row.try_get::<String, _>("sql").ok();
                                if let Some(raw) = raw_sql {
                                    let upper = raw.to_uppercase();
                                    if let Some(idx) = upper.find(" AS ") {
                                        let body =
                                            raw[idx + 4..].trim().trim_end_matches(';');
                                        let escape =
                                            |name: &str| name.replace('"', "\"\"");
                                        let script = format!(
                                            "ALTER VIEW \"{}\" AS\n{};",
                                            escape(&view_name),
                                            body
                                        );
                                        Some(script)
                                    } else if let Some(idx) = upper.find("CREATE") {
                                        let mut script = raw.clone();
                                        script.replace_range(
                                            idx..idx + "CREATE".len(),
                                            "ALTER",
                                        );
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
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
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
                    config.authentication(AuthMethod::sql_server(
                        user.clone(),
                        pass.clone(),
                    ));
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
                                Some(
                                    schema.trim_matches(&['[', ']'][..]).to_string(),
                                ),
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
                        query.push_str(&format!(
                            " AND TABLE_SCHEMA = '{}'",
                            schema.replace("'", "''")
                        ));
                    }

                    let mut stream = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        client.simple_query(query),
                    )
                    .await
                    .map_err(|_| "timeout".to_string())?
                    .map_err(|e| e.to_string())?;

                    while let Some(item) =
                        stream.try_next().await.map_err(|e| e.to_string())?
                    {
                        if let tiberius::QueryItem::Row(row) = item {
                            let schema: Option<&str> = row.get(0);
                            let definition: Option<&str> = row.get(1);
                            if let Some(def) = definition {
                                let schema_name = schema
                                    .map(|s| s.to_string())
                                    .or(schema_opt.clone())
                                    .unwrap_or_else(|| "dbo".to_string());
                                let mut body =
                                    def.trim().trim_end_matches(';').to_string();
                                if body.is_empty() {
                                    body = format!(
                                        "SELECT * FROM [{}].[{}]",
                                        schema_name, view_only
                                    );
                                }
                                let qualified =
                                    format!("[{}].[{}]", schema_name, view_only);
                                let script =
                                    format!("ALTER VIEW {} AS\n{};", qualified, body);
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
            models::enums::DatabaseType::Redis
            | models::enums::DatabaseType::MongoDB
            | models::enums::DatabaseType::ApiHttp => None,
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
                        let qualified = format!(
                            "`{}`.`{}`",
                            db_name.replace('`', "``"),
                            proc_name.replace('`', "``")
                        );
                        let query = format!("SHOW CREATE PROCEDURE {}", qualified);
                        match sqlx::query(&query).fetch_optional(&pool).await {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                let def = row
                                    .try_get::<String, _>(2)
                                    .ok()
                                    .or_else(|| {
                                        row.try_get::<String, _>("Create Procedure").ok()
                                    });
                                if let Some(text) = def {
                                    Some(text)
                                } else {
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
                                        Err(_) => None,
                                    }
                                }
                            }
                            Ok(None) => None,
                            Err(e) => {
                                debug!(
                                    "Failed to SHOW CREATE PROCEDURE for {}: {}",
                                    proc_name, e
                                );
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
                                    Err(_) => None,
                                }
                            }
                        }
                    }
                    Err(e) => {
                        debug!(
                            "MySQL connection error fetching procedure definition: {}",
                            e
                        );
                        None
                    }
                }
            }
            models::enums::DatabaseType::MsSQL => {
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
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
                    config.authentication(AuthMethod::sql_server(
                        user.clone(),
                        pass.clone(),
                    ));
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

                    let parse_qualified = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(&['[', ']'][..]);
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 {
                                return (Some(parts[0].to_string()), parts[1].to_string());
                            }
                        }
                        if let Some((schema, obj)) = name.split_once('.') {
                            return (
                                Some(
                                    schema.trim_matches(&['[', ']'][..]).to_string(),
                                ),
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

                    while let Some(item) =
                        stream.try_next().await.map_err(|e| e.to_string())?
                    {
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

// Fetch foreign keys for a given connection/database. Currently implemented for MySQL only.
pub(crate) async fn get_foreign_keys(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) -> Vec<models::structs::ForeignKey> {
    if let Some(pool) = tabular.connection_pools.get(&connection_id) {
        match pool {
            models::enums::DatabasePool::MySQL(p) => {
                match driver_mysql::fetch_mysql_foreign_keys(p, database_name).await {
                    Ok(keys) => return keys,
                    Err(e) => {
                        debug!("Failed to fetch MySQL foreign keys: {}", e);
                    }
                }
            }
            _ => {
                debug!("Foreign keys not yet supported for this DB type");
            }
        }
    } else {
        debug!("Pool not found for connection {}", connection_id);
    }
    Vec::new()
}

// Fetch table definition (DDL) for supported databases (MySQL, SQLite)
pub(crate) fn fetch_table_definition(
    connection: &models::structs::ConnectionConfig,
    database_name: Option<&str>,
    table_name: &str,
) -> Option<String> {
    let rt = tokio::runtime::Runtime::new().ok()?;

    let connection_clone = connection.clone();
    let db_name = database_name
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| connection_clone.database.clone());
    let tbl_name = table_name.to_string();

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
                        let qualified = format!(
                            "`{}`.`{}`",
                            db_name.replace('`', "``"),
                            tbl_name.replace('`', "``")
                        );
                        let query = format!("SHOW CREATE TABLE {}", qualified);
                        match sqlx::query(&query).fetch_optional(&pool).await {
                            Ok(Some(row)) => {
                                use sqlx::Row;
                                row.try_get::<String, _>(1).ok().or_else(|| {
                                    row.try_get::<String, _>("Create Table").ok()
                                })
                            }
                            Err(e) => {
                                debug!("Failed to fetch table definition: {}", e);
                                None
                            }
                            _ => None,
                        }
                    }
                    Err(e) => {
                        debug!("Failed to connect to MySQL for table definition: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::SQLite => {
                let connection_string = if connection_clone.host.starts_with("sqlite:") {
                    connection_clone.host.clone()
                } else {
                    format!("sqlite:{}", connection_clone.host)
                };

                match sqlx::sqlite::SqlitePoolOptions::new()
                    .max_connections(1)
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => {
                        match sqlx::query_scalar::<_, String>(
                            "SELECT sql FROM sqlite_master WHERE type='table' AND name = ?",
                        )
                        .bind(&tbl_name)
                        .fetch_optional(&pool)
                        .await
                        {
                            Ok(Some(def)) => Some(def),
                            Err(e) => {
                                debug!("Failed to fetch SQLite table definition: {}", e);
                                None
                            }
                            _ => None,
                        }
                    }
                    Err(e) => {
                        debug!("Failed to connect to SQLite for table definition: {}", e);
                        None
                    }
                }
            }
            models::enums::DatabaseType::PostgreSQL => Some(
                "-- Generate Create Table is not yet fully supported for PostgreSQL.\n-- You can view columns in the 'Structure' tab.".to_string(),
            ),
            _ => None,
        }
    })
}
