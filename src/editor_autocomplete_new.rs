//! Temporary clean replacement for editor_autocomplete while original is corrupted.
use crate::cache_data::{get_columns_from_cache, get_tables_from_cache};
use crate::window_egui::Tabular;
use eframe::egui;

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
        if c.is_alphanumeric() || c == '_' {
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
fn extract_tables(sql: &str) -> Vec<String> {
    let lower = sql.to_ascii_lowercase();
    if let Some(i) = lower.find(" from ") {
        let after = &sql[i + 6..];
        let mut out = Vec::new();
        for seg in after.split([',', '\n']) {
            let t = seg.trim();
            if t.is_empty() {
                continue;
            }
            let stop = [
                "where", "group", "order", "limit", "offset", "join", "left", "right", "inner",
                "outer",
            ];
            let first = t
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            if stop.contains(&first.as_str()) {
                break;
            }
            let cleaned = t
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches('`')
                .trim_matches('"');
            if cleaned.is_empty() {
                continue;
            }
            let final_name = cleaned.split('.').next_back().unwrap_or(cleaned);
            out.push(final_name.to_string());
        }
        out.sort_unstable();
        out.dedup();
        return out;
    }
    Vec::new()
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
    Some(out)
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
    let tables_in = extract_tables(text);
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
                && let Some(cols) = get_cached_columns(app, cid, &db, tables_in)
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
                && let Some(cols) = get_cached_columns(app, cid, &db, tables_in)
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
                if let Some(cols) = get_cached_columns(app, cid, &db, tables_in) {
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
    // Clone editor text first to avoid immutable + mutable borrow overlap
    let editor_text = app.editor.text.clone();
    let cursor = app.cursor_position.min(editor_text.len());
    let (pref, _) = current_prefix(&editor_text, cursor);
    app.autocomplete_prefix = pref.clone();

    if pref.is_empty() {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        return;
    }

    if app.last_autocomplete_trigger_len != pref.len() || !app.show_autocomplete {
        let suggestions = build_suggestions(app, &editor_text, cursor, &pref);
        if suggestions.is_empty() {
            app.show_autocomplete = false;
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

            use std::collections::HashSet;
            let tables_set: HashSet<String> = if cid != 0 {
                get_cached_tables(app, cid, &db)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            } else {
                get_all_tables(app).into_iter().collect()
            };
            let cols_set: HashSet<String> = if cid != 0 {
                get_cached_columns(app, cid, &db, extract_tables(&editor_text))
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
            for t in tables {
                ordered.push(t);
                kinds.push(crate::models::enums::AutocompleteKind::Table);
                notes.push(Some(if db.is_empty() {
                    "table".into()
                } else {
                    format!("db: {}", db)
                }));
            }
            for c in columns {
                ordered.push(c);
                kinds.push(crate::models::enums::AutocompleteKind::Column);
                notes.push(Some("column".into()));
            }
            for kw in syntax {
                let is_wc = kw == "*";
                ordered.push(kw);
                kinds.push(crate::models::enums::AutocompleteKind::Syntax);
                notes.push(Some(if is_wc {
                    "wildcard".into()
                } else {
                    "keyword".into()
                }));
            }

            app.autocomplete_suggestions = ordered;
            app.autocomplete_kinds = kinds;
            app.autocomplete_notes = notes;
            app.selected_autocomplete_index = 0;
        }
        app.last_autocomplete_trigger_len = pref.len();
    }
}

pub fn accept_current_suggestion(app: &mut Tabular) {
    if !app.show_autocomplete {
        return;
    }
    if let Some(s) = app
        .autocomplete_suggestions
        .get(app.selected_autocomplete_index)
        .cloned()
    {
        let cursor = app.cursor_position.min(app.editor.text.len());
        let (_pref, start) = current_prefix(&app.editor.text, cursor);
        let start_idx = start;
        app.editor.apply_single_replace(start_idx..cursor, &s);
        app.cursor_position = start_idx + s.len();
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
    let mut max_px = 0.0;
    ui.ctx().fonts(|f| {
        for s in &app.autocomplete_suggestions {
            let g = f.layout_no_wrap(s.to_string(), font_id.clone(), egui::Color32::WHITE);
            if g.size().x > max_px {
                max_px = g.size().x;
            }
        }
    });
    let popup_w = (max_px + 32.0).clamp(160.0, (screen.width() * 0.55).clamp(220.0, 600.0));
    let mut desired_h = (app.autocomplete_suggestions.len() as f32) * 12.0;
    let screen_h = screen.height();
    if desired_h > screen_h * 0.6 {
        desired_h = screen_h * 0.5;
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
                    let suggestions = app.autocomplete_suggestions.clone();
                    let kinds = app.autocomplete_kinds.clone();
                    let notes = app.autocomplete_notes.clone();
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
        }
    }
}
