use eframe::egui;
use crate::models::enums::DatabaseType;
use crate::models::structs::{ColumnMetadata, ColumnStructInfo};
use crate::plugin_runtime::host_api::{PluginColumnSchema, PluginSelectionData, PluginTableSchema};
use crate::plugin_runtime::manager::{
    PluginCategory, PluginManifest, PluginManager, PluginModalState, PluginModalTab,
};
use crate::plugin_runtime::templates::{
    generate_orm_code, OrmTarget, WAT_ORM_STARTER, WAT_PARQUET_STARTER,
};
use crate::rfd;

/// Extract table schema from Tabular table state
pub fn extract_plugin_table_schema(
    table_name: &str,
    headers: &[String],
    struct_columns: Option<&[ColumnStructInfo]>,
    meta_columns: Option<&[ColumnMetadata]>,
    db_type: Option<&DatabaseType>,
    total_rows: usize,
) -> PluginTableSchema {
    let clean_name = table_name
        .trim()
        .strip_prefix("Table:")
        .map(str::trim)
        .unwrap_or(table_name.trim())
        .to_string();

    let mut columns = Vec::new();

    if let Some(struct_cols) = struct_columns.filter(|c| !c.is_empty()) {
        for col in struct_cols {
            let extra_lower = col.extra.as_deref().unwrap_or("").to_lowercase();
            let is_pk = extra_lower.contains("pri") || col.name.eq_ignore_ascii_case("id");
            let is_auto = extra_lower.contains("auto_increment") || extra_lower.contains("identity");

            columns.push(PluginColumnSchema {
                name: col.name.clone(),
                data_type: if col.data_type.is_empty() {
                    "VARCHAR".to_string()
                } else {
                    col.data_type.clone()
                },
                is_nullable: col.nullable.unwrap_or(!is_pk),
                is_primary_key: is_pk,
                is_auto_increment: is_auto,
                default_value: col.default_value.clone(),
                comment: None,
            });
        }
    } else if let Some(meta_cols) = meta_columns.filter(|c| !c.is_empty()) {
        for col in meta_cols {
            columns.push(PluginColumnSchema {
                name: col.name.clone(),
                data_type: if col.type_name.is_empty() {
                    "VARCHAR".to_string()
                } else {
                    col.type_name.clone()
                },
                is_nullable: !col.is_primary_key,
                is_primary_key: col.is_primary_key,
                is_auto_increment: col.is_primary_key,
                default_value: None,
                comment: None,
            });
        }
    } else {
        for header in headers {
            let is_id = header.eq_ignore_ascii_case("id");
            columns.push(PluginColumnSchema {
                name: header.clone(),
                data_type: if is_id { "BIGINT".to_string() } else { "VARCHAR(255)".to_string() },
                is_nullable: !is_id,
                is_primary_key: is_id,
                is_auto_increment: is_id,
                default_value: None,
                comment: None,
            });
        }
    }

    PluginTableSchema {
        table_name: if clean_name.is_empty() { "exported_table".to_string() } else { clean_name },
        schema_name: None,
        database_type: db_type
            .map(|d| format!("{:?}", d))
            .unwrap_or_else(|| "GenericSQL".to_string()),
        columns,
        total_rows,
    }
}

