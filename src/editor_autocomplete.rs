use crate::cache_data::get_columns_from_cache;
use crate::models;
use crate::window_egui::{PrefTab, Tabular};
use eframe::egui;
use log::debug;

// Basic SQL keywords list (extend as needed)
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "DROP", "ALTER", "ADD", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP",
    "BY", "ORDER", "LIMIT", "OFFSET", "AND", "OR", "NOT", "NULL", "AS", "DISTINCT", "COUNT", "SUM",
    "AVG", "MIN", "MAX", "LIKE", "IN", "IS", "BETWEEN", "UNION", "ALL",
];

/// Extract current word prefix before cursor.
fn current_prefix(text: &str, cursor: usize) -> (String, usize) {
    if text.is_empty() {
        return (String::new(), cursor);
    }
    let bytes = text.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_alphanumeric() || c == '_' {
            start -= 1;
        } else {
            break;
        }
    }
    (text[start..cursor.min(text.len())].to_string(), start)
}

/// Tokenize helper: split on non-alphanumeric/_ characters.
#[allow(dead_code)]
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            cur.push(ch);
        } else if !cur.is_empty() {
            tokens.push(cur.clone());
            cur.clear();
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Return active (connection_id, database_name) if available.
#[allow(dead_code)]
fn active_connection_and_db(app: &Tabular) -> Option<(i64, String)> {
    if let Some(tab) = app.query_tabs.get(app.active_tab_index)
        && let Some(cid) = tab.connection_id
    {
        if let Some(db) = &tab.database_name {
            return Some((cid, db.clone()));
        }
        // fallback to connection default database
        if let Some(conn) = app.connections.iter().find(|c| c.id == Some(cid)) {
            return Some((cid, conn.database.clone()));
        }
        return Some((cid, String::new()));
    }
    None
}

/// Extract table names appearing after first FROM (comma separated; stop at clause keywords).
#[allow(dead_code)]
fn extract_tables(full_text: &str) -> Vec<String> {
    // Cari token FROM (case-insensitive) sebagai token utuh, lalu kumpulkan nama tabel setelahnya
    let bytes = full_text.as_bytes();
    let upper = full_text.to_uppercase();
    let mut idx = 0usize;
    let mut tables: Vec<String> = Vec::new();
    let stop_tokens = [
        "WHERE", "GROUP", "ORDER", "LIMIT", "OFFSET", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER",
    ]; // clause penutup
    while idx < upper.len() {
        if upper[idx..].starts_with("FROM") {
            // Pastikan token berdiri sendiri (awal atau non ident char sebelum & sesudah)
            let before_ok = idx == 0 || !upper.as_bytes()[idx - 1].is_ascii_alphanumeric();
            let after = idx + 4;
            let after_ok = after >= upper.len() || !upper.as_bytes()[after].is_ascii_alphanumeric();
            if before_ok && after_ok {
                // ditemukan FROM
                // Skip whitespace/newline setelah FROM
                let mut j = after;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                // Ambil segmen sampai semicolon atau akhir atau clause token
                let mut seg_end = j;
                while seg_end < bytes.len() {
                    let ch = bytes[seg_end] as char;
                    if ch == ';' {
                        break;
                    }
                    seg_end += 1;
                }
                let segment = &full_text[j..seg_end];
                // Split by commas (support multi-line)
                for part in segment.split(',') {
                    let trimmed = part.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // Hentikan jika bagian ini diawali clause (WHERE, GROUP, ...)
                    let upper_first = trimmed
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_uppercase();
                    if stop_tokens.contains(&upper_first.as_str()) {
                        break;
                    }
                    // token pertama adalah nama tabel sebelum alias
                    let first = trimmed.split_whitespace().next().unwrap_or("");
                    // Berhenti kalau first adalah clause
                    if stop_tokens.contains(&first.to_uppercase().as_str()) {
                        break;
                    }
                    let cleaned = first.trim_matches('`').trim_matches('"');
                    if !cleaned.is_empty() {
                        // Ambil nama terakhir jika schema.table
                        let final_name = cleaned
                            .split('.')
                            .next_back()
                            .unwrap_or(cleaned)
                            .to_string();
                        tables.push(final_name);
                    }
                }
                break; // hanya proses FROM pertama
            }
        }
        idx += 1;
    }
    // Dedup
    let mut seen = std::collections::HashSet::new();
    tables.retain(|t| seen.insert(t.to_lowercase()));
    tables
}

/// Build smart context-aware suggestions
pub fn build_suggestions(
    app: &Tabular,
    _text: &str,
    cursor_pos: usize,
    prefix: &str,
) -> Vec<String> {
    let mut suggestions = Vec::new();
    let prefix_lower = prefix.to_lowercase();

    // Get current connection and context
    let connection_id = app
        .query_tabs
        .get(app.active_tab_index)
        .and_then(|tab| tab.connection_id);

    // Context-aware suggestions based on SQL position
    let context = detect_sql_context(_text, cursor_pos);
    let tables = extract_tables(_text);
    let database = if let Some((_, db)) = active_connection_and_db(app) {
        db
    } else {
        String::new()
    };
    debug!("Detected tables in FROM clause: {:?}", tables);

    match context {
        SqlContext::AfterSelect => {

            // Suggest column names if we have connection context
            if let Some(conn_id) = connection_id
                && let Some(columns) = get_cached_columns(app, conn_id, &database, tables)
            {
                for col in columns {
                    if col.to_lowercase().starts_with(&prefix_lower) {
                        suggestions.push(col);
                    }
                }
            }
            // Also suggest * for SELECT *
            if "*".starts_with(&prefix_lower) {
                suggestions.push("*".to_string());
            }
        }
        SqlContext::AfterFrom => {
            // Suggest table names
            if let Some(conn_id) = connection_id
                && let Some(tables) = get_cached_tables(app, conn_id)
            {
                for table in tables {
                    if table.to_lowercase().starts_with(&prefix_lower) {
                        suggestions.push(table);
                    }
                }
            }
        }
        SqlContext::AfterWhere => {
            // Suggest column names for WHERE conditions
            if let Some(conn_id) = connection_id
                && let Some(columns) = get_cached_columns(app, conn_id, &database, tables)
            {
                for col in columns {
                    if col.to_lowercase().starts_with(&prefix_lower) {
                        suggestions.push(col);
                    }
                }
            }
        }
        SqlContext::General => {
            // General SQL keywords
            add_sql_keywords(&mut suggestions, &prefix_lower);

            // Add table and column names as secondary suggestions
            if let Some(conn_id) = connection_id {
                if let Some(tables) = get_cached_tables(app, conn_id) {
                    for table in tables {
                        if table.to_lowercase().starts_with(&prefix_lower) {
                            suggestions.push(table);
                        }
                    }
                }
                if let Some(columns) = get_cached_columns(app, conn_id, &database, tables) {
                    for col in columns {
                        if col.to_lowercase().starts_with(&prefix_lower) {
                            suggestions.push(col);
                        }
                    }
                }
            }
        }
    }

    // Remove duplicates and sort
    suggestions.sort_unstable();
    suggestions.dedup();
    suggestions
}

#[derive(Debug, PartialEq)]
enum SqlContext {
    AfterSelect,
    AfterFrom,
    AfterWhere,
    General,
}

fn detect_sql_context(text: &str, cursor_pos: usize) -> SqlContext {
    let before_cursor = &text[..cursor_pos.min(text.len())];
    let words: Vec<&str> = before_cursor
        .split_whitespace()
        .map(|s| s.trim_end_matches(&[',', ';', '(', ')'][..]))
        .collect();

    if words.is_empty() {
        return SqlContext::General;
    }

    // Look for the most recent SQL keyword
    for word in words.iter().rev() {
        match word.to_uppercase().as_str() {
            "SELECT" => return SqlContext::AfterSelect,
            "FROM" | "JOIN" | "INNER" | "LEFT" | "RIGHT" => return SqlContext::AfterFrom,
            "WHERE" | "AND" | "OR" | "HAVING" => return SqlContext::AfterWhere,
            _ => continue,
        }
    }

    SqlContext::General
}

fn get_cached_tables(app: &Tabular, connection_id: i64) -> Option<Vec<String>> {
    // Try to get from database cache
    app.database_cache
        .get(&connection_id)
        .cloned()
}

fn get_cached_columns(mut _app: &Tabular, _connection_id: i64, _database: &str, _tables: Vec<String>) -> Option<Vec<String>> {
    // get columns for specific tables if provided
    // For simplicity, we return a static list here; in real implementation, query cache DB
    let mut columns = vec![];

    // loop through tables
    // Use a shallow mutable clone for cache APIs that require &mut Tabular
    let mut shallow = _app.shallow_for_cache();
    for table in _tables {
        let database: Option<Vec<(String, String)>> =
            get_columns_from_cache(&mut shallow, _connection_id, _database, &table);
        if let Some(cols) = database {
            for (col_name, _) in cols {
                if !columns.contains(&col_name) {
                    columns.push(col_name);
                }
            }
        }
    }
    columns.sort();
    Some(columns)
}

fn add_sql_keywords(suggestions: &mut Vec<String>, prefix_lower: &str) {
    for keyword in &[
        "SELECT",
        "FROM",
        "WHERE",
        "JOIN",
        "INNER",
        "LEFT",
        "RIGHT",
        "OUTER",
        "INSERT",
        "INTO",
        "VALUES",
        "UPDATE",
        "SET",
        "DELETE",
        "CREATE",
        "TABLE",
        "ALTER",
        "DROP",
        "INDEX",
        "PRIMARY",
        "KEY",
        "FOREIGN",
        "REFERENCES",
        "NOT",
        "NULL",
        "DEFAULT",
        "AUTO_INCREMENT",
        "UNIQUE",
        "CONSTRAINT",
        "AND",
        "OR",
        "IN",
        "LIKE",
        "BETWEEN",
        "EXISTS",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "GROUP",
        "BY",
        "ORDER",
        "ASC",
        "DESC",
        "HAVING",
        "LIMIT",
        "OFFSET",
        "UNION",
        "ALL",
        "DISTINCT",
        "AS",
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
    ] {
        if keyword.to_lowercase().starts_with(prefix_lower) {
            suggestions.push(keyword.to_string());
        }
    }
}

// Removed unused TreeNodeExt trait & impl (was an accessor wrapper) to silence dead_code warning.

// Provide a lightweight clone for cache access (cache functions require &mut Tabular)
#[allow(dead_code)]
trait ShallowForCache {
    fn shallow_for_cache(&self) -> Box<Tabular>;
}
impl ShallowForCache for Tabular {
    fn shallow_for_cache(&self) -> Box<Tabular> {
        Box::new(Tabular {
            db_pool: self.db_pool.clone(),
            connections: self.connections.clone(),
            query_tabs: self.query_tabs.clone(),
            active_tab_index: self.active_tab_index,
            // The rest are default/empty; not used by cache getters
            editor: crate::editor_buffer::EditorBuffer::new(""),
            multi_selection: crate::editor_selection::MultiSelection::new(),
            selected_menu: String::new(),
            items_tree: Vec::new(),
            queries_tree: Vec::new(),
            history_tree: Vec::new(),
            history_items: Vec::new(),
            show_add_connection: false,
            new_connection: models::structs::ConnectionConfig::default(),
            runtime: self.runtime.clone(),
            connection_pools: self.connection_pools.clone(),
            pending_connection_pools: std::collections::HashSet::new(), // Empty for shallow clone
            shared_connection_pools: self.shared_connection_pools.clone(), // Share the same pools
            show_edit_connection: false,
            edit_connection: models::structs::ConnectionConfig::default(),
            needs_refresh: false,
            current_table_data: Vec::new(),
            current_table_headers: Vec::new(),
            current_table_name: String::new(),
            current_connection_id: None,
            current_page: 0,
            page_size: 0,
            total_rows: 0,
            all_table_data: Vec::new(),
            selection_start: 0,
            selection_end: 0,
            auto_updater: None, // Not needed for cache operations
            // Server-side pagination fields
            use_server_pagination: self.use_server_pagination,
            actual_total_rows: None,
            current_base_query: String::new(),
            table_split_ratio: 0.0,
            sort_column: None,
            sort_ascending: true,
            test_connection_status: None,
            test_connection_in_progress: false,
            background_sender: None,
            background_receiver: None,
            refreshing_connections: std::collections::HashSet::new(),
            next_tab_id: 0,
            show_save_dialog: false,
            save_filename: String::new(),
            save_directory: String::new(),
            save_directory_picker_result: None,
            show_connection_selector: false,
            pending_query: String::new(),
            auto_execute_after_connection: false,
            error_message: String::new(),
            show_error_message: false,
            advanced_editor: models::structs::AdvancedEditor::default(),
            selected_text: String::new(),
            cursor_position: 0,
            show_command_palette: false,
            command_palette_input: String::new(),
            show_theme_selector: false,
            command_palette_items: Vec::new(),
            command_palette_selected_index: 0,
            theme_selector_selected_index: 0,
            request_theme_selector: false,
            is_dark_mode: true,
            show_settings_window: false,
            database_search_text: String::new(),
            filtered_items_tree: Vec::new(),
            show_search_results: false,
            show_create_folder_dialog: false,
            new_folder_name: String::new(),
            selected_query_for_move: None,
            show_move_to_folder_dialog: false,
            target_folder_name: String::new(),
            parent_folder_for_creation: None,
            selected_folder_for_removal: None,
            folder_removal_map: std::collections::HashMap::new(),
            last_cleanup_time: std::time::Instant::now(),
            selected_row: None,
            selected_cell: None,
            selected_rows: std::collections::BTreeSet::new(),
            selected_columns: std::collections::BTreeSet::new(),
            last_clicked_row: None,
            last_clicked_column: None,
            table_recently_clicked: false,
            scroll_to_selected_cell: false,
            column_widths: Vec::new(),
            min_column_width: 0.0,
            show_about_dialog: false,
            logo_texture: None,
            database_cache: std::collections::HashMap::new(),
            database_cache_time: std::collections::HashMap::new(),
            show_autocomplete: false,
            autocomplete_suggestions: Vec::new(),
            selected_autocomplete_index: 0,
            autocomplete_prefix: String::new(),
            last_autocomplete_trigger_len: 0,
            pending_cursor_set: None,
            extra_cursors: Vec::new(),
            last_editor_text: String::new(),
            highlight_cache: std::collections::HashMap::new(),
            last_highlight_hash: None,
            show_index_dialog: false,
            index_dialog: None,
            table_bottom_view: models::structs::TableBottomView::Data,
            structure_columns: Vec::new(),
            structure_indexes: Vec::new(),
            structure_col_widths: Vec::new(),
            structure_idx_col_widths: Vec::new(),
            structure_sub_view: models::structs::StructureSubView::Columns,
            last_structure_target: None,
            request_structure_refresh: false,
            adding_column: false,
            new_column_name: String::new(),
            new_column_type: String::new(),
            new_column_nullable: true,
            new_column_default: String::new(),
            editing_column: false,
            edit_column_original_name: String::new(),
            edit_column_name: String::new(),
            edit_column_type: String::new(),
            edit_column_nullable: true,
            edit_column_default: String::new(),
            adding_index: false,
            new_index_name: String::new(),
            new_index_method: String::new(),
            new_index_unique: false,
            new_index_columns: String::new(),
            sql_filter_text: String::new(),
            is_table_browse_mode: false,
            pending_drop_index_name: None,
            pending_drop_index_stmt: None,
            pending_drop_column_name: None,
            pending_drop_column_stmt: None,
            link_editor_theme: self.link_editor_theme,
            config_store: None,
            last_saved_prefs: None,
            prefs_dirty: false,
            prefs_save_feedback: None,
            prefs_last_saved_at: None,
            prefs_loaded: true,
            // Data directory settings
            data_directory: String::new(),
            temp_data_directory: String::new(),
            show_directory_picker: false,
            directory_picker_result: None,
            // Self-update settings
            update_info: None,
            show_update_dialog: false,
            update_check_in_progress: false,
            update_check_error: None,
            last_update_check: None,
            update_download_in_progress: false,
            auto_check_updates: false,
            manual_update_check: false,
            show_update_notification: false,
            update_download_started: false,
            update_installed: false,
            update_install_receiver: None,
            settings_active_pref_tab: PrefTab::ApplicationTheme,
            show_settings_menu: false,
            // Pool wait state (not used in cache clone)
            pool_wait_in_progress: false,
            pool_wait_connection_id: None,
            pool_wait_query: String::new(),
            pool_wait_started_at: None,
            spreadsheet_state: models::structs::SpreadsheetState::default(),
        })
    }
}

/// Update autocomplete state after text change or cursor move.
pub fn update_autocomplete(app: &mut Tabular) {
    let cursor = app.cursor_position.min(app.editor.text.len());
    let (prefix, start_idx) = current_prefix(&app.editor.text, cursor);
    app.autocomplete_prefix = prefix.clone();

    if prefix.is_empty() {
        // hide jika kosong
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        return;
    }

    // Only rebuild if prefix length changed or previously hidden
    if app.last_autocomplete_trigger_len != prefix.len() || !app.show_autocomplete {
        let suggestions = build_suggestions(app, &app.editor.text, cursor, &prefix);
        if suggestions.is_empty() {
            app.show_autocomplete = false;
        } else {
            app.show_autocomplete = true;
            app.autocomplete_suggestions = suggestions;
            app.selected_autocomplete_index = 0;
        }
        app.last_autocomplete_trigger_len = prefix.len();
    }
    // Store start index in last_autocomplete_trigger_len encoded (optional) - keeping simple
    let _ = start_idx; // could be used later for replacement
}

/// Accept currently selected suggestion and replace text.
pub fn accept_current_suggestion(app: &mut Tabular) {
    if !app.show_autocomplete {
        return;
    }
    if let Some(sugg) = app
        .autocomplete_suggestions
        .get(app.selected_autocomplete_index)
    {
        let cursor = app.cursor_position.min(app.editor.text.len());
        let (prefix, start_idx) = current_prefix(&app.editor.text, cursor);
        debug!("Current prefix: '{}', start index: {}", prefix, start_idx);
        // Determine replacement range [start .. cursor]
        let effective_start = if prefix.is_empty() {
            // Scan backwards for contiguous identifier chars just typed
            let bytes = app.editor.text.as_bytes();
            let mut s = cursor;
            while s > 0 {
                let ch = bytes[s - 1] as char;
                if ch.is_alphanumeric() || ch == '_' {
                    s -= 1;
                } else {
                    break;
                }
            }
            s
        } else {
            start_idx
        };
        // Apply via rope edit API. We replace the prefix (if any) with the suggestion
        app.editor.apply_single_replace(effective_start..cursor, sugg);
        // Update cursor position after insertion
        app.cursor_position = effective_start + sugg.len();
        // Update primary selection to caret-only at new cursor
        app.multi_selection.set_primary_range(app.cursor_position, app.cursor_position);
        // Note: We don't have direct ui::Context here; mark a pending cursor set so the editor regains focus next frame
        app.pending_cursor_set = Some(app.cursor_position);
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
    }
}

/// Keyboard navigation for suggestions.
pub fn navigate(app: &mut Tabular, delta: i32) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let len = app.autocomplete_suggestions.len();
    if delta > 0 {
        app.selected_autocomplete_index = (app.selected_autocomplete_index + 1) % len;
    } else if app.selected_autocomplete_index == 0 {
        app.selected_autocomplete_index = len - 1;
    } else {
        app.selected_autocomplete_index -= 1;
    }
}

