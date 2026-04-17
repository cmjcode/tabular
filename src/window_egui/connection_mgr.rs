use log::debug;
use crate::{models, cache_data, data_table, editor, directory, sidebar_query};

impl super::Tabular {
    pub fn handle_alter_table_request(
        &mut self,
        connection_id: i64,
        database_name: Option<String>,
        table_name: String,
    ) {
        debug!(
            "🔍 handle_alter_table_request called with connection_id: {}, table: {}",
            connection_id, table_name
        );

        let connection = match self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .cloned()
        {
            Some(conn) => conn,
            None => {
                debug!("❌ Connection with ID {} not found", connection_id);
                return;
            }
        };

        let resolved_db = database_name
            .filter(|db| !db.trim().is_empty())
            .or_else(|| {
                let default_db = connection.database.trim();
                if default_db.is_empty() {
                    None
                } else {
                    Some(connection.database.clone())
                }
            });

        let table_title = format!("Table: {}", table_name);
        let matches_target = |tab: &models::structs::QueryTab| {
            tab.title == table_title
                && tab.connection_id == Some(connection_id)
                && match (&resolved_db, &tab.database_name) {
                    (Some(expected), Some(existing)) => expected == existing,
                    (Some(_), None) => false,
                    _ => true,
                }
        };

        if let Some((existing_index, _)) = self
            .query_tabs
            .iter()
            .enumerate()
            .find(|(_, tab)| matches_target(tab))
        {
            if existing_index != self.active_tab_index {
                editor::switch_to_tab(self, existing_index);
            }
        } else {
            editor::create_new_tab_with_connection_and_database(
                self,
                table_title.clone(),
                String::new(),
                Some(connection_id),
                resolved_db.clone(),
            );
        }

        self.current_connection_id = Some(connection_id);
        self.is_table_browse_mode = true;

        let caption = resolved_db
            .as_ref()
            .map(|db| format!("Table: {} (Database: {})", table_name, db))
            .unwrap_or_else(|| format!("Table: {}", table_name));

        self.current_table_name = caption.clone();
        self.table_bottom_view = models::structs::TableBottomView::Structure;
        self.structure_sub_view = models::structs::StructureSubView::Columns;
        self.last_structure_target = None;
        self.request_structure_refresh = false;

        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.title = table_title;
            active_tab.connection_id = Some(connection_id);
            if resolved_db.is_some() {
                active_tab.database_name = resolved_db.clone();
            }
            active_tab.is_table_browse_mode = true;
            active_tab.result_table_name = caption.clone();
        }

