use crate::{
       connection, driver_mysql, driver_postgres, driver_redis, driver_sqlite, driver_mssql, models, modules, window_egui::{self, Tabular}
};
use futures_util::TryStreamExt; // for MSSQL try_next
use eframe::egui;
use sqlx::{mysql::MySqlPoolOptions, postgres::PgPoolOptions, sqlite::SqlitePoolOptions, Column, Row, SqlitePool};
use sqlx::mysql::MySqlConnection;
use sqlx::Connection; // for MySqlConnection::connect
use std::sync::Arc;
use redis::{Client, aio::ConnectionManager};
use mongodb::{Client as MongoClient, bson::doc};
use log::{debug};

// Helper function to add auto LIMIT if not present
pub fn add_auto_limit_if_needed(query: &str, db_type: &models::enums::DatabaseType) -> String {
    let trimmed_query = query.trim();
    
    // Don't add LIMIT if the entire query already has LIMIT/TOP
    let upper_query = trimmed_query.to_uppercase();
    if upper_query.contains("LIMIT") || upper_query.contains(" TOP ") {
        return query.to_string();
    }
    
    // Skip if it's a single utility query (but allow multi-statement with USE + SELECT)
    if !upper_query.contains("SELECT") {
        return query.to_string();
    }
    
    // Split by semicolon to handle multiple statements
    let statements: Vec<&str> = trimmed_query.split(';').collect();
    let mut result_statements = Vec::new();
    
    for statement in statements {
        let trimmed_stmt = statement.trim();
        if trimmed_stmt.is_empty() {
            continue;
        }
        
        let upper_stmt = trimmed_stmt.to_uppercase();
        
        // Skip utility statements (SHOW, DESCRIBE, PRAGMA, USE, etc.)
        if upper_stmt.starts_with("SHOW ") ||
           upper_stmt.starts_with("DESCRIBE ") ||
           upper_stmt.starts_with("EXPLAIN ") ||
           upper_stmt.starts_with("PRAGMA ") ||
           upper_stmt.starts_with("USE ") ||
           upper_stmt.starts_with("SET ") ||
           upper_stmt.starts_with("CREATE ") ||
           upper_stmt.starts_with("DROP ") ||
           upper_stmt.starts_with("ALTER ") ||
           upper_stmt.starts_with("INSERT ") ||
           upper_stmt.starts_with("UPDATE ") ||
           upper_stmt.starts_with("DELETE ") {
            result_statements.push(trimmed_stmt.to_string());
            continue;
        }
        
       //  // Only add LIMIT to SELECT statements that don't already have LIMIT/TOP
       //  if upper_stmt.starts_with("SELECT") && 
       //     !upper_stmt.contains("LIMIT") && 
       //     !upper_stmt.contains(" TOP ") {
            
       //      match db_type {
       //          models::enums::DatabaseType::MSSQL => {
       //              // For MSSQL, we need to add TOP after SELECT
       //              // Handle both "SELECT" and "SELECT DISTINCT" cases
       //              if upper_stmt.starts_with("SELECT DISTINCT") {
       //                  let modified = trimmed_stmt.replace("SELECT DISTINCT", "SELECT DISTINCT TOP 1000");
       //                  result_statements.push(modified);
       //              } else {
       //                  let modified = trimmed_stmt.replace("SELECT", "SELECT TOP 1000");
       //                  result_statements.push(modified);
       //              }
       //          }
       //          _ => {
       //              // For MySQL, PostgreSQL, SQLite - add LIMIT at the end
       //              result_statements.push(format!("{} LIMIT 1000", trimmed_stmt));
       //          }
       //      }
       //  } else {
       //      result_statements.push(trimmed_stmt.to_string());
       //  }
       result_statements.push(trimmed_stmt.to_string());
    }
    
    result_statements.join(";\n")
}



pub(crate) fn render_connection_selector(tabular: &mut Tabular, ctx: &egui::Context) {
       if tabular.show_connection_selector {

       let mut open = true;
              
       egui::Window::new("Select Connection to Execute Query")
              .collapsible(false)
              .resizable(true)
              .default_width(400.0)
              .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
              .open(&mut open)
              .show(ctx, |ui| {

              ui.add_space(5.0);
              
              egui::ScrollArea::vertical()
              .show(ui, |ui| {
              let mut selected_connection = None;

              // sort tabular.connections by folder and name
              tabular.connections.sort_by(|a, b| {
                     a.folder.cmp(&b.folder).then_with(|| a.name.cmp(&b.name))
              });
              
              for connection in &tabular.connections {
                     let mut sfolder = connection.folder.as_deref().unwrap_or("");
                     if sfolder.is_empty() {
                            sfolder = "Default";
                     }
                     let connection_text = format!("{} / {} / {}", 
                            sfolder,
                            match connection.connection_type {
                            models::enums::DatabaseType::MySQL => "MySQL",
                            models::enums::DatabaseType::MSSQL => "MSSQL",
                            models::enums::DatabaseType::PostgreSQL => "PostgreSQL",
                            models::enums::DatabaseType::SQLite => "SQLite",
                            models::enums::DatabaseType::Redis => "Redis",
                            models::enums::DatabaseType::MongoDB => "MongoDB",
                            },
                            connection.name
                     );
                                          
                            // Custom button with red fill on hover
                            let button = egui::Button::new(&connection_text);
                            let response = ui.add_sized([ui.available_width(), 32.0], button);

                            if response.hovered() {
                                   let rect = response.rect;
                                   let visuals = ui.style().visuals.clone();
                                   let fill_color = egui::Color32::RED; // Red
                                   ui.painter().rect_filled(rect, visuals.widgets.inactive.corner_radius, fill_color);
                                   // Repaint the text over the fill
                                   ui.painter().text(
                                   rect.center(),
                                   egui::Align2::CENTER_CENTER,
                                   &connection_text,
                                   egui::TextStyle::Button.resolve(ui.style()),
                                   visuals.text_color(),
                                   );
                            }

                            if response.clicked() {
                                   if let Some(connection_id) = connection.id {
                                   selected_connection = Some(connection_id);
                                   }
                            }
                         ui.add_space(7.0);

              }
              
              // Handle selection outside the loop to avoid borrowing issues
              if let Some(connection_id) = selected_connection {
                     // Set active connection
                     tabular.current_connection_id = Some(connection_id);
                     
                     // Find the selected connection to get its default database
                     let default_database = if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(connection_id)) {
                            let connection_name = connection.name.clone();
                            let database = if !connection.database.is_empty() {
                                   Some(connection.database.clone())
                            } else {
                                   None
                            };
                            
                            debug!("Connection selector: Set connection '{}' and database '{}' for active tab", 
                                   connection_name, 
                                   database.as_deref().unwrap_or("None"));
                            
                            database
                     } else {
                            None
                     };
                     
                     // Set connection and default database in bottom combobox for active tab
                     tabular.set_active_tab_connection_with_database(Some(connection_id), default_database);
                     
                     if tabular.auto_execute_after_connection {
                            // Execute the query immediately
                            let query = tabular.pending_query.clone();
                            if let Some((headers, data)) = execute_query_with_connection(tabular, connection_id, query) {
                            tabular.current_table_headers = headers;
                            
                            // Use pagination for query results
                            tabular.update_pagination_data(data);
                            
                            if tabular.total_rows == 0 {
                                   tabular.current_table_name = "Query executed successfully (no results)".to_string();
                            } else {
                                   tabular.current_table_name = format!("Query Results ({} total rows, showing page {} of {})", 
                                          tabular.total_rows, tabular.current_page + 1, tabular.get_total_pages());
                            }
                            } else {
                            tabular.current_table_name = "Query execution failed".to_string();
                            tabular.current_table_headers.clear();
                            tabular.current_table_data.clear();
                            tabular.all_table_data.clear();
                            tabular.total_rows = 0;
                            }
                            // Mark active tab as having executed a query
                            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                                   tab.has_executed_query = true;
                            }
                     }
                     
                     tabular.show_connection_selector = false;
                     tabular.pending_query.clear();
                     tabular.auto_execute_after_connection = false;
              }
              });

              });

              // Handle close button click
              if !open {
                     tabular.show_connection_selector = false;
                     tabular.pending_query.clear();
                     tabular.auto_execute_after_connection = false;
              }
              
       }
}


