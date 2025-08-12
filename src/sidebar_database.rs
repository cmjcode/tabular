use std::sync::Arc;

use eframe::egui;
use log::{debug, error, info, warn};
use sqlx::SqlitePool;

use crate::{connection, directory, models, modules, sidebar_history, window_egui};

// Helper to parse an editable connection URL and sync it back to fields
#[derive(Debug, Clone)]
struct ParsedUrl {
    db_type: models::enums::DatabaseType,
    host: String,
    port: String,
    username: String,
    password: String,
    database: String,
}

fn parse_connection_url(input: &str) -> Option<ParsedUrl> {
    let url = input.trim();
    if url.is_empty() { return None; }

    // Handle sqlite: special cases (sqlite:path or sqlite://path)
    if let Some(rest) = url.strip_prefix("sqlite:") {
        let path = rest.strip_prefix("//").unwrap_or(rest);
        return Some(ParsedUrl {
            db_type: models::enums::DatabaseType::SQLite,
            host: path.to_string(),
            port: String::new(),
            username: String::new(),
            password: String::new(),
            database: String::new(),
        });
    }

    // General scheme:// parser
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (s.to_lowercase(), r),
        None => {
            // Accept mssql:/mysql:/postgresql:/redis: without // if user types quickly
            if let Some((s, r)) = url.split_once(':') { (s.to_lowercase(), r) } else { return None }
        }
    };

    let db_type = match scheme.as_str() {
        "mysql" => models::enums::DatabaseType::MySQL,
        "postgres" | "postgresql" => models::enums::DatabaseType::PostgreSQL,
        "redis" => models::enums::DatabaseType::Redis,
        "mssql" | "sqlserver" => models::enums::DatabaseType::MSSQL,
        _ => return None,
    };

    let mut user = String::new();
    let mut pass = String::new();
    let mut hostport_and_path = rest;

    // Extract auth if present: use last '@' to avoid '@' in password (should be %40 anyway)
    if let Some(at_idx) = hostport_and_path.rfind('@') {
        let (auth, after) = hostport_and_path.split_at(at_idx);
        hostport_and_path = &after[1..]; // skip '@'
        if let Some((u, p)) = auth.split_once(':') {
            user = modules::url_decode(u);
            pass = modules::url_decode(p);
        } else {
            user = modules::url_decode(auth);
        }
    }

    // Split host:port and optional /database
    let (hostport, path) = match hostport_and_path.split_once('/') {
        Some((hp, p)) => (hp, Some(p)),
        None => (hostport_and_path, None),
    };

    let host: String;
    let mut port = String::new();

    if hostport.starts_with('[') {
        // IPv6 literal [::1]:port
        if let Some(end) = hostport.find(']') {
            host = hostport[1..end].to_string();
            if let Some(rem) = hostport[end + 1..].strip_prefix(':') { port = rem.to_string(); }
        } else {
            host = hostport.to_string();
        }
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        host = h.to_string();
        port = p.to_string();
    } else {
        host = hostport.to_string();
    }

    let database = path.unwrap_or("").trim_matches('/').to_string();

    // Defaults for ports if missing
    if port.is_empty() {
        port = match db_type {
            models::enums::DatabaseType::MySQL => "3306".into(),
            models::enums::DatabaseType::PostgreSQL => "5432".into(),
            models::enums::DatabaseType::Redis => "6379".into(),
            models::enums::DatabaseType::MSSQL => "1433".into(),
            models::enums::DatabaseType::SQLite => String::new(),
        };
    }

    Some(ParsedUrl { db_type, host, port, username: user, password: pass, database })
}


