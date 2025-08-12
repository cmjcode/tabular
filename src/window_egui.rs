
use eframe::{egui, App, Frame};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use log::{debug, error};

use crate::{
    dialog, cache_data, connection, directory, driver_mysql, driver_postgres, driver_redis, driver_sqlite, editor, export, models, sidebar_database, sidebar_query
};





pub struct Tabular {
    pub editor_text: String,
    pub selected_menu: String,
    pub items_tree: Vec<models::structs::TreeNode>,
    pub queries_tree: Vec<models::structs::TreeNode>,
    pub history_tree: Vec<models::structs::TreeNode>,
    pub history_items: Vec<models::structs::HistoryItem>, // Actual history data
    pub connections: Vec<models::structs::ConnectionConfig>,
    pub show_add_connection: bool,
    pub new_connection: models::structs::ConnectionConfig,
    pub db_pool: Option<Arc<SqlitePool>>,
    // Global async runtime for all database operations
    pub runtime: Option<Arc<tokio::runtime::Runtime>>,
    // Connection cache untuk menghindari membuat koneksi berulang
    pub connection_pools: HashMap<i64, models::enums::DatabasePool>,
    // Context menu and edit connection fields
    pub show_edit_connection: bool,
    pub edit_connection: models::structs::ConnectionConfig,
    // UI refresh flag
    pub needs_refresh: bool,
    // Table data display
    pub current_table_data: Vec<Vec<String>>,
    pub current_table_headers: Vec<String>,
    pub current_table_name: String,
    pub current_connection_id: Option<i64>,
    // Pagination
    pub current_page: usize,
    pub page_size: usize,
    pub total_rows: usize,
    pub all_table_data: Vec<Vec<String>>, // Store all data for pagination
    // Splitter position for resizable table view (0.0 to 1.0)
    pub table_split_ratio: f32,
    // Table sorting state
    pub sort_column: Option<usize>,
    pub sort_ascending: bool,
    // Test connection status
    pub test_connection_status: Option<(bool, String)>, // (success, message)
    pub test_connection_in_progress: bool,
    // Background processing channels
    pub background_sender: Option<Sender<models::enums::BackgroundTask>>,
    pub background_receiver: Option<Receiver<models::enums::BackgroundResult>>,
    // Background refresh status tracking
    pub refreshing_connections: std::collections::HashSet<i64>,
    // Query tab system
    pub query_tabs: Vec<models::structs::QueryTab>,
    pub active_tab_index: usize,
    pub next_tab_id: usize,
    // Save dialog
    pub show_save_dialog: bool,
    pub save_filename: String,
    // Connection selection dialog
    pub show_connection_selector: bool,
    pub pending_query: String, // Store query to execute after connection is selected
    pub auto_execute_after_connection: bool, // Flag to auto-execute after connection selected
    // Error message display
    pub error_message: String,
    pub show_error_message: bool,
    // Advanced Editor Configuration
    pub advanced_editor: models::structs::AdvancedEditor,
    // Selected text for executing only selected queries
    pub selected_text: String,
    // Cursor position for query extraction
    pub cursor_position: usize,
    // Command Palette
    pub show_command_palette: bool,
    pub command_palette_input: String,
    pub show_theme_selector: bool,
    pub command_palette_items: Vec<String>,
    pub command_palette_selected_index: usize,
    pub theme_selector_selected_index: usize,
    // Flag to request theme selector on next frame
    pub request_theme_selector: bool,
    // Database search functionality
    pub database_search_text: String,
    pub filtered_items_tree: Vec<models::structs::TreeNode>,
    pub show_search_results: bool,
    // Query folder management
    pub show_create_folder_dialog: bool,
    pub new_folder_name: String,
    pub selected_query_for_move: Option<String>,
    pub show_move_to_folder_dialog: bool,
    pub target_folder_name: String,
    pub parent_folder_for_creation: Option<String>,
    pub selected_folder_for_removal: Option<String>,
    pub folder_removal_map: std::collections::HashMap<i64, String>, // Map hash to folder path
    // Connection pool cleanup tracking
    pub last_cleanup_time: std::time::Instant,
    // Table selection tracking
    pub selected_row: Option<usize>,
    pub selected_cell: Option<(usize, usize)>, // (row_index, column_index)
    // Column width management for resizable columns
    pub column_widths: Vec<f32>, // Store individual column widths
    pub min_column_width: f32,
    // Gear menu and about dialog
    pub show_about_dialog: bool,
    // Logo texture
    pub logo_texture: Option<egui::TextureHandle>,
    // Database cache for performance
    pub database_cache: std::collections::HashMap<i64, Vec<String>>, // connection_id -> databases
    pub database_cache_time: std::collections::HashMap<i64, std::time::Instant>, // connection_id -> cache time
    // Autocomplete state
    pub show_autocomplete: bool,
    pub autocomplete_suggestions: Vec<String>,
    pub selected_autocomplete_index: usize,
    pub autocomplete_prefix: String,
    pub last_autocomplete_trigger_len: usize,
    pub pending_cursor_set: Option<usize>,
}



impl Tabular {

// Small painter-drawn triangle toggle to avoid font glyph issues
fn triangle_toggle(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
    let size = egui::vec2(16.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let color = ui.visuals().text_color();
        let stroke = egui::Stroke { width: 1.0, color };
        if expanded {
            // Down triangle
            let p1 = egui::pos2(rect.center().x - 6.0, rect.top() + 5.0);
            let p2 = egui::pos2(rect.center().x + 6.0, rect.top() + 5.0);
            let p3 = egui::pos2(rect.center().x, rect.top() + 11.0);
            painter.add(egui::Shape::convex_polygon(vec![p1, p2, p3], color, stroke));
        } else {
            // Right triangle
            let p1 = egui::pos2(rect.left() + 5.0, rect.center().y - 6.0);
            let p2 = egui::pos2(rect.left() + 5.0, rect.center().y + 6.0);
            let p3 = egui::pos2(rect.left() + 11.0, rect.center().y);
            painter.add(egui::Shape::convex_polygon(vec![p1, p2, p3], color, stroke));
        }
    }

    response
}
    // Helper: build MSSQL SELECT ensuring database context and proper quoting.
    // db_name: selected database (can be empty -> fallback to object-provided or omit USE)
    // raw_name: could be formats: table, [schema].[object], schema.object, [db].[schema].[object], db.schema.object
    fn build_mssql_select_query(db_name: String, raw_name: String) -> String {
        // Normalize raw name: remove trailing semicolons/spaces
        let cleaned = raw_name.trim().trim_end_matches(';').to_string();

        // Split by '.' ignoring brackets segments
        // Strategy: remove outer brackets then split, re-wrap each part with []
        let mut parts = Vec::new();
        let mut current = String::new();
        let mut in_bracket = false;
        for ch in cleaned.chars() {
            match ch {
                '[' => { in_bracket = true; current.push(ch); },
                ']' => { in_bracket = false; current.push(ch); },
                '.' if !in_bracket => { parts.push(current.clone()); current.clear(); },
                _ => current.push(ch),
            }
        }
        if !current.is_empty() { parts.push(current); }

        // Remove surrounding brackets from each part and re-apply sanitized
        let mut plain_parts: Vec<String> = parts.into_iter().map(|p| {
            let p2 = p.trim();
            let p2 = p2.strip_prefix('[').unwrap_or(p2);
            let p2 = p2.strip_suffix(']').unwrap_or(p2);
            p2.to_string()
        }).collect();

        // Decide final composition
        // Cases by length: 1=object, 2=schema.object, 3=db.schema.object
        // If db_name provided, override database part.
        let (database_part, schema_part, object_part) = match plain_parts.len() {
            3 => {
                let obj = plain_parts.pop().unwrap();
                let schema = plain_parts.pop().unwrap();
                let db = if !db_name.is_empty() { db_name.clone() } else { plain_parts.pop().unwrap() };
                (db, schema, obj)
            },
            2 => {
                let obj = plain_parts.pop().unwrap();
                let schema = plain_parts.pop().unwrap();
                let db = if !db_name.is_empty() { db_name.clone() } else { String::new() };
                (db, schema, obj)
            },
            1 => {
                let obj = plain_parts.pop().unwrap();
                let db = db_name.clone();
                (db, "dbo".to_string(), obj)
            },
            _ => (db_name.clone(), "dbo".to_string(), cleaned),
        };

        // Build fully qualified name with brackets
        let fq = if database_part.is_empty() {
            format!("[{}].[{}]", schema_part, object_part)
        } else {
            format!("[{}].[{}].[{}]", database_part, schema_part, object_part)
        };

        // If database part present, prepend USE to ensure context
        if database_part.is_empty() {
            format!("SELECT TOP 100 * FROM {};", fq)
        } else {
            format!("USE [{}];\nSELECT TOP 100 * FROM {};", database_part, fq)
        }
    }


    pub fn new() -> Self {
        // Create background processing channels
        let (background_sender, background_receiver) = mpsc::channel::<models::enums::BackgroundTask>();
        let (result_sender, result_receiver) = mpsc::channel::<models::enums::BackgroundResult>();

        // Create shared runtime for all database operations
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(rt) => Some(Arc::new(rt)),
            Err(e) => {
                error!("Failed to create runtime: {}", e);
                None
            }
        };

        let mut app = Self {
            editor_text: String::new(),
            selected_menu: "Database".to_string(),
            items_tree: Vec::new(),
            queries_tree: Vec::new(),
            history_tree: Vec::new(),
            history_items: Vec::new(),
            connections: Vec::new(),
            show_add_connection: false,
            new_connection: models::structs::ConnectionConfig::default(),
            db_pool: None,
            runtime,
            connection_pools: HashMap::new(), // Start with empty cache
            show_edit_connection: false,
            edit_connection: models::structs::ConnectionConfig::default(),
            needs_refresh: false,
            current_table_data: Vec::new(),
            current_table_headers: Vec::new(),
            current_table_name: String::new(),
            current_connection_id: None,
            current_page: 0,
            page_size: 100, // Default 100 rows per page
            total_rows: 0,
            all_table_data: Vec::new(),
            table_split_ratio: 0.6, // Default 60% for editor, 40% for table
            sort_column: None,
            sort_ascending: true,
            test_connection_status: None,
            test_connection_in_progress: false,
            background_sender: Some(background_sender),
            background_receiver: Some(result_receiver),
            refreshing_connections: std::collections::HashSet::new(),
            query_tabs: Vec::new(),
            active_tab_index: 0,
            next_tab_id: 1,
            show_save_dialog: false,
            save_filename: String::new(),
            show_connection_selector: false,
            pending_query: String::new(),
            auto_execute_after_connection: false,
            error_message: String::new(),
            show_error_message: false,
            advanced_editor: models::structs::AdvancedEditor::default(),
            selected_text: String::new(),
            cursor_position: 0,
            show_command_palette: false,
            command_palette_input: String::new(),
            show_theme_selector: false,
            command_palette_items: Vec::new(),
            command_palette_selected_index: 0,
            theme_selector_selected_index: 0,
            request_theme_selector: false,
            // Database search functionality
            database_search_text: String::new(),
            filtered_items_tree: Vec::new(),
            show_search_results: false,
            // Query folder management
            show_create_folder_dialog: false,
            new_folder_name: String::new(),
            selected_query_for_move: None,
            show_move_to_folder_dialog: false,
            target_folder_name: String::new(),
            parent_folder_for_creation: None,
            selected_folder_for_removal: None,
            folder_removal_map: std::collections::HashMap::new(),
            last_cleanup_time: std::time::Instant::now(),
            selected_row: None,
            selected_cell: None,
            // Column width management
            column_widths: Vec::new(),
            min_column_width: 50.0,
            // Gear menu and about dialog
            show_about_dialog: false,
            // Logo texture
            logo_texture: None,
            // Database cache for performance
            database_cache: std::collections::HashMap::new(),
            database_cache_time: std::collections::HashMap::new(),
            // Autocomplete
            show_autocomplete: false,
            autocomplete_suggestions: Vec::new(),
            selected_autocomplete_index: 0,
            autocomplete_prefix: String::new(),
            last_autocomplete_trigger_len: 0,
            pending_cursor_set: None,
        };
        
        // Clear any old cached pools
        app.connection_pools.clear();
        
        // Initialize database and sample data FIRST
        sidebar_database::initialize_database(&mut app);
        sidebar_database::initialize_sample_data(&mut app);
        
        // Load saved queries from directory
        sidebar_query::load_queries_from_directory(&mut app);
        
        // Create initial query tab
        editor::create_new_tab(&mut app, "Untitled Query".to_string(), String::new());
        
        // Start background thread AFTER database is initialized
        app.start_background_worker(background_receiver, result_sender);
        
