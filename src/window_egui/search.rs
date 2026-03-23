use eframe::egui;
use std::collections::HashMap;
use crate::{models, connection, cache_data};
use log::debug;

impl super::Tabular {
    pub fn update_search_results(&mut self) {
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

        // Search in all connections' cached data
        let connection_ids: Vec<i64> = self.connections.iter().filter_map(|c| c.id).collect();

        for connection_id in connection_ids {
            self.search_in_connection_data(connection_id, &search_text);
        }
    }
    pub fn filter_node_with_like_search(
        &self,
        node: &models::structs::TreeNode,
        search_text: &str,
    ) -> Option<models::structs::TreeNode> {
        let mut matches = false;
        let mut filtered_children = Vec::new();

        // Check if current node matches using case-sensitive LIKE search
        // LIKE search: if search text is contained anywhere in the node name
        if node.name.contains(search_text) {
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

            // For table nodes, preserve loaded state and children from main tree
            if (filtered_node.node_type == models::enums::NodeType::Table
                || filtered_node.node_type == models::enums::NodeType::View)
                && filtered_node.connection_id.is_some()
            {
                if let Some(main_tree_node) = self.find_table_node_in_main_tree(
                    &filtered_node.name,
                    filtered_node.connection_id.unwrap(),
                ) {
                    filtered_node.is_loaded = main_tree_node.is_loaded;
                    filtered_node.is_expanded = main_tree_node.is_expanded;
                    if main_tree_node.is_loaded {
                        filtered_node.children = main_tree_node.children.clone();
                    } else {
                        filtered_node.children = filtered_children;
                    }
                } else {
                    filtered_node.children = filtered_children;
                    filtered_node.is_expanded = true; // Auto-expand search results
                }
            } else {
                filtered_node.children = filtered_children;
                filtered_node.is_expanded = true; // Auto-expand search results
            }

            Some(filtered_node)
        } else {
            None
        }
    }
    pub fn search_in_connection_data(&mut self, connection_id: i64, search_text: &str) {
        // Find the connection to determine its type
        let connection_type = self
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .map(|c| c.connection_type.clone());

        if let Some(conn_type) = connection_type {
            match conn_type {
                models::enums::DatabaseType::Redis => {
                    self.search_redis_keys(connection_id, search_text);
                }
                models::enums::DatabaseType::MySQL
                | models::enums::DatabaseType::PostgreSQL
                | models::enums::DatabaseType::SQLite => {
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
                models::enums::DatabaseType::MsSQL => {
                    // Basic table search (reuse SQL logic)
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
                models::enums::DatabaseType::MongoDB => {
                    // Reuse SQL table cache search; collections are stored in table_cache with table_type='collection'
                    self.search_sql_tables(connection_id, search_text, &conn_type);
                }
                models::enums::DatabaseType::ApiHttp => {}
            }
        }
    }
    pub fn search_redis_keys(&mut self, connection_id: i64, search_text: &str) {
        // Search through Redis keys using SCAN with flexible pattern
        let rt = tokio::runtime::Runtime::new().unwrap();

        let search_results = rt.block_on(async {
            if let Some(models::enums::DatabasePool::Redis(redis_manager)) =
                connection::get_or_create_connection_pool(self, connection_id).await
            {
                let mut conn = redis_manager.as_ref().clone();

                // Use flexible pattern for LIKE search - search text can appear anywhere
                let pattern = format!("*{}*", search_text);
                let mut cursor = 0u64;
                let mut found_keys = Vec::new();

                // First try exact pattern match
                for _iteration in 0..20 {
                    // Increase iterations for more comprehensive search
                    let scan_result: Result<(u64, Vec<String>), _> = redis::cmd("SCAN")
                        .arg(cursor)
                        .arg("MATCH")
                        .arg(&pattern)
                        .arg("COUNT")
                        .arg(100) // Increase count for better performance
                        .query_async(&mut conn)
                        .await;

                    if let Ok((new_cursor, keys)) = scan_result {
                        // Additional filtering for case-sensitive LIKE search
                        for key in keys {
                            if key.contains(search_text) {
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

                found_keys
            } else {
                Vec::new()
            }
        });

        // Add search results to filtered tree
        if !search_results.is_empty() {
            // Find or create the connection node in filtered results
            let connection_name = self
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "Unknown Connection".to_string());

            let mut search_result_node = models::structs::TreeNode::new(
                format!(
                    "🔍 Search Results in {} ({} keys)",
                    connection_name,
                    search_results.len()
                ),
                models::enums::NodeType::CustomFolder,
            );
            search_result_node.connection_id = Some(connection_id);
            search_result_node.is_expanded = true;

            // Add found keys as children
            for key in search_results {
                let mut key_node =
                    models::structs::TreeNode::new(key.clone(), models::enums::NodeType::Table);
                key_node.connection_id = Some(connection_id);
                search_result_node.children.push(key_node);
            }

            self.filtered_items_tree.push(search_result_node);
        }
    }
    pub fn search_sql_tables(
        &mut self,
        connection_id: i64,
        search_text: &str,
        db_type: &models::enums::DatabaseType,
    ) {
        // Search through cached table data and column data
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let search_pattern = format!("*{}*", search_text); // Using GLOB pattern for case-sensitive search
            let rt = tokio::runtime::Runtime::new().unwrap();

            // Search tables
            let table_search_results = rt.block_on(async {
                let query = match db_type {
                    models::enums::DatabaseType::SQLite => {
                        "SELECT table_name, database_name, table_type FROM table_cache WHERE connection_id = ? AND table_name GLOB ? ORDER BY table_name"
                    }
                    _ => {
                        "SELECT table_name, database_name, table_type FROM table_cache WHERE connection_id = ? AND table_name LIKE ? COLLATE BINARY ORDER BY database_name, table_name"
                    }
                };

                let search_param = match db_type {
                    models::enums::DatabaseType::SQLite => &search_pattern,
                    _ => &format!("%{}%", search_text), // For non-SQLite, use LIKE with COLLATE BINARY for case sensitivity
                };

                sqlx::query_as::<_, (String, String, String)>(query)
                    .bind(connection_id)
                    .bind(search_param)
                    .fetch_all(pool_clone.as_ref())
                    .await
                    .unwrap_or_default()
            });

            // Search columns
            let column_search_results = rt.block_on(async {
                let query = match db_type {
                    models::enums::DatabaseType::SQLite => {
                        "SELECT DISTINCT table_name, database_name, column_name, data_type FROM column_cache WHERE connection_id = ? AND column_name GLOB ? ORDER BY table_name"
                    }
                    _ => {
                        "SELECT DISTINCT table_name, database_name, column_name, data_type FROM column_cache WHERE connection_id = ? AND column_name LIKE ? COLLATE BINARY ORDER BY database_name, table_name"
                    }
                };

                let search_param = match db_type {
                    models::enums::DatabaseType::SQLite => &search_pattern,
                    _ => &format!("%{}%", search_text), // For non-SQLite, use LIKE with COLLATE BINARY for case sensitivity
                };

                sqlx::query_as::<_, (String, String, String, String)>(query)
                    .bind(connection_id)
                    .bind(search_param)
                    .fetch_all(pool_clone.as_ref())
                    .await
                    .unwrap_or_default()
            });

            // Group table results by database
            let mut table_results_by_db: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            for (table_name, database_name, _table_type) in table_search_results {
                table_results_by_db
                    .entry(database_name)
                    .or_default()
                    .push(table_name);
            }

            // Group column results by database and table
            let mut column_results_by_db: std::collections::HashMap<
                String,
                std::collections::HashMap<String, Vec<(String, String)>>,
            > = std::collections::HashMap::new();
            for (table_name, database_name, column_name, data_type) in column_search_results {
                column_results_by_db
                    .entry(database_name)
                    .or_default()
                    .entry(table_name)
                    .or_default()
                    .push((column_name, data_type));
            }

            // Add search results to filtered tree
            if !table_results_by_db.is_empty() || !column_results_by_db.is_empty() {
                let connection_name = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "Unknown Connection".to_string());

                let total_tables: usize = table_results_by_db.values().map(|v| v.len()).sum();
                let total_columns: usize = column_results_by_db
                    .values()
                    .flat_map(|db| db.values())
                    .map(|cols| cols.len())
                    .sum();

                let mut search_result_node = models::structs::TreeNode::new(
                    format!(
                        "🔍 Search Results in {} ({} tables, {} columns)",
                        connection_name, total_tables, total_columns
                    ),
                    models::enums::NodeType::CustomFolder,
                );
                search_result_node.connection_id = Some(connection_id);
                search_result_node.is_expanded = true;

                // Combine all databases from both searches
                let mut all_databases: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                all_databases.extend(table_results_by_db.keys().cloned());
                all_databases.extend(column_results_by_db.keys().cloned());

                // Add databases and their tables/columns
                for database_name in all_databases {
                    let tables = table_results_by_db
                        .get(&database_name)
                        .cloned()
                        .unwrap_or_default();
                    let column_tables = column_results_by_db
                        .get(&database_name)
                        .cloned()
                        .unwrap_or_default();

                    let mut db_node = models::structs::TreeNode::new(
                        format!(
                            "📁 {} ({} tables, {} column matches)",
                            database_name,
                            tables.len(),
                            column_tables.values().map(|cols| cols.len()).sum::<usize>()
                        ),
                        models::enums::NodeType::Database,
                    );
                    db_node.connection_id = Some(connection_id);
                    db_node.database_name = Some(database_name.clone());
                    db_node.is_expanded = true;

                    // Add tables found by table name search
                    for table_name in tables {
                        let mut table_node = models::structs::TreeNode::new(
                            format!("📋 {} (table name match)", table_name),
                            models::enums::NodeType::Table,
                        );
                        table_node.connection_id = Some(connection_id);
                        table_node.database_name = Some(database_name.clone());
                        // Store the actual table name without icon for query generation
                        table_node.table_name = Some(table_name);
                        db_node.children.push(table_node);
                    }

                    // Add tables found by column name search
                    for (table_name, columns) in column_tables {
                        let mut table_node = models::structs::TreeNode::new(
                            format!("📋 {} ({} column matches)", table_name, columns.len()),
                            models::enums::NodeType::Table,
                        );
                        table_node.connection_id = Some(connection_id);
                        table_node.database_name = Some(database_name.clone());
                        // Store the actual table name without icon for query generation
                        table_node.table_name = Some(table_name.clone());

                        // Add matching columns as children
                        for (column_name, data_type) in columns {
                            let mut column_node = models::structs::TreeNode::new(
                                format!("🔧 {} ({})", column_name, data_type),
                                models::enums::NodeType::Column,
                            );
                            column_node.connection_id = Some(connection_id);
                            column_node.database_name = Some(database_name.clone());
                            // For columns, we can store the table name in table_name field
                            // The actual column name is already in the display name without icon
                            column_node.table_name = Some(table_name.clone());
                            table_node.children.push(column_node);
                        }

                        db_node.children.push(table_node);
                    }

                    search_result_node.children.push(db_node);
                }

                self.filtered_items_tree.push(search_result_node);
            }
        }
    }
    pub fn find_redis_key_info(
        node: &models::structs::TreeNode,
        key_name: &str,
    ) -> Option<(String, String)> {
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
                debug!(
                    "🔍 Checking child: '{}' (type: {:?})",
                    child.name, child.node_type
                );
                if child.node_type == models::enums::NodeType::Table
                    && child.name == key_name
                    && let Some(db_name) = &child.database_name
                {
                    return Some((db_name.clone(), folder_type.to_string()));
                }
            }
        }

        // Recursively search in children
        for child in &node.children {
            if let Some((db_name, key_type)) = Self::find_redis_key_info(child, key_name) {
                return Some((db_name, key_type));
            }
        }

        None
    }
    pub fn find_database_name_for_table(
        node: &models::structs::TreeNode,
        connection_id: i64,
        table_name: &str,
    ) -> Option<String> {
        // Look for the table in the tree structure to find its database context

        // Check if this node is a table with the matching name and connection
        // Use table_name field if available (for search results), otherwise use node.name
        let actual_table_name = node.table_name.as_ref().unwrap_or(&node.name);
        if (node.node_type == models::enums::NodeType::Table
            || node.node_type == models::enums::NodeType::View)
            && actual_table_name == table_name
            && node.connection_id == Some(connection_id)
        {
            return node.database_name.clone();
        }

        // Recursively search in children
        for child in &node.children {
            if let Some(db_name) =
                Self::find_database_name_for_table(child, connection_id, table_name)
            {
                return Some(db_name);
            }
        }

        None
    }

    pub fn highlight_sql_syntax(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
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
            "SELECT",
            "FROM",
            "WHERE",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "DROP",
            "ALTER",
            "TABLE",
            "INDEX",
            "VIEW",
            "TRIGGER",
            "PROCEDURE",
            "FUNCTION",
            "JOIN",
            "INNER",
            "LEFT",
            "RIGHT",
            "OUTER",
            "ON",
            "AS",
            "AND",
            "OR",
            "NOT",
            "NULL",
            "TRUE",
            "FALSE",
            "CASE",
            "WHEN",
            "THEN",
            "ELSE",
            "END",
            "IF",
            "EXISTS",
            "IN",
            "LIKE",
            "BETWEEN",
            "GROUP BY",
            "ORDER BY",
            "HAVING",
            "LIMIT",
            "OFFSET",
            "UNION",
            "ALL",
            "DISTINCT",
            "COUNT",
            "SUM",
            "AVG",
            "MIN",
            "MAX",
            "ASC",
            "DESC",
            "PRIMARY",
            "KEY",
            "FOREIGN",
            "REFERENCES",
            "CONSTRAINT",
            "UNIQUE",
            "DEFAULT",
            "AUTO_INCREMENT",
            "SERIAL",
            "INT",
            "INTEGER",
            "VARCHAR",
            "TEXT",
            "CHAR",
            "DECIMAL",
            "FLOAT",
            "DOUBLE",
            "DATE",
            "TIME",
            "DATETIME",
            "TIMESTAMP",
            "BOOLEAN",
            "BOOL",
            "USE",
        ];

        // Define colors for different themes
        let text_color = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(220, 220, 220) // Light text for dark mode
        } else {
            egui::Color32::from_rgb(40, 40, 40) // Dark text for light mode
        };

        let keyword_color = egui::Color32::from_rgb(86, 156, 214); // Blue - SQL keywords
        let string_color = egui::Color32::from_rgb(255, 0, 0); // Orange - strings
        let comment_color = egui::Color32::from_rgb(106, 153, 85); // Green - comments
        let number_color = egui::Color32::from_rgb(181, 206, 168); // Light green - numbers
        let function_color = egui::Color32::from_rgb(255, 0, 0); // Red - functions
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
                        let absolute_word_start =
                            line_start_offset + word_search_start + word_start_in_line;
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
                        } else if word.chars().all(|c| c.is_ascii_digit() || c == '.')
                            && !word.is_empty()
                        {
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
}
