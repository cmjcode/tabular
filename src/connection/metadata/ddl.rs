use crate::{driver_mysql, models, modules, window_egui};
use futures_util::TryStreamExt;
use log::debug;
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions};
use std::collections::HashMap;

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

// Fetch foreign keys for a given connection/database (MySQL, PostgreSQL, SQLite, MSSQL).
pub(crate) async fn get_foreign_keys(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) -> Vec<models::structs::ForeignKey> {
    if let Some(pool) = tabular.connection_pools.get(&connection_id).cloned() {
        match pool {
            models::enums::DatabasePool::MySQL(p) => {
                match driver_mysql::fetch_mysql_foreign_keys(&p, database_name).await {
                    Ok(keys) => return keys,
                    Err(e) => debug!("Failed to fetch MySQL foreign keys: {}", e),
                }
            }
            models::enums::DatabasePool::PostgreSQL(p) => {
                match crate::driver_postgres::fetch_postgres_foreign_keys(&p).await {
                    Ok(keys) => return keys,
                    Err(e) => debug!("Failed to fetch PostgreSQL foreign keys: {}", e),
                }
            }
            models::enums::DatabasePool::SQLite(p) => {
                match crate::driver_sqlite::fetch_sqlite_foreign_keys(&p).await {
                    Ok(keys) => return keys,
                    Err(e) => debug!("Failed to fetch SQLite foreign keys: {}", e),
                }
            }
            _ => {}
        }
    } else {
        // MSSQL uses tiberius (no sqlx pool) — fetch via one-off connection
        let conn_opt = tabular.connections.iter().find(|c| c.id == Some(connection_id)).cloned();
        if let Some(conn) = conn_opt {
            if conn.connection_type == models::enums::DatabaseType::MsSQL {
                return fetch_mssql_foreign_keys(&conn, database_name).await;
            }
        }
        debug!("Pool not found for connection {}", connection_id);
    }
    Vec::new()
}

async fn fetch_mssql_foreign_keys(
    conn: &models::structs::ConnectionConfig,
    database_name: &str,
) -> Vec<models::structs::ForeignKey> {
    use tiberius::{AuthMethod, Config};
    use tokio_util::compat::TokioAsyncWriteCompatExt;
    use futures_util::TryStreamExt;

    let host = conn.host.clone();
    let port: u16 = conn.port.parse().unwrap_or(1433);
    let db = if !conn.database.is_empty() { conn.database.clone() } else { database_name.to_string() };

    let mut config = Config::new();
    config.host(&host);
    config.port(port);
    config.authentication(AuthMethod::sql_server(&conn.username, &conn.password));
    config.trust_cert();
    if !db.is_empty() { config.database(&db); }

    let tcp = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect((host.as_str(), port)),
    ).await {
        Ok(Ok(t)) => t,
        _ => return Vec::new(),
    };
    let _ = tcp.set_nodelay(true);

    let mut client = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tiberius::Client::connect(config, tcp.compat_write()),
    ).await {
        Ok(Ok(c)) => c,
        _ => return Vec::new(),
    };

    let q = r#"
        SELECT
            fk.name AS constraint_name,
            OBJECT_NAME(fkc.parent_object_id)     AS table_name,
            c1.name AS column_name,
            OBJECT_NAME(fkc.referenced_object_id) AS referenced_table_name,
            c2.name AS referenced_column_name
        FROM sys.foreign_keys fk
        JOIN sys.foreign_key_columns fkc ON fk.object_id = fkc.constraint_object_id
        JOIN sys.columns c1 ON c1.object_id = fkc.parent_object_id     AND c1.column_id = fkc.parent_column_id
        JOIN sys.columns c2 ON c2.object_id = fkc.referenced_object_id AND c2.column_id = fkc.referenced_column_id
        ORDER BY table_name, column_name
    "#;

    let mut keys = Vec::new();
    if let Ok(Ok(mut stream)) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.simple_query(q),
    ).await {
        while let Ok(Some(item)) = stream.try_next().await {
            if let tiberius::QueryItem::Row(row) = item {
                let get = |i: usize| -> String {
                    row.get::<&str, _>(i).unwrap_or("").to_string()
                };
                keys.push(models::structs::ForeignKey {
                    constraint_name:        get(0),
                    table_name:             get(1),
                    column_name:            get(2),
                    referenced_table_name:  get(3),
                    referenced_column_name: get(4),
                });
            }
        }
    }
    keys
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
            models::enums::DatabaseType::PostgreSQL => {
                if db_name.is_empty() { return None; }
                let conn_str = format!(
                    "postgresql://{}:{}@{}:{}/{}",
                    connection_clone.username, connection_clone.password,
                    connection_clone.host, connection_clone.port, db_name
                );
                let pool = match sqlx::postgres::PgPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&conn_str).await
                {
                    Ok(p) => p,
                    Err(e) => { debug!("PG DDL connect error: {}", e); return None; }
                };
                generate_postgres_ddl(&pool, &tbl_name).await
            }
            models::enums::DatabaseType::MsSQL => {
                generate_mssql_ddl(&connection_clone, &db_name, &tbl_name).await
            }
            _ => None,
        }
    })
}