/// Modular panel renderer that can be embedded in Preferences or shown in a modal window
pub fn render_plugin_panel(
    ui: &mut egui::Ui,
    state: &mut PluginModalState,
    manager: &mut PluginManager,
    current_table_name: &str,
    current_headers: &[String],
    selected_rows: &[Vec<String>],
    all_rows: &[Vec<String>],
    struct_columns: Option<&[ColumnStructInfo]>,
    meta_columns: Option<&[ColumnMetadata]>,
    db_type: Option<&DatabaseType>,
) {
    let accent = crate::window_egui::style::theme_accent(ui.ctx());
    let dark = ui.visuals().dark_mode;

    // Top Segmented Pill Navigation Bar
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        let mut draw_subtab = |target: PluginModalTab, icon: &str, label: &str| {
            let is_selected = state.active_tab == target;
            let bg = if is_selected {
                accent
            } else if dark {
                egui::Color32::from_rgb(32, 35, 43)
            } else {
                egui::Color32::from_rgb(236, 240, 246)
            };
            let fg = if is_selected {
                egui::Color32::WHITE
            } else if dark {
                egui::Color32::from_rgb(175, 182, 195)
            } else {
                egui::Color32::from_rgb(55, 62, 75)
            };

            let btn = egui::Button::new(
                egui::RichText::new(format!("{} {}", icon, label))
                    .size(12.0)
                    .color(fg)
                    .strong(),
            )
            .fill(bg)
            .corner_radius(6.0)
            .min_size(egui::vec2(0.0, 28.0));

            if ui.add(btn).clicked() {
                state.active_tab = target;
            }
        };

        draw_subtab(
            PluginModalTab::PluginsCatalog,
            egui_icons::icons::MDI_PUZZLE.codepoint,
            "Plugins Catalog",
        );
        draw_subtab(
            PluginModalTab::ExecutionOutput,
            egui_icons::icons::ICON_TERMINAL.codepoint,
            "Output & Artifacts",
        );
        draw_subtab(
            PluginModalTab::CustomWasmRunner,
            egui_icons::icons::MDI_FILE_CODE.codepoint,
            "Custom Wasm Runner",
        );
        draw_subtab(
            PluginModalTab::StarterTemplates,
            egui_icons::icons::ICON_DESCRIPTION.codepoint,
            "SDK Templates",
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;

            let folder_btn = egui::Button::new(
                egui::RichText::new(format!(
                    "{} Plugins Folder",
                    egui_icons::icons::ICON_FOLDER_OPEN.codepoint
                ))
                .size(11.5),
            )
            .corner_radius(5.0);

            if ui
                .add(folder_btn)
                .on_hover_text("Open ~/.tabular/plugins folder on disk")
                .clicked()
            {
                let path = crate::config::get_data_dir().join("plugins");
                let _ = std::fs::create_dir_all(&path);
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(&path).spawn();
                #[cfg(target_os = "linux")]
                let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                #[cfg(target_os = "windows")]
                let _ = std::process::Command::new("explorer").arg(&path).spawn();
            }

            let reload_btn = egui::Button::new(
                egui::RichText::new(format!(
                    "{} Reload",
                    egui_icons::icons::ICON_REFRESH.codepoint
                ))
                .size(11.5),
            )
            .corner_radius(5.0);

            if ui
                .add(reload_btn)
                .on_hover_text("Reload installed plugins from disk")
                .clicked()
            {
                manager.load_plugins_from_disk();
                state.status_message = Some("Plugins reloaded from disk".to_string());
            }
        });
    });

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    // Status or Error notifications
    let mut clear_error = false;
    if let Some(ref err) = state.error_message {
        egui::Frame::new()
            .fill(if dark {
                egui::Color32::from_rgb(50, 20, 20)
            } else {
                egui::Color32::from_rgb(255, 235, 235)
            })
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 70, 70)))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("⚠ Error: {}", err))
                            .color(egui::Color32::from_rgb(230, 80, 80))
                            .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Dismiss").clicked() {
                            clear_error = true;
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }
    if clear_error {
        state.error_message = None;
    }

    let mut clear_status = false;
    if let Some(ref stat) = state.status_message {
        egui::Frame::new()
            .fill(if dark {
                egui::Color32::from_rgb(20, 45, 30)
            } else {
                egui::Color32::from_rgb(235, 255, 240)
            })
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 180, 100)))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("✓ {}", stat))
                            .color(egui::Color32::from_rgb(50, 180, 100))
                            .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Dismiss").clicked() {
                            clear_status = true;
                        }
                    });
                });
            });
        ui.add_space(4.0);
    }
    if clear_status {
        state.status_message = None;
    }

    // Tab Content
    match state.active_tab {
        PluginModalTab::PluginsCatalog => {
            render_catalog_tab(
                ui,
                state,
                manager,
                current_table_name,
                current_headers,
                selected_rows,
                all_rows,
                struct_columns,
                meta_columns,
                db_type,
            );
        }
        PluginModalTab::ExecutionOutput => {
            render_output_tab(ui, state);
        }
        PluginModalTab::CustomWasmRunner => {
            render_custom_wasm_tab(
                ui,
                state,
                manager,
                current_table_name,
                current_headers,
                selected_rows,
                all_rows,
                struct_columns,
                meta_columns,
                db_type,
            );
        }
        PluginModalTab::StarterTemplates => {
            render_starter_templates_tab(ui, state);
        }
    }
}

