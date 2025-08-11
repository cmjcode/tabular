use eframe::egui;
use egui_code_editor::{CodeEditor, ColorTheme};
use log::{debug};

use crate::{connection, directory, editor, models, sidebar_history, sidebar_query, window_egui};


    // Tab management methods
 pub(crate) fn create_new_tab(tabular: &mut window_egui::Tabular, title: String, content: String) -> usize {
        let tab_id = tabular.next_tab_id;
        tabular.next_tab_id += 1;
        
        let new_tab = models::structs::QueryTab {
            title,
            content: content.clone(),
            file_path: None,
            is_saved: false,
            is_modified: false,
            connection_id: None, // No connection assigned by default
            database_name: None, // No database assigned by default
            has_executed_query: false, // New tab hasn't executed any query yet
        };
        
        tabular.query_tabs.push(new_tab);
        let new_index = tabular.query_tabs.len() - 1;
        tabular.active_tab_index = new_index;
        
        // Update editor with new tab content
        tabular.editor_text = content;
        
        tab_id
    }

 pub(crate) fn create_new_tab_with_connection(tabular: &mut window_egui::Tabular, title: String, content: String, connection_id: Option<i64>) -> usize {
        create_new_tab_with_connection_and_database(tabular, title, content, connection_id, None)
    }
    
 pub(crate) fn create_new_tab_with_connection_and_database(tabular: &mut window_egui::Tabular, title: String, content: String, connection_id: Option<i64>, database_name: Option<String>) -> usize {
        let tab_id = tabular.next_tab_id;
        tabular.next_tab_id += 1;
        
        let new_tab = models::structs::QueryTab {
            title,
            content: content.clone(),
            file_path: None,
            is_saved: false,
            is_modified: false,
            connection_id,
            database_name,
            has_executed_query: false, // New tab hasn't executed any query yet
        };
        
        tabular.query_tabs.push(new_tab);
        let new_index = tabular.query_tabs.len() - 1;
        tabular.active_tab_index = new_index;
        
        // Update editor with new tab content
        tabular.editor_text = content;
        
        tab_id
    }

 pub(crate) fn close_tab(tabular: &mut window_egui::Tabular, tab_index: usize) {
        if tabular.query_tabs.len() <= 1 {
            // Don't close the last tab, just clear it
            if let Some(tab) = tabular.query_tabs.get_mut(0) {
                tab.content.clear();
                tab.title = "Untitled Query".to_string();
                tab.file_path = None;
                tab.is_saved = false;
                tab.is_modified = false;
                tab.connection_id = None; // Clear connection as well
                tab.database_name = None; // Clear database as well
            }
            tabular.editor_text.clear();
            return;
        }

        if tab_index < tabular.query_tabs.len() {
            tabular.query_tabs.remove(tab_index);
            
            // Adjust active tab index
            if tabular.active_tab_index >= tabular.query_tabs.len() {
                tabular.active_tab_index = tabular.query_tabs.len() - 1;
            } else if tabular.active_tab_index > tab_index {
                tabular.active_tab_index -= 1;
            }
            
            // Update editor with active tab content
            if let Some(active_tab) = tabular.query_tabs.get(tabular.active_tab_index) {
                tabular.editor_text = active_tab.content.clone();
            }
        }
    }

 pub(crate) fn switch_to_tab(tabular: &mut window_egui::Tabular, tab_index: usize) {
        if tab_index < tabular.query_tabs.len() {
            // Save current tab content
            if let Some(current_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                if current_tab.content != tabular.editor_text {
                    current_tab.content = tabular.editor_text.clone();
                    current_tab.is_modified = true;
                }
            }
            
            // Switch to new tab
            tabular.active_tab_index = tab_index;
            if let Some(new_tab) = tabular.query_tabs.get(tab_index) {
                tabular.editor_text = new_tab.content.clone();
            }
        }
    }

 pub(crate) fn save_current_tab(tabular: &mut window_egui::Tabular) -> Result<(), String> {
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor_text.clone();
            
            if tab.file_path.is_some() {
                // File already exists, save directly
                let file_path = tab.file_path.as_ref().unwrap().clone();
                std::fs::write(&file_path, &tab.content)
                    .map_err(|e| format!("Failed to save file: {}", e))?;
                
                tab.is_saved = true;
                tab.is_modified = false;
                
                Ok(())
            } else {
                // Show save dialog for new file
                tabular.save_filename = tab.title.replace("Untitled Query", "").trim().to_string();
                if tabular.save_filename.is_empty() {
                    tabular.save_filename = "new_query".to_string();
                }
                if !tabular.save_filename.ends_with(".sql") {
                    tabular.save_filename.push_str(".sql");
                }
                tabular.show_save_dialog = true;
                Ok(())
            }
        } else {
            Err("No active tab".to_string())
        }
    }

 pub(crate) fn save_current_tab_with_name(tabular: &mut window_egui::Tabular, filename: String) -> Result<(), String> {
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            // Get query directory and ensure it exists
            let query_dir = directory::get_query_dir();
            std::fs::create_dir_all(&query_dir).map_err(|e| format!("Failed to create query directory: {}", e))?;
            
            let mut clean_filename = filename.trim().to_string();
            if !clean_filename.ends_with(".sql") {
                clean_filename.push_str(".sql");
            }
            
            let file_path = query_dir.join(&clean_filename);
            
            std::fs::write(&file_path, &tab.content)
                .map_err(|e| format!("Failed to save file: {}", e))?;
            
            tab.file_path = Some(file_path.to_string_lossy().to_string());
            tab.title = clean_filename;
            tab.is_saved = true;
            tab.is_modified = false;
            
            // Refresh queries tree to show the new file
            sidebar_query::load_queries_from_directory(tabular);
            
            Ok(())
        } else {
            Err("No active tab".to_string())
        }
    }