async fn generate_postgres_ddl(pool: &sqlx::PgPool, tbl_name: &str) -> Option<String> {
    use sqlx::Row;
    // Columns
    let col_rows = sqlx::query(
        "SELECT column_name, data_type, character_maximum_length, numeric_precision, numeric_scale, \
         is_nullable, column_default \
         FROM information_schema.columns \
         WHERE table_schema NOT IN ('pg_catalog','information_schema') AND table_name = $1 \
         ORDER BY ordinal_position"
    ).bind(tbl_name).fetch_all(pool).await.ok()?;

    if col_rows.is_empty() { return None; }

    // PK columns
    let pk_rows = sqlx::query(
        "SELECT kcu.column_name FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
         WHERE tc.table_name = $1 AND tc.constraint_type = 'PRIMARY KEY' \
         ORDER BY kcu.ordinal_position"
    ).bind(tbl_name).fetch_all(pool).await.unwrap_or_default();
    let pk_cols: Vec<String> = pk_rows.iter()
        .filter_map(|r| r.try_get::<String,_>("column_name").ok())
        .collect();

    // FK constraints
    let fk_rows = sqlx::query(
        "SELECT tc.constraint_name, kcu.column_name, ccu.table_name AS ref_table, ccu.column_name AS ref_col \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
         JOIN information_schema.constraint_column_usage ccu \
           ON tc.constraint_name = ccu.constraint_name AND tc.table_schema = ccu.table_schema \
         WHERE tc.table_name = $1 AND tc.constraint_type = 'FOREIGN KEY'"
    ).bind(tbl_name).fetch_all(pool).await.unwrap_or_default();

    // Unique constraints
    let uq_rows = sqlx::query(
        "SELECT tc.constraint_name, kcu.column_name \
         FROM information_schema.table_constraints tc \
         JOIN information_schema.key_column_usage kcu \
           ON tc.constraint_name = kcu.constraint_name AND tc.table_schema = kcu.table_schema \
         WHERE tc.table_name = $1 AND tc.constraint_type = 'UNIQUE' \
         ORDER BY tc.constraint_name, kcu.ordinal_position"
    ).bind(tbl_name).fetch_all(pool).await.unwrap_or_default();

    let esc = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));

    let mut lines: Vec<String> = Vec::new();
    for row in &col_rows {
        let col:     String = row.try_get("column_name").unwrap_or_default();
        let dtype:   String = row.try_get("data_type").unwrap_or_default();
        let char_len: Option<i32> = row.try_get("character_maximum_length").ok();
        let num_p:   Option<i32> = row.try_get("numeric_precision").ok();
        let num_s:   Option<i32> = row.try_get("numeric_scale").ok();
        let nullable: String = row.try_get("is_nullable").unwrap_or_else(|_| "YES".to_string());
        let default:  Option<String> = row.try_get("column_default").ok().flatten();

        let full_type = match dtype.as_str() {
            "character varying" | "character" | "char" | "varchar" => {
                if let Some(l) = char_len { format!("{}({})", dtype, l) } else { dtype.clone() }
            }
            "numeric" | "decimal" => match (num_p, num_s) {
                (Some(p), Some(s)) => format!("{}({},{})", dtype, p, s),
                (Some(p), None)    => format!("{}({})", dtype, p),
                _                  => dtype.clone(),
            },
            _ => dtype.clone(),
        };
        let mut col_def = format!("  {} {}", esc(&col), full_type.to_uppercase());
        if nullable == "NO" { col_def.push_str(" NOT NULL"); }
        if let Some(d) = default { col_def.push_str(&format!(" DEFAULT {}", d)); }
        lines.push(col_def);
    }

    if !pk_cols.is_empty() {
        let pk_str = pk_cols.iter().map(|c| esc(c)).collect::<Vec<_>>().join(", ");
        lines.push(format!("  PRIMARY KEY ({})", pk_str));
    }

    // Group UQ constraints
    let mut uq_map: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
    for row in &uq_rows {
        let name: String = row.try_get("constraint_name").unwrap_or_default();
        let col:  String = row.try_get("column_name").unwrap_or_default();
        uq_map.entry(name).or_default().push(col);
    }
    for (name, cols) in &uq_map {
        let col_str = cols.iter().map(|c| esc(c)).collect::<Vec<_>>().join(", ");
        lines.push(format!("  CONSTRAINT {} UNIQUE ({})", esc(name), col_str));
    }

    for row in &fk_rows {
        let cname:     String = row.try_get("constraint_name").unwrap_or_default();
        let col:       String = row.try_get("column_name").unwrap_or_default();
        let ref_table: String = row.try_get("ref_table").unwrap_or_default();
        let ref_col:   String = row.try_get("ref_col").unwrap_or_default();
        lines.push(format!(
            "  CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}({})",
            esc(&cname), esc(&col), esc(&ref_table), esc(&ref_col)
        ));
    }

    Some(format!("CREATE TABLE {} (\n{}\n);", esc(tbl_name), lines.join(",\n")))
}

