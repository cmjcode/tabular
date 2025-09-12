use eframe::egui;
use egui::text::{CCursor, CCursorRange};
use egui::text_edit::TextEditState;
// Removed egui_code_editor; using basic TextEdit and custom theme enum
use lapce_core::buffer::Buffer;
// syntax highlighting module temporarily disabled
use log::debug;

use crate::{
    connection, data_table, directory, editor, editor_autocomplete, models, sidebar_history, sidebar_query,
    window_egui,
};

// Tab management methods
pub(crate) fn create_new_tab(
    tabular: &mut window_egui::Tabular,
    title: String,
    content: String,
) -> usize {
    let tab_id = tabular.next_tab_id;
    tabular.next_tab_id += 1;

    let new_tab = models::structs::QueryTab {
        title,
        content: content.clone(),
        file_path: None,
        is_saved: false,
        is_modified: false,
        connection_id: None,       // No connection assigned by default
        database_name: None,       // No database assigned by default
        has_executed_query: false, // New tab hasn't executed any query yet
        result_headers: Vec::new(),
        result_rows: Vec::new(),
        result_all_rows: Vec::new(),
        result_table_name: String::new(),
        is_table_browse_mode: false,
        current_page: 0,
        page_size: 100, // default page size aligns with global default
        total_rows: 0,
        base_query: String::new(), // Empty base query initially
    };

    tabular.query_tabs.push(new_tab);
    let new_index = tabular.query_tabs.len() - 1;
    tabular.active_tab_index = new_index;

    // Update editor with new tab content
    tabular.editor_text = content;
    // Clear global result state so a fresh tab starts clean (no lingering table below)
    tabular.current_table_headers.clear();
    tabular.current_table_data.clear();
    tabular.all_table_data.clear();
    tabular.current_table_name.clear();
    tabular.total_rows = 0;
    tabular.is_table_browse_mode = false;

    tab_id
}

pub(crate) fn create_new_tab_with_connection(
    tabular: &mut window_egui::Tabular,
    title: String,
    content: String,
    connection_id: Option<i64>,
) -> usize {
    create_new_tab_with_connection_and_database(tabular, title, content, connection_id, None)
}

pub(crate) fn create_new_tab_with_connection_and_database(
    tabular: &mut window_egui::Tabular,
    title: String,
    content: String,
    connection_id: Option<i64>,
    database_name: Option<String>,
) -> usize {
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
        result_headers: Vec::new(),
        result_rows: Vec::new(),
        result_all_rows: Vec::new(),
        result_table_name: String::new(),
        is_table_browse_mode: false,
        current_page: 0,
        page_size: 100, // default page size aligns with global default
        total_rows: 0,
        base_query: String::new(), // Empty base query initially
    };

    tabular.query_tabs.push(new_tab);
    let new_index = tabular.query_tabs.len() - 1;
    tabular.active_tab_index = new_index;

    // Update editor with new tab content
    tabular.editor_text = content;
    // Clear global result state for a clean start on this new tab
    tabular.current_table_headers.clear();
    tabular.current_table_data.clear();
    tabular.all_table_data.clear();
    tabular.current_table_name.clear();
    tabular.total_rows = 0;
    tabular.is_table_browse_mode = false;

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
            // Persist current global result state into the tab before switching
            current_tab.result_headers = tabular.current_table_headers.clone();
            current_tab.result_rows = tabular.current_table_data.clone();
            current_tab.result_all_rows = tabular.all_table_data.clone();
            current_tab.result_table_name = tabular.current_table_name.clone();
            current_tab.is_table_browse_mode = tabular.is_table_browse_mode;
            current_tab.current_page = tabular.current_page;
            current_tab.page_size = tabular.page_size;
            current_tab.total_rows = tabular.total_rows;
            current_tab.base_query = tabular.current_base_query.clone(); // Save base query
            debug!(
                "💾 Saving tab {} state: base_query='{}'",
                tabular.active_tab_index, current_tab.base_query
            );
        }

        // Switch to new tab
        tabular.active_tab_index = tab_index;
        if let Some(new_tab) = tabular.query_tabs.get(tab_index) {
            tabular.editor_text = new_tab.content.clone();
            // Restore per-tab result state into global display
            tabular.current_table_headers = new_tab.result_headers.clone();
            tabular.current_table_data = new_tab.result_rows.clone();
            tabular.all_table_data = new_tab.result_all_rows.clone();
            tabular.current_table_name = new_tab.result_table_name.clone();
            tabular.is_table_browse_mode = new_tab.is_table_browse_mode;
            tabular.current_page = new_tab.current_page;
            tabular.page_size = new_tab.page_size;
            tabular.total_rows = new_tab.total_rows;
            tabular.current_base_query = new_tab.base_query.clone(); // Restore base query
            debug!(
                "📂 Restoring tab {} state: base_query='{}', connection_id={:?}",
                tab_index, new_tab.base_query, new_tab.connection_id
            );
            // IMPORTANT: kembalikan connection id aktif sesuai tab baru
            tabular.current_connection_id = new_tab.connection_id;

            // Jika user sedang berada di tampilan Structure dan tab tujuan adalah tab Table, reload struktur tabel tsb.
            if tabular.table_bottom_view == models::structs::TableBottomView::Structure
                && new_tab.title.starts_with("Table:")
            {
                // load_structure_info_for_current_table adalah metode pada Tabular (dibuat pub(crate))
                data_table::load_structure_info_for_current_table(tabular);
            }
        }
    }
}

