use crate::config::AppTheme;
use eframe::egui;

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
    // Latar semua text box; sedikit lebih terang dari panel agar terbaca sebagai field.
    v.text_edit_bg_color = Some(egui::Color32::from_rgb(30, 31, 36));

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
    v.text_edit_bg_color = Some(egui::Color32::from_rgb(255, 255, 255));

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
    v.text_edit_bg_color = Some(egui::Color32::from_rgb(255, 252, 248));

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
            egui::vec2(9.0, 7.0)
        } else {
            egui::vec2(8.0, 6.0)
        };
        style.spacing.window_margin = metrics.panel_margin;
        style.spacing.button_padding = metrics.button_padding;
        style.spacing.menu_margin = if metrics.is_touch {
            egui::Margin::same(10)
        } else {
            egui::Margin::same(8)
        };
        style.spacing.indent = if metrics.is_touch { 18.0 } else { 16.0 };
        style.spacing.interact_size = metrics.min_touch_size;
        style.spacing.scroll.bar_width = metrics.scrollbar_width;

        // Rounded widgets across the app.
        let radius = if metrics.is_touch { 10.0 } else { 6.0 };
        style.visuals.widgets.inactive.corner_radius = radius.into();
        style.visuals.widgets.hovered.corner_radius = radius.into();
        style.visuals.widgets.active.corner_radius = radius.into();
        style.visuals.widgets.open.corner_radius = radius.into();

        // Typography dynamically sized for desktop or touch tablet.
        style.override_font_id = Some(egui::FontId::new(
            metrics.font_body_size,
            egui::FontFamily::Proportional,
        ));
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

    // Synchronize OS-level window titlebar and frame theme with application theme
    let sys_theme = match theme {
        AppTheme::Dark => egui::SystemTheme::Dark,
        AppTheme::Light | AppTheme::LightSoft => egui::SystemTheme::Light,
    };
    let theme_id = egui::Id::new("tabular_applied_viewport_theme");
    let prev_theme = ctx.data(|d| d.get_temp::<egui::SystemTheme>(theme_id));
    if prev_theme != Some(sys_theme) {
        ctx.data_mut(|d| d.insert_temp(theme_id, sys_theme));
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(sys_theme));
    }
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

/// Tombol aksi pendamping text field (mis. Apply, Detect, Default, Browse).
/// Tingginya disesuaikan persis dengan `render_text_field` (30.0 desktop, 40.0 touch).
pub fn btn_field_action<'a>(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'a> {
    let is_touch = ui.spacing().interact_size.y >= 30.0;
    let height = if is_touch { 40.0 } else { 30.0 };
    let font_size = if is_touch { 14.5 } else { 12.5 };
    egui::Button::new(egui::RichText::new(text.into()).size(font_size))
        .min_size(egui::vec2(0.0, height))
        .corner_radius(6.0)
}

/// Tombol aksi utama pendamping text field dengan warna aksen (primary).
pub fn btn_field_action_primary<'a>(ui: &egui::Ui, text: impl Into<String>) -> egui::Button<'a> {
    let accent = theme_accent(ui.ctx());
    let is_touch = ui.spacing().interact_size.y >= 30.0;
    let height = if is_touch { 40.0 } else { 30.0 };
    let font_size = if is_touch { 14.5 } else { 12.5 };
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(egui::Color32::WHITE)
            .strong()
            .size(font_size),
    )
    .fill(accent)
    .min_size(egui::vec2(0.0, height))
    .corner_radius(6.0)
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

// ── Token warna navigasi sidebar ─────────────────────────────────────────────
// Satu palet abu netral (tanpa campuran abu kebiruan) supaya sidebar, tab,
// segmented control, dan search box terasa satu keluarga dengan editor.

/// Permukaan dasar sidebar.
pub fn nav_surface(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(20, 20, 20)
    } else {
        egui::Color32::from_rgb(245, 245, 245)
    }
}

/// Permukaan cekung: track segmented control & field pencarian.
pub fn nav_track(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(30, 30, 30)
    } else {
        egui::Color32::from_rgb(233, 233, 235)
    }
}

/// Permukaan terangkat: segmen aktif.
pub fn nav_raised(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(52, 52, 54)
    } else {
        egui::Color32::from_rgb(255, 255, 255)
    }
}

/// Garis pemisah / border tipis.
pub fn nav_border(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(44, 44, 46)
    } else {
        egui::Color32::from_rgb(218, 218, 222)
    }
}

/// Teks/ikon utama (aktif).
pub fn nav_text_strong(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(236, 236, 238)
    } else {
        egui::Color32::from_rgb(24, 24, 27)
    }
}

/// Teks/ikon sekunder (non-aktif, hint).
pub fn nav_text_muted(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(140, 140, 146)
    } else {
        egui::Color32::from_rgb(113, 113, 122)
    }
}