        app
    }


    fn start_background_worker(&self, task_receiver: Receiver<models::enums::BackgroundTask>, result_sender: Sender<models::enums::BackgroundResult>) {
        // Get the current db_pool for cache operations
        let db_pool = self.db_pool.clone();
        
        std::thread::spawn(move || {
            // Create a single-threaded Tokio runtime for this background thread
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            
            while let Ok(task) = task_receiver.recv() {
                match task {
                    models::enums::BackgroundTask::RefreshConnection { connection_id } => {
                        let success = rt.block_on(async {
                            connection::refresh_connection_background_async(
                                connection_id,
                                &db_pool
                            ).await
                        });
                        
                        let _ = result_sender.send(models::enums::BackgroundResult::RefreshComplete {
                            connection_id,
                            success,
                        });
                    }
                }
            }
        });
    }



    fn get_active_tab_title(&self) -> String {
        if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
            if tab.is_modified {
                format!("● {}", tab.title)
            } else {
                tab.title.clone()
            }
        } else {
            "No Tab".to_string()
        }
    }

    pub fn set_active_tab_connection(&mut self, connection_id: Option<i64>) {
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.connection_id = connection_id;
            // Reset database when changing connection
            tab.database_name = None;
        }
    }

    pub fn set_active_tab_connection_with_database(&mut self, connection_id: Option<i64>, database_name: Option<String>) {
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.connection_id = connection_id;
            tab.database_name = database_name;
        }
    }

    pub fn set_active_tab_database(&mut self, database_name: Option<String>) {
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.database_name = database_name;
        }
    }


    fn get_connection_name(&self, connection_id: i64) -> Option<String> {
        self.connections.iter()
            .find(|conn| conn.id == Some(connection_id))
            .map(|conn| conn.name.clone())
    }

    fn render_tree(&mut self, ui: &mut egui::Ui, nodes: &mut [models::structs::TreeNode]) -> Vec<(String, String, String)> {
        let mut expansion_requests = Vec::new();
        let mut tables_to_expand = Vec::new();
        let mut context_menu_requests = Vec::new();
        let mut table_click_requests = Vec::new();
        let mut connection_click_requests = Vec::new();
        let mut query_files_to_open = Vec::new();
        
        for (index, node) in nodes.iter_mut().enumerate() {
            let (expansion_request, table_expansion, context_menu_request, table_click_request, connection_click_request, query_file_to_open, folder_for_removal, parent_for_creation, folder_removal_mapping) = Self::render_tree_node_with_table_expansion(ui, node, &mut self.editor_text, index, &self.refreshing_connections);
            if let Some(expansion_req) = expansion_request {
                expansion_requests.push(expansion_req);
            }
            if let Some((table_index, connection_id, table_name)) = table_expansion {
                tables_to_expand.push((table_index, connection_id, table_name));
            }
            if let Some(folder_name) = folder_for_removal {
                self.selected_folder_for_removal = Some(folder_name.clone());
            }
            if let Some((hash, folder_path)) = folder_removal_mapping {
                self.folder_removal_map.insert(hash, folder_path);
                debug!("📁 Stored folder removal mapping: hash={} -> path={}", hash, self.folder_removal_map.get(&hash).unwrap_or(&"NONE".to_string()));
            }
            if let Some(parent_folder) = parent_for_creation {
                self.parent_folder_for_creation = Some(parent_folder);
            }
            if let Some(context_id) = context_menu_request {
                context_menu_requests.push(context_id);
            }
            if let Some((connection_id, table_name)) = table_click_request {
                table_click_requests.push((connection_id, table_name));
            }
            if let Some(connection_id) = connection_click_request {
                connection_click_requests.push(connection_id);
            }
            if let Some((filename, content, file_path)) = query_file_to_open {
                debug!("📋 Collected query file to open: {} (path: {})", filename, file_path);
                query_files_to_open.push((filename, content, file_path));
            }
        }
        
        // Handle connection clicks (create new tab with that connection)
        for connection_id in connection_click_requests {
            // Find connection name for tab title
            let connection_name = self.connections.iter()
                .find(|conn| conn.id == Some(connection_id))
                .map(|conn| conn.name.clone())
                .unwrap_or_else(|| format!("Connection {}", connection_id));
            
            // Create new tab with this connection pre-selected
            let tab_title = format!("Query - {}", connection_name);
            editor::create_new_tab_with_connection(self, tab_title, String::new(), Some(connection_id));
            
            debug!("Created new tab with connection ID: {}", connection_id);
        }
        
        // Handle expansions after rendering
        for expansion_req in expansion_requests {
            
            match expansion_req.node_type {
                models::enums::NodeType::Connection => {
                    // Find Connection node recursively and load if not already loaded
                    if let Some(connection_node) = Self::find_connection_node_recursive(nodes, expansion_req.connection_id) {
                        if !connection_node.is_loaded {
                            self.load_connection_tables(expansion_req.connection_id, connection_node);
                        }
                    } else {
                        debug!("Connection node not found for ID: {}", expansion_req.connection_id);
                    }
                },
                models::enums::NodeType::DatabasesFolder => {
                    // Handle DatabasesFolder expansion - load actual databases from server
                    for node in nodes.iter_mut() {
                        if node.node_type == models::enums::NodeType::Connection && node.connection_id == Some(expansion_req.connection_id) {
                            // Find the DatabasesFolder within this connection
                            for child in &mut node.children {
                                if child.node_type == models::enums::NodeType::DatabasesFolder && !child.is_loaded {
                                    self.load_databases_for_folder(expansion_req.connection_id, child);
                                    break;
                                }
                            }
                            break;
                        }
                    }
                },
                models::enums::NodeType::Database => {
                    debug!("🔍 Database expansion request received for connection_id: {}, database_name: {:?}", 
                             expansion_req.connection_id, expansion_req.database_name);
                    
                    // Handle Database expansion for Redis - load keys for the database
                    if let Some(connection) = self.connections.iter().find(|c| c.id == Some(expansion_req.connection_id)) {
                        debug!("✅ Found connection: {} (type: {:?})", connection.name, connection.connection_type);
                        
                        if connection.connection_type == models::enums::DatabaseType::Redis {
                            debug!("🔑 Processing Redis database expansion");
                            
                            // Find the database node and load its keys
                            let mut node_found = false;
                            for (node_idx, node) in nodes.iter_mut().enumerate() {
                                debug!("🌳 Checking tree node [{}]: '{}' (type: {:?}, connection_id: {:?})", 
                                         node_idx, node.name, node.node_type, node.connection_id);
                                
                                if let Some(db_node) = Self::find_redis_database_node(node, expansion_req.connection_id, &expansion_req.database_name) {
                                    debug!("📁 Found database node: {}, is_loaded: {}", db_node.name, db_node.is_loaded);
                                    node_found = true;
                                    
                                    if !db_node.is_loaded {
                                        debug!("⏳ Loading keys for database: {}", expansion_req.database_name.clone().unwrap_or_default());
                                        self.load_redis_keys_for_database(expansion_req.connection_id, &expansion_req.database_name.clone().unwrap_or_default(), db_node);
                                    } else {
                                        debug!("✅ Database already loaded with {} children", db_node.children.len());
                                    }
                                    break;
                                }
                            }
                            
                            if !node_found {
                                debug!("❌ Database node not found in any tree branch for database: {:?}", expansion_req.database_name);
                            }
                        } else {
                            debug!("❌ Connection is not Redis type: {:?}", connection.connection_type);
                        }
                    } else {
                        debug!("❌ Connection not found for ID: {}", expansion_req.connection_id);
                    }
                },
                models::enums::NodeType::TablesFolder | models::enums::NodeType::ViewsFolder | models::enums::NodeType::StoredProceduresFolder |
                models::enums::NodeType::UserFunctionsFolder | models::enums::NodeType::TriggersFolder | models::enums::NodeType::EventsFolder => {
                    // Find the specific folder node and load if not already loaded
                    
                    // We need to find the exact folder node in the tree
                    let connection_id = expansion_req.connection_id;
                    let folder_type = expansion_req.node_type.clone();
                    let database_name = expansion_req.database_name.clone();
                    
                    // Search for folder node by traversing the tree recursively
                    let mut found = false;
                    for node in nodes.iter_mut() {
                        // Search recursively through all nodes, not just top level
                        if let Some(folder_node) = Self::find_specific_folder_node(node, connection_id, &folder_type, &database_name) {
                            if !folder_node.is_loaded {
                                self.load_folder_content(connection_id, folder_node, folder_type.clone());
                                found = true;
                            }
                            break;
                        }
                    }
                    if !found {
                        debug!("Could not find folder node with type {:?} and database {:?} in any of the nodes", folder_type, database_name);
                    }
                },
                _ => {
                    debug!("Unhandled node type: {:?}", expansion_req.node_type);
                }
            }
        }
        
        // Handle table column expansions
        // Handle table expansions
        for (table_index, connection_id, table_name) in tables_to_expand {
            self.load_table_columns_for_node(connection_id, &table_name, nodes, table_index);
        }
        
        // Handle table click requests - create new tab for each table
        for (connection_id, table_name) in table_click_requests {
            // Find the connection to determine the database type and database name
            let connection = self.connections.iter()
                .find(|conn| conn.id == Some(connection_id))
                .cloned();
            
            if let Some(conn) = connection {
                // Find the database name from the tree structure
                let mut database_name: Option<String> = None;
                for node in nodes.iter() {
                    if let Some(db_name) = self.find_database_name_for_table(node, connection_id, &table_name) {
                        database_name = Some(db_name);
                        break;
                    }
                }
                
                // If no database found in tree, use connection default
                if database_name.is_none() {
                    database_name = Some(conn.database.clone());
                }
                
                match conn.connection_type {
                    models::enums::DatabaseType::Redis => {
                        
                        // Check if this is a Redis key (has specific Redis data types in the tree structure)
                        // For Redis keys, we need to find which database they belong to
                        let mut is_redis_key = false;
                        let mut key_type: Option<String> = None;
                        
                        for node in nodes.iter() {
                            if let Some((_, k_type)) = self.find_redis_key_info(node, &table_name) {
                                key_type = Some(k_type.clone());
                                is_redis_key = true;
                                break;
                            }
                        }
                        
                        if is_redis_key {
                            if let Some(k_type) = key_type {
                                // This is a Redis key - create a query tab with appropriate Redis command
                                let redis_command = match k_type.to_lowercase().as_str() {
                                    "string" => format!("GET {}", table_name),
                                    "hash" => format!("HGETALL {}", table_name),
                                    "list" => format!("LRANGE {} 0 -1", table_name),
                                    "set" => format!("SMEMBERS {}", table_name),
                                    "zset" | "sorted_set" => format!("ZRANGE {} 0 -1 WITHSCORES", table_name),
                                    "stream" => format!("XRANGE {} - +", table_name),
                                    _ => format!("TYPE {}", table_name), // Fallback to show type
                                };
                                
                                let tab_title = format!("Redis Key: {} ({})", table_name, k_type);
                                editor::create_new_tab_with_connection_and_database(self, tab_title, redis_command.clone(), Some(connection_id), database_name.clone());
                                
                                // Set current connection ID and database for Redis query execution
                                self.current_connection_id = Some(connection_id);
                                
                                // Auto-execute the Redis query and display results in bottom
                                if let Some((headers, data)) = connection::execute_query_with_connection(self, connection_id, redis_command) {
                                    self.current_table_headers = headers;
                                    self.current_table_data = data.clone();
                                    self.all_table_data = data;
                                    self.current_table_name = format!("Redis Key: {}", table_name);
                                    self.total_rows = self.all_table_data.len();
                                    self.current_page = 0;
                                }
                            }
                        } else {
                            // This is a Redis folder/type - create a query tab for scanning keys
                            let redis_command = match table_name.as_str() {
                                "hashes" => "SCAN 0 MATCH *:* TYPE hash COUNT 100".to_string(),
                                "strings" => "SCAN 0 MATCH *:* TYPE string COUNT 100".to_string(),
                                "lists" => "SCAN 0 MATCH *:* TYPE list COUNT 100".to_string(),
                                "sets" => "SCAN 0 MATCH *:* TYPE set COUNT 100".to_string(),
                                "sorted_sets" => "SCAN 0 MATCH *:* TYPE zset COUNT 100".to_string(),
                                "streams" => "SCAN 0 MATCH *:* TYPE stream COUNT 100".to_string(),
                                _ => {
                                    // Extract folder name from display format like "Strings (5)"
                                    let clean_name = table_name.split('(').next().unwrap_or(&table_name).trim();
                                    format!("SCAN 0 MATCH *:* COUNT 100 # Browse {}", clean_name)
                                }
                            };
                            let tab_title = format!("Redis {}", table_name);
                            editor::create_new_tab_with_connection_and_database(self, tab_title, redis_command.clone(), Some(connection_id), database_name.clone());
                            
                            // Set database and auto-execute
                            self.current_connection_id = Some(connection_id);
                            if let Some((headers, data)) = connection::execute_query_with_connection(self, connection_id, redis_command) {
                                self.current_table_headers = headers;
                                self.current_table_data = data.clone();
                                self.all_table_data = data;
                                self.current_table_name = format!("Redis {}", table_name);
                                self.total_rows = self.all_table_data.len();
                                self.current_page = 0;
                            }
                        }
                    }
                    _ => {
                        // SQL databases - use regular SELECT query with proper database context
                        let query_content = if let Some(db_name) = &database_name {
                            match conn.connection_type {
                                models::enums::DatabaseType::MySQL => {
                                    format!("USE `{}`;\nSELECT * FROM `{}` LIMIT 100;", db_name, table_name)
                                }
                                models::enums::DatabaseType::PostgreSQL => {
                                    format!("SELECT * FROM \"{}\".\"{}\" LIMIT 100;", db_name, table_name)
                                }
                                models::enums::DatabaseType::MSSQL => {
                                    // Build robust MSSQL SELECT with explicit database context
                                    Self::build_mssql_select_query(db_name.clone(), table_name.clone())
                                }
                                models::enums::DatabaseType::SQLite | models::enums::DatabaseType::Redis => {
                                    format!("SELECT * FROM `{}` LIMIT 100;", table_name)
                                }
                            }
                        } else {
                            match conn.connection_type {
                                models::enums::DatabaseType::MSSQL => Self::build_mssql_select_query("".to_string(), table_name.clone()),
                                _ => format!("SELECT * FROM `{}` LIMIT 100;", table_name)
                            }
                        };
                        
                        let tab_title = format!("Table: {}", table_name);
                        editor::create_new_tab_with_connection_and_database(self, tab_title, query_content.clone(), Some(connection_id), database_name.clone());
                        
                        // Set database context for current tab and auto-execute the query and display results in bottom
                    self.current_connection_id = Some(connection_id);
                    // Ensure the newly created tab stores selected database (important for MSSQL)
                    if let Some(dbn) = &database_name { if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) { active_tab.database_name = Some(dbn.clone()); } }
                        if let Some((headers, data)) = connection::execute_query_with_connection(self, connection_id, query_content) {
                            self.current_table_headers = headers;
                            self.current_table_data = data.clone();
                            self.all_table_data = data;
                            self.current_table_name = format!("Table: {} (Database: {})", table_name, database_name.as_deref().unwrap_or("Unknown"));
                            self.total_rows = self.all_table_data.len();
                            self.current_page = 0;
                        }
                    }
                };
            }
        }
        
        // Handle query file open requests
        let results = query_files_to_open.clone();
        for (filename, content, file_path) in query_files_to_open {
            debug!("📂 Processing file: {} (path: {})", filename, file_path);
            if file_path.is_empty() {
                // This is a placeholder query without a file path - create a new unsaved tab
                debug!("📝 Creating new tab for placeholder query: {}", filename);
                editor::create_new_tab(self, filename, content);
            } else {
                // Use existing open_query_file logic which checks for already open tabs
                debug!("📁 Opening query file: {}", file_path);
                if let Err(err) = sidebar_query::open_query_file(self, &file_path) {
                    debug!("❌ Failed to open query file: {}", err);
                } else {
                    debug!("✅ Successfully opened query file: {}", file_path);
                }
            }
        }
        
        // Handle context menu requests (deduplicate to avoid multiple calls)
        let mut processed_removals = std::collections::HashSet::new();
        let mut processed_refreshes = std::collections::HashSet::new();
        let mut needs_full_refresh = false;
                
        for context_id in context_menu_requests {
            debug!("🔍 Processing context_id: {}", context_id);
            
            if context_id >= 50000 {
                // ID >= 50000 means create folder in folder operation
                let hash = context_id - 50000;
                debug!("📁 Create folder operation with hash: {}", hash);
                self.handle_create_folder_in_folder_request(hash);
                // Force immediate UI repaint after create folder request
                ui.ctx().request_repaint();
            } else if context_id >= 40000 {
                // ID >= 40000 means move query to folder operation
                let hash = context_id - 40000;
                debug!("📦 Move query operation with hash: {}", hash);
                sidebar_query::handle_query_move_request(self, hash);
            } else if context_id >= 30000 {
                // ID >= 30000 means alter table operation
                let connection_id = context_id - 30000;
                debug!("🔧 Alter table operation for connection: {}", connection_id);
                self.handle_alter_table_request(connection_id);
            } else if context_id >= 20000 {
                // ID >= 20000 means query edit operation
                let hash = context_id - 20000;
                debug!("✏️ Query edit operation with hash: {}", hash);
                sidebar_query::handle_query_edit_request(self, hash);
            } else if context_id <= -50000 {
                // ID <= -50000 means remove folder operation
                let hash = (-context_id) - 50000;
                debug!("🗑️ Remove folder operation with hash: {}", hash);
                self.handle_remove_folder_request(hash);
                // Force immediate UI repaint after folder removal
                ui.ctx().request_repaint();
            } else if context_id <= -20000 {
                // ID <= -20000 means query removal operation  
                let hash = (-context_id) - 20000;
                debug!("🗑️ Remove query operation with hash: {}", hash);
                if sidebar_query::handle_query_remove_request_by_hash(self, hash) {
                    // Force refresh of queries tree if removal was successful
                    sidebar_query::load_queries_from_directory(self);
                    
                    // Force immediate UI repaint - this is crucial!
                    ui.ctx().request_repaint();
                    
                    // Set needs_refresh flag to ensure UI updates
                    self.needs_refresh = true;
                    
                }
            } else if context_id > 10000 {
                // ID > 10000 means copy connection (connection_id = context_id - 10000)
                let connection_id = context_id - 10000;
                debug!("📋 Copy connection operation for connection: {}", connection_id);
                sidebar_database::copy_connection(self, connection_id);
                
                // Force immediate tree refresh and UI update
                self.items_tree.clear();
                sidebar_database::refresh_connections_tree(self);
                needs_full_refresh = true;
                ui.ctx().request_repaint();
                
                // Break early to prevent further processing
                break;
            } else if (1000..10000).contains(&context_id) {
                // ID 1000-9999 means refresh connection (connection_id = context_id - 1000)
                let connection_id = context_id - 1000;
                debug!("🔄 Refresh connection operation for connection: {}", connection_id);
                if !processed_refreshes.contains(&connection_id) {
                    processed_refreshes.insert(connection_id);
                    self.refresh_connection(connection_id);
                    needs_full_refresh = true;
                }
            } else if context_id > 0 {
                // Positive ID means edit connection
                sidebar_database::start_edit_connection(self, context_id);
            } else {
                // Negative ID means remove connection
                let connection_id = -context_id;
                if !processed_removals.contains(&connection_id) {
                    processed_removals.insert(connection_id);
                    connection::remove_connection(self, connection_id);
                    
                    // Force immediate tree refresh and UI update
                    self.items_tree.clear();
                    sidebar_database::refresh_connections_tree(self);
                    needs_full_refresh = true;
                    ui.ctx().request_repaint();
                    
                    // Break early to prevent further processing
                    break;
                }
            }
        }
        
        // Force complete UI refresh after any removal
        if needs_full_refresh {
            // Completely clear and rebuild the tree
            self.items_tree.clear();
            sidebar_database::refresh_connections_tree(self);
            self.needs_refresh = true; // Set flag for next update cycle
            ui.ctx().request_repaint();
            
            // Return early to prevent any further processing of the old tree
            return Vec::new();
        }
        
        // Clean up processed folder removal mappings (optional - only if we want to prevent memory buildup)
        // We could also keep them for potential retry scenarios
        
        // Return query files that were clicked
        results
    }

    fn render_tree_node_with_table_expansion(
            ui: &mut egui::Ui, node: &mut models::structs::TreeNode, 
            editor_text: &mut String, node_index: usize, 
            refreshing_connections: &std::collections::HashSet<i64>
        ) -> (
            Option<models::structs::ExpansionRequest>, Option<(usize, i64, String)>, 
            Option<i64>, Option<(i64, String)>, Option<i64>, 
            Option<(String, String, String)>, Option<String>, Option<String>, // Add parent folder for creation
            Option<(i64, String)> // Add mapping for folder removal: (hash, folder_path)
        ) {
        let has_children = !node.children.is_empty();
        let mut expansion_request = None;
        let mut table_expansion = None;
        let mut context_menu_request = None;
        let mut table_click_request = None;
        let mut folder_removal_mapping: Option<(i64, String)> = None;
        let mut connection_click_request = None;
        let mut query_file_to_open = None;
        let mut folder_name_for_removal = None;
        let mut parent_folder_for_creation = None;
        
    if has_children || node.node_type == models::enums::NodeType::Connection || node.node_type == models::enums::NodeType::Table || 
       node.node_type == models::enums::NodeType::View ||
       node.node_type == models::enums::NodeType::DatabasesFolder || node.node_type == models::enums::NodeType::TablesFolder ||
       node.node_type == models::enums::NodeType::ViewsFolder || node.node_type == models::enums::NodeType::StoredProceduresFolder ||
       node.node_type == models::enums::NodeType::UserFunctionsFolder || node.node_type == models::enums::NodeType::TriggersFolder ||
       node.node_type == models::enums::NodeType::EventsFolder || node.node_type == models::enums::NodeType::DBAViewsFolder ||
       node.node_type == models::enums::NodeType::UsersFolder || node.node_type == models::enums::NodeType::PrivilegesFolder ||
       node.node_type == models::enums::NodeType::ProcessesFolder || node.node_type == models::enums::NodeType::StatusFolder ||
       node.node_type == models::enums::NodeType::Database || node.node_type == models::enums::NodeType::QueryFolder {
            // Use more unique ID including connection_id for connections
            let unique_id = match node.node_type {
                models::enums::NodeType::Connection => format!("conn_{}_{}", node_index, node.connection_id.unwrap_or(0)),
                _ => format!("node_{}_{:?}", node_index, node.node_type),
            };
            let id = egui::Id::new(&unique_id);
            ui.horizontal(|ui| {
                // Painter-drawn triangle toggle (no font dependency)
                if Self::triangle_toggle(ui, node.is_expanded).clicked() {
                    node.is_expanded = !node.is_expanded;
                    
                    // If this is a connection node and not loaded, request expansion
                    if node.node_type == models::enums::NodeType::Connection && !node.is_loaded && node.is_expanded {
                        if let Some(conn_id) = node.connection_id {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: models::enums::NodeType::Connection,
                                connection_id: conn_id,
                                database_name: None,
                            });
                            // Also set as active connection when expanding
                            connection_click_request = Some(conn_id);
                        }
                    }
                    
                    // If this is a table or view node and not loaded, request column expansion
                    if (node.node_type == models::enums::NodeType::Table || node.node_type == models::enums::NodeType::View) && !node.is_loaded && node.is_expanded {
                        if let Some(conn_id) = node.connection_id {
                            table_expansion = Some((node_index, conn_id, node.name.clone()));
                        }
                    }
                    
                    // If this is a folder node and not loaded, request folder content expansion
                    if (node.node_type == models::enums::NodeType::DatabasesFolder ||
                        node.node_type == models::enums::NodeType::TablesFolder || 
                        node.node_type == models::enums::NodeType::ViewsFolder ||
                        node.node_type == models::enums::NodeType::StoredProceduresFolder ||
                        node.node_type == models::enums::NodeType::UserFunctionsFolder ||
                        node.node_type == models::enums::NodeType::TriggersFolder ||
                        node.node_type == models::enums::NodeType::EventsFolder) && 
                       !node.is_loaded && node.is_expanded {
                        if let Some(conn_id) = node.connection_id {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: node.node_type.clone(),
                                connection_id: conn_id,
                                database_name: node.database_name.clone(),
                            });
                        }
                    }
                    
                    // If this is a Database node and not loaded, request database expansion (for Redis keys)
                    if node.node_type == models::enums::NodeType::Database && !node.is_loaded && node.is_expanded {
                        if let Some(conn_id) = node.connection_id {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: models::enums::NodeType::Database,
                                connection_id: conn_id,
                                database_name: node.database_name.clone(),
                            });
                        }
                    }
                }
                
                let icon = match node.node_type {
                    models::enums::NodeType::Database => "🗄",
                    models::enums::NodeType::Table => "📋",
                    models::enums::NodeType::Column => "📄",
                    models::enums::NodeType::Query => "🔍",
                    models::enums::NodeType::QueryHistItem => "📜",
                    models::enums::NodeType::Connection => "",
                    models::enums::NodeType::DatabasesFolder => "📁",
                    models::enums::NodeType::TablesFolder => "📋",
                    models::enums::NodeType::ViewsFolder => "👁",
                    models::enums::NodeType::StoredProceduresFolder => "⚙️",
                    models::enums::NodeType::UserFunctionsFolder => "🔧",
                    models::enums::NodeType::TriggersFolder => "⚡",
                    models::enums::NodeType::EventsFolder => "📅",
                    models::enums::NodeType::DBAViewsFolder => "👨‍💼",
                    models::enums::NodeType::UsersFolder => "👥",
                    models::enums::NodeType::PrivilegesFolder => "🔒",
                    models::enums::NodeType::ProcessesFolder => "⚡",
                    models::enums::NodeType::StatusFolder => "📊",
                    models::enums::NodeType::View => "👁",
                    models::enums::NodeType::StoredProcedure => "⚙️",
                    models::enums::NodeType::UserFunction => "🔧",
                    models::enums::NodeType::Trigger => "⚡",
                    models::enums::NodeType::Event => "📅",
                    models::enums::NodeType::MySQLFolder => "🐬",
                    models::enums::NodeType::PostgreSQLFolder => "🐘",
                    models::enums::NodeType::SQLiteFolder => "📄",
                    models::enums::NodeType::RedisFolder => "🔴",
                    models::enums::NodeType::CustomFolder => "📁",
                    models::enums::NodeType::QueryFolder => "📂",
                    models::enums::NodeType::HistoryDateFolder => "📅",
                    models::enums::NodeType::MSSQLFolder => "🧰",
                };
                
                let label_text = if icon.is_empty() { 
                    // For connection nodes, add loading indicator if refreshing
                    if node.node_type == models::enums::NodeType::Connection {
                        if let Some(conn_id) = node.connection_id {
                            if refreshing_connections.contains(&conn_id) {
                                format!("{} 🔄", node.name) // Add refresh spinner
                            } else {
                                node.name.clone()
                            }
                        } else {
                            node.name.clone()
                        }
                    } else {
                        node.name.clone()
                    }
                } else { 
                    format!("{} {}", icon, node.name) 
                };
                let response = if node.node_type == models::enums::NodeType::Connection {
                    // Use button for connections to make them more clickable
                    ui.button(&label_text)
                } else {
                    ui.label(label_text)
                };
                
                // Handle clicks on connection labels to set active connection
                if node.node_type == models::enums::NodeType::Connection && response.clicked() {
                    if let Some(conn_id) = node.connection_id {
                        connection_click_request = Some(conn_id);
                    }
                }
                
                // Handle clicks on table/view labels to load data - open in new tab
                if (node.node_type == models::enums::NodeType::Table || node.node_type == models::enums::NodeType::View) && response.clicked() {
                    if let Some(conn_id) = node.connection_id {
                        table_click_request = Some((conn_id, node.name.clone()));
                    }
                }
                
                // Add context menu for connection nodes
                if node.node_type == models::enums::NodeType::Connection {
                    response.context_menu(|ui| {
                        if ui.button("Copy Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 10000); // Use +10000 to indicate copy
                            }
                            ui.close_menu();
                        }
                        if ui.button("Edit Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id);
                            }
                            ui.close_menu();
                        }
                        if ui.button("Remove Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(-conn_id); // Negative ID indicates removal
                            }
                            ui.close_menu();
                        }
                        if ui.button("Refresh").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(1000 + conn_id); // Add to 1000 base for refresh (range 1001-9999)
                            }
                            ui.close_menu();
                        }
                    });
                }
                
                // Add context menu for folder nodes
                if node.node_type == models::enums::NodeType::QueryFolder {
                    response.context_menu(|ui| {
                        if ui.button("📁 Create New Folder").clicked() {
                            // Store the parent folder name for creation
                            parent_folder_for_creation = Some(node.name.clone());
                            // Use ID range 50000+ for create folder in folder operations
                            let create_in_folder_id = 50000 + (node.name.len() as i64 % 1000);
                            context_menu_request = Some(create_in_folder_id);
                            ui.close_menu();
                        }
                        
                        if ui.button("🗑️ Remove Folder").clicked() {
                            // Store the full folder path for removal (relative to query dir)
                            if let Some(full_path) = &node.file_path {
                                let query_dir = directory::get_query_dir();
                                // Get relative path from query directory
                                let relative_path = std::path::Path::new(full_path)
                                    .strip_prefix(&query_dir)
                                    .unwrap_or(std::path::Path::new(&node.name))
                                    .to_string_lossy()
                                    .to_string();
                                folder_name_for_removal = Some(relative_path.clone());
                                
                                // Use ID range -50000 for remove folder operations
                                let remove_folder_id = -50000 - (node.name.len() as i64 % 1000);
                                let hash = (-remove_folder_id) - 50000;
                                folder_removal_mapping = Some((hash, relative_path));
                                context_menu_request = Some(remove_folder_id);
                            } else {
                                // Fallback to just folder name for root folders
                                let folder_name = node.name.clone();
                                folder_name_for_removal = Some(folder_name.clone());
                                
                                // Use ID range -50000 for remove folder operations
                                let remove_folder_id = -50000 - (node.name.len() as i64 % 1000);
                                let hash = (-remove_folder_id) - 50000;
                                folder_removal_mapping = Some((hash, folder_name));
                                context_menu_request = Some(remove_folder_id);
                            }
                            ui.close_menu();
                        }
                    });
                }
                
                // Add context menu for table nodes
                if node.node_type == models::enums::NodeType::Table {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                table_click_request = Some((conn_id, node.name.clone()));
                            }
                            ui.close_menu();
                        }
                        if ui.button("📋 SELECT Query (New Tab)").clicked() {
                            // We'll create a new tab instead of modifying current editor
                            // Store the request and handle it in render_tree
                            ui.close_menu();
                        }
                        if ui.button("🔍 COUNT Query (Current Tab)").clicked() {
                            *editor_text = format!("SELECT COUNT(*) FROM {};", node.name);
                            ui.close_menu();
                        }
                        if ui.button("📝 DESCRIBE Query (Current Tab)").clicked() {
                            // Different DESCRIBE syntax for different database types
                            if node.database_name.is_some() {
                                *editor_text = format!("DESCRIBE {};", node.name);
                            } else {
                                *editor_text = format!("PRAGMA table_info({});", node.name); // SQLite syntax
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("🔧 Alter Table").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use connection_id + 30000 to indicate alter table request
                                context_menu_request = Some(conn_id + 30000);
                            }
                            ui.close_menu();
                        }
                    });
                }
                
                // Add context menu for view nodes
                if node.node_type == models::enums::NodeType::View {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                table_click_request = Some((conn_id, node.name.clone()));
                            }
                            ui.close_menu();
                        }
                        if ui.button("📋 SELECT Query (New Tab)").clicked() {
                            // We'll create a new tab instead of modifying current editor  
                            // Store the request and handle it in render_tree
                            ui.close_menu();
                        }
                        if ui.button("🔍 COUNT Query (Current Tab)").clicked() {
                            *editor_text = format!("SELECT COUNT(*) FROM {};", node.name);
                            ui.close_menu();
                        }
                        if ui.button("📝 DESCRIBE View (Current Tab)").clicked() {
                            // Different DESCRIBE syntax for different database types
                            if node.database_name.is_some() {
                                *editor_text = format!("DESCRIBE {};", node.name);
                            } else {
                                *editor_text = format!("PRAGMA table_info({});", node.name); // SQLite syntax
                            }
                            ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("🗂️ Show Columns").clicked() {
                            // Trigger table expansion to show columns
                            if let Some(conn_id) = node.connection_id {
                                table_expansion = Some((0, conn_id, node.name.clone()));
                            }
                            ui.close_menu();
                        }
                    });
                }
            });

            if node.is_expanded {
                // Khusus HistoryDateFolder: render children tanpa indent tambahan (full width)
                let is_history_date_folder = node.node_type == models::enums::NodeType::HistoryDateFolder;
                if is_history_date_folder {
                    for (child_index, child) in node.children.iter_mut().enumerate() {
                        let (child_expansion_request, child_table_expansion, child_context, child_table_click, _child_connection_click, _child_query_file, _child_folder_removal, _child_parent_creation, _child_folder_removal_mapping) = Self::render_tree_node_with_table_expansion(ui, child, editor_text, child_index, refreshing_connections);
                        if let Some(child_expansion) = child_expansion_request { expansion_request = Some(child_expansion); }
                        if table_expansion.is_none() { if let Some((child_index, child_conn_id, table_name)) = child_table_expansion { if let Some(conn_id) = node.connection_id { table_expansion = Some((child_index, conn_id, table_name)); } else { table_expansion = Some((child_index, child_conn_id, table_name)); } } }
                        if let Some((conn_id, table_name)) = child_table_click { table_click_request = Some((conn_id, table_name)); }
                        if let Some(child_context_id) = child_context { context_menu_request = Some(child_context_id); }
                    }
                } else {
                    ui.indent(id, |ui| {
                    for (child_index, child) in node.children.iter_mut().enumerate() {
                        let (child_expansion_request, child_table_expansion, child_context, child_table_click, _child_connection_click, _child_query_file, _child_folder_removal, _child_parent_creation, _child_folder_removal_mapping) = Self::render_tree_node_with_table_expansion(ui, child, editor_text, child_index, refreshing_connections);
                        
                        // Handle child expansion requests - propagate to parent
                        if let Some(child_expansion) = child_expansion_request {
                            expansion_request = Some(child_expansion);
                        }
                        
                        // Handle child table expansions with the parent connection ID
                        // Only set if we don't already have a table expansion from this node
                        if table_expansion.is_none() {
                            if let Some((child_index, child_conn_id, table_name)) = child_table_expansion {
                                if let Some(conn_id) = node.connection_id {
                                    table_expansion = Some((child_index, conn_id, table_name));
                                } else {
                                    table_expansion = Some((child_index, child_conn_id, table_name));
                                }
                            }
                        }
                        
                        // Handle child table clicks - propagate to parent
                        if let Some((conn_id, table_name)) = child_table_click {
                            table_click_request = Some((conn_id, table_name));
                        }
                        
                        // Handle child folder removal - propagate to parent
                        if let Some(child_folder_name) = _child_folder_removal {
                            folder_name_for_removal = Some(child_folder_name);
                        }
                        
                        // Handle child parent folder creation - propagate to parent
                        if let Some(child_parent) = _child_parent_creation {
                            parent_folder_for_creation = Some(child_parent);
                        }
                        
                        // Handle child folder removal mapping - propagate to parent
                        if let Some(child_mapping) = _child_folder_removal_mapping {
                            folder_removal_mapping = Some(child_mapping);
                        }
                        
                        // Handle child query file open requests - propagate to parent
                        if let Some(child_query_file) = _child_query_file {
                            query_file_to_open = Some(child_query_file);
                        }
                        
                        // Handle child context menu requests - propagate to parent
                        if let Some(child_context_id) = child_context {
                            context_menu_request = Some(child_context_id);
                        }
                    }
                });
                }
            }
        } else {
            let response = if node.node_type == models::enums::NodeType::QueryHistItem {
                // Special handling for history items - text only, completely left aligned with no indentation
                let available_width = ui.available_width();
                ui.add_sized(
                    [available_width, ui.text_style_height(&egui::TextStyle::Body) * 3.0], // Allow up to 3 lines
                    egui::SelectableLabel::new(false, &node.name)
                )
            } else {
                // For all other node types, use horizontal layout with icons
                ui.horizontal(|ui| {
                    let icon = match node.node_type {
                        models::enums::NodeType::Database => "�",
                        models::enums::NodeType::Table => "�",
                        models::enums::NodeType::Column => "�",
                        models::enums::NodeType::Query => "�",
                        models::enums::NodeType::Connection => "🔗",
                        models::enums::NodeType::DatabasesFolder => "📁",
                        models::enums::NodeType::TablesFolder => "📋",
                        models::enums::NodeType::ViewsFolder => "👁",
                        models::enums::NodeType::StoredProceduresFolder => "⚙️",
                        models::enums::NodeType::UserFunctionsFolder => "🔧",
                        models::enums::NodeType::TriggersFolder => "⚡",
                        models::enums::NodeType::EventsFolder => "📅",
                        models::enums::NodeType::DBAViewsFolder => "👨‍💼",
                        models::enums::NodeType::UsersFolder => "👥",
                        models::enums::NodeType::PrivilegesFolder => "🔒",
                        models::enums::NodeType::ProcessesFolder => "⚡",
                        models::enums::NodeType::StatusFolder => "📊",
                        models::enums::NodeType::View => "👁",
                        models::enums::NodeType::StoredProcedure => "⚙️",
                        models::enums::NodeType::UserFunction => "🔧",
                        models::enums::NodeType::Trigger => "⚡",
                        models::enums::NodeType::Event => "📅",
                        models::enums::NodeType::MySQLFolder => "🐬",
                        models::enums::NodeType::PostgreSQLFolder => "🐘",
                        models::enums::NodeType::SQLiteFolder => "📄",
                        models::enums::NodeType::RedisFolder => "🔴",
                        models::enums::NodeType::CustomFolder => "📁",
                        models::enums::NodeType::QueryFolder => "📂",
                        models::enums::NodeType::HistoryDateFolder => "📅",
                        _ => "❓",
                    };
                    
                    ui.button(format!("{} {}", icon, node.name))
                }).inner
            };
            
            if response.clicked() {
                // Handle node selection
                match node.node_type {
                        models::enums::NodeType::Table | models::enums::NodeType::View => {
                            // Don't modify current editor_text, we'll create a new tab
                            // Just trigger table data loading 
                            if let Some(conn_id) = node.connection_id {
                                table_click_request = Some((conn_id, node.name.clone()));
                            }
                        },
                        models::enums::NodeType::Query => {
                            // Load query file content
                            debug!("🔍 Query node clicked: {}", node.name);
                            if let Some(file_path) = &node.file_path {
                                debug!("📁 File path: {}", file_path);
                                if let Ok(content) = std::fs::read_to_string(file_path) {
                                    debug!("✅ File read successfully, content length: {}", content.len());
                                    // Don't modify editor_text directly, let open_query_file handle it
                                    query_file_to_open = Some((node.name.clone(), content, file_path.clone()));
                                } else {
                                    debug!("❌ Failed to read file: {}", file_path);
                                    // Handle read error case
                                    query_file_to_open = Some((node.name.clone(), format!("-- Failed to load query file: {}", node.name), file_path.clone()));
                                }
                            } else {
                                debug!("❌ No file path for query node: {}", node.name);
                                // Handle missing file path case - create a placeholder query
                                let placeholder_content = format!("-- {}\nSELECT * FROM table_name;", node.name);
                                // For files without path, we'll create a new unsaved tab
                                query_file_to_open = Some((node.name.clone(), placeholder_content, String::new()));
                            }
                        },
                        models::enums::NodeType::QueryHistItem => {
                            // Store the display name for processing later
                            *editor_text = node.name.clone();
                        },
                        _ => {}
                    }
                }
                
                // Add context menu for query nodes
                if node.node_type == models::enums::NodeType::Query {
                    response.context_menu(|ui| {
                        if ui.button("Edit Query").clicked() {
                            if let Some(file_path) = &node.file_path {
                                // Use the file path directly as context identifier
                                // Format: 20000 + simple index to differentiate from connections
                                let edit_id = 20000 + (file_path.len() as i64 % 1000); // Simple deterministic ID
                                context_menu_request = Some(edit_id);
                            }
                            ui.close_menu();
                        }
                        
                        if ui.button("Move to Folder").clicked() {
                            if let Some(file_path) = &node.file_path {
                                // Use a different ID range for move operations
                                let move_id = 40000 + (file_path.len() as i64 % 1000);
                                context_menu_request = Some(move_id);
                            }
                            ui.close_menu();
                        }
                        
                        if ui.button("Remove Query").clicked() {
                            if let Some(file_path) = &node.file_path {
                                // Use the file path directly as context identifier
                                // Format: -20000 - simple index to differentiate from connections
                                let remove_id = -20000 - (file_path.len() as i64 % 1000); // Simple deterministic ID
                                context_menu_request = Some(remove_id);
                            }
                            ui.close_menu();
                        }
                    });
                }
        }
        
        (expansion_request, table_expansion, context_menu_request, table_click_request, connection_click_request, query_file_to_open, folder_name_for_removal, parent_folder_for_creation, folder_removal_mapping)
    }


    fn handle_alter_table_request(&mut self, connection_id: i64) {
        debug!("🔍 handle_alter_table_request called with connection_id: {}", connection_id);
        
        // Find the connection by ID to determine database type
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            // Find the currently selected table in the tree
            if let Some(table_name) = self.find_selected_table_name(connection_id) {
                // Generate ALTER TABLE template based on database type
                let alter_template = match connection.connection_type {
                    models::enums::DatabaseType::MySQL => self.generate_mysql_alter_table_template(&table_name),
                    models::enums::DatabaseType::PostgreSQL => self.generate_postgresql_alter_table_template(&table_name),
                    models::enums::DatabaseType::SQLite => self.generate_sqlite_alter_table_template(&table_name),
                    models::enums::DatabaseType::Redis => "-- Redis does not support ALTER TABLE operations\n-- Redis is a key-value store, not a relational database".to_string(),
                    models::enums::DatabaseType::MSSQL => self.generate_mysql_alter_table_template(&table_name).replace("MySQL", "MSSQL"),
                };
                
                // Set the ALTER TABLE template in the editor
                self.editor_text = alter_template;
                self.current_connection_id = Some(connection_id);
                
            } else {
                // If no specific table is selected, show a generic template
                let alter_template = match connection.connection_type {
                    models::enums::DatabaseType::MySQL => "-- MySQL ALTER TABLE template\nALTER TABLE your_table_name\n  ADD COLUMN new_column VARCHAR(255),\n  MODIFY COLUMN existing_column INT,\n  DROP COLUMN old_column;".to_string(),
                    models::enums::DatabaseType::PostgreSQL => "-- PostgreSQL ALTER TABLE template\nALTER TABLE your_table_name\n  ADD COLUMN new_column VARCHAR(255),\n  ALTER COLUMN existing_column TYPE INTEGER,\n  DROP COLUMN old_column;".to_string(),
                    models::enums::DatabaseType::SQLite => "-- SQLite ALTER TABLE template\n-- Note: SQLite has limited ALTER TABLE support\nALTER TABLE your_table_name\n  ADD COLUMN new_column TEXT;".to_string(),
                    models::enums::DatabaseType::Redis => "-- Redis does not support ALTER TABLE operations\n-- Redis is a key-value store, not a relational database\n-- Use Redis commands like SET, GET, HSET, etc.".to_string(),
                    models::enums::DatabaseType::MSSQL => "-- MSSQL ALTER TABLE template\nALTER TABLE your_table_name\n  ADD new_column VARCHAR(255) NULL,\n  ALTER COLUMN existing_column INT,\n  DROP COLUMN old_column;".to_string(),
                };
                
                self.editor_text = alter_template;
                self.current_connection_id = Some(connection_id);
                
            }
        } else {
            debug!("❌ Connection with ID {} not found", connection_id);
        }
    }

    fn find_selected_table_name(&self, _connection_id: i64) -> Option<String> {
        // This is a simplified approach - in a more sophisticated implementation,
        // you might track which table was right-clicked
        // For now, we'll return None to show the generic template
        None
    }

    fn generate_mysql_alter_table_template(&self, table_name: &str) -> String {
        format!(
            "-- MySQL ALTER TABLE for {}\nALTER TABLE {}\n  ADD COLUMN new_column VARCHAR(255) DEFAULT NULL COMMENT 'New column description',\n  MODIFY COLUMN existing_column INT NOT NULL,\n  DROP COLUMN old_column,\n  ADD INDEX idx_new_column (new_column);",
            table_name, table_name
        )
    }

    fn generate_postgresql_alter_table_template(&self, table_name: &str) -> String {
        format!(
            "-- PostgreSQL ALTER TABLE for {}\nALTER TABLE {}\n  ADD COLUMN new_column VARCHAR(255) DEFAULT NULL,\n  ALTER COLUMN existing_column TYPE INTEGER,\n  DROP COLUMN old_column;\n\n-- Add constraint example\n-- ALTER TABLE {} ADD CONSTRAINT chk_constraint CHECK (new_column IS NOT NULL);",
            table_name, table_name, table_name
        )
    }

    fn generate_sqlite_alter_table_template(&self, table_name: &str) -> String {
        format!(
            "-- SQLite ALTER TABLE for {}\n-- Note: SQLite has limited ALTER TABLE support\n-- Only ADD COLUMN and RENAME operations are supported\n\nALTER TABLE {} ADD COLUMN new_column TEXT DEFAULT NULL;\n\n-- To modify or drop columns, you need to recreate the table:\n-- CREATE TABLE {}_new AS SELECT existing_columns FROM {};\n-- DROP TABLE {};\n-- ALTER TABLE {}_new RENAME TO {};",
            table_name, table_name, table_name, table_name, table_name, table_name, table_name
        )
    }

    fn handle_create_folder_in_folder_request(&mut self, _hash: i64) {
        debug!("🔍 handle_create_folder_in_folder_request called with hash: {}", _hash);
        // Parent folder should already be set when context menu was clicked
        if self.parent_folder_for_creation.is_some() {
            // Show the create folder dialog
            self.show_create_folder_dialog = true;
        } else {
            debug!("❌ No parent folder set for creation! This should not happen.");
            self.error_message = "No parent folder selected for creation".to_string();
            self.show_error_message = true;
        }
    }

    fn handle_remove_folder_request(&mut self, hash: i64) {
        
        // Look up the folder path using the hash
        if let Some(folder_relative_path) = self.folder_removal_map.get(&hash).cloned() {
            let query_dir = directory::get_query_dir();
            let folder_path = query_dir.join(&folder_relative_path);
            
            
            if folder_path.exists() && folder_path.is_dir() {
                // Check if folder is empty (recursively)
                let is_empty = Self::is_directory_empty(&folder_path);
                
                if is_empty {
                    // Remove empty folder
                    match std::fs::remove_dir(&folder_path) {
                        Ok(()) => {
                            // Refresh the queries tree
                            sidebar_query::load_queries_from_directory(self);
                            // Force UI refresh
                            self.needs_refresh = true;
                        }
                        Err(e) => {
                            debug!("❌ Failed to remove folder: {}", e);
                            self.error_message = format!("Failed to remove folder '{}': {}", folder_relative_path, e);
                            self.show_error_message = true;
                        }
                    }
                } else {
                    // Offer option to remove folder and all contents
                    self.error_message = format!("Folder '{}' is not empty.\n\nWould you like to remove it and all its contents?", folder_relative_path);
                    self.show_error_message = true;
                    debug!("❌ Cannot remove non-empty folder: {}", folder_relative_path);
                }
            } else {
                self.error_message = format!("Folder '{}' does not exist", folder_relative_path);
                self.show_error_message = true;
                debug!("❌ Folder does not exist: {}", folder_relative_path);
            }
            
            // Remove the mapping after processing
            self.folder_removal_map.remove(&hash);
        } else {
            debug!("❌ No folder path found for hash: {}", hash);
            debug!("❌ Available mappings: {:?}", self.folder_removal_map);
            // Fallback to the old method
            if let Some(folder_relative_path) = &self.selected_folder_for_removal {
                let query_dir = directory::get_query_dir();
                let folder_path = query_dir.join(folder_relative_path);
                
                
                if folder_path.exists() && folder_path.is_dir() {
                    let is_empty = Self::is_directory_empty(&folder_path);
                    
                    if is_empty {
                        match std::fs::remove_dir(&folder_path) {
                            Ok(()) => {
                                sidebar_query::load_queries_from_directory(self);
                                self.needs_refresh = true;
                            }
                            Err(e) => {
                                debug!("❌ Failed to remove folder: {}", e);
                                self.error_message = format!("Failed to remove folder '{}': {}", folder_relative_path, e);
                                self.show_error_message = true;
                            }
                        }
                    } else {
                        self.error_message = format!("Folder '{}' is not empty.\n\nWould you like to remove it and all its contents?", folder_relative_path);
                        self.show_error_message = true;
                        debug!("❌ Cannot remove non-empty folder: {}", folder_relative_path);
                    }
                } else {
                    self.error_message = format!("Folder '{}' does not exist", folder_relative_path);
                    self.show_error_message = true;
                    debug!("❌ Folder does not exist: {}", folder_relative_path);
                }
                
                self.selected_folder_for_removal = None;
            } else {
                debug!("❌ No folder selected for removal in fallback either");
            }
        }
    }

    fn is_directory_empty(dir_path: &std::path::Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            entries.count() == 0
        } else {
            false
        }
    }

    fn find_connection_node_recursive(nodes: &mut [models::structs::TreeNode], connection_id: i64) -> Option<&mut models::structs::TreeNode> {
        for node in nodes.iter_mut() {
            // Check if this is the connection node we're looking for
            if node.node_type == models::enums::NodeType::Connection && 
               node.connection_id == Some(connection_id) {
                return Some(node);
            }
            
            // Recursively search in children
            if !node.children.is_empty() {
                if let Some(found) = Self::find_connection_node_recursive(&mut node.children, connection_id) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn refresh_connection(&mut self, connection_id: i64) {
        
        // Clear all cached data for this connection
        self.clear_connection_cache(connection_id);
        
        // Remove from connection pool cache to force reconnection
        self.connection_pools.remove(&connection_id);
        
        // Mark as refreshing
        self.refreshing_connections.insert(connection_id);
        
        // Find the connection node in the tree and reset its loaded state
        for node in &mut self.items_tree {
            if node.node_type == models::enums::NodeType::Connection && node.connection_id == Some(connection_id) {
                // Reset the connection node
                node.is_loaded = false;
                node.is_expanded = false;
                node.children.clear();
                debug!("Reset connection node: {}", node.name);
                break;
            }
        }
        
        // Send background task instead of blocking refresh
        if let Some(sender) = &self.background_sender {
            if let Err(e) = sender.send(models::enums::BackgroundTask::RefreshConnection { connection_id }) {
                debug!("Failed to send background refresh task: {}", e);
                // Fallback to synchronous refresh if background thread is not available
                self.refreshing_connections.remove(&connection_id);
                cache_data::fetch_and_cache_connection_data(self, connection_id);
            } else {
                debug!("Background refresh task sent for connection {}", connection_id);
            }
        } else {
            // Fallback to synchronous refresh if background system is not initialized
            self.refreshing_connections.remove(&connection_id);
            cache_data::fetch_and_cache_connection_data(self, connection_id);
        }
    }


    // Function to clear cache for a connection (useful for refresh)
    fn clear_connection_cache(&self, connection_id: i64) {
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();
            
            rt.block_on(async {
                // Clear all cache tables for this connection
                let _ = sqlx::query("DELETE FROM database_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;
                
                let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;
                
                let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;
            });
            
        }
    }

    fn load_connection_tables(&mut self, connection_id: i64, node: &mut models::structs::TreeNode) {

        debug!("Loading connection tables for ID: {}", connection_id);

        // First check if we have cached data
        if let Some(databases) = cache_data::get_databases_from_cache(self, connection_id) {
            debug!("Found cached databases for connection {}: {:?}", connection_id, databases);
            if !databases.is_empty() {
                self.build_connection_structure_from_cache(connection_id, node, &databases);
                node.is_loaded = true;
                return;
            }
        }

        debug!("🔄 Cache empty or not found, fetching databases from server for connection {}", connection_id);
        
        // Try to fetch from actual database server
        if let Some(fresh_databases) = connection::fetch_databases_from_connection(self, connection_id) {
            debug!("✅ Successfully fetched {} databases from server", fresh_databases.len());
            // Save to cache for future use
            cache_data::save_databases_to_cache(self, connection_id, &fresh_databases);
            // Build structure from fresh data
            self.build_connection_structure_from_cache(connection_id, node, &fresh_databases);
            node.is_loaded = true;
            return;
        } else {
            debug!("❌ Failed to fetch databases from server, creating default structure");
        }

        
        // Find the connection by ID
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            let connection = connection.clone();

            // Create the main structure based on database type
            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    driver_mysql::load_mysql_structure(connection_id, &connection, node);
                },
                models::enums::DatabaseType::PostgreSQL => {
                    driver_postgres::load_postgresql_structure(connection_id, &connection, node);
                },
                models::enums::DatabaseType::SQLite => {
                    driver_sqlite::load_sqlite_structure(connection_id, &connection, node);
                },
                models::enums::DatabaseType::Redis => {
                    driver_redis::load_redis_structure(self, connection_id, &connection, node);
                },
                models::enums::DatabaseType::MSSQL => {
                    crate::driver_mssql::load_mssql_structure(connection_id, &connection, node);
                },
            }
            node.is_loaded = true;
        }
    }

    fn build_connection_structure_from_cache(&mut self, connection_id: i64, node: &mut models::structs::TreeNode, databases: &[String]) {
        // Find the connection to get its type
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            let mut main_children = Vec::new();
            
            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    // 1. Databases folder
                    let mut databases_folder = models::structs::TreeNode::new("Databases".to_string(), models::enums::NodeType::DatabasesFolder);
                    databases_folder.connection_id = Some(connection_id);
                    
                    // Add each database from cache
                    for db_name in databases {
                        // Skip system databases for cleaner view
                        if !["information_schema", "performance_schema", "mysql", "sys"].contains(&db_name.as_str()) {
                            let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                            db_node.connection_id = Some(connection_id);
                            db_node.database_name = Some(db_name.clone());
                            db_node.is_loaded = false; // Will be loaded when expanded
                            
                            // Create folder structure but don't load content yet
                            let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                            tables_folder.connection_id = Some(connection_id);
                            tables_folder.database_name = Some(db_name.clone());
                            tables_folder.is_loaded = false;
                            
                            let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                            views_folder.connection_id = Some(connection_id);
                            views_folder.database_name = Some(db_name.clone());
                            views_folder.is_loaded = false;
                            
                            let mut procedures_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                            procedures_folder.connection_id = Some(connection_id);
                            procedures_folder.database_name = Some(db_name.clone());
                            procedures_folder.is_loaded = false;
                            
                            let mut functions_folder = models::structs::TreeNode::new("Functions".to_string(), models::enums::NodeType::UserFunctionsFolder);
                            functions_folder.connection_id = Some(connection_id);
                            functions_folder.database_name = Some(db_name.clone());
                            functions_folder.is_loaded = false;
                            
                            let mut triggers_folder = models::structs::TreeNode::new("Triggers".to_string(), models::enums::NodeType::TriggersFolder);
                            triggers_folder.connection_id = Some(connection_id);
                            triggers_folder.database_name = Some(db_name.clone());
                            triggers_folder.is_loaded = false;
                            
                            let mut events_folder = models::structs::TreeNode::new("Events".to_string(), models::enums::NodeType::EventsFolder);
                            events_folder.connection_id = Some(connection_id);
                            events_folder.database_name = Some(db_name.clone());
                            events_folder.is_loaded = false;
                            
                            db_node.children = vec![
                                tables_folder,
                                views_folder,
                                procedures_folder,
                                functions_folder,
                                triggers_folder,
                                events_folder,
                            ];
                            
                            databases_folder.children.push(db_node);
                        }
                    }
                    
                    // 2. DBA Views folder
                    let mut dba_folder = models::structs::TreeNode::new("DBA Views".to_string(), models::enums::NodeType::DBAViewsFolder);
                    dba_folder.connection_id = Some(connection_id);
                    
                    let mut dba_children = Vec::new();
                    
                    // Users
                    let mut users_folder = models::structs::TreeNode::new("Users".to_string(), models::enums::NodeType::UsersFolder);
                    users_folder.connection_id = Some(connection_id);
                    users_folder.is_loaded = false;
                    dba_children.push(users_folder);
                    
                    // Privileges
                    let mut priv_folder = models::structs::TreeNode::new("Privileges".to_string(), models::enums::NodeType::PrivilegesFolder);
                    priv_folder.connection_id = Some(connection_id);
                    priv_folder.is_loaded = false;
                    dba_children.push(priv_folder);
                    
                    // Processes
                    let mut proc_folder = models::structs::TreeNode::new("Processes".to_string(), models::enums::NodeType::ProcessesFolder);
                    proc_folder.connection_id = Some(connection_id);
                    proc_folder.is_loaded = false;
                    dba_children.push(proc_folder);
                    
                    // Status
                    let mut status_folder = models::structs::TreeNode::new("Status".to_string(), models::enums::NodeType::StatusFolder);
                    status_folder.connection_id = Some(connection_id);
                    status_folder.is_loaded = false;
                    dba_children.push(status_folder);
                    
                    dba_folder.children = dba_children;
                    
                    main_children.push(databases_folder);
                    main_children.push(dba_folder);
                },
                models::enums::DatabaseType::PostgreSQL => {
                    // Similar structure for PostgreSQL
                    let mut databases_folder = models::structs::TreeNode::new("Databases".to_string(), models::enums::NodeType::DatabasesFolder);
                    databases_folder.connection_id = Some(connection_id);
                    
                    for db_name in databases {
                        if !["template0", "template1", "postgres"].contains(&db_name.as_str()) {
                            let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                            db_node.connection_id = Some(connection_id);
                            db_node.database_name = Some(db_name.clone());
                            db_node.is_loaded = false;
                            
                            let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                            tables_folder.connection_id = Some(connection_id);
                            tables_folder.database_name = Some(db_name.clone());
                            tables_folder.is_loaded = false;
                            
                            let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                            views_folder.connection_id = Some(connection_id);
                            views_folder.database_name = Some(db_name.clone());
                            views_folder.is_loaded = false;
                            
                            db_node.children = vec![tables_folder, views_folder];
                            databases_folder.children.push(db_node);
                        }
                    }
                    
                    main_children.push(databases_folder);
                },
                models::enums::DatabaseType::SQLite => {
                    // SQLite structure - single database
                    let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                    tables_folder.connection_id = Some(connection_id);
                    tables_folder.database_name = Some("main".to_string());
                    tables_folder.is_loaded = false;
                    
                    let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                    views_folder.connection_id = Some(connection_id);
                    views_folder.database_name = Some("main".to_string());
                    views_folder.is_loaded = false;
                    
                    main_children = vec![tables_folder, views_folder];
                },
                models::enums::DatabaseType::Redis => {
                    // Redis structure with databases
                    cache_data::build_redis_structure_from_cache(self, connection_id, node, databases);
                    return;
                }
                models::enums::DatabaseType::MSSQL => {
                    // Basic placeholder similar to MySQL (without DBA views for now)
                    let mut databases_folder = models::structs::TreeNode::new("Databases".to_string(), models::enums::NodeType::DatabasesFolder);
                    databases_folder.connection_id = Some(connection_id);
                    for db_name in databases {
                        let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                        db_node.connection_id = Some(connection_id);
                        db_node.database_name = Some(db_name.clone());
                        db_node.is_loaded = false;
                        let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                        tables_folder.connection_id = Some(connection_id);
                        tables_folder.database_name = Some(db_name.clone());
                        tables_folder.is_loaded = false;
                        let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                        views_folder.connection_id = Some(connection_id);
                        views_folder.database_name = Some(db_name.clone());
                        views_folder.is_loaded = false;
                        // Stored Procedures folder
                        let mut sp_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                        sp_folder.connection_id = Some(connection_id);
                        sp_folder.database_name = Some(db_name.clone());
                        sp_folder.is_loaded = false;
                        // Functions folder
                        let mut fn_folder = models::structs::TreeNode::new("Functions".to_string(), models::enums::NodeType::UserFunctionsFolder);
                        fn_folder.connection_id = Some(connection_id);
                        fn_folder.database_name = Some(db_name.clone());
                        fn_folder.is_loaded = false;
                        // Triggers folder (events not supported in MSSQL)
                        let mut trg_folder = models::structs::TreeNode::new("Triggers".to_string(), models::enums::NodeType::TriggersFolder);
                        trg_folder.connection_id = Some(connection_id);
                        trg_folder.database_name = Some(db_name.clone());
                        trg_folder.is_loaded = false;

                        db_node.children = vec![tables_folder, views_folder, sp_folder, fn_folder, trg_folder];
                        databases_folder.children.push(db_node);
                    }
                    main_children.push(databases_folder);
                }
            }
            
            node.children = main_children;
        }
    }



    // More specific function to find folder node with exact type and database name
    fn find_specific_folder_node<'a>(node: &'a mut models::structs::TreeNode, connection_id: i64, folder_type: &models::enums::NodeType, database_name: &Option<String>) -> Option<&'a mut models::structs::TreeNode> {
        // Check if this node is the folder we're looking for
        if node.node_type == *folder_type && 
           node.connection_id == Some(connection_id) && 
           node.database_name == *database_name &&
           node.is_expanded && 
           !node.is_loaded {
            return Some(node);
        }
        
        // Recursively search in children
        for child in &mut node.children {
            if let Some(result) = Self::find_specific_folder_node(child, connection_id, folder_type, database_name) {
                return Some(result);
            }
        }
        
        None
    }

    fn load_databases_for_folder(&mut self, connection_id: i64, databases_folder: &mut models::structs::TreeNode) {
        // Check connection type to handle Redis differently
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            if connection.connection_type == models::enums::DatabaseType::Redis {
                self.load_redis_databases_for_folder(connection_id, databases_folder);
                return;
            }
        }
        
        // Clear any loading placeholders
        databases_folder.children.clear();
        
        // First check cache
        if let Some(cached_databases) = cache_data::get_databases_from_cache(self, connection_id) {
            if !cached_databases.is_empty() {
                
                for db_name in cached_databases {
                    let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                    db_node.connection_id = Some(connection_id);
                    db_node.database_name = Some(db_name.clone());
                    db_node.is_loaded = false;
                    
                    // Add subfolders for each database
                    let mut db_children = Vec::new();
                    
                    // Tables folder
                    let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                    tables_folder.connection_id = Some(connection_id);
                    tables_folder.database_name = Some(db_name.clone());
                    tables_folder.is_loaded = false;
                    db_children.push(tables_folder);
                    
                    // Views folder
                    let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                    views_folder.connection_id = Some(connection_id);
                    views_folder.database_name = Some(db_name.clone());
                    views_folder.is_loaded = false;
                    db_children.push(views_folder);
                    
                    // Stored Procedures folder
                    let mut sp_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);
                    
                    db_node.children = db_children;
                    databases_folder.children.push(db_node);
                }
                
                databases_folder.is_loaded = true;
                return;
            }
        }
                
        // Try to fetch real databases from the connection
        if let Some(real_databases) = connection::fetch_databases_from_connection(self, connection_id) {
            
            // Save to cache for future use
            cache_data::save_databases_to_cache(self, connection_id, &real_databases);
            
            // Create tree nodes from fetched data
            for db_name in real_databases {
                let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;
                
                // Add subfolders for each database
                let mut db_children = Vec::new();
                
                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);
                
                // Views folder
                let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);
                
                // Stored Procedures / Functions / Triggers depending on DB type
                if let Some(conn) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
                    match conn.connection_type {
                        models::enums::DatabaseType::MySQL => {
                            let mut sp_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                            sp_folder.connection_id = Some(connection_id);
                            sp_folder.database_name = Some(db_name.clone());
                            sp_folder.is_loaded = false;
                            db_children.push(sp_folder);
                        }
                        models::enums::DatabaseType::MSSQL => {
                            let mut sp_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                            sp_folder.connection_id = Some(connection_id);
                            sp_folder.database_name = Some(db_name.clone());
                            sp_folder.is_loaded = false;
                            db_children.push(sp_folder);
                            let mut fn_folder = models::structs::TreeNode::new("Functions".to_string(), models::enums::NodeType::UserFunctionsFolder);
                            fn_folder.connection_id = Some(connection_id);
                            fn_folder.database_name = Some(db_name.clone());
                            fn_folder.is_loaded = false;
                            db_children.push(fn_folder);
                            let mut trg_folder = models::structs::TreeNode::new("Triggers".to_string(), models::enums::NodeType::TriggersFolder);
                            trg_folder.connection_id = Some(connection_id);
                            trg_folder.database_name = Some(db_name.clone());
                            trg_folder.is_loaded = false;
                            db_children.push(trg_folder);
                        }
                        _ => {}
                    }
                }
                
                db_node.children = db_children;
                databases_folder.children.push(db_node);
            }
            
            databases_folder.is_loaded = true;
        } else {
            self.populate_sample_databases_for_folder(connection_id, databases_folder);
        }
    }
    
    fn populate_sample_databases_for_folder(&mut self, connection_id: i64, databases_folder: &mut models::structs::TreeNode) {
        // Find the connection to determine type
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            let sample_databases = match connection.connection_type {
                models::enums::DatabaseType::MySQL => vec!["information_schema".to_string(), "sakila".to_string(), "world".to_string(), "test".to_string()],
                models::enums::DatabaseType::PostgreSQL => vec!["postgres".to_string(), "template1".to_string(), "dvdrental".to_string()],
                models::enums::DatabaseType::SQLite => vec!["main".to_string()],
                models::enums::DatabaseType::Redis => vec!["redis".to_string(), "info".to_string()],
                models::enums::DatabaseType::MSSQL => vec!["master".to_string(), "tempdb".to_string(), "model".to_string(), "msdb".to_string()],
            };
            
            // Clear loading message
            databases_folder.children.clear();
            
            // Add sample databases
            for db_name in sample_databases {
                // Skip system databases for display
                if matches!(connection.connection_type, models::enums::DatabaseType::MySQL) && 
                   ["information_schema", "performance_schema", "mysql", "sys"].contains(&db_name.as_str()) {
                    continue;
                }
                
                let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;
                
                // Add subfolders for each database
                let mut db_children = Vec::new();
                
                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new("Tables".to_string(), models::enums::NodeType::TablesFolder);
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);
                
                // Views folder  
                let mut views_folder = models::structs::TreeNode::new("Views".to_string(), models::enums::NodeType::ViewsFolder);
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);
                
                if matches!(connection.connection_type, models::enums::DatabaseType::MySQL) {
                    // Stored Procedures folder
                    let mut sp_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);
                    
                    // User Functions folder
                    let mut uf_folder = models::structs::TreeNode::new("User Functions".to_string(), models::enums::NodeType::UserFunctionsFolder);
                    uf_folder.connection_id = Some(connection_id);
                    uf_folder.database_name = Some(db_name.clone());
                    uf_folder.is_loaded = false;
                    db_children.push(uf_folder);
                    
                    // Triggers folder
                    let mut triggers_folder = models::structs::TreeNode::new("Triggers".to_string(), models::enums::NodeType::TriggersFolder);
                    triggers_folder.connection_id = Some(connection_id);
                    triggers_folder.database_name = Some(db_name.clone());
                    triggers_folder.is_loaded = false;
                    db_children.push(triggers_folder);
                    
                    // Events folder
                    let mut events_folder = models::structs::TreeNode::new("Events".to_string(), models::enums::NodeType::EventsFolder);
                    events_folder.connection_id = Some(connection_id);
                    events_folder.database_name = Some(db_name.clone());
                    events_folder.is_loaded = false;
                    db_children.push(events_folder);
                } else if matches!(connection.connection_type, models::enums::DatabaseType::MSSQL) {
                    // For MSSQL, add Procedures, Functions, and Triggers (no Events)
                    let mut sp_folder = models::structs::TreeNode::new("Stored Procedures".to_string(), models::enums::NodeType::StoredProceduresFolder);
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);

                    let mut fn_folder = models::structs::TreeNode::new("Functions".to_string(), models::enums::NodeType::UserFunctionsFolder);
                    fn_folder.connection_id = Some(connection_id);
                    fn_folder.database_name = Some(db_name.clone());
                    fn_folder.is_loaded = false;
                    db_children.push(fn_folder);

                    let mut trg_folder = models::structs::TreeNode::new("Triggers".to_string(), models::enums::NodeType::TriggersFolder);
                    trg_folder.connection_id = Some(connection_id);
                    trg_folder.database_name = Some(db_name.clone());
                    trg_folder.is_loaded = false;
                    db_children.push(trg_folder);
                }
                
                db_node.children = db_children;
                databases_folder.children.push(db_node);
            }
            
        }
    }
    
    fn load_redis_databases_for_folder(&mut self, connection_id: i64, databases_folder: &mut models::structs::TreeNode) {
        // Clear loading placeholders
        databases_folder.children.clear();

        // Ambil daftar database Redis dari cache
        if let Some(cached_databases) = cache_data::get_databases_from_cache(self, connection_id) {
            for db_name in cached_databases {
                if db_name.starts_with("db") {
                    let mut db_node = models::structs::TreeNode::new(db_name.clone(), models::enums::NodeType::Database);
                    db_node.connection_id = Some(connection_id);
                    db_node.database_name = Some(db_name.clone());
                    db_node.is_loaded = false;

                    // Tambahkan node child untuk key, akan di-load saat node db di-expand
                    let loading_keys_node = models::structs::TreeNode::new("Loading keys...".to_string(), models::enums::NodeType::Table);
                    db_node.children.push(loading_keys_node);

                    databases_folder.children.push(db_node);
                }
            }
            databases_folder.is_loaded = true;
        }
    }

    fn find_redis_database_node<'a>(node: &'a mut models::structs::TreeNode, connection_id: i64, database_name: &Option<String>) -> Option<&'a mut models::structs::TreeNode> {
        // Check if this is the database node we're looking for
        if node.connection_id == Some(connection_id) && 
           node.node_type == models::enums::NodeType::Database && 
           node.database_name == *database_name {
            return Some(node);
        }
        
        // Recursively search in children
        for child in &mut node.children {
            if let Some(found) = Self::find_redis_database_node(child, connection_id, database_name) {
                return Some(found);
            }
        }
        
        None
    }

    fn load_redis_keys_for_database(&mut self, connection_id: i64, database_name: &str, db_node: &mut models::structs::TreeNode) {
        
        // Clear existing children and mark as loading
        db_node.children.clear();
        
        // Extract database number from database_name (e.g., "db0" -> 0)
        let db_number = if let Some(suffix) = database_name.strip_prefix("db") {
            suffix.parse::<u8>().unwrap_or(0)
        } else {
            0
        };
        
        // Get connection pool and fetch keys
        let rt = tokio::runtime::Runtime::new().unwrap();
        let keys_result = rt.block_on(async {
            if let Some(pool) = connection::get_or_create_connection_pool(self, connection_id).await {
                if let models::enums::DatabasePool::Redis(redis_manager) = pool {
                    let mut conn = redis_manager.as_ref().clone();
                    
                    // Select the specific database
                    if let Err(e) = redis::cmd("SELECT").arg(db_number).query_async::<_, ()>(&mut conn).await {
                        debug!("❌ Failed to select database {}: {}", db_number, e);
                        return Vec::new();
                    }
                    
                    // Use SCAN for safe key enumeration (better than KEYS * in production)
                    let mut cursor = 0u64;
                    let mut all_keys = Vec::new();
                    let max_keys = 100; // Limit to first 100 keys to avoid overwhelming UI
                    
                    loop {
                        match redis::cmd("SCAN")
                            .arg(cursor)
                            .arg("COUNT")
                            .arg(10)
                            .query_async::<_, (u64, Vec<String>)>(&mut conn)
                            .await 
                        {
                            Ok((next_cursor, keys)) => {
                                for key in keys {
                                    if all_keys.len() >= max_keys {
                                        break;
                                    }
                                    
                                    // Get the type of each key
                                    if let Ok(key_type) = redis::cmd("TYPE").arg(&key).query_async::<_, String>(&mut conn).await {
                                        all_keys.push((key, key_type));
                                    }
                                }
                                
                                cursor = next_cursor;
                                if cursor == 0 || all_keys.len() >= max_keys {
                                    break;
                                }
                            }
                            Err(e) => {
                                debug!("❌ SCAN command failed: {}", e);
                                break;
                            }
                        }
                    }
                    
                    debug!("✅ Found {} keys in database {}", all_keys.len(), database_name);
                    all_keys
                } else {
                    debug!("❌ Connection pool is not Redis type");
                    Vec::new()
                }
            } else {
                debug!("❌ Failed to get Redis connection pool");
                Vec::new()
            }
        });
        
        // Group keys by type
        let mut keys_by_type: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();
        for (key, key_type) in keys_result {
            keys_by_type.entry(key_type.clone()).or_insert_with(Vec::new).push((key, key_type));
        }
        
        // Create folder structure for each Redis data type
        for (data_type, keys) in keys_by_type {
            let folder_name = match data_type.as_str() {
                "string" => "Strings",
                "hash" => "Hashes", 
                "list" => "Lists",
                "set" => "Sets",
                "zset" => "Sorted Sets",
                "stream" => "Streams",
                _ => &data_type,
            };
            
            let mut type_folder = models::structs::TreeNode::new(format!("{} ({})", folder_name, keys.len()), models::enums::NodeType::TablesFolder);
            type_folder.connection_id = Some(connection_id);
            type_folder.database_name = Some(database_name.to_string());
            type_folder.is_expanded = false;
            type_folder.is_loaded = true;
            
            // Add keys of this type to the folder
            for (key, _key_type) in keys {
                let mut key_node = models::structs::TreeNode::new(key.clone(), models::enums::NodeType::Table);
                key_node.connection_id = Some(connection_id);
                key_node.database_name = Some(database_name.to_string());
                type_folder.children.push(key_node);
            }
            
            db_node.children.push(type_folder);
        }
        
        db_node.is_loaded = true;
        debug!("✅ Database node loaded with {} type folders", db_node.children.len());
    }
    
    // Cached database fetcher for better performance
    fn get_databases_cached(&mut self, connection_id: i64) -> Vec<String> {
        const CACHE_DURATION: std::time::Duration = std::time::Duration::from_secs(300); // 5 minutes cache
        
        // Check if we have cached data and it's still valid
        if let Some(cache_time) = self.database_cache_time.get(&connection_id) {
            if cache_time.elapsed() < CACHE_DURATION {
                if let Some(cached_databases) = self.database_cache.get(&connection_id) {
                    return cached_databases.clone();
                }
            }
        }
        
        // Cache is invalid or doesn't exist, fetch fresh data
        // But do this in background to avoid blocking UI
        if let Some(databases) = connection::fetch_databases_from_connection(self, connection_id) {
            // Update cache
            self.database_cache.insert(connection_id, databases.clone());
            self.database_cache_time.insert(connection_id, std::time::Instant::now());
            databases
        } else {
            // Return empty list if fetch failed, but don't cache the failure
            Vec::new()
        }
    }
    

    fn load_folder_content(&mut self, connection_id: i64, node: &mut models::structs::TreeNode, folder_type: models::enums::NodeType) {        
        // Find the connection by ID
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            let connection = connection.clone();
            
            
            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    self.load_mysql_folder_content(connection_id, &connection, node, folder_type);
                },
                models::enums::DatabaseType::PostgreSQL => {
                    self.load_postgresql_folder_content(connection_id, &connection, node, folder_type);
                },
                models::enums::DatabaseType::SQLite => {
                    self.load_sqlite_folder_content(connection_id, &connection, node, folder_type);
                },
                models::enums::DatabaseType::Redis => {
                    self.load_redis_folder_content(connection_id, &connection, node, folder_type);
                }
                models::enums::DatabaseType::MSSQL => {
                    self.load_mssql_folder_content(connection_id, &connection, node, folder_type);
                }
            }
            
            node.is_loaded = true;
        } else {
            debug!("ERROR: Connection with ID {} not found!", connection_id);
        }
    }

    fn load_mysql_folder_content(&mut self, connection_id: i64, connection: &models::structs::ConnectionConfig, node: &mut models::structs::TreeNode, folder_type: models::enums::NodeType) {
        // Get database name from node or connection default
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);
        
        // Map folder type to cache table type
        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            models::enums::NodeType::StoredProceduresFolder => "procedure",
            models::enums::NodeType::UserFunctionsFolder => "function",
            models::enums::NodeType::TriggersFolder => "trigger",
            models::enums::NodeType::EventsFolder => "event",
            _ => {
                debug!("Unsupported folder type: {:?}", folder_type);
                return;
            }
        };
        
        // First try to get from cache
        if let Some(cached_items) = cache_data::get_tables_from_cache(self, connection_id, database_name, table_type) {
            if !cached_items.is_empty() {                
                // Create tree nodes from cached data
                let child_nodes: Vec<models::structs::TreeNode> = cached_items.into_iter().map(|item_name| {
                    let mut child_node = models::structs::TreeNode::new(item_name.clone(), match folder_type {
                        models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                        models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                        models::enums::NodeType::StoredProceduresFolder => models::enums::NodeType::StoredProcedure,
                        models::enums::NodeType::UserFunctionsFolder => models::enums::NodeType::UserFunction,
                        models::enums::NodeType::TriggersFolder => models::enums::NodeType::Trigger,
                        models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                        _ => models::enums::NodeType::Table,
                    });
                    child_node.connection_id = Some(connection_id);
                    child_node.database_name = Some(database_name.clone());
                    child_node.is_loaded = false; // Will load columns on expansion if it's a table
                    child_node
                }).collect();
                
                node.children = child_nodes;
                return;
            }
        }
        
        // If cache is empty, fetch from actual database        
        if let Some(real_items) = driver_mysql::fetch_tables_from_mysql_connection(self, connection_id, database_name, table_type) {
            debug!("Successfully fetched {} {} from MySQL database", real_items.len(), table_type);
            
            // Save to cache for future use
            let table_data: Vec<(String, String)> = real_items.iter().map(|name| (name.clone(), table_type.to_string())).collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            
            // Create tree nodes from fetched data
            let child_nodes: Vec<models::structs::TreeNode> = real_items.into_iter().map(|item_name| {
                let mut child_node = models::structs::TreeNode::new(item_name.clone(), match folder_type {
                    models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                    models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                    models::enums::NodeType::StoredProceduresFolder => models::enums::NodeType::StoredProcedure,
                    models::enums::NodeType::UserFunctionsFolder => models::enums::NodeType::UserFunction,
                    models::enums::NodeType::TriggersFolder => models::enums::NodeType::Trigger,
                    models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                    _ => models::enums::NodeType::Table,
                });
                child_node.connection_id = Some(connection_id);
                child_node.database_name = Some(database_name.clone());
                child_node.is_loaded = false; // Will load columns on expansion if it's a table
                child_node
            }).collect();
            
            node.children = child_nodes;
        } else {
            // If database fetch fails, add sample data as fallback
            debug!("Failed to fetch from MySQL, using sample {} data", table_type);
            
            let sample_items = match folder_type {
                models::enums::NodeType::TablesFolder => vec!["users".to_string(), "products".to_string(), "orders".to_string()],
                models::enums::NodeType::ViewsFolder => vec!["user_orders_view".to_string(), "product_summary_view".to_string()],
                models::enums::NodeType::StoredProceduresFolder => vec!["sp_get_user".to_string(), "sp_create_order".to_string()],
                models::enums::NodeType::UserFunctionsFolder => vec!["fn_calculate_total".to_string()],
                models::enums::NodeType::TriggersFolder => vec!["tr_update_timestamp".to_string()],
                models::enums::NodeType::EventsFolder => vec!["ev_cleanup".to_string()],
                _ => vec![],
            };
            
            // Create tree nodes
            let child_nodes: Vec<models::structs::TreeNode> = sample_items.into_iter().map(|item_name| {
                let mut child_node = models::structs::TreeNode::new(item_name.clone(), match folder_type {
                    models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                    models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                    models::enums::NodeType::StoredProceduresFolder => models::enums::NodeType::StoredProcedure,
                    models::enums::NodeType::UserFunctionsFolder => models::enums::NodeType::UserFunction,
                    models::enums::NodeType::TriggersFolder => models::enums::NodeType::Trigger,
                    models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                    _ => models::enums::NodeType::Table,
                });
                child_node.connection_id = Some(connection_id);
                child_node.database_name = Some(database_name.clone());
                child_node.is_loaded = false; // Will load columns on expansion if it's a table
                child_node
            }).collect();
            
            node.children = child_nodes;
        }
        
        debug!("Loaded {} {} items for MySQL", node.children.len(), table_type);
    }

    fn load_postgresql_folder_content(&mut self, connection_id: i64, connection: &models::structs::ConnectionConfig, node: &mut models::structs::TreeNode, folder_type: models::enums::NodeType) {
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            _ => {
                node.children = vec![models::structs::TreeNode::new("Not supported for PostgreSQL".to_string(), models::enums::NodeType::Column)];
                return;
            }
        };

        // Try cache first
        if let Some(cached) = cache_data::get_tables_from_cache(self, connection_id, database_name, table_type) {
            if !cached.is_empty() {
                node.children = cached.into_iter().map(|name| {
                    let mut child = models::structs::TreeNode::new(
                        name,
                        match folder_type { models::enums::NodeType::TablesFolder => models::enums::NodeType::Table, _ => models::enums::NodeType::View }
                    );
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child.is_loaded = false;
                    child
                }).collect();
                return;
            }
        }

        // Fallback to live fetch
        if let Some(real_items) = crate::driver_postgres::fetch_tables_from_postgres_connection(self, connection_id, database_name, table_type) {
            let table_data: Vec<(String, String)> = real_items.iter().map(|n| (n.clone(), table_type.to_string())).collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            node.children = real_items.into_iter().map(|name| {
                let mut child = models::structs::TreeNode::new(
                    name,
                    match folder_type { models::enums::NodeType::TablesFolder => models::enums::NodeType::Table, _ => models::enums::NodeType::View }
                );
                child.connection_id = Some(connection_id);
                child.database_name = Some(database_name.clone());
                child.is_loaded = false;
                child
            }).collect();
        } else {
            node.children = vec![models::structs::TreeNode::new("Failed to load items".to_string(), models::enums::NodeType::Column)];
        }
    }

    fn load_sqlite_folder_content(&mut self, connection_id: i64, _connection: &models::structs::ConnectionConfig, node: &mut models::structs::TreeNode, folder_type: models::enums::NodeType) {
        debug!("Loading {:?} content for SQLite", folder_type);
        
        // Try to get from cache first
        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            _ => {
                // For other folder types, return empty for now
                node.children = vec![models::structs::TreeNode::new("Not supported for SQLite".to_string(), models::enums::NodeType::Column)];
                return;
            }
        };
        
        if let Some(cached_items) = cache_data::get_tables_from_cache(self, connection_id, "main", table_type) {
            if !cached_items.is_empty() {
                debug!("Loading {} {} from cache for SQLite", cached_items.len(), table_type);
                
                node.children = cached_items.into_iter().map(|item_name| {
                    let node_type = match folder_type {
                        models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                        models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                        _ => models::enums::NodeType::Table,
                    };
                    
                    let mut item_node = models::structs::TreeNode::new(item_name, node_type);
                    item_node.connection_id = Some(connection_id);
                    item_node.database_name = Some("main".to_string());
                    item_node.is_loaded = false; // Will load columns on expansion if it's a table
                    item_node
                }).collect();
                
                return;
            }
        }
        
        // If cache is empty, fetch from actual SQLite database
        debug!("Cache miss, fetching {} from actual SQLite database", table_type);
        
        if let Some(real_items) = driver_sqlite::fetch_tables_from_sqlite_connection(self, connection_id, table_type) {
            debug!("Successfully fetched {} {} from SQLite database", real_items.len(), table_type);
            
            // Save to cache for future use
            let table_data: Vec<(String, String)> = real_items.iter().map(|name| (name.clone(), table_type.to_string())).collect();
            cache_data::save_tables_to_cache(self, connection_id, "main", &table_data);
            
            // Create tree nodes from fetched data
            let child_nodes: Vec<models::structs::TreeNode> = real_items.into_iter().map(|item_name| {
                let node_type = match folder_type {
                    models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                    models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                    _ => models::enums::NodeType::Table,
                };
                
                let mut item_node = models::structs::TreeNode::new(item_name, node_type);
                item_node.connection_id = Some(connection_id);
                item_node.database_name = Some("main".to_string());
                item_node.is_loaded = false; // Will load columns on expansion if it's a table
                item_node
            }).collect();
            
            node.children = child_nodes;
        } else {
            // If database fetch fails, add sample data as fallback
            debug!("Failed to fetch from SQLite, using sample {} data", table_type);
            
            let sample_items = match folder_type {
                models::enums::NodeType::TablesFolder => vec!["users".to_string(), "products".to_string(), "orders".to_string(), "categories".to_string()],
                models::enums::NodeType::ViewsFolder => vec!["user_summary".to_string(), "order_details".to_string()],
                _ => vec![],
            };
            
            let item_type = match folder_type {
                models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                _ => models::enums::NodeType::Column, // fallback
            };
            
            node.children = sample_items.into_iter().map(|item_name| {
                let mut item_node = models::structs::TreeNode::new(item_name.clone(), item_type.clone());
                item_node.connection_id = Some(connection_id);
                item_node.database_name = Some("main".to_string());
                item_node.is_loaded = false;
                item_node
            }).collect();
        }
        
        debug!("Loaded {} items into {:?} folder for SQLite", node.children.len(), folder_type);
    }

    fn load_mssql_folder_content(&mut self, connection_id: i64, connection: &models::structs::ConnectionConfig, node: &mut models::structs::TreeNode, folder_type: models::enums::NodeType) {
        debug!("Loading {:?} content for MSSQL", folder_type);
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        let (kind, node_mapper): (&str, fn(String) -> models::structs::TreeNode) = match folder_type {
            models::enums::NodeType::TablesFolder => ("table", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::Table);
                child.is_loaded = false;
                child
            }),
            models::enums::NodeType::ViewsFolder => ("view", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::View);
                child.is_loaded = false;
                child
            }),
            models::enums::NodeType::StoredProceduresFolder => ("procedure", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::StoredProcedure);
                child.is_loaded = true;
                child
            }),
            models::enums::NodeType::UserFunctionsFolder => ("function", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::UserFunction);
                child.is_loaded = true;
                child
            }),
            models::enums::NodeType::TriggersFolder => ("trigger", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::Trigger);
                child.is_loaded = true;
                child
            }),
            _ => {
                node.children = vec![models::structs::TreeNode::new("Unsupported folder for MSSQL".to_string(), models::enums::NodeType::Column)];
                return;
            }
        };

        // Try cache first
        if let Some(cached) = cache_data::get_tables_from_cache(self, connection_id, database_name, kind) {
            if !cached.is_empty() {
                node.children = cached.into_iter().map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                }).collect();
                return;
            }
        }

        let fetched = match kind {
            "table" | "view" => crate::driver_mssql::fetch_tables_from_mssql_connection(self, connection_id, database_name, kind),
            "procedure" | "function" | "trigger" => crate::driver_mssql::fetch_objects_from_mssql_connection(self, connection_id, database_name, kind),
            _ => None,
        };

        if let Some(real_items) = fetched {
            let table_data: Vec<(String, String)> = real_items.iter().map(|n| (n.clone(), kind.to_string())).collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            node.children = real_items.into_iter().map(|name| {
                let mut child = node_mapper(name);
                child.connection_id = Some(connection_id);
                child.database_name = Some(database_name.clone());
                child
            }).collect();
        } else {
            // fallback sample
            let sample = match kind {
                "table" => vec!["users".to_string(), "orders".to_string()],
                "view" => vec!["user_summary".to_string()],
                "procedure" => vec!["[dbo].[sp_sample]".to_string()],
                "function" => vec!["[dbo].[fn_sample]".to_string()],
                "trigger" => vec!["[dbo].[Table].[trg_sample]".to_string()],
                _ => Vec::new(),
            };
            node.children = sample.into_iter().map(|name| {
                let mut child = node_mapper(name);
                child.connection_id = Some(connection_id);
                child.database_name = Some(database_name.clone());
                child
            }).collect();
        }
    }

    fn load_redis_folder_content(&mut self, connection_id: i64, _connection: &models::structs::ConnectionConfig, node: &mut models::structs::TreeNode, folder_type: models::enums::NodeType) {
        debug!("Loading {:?} content for Redis", folder_type);
        
        // Redis doesn't have traditional folder structures like SQL databases
        // We'll create a simplified structure based on Redis concepts
        match folder_type {
            models::enums::NodeType::TablesFolder => {
                // For Redis, "tables" could be key patterns or data structures
                let redis_structures = vec![
                    "strings".to_string(),
                    "hashes".to_string(), 
                    "lists".to_string(),
                    "sets".to_string(),
                    "sorted_sets".to_string(),
                    "streams".to_string(),
                ];
                
                node.children = redis_structures.into_iter().map(|structure_name| {
                    let mut structure_node = models::structs::TreeNode::new(structure_name, models::enums::NodeType::Table);
                    structure_node.connection_id = Some(connection_id);
                    structure_node.database_name = Some("redis".to_string());
                    structure_node.is_loaded = false;
                    structure_node
                }).collect();
            },
            models::enums::NodeType::ViewsFolder => {
                // For Redis, "views" could be info sections
                let info_sections = vec![
                    "server".to_string(),
                    "clients".to_string(),
                    "memory".to_string(),
                    "persistence".to_string(),
                    "stats".to_string(),
                    "replication".to_string(),
                    "cpu".to_string(),
                    "keyspace".to_string(),
                ];
                
                node.children = info_sections.into_iter().map(|section_name| {
                    let mut section_node = models::structs::TreeNode::new(section_name, models::enums::NodeType::View);
                    section_node.connection_id = Some(connection_id);
                    section_node.database_name = Some("info".to_string());
                    section_node.is_loaded = false;
                    section_node
                }).collect();
            },
            _ => {
                // Other folder types not supported for Redis
                node.children = vec![models::structs::TreeNode::new("Not supported for Redis".to_string(), models::enums::NodeType::Column)];
            }
        }
        
        debug!("Loaded {} items into {:?} folder for Redis", node.children.len(), folder_type);
    }

    fn load_table_columns_sync(&mut self, connection_id: i64, table_name: &str, connection: &models::structs::ConnectionConfig, database_name: &str) -> Vec<models::structs::TreeNode> {
        // First try to get from cache
        if let Some(cached_columns) = cache_data::get_columns_from_cache(self, connection_id, database_name, table_name) {
            if !cached_columns.is_empty() {
                return cached_columns.into_iter().map(|(column_name, data_type)| {
                    models::structs::TreeNode::new(format!("{} ({})", column_name, data_type), models::enums::NodeType::Column)
                }).collect();
            }
        }
        
        // If cache is empty, fetch from actual database
        if let Some(real_columns) = connection::fetch_columns_from_database(connection_id, database_name, table_name, connection) {
            // Save to cache for future use
            cache_data::save_columns_to_cache(self, connection_id, database_name, table_name, &real_columns);
            
            // Convert to models::structs::TreeNode
            real_columns.into_iter().map(|(column_name, data_type)| {
                models::structs::TreeNode::new(format!("{} ({})", column_name, data_type), models::enums::NodeType::Column)
            }).collect()
        } else {
            // If database fetch fails, return sample columns
            vec![
                models::structs::TreeNode::new("id (INTEGER)".to_string(), models::enums::NodeType::Column),
                models::structs::TreeNode::new("name (VARCHAR)".to_string(), models::enums::NodeType::Column),
                models::structs::TreeNode::new("created_at (TIMESTAMP)".to_string(), models::enums::NodeType::Column),
            ]
        }
    }

    fn load_table_columns_for_node(&mut self, connection_id: i64, table_name: &str, nodes: &mut [models::structs::TreeNode], _table_index: usize) {
        // Find the connection by ID
        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(connection_id)) {
            let connection = connection.clone();
            
            // Find the table node to get the correct database_name
            let database_name = self.find_table_database_name(nodes, table_name, connection_id)
                .unwrap_or_else(|| connection.database.clone());
            
            // Load columns for this table without creating new runtime
            let columns = self.load_table_columns_sync(connection_id, table_name, &connection, &database_name);
            
            // Find the table node recursively and update it
            let updated = self.update_table_node_with_columns_recursive(nodes, table_name, columns, connection_id);
            
            if !updated {
                // Log only if update failed
            }
        }
    }

    fn find_table_database_name(&self, nodes: &[models::structs::TreeNode], table_name: &str, connection_id: i64) -> Option<String> {
        for node in nodes {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table || node.node_type == models::enums::NodeType::View) && 
               node.name == table_name && 
               node.connection_id == Some(connection_id) {
                return node.database_name.clone();
            }
            
            // Recursively search in children
            if let Some(found_db) = self.find_table_database_name(&node.children, table_name, connection_id) {
                return Some(found_db);
            }
        }
        None
    }

    fn update_table_node_with_columns_recursive(&mut self, nodes: &mut [models::structs::TreeNode], table_name: &str, columns: Vec<models::structs::TreeNode>, connection_id: i64) -> bool {
        for node in nodes.iter_mut() {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table || node.node_type == models::enums::NodeType::View) && 
               node.name == table_name && 
               node.connection_id == Some(connection_id) {
                node.children = columns;
                node.is_loaded = true;
                return true;
            }
            
            // Recursively search in children
            if self.update_table_node_with_columns_recursive(&mut node.children, table_name, columns.clone(), connection_id) {
                return true;
            }
        }
        false
    }

    // Pagination methods
    pub fn update_pagination_data(&mut self, all_data: Vec<Vec<String>>) {
        debug!("=== UPDATE_PAGINATION_DATA DEBUG ===");
        debug!("Received data rows: {}", all_data.len());
        if !all_data.is_empty() {
            debug!("First row sample: {:?}", &all_data[0]);
        }
        
        self.all_table_data = all_data;
        self.total_rows = self.all_table_data.len();
        self.current_page = 0; // Reset to first page
        
        debug!("After assignment - all_table_data.len(): {}", self.all_table_data.len());
        debug!("After assignment - total_rows: {}", self.total_rows);
        debug!("====================================");
        
        self.update_current_page_data();
        
        // Initialize column widths when new data is loaded
        self.initialize_column_widths();
    }

    // Column width management methods
    fn initialize_column_widths(&mut self) {
        let num_columns = self.current_table_headers.len();
        if num_columns > 0 {
            // Calculate initial column width based on available space
            let base_width = 180.0; // Base width per column
            self.column_widths = vec![base_width; num_columns];
        } else {
            self.column_widths.clear();
        }
    }

    fn get_column_width(&self, column_index: usize) -> f32 {
        self.column_widths.get(column_index).copied().unwrap_or(180.0).max(self.min_column_width)
    }

    fn set_column_width(&mut self, column_index: usize, width: f32) {
        if column_index < self.column_widths.len() {
            // Only enforce minimum width, allow unlimited maximum width
            let safe_width = width.max(self.min_column_width);
            // Ensure we never have invalid floating point values
            let final_width = if safe_width.is_finite() && safe_width > 0.0 {
                safe_width
            } else {
                self.min_column_width
            };
            self.column_widths[column_index] = final_width;
        }
    }

    fn update_current_page_data(&mut self) {
        let start_index = self.current_page * self.page_size;
        let end_index = ((self.current_page + 1) * self.page_size).min(self.all_table_data.len());
        
        if start_index < self.all_table_data.len() {
            self.current_table_data = self.all_table_data[start_index..end_index].to_vec();
        } else {
            self.current_table_data.clear();
        }
    }

    fn next_page(&mut self) {
        let max_page = (self.total_rows.saturating_sub(1)) / self.page_size;
        if self.current_page < max_page {
            self.current_page += 1;
            self.update_current_page_data();
            self.clear_table_selection();
        }
    }

    fn previous_page(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.update_current_page_data();
            self.clear_table_selection();
        }
    }

    fn go_to_page(&mut self, page: usize) {
        let max_page = (self.total_rows.saturating_sub(1)) / self.page_size;
        if page <= max_page {
            self.current_page = page;
            self.update_current_page_data();
            self.clear_table_selection();
        }
    }

    fn set_page_size(&mut self, new_size: usize) {
        if new_size > 0 {
            self.page_size = new_size;
            self.current_page = 0; // Reset to first page
            self.update_current_page_data();
            self.clear_table_selection();
        }
    }

    pub fn get_total_pages(&self) -> usize {
        if self.total_rows == 0 {
            0
        } else {
            (self.total_rows + self.page_size - 1) / self.page_size
        }
    }



    fn render_tree_for_database_section(&mut self, ui: &mut egui::Ui) {
        // Add responsive search box
        ui.horizontal(|ui| {            
            // Make search box responsive to sidebar width
            let available_width = ui.available_width() - 5.0; // Leave space for clear button and padding
            let search_response = ui.add_sized(
                [available_width, 20.0], 
                egui::TextEdit::singleline(&mut self.database_search_text)
                    .hint_text("Search databases, tables, keys...")
            );
            
            if search_response.changed() {
                self.update_search_results();
            }
        });
        
        ui.separator();
        
        // Use search results if search is active, otherwise use normal tree
        if self.show_search_results && !self.database_search_text.trim().is_empty() {
            // Show search results
            let mut filtered_tree = std::mem::take(&mut self.filtered_items_tree);
            let _ = self.render_tree(ui, &mut filtered_tree);
            self.filtered_items_tree = filtered_tree;
        } else {
            // Show normal tree
            // Use slice to avoid borrowing issues
            let mut items_tree = std::mem::take(&mut self.items_tree);
            
            let _ = self.render_tree(ui, &mut items_tree);
            
            // Check if tree was refreshed inside render_tree
            if self.items_tree.is_empty() {
                // Tree was not refreshed, restore the modified tree
                self.items_tree = items_tree;
            } else {
                // Tree was refreshed inside render_tree, keep the new tree
                debug!("Tree was refreshed inside render_tree, keeping the new tree");
            }
        }
    }

    fn update_search_results(&mut self) {
        // Clone search text to avoid borrowing issues
        let search_text = self.database_search_text.trim().to_string();
        
        if search_text.is_empty() {
            self.show_search_results = false;
            self.filtered_items_tree.clear();
            return;
        }
        
        self.show_search_results = true;
        self.filtered_items_tree.clear();
        
        // Search through the main items_tree with LIKE functionality
        for node in &self.items_tree {
            if let Some(filtered_node) = self.filter_node_with_like_search(node, &search_text) {
                self.filtered_items_tree.push(filtered_node);
            }
        }
        
        // If we have an active connection, also search in its database/table cache
        if let Some(connection_id) = self.current_connection_id {
            self.search_in_connection_data(connection_id, &search_text);
        }
        // Note: We don't search all connections to avoid borrowing issues
        // Users can select a specific connection to search within it
    }
    
    fn filter_node_with_like_search(&self, node: &models::structs::TreeNode, search_text: &str) -> Option<models::structs::TreeNode> {
        let mut matches = false;
        let mut filtered_children = Vec::new();
        
        // Check if current node matches using case-insensitive LIKE search
        let node_name_lower = node.name.to_lowercase();
        let search_lower = search_text.to_lowercase();
        
        // LIKE search: if search text is contained anywhere in the node name
        if node_name_lower.contains(&search_lower) {
            matches = true;
        }
        
        // Check children recursively
        for child in &node.children {
            if let Some(filtered_child) = self.filter_node_with_like_search(child, search_text) {
                filtered_children.push(filtered_child);
                matches = true;
            }
        }
        
        if matches {
            let mut filtered_node = node.clone();
            filtered_node.children = filtered_children;
            filtered_node.is_expanded = true; // Auto-expand search results
            Some(filtered_node)
        } else {
            None
        }
    }
    
    fn search_in_connection_data(&mut self, connection_id: i64, search_text: &str) {
        // Find the connection to determine its type
        let connection_type = self.connections.iter()
            .find(|c| c.id == Some(connection_id))
            .map(|c| c.connection_type.clone());
            
        if let Some(conn_type) = connection_type {
            match conn_type {
                models::enums::DatabaseType::Redis => {
                    self.search_redis_keys(connection_id, search_text);
                }
                models::enums::DatabaseType::MySQL | models::enums::DatabaseType::PostgreSQL | models::enums::DatabaseType::SQLite => {
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
                models::enums::DatabaseType::MSSQL => {
                    // Basic table search (reuse SQL logic)
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
            }
        }
    }
    
    fn search_redis_keys(&mut self, connection_id: i64, search_text: &str) {
        // Search through Redis keys using SCAN with flexible pattern
        let rt = tokio::runtime::Runtime::new().unwrap();
        
        let search_results = rt.block_on(async {
            if let Some(pool) = connection::get_or_create_connection_pool(self, connection_id).await {
                if let models::enums::DatabasePool::Redis(redis_manager) = pool {
                    let mut conn = redis_manager.as_ref().clone();
                    
                    // Use flexible pattern for LIKE search - search text can appear anywhere
                    let pattern = format!("*{}*", search_text.to_lowercase());
                    let mut cursor = 0u64;
                    let mut found_keys = Vec::new();
                    
                    // First try exact pattern match
                    for _iteration in 0..20 { // Increase iterations for more comprehensive search
                        let scan_result: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
                            .arg(cursor)
                            .arg("MATCH")
                            .arg(&pattern)
                            .arg("COUNT")
                            .arg(100) // Increase count for better performance
                            .query_async(&mut conn)
                            .await;
                            
                        if let Ok((new_cursor, keys)) = scan_result {
                            // Additional filtering for case-insensitive LIKE search
                            let search_lower = search_text.to_lowercase();
                            for key in keys {
                                let key_lower = key.to_lowercase();
                                if key_lower.contains(&search_lower) {
                                    found_keys.push(key);
                                }
                            }
                            cursor = new_cursor;
                            if cursor == 0 {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    
                    // Also try case-insensitive pattern if not found enough results
                    if found_keys.len() < 10 {
                        let upper_pattern = format!("*{}*", search_text.to_uppercase());
                        cursor = 0u64;
                        
                        for _iteration in 0..10 {
                            let scan_result: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
                                .arg(cursor)
                                .arg("MATCH")
                                .arg(&upper_pattern)
                                .arg("COUNT")
                                .arg(100)
                                .query_async(&mut conn)
                                .await;
                                
                            if let Ok((new_cursor, keys)) = scan_result {
                                let search_lower = search_text.to_lowercase();
                                for key in keys {
                                    let key_lower = key.to_lowercase();
                                    if key_lower.contains(&search_lower) && !found_keys.contains(&key) {
                                        found_keys.push(key);
                                    }
                                }
                                cursor = new_cursor;
                                if cursor == 0 {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    
                    found_keys
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        });
        
        // Add search results to filtered tree
        if !search_results.is_empty() {
            // Find or create the connection node in filtered results
            let connection_name = self.connections.iter()
                .find(|c| c.id == Some(connection_id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "Unknown Connection".to_string());
                
            let mut search_result_node = models::structs::TreeNode::new(
                format!("🔍 Search Results in {} ({} keys)", connection_name, search_results.len()), 
                models::enums::NodeType::CustomFolder
            );
            search_result_node.connection_id = Some(connection_id);
            search_result_node.is_expanded = true;
            
            // Add found keys as children
            for key in search_results {
                let mut key_node = models::structs::TreeNode::new(key.clone(), models::enums::NodeType::Table);
                key_node.connection_id = Some(connection_id);
                search_result_node.children.push(key_node);
            }
            
            self.filtered_items_tree.push(search_result_node);
        }
    }
    
    fn search_sql_tables(&mut self, connection_id: i64, search_text: &str, db_type: &models::enums::DatabaseType) {
        // Search through cached table data first
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let search_pattern = format!("%{}%", search_text);
            let rt = tokio::runtime::Runtime::new().unwrap();
            
            let search_results = rt.block_on(async {
                let query = match db_type {
                    models::enums::DatabaseType::SQLite => {
                        "SELECT table_name, database_name, table_type FROM table_cache WHERE connection_id = ? AND table_name LIKE ? ORDER BY table_name"
                    }
                    _ => {
                        "SELECT table_name, database_name, table_type FROM table_cache WHERE connection_id = ? AND table_name LIKE ? ORDER BY database_name, table_name"
                    }
                };
                
                sqlx::query_as::<_, (String, String, String)>(query)
                    .bind(connection_id)
                    .bind(&search_pattern)
                    .fetch_all(pool_clone.as_ref())
                    .await
                    .unwrap_or_default()
            });
            
            // Group results by database
            let mut results_by_db: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
            for (table_name, database_name, _table_type) in search_results {
                results_by_db.entry(database_name).or_insert_with(Vec::new).push(table_name);
            }
            
            // Add search results to filtered tree
            if !results_by_db.is_empty() {
                let connection_name = self.connections.iter()
                    .find(|c| c.id == Some(connection_id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "Unknown Connection".to_string());
                
                let total_tables: usize = results_by_db.values().map(|v| v.len()).sum();
                let mut search_result_node = models::structs::TreeNode::new(
                    format!("🔍 Search Results in {} ({} tables)", connection_name, total_tables), 
                    models::enums::NodeType::CustomFolder
                );
                search_result_node.connection_id = Some(connection_id);
                search_result_node.is_expanded = true;
                
                // Add databases and their tables
                for (database_name, tables) in results_by_db {
                    let mut db_node = models::structs::TreeNode::new(
                        format!("📁 {} ({} tables)", database_name, tables.len()),
                        models::enums::NodeType::Database
                    );
                    db_node.connection_id = Some(connection_id);
                    db_node.database_name = Some(database_name.clone());
                    db_node.is_expanded = true;
                    
                    for table_name in tables {
                        let mut table_node = models::structs::TreeNode::new(table_name.clone(), models::enums::NodeType::Table);
                        table_node.connection_id = Some(connection_id);
                        table_node.database_name = Some(database_name.clone());
                        db_node.children.push(table_node);
                    }
                    
                    search_result_node.children.push(db_node);
                }
                
                self.filtered_items_tree.push(search_result_node);
            }
        }
    }

    fn clear_table_selection(&mut self) {
        self.selected_row = None;
        self.selected_cell = None;
    }


    fn find_redis_key_info(&self, node: &models::structs::TreeNode, key_name: &str) -> Option<(String, String)> {
        // Check if this node is a type folder (like "Strings (5)")
        if node.node_type == models::enums::NodeType::TablesFolder {            
            // Extract the type from folder name
            let folder_type = if node.name.starts_with("Strings") {
                "string"
            } else if node.name.starts_with("Hashes") {
                "hash"
            } else if node.name.starts_with("Lists") {
                "list"
            } else if node.name.starts_with("Sets") {
                "set"
            } else if node.name.starts_with("Sorted Sets") {
                "zset"
            } else if node.name.starts_with("Streams") {
                "stream"
            } else {
                // Continue searching instead of returning None
                "unknown"
            };
            
            
            // Search for the key in this folder's children
            for child in &node.children {
                debug!("🔍 Checking child: '{}' (type: {:?})", child.name, child.node_type);
                if child.node_type == models::enums::NodeType::Table && child.name == key_name {
                    if let Some(db_name) = &child.database_name {
                        return Some((db_name.clone(), folder_type.to_string()));
                    }
                }
            }
        }
        
        // Recursively search in children
        for child in &node.children {
            if let Some((db_name, key_type)) = self.find_redis_key_info(child, key_name) {
                return Some((db_name, key_type));
            }
        }
        
        None
    }

    fn find_database_name_for_table(&self, node: &models::structs::TreeNode, connection_id: i64, table_name: &str) -> Option<String> {
        // Look for the table in the tree structure to find its database context
        
        // Check if this node is a table with the matching name and connection
    if (node.node_type == models::enums::NodeType::Table || node.node_type == models::enums::NodeType::View) && 
           node.name == table_name &&
           node.connection_id == Some(connection_id) {
            return node.database_name.clone();
        }
        
        // Recursively search in children
        for child in &node.children {
            if let Some(db_name) = self.find_database_name_for_table(child, connection_id, table_name) {
                return Some(db_name);
            }
        }
        
        None
    }


    #[allow(dead_code)]
    fn highlight_sql_syntax(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
        let mut job = egui::text::LayoutJob {
            text: text.to_owned(),
            ..Default::default()
        };
        
        // If text is empty, return empty job
        if text.is_empty() {
            return job;
        }
        
        // SQL keywords for highlighting
        let keywords = [
            "SELECT", "FROM", "WHERE", "INSERT", "UPDATE", "DELETE", "CREATE", "DROP",
            "ALTER", "TABLE", "INDEX", "VIEW", "TRIGGER", "PROCEDURE", "FUNCTION",
            "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "ON", "AS", "AND", "OR", "NOT",
            "NULL", "TRUE", "FALSE", "CASE", "WHEN", "THEN", "ELSE", "END", "IF",
            "EXISTS", "IN", "LIKE", "BETWEEN", "GROUP", "BY", "ORDER", "HAVING",
            "LIMIT", "OFFSET", "UNION", "ALL", "DISTINCT", "COUNT", "SUM", "AVG",
            "MIN", "MAX", "ASC", "DESC", "PRIMARY", "KEY", "FOREIGN", "REFERENCES",
            "CONSTRAINT", "UNIQUE", "DEFAULT", "AUTO_INCREMENT", "SERIAL", "INT",
            "INTEGER", "VARCHAR", "TEXT", "CHAR", "DECIMAL", "FLOAT", "DOUBLE",
            "DATE", "TIME", "DATETIME", "TIMESTAMP", "BOOLEAN", "BOOL", "USE",
        ];
        
        // Define colors for different themes
        let text_color = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(220, 220, 220) // Light text for dark mode
        } else {
            egui::Color32::from_rgb(40, 40, 40) // Dark text for light mode
        };
        
        let keyword_color = egui::Color32::from_rgb(86, 156, 214); // Blue - SQL keywords
        let string_color = egui::Color32::from_rgb(255, 60, 0); // Orange - strings
        let comment_color = egui::Color32::from_rgb(106, 153, 85); // Green - comments
        let number_color = egui::Color32::from_rgb(181, 206, 168); // Light green - numbers
        let function_color = egui::Color32::from_rgb(255, 206, 84); // Yellow - functions
        let operator_color = egui::Color32::from_rgb(212, 212, 212); // Light gray - operators
        
        // Process line by line to handle comments properly
        let lines: Vec<&str> = text.lines().collect();
        let mut byte_offset = 0;
        
        for (line_idx, line) in lines.iter().enumerate() {
            let line_start_offset = byte_offset;
            
            // Check if this line is a comment
            if line.trim_start().starts_with("--") {
                // Entire line is a comment
                job.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: line_start_offset..line_start_offset + line.len(),
                    format: egui::TextFormat {
                        color: comment_color,
                        font_id: egui::FontId::monospace(14.0),
                        ..Default::default()
                    },
                });
            } else {
                // Process words in the line
                let words: Vec<&str> = line.split_whitespace().collect();
                let mut line_pos = line_start_offset;
                let mut word_search_start = 0;
                
                for word in words {
                    // Find the word position in the line
                    if let Some(word_start_in_line) = line[word_search_start..].find(word) {
                        let absolute_word_start = line_start_offset + word_search_start + word_start_in_line;
                        let absolute_word_end = absolute_word_start + word.len();
                        
                        // Add whitespace before word if any
                        if absolute_word_start > line_pos {
                            job.sections.push(egui::text::LayoutSection {
                                leading_space: 0.0,
                                byte_range: line_pos..absolute_word_start,
                                format: egui::TextFormat {
                                    color: text_color,
                                    font_id: egui::FontId::monospace(14.0),
                                    ..Default::default()
                                },
                            });
                        }
                        
                        // Determine word color
                        let word_color = if word.starts_with('\'') || word.starts_with('"') {
                            // String
                            string_color
                        } else if word.chars().all(|c| c.is_ascii_digit() || c == '.') && !word.is_empty() {
                            // Number
                            number_color
                        } else if keywords.contains(&word.to_uppercase().as_str()) {
                            // SQL keyword
                            keyword_color
                        } else if word.contains('(') {
                            // Function call
                            function_color
                        } else if "(){}[]<>=!+-*/%,;".contains(word.chars().next().unwrap_or(' ')) {
                            // Operator
                            operator_color
                        } else {
                            // Default text
                            text_color
                        };
                        
                        // Add the word with appropriate color
                        job.sections.push(egui::text::LayoutSection {
                            leading_space: 0.0,
                            byte_range: absolute_word_start..absolute_word_end,
                            format: egui::TextFormat {
                                color: word_color,
                                font_id: egui::FontId::monospace(14.0),
                                ..Default::default()
                            },
                        });
                        
                        // Update positions
                        word_search_start = word_search_start + word_start_in_line + word.len();
                        line_pos = absolute_word_end;
                    }
                }
                
                // Add any remaining text in the line
                if line_pos < line_start_offset + line.len() {
                    job.sections.push(egui::text::LayoutSection {
                        leading_space: 0.0,
                        byte_range: line_pos..line_start_offset + line.len(),
                        format: egui::TextFormat {
                            color: text_color,
                            font_id: egui::FontId::monospace(14.0),
                            ..Default::default()
                        },
                    });
                }
            }
            
            // Add newline character if not the last line
            byte_offset += line.len();
            if line_idx < lines.len() - 1 {
                // Add the newline character
                job.sections.push(egui::text::LayoutSection {
                    leading_space: 0.0,
                    byte_range: byte_offset..byte_offset + 1,
                    format: egui::TextFormat {
                        color: text_color,
                        font_id: egui::FontId::monospace(14.0),
                        ..Default::default()
                    },
                });
                byte_offset += 1; // for the \n character
            }
        }
        
        job
    }

    fn render_table_data(&mut self, ui: &mut egui::Ui) {
        if !self.current_table_headers.is_empty() || !self.current_table_name.is_empty() {
            // Render pagination controls at the top
            if self.total_rows > 0 {
                ui.horizontal(|ui| {
                    ui.label(format!("Total rows: {}", self.total_rows));
                    ui.separator();
                    
                    // Page size selector
                    ui.label("Rows per page:");
                    let mut page_size_str = self.page_size.to_string();
                    if ui.text_edit_singleline(&mut page_size_str).changed() {
                        if let Ok(new_size) = page_size_str.parse::<usize>() {
                            if new_size > 0 && new_size <= 10000 {
                                self.set_page_size(new_size);
                            }
                        }
                    }
                    
                    ui.separator();
                    
                    // Navigation buttons
                    if ui.button("⏮ First").clicked() {
                        self.go_to_page(0);
                    }
                    
                    ui.add_enabled(self.current_page > 0, egui::Button::new("◀ Prev"))
                        .clicked()
                        .then(|| self.previous_page());
                    
                    ui.label(format!("Page {} of {}", self.current_page + 1, self.get_total_pages()));
                    
                    ui.add_enabled(self.current_page < self.get_total_pages().saturating_sub(1), egui::Button::new("Next >"))
                        .clicked()
                        .then(|| self.next_page());
                    
                    if ui.button("Last ⏭").clicked() {
                        let last_page = self.get_total_pages().saturating_sub(1);
                        self.go_to_page(last_page);
                    }
                    
                    ui.separator();
                    
                    // Quick page jump
                    ui.label("Go to page:");
                    let mut page_input = (self.current_page + 1).to_string();
                    if ui.text_edit_singleline(&mut page_input).changed() {
                        if let Ok(page_num) = page_input.parse::<usize>() {
                            if page_num > 0 {
                                self.go_to_page(page_num - 1);
                            }
                        }
                    }
                });
                ui.separator();
            }
            
            if !self.current_table_headers.is_empty() && !self.current_table_data.is_empty() {
                // Store sort state locally to avoid borrowing issues
                let current_sort_column = self.sort_column;
                let current_sort_ascending = self.sort_ascending;
                let headers = self.current_table_headers.clone();
                let mut sort_requests = Vec::new();

                // Ensure column widths are initialized
                if self.column_widths.len() != headers.len() {
                    self.initialize_column_widths();
                }

                // If this is an error table (usually 1 column, header contains "error"), set error column width to max
                let mut error_column_index: Option<usize> = None;
                if headers.len() == 1 && headers[0].to_lowercase().contains("error") {
                    error_column_index = Some(0);
                } else {
                    // If there is a column named "error" (case-insensitive), set its width to max
                    for (i, h) in headers.iter().enumerate() {
                        if h.to_lowercase().contains("error") {
                            error_column_index = Some(i);
                            break;
                        }
                    }
                }

                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let grid_response = egui::Grid::new("table_data_grid")
                            .striped(true)
                            .spacing([0.0, 0.0])
                            .min_col_width(0.0)
                            .max_col_width(f32::INFINITY)
                            .show(ui, |ui| {
                                // Render No column header first (centered)
                                ui.allocate_ui_with_layout(
                                    [60.0, ui.available_height().max(30.0)].into(),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        let rect = ui.available_rect_before_wrap();
                                        let border_color = if ui.visuals().dark_mode {
                                            egui::Color32::from_gray(60)
                                        } else {
                                            egui::Color32::from_gray(200)
                                        };
                                        let thin_stroke = egui::Stroke::new(0.5, border_color);
                                        ui.painter().line_segment([rect.left_top(), rect.right_top()], thin_stroke);
                                        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], thin_stroke);
                                        ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], thin_stroke);
                                        ui.painter().line_segment([rect.left_bottom(), rect.left_top()], thin_stroke);
                                        ui.add(egui::Label::new(
                                            egui::RichText::new("No")
                                                .strong()
                                                .size(14.0)
                                                .color(if ui.visuals().dark_mode {
                                                    egui::Color32::from_rgb(220, 220, 255)
                                                } else {
                                                    egui::Color32::from_rgb(60, 60, 120)
                                                })
                                        ));
                                    }
                                );

                                // Render enhanced headers with sort buttons and resize handles
                                for (col_index, header) in headers.iter().enumerate() {
                                    // For error columns, use a larger default width but still allow resizing
                                    let column_width = if Some(col_index) == error_column_index {
                                        // If this is the first time we see an error column, set a larger default width
                                        if self.get_column_width(col_index) <= 180.0 { // Default width
                                            self.set_column_width(col_index, 600.0); // Set larger default for error columns
                                        }
                                        self.get_column_width(col_index).max(100.0)
                                    } else {
                                        self.get_column_width(col_index).max(30.0)
                                    };
                                    let available_height = ui.available_height().max(30.0);

                                    ui.allocate_ui_with_layout(
                                        [column_width, available_height].into(),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let rect = ui.available_rect_before_wrap();
                                            let border_color = if ui.visuals().dark_mode {
                                                egui::Color32::from_gray(60)
                                            } else {
                                                egui::Color32::from_gray(200)
                                            };
                                            let thin_stroke = egui::Stroke::new(0.5, border_color);
                                            ui.painter().line_segment([rect.left_top(), rect.right_top()], thin_stroke);
                                            ui.painter().line_segment([rect.right_top(), rect.right_bottom()], thin_stroke);
                                            ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], thin_stroke);
                                            ui.painter().line_segment([rect.left_bottom(), rect.left_top()], thin_stroke);
                                            ui.horizontal(|ui| {
                                                let sort_button_width = 25.0;
                                                let text_area_width = ui.available_width() - sort_button_width;
                                                ui.allocate_ui_with_layout(
                                                    [text_area_width, ui.available_height()].into(),
                                                    egui::Layout::top_down(egui::Align::Center),
                                                    |ui| {
                                                        ui.add(egui::Label::new(
                                                            egui::RichText::new(header)
                                                                .strong()
                                                                .size(14.0)
                                                                .color(if ui.visuals().dark_mode {
                                                                    egui::Color32::from_rgb(220, 220, 255)
                                                                } else {
                                                                    egui::Color32::from_rgb(60, 60, 120)
                                                                })
                                                        ));
                                                    }
                                                );
                                                let (sort_icon, is_active) = if current_sort_column == Some(col_index) {
                                                    if current_sort_ascending {
                                                        ("^", true)
                                                    } else {
                                                        ("v", true)
                                                    }
                                                } else {
                                                    ("-", false)
                                                };
                                                let sort_button = ui.add(
                                                    egui::Button::new(
                                                        egui::RichText::new(sort_icon)
                                                            .size(12.0)
                                                            .color(if is_active {
                                                                egui::Color32::from_rgb(100, 150, 255)
                                                            } else {
                                                                egui::Color32::GRAY
                                                            })
                                                    )
                                                    .small()
                                                    .fill(if is_active {
                                                        egui::Color32::from_rgba_unmultiplied(100, 150, 255, 50)
                                                    } else {
                                                        egui::Color32::TRANSPARENT
                                                    })
                                                );
                                                if sort_button.clicked() {
                                                    let new_ascending = if current_sort_column == Some(col_index) {
                                                        !current_sort_ascending
                                                    } else {
                                                        true
                                                    };
                                                    sort_requests.push((col_index, new_ascending));
                                                }
                                            });
                                            // Add resize handle for all columns except the last one, 
                                            // BUT always add for error columns (even if they are the last/only column)
                                            if col_index < headers.len() - 1 || Some(col_index) == error_column_index {
                                                let handle_x = ui.max_rect().max.x;
                                                let handle_y = ui.max_rect().min.y;
                                                let handle_height = available_height;
                                                let resize_handle_rect = egui::Rect::from_min_size(
                                                    egui::pos2(handle_x - 3.0, handle_y),
                                                    egui::vec2(6.0, handle_height)
                                                );
                                                let resize_response = ui.allocate_rect(resize_handle_rect, egui::Sense::drag());
                                                
                                                // Always show a subtle resize indicator
                                                let indicator_color = if resize_response.hovered() || resize_response.dragged() {
                                                    egui::Color32::from_rgba_unmultiplied(100, 150, 255, 200)
                                                } else if ui.visuals().dark_mode {
                                                    egui::Color32::from_rgba_unmultiplied(120, 120, 120, 80)
                                                } else {
                                                    egui::Color32::from_rgba_unmultiplied(150, 150, 150, 60)
                                                };
                                                
                                                // Draw the resize handle with dotted pattern
                                                let center_x = handle_x - 1.5;
                                                let dot_size = 1.0;
                                                let dot_spacing = 4.0;
                                                let start_y = handle_y + 8.0;
                                                let end_y = handle_y + handle_height - 8.0;
                                                
                                                for y in (start_y as i32..end_y as i32).step_by(dot_spacing as usize) {
                                                    ui.painter().circle_filled(
                                                        egui::pos2(center_x, y as f32),
                                                        dot_size,
                                                        indicator_color
                                                    );
                                                }
                                                
                                                if resize_response.hovered() {
                                                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                                                }
                                                if resize_response.dragged() {
                                                    let delta_x = resize_response.drag_delta().x;
                                                    let new_width = column_width + delta_x;
                                                    self.set_column_width(col_index, new_width);
                                                }
                                            }
                                        }
                                    );
                                }
                                ui.end_row();

                                // Render data rows with row numbers
                                for (row_index, row) in self.current_table_data.iter().enumerate() {
                                    let is_selected_row = self.selected_row == Some(row_index);
                                    let row_color = if is_selected_row {
                                        if ui.visuals().dark_mode {
                                            egui::Color32::from_rgba_unmultiplied(100, 150, 255, 30)
                                        } else {
                                            egui::Color32::from_rgba_unmultiplied(200, 220, 255, 80)
                                        }
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    ui.allocate_ui_with_layout(
                                        [60.0, ui.available_height().max(25.0)].into(),
                                        egui::Layout::top_down(egui::Align::Center),
                                        |ui| {
                                            let rect = ui.available_rect_before_wrap();
                                            if row_color != egui::Color32::TRANSPARENT {
                                                ui.painter().rect_filled(rect, 3.0, row_color);
                                            }
                                            let border_color = if ui.visuals().dark_mode {
                                                egui::Color32::from_gray(60)
                                            } else {
                                                egui::Color32::from_gray(200)
                                            };
                                            let thin_stroke = egui::Stroke::new(0.5, border_color);
                                            ui.painter().line_segment([rect.left_top(), rect.right_top()], thin_stroke);
                                            ui.painter().line_segment([rect.right_top(), rect.right_bottom()], thin_stroke);
                                            ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], thin_stroke);
                                            ui.painter().line_segment([rect.left_bottom(), rect.left_top()], thin_stroke);
                                            let label_response = ui.label((row_index + 1).to_string());
                                            if label_response.clicked() {
                                                self.selected_row = Some(row_index);
                                                self.selected_cell = None;
                                            }
                                            label_response
                                        }
                                    );
                                    for (col_index, cell) in row.iter().enumerate() {
                                        let is_selected_cell = self.selected_cell == Some((row_index, col_index));
                                        let column_width = if Some(col_index) == error_column_index {
                                            self.get_column_width(col_index).max(100.0)
                                        } else {
                                            self.get_column_width(col_index).max(50.0)
                                        };
                                        let cell_height = ui.available_height().max(25.0);
                                        ui.allocate_ui_with_layout(
                                            [column_width, cell_height].into(),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                let rect = ui.available_rect_before_wrap();
                                                if row_color != egui::Color32::TRANSPARENT {
                                                    ui.painter().rect_filled(rect, 3.0, row_color);
                                                }
                                                let border_color = if ui.visuals().dark_mode {
                                                    egui::Color32::from_gray(60)
                                                } else {
                                                    egui::Color32::from_gray(200)
                                                };
                                                let thin_stroke = egui::Stroke::new(0.5, border_color);
                                                ui.painter().line_segment([rect.left_top(), rect.right_top()], thin_stroke);
                                                ui.painter().line_segment([rect.right_top(), rect.right_bottom()], thin_stroke);
                                                ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], thin_stroke);
                                                ui.painter().line_segment([rect.left_bottom(), rect.left_top()], thin_stroke);
                                                if is_selected_cell {
                                                    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 60, 0));
                                                    ui.painter().rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(255, 60, 10, 20));
                                                    ui.painter().line_segment([rect.left_top(), rect.right_top()], stroke);
                                                    ui.painter().line_segment([rect.right_top(), rect.right_bottom()], stroke);
                                                    ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], stroke);
                                                    ui.painter().line_segment([rect.left_bottom(), rect.left_top()], stroke);
                                                }
                                                let max_chars = ((column_width / 8.0).floor() as usize).max(10);
                                                let display_text = if cell.chars().count() > max_chars {
                                                    format!("{}...", cell.chars().take(max_chars.saturating_sub(3)).collect::<String>())
                                                } else {
                                                    cell.clone()
                                                };
                                                let cell_response = ui.allocate_response(rect.size(), egui::Sense::click());
                                                if cell_response.clicked() {
                                                    self.selected_row = Some(row_index);
                                                    self.selected_cell = Some((row_index, col_index));
                                                }
                                                let hover_response = if cell.chars().count() > max_chars || !cell.is_empty() {
                                                    cell_response.on_hover_text(cell)
                                                } else {
                                                    cell_response
                                                };
                                                let text_pos = rect.left_top() + egui::vec2(5.0, rect.height() * 0.5);
                                                ui.painter().text(
                                                    text_pos,
                                                    egui::Align2::LEFT_CENTER,
                                                    &display_text,
                                                    egui::FontId::default(),
                                                    if is_selected_cell {
                                                        if ui.visuals().dark_mode {
                                                            egui::Color32::WHITE
                                                        } else {
                                                            egui::Color32::BLACK
                                                        }
                                                    } else {
                                                        ui.visuals().text_color()
                                                    }
                                                );
                                                hover_response.context_menu(|ui| {
                                                    ui.set_min_width(150.0);
                                                    ui.vertical(|ui| {
                                                        if ui.button("📋 Copy Cell Value").clicked() {
                                                            ui.ctx().copy_text(cell.clone());
                                                            ui.close_menu();
                                                        }
                                                        if let Some(selected_row_idx) = self.selected_row {
                                                            if ui.button("📄 Copy Row as CSV").clicked() {
                                                                if let Some(row_data) = self.current_table_data.get(selected_row_idx) {
                                                                    let csv_row = row_data.iter()
                                                                        .map(|cell| {
                                                                            if cell.contains(',') || cell.contains('"') || cell.contains('\n') {
                                                                                format!("\"{}\"", cell.replace('"', "\"\""))
                                                                            } else {
                                                                                cell.clone()
                                                                            }
                                                                        })
                                                                        .collect::<Vec<_>>()
                                                                        .join(",");
                                                                    ui.ctx().copy_text(csv_row);
                                                                }
                                                                ui.close_menu();
                                                            }
                                                        }
                                                        ui.separator();
                                                        if ui.button("📄 Export to CSV").clicked() {
                                                            export::export_to_csv(&self.all_table_data, &self.current_table_headers, &self.current_table_name);
                                                            ui.close_menu();
                                                        }
                                                        if ui.button("📊 Export to XLSX").clicked() {
                                                            export::export_to_xlsx(&self.all_table_data, &self.current_table_headers, &self.current_table_name);
                                                            ui.close_menu();
                                                        }
                                                    });
                                                });
                                            }
                                        );
                                    }
                                    ui.end_row();
                                }
                            });
                        grid_response.response.context_menu(|ui| {
                            ui.set_min_width(150.0);
                            ui.vertical(|ui| {
                                if ui.button("📄 Export to CSV").clicked() {
                                    export::export_to_csv(&self.all_table_data, &self.current_table_headers, &self.current_table_name);
                                    ui.close_menu();
                                }
                                if ui.button("📊 Export to XLSX").clicked() {
                                    export::export_to_xlsx(&self.all_table_data, &self.current_table_headers, &self.current_table_name);
                                    ui.close_menu();
                                }
                            });
                        });
                    });
                for (column_index, ascending) in sort_requests {
                    self.sort_table_data(column_index, ascending);
                }
            } else if self.current_table_name.starts_with("Failed") {
                ui.colored_label(egui::Color32::RED, &self.current_table_name);
            } else {
                ui.label("No data available");
            }
        }
    }
    
    fn sort_table_data(&mut self, column_index: usize, ascending: bool) {
        if column_index >= self.current_table_headers.len() || self.all_table_data.is_empty() {
            return;
        }
        
        // Update sort state
        self.sort_column = Some(column_index);
        self.sort_ascending = ascending;
        
        // Sort ALL the data (not just current page)
        self.all_table_data.sort_by(|a, b| {
            if column_index >= a.len() || column_index >= b.len() {
                return std::cmp::Ordering::Equal;
            }
            
            let cell_a = &a[column_index];
            let cell_b = &b[column_index];
            
            // Handle NULL or empty values (put them at the end)
            let comparison = match (cell_a.as_str(), cell_b.as_str()) {
                ("NULL", "NULL") | ("", "") => std::cmp::Ordering::Equal,
                ("NULL", _) | ("", _) => std::cmp::Ordering::Greater,
                (_, "NULL") | (_, "") => std::cmp::Ordering::Less,
                (a_val, b_val) => {
                    // Try to parse as numbers first for better numeric sorting
                    match (a_val.parse::<f64>(), b_val.parse::<f64>()) {
                        (Ok(num_a), Ok(num_b)) => {
                            num_a.partial_cmp(&num_b).unwrap_or(std::cmp::Ordering::Equal)
                        },
                        _ => {
                            // Fall back to string comparison (case-insensitive)
                            a_val.to_lowercase().cmp(&b_val.to_lowercase())
                        }
                    }
                }
            };
            
            if ascending {
                comparison
            } else {
                comparison.reverse()
            }
        });
        
        // Update current page data after sorting
        self.update_current_page_data();
        
        let sort_direction = if ascending { "^ ascending" } else { "v descending" };
        debug!("✓ Sorted table by column '{}' in {} order ({} total rows)", 
            self.current_table_headers[column_index], 
            sort_direction,
            self.all_table_data.len()
        );
    }


}