async fn generate_mssql_ddl(
    conn: &models::structs::ConnectionConfig,
    db_name: &str,
    tbl_name: &str,
) -> Option<String> {
    use tiberius::{AuthMethod, Config};
    use tokio_util::compat::TokioAsyncWriteCompatExt;
    use futures_util::TryStreamExt;

    let host = conn.host.clone();
    let port: u16 = conn.port.parse().unwrap_or(1433);
    let mut config = Config::new();
    config.host(&host);
    config.port(port);
    config.authentication(AuthMethod::sql_server(&conn.username, &conn.password));
    config.trust_cert();
    if !db_name.is_empty() { config.database(db_name); }

    let tcp = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::TcpStream::connect((host.as_str(), port)),
    ).await {
        Ok(Ok(t)) => t,
        _ => return None,
    };
    let _ = tcp.set_nodelay(true);
    let mut client = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tiberius::Client::connect(config, tcp.compat_write()),
    ).await {
        Ok(Ok(c)) => c,
        _ => return None,
    };

    let tbl_esc = tbl_name.replace("'", "''");
    let q = format!(
        "SELECT c.name AS col_name, tp.name AS type_name, c.max_length, c.precision, c.scale, \
               c.is_nullable, dc.definition AS col_default, c.is_identity \
         FROM sys.columns c \
         JOIN sys.types tp ON tp.user_type_id = c.user_type_id \
         LEFT JOIN sys.default_constraints dc ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id \
         WHERE c.object_id = OBJECT_ID(N'{}') \
         ORDER BY c.column_id", tbl_esc
    );

    let mut col_lines: Vec<String> = Vec::new();
    if let Ok(Ok(mut stream)) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.simple_query(q),
    ).await {
        while let Ok(Some(item)) = stream.try_next().await {
            if let tiberius::QueryItem::Row(row) = item {
                let col:      String = row.get::<&str, _>(0).unwrap_or("").to_string();
                let typename: String = row.get::<&str, _>(1).unwrap_or("").to_string();
                let max_len:  i16    = row.get::<i16, _>(2).unwrap_or(0);
                let prec:     u8     = row.get::<u8,  _>(3).unwrap_or(0);
                let scale:    u8     = row.get::<u8,  _>(4).unwrap_or(0);
                let nullable: bool   = row.get::<bool,_>(5).unwrap_or(true);
                let default:  Option<String> = row.get::<&str,_>(6).map(str::to_string);
                let identity: bool   = row.get::<bool,_>(7).unwrap_or(false);

                let full_type = match typename.to_lowercase().as_str() {
                    "nvarchar" | "varchar" | "nchar" | "char" | "binary" | "varbinary" => {
                        if max_len == -1 { format!("{}(MAX)", typename) }
                        else { format!("{}({})", typename, max_len) }
                    }
                    "decimal" | "numeric" => format!("{}({},{})", typename, prec, scale),
                    _ => typename.clone(),
                };
                let esc_col = format!("[{}]", col);
                let mut line = format!("  {} {}", esc_col, full_type.to_uppercase());
                if identity { line.push_str(" IDENTITY(1,1)"); }
                if !nullable { line.push_str(" NOT NULL"); }
                if let Some(d) = default { line.push_str(&format!(" DEFAULT {}", d)); }
                col_lines.push(line);
            }
        }
    }

    if col_lines.is_empty() { return None; }

    // PK query
    let pk_q = format!(
        "SELECT c.name FROM sys.index_columns ic \
         JOIN sys.indexes i ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
         JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
         WHERE i.is_primary_key = 1 AND ic.object_id = OBJECT_ID(N'{}') \
         ORDER BY ic.key_ordinal", tbl_esc
    );
    let mut pk_cols: Vec<String> = Vec::new();
    if let Ok(Ok(mut stream)) = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        client.simple_query(pk_q),
    ).await {
        while let Ok(Some(item)) = stream.try_next().await {
            if let tiberius::QueryItem::Row(row) = item {
                if let Some(n) = row.get::<&str, _>(0) {
                    pk_cols.push(format!("[{}]", n));
                }
            }
        }
    }
    if !pk_cols.is_empty() {
        col_lines.push(format!("  PRIMARY KEY ({})", pk_cols.join(", ")));
    }

    Some(format!(
        "CREATE TABLE [{}] (\n{}\n);",
        tbl_name, col_lines.join(",\n")
    ))
}