/// Tab level-1 (Sidebar, header Data/Structure/Query, sub-view Structure).
/// Gaya underline murni: tanpa fill/border, hanya warna teks + garis aksen 2px
/// pada tab aktif. Ini satu-satunya tempat aksen merah dipakai di navigasi.
pub fn render_custom_tab(
    ui: &mut egui::Ui,
    title: &str,
    is_active: bool,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx().clone();
        let text_color = if is_active || response.hovered() {
            nav_text_strong(&ctx)
        } else {
            nav_text_muted(&ctx)
        };

        let font_size = (size.y * 0.30).clamp(13.0, 15.0);
        let font_id = egui::FontId::new(font_size, egui::FontFamily::Proportional);
        let galley = ui
            .painter()
            .layout_no_wrap(title.to_string(), font_id, text_color);
        let text_pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(text_pos, galley, text_color);

        if is_active {
            let line_height = 2.0;
            let accent_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left() + 6.0, rect.bottom() - line_height),
                egui::pos2(rect.right() - 6.0, rect.bottom()),
            );
            ui.painter()
                .rect_filled(accent_rect, 1.0, theme_accent(&ctx));
        }
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Text box standar untuk seluruh aplikasi (form, preferences, dialog).
/// Frame digambar manual supaya tinggi, padding, radius, dan border fokus
/// konsisten — `TextEdit` bawaan egui meng-hardcode margin (4,2) dan memakai
/// `selection.stroke` (putih) sebagai border fokus.
///
/// `edit` diteruskan apa adanya, jadi `.password()`, `.hint_text()`, dll tetap
/// bisa dipakai pemanggil. `width`: `f32::INFINITY` = isi seluruh lebar tersedia.
/// `icon`: ikon opsional di sisi kiri (mis. ikon search).
pub fn render_text_field(
    ui: &mut egui::Ui,
    edit: egui::TextEdit<'_>,
    width: f32,
    icon: Option<&str>,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    // Mode touch memakai interact_size yang lebih besar (lihat DeviceUiMetrics).
    let is_touch = ui.spacing().interact_size.y >= 30.0;
    let height = if is_touch { 40.0 } else { 30.0 };
    let font_size = if is_touch { 15.5 } else { 13.0 };
    let width = width.min(ui.available_width()).max(40.0);
    let radius = visuals.widgets.inactive.corner_radius;
    let muted = nav_text_muted(ui.ctx());

    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, radius, visuals.text_edit_bg_color());

    let mut text_left = 9.0;
    if let Some(icon) = icon {
        let icon_galley = ui.painter().layout_no_wrap(
            icon.to_string(),
            egui::FontId::proportional(font_size + 3.0),
            muted,
        );
        let icon_w = icon_galley.size().x;
        ui.painter().galley(
            egui::pos2(
                rect.left() + text_left,
                rect.center().y - icon_galley.size().y / 2.0,
            ),
            icon_galley,
            muted,
        );
        text_left += icon_w + 6.0;
    }

    let edit_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + text_left, rect.top()),
        egui::pos2(rect.right() - 8.0, rect.bottom()),
    );
    let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect).layout(
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
    ));
    let response = child_ui.add(
        edit.frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .desired_width(f32::INFINITY)
            .vertical_align(egui::Align::Center)
            .font(egui::FontId::proportional(font_size)),
    );

    let is_hovered = response.hovered() || ui.rect_contains_pointer(rect);
    let border = if response.has_focus() {
        visuals.widgets.active.bg_stroke.color
    } else if is_hovered {
        visuals.widgets.hovered.bg_stroke.color
    } else {
        visuals.widgets.inactive.bg_stroke.color
    };
    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    response
}

/// Field pencarian/filter standar: `render_text_field` + ikon search.
pub fn render_search_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
) -> egui::Response {
    let muted = nav_text_muted(ui.ctx());
    render_text_field(
        ui,
        egui::TextEdit::singleline(text).hint_text(egui::RichText::new(hint).color(muted)),
        width,
        Some(egui_icons::icons::ICON_SEARCH.codepoint),
    )
}

/// Satu item segmented control: (key, ikon, label).
pub struct NavSegment<'a> {
    pub key: &'a str,
    pub icon: &'a str,
    pub label: &'a str,
}

