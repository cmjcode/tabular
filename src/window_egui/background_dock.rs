//! Panel "Background Processes" di bagian bawah sidebar kiri. Tampil di semua
//! tab sidebar, tingginya 20% dari tinggi jendela, dan bisa dilipat ke bawah
//! sampai tersisa baris judulnya.
//!
//! Isinya dibaca dari [`BackgroundTasks`]; tombol di sini hanya menulis
//! permintaan yang lalu dibaca pemilik jendela progress masing-masing.

use eframe::egui;

use super::background_tasks::{BackgroundTask, BackgroundTasks, TaskId, TaskStatus};
use super::style;

/// Tinggi baris judul; ini juga tinggi panel saat dilipat.
const HEADER_HEIGHT: f32 = 28.0;
/// Bagian tinggi jendela yang dipakai panel saat terbuka.
const HEIGHT_FRACTION: f32 = 0.20;
/// Tinggi minimum saat terbuka supaya satu baris proses tetap terbaca.
const MIN_OPEN_HEIGHT: f32 = 110.0;

fn collapsed_id() -> egui::Id {
    egui::Id::new("sidebar_background_dock_collapsed")
}

/// Tinggi panel untuk jendela setinggi `window_height`.
pub fn dock_height(window_height: f32, collapsed: bool) -> f32 {
    if collapsed {
        HEADER_HEIGHT
    } else {
        (window_height * HEIGHT_FRACTION).max(MIN_OPEN_HEIGHT)
    }
}

enum DockAction {
    Show(TaskId),
    Cancel(TaskId),
    Dismiss(TaskId),
    DismissFinished,
}

/// Gambar panel di dasar `ui` (isi sidebar). Panggil sebelum isi sidebar
/// lainnya supaya ruangnya dipesan lebih dulu.
pub fn render_background_dock(registry: &mut BackgroundTasks, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut collapsed: bool = ctx
        .data_mut(|d| d.get_persisted(collapsed_id()))
        .unwrap_or(true);
    // Proses yang baru dikirim ke background langsung terlihat.
    if registry.take_expand_request() && collapsed {
        collapsed = false;
        ctx.data_mut(|d| d.insert_persisted(collapsed_id(), collapsed));
    }
    let height = dock_height(ctx.content_rect().height(), collapsed);
    let mut toggle = false;
    let mut actions: Vec<DockAction> = Vec::new();
    let tasks = &*registry;

    egui::Panel::bottom("sidebar_background_dock")
        .resizable(false)
        .show_separator_line(false)
        .exact_size(height)
        .frame(egui::Frame::NONE.fill(style::nav_surface(&ctx)))
        .show(ui, |ui| {
            ui.painter().hline(
                ui.max_rect().x_range(),
                ui.max_rect().top() + 0.5,
                egui::Stroke::new(1.0, style::nav_border(&ctx)),
            );
            render_header(ui, tasks, collapsed, &mut toggle, &mut actions);
            if collapsed {
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("sidebar_background_dock_list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if tasks.is_empty() {
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("No background processes")
                                .small()
                                .color(style::nav_text_muted(&ctx)),
                        );
                        return;
                    }
                    let now = std::time::Instant::now();
                    // Yang terbaru di atas.
                    for task in tasks.tasks().iter().rev() {
                        render_task_row(ui, task, now, &mut actions);
                    }
                });
        });

    if toggle {
        ctx.data_mut(|d| d.insert_persisted(collapsed_id(), !collapsed));
    }
    for action in actions {
        match action {
            DockAction::Show(id) => registry.request_show(id),
            DockAction::Cancel(id) => registry.request_cancel(id),
            DockAction::Dismiss(id) => registry.dismiss(id),
            DockAction::DismissFinished => registry.dismiss_finished(),
        }
    }
    if registry.running_count() > 0 {
        // Durasi di baris status terus bertambah.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn render_header(
    ui: &mut egui::Ui,
    tasks: &BackgroundTasks,
    collapsed: bool,
    toggle: &mut bool,
    actions: &mut Vec<DockAction>,
) {
    use egui_icons::icons as i;
    let ctx = ui.ctx().clone();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADER_HEIGHT),
        egui::Sense::click(),
    );
    let response = response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if collapsed {
            "Show background processes"
        } else {
            "Hide background processes"
        });
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, style::nav_track(&ctx));
    }
    if response.clicked() {
        *toggle = true;
    }
    let running = tasks.running_count();
    let finished = tasks.tasks().len() - running;
    let mut header_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(4.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut header_ui;
    let chevron = if collapsed {
        i::ICON_KEYBOARD_ARROW_UP
    } else {
        i::ICON_KEYBOARD_ARROW_DOWN
    };
    // Label pasif supaya klik jatuh ke area judul di bawahnya.
    let passive = |text: egui::RichText| egui::Label::new(text).selectable(false);
    ui.add(passive(
        egui::RichText::new(chevron.codepoint)
            .size(15.0)
            .color(style::nav_text_muted(&ctx)),
    ));
    ui.add(passive(
        egui::RichText::new("Background Processes")
            .size(12.0)
            .strong()
            .color(style::nav_text_strong(&ctx)),
    ));
    if running > 0 {
        ui.add(egui::Spinner::new().size(11.0));
        ui.add(passive(
            egui::RichText::new(format!("{running} running"))
                .size(11.0)
                .color(style::nav_text_muted(&ctx)),
        ));
    } else if finished > 0 {
        ui.add(passive(
            egui::RichText::new(format!("{finished} finished"))
                .size(11.0)
                .color(style::nav_text_muted(&ctx)),
        ));
    }
    if !collapsed && finished > 0 {
        // Tombol digambar di atas area klik judul, jadi kliknya tidak melipat panel.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("Clear").size(11.0)).small())
                .on_hover_text("Remove finished processes from this list")
                .clicked()
            {
                actions.push(DockAction::DismissFinished);
            }
        });
    }
}

