use eframe::egui;
use eframe::egui::text_edit::TextEditState;
use egui::text::{CCursor, CCursorRange};
// Using adapter for cursor state (removes direct TextEditState dependency from rest of file)
// syntax highlighting module temporarily disabled
use log::debug;
use sqlformat::{QueryParams, format as sqlfmt};

use crate::{
    connection, data_table, directory, editor, editor_autocomplete, models, query_tools,
    sidebar_history, sidebar_query, window_egui,
};
use crate::spreadsheet::SpreadsheetOperations;
use std::borrow::Cow;
use std::time::Instant;

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
        result_column_metadata: None,
        results: Vec::new(),
        active_result_index: 0,
        is_table_browse_mode: false,
        current_page: 0,
        page_size: 500, // default page size aligns with global default
        total_rows: 0,
        base_query: String::new(), // Empty base query initially
        dba_special_mode: None,
        object_ddl: None,
        query_message: String::new(),
        query_message_is_error: false,
        diagram_state: None,
        should_run_on_open: false,
        http_client_state: None,
        redis_browser_state: None,
        tx_mode: false,
        tx_active: false,
        session: None,
    };

    tabular.query_tabs.push(new_tab);
    let new_index = tabular.query_tabs.len() - 1;
    tabular.active_tab_index = new_index;

    // Update editor with new tab content
    tabular.editor.set_text(content.clone());
    tabular.highlight_cache.clear();
    tabular.last_highlight_hash = None;
    tabular.sql_semantic_snapshot = None;
    // Clear global result state so a fresh tab starts clean (no lingering table below)
    tabular.current_table_headers.clear();
    tabular.current_table_data.clear();
    tabular.all_table_data.clear();
    tabular.current_table_name.clear();
    tabular.total_rows = 0;
    tabular.is_table_browse_mode = false;
    tabular.current_object_ddl = None;

    tab_id
}

// Convenience: create a new query tab and pre-assign a connection
pub(crate) fn create_new_tab_with_connection(
    tabular: &mut window_egui::Tabular,
    title: String,
    content: String,
    connection_id: Option<i64>,
) -> usize {
    let tab_id = create_new_tab(tabular, title, content);
    if let Some(active_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        active_tab.connection_id = connection_id;
        // Keep global current_connection_id in sync with the newly created tab
        tabular.current_connection_id = connection_id;
        // New tabs have no database selected by default here; leave as-is
    }
    tab_id
}

// Convenience: create a new query tab and pre-assign connection + database context
pub(crate) fn create_new_tab_with_connection_and_database(
    tabular: &mut window_egui::Tabular,
    title: String,
    content: String,
    connection_id: Option<i64>,
    database_name: Option<String>,
) -> usize {
    let tab_id = create_new_tab(tabular, title, content);
    if let Some(active_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        active_tab.connection_id = connection_id;
        active_tab.database_name = database_name.clone();
        // Sync global state with the tab's assigned connection
        tabular.current_connection_id = connection_id;
    }
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
            // Clear per-tab result state as well
            tab.result_headers.clear();
            tab.result_rows.clear();
            tab.result_all_rows.clear();
            tab.result_table_name.clear();
            tab.is_table_browse_mode = false;
            tab.current_page = 0;
            // Keep default page_size as-is (don't force overwrite)
            tab.total_rows = 0;
            tab.base_query.clear();
            tab.has_executed_query = false;
            tab.dba_special_mode = None;
            tab.object_ddl = None;
        }
        tabular.editor.set_text(String::new());
        tabular.highlight_cache.clear();
        tabular.last_highlight_hash = None;
        tabular.sql_semantic_snapshot = None;
        // Also clear global result state so the UI table area is reset
        tabular.current_table_headers.clear();
        tabular.current_table_data.clear();
        tabular.all_table_data.clear();
        tabular.current_table_name.clear();
        tabular.total_rows = 0;
        tabular.is_table_browse_mode = false;
        tabular.current_page = 0;
        // Keep the configured default page_size; don't override
        tabular.current_base_query.clear();
        tabular.current_connection_id = None;
        tabular.current_object_ddl = None;
        return;
    }

    if tab_index < tabular.query_tabs.len() {
        // End the tab's manual-commit session (implicit rollback), if any.
        if let Some(session) = tabular.query_tabs[tab_index].session.take() {
            session.close();
        }
        tabular.query_tabs.remove(tab_index);

        // Adjust active tab index
        if tabular.active_tab_index >= tabular.query_tabs.len() {
            tabular.active_tab_index = tabular.query_tabs.len() - 1;
        } else if tabular.active_tab_index > tab_index {
            tabular.active_tab_index -= 1;
        }

        // Update editor with active tab content
        if let Some(active_tab) = tabular.query_tabs.get(tabular.active_tab_index) {
            tabular.editor.set_text(active_tab.content.clone());
            tabular.highlight_cache.clear();
            tabular.last_highlight_hash = None;
            tabular.sql_semantic_snapshot = None;
        }
        tabular.current_object_ddl = None;
    }
}

/// Find an already-open tab representing the same title/connection/database,
/// so callers can activate it instead of opening a duplicate tab.
pub(crate) fn find_tab_for_target(
    tabular: &window_egui::Tabular,
    title: &str,
    connection_id: i64,
    database_name: Option<&str>,
) -> Option<usize> {
    tabular.query_tabs.iter().position(|tab| {
        tab.title == title
            && tab.connection_id == Some(connection_id)
            && match (database_name, tab.database_name.as_deref()) {
                (Some(expected), Some(existing)) => expected == existing,
                (Some(_), None) => false,
                _ => true,
            }
    })
}

pub(crate) fn switch_to_tab(tabular: &mut window_egui::Tabular, tab_index: usize) {
    let mut need_connect: Option<i64> = None;
    if tab_index < tabular.query_tabs.len() {
        // Save current tab content
        if let Some(current_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            if current_tab.content != tabular.editor.text {
                current_tab.content = tabular.editor.text.clone();
                current_tab.is_modified = true;
            }
            // Persist current global result state into the tab before switching (swap to avoid clones)
            std::mem::swap(
                &mut current_tab.result_headers,
                &mut tabular.current_table_headers,
            );
            std::mem::swap(
                &mut current_tab.result_rows,
                &mut tabular.current_table_data,
            );
            std::mem::swap(
                &mut current_tab.result_all_rows,
                &mut tabular.all_table_data,
            );
            std::mem::swap(
                &mut current_tab.result_table_name,
                &mut tabular.current_table_name,
            );
            current_tab.is_table_browse_mode = tabular.is_table_browse_mode;
            current_tab.current_page = tabular.current_page;
            current_tab.page_size = tabular.page_size;
            current_tab.total_rows = tabular.total_rows;
            std::mem::swap(&mut current_tab.base_query, &mut tabular.current_base_query);
            debug!(
                "💾 Saving tab {} state (swap): base_query='{}'",
                tabular.active_tab_index, current_tab.base_query
            );
            std::mem::swap(&mut current_tab.object_ddl, &mut tabular.current_object_ddl);
            // Save query message state
            current_tab.query_message = tabular.query_message.clone();
            current_tab.query_message_is_error = tabular.query_message_is_error;
            // dba_special_mode already resides on current_tab; no action required here
        }

        // Switch to new tab
        tabular.active_tab_index = tab_index;
        if let Some(new_tab) = tabular.query_tabs.get_mut(tab_index) {
            tabular.editor.set_text(new_tab.content.clone());
            tabular.highlight_cache.clear();
            tabular.last_highlight_hash = None;
            tabular.sql_semantic_snapshot = None;
            // Restore per-tab result state into global display (swap to avoid clones)
            std::mem::swap(
                &mut tabular.current_table_headers,
                &mut new_tab.result_headers,
            );
            std::mem::swap(&mut tabular.current_table_data, &mut new_tab.result_rows);
            std::mem::swap(&mut tabular.all_table_data, &mut new_tab.result_all_rows);
            std::mem::swap(
                &mut tabular.current_table_name,
                &mut new_tab.result_table_name,
            );
            tabular.is_table_browse_mode = new_tab.is_table_browse_mode;
            tabular.current_page = new_tab.current_page;
            tabular.page_size = new_tab.page_size;
            tabular.total_rows = new_tab.total_rows;
            std::mem::swap(&mut tabular.current_base_query, &mut new_tab.base_query);
            debug!(
                "📂 Restoring tab {} state (swap): base_query='{}', connection_id={:?}",
                tab_index, tabular.current_base_query, new_tab.connection_id
            );
            std::mem::swap(&mut tabular.current_object_ddl, &mut new_tab.object_ddl);
            // IMPORTANT: kembalikan connection id aktif sesuai tab baru
            tabular.current_connection_id = new_tab.connection_id;
            // Restore query message state
            tabular.query_message = new_tab.query_message.clone();
            tabular.query_message_is_error = new_tab.query_message_is_error;
            tabular.show_message_panel = !tabular.query_message.is_empty();
            // dba_special_mode automatically follows with new_tab

            // Auto-connect restoration: jika tab memiliki connection_id dan pool belum siap, trigger creation
            if let Some(conn_id) = new_tab.connection_id {
                let has_pool = tabular.connection_pools.contains_key(&conn_id)
                    || tabular
                        .shared_connection_pools
                        .lock()
                        .map(|p| p.contains_key(&conn_id))
                        .unwrap_or(false);
                if !has_pool {
                    need_connect = Some(conn_id);
                }
            }

            // Jika user sedang berada di tampilan Structure dan tab tujuan adalah tab Table, reload struktur tabel tsb.
            if tabular.table_bottom_view == models::structs::TableBottomView::Structure
                && (new_tab.title.starts_with("Table:") || new_tab.title.starts_with("View:"))
            {
                // load_structure_info_for_current_table adalah metode pada Tabular (dibuat pub(crate))
                data_table::load_structure_info_for_current_table(tabular);
            }
        }
    }
    // Deferred connection attempt after borrows released: trigger background creation without blocking UI
    if let Some(conn_id) = need_connect {
        log::debug!(
            "Triggering background connection pool creation for {}",
            conn_id
        );
        crate::connection::ensure_background_pool_creation(tabular, conn_id);
        // Optionally, we can set a friendly status indicator here if needed.
    }
}

pub(crate) fn save_current_tab(tabular: &mut window_egui::Tabular) -> Result<(), String> {
    // HTTP API tabs: save the HTTP client state to disk instead of showing SQL save dialog
    if let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index)
        && tab.http_client_state.is_some() {
            if let (Some(conn_id), Some(state)) = (tab.connection_id, &tab.http_client_state) {
                crate::http_client::save_http_state(conn_id, state);
            }
            return Ok(());
        }
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        // Ensure content holds editor text plus metadata header (id + optional db)
        let mut final_content = tabular.editor.text.clone();
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

        if let Some(path) = &tab.file_path {
            // File already exists, save directly
            let file_path = path.clone();
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
    // Keep editor content as-is when saving (no auto-format on save)
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        // Mirror header injection as in save_current_tab
        let mut final_content = tabular.editor.text.clone();
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

// Helper function to handle CMD+D / CTRL+D - Add next occurrence to multi-selection
//
// This implements VSCode-style multi-cursor behavior:
// 1. First CMD+D: Select word under cursor (if no selection) or use current selection
// 2. Subsequent CMD+D: Find and add next occurrence of the selected text
// 3. All cursors stay active and typing applies to all positions
// 4. Press Escape or navigate with arrow keys to clear multi-selection
fn handle_add_next_occurrence(tabular: &mut window_egui::Tabular, ui: &egui::Ui) {
    let id = ui.make_persistent_id("sql_editor");

    // Get current selection or word under cursor
    let (sel_start, sel_end) = if tabular.selection_start != tabular.selection_end {
        // Use existing selection
        (tabular.selection_start, tabular.selection_end)
    } else {
        // No selection: select word under cursor
        let pos = tabular.cursor_position.min(tabular.editor.text.len());
        let (word_start, word_end) = find_word_boundaries(&tabular.editor.text, pos);
        if word_start < word_end {
            // Update selection to the word
            tabular.selection_start = word_start;
            tabular.selection_end = word_end;
            (word_start, word_end)
        } else {
            // No word found, do nothing
            log::debug!("🎯 CMD+D: No word found under cursor");
            return;
        }
    };

    let selected_text = if sel_start < sel_end && sel_end <= tabular.editor.text.len() {
        tabular.editor.text[sel_start..sel_end].to_string()
    } else {
        return;
    };

    if selected_text.is_empty() {
        log::debug!("🎯 CMD+D: Empty selection, nothing to find");
        return;
    }

    log::debug!(
        "🎯 CMD+D: Selected text='{}' at {}..{}",
        selected_text.escape_debug(),
        sel_start,
        sel_end
    );

    // Initialize multi-selection with current selection if it's the first occurrence
    if tabular.multi_selection.is_empty() {
        tabular
            .multi_selection
            .set_primary_range(sel_start, sel_end);
        log::debug!(
            "🎯 Initialized multi-selection with primary range: {}..{}",
            sel_start,
            sel_end
        );

        // Store the selected text for visual feedback
        tabular.selected_text = selected_text.clone();
        ui.ctx().request_repaint();

        // Don't search for next occurrence on first CMD+D, just initialize
        return;
    }

    // Debug: print all existing regions
    log::debug!("🎯 Existing regions before add:");
    for (i, r) in tabular.multi_selection.regions().iter().enumerate() {
        let text_at_region = &tabular.editor.text[r.min()..r.max()];
        log::debug!(
            "   [{}] {}..{} = '{}'",
            i,
            r.min(),
            r.max(),
            text_at_region.escape_debug()
        );
    }

    // Add next occurrence
    let found = tabular
        .multi_selection
        .add_next_occurrence(&tabular.editor.text, &selected_text);

    if found {
        log::debug!(
            "✅ Added next occurrence. Total selections: {}",
            tabular.multi_selection.len()
        );

        // Debug: print all regions after add
        log::debug!("🎯 All regions after add:");
        for (i, r) in tabular.multi_selection.regions().iter().enumerate() {
            let text_at_region = &tabular.editor.text[r.min()..r.max()];
            log::debug!(
                "   [{}] {}..{} = '{}'",
                i,
                r.min(),
                r.max(),
                text_at_region.escape_debug()
            );
        }

        // Update visual feedback
        tabular.selected_text = selected_text.clone();

        // Get the last added selection to update cursor position
        if let Some(last_region) = tabular.multi_selection.regions().last() {
            let last_end = last_region.max();
            tabular.cursor_position = last_end;
            tabular.selection_start = last_region.min();
            tabular.selection_end = last_end;

            // Sync with egui state
            let to_char_index = |s: &str, byte_idx: usize| -> usize {
                let b = byte_idx.min(s.len());
                s[..b].chars().count()
            };

            let start_ci = to_char_index(&tabular.editor.text, last_region.min());
            let end_ci = to_char_index(&tabular.editor.text, last_end);
            crate::editor_state_adapter::EditorStateAdapter::set_selection(
                ui.ctx(),
                id,
                start_ci,
                end_ci,
                end_ci,
            );
        }

        ui.ctx().request_repaint();
    } else {
        log::debug!(
            "ℹ️ No more occurrences found for '{}'",
            selected_text.escape_debug()
        );
    }
}

fn clear_multi_selection_state(tabular: &mut window_egui::Tabular, ui: &egui::Ui, reason: &str) {
    tabular.multi_selection.clear();
    tabular.clear_extra_cursors();
    tabular.selected_text.clear();
    let caret = tabular.cursor_position.min(tabular.editor.text.len());
    tabular.cursor_position = caret;
    tabular.selection_start = caret;
    tabular.selection_end = caret;
    tabular.selection_force_clear = true;
    tabular.pending_cursor_set = Some(caret);
    tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(6);

    let id = ui.make_persistent_id("sql_editor");
    let s = &tabular.editor.text;
    let caret_chars = s[..caret].chars().count();
    crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, caret_chars);
    ui.memory_mut(|m| m.request_focus(id));
    ui.ctx().request_repaint();
    log::debug!("🎯 Multi-selection cleared {reason}");
}

// Helper: Find word boundaries at the given position (for selecting word under cursor)
fn find_word_boundaries(text: &str, pos: usize) -> (usize, usize) {
    use unicode_segmentation::UnicodeSegmentation;

    let pos = pos.min(text.len());

    // Find word boundaries using Unicode word segmentation
    let mut word_start = pos;
    let mut word_end = pos;

    // Find all word boundaries
    for (idx, word) in text.unicode_word_indices() {
        let start = idx;
        let end = start + word.len();

        // Check if position is within this word
        if pos >= start && pos <= end {
            word_start = start;
            word_end = end;
            break;
        }
    }

    (word_start, word_end)
}

