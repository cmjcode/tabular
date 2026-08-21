use crate::http_collection::{
    SavedRequest, import_from_postman, import_from_yaak, save_workspaces,
};
use crate::rfd;
use crate::window_egui::Tabular;
/// sidebar_collection.rs
///
/// Renders the "Collections" tab in the main Tabular sidebar.
/// Collections are populated by importing from Yaak (or saved manually via HTTP client).
use eframe::egui;

// ─── Entry point called from app_impl.rs ─────────────────────────────────────

/// Render the entire Collections sidebar content (called inside the ScrollArea).
pub fn render_collections_sidebar(app: &mut Tabular, ui: &mut egui::Ui) {
    // ── Yaak & Postman import dialogs ────────────────────────────────────
    render_yaak_import_dialog(app, ui);
    render_postman_import_dialog(app, ui);

    // ── Search box ────────────────────────────────────────────────────────
    let search_bg = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(30, 32, 42)
    } else {
        egui::Color32::from_rgb(235, 238, 243)
    };
    let available_width = ui.available_width() - 5.0;
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.add_sized(
            [available_width, 24.0],
            egui::TextEdit::singleline(&mut app.collection_search)
                .hint_text("🔍 Filter requests…")
                .background_color(search_bg),
        );
    });

    let filter = app.collection_search.to_lowercase();
    let accent = crate::window_egui::style::theme_accent(ui.ctx());

    // ── 1. HTTP Connections section ───────────────────────────────────────
    let http_conns: Vec<crate::models::structs::ConnectionConfig> = app
        .connections
        .iter()
        .filter(|c| c.connection_type == crate::models::enums::DatabaseType::ApiHttp)
        .cloned()
        .collect();

    let mut conn_to_open: Option<(String, Option<i64>)> = None;
    let mut conn_to_edit: Option<crate::models::structs::ConnectionConfig> = None;
    let mut conn_to_delete: Option<i64> = None;

    let mut conn_to_reorder: Option<(i64, i64)> = None;
    let pointer_released = ui.input(|i| i.pointer.any_released());

    if !http_conns.is_empty() {
        let matching_conns: Vec<&crate::models::structs::ConnectionConfig> = http_conns
            .iter()
            .filter(|c| filter.is_empty() || c.name.to_lowercase().contains(&filter))
            .collect();

        if !matching_conns.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!(" HTTP Connections ({})", matching_conns.len()))
                    .strong(),
            )
            .id_salt("sidebar_http_conns_header")
            .default_open(true)
            .show(ui, |ui| {
                for conn in &matching_conns {
                    let conn_id = conn.id.unwrap_or(0);
                    let is_being_dragged = app.dragged_http_conn_id == Some(conn_id);

                    let inner_resp = ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        let icon = if is_being_dragged { "🖐️" } else { "🌐" };
                        let text_color = if is_being_dragged {
                            accent
                        } else {
                            ui.style().visuals.text_color()
                        };

                        let response = ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{}  {}", icon, conn.name))
                                    .small()
                                    .color(text_color),
                            )
                            .sense(egui::Sense::click_and_drag())
                            .truncate(),
                        );

                        if response.drag_started() && conn_id != 0 {
                            app.dragged_http_conn_id = Some(conn_id);
                        }

                        if response.hovered() {
                            ui.ctx().set_cursor_icon(if is_being_dragged {
                                egui::CursorIcon::Grabbing
                            } else {
                                egui::CursorIcon::PointingHand
                            });

                            if let Some(src_id) = app.dragged_http_conn_id {
                                if src_id != conn_id {
                                    ui.painter().line_segment(
                                        [response.rect.left_top(), response.rect.right_top()],
                                        egui::Stroke::new(2.0, accent),
                                    );
                                    if pointer_released {
                                        conn_to_reorder = Some((src_id, conn_id));
                                    }
                                }
                            }
                        }

                        if response.clicked() && app.dragged_http_conn_id.is_none() {
                            conn_to_open = Some((conn.name.clone(), conn.id));
                        }

                        response
                    });

                    let row_rect = inner_resp.response.rect;
                    let label_response = inner_resp.inner;

                    // Use interact() on the full row rect so right-click is captured reliably across the entire row
                    let row_id = egui::Id::new("http_conn_row").with(conn_id);
                    let row_interact = ui.interact(row_rect, row_id, egui::Sense::click());
                    if (label_response.clicked() || row_interact.clicked())
                        && app.dragged_http_conn_id.is_none()
                    {
                        conn_to_open = Some((conn.name.clone(), conn.id));
                    }
                    let conn_clone = (*conn).clone();
                    let conn_id_opt = conn.id;

                    let conn_clone_edit = conn_clone.clone();
                    label_response.context_menu(|ui| {
                        if ui.button("Edit Connection").clicked() {
                            conn_to_edit = Some(conn_clone_edit);
                            ui.close();
                        }
                        if ui.button("Delete Connection").clicked() {
                            conn_to_delete = conn_id_opt;
                            ui.close();
                        }
                    });
                    row_interact.context_menu(|ui| {
                        if ui.button(" Edit Connection").clicked() {
                            conn_to_edit = Some(conn_clone);
                            ui.close();
                        }
                        if ui.button("Delete Connection").clicked() {
                            conn_to_delete = conn_id_opt;
                            ui.close();
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
    }

    if pointer_released {
        app.dragged_http_conn_id = None;
    }

    if let Some((src_id, target_id)) = conn_to_reorder {
        if let (Some(src_idx), Some(target_idx)) = (
            app.connections.iter().position(|c| c.id == Some(src_id)),
            app.connections.iter().position(|c| c.id == Some(target_id)),
        ) {
            let item = app.connections.remove(src_idx);
            app.connections.insert(target_idx, item);
            app.toasts.info("Reordered HTTP connection");
        }
    }

    if http_conns.is_empty() && app.yaak_workspaces.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(20.0);
            ui.label(
                egui::RichText::new("No HTTP connections or collections yet.")
                    .weak()
                    .small(),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Click ➕ to add a new HTTP connection\nor import a Yaak/Postman collection.",
                )
                .weak()
                .small(),
            );
        });
        return;
    }

    // ── 2. Yaak Workspaces / Collections section ──────────────────────────
    // Active DnD source for HTTP requests or folders
    let active_dnd_source = ui
        .ctx()
        .data(|d| d.get_temp::<HttpDndSource>(egui::Id::new("http_dnd_source")))
        .filter(|s| !matches!(s, HttpDndSource::None));
    let is_dnd_active = active_dnd_source.is_some();

    // Collect workspace ids to avoid borrow issues
    let ws_ids: Vec<String> = app.yaak_workspaces.iter().map(|w| w.id.clone()).collect();

    // Track actions requested from top-level and folder requests
    let mut req_action: Option<(SavedRequest, RequestAction)> = None;
    let mut ws_to_delete: Option<String> = None;
    let mut folder_to_delete: Option<(String, String, String)> = None;
    let mut folder_to_create: Option<(String, Option<String>, String)> = None;
    let mut folder_to_rename: Option<(String, String, String)> = None;
    let mut new_req_target: Option<(String, Option<String>, String)> = None;
    let mut expanded_folders = std::mem::take(&mut app.collection_expanded_folders);

    for ws_id in &ws_ids {
        let ws_idx = match app.yaak_workspaces.iter().position(|w| &w.id == ws_id) {
            Some(i) => i,
            None => continue,
        };

        let ws_name = app.yaak_workspaces[ws_idx].name.clone();
        let request_count: usize = count_workspace_requests(&app.yaak_workspaces[ws_idx]);

        // Check if workspace has any matching requests when filtering
        if !filter.is_empty() && !workspace_has_match(&app.yaak_workspaces[ws_idx], &filter) {
            continue;
        }

        let default_open = app.yaak_workspaces.len() == 1;

        let ws_header_resp = egui::CollapsingHeader::new(
            egui::RichText::new(format!("📁  {}  ({})", ws_name, request_count)).strong(),
        )
        .id_salt(format!("sidebar_coll_ws_{}", ws_id))
        .default_open(default_open)
        .show(ui, |ui| {
            // ── Top-level requests ────────────────────────────────────────
            let top_req_ids: Vec<String> = app.yaak_workspaces[ws_idx]
                .requests
                .iter()
                .map(|r| r.id.clone())
                .collect();

            for req_id in &top_req_ids {
                let req_idx = match app.yaak_workspaces[ws_idx]
                    .requests
                    .iter()
                    .position(|r| &r.id == req_id)
                {
                    Some(i) => i,
                    None => continue,
                };
                let req = app.yaak_workspaces[ws_idx].requests[req_idx].clone();
                if !filter.is_empty()
                    && !req.display_name().to_lowercase().contains(&filter)
                    && !req.url.to_lowercase().contains(&filter)
                {
                    continue;
                }
                if let Some(act) = render_request_row(ui, ws_id, &req, accent, active_dnd_source.as_ref()) {
                    req_action = Some((req, act));
                }
            }

            // ── Folders ───────────────────────────────────────────────────
            let folder_ids: Vec<String> = app.yaak_workspaces[ws_idx]
                .folders
                .iter()
                .map(|f| f.id.clone())
                .collect();

            for folder_id in &folder_ids {
                let folder_idx = match app.yaak_workspaces[ws_idx]
                    .folders
                    .iter()
                    .position(|f| &f.id == folder_id)
                {
                    Some(i) => i,
                    None => continue,
                };
                let folder = app.yaak_workspaces[ws_idx].folders[folder_idx].clone();
                if !filter.is_empty() && !folder_has_match(&folder, &filter) {
                    continue;
                }
                render_folder_node(
                    ui,
                    ws_id,
                    &folder,
                    &mut expanded_folders,
                    &filter,
                    accent,
                    active_dnd_source.as_ref(),
                    &mut req_action,
                    &mut new_req_target,
                    &mut folder_to_create,
                    &mut folder_to_rename,
                    &mut folder_to_delete,
                );
            }
        });

        // Drop target check for workspace root
        if is_dnd_active {
            let row_rect = ws_header_resp.header_response.rect;
            let ptr_pos = ui.ctx().input(|i| i.pointer.hover_pos());
            let pointer_over = ptr_pos.is_some_and(|p| {
                p.y >= row_rect.min.y
                    && p.y <= row_rect.max.y
                    && p.x >= ui.clip_rect().min.x
                    && p.x <= ui.clip_rect().max.x
            });

            if pointer_over {
                let clip = ui.clip_rect();
                let highlight_rect = egui::Rect::from_min_max(
                    egui::pos2(clip.min.x, row_rect.min.y - 1.0),
                    egui::pos2(clip.max.x, row_rect.max.y + 1.0),
                );
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("http_dnd_highlight"),
                ));
                painter.rect_filled(
                    highlight_rect,
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 35),
                );
                painter.rect_stroke(
                    highlight_rect,
                    2.0,
                    egui::Stroke::new(2.0, accent),
                    egui::StrokeKind::Outside,
                );
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("http_dnd_pending_target"),
                        HttpDndTarget::Workspace {
                            ws_id: ws_id.clone(),
                            ws_name: ws_name.clone(),
                        },
                    );
                });
                ui.ctx().request_repaint();
            }
        }

        ws_header_resp.header_response.context_menu(|ui| {
            if ui.button("➕ Add New Http").clicked() {
                new_req_target = Some((ws_id.clone(), None, ws_name.clone()));
                ui.close();
            }
            if ui.button("📁 Add New Folder").clicked() {
                folder_to_create = Some((ws_id.clone(), None, ws_name.clone()));
                ui.close();
            }
            ui.separator();
            if ui.button("🗑 Delete Workspace").clicked() {
                ws_to_delete = Some(ws_id.clone());
                ui.close();
            }
        });
    }

    app.collection_expanded_folders = expanded_folders;

    // Apply deferred actions after rendering loop
    if let Some((name, id)) = conn_to_open {
        open_http_connection_tab(app, name, id);
    }
    if let Some(conn) = conn_to_edit {
        if let Some(conn_id) = conn.id {
            crate::sidebar_database::start_edit_connection(app, conn_id);
        } else {
            app.edit_connection = conn;
            app.show_edit_connection = true;
        }
    }
    if let Some(id) = conn_to_delete {
        let conn_name = app
            .connections
            .iter()
            .find(|c| c.id == Some(id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "HTTP Connection".to_string());
        app.pending_delete_connection = Some((id, conn_name));
    }
    if let Some((req, act)) = req_action {
        match act {
            RequestAction::Open => {
                apply_collection_request_to_active_tab(app, &req);
            }
            RequestAction::Rename => {
                let name = req.display_name();
                app.pending_rename_http_request = Some((req.id.clone(), name.clone(), name));
            }
            RequestAction::Duplicate => {
                duplicate_request_in_workspaces(&mut app.yaak_workspaces, &req);
                save_workspaces(&app.yaak_workspaces);
                app.toasts
                    .info(format!("Duplicated request '{}'", req.display_name()));
            }
            RequestAction::Delete => {
                app.pending_delete_http_request = Some((req.id.clone(), req.display_name()));
            }
        }
    }
    if let Some((ws_id, folder_id, folder_name)) = folder_to_delete {
        app.pending_delete_http_folder = Some((ws_id, folder_id, folder_name));
    }
    if let Some((ws_id, parent_id_opt, parent_name)) = folder_to_create {
        app.pending_create_http_folder = Some((ws_id, parent_id_opt, parent_name, String::new()));
    }
    if let Some((ws_id, folder_id, folder_name)) = folder_to_rename {
        app.pending_rename_http_folder = Some((ws_id, folder_id, folder_name.clone(), folder_name));
    }
    if let Some(ws_id) = ws_to_delete {
        let ws_name = app
            .yaak_workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "Workspace".to_string());
        app.pending_delete_http_workspace = Some((ws_id, ws_name));
    }
    if let Some((ws_id, folder_id_opt, parent_name)) = new_req_target {
        let new_req_id = format!("sr_{}", chrono::Utc::now().timestamp_millis());
        let new_req = SavedRequest {
            id: new_req_id,
            workspace_id: ws_id.clone(),
            folder_id: folder_id_opt.clone(),
            name: "New Request".to_string(),
            url: "".to_string(),
            method: crate::models::structs::HttpMethod::GET,
            ..Default::default()
        };

        if let Some(ws) = app.yaak_workspaces.iter_mut().find(|w| w.id == ws_id) {
            if let Some(f_id) = &folder_id_opt {
                fn add_req_to_folder(
                    folders: &mut [crate::http_collection::HttpFolder],
                    f_id: &str,
                    req: SavedRequest,
                ) -> bool {
                    for folder in folders.iter_mut() {
                        if folder.id == f_id {
                            folder.requests.push(req);
                            return true;
                        }
                        if add_req_to_folder(&mut folder.children, f_id, req.clone()) {
                            return true;
                        }
                    }
                    false
                }
                add_req_to_folder(&mut ws.folders, f_id, new_req.clone());
            } else {
                ws.requests.push(new_req.clone());
            }
            save_workspaces(&app.yaak_workspaces);
            apply_collection_request_to_active_tab(app, &new_req);
            app.toasts
                .success(format!("Added new HTTP request to '{}'", parent_name));
        }
    }

    // Floating ghost badge while dragging
    if let Some(ref source) = active_dnd_source {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if let Some(pos) = ui.ctx().input(|i| i.pointer.hover_pos()) {
            let badge_text = match source {
                HttpDndSource::Request { req_name, .. } => format!("📄 {}", req_name),
                HttpDndSource::Folder { folder_name, .. } => format!("📁 {}", folder_name),
                HttpDndSource::None => String::new(),
            };
            if !badge_text.is_empty() {
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::new("http_dnd_badge"),
                ));
                let font_id = egui::FontId::proportional(12.0);
                let galley = painter.layout_no_wrap(badge_text, font_id, ui.visuals().text_color());
                let badge_rect = egui::Rect::from_min_size(
                    pos + egui::vec2(12.0, -10.0),
                    galley.size() + egui::vec2(12.0, 8.0),
                );
                painter.rect_filled(
                    badge_rect,
                    4.0,
                    if ui.visuals().dark_mode {
                        egui::Color32::from_rgb(35, 38, 48)
                    } else {
                        egui::Color32::from_rgb(240, 243, 248)
                    },
                );
                painter.rect_stroke(
                    badge_rect,
                    4.0,
                    egui::Stroke::new(1.0, accent),
                    egui::StrokeKind::Outside,
                );
                painter.galley(
                    badge_rect.min + egui::vec2(6.0, 4.0),
                    galley,
                    ui.visuals().text_color(),
                );
            }
        }
    }

    if pointer_released {
        app.dragged_http_conn_id = None;

        let pending_target = ui
            .ctx()
            .data(|d| d.get_temp::<HttpDndTarget>(egui::Id::new("http_dnd_pending_target")))
            .filter(|t| !matches!(t, HttpDndTarget::None));

        if let (Some(src), Some(target)) = (active_dnd_source, pending_target) {
            match src {
                HttpDndSource::Request {
                    req_id,
                    req_name,
                    ..
                } => match target {
                    HttpDndTarget::Workspace { ws_id, ws_name } => {
                        if crate::http_collection::move_request(
                            &mut app.yaak_workspaces,
                            &req_id,
                            &ws_id,
                            None,
                        ) {
                            app.toasts.success(format!(
                                "Moved request '{}' to workspace '{}'",
                                req_name, ws_name
                            ));
                        }
                    }
                    HttpDndTarget::Folder {
                        ws_id,
                        folder_id,
                        folder_name,
                    } => {
                        if crate::http_collection::move_request(
                            &mut app.yaak_workspaces,
                            &req_id,
                            &ws_id,
                            Some(&folder_id),
                        ) {
                            app.toasts.success(format!(
                                "Moved request '{}' to folder '{}'",
                                req_name, folder_name
                            ));
                        }
                    }
                    HttpDndTarget::None => {}
                },
                HttpDndSource::Folder {
                    folder_id,
                    folder_name,
                    ..
                } => match target {
                    HttpDndTarget::Workspace { ws_id, ws_name } => {
                        if crate::http_collection::move_folder(
                            &mut app.yaak_workspaces,
                            &folder_id,
                            &ws_id,
                            None,
                        ) {
                            app.toasts.success(format!(
                                "Moved folder '{}' to workspace '{}'",
                                folder_name, ws_name
                            ));
                        }
                    }
                    HttpDndTarget::Folder {
                        ws_id,
                        folder_id: target_folder_id,
                        folder_name: target_folder_name,
                    } => {
                        if crate::http_collection::move_folder(
                            &mut app.yaak_workspaces,
                            &folder_id,
                            &ws_id,
                            Some(&target_folder_id),
                        ) {
                            app.toasts.success(format!(
                                "Moved folder '{}' into '{}'",
                                folder_name, target_folder_name
                            ));
                        }
                    }
                    HttpDndTarget::None => {}
                },
                HttpDndSource::None => {}
            }
        }

        ui.ctx().data_mut(|d| {
            d.remove_temp::<HttpDndSource>(egui::Id::new("http_dnd_source"));
            d.remove_temp::<HttpDndTarget>(egui::Id::new("http_dnd_pending_target"));
        });
    }

    if let Some((src_id, target_id)) = conn_to_reorder {
        if let (Some(src_idx), Some(target_idx)) = (
            app.connections.iter().position(|c| c.id == Some(src_id)),
            app.connections.iter().position(|c| c.id == Some(target_id)),
        ) {
            let item = app.connections.remove(src_idx);
            app.connections.insert(target_idx, item);
            app.toasts.info("Reordered HTTP connection");
        }
    }
}

