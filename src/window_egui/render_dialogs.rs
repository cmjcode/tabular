use eframe::egui;
use log::debug;
use crate::{models, connection, query_tools, editor, data_table};

fn draw_format_sql_button(
    ctx: &egui::Context,
    area_id: egui::Id,
    pos: egui::Pos2,
    size: egui::Vec2,
    corner: u8,
) -> bool {
    let mut clicked = false;
    let format_text = egui::RichText::new("</>").size(16.0);
    egui::Area::new(area_id)
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |area_ui| {
            let button = egui::Button::new(format_text.clone())
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.5, egui::Color32::TRANSPARENT))
                .corner_radius(egui::CornerRadius::same(corner));
            let response = area_ui
                .add_sized(size, button)
                .on_hover_text("Format SQL (Cmd+Shift+F)");
            if response.clicked() {
                clicked = true;
            }
        });
    clicked
}


impl super::Tabular {
    pub fn render_lint_panel(&mut self, ui: &mut egui::Ui) {
        if self.lint_messages.is_empty() {
            return;
        }

        ui.add_space(6.0);
        let count = self.lint_messages.len();
        let plural = if count == 1 { "" } else { "s" };

        if !self.show_lint_panel {
            ui.horizontal(|ui| {
                let warning_text =
                    egui::RichText::new(format!("⚠ {} lint issue{} detected", count, plural))
                        .color(egui::Color32::from_rgb(255, 183, 0));
                ui.label(warning_text);
                if ui.button("Show details").clicked() {
                    self.show_lint_panel = true;
                    self.lint_panel_shown_at = Some(std::time::Instant::now());
                }
            });
            return;
        }

        // Panel is shown: start timer if needed (hover/pin logic handled after rendering)
        if self.lint_panel_shown_at.is_none() {
            self.lint_panel_shown_at = Some(std::time::Instant::now());
        }

        let panel_fill = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(40, 40, 40)
        } else {
            egui::Color32::from_rgb(255, 244, 234)
        };