impl App for Tabular {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        // Handle forced refresh flag
        if self.needs_refresh {
            self.needs_refresh = false;
            
            // Force refresh of query tree
            sidebar_query::load_queries_from_directory(self);
            
            // Request UI repaint
            ctx.request_repaint();
        }
        
        // Periodic cleanup of stale connection pools (every 10 minutes to reduce overhead)
        if self.last_cleanup_time.elapsed().as_secs() > 600 { // 10 minutes instead of 5
            debug!("🧹 Performing periodic connection pool cleanup");
            
            // Clean up connections that might be stale
            let mut connections_to_refresh: Vec<i64> = self.connection_pools.keys().copied().collect();
            
            // Limit cleanup to avoid blocking UI
            if connections_to_refresh.len() > 5 {
                connections_to_refresh.truncate(5);
            }
            
            for connection_id in connections_to_refresh {
                connection::cleanup_connection_pool(self, connection_id);
            }
            
            self.last_cleanup_time = std::time::Instant::now();
        }
        
        // Handle deferred theme selector request
        if self.request_theme_selector {
            self.request_theme_selector = false;
            self.show_theme_selector = true;
        }
        
        // Handle keyboard shortcuts
        ctx.input(|i| {
            // CMD+W or CTRL+W to close current tab
            if (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::W) && !self.query_tabs.is_empty() {
                editor::close_tab(self, self.active_tab_index);
            }
            
            // CMD+Q or CTRL+Q to quit application
            if (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::Q) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            
            // CMD+SHIFT+P to open command palette (on macOS)
            if i.modifiers.mac_cmd && i.modifiers.shift && i.key_pressed(egui::Key::P) {
                editor::open_command_palette(self);
            }
            
            // Handle command palette navigation
            if self.show_command_palette {
                // Arrow key navigation
                if i.key_pressed(egui::Key::ArrowDown) {
                    editor::navigate_command_palette(self, 1);
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    editor::navigate_command_palette(self, -1);
                }
                // Enter to execute selected command
                else if i.key_pressed(egui::Key::Enter) {
                    editor::execute_selected_command(self);
                }
            }
            
            // Handle theme selector navigation
            if self.show_theme_selector {
                // Arrow key navigation
                if i.key_pressed(egui::Key::ArrowDown) {
                    editor::navigate_theme_selector(self,1);
                } else if i.key_pressed(egui::Key::ArrowUp) {
                    editor::navigate_theme_selector(self, -1);
                }
                // Enter to select theme
                else if i.key_pressed(egui::Key::Enter) {
                    editor::select_current_theme(self);
                }
            }
            
            // Escape to close command palette or theme selector  
            if i.key_pressed(egui::Key::Escape) {
                if self.show_theme_selector {
                    self.show_theme_selector = false;
                } else if self.show_command_palette {
                    self.show_command_palette = false;
                    self.command_palette_input.clear();
                    self.command_palette_selected_index = 0;
                }
            }
        });

