use log::debug;
use sqlx::{Row, SqlitePool};

use crate::{connection, models, window_egui};

pub async fn fetch_data(connection_id: i64, pool: &SqlitePool, cache_pool: &SqlitePool) -> bool {
    // For SQLite, we typically work with the main database, but we can get table info
    if let Ok(rows) = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await
    {
        // Cache the main database
        let db_name = "main";
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO database_cache (connection_id, database_name) VALUES (?, ?)",
        )
        .bind(connection_id)
        .bind(db_name)
        .execute(cache_pool)
        .await;

        // Cache tables
        for row in rows {
            if let Ok(table_name) = row.try_get::<String, _>(0) {
                // Cache table
                let _ = sqlx::query("INSERT OR REPLACE INTO table_cache (connection_id, database_name, table_name, table_type) VALUES (?, ?, ?, ?)")
                            .bind(connection_id)
                            .bind(db_name)
                            .bind(&table_name)
                            .bind("table")
                            .execute(cache_pool)
                            .await;

                // Fetch columns for this table
                // Quote and escape table name to avoid issues with special characters or injection
                let escaped_table = table_name.replace("'", "''");
                let col_query = format!("PRAGMA table_info('{}')", escaped_table);
                if let Ok(col_rows) = sqlx::query(&col_query).fetch_all(pool).await {
                    for col_row in col_rows {
                        if let (Ok(col_name), Ok(col_type)) = (
                            col_row.try_get::<String, _>("name"),
                            col_row.try_get::<String, _>("type"),
                        ) {
                            // Cache column
                            let _ = sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)")
                                          .bind(connection_id)
                                          .bind(db_name)
                                          .bind(&table_name)
                                          .bind(&col_name)
                                          .bind(&col_type)
                                          .bind(0) // SQLite doesn't have ordinal position in PRAGMA
                                          .execute(cache_pool)
                                          .await;
                        }
                    }
                }
            }
        }
        true
    } else {
        false
    }
}