        let inner = egui::Frame::group(ui.style())
            .fill(panel_fill)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 0, 0)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Lint ({})", count)).strong());
                    if ui.button("Hide").clicked() {
                        self.show_lint_panel = false;
                        self.lint_panel_shown_at = None;
                    }
                    ui.add_space(12.0);
                    ui.checkbox(&mut self.lint_panel_pinned, "Pin (keep open)");
                    ui.checkbox(
                        &mut self.auto_format_on_execute,
                        "Auto-format before execute",
                    );
                    if ui.button("Format now").clicked()
                        && let Some(formatted) = query_tools::format_sql(&self.editor.text)
                        && formatted != self.editor.text
                    {
                        self.editor.set_text(formatted.clone());
                        let new_len = self.editor.text.len();
                        self.cursor_position = new_len;
                        self.multi_selection.clear();
                        self.multi_selection.add_collapsed(self.cursor_position);
                        self.last_editor_text = self.editor.text.clone();
                        self.lint_messages = query_tools::lint_sql(&self.editor.text);
                        self.show_lint_panel = !self.lint_messages.is_empty();
                        if self.show_lint_panel {
                            self.lint_panel_shown_at = Some(std::time::Instant::now());
                        } else {
                            self.lint_panel_shown_at = None;
                        }
                        self.editor_focus_boost_frames = self.editor_focus_boost_frames.max(4);
                        self.pending_cursor_set = Some(self.cursor_position);
                    }
                });

                ui.separator();

                for msg in &self.lint_messages {
                    let (icon, color) = match msg.severity {
                        query_tools::LintSeverity::Info => {
                            ("ℹ", egui::Color32::from_rgb(120, 170, 255))
                        }
                        query_tools::LintSeverity::Warning => {
                            ("⚠", egui::Color32::from_rgb(255, 183, 0))
                        }
                        query_tools::LintSeverity::Error => {
                            ("⛔", egui::Color32::from_rgb(255, 0, 0))
                        }
                    };

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icon).color(color).strong());
                        ui.label(egui::RichText::new(&msg.message));
                    });

                    if let Some(hint) = &msg.hint {
                        ui.label(egui::RichText::new(hint).small().italics().weak());
                    }

                    if let Some(span) = &msg.span {
                        ui.label(
                            egui::RichText::new(format!("range {}..{}", span.start, span.end))
                                .small()
                                .weak(),
                        );
                    }

                    ui.add_space(4.0);
                }
            });

        // After rendering, handle auto-hide with hover/pin behavior
        if self.show_lint_panel {
            // If pinned, do not auto-hide
            if self.lint_panel_pinned {
                self.lint_panel_shown_at = Some(std::time::Instant::now());
            } else {
                // If hovered, refresh timer to prevent hiding while interacting
                if inner.response.hovered() {
                    self.lint_panel_shown_at = Some(std::time::Instant::now());
                }
                // Check elapsed when not hovered
                if let Some(shown_at) = self.lint_panel_shown_at {
                    let elapsed_ms = shown_at.elapsed().as_millis() as u64;
                    if elapsed_ms >= self.lint_panel_auto_hide_ms {
                        self.show_lint_panel = false;
                        self.lint_panel_shown_at = None;
                    }
                }
            }
        }
    }
    pub fn render_replication_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_add_replication_dialog {
            return;
        }

        let mut open = true;
        let mut close_dialog = false;
        let mut start_replication = false;
        let mut source_id_to_start = None;
        let mut target_id_for_start = 0;
        let mut repl_user_to_start = String::new();
        let mut repl_pass_to_start = String::new();

        let mut source_candidates = Vec::new();

        // Extract candidates to avoid borrowing self inside closure
        // Only include connections that have an active pool
        if let Some(state) = &self.replication_dialog {
            log::info!("[REPLICATION] Building source candidates for target_id: {}", state.target_connection_id);
            log::info!("[REPLICATION] Total connections: {}", self.connections.len());
            log::info!("[REPLICATION] Active pools: {}", self.connection_pools.len());
            
            for conn in &self.connections {
                if let Some(conn_id) = conn.id {
                    let is_target = conn_id == state.target_connection_id;
                    let is_mysql = conn.connection_type == models::enums::DatabaseType::MySQL;
                    let has_pool = self.connection_pools.contains_key(&conn_id);
                    
                    log::info!(
                        "[REPLICATION] Conn '{}' (id={}): is_target={}, is_mysql={}, has_pool={}",
                        conn.name, conn_id, is_target, is_mysql, has_pool
                    );
                    
                    if !is_target && is_mysql && has_pool {
                        source_candidates.push((Some(conn_id), conn.name.clone()));
                        log::info!("[REPLICATION] ✓ Added '{}' to candidates", conn.name);
                    }
                }
            }
            
            log::info!("[REPLICATION] Total source candidates: {}", source_candidates.len());
        }

        egui::Window::new("Setup Replication")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                if let Some(state) = &mut self.replication_dialog {
                    ui.heading("Configure Replication");
                    ui.add_space(8.0);
                    ui.label("Select the Master connection to replicate from:");
                    
                    let current_source = state.source_connection_id;
                    let current_name = source_candidates.iter()
                        .find(|(id, _)| *id == current_source)
                        .map(|(_, name)| name.as_str())
                        .unwrap_or("Select Master...");

                    egui::ComboBox::from_id_salt("repl_master_combo")
                        .selected_text(current_name)
                        .show_ui(ui, |ui| {
                            for (id, name) in &source_candidates {
                                let is_selected = current_source == *id;
                                if ui.selectable_label(is_selected, name).clicked() {
                                    state.source_connection_id = *id;
                                    state.error = None;
                                }
                            }
                        });
                        
                    ui.add_space(8.0);
                    ui.label("Replication User (Optional - leave empty to use connection default):");
                    ui.text_edit_singleline(&mut state.replication_user);
                    
                    ui.add_space(8.0);
                    ui.label("Replication Password (Optional):");
                    ui.add(egui::TextEdit::singleline(&mut state.replication_password).password(true));

                    ui.add_space(8.0);
                    
                    if let Some(err) = &state.error {
                         ui.label(egui::RichText::new(err).color(egui::Color32::RED));
                         ui.add_space(8.0);
                    }
                    
                    ui.separator();
                    
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close_dialog = true;
                        }
                        
                        let can_start = state.source_connection_id.is_some() && !state.is_executing;
                        if ui.add_enabled(can_start, egui::Button::new("Init & Start Replication")).clicked() {
                            if let Some(sid) = state.source_connection_id {
                                log::info!("[REPLICATION] Button clicked! Setting state: is_executing=true, start_replication=true");
                                log::info!("[REPLICATION] source_id (sid) = {:?}, target_id = {}", sid, state.target_connection_id);
                                state.is_executing = true;
                                start_replication = true;
                                source_id_to_start = Some(sid);
                                target_id_for_start = state.target_connection_id;
                                repl_user_to_start = state.replication_user.clone();
                                repl_pass_to_start = state.replication_password.clone();
                            } else {
                                log::error!("[REPLICATION] Button clicked but source_connection_id is None!");
                            }
                        }
                    });
                    
                    if state.is_executing {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Setting up replication... please wait.");
                        });
                    }
                }
            });

        if !open || close_dialog {
            self.show_add_replication_dialog = false;
            self.replication_dialog = None;
        }
        
        if start_replication {
            log::info!("[REPLICATION] start_replication=true, source_id_to_start={:?}", source_id_to_start);
            if let Some(source_id) = source_id_to_start {
                let target_id = target_id_for_start;
                
                log::info!("[REPLICATION] Starting replication setup task for source_id={}, target_id={}", source_id, target_id);
                
                let runtime = self.get_runtime();
                let (tx, rx) = std::sync::mpsc::channel();
                self.replication_setup_receiver = Some(rx);
                
                // Clone necessary data for async task
                // (No need to clone self, we have cloned configs)
                
                let source_config_opt = self.connections.iter().find(|c| c.id == Some(source_id)).cloned();
                let target_config_opt = self.connections.iter().find(|c| c.id == Some(target_id)).cloned();
                
                if let (Some(source_config), Some(target_config)) = (source_config_opt, target_config_opt) {
                    runtime.spawn(async move {
                        log::info!("[REPLICATION] Async task started");
                        
                        // Helper to create pool manually since we can't easily use app-wide helpers here
                        async fn create_mysql_pool(config: &models::structs::ConnectionConfig) -> Result<sqlx::MySqlPool, String> {
                            let encoded_username = crate::modules::url_encode(&config.username);
                            let encoded_password = crate::modules::url_encode(&config.password);
                            let dsn = format!(
                                "mysql://{}:{}@{}:{}/{}",
                                encoded_username, encoded_password, config.host, config.port, config.database
                            );
                            
                            sqlx::mysql::MySqlPoolOptions::new()
                                .max_connections(5)
                                .acquire_timeout(std::time::Duration::from_secs(5))
                                .connect(&dsn)
                                .await
                                .map_err(|e| e.to_string())
                        }

                        // Create pools on demand
                        let source_pool_res = create_mysql_pool(&source_config).await;
                        let target_pool_res = create_mysql_pool(&target_config).await;
                        
                        match (source_pool_res, target_pool_res) {
                            (Ok(source_pool), Ok(target_pool)) => {
                                log::info!("[REPLICATION] Pools created successfully, running setup...");
                                let res = crate::driver_mysql::setup_replication(
                                    &source_pool, 
                                    &target_pool, 
                                    &source_config,
                                    repl_user_to_start,
                                    repl_pass_to_start
                                ).await;
                                let _ = tx.send(res);
                            },
                             (Err(e), _) => {
                                 let _ = tx.send(Err(format!("Failed to connect to Master: {}", e)));
                             },
                             (_, Err(e)) => {
                                 let _ = tx.send(Err(format!("Failed to connect to Replica: {}", e)));
                             }
                        }
                    });
                } else {
                    log::error!("[REPLICATION] Could not find config for source or target");
                }
            }
        }
    }
    pub fn render_messages_content(&mut self, ui: &mut egui::Ui) {
        if self.query_message.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    egui::RichText::new("No messages")
                        .size(16.0)
                        .weak()
                );
            });
            return;
        }

        // Full-height messages view
        egui::ScrollArea::vertical()
            .id_salt("messages_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(12.0);

                let (text_color, icon) = if self.query_message_is_error {
                    // Error styling
                    if ui.visuals().dark_mode {
                        (egui::Color32::from_rgb(255, 120, 120), "❌")
                    } else {
                        (egui::Color32::from_rgb(180, 40, 40), "❌")
                    }
                } else {
                    // Success styling
                    if ui.visuals().dark_mode {
                        (egui::Color32::from_rgb(120, 220, 120), "👍")
                    } else {
                        (egui::Color32::from_rgb(40, 140, 40), "👍")
                    }
                };

                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(icon)
                            .size(20.0)
                            .color(text_color)
                    );
                    
                    ui.spacing_mut().item_spacing.x = 8.0;
                    
                    // Sync display buffer with actual message if they differ
                    if self.query_message_display_buffer != self.query_message {
                        self.query_message_display_buffer = self.query_message.clone();
                    }
                    
                    // Use TextEdit with persistent buffer for selection state
                    // Use absolute ID so we can check focus in copy handler
                    let message_text_id = egui::Id::new("tabular_message_text_edit_widget");
                    let output = egui::TextEdit::multiline(&mut self.query_message_display_buffer)
                        .id(message_text_id)
                        .desired_width(f32::INFINITY)
                        .text_color(text_color)
                        .font(egui::TextStyle::Body)
                        .frame(false)
                        .interactive(true)
                        .show(ui);
                    
                    // Request focus when clicked to ensure CMD+C works
                    if output.response.clicked() {
                        output.response.request_focus();
                    }
                    
                    // Manual copy handling for CMD+C in message TextEdit
                    if output.response.has_focus() {
                        ui.input(|i| {
                            let copy_event = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                            let key_combo = (i.modifiers.mac_cmd || i.modifiers.ctrl) && i.key_pressed(egui::Key::C);
                            
                            if copy_event || key_combo {
                                // Get cursor range to find selected text
                                if let Some(state) = egui::TextEdit::load_state(ui.ctx(), message_text_id)
                                    && let Some(cursor_range) = state.cursor.char_range() {
                                    let start = cursor_range.primary.index;
                                    let end = cursor_range.secondary.index;
                                    let (min, max) = if start < end { (start, end) } else { (end, start) };
                                    
                                    if min < max && max <= self.query_message_display_buffer.len() {
                                        let selected_text = &self.query_message_display_buffer[min..max];
                                        ui.ctx().copy_text(selected_text.to_string());
                                        debug!("📋 Copied selected text from message: {} chars", selected_text.len());
                                    }
                                }
                            }
                        });
                    }
                    
                    // Don't sync changes back - keep it read-only
                    // But preserve selection state by keeping the buffer
                    
                    // Context menu on right-click
                    output.response.context_menu(|ui| {
                        if ui.button("📋 Copy Text").clicked() {
                            ui.ctx().copy_text(self.query_message.clone());
                            ui.close();
                        }
                    });
                });

                ui.add_space(8.0);
            });
    }

    pub fn render_add_view_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let show_dialog = self.show_add_view_dialog;
        
        if show_dialog {
            // Log raw input events
            ctx.input(|i| {
                if i.key_pressed(egui::Key::Backspace) {
                    println!("🔍 [Dialog] Backspace key detected in raw input");
                }
                if !i.events.is_empty() {
                    println!("🔍 [Dialog] Input events count: {}", i.events.len());
                }
            });
                        


            let title = if self.edit_view_original_name.is_some() { "Edit Custom View" } else { "Add Custom View" };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(true)
                .default_size([600.0, 400.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Name:");
                    let before_len = self.new_view_name.len();
                    let name_edit = egui::TextEdit::singleline(&mut self.new_view_name)
                        .desired_width(f32::INFINITY);
                    
                    let name_response = ui.add(name_edit);
                    let after_len = self.new_view_name.len();
                    
                    println!("🔍 [Name Field] Before len: {}, After len: {}", before_len, after_len);
                    println!("🔍 [Name Field] Has focus: {}, changed: {}, lost_focus: {}", 
                        name_response.has_focus(), name_response.changed(), name_response.lost_focus());
                    
                    // Request focus on the name field when dialog first opens
                    if ui.memory(|mem| mem.focused().is_none()) {
                        println!("🔍 [Name Field] Requesting focus (first open)");
                        name_response.request_focus();
                    }

                    ui.add_space(8.0);
                    ui.label("SQL Query:");
                    
                    let before_query_len = self.new_view_query.len();
                    let query_edit = egui::TextEdit::multiline(&mut self.new_view_query)
                        .desired_width(f32::INFINITY)
                        .desired_rows(10);
                    
                    let query_response = ui.add(query_edit);
                    let after_query_len = self.new_view_query.len();
                    
                    println!("🔍 [Query Field] Before len: {}, After len: {}", before_query_len, after_query_len);
                    println!("🔍 [Query Field] Has focus: {}, changed: {}", 
                        query_response.has_focus(), query_response.changed());

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked()
                             && !self.new_view_name.is_empty() && !self.new_view_query.is_empty()
                                 && let Some(conn_id) = self.new_view_connection_id {
                                     // Save logic
                                     if let Some(conn_idx) = self.connections.iter().position(|c| c.id == Some(conn_id)) {
                                         let mut conn = self.connections[conn_idx].clone();
                                         let new_view = models::structs::CustomView {
                                             name: self.new_view_name.clone(),
                                             query: self.new_view_query.clone(),
                                         };
                                         
                                         if let Some(original_name) = &self.edit_view_original_name {
                                             // Edit mode: find and update
                                             if let Some(view_idx) = conn.custom_views.iter().position(|v| v.name == *original_name) {
                                                 conn.custom_views[view_idx] = new_view;
                                             } else {
                                                 // Should not happen normally, but treat as new if not found
                                                 conn.custom_views.push(new_view);
                                             }
                                         } else {
                                             // Add mode: append
                                             conn.custom_views.push(new_view);
                                         }
                                         
                                         // Update database
                                         if crate::sidebar_database::update_connection_in_database(self, &conn) {
                                             // Update in-memory
                                              self.connections[conn_idx] = conn;
                                              // Trigger refresh
                                              crate::sidebar_database::refresh_connections_tree(self);
                                              self.show_add_view_dialog = false;
                                         } else {
                                             // Handle error (maybe show toast/log)
                                             log::error!("Failed to save custom view to database");
                                         }
                                     }
                                 }
                        if ui.button("Cancel").clicked() {
                            self.show_add_view_dialog = false;
                        }
                    });
                });
        }

        if !open {
            self.show_add_view_dialog = false;
        }
    }

    /// Consolidated rendering of query editor with split results panel
    pub fn render_query_editor_with_split(
        &mut self,
        ui: &mut egui::Ui,
        context_id: &str, // "view_query" or "regular_query"
    ) {
        let avail = ui.available_height();
        let executed = self
            .query_tabs
            .get(self.active_tab_index)
            .map(|t| t.has_executed_query)
            .unwrap_or(false);
        let has_headers = !self.current_table_headers.is_empty();
        let has_message = !self.current_table_name.is_empty();
        let show_bottom = has_headers || has_message || executed;

        if show_bottom {
            self.table_split_ratio = self.table_split_ratio.clamp(0.05, 0.995);
        }

        let editor_h = if show_bottom {
            let mut h = avail * self.table_split_ratio;
            if has_headers {
                h = h.clamp(100.0, (avail - 50.0).max(100.0));
            } else {
                h = h.clamp(140.0, (avail - 30.0).max(140.0));
            }
            h
        } else {
            avail
        };

        egui::Frame::NONE
            .fill(if ui.visuals().dark_mode {
                egui::Color32::from_rgb(30, 30, 30)
            } else {
                egui::Color32::WHITE
            })
            .show(ui, |ui| {
                let editor_area_height = editor_h.max(200.0);
                let mono_h = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
                let rows = ((editor_area_height / mono_h).floor() as i32) as usize;
                self.advanced_editor.desired_rows = rows;

                let avail_w = ui.available_width() - 4.0;
                let desired = egui::vec2(avail_w, editor_area_height);
                let (rect, _resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
                let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));

                egui::ScrollArea::vertical()
                    .id_salt(format!("query_editor_scroll_{}", context_id))
                    .auto_shrink([false, false])
                    .show(&mut child_ui, |ui| {
                        ui.set_min_width(avail_w - 4.0);
                        editor::render_advanced_editor(self, ui);
                    });

                // Floating execute button
                let button_margin = 4.0;
                let button_size = egui::vec2(32.0, 32.0);
                let button_pos = egui::pos2(
                    rect.max.x - button_size.x - button_margin,
                    rect.min.y + button_margin,
                );
                let play_fill = egui::Color32::TRANSPARENT;
                let is_loading = self.query_execution_in_progress || self.pool_wait_in_progress;
                let (play_icon, play_color, play_border, tooltip_text) = if is_loading {
                    ("⏳", egui::Color32::WHITE, egui::Color32::TRANSPARENT, "Executing query…")
                } else {
                    ("▶", egui::Color32::from_rgb(50,205,50), egui::Color32::TRANSPARENT, "CMD+Enter to execute")
                };
                let play_text = egui::RichText::new(play_icon).color(play_color).size(18.0);
                let button_corner = (button_size.y / 2.0).round().clamp(2.0, u8::MAX as f32) as u8;

                let mut execute_clicked = false;
                let mut captured_selection_text = String::new();

                // Auto-execute if requested by the tab (e.g. Custom View opened)
                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                    && tab.should_run_on_open {
                        execute_clicked = true;
                        tab.should_run_on_open = false;
                        self.query_execution_in_progress = true;
                    }

                egui::Area::new(egui::Id::new((format!("floating_execute_button_{}", context_id), self.active_tab_index)))
                    .order(egui::Order::Foreground)
                    .fixed_pos(button_pos)
                    .show(ui.ctx(), |area_ui| {
                        let mut button = egui::Button::new(play_text.clone())
                            .fill(play_fill)
                            .stroke(egui::Stroke::new(1.5, play_border))
                            .corner_radius(egui::CornerRadius::same(button_corner));
                        if is_loading {
                            button = button.sense(egui::Sense::hover());
                        }
                        let response = area_ui.add_sized(button_size, button).on_hover_text(tooltip_text);
                        if !is_loading && response.clicked() {
                            let id = egui::Id::new("sql_editor");
                            let mut direct_selected = String::new();
                            if let Some(range) = crate::editor_state_adapter::EditorStateAdapter::get_range(area_ui.ctx(), id) {
                                let to_byte_index = |s: &str, char_idx: usize| -> usize {
                                    s.char_indices().map(|(b, _)| b).chain(std::iter::once(s.len())).nth(char_idx).unwrap_or(s.len())
                                };
                                let start_b = to_byte_index(&self.editor.text, range.start);
                                let end_b = to_byte_index(&self.editor.text, range.end);
                                if start_b < end_b && end_b <= self.editor.text.len() {
                                    direct_selected = self.editor.text[start_b..end_b].to_string();
                                }
                            }
                            self.query_execution_in_progress = true;
                            execute_clicked = true;
                            captured_selection_text = if !direct_selected.is_empty() {
                                direct_selected
                            } else {
                                self.selected_text.clone()
                            };
                        }
                    });

                // Floating format button
                let format_spacing = 6.0;
                let format_button_pos = egui::pos2(
                    button_pos.x - button_size.x - format_spacing,
                    button_pos.y,
                );
                let format_clicked = draw_format_sql_button(
                    ui.ctx(),
                    egui::Id::new((format!("floating_format_button_{}", context_id), self.active_tab_index)),
                    format_button_pos,
                    button_size,
                    button_corner,
                );

                if execute_clicked {
                    self.is_table_browse_mode = false;
                    self.extend_query_icon_hold();
                    editor::execute_query_with_text(self, captured_selection_text);
                    ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                    ui.ctx().request_repaint();
                }

                if format_clicked {
                    editor::reformat_current_sql(self, ui);
                    ui.ctx().memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                    ui.ctx().request_repaint();
                }

                // Keyboard shortcut
                if ui.input(|i| (i.modifiers.ctrl || i.modifiers.mac_cmd) && i.key_pressed(egui::Key::Enter)) {
                    let has_q = if !self.selected_text.trim().is_empty() {
                        true
                    } else {
                        let cq = editor::extract_query_from_cursor(self);
                        !cq.trim().is_empty() || !self.editor.text.trim().is_empty()
                    };
                    if has_q {
                        let id = egui::Id::new("sql_editor");
                        let mut direct_selected = String::new();
                        if let Some(range) = crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id) {
                            let to_byte_index = |s: &str, char_idx: usize| -> usize {
                                s.char_indices().map(|(b, _)| b).chain(std::iter::once(s.len())).nth(char_idx).unwrap_or(s.len())
                            };
                            let start_b = to_byte_index(&self.editor.text, range.start);
                            let end_b = to_byte_index(&self.editor.text, range.end);
                            if start_b < end_b && end_b <= self.editor.text.len() {
                                direct_selected = self.editor.text[start_b..end_b].to_string();
                            }
                        }
                        self.extend_query_icon_hold();
                        let captured_selection = if !direct_selected.is_empty() {
                            direct_selected
                        } else {
                            self.selected_text.clone()
                        };
                        editor::execute_query_with_text(self, captured_selection);
                    }
                }
            });

        self.render_lint_panel(ui);

        if show_bottom {
            let handle_id = ui.make_persistent_id(format!("editor_table_splitter_{}", context_id));
            let desired_h = 6.0;
            let available_w = ui.available_width();
            let (rect, resp) = ui.allocate_at_least(egui::vec2(available_w, desired_h), egui::Sense::click_and_drag());
            let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color);
            ui.painter().hline(rect.x_range(), rect.center().y, stroke);
            if resp.dragged() {
                let drag_delta = resp.drag_delta().y;
                if avail > 0.0 {
                    self.table_split_ratio = (self.table_split_ratio + (drag_delta / avail)).clamp(0.05, 0.995);
                }
                ui.memory_mut(|m| m.request_focus(handle_id));
            }
            ui.add_space(2.0);
            
            // RESULT TAB BAR
            // Only show if we have more than one result in the active tab
            let mut result_tabs_info: Option<(usize, usize)> = None; // (count, active_index)
            if let Some(tab) = self.query_tabs.get(self.active_tab_index).filter(|t| t.results.len() > 1) {
                    result_tabs_info = Some((tab.results.len(), tab.active_result_index));
            }

            if let Some((count, active_idx)) = result_tabs_info {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for i in 0..count {
                        let label = format!("Result {}", i + 1);
                        let is_active = i == active_idx;
                        let btn = if is_active {
                             egui::Button::new(egui::RichText::new(label).strong().color(egui::Color32::WHITE))
                                .fill(egui::Color32::from_rgb(255, 0, 0)) // default red 
                        } else {
                             egui::Button::new(label)
                        };
                        
                        if ui.add(btn).clicked() {
                            // Switch result tab!
                             if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                                tab.active_result_index = i;
                                if let Some(res) = tab.results.get(i) {
                                    // Sync to viewport fields
                                    self.current_table_headers = res.headers.clone();
                                    self.current_table_data = res.rows.clone();
                                    self.all_table_data = res.all_rows.clone();
                                    self.current_table_name = res.table_name.clone();
                                    self.total_rows = res.total_rows;
                                    self.current_page = res.current_page;
                                    self.page_size = res.page_size;
                                    self.query_message = res.query_message.clone();
                                    self.query_message_is_error = res.query_message_is_error;
                                    self.show_message_panel = true; // Always show message panel context
                                     // Also update Viewport fields in Tab
                                    tab.result_headers = res.headers.clone();
                                    tab.result_rows = res.rows.clone();
                                    tab.result_all_rows = res.all_rows.clone();
                                    tab.result_table_name = res.table_name.clone();
                                    tab.query_message = res.query_message.clone();
                                    tab.query_message_is_error = res.query_message_is_error;
                                    tab.total_rows = res.total_rows;
                                    tab.current_page = res.current_page;
                                }
                             }
                        }
                    }
                });
                ui.separator();
            }

            // Render bottom panel based on view mode
            match self.table_bottom_view {
                models::structs::TableBottomView::Messages => {
                    self.render_messages_content(ui);
                }
                _ => {
                    data_table::render_table_data(self, ui);
                }
            }
        }
    }
    pub fn render_active_query_jobs_overlay(&mut self, ctx: &egui::Context) {
        self.prune_cancelled_jobs();
        if self.active_query_jobs.is_empty() {
            return;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(200));

        let mut jobs: Vec<connection::QueryJobStatus> =
            self.active_query_jobs.values().cloned().collect();
        jobs.sort_by_key(|status| status.started_at);

        let count = jobs.len();
        let title = if count == 1 {
            "1 running query".to_string()
        } else {
            format!("{} running queries", count)
        };

        let visuals = ctx.style().visuals.clone();
        let frame_fill = if visuals.dark_mode {
            egui::Color32::from_rgb(40, 40, 40)
        } else {
            egui::Color32::from_rgb(255, 245, 235)
        };
        let frame_stroke = if visuals.dark_mode {
            egui::Color32::from_rgb(70, 70, 70)
        } else {
            egui::Color32::from_rgb(225, 190, 170)
        };
        egui::Area::new(egui::Id::new("active_query_jobs_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .show(ctx, |area_ui| {
                egui::Frame::default()
                    .fill(frame_fill)
                    .stroke(egui::Stroke::new(1.0, frame_stroke))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(area_ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("⏳").strong().size(14.0));
                            ui.label(
                                egui::RichText::new(title.clone())
                                    .strong()
                                    .color(egui::Color32::from_rgb(255, 0, 0)), // rgba(255, 60, 0, 1)
                            );
                        });

                        ui.add_space(4.0);

                        ui.vertical(|ui| {
                            ui.set_max_width(420.0);
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 6.0);
                            for status in jobs.iter() {
                                let connection_label = self
                                    .get_connection_name(status.connection_id)
                                    .unwrap_or_else(|| {
                                        format!("Connection {}", status.connection_id)
                                    });
                                let elapsed = status.started_at.elapsed();
                                let elapsed_label = if elapsed.as_secs() >= 60 {
                                    let minutes = elapsed.as_secs() / 60;
                                    let seconds = elapsed.as_secs() % 60;
                                    format!("{}m {:02}s", minutes, seconds)
                                } else {
                                    format!("{:.1}s", elapsed.as_secs_f32())
                                };

                                let sanitised = status.query_preview.replace('\n', " ");
                                let mut preview = sanitised.chars().take(60).collect::<String>();
                                if sanitised.chars().count() > 60 {
                                    preview.push('…');
                                }

                                let chip_text = format!(
                                    "{} • {} • {}",
                                    connection_label,
                                    elapsed_label,
                                    preview.trim()
                                );

                                let job_id = status.job_id;
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);

                                    let response = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(chip_text.clone()).size(11.0),
                                        )
                                        .wrap(),
                                    );
                                    response.on_hover_text(status.query_preview.clone());

                                    let cancel_button = ui.add(
                                        egui::Button::new(
                                            egui::RichText::new("Cancel")
                                                .size(11.0)
                                                .color(egui::Color32::from_rgb(255, 0, 0)),
                                        )
                                        .min_size(egui::vec2(64.0, 22.0)),
                                    );

                                    if cancel_button.clicked()
                                        && self.cancel_active_query_job(job_id)
                                    {
                                        ctx.request_repaint();
                                    }
                                });
                            }
                        });
                    });
            });
    }
}
