use std::collections::HashSet;

use log::{debug, info, warn};
use redis::{Client, aio::ConnectionManager};
use serde_json::{Map, Value, json};
use sqlx::Row;
use sqlx::SqlitePool;

use crate::{cache_data, connection, models, window_egui};

pub(crate) const REDIS_CLUSTER_KEYSPACE: &str = "__redis_cluster__";

fn parse_moved_target(error: &str) -> Option<(String, String)> {
    let address = error
        .split_whitespace()
        .rev()
        .find(|part| part.contains(':'))?
        .trim_matches(|ch: char| matches!(ch, ')' | ']' | ','));

    let (host, port) = address.rsplit_once(':')?;
    Some((host.to_string(), port.to_string()))
}

fn parse_string_as_json(value: String) -> Value {
    serde_json::from_str(&value).unwrap_or(Value::String(value))
}

async fn create_redis_manager_for_target(
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    target: Option<(&str, &str)>,
) -> Result<ConnectionManager, String> {
    let (host, port) = match target {
        Some((host, port)) => (host.to_string(), port.to_string()),
        None => crate::connection::pool::resolve_connection_target(connection)?,
    };

    let connection_string = build_redis_connection_string(
        &host,
        &port,
        &connection.username,
        &connection.password,
    );
    let client = Client::open(connection_string)
        .map_err(|error| format!("Failed to open Redis client for {}:{}: {}", host, port, error))?;
    let mut conn = ConnectionManager::new(client)
        .await
        .map_err(|error| format!("Failed to create Redis connection manager for {}:{}: {}", host, port, error))?;

    if database_name.starts_with("db") {
        let db_num = database_name
            .trim_start_matches("db")
            .parse::<i32>()
            .map_err(|error| format!("Invalid Redis database '{}': {}", database_name, error))?;
        redis::cmd("SELECT")
            .arg(db_num)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|error| format!("Failed to SELECT {} on {}:{}: {}", db_num, host, port, error))?;
    }

    Ok(conn)
}

async fn retry_on_moved_string_command(
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    key_name: &str,
    command: &str,
    extra_args: &[&str],
) -> Result<Option<String>, String> {
    let mut conn = create_redis_manager_for_target(connection, database_name, None).await?;
    let mut cmd = redis::cmd(command);
    cmd.arg(key_name);
    for arg in extra_args {
        cmd.arg(arg);
    }

    match cmd.query_async::<Option<String>>(&mut conn).await {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some((host, port)) = parse_moved_target(&error.to_string()) {
                let mut redirected =
                    create_redis_manager_for_target(connection, database_name, Some((&host, &port)))
                        .await?;
                let mut redirected_cmd = redis::cmd(command);
                redirected_cmd.arg(key_name);
                for arg in extra_args {
                    redirected_cmd.arg(arg);
                }
                redirected_cmd
                    .query_async::<Option<String>>(&mut redirected)
                    .await
                    .map_err(|redirect_error| {
                        format!(
                            "Redis {} failed after MOVED redirect to {}:{}: {}",
                            command, host, port, redirect_error
                        )
                    })
            } else {
                Err(format!("Redis {} failed: {}", command, error))
            }
        }
    }
}

async fn retry_on_moved_vec_command(
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    key_name: &str,
    command: &str,
    extra_args: &[&str],
) -> Result<Vec<String>, String> {
    let mut conn = create_redis_manager_for_target(connection, database_name, None).await?;
    let mut cmd = redis::cmd(command);
    cmd.arg(key_name);
    for arg in extra_args {
        cmd.arg(arg);
    }

    match cmd.query_async::<Vec<String>>(&mut conn).await {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some((host, port)) = parse_moved_target(&error.to_string()) {
                let mut redirected =
                    create_redis_manager_for_target(connection, database_name, Some((&host, &port)))
                        .await?;
                let mut redirected_cmd = redis::cmd(command);
                redirected_cmd.arg(key_name);
                for arg in extra_args {
                    redirected_cmd.arg(arg);
                }
                redirected_cmd
                    .query_async::<Vec<String>>(&mut redirected)
                    .await
                    .map_err(|redirect_error| {
                        format!(
                            "Redis {} failed after MOVED redirect to {}:{}: {}",
                            command, host, port, redirect_error
                        )
                    })
            } else {
                Err(format!("Redis {} failed: {}", command, error))
            }
        }
    }
}

