use eframe::egui;
use crate::config::AppTheme;

pub fn dark_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let panel = egui::Color32::from_rgb(24, 25, 32);
    let bg = egui::Color32::from_rgb(18, 19, 24);
    let text = egui::Color32::from_rgb(226, 232, 240);
    let widget_bg = egui::Color32::from_rgb(38, 41, 52);
    let widget_bg_hovered = egui::Color32::from_rgb(52, 56, 70);
    let widget_bg_active = egui::Color32::from_rgb(67, 72, 90);
    let border_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 59, 74));

    v.override_text_color = Some(text);
    v.window_fill = bg;
    v.panel_fill = panel;
    v.faint_bg_color = egui::Color32::from_rgb(30, 32, 42);
    v.extreme_bg_color = egui::Color32::from_rgb(15, 16, 20);

    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.noninteractive.weak_bg_fill = panel;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.inactive.bg_fill = widget_bg;
    v.widgets.inactive.weak_bg_fill = widget_bg;
    v.widgets.inactive.bg_stroke = border_stroke;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.hovered.bg_fill = widget_bg_hovered;
    v.widgets.hovered.weak_bg_fill = widget_bg_hovered;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(82, 86, 110));
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    v.widgets.active.bg_fill = widget_bg_active;
    v.widgets.active.weak_bg_fill = widget_bg_active;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(98, 103, 130));
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    v.widgets.open.bg_fill = widget_bg_active;
    v.widgets.open.weak_bg_fill = widget_bg_active;
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(98, 103, 130));
    v.widgets.open.fg_stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);

    v.selection.bg_fill = egui::Color32::from_rgb(255, 0, 0);
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v
}

pub fn light_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let panel = egui::Color32::from_rgb(248, 250, 252);
    let bg = egui::Color32::from_rgb(255, 255, 255);
    let text = egui::Color32::from_rgb(15, 23, 42);
    let widget_bg = egui::Color32::from_rgb(241, 245, 249);
    let widget_bg_hovered = egui::Color32::from_rgb(226, 232, 240);
    let widget_bg_active = egui::Color32::from_rgb(203, 213, 225);
    let border_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(203, 213, 225));

    v.override_text_color = Some(text);
    v.window_fill = bg;
    v.panel_fill = panel;
    v.faint_bg_color = egui::Color32::from_rgb(241, 245, 249);
    v.extreme_bg_color = egui::Color32::from_rgb(255, 255, 255);

    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.noninteractive.weak_bg_fill = panel;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.inactive.bg_fill = widget_bg;
    v.widgets.inactive.weak_bg_fill = widget_bg;
    v.widgets.inactive.bg_stroke = border_stroke;
    v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.hovered.bg_fill = widget_bg_hovered;
    v.widgets.hovered.weak_bg_fill = widget_bg_hovered;
    v.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(148, 163, 184));
    v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.active.bg_fill = widget_bg_active;
    v.widgets.active.weak_bg_fill = widget_bg_active;
    v.widgets.active.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 116, 139));
    v.widgets.active.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.open.bg_fill = widget_bg_active;
    v.widgets.open.weak_bg_fill = widget_bg_active;
    v.widgets.open.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 116, 139));
    v.widgets.open.fg_stroke = egui::Stroke::new(1.0, text);

    v.selection.bg_fill = egui::Color32::from_rgb(255, 0, 0);
    v.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
    v
}

