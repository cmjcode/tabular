//! Kartu melayang "konteks database" di pojok kanan atas area editor.
//!
//! Berisi badge environment, picker koneksi, database, dan schema untuk tab
//! aktif. Picker ini hanya relevan untuk Database Client, jadi tidak lagi
//! menempati header kanan (header kanan kini dipakai switcher project) dan
//! disembunyikan pada tab HTTP.

use eframe::egui;

use super::device_profile::DeviceUiMetrics;
use super::searchable_picker::{PickerConfig, searchable_picker};
use crate::window_egui::Tabular;

/// Jarak kartu dari tepi kanan dan atas area konten.
const CARD_OFFSET: egui::Vec2 = egui::vec2(-12.0, 8.0);
/// Opacity latar kartu, sama dengan floating toolbar kanvas lain.
const CARD_OPACITY: f32 = 0.92;

/// Apakah tab aktif memakai konteks database (bukan tab HTTP).
fn tab_uses_db_context(t: &Tabular) -> bool {
    t.query_tabs
        .get(t.active_tab_index)
        .is_some_and(|tab| tab.http_client_state.is_none() && tab.git_state.is_none())
}

/// Render kartu melayang di pojok kanan atas `content_rect`.
pub fn render(
    t: &mut Tabular,
    ui: &mut egui::Ui,
    content_rect: egui::Rect,
    metrics: &DeviceUiMetrics,
) {
    if !tab_uses_db_context(t) || content_rect.width() < 120.0 {
        return;
    }

    egui::Area::new(egui::Id::new("db_context_bar"))
        .order(egui::Order::Middle)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(content_rect.right_top() + CARD_OFFSET)
        .constrain_to(content_rect)
        .show(ui.ctx(), |ui| {
            let visuals = ui.visuals().clone();
            egui::Frame::NONE
                .fill(visuals.window_fill.gamma_multiply(CARD_OPACITY))
                .stroke(visuals.widgets.noninteractive.bg_stroke)
                .corner_radius(egui::CornerRadius::same(6))
                .shadow(visuals.popup_shadow)
                .inner_margin(egui::Margin::symmetric(4, 3))
                .show(ui, |ui| {
                    ui.set_max_width((content_rect.width() - 24.0).max(100.0));
                    ui.horizontal(|ui| render_pickers(t, ui, metrics));
                });
        });
}