/// Open an HTTP Client tab pre-assigned to the given HTTP connection.
fn open_http_connection_tab(app: &mut Tabular, conn_name: String, connection_id: Option<i64>) {
    let existing_tab = connection_id.and_then(|conn_id| {
        app.query_tabs
            .iter()
            .position(|t| t.connection_id == Some(conn_id) && t.http_client_state.is_some())
    });
    if let Some(idx) = existing_tab {
        app.active_tab_index = idx;
        app.current_connection_id = connection_id;
        return;
    }

    // Reuse active tab if it's an unused default blank tab
    if let Some(tab) = app.query_tabs.get_mut(app.active_tab_index)
        && tab.content.trim().is_empty()
        && !tab.is_modified
        && !tab.has_executed_query
        && tab.connection_id.is_none()
        && tab.http_client_state.is_none()
        && tab.redis_browser_state.is_none()
        && tab.diagram_state.is_none()
    {
        tab.title = conn_name;
        tab.connection_id = connection_id;
        let loaded_state = connection_id
            .and_then(crate::http_client::load_http_state)
            .unwrap_or_default();
        tab.http_client_state = Some(loaded_state);
        app.current_connection_id = connection_id;
        return;
    }

    crate::editor::create_new_tab_with_connection(app, conn_name, String::new(), connection_id);
    let loaded_state = connection_id
        .and_then(crate::http_client::load_http_state)
        .unwrap_or_default();
    if let Some(tab) = app.query_tabs.get_mut(app.active_tab_index) {
        tab.http_client_state = Some(loaded_state);
    }
}