pub(crate) fn render_connection_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context, is_edit_mode: bool) {
       let should_show = if is_edit_mode { tabular.show_edit_connection } else { tabular.show_add_connection };
       
       if !should_show {
       return;
       }
       
       let mut open = true;
       let title = if is_edit_mode { "Edit Connection" } else { "Add New Connection" };
       
        // Clone the connection data to work with
        let mut connection_data = if is_edit_mode {
            tabular.edit_connection.clone()
        } else {
            tabular.new_connection.clone()
        };
       
        egui::Window::new(title)
        .resizable(false)
        .default_width(400.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
              ui.vertical(|ui| {
              egui::Grid::new("connection_form")
                     .num_columns(2)
                     .spacing([10.0, 8.0])
                     .show(ui, |ui| {
                     ui.label("Connection Name:");
                     ui.text_edit_singleline(&mut connection_data.name);
                     ui.end_row();

                     ui.label("Database Type:");
                     egui::ComboBox::from_label("")
                .selected_text(match connection_data.connection_type {
                    models::enums::DatabaseType::MySQL => "MySQL",
                    models::enums::DatabaseType::PostgreSQL => "PostgreSQL",
                    models::enums::DatabaseType::SQLite => "SQLite",
                    models::enums::DatabaseType::Redis => "Redis",
                    models::enums::DatabaseType::MSSQL => "MSSQL",
                })
                            .show_ui(ui, |ui| {
                                   ui.selectable_value(&mut connection_data.connection_type, models::enums::DatabaseType::MySQL, "MySQL");
                                   ui.selectable_value(&mut connection_data.connection_type, models::enums::DatabaseType::PostgreSQL, "PostgreSQL");
                                   ui.selectable_value(&mut connection_data.connection_type, models::enums::DatabaseType::SQLite, "SQLite");
                                   ui.selectable_value(&mut connection_data.connection_type, models::enums::DatabaseType::Redis, "Redis");
                                   ui.selectable_value(&mut connection_data.connection_type, models::enums::DatabaseType::MSSQL, "MSSQL");
                            });
                     ui.end_row();

                     ui.label("Host:");
                     ui.text_edit_singleline(&mut connection_data.host);
                     ui.end_row();

                     ui.label("Port:");
                     ui.text_edit_singleline(&mut connection_data.port);
                     ui.end_row();

                     ui.label("Username:");
                     ui.text_edit_singleline(&mut connection_data.username);
                     ui.end_row();

                     ui.label("Password:");
                     ui.add(egui::TextEdit::singleline(&mut connection_data.password).password(true));
                     ui.end_row();

                     ui.label("Database:");
                     ui.text_edit_singleline(&mut connection_data.database);
                     ui.end_row();

                     ui.label("Folder (Optional):");
                     let mut folder_text = connection_data.folder.as_ref().unwrap_or(&String::new()).clone();
                     ui.text_edit_singleline(&mut folder_text);
                     connection_data.folder = if folder_text.trim().is_empty() { None } else { Some(folder_text.trim().to_string()) };
                     ui.end_row();

                     // Build and edit Connection URL inline to keep alignment with other fields
                     let full_url = {
                         let host = connection_data.host.trim();
                         let port = connection_data.port.trim();
                         let user = connection_data.username.trim();
                         let pass = connection_data.password.clone();
                         let db = connection_data.database.trim();

                         match connection_data.connection_type {
                             models::enums::DatabaseType::MySQL => {
                                 let enc_user = modules::url_encode(user);
                                 let enc_pass = modules::url_encode(&pass);
                                 let path = if db.is_empty() { String::new() } else { format!("/{}", db) };
                                 let auth = if user.is_empty() { String::new() } else if pass.is_empty() { format!("{}@", enc_user) } else { format!("{}:{}@", enc_user, enc_pass) };
                                 format!("mysql://{}{}:{}{}", auth, host, port, path)
                             }
                             models::enums::DatabaseType::PostgreSQL => {
                                 let path = if db.is_empty() { String::new() } else { format!("/{}", db) };
                                 let auth = if user.is_empty() { String::new() } else if pass.is_empty() { format!("{}@", user) } else { format!("{}:{}@", user, pass) };
                                 format!("postgresql://{}{}:{}{}", auth, host, port, path)
                             }
                             models::enums::DatabaseType::SQLite => {
                                 format!("sqlite:{}", host)
                             }
                             models::enums::DatabaseType::Redis => {
                                 if pass.is_empty() && user.is_empty() { format!("redis://{}:{}", host, port) } else if pass.is_empty() { format!("redis://{}@{}:{}", user, host, port) } else { format!("redis://{}:{}@{}:{}", user, pass, host, port) }
                             }
                             models::enums::DatabaseType::MSSQL => {
                                 let path = if db.is_empty() { String::new() } else { format!("/{}", db) };
                                 let auth = if user.is_empty() { String::new() } else if pass.is_empty() { format!("{}@", user) } else { format!("{}:{}@", user, pass) };
                                 format!("mssql://{}{}:{}{}", auth, host, port, path)
                             }
                         }
                     };

                     ui.label("Connection URL:");
                     let mut url_text = full_url.clone();
                     let resp = ui.text_edit_singleline(&mut url_text);
                     if resp.changed() {
                         if let Some(parsed) = parse_connection_url(&url_text) {
                             connection_data.connection_type = parsed.db_type;
                             connection_data.host = parsed.host;
                             connection_data.port = parsed.port;
                             connection_data.username = parsed.username;
                             connection_data.password = parsed.password;
                             connection_data.database = parsed.database;
                         }
                     }
                     ui.end_row();
                     });

              ui.separator();

              ui.horizontal(|ui| {
                     let save_button_text = if is_edit_mode { "Update" } else { "Save" };
                     if ui.button(save_button_text).clicked() && !connection_data.name.is_empty() {
                     if is_edit_mode {
                            // Update existing connection
                            if let Some(id) = connection_data.id {
                                   
                                   if connection::update_connection_in_database(tabular, &connection_data) {
                                          load_connections(tabular);
                                          refresh_connections_tree(tabular);
                                   } else {
                                   // Fallback to in-memory update
                                   if let Some(existing) = tabular.connections.iter_mut().find(|c| c.id == Some(id)) {
                                          *existing = connection_data.clone();
                                          refresh_connections_tree(tabular);
                                   } else {
                                          debug!("ERROR: Could not find connection {} in memory", id);
                                   }
                                   }
                            } else {
                                   debug!("ERROR: Connection has no ID, cannot update");
                            }
                            tabular.show_edit_connection = false;
                     } else {
                            // Add new connection
                            let mut connection_to_add = connection_data.clone();
                            
                            // Try to save to database first
                            if save_connection_to_database(tabular, &connection_to_add) {
                                   // If database save successful, reload from database to get ID
                                   load_connections(tabular);
                                   refresh_connections_tree(tabular);
                            } else {
                                   // Fallback to in-memory storage
                                   let new_id = tabular.connections.iter()
                                   .filter_map(|c| c.id)
                                   .max()
                                   .unwrap_or(0) + 1;
                                   connection_to_add.id = Some(new_id);
                                   tabular.connections.push(connection_to_add);
                                   refresh_connections_tree(tabular);
                            }
                            
                            tabular.new_connection = models::structs::ConnectionConfig::default();
                            tabular.test_connection_status = None;
                            tabular.test_connection_in_progress = false;
                            tabular.show_add_connection = false;
                     }
                     }

                     // Push Test Connection button ke kanan
                     ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                     // Test Connection button (untuk kedua mode add dan edit)
                     if tabular.test_connection_in_progress {
                            ui.spinner();
                            ui.label("Testing connection...");
                     } else if ui.button("Test Connection").clicked() {
                            tabular.test_connection_in_progress = true;
                            tabular.test_connection_status = None;
                            
                            // Test connection based on database type
                            let result = connection::test_database_connection(&connection_data);
                            tabular.test_connection_in_progress = false;
                            tabular.test_connection_status = Some(result);
                     }
                     });
              });
              
              // Display test connection status (untuk kedua mode add dan edit)
              if let Some((success, message)) = &tabular.test_connection_status {
                     ui.separator();
                     if *success {
                     ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::GREEN, "✓");
                            ui.colored_label(egui::Color32::GREEN, message);
                     });
                     } else {
                     ui.horizontal(|ui| {
                            ui.colored_label(egui::Color32::RED, "✗");
                            ui.colored_label(egui::Color32::RED, message);
                     });
                     }
              }
              });
       });
       
       // Update the original data with any changes made in the dialog
       if is_edit_mode {
       tabular.edit_connection = connection_data;
       } else {
       tabular.new_connection = connection_data;
       }
       
       // Handle window close via X button
       if !open {
       if is_edit_mode {
              tabular.show_edit_connection = false;
       } else {
              tabular.new_connection = models::structs::ConnectionConfig::default();
              tabular.test_connection_status = None;
              tabular.test_connection_in_progress = false;
              tabular.show_add_connection = false;
       }
       }
}