pub(crate) fn render_advanced_editor(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
        // Find & Replace panel
        if tabular.advanced_editor.show_find_replace {
            ui.horizontal(|ui| {
                ui.label("Find:");
                ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut tabular.advanced_editor.find_text));
                
                ui.label("Replace:");
                ui.add_sized([200.0, 20.0], egui::TextEdit::singleline(&mut tabular.advanced_editor.replace_text));
                
                ui.checkbox(&mut tabular.advanced_editor.case_sensitive, "Case Sensitive");
                ui.checkbox(&mut tabular.advanced_editor.use_regex, "Regex");
                
                if ui.button("Replace All").clicked() {
                    perform_replace_all(tabular);
                }
                
                if ui.button("Find Next").clicked() {
                    find_next(tabular);
                }
                
                if ui.button("✕").clicked() {
                    tabular.advanced_editor.show_find_replace = false;
                }
            });
        }

        // Main code editor using egui_code_editor
        let mut editor = CodeEditor::default()
            .id_source("sql_editor")
            .with_rows(25)
            .with_fontsize(tabular.advanced_editor.font_size)
            .with_theme(tabular.advanced_editor.theme)
            .with_syntax(egui_code_editor::Syntax::sql())
            .with_numlines(tabular.advanced_editor.show_line_numbers);

        let response = editor.show(ui, &mut tabular.editor_text);
        
        // Try to capture selected text from the response
        // Note: This is a simplified approach. The actual implementation may vary depending on the CodeEditor version
        if let Some(text_cursor_range) = response.cursor_range {
            let start = text_cursor_range.primary.ccursor.index.min(text_cursor_range.secondary.ccursor.index);
            let end = text_cursor_range.primary.ccursor.index.max(text_cursor_range.secondary.ccursor.index);
            
            // Store cursor position (use primary cursor position)
            tabular.cursor_position = text_cursor_range.primary.ccursor.index;
            
            if start != end {
                // There is a selection
                if let Some(selected) = tabular.editor_text.get(start..end) {
                    tabular.selected_text = selected.to_string();
                } else {
                    tabular.selected_text.clear();
                }
            } else {
                // No selection
                tabular.selected_text.clear();
            }
        } else {
            // No cursor range available, clear selection
            tabular.selected_text.clear();
        }
        
        // If you get a type error here, try:
        // let mut buffer = egui_code_editor::SimpleTextBuffer::from(&tabular.editor_text);
        // let response = editor.show(ui, &mut buffer);
        // tabular.editor_text = buffer.text().to_string();
        
        // Update tab content when editor changes
        if response.response.changed() {
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.content = tabular.editor_text.clone();
                tab.is_modified = true;
            }
        }
    }