/// Render the Plugin Runtime & Extensibility Modal
pub fn render_plugin_modal(
    ctx: &egui::Context,
    state: &mut PluginModalState,
    manager: &mut PluginManager,
    current_table_name: &str,
    current_headers: &[String],
    selected_rows: &[Vec<String>],
    all_rows: &[Vec<String>],
    struct_columns: Option<&[ColumnStructInfo]>,
    meta_columns: Option<&[ColumnMetadata]>,
    db_type: Option<&DatabaseType>,
) {
    if !state.is_open {
        return;
    }

    let mut open = state.is_open;
    let screen_rect = ctx.content_rect();
    let modal_width = (screen_rect.width() * 0.85).clamp(720.0, 1100.0);
    let modal_height = (screen_rect.height() * 0.85).clamp(540.0, 800.0);

    let window_title = format!(
        "{}  Plugin Extensibility & Automation",
        egui_icons::icons::MDI_PUZZLE.codepoint
    );

    egui::Window::new(window_title)
        .open(&mut open)
        .resizable(true)
        .default_size([modal_width, modal_height])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            render_plugin_panel(
                ui,
                state,
                manager,
                current_table_name,
                current_headers,
                selected_rows,
                all_rows,
                struct_columns,
                meta_columns,
                db_type,
            );
        });

    state.is_open = open;
}