pub(crate) fn render_add_connection_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
       render_connection_dialog(tabular, ctx, false);
}

pub(crate) fn render_edit_connection_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
       render_connection_dialog(tabular, ctx, true);
}

pub(crate) fn load_connections(tabular: &mut window_egui::Tabular) {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let connections_result = rt.block_on(async {
              sqlx::query_as::<_, (i64, String, String, String, String, String, String, String, Option<String>)>(
              "SELECT id, name, host, port, username, password, database_name, connection_type, folder FROM connections"
              )
              .fetch_all(pool_clone.as_ref())
              .await
       });
       
       if let Ok(rows) = connections_result {

              tabular.connections = rows.into_iter().map(|(id, name, host, port, username, password, database_name, connection_type, folder)| {
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
                     "MSSQL" => models::enums::DatabaseType::MSSQL,
                     _ => models::enums::DatabaseType::SQLite,
                     },
                     folder,
              }
              }).collect();
       }
       }
       
       // Refresh the tree after loading connections
       refresh_connections_tree(tabular);
}


pub(crate) fn save_connection_to_database(tabular: &mut window_egui::Tabular, connection: &models::structs::ConnectionConfig) -> bool {
       if let Some(ref pool) = tabular.db_pool {
       let pool_clone = pool.clone();
       let connection = connection.clone();
       let rt = tokio::runtime::Runtime::new().unwrap();
       
       let result = rt.block_on(async {
              sqlx::query(
              "INSERT INTO connections (name, host, port, username, password, database_name, connection_type, folder) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
              )
              .bind(connection.name)
              .bind(connection.host)
              .bind(connection.port)
              .bind(connection.username)
              .bind(connection.password)
              .bind(connection.database)
              .bind(format!("{:?}", connection.connection_type))
              .bind(connection.folder)
              .execute(pool_clone.as_ref())
              .await
       });
       
       result.is_ok()
       } else {
       false
       }
}