pub(crate) fn perform_replace_all(tabular: &mut window_egui::Tabular) {
        if tabular.advanced_editor.find_text.is_empty() {
            return;
        }

        let find_text = &tabular.advanced_editor.find_text;
        let replace_text = &tabular.advanced_editor.replace_text;

        if tabular.advanced_editor.use_regex {
            // Use regex replacement
            if let Ok(re) = regex::Regex::new(find_text) {
                tabular.editor_text = re.replace_all(&tabular.editor_text, replace_text).into_owned();
            }
        } else {
            // Simple string replacement
            if tabular.advanced_editor.case_sensitive {
                tabular.editor_text = tabular.editor_text.replace(find_text, replace_text);
            } else {
                // Case insensitive replacement
                let find_lower = find_text.to_lowercase();
                let mut result = String::new();
                let mut last_end = 0;
                
                for (start, part) in tabular.editor_text.match_indices(&find_lower) {
                    result.push_str(&tabular.editor_text[last_end..start]);
                    result.push_str(replace_text);
                    last_end = start + part.len();
                }
                result.push_str(&tabular.editor_text[last_end..]);
                tabular.editor_text = result;
            }
        }

        // Update current tab content
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor_text.clone();
            tab.is_modified = true;
        }
    }

pub(crate) fn find_next(tabular: &mut window_egui::Tabular) {
        // This is a simplified find implementation
        // In a real implementation, you'd want to track cursor position and highlight matches
        if !tabular.advanced_editor.find_text.is_empty() {
            if let Some(_pos) = tabular.editor_text.find(&tabular.advanced_editor.find_text) {
                // In a full implementation, you would scroll to and highlight the match
                debug!("Found match for: {}", tabular.advanced_editor.find_text);
            }
        }
    }

pub(crate) fn open_command_palette(tabular: &mut window_egui::Tabular) {
        tabular.show_command_palette = true;
        tabular.command_palette_input.clear();
        tabular.show_theme_selector = false;
        tabular.command_palette_selected_index = 0;
        
        // Initialize command palette items
        tabular.command_palette_items = vec![
            "Preferences: Color Theme".to_string(),
            "View: Toggle Word Wrap".to_string(),
            "View: Toggle Line Numbers".to_string(),
            "View: Toggle Find and Replace".to_string(),
        ];
    }

pub(crate) fn navigate_command_palette(tabular: &mut window_egui::Tabular, direction: i32) {
        // Filter commands based on current input
        let filtered_commands: Vec<String> = if tabular.command_palette_input.is_empty() {
            tabular.command_palette_items.clone()
        } else {
            tabular.command_palette_items
                .iter()
                .filter(|cmd| cmd.to_lowercase().contains(&tabular.command_palette_input.to_lowercase()))
                .cloned()
                .collect()
        };

        if filtered_commands.is_empty() {
            return;
        }

        // Update selected index with wrapping
        if direction > 0 {
            // Down arrow
            tabular.command_palette_selected_index = (tabular.command_palette_selected_index + 1) % filtered_commands.len();
        } else {
            // Up arrow
            if tabular.command_palette_selected_index == 0 {
                tabular.command_palette_selected_index = filtered_commands.len() - 1;
            } else {
                tabular.command_palette_selected_index -= 1;
            }
        }
    }

pub(crate) fn execute_selected_command(tabular: &mut window_egui::Tabular) {
        // Filter commands based on current input
        let filtered_commands: Vec<String> = if tabular.command_palette_input.is_empty() {
            tabular.command_palette_items.clone()
        } else {
            tabular.command_palette_items
                .iter()
                .filter(|cmd| cmd.to_lowercase().contains(&tabular.command_palette_input.to_lowercase()))
                .cloned()
                .collect()
        };

        if tabular.command_palette_selected_index < filtered_commands.len() {
            let selected_command = filtered_commands[tabular.command_palette_selected_index].clone();
            execute_command(tabular, &selected_command);
        }
    }

pub(crate) fn navigate_theme_selector(tabular: &mut window_egui::Tabular, direction: i32) {
        // There are 3 themes available
        let theme_count = 3;

        // Update selected index with wrapping
        if direction > 0 {
            // Down arrow
            tabular.theme_selector_selected_index = (tabular.theme_selector_selected_index + 1) % theme_count;
        } else {
            // Up arrow
            if tabular.theme_selector_selected_index == 0 {
                tabular.theme_selector_selected_index = theme_count - 1;
            } else {
                tabular.theme_selector_selected_index -= 1;
            }
        }
    }

