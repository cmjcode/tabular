use eframe::egui;
use log::debug;

use crate::{directory, editor, models, sidebar_query, window_egui};


 pub(crate) fn load_queries_from_directory(tabular: &mut window_egui::Tabular) {
        tabular.queries_tree.clear();

        let query_dir = directory::get_query_dir();
        tabular.queries_tree = directory::load_directory_recursive(&query_dir);
        
        // Sort folders and files alphabetically
        tabular.queries_tree.sort_by(|a, b| {
            match (&a.node_type, &b.node_type) {
                (models::enums::NodeType::QueryFolder, models::enums::NodeType::Query) => std::cmp::Ordering::Less, // Folders first
                (models::enums::NodeType::Query, models::enums::NodeType::QueryFolder) => std::cmp::Ordering::Greater, // Files after folders
                _ => a.name.cmp(&b.name), // Alphabetical within same type
            }
        });
    }

 pub(crate) fn create_query_folder(tabular: &mut window_egui::Tabular, folder_name: &str) -> Result<(), String> {
        if folder_name.trim().is_empty() {
            return Err("Folder name cannot be empty".to_string());
        }

        let query_dir = directory::get_query_dir();
        let folder_path = query_dir.join(folder_name);
        
        if folder_path.exists() {
            return Err("Folder already exists".to_string());
        }
        
        std::fs::create_dir_all(&folder_path)
            .map_err(|e| format!("Failed to create folder: {}", e))?;
            
        // Refresh the queries tree
        load_queries_from_directory(tabular);
        
        Ok(())
    }

 pub(crate) fn create_query_folder_in_parent(tabular: &mut window_egui::Tabular, folder_name: &str, parent_folder: &str) -> Result<(), String> {
        if folder_name.trim().is_empty() {
            return Err("Folder name cannot be empty".to_string());
        }

        let query_dir = directory::get_query_dir();
        let parent_path = query_dir.join(parent_folder);
        
        if !parent_path.exists() || !parent_path.is_dir() {
            return Err(format!("Parent folder '{}' does not exist", parent_folder));
        }
        
        let folder_path = parent_path.join(folder_name);
        
        if folder_path.exists() {
            return Err(format!("Folder '{}' already exists in '{}'", folder_name, parent_folder));
        }
        
        std::fs::create_dir_all(&folder_path)
            .map_err(|e| format!("Failed to create folder: {}", e))?;
            
        // Refresh the queries tree
        load_queries_from_directory(tabular);
        
        Ok(())
    }

 pub(crate) fn move_query_to_folder(tabular: &mut window_egui::Tabular, query_file_path: &str, target_folder: &str) -> Result<(), String> {
        let source_path = std::path::Path::new(query_file_path);
        let file_name = source_path.file_name()
            .ok_or("Invalid file path")?;
            
        let query_dir = directory::get_query_dir();
        let target_folder_path = query_dir.join(target_folder);
        let target_file_path = target_folder_path.join(file_name);
        
        // Create target folder if it doesn't exist
        std::fs::create_dir_all(&target_folder_path)
            .map_err(|e| format!("Failed to create target folder: {}", e))?;
            
        // Move the file
        std::fs::rename(source_path, &target_file_path)
            .map_err(|e| format!("Failed to move file: {}", e))?;
            
        // Close any open tabs for this file and update with new path
        editor::close_tabs_for_file(tabular, query_file_path);
        
        // Refresh the queries tree
        load_queries_from_directory(tabular);
        
        Ok(())
    }

 pub(crate) fn move_query_to_root(tabular: &mut window_egui::Tabular, query_file_path: &str) -> Result<(), String> {
        let source_path = std::path::Path::new(query_file_path);
        let file_name = source_path.file_name()
            .ok_or("Invalid file path")?;
            
        let query_dir = directory::get_query_dir();
        let target_file_path = query_dir.join(file_name);
        
        // Move the file to root
        std::fs::rename(source_path, &target_file_path)
            .map_err(|e| format!("Failed to move file: {}", e))?;
            
        // Close any open tabs for this file and update with new path
        editor::close_tabs_for_file(tabular, query_file_path);
        
        // Refresh the queries tree
        load_queries_from_directory(tabular);
        
        Ok(())
    }




pub(crate)fn render_create_folder_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        if tabular.show_create_folder_dialog {
            let window_title = if let Some(ref parent) = tabular.parent_folder_for_creation {
                format!("Create Folder in '{}'", parent)
            } else {
                "Create Query Folder".to_string()
            };
            
            egui::Window::new(window_title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if let Some(ref parent) = tabular.parent_folder_for_creation {
                        ui.label(format!("Creating folder inside: {}", parent));
                        ui.separator();
                    }
                    
                    ui.label("Folder name:");
                    ui.text_edit_singleline(&mut tabular.new_folder_name);
                    ui.separator();
                    
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            let folder_name = tabular.new_folder_name.clone();
                            let parent_folder = tabular.parent_folder_for_creation.clone();
                            
                            let result = if let Some(parent) = parent_folder {
                                sidebar_query::create_query_folder_in_parent(tabular, &folder_name, &parent)
                            } else {
                                sidebar_query::create_query_folder(tabular, &folder_name)
                            };
                            
                            if let Err(err) = result {
                                tabular.error_message = err;
                                tabular.show_error_message = true;
                            } else {
                                // Force immediate UI repaint after successful folder creation
                                ui.ctx().request_repaint();
                            }
                            
                            tabular.show_create_folder_dialog = false;
                            tabular.new_folder_name.clear();
                            tabular.parent_folder_for_creation = None;
                        }
                        
                        if ui.button("Cancel").clicked() {
                            tabular.show_create_folder_dialog = false;
                            tabular.new_folder_name.clear();
                            tabular.parent_folder_for_creation = None;
                        }
                    });
                });
        }
    }

