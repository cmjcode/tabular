use eframe::egui;
pub use crate::config::UiModePreference;


#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviceUiMetrics {
    /// True when running in touch/tablet optimized mode
    pub is_touch: bool,
    /// True when screen width is constrained (e.g. tablet portrait or small window)
    pub is_compact_screen: bool,
    /// Screen width in logical points
    pub screen_width: f32,
    /// Screen height in logical points
    pub screen_height: f32,
    /// Minimum touch/interact target size (Apple HIG & Material minimum is 44pt for touch)
    pub min_touch_size: egui::Vec2,
    /// Button internal padding
    pub button_padding: egui::Vec2,
    /// Body font size
    pub font_body_size: f32,
    /// Monospace / editor font size
    pub font_monospace_size: f32,
    /// Heading font size
    pub font_heading_size: f32,
    /// Row height in data table / spreadsheet
    pub table_row_height: f32,
    /// Item height in sidebar trees (connections, tables, folders)
    pub sidebar_item_height: f32,
    /// Tab button height in top bars
    pub tab_button_height: f32,
    /// Scrollbar width
    pub scrollbar_width: f32,
    /// Extra margin/spacing around major panels
    pub panel_margin: egui::Margin,
}

impl Default for DeviceUiMetrics {
    fn default() -> Self {
        Self::desktop(1280.0, 800.0)
    }
}

impl DeviceUiMetrics {
    pub fn desktop(width: f32, height: f32) -> Self {
        Self {
            is_touch: false,
            is_compact_screen: width < 900.0,
            screen_width: width,
            screen_height: height,
            min_touch_size: egui::vec2(28.0, 26.0),
            button_padding: egui::vec2(12.0, 6.0),
            font_body_size: 14.0,
            font_monospace_size: 13.0,
            font_heading_size: 18.0,
            table_row_height: 28.0,
            sidebar_item_height: 28.0,
            tab_button_height: 38.0,
            scrollbar_width: 7.0,
            panel_margin: egui::Margin::same(6),
        }
    }

    pub fn touch_tablet(width: f32, height: f32) -> Self {
        Self {
            is_touch: true,
            is_compact_screen: width < 960.0,
            screen_width: width,
            screen_height: height,
            min_touch_size: egui::vec2(36.0, 32.0),
            button_padding: egui::vec2(12.0, 8.0),
            font_body_size: 14.5,
            font_monospace_size: 13.5,
            font_heading_size: 18.0,
            table_row_height: 36.0,
            sidebar_item_height: 36.0,
            tab_button_height: 40.0,
            scrollbar_width: 12.0,
            panel_margin: egui::Margin::same(8),
        }
    }

    /// Compute device metrics based on current context and user preference
    pub fn compute(ctx: &egui::Context, preference: UiModePreference) -> Self {
        let screen_rect = ctx.input(|i| i.raw.screen_rect).unwrap_or(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1024.0, 768.0),
        ));
        let width = screen_rect.width();
        let height = screen_rect.height();

        let is_mobile_os = cfg!(target_os = "ios") || cfg!(target_os = "android");

        let touch_detected = ctx.input(|i| {
            // Check if any touch events occurred or if raw touch inputs exist
            !i.events.is_empty() && i.events.iter().any(|e| matches!(e, egui::Event::Touch { .. }))
        });

        let is_touch = match preference {
            UiModePreference::Desktop => false,
            UiModePreference::TouchTablet => true,
            UiModePreference::Auto => {
                is_mobile_os || touch_detected || (width <= 820.0 && height >= 1000.0)
            }
        };

        if is_touch {
            Self::touch_tablet(width, height)
        } else {
            Self::desktop(width, height)
        }
    }
}
