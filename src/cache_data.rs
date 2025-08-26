use log::debug;

use crate::{cache_data, connection, driver_mysql, driver_redis, driver_sqlite, models, window_egui::{self, Tabular}};


pub(crate) fn get_tables_from_cache(tabular: &Tabular, connection_id: i64, database_name: &str, table_type: &str) -> Option<Vec<String>> {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let result = rt.block_on(async {
              sqlx::query_as::<_, (String,)>("SELECT table_name FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type = ? ORDER BY table_name")
              .bind(connection_id)
              .bind(database_name)
              .bind(table_type)
              .fetch_all(pool_clone.as_ref())
              .await
       });
       
       match result {
              Ok(rows) => Some(rows.into_iter().map(|(name,)| name).collect()),
              Err(_) => None,
       }
       } else {
       None
       }
}

pub(crate) fn get_databases_from_cache(tabular: &mut window_egui::Tabular, connection_id: i64) -> Option<Vec<String>> {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let result = rt.block_on(async {
              sqlx::query_as::<_, (String,)>("SELECT database_name FROM database_cache WHERE connection_id = ? ORDER BY database_name")
              .bind(connection_id)
              .fetch_all(pool_clone.as_ref())
              .await
       });
       
       match result {
              Ok(rows) => {
              let databases: Vec<String> = rows.into_iter().map(|(name,)| name).collect();
              Some(databases)
              },
              Err(e) => {
              debug!("Error reading from cache: {}", e);
              None
              }
       }
       } else {
       debug!("No database pool available for cache lookup");
       None
       }
}


pub(crate) fn build_redis_structure_from_cache(tabular: &mut window_egui::Tabular, connection_id: i64, node: &mut models::structs::TreeNode, databases: &[String]) {
       let mut main_children = Vec::new();
       
       // Create databases folder for Redis
       let mut databases_folder = models::structs::TreeNode::new("Databases".to_string(), models::enums::NodeType::DatabasesFolder);
       databases_folder.connection_id = Some(connection_id);
       databases_folder.is_expanded = false;
       databases_folder.is_loaded = true;
       
       // Add each Redis database from cache (db0, db1, etc.)
       for db_name in databases {
       if db_name.starts_with("db") {
              let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
              db_node.connection_id = Some(connection_id);
              db_node.database_name = Some(db_name.clone());
              db_node.is_loaded = false; // Keys will be loaded when clicked
              
              // Check if this database has keys by looking for the marker
              let has_keys = driver_redis::check_redis_database_has_keys(tabular, connection_id, db_name);
              if has_keys {
              // Add a placeholder for keys that will be loaded on expansion
              let loading_node = models::structs::TreeNode::new("Loading keys...".to_string(), models::enums::NodeType::Table);
              db_node.children.push(loading_node);
              }
              
              databases_folder.children.push(db_node);
       }
       }
       
       main_children.push(databases_folder);
       node.children = main_children;
}


// Cache functions for database structure
pub(crate) fn save_databases_to_cache(tabular: &mut window_egui::Tabular, connection_id: i64, databases: &[String]) {
       for db_name in databases {
       debug!("  - {}", db_name);
       }
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let databases_clone = databases.to_vec();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       rt.block_on(async {
              // Clear existing cache for this connection
              let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
              .bind(connection_id)
              .execute(pool_clone.as_ref())
              .await;
              
              // Insert new database names
              for db_name in databases_clone {
              let _ = sqlx::query("INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)")
                     .bind(connection_id)
                     .bind(db_name)
                     .execute(pool_clone.as_ref())
                     .await;
              }
       });
       }
}