pub fn light_soft_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::light();
    let bg = egui::Color32::from_rgb(245, 242, 238);
    let panel = egui::Color32::from_rgb(237, 233, 227);
    let text = egui::Color32::from_rgb(55, 50, 45);
    let widget_bg = egui::Color32::from_rgb(230, 226, 219);
    let widget_bg_hovered = egui::Color32::from_rgb(218, 213, 205);
    let widget_bg_open = egui::Color32::from_rgb(210, 205, 197);

    v.override_text_color = Some(text);
    v.window_fill = bg;
    v.panel_fill = panel;
    v.faint_bg_color = egui::Color32::from_rgb(240, 237, 232);
    v.extreme_bg_color = egui::Color32::from_rgb(255, 252, 248);

    v.widgets.noninteractive.bg_fill = panel;
    v.widgets.noninteractive.weak_bg_fill = panel;
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, text);

    v.widgets.inactive.bg_fill = widget_bg;
    v.widgets.inactive.weak_bg_fill = widget_bg;

    v.widgets.hovered.bg_fill = widget_bg_hovered;
    v.widgets.hovered.weak_bg_fill = widget_bg_hovered;

    v.widgets.active.bg_fill = widget_bg_open;
    v.widgets.active.weak_bg_fill = widget_bg_open;

    v.widgets.open.bg_fill = widget_bg_open;
    v.widgets.open.weak_bg_fill = widget_bg_open;

    v.selection.bg_fill = egui::Color32::from_rgb(255, 0, 0);
    v.window_stroke = egui::Stroke::NONE;
    v
}

fn theme_visuals(theme: AppTheme) -> egui::Visuals {
    match theme {
        AppTheme::Dark => dark_visuals(),
        AppTheme::Light => light_visuals(),
        AppTheme::LightSoft => light_soft_visuals(),
    }
}

use crate::window_egui::device_profile::DeviceUiMetrics;

pub fn apply_theme(ctx: &egui::Context, theme: AppTheme, metrics: &DeviceUiMetrics) {
    let visuals = theme_visuals(theme);

    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();

        // Global spacing and padding for a modern, touch-friendly or compact desktop layout.
        style.spacing.item_spacing = if metrics.is_touch {
            egui::vec2(12.0, 10.0)
        } else {
            egui::vec2(8.0, 6.0)
        };
        style.spacing.window_margin = metrics.panel_margin;
        style.spacing.button_padding = metrics.button_padding;
        style.spacing.menu_margin = if metrics.is_touch {
            egui::Margin::same(14)
        } else {
            egui::Margin::same(8)
        };
        style.spacing.indent = if metrics.is_touch { 22.0 } else { 16.0 };
        style.spacing.interact_size = metrics.min_touch_size;
        style.spacing.scroll.bar_width = metrics.scrollbar_width;

        // Rounded widgets across the app.
        let radius = if metrics.is_touch { 10.0 } else { 6.0 };
        style.visuals.widgets.inactive.corner_radius = radius.into();
        style.visuals.widgets.hovered.corner_radius = radius.into();
        style.visuals.widgets.active.corner_radius = radius.into();
        style.visuals.widgets.open.corner_radius = radius.into();

        // Typography dynamically sized for desktop or touch tablet.
        style.override_font_id = Some(egui::FontId::new(metrics.font_body_size, egui::FontFamily::Proportional));
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(metrics.font_body_size, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(metrics.font_monospace_size, egui::FontFamily::Monospace),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(metrics.font_body_size, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(metrics.font_heading_size, egui::FontFamily::Proportional),
        );
    });
}

pub fn theme_accent(_ctx: &egui::Context) -> egui::Color32 {
    egui::Color32::from_rgb(255, 0, 0)
}

// Standardized Button Builders for Professional UI Theme Consistency
pub fn btn_primary_ctx<'a>(ctx: &egui::Context, text: impl Into<String>) -> egui::Button<'a> {
    let accent = theme_accent(ctx);
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .fill(accent)
    .corner_radius(6.0)
}

pub fn btn_secondary<'a>(text: impl Into<String>) -> egui::Button<'a> {
    egui::Button::new(text.into()).corner_radius(6.0)
}

pub fn btn_danger_ctx<'a>(ctx: &egui::Context, text: impl Into<String>) -> egui::Button<'a> {
    let danger = theme_danger(ctx);
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .fill(danger)
    .corner_radius(6.0)
}

pub fn btn_success_ctx<'a>(ctx: &egui::Context, text: impl Into<String>) -> egui::Button<'a> {
    let success = theme_success(ctx);
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(egui::Color32::WHITE)
            .strong(),
    )
    .fill(success)
    .corner_radius(6.0)
}

