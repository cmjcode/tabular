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