/// Segmented control untuk navigasi level-2 (mis. Connections/Queries/History).
/// Sengaja netral (tanpa aksen merah) agar hierarkinya jelas di bawah tab level-1.
///
/// Label ditampilkan adaptif: bila lebar cukup semua segmen berlabel; bila sempit,
/// hanya segmen aktif yang berlabel dan sisanya ikon saja (dengan tooltip).
/// Mengembalikan key segmen yang diklik pada frame ini.
pub fn render_segmented_nav<'a>(
    ui: &mut egui::Ui,
    id_salt: &str,
    segments: &[NavSegment<'a>],
    selected: &str,
    height: f32,
) -> Option<&'a str> {
    let n = segments.len();
    if n == 0 {
        return None;
    }
    let ctx = ui.ctx().clone();
    let width = ui.available_width().max(40.0);
    let (track_rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());

    let track_pad = 3.0;
    let seg_gap = 2.0;
    let inner = track_rect.shrink(track_pad);
    let inner_w = inner.width() - seg_gap * (n as f32 - 1.0);

    let icon_font = egui::FontId::proportional((height * 0.46).clamp(15.0, 19.0));
    let label_font = egui::FontId::proportional(if height >= 36.0 { 14.0 } else { 12.5 });
    let icon_label_gap = 6.0;
    let h_pad = 10.0;

    // Ukur kebutuhan lebar tiap segmen jika memakai label.
    let painter = ui.painter().clone();
    let measure = |text: &str, font: &egui::FontId| {
        painter
            .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    let icon_w: Vec<f32> = segments
        .iter()
        .map(|s| measure(s.icon, &icon_font))
        .collect();
    let full_w: Vec<f32> = segments
        .iter()
        .zip(&icon_w)
        .map(|(s, iw)| iw + icon_label_gap + measure(s.label, &label_font) + h_pad * 2.0)
        .collect();
    let max_full = full_w.iter().cloned().fold(0.0, f32::max);
    let sum_full: f32 = full_w.iter().sum();

    let active_idx = segments.iter().position(|s| s.key == selected);
    let compact_min = 34.0;

    // Tentukan lebar & visibilitas label per segmen.
    let (widths, show_label): (Vec<f32>, Vec<bool>) = if max_full * n as f32 <= inner_w {
        (vec![inner_w / n as f32; n], vec![true; n])
    } else if sum_full <= inner_w {
        let extra = (inner_w - sum_full) / n as f32;
        (full_w.iter().map(|w| w + extra).collect(), vec![true; n])
    } else if let Some(ai) =
        active_idx.filter(|&ai| n > 1 && full_w[ai] + compact_min * (n as f32 - 1.0) <= inner_w)
    {
        let rest = (inner_w - full_w[ai]) / (n as f32 - 1.0);
        (
            (0..n)
                .map(|i| if i == ai { full_w[ai] } else { rest })
                .collect(),
            (0..n).map(|i| i == ai).collect(),
        )
    } else {
        (vec![inner_w / n as f32; n], vec![false; n])
    };

    // Track.
    painter.rect_filled(track_rect, 6.0, nav_track(&ctx));

    let mut clicked = None;
    let mut x = inner.left();
    for (i, seg) in segments.iter().enumerate() {
        let seg_rect = egui::Rect::from_min_size(
            egui::pos2(x, inner.top()),
            egui::vec2(widths[i], inner.height()),
        );
        x += widths[i] + seg_gap;

        let mut resp = ui
            .interact(
                seg_rect,
                egui::Id::new((id_salt, seg.key)),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if !show_label[i] {
            resp = resp.on_hover_text(seg.label);
        }
        if resp.clicked() {
            clicked = Some(seg.key);
        }

        let is_active = seg.key == selected;
        if is_active {
            painter.rect_filled(seg_rect, 4.0, nav_raised(&ctx));
            painter.rect_stroke(
                seg_rect,
                4.0,
                egui::Stroke::new(1.0, nav_border(&ctx)),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            let hover = if ctx.global_style().visuals.dark_mode {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 10)
            } else {
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 8)
            };
            painter.rect_filled(seg_rect, 4.0, hover);
        }

        let color = if is_active || resp.hovered() {
            nav_text_strong(&ctx)
        } else {
            nav_text_muted(&ctx)
        };

        let icon_galley = painter.layout_no_wrap(seg.icon.to_string(), icon_font.clone(), color);
        if show_label[i] {
            let label_galley =
                painter.layout_no_wrap(seg.label.to_string(), label_font.clone(), color);
            let content_w = icon_galley.size().x + icon_label_gap + label_galley.size().x;
            let left = seg_rect.center().x - content_w / 2.0;
            let cy = seg_rect.center().y;
            painter.galley(
                egui::pos2(left, cy - icon_galley.size().y / 2.0),
                icon_galley.clone(),
                color,
            );
            painter.galley(
                egui::pos2(
                    left + icon_galley.size().x + icon_label_gap,
                    cy - label_galley.size().y / 2.0,
                ),
                label_galley,
                color,
            );
        } else {
            painter.galley(
                seg_rect.center() - icon_galley.size() / 2.0,
                icon_galley,
                color,
            );
        }
    }
    clicked
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
            (
                egui::Color32::from_rgb(60, 25, 28),
                egui::Color32::from_rgb(180, 60, 60),
            )
        } else {
            (
                egui::Color32::from_rgb(255, 235, 238),
                egui::Color32::from_rgb(230, 100, 100),
            )
        }
    } else {
        if visuals.dark_mode {
            (
                egui::Color32::from_rgb(25, 45, 30),
                egui::Color32::from_rgb(60, 150, 80),
            )
        } else {
            (
                egui::Color32::from_rgb(235, 248, 238),
                egui::Color32::from_rgb(100, 200, 120),
            )
        }
    };
    egui::Frame::group(&ctx.global_style())
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, stroke_col))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(8))
}

// ─── Palet panel AI Assistant ───────────────────────────────────────────────

/// Latar panel AI Assistant (dipakai juga oleh frame `Panel::right`).
pub fn ai_panel_bg(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(24, 26, 32)
    } else {
        egui::Color32::from_rgb(247, 248, 250)
    }
}

/// Permukaan terangkat di panel AI: composer, kartu edit, blok status.
pub fn ai_surface(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(33, 35, 43)
    } else {
        egui::Color32::WHITE
    }
}

/// Garis tepi halus untuk elemen di panel AI.
pub fn ai_border(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(52, 56, 66)
    } else {
        egui::Color32::from_rgb(218, 222, 230)
    }
}

/// Latar gelembung pesan pengguna.
pub fn ai_user_bubble(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(45, 49, 62)
    } else {
        egui::Color32::from_rgb(232, 236, 246)
    }
}

/// Chip kecil (konteks tab, jumlah tabel, nama tool). Pakai `Sense::hover()`
/// untuk chip informasi dan `Sense::click()` untuk chip yang bisa diklik,
/// sehingga keduanya punya tinggi dan bentuk yang sama.
pub fn ai_chip(ui: &mut egui::Ui, text: egui::RichText, sense: egui::Sense) -> egui::Response {
    let ctx = ui.ctx().clone();
    ui.add(
        egui::Button::new(text.size(11.0))
            .fill(ai_surface(&ctx))
            .stroke(egui::Stroke::new(1.0, ai_border(&ctx)))
            .corner_radius(10.0)
            .min_size(egui::vec2(0.0, 20.0))
            .sense(sense),
    )
}

