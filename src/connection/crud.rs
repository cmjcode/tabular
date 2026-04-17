use crate::{models, modules, ssh_tunnel, window_egui};
use log::{debug, warn};
use mongodb::Client as MongoClient;
use redis::Client;
use sqlx::{
    SqlitePool, mysql::MySqlPoolOptions, postgres::PgPoolOptions, sqlite::SqlitePoolOptions,
};
use std::sync::Arc;

use super::metadata::fetch_and_cache_all_data;
use super::pool::{create_database_pool, resolve_connection_target};

pub(crate) fn update_connection_in_database(
    tabular: &mut window_egui::Tabular,
    connection: &models::structs::ConnectionConfig,
) -> bool {
    if let Some(ref pool) = tabular.db_pool {
        if let Some(id) = connection.id {
            let pool_clone = pool.clone();
            let connection = connection.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();

            // Restart any existing SSH tunnel with updated settings
            ssh_tunnel::shutdown_for_connection(&connection);

            let result = rt.block_on(async {
                sqlx::query(
                    "UPDATE connections SET name = ?, host = ?, port = ?, username = ?, password = ?, database_name = ?, connection_type = ?, folder = ?, ssh_enabled = ?, ssh_host = ?, ssh_port = ?, ssh_username = ?, ssh_auth_method = ?, ssh_private_key = ?, ssh_password = ?, ssh_accept_unknown_host_keys = ? WHERE id = ?"
                )
                .bind(connection.name)
                .bind(connection.host)
                .bind(connection.port)
                .bind(connection.username)
                .bind(connection.password)
                .bind(connection.database)
                .bind(format!("{:?}", connection.connection_type))
                .bind(connection.folder)
                .bind(if connection.ssh_enabled { 1 } else { 0 })
                .bind(connection.ssh_host)
                .bind(connection.ssh_port)
                .bind(connection.ssh_username)
                .bind(connection.ssh_auth_method.as_db_value())
                .bind(connection.ssh_private_key)
                .bind(connection.ssh_password)
                .bind(if connection.ssh_accept_unknown_host_keys { 1 } else { 0 })
                .bind(id)
                .execute(pool_clone.as_ref())
                .await
            });

            match &result {
                Ok(query_result) => {
                    debug!(
                        "Update successful: {} rows affected",
                        query_result.rows_affected()
                    );
                }
                Err(e) => {
                    debug!("Update failed: {}", e);
                }
            }

            result.is_ok()
        } else {
            debug!("Cannot update connection: no ID found");
            false
        }
    } else {
        debug!("Cannot update connection: no database pool available");
        false
    }
}

pub(crate) fn remove_connection(tabular: &mut window_egui::Tabular, connection_id: i64) {
    if let Some(ref pool) = tabular.db_pool {
        let pool_clone = pool.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let result: Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> = rt.block_on(async {
            let mut tx = pool_clone.begin().await?;

            let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await;

            let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await;

            let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await;

            let delete_result = sqlx::query("DELETE FROM connections WHERE id = ?")
                .bind(connection_id)
                .execute(&mut *tx)
                .await?;

            tx.commit().await?;

            Ok(delete_result)
        });

        match result {
            Ok(delete_result) => {
                if delete_result.rows_affected() == 0 {
                    debug!("Warning: No rows were deleted from database!");
                    return;
                }
            }
            Err(e) => {
                debug!("Failed to delete from database: {}", e);
                return;
            }
        }
    }

    tabular.connections.retain(|c| c.id != Some(connection_id));
    tabular.connection_pools.remove(&connection_id);
    tabular.pending_connection_pools.remove(&connection_id);
    ssh_tunnel::shutdown_by_id(connection_id);

    crate::sidebar_database::remove_connection_from_tree(tabular, connection_id);

    tabular.needs_refresh = true;
}

