//! Experimental custom editor widget (Phase A skeleton).
//! Renders plain text from `EditorBuffer` and applies simple insert/backspace/newline.
//! Multi-cursor support (uniform insert/backspace) reused from `MultiSelection`.

use eframe::egui;
use egui::Ui;

use crate::{
    editor_buffer::EditorBuffer,
    editor_selection::MultiSelection,
    syntax::{LanguageKind, highlight_line},
};

#[derive(Default)]
pub struct EditorViewState {
    pub scroll_y: f32,
    pub scroll_x: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub line_height: f32,
    pub desired_caret_x: Option<f32>,
}

#[derive(Default)]
pub struct EditorWidgetState {
    pub view: EditorViewState,
    pub show_line_numbers: bool,
    // Inline-find (ported from Lapce behavior): when active, next typed text is used to jump
    pub inline_find: Option<InlineFindDirection>,
    pub last_inline_find: Option<(InlineFindDirection, String)>,
    // Snippet state: list of (tab_index, (start,end)) and current index
    pub snippet_placeholders: Vec<(usize, (usize, usize))>,
    pub snippet_current: Option<usize>,
}

impl EditorWidgetState {
    pub fn new() -> Self {
        Self {
            view: EditorViewState::default(),
            show_line_numbers: true,
            inline_find: None,
            last_inline_find: None,
            snippet_placeholders: Vec::new(),
            snippet_current: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlineFindDirection {
    Left,
    Right,
}

#[derive(Default)]
pub struct EditorSignals {
    pub caret_moved: bool,
    pub text_changed: bool,
    pub inserted_char: Option<char>,
}

/// Phase A show function – minimal feature set.
pub fn show(
    ui: &mut Ui,
    state: &mut EditorWidgetState,
    buffer: &mut EditorBuffer,
    selection: &mut MultiSelection,
    lang: LanguageKind,
    dark: bool,
    line_cache: &mut std::collections::HashMap<(usize, u64), egui::text::LayoutJob>,
    _current_revision: u64, // deprecated param retained for compatibility; will use buffer.revision
) -> EditorSignals {
    let mut signals = EditorSignals::default();
    if selection.to_lapce_selection().is_empty() {
        selection.ensure_primary(0);
    }

    // --- Layout & interaction base ---
    let available = ui.available_rect_before_wrap();
    state.view.viewport_w = available.width();
    state.view.viewport_h = available.height();
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    state.view.line_height = line_height;
    let id = ui.make_persistent_id("custom_editor_phase_a");

    // --- Gather input BEFORE painting so edits show immediately ---
    let mut inserted_batch = String::new();
    let mut backspace = false;
    let mut move_left = false;
    let mut move_right = false;
    let mut move_up = false;
    let mut move_down = false;
    let mut move_word_left = false;
    let mut move_word_right = false;
    let mut shift = false;
    let mut undo_cmd = false;
    let mut redo_cmd = false;
    let mut tab_pressed = false;
    let mut shift_tab_pressed = false;
    let mut esc_pressed = false;
    // Inline-find ephemeral inputs
    let mut inline_find_pattern: Option<(InlineFindDirection, String)> = None;
    let mut repeat_inline_find = false;

    ui.input(|i| {
        for ev in &i.events {
            match ev {
                egui::Event::Text(t) => {
                    // Intercept inline-find input: do not insert into buffer.
                    if let Some(dir) = state.inline_find {
                        if !t.is_empty() {
                            inline_find_pattern = Some((dir, t.clone()));
                        }
                    } else if !t.chars().all(|c| c < ' ' && c != '\t') {
                        inserted_batch.push_str(t);
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Enter,
                    pressed: true,
                    ..
                } => {
                    inserted_batch.push('\n');
                }
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    backspace = true;
                }
                egui::Event::Key { key: egui::Key::Tab, pressed: true, modifiers, .. } => {
                    if modifiers.shift { shift_tab_pressed = true; } else { tab_pressed = true; }
                }
                // Inline-find commands: Alt+F (right), Alt+Shift+F (left), Alt+G (repeat)
                egui::Event::Key { key: egui::Key::F, pressed: true, modifiers, .. } => {
                    if modifiers.alt && !modifiers.shift { state.inline_find = Some(InlineFindDirection::Right); }
                    else if modifiers.alt && modifiers.shift { state.inline_find = Some(InlineFindDirection::Left); }
                }
                egui::Event::Key { key: egui::Key::G, pressed: true, modifiers, .. } => {
                    if modifiers.alt { repeat_inline_find = true; }
                }
                // Multi-cursor:
                // - Alt+D -> SelectNextCurrent
                // - Alt+Shift+D -> SelectSkipCurrent
                // - Cmd+D (macOS) -> SelectNextCurrent
                // - Cmd+Shift+D (macOS) -> SelectSkipCurrent
                egui::Event::Key { key: egui::Key::D, pressed: true, modifiers, .. } => {
                    let is_next = (modifiers.alt && !modifiers.shift)
                        || ((modifiers.command || modifiers.mac_cmd) && !modifiers.shift);
                    let is_skip = (modifiers.alt && modifiers.shift)
                        || ((modifiers.command || modifiers.mac_cmd) && modifiers.shift);
                    if is_next {
                        select_next_current(&buffer.text, selection);
                    } else if is_skip {
                        select_skip_current(&buffer.text, selection);
                    }
                }
                egui::Event::Key {
                    key: egui::Key::ArrowLeft,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.alt || modifiers.mac_cmd { // mac uses Option as Alt, but mac_cmd is Command; use alt primarily
                        move_word_left = true;
                    } else {
                        move_left = true;
                    }
                    shift = shift || modifiers.shift;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowRight,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.alt || modifiers.mac_cmd {
                        move_word_right = true;
                    } else {
                        move_right = true;
                    }
                    shift = shift || modifiers.shift;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowUp,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    move_up = true;
                    shift = shift || modifiers.shift;
                }
                egui::Event::Key {
                    key: egui::Key::ArrowDown,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    move_down = true;
                    shift = shift || modifiers.shift;
                }
                egui::Event::Key {
                    key: egui::Key::Z,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Cmd/Ctrl+Z -> undo ; Shift+Cmd/Ctrl+Z -> redo
                    if modifiers.command || modifiers.ctrl {
                        // command covers mac_cmd on macOS
                        if modifiers.shift {
                            redo_cmd = true;
                        } else {
                            undo_cmd = true;
                        }
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Y,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Ctrl+Y often redo on Windows/Linux
                    if modifiers.command || modifiers.ctrl {
                        redo_cmd = true;
                    }
                }
                egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => {
                    esc_pressed = true;
                }
                _ => {}
            }
        }
    });

    // Handle repeat inline-find (Alt+G)
    if repeat_inline_find {
        if let Some((dir, pat)) = state.last_inline_find.clone() {
            inline_find_pattern = Some((dir, pat));
        }
    }

    // Execute inline-find if any pattern captured; consumes inline-find mode
    if let Some((dir, pattern)) = inline_find_pattern.take() {
        if !pattern.is_empty() {
            if let Some((_anchor, head)) = selection.primary_range() {
                if let Some(new_head) = inline_find_in_line(buffer, head, dir, &pattern) {
                    selection.set_primary_range(new_head, new_head);
                    signals.caret_moved = true;
                    state.last_inline_find = Some((dir, pattern.clone()));
                }
            }
        }
        // always exit inline-find mode after a key
        state.inline_find = None;
    }

    // Snippet placeholder navigation: Tab / Shift+Tab
    if tab_pressed || shift_tab_pressed {
        if let Some(idx) = state.snippet_current {
            if !state.snippet_placeholders.is_empty() {
                let mut new_idx = idx as isize + if tab_pressed { 1 } else { -1 };
                let last = (state.snippet_placeholders.len() - 1) as isize;
                if new_idx < 0 { new_idx = 0; }
                if new_idx > last { new_idx = last; }
                state.snippet_current = Some(new_idx as usize);
                if let Some((_, (s, e))) = state.snippet_placeholders.get(new_idx as usize) {
                    selection.set_primary_range(*s, *e);
                    signals.caret_moved = true;
                }
                // If on last placeholder and Tab, clear snippet state
                if tab_pressed && (new_idx as usize) == last as usize {
                    state.snippet_placeholders.clear();
                    state.snippet_current = None;
                }
            } else {
                state.snippet_current = None;
            }
        }
    }

    // ESC: cancel multi-cursor and ephemeral modes
    if esc_pressed {
        // Reduce selection to a single primary caret at its current head position
        if let Some((_a, h)) = selection.primary_range() {
            selection.set_primary_range(h, h);
        } else {
            selection.ensure_primary(0);
        }
        // Clear additional regions beyond primary
        let mut primary_only = lapce_core::selection::Selection::new();
        if let Some((a, h)) = selection.primary_range() {
            primary_only.add_region(lapce_core::selection::SelRegion::new(a.min(h), a.max(h), None));
        }
        selection.set_from_lapce_selection(primary_only);
        // Clear inline-find and snippet modes
        state.inline_find = None;
        state.last_inline_find = None;
        state.snippet_placeholders.clear();
        state.snippet_current = None;
        signals.caret_moved = true;
    }

    // --- Text mutations ---
    if undo_cmd {
        if buffer.undo() {
            signals.text_changed = true;
        }
    } else if redo_cmd {
        if buffer.redo() {
            signals.text_changed = true;
        }
    } else if !inserted_batch.is_empty() {
        // Use lapce-core granular edits for multi-caret uniform insert
        let positions = selection.caret_positions();
        if !positions.is_empty() {
            // Apply edits from right to left to keep indices stable
            for &pos in positions.iter().rev() {
                buffer.apply_single_replace(pos..pos, &inserted_batch);
            }
            // Update caret/anchor positions based on original positions
            let len = inserted_batch.len();
            for &pos in &positions {
                selection.apply_simple_insert(pos, len);
            }
            signals.text_changed = true;
            signals.inserted_char = inserted_batch.chars().last();
        }
    } else if backspace {
        // Multi-caret backspace using granular edits
        let mut positions = selection.caret_positions();
        if !positions.is_empty() {
            positions.sort_unstable();
            // Compute deletions against a snapshot to find char boundaries safely
            let snap = buffer.text.clone();
            let mut performed: Vec<(usize, usize)> = Vec::new(); // (start,len)
            for &pos in &positions {
                if pos == 0 { continue; }
                let mut real_start = pos - 1;
                while real_start > 0 && !snap.is_char_boundary(real_start) { real_start -= 1; }
                let mut real_end = pos;
                while real_end < snap.len() && !snap.is_char_boundary(real_end) { real_end += 1; }
                if real_start < real_end && real_end <= snap.len() {
                    performed.push((real_start, real_end - real_start));
                }
            }
            // Apply deletions from right to left
            performed.sort_by_key(|(s, _)| *s);
            for (start, len) in performed.iter().rev() {
                let s = *start; let e = s + *len;
                buffer.apply_single_replace(s..e, "");
            }
            // Update selection from last deletion to first (to handle shifts correctly)
            for (start, len) in performed.into_iter().rev() {
                selection.apply_simple_delete(start, len);
            }
            signals.text_changed = true;
        }
    }
    // No full clear; stale entries become unreachable because line_version changes.
    // Periodic pruning to avoid unbounded growth.
    if signals.text_changed && line_cache.len() > 10_000 {
        line_cache.retain(|(idx, ver), _| *ver == buffer.line_version(*idx));
    }

    // --- Movement: apply to all carets/selections ---
    if move_left || move_right || move_word_left || move_word_right || move_up || move_down {
        // Helper closures for word boundaries (alnum or underscore as word)
        let word_start = |s: &str, mut pos: usize| {
            let b = s.as_bytes();
            pos = pos.min(b.len());
            while pos > 0 && !s.is_char_boundary(pos) { pos -= 1; }
            while pos > 0 {
                let ch = b[pos - 1] as char;
                if ch.is_alphanumeric() || ch == '_' { pos -= 1; } else { break; }
            }
            pos
        };
        let word_end = |s: &str, mut pos: usize| {
            let b = s.as_bytes();
            pos = pos.min(b.len());
            while pos < b.len() && !s.is_char_boundary(pos) { pos += 1; }
            while pos < b.len() {
                let ch = b[pos] as char;
                if ch.is_alphanumeric() || ch == '_' { pos += 1; } else { break; }
            }
            pos
        };

        let sel0 = selection.to_lapce_selection();
        let mut moved_any = false;
        let mut new_sel = lapce_core::selection::Selection::new();
        for r in sel0.regions() {
            let mut anchor = r.min();
            let mut head = r.max();
            let was_range = anchor != head;

            if move_word_left {
                if was_range && !shift {
                    // collapse to left edge
                    head = anchor;
                } else {
                    head = word_start(&buffer.text, head);
                }
                if !shift { anchor = head; }
            } else if move_left {
                if was_range && !shift {
                    head = anchor; // collapse to left
                } else {
                    if head > 0 {
                        head -= 1;
                        while head > 0 && !buffer.text.is_char_boundary(head) { head -= 1; }
                    }
                }
                if !shift { anchor = head; }
            }

            if move_word_right {
                if was_range && !shift {
                    head = r.max(); // collapse to right edge
                } else {
                    head = word_end(&buffer.text, head).min(buffer.text.len());
                }
                if !shift { anchor = head; }
            } else if move_right {
                if was_range && !shift {
                    head = r.max(); // collapse to right
                } else if head < buffer.text.len() {
                    head += 1;
                    while head < buffer.text.len() && !buffer.text.is_char_boundary(head) { head += 1; }
                }
                if !shift { anchor = head; }
            }

            if move_up || move_down {
                // Use cached offset translation
                let (line_idx, col) = buffer.offset_to_line_col(head);
                let target_line = if move_up { line_idx.saturating_sub(1) } else { line_idx + 1 };
                if target_line < buffer.line_count() {
                    let start = buffer.line_start(target_line);
                    let end = if target_line + 1 < buffer.line_count() {
                        buffer.line_start(target_line + 1) - 1
                    } else {
                        buffer.text.len()
                    };
                    let new_col = col.min(end.saturating_sub(start));
                    let new_pos = start + new_col;
                    head = new_pos.min(buffer.text.len());
                } else if move_down {
                    // beyond last line
                    head = buffer.text.len();
                }
                if !shift { anchor = head; }
            }

            if head != r.max() {
                moved_any = true;
            }
            new_sel.add_region(lapce_core::selection::SelRegion::new(anchor, head, None));
        }
        if moved_any {
            signals.caret_moved = true;
        }
        selection.set_from_lapce_selection(new_sel);
    }

    // Selection is directly backed by lapce-core; no resync needed

    // --- After movement, ensure visible (primitive scroll) ---
    if let Some((_anchor, head)) = selection.primary_range() {
        // compute logical line/col without extra allocation
        let (line_idx, col) = buffer.offset_to_line_col(head.min(buffer.text.len()));
        let char_w =
            ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), 'M'));
        let caret_x = (col as f32) * char_w;
        let caret_y = (line_idx as f32) * line_height;
        let margin = 8.0;
        // vertical
        if caret_y < state.view.scroll_y {
            state.view.scroll_y = caret_y.saturating_sub_f32(margin);
        } else if caret_y + line_height > state.view.scroll_y + state.view.viewport_h {
            state.view.scroll_y = (caret_y + line_height) - state.view.viewport_h + margin;
        }
        // horizontal
        if caret_x < state.view.scroll_x {
            state.view.scroll_x = (caret_x - margin).max(0.0);
        } else if caret_x + char_w > state.view.scroll_x + state.view.viewport_w {
            state.view.scroll_x = caret_x + char_w - state.view.viewport_w + margin;
        }
    }

    // --- Recompute size & paint ---
    let line_count = buffer.line_count().max(1);
    let desired_h = line_height * line_count as f32 + line_height * 2.0;
    // Allocate interactive area for the editor and obtain response for mouse focus/clicks
    let (rect, response) = ui.allocate_at_least(
        egui::vec2(available.width(), desired_h),
        egui::Sense::click_and_drag(),
    );
    if response.has_focus() || response.clicked() {
        ui.memory_mut(|m| m.request_focus(id));
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

    let gutter_w = if state.show_line_numbers {
        8.0 * (line_count.to_string().len() as f32 + 1.0)
    } else {
        0.0
    };
    let text_origin = egui::pos2(
        rect.left() + gutter_w + 6.0 - state.view.scroll_x,
        rect.top() + 4.0 - state.view.scroll_y,
    );
    let char_w = ui.fonts(|f| f.glyph_width(&egui::TextStyle::Monospace.resolve(ui.style()), 'M'));
    let lines_vec = build_lines(&buffer.text); // Could be replaced by cached indices; keep for now (Stage2 optimization)

    // Selection highlighting via lapce-core selection regions
    let sel_color = ui.visuals().selection.bg_fill;
    let regions = selection.to_lapce_selection();
    for r in regions.regions() {
        let start = r.min();
        let end = r.max();
        if start == end { continue; }
        highlight_range(
            painter,
            &buffer.text,
            &lines_vec,
            start,
            end,
            text_origin,
            line_height,
            char_w,
            sel_color,
            state.show_line_numbers,
            gutter_w,
        );
    }

    // Handle mouse click: move caret to clicked position (single-click)
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            // Translate screen pos to text grid
            let rel_x = pos.x - text_origin.x;
            let rel_y = pos.y - text_origin.y;
            if rel_y >= 0.0 {
                let line_idx = (rel_y / line_height).floor() as usize;
                if line_idx < buffer.line_count() {
                    // Determine column using monospaced char width
                    let mut col = if char_w > 0.0 { (rel_x / char_w).floor().max(0.0) as usize } else { 0 };
                    let line_start = buffer.line_start(line_idx);
                    let line_end = if line_idx + 1 < buffer.line_count() {
                        buffer.line_start(line_idx + 1) - 1
                    } else {
                        buffer.text.len()
                    };
                    let line_len = line_end.saturating_sub(line_start);
                    if col > line_len { col = line_len; }
                    let new_pos = (line_start + col).min(buffer.text.len());
                    selection.set_primary_range(new_pos, new_pos);
                }
            }
        }
    }

