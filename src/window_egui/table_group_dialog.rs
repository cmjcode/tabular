//! Modal dialog untuk konfigurasi pola format komentar tabel (Group & Sub Group).

use crate::table_group::{self, TableGroupConfig};
use crate::window_egui::{Tabular, style};
use eframe::egui;

/// State modal dialog konfigurasi table grouping.
#[derive(Clone, Debug)]
pub struct TableGroupDialogState {
    pub show: bool,
    pub connection_id: Option<i64>,
    pub database_name: String,
    pub pattern: String,
    pub enabled: bool,
    pub sample_tables: Vec<(String, Option<String>)>,
}

impl Default for TableGroupDialogState {
    fn default() -> Self {
        Self {
            show: false,
            connection_id: None,
            database_name: String::new(),
            pattern: "[GROUP]-[SUB GROUP]-[Comment Table]".to_string(),
            enabled: true,
            sample_tables: Vec::new(),
        }
    }
}

/// Render modal dialog konfigurasi pola komentar tabel.
pub fn render_table_group_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.table_group_dialog.show {
        return;
    }

    style::render_modal_backdrop(
        ctx,
        "table_group_config_modal",
        tabular.table_group_dialog.show,
    );

    let mut close = false;
    let mut save_and_apply = false;

    egui::Window::new("table_group_config_window")
        .frame(style::modal_window_frame(ctx))
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .fixed_size(egui::vec2(660.0, 540.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            style::render_modal_header(ui, "🏷️ Table Grouping by Comment", &mut close);
            ui.add_space(8.0);

            let db_name = tabular.table_group_dialog.database_name.clone();
            let subtitle = if db_name.is_empty() {
                "Configure table grouping patterns for connections".to_string()
            } else {
                format!("Database: {}", db_name)
            };

            style::render_modal_card(
                ui,
                Some("Configuration"),
                Some(&subtitle),
                |ui| {
                    ui.checkbox(
                        &mut tabular.table_group_dialog.enabled,
                        "Enable grouping tables by comment",
                    );

                    if tabular.table_group_dialog.enabled {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Format Pattern:").strong().size(12.5));
                        ui.add_space(3.0);
                        ui.add(
                            egui::TextEdit::singleline(&mut tabular.table_group_dialog.pattern)
                                .desired_width(ui.available_width())
                                .hint_text("e.g. [GROUP]-[SUB GROUP]-[Comment Table]"),
                        );

                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Presets:").weak().small());
                            if ui.button("[GROUP]-[SUB GROUP]-[Comment Table]").clicked() {
                                tabular.table_group_dialog.pattern =
                                    "[GROUP]-[SUB GROUP]-[Comment Table]".to_string();
                            }
                            if ui.button("[GROUP]/[SUB GROUP]/[Comment Table]").clicked() {
                                tabular.table_group_dialog.pattern =
                                    "[GROUP]/[SUB GROUP]/[Comment Table]".to_string();
                            }
                            if ui.button("[GROUP] - [SUB GROUP]").clicked() {
                                tabular.table_group_dialog.pattern =
                                    "[GROUP] - [SUB GROUP]".to_string();
                            }
                        });

                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "Supported tokens: [GROUP], [SUB GROUP], [Comment Table]. Delimiters: -, /, _, :, |",
                            )
                            .weak()
                            .small(),
                        );
                    }
                },
            );

            ui.add_space(8.0);

            // Live Preview Card
            let pattern = tabular.table_group_dialog.pattern.clone();
            let is_enabled = tabular.table_group_dialog.enabled;

            style::render_modal_card(
                ui,
                Some("Live Preview"),
                Some("Real-time preview of parsed table comments with current pattern"),
                |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .show(ui, |ui| {
                            egui::Grid::new("table_group_preview_grid")
                                .striped(true)
                                .num_columns(5)
                                .spacing([12.0, 6.0])
                                .show(ui, |ui| {
                                    // Headers
                                    ui.label(egui::RichText::new("Table Name").strong().small());
                                    ui.label(egui::RichText::new("Raw Comment").strong().small());
                                    ui.label(egui::RichText::new("Group").strong().small());
                                    ui.label(egui::RichText::new("Sub Group").strong().small());
                                    ui.label(egui::RichText::new("Description").strong().small());
                                    ui.end_row();

                                    if !is_enabled {
                                        ui.label(egui::RichText::new("(Grouping disabled - tables will show in flat list)").weak().small());
                                        ui.label("");
                                        ui.label("-");
                                        ui.label("-");
                                        ui.label("-");
                                        ui.end_row();
                                        return;
                                    }

                                    // Baris sampel tabel
                                    let sample_tables = &tabular.table_group_dialog.sample_tables;
                                    if sample_tables.is_empty() {
                                        // Demo jika belum ada tabel ter-cache
                                        let demo = [
                                            ("m_users", Some("[AUTH]-[USER]-[User account records]")),
                                            ("m_roles", Some("[AUTH]-[ROLE]-[User roles & permissions]")),
                                            ("t_payroll", Some("[HR]-[PAYROLL]-[Monthly salary data]")),
                                            ("t_employees", Some("[HR] - Employee personal info")),
                                            ("sys_logs", Some("System audit log")),
                                        ];
                                        for (tbl, comment) in demo {
                                            let parsed = table_group::parse_table_comment(&pattern, comment);
                                            ui.label(egui::RichText::new(tbl).small());
                                            ui.label(egui::RichText::new(comment.unwrap_or("-")).weak().small());
                                            ui.label(egui::RichText::new(&parsed.group).color(egui::Color32::from_rgb(66, 133, 244)).small());
                                            ui.label(egui::RichText::new(parsed.sub_group.as_deref().unwrap_or("-")).color(egui::Color32::from_rgb(171, 71, 188)).small());
                                            ui.label(egui::RichText::new(parsed.description.as_deref().unwrap_or("-")).weak().small());
                                            ui.end_row();
                                        }
                                    } else {
                                        for (tbl, comment) in sample_tables {
                                            let parsed = table_group::parse_table_comment(&pattern, comment.as_deref());
                                            ui.label(egui::RichText::new(tbl).small());
                                            ui.label(egui::RichText::new(comment.as_deref().unwrap_or("-")).weak().small());
                                            ui.label(egui::RichText::new(&parsed.group).color(egui::Color32::from_rgb(66, 133, 244)).small());
                                            ui.label(egui::RichText::new(parsed.sub_group.as_deref().unwrap_or("-")).color(egui::Color32::from_rgb(171, 71, 188)).small());
                                            ui.label(egui::RichText::new(parsed.description.as_deref().unwrap_or("-")).weak().small());
                                            ui.end_row();
                                        }
                                    }
                                });
                        });
                },
            );

            ui.add_space(14.0);

            // Action Buttons
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(style::btn_primary_ctx(ctx, "Save & Apply")).clicked() {
                    save_and_apply = true;
                }
                ui.add_space(8.0);
                if ui.add(style::btn_secondary("Cancel")).clicked() {
                    close = true;
                }
            });
        });

    if save_and_apply {
        let conn_id = tabular.table_group_dialog.connection_id;
        match (conn_id, tabular.db_pool.clone(), tabular.runtime.clone()) {
            (Some(conn_id), Some(pool), Some(rt)) => {
                let db_name = tabular.table_group_dialog.database_name.clone();
                let cfg = TableGroupConfig {
                    pattern: tabular.table_group_dialog.pattern.clone(),
                    enabled: tabular.table_group_dialog.enabled,
                };

                // Simpan dulu sampai selesai: pengambilan skema diagram di bawah
                // membaca konfigurasi ini, jadi tidak boleh balapan dengan task
                // background.
                let saved = rt.block_on(crate::sidebar_database::save_table_group_config(
                    pool.as_ref(),
                    conn_id,
                    &db_name,
                    &cfg,
                ));
                match saved {
                    Ok(()) => {
                        // Force refresh tree node tables untuk database ini
                        tabular.reload_tables_tree_node(conn_id, &db_name);
                        // Sinkronkan ulang skema diagram database ini jika sedang terbuka
                        tabular.request_diagram_schema(conn_id, &db_name);
                        tabular.toasts.info("Table grouping configuration applied");
                    }
                    Err(e) => {
                        log::error!("[TABLE_GROUP] Failed to save config: {}", e);
                        tabular
                            .toasts
                            .error(format!("Failed to save table grouping: {e}"));
                    }
                }
            }
            _ => {
                log::warn!(
                    "[TABLE_GROUP] Save skipped: connection, cache pool or runtime unavailable"
                );
                tabular
                    .toasts
                    .error("Cannot save table grouping: no connection selected");
            }
        }
        tabular.table_group_dialog.show = false;
    } else if close {
        tabular.table_group_dialog.show = false;
    }
}