pub(crate) fn save_current_tab(tabular: &mut window_egui::Tabular) -> Result<(), String> {
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        // Ensure content holds editor text plus metadata header (id + optional db)
        let mut final_content = tabular.editor_text.clone();
        // Prepare or update metadata header at top of file
        let (conn_meta, db_meta) = (tab.connection_id, tab.database_name.clone());
        let mut header_lines: Vec<String> = Vec::new();
        if conn_meta.is_some() || db_meta.is_some() {
            if let Some(id) = conn_meta {
                header_lines.push(format!("-- tabular: connection_id={}", id));
                // Also persist connection_name for resilience if IDs change later
                if let Some(conn_name) = tabular
                    .connections
                    .iter()
                    .find(|c| c.id == Some(id))
                    .map(|c| c.name.clone())
                {
                    header_lines.push(format!("-- tabular: connection_name={}", conn_name));
                }
            }
            if let Some(db) = db_meta.filter(|d| !d.trim().is_empty()) {
                header_lines.push(format!("-- tabular: database={}", db));
            }
        }
        if !header_lines.is_empty() {
            // Remove existing tabular header lines to avoid duplicates
            let filtered_existing: String = final_content
                .lines()
                .filter(|l| !l.trim_start().starts_with("-- tabular:"))
                .collect::<Vec<_>>()
                .join("\n");
            final_content = format!(
                "{}\n\n{}",
                header_lines.join("\n"),
                filtered_existing.trim_start_matches('\n')
            );
        }
        // Keep a clone for potential content-based file path resolution below
        tab.content = final_content.clone();

        // Best-effort: if file_path is missing but this tab likely comes from a query file,
        // try to resolve the path from the queries tree by matching the tab title (filename).
        if tab.file_path.is_none() {
            let title_name = tab.title.clone();
            if title_name.ends_with(".sql") {
                // Flatten queries_tree and find unique match by name
                fn collect_matches(
                    nodes: &Vec<crate::models::structs::TreeNode>,
                    name: &str,
                    out: &mut Vec<String>,
                ) {
                    for n in nodes {
                        if let Some(path) = &n.file_path
                            && n.node_type == crate::models::enums::NodeType::Query
                            && n.name == name
                        {
                            out.push(path.clone());
                        }
                        if !n.children.is_empty() {
                            collect_matches(&n.children, name, out);
                        }
                    }
                }
                let mut candidates = Vec::new();
                collect_matches(&tabular.queries_tree, &title_name, &mut candidates);
                if candidates.len() == 1 {
                    tab.file_path = Some(candidates.remove(0));
                } else if candidates.len() > 1 {
                    // Ambiguous by name; try content-based disambiguation (ignore tabular headers)
                    let strip_headers = |s: &str| -> String {
                        s.lines()
                            .filter(|l| !l.trim_start().starts_with("-- tabular:"))
                            .collect::<Vec<_>>()
                            .join("\n")
                            .trim_start_matches('\n')
                            .to_string()
                    };
                    let current_body = strip_headers(&final_content);
                    let mut match_path: Option<String> = None;
                    for p in candidates.iter() {
                        if let Ok(c) = std::fs::read_to_string(p)
                            && strip_headers(&c) == current_body
                        {
                            match_path = Some(p.clone());
                            break;
                        }
                    }
                    if let Some(p) = match_path {
                        tab.file_path = Some(p);
                    }
                }
            }
        }

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

            // Initialize save directory with config query directory
            if tabular.save_directory.is_empty() {
                tabular.save_directory = crate::directory::get_query_dir()
                    .to_string_lossy()
                    .to_string();
            }

            tabular.show_save_dialog = true;
            Ok(())
        }
    } else {
        Err("No active tab".to_string())
    }
}