pub(crate)fn render_move_to_folder_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        if tabular.show_move_to_folder_dialog {
            egui::Window::new("Move Query to Folder")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    if let Some(query_path) = &tabular.selected_query_for_move {
                        let file_name = std::path::Path::new(query_path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Unknown");
                        ui.label(format!("Moving: {}", file_name));
                        ui.separator();
                    }
                    
                    ui.label("Target folder:");
                    ui.text_edit_singleline(&mut tabular.target_folder_name);
                    ui.small("(Leave empty to move to root, or enter folder name)");
                    ui.separator();
                    
                    ui.horizontal(|ui| {
                        if ui.button("Move").clicked() {
                            if let Some(query_path) = tabular.selected_query_for_move.clone() {
                                if tabular.target_folder_name.trim().is_empty() {
                                    // Move to root
                                    if let Err(err) = sidebar_query::move_query_to_root(tabular, &query_path) {
                                        tabular.error_message = err;
                                        tabular.show_error_message = true;
                                    }
                                } else if let Err(err) = sidebar_query::move_query_to_folder(tabular, &query_path, &tabular.target_folder_name.clone()) {
                                    tabular.error_message = err;
                                    tabular.show_error_message = true;
                                }
                            }
                            tabular.show_move_to_folder_dialog = false;
                            tabular.selected_query_for_move = None;
                            tabular.target_folder_name.clear();
                        }
                        
                        if ui.button("Cancel").clicked() {
                            tabular.show_move_to_folder_dialog = false;
                            tabular.selected_query_for_move = None;
                            tabular.target_folder_name.clear();
                        }
                    });
                });
        }
    }

pub(crate)fn open_query_file(tabular: &mut window_egui::Tabular, file_path: &str) -> Result<(), String> {
        let content = std::fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file: {}", e))?;
        
        let filename = std::path::Path::new(file_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown")
            .to_string();
        
        // Check if file is already open
        for (index, tab) in tabular.query_tabs.iter().enumerate() {
            if tab.file_path.as_deref() == Some(file_path) {
                editor::switch_to_tab(tabular, index);
                return Ok(());
            }
        }
        
        // Create new tab for the file
        let new_tab = models::structs::QueryTab {
            title: filename,
            content: content.clone(),
            file_path: Some(file_path.to_string()),
            is_saved: true,
            is_modified: false,
            connection_id: None, // File queries don't have connection by default
            database_name: None, // File queries don't have database by default
            has_executed_query: false, // New tab hasn't executed any query yet
            result_headers: Vec::new(),
            result_rows: Vec::new(),
            result_all_rows: Vec::new(),
            result_table_name: String::new(),
            is_table_browse_mode: false,
            current_page: 0,
            page_size: 0,
            total_rows: 0,
            base_query: String::new(), // Empty base query for file queries
        };
        
        tabular.query_tabs.push(new_tab);
        let new_index = tabular.query_tabs.len() - 1;
        tabular.active_tab_index = new_index;
        tabular.editor_text = content;
        
        Ok(())
    }



pub(crate) fn handle_query_edit_request(tabular: &mut window_egui::Tabular, hash: i64) {
        
        // Find the query file by hash
        if let Some(query_file_path) = find_query_file_by_hash(hash) {
            
            // Open the query file in a new tab for editing
            if let Err(err) = sidebar_query::open_query_file(tabular, &query_file_path) {
                debug!("Failed to open query file for editing: {}", err);
            }
        } else {
            debug!("Query file not found for hash: {}", hash);
        }
    }

pub(crate) fn handle_query_move_request(tabular: &mut window_egui::Tabular, hash: i64) {
        
        // Find the query file by hash
        if let Some(query_file_path) = find_query_file_by_hash(hash) {
            
            // Set up the move dialog
            tabular.selected_query_for_move = Some(query_file_path);
            tabular.show_move_to_folder_dialog = true;
        } else {
            debug!("Query file not found for hash: {}", hash);
        }
    }

pub(crate) fn handle_query_remove_request_by_hash(tabular: &mut window_egui::Tabular, hash: i64) -> bool {
        
        // Find the query file by hash using recursive search
        if let Some(file_path) = find_query_file_by_hash(hash) {
            
            // Close any open tabs for this file first
            editor::close_tabs_for_file(tabular, &file_path);
            
            // Remove the file from filesystem
            match std::fs::remove_file(&file_path) {
                Ok(()) => {
                    
                    // Set needs_refresh flag for next update cycle
                    tabular.needs_refresh = true;
                    
                    return true;
                },
                Err(e) => {
                    debug!("❌ Failed to remove query file: {}", e);
                    return false;
                }
            }
        }
        
        debug!("❌ Query file not found for hash: {}", hash);
        false
    }


pub(crate) fn find_query_file_by_hash(hash: i64) -> Option<String> {
        let query_dir = directory::get_query_dir();
        
        // Function to search recursively in directories
        fn search_in_dir(dir: &std::path::Path, target_hash: i64) -> Option<String> {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() {
                            if let Some(filename) = entry.file_name().to_str()
                                && filename.ends_with(".sql") {
                                    let file_path = entry.path().to_string_lossy().to_string();
                                    
                                    // Use same hash calculation as in context menu: file_path.len() % 1000
                                    let file_hash = (file_path.len() as i64) % 1000;
                                    
                                    if file_hash == target_hash {
                                        return Some(file_path);
                                    }
                                }
                        } else if metadata.is_dir() {
                            // Recursively search in subdirectories
                            if let Some(found) = search_in_dir(&entry.path(), target_hash) {
                                return Some(found);
                            }
                        }
                    }
                }
            }
            None
        }
        
        search_in_dir(&query_dir, hash)
    }
