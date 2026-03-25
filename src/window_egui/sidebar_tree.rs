use eframe::egui;
use std::sync::Arc;
use std::collections::HashMap;
use log::{debug, info};
use super::Tabular;
use crate::spreadsheet::SpreadsheetOperations;
use crate::{models, connection, editor, sidebar_database,
            sidebar_query, data_table, driver_mssql, directory};

pub(crate) struct RenderTreeNodeParams<'a> {
    node_index: usize,
    refreshing_connections: &'a std::collections::HashSet<i64>,
    connection_pools: &'a std::collections::HashMap<i64, models::enums::DatabasePool>,
    pending_connection_pools: &'a std::collections::HashSet<i64>,
    shared_connection_pools:
        &'a Arc<std::sync::Mutex<std::collections::HashMap<i64, models::enums::DatabasePool>>>,
    is_search_mode: bool,
    // New: fallback map of connection_id -> DatabaseType for DB type detection when pool not ready
    connection_types: &'a std::collections::HashMap<i64, models::enums::DatabaseType>,
    // Prefetch progress tracking
    prefetch_progress: &'a HashMap<i64, (usize, usize)>,
    // Pre-loaded PNG textures for DB type icons (key = DatabaseType::icon_key())
    db_icon_textures: &'a HashMap<String, egui::TextureHandle>,
}


impl super::Tabular {
    pub fn get_connection_name(&self, connection_id: i64) -> Option<String> {
        self.connections
            .iter()
            .find(|conn| conn.id == Some(connection_id))
            .map(|conn| conn.name.clone())
    }
    pub fn render_tree(
        &mut self,
        ui: &mut egui::Ui,
        nodes: &mut [models::structs::TreeNode],
        is_search_mode: bool,
    ) -> Vec<(String, String, String, Option<i64>)> {
        // Process pending auto-load requests FIRST, before rendering
        // This ensures expanded nodes are loaded from cache before first render
        let pending_loads: Vec<i64> = self.pending_auto_load.drain().collect();
        if !pending_loads.is_empty() {
            info!(
                "📂 Processing {} pending auto-loads BEFORE render",
                pending_loads.len()
            );
        }
        for connection_id in pending_loads {
            info!("📂 Processing auto-load for connection {}", connection_id);
            // Find the connection node
            let mut found = false;
            for node in nodes.iter_mut() {
                if node.node_type == models::enums::NodeType::Connection
                    && node.connection_id == Some(connection_id)
                {
                    info!("   ✅ Found connection node: {}", node.name);
                    info!("   🔄 Loading expanded nodes recursively from cache...");
                    self.load_expanded_nodes_recursive(connection_id, node);
                    found = true;
                    break;
                }
            }
            if !found {
                info!("   ❌ Connection node {} not found in tree!", connection_id);
            }
        }

        // Build quick lookup: connection_id -> DatabaseType
        let mut connection_types: std::collections::HashMap<i64, models::enums::DatabaseType> =
            std::collections::HashMap::new();
        for c in &self.connections {
            if let Some(id) = c.id {
                connection_types.insert(id, c.connection_type.clone());
            }
        }
        let mut expansion_requests = Vec::new();
        let mut tables_to_expand = Vec::new();
        let mut context_menu_requests = Vec::new();
        let mut table_click_requests: Vec<(i64, String, models::enums::NodeType, Option<String>)> = Vec::new();
        let mut connection_click_requests = Vec::new();
        let mut index_click_requests: Vec<(i64, String, Option<String>, Option<String>)> =
            Vec::new();
        let mut create_index_requests: Vec<(i64, Option<String>, Option<String>)> = Vec::new();
        let mut alter_table_requests: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut query_files_to_open: Vec<(String, String, String, Option<i64>)> = Vec::new();
        let mut create_table_requests: Vec<(i64, Option<String>)> = Vec::new();
        let mut stored_procedure_click_requests: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut generate_ddl_requests: Vec<(i64, Option<String>, String)> = Vec::new();
        let mut open_diagram_requests: Vec<(i64, String)> = Vec::new();
        let mut add_view_requests: Vec<i64> = Vec::new();
        let mut custom_view_click_requests: Vec<(i64, String, String)> = Vec::new();
        let mut delete_custom_view_requests: Vec<(i64, String)> = Vec::new();
        let mut edit_custom_view_requests: Vec<(i64, String, String)> = Vec::new();

        for (index, node) in nodes.iter_mut().enumerate() {
            let (
                expansion_request,
                table_expansion,
                context_menu_request,
                table_click_request,
                connection_click_request,
                query_file_to_open,
                folder_for_removal,
                parent_for_creation,
                folder_removal_mapping,
                _dba_click_request,
                index_click_request,
                create_index_request,
                alter_table_request,
                request_add_replication_dialog,
                drop_collection_request,
                drop_table_request,
                create_table_request,
                stored_procedure_click_request,
                generate_ddl_request,
                open_diagram_request,
                request_add_view_dialog,
                custom_view_click_request,
                delete_custom_view_request,
                edit_custom_view_request,
            ) = Self::render_tree_node_with_table_expansion(
                ui,
                node,
                &mut self.editor,
                RenderTreeNodeParams {
                    node_index: index,
                    refreshing_connections: &self.refreshing_connections,
                    connection_pools: &self.connection_pools,
                    pending_connection_pools: &self.pending_connection_pools,
                    shared_connection_pools: &self.shared_connection_pools,
                    is_search_mode,
                    connection_types: &connection_types,
                    prefetch_progress: &self.prefetch_progress,
                    db_icon_textures: &self.db_icon_textures,
                },
            );
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
            }
            if let Some(parent_folder) = parent_for_creation {
                self.parent_folder_for_creation = Some(parent_folder);
            }
            if let Some(context_id) = context_menu_request {
                context_menu_requests.push(context_id);
            }
            if let Some((connection_id, table_name, node_type, db_name)) = table_click_request {
                table_click_requests.push((connection_id, table_name, node_type, db_name));
            }
            if let Some(connection_id) = connection_click_request {
                connection_click_requests.push(connection_id);
            }
            if let Some((filename, content, file_path)) = query_file_to_open {
                query_files_to_open.push((filename, content, file_path, node.connection_id));
            }
            if let Some((conn_id, db_name, table_name)) = alter_table_request {
                alter_table_requests.push((conn_id, db_name, table_name));
            }
            // Collect DBA quick view requests
            // Collect Custom View click requests (Run immediately like DBA Views)
            if let Some(req) = custom_view_click_request {
                custom_view_click_requests.push(req);
            }
            // Collect index click requests
            if let Some((conn_id, index_name, db_name, table_name)) = index_click_request {
                index_click_requests.push((conn_id, index_name, db_name, table_name));
            }
            // Collect create index requests
            if let Some((conn_id, db_name, table_name)) = create_index_request {
                create_index_requests.push((conn_id, db_name, table_name));
            }
            // Collect Mongo drop collection requests
            if let Some((conn_id, db, coll)) = drop_collection_request {
                // Store pending state for confirmation window outside the loop
                self.pending_drop_collection = Some((conn_id, db, coll));
            }
            // Collect DROP TABLE requests
            if let Some((conn_id, db, table, stmt)) = drop_table_request {
                // Store pending state for confirmation window outside the loop
                self.pending_drop_table = Some((conn_id, db, table, stmt));
            }
            if let Some((conn_id, db_name)) = create_table_request {
                create_table_requests.push((conn_id, db_name));
            }
            if let Some((conn_id, db_name, proc_name)) = stored_procedure_click_request {
                stored_procedure_click_requests.push((conn_id, db_name, proc_name));
            }
            if let Some((conn_id, db_name, table_name)) = generate_ddl_request {
                generate_ddl_requests.push((conn_id, db_name, table_name));
            }
            if let Some((conn_id, db_name)) = open_diagram_request {
                open_diagram_requests.push((conn_id, db_name));
            }

            if let Some(conn_id) = request_add_view_dialog {
                log::warn!("!!! REQUEST ADD VIEW DIALOG for conn_id: {}", conn_id);
                add_view_requests.push(conn_id);
            }
            if let Some(conn_id) = request_add_replication_dialog {
                self.show_add_replication_dialog = true;
                self.replication_dialog = Some(models::structs::ReplicationDialogState::new(conn_id));
            }
            if let Some(req) = delete_custom_view_request {
                delete_custom_view_requests.push(req);
            }
            if let Some(req) = edit_custom_view_request {
                edit_custom_view_requests.push(req);
            }
        }

