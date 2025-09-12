//! Experimental custom editor widget (Phase A skeleton).
//! Renders plain text from `EditorBuffer` and applies simple insert/backspace/newline.
//! Multi-cursor support (uniform insert/backspace) reused from `MultiSelection`.

use eframe::egui;
use egui::Ui;

use crate::{editor_buffer::EditorBuffer, editor_selection::MultiSelection};

#[derive(Default)]
pub struct EditorViewState {
    pub scroll_y: f32,
    pub scroll_x: f32,
    pub viewport_w: f32,
    pub viewport_h: f32,
    pub line_height: f32,
    pub desired_caret_x: Option<f32>,
}

pub struct EditorWidgetState {
    pub view: EditorViewState,
    pub show_line_numbers: bool,
}

impl EditorWidgetState {
    pub fn new() -> Self {
        Self { view: EditorViewState::default(), show_line_numbers: true }
    }
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
) -> EditorSignals {
    let mut signals = EditorSignals::default();
    let available = ui.available_rect_before_wrap();
    // Basic metrics
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    state.view.line_height = line_height;

    // For now we take full width, height grows with content (rough estimate)
    let line_count = buffer.text.lines().count().max(1); // temporary mirror usage; later rope lines
    let desired_h = line_height * line_count as f32 + line_height * 2.0;
    let rect = egui::Rect::from_min_size(available.min, egui::vec2(available.width(), desired_h));
    let id = ui.make_persistent_id("custom_editor_phase_a");
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());

    // Simple background
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

    let gutter_w = if state.show_line_numbers { 8.0 * (line_count.to_string().len() as f32 + 1.0) } else { 0.0 };
    let text_origin = egui::pos2(rect.left() + gutter_w + 6.0 - state.view.scroll_x, rect.top() + 4.0 - state.view.scroll_y);

    // Paint lines (no highlight yet)
    let mut y = text_origin.y;
    for (idx, line) in buffer.text.lines().enumerate() { // temporary; replace with rope line iteration
        if state.show_line_numbers {
            painter.text(
                egui::pos2(rect.left() + 4.0, y),
                egui::Align2::LEFT_TOP,
                (idx + 1).to_string(),
                egui::TextStyle::Monospace.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );
        }
        painter.text(
            egui::pos2(text_origin.x, y),
            egui::Align2::LEFT_TOP,
            line,
            egui::TextStyle::Monospace.resolve(ui.style()),
            ui.visuals().text_color(),
        );
        y += line_height;
    }

    // TODO: Draw carets from selection (Phase B) – stubbed

    // Handle basic key events (Phase A): insert text chars & Enter & Backspace
    if response.has_focus() || response.clicked() {
        ui.memory_mut(|m| m.request_focus(id));
    }

    let mut inserted_batch = String::new();
    let mut backspace = false;
    ui.input(|i| {
        for ev in &i.events {
            match ev {
                egui::Event::Text(t) => {
                    // Filter control chars except tab (optional)
                    if !t.chars().all(|c| c < ' ' && c != '\t') {
                        inserted_batch.push_str(t);
                    }
                }
                egui::Event::Key { key: egui::Key::Enter, pressed: true, .. } => {
                    inserted_batch.push('\n');
                }
                egui::Event::Key { key: egui::Key::Backspace, pressed: true, .. } => {
                    backspace = true;
                }
                _ => {}
            }
        }
    });

    if !inserted_batch.is_empty() {
        // Apply uniform insert to all carets (currently collapsed assumption)
        selection.apply_insert_text(&mut buffer.text, &inserted_batch); // still using mirror text path
        signals.text_changed = true;
        signals.inserted_char = inserted_batch.chars().last();
    } else if backspace {
        selection.apply_backspace(&mut buffer.text);
        signals.text_changed = true;
    }

    signals
}