// ─── Yaak Import Dialog ───────────────────────────────────────────────────────

fn render_yaak_import_dialog(app: &mut Tabular, _ui: &mut egui::Ui) {
    if !app.show_yaak_import_dialog {
        return;
    }
    // Fire the file picker immediately (blocking call; rfd shows native file picker)
    app.show_yaak_import_dialog = false;

    let default_path = dirs::data_dir()
        .map(|d| d.join("app.yaak.desktop"))
        .unwrap_or_default();

    if let Some(path) = rfd::FileDialog::new()
        .set_title("Select Yaak db.sqlite file")
        .set_directory(&default_path)
        .add_filter("SQLite database", &["sqlite", "sqlite3", "db"])
        .pick_file()
    {
        match import_from_yaak(&path) {
            Ok(result) => {
                let imported_ids: std::collections::HashSet<String> =
                    result.workspaces.iter().map(|w| w.id.clone()).collect();

                // Merge: replace existing workspaces that were re-imported
                app.yaak_workspaces
                    .retain(|w| !imported_ids.contains(&w.id));
                app.yaak_workspaces.extend(result.workspaces.clone());
                app.yaak_workspaces.sort_by(|a, b| a.name.cmp(&b.name));
                save_workspaces(&result.workspaces);

                let msg = format!(
                    "Imported {} requests from {} workspace(s)",
                    result.total_requests,
                    imported_ids.len()
                );
                app.toasts.success(msg);
                for w in result.warnings {
                    app.toasts.warning(w);
                }

                // Auto-switch to HTTP Clients tab so user sees the result
                app.selected_menu = "HTTP Clients".to_string();
            }
            Err(e) => {
                app.toasts.error(format!("Yaak import failed: {e}"));
            }
        }
    }
}

