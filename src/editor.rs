use eframe::egui;
use egui::text_edit::TextEditState;
use egui::text::{CCursor, CCursorRange};
use egui_code_editor::{CodeEditor, ColorTheme};
use log::{debug};

use crate::{connection, directory, editor, models, sidebar_history, sidebar_query, window_egui, editor_autocomplete};


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
                debug!("💾 Saving tab {} state: base_query='{}'", tabular.active_tab_index, current_tab.base_query);
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
                debug!("📂 Restoring tab {} state: base_query='{}', connection_id={:?}", tab_index, new_tab.base_query, new_tab.connection_id);
                // IMPORTANT: kembalikan connection id aktif sesuai tab baru
                tabular.current_connection_id = new_tab.connection_id;

                // Jika user sedang berada di tampilan Structure dan tab tujuan adalah tab Table, reload struktur tabel tsb.
                if tabular.table_bottom_view == models::structs::TableBottomView::Structure && new_tab.title.starts_with("Table:") {
                    // load_structure_info_for_current_table adalah metode pada Tabular (dibuat pub(crate))
                    tabular.load_structure_info_for_current_table();
                }
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
                
                // Initialize save directory with config query directory
                if tabular.save_directory.is_empty() {
                    tabular.save_directory = crate::directory::get_query_dir().to_string_lossy().to_string();
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
            // Use selected save directory or fallback to query directory
            let target_dir = if !tabular.save_directory.is_empty() {
                std::path::PathBuf::from(&tabular.save_directory)
            } else {
                directory::get_query_dir()
            };
            
            // Ensure the target directory exists
            std::fs::create_dir_all(&target_dir).map_err(|e| format!("Failed to create target directory: {}", e))?;
            
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
    // (Removed pre_text clone; not needed now)
    // Detect Tab & Enter via key_pressed AND raw events (key_pressed may miss if focus moving)
    let mut tab_pressed_pre = ui.input(|i| i.key_pressed(egui::Key::Tab));
    let shift_pressed_pre = ui.input(|i| i.modifiers.shift);
    // Snapshot current selection BEFORE rendering (so we still have original range if CodeEditor would replace it)
    let pre_selection_start = tabular.selection_start;
    let pre_selection_end = tabular.selection_end;
    let pre_selected_text = if pre_selection_start < pre_selection_end && pre_selection_end <= tabular.editor_text.len() { Some(tabular.editor_text[pre_selection_start..pre_selection_end].to_string()) } else { None };
    // (flag removed) multiline indent handled pre-render
    if tab_pressed_pre {
        if let Some(sel_text) = &pre_selected_text {
            if sel_text.contains('\n') { // multi-line selection
                // Compute line_start (start of first line)
                let mut line_start = pre_selection_start;
                while line_start > 0 && tabular.editor_text.as_bytes()[line_start-1] != b'\n' { line_start -= 1; }
                let sel_end_clamped = pre_selection_end.min(tabular.editor_text.len());
                let block = tabular.editor_text[line_start..sel_end_clamped].to_string();
                if !shift_pressed_pre { // Indent
                    let mut indented = String::with_capacity(block.len()+8);
                    for line in block.split_inclusive('\n') {
                        if line == "\n" { indented.push('\n'); continue; }
                        let (content, nl) = if let Some(p) = line.rfind('\n') { (&line[..p], &line[p..]) } else { (line, "") };
                        indented.push('\t');
                        indented.push_str(content);
                        indented.push_str(nl);
                    }
                    tabular.editor_text.replace_range(line_start..sel_end_clamped, &indented);
                    tabular.selection_start = line_start;
                    tabular.selection_end = line_start + indented.len();
                    tabular.cursor_position = tabular.selection_end;
                } else { // Outdent
                    let mut outdented = String::with_capacity(block.len());
                    let mut changed = false;
                    for line in block.split_inclusive('\n') {
                        if line == "\n" { outdented.push('\n'); continue; }
                        let (content, nl) = if let Some(p) = line.rfind('\n') { (&line[..p], &line[p..]) } else { (line, "") };
                        let trimmed = if let Some(rest) = content.strip_prefix('\t') { changed = true; rest }
                        else if let Some(rest) = content.strip_prefix("    ") { changed = true; rest }
                        else { content };
                        outdented.push_str(trimmed); outdented.push_str(nl);
                    }
                    if changed {
                        tabular.editor_text.replace_range(line_start..sel_end_clamped, &outdented);
                        tabular.selection_start = line_start;
                        tabular.selection_end = line_start + outdented.len();
                        tabular.cursor_position = tabular.selection_end;
                    }
                }
                // Prevent CodeEditor from seeing Tab key (so it doesn't insert a tab or replace selection)
                ui.ctx().input_mut(|ri| { ri.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Tab, .. })); });
                // indentation applied
                // Mark changed for autocomplete logic later
                if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) { tab.content = tabular.editor_text.clone(); tab.is_modified = true; }
            }
        }
    }
    let mut enter_pressed_pre = ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut raw_tab = false;
        // Intercept arrow keys when autocomplete popup shown so caret tidak ikut bergerak
        let mut arrow_down_pressed = false;
        let mut arrow_up_pressed = false;
        ui.input(|i| {
            for ev in &i.events {
                if let egui::Event::Key { key: egui::Key::Tab, pressed: true, .. } = ev { raw_tab = true; }
            }
        });
        if tabular.show_autocomplete {
            ui.ctx().input_mut(|ri| {
                // Drain & filter events: buang ArrowUp/ArrowDown pressed supaya TextEdit tidak memproses
                let mut kept = Vec::with_capacity(ri.events.len());
                for ev in ri.events.drain(..) {
                    match ev {
                        egui::Event::Key { key: egui::Key::ArrowDown, pressed: true, .. } => { arrow_down_pressed = true; }
                        egui::Event::Key { key: egui::Key::ArrowUp, pressed: true, .. } => { arrow_up_pressed = true; }
                        // Intercept Enter pressed untuk autocomplete acceptance (supaya tidak newline)
                        egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => { enter_pressed_pre = true; }
                        // Jangan hilangkan release events agar repeat logic internal tidak stuck; hanya pressed yang kita konsumsi
                        other @ egui::Event::Key { key: egui::Key::ArrowDown, pressed: false, .. } => { kept.push(other); }
                        other @ egui::Event::Key { key: egui::Key::ArrowUp, pressed: false, .. } => { kept.push(other); }
                        other => kept.push(other)
                    }
                }
                ri.events = kept;
            });
        }
    if raw_tab { tab_pressed_pre = true; log::debug!("Raw Tab event captured before editor render"); }
        let accept_via_tab_pre = tab_pressed_pre && tabular.show_autocomplete;
        let accept_via_enter_pre = enter_pressed_pre && tabular.show_autocomplete;
        // If accepting, prepare to inject remaining characters as text events so caret advances naturally
        if accept_via_tab_pre || accept_via_enter_pre {
            if let Some(sugg) = tabular.autocomplete_suggestions.get(tabular.selected_autocomplete_index).cloned() {
                // Remove the Tab key event itself so CodeEditor won't insert a tab char
                ui.ctx().input_mut(|ri| {
                    let before = ri.events.len();
                    ri.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Tab, .. }));
                    let removed = before - ri.events.len();
                    if removed > 0 { log::debug!("Removed {} Tab key event(s) to prevent tab insertion", removed); }
                });
                // Remove Enter key event(s) so tidak newline
                ui.ctx().input_mut(|ri| {
                    let before = ri.events.len();
                    ri.events.retain(|e| !matches!(e, egui::Event::Key { key: egui::Key::Enter, .. }));
                    let removed = before - ri.events.len();
                    if removed > 0 { log::debug!("Removed {} Enter key event(s) to prevent newline", removed); }
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
                log::debug!("Autocomplete accepted via {} by injecting remainder (suggestion '{}')", if accept_via_tab_pre {"Tab"} else {"Enter"}, sugg);
            }
        }
    // Add small horizontal padding so the editor doesn't get clipped on the right edge
    let response = egui::Frame::default()
        .inner_margin(egui::Margin::rightf(2.0.into()))
        .show(ui, |ui| {
            editor.show(ui, &mut tabular.editor_text)
        })
        .inner;
        // After show(), TextEditState should exist; apply pending cursor now
        if let Some(pos) = tabular.pending_cursor_set {
            let id = egui::Id::new("sql_editor");
                if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
                    let clamped = pos.min(tabular.editor_text.len());
                    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(clamped))));
                    state.store(ui.ctx(), id);
                    tabular.cursor_position = clamped;
                    tabular.pending_cursor_set = None;
                    ui.memory_mut(|m| m.request_focus(id));
                    debug!("Applied pending cursor position {} post-show", clamped);
                } else {
                    // Create a new state manually so cursor moves even if CodeEditor doesn't create one
                    let mut state = TextEditState::default();
                    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
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
                let ch = bytes[idx-1] as char;
                if ch.is_alphanumeric() || ch == '_' || ch == '\t' { idx -= 1; } else { break; }
            }
            // Now [idx .. cursor_position] spans the token (possibly including a tab)
            if idx < tabular.cursor_position {
                let token_range_end = tabular.cursor_position;
                let token_owned = tabular.editor_text[idx..token_range_end].to_string();
                if token_owned.contains('\t') {
                    let cleaned: String = token_owned.chars().filter(|c| *c != '\t').collect();
                    if cleaned != token_owned {
                        tabular.editor_text.replace_range(idx..token_range_end, &cleaned);
                        let shift = token_owned.len() - cleaned.len();
                        tabular.cursor_position -= shift;
                        // Adjust egui state cursor
                        let id = egui::Id::new("sql_editor");
                        if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
                            state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(tabular.cursor_position))));
                            state.store(ui.ctx(), id);
                        } else {
                            tabular.pending_cursor_set = Some(tabular.cursor_position);
                        }
                        log::debug!("Removed tab character from accepted token; new token='{}'", cleaned);
                    }
                }
            }
        }
        
        // Try to capture selected text from the response
        // Note: This is a simplified approach. The actual implementation may vary depending on the CodeEditor version
        if let Some(text_cursor_range) = response.cursor_range {
            let start = text_cursor_range.primary.index.min(text_cursor_range.secondary.index);
            let end = text_cursor_range.primary.index.max(text_cursor_range.secondary.index);
            tabular.cursor_position = text_cursor_range.primary.index;
            tabular.selection_start = start;
            tabular.selection_end = end;
            if start != end {
                if let Some(selected) = tabular.editor_text.get(start..end) { tabular.selected_text = selected.to_string(); } else { tabular.selected_text.clear(); }
            } else { tabular.selected_text.clear(); }
        } else { tabular.selected_text.clear(); }

        // Reset table focus flag when editor is interacted with
        if response.response.clicked() || response.response.has_focus() {
            tabular.table_recently_clicked = false;
        }

    // (Multi-line indent already handled pre-render if applied)
        
        // If you get a type error here, try:
        // let mut buffer = egui_code_editor::SimpleTextBuffer::from(&tabular.editor_text);
        // let response = editor.show(ui, &mut buffer);
        // tabular.editor_text = buffer.text().to_string();
        
        // Update tab content when editor changes (but skip autocomplete update if we're accepting via Tab)
        if response.response.changed() {
            if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                tab.content = tabular.editor_text.clone();
                tab.is_modified = true;
            }
            if !accept_via_tab_pre { // don't recalc suggestions; we need the old one
                editor_autocomplete::update_autocomplete(tabular);
            } else {
                log::debug!("Skipping update_autocomplete due to Tab acceptance in progress");
            }
        }

    // (Old forced replacement path removed; injection handles caret advance)

    // Keyboard handling for autocomplete
        let input = ui.input(|i| i.clone());
        if input.key_pressed(egui::Key::Space) && (input.modifiers.ctrl || input.modifiers.command) {
            editor_autocomplete::trigger_manual(tabular);
        }

        // Fallback: detect raw tab character insertion (editor consumed Tab key)
    if tabular.show_autocomplete && !tab_pressed_pre { // only if we didn't already detect it
                let cur = tabular.cursor_position.min(tabular.editor_text.len());
                if cur > 0 && tabular.editor_text.chars().nth(cur - 1) == Some('\t') {
                    // Remove the inserted tab char then accept suggestion
                    tabular.editor_text.remove(cur - 1);
                    tabular.cursor_position = tabular.cursor_position.saturating_sub(1);
                    log::debug!("Detected tab character insertion -> triggering autocomplete accept");
                    editor_autocomplete::accept_current_suggestion(tabular);
                } else if cur >= 4 && &tabular.editor_text[cur-4..cur] == "    " {
                    // Four spaces indentation
                    tabular.editor_text.replace_range(cur-4..cur, "");
                    tabular.cursor_position -= 4;
                    log::debug!("Detected 4-space indentation -> triggering autocomplete accept");
                    editor_autocomplete::accept_current_suggestion(tabular);
                }
    }
        if tabular.show_autocomplete {
            // Navigasi popup autocomplete: gunakan arrow yang sudah kita intercept sebelum render editor
            if arrow_down_pressed { editor_autocomplete::navigate(tabular, 1); }
            if arrow_up_pressed { editor_autocomplete::navigate(tabular, -1); }
            let mut accepted = false;
            if input.key_pressed(egui::Key::Enter) && !accept_via_enter_pre { editor_autocomplete::accept_current_suggestion(tabular); accepted = true; }
            // Skip Tab acceptance here if already processed earlier
            if tab_pressed_pre && !accept_via_tab_pre { editor_autocomplete::accept_current_suggestion(tabular); accepted = true; }
            if accepted {
        log::debug!("Autocomplete accepted via {}", if tab_pressed_pre {"Tab"} else {"Enter(post)"});
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
                if cur >= 4 && &tabular.editor_text[cur-4..cur] == "    " {
                    tabular.editor_text.replace_range(cur-4..cur, "");
                    tabular.cursor_position -= 4;
                }
                // Update internal egui state for cursor after Enter accept path
                let id = egui::Id::new("sql_editor");
                if let Some(mut state) = TextEditState::load(ui.ctx(), id) {
                    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(tabular.cursor_position))));
                    state.store(ui.ctx(), id);
                } else {
                    tabular.pending_cursor_set = Some(tabular.cursor_position);
                }
        // Re-focus editor so Tab doesn't move focus away
        ui.memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
            }
            if input.key_pressed(egui::Key::Escape) { tabular.show_autocomplete = false; }
        }

        // Update suggestions saat kursor bergerak kiri/kanan (tanpa perubahan teks)
        let moved_lr = input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowRight);
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
                if i >= cursor { break; }
                if ch == '\n' { line_no += 1; line_start = i + 1; }
            }
            let column = cursor - line_start;
            let char_w = 8.0_f32; // heuristic monospace width
            let line_h = ui.text_style_height(&egui::TextStyle::Monospace);
            let editor_rect = response.response.rect; // CodeEditor main rect
            let mut pos = egui::pos2(editor_rect.left() + 8.0 + (column as f32)*char_w,
                                     editor_rect.top() + 4.0 + (line_no as f32 + 1.0)*line_h);
            // Clamp horizontally inside editor area
            if pos.x > editor_rect.right() - 150.0 { pos.x = editor_rect.right() - 150.0; }
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
                let is_error_result = headers.first().map(|h| h == "Error").unwrap_or(false);
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
                
                // Set the base query for pagination - this is crucial for regular queries!
                // For regular queries, we set the base query to the executed query (without LIMIT)
                let base_query_for_pagination = if !is_error_result && tabular.total_rows > 0 {
                    // Simple LIMIT removal for pagination - remove LIMIT clause if present
                    let mut clean_query = query.clone();
                    if let Some(limit_pos) = clean_query.to_uppercase().rfind("LIMIT") {
                        // Find the end of the LIMIT clause (look for semicolon or end of string)
                        if let Some(semicolon_pos) = clean_query[limit_pos..].find(';') {
                            clean_query = format!("{}{}", &clean_query[..limit_pos].trim(), &clean_query[limit_pos + semicolon_pos..]);
                        } else {
                            clean_query = clean_query[..limit_pos].trim().to_string();
                        }
                    }
                    clean_query
                } else {
                    String::new()
                };
                tabular.current_base_query = base_query_for_pagination.clone();
                debug!("📝 Set base_query for pagination: '{}'", base_query_for_pagination);
                
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