pub(crate) fn select_current_theme(tabular: &mut window_egui::Tabular) {
        // Map index to theme
        let theme = match tabular.theme_selector_selected_index {
            0 => ColorTheme::GITHUB_DARK,
            1 => ColorTheme::GITHUB_LIGHT,
            2 => ColorTheme::GRUVBOX,
            _ => ColorTheme::GITHUB_DARK, // fallback
        };

        tabular.advanced_editor.theme = theme;
        tabular.show_theme_selector = false;
    }

pub(crate) fn render_command_palette(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        // Create a centered modal dialog
        egui::Area::new(egui::Id::new("command_palette"))
            .fixed_pos(egui::pos2(
                ctx.screen_rect().center().x - 300.0,
                ctx.screen_rect().center().y - 200.0,
            ))
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(ui.style().visuals.window_fill)
                    .stroke(ui.style().visuals.window_stroke)
                    .shadow(egui::epaint::Shadow::default())
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        
                        ui.vertical(|ui| {                            
                            // Search input
                            let response = ui.add_sized(
                                [580.0, 25.0],
                                egui::TextEdit::singleline(&mut tabular.command_palette_input)
                                    .hint_text("Type command name...")
                            );
                            
                            // Reset selection when text changes
                            if response.changed() {
                                tabular.command_palette_selected_index = 0;
                            }
                            
                            // Auto-focus the input when palette opens
                            if tabular.command_palette_input.is_empty() {
                                response.request_focus();
                            }
                            
                            ui.separator();
                            
                            // Filter commands based on input
                            let filtered_commands: Vec<String> = if tabular.command_palette_input.is_empty() {
                                tabular.command_palette_items.clone()
                            } else {
                                tabular.command_palette_items
                                    .iter()
                                    .filter(|cmd| cmd.to_lowercase().contains(&tabular.command_palette_input.to_lowercase()))
                                    .cloned()
                                    .collect()
                            };

                            // Ensure selected index is within bounds when filtering
                            if tabular.command_palette_selected_index >= filtered_commands.len() && !filtered_commands.is_empty() {
                                tabular.command_palette_selected_index = 0;
                            }
                            
                            // Command list
                            egui::ScrollArea::vertical()
                                .max_height(500.0)
                                .show(ui, |ui| {
                                    for (index, command) in filtered_commands.iter().enumerate() {
                                        let is_selected = index == tabular.command_palette_selected_index;
                                        
                                        // Highlight selected item
                                        let text = if is_selected {
                                            egui::RichText::new(command)
                                                .background_color(ui.style().visuals.selection.bg_fill)
                                                .color(ui.style().visuals.selection.stroke.color)
                                        } else {
                                            egui::RichText::new(command)
                                        };
                                        
                                        if ui.selectable_label(is_selected, text).clicked() {
                                            execute_command(tabular, command);
                                        }
                                    }
                                });
                            
                        });
                    });
            });
    }

pub(crate) fn execute_command(tabular: &mut window_egui::Tabular, command: &str) {
        match command {
            "Preferences: Color Theme" => {
                tabular.show_command_palette = false;
                // Instead of directly setting show_theme_selector, use a flag
                tabular.request_theme_selector = true;
                tabular.theme_selector_selected_index = 0; // Reset to first theme
            }
            "View: Toggle Word Wrap" => {
                tabular.advanced_editor.word_wrap = !tabular.advanced_editor.word_wrap;
                tabular.show_command_palette = false;
            }
            "View: Toggle Line Numbers" => {
                tabular.advanced_editor.show_line_numbers = !tabular.advanced_editor.show_line_numbers;
                tabular.show_command_palette = false;
            }
            "View: Toggle Find and Replace" => {
                tabular.advanced_editor.show_find_replace = !tabular.advanced_editor.show_find_replace;
                tabular.show_command_palette = false;
            }
            _ => {
                debug!("Unknown command: {}", command);
                tabular.show_command_palette = false;
            }
        }
    }