// ─── Postman Import Dialog ────────────────────────────────────────────────────

fn render_postman_import_dialog(app: &mut Tabular, _ui: &mut egui::Ui) {
    if !app.show_postman_import_dialog {
        return;
    }
    app.show_postman_import_dialog = false;

    if let Some(path) = rfd::FileDialog::new()
        .set_title("Select Postman Collection / Environment JSON file")
        .add_filter("Postman JSON", &["json"])
        .pick_file()
    {
        match import_from_postman(&path) {
            Ok(result) => {
                let imported_ids: std::collections::HashSet<String> =
                    result.workspaces.iter().map(|w| w.id.clone()).collect();

                app.yaak_workspaces
                    .retain(|w| !imported_ids.contains(&w.id));
                app.yaak_workspaces.extend(result.workspaces.clone());
                app.yaak_workspaces.sort_by(|a, b| a.name.cmp(&b.name));
                save_workspaces(&result.workspaces);

                let msg = format!(
                    "Imported {} requests from Postman ({})",
                    result.total_requests,
                    result
                        .workspaces
                        .first()
                        .map(|w| w.name.as_str())
                        .unwrap_or("Collection")
                );
                app.toasts.success(msg);
                for w in result.warnings {
                    app.toasts.warning(w);
                }

                app.selected_menu = "HTTP Clients".to_string();
            }
            Err(e) => {
                app.toasts.error(format!("Postman import failed: {e}"));
            }
        }
    }
}

