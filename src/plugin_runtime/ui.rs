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
    let modal_width = (screen_rect.width() * 0.82).clamp(700.0, 1100.0);
    let modal_height = (screen_rect.height() * 0.82).clamp(520.0, 780.0);

    egui::Window::new("🧩 Plugin Extensibility & Advanced Automation")
        .open(&mut open)
        .resizable(true)
        .default_size([modal_width, modal_height])
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            // Top Tab Navigation Bar
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut state.active_tab,
                    PluginModalTab::PluginsCatalog,
                    "🔌 Plugins Catalog",
                );
                ui.selectable_value(
                    &mut state.active_tab,
                    PluginModalTab::ExecutionOutput,
                    "🚀 Output & Artifacts",
                );
                ui.selectable_value(
                    &mut state.active_tab,
                    PluginModalTab::CustomWasmRunner,
                    "🛠 Custom Wasm / WAT Runner",
                );
                ui.selectable_value(
                    &mut state.active_tab,
                    PluginModalTab::StarterTemplates,
                    "📋 Starter SDK Templates",
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("📂 Open Plugins Folder").on_hover_text("Open ~/.tabular/plugins folder").clicked() {
                        let path = crate::config::get_data_dir().join("plugins");
                        let _ = std::fs::create_dir_all(&path);
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(&path).spawn();
                        #[cfg(target_os = "linux")]
                        let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                        #[cfg(target_os = "windows")]
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                    }
                    if ui.button("🔄 Reload").on_hover_text("Reload plugins from disk").clicked() {
                        manager.load_plugins_from_disk();
                        state.status_message = Some("Plugins reloaded from disk".to_string());
                    }
                });
            });

            ui.separator();

            // Status or Error notifications
            let mut clear_error = false;
            if let Some(ref err) = state.error_message {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("❌ Error: {}", err)).color(egui::Color32::from_rgb(230, 80, 80)));
                    if ui.small_button("Dismiss").clicked() {
                        clear_error = true;
                    }
                });
                ui.separator();
            }
            if clear_error {
                state.error_message = None;
            }

            let mut clear_status = false;
            if let Some(ref stat) = state.status_message {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("✓ {}", stat)).color(egui::Color32::from_rgb(80, 200, 120)));
                    if ui.small_button("Dismiss").clicked() {
                        clear_status = true;
                    }
                });
                ui.separator();
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
    ui.horizontal(|ui| {
        ui.label("🔍 Search:");
        ui.text_edit_singleline(&mut state.search_query);

        ui.add_space(10.0);
        ui.label("Category:");
        let all_selected = state.filter_category.is_none();
        if ui.selectable_label(all_selected, "All").clicked() {
            state.filter_category = None;
        }
        if ui.selectable_label(state.filter_category == Some(PluginCategory::Export), "Export").clicked() {
            state.filter_category = Some(PluginCategory::Export);
        }
        if ui.selectable_label(state.filter_category == Some(PluginCategory::OrmCodeGen), "ORM & Models").clicked() {
            state.filter_category = Some(PluginCategory::OrmCodeGen);
        }
        if ui.selectable_label(state.filter_category == Some(PluginCategory::Custom), "Custom Wasm").clicked() {
            state.filter_category = Some(PluginCategory::Custom);
        }
    });

    ui.separator();

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

    // Split layout: Left = Plugin List, Right = Details & Run Panel
    ui.columns(2, |cols| {
        // Left Column: Plugins List
        cols[0].vertical(|ui| {
            ui.heading(egui::RichText::new("Installed & Builtin Plugins").size(14.0));
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .id_salt("plugins_list_scroll")
                .max_height(480.0)
                .show(ui, |ui| {
                    for p in filtered_plugins {
                        let is_selected = state.selected_plugin_id == p.id;
                        let item_frame = egui::Frame::group(ui.style())
                            .fill(if is_selected {
                                ui.visuals().selection.bg_fill.linear_multiply(0.2)
                            } else {
                                ui.visuals().faint_bg_color
                            })
                            .corner_radius(egui::CornerRadius::same(6u8))
                            .inner_margin(egui::Margin::symmetric(8, 6));

                        item_frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&p.icon).size(20.0));
                                ui.vertical(|ui| {
                                    if ui
                                        .selectable_label(
                                            is_selected,
                                            egui::RichText::new(&p.name).strong(),
                                        )
                                        .clicked()
                                    {
                                        state.selected_plugin_id = p.id.clone();
                                    }
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "v{} • {} • {}",
                                            p.version,
                                            p.category.display_name(),
                                            if p.is_builtin { "Built-in" } else { "Local File" }
                                        ))
                                        .small()
                                        .weak(),
                                    );
                                });
                            });
                        });
                        ui.add_space(4.0);
                    }
                });
        });

        // Right Column: Plugin Details & Execution Controls
        cols[1].vertical(|ui| {
            if let Some(selected_plugin) = manager.get_plugin(&state.selected_plugin_id).cloned() {
                ui.heading(egui::RichText::new(format!("{} {}", selected_plugin.icon, selected_plugin.name)).size(16.0));
                ui.label(egui::RichText::new(&selected_plugin.description).italics());
                ui.add_space(8.0);

                ui.group(|ui| {
                    ui.label(egui::RichText::new("Target Table & Data Context").strong());
                    ui.label(format!("• Active Table: {}", current_table_name));
                    ui.label(format!("• Total Columns: {}", current_headers.len()));
                    let data_rows_count = if !selected_rows.is_empty() {
                        format!("{} rows (Selection)", selected_rows.len())
                    } else {
                        format!("{} rows (Full Table)", all_rows.len())
                    };
                    ui.label(format!("• Data Scope: {}", data_rows_count));
                });

                ui.add_space(8.0);

                // Plugin-specific configuration controls
                if selected_plugin.id == "builtin_parquet_duckdb" {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new("Parquet & DuckDB Settings").strong());
                        ui.horizontal(|ui| {
                            ui.label("Output Parquet Filename:");
                            ui.text_edit_singleline(&mut state.parquet_output_path);
                        });
                    });
                }

                ui.add_space(12.0);

                // Run Plugin Button
                let run_btn = ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::Button::new(egui::RichText::new("▶ Execute Plugin").strong().size(14.0)),
                );

                if run_btn.clicked() {
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
                            state.status_message = Some(format!("Executed plugin '{}' successfully!", selected_plugin.name));
                            state.active_tab = PluginModalTab::ExecutionOutput;
                        }
                        Err(e) => {
                            state.error_message = Some(e);
                        }
                    }
                }
            } else {
                ui.label("Select a plugin from the list on the left.");
            }
        });
    });
}