        // Render command palette if open
        if self.show_command_palette {
            editor::render_command_palette(self, ctx);
        }

        // Render theme selector if open
        if self.show_theme_selector {
            editor::render_theme_selector(self, ctx);
        }

        // Check for background task results
        if let Some(receiver) = &self.background_receiver {
            while let Ok(result) = receiver.try_recv() {
                match result {
                    models::enums::BackgroundResult::RefreshComplete { connection_id, success } => {
                        // Remove from refreshing set
                        self.refreshing_connections.remove(&connection_id);
                        
                        if success {
                            debug!("Background refresh completed successfully for connection {}", connection_id);
                            // Re-expand connection node to show fresh data
                            for node in &mut self.items_tree {
                                if node.node_type == models::enums::NodeType::Connection && node.connection_id == Some(connection_id) {
                                    node.is_loaded = false; // Force reload from cache
                                    // Don't auto-expand after refresh, let user manually expand
                                    break;
                                }
                            }
                            // Request UI repaint to show updated data
                            ctx.request_repaint();
                        } else {
                            debug!("Background refresh failed for connection {}", connection_id);
                        }
                    }
                }
            }
        }

        // Disable visual indicators for active/focused elements (but keep text selection visible)
        ctx.style_mut(|style| {
            // Keep text selection visible with a subtle highlight
            style.visuals.selection.bg_fill = egui::Color32::from_rgb(255, 60, 0);
            style.visuals.selection.stroke.color = egui::Color32::from_rgb(0, 0, 0);
            
            // Only disable other widget visual indicators
            style.visuals.widgets.active.bg_fill = egui::Color32::TRANSPARENT;
            style.visuals.widgets.active.bg_stroke.color = egui::Color32::TRANSPARENT;
            style.visuals.widgets.hovered.bg_stroke.color = egui::Color32::TRANSPARENT;
        });
        