pub(crate) fn fetch_and_cache_connection_data(tabular: &mut window_egui::Tabular, connection_id: i64) {
       
       // Clone connection info to avoid borrowing issues
       let connection = if let Some(conn) = tabular.connections.iter().find(|c| c.id == Some(connection_id)) {
       conn.clone()
       } else {
       debug!("Connection not found for ID: {}", connection_id);
       return;
       };
       
       // Fetch databases from server
       let databases_result = connection::fetch_databases_from_connection(tabular, connection_id);
       
       if let Some(databases) = databases_result {
       
       // Save databases to cache
       save_databases_to_cache(tabular, connection_id, &databases);
       
       // For each database, fetch tables and columns
       for database_name in &databases {
              
              // Fetch different types of tables based on database type
                       let table_types = match connection.connection_type {
                              models::enums::DatabaseType::MySQL => vec!["table", "view", "procedure", "function", "trigger", "event"],
                              models::enums::DatabaseType::PostgreSQL => vec!["table", "view"],
                              models::enums::DatabaseType::SQLite => vec!["table", "view"],
                              models::enums::DatabaseType::Redis => vec!["info_section", "redis_keys"],
                              models::enums::DatabaseType::MsSQL => vec!["table", "view", "procedure", "function", "trigger"],
                              models::enums::DatabaseType::MongoDB => vec!["collection"],
                       };
              
              let mut all_tables = Vec::new();
              
              for table_type in table_types {
                       let tables_result = match connection.connection_type {
                     models::enums::DatabaseType::MySQL => {
                     driver_mysql::fetch_tables_from_mysql_connection(tabular, connection_id, database_name, table_type)
                     },
                     models::enums::DatabaseType::SQLite => {
                     driver_sqlite::fetch_tables_from_sqlite_connection(tabular, connection_id, table_type)
                     },
                     models::enums::DatabaseType::PostgreSQL => {
                     crate::driver_postgres::fetch_tables_from_postgres_connection(tabular, connection_id, database_name, table_type)
                     },
                     models::enums::DatabaseType::Redis => {
                     driver_redis::fetch_tables_from_redis_connection(tabular, connection_id, database_name, table_type)
                     },
                                      models::enums::DatabaseType::MsSQL => {
                                             match table_type {
                                                    "table" | "view" => crate::driver_mssql::fetch_tables_from_mssql_connection(tabular, connection_id, database_name, table_type),
                                                    "procedure" | "function" | "trigger" => crate::driver_mssql::fetch_objects_from_mssql_connection(tabular, connection_id, database_name, table_type),
                                                    _ => None,
                                             }
                                       },
                     models::enums::DatabaseType::MongoDB => {
                            if table_type == "collection" {
                                   crate::driver_mongodb::fetch_collections_from_mongodb_connection(tabular, connection_id, database_name)
                            } else { None }
                     },
              };
              
              if let Some(tables) = tables_result {
                     for table_name in tables {
                     all_tables.push((table_name, table_type.to_string()));
                     }
              }
              }
              
              if !all_tables.is_empty() {
              
              // Save tables to cache
              cache_data::save_tables_to_cache(tabular, connection_id, database_name, &all_tables);
              
              // For each table, fetch columns
              for (table_name, table_type) in &all_tables {
                     if table_type == "table" { // Only fetch columns for actual tables, not views/procedures

                     let columns_result = connection::fetch_columns_from_database(connection_id, database_name, table_name, &connection);
                     
                     if let Some(columns) = columns_result {                                
                            // Save columns to cache
                            cache_data::save_columns_to_cache(tabular, connection_id, database_name, table_name, &columns);
                     }
                     }
              }
              }
       }
       
       } else {
       debug!("Failed to fetch databases from server for connection_id: {}", connection_id);
       }
}


pub(crate) fn save_tables_to_cache(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, tables: &[(String, String)]) {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let tables_clone = tables.to_vec();
       let database_name = database_name.to_string();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       rt.block_on(async {
              // Clear existing cache for this database
              let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ? AND database_name = ?")
              .bind(connection_id)
              .bind(&database_name)
              .execute(pool_clone.as_ref())
              .await;
              
              // Insert new table names with types
              for (table_name, table_type) in tables_clone {
              let _ = sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)")
                     .bind(connection_id)
                     .bind(&database_name)
                     .bind(table_name)
                     .bind(table_type)
                     .execute(pool_clone.as_ref())
                     .await;
              }
       });
       }
}

pub(crate) fn save_columns_to_cache(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, table_name: &str, columns: &[(String, String)]) {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let columns_clone = columns.to_vec();
       let database_name = database_name.to_string();
       let table_name = table_name.to_string();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       rt.block_on(async {
              // Clear existing cache for this table
              let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
              .bind(connection_id)
              .bind(&database_name)
              .bind(&table_name)
              .execute(pool_clone.as_ref())
              .await;
              
              // Insert new column names with types
              for (i, (column_name, data_type)) in columns_clone.iter().enumerate() {
              let _ = sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)")
                     .bind(connection_id)
                     .bind(&database_name)
                     .bind(&table_name)
                     .bind(column_name)
                     .bind(data_type)
                     .bind(i as i64)
                     .execute(pool_clone.as_ref())
                     .await;
              }
       });
       }
}

pub(crate) fn get_columns_from_cache(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, table_name: &str) -> Option<Vec<(String, String)>> {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let result = rt.block_on(async {
              sqlx::query_as::<_, (String, String)>("SELECT column_name, data_type FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? ORDER BY ordinal_position")
              .bind(connection_id)
              .bind(database_name)
              .bind(table_name)
              .fetch_all(pool_clone.as_ref())
              .await
       });
       
       result.ok()
       } else {
       None
       }
}

pub(crate) fn get_primary_keys_from_cache(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, table_name: &str) -> Option<Vec<String>> {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let result = rt.block_on(async {
              sqlx::query_as::<_, (String,)>("SELECT column_name FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? AND is_primary_key = 1 ORDER BY ordinal_position")
              .bind(connection_id)
              .bind(database_name)
              .bind(table_name)
              .fetch_all(pool_clone.as_ref())
              .await
       });
       
       match result {
              Ok(rows) => Some(rows.into_iter().map(|(name,)| name).collect()),
              Err(_) => None,
       }
       } else {
       None
       }
}

pub(crate) fn get_indexed_columns_from_cache(tabular: &mut window_egui::Tabular, connection_id: i64, database_name: &str, table_name: &str) -> Option<Vec<String>> {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let result = rt.block_on(async {
              sqlx::query_as::<_, (String,)>("SELECT DISTINCT column_name FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ? AND is_indexed = 1 ORDER BY column_name")
              .bind(connection_id)
              .bind(database_name)
              .bind(table_name)
              .fetch_all(pool_clone.as_ref())
              .await
       });
       
       match result {
              Ok(rows) => Some(rows.into_iter().map(|(name,)| name).collect()),
              Err(_) => None,
       }
       } else {
       None
       }
}
