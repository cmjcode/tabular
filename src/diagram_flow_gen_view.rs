//! Jendela progress generate alur bisnis flow card (AI). Job-nya berjalan di
//! `window_egui::diagram_flow_jobs`; jendela ini hanya membaca dan menulis
//! [`FlowGenWindow`] (batal, sembunyikan, tutup).

use eframe::egui;

use crate::diagram_repo::{accent_button, render_job_progress};
use crate::models::structs::{DiagramState, FlowGenWindow};
use crate::window_egui::style;

/// Baris gagal yang ditampilkan sebelum "+n more".
const MAX_FAILED_SHOWN: usize = 20;

/// Teks di bawah judul: jumlah endpoint dan repository yang sedang diproses.
pub fn subtitle(win: &FlowGenWindow) -> String {
    let mut s = format!("{} endpoint(s)", win.card_count);
    if !win.repo_label.is_empty() {
        s.push_str(&format!(" from {}", win.repo_label));
    }
    if win.repo_total > 1 {
        s.push_str(&format!(
            " · repository {} of {}",
            win.repo_index.min(win.repo_total),
            win.repo_total
        ));
    }
    s
}

/// Pilihan dari item menu generate alur bisnis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenMenuPick {
    Generate,
    /// Tampilkan lagi jendela progress yang disembunyikan.
    ShowProgress,
}

/// `true` bila job generate diagram ini masih berjalan.
pub fn is_running(state: &DiagramState) -> bool {
    state.flow_gen.as_ref().is_some_and(|w| w.running)
}

/// Item menu generate alur bisnis. Saat job diagram ini berjalan, item ini
/// berganti menjadi "Show Generation Progress". Tidak tampil di build tanpa
/// proses eksternal (iOS).
pub fn generate_menu_item(
    ui: &mut egui::Ui,
    running: bool,
    enabled: bool,
    regenerate: bool,
    disabled_tip: &str,
) -> Option<GenMenuPick> {
    if cfg!(target_os = "ios") {
        return None;
    }
    if running {
        let clicked = ui
            .button(format!(
                "{} Show Generation Progress",
                egui_icons::icons::ICON_VISIBILITY.codepoint
            ))
            .on_hover_text("A business process generation is running for this diagram")
            .clicked();
        return clicked.then_some(GenMenuPick::ShowProgress);
    }
    let text = if regenerate {
        format!("{} Regenerate", egui_icons::icons::ICON_REFRESH.codepoint)
    } else {
        format!(
            "{} Generate Business Process (AI)",
            egui_icons::icons::ICON_AUTO_AWESOME.codepoint
        )
    };
    let clicked = ui
        .add_enabled(enabled, egui::Button::new(text))
        .on_hover_text("Trace what each endpoint does step by step from the repository code")
        .on_disabled_hover_text(disabled_tip)
        .clicked();
    clicked.then_some(GenMenuPick::Generate)
}

/// Munculkan lagi jendela progress yang disembunyikan.
pub fn show_progress(state: &mut DiagramState) {
    if let Some(win) = state.flow_gen.as_mut() {
        win.hidden = false;
    }
}

/// Gambar jendela progress bila ada dan tidak disembunyikan.
pub fn render_flow_gen_window(ctx: &egui::Context, state: &mut DiagramState) {
    let Some(win) = state.flow_gen.as_mut() else {
        return;
    };
    if win.hidden {
        return;
    }
    let mut close = false;
    style::render_modal_backdrop(ctx, "diagram_flow_gen_backdrop", true);
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(380.0, 620.0);
    let list_h = (screen.height() * 0.4).clamp(160.0, 360.0);

    egui::Window::new("Generate Business Process")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            style::render_modal_header(
                ui,
                format!(
                    "{} Business process: {}",
                    egui_icons::icons::ICON_ACCOUNT_TREE.codepoint,
                    win.scope
                ),
                &mut close,
            );
            ui.label(egui::RichText::new(subtitle(win)).weak().small());
            ui.add_space(8.0);

            if win.running {
                style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .max_height(list_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            render_job_progress(
                                ui,
                                &win.progress,
                                win.started_at,
                                win.last_activity_at,
                                None,
                                true,
                            );
                        });
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if win.cancel_requested {
                            "Cancelling…"
                        } else {
                            "Cancel"
                        };
                        if ui
                            .add_enabled(
                                !win.cancel_requested,
                                egui::Button::new(label).min_size(egui::vec2(0.0, 28.0)),
                            )
                            .clicked()
                        {
                            win.cancel_requested = true;
                        }
                        if ui
                            .add(
                                egui::Button::new("Run in Background")
                                    .min_size(egui::vec2(0.0, 28.0)),
                            )
                            .on_hover_text("Hide this window; a notification appears when it ends")
                            .clicked()
                        {
                            win.hidden = true;
                        }
                    });
                });
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }

            if let Some(err) = &win.error {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {err}",
                        egui_icons::icons::ICON_ERROR.codepoint
                    ))
                    .color(ui.visuals().error_fg_color),
                );
                ui.add_space(4.0);
            }
            if let Some(note) = &win.note {
                ui.label(
                    egui::RichText::new(format!(
                        "{} {note}",
                        egui_icons::icons::ICON_INFO.codepoint
                    ))
                    .small()
                    .color(ui.visuals().warn_fg_color),
                );
                ui.add_space(4.0);
            }
            style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!(
                        "{} generated · {} unchanged · {} failed",
                        win.generated,
                        win.skipped_fresh,
                        win.failed.len()
                    ))
                    .strong(),
                );
                if win.generated > 0 {
                    ui.label(
                        egui::RichText::new(
                            "Select a card on the canvas to see its steps, or double-click it to \
                             play the process.",
                        )
                        .small()
                        .weak(),
                    );
                }
                if !win.failed.is_empty() {
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .max_height(list_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (label, msg) in win.failed.iter().take(MAX_FAILED_SHOWN) {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new(label).monospace().small().strong(),
                                    );
                                    ui.label(egui::RichText::new(msg).small().weak());
                                });
                            }
                            let more = win.failed.len().saturating_sub(MAX_FAILED_SHOWN);
                            if more > 0 {
                                ui.label(egui::RichText::new(format!("+{more} more")).weak());
                            }
                        });
                }
            });
            if !win.progress.is_empty() {
                egui::CollapsingHeader::new(egui::RichText::new("Details").small().weak())
                    .default_open(win.error.is_some())
                    .id_salt("diagram_flow_gen_details")
                    .show(ui, |ui| {
                        render_job_progress(
                            ui,
                            &win.progress,
                            win.started_at,
                            win.last_activity_at,
                            win.elapsed,
                            false,
                        );
                    });
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(accent_button(ui, "Close")).clicked() {
                        close = true;
                    }
                });
            });
        });

    if close {
        // Menutup saat berjalan = sembunyikan; job tetap berjalan.
        if win.running {
            win.hidden = true;
        } else {
            state.flow_gen = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_mentions_repository_position() {
        let mut win = FlowGenWindow {
            card_count: 4,
            repo_label: "~/code/shop".into(),
            repo_index: 1,
            repo_total: 1,
            ..Default::default()
        };
        assert_eq!(subtitle(&win), "4 endpoint(s) from ~/code/shop");
        win.repo_total = 3;
        win.repo_index = 2;
        assert_eq!(
            subtitle(&win),
            "4 endpoint(s) from ~/code/shop · repository 2 of 3"
        );
    }
}
