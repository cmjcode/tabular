//! Temporary clean replacement for editor_autocomplete while original is corrupted.
use crate::cache_data::{get_columns_from_cache, get_tables_from_cache};
use crate::query_tools;
use crate::window_egui::Tabular;
use eframe::egui;
use std::collections::HashSet;

const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "DROP", "ALTER", "ADD", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP",
    "BY", "ORDER", "LIMIT", "OFFSET", "AND", "OR", "NOT", "NULL", "AS", "DISTINCT", "COUNT", "SUM",
    "AVG", "MIN", "MAX", "LIKE", "IN", "IS", "BETWEEN", "UNION", "ALL",
];

fn current_prefix(text: &str, cursor: usize) -> (String, usize) {
    if text.is_empty() {
        return (String::new(), cursor);
    }
    let bytes = text.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_alphanumeric() || matches!(c, '_' | ':' | '@' | '$') {
            start -= 1;
        } else {
            break;
        }
    }
    (text[start..cursor.min(text.len())].to_string(), start)
}
fn active_connection_and_db(app: &Tabular) -> Option<(i64, String)> {
    app.query_tabs.get(app.active_tab_index).and_then(|tab| {
        tab.connection_id
            .map(|cid| (cid, tab.database_name.clone().unwrap_or_default()))
    })
}
fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn strip_wrapping_pair(s: &str) -> &str {
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        match (bytes[0], bytes[s.len() - 1]) {
            (b'"', b'"') | (b'`', b'`') | (b'[', b']') => return &s[1..s.len() - 1],
            _ => {}
        }
    }
    s
}

fn parse_table_name(sql: &str, mut idx: usize) -> Option<(usize, String)> {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    while idx < len && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if idx >= len {
        return None;
    }
    if bytes[idx] == b'(' {
        return None;
    }
    let start = idx;
    while idx < len {
        let b = bytes[idx];
        if b.is_ascii_alphanumeric()
            || matches!(b, b'_' | b'.' | b'"' | b'`' | b'[' | b']')
        {
            idx += 1;
        } else {
            break;
        }
    }
    if start == idx {
        return None;
    }
    let mut token = sql[start..idx].trim();
    token = token.trim_end_matches(|c: char| c == ',' || c == ';');
    if token.is_empty() {
        return None;
    }
    let mut final_seg = None;
    for seg in token.split('.') {
        let stripped = strip_wrapping_pair(seg.trim());
        if !stripped.is_empty() {
            final_seg = Some(strip_wrapping_pair(stripped));
        }
    }
    let final_name = final_seg?.trim();
    if final_name.is_empty() {
        return None;
    }
    Some((start, final_name.to_string()))
}

fn collect_table_hits(sql: &str) -> Vec<(usize, String)> {
    let lower = sql.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut hits = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(b"from")
            && (i == 0 || !is_word_char(bytes[i - 1]))
            && (i + 4 >= bytes.len() || !is_word_char(bytes[i + 4]))
        {
            if let Some((pos, name)) = parse_table_name(sql, i + 4) {
                hits.push((pos, name));
            }
            i += 4;
            continue;
        }
        if bytes[i..].starts_with(b"join")
            && (i == 0 || !is_word_char(bytes[i - 1]))
            && (i + 4 >= bytes.len() || !is_word_char(bytes[i + 4]))
        {
            if let Some((pos, name)) = parse_table_name(sql, i + 4) {
                hits.push((pos, name));
            }
            i += 4;
            continue;
        }
        i += 1;
    }
    hits
}

fn tables_near_cursor(sql: &str, cursor: usize) -> Vec<String> {
    let hits = collect_table_hits(sql);
    if hits.is_empty() {
        return Vec::new();
    }
    let cursor = cursor.min(sql.len());
    let mut below: Vec<_> = hits
        .iter()
        .filter(|(pos, _)| *pos >= cursor)
        .cloned()
        .collect();
    below.sort_by_key(|(pos, _)| *pos);
    let mut above: Vec<_> = hits
        .iter()
        .filter(|(pos, _)| *pos < cursor)
        .cloned()
        .collect();
    above.sort_by_key(|(pos, _)| cursor - *pos);
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for (_, name) in below.into_iter().chain(above.into_iter()) {
        if seen.insert(name.clone()) {
            result.push(name);
        }
    }
    result
}