#[inline]
fn clamp_char_boundary_left(text: &str, idx: usize) -> usize {
    let len = text.len();
    let mut pos = idx.min(len);
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[inline]
fn clamp_char_boundary_right(text: &str, idx: usize) -> usize {
    let len = text.len();
    let mut pos = idx.min(len);
    while pos < len && !text.is_char_boundary(pos) {
        pos += 1;
    }
    pos
}

#[inline]
fn slice_on_char_boundaries(
    text: &str,
    start: usize,
    end: usize,
) -> Option<(usize, usize, String)> {
    let s = clamp_char_boundary_left(text, start);
    let e = clamp_char_boundary_right(text, end);
    if s <= e && e <= text.len() {
        Some((s, e, text[s..e].to_string()))
    } else {
        None
    }
}

pub(crate) fn render_advanced_editor(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    let mut request_scroll_to_cursor = false;
    let mut inserted_newline_this_frame = false;
    let editor_id = ui.make_persistent_id("sql_editor");

    // Shortcut: Format SQL (Cmd/Ctrl + Shift + F)
    let mut trigger_format_sql = false;
    ui.input(|i| {
        // Accept platform command (command on macOS, control elsewhere)
        if (i.modifiers.mac_cmd || i.modifiers.command)
            && i.modifiers.shift
            && i.key_pressed(egui::Key::F)
        {
            trigger_format_sql = true;
        }
    });
    if trigger_format_sql {
        // Consume the key event so TextEdit doesn't see it
        ui.ctx().input_mut(|ri| {
            ri.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::F,
                        pressed: true,
                        ..
                    }
                )
            });
        });
        reformat_current_sql(tabular, ui);
        request_scroll_to_cursor = true;
        // Early repaint for snappy UX
        ui.ctx().request_repaint();
    }
    
    // Shortcut: Toggle Comment (Cmd/Ctrl + /)
    let mut trigger_toggle_comment = false;
    ui.input(|i| {
        if (i.modifiers.mac_cmd || i.modifiers.command)
            && !i.modifiers.shift
            && i.key_pressed(egui::Key::Slash)
        {
            trigger_toggle_comment = true;
        }
    });
    if trigger_toggle_comment {
        // Consume the key event so TextEdit doesn't see it
        ui.ctx().input_mut(|ri| {
            ri.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Slash,
                        pressed: true,
                        ..
                    }
                )
            });
        });
        toggle_line_comment(tabular);
        request_scroll_to_cursor = true;
        // Early repaint for snappy UX
        ui.ctx().request_repaint();
    }

    // Shortcut: Toggle AI Panel (Cmd/Ctrl + Shift + A)
    let mut trigger_toggle_ai = false;
    ui.input(|i| {
        if (i.modifiers.mac_cmd || i.modifiers.command)
            && i.modifiers.shift
            && i.key_pressed(egui::Key::A)
        {
            trigger_toggle_ai = true;
        }
    });
    if trigger_toggle_ai {
        ui.ctx().input_mut(|ri| {
            ri.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::A,
                        pressed: true,
                        ..
                    }
                )
            });
        });
        tabular.show_ai_panel = !tabular.show_ai_panel;
        if tabular.show_ai_panel && tabular.ai_input.is_empty() {
            // Pre-fill the AI prompt with selected text or the whole editor content (capped)
            let sel = if tabular.selection_start < tabular.selection_end
                && tabular.selection_end <= tabular.editor.text.len()
            {
                tabular.editor.text[tabular.selection_start..tabular.selection_end].to_string()
            } else {
                String::new()
            };
            tabular.ai_input = if sel.is_empty() {
                String::new()
            } else {
                format!("About this SQL:\n```sql\n{sel}\n```\n")
            };
        }
        ui.ctx().request_repaint();
    }
    
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
    let rows = if tabular.advanced_editor.desired_rows > 0 {
        tabular.advanced_editor.desired_rows
    } else {
        25
    };
    let mut tab_pressed_pre = ui.input(|i| i.key_pressed(egui::Key::Tab));
    let shift_pressed_pre = ui.input(|i| i.modifiers.shift);
    let (sel_start, sel_end) = (tabular.selection_start, tabular.selection_end);
    if tab_pressed_pre && sel_start < sel_end && sel_end <= tabular.editor.text.len() {
        let slice = &tabular.editor.text[sel_start..sel_end];
        if slice.contains('\n') {
            // multi-line
            // Find first line start
            let mut line_start = sel_start;
            while line_start > 0 && tabular.editor.text.as_bytes()[line_start - 1] != b'\n' {
                line_start -= 1;
            }
            let sel_end_clamped = sel_end.min(tabular.editor.text.len());
            let block = tabular.editor.text[line_start..sel_end_clamped].to_string();
            if !shift_pressed_pre {
                let mut indented = String::with_capacity(block.len() + 8);
                for line in block.split_inclusive('\n') {
                    if line == "\n" {
                        indented.push('\n');
                        continue;
                    }
                    let (content, nl) = if let Some(p) = line.rfind('\n') {
                        (&line[..p], &line[p..])
                    } else {
                        (line, "")
                    };
                    indented.push('\t');
                    indented.push_str(content);
                    indented.push_str(nl);
                }
                // Apply via rope edit API for consistency
                tabular
                    .editor
                    .apply_single_replace(line_start..sel_end_clamped, &indented);
                tabular.selection_start = line_start;
                tabular.selection_end = line_start + indented.len();
                tabular.cursor_position = tabular.selection_end;
                request_scroll_to_cursor = true;
            } else {
                let mut outdented = String::with_capacity(block.len());
                let mut changed = false;
                for line in block.split_inclusive('\n') {
                    if line == "\n" {
                        outdented.push('\n');
                        continue;
                    }
                    let (content, nl) = if let Some(p) = line.rfind('\n') {
                        (&line[..p], &line[p..])
                    } else {
                        (line, "")
                    };
                    let trimmed = if let Some(rest) = content.strip_prefix('\t') {
                        changed = true;
                        rest
                    } else if let Some(rest) = content.strip_prefix("    ") {
                        changed = true;
                        rest
                    } else {
                        content
                    };
                    outdented.push_str(trimmed);
                    outdented.push_str(nl);
                }
                if changed {
                    tabular
                        .editor
                        .apply_single_replace(line_start..sel_end_clamped, &outdented);
                    tabular.selection_start = line_start;
                    tabular.selection_end = line_start + outdented.len();
                    tabular.cursor_position = tabular.selection_end;
                    request_scroll_to_cursor = true;
                }
            }
            // consume Tab key event so TextEdit tidak menambah tab baru
            ui.ctx().input_mut(|ri| {
                ri.events.retain(|e| {
                    !matches!(
                        e,
                        egui::Event::Key {
                            key: egui::Key::Tab,
                            ..
                        }
                    )
                })
            });
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.content = tabular.editor.text.clone();
                tab.is_modified = true;
            }
        }
    }
    // Track autocomplete visibility to restore focus when popup closes this frame
    let autocomplete_was_visible_at_start = tabular.show_autocomplete;

    // ----- Handle autocomplete key interception and pre-acceptance BEFORE building TextEdit -----
    let mut enter_pressed_pre = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let mut raw_tab = false;
    // VSCode-like navigation/action flags
    let mut multi_nav_left = false;

    let mut multi_nav_right = false;
    let mut multi_nav_up = false;
    let mut multi_nav_down = false;
    let mut multi_nav_home = false;
    let mut multi_nav_end = false;
    let mut multi_nav_home_extend = false;
    let mut multi_nav_end_extend = false;
    let mut multi_extend_left = false;
    let mut multi_extend_right = false;
    let mut multi_extend_up = false;
    let mut multi_extend_down = false;
    let mut single_nav_home = false;
    let mut single_nav_end = false;
    let mut single_nav_home_extend = false;
    let mut single_nav_end_extend = false;
    let mut move_line_up = false;
    let mut move_line_down = false;
    let mut dup_line_up = false;
    let mut dup_line_down = false;
    let mut multi_edit_pre_applied = false;
    let mut intercepted_multi_texts: Vec<String> = Vec::new();
    let mut intercepted_multi_pastes: Vec<String> = Vec::new();
    let mut intercept_multi_backspace = false;
    let mut intercept_multi_delete = false;
    let mut copy_requested = false;
    let mut cut_requested = false;
    // Intercept arrow keys when autocomplete popup shown so caret tidak ikut bergerak
    let mut arrow_down_pressed = false;
    let mut arrow_up_pressed = false;
    ui.input(|i| {
        let cmd_or_ctrl = i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd;
        if cmd_or_ctrl && i.key_pressed(egui::Key::C) {
            copy_requested = true;
        }
        if cmd_or_ctrl && i.key_pressed(egui::Key::X) {
            cut_requested = true;
        }
        for ev in &i.events {
            match ev {
                egui::Event::Key {
                    key: egui::Key::Tab,
                    pressed: true,
                    ..
                } => {
                    raw_tab = true;
                }
                egui::Event::Copy => {
                    copy_requested = true;
                }
                egui::Event::Cut => {
                    copy_requested = true;
                    cut_requested = true;
                }
                egui::Event::Key {
                    key: egui::Key::C,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command || modifiers.ctrl || modifiers.mac_cmd => {
                    copy_requested = true;
                }
                egui::Event::Key {
                    key: egui::Key::X,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command || modifiers.ctrl || modifiers.mac_cmd => {
                    cut_requested = true;
                }
                _ => {}
            }
        }
    });

    if copy_requested || cut_requested {
        ui.ctx().input_mut(|ri| {
            ri.events.retain(|ev| match ev {
                egui::Event::Copy | egui::Event::Cut => false,
                egui::Event::Key {
                    key: egui::Key::C,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command || modifiers.ctrl || modifiers.mac_cmd => false,
                egui::Event::Key {
                    key: egui::Key::X,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.command || modifiers.ctrl || modifiers.mac_cmd => false,
                _ => true,
            });
        });
    }
    // Defer actual accept application until after TextEdit is rendered to avoid borrow conflicts
    let mut defer_accept_autocomplete = false;
    if tabular.show_autocomplete {
        ui.ctx().input_mut(|ri| {
            // Drain & filter events: buang ArrowUp/ArrowDown pressed supaya TextEdit tidak memproses
            let mut kept = Vec::with_capacity(ri.events.len());
            let mut _enter_consumed = false; // renamed to silence unused warning
            for ev in ri.events.drain(..) {
                match ev {
                    egui::Event::Key {
                        key: egui::Key::ArrowDown,
                        pressed: true,
                        ..
                    } => {
                        arrow_down_pressed = true;
                        // user navigated popup
                        tabular.autocomplete_navigated = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::ArrowUp,
                        pressed: true,
                        ..
                    } => {
                        arrow_up_pressed = true;
                        // user navigated popup
                        tabular.autocomplete_navigated = true;
                    }
                    // Smart Enter handling: only consume if we should accept autocomplete
                    e @ egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        // Heuristic: accept on Enter if user navigated OR
                        //  - there is only one suggestion OR
                        //  - selected suggestion extends current prefix (case-insensitive)
                        let mut should_accept = tabular.autocomplete_navigated;

                        if !should_accept {
                            let sugg_count = tabular.autocomplete_suggestions.len();
                            if sugg_count == 1 {
                                should_accept = true;
                            } else {
                                let prefix = tabular.autocomplete_prefix.clone();
                                if let Some(sugg) = tabular
                                    .autocomplete_suggestions
                                    .get(tabular.selected_autocomplete_index)
                                    && !prefix.is_empty()
                                {
                                    let p = prefix.to_lowercase();
                                    let s = sugg.to_lowercase();
                                    if s.starts_with(&p) {
                                        should_accept = true;
                                    }
                                }
                            }
                        }

                        if should_accept {
                            enter_pressed_pre = true; // we'll accept suggestion
                            _enter_consumed = true;
                        } else {
                            // don't consume: let TextEdit insert newline
                            kept.push(e);
                        }
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
    // VSCode-like word navigation & line operations (pre-TextEdit)
    // Helper: compute previous and next word boundaries using Unicode segmentation (UAX#29)



    // Helper: convert byte index -> char index for egui CCursor
    let to_char_index = |s: &str, byte_idx: usize| -> usize {
        let b = byte_idx.min(s.len());
        s[..b].chars().count()
    };
    // Helper: convert char index -> byte index for our rope/editor
    let to_byte_index = |s: &str, char_idx: usize| -> usize {
        match s.char_indices().nth(char_idx) {
            Some((b, _)) => b,
            None => s.len(),
        }
    };
    // Auto-close quotes and Overtype behavior
    // Detect ' or " input.
    // If Custom View dialog is open, skip to avoid interference.
    if !tabular.show_add_view_dialog && tabular.multi_selection.len() <= 1 {
        let handle_quote = ui.input(|i| {
            i.events.iter().find_map(|ev| match ev {
                egui::Event::Text(text) if text == "'" || text == "\"" => Some(text.clone()),
                _ => None,
            })
        });

        if let Some(quote_char) = handle_quote {
            let cursor = tabular.cursor_position;
            let text_len = tabular.editor.text.len();
            let safe_cursor = cursor.min(text_len);
            
            // Check character valid for auto-close (at end, or before whitespace/closer)
            let next_char = tabular.editor.text[safe_cursor..].chars().next();
            // Allow auto-close if next char is whitespace/empty or closing punctuation
            let should_autoclose = match next_char {
                None => true, // End of file
                Some(c) => c.is_whitespace() || c == ')' || c == ']' || c == '}' || c == ',' || c == ';'
            };
            
            // Special Overtype case: cursor is before matching quote
            let is_overtype = if let Some(c) = next_char {
                 c.to_string() == quote_char
            } else {
                 false
            };

            let mut handled = false;

            if is_overtype {
                // Just move cursor forward
                tabular.cursor_position += 1;
                tabular.selection_start = tabular.cursor_position;
                tabular.selection_end = tabular.cursor_position;
                handled = true;
                request_scroll_to_cursor = true;
                log::debug!("Overtyped quote '{}'", quote_char);
            } else if should_autoclose {
                // Insert quote pair: quote + quote
                let pair = format!("{}{}", quote_char, quote_char);
                tabular.editor.apply_single_replace(safe_cursor..safe_cursor, &pair);
                
                // Move cursor between them
                tabular.cursor_position += 1;
                tabular.selection_start = tabular.cursor_position;
                tabular.selection_end = tabular.cursor_position;
                handled = true;
                log::debug!("Auto-closed quote '{}'", quote_char);
            }

            if handled {
                // Sync egui state
                let id = editor_id;
                
                // FORCE UPDATE of egui TextEdit state immediately
                // We must update the internal state so TextEdit knows the cursor moved
                if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
                     let ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
                     state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(ci))));
                     state.store(ui.ctx(), id);
                } else {
                     // Fallback if state doesn't exist yet (first frame?)
                     let ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
                     crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, ci);
                }

                // Consume the text event so TextEdit doesn't insert another quote
                ui.ctx().input_mut(|ri| {
                    let mut consumed = false;
                    ri.events.retain(|e| {
                        if !consumed {
                             match e {
                                egui::Event::Text(t) if t == &quote_char => {
                                    consumed = true;
                                    return false;
                                }
                                _ => {}
                             }
                        }
                        true
                    });
                });
                
                // Mark modified
                if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                     tab.content = tabular.editor.text.clone();
                     tab.is_modified = true;
                } else {
                     tabular.editor.mark_text_modified();
                }

                ui.ctx().request_repaint();
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
    }

    // Pre-handle Delete/Backspace when a selection exists: remove the whole selection (not just one char)
    // This ensures expected behavior “press Delete removes all selected text”.
    // SKIP this handling if Custom View dialog is open OR if a cell is being edited to avoid consuming backspace events
    if !tabular.show_add_view_dialog && tabular.spreadsheet_state.editing_cell.is_none() {
        let id = editor_id;
        let mut do_delete_selection = false;
        let mut del_key_consumed = false;
        let mut has_selection = false;

        // Check for key presses first
        let (pressed_bs, pressed_del) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Backspace),
                i.key_pressed(egui::Key::Delete),
            )
        });

        if pressed_bs || pressed_del {
            // Method 1: Check egui state selection (char indices)
            if let Some(rng) =
                crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id)
                && rng.start != rng.end
            {
                has_selection = true;
                log::debug!(
                    "Selection detected via egui state: {} to {}",
                    rng.start,
                    rng.end
                );
            }

            // Method 2: Fallback to stored selection state (byte indices)
            if !has_selection && tabular.selection_start != tabular.selection_end {
                has_selection = true;
                log::debug!(
                    "Selection detected via stored state: {} to {}",
                    tabular.selection_start,
                    tabular.selection_end
                );
            }

            // Method 3: Check if there's selected text
            if !has_selection && !tabular.selected_text.is_empty() {
                has_selection = true;
                log::debug!(
                    "Selection detected via selected_text: '{}'",
                    tabular.selected_text
                );
            }

            // Only intercept and handle deletion if there's actually a selection
            // If no selection, let egui TextEdit handle normal Delete/Backspace behavior
            if has_selection {
                do_delete_selection = true;
                del_key_consumed = true;
                log::debug!(
                    "Will delete selection on {} key",
                    if pressed_del { "Delete" } else { "Backspace" }
                );

                // Remove the key event so TextEdit doesn't do additional mutation
                ui.ctx().input_mut(|ri| {
                    ri.events.retain(|e| {
                        !matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Backspace,
                                pressed: true,
                                ..
                            }
                        ) && !matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Delete,
                                pressed: true,
                                ..
                            }
                        )
                    });
                });
            } else {
                // No selection - let egui TextEdit handle normal Delete/Backspace
                log::debug!(
                    "No selection detected, letting TextEdit handle {} key normally",
                    if pressed_del { "Delete" } else { "Backspace" }
                );
                // Proactively request a repaint to avoid any visual lag/stale frame (outside locks)
                ui.ctx().request_repaint();
            }
        }

        if do_delete_selection {
            let mut handled_multi_delete = false;
            if tabular.multi_selection.len() > 1 {
                if tabular.multi_selection.has_expanded_ranges() {
                    log::debug!(
                        "[multi] Deleting {} expanded selections via {} key",
                        tabular.multi_selection.len(),
                        if del_key_consumed {
                            "Delete"
                        } else {
                            "Backspace"
                        }
                    );
                    tabular
                        .multi_selection
                        .apply_replace_selected(&mut tabular.editor.text, "");
                } else if pressed_del {
                    log::debug!(
                        "[multi] Forward delete across {} carets",
                        tabular.multi_selection.len()
                    );
                    tabular
                        .multi_selection
                        .apply_delete_forward(&mut tabular.editor.text);
                } else {
                    log::debug!(
                        "[multi] Backspace across {} carets",
                        tabular.multi_selection.len()
                    );
                    tabular
                        .multi_selection
                        .apply_backspace(&mut tabular.editor.text);
                }

                if let Some((start, caret)) = tabular.multi_selection.primary_range() {
                    tabular.selection_start = start;
                    tabular.selection_end = caret;
                    tabular.cursor_position = caret;
                    tabular.pending_cursor_set = Some(caret);
                } else {
                    let caret = tabular.cursor_position.min(tabular.editor.text.len());
                    tabular.selection_start = caret;
                    tabular.selection_end = caret;
                    tabular.cursor_position = caret;
                    tabular.pending_cursor_set = Some(caret);
                }
                tabular.selected_text.clear();
                tabular.selection_force_clear = true;
                let ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
                crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, ci);
                ui.memory_mut(|m| m.request_focus(id));
                tabular.editor_focus_boost_frames = 10;
                request_scroll_to_cursor = true;
                if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                    let new_owned = tabular.editor.text.clone();
                    tabular.editor.set_text(new_owned.clone());
                    tab.content = new_owned;
                    tab.is_modified = true;
                } else {
                    tabular.editor.mark_text_modified();
                }
                editor_autocomplete::update_autocomplete(tabular);
                ui.ctx().request_repaint();
                handled_multi_delete = true;
                multi_edit_pre_applied = true;
            } else {
                let mut start_b = 0;
                let mut end_b = 0;

                // Try to get selection range from egui state first
                if let Some(rng) =
                    crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id)
                    && rng.start != rng.end
                {
                    start_b = to_byte_index(&tabular.editor.text, rng.start);
                    end_b = to_byte_index(&tabular.editor.text, rng.end);
                }

                // Fallback to stored selection
                if start_b == end_b {
                    start_b = tabular.selection_start;
                    end_b = tabular.selection_end;
                }

                if start_b < end_b && end_b <= tabular.editor.text.len() {
                    let selected_text = &tabular.editor.text[start_b..end_b];
                    log::debug!(
                        "Deleting selection from {} to {}: '{}'",
                        start_b,
                        end_b,
                        selected_text
                    );

                    tabular.editor.apply_single_replace(start_b..end_b, "");
                    tabular.cursor_position = start_b;
                    tabular.selection_start = start_b;
                    tabular.selection_end = start_b;
                    tabular.pending_cursor_set = Some(start_b);
                    tabular.selected_text.clear();
                    // Mark for hard selection clear enforcement next frame
                    tabular.selection_force_clear = true;

                    // Sync egui caret to collapsed at start
                    let ci = to_char_index(&tabular.editor.text, start_b);
                    crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, ci);

                    // CRITICAL: Ensure editor maintains focus and cursor stays active for immediate typing
                    ui.memory_mut(|m| m.request_focus(id));
                    request_scroll_to_cursor = true;

                    // Set focus boost to keep editor focused for several frames
                    tabular.editor_focus_boost_frames = 10;

                    // Mark tab as modified
                    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                        tab.content = tabular.editor.text.clone();
                        tab.is_modified = true;
                    }

                    log::debug!(
                        "Selection deleted successfully, cursor now at {} with focus maintained",
                        start_b
                    );
                    // Log remaining text preview
                    {
                        let s = &tabular.editor.text;
                        let mut end = s.len();
                        for (count, (i, _)) in s.char_indices().enumerate() {
                            if count >= 200 {
                                end = i;
                                break;
                            }
                        }
                        let rem = if end < s.len() {
                            format!("{}… (len={})", s[..end].escape_debug(), s.len())
                        } else {
                            s.escape_debug().to_string()
                        };
                        log::debug!("Remaining text after selection delete: {}", rem);
                    }
                }
            }

            // If we consumed the key, request a repaint so UI reflects the change immediately
            if del_key_consumed && !handled_multi_delete {
                ui.ctx().request_repaint();
                // Double focus request to ensure it sticks
                ui.memory_mut(|m| m.request_focus(id));
            }
        }
    }
    // Special guard: Backspace on completely empty text -> consume and do nothing (avoid odd widget churn)
    // SKIP this handling if Custom View dialog is open
    if !tabular.show_add_view_dialog {
        let id = editor_id;
        let bs_pressed = ui.input(|i| i.key_pressed(egui::Key::Backspace));
        if bs_pressed && tabular.editor.text.is_empty() {
            // Ensure there's no selection
            let selection_exists = if let Some(rng) =
                crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id)
            {
                rng.start != rng.end
            } else {
                tabular.selection_start != tabular.selection_end
            };
            if !selection_exists {
                // Consume the Backspace key event and request a repaint outside the lock
                let mut consumed_bs_empty = false;
                ui.ctx().input_mut(|ri| {
                    let mut kept = Vec::with_capacity(ri.events.len());
                    for ev in ri.events.drain(..) {
                        if !consumed_bs_empty
                            && let egui::Event::Key {
                                key: egui::Key::Backspace,
                                pressed: true,
                                ..
                            } = ev
                        {
                            consumed_bs_empty = true;
                            continue;
                        }
                        kept.push(ev);
                    }
                    ri.events = kept;
                });
                if consumed_bs_empty {
                    log::debug!("Consumed Backspace on empty text (no-op)");
                    ui.ctx().request_repaint();
                }
            }
        }
    }
    // Capture multi-cursor typing/deletion events before TextEdit consumes them
    // SKIP this when Custom View dialog is open
    if !tabular.show_add_view_dialog && tabular.multi_selection.len() > 1 {
        ui.ctx().input_mut(|ri| {
            let mut kept_multi = Vec::with_capacity(ri.events.len());
            for ev in ri.events.drain(..) {
                match ev {
                    egui::Event::Text(text) => {
                        log::debug!("[multi] queue text event '{}'", text.escape_debug());
                        intercepted_multi_texts.push(text);
                    }
                    egui::Event::Paste(text) => {
                        log::debug!("[multi] queue paste event len={}", text.len());
                        intercepted_multi_pastes.push(text);
                    }
                    egui::Event::Key {
                        key: egui::Key::Backspace,
                        pressed: true,
                        modifiers,
                        ..
                    } if !tabular.multi_selection.has_expanded_ranges()
                        && !modifiers.command
                        && !modifiers.ctrl
                        && !modifiers.alt =>
                    {
                        log::debug!("[multi] queue Backspace event");
                        intercept_multi_backspace = true;
                    }
                    egui::Event::Key {
                        key: egui::Key::Delete,
                        pressed: true,
                        modifiers,
                        ..
                    } if !tabular.multi_selection.has_expanded_ranges()
                        && !modifiers.command
                        && !modifiers.ctrl
                        && !modifiers.alt =>
                    {
                        log::debug!("[multi] queue Delete event");
                        intercept_multi_delete = true;
                    }
                    other => kept_multi.push(other),
                }
            }
            ri.events = kept_multi;
        });
    } else {
        intercepted_multi_texts.clear();
        intercepted_multi_pastes.clear();
        intercept_multi_backspace = false;
        intercept_multi_delete = false;
    }

    if copy_requested || cut_requested {
        let text_snapshot = tabular.editor.text.clone();
        let text_len = text_snapshot.len();
        let multi_mode = tabular.multi_selection.len() > 1;
        let mut collected_segments: Vec<String> = Vec::new();
        let mut collected_ranges: Vec<(usize, usize)> = Vec::new();
        let mut multi_cut_has_content = false;

        if multi_mode {
            for region in tabular.multi_selection.regions() {
                let raw_start = region.min().min(text_len);
                let raw_end = region.max().min(text_len);
                if let Some((start, end, segment)) =
                    slice_on_char_boundaries(&text_snapshot, raw_start, raw_end)
                {
                    if cut_requested && start < end {
                        multi_cut_has_content = true;
                    }
                    collected_ranges.push((start, end));
                    collected_segments.push(segment);
                } else {
                    collected_ranges.push((raw_start, raw_start));
                    collected_segments.push(String::new());
                }
            }
        } else {
            let raw_start = tabular.selection_start.min(text_len);
            let raw_end = tabular.selection_end.min(text_len);
            if raw_start < raw_end {
                if let Some((start, end, segment)) =
                    slice_on_char_boundaries(&text_snapshot, raw_start, raw_end)
                {
                    collected_ranges.push((start, end));
                    collected_segments.push(segment);
                }
            } else if !tabular.selected_text.is_empty() {
                collected_segments.push(tabular.selected_text.clone());
            }
        }

        if !collected_segments.is_empty() {
            let clipboard_payload = if collected_segments.len() == 1 {
                collected_segments[0].clone()
            } else {
                collected_segments.join("\n")
            };
            ui.ctx().copy_text(clipboard_payload);
        }

        if multi_mode {
            if collected_segments.is_empty() {
                tabular.clipboard_multi_segments = None;
            } else {
                tabular.clipboard_multi_segments = Some(collected_segments.clone());
            }
            if !cut_requested && collected_segments.len() == tabular.multi_selection.len() {
                tabular.clipboard_multi_regions = Some(tabular.multi_selection.regions().to_vec());
                tabular.clipboard_multi_version = Some(tabular.multi_selection.version());
            } else {
                tabular.clipboard_multi_regions = None;
                tabular.clipboard_multi_version = None;
            }
        } else {
            tabular.clipboard_multi_segments = None;
            tabular.clipboard_multi_regions = None;
            tabular.clipboard_multi_version = None;
        }

        let mut cut_performed = false;
        if cut_requested {
            if multi_mode && multi_cut_has_content {
                tabular
                    .multi_selection
                    .apply_replace_selected(&mut tabular.editor.text, "");
                cut_performed = true;
                multi_edit_pre_applied = true;
            } else if !multi_mode && collected_ranges.len() == 1 {
                let (start, end) = collected_ranges[0];
                if end > start {
                    tabular.editor.apply_single_replace(start..end, "");
                    tabular.cursor_position = start;
                    tabular.selection_start = start;
                    tabular.selection_end = start;
                    tabular.selected_text.clear();
                    cut_performed = true;
                }
            }

            if cut_performed {
                tabular.editor.mark_text_modified();
                if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                    let new_owned = tabular.editor.text.clone();
                    tabular.editor.set_text(new_owned.clone());
                    tab.content = new_owned;
                    tab.is_modified = true;
                }

                let id = editor_id;
                if multi_mode {
                    if let Some((start, caret)) = tabular.multi_selection.primary_range() {
                        tabular.selection_start = start;
                        tabular.selection_end = caret;
                        tabular.cursor_position = caret;
                    } else {
                        let caret = tabular.cursor_position.min(tabular.editor.text.len());
                        tabular.selection_start = caret;
                        tabular.selection_end = caret;
                        tabular.cursor_position = caret;
                    }
                } else {
                    let caret = tabular.cursor_position.min(tabular.editor.text.len());
                    tabular.selection_start = caret;
                    tabular.selection_end = caret;
                }
                tabular.selected_text.clear();
                let caret_ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
                crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, caret_ci);
                tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(6);
                ui.memory_mut(|m| m.request_focus(id));
                ui.ctx().request_repaint();
            }
        }
    }
    // Apply queued multi-cursor edits immediately so TextEdit reflects the final state this frame
    if tabular.multi_selection.len() > 1
        && (!intercepted_multi_texts.is_empty()
            || !intercepted_multi_pastes.is_empty()
            || intercept_multi_backspace
            || intercept_multi_delete)
    {
        let mut multi_applied_in_frame = false;
        for text in intercepted_multi_texts.drain(..) {
            if tabular.multi_selection.has_expanded_ranges() {
                tabular
                    .multi_selection
                    .apply_replace_selected(&mut tabular.editor.text, &text);
            } else {
                tabular
                    .multi_selection
                    .apply_insert_text(&mut tabular.editor.text, &text);
            }
            log::debug!(
                "[multi] applied text '{}' across {} carets",
                text.escape_debug(),
                tabular.multi_selection.len()
            );
            multi_applied_in_frame = true;
        }
        for text in intercepted_multi_pastes.drain(..) {
            let mut handled_segmented_paste = false;
            if tabular.multi_selection.len() > 1
                && let Some(segments) = tabular.clipboard_multi_segments.as_ref()
                && segments.len() == tabular.multi_selection.len()
            {
                let expected: Cow<'_, str> = if segments.len() == 1 {
                    Cow::Borrowed(segments[0].as_str())
                } else {
                    Cow::Owned(segments.join("\n"))
                };
                if expected.as_ref() == text {
                    let had_expanded = tabular.multi_selection.has_expanded_ranges();
                    if had_expanded {
                        tabular
                            .multi_selection
                            .apply_replace_segments(&mut tabular.editor.text, segments);
                    } else {
                        tabular
                            .multi_selection
                            .apply_insert_segments(&mut tabular.editor.text, segments);
                    }
                    log::debug!(
                        "[multi] applied segmented paste segments={} has_expanded={}",
                        segments.len(),
                        had_expanded
                    );
                    multi_applied_in_frame = true;
                    handled_segmented_paste = true;
                }
            }

            if handled_segmented_paste {
                continue;
            }

            if tabular.multi_selection.has_expanded_ranges() {
                tabular
                    .multi_selection
                    .apply_replace_selected(&mut tabular.editor.text, &text);
            } else {
                tabular
                    .multi_selection
                    .apply_insert_text(&mut tabular.editor.text, &text);
            }
            log::debug!(
                "[multi] applied paste len={} across {} carets (uniform)",
                text.len(),
                tabular.multi_selection.len()
            );
            multi_applied_in_frame = true;
        }
        if intercept_multi_backspace {
            if tabular.multi_selection.has_expanded_ranges() {
                tabular
                    .multi_selection
                    .apply_replace_selected(&mut tabular.editor.text, "");
            } else {
                tabular
                    .multi_selection
                    .apply_backspace(&mut tabular.editor.text);
            }
            multi_applied_in_frame = true;
        }
        if intercept_multi_delete {
            if tabular.multi_selection.has_expanded_ranges() {
                tabular
                    .multi_selection
                    .apply_replace_selected(&mut tabular.editor.text, "");
            } else {
                tabular
                    .multi_selection
                    .apply_delete_forward(&mut tabular.editor.text);
            }
            multi_applied_in_frame = true;
        }

        if multi_applied_in_frame {
            multi_edit_pre_applied = true;
            if let Some((start, caret)) = tabular.multi_selection.primary_range() {
                tabular.selection_start = start;
                tabular.selection_end = caret;
                tabular.cursor_position = caret;
            } else {
                let caret = tabular.cursor_position.min(tabular.editor.text.len());
                tabular.selection_start = caret;
                tabular.selection_end = caret;
                tabular.cursor_position = caret;
            }
            tabular.selected_text.clear();
            let id = editor_id;
            let ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
            crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, ci);
            tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(6);
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                let new_owned = tabular.editor.text.clone();
                tabular.editor.set_text(new_owned.clone());
                tab.content = new_owned;
                tab.is_modified = true;
            } else {
                tabular.editor.mark_text_modified();
            }
            editor_autocomplete::update_autocomplete(tabular);
            ui.ctx().request_repaint();
        }
    }
    // Forward Delete (no selection): delete the next grapheme to the right of the caret
    // On macOS laptops, this is typically triggered via Fn+Delete and should map to egui::Key::Delete
    {
        let id = editor_id;
        let del_pressed_no_sel = ui.input(|i| i.key_pressed(egui::Key::Delete));
        if del_pressed_no_sel {
            // Determine if there's an active selection via egui state first, otherwise via stored state
            let mut has_selection = false;
            if let Some(rng) =
                crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id)
            {
                has_selection = rng.start != rng.end;
            } else if tabular.selection_start != tabular.selection_end {
                has_selection = true;
            }
            if !has_selection {
                // No selection: perform forward-delete of the next grapheme cluster
                let pos_b = tabular.cursor_position.min(tabular.editor.text.len());
                if pos_b < tabular.editor.text.len() {
                    use unicode_segmentation::UnicodeSegmentation;
                    let tail = &tabular.editor.text[pos_b..];
                    if let Some((_, first_gr)) = tail.grapheme_indices(true).next() {
                        let end_b = pos_b + first_gr.len();
                        let deleted_dbg = &tabular.editor.text[pos_b..end_b];
                        log::debug!(
                            "Forward Delete (no selection): removing '{}' at [{}..{}]",
                            deleted_dbg.escape_debug(),
                            pos_b,
                            end_b
                        );
                        tabular.editor.apply_single_replace(pos_b..end_b, "");
                        // Caret stays at pos_b
                        tabular.cursor_position = pos_b;
                        tabular.selection_start = pos_b;
                        tabular.selection_end = pos_b;
                        tabular.selected_text.clear();

                        // Sync egui caret to collapsed at pos_b (convert byte -> char)
                        let ci = {
                            let s = &tabular.editor.text;
                            let b = pos_b.min(s.len());
                            s[..b].chars().count()
                        };
                        crate::editor_state_adapter::EditorStateAdapter::set_single(
                            ui.ctx(),
                            id,
                            ci,
                        );

                        // Keep focus and mark modified
                        ui.memory_mut(|m| m.request_focus(id));
                        tabular.editor_focus_boost_frames = 6;
                        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                            tab.content = tabular.editor.text.clone();
                            tab.is_modified = true;
                        } else {
                            tabular.editor.mark_text_modified();
                        }

                        // Consume the Delete key event so TextEdit doesn't also process it
                        let mut consumed_delete_event = false;
                        ui.ctx().input_mut(|ri| {
                            let before = ri.events.len();
                            let mut kept = Vec::with_capacity(before);
                            for ev in ri.events.drain(..) {
                                if !consumed_delete_event
                                    && let egui::Event::Key {
                                        key: egui::Key::Delete,
                                        pressed: true,
                                        ..
                                    } = ev
                                {
                                    consumed_delete_event = true;
                                    continue;
                                }
                                kept.push(ev);
                            }
                            ri.events = kept;
                        });
                        if consumed_delete_event {
                            ui.ctx().request_repaint();
                        }
                        // Log remaining text preview
                        {
                            let s = &tabular.editor.text;
                            let mut end = s.len();
                            for (count, (i, _)) in s.char_indices().enumerate() {
                                if count >= 200 {
                                    end = i;
                                    break;
                                }
                            }
                            let rem = if end < s.len() {
                                format!("{}… (len={})", s[..end].escape_debug(), s.len())
                            } else {
                                s.escape_debug().to_string()
                            };
                            log::debug!("Remaining text after forward delete: {}", rem);
                        }
                    }
                } else if pos_b == tabular.editor.text.len() && pos_b > 0 {
                    // At end-of-text: treat Delete as Backspace (delete previous grapheme to the left)
                    use unicode_segmentation::UnicodeSegmentation;
                    let head = &tabular.editor.text[..pos_b];
                    // Find previous grapheme boundary by scanning the last grapheme in head
                    if let Some((start_off, prev_gr)) = head.grapheme_indices(true).next_back() {
                        let start_b = start_off;
                        let end_b = pos_b;
                        log::debug!(
                            "Delete at end -> backspace: removing '{}' at [{}..{}]",
                            prev_gr.escape_debug(),
                            start_b,
                            end_b
                        );
                        tabular.editor.apply_single_replace(start_b..end_b, "");
                        // Move caret left to start_b
                        tabular.cursor_position = start_b;
                        tabular.selection_start = start_b;
                        tabular.selection_end = start_b;
                        tabular.selected_text.clear();

                        // Sync egui caret to start_b
                        let ci = {
                            let s = &tabular.editor.text;
                            let b = start_b.min(s.len());
                            s[..b].chars().count()
                        };
                        crate::editor_state_adapter::EditorStateAdapter::set_single(
                            ui.ctx(),
                            id,
                            ci,
                        );
                        ui.memory_mut(|m| m.request_focus(id));
                        tabular.editor_focus_boost_frames = 6;
                        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                            tab.content = tabular.editor.text.clone();
                            tab.is_modified = true;
                        } else {
                            tabular.editor.mark_text_modified();
                        }
                        // Consume the Delete event
                        let mut consumed_delete_event = false;
                        ui.ctx().input_mut(|ri| {
                            let mut kept = Vec::with_capacity(ri.events.len());
                            for ev in ri.events.drain(..) {
                                if !consumed_delete_event
                                    && let egui::Event::Key {
                                        key: egui::Key::Delete,
                                        pressed: true,
                                        ..
                                    } = ev
                                {
                                    consumed_delete_event = true;
                                    continue;
                                }
                                kept.push(ev);
                            }
                            ri.events = kept;
                        });
                        if consumed_delete_event {
                            ui.ctx().request_repaint();
                        }
                        // Log remaining text preview
                        {
                            let s = &tabular.editor.text;
                            let mut end = s.len();
                            for (count, (i, _)) in s.char_indices().enumerate() {
                                if count >= 200 {
                                    end = i;
                                    break;
                                }
                            }
                            let rem = if end < s.len() {
                                format!("{}… (len={})", s[..end].escape_debug(), s.len())
                            } else {
                                s.escape_debug().to_string()
                            };
                            log::debug!("Remaining text after delete-at-end/backspace: {}", rem);
                        }
                    }
                }
            }
        }
    }
    // Note: Removed the confusing "Backspace at start -> forward delete" behavior
    // Now Backspace at start of text does nothing (standard behavior)
    // If users want to delete forward, they should use the Delete key (Fn+Delete on Mac)

    // [DISABLED] Special-case: Backspace at start-of-text with no selection -> perform forward delete of next grapheme
    // This entire block has been commented out because it was confusing - backspace at start should do nothing
    /*
    {
        let id = editor_id;
        let bs_pressed = ui.input(|i| i.key_pressed(egui::Key::Backspace));
        if bs_pressed {
            // Determine selection
            let mut has_selection = false;
            let mut caret_b = tabular.cursor_position.min(tabular.editor.text.len());
            if let Some(rng) = crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id) {
                has_selection = rng.start != rng.end;
                // Prefer caret from egui state if available
                let to_b = |s: &str, ci: usize| -> usize {
                    match s.char_indices().nth(ci) { Some((b,_)) => b, None => s.len() }
                };
                caret_b = to_b(&tabular.editor.text, rng.primary).min(tabular.editor.text.len());
            } else if tabular.selection_start != tabular.selection_end {
                has_selection = true;
            }
            // The confusing forward delete behavior was here
        }
    }
    */

    // Helper: find line start and end byte indices for a given cursor
    let line_bounds = |s: &str, pos: usize| -> (usize, usize, usize) {
        let bytes = s.as_bytes();
        let mut start = pos.min(bytes.len());
        while start > 0 && bytes[start - 1] != b'\n' {
            start -= 1;
        }
        let mut end = pos.min(bytes.len());
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        // compute line number (slow but fine for pre-op)
        let mut ln = 0usize;
        let mut idx = 0usize;
        while idx < start {
            if bytes[idx] == b'\n' {
                ln += 1;
            }
            idx += 1;
        }
        (start, end, ln)
    };
    // Consume relevant events and set flags
    ui.ctx().input_mut(|ri| {
        let mut kept = Vec::with_capacity(ri.events.len());
        for ev in ri.events.drain(..) {
            match ev {
                egui::Event::Key {
                    key: egui::Key::ArrowLeft,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() > 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        multi_extend_left = true;
                    } else {
                        multi_nav_left = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() > 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        multi_extend_right = true;
                    } else {
                        multi_nav_right = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.alt && !modifiers.shift => {
                    move_line_up = true;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.alt && !modifiers.shift => {
                    move_line_down = true;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() > 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        multi_extend_up = true;
                    } else {
                        multi_nav_up = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() > 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        multi_extend_down = true;
                    } else {
                        multi_nav_down = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Home,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() > 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        multi_nav_home_extend = true;
                    } else {
                        multi_nav_home = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Home,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() <= 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        single_nav_home_extend = true;
                    } else {
                        single_nav_home = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::End,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() > 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        multi_nav_end_extend = true;
                    } else {
                        multi_nav_end = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::End,
                    pressed: true,
                    modifiers,
                    ..
                } if tabular.multi_selection.len() <= 1
                    && !modifiers.alt
                    && !modifiers.ctrl
                    && !modifiers.command =>
                {
                    if modifiers.shift {
                        single_nav_end_extend = true;
                    } else {
                        single_nav_end = true;
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.alt && modifiers.shift => {
                    dup_line_up = true;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.alt && modifiers.shift => {
                    dup_line_down = true;
                }
                other => kept.push(other),
            }
        }
        ri.events = kept;
    });
    // Apply word navigation immediately by updating egui TextEditState before widget is built

    if single_nav_home || single_nav_end || single_nav_home_extend || single_nav_end_extend {
        let id = editor_id;
        let text = &tabular.editor.text;
        let len = text.len();
        let range_opt = crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id);
        let (start_char, end_char, primary_char) = if let Some(r) = range_opt {
            (r.start, r.end, r.primary)
        } else {
            let caret_b = tabular.cursor_position.min(len);
            let caret_ci = to_char_index(text, caret_b);
            (caret_ci, caret_ci, caret_ci)
        };
        let caret_b = to_byte_index(text, primary_char);
        let anchor_char = if start_char == end_char || primary_char == end_char {
            start_char
        } else {
            end_char
        };
        let anchor_b = to_byte_index(text, anchor_char).min(len);
        let (line_start, line_end, _) = line_bounds(text, caret_b);
        let target_b = if single_nav_home || single_nav_home_extend {
            line_start
        } else {
            line_end
        };
        let (new_anchor_b, new_head_b) = if single_nav_home_extend || single_nav_end_extend {
            (anchor_b, target_b.min(len))
        } else {
            let clamped = target_b.min(len);
            (clamped, clamped)
        };
        let new_anchor_b = new_anchor_b.min(len);
        let new_head_b = new_head_b.min(len);
        tabular.cursor_position = new_head_b;
        tabular.selection_start = new_anchor_b.min(new_head_b);
        tabular.selection_end = new_anchor_b.max(new_head_b);
        if tabular.selection_start < tabular.selection_end && tabular.selection_end <= text.len() {
            tabular.selected_text =
                text[tabular.selection_start..tabular.selection_end].to_string();
        } else {
            tabular.selected_text.clear();
        }
        tabular.multi_selection.clear();
        tabular.selection_force_clear = false;
        tabular.pending_cursor_set = None;

        let anchor_ci = to_char_index(text, new_anchor_b);
        let head_ci = to_char_index(text, new_head_b);
        if single_nav_home_extend || single_nav_end_extend {
            crate::editor_state_adapter::EditorStateAdapter::set_selection(
                ui.ctx(),
                id,
                anchor_ci.min(head_ci),
                anchor_ci.max(head_ci),
                head_ci,
            );
        } else {
            crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, head_ci);
        }
        ui.memory_mut(|m| m.request_focus(id));
        ui.ctx().request_repaint();
        tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(6);
        request_scroll_to_cursor = true;
    }
    if multi_nav_left
        || multi_nav_right
        || multi_nav_up
        || multi_nav_down
        || multi_nav_home
        || multi_nav_end
        || multi_nav_home_extend
        || multi_nav_end_extend
        || multi_extend_left
        || multi_extend_right
        || multi_extend_up
        || multi_extend_down
    {
        let id = editor_id;
        if multi_nav_left {
            tabular.multi_selection.move_left(&tabular.editor.text);
        } else if multi_nav_right {
            tabular.multi_selection.move_right(&tabular.editor.text);
        } else if multi_nav_up {
            tabular.multi_selection.move_up(&tabular.editor.text);
        } else if multi_nav_down {
            tabular.multi_selection.move_down(&tabular.editor.text);
        } else if multi_extend_left {
            tabular.multi_selection.extend_left(&tabular.editor.text);
        } else if multi_extend_right {
            tabular.multi_selection.extend_right(&tabular.editor.text);
        } else if multi_extend_up {
            tabular.multi_selection.extend_up(&tabular.editor.text);
        } else if multi_extend_down {
            tabular.multi_selection.extend_down(&tabular.editor.text);
        } else if multi_nav_home_extend {
            tabular
                .multi_selection
                .extend_line_start(&tabular.editor.text);
        } else if multi_nav_end_extend {
            tabular
                .multi_selection
                .extend_line_end(&tabular.editor.text);
        } else if multi_nav_home {
            tabular
                .multi_selection
                .move_line_start(&tabular.editor.text);
        } else if multi_nav_end {
            tabular.multi_selection.move_line_end(&tabular.editor.text);
        }
        let len = tabular.editor.text.len();
        if let Some(region) = tabular.multi_selection.regions().first() {
            let head_b = region.head.min(len);
            let anchor_b = region.anchor.min(len);
            let to_char_index = |s: &str, byte_idx: usize| -> usize {
                let clamp = byte_idx.min(s.len());
                s[..clamp].chars().count()
            };
            crate::editor_state_adapter::EditorStateAdapter::set_single(
                ui.ctx(),
                id,
                to_char_index(&tabular.editor.text, head_b),
            );
            tabular.selection_start = anchor_b.min(head_b);
            tabular.selection_end = anchor_b.max(head_b);
            tabular.cursor_position = head_b;
            if tabular.selection_start < tabular.selection_end
                && tabular.selection_end <= tabular.editor.text.len()
            {
                tabular.selected_text =
                    tabular.editor.text[tabular.selection_start..tabular.selection_end].to_string();
            } else {
                tabular.selected_text.clear();
            }
        }
        ui.memory_mut(|m| m.request_focus(id));
        ui.ctx().request_repaint();
        request_scroll_to_cursor = true;
    }
    // Apply move/duplicate line operations pre-TextEdit (so content shows updated this frame)
    if move_line_up || move_line_down || dup_line_up || dup_line_down {
        let id = editor_id;
        let text = &mut tabular.editor.text;
        let len = text.len();
        let rng = crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id);
        let (sel_start, sel_end) = if let Some(r) = rng {
            (r.start, r.end)
        } else {
            (
                tabular.selection_start.min(len),
                tabular.selection_end.min(len),
            )
        };
        // Expand to whole lines
        let (line_start, _, _) = line_bounds(text, sel_start);
        let (_, mut line_end, _) = line_bounds(text, sel_end.max(sel_start));
        if line_end < len {
            line_end += 1; /* include trailing \n of last line if present */
        }
        // Extract block
        let block = text.get(line_start..line_end).unwrap_or("").to_string();
        // Locate neighbor line bounds
        if move_line_up || dup_line_up {
            if line_start == 0 {
                // Top-most; duplicate above still allowed
                if dup_line_up {
                    tabular
                        .editor
                        .apply_single_replace(line_start..line_start, &block);
                    let new_start = line_start;
                    let new_end = line_end + block.len();
                    let s_ci = to_char_index(&tabular.editor.text, new_start);
                    let e_ci = to_char_index(&tabular.editor.text, new_end);
                    crate::editor_state_adapter::EditorStateAdapter::set_selection(
                        ui.ctx(),
                        id,
                        s_ci,
                        e_ci,
                        e_ci,
                    );
                    tabular.selection_start = new_start;
                    tabular.selection_end = new_end;
                    tabular.cursor_position = new_end;
                }
            } else {
                // Find previous line start
                let prev_start = {
                    let bytes = text.as_bytes();
                    let mut s = line_start - 1; // currently at '\n' or char before current line
                    while s > 0 && bytes[s - 1] != b'\n' {
                        s -= 1;
                    }
                    s
                };
                if move_line_up {
                    // Remove block and insert before previous line
                    // First remove
                    let removed = block.clone();
                    tabular
                        .editor
                        .apply_single_replace(line_start..line_end, "");
                    // Adjust indices after removal
                    let insert_at = prev_start;
                    tabular
                        .editor
                        .apply_single_replace(insert_at..insert_at, &removed);
                    let new_start = insert_at;
                    let new_end = insert_at + removed.len();
                    let s_ci = to_char_index(&tabular.editor.text, new_start);
                    let e_ci = to_char_index(&tabular.editor.text, new_end);
                    crate::editor_state_adapter::EditorStateAdapter::set_selection(
                        ui.ctx(),
                        id,
                        s_ci,
                        e_ci,
                        e_ci,
                    );
                    tabular.selection_start = new_start;
                    tabular.selection_end = new_end;
                    tabular.cursor_position = new_end;
                } else if dup_line_up {
                    let insert_at = prev_start;
                    tabular
                        .editor
                        .apply_single_replace(insert_at..insert_at, &block);
                    let new_start = insert_at;
                    let new_end = insert_at + block.len();
                    let s_ci = to_char_index(&tabular.editor.text, new_start);
                    let e_ci = to_char_index(&tabular.editor.text, new_end);
                    crate::editor_state_adapter::EditorStateAdapter::set_selection(
                        ui.ctx(),
                        id,
                        s_ci,
                        e_ci,
                        e_ci,
                    );
                    tabular.selection_start = new_start;
                    tabular.selection_end = new_end;
                    tabular.cursor_position = new_end;
                }
            }
        } else if move_line_down || dup_line_down {
            // Find next line end start position
            let insert_after = line_end.min(text.len());
            if move_line_down {
                // Remove block, then insert after next line
                let removed = block.clone();
                tabular
                    .editor
                    .apply_single_replace(line_start..line_end, "");
                // After removal, the insertion point shifts left by removed.len()
                let mut after_next = insert_after - removed.len();
                // Move past the next line (find its end)
                let bytes2 = tabular.editor.text.as_bytes();
                let mut s = after_next;
                while s < bytes2.len() && bytes2[s] != b'\n' {
                    s += 1;
                }
                if s < bytes2.len() {
                    s += 1;
                }
                after_next = s;
                tabular
                    .editor
                    .apply_single_replace(after_next..after_next, &removed);
                let new_start = after_next;
                let new_end = after_next + block.len();
                let s_ci = to_char_index(&tabular.editor.text, new_start);
                let e_ci = to_char_index(&tabular.editor.text, new_end);
                crate::editor_state_adapter::EditorStateAdapter::set_selection(
                    ui.ctx(),
                    id,
                    s_ci,
                    e_ci,
                    e_ci,
                );
                tabular.selection_start = new_start;
                tabular.selection_end = new_end;
                tabular.cursor_position = new_end;
            } else if dup_line_down {
                tabular
                    .editor
                    .apply_single_replace(insert_after..insert_after, &block);
                let new_start = insert_after;
                let new_end = insert_after + block.len();
                let s_ci = to_char_index(&tabular.editor.text, new_start);
                let e_ci = to_char_index(&tabular.editor.text, new_end);
                crate::editor_state_adapter::EditorStateAdapter::set_selection(
                    ui.ctx(),
                    id,
                    s_ci,
                    e_ci,
                    e_ci,
                );
                tabular.selection_start = new_start;
                tabular.selection_end = new_end;
                tabular.cursor_position = new_end;
            }
        }
        ui.memory_mut(|m| m.request_focus(id));
        request_scroll_to_cursor = true;
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor.text.clone();
            tab.is_modified = true;
        }
    }
    if raw_tab {
        tab_pressed_pre = true;
        log::debug!("Raw Tab event captured before editor render");
    }
    let accept_via_tab_pre = tab_pressed_pre && tabular.show_autocomplete;
    // Only accept via Enter if popup shown AND acceptance criteria met
    // Only intercept Enter for autocomplete when popup is visible AND there are suggestions
    let accept_via_enter_pre = enter_pressed_pre
        && tabular.show_autocomplete
        && !tabular.autocomplete_suggestions.is_empty();
    if accept_via_tab_pre || accept_via_enter_pre {
        // Remove Tab/Enter pressed events so TextEdit tidak menyisipkan tab/newline
        ui.ctx().input_mut(|ri| {
            let before = ri.events.len();
            ri.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Tab,
                        pressed: true,
                        ..
                    }
                ) && !matches!(
                    e,
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    }
                )
            });
            let removed = before - ri.events.len();
            if removed > 0 {
                log::debug!(
                    "Removed {} key event(s) (Tab/Enter) before autocomplete accept",
                    removed
                );
            }
        });
        // Mark acceptance to be applied after TextEdit render
        defer_accept_autocomplete = true;
    }

    // ----- Build widget after mutations -----
    // Re-enable syntax highlighting with cache
    let lang = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.file_path.as_ref())
        .map(|p| crate::syntax_ts::detect_language_from_name(p))
        .unwrap_or(crate::syntax_ts::LanguageKind::Sql);
    let dark = matches!(
        tabular.advanced_editor.theme,
        models::structs::EditorColorTheme::GithubDark | models::structs::EditorColorTheme::Gruvbox
    );

    // Simple layouter with cached highlighting; honor Word Wrap by adjusting max_width
    let word_wrap = tabular.advanced_editor.word_wrap;
    // Capture a mutable handle to the highlight cache for this frame to avoid recomputing
    let cache = &mut tabular.highlight_cache;
    let mut layouter = move |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let mut job = crate::syntax_ts::highlight_text_cached(text.as_str(), lang, dark, cache);
        job.wrap.max_width = if word_wrap { wrap_width } else { f32::INFINITY };
        ui.fonts(|f| f.layout_job(job))
    };

    // Pre-compute line count — O(1) via cached line_starts, not O(n) text scan
    let pre_line_count = if tabular.advanced_editor.show_line_numbers {
        tabular.editor.line_count().max(1)
    } else {
        0
    };
    // Pre-calc total_lines (used later for dynamic height) — O(1)
    let total_lines_for_layout = tabular.editor.line_count().max(1);
    // Record text length before TextEdit renders (O(1)) — used in response.changed() to detect insertions
    let pre_text_len = tabular.editor.text.len();

    // Calculate gutter width and editor rect
    let gutter_width = if tabular.advanced_editor.show_line_numbers {
        let digits = (pre_line_count as f32).log10().floor() as usize + 1;
        (digits as f32) * 8.0 + 16.0 // approximate monospace char width
    } else {
        0.0
    };

    let avail_rect = ui.available_rect_before_wrap();
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    // Anticipate an extra line when Enter is pressed this frame so the new line is visible immediately
    let total_lines = total_lines_for_layout + if enter_pressed_pre { 1 } else { 0 };
    // Tinggi minimal mengikuti rows (tinggi viewport awal), tinggi maksimal mengikuti jumlah baris.
    // Tambahkan padding extra 3 * line_height agar baris terakhir tidak "nempel" pada panel bawah / handle.
    let min_height = line_height * rows as f32;
    let needed_height = line_height * total_lines as f32 + line_height * 3.0;
    let desired_height = needed_height.max(min_height);
    // Editor rect (tanpa gutter) – biarkan lebar penuh.
    let mut editor_rect = egui::Rect::from_min_size(
        avail_rect.min,
        egui::vec2(avail_rect.width(), desired_height),
    );

    if gutter_width > 0.0 {
        let gutter_rect = egui::Rect::from_min_max(
            editor_rect.min,
            egui::pos2(editor_rect.min.x + gutter_width, editor_rect.max.y),
        );
        editor_rect.min.x += gutter_width;
        ui.data_mut(|d| d.insert_temp::<egui::Rect>(egui::Id::new("gutter_rect"), gutter_rect));
    }

    // Build TextEdit widget directly and capture full output (galley, clip rect, etc.)
    // NOTE: Removed .code_editor() as it may interfere with cursor rendering
    let text_edit = egui::TextEdit::multiline(&mut tabular.editor.text)
        .font(egui::TextStyle::Monospace)
        .desired_rows(rows)
        .desired_width(f32::INFINITY)
        .cursor_at_end(false) // Allow cursor to be positioned anywhere
        .frame(false)
        .id(editor_id)
        .layouter(&mut layouter);

    let egui::InnerResponse {
        inner: text_output, ..
    } = ui.scope_builder(egui::UiBuilder::new().max_rect(editor_rect), |ui| {
        text_edit.show(ui)
    });
    let egui::text_edit::TextEditOutput {
        response,
        galley,
        galley_pos,
        text_clip_rect,
        cursor_range,
        ..
    } = text_output;

    // Fix: Check response immediately after widget interaction
    if response.gained_focus() && tabular.spreadsheet_state.editing_cell.is_some() {
        log::debug!("⎋ Editor gained focus (click) while cell editing -> Cancelling cell edit");
        tabular.spreadsheet_finish_cell_edit(false);
    }

    let cursor_range_after = cursor_range;

    #[cfg(feature = "tree_sitter_sequel")]
    {
        if matches!(lang, crate::syntax_ts::LanguageKind::Sql) {
            tabular.sql_semantic_snapshot = crate::syntax_ts::get_last_sql_snapshot();
        } else {
            tabular.sql_semantic_snapshot = None;
        }
    }
    let did_double_click = response.double_clicked();

    // CRITICAL: Ensure focus and cursor visibility on interaction
    if response.clicked() || response.gained_focus() {
        response.request_focus();
        tabular.editor_focus_boost_frames = 10;
        ui.ctx().request_repaint();
    }

    // Rely on egui's built-in double-click word selection.
    // We purposely do NOT collapse selection on double-click frame (handled via did_double_click checks above),
    // to avoid wiping the freshly formed selection.

    // VSCode-like: highlight logic
    if response.has_focus() {
        let text = &tabular.editor.text;
        let text_len = text.len();
        let cur = tabular.cursor_position.min(text_len);

        // Check if cursor is on an empty line
        let line_start = text[..cur].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = text[cur..].find('\n').map(|i| cur + i).unwrap_or(text_len);
        let is_empty_line = text[line_start..line_end].trim().is_empty();

        if is_empty_line {
            // Single line highlight for empty areas
            let char_idx = text[..cur].chars().count();
            let cursor = CCursor::new(char_idx);
            let layout = galley.layout_from_cursor(cursor);

            if layout.row < galley.rows.len() {
                let placed_row = &galley.rows[layout.row];
                let row_min_y = galley_pos.y + placed_row.min_y();
                let row_max_y = galley_pos.y + placed_row.max_y();
                
                let rect = egui::Rect::from_min_max(
                    egui::pos2(response.rect.left(), row_min_y),
                    egui::pos2(response.rect.right(), row_max_y),
                );
                let col = egui::Color32::from_rgba_unmultiplied(100, 100, 140, 30);
                ui.painter().rect_filled(rect, 0.0, col);
            }
        } else {
            // Block highlight for statements
            // Quick parse to find statement boundaries with robust comment handling
            // Only run if text is reasonably sized to avoid lags on huge files every frame
            let (start_byte, end_byte) = {
               let mut stmt_start = 0;
               let mut found_range = (0, text_len);
               
               let mut chars = text.char_indices().peekable();
               let mut in_quote = None; // None, Some('\''), Some('"'), Some('`')
               let mut in_line_comment = false;
               let mut in_block_comment = false;
               let mut found = false;
               
               while let Some((i, c)) = chars.next() {
                   // 1. Handle String Literals
                   if let Some(q) = in_quote {
                       if c == '\\' {
                           // Skip next char (escape)
                           let _ = chars.next();
                       } else if c == q {
                           in_quote = None;
                       }
                       continue;
                   }

                   // 2. Handle Block Comments
                   if in_block_comment && c == '*' {
                        if let Some(&(_, '/')) = chars.peek() {
                            chars.next(); // consume '/'
                            in_block_comment = false;
                        }
                       continue;
                   }

                   // 3. Handle Line Comments
                   if in_line_comment {
                       if c == '\n' || c == '\r' {
                           in_line_comment = false;
                       }
                       continue;
                   }

                   // 4. Normal Mode
                   match c {
                       '\'' | '"' | '`' => in_quote = Some(c),
                       '-' => {
                           if let Some(&(_, '-')) = chars.peek() {
                               chars.next(); // consume second '-'
                               in_line_comment = true;
                           }
                       }
                       '#' => in_line_comment = true,
                       '/' => {
                           if let Some(&(_, '*')) = chars.peek() {
                               chars.next(); // consume '*'
                               in_block_comment = true;
                           }
                       }
                       ';' => {
                           // Statement ends here
                           let stmt_end = i + 1; 
                           if cur >= stmt_start && cur <= stmt_end {
                               found_range = (stmt_start, stmt_end);
                               found = true;
                               break;
                           }
                           stmt_start = stmt_end;
                       }
                       _ => {}
                   }
               }
               // Handle last statement if cursor is past the last semicolon
               if !found && cur >= stmt_start {
                   found_range = (stmt_start, text_len);
               }
               found_range
            };
            
            let (raw_start, raw_end) = (start_byte, end_byte);
            // Trim leading whitespace so highlight starts at text
            let start_byte = text[raw_start..raw_end]
                .char_indices()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| raw_start + i)
                .unwrap_or(raw_start);
            let end_byte = raw_end;

            // Convert byte range to char indices for the galley
            let start_char_idx = text[..start_byte].chars().count();
            let end_char_idx = text[..end_byte].chars().count();

            let start_cursor = CCursor::new(start_char_idx);
            let end_cursor = CCursor::new(end_char_idx);
            
            let start_layout = galley.layout_from_cursor(start_cursor);
            let end_layout = galley.layout_from_cursor(end_cursor);
            
            // Paint the block from start row to end row
            // We use min/max to be safe, though start should be <= end
            let first_row_idx = start_layout.row.min(galley.rows.len().saturating_sub(1));
            let last_row_idx = end_layout.row.min(galley.rows.len().saturating_sub(1));

            if first_row_idx < galley.rows.len() && last_row_idx < galley.rows.len() {
                 let first_row = &galley.rows[first_row_idx];
                 let last_row = &galley.rows[last_row_idx];
                 
                 let block_top = galley_pos.y + first_row.min_y();
                 let block_bottom = galley_pos.y + last_row.max_y();

                 let rect = egui::Rect::from_min_max(
                     egui::pos2(response.rect.left(), block_top),
                     egui::pos2(response.rect.right(), block_bottom),
                 );
                 
                 let col = egui::Color32::from_rgba_unmultiplied(100, 100, 140, 30);
                 ui.painter().rect_filled(rect, 0.0, col);
            }
        }
    }
    // Apply deferred autocomplete acceptance after TextEdit borrow is released
    if defer_accept_autocomplete {
        crate::editor_autocomplete::accept_current_suggestion(tabular);
        let clamped = tabular.cursor_position.min(tabular.editor.text.len());
        tabular.pending_cursor_set = Some(clamped);
        // Keep focus on editor so Tab/Enter doesn't move focus
        ui.memory_mut(|m| m.request_focus(response.id));
        // Immediately sync caret to the new end position in this frame as well
        let id = response.id;
        let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
        let ci = to_char_index(&tabular.editor.text, clamped);
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        state.store(ui.ctx(), id);
        tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(12);
        ui.ctx().request_repaint();
        request_scroll_to_cursor = true;
    }
    // Multi-cursor: key handling (Cmd+D / Ctrl+D for next occurrence) and Esc to clear
    let input_snapshot = ui.input(|i| i.clone());

    // Guard: treat a single collapsed region at the current caret as no multi-selection.
    // This avoids switching to the custom multi-cursor paint path unnecessarily,
    // which in some cases could make the caret appear to "freeze".
    if tabular.multi_selection.len() == 1
        && let Some((a, b)) = tabular.multi_selection.primary_range()
        && a == b
    {
        let caret_b = tabular.cursor_position.min(tabular.editor.text.len());
        if b == caret_b {
            tabular.multi_selection.clear();
        }
    }

    // Clear multi-selection on Escape
    if input_snapshot.key_pressed(egui::Key::Escape) && !tabular.multi_selection.is_empty() {
        clear_multi_selection_state(tabular, ui, "via Escape");
    }

    // Clear multi-selection when user navigates with arrow keys (without Shift)
    // This gives natural single-cursor behavior when moving around
    let home_pressed = input_snapshot.key_pressed(egui::Key::Home);
    let end_pressed = input_snapshot.key_pressed(egui::Key::End);
    let navigation_clears = (home_pressed && !(multi_nav_home || multi_nav_home_extend))
        || (end_pressed && !(multi_nav_end || multi_nav_end_extend))
        || input_snapshot.key_pressed(egui::Key::PageUp)
        || input_snapshot.key_pressed(egui::Key::PageDown);
    if !tabular.multi_selection.is_empty()
        && !input_snapshot.modifiers.shift
        && !input_snapshot.modifiers.alt // Don't clear on Alt+Arrow (word navigation)
        && navigation_clears
    {
        clear_multi_selection_state(tabular, ui, "due to navigation");
    }

    let cmd_or_ctrl = input_snapshot.modifiers.command || input_snapshot.modifiers.ctrl;
    if cmd_or_ctrl
        && input_snapshot.key_pressed(egui::Key::Z)
        && !tabular.multi_selection.is_empty()
    {
        clear_multi_selection_state(tabular, ui, "due to Undo (Cmd/Ctrl+Z)");
    }
    if cmd_or_ctrl && input_snapshot.key_pressed(egui::Key::D) {
        // CMD+D / CTRL+D: Add next occurrence to multi-selection
        handle_add_next_occurrence(tabular, ui);
    }

    // Alt/Option + Click to add an extra caret (approximate hit-test on monospace grid)
    if input_snapshot.modifiers.alt
        && let Some(pos) = input_snapshot.pointer.interact_pos()
        && input_snapshot.pointer.primary_clicked()
        && response.rect.contains(pos)
    {
        // Compute line/column based on painter metrics
        let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let char_w =
            ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), 'M'));
        let gutter_width = if tabular.advanced_editor.show_line_numbers {
            let total_lines = tabular.editor.line_count().max(1);
            ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), '0'))
                * (total_lines.to_string().len() as f32)
        } else {
            0.0
        };
        let local = pos - response.rect.min;
        let mut line = ((local.y - 6.0) / line_height).floor() as isize;
        let mut col = ((local.x - gutter_width - 6.0) / char_w).floor() as isize;
        if line < 0 {
            line = 0;
        }
        if col < 0 {
            col = 0;
        }
        let line = line as usize;
        let col = col as usize;
        let start = tabular.editor.line_start(line);
        let end = if line + 1 < tabular.editor.line_count() {
            tabular
                .editor
                .line_start(line + 1)
                .min(tabular.editor.text.len())
        } else {
            tabular.editor.text.len()
        };
        let slice = &tabular.editor.text[start..end];
        let mut byte_off = start;
        let mut chars = 0usize;
        for (i, _) in slice.char_indices() {
            if chars >= col {
                break;
            }
            chars += 1;
            byte_off = start + i + 1;
        }
        // If requested column beyond line length, clamp to end
        if chars < col {
            byte_off = end;
        }
        tabular.add_cursor(byte_off);
        ui.ctx().request_repaint();
    }

    // Handle multi-cursor typing - apply changes to all cursors
    // Multi-selection typing compensations handled later in response.changed() branch now.
    if tabular.advanced_editor.show_line_numbers
        && let Some(gutter_rect) =
            ui.data(|d| d.get_temp::<egui::Rect>(egui::Id::new("gutter_rect")))
    {
        let total_lines = tabular.editor.line_count().max(1);
        let editor_height = response.rect.height();
        let painter = ui.painter();
        
        // Use galley to get actual line positions for perfect alignment
        let final_rect = egui::Rect::from_min_size(
            gutter_rect.min,
            egui::vec2(gutter_rect.width(), editor_height),
        );
        painter.rect_filled(final_rect, 0.0, ui.visuals().faint_bg_color);

        // Render line numbers aligned with actual galley rows
        let mut line_num = 1;
        for row in &galley.rows {
            // Use galley_pos to get the actual vertical position of each row
            let y = galley_pos.y + row.rect().min.y;
            
            // Only render if within visible gutter area
            if y >= final_rect.top() && y <= final_rect.bottom() + 20.0 {
                painter.text(
                    egui::pos2(final_rect.right() - 8.0, y + 1.5),
                    egui::Align2::RIGHT_TOP,
                    line_num.to_string(),
                    egui::TextStyle::Monospace.resolve(ui.style()),
                    ui.visuals().weak_text_color(),
                );
            }
            
            // Increment line number after rendering each row that ends with newline
            // This ensures wrapped lines show the same line number
            if row.ends_with_newline {
                line_num += 1;
                // Stop if we've rendered all lines
                if line_num > total_lines {
                    break;
                }
            }
        }
    }

    // Paint extra cursors and selection highlights (after gutter so they appear above text)
    if !tabular.multi_selection.is_empty() {
        let galley = galley.clone();
        let selection_painter = ui.painter().with_clip_rect(text_clip_rect);

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "🎨 Rendering multi-cursor highlights using galley_pos=({}, {})",
                galley_pos.x,
                galley_pos.y
            );
        }

        let to_char_index = |s: &str, byte_idx: usize| -> usize {
            let clamped = byte_idx.min(s.len());
            s[..clamped].chars().count()
        };

        // First pass: Draw selection highlights for all regions
        for (idx, r) in tabular.multi_selection.regions().iter().enumerate() {
            let start_pos = r.min();
            let end_pos = r.max();

            if start_pos < end_pos {
                let start_ci = to_char_index(&tabular.editor.text, start_pos);
                let end_ci = to_char_index(&tabular.editor.text, end_pos);
                let start_cursor = CCursor::new(start_ci);
                let end_cursor = CCursor::new(end_ci);
                let range = CCursorRange::two(start_cursor, end_cursor);
                let [min_cursor, max_cursor] = range.sorted_cursors();
                let min_layout = galley.layout_from_cursor(min_cursor);
                let max_layout = galley.layout_from_cursor(max_cursor);

                let is_primary = idx == 0;
                let highlight_color = if is_primary {
                    egui::Color32::from_rgba_unmultiplied(70, 130, 180, 100)
                } else {
                    egui::Color32::from_rgba_unmultiplied(100, 150, 200, 80)
                };

                if log::log_enabled!(log::Level::Trace) {
                    log::trace!(
                        "   Region [{}]: bytes {}..{} -> rows {}..{} (cols {} -> {})",
                        idx,
                        start_pos,
                        end_pos,
                        min_layout.row,
                        max_layout.row,
                        min_layout.column,
                        max_layout.column
                    );
                }

                for row_idx in min_layout.row..=max_layout.row {
                    let placed_row = &galley.rows[row_idx];
                    let row = &placed_row.row;

                    let left_local = if row_idx == min_layout.row {
                        row.x_offset(min_layout.column)
                    } else {
                        0.0
                    };
                    let right_local = if row_idx == max_layout.row {
                        row.x_offset(max_layout.column)
                    } else {
                        let newline_size = if row.ends_with_newline {
                            row.height() / 2.0
                        } else {
                            0.0
                        };
                        row.size.x + newline_size
                    };

                    let row_top = galley_pos.y + placed_row.min_y();
                    let row_bottom = galley_pos.y + placed_row.max_y();
                    let left = galley_pos.x + placed_row.pos.x + left_local;
                    let right = galley_pos.x + placed_row.pos.x + right_local;

                    let highlight_rect = egui::Rect::from_min_max(
                        egui::pos2(left, row_top),
                        egui::pos2(right, row_bottom),
                    );

                    if highlight_rect.is_positive() {
                        selection_painter.rect_filled(highlight_rect, 2.0, highlight_color);
                    }
                }
            }
        }

        // Second pass: Draw cursors on top of highlights
        for r in tabular.multi_selection.regions() {
            let caret_char_idx = to_char_index(&tabular.editor.text, r.max());
            let caret_cursor = CCursor::new(caret_char_idx);
            let caret_line_rect = galley
                .pos_from_cursor(caret_cursor)
                .translate(galley_pos.to_vec2());

            if caret_line_rect.height() > 0.0 {
                let caret_rect = egui::Rect::from_min_max(
                    egui::pos2(caret_line_rect.left(), caret_line_rect.top()),
                    egui::pos2(caret_line_rect.left() + 2.0, caret_line_rect.bottom()),
                );
                let color = egui::Color32::from_rgba_unmultiplied(100, 150, 255, 220);
                selection_painter.rect_filled(caret_rect, 1.0, color);
            } else {
                // Fallback: draw a caret using line height when galley returns zero height
                let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
                let caret_rect = egui::Rect::from_min_max(
                    egui::pos2(caret_line_rect.left(), caret_line_rect.top()),
                    egui::pos2(
                        caret_line_rect.left() + 2.0,
                        caret_line_rect.top() + line_height,
                    ),
                );
                let color = egui::Color32::from_rgba_unmultiplied(100, 150, 255, 220);
                selection_painter.rect_filled(caret_rect, 1.0, color);
            }
        }
    }

    // ALWAYS paint cursor when no multi-selection - fallback for egui's built-in cursor
    if tabular.multi_selection.is_empty() {
        let has_focus = response.has_focus() || ui.ctx().memory(|m| m.has_focus(response.id));

        // Only paint when editor has focus or during focus boost window
        if has_focus || tabular.editor_focus_boost_frames > 0 {
            let caret_b = tabular.cursor_position.min(tabular.editor.text.len());
            let caret_char_idx = {
                let s = &tabular.editor.text;
                let clamp = caret_b.min(s.len());
                s[..clamp].chars().count()
            };
            let caret_cursor = CCursor::new(caret_char_idx);
            let caret_layout = galley.layout_from_cursor(caret_cursor);

            // Use simple fallback if galley layout fails - paint at top-left as last resort
            if caret_layout.row < galley.rows.len() && !galley.rows.is_empty() {
                let placed_row = &galley.rows[caret_layout.row];
                let row = &placed_row.row;
                let x_offset = row.x_offset(caret_layout.column);
                let caret_x = galley_pos.x + placed_row.pos.x + x_offset;
                let caret_top = galley_pos.y + placed_row.min_y();
                let mut caret_bottom = galley_pos.y + placed_row.max_y();

                // FIX: If galley gives zero height, use text style height as fallback
                if (caret_bottom - caret_top).abs() < 1.0 {
                    let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
                    caret_bottom = caret_top + line_height;
                }

                let caret_width = 2.0;
                let caret_shape = egui::Rect::from_min_max(
                    egui::pos2(caret_x, caret_top),
                    egui::pos2(caret_x + caret_width, caret_bottom),
                );

                // Paint cursor regardless of height (we already fixed it above)
                let painter = ui.painter();
                let color = if ui.visuals().dark_mode {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::BLACK
                };
                painter.rect_filled(caret_shape, 0.0, color);
                if request_scroll_to_cursor {
                    ui.scroll_to_rect(caret_shape, None);
                }
            } else {
                let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
                let caret_rect = egui::Rect::from_min_max(
                    egui::pos2(response.rect.left() + 6.0, response.rect.top() + 6.0),
                    egui::pos2(
                        response.rect.left() + 8.0,
                        response.rect.top() + 6.0 + line_height,
                    ),
                );
                let color = if ui.visuals().dark_mode {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::BLACK
                };
                ui.painter().rect_filled(caret_rect, 0.0, color);
                log::debug!("✏️ Painted FALLBACK cursor at editor top");
            }
        }
    }

    // After show(), apply any pending cursor via direct set_ccursor_range
    // IMPORTANT: Never collapse selection on the same frame as a double-click, to preserve word-select.
    if !did_double_click && let Some(pos) = tabular.pending_cursor_set {
        // Guard: if there's an active selection range in egui state or Shift is held, skip applying
        // a collapsed caret to avoid wiping a freshly created Shift+Click selection.
        let id = response.id;
        let skip_due_to_active_range = if let Some(st) = TextEditState::load(ui.ctx(), id) {
            if let Some(rng) = st.cursor.char_range() {
                // Treat as active range when primary and secondary differ
                rng.primary.index != rng.secondary.index
            } else {
                false
            }
        } else {
            false
        };
        let shift_now = ui.input(|i| i.modifiers.shift);
        if skip_due_to_active_range || shift_now {
            tabular.pending_cursor_set = None;
        }
        let id = response.id;
        let clamped = pos.min(tabular.editor.text.len());
        // Use a collapsed selection to set the caret directly
        let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
        let ci = to_char_index(&tabular.editor.text, clamped);
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
        state.store(ui.ctx(), id);
        // Verify and re-assert if needed in the same frame
        if let Some(s2) = TextEditState::load(ui.ctx(), id)
            && let Some(rng) = s2.cursor.char_range()
            && rng.primary.index != ci
        {
            let mut s3 = s2;
            s3.cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            s3.store(ui.ctx(), id);
        }
        tabular.cursor_position = clamped;
        tabular.pending_cursor_set = None;
        // Enforce for a few frames to fight any late overrides
        tabular.autocomplete_expected_cursor = Some(clamped);
        tabular.autocomplete_protection_frames = tabular.autocomplete_protection_frames.max(8);
        // Keep focus and repaint so the caret moves visually this frame
        ui.memory_mut(|m| m.request_focus(id));
        ui.ctx().request_repaint();
        // Avoid double-setting the cursor in the same frame; the single store above is sufficient
    }
    // Enforce expected caret for a short window after autocomplete accept
    // BUT: never override an active selection range (e.g., from double-click word selection).
    if tabular.autocomplete_protection_frames > 0 {
        let id = response.id;
        let has_active_selection = if let Some(state) = TextEditState::load(ui.ctx(), id) {
            if let Some(rng) = state.cursor.char_range() {
                rng.primary.index != rng.secondary.index
            } else {
                false
            }
        } else {
            false
        };

        if !has_active_selection && let Some(expected) = tabular.autocomplete_expected_cursor {
            // Read current state
            if let Some(state) = TextEditState::load(ui.ctx(), id) {
                if let Some(rng) = state.cursor.char_range() {
                    let current = rng.primary.index;
                    let exp_ci = to_char_index(&tabular.editor.text, expected);
                    if current != exp_ci {
                        let mut st = state;
                        st.cursor
                            .set_char_range(Some(CCursorRange::one(CCursor::new(exp_ci))));
                        st.store(ui.ctx(), id);
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                }
            } else {
                let mut st = TextEditState::default();
                let exp_ci = to_char_index(&tabular.editor.text, expected);
                st.cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(exp_ci))));
                st.store(ui.ctx(), id);
                ui.memory_mut(|m| m.request_focus(id));
            }
        }

        tabular.autocomplete_protection_frames =
            tabular.autocomplete_protection_frames.saturating_sub(1);
        if tabular.autocomplete_protection_frames == 0 {
            tabular.autocomplete_expected_cursor = None;
        }
    }
    // Decrement focus boost window
    if tabular.editor_focus_boost_frames > 0 {
        tabular.editor_focus_boost_frames -= 1;
    }

    // Cleanup stray tab character inside the just-completed identifier (from Tab key) if any
    if accept_via_tab_pre {
        // Cursor currently at end of identifier after injection; scan backwards
        let mut idx = tabular.cursor_position.min(tabular.editor.text.len());
        let bytes = tabular.editor.text.as_bytes();
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
            let token_owned = tabular.editor.text[idx..token_range_end].to_string();
            if token_owned.contains('\t') {
                let cleaned: String = token_owned.chars().filter(|c| *c != '\t').collect();
                if cleaned != token_owned {
                    tabular
                        .editor
                        .text
                        .replace_range(idx..token_range_end, &cleaned);
                    let shift = token_owned.len() - cleaned.len();
                    tabular.cursor_position -= shift;
                    // Adjust egui state cursor (convert byte -> char index)
                    let id = editor_id;
                    let ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
                    crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, ci);
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
    // CRITICAL: Only update cursor/selection if NOT in a text-change frame, because
    // response.changed() already set the correct cursor position from diff calculation.
    // Reading egui state here would overwrite with stale values.
    if !response.changed()
        && let Some(rng) =
            crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), response.id)
    {
        // Convert char indices from egui to byte indices for our buffer
        let primary_b = to_byte_index(&tabular.editor.text, rng.primary);
        let start_b = to_byte_index(&tabular.editor.text, rng.start);
        let end_b = to_byte_index(&tabular.editor.text, rng.end);
        tabular.cursor_position = primary_b;
        tabular.selection_start = start_b;
        tabular.selection_end = end_b;
        if start_b != end_b {
            if let Some(selected) = tabular.editor.text.get(start_b..end_b) {
                tabular.selected_text = selected.to_string();
            } else {
                tabular.selected_text.clear();
            }
        } else {
            tabular.selected_text.clear();
        }
    }

    // Enforce selection collapse visually if requested by a previous destructive action
    if tabular.selection_force_clear {
        let id = response.id;
        let caret_b = tabular.cursor_position.min(tabular.editor.text.len());
        // Convert byte index to char index for egui state
        let to_char_index = |s: &str, byte_idx: usize| -> usize {
            let b = byte_idx.min(s.len());
            s[..b].chars().count()
        };
        let ci = to_char_index(&tabular.editor.text, caret_b);
        crate::editor_state_adapter::EditorStateAdapter::set_single(ui.ctx(), id, ci);
        tabular.selection_start = caret_b;
        tabular.selection_end = caret_b;
        tabular.selected_text.clear();
        tabular.selection_force_clear = false;
        // keep focus and repaint to reflect collapse immediately
        ui.memory_mut(|m| m.request_focus(id));
        ui.ctx().request_repaint();
    }

    // Reset table focus flag when editor is interacted with
    if response.clicked() || response.gained_focus() {
        ui.memory_mut(|m| m.request_focus(response.id));
        tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(10);

        // CRITICAL: Force read cursor position from egui state immediately on click
        if let Some(rng) =
            crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), response.id)
        {
            let primary_b = to_byte_index(&tabular.editor.text, rng.primary);
            tabular.cursor_position = primary_b;
            tabular.selection_start = to_byte_index(&tabular.editor.text, rng.start);
            tabular.selection_end = to_byte_index(&tabular.editor.text, rng.end);
            log::debug!(
                "🖱️ Click detected! Updated cursor_position to {} (char index {})",
                primary_b,
                rng.primary
            );
        }

        // Request repaint to ensure caret appears immediately
        ui.ctx().request_repaint();

        // IMPORTANT: Do not collapse selection on Shift+Click or when a non-collapsed selection exists.
        // Previously, we always set a pending collapsed caret here, which overwrote egui's range
        // selection on the final Shift+Click, making the block selection disappear.
        let _shift_down = ui.input(|i| i.modifiers.shift);
        let _has_range = tabular.selection_start != tabular.selection_end;
        // DO NOT schedule pending collapsed caret on simple click; this caused next-frame overrides
        // that move the caret back to old position (e.g., 0) right after typing the first character.
        // Keep pending_cursor_set clear here. We'll only use it for explicit flows (e.g., autocomplete).
        tabular.pending_cursor_set = None;
    }
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
        let post_text_len = tabular.editor.text.len();
        // Use key event for newline detection \u2014 avoids O(n) text diff entirely
        let just_inserted_newline = enter_pressed_pre;
        let is_insertion = post_text_len > pre_text_len;
        let is_text_changed = post_text_len != pre_text_len;

        // Derive cursor position directly from the widget's report (O(1), no string scan)
        // Also preserve pri_char_idx so cursor sync below doesn't need another O(n) to_char_index call
        let (post_cursor_b_for_diff, post_cursor_ci, post_sel_start_b, post_sel_end_b) = {
            if let Some(range) = cursor_range_after {
                let primary_ci = range.primary.index; // already in char space
                let primary_b = to_byte_index(&tabular.editor.text, primary_ci);
                let [min_cursor, max_cursor] = range.sorted_cursors();
                let start_b = to_byte_index(&tabular.editor.text, min_cursor.index);
                let end_b = to_byte_index(&tabular.editor.text, max_cursor.index);
                (primary_b, primary_ci, start_b, end_b)
            } else {
                // Fallback: keep current cursor position (no diff scan needed)
                let p = tabular.cursor_position.min(tabular.editor.text.len());
                let p_ci = to_char_index(&tabular.editor.text, p);
                (p, p_ci, p, p)
            }
        };
        tabular.cursor_position = post_cursor_b_for_diff.min(tabular.editor.text.len());
        tabular.selection_start = post_sel_start_b.min(tabular.editor.text.len());
        tabular.selection_end = post_sel_end_b.min(tabular.editor.text.len());
        // Sync egui TextEditState cursor — use already-computed char index (O(1)) not to_char_index O(n)
        if !just_inserted_newline && tabular.selection_start == tabular.selection_end {
            let id = response.id;
            let ci = post_cursor_ci; // reuse char index from cursor_range_after, no O(n) scan
            let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            state.store(ui.ctx(), id);
            // CRITICAL: Request focus and repaint immediately to commit cursor state
            // Without this, egui won't commit the cursor change until next frame,
            // causing the next character to insert at the old cursor position!
            ui.memory_mut(|m| m.request_focus(id));
            ui.ctx().request_repaint();
        }

        // Just inserted a newline? Force scroll to the new cursor position.
        if just_inserted_newline {
             inserted_newline_this_frame = true;
             request_scroll_to_cursor = true;
        }
        log::debug!(
            "edit: newline={} insertion={} cursor->{} sel {}..{}",
            just_inserted_newline, is_insertion, post_cursor_b_for_diff,
            post_sel_start_b, post_sel_end_b
        );
        // Apply multi-cursor editing only when there are truly multiple cursors
        // (avoid interfering with normal single-caret Delete/Backspace behavior)
        if !multi_edit_pre_applied {
            let multi_len = tabular.multi_selection.len();
            log::debug!("[multi] response.changed multi_len={} is_insertion={}", multi_len, is_insertion);
            let multi_count = tabular.multi_selection.len();
            if multi_count > 1 {
                let caret_positions_before = tabular.multi_selection.caret_positions();
                let ranges_before = tabular.multi_selection.ranges();
                log::debug!(
                    "[multi] response.changed with {} carets positions={:?} ranges={:?}",
                    multi_count,
                    caret_positions_before,
                    ranges_before
                );
                // Multi-cursor mode is active: apply typing to all cursors
                // Use TextEditState to detect what got inserted (only handles uniform insert across collapsed carets)
                if let Some(rng) = crate::editor_state_adapter::EditorStateAdapter::get_range(
                    ui.ctx(),
                    response.id,
                ) {
                    // Convert char -> byte for comparisons and slicing
                    let new_primary_b = to_byte_index(&tabular.editor.text, rng.primary);
                    let old_primary = tabular.cursor_position;
                    log::debug!(
                        "[multi] primary cursor moved {} -> {} (delta={})",
                        old_primary,
                        new_primary_b,
                        new_primary_b as isize - old_primary as isize
                    );

                    // Detect if this was a typing action (insertion)
                    if new_primary_b > old_primary {
                        if let Some(inserted_slice) =
                            tabular.editor.text.get(old_primary..new_primary_b)
                        {
                            let inserted = inserted_slice.to_string();
                            log::debug!(
                                "[multi] typing insert='{}' caret_count={} positions_before={:?}",
                                inserted.escape_debug(),
                                multi_count,
                                caret_positions_before
                            );

                            // Mirror the widget's primary insertion to other carets only
                            tabular.multi_selection.apply_insert_text_others(
                                &mut tabular.editor.text,
                                &inserted,
                                old_primary,
                            );
                            let caret_positions_after = tabular.multi_selection.caret_positions();
                            log::debug!(
                                "[multi] insert applied text_len={} positions_after={:?}",
                                tabular.editor.text.len(),
                                caret_positions_after
                            );
                            tabular.cursor_position = new_primary_b;
                        } else {
                            log::debug!(
                                "[multi] insert range {}..{} not available in buffer",
                                old_primary,
                                new_primary_b
                            );
                        }
                    }
                    // Detect if this was a backspace action
                    else if new_primary_b < old_primary
                        && old_primary.saturating_sub(new_primary_b) == 1
                    {
                        log::debug!(
                            "[multi] backspace caret_count={} positions_before={:?}",
                            multi_count,
                            caret_positions_before
                        );
                        tabular
                            .multi_selection
                            .apply_backspace(&mut tabular.editor.text);
                        let caret_positions_after = tabular.multi_selection.caret_positions();
                        log::debug!(
                            "[multi] backspace applied text_len={} positions_after={:?}",
                            tabular.editor.text.len(),
                            caret_positions_after
                        );
                        tabular.cursor_position = new_primary_b;
                    } else {
                        log::debug!(
                            "[multi] no multi-caret edit detected old={} new={}",
                            old_primary,
                            new_primary_b
                        );
                    }
                } else {
                    log::debug!("[multi] missing TextEdit range; skipping multi-caret sync");
                }
            } else if tabular.multi_selection.len() == 1 {
                // User is typing with only one cursor active (after CMD+D once)
                // Keep the multi-selection active for potential next CMD+D
                // but update the primary cursor position
                if let Some(rng) = crate::editor_state_adapter::EditorStateAdapter::get_range(
                    ui.ctx(),
                    response.id,
                ) {
                    let new_primary_b = to_byte_index(&tabular.editor.text, rng.primary);
                    tabular.cursor_position = new_primary_b;

                    // Update the selection range in multi_selection
                    let start_b = to_byte_index(&tabular.editor.text, rng.start);
                    let end_b = to_byte_index(&tabular.editor.text, rng.end);
                    tabular.multi_selection.set_primary_range(start_b, end_b);
                }
            } else {
                // No multi-cursor active: normal single-caret behavior
                // Skip multi-cursor compensation when only a single caret is active
                // to avoid misinterpreting normal Delete/Backspace edits.
            }
        } else {
            log::debug!("[multi] response.changed skipped (pre-applied this frame)");
        }

        // Rebuild autocomplete suggestions on text changes unless accepting via Tab/Enter
        // Skip on newline (avoids lag after Enter) and on deletion (avoids stale suggestions)
        if !accept_via_tab_pre && !accept_via_enter_pre {
            if is_insertion && !just_inserted_newline {
                editor_autocomplete::update_autocomplete(tabular);
                request_scroll_to_cursor = true;
            } else if is_text_changed {
                // Hide popup on deletion or newline
                tabular.show_autocomplete = false;
            }
        }

        // Sync buffer state and tab content once (after multi-cursor may have mutated editor.text)
        tabular.editor.mark_text_modified();
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor.text.clone();
            tab.is_modified = true;
        }

        // Scan for new --AI ... -- blocks to process (only when no inline AI request already in flight)
        // TRIGGER: only when user just pressed Enter (completing the closing --)
        if just_inserted_newline && tabular.ai_inline_receiver.is_none() && !tabular.ai_api_key.is_empty()
            && let Some((block_hash, prompt)) = detect_ai_block_closed_by_enter(tabular) {
                let schema_context = crate::ai_assistant::build_schema_context(tabular, 30);
                let system = crate::ai_assistant::sql_system_prompt_with_schema(&schema_context);

                // Insert a loading placeholder at the current cursor position (new empty line after --)
                let placeholder = "-- ✨ AI: Thinking...\n";
                let cursor_pos = tabular.cursor_position.min(tabular.editor.text.len());
                tabular.editor.text.insert_str(cursor_pos, placeholder);
                let placeholder_start = cursor_pos;
                let placeholder_end = cursor_pos + placeholder.len();
                // Advance cursor past the placeholder
                tabular.cursor_position = placeholder_end;
                tabular.selection_start = placeholder_end;
                tabular.selection_end = placeholder_end;
                tabular.editor.mark_text_modified();
                tabular.highlight_cache.clear();
                if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                    tab.content = tabular.editor.text.clone();
                    tab.is_modified = true;
                }

                let rx = crate::ai_assistant::request_ai_suggestion(
                    tabular.ai_provider,
                    tabular.ai_api_key.clone(),
                    tabular.ai_model.clone(),
                    tabular.ai_base_url.clone(),
                    system,
                    prompt,
                );
                tabular.ai_inline_receiver = Some((block_hash, placeholder_start, placeholder_end, rx));
                request_scroll_to_cursor = true;
                ui.ctx().request_repaint();
        }

        // Force a repaint after text changes to ensure visual sync (avoids any lingering glyphs)
        ui.ctx().request_repaint();
        // Keep caret visible for a few frames after typing/Enter
        tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(8);
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
        let cur = tabular.cursor_position.min(tabular.editor.text.len());
        // Use byte check (O(1)) instead of chars().nth() O(n) scan
        if cur > 0 && tabular.editor.text.as_bytes().get(cur - 1) == Some(&b'\t') {
            // Remove the inserted tab via rope edit
            let start = cur - 1;
            tabular.editor.apply_single_replace(start..cur, "");
            tabular.cursor_position = tabular.cursor_position.saturating_sub(1);
            log::debug!("Detected tab character insertion -> triggering autocomplete accept");
            editor_autocomplete::accept_current_suggestion(tabular);
            // Immediately set caret to new position and refocus
            let id = response.id;
            let clamped = tabular.cursor_position.min(tabular.editor.text.len());
            let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
            let ci = to_char_index(&tabular.editor.text, clamped);
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            state.store(ui.ctx(), id);
            ui.memory_mut(|m| m.request_focus(id));
            request_scroll_to_cursor = true;
        } else if cur >= 4 && &tabular.editor.text[cur - 4..cur] == "    " {
            // Remove inserted 4 spaces via rope edit
            let start = cur - 4;
            tabular.editor.apply_single_replace(start..cur, "");
            tabular.cursor_position = tabular.cursor_position.saturating_sub(4);
            log::debug!("Detected 4-space indentation -> triggering autocomplete accept");
            editor_autocomplete::accept_current_suggestion(tabular);
            // Immediately set caret to new position and refocus
            let id = response.id;
            let clamped = tabular.cursor_position.min(tabular.editor.text.len());
            let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
            let ci = to_char_index(&tabular.editor.text, clamped);
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            state.store(ui.ctx(), id);
            ui.memory_mut(|m| m.request_focus(id));
            request_scroll_to_cursor = true;
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
            // Apply same heuristic as pre-render
            let mut should_accept = tabular.autocomplete_navigated;
            if !should_accept {
                let sugg_count = tabular.autocomplete_suggestions.len();
                if sugg_count == 1 {
                    should_accept = true;
                } else {
                    let prefix = tabular.autocomplete_prefix.clone();
                    if let Some(sugg) = tabular
                        .autocomplete_suggestions
                        .get(tabular.selected_autocomplete_index)
                        && !prefix.is_empty()
                    {
                        let p = prefix.to_lowercase();
                        let s = sugg.to_lowercase();
                        if s.starts_with(&p) {
                            should_accept = true;
                        }
                    }
                }
            }

            if should_accept {
                editor_autocomplete::accept_current_suggestion(tabular);
                accepted = true;
            }
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
            if tabular.editor.text.contains('\t') {
                // Remove a lone tab right before cursor via rope edit if exists
                let cur = tabular.cursor_position.min(tabular.editor.text.len());
                if cur > 0 && tabular.editor.text.chars().nth(cur - 1) == Some('\t') {
                    let start = cur - 1;
                    tabular.editor.apply_single_replace(start..cur, "");
                    tabular.cursor_position = tabular.cursor_position.saturating_sub(1);
                }
            }
            // Remove four leading spaces sequence before cursor (indent) if present
            let cur = tabular.cursor_position.min(tabular.editor.text.len());
            if cur >= 4 && &tabular.editor.text[cur - 4..cur] == "    " {
                let start = cur - 4;
                tabular.editor.apply_single_replace(start..cur, "");
                tabular.cursor_position = tabular.cursor_position.saturating_sub(4);
            }
            // Update internal egui state for cursor after Enter accept path
            let id = response.id;
            // Apply collapsed caret directly via set_ccursor_range equivalent
            let mut state = TextEditState::load(ui.ctx(), id).unwrap_or_default();
            let ci = to_char_index(&tabular.editor.text, tabular.cursor_position);
            state
                .cursor
                .set_char_range(Some(CCursorRange::one(CCursor::new(ci))));
            state.store(ui.ctx(), id);
            // Ensure caret stays active after acceptance
            let clamped = tabular.cursor_position.min(tabular.editor.text.len());
            tabular.pending_cursor_set = Some(clamped);
            tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(12);
            // Re-focus editor so Tab doesn't move focus away
            ui.memory_mut(|m| m.request_focus(id));
            ui.ctx().request_repaint();
            request_scroll_to_cursor = true;
        }
        if input.key_pressed(egui::Key::Escape) {
            tabular.show_autocomplete = false;
        }
    }

    // Update suggestions saat kursor bergerak kiri/kanan (tanpa perubahan teks)
    // DISABLED: Arrow key updates cause freeze - just hide popup instead
    let moved_lr =
        input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowRight);
    if moved_lr && !accept_via_tab_pre && !accept_via_enter_pre {
        // Hide autocomplete when navigating with arrows to avoid stale position
        tabular.show_autocomplete = false;
        tabular.suppress_editor_arrow_once = false;
    }

    // Render autocomplete popup positioned under cursor
    if tabular.show_autocomplete && !tabular.autocomplete_suggestions.is_empty() {
        // Use O(log n) binary-search via offset_to_line_col instead of O(n) char scan
        let cursor = tabular.cursor_position.min(tabular.editor.text.len());
        let (line_no, col_bytes) = tabular.editor.offset_to_line_col(cursor);
        let column = col_bytes; // byte offset within line (good enough for monospace approximation)
        let char_w = 8.0_f32; // heuristic monospace width
        let line_h = ui.text_style_height(&egui::TextStyle::Monospace);
        let editor_rect = response.rect; // basic TextEdit rect
        let mut pos = egui::pos2(
            editor_rect.left() + (column as f32) * char_w,
            editor_rect.top() + 4.0 + (line_no as f32) * line_h,
        );
        // Clamp horizontally inside editor area
        if pos.x > editor_rect.right() - 150.0 {
            pos.x = editor_rect.right() - 150.0;
        }
        editor_autocomplete::render_autocomplete(tabular, ui, pos);
    }

    if autocomplete_was_visible_at_start && !tabular.show_autocomplete {
        ui.memory_mut(|m| m.request_focus(response.id));
        tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(8);
        ui.ctx().request_repaint();
    }

    // ── Inline AI block response polling ──────────────────────────────────────
    // Check if an in-flight inline AI request has a response ready and replace the placeholder.
    let inline_result = {
        if let Some((block_hash, placeholder_start, placeholder_end, ref rx)) = tabular.ai_inline_receiver {
            match rx.try_recv() {
                Ok(result) => Some((block_hash, placeholder_start, placeholder_end, result)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.ctx().request_repaint();
                    None
                }
                Err(_) => {
                    Some((block_hash, placeholder_start, placeholder_end, Err("Inline AI channel closed".to_string())))
                }
            }
        } else {
            None
        }
    };
    if let Some((block_hash, placeholder_start, placeholder_end, result)) = inline_result {
        tabular.ai_inline_receiver = None;
        tabular.ai_inline_processed.insert(block_hash);
        let response_text = match result {
            Ok(text) => format_ai_response_as_sql(&text),
            Err(e) => format!("-- AI Error: {}\n", e),
        };
        let start = placeholder_start.min(tabular.editor.text.len());
        let end = placeholder_end.min(tabular.editor.text.len());
        tabular.editor.text.replace_range(start..end, &response_text);
        let new_cursor = start + response_text.len();
        tabular.cursor_position = new_cursor;
        tabular.selection_start = new_cursor;
        tabular.selection_end = new_cursor;
        tabular.editor.mark_text_modified();
        tabular.highlight_cache.clear();
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor.text.clone();
            tab.is_modified = true;
        }
        request_scroll_to_cursor = true;
        ui.ctx().request_repaint();
    }

    // FINAL SCROLL LOGIC: Check if we need to scroll to cursor at the end of the frame
    // This is the most reliable place as all logic (key presses, changes, etc.) has finished.
    // We also compensate for stale galley layout if a newline was just inserted.
    if request_scroll_to_cursor {
        let caret_b = tabular.cursor_position.min(tabular.editor.text.len());
        let caret_char_idx = {
            let s = &tabular.editor.text;
            let clamp = caret_b.min(s.len());
            s[..clamp].chars().count()
        };
        let caret_cursor = CCursor::new(caret_char_idx);
        let caret_line_rect = galley
            .pos_from_cursor(caret_cursor)
            .translate(galley_pos.to_vec2());

        let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let mut caret_rect = egui::Rect::from_min_max(
            egui::pos2(caret_line_rect.left(), caret_line_rect.top()),
            egui::pos2(
                caret_line_rect.left() + 2.0,
                caret_line_rect.top() + line_height,
            ),
        );

        // Compensation: If we just inserted a newline, the galley we have is STALE (doesn't have the new line).
        // The caret position has already advanced to the next line in the text buffer,
        // but the galley visual layout thinks it's still on the old line or somewhere else.
        // We heuristically shift the target rect DOWN by one line height to ensure the scroll view accommodates the new line.
        if inserted_newline_this_frame {
             caret_rect = caret_rect.translate(egui::vec2(0.0, line_height));
             log::debug!("↵ Enter pressed: Shifting scroll target down by {}px to compensate for layout lag", line_height);
        }

        // Using Align::Center usually gives better context than Bottom/Top which might auto-shrink weirdly
        ui.scroll_to_rect(caret_rect, None);
        log::debug!("📜 Requesting scroll to {:?} (newline={})", caret_rect, inserted_newline_this_frame);
    }
}