        // Check if we need to refresh the UI after a connection removal
        if self.needs_refresh {
            self.needs_refresh = false;
            ctx.request_repaint();
        }
        
        sidebar_database::render_add_connection_dialog(self, ctx);
        sidebar_database::render_edit_connection_dialog(self, ctx);
        dialog::render_save_dialog(self, ctx);
        connection::render_connection_selector(self, ctx);
        dialog::render_error_dialog(self, ctx);
        dialog::render_about_dialog(self, ctx);
        sidebar_query::render_create_folder_dialog(self, ctx);
        sidebar_query::render_move_to_folder_dialog(self, ctx);

        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(250.0)
            .min_width(150.0)
            .max_width(500.0)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    // Top section with tabs
                    ui.horizontal(|ui| {
                        // Calculate equal width for three buttons with responsive design
                        let available_width = ui.available_width();
                        let button_spacing = ui.spacing().item_spacing.x;
                        let button_width = (available_width - (button_spacing * 2.0)) / 3.0;
                        let button_height = 24.0;
                        
                        // Database tab
                        let database_button = if self.selected_menu == "Database" {
                            egui::Button::new(
                                egui::RichText::new("Database").color(egui::Color32::WHITE).text_style(egui::TextStyle::Body)
                            ).fill(egui::Color32::from_rgb(255, 60, 0))
                        } else {
                            egui::Button::new("Database").fill(egui::Color32::TRANSPARENT)
                        };
                        if ui.add_sized([button_width, button_height], database_button).clicked() {
                            self.selected_menu = "Database".to_string();
                        }
                        
                        // Queries tab
                        let queries_button = if self.selected_menu == "Queries" {
                            egui::Button::new(egui::RichText::new("Queries").color(egui::Color32::WHITE).text_style(egui::TextStyle::Body)).fill(egui::Color32::from_rgb(255, 60, 0)) // Orange fill for active
                        } else {
                            egui::Button::new("Queries").fill(egui::Color32::TRANSPARENT)
                        };
                        if ui.add_sized([button_width, button_height], queries_button).clicked() {
                            self.selected_menu = "Queries".to_string();
                        }
                        
                        // History tab
                        let history_button = if self.selected_menu == "History" {
                            egui::Button::new(egui::RichText::new("History").color(egui::Color32::WHITE).text_style(egui::TextStyle::Body)).fill(egui::Color32::from_rgb(255, 60, 0)) // Orange fill for active
                        } else {
                            egui::Button::new("History").fill(egui::Color32::TRANSPARENT)
                        };
                        if ui.add_sized([button_width, button_height], history_button).clicked() {
                            self.selected_menu = "History".to_string();
                        }
                    });