/// Render dropdown near top-right of editor area (simplified positioning). Call after editor.
pub fn render_autocomplete(app: &mut Tabular, ui: &mut egui::Ui, pos: egui::Pos2) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
    let max_visible = 8usize;
    let visible = app.autocomplete_suggestions.len().min(max_visible);
    let est_height = (visible as f32) * line_height + 8.0;
    let screen = ui.ctx().screen_rect();
    // Hitung lebar ideal berdasarkan suggestion terpanjang (no-wrap) dengan batas min/max
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let mut max_px = 0.0_f32;
    ui.ctx().fonts(|f| {
        for s in &app.autocomplete_suggestions {
            let galley = f.layout_no_wrap(s.to_string(), font_id.clone(), egui::Color32::WHITE);
            if galley.size().x > max_px {
                max_px = galley.size().x;
            }
        }
    });
    let padding = 32.0; // ruang kiri/kanan
    let min_w = 140.0;
    let max_w = 380.0;
    let desired_w = max_px + padding;
    let popup_w = desired_w.clamp(min_w, max_w);
    let mut popup_pos = pos;
    if popup_pos.y + est_height > screen.bottom() {
        popup_pos.y = (popup_pos.y - est_height).max(screen.top());
    }
    if popup_pos.x + popup_w > screen.right() {
        popup_pos.x = (screen.right() - popup_w).max(screen.left());
    }

    egui::Area::new(egui::Id::new("autocomplete_popup"))
        .fixed_pos(popup_pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            // Gunakan frame popup semi-transparan
            let bg = ui.style().visuals.window_fill; // warna dasar window
            let translucent = egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 200); // alpha ~78%
            egui::Frame::popup(ui.style())
                .fill(translucent)
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(200, 0, 0)))
                .corner_radius(4.0)
                .show(ui, |ui| {
                    ui.set_min_width(popup_w);
                    ui.set_max_width(popup_w);
                    egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .show(ui, |ui| {
                            for (i, s) in app.autocomplete_suggestions.iter().enumerate() {
                                let selected = i == app.selected_autocomplete_index;
                                let rich = if selected {
                                    egui::RichText::new(s)
                                        .background_color(ui.style().visuals.selection.bg_fill)
                                        .color(ui.style().visuals.selection.stroke.color)
                                } else {
                                    egui::RichText::new(s)
                                };
                                let resp = ui.selectable_label(selected, rich);
                                if resp.clicked() {
                                    app.selected_autocomplete_index = i;
                                    accept_current_suggestion(app);
                                    // Immediately sync egui TextEdit caret to new position
                                    let id = egui::Id::new("sql_editor");
                                    crate::editor_state_adapter::EditorStateAdapter::set_single(
                                        ui.ctx(),
                                        id,
                                        app.cursor_position,
                                    );
                                    // Immediately request focus back to the main editor widget
                                    ui.memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                                    break;
                                }
                            }
                        });
                    // Removed deprecated style.wrap adjustments; suggestions are short so wrapping off tweak not needed.
                });
        });
}

/// Manual trigger (e.g. Ctrl+Space) even if prefix short. Shows all keywords or filtered list.
pub fn trigger_manual(app: &mut Tabular) {
    update_autocomplete(app); // to refresh prefix
    if app.autocomplete_prefix.is_empty() {
        app.autocomplete_suggestions = SQL_KEYWORDS.iter().map(|s| s.to_string()).collect();
        app.autocomplete_suggestions.sort();
        app.selected_autocomplete_index = 0;
        app.show_autocomplete = true;
    } else {
        // If prefix produces no suggestions, still show keywords
        if app.autocomplete_suggestions.is_empty() {
            app.autocomplete_suggestions = SQL_KEYWORDS
                .iter()
                .filter(|k| {
                    k.to_lowercase()
                        .starts_with(&app.autocomplete_prefix.to_lowercase())
                })
                .map(|s| s.to_string())
                .collect();
            if !app.autocomplete_suggestions.is_empty() {
                app.show_autocomplete = true;
            }
        } else {
            app.show_autocomplete = true;
        }
    }
}