/// Warna judul markdown di jawaban AI per level (1 = paling besar).
pub fn ai_heading_color(ctx: &egui::Context, level: u8) -> egui::Color32 {
    let dark = ctx.global_style().visuals.dark_mode;
    let (d, l) = match level {
        1 => ((96, 165, 250), (37, 99, 235)),   // biru
        2 => ((129, 140, 248), (79, 70, 229)),  // indigo
        3 => ((192, 132, 252), (147, 51, 234)), // ungu
        _ => ((45, 212, 191), (13, 148, 136)),  // teal
    };
    let (r, g, b) = if dark { d } else { l };
    egui::Color32::from_rgb(r, g, b)
}

/// Latar isi code block di jawaban AI (sedikit lebih gelap dari panel).
pub fn ai_code_bg(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(18, 20, 26)
    } else {
        egui::Color32::from_rgb(246, 248, 250)
    }
}

/// Latar bilah judul code block (label bahasa + tombol Copy).
pub fn ai_code_header_bg(ctx: &egui::Context) -> egui::Color32 {
    if ctx.global_style().visuals.dark_mode {
        egui::Color32::from_rgb(30, 33, 42)
    } else {
        egui::Color32::from_rgb(234, 237, 243)
    }
}

/// Frame pemberitahuan berwarna (peringatan/error) dengan latar tipis dari `color`.
pub fn ai_notice_frame(color: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.55)))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
}

/// Tombol ikon tanpa bingkai (bingkai hanya muncul saat hover), ukuran seragam.
pub fn ai_icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    let muted = theme_muted_text(ui.ctx());
    ui.add(
        egui::Button::new(egui::RichText::new(icon).size(15.0).color(muted))
            .frame_when_inactive(false)
            .corner_radius(5.0)
            .min_size(egui::vec2(26.0, 24.0)),
    )
    .on_hover_text(tooltip)
}

pub fn render_badge(
    ui: &mut egui::Ui,
    text: &str,
    bg_color: egui::Color32,
    fg_color: egui::Color32,
) {
    egui::Frame::new()
        .fill(bg_color)
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(11.0)
                    .color(fg_color)
                    .strong(),
            );
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
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(10u8), bg_color);
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

/// Smooth cubic easing out for fluid UI transitions
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Helper to get an animated 0.0 -> 1.0 modal presentation factor
pub fn animate_modal_progress(
    ctx: &egui::Context,
    id_source: &str,
    open: bool,
    duration_secs: f32,
) -> f32 {
    let raw = ctx.animate_value_with_time(
        egui::Id::new(id_source),
        if open { 1.0 } else { 0.0 },
        duration_secs,
    );
    ease_out_cubic(raw)
}

/// Render a smooth dimming backdrop overlay behind modals and spotlights
pub fn render_modal_backdrop(ctx: &egui::Context, id_source: &str, open: bool) -> f32 {
    let progress = animate_modal_progress(ctx, id_source, open, 0.18);
    if progress > 0.01 {
        let is_dark = ctx.global_style().visuals.dark_mode;
        let max_alpha = if is_dark { 200 } else { 130 };
        let alpha = (max_alpha as f32 * progress) as u8;
        let screen_rect = ctx.content_rect();
        let fill_color = if is_dark {
            egui::Color32::from_rgba_unmultiplied(10, 12, 18, alpha)
        } else {
            egui::Color32::from_rgba_unmultiplied(15, 23, 42, alpha)
        };

        egui::Area::new(egui::Id::new(format!("{}_backdrop_area", id_source)))
            .order(egui::Order::Middle)
            .fixed_pos(screen_rect.min)
            .show(ctx, |ui| {
                ui.painter().rect_filled(screen_rect, 0.0, fill_color);
            });
    }
    progress
}

/// Render a modern macOS/Linear style keyboard shortcut pill badge
pub fn render_shortcut_badge(ui: &mut egui::Ui, shortcut: &str) {
    let is_dark = ui.visuals().dark_mode;
    let bg = if is_dark {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 15)
    };
    let border = if is_dark {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 30)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 25)
    };
    let fg = ui.visuals().text_color().linear_multiply(0.85);

    egui::Frame::new()
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, border))
        .corner_radius(egui::CornerRadius::same(4u8))
        .inner_margin(egui::Margin::symmetric(5, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(shortcut)
                    .size(10.5)
                    .family(egui::FontFamily::Monospace)
                    .color(fg),
            );
        });
}

/// Render an interactive execution time / row count badge (e.g. ⚡ 14ms • 200 rows)
pub fn render_execution_pill(ui: &mut egui::Ui, duration_ms: u128, row_count: usize) {
    let is_dark = ui.visuals().dark_mode;
    let bg = if is_dark {
        egui::Color32::from_rgb(16, 44, 32)
    } else {
        egui::Color32::from_rgb(220, 248, 230)
    };
    let stroke = if is_dark {
        egui::Color32::from_rgb(34, 134, 80)
    } else {
        egui::Color32::from_rgb(70, 180, 110)
    };
    let text_col = if is_dark {
        egui::Color32::from_rgb(110, 235, 160)
    } else {
        egui::Color32::from_rgb(20, 110, 55)
    };

    egui::Frame::new()
        .fill(bg)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(egui::CornerRadius::same(12u8))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 0.0);
                ui.label(egui::RichText::new("⚡").size(11.0).color(text_col));
                ui.label(
                    egui::RichText::new(format!("{}ms • {} rows", duration_ms, row_count))
                        .size(11.5)
                        .strong()
                        .color(text_col),
                );
            });
        });
}