// Helper function to convert SQLite rows to Vec<Vec<String>> with proper type checking
pub(crate) fn convert_sqlite_rows_to_table_data(
    rows: Vec<sqlx::sqlite::SqliteRow>,
) -> Vec<Vec<String>> {
    use sqlx::{Column, Row, TypeInfo};

    let mut table_data = Vec::new();

    for row in &rows {
        let mut row_data = Vec::new();
        let columns = row.columns();

        for (col_idx, column) in columns.iter().enumerate() {
            let column_name = column.name();
            let type_info = column.type_info();
            let type_name = type_info.name();

            // Convert value based on SQLite type
            let value_str = match type_name {
                // SQLite INTEGER type
                "INTEGER" => {
                    // Try different integer sizes
                    if let Ok(Some(val)) = row.try_get::<Option<i64>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(None) = row.try_get::<Option<i64>, _>(col_idx) {
                        "NULL".to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<i32>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(None) = row.try_get::<Option<i32>, _>(col_idx) {
                        "NULL".to_string()
                    } else {
                        // Fallback to string
                        match row.try_get::<Option<String>, _>(col_idx) {
                            Ok(Some(val)) => val,
                            Ok(None) => "NULL".to_string(),
                            Err(_) => format!("Error reading INTEGER from column {}", column_name),
                        }
                    }
                }
                // SQLite REAL type
                "REAL" => {
                    if let Ok(Some(val)) = row.try_get::<Option<f64>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(None) = row.try_get::<Option<f64>, _>(col_idx) {
                        "NULL".to_string()
                    } else {
                        // Fallback to string
                        match row.try_get::<Option<String>, _>(col_idx) {
                            Ok(Some(val)) => val,
                            Ok(None) => "NULL".to_string(),
                            Err(_) => format!("Error reading REAL from column {}", column_name),
                        }
                    }
                }
                // SQLite TEXT type
                "TEXT" => match row.try_get::<Option<String>, _>(col_idx) {
                    Ok(Some(val)) => val,
                    Ok(None) => "NULL".to_string(),
                    Err(_) => format!("Error reading TEXT from column {}", column_name),
                },
                // SQLite BLOB type
                "BLOB" => {
                    match row.try_get::<Option<Vec<u8>>, _>(col_idx) {
                        Ok(Some(val)) => format!("<BLOB {} bytes>", val.len()),
                        Ok(None) => "NULL".to_string(),
                        Err(_) => {
                            // Try as string fallback
                            match row.try_get::<Option<String>, _>(col_idx) {
                                Ok(Some(val)) => val,
                                Ok(None) => "NULL".to_string(),
                                Err(_) => format!("Error reading BLOB from column {}", column_name),
                            }
                        }
                    }
                }
                // SQLite NUMERIC/DECIMAL (stored as TEXT, INTEGER, or REAL)
                "NUMERIC" | "DECIMAL" => {
                    // Try as number first, then string
                    if let Ok(Some(val)) = row.try_get::<Option<f64>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<i64>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<String>, _>(col_idx) {
                        val
                    } else if let Ok(None) = row.try_get::<Option<String>, _>(col_idx) {
                        "NULL".to_string()
                    } else {
                        format!("Error reading NUMERIC from column {}", column_name)
                    }
                }
                // Boolean type (stored as INTEGER 0/1)
                "BOOLEAN" => {
                    if let Ok(Some(val)) = row.try_get::<Option<bool>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(None) = row.try_get::<Option<bool>, _>(col_idx) {
                        "NULL".to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<i64>, _>(col_idx) {
                        // Convert 0/1 to boolean
                        match val {
                            0 => "false".to_string(),
                            1 => "true".to_string(),
                            _ => val.to_string(),
                        }
                    } else {
                        // Fallback to string
                        match row.try_get::<Option<String>, _>(col_idx) {
                            Ok(Some(val)) => val,
                            Ok(None) => "NULL".to_string(),
                            Err(_) => format!("Error reading BOOLEAN from column {}", column_name),
                        }
                    }
                }
                // Date and time types in SQLite (stored as TEXT, REAL, or INTEGER)
                "DATE" | "DATETIME" | "TIMESTAMP" => {
                    // SQLite doesn't have native date types, try string first
                    match row.try_get::<Option<String>, _>(col_idx) {
                        Ok(Some(val)) => val,
                        Ok(None) => "NULL".to_string(),
                        Err(_) => {
                            // Try as integer (Unix timestamp)
                            if let Ok(Some(val)) = row.try_get::<Option<i64>, _>(col_idx) {
                                val.to_string()
                            } else {
                                format!("Error reading DATE/TIME from column {}", column_name)
                            }
                        }
                    }
                }
                // Default case: try different types in order of preference
                _ => {
                    // Try string first
                    if let Ok(Some(val)) = row.try_get::<Option<String>, _>(col_idx) {
                        val
                    } else if let Ok(None) = row.try_get::<Option<String>, _>(col_idx) {
                        "NULL".to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<i64>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<f64>, _>(col_idx) {
                        val.to_string()
                    } else if let Ok(Some(val)) = row.try_get::<Option<bool>, _>(col_idx) {
                        val.to_string()
                    } else {
                        format!("Unsupported type '{}' in column {}", type_name, column_name)
                    }
                }
            };

            row_data.push(value_str);
        }
        table_data.push(row_data);
    }

    table_data
}

pub(crate) fn load_sqlite_structure(
    connection_id: i64,
    _connection: &models::structs::ConnectionConfig,
    node: &mut models::structs::TreeNode,
) {
    // Create basic structure for SQLite
    let mut main_children = Vec::new();

    // Tables folder
    let mut tables_folder =
        models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
    tables_folder.connection_id = Some(connection_id);
    tables_folder.database_name = Some("main".to_string());
    tables_folder.is_loaded = false;

    // Add a loading indicator
    let loading_node = models::structs::TreeNode::new(
        "Loading tables...".to_string(),
        models::enums::NodeType::Table,
    );
    tables_folder.children.push(loading_node);

    main_children.push(tables_folder);

    // Views folder
    let mut views_folder =
        models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
    views_folder.connection_id = Some(connection_id);
    views_folder.database_name = Some("main".to_string());
    views_folder.is_loaded = false;
    main_children.push(views_folder);

    node.children = main_children;
}

pub(crate) fn fetch_tables_from_sqlite_connection(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    table_type: &str,
) -> Option<Vec<String>> {
    // Create a new runtime for the database query
    let rt = tokio::runtime::Runtime::new().ok()?;

    rt.block_on(async {
       // Get or create connection pool
       let pool = connection::get_or_create_connection_pool(tabular, connection_id).await?;

       match pool {
              models::enums::DatabasePool::SQLite(sqlite_pool) => {
              let query = match table_type {
                     "table" => "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                     "view" => "SELECT name FROM sqlite_master WHERE type='view'",
                     _ => {
                     debug!("Unsupported table type for SQLite: {}", table_type);
                     return None;
                     }
              };

              let result = sqlx::query_as::<_, (String,)>(query)
                     .fetch_all(sqlite_pool.as_ref())
                     .await;

              match result {
                     Ok(rows) => {
                     let items: Vec<String> = rows.into_iter().map(|(name,)| name).collect();
                     Some(items)
                     },
                     Err(e) => {
                     debug!("Error querying SQLite {} from database: {}", table_type, e);
                     None
                     }
              }
              },
              _ => {
              debug!("Wrong pool type for SQLite connection");
              None
              }
       }
       })
}
