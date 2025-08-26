use log::debug;
use redis::aio::ConnectionManager;
use sqlx::SqlitePool;

use crate::{cache_data, connection, models, window_egui};

pub(crate) async fn fetch_redis_data(
    connection_id: i64,
    redis_manager: &ConnectionManager,
    cache_pool: &SqlitePool,
) -> bool {
    // Try to get a Redis connection
    let mut conn = redis_manager.clone();
    match redis::cmd("PING").query_async::<_, String>(&mut conn).await {
        Ok(_) => {
            // Get CONFIG GET databases to determine max database count
            let max_databases = if let Ok(config_result) = redis::cmd("CONFIG")
                .arg("GET")
                .arg("databases")
                .query_async::<_, Vec<String>>(&mut conn)
                .await
            {
                if config_result.len() >= 2 {
                    config_result[1].parse::<i32>().unwrap_or(16)
                } else {
                    16 // Default Redis databases count
                }
            } else {
                16 // Default fallback
            };

            // Cache all potential databases (db0 to db15 by default)
            for db_num in 0..max_databases {
                let db_name = format!("db{}", db_num);
                let _ = sqlx::query("INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)")
                     .bind(connection_id)
                     .bind(&db_name)
                     .execute(cache_pool)
                     .await;
            }

            // Get keyspace info to identify which databases actually have keys
            if let Ok(keyspace_result) = redis::cmd("INFO")
                .arg("keyspace")
                .query_async::<_, String>(&mut conn)
                .await
            {
                for line in keyspace_result.lines() {
                    if line.starts_with("db")
                        && let Some(db_part) = line.split(':').next()
                    {
                        // Mark this database as having keys by adding a special marker
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

            true
        }
        Err(_e) => false,
    }
}

pub(crate) fn load_redis_structure(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Check if we have cached databases
    if let Some(databases) = cache_data::get_databases_from_cache(tabular, connection_id) {
        debug!("🔍 Found cached Redis databases: {:?}", databases);
        if !databases.is_empty() {
            cache_data::build_redis_structure_from_cache(tabular, connection_id, node, &databases);
            node.is_loaded = true;
            return;
        }
    }

    debug!("🔄 No cached Redis databases found, fetching from server...");

    // Fetch fresh data from Redis server
    cache_data::fetch_and_cache_connection_data(tabular, connection_id);

    // Try again to get from cache after fetching
    if let Some(databases) = cache_data::get_databases_from_cache(tabular, connection_id) {
        debug!(
            "✅ Successfully loaded Redis databases from server: {:?}",
            databases
        );
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
                            // Get Redis INFO sections
                            match redis::cmd("INFO").query_async::<_, String>(&mut conn).await {
                                Ok(info_result) => {
                                    let sections: Vec<String> = info_result
                                        .lines()
                                        .filter(|line| line.starts_with('#') && !line.is_empty())
                                        .map(|line| line.trim_start_matches('#').trim().to_string())
                                        .filter(|section| !section.is_empty())
                                        .collect();
                                    Some(sections)
                                }
                                Err(e) => {
                                    debug!("Error getting Redis INFO: {}", e);
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    }
                    "redis_keys" => {
                        // Get sample keys from Redis
                        if database_name.starts_with("db") {
                            // Select the specific database
                            if let Ok(db_num) =
                                database_name.trim_start_matches("db").parse::<i32>()
                            {
                                if (redis::cmd("SELECT")
                                    .arg(db_num)
                                    .query_async::<_, String>(&mut conn)
                                    .await)
                                    .is_ok()
                                {
                                    // Get a sample of keys (limit to first 100)
                                    match redis::cmd("SCAN")
                                        .arg(0)
                                        .arg("COUNT")
                                        .arg(100)
                                        .query_async::<_, Vec<String>>(&mut conn)
                                        .await
                                    {
                                        Ok(keys) => Some(keys),
                                        Err(e) => {
                                            debug!("Error scanning Redis keys: {}", e);
                                            Some(vec!["keys".to_string()]) // Return generic "keys" entry
                                        }
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
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