/// Renders the execution output and artifact viewer
fn render_output_tab(ui: &mut egui::Ui, state: &mut PluginModalState) {
    ui.horizontal(|ui| {
        ui.heading("🚀 Execution Output & Generated Artifacts");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(ref text) = state.execution_output {
                if ui.button("📋 Copy to Clipboard").clicked() {
                    ui.ctx().copy_text(text.clone());
                    state.status_message = Some("Copied output to clipboard!".to_string());
                }

                if ui.button("💾 Save to File...").clicked() {
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

    ui.separator();

    if let Some(ref text) = state.execution_output {
        let mut display_text = text.clone();
        egui::ScrollArea::both()
            .id_salt("output_content_scroll")
            .max_height(420.0)
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
            ui.separator();
            ui.label(egui::RichText::new("Execution Logs:").strong());
            egui::ScrollArea::vertical()
                .id_salt("plugin_logs_scroll")
                .max_height(100.0)
                .show(ui, |ui| {
                    for log in &state.execution_logs {
                        ui.label(format!("[{:?}] {}", log.level, log.message));
                    }
                });
        }
    } else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(egui::RichText::new("No execution output yet.").size(15.0).weak());
            ui.label("Select a plugin from the Catalog or Custom Wasm runner and click 'Execute Plugin'.");
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
    ui.heading("🛠 Custom WebAssembly (WASM / WAT) Sandboxed Runner");
    ui.label("Write or paste WebAssembly Text Format (WAT) code or load a pre-compiled .wasm module to execute inside Tabular's sandboxed runtime.");
    ui.separator();

    ui.horizontal(|ui| {
        if ui.button("📂 Load .wasm / .wat File...").clicked() {
            let dialog = rfd::FileDialog::new()
                .add_filter("WebAssembly Files", &["wasm", "wat"]);
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

        if ui.button("🔄 Reset to Starter Parquet WAT").clicked() {
            state.custom_wat_code = WAT_PARQUET_STARTER.to_string();
            state.custom_wasm_file = None;
        }

        if ui.button("🔄 Reset to Starter ORM WAT").clicked() {
            state.custom_wat_code = WAT_ORM_STARTER.to_string();
            state.custom_wasm_file = None;
        }
    });

    ui.add_space(4.0);

    egui::ScrollArea::both()
        .id_salt("custom_wat_code_scroll")
        .max_height(340.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut state.custom_wat_code)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });

    ui.add_space(8.0);

    if ui.button(egui::RichText::new("▶ Execute Custom WebAssembly Code").strong().size(13.0)).clicked() {
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
                Ok(bytes) => manager.execute_raw(&bytes, "tabular_main", Some(&schema), selection_data.as_ref()),
                Err(e) => Err(format!("Failed to read WASM file: {}", e)),
            }
        } else {
            manager.execute_raw(state.custom_wat_code.as_bytes(), "tabular_main", Some(&schema), selection_data.as_ref())
        };

        match res {
            Ok(ctx_res) => {
                state.execution_output = ctx_res.result_output;
                state.execution_logs = ctx_res.captured_logs;
                state.execution_exports = ctx_res.captured_exports;
                state.error_message = None;
                state.status_message = Some("Custom WebAssembly module executed successfully!".to_string());
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
    ui.heading("📋 Starter SDK Templates & Host API Reference");
    ui.label("Use these templates to develop custom plugins for Tabular in Rust, WebAssembly, or TypeScript.");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Select Target ORM / Template:");
        egui::ComboBox::from_id_salt("template_orm_picker")
            .selected_text(state.selected_orm_target.display_name())
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.selected_orm_target, OrmTarget::RustDiesel, "Rust (Diesel)");
                ui.selectable_value(&mut state.selected_orm_target, OrmTarget::RustSeaOrm, "Rust (SeaORM)");
                ui.selectable_value(&mut state.selected_orm_target, OrmTarget::TypeScriptPrisma, "TypeScript (Prisma)");
                ui.selectable_value(&mut state.selected_orm_target, OrmTarget::TypeScriptTypeOrm, "TypeScript (TypeORM)");
                ui.selectable_value(&mut state.selected_orm_target, OrmTarget::PythonSqlAlchemy2, "Python (SQLAlchemy 2.0)");
                ui.selectable_value(&mut state.selected_orm_target, OrmTarget::PythonSqlAlchemy1, "Python (SQLAlchemy 1.4)");
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
            if ui.button("📋 Copy Code").clicked() {
                ui.ctx().copy_text(preview_code.clone());
                state.status_message = Some("Copied template to clipboard!".to_string());
            }
        });
    });

    egui::ScrollArea::both()
        .id_salt("template_preview_scroll")
        .max_height(350.0)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview_code)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
        });
}
