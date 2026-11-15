//! Panel "Query History" yang meluncur dari sisi kanan area editor.
//!
//! Dulu riwayat query berupa sub-tab "History" di sidebar Database. Sekarang
//! dibuka dari tombol di kluster aksi pojok kanan bawah query editor, dan
//! tampil sebagai panel melayang (overlay) seperti jendela chat: masuk dari
//! kanan dengan animasi geser, ditutup lewat tombol close atau Esc.

use eframe::egui;
use egui_icons::icons;

use super::Tabular;
use super::style;

/// Lebar panel yang diinginkan (dibatasi lebar area konten).
const PANEL_WIDTH: f32 = 380.0;
/// Durasi animasi geser masuk/keluar (detik).
const SLIDE_SECONDS: f32 = 0.18;

/// Id data sementara egui tempat area konten (di bawah tab bar) disimpan
/// oleh panel tengah untuk dipakai panel ini.
fn anchor_id() -> egui::Id {
    egui::Id::new("history_panel_anchor")
}

/// Simpan area konten (di bawah tab bar sampai pojok kanan bawah) frame ini.
pub fn set_anchor(ctx: &egui::Context, rect: egui::Rect) {
    ctx.data_mut(|d| d.insert_temp(anchor_id(), rect));
}

/// Buka/tutup panel riwayat.
pub fn toggle(t: &mut Tabular) {
    t.show_history_panel = !t.show_history_panel;
}

/// Render panel bila terbuka (atau sedang beranimasi keluar).
/// Dipanggil setelah panel tengah agar Esc dari editor (autocomplete, find)
/// diproses lebih dulu.
pub fn render(t: &mut Tabular, ctx: &egui::Context) {
    let progress = ctx.animate_bool_with_time(
        egui::Id::new("history_panel_slide"),
        t.show_history_panel,
        SLIDE_SECONDS,
    );
    if progress <= 0.0 {
        return;
    }
    let Some(content) = ctx.data(|d| d.get_temp::<egui::Rect>(anchor_id())) else {
        return;
    };
    if content.width() < 160.0 || content.height() < 120.0 {
        return;
    }

    if t.show_history_panel
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        t.show_history_panel = false;
    }

    let width = PANEL_WIDTH.min(content.width() * 0.9);
    let offset = (1.0 - progress) * (width + 16.0);
    let pos = content.right_top() + egui::vec2(offset, 0.0);

    egui::Area::new(egui::Id::new("history_slide_panel"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(pos)
        .constrain(false)
        .interactable(progress > 0.5)
        .show(ctx, |ui| {
            // Potong bagian yang masih di luar area konten saat beranimasi.
            ui.set_clip_rect(content);
            let visuals = ui.visuals().clone();
            egui::Frame::NONE
                .fill(style::ai_panel_bg(ctx))
                .stroke(visuals.widgets.noninteractive.bg_stroke)
                .shadow(visuals.popup_shadow)
                .corner_radius(egui::CornerRadius {
                    nw: 8,
                    sw: 8,
                    ne: 0,
                    se: 0,
                })
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    let inner_w = width - 24.0;
                    let inner_h = content.height() - 20.0;
                    ui.set_width(inner_w);
                    ui.set_height(inner_h);
                    render_body(t, ui);
                });
        });
}

/// Isi panel: header, pencarian, auto refresh, lalu pohon riwayat.
fn render_body(t: &mut Tabular, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(icons::ICON_HISTORY.codepoint)
                .size(16.0)
                .color(style::theme_accent(&ctx)),
        );
        ui.label(egui::RichText::new("Query History").strong().size(13.5));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if style::ai_icon_button(ui, icons::ICON_CLOSE.codepoint, "Close history (Esc)")
                .clicked()
            {
                t.show_history_panel = false;
            }
            if !t.history_tree.is_empty()
                && style::ai_icon_button(ui, icons::ICON_DELETE.codepoint, "Clear History")
                    .clicked()
            {
                t.show_clear_history_confirm = true;
            }
        });
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    let search = style::render_search_field_with_id(
        ui,
        egui::Id::new("history_panel_search"),
        &mut t.history_search_text,
        "Search history…",
        f32::INFINITY,
    );
    if search.changed() {
        crate::sidebar_history::filter_history_tree(t);
    }
    ui.add_space(6.0);

    if t.auto_refresh_active {
        render_auto_refresh(t, ui);
        ui.add_space(6.0);
    }

    let is_searching = !t.history_search_text.trim().is_empty();
    let mut opened = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("history_panel_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut tree = if is_searching {
                std::mem::take(&mut t.filtered_history_tree)
            } else {
                std::mem::take(&mut t.history_tree)
            };
            opened = t.render_tree(ui, &mut tree, false);
            if is_searching {
                t.filtered_history_tree = tree;
            } else {
                t.history_tree = tree;
            }

            if t.history_tree.is_empty() && !is_searching {
                ui.add_space(8.0);
                ui.weak("No query history yet");
                ui.weak("Executed queries will show up here");
            } else if is_searching && t.filtered_history_tree.is_empty() {
                ui.add_space(8.0);
                ui.weak("No history matches the search");
            }
        });

    open_entries(t, opened);
}

/// Kotak status Auto Query (interval, tombol stop, query yang diulang).
fn render_auto_refresh(t: &mut Tabular, ui: &mut egui::Ui) {
    let danger = style::theme_danger(ui.ctx());
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, danger))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let interval = t.auto_refresh_interval_seconds.max(1) as u64;
                let remaining = match t.auto_refresh_last_run {
                    Some(last) => interval.saturating_sub(last.elapsed().as_secs()),
                    None => interval,
                };
                ui.label(format!("Auto Query in {remaining} second(s)"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let stop = egui::Button::new(
                        egui::RichText::new(format!("{} Stop", icons::ICON_STOP.codepoint))
                            .color(egui::Color32::WHITE),
                    )
                    .fill(danger);
                    if ui.add(stop).clicked() {
                        t.stop_auto_refresh();
                    }
                });
            });
            if let Some(q) = &t.auto_refresh_query {
                ui.add(
                    egui::TextEdit::multiline(&mut q.clone())
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .interactive(false),
                );
            }
        });
}

/// Buka entri riwayat yang diklik sebagai tab baru, terikat ke koneksinya
/// bila koneksi itu masih ada.
fn open_entries(t: &mut Tabular, opened: Vec<(String, String, String, Option<i64>)>) {
    for (filename, content, file_data, _) in opened {
        if let Some((connection_name, _query)) = file_data.split_once("||") {
            let conn_id = t
                .connections
                .iter()
                .find(|c| c.name == connection_name)
                .and_then(|c| c.id);
            if let Some(cid) = conn_id {
                crate::editor::create_new_tab_with_connection(t, filename, content, Some(cid));
                continue;
            }
            if !connection_name.is_empty() {
                log::debug!(
                    "[HISTORY] Connection '{connection_name}' not found, opening tab without binding"
                );
            }
        }
        crate::editor::create_new_tab(t, filename, content);
    }
}