// ─── Inline --AI...-- block helpers ──────────────────────────────────────────

/// Detect if Enter was just pressed after typing the closing `--` line inside a `--AI ... --` block.
/// Cursor is now at the beginning of the freshly created empty line.
/// Returns `(block_hash, prompt_text)` when a valid, unprocessed block is found.
fn detect_ai_block_closed_by_enter(tabular: &window_egui::Tabular) -> Option<(u64, String)> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let text = &tabular.editor.text;
    let cursor = tabular.cursor_position.min(text.len());

    // cursor is at the start of a new empty line after Enter.
    // The line we just left is at [prev_line_start .. cursor-1).
    if cursor == 0 {
        return None;
    }
    let prev_nl = cursor - 1; // byte index of the '\n' we just inserted
    // Find the line before that '\n'
    let prev_line_end = prev_nl;
    let prev_line_start = text[..prev_line_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let prev_line = &text[prev_line_start..prev_line_end];

    // The closing marker must be exactly "--"
    if prev_line.trim() != "--" {
        return None;
    }

    // Walk backwards through lines to collect prompt and find opening --AI
    if prev_line_start == 0 {
        return None;
    }
    let mut search_end = prev_line_start - 1; // byte just before the '\n' of prev_line
    let mut prompt_lines: Vec<&str> = Vec::new();
    let block_close_start = prev_line_start; // byte start of the "--" line

    loop {
        let line_end = search_end;
        let line_start = text[..line_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line = &text[line_start..line_end];
        let trimmed = line.trim();

        if trimmed == "--AI" || trimmed.starts_with("--AI ") || trimmed.starts_with("--AI\t") {
            // Found the opening marker
            if prompt_lines.is_empty() {
                return None; // nothing between the markers
            }
            prompt_lines.reverse();
            let prompt = prompt_lines.join("\n");

            // Hash the block so we can track it and avoid re-sending
            let block_text = &text[line_start..block_close_start];
            let mut hasher = DefaultHasher::new();
            block_text.hash(&mut hasher);
            let block_hash = hasher.finish();

            if tabular.ai_inline_processed.contains(&block_hash) {
                return None;
            }

            return Some((block_hash, prompt));
        }

        // Stop on nested / extra markers to avoid runaway scanning
        if trimmed == "--" || trimmed.starts_with("--AI") {
            break;
        }

        prompt_lines.push(line);

        if line_start == 0 {
            break;
        }
        search_end = line_start - 1;
    }

    None
}

/// Format an AI response for inline insertion into a SQL editor.
/// - Lines inside ```sql / ``` fences are inserted as plain SQL.
/// - Lines outside fences (explanatory text) are prefixed with `-- ` to make them SQL comments.
/// - A trailing newline is always appended.
fn format_ai_response_as_sql(text: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;

    for line in text.lines() {
        let trimmed = line.trim();

        // Opening fence
        if !in_code_block && (trimmed.starts_with("```sql") || trimmed == "```") {
            in_code_block = true;
            continue; // skip the fence line itself
        }
        // Closing fence
        if in_code_block && trimmed == "```" {
            in_code_block = false;
            continue;
        }

        if in_code_block {
            // Raw SQL — keep indentation as-is
            out.push_str(line);
            out.push('\n');
        } else if trimmed.is_empty() {
            // Blank line outside code — emit one empty line (collapse multiple blanks)
            if !out.ends_with("\n\n") && !out.is_empty() {
                out.push('\n');
            }
        } else {
            // Explanatory text — convert to SQL comment; strip markdown backticks
            let clean = trimmed.replace('`', "");
            out.push_str("-- ");
            out.push_str(&clean);
            out.push('\n');
        }
    }

    // Ensure exactly one trailing newline
    let body = out.trim_end_matches('\n');
    format!("{}\n", body)
}

// ─── AI Assistant Panel ───────────────────────────────────────────────────────

pub(crate) fn render_ai_panel(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    // Poll for pending AI response
    if tabular.ai_is_loading
        && let Some(rx) = &tabular.ai_suggestion_receiver {
            if let Ok(result) = rx.try_recv() {
                tabular.ai_is_loading = false;
                tabular.ai_suggestion_receiver = None;
                match result {
                    Ok(text) => {
                        tabular.ai_suggestion = text;
                        tabular.ai_error = None;
                    }
                    Err(e) => {
                        tabular.ai_error = Some(e);
                    }
                }
                ui.ctx().request_repaint();
            } else {
                // Still loading — keep repainting so spinner animates
                ui.ctx().request_repaint();
            }
    }

    let no_api_key = tabular.ai_api_key.is_empty();
    let accent = egui::Color32::from_rgb(99, 135, 255);
    let panel_bg = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(28, 30, 40)
    } else {
        egui::Color32::from_rgb(242, 244, 255)
    };

    egui::Frame::new()
        .fill(panel_bg)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(if ui.visuals().dark_mode { 55 } else { 200 })))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            // Header row
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("✨ AI Assistant")
                        .strong()
                        .color(accent)
                        .size(13.0),
                );
                let provider_label = tabular.ai_provider.display_name();
                ui.label(
                    egui::RichText::new(format!("({provider_label})"))
                        .size(11.0)
                        .color(egui::Color32::from_gray(140)),
                );
                // Schema context indicator
                let schema_preview = crate::ai_assistant::build_schema_context(tabular, 30);
                if schema_preview.is_empty() {
                    ui.label(
                        egui::RichText::new("⚠ no schema")
                            .size(10.0)
                            .color(egui::Color32::from_rgb(180, 140, 40)),
                    ).on_hover_text("No table schema found in cache. Browse a table first to populate the schema cache.");
                } else {
                    let table_count = schema_preview.lines()
                        .filter(|l| l.starts_with("CREATE TABLE") || l.starts_with("-- Table:"))
                        .count();
                    ui.label(
                        egui::RichText::new(format!("🗄 {table_count} tables"))
                            .size(10.0)
                            .color(egui::Color32::from_rgb(80, 200, 120)),
                    ).on_hover_text(format!("Schema context will be sent with every prompt:\n\n{}", &schema_preview.chars().take(600).collect::<String>()));
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text("Close panel (Cmd+Shift+A)").clicked() {
                        tabular.show_ai_panel = false;
                    }
                    if ui.small_button("⚙").on_hover_text("Open AI settings").clicked() {
                        tabular.show_settings_window = true;
                        tabular.settings_active_pref_tab = crate::window_egui::PrefTab::AiAssistant;
                    }
                });
            });

            if no_api_key {
                ui.label(
                    egui::RichText::new("⚠ No API key configured. Open Settings → AI Assistant to add one.")
                        .color(egui::Color32::from_rgb(220, 160, 30))
                        .size(12.0),
                );
                return;
            }

            ui.add_space(4.0);

            // Prompt input
            ui.label(egui::RichText::new("Prompt:").size(12.0));
            let input_resp = ui.add(
                egui::TextEdit::multiline(&mut tabular.ai_input)
                    .desired_rows(3)
                    .hint_text("Ask something about SQL, databases, or your current query…")
                    .font(egui::TextStyle::Body)
                    .desired_width(f32::INFINITY),
            );

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let can_send = !tabular.ai_input.trim().is_empty() && !tabular.ai_is_loading;
                let send_btn = ui.add_enabled(
                    can_send,
                    egui::Button::new(egui::RichText::new("Send ↵").color(egui::Color32::WHITE))
                        .fill(if can_send { accent } else { egui::Color32::from_gray(100) }),
                );
                let send_via_enter = input_resp.has_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);

                if (send_btn.clicked() || send_via_enter) && can_send {
                    let context_sql = if tabular.selection_start < tabular.selection_end
                        && tabular.selection_end <= tabular.editor.text.len()
                    {
                        tabular.editor.text[tabular.selection_start..tabular.selection_end]
                            .to_string()
                    } else {
                        // Provide up to 2000 chars of editor context if no selection
                        let t = &tabular.editor.text;
                        if t.len() > 2000 {
                            t[..2000].to_string()
                        } else {
                            t.clone()
                        }
                    };

                    let schema_context = crate::ai_assistant::build_schema_context(tabular, 30);
                    let system = crate::ai_assistant::sql_system_prompt_with_schema(&schema_context);
                    let user = if context_sql.is_empty() {
                        tabular.ai_input.clone()
                    } else {
                        format!(
                            "Current SQL context:\n```sql\n{context_sql}\n```\n\n{}",
                            tabular.ai_input
                        )
                    };

                    let rx = crate::ai_assistant::request_ai_suggestion(
                        tabular.ai_provider,
                        tabular.ai_api_key.clone(),
                        tabular.ai_model.clone(),
                        tabular.ai_base_url.clone(),
                        system,
                        user,
                    );
                    tabular.ai_suggestion_receiver = Some(rx);
                    tabular.ai_is_loading = true;
                    tabular.ai_suggestion.clear();
                    tabular.ai_error = None;
                    ui.ctx().request_repaint();
                }

                if tabular.ai_is_loading {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Thinking…")
                            .size(11.0)
                            .color(egui::Color32::from_gray(160)),
                    );
                }

                if (!tabular.ai_suggestion.is_empty() || tabular.ai_error.is_some())
                    && ui.small_button("🗑 Clear").clicked() {
                        tabular.ai_suggestion.clear();
                        tabular.ai_error = None;
                }
            });

            // Error display
            if let Some(ref err) = tabular.ai_error.clone() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("Error: {err}"))
                        .color(egui::Color32::from_rgb(255, 80, 80))
                        .size(12.0),
                );
            }

            // Response display
            if !tabular.ai_suggestion.is_empty() {
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Response:").size(12.0).strong());

                egui::ScrollArea::vertical()
                    .id_salt("ai_response_scroll")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut tabular.ai_suggestion.clone())
                                .desired_width(f32::INFINITY)
                                .font(egui::TextStyle::Monospace)
                                .interactive(false),
                        );
                    });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("📋 Copy").clicked() {
                        ui.ctx().copy_text(tabular.ai_suggestion.clone());
                    }
                    if ui.button("⬆ Insert at cursor").clicked() {
                        let insert_text = tabular.ai_suggestion.clone();
                        let pos = tabular.cursor_position.min(tabular.editor.text.len());
                        tabular.editor.text.insert_str(pos, &insert_text);
                        let new_cursor = pos + insert_text.len();
                        tabular.cursor_position = new_cursor;
                        tabular.selection_start = new_cursor;
                        tabular.selection_end = new_cursor;
                        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                            tab.content = tabular.editor.text.clone();
                            tab.is_modified = true;
                        }
                        ui.ctx().request_repaint();
                    }
                    if ui.button("📝 Replace selection").on_hover_text(
                        "Replace the currently selected text with this response"
                    ).clicked() && tabular.selection_start < tabular.selection_end {
                        let insert_text = tabular.ai_suggestion.clone();
                        let s = tabular.selection_start;
                        let e = tabular.selection_end.min(tabular.editor.text.len());
                        tabular.editor.text.replace_range(s..e, &insert_text);
                        let new_cursor = s + insert_text.len();
                        tabular.cursor_position = new_cursor;
                        tabular.selection_start = new_cursor;
                        tabular.selection_end = new_cursor;
                        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                            tab.content = tabular.editor.text.clone();
                            tab.is_modified = true;
                        }
                        ui.ctx().request_repaint();
                    }
                });
            }
        });

    ui.add_space(4.0);
}


