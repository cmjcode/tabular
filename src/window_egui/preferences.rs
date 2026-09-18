//! Jendela Preferences: navigasi kiri, halaman per kategori, dan komponen
//! UI bersama (header, section card, baris form, toggle, callout) supaya
//! semua halaman punya tampilan yang seragam dan mengikuti tema aktif.

use eframe::egui;

use super::{PrefTab, Tabular, style};
use crate::config::{AiBackend, AiProvider, AppTheme, UiModePreference};
use crate::models::structs::EditorColorTheme;

/// Lebar kolom navigasi kiri.
const NAV_WIDTH: f32 = 184.0;
/// Tinggi area footer (separator + tombol).
const FOOTER_HEIGHT: f32 = 44.0;
/// Lama pesan umpan balik di footer tetap tampil.
const FEEDBACK_SECS: f32 = 4.0;

impl PrefTab {
    /// Urutan tab di navigasi. Tab Update disembunyikan bila self-update
    /// tidak didukung (build iOS / App Store).
    pub fn visible() -> Vec<PrefTab> {
        let mut tabs = vec![
            PrefTab::ApplicationTheme,
            PrefTab::EditorTheme,
            PrefTab::Performance,
            PrefTab::DataDirectory,
        ];
        if crate::self_update::SELF_UPDATE_SUPPORTED {
            tabs.push(PrefTab::Update);
        }
        tabs.extend([PrefTab::AiAssistant, PrefTab::Sync, PrefTab::Plugins]);
        tabs
    }

    pub fn label(self) -> &'static str {
        match self {
            PrefTab::ApplicationTheme => "Appearance",
            PrefTab::EditorTheme => "Editor",
            PrefTab::Performance => "Performance",
            PrefTab::DataDirectory => "Data & Backup",
            PrefTab::Update => "Updates",
            PrefTab::AiAssistant => "AI Assistant",
            PrefTab::Sync => "Cloud Sync",
            PrefTab::Plugins => "Plugins",
        }
    }

    pub fn icon(self) -> &'static str {
        use egui_icons::icons as i;
        match self {
            PrefTab::ApplicationTheme => i::ICON_PALETTE.codepoint,
            PrefTab::EditorTheme => i::ICON_CODE.codepoint,
            PrefTab::Performance => i::ICON_SPEED.codepoint,
            PrefTab::DataDirectory => i::ICON_FOLDER.codepoint,
            PrefTab::Update => i::ICON_SYSTEM_UPDATE.codepoint,
            PrefTab::AiAssistant => i::ICON_AUTO_AWESOME.codepoint,
            PrefTab::Sync => i::ICON_CLOUD_SYNC.codepoint,
            PrefTab::Plugins => i::MDI_PUZZLE.codepoint,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Komponen UI bersama
// ─────────────────────────────────────────────────────────────────────────────

/// Jenis status untuk teks status dan callout.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Success,
    Warning,
    Danger,
    Info,
    Muted,
}

pub(crate) fn tone_color(ctx: &egui::Context, tone: Tone) -> egui::Color32 {
    match tone {
        Tone::Success => style::theme_success(ctx),
        Tone::Warning => style::theme_warning(ctx),
        Tone::Danger => style::theme_danger(ctx),
        Tone::Info => style::theme_info(ctx),
        Tone::Muted => style::theme_muted_text(ctx),
    }
}

/// Judul halaman beserta kalimat pengantar.
pub(crate) fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(title).size(19.0).strong());
    if !subtitle.is_empty() {
        ui.add_space(2.0);
        ui.label(
            egui::RichText::new(subtitle)
                .size(12.0)
                .color(style::theme_muted_text(ui.ctx())),
        );
    }
    ui.add_space(14.0);
}

/// Section berjudul dengan isi di dalam card bertema.
pub(crate) fn section<R>(
    ui: &mut egui::Ui,
    title: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .size(10.5)
            .strong()
            .color(style::theme_muted_text(ui.ctx())),
    );
    ui.add_space(4.0);
    let inner = style::theme_card_frame(ui.ctx())
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add_contents(ui)
        })
        .inner;
    ui.add_space(16.0);
    inner
}

/// Garis pemisah tipis antar baris di dalam section.
pub(crate) fn divider(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    let color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, color.gamma_multiply(0.7));
    ui.add_space(4.0);
}

/// Teks bantuan kecil berwarna redup.
pub(crate) fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(
        egui::RichText::new(text.into())
            .size(11.0)
            .color(style::theme_muted_text(ui.ctx())),
    );
}

/// Teks status berwarna sesuai tone.
pub(crate) fn status(ui: &mut egui::Ui, tone: Tone, text: impl Into<String>) {
    let color = tone_color(ui.ctx(), tone);
    ui.label(egui::RichText::new(text.into()).size(11.5).color(color));
}

/// Kotak pemberitahuan dengan latar tipis sesuai tone.
pub(crate) fn callout(ui: &mut egui::Ui, tone: Tone, add_contents: impl FnOnce(&mut egui::Ui)) {
    let color = tone_color(ui.ctx(), tone);
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            add_contents(ui);
        });
}