    // Paint lines (with syntax highlight)
    let mut y = text_origin.y;
    for (idx, (lstart, lend)) in lines_vec.iter().enumerate() {
        let line_str = &buffer.text[*lstart..*lend];
        if state.show_line_numbers {
            painter.text(
                egui::pos2(rect.left() + 4.0, y),
                egui::Align2::LEFT_TOP,
                (idx + 1).to_string(),
                egui::TextStyle::Monospace.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }
        // Use per-line version for more granular caching (fallback to global revision if line_versions not updated)
        let key = (idx, buffer.line_version(idx));
        let job = if let Some(cached) = line_cache.get(&key) {
            cached.clone()
        } else {
            let mut lj = highlight_line(line_str, lang, dark);
            // Set wrapping width large so we don't wrap inside line (horizontal scroll later)
            lj.wrap.max_width = f32::INFINITY;
            line_cache.insert(key, lj.clone());
            lj
        };
        let galley = ui.fonts(|f| f.layout_job(job));
        painter.galley(
            egui::pos2(text_origin.x, y),
            galley,
            ui.visuals().text_color(),
        );
        y += line_height;
    }

    // Draw carets (all)
    let caret_color = egui::Color32::from_rgb(120, 180, 250);
    for r in selection.to_lapce_selection().regions() {
        let head = r.max().min(buffer.text.len());
        let (line_idx, col) = buffer.offset_to_line_col(head);
        let x = text_origin.x + (col as f32) * char_w;
        let y = text_origin.y + (line_idx as f32) * line_height;
        let caret_rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(1.5, line_height));
        painter.rect_filled(caret_rect, 0.0, caret_color);
    }

