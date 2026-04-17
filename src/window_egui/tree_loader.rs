use log::debug;
use super::Tabular;
use crate::{models, connection, cache_data, driver_mysql, driver_postgres, driver_sqlite, driver_redis};

impl super::Tabular {
    pub fn remove_table_from_connection_node(
        conn_node: &mut models::structs::TreeNode,
        database_name: &str,
        table_name: &str,
        matches_table: &dyn Fn(&str, &str) -> bool,
    ) -> bool {

        // Navigate through the tree structure to find the table
        // Structure: Connection -> Databases Folder -> Database -> Tables Folder -> Table
        for child in &mut conn_node.children {
            // Look for Databases folder
            if child.node_type == models::enums::NodeType::DatabasesFolder {
                debug!("   Found DatabasesFolder");
                for db_node in &mut child.children {
                    // Find matching database
                    if let Some(ref db_name) = db_node.database_name {
                        debug!("   Checking database: {}", db_name);
                        if db_name == database_name {
                            debug!("   ✓ Database matches!");
                            // Find Tables folder in this database
                            for folder in &mut db_node.children {
                                if folder.node_type == models::enums::NodeType::TablesFolder {
                                    debug!(
                                        "   Found TablesFolder with {} tables",
                                        folder.children.len()
                                    );

                                    // Log all tables before removal
                                    for table_node in &folder.children {
                                        let tbl_name = table_node
                                            .table_name
                                            .as_ref()
                                            .unwrap_or(&table_node.name);
                                        debug!(
                                            "      - Table in tree: '{}' (node.name='{}', node.table_name={:?})",
                                            tbl_name, table_node.name, table_node.table_name
                                        );
                                    }

                                    // Remove the table from Tables folder
                                    let before_count = folder.children.len();
                                    folder.children.retain(|table_node| {
                                        let node_name = table_node.table_name.as_ref().unwrap_or(&table_node.name);
                                        let keep = !matches_table(node_name, table_name);
                                        if !keep {
                                            debug!("   ✅ Removed table '{}' from tree (matched with '{}')", node_name, table_name);
                                        }
                                        keep
                                    });
                                    let after_count = folder.children.len();
                                    debug!("   Tables count: {} -> {}", before_count, after_count);
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
            // Also check direct children for databases (some DB types don't use DatabasesFolder)
            else if child.node_type == models::enums::NodeType::Database
                && let Some(ref db_name) = child.database_name
            {
                debug!("   Checking direct database node: {}", db_name);
                if db_name == database_name {
                    debug!("   ✓ Database matches!");
                    // Find Tables folder in this database
                    for folder in &mut child.children {
                        if folder.node_type == models::enums::NodeType::TablesFolder {
                            debug!(
                                "   Found TablesFolder with {} tables",
                                folder.children.len()
                            );

                            // Log all tables before removal
                            for table_node in &folder.children {
                                let tbl_name =
                                    table_node.table_name.as_ref().unwrap_or(&table_node.name);
                                debug!(
                                    "      - Table in tree: '{}' (node.name='{}', node.table_name={:?})",
                                    tbl_name, table_node.name, table_node.table_name
                                );
                            }

                            // Remove the table from Tables folder
                            let before_count = folder.children.len();
                            folder.children.retain(|table_node| {
                                let node_name =
                                    table_node.table_name.as_ref().unwrap_or(&table_node.name);
                                let keep = !matches_table(node_name, table_name);
                                if !keep {
                                    debug!(
                                        "   ✅ Removed table '{}' from tree (matched with '{}')",
                                        node_name, table_name
                                    );
                                }
                                keep
                            });
                            let after_count = folder.children.len();
                            debug!("   Tables count: {} -> {}", before_count, after_count);
                            return true;
                        }
                    }
                }
            }
        }

        false // Table not found
    }
    pub fn load_connection_tables(&mut self, connection_id: i64, node: &mut models::structs::TreeNode) {
        debug!("Loading connection tables for ID: {}", connection_id);

        // Ensure a connection pool is opened/initialized before proceeding.
        if !self.connection_pools.contains_key(&connection_id) {
            let rt = self.get_runtime();
            let start_time = std::time::Instant::now();
            let pool_res = rt.block_on(async {
                crate::connection::get_or_create_connection_pool(self, connection_id).await
            });
            match pool_res {
                Some(_) => debug!(
                    "✅ Connection pool ready for {} (took {:?})",
                    connection_id,
                    start_time.elapsed()
                ),
                None => debug!(
                    "❌ Failed to initialize connection pool for {}",
                    connection_id
                ),
            }
        } else {
            debug!("🔁 Reusing existing connection pool for {}", connection_id);
        }

        // Always try to fetch from the actual database server first.
        // This ensures that after a Refresh the UI always reflects live server data
        // (avoiding stale/incomplete cached data, e.g. missing databases).
        // Cache is only used as a fallback when the live fetch fails.
        let (fresh_databases_opt, is_replica) = {
            let rt = self.get_runtime();
            rt.block_on(async {
                let dbs = crate::connection::fetch_databases_from_connection_async(self, connection_id).await;
                
                // Check replication status for MySQL
                let mut is_replica = false;
                if let Some(conn) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                    && conn.connection_type == models::enums::DatabaseType::MySQL
                    && let Some(models::enums::DatabasePool::MySQL(mysql_pool)) =
                        crate::connection::get_or_create_connection_pool(self, connection_id).await
                {
                    is_replica = driver_mysql::check_replication_status(&mysql_pool).await;
                }
                (dbs, is_replica)
            })
        };
        if let Some(fresh_databases) = fresh_databases_opt {
            debug!(
                "✅ Successfully fetched {} databases from server",
                fresh_databases.len()
            );
            eprintln!("[DB-FETCH] load_connection_tables: live fetch returned {} databases for conn {}: {:?}", fresh_databases.len(), connection_id, fresh_databases);
            // Save to cache for future use
            cache_data::save_databases_to_cache(self, connection_id, &fresh_databases);
            // Also update in-memory cache
            self.database_cache.insert(connection_id, fresh_databases.clone());
            self.database_cache_time.insert(connection_id, std::time::Instant::now());
            // Build structure from fresh data
            self.build_connection_structure_from_cache(connection_id, node, &fresh_databases, is_replica);
            node.is_loaded = true;
            return;
        } else {
            debug!("❌ Failed to fetch databases from server, falling back to cache");
            eprintln!("[DB-FETCH] load_connection_tables: live fetch failed/empty, falling back to cache for conn {}", connection_id);
            // Fallback: use cached data if available
            let dbs = self.get_databases_cached(connection_id);
            if !dbs.is_empty() {
                debug!("Found cached databases for connection {}: {:?}", connection_id, dbs);
                self.build_connection_structure_from_cache(connection_id, node, &dbs, false);
                node.is_loaded = true;
                return;
            }
        }

        // Find the connection by ID
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();

            // Create the main structure based on database type
            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    driver_mysql::load_mysql_structure(connection_id, &connection, node, is_replica);
                }
                models::enums::DatabaseType::PostgreSQL => {
                    driver_postgres::load_postgresql_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::SQLite => {
                    driver_sqlite::load_sqlite_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::Redis => {
                    driver_redis::load_redis_structure(self, connection_id, &connection, node);
                }
                models::enums::DatabaseType::MsSQL => {
                    crate::driver_mssql::load_mssql_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::MongoDB => {
                    crate::driver_mongodb::load_mongodb_structure(connection_id, &connection, node);
                }
                models::enums::DatabaseType::ApiHttp => {}
            }
            node.is_loaded = true;
        }
    }
    pub fn build_connection_structure_from_cache(
        &mut self,
        connection_id: i64,
        node: &mut models::structs::TreeNode,
        databases: &[String],
        is_replica: bool,
    ) {
        // Find the connection to get its type
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let mut main_children = Vec::new();

            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    // 1. Databases folder
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);
                    databases_folder.is_loaded = false;

                    // Add each database from cache
                    for db_name in databases {
                        // Skip system databases for cleaner view
                        if !["information_schema", "performance_schema", "mysql", "sys"]
                            .contains(&db_name.as_str())
                        {
                            let mut db_node = models::structs::TreeNode::new(
                                db_name.clone(),
                                models::enums::NodeType::Database,
                            );
                            db_node.connection_id = Some(connection_id);
                            db_node.database_name = Some(db_name.clone());
                            db_node.is_loaded = false; // Will be loaded when expanded

                            // Create folder structure but don't load content yet
                            let mut tables_folder = models::structs::TreeNode::new(
                                "Tables".to_string(),
                                models::enums::NodeType::TablesFolder,
                            );
                            tables_folder.connection_id = Some(connection_id);
                            tables_folder.database_name = Some(db_name.clone());
                            tables_folder.is_loaded = false;

                            let mut views_folder = models::structs::TreeNode::new(
                                "Views".to_string(),
                                models::enums::NodeType::ViewsFolder,
                            );
                            views_folder.connection_id = Some(connection_id);
                            views_folder.database_name = Some(db_name.clone());
                            views_folder.is_loaded = false;

                            let mut procedures_folder = models::structs::TreeNode::new(
                                "Stored Procedures".to_string(),
                                models::enums::NodeType::StoredProceduresFolder,
                            );
                            procedures_folder.connection_id = Some(connection_id);
                            procedures_folder.database_name = Some(db_name.clone());
                            procedures_folder.is_loaded = false;

                            let mut functions_folder = models::structs::TreeNode::new(
                                "Functions".to_string(),
                                models::enums::NodeType::UserFunctionsFolder,
                            );
                            functions_folder.connection_id = Some(connection_id);
                            functions_folder.database_name = Some(db_name.clone());
                            functions_folder.is_loaded = false;

                            let mut triggers_folder = models::structs::TreeNode::new(
                                "Triggers".to_string(),
                                models::enums::NodeType::TriggersFolder,
                            );
                            triggers_folder.connection_id = Some(connection_id);
                            triggers_folder.database_name = Some(db_name.clone());
                            triggers_folder.is_loaded = false;

                            let mut events_folder = models::structs::TreeNode::new(
                                "Events".to_string(),
                                models::enums::NodeType::EventsFolder,
                            );
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


                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::MySQL) {
                        let mut node = models::structs::TreeNode::new(name.to_string(), node_type);
                        node.connection_id = Some(connection_id);
                        node.is_loaded = false;
                        node.query = Some(query.to_string());
                        dba_children.push(node);
                    }

                    // Render Custom Views
                    log::debug!("Cache Builder: Rendering custom views for connection {}: found {}", connection_id, connection.custom_views.len());
                    for view in connection.custom_views.iter() {
                        let mut view_node = models::structs::TreeNode::new(
                            view.name.clone(),
                            models::enums::NodeType::CustomView,
                        );
                        view_node.connection_id = Some(connection_id);
                        view_node.query = Some(view.query.clone()); 
                        view_node.is_loaded = true;
                        dba_children.push(view_node);
                    }

                    dba_folder.children = dba_children;

                    main_children.push(databases_folder);
                    main_children.push(dba_folder);

                    if is_replica {
                        let mut replication_folder = models::structs::TreeNode::new(
                            "Replication".to_string(),
                            models::enums::NodeType::ReplicationStatusFolder,
                        );
                        replication_folder.connection_id = Some(connection_id);
                        replication_folder.is_loaded = true;
                        
                        let mut status_node = models::structs::TreeNode::new(
                            "Status".to_string(),
                            models::enums::NodeType::ReplicationStatusFolder,
                        );
                        status_node.connection_id = Some(connection_id);
                        status_node.is_loaded = false;
                        
                        main_children.push(replication_folder);
                    }
                    
                    node.children = main_children;
                    return;
                }
                models::enums::DatabaseType::PostgreSQL => {
                    // Similar structure for PostgreSQL
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);

                    for db_name in databases {
                        if !["template0", "template1", "postgres"].contains(&db_name.as_str()) {
                            let mut db_node = models::structs::TreeNode::new(
                                db_name.clone(),
                                models::enums::NodeType::Database,
                            );
                            db_node.connection_id = Some(connection_id);
                            db_node.database_name = Some(db_name.clone());
                            db_node.is_loaded = false;

                            let mut tables_folder = models::structs::TreeNode::new(
                                "Tables".to_string(),
                                models::enums::NodeType::TablesFolder,
                            );
                            tables_folder.connection_id = Some(connection_id);
                            tables_folder.database_name = Some(db_name.clone());
                            tables_folder.is_loaded = false;

                            let mut views_folder = models::structs::TreeNode::new(
                                "Views".to_string(),
                                models::enums::NodeType::ViewsFolder,
                            );
                            views_folder.connection_id = Some(connection_id);
                            views_folder.database_name = Some(db_name.clone());
                            views_folder.is_loaded = false;

                            db_node.children = vec![tables_folder, views_folder];
                            databases_folder.children.push(db_node);
                        }
                    }

                    main_children.push(databases_folder);

                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::PostgreSQL) {
                         let mut node = models::structs::TreeNode::new(name.to_string(), node_type);
                         node.connection_id = Some(connection_id);
                         node.is_loaded = false;
                         node.query = Some(query.to_string());
                         dba_children.push(node);
                    }

                    // Render Custom Views
                    log::debug!("Cache Builder: Rendering custom views for connection {}: found {}", connection_id, connection.custom_views.len());
                    for view in connection.custom_views.iter() {
                        let mut view_node = models::structs::TreeNode::new(
                            view.name.clone(),
                            models::enums::NodeType::CustomView,
                        );
                        view_node.connection_id = Some(connection_id);
                        view_node.query = Some(view.query.clone()); 
                        view_node.is_loaded = true;
                        dba_children.push(view_node);
                    }

                    dba_folder.children = dba_children;
                    main_children.push(dba_folder);
                }
                models::enums::DatabaseType::MongoDB => {
                    // MongoDB: Databases -> Collections
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);

                    for db_name in databases {
                        let mut db_node = models::structs::TreeNode::new(
                            db_name.clone(),
                            models::enums::NodeType::Database,
                        );
                        db_node.connection_id = Some(connection_id);
                        db_node.database_name = Some(db_name.clone());
                        db_node.is_loaded = false;

                        // Collections folder (reuse TablesFolder type for UI rendering)
                        let mut collections_folder = models::structs::TreeNode::new(
                            "Collections".to_string(),
                            models::enums::NodeType::TablesFolder,
                        );
                        collections_folder.connection_id = Some(connection_id);
                        collections_folder.database_name = Some(db_name.clone());
                        collections_folder.is_loaded = false;
                        db_node.children = vec![collections_folder];
                        databases_folder.children.push(db_node);
                    }

                    main_children.push(databases_folder);
                }
                models::enums::DatabaseType::SQLite => {
                    // SQLite structure - single database
                    let mut tables_folder = models::structs::TreeNode::new(
                        "Tables".to_string(),
                        models::enums::NodeType::TablesFolder,
                    );
                    tables_folder.connection_id = Some(connection_id);
                    tables_folder.database_name = Some("main".to_string());
                    tables_folder.is_loaded = false;

                    let mut views_folder = models::structs::TreeNode::new(
                        "Views".to_string(),
                        models::enums::NodeType::ViewsFolder,
                    );
                    views_folder.connection_id = Some(connection_id);
                    views_folder.database_name = Some("main".to_string());
                    views_folder.is_loaded = false;

                    main_children = vec![tables_folder, views_folder];
                }
                models::enums::DatabaseType::Redis => {
                    // Redis structure with databases
                    cache_data::build_redis_structure_from_cache(
                        self,
                        connection_id,
                        node,
                        databases,
                    );
                    return;
                }
                models::enums::DatabaseType::MsSQL => {
                    // Databases folder
                    let mut databases_folder = models::structs::TreeNode::new(
                        "Databases".to_string(),
                        models::enums::NodeType::DatabasesFolder,
                    );
                    databases_folder.connection_id = Some(connection_id);
                    for db_name in databases {
                        let mut db_node = models::structs::TreeNode::new(
                            db_name.clone(),
                            models::enums::NodeType::Database,
                        );
                        db_node.connection_id = Some(connection_id);
                        db_node.database_name = Some(db_name.clone());
                        db_node.is_loaded = false;
                        let mut tables_folder = models::structs::TreeNode::new(
                            "Tables".to_string(),
                            models::enums::NodeType::TablesFolder,
                        );
                        tables_folder.connection_id = Some(connection_id);
                        tables_folder.database_name = Some(db_name.clone());
                        tables_folder.is_loaded = false;
                        let mut views_folder = models::structs::TreeNode::new(
                            "Views".to_string(),
                            models::enums::NodeType::ViewsFolder,
                        );
                        views_folder.connection_id = Some(connection_id);
                        views_folder.database_name = Some(db_name.clone());
                        views_folder.is_loaded = false;
                        // Stored Procedures folder
                        let mut sp_folder = models::structs::TreeNode::new(
                            "Stored Procedures".to_string(),
                            models::enums::NodeType::StoredProceduresFolder,
                        );
                        sp_folder.connection_id = Some(connection_id);
                        sp_folder.database_name = Some(db_name.clone());
                        sp_folder.is_loaded = false;
                        // Functions folder
                        let mut fn_folder = models::structs::TreeNode::new(
                            "Functions".to_string(),
                            models::enums::NodeType::UserFunctionsFolder,
                        );
                        fn_folder.connection_id = Some(connection_id);
                        fn_folder.database_name = Some(db_name.clone());
                        fn_folder.is_loaded = false;
                        // Triggers folder (events not supported in MsSQL)
                        let mut trg_folder = models::structs::TreeNode::new(
                            "Triggers".to_string(),
                            models::enums::NodeType::TriggersFolder,
                        );
                        trg_folder.connection_id = Some(connection_id);
                        trg_folder.database_name = Some(db_name.clone());
                        trg_folder.is_loaded = false;

                        db_node.children = vec![
                            tables_folder,
                            views_folder,
                            sp_folder,
                            fn_folder,
                            trg_folder,
                        ];
                        databases_folder.children.push(db_node);
                    }

                    let mut dba_folder = models::structs::TreeNode::new(
                        "DBA Views".to_string(),
                        models::enums::NodeType::DBAViewsFolder,
                    );
                    dba_folder.connection_id = Some(connection_id);

                    let mut dba_children = Vec::new();

                    for (name, node_type, query) in crate::sidebar_database::get_default_dba_views(&models::enums::DatabaseType::MsSQL) {
                        let mut node = models::structs::TreeNode::new(name.to_string(), node_type);
                        node.connection_id = Some(connection_id);
                        node.is_loaded = false;
                        node.query = Some(query.to_string());
                        dba_children.push(node);
                   }

                    // Render Custom Views
                    log::debug!("Cache Builder: Rendering custom views for connection {}: found {}", connection_id, connection.custom_views.len());
                    for view in connection.custom_views.iter() {
                        let mut view_node = models::structs::TreeNode::new(
                            view.name.clone(),
                            models::enums::NodeType::CustomView,
                        );
                        view_node.connection_id = Some(connection_id);
                        view_node.query = Some(view.query.clone()); 
                        view_node.is_loaded = true;
                        dba_children.push(view_node);
                    }

                    dba_folder.children = dba_children;

                    main_children.push(databases_folder);
                    main_children.push(dba_folder);
                }
                models::enums::DatabaseType::ApiHttp => {}
            }

            node.children = main_children;
        }
    }