/// Renders the catalog tab listing available plugins
fn render_catalog_tab(
    ui: &mut egui::Ui,
    state: &mut PluginModalState,
    manager: &mut PluginManager,
    current_table_name: &str,
    current_headers: &[String],
    selected_rows: &[Vec<String>],
    all_rows: &[Vec<String>],
    struct_columns: Option<&[ColumnStructInfo]>,
    meta_columns: Option<&[ColumnMetadata]>,
    db_type: Option<&DatabaseType>,
) {
    let accent = crate::window_egui::style::theme_accent(ui.ctx());
    let dark = ui.visuals().dark_mode;

    // Search and Filter Header
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        // Search box with icon
        let search_frame = egui::Frame::new()
            .fill(if dark {
                egui::Color32::from_rgb(22, 24, 30)
            } else {
                egui::Color32::from_rgb(244, 247, 251)
            })
            .stroke(egui::Stroke::new(
                1.0,
                if dark {
                    egui::Color32::from_rgb(46, 50, 60)
                } else {
                    egui::Color32::from_rgb(215, 220, 230)
                },
            ))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(8, 4));

        search_frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(egui_icons::icons::ICON_SEARCH.codepoint)
                        .size(13.0)
                        .color(egui::Color32::GRAY),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut state.search_query)
                        .hint_text("Search plugins by name, tag, or description...")
                        .desired_width(260.0)
                        .frame(egui::Frame::NONE),
                );
            });
        });

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Category:").small().weak());

        let mut category_pill = |is_active: bool, label: &str| -> bool {
            let bg = if is_active {
                accent
            } else if dark {
                egui::Color32::from_rgb(32, 35, 43)
            } else {
                egui::Color32::from_rgb(236, 240, 246)
            };
            let fg = if is_active {
                egui::Color32::WHITE
            } else {
                ui.visuals().text_color()
            };
            ui.add(
                egui::Button::new(egui::RichText::new(label).size(11.0).color(fg))
                    .fill(bg)
                    .corner_radius(12.0)
                    .min_size(egui::vec2(0.0, 22.0)),
            )
            .clicked()
        };

        if category_pill(state.filter_category.is_none(), "All") {
            state.filter_category = None;
        }
        if category_pill(
            state.filter_category == Some(PluginCategory::Export),
            "Export & Storage",
        ) {
            state.filter_category = Some(PluginCategory::Export);
        }
        if category_pill(
            state.filter_category == Some(PluginCategory::OrmCodeGen),
            "ORM & Models",
        ) {
            state.filter_category = Some(PluginCategory::OrmCodeGen);
        }
        if category_pill(
            state.filter_category == Some(PluginCategory::Custom),
            "Custom Wasm",
        ) {
            state.filter_category = Some(PluginCategory::Custom);
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(4.0);

    let plugins = manager.get_plugins();
    let search_lower = state.search_query.to_lowercase();

    let filtered_plugins: Vec<&PluginManifest> = plugins
        .into_iter()
        .filter(|p| {
            if let Some(ref cat) = state.filter_category {
                if &p.category != cat {
                    return false;
                }
            }
            if !search_lower.is_empty() {
                return p.name.to_lowercase().contains(&search_lower)
                    || p.description.to_lowercase().contains(&search_lower);
            }
            true
        })
        .collect();

    // Auto-select first plugin if selection is empty or invalid
    if !filtered_plugins.is_empty()
        && (state.selected_plugin_id.is_empty()
            || !filtered_plugins.iter().any(|p| p.id == state.selected_plugin_id))
    {
        state.selected_plugin_id = filtered_plugins[0].id.clone();
    }

    // Split layout: Left = Plugin Cards, Right = Details & Run Panel
    ui.horizontal(|ui| {
        // Left Column: Plugins List (Fixed comfortable width: 295px)
        ui.vertical(|ui| {
            ui.set_width(295.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Installed & Builtin Plugins")
                        .size(12.5)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{} available", filtered_plugins.len()))
                            .size(11.0)
                            .weak(),
                    );
                });
            });
            ui.add_space(6.0);

            for p in &filtered_plugins {
                            let is_selected = state.selected_plugin_id == p.id;
                            let card_bg = if is_selected {
                                if dark {
                                    egui::Color32::from_rgba_unmultiplied(
                                        accent.r(),
                                        accent.g(),
                                        accent.b(),
                                        40,
                                    )
                                } else {
                                    egui::Color32::from_rgba_unmultiplied(
                                        accent.r(),
                                        accent.g(),
                                        accent.b(),
                                        25,
                                    )
                                }
                            } else if dark {
                                egui::Color32::from_rgb(26, 28, 35)
                            } else {
                                egui::Color32::from_rgb(246, 248, 252)
                            };

                            let stroke_color = if is_selected {
                                accent
                            } else if dark {
                                egui::Color32::from_rgb(44, 48, 58)
                            } else {
                                egui::Color32::from_rgb(222, 226, 235)
                            };

                            let card_frame = egui::Frame::new()
                                .fill(card_bg)
                                .stroke(egui::Stroke::new(
                                    if is_selected { 1.5 } else { 1.0 },
                                    stroke_color,
                                ))
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::symmetric(10, 8));

                            let card_resp = card_frame.show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    // Icon container badge (34x34)
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(34.0, 34.0),
                                        egui::Sense::hover(),
                                    );
                                    let icon_box_bg = if dark {
                                        egui::Color32::from_rgb(38, 42, 52)
                                    } else {
                                        egui::Color32::from_rgb(230, 235, 244)
                                    };
                                    ui.painter().rect_filled(rect, 6.0, icon_box_bg);
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        &p.icon,
                                        egui::FontId::proportional(18.0),
                                        accent,
                                    );

                                    ui.add_space(6.0);

                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(&p.name).size(12.5).strong(),
                                            );
                                        });

                                        ui.add_space(2.0);

                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("v{}", p.version))
                                                    .small()
                                                    .weak(),
                                            );
                                            ui.label(egui::RichText::new("•").small().weak());
                                            ui.label(
                                                egui::RichText::new(p.category.display_name())
                                                    .small()
                                                    .weak(),
                                            );
                                            ui.label(egui::RichText::new("•").small().weak());
                                            let tag_color = if p.is_builtin {
                                                if dark {
                                                    egui::Color32::from_rgb(100, 160, 230)
                                                } else {
                                                    egui::Color32::from_rgb(30, 90, 180)
                                                }
                                            } else {
                                                if dark {
                                                    egui::Color32::from_rgb(180, 130, 230)
                                                } else {
                                                    egui::Color32::from_rgb(120, 50, 180)
                                                }
                                            };
                                            ui.label(
                                                egui::RichText::new(if p.is_builtin {
                                                    "Built-in"
                                                } else {
                                                    "Local File"
                                                })
                                                .small()
                                                .color(tag_color),
                                            );
                                        });
                                    });
                                });
                            });

                    if card_resp.response.interact(egui::Sense::click()).clicked() {
                        state.selected_plugin_id = p.id.clone();
                    }
                    ui.add_space(3.0);
                }
        });

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        // Right Column: Plugin Details & Execution Controls (Fills remaining width)
        ui.vertical(|ui| {
            if let Some(selected_plugin) = manager.get_plugin(&state.selected_plugin_id).cloned() {
                // Plugin Header Banner Card
                egui::Frame::new()
                            .fill(if dark {
                                egui::Color32::from_rgb(24, 26, 33)
                            } else {
                                egui::Color32::from_rgb(248, 250, 254)
                            })
                            .stroke(egui::Stroke::new(
                                1.0,
                                if dark {
                                    egui::Color32::from_rgb(44, 48, 58)
                                } else {
                                    egui::Color32::from_rgb(225, 230, 240)
                                },
                            ))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let (icon_rect, _) = ui.allocate_exact_size(
                                        egui::vec2(40.0, 40.0),
                                        egui::Sense::hover(),
                                    );
                                    let icon_bg = if dark {
                                        egui::Color32::from_rgb(36, 40, 52)
                                    } else {
                                        egui::Color32::from_rgb(232, 236, 245)
                                    };
                                    ui.painter().rect_filled(icon_rect, 8.0, icon_bg);
                                    ui.painter().text(
                                        icon_rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        &selected_plugin.icon,
                                        egui::FontId::proportional(22.0),
                                        accent,
                                    );

                                    ui.add_space(8.0);
                                    ui.vertical(|ui| {
                                        ui.heading(
                                            egui::RichText::new(&selected_plugin.name)
                                                .size(15.0)
                                                .strong(),
                                        );
                                        ui.add_space(2.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Version {} • By {}",
                                                    selected_plugin.version, selected_plugin.author
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                            ui.label(egui::RichText::new("•").small().weak());
                                            ui.label(
                                                egui::RichText::new(
                                                    selected_plugin.category.display_name(),
                                                )
                                                .small()
                                                .strong()
                                                .color(accent),
                                            );
                                        });
                                    });
                                });

                                ui.add_space(8.0);
                                ui.label(
                                    egui::RichText::new(&selected_plugin.description).size(12.5),
                                );
                            });

                        ui.add_space(10.0);

                        // Target Table & Context Card
                        egui::Frame::new()
                            .fill(if dark {
                                egui::Color32::from_rgb(24, 26, 33)
                            } else {
                                egui::Color32::from_rgb(248, 250, 254)
                            })
                            .stroke(egui::Stroke::new(
                                1.0,
                                if dark {
                                    egui::Color32::from_rgb(44, 48, 58)
                                } else {
                                    egui::Color32::from_rgb(225, 230, 240)
                                },
                            ))
                            .corner_radius(8.0)
                            .inner_margin(egui::Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(egui_icons::icons::MDI_TABLE.codepoint)
                                            .size(15.0)
                                            .color(accent),
                                    );
                                    ui.label(
                                        egui::RichText::new("Target Table & Data Context")
                                            .size(13.0)
                                            .strong(),
                                    );
                                });

                                ui.add_space(6.0);

                                let has_table = !current_table_name.trim().is_empty();
                                if has_table {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Active Table:").size(12.0).weak(),
                                        );
                                        ui.label(
                                            egui::RichText::new(current_table_name)
                                                .size(12.0)
                                                .strong(),
                                        );
                                        if let Some(dt) = db_type {
                                            let chip_bg = if dark {
                                                egui::Color32::from_rgb(35, 40, 55)
                                            } else {
                                                egui::Color32::from_rgb(225, 235, 250)
                                            };
                                            egui::Frame::new()
                                                .fill(chip_bg)
                                                .corner_radius(4.0)
                                                .inner_margin(egui::Margin::symmetric(5, 1))
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        egui::RichText::new(format!("{:?}", dt))
                                                            .small()
                                                            .color(accent),
                                                    );
                                                });
                                        }
                                    });

                                    ui.add_space(2.0);

                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "Columns: {} detected",
                                                current_headers.len()
                                            ))
                                            .size(12.0)
                                            .weak(),
                                        );
                                        ui.label(egui::RichText::new("•").size(12.0).weak());
                                        if !selected_rows.is_empty() {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "✓ {} rows selected",
                                                    selected_rows.len()
                                                ))
                                                .size(12.0)
                                                .color(egui::Color32::from_rgb(40, 180, 90))
                                                .strong(),
                                            );
                                        } else if !all_rows.is_empty() {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Full table ({} rows)",
                                                    all_rows.len()
                                                ))
                                                .size(12.0)
                                                .weak(),
                                            );
                                        } else {
                                            ui.label(
                                                egui::RichText::new("0 rows loaded").size(12.0).weak(),
                                            );
                                        }
                                    });
                                } else {
                                    ui.label(
                                        egui::RichText::new(
                                            "ℹ No table currently selected in Tabular. The plugin will execute with a clean sample schema template.",
                                        )
                                        .small()
                                        .color(egui::Color32::from_gray(140)),
                                    );
                                }
                            });

                        ui.add_space(10.0);

                        // Plugin-specific configuration controls
                        if selected_plugin.id == "builtin_parquet_duckdb" {
                            egui::Frame::new()
                                .fill(if dark {
                                    egui::Color32::from_rgb(24, 26, 33)
                                } else {
                                    egui::Color32::from_rgb(248, 250, 254)
                                })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if dark {
                                        egui::Color32::from_rgb(44, 48, 58)
                                    } else {
                                        egui::Color32::from_rgb(225, 230, 240)
                                    },
                                ))
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::symmetric(14, 12))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("Parquet Export Configuration")
                                            .size(13.0)
                                            .strong(),
                                    );
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Output Filename:").size(12.0),
                                        );
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut state.parquet_output_path,
                                            )
                                            .hint_text("export.parquet")
                                            .desired_width(220.0),
                                        );
                                    });
                                });

                            ui.add_space(10.0);
                        }

                        // Prominent Primary Action Button: Execute Plugin
                        let play_icon = egui_icons::icons::ICON_PLAY_ARROW.codepoint;
                        let execute_btn = egui::Button::new(
                            egui::RichText::new(format!("{}  Execute Plugin", play_icon))
                                .size(13.5)
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(accent)
                        .corner_radius(6.0)
                        .min_size(egui::vec2(ui.available_width(), 36.0));

                        if ui.add(execute_btn).clicked() {
                            let schema = extract_plugin_table_schema(
                                current_table_name,
                                current_headers,
                                struct_columns,
                                meta_columns,
                                db_type,
                                all_rows.len(),
                            );

                            let selection_data = if !selected_rows.is_empty() {
                                Some(PluginSelectionData {
                                    table_name: schema.table_name.clone(),
                                    headers: current_headers.to_vec(),
                                    rows: selected_rows.to_vec(),
                                    total_selected: selected_rows.len(),
                                })
                            } else if !all_rows.is_empty() {
                                Some(PluginSelectionData {
                                    table_name: schema.table_name.clone(),
                                    headers: current_headers.to_vec(),
                                    rows: all_rows.to_vec(),
                                    total_selected: all_rows.len(),
                                })
                            } else {
                                None
                            };

                            match manager.execute_plugin(
                                &selected_plugin.id,
                                &schema,
                                selection_data.as_ref(),
                                None,
                                Some(&state.parquet_output_path),
                            ) {
                                Ok(ctx_res) => {
                                    state.execution_output = ctx_res.result_output;
                                    state.execution_logs = ctx_res.captured_logs;
                                    state.execution_exports = ctx_res.captured_exports;
                                    state.error_message = None;
                                    state.status_message = Some(format!(
                                        "Executed plugin '{}' successfully!",
                                        selected_plugin.name
                                    ));
                                    state.active_tab = PluginModalTab::ExecutionOutput;
                                }
                                Err(e) => {
                                    state.error_message = Some(e);
                                }
                            }
                        }
            } else {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label(
                        egui::RichText::new("Select a plugin from the list on the left.")
                            .size(14.0)
                            .weak(),
                    );
                });
            }
        });
    });
}