    signals
}

// --- Helpers ---

// Removed local compute_line_starts/index_to_line_col in favor of buffer cached helpers.

fn build_lines(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, ch) in text.char_indices() {
        if ch == '\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    if start <= text.len() {
        out.push((start, text.len()));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn highlight_range(
    painter: &egui::Painter,
    _text: &str,
    lines: &[(usize, usize)],
    start: usize,
    end: usize,
    origin: egui::Pos2,
    line_height: f32,
    char_w: f32,
    color: egui::Color32,
    _show_line_numbers: bool,
    _gutter_w: f32,
) {
    if start >= end {
        return;
    }
    for (idx, (ls, le)) in lines.iter().enumerate() {
        if *le <= start {
            continue;
        }
        if *ls >= end {
            break;
        }
        let seg_start = start.max(*ls);
        let seg_end = end.min(*le);
        if seg_start >= seg_end {
            continue;
        }
        let col_start = seg_start - *ls;
        let col_end = seg_end - *ls;
        let x0 = origin.x + (col_start as f32) * char_w;
        let width = (col_end - col_start) as f32 * char_w;
        let y = origin.y + (idx as f32) * line_height;
        let rect =
            egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(width.max(1.0), line_height));
        painter.rect_filled(rect, 0.0, color.gamma_multiply(0.8));
    }
}