pub(crate) fn render_theme_selector(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
        // Create a centered modal dialog for theme selection
        egui::Area::new(egui::Id::new("theme_selector"))
            .fixed_pos(egui::pos2(
                ctx.screen_rect().center().x - 200.0,
                ctx.screen_rect().center().y - 150.0,
            ))
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(ui.style().visuals.window_fill)
                    .stroke(ui.style().visuals.window_stroke)
                    .shadow(egui::epaint::Shadow::default())
                    .inner_margin(egui::Margin::same(15))
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(400.0, 300.0));
                        
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Select Color Theme").heading());
                            ui.separator();
                            
                            ui.spacing_mut().item_spacing.y = 8.0;
                            
                            // Available themes with descriptions
                            let themes = vec![
                                (ColorTheme::GITHUB_DARK, "GitHub Dark", "Dark theme with blue accents"),
                                (ColorTheme::GITHUB_LIGHT, "GitHub Light", "Light theme with subtle colors"),
                                (ColorTheme::GRUVBOX, "Gruvbox", "Retro warm theme with earthy colors"),
                            ];
                            
                            for (index, (theme, name, description)) in themes.iter().enumerate() {
                                let is_current = tabular.advanced_editor.theme == *theme;
                                let is_selected = index == tabular.theme_selector_selected_index;
                                
                                // Create horizontal layout for theme item
                                ui.horizontal(|ui| {
                                    // Current theme indicator (checkmark)
                                    if is_current {
                                        ui.label("✓");
                                    } else {
                                        ui.label(" "); // Space for alignment
                                    }
                                    
                                    // Theme name with different styling based on selection
                                    let text = if is_selected {
                                        // Highlight the selected item for keyboard navigation
                                        egui::RichText::new(*name)
                                            .size(16.0)
                                            .background_color(ui.style().visuals.selection.bg_fill)
                                            .color(ui.style().visuals.selection.stroke.color)
                                    } else if is_current {
                                        // Bold text for current theme
                                        egui::RichText::new(*name)
                                            .size(16.0)
                                            .strong()
                                            .color(egui::Color32::from_rgb(0, 150, 255)) // Blue for current
                                    } else {
                                        // Normal text for other themes
                                        egui::RichText::new(*name).size(16.0)
                                    };
                                    
                                    let response = ui.label(text);
                                    
                                    // Handle click to select theme
                                    if response.clicked() && !is_current {
                                        tabular.advanced_editor.theme = *theme;
                                        tabular.show_theme_selector = false;
                                    }
                                });
                                
                                // Show description with indentation
                                ui.horizontal(|ui| {
                                    ui.add_space(20.0); // Indent description
                                    ui.label(egui::RichText::new(*description).size(12.0).weak());
                                });
                                ui.add_space(5.0);
                            }
                            
                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label("Use");
                                ui.code("↑↓");
                                ui.label("to navigate,");
                                ui.code("Enter");
                                ui.label("to select,");
                                ui.code("Escape");
                                ui.label("to close");
                            });
                        });
                    });
            });
    }