                    ui.separator();

                    // Middle section with scrollable content
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        match self.selected_menu.as_str() {
                            "Database" => {
                                if self.connections.is_empty() {
                                    ui.label("No connections configured");
                                    ui.label("Click ➕ to add a new connection");
                                } else {                                
                                    // Render tree directly without mem::take to avoid race conditions
                                    self.render_tree_for_database_section(ui);
                                }
                            },
                            "Queries" => {                                
                                // Add right-click context menu support to the UI area itself
                                let queries_response = ui.interact(ui.available_rect_before_wrap(), egui::Id::new("queries_area"), egui::Sense::click());
                                queries_response.context_menu(|ui| {
                                    if ui.button("📂 Create Folder").clicked() {
                                        self.show_create_folder_dialog = true;
                                        ui.close_menu();
                                    }
                                });
                                
                                // Render the queries tree normally
                                let mut queries_tree = std::mem::take(&mut self.queries_tree);
                                let _ = self.render_tree(ui, &mut queries_tree);
                                self.queries_tree = queries_tree;
                            },
                            "History" => {
                                let mut history_tree = std::mem::take(&mut self.history_tree);
                                let query_files_to_open = self.render_tree(ui, &mut history_tree);
                                self.history_tree = history_tree;
                                
                                // Handle history item clicks
                                for (display_name, _, _) in query_files_to_open {
                                    if let Some(history_item) = self.history_items.iter().find(|item| {
                                        // Use full query text for comparison instead of truncated preview
                                        item.query == display_name
                                    }) {
                                        // Set the query text in the active tab
                                        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                                            active_tab.content = history_item.query.clone();
                                            active_tab.is_modified = true;
                                            self.editor_text = history_item.query.clone();
                                        }
                                        // Set the connection and execute automatically
                                        self.current_connection_id = Some(history_item.connection_id);
                                        // Execute the query immediately
                                        if let Some((headers, data)) = connection::execute_query_with_connection(self, history_item.connection_id, history_item.query.clone()) {
                                            self.current_table_headers = headers;
                                            self.current_table_data = data;
                                            if self.current_table_data.is_empty() {
                                                self.current_table_name = "Query executed successfully (no results)".to_string();
                                            } else {
                                                self.current_table_name = format!("Query Results ({} rows)", self.current_table_data.len());
                                            }
                                        }
                                    }
                                }
                            },
                            _ => {}
                        }
                    });

                    // Bottom section with add button - conditional based on active tab
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                        ui.separator();
                        
                        match self.selected_menu.as_str() {
                            "Database" => {
                                if ui.add_sized(
                                    [24.0, 24.0], // Small square button
                                    egui::Button::new("➕")
                                        .fill(egui::Color32::RED)
                                ).on_hover_text("Add New Database Connection").clicked() {
                                    // Reset test connection status saat buka add dialog
                                    self.test_connection_status = None;
                                    self.test_connection_in_progress = false;
                                    self.show_add_connection = true;
                                }
                            },
                            // "Queries" => {
                            //     if ui.add_sized(
                            //         [24.0, 24.0], // Small square button
                            //         egui::Button::new("➕")
                            //             .fill(egui::Color32::RED)
                            //     ).on_hover_text("New Query File").clicked() {
                            //         // Create new tab instead of clearing editor
                            //         editor::create_new_tab(self, "Untitled Query".to_string(), String::new());
                            //     }
                            // },
                            _ => {
                                // No button for History tab
                            }
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin::ZERO)) // Remove all padding
            .show(ctx, |ui| {
            ui.vertical_centered_justified(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0; // Remove vertical spacing
                ui.spacing_mut().indent = 0.0; // Remove indentation
                
                // Use egui's built-in resizable panels for smooth resizing
                let available_height = ui.available_height();
                let editor_height = available_height * self.table_split_ratio;
                
                // SQL Editor in a resizable top panel - completely flush
                egui::TopBottomPanel::top("sql_editor_panel")
                    .resizable(true)
                    .height_range(available_height * 0.1..=available_height * 0.9)
                    .default_height(editor_height)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::ZERO)) // Remove panel margin
                    .show_inside(ui, |ui| {
                        ui.vertical(|ui| {
                            // Tab bar
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0; // Remove spacing between tabs
                                
                                // Render tabs
                                let mut tab_to_close = None;
                                let mut tab_to_switch = None;
                                let mut connection_to_set: Option<(usize, Option<i64>)> = None;
                                
                                for (index, tab) in self.query_tabs.iter().enumerate() {
                                    let is_active = index == self.active_tab_index;
                                    let tab_color = if is_active {
                                        egui::Color32::from_rgb(255, 60, 0) // Orange for active
                                    } else {
                                        ui.visuals().text_color()
                                    };
                                    
                                    let tab_bg = egui::Color32::from_rgb(0, 0, 0); // Black background for all tabs
                                    
                                    ui.horizontal(|ui| {
                                        // Tab button with connection indicator
                                        let mut tab_text = tab.title.clone();
                                        
                                        // Add connection indicator to tab title
                                        if let Some(conn_id) = tab.connection_id {
                                            if let Some(conn_name) = self.get_connection_name(conn_id) {
                                                tab_text = format!("{} [{}]", tab.title, conn_name);
                                            }
                                        }
                                        
                                        let tab_response = ui.add(
                                            egui::Button::new(
                                                egui::RichText::new(&tab_text)
                                                    .color(tab_color)
                                                    .size(12.0)
                                            )
                                            .fill(tab_bg)
                                            .stroke(egui::Stroke::NONE)
                                        );
                                        
                                        if tab_response.clicked() && !is_active {
                                            tab_to_switch = Some(index);
                                        }
                                        
                                        // Right-click context menu for connection selection
                                        tab_response.context_menu(|ui| {
                                            ui.label("Select Connection:");
                                            ui.separator();
                                            
                                            // None option
                                            if ui.selectable_label(tab.connection_id.is_none(), "None").clicked() {
                                                connection_to_set = Some((index, None));
                                                ui.close_menu();
                                            }
                                            
                                            // Available connections
                                            for connection in &self.connections {
                                                if let Some(conn_id) = connection.id {
                                                    let is_selected = tab.connection_id == Some(conn_id);
                                                    if ui.selectable_label(is_selected, &connection.name).clicked() {
                                                        connection_to_set = Some((index, Some(conn_id)));
                                                        ui.close_menu();
                                                    }
                                                }
                                            }
                                        });
                                        
                                        // Close button (only show for non-active tabs or if more than 1 tab)
                                        if self.query_tabs.len() > 1 || !is_active {
                                            let close_response = ui.add_sized(
                                                [16.0, 16.0],
                                                egui::Button::new("×")
                                                    .fill(egui::Color32::TRANSPARENT)
                                                    .stroke(egui::Stroke::NONE)
                                            );
                                            
                                            if close_response.clicked() {
                                                tab_to_close = Some(index);
                                            }
                                        }
                                    });
                                }
                                
                                // Handle deferred operations after the loop
                                if let Some((tab_index, conn_id)) = connection_to_set {
                                    if let Some(tab) = self.query_tabs.get_mut(tab_index) {
                                        tab.connection_id = conn_id;
                                    }
                                }
                                
                                // New tab button
                                if ui.add_sized(
                                    [24.0, 24.0],
                                    egui::Button::new("+")
                                        .fill(egui::Color32::BLACK)
                                ).clicked() {
                                    editor::create_new_tab(self, "Untitled Query".to_string(), String::new());
                                }
                                
                                // Push gear icon to the far right
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // Gear icon with context menu
                                    let gear_button = ui.add_sized(
                                        [24.0, 24.0],
                                        egui::Button::new("⚙")
                                            .fill(egui::Color32::TRANSPARENT)
                                            .stroke(egui::Stroke::NONE)
                                    );
                                    
                                    // Show context menu when gear is left-clicked
                                    if gear_button.clicked() {
                                        ui.memory_mut(|mem| mem.toggle_popup(egui::Id::new("gear_menu")));
                                    }
                                    
                                    // Render popup menu
                                    egui::popup::popup_below_widget(ui, egui::Id::new("gear_menu"), &gear_button, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                                        ui.set_min_width(120.0); // Set minimum width for the popup
                                        ui.spacing_mut().button_padding = egui::vec2(8.0, 6.0); // Add more padding to buttons
                                        ui.spacing_mut().item_spacing.y = 4.0; // Add vertical spacing between items
                                        
                                        if ui.add_sized([100.0, 24.0], egui::Button::new("Settings")).clicked() {
                                            editor::open_command_palette(self);
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                        if ui.add_sized([100.0, 24.0], egui::Button::new("About")).clicked() {
                                            self.show_about_dialog = true;
                                            ui.memory_mut(|mem| mem.close_popup());
                                        }
                                    });
                                });
                                
                                
                                // Handle tab operations
                                if let Some(index) = tab_to_close {
                                    editor::close_tab(self, index);
                                }
                                if let Some(index) = tab_to_switch {
                                    editor::switch_to_tab(self, index);
                                }
                            });
                            
                            ui.separator();
                            
                            // Update current tab content with editor changes
                            if let Some(current_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                                if current_tab.content != self.editor_text {
                                    current_tab.content = self.editor_text.clone();
                                    current_tab.is_modified = true;
                                }
                            }
                            
                            // SQL Editor area
                            egui::Frame::NONE
                                .fill(if ui.visuals().dark_mode { 
                                    egui::Color32::from_rgb(30, 30, 30) // Slightly darker for contrast
                                } else { 
                                    egui::Color32::WHITE // Pure white for light mode
                                })
                                .inner_margin(egui::Margin::ZERO) // No padding for compact design
                                .show(ui, |ui| {
                                    editor::render_advanced_editor(self, ui);
                                    
                                    // Check for Ctrl+Enter or Cmd+Enter to execute query
                                    if ui.input(|i| {
                                        (i.modifiers.ctrl || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::Enter)
                                    }) {
                                        // Check if there's any query to execute using same priority as execute_query
                                        let has_query = if !self.selected_text.trim().is_empty() {
                                            true
                                        } else {
                                            let cursor_query = editor::extract_query_from_cursor(self);
                                            if !cursor_query.trim().is_empty() {
                                                true
                                            } else {
                                                !self.editor_text.trim().is_empty()
                                            }
                                        };

                                        if has_query {
                                            // Always call execute_query, it will handle connection logic internally
                                            editor::execute_query(self);
                                        }
                                    }
                                    
                                    // Check for Ctrl+S or Cmd+S to save
                                    if ui.input(|i| {
                                        (i.modifiers.ctrl || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::S)
                                    }) {
                                        if let Err(err) = editor::save_current_tab(self) {
                                            error!("Failed to save: {}", err);
                                        }
                                    }
                                });
                        });
                        
                        // Update split ratio when panel is resized
                        let current_height = ui.min_rect().height();
                        self.table_split_ratio = (current_height / available_height).clamp(0.1, 0.9);
                    });
                
                // Table data in the remaining space
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::ZERO)) // Remove all padding
                    .show_inside(ui, |ui| {
                    self.render_table_data(ui);
                });
            });
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Active Tab: {}", self.get_active_tab_title()));
                ui.separator();
                ui.label(format!("Lines: {}", self.editor_text.lines().count()));
                ui.separator();
                                
                // Show selection status
                if !self.selected_text.trim().is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(0, 150, 255), 
                        format!("Selected: {} chars", self.selected_text.len()));
                    ui.separator();
                }
                
                ui.label(format!("Tabs: {}", self.query_tabs.len()));
                ui.separator();
                ui.label(format!("Connections: {}", self.connections.len()));
                ui.separator();
                
                // Connection selector ComboBox for current tab
                if !self.connections.is_empty() {
                    ui.label("Conn:");
                    let current_tab_connection_name = if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
                        if let Some(conn_id) = tab.connection_id {
                            self.get_connection_name(conn_id).unwrap_or_else(|| format!("ID {}", conn_id))
                        } else {
                            "None".to_string()
                        }
                    } else {
                        "No Tab".to_string()
                    };
                    
                    let mut connection_to_set: Option<Option<i64>> = None;
                    
                    egui::ComboBox::from_id_salt("status_tab_connection_selector")
                        .selected_text(&current_tab_connection_name)
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            // Option for no connection
                            if ui.selectable_label(
                                self.query_tabs.get(self.active_tab_index).is_some_and(|tab| tab.connection_id.is_none()), 
                                "None"
                            ).clicked() {
                                connection_to_set = Some(None);
                            }
                            
                            // All available connections
                            for connection in &self.connections {
                                if let Some(connection_id) = connection.id {
                                    let is_selected = self.query_tabs.get(self.active_tab_index).is_some_and(|tab| tab.connection_id == Some(connection_id));
                                    if ui.selectable_label(is_selected, &connection.name).clicked() {
                                        connection_to_set = Some(Some(connection_id));
                                    }
                                }
                            }
                        });
                    
                    // Apply connection change after the borrow is released
                    if let Some(conn_id) = connection_to_set {
                        self.set_active_tab_connection(conn_id);
                    }
                    
                    ui.separator();
                    
                    // Database selector for current tab connection (for MySQL and PostgreSQL)
                    let current_tab_connection = if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
                        tab.connection_id
                    } else {
                        None
                    };
                    
                    let current_tab_database = if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
                        tab.database_name.clone()
                    } else {
                        None
                    };
                    
                    if let Some(conn_id) = current_tab_connection {
                        if let Some(connection) = self.connections.iter().find(|c| c.id == Some(conn_id)) {
                            // Store connection info to avoid borrow checker issues
                            let connection_type = connection.connection_type.clone();
                            let connection_database = connection.database.clone();
                            
                            // Show database selector for all connection types
                            ui.label("Database:");
                            
                            // Get current tab's database selection
                            let current_database = current_tab_database.clone().unwrap_or_else(|| "Select Database".to_string());
                            
                            let mut database_to_set: Option<Option<String>> = None;
                            
                            egui::ComboBox::from_id_salt("status_database_selector")
                                .selected_text(&current_database)
                                .width(120.0)
                                .show_ui(ui, |ui| {
                                    // For MySQL and PostgreSQL, get available databases
                                    if matches!(connection_type, models::enums::DatabaseType::MySQL | models::enums::DatabaseType::PostgreSQL | models::enums::DatabaseType::MSSQL) {
                                        let available_databases = self.get_databases_cached(conn_id);
                                        
                                        if available_databases.is_empty() {
                                            ui.colored_label(egui::Color32::GRAY, "Loading databases...");
                                        } else {
                                            for database in &available_databases {
                                                let is_selected = current_tab_database.as_ref() == Some(database);
                                                if ui.selectable_label(is_selected, database).clicked() {
                                                    database_to_set = Some(Some(database.clone()));
                                                }
                                            }
                                        }
                                    } else {
                                        // For SQLite and Redis, show the configured database
                                        let is_selected = current_tab_database.as_ref() == Some(&connection_database);
                                        if ui.selectable_label(is_selected, &connection_database).clicked() {
                                            database_to_set = Some(Some(connection_database.clone()));
                                        }
                                    }
                                });
                            
                            // Apply database change after the borrow is released
                            if let Some(db_name) = database_to_set {
                                self.set_active_tab_database(db_name.clone());
                                
                                // Clear current table data when switching databases
                                self.current_table_data.clear();
                                self.current_table_headers.clear();
                                self.current_table_name.clear();
                                self.total_rows = 0;
                                self.current_page = 0;
                                
                                if let Some(db) = &db_name {
                                    debug!("Switched to database: {}", db);
                                }
                            }
                            
                            ui.separator();
                        }
                    }
                } else {
                    ui.colored_label(egui::Color32::RED, "No connections available");
                }
                ui.separator();
                
                // Show pagination info
                if self.total_rows > 0 {
                    ui.label(format!("Showing {} of {} rows (page {}/{})", 
                        self.current_table_data.len(), 
                        self.total_rows,
                        self.current_page + 1,
                        self.get_total_pages()));
                } else {
                    ui.label(format!("Showing {} rows", self.current_table_data.len()));
                }
                ui.separator();
                
                // Show execution hint
                if !self.selected_text.trim().is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(100, 200, 100), "CMD+Enter: Execute selection");
                } else {
                    let cursor_query = editor::extract_query_from_cursor(self);
                    if !cursor_query.trim().is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(200, 150, 100), "CMD+Enter: Execute query at cursor");
                    } else {
                        ui.label("CMD+Enter: Execute all");
                    }
                }
            });
        });
    }
}