fn redis_key_preview_filename(key_name: &str) -> String {
    let mut sanitized = key_name
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    if sanitized.is_empty() {
        sanitized = "preview".to_string();
    }
    if sanitized.len() > 48 {
        sanitized.truncate(48);
    }
    format!("redis_{}.json", sanitized)
}

pub(crate) fn fetch_redis_key_preview_filename(key_name: &str) -> String {
    redis_key_preview_filename(key_name)
}

pub(crate) fn fetch_redis_key_pretty_json(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    key_name: &str,
    key_type: &str,
) -> Result<String, String> {
    let connection = tabular
        .connections
        .iter()
        .find(|candidate| candidate.id == Some(connection_id))
        .cloned()
        .ok_or_else(|| format!("Redis connection {} not found", connection_id))?;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("Failed to create runtime for Redis preview: {}", error))?;

    runtime.block_on(async move {
        let value = match key_type.to_lowercase().as_str() {
            "string" => {
                let value = retry_on_moved_string_command(
                    &connection,
                    database_name,
                    key_name,
                    "GET",
                    &[],
                )
                .await?;
                value.map(parse_string_as_json).unwrap_or(Value::Null)
            }
            "hash" => {
                let hash_data = retry_on_moved_vec_command(
                    &connection,
                    database_name,
                    key_name,
                    "HGETALL",
                    &[],
                )
                .await?;
                let mut object = Map::new();
                for chunk in hash_data.chunks(2) {
                    if chunk.len() == 2 {
                        object.insert(chunk[0].clone(), parse_string_as_json(chunk[1].clone()));
                    }
                }
                Value::Object(object)
            }
            "list" => {
                let items = retry_on_moved_vec_command(
                    &connection,
                    database_name,
                    key_name,
                    "LRANGE",
                    &["0", "-1"],
                )
                .await?;
                Value::Array(items.into_iter().map(parse_string_as_json).collect())
            }
            "set" => {
                let items = retry_on_moved_vec_command(
                    &connection,
                    database_name,
                    key_name,
                    "SMEMBERS",
                    &[],
                )
                .await?;
                Value::Array(items.into_iter().map(parse_string_as_json).collect())
            }
            "zset" | "sorted_set" => {
                let items = retry_on_moved_vec_command(
                    &connection,
                    database_name,
                    key_name,
                    "ZRANGE",
                    &["0", "-1", "WITHSCORES"],
                )
                .await?;
                let entries = items
                    .chunks(2)
                    .filter(|chunk| chunk.len() == 2)
                    .map(|chunk| {
                        json!({
                            "member": parse_string_as_json(chunk[0].clone()),
                            "score": chunk[1].parse::<f64>().map(Value::from).unwrap_or(Value::String(chunk[1].clone())),
                        })
                    })
                    .collect();
                Value::Array(entries)
            }
            "stream" => json!({
                "message": "Stream preview is not implemented yet",
            }),
            other => json!({
                "message": format!("Preview is not implemented for Redis type {}", other),
            }),
        };

        serde_json::to_string_pretty(&json!({
            "key": key_name,
            "type": key_type,
            "database": database_name,
            "value": value,
        }))
        .map_err(|error| format!("Failed to format Redis key preview as JSON: {}", error))
    })
}

fn build_redis_connection_string(host: &str, port: &str, username: &str, password: &str) -> String {
    if password.is_empty() {
        format!("redis://{}:{}", host, port)
    } else if username.is_empty() {
        format!("redis://:{}@{}:{}", password, host, port)
    } else {
        format!("redis://{}:{}@{}:{}", username, password, host, port)
    }
}

