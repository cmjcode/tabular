use eframe::egui; 
use crate::{window_egui::Tabular, models};
use log::debug;

// Basic SQL keywords list (extend as needed)
const SQL_KEYWORDS: &[&str] = &[
    "SELECT","FROM","WHERE","INSERT","INTO","VALUES","UPDATE","SET","DELETE","CREATE","TABLE","DROP","ALTER","ADD","JOIN","LEFT","RIGHT","INNER","OUTER","ON","GROUP","BY","ORDER","LIMIT","OFFSET","AND","OR","NOT","NULL","AS","DISTINCT","COUNT","SUM","AVG","MIN","MAX","LIKE","IN","IS","BETWEEN","UNION","ALL" 
];

/// Extract current word prefix before cursor.
fn current_prefix(text: &str, cursor: usize) -> (String, usize) {
    if text.is_empty() { return (String::new(), cursor); }
    let bytes = text.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 {
        let c = bytes[start-1] as char;
        if c.is_alphanumeric() || c == '_' { start -= 1; } else { break; }
    }
    (text[start..cursor.min(text.len())].to_string(), start)
}

/// Build suggestions based on prefix: SQL keywords + table names + column names from active connection tree.
pub fn build_suggestions(app: &Tabular, prefix: &str) -> Vec<String> {
    if prefix.len() < 2 { return Vec::new(); } // minimal length
    let mut out: Vec<String> = Vec::new();
    let low = prefix.to_lowercase();

    // Keywords
    for k in SQL_KEYWORDS { if k.to_lowercase().starts_with(&low) { out.push((*k).to_string()); } }

    // Tables & columns from items_tree (database explorer)
    for node in &app.items_tree { collect_names(node, &low, &mut out); }

    // Remove duplicates while preserving order
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.to_lowercase()));
    out.sort();
    out
}

fn collect_names(node: &models::structs::TreeNode, prefix_low: &str, out: &mut Vec<String>) {
    use crate::models::enums::NodeType;
    match node.node_type() {
        NodeType::Table | NodeType::View => {
            if node.name.to_lowercase().starts_with(prefix_low) { out.push(node.name.clone()); }
        }
        NodeType::Column => {
            if node.name.to_lowercase().starts_with(prefix_low) { out.push(node.name.clone()); }
        }
        _ => {}
    }
    for c in &node.children { collect_names(c, prefix_low, out); }
}

// Provide accessor for node_type since field is crate private in original struct.
trait TreeNodeExt { fn node_type(&self) -> models::enums::NodeType; }
impl TreeNodeExt for models::structs::TreeNode { fn node_type(&self) -> models::enums::NodeType { self.node_type.clone() } }

/// Update autocomplete state after text change or cursor move.
pub fn update_autocomplete(app: &mut Tabular) {
    let cursor = app.cursor_position.min(app.editor_text.len());
    let (prefix, start_idx) = current_prefix(&app.editor_text, cursor);
    app.autocomplete_prefix = prefix.clone();

    if prefix.is_empty() || prefix.len() < 2 { // hide if too short
        app.show_autocomplete = false; 
        app.autocomplete_suggestions.clear();
        return;
    }

    // Only rebuild if prefix length changed or previously hidden
    if app.last_autocomplete_trigger_len != prefix.len() || !app.show_autocomplete {
        let suggestions = build_suggestions(app, &prefix);
        if suggestions.is_empty() {
            app.show_autocomplete = false;
        } else {
            app.show_autocomplete = true;
            app.autocomplete_suggestions = suggestions;
            app.selected_autocomplete_index = 0;
        }
        app.last_autocomplete_trigger_len = prefix.len();
    }

    debug!("Cursor position A {}", app.cursor_position);
    // Store start index in last_autocomplete_trigger_len encoded (optional) - keeping simple
    let _ = start_idx; // could be used later for replacement
}

/// Accept currently selected suggestion and replace text.
pub fn accept_current_suggestion(app: &mut Tabular) {
    if !app.show_autocomplete { return; }
    debug!("Accepting suggestion: {}", app.autocomplete_suggestions[app.selected_autocomplete_index]);
    if let Some(sugg) = app.autocomplete_suggestions.get(app.selected_autocomplete_index) {
        let cursor = app.cursor_position.min(app.editor_text.len());
        let (prefix, start_idx) = current_prefix(&app.editor_text, cursor);
        debug!("Current prefix: '{}', start index: {}", prefix, start_idx);
        // If prefix empty but we still want to accept (e.g., early Tab) try to look back until whitespace
        let (effective_start, effective_prefix_len) = if prefix.is_empty() {
            // Scan backwards for contiguous identifier chars just typed
            let bytes = app.editor_text.as_bytes();
            let mut s = cursor;
            while s>0 { let ch = bytes[s-1] as char; if ch.is_alphanumeric() || ch=='_' { s-=1; } else { break; } }
            (s, cursor - s)
        } else { (start_idx, prefix.len()) };

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
    debug!("Cursor position B {}", app.cursor_position);
    }
}

/// Keyboard navigation for suggestions.
pub fn navigate(app: &mut Tabular, delta: i32) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() { return; }
    let len = app.autocomplete_suggestions.len();
    if delta > 0 { app.selected_autocomplete_index = (app.selected_autocomplete_index + 1) % len; }
    else { if app.selected_autocomplete_index == 0 { app.selected_autocomplete_index = len - 1; } else { app.selected_autocomplete_index -= 1; } }
}

/// Render dropdown near top-right of editor area (simplified positioning). Call after editor.
pub fn render_autocomplete(app: &mut Tabular, ui: &mut egui::Ui, pos: egui::Pos2) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() { return; }
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
    let max_visible = 8usize;
    let visible = app.autocomplete_suggestions.len().min(max_visible);
    let est_height = (visible as f32) * line_height + 8.0;
    let screen = ui.ctx().screen_rect();
    let mut popup_pos = pos;
    if popup_pos.y + est_height > screen.bottom() { popup_pos.y = (popup_pos.y - est_height).max(screen.top()); }
    if popup_pos.x + 250.0 > screen.right() { popup_pos.x = (screen.right() - 250.0).max(screen.left()); }

    egui::Area::new(egui::Id::new("autocomplete_popup"))
        .fixed_pos(popup_pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style())
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for (i, s) in app.autocomplete_suggestions.iter().enumerate() {
                            let selected = i == app.selected_autocomplete_index;
                            let text = if selected { egui::RichText::new(s).background_color(ui.style().visuals.selection.bg_fill).color(ui.style().visuals.selection.stroke.color) } else { egui::RichText::new(s) };
                            if ui.selectable_label(selected, text).clicked() { app.selected_autocomplete_index = i; accept_current_suggestion(app); break; }
                        }
                    });
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
            app.autocomplete_suggestions = SQL_KEYWORDS.iter()
                .filter(|k| k.to_lowercase().starts_with(&app.autocomplete_prefix.to_lowercase()))
                .map(|s| s.to_string()).collect();
            if !app.autocomplete_suggestions.is_empty() { app.show_autocomplete = true; }
        } else {
            app.show_autocomplete = true;
        }
    }
}