/// Renders the execution output and artifact viewer
fn render_output_tab(ui: &mut egui::Ui, state: &mut PluginModalState) {
    let dark = ui.visuals().dark_mode;

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{}  Execution Output & Artifacts",
                egui_icons::icons::ICON_TERMINAL.codepoint
            ))
            .size(14.0)
            .strong(),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(ref text) = state.execution_output {
                if ui
                    .button(format!(
                        "{} Copy to Clipboard",
                        egui_icons::icons::ICON_CONTENT_COPY.codepoint
                    ))
                    .clicked()
                {
                    ui.ctx().copy_text(text.clone());
                    state.status_message = Some("Copied output to clipboard!".to_string());
                }

                if ui
                    .button(format!(
                        "{} Save to File...",
                        egui_icons::icons::ICON_SAVE.codepoint
                    ))
                    .clicked()
                {
                    let default_name = state
                        .execution_exports
                        .first()
                        .map(|e| e.filename_suggestion.as_str())
                        .unwrap_or("plugin_export.txt");

                    let dialog = rfd::FileDialog::new().set_file_name(default_name);
                    if let Some(path) = dialog.save_file() {
                        if let Err(e) = std::fs::write(&path, text) {
                            state.error_message = Some(format!("Failed to write file: {}", e));
                        } else {
                            state.status_message = Some(format!("Saved artifact to {:?}", path));
                        }
                    }
                }
            }
        });
    });

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    if let Some(ref text) = state.execution_output {
        let mut display_text = text.clone();
        let out_scroll_h = (ui.available_height() - if state.execution_logs.is_empty() { 20.0 } else { 130.0 }).max(360.0);
        egui::ScrollArea::both()
            .id_salt("output_content_scroll")
            .max_height(out_scroll_h)
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut display_text)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace),
                );
            });

        // Show execution logs if any
        if !state.execution_logs.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Execution Logs:").strong().size(12.0));
            egui::ScrollArea::vertical()
                .id_salt("plugin_logs_scroll")
                .max_height(100.0)
                .show(ui, |ui| {
                    for log in &state.execution_logs {
                        ui.label(
                            egui::RichText::new(format!("[{:?}] {}", log.level, log.message))
                                .small()
                                .color(if dark {
                                    egui::Color32::from_gray(180)
                                } else {
                                    egui::Color32::from_gray(70)
                                }),
                        );
                    }
                });
        }
    } else {
        ui.vertical_centered(|ui| {
            ui.add_space(60.0);
            ui.label(
                egui::RichText::new(egui_icons::icons::MDI_FILE_CODE.codepoint)
                    .size(36.0)
                    .color(egui::Color32::GRAY),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("No execution output yet.")
                    .size(14.0)
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Select a plugin from the Catalog or Custom Wasm runner and click 'Execute Plugin'.",
                )
                .small()
                .weak(),
            );
        });
    }
}