/// Baris form dua kolom: label (dan hint) di kiri, kontrol di kanan.
/// Kolom kontrol selalu mulai pada posisi x yang sama sehingga rata.
pub(crate) fn row<R>(
    ui: &mut egui::Ui,
    label: &str,
    hint_text: Option<&str>,
    add_control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        let total = ui.available_width();
        let label_w = (total * 0.42).clamp(150.0, 300.0);
        ui.allocate_ui_with_layout(
            egui::vec2(label_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(label_w);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(egui::RichText::new(label).size(13.0));
                if let Some(h) = hint_text {
                    hint(ui, h);
                }
            },
        );
        ui.add_space(12.0);
        add_control(ui)
    })
    .inner
}

/// Baris dengan toggle switch. Mengembalikan `true` bila nilainya berubah.
pub(crate) fn toggle_row(
    ui: &mut egui::Ui,
    value: &mut bool,
    label: &str,
    hint_text: Option<&str>,
) -> bool {
    row(ui, label, hint_text, |ui| toggle(ui, value).changed())
}

/// Field bertumpuk: label di atas, kontrol selebar penuh di bawah.
pub(crate) fn stacked<R>(
    ui: &mut egui::Ui,
    label: &str,
    hint_text: Option<&str>,
    add_control: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.label(egui::RichText::new(label).size(13.0));
    let r = add_control(ui);
    if let Some(h) = hint_text {
        hint(ui, h);
    }
    r
}

/// Toggle switch bergaya iOS/macOS dengan animasi.
pub(crate) fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let height = 20.0;
    let desired = egui::vec2(36.0, height);
    let (rect, mut response) = ui.allocate_exact_size(desired, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let off_bg = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(70, 74, 84)
        } else {
            egui::Color32::from_rgb(200, 204, 212)
        };
        let bg = off_bg.lerp_to_gamma(style::theme_accent(ui.ctx()), t);
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, bg);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob = egui::pos2(x, rect.center().y);
        ui.painter()
            .circle_filled(knob, radius - 3.0, egui::Color32::WHITE);
    }
    response
}

/// Kontrol tersegmentasi. Mengembalikan indeks opsi yang baru diklik.
pub(crate) fn segmented(ui: &mut egui::Ui, options: &[&str], selected: usize) -> Option<usize> {
    let accent = style::theme_accent(ui.ctx());
    let mut clicked = None;
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(7.0)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, opt) in options.iter().enumerate() {
                    let is_sel = i == selected;
                    let mut text = egui::RichText::new(*opt).size(12.0);
                    if is_sel {
                        text = text.color(egui::Color32::WHITE).strong();
                    }
                    let btn = egui::Button::new(text)
                        .fill(if is_sel {
                            accent
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(5.0)
                        .min_size(egui::vec2(72.0, 24.0));
                    if ui.add(btn).clicked() && !is_sel {
                        clicked = Some(i);
                    }
                }
            });
        });
    clicked
}

/// Chip yang bisa dipilih (dipakai untuk "quick pick" model).
pub(crate) fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let accent = style::theme_accent(ui.ctx());
    let (fill, stroke, fg) = if selected {
        (
            accent.gamma_multiply(0.18),
            accent,
            ui.visuals().strong_text_color(),
        )
    } else {
        (
            egui::Color32::TRANSPARENT,
            ui.visuals().widgets.inactive.bg_stroke.color,
            ui.visuals().text_color(),
        )
    };
    ui.add(
        egui::Button::new(egui::RichText::new(text).size(11.0).monospace().color(fg))
            .fill(fill)
            .stroke(egui::Stroke::new(1.0, stroke))
            .corner_radius(12.0)
            .min_size(egui::vec2(0.0, 22.0)),
    )
}

/// Deretan chip model pilihan cepat; mengembalikan model yang diklik.
pub(crate) fn quick_pick(
    ui: &mut egui::Ui,
    presets: &[&'static str],
    current: &str,
) -> Option<&'static str> {
    if presets.is_empty() {
        return None;
    }
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
        hint(ui, "Quick pick:");
        for &m in presets {
            if chip(ui, m, current == m).clicked() {
                picked = Some(m);
            }
        }
    });
    picked
}

