use crate::cache_data; // for table/column cache access
use crate::{
    models,
    window_egui::{PrefTab, Tabular},
};
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

/// Build suggestions context-aware per requirement.
/// - Jika token sebelumnya FROM -> daftar nama tabel
/// - Jika token sebelumnya SELECT/WHERE atau pasangan GROUP BY -> daftar kolom dari tabel setelah FROM
/// - Selain itu -> SQL_KEYWORDS
pub fn build_suggestions(
    app: &Tabular,
    full_text: &str,
    cursor: usize,
    prefix: &str,
) -> Vec<String> {
    let is_upper = prefix.chars().all(|c| c.is_uppercase());

    if prefix.is_empty() {
        return Vec::new();
    }
    let (_cur_pref, start_idx) = current_prefix(full_text, cursor); // ensure prefix matches
    let before = &full_text[..start_idx.min(full_text.len())];
    let tokens = tokenize(before);
    let last = tokens.last().map(|s| s.to_uppercase());
    let last2 = if tokens.len() >= 2 {
        Some(tokens[tokens.len() - 2].to_uppercase())
    } else {
        None
    };
    // Deteksi apakah sedang berada di dalam daftar SELECT sebelum FROM
    let upper_before = before.to_uppercase();
    let in_select_list = if let Some(sel_pos) = upper_before.rfind("SELECT") {
        // Ada FROM setelah SELECT? kalau belum berarti masih di daftar SELECT
        let after_sel = &upper_before[sel_pos + 6..];
        !after_sel.contains("FROM")
    } else {
        false
    };
    let want_columns = match (last2.as_deref(), last.as_deref()) {
        (Some("GROUP"), Some("BY")) => true,
        (_, Some("SELECT")) => true,
        (_, Some("WHERE")) => true,
        _ => in_select_list, // fallback: jika sedang di SELECT list
    };
    let want_tables = matches!(last.as_deref(), Some("FROM"));

    let low_pref = prefix.to_lowercase();
    let mut out: Vec<String> = Vec::new();

    if want_tables {
        // List table names from cache (table + view)
        if let Some((cid, db)) = active_connection_and_db(app) {
            let clone_for_cache = app.shallow_for_cache();
            for tt in ["table", "view"] {
                if let Some(names) =
                    cache_data::get_tables_from_cache(&clone_for_cache, cid, &db, tt)
                {
                    for n in names {
                        if n.to_lowercase().starts_with(&low_pref) {
                            out.push(n);
                        }
                    }
                }
            }
        }
    } else if want_columns {
        let tables = extract_tables(full_text);
        if let Some((cid, db)) = active_connection_and_db(app) {
            let mut clone_for_cache = app.shallow_for_cache();
            if !tables.is_empty() {
                for table in &tables {
                    if let Some(cols) =
                        cache_data::get_columns_from_cache(&mut clone_for_cache, cid, &db, table)
                    {
                        for (col, _ty) in cols {
                            if col.to_lowercase().starts_with(&low_pref) {
                                out.push(col);
                            }
                        }
                    }
                }
            } else {
                // Belum ada FROM: kumpulkan semua kolom dari semua tabel untuk database ini
                if let Some(all_tables) =
                    cache_data::get_tables_from_cache(&clone_for_cache, cid, &db, "table")
                {
                    for table in all_tables {
                        if let Some(cols) = cache_data::get_columns_from_cache(
                            &mut clone_for_cache,
                            cid,
                            &db,
                            &table,
                        ) {
                            for (col, _ty) in cols.iter() {
                                if col.to_lowercase().starts_with(&low_pref) {
                                    out.push(col.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        // Jika tetap kosong, fallback ke keywords
        if out.is_empty() {
            for k in SQL_KEYWORDS {
                if k.to_lowercase().starts_with(&low_pref) {
                    out.push((*k).to_string());
                }
            }
        }
    } else {
        // Keywords default
        for k in SQL_KEYWORDS {
            if k.to_lowercase().starts_with(&low_pref) {
                out.push((*k).to_string());
            }
        }
    }

    // Dedup & sort
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.to_lowercase()));
    out.sort_unstable();

    if is_upper {
        out = out.into_iter().map(|s| s.to_uppercase()).collect();
    } else {
        out = out.into_iter().map(|s| s.to_lowercase()).collect();
    }
    out
}

// Removed unused TreeNodeExt trait & impl (was an accessor wrapper) to silence dead_code warning.

// Provide a lightweight clone for cache access (cache functions require &mut Tabular)
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
            editor_text: String::new(),
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
            lapce_buffer: Some(lapce_core::buffer::Buffer::new("")),
        })
    }
}

/// Update autocomplete state after text change or cursor move.
pub fn update_autocomplete(app: &mut Tabular) {
    let cursor = app.cursor_position.min(app.editor_text.len());
    let (prefix, start_idx) = current_prefix(&app.editor_text, cursor);
    app.autocomplete_prefix = prefix.clone();

    if prefix.is_empty() {
        // hide jika kosong
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        return;
    }

    // Only rebuild if prefix length changed or previously hidden
    if app.last_autocomplete_trigger_len != prefix.len() || !app.show_autocomplete {
        let suggestions = build_suggestions(app, &app.editor_text, cursor, &prefix);
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
        let cursor = app.cursor_position.min(app.editor_text.len());
        let (prefix, start_idx) = current_prefix(&app.editor_text, cursor);
        debug!("Current prefix: '{}', start index: {}", prefix, start_idx);
        // If prefix empty but we still want to accept (e.g., early Tab) try to look back until whitespace
        let (effective_start, effective_prefix_len) = if prefix.is_empty() {
            // Scan backwards for contiguous identifier chars just typed
            let bytes = app.editor_text.as_bytes();
            let mut s = cursor;
            while s > 0 {
                let ch = bytes[s - 1] as char;
                if ch.is_alphanumeric() || ch == '_' {
                    s -= 1;
                } else {
                    break;
                }
            }
            (s, cursor - s)
        } else {
            (start_idx, prefix.len())
        };

        if effective_prefix_len > 0 || !prefix.is_empty() {
            let mut new_text = String::with_capacity(app.editor_text.len() + sugg.len());
            new_text.push_str(&app.editor_text[..effective_start]);
            new_text.push_str(sugg);
            new_text.push_str(&app.editor_text[cursor..]);
            app.editor_text = new_text;
            app.cursor_position = effective_start + sugg.len();
        }
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