/// Renders the custom WAT/Wasm bytecode runner tab
fn render_custom_wasm_tab(
    ui: &mut egui::Ui,
    state: &mut PluginModalState,
    manager: &mut PluginManager,
    current_table_name: &str,
    current_headers: &[String],
    selected_rows: &[Vec<String>],
    all_rows: &[Vec<String>],
    struct_columns: Option<&[ColumnStructInfo]>,
    meta_columns: Option<&[ColumnMetadata]>,
    db_type: Option<&DatabaseType>,
) {
    let accent = crate::window_egui::style::theme_accent(ui.ctx());

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{}  Custom WebAssembly (WASM / WAT) Sandboxed Runner",
                egui_icons::icons::MDI_FILE_CODE.codepoint
            ))
            .size(14.0)
            .strong(),
        );
    });
    ui.label(
        egui::RichText::new(
            "Write or paste WebAssembly Text Format (WAT) code or load a pre-compiled .wasm module to execute inside Tabular's sandboxed runtime.",
        )
        .small()
        .weak(),
    );

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        if ui
            .button(format!(
                "{} Load .wasm / .wat File...",
                egui_icons::icons::ICON_FOLDER_OPEN.codepoint
            ))
            .clicked()
        {
            let dialog =
                rfd::FileDialog::new().add_filter("WebAssembly Files", &["wasm", "wat"]);
            if let Some(path) = dialog.pick_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if ext.eq_ignore_ascii_case("wat") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            state.custom_wat_code = content;
                            state.status_message = Some(format!("Loaded WAT file: {:?}", path));
                        }
                    } else if ext.eq_ignore_ascii_case("wasm") {
                        state.custom_wasm_file = Some(path.clone());
                        state.status_message = Some(format!("Selected WASM file: {:?}", path));
                    }
                }
            }
        }

        if ui
            .button(format!(
                "{} Preset: Parquet WAT",
                egui_icons::icons::ICON_REFRESH.codepoint
            ))
            .clicked()
        {
            state.custom_wat_code = WAT_PARQUET_STARTER.to_string();
            state.custom_wasm_file = None;
        }

        if ui
            .button(format!(
                "{} Preset: ORM WAT",
                egui_icons::icons::ICON_REFRESH.codepoint
            ))
            .clicked()
        {
            state.custom_wat_code = WAT_ORM_STARTER.to_string();
            state.custom_wasm_file = None;
        }
    });

    ui.add_space(6.0);

    let wat_h = (ui.available_height() - 60.0).max(360.0);
    egui::ScrollArea::both()
        .id_salt("custom_wat_code_scroll")
        .max_height(wat_h)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut state.custom_wat_code)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });

    ui.add_space(8.0);

    let run_btn = egui::Button::new(
        egui::RichText::new(format!(
            "{}  Execute Custom WebAssembly Code",
            egui_icons::icons::ICON_PLAY_ARROW.codepoint
        ))
        .size(13.0)
        .color(egui::Color32::WHITE)
        .strong(),
    )
    .fill(accent)
    .corner_radius(6.0)
    .min_size(egui::vec2(ui.available_width(), 34.0));

    if ui.add(run_btn).clicked() {
        let schema = extract_plugin_table_schema(
            current_table_name,
            current_headers,
            struct_columns,
            meta_columns,
            db_type,
            all_rows.len(),
        );

        let selection_data = if !selected_rows.is_empty() {
            Some(PluginSelectionData {
                table_name: schema.table_name.clone(),
                headers: current_headers.to_vec(),
                rows: selected_rows.to_vec(),
                total_selected: selected_rows.len(),
            })
        } else if !all_rows.is_empty() {
            Some(PluginSelectionData {
                table_name: schema.table_name.clone(),
                headers: current_headers.to_vec(),
                rows: all_rows.to_vec(),
                total_selected: all_rows.len(),
            })
        } else {
            None
        };

        let res = if let Some(ref wasm_path) = state.custom_wasm_file {
            match std::fs::read(wasm_path) {
                Ok(bytes) => manager.execute_raw(
                    &bytes,
                    "tabular_main",
                    Some(&schema),
                    selection_data.as_ref(),
                ),
                Err(e) => Err(format!("Failed to read WASM file: {}", e)),
            }
        } else {
            manager.execute_raw(
                state.custom_wat_code.as_bytes(),
                "tabular_main",
                Some(&schema),
                selection_data.as_ref(),
            )
        };

        match res {
            Ok(ctx_res) => {
                state.execution_output = ctx_res.result_output;
                state.execution_logs = ctx_res.captured_logs;
                state.execution_exports = ctx_res.captured_exports;
                state.error_message = None;
                state.status_message =
                    Some("Custom WebAssembly module executed successfully!".to_string());
                state.active_tab = PluginModalTab::ExecutionOutput;
            }
            Err(e) => {
                state.error_message = Some(e);
            }
        }
    }
}