pub(crate) fn execute_query(tabular: &mut window_egui::Tabular) {
        // Priority: 1) Selected text, 2) Query from cursor position, 3) Full editor text
        let query = if !tabular.selected_text.trim().is_empty() {
            tabular.selected_text.trim().to_string()
        } else {
            let cursor_query = extract_query_from_cursor(tabular);
            if !cursor_query.trim().is_empty() {
                cursor_query
            } else {
                tabular.editor_text.trim().to_string()
            }
        };
        
        if query.is_empty() {
            tabular.current_table_name = "No query to execute".to_string();
            tabular.current_table_headers.clear();
            tabular.current_table_data.clear();
            return;
        }

        // Check if current tab has never executed a query
        let tab_never_executed = if let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index) {
            !tab.has_executed_query
        } else {
            true // If no tab exists, treat as never executed
        };

        // Get connection_id from current active tab (don't fallback to global for first-time execution)
        let connection_id = if let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index) {
            if tab_never_executed {
                // For first-time execution, only use tab's own connection, don't inherit from other tabs
                tab.connection_id
            } else {
                // For subsequent executions, allow fallback to global
                tab.connection_id.or(tabular.current_connection_id)
            }
        } else {
            None
        };

        // If tab has never executed a query and has no connection, show connection selector
        if tab_never_executed && connection_id.is_none() {
            debug!("First-time query execution - showing connection selector");
            tabular.pending_query = query;
            tabular.auto_execute_after_connection = true;
            tabular.show_connection_selector = true;
            return;
        }

        // Check if we have an active connection
        if let Some(connection_id) = connection_id {
            debug!("=== EXECUTING QUERY ===");
            debug!("Connection ID: {}", connection_id);
            debug!("Query: {}", query);
            
            let result = connection::execute_query_with_connection(tabular, connection_id, query.clone());
            
            debug!("Query execution result: {:?}", result.is_some());
            
            // Mark tab as having executed a query (regardless of success/failure)
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.has_executed_query = true;
            }
            
            if let Some((headers, data)) = result {
                debug!("=== QUERY RESULT SUCCESS ===");
                debug!("Headers received: {} - {:?}", headers.len(), headers);
                debug!("Data rows received: {}", data.len());
                if !data.is_empty() {
                    debug!("First row sample: {:?}", &data[0]);
                }
                
                tabular.current_table_headers = headers;
                
                // Use pagination for query results
                tabular.update_pagination_data(data);
                
                if tabular.total_rows == 0 {
                    tabular.current_table_name = "Query executed successfully (no results)".to_string();
                } else {
                    tabular.current_table_name = format!("Query Results ({} total rows, showing page {} of {})", 
                        tabular.total_rows, tabular.current_page + 1, tabular.get_total_pages());
                }
                debug!("After update_pagination_data - total_rows: {}, all_table_data.len(): {}", 
                         tabular.total_rows, tabular.all_table_data.len());
                debug!("============================");
                
                // Save query to history after successful execution
                sidebar_history::save_query_to_history(tabular, &query, connection_id);
            } else {
                tabular.current_table_name = "Query execution failed".to_string();
                tabular.current_table_headers.clear();
                tabular.current_table_data.clear();
                tabular.all_table_data.clear();
                tabular.total_rows = 0;
            }
        } else {
            // No active connection - check if we have any connections available
            if tabular.connections.is_empty() {
                tabular.current_table_name = "No connections available. Please add a connection first.".to_string();
                tabular.current_table_headers.clear();
                tabular.current_table_data.clear();
                tabular.all_table_data.clear();
                tabular.total_rows = 0;
            } else {
                // For tabs that have executed before but lost connection, show connection selector
                debug!("No connection available for query execution - showing connection selector");
                tabular.pending_query = query.clone();
                tabular.show_connection_selector = true;
                tabular.auto_execute_after_connection = true;
            }
        }
    }


pub(crate)fn extract_query_from_cursor(tabular: &mut window_egui::Tabular) -> String {
        if tabular.editor_text.is_empty() {
            return String::new();
        }

        let text_bytes = tabular.editor_text.as_bytes();
        let cursor_pos = tabular.cursor_position.min(text_bytes.len());

        // Find start position: go backwards from cursor to find the last semicolon (or start of file)
        let mut start_pos = 0;
        for i in (0..cursor_pos).rev() {
            if text_bytes[i] == b';' {
                // Start after the semicolon
                start_pos = i + 1;
                break;
            }
        }

        // Find end position: go forwards from cursor to find the next semicolon (or end of file)
        let mut end_pos = text_bytes.len();
        for (offset, &byte) in text_bytes[cursor_pos..].iter().enumerate() {
            if byte == b';' {
                // Include the semicolon
                end_pos = cursor_pos + offset + 1;
                break;
            }
        }

        // Extract the query text
        if let Ok(query_text) = std::str::from_utf8(&text_bytes[start_pos..end_pos]) {
            query_text.trim().to_string()
        } else {
            String::new()
        }
    }


pub(crate) fn close_tabs_for_file(tabular: &mut window_egui::Tabular, file_path: &str) {
        // Find all tabs that have this file open and close them
        let mut indices_to_close = Vec::new();
        
        for (index, tab) in tabular.query_tabs.iter().enumerate() {
            if tab.file_path.as_deref() == Some(file_path) {
                indices_to_close.push(index);
            }
        }
        
        // Close tabs in reverse order to maintain correct indices
        for &index in indices_to_close.iter().rev() {
            editor::close_tab(tabular, index);
        }
    }