/// Card pilihan yang bisa diklik (judul, deskripsi, tanda centang).
fn choice_card(
    ui: &mut egui::Ui,
    width: f32,
    title: &str,
    desc: &str,
    selected: bool,
) -> egui::Response {
    let size = egui::vec2(width, 64.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        paint_card_background(ui, rect, selected, response.hovered());
        let muted = style::theme_muted_text(ui.ctx());
        let text_col = ui.visuals().strong_text_color();
        let painter = ui.painter();
        painter.text(
            rect.left_top() + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(13.0),
            text_col,
        );
        let galley = painter.layout(
            desc.to_string(),
            egui::FontId::proportional(11.0),
            muted,
            width - 24.0,
        );
        painter.galley(rect.left_top() + egui::vec2(12.0, 32.0), galley, muted);
        if selected {
            paint_check(ui, rect);
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_card_background(ui: &egui::Ui, rect: egui::Rect, selected: bool, hovered: bool) {
    let v = ui.visuals();
    let accent = style::theme_accent(ui.ctx());
    let fill = if selected {
        accent.gamma_multiply(0.08)
    } else if hovered {
        v.widgets.hovered.weak_bg_fill.gamma_multiply(0.5)
    } else {
        v.extreme_bg_color
    };
    let stroke = if selected {
        egui::Stroke::new(1.5, accent)
    } else if hovered {
        egui::Stroke::new(1.0, v.widgets.hovered.bg_stroke.color)
    } else {
        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color)
    };
    ui.painter()
        .rect(rect, 8.0, fill, stroke, egui::StrokeKind::Inside);
}

fn paint_check(ui: &egui::Ui, rect: egui::Rect) {
    let accent = style::theme_accent(ui.ctx());
    let center = egui::pos2(rect.right() - 16.0, rect.top() + 18.0);
    ui.painter().circle_filled(center, 8.0, accent);
    ui.painter().text(
        center,
        egui::Align2::CENTER_CENTER,
        egui_icons::icons::ICON_CHECK.codepoint,
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
}

/// Card tema aplikasi dengan pratinjau mini warna tema tersebut.
fn theme_card(
    ui: &mut egui::Ui,
    width: f32,
    theme: AppTheme,
    title: &str,
    caption: &str,
    selected: bool,
) -> egui::Response {
    let size = egui::vec2(width, 150.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        paint_card_background(ui, rect, selected, response.hovered());
        let tv = match theme {
            AppTheme::Dark => style::dark_visuals(),
            AppTheme::Light => style::light_visuals(),
            AppTheme::LightSoft => style::light_soft_visuals(),
        };
        let accent = style::theme_accent(ui.ctx());
        let painter = ui.painter();

        // Pratinjau: panel utama, sidebar, dan beberapa "baris teks".
        let preview = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::vec2(width - 20.0, 70.0),
        );
        painter.rect(
            preview,
            5.0,
            tv.panel_fill,
            egui::Stroke::new(1.0, tv.widgets.noninteractive.bg_stroke.color),
            egui::StrokeKind::Inside,
        );
        let sidebar = egui::Rect::from_min_max(
            preview.left_top() + egui::vec2(1.0, 1.0),
            egui::pos2(
                preview.left() + preview.width() * 0.28,
                preview.bottom() - 1.0,
            ),
        );
        painter.rect_filled(
            sidebar,
            egui::CornerRadius {
                nw: 4,
                sw: 4,
                ne: 0,
                se: 0,
            },
            tv.extreme_bg_color,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(
                sidebar.left_top() + egui::vec2(6.0, 8.0),
                egui::vec2(sidebar.width() - 12.0, 5.0),
            ),
            2.0,
            accent,
        );
        for (i, frac) in [0.55_f32, 0.4, 0.62, 0.3].iter().enumerate() {
            let y = preview.top() + 12.0 + i as f32 * 13.0;
            let x = sidebar.right() + 10.0;
            let w = (preview.right() - x - 10.0) * frac;
            painter.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, 5.0)),
                2.0,
                tv.text_color().gamma_multiply(0.45),
            );
        }

        let text_col = ui.visuals().strong_text_color();
        let muted = style::theme_muted_text(ui.ctx());
        painter.text(
            egui::pos2(rect.left() + 12.0, preview.bottom() + 10.0),
            egui::Align2::LEFT_TOP,
            title,
            egui::FontId::proportional(13.0),
            text_col,
        );
        let galley = painter.layout(
            caption.to_string(),
            egui::FontId::proportional(11.0),
            muted,
            width - 24.0,
        );
        painter.galley(
            egui::pos2(rect.left() + 12.0, preview.bottom() + 30.0),
            galley,
            muted,
        );
        if selected {
            paint_check(
                ui,
                egui::Rect::from_min_max(egui::pos2(rect.left(), preview.bottom()), rect.max),
            );
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Lebar card agar `count` card muat dalam satu baris.
fn card_width(ui: &egui::Ui, count: usize, gap: f32, min: f32) -> f32 {
    let avail = ui.available_width();
    ((avail - gap * (count as f32 - 1.0)) / count as f32)
        .max(min)
        .floor()
}

/// Item navigasi kiri: ikon + label, latar lembut dan garis accent saat aktif.
fn nav_item(ui: &mut egui::Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    let height = ui.spacing().interact_size.y.max(32.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if ui.is_rect_visible(rect) {
        let dark = ui.visuals().dark_mode;
        let hovered = response.hovered();
        if selected || hovered {
            let alpha = if selected { 1.0 } else { 0.5 };
            let base = if dark {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 16)
            } else {
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 12)
            };
            ui.painter()
                .rect_filled(rect, 6.0, base.gamma_multiply(alpha));
        }
        if selected {
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + 7.0),
                egui::vec2(3.0, rect.height() - 14.0),
            );
            ui.painter()
                .rect_filled(bar, 1.5, style::theme_accent(ui.ctx()));
        }
        let color = if selected {
            ui.visuals().strong_text_color()
        } else if hovered {
            ui.visuals().text_color()
        } else {
            style::theme_muted_text(ui.ctx())
        };
        ui.painter().text(
            egui::pos2(rect.left() + 22.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(15.0),
            color,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 40.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(13.0),
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ─────────────────────────────────────────────────────────────────────────────
// Shell dialog
// ─────────────────────────────────────────────────────────────────────────────

impl Tabular {
    /// Render jendela Preferences (modal).
    pub(super) fn render_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_settings_window {
            return;
        }
        // Preferensi lama bisa menyimpan tab Update di platform tanpa self-update.
        if !PrefTab::visible().contains(&self.settings_active_pref_tab) {
            self.settings_active_pref_tab = PrefTab::ApplicationTheme;
        }

        let screen = ctx.content_rect();
        let dialog_w = 1000.0_f32.min(screen.width() - 40.0).max(560.0);
        let dialog_h = 640.0_f32.min(screen.height() - 60.0).max(380.0);
        let body_h = dialog_h - FOOTER_HEIGHT;

        let mut open_flag = true;
        let mut close_requested = false;

        egui::Window::new("Preferences")
            .open(&mut open_flag)
            .collapsible(false)
            .resizable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(screen.center())
            .fixed_size(egui::vec2(dialog_w, dialog_h))
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    // Navigasi kiri
                    ui.allocate_ui_with_layout(
                        egui::vec2(NAV_WIDTH, body_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_size(egui::vec2(NAV_WIDTH, body_h));
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.add_space(4.0);
                            for tab in PrefTab::visible() {
                                let selected = self.settings_active_pref_tab == tab;
                                if nav_item(ui, tab.icon(), tab.label(), selected).clicked() {
                                    self.settings_active_pref_tab = tab;
                                }
                            }
                        },
                    );

                    ui.add_space(12.0);
                    let (line, _) =
                        ui.allocate_exact_size(egui::vec2(1.0, body_h), egui::Sense::hover());
                    ui.painter().rect_filled(
                        line,
                        0.0,
                        ui.visuals().widgets.noninteractive.bg_stroke.color,
                    );
                    ui.add_space(20.0);

                    // Konten halaman
                    let content_w = ui.available_width();
                    ui.allocate_ui_with_layout(
                        egui::vec2(content_w, body_h),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_min_size(egui::vec2(content_w, body_h));
                            ui.set_max_height(body_h);
                            if self.settings_active_pref_tab == PrefTab::Plugins {
                                ui.add_space(4.0);
                                self.render_pref_plugins(ui);
                            } else {
                                egui::ScrollArea::vertical()
                                    .id_salt((
                                        "settings_content_scroll",
                                        self.settings_active_pref_tab as u8,
                                    ))
                                    .auto_shrink([false, false])
                                    .max_height(body_h)
                                    .show(ui, |ui| {
                                        ui.add_space(4.0);
                                        // Sisakan ruang untuk scrollbar di kanan.
                                        ui.set_width(ui.available_width() - 14.0);
                                        self.render_pref_page(ui);
                                        ui.add_space(8.0);
                                    });
                            }
                        },
                    );
                });

                self.render_pref_footer(ui, &mut close_requested);
            });

        if !open_flag || close_requested {
            self.show_settings_window = false;
        }
    }

    fn render_pref_page(&mut self, ui: &mut egui::Ui) {
        match self.settings_active_pref_tab {
            PrefTab::ApplicationTheme => self.render_pref_appearance(ui),
            PrefTab::EditorTheme => self.render_pref_editor(ui),
            PrefTab::Performance => self.render_pref_performance(ui),
            PrefTab::DataDirectory => self.render_pref_data(ui),
            PrefTab::Update => self.render_pref_updates(ui),
            PrefTab::AiAssistant => self.render_pref_ai(ui),
            PrefTab::Sync => crate::sync::ui_login::render_sync_panel(self, ui),
            PrefTab::Plugins => {}
        }
    }

    fn render_pref_footer(&mut self, ui: &mut egui::Ui, close_requested: &mut bool) {
        let divider_col = ui.visuals().widgets.noninteractive.bg_stroke.color;
        ui.add_space(6.0);
        let (line, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(line, 0.0, divider_col);
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            let recent = self
                .prefs_last_saved_at
                .map(|t| t.elapsed().as_secs_f32() < FEEDBACK_SECS)
                .unwrap_or(false);
            match (&self.prefs_save_feedback, recent) {
                (Some(msg), true) => {
                    status(
                        ui,
                        Tone::Success,
                        format!("{}  {}", egui_icons::icons::ICON_CHECK.codepoint, msg),
                    );
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(500));
                }
                _ => hint(ui, "Changes are saved automatically."),
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(style::btn_secondary("Close").min_size(egui::vec2(80.0, 28.0)))
                    .clicked()
                {
                    *close_requested = true;
                }
                if ui
                    .add(style::btn_primary_ctx(ui.ctx(), "Save").min_size(egui::vec2(80.0, 28.0)))
                    .clicked()
                {
                    self.prefs_dirty = true;
                    self.try_save_prefs();
                    self.set_pref_feedback("Preferences saved");
                }
            });
        });
    }

    /// Tampilkan pesan singkat di footer Preferences.
    pub(crate) fn set_pref_feedback(&mut self, msg: impl Into<String>) {
        self.prefs_save_feedback = Some(msg.into());
        self.prefs_last_saved_at = Some(std::time::Instant::now());
    }

    fn save_prefs_now(&mut self) {
        self.prefs_dirty = true;
        self.try_save_prefs();
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: Appearance
    // ─────────────────────────────────────────────────────────────────────

    fn apply_app_theme(&mut self, ctx: &egui::Context, theme: AppTheme) {
        self.app_theme = theme;
        let metrics =
            crate::window_egui::device_profile::DeviceUiMetrics::compute(ctx, self.ui_mode);
        style::apply_theme(ctx, self.app_theme, &metrics);
        if self.link_editor_theme {
            self.advanced_editor.theme = linked_editor_theme(self.app_theme);
        }
        self.save_prefs_now();
    }

    fn render_pref_appearance(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        page_header(
            ui,
            "Appearance",
            "Choose how Tabular looks and how its controls are sized.",
        );

        section(ui, "Theme", |ui| {
            let themes = [
                (
                    AppTheme::Dark,
                    "Dark",
                    "Rich contrast and calm surfaces for late-night work.",
                ),
                (
                    AppTheme::Light,
                    "Light",
                    "Bright, crisp palette for a clean editor experience.",
                ),
                (
                    AppTheme::LightSoft,
                    "Light Soft",
                    "Gentle warmth with soft backgrounds for long sessions.",
                ),
            ];
            let gap = 10.0;
            let w = card_width(ui, themes.len(), gap, 150.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                for (theme, title, caption) in themes {
                    let selected = self.app_theme == theme;
                    if theme_card(ui, w, theme, title, caption, selected).clicked() && !selected {
                        self.apply_app_theme(&ctx, theme);
                    }
                }
            });
        });

        section(ui, "Interface", |ui| {
            let modes = [
                (
                    UiModePreference::Auto,
                    "Automatic",
                    "Detect from the system (iOS/Android) or the screen resolution.",
                ),
                (
                    UiModePreference::Desktop,
                    "Desktop",
                    "Compact, dense controls for mouse and physical keyboard.",
                ),
                (
                    UiModePreference::TouchTablet,
                    "Touch",
                    "44pt touch targets, 38px table rows and a quick keyword toolbar.",
                ),
            ];
            let current = modes.iter().position(|m| m.0 == self.ui_mode).unwrap_or(0);
            let labels: Vec<&str> = modes.iter().map(|m| m.1).collect();
            row(ui, "Interface mode", Some(modes[current].2), |ui| {
                if let Some(i) = segmented(ui, &labels, current) {
                    self.ui_mode = modes[i].0;
                    let metrics = crate::window_egui::device_profile::DeviceUiMetrics::compute(
                        &ctx,
                        self.ui_mode,
                    );
                    style::apply_theme(&ctx, self.app_theme, &metrics);
                    self.save_prefs_now();
                    ctx.request_repaint();
                }
            });
        });
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: Editor
    // ─────────────────────────────────────────────────────────────────────

    fn render_pref_editor(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Editor",
            "Syntax highlighting and text settings for the SQL editor.",
        );

        section(ui, "Syntax Theme", |ui| {
            if toggle_row(
                ui,
                &mut self.link_editor_theme,
                "Follow application theme",
                Some("Uses GitHub Dark or GitHub Light to match the current app theme."),
            ) {
                if self.link_editor_theme {
                    self.advanced_editor.theme = linked_editor_theme(self.app_theme);
                }
                self.save_prefs_now();
            }
            divider(ui);

            let themes = [
                (
                    EditorColorTheme::GithubDark,
                    "GitHub Dark",
                    "Dark theme with blue accents",
                ),
                (
                    EditorColorTheme::GithubLight,
                    "GitHub Light",
                    "Clean light theme",
                ),
                (
                    EditorColorTheme::Gruvbox,
                    "Gruvbox",
                    "Warm earthy retro palette",
                ),
            ];
            let gap = 10.0;
            let w = card_width(ui, themes.len(), gap, 150.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
                for (theme, name, desc) in themes {
                    let selected = self.advanced_editor.theme == theme;
                    if choice_card(ui, w, name, desc, selected).clicked() && !selected {
                        self.advanced_editor.theme = theme;
                        // Memilih tema manual memutus tautan ke tema aplikasi.
                        self.link_editor_theme = false;
                        self.save_prefs_now();
                    }
                }
            });
        });

        section(ui, "Text", |ui| {
            row(ui, "Font size", Some("Between 8 and 32 points."), |ui| {
                let mut fs = self.advanced_editor.font_size as i32;
                if ui
                    .add(egui::DragValue::new(&mut fs).range(8..=32).suffix(" pt"))
                    .changed()
                {
                    self.advanced_editor.font_size = fs as f32;
                    self.save_prefs_now();
                }
            });
            divider(ui);
            if toggle_row(
                ui,
                &mut self.advanced_editor.show_line_numbers,
                "Line numbers",
                None,
            ) {
                self.save_prefs_now();
            }
            divider(ui);
            if toggle_row(
                ui,
                &mut self.advanced_editor.word_wrap,
                "Word wrap",
                Some("Wrap long lines instead of scrolling horizontally."),
            ) {
                self.save_prefs_now();
            }
        });
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: Performance
    // ─────────────────────────────────────────────────────────────────────

    fn render_pref_performance(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Performance",
            "Control how much data is loaded and how long queries may run.",
        );

        section(ui, "Data Loading", |ui| {
            let prev = self.use_server_pagination;
            if toggle_row(
                ui,
                &mut self.use_server_pagination,
                "Server-side pagination",
                Some(
                    "Fetch large tables in pages (e.g. 100 rows) instead of all at once. May not work with every custom query.",
                ),
            ) {
                self.save_prefs_now();
                if prev != self.use_server_pagination && !self.current_table_headers.is_empty() {
                    self.set_pref_feedback(if self.use_server_pagination {
                        "Server pagination enabled. Browse a table to see the difference!"
                    } else {
                        "Client pagination enabled. Data will be loaded all at once."
                    });
                }
            }
            divider(ui);
            row(
                ui,
                "Max rows per result",
                Some(
                    "Larger result sets are truncated (with a notice) so a stray SELECT * cannot exhaust memory.",
                ),
                |ui| {
                    let mut rows = self.max_result_rows as i64;
                    if ui
                        .add(
                            egui::DragValue::new(&mut rows)
                                .range(100..=5_000_000)
                                .speed(100),
                        )
                        .changed()
                    {
                        self.max_result_rows = rows.max(100) as u32;
                        self.save_prefs_now();
                    }
                },
            );
        });

        section(ui, "Query Execution", |ui| {
            row(
                ui,
                "Query timeout",
                Some(
                    "A statement running longer than this is cancelled on the server. 0 = never time out.",
                ),
                |ui| {
                    let mut secs = self.query_timeout_secs as i64;
                    if ui
                        .add(
                            egui::DragValue::new(&mut secs)
                                .range(0..=86_400)
                                .suffix(" s"),
                        )
                        .changed()
                    {
                        self.query_timeout_secs = secs.max(0) as u32;
                        self.save_prefs_now();
                    }
                    if self.query_timeout_secs == 0 {
                        hint(ui, "no limit");
                    }
                },
            );
        });

        section(ui, "Redis Browser", |ui| {
            row(
                ui,
                "Auto-refresh interval",
                Some("Default interval used when Redis browser auto-refresh is enabled."),
                |ui| {
                    let mut seconds = self.redis_browser_auto_refresh_default_seconds.max(1) as i32;
                    if ui
                        .add(
                            egui::DragValue::new(&mut seconds)
                                .range(1..=3600)
                                .suffix(" s"),
                        )
                        .changed()
                    {
                        self.redis_browser_auto_refresh_default_seconds = seconds.max(1) as u32;
                        self.save_prefs_now();
                    }
                },
            );
        });

        section(ui, "Session & Diagnostics", |ui| {
            if toggle_row(
                ui,
                &mut self.restore_session,
                "Restore session on startup",
                Some("Reopen tabs and unsaved drafts from the last session."),
            ) {
                self.save_prefs_now();
            }
            divider(ui);
            let log_hint = format!(
                "Verbose logs (may include SQL text) are written to {}. Leave off for normal use.",
                crate::app_logging::log_file_path().display()
            );
            if toggle_row(
                ui,
                &mut self.enable_debug_logging,
                "Debug logging",
                Some(&log_hint),
            ) {
                self.save_prefs_now();
                crate::app_logging::set_verbose(self.enable_debug_logging);
                self.set_pref_feedback(if self.enable_debug_logging {
                    "Debug logging enabled."
                } else {
                    "Debug logging disabled."
                });
            }
        });
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: Data & Backup
    // ─────────────────────────────────────────────────────────────────────

    fn render_pref_data(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Data & Backup",
            "Where Tabular stores connections, saved queries and history, and how to back them up.",
        );

        if self.temp_data_directory.is_empty() {
            self.temp_data_directory = self.data_directory.clone();
        }

        section(ui, "Storage Location", |ui| {
            stacked(ui, "Current location", None, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .corner_radius(5.0)
                    .inner_margin(egui::Margin::symmetric(8, 5))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&self.data_directory)
                                    .monospace()
                                    .size(12.0),
                            )
                            .selectable(true),
                        );
                    });
            });
            ui.add_space(4.0);
            stacked(ui, "New location", None, |ui| {
                ui.horizontal(|ui| {
                    let browse_w = 96.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut self.temp_data_directory)
                            .desired_width(ui.available_width() - browse_w - 8.0)
                            .hint_text("/absolute/path/to/folder"),
                    );
                    let label = format!("{}  Browse", egui_icons::icons::ICON_FOLDER.codepoint);
                    if ui
                        .add(style::btn_secondary(label).min_size(egui::vec2(browse_w, 0.0)))
                        .clicked()
                    {
                        self.handle_directory_picker();
                    }
                });
            });
            ui.add_space(6.0);
            callout(ui, Tone::Warning, |ui| {
                status(
                    ui,
                    Tone::Warning,
                    "Changing the data directory requires restarting the application.",
                );
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let changed = self.temp_data_directory != self.data_directory;
                let valid = !self.temp_data_directory.trim().is_empty()
                    && std::path::Path::new(&self.temp_data_directory).is_absolute();
                if ui
                    .add_enabled(
                        changed && valid,
                        style::btn_primary_ctx(ui.ctx(), "Apply Changes"),
                    )
                    .clicked()
                {
                    self.apply_data_directory();
                }
                if ui.add(style::btn_secondary("Reset to Default")).clicked() {
                    self.temp_data_directory = dirs::home_dir()
                        .map(|mut p| {
                            p.push(".tabular");
                            p.to_string_lossy().to_string()
                        })
                        .unwrap_or_else(|| ".".to_string());
                }
                if changed && !valid {
                    status(ui, Tone::Danger, "Enter an absolute path.");
                }
            });
        });

        section(ui, "Backup & Restore", |ui| {
            hint(
                ui,
                "Export or restore all database connections, saved queries, HTTP API collections and query history as a portable ZIP archive.",
            );
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(style::btn_secondary("📦  Export All Data…"))
                    .clicked()
                {
                    self.show_export_all_dialog = true;
                }
                if ui
                    .add(style::btn_secondary("📥  Import & Restore…"))
                    .clicked()
                {
                    self.show_import_all_dialog = true;
                }
            });
        });
    }

    fn apply_data_directory(&mut self) {
        match crate::config::set_data_dir(&self.temp_data_directory) {
            Ok(()) => {
                self.refresh_data_directory();
                self.save_prefs_now();
                if let Some(rt) = &self.runtime
                    && let Ok(new_store) = rt.block_on(crate::config::ConfigStore::new())
                {
                    self.config_store = Some(new_store);
                    log::debug!("Config store reinitialized for new data directory");
                }
                self.set_pref_feedback("Data directory updated successfully!");
                log::debug!("Data directory changed to: {}", self.data_directory);
            }
            Err(e) => {
                self.toasts
                    .error(format!("Failed to change data directory: {}", e));
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: Updates
    // ─────────────────────────────────────────────────────────────────────

    fn render_pref_updates(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Updates",
            "Keep Tabular up to date with the latest GitHub release.",
        );

        section(ui, "Software Update", |ui| {
            row(ui, "Installed version", None, |ui| {
                ui.label(
                    egui::RichText::new(env!("CARGO_PKG_VERSION"))
                        .monospace()
                        .strong(),
                );
            });
            divider(ui);
            if toggle_row(
                ui,
                &mut self.auto_check_updates,
                "Check automatically",
                Some("Look for a new version on startup (at most once a day)."),
            ) {
                self.save_prefs_now();
            }
            divider(ui);
            row(ui, "Check now", None, |ui| {
                let checking = self.update_check_in_progress;
                if ui
                    .add_enabled(!checking, style::btn_secondary("Check for Updates"))
                    .clicked()
                {
                    self.check_for_updates(true);
                }
                if checking {
                    ui.spinner();
                    hint(ui, "Checking…");
                }
            });
            if let Some(err) = &self.update_check_error {
                status(ui, Tone::Danger, format!("Last check failed: {err}"));
            }
        });
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: AI Assistant
    // ─────────────────────────────────────────────────────────────────────

    fn render_pref_ai(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "AI Assistant",
            "Press Cmd+Shift+A in the editor to toggle the AI panel.",
        );

        self.render_ai_backend_settings(ui);

        if self.ai_backend != AiBackend::Api {
            return;
        }

        section(ui, "Provider", |ui| {
            row(ui, "Provider", None, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for p in [
                        AiProvider::OpenAI,
                        AiProvider::Anthropic,
                        AiProvider::Groq,
                        AiProvider::GitHub,
                        AiProvider::Custom,
                    ] {
                        if ui
                            .radio_value(&mut self.ai_provider, p, p.display_name())
                            .clicked()
                        {
                            // Reset model + base URL ke default provider baru.
                            self.ai_settings_model_input = p.default_model().to_string();
                            self.ai_settings_base_url_input = p.default_base_url().to_string();
                            self.ai_model = self.ai_settings_model_input.clone();
                            self.ai_base_url = self.ai_settings_base_url_input.clone();
                            self.save_prefs_now();
                        }
                    }
                });
            });
            if self.ai_provider == AiProvider::GitHub {
                ui.add_space(4.0);
                callout(ui, Tone::Info, |ui| {
                    ui.label(
                        egui::RichText::new("GitHub Copilot / Models")
                            .strong()
                            .size(12.0),
                    );
                    hint(
                        ui,
                        "Requires a GitHub Personal Access Token (PAT) with 'models:read' scope (or 'copilot' scope for Copilot subscribers).",
                    );
                    ui.hyperlink_to(
                        egui::RichText::new("Create a token at github.com/settings/tokens")
                            .size(11.0),
                        "https://github.com/settings/tokens",
                    );
                });
            }
        });

        section(ui, "Credentials", |ui| {
            row(
                ui,
                "API key",
                Some("Stored locally and only sent to the chosen provider."),
                |ui| {
                    let hint_text = self.ai_provider.api_key_hint();
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.ai_settings_api_key_input)
                            .password(true)
                            .desired_width(240.0)
                            .hint_text(hint_text),
                    );
                    if resp.lost_focus() || ui.add(style::btn_secondary("Apply")).clicked() {
                        self.ai_api_key = self.ai_settings_api_key_input.clone();
                        self.save_prefs_now();
                        self.set_pref_feedback("API key saved.");
                    }
                },
            );
            divider(ui);
            if self.ai_api_key.is_empty() {
                status(
                    ui,
                    Tone::Warning,
                    "⚠ No API key set. The AI panel will show a warning.",
                );
            } else {
                status(
                    ui,
                    Tone::Success,
                    format!("✓ Key configured: {}", mask_secret(&self.ai_api_key)),
                );
            }
        });

        section(ui, "Model", |ui| {
            row(ui, "Model", None, |ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.ai_settings_model_input)
                        .desired_width(200.0)
                        .hint_text(self.ai_provider.default_model()),
                );
                if resp.lost_focus() || ui.add(style::btn_secondary("Apply")).clicked() {
                    self.ai_model = self.ai_settings_model_input.clone();
                    self.save_prefs_now();
                }
                if ui.add(style::btn_secondary("Default")).clicked() {
                    self.ai_settings_model_input = self.ai_provider.default_model().to_string();
                    self.ai_model = self.ai_settings_model_input.clone();
                    self.save_prefs_now();
                }
            });
            if let Some(m) = quick_pick(
                ui,
                self.ai_provider.preset_models(),
                &self.ai_settings_model_input,
            ) {
                self.ai_settings_model_input = m.to_string();
                self.ai_model = m.to_string();
                self.save_prefs_now();
            }
        });

        section(ui, "Endpoint", |ui| {
            let is_custom = self.ai_provider == AiProvider::Custom;
            let label = if is_custom {
                "Server URL (required)"
            } else {
                "Base URL"
            };
            let default_url = self.ai_provider.default_base_url();
            stacked(ui, label, None, |ui| {
                ui.horizontal(|ui| {
                    let buttons_w = if is_custom { 160.0 } else { 140.0 };
                    let hint_url = if is_custom {
                        "https://localhost:11434/v1"
                    } else {
                        default_url
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.ai_settings_base_url_input)
                            .desired_width(ui.available_width() - buttons_w)
                            .hint_text(hint_url),
                    );
                    if resp.lost_focus() || ui.add(style::btn_secondary("Apply")).clicked() {
                        self.ai_base_url = self.ai_settings_base_url_input.clone();
                        self.save_prefs_now();
                    }
                    if ui.add(style::btn_secondary("Default")).clicked() {
                        self.ai_settings_base_url_input = default_url.to_string();
                        self.ai_base_url = self.ai_settings_base_url_input.clone();
                        self.save_prefs_now();
                    }
                });
            });
            if is_custom {
                hint(
                    ui,
                    "Base URL of your OpenAI-compatible server (e.g. Ollama, LM Studio).",
                );
            } else {
                hint(
                    ui,
                    format!(
                        "Default: {default_url}. Change it for OpenAI-compatible local servers (e.g. Ollama, LM Studio)."
                    ),
                );
            }
        });
    }

    // ─────────────────────────────────────────────────────────────────────
    // Halaman: Plugins
    // ─────────────────────────────────────────────────────────────────────

    fn render_pref_plugins(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Plugins",
            "Browse, run and build WebAssembly plugins for the current table.",
        );

        let db_type = self
            .current_connection_id
            .and_then(|cid| self.connections.iter().find(|c| c.id == Some(cid)))
            .map(|c| c.connection_type.clone());

        let selected_rows_vec: Vec<Vec<String>> = self
            .selected_rows
            .iter()
            .filter_map(|&idx| self.current_table_data.get(idx).cloned())
            .collect();

        crate::plugin_runtime::ui::render_plugin_panel(
            ui,
            &mut self.plugin_modal_state,
            &mut self.plugin_manager,
            &self.current_table_name,
            &self.current_table_headers,
            &selected_rows_vec,
            &self.all_table_data,
            Some(&self.structure_columns),
            self.current_column_metadata.as_deref(),
            db_type.as_ref(),
        );
    }
}