pub(crate) fn execute_query_with_connection(tabular: &mut Tabular, connection_id: i64, query: String) -> Option<(Vec<String>, Vec<Vec<String>>)> {
       debug!("Query execution requested for connection {} with query: {}", connection_id, query);
       
          if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(connection_id)).cloned() {
          // Determine selected database from active tab (if any)
          let selected_db = tabular.query_tabs.get(tabular.active_tab_index)
                 .and_then(|t| t.database_name.clone())
                 .filter(|s| !s.is_empty());

          // Auto-prepend USE for MSSQL/MySQL if not already present
          let mut final_query = query.clone();
          if let Some(db_name) = selected_db {
                 match connection.connection_type {
                        models::enums::DatabaseType::MSSQL => {
                               let upper = final_query.to_uppercase();
                               if !upper.starts_with("USE ") {
                                      final_query = format!("USE [{}];\n{}", db_name, final_query);
                               }
                        }
                        models::enums::DatabaseType::MySQL => {
                               let upper = final_query.to_uppercase();
                               if !upper.starts_with("USE ") {
                                      final_query = format!("USE `{}`;\n{}", db_name, final_query);
                               }
                        }
                        _ => {}
                 }
          }

          // Add auto LIMIT 1000 if no LIMIT is present in SELECT queries
          let original_query = final_query.clone();
          final_query = add_auto_limit_if_needed(&final_query, &connection.connection_type);
          
          if original_query != final_query {
              debug!("Auto LIMIT applied. Original: {}", original_query);
              debug!("Modified: {}", final_query);
          }

          execute_table_query_sync(tabular, connection_id, &connection, &final_query)
       } else {
       debug!("Connection not found for ID: {}", connection_id);
       None
       }
}