fn extract_tables(sql: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, name) in collect_table_hits(sql) {
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlContext {
    AfterSelect,
    AfterFrom,
    AfterWhere,
    General,
}
fn detect_ctx(sql: &str, cursor: usize) -> SqlContext {
    let slice = &sql[..cursor.min(sql.len())];
    let mut last = None;
    for tok in slice.split_whitespace() {
        match tok.to_ascii_uppercase().as_str() {
            "SELECT" => last = Some(SqlContext::AfterSelect),
            "FROM" | "JOIN" | "LEFT" | "RIGHT" | "INNER" | "OUTER" => {
                last = Some(SqlContext::AfterFrom)
            }
            "WHERE" | "AND" | "OR" | "HAVING" => last = Some(SqlContext::AfterWhere),
            _ => {}
        }
    }
    last.unwrap_or(SqlContext::General)
}
fn get_cached_tables(app: &Tabular, cid: i64, db: &str) -> Option<Vec<String>> {
    let dbs = if db.is_empty() {
        app.database_cache.get(&cid).cloned().unwrap_or_default()
    } else {
        vec![db.to_string()]
    };
    let mut all = Vec::new();
    for d in dbs {
        for tt in ["table", "view"] {
            if let Some(mut ls) = get_tables_from_cache(app, cid, &d, tt) {
                all.append(&mut ls);
            }
        }
    }
    if all.is_empty() {
        None
    } else {
        all.sort_unstable();
        all.dedup();
        Some(all)
    }
}
fn get_all_tables(app: &Tabular) -> Vec<String> {
    let mut all = Vec::new();
    for (cid, dbs) in &app.database_cache {
        for d in dbs {
            for tt in ["table", "view"] {
                if let Some(mut ls) = get_tables_from_cache(app, *cid, d, tt) {
                    all.append(&mut ls);
                }
            }
        }
    }
    all.sort_unstable();
    all.dedup();
    all
}
fn get_cached_columns(
    app: &mut Tabular,
    cid: i64,
    db: &str,
    tables: Vec<String>,
) -> Option<Vec<String>> {
    if tables.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for t in tables {
        if let Some(cols) = get_columns_from_cache(app, cid, db, &t) {
            for (c, _) in cols {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out.sort_unstable();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
fn add_keywords(out: &mut Vec<String>, pref: &str) {
    for kw in SQL_KEYWORDS {
        if kw.to_ascii_lowercase().starts_with(pref) {
            out.push((*kw).to_string());
        }
    }
}

pub fn build_suggestions(
    app: &mut Tabular,
    text: &str,
    cursor: usize,
    prefix: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let pl = prefix.to_ascii_lowercase();
    let ctx = detect_ctx(text, cursor);
    let mut tables_in_scope = tables_near_cursor(text, cursor);
    let tables_all = extract_tables(text);
    if tables_in_scope.is_empty() {
        tables_in_scope = tables_all.clone();
    }
    let conn_id = app
        .query_tabs
        .get(app.active_tab_index)
        .and_then(|t| t.connection_id);
    let db = active_connection_and_db(app)
        .map(|(_, d)| d)
        .unwrap_or_default();
    match ctx {
        SqlContext::AfterSelect => {
            if let Some(cid) = conn_id
                && let Some(cols) = get_cached_columns(app, cid, &db, tables_in_scope.clone())
            {
                for c in cols {
                    if c.to_ascii_lowercase().starts_with(&pl) {
                        out.push(c);
                    }
                }
            }
            if "*".starts_with(&pl) {
                out.push("*".into());
            }
        }
        SqlContext::AfterFrom => {
            if let Some(cid) = conn_id {
                if let Some(ts) = get_cached_tables(app, cid, &db) {
                    for t in ts {
                        if t.to_ascii_lowercase().starts_with(&pl) {
                            out.push(t);
                        }
                    }
                }
            } else {
                for t in get_all_tables(app) {
                    if t.to_ascii_lowercase().starts_with(&pl) {
                        out.push(t);
                    }
                }
            }
        }
        SqlContext::AfterWhere => {
            if let Some(cid) = conn_id
                && let Some(cols) = get_cached_columns(app, cid, &db, tables_in_scope.clone())
            {
                for c in cols {
                    if c.to_ascii_lowercase().starts_with(&pl) {
                        out.push(c);
                    }
                }
            }
        }
        SqlContext::General => {
            add_keywords(&mut out, &pl);
            if let Some(cid) = conn_id {
                if let Some(ts) = get_cached_tables(app, cid, &db) {
                    for t in ts {
                        if t.to_ascii_lowercase().starts_with(&pl) {
                            out.push(t);
                        }
                    }
                }
                if let Some(cols) = get_cached_columns(app, cid, &db, tables_in_scope.clone()) {
                    for c in cols {
                        if c.to_ascii_lowercase().starts_with(&pl) {
                            out.push(c);
                        }
                    }
                }
            } else {
                for t in get_all_tables(app) {
                    if t.to_ascii_lowercase().starts_with(&pl) {
                        out.push(t);
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn update_autocomplete(app: &mut Tabular) {
    // Throttle autocomplete updates to avoid heavy work on every keystroke
    let now = std::time::Instant::now();
    if let Some(last) = app.autocomplete_last_update {
        let elapsed = now.saturating_duration_since(last);
        if elapsed < std::time::Duration::from_millis(app.autocomplete_debounce_ms) {
            return;
        }
    }
    app.autocomplete_last_update = Some(now);
    // Clone editor text first to avoid immutable + mutable borrow overlap
    let editor_text = app.editor.text.clone();
    let cursor = app.cursor_position.min(editor_text.len());
    let (pref, _) = current_prefix(&editor_text, cursor);
    
    // CRITICAL: Don't touch autocomplete state while typing - let text settle first
    // This prevents freeze and caret jumping by avoiding mid-keystroke state mutations
    
    let prev_char = editor_text[..cursor].chars().rev().next();
    if matches!(prev_char, Some(';')) || matches!(prev_char, Some('*')) {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
        app.autocomplete_prefix.clear();
        app.last_autocomplete_trigger_len = 0;
        return;
    }

    if pref.is_empty() {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
        app.autocomplete_prefix.clear();
        app.last_autocomplete_trigger_len = 0;
        return;
    }

    let pre_prefix_char = if pref.len() <= cursor {
        editor_text[..cursor - pref.len()].chars().rev().next()
    } else {
        None
    };
    let triggered_by_space =
        matches!(pre_prefix_char, Some(ch) if ch.is_whitespace());
    let triggered_by_len = pref.len() >= 2;
    if !triggered_by_space && !triggered_by_len {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
        app.autocomplete_prefix.clear();
        app.last_autocomplete_trigger_len = 0;
        return;
    }

    // Only rebuild if prefix length changed (avoid redundant calls)
    if app.last_autocomplete_trigger_len == pref.len() 
        && app.show_autocomplete 
        && app.autocomplete_prefix == pref 
    {
        // Suggestions already up-to-date for this prefix
        return;
    }
    
    app.autocomplete_prefix = pref.clone();

    if app.last_autocomplete_trigger_len != pref.len() || !app.show_autocomplete {
        let suggestions = build_suggestions(app, &editor_text, cursor, &pref);
        if suggestions.is_empty() {
            app.show_autocomplete = false;
            app.autocomplete_payloads.clear();
        } else {
            app.show_autocomplete = true;
            let context = detect_ctx(&editor_text, cursor);
            let (cid, db) = app
                .query_tabs
                .get(app.active_tab_index)
                .and_then(|tab| {
                    tab.connection_id
                        .map(|c| (c, tab.database_name.clone().unwrap_or_default()))
                })
                .unwrap_or((0, String::new()));

            let tables_set: HashSet<String> = if cid != 0 {
                get_cached_tables(app, cid, &db)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            } else {
                get_all_tables(app).into_iter().collect()
            };
            // Reuse tables_in_scope from build_suggestions to avoid redundant cache lookups
            let tables_for_cols = tables_near_cursor(&editor_text, cursor);
            let cols_set: HashSet<String> = if cid != 0 && !tables_for_cols.is_empty() {
                get_cached_columns(app, cid, &db, tables_for_cols)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            } else {
                HashSet::new()
            };
            let syntax_set: HashSet<String> = SQL_KEYWORDS
                .iter()
                .map(|s| s.to_string())
                .chain(std::iter::once("*".to_string()))
                .collect();

            let mut tables = Vec::new();
            let mut columns = Vec::new();
            let mut syntax = Vec::new();
            for s in suggestions.into_iter() {
                if tables_set.contains(&s) {
                    tables.push(s);
                } else if cols_set.contains(&s) {
                    columns.push(s);
                } else if syntax_set.contains(&s) {
                    syntax.push(s);
                } else {
                    match context {
                        SqlContext::AfterFrom => tables.push(s),
                        SqlContext::AfterSelect | SqlContext::AfterWhere => columns.push(s),
                        SqlContext::General => syntax.push(s),
                    }
                }
            }
            tables.sort_unstable();
            tables.dedup();
            columns.sort_unstable();
            columns.dedup();
            syntax.sort_unstable();
            syntax.dedup();

            let mut ordered = Vec::new();
            let mut kinds = Vec::new();
            let mut notes = Vec::new();
            let mut payloads = Vec::new();
            let mut seen_labels: HashSet<String> = HashSet::new();
            let mut push_suggestion = |label: String,
                                       kind: crate::models::enums::AutocompleteKind,
                                       note: Option<String>,
                                       payload: Option<String>| {
                if seen_labels.insert(label.clone()) {
                    ordered.push(label);
                    kinds.push(kind);
                    notes.push(note);
                    payloads.push(payload);
                }
            };

            for t in tables {
                let note = if db.is_empty() {
                    Some("table".to_string())
                } else {
                    Some(format!("db: {}", db))
                };
                push_suggestion(
                    t,
                    crate::models::enums::AutocompleteKind::Table,
                    note,
                    None,
                );
            }

            for c in columns {
                push_suggestion(
                    c,
                    crate::models::enums::AutocompleteKind::Column,
                    Some("column".to_string()),
                    None,
                );
            }

            for param in query_tools::parameter_candidates(&pref) {
                push_suggestion(
                    param.label.to_string(),
                    crate::models::enums::AutocompleteKind::Parameter,
                    Some(param.note.to_string()),
                    Some(param.template.to_string()),
                );
            }

            for kw in syntax {
                let is_wc = kw == "*";
                push_suggestion(
                    kw,
                    crate::models::enums::AutocompleteKind::Syntax,
                    Some(if is_wc {
                        "wildcard".to_string()
                    } else {
                        "keyword".to_string()
                    }),
                    None,
                );
            }

            let snippet_context = match context {
                SqlContext::AfterSelect => query_tools::SnippetContext::SelectList,
                SqlContext::AfterFrom => query_tools::SnippetContext::FromClause,
                SqlContext::AfterWhere => query_tools::SnippetContext::WhereClause,
                SqlContext::General => query_tools::SnippetContext::Any,
            };

            for snippet in query_tools::snippet_candidates(&pref, snippet_context) {
                push_suggestion(
                    snippet.label.to_string(),
                    crate::models::enums::AutocompleteKind::Snippet,
                    Some(snippet.note.to_string()),
                    Some(snippet.template.to_string()),
                );
            }

            app.autocomplete_suggestions = ordered;
            app.autocomplete_kinds = kinds;
            app.autocomplete_notes = notes;
            app.autocomplete_payloads = payloads;
            app.selected_autocomplete_index = 0;
        }
        app.last_autocomplete_trigger_len = pref.len();
    }
}

pub fn accept_current_suggestion(app: &mut Tabular) {
    if !app.show_autocomplete {
        return;
    }
    if let Some(display) = app
        .autocomplete_suggestions
        .get(app.selected_autocomplete_index)
        .cloned()
    {
        let cursor = app.cursor_position.min(app.editor.text.len());
        let (_pref, start) = current_prefix(&app.editor.text, cursor);
        let start_idx = start;
        let replacement = app
            .autocomplete_payloads
            .get(app.selected_autocomplete_index)
            .and_then(|p| p.clone())
            .unwrap_or_else(|| display.clone());
        app.editor
            .apply_single_replace(start_idx..cursor, &replacement);
        app.cursor_position = start_idx + replacement.len();
        app.multi_selection
            .set_primary_range(app.cursor_position, app.cursor_position);
        app.pending_cursor_set = Some(app.cursor_position);
        app.autocomplete_expected_cursor = Some(app.cursor_position);
        app.autocomplete_protection_frames = app.autocomplete_protection_frames.max(8);
        app.editor_focus_boost_frames = app.editor_focus_boost_frames.max(6);
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
    }
}

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

pub fn render_autocomplete(app: &mut Tabular, ui: &mut egui::Ui, pos: egui::Pos2) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let screen = ui.ctx().screen_rect();
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let small_font_id = egui::TextStyle::Small.resolve(ui.style());
    let heading_font_id = egui::TextStyle::Body.resolve(ui.style());
    let suggestions = app.autocomplete_suggestions.clone();
    let kinds = app.autocomplete_kinds.clone();
    let notes = app.autocomplete_notes.clone();
    let mut max_label_px: f32 = 0.0;
    let mut max_note_px: f32 = 0.0;
    let mut max_heading_px: f32 = 0.0;
    let mut note_count = 0usize;
    let mut group_count = 0usize;
    let mut last_kind: Option<crate::models::enums::AutocompleteKind> = None;
    ui.ctx().fonts(|f| {
        for (idx, s) in suggestions.iter().enumerate() {
            let g = f.layout_no_wrap(s.clone(), font_id.clone(), egui::Color32::WHITE);
            max_label_px = max_label_px.max(g.size().x);

            if let Some(Some(note)) = notes.get(idx) {
                let ng = f.layout_no_wrap(note.clone(), small_font_id.clone(), egui::Color32::WHITE);
                max_note_px = max_note_px.max(ng.size().x);
                note_count += 1;
            }

            if let Some(&kind) = kinds.get(idx) {
                if last_kind != Some(kind) {
                    group_count += 1;
                    last_kind = Some(kind);
                    let heading = match kind {
                        crate::models::enums::AutocompleteKind::Table => "Tables",
                        crate::models::enums::AutocompleteKind::Column => "Columns",
                        crate::models::enums::AutocompleteKind::Syntax => "Syntax",
                        crate::models::enums::AutocompleteKind::Snippet => "Snippets",
                        crate::models::enums::AutocompleteKind::Parameter => "Parameters",
                    };
                    let hg =
                        f.layout_no_wrap(heading.to_string(), heading_font_id.clone(), egui::Color32::WHITE);
                    max_heading_px = max_heading_px.max(hg.size().x);
                }
            }
        }
    });

    let base_width = max_label_px.max(max_note_px).max(max_heading_px);
    let popup_w = (base_width + 48.0).clamp(220.0, (screen.width() - 32.0).max(220.0));

    let entry_count = suggestions.len() as f32;
    let mut desired_h = entry_count * 26.0
        + (note_count as f32) * 12.0
        + (group_count as f32) * 20.0;
    if desired_h < 64.0 {
        desired_h = 64.0;
    }
    let screen_h = screen.height();
    let desired_cap = screen_h * 0.65;
    if desired_h > desired_cap {
        desired_h = desired_cap;
    }
    let margin = 8.0;
    let space_below = (screen.bottom() - pos.y - margin).max(0.0);
    let space_above = (pos.y - screen.top() - margin).max(0.0);
    let show_above = space_below + 2.0 < desired_h && space_above > space_below;
    let max_h = desired_h.min(if show_above { space_above } else { space_below });
    let mut popup_pos = pos;
    if show_above {
        popup_pos.y = (pos.y - max_h).max(screen.top());
    }
    if popup_pos.x + popup_w > screen.right() {
        popup_pos.x = (screen.right() - popup_w).max(screen.left());
    }
    egui::Area::new(egui::Id::new("autocomplete_popup"))
        .fixed_pos(popup_pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let bg = ui.style().visuals.window_fill;
            let translucent = egui::Color32::from_rgba_unmultiplied(bg.r(), bg.g(), bg.b(), 210);
            egui::Frame::popup(ui.style())
                .fill(translucent)
                .stroke(egui::Stroke::new(0.5, egui::Color32::from_rgb(255, 30, 0))) // rgba(255, 30, 0, 1)
                .corner_radius(4.0)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_min_width(popup_w);
                    ui.set_max_width(popup_w);
                    ui.set_min_height(max_h);
                    let suggestions = suggestions.clone();
                    let kinds = kinds.clone();
                    let notes = notes.clone();
                    let mut last_kind = None;
                    egui::ScrollArea::vertical()
                        .max_height(max_h - 10.0)
                        .show(ui, |ui| {
                            for (i, s) in suggestions.iter().enumerate() {
                                if let Some(k) = kinds.get(i).copied()
                                    && last_kind != Some(k)
                                {
                                    last_kind = Some(k);
                                    let label = match k {
                                        crate::models::enums::AutocompleteKind::Table => "Tables",
                                        crate::models::enums::AutocompleteKind::Column => "Columns",
                                        crate::models::enums::AutocompleteKind::Syntax => "Syntax",
                                        crate::models::enums::AutocompleteKind::Snippet => "Snippets",
                                        crate::models::enums::AutocompleteKind::Parameter => "Parameters",
                                    };
                                    if i != 0 {
                                        ui.add(egui::Separator::default().spacing(4.0));
                                    }
                                    ui.label(egui::RichText::new(label).strong());
                                }
                                let selected = i == app.selected_autocomplete_index;
                                let mut rt = egui::RichText::new(s.clone());
                                if selected {
                                    rt = rt
                                        .background_color(ui.style().visuals.selection.bg_fill)
                                        .color(ui.style().visuals.selection.stroke.color);
                                }
                                let resp = ui.selectable_label(selected, rt);
                                if let Some(note) = notes.get(i).and_then(|n| n.clone()) {
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new(note).weak().small());
                                }
                                if resp.clicked() {
                                    app.selected_autocomplete_index = i;
                                    accept_current_suggestion(app);
                                    let id = egui::Id::new("sql_editor");
                                    if let Some(mut state) =
                                        egui::text_edit::TextEditState::load(ui.ctx(), id)
                                    {
                                        use egui::text::{CCursor, CCursorRange};
                                        state.cursor.set_char_range(Some(CCursorRange::one(
                                            CCursor::new(app.cursor_position),
                                        )));
                                        state.store(ui.ctx(), id);
                                    }
                                    ui.memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                                    app.editor_focus_boost_frames =
                                        app.editor_focus_boost_frames.max(6);
                                }
                            }
                        });
                });
        });
}

pub fn trigger_manual(app: &mut Tabular) {
    update_autocomplete(app);
    if app.autocomplete_prefix.is_empty() {
        app.autocomplete_suggestions = SQL_KEYWORDS.iter().map(|s| s.to_string()).collect();
        app.autocomplete_suggestions.sort_unstable();
        app.selected_autocomplete_index = 0;
        app.show_autocomplete = true;
        app.autocomplete_kinds = vec![
            crate::models::enums::AutocompleteKind::Syntax;
            app.autocomplete_suggestions.len()
        ];
        app.autocomplete_notes =
            vec![Some("keyword".to_string()); app.autocomplete_suggestions.len()];
        app.autocomplete_payloads = vec![None; app.autocomplete_suggestions.len()];
    } else if app.autocomplete_suggestions.is_empty() {
        app.autocomplete_suggestions = SQL_KEYWORDS
            .iter()
            .filter(|k| {
                k.to_lowercase()
                    .starts_with(&app.autocomplete_prefix.to_ascii_lowercase())
            })
            .map(|s| s.to_string())
            .collect();
        if !app.autocomplete_suggestions.is_empty() {
            app.show_autocomplete = true;
            app.autocomplete_kinds = vec![
                crate::models::enums::AutocompleteKind::Syntax;
                app.autocomplete_suggestions.len()
            ];
            app.autocomplete_notes =
                vec![Some("keyword".to_string()); app.autocomplete_suggestions.len()];
            app.autocomplete_payloads = vec![None; app.autocomplete_suggestions.len()];
        }
    }
}
