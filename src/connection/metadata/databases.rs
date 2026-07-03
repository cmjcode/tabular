use crate::{models, window_egui};
use log::{debug, warn};
use sqlx::SqlitePool;
use crate::connection::pool::{create_database_pool, get_or_create_connection_pool};

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

                // Detect cluster mode — clusters have only a single keyspace.
                let is_cluster = match redis::cmd("INFO")
                    .arg("server")
                    .query_async::<String>(&mut conn)
                    .await
                {
                    Ok(info) => info
                        .lines()
                        .any(|l| l.trim().eq_ignore_ascii_case("redis_mode:cluster")),
                    Err(_) => false,
                };

                if is_cluster {
                    debug!("🔀 Redis Cluster detected — single keyspace");
                    return Some(vec![crate::driver_redis::REDIS_CLUSTER_KEYSPACE.to_string()]);
                }

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
                    let mut dbs = Vec::new();
                    for row in crate::driver_mssql::pooled_query(
                        &pool,
                        "SELECT name FROM sys.databases ORDER BY name",
                    )
                    .await?
                    {
                        if let Some(n) = row.get_string(0) {
                            dbs.push(n);
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
            debug!("[DB-FETCH] conn={} querying INFORMATION_SCHEMA.SCHEMATA...", connection_id);
            let result = sqlx::query_as::<_, (String,)>(
                "SELECT CONVERT(SCHEMA_NAME USING utf8mb4) AS schema_name FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME"
            )
            .fetch_all(mysql_pool.as_ref())
            .await;
            match result {
                Ok(rows) => {
                    debug!("[DB-FETCH] conn={} INFORMATION_SCHEMA.SCHEMATA => {} schemas total:", connection_id, rows.len());
                    for (db,) in &rows {
                        debug!("[DB-FETCH]   - {}", db);
                    }
                    let filtered: Vec<String> = rows
                        .into_iter()
                        .map(|(db_name,)| db_name)
                        .filter(|db| {
                            !["information_schema", "performance_schema", "mysql", "sys"]
                                .contains(&db.as_str())
                        })
                        .collect();
                    debug!("[DB-FETCH] conn={} after filter => {} user databases: {:?}", connection_id, filtered.len(), filtered);
                    if filtered.is_empty() {
                        warn!("[DB-FETCH] conn={} INFORMATION_SCHEMA returned 0 user databases — falling back to SHOW DATABASES", connection_id);
                        match sqlx::query_as::<_, (String,)>("SHOW DATABASES")
                            .fetch_all(mysql_pool.as_ref())
                            .await
                        {
                            Ok(show_rows) => {
                                let show_filtered: Vec<String> = show_rows
                                    .into_iter()
                                    .map(|(db,)| db)
                                    .filter(|db| {
                                        !["information_schema", "performance_schema", "mysql", "sys"]
                                            .contains(&db.as_str())
                                    })
                                    .collect();
                                debug!("[DB-FETCH] conn={} SHOW DATABASES => {} databases: {:?}", connection_id, show_filtered.len(), show_filtered);
                                Some(show_filtered)
                            }
                            Err(e2) => {
                                warn!("[DB-FETCH] conn={} SHOW DATABASES also failed: {}", connection_id, e2);
                                None
                            }
                        }
                    } else {
                        Some(filtered)
                    }
                },
                Err(e) => {
                    warn!("[DB-FETCH] conn={} INFORMATION_SCHEMA.SCHEMATA error: {} — falling back to SHOW DATABASES", connection_id, e);
                    match sqlx::query_as::<_, (String,)>("SHOW DATABASES")
                        .fetch_all(mysql_pool.as_ref())
                        .await
                    {
                        Ok(show_rows) => {
                            let show_filtered: Vec<String> = show_rows
                                .into_iter()
                                .map(|(db,)| db)
                                .filter(|db| {
                                    !["information_schema", "performance_schema", "mysql", "sys"]
                                        .contains(&db.as_str())
                                })
                                .collect();
                            debug!("[DB-FETCH] conn={} SHOW DATABASES => {} databases: {:?}", connection_id, show_filtered.len(), show_filtered);
                            Some(show_filtered)
                        }
                        Err(e2) => {
                            warn!("[DB-FETCH] conn={} SHOW DATABASES also failed: {}", connection_id, e2);
                            None
                        }
                    }
                }
            }
        }
        models::enums::DatabasePool::PostgreSQL(pg_pool) => {
            debug!("[DB-FETCH] conn={} querying PostgreSQL pg_database...", connection_id);
            let result = sqlx::query_as::<_, (String,)>(
                "SELECT datname FROM pg_database WHERE datistemplate = false AND datname NOT IN ('postgres', 'template0', 'template1')"
            )
            .fetch_all(pg_pool.as_ref())
            .await;
            match result {
                Ok(rows) => {
                    let dbs: Vec<String> = rows.into_iter().map(|(db_name,)| db_name).collect();
                    debug!("[DB-FETCH] conn={} PostgreSQL => {} databases: {:?}", connection_id, dbs.len(), dbs);
                    Some(dbs)
                },
                Err(e) => {
                    warn!("[DB-FETCH] conn={} PostgreSQL query error: {}", connection_id, e);
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

            // Detect cluster mode.
            let is_cluster = match redis::cmd("INFO")
                .arg("server")
                .query_async::<String>(&mut conn)
                .await
            {
                Ok(info) => info
                    .lines()
                    .any(|l| l.trim().eq_ignore_ascii_case("redis_mode:cluster")),
                Err(_) => false,
            };

            if is_cluster {
                debug!("🔀 Redis Cluster detected — single keyspace");
                return Some(vec![crate::driver_redis::REDIS_CLUSTER_KEYSPACE.to_string()]);
            }

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
                let mut dbs = Vec::new();
                for row in crate::driver_mssql::pooled_query(
                    &pool,
                    "SELECT name FROM sys.databases ORDER BY name",
                )
                .await?
                {
                    if let Some(n) = row.get_string(0) {
                        dbs.push(n);
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
            debug!("[DB-FETCH] conn={} (background) querying INFORMATION_SCHEMA.SCHEMATA...", connection_id);
            let result = sqlx::query_as::<_, (String,)>(
                "SELECT CONVERT(SCHEMA_NAME USING utf8mb4) AS schema_name FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME"
            )
            .fetch_all(mysql_pool.as_ref())
            .await;
            match result {
                Ok(rows) => {
                    debug!("[DB-FETCH] conn={} INFORMATION_SCHEMA.SCHEMATA => {} schemas total:", connection_id, rows.len());
                    for (db,) in &rows {
                        debug!("[DB-FETCH]   - {}", db);
                    }
                    let filtered: Vec<String> = rows
                        .into_iter()
                        .map(|(db_name,)| db_name)
                        .filter(|db| {
                            !["information_schema", "performance_schema", "mysql", "sys"]
                                .contains(&db.as_str())
                        })
                        .collect();
                    debug!("[DB-FETCH] conn={} after filter => {} user databases: {:?}", connection_id, filtered.len(), filtered);
                    // If INFORMATION_SCHEMA.SCHEMATA returned nothing (permissions issue), fallback to SHOW DATABASES
                    if filtered.is_empty() {
                        warn!("[DB-FETCH] conn={} INFORMATION_SCHEMA returned 0 user databases — falling back to SHOW DATABASES", connection_id);
                        match sqlx::query_as::<_, (String,)>("SHOW DATABASES")
                            .fetch_all(mysql_pool.as_ref())
                            .await
                        {
                            Ok(show_rows) => {
                                let show_filtered: Vec<String> = show_rows
                                    .into_iter()
                                    .map(|(db,)| db)
                                    .filter(|db| {
                                        !["information_schema", "performance_schema", "mysql", "sys"]
                                            .contains(&db.as_str())
                                    })
                                    .collect();
                                debug!("[DB-FETCH] conn={} SHOW DATABASES => {} databases: {:?}", connection_id, show_filtered.len(), show_filtered);
                                Some(show_filtered)
                            }
                            Err(e2) => {
                                warn!("[DB-FETCH] conn={} SHOW DATABASES also failed: {}", connection_id, e2);
                                None
                            }
                        }
                    } else {
                        Some(filtered)
                    }
                },
                Err(e) => {
                    warn!("[DB-FETCH] conn={} INFORMATION_SCHEMA.SCHEMATA error: {} — falling back to SHOW DATABASES", connection_id, e);
                    match sqlx::query_as::<_, (String,)>("SHOW DATABASES")
                        .fetch_all(mysql_pool.as_ref())
                        .await
                    {
                        Ok(show_rows) => {
                            let show_filtered: Vec<String> = show_rows
                                .into_iter()
                                .map(|(db,)| db)
                                .filter(|db| {
                                    !["information_schema", "performance_schema", "mysql", "sys"]
                                        .contains(&db.as_str())
                                })
                                .collect();
                            debug!("[DB-FETCH] conn={} SHOW DATABASES => {} databases: {:?}", connection_id, show_filtered.len(), show_filtered);
                            Some(show_filtered)
                        }
                        Err(e2) => {
                            warn!("[DB-FETCH] conn={} SHOW DATABASES also failed: {}", connection_id, e2);
                            None
                        }
                    }
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
                let mut dbs = Vec::new();
                for row in crate::driver_mssql::pooled_query(
                    &pool,
                    "SELECT name FROM sys.databases ORDER BY name",
                )
                .await?
                {
                    if let Some(n) = row.get_string(0) {
                        dbs.push(n);
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