    pub fn find_specific_folder_node<'a>(
        node: &'a mut models::structs::TreeNode,
        connection_id: i64,
        folder_type: &models::enums::NodeType,
        database_name: &Option<String>,
    ) -> Option<&'a mut models::structs::TreeNode> {
        // Check if this node is the folder we're looking for
        if node.node_type == *folder_type
            && node.connection_id == Some(connection_id)
            && node.database_name == *database_name
            && node.is_expanded
            && !node.is_loaded
        {
            return Some(node);
        }

        // Recursively search in children
        for child in &mut node.children {
            if let Some(result) =
                Self::find_specific_folder_node(child, connection_id, folder_type, database_name)
            {
                return Some(result);
            }
        }

        None
    }
    pub fn load_databases_for_folder(
        &mut self,
        connection_id: i64,
        databases_folder: &mut models::structs::TreeNode,
    ) {
        // Check connection type to handle Redis differently
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            && connection.connection_type == models::enums::DatabaseType::Redis
        {
            self.load_redis_databases_for_folder(connection_id, databases_folder);
            return;
        }

        // Clear any loading placeholders
        databases_folder.children.clear();

        // First check cache
        if let Some(cached_databases) = cache_data::get_databases_from_cache(self, connection_id)
            && !cached_databases.is_empty()
        {
            for db_name in cached_databases {
                let mut db_node = models::structs::TreeNode::new(
                    db_name.clone(),
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Add subfolders for each database
                let mut db_children = Vec::new();



                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new(
                    "Tables".to_string(),
                    models::enums::NodeType::TablesFolder,
                );
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);

                // Views folder
                let mut views_folder = models::structs::TreeNode::new(
                    "Views".to_string(),
                    models::enums::NodeType::ViewsFolder,
                );
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);

                // Stored Procedures folder
                let mut sp_folder = models::structs::TreeNode::new(
                    "Stored Procedures".to_string(),
                    models::enums::NodeType::StoredProceduresFolder,
                );
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

        // Try to fetch real databases from the connection
        if let Some(databases) = {
             let dbs = self.get_databases_cached(connection_id);
             if !dbs.is_empty() { Some(dbs) } else { None }
        } {
            // Save to cache for future use
            cache_data::save_databases_to_cache(self, connection_id, &databases);

            // Create tree nodes from fetched data
            for db_name in databases {
                let mut db_node = models::structs::TreeNode::new(
                    db_name.clone(),
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Add subfolders for each database
                let mut db_children = Vec::new();



                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new(
                    "Tables".to_string(),
                    models::enums::NodeType::TablesFolder,
                );
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);

                // Views folder
                let mut views_folder = models::structs::TreeNode::new(
                    "Views".to_string(),
                    models::enums::NodeType::ViewsFolder,
                );
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);

                // Stored Procedures / Functions / Triggers depending on DB type
                if let Some(conn) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                {
                    match conn.connection_type {
                        models::enums::DatabaseType::MySQL => {
                            let mut sp_folder = models::structs::TreeNode::new(
                                "Stored Procedures".to_string(),
                                models::enums::NodeType::StoredProceduresFolder,
                            );
                            sp_folder.connection_id = Some(connection_id);
                            sp_folder.database_name = Some(db_name.clone());
                            sp_folder.is_loaded = false;
                            db_children.push(sp_folder);
                        }
                        models::enums::DatabaseType::MsSQL => {
                            let mut sp_folder = models::structs::TreeNode::new(
                                "Stored Procedures".to_string(),
                                models::enums::NodeType::StoredProceduresFolder,
                            );
                            sp_folder.connection_id = Some(connection_id);
                            sp_folder.database_name = Some(db_name.clone());
                            sp_folder.is_loaded = false;
                            db_children.push(sp_folder);
                            let mut fn_folder = models::structs::TreeNode::new(
                                "Functions".to_string(),
                                models::enums::NodeType::UserFunctionsFolder,
                            );
                            fn_folder.connection_id = Some(connection_id);
                            fn_folder.database_name = Some(db_name.clone());
                            fn_folder.is_loaded = false;
                            db_children.push(fn_folder);
                            let mut trg_folder = models::structs::TreeNode::new(
                                "Triggers".to_string(),
                                models::enums::NodeType::TriggersFolder,
                            );
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
    pub fn populate_sample_databases_for_folder(
        &mut self,
        connection_id: i64,
        databases_folder: &mut models::structs::TreeNode,
    ) {
        // Find the connection to determine type
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let sample_databases = match connection.connection_type {
                models::enums::DatabaseType::MySQL => vec![
                    "information_schema".to_string(),
                    "sakila".to_string(),
                    "world".to_string(),
                    "test".to_string(),
                ],
                models::enums::DatabaseType::PostgreSQL => vec![
                    "postgres".to_string(),
                    "template1".to_string(),
                    "dvdrental".to_string(),
                ],
                models::enums::DatabaseType::SQLite => vec!["main".to_string()],
                models::enums::DatabaseType::Redis => vec!["redis".to_string(), "info".to_string()],
                models::enums::DatabaseType::MsSQL => vec![
                    "master".to_string(),
                    "tempdb".to_string(),
                    "model".to_string(),
                    "msdb".to_string(),
                ],
                models::enums::DatabaseType::MongoDB => {
                    vec!["admin".to_string(), "local".to_string()]
                }
                models::enums::DatabaseType::ApiHttp => vec![],
            };

            // Clear loading message
            databases_folder.children.clear();

            // Add sample databases
            for db_name in sample_databases {
                // Skip system databases for display
                if matches!(
                    connection.connection_type,
                    models::enums::DatabaseType::MySQL
                ) && ["information_schema", "performance_schema", "mysql", "sys"]
                    .contains(&db_name.as_str())
                {
                    continue;
                }

                let mut db_node = models::structs::TreeNode::new(
                    db_name.clone(),
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Add subfolders for each database
                let mut db_children = Vec::new();

                // Tables folder
                let mut tables_folder = models::structs::TreeNode::new(
                    "Tables".to_string(),
                    models::enums::NodeType::TablesFolder,
                );
                tables_folder.connection_id = Some(connection_id);
                tables_folder.database_name = Some(db_name.clone());
                tables_folder.is_loaded = false;
                db_children.push(tables_folder);

                // Views folder
                let mut views_folder = models::structs::TreeNode::new(
                    "Views".to_string(),
                    models::enums::NodeType::ViewsFolder,
                );
                views_folder.connection_id = Some(connection_id);
                views_folder.database_name = Some(db_name.clone());
                views_folder.is_loaded = false;
                db_children.push(views_folder);

                if matches!(
                    connection.connection_type,
                    models::enums::DatabaseType::MySQL
                ) {
                    // Stored Procedures folder
                    let mut sp_folder = models::structs::TreeNode::new(
                        "Stored Procedures".to_string(),
                        models::enums::NodeType::StoredProceduresFolder,
                    );
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);

                    // User Functions folder
                    let mut uf_folder = models::structs::TreeNode::new(
                        "User Functions".to_string(),
                        models::enums::NodeType::UserFunctionsFolder,
                    );
                    uf_folder.connection_id = Some(connection_id);
                    uf_folder.database_name = Some(db_name.clone());
                    uf_folder.is_loaded = false;
                    db_children.push(uf_folder);

                    // Triggers folder
                    let mut triggers_folder = models::structs::TreeNode::new(
                        "Triggers".to_string(),
                        models::enums::NodeType::TriggersFolder,
                    );
                    triggers_folder.connection_id = Some(connection_id);
                    triggers_folder.database_name = Some(db_name.clone());
                    triggers_folder.is_loaded = false;
                    db_children.push(triggers_folder);

                    // Events folder
                    let mut events_folder = models::structs::TreeNode::new(
                        "Events".to_string(),
                        models::enums::NodeType::EventsFolder,
                    );
                    events_folder.connection_id = Some(connection_id);
                    events_folder.database_name = Some(db_name.clone());
                    events_folder.is_loaded = false;
                    db_children.push(events_folder);
                } else if matches!(
                    connection.connection_type,
                    models::enums::DatabaseType::MsSQL
                ) {
                    // For MsSQL, add Procedures, Functions, and Triggers (no Events)
                    let mut sp_folder = models::structs::TreeNode::new(
                        "Stored Procedures".to_string(),
                        models::enums::NodeType::StoredProceduresFolder,
                    );
                    sp_folder.connection_id = Some(connection_id);
                    sp_folder.database_name = Some(db_name.clone());
                    sp_folder.is_loaded = false;
                    db_children.push(sp_folder);

                    let mut fn_folder = models::structs::TreeNode::new(
                        "Functions".to_string(),
                        models::enums::NodeType::UserFunctionsFolder,
                    );
                    fn_folder.connection_id = Some(connection_id);
                    fn_folder.database_name = Some(db_name.clone());
                    fn_folder.is_loaded = false;
                    db_children.push(fn_folder);

                    let mut trg_folder = models::structs::TreeNode::new(
                        "Triggers".to_string(),
                        models::enums::NodeType::TriggersFolder,
                    );
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
    pub fn load_redis_databases_for_folder(
        &mut self,
        connection_id: i64,
        databases_folder: &mut models::structs::TreeNode,
    ) {
        // Clear loading placeholders
        databases_folder.children.clear();

        // Ambil daftar database Redis dari cache
        if let Some(cached_databases) = cache_data::get_databases_from_cache(self, connection_id) {
            for db_name in cached_databases {
                if !db_name.starts_with("db")
                    && db_name != crate::driver_redis::REDIS_CLUSTER_KEYSPACE
                {
                    continue;
                }

                let display_name = if db_name == crate::driver_redis::REDIS_CLUSTER_KEYSPACE {
                    "Keys".to_string()
                } else {
                    db_name.clone()
                };

                let mut db_node = models::structs::TreeNode::new(
                    display_name,
                    models::enums::NodeType::Database,
                );
                db_node.connection_id = Some(connection_id);
                db_node.database_name = Some(db_name.clone());
                db_node.is_loaded = false;

                // Tambahkan node child untuk key, akan di-load saat node di-expand
                let loading_keys_node = models::structs::TreeNode::new(
                    "Loading keys...".to_string(),
                    models::enums::NodeType::Table,
                );
                db_node.children.push(loading_keys_node);

                databases_folder.children.push(db_node);
            }
            databases_folder.is_loaded = true;
        }
    }
    pub fn find_redis_database_node<'a>(
        node: &'a mut models::structs::TreeNode,
        connection_id: i64,
        database_name: &Option<String>,
    ) -> Option<&'a mut models::structs::TreeNode> {
        // Check if this is the database node we're looking for
        if node.connection_id == Some(connection_id)
            && node.node_type == models::enums::NodeType::Database
            && node.database_name == *database_name
        {
            return Some(node);
        }

        // Recursively search in children
        for child in &mut node.children {
            if let Some(found) = Self::find_redis_database_node(child, connection_id, database_name)
            {
                return Some(found);
            }
        }

        None
    }
    pub fn load_redis_keys_for_database(
        &mut self,
        connection_id: i64,
        database_name: &str,
        db_node: &mut models::structs::TreeNode,
    ) {
        // If already fetching, do nothing — the background result will update the tree
        if self.fetching_redis_keys.contains(&(connection_id, database_name.to_string())) {
            log::debug!(
                "[redis_keys] fetch already in progress for connection {} keyspace {}",
                connection_id,
                database_name
            );
            return;
        }

        log::debug!(
            "[redis_keys] queueing key fetch for connection {} keyspace {} node '{}'",
            connection_id,
            database_name,
            db_node.name
        );

        // Mark as fetching and show a loading placeholder
        self.fetching_redis_keys.insert((connection_id, database_name.to_string()));
        db_node.children.clear();
        let loading_node = models::structs::TreeNode::new(
            "Loading keys...".to_string(),
            models::enums::NodeType::Table,
        );
        db_node.children.push(loading_node);
        // Keep is_loaded = false so the tree knows it is still pending

        // Dispatch background task — ensure connection pool is available first.
        // After creation, also copy the pool into shared_connection_pools so the
        // background FetchRedisKeys task can find it there.
        let rt = tokio::runtime::Runtime::new().unwrap();
        if let Some(pool) = rt.block_on(connection::get_or_create_connection_pool(self, connection_id)) {
            if let Ok(mut shared) = self.shared_connection_pools.lock() {
                shared.entry(connection_id).or_insert(pool);
                log::debug!(
                    "[redis_keys] shared pool ready for connection {} keyspace {}",
                    connection_id,
                    database_name
                );
            }
        } else {
            log::warn!(
                "[redis_keys] no pool available when queueing fetch for connection {} keyspace {}",
                connection_id,
                database_name
            );
        }

        if let Some(sender) = &self.background_sender {
            let _ = sender.send(models::enums::BackgroundTask::FetchRedisKeys {
                connection_id,
                database_name: database_name.to_string(),
            });
        }
    }

    pub fn get_databases_cached(&mut self, connection_id: i64) -> Vec<String> {
        // Try to get from cache first
        if let Some(databases) = cache_data::get_databases_from_cache(self, connection_id)
            && !databases.is_empty()
        {
            return databases;
        }
        
        // If not in cache or empty, trigger background fetch
        // Check if we are already fetching for this connection to avoid spamming
        let is_fetching = self.fetching_databases.contains(&connection_id);
        
        if !is_fetching {
             // Dispatch background task
             if let Some(sender) = &self.background_sender {
                 // Mark as fetching
                 self.fetching_databases.insert(connection_id);
                 let _ = sender.send(models::enums::BackgroundTask::FetchDatabases {
                     connection_id,
                 });
             }
        }

        // Return empty for now; UI will update when background task completes
        Vec::new()
    }
    pub fn load_folder_content(
        &mut self,
        connection_id: i64,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
        force_live_fetch: bool,
    ) {
        // Find the connection by ID
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();

            match connection.connection_type {
                models::enums::DatabaseType::MySQL => {
                    self.load_mysql_folder_content(connection_id, &connection, node, folder_type, force_live_fetch);
                }
                models::enums::DatabaseType::PostgreSQL => {
                    self.load_postgresql_folder_content(
                        connection_id,
                        &connection,
                        node,
                        folder_type,
                        force_live_fetch,
                    );
                }
                models::enums::DatabaseType::SQLite => {
                    self.load_sqlite_folder_content(connection_id, &connection, node, folder_type, force_live_fetch);
                }
                models::enums::DatabaseType::Redis => {
                    self.load_redis_folder_content(connection_id, &connection, node, folder_type);
                }
                models::enums::DatabaseType::MsSQL => {
                    self.load_mssql_folder_content(connection_id, &connection, node, folder_type, force_live_fetch);
                }
                models::enums::DatabaseType::MongoDB => {
                    // For MongoDB, TablesFolder represents collections
                    let database_name = node
                        .database_name
                        .clone()
                        .unwrap_or_else(|| connection.database.clone());
                    let table_type = "collection";

                    // Try cache first (skipped when force_live_fetch is true)
                    if !force_live_fetch
                    && let Some(cached) = cache_data::get_tables_from_cache(
                        self,
                        connection_id,
                        &database_name,
                        table_type,
                    ) && !cached.is_empty()
                    {
                        node.children = cached
                            .into_iter()
                            .map(|name| {
                                let mut child = models::structs::TreeNode::new(
                                    name,
                                    models::enums::NodeType::Table,
                                );
                                child.connection_id = Some(connection_id);
                                child.database_name = Some(database_name.clone());
                                child.is_loaded = false;
                                child
                            })
                            .collect();
                        node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                        return;
                    }

                    // Fallback to live fetch
                    if let Some(cols) =
                        crate::driver_mongodb::fetch_collections_from_mongodb_connection(
                            self,
                            connection_id,
                            &database_name,
                        )
                    {
                        let table_data: Vec<(String, String)> = cols
                            .iter()
                            .map(|n| (n.clone(), table_type.to_string()))
                            .collect();
                        cache_data::save_tables_to_cache(
                            self,
                            connection_id,
                            &database_name,
                            &table_data,
                        );
                        node.children = cols
                            .into_iter()
                            .map(|name| {
                                let mut child = models::structs::TreeNode::new(
                                    name,
                                    models::enums::NodeType::Table,
                                );
                                child.connection_id = Some(connection_id);
                                child.database_name = Some(database_name.clone());
                                child.is_loaded = false;
                                child
                            })
                            .collect();
                        node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                    } else {
                        node.children = vec![models::structs::TreeNode::new(
                            "Failed to load collections".to_string(),
                            models::enums::NodeType::Column,
                        )];
                    }
                }
                models::enums::DatabaseType::ApiHttp => {}
            }

            node.is_loaded = true;
        } else {
            debug!("ERROR: Connection with ID {} not found!", connection_id);
        }
    }
    pub fn load_mysql_folder_content(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
        force_live_fetch: bool,
    ) {
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

        // First try to get from cache (skipped when force_live_fetch is true)
        if !force_live_fetch
        && let Some(cached_items) =
            cache_data::get_tables_from_cache(self, connection_id, database_name, table_type)
            && !cached_items.is_empty()
        {
            eprintln!(
                "[TABULAR-DEBUG] MySQL load_folder: CACHE HIT conn={} db={:?} type={:?} count={} panen_found={}",
                connection_id, database_name, table_type, cached_items.len(),
                cached_items.iter().any(|n| n.to_lowercase().contains("panen"))
            );
            // Create tree nodes from cached data
            let child_nodes: Vec<models::structs::TreeNode> = cached_items
                .into_iter()
                .map(|item_name| {
                    let mut child_node = models::structs::TreeNode::new(
                        item_name.clone(),
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                            models::enums::NodeType::StoredProceduresFolder => {
                                models::enums::NodeType::StoredProcedure
                            }
                            models::enums::NodeType::UserFunctionsFolder => {
                                models::enums::NodeType::UserFunction
                            }
                            models::enums::NodeType::TriggersFolder => {
                                models::enums::NodeType::Trigger
                            }
                            models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                            _ => models::enums::NodeType::Table,
                        },
                    );
                    child_node.connection_id = Some(connection_id);
                    child_node.database_name = Some(database_name.clone());
                    child_node.is_loaded = false; // Will load columns on expansion if it's a table
                    child_node
                })
                .collect();

            node.children = child_nodes;
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return;
        }

        // If cache is empty, fetch from actual database
        if let Some(real_items) = driver_mysql::fetch_tables_from_mysql_connection(
            self,
            connection_id,
            database_name,
            table_type,
        ) {
            eprintln!(
                "[TABULAR-DEBUG] MySQL load_folder: LIVE FETCH conn={} db={:?} type={:?} count={} panen_found={}",
                connection_id, database_name, table_type, real_items.len(),
                real_items.iter().any(|n| n.to_lowercase().contains("panen"))
            );

            // Save to cache for future use
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|name| (name.clone(), table_type.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);

            // Create tree nodes from fetched data
            let child_nodes: Vec<models::structs::TreeNode> = real_items
                .into_iter()
                .map(|item_name| {
                    let mut child_node = models::structs::TreeNode::new(
                        item_name.clone(),
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                            models::enums::NodeType::StoredProceduresFolder => {
                                models::enums::NodeType::StoredProcedure
                            }
                            models::enums::NodeType::UserFunctionsFolder => {
                                models::enums::NodeType::UserFunction
                            }
                            models::enums::NodeType::TriggersFolder => {
                                models::enums::NodeType::Trigger
                            }
                            models::enums::NodeType::EventsFolder => models::enums::NodeType::Event,
                            _ => models::enums::NodeType::Table,
                        },
                    );
                    child_node.connection_id = Some(connection_id);
                    child_node.database_name = Some(database_name.clone());
                    child_node.is_loaded = false; // Will load columns on expansion if it's a table
                    child_node
                })
                .collect();

            node.children = child_nodes;
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        } else {
            // If database fetch fails, show an informative placeholder instead of confusing sample data
            debug!(
                "Failed to fetch from MySQL, showing placeholder for {}",
                table_type
            );
            let placeholder = match folder_type {
                models::enums::NodeType::TablesFolder => "Failed to load tables",
                models::enums::NodeType::ViewsFolder => "Failed to load views",
                models::enums::NodeType::StoredProceduresFolder => "Failed to load procedures",
                models::enums::NodeType::UserFunctionsFolder => "Failed to load functions",
                models::enums::NodeType::TriggersFolder => "Failed to load triggers",
                models::enums::NodeType::EventsFolder => "Failed to load events",
                _ => "Failed to load items",
            };
            node.children = vec![models::structs::TreeNode::new(
                placeholder.to_string(),
                models::enums::NodeType::Column,
            )];
        }

        debug!(
            "Loaded {} {} items for MySQL",
            node.children.len(),
            table_type
        );
    }
    pub fn load_postgresql_folder_content(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
        force_live_fetch: bool,
    ) {
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            _ => {
                node.children = vec![models::structs::TreeNode::new(
                    "Not supported for PostgreSQL".to_string(),
                    models::enums::NodeType::Column,
                )];
                return;
            }
        };

        // Try cache first (skipped when force_live_fetch is true)
        if !force_live_fetch
        && let Some(cached) =
            cache_data::get_tables_from_cache(self, connection_id, database_name, table_type)
            && !cached.is_empty()
        {
            eprintln!(
                "[TABULAR-DEBUG] PG load_folder: CACHE HIT conn={} db={:?} type={:?} count={} panen_found={}",
                connection_id, database_name, table_type, cached.len(),
                cached.iter().any(|n| n.to_lowercase().contains("panen"))
            );
            node.children = cached
                .into_iter()
                .map(|name| {
                    let mut child = models::structs::TreeNode::new(
                        name,
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            _ => models::enums::NodeType::View,
                        },
                    );
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child.is_loaded = false;
                    child
                })
                .collect();
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return;
        }

        // Fallback to live fetch
        if let Some(real_items) = crate::driver_postgres::fetch_tables_from_postgres_connection(
            self,
            connection_id,
            database_name,
            table_type,
        ) {
            eprintln!(
                "[TABULAR-DEBUG] PG load_folder: LIVE FETCH conn={} db={:?} type={:?} count={} panen_found={}",
                connection_id, database_name, table_type, real_items.len(),
                real_items.iter().any(|n| n.to_lowercase().contains("panen"))
            );
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|n| (n.clone(), table_type.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            node.children = real_items
                .into_iter()
                .map(|name| {
                    let mut child = models::structs::TreeNode::new(
                        name,
                        match folder_type {
                            models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                            _ => models::enums::NodeType::View,
                        },
                    );
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child.is_loaded = false;
                    child
                })
                .collect();
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        } else {
            node.children = vec![models::structs::TreeNode::new(
                "Failed to load items".to_string(),
                models::enums::NodeType::Column,
            )];
        }
    }
    pub fn load_sqlite_folder_content(
        &mut self,
        connection_id: i64,
        _connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
        force_live_fetch: bool,
    ) {
        debug!("Loading {:?} content for SQLite", folder_type);

        // Try to get from cache first
        let table_type = match folder_type {
            models::enums::NodeType::TablesFolder => "table",
            models::enums::NodeType::ViewsFolder => "view",
            _ => {
                // For other folder types, return empty for now
                node.children = vec![models::structs::TreeNode::new(
                    "Not supported for SQLite".to_string(),
                    models::enums::NodeType::Column,
                )];
                return;
            }
        };

        // Try cache first (skipped when force_live_fetch is true)
        if !force_live_fetch
        && let Some(cached_items) =
            cache_data::get_tables_from_cache(self, connection_id, "main", table_type)
            && !cached_items.is_empty()
        {
            debug!(
                "Loading {} {} from cache for SQLite",
                cached_items.len(),
                table_type
            );

            node.children = cached_items
                .into_iter()
                .map(|item_name| {
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
                })
                .collect();

            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return;
        }

        // If cache is empty, fetch from actual SQLite database
        debug!(
            "Cache miss, fetching {} from actual SQLite database",
            table_type
        );

        if let Some(real_items) =
            driver_sqlite::fetch_tables_from_sqlite_connection(self, connection_id, table_type)
        {
            debug!(
                "Successfully fetched {} {} from SQLite database",
                real_items.len(),
                table_type
            );

            // Save to cache for future use
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|name| (name.clone(), table_type.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, "main", &table_data);

            // Create tree nodes from fetched data
            let child_nodes: Vec<models::structs::TreeNode> = real_items
                .into_iter()
                .map(|item_name| {
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
                })
                .collect();

            node.children = child_nodes;
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        } else {
            // If database fetch fails, add sample data as fallback
            debug!(
                "Failed to fetch from SQLite, using sample {} data",
                table_type
            );

            let sample_items = match folder_type {
                models::enums::NodeType::TablesFolder => vec![
                    "users".to_string(),
                    "products".to_string(),
                    "orders".to_string(),
                    "categories".to_string(),
                ],
                models::enums::NodeType::ViewsFolder => {
                    vec!["user_summary".to_string(), "order_details".to_string()]
                }
                _ => vec![],
            };

            let item_type = match folder_type {
                models::enums::NodeType::TablesFolder => models::enums::NodeType::Table,
                models::enums::NodeType::ViewsFolder => models::enums::NodeType::View,
                _ => models::enums::NodeType::Column, // fallback
            };

            node.children = sample_items
                .into_iter()
                .map(|item_name| {
                    let mut item_node =
                        models::structs::TreeNode::new(item_name.clone(), item_type.clone());
                    item_node.connection_id = Some(connection_id);
                    item_node.database_name = Some("main".to_string());
                    item_node.is_loaded = false;
                    item_node
                })
                .collect();
        }

        debug!(
            "Loaded {} items into {:?} folder for SQLite",
            node.children.len(),
            folder_type
        );
    }
    pub fn load_mssql_folder_content(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
        force_live_fetch: bool,
    ) {
        debug!("Loading {:?} content for MsSQL", folder_type);
        let database_name = node.database_name.as_ref().unwrap_or(&connection.database);

        let (kind, node_mapper): (&str, fn(String) -> models::structs::TreeNode) = match folder_type
        {
            models::enums::NodeType::TablesFolder => ("table", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::Table);
                child.is_loaded = false;
                child
            }),
            models::enums::NodeType::ViewsFolder => ("view", |name: String| {
                let mut child = models::structs::TreeNode::new(name, models::enums::NodeType::View);
                child.is_loaded = false;
                child
            }),
            models::enums::NodeType::StoredProceduresFolder => ("procedure", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::StoredProcedure);
                child.is_loaded = true;
                child
            }),
            models::enums::NodeType::UserFunctionsFolder => ("function", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::UserFunction);
                child.is_loaded = true;
                child
            }),
            models::enums::NodeType::TriggersFolder => ("trigger", |name: String| {
                let mut child =
                    models::structs::TreeNode::new(name, models::enums::NodeType::Trigger);
                child.is_loaded = true;
                child
            }),
            _ => {
                node.children = vec![models::structs::TreeNode::new(
                    "Unsupported folder for MsSQL".to_string(),
                    models::enums::NodeType::Column,
                )];
                return;
            }
        };

        // Try cache first (skipped when force_live_fetch is true)
        if !force_live_fetch
        && let Some(cached) =
            cache_data::get_tables_from_cache(self, connection_id, database_name, kind)
            && !cached.is_empty()
        {
            node.children = cached
                .into_iter()
                .map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                })
                .collect();
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            return;
        }

        let fetched = match kind {
            "table" | "view" => crate::driver_mssql::fetch_tables_from_mssql_connection(
                self,
                connection_id,
                database_name,
                kind,
            ),
            "procedure" | "function" | "trigger" => {
                crate::driver_mssql::fetch_objects_from_mssql_connection(
                    self,
                    connection_id,
                    database_name,
                    kind,
                )
            }
            _ => None,
        };

        if let Some(real_items) = fetched {
            let table_data: Vec<(String, String)> = real_items
                .iter()
                .map(|n| (n.clone(), kind.to_string()))
                .collect();
            cache_data::save_tables_to_cache(self, connection_id, database_name, &table_data);
            node.children = real_items
                .into_iter()
                .map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                })
                .collect();
            node.children.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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
            node.children = sample
                .into_iter()
                .map(|name| {
                    let mut child = node_mapper(name);
                    child.connection_id = Some(connection_id);
                    child.database_name = Some(database_name.clone());
                    child
                })
                .collect();
        }
    }
    pub fn load_redis_folder_content(
        &mut self,
        connection_id: i64,
        _connection: &models::structs::ConnectionConfig,
        node: &mut models::structs::TreeNode,
        folder_type: models::enums::NodeType,
    ) {
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

                node.children = redis_structures
                    .into_iter()
                    .map(|structure_name| {
                        let mut structure_node = models::structs::TreeNode::new(
                            structure_name,
                            models::enums::NodeType::Table,
                        );
                        structure_node.connection_id = Some(connection_id);
                        structure_node.database_name = Some("redis".to_string());
                        structure_node.is_loaded = false;
                        structure_node
                    })
                    .collect();
            }
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

                node.children = info_sections
                    .into_iter()
                    .map(|section_name| {
                        let mut section_node = models::structs::TreeNode::new(
                            section_name,
                            models::enums::NodeType::View,
                        );
                        section_node.connection_id = Some(connection_id);
                        section_node.database_name = Some("info".to_string());
                        section_node.is_loaded = false;
                        section_node
                    })
                    .collect();
            }
            _ => {
                // Other folder types not supported for Redis
                node.children = vec![models::structs::TreeNode::new(
                    "Not supported for Redis".to_string(),
                    models::enums::NodeType::Column,
                )];
            }
        }

        debug!(
            "Loaded {} items into {:?} folder for Redis",
            node.children.len(),
            folder_type
        );
    }
    pub fn load_table_columns_sync(
        &mut self,
        connection_id: i64,
        table_name: &str,
        connection: &models::structs::ConnectionConfig,
        database_name: &str,
    ) -> Vec<models::structs::TreeNode> {
        // First try to get from cache
        if let Some(cached_columns) =
            cache_data::get_columns_from_cache(self, connection_id, database_name, table_name)
            && !cached_columns.is_empty()
        {
            return cached_columns
                .into_iter()
                .map(|(column_name, data_type)| {
                    models::structs::TreeNode::new(
                        format!("{} ({})", column_name, data_type),
                        models::enums::NodeType::Column,
                    )
                })
                .collect();
        }

        // If cache is empty, fetch from actual database
        if let Some(real_columns) = connection::fetch_columns_from_database(
            connection_id,
            database_name,
            table_name,
            connection,
        ) {
            // Save to cache for future use
            cache_data::save_columns_to_cache(
                self,
                connection_id,
                database_name,
                table_name,
                &real_columns,
            );

            // Convert to models::structs::TreeNode
            real_columns
                .into_iter()
                .map(|(column_name, data_type)| {
                    models::structs::TreeNode::new(
                        format!("{} ({})", column_name, data_type),
                        models::enums::NodeType::Column,
                    )
                })
                .collect()
        } else {
            // If database fetch fails, return sample columns
            vec![
                models::structs::TreeNode::new(
                    "id (INTEGER)".to_string(),
                    models::enums::NodeType::Column,
                ),
                models::structs::TreeNode::new(
                    "name (VARCHAR)".to_string(),
                    models::enums::NodeType::Column,
                ),
                models::structs::TreeNode::new(
                    "created_at (TIMESTAMP)".to_string(),
                    models::enums::NodeType::Column,
                ),
            ]
        }
    }
    pub fn load_table_columns_for_node(
        &mut self,
        connection_id: i64,
        table_name: &str,
        nodes: &mut [models::structs::TreeNode],
        _table_index: usize,
    ) {
        // Find the connection by ID
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();

            // Find the table node to get the correct database_name
            let database_name = Tabular::find_table_database_name(nodes, table_name, connection_id)
                .unwrap_or_else(|| connection.database.clone());

            // Load columns, indexes, and primary keys from cache instead of querying server
            let columns_from_cache =
                self.load_table_columns_from_cache(connection_id, table_name, &database_name);
            let (indexes_list, pk_columns) =
                self.extract_indexes_and_pks_from_cache(connection_id, &database_name, table_name);
            let partitions_list = self.extract_partitions_from_cache(connection_id, &database_name, table_name);

            let mut columns_folder = models::structs::TreeNode::new(
                "Columns".to_string(),
                models::enums::NodeType::ColumnsFolder,
            );
            columns_folder.connection_id = Some(connection_id);
            columns_folder.database_name = Some(database_name.clone());
            columns_folder.table_name = Some(table_name.to_string());
            columns_folder.is_loaded = true;
            columns_folder.children = columns_from_cache;

            let mut indexes_folder = models::structs::TreeNode::new(
                "Indexes".to_string(),
                models::enums::NodeType::IndexesFolder,
            );
            indexes_folder.connection_id = Some(connection_id);
            indexes_folder.database_name = Some(database_name.clone());
            indexes_folder.table_name = Some(table_name.to_string());
            indexes_folder.is_loaded = true;
            indexes_folder.children = indexes_list
                .into_iter()
                .map(|idx| {
                    let mut n = models::structs::TreeNode::new(idx, models::enums::NodeType::Index);
                    n.connection_id = Some(connection_id);
                    n.database_name = Some(database_name.clone());
                    n.table_name = Some(table_name.to_string());
                    n
                })
                .collect();

            let mut pks_folder = models::structs::TreeNode::new(
                "Primary Keys".to_string(),
                models::enums::NodeType::PrimaryKeysFolder,
            );
            pks_folder.connection_id = Some(connection_id);
            pks_folder.database_name = Some(database_name.clone());
            pks_folder.table_name = Some(table_name.to_string());
            pks_folder.is_loaded = true;
            pks_folder.children = pk_columns
                .into_iter()
                .map(|col| models::structs::TreeNode::new(col, models::enums::NodeType::Column))
                .collect();

            let mut partitions_folder = models::structs::TreeNode::new(
                "Partitions".to_string(),
                models::enums::NodeType::PartitionsFolder,
            );
            partitions_folder.connection_id = Some(connection_id);
            partitions_folder.database_name = Some(database_name.clone());
            partitions_folder.table_name = Some(table_name.to_string());
            partitions_folder.is_loaded = true;
            partitions_folder.children = partitions_list
                .into_iter()
                .map(|part| {
                    // Format partition display: "name (TYPE)" if type is available
                    let display_name = if let Some(ref ptype) = part.partition_type {
                        format!("{} ({})", part.name, ptype)
                    } else {
                        part.name.clone()
                    };
                    let mut n = models::structs::TreeNode::new(display_name, models::enums::NodeType::Index);
                    n.connection_id = Some(connection_id);
                    n.database_name = Some(database_name.clone());
                    n.table_name = Some(table_name.to_string());
                    // Store the full partition info in file_path for later use
                    if let Some(ref ptype) = part.partition_type {
                        n.file_path = Some(format!("{}|{}", part.name, ptype));
                    }
                    n
                })
                .collect();

            let subfolders = vec![columns_folder, indexes_folder, pks_folder, partitions_folder];

            // Find the table node recursively and update it with subfolders
            let updated = Self::update_table_node_with_columns_recursive(
                nodes,
                table_name,
                subfolders,
                connection_id,
            );

            if !updated {
                // Log only if update failed
            }
        }
    }
    pub fn find_table_database_name(
        nodes: &[models::structs::TreeNode],
        table_name: &str,
        connection_id: i64,
    ) -> Option<String> {
        for node in nodes {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table
                || node.node_type == models::enums::NodeType::View)
                && node.connection_id == Some(connection_id)
            {
                let matches = if let Some(raw) = &node.table_name {
                    raw == table_name
                } else {
                    node.name == table_name
                        || Self::sanitize_display_table_name(&node.name) == table_name
                };
                if matches {
                    return node.database_name.clone();
                }
            }

            // Recursively search in children
            if let Some(found_db) =
                Self::find_table_database_name(&node.children, table_name, connection_id)
            {
                return Some(found_db);
            }
        }
        None
    }
    pub fn update_table_node_with_columns_recursive(
        nodes: &mut [models::structs::TreeNode],
        table_name: &str,
        columns: Vec<models::structs::TreeNode>,
        connection_id: i64,
    ) -> bool {
        for node in nodes.iter_mut() {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table
                || node.node_type == models::enums::NodeType::View)
                && node.connection_id == Some(connection_id)
            {
                let matches = if let Some(raw) = &node.table_name {
                    raw == table_name
                } else {
                    node.name == table_name
                        || Self::sanitize_display_table_name(&node.name) == table_name
                };
                if matches {
                    node.children = columns;
                    node.is_loaded = true;
                    return true;
                }
            }

            // Recursively search in children
            if Self::update_table_node_with_columns_recursive(
                &mut node.children,
                table_name,
                columns.clone(),
                connection_id,
            ) {
                return true;
            }
        }
        false
    }
    pub fn find_table_node_in_main_tree(
        &self,
        table_name: &str,
        connection_id: i64,
    ) -> Option<models::structs::TreeNode> {
        Self::find_table_node_recursive(&self.items_tree, table_name, connection_id)
    }
    pub fn find_table_node_recursive(
        nodes: &[models::structs::TreeNode],
        table_name: &str,
        connection_id: i64,
    ) -> Option<models::structs::TreeNode> {
        for node in nodes {
            // If this is the table node we're looking for
            if (node.node_type == models::enums::NodeType::Table
                || node.node_type == models::enums::NodeType::View)
                && node.connection_id == Some(connection_id)
            {
                let matches = if let Some(raw) = &node.table_name {
                    raw == table_name
                } else {
                    node.name == table_name
                        || Self::sanitize_display_table_name(&node.name) == table_name
                };
                if matches {
                    return Some(node.clone());
                }
            }

            // Recursively search in children
            if let Some(found_node) =
                Self::find_table_node_recursive(&node.children, table_name, connection_id)
            {
                return Some(found_node);
            }
        }
        None
    }
    pub fn load_table_columns_from_cache(
        &mut self,
        connection_id: i64,
        table_name: &str,
        database_name: &str,
    ) -> Vec<models::structs::TreeNode> {
        // First try to get columns from cache
        let columns_from_cache = crate::cache_data::get_columns_from_cache(
            self,
            connection_id,
            database_name,
            table_name,
        );

        if let Some(columns_data) = columns_from_cache
            && !columns_data.is_empty()
        {
            // If cache has data, use it
            return columns_data
                .into_iter()
                .map(|(column_name, data_type)| {
                    let mut column_node = models::structs::TreeNode::new(
                        format!("{} ({})", column_name, data_type),
                        models::enums::NodeType::Column,
                    );
                    column_node.connection_id = Some(connection_id);
                    column_node.database_name = Some(database_name.to_string());
                    column_node.table_name = Some(table_name.to_string());
                    column_node
                })
                .collect();
        }

        // If cache doesn't have data or is empty, fallback to server query
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();
            self.load_table_columns_sync(connection_id, table_name, &connection, database_name)
        } else {
            Vec::new()
        }
    }
    pub fn extract_indexes_and_pks_from_cache(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) -> (Vec<String>, Vec<String>) {
        // Try to get primary keys from cache first
        let pk_columns = if let Some(pks) =
            cache_data::get_primary_keys_from_cache(self, connection_id, database_name, table_name)
        {
            if !pks.is_empty() {
                pks
            } else {
                // Cache is empty, fallback to server query
                if let Some(connection) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                {
                    let connection = connection.clone();
                    self.fetch_primary_key_columns_for_table(
                        connection_id,
                        &connection,
                        database_name,
                        table_name,
                    )
                } else {
                    Vec::new()
                }
            }
        } else {
            // Cache doesn't have data, fallback to server query
            if let Some(connection) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
            {
                let connection = connection.clone();
                self.fetch_primary_key_columns_for_table(
                    connection_id,
                    &connection,
                    database_name,
                    table_name,
                )
            } else {
                Vec::new()
            }
        };

        // Try to get index names from cache first (fast tree render)
        let indexes_list = if let Some(names) =
            cache_data::get_index_names_from_cache(self, connection_id, database_name, table_name)
        {
            if !names.is_empty() {
                names
            } else {
                // Cache empty: fallback to live fetch and seed cache with names
                if let Some(connection) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                {
                    let connection = connection.clone();
                    let names = self.fetch_index_names_for_table(
                        connection_id,
                        &connection,
                        database_name,
                        table_name,
                    );
                    if !names.is_empty() {
                        let stubs: Vec<models::structs::IndexStructInfo> = names
                            .iter()
                            .map(|n| models::structs::IndexStructInfo {
                                name: n.clone(),
                                method: None,
                                unique: false,
                                columns: Vec::new(),
                            })
                            .collect();
                        cache_data::save_indexes_to_cache(
                            self,
                            connection_id,
                            database_name,
                            table_name,
                            &stubs,
                        );
                    }
                    names
                } else {
                    Vec::new()
                }
            }
        } else {
            // No cache table or error: fallback and seed cache
            if let Some(connection) = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
            {
                let connection = connection.clone();
                let names = self.fetch_index_names_for_table(
                    connection_id,
                    &connection,
                    database_name,
                    table_name,
                );
                if !names.is_empty() {
                    let stubs: Vec<models::structs::IndexStructInfo> = names
                        .iter()
                        .map(|n| models::structs::IndexStructInfo {
                            name: n.clone(),
                            method: None,
                            unique: false,
                            columns: Vec::new(),
                        })
                        .collect();
                    cache_data::save_indexes_to_cache(
                        self,
                        connection_id,
                        database_name,
                        table_name,
                        &stubs,
                    );
                }
                names
            } else {
                Vec::new()
            }
        };

        (indexes_list, pk_columns)
    }
    pub fn extract_partitions_from_cache(
        &mut self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) -> Vec<models::structs::PartitionStructInfo> {
        // Try cache first
        if let Some(cached_partitions) = cache_data::get_partitions_from_cache(self, connection_id, database_name, table_name) {
            debug!("📚 Using cached partitions for {}/{} ({} partitions)", database_name, table_name, cached_partitions.len());
            return cached_partitions;
        }
        
        // Cache miss - fetch from database
        if let Some(connection) = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
        {
            let connection = connection.clone();
            debug!("🔍 Fetching partition details from DB for {}/{}", database_name, table_name);
            // Use the fetch function from data_table module
            let partitions = crate::data_table::fetch_partition_details_for_table(
                self,
                connection_id,
                &connection,
                database_name,
                table_name,
            );
            debug!("📊 Fetched {} partitions from database", partitions.len());
            if !partitions.is_empty() {
                debug!("💾 Saving {} partitions to cache", partitions.len());
                cache_data::save_partitions_to_cache(
                    self,
                    connection_id,
                    database_name,
                    table_name,
                    &partitions,
                );
            }
            partitions
        } else {
            debug!("⚠️  Connection not found: {}", connection_id);
            Vec::new()
        }
    }
    pub fn fetch_index_names_for_table(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        database_name: &str,
        table_name: &str,
    ) -> Vec<String> {
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT DISTINCT INDEX_NAME FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(database_name)
                            .bind(table_name)
                            .fetch_all(mysql_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::PostgreSQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT indexname FROM pg_indexes WHERE schemaname = 'public' AND tablename = $1 ORDER BY indexname";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(table_name)
                            .fetch_all(pg_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::SQLite => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::SQLite(sqlite_pool)) =
                        connection::get_or_create_connection_pool(self, connection_id).await
                    {
                        let escaped = table_name.replace("'", "''");
                        let q = format!("PRAGMA index_list('{}')", escaped);
                        match sqlx::query(&q).fetch_all(sqlite_pool.as_ref()).await {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut names = Vec::new();
                                for r in rows {
                                    if let Ok(Some(n)) = r.try_get::<Option<String>, _>("name") {
                                        names.push(n);
                                    }
                                }
                                names
                            }
                            Err(_) => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                })
            }
            models::enums::DatabaseType::MsSQL => {
                // Use tiberius
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                let host = connection.host.clone();
                let port: u16 = connection.port.parse().unwrap_or(1433);
                let user = connection.username.clone();
                let pass = connection.password.clone();
                let db = database_name.to_string();
                let tbl = table_name.to_string();
                let rt_res = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    // Parse schema-qualified name
                    let parse = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 { return (Some(parts[0].to_string()), parts[1].to_string()); }
                        }
                        if let Some((s, t)) = name.split_once('.') { return (Some(s.trim_matches(|c| c=='['||c==']').to_string()), t.trim_matches(|c| c=='['||c==']').to_string()); }
                        (None, name.trim_matches(|c| c=='['||c==']').to_string())
                    };
                    let (schema_opt, table_only) = parse(&tbl);
                    let mut q = format!("SELECT i.name FROM sys.indexes i INNER JOIN sys.objects o ON i.object_id = o.object_id WHERE o.name = '{}' AND i.name IS NOT NULL", table_only.replace("'", "''"));
                    if let Some(s) = schema_opt { q.push_str(&format!(" AND SCHEMA_NAME(o.schema_id) = '{}'", s.replace("'", "''"))); }
                    q.push_str(" ORDER BY i.name");
                    let mut stream = client.simple_query(q).await.map_err(|e| e.to_string())?;
                    let mut list = Vec::new();
                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let tiberius::QueryItem::Row(r) = item { let n: Option<&str> = r.get(0); if let Some(nm) = n { list.push(nm.to_string()); } } }
                    Ok::<_, String>(list)
                });
                rt_res.unwrap_or_default()
            }
            models::enums::DatabaseType::Redis => Vec::new(),
            models::enums::DatabaseType::MongoDB => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MongoDB(client)) =
                        connection::get_or_create_connection_pool(self, connection_id).await
                    {
                        let coll = client
                            .database(database_name)
                            .collection::<mongodb::bson::Document>(table_name);
                        (coll.list_index_names().await).unwrap_or_default()
                    } else {
                        Vec::new()
                    }
                })
            }
            models::enums::DatabaseType::ApiHttp => Vec::new(),
        }
    }

    pub fn fetch_primary_key_columns_for_table(
        &mut self,
        connection_id: i64,
        connection: &models::structs::ConnectionConfig,
        database_name: &str,
        table_name: &str,
    ) -> Vec<String> {
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY' ORDER BY ORDINAL_POSITION";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(database_name)
                            .bind(table_name)
                            .fetch_all(mysql_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::PostgreSQL => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = connection::get_or_create_connection_pool(self, connection_id).await {
                        let q = "SELECT a.attname FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum = ANY(i.indkey) JOIN pg_namespace n ON n.oid = c.relnamespace WHERE i.indisprimary AND c.relname = $1 AND n.nspname = 'public' ORDER BY a.attnum";
                        match sqlx::query_as::<_, (String,)>(q)
                            .bind(table_name)
                            .fetch_all(pg_pool.as_ref())
                            .await {
                                Ok(rows) => rows.into_iter().map(|(n,)| n).collect(),
                                Err(_) => Vec::new(),
                            }
                    } else { Vec::new() }
                })
            }
            models::enums::DatabaseType::SQLite => {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    if let Some(models::enums::DatabasePool::SQLite(sqlite_pool)) =
                        connection::get_or_create_connection_pool(self, connection_id).await
                    {
                        let escaped = table_name.replace("'", "''");
                        let q = format!("PRAGMA table_info('{}')", escaped);
                        match sqlx::query(&q).fetch_all(sqlite_pool.as_ref()).await {
                            Ok(rows) => {
                                use sqlx::Row;
                                let mut names = Vec::new();
                                for r in rows {
                                    let pk: i64 = r.try_get::<i64, _>("pk").unwrap_or(0);
                                    if pk > 0
                                        && let Ok(Some(n)) = r.try_get::<Option<String>, _>("name")
                                    {
                                        names.push(n);
                                    }
                                }
                                names
                            }
                            Err(_) => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                })
            }
            models::enums::DatabaseType::MsSQL => {
                // Use tiberius
                use tiberius::{AuthMethod, Config};
                use tokio_util::compat::TokioAsyncWriteCompatExt;
                let host = connection.host.clone();
                let port: u16 = connection.port.parse().unwrap_or(1433);
                let user = connection.username.clone();
                let pass = connection.password.clone();
                let db = database_name.to_string();
                let tbl = table_name.to_string();
                let rt_res = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let mut config = Config::new();
                    config.host(host.clone());
                    config.port(port);
                    config.authentication(AuthMethod::sql_server(user.clone(), pass.clone()));
                    config.trust_cert();
                    if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?;
                    tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    // Parse schema-qualified name
                    let parse = |name: &str| -> (Option<String>, String) {
                        if name.starts_with('[') && name.contains("].[") && name.ends_with(']') {
                            let trimmed = name.trim_matches(|c| c == '[' || c == ']');
                            let parts: Vec<&str> = trimmed.split("].[").collect();
                            if parts.len() >= 2 { return (Some(parts[0].to_string()), parts[1].to_string()); }
                        }
                        if let Some((s, t)) = name.split_once('.') { return (Some(s.trim_matches(|c| c=='['||c==']').to_string()), t.trim_matches(|c| c=='['||c==']').to_string()); }
                        (None, name.trim_matches(|c| c=='['||c==']').to_string())
                    };
                    let (schema_opt, table_only) = parse(&tbl);
                    let mut q = String::from("SELECT c.name FROM sys.indexes i JOIN sys.index_columns ic ON i.object_id = ic.object_id AND i.index_id = ic.index_id JOIN sys.columns c ON ic.object_id = c.object_id AND ic.column_id = c.column_id JOIN sys.objects o ON i.object_id = o.object_id WHERE i.is_primary_key = 1");
                    q.push_str(&format!(" AND o.name = '{}'", table_only.replace("'", "''")));
                    if let Some(s) = schema_opt { q.push_str(&format!(" AND SCHEMA_NAME(o.schema_id) = '{}'", s.replace("'", "''"))); }
                    q.push_str(" ORDER BY ic.key_ordinal");
                    let mut stream = client.simple_query(q).await.map_err(|e| e.to_string())?;
                    let mut list = Vec::new();
                    use futures_util::TryStreamExt;
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let tiberius::QueryItem::Row(r) = item { let n: Option<&str> = r.get(0); if let Some(nm) = n { list.push(nm.to_string()); } } }
                    Ok::<_, String>(list)
                });
                rt_res.unwrap_or_default()
            }
            models::enums::DatabaseType::Redis => Vec::new(),
            models::enums::DatabaseType::MongoDB => vec!["_id".to_string()],
            models::enums::DatabaseType::ApiHttp => vec![],
        }
    }

    /// Refresh all currently-expanded table/view/etc folders for a connection.
    /// Called after background prefetch completes so newly-cached tables become visible
    /// without requiring the user to close and re-open the folder.
    fn refresh_folder_nodes_recursive(
        nodes: &mut [models::structs::TreeNode],
        connection_id: i64,
        tabular: &mut super::Tabular,
    ) {
        for node in nodes.iter_mut() {
            let reloadable = matches!(
                node.node_type,
                models::enums::NodeType::TablesFolder
                    | models::enums::NodeType::ViewsFolder
                    | models::enums::NodeType::StoredProceduresFolder
                    | models::enums::NodeType::UserFunctionsFolder
                    | models::enums::NodeType::TriggersFolder
                    | models::enums::NodeType::EventsFolder
            );

            if reloadable
                && node.connection_id == Some(connection_id)
                && node.is_expanded
                && node.is_loaded
            {
                node.is_loaded = false;
                let folder_type = node.node_type.clone();
                tabular.load_folder_content(connection_id, node, folder_type, false);
            }

            // Recurse — node.children is separate from tabular (items_tree was taken out)
            Self::refresh_folder_nodes_recursive(&mut node.children, connection_id, tabular);
        }
    }

    /// Public entry point: refresh all table folders across the entire items_tree for a connection.
    pub fn refresh_all_table_folders(&mut self, connection_id: i64) {
        // Take items_tree out so we can mutably borrow self alongside the nodes.
        let mut items_tree = std::mem::take(&mut self.items_tree);
        Self::refresh_folder_nodes_recursive(&mut items_tree, connection_id, self);
        self.items_tree = items_tree;
    }
}