fn parse_cluster_master_addresses(cluster_nodes: &str) -> Vec<(String, String)> {
    let mut addresses = Vec::new();
    let mut seen = HashSet::new();

    for line in cluster_nodes.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 8 {
            continue;
        }

        let flags = parts[2];
        let link_state = parts[7];
        if !flags.contains("master") || flags.contains("fail") || flags.contains("handshake") {
            continue;
        }
        if link_state != "connected" {
            continue;
        }

        let address = parts[1].split('@').next().unwrap_or(parts[1]);
        let address = address.split(',').next().unwrap_or(address);
        if let Some((host, port)) = address.rsplit_once(':') {
            let host = host.to_string();
            let port = port.to_string();
            if seen.insert((host.clone(), port.clone())) {
                addresses.push((host, port));
            }
        }
    }

    addresses
}

async fn scan_keys_and_types_on_node(
    conn: &mut ConnectionManager,
    max_keys: usize,
) -> Vec<(String, String)> {
    let mut all_keys = Vec::new();
    let mut cursor = 0u64;

    loop {
        match redis::cmd("SCAN")
            .arg(cursor)
            .arg("COUNT")
            .arg(100)
            .query_async::<(u64, Vec<String>)>(conn)
            .await
        {
            Ok((next_cursor, keys)) => {
                for key in keys {
                    if all_keys.len() >= max_keys {
                        break;
                    }

                    let key_type = match redis::cmd("TYPE").arg(&key).query_async::<String>(conn).await {
                        Ok(key_type) => key_type,
                        Err(error) => {
                            warn!("[redis_cluster] TYPE failed for key {}: {}", key, error);
                            continue;
                        }
                    };

                    all_keys.push((key, key_type));
                }

                cursor = next_cursor;
                if cursor == 0 || all_keys.len() >= max_keys {
                    break;
                }
            }
            Err(error) => {
                warn!("[redis_cluster] SCAN failed on node: {}", error);
                break;
            }
        }
    }

    all_keys
}

pub(crate) async fn load_redis_connection_config(
    cache_pool: &SqlitePool,
    connection_id: i64,
) -> Option<models::structs::ConnectionConfig> {
    let row = sqlx::query(
        "SELECT id, name, host, port, username, password, database_name, connection_type, folder, \
                COALESCE(ssh_enabled, 0) AS ssh_enabled, \
                COALESCE(ssh_host, '') AS ssh_host, \
                COALESCE(ssh_port, '22') AS ssh_port, \
                COALESCE(ssh_username, '') AS ssh_username, \
                COALESCE(ssh_auth_method, 'key') AS ssh_auth_method, \
                COALESCE(ssh_private_key, '') AS ssh_private_key, \
                COALESCE(ssh_password, '') AS ssh_password, \
                COALESCE(ssh_accept_unknown_host_keys, 0) AS ssh_accept_unknown_host_keys \
         FROM connections WHERE id = ?",
    )
    .bind(connection_id)
    .fetch_optional(cache_pool)
    .await
    .ok()??;

    Some(models::structs::ConnectionConfig {
        id: Some(row.try_get::<i64, _>("id").ok()?),
        name: row.try_get::<String, _>("name").unwrap_or_default(),
        host: row.try_get::<String, _>("host").unwrap_or_default(),
        port: row
            .try_get::<String, _>("port")
            .unwrap_or_else(|_| "6379".to_string()),
        username: row.try_get::<String, _>("username").unwrap_or_default(),
        password: row.try_get::<String, _>("password").unwrap_or_default(),
        database: row.try_get::<String, _>("database_name").unwrap_or_default(),
        connection_type: models::enums::DatabaseType::Redis,
        folder: row.try_get::<Option<String>, _>("folder").unwrap_or(None),
        ssh_enabled: row.try_get::<i64, _>("ssh_enabled").unwrap_or(0) != 0,
        ssh_host: row.try_get::<String, _>("ssh_host").unwrap_or_default(),
        ssh_port: row
            .try_get::<String, _>("ssh_port")
            .unwrap_or_else(|_| "22".to_string()),
        ssh_username: row.try_get::<String, _>("ssh_username").unwrap_or_default(),
        ssh_auth_method: models::enums::SshAuthMethod::from_db_value(
            &row.try_get::<String, _>("ssh_auth_method")
                .unwrap_or_else(|_| "key".to_string()),
        ),
        ssh_private_key: row.try_get::<String, _>("ssh_private_key").unwrap_or_default(),
        ssh_password: row.try_get::<String, _>("ssh_password").unwrap_or_default(),
        ssh_accept_unknown_host_keys: row.try_get::<i64, _>("ssh_accept_unknown_host_keys").unwrap_or(0) != 0,
        custom_views: Vec::new(),
        replication_master_id: None,
    })
}