/// Unified active/inactive tab component across the app (Sidebar, Workspace Header, Sub-views, Settings)
pub fn render_custom_tab(
    ui: &mut egui::Ui,
    title: &str,
    is_active: bool,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let is_dark = ui.visuals().dark_mode;
        let is_hovered = response.hovered();

        let tab_corner = egui::CornerRadius {
            nw: 4,
            ne: 4,
            sw: 0,
            se: 0,
        };

        // 1. Background Fill (clean elevated surface, no red box fill)
        let bg_fill = if is_active {
            if is_dark {
                egui::Color32::from_rgb(40, 43, 56)
            } else {
                egui::Color32::from_rgb(255, 255, 255)
            }
        } else if is_hovered {
            if is_dark {
                egui::Color32::from_rgb(30, 33, 44)
            } else {
                egui::Color32::from_rgb(238, 242, 246)
            }
        } else {
            egui::Color32::TRANSPARENT
        };

        ui.painter().rect_filled(rect, tab_corner, bg_fill);

        // 2. Subtle Neutral Border (no red box stroke surrounding tab)
        let stroke_color = if is_active {
            if is_dark {
                egui::Color32::from_rgb(55, 60, 76)
            } else {
                egui::Color32::from_rgb(215, 222, 232)
            }
        } else if is_hovered {
            if is_dark {
                egui::Color32::from_rgb(45, 48, 62)
            } else {
                egui::Color32::from_rgb(225, 232, 240)
            }
        } else {
            egui::Color32::TRANSPARENT
        };

        if stroke_color != egui::Color32::TRANSPARENT {
            ui.painter().rect_stroke(
                rect,
                tab_corner,
                egui::Stroke::new(1.0, stroke_color),
                egui::StrokeKind::Outside,
            );
        }

        // 3. Bottom Red Line Accent (drawn only at the bottom edge for active tabs)
        if is_active {
            let line_height = 3.0;
            let bottom_accent_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.bottom() - line_height),
                egui::vec2(rect.width(), line_height),
            );
            ui.painter().rect_filled(bottom_accent_rect, 0.0, theme_accent(ui.ctx()));
        }

        // 4. Text
        let text_color = if is_active {
            if is_dark {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgb(15, 23, 42)
            }
        } else if is_hovered {
            if is_dark {
                egui::Color32::from_rgb(226, 232, 240)
            } else {
                egui::Color32::from_rgb(30, 41, 59)
            }
        } else {
            if is_dark {
                egui::Color32::from_rgb(150, 160, 175)
            } else {
                egui::Color32::from_rgb(100, 116, 139)
            }
        };

        let tab_font_size = (size.y * 0.32).clamp(13.0, 16.0);
        let font_id = egui::FontId::new(tab_font_size, egui::FontFamily::Proportional);

        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            title,
            font_id,
            text_color,
        );
    }
    response
}

/// Compact icon-only sub-tab used for secondary navigation nested inside a main tab
/// (e.g. Connections/Queries/History inside "Database"). Deliberately flat — no
/// elevated card background, no border, no rounded-top-corner shape — so its active
/// state reads differently from `render_custom_tab` and the two levels don't get
/// confused. Mirrors VS Code's flat, underline-accented secondary tabs.
pub fn render_sidebar_subtab(
    ui: &mut egui::Ui,
    icon: &str,
    is_active: bool,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let is_dark = ui.visuals().dark_mode;
        let is_hovered = response.hovered();

        // Soft, fully-rounded highlight (not a card) — only on hover/active.
        if is_active || is_hovered {
            let bg = if is_active {
                if is_dark {
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)
                } else {
                    egui::Color32::from_rgba_unmultiplied(0, 0, 0, 14)
                }
            } else if is_dark {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 8)
            } else {
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 6)
            };
            ui.painter().rect_filled(rect, 4.0, bg);
        }

        let icon_color = if is_active {
            if is_dark {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgb(15, 23, 42)
            }
        } else if is_hovered {
            if is_dark {
                egui::Color32::from_rgb(210, 216, 226)
            } else {
                egui::Color32::from_rgb(50, 60, 75)
            }
        } else if is_dark {
            egui::Color32::from_rgb(130, 138, 150)
        } else {
            egui::Color32::from_rgb(140, 148, 162)
        };
        let font_size = (size.y * 0.50).clamp(14.0, 18.0);
        let font_id = egui::FontId::new(font_size, egui::FontFamily::Proportional);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            font_id,
            icon_color,
        );

        // Thin, short underline accent — distinct from the main tab's thicker,
        // full-width bottom line.
        if is_active {
            let underline_rect = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.bottom() - 1.0),
                egui::vec2(rect.width() * 0.5, 2.0),
            );
            ui.painter().rect_filled(underline_rect, 1.0, theme_accent(ui.ctx()));
        }
    }
    response
}