// ─── Tree rendering helpers ───────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum HttpDndSource {
    #[default]
    None,
    Request {
        req_id: String,
        req_name: String,
        from_ws_id: String,
    },
    Folder {
        folder_id: String,
        folder_name: String,
        from_ws_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum HttpDndTarget {
    #[default]
    None,
    Workspace {
        ws_id: String,
        ws_name: String,
    },
    Folder {
        ws_id: String,
        folder_id: String,
        folder_name: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestAction {
    Open,
    Rename,
    Delete,
    Duplicate,
}

fn is_descendant_folder(folder: &crate::http_collection::HttpFolder, target_id: &str) -> bool {
    for child in &folder.children {
        if child.id == target_id || is_descendant_folder(child, target_id) {
            return true;
        }
    }
    false
}

fn render_folder_node(
    ui: &mut egui::Ui,
    ws_id: &str,
    folder: &crate::http_collection::HttpFolder,
    expanded_folders: &mut std::collections::HashSet<String>,
    filter: &str,
    accent: egui::Color32,
    active_dnd_source: Option<&HttpDndSource>,
    req_action_out: &mut Option<(SavedRequest, RequestAction)>,
    new_req_target: &mut Option<(String, Option<String>, String)>,
    folder_to_create: &mut Option<(String, Option<String>, String)>,
    folder_to_rename: &mut Option<(String, String, String)>,
    folder_to_delete: &mut Option<(String, String, String)>,
) {
    let is_expanded = expanded_folders.contains(&folder.id) || !filter.is_empty();
    let is_being_dragged = active_dnd_source.is_some_and(|src| {
        matches!(src, HttpDndSource::Folder { folder_id, .. } if folder_id == &folder.id)
    });

    let mut toggle_clicked = false;
    let mut row_clicked = false;

    let inner_resp = ui.horizontal(|ui| {
        ui.add_space(2.0);

        // Dedicated triangle toggle (consistent with database & query sidebar trees)
        if Tabular::triangle_toggle(ui, is_expanded).clicked() {
            toggle_clicked = true;
        }

        let text_color = if is_being_dragged {
            accent
        } else {
            ui.style().visuals.text_color()
        };

        let lbl = ui.add(
            egui::Label::new(
                egui::RichText::new(format!("📂  {}", folder.name))
                    .small()
                    .strong()
                    .color(text_color),
            )
            .sense(egui::Sense::click_and_drag())
            .truncate(),
        );

        if lbl.hovered() {
            ui.ctx().set_cursor_icon(if is_being_dragged {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::PointingHand
            });
        }

        if lbl.drag_started() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("http_dnd_source"),
                    HttpDndSource::Folder {
                        folder_id: folder.id.clone(),
                        folder_name: folder.name.clone(),
                        from_ws_id: ws_id.to_string(),
                    },
                );
            });
        }

        if lbl.clicked() && !is_being_dragged {
            row_clicked = true;
        }

        lbl
    });

    let folder_row_rect = inner_resp.response.rect;
    let label_resp = inner_resp.inner;

    if toggle_clicked || row_clicked {
        if expanded_folders.contains(&folder.id) {
            expanded_folders.remove(&folder.id);
        } else {
            expanded_folders.insert(folder.id.clone());
        }
    }

    if let Some(src) = active_dnd_source {
        let ptr_pos = ui.ctx().input(|i| i.pointer.hover_pos());
        let pointer_over = ptr_pos.is_some_and(|p| {
            p.y >= folder_row_rect.min.y
                && p.y <= folder_row_rect.max.y
                && p.x >= ui.clip_rect().min.x
                && p.x <= ui.clip_rect().max.x
        });

        if pointer_over {
            let is_valid = match src {
                HttpDndSource::Request { .. } => true,
                HttpDndSource::Folder {
                    folder_id: src_fid, ..
                } => src_fid != &folder.id && !is_descendant_folder(folder, src_fid),
                HttpDndSource::None => false,
            };

            if is_valid {
                let clip = ui.clip_rect();
                let highlight_rect = egui::Rect::from_min_max(
                    egui::pos2(clip.min.x, folder_row_rect.min.y - 1.0),
                    egui::pos2(clip.max.x, folder_row_rect.max.y + 1.0),
                );
                let painter = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("http_dnd_highlight"),
                ));
                painter.rect_filled(
                    highlight_rect,
                    2.0,
                    egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 35),
                );
                painter.rect_stroke(
                    highlight_rect,
                    2.0,
                    egui::Stroke::new(2.0, accent),
                    egui::StrokeKind::Outside,
                );
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("http_dnd_pending_target"),
                        HttpDndTarget::Folder {
                            ws_id: ws_id.to_string(),
                            folder_id: folder.id.clone(),
                            folder_name: folder.name.clone(),
                        },
                    );
                });
                ui.ctx().request_repaint();
            }
        }
    }

    label_resp.context_menu(|ui| {
        if ui.button("➕ Add New Http").clicked() {
            *new_req_target = Some((
                ws_id.to_string(),
                Some(folder.id.clone()),
                folder.name.clone(),
            ));
            ui.close();
        }
        if ui.button("📁 Add New Subfolder").clicked() {
            *folder_to_create = Some((
                ws_id.to_string(),
                Some(folder.id.clone()),
                folder.name.clone(),
            ));
            ui.close();
        }
        if ui.button("✏️ Rename Folder").clicked() {
            *folder_to_rename = Some((
                ws_id.to_string(),
                folder.id.clone(),
                folder.name.clone(),
            ));
            ui.close();
        }
        ui.separator();
        if ui.button("🗑 Delete Folder").clicked() {
            *folder_to_delete = Some((ws_id.to_string(), folder.id.clone(), folder.name.clone()));
            ui.close();
        }
    });

    if is_expanded {
        ui.indent(format!("fld_body_{}", folder.id), |ui| {
            for req in &folder.requests {
                if !filter.is_empty()
                    && !req.display_name().to_lowercase().contains(filter)
                    && !req.url.to_lowercase().contains(filter)
                {
                    continue;
                }
                if let Some(act) = render_request_row(ui, ws_id, req, accent, active_dnd_source) {
                    *req_action_out = Some((req.clone(), act));
                }
            }
            for child in &folder.children {
                if !filter.is_empty() && !folder_has_match(child, filter) {
                    continue;
                }
                render_folder_node(
                    ui,
                    ws_id,
                    child,
                    expanded_folders,
                    filter,
                    accent,
                    active_dnd_source,
                    req_action_out,
                    new_req_target,
                    folder_to_create,
                    folder_to_rename,
                    folder_to_delete,
                );
            }
        });
    }
}

