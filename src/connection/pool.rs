use crate::{models, modules, ssh_tunnel, window_egui::Tabular};
use log::debug;
use mongodb::Client as MongoClient;
use redis::{Client, aio::ConnectionManager};
use sqlx::{
    mysql::MySqlPoolOptions, postgres::PgPoolOptions, sqlite::SqlitePoolOptions,
};
use std::sync::Arc;

/// Resolve the actual host/port to connect to, accounting for SSH tunnels.
pub(crate) fn resolve_connection_target(
    connection: &models::structs::ConnectionConfig,
) -> Result<(String, String), String> {
    if connection.ssh_enabled {
        match connection.connection_type {
            models::enums::DatabaseType::SQLite => {
                Err("SSH tunnel is not supported for SQLite connections".to_string())
            }
            _ => {
                let local_port = ssh_tunnel::ensure_tunnel(connection)?;
                Ok(("127.0.0.1".to_string(), local_port.to_string()))
            }
        }
    } else {
        Ok((connection.host.clone(), connection.port.clone()))
    }
}

// Helper function to clean up completed background pools
pub(crate) fn cleanup_completed_background_pools(tabular: &mut Tabular) {
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
pub(crate) fn cleanup_stuck_pending_connections(tabular: &mut Tabular) {
    if !tabular.pending_connection_pools.is_empty() {
        let stuck_connections: Vec<i64> =
            tabular.pending_connection_pools.iter().copied().collect();
        for connection_id in stuck_connections {
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

/// Create a new connection pool for the given connection configuration.
pub(crate) async fn create_connection_pool_for_config(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MySQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let _encoded_username = modules::url_encode(&connection.username);
            let _encoded_password = modules::url_encode(&connection.password);
            let connection_string = format!(
                "mysql://{}:{}@{}:{}/{}",
                _encoded_username, _encoded_password, target_host, target_port, connection.database
            );

            let mut last_err: Option<sqlx::Error> = None;

            for attempt in 1..=2u8 {
                let start = std::time::Instant::now();
                let (min_conns, test_before, acquire_secs) = match attempt {
                    1 => (0u32, false, 30u64),
                    _ => (1u32, true, 45u64),
                };

                let pool_result = MySqlPoolOptions::new()
                    .max_connections(10)
                    .min_connections(min_conns)
                    .acquire_timeout(std::time::Duration::from_secs(acquire_secs))
                    .idle_timeout(std::time::Duration::from_secs(600))
                    .max_lifetime(std::time::Duration::from_secs(1800))
                    .test_before_acquire(test_before)
                    .after_connect(|conn, _| {
                        Box::pin(async move {
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
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "✅ Created MySQL connection pool (attempt {}, {} ms) for connection {:?}",
                            attempt, elapsed, connection.id
                        );
                        return Some(models::enums::DatabasePool::MySQL(Arc::new(pool)));
                    }
                    Err(e) => {
                        let elapsed = start.elapsed().as_millis();
                        debug!(
                            "❌ MySQL pool attempt {} failed after {} ms for connection {:?}: {:?}",
                            attempt, elapsed, connection.id, e
                        );
                        let is_timeout = matches!(e, sqlx::Error::PoolTimedOut)
                            || e.to_string().contains("timeout");
                        last_err = Some(e);
                        if !is_timeout || attempt == 2 {
                            break;
                        }
                    }
                }
            }

            if let Some(e) = last_err {
                debug!(
                    "❌ Failed to create MySQL pool for connection {:?} after retries: {:?}",
                    connection.id, e
                );
            }
            None
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for PostgreSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                target_host,
                target_port,
                connection.database
            );

            let pool_result = PgPoolOptions::new()
                .max_connections(15)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .max_lifetime(std::time::Duration::from_secs(1800))
                .test_before_acquire(false)
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

            let pool_result = SqlitePoolOptions::new()
                .max_connections(5)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .max_lifetime(std::time::Duration::from_secs(1800))
                .test_before_acquire(false)
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
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for Redis connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", target_host, target_port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, target_host, target_port
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
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MongoDB connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", target_host, target_port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, target_host, target_port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, target_host, target_port
                )
            };
            debug!("Creating MongoDB client for URI: {}", uri);
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                MongoClient::with_uri_str(uri),
            )
            .await
            {
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
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MsSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };

            let client_config = crate::driver_mssql::mssql_config(
                &target_host,
                target_port.parse::<u16>().unwrap_or(1433),
                &connection.username,
                &connection.password,
                Some(&connection.database),
            );

            match mssql_driver_pool::Pool::builder()
                .client_config(client_config)
                .max_connections(20)
                .build()
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::MsSQL(Arc::new(pool))),
                Err(e) => {
                    debug!("MsSQL pool creation failed: {}", e);
                    None
                }
            }
        }
        models::enums::DatabaseType::ApiHttp => {
            // API-HTTP connections do not use a database pool
            None
        }
    }
}