/// Mengembalikan pasangan warna `(icon_color, text_color)` untuk tipe node di sidebar tree.
/// Membedakan Databases, Tables, Columns, Views, Stored Procedures, Triggers, DBA Views, dll.
pub fn sidebar_node_colors(
    node_type: &crate::models::enums::NodeType,
    is_dark: bool,
) -> (egui::Color32, egui::Color32) {
    use crate::models::enums::NodeType;
    if is_dark {
        match node_type {
            NodeType::DatabasesFolder | NodeType::Database => (
                egui::Color32::from_rgb(251, 191, 36), // Amber-400
                egui::Color32::from_rgb(253, 230, 138), // Amber-200
            ),
            NodeType::TablesFolder | NodeType::Table => (
                egui::Color32::from_rgb(56, 189, 248), // Sky-400
                egui::Color32::from_rgb(186, 230, 253), // Sky-200
            ),
            NodeType::ColumnsFolder | NodeType::Column => (
                egui::Color32::from_rgb(45, 212, 191), // Teal-400
                egui::Color32::from_rgb(153, 246, 228), // Teal-200
            ),
            NodeType::PrimaryKeysFolder => (
                egui::Color32::from_rgb(252, 211, 77), // Gold-300
                egui::Color32::from_rgb(254, 240, 138), // Gold-200
            ),
            NodeType::IndexesFolder | NodeType::Index => (
                egui::Color32::from_rgb(251, 146, 60), // Orange-400
                egui::Color32::from_rgb(254, 215, 170), // Orange-200
            ),
            NodeType::PartitionsFolder => (
                egui::Color32::from_rgb(167, 139, 250), // Purple-400
                egui::Color32::from_rgb(221, 214, 254), // Purple-200
            ),
            NodeType::ViewsFolder | NodeType::View | NodeType::CustomView => (
                egui::Color32::from_rgb(52, 211, 153), // Emerald-400
                egui::Color32::from_rgb(167, 243, 208), // Emerald-200
            ),
            NodeType::StoredProceduresFolder | NodeType::StoredProcedure => (
                egui::Color32::from_rgb(192, 132, 252), // Purple-400
                egui::Color32::from_rgb(233, 213, 255), // Purple-200
            ),
            NodeType::UserFunctionsFolder | NodeType::UserFunction => (
                egui::Color32::from_rgb(167, 139, 250), // Violet-400
                egui::Color32::from_rgb(221, 214, 254), // Violet-200
            ),
            NodeType::TriggersFolder
            | NodeType::Trigger
            | NodeType::EventsFolder
            | NodeType::Event => (
                egui::Color32::from_rgb(251, 113, 133), // Rose-400
                egui::Color32::from_rgb(254, 205, 211), // Rose-200
            ),
            NodeType::DBAViewsFolder => (
                egui::Color32::from_rgb(251, 113, 133), // Rose-400
                egui::Color32::from_rgb(254, 205, 211), // Rose-200
            ),
            NodeType::UsersFolder => (
                egui::Color32::from_rgb(96, 165, 250), // Blue-400
                egui::Color32::from_rgb(191, 219, 254), // Blue-200
            ),
            NodeType::PrivilegesFolder => (
                egui::Color32::from_rgb(251, 191, 36), // Amber-400
                egui::Color32::from_rgb(253, 230, 138), // Amber-200
            ),
            NodeType::ProcessesFolder => (
                egui::Color32::from_rgb(74, 222, 128), // Green-400
                egui::Color32::from_rgb(187, 247, 208), // Green-200
            ),
            NodeType::StatusFolder => (
                egui::Color32::from_rgb(167, 139, 250), // Purple-400
                egui::Color32::from_rgb(221, 214, 254), // Purple-200
            ),
            NodeType::BlockedQueriesFolder => (
                egui::Color32::from_rgb(248, 113, 113), // Red-400
                egui::Color32::from_rgb(254, 202, 202), // Red-200
            ),
            NodeType::ReplicationStatusFolder | NodeType::MasterStatusFolder => (
                egui::Color32::from_rgb(45, 212, 191), // Teal-400
                egui::Color32::from_rgb(153, 246, 228), // Teal-200
            ),
            NodeType::MetricsUserActiveFolder => (
                egui::Color32::from_rgb(129, 140, 248), // Indigo-400
                egui::Color32::from_rgb(199, 210, 254), // Indigo-200
            ),
            NodeType::DiagramsFolder | NodeType::Diagram => (
                egui::Color32::from_rgb(147, 197, 253), // Sky-300
                egui::Color32::from_rgb(224, 242, 254), // Sky-100
            ),
            NodeType::QueryFolder | NodeType::Query => (
                egui::Color32::from_rgb(148, 163, 184), // Slate-400
                egui::Color32::from_rgb(226, 232, 240), // Slate-200
            ),
            _ => (
                egui::Color32::from_rgb(203, 213, 225),
                egui::Color32::from_rgb(226, 232, 240),
            ),
        }
    } else {
        // Light mode
        match node_type {
            NodeType::DatabasesFolder | NodeType::Database => (
                egui::Color32::from_rgb(180, 83, 9), // Amber-700
                egui::Color32::from_rgb(120, 53, 15), // Amber-900
            ),
            NodeType::TablesFolder | NodeType::Table => (
                egui::Color32::from_rgb(2, 132, 199), // Sky-600
                egui::Color32::from_rgb(12, 74, 110), // Sky-900
            ),
            NodeType::ColumnsFolder | NodeType::Column => (
                egui::Color32::from_rgb(13, 148, 136), // Teal-600
                egui::Color32::from_rgb(19, 78, 74), // Teal-900
            ),
            NodeType::PrimaryKeysFolder => (
                egui::Color32::from_rgb(202, 138, 4), // Gold-600
                egui::Color32::from_rgb(113, 63, 18), // Gold-900
            ),
            NodeType::IndexesFolder | NodeType::Index => (
                egui::Color32::from_rgb(234, 88, 12), // Orange-600
                egui::Color32::from_rgb(124, 45, 18), // Orange-900
            ),
            NodeType::PartitionsFolder => (
                egui::Color32::from_rgb(124, 58, 237), // Purple-600
                egui::Color32::from_rgb(76, 29, 149), // Purple-900
            ),
            NodeType::ViewsFolder | NodeType::View | NodeType::CustomView => (
                egui::Color32::from_rgb(5, 150, 105), // Emerald-600
                egui::Color32::from_rgb(6, 78, 59), // Emerald-900
            ),
            NodeType::StoredProceduresFolder | NodeType::StoredProcedure => (
                egui::Color32::from_rgb(147, 51, 234), // Purple-600
                egui::Color32::from_rgb(88, 28, 135), // Purple-900
            ),
            NodeType::UserFunctionsFolder | NodeType::UserFunction => (
                egui::Color32::from_rgb(124, 58, 237), // Violet-600
                egui::Color32::from_rgb(76, 29, 149), // Violet-900
            ),
            NodeType::TriggersFolder
            | NodeType::Trigger
            | NodeType::EventsFolder
            | NodeType::Event => (
                egui::Color32::from_rgb(225, 29, 72), // Rose-600
                egui::Color32::from_rgb(136, 19, 55), // Rose-900
            ),
            NodeType::DBAViewsFolder => (
                egui::Color32::from_rgb(225, 29, 72), // Rose-600
                egui::Color32::from_rgb(136, 19, 55), // Rose-900
            ),
            NodeType::UsersFolder => (
                egui::Color32::from_rgb(29, 78, 216), // Blue-700
                egui::Color32::from_rgb(30, 58, 138), // Blue-900
            ),
            NodeType::PrivilegesFolder => (
                egui::Color32::from_rgb(180, 83, 9), // Amber-700
                egui::Color32::from_rgb(120, 53, 15), // Amber-900
            ),
            NodeType::ProcessesFolder => (
                egui::Color32::from_rgb(22, 163, 74), // Green-600
                egui::Color32::from_rgb(20, 83, 45), // Green-900
            ),
            NodeType::StatusFolder => (
                egui::Color32::from_rgb(124, 58, 237), // Purple-600
                egui::Color32::from_rgb(76, 29, 149), // Purple-900
            ),
            NodeType::BlockedQueriesFolder => (
                egui::Color32::from_rgb(220, 38, 38), // Red-600
                egui::Color32::from_rgb(127, 29, 29), // Red-900
            ),
            NodeType::ReplicationStatusFolder | NodeType::MasterStatusFolder => (
                egui::Color32::from_rgb(13, 148, 136), // Teal-600
                egui::Color32::from_rgb(19, 78, 74), // Teal-900
            ),
            NodeType::MetricsUserActiveFolder => (
                egui::Color32::from_rgb(67, 56, 202), // Indigo-700
                egui::Color32::from_rgb(49, 46, 129), // Indigo-900
            ),
            NodeType::DiagramsFolder | NodeType::Diagram => (
                egui::Color32::from_rgb(2, 132, 199), // Sky-600
                egui::Color32::from_rgb(12, 74, 110), // Sky-900
            ),
            NodeType::QueryFolder | NodeType::Query => (
                egui::Color32::from_rgb(100, 116, 139), // Slate-500
                egui::Color32::from_rgb(30, 41, 59), // Slate-800
            ),
            _ => (
                egui::Color32::from_rgb(71, 85, 105),
                egui::Color32::from_rgb(15, 23, 42),
            ),
        }
    }
}

