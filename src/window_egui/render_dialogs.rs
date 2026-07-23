use eframe::egui;
use log::debug;
use crate::{models, connection, query_tools, editor, data_table};



impl super::Tabular {
    pub fn render_bottom_right_dock(&mut self, ctx: &egui::Context, rendered_http: bool, rendered_redis_browser: bool) {
        let executed = self
            .query_tabs
            .get(self.active_tab_index)
            .map(|t| t.has_executed_query)
            .unwrap_or(false);
        let has_headers = !self.current_table_headers.is_empty();
        let has_message = !self.query_message.is_empty();
        let has_lint = !self.lint_messages.is_empty();
        if rendered_http || rendered_redis_browser || (!executed && !has_headers && !has_message && !has_lint) {
            return;
        }

        // 5-second auto-hide timer check for Query Message toast
        let mut msg_hovered = false;
        if self.show_message_panel && has_message
            && let Some(shown_at) = self.message_shown_at
            && shown_at.elapsed() < std::time::Duration::from_secs(5)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        let is_msg_open = self.show_message_panel && has_message;
        let is_lint_open = self.show_lint_panel && has_lint;

        if !is_msg_open && !is_lint_open {
            return;
        }

        let mut close_msg_toast = false;
        let mut close_lint_toast = false;
        let mut format_clicked = false;
        let toast_width = 380.0;

        // 1. MESSAGE TOAST CARD (Anchored at fixed RIGHT_BOTTOM position -8.0, -44.0)
        if is_msg_open {
            let area_resp = egui::Area::new(egui::Id::new("message_toast_overlay"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -44.0))
                .show(ctx, |ui| {
                    let container_fill = if ctx.global_style().visuals.dark_mode {
                        egui::Color32::from_rgb(30, 31, 36)
                    } else {
                        egui::Color32::from_rgb(255, 250, 245)
                    };
                    let stroke_color = if self.query_message_is_error {
                        super::style::theme_danger(ctx)
                    } else {
                        super::style::theme_accent(ctx)
                    };
                    let container_stroke = egui::Stroke::new(1.0, stroke_color);

                    egui::Frame::new()
                        .fill(container_fill)
                        .stroke(container_stroke)
                        .corner_radius(egui::CornerRadius::same(10u8))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .shadow(egui::Shadow {
                            offset: [0, 4],
                            blur: 10,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(100),
                        })
                        .show(ui, |ui| {
                            ui.set_min_width(toast_width);
                            ui.set_max_width(toast_width);
                            ui.vertical(|ui| {
                                let (title, title_color) = if self.query_message_is_error {
                                    ("❌ Error Details", super::style::theme_danger(ctx))
                                } else {
                                    ("💬 Query Message", super::style::theme_accent(ctx))
                                };

                                // Header row with title & close button (X) aligned right
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(title)
                                            .color(title_color)
                                            .strong(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            if super::style::render_close_icon_button(ui).clicked() {
                                                close_msg_toast = true;
                                            }
                                        },
                                    );
                                });

                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);

                                // Scrollable message text box
                                egui::ScrollArea::vertical()
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        if self.query_message_display_buffer != self.query_message {
                                            self.query_message_display_buffer = self.query_message.clone();
                                        }

                                        let message_text_id = egui::Id::new("tabular_message_toast_text");
                                        let text_color = if self.query_message_is_error {
                                            super::style::theme_danger(ctx)
                                        } else {
                                            ui.visuals().text_color()
                                        };
                                        let output = egui::TextEdit::multiline(&mut self.query_message_display_buffer)
                                            .id(message_text_id)
                                            .desired_width(f32::INFINITY)
                                            .text_color(text_color)
                                            .font(egui::TextStyle::Body)
                                            .frame(egui::Frame::NONE)
                                            .interactive(true)
                                            .show(ui);

                                        if output.response.clicked() {
                                            output.response.request_focus();
                                        }

                                        output.response.context_menu(|ui| {
                                            if ui.button("📋 Copy Text").clicked() {
                                                ui.ctx().copy_text(self.query_message.clone());
                                                ui.close();
                                            }
                                        });
                                    });
                            });
                        });
                });

            let h = area_resp.response.rect.height();
            if h > 30.0 {
                self.message_panel_height = h;
            }
            if area_resp.response.hovered() {
                msg_hovered = true;
            }
        }

        // 2. LINT DETAIL TOAST CARD (Positioned right above Message Toast if Message Toast is open)
        if is_lint_open {
            let lint_y_offset = if is_msg_open {
                -44.0 - self.message_panel_height.max(70.0) - 8.0
            } else {
                -44.0
            };

            egui::Area::new(egui::Id::new("lint_toast_overlay"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, lint_y_offset))
                .show(ctx, |ui| {
                    let container_fill = if ctx.global_style().visuals.dark_mode {
                        egui::Color32::from_rgb(30, 31, 36)
                    } else {
                        egui::Color32::from_rgb(255, 250, 245)
                    };
                    let container_stroke = egui::Stroke::new(1.0, super::style::theme_warning(ctx));

                    egui::Frame::new()
                        .fill(container_fill)
                        .stroke(container_stroke)
                        .corner_radius(egui::CornerRadius::same(10u8))
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .shadow(egui::Shadow {
                            offset: [0, 4],
                            blur: 10,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(100),
                        })
                        .show(ui, |ui| {
                            ui.set_min_width(toast_width);
                            ui.set_max_width(toast_width);
                            ui.vertical(|ui| {
                                let count = self.lint_messages.len();
                                let plural = if count == 1 { "" } else { "s" };
                                let warning_color = super::style::theme_warning(ctx);

                                // Header row with title & close button (X) aligned right
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(format!("⚠ Lint Detail{} ({})", plural, count))
                                            .color(warning_color)
                                            .strong(),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            if super::style::render_close_icon_button(ui).clicked() {
                                                close_lint_toast = true;
                                            }
                                        },
                                    );
                                });

                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);

                                // Actions bar inside toast
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut self.auto_format_on_execute, "Auto-format");
                                    if ui.button("✨ Format now").clicked() {
                                        format_clicked = true;
                                    }
                                });

                                ui.add_space(4.0);

                                // Scrollable list of lint messages
                                egui::ScrollArea::vertical()
                                    .max_height(180.0)
                                    .show(ui, |ui| {
                                        for msg in &self.lint_messages {
                                            let (icon, color) = match msg.severity {
                                                query_tools::LintSeverity::Info => {
                                                    ("ℹ", super::style::theme_info(ui.ctx()))
                                                }
                                                query_tools::LintSeverity::Warning => {
                                                    ("⚠", super::style::theme_warning(ui.ctx()))
                                                }
                                                query_tools::LintSeverity::Error => {
                                                    ("⛔", super::style::theme_danger(ui.ctx()))
                                                }
                                            };

                                            ui.horizontal(|ui| {
                                                ui.label(egui::RichText::new(icon).color(color).strong());
                                                ui.label(egui::RichText::new(&msg.message).small());
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
                            });
                        });
                });
        }

        // 5-second auto-hide check for Message Toast
        if is_msg_open
            && !msg_hovered
            && let Some(shown_at) = self.message_shown_at
            && shown_at.elapsed() >= std::time::Duration::from_secs(5)
        {
            close_msg_toast = true;
        }

        if close_msg_toast {
            self.show_message_panel = false;
            self.message_shown_at = None;
        }

        if close_lint_toast {
            self.show_lint_panel = false;
        }

        if format_clicked
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
            if self.lint_messages.is_empty() {
                self.show_lint_panel = false;
            }
            self.editor_focus_boost_frames = self.editor_focus_boost_frames.max(4);
            self.pending_cursor_set = Some(self.cursor_position);
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
            log::debug!("[REPLICATION] Building source candidates for target_id: {}", state.target_connection_id);
            log::debug!("[REPLICATION] Total connections: {}", self.connections.len());
            log::debug!("[REPLICATION] Active pools: {}", self.connection_pools.len());
            
            for conn in &self.connections {
                if let Some(conn_id) = conn.id {
                    let is_target = conn_id == state.target_connection_id;
                    let is_mysql = conn.connection_type == models::enums::DatabaseType::MySQL;
                    let has_pool = self.connection_pools.contains_key(&conn_id);
                    
                    log::debug!(
                        "[REPLICATION] Conn '{}' (id={}): is_target={}, is_mysql={}, has_pool={}",
                        conn.name, conn_id, is_target, is_mysql, has_pool
                    );
                    
                    if !is_target && is_mysql && has_pool {
                        source_candidates.push((Some(conn_id), conn.name.clone()));
                        log::debug!("[REPLICATION] ✓ Added '{}' to candidates", conn.name);
                    }
                }
            }
            
            log::debug!("[REPLICATION] Total source candidates: {}", source_candidates.len());
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
                         ui.label(egui::RichText::new(err).color(super::style::theme_danger(ui.ctx())));
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
                                log::debug!("[REPLICATION] Button clicked! Setting state: is_executing=true, start_replication=true");
                                log::debug!("[REPLICATION] source_id (sid) = {:?}, target_id = {}", sid, state.target_connection_id);
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
            log::debug!("[REPLICATION] start_replication=true, source_id_to_start={:?}", source_id_to_start);
            if let Some(source_id) = source_id_to_start {
                let target_id = target_id_for_start;
                
                log::debug!("[REPLICATION] Starting replication setup task for source_id={}, target_id={}", source_id, target_id);
                
                let runtime = self.get_runtime();
                let (tx, rx) = std::sync::mpsc::channel();
                self.replication_setup_receiver = Some(rx);
                
                // Clone necessary data for async task
                // (No need to clone self, we have cloned configs)
                
                let source_config_opt = self.connections.iter().find(|c| c.id == Some(source_id)).cloned();
                let target_config_opt = self.connections.iter().find(|c| c.id == Some(target_id)).cloned();
                
                if let (Some(source_config), Some(target_config)) = (source_config_opt, target_config_opt) {
                    runtime.spawn(async move {
                        log::debug!("[REPLICATION] Async task started");
                        
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
                                log::debug!("[REPLICATION] Pools created successfully, running setup...");
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
        let avail_h = ui.available_height();
        let footer_h = 44.0;
        let content_h = (avail_h - footer_h).max(40.0);

        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), content_h),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
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
                            (super::style::theme_danger(ui.ctx()), "❌")
                        } else {
                            (super::style::theme_success(ui.ctx()), "👍")
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
                                .frame(egui::Frame::NONE)
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
                                            let start = cursor_range.primary.index.0;
                                            let end = cursor_range.secondary.index.0;
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
            },
        );

        // Integrated footer bar matching data table pagination bar
        let bg_color = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(22, 22, 26)
        } else {
            egui::Color32::from_rgb(245, 245, 250)
        };
        let stroke_color = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(45, 45, 50)
        } else {
            egui::Color32::from_rgb(215, 215, 220)
        };

        egui::Frame::new()
            .fill(bg_color)
            .stroke(egui::Stroke::new(1.0, stroke_color))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("💬 Query Execution Message")
                            .weak()
                            .small(),
                    );
                    data_table::render_footer_view_buttons(self, ui);
                });
            });
    }

    pub fn render_add_view_dialog(&mut self, ctx: &egui::Context) {
        let mut open = true;

        if self.show_add_view_dialog {
            let title = if self.edit_view_original_name.is_some() { "Edit Custom View" } else { "Add Custom View" };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(true)
                .default_size([600.0, 400.0])
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.label("Name:");
                    let name_response = ui.add(
                        egui::TextEdit::singleline(&mut self.new_view_name)
                            .desired_width(f32::INFINITY),
                    );

                    // Request focus on the name field when dialog first opens
                    if ui.memory(|mem| mem.focused().is_none()) {
                        name_response.request_focus();
                    }

                    ui.add_space(8.0);
                    ui.label("SQL Query:");
                    ui.add(
                        egui::TextEdit::multiline(&mut self.new_view_query)
                            .desired_width(f32::INFINITY)
                            .desired_rows(10),
                    );

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

                                         // Optimistic: apply in memory right away and persist on
                                         // the shared runtime; the result lands in
                                         // custom_view_save_receiver (polled in app_impl).
                                         self.connections[conn_idx] = conn.clone();
                                         crate::sidebar_database::refresh_connections_tree(self);
                                         crate::sidebar_database::update_connection_in_database_background(self, &conn);
                                         self.show_add_view_dialog = false;
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

                let button_size = egui::vec2(34.0, 34.0);
                let _button_spacing = 2.0;
                let button_corner = 2_u8;
                let right_margin = 8.0; // Compact right margin to align closely with editor border
                let cluster_pos = egui::pos2(
                    rect.max.x - right_margin,
                    rect.min.y + 6.0,
                );
                let is_loading = self.query_execution_in_progress || self.pool_wait_in_progress;
                let play_text = if is_loading {
                    egui::RichText::new("⏳").color(egui::Color32::WHITE).size(12.0)
                } else {
                    egui::RichText::new("▶")
                        .color(egui::Color32::WHITE)
                        .size(12.0)
                };
                let play_tooltip = if is_loading {
                    "Executing query…"
                } else {
                    "CMD+Enter to execute"
                };

                let (tx_mode, tx_active) = self
                    .query_tabs
                    .get(self.active_tab_index)
                    .map(|t| (t.tx_mode, t.tx_active))
                    .unwrap_or((false, false));

                let mut toggle_changed = false;
                let mut commit_clicked = false;
                let mut rollback_clicked = false;
                let mut execute_clicked = false;
                let mut format_clicked = false;
                let mut explain_clicked = false;
                let mut captured_selection_text = String::new();

                // Auto-execute if requested by the tab (e.g. Custom View opened)
                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                    && tab.should_run_on_open
                {
                    execute_clicked = true;
                    tab.should_run_on_open = false;
                    self.query_execution_in_progress = true;
                }

                egui::Area::new(egui::Id::new((format!("floating_query_actions_{}", context_id), self.active_tab_index)))
                    .order(egui::Order::Foreground)
                    .pivot(egui::Align2::RIGHT_TOP)
                    .fixed_pos(cluster_pos)
                    .show(ui.ctx(), |area_ui| {
                        let ctx = area_ui.ctx().clone();
                        let cluster_bg = if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(25, 25, 25)
                        } else {
                            egui::Color32::from_rgb(248, 248, 248)
                        };
                        let cluster_border = if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(80, 80, 80)
                        } else {
                            egui::Color32::from_rgb(200, 200, 200)
                        };

                        egui::Frame::new()
                            .fill(cluster_bg)
                            .stroke(egui::Stroke::new(1.0, cluster_border))
                            .corner_radius(egui::CornerRadius::same(8u8))
                            .inner_margin(egui::Margin::same(3))
                            .show(area_ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;

                                    if tx_mode {
                                        let mut mode = tx_mode;
                                        if ui
                                            .checkbox(&mut mode, "Manual commit")
                                            .on_hover_text(
                                                "Run statements in a transaction on a dedicated \
                                                 connection; nothing is committed until you press Commit",
                                            )
                                            .changed()
                                        {
                                            toggle_changed = true;
                                        }
                                        if tx_active {
                                            ui.colored_label(egui::Color32::from_rgb(255, 165, 0), "●")
                                                .on_hover_text("Transaction open (uncommitted)");
                                        }
                                        if ui
                                            .add_enabled(tx_active, egui::Button::new("Commit").small())
                                            .clicked()
                                        {
                                            commit_clicked = true;
                                        }
                                        if ui
                                            .add_enabled(tx_active, egui::Button::new("Rollback").small())
                                            .clicked()
                                        {
                                            rollback_clicked = true;
                                        }
                                        ui.add_space(2.0);
                                        ui.separator();
                                        ui.add_space(2.0);
                                    }

                                    let base_fill = if ui.visuals().dark_mode {
                                        egui::Color32::from_rgb(36, 36, 36)
                                    } else {
                                        egui::Color32::from_rgb(245, 245, 245)
                                    };
                                    let base_border = if ui.visuals().dark_mode {
                                        egui::Color32::from_rgb(90, 90, 90)
                                    } else {
                                        egui::Color32::from_rgb(190, 190, 190)
                                    };

                                    let format_button = egui::Button::new(egui::RichText::new("</>").size(11.0))
                                        .fill(base_fill)
                                        .stroke(egui::Stroke::new(1.0, base_border))
                                        .corner_radius(egui::CornerRadius::same(button_corner));
                                    if ui
                                        .add_sized(button_size, format_button)
                                        .on_hover_text("Format SQL (Cmd+Shift+F)")
                                        .clicked()
                                    {
                                        format_clicked = true;
                                    }

                                    let explain_button = egui::Button::new(egui::RichText::new("🔍").size(11.0))
                                        .fill(base_fill)
                                        .stroke(egui::Stroke::new(1.0, base_border))
                                        .corner_radius(egui::CornerRadius::same(button_corner));
                                    if ui
                                        .add_sized(button_size, explain_button)
                                        .on_hover_text("Explain query plan (EXPLAIN)")
                                        .clicked()
                                    {
                                        explain_clicked = true;
                                    }

                                    let execute_button = egui::Button::new(play_text.clone())
                                        .fill(if is_loading {
                                            egui::Color32::from_rgb(60, 60, 60)
                                        } else {
                                            super::style::theme_accent(ui.ctx())
                                        })
                                        .stroke(egui::Stroke::new(1.0, base_border))
                                        .corner_radius(egui::CornerRadius::same(button_corner));
                                    if ui
                                        .add_sized(button_size, execute_button)
                                        .on_hover_text(play_tooltip)
                                        .clicked()
                                        && !is_loading
                                    {
                                        let id = egui::Id::new("sql_editor");
                                        let mut direct_selected = String::new();
                                        if let Some(range) =
                                            crate::editor_state_adapter::EditorStateAdapter::get_range(&ctx, id)
                                        {
                                            let to_byte_index = |s: &str, char_idx: usize| -> usize {
                                                s.char_indices()
                                                    .map(|(b, _)| b)
                                                    .chain(std::iter::once(s.len()))
                                                    .nth(char_idx)
                                                    .unwrap_or(s.len())
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
                            });
                    });

                if toggle_changed
                    && let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                        tab.tx_mode = !tab.tx_mode;
                        if !tab.tx_mode {
                            if let Some(s) = tab.session.take() {
                                s.close();
                            }
                            tab.tx_active = false;
                        }
                    }
                if commit_clicked {
                    editor::send_session_tx_command(self, true);
                }
                if rollback_clicked {
                    editor::send_session_tx_command(self, false);
                }

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

                if explain_clicked {
                    let id = egui::Id::new("sql_editor");
                    let mut direct_selected = String::new();
                    if let Some(range) =
                        crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id)
                    {
                        let to_byte_index = |s: &str, char_idx: usize| -> usize {
                            s.char_indices()
                                .map(|(b, _)| b)
                                .chain(std::iter::once(s.len()))
                                .nth(char_idx)
                                .unwrap_or(s.len())
                        };
                        let start_b = to_byte_index(&self.editor.text, range.start);
                        let end_b = to_byte_index(&self.editor.text, range.end);
                        if start_b < end_b && end_b <= self.editor.text.len() {
                            direct_selected = self.editor.text[start_b..end_b].to_string();
                        }
                    }
                    let captured = if !direct_selected.is_empty() {
                        direct_selected
                    } else {
                        self.selected_text.clone()
                    };
                    editor::explain_current_query(self, captured);
                    ui.ctx().request_repaint();
                }

                // Inline AI loading indicator (shown below the format/run buttons while AI is working)
                if self.ai_inline_receiver.is_some() {
                    let ai_indicator_pos = egui::pos2(
                        cluster_pos.x - 120.0,
                        cluster_pos.y + button_size.y + 4.0,
                    );
                    egui::Area::new(egui::Id::new((format!("ai_inline_loading_{}", context_id), self.active_tab_index)))
                        .order(egui::Order::Foreground)
                        .fixed_pos(ai_indicator_pos)
                        .show(ui.ctx(), |area_ui| {
                            egui::Frame::new()
                                .fill(egui::Color32::from_rgba_unmultiplied(20, 18, 40, 220))
                                .corner_radius(egui::CornerRadius::same(6))
                                .inner_margin(egui::Margin::symmetric(4, 2))
                                .show(area_ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new("AI thinking…")
                                                .color(egui::Color32::from_rgb(190, 170, 255))
                                                .size(10.0),
                                        );
                                    });
                                });
                        });
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
                                .fill(super::style::theme_accent(ui.ctx()))
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

            // Render bottom panel data grid
            data_table::render_table_data(self, ui);
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

        let visuals = ctx.global_style().visuals.clone();
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
        let _executed = self
            .query_tabs
            .get(self.active_tab_index)
            .map(|t| t.has_executed_query)
            .unwrap_or(false);
        let _has_headers = !self.current_table_headers.is_empty();
        let has_message = !self.query_message.is_empty();
        let has_lint = !self.lint_messages.is_empty();
        let is_msg_open = self.show_message_panel && has_message;
        let is_lint_open = self.show_lint_panel && has_lint;

        let mut y_offset = -48.0;
        if is_msg_open {
            y_offset -= self.message_panel_height.max(70.0) + 8.0;
        }
        if is_lint_open {
            y_offset -= 120.0;
        }

        egui::Area::new(egui::Id::new("active_query_jobs_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, y_offset))
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
                                    .color(super::style::theme_accent(ui.ctx())),
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
                                                .color(super::style::theme_danger(ui.ctx())),
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

pub fn render_schema_diff_dialog(tabular: &mut super::Tabular, ctx: &egui::Context) {
    use crate::models::structs::{DiffStatus, SchemaDiffStatus};

    // Collect values needed outside closure upfront to avoid borrow conflicts.
    let conn_labels: Vec<(i64, String)> = tabular.connections.iter()
        .filter_map(|c| c.id.map(|id| (id, c.name.clone())))
        .collect();

    // Variables set inside the window closure and used after.
    let mut run_diff: Option<(i64, String, i64, String)> = None;
    let mut open = tabular.show_schema_diff_dialog;

    egui::Window::new("Schema Diff")
        .open(&mut open)
        .default_size(egui::vec2(820.0, 560.0))
        .resizable(true)
        .collapsible(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            if let Some(state) = &mut tabular.schema_diff_state {
                // ── Connection pickers ────────────────────────────────────
                ui.horizontal(|ui| {
                    ui.label("Left:");
                    egui::ComboBox::from_id_salt("schema_diff_left_conn")
                        .selected_text(
                            conn_labels.iter()
                                .find(|(id, _)| *id == state.left_conn_id)
                                .map(|(_, n)| n.as_str())
                                .unwrap_or("—")
                        )
                        .show_ui(ui, |ui| {
                            for (id, name) in &conn_labels {
                                ui.selectable_value(&mut state.left_conn_id, *id, name);
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut state.left_db)
                            .hint_text("database")
                            .desired_width(120.0),
                    );
                    ui.add_space(16.0);
                    ui.label("Right:");
                    egui::ComboBox::from_id_salt("schema_diff_right_conn")
                        .selected_text(
                            conn_labels.iter()
                                .find(|(id, _)| *id == state.right_conn_id)
                                .map(|(_, n)| n.as_str())
                                .unwrap_or("—")
                        )
                        .show_ui(ui, |ui| {
                            for (id, name) in &conn_labels {
                                ui.selectable_value(&mut state.right_conn_id, *id, name);
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut state.right_db)
                            .hint_text("database")
                            .desired_width(120.0),
                    );
                });

                ui.add_space(6.0);

                // ── Action bar ────────────────────────────────────────────
                let running = state.status == SchemaDiffStatus::Running;
                ui.horizontal(|ui| {
                    if ui.add_enabled(
                        !running,
                        egui::Button::new(if running { "⏳ Running…" } else { "▶ Compare" }),
                    ).clicked() {
                        run_diff = Some((
                            state.left_conn_id, state.left_db.clone(),
                            state.right_conn_id, state.right_db.clone(),
                        ));
                        state.status = SchemaDiffStatus::Running;
                    }
                    ui.checkbox(&mut state.show_same, "Show identical tables");
                    ui.add_space(10.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut state.filter_text)
                            .hint_text("Filter tables…")
                            .desired_width(160.0),
                    );
                });

                ui.separator();

                // ── Results ───────────────────────────────────────────────
                if let Some(result) = &state.result {
                    let filter = state.filter_text.to_lowercase();
                    let show_same = state.show_same;

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let diffs: Vec<_> = result.diffs.iter()
                            .filter(|d| show_same || d.status != DiffStatus::Same)
                            .filter(|d| filter.is_empty() || d.table_name.to_lowercase().contains(&filter))
                            .collect();

                        if diffs.is_empty() {
                            ui.label("No differences found.");
                            return;
                        }

                        egui::Grid::new("schema_diff_grid")
                            .striped(true)
                            .min_col_width(60.0)
                            .show(ui, |ui| {
                                ui.strong("Table");
                                ui.strong("Status");
                                ui.strong("Column changes");
                                ui.end_row();

                                for diff in diffs {
                                    let (status_label, color) = match diff.status {
                                        DiffStatus::Added    => ("+ Added",    egui::Color32::from_rgb(80, 180, 80)),
                                        DiffStatus::Removed  => ("- Removed",  egui::Color32::from_rgb(220, 70, 70)),
                                        DiffStatus::Modified => ("~ Modified", egui::Color32::from_rgb(220, 165, 30)),
                                        DiffStatus::Same     => ("= Same",     egui::Color32::GRAY),
                                    };

                                    ui.label(&diff.table_name);
                                    ui.colored_label(color, status_label);

                                    if diff.column_diffs.is_empty() {
                                        ui.label("—");
                                    } else {
                                        let summary: Vec<String> = diff.column_diffs.iter().map(|cd| {
                                            match (&cd.left_type, &cd.right_type) {
                                                (None, Some(rt))     => format!("+{} ({})", cd.name, rt),
                                                (Some(_), None)      => format!("-{}", cd.name),
                                                (Some(lt), Some(rt)) => format!("{}: {}→{}", cd.name, lt, rt),
                                                _                    => cd.name.clone(),
                                            }
                                        }).collect();
                                        ui.label(summary.join(", "))
                                            .on_hover_text(summary.join("\n"));
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                } else if state.status == SchemaDiffStatus::Running {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                        ui.label("Comparing schemas…");
                    });
                } else if state.status == SchemaDiffStatus::Idle {
                    ui.centered_and_justified(|ui| {
                        ui.label("Select connections/databases and click ▶ Compare");
                    });
                }
            }
        });

    // Process diff run outside the window closure so we can freely borrow tabular.
    if let Some((left_conn_id, left_db, right_conn_id, right_db)) = run_diff {
        let result = crate::connection::compute_schema_diff(
            tabular,
            left_conn_id, &left_db,
            right_conn_id, &right_db,
        );
        if let Some(s) = &mut tabular.schema_diff_state {
            s.result = Some(result);
            s.status = SchemaDiffStatus::Done;
        }
    }

    if !open {
        tabular.show_schema_diff_dialog = false;
    }
}