/// Create a database pool (legacy / refresh path). Delegates to create_connection_pool_for_config.
#[allow(dead_code)]
pub(crate) async fn create_database_pool(
    connection: &models::structs::ConnectionConfig,
) -> Option<models::enums::DatabasePool> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            return create_connection_pool_for_config(connection).await;
        }
        models::enums::DatabaseType::PostgreSQL => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for PostgreSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = format!(
                "postgresql://{}:{}@{}:{}/{}",
                connection.username,
                connection.password,
                target_host,
                target_port,
                connection.database
            );

            match PgPoolOptions::new()
                .max_connections(3)
                .min_connections(1)
                .acquire_timeout(std::time::Duration::from_secs(10))
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
                .acquire_timeout(std::time::Duration::from_secs(10))
                .idle_timeout(std::time::Duration::from_secs(300))
                .connect(&connection_string)
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::SQLite(Arc::new(pool))),
                Err(_e) => None,
            }
        }
        models::enums::DatabaseType::Redis => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for Redis connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let connection_string = if connection.password.is_empty() {
                format!("redis://{}:{}", target_host, target_port)
            } else {
                format!(
                    "redis://{}:{}@{}:{}",
                    connection.username, connection.password, target_host, target_port
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
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MsSQL connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };

            let client_config = crate::driver_mssql::mssql_config(
                &target_host,
                target_port.parse::<u16>().unwrap_or(1433),
                &connection.username,
                &connection.password,
                Some(&connection.database),
            );

            match mssql_driver_pool::Pool::builder()
                .client_config(client_config)
                .max_connections(5) // smaller size for temp/check connections
                .build()
                .await
            {
                Ok(pool) => Some(models::enums::DatabasePool::MsSQL(Arc::new(pool))),
                Err(e) => {
                    debug!("MsSQL temp pool creation failed: {}", e);
                    None
                }
            }
        }
        models::enums::DatabaseType::MongoDB => {
            let (target_host, target_port) = match resolve_connection_target(connection) {
                Ok(tuple) => tuple,
                Err(err) => {
                    debug!(
                        "Failed to resolve connection target for MongoDB connection {:?}: {}",
                        connection.id, err
                    );
                    return None;
                }
            };
            let uri = if connection.username.is_empty() {
                format!("mongodb://{}:{}", target_host, target_port)
            } else if connection.password.is_empty() {
                format!(
                    "mongodb://{}@{}:{}",
                    connection.username, target_host, target_port
                )
            } else {
                let enc_user = modules::url_encode(&connection.username);
                let enc_pass = modules::url_encode(&connection.password);
                format!(
                    "mongodb://{}:{}@{}:{}",
                    enc_user, enc_pass, target_host, target_port
                )
            };
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                MongoClient::with_uri_str(uri),
            )
            .await
            {
                Ok(Ok(client)) => Some(models::enums::DatabasePool::MongoDB(Arc::new(client))),
                _ => None,
            }
        }
        models::enums::DatabaseType::ApiHttp => None,
    }
}

/// Try to create pool quickly (with short timeout); returns None if it times out.
async fn try_quick_pool_creation(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    let connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))?
        .clone();

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(500),
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

/// Start background pool creation without blocking the UI thread.
pub(crate) fn start_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
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

/// Ensure a background pool creation is in progress. No-op if pool already exists or pending.
pub(crate) fn ensure_background_pool_creation(tabular: &mut Tabular, connection_id: i64) {
    let has_pool = tabular.connection_pools.contains_key(&connection_id)
        || tabular
            .shared_connection_pools
            .lock()
            .map(|p| p.contains_key(&connection_id))
            .unwrap_or(false);
    if has_pool {
        return;
    }
    if tabular.pending_connection_pools.contains(&connection_id) {
        return;
    }
    tabular.pending_connection_pools.insert(connection_id);
    start_background_pool_creation(tabular, connection_id);
}