/// Renders starter SDK templates and references
fn render_starter_templates_tab(ui: &mut egui::Ui, state: &mut PluginModalState) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{}  Starter SDK Templates & Host API Reference",
                egui_icons::icons::ICON_DESCRIPTION.codepoint
            ))
            .size(14.0)
            .strong(),
        );
    });
    ui.label(
        egui::RichText::new(
            "Use these templates to develop custom plugins for Tabular in Rust, WebAssembly, or TypeScript.",
        )
        .small()
        .weak(),
    );

    ui.add_space(4.0);
    ui.separator();
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("Select Target ORM / Template:");
        egui::ComboBox::from_id_salt("template_orm_picker")
            .selected_text(state.selected_orm_target.display_name())
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut state.selected_orm_target,
                    OrmTarget::RustDiesel,
                    "Rust (Diesel)",
                );
                ui.selectable_value(
                    &mut state.selected_orm_target,
                    OrmTarget::RustSeaOrm,
                    "Rust (SeaORM)",
                );
                ui.selectable_value(
                    &mut state.selected_orm_target,
                    OrmTarget::TypeScriptPrisma,
                    "TypeScript (Prisma)",
                );
                ui.selectable_value(
                    &mut state.selected_orm_target,
                    OrmTarget::TypeScriptTypeOrm,
                    "TypeScript (TypeORM)",
                );
                ui.selectable_value(
                    &mut state.selected_orm_target,
                    OrmTarget::PythonSqlAlchemy2,
                    "Python (SQLAlchemy 2.0)",
                );
                ui.selectable_value(
                    &mut state.selected_orm_target,
                    OrmTarget::PythonSqlAlchemy1,
                    "Python (SQLAlchemy 1.4)",
                );
            });
    });

    ui.add_space(8.0);

    let sample_schema = PluginTableSchema {
        table_name: "users".to_string(),
        schema_name: Some("public".to_string()),
        database_type: "PostgreSQL".to_string(),
        columns: vec![
            PluginColumnSchema {
                name: "id".to_string(),
                data_type: "BIGINT".to_string(),
                is_nullable: false,
                is_primary_key: true,
                is_auto_increment: true,
                default_value: None,
                comment: None,
            },
            PluginColumnSchema {
                name: "email".to_string(),
                data_type: "VARCHAR(255)".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_auto_increment: false,
                default_value: None,
                comment: None,
            },
            PluginColumnSchema {
                name: "is_active".to_string(),
                data_type: "BOOLEAN".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_auto_increment: false,
                default_value: Some("true".to_string()),
                comment: None,
            },
            PluginColumnSchema {
                name: "created_at".to_string(),
                data_type: "TIMESTAMP".to_string(),
                is_nullable: false,
                is_primary_key: false,
                is_auto_increment: false,
                default_value: Some("NOW()".to_string()),
                comment: None,
            },
        ],
        total_rows: 1000,
    };

    let mut preview_code = generate_orm_code(&sample_schema, state.selected_orm_target);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Generated Template Code:").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button(format!(
                    "{} Copy Code",
                    egui_icons::icons::ICON_CONTENT_COPY.codepoint
                ))
                .clicked()
            {
                ui.ctx().copy_text(preview_code.clone());
                state.status_message = Some("Copied template to clipboard!".to_string());
            }
        });
    });

    ui.add_space(4.0);

    let tpl_h = (ui.available_height() - 20.0).max(360.0);
    egui::ScrollArea::both()
        .id_salt("template_preview_scroll")
        .max_height(tpl_h)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview_code)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
}