/// Menentukan warna teks sel dan penanda italic (untuk NULL).
/// Mengombinasikan ColumnMetadata (jika ada) dan parsing nilai cerdas (fallback).
pub fn table_cell_style(
    cell: &str,
    col_type_hint: Option<&str>,
    is_dark: bool,
) -> (egui::Color32, bool) {
    let trimmed = cell.trim();

    // 1. Cek NULL
    if trimmed.is_empty() || trimmed == "NULL" || trimmed.eq_ignore_ascii_case("null") {
        let null_color = if is_dark {
            egui::Color32::from_rgb(148, 163, 184) // Slate-400
        } else {
            egui::Color32::from_rgb(100, 116, 139) // Slate-500
        };
        return (null_color, true); // Italic = true
    }

    // 2. Cek Boolean
    let is_bool_type = col_type_hint.is_some_and(|t| {
        let upper = t.to_uppercase();
        upper.contains("BOOL")
    });
    if is_bool_type
        || trimmed.eq_ignore_ascii_case("true")
        || trimmed.eq_ignore_ascii_case("false")
    {
        let bool_color = if is_dark {
            egui::Color32::from_rgb(192, 132, 252) // Purple-400
        } else {
            egui::Color32::from_rgb(126, 34, 206) // Purple-700
        };
        return (bool_color, false);
    }

    // 3. Cek Integer
    let is_int_type = col_type_hint.is_some_and(|t| {
        let upper = t.to_uppercase();
        upper.contains("INT") || upper.contains("SERIAL")
    });
    let is_int_val = trimmed.parse::<i64>().is_ok();
    if (col_type_hint.is_none() || is_int_type) && is_int_val {
        let int_color = if is_dark {
            egui::Color32::from_rgb(103, 232, 249) // Cyan-300
        } else {
            egui::Color32::from_rgb(2, 132, 199) // Sky-600
        };
        return (int_color, false);
    }

    // 4. Cek Float / Decimal
    let is_float_type = col_type_hint.is_some_and(|t| {
        let upper = t.to_uppercase();
        upper.contains("FLOAT")
            || upper.contains("DOUBLE")
            || upper.contains("DECIMAL")
            || upper.contains("NUMERIC")
            || upper.contains("REAL")
    });
    let is_float_val = trimmed.parse::<f64>().is_ok()
        && (trimmed.contains('.') || trimmed.contains('e') || trimmed.contains('E'));
    if (is_float_type && (is_float_val || is_int_val))
        || (col_type_hint.is_none() && is_float_val)
    {
        let float_color = if is_dark {
            egui::Color32::from_rgb(56, 189, 248) // Sky-400
        } else {
            egui::Color32::from_rgb(3, 105, 161) // Sky-700
        };
        return (float_color, false);
    }

    // 5. Cek Date / DateTime / Timestamp
    let is_date_type = col_type_hint.is_some_and(|t| {
        let upper = t.to_uppercase();
        upper.contains("DATE") || upper.contains("TIME")
    });
    let is_iso_date = (trimmed.len() >= 10
        && trimmed.as_bytes().get(4) == Some(&b'-')
        && trimmed.as_bytes().get(7) == Some(&b'-'))
        || (trimmed.len() >= 8
            && trimmed.as_bytes().get(2) == Some(&b':')
            && trimmed.as_bytes().get(5) == Some(&b':'));
    if is_date_type || is_iso_date {
        let date_color = if is_dark {
            egui::Color32::from_rgb(251, 191, 36) // Amber-400
        } else {
            egui::Color32::from_rgb(180, 83, 9) // Amber-700
        };
        return (date_color, false);
    }

    // 6. Cek JSON / Array
    let is_json_type = col_type_hint.is_some_and(|t| t.to_uppercase().contains("JSON"));
    let is_json_val = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'));
    if is_json_type || is_json_val {
        let json_color = if is_dark {
            egui::Color32::from_rgb(244, 114, 182) // Pink-400
        } else {
            egui::Color32::from_rgb(190, 24, 93) // Pink-700
        };
        return (json_color, false);
    }

    // 7. Default: Text / String
    let text_color = if is_dark {
        egui::Color32::from_rgb(226, 232, 240) // Slate-200 (bersih dan kontras)
    } else {
        egui::Color32::from_rgb(30, 41, 59) // Slate-800
    };
    (text_color, false)
}

