use eframe::egui;
use crate::config::AppTheme;

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

    v.selection.bg_fill = egui::Color32::from_rgba_premultiplied(180, 160, 140, 100);
    v.window_stroke = egui::Stroke::NONE;
    v
}

fn theme_visuals(theme: AppTheme) -> egui::Visuals {
    match theme {
        AppTheme::Dark => egui::Visuals::dark(),
        AppTheme::Light => egui::Visuals::light(),
        AppTheme::LightSoft => light_soft_visuals(),
    }
}

pub fn apply_theme(ctx: &egui::Context, theme: AppTheme) {
    let visuals = theme_visuals(theme);

    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();

        // Global spacing and padding for a more modern layout.
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.window_margin = egui::Margin::same(10);
        style.spacing.button_padding = egui::vec2(12.0, 8.0);
        style.spacing.menu_margin = egui::Margin::same(8);
        style.spacing.indent = 16.0;
        style.spacing.interact_size = egui::vec2(44.0, 22.0);

        // Rounded widgets across the app.
        style.visuals.widgets.inactive.corner_radius = 8.0.into();
        style.visuals.widgets.hovered.corner_radius = 8.0.into();
        style.visuals.widgets.active.corner_radius = 8.0.into();
        style.visuals.widgets.open.corner_radius = 8.0.into();

        // Use a consistent app font / body size.
        style.override_font_id = Some(egui::FontId::new(14.0, egui::FontFamily::Proportional));
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::new(13.0, egui::FontFamily::Monospace),
        );
        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(18.0, egui::FontFamily::Proportional),
        );
    });
}

// Theme-aware status & UI color helpers
pub fn theme_accent(_ctx: &egui::Context) -> egui::Color32 {
    egui::Color32::from_rgb(255, 0, 0)
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