/// Fetch table→[col_name: type] map for any supported engine pool.
async fn fetch_schema_columns(
    pool: &models::enums::DatabasePool,
    db_name: &str,
) -> HashMap<String, Vec<(String, String)>> {
    match pool {
        models::enums::DatabasePool::MySQL(p) => {
            let q = r#"SELECT TABLE_NAME, COLUMN_NAME, DATA_TYPE
                       FROM INFORMATION_SCHEMA.COLUMNS
                       WHERE TABLE_SCHEMA = ?
                       ORDER BY TABLE_NAME, ORDINAL_POSITION"#;
            sqlx::query(q).bind(db_name).fetch_all(p.as_ref()).await
                .unwrap_or_default()
                .into_iter()
                .fold(HashMap::new(), |mut m, row| {
                    use sqlx::Row;
                    let t: String = row.try_get(0).unwrap_or_default();
                    let c: String = row.try_get(1).unwrap_or_default();
                    let dt: String = row.try_get(2).unwrap_or_default();
                    m.entry(t).or_default().push((c, dt));
                    m
                })
        }
        models::enums::DatabasePool::PostgreSQL(p) => {
            let q = r#"SELECT table_name, column_name, data_type
                       FROM information_schema.columns
                       WHERE table_schema NOT IN ('pg_catalog','information_schema')
                       ORDER BY table_name, ordinal_position"#;
            sqlx::query(q).fetch_all(p.as_ref()).await
                .unwrap_or_default()
                .into_iter()
                .fold(HashMap::new(), |mut m, row| {
                    use sqlx::Row;
                    let t: String = row.try_get(0).unwrap_or_default();
                    let c: String = row.try_get(1).unwrap_or_default();
                    let dt: String = row.try_get(2).unwrap_or_default();
                    m.entry(t).or_default().push((c, dt));
                    m
                })
        }
        models::enums::DatabasePool::SQLite(p) => {
            let tables: Vec<String> = sqlx::query_as::<_, (String,)>(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            ).fetch_all(p.as_ref()).await.unwrap_or_default()
             .into_iter().map(|(n,)| n).collect();
            let mut map: HashMap<String, Vec<(String, String)>> = HashMap::new();
            for tbl in tables {
                let pragma = format!("PRAGMA table_info('{}')", tbl.replace('\'', "''"));
                if let Ok(rows) = sqlx::query(&pragma).fetch_all(p.as_ref()).await {
                    for row in rows {
                        use sqlx::Row;
                        let c: String = row.try_get("name").unwrap_or_default();
                        let dt: String = row.try_get("type").unwrap_or_default();
                        map.entry(tbl.clone()).or_default().push((c, dt));
                    }
                }
            }
            map
        }
        _ => HashMap::new(),
    }
}