/// Mengembalikan (background_color, text_color) untuk sticky header tabel.
pub fn table_header_colors(is_dark: bool, is_pinned: bool) -> (egui::Color32, egui::Color32) {
    if is_dark {
        if is_pinned {
            (
                egui::Color32::from_rgb(37, 48, 74),
                egui::Color32::from_rgb(186, 230, 253),
            )
        } else {
            (
                egui::Color32::from_rgb(28, 32, 44),
                egui::Color32::from_rgb(147, 197, 253), // Soft Sky Blue
            )
        }
    } else if is_pinned {
        (
            egui::Color32::from_rgb(219, 234, 254),
            egui::Color32::from_rgb(30, 58, 138),
        )
    } else {
        (
            egui::Color32::from_rgb(237, 242, 247),
            egui::Color32::from_rgb(30, 64, 175), // Deep Royal Blue
        )
    }
}

/// Mengembalikan warna untuk SQL data type string (misal: "varchar(255)", "int", "datetime", dll.)
pub fn sql_type_color(data_type_str: &str, is_dark: bool) -> egui::Color32 {
    let lower = data_type_str.trim().to_ascii_lowercase();
    let base = lower.split('(').next().unwrap_or(&lower).trim();

    // 1. Integer / Serial / Identity -> Cyan
    if base.contains("int") || base.contains("serial") || base == "rowid" || base == "identity" {
        if is_dark {
            egui::Color32::from_rgb(103, 232, 249) // Cyan-300
        } else {
            egui::Color32::from_rgb(2, 132, 199) // Sky-600
        }
    }
    // 2. Float / Double / Decimal / Numeric / Real -> Sky Blue
    else if base.contains("float")
        || base.contains("double")
        || base.contains("decimal")
        || base.contains("numeric")
        || base.contains("real")
        || base.contains("money")
    {
        if is_dark {
            egui::Color32::from_rgb(56, 189, 248) // Sky-400
        } else {
            egui::Color32::from_rgb(3, 105, 161) // Sky-700
        }
    }
    // 3. String / Text / Char -> Emerald Green
    else if base.contains("char")
        || base.contains("text")
        || base.contains("string")
        || base.contains("clob")
    {
        if is_dark {
            egui::Color32::from_rgb(110, 231, 183) // Emerald-300
        } else {
            egui::Color32::from_rgb(5, 150, 105) // Emerald-600
        }
    }
    // 4. Date / Time / Timestamp / Year -> Amber
    else if base.contains("date")
        || base.contains("time")
        || base.contains("year")
    {
        if is_dark {
            egui::Color32::from_rgb(251, 191, 36) // Amber-400
        } else {
            egui::Color32::from_rgb(180, 83, 9) // Amber-700
        }
    }
    // 5. Boolean / Bit -> Purple
    else if base.contains("bool") || base == "bit" {
        if is_dark {
            egui::Color32::from_rgb(192, 132, 252) // Purple-400
        } else {
            egui::Color32::from_rgb(126, 34, 206) // Purple-700
        }
    }
    // 6. JSON / Binary / UUID / BLOB -> Pink / Fuchsia
    else if base.contains("json")
        || base.contains("blob")
        || base.contains("bytea")
        || base.contains("binary")
        || base.contains("uuid")
        || base.contains("guid")
    {
        if is_dark {
            egui::Color32::from_rgb(244, 114, 182) // Pink-400
        } else {
            egui::Color32::from_rgb(190, 24, 93) // Pink-700
        }
    }
    // 7. Enum / Set -> Indigo
    else if base.contains("enum") || base.contains("set") {
        if is_dark {
            egui::Color32::from_rgb(165, 180, 252) // Indigo-300
        } else {
            egui::Color32::from_rgb(79, 70, 229) // Indigo-600
        }
    }
    // 8. Default
    else if is_dark {
        egui::Color32::from_rgb(203, 213, 225) // Slate-300
    } else {
        egui::Color32::from_rgb(51, 65, 85) // Slate-700
    }
}