pub(crate) fn test_database_connection(
    connection: &models::structs::ConnectionConfig,
) -> (bool, String) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                let encoded_username = modules::url_encode(&connection.username);
                let encoded_password = modules::url_encode(&connection.password);
                let connection_string = format!(
                    "mysql://{}:{}@{}:{}/{}",
                    encoded_username,
                    encoded_password,
                    target_host,
                    target_port,
                    connection.database
                );

                match MySqlPoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => match sqlx::query("SELECT 1").execute(&pool).await {
                        Ok(_) => (true, "MySQL connection successful!".to_string()),
                        Err(e) => (false, format!("MySQL query failed: {}", e)),
                    },
                    Err(e) => (false, format!("MySQL connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::PostgreSQL => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
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
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&connection_string)
                    .await
                {
                    Ok(pool) => match sqlx::query("SELECT 1").execute(&pool).await {
                        Ok(_) => (true, "PostgreSQL connection successful!".to_string()),
                        Err(e) => (false, format!("PostgreSQL query failed: {}", e)),
                    },
                    Err(e) => (false, format!("PostgreSQL connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::SQLite => {
                let raw = if connection.database.starts_with("sqlite:") {
                    connection.database.clone()
                } else if !connection.host.is_empty() && connection.host.starts_with("sqlite:") {
                    connection.host.clone()
                } else if !connection.host.is_empty() {
                    format!("sqlite:{}", connection.host)
                } else {
                    format!("sqlite:{}", connection.database)
                };

                if let Some(path_str) = raw.strip_prefix("sqlite:") {
                    let path = std::path::PathBuf::from(path_str);
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if !path.exists()
                        && let Ok(_file) = std::fs::File::create(&path)
                    {
                        // file created successfully
                    }
                }

                match SqlitePoolOptions::new()
                    .max_connections(1)
                    .acquire_timeout(std::time::Duration::from_secs(10))
                    .connect(&raw)
                    .await
                {
                    Ok(pool) => match sqlx::query("SELECT 1").execute(&pool).await {
                        Ok(_) => (true, "SQLite connection successful!".to_string()),
                        Err(e) => (false, format!("SQLite query failed: {}", e)),
                    },
                    Err(e) => (false, format!("SQLite connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::MongoDB => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
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
                match MongoClient::with_uri_str(uri).await {
                    Ok(client) => {
                        let admin = client.database("admin");
                        match admin.run_command(mongodb::bson::doc!("ping": 1)).await {
                            Ok(_) => (true, "MongoDB connection successful!".to_string()),
                            Err(e) => (false, format!("MongoDB ping failed: {}", e)),
                        }
                    }
                    Err(e) => (false, format!("MongoDB client error: {}", e)),
                }
            }
            models::enums::DatabaseType::Redis => {
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
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
                    Ok(client) => match client.get_connection() {
                        Ok(mut conn) => {
                            match redis::cmd("PING").query::<String>(&mut conn) {
                                Ok(response) => {
                                    if response == "PONG" {
                                        (true, "Redis connection successful!".to_string())
                                    } else {
                                        (
                                            false,
                                            "Redis PING returned unexpected response".to_string(),
                                        )
                                    }
                                }
                                Err(e) => (false, format!("Redis PING failed: {}", e)),
                            }
                        }
                        Err(e) => (false, format!("Redis connection failed: {}", e)),
                    },
                    Err(e) => (false, format!("Redis client creation failed: {}", e)),
                }
            }
            models::enums::DatabaseType::MsSQL => {
                use futures_util::TryStreamExt;
                let (target_host, target_port) = match resolve_connection_target(connection) {
                    Ok(tuple) => tuple,
                    Err(err) => return (false, err),
                };
                let host = target_host.clone();
                let port: u16 = target_port.parse().unwrap_or(1433);
                let db = connection.database.clone();
                let user = connection.username.clone();
                let pass = connection.password.clone();
                let res = async {
                    use tiberius::{AuthMethod, Config};
                    use tokio_util::compat::TokioAsyncWriteCompatExt;
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
                    let mut client = tiberius::Client::connect(config, tcp.compat_write())
                        .await
                        .map_err(|e| e.to_string())?;
                    let mut s = client
                        .simple_query("SELECT 1")
                        .await
                        .map_err(|e| e.to_string())?;
                    while let Some(item) = s.try_next().await.map_err(|e| e.to_string())? {
                        if let tiberius::QueryItem::Row(_r) = item {
                            break;
                        }
                    }
                    Ok::<_, String>(())
                }
                .await;
                match res {
                    Ok(_) => (true, "MsSQL connection successful!".to_string()),
                    Err(e) => (false, format!("MsSQL connection failed: {}", e)),
                }
            }
            models::enums::DatabaseType::ApiHttp => (
                false,
                "API-HTTP connections do not support database testing".to_string(),
            ),
        }
    })
}

/// Returns true if the error is a SQLite database corruption error (SQLITE_CORRUPT, code 11).
fn is_sqlite_corrupt(e: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = e {
        if db_err
            .code()
            .map_or(false, |c| c.as_ref() == "11")
        {
            return true;
        }
        let msg = db_err.message().to_lowercase();
        return msg.contains("malformed") || msg.contains("disk image is malformed");
    }
    false
}

/// When the SQLite cache is corrupt, attempts recovery by recreating only the cache tables
/// while preserving the `connections` table.  Returns true if recovery succeeded.
async fn recover_corrupt_cache(cache_pool: &SqlitePool) -> bool {
    warn!("[cache_recovery] SQLite cache corruption detected — attempting recovery");

    // Try VACUUM first; it can fix WAL/journal corruption without touching data
    if sqlx::query("VACUUM").execute(cache_pool).await.is_ok() {
        let check = sqlx::query_scalar::<_, String>("PRAGMA integrity_check(1)")
            .fetch_one(cache_pool)
            .await;
        if matches!(check, Ok(ref s) if s == "ok") {
            debug!("[cache_recovery] VACUUM resolved the corruption");
            return true;
        }
    }

    // VACUUM insufficient — drop and recreate all cache tables (connections table is untouched)
    warn!("[cache_recovery] VACUUM insufficient, recreating cache tables...");
    let drops = [
        "DROP TABLE IF EXISTS partition_cache",
        "DROP TABLE IF EXISTS index_cache",
        "DROP TABLE IF EXISTS row_cache",
        "DROP TABLE IF EXISTS column_cache",
        "DROP TABLE IF EXISTS table_cache",
        "DROP TABLE IF EXISTS database_cache",
    ];
    for stmt in &drops {
        if let Err(e) = sqlx::query(stmt).execute(cache_pool).await {
            warn!("[cache_recovery] failed to drop cache table: {}", e);
            // Continue — we still try to recreate below
        }
    }

    let creates = [
        "CREATE TABLE IF NOT EXISTS database_cache (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            connection_id INTEGER NOT NULL,\
            database_name TEXT NOT NULL,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            UNIQUE(connection_id, database_name))",
        "CREATE TABLE IF NOT EXISTS table_cache (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            connection_id INTEGER NOT NULL,\
            database_name TEXT NOT NULL,\
            table_name TEXT NOT NULL,\
            table_type TEXT NOT NULL,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            UNIQUE(connection_id, database_name, table_name, table_type))",
        "CREATE TABLE IF NOT EXISTS column_cache (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            connection_id INTEGER NOT NULL,\
            database_name TEXT NOT NULL,\
            table_name TEXT NOT NULL,\
            column_name TEXT NOT NULL,\
            data_type TEXT NOT NULL,\
            ordinal_position INTEGER NOT NULL,\
            is_primary_key INTEGER NOT NULL DEFAULT 0,\
            is_indexed INTEGER NOT NULL DEFAULT 0,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            UNIQUE(connection_id, database_name, table_name, column_name))",
        "CREATE TABLE IF NOT EXISTS row_cache (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            connection_id INTEGER NOT NULL,\
            database_name TEXT NOT NULL,\
            table_name TEXT NOT NULL,\
            headers_json TEXT NOT NULL,\
            rows_json TEXT NOT NULL,\
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            UNIQUE(connection_id, database_name, table_name))",
        "CREATE TABLE IF NOT EXISTS index_cache (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            connection_id INTEGER NOT NULL,\
            database_name TEXT NOT NULL,\
            table_name TEXT NOT NULL,\
            index_name TEXT NOT NULL,\
            method TEXT NULL,\
            is_unique INTEGER NOT NULL DEFAULT 0,\
            columns_json TEXT NOT NULL DEFAULT '[]',\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            UNIQUE(connection_id, database_name, table_name, index_name))",
        "CREATE TABLE IF NOT EXISTS partition_cache (\
            id INTEGER PRIMARY KEY AUTOINCREMENT,\
            connection_id INTEGER NOT NULL,\
            database_name TEXT NOT NULL,\
            table_name TEXT NOT NULL,\
            partition_name TEXT NOT NULL,\
            partition_type TEXT NULL,\
            partition_expression TEXT NULL,\
            subpartition_type TEXT NULL,\
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,\
            UNIQUE(connection_id, database_name, table_name, partition_name))",
    ];
    let mut all_ok = true;
    for stmt in &creates {
        if let Err(e) = sqlx::query(stmt).execute(cache_pool).await {
            warn!("[cache_recovery] failed to recreate cache table: {}", e);
            all_ok = false;
        }
    }
    if all_ok {
        debug!("[cache_recovery] Cache tables successfully recreated");
    } else {
        warn!("[cache_recovery] Some cache tables could not be recreated; cache may be unavailable until app restart");
    }
    all_ok
}

#[allow(dead_code)]
pub(crate) async fn refresh_connection_background_async(
    connection_id: i64,
    db_pool: &Option<Arc<SqlitePool>>,
) -> bool {
    debug!(
        "[refresh_connection] starting background refresh for connection {}",
        connection_id
    );

    if let Some(cache_pool_arc) = db_pool {
        let connection_result = sqlx::query(
            "SELECT id, name, host, port, username, password, database_name, connection_type, folder, \
                    COALESCE(ssh_enabled, 0) AS ssh_enabled, \
                    COALESCE(ssh_host, '') AS ssh_host, \
                    COALESCE(ssh_port, '22') AS ssh_port, \
                    COALESCE(ssh_username, '') AS ssh_username, \
                    COALESCE(ssh_auth_method, 'key') AS ssh_auth_method, \
                    COALESCE(ssh_private_key, '') AS ssh_private_key, \
                    COALESCE(ssh_password, '') AS ssh_password, \
                    COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys \
             FROM connections WHERE id = ?"
        )
        .bind(connection_id)
        .fetch_optional(cache_pool_arc.as_ref())
        .await;

        if let Ok(Some(row)) = connection_result {
            use sqlx::Row;
            let id = row.try_get::<i64, _>("id").unwrap_or(connection_id);
            let name = row.try_get::<String, _>("name").unwrap_or_default();
            let host = row.try_get::<String, _>("host").unwrap_or_default();
            let port = row
                .try_get::<String, _>("port")
                .unwrap_or_else(|_| "3306".to_string());
            let username = row.try_get::<String, _>("username").unwrap_or_default();
            let password = row.try_get::<String, _>("password").unwrap_or_default();
            let database_name = row
                .try_get::<String, _>("database_name")
                .unwrap_or_default();
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

            let connection = models::structs::ConnectionConfig {
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
            };

            debug!(
                "[refresh_connection] clearing SQLite cache rows for connection {} before reload",
                connection_id
            );

            // Probe for corruption with the first DELETE; recover before touching more tables.
            let first_del = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await;
            match &first_del {
                Ok(r) => debug!(
                    "[refresh_connection] database_cache cleared: {} rows",
                    r.rows_affected()
                ),
                Err(e) if is_sqlite_corrupt(e) => {
                    warn!(
                        "[refresh_connection] cache is corrupt for connection {}: {} — attempting recovery",
                        connection_id, e
                    );
                    recover_corrupt_cache(cache_pool_arc.as_ref()).await;
                    // After recovery the tables are empty; no further DELETE needed.
                }
                Err(e) => warn!(
                    "[refresh_connection] failed clearing database_cache for {}: {}",
                    connection_id, e
                ),
            }

            // Only run remaining DELETEs when the first one did not detect corruption
            // (after recovery the tables are already empty).
            if !matches!(&first_del, Err(e) if is_sqlite_corrupt(e)) {
            match sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await
            {
                Ok(result) => debug!(
                    "[refresh_connection] table_cache cleared: {} rows",
                    result.rows_affected()
                ),
                Err(error) => warn!(
                    "[refresh_connection] failed clearing table_cache for {}: {}",
                    connection_id,
                    error
                ),
            }
            match sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await
            {
                Ok(result) => debug!(
                    "[refresh_connection] column_cache cleared: {} rows",
                    result.rows_affected()
                ),
                Err(error) => warn!(
                    "[refresh_connection] failed clearing column_cache for {}: {}",
                    connection_id,
                    error
                ),
            }
            match sqlx::query("DELETE FROM row_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await
            {
                Ok(result) => debug!(
                    "[refresh_connection] row_cache cleared: {} rows",
                    result.rows_affected()
                ),
                Err(error) => warn!(
                    "[refresh_connection] failed clearing row_cache for {}: {}",
                    connection_id,
                    error
                ),
            }
            match sqlx::query("DELETE FROM index_cache WHERE connection_id = ?")
                .bind(connection_id)
                .execute(cache_pool_arc.as_ref())
                .await
            {
                Ok(result) => debug!(
                    "[refresh_connection] index_cache cleared: {} rows",
                    result.rows_affected()
                ),
                Err(error) => warn!(
                    "[refresh_connection] failed clearing index_cache for {}: {}",
                    connection_id,
                    error
                ),
            }
            } // end of non-corrupt-cache DELETE block

            match tokio::time::timeout(
                std::time::Duration::from_secs(30),
                create_database_pool(&connection),
            )
            .await
            {
                Ok(Some(new_pool)) => {
                    debug!(
                        "[refresh_connection] database pool recreated for connection {} ({:?})",
                        connection_id,
                        connection.connection_type
                    );
                    let fetch_result = fetch_and_cache_all_data(
                        connection_id,
                        &connection,
                        &new_pool,
                        cache_pool_arc.as_ref(),
                    )
                    .await;
                    debug!(
                        "[refresh_connection] cache reload finished for connection {} => {}",
                        connection_id,
                        fetch_result
                    );
                    fetch_result
                }
                Ok(None) => {
                    warn!(
                        "[refresh_connection] failed to recreate database pool for connection {}",
                        connection_id
                    );
                    false
                }
                Err(error) => {
                    warn!(
                        "[refresh_connection] timed out recreating pool for connection {}: {}",
                        connection_id,
                        error
                    );
                    false
                }
            }
        } else {
            warn!(
                "[refresh_connection] connection {} not found in sqlite metadata store",
                connection_id
            );
            false
        }
    } else {
        warn!(
            "[refresh_connection] sqlite cache pool unavailable for connection {}",
            connection_id
        );
        false
    }
}