fn render_task_row(
    ui: &mut egui::Ui,
    task: &BackgroundTask,
    now: std::time::Instant,
    actions: &mut Vec<DockAction>,
) {
    use egui_icons::icons as i;
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.allocate_ui_with_layout(
            egui::vec2(18.0, 30.0),
            egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
            |ui| match task.status {
                TaskStatus::Running => {
                    ui.add(egui::Spinner::new().size(13.0));
                }
                TaskStatus::Done => {
                    ui.label(
                        egui::RichText::new(i::ICON_CHECK_CIRCLE.codepoint)
                            .size(14.0)
                            .color(style::theme_success(&ctx)),
                    );
                }
                TaskStatus::Failed => {
                    ui.label(
                        egui::RichText::new(i::ICON_ERROR.codepoint)
                            .size(14.0)
                            .color(style::theme_danger(&ctx)),
                    );
                }
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if task.is_running() {
                if ui
                    .add_enabled_ui(!task.cancelling, |ui| {
                        style::ai_icon_button(ui, i::ICON_STOP.codepoint, "Cancel this process")
                    })
                    .inner
                    .clicked()
                {
                    actions.push(DockAction::Cancel(task.id));
                }
            } else if style::ai_icon_button(ui, i::ICON_CLOSE.codepoint, "Remove from this list")
                .clicked()
            {
                actions.push(DockAction::Dismiss(task.id));
            }
            let show_tip = if task.is_running() {
                "Show the progress window"
            } else {
                "Show the result"
            };
            if style::ai_icon_button(ui, i::ICON_OPEN_IN_NEW.codepoint, show_tip).clicked() {
                actions.push(DockAction::Show(task.id));
            }
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let title = ui.add(
                    egui::Label::new(
                        egui::RichText::new(&task.title)
                            .size(12.0)
                            .color(style::nav_text_strong(&ctx)),
                    )
                    .truncate(),
                );
                if !task.subtitle.is_empty() {
                    title.on_hover_text(&task.subtitle);
                }
                let status_color = if task.status == TaskStatus::Failed {
                    style::theme_danger(&ctx)
                } else {
                    style::nav_text_muted(&ctx)
                };
                let line = task.status_line(now);
                ui.add(
                    egui::Label::new(egui::RichText::new(&line).size(11.0).color(status_color))
                        .truncate(),
                )
                .on_hover_text(line);
            });
        });
    });
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_dock_takes_a_fifth_of_the_window() {
        assert_eq!(dock_height(1000.0, false), 200.0);
        // Jendela pendek: tetap cukup untuk satu baris proses.
        assert_eq!(dock_height(400.0, false), MIN_OPEN_HEIGHT);
        assert_eq!(dock_height(1000.0, true), HEADER_HEIGHT);
    }
}