pub(crate) async fn fetch_cluster_keys_with_types(
    connection: &models::structs::ConnectionConfig,
    seed_manager: &ConnectionManager,
    max_keys: usize,
) -> Vec<(String, String)> {
    let mut seed_conn = seed_manager.clone();
    let cluster_nodes = match redis::cmd("CLUSTER")
        .arg("NODES")
        .query_async::<String>(&mut seed_conn)
        .await
    {
        Ok(cluster_nodes) => cluster_nodes,
        Err(error) => {
            warn!(
                "[redis_cluster] CLUSTER NODES failed for connection {:?}: {}",
                connection.id,
                error
            );
            return Vec::new();
        }
    };

    let master_addresses = parse_cluster_master_addresses(&cluster_nodes);
    info!(
        "[redis_cluster] discovered {} master nodes for connection {:?}",
        master_addresses.len(),
        connection.id
    );

    let mut all_keys = Vec::new();
    let mut seen_keys = HashSet::new();

    for (host, port) in master_addresses {
        if all_keys.len() >= max_keys {
            break;
        }

        let connection_string = build_redis_connection_string(
            &host,
            &port,
            &connection.username,
            &connection.password,
        );
        debug!("[redis_cluster] scanning master node {}:{}", host, port);

        let client = match Client::open(connection_string) {
            Ok(client) => client,
            Err(error) => {
                warn!("[redis_cluster] failed creating client for {}:{}: {}", host, port, error);
                continue;
            }
        };

        let mut node_conn = match ConnectionManager::new(client).await {
            Ok(conn) => conn,
            Err(error) => {
                warn!("[redis_cluster] failed creating connection manager for {}:{}: {}", host, port, error);
                continue;
            }
        };

        for (key, key_type) in scan_keys_and_types_on_node(&mut node_conn, max_keys - all_keys.len()).await {
            if seen_keys.insert(key.clone()) {
                all_keys.push((key, key_type));
            }
            if all_keys.len() >= max_keys {
                break;
            }
        }
    }

    info!(
        "[redis_cluster] collected {} keys across cluster for connection {:?}",
        all_keys.len(),
        connection.id
    );
    all_keys
}

/// Detect Redis Cluster mode by inspecting `INFO server`.
/// Returns true when `redis_mode:cluster` is present.
async fn detect_cluster_mode(conn: &mut ConnectionManager) -> bool {
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        redis::cmd("INFO").arg("server").query_async::<String>(conn),
    )
    .await
    {
        Ok(Ok(info)) => info
            .lines()
            .any(|l| l.trim().eq_ignore_ascii_case("redis_mode:cluster")),
        _ => false,
    }
}