/// Preserves caret and selection where possible.
pub(crate) fn reformat_current_sql(tabular: &mut window_egui::Tabular, ui: &egui::Ui) {
    let id = ui.make_persistent_id("sql_editor");
    // Helper: convert char idx -> byte idx
    let to_b = |s: &str, ci: usize| -> usize {
        match s.char_indices().nth(ci) {
            Some((b, _)) => b,
            None => s.len(),
        }
    };
    // Try to read selection from egui state first (in chars), then map to bytes
    let text_len = tabular.editor.text.len();
    let (start_b, end_b) = if let Some(rng) =
        crate::editor_state_adapter::EditorStateAdapter::get_range(ui.ctx(), id)
    {
        let s_b = to_b(&tabular.editor.text, rng.start).min(text_len);
        let e_b = to_b(&tabular.editor.text, rng.end).min(text_len);
        (s_b.min(e_b), s_b.max(e_b))
    } else {
        // Fallback to stored selection (bytes)
        let s = tabular.selection_start.min(text_len);
        let e = tabular.selection_end.min(text_len);
        (s.min(e), s.max(e))
    };

    let (range_start, range_end) = if start_b < end_b {
        (start_b, end_b)
    } else {
        (0, text_len)
    };
    let original = &tabular.editor.text[range_start..range_end];
    // Apply sqlformat with sane defaults: 4-space indent, uppercase keywords, 1 line between queries
    let opts = crate::query_tools::default_sqlformat_options();
    let formatted = sqlfmt(original, &QueryParams::None, &opts);
    if formatted == original {
        return; // no change
    }

    // Replace in editor text using rope-friendly method
    tabular
        .editor
        .apply_single_replace(range_start..range_end, &formatted);

    // Update caret and selection: select the newly formatted block
    let new_end = range_start + formatted.len();
    tabular.selection_start = range_start;
    tabular.selection_end = new_end;
    tabular.cursor_position = new_end;

    // Sync egui selection/caret using char indices
    let to_ci = |s: &str, bi: usize| -> usize { s[..bi.min(s.len())].chars().count() };
    let start_ci = to_ci(&tabular.editor.text, range_start);
    let end_ci = to_ci(&tabular.editor.text, new_end);
    crate::editor_state_adapter::EditorStateAdapter::set_selection(
        ui.ctx(),
        id,
        start_ci,
        end_ci,
        end_ci,
    );
    ui.memory_mut(|m| m.request_focus(id));

    // Mark tab modified and keep content synced
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = tabular.editor.text.clone();
        tab.is_modified = true;
    } else {
        tabular.editor.mark_text_modified();
    }

    // Recompute autocomplete, lint etc. if needed
    editor_autocomplete::update_autocomplete(tabular);
}