// Utility for f32 saturating subtraction
trait SaturatingSubF32 {
    fn saturating_sub_f32(self, other: f32) -> f32;
}
impl SaturatingSubF32 for f32 {
    fn saturating_sub_f32(self, other: f32) -> f32 {
        if self > other { self - other } else { 0.0 }
    }
}

fn inline_find_in_line(
    buffer: &EditorBuffer,
    head: usize,
    dir: InlineFindDirection,
    pattern: &str,
) -> Option<usize> {
    // Locate current line bounds (excluding trailing newline)
    let (line_idx, _col) = buffer.offset_to_line_col(head.min(buffer.text.len()));
    let start = buffer.line_start(line_idx);
    let end_exclusive = if line_idx + 1 < buffer.line_count() {
        buffer.line_start(line_idx + 1) - 1
    } else {
        buffer.text.len()
    };
    if start >= end_exclusive || start >= buffer.text.len() || end_exclusive > buffer.text.len() {
        return None;
    }
    let line = &buffer.text[start..end_exclusive];

    // Compute current index within the line
    let idx_in_line = head.saturating_sub(start).min(line.len());
    // Helper to move to next char boundary (strictly after current position)
    let next_boundary = |s: &str, mut pos: usize| {
        let b = s.as_bytes();
        pos = pos.min(b.len());
        if pos < b.len() {
            pos += 1;
            while pos < b.len() && !s.is_char_boundary(pos) { pos += 1; }
        }
        pos
    };

    match dir {
        InlineFindDirection::Left => {
            let hay = &line[..idx_in_line];
            if let Some(pos) = hay.rfind(pattern) {
                return Some(start + pos);
            }
        }
        InlineFindDirection::Right => {
            let start_search = next_boundary(line, idx_in_line);
            if start_search <= line.len() {
                let hay = &line[start_search..];
                if let Some(rel) = hay.find(pattern) {
                    return Some(start + start_search + rel);
                }
            }
        }
    }
    None
}