#[allow(dead_code)]
pub(crate) async fn fetch_redis_data(
    connection_id: i64,
    redis_manager: &ConnectionManager,
    cache_pool: &SqlitePool,
) -> bool {
    // Try to get a Redis connection
    let mut conn = redis_manager.clone();
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        redis::cmd("PING").query_async::<String>(&mut conn),
    )
    .await
    {
        Ok(Ok(_)) => {
            // Detect cluster mode first; clusters have no db0/db1 concept.
            let is_cluster = detect_cluster_mode(&mut conn).await;

            if is_cluster {
                debug!("🔀 Redis Cluster detected — using single keyspace");

                // Cluster has one keyspace — store it under a special internal identifier.
                let _ = sqlx::query(
                    "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
                )
                .bind(connection_id)
                .bind(REDIS_CLUSTER_KEYSPACE)
                .execute(cache_pool)
                .await;

                // Always mark the cluster keyspace as having keys.
                let _ = sqlx::query(
                    "INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)",
                )
                .bind(connection_id)
                .bind(REDIS_CLUSTER_KEYSPACE)
                .bind("_has_keys")
                .bind("redis_marker")
                .execute(cache_pool)
                .await;
            } else {
                // Standard Redis: determine the number of logical databases.
                let max_databases = match tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    redis::cmd("CONFIG")
                        .arg("GET")
                        .arg("databases")
                        .query_async::<Vec<String>>(&mut conn),
                )
                .await
                {
                    Ok(Ok(config_result)) => {
                        if config_result.len() >= 2 {
                            config_result[1].parse::<i32>().unwrap_or(16)
                        } else {
                            16
                        }
                    }
                    _ => 16,
                };

                // Cache db0 … db(N-1).
                for db_num in 0..max_databases {
                    let db_name = format!("db{}", db_num);
                    let _ = sqlx::query(
                        "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
                    )
                    .bind(connection_id)
                    .bind(&db_name)
                    .execute(cache_pool)
                    .await;
                }

                // Get keyspace info to identify which databases actually have keys.
                if let Ok(Ok(keyspace_result)) = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    redis::cmd("INFO")
                        .arg("keyspace")
                        .query_async::<String>(&mut conn),
                )
                .await
                {
                    for line in keyspace_result.lines() {
                        if line.starts_with("db")
                            && let Some(db_part) = line.split(':').next()
                        {
                            let _ = sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)")
                                .bind(connection_id)
                                .bind(db_part)
                                .bind("_has_keys")
                                .bind("redis_marker")
                                .execute(cache_pool)
                                .await;
                        }
                    }
                }
            }

            true
        }
        _ => false,
    }
}

pub(crate) fn load_redis_structure(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Always do a live fetch of the database list so that cluster/standalone mode is
    // re-detected on every connection open. This clears any stale db0-db15 entries that
    // may have been cached from a previous standalone session.
    debug!("🔄 Fetching fresh Redis database list (cluster detection)...");
    #[allow(deprecated)]
    if let Some(databases) =
        connection::fetch_databases_from_connection_blocking(tabular, connection_id)
    {
        cache_data::save_databases_to_cache(tabular, connection_id, &databases);
        if !databases.is_empty() {
            cache_data::build_redis_structure_from_cache(tabular, connection_id, node, &databases);
            node.is_loaded = true;
            return;
        }
    }

    // Fallback: use whatever is in cache (e.g., when connection is temporarily unreachable).
    debug!("⚠️  Live Redis db fetch failed — falling back to cache");
    if let Some(databases) = cache_data::get_databases_from_cache(tabular, connection_id) {
        if !databases.is_empty() {
            cache_data::build_redis_structure_from_cache(tabular, connection_id, node, &databases);
            node.is_loaded = true;
            return;
        }
    }

    // Create basic structure for Redis with databases as fallback
    let mut main_children = Vec::new();

    // Add databases folder for Redis
    let mut databases_folder = models::structs::TreeNode::new(
        "Databases".to_string(),
        models::enums::NodeType::DatabasesFolder,
    );
    databases_folder.connection_id = Some(connection_id);
    databases_folder.is_loaded = false;

    // Add a loading indicator
    let loading_node = models::structs::TreeNode::new(
        "Loading databases...".to_string(),
        models::enums::NodeType::Database,
    );
    databases_folder.children.push(loading_node);

    main_children.push(databases_folder);

    node.children = main_children;
}

