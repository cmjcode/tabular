use crate::http_collection::{
    SavedRequest, import_from_postman, import_from_yaak, save_workspaces,
};
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
    // Collect workspace ids to avoid borrow issues
    let ws_ids: Vec<String> = app.yaak_workspaces.iter().map(|w| w.id.clone()).collect();

    // Track actions requested from top-level and folder requests
    let mut req_action: Option<(SavedRequest, RequestAction)> = None;
    let mut ws_to_delete: Option<String> = None;
    let mut folder_to_delete: Option<(String, String, String)> = None;
    let mut new_req_target: Option<(String, Option<String>, String)> = None;

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
                if let Some(act) = render_request_row(ui, &req, accent) {
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
                    &filter,
                    accent,
                    &mut req_action,
                    &mut new_req_target,
                    &mut folder_to_delete,
                );
            }
        });

        ws_header_resp.header_response.context_menu(|ui| {
            if ui.button("Add New Http").clicked() {
                new_req_target = Some((ws_id.clone(), None, ws_name.clone()));
                ui.close();
            }
            if ui.button("Delete Workspace").clicked() {
                ws_to_delete = Some(ws_id.clone());
                ui.close();
            }
        });
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestAction {
    Open,
    Delete,
    Duplicate,
}

fn render_folder_node(
    ui: &mut egui::Ui,
    ws_id: &str,
    folder: &crate::http_collection::HttpFolder,
    filter: &str,
    accent: egui::Color32,
    req_action_out: &mut Option<(SavedRequest, RequestAction)>,
    new_req_target: &mut Option<(String, Option<String>, String)>,
    folder_to_delete: &mut Option<(String, String, String)>,
) {
    let folder_resp =
        egui::CollapsingHeader::new(egui::RichText::new(format!("📂  {}", folder.name)))
            .id_salt(format!("sidebar_coll_folder_{}", folder.id))
            .show(ui, |ui| {
                for req in &folder.requests {
                    if !filter.is_empty()
                        && !req.display_name().to_lowercase().contains(filter)
                        && !req.url.to_lowercase().contains(filter)
                    {
                        continue;
                    }
                    if let Some(act) = render_request_row(ui, req, accent) {
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
                        filter,
                        accent,
                        req_action_out,
                        new_req_target,
                        folder_to_delete,
                    );
                }
            });

    folder_resp.header_response.context_menu(|ui| {
        if ui.button("Add New Http").clicked() {
            *new_req_target = Some((
                ws_id.to_string(),
                Some(folder.id.clone()),
                folder.name.clone(),
            ));
            ui.close();
        }
        if ui.button("Delete Folder").clicked() {
            *folder_to_delete = Some((ws_id.to_string(), folder.id.clone(), folder.name.clone()));
            ui.close();
        }
    });
}

/// Render a single saved-request row. Returns Some(RequestAction) if the user interacted with it.
fn render_request_row(
    ui: &mut egui::Ui,
    req: &SavedRequest,
    _accent: egui::Color32,
) -> Option<RequestAction> {
    let method_color = method_color_for(req.method.label());
    let mut action = None;
    let display_name = req.display_name();

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
        // Request name or endpoint URL, clickable
        let mut lbl = ui.add(
            egui::Label::new(
                egui::RichText::new(&display_name)
                    .small()
                    .color(ui.style().visuals.text_color()),
            )
            .sense(egui::Sense::click())
            .truncate(),
        );
        if lbl.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        lbl = lbl.on_hover_text(&req.url);
        if lbl.clicked() {
            action = Some(RequestAction::Open);
        }
        lbl
    });

    let row_rect = inner_resp.response.rect;
    let label_response = inner_resp.inner;
    let row_id = egui::Id::new("http_req_row").with(&req.id);
    let row_interact = ui.interact(row_rect, row_id, egui::Sense::click());

    if (label_response.clicked() || row_interact.clicked()) && action.is_none() {
        action = Some(RequestAction::Open);
    }

    label_response.context_menu(|ui| {
        if ui.button("Open Request").clicked() {
            action = Some(RequestAction::Open);
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
/// Automatically creates or switches to an HTTP client tab if one isn't currently active.
fn apply_collection_request_to_active_tab(app: &mut Tabular, req: &SavedRequest) {
    let display_title = req.display_name();

    // 1. If currently active tab is already an HTTP Client tab, load request into it and update title
    if let Some(tab) = app.query_tabs.get_mut(app.active_tab_index)
        && tab.http_client_state.is_some()
    {
        tab.title = display_title.clone();
        if let Some(http_state) = tab.http_client_state.as_mut() {
            crate::http_collection::apply_saved_request(req, http_state);
            app.toasts.success(format!("Loaded: {}", display_title));
            return;
        }
    }

    // 2. If active tab is an unused default empty tab, transform it into an HTTP Client tab
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

    // 3. Otherwise, if another open tab IS an HTTP Client tab, switch active tab index to that tab
    if let Some(idx) = app
        .query_tabs
        .iter()
        .position(|t| t.http_client_state.is_some())
    {
        app.active_tab_index = idx;
        if let Some(tab) = app.query_tabs.get_mut(idx) {
            tab.title = display_title.clone();
            if let Some(http_state) = tab.http_client_state.as_mut() {
                crate::http_collection::apply_saved_request(req, http_state);
                app.toasts.success(format!("Loaded: {}", display_title));
                return;
            }
        }
    }

    // 4. If no HTTP Client tab exists, create a new HTTP Client tab
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