/// Tema editor yang dipakai saat tertaut ke tema aplikasi.
fn linked_editor_theme(app: AppTheme) -> EditorColorTheme {
    if app.is_dark() {
        EditorColorTheme::GithubDark
    } else {
        EditorColorTheme::GithubLight
    }
}

/// Samarkan secret: 6 karakter awal + 4 karakter akhir (aman untuk UTF-8).
pub(crate) fn mask_secret(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 10 {
        return "•".repeat(chars.len().max(4));
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_secret_keeps_head_and_tail() {
        assert_eq!(mask_secret("sk-abcdef123456wxyz"), "sk-abc…wxyz");
    }

    #[test]
    fn mask_secret_hides_short_values_entirely() {
        assert_eq!(mask_secret("abc"), "••••");
        assert_eq!(mask_secret("0123456789"), "••••••••••");
    }

    #[test]
    fn mask_secret_handles_multibyte_chars() {
        // Sebelumnya slicing byte bisa panic pada batas karakter UTF-8.
        let s = "ééééééééééééé";
        assert_eq!(mask_secret(s), "éééééé…éééé");
    }

    #[test]
    fn visible_tabs_start_with_appearance_and_end_with_plugins() {
        let tabs = PrefTab::visible();
        assert_eq!(tabs.first(), Some(&PrefTab::ApplicationTheme));
        assert_eq!(tabs.last(), Some(&PrefTab::Plugins));
        assert_eq!(
            tabs.contains(&PrefTab::Update),
            crate::self_update::SELF_UPDATE_SUPPORTED
        );
    }
}