pub(crate) fn start_edit_connection(tabular: &mut window_egui::Tabular, connection_id: i64) {
       // Find the connection to edit
       if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(connection_id)) {
       tabular.edit_connection = connection.clone();
       // Reset test connection status saat buka edit dialog
       tabular.test_connection_status = None;
       tabular.test_connection_in_progress = false;
       tabular.show_edit_connection = true;
       } else {
       for conn in &tabular.connections {
              debug!("  - {} (ID: {:?})", conn.name, conn.id);
       }
       }
}

pub(crate) fn copy_connection(tabular: &mut window_egui::Tabular, connection_id: i64) {
       // Find the connection to copy
       if let Some(connection) = tabular.connections.iter().find(|c| c.id == Some(connection_id)).cloned() {
       let mut copied_connection = connection.clone();
       
       // Reset ID and modify name to indicate it's a copy
       copied_connection.id = None;
       copied_connection.name = format!("{} - Copy", copied_connection.name);
       
       
       // Try to save to database first
       if save_connection_to_database(tabular, &copied_connection) {
              // If database save successful, reload from database to get ID
              load_connections(tabular);
       } else {
              // Fallback to in-memory storage
              let new_id = tabular.connections.iter()
              .filter_map(|c| c.id)
              .max()
              .unwrap_or(0) + 1;
              copied_connection.id = Some(new_id);
              tabular.connections.push(copied_connection);
       }
       
       } else {
       debug!("❌ Connection with ID {} not found for copying", connection_id);
       }
}