/// Toggle line comment (CMD/CTRL + /) for SQL queries
/// Supports both single line and multi-line selections
pub(crate) fn toggle_line_comment(tabular: &mut window_egui::Tabular) {
    let text_len = tabular.editor.text.len();
    if text_len == 0 {
        return;
    }

    // Get selection range
    let sel_start = tabular.selection_start.min(text_len);
    let sel_end = tabular.selection_end.min(text_len);
    let (range_start, range_end) = if sel_start < sel_end {
        (sel_start, sel_end)
    } else {
        // No selection, use cursor position to find current line
        let cursor = tabular.cursor_position.min(text_len);
        (cursor, cursor)
    };

    // Find the start of the first line
    let mut line_start = range_start;
    while line_start > 0 && tabular.editor.text.as_bytes()[line_start - 1] != b'\n' {
        line_start -= 1;
    }

    // Find the end of the last line (include the line with cursor if no selection)
    let mut line_end = if range_end > range_start {
        range_end
    } else {
        // Single line: find end of current line
        let mut end = range_start;
        while end < text_len && tabular.editor.text.as_bytes()[end] != b'\n' {
            end += 1;
        }
        end
    };
    
    // Clamp to text length
    line_end = line_end.min(text_len);

    // Extract the block of lines
    let block = &tabular.editor.text[line_start..line_end];
    
    // Check if all non-empty lines are commented
    let mut all_commented = true;
    let mut has_content_lines = false;
    
    for line in block.lines() {
        let trimmed = line.trim_start();
        if !trimmed.is_empty() {
            has_content_lines = true;
            if !trimmed.starts_with("--") {
                all_commented = false;
                break;
            }
        }
    }

    // If no content lines, treat as uncommented
    if !has_content_lines {
        all_commented = false;
    }

    // Build the new block
    let mut new_block = String::with_capacity(block.len() + 100);
    
    if all_commented {
        // Uncomment: remove "-- " or "--" from start of each line
        for line in block.split_inclusive('\n') {
            if line == "\n" {
                new_block.push('\n');
                continue;
            }
            
            let (content, nl) = if let Some(p) = line.rfind('\n') {
                (&line[..p], &line[p..])
            } else {
                (line, "")
            };
            
            let trimmed = content.trim_start();
            let indent_len = content.len() - trimmed.len();
            let indent = &content[..indent_len];
            
            if let Some(rest) = trimmed.strip_prefix("-- ") {
                new_block.push_str(indent);
                new_block.push_str(rest);
            } else if let Some(rest) = trimmed.strip_prefix("--") {
                new_block.push_str(indent);
                new_block.push_str(rest);
            } else {
                new_block.push_str(content);
            }
            new_block.push_str(nl);
        }
    } else {
        // Comment: add "-- " to start of each line
        for line in block.split_inclusive('\n') {
            if line == "\n" {
                new_block.push('\n');
                continue;
            }
            
            let (content, nl) = if let Some(p) = line.rfind('\n') {
                (&line[..p], &line[p..])
            } else {
                (line, "")
            };
            
            let trimmed = content.trim_start();
            let indent_len = content.len() - trimmed.len();
            let indent = &content[..indent_len];
            
            // Add comment marker
            new_block.push_str(indent);
            new_block.push_str("-- ");
            new_block.push_str(trimmed);
            new_block.push_str(nl);
        }
    }

    // Apply the change
    tabular
        .editor
        .apply_single_replace(line_start..line_end, &new_block);

    // Update selection to cover the modified block
    let new_end = line_start + new_block.len();
    tabular.selection_start = line_start;
    tabular.selection_end = new_end;
    tabular.cursor_position = new_end;

    // Mark tab as modified
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = tabular.editor.text.clone();
        tab.is_modified = true;
    } else {
        tabular.editor.mark_text_modified();
    }

    // Update autocomplete
    editor_autocomplete::update_autocomplete(tabular);
}