/// Isi kartu: badge environment, koneksi, database, lalu schema (kiri ke kanan).
fn render_pickers(t: &mut Tabular, ui: &mut egui::Ui, metrics: &DeviceUiMetrics) {
    ui.spacing_mut().item_spacing.x = if metrics.is_touch { 4.0 } else { 2.0 };
    ui.spacing_mut().button_padding = if metrics.is_touch {
        egui::vec2(10.0, 6.0)
    } else {
        egui::vec2(8.0, 4.0)
    };
    ui.spacing_mut().interact_size.y = if metrics.is_touch { 34.0 } else { 28.0 };

    // Tampilan flat seperti sebelumnya di header: tanpa latar dan garis tepi.
    let widgets = &mut ui.style_mut().visuals.widgets;
    widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
    widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    widgets.inactive.bg_stroke = egui::Stroke::NONE;
    for w in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        w.corner_radius = egui::CornerRadius::same(5);
    }

    let sep_color = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(55, 55, 60)
    } else {
        egui::Color32::from_rgb(210, 210, 215)
    };
    let add_divider = |ui: &mut egui::Ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, 20.0), egui::Sense::hover());
        ui.painter().vline(
            rect.center().x,
            rect.y_range(),
            egui::Stroke::new(1.0, sep_color),
        );
    };

    let mut conn_list: Vec<(i64, String)> = t
        .connections
        .iter()
        .filter_map(|c| c.id.map(|id| (id, c.display_name())))
        .collect();
    conn_list.sort_by_key(|a| a.1.to_lowercase());
    let (tab_conn_id, tab_db_name) = t
        .query_tabs
        .get(t.active_tab_index)
        .map(|tab| (tab.connection_id, tab.database_name.clone()))
        .unwrap_or((None, None));
    let current_conn_name = match tab_conn_id {
        Some(cid) => t
            .get_connection_name(cid)
            .unwrap_or_else(|| "(conn)".to_string()),
        None => "Select Connection".to_string(),
    };

    // Badge environment di kiri picker koneksi.
    if let Some(env) = tab_conn_id.and_then(|cid| t.connection_environment_by_id(cid)) {
        super::platform_ui::environment_badge(ui, env);
    }

    // 1. Picker koneksi
    let conn_picker = PickerConfig {
        id_salt: "query_conn_select",
        icon: egui_icons::icons::ICON_DNS.codepoint,
        title: "Connections",
        search_hint: "Search connections…",
        tooltip: "Active connection",
        min_width: if metrics.is_touch { 170.0 } else { 150.0 },
        max_width: if metrics.is_touch { 280.0 } else { 240.0 },
        is_touch: metrics.is_touch,
    };
    let conn_names: Vec<String> = conn_list.iter().map(|(_, n)| n.clone()).collect();
    let conn_selected = conn_list
        .iter()
        .position(|(cid, _)| tab_conn_id == Some(*cid));
    if let Some(i) = searchable_picker(
        ui,
        &conn_picker,
        &current_conn_name,
        &conn_names,
        conn_selected,
    ) {
        let cid = conn_list[i].0;
        if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
            tab.connection_id = Some(cid);
            tab.database_name = None; // reset db untuk koneksi baru
        }
        t.current_table_headers.clear();
        t.current_table_data.clear();
    }

    let Some(cid) = tab_conn_id else {
        return;
    };

    // 2. Picker database
    add_divider(ui);
    let mut dbs = t.get_databases_cached(cid);
    if dbs.is_empty() {
        dbs.push("(default)".to_string());
    }
    let active_db = tab_db_name
        .clone()
        .unwrap_or_else(|| "(default)".to_string());
    let db_picker = PickerConfig {
        id_salt: "query_db_select",
        icon: egui_icons::icons::ICON_STORAGE.codepoint,
        title: "Databases",
        search_hint: "Search databases…",
        tooltip: "Active database",
        min_width: if metrics.is_touch { 150.0 } else { 130.0 },
        max_width: if metrics.is_touch { 260.0 } else { 220.0 },
        is_touch: metrics.is_touch,
    };
    let db_selected = dbs.iter().position(|d| *d == active_db);
    if let Some(i) = searchable_picker(ui, &db_picker, &active_db, &dbs, db_selected) {
        let db = &dbs[i];
        if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
            tab.database_name = if db == "(default)" {
                None
            } else {
                Some(db.clone())
            };
        }
        t.current_table_headers.clear();
        t.current_table_data.clear();
    }

    // 3. Picker schema / search path (hanya engine yang mendukung schema, mis. PostgreSQL & MsSQL)
    let (tab_schema, active_conn_type) = t
        .query_tabs
        .get(t.active_tab_index)
        .map(|tab| {
            let conn_type = t
                .connections
                .iter()
                .find(|c| c.id == tab.connection_id)
                .map(|c| c.connection_type.clone());
            (tab.schema_name.clone(), conn_type)
        })
        .unwrap_or((None, None));
    if !active_conn_type
        .as_ref()
        .is_some_and(|ct| ct.supports_schemas())
    {
        return;
    }
    add_divider(ui);
    let mut schemas = t.get_schemas_cached(cid, tab_db_name.as_deref());
    if schemas.is_empty() {
        schemas.push("public".to_string());
    }
    let active_schema = tab_schema.unwrap_or_else(|| "public".to_string());
    let schema_picker = PickerConfig {
        id_salt: "query_schema_select",
        icon: egui_icons::icons::ICON_SCHEMA.codepoint,
        title: "Schemas",
        search_hint: "Search schemas…",
        tooltip: "Active schema",
        min_width: if metrics.is_touch { 120.0 } else { 100.0 },
        max_width: if metrics.is_touch { 200.0 } else { 170.0 },
        is_touch: metrics.is_touch,
    };
    let schema_selected = schemas.iter().position(|s| *s == active_schema);
    if let Some(i) = searchable_picker(
        ui,
        &schema_picker,
        &active_schema,
        &schemas,
        schema_selected,
    ) {
        let s = &schemas[i];
        if let Some(tab) = t.query_tabs.get_mut(t.active_tab_index) {
            tab.schema_name = Some(s.clone());
        }
        // search_path diterapkan executor pada koneksi yang menjalankan query
        // (lihat QueryExecutionOptions::schema_name).
        t.toasts.info(format!("Switched active schema to '{}'", s));
    }
}