pub(crate) fn initialize_database(tabular: &mut window_egui::Tabular) {
        // Ensure app directories exist
        if let Err(e) = directory::ensure_app_directories() {
            error!("Failed to create app directories: {}", e);
            return;
        }
        
        // Initialize SQLite database
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pool_result = rt.block_on(async {
            // Get the data directory path
            let data_dir = directory::get_data_dir();
            let db_path = data_dir.join("connections.db");
                        
            // Convert path to string and use file:// prefix for SQLite
            let db_path_str = db_path.to_string_lossy();
            let connection_string = format!("sqlite://{}?mode=rwc", db_path_str);
            let pool = SqlitePool::connect(&connection_string).await;
            
            match pool {
                Ok(pool) => {
                    info!("Database connection successful");
                    
                    // Create connections table
                    let create_connections_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS connections (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            name TEXT NOT NULL,
                            host TEXT NOT NULL,
                            port TEXT NOT NULL,
                            username TEXT NOT NULL,
                            password TEXT NOT NULL,
                            database_name TEXT NOT NULL,
                            connection_type TEXT NOT NULL,
                            folder TEXT DEFAULT NULL
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;
                    
                    // Add folder column if it doesn't exist (for existing databases)
                    let _ = sqlx::query(
                        "ALTER TABLE connections ADD COLUMN folder TEXT DEFAULT NULL"
                    )
                    .execute(&pool)
                    .await;
                    
                    
                    // Create database cache table
                    let create_db_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS database_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;
                    
                    // Create table cache table
                    let create_table_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS table_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            table_type TEXT NOT NULL, -- 'table', 'view', 'procedure', etc.
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name, table_type)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;
                    
                    // Create column cache table
                    let create_column_cache_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS column_cache (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            connection_id INTEGER NOT NULL,
                            database_name TEXT NOT NULL,
                            table_name TEXT NOT NULL,
                            column_name TEXT NOT NULL,
                            data_type TEXT NOT NULL,
                            ordinal_position INTEGER NOT NULL,
                            -- New flags for schema insights
                            is_primary_key INTEGER NOT NULL DEFAULT 0, -- 0 = false, 1 = true
                            is_indexed INTEGER NOT NULL DEFAULT 0,     -- 0 = false, 1 = true
                            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE,
                            UNIQUE(connection_id, database_name, table_name, column_name)
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;
                    
                    // Create query history table
                    let create_history_result = sqlx::query(
                        r#"
                        CREATE TABLE IF NOT EXISTS query_history (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            query_text TEXT NOT NULL,
                            connection_id INTEGER NOT NULL,
                            connection_name TEXT NOT NULL,
                            executed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (connection_id) REFERENCES connections (id) ON DELETE CASCADE
                        )
                        "#
                    )
                    .execute(&pool)
                    .await;
                    
                    match (create_connections_result, create_db_cache_result, create_table_cache_result, create_column_cache_result, create_history_result) {
                        (Ok(_), Ok(_), Ok(_), Ok(_), Ok(_)) => {
                            Some(pool)
                        },
                        _ => {
                            warn!("Error creating some tables");
                            None
                        }
                    }
                },
                Err(e) => {
                    error!("Database connection failed: {}", e);
                    None
                }
            }
        });
        
        if let Some(pool) = pool_result {
            tabular.db_pool = Some(Arc::new(pool));
            // Best-effort migrations for new columns (idempotent): add flags to column_cache
            // Ignore errors if columns already exist
            if let Some(ref pool) = tabular.db_pool {
                let _ = rt.block_on(async {
                    let _ = sqlx::query(
                        "ALTER TABLE column_cache ADD COLUMN is_primary_key INTEGER NOT NULL DEFAULT 0"
                    )
                    .execute(pool.as_ref())
                    .await;
                    let _ = sqlx::query(
                        "ALTER TABLE column_cache ADD COLUMN is_indexed INTEGER NOT NULL DEFAULT 0"
                    )
                    .execute(pool.as_ref())
                    .await;
                });
            }
            // Load existing connections from database
            load_connections(tabular);
            // Load query history from database
            sidebar_history::load_query_history(tabular);
        }
    }

pub(crate) fn initialize_sample_data(tabular: &mut window_egui::Tabular) {
        // Initialize with connections as root nodes
        refresh_connections_tree(tabular);

        // Don't add sample queries - let load_queries_from_directory handle the real structure
        // self.queries_tree will be populated by load_queries_from_directory()

        // Initialize empty history tree (will be loaded from database)
       //  self.refresh_history_tree();
    }

pub(crate) fn refresh_connections_tree(tabular: &mut window_egui::Tabular) {
                
        // Clear existing tree
        tabular.items_tree.clear();

        // Create folder structure for connections
        tabular.items_tree = create_connections_folder_structure(tabular);
            
        
    }

pub(crate) fn create_connections_folder_structure(tabular: &mut window_egui::Tabular) -> Vec<models::structs::TreeNode> {
        // Group connections by custom folder first, then by database type
        let mut folder_groups: std::collections::HashMap<String, Vec<&models::structs::ConnectionConfig>> = std::collections::HashMap::new();
        
        // Group connections by custom folder
        for conn in &tabular.connections {
            let folder_name = conn.folder.as_ref().unwrap_or(&"Default".to_string()).clone();
            folder_groups.entry(folder_name).or_default().push(conn);
        }
        
        let mut result = Vec::new();
        
        // Create folder structure for each custom folder
        for (folder_name, connections) in folder_groups {
            if connections.is_empty() {
                continue;
            }
            
            // Create custom folder node
            let mut custom_folder = models::structs::TreeNode::new(folder_name.clone(), models::enums::NodeType::CustomFolder);
            custom_folder.is_expanded = false; // Start collapsed
            
            // Within each custom folder, group by database type
            let mut mysql_connections = Vec::new();
            let mut postgresql_connections = Vec::new();
            let mut sqlite_connections = Vec::new();
            let mut redis_connections = Vec::new();
            let mut mssql_connections = Vec::new();
            
            for conn in connections {
                if let Some(id) = conn.id {
                    let node = models::structs::TreeNode::new_connection(conn.name.clone(), id);
                    match conn.connection_type {
                        models::enums::DatabaseType::MySQL => {
                            mysql_connections.push(node);
                        },
                        models::enums::DatabaseType::PostgreSQL => {
                            postgresql_connections.push(node);
                        },
                        models::enums::DatabaseType::SQLite => {
                            sqlite_connections.push(node);
                        },
                        models::enums::DatabaseType::Redis => {
                            redis_connections.push(node);
                        },
                        models::enums::DatabaseType::MSSQL => {
                            mssql_connections.push(node);
                        },
                    }
                } else {
                    debug!("  -> Skipping connection with no ID");
                }
            }
            
            // Create database type folders within custom folder
            let mut db_type_folders = Vec::new();
                        
            if !mysql_connections.is_empty() {
                let _ = mysql_connections.len();
                let mut mysql_folder = models::structs::TreeNode::new("MySQL".to_string(), models::enums::NodeType::MySQLFolder);
                mysql_folder.children = mysql_connections;
                mysql_folder.is_expanded = false;
                db_type_folders.push(mysql_folder);
            }
            
            if !postgresql_connections.is_empty() {
                let _ = postgresql_connections.len();
                let mut postgresql_folder = models::structs::TreeNode::new("PostgreSQL".to_string(), models::enums::NodeType::PostgreSQLFolder);
                postgresql_folder.children = postgresql_connections;
                postgresql_folder.is_expanded = false;
                db_type_folders.push(postgresql_folder);
            }
            
            if !sqlite_connections.is_empty() {
                let _ = sqlite_connections.len();
                let mut sqlite_folder = models::structs::TreeNode::new("SQLite".to_string(), models::enums::NodeType::SQLiteFolder);
                sqlite_folder.children = sqlite_connections;
                sqlite_folder.is_expanded = false;
                db_type_folders.push(sqlite_folder);
            }
            
            if !redis_connections.is_empty() {
                let _ = redis_connections.len();
                let mut redis_folder = models::structs::TreeNode::new("Redis".to_string(), models::enums::NodeType::RedisFolder);
                redis_folder.children = redis_connections;
                redis_folder.is_expanded = false;
                db_type_folders.push(redis_folder);
            }
            if !mssql_connections.is_empty() {
                let _ = mssql_connections.len();
                // Correct NodeType for MSSQL folder (previously mistakenly used MySQLFolder)
                let mut mssql_folder = models::structs::TreeNode::new("MSSQL".to_string(), models::enums::NodeType::MSSQLFolder);
                mssql_folder.children = mssql_connections;
                mssql_folder.is_expanded = false;
                db_type_folders.push(mssql_folder);
            }
            
            custom_folder.children = db_type_folders;
            result.push(custom_folder);
        }
        
        // Sort folders alphabetically, but put "Default" first
        result.sort_by(|a, b| {
            if a.name == "Default" {
                std::cmp::Ordering::Less
            } else if b.name == "Default" {
                std::cmp::Ordering::Greater
            } else {
                a.name.cmp(&b.name)
            }
        });
        
        if result.is_empty() {
            debug!("No connections found, returning empty tree");
        }
        
        result
    }