pub(crate) fn perform_replace_all(tabular: &mut window_egui::Tabular) {
    if tabular.advanced_editor.find_text.is_empty() {
        return;
    }

    let find_text = &tabular.advanced_editor.find_text;
    let replace_text = &tabular.advanced_editor.replace_text;

    let new_text = if tabular.advanced_editor.use_regex {
        if let Ok(re) = regex::Regex::new(find_text) {
            re.replace_all(&tabular.editor.text, replace_text)
                .into_owned()
        } else {
            return;
        }
    } else if tabular.advanced_editor.case_sensitive {
        tabular.editor.text.replace(find_text, replace_text)
    } else {
        // case-insensitive simple replace
        let src = tabular.editor.text.clone();
        let find_lower = find_text.to_lowercase();
        let mut result = String::new();
        let mut last = 0;
        let src_lower = src.to_lowercase();
        let mut i = 0;
        while let Some(pos) = src_lower[i..].find(&find_lower) {
            let start = i + pos;
            result.push_str(&src[last..start]);
            result.push_str(replace_text);
            last = start + find_lower.len();
            i = last;
        }
        result.push_str(&src[last..]);
        result
    };

    // Bulk set text via buffer to keep rope in sync and record undo
    tabular.editor.set_text(new_text.clone());
    // Keep cursor within bounds
    tabular.cursor_position = tabular.cursor_position.min(tabular.editor.text.len());
    // Update current tab content
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = new_text;
        tab.is_modified = true;
    }
}