pub fn theme_danger(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(220, 70, 70) // Soft ergonomic red
    } else {
        egui::Color32::from_rgb(220, 38, 38)
    }
}

pub fn theme_success(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(34, 197, 94) // Solid green
    } else {
        egui::Color32::from_rgb(22, 163, 74)
    }
}

pub fn theme_warning(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(234, 179, 8) // Solid warm amber
    } else {
        egui::Color32::from_rgb(202, 138, 4)
    }
}

pub fn theme_info(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(96, 165, 250)
    } else {
        egui::Color32::from_rgb(37, 99, 235)
    }
}

pub fn theme_muted_text(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(160, 165, 175)
    } else {
        egui::Color32::from_rgb(110, 115, 125)
    }
}

pub fn theme_card_frame(ctx: &egui::Context) -> egui::Frame {
    let visuals = &ctx.global_style().visuals;
    let bg = if visuals.dark_mode {
        egui::Color32::from_rgb(32, 34, 40)
    } else {
        egui::Color32::from_rgb(250, 250, 252)
    };
    let stroke_col = if visuals.dark_mode {
        egui::Color32::from_rgb(50, 54, 64)
    } else {
        egui::Color32::from_rgb(220, 224, 230)
    };
    egui::Frame::group(&ctx.global_style())
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, stroke_col))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::same(10))
}

pub fn theme_alert_frame(ctx: &egui::Context, is_danger: bool) -> egui::Frame {
    let visuals = &ctx.global_style().visuals;
    let (bg, stroke_col) = if is_danger {
        if visuals.dark_mode {
            (egui::Color32::from_rgb(60, 25, 28), egui::Color32::from_rgb(180, 60, 60))
        } else {
            (egui::Color32::from_rgb(255, 235, 238), egui::Color32::from_rgb(230, 100, 100))
        }
    } else {
        if visuals.dark_mode {
            (egui::Color32::from_rgb(25, 45, 30), egui::Color32::from_rgb(60, 150, 80))
        } else {
            (egui::Color32::from_rgb(235, 248, 238), egui::Color32::from_rgb(100, 200, 120))
        }
    };
    egui::Frame::group(&ctx.global_style())
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, stroke_col))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(8))
}

pub fn render_badge(ui: &mut egui::Ui, text: &str, bg_color: egui::Color32, fg_color: egui::Color32) {
    egui::Frame::new()
        .fill(bg_color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(11.0).color(fg_color).strong());
        });
}

pub fn render_close_icon_button(ui: &mut egui::Ui) -> egui::Response {
    let size = egui::vec2(20.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let hover = response.hovered();
        let bg_color = if hover {
            if ui.visuals().dark_mode {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30)
            } else {
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 25)
            }
        } else {
            egui::Color32::TRANSPARENT
        };

        if hover {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(10u8),
                bg_color,
            );
        }

        let icon_color = if hover {
            ui.visuals().widgets.hovered.fg_stroke.color
        } else {
            ui.visuals().text_color().linear_multiply(0.6)
        };

        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(15.0),
            icon_color,
        );
    }

    response.on_hover_text("Close")
}