// --- Snippet minimal support ---

/// Apply a simple VS Code-style snippet into the current selection (first region),
/// supporting ${n[:default]}, $n and $0 placeholders. Returns placeholder ranges in new text.
pub fn apply_snippet(
    buffer: &mut EditorBuffer,
    selection: &mut MultiSelection,
    snippet: &str,
) -> Vec<(usize, (usize, usize))> {
    // Determine target range: if selection non-empty, replace; else insert at caret.
    let (anchor, head) = selection
        .primary_range()
        .map(|(a, h)| (a.min(h), a.max(h)))
        .unwrap_or((0, 0));
    let start = anchor;
    let end = head;

    let (expanded, placeholders) = parse_snippet(snippet);
    buffer.apply_single_replace(start..end, &expanded);
    // Adjust placeholder offsets by start
    placeholders
        .into_iter()
        .map(|(idx, (s, e))| (idx, (start + s, start + e)))
        .collect()
}

/// Very small snippet parser: handles $n, ${n}, ${n:default}, and $0.
fn parse_snippet(input: &str) -> (String, Vec<(usize, (usize, usize))>) {
    let mut out = String::with_capacity(input.len());
    let mut placeholders: Vec<(usize, (usize, usize))> = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            // Try $0 or $n or ${...}
            if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                let n = (bytes[i + 1] - b'0') as usize;
                let s = out.len();
                let e = s; // empty placeholder text
                placeholders.push((n, (s, e)));
                i += 2;
                continue;
            } else if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                // ${...}
                let mut j = i + 2;
                while j < bytes.len() && bytes[j] != b'}' { j += 1; }
                if j < bytes.len() && bytes[j] == b'}' {
                    let inner = &input[i + 2..j];
                    // inner like: n or n:default
                    if let Some((n_str, default)) = inner.split_once(':') {
                        if let Ok(n) = n_str.parse::<usize>() {
                            let s = out.len();
                            out.push_str(default);
                            let e = out.len();
                            placeholders.push((n, (s, e)));
                            i = j + 1;
                            continue;
                        }
                    } else if inner == "0" {
                        // final position
                        let pos = out.len();
                        placeholders.push((0, (pos, pos)));
                        i = j + 1;
                        continue;
                    } else if let Ok(n) = inner.parse::<usize>() {
                        let s = out.len();
                        let e = s; // empty placeholder
                        placeholders.push((n, (s, e)));
                        i = j + 1;
                        continue;
                    }
                }
            }
        }
        // Fallback: copy literal char
        out.push(bytes[i] as char);
        i += 1;
    }
    // Sort placeholders by index (n), keep insertion order within same n
    placeholders.sort_by_key(|(n, _)| *n);
    (out, placeholders)
}