pub(crate) fn find_next(tabular: &mut window_egui::Tabular) {
    // This is a simplified find implementation
    // In a real implementation, you'd want to track cursor position and highlight matches
    if !tabular.advanced_editor.find_text.is_empty()
        && let Some(_pos) = tabular.editor.text.find(&tabular.advanced_editor.find_text)
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

    // Initialize command palette items with shortcut hints
    tabular.command_palette_items = vec![
        "Query: Run                    ⌘ Enter".to_string(),
        "Query: Format SQL             ⌘ Shift+F".to_string(),
        "Query: Explain                ⌘ Shift+E".to_string(),
        "Query: New Tab                ⌘T".to_string(),
        "Query: Close Tab              ⌘W".to_string(),
        "Query: Save Tab               ⌘S".to_string(),
        "Editor: Go to Definition      F12".to_string(),
        "Editor: Rename Symbol         F2".to_string(),
        "Editor: Toggle Find & Replace ⌘F".to_string(),
        "Editor: Toggle Word Wrap".to_string(),
        "Editor: Toggle Line Numbers".to_string(),
        "Data: Export CSV".to_string(),
        "Data: Export JSON".to_string(),
        "Data: Export SQL Inserts".to_string(),
        "Data: Export Markdown".to_string(),
        "Data: Import CSV".to_string(),
        "Transaction: Begin / Toggle   ⌘ Shift+T".to_string(),
        "Transaction: Commit".to_string(),
        "Transaction: Rollback".to_string(),
        "View: Refresh                 ⌘R".to_string(),
        "Preferences: Color Theme".to_string(),
        "Preferences: Settings         ⌘,".to_string(),
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
    tabular.is_table_browse_mode = false;
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
    // Strip trailing shortcut hint (everything after first "  " sequence of spaces) for matching
    let cmd = command.trim_end();
    let key = if let Some(pos) = cmd.find("  ") { cmd[..pos].trim() } else { cmd };

    tabular.show_command_palette = false;
    tabular.command_palette_input.clear();
    tabular.command_palette_selected_index = 0;

    match key {
        "Query: Run" => {
            execute_query(tabular);
        }
        "Query: Format SQL" => {
            // reformat_current_sql requires a Ui reference; hint shown, user uses ⌘⇧F keyboard shortcut
        }
        "Query: Explain" => {
            let text = tabular.editor.text.clone();
            explain_current_query(tabular, text);
        }
        "Query: New Tab" => {
            create_new_tab(tabular, String::new(), String::new());
        }
        "Query: Close Tab" => {
            if !tabular.query_tabs.is_empty() {
                let idx = tabular.active_tab_index;
                close_tab(tabular, idx);
            }
        }
        "Query: Save Tab" => {
            let _ = save_current_tab(tabular);
        }
        "Editor: Go to Definition" => {
            go_to_definition(tabular);
        }
        "Editor: Rename Symbol" => {
            begin_rename_symbol(tabular);
        }
        "Editor: Toggle Find & Replace" => {
            tabular.advanced_editor.show_find_replace = !tabular.advanced_editor.show_find_replace;
        }
        "Editor: Toggle Word Wrap" => {
            tabular.advanced_editor.word_wrap = !tabular.advanced_editor.word_wrap;
        }
        "Editor: Toggle Line Numbers" => {
            tabular.advanced_editor.show_line_numbers = !tabular.advanced_editor.show_line_numbers;
        }
        "Data: Export CSV" => {
            crate::export::export_to_csv(
                &tabular.all_table_data,
                &tabular.current_table_headers,
                &tabular.current_table_name,
            );
        }
        "Data: Export JSON" => {
            crate::export::export_to_json(
                &tabular.all_table_data,
                &tabular.current_table_headers,
                &tabular.current_table_name,
            );
        }
        "Data: Export SQL Inserts" => {
            let db_type = tabular.current_connection_id
                .and_then(|id| tabular.connections.iter().find(|c| c.id == Some(id)))
                .map(|c| c.connection_type.clone());
            crate::export::export_to_sql_inserts(
                &tabular.all_table_data,
                &tabular.current_table_headers,
                &tabular.current_table_name,
                db_type.as_ref(),
            );
        }
        "Data: Export Markdown" => {
            crate::export::export_to_markdown(
                &tabular.all_table_data,
                &tabular.current_table_headers,
                &tabular.current_table_name,
            );
        }
        "Data: Import CSV" => {
            if let Some(conn_id) = tabular.current_connection_id {
                let db_type = tabular.connections.iter()
                    .find(|c| c.id == Some(conn_id))
                    .map(|c| c.connection_type.clone())
                    .unwrap_or(crate::models::enums::DatabaseType::MySQL);
                tabular.show_csv_import_dialog = true;
                tabular.csv_import_state = Some(crate::models::structs::CsvImportState {
                    connection_id: conn_id,
                    database_name: None,
                    table_name: tabular.current_table_name.clone(),
                    db_type,
                    file_path: None,
                    delimiter: ',',
                    has_header_row: true,
                    null_value: String::new(),
                    preview_headers: Vec::new(),
                    preview_rows: Vec::new(),
                    table_columns: tabular.current_table_headers.clone(),
                    column_mappings: Vec::new(),
                    status: crate::models::structs::CsvImportStatus::Idle,
                    progress_message: String::new(),
                });
            }
        }
        "Transaction: Begin / Toggle" => {
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.tx_mode = !tab.tx_mode;
            }
        }
        "Transaction: Commit" => {
            send_session_tx_command(tabular, true);
        }
        "Transaction: Rollback" => {
            send_session_tx_command(tabular, false);
        }
        "View: Refresh" => {
            crate::data_table::refresh_current_table_data(tabular);
        }
        "Preferences: Color Theme" => {
            tabular.request_theme_selector = true;
            tabular.theme_selector_selected_index = 0;
        }
        "Preferences: Settings" => {
            tabular.show_settings_window = true;
        }
        _ => {
            debug!("Unknown command: {}", key);
        }
    }
}

/// Extract the identifier word at `pos` in `text` (SQL identifier chars: alphanumeric + _).
fn word_at_cursor(text: &str, pos: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len == 0 || pos > len {
        return None;
    }
    // Clamp to last char if pos == len
    let p = pos.min(len.saturating_sub(1));
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    if !is_ident(bytes[p]) {
        return None;
    }
    let start = (0..=p).rev().take_while(|&i| is_ident(bytes[i])).last()?;
    let end = (p..len).take_while(|&i| is_ident(bytes[i])).last().map(|i| i + 1).unwrap_or(p + 1);
    Some(&text[start..end])
}

/// Find first tree node whose name (case-insensitive) matches `name` and is a table/view.
fn find_table_in_tree<'a>(
    nodes: &'a [crate::models::structs::TreeNode],
    name: &str,
) -> Option<&'a crate::models::structs::TreeNode> {
    use crate::models::enums::NodeType;
    for node in nodes {
        if matches!(node.node_type, NodeType::Table | NodeType::View)
            && node.name.to_lowercase() == name.to_lowercase()
        {
            return Some(node);
        }
        if let Some(found) = find_table_in_tree(&node.children, name) {
            return Some(found);
        }
    }
    None
}

/// Go-to-definition: finds the word under the editor cursor and navigates the sidebar
/// to the matching table/view. Shows a toast if nothing is found.
pub(crate) fn go_to_definition(tabular: &mut window_egui::Tabular) {
    let cursor = tabular.cursor_position;
    let text = tabular.editor.text.clone();
    let word = match word_at_cursor(&text, cursor) {
        Some(w) => w.to_string(),
        None => {
            tabular.toasts.info("Go to definition: no identifier at cursor");
            return;
        }
    };

    // Search sidebar tree for a matching table/view
    let tree = tabular.items_tree.clone();
    if let Some(node) = find_table_in_tree(&tree, &word) {
        // Navigate: set the connection from the node's context
        if let Some(conn_id) = node.connection_id {
            tabular.current_connection_id = Some(conn_id);
        }
        tabular.current_table_name = node.name.clone();
        // Expand the tree to reveal the node
        expand_tree_to_table(&mut tabular.items_tree, &word);
        tabular.toasts.info(format!("Go to definition: navigated to '{}'", word));
    } else {
        tabular.toasts.info(format!("Go to definition: '{}' not found in schema", word));
    }
}