/// Render a single saved-request row. Returns Some(RequestAction) if the user interacted with it.
fn render_request_row(
    ui: &mut egui::Ui,
    ws_id: &str,
    req: &SavedRequest,
    accent: egui::Color32,
    active_dnd_source: Option<&HttpDndSource>,
) -> Option<RequestAction> {
    let method_color = method_color_for(req.method.label());
    let mut action = None;
    let display_name = req.display_name();

    let is_being_dragged = active_dnd_source.is_some_and(|src| {
        matches!(src, HttpDndSource::Request { req_id, .. } if req_id == &req.id)
    });

    let inner_resp = ui.horizontal(|ui| {
        ui.add_space(4.0);
        // Colored method badge
        ui.label(
            egui::RichText::new(req.method.label())
                .color(method_color)
                .monospace()
                .small()
                .strong(),
        );
        // Request name or endpoint URL, clickable and draggable
        let mut lbl = ui.add(
            egui::Label::new(
                egui::RichText::new(&display_name)
                    .small()
                    .color(if is_being_dragged {
                        accent
                    } else {
                        ui.style().visuals.text_color()
                    }),
            )
            .sense(egui::Sense::click_and_drag())
            .truncate(),
        );
        if lbl.hovered() {
            ui.ctx().set_cursor_icon(if is_being_dragged {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::PointingHand
            });
        }
        lbl = lbl.on_hover_text(&req.url);
        if lbl.drag_started() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(
                    egui::Id::new("http_dnd_source"),
                    HttpDndSource::Request {
                        req_id: req.id.clone(),
                        req_name: display_name.clone(),
                        from_ws_id: ws_id.to_string(),
                    },
                );
            });
        }
        if lbl.clicked() && !is_being_dragged {
            action = Some(RequestAction::Open);
        }
        lbl
    });

    let row_rect = inner_resp.response.rect;
    let label_response = inner_resp.inner;
    let row_id = egui::Id::new("http_req_row").with(&req.id);
    let row_interact = ui.interact(row_rect, row_id, egui::Sense::click_and_drag());

    if row_interact.drag_started() {
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("http_dnd_source"),
                HttpDndSource::Request {
                    req_id: req.id.clone(),
                    req_name: display_name.clone(),
                    from_ws_id: ws_id.to_string(),
                },
            );
        });
    }

    if (label_response.clicked() || row_interact.clicked())
        && action.is_none()
        && !is_being_dragged
    {
        action = Some(RequestAction::Open);
    }

    label_response.context_menu(|ui| {
        if ui.button("Open Request").clicked() {
            action = Some(RequestAction::Open);
            ui.close();
        }
        if ui.button("Rename Request").clicked() {
            action = Some(RequestAction::Rename);
            ui.close();
        }
        if ui.button("Duplicate Request").clicked() {
            action = Some(RequestAction::Duplicate);
            ui.close();
        }
        if ui.button("Delete Request").clicked() {
            action = Some(RequestAction::Delete);
            ui.close();
        }
    });

    row_interact.context_menu(|ui| {
        if ui.button("Open Request").clicked() {
            action = Some(RequestAction::Open);
            ui.close();
        }
        if ui.button("Rename Request").clicked() {
            action = Some(RequestAction::Rename);
            ui.close();
        }
        if ui.button("Duplicate Request").clicked() {
            action = Some(RequestAction::Duplicate);
            ui.close();
        }
        if ui.button("Delete Request").clicked() {
            action = Some(RequestAction::Delete);
            ui.close();
        }
    });

    action
}