pub(crate) fn execute_table_query_sync(tabular: &mut Tabular, connection_id: i64, connection: &models::structs::ConnectionConfig, query: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
         debug!("Executing query synchronously: {}", query);

         // Use the shared runtime from tabular instead of creating a new one
         let runtime = match &tabular.runtime {
                Some(rt) => rt.clone(),
                None => {
                       debug!("No runtime available, creating temporary one");
                       match tokio::runtime::Runtime::new() {
                              Ok(rt) => Arc::new(rt),
                              Err(e) => {
                                     debug!("Failed to create runtime: {}", e);
                                     return None;
                              }
                       }
                }
         };

         runtime.block_on(async {
                match get_or_create_connection_pool(tabular, connection_id).await {
                       Some(pool) => {
                              match pool {
                                     models::enums::DatabasePool::MySQL(_mysql_pool) => {
                                            debug!("Executing MySQL query: {}", query);

                                            // Split into statements
                                            let statements: Vec<&str> = query
                                                   .split(';')
                                                   .map(|s| s.trim())
                                                   .filter(|s| !s.is_empty())
                                                   .collect();
                                            debug!("Found {} SQL statements to execute", statements.len());

                                            let mut final_headers = Vec::new();
                                            let mut final_data = Vec::new();

                                            let mut attempts = 0;
                                            let max_attempts = 3;
                                            while attempts < max_attempts {
                                                   attempts += 1;
                                                   let mut execution_success = true;
                                                   let mut error_message = String::new();

                                                   // Open a dedicated connection; we'll reconnect on USE to switch DB
                                                   let encoded_username = modules::url_encode(&connection.username);
                                                   let encoded_password = modules::url_encode(&connection.password);
                                                   let dsn = format!(
                                                          "mysql://{}:{}@{}:{}/{}",
                                                          encoded_username, encoded_password, connection.host, connection.port, connection.database
                                                   );
                                                   let mut conn = match MySqlConnection::connect(&dsn).await {
                                                          Ok(c) => c,
                                                          Err(e) => {
                                                                 error_message = e.to_string();
                                                                 debug!("Failed to open MySQL connection: {}", error_message);
                                                                 if attempts >= max_attempts { break; } else { continue; }
                                                          }
                                                   };

                                                   for (i, statement) in statements.iter().enumerate() {
                                                          let trimmed = statement.trim();
                                                          if trimmed.is_empty() { continue; }
                                                          let upper = trimmed.to_uppercase();

                                                          if upper.starts_with("USE ") {
                                                                 // Parse target database and reconnect instead of executing USE (not supported in prepared protocol)
                                                                 let db_part = trimmed[3..].trim();
                                                                 let db_name = db_part
                                                                        .trim_matches('`')
                                                                        .trim_matches('\"')
                                                                        .trim_matches('[')
                                                                        .trim_matches(']')
                                                                        .trim();
                                                                 let new_dsn = format!(
                                                                        "mysql://{}:{}@{}:{}/{}",
                                                                        encoded_username, encoded_password, connection.host, connection.port, db_name
                                                                 );
                                                                 match MySqlConnection::connect(&new_dsn).await {
                                                                        Ok(new_conn) => {
                                                                               debug!("Switched MySQL database by reconnecting to '{}'.", db_name);
                                                                               conn = new_conn;
                                                                        }
                                                                        Err(e) => {
                                                                               error_message = format!("USE failed (reconnect): {}", e);
                                                                               break;
                                                                        }
                                                                 }
                                                                 continue;
                                                          }

                                                          match sqlx::query(trimmed).fetch_all(&mut conn).await {
                                                                 Ok(rows) => {
                                                                        if i == statements.len() - 1 {
                                                                               // Get headers from metadata, even if no rows
                                                                               if !rows.is_empty() {
                                                                                      final_headers = rows[0]
                                                                                             .columns()
                                                                                             .iter()
                                                                                             .map(|c| c.name().to_string())
                                                                                             .collect();
                                                                                      final_data = driver_mysql::convert_mysql_rows_to_table_data(rows);
                                                                               } else {
                                                                                      // Query executed successfully but returned no rows
                                                                                      // For MySQL, try to get column info using DESCRIBE if it's a table query
                                                                                      if trimmed.to_uppercase().contains("FROM") {
                                                                                             // Extract table name for DESCRIBE
                                                                                             let words: Vec<&str> = trimmed.split_whitespace().collect();
                                                                                             if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM") {
                                                                                                    if let Some(table_name) = words.get(from_idx + 1) {
                                                                                                           let describe_query = format!("DESCRIBE {}", table_name);
                                                                                                           match sqlx::query(&describe_query).fetch_all(&mut conn).await {
                                                                                                                  Ok(desc_rows) => {
                                                                                                                         if !desc_rows.is_empty() {
                                                                                                                                // For DESCRIBE, the first column contains field names
                                                                                                                                final_headers = desc_rows.iter().map(|row| {
                                                                                                                                       match row.try_get::<String, _>(0) {
                                                                                                                                              Ok(field_name) => field_name,
                                                                                                                                              Err(_) => "Field".to_string(),
                                                                                                                                       }
                                                                                                                                }).collect();
                                                                                                                         }
                                                                                                                  }
                                                                                                                  Err(_) => {
                                                                                                                         // DESCRIBE failed, try LIMIT 0 as fallback
                                                                                                                         let info_query = format!("{} LIMIT 0", trimmed);
                                                                                                                         match sqlx::query(&info_query).fetch_all(&mut conn).await {
                                                                                                                                Ok(info_rows) => {
                                                                                                                                       if !info_rows.is_empty() {
                                                                                                                                              final_headers = info_rows[0]
                                                                                                                                                     .columns()
                                                                                                                                                     .iter()
                                                                                                                                                     .map(|c| c.name().to_string())
                                                                                                                                                     .collect();
                                                                                                                                       }
                                                                                                                                }
                                                                                                                                Err(_) => {
                                                                                                                                       // Both methods failed
                                                                                                                                       final_headers = Vec::new();
                                                                                                                                }
                                                                                                                         }
                                                                                                                  }
                                                                                                           }
                                                                                                    }
                                                                                             }
                                                                                      } else {
                                                                                             // Non-table query, just return empty result
                                                                                             final_headers = Vec::new();
                                                                                      }
                                                                                      final_data = Vec::new(); // Empty data but possibly with headers
                                                                               }
                                                                        }
                                                                 }
                                                                 Err(e) => {
                                                                        error_message = e.to_string();
                                                                        execution_success = false;
                                                                        break;
                                                                 }
                                                          }
                                                   }

                                                   if execution_success {
                                                          return Some((final_headers, final_data));
                                                   } else {
                                                          debug!("MySQL query failed on attempt {}: {}", attempts, error_message);
                                                          if (error_message.contains("timed out") || error_message.contains("pool")) && attempts < max_attempts {
                                                                 tabular.connection_pools.remove(&connection_id);
                                                                 continue;
                                                          }
                                                          if attempts >= max_attempts {
                                                                 return Some((
                                                                        vec!["Error".to_string()],
                                                                        vec![vec![format!("Query error: {}", error_message)]]
                                                                 ));
                                                          }
                                                   }
                                            }

                                            Some((
                                                   vec!["Error".to_string()],
                                                   vec![vec!["Failed to execute query after multiple attempts".to_string()]]
                                            ))
                                     }
                                     models::enums::DatabasePool::PostgreSQL(pg_pool) => {
                                            debug!("Executing PostgreSQL query: {}", query);
                                            let statements: Vec<&str> = query
                                                   .split(';')
                                                   .map(|s| s.trim())
                                                   .filter(|s| !s.is_empty())
                                                   .collect();
                                            debug!("Found {} SQL statements to execute", statements.len());

                                            let mut final_headers = Vec::new();
                                            let mut final_data = Vec::new();

                                            for (i, statement) in statements.iter().enumerate() {
                                                   match sqlx::query(statement).fetch_all(pg_pool.as_ref()).await {
                                                          Ok(rows) => {
                                                                 if i == statements.len() - 1 {
                                                                        // For the last statement, try to get headers even if no rows
                                                                        if !rows.is_empty() {
                                                                               final_headers = rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                               final_data = rows.iter().map(|row| {
                                                                                      (0..row.len()).map(|j| match row.try_get::<Option<String>, _>(j) {
                                                                                             Ok(Some(v)) => v,
                                                                                             Ok(None) => "NULL".to_string(),
                                                                                             Err(_) => "Error".to_string(),
                                                                                      }).collect()
                                                                               }).collect();
                                                                        } else {
                                                                               // Query executed successfully but returned no rows
                                                                               // For PostgreSQL, try to get column info from information_schema
                                                                               if statement.to_uppercase().contains("FROM") {
                                                                                      // Extract table name for information_schema query
                                                                                      let words: Vec<&str> = statement.split_whitespace().collect();
                                                                                      if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM") {
                                                                                             if let Some(table_name) = words.get(from_idx + 1) {
                                                                                                    let clean_table = table_name.trim_matches('"').trim_matches('`');
                                                                                                    let info_query = format!(
                                                                                                           "SELECT column_name FROM information_schema.columns WHERE table_name = '{}' ORDER BY ordinal_position",
                                                                                                           clean_table
                                                                                                    );
                                                                                                    match sqlx::query(&info_query).fetch_all(pg_pool.as_ref()).await {
                                                                                                           Ok(info_rows) => {
                                                                                                                  final_headers = info_rows.iter().map(|row| {
                                                                                                                         match row.try_get::<String, _>(0) {
                                                                                                                                Ok(col_name) => col_name,
                                                                                                                                Err(_) => "Column".to_string(),
                                                                                                                         }
                                                                                                                  }).collect();
                                                                                                           }
                                                                                                           Err(_) => {
                                                                                                                  // information_schema failed, try LIMIT 0 as fallback
                                                                                                                  let limit_query = format!("{} LIMIT 0", statement);
                                                                                                                  match sqlx::query(&limit_query).fetch_all(pg_pool.as_ref()).await {
                                                                                                                         Ok(limit_rows) => {
                                                                                                                                if !limit_rows.is_empty() {
                                                                                                                                       final_headers = limit_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                                                                                }
                                                                                                                         }
                                                                                                                         Err(_) => {
                                                                                                                                // Both methods failed
                                                                                                                                final_headers = Vec::new();
                                                                                                                         }
                                                                                                                  }
                                                                                                           }
                                                                                                    }
                                                                                             }
                                                                                      }
                                                                               } else {
                                                                                      // Non-table query, just return empty result
                                                                                      final_headers = Vec::new();
                                                                               }
                                                                               final_data = Vec::new(); // Empty data but possibly with headers
                                                                        }
                                                                 }
                                                          }
                                                          Err(e) => {
                                                                 return Some((vec!["Error".to_string()], vec![vec![format!("Query error: {}", e)]]));
                                                          }
                                                   }
                                            }

                                            Some((final_headers, final_data))
                                     }
                                     models::enums::DatabasePool::SQLite(sqlite_pool) => {
                                            debug!("Executing SQLite query: {}", query);
                                            let statements: Vec<&str> = query
                                                   .split(';')
                                                   .map(|s| s.trim())
                                                   .filter(|s| !s.is_empty())
                                                   .collect();
                                            debug!("Found {} SQL statements to execute", statements.len());

                                            let mut final_headers = Vec::new();
                                            let mut final_data = Vec::new();

                                            for (i, statement) in statements.iter().enumerate() {
                                                   match sqlx::query(statement).fetch_all(sqlite_pool.as_ref()).await {
                                                          Ok(rows) => {
                                                                 if i == statements.len() - 1 {
                                                                        // For the last statement, try to get headers even if no rows
                                                                        if !rows.is_empty() {
                                                                               final_headers = rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                               final_data = driver_sqlite::convert_sqlite_rows_to_table_data(rows);
                                                                        } else {
                                                                               // Query executed successfully but returned no rows
                                                                               // For SQLite, try to get column info using PRAGMA table_info
                                                                               if statement.to_uppercase().contains("FROM") {
                                                                                      // Extract table name for PRAGMA table_info
                                                                                      let words: Vec<&str> = statement.split_whitespace().collect();
                                                                                      if let Some(from_idx) = words.iter().position(|&w| w.to_uppercase() == "FROM") {
                                                                                             if let Some(table_name) = words.get(from_idx + 1) {
                                                                                                    let clean_table = table_name.trim_matches('"').trim_matches('`').trim_matches('[').trim_matches(']');
                                                                                                    let pragma_query = format!("PRAGMA table_info({})", clean_table);
                                                                                                    match sqlx::query(&pragma_query).fetch_all(sqlite_pool.as_ref()).await {
                                                                                                           Ok(pragma_rows) => {
                                                                                                                  final_headers = pragma_rows.iter().map(|row| {
                                                                                                                         // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
                                                                                                                         // We want the name column (index 1)
                                                                                                                         match row.try_get::<String, _>(1) {
                                                                                                                                Ok(col_name) => col_name,
                                                                                                                                Err(_) => "Column".to_string(),
                                                                                                                         }
                                                                                                                  }).collect();
                                                                                                           }
                                                                                                           Err(_) => {
                                                                                                                  // PRAGMA failed, try LIMIT 0 as fallback
                                                                                                                  let limit_query = format!("{} LIMIT 0", statement);
                                                                                                                  match sqlx::query(&limit_query).fetch_all(sqlite_pool.as_ref()).await {
                                                                                                                         Ok(limit_rows) => {
                                                                                                                                if !limit_rows.is_empty() {
                                                                                                                                       final_headers = limit_rows[0].columns().iter().map(|c| c.name().to_string()).collect();
                                                                                                                                }
                                                                                                                         }
                                                                                                                         Err(_) => {
                                                                                                                                // Both methods failed
                                                                                                                                final_headers = Vec::new();
                                                                                                                         }
                                                                                                                  }
                                                                                                           }
                                                                                                    }
                                                                                             }
                                                                                      }
                                                                               } else {
                                                                                      // Non-table query, just return empty result
                                                                                      final_headers = Vec::new();
                                                                               }
                                                                               final_data = Vec::new(); // Empty data but possibly with headers
                                                                        }
                                                                 }
                                                          }
                                                          Err(e) => {
                                                                 return Some((vec!["Error".to_string()], vec![vec![format!("Query error: {}", e)]]));
                                                          }
                                                   }
                                            }

                                            Some((final_headers, final_data))
                                     }
                                     models::enums::DatabasePool::Redis(redis_manager) => {
                                            debug!("Executing Redis command: {}", query);
                                            let mut connection = redis_manager.as_ref().clone();
                                            use redis::AsyncCommands;

                                            let parts: Vec<&str> = query.split_whitespace().collect();
                                            if parts.is_empty() {
                                                   return Some((vec!["Error".to_string()], vec![vec!["Empty command".to_string()]]));
                                            }

                                            match parts[0].to_uppercase().as_str() {
                                                   "GET" => {
                                                          if parts.len() != 2 {
                                                                 return Some((vec!["Error".to_string()], vec![vec!["GET requires exactly one key".to_string()]]));
                                                          }
                                                          match connection.get::<&str, Option<String>>(parts[1]).await {
                                                                 Ok(Some(value)) => Some((vec!["Key".to_string(), "Value".to_string()], vec![vec![parts[1].to_string(), value]])),
                                                                 Ok(None) => Some((vec!["Key".to_string(), "Value".to_string()], vec![vec![parts[1].to_string(), "NULL".to_string()]])),
                                                                 Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Redis GET error: {}", e)]])),
                                                          }
                                                   }
                                                   "KEYS" => {
                                                          if parts.len() != 2 {
                                                                 return Some((vec!["Error".to_string()], vec![vec!["KEYS requires exactly one pattern".to_string()]]));
                                                          }
                                                          match connection.keys::<&str, Vec<String>>(parts[1]).await {
                                                                 Ok(keys) => {
                                                                        let table_data: Vec<Vec<String>> = keys.into_iter().map(|k| vec![k]).collect();
                                                                        Some((vec!["Key".to_string()], table_data))
                                                                 }
                                                                 Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Redis KEYS error: {}", e)]])),
                                                          }
                                                   }
                                                   "SCAN" => {
                                                          if parts.len() < 2 {
                                                                 return Some((vec!["Error".to_string()], vec![vec!["SCAN requires cursor parameter".to_string()]]));
                                                          }
                                                          let cursor = parts[1];
                                                          let mut match_pattern = "*";
                                                          let mut count: i64 = 10;
                                                          let mut i = 2;
                                                          while i < parts.len() {
                                                                 match parts[i].to_uppercase().as_str() {
                                                                        "MATCH" => { if i + 1 < parts.len() { match_pattern = parts[i + 1]; i += 2; } else { return Some((vec!["Error".to_string()], vec![vec!["MATCH requires a pattern".to_string()]])); } }
                                                                        "COUNT" => { if i + 1 < parts.len() { if let Ok(c) = parts[i + 1].parse::<i64>() { count = c; i += 2; } else { return Some((vec!["Error".to_string()], vec![vec!["COUNT must be a number".to_string()]])); } } else { return Some((vec!["Error".to_string()], vec![vec!["COUNT requires a number".to_string()]])); } }
                                                                        _ => { return Some((vec!["Error".to_string()], vec![vec![format!("Unknown SCAN parameter: {}", parts[i])]])); }
                                                                 }
                                                          }

                                                          let mut cmd = redis::cmd("SCAN");
                                                          cmd.arg(cursor);
                                                          if match_pattern != "*" { cmd.arg("MATCH").arg(match_pattern); }
                                                          cmd.arg("COUNT").arg(count);

                                                          match cmd.query_async::<_, (String, Vec<String>)>(&mut connection).await {
                                                                 Ok((next_cursor, keys)) => {
                                                                        let mut table_data = Vec::new();
                                                                        if keys.is_empty() {
                                                                               table_data.push(vec!["Info".to_string(), format!("No keys found matching pattern: {}", match_pattern)]);
                                                                               table_data.push(vec!["Cursor".to_string(), next_cursor.clone()]);
                                                                               table_data.push(vec!["Suggestion".to_string(), "Try different pattern or use 'SCAN 0 COUNT 100' to see all keys".to_string()]);
                                                                               if match_pattern != "*" {
                                                                                      if let Ok((_, sample_keys)) = redis::cmd("SCAN").arg("0").arg("COUNT").arg("10").query_async::<_, (String, Vec<String>)>(&mut connection).await {
                                                                                             if !sample_keys.is_empty() {
                                                                                                    table_data.push(vec!["Sample Keys Found".to_string(), "".to_string()]);
                                                                                                    for (i, key) in sample_keys.iter().take(5).enumerate() { table_data.push(vec![format!("Sample {}", i + 1), key.clone()]); }
                                                                                             }
                                                                                      }
                                                                               }
                                                                        } else {
                                                                               table_data.push(vec!["CURSOR".to_string(), next_cursor]);
                                                                               for key in keys { table_data.push(vec!["KEY".to_string(), key]); }
                                                                        }
                                                                        Some((vec!["Type".to_string(), "Value".to_string()], table_data))
                                                                 }
                                                                 Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Redis SCAN error: {}", e)]])),
                                                          }
                                                   }
                                                   "INFO" => {
                                                          let section = if parts.len() > 1 { parts[1] } else { "default" };
                                                          match redis::cmd("INFO").arg(section).query_async::<_, String>(&mut connection).await {
                                                                 Ok(info_result) => {
                                                                        let mut table_data = Vec::new();
                                                                        for line in info_result.lines() {
                                                                               if line.trim().is_empty() || line.starts_with('#') { continue; }
                                                                               if let Some((key, value)) = line.split_once(':') { table_data.push(vec![key.to_string(), value.to_string()]); }
                                                                        }
                                                                        Some((vec!["Property".to_string(), "Value".to_string()], table_data))
                                                                 }
                                                                 Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Redis INFO error: {}", e)]])),
                                                          }
                                                   }
                                                   "HGETALL" => {
                                                          if parts.len() != 2 { return Some((vec!["Error".to_string()], vec![vec!["HGETALL requires exactly one key".to_string()]])); }
                                                          match redis::cmd("HGETALL").arg(parts[1]).query_async::<_, Vec<String>>(&mut connection).await {
                                                                 Ok(hash_data) => {
                                                                        let mut table_data = Vec::new();
                                                                        for chunk in hash_data.chunks(2) { if chunk.len() == 2 { table_data.push(vec![chunk[0].clone(), chunk[1].clone()]); } }
                                                                        if table_data.is_empty() { table_data.push(vec!["No data".to_string(), "Hash is empty or key does not exist".to_string()]); }
                                                                        Some((vec!["Field".to_string(), "Value".to_string()], table_data))
                                                                 }
                                                                 Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Redis HGETALL error: {}", e)]])),
                                                          }
                                                   }
                                                   _ => Some((vec!["Error".to_string()], vec![vec![format!("Unsupported Redis command: {}", parts[0])]])),
                                            }
                                     }
                                     models::enums::DatabasePool::MSSQL(mssql_cfg) => {
                                            debug!("Executing MSSQL query: {}", query);
                                            match driver_mssql::execute_query(mssql_cfg.clone(), query).await {
                                                   Ok((h, d)) => Some((h, d)),
                                                   Err(e) => Some((vec!["Error".to_string()], vec![vec![format!("Query error: {}", e)]])),
                                            }
                                     }
                                     models::enums::DatabasePool::MongoDB(_client) => {
                                            // For now, MongoDB queries are not supported via SQL editor. Provide hint.
                                            Some((vec!["Info".to_string()], vec![vec!["MongoDB query execution is not supported. Use tree to browse collections.".to_string()]]))
                                     }
                              }
                       }
                       None => {
                              debug!("Failed to get connection pool for connection_id: {}", connection_id);
                              Some((vec!["Error".to_string()], vec![vec!["Failed to connect to database".to_string()]]))
                       }
                }
         })
}


