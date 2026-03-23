use crate::{
    driver_mssql, driver_mysql, driver_postgres, driver_redis, driver_sqlite, models, modules,
    window_egui,
};
use futures_util::TryStreamExt;
use futures_util::stream::StreamExt;
use log::debug;
use mongodb::{Client as MongoClient, bson::doc};
use sqlx::{
    Column, Row, SqlitePool, mysql::MySqlPoolOptions, postgres::PgPoolOptions,
};
use sqlx::Connection as SqlxConnection; // required for MySqlConnection::connect

use super::pool::{create_database_pool, get_or_create_connection_pool};

// Limit concurrent prefetch tasks
const PREFETCH_CONCURRENCY: usize = 6;

// Fetch and cache metadata for all databases/tables/columns per connection
#[allow(dead_code)]
pub(crate) async fn fetch_and_cache_all_data(
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
        models::enums::DatabaseType::ApiHttp => false,
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

// After metadata is cached, fetch first 100 rows for all tables and store in row_cache
#[allow(dead_code)]
async fn prefetch_first_rows_for_all_tables(
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    pool: &models::enums::DatabasePool,
    cache_pool: &SqlitePool,
) -> bool {
    use sqlx::Row;
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
            let enc_user = modules::url_encode(&connection.username);
            let enc_pass = modules::url_encode(&connection.password);
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
                                let data =
                                    crate::driver_mysql::convert_mysql_rows_to_table_data(mysql_rows);
                                save_row_cache_direct(
                                    cache_pool,
                                    connection_id,
                                    &dbn,
                                    &tbn,
                                    &headers,
                                    &data,
                                )
                                .await;
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
                        let q = format!(
                            "SELECT * FROM \"public\".\"{}\" LIMIT 100",
                            tbn.replace('"', "\\\"")
                        );
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
                                                if let Ok(Some(bytes)) =
                                                    row.try_get::<Option<Vec<u8>>, _>(j)
                                                {
                                                    String::from_utf8_lossy(&bytes).to_string()
                                                } else {
                                                    "".to_string()
                                                }
                                            }
                                        })
                                        .collect()
                                })
                                .collect();
                            save_row_cache_direct(
                                cache_pool, connection_id, &dbn, &tbn, &headers, &data,
                            )
                            .await;
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
                        let q =
                            format!("SELECT * FROM `{}` LIMIT 100", tbn.replace('`', "``"));
                        if let Ok(sqlite_rows) = sqlx::query(&q).fetch_all(pool.as_ref()).await {
                            let headers: Vec<String> = if let Some(r0) = sqlite_rows.first() {
                                r0.columns().iter().map(|c| c.name().to_string()).collect()
                            } else {
                                let iq = format!(
                                    "PRAGMA table_info(\"{}\")",
                                    tbn.replace('"', "\\\"")
                                );
                                match sqlx::query(&iq).fetch_all(pool.as_ref()).await {
                                    Ok(infos) => infos
                                        .iter()
                                        .filter_map(|r| r.try_get::<String, _>(1).ok())
                                        .collect(),
                                    Err(_) => Vec::new(),
                                }
                            };
                            let data =
                                crate::driver_sqlite::convert_sqlite_rows_to_table_data(sqlite_rows);
                            save_row_cache_direct(
                                cache_pool,
                                connection_id,
                                "main",
                                &tbn,
                                &headers,
                                &data,
                            )
                            .await;
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

#[deprecated(note = "Use fetch_databases_from_connection_async or background task instead")]
pub(crate) fn fetch_databases_from_connection_blocking(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
) -> Option<Vec<String>> {
    let _connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    let rt = tokio::runtime::Runtime::new().ok()?;

    rt.block_on(async {
        let pool = get_or_create_connection_pool(tabular, connection_id).await?;

        match pool {
            models::enums::DatabasePool::MySQL(mysql_pool) => {
                let result = sqlx::query_as::<_, (String,)>(
                    "SELECT CONVERT(SCHEMA_NAME USING utf8mb4) AS schema_name FROM INFORMATION_SCHEMA.SCHEMATA"
                )
                .fetch_all(mysql_pool.as_ref())
                .await;

                match result {
                    Ok(rows) => {
                        let databases: Vec<String> = rows
                            .into_iter()
                            .map(|(db_name,)| db_name)
                            .filter(|db| {
                                !["information_schema", "performance_schema", "mysql", "sys"]
                                    .contains(&db.as_str())
                            })
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
                        let databases: Vec<String> =
                            rows.into_iter().map(|(db_name,)| db_name).collect();
                        Some(databases)
                    }
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
                    Ok(rows) => {
                        let table_count = rows.len();
                        if table_count > 0 {
                            Some(vec!["main".to_string()])
                        } else {
                            debug!("No tables found in SQLite database, returning 'main' database anyway");
                            Some(vec!["main".to_string()])
                        }
                    }
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

                debug!("Redis max databases: {}", max_databases);

                let mut databases = Vec::new();
                for db_num in 0..max_databases {
                    databases.push(format!("db{}", db_num));
                }

                debug!("Generated Redis databases: {:?}", databases);
                Some(databases)
            }
            models::enums::DatabasePool::MsSQL(pool) => {
                let rt_res = async move {
                    let mut client = pool.get().await.map_err(|e| e.to_string())?;
                    let mut dbs = Vec::new();
                    let mut stream = client
                        .simple_query("SELECT name FROM sys.databases ORDER BY name")
                        .await
                        .map_err(|e| e.to_string())?;
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
                            debug!("MsSQL database list is empty; returning current database only");
                            Some(vec!["master".to_string()])
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
    let _connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    let pool = get_or_create_connection_pool(tabular, connection_id).await?;
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
            .fetch_all(pg_pool.as_ref())
            .await;
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
        models::enums::DatabasePool::MsSQL(pool) => {
            let rt_res = async move {
                let mut client = pool.get().await.map_err(|e| e.to_string())?;
                let mut dbs = Vec::new();
                let mut stream = client
                    .simple_query("SELECT name FROM sys.databases ORDER BY name")
                    .await
                    .map_err(|e| e.to_string())?;
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
                        Some(vec!["master".to_string()])
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

// Background-friendly fetch that doesn't rely on `Tabular` struct
pub async fn fetch_databases_background_task(
    connection_id: i64,
    cache_pool: &SqlitePool,
    shared_pools: &std::sync::Arc<
        std::sync::Mutex<std::collections::HashMap<i64, models::enums::DatabasePool>>,
    >,
) -> Option<Vec<String>> {
    debug!("Background fetch databases for connection {}", connection_id);

    // 1. Get connection config from cache
    let connection_result = sqlx::query("SELECT * FROM connection WHERE id = ?")
        .bind(connection_id)
        .fetch_optional(cache_pool)
        .await;

    let connection = match connection_result {
        Ok(Some(row)) => {
            use sqlx::Row;
            let id = row.try_get::<i64, _>("id").unwrap_or(connection_id);
            let name = row.try_get::<String, _>("name").unwrap_or_default();
            let host = row.try_get::<String, _>("host").unwrap_or_default();
            let port = row
                .try_get::<String, _>("port")
                .unwrap_or_else(|_| "3306".to_string());
            let username = row.try_get::<String, _>("username").unwrap_or_default();
            let password = row.try_get::<String, _>("password").unwrap_or_default();
            let database_name = row.try_get::<String, _>("database_name").unwrap_or_default();
            let connection_type = row
                .try_get::<String, _>("connection_type")
                .unwrap_or_else(|_| "SQLite".to_string());
            let folder = row.try_get::<Option<String>, _>("folder").unwrap_or(None);
            let ssh_enabled = row.try_get::<i64, _>("ssh_enabled").unwrap_or(0);
            let ssh_host = row.try_get::<String, _>("ssh_host").unwrap_or_default();
            let ssh_port = row
                .try_get::<String, _>("ssh_port")
                .unwrap_or_else(|_| "22".to_string());
            let ssh_username = row.try_get::<String, _>("ssh_username").unwrap_or_default();
            let ssh_auth_method = row
                .try_get::<String, _>("ssh_auth_method")
                .unwrap_or_else(|_| "key".to_string());
            let ssh_private_key = row
                .try_get::<String, _>("ssh_private_key")
                .unwrap_or_default();
            let ssh_password = row.try_get::<String, _>("ssh_password").unwrap_or_default();
            let ssh_accept_unknown_host_keys = row
                .try_get::<i64, _>("ssh_accept_unknown_host_keys")
                .unwrap_or(0);

            models::structs::ConnectionConfig {
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
                    "MongoDB" => models::enums::DatabaseType::MongoDB,
                    _ => models::enums::DatabaseType::SQLite,
                },
                folder,
                ssh_enabled: ssh_enabled != 0,
                ssh_host,
                ssh_port,
                ssh_username,
                ssh_auth_method: models::enums::SshAuthMethod::from_db_value(&ssh_auth_method),
                ssh_private_key,
                ssh_password,
                ssh_accept_unknown_host_keys: ssh_accept_unknown_host_keys != 0,
                custom_views: Vec::new(),
                replication_master_id: None,
            }
        }
        _ => {
            debug!("Connection {} not found in cache", connection_id);
            return None;
        }
    };

    // 2. Get or create pool (check shared first)
    let pool = {
        let mut pool_opt = None;
        if let Ok(shared) = shared_pools.lock()
            && let Some(p) = shared.get(&connection_id)
        {
            pool_opt = Some(p.clone());
        }

        if let Some(p) = pool_opt {
            p
        } else {
            match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                create_database_pool(&connection),
            )
            .await
            {
                Ok(Some(p)) => {
                    if let Ok(mut shared) = shared_pools.lock() {
                        shared.insert(connection_id, p.clone());
                    }
                    p
                }
                _ => return None,
            }
        }
    };

    // 3. Fetch databases from pool
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
                    debug!("Error querying MySQL databases: {}", e);
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
            let mut databases = Vec::new();
            for db_num in 0..max_databases {
                databases.push(format!("db{}", db_num));
            }
            Some(databases)
        }
        models::enums::DatabasePool::MsSQL(pool) => {
            let rt_res = async move {
                let mut client = pool.get().await.map_err(|e| e.to_string())?;
                let mut dbs = Vec::new();
                let mut stream = client
                    .simple_query("SELECT name FROM sys.databases ORDER BY name")
                    .await
                    .map_err(|e| e.to_string())?;
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
                        Some(vec!["master".to_string()])
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
                match crate::driver_mysql::fetch_mysql_foreign_keys(p, database_name).await {
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