/// Recursively expand tree nodes until a Table/View named `target` is found.
fn expand_tree_to_table(nodes: &mut [crate::models::structs::TreeNode], target: &str) -> bool {
    use crate::models::enums::NodeType;
    for node in nodes.iter_mut() {
        if matches!(node.node_type, NodeType::Table | NodeType::View)
            && node.name.to_lowercase() == target.to_lowercase()
        {
            node.is_expanded = true;
            return true;
        }
        if expand_tree_to_table(&mut node.children, target) {
            node.is_expanded = true;
            return true;
        }
    }
    false
}

/// Begin rename: extract word at cursor and open the rename dialog.
pub(crate) fn begin_rename_symbol(tabular: &mut window_egui::Tabular) {
    let cursor = tabular.cursor_position;
    let text = tabular.editor.text.clone();
    match word_at_cursor(&text, cursor) {
        Some(w) => {
            tabular.rename_symbol_old = w.to_string();
            tabular.rename_symbol_new = w.to_string();
            tabular.rename_symbol_active = true;
        }
        None => {
            tabular.toasts.info("Rename: no identifier at cursor");
        }
    }
}

/// Apply the rename: replace all whole-word occurrences of `old` with `new` in the current tab.
pub(crate) fn commit_rename_symbol(tabular: &mut window_egui::Tabular) {
    let old = tabular.rename_symbol_old.clone();
    let new = tabular.rename_symbol_new.clone();
    tabular.rename_symbol_active = false;

    if old.is_empty() || new.is_empty() || old == new {
        return;
    }

    // Replace whole-word occurrences (not inside longer identifiers)
    let text = tabular.editor.text.clone();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();
    let old_chars: Vec<char> = old.chars().collect();
    let olen = old_chars.len();

    while i < chars.len() {
        if chars[i..].starts_with(&old_chars) {
            let before_ok = i == 0 || !is_ident(chars[i - 1]);
            let after_ok = (i + olen) >= chars.len() || !is_ident(chars[i + olen]);
            if before_ok && after_ok {
                result.push_str(&new);
                i += olen;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    tabular.editor.text = result;
    tabular.toasts.info(format!("Renamed '{}' → '{}'", old, new));
}

/// Render the floating rename-symbol dialog.
pub(crate) fn render_rename_symbol_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    let mut commit = false;
    let mut cancel = false;

    egui::Area::new(egui::Id::new("rename_symbol_dialog"))
        .fixed_pos(egui::pos2(
            ctx.screen_rect().center().x - 200.0,
            ctx.screen_rect().center().y - 60.0,
        ))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(ui.style().visuals.window_fill)
                .stroke(ui.style().visuals.window_stroke)
                .shadow(egui::epaint::Shadow::default())
                .inner_margin(egui::Margin::same(16))
                .show(ui, |ui| {
                    ui.set_min_width(400.0);
                    ui.label(egui::RichText::new(format!("Rename '{}'", tabular.rename_symbol_old)).strong());
                    ui.add_space(8.0);
                    let resp = ui.add_sized(
                        [380.0, 24.0],
                        egui::TextEdit::singleline(&mut tabular.rename_symbol_new)
                            .hint_text("New name…"),
                    );
                    resp.request_focus();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            commit = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
        });

    // Check Enter/Escape outside the closure (no borrow conflict)
    ctx.input(|i| {
        if i.key_pressed(egui::Key::Enter) {
            commit = true;
        }
        if i.key_pressed(egui::Key::Escape) {
            cancel = true;
        }
    });

    if commit {
        commit_rename_symbol(tabular);
    } else if cancel {
        tabular.rename_symbol_active = false;
        tabular.rename_symbol_old.clear();
        tabular.rename_symbol_new.clear();
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
                        let themes = [
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
                                        .color(egui::Color32::from_rgb(0, 150, 255))
                                // Blue for current
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

pub(crate) fn execute_query_with_text(tabular: &mut window_egui::Tabular, selected_text: String) {
    tabular.is_table_browse_mode = false;
    tabular.extend_query_icon_hold();

    let text_hash = format!("{:x}", md5::compute(&selected_text));
    log::debug!(
        "🚀 EXECUTE - Received (len {}, hash {}): '{}'",
        selected_text.len(),
        text_hash,
        selected_text.chars().take(150).collect::<String>()
    );
    log::debug!(
        "   pending_query: '{}'",
        tabular.pending_query.chars().take(50).collect::<String>()
    );
    log::debug!(
        "   tabular.selected_text field: '{}'",
        tabular.selected_text.chars().take(100).collect::<String>()
    );

    let mut used_pending_query = false;
    let query = if !tabular.pending_query.trim().is_empty() {
        used_pending_query = true;
        log::debug!("   ✓ Using pending_query");
        tabular.pending_query.trim().to_string()
    } else if !selected_text.trim().is_empty() {
        log::debug!(
            "   ✓ Using provided selected_text (len: {})",
            selected_text.len()
        );
        let result = selected_text.trim().to_string();
        log::debug!("   After trim, query length: {}", result.len());
        result
    } else {
        log::debug!("   ⚠️ Falling back to cursor/full text");
        let cursor_query = extract_query_from_cursor(tabular);
        if !cursor_query.trim().is_empty() {
            log::debug!("   ✓ Using cursor query");
            cursor_query
        } else {
            log::debug!("   ✓ Using full editor text");
            tabular.editor.text.trim().to_string()
        }
    };

    if used_pending_query && tabular.selected_text.trim().is_empty() {
        tabular.selected_text = query.clone();
    }

    log::debug!(
        "   Final query (len {}): '{}'",
        query.len(),
        query.chars().take(150).collect::<String>()
    );

    execute_query_internal(tabular, query);
}

/// Run the engine-appropriate EXPLAIN for the current statement
/// (selection > statement at cursor > full editor text). The plan comes
/// back through the normal result grid.
pub(crate) fn explain_current_query(
    tabular: &mut window_egui::Tabular,
    selected_text: String,
) {
    tabular.is_table_browse_mode = false;
    tabular.extend_query_icon_hold();

    let raw = if !selected_text.trim().is_empty() {
        selected_text.trim().to_string()
    } else {
        let cursor_query = extract_query_from_cursor(tabular);
        if !cursor_query.trim().is_empty() {
            cursor_query
        } else {
            tabular.editor.text.trim().to_string()
        }
    };
    if raw.is_empty() {
        return;
    }

    let connection_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id)
        .or(tabular.current_connection_id);
    let Some(connection_id) = connection_id else {
        tabular
            .toasts
            .error("Pick a connection before running EXPLAIN".to_string());
        return;
    };
    let connection_type = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .map(|c| c.connection_type.clone());

    let prefix = match connection_type {
        Some(crate::models::enums::DatabaseType::MySQL)
        | Some(crate::models::enums::DatabaseType::PostgreSQL) => "EXPLAIN ",
        Some(crate::models::enums::DatabaseType::SQLite) => "EXPLAIN QUERY PLAN ",
        // MsSQL needs SET SHOWPLAN_ALL in its own batch — follow-up work.
        _ => {
            tabular
                .toasts
                .error("EXPLAIN is not supported for this connection type yet".to_string());
            return;
        }
    };

    // EXPLAIN applies to a single statement: take the first one.
    let is_mysql = matches!(
        connection_type,
        Some(crate::models::enums::DatabaseType::MySQL)
    );
    let stmt = connection::split_sql_statements(&raw, is_mysql)
        .into_iter()
        .next()
        .unwrap_or(raw);

    let explain_sql = if stmt.trim_start().to_uppercase().starts_with("EXPLAIN") {
        stmt
    } else {
        format!("{}{}", prefix, stmt)
    };
    execute_query_internal(tabular, explain_sql);
}

pub(crate) fn execute_query(tabular: &mut window_egui::Tabular) {
    tabular.is_table_browse_mode = false;
    tabular.extend_query_icon_hold();

    // Priority: 1) Pending query (auto-run after connection), 2) Selected text (already captured),
    // 3) Query from cursor position, 4) Full editor text
    // NOTE: selected_text is already refreshed by capture_current_editor_selection before this call
    log::debug!("🚀 execute_query called");
    log::debug!(
        "   pending_query: '{}'",
        tabular.pending_query.chars().take(50).collect::<String>()
    );
    log::debug!(
        "   selected_text: '{}'",
        tabular.selected_text.chars().take(50).collect::<String>()
    );

    let mut used_pending_query = false;
    let query = if !tabular.pending_query.trim().is_empty() {
        used_pending_query = true;
        log::debug!("   ✓ Using pending_query");
        tabular.pending_query.trim().to_string()
    } else if !tabular.selected_text.trim().is_empty() {
        log::debug!("   ✓ Using selected_text");
        tabular.selected_text.trim().to_string()
    } else {
        let cursor_query = extract_query_from_cursor(tabular);
        if !cursor_query.trim().is_empty() {
            log::debug!("   ✓ Using cursor query");
            cursor_query
        } else {
            log::debug!("   ✓ Using full editor text");
            tabular.editor.text.trim().to_string()
        }
    };

    if used_pending_query && tabular.selected_text.trim().is_empty() {
        tabular.selected_text = query.clone();
    }

    log::debug!(
        "   Final query to execute: '{}'",
        query.chars().take(100).collect::<String>()
    );

    execute_query_internal(tabular, query);
}

fn execute_query_internal(tabular: &mut window_egui::Tabular, mut query: String) {
    query = query.trim().to_string();

    // Reset pagination state for each fresh execution; we will re-enable if heuristics say so.
    tabular.use_server_pagination = false;
    tabular.current_base_query.clear();
    tabular.current_page = 0;
    tabular.actual_total_rows = None;

    tabular.lint_messages = query_tools::lint_sql(&query);
    tabular.show_lint_panel = !tabular.lint_messages.is_empty();

    if tabular.auto_format_on_execute
        && let Some(formatted) = query_tools::format_sql(&query)
        && formatted != query
    {
        let executed_full_editor = tabular.editor.text.trim() == query;
        query = formatted.clone();
        if executed_full_editor {
            tabular.editor.set_text(formatted);
            let new_len = tabular.editor.text.len();
            tabular.cursor_position = tabular.cursor_position.min(new_len);
            tabular.multi_selection.clear();
            tabular
                .multi_selection
                .add_collapsed(tabular.cursor_position);
            tabular.last_editor_text = tabular.editor.text.clone();
        }
    }

    if query.is_empty() {
        tabular.query_execution_in_progress = false;
        tabular.extend_query_icon_hold();
        tabular.current_table_name = "No query to execute".to_string();
        tabular.current_table_headers.clear();
        tabular.current_table_data.clear();
        return;
    }

    // Reset pagination state before evaluating auto-pagination rules
    tabular.use_server_pagination = false;

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
        tabular.query_execution_in_progress = false;
        tabular.extend_query_icon_hold();
        return;
    }

    // Check if we have an active connection
    if let Some(connection_id) = connection_id {
        // Clear pending query since we're executing now
        tabular.pending_query.clear();

        // Clear existing results in the active tab since we are running a new batch
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.results.clear();
            tab.active_result_index = 0;
        }

        // Split the script into statements (quote/comment aware). '#' starts
        // a line comment only on MySQL, where it is not an operator.
        let hash_is_comment = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(connection_id))
            .map(|c| {
                matches!(
                    c.connection_type,
                    crate::models::enums::DatabaseType::MySQL
                )
            })
            .unwrap_or(false);
        let mut statements = connection::split_sql_statements(&query, hash_is_comment);

        if statements.is_empty() {
            // Should not happen as we checked query.is_empty() above
            statements.push(query.clone());
        }

        tabular.query_execution_in_progress = true;
        
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

        // If no pool exists yet, trigger background creation; show loading
        if !tabular.connection_pools.contains_key(&connection_id) {
            log::debug!(
                "🔧 Pool not ready for {}, triggering background creation and queuing",
                connection_id
            );
            
            // Trigger creation (safe to call multiple times, handles dedup)
            crate::connection::ensure_background_pool_creation(tabular, connection_id);

            // Set wait state so the UI shows a spinner and we retry later
            tabular.pool_wait_in_progress = true;
            tabular.pool_wait_connection_id = Some(connection_id);
            tabular.pool_wait_query = query.clone();
            tabular.pool_wait_started_at = Some(std::time::Instant::now());
            tabular.current_table_name = "Connecting… waiting for pool".to_string();
            return;
        }

        debug!("=== EXECUTING {} QUERIES ===", statements.len());
        debug!("Connection ID: {}", connection_id);
        
        // Manual-commit mode: route statements to the tab's dedicated session
        // connection so BEGIN/COMMIT and session state persist across runs.
        let tx_mode_active = tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .map(|t| t.tx_mode)
            .unwrap_or(false);
        if tx_mode_active {
            let supported = tabular
                .connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .map(|c| crate::connection::session::supports_transactions(&c.connection_type))
                .unwrap_or(false);
            if supported {
                execute_statements_in_session(tabular, connection_id, statements);
                return;
            }
        }

        if statements.len() == 1 {
            let stmt = statements.remove(0);

            // Auto-enable server-side pagination when the query does not specify
            // LIMIT/TOP/OFFSET/FETCH. Only applicable to a single statement.
            if connection::should_enable_auto_pagination(&stmt) {
                let base_query = stmt.trim().trim_end_matches(';').to_string();

                tabular.use_server_pagination = true;
                tabular.current_base_query = base_query.clone();
                tabular.current_page = 0;
                tabular.actual_total_rows = Some(10_000);

                if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                    tab.base_query = base_query;
                    tab.current_page = tabular.current_page;
                    tab.page_size = tabular.page_size;
                }

                debug!("🚀 Auto server-pagination enabled (simple SELECT). Executing first page...");
                tabular.execute_paginated_query();
                return;
            }

            let job_id = tabular.next_query_job_id;
            tabular.next_query_job_id = tabular.next_query_job_id.wrapping_add(1);

            match connection::prepare_query_job(tabular, connection_id, stmt.clone(), job_id) {
                Ok(job) => {
                    let status = connection::QueryJobStatus {
                        job_id,
                        connection_id,
                        query_preview: stmt.chars().take(80).collect(),
                        started_at: Instant::now(),
                        completed: false,
                    };
                    tabular.active_query_jobs.insert(job_id, status);

                    match connection::spawn_query_job(tabular, job, tabular.query_result_sender.clone())
                    {
                        Ok(handle) => {
                            tabular.active_query_handles.insert(job_id, handle);
                            tabular.current_table_name = "Running query…".to_string();
                        }
                        Err(err) => {
                            tabular.active_query_jobs.remove(&job_id);
                            debug!("Failed to spawn async job: {:?}", err);
                        }
                    }
                }
                Err(err) => {
                    debug!("Failed to prepare async job: {:?}", err);
                }
            }
        } else {
            // Sequential batch: prepare every statement first, then run them
            // in order on ONE background task so script-like input behaves
            // like a script (no races between pool connections). Results
            // arrive per statement, in statement order.
            let total = statements.len();
            let mut jobs = Vec::with_capacity(total);
            let mut job_ids = Vec::with_capacity(total);

            for (idx, stmt) in statements.into_iter().enumerate() {
                debug!("Preparing statement {}/{}: {}", idx + 1, total, stmt);
                let job_id = tabular.next_query_job_id;
                tabular.next_query_job_id = tabular.next_query_job_id.wrapping_add(1);

                match connection::prepare_query_job(tabular, connection_id, stmt.clone(), job_id) {
                    Ok(job) => {
                        let preview: String = stmt.chars().take(72).collect();
                        let status = connection::QueryJobStatus {
                            job_id,
                            connection_id,
                            query_preview: format!("[{}/{}] {}", idx + 1, total, preview),
                            started_at: Instant::now(),
                            completed: false,
                        };
                        tabular.active_query_jobs.insert(job_id, status);
                        job_ids.push(job_id);
                        jobs.push(job);
                    }
                    Err(err) => {
                        debug!("Failed to prepare statement {}/{}: {:?}", idx + 1, total, err);
                    }
                }
            }

            if jobs.is_empty() {
                tabular.query_execution_in_progress = false;
                return;
            }

            match connection::spawn_query_job_batch(tabular, jobs, tabular.query_result_sender.clone())
            {
                Ok(handle) => {
                    // The whole batch runs on one task; cancelling any member
                    // job id aborts the entire batch (see cancel_active_query_job).
                    let last_id = *job_ids.last().expect("jobs not empty");
                    tabular
                        .query_job_batches
                        .push((job_ids, handle.abort_handle()));
                    tabular.active_query_handles.insert(last_id, handle);
                    tabular.current_table_name = format!("Running {} queries…", total);
                }
                Err(err) => {
                    for job_id in &job_ids {
                        tabular.active_query_jobs.remove(job_id);
                    }
                    tabular.query_execution_in_progress = false;
                    debug!("Failed to spawn batch job: {:?}", err);
                }
            }
        }
    }
}

/// Send statements to the active tab's dedicated session connection
/// (manual-commit mode), creating or replacing the session as needed.
fn execute_statements_in_session(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    statements: Vec<String>,
) {
    let database_name = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone());

    // (Re)create the session when missing, dead, or bound to another connection.
    let needs_new = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| match &t.session {
            Some(s) => s.connection_id != connection_id || s.sender.is_closed(),
            None => true,
        })
        .unwrap_or(true);
    if needs_new {
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index)
            && let Some(old) = tab.session.take()
        {
            old.close();
            tab.tx_active = false;
        }
        let new_session =
            crate::connection::session::spawn_session(tabular, connection_id, database_name);
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.session = new_session;
        }
    }

    let Some(session) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.session.clone())
    else {
        tabular.error_message =
            "Cannot start a session connection for manual-commit mode".to_string();
        tabular.show_error_message = true;
        tabular.query_execution_in_progress = false;
        return;
    };

    let total = statements.len();
    for (idx, stmt) in statements.into_iter().enumerate() {
        let job_id = tabular.next_query_job_id;
        tabular.next_query_job_id = tabular.next_query_job_id.wrapping_add(1);
        let preview: String = stmt.chars().take(72).collect();
        let status = connection::QueryJobStatus {
            job_id,
            connection_id,
            query_preview: if total > 1 {
                format!("[tx {}/{}] {}", idx + 1, total, preview)
            } else {
                format!("[tx] {}", preview)
            },
            started_at: Instant::now(),
            completed: false,
        };
        tabular.active_query_jobs.insert(job_id, status);

        if !session.send(crate::connection::session::SessionCommand::Execute {
            job_id,
            sql: stmt,
        }) {
            tabular.active_query_jobs.remove(&job_id);
            tabular.error_message =
                "Session connection is gone; toggle manual commit off and on again".to_string();
            tabular.show_error_message = true;
            tabular.query_execution_in_progress = false;
            return;
        }
    }

    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.tx_active = true;
    }
    tabular.current_table_name = if total > 1 {
        format!("Running {} queries (tx)…", total)
    } else {
        "Running query (tx)…".to_string()
    };
}

/// Send COMMIT or ROLLBACK to the active tab's session (manual-commit mode).
pub(crate) fn send_session_tx_command(tabular: &mut window_egui::Tabular, commit: bool) {
    let Some(session) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.session.clone())
    else {
        return;
    };
    let job_id = tabular.next_query_job_id;
    tabular.next_query_job_id = tabular.next_query_job_id.wrapping_add(1);
    let verb = if commit { "COMMIT" } else { "ROLLBACK" };
    let status = connection::QueryJobStatus {
        job_id,
        connection_id: session.connection_id,
        query_preview: format!("[tx] {}", verb),
        started_at: Instant::now(),
        completed: false,
    };
    tabular.active_query_jobs.insert(job_id, status);
    tabular.query_execution_in_progress = true;

    let command = if commit {
        crate::connection::session::SessionCommand::Commit { job_id }
    } else {
        crate::connection::session::SessionCommand::Rollback { job_id }
    };
    if !session.send(command) {
        tabular.active_query_jobs.remove(&job_id);
        tabular.query_execution_in_progress = false;
    }
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.tx_active = false;
    }
}

pub(crate) fn process_query_result(
    tabular: &mut window_egui::Tabular,
    query: &str,
    connection_id: i64,
    result: Option<(Vec<String>, Vec<Vec<String>>)>,
    column_metadata: Option<Vec<models::structs::ColumnMetadata>>,
) {
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
        tabular.current_column_metadata = column_metadata;

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
            let mut clean_query = query.to_string();
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

        let allows_auto_pagination =
            connection::should_enable_auto_pagination(&base_query_for_pagination);

        if allows_auto_pagination {
            tabular.current_base_query = base_query_for_pagination.clone();
            debug!(
                "📝 Set base_query for pagination: '{}'",
                base_query_for_pagination
            );
        } else {
            tabular.current_base_query.clear();
            debug!("📝 Skipping base_query persistence (not eligible for auto pagination)");
        }

        // Save query to history hanya jika bukan hasil error
        if !is_error_result {
            sidebar_history::save_query_to_history(tabular, query, connection_id);
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
            if allows_auto_pagination {
                tab.base_query = tabular.current_base_query.clone();
            } else {
                tab.base_query.clear();
            }
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

    tabular.query_execution_in_progress = false;
    tabular.extend_query_icon_hold();
}

pub(crate) fn extract_query_from_cursor(tabular: &mut window_egui::Tabular) -> String {
    if tabular.editor.text.is_empty() {
        return String::new();
    }

    let text_bytes = tabular.editor.text.as_bytes();
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