        // Process collected Custom View requests OUTSIDE the loop
        for (conn_id, view_name, query) in custom_view_click_requests {
             editor::create_new_tab_with_connection(
                self,
                view_name.clone(),
                query.clone(),
                Some(conn_id),
            );

            // Detect special mode from query (Preserve DBA special modes)
            let trimmed_query = query.trim();
            let special_mode = if trimmed_query.eq_ignore_ascii_case("SHOW REPLICA STATUS;") {
                Some(models::enums::DBASpecialMode::ReplicationStatus)
            } else if trimmed_query.eq_ignore_ascii_case("SHOW MASTER STATUS;") {
                Some(models::enums::DBASpecialMode::MasterStatus)
            } else {
                None
            };
            
            if let Some(mode) = special_mode
                && let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
            {
                tab.dba_special_mode = Some(mode);
            }
            
            self.current_connection_id = Some(conn_id);
             // Ensure (or kick off) connection pool before executing
            if let Some(rt) = self.runtime.clone() {
                rt.block_on(async {
                    let _ =
                        crate::connection::get_or_create_connection_pool(self, conn_id)
                            .await;
                });
            }
            
            // Auto run query
             if let Some((headers, data)) =
                    connection::execute_query_with_connection(self, conn_id, query.clone())
                {
                    self.current_table_headers = headers;
                    self.current_table_data = data.clone();
                    self.all_table_data = data;
                    self.current_table_name = view_name;
                    self.is_table_browse_mode = false;
                    self.total_rows = self.all_table_data.len();
                    self.current_page = 0;
                    
                    // Mark as executed
                    if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                         tab.has_executed_query = true;
                    }
                }
        }

        // Process add view requests
        for conn_id in add_view_requests {
             self.show_add_view_dialog = true;
             self.new_view_connection_id = Some(conn_id);
             self.new_view_name = String::new();
             self.new_view_query = "SELECT * FROM ...".to_string();
        }

        if let Some((conn_id, view_name)) = delete_custom_view_requests.pop() {
            let mut conn_to_save = None;
            // Find connection and remove view
            if let Some(conn) = self.connections.iter_mut().find(|c| c.id == Some(conn_id)) {
                 conn.custom_views.retain(|v| v.name != view_name);
                 conn_to_save = Some(conn.clone());
            }

            // Save connection (outside of mutable borrow of connections)
            if let Some(conn) = conn_to_save
                 && crate::sidebar_database::save_connection_to_database(self, &conn) {
                     crate::sidebar_database::refresh_connections_tree(self);
                 }
        }

        if let Some((conn_id, view_name, query)) = edit_custom_view_requests.pop() {
            self.show_add_view_dialog = true;
            self.new_view_connection_id = Some(conn_id);
            self.new_view_name = view_name.clone();
            self.new_view_query = query;
            self.edit_view_original_name = Some(view_name);
        }


        for (conn_id, db_name) in create_table_requests {
            self.open_create_table_wizard(conn_id, db_name);
        }

        for (conn_id, db_name) in open_diagram_requests {
            // 1. Fetch Foreign Keys (blocking for now, MVP)
            let mut fks = Vec::new();
            let mut columns_map = std::collections::HashMap::new();
            if let Some(rt) = self.runtime.clone() {
                // Ensure pool exists
                rt.block_on(async {
                    let _ = crate::connection::get_or_create_connection_pool(self, conn_id).await;
                    fks = crate::connection::get_foreign_keys(self, conn_id, &db_name).await;
                    
                    // Fetch all columns for diagram (MySQL optimization)
                    if let Some(pool_enum) = self.connection_pools.get(&conn_id)
                         && let models::enums::DatabasePool::MySQL(p) = pool_enum
                             && let Ok(cols) = crate::driver_mysql::fetch_mysql_columns(p, &db_name).await {
                                 columns_map = cols;
                             }
                });
            }

            // 1b. Fetch All Tables (to ensure isolated tables are shown)
            let mut all_tables = Vec::new();
            let db_type = self.connections.iter().find(|c| c.id == Some(conn_id)).map(|c| c.connection_type.clone());
            match db_type {
                Some(models::enums::DatabaseType::MySQL) => {
                     if let Some(t) = crate::driver_mysql::fetch_tables_from_mysql_connection(self, conn_id, &db_name, "table") {
                         all_tables = t;
                     }
                },
                Some(models::enums::DatabaseType::PostgreSQL) => {
                      if let Some(t) = crate::driver_postgres::fetch_tables_from_postgres_connection(self, conn_id, &db_name, "BASE TABLE") {
                          all_tables = t;
                      }
                },
                Some(models::enums::DatabaseType::SQLite) => {
                      if let Some(t) = crate::driver_sqlite::fetch_tables_from_sqlite_connection(self, conn_id, "table") {
                          all_tables = t;
                      }
                },
                Some(models::enums::DatabaseType::MsSQL) => {
                      if let Some(t) = crate::driver_mssql::fetch_tables_from_mssql_connection(self, conn_id, &db_name, "table") {
                          all_tables = t;
                      }
                },
                _ => {}
            }

            // 2. Initialize Diagram State
            let mut state = self.load_diagram(conn_id, &db_name).unwrap_or_default();
            
            // Populate nodes (tables)
            let mut table_names = std::collections::HashSet::new();
            for fk in &fks {
                table_names.insert(fk.table_name.clone());
                table_names.insert(fk.referenced_table_name.clone());
            }
            log::info!("Diagram Init: Found {} FKs and {} tables", fks.len(), all_tables.len());
            for t in all_tables {
                table_names.insert(t);
            }
            
            for (t_name, cols) in &columns_map {
                log::debug!("Table {} has {} columns", t_name, cols.len());
            }

            // Sync FKs (edges) - Always refresh edges based on current Schema
            let edges: Vec<models::structs::DiagramEdge> = fks.iter().map(|fk| models::structs::DiagramEdge {
                source: fk.table_name.clone(),
                target: fk.referenced_table_name.clone(),
                label: "".to_string(),
            }).collect();
            state.edges = edges;

            // Grouping Logic (Refresh groups if empty or for new nodes?)
            // For MVP, we regenerate groups map for new nodes usage, 
            // but we should probably keep existing groups if possible?
            // Let's re-calculate groups for ALL tables.
            let mut groups_map: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
            
            // Helper to get prefix
            let get_prefix = |name: &str| -> String {
                name.split('_').next().unwrap_or(name).to_string()
            };

            for table in &table_names {
                let prefix = get_prefix(table);
                groups_map.entry(prefix).or_default().push(table.to_string());
            }

            // Update/Create DiagramGroups 
            let mut existing_group_ids: std::collections::HashSet<String> = state.groups.iter().map(|g| g.id.clone()).collect();
            
            // Simple color palette generator
            let colors = [
                eframe::egui::Color32::from_rgb(100, 149, 237), // Cornflower Blue
                eframe::egui::Color32::from_rgb(60, 179, 113),  // Medium Sea Green
                eframe::egui::Color32::from_rgb(255, 0, 0),   // Indian Red
                eframe::egui::Color32::from_rgb(218, 165, 32),  // Goldenrod
                eframe::egui::Color32::from_rgb(147, 112, 219), // Medium Purple
                eframe::egui::Color32::from_rgb(70, 130, 180),  // Steel Blue
                eframe::egui::Color32::from_rgb(255, 127, 80),  // Coral
            ];
            let mut color_idx = 0;

            for (prefix, tables) in groups_map {
                if tables.len() > 1 {
                    let group_id = format!("group_{}", prefix);
                    
                    if !existing_group_ids.contains(&group_id) {
                        let title = prefix[0..1].to_uppercase() + &prefix[1..]; // Capitalize
                        let color = colors[color_idx % colors.len()];
                        color_idx += 1;

                        state.groups.push(models::structs::DiagramGroup {
                            id: group_id.clone(),
                            title,
                            color,
                            manual_pos: None,
                        });
                        existing_group_ids.insert(group_id.clone());
                    }
                }
            }

            // Sync Nodes
            // 1. Remove nodes that no longer exist
            state.nodes.retain(|n| table_names.contains(&n.id));
            
            // 2. Identify new nodes
            let existing_node_ids: std::collections::HashSet<String> = state.nodes.iter().map(|n| n.id.clone()).collect();
            let new_tables: Vec<String> = table_names.iter().filter(|t| !existing_node_ids.contains(*t)).cloned().collect();
            let is_init = state.nodes.is_empty();

            // apply to state (this block replaces the old logic)
            // We need to call layout ONLY if it was empty, or only for new nodes?
            // If we have saved state, we DON'T run full auto layout that resets everything.
            
            // Add new nodes
            for table in new_tables {
                 let hash: u64 = table.bytes().fold(5381, |acc, c| acc.wrapping_shl(5).wrapping_add(acc).wrapping_add(c as u64));
                 let x = (hash % 800) as f32 + 100.0;
                 let y = ((hash / 800) % 600) as f32 + 100.0;
                 
                  let mut node = models::structs::DiagramNode {
                    id: table.clone(),
                    title: table.clone(),
                     pos: eframe::egui::pos2(x, y),
                     size: eframe::egui::vec2(150.0, 100.0), // Default, will be auto-sized
                     columns: columns_map.get(&table).cloned().unwrap_or_default(),
                     foreign_keys: fks.iter().filter(|fk| fk.table_name == table).cloned().collect(),
                     group_id: None,
                 };
                // Assign group
                let prefix = get_prefix(&table);
                if existing_group_ids.contains(&format!("group_{}", prefix)) {
                     node.group_id = Some(format!("group_{}", prefix));
                }
                state.nodes.push(node);
            }
            
             // Refresh columns for existing nodes too (in case of schema change)
             for node in &mut state.nodes {
                  if let Some(cols) = columns_map.get(&node.id) {
                      node.columns = cols.clone();
                  }
             }

            // Apply Layout ONLY if it was fresh init (no saved state used)
            if is_init {
                 crate::diagram_view::perform_auto_layout(&mut state);
            }
            
            // 3. Create Tab
            // fks consumed? No, we used iter().
            // Original code used into_iter() for edges. I replaced it with iter above.


            // 3. Create Tab
            let title = format!("Diagram: {}", db_name);
            editor::create_new_tab_with_connection_and_database(
                self,
                title,
                String::new(), // No query content
                Some(conn_id),
                Some(db_name.clone()),
            );
            
            // 4. Attach Diagram State to the new active tab
            if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                tab.diagram_state = Some(state);
            }
            self.table_bottom_view = models::structs::TableBottomView::Query;
        }

        for (conn_id, db_name, table_name) in generate_ddl_requests {
            if let Some(conn) = self.connections.iter().find(|c| c.id == Some(conn_id)).cloned() {
                let definition = crate::connection::fetch_table_definition(&conn, db_name.as_deref(), &table_name);
                if let Some(sql) = definition {
                    let title = format!("DDL: {}", table_name);
                    crate::editor::create_new_tab_with_connection_and_database(
                        self,
                        title,
                        sql,
                        Some(conn_id),
                        db_name.clone(),
                    );
                    self.table_bottom_view = models::structs::TableBottomView::Query;
                } else {
                    self.error_message = format!("Could not generate DDL for table '{}'. It might not be supported for this database type.", table_name);
                    self.show_error_message = true;
                }
            }
        }

        for (connection_id, database_name, table_name) in alter_table_requests {
            self.handle_alter_table_request(connection_id, database_name, table_name);
        }

        // Handle stored procedure clicks - open the actual definition in a new tab (no templates)
        for (conn_id, db_name, proc_name) in stored_procedure_click_requests {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.id == Some(conn_id))
                .cloned()
            {
                let script =
                    connection::fetch_procedure_definition(&conn, db_name.as_deref(), &proc_name)
                        // If we can't fetch, just show the procedure name (no template as requested)
                        .unwrap_or_else(|| proc_name.clone());

                let title = format!("Procedure: {}", proc_name);
                editor::create_new_tab_with_connection_and_database(
                    self,
                    title,
                    script,
                    Some(conn_id),
                    db_name.clone(),
                );
                // Ensure the active tab stores selected database context for later executions
                if let (Some(dbn), Some(active_tab)) =
                    (db_name, self.query_tabs.get_mut(self.active_tab_index))
                {
                    active_tab.database_name = Some(dbn);
                }
                // Focus Query view
                self.table_bottom_view = models::structs::TableBottomView::Query;
            }
        }

        // Handle connection clicks (create new tab with that connection)
        // We'll collect connection IDs needing eager pool creation to process after loop
        let mut pools_to_create: Vec<i64> = Vec::new();

        // Check table clicks for missing pools too
        for (connection_id, _, _, _) in &table_click_requests {
             if !self.connection_pools.contains_key(connection_id) && !pools_to_create.contains(connection_id) {
                 pools_to_create.push(*connection_id);
             }
        }

        for connection_id in connection_click_requests {
            // Find connection name and type for tab title & behavior
            let (connection_name, is_api_http) = self
                .connections
                .iter()
                .find(|conn| conn.id == Some(connection_id))
                .map(|conn| (conn.name.clone(), conn.connection_type == models::enums::DatabaseType::ApiHttp))
                .unwrap_or_else(|| (format!("Connection {}", connection_id), false));

            // Create new tab with this connection pre-selected
            let tab_title = if is_api_http {
                connection_name.clone()
            } else {
                format!("Query - {}", connection_name)
            };
            editor::create_new_tab_with_connection(
                self,
                tab_title,
                String::new(),
                Some(connection_id),
            );

            // For API-HTTP connections, set up the HTTP client state on the new tab
            if is_api_http
                && let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                    // Load previously saved state if available, else use defaults
                    let state = crate::http_client::load_http_state(connection_id)
                        .unwrap_or_default();
                    tab.http_client_state = Some(state);
                }

            debug!("Created new tab with connection ID: {}", connection_id);

            // NEW: Immediately (lazily-once) create the underlying connection pool so that
            // first table/data click feels faster. Previously pool was only created
            // when executing a query or expanding tables.
            if !is_api_http && !self.connection_pools.contains_key(&connection_id) {
                pools_to_create.push(connection_id);
            } else if !is_api_http {
                debug!(
                    "✅ Connection pool already exists for {} (click)",
                    connection_id
                );
            }
        }

        // Now create pools (after mutable/immutable borrows ended)
        // Now create pools (after mutable/immutable borrows ended)
        if !pools_to_create.is_empty() {
             for cid in pools_to_create {
                 if !self.connection_pools.contains_key(&cid) {
                    crate::connection::start_background_pool_creation(self, cid);
                 }
             }
        }

        // Handle expansions after rendering
        for expansion_req in expansion_requests {
            match expansion_req.node_type {
                models::enums::NodeType::Connection => {
                    // Find Connection node recursively and load if not already loaded
                    if let Some(connection_node) =
                        Self::find_connection_node_recursive(nodes, expansion_req.connection_id)
                    {
                        if !connection_node.is_loaded {
                            self.load_connection_tables(
                                expansion_req.connection_id,
                                connection_node,
                            );
                        }
                    } else {
                        debug!(
                            "Connection node not found for ID: {}",
                            expansion_req.connection_id
                        );
                    }
                }
                models::enums::NodeType::DatabasesFolder => {
                    // Handle DatabasesFolder expansion - load actual databases from server
                    for node in nodes.iter_mut() {
                        if node.node_type == models::enums::NodeType::Connection
                            && node.connection_id == Some(expansion_req.connection_id)
                        {
                            // Find the DatabasesFolder within this connection
                            for child in &mut node.children {
                                if child.node_type == models::enums::NodeType::DatabasesFolder
                                    && !child.is_loaded
                                {
                                    self.load_databases_for_folder(
                                        expansion_req.connection_id,
                                        child,
                                    );
                                    break;
                                }
                            }
                            break;
                        }
                    }
                }
                models::enums::NodeType::Database => {
                    debug!(
                        "🔍 Database expansion request received for connection_id: {}, database_name: {:?}",
                        expansion_req.connection_id, expansion_req.database_name
                    );

                    // Handle Database expansion for Redis - load keys for the database
                    if let Some(connection) = self
                        .connections
                        .iter()
                        .find(|c| c.id == Some(expansion_req.connection_id))
                    {
                        debug!(
                            "✅ Found connection: {} (type: {:?})",
                            connection.name, connection.connection_type
                        );

                        if connection.connection_type == models::enums::DatabaseType::Redis {
                            debug!("🔑 Processing Redis database expansion");

                            // Find the database node and load its keys
                            let mut node_found = false;
                            for (node_idx, node) in nodes.iter_mut().enumerate() {
                                debug!(
                                    "🌳 Checking tree node [{}]: '{}' (type: {:?}, connection_id: {:?})",
                                    node_idx, node.name, node.node_type, node.connection_id
                                );

                                if let Some(db_node) = Self::find_redis_database_node(
                                    node,
                                    expansion_req.connection_id,
                                    &expansion_req.database_name,
                                ) {
                                    debug!(
                                        "📁 Found database node: {}, is_loaded: {}",
                                        db_node.name, db_node.is_loaded
                                    );
                                    node_found = true;

                                    if !db_node.is_loaded {
                                        debug!(
                                            "⏳ Loading keys for database: {}",
                                            expansion_req.database_name.clone().unwrap_or_default()
                                        );
                                        self.load_redis_keys_for_database(
                                            expansion_req.connection_id,
                                            &expansion_req
                                                .database_name
                                                .clone()
                                                .unwrap_or_default(),
                                            db_node,
                                        );
                                    } else {
                                        debug!(
                                            "✅ Database already loaded with {} children",
                                            db_node.children.len()
                                        );
                                    }
                                    break;
                                }
                            }

                            if !node_found {
                                debug!(
                                    "❌ Database node not found in any tree branch for database: {:?}",
                                    expansion_req.database_name
                                );
                            }
                        } else {
                            debug!(
                                "❌ Connection is not Redis type: {:?}",
                                connection.connection_type
                            );
                        }
                    } else {
                        debug!(
                            "❌ Connection not found for ID: {}",
                            expansion_req.connection_id
                        );
                    }
                }
                models::enums::NodeType::TablesFolder
                | models::enums::NodeType::ViewsFolder
                | models::enums::NodeType::StoredProceduresFolder
                | models::enums::NodeType::UserFunctionsFolder
                | models::enums::NodeType::TriggersFolder
                | models::enums::NodeType::EventsFolder => {
                    // Find the specific folder node and load if not already loaded

                    // We need to find the exact folder node in the tree
                    let connection_id = expansion_req.connection_id;
                    let folder_type = expansion_req.node_type.clone();
                    let database_name = expansion_req.database_name.clone();

                    // Search for folder node by traversing the tree recursively
                    let mut found = false;
                    for node in nodes.iter_mut() {
                        // Search recursively through all nodes, not just top level
                        if let Some(folder_node) = Self::find_specific_folder_node(
                            node,
                            connection_id,
                            &folder_type,
                            &database_name,
                        ) {
                            if !folder_node.is_loaded {
                                self.load_folder_content(
                                    connection_id,
                                    folder_node,
                                    folder_type.clone(),
                                );
                                found = true;
                            }
                            break;
                        }
                    }
                    if !found {
                        debug!(
                            "Could not find folder node with type {:?} and database {:?} in any of the nodes",
                            folder_type, database_name
                        );
                    }
                }
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
        for (connection_id, table_name, node_type, predefined_db_name) in table_click_requests {
            // Find the connection to determine the database type and database name
            let connection = self
                .connections
                .iter()
                .find(|conn| conn.id == Some(connection_id))
                .cloned();

            if let Some(conn) = connection {
                let is_view = node_type == models::enums::NodeType::View;
                // Find the database name from the tree structure
                let mut database_name: Option<String> = predefined_db_name;

                // Optimization: Only search if not provided (should be provided for most table clicks)
                if database_name.is_none() {
                    for node in nodes.iter() {
                        if let Some(db_name) =
                            Tabular::find_database_name_for_table(node, connection_id, &table_name)
                        {
                            database_name = Some(db_name);
                            break;
                        }
                    }
                }

                // If no database found in tree, use connection default
                if database_name.is_none() {
                    database_name = Some(conn.database.clone());
                }

                match conn.connection_type {
                    models::enums::DatabaseType::Redis => {
                        // Redis objects never carry ALTER view DDL
                        self.current_object_ddl = None;
                        // Check if this is a Redis key (has specific Redis data types in the tree structure)
                        // For Redis keys, we need to find which database they belong to
                        let mut is_redis_key = false;
                        let mut key_type: Option<String> = None;

                        for node in nodes.iter() {
                            if let Some((_, k_type)) =
                                Tabular::find_redis_key_info(node, &table_name)
                            {
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
                                    "zset" | "sorted_set" => {
                                        format!("ZRANGE {} 0 -1 WITHSCORES", table_name)
                                    }
                                    "stream" => format!("XRANGE {} - +", table_name),
                                    _ => format!("TYPE {}", table_name), // Fallback to show type
                                };

                                let tab_title = format!("Redis Key: {} ({})", table_name, k_type);
                                editor::create_new_tab_with_connection_and_database(
                                    self,
                                    tab_title,
                                    redis_command.clone(),
                                    Some(connection_id),
                                    database_name.clone(),
                                );

                                // Set current connection ID and database for Redis query execution
                                self.current_connection_id = Some(connection_id);

                                // Auto-execute the Redis query and display results in bottom
                                if let Some((headers, data)) =
                                    connection::execute_query_with_connection(
                                        self,
                                        connection_id,
                                        redis_command,
                                    )
                                {
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
                                    let clean_name =
                                        table_name.split('(').next().unwrap_or(&table_name).trim();
                                    format!("SCAN 0 MATCH *:* COUNT 100 # Browse {}", clean_name)
                                }
                            };
                            let tab_title = format!("Redis {}", table_name);
                            editor::create_new_tab_with_connection_and_database(
                                self,
                                tab_title,
                                redis_command.clone(),
                                Some(connection_id),
                                database_name.clone(),
                            );

                            // Set database and auto-execute
                            self.current_connection_id = Some(connection_id);
                            // Reset spreadsheet editing state when opening a key browse
                            self.reset_spreadsheet_state();
                            if let Some((headers, data)) = connection::execute_query_with_connection(
                                self,
                                connection_id,
                                redis_command,
                            ) {
                                self.current_table_headers = headers;
                                self.current_table_data = data.clone();
                                self.all_table_data = data;
                                self.current_table_name = format!("Redis {}", table_name);
                                self.total_rows = self.all_table_data.len();
                                self.current_page = 0;
                                if let Some(active_tab) =
                                    self.query_tabs.get_mut(self.active_tab_index)
                                {
                                    active_tab.result_headers = self.current_table_headers.clone();
                                    active_tab.result_rows = self.current_table_data.clone();
                                    active_tab.result_all_rows = self.all_table_data.clone();
                                    active_tab.result_table_name = self.current_table_name.clone();
                                    active_tab.is_table_browse_mode = self.is_table_browse_mode;
                                    active_tab.current_page = self.current_page;
                                    active_tab.page_size = self.page_size;
                                    active_tab.total_rows = self.total_rows;
                                }
                            }
                        }
                    }
                    models::enums::DatabaseType::MongoDB => {
                        // For MongoDB, treat table_name as a collection; database_name must be present
                        if let Some(db_name) = &database_name {
                            let tab_title = format!("Collection: {}.{}", db_name, table_name);
                            editor::create_new_tab_with_connection_and_database(
                                self,
                                tab_title.clone(),
                                String::new(),
                                Some(connection_id),
                                database_name.clone(),
                            );
                            self.current_connection_id = Some(connection_id);
                            // Reset spreadsheet editing state when opening a collection
                            self.reset_spreadsheet_state();
                            if let Some((headers, data)) =
                                crate::driver_mongodb::sample_collection_documents(
                                    self,
                                    connection_id,
                                    db_name,
                                    &table_name,
                                    100,
                                )
                            {
                                self.current_table_headers = headers;
                                self.current_table_data = data.clone();
                                self.all_table_data = data;
                                self.current_table_name = tab_title;
                                self.total_rows = self.all_table_data.len();
                                self.current_page = 0;
                                if let Some(active_tab) =
                                    self.query_tabs.get_mut(self.active_tab_index)
                                {
                                    active_tab.result_headers = self.current_table_headers.clone();
                                    active_tab.result_rows = self.current_table_data.clone();
                                    active_tab.result_all_rows = self.all_table_data.clone();
                                    active_tab.result_table_name = self.current_table_name.clone();
                                    active_tab.is_table_browse_mode = self.is_table_browse_mode;
                                    active_tab.current_page = self.current_page;
                                    active_tab.page_size = self.page_size;
                                    active_tab.total_rows = self.total_rows;
                                }
                            }
                        } else {
                            self.error_message =
                                "MongoDB requires a database; please select a database."
                                    .to_string();
                            self.show_error_message = true;
                        }
                    }
                    _ => {
                        if !is_view
                            && self.table_bottom_view == models::structs::TableBottomView::Query
                        {
                            self.table_bottom_view = models::structs::TableBottomView::Data;
                        }
                        self.current_object_ddl = None;
                        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            active_tab.object_ddl = None;
                        }
                        // SQL databases - use regular SELECT query with proper database context
                        let query_content = if let Some(db_name) = &database_name {
                            match conn.connection_type {
                                models::enums::DatabaseType::MySQL => {
                                    format!(
                                        "USE `{}`;\nSELECT * FROM `{}` LIMIT 100;",
                                        db_name, table_name
                                    )
                                }
                                models::enums::DatabaseType::PostgreSQL => {
                                    format!(
                                        "SELECT * FROM \"{}\".\"{}\" LIMIT 100;",
                                        db_name, table_name
                                    )
                                }
                                models::enums::DatabaseType::MsSQL => {
                                    // Build robust MsSQL SELECT with explicit database context
                                    driver_mssql::build_mssql_select_query(
                                        db_name.clone(),
                                        table_name.clone(),
                                    )
                                }
                                models::enums::DatabaseType::SQLite
                                | models::enums::DatabaseType::Redis => {
                                    format!("SELECT * FROM `{}` LIMIT 100;", table_name)
                                }
                                models::enums::DatabaseType::MongoDB
                                | models::enums::DatabaseType::ApiHttp => {
                                    // Unreachable here; MongoDB/ApiHttp handled above with sampling
                                    String::new()
                                }
                            }
                        } else {
                            match conn.connection_type {
                                models::enums::DatabaseType::MsSQL => {
                                    driver_mssql::build_mssql_select_query(
                                        "".to_string(),
                                        table_name.clone(),
                                    )
                                }
                                _ => format!("SELECT * FROM `{}` LIMIT 100;", table_name),
                            }
                        };
                        let tab_title = if is_view {
                            format!("View: {}", table_name)
                        } else {
                            format!("Table: {}", table_name)
                        };
                        editor::create_new_tab_with_connection_and_database(
                            self,
                            tab_title.clone(),
                            query_content.clone(),
                            Some(connection_id),
                            database_name.clone(),
                        );

                        // Reset spreadsheet editing state when opening a table
                        self.reset_spreadsheet_state();

                        // Set database context for current tab and auto-execute the query and display results in bottom
                        self.current_connection_id = Some(connection_id);
                        // Ensure the newly created tab stores selected database (important for MsSQL)
                        if let Some(dbn) = &database_name
                            && let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index)
                        {
                            active_tab.database_name = Some(dbn.clone());
                        }

                        // Set early so infer_current_table_name() bekerja saat Structure view aktif
                        let label_prefix = if is_view { "View" } else { "Table" };
                        self.current_table_name = format!(
                            "{}: {} (Database: {})",
                            label_prefix,
                            table_name,
                            database_name.as_deref().unwrap_or("Unknown")
                        );

                        // Clear newly created rows highlight when switching tables
                        self.newly_created_rows.clear();

                        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            active_tab.result_table_name = self.current_table_name.clone();
                        }

                        // Try show cached 100 rows immediately (cache-first UX)
                        let mut had_cache = false;
                        if let Some(dbn) = &database_name
                            && let Some((cached_headers, cached_rows)) =
                                crate::cache_data::get_table_rows_from_cache(
                                    self,
                                    connection_id,
                                    dbn,
                                    &table_name,
                                )
                            && !cached_headers.is_empty()
                        {
                            info!(
                                "📦 Showing cached data for table {}/{} ({} cols, {} rows)",
                                dbn,
                                table_name,
                                cached_headers.len(),
                                cached_rows.len()
                            );
                            self.current_table_headers = cached_headers.clone();
                            self.current_table_data = cached_rows.clone();
                            self.all_table_data = cached_rows;
                            self.total_rows = self.all_table_data.len();
                            self.current_page = 0;
                            had_cache = true;
                            // Table context changed; ensure future Structure load is for this table
                            self.last_structure_target = None;
                            if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index)
                            {
                                active_tab.result_headers = self.current_table_headers.clone();
                                active_tab.result_rows = self.current_table_data.clone();
                                active_tab.result_all_rows = self.all_table_data.clone();
                                active_tab.result_table_name = self.current_table_name.clone();
                                active_tab.is_table_browse_mode = true;
                                active_tab.current_page = self.current_page;
                                active_tab.page_size = self.page_size;
                                active_tab.total_rows = self.total_rows;
                            }
                        }

                        // Use server-side pagination only when refreshing or when no cache available.
                        if self.use_server_pagination {
                            // Build base query without LIMIT for potential server pagination (store for future refresh),
                            // but don't execute it if we already have cache.
                            let base_query = if let Some(db_name) = &database_name {
                                match conn.connection_type {
                                    models::enums::DatabaseType::MySQL => {
                                        format!(
                                            "USE `{}`;\nSELECT * FROM `{}`",
                                            db_name, table_name
                                        )
                                    }
                                    models::enums::DatabaseType::PostgreSQL => {
                                        format!("SELECT * FROM \"{}\".\"{}\"", db_name, table_name)
                                    }
                                    models::enums::DatabaseType::MsSQL => {
                                        // Build robust MsSQL SELECT with explicit database context but without LIMIT
                                        let mssql_query = driver_mssql::build_mssql_select_query(
                                            db_name.clone(),
                                            table_name.clone(),
                                        );
                                        // Remove the LIMIT part from MsSQL query
                                        mssql_query.replace("SELECT TOP 100", "SELECT")
                                    }
                                    models::enums::DatabaseType::SQLite
                                    | models::enums::DatabaseType::Redis => {
                                        format!("SELECT * FROM `{}`", table_name)
                                    }
                                    models::enums::DatabaseType::MongoDB
                                    | models::enums::DatabaseType::ApiHttp => {
                                        // MongoDB/ApiHttp handled separately above
                                        String::new()
                                    }
                                }
                            } else {
                                match conn.connection_type {
                                    models::enums::DatabaseType::MsSQL => {
                                        let mssql_query = driver_mssql::build_mssql_select_query(
                                            "".to_string(),
                                            table_name.clone(),
                                        );
                                        mssql_query.replace("SELECT TOP 100", "SELECT")
                                    }
                                    _ => format!("SELECT * FROM `{}`", table_name),
                                }
                            };
                            // Always store base_query for potential manual refresh
                            if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index)
                            {
                                active_tab.base_query = base_query.clone();
                            }
                            self.current_base_query = base_query;

                            // If we already showed cache, do NOT auto-fetch from server now.
                            if had_cache {
                                debug!(
                                    "🛑 Skipping live server load on table click because cache exists"
                                );
                                // Keep browse mode enabled for filters to apply on cached data
                                self.is_table_browse_mode = true;
                                self.sql_filter_text.clear();
                                // New table opened; structure target should refresh on demand
                                self.last_structure_target = None;
                            } else {
                                // Set browse mode when opening table via sidebar click
                                self.is_table_browse_mode = true;
                                // If the pool is not ready, queue the first-page query; otherwise execute.
                                let mut pool_ready = true;
                                if self.pending_connection_pools.contains(&connection_id) {
                                    pool_ready = false;
                                } else if !self.connection_pools.contains_key(&connection_id) {
                                    let created_now = if let Some(rt) = self.runtime.clone() {
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    } else {
                                        let rt = self.get_runtime();
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    };
                                    if !created_now {
                                        pool_ready = false;
                                    }
                                }

                                if !pool_ready {
                                    // Prepare server pagination state but defer execution
                                    self.current_page = 0;
                                    if let Some(total) = self.execute_count_query() {
                                        self.actual_total_rows = Some(total);
                                    }
                                    let first_query = self.build_paginated_query(0, self.page_size);
                                    self.pool_wait_in_progress = true;
                                    self.pool_wait_connection_id = Some(connection_id);
                                    self.pool_wait_query = first_query;
                                    self.pool_wait_started_at = Some(std::time::Instant::now());
                                    self.current_table_name =
                                        "Connecting… waiting for pool".to_string();
                                } else {
                                    self.initialize_server_pagination(
                                        self.current_base_query.clone(),
                                    );
                                }
                            }
                        } else {
                            // Client-side path (rare). Only run live query if no cache.
                            if !had_cache {
                                // Set browse mode when opening table via sidebar click
                                self.is_table_browse_mode = true;
                                println!("================== 1 ============================ ");
                                debug!("🔄 Taking client-side pagination fallback path");
                                info!(
                                    "🌐 Loading live data from server for table {}/{} (client pagination)",
                                    database_name.clone().unwrap_or_default(),
                                    table_name
                                );
                                // New table; force structure reload on next toggle
                                self.last_structure_target = None;
                                // Fallback to client-side pagination (original behavior)
                                // For MsSQL, we need to strip TOP from query_content to avoid conflicts
                                let safe_query =
                                    if conn.connection_type == models::enums::DatabaseType::MsSQL {
                                        driver_mssql::sanitize_mssql_select_for_pagination(
                                            &query_content,
                                        )
                                    } else {
                                        query_content.clone()
                                    };
                                debug!("🔄 Client-side query after sanitization: {}", safe_query);

                                // If pool not ready, queue and show loading; otherwise execute now
                                let mut pool_ready = true;
                                if self.pending_connection_pools.contains(&connection_id) {
                                    pool_ready = false;
                                } else if !self.connection_pools.contains_key(&connection_id) {
                                    let created_now = if let Some(rt) = self.runtime.clone() {
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    } else {
                                        let rt = self.get_runtime();
                                        rt.block_on(async {
                                            crate::connection::try_get_connection_pool(
                                                self,
                                                connection_id,
                                            )
                                            .await
                                            .is_some()
                                        })
                                    };
                                    if !created_now {
                                        pool_ready = false;
                                    }
                                }

                                if !pool_ready {
                                    self.pool_wait_in_progress = true;
                                    self.pool_wait_connection_id = Some(connection_id);
                                    self.pool_wait_query = safe_query;
                                    self.pool_wait_started_at = Some(std::time::Instant::now());
                                    self.current_table_name =
                                        "Connecting… waiting for pool".to_string();
                                } else if let Some((headers, data)) =
                                    connection::execute_query_with_connection(
                                        self,
                                        connection_id,
                                        safe_query,
                                    )
                                {
                                    self.current_table_headers = headers;
                                    self.current_table_data = data.clone();
                                    self.all_table_data = data;
                                    // current_table_name sudah diset lebih awal
                                    self.is_table_browse_mode = true; // Enable filter for table browse
                                    self.sql_filter_text.clear(); // Clear any previous filter
                                    self.total_rows = self.all_table_data.len();
                                    self.current_page = 0;
                                    if let Some(active_tab) =
                                        self.query_tabs.get_mut(self.active_tab_index)
                                    {
                                        active_tab.result_headers =
                                            self.current_table_headers.clone();
                                        active_tab.result_rows = self.current_table_data.clone();
                                        active_tab.result_all_rows = self.all_table_data.clone();
                                        active_tab.result_table_name =
                                            self.current_table_name.clone();
                                        active_tab.is_table_browse_mode = self.is_table_browse_mode;
                                        active_tab.current_page = self.current_page;
                                        active_tab.page_size = self.page_size;
                                        active_tab.total_rows = self.total_rows;
                                    }
                                    // Save latest first page into row cache (best-effort)
                                    if let Some(dbn) = &database_name {
                                        let snapshot: Vec<Vec<String>> =
                                            self.all_table_data.iter().take(100).cloned().collect();
                                        let headers_clone = self.current_table_headers.clone();
                                        crate::cache_data::save_table_rows_to_cache(
                                            self,
                                            connection_id,
                                            dbn,
                                            &table_name,
                                            &headers_clone,
                                            &snapshot,
                                        );
                                        info!(
                                            "💾 Cached first 100 rows after live fetch for {}/{}",
                                            dbn, table_name
                                        );
                                    }
                                }
                            } else {
                                debug!(
                                    "🛑 Skipping client-side live load on table click because cache exists"
                                );
                                self.last_structure_target = None;
                            }
                        }
                    }
                };

                if is_view {
                    let ddl = connection::fetch_view_definition(
                        &conn,
                        database_name.as_deref(),
                        &table_name,
                    )
                    .unwrap_or_else(|| {
                        format!("-- Unable to fetch view definition for {}", table_name)
                    });
                    self.current_object_ddl = Some(ddl.clone());
                    if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                        active_tab.object_ddl = Some(ddl);
                    }
                    self.table_bottom_view = models::structs::TableBottomView::Query;
                }
            }

            // FIX: Jika user sedang berada pada view Structure dan berpindah klik ke table lain,
            // sebelumnya struktur tidak di-refresh sehingga masih menampilkan struktur table lama.
            // Di sini kita paksa reload struktur untuk table baru.
            if self.table_bottom_view == models::structs::TableBottomView::Structure {
                // Load only if target changed
                if let Some(conn_id) = self.current_connection_id {
                    let db = self
                        .query_tabs
                        .get(self.active_tab_index)
                        .and_then(|t| t.database_name.clone())
                        .unwrap_or_default();
                    let table = data_table::infer_current_table_name(self);
                    let current_target = (conn_id, db.clone(), table.clone());
                    if self
                        .last_structure_target
                        .as_ref()
                        .map(|t| t != &current_target)
                        .unwrap_or(true)
                    {
                        data_table::load_structure_info_for_current_table(self);
                    }
                } else {
                    data_table::load_structure_info_for_current_table(self);
                }
            } else {
                // Pastikan struktur lama dibersihkan agar ketika user pindah ke Structure langsung memicu load.
                self.structure_columns.clear();
                self.structure_indexes.clear();
            }
        }

        // Handle index click requests - open Edit Index dialog
        for (connection_id, index_name, database_name, table_name) in index_click_requests {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .cloned()
            {
                // Prefill dialog state for Edit
                if let Some(tn) = table_name.clone() {
                    self.index_dialog = Some(models::structs::IndexDialogState {
                        mode: models::structs::IndexDialogMode::Edit,
                        connection_id,
                        database_name: database_name.clone(),
                        table_name: tn,
                        existing_index_name: Some(index_name.clone()),
                        index_name: index_name.clone(),
                        columns: String::new(),
                        unique: false,
                        method: None,
                        db_type: conn.connection_type.clone(),
                    });
                    self.show_index_dialog = true;
                }
            }
        }

        // Handle create index requests - open Create Index dialog
        for (connection_id, database_name, table_name) in create_index_requests {
            if let Some(conn) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .cloned()
                && let Some(tn) = table_name.clone()
            {
                self.index_dialog = Some(models::structs::IndexDialogState {
                    mode: models::structs::IndexDialogMode::Create,
                    connection_id,
                    database_name: database_name.clone(),
                    table_name: tn.clone(),
                    existing_index_name: None,
                    index_name: format!("idx_{}_col", tn),
                    columns: "columns comma-separated".to_string(),
                    unique: false,
                    method: None,
                    db_type: conn.connection_type.clone(),
                });
                self.show_index_dialog = true;
            }
        }

        let results = query_files_to_open.clone();

        // Handle context menu requests (deduplicate to avoid multiple calls)
        let mut processed_removals = std::collections::HashSet::new();
        let mut processed_refreshes = std::collections::HashSet::new();
        let mut needs_full_refresh = false;

        for context_id in context_menu_requests {
            debug!("🔍 Processing context_id: {}", context_id);

            if context_id >= 62000 {
                // Restart Replication
                let conn_id = context_id - 62000;
                let replica_pool_opt = self.connection_pools.get(&conn_id).cloned();
                let mut master_id_opt = None;
                if let Some(conn) = self.connections.iter().find(|c| c.id == Some(conn_id)) {
                    master_id_opt = conn.replication_master_id;
                }
                
                if let (Some(models::enums::DatabasePool::MySQL(replica_pool)), Some(master_id)) = (replica_pool_opt, master_id_opt) {
                     if let Some(models::enums::DatabasePool::MySQL(master_pool)) = self.connection_pools.get(&master_id).cloned() {
                         let rt = self.get_runtime();
                         let (tx, rx) = std::sync::mpsc::channel();
                         self.replication_setup_receiver = Some(rx);
                         
                         rt.spawn(async move {
                             let res = crate::driver_mysql::restart_replication(&master_pool, &replica_pool).await;
                             let _ = tx.send(res);
                         });
                         
                         self.query_message = "Restarting replication...".to_string();
                         self.show_message_panel = true;
                         self.query_message_is_error = false;
                     } else {
                         self.query_message = "Master connection is not active. Please connect to Master first.".to_string();
                         self.show_message_panel = true;
                         self.query_message_is_error = true;
                     }
                } else {
                     self.query_message = "Could not identify Master connection or pools not active.".to_string();
                     self.show_message_panel = true;
                     self.query_message_is_error = true;
                }
            } else if context_id >= 61000 {
                // Stop Replication
                let conn_id = context_id - 61000;
                if let Some(models::enums::DatabasePool::MySQL(pool)) = self.connection_pools.get(&conn_id).cloned() {
                     let rt = self.get_runtime();
                     let (tx, rx) = std::sync::mpsc::channel();
                     self.replication_setup_receiver = Some(rx);
                     rt.spawn(async move {
                         let res = crate::driver_mysql::stop_replication(&pool).await;
                         let _ = tx.send(res);
                     });
                     self.query_message = "Stopping replication...".to_string();
                     self.show_message_panel = true;
                     self.query_message_is_error = false;
                }
            } else if context_id >= 60000 {
                // Start Replication
                let conn_id = context_id - 60000;
                if let Some(models::enums::DatabasePool::MySQL(pool)) = self.connection_pools.get(&conn_id).cloned() {
                     let rt = self.get_runtime();
                     let (tx, rx) = std::sync::mpsc::channel();
                     self.replication_setup_receiver = Some(rx);
                     rt.spawn(async move {
                         let res = crate::driver_mysql::start_replication(&pool).await;
                         let _ = tx.send(res);
                     });
                     self.query_message = "Starting replication...".to_string();
                     self.show_message_panel = true;
                     self.query_message_is_error = false;
                }
            } else if context_id >= 50000 {
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
                debug!(
                    "📋 Copy connection operation for connection: {}",
                    connection_id
                );
                sidebar_database::copy_connection(self, connection_id);

                // Force immediate tree refresh and UI update
                self.items_tree.clear();
                sidebar_database::refresh_connections_tree(self);
                needs_full_refresh = true;
                ui.ctx().request_repaint();

                // Break early to prevent further processing
                break;
            } else if (3000..4000).contains(&context_id) {
                // ID 3000-3999 means disconnect (connection_id = context_id - 3000)
                let connection_id = context_id - 3000;
                debug!("🔌 Disconnect operation for connection: {}", connection_id);
                self.disconnect_connection(connection_id);
                // Mark for repaint so status updates immediately
                ui.ctx().request_repaint();
            } else if (1000..10000).contains(&context_id) {
                // ID 1000-9999 means refresh connection (connection_id = context_id - 1000)
                let connection_id = context_id - 1000;
                debug!(
                    "🔄 Refresh connection operation for connection: {}",
                    connection_id
                );
                if !processed_refreshes.contains(&connection_id) {
                    processed_refreshes.insert(connection_id);
                    // Only refresh that single connection node without rebuilding the whole tree
                    self.refresh_connection(connection_id);
                    // Mark for repaint so spinner state shows immediately
                    ui.ctx().request_repaint();
                    // Do NOT trigger full tree rebuild here; preserving folder expansion avoids the
                    // perception that the connection disappeared after refresh.
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

                    // No need for full tree refresh - remove_connection already does incremental update
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

        // Handle DnD drop: move connection to target folder
        let dnd_drop: Option<(i64, String)> = ui
            .ctx()
            .data(|d| d.get_temp(egui::Id::new("conn_dnd_drop")));
        if let Some((drag_conn_id, target_folder)) = dnd_drop {
            log::warn!("[DnD] EXECUTING DROP conn_id={} -> folder='{}'", drag_conn_id, target_folder);
            ui.ctx().data_mut(|d| {
                d.remove_temp::<(i64, String)>(egui::Id::new("conn_dnd_drop"));
                d.remove_temp::<i64>(egui::Id::new("conn_dnd_source"));
            });
            if let Some(conn) = self
                .connections
                .iter_mut()
                .find(|c| c.id == Some(drag_conn_id))
            {
                conn.folder = Some(target_folder);
                let conn_clone = conn.clone();
                sidebar_database::update_connection_in_database(self, &conn_clone);
                sidebar_database::refresh_connections_tree(self);
                self.needs_refresh = true;
                ui.ctx().request_repaint();
            }
        }

        // Handle "Create Subfolder" context menu request
        let subfolder_req: Option<String> = ui
            .ctx()
            .data(|d| d.get_temp(egui::Id::new("conn_subfolder_req")));
        if let Some(parent_path) = subfolder_req {
            ui.ctx().data_mut(|d| {
                d.remove_temp::<String>(egui::Id::new("conn_subfolder_req"));
            });
            self.subfolder_parent_path = parent_path;
            self.new_subfolder_name.clear();
            self.show_create_subfolder_dialog = true;
        }

        // Handle "Add Connection Here" context menu request
        let add_to_folder: Option<String> = ui
            .ctx()
            .data(|d| d.get_temp(egui::Id::new("conn_add_to_folder")));
        if let Some(folder_path) = add_to_folder {
            ui.ctx().data_mut(|d| {
                d.remove_temp::<String>(egui::Id::new("conn_add_to_folder"));
            });
            self.new_connection.folder = Some(folder_path);
            self.show_add_connection = true;
        }

        // Return query files that were clicked
        results
    }
    pub(crate) fn render_tree_node_with_table_expansion(
        ui: &mut egui::Ui,
        node: &mut models::structs::TreeNode,
        editor: &mut crate::editor_buffer::EditorBuffer,
        params: RenderTreeNodeParams,
    ) -> models::structs::RenderTreeNodeResult {
        let has_children = !node.children.is_empty();
        let mut expansion_request = None;
        let mut table_expansion = None;
        let mut context_menu_request = None;
        let mut table_click_request: Option<(i64, String, models::enums::NodeType, Option<String>)> = None;
        let mut folder_removal_mapping: Option<(i64, String)> = None;
        let mut connection_click_request = None;
        let mut query_file_to_open = None;
        let mut folder_name_for_removal = None;
        let mut parent_folder_for_creation = None;
        let mut dba_click_request: Option<(i64, models::enums::NodeType)> = None;
        let mut index_click_request: Option<(i64, String, Option<String>, Option<String>)> = None;
        let mut create_index_request: Option<(i64, Option<String>, Option<String>)> = None;
        let mut alter_table_request: Option<(i64, Option<String>, String)> = None;
        let mut drop_collection_request: Option<(i64, String, String)> = None;
        let mut drop_table_request: Option<(i64, String, String, String)> = None;
        let mut create_table_request: Option<(i64, Option<String>)> = None;
        let mut request_add_view_dialog: Option<i64> = None;
        let mut stored_procedure_click_request: Option<(i64, Option<String>, String)> = None;
        let mut generate_ddl_request: Option<(i64, Option<String>, String)> = None;
        let mut open_diagram_request: Option<(i64, String)> = None;
        let mut custom_view_click_request: Option<(i64, String, String)> = None;
        let mut delete_custom_view_request: Option<(i64, String)> = None;
        let mut edit_custom_view_request: Option<(i64, String, String)> = None;
        let mut request_add_replication_dialog: Option<i64> = None;

        if has_children || node.node_type == models::enums::NodeType::Connection || node.node_type == models::enums::NodeType::Table ||
       node.node_type == models::enums::NodeType::View ||
        // Show expand toggles for container folders and schema folders only
       node.node_type == models::enums::NodeType::DatabasesFolder || node.node_type == models::enums::NodeType::TablesFolder ||
       node.node_type == models::enums::NodeType::ViewsFolder || node.node_type == models::enums::NodeType::StoredProceduresFolder ||
       node.node_type == models::enums::NodeType::UserFunctionsFolder || node.node_type == models::enums::NodeType::TriggersFolder ||
    node.node_type == models::enums::NodeType::EventsFolder || node.node_type == models::enums::NodeType::DBAViewsFolder ||
       // Do NOT show expand toggles for DBA leaf items; they act as actions when clicked
    node.node_type == models::enums::NodeType::Database || node.node_type == models::enums::NodeType::QueryFolder
       // Always use the main (expandable) path for CustomFolder so DnD drop target code runs
       // regardless of whether the folder currently has children.
       || node.node_type == models::enums::NodeType::CustomFolder
        {
            // Use more unique ID including connection_id for connections
            let unique_id = match node.node_type {
                models::enums::NodeType::Connection => {
                    format!(
                        "conn_{}_{}",
                        params.node_index,
                        node.connection_id.unwrap_or(0)
                    )
                }
                _ => format!("node_{}_{:?}", params.node_index, node.node_type),
            };
            let id = egui::Id::new(&unique_id);
            ui.horizontal(|ui| {
                // Painter-drawn triangle toggle (no font dependency)
                if Self::triangle_toggle(ui, node.is_expanded).clicked() {
                    node.is_expanded = !node.is_expanded;

                    // If this is a connection node and not loaded, request expansion
                    if node.node_type == models::enums::NodeType::Connection
                        && !node.is_loaded
                        && node.is_expanded
                        && let Some(conn_id) = node.connection_id
                    {
                        expansion_request = Some(models::structs::ExpansionRequest {
                            node_type: models::enums::NodeType::Connection,
                            connection_id: conn_id,
                            database_name: None,
                        });
                        // Also set as active connection when expanding
                        connection_click_request = Some(conn_id);
                    }

                    // If this is a table or view node and not loaded, request column expansion
                    // In search mode, always allow expansion even if already loaded
                    if (node.node_type == models::enums::NodeType::Table
                        || node.node_type == models::enums::NodeType::View)
                        && node.is_expanded
                        && ((!node.is_loaded) || params.is_search_mode)
                        && let Some(conn_id) = node.connection_id
                    {
                        // Use stored raw table_name if present; otherwise sanitize display name (strip emojis / annotations)
                        let raw_name = node
                            .table_name
                            .clone()
                            .unwrap_or_else(|| Self::sanitize_display_table_name(&node.name));
                        table_expansion = Some((params.node_index, conn_id, raw_name));
                    }

                    // If this is a folder node and not loaded, request folder content expansion
                    if (node.node_type == models::enums::NodeType::DatabasesFolder
                        || node.node_type == models::enums::NodeType::TablesFolder
                        || node.node_type == models::enums::NodeType::ViewsFolder
                        || node.node_type == models::enums::NodeType::StoredProceduresFolder
                        || node.node_type == models::enums::NodeType::UserFunctionsFolder
                        || node.node_type == models::enums::NodeType::TriggersFolder
                        || node.node_type == models::enums::NodeType::EventsFolder
                        || node.node_type == models::enums::NodeType::ColumnsFolder
                        || node.node_type == models::enums::NodeType::IndexesFolder
                        || node.node_type == models::enums::NodeType::PrimaryKeysFolder
                        || node.node_type == models::enums::NodeType::PartitionsFolder)
                        && !node.is_loaded
                        && node.is_expanded
                        && let Some(conn_id) = node.connection_id
                    {
                        expansion_request = Some(models::structs::ExpansionRequest {
                            node_type: node.node_type.clone(),
                            connection_id: conn_id,
                            database_name: node.database_name.clone(),
                        });
                    }

                    // If this is a Database node and not loaded, request database expansion (for Redis keys)
                    if node.node_type == models::enums::NodeType::Database
                        && !node.is_loaded
                        && node.is_expanded
                        && let Some(conn_id) = node.connection_id
                    {
                        expansion_request = Some(models::structs::ExpansionRequest {
                            node_type: models::enums::NodeType::Database,
                            connection_id: conn_id,
                            database_name: node.database_name.clone(),
                        });
                    }
                }

                let icon = match node.node_type {
                    models::enums::NodeType::Database => "🗄",
                    models::enums::NodeType::Table => "",
                    // Use a plain bullet to avoid emoji font issues for column icons
                    models::enums::NodeType::Column => "•",
                    models::enums::NodeType::ColumnsFolder => "📑",
                    models::enums::NodeType::IndexesFolder => "🧭",
                    models::enums::NodeType::PrimaryKeysFolder => "🔑",
                    models::enums::NodeType::PartitionsFolder => "📊",
                    models::enums::NodeType::Index => "#",
                    models::enums::NodeType::Query => "🔍",
                    models::enums::NodeType::QueryHistItem => "📜",
                    models::enums::NodeType::Connection => "",
                    models::enums::NodeType::DatabasesFolder => "📁",
                    models::enums::NodeType::TablesFolder => "📋",
                    models::enums::NodeType::ViewsFolder => "👁",
                    models::enums::NodeType::StoredProceduresFolder => "📦",
                    models::enums::NodeType::CustomView => "👁️",
                    models::enums::NodeType::UserFunctionsFolder => "🔧",
                    models::enums::NodeType::TriggersFolder => "⚡",
                    models::enums::NodeType::EventsFolder => "📅",
                    models::enums::NodeType::DBAViewsFolder => "☢",
                    models::enums::NodeType::UsersFolder => "👥",
                    models::enums::NodeType::PrivilegesFolder => "🔒",
                    models::enums::NodeType::ProcessesFolder => "⚡",
                    models::enums::NodeType::StatusFolder => "📊",
                    models::enums::NodeType::BlockedQueriesFolder => "🚫",
                    models::enums::NodeType::ReplicationStatusFolder => "🔁",
                    models::enums::NodeType::MasterStatusFolder => "⭐",
                    models::enums::NodeType::MetricsUserActiveFolder => "👨‍💼",
                    models::enums::NodeType::View => "👁",
                    models::enums::NodeType::StoredProcedure => "⚛",
                    models::enums::NodeType::UserFunction => "🔧",
                    models::enums::NodeType::Trigger => "⚡",
                    models::enums::NodeType::Event => "📅",
                    models::enums::NodeType::MySQLFolder => "🐬",
                    models::enums::NodeType::PostgreSQLFolder => "🐘",
                    models::enums::NodeType::SQLiteFolder => "📄",
                    models::enums::NodeType::RedisFolder => "🔴",
                    models::enums::NodeType::MongoDBFolder => "🍃",
                    models::enums::NodeType::CustomFolder => "📁",
                    models::enums::NodeType::QueryFolder => "📂",
                    models::enums::NodeType::HistoryDateFolder => "📅",
                    models::enums::NodeType::MsSQLFolder => "🗳️",
                    models::enums::NodeType::DiagramsFolder => "📂",
                    models::enums::NodeType::Diagram => "🗺",
                };

                // Build status info for Connection nodes (used below)
                let (status_color, status_text) = if node.node_type == models::enums::NodeType::Connection {
                    if let Some(conn_id) = node.connection_id {
                        // Determine connected/connecting/disconnected
                        let mut has_shared = false;
                        if let Ok(shared) = params.shared_connection_pools.lock() {
                            has_shared = shared.contains_key(&conn_id);
                        }
                        if params.connection_pools.contains_key(&conn_id) || has_shared {
                            (egui::Color32::from_rgb(46, 204, 113), "Connected") // green
                        } else if params.pending_connection_pools.contains(&conn_id) {
                            (egui::Color32::from_rgb(255, 0, 0), "Connecting") // red
                        } else {
                            (egui::Color32::from_rgb(255, 0, 0), "Disconnected") // red
                        }
                    } else {
                        (egui::Color32::from_rgb(255, 0, 0), "Disconnected")
                    }
                } else { (ui.visuals().text_color(), "") };

                let mut response = if node.node_type == models::enums::NodeType::Connection {
                    // Draw PNG icon or emoji badge (NO status dot — status color goes on the name)
                    if let Some(conn_id) = node.connection_id
                        && let Some(db_type) = params.connection_types.get(&conn_id) {
                            let (r, g, b) = db_type.badge_color();
                            let badge_color = egui::Color32::from_rgb(r, g, b);
                            // PNG icon if loaded, otherwise fall back to emoji
                            let icon_key = db_type.icon_key();
                            if let Some(texture) = params.db_icon_textures.get(icon_key) {
                                ui.add(
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(egui::Vec2::splat(16.0)),
                                );
                            } else {
                                ui.label(db_type.icon());
                            }
                            // Colored short label (e.g. "MY", "PG") for text clarity
                            let badge_text = egui::RichText::new(db_type.badge_label())
                                .strong()
                                .small()
                                .color(badge_color);
                            ui.label(badge_text);
                        }
                    let mut name_text = node.name.clone();
                    if let Some(conn_id) = node.connection_id {
                        // Show refreshing spinner
                        if params.refreshing_connections.contains(&conn_id) {
                            name_text.push_str(" 🔄");
                        }
                        // Show prefetch progress
                        if let Some((completed, total)) = params.prefetch_progress.get(&conn_id) {
                            name_text.push_str(&format!(" 📦 {}/{}", completed, total));
                        }
                    }
                    // Color the connection name: green = connected, red = disconnected/connecting
                    ui.add(
                        egui::Label::new(egui::RichText::new(name_text).color(status_color))
                            .truncate()
                            .sense(egui::Sense::click_and_drag()),
                    )
                } else {
                    // Non-connection nodes: icon + name, truncated to available width and clickable
                    let label_text = if icon.is_empty() {
                        node.name.clone()
                    } else {
                        format!("{} {}", icon, node.name)
                    };
                    // Left-align non-connection labels as well; rely on parent row width for truncation.
                    ui.add(
                        egui::Label::new(label_text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                };

                // Tooltip for connection status
                if node.node_type == models::enums::NodeType::Connection && !status_text.is_empty() {
                    let mut tip = format!("Status: {}", status_text);
                    if let Some(conn_id) = node.connection_id
                        && let Some(db_type) = params.connection_types.get(&conn_id) {
                            let db_name = match db_type {
                                models::enums::DatabaseType::MySQL => "MySQL",
                                models::enums::DatabaseType::PostgreSQL => "PostgreSQL",
                                models::enums::DatabaseType::SQLite => "SQLite",
                                models::enums::DatabaseType::Redis => "Redis",
                                models::enums::DatabaseType::MsSQL => "Microsoft SQL Server",
                                models::enums::DatabaseType::MongoDB => "MongoDB",
                                models::enums::DatabaseType::ApiHttp => "HTTP API",
                            };
                            tip = format!("{} · {}", db_name, tip);
                        }
                    response = response.on_hover_text(tip);
                }

                // Drag source: Connection nodes can be dragged to a folder.
                if node.node_type == models::enums::NodeType::Connection {
                    if let Some(conn_id) = node.connection_id {
                        if response.drag_started() {
                            log::warn!("[DnD] DRAG STARTED conn_id={} name='{}'", conn_id, node.name);
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("conn_dnd_source"), conn_id);
                                d.remove_temp::<String>(egui::Id::new("conn_dnd_pending_folder"));
                            });
                        }
                        // Show drag cursor & ghost label while dragging
                        if response.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
                                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                                    egui::Order::Tooltip,
                                    egui::Id::new("conn_drag_label"),
                                ));
                                painter.text(
                                    pos + egui::vec2(12.0, -8.0),
                                    egui::Align2::LEFT_TOP,
                                    format!("📦 {}", node.name),
                                    egui::FontId::proportional(13.0),
                                    ui.visuals().text_color(),
                                );
                            }
                        }
                        // On drop: read the last hovered folder (set in PREVIOUS frame, so render
                        // order doesn't matter) and execute the move.
                        if response.drag_stopped() {
                            let pending_folder: Option<String> = ui
                                .ctx()
                                .data(|d| d.get_temp(egui::Id::new("conn_dnd_pending_folder")));
                            log::warn!("[DnD] DRAG STOPPED conn_id={} pending_folder={:?}", conn_id, pending_folder);
                            if let Some(folder_path) = pending_folder {
                                log::warn!("[DnD] => setting conn_dnd_drop: ({}, '{}')", conn_id, folder_path);
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(
                                        egui::Id::new("conn_dnd_drop"),
                                        (conn_id, folder_path),
                                    );
                                });
                            }
                            ui.ctx().data_mut(|d| {
                                d.remove_temp::<i64>(egui::Id::new("conn_dnd_source"));
                                d.remove_temp::<String>(egui::Id::new("conn_dnd_pending_folder"));
                            });
                        }
                    }
                }

                // Drop target: CustomFolder nodes accept dragged connections.
                // We update conn_dnd_pending_folder every frame the pointer hovers over a folder
                // while a drag is active.  drag_stopped() on the source reads this value from
                // the PREVIOUS frame so render order doesn't matter.
                // Use Y-only containment so indented subfolders (where label rect is shifted right)
                // are detected as long as the pointer is on the same row, regardless of X offset.
                if node.node_type == models::enums::NodeType::CustomFolder {
                    let is_dragging: bool = ui
                        .ctx()
                        .data(|d| d.get_temp::<i64>(egui::Id::new("conn_dnd_source")).is_some());
                    if is_dragging {
                        let row_rect = response.rect;
                        let ptr_pos = ui.ctx().input(|i| i.pointer.hover_pos());
                        let pointer_over = ptr_pos
                            .map_or(false, |p| p.y >= row_rect.min.y && p.y <= row_rect.max.y);
                        log::warn!(
                            "[DnD] CustomFolder='{}' path={:?} row_rect=({:.0},{:.0})-({:.0},{:.0}) ptr={:?} over={}",
                            node.name,
                            node.file_path,
                            row_rect.min.x, row_rect.min.y,
                            row_rect.max.x, row_rect.max.y,
                            ptr_pos.map(|p| (p.x as i32, p.y as i32)),
                            pointer_over
                        );
                        if pointer_over {
                            // Use foreground layer painter so the highlight isn't clipped by the
                            // indent, but constrain X to the sidebar clip rect (not full screen).
                            let clip = ui.clip_rect();
                            let highlight_rect = egui::Rect::from_min_max(
                                egui::pos2(clip.min.x, row_rect.min.y - 1.0),
                                egui::pos2(clip.max.x, row_rect.max.y + 1.0),
                            );
                            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                                egui::Order::Foreground,
                                egui::Id::new("conn_dnd_highlight"),
                            ));
                            painter.rect_filled(
                                highlight_rect,
                                2.0,
                                egui::Color32::from_rgba_unmultiplied(52, 152, 219, 30),
                            );
                            painter.rect_stroke(
                                highlight_rect,
                                2.0,
                                egui::Stroke::new(2.0, egui::Color32::from_rgb(52, 152, 219)),
                                egui::StrokeKind::Outside,
                            );
                            ui.ctx().request_repaint();
                            // Update the pending folder every frame we hover over it
                            let folder_path = node
                                .file_path
                                .clone()
                                .unwrap_or_else(|| node.name.clone());
                            log::warn!("[DnD] => pending_folder set to '{}'", folder_path);
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new("conn_dnd_pending_folder"),
                                    folder_path,
                                );
                            });
                        }
                    }
                }

                // New: Allow clicking the label to also expand/collapse for expandable nodes
                if response.clicked() {
                    // We toggle on label click for expandable/container nodes, but not for Table/View (they open data)
                    let allow_label_toggle = has_children
                        || matches!(
                            node.node_type,
                            models::enums::NodeType::Connection
                                | models::enums::NodeType::Database
                                | models::enums::NodeType::DatabasesFolder
                                | models::enums::NodeType::TablesFolder
                                | models::enums::NodeType::ViewsFolder
                                | models::enums::NodeType::StoredProceduresFolder
                                | models::enums::NodeType::UserFunctionsFolder
                                | models::enums::NodeType::TriggersFolder
                                | models::enums::NodeType::EventsFolder
                                | models::enums::NodeType::DBAViewsFolder
                                | models::enums::NodeType::UsersFolder
                                | models::enums::NodeType::PrivilegesFolder
                                | models::enums::NodeType::ProcessesFolder
                                | models::enums::NodeType::StatusFolder
                                | models::enums::NodeType::BlockedQueriesFolder
                                | models::enums::NodeType::ReplicationStatusFolder
                                | models::enums::NodeType::MasterStatusFolder
                                | models::enums::NodeType::MetricsUserActiveFolder
                                | models::enums::NodeType::ColumnsFolder
                                | models::enums::NodeType::IndexesFolder
                                | models::enums::NodeType::PrimaryKeysFolder
                        ) && node.node_type != models::enums::NodeType::Table
                            && node.node_type != models::enums::NodeType::View;

                    if allow_label_toggle {
                        node.is_expanded = !node.is_expanded;

                        // Mirror triangle click behaviors for lazy-loading
                        if node.node_type == models::enums::NodeType::Connection
                            && !node.is_loaded
                            && node.is_expanded
                            && let Some(conn_id) = node.connection_id
                        {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: models::enums::NodeType::Connection,
                                connection_id: conn_id,
                                database_name: None,
                            });
                            // Also set as active connection when expanding
                            connection_click_request = Some(conn_id);
                        }

                        if (node.node_type == models::enums::NodeType::DatabasesFolder
                            || node.node_type == models::enums::NodeType::TablesFolder
                            || node.node_type == models::enums::NodeType::ViewsFolder
                            || node.node_type == models::enums::NodeType::StoredProceduresFolder
                            || node.node_type == models::enums::NodeType::UserFunctionsFolder
                            || node.node_type == models::enums::NodeType::TriggersFolder
                            || node.node_type == models::enums::NodeType::EventsFolder
                            || node.node_type == models::enums::NodeType::ColumnsFolder
                            || node.node_type == models::enums::NodeType::IndexesFolder
                            || node.node_type == models::enums::NodeType::PrimaryKeysFolder)
                            && !node.is_loaded
                            && node.is_expanded
                            && let Some(conn_id) = node.connection_id
                        {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: node.node_type.clone(),
                                connection_id: conn_id,
                                database_name: node.database_name.clone(),
                            });
                        }

                        // Database node expansion (e.g., Redis keys)
                        if node.node_type == models::enums::NodeType::Database
                            && !node.is_loaded
                            && node.is_expanded
                            && let Some(conn_id) = node.connection_id
                        {
                            expansion_request = Some(models::structs::ExpansionRequest {
                                node_type: models::enums::NodeType::Database,
                                connection_id: conn_id,
                                database_name: node.database_name.clone(),
                            });
                        }
                    }
                }

                // Handle clicks on connection labels to set active connection
                if node.node_type == models::enums::NodeType::Connection
                    && response.clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    connection_click_request = Some(conn_id);
                }

                // Handle clicks on table/view labels to load data - open in new tab
                if (node.node_type == models::enums::NodeType::Table
                    || node.node_type == models::enums::NodeType::View)
                    && response.double_clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    // Use table_name field if available (for search results), otherwise use node.name
                    let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name).clone();
                    table_click_request = Some((conn_id, actual_table_name, node.node_type.clone(), node.database_name.clone()));
                }

                // Handle clicks on Diagram nodes
                if node.node_type == models::enums::NodeType::Diagram
                    && response.clicked()
                    && let Some(conn_id) = node.connection_id
                {
                    open_diagram_request = Some((conn_id, node.database_name.clone().unwrap_or_default()));
                }

                // Index items: no left-click action; use context menu for Alter Index

                // Add context menu for connection nodes
                if node.node_type == models::enums::NodeType::Connection {
                    response.context_menu(|ui| {
                        if ui.button("📋 Copy Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 10000); // Use +10000 to indicate copy
                            }
                            ui.close();
                        }
                        if ui.button("🔄 Refresh Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use +1000 range to indicate refresh (handled in render_tree handler)
                                context_menu_request = Some(conn_id + 1000);
                            }
                            ui.close();
                        }
                        // NEW: Disconnect option
                        if ui.button("🔌 Disconnect").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Use +3000 range to indicate disconnect (handled in render_tree handler)
                                context_menu_request = Some(conn_id + 3000);
                            }
                            ui.close();
                        }
                        if ui.button("🔧 Edit Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id);
                            }
                            ui.close();
                        }
                        // Add Replication option for MySQL
                        if let Some(conn_id) = node.connection_id
                            && let Some(models::enums::DatabaseType::MySQL) =
                                params.connection_types.get(&conn_id)
                            && ui.button("🔗 Add Replication").clicked()
                        {
                            request_add_replication_dialog = Some(conn_id);
                            ui.close();
                        }
                        if ui.button("🗑 Remove Connection").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(-conn_id); // Negative ID indicates removal
                            }
                            ui.close();
                        }
                    });
                }

                // Add context menu for Replication Status Folder
                if node.node_type == models::enums::NodeType::ReplicationStatusFolder {
                    response.context_menu(|ui| {
                        if ui.button("▶️ Start Replication").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 60000);
                            }
                            ui.close();
                        }
                        if ui.button("⏹️ Stop Replication").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 61000);
                            }
                            ui.close();
                        }
                        if ui.button("🔄 Restart Replication").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                context_menu_request = Some(conn_id + 62000);
                            }
                            ui.close();
                        }
                    });
                }

                // Add context menu for DBA Views folder
                if node.node_type == models::enums::NodeType::DBAViewsFolder {
                    response.context_menu(|ui| {
                        if ui.button("➕ Add New View").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                // Set state to show dialog
                                request_add_view_dialog = Some(conn_id);
                            }
                            ui.close();
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
                            ui.close();
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
                            ui.close();
                        }
                    });
                }

                // Context menu for connection group/subfolder nodes
                if node.node_type == models::enums::NodeType::CustomFolder {
                    let folder_path = node
                        .file_path
                        .clone()
                        .unwrap_or_else(|| node.name.clone());
                    response.context_menu(|ui| {
                        if ui.button("📁 Create Subfolder").clicked() {
                            // Signal: "subfolder_req:<folder_path>"
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new("conn_subfolder_req"),
                                    folder_path.clone(),
                                );
                            });
                            ui.close();
                        }
                        if ui.button("➕ Add Connection Here").clicked() {
                            // Signal: "add_to_folder_req:<folder_path>"
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new("conn_add_to_folder"),
                                    folder_path.clone(),
                                );
                            });
                            ui.close();
                        }
                    });
                }

                if node.node_type == models::enums::NodeType::Database {
                    response.context_menu(|ui| {
                        if let Some(conn_id) = node.connection_id {
                            let db_type = params.connection_types.get(&conn_id);
                            let supported = matches!(
                                db_type,
                                Some(models::enums::DatabaseType::MySQL)
                                    | Some(models::enums::DatabaseType::PostgreSQL)
                                    | Some(models::enums::DatabaseType::SQLite)
                                    | Some(models::enums::DatabaseType::MsSQL)
                            );
                            if supported {
                                if ui.button("➕ Create New Table").clicked() {
                                    let database_name = node
                                        .database_name
                                        .clone()
                                        .or_else(|| Some(node.name.clone()));
                                    create_table_request = Some((conn_id, database_name));
                                    ui.close();
                                }
                                if ui.button("📊 Diagrams").clicked() {
                                    let database_name = node
                                        .database_name
                                        .clone()
                                        .or_else(|| Some(node.name.clone()))
                                        .unwrap_or_default();
                                    open_diagram_request = Some((conn_id, database_name));
                                    ui.close();
                                }
                            } else {
                                ui.label("Create table not supported for this database");
                            }
                        }
                    });
                }

                if node.node_type == models::enums::NodeType::TablesFolder {
                    response.context_menu(|ui| {
                        if let Some(conn_id) = node.connection_id {
                            let db_type = params.connection_types.get(&conn_id);
                            let supported = matches!(
                                db_type,
                                Some(models::enums::DatabaseType::MySQL)
                                    | Some(models::enums::DatabaseType::PostgreSQL)
                                    | Some(models::enums::DatabaseType::SQLite)
                                    | Some(models::enums::DatabaseType::MsSQL)
                            );
                            if supported {
                                if ui.button("➕ Create New Table").clicked() {
                                    create_table_request = Some((
                                        conn_id,
                                        node.database_name.clone(),
                                    ));
                                    ui.close();
                                }
                            } else {
                                ui.label("Create table not supported for this database");
                            }
                        }
                    });
                }

                // Add context menu for table nodes
                if node.node_type == models::enums::NodeType::Table {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                table_click_request = Some((conn_id, actual_table_name, models::enums::NodeType::Table, node.database_name.clone()));
                            }
                            ui.close();
                        }
                        // Detect DB type for MongoDB-specific options using available pools; fallback to connection_types
                        let mut is_mongodb = false;
                        if let Some(conn_id) = node.connection_id {
                            if let Some(pool) = params.connection_pools.get(&conn_id) {
                                if let models::enums::DatabasePool::MongoDB(_) = pool {
                                    is_mongodb = true;
                                }
                            } else if let Some(t) = params.connection_types.get(&conn_id)
                                && *t == models::enums::DatabaseType::MongoDB {
                                    is_mongodb = true;
                                }
                        }

                        if !is_mongodb {
                            if ui.button("📜 Generate Query Create Table").clicked() {
                                if let Some(conn_id) = node.connection_id {
                                    let actual_table_name =
                                        node.table_name.as_ref().unwrap_or(&node.name).clone();
                                    generate_ddl_request = Some((
                                        conn_id,
                                        node.database_name.clone(),
                                        actual_table_name,
                                    ));
                                }
                                ui.close();
                            }
                        } else {
                            // MongoDB specific quick actions
                            if ui.button("🔍 Count Documents (Current Tab)").clicked() {
                                if let Some(db) = node.database_name.as_ref() {
                                    let coll = node.table_name.as_ref().unwrap_or(&node.name);
                                    editor.set_text(format!("// MongoDB mongo shell snippet\ndb.{}.{}.countDocuments({{}});", db, coll));
                                } else {
                                    editor.set_text("// Select a database first for MongoDB operations".to_string());
                                }
                                editor.mark_text_modified();
                                ui.close();
                            }
                            if ui.button("📝 Show Collection Stats (Current Tab)").clicked() {
                                if let Some(db) = node.database_name.as_ref() {
                                    let coll = node.table_name.as_ref().unwrap_or(&node.name);
                                    editor.set_text(format!("db.{}.runCommand({{ collStats: \"{}\" }});", db, coll));
                                } else {
                                    editor.set_text("// Select a database first for MongoDB operations".to_string());
                                }
                                editor.mark_text_modified();
                                ui.close();
                            }
                        }
                        ui.separator();
                        if !is_mongodb {
                            if ui.button("🗑 Drop Table").clicked() {
                                if let (Some(conn_id), Some(db)) = (node.connection_id, node.database_name.as_ref()) {
                                    let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name).clone();
                                    // Generate the DROP TABLE statement with USE database
                                    let stmt = format!("USE [{}];\nDROP TABLE IF EXISTS {};", db, actual_table_name);
                                    drop_table_request = Some((conn_id, db.clone(), actual_table_name, stmt));
                                }
                                ui.close();
                            }
                        } else if ui.button("🗑️ Drop Collection").clicked() {
                            if let (Some(conn_id), Some(db)) = (node.connection_id, node.database_name.as_ref()) {
                                let coll = node.table_name.as_ref().unwrap_or(&node.name).clone();
                                drop_collection_request = Some((conn_id, db.clone(), coll));
                            }
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("➕ Add Index (New Tab)").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                create_index_request = Some((
                                    conn_id,
                                    node.database_name.clone(),
                                    Some(actual_table_name),
                                ));
                            }
                            ui.close();
                        }
                        ui.separator();
                        if !is_mongodb && ui.button("🔧 Alter Table").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                alter_table_request = Some((
                                    conn_id,
                                    node.database_name.clone(),
                                    actual_table_name,
                                ));
                            }
                            ui.close();
                        }
                    });
                }

                // Add context menu for view nodes
                if node.node_type == models::enums::NodeType::View {
                    response.context_menu(|ui| {
                        if ui.button("📊 View Data").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                let actual_table_name =
                                    node.table_name.as_ref().unwrap_or(&node.name).clone();
                                table_click_request = Some((conn_id, actual_table_name, models::enums::NodeType::View, node.database_name.clone()));
                            }
                            ui.close();
                        }
                        if ui.button("📝 DESCRIBE View (Current Tab)").clicked() {
                            // Different DESCRIBE syntax for different database types
                            if node.database_name.is_some() {
                                editor.set_text(format!("DESCRIBE {};", node.name));
                            } else {
                                editor.set_text(format!("PRAGMA table_info({});", node.name)); // SQLite syntax
                            }
                            editor.mark_text_modified();
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("🗂️ Show Columns").clicked() {
                            // Trigger table expansion to show columns
                            if let Some(conn_id) = node.connection_id {
                                table_expansion = Some((0, conn_id, node.name.clone()));
                            }
                            ui.close();
                        }
                    });
                }

                // Context menu for Indexes folder: create index
                if node.node_type == models::enums::NodeType::IndexesFolder {
                    response.context_menu(|ui| {
                        if ui.button("➕ New Index").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                create_index_request = Some((
                                    conn_id,
                                    node.database_name.clone(),
                                    node.table_name.clone(),
                                ));
                            }
                            ui.close();
                        }
                    });
                }

                // Context menu for Index node: edit index
                if node.node_type == models::enums::NodeType::Index {
                    response.context_menu(|ui| {
                        if ui.button("✏️ Edit Index").clicked() {
                            if let Some(conn_id) = node.connection_id {
                                index_click_request = Some((
                                    conn_id,
                                    node.name.clone(),
                                    node.database_name.clone(),
                                    node.table_name.clone(),
                                ));
                            }
                            ui.close();
                        }
                    });
                }
            });

            // (central panel logic handled inside update previously)

            if node.is_expanded {
                // Khusus HistoryDateFolder: render children tanpa indent tambahan (full width)
                let is_history_date_folder =
                    node.node_type == models::enums::NodeType::HistoryDateFolder;
                if is_history_date_folder {
                    for (child_index, child) in node.children.iter_mut().enumerate() {
                        let (
                            child_expansion_request,
                            child_table_expansion,
                            child_context,
                            child_table_click,
                            child_connection_click,
                            _child_query_file,
                            _child_folder_removal,
                            _child_parent_creation,
                            _child_folder_removal_mapping,
                            child_dba_click,
                            child_index_click,
                            child_create_index_request,
                            child_alter_table_request,
                            _child_request_add_replication_dialog,
                            _child_drop_collection_request,
                            _child_drop_table_request,
                            _child_create_table_request,
                            _child_sp_click,
                            child_generate_ddl_request,
                            child_open_diagram_request,
                            child_request_add_view_dialog,
                            child_custom_view_click_request,
                            child_delete_custom_view,
                            child_edit_custom_view,
                        ) = Self::render_tree_node_with_table_expansion(
                            ui,
                            child,
                            editor,
                            RenderTreeNodeParams {
                                node_index: child_index,
                                refreshing_connections: params.refreshing_connections,
                                connection_pools: params.connection_pools,
                                pending_connection_pools: params.pending_connection_pools,
                                shared_connection_pools: params.shared_connection_pools,
                                is_search_mode: params.is_search_mode,
                                connection_types: params.connection_types,
                                prefetch_progress: params.prefetch_progress,
                                db_icon_textures: params.db_icon_textures,
                            },
                        );
                        if let Some(child_expansion) = child_expansion_request {
                            expansion_request = Some(child_expansion);
                        }
                        if table_expansion.is_none()
                            && let Some((child_index, child_conn_id, table_name)) =
                                child_table_expansion
                        {
                            if let Some(conn_id) = node.connection_id {
                                table_expansion = Some((child_index, conn_id, table_name));
                            } else {
                                table_expansion = Some((child_index, child_conn_id, table_name));
                            }
                        }
                        if let Some((conn_id, table_name, node_type, db_name)) = child_table_click {
                            table_click_request = Some((conn_id, table_name, node_type, db_name));
                        }
                        if let Some(v) = child_connection_click {
                            connection_click_request = Some(v);
                        }
                        if let Some(v) = _child_drop_collection_request {
                            drop_collection_request = Some(v);
                        }
                        if let Some(v) = _child_drop_table_request {
                            drop_table_request = Some(v);
                        }
                        if let Some(v) = child_dba_click {
                            dba_click_request = Some(v);
                        }
                        if let Some(v) = child_index_click {
                            index_click_request = Some(v);
                        }
                        if let Some(v) = child_create_index_request {
                            create_index_request = Some(v);
                        }
                        if let Some(v) = child_alter_table_request {
                            alter_table_request = Some(v);
                        }
                        if let Some(v) = _child_create_table_request {
                            create_table_request = Some(v);
                        }
                        if let Some(v) = _child_sp_click {
                            stored_procedure_click_request = Some(v);
                        }
                        if let Some(v) = child_generate_ddl_request {
                            generate_ddl_request = Some(v);
                        }
                        if let Some(v) = child_open_diagram_request {
                            open_diagram_request = Some(v);
                        }
                        if let Some(child_context_id) = child_context {
                            context_menu_request = Some(child_context_id);
                        }
                        // Propagate child query file open requests (History) to parent
                        if let Some(child_query_file) = _child_query_file {
                            query_file_to_open = Some(child_query_file);
                        }
                        if let Some(child_req) = child_request_add_view_dialog {
                            request_add_view_dialog = Some(child_req);
                        }
                        if let Some(child_req) = child_custom_view_click_request {
                            custom_view_click_request = Some(child_req);
                        }
                        if let Some(child_req) = child_delete_custom_view {
                             delete_custom_view_request = Some(child_req);
                        }
                        if let Some(child_req) = child_edit_custom_view {
                             edit_custom_view_request = Some(child_req);
                        }
                    }
                } else {
                    ui.indent(id, |ui| {
                        for (child_index, child) in node.children.iter_mut().enumerate() {
                            let (
                                child_expansion_request,
                                child_table_expansion,
                                child_context,
                                child_table_click,
                                child_connection_click,
                                _child_query_file,
                                _child_folder_removal,
                                _child_parent_creation,
                                _child_folder_removal_mapping,
                                child_dba_click,
                                child_index_click,
                                child_create_index_request,
                                child_alter_table_request,
                                child_request_add_replication_dialog,
                                _child_drop_collection_request,
                                child_drop_table_request,
                                child_create_table_request,
                                child_stored_procedure_click_request,
                                child_generate_ddl_request,
                                child_open_diagram_request,
                                child_request_add_view_dialog,
                                child_custom_view_click_request,
                                child_delete_custom_view_request,
                                child_edit_custom_view_request,
                            ) = Self::render_tree_node_with_table_expansion(
                                ui,
                                child,
                                editor,
                                RenderTreeNodeParams {
                                    node_index: child_index,
                                    refreshing_connections: params.refreshing_connections,
                                    connection_pools: params.connection_pools,
                                    pending_connection_pools: params.pending_connection_pools,
                                    shared_connection_pools: params.shared_connection_pools,
                                    is_search_mode: params.is_search_mode,
                                    connection_types: params.connection_types,
                                    prefetch_progress: params.prefetch_progress,
                                    db_icon_textures: params.db_icon_textures,
                                },
                            );

                            // Handle child expansion requests - propagate to parent
                            if let Some(child_expansion) = child_expansion_request {
                                expansion_request = Some(child_expansion);
                            }

                            // Handle child table expansions with the parent connection ID
                            // Only set if we don't already have a table expansion from this node
                            if table_expansion.is_none()
                                && let Some((child_index, child_conn_id, table_name)) =
                                    child_table_expansion
                            {
                                if let Some(conn_id) = node.connection_id {
                                    table_expansion = Some((child_index, conn_id, table_name));
                                } else {
                                    table_expansion =
                                        Some((child_index, child_conn_id, table_name));
                                }
                            }

                            // Handle child table clicks - propagate to parent
                            if let Some((conn_id, table_name, node_type, db_name)) = child_table_click {
                                table_click_request = Some((conn_id, table_name, node_type, db_name));
                            }
                            // Propagate connection click to parent
                            if let Some(v) = child_connection_click {
                                connection_click_request = Some(v);
                            }
                            // Propagate drop collection request to parent
                            if let Some(v) = _child_drop_collection_request {
                                drop_collection_request = Some(v);
                            }
                            // Propagate drop table request to parent
                            if let Some(v) = child_drop_table_request {
                                drop_table_request = Some(v);
                            }
                            // Propagate DBA click to parent
                            if let Some(v) = child_dba_click {
                                dba_click_request = Some(v);
                            }
                            if let Some(v) = child_index_click {
                                index_click_request = Some(v);
                            }
                            if let Some(v) = child_create_index_request {
                                create_index_request = Some(v);
                            }
                            if let Some(v) = child_alter_table_request {
                                alter_table_request = Some(v);
                            }
                            if let Some(v) = child_create_table_request {
                                create_table_request = Some(v);
                            }
                            if let Some(v) = child_stored_procedure_click_request {
                                stored_procedure_click_request = Some(v);
                            }
                            if let Some(v) = child_generate_ddl_request {
                                generate_ddl_request = Some(v);
                            }
                            if let Some(v) = child_open_diagram_request {
                                open_diagram_request = Some(v);
                            }
                            if let Some(child_req) = child_request_add_view_dialog {
                                request_add_view_dialog = Some(child_req);
                            }
                            if let Some(child_req) = child_custom_view_click_request {
                                custom_view_click_request = Some(child_req);
                            }
                            if let Some(child_req) = child_delete_custom_view_request {
                                delete_custom_view_request = Some(child_req);
                            }
                            if let Some(child_req) = child_edit_custom_view_request {
                                edit_custom_view_request = Some(child_req);
                            }
                            if let Some(child_req) = child_request_add_replication_dialog {
                                request_add_replication_dialog = Some(child_req);
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
                // Special handling for history items - make the entire area clickable
                let available_width = ui.available_width();
                let button_response = ui.add_sized(
                    [
                        available_width,
                        ui.text_style_height(&egui::TextStyle::Body),
                    ],
                    egui::Button::new(format!("📜  {}", node.name))
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(egui::Stroke::NONE),
                );

                // Add tooltip with the full query if available
                if let Some(data) = &node.file_path {
                    if let Some((connection_name, original_query)) = data.split_once("||") {
                        button_response.on_hover_text_at_pointer(format!(
                            "Connection: {}\nFull query:\n{}",
                            connection_name, original_query
                        ))
                    } else {
                        button_response.on_hover_text_at_pointer(format!("Full query:\n{}", data))
                    }
                } else {
                    button_response
                }
            } else {
                // For all other node types, use horizontal layout with icons.
                // Add a spacer equal to the triangle width so leaf rows align with expandable rows (left-aligned look).
                ui.horizontal(|ui| {
                    // Reserve space equal to triangle toggle width (16px) for alignment
                    let _sp = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());

                    let icon = match node.node_type {
                        models::enums::NodeType::Database => "🗄",
                        models::enums::NodeType::Table => "",
                        // Use a plain bullet again for columns in fallback rendering
                        models::enums::NodeType::Column => "•",
                        models::enums::NodeType::Query => "🔍",
                        models::enums::NodeType::Connection => "🔗",
                        models::enums::NodeType::DatabasesFolder => "📁",
                        models::enums::NodeType::TablesFolder => "📋",
                        models::enums::NodeType::ViewsFolder => "👁",
                        models::enums::NodeType::StoredProceduresFolder => "📦",
                        models::enums::NodeType::UserFunctionsFolder => "🔧",
                        models::enums::NodeType::TriggersFolder => "⚡",
                        models::enums::NodeType::EventsFolder => "📅",
                        models::enums::NodeType::DBAViewsFolder => "☢",
                        models::enums::NodeType::UsersFolder => "👥",
                        models::enums::NodeType::PrivilegesFolder => "🔒",
                        models::enums::NodeType::ProcessesFolder => "⚡",
                        models::enums::NodeType::StatusFolder => "📊",
                        models::enums::NodeType::BlockedQueriesFolder => "🚫",
                        models::enums::NodeType::ReplicationStatusFolder => "🔁",
                        models::enums::NodeType::MasterStatusFolder => "⭐",
                        models::enums::NodeType::View => "👁",
                        models::enums::NodeType::StoredProcedure => "⚛",
                        models::enums::NodeType::UserFunction => "🔧",
                        models::enums::NodeType::Trigger => "⚡",
                        models::enums::NodeType::Event => "📅",
                        models::enums::NodeType::MySQLFolder => "🐬",
                        models::enums::NodeType::PostgreSQLFolder => "🐘",
                        models::enums::NodeType::SQLiteFolder => "📄",
                        models::enums::NodeType::RedisFolder => "🔴",
                        models::enums::NodeType::MongoDBFolder => "🍃",
                        models::enums::NodeType::MsSQLFolder => "⛁",
                        models::enums::NodeType::CustomFolder => "📁",
                        models::enums::NodeType::QueryFolder => "📂",
                        models::enums::NodeType::HistoryDateFolder => "📅",
                        models::enums::NodeType::ColumnsFolder => "📑",
                        models::enums::NodeType::IndexesFolder => "🧭",
                        models::enums::NodeType::PrimaryKeysFolder => "🔑",
                        models::enums::NodeType::PartitionsFolder => "📊",
                        models::enums::NodeType::Index => "#",
                        _ => "🧾",
                    };

                    let label_text = format!("{} {}", icon, node.name);
                    // Use left-aligned label without forcing a full-row size to avoid centered look.
                    ui.add(
                        egui::Label::new(label_text)
                            .truncate()
                            .sense(egui::Sense::click()),
                    )
                })
                .inner
            };

            if response.clicked() {
                debug!(
                    "🎯 CLICK DETECTED! Node type: {:?}, Name: {}",
                    node.node_type, node.name
                );
                // Handle node selection
                match node.node_type {
                    models::enums::NodeType::Table | models::enums::NodeType::View => {
                        // Don't modify current editor_text, we'll create a new tab
                        // Just trigger table data loading
                        if let Some(conn_id) = node.connection_id {
                            let actual_table_name =
                                node.table_name.as_ref().unwrap_or(&node.name).clone();
                            table_click_request =
                                Some((conn_id, actual_table_name, node.node_type.clone(), node.database_name.clone()));
                        }
                    }
                    // DBA quick views: emit a click request to be handled by parent (needs self)
                    // Unified View Processing (DBA Views + Custom Views)
                    models::enums::NodeType::UsersFolder
                    | models::enums::NodeType::PrivilegesFolder
                    | models::enums::NodeType::ProcessesFolder
                    | models::enums::NodeType::StatusFolder
                    | models::enums::NodeType::BlockedQueriesFolder
                    | models::enums::NodeType::ReplicationStatusFolder
                    | models::enums::NodeType::MasterStatusFolder
                    | models::enums::NodeType::MetricsUserActiveFolder
                    | models::enums::NodeType::CustomView => {
                        debug!("👁️ View clicked: {}", node.name);
                        if let Some(query) = &node.query {
                           // Use the robust execution path
                           if let Some(conn_id) = node.connection_id {
                                custom_view_click_request = Some((conn_id, node.name.clone(), query.clone()));
                           }
                        }
                    }
                    models::enums::NodeType::Query => {
                        // Load query file content
                        debug!("🔍 Query node clicked: {}", node.name);
                        if let Some(file_path) = &node.file_path {
                            debug!("📁 File path: {}", file_path);
                            if let Ok(content) = std::fs::read_to_string(file_path) {
                                debug!(
                                    "✅ File read successfully, content length: {}",
                                    content.len()
                                );
                                // Don't modify editor_text directly, let open_query_file handle it
                                query_file_to_open =
                                    Some((node.name.clone(), content, file_path.clone()));
                            } else {
                                debug!("❌ Failed to read file: {}", file_path);
                                // Handle read error case
                                query_file_to_open = Some((
                                    node.name.clone(),
                                    format!("-- Failed to load query file: {}", node.name),
                                    file_path.clone(),
                                ));
                            }
                        } else {
                            debug!("❌ No file path for query node: {}", node.name);
                            // Handle missing file path case - create a placeholder query
                            let placeholder_content =
                                format!("-- {}\nSELECT * FROM table_name;", node.name);
                            // For files without path, we'll create a new unsaved tab
                            query_file_to_open =
                                Some((node.name.clone(), placeholder_content, String::new()));
                        }
                    }
                    models::enums::NodeType::QueryHistItem => {
                        debug!("🖱️ QueryHistItem clicked: {}", node.name);
                        // For history items, create a new tab with the original query
                        if let Some(data) = &node.file_path {
                            // Parse connection name, timestamp, and query from the stored data
                            // Format: "connection_name||executed_at||original_query"
                            if let Some((_connection_name, rest)) = data.split_once("||") {
                                let (executed_at, original_query) = rest
                                    .split_once("||")
                                    .unwrap_or(("", rest));
                                // Build compact tab title: Hist-YYMMDD HH:MM:SS
                                let tab_title = {
                                    // executed_at is e.g. "2026-03-11 11:45:56" or "2026-03-11T11:45:56"
                                    let ts = executed_at.trim();
                                    let yy = ts.get(2..4).unwrap_or("");
                                    let mm = ts.get(5..7).unwrap_or("");
                                    let dd = ts.get(8..10).unwrap_or("");
                                    let time_part = ts.get(11..19)
                                        .or_else(|| ts.get(11..))
                                        .unwrap_or("");
                                    if !yy.is_empty() && !time_part.is_empty() {
                                        format!("Hist-{}{}{} {}", yy, mm, dd, time_part)
                                    } else {
                                        "Hist".to_string()
                                    }
                                };
                                // Collect to be handled by parent (render_tree) -> will create a NEW TAB
                                debug!(
                                    "📝 Setting query_file_to_open (history): title='{}', query_len={}",
                                    tab_title,
                                    original_query.len()
                                );
                                // Pass the original data in the 3rd field so caller can bind connection
                                query_file_to_open =
                                    Some((tab_title, original_query.to_string(), data.clone()));
                            } else {
                                debug!("📝 Using fallback format for old history item");
                                // Fallback for old format without connection name
                                query_file_to_open = Some((
                                    "Hist".to_string(),
                                    data.clone(),
                                    String::new(),
                                ));
                            }
                        } else {
                            debug!("❌ No file_path data for history item");
                            // Fallback to display name if no original query stored
                        }
                    }
                    models::enums::NodeType::StoredProcedure => {
                        if let Some(conn_id) = node.connection_id {
                            stored_procedure_click_request =
                                Some((conn_id, node.database_name.clone(), node.name.clone()));
                        }
                    }
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
                        ui.close();
                    }

                    if ui.button("Move to Folder").clicked() {
                        if let Some(file_path) = &node.file_path {
                            // Use a different ID range for move operations
                            let move_id = 40000 + (file_path.len() as i64 % 1000);
                            context_menu_request = Some(move_id);
                        }
                        ui.close();
                    }

                    if ui.button("Remove Query").clicked() {
                        if let Some(file_path) = &node.file_path {
                            // Use the file path directly as context identifier
                            // Format: -20000 - simple index to differentiate from connections
                            let remove_id = -20000 - (file_path.len() as i64 % 1000); // Simple deterministic ID
                            context_menu_request = Some(remove_id);
                        }
                        ui.close();
                    }
                });
            }

            // Add context menu for history items
            if node.node_type == models::enums::NodeType::QueryHistItem {
                response.context_menu(|ui| {
                    if ui.button("📋 Copy Query").clicked() {
                        if let Some(data) = &node.file_path {
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                ui.ctx().copy_text(original_query.to_string());
                            } else {
                                ui.ctx().copy_text(data.clone());
                            }
                        }
                        ui.close();
                    }

                    if ui.button("▶️ Execute Query").clicked() {
                        if let Some(data) = &node.file_path {
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                editor.set_text(original_query.to_string());
                            } else {
                                editor.set_text(data.clone());
                            }
                            editor.mark_text_modified();
                            // This will trigger the execution flow when the context menu closes
                        }
                        ui.close();
                    }

                    if ui.button("🔁 Auto Refresh Execute").clicked() {
                        if let Some(data) = &node.file_path {
                            if let Some((_connection_name, original_query)) = data.split_once("||")
                            {
                                editor.set_text(original_query.to_string());
                            } else {
                                editor.set_text(data.clone());
                            }
                            editor.mark_text_modified();
                            // Store query + connection for auto refresh; central UI will read these fields
                            if let Some(conn_id) = node.connection_id {
                                let query_text = editor.text_snapshot();
                                ui.ctx().data_mut(|data| {
                                    data.insert_persisted(
                                        egui::Id::new("auto_refresh_request_conn_id"),
                                        conn_id,
                                    );
                                    data.insert_persisted(
                                        egui::Id::new("auto_refresh_request_query"),
                                        query_text,
                                    );
                                });
                            }
                        }
                        ui.close();
                    }
                });
            }


            // Add context menu for Custom View items
            if node.node_type == models::enums::NodeType::CustomView {
                response.context_menu(|ui| {
                     if ui.button("✏️ Edit this view").clicked() {
                         if let Some(conn_id) = node.connection_id
                             && let Some(query) = &node.query {
                                 edit_custom_view_request = Some((conn_id, node.name.clone(), query.clone()));
                             }
                         ui.close();
                     }
                     if ui.button("🗑️ Delete this dba view").clicked() {
                         if let Some(conn_id) = node.connection_id {
                             delete_custom_view_request = Some((conn_id, node.name.clone()));
                         }
                         ui.close();
                     }
                });
            }

            // Add context menu for Index nodes (non-expandable branch)
            if node.node_type == models::enums::NodeType::Index {
                response.context_menu(|ui| {
                    if ui.button("✏️ Edit Index").clicked() {
                        if let Some(conn_id) = node.connection_id {
                            index_click_request = Some((
                                conn_id,
                                node.name.clone(),
                                node.database_name.clone(),
                                node.table_name.clone(),
                            ));
                        }
                        ui.close();
                    }
                });
            }
        }

        (
            expansion_request,
            table_expansion,
            context_menu_request,
            table_click_request,
            connection_click_request,
            query_file_to_open,
            folder_name_for_removal,
            parent_folder_for_creation,
            folder_removal_mapping,
            dba_click_request,
            index_click_request,
            create_index_request,
            alter_table_request,
            request_add_replication_dialog,
            drop_collection_request,
            drop_table_request,
            create_table_request,
            stored_procedure_click_request,
            generate_ddl_request,
            open_diagram_request,
            request_add_view_dialog,
            custom_view_click_request,
            delete_custom_view_request,
            edit_custom_view_request,
        )
    }


    pub fn sanitize_display_table_name(display: &str) -> String {
        // Remove leading known emoji + whitespace
        let mut s = display.trim_start();
        for prefix in ["📋", "📁", "🔧", "🗄", "•", "#", "📑"] {
            // extend as needed
            if s.starts_with(prefix) {
                s = s[prefix.len()..].trim_start();
            }
        }
        // Truncate at first " (" which denotes annotations like "(table name match)" or column counts
        if let Some(pos) = s.find(" (") {
            s[..pos].trim().to_string()
        } else {
            s.to_string()
        }
    }



    pub fn render_tree_for_database_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(-2.0);
        // Add responsive search box
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let search_bg = if ui.visuals().dark_mode {
                egui::Color32::from_rgb(40, 40, 40)
            } else {
                egui::Color32::from_rgb(210, 210, 210)
            };
            // Make search box responsive to sidebar width
            let available_width = ui.available_width() - 5.0; // Leave space for clear button and padding
            let search_response = ui.add_sized(
                [available_width, 20.0],
                egui::TextEdit::singleline(&mut self.database_search_text)
                    .hint_text("Search Databases, Table, etc...")
                    .background_color(search_bg),
            );
            if search_response.has_focus() {
                ui.painter().rect_stroke(
                    search_response.rect,
                    0.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 0, 0)),
                    egui::StrokeKind::Outside,
                );
            }

            // // Make search box responsive to sidebar width
            // let available_width = ui.available_width() - 5.0; // Leave space for clear button and padding
            // let search_response = ui.add_sized(
            //     [available_width, 20.0],
            //     egui::TextEdit::singleline(&mut self.database_search_text)
            //         .hint_text("Search databases, tables, keys..."),
            // );

            if search_response.changed() {
                self.update_search_results();
            }
        });

        // Use search results if search is active, otherwise use normal tree
        if self.show_search_results && !self.database_search_text.trim().is_empty() {
            // Show search results
            let mut filtered_tree = std::mem::take(&mut self.filtered_items_tree);
            let _ = self.render_tree(ui, &mut filtered_tree, true);
            self.filtered_items_tree = filtered_tree;
        } else {
            // Show normal tree
            // Use slice to avoid borrowing issues
            let mut items_tree = std::mem::take(&mut self.items_tree);

            let query_files_to_open = self.render_tree(ui, &mut items_tree, false);
            
            for (filename, content, file_path, context_connection_id) in query_files_to_open {
                 if file_path.is_empty() {
                     // Custom View or similar: Use the context connection ID if available
                     let _ = crate::editor::create_new_tab_with_connection_and_database(
                        self,
                        filename,
                        content,
                        context_connection_id,
                        None // Database name is usually baked into the query or will be selected
                     );
                     
                     // Auto-execute if it's a Custom View (implied by having a connection ID context)
                     if context_connection_id.is_some()
                        && let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                            tab.should_run_on_open = true;
                        }
                 } else if let Err(err) = crate::sidebar_query::open_query_file(self, &file_path) {
                     log::error!("Failed to open query file: {}", err);
                 }
            }


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
}
