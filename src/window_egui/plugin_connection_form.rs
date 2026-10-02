//! Field form koneksi untuk engine plugin (ADR 0002), dirender dari
//! `EngineDescriptor` milik driver. Dipanggil di dalam `egui::Grid` form
//! koneksi, jadi setiap field diakhiri `ui.end_row()`.

use crate::driver_api::{OptionKind, registry};
use crate::models::structs::ConnectionConfig;
use eframe::egui;

pub(crate) fn render_plugin_fields(
    ui: &mut egui::Ui,
    conn: &mut ConnectionConfig,
    engine_id: &str,
) {
    let Some(descriptor) = registry::descriptor(engine_id) else {
        ui.label("Driver:");
        ui.colored_label(
            egui::Color32::from_rgb(220, 120, 60),
            format!(
                "Driver '{engine_id}' is not installed. Install it from Plugins → Database Drivers."
            ),
        );
        ui.end_row();
        return;
    };
    let fields = &descriptor.standard_fields;

    if fields.host {
        ui.label("Host:");
        ui.text_edit_singleline(&mut conn.host);
        ui.end_row();
    }
    if fields.port {
        ui.label("Port:");
        let hint = descriptor
            .default_port
            .map(|p| p.to_string())
            .unwrap_or_default();
        ui.add(egui::TextEdit::singleline(&mut conn.port).hint_text(hint));
        ui.end_row();
    }
    if fields.username {
        ui.label("Username:");
        ui.text_edit_singleline(&mut conn.username);
        ui.end_row();
    }
    if fields.password {
        ui.label("Password:");
        ui.add(egui::TextEdit::singleline(&mut conn.password).password(true));
        ui.end_row();
    }
    if fields.database {
        ui.label("Database:");
        ui.text_edit_singleline(&mut conn.database);
        ui.end_row();
    }

    for field in &descriptor.options {
        let value = conn
            .plugin_options
            .entry(field.key.clone())
            .or_insert_with(|| field.default.clone().unwrap_or_default());
        let label = if field.required {
            format!("{}*:", field.label)
        } else {
            format!("{}:", field.label)
        };
        let label_resp = ui.label(label);
        if let Some(help) = &field.help {
            label_resp.on_hover_text(help);
        }
        match field.kind {
            OptionKind::Text | OptionKind::Number => {
                ui.text_edit_singleline(value);
            }
            OptionKind::Secret => {
                ui.add(egui::TextEdit::singleline(value).password(true));
            }
            OptionKind::Bool => {
                let mut checked = value == "true";
                if ui.checkbox(&mut checked, "").changed() {
                    *value = checked.to_string();
                }
            }
            OptionKind::Select => {
                egui::ComboBox::from_id_salt(("plugin_option", &field.key))
                    .selected_text(value.clone())
                    .show_ui(ui, |ui| {
                        for choice in &field.choices {
                            ui.selectable_value(value, choice.clone(), choice);
                        }
                    });
            }
        }
        ui.end_row();
    }
}

/// Engine plugin yang terpasang dan preset L2/L3, untuk combobox tipe
/// koneksi. Dipanggil di dalam `ComboBox::show_ui`.
pub(crate) fn render_engine_choices(ui: &mut egui::Ui, conn: &mut ConnectionConfig) {
    use crate::models::enums::DatabaseType;

    let engines = registry::descriptors();
    if !engines.is_empty() {
        ui.separator();
        ui.label(egui::RichText::new("Plugin drivers").small().weak());
        for d in engines {
            let ty = DatabaseType::Plugin(d.id.clone());
            let icon = d.icon.clone().unwrap_or_else(|| "🧩".to_string());
            if ui
                .selectable_label(conn.connection_type == ty, format!("{icon} {}", d.name))
                .clicked()
            {
                conn.connection_type = ty;
                if conn.port.is_empty()
                    && let Some(port) = d.default_port
                {
                    conn.port = port.to_string();
                }
            }
        }
    }

    ui.separator();
    ui.label(egui::RichText::new("Compatible engines").small().weak());
    for preset in crate::driver_api::presets::presets() {
        let resp = ui
            .selectable_label(
                false,
                format!("{} (via {})", preset.name, preset.based_on.badge_label()),
            )
            .on_hover_text(preset.note);
        if resp.clicked() {
            conn.connection_type = preset.based_on.clone();
            conn.port = preset.default_port.to_string();
            if conn.name.trim().is_empty() {
                conn.name = preset.name.to_string();
            }
        }
    }
}