// Helper function untuk mendapatkan atau membuat connection pool
pub(crate) async fn get_or_create_connection_pool(tabular: &mut Tabular, connection_id: i64) -> Option<models::enums::DatabasePool> {
       // First check if we already have a cached connection pool for this connection
       if let Some(cached_pool) = tabular.connection_pools.get(&connection_id) {
       debug!("✅ Using cached connection pool for connection {}", connection_id);
       return Some(cached_pool.clone());
       }

       debug!("🔄 Creating new connection pool for connection {}", connection_id);
       
       // If not cached, create a new connection pool
       if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(connection_id)) {
       match connection.connection_type {
              models::enums::DatabaseType::MySQL => {
              let encoded_username = modules::url_encode(&connection.username);
              let encoded_password = modules::url_encode(&connection.password);
              let connection_string = format!(
                     "mysql://{}:{}@{}:{}/{}",
                     encoded_username, encoded_password, connection.host, connection.port, connection.database
              );

              // Don't block on ICMP ping (often disabled on Windows firewalls). Attempt direct connect.
              // If you still want diagnostics, you can log ping result without failing the flow:
              // let _ = helpers::ping_host(&connection.host);
              
              // Configure MySQL pool with improved settings for stability
              let pool_result = MySqlPoolOptions::new()
                     .max_connections(20)  // Increase max connections
                     .min_connections(1)   // Start with fewer minimum connections
                     .acquire_timeout(std::time::Duration::from_secs(60))  // Longer timeout for complex queries
                     .idle_timeout(std::time::Duration::from_secs(300))    // 5 minute idle timeout
                     .max_lifetime(std::time::Duration::from_secs(1800))   // 30 minute max lifetime
                     .test_before_acquire(false)  // Disable pre-test for better performance
                     .after_connect(|conn, _| Box::pin(async move {
                         // Set connection-specific settings for better stability
                         let _ = sqlx::query("SET SESSION wait_timeout = 300").execute(&mut *conn).await;
                         let _ = sqlx::query("SET SESSION interactive_timeout = 300").execute(&mut *conn).await;
                         Ok(())
                     }))
                     .connect(&connection_string)
                     .await;
              
              match pool_result {
                     Ok(pool) => {
                     let database_pool = models::enums::DatabasePool::MySQL(Arc::new(pool));
                     tabular.connection_pools.insert(connection_id, database_pool.clone());
                     debug!("✅ Created MySQL connection pool for connection {}", connection_id);
                     Some(database_pool)
                     },
                     Err(e) => {
                     debug!("❌ Failed to create MySQL pool for connection {}: {}", connection_id, e);
                     None
                     }
              }
              },
              models::enums::DatabaseType::PostgreSQL => {
              let connection_string = format!(
                     "postgresql://{}:{}@{}:{}/{}",
                     connection.username, connection.password, connection.host, connection.port, connection.database
              );
                                   
              // Configure PostgreSQL pool with improved settings
              let pool_result = PgPoolOptions::new()
                     .max_connections(15)  // Increase max connections
                     .min_connections(1)   // Start with fewer minimum connections  
                     .acquire_timeout(std::time::Duration::from_secs(60))  // Longer timeout
                     .idle_timeout(std::time::Duration::from_secs(300))    // 5 minute idle timeout
                     .max_lifetime(std::time::Duration::from_secs(1800))   // 30 minute max lifetime
                     .test_before_acquire(false)  // Disable pre-test for better performance
                     .connect(&connection_string)
                     .await;
              
              match pool_result {
                     Ok(pool) => {
                     let database_pool = models::enums::DatabasePool::PostgreSQL(Arc::new(pool));
                     tabular.connection_pools.insert(connection_id, database_pool.clone());
                     Some(database_pool)
                     },
                     Err(e) => {
                     debug!("Failed to create PostgreSQL pool: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabaseType::SQLite => {
              let connection_string = format!("sqlite:{}", connection.host);
              
              // Configure SQLite pool with improved settings
              let pool_result = SqlitePoolOptions::new()
                     .max_connections(5)   // SQLite doesn't need many connections
                     .min_connections(1)   // Start with one connection
                     .acquire_timeout(std::time::Duration::from_secs(60))  // Longer timeout
                     .idle_timeout(std::time::Duration::from_secs(300))    // 5 minute idle timeout
                     .max_lifetime(std::time::Duration::from_secs(1800))   // 30 minute max lifetime
                     .test_before_acquire(false)  // Disable pre-test for better performance
                     .connect(&connection_string)
                     .await;
              
              match pool_result {
                     Ok(pool) => {
                     let database_pool = models::enums::DatabasePool::SQLite(Arc::new(pool));
                     tabular.connection_pools.insert(connection_id, database_pool.clone());
                     Some(database_pool)
                     },
                     Err(e) => {
                     debug!("Failed to create SQLite pool: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabaseType::Redis => {
              let connection_string = if connection.password.is_empty() {
                     format!("redis://{}:{}", connection.host, connection.port)
              } else {
                     format!("redis://{}:{}@{}:{}", connection.username, connection.password, connection.host, connection.port)
              };
              
              debug!("Creating new Redis connection manager for: {}", connection.name);
              match Client::open(connection_string) {
                     Ok(client) => {
                     match ConnectionManager::new(client).await {
                            Ok(manager) => {
                                   let database_pool = models::enums::DatabasePool::Redis(Arc::new(manager));
                                   tabular.connection_pools.insert(connection_id, database_pool.clone());
                                   Some(database_pool)
                            },
                            Err(e) => {
                                   debug!("Failed to create Redis connection manager: {}", e);
                                   None
                            }
                     }
                     }
                     Err(e) => {
                     debug!("Failed to create Redis client: {}", e);
                     None
                     }
              }
              }
              models::enums::DatabaseType::MongoDB => {
              // Build MongoDB connection string
              let uri = if connection.username.is_empty() {
                     format!("mongodb://{}:{}", connection.host, connection.port)
              } else if connection.password.is_empty() {
                     format!("mongodb://{}@{}:{}", connection.username, connection.host, connection.port)
              } else {
                     let enc_user = modules::url_encode(&connection.username);
                     let enc_pass = modules::url_encode(&connection.password);
                     format!("mongodb://{}:{}@{}:{}", enc_user, enc_pass, connection.host, connection.port)
              };
              debug!("Creating MongoDB client for URI: {}", uri);
              match MongoClient::with_uri_str(uri).await {
                     Ok(client) => {
                            let pool = models::enums::DatabasePool::MongoDB(Arc::new(client));
                            tabular.connection_pools.insert(connection_id, pool.clone());
                            Some(pool)
                     }
                     Err(e) => { debug!("Failed to create MongoDB client: {}", e); None }
              }
              }
              models::enums::DatabaseType::MSSQL => {
                     let cfg = driver_mssql::MssqlConfigWrapper::new(
                            connection.host.clone(),
                            connection.port.clone(),
                            connection.database.clone(),
                            connection.username.clone(),
                            connection.password.clone()
                     );
                     let database_pool = models::enums::DatabasePool::MSSQL(Arc::new(cfg));
                     tabular.connection_pools.insert(connection_id, database_pool.clone());
                     Some(database_pool)
              }
       }
       } else {
       None
       }
}

// Function to cleanup and recreate connection pools
pub(crate) fn cleanup_connection_pool(tabular: &mut Tabular, connection_id: i64) {
       debug!("🧹 Cleaning up connection pool for connection {}", connection_id);
       tabular.connection_pools.remove(&connection_id);
}



pub(crate) async fn refresh_connection_background_async(
       connection_id: i64,
       db_pool: &Option<Arc<SqlitePool>>,
) -> bool {

       debug!("Refreshing connection with ID: {}", connection_id);

       // Get connection from database
       if let Some(cache_pool_arc) = db_pool {
       let connection_result = sqlx::query_as::<_, (i64, String, String, String, String, String, String, String)>(
              "SELECT id, name, host, port, username, password, database_name, connection_type FROM connections WHERE id = ?"
       )
       .bind(connection_id)
       .fetch_optional(cache_pool_arc.as_ref())
       .await;
       
       if let Ok(Some((id, name, host, port, username, password, database_name, connection_type))) = connection_result {
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
                     "MSSQL" => models::enums::DatabaseType::MSSQL,
                     _ => models::enums::DatabaseType::SQLite,
              },
              folder: None, // Will be loaded from database later
              };
              
              // Clear cache
              let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
              .bind(connection_id)
              .execute(cache_pool_arc.as_ref())
              .await;
              
              let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
              .bind(connection_id)
              .execute(cache_pool_arc.as_ref())
              .await;
              
              let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
              .bind(connection_id)
              .execute(cache_pool_arc.as_ref())
              .await;

              // Create new connection pool
              match tokio::time::timeout(
              std::time::Duration::from_secs(30), // 30 second timeout
              create_database_pool(&connection)
              ).await {
              Ok(Some(new_pool)) => {
                     fetch_and_cache_all_data(connection_id, &connection, &new_pool, cache_pool_arc.as_ref()).await
              }
              Ok(None) => {
                     false
              }
              Err(_) => {
                     false
              }
              }
       } else {
              false
       }
       } else {
       false
       }
}

pub(crate) async fn create_database_pool(connection: &models::structs::ConnectionConfig) -> Option<models::enums::DatabasePool> {
       match connection.connection_type {
       models::enums::DatabaseType::MySQL => {
              let encoded_username = modules::url_encode(&connection.username);
              let encoded_password = modules::url_encode(&connection.password);
              let connection_string = format!(
              "mysql://{}:{}@{}:{}/{}",
              encoded_username, encoded_password, connection.host, connection.port, connection.database
              );
              
              match MySqlPoolOptions::new()
              .max_connections(3) // Reduced from 5 to 3
              .min_connections(1)
              .acquire_timeout(std::time::Duration::from_secs(10))
              .idle_timeout(std::time::Duration::from_secs(300))
              .connect(&connection_string)
              .await
              {
              Ok(pool) => {
                     Some(models::enums::DatabasePool::MySQL(Arc::new(pool)))
              }
              Err(_e) => {
                     None
              }
              }
       }
       models::enums::DatabaseType::PostgreSQL => {
              let connection_string = format!(
              "postgresql://{}:{}@{}:{}/{}",
              connection.username, connection.password, connection.host, connection.port, connection.database
              );
              
              match PgPoolOptions::new()
              .max_connections(3)
              .min_connections(1)
              .acquire_timeout(std::time::Duration::from_secs(10))
              .idle_timeout(std::time::Duration::from_secs(300))
              .connect(&connection_string)
              .await
              {
              Ok(pool) => {
                     Some(models::enums::DatabasePool::PostgreSQL(Arc::new(pool)))
              }
              Err(_e) => {
                     None
              }
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
              Ok(pool) => {
                     Some(models::enums::DatabasePool::SQLite(Arc::new(pool)))
              }
              Err(_e) => {
                     None
              }
              }
       }
       models::enums::DatabaseType::Redis => {
              let connection_string = if connection.password.is_empty() {
              format!("redis://{}:{}", connection.host, connection.port)
              } else {
              format!("redis://{}:{}@{}:{}", connection.username, connection.password, connection.host, connection.port)
              };
              
              match Client::open(connection_string) {
              Ok(client) => {
                     match ConnectionManager::new(client).await {
                     Ok(manager) => Some(models::enums::DatabasePool::Redis(Arc::new(manager))),
                     Err(_e) => None,
                     }
              }
              Err(_e) => None,
              }
       }
       models::enums::DatabaseType::MSSQL => {
              let cfg = driver_mssql::MssqlConfigWrapper::new(
                     connection.host.clone(),
                     connection.port.clone(),
                     connection.database.clone(),
                     connection.username.clone(),
                     connection.password.clone(),
              );
              Some(models::enums::DatabasePool::MSSQL(Arc::new(cfg)))
       }
       models::enums::DatabaseType::MongoDB => {
              let uri = if connection.username.is_empty() {
                     format!("mongodb://{}:{}", connection.host, connection.port)
              } else if connection.password.is_empty() {
                     format!("mongodb://{}@{}:{}", connection.username, connection.host, connection.port)
              } else {
                     let enc_user = modules::url_encode(&connection.username);
                     let enc_pass = modules::url_encode(&connection.password);
                     format!("mongodb://{}:{}@{}:{}", enc_user, enc_pass, connection.host, connection.port)
              };
              match MongoClient::with_uri_str(uri).await {
                     Ok(client) => Some(models::enums::DatabasePool::MongoDB(Arc::new(client))),
                     Err(_) => None,
              }
       }
       }
}



async fn fetch_and_cache_all_data(
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
       models::enums::DatabaseType::MSSQL => {
              if let models::enums::DatabasePool::MSSQL(mssql_cfg) = pool {
                     driver_mssql::fetch_mssql_data(connection_id, mssql_cfg.clone(), cache_pool).await
              } else { false }
       }
       models::enums::DatabaseType::MongoDB => {
              if let models::enums::DatabasePool::MongoDB(client) = pool {
                     // Use helper to populate cache
                     crate::driver_mongodb::fetch_mongodb_data(connection_id, client.clone(), cache_pool).await
              } else { false }
       }
       }
}



pub(crate) fn fetch_databases_from_connection(tabular: &mut window_egui::Tabular, connection_id: i64) -> Option<Vec<String>> {
       
       // Find the connection configuration
       let _connection = tabular.connections.iter().find(|c| c.id == Some(connection_id))?.clone();
       
       // Create a new runtime for the database query
       let rt = tokio::runtime::Runtime::new().ok()?;
       
       rt.block_on(async {
       // Get or create connection pool
       let pool = connection::get_or_create_connection_pool(tabular, connection_id).await?;
       
       match pool {
              models::enums::DatabasePool::MySQL(mysql_pool) => {
              // Use INFORMATION_SCHEMA to avoid VARBINARY decode issues from SHOW DATABASES on some setups
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
                            .filter(|db| !["information_schema", "performance_schema", "mysql", "sys"].contains(&db.as_str()))
                            .collect();
                     Some(databases)
                     },
                     Err(e) => {
                     debug!("Error querying MySQL databases via INFORMATION_SCHEMA: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabasePool::PostgreSQL(pg_pool) => {
              let result = sqlx::query_as::<_, (String,)>(
                     "SELECT datname FROM pg_database WHERE datistemplate = false AND datname NOT IN ('postgres', 'template0', 'template1')"
              )
              .fetch_all(pg_pool.as_ref())
              .await;
              
              match result {
                     Ok(rows) => {
                     let databases: Vec<String> = rows.into_iter().map(|(db_name,)| db_name).collect();
                     Some(databases)
                     },
                     Err(e) => {
                     debug!("Error querying PostgreSQL databases: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabasePool::SQLite(sqlite_pool) => {
              // For SQLite, we'll query the actual database for table information
              let result = sqlx::query_as::<_, (String,)>("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                     .fetch_all(sqlite_pool.as_ref())
                     .await;
                     
              match result {
                     Ok(rows) => {
                     let table_count = rows.len();
                     if table_count > 0 {
                            // Since SQLite has tables, return main database
                            Some(vec!["main".to_string()])
                     } else {
                            debug!("No tables found in SQLite database, returning 'main' database anyway");
                            Some(vec!["main".to_string()])
                     }
                     },
                     Err(e) => {
                     debug!("Error querying SQLite tables: {}", e);
                     Some(vec!["main".to_string()]) // Fallback to main
                     }
              }
              },
              models::enums::DatabasePool::Redis(redis_manager) => {
              // For Redis, get actual databases (db0, db1, etc.)
              let mut conn = redis_manager.as_ref().clone();
              
              // Get CONFIG GET databases to determine max database count
              let max_databases = match redis::cmd("CONFIG").arg("GET").arg("databases").query_async::<_, Vec<String>>(&mut conn).await {
                     Ok(config_result) if config_result.len() >= 2 => {
                     config_result[1].parse::<i32>().unwrap_or(16)
                     }
                     _ => 16 // Default fallback
              };
              
              debug!("Redis max databases: {}", max_databases);
              
              // Create list of all Redis databases (db0 to db15 by default)
              let mut databases = Vec::new();
              for db_num in 0..max_databases {
                     let db_name = format!("db{}", db_num);
                     databases.push(db_name);
              }
              
              debug!("Generated Redis databases: {:?}", databases);
              Some(databases)
              }
              models::enums::DatabasePool::MSSQL(ref mssql_cfg) => {
                     // Fetch list of databases from MSSQL server
                     use tokio_util::compat::TokioAsyncWriteCompatExt;
                     use tiberius::{AuthMethod, Config};
                     {
                            let mssql_cfg = mssql_cfg.clone();
                            // Attempt connection with master database to enumerate all
                            let host = mssql_cfg.host.clone();
                            let port = mssql_cfg.port;
                            let user = mssql_cfg.username.clone();
                            let pass = mssql_cfg.password.clone();
                            let rt_res = async move {
                                   let mut config = Config::new();
                                   config.host(host.clone());
                                   config.port(port);
                                   config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                                   config.trust_cert();
                                   // Always use master for listing
                                   config.database("master");
                                   let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                                   tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                                   let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                                   let mut dbs = Vec::new();
                                   let mut stream = client.simple_query("SELECT name FROM sys.databases ORDER BY name").await.map_err(|e| e.to_string())?;
                                   use futures_util::TryStreamExt;
                                   while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? {
                                          if let tiberius::QueryItem::Row(r) = item {
                                                 let name: Option<&str> = r.get(0);
                                                 if let Some(n) = name {
                                                        // Optionally skip system DBs? Keep them for completeness; can filter later.
                                                        dbs.push(n.to_string());
                                                 }
                                          }
                                   }
                                   Ok::<_, String>(dbs)
                            }.await;
                            match rt_res {
                                   Ok(mut list) => {
                                          if list.is_empty() {
                                                 debug!("MSSQL database list is empty; returning current database only");
                                                 Some(vec![mssql_cfg.database.clone()])
                                          } else {
                                                 // Move system DBs (master, model, msdb, tempdb) to end for nicer UX
                                                 let system = ["master", "model", "msdb", "tempdb"]; 
                                                 list.sort();
                                                 let mut user_dbs: Vec<String> = list.iter().filter(|d| !system.contains(&d.as_str())).cloned().collect();
                                                 let mut sys_dbs: Vec<String> = list.into_iter().filter(|d| system.contains(&d.as_str())).collect();
                                                 user_dbs.append(&mut sys_dbs);
                                                 Some(user_dbs)
                                          }
                                   }
                                   Err(e) => {
                                          debug!("Failed to fetch MSSQL databases: {}", e);
                                          // Fallback to default known system DBs so UI still shows something
                                          Some(vec!["master".to_string(), "tempdb".to_string(), "model".to_string(), "msdb".to_string()])
                                   }
                            }
                     }
              }
              models::enums::DatabasePool::MongoDB(client) => {
              match client.list_database_names().await {
                     Ok(dbs) => Some(dbs),
                     Err(e) => { debug!("MongoDB list databases error: {}", e); None }
              }
              }
       }
       })
}




pub(crate) fn fetch_columns_from_database(_connection_id: i64, database_name: &str, table_name: &str, connection: &models::structs::ConnectionConfig) -> Option<Vec<(String, String)>> {
       
       // Create a new runtime for the database query
       let rt = tokio::runtime::Runtime::new().ok()?;
       
       // Clone data to move into async block
       let connection_clone = connection.clone();
       let database_name = database_name.to_string();
       let table_name = table_name.to_string();
       
       rt.block_on(async {
       match connection_clone.connection_type {
              models::enums::DatabaseType::MySQL => {
              // Create MySQL connection
              let encoded_username = modules::url_encode(&connection_clone.username);
              let encoded_password = modules::url_encode(&connection_clone.password);
              let connection_string = format!(
                     "mysql://{}:{}@{}:{}/{}",
                     encoded_username, encoded_password, connection_clone.host, connection_clone.port, database_name
              );
              
              match MySqlPoolOptions::new()
                     .max_connections(1)
                     .acquire_timeout(std::time::Duration::from_secs(10))
                     .connect(&connection_string)
                     .await
              {
                     Ok(pool) => {
                     // Force text decoding to avoid VARBINARY/BLOB issues on some setups
                     let query = "SELECT CONVERT(COLUMN_NAME USING utf8mb4) AS COLUMN_NAME, CONVERT(DATA_TYPE USING utf8mb4) AS DATA_TYPE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION";
                     match sqlx::query_as::<_, (String, String)>(query)
                            .bind(&database_name)
                            .bind(&table_name)
                            .fetch_all(&pool)
                            .await
                     {
                            Ok(rows) => {
                                   let columns: Vec<(String, String)> = rows.into_iter().collect();
                                   Some(columns)
                            },
                            Err(e) => {
                                   debug!("Error querying MySQL columns for table {}: {}", table_name, e);
                                   None
                            }
                     }
                     },
                     Err(e) => {
                     debug!("Error connecting to MySQL database: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabaseType::SQLite => {
              // Create SQLite connection
              let connection_string = format!("sqlite:{}", connection_clone.host);
              
              match SqlitePoolOptions::new()
                     .max_connections(1)
                     .acquire_timeout(std::time::Duration::from_secs(10))
                     .connect(&connection_string)
                     .await
              {
                     Ok(pool) => {
                     // Use dynamic row extraction to avoid issues with NULL dflt_value, and quote table name safely
                     let escaped = table_name.replace("'", "''");
                     let query = format!("PRAGMA table_info('{}')", escaped);
                     match sqlx::query(&query).fetch_all(&pool).await {
                            Ok(rows) => {
                                   use sqlx::Row;
                                   let mut columns: Vec<(String, String)> = Vec::new();
                                   for row in rows {
                                          // Columns in pragma: cid, name, type, notnull, dflt_value, pk
                                          let name: Option<String> = row.try_get("name").ok();
                                          let data_type: Option<String> = row.try_get("type").ok();
                                          if let (Some(n), Some(t)) = (name, data_type) {
                                                 columns.push((n, t));
                                          }
                                   }
                                   Some(columns)
                            }
                            Err(e) => {
                                   debug!("Error querying SQLite columns for table {}: {}", table_name, e);
                                   None
                            }
                     }
                     },
                     Err(e) => {
                     debug!("Error connecting to SQLite database: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabaseType::PostgreSQL => {
              // Create PostgreSQL connection
              let connection_string = format!(
                     "postgresql://{}:{}@{}:{}/{}",
                     connection_clone.username, connection_clone.password, connection_clone.host, connection_clone.port, database_name
              );
              
              match PgPoolOptions::new()
                     .max_connections(1)
                     .acquire_timeout(std::time::Duration::from_secs(10))
                     .connect(&connection_string)
                     .await
              {
                     Ok(pool) => {
                     // Use PostgreSQL-style positional parameters ($1, $2, ...)
                     let query = "SELECT column_name, data_type FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position";
                     match sqlx::query_as::<_, (String, String)>(query)
                            .bind(&table_name)
                            .fetch_all(&pool)
                            .await
                     {
                            Ok(rows) => {
                                   let columns: Vec<(String, String)> = rows.into_iter().collect();
                                   Some(columns)
                            },
                            Err(e) => {
                                   debug!("Error querying PostgreSQL columns for table {}: {}", table_name, e);
                                   None
                            }
                     }
                     },
                     Err(e) => {
                     debug!("Error connecting to PostgreSQL database: {}", e);
                     None
                     }
              }
              },
              models::enums::DatabaseType::Redis => {
              // Redis doesn't have traditional tables/columns
              // Return some generic "columns" for Redis key-value structure
              Some(vec![
                     ("key".to_string(), "String".to_string()),
                     ("value".to_string(), "Any".to_string()),
                     ("type".to_string(), "String".to_string()),
                     ("ttl".to_string(), "Integer".to_string()),
              ])
              }
                       models::enums::DatabaseType::MongoDB => {
                              // Connect directly and sample one document to infer top-level fields
                              let uri = if connection_clone.username.is_empty() {
                                     format!("mongodb://{}:{}", connection_clone.host, connection_clone.port)
                              } else if connection_clone.password.is_empty() {
                                     format!("mongodb://{}@{}:{}", connection_clone.username, connection_clone.host, connection_clone.port)
                              } else {
                                     let enc_user = modules::url_encode(&connection_clone.username);
                                     let enc_pass = modules::url_encode(&connection_clone.password);
                                     format!("mongodb://{}:{}@{}:{}", enc_user, enc_pass, connection_clone.host, connection_clone.port)
                              };
                              match MongoClient::with_uri_str(uri).await {
                                     Ok(client) => {
                                            let coll = client.database(&database_name).collection::<mongodb::bson::Document>(&table_name);
                                            match coll.find(doc!{}).limit(1).await {
                                                   Ok(mut cursor) => {
                                                          if let Some(doc) = cursor.try_next().await.unwrap_or(None) {
                                                                 use mongodb::bson::Bson;
                                                                 let cols: Vec<(String, String)> = doc.into_iter().map(|(k,v)| {
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
                                                                 }).collect();
                                                                 Some(cols)
                                                          } else { None }
                                                   }
                                                   Err(_) => None,
                                            }
                                     }
                                     Err(_) => None,
                              }
                       }
              models::enums::DatabaseType::MSSQL => {
              // Basic column metadata using INFORMATION_SCHEMA
              use tokio_util::compat::TokioAsyncWriteCompatExt;
              use tiberius::{Config, AuthMethod};
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
                     if !db.is_empty() { config.database(db.clone()); }
                     let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                     tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                     let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                     // Parse possible qualified MSSQL names like [schema].[table] or schema.table
                     let parse_qualified = |name: &str| -> (Option<String>, String) {
                            // Handle [schema].[table] or [schema].[table].[extra]
                            if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                                   let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                                   let parts: Vec<&str> = trimmed.split("].[" ).collect();
                                   if parts.len() >= 2 {
                                          return (Some(parts[0].to_string()), parts[1].to_string());
                                   }
                            }
                            // Handle schema.table
                            if let Some((schema, tbl)) = name.split_once('.') {
                                   return (
                                          Some(schema.trim_matches(|c| c == '[' || c == ']').to_string()),
                                          tbl.trim_matches(|c| c == '[' || c == ']').to_string()
                                   );
                            }
                            // Only table
                            (None, name.trim_matches(|c| c == '[' || c == ']').to_string())
                     };

                     let (schema_opt, table_only) = parse_qualified(&table);

                     // Build INFORMATION_SCHEMA query with optional schema filter
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
                     use futures_util::TryStreamExt;
                     while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let tiberius::QueryItem::Row(r) = item { let name: Option<&str> = r.get(0); let dt: Option<&str> = r.get(1); if let (Some(n), Some(d)) = (name, dt) { cols.push((n.to_string(), d.to_string())); } } }
                     Ok::<_, String>(cols)
              }.await;
              match rt_res { Ok(v) => Some(v), Err(e) => { debug!("MSSQL column fetch error: {}", e); None } }
              }
              // MongoDB has been handled above; no additional branch here.
       }
       })
}





pub(crate) fn update_connection_in_database(tabular: &mut window_egui::Tabular, connection: &models::structs::ConnectionConfig) -> bool {
        if let Some(ref pool) = tabular.db_pool {
            if let Some(id) = connection.id {
                let pool_clone = pool.clone();
                let connection = connection.clone();
                let rt = tokio::runtime::Runtime::new().unwrap();
                
                
                let result = rt.block_on(async {
                    sqlx::query(
                        "UPDATE connections SET name = ?, host = ?, port = ?, username = ?, password = ?, database_name = ?, connection_type = ?, folder = ? WHERE id = ?"
                    )
                    .bind(connection.name)
                    .bind(connection.host)
                    .bind(connection.port)
                    .bind(connection.username)
                    .bind(connection.password)
                    .bind(connection.database)
                    .bind(format!("{:?}", connection.connection_type))
                    .bind(connection.folder)
                    .bind(id)
                    .execute(pool_clone.as_ref())
                    .await
                });
                
                match &result {
                    Ok(query_result) => {
                        debug!("Update successful: {} rows affected", query_result.rows_affected());
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
        
        // Remove from database first with explicit transaction
        if let Some(ref pool) = tabular.db_pool {
            let pool_clone = pool.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();
            
            let result: Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> = rt.block_on(async {
                // Begin transaction
                let mut tx = pool_clone.begin().await?;
                
                // Delete cache data first (foreign key constraints will handle this automatically due to CASCADE)
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
                
                // Delete the connection
                let delete_result = sqlx::query("DELETE FROM connections WHERE id = ?")
                    .bind(connection_id)
                    .execute(&mut *tx)
                    .await?;
                
                // Commit transaction
                tx.commit().await?;
                
                Ok(delete_result)
            });
            
            match result {
                Ok(delete_result) => {
                    
                    // Only proceed if we actually deleted something
                    if delete_result.rows_affected() == 0 {
                        debug!("Warning: No rows were deleted from database!");
                        return;
                    }
                },
                Err(e) => {
                    debug!("Failed to delete from database: {}", e);
                    return; // Don't proceed if database deletion failed
                }
            }
        }
        
        tabular.connections.retain(|c| c.id != Some(connection_id));
        // Remove from connection pool cache
        tabular.connection_pools.remove(&connection_id);
                
        // Set flag to force refresh on next update
        tabular.needs_refresh = true;
        
    }


pub(crate) fn test_database_connection(connection: &models::structs::ConnectionConfig) -> (bool, String) {
       // Do not require ICMP ping; many environments (esp. Windows) block it. Try actual DB connect.
        let rt = tokio::runtime::Runtime::new().unwrap();
        
        rt.block_on(async {
            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    let encoded_username = modules::url_encode(&connection.username);
                    let encoded_password = modules::url_encode(&connection.password);
                    let connection_string = format!(
                        "mysql://{}:{}@{}:{}/{}",
                        encoded_username, encoded_password, connection.host, connection.port, connection.database
                    );
                    
                    match MySqlPoolOptions::new()
                        .max_connections(1)
                        .acquire_timeout(std::time::Duration::from_secs(10))
                        .connect(&connection_string)
                        .await
                    {
                        Ok(pool) => {
                            // Test with a simple query
                            match sqlx::query("SELECT 1").execute(&pool).await {
                                Ok(_) => (true, "MySQL connection successful!".to_string()),
                                Err(e) => (false, format!("MySQL query failed: {}", e)),
                            }
                        },
                        Err(e) => (false, format!("MySQL connection failed: {}", e)),
                    }
                },
                models::enums::DatabaseType::PostgreSQL => {
                    let connection_string = format!(
                        "postgresql://{}:{}@{}:{}/{}",
                        connection.username, connection.password, connection.host, connection.port, connection.database
                    );
                    
                    match PgPoolOptions::new()
                        .max_connections(1)
                        .acquire_timeout(std::time::Duration::from_secs(10))
                        .connect(&connection_string)
                        .await
                    {
                        Ok(pool) => {
                            // Test with a simple query
                            match sqlx::query("SELECT 1").execute(&pool).await {
                                Ok(_) => (true, "PostgreSQL connection successful!".to_string()),
                                Err(e) => (false, format!("PostgreSQL query failed: {}", e)),
                            }
                        },
                        Err(e) => (false, format!("PostgreSQL connection failed: {}", e)),
                    }
                },
                models::enums::DatabaseType::SQLite => {
                    let connection_string = format!("sqlite:{}", connection.host);
                    
                    match SqlitePoolOptions::new()
                        .max_connections(1)
                        .acquire_timeout(std::time::Duration::from_secs(10))
                        .connect(&connection_string)
                        .await
                    {
                        Ok(pool) => {
                            // Test with a simple query
                            match sqlx::query("SELECT 1").execute(&pool).await {
                                Ok(_) => (true, "SQLite connection successful!".to_string()),
                                Err(e) => (false, format!("SQLite query failed: {}", e)),
                            }
                        },
                        Err(e) => (false, format!("SQLite connection failed: {}", e)),
                    }
                },
                            models::enums::DatabaseType::MongoDB => {
                                   // Build URI and ping
                                   let uri = if connection.username.is_empty() {
                                          format!("mongodb://{}:{}", connection.host, connection.port)
                                   } else if connection.password.is_empty() {
                                          format!("mongodb://{}@{}:{}", connection.username, connection.host, connection.port)
                                   } else {
                                          let enc_user = modules::url_encode(&connection.username);
                                          let enc_pass = modules::url_encode(&connection.password);
                                          format!("mongodb://{}:{}@{}:{}", enc_user, enc_pass, connection.host, connection.port)
                                   };
                                   match MongoClient::with_uri_str(uri).await {
                                          Ok(client) => {
                                                 let admin = client.database("admin");
                                                 match admin.run_command(doc!("ping": 1)).await {
                                                        Ok(_) => (true, "MongoDB connection successful!".to_string()),
                                                        Err(e) => (false, format!("MongoDB ping failed: {}", e)),
                                                 }
                                          }
                                          Err(e) => (false, format!("MongoDB client error: {}", e)),
                                   }
                            },
                            models::enums::DatabaseType::Redis => {
                    let connection_string = if connection.password.is_empty() {
                        format!("redis://{}:{}", connection.host, connection.port)
                    } else {
                        format!("redis://{}:{}@{}:{}", connection.username, connection.password, connection.host, connection.port)
                    };
                    
                    match Client::open(connection_string) {
                        Ok(client) => {
                            match client.get_connection() {
                                Ok(mut conn) => {
                                    // Test with a simple PING command
                                    match redis::cmd("PING").query::<String>(&mut conn) {
                                        Ok(response) => {
                                            if response == "PONG" {
                                                (true, "Redis connection successful!".to_string())
                                            } else {
                                                (false, "Redis PING returned unexpected response".to_string())
                                            }
                                        },
                                        Err(e) => (false, format!("Redis PING failed: {}", e)),
                                    }
                                },
                                Err(e) => (false, format!("Redis connection failed: {}", e)),
                            }
                        },
                        Err(e) => (false, format!("Redis client creation failed: {}", e)),
                    }
                            },
                            models::enums::DatabaseType::MSSQL => {
                                   // Simple test using tiberius
                                   let host = connection.host.clone();
                                   let port: u16 = connection.port.parse().unwrap_or(1433);
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
                                          if !db.is_empty() { config.database(db.clone()); }
                                          let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                                          tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                                          let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                                          let mut s = client.simple_query("SELECT 1").await.map_err(|e| e.to_string())?;
                                          while let Some(item) = s.try_next().await.map_err(|e| e.to_string())? { if let tiberius::QueryItem::Row(_r) = item { break; } }
                                          Ok::<_, String>(())
                                   }.await;
                                   match res { Ok(_) => (true, "MSSQL connection successful!".to_string()), Err(e) => (false, format!("MSSQL connection failed: {}", e)) }
                            },
            }
        })
    }