pub(crate) fn rename_request_in_workspaces(
    workspaces: &mut [crate::http_collection::HttpWorkspace],
    req_id: &str,
    new_name: &str,
) -> bool {
    for ws in workspaces.iter_mut() {
        if let Some(req) = ws.requests.iter_mut().find(|r| r.id == req_id) {
            req.name = new_name.to_string();
            return true;
        }
        fn rename_in_folders(
            folders: &mut [crate::http_collection::HttpFolder],
            req_id: &str,
            new_name: &str,
        ) -> bool {
            for f in folders.iter_mut() {
                if let Some(req) = f.requests.iter_mut().find(|r| r.id == req_id) {
                    req.name = new_name.to_string();
                    return true;
                }
                if rename_in_folders(&mut f.children, req_id, new_name) {
                    return true;
                }
            }
            false
        }
        if rename_in_folders(&mut ws.folders, req_id, new_name) {
            return true;
        }
    }
    false
}

pub(crate) fn delete_request_from_workspaces(
    workspaces: &mut [crate::http_collection::HttpWorkspace],
    req_id: &str,
) -> bool {
    for ws in workspaces.iter_mut() {
        if let Some(pos) = ws.requests.iter().position(|r| r.id == req_id) {
            ws.requests.remove(pos);
            return true;
        }
        if remove_from_folders(&mut ws.folders, req_id) {
            return true;
        }
    }
    false
}

fn remove_from_folders(folders: &mut [crate::http_collection::HttpFolder], req_id: &str) -> bool {
    for f in folders.iter_mut() {
        if let Some(pos) = f.requests.iter().position(|r| r.id == req_id) {
            f.requests.remove(pos);
            return true;
        }
        if remove_from_folders(&mut f.children, req_id) {
            return true;
        }
    }
    false
}

fn duplicate_request_in_workspaces(
    workspaces: &mut [crate::http_collection::HttpWorkspace],
    req: &SavedRequest,
) {
    let mut new_req = req.clone();
    new_req.id = format!("sr_{}", chrono::Utc::now().timestamp_millis());
    new_req.name = format!("{} (Copy)", req.display_name());

    for ws in workspaces.iter_mut() {
        if ws.id == req.workspace_id {
            if let Some(ref folder_id) = req.folder_id {
                fn add_to_folder(
                    folders: &mut [crate::http_collection::HttpFolder],
                    folder_id: &str,
                    new_req: SavedRequest,
                ) -> bool {
                    for f in folders.iter_mut() {
                        if f.id == folder_id {
                            f.requests.push(new_req);
                            return true;
                        }
                        if add_to_folder(&mut f.children, folder_id, new_req.clone()) {
                            return true;
                        }
                    }
                    false
                }
                add_to_folder(&mut ws.folders, folder_id, new_req.clone());
            } else {
                ws.requests.push(new_req.clone());
            }
            break;
        }
    }
}