pub(crate) fn save_current_tab_with_name(
    tabular: &mut window_egui::Tabular,
    filename: String,
) -> Result<(), String> {
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        // Mirror header injection as in save_current_tab
        let mut final_content = tabular.editor_text.clone();
        let (conn_meta, db_meta) = (tab.connection_id, tab.database_name.clone());
        let mut header_lines: Vec<String> = Vec::new();
        if conn_meta.is_some() || db_meta.is_some() {
            if let Some(id) = conn_meta {
                header_lines.push(format!("-- tabular: connection_id={}", id));
                // Also persist connection_name for resilience
                if let Some(conn_name) = tabular
                    .connections
                    .iter()
                    .find(|c| c.id == Some(id))
                    .map(|c| c.name.clone())
                {
                    header_lines.push(format!("-- tabular: connection_name={}", conn_name));
                }
            }
            if let Some(db) = db_meta.filter(|d| !d.trim().is_empty()) {
                header_lines.push(format!("-- tabular: database={}", db));
            }
        }
        if !header_lines.is_empty() {
            let filtered_existing: String = final_content
                .lines()
                .filter(|l| !l.trim_start().starts_with("-- tabular:"))
                .collect::<Vec<_>>()
                .join("\n");
            final_content = format!(
                "{}\n\n{}",
                header_lines.join("\n"),
                filtered_existing.trim_start_matches('\n')
            );
        }
        tab.content = final_content;
        // Use selected save directory or fallback to query directory
        let target_dir = if !tabular.save_directory.is_empty() {
            std::path::PathBuf::from(&tabular.save_directory)
        } else {
            directory::get_query_dir()
        };

        // Ensure the target directory exists
        std::fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Failed to create target directory: {}", e))?;

        let mut clean_filename = filename.trim().to_string();
        if !clean_filename.ends_with(".sql") {
            clean_filename.push_str(".sql");
        }

        let file_path = target_dir.join(&clean_filename);

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
            ui.add_sized(
                [200.0, 20.0],
                egui::TextEdit::singleline(&mut tabular.advanced_editor.find_text),
            );

            ui.label("Replace:");
            ui.add_sized(
                [200.0, 20.0],
                egui::TextEdit::singleline(&mut tabular.advanced_editor.replace_text),
            );

            ui.checkbox(
                &mut tabular.advanced_editor.case_sensitive,
                "Case Sensitive",
            );
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

    // ----- Pre-widget key handling & indentation (no active borrow of editor_text) -----
    let rows = if tabular.advanced_editor.desired_rows > 0 { tabular.advanced_editor.desired_rows } else { 25 };
    let mut tab_pressed_pre = ui.input(|i| i.key_pressed(egui::Key::Tab));
    let shift_pressed_pre = ui.input(|i| i.modifiers.shift);
    let (sel_start, sel_end) = (tabular.selection_start, tabular.selection_end);
    if tab_pressed_pre && sel_start < sel_end && sel_end <= tabular.editor_text.len() {
        let slice = &tabular.editor_text[sel_start..sel_end];
        if slice.contains('\n') { // multi-line
            // Find first line start
            let mut line_start = sel_start;
            while line_start > 0 && tabular.editor_text.as_bytes()[line_start - 1] != b'\n' { line_start -= 1; }
            let sel_end_clamped = sel_end.min(tabular.editor_text.len());
            let block = tabular.editor_text[line_start..sel_end_clamped].to_string();
            if !shift_pressed_pre {
                let mut indented = String::with_capacity(block.len() + 8);
                for line in block.split_inclusive('\n') { if line == "\n" { indented.push('\n'); continue; } let (content,nl)= if let Some(p)=line.rfind('\n'){(&line[..p],&line[p..])} else {(line,"")}; indented.push('\t'); indented.push_str(content); indented.push_str(nl);}            
                tabular.editor_text.replace_range(line_start..sel_end_clamped, &indented);
                tabular.selection_start = line_start; tabular.selection_end = line_start + indented.len(); tabular.cursor_position = tabular.selection_end;
            } else {
                let mut outdented = String::with_capacity(block.len()); let mut changed=false; for line in block.split_inclusive('\n'){ if line=="\n" { outdented.push('\n'); continue;} let (content,nl)= if let Some(p)=line.rfind('\n'){(&line[..p],&line[p..])} else {(line,"")}; let trimmed = if let Some(rest)=content.strip_prefix('\t'){changed=true;rest} else if let Some(rest)=content.strip_prefix("    "){changed=true;rest} else {content}; outdented.push_str(trimmed); outdented.push_str(nl);} if changed { tabular.editor_text.replace_range(line_start..sel_end_clamped,&outdented); tabular.selection_start=line_start; tabular.selection_end=line_start+outdented.len(); tabular.cursor_position=tabular.selection_end; }
            }
            // consume Tab key event so TextEdit tidak menambah tab baru
            ui.ctx().input_mut(|ri| { ri.events.retain(|e| !matches!(e, egui::Event::Key{ key:egui::Key::Tab, ..})) });
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) { tab.content = tabular.editor_text.clone(); tab.is_modified = true; }
        }
    }
    // ----- Build widget after mutations -----
    // Re-enable syntax highlighting with cache
    let lang = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.file_path.as_ref())
        .map(|p| crate::syntax::detect_language_from_name(p))
        .unwrap_or(crate::syntax::LanguageKind::Sql);
    let dark = matches!(tabular.advanced_editor.theme, models::structs::EditorColorTheme::GithubDark | models::structs::EditorColorTheme::Gruvbox);
    
    // Simple layouter without cache for now (cache causes borrow issues)
    let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = crate::syntax::highlight_text(text.as_str(), lang, dark);
        job.wrap.max_width = wrap_width;
        ui.fonts(|f| f.layout_job(job))
    };

    // Pre-compute line count (immutable borrow) before creating mutable reference for TextEdit
    let pre_line_count = if tabular.advanced_editor.show_line_numbers { tabular.editor_text.lines().count().max(1) } else { 0 };
    let text_edit = egui::TextEdit::multiline(&mut tabular.editor_text)
        .font(egui::TextStyle::Monospace)
        .desired_rows(rows)
        .lock_focus(true)
        .desired_width(f32::INFINITY)
        .code_editor()
        .id_source("sql_editor")
        .layouter(&mut layouter);
    // (Old duplicate indentation block removed)
    let mut enter_pressed_pre = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut raw_tab = false;
    // Intercept arrow keys when autocomplete popup shown so caret tidak ikut bergerak
    let mut arrow_down_pressed = false;
    let mut arrow_up_pressed = false;
    ui.input(|i| {
        for ev in &i.events {
            if let egui::Event::Key {
                key: egui::Key::Tab,
                pressed: true,
                ..
            } = ev
            {
                raw_tab = true;
            }
        }
    });
    if tabular.show_autocomplete {
        ui.ctx().input_mut(|ri| {
            // Drain & filter events: buang ArrowUp/ArrowDown pressed supaya TextEdit tidak memproses
            let mut kept = Vec::with_capacity(ri.events.len());
            for ev in ri.events.drain(..) {
                match ev {
                    egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        pressed: true,
                        ..
                    } => {
                        arrow_down_pressed = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::ArrowUp,
                        pressed: true,
                        ..
                    } => {
                        arrow_up_pressed = true;
                    }
                    // Intercept Enter pressed untuk autocomplete acceptance (supaya tidak newline)
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        enter_pressed_pre = true;
                    }
                    // Jangan hilangkan release events agar repeat logic internal tidak stuck; hanya pressed yang kita konsumsi
                    other @ egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        pressed: false,
                        ..
                    } => {
                        kept.push(other);
                    }
                    other @ egui::Event::Key {
                        key: egui::Key::ArrowUp,
                        pressed: false,
                        ..
                    } => {
                        kept.push(other);
                    }
                    other => kept.push(other),
                }
            }
            ri.events = kept;
        });
    }
    if raw_tab {
        tab_pressed_pre = true;
        log::debug!("Raw Tab event captured before editor render");
    }
    let accept_via_tab_pre = tab_pressed_pre && tabular.show_autocomplete;
    let accept_via_enter_pre = enter_pressed_pre && tabular.show_autocomplete;
    // If accepting, prepare to inject remaining characters as text events so caret advances naturally
    if (accept_via_tab_pre || accept_via_enter_pre)
        && let Some(sugg) = tabular
            .autocomplete_suggestions
            .get(tabular.selected_autocomplete_index)
            .cloned()
    {
        // Remove the Tab key event itself so CodeEditor won't insert a tab char
        ui.ctx().input_mut(|ri| {
            let before = ri.events.len();
            ri.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        ..
                    }
                )
            });
            let removed = before - ri.events.len();
            if removed > 0 {
                log::debug!(
                    "Removed {} Tab key event(s) to prevent tab insertion",
                    removed
                );
            }
        });
        // Remove Enter key event(s) so tidak newline
        ui.ctx().input_mut(|ri| {
            let before = ri.events.len();
            ri.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        ..
                    }
                )
            });
            let removed = before - ri.events.len();
            if removed > 0 {
                log::debug!("Removed {} Enter key event(s) to prevent newline", removed);
            }
        });
        let prefix_len = tabular.autocomplete_prefix.len();
        if sugg.len() >= prefix_len {
            let remainder = &sugg[prefix_len..];
            if !remainder.is_empty() {
                ui.ctx().input_mut(|ri| {
                    ri.events.push(egui::Event::Text(remainder.to_string()));
                });
            }
        }
        tabular.show_autocomplete = false;
        tabular.autocomplete_suggestions.clear();
        log::debug!(
            "Autocomplete accepted via {} by injecting remainder (suggestion '{}')",
            if accept_via_tab_pre { "Tab" } else { "Enter" },
            sugg
        );
    }
    // Add small horizontal padding so the editor doesn't get clipped on the right edge
    // let response = egui::Frame::default()
    //     // .inner_margin(egui::Margin::rightf(2.0.into()))
    //     .show(ui, |ui| {
    //         editor.show(ui, &mut tabular.editor_text)
    //     })
    //     .inner;
    // Reserve gutter width if needed
    let gutter_width = if tabular.advanced_editor.show_line_numbers {
        let digits = (pre_line_count as f32).log10().floor() as usize + 1;
        (digits as f32) * 8.0 + 16.0 // approximate monospace char width
    } else { 0.0 };
    let mut rect = ui.available_rect_before_wrap();
    if gutter_width > 0.0 {
        let gutter_rect = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.min.x + gutter_width, rect.max.y),
        );
        rect.min.x += gutter_width;
        // Paint gutter after we know editor height; store for later
        ui.data_mut(|d| d.insert_temp::<egui::Rect>(egui::Id::new("gutter_rect"), gutter_rect));
    }
    let old_clip = ui.clip_rect();
    ui.set_clip_rect(rect);
    let response = ui.put(rect, text_edit);
    ui.set_clip_rect(old_clip);
    // Multi-cursor: key handling (Cmd+D / Ctrl+D for next occurrence) and Esc to clear
    let input_snapshot = ui.input(|i| i.clone());
    if input_snapshot.key_pressed(egui::Key::Escape) {
        if !tabular.extra_cursors.is_empty() { 
            tabular.clear_extra_cursors(); 
            tabular.selected_text.clear(); // Also clear selection
        }
    }
    let cmd_or_ctrl = input_snapshot.modifiers.command || input_snapshot.modifiers.ctrl;
    if cmd_or_ctrl && input_snapshot.key_pressed(egui::Key::D) {
        println!("DEBUG: Cmd+D key detected!");
        // Find next occurrence of selected text or word under cursor
        tabular.add_next_occurrence_cursor();
    }
    
    // Handle multi-cursor typing - apply changes to all cursors
    if !tabular.extra_cursors.is_empty() {
        // Check if text was changed this frame
        if let Some(state) = TextEditState::load(ui.ctx(), response.id) {
            if let Some(range) = state.cursor.char_range() {
                let new_main_cursor = range.primary.index;
                let old_main_cursor = tabular.cursor_position;
                
                // If cursor moved due to typing (not clicking), apply to all cursors
                if new_main_cursor != old_main_cursor && tabular.editor_text != tabular.last_editor_text {
                    let cursor_delta = new_main_cursor as i32 - old_main_cursor as i32;
                    println!("DEBUG: Multi-cursor typing detected, delta: {}", cursor_delta);
                    
                    // Update all extra cursors by the same delta
                    for cursor in &mut tabular.extra_cursors {
                        let new_pos = (*cursor as i32 + cursor_delta).max(0) as usize;
                        *cursor = new_pos.min(tabular.editor_text.len());
                    }
                    
                    // Sort and remove duplicates
                    tabular.extra_cursors.sort_unstable();
                    tabular.extra_cursors.dedup();
                    
                    // Remove any that match main cursor
                    tabular.extra_cursors.retain(|&pos| pos != new_main_cursor);
                    
                    tabular.cursor_position = new_main_cursor;
                }
            }
        }
        
        // Store current text for next frame comparison
        tabular.last_editor_text = tabular.editor_text.clone();
    }
    if tabular.advanced_editor.show_line_numbers {
        if let Some(gutter_rect) = ui.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("gutter_rect"))) {
            let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
            let total_lines = tabular.editor_text.lines().count().max(1);
            let editor_height = response.rect.height();
            let needed_height = line_height * total_lines as f32 + 8.0;
            let final_rect = egui::Rect::from_min_size(
                gutter_rect.min,
                egui::vec2(gutter_rect.width(), editor_height.max(needed_height)),
            );
            let painter = ui.painter();
            painter.rect_filled(final_rect, 0.0, ui.visuals().faint_bg_color);
            let mut y = final_rect.top() + 4.0;
            for (i, _) in tabular.editor_text.lines().enumerate() {
                painter.text(
                    egui::pos2(final_rect.right() - 6.0, y),
                    egui::Align2::RIGHT_TOP,
                    (i + 1).to_string(),
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    ui.visuals().weak_text_color(),
                );
                y += line_height;
                if y > final_rect.bottom() { break; }
            }
        }
    }

    // Paint extra cursors (after gutter so they appear above text) using approximate positioning
    if !tabular.extra_cursors.is_empty() {
        let painter = ui.painter();
        let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
        
        // Calculate gutter width
        let gutter_width = if tabular.advanced_editor.show_line_numbers {
            let total_lines = tabular.editor_text.lines().count().max(1);
            ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), '0')) 
                * (total_lines.to_string().len() as f32 + 1.0)
        } else {
            0.0
        };
        
        println!("DEBUG: Drawing {} cursors, gutter_width={}", tabular.extra_cursors.len(), gutter_width);
        
        for &cpos in &tabular.extra_cursors {
            let mut line_start = 0usize; 
            let mut line_no = 0usize;
            for (i, ch) in tabular.editor_text.char_indices() { 
                if i >= cpos { break; } 
                if ch == '\n' { 
                    line_no += 1; 
                    line_start = i + 1; 
                } 
            }
            let column = cpos.saturating_sub(line_start);
            let char_w = ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), 'M'));
            
            // Position cursor properly with better offset calculation
            let x = response.rect.left() + gutter_width + 6.0 + (column as f32) * char_w;
            let y_top = response.rect.top() + 6.0 + (line_no as f32) * line_height;
            
            // Make cursor thinner and more subtle
            let caret_rect = egui::Rect::from_min_size(egui::pos2(x, y_top), egui::vec2(1.5, line_height - 2.0));
            let color = egui::Color32::from_rgba_unmultiplied(100, 150, 255, 180); // Semi-transparent blue
            painter.rect_filled(caret_rect, 1.0, color);
            
            println!("DEBUG: Drawing cursor at pos {} -> line {}, col {} -> x={}, y={}", 
                    cpos, line_no, column, x, y_top);
        }
    }

    // After show(), TextEditState should exist; apply pending cursor now
    if let Some(pos) = tabular.pending_cursor_set {
        let id = egui::Id::new("sql_editor");
        if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
            let clamped = pos.min(tabular.editor_text.len());
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(clamped))));
            state.store(ui.ctx(), id);
            tabular.cursor_position = clamped;
            tabular.pending_cursor_set = None;
            ui.memory_mut(|m| m.request_focus(id));
            debug!("Applied pending cursor position {} post-show", clamped);
        } else {
            // Create a new state manually so cursor moves even if CodeEditor doesn't create one
            let mut state = TextEditState::default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
            state.store(ui.ctx(), id);
            debug!("Manually created TextEditState with cursor {}", pos);
        }
    }

    // Cleanup stray tab character inside the just-completed identifier (from Tab key) if any
    if accept_via_tab_pre {
        // Cursor currently at end of identifier after injection; scan backwards
        let mut idx = tabular.cursor_position.min(tabular.editor_text.len());
        let bytes = tabular.editor_text.as_bytes();
        while idx > 0 {
            let ch = bytes[idx - 1] as char;
            if ch.is_alphanumeric() || ch == '_' || ch == '\t' {
                idx -= 1;
            } else {
                break;
            }
        }
        // Now [idx .. cursor_position] spans the token (possibly including a tab)
        if idx < tabular.cursor_position {
            let token_range_end = tabular.cursor_position;
            let token_owned = tabular.editor_text[idx..token_range_end].to_string();
            if token_owned.contains('\t') {
                let cleaned: String = token_owned.chars().filter(|c| *c != '\t').collect();
                if cleaned != token_owned {
                    tabular
                        .editor_text
                        .replace_range(idx..token_range_end, &cleaned);
                    let shift = token_owned.len() - cleaned.len();
                    tabular.cursor_position -= shift;
                    // Adjust egui state cursor
                    let id = egui::Id::new("sql_editor");
                    if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
                        state
                            .cursor
                            .set_char_range(Some(CCursorRange::one(CCursor::new(
                                tabular.cursor_position,
                            ))));
                        state.store(ui.ctx(), id);
                    } else {
                        tabular.pending_cursor_set = Some(tabular.cursor_position);
                    }
                    log::debug!(
                        "Removed tab character from accepted token; new token='{}'",
                        cleaned
                    );
                }
            }
        }
    }

    // Try to capture selected text from the response
    // Note: This is a simplified approach. The actual implementation may vary depending on the CodeEditor version
    // Recover cursor + selection from TextEditState (single range only for now)
    if let Some(state) = TextEditState::load(ui.ctx(), response.id) {
        if let Some(range) = state.cursor.char_range() {
            let primary = range.primary;
            let secondary = range.secondary;
            let start = primary.index.min(secondary.index);
            let end = primary.index.max(secondary.index);
            tabular.cursor_position = primary.index;
            tabular.selection_start = start;
            tabular.selection_end = end;
            if start != end {
                if let Some(selected) = tabular.editor_text.get(start..end) {
                    tabular.selected_text = selected.to_string();
                } else {
                    tabular.selected_text.clear();
                }
            } else {
                tabular.selected_text.clear();
            }
        }
    }

    // Reset table focus flag when editor is interacted with
    if response.clicked() || response.has_focus() {
        tabular.table_recently_clicked = false;
    }

    // (Multi-line indent already handled pre-render if applied)

    // If you get a type error here, try:
    // let mut buffer = egui_code_editor::SimpleTextBuffer::from(&tabular.editor_text);
    // let response = editor.show(ui, &mut buffer);
    // tabular.editor_text = buffer.text().to_string();

    // Update tab content when editor changes (but skip autocomplete update if we're accepting via Tab)
    if response.changed() {
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor_text.clone();
            tab.is_modified = true;
        }
        // Replace entire lapce buffer (simplistic sync)
        if let Some(buf) = tabular.lapce_buffer.as_mut() { *buf = Buffer::new(&tabular.editor_text); }
        
        // Apply multi-cursor editing if there are extra cursors
        if !tabular.extra_cursors.is_empty() {
            // Store current cursor position before borrowing state
            let current_cursor = tabular.cursor_position;
            let editor_text_copy = tabular.editor_text.clone();
            
            // Detect what changed and apply to all cursors
            if let Some(state) = TextEditState::load(ui.ctx(), response.id) {
                if let Some(range) = state.cursor.char_range() {
                    let new_cursor = range.primary.index;
                    if new_cursor != current_cursor {
                        // Something was typed
                        let change_len = new_cursor as i32 - current_cursor as i32;
                        if change_len > 0 && change_len < 10 { // Simple character insertion
                            if let Some(inserted_text) = editor_text_copy.get(current_cursor..new_cursor) {
                                tabular.apply_multi_edit(current_cursor, current_cursor, inserted_text);
                            }
                        }
                    }
                }
            }
        }
        
        // Disable autocomplete for now (needs reimplementation on lapce-core)
        tabular.show_autocomplete = false;
        tabular.autocomplete_suggestions.clear();
    }

    // (Old forced replacement path removed; injection handles caret advance)

    // Keyboard handling for autocomplete
    let input = ui.input(|i| i.clone());
    if input.key_pressed(egui::Key::Space) && (input.modifiers.ctrl || input.modifiers.command) {
        editor_autocomplete::trigger_manual(tabular);
    }

    // Fallback: detect raw tab character insertion (editor consumed Tab key)
    if tabular.show_autocomplete && !tab_pressed_pre {
        // only if we didn't already detect it
        let cur = tabular.cursor_position.min(tabular.editor_text.len());
        if cur > 0 && tabular.editor_text.chars().nth(cur - 1) == Some('\t') {
            // Remove the inserted tab char then accept suggestion
            tabular.editor_text.remove(cur - 1);
            tabular.cursor_position = tabular.cursor_position.saturating_sub(1);
            log::debug!("Detected tab character insertion -> triggering autocomplete accept");
            editor_autocomplete::accept_current_suggestion(tabular);
        } else if cur >= 4 && &tabular.editor_text[cur - 4..cur] == "    " {
            // Four spaces indentation
            tabular.editor_text.replace_range(cur - 4..cur, "");
            tabular.cursor_position -= 4;
            log::debug!("Detected 4-space indentation -> triggering autocomplete accept");
            editor_autocomplete::accept_current_suggestion(tabular);
        }
    }
    if tabular.show_autocomplete {
        // Navigasi popup autocomplete: gunakan arrow yang sudah kita intercept sebelum render editor
        if arrow_down_pressed {
            editor_autocomplete::navigate(tabular, 1);
        }
        if arrow_up_pressed {
            editor_autocomplete::navigate(tabular, -1);
        }
        let mut accepted = false;
        if input.key_pressed(egui::Key::Enter) && !accept_via_enter_pre {
            editor_autocomplete::accept_current_suggestion(tabular);
            accepted = true;
        }
        // Skip Tab acceptance here if already processed earlier
        if tab_pressed_pre && !accept_via_tab_pre {
            editor_autocomplete::accept_current_suggestion(tabular);
            accepted = true;
        }
        if accepted {
            log::debug!(
                "Autocomplete accepted via {}",
                if tab_pressed_pre {
                    "Tab"
                } else {
                    "Enter(post)"
                }
            );
            // Clean up potential inserted tab characters or spaces from editor before replacement
            // Detect diff compared to pre_text
            if tabular.editor_text.contains('\t') {
                // Remove a lone tab right before cursor if exists
                let cur = tabular.cursor_position.min(tabular.editor_text.len());
                if cur > 0 && tabular.editor_text.chars().nth(cur - 1) == Some('\t') {
                    tabular.editor_text.remove(cur - 1);
                    tabular.cursor_position = tabular.cursor_position.saturating_sub(1);
                }
            }
            // Remove four leading spaces sequence before cursor (indent) if present
            let cur = tabular.cursor_position.min(tabular.editor_text.len());
            if cur >= 4 && &tabular.editor_text[cur - 4..cur] == "    " {
                tabular.editor_text.replace_range(cur - 4..cur, "");
                tabular.cursor_position -= 4;
            }
            // Update internal egui state for cursor after Enter accept path
            let id = egui::Id::new("sql_editor");
            if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(
                        tabular.cursor_position,
                    ))));
                state.store(ui.ctx(), id);
            } else {
                tabular.pending_cursor_set = Some(tabular.cursor_position);
            }
            // Re-focus editor so Tab doesn't move focus away
            ui.memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
        }
        if input.key_pressed(egui::Key::Escape) {
            tabular.show_autocomplete = false;
        }
    }

    // Update suggestions saat kursor bergerak kiri/kanan (tanpa perubahan teks)
    let moved_lr =
        input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowRight);
    if moved_lr && !accept_via_tab_pre && !accept_via_enter_pre {
        // cursor_position sudah diperbarui via response.cursor_range; cukup panggil update
        editor_autocomplete::update_autocomplete(tabular);
    }

    // Render autocomplete popup positioned under cursor
    if tabular.show_autocomplete && !tabular.autocomplete_suggestions.is_empty() {
        // Approximate cursor line & column
        let cursor = tabular.cursor_position.min(tabular.editor_text.len());
        let mut line_start = 0usize;
        let mut line_no = 0usize;
        for (i, ch) in tabular.editor_text.char_indices() {
            if i >= cursor {
                break;
            }
            if ch == '\n' {
                line_no += 1;
                line_start = i + 1;
            }
        }
        let column = cursor - line_start;
        let char_w = 8.0_f32; // heuristic monospace width
        let line_h = ui.text_style_height(&egui::TextStyle::Monospace);
    let editor_rect = response.rect; // basic TextEdit rect
        let mut pos = egui::pos2(
            editor_rect.left() + 8.0 + (column as f32) * char_w,
            editor_rect.top() + 4.0 + (line_no as f32 + 1.0) * line_h,
        );
        // Clamp horizontally inside editor area
        if pos.x > editor_rect.right() - 150.0 {
            pos.x = editor_rect.right() - 150.0;
        }
        editor_autocomplete::render_autocomplete(tabular, ui, pos);
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
            tabular.editor_text = re
                .replace_all(&tabular.editor_text, replace_text)
                .into_owned();
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
    if !tabular.advanced_editor.find_text.is_empty()
        && let Some(_pos) = tabular.editor_text.find(&tabular.advanced_editor.find_text)
    {
        // In a full implementation, you would scroll to and highlight the match
        debug!("Found match for: {}", tabular.advanced_editor.find_text);
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
        tabular
            .command_palette_items
            .iter()
            .filter(|cmd| {
                cmd.to_lowercase()
                    .contains(&tabular.command_palette_input.to_lowercase())
            })
            .cloned()
            .collect()
    };

    if filtered_commands.is_empty() {
        return;
    }

    // Update selected index with wrapping
    if direction > 0 {
        // Down arrow
        tabular.command_palette_selected_index =
            (tabular.command_palette_selected_index + 1) % filtered_commands.len();
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
        tabular
            .command_palette_items
            .iter()
            .filter(|cmd| {
                cmd.to_lowercase()
                    .contains(&tabular.command_palette_input.to_lowercase())
            })
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
        tabular.theme_selector_selected_index =
            (tabular.theme_selector_selected_index + 1) % theme_count;
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
        0 => models::structs::EditorColorTheme::GithubDark,
        1 => models::structs::EditorColorTheme::GithubLight,
        2 => models::structs::EditorColorTheme::Gruvbox,
        _ => models::structs::EditorColorTheme::GithubDark, // fallback
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
                                .hint_text("Type command name..."),
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
                        let filtered_commands: Vec<String> =
                            if tabular.command_palette_input.is_empty() {
                                tabular.command_palette_items.clone()
                            } else {
                                tabular
                                    .command_palette_items
                                    .iter()
                                    .filter(|cmd| {
                                        cmd.to_lowercase()
                                            .contains(&tabular.command_palette_input.to_lowercase())
                                    })
                                    .cloned()
                                    .collect()
                            };

                        // Ensure selected index is within bounds when filtering
                        if tabular.command_palette_selected_index >= filtered_commands.len()
                            && !filtered_commands.is_empty()
                        {
                            tabular.command_palette_selected_index = 0;
                        }

                        // Command list
                        egui::ScrollArea::vertical()
                            .max_height(500.0)
                            .show(ui, |ui| {
                                for (index, command) in filtered_commands.iter().enumerate() {
                                    let is_selected =
                                        index == tabular.command_palette_selected_index;

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
                            (
                                models::structs::EditorColorTheme::GithubDark,
                                "GitHub Dark",
                                "Dark theme with blue accents",
                            ),
                            (
                                models::structs::EditorColorTheme::GithubLight,
                                "GitHub Light",
                                "Light theme with subtle colors",
                            ),
                            (
                                models::structs::EditorColorTheme::Gruvbox,
                                "Gruvbox",
                                "Retro warm theme with earthy colors",
                            ),
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
    tabular.is_table_browse_mode = false;
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

    // We no longer branch on first execution; per-tab connection must always be set explicitly.

    // Strict per-tab connection: always use the tab's own connection_id (no global fallback)
    let connection_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id);

    // If tab has no connection, show connection selector (first time OR after user cleared it)
    if connection_id.is_none() {
        debug!("Query execution requested but tab has no connection - opening selector");
        tabular.pending_query = query;
        tabular.auto_execute_after_connection = true;
        tabular.show_connection_selector = true;
        return;
    }

    // Check if we have an active connection
    if let Some(connection_id) = connection_id {
        // If a pool creation is already in progress for this connection, show loading and queue the query
        if tabular.pending_connection_pools.contains(&connection_id) {
            log::debug!(
                "⏳ Pool creation in progress for {}, queueing query and showing loading",
                connection_id
            );
            tabular.pool_wait_in_progress = true;
            tabular.pool_wait_connection_id = Some(connection_id);
            tabular.pool_wait_query = query.clone();
            tabular.pool_wait_started_at = Some(std::time::Instant::now());
            // Friendly status message; keep current data intact
            tabular.current_table_name = "Connecting… waiting for pool".to_string();
            // Do not execute now
            return;
        }

        // If no pool exists yet, try quick creation; if not immediately available, show loading
        if !tabular.connection_pools.contains_key(&connection_id) {
            // Attempt a quick creation via the runtime without capturing &mut tabular inside the future
            if let Some(rt) = tabular.runtime.clone() {
                // Use the non-blocking helper that handles pending state and background spawn
                let created = rt.block_on(async {
                    // SAFETY: try_get_connection_pool only briefly borrows tabular inside the await; we avoid
                    // capturing &mut tabular by doing only a readiness check here and letting the update loop handle execution.
                    // We return true only if a pool is immediately available; otherwise background creation will be started elsewhere.
                    crate::connection::try_get_connection_pool(tabular, connection_id)
                        .await
                        .is_some()
                });
                if !created {
                    // Not ready now; show loading and queue the query. Background creation will happen via get_or_create on demand.
                    log::debug!(
                        "🔧 Pool not ready for {}, queueing and showing loading",
                        connection_id
                    );
                    tabular.pool_wait_in_progress = true;
                    tabular.pool_wait_connection_id = Some(connection_id);
                    tabular.pool_wait_query = query.clone();
                    tabular.pool_wait_started_at = Some(std::time::Instant::now());
                    tabular.current_table_name = "Connecting… waiting for pool".to_string();
                    return;
                }
            } else {
                // No runtime configured yet; just set wait state
                tabular.pool_wait_in_progress = true;
                tabular.pool_wait_connection_id = Some(connection_id);
                tabular.pool_wait_query = query.clone();
                tabular.pool_wait_started_at = Some(std::time::Instant::now());
                tabular.current_table_name = "Connecting… waiting for pool".to_string();
                return;
            }
        }

        debug!("=== EXECUTING QUERY ===");
        debug!("Connection ID: {}", connection_id);
        debug!("Query: {}", query);

        let result =
            connection::execute_query_with_connection(tabular, connection_id, query.clone());

        debug!("Query execution result: {:?}", result.is_some());

        // Mark tab as having executed a query (regardless of success/failure)
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.has_executed_query = true;
        }

        if let Some((headers, data)) = result {
            let is_error_result = headers.first().map(|h| h == "Error").unwrap_or(false);
            debug!("=== QUERY RESULT SUCCESS ===");
            debug!("Headers received: {} - {:?}", headers.len(), headers);
            debug!("Data rows received: {}", data.len());
            if !data.is_empty() {
                debug!("First row sample: {:?}", &data[0]);
            }

            tabular.current_table_headers = headers;

            // Use pagination for query results
            data_table::update_pagination_data(tabular, data);

            if tabular.total_rows == 0 {
                tabular.current_table_name = "Query executed successfully (no results)".to_string();
            } else {
                tabular.current_table_name = format!(
                    "Query Results ({} total rows, showing page {} of {})",
                    tabular.total_rows,
                    tabular.current_page + 1,
                    data_table::get_total_pages(tabular)
                );
            }
            debug!(
                "After update_pagination_data - total_rows: {}, all_table_data.len(): {}",
                tabular.total_rows,
                tabular.all_table_data.len()
            );
            debug!("============================");

            // Set the base query for pagination - this is crucial for regular queries!
            // For regular queries, we set the base query to the executed query (without LIMIT)
            let base_query_for_pagination = if !is_error_result && tabular.total_rows > 0 {
                // Simple LIMIT removal for pagination - remove LIMIT clause if present
                let mut clean_query = query.clone();
                if let Some(limit_pos) = clean_query.to_uppercase().rfind("LIMIT") {
                    // Find the end of the LIMIT clause (look for semicolon or end of string)
                    if let Some(semicolon_pos) = clean_query[limit_pos..].find(';') {
                        clean_query = format!(
                            "{}{}",
                            &clean_query[..limit_pos].trim(),
                            &clean_query[limit_pos + semicolon_pos..]
                        );
                    } else {
                        clean_query = clean_query[..limit_pos].trim().to_string();
                    }
                }
                clean_query
            } else {
                String::new()
            };
            tabular.current_base_query = base_query_for_pagination.clone();
            debug!(
                "📝 Set base_query for pagination: '{}'",
                base_query_for_pagination
            );

            // Save query to history hanya jika bukan hasil error
            if !is_error_result {
                sidebar_history::save_query_to_history(tabular, &query, connection_id);
            } else {
                debug!("Skip saving to history karena hasil error");
            }
            // Persist into tab state
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.result_headers = tabular.current_table_headers.clone();
                tab.result_rows = tabular.current_table_data.clone();
                tab.result_all_rows = tabular.all_table_data.clone();
                tab.result_table_name = tabular.current_table_name.clone();
                tab.is_table_browse_mode = tabular.is_table_browse_mode;
                tab.current_page = tabular.current_page;
                tab.page_size = tabular.page_size;
                tab.total_rows = tabular.total_rows;
                tab.base_query = tabular.current_base_query.clone(); // Save the base query to the tab
            }
        } else {
            tabular.current_table_name = "Query execution failed".to_string();
            tabular.current_table_headers.clear();
            tabular.current_table_data.clear();
            tabular.all_table_data.clear();
            tabular.total_rows = 0;
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.result_headers.clear();
                tab.result_rows.clear();
                tab.result_all_rows.clear();
                tab.result_table_name = tabular.current_table_name.clone();
                tab.total_rows = 0;
                tab.current_page = 0;
                tab.base_query.clear(); // Clear base query on failure
            }
        }
    }
}

pub(crate) fn extract_query_from_cursor(tabular: &mut window_egui::Tabular) -> String {
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