pub(crate) fn fetch_tables_from_redis_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
    table_type: &str,
) -> Option<Vec<String>> {
    // Create a new runtime for the database query
    let rt = tokio::runtime::Runtime::new().ok()?;

    rt.block_on(async {
        // Get or create connection pool
        let pool = connection::get_or_create_connection_pool(tabular, connection_id).await?;

        match pool {
            models::enums::DatabasePool::Redis(redis_manager) => {
                let mut conn = redis_manager.as_ref().clone();
                match table_type {
                    "info_section" => {
                        // Return the info sections we cached
                        if database_name == "info" {
                            // Get Redis INFO sections (5s timeout)
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                redis::cmd("INFO").query_async::<String>(&mut conn),
                            )
                            .await
                            {
                                Ok(Ok(info_result)) => {
                                    let sections: Vec<String> = info_result
                                        .lines()
                                        .filter(|line| line.starts_with('#') && !line.is_empty())
                                        .map(|line| line.trim_start_matches('#').trim().to_string())
                                        .filter(|section| !section.is_empty())
                                        .collect();
                                    Some(sections)
                                }
                                _ => {
                                    debug!("Error or timeout getting Redis INFO");
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    }
                    "redis_keys" => {
                        // Get sample keys from Redis
                        let is_standard_db = database_name.starts_with("db");
                        let is_cluster_keyspace = database_name == REDIS_CLUSTER_KEYSPACE;

                        if !is_standard_db && !is_cluster_keyspace {
                            return None;
                        }

                        if is_cluster_keyspace {
                            let connection = tabular
                                .connections
                                .iter()
                                .find(|candidate| candidate.id == Some(connection_id))
                                .cloned()?;
                            let keys = fetch_cluster_keys_with_types(&connection, redis_manager.as_ref(), 100)
                                .await
                                .into_iter()
                                .map(|(key, _)| key)
                                .collect();
                            return Some(keys);
                        }

                        if is_standard_db
                            && let Ok(db_num) =
                                database_name.trim_start_matches("db").parse::<i32>()
                        {
                            if tokio::time::timeout(
                                std::time::Duration::from_secs(10),
                                redis::cmd("SELECT")
                                    .arg(db_num)
                                    .query_async::<String>(&mut conn),
                            )
                            .await
                            .is_err()
                            {
                                return None;
                            }
                        }

                        // Get a sample of keys (limit to first 100)
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            redis::cmd("SCAN")
                                .arg(0)
                                .arg("COUNT")
                                .arg(100)
                                .query_async::<Vec<String>>(&mut conn),
                        )
                        .await
                        {
                            Ok(Ok(keys)) => Some(keys),
                            _ => {
                                debug!("Error or timeout scanning Redis keys");
                                Some(vec!["keys".to_string()])
                            }
                        }
                    }
                    _ => {
                        debug!("Unsupported Redis table type: {}", table_type);
                        None
                    }
                }
            }
            _ => {
                debug!("Wrong pool type for Redis connection");
                None
            }
        }
    })
}

#[allow(dead_code)]
pub(crate) fn check_redis_database_has_keys(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    database_name: &str,
) -> bool {
    if let Some(ref pool) = tabular.db_pool {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pool_clone = pool.clone();
        let database_name = database_name.to_string();

        let result = rt.block_on(async move {
              sqlx::query_scalar::<_, i64>(
              "SELECT COUNT(*) FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_name = '_has_keys'"
              )
              .bind(connection_id)
              .bind(database_name)
              .fetch_one(pool_clone.as_ref())
              .await
              .unwrap_or(0)
       });

        result > 0
    } else {
        false
    }
}