        data_table::load_structure_info_for_current_table(self);
    }
    pub fn handle_create_folder_in_folder_request(&mut self, _hash: i64) {
        debug!(
            "🔍 handle_create_folder_in_folder_request called with hash: {}",
            _hash
        );
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
    pub fn handle_remove_folder_request(&mut self, hash: i64) {
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
                            self.error_message = format!(
                                "Failed to remove folder '{}': {}",
                                folder_relative_path, e
                            );
                            self.show_error_message = true;
                        }
                    }
                } else {
                    // Offer option to remove folder and all contents
                    self.error_message = format!(
                        "Folder '{}' is not empty.\n\nWould you like to remove it and all its contents?",
                        folder_relative_path
                    );
                    self.show_error_message = true;
                    debug!(
                        "❌ Cannot remove non-empty folder: {}",
                        folder_relative_path
                    );
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
                                self.error_message = format!(
                                    "Failed to remove folder '{}': {}",
                                    folder_relative_path, e
                                );
                                self.show_error_message = true;
                            }
                        }
                    } else {
                        self.error_message = format!(
                            "Folder '{}' is not empty.\n\nWould you like to remove it and all its contents?",
                            folder_relative_path
                        );
                        self.show_error_message = true;
                        debug!(
                            "❌ Cannot remove non-empty folder: {}",
                            folder_relative_path
                        );
                    }
                } else {
                    self.error_message =
                        format!("Folder '{}' does not exist", folder_relative_path);
                    self.show_error_message = true;
                    debug!("❌ Folder does not exist: {}", folder_relative_path);
                }

                self.selected_folder_for_removal = None;
            } else {
                debug!("❌ No folder selected for removal in fallback either");
            }
        }
    }
    pub fn is_directory_empty(dir_path: &std::path::Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(dir_path) {
            entries.count() == 0
        } else {
            false
        }
    }
    pub fn find_connection_node_recursive(
        nodes: &mut [models::structs::TreeNode],
        connection_id: i64,
    ) -> Option<&mut models::structs::TreeNode> {
        for node in nodes.iter_mut() {
            // Check if this is the connection node we're looking for
            if node.node_type == models::enums::NodeType::Connection
                && node.connection_id == Some(connection_id)
            {
                return Some(node);
            }

            // Recursively search in children
            if !node.children.is_empty()
                && let Some(found) =
                    Self::find_connection_node_recursive(&mut node.children, connection_id)
            {
                return Some(found);
            }
        }
        None
    }
    pub fn refresh_connection(&mut self, connection_id: i64) {
        // Clear all cached data for this connection (SQLite tables)
        self.clear_connection_cache(connection_id);

        // Also clear in-memory database cache so next load always hits the server
        self.database_cache.remove(&connection_id);
        self.database_cache_time.remove(&connection_id);

        // Remove from connection pool cache to force reconnection
        self.connection_pools.remove(&connection_id);
        // Also remove from shared pools
        if let Ok(mut shared) = self.shared_connection_pools.lock() {
            shared.remove(&connection_id);
        }

        // Mark as refreshing
        self.refreshing_connections.insert(connection_id);

        // Find the connection node in the tree (recursively) and reset its loaded state
        if let Some(conn_node) =
            Self::find_connection_node_recursive(&mut self.items_tree, connection_id)
        {
            conn_node.is_loaded = false;
            // Keep current expansion state so it doesn't visually disappear; we'll repopulate on next expand
            let was_expanded = conn_node.is_expanded;
            conn_node.children.clear();
            conn_node.is_expanded = was_expanded; // preserve state
            debug!(
                "🔄 Reset (cached cleared) connection node: {} (expanded: {})",
                conn_node.name, was_expanded
            );
        } else {
            debug!(
                "⚠️ Could not locate connection node {} in primary tree; trying filtered tree / rebuild",
                connection_id
            );
            // Try filtered tree (search results)
            if let Some(conn_node) =
                Self::find_connection_node_recursive(&mut self.filtered_items_tree, connection_id)
            {
                let was_expanded = conn_node.is_expanded;
                conn_node.children.clear();
                conn_node.is_loaded = false;
                conn_node.is_expanded = was_expanded;
                debug!(
                    "🔄 Reset connection node in filtered tree: {} (expanded: {})",
                    conn_node.name, was_expanded
                );
            } else {
                // As a last resort rebuild the whole tree then search again
                crate::sidebar_database::refresh_connections_tree(self);
                if let Some(conn_node2) =
                    Self::find_connection_node_recursive(&mut self.items_tree, connection_id)
                {
                    let was_expanded = conn_node2.is_expanded;
                    conn_node2.children.clear();
                    conn_node2.is_loaded = false;
                    conn_node2.is_expanded = was_expanded;
                    debug!(
                        "🔄 Reset connection node after rebuild: {} (expanded: {})",
                        conn_node2.name, was_expanded
                    );
                } else {
                    debug!(
                        "❌ Still could not locate connection node {} after rebuild. Existing connection IDs: {:?}",
                        connection_id,
                        self.connections
                            .iter()
                            .filter_map(|c| c.id)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }

        // Send background task instead of blocking refresh
        if let Some(sender) = &self.background_sender {
            if let Err(e) =
                sender.send(models::enums::BackgroundTask::RefreshConnection { connection_id })
            {
                debug!("Failed to send background refresh task: {}", e);
                // Fallback to synchronous refresh if background thread is not available
                self.refreshing_connections.remove(&connection_id);
                cache_data::fetch_and_cache_connection_data(self, connection_id);
            } else {
                debug!(
                    "Background refresh task sent for connection {}",
                    connection_id
                );
            }
        } else {
            // Fallback to synchronous refresh if background system is not initialized
            self.refreshing_connections.remove(&connection_id);
            cache_data::fetch_and_cache_connection_data(self, connection_id);
        }
    }

    pub fn restore_expansion_state(
        node: &mut models::structs::TreeNode,
        state_map: &std::collections::HashMap<String, bool>,
    ) {

        // Create unique key for this node
        let node_type_str = format!("{:?}", node.node_type);
        let key = format!(
            "{}:{}:{}:{}",
            node.connection_id.unwrap_or(0),
            node.database_name.as_ref().unwrap_or(&String::new()),
            node_type_str,
            node.name
        );

        // Restore expansion state from saved map
        if let Some(&expanded) = state_map.get(&key) {
            node.is_expanded = expanded;
            if expanded {
                debug!(
                    "   📂 Restoring expanded: {:?} - {}",
                    node.node_type, node.name
                );
            }
        }

        // Force expand important container folders if they were expanded before
        // This ensures Database and TablesFolder are visible after refresh
        match node.node_type {
            models::enums::NodeType::Connection => {
                // If connection was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            models::enums::NodeType::DatabasesFolder => {
                // If DatabasesFolder was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            models::enums::NodeType::Database => {
                // If Database was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            models::enums::NodeType::TablesFolder => {
                // If TablesFolder was expanded, keep it expanded
                if state_map.get(&key).copied().unwrap_or(false) {
                    node.is_expanded = true;
                }
            }
            _ => {}
        }

        // Recursively restore children
        for child in &mut node.children {
            Self::restore_expansion_state(child, state_map);
        }
    }

    pub fn mark_expanded_nodes_loaded(node: &mut models::structs::TreeNode) {
        use log::debug;

        // If this node is expanded, mark it as not loaded so it will reload from cache
        if node.is_expanded {
            match node.node_type {
                models::enums::NodeType::Database
                | models::enums::NodeType::TablesFolder
                | models::enums::NodeType::ViewsFolder
                | models::enums::NodeType::StoredProceduresFolder
                | models::enums::NodeType::UserFunctionsFolder
                | models::enums::NodeType::TriggersFolder
                | models::enums::NodeType::EventsFolder => {
                    // Mark as not loaded so it will trigger loading from cache on next render
                    node.is_loaded = false;
                    debug!(
                        "   📂 Marked expanded {:?} as needing reload: {}",
                        node.node_type, node.name
                    );
                }
                _ => {}
            }
        }

        // Recursively process children
        for child in &mut node.children {
            Self::mark_expanded_nodes_loaded(child);
        }
    }

    pub fn load_expanded_nodes_recursive(
        &mut self,
        connection_id: i64,
        node: &mut models::structs::TreeNode,
    ) {

        debug!(
            "🔍 Checking node: {:?} '{}' - expanded={}, loaded={}",
            node.node_type, node.name, node.is_expanded, node.is_loaded
        );

        // If this node is expanded and not loaded, load it
        if node.is_expanded && !node.is_loaded {
            match node.node_type {
                models::enums::NodeType::Connection => {
                    debug!("   📂 Loading Connection node from cache");
                    self.load_connection_tables(connection_id, node);
                }
                models::enums::NodeType::DatabasesFolder => {
                    debug!("   📂 Loading DatabasesFolder from cache");
                    self.load_databases_for_folder(connection_id, node);
                }
                models::enums::NodeType::Database => {
                    debug!("   📂 Loading Database node from cache: {}", node.name);
                    // Database node contains folders (Tables, Views, etc), they'll be loaded by their children
                    node.is_loaded = true;
                }
                models::enums::NodeType::TablesFolder => {
                    debug!("   📂 Loading TablesFolder from cache");
                    self.load_folder_content(
                        connection_id,
                        node,
                        models::enums::NodeType::TablesFolder,
                        false,
                    );
                }
                models::enums::NodeType::ViewsFolder => {
                    debug!("   📂 Loading ViewsFolder from cache");
                    self.load_folder_content(
                        connection_id,
                        node,
                        models::enums::NodeType::ViewsFolder,
                        false,
                    );
                }
                models::enums::NodeType::StoredProceduresFolder => {
                    debug!("   📂 Loading StoredProceduresFolder from cache");
                    self.load_folder_content(
                        connection_id,
                        node,
                        models::enums::NodeType::StoredProceduresFolder,
                        false,
                    );
                }
                _ => {
                    debug!("   ⏭️  Skipping {:?} node (no loader)", node.node_type);
                }
            }
        } else if node.is_expanded {
            debug!("   ⏭️  Node already loaded, skipping");
        }

        // Recursively process children (depth-first)
        // Clone children vec to avoid borrow issues
        let children_count = node.children.len();
        debug!("   👶 Processing {} children...", children_count);
        for i in 0..children_count {
            // Process each child
            if let Some(child) = node.children.get_mut(i) {
                Self::load_expanded_nodes_recursive(self, connection_id, child);
            }
        }
    }

    pub fn disconnect_connection(&mut self, connection_id: i64) {
        debug!("🔌 Disconnecting connection: {}", connection_id);

        // 1. Remove from local connection pool cache
        if self.connection_pools.remove(&connection_id).is_some() {
            debug!("✅ Removed connection pool from local cache");
        }

        // 2. Remove from shared connection pools
        if let Ok(mut shared_pools) = self.shared_connection_pools.lock()
            && shared_pools.remove(&connection_id).is_some()
        {
            debug!("✅ Removed connection pool from shared cache");
        }

        // 3. Remove from pending pools (if connection was being created)
        if self.pending_connection_pools.remove(&connection_id) {
            debug!("✅ Removed from pending connection pools");
        }

        // 4. Stop any prefetch in progress
        if self.prefetch_in_progress.remove(&connection_id) {
            debug!("✅ Stopped prefetch for connection");
        }
        self.prefetch_progress.remove(&connection_id);

        // 5. Remove from refreshing set
        if self.refreshing_connections.remove(&connection_id) {
            debug!("✅ Removed from refreshing connections");
        }

        // 6. Clear database cache for this connection
        self.database_cache.remove(&connection_id);
        self.database_cache_time.remove(&connection_id);

        // 7. Clear connection cache (database/table/column metadata)
        self.clear_connection_cache(connection_id);

        // 8. Reset connection node state in tree
        if let Some(conn_node) =
            Self::find_connection_node_recursive(&mut self.items_tree, connection_id)
        {
            conn_node.is_loaded = false;
            conn_node.is_expanded = false; // Collapse node
            conn_node.children.clear();
            debug!(
                "✅ Reset connection node: {} (collapsed and cleared)",
                conn_node.name
            );
        }

        // Also check filtered tree
        if let Some(conn_node) =
            Self::find_connection_node_recursive(&mut self.filtered_items_tree, connection_id)
        {
            conn_node.is_loaded = false;
            conn_node.is_expanded = false;
            conn_node.children.clear();
        }

        debug!("✅ Connection {} disconnected successfully", connection_id);
    }

    pub fn clear_connection_cache(&self, connection_id: i64) {
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let rt = tokio::runtime::Runtime::new().unwrap();

            rt.block_on(async {
                log::debug!(
                    "[clear_connection_cache] clearing sqlite cache for connection {}",
                    connection_id
                );

                // Run a quick integrity check first; if the cache db is corrupted, skip
                // clearing (the corrupted file will be replaced on next startup) and
                // just proceed — the live fetch path will still work.
                let is_ok = sqlx::query_as::<_, (String,)>("PRAGMA integrity_check")
                    .fetch_one(pool_clone.as_ref())
                    .await
                    .map_or(false, |(s,)| s == "ok");

                if !is_ok {
                    log::warn!(
                        "[clear_connection_cache] cache db integrity check failed for connection {} — skipping cache clear (will be recovered on restart)",
                        connection_id
                    );
                    return;
                }

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

                // Also clear row and index caches to avoid stale data after refresh
                let _ = sqlx::query("DELETE FROM row_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;

                let _ = sqlx::query("DELETE FROM index_cache WHERE connection_id = ?")
                    .bind(connection_id)
                    .execute(pool_clone.as_ref())
                    .await;

                log::debug!(
                    "[clear_connection_cache] finished clearing sqlite cache for connection {}",
                    connection_id
                );
            });
        }
    }

    // Clear cache for a specific table only
    pub(crate) fn clear_table_cache(
        &self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) {

        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let db = database_name.to_string();
            let tbl = table_name.to_string();
            let rt = tokio::runtime::Runtime::new().unwrap();

            rt.block_on(async {
                debug!("🧹 Clearing cache for table {}.{}", db, tbl);

                // Clear table cache entry
                let _ = sqlx::query("DELETE FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                // Clear column cache for this table
                let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                // Clear row cache for this table
                let _ = sqlx::query("DELETE FROM row_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                // Clear index cache for this table
                let _ = sqlx::query("DELETE FROM index_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                    .bind(connection_id)
                    .bind(&db)
                    .bind(&tbl)
                    .execute(pool_clone.as_ref())
                    .await;

                debug!("✅ Cache cleared for table {}.{}", db, tbl);
            });
        }
    }

    // Remove a specific table from the sidebar tree without reloading entire connection
    pub(crate) fn remove_table_from_tree(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) {
        use log::{debug};

        debug!(
            "🌲 Removing table {}.{} from sidebar tree",
            database_name, table_name
        );
        debug!("   Connection ID: {}", connection_id);
        debug!("   Database name: '{}'", database_name);
        debug!("   Table name: '{}'", table_name);

        // Debug: print tree structure
        debug!("   Current tree structure:");
        for (i, conn_node) in self.items_tree.iter().enumerate() {
            debug!(
                "     [{}] Connection: {} (id={:?}, type={:?})",
                i, conn_node.name, conn_node.connection_id, conn_node.node_type
            );
            for (j, child) in conn_node.children.iter().enumerate() {
                debug!(
                    "       [{}] Child: {} (type={:?}, db={:?})",
                    j, child.name, child.node_type, child.database_name
                );
            }
        }

        // Helper to match table names - handles [schema].[table], schema.table, or just table
        let matches_table = |node_name: &str, search_name: &str| -> bool {
            // Direct match
            if node_name == search_name {
                return true;
            }

            // Remove brackets and compare
            let clean_node = node_name.replace("[", "").replace("]", "");
            let clean_search = search_name.replace("[", "").replace("]", "");

            if clean_node == clean_search {
                return true;
            }

            // Compare just the table part (after last dot)
            let node_table = clean_node.split('.').next_back().unwrap_or(&clean_node);
            let search_table = clean_search.split('.').next_back().unwrap_or(&clean_search);

            node_table == search_table
        };

        // Find the connection node (may be inside a CustomFolder)
        for folder_or_conn in &mut self.items_tree {
            // First check if this is a CustomFolder, if so search its children for the connection
            if folder_or_conn.node_type == models::enums::NodeType::CustomFolder {
                debug!("   Searching in folder: {}", folder_or_conn.name);
                for conn_node in &mut folder_or_conn.children {
                    if conn_node.connection_id == Some(connection_id) {
                        debug!(
                            "   ✓ Found connection node: {} (ID: {})",
                            conn_node.name, connection_id
                        );

                        // Navigate through the tree structure to find the table
                        // Structure: Connection -> Databases Folder -> Database -> Tables Folder -> Table
                        if Self::remove_table_from_connection_node(
                            conn_node,
                            database_name,
                            table_name,
                            &matches_table,
                        ) {
                            return;
                        }
                    }
                }
            }
            // Also check if this node itself is a connection (for backward compatibility with non-folder structure)
            else if folder_or_conn.connection_id == Some(connection_id) {
                debug!(
                    "   ✓ Found connection node (direct): {}",
                    folder_or_conn.name
                );

                // Navigate through the tree structure to find the table
                if Self::remove_table_from_connection_node(
                    folder_or_conn,
                    database_name,
                    table_name,
                    &matches_table,
                ) {
                    return;
                }
            }
        }

        debug!("   ⚠️ Connection {} not found in tree", connection_id);
        debug!(
            "   ⚠️ Table '{}' not found in tree (may have been already removed)",
            table_name
        );
    }

}