// --- Multi-cursor helpers ---

fn select_word_at(text: &str, pos: usize) -> (usize, usize) {
    let b = text.as_bytes();
    let mut s = pos.min(b.len());
    while s > 0 && !text.is_char_boundary(s) { s -= 1; }
    let mut e = s;
    while e < b.len() && !text.is_char_boundary(e) { e += 1; }
    while s > 0 {
        let ch = b[s - 1] as char;
        if ch.is_alphanumeric() || ch == '_' { s -= 1; } else { break; }
    }
    while e < b.len() {
        let ch = b[e] as char;
        if ch.is_alphanumeric() || ch == '_' { e += 1; } else { break; }
    }
    (s, e)
}

fn select_next_current(text: &str, selection: &mut MultiSelection) {
    let mut sel = selection.to_lapce_selection();
    // Fallback to the last region if last_inserted is not tracked
    let base_region = sel
        .last_inserted()
        .cloned()
        .or_else(|| sel.regions().last().cloned());
    let Some(r) = base_region else { return; };
    let (start, end) = if r.is_caret() { select_word_at(text, r.start) } else { (r.min(), r.max()) };
    let needle = &text[start..end];
    if needle.is_empty() { return; }
    if r.is_caret() {
        // First press: turn caret into selection for current word
        let mut regions = sel.regions().to_vec();
        if let Some(last) = regions.last_mut() {
            *last = lapce_core::selection::SelRegion::new(start, end, None);
        }
        let mut new_sel = lapce_core::selection::Selection::new();
        for rr in regions { new_sel.add_region(rr); }
        selection.set_from_lapce_selection(new_sel);
        return;
    }
    // Subsequent press: add next occurrence after the last selection
    if let Some(pos) = text[end..].find(needle) {
        let s = end + pos;
        let e = s + needle.len();
        sel.add_region(lapce_core::selection::SelRegion::new(s, e, None));
        selection.set_from_lapce_selection(sel);
    }
}

fn select_skip_current(text: &str, selection: &mut MultiSelection) {
    let sel0 = selection.to_lapce_selection();
    let mut regions = sel0.regions().to_vec();
    let Some(cur) = regions.last().cloned() else { return; };
    let (start, end) = if cur.is_caret() { select_word_at(text, cur.start) } else { (cur.min(), cur.max()) };
    let needle = &text[start..end];
    if needle.is_empty() { return; }

    if let Some(pos) = text[end..].find(needle) {
        let s = end + pos;
        let e = s + needle.len();
        // Replace last region with new one
        regions.pop();
        regions.push(lapce_core::selection::SelRegion::new(s, e, None));
        let mut new_sel = lapce_core::selection::Selection::new();
        for r in regions { new_sel.add_region(r); }
        selection.set_from_lapce_selection(new_sel);
    }
}