/// Compare two schemas and return a SchemaDiffResult.
pub(crate) fn compute_schema_diff(
    tabular: &window_egui::Tabular,
    left_conn_id: i64,
    left_db: &str,
    right_conn_id: i64,
    right_db: &str,
) -> models::structs::SchemaDiffResult {
    // Must use the SAME runtime the pools were created on — using a new runtime
    // causes cross-runtime I/O driver conflicts with sqlx (silent panic/failure).
    let rt = match &tabular.runtime {
        Some(rt) => rt.clone(),
        None => return models::structs::SchemaDiffResult { diffs: vec![] },
    };

    let get_pool = |conn_id: i64| -> Option<models::enums::DatabasePool> {
        if let Some(p) = tabular.connection_pools.get(&conn_id) {
            return Some(p.clone());
        }
        tabular.shared_connection_pools.lock().ok()
            .and_then(|shared| shared.get(&conn_id).cloned())
    };
    let left_pool  = get_pool(left_conn_id);
    let right_pool = get_pool(right_conn_id);

    let (left_schema, right_schema) = rt.block_on(async {
        let l = if let Some(p) = left_pool  { fetch_schema_columns(&p, left_db).await  } else { HashMap::new() };
        let r = if let Some(p) = right_pool { fetch_schema_columns(&p, right_db).await } else { HashMap::new() };
        (l, r)
    });

    let mut all_tables: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for k in left_schema.keys()  { all_tables.insert(k); }
    for k in right_schema.keys() { all_tables.insert(k); }

    let mut diffs = Vec::new();
    for table in all_tables {
        let left_cols  = left_schema.get(table);
        let right_cols = right_schema.get(table);

        let status = match (left_cols, right_cols) {
            (Some(_), None)    => models::structs::DiffStatus::Removed,
            (None, Some(_))    => models::structs::DiffStatus::Added,
            (Some(l), Some(r)) => {
                if l == r { models::structs::DiffStatus::Same }
                else      { models::structs::DiffStatus::Modified }
            }
            (None, None) => continue,
        };

        let mut col_diffs = Vec::new();
        if status == models::structs::DiffStatus::Modified {
            let left_map:  HashMap<&str, &str> = left_cols.unwrap().iter().map(|(c, t)| (c.as_str(), t.as_str())).collect();
            let right_map: HashMap<&str, &str> = right_cols.unwrap().iter().map(|(c, t)| (c.as_str(), t.as_str())).collect();
            let mut all_cols: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            for k in left_map.keys()  { all_cols.insert(k); }
            for k in right_map.keys() { all_cols.insert(k); }
            for col in all_cols {
                let lt = left_map.get(col).map(|s| s.to_string());
                let rt2 = right_map.get(col).map(|s| s.to_string());
                if lt != rt2 {
                    col_diffs.push(models::structs::ColumnDiff {
                        name: col.to_string(),
                        left_type:  lt,
                        right_type: rt2,
                    });
                }
            }
        }

        diffs.push(models::structs::TableDiff {
            table_name: table.to_string(),
            status,
            column_diffs: col_diffs,
        });
    }

    models::structs::SchemaDiffResult { diffs }
}