pub(crate) fn delete_folder_from_workspaces(
    workspaces: &mut [crate::http_collection::HttpWorkspace],
    ws_id: &str,
    folder_id: &str,
) {
    if let Some(ws) = workspaces.iter_mut().find(|w| w.id == ws_id) {
        fn remove_folder(
            folders: &mut Vec<crate::http_collection::HttpFolder>,
            folder_id: &str,
        ) -> bool {
            if let Some(pos) = folders.iter().position(|f| f.id == folder_id) {
                folders.remove(pos);
                return true;
            }
            for f in folders.iter_mut() {
                if remove_folder(&mut f.children, folder_id) {
                    return true;
                }
            }
            false
        }
        remove_folder(&mut ws.folders, folder_id);
    }
}

// ─── Apply saved request to the active HTTP client tab ───────────────────────

/// Load a saved request into an HTTP client tab.
/// Automatically creates a new tab for different requests or switches to an existing tab if already open.
fn apply_collection_request_to_active_tab(app: &mut Tabular, req: &SavedRequest) {
    let display_title = req.display_name();

    // 1. If a tab for this exact saved request is already open, switch to that tab and refresh it
    if let Some(idx) = app.query_tabs.iter().position(|t| {
        t.http_client_state
            .as_ref()
            .and_then(|st| st.saved_request_id.as_deref())
            == Some(req.id.as_str())
    }) {
        app.active_tab_index = idx;
        if let Some(tab) = app.query_tabs.get_mut(idx) {
            tab.title = display_title.clone();
            if let Some(http_state) = tab.http_client_state.as_mut() {
                crate::http_collection::apply_saved_request(req, http_state);
            }
        }
        app.toasts.success(format!("Loaded: {}", display_title));
        return;
    }

    // 2. If active tab is an unused default empty tab, transform it into an HTTP Client tab for this request
    if let Some(tab) = app.query_tabs.get_mut(app.active_tab_index)
        && tab.content.trim().is_empty()
        && !tab.is_modified
        && !tab.has_executed_query
        && tab.connection_id.is_none()
        && tab.http_client_state.is_none()
        && tab.redis_browser_state.is_none()
        && tab.diagram_state.is_none()
    {
        tab.title = display_title.clone();
        let mut http_state = crate::models::structs::HttpClientState::default();
        crate::http_collection::apply_saved_request(req, &mut http_state);
        tab.http_client_state = Some(http_state);
        app.toasts.success(format!("Loaded: {}", display_title));
        return;
    }

    // 3. Otherwise, create a new HTTP Client tab for this request
    crate::editor::create_new_tab(app, display_title.clone(), String::new());
    let new_idx = app.active_tab_index;
    if let Some(tab) = app.query_tabs.get_mut(new_idx) {
        let mut http_state = crate::models::structs::HttpClientState::default();
        crate::http_collection::apply_saved_request(req, &mut http_state);
        tab.http_client_state = Some(http_state);
        app.toasts.success(format!("Loaded: {}", display_title));
    }
}

// ─── Filter helpers ───────────────────────────────────────────────────────────

fn workspace_has_match(ws: &crate::http_collection::HttpWorkspace, filter: &str) -> bool {
    for req in &ws.requests {
        if req.display_name().to_lowercase().contains(filter)
            || req.url.to_lowercase().contains(filter)
        {
            return true;
        }
    }
    for folder in &ws.folders {
        if folder_has_match(folder, filter) {
            return true;
        }
    }
    false
}

fn folder_has_match(folder: &crate::http_collection::HttpFolder, filter: &str) -> bool {
    for req in &folder.requests {
        if req.display_name().to_lowercase().contains(filter)
            || req.url.to_lowercase().contains(filter)
        {
            return true;
        }
    }
    for child in &folder.children {
        if folder_has_match(child, filter) {
            return true;
        }
    }
    false
}

fn count_workspace_requests(ws: &crate::http_collection::HttpWorkspace) -> usize {
    let top = ws.requests.len();
    let in_folders: usize = ws.folders.iter().map(count_folder_requests).sum();
    top + in_folders
}

fn count_folder_requests(folder: &crate::http_collection::HttpFolder) -> usize {
    folder.requests.len()
        + folder
            .children
            .iter()
            .map(count_folder_requests)
            .sum::<usize>()
}

// ─── Method colors ────────────────────────────────────────────────────────────

fn method_color_for(method: &str) -> egui::Color32 {
    match method {
        "GET" => egui::Color32::from_rgb(97, 175, 254),
        "POST" => egui::Color32::from_rgb(73, 204, 144),
        "PUT" => egui::Color32::from_rgb(252, 161, 48),
        "DELETE" => egui::Color32::from_rgb(249, 62, 62),
        "PATCH" => egui::Color32::from_rgb(80, 227, 194),
        "HEAD" => egui::Color32::from_rgb(144, 150, 160),
        _ => egui::Color32::from_rgb(200, 200, 200),
    }
}