/// Mengembalikan warna untuk nama kolom di tampilan struktur tabel.
pub fn column_name_color(is_dark: bool, is_pk: bool) -> egui::Color32 {
    if is_pk {
        if is_dark {
            egui::Color32::from_rgb(252, 211, 77) // Gold-300
        } else {
            egui::Color32::from_rgb(180, 83, 9) // Amber-700
        }
    } else if is_dark {
        egui::Color32::from_rgb(45, 212, 191) // Teal-400
    } else {
        egui::Color32::from_rgb(15, 118, 110) // Teal-700
    }
}

/// Mengembalikan warna untuk nama index di tampilan indeks.
pub fn index_name_color(name: &str, is_unique: bool, is_dark: bool) -> egui::Color32 {
    if name.eq_ignore_ascii_case("PRIMARY") || (is_unique && name.to_ascii_lowercase().contains("primary")) {
        if is_dark {
            egui::Color32::from_rgb(252, 211, 77) // Gold-300
        } else {
            egui::Color32::from_rgb(180, 83, 9) // Amber-700
        }
    } else if is_unique {
        if is_dark {
            egui::Color32::from_rgb(52, 211, 153) // Emerald-400
        } else {
            egui::Color32::from_rgb(5, 150, 105) // Emerald-600
        }
    } else if is_dark {
        egui::Color32::from_rgb(251, 146, 60) // Orange-400
    } else {
        egui::Color32::from_rgb(234, 88, 12) // Orange-600
    }
}

/// Mengembalikan warna untuk metode/algoritma index (BTREE, HASH, dll.)
pub fn index_algorithm_color(is_dark: bool) -> egui::Color32 {
    if is_dark {
        egui::Color32::from_rgb(165, 180, 252) // Lavender-300
    } else {
        egui::Color32::from_rgb(79, 70, 229) // Indigo-600
    }
}

/// Mengembalikan warna untuk badge nullable ("YES", "NO", "?")
pub fn nullable_badge_color(nullable_str: &str, is_dark: bool) -> egui::Color32 {
    match nullable_str.trim() {
        "NO" => {
            if is_dark {
                egui::Color32::from_rgb(248, 113, 113) // Red-400 (NOT NULL penting terlihat)
            } else {
                egui::Color32::from_rgb(220, 38, 38) // Red-600
            }
        }
        "YES" => {
            if is_dark {
                egui::Color32::from_rgb(148, 163, 184) // Slate-400 (boleh NULL)
            } else {
                egui::Color32::from_rgb(100, 116, 139) // Slate-500
            }
        }
        _ => {
            if is_dark {
                egui::Color32::from_rgb(100, 116, 139) // Muted
            } else {
                egui::Color32::from_rgb(148, 163, 184)
            }
        }
    }
}

/// Mengembalikan warna untuk nomor baris (#) di tabel struktur & indeks.
pub fn table_row_number_color(is_dark: bool) -> egui::Color32 {
    if is_dark {
        egui::Color32::from_rgb(148, 163, 184) // Slate-400
    } else {
        egui::Color32::from_rgb(100, 116, 139) // Slate-500
    }
}

/// Mengembalikan warna untuk kolom "extra" (misal: auto_increment)
pub fn extra_info_color(extra: &str, is_dark: bool) -> egui::Color32 {
    let lower = extra.to_ascii_lowercase();
    if lower.contains("auto_increment") || lower.contains("identity") || lower.contains("generated") {
        if is_dark {
            egui::Color32::from_rgb(252, 211, 77) // Gold-300
        } else {
            egui::Color32::from_rgb(180, 83, 9) // Amber-700
        }
    } else if is_dark {
        egui::Color32::from_rgb(148, 163, 184)
    } else {
        egui::Color32::from_rgb(100, 116, 139)
    }
}