/// Get or create a connection pool, using cache, background tasks, or quick creation.
pub(crate) async fn get_or_create_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    cleanup_completed_background_pools(tabular);
    cleanup_stuck_pending_connections(tabular);

    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    if let Ok(shared_pools) = tabular.shared_connection_pools.lock()
        && let Some(shared_pool) = shared_pools.get(&connection_id)
    {
        debug!(
            "✅ Using background-created connection pool for connection {}",
            connection_id
        );
        let pool = shared_pool.clone();
        tabular.connection_pools.insert(connection_id, pool.clone());
        tabular.pending_connection_pools.remove(&connection_id);
        return Some(pool);
    }

    if tabular.pending_connection_pools.contains(&connection_id) {
        let now = std::time::Instant::now();
        let should_log = match tabular.pending_pool_log_last.get(&connection_id) {
            Some(last) => now.duration_since(*last) > std::time::Duration::from_secs(1),
            None => true,
        };
        if should_log {
            debug!(
                "⏳ Connection pool creation already in progress for connection {}",
                connection_id
            );
            tabular.pending_pool_log_last.insert(connection_id, now);
        }
        return None;
    }

    debug!(
        "🔄 Creating new connection pool for connection {}",
        connection_id
    );

    tabular.pending_connection_pools.insert(connection_id);

    match try_quick_pool_creation(tabular, connection_id).await {
        Some(pool) => {
            tabular.connection_pools.insert(connection_id, pool.clone());
            tabular.pending_connection_pools.remove(&connection_id);
            tabular.pending_pool_log_last.remove(&connection_id);
            debug!(
                "✅ Quickly created connection pool for connection {}",
                connection_id
            );
            Some(pool)
        }
        None => {
            start_background_pool_creation(tabular, connection_id);
            None
        }
    }
}

/// Non-blocking version. Returns None immediately if pool is currently being created.
pub(crate) async fn try_get_connection_pool(
    tabular: &mut Tabular,
    connection_id: i64,
) -> Option<models::enums::DatabasePool> {
    cleanup_completed_background_pools(tabular);
    cleanup_stuck_pending_connections(tabular);

    if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
        debug!(
            "✅ Using cached connection pool for connection {}",
            connection_id
        );
        return Some(cached_pool.clone());
    }

    if tabular.pending_connection_pools.contains(&connection_id) {
        debug!(
            "⏳ Connection pool creation in progress for connection {}, skipping for now",
            connection_id
        );
        return None;
    }

    get_or_create_connection_pool(tabular, connection_id).await
}

/// Retry-based pool retrieval. Waits between retries if pool is being created.
#[allow(dead_code)]
pub(crate) async fn get_or_create_connection_pool_with_retry(
    tabular: &mut Tabular,
    connection_id: i64,
    max_retries: u32,
) -> Option<models::enums::DatabasePool> {
    for attempt in 0..=max_retries {
        if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
            debug!(
                "✅ Using cached connection pool for connection {}",
                connection_id
            );
            return Some(cached_pool.clone());
        }

        if !tabular.pending_connection_pools.contains(&connection_id) {
            return get_or_create_connection_pool(tabular, connection_id).await;
        }

        if attempt < max_retries {
            debug!(
                "⏳ Waiting for connection pool creation (attempt {}/{})",
                attempt + 1,
                max_retries + 1
            );
            tokio::time::sleep(std::time::Duration::from_millis(
                500 + attempt as u64 * 200,
            ))
            .await;
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

/// Remove and clean up a connection pool (local cache, shared cache, SSH tunnels).
pub(crate) fn cleanup_connection_pool(tabular: &mut Tabular, connection_id: i64) {
    debug!(
        "🧹 Cleaning up connection pool for connection {}",
        connection_id
    );
    tabular.connection_pools.remove(&connection_id);
    tabular.pending_connection_pools.remove(&connection_id);

    if let Ok(mut shared_pools) = tabular.shared_connection_pools.lock() {
        shared_pools.remove(&connection_id);
    }

    ssh_tunnel::shutdown_by_id(connection_id);
}
