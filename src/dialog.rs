use eframe::egui;
use log::error;

use crate::{editor, models, window_egui};

fn load_logo_texture(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.logo_texture.is_some() {
        return;
    }

    // Try filesystem asset first (useful during dev runs)
    let bytes_from_fs = std::fs::read("assets/logo.png").ok();

    // Fallback to embedded bytes so packaged apps always show the logo
    // SAFETY: the file path is compile-time checked
    let embedded_bytes: &[u8] = include_bytes!("../assets/logo.png");

    let image_bytes: Vec<u8> = bytes_from_fs.unwrap_or_else(|| embedded_bytes.to_vec());

    if let Ok(image) = image::load_from_memory(&image_bytes) {
        let rgba_image = image.to_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let pixels = rgba_image.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
        tabular.logo_texture = Some(ctx.load_texture("logo", color_image, Default::default()));
    }
}
pub(crate) fn render_about_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.show_about_dialog {
        // Load logo texture if not already loaded
        load_logo_texture(tabular, ctx);

        let mut should_check_updates = false;

        egui::Window::new("About Tabular")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(400.0)
            .open(&mut tabular.show_about_dialog)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);

                    // App icon/logo - use actual logo if loaded, fallback to emoji
                    if let Some(logo_texture) = &tabular.logo_texture {
                        ui.add(
                            egui::Image::from_texture(logo_texture)
                                .max_size(egui::vec2(180.0, 180.0)),
                        );
                    } else {
                        ui.label(egui::RichText::new("📊").size(48.0));
                    }
                    ui.add_space(10.0);

                    // App name and version
                    ui.label(egui::RichText::new("Tabular").size(26.0).strong());
                    ui.label(
                        egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .size(18.0)
                            .color(egui::Color32::GRAY),
                    );
                    ui.label(
                        egui::RichText::new("Built with ❤️ using Rust")
                            .size(14.0)
                            .color(egui::Color32::GRAY),
                    );
                    ui.add_space(15.0);

                    // Description
                    ui.label(
                        egui::RichText::new(
                            "Your SQL Editor, Forged with Rust: Fast, Safe, Efficient.",
                        )
                        .size(14.0),
                    );
                    ui.label("Credit : Pamungkas Jayuda (https://github.com/Jayuda), Mualip Suhal (https://github.com/msuhal),  Davin Adesta Putra (https://github.com/Davin-adesta), Mohamad Ardiansah Pratama (https://github.com/ardiansyah20007) ");
                    ui.add_space(10.0);

                    // Update check button
                    if ui.button("🔄 Check for Updates").clicked() {
                        should_check_updates = true;
                    }
                    ui.add_space(10.0);

                    ui.hyperlink_to(
                        "https://github.com/tabular-id/tabular",
                        "https://github.com/tabular-id/tabular",
                    );
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("© 2025 PT. Vneu Teknologi Indonesia ")
                            .size(10.0)
                            .color(egui::Color32::GRAY),
                    );
                    ui.add_space(15.0);
                });
            });

        if should_check_updates {
            tabular.check_for_updates(true); // Manual check from About dialog
        }
    }
}

pub(crate) fn render_error_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.show_error_message {
        egui::Window::new("Error")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(&tabular.error_message);
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        tabular.show_error_message = false;
                        tabular.error_message.clear();
                    }
                });
            });
    }
}

pub(crate) fn render_save_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.show_save_dialog {
        egui::Window::new("Save Query")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(500.0)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.add_space(5.0);

                    // Current save directory display
                    ui.label("Save location:");
                    ui.horizontal(|ui| {
                        let display_path = if !tabular.save_directory.is_empty() {
                            &tabular.save_directory
                        } else {
                            "Using default query directory"
                        };
                        ui.label(egui::RichText::new(display_path).weak().monospace());

                        if ui.button("📁 Browse").clicked() {
                            tabular.handle_save_directory_picker();
                        }
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(5.0);

                    // Filename input
                    ui.label("Enter filename:");
                    ui.text_edit_singleline(&mut tabular.save_filename);

                    ui.add_space(10.0);

                    // Action buttons
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() && !tabular.save_filename.is_empty() {
                            if let Err(err) = editor::save_current_tab_with_name(
                                tabular,
                                tabular.save_filename.clone(),
                            ) {
                                error!("Failed to save: {}", err);
                            }
                            tabular.show_save_dialog = false;
                            tabular.save_filename.clear();
                            // Reset save directory for next save
                            tabular.save_directory.clear();
                        }

                        if ui.button("Cancel").clicked() {
                            tabular.show_save_dialog = false;
                            tabular.save_filename.clear();
                            // Reset save directory for next save
                            tabular.save_directory.clear();
                        }
                    });
                });
            });
    }
}

pub(crate) fn render_index_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if !tabular.show_index_dialog {
        return;
    }
    let mut open_flag = tabular.show_index_dialog;
    // Work on a local copy and write back after UI, so typing/checkbox persist across frames
    let Some(initial_state) = tabular.index_dialog.clone() else {
        return;
    };
    let mut working = initial_state;
    // Defer opening tab until after closure to avoid borrow conflicts
    let mut open_tab_request: Option<(String /*title*/, String /*sql*/)> = None;

    let mut should_close = false;
    egui::Window::new("Generate Query Index")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(450.0)
            .max_height(150.0)
            .open(&mut open_flag)
            .show(ctx, |ui| {
                ui.vertical(|ui| {

                    // Fields - aligned using a two-column Grid
                    ui.add_space(4.0);
                    egui::Grid::new("index_form_grid").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                        ui.label("Index name");
                        ui.add(egui::TextEdit::singleline(&mut working.index_name).desired_width(360.0));
                        ui.end_row();

                        ui.label("Columns");
                        ui.add(egui::TextEdit::singleline(&mut working.columns).desired_width(360.0));
                        ui.end_row();

                        ui.label("Unique");
                        ui.checkbox(&mut working.unique, "");
                        ui.end_row();

                        ui.label("Method");
                        // Determine db type for appropriate method options
                        let db_type = tabular
                            .connections
                            .iter()
                            .find(|c| c.id == Some(working.connection_id))
                            .map(|c| c.connection_type.clone())
                            .unwrap_or(working.db_type.clone());
                        match db_type {
                            crate::models::enums::DatabaseType::SQLite | crate::models::enums::DatabaseType::Redis => {
                                                        ui.label(egui::RichText::new("N/A").italics().color(egui::Color32::GRAY));
                                                        working.method = None;
                                                    }
                            crate::models::enums::DatabaseType::MySQL => {
                                                        let options = ["BTREE", "HASH"];
                                                        let mut selected = working.method.clone().unwrap_or_else(|| options[0].to_string());
                                                        egui::ComboBox::from_label("")
                                                            .selected_text(selected.clone())
                                                            .show_ui(ui, |ui| {
                                                                for opt in options.iter() {
                                                                    ui.selectable_value(&mut selected, opt.to_string(), *opt);
                                                                }
                                                            });
                                                        working.method = Some(selected);
                                                    }
                            crate::models::enums::DatabaseType::PostgreSQL => {
                                                        let options = ["btree", "hash", "gist", "gin", "spgist", "brin"];
                                                        let mut selected = working.method.clone().unwrap_or_else(|| options[0].to_string());
                                                        egui::ComboBox::from_label("")
                                                            .selected_text(selected.clone())
                                                            .show_ui(ui, |ui| {
                                                                for opt in options.iter() {
                                                                    ui.selectable_value(&mut selected, opt.to_string(), *opt);
                                                                }
                                                            });
                                                        working.method = Some(selected);
                                                    }
                            crate::models::enums::DatabaseType::MsSQL => {
                                                        let options = ["NONCLUSTERED", "CLUSTERED"];
                                                        let mut selected = working.method.clone().unwrap_or_else(|| options[0].to_string());
                                                        egui::ComboBox::from_label("")
                                                            .selected_text(selected.clone())
                                                            .show_ui(ui, |ui| {
                                                                for opt in options.iter() {
                                                                    ui.selectable_value(&mut selected, opt.to_string(), *opt);
                                                                }
                                                            });
                                                        working.method = Some(selected);
                                                    }
                            crate::models::enums::DatabaseType::MongoDB => {
                                // MongoDB index "method" is the key spec (1/-1 per field), handled via Columns text.
                                // Show a small hint instead of an algorithm picker.
                                ui.label("Use Columns as 'field1:1, field2:-1'");
                            }
                        }
                        ui.end_row();
                    });

                    ui.add_space(8.0);


                    // Build SQL preview string depending on the connection type.
                    let sql_preview = {
                        let conn = tabular.connections.iter().find(|c| c.id == Some(working.connection_id));
                        if let Some(conn) = conn {
                            use crate::models::enums::DatabaseType;
                            match (working.mode.clone(), conn.connection_type.clone()) {
                                (crate::models::structs::IndexDialogMode::Create, DatabaseType::MySQL) => {
                                                                let method = working.method.clone().unwrap_or("BTREE".to_string());
                                                                format!("CREATE {unique} INDEX `{name}` ON `{table}` ({cols}) USING {method};",
                                                                    unique = if working.unique {"UNIQUE"} else {""},
                                                                    name = working.index_name,
                                                                    table = working.table_name,
                                                                    cols = working.columns,
                                                                    method = method
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Create, DatabaseType::PostgreSQL) => {
                                                                let schema = working.database_name.clone().unwrap_or_else(|| "public".to_string());
                                                                let method = working.method.clone().unwrap_or("btree".to_string());
                                                                format!("CREATE {unique} INDEX {name} ON \"{schema}\".\"{table}\" USING {method} ({cols});",
                                                                    unique = if working.unique {"UNIQUE"} else {""},
                                                                    name = working.index_name,
                                                                    schema = schema,
                                                                    table = working.table_name,
                                                                    cols = working.columns,
                                                                    method = method
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Create, DatabaseType::SQLite) => {
                                                                format!("CREATE {unique} INDEX IF NOT EXISTS \"{name}\" ON \"{table}\"({cols});",
                                                                    unique = if working.unique {"UNIQUE"} else {""},
                                                                    name = working.index_name,
                                                                    table = working.table_name,
                                                                    cols = working.columns,
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Create, DatabaseType::MsSQL) => {
                                                                let db = working.database_name.clone().unwrap_or_else(|| conn.database.clone());
                                                                let clustered = working.method.clone().unwrap_or("NONCLUSTERED".to_string());
                                                                format!("USE [{db}];\nCREATE {unique} {clustered} INDEX [{name}] ON [dbo].[{table}] ({cols});",
                                                                    unique = if working.unique {"UNIQUE"} else {""},
                                                                    name = working.index_name,
                                                                    db = db,
                                                                    clustered = clustered,
                                                                    table = working.table_name,
                                                                    cols = working.columns,
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Create, DatabaseType::Redis) => {
                                                                "-- Not applicable for Redis".to_string()
                                                            }
                                (crate::models::structs::IndexDialogMode::Edit, DatabaseType::MySQL) => {
                                                                let idx = working.existing_index_name.clone().unwrap_or(working.index_name.clone());
                                                                let method = working.method.clone().unwrap_or("BTREE".to_string());
                                                                format!("-- MySQL has no ALTER INDEX; typically DROP then CREATE\nALTER TABLE `{table}` DROP INDEX `{idx}`;\nCREATE {unique} INDEX `{name}` ON `{table}` ({cols}) USING {method};",
                                                                    unique = if working.unique {"UNIQUE"} else {""},
                                                                    name = working.index_name,
                                                                    table = working.table_name,
                                                                    cols = working.columns,
                                                                    method = method,
                                                                    idx = idx,
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Edit, DatabaseType::PostgreSQL) => {
                                                                let idx = working.existing_index_name.clone().unwrap_or(working.index_name.clone());
                                                                format!("-- PostgreSQL example edits\nALTER INDEX IF EXISTS \"{idx}\" RENAME TO \"{new}\";\n-- or REBUILD/SET options\n-- ALTER INDEX IF EXISTS \"{new}\" SET (fillfactor = 90);",
                                                                    idx = idx,
                                                                    new = working.index_name,
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Edit, DatabaseType::SQLite) => {
                                                                let idx = working.existing_index_name.clone().unwrap_or(working.index_name.clone());
                                                                format!("-- SQLite has no ALTER INDEX; DROP and CREATE\nDROP INDEX IF EXISTS \"{idx}\";\nCREATE {unique} INDEX \"{name}\" ON \"{table}\"({cols});",
                                                                    unique = if working.unique {"UNIQUE"} else {""},
                                                                    name = working.index_name,
                                                                    table = working.table_name,
                                                                    cols = working.columns,
                                                                    idx = idx,
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Edit, DatabaseType::MsSQL) => {
                                                                let db = working.database_name.clone().unwrap_or_else(|| conn.database.clone());
                                                                let idx = working.existing_index_name.clone().unwrap_or(working.index_name.clone());
                                                                format!("USE [{db}];\nALTER INDEX [{idx}] ON [dbo].[{table}] REBUILD;\n-- To rename: EXEC sp_rename N'[dbo].[{idx}]', N'{new}', N'INDEX';",
                                                                    db = db,
                                                                    idx = idx,
                                                                    table = working.table_name,
                                                                    new = working.index_name,
                                                                )
                                                            }
                                (crate::models::structs::IndexDialogMode::Edit, DatabaseType::Redis) => {
                                                                "-- Not applicable for Redis".to_string()
                                                            }
                                (crate::models::structs::IndexDialogMode::Create, DatabaseType::MongoDB) => {
                                    // Build MongoDB createIndex JavaScript snippet
                                    let db = working
                                        .database_name
                                        .clone()
                                        .unwrap_or_else(|| conn.database.clone());
                                    // Parse columns into key doc: "a:1, b:-1" or plain "a,b" => "a:1,b:1"
                                    let cols_raw = working.columns.clone();
                                    let keys: Vec<String> = cols_raw
                                        .split(',')
                                        .map(|s| s.trim())
                                        .filter(|s| !s.is_empty())
                                        .map(|tok| if tok.contains(':') { tok.to_string() } else { format!("{}: 1", tok) })
                                        .collect();
                                    let keys_doc = if keys.is_empty() { "_id: 1".to_string() } else { keys.join(", ") };
                                    format!(
                                        "db.{}.{}.createIndex({{{}}}, {{ name: \"{}\", unique: {} }});",
                                        db,
                                        working.table_name,
                                        keys_doc,
                                        working.index_name,
                                        if working.unique { "true" } else { "false" }
                                    )
                                }
                                (crate::models::structs::IndexDialogMode::Edit, DatabaseType::MongoDB) => {
                                    let db = working
                                        .database_name
                                        .clone()
                                        .unwrap_or_else(|| conn.database.clone());
                                    let target_idx = working
                                        .existing_index_name
                                        .clone()
                                        .unwrap_or_else(|| working.index_name.clone());
                                    let cols_raw = working.columns.clone();
                                    let keys: Vec<String> = cols_raw
                                        .split(',')
                                        .map(|s| s.trim())
                                        .filter(|s| !s.is_empty())
                                        .map(|tok| if tok.contains(':') { tok.to_string() } else { format!("{}: 1", tok) })
                                        .collect();
                                    let keys_doc = if keys.is_empty() { "_id: 1".to_string() } else { keys.join(", ") };
                                    let drop_cmd = format!(
                                        "db.{}.{}.dropIndex(\"{}\");",
                                        db, working.table_name, target_idx
                                    );
                                    let create_cmd = format!(
                                        "db.{}.{}.createIndex({{{}}}, {{ name: \"{}\", unique: {} }});",
                                        db,
                                        working.table_name,
                                        keys_doc,
                                        working.index_name,
                                        if working.unique { "true" } else { "false" }
                                    );
                                    format!(
                                        "// MongoDB has no ALTER INDEX; typically drop and recreate\n{}\n{}",
                                        drop_cmd,
                                        create_cmd
                                    )
                                }
                            }
                        } else {
                            "-- No connection selected".to_string()
                        }
                    };

                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| { ui.code(sql_preview.clone()); });

                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let big_btn = egui::Button::new(egui::RichText::new("Open in Editor").strong())
                            .min_size(egui::vec2(150.0, 30.0));
                        if ui.add(big_btn).clicked() {
                            let title = match working.mode {
                                crate::models::structs::IndexDialogMode::Create => format!("Create Index on {}", working.table_name),
                                crate::models::structs::IndexDialogMode::Edit => format!("Edit Index {}", working.index_name),
                            };
                            open_tab_request = Some((title, sql_preview.clone()));
                            should_close = true; // close dialog after UI
                        }
                    });
                });
            });
    // Persist user edits back into app state
    tabular.index_dialog = Some(working);
    // Update dialog visibility from open_flag set in UI
    if should_close {
        open_flag = false;
    }
    tabular.show_index_dialog = open_flag;
    // If user requested opening a tab, do it now (outside of UI borrow)
    if let Some((title, sql)) = open_tab_request
        && let Some(state) = &tabular.index_dialog
    {
        editor::create_new_tab_with_connection_and_database(
            tabular,
            title,
            sql,
            Some(state.connection_id),
            state.database_name.clone(),
        );
    }
}

pub(crate) fn render_create_table_dialog(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
) {
    if !tabular.show_create_table_dialog {
        return;
    }

    if tabular.create_table_wizard.is_none() {
        tabular.show_create_table_dialog = false;
        tabular.create_table_error = None;
        return;
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum WizardAction {
        None,
        Cancel,
        Back,
        Next,
        Create,
    }

    let preview_result = tabular
        .create_table_wizard
        .as_ref()
        .and_then(|state| {
            if state.current_step == models::structs::CreateTableWizardStep::Review {
                let state_clone = state.clone();
                Some(tabular.generate_create_table_sql(&state_clone))
            } else {
                None
            }
        });

    let connection_caption = tabular
        .create_table_wizard
        .as_ref()
        .and_then(|state| {
            tabular
                .connections
                .iter()
                .find(|c| c.id == Some(state.connection_id))
                .map(|conn| conn.name.clone())
        })
        .unwrap_or_else(|| "Selected connection".to_string());

    let mut action = WizardAction::None;
    let mut copy_preview: Option<String> = None;
    let mut keep_open = tabular.show_create_table_dialog;

    egui::Window::new("Create Table Wizard")
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .min_width(520.0)
        .min_height(380.0)
        .open(&mut keep_open)
        .show(ctx, |ui| {
            let Some(state) = tabular.create_table_wizard.as_mut() else {
                action = WizardAction::Cancel;
                ui.label("Wizard state unavailable.");
                return;
            };

            let current_step = state.current_step;

            ui.horizontal(|ui| {
                for step in models::structs::CreateTableWizardStep::all_steps() {
                    let active = step == current_step;
                    let bullet = if active { "●" } else { "○" };
                    ui.label(
                        egui::RichText::new(format!("{} {}", bullet, step.title()))
                            .color(if active {
                                ui.visuals().strong_text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            })
                            .strong(),
                    );
                }
            });
            ui.separator();

            // Connection summary
            ui.label(
                egui::RichText::new(format!("Connection: {}", connection_caption.clone()))
                    .strong(),
            );
            if let Some(db_name) = state.database_name.as_ref() {
                ui.label(format!("Target: {}", db_name));
            } else {
                ui.label("Target: connection default database/schema");
            }
            ui.add_space(6.0);

            match current_step {
                models::structs::CreateTableWizardStep::Basics => {
                    ui.heading("Basics");
                    ui.label("Name the table and optional schema/database context.");
                    ui.add_space(6.0);

                    let response = ui.text_edit_singleline(&mut state.table_name);
                    if response.changed() {
                        tabular.create_table_error = None;
                    }

                    let mut target_text = state.database_name.clone().unwrap_or_default();
                    let target_label = match state.db_type {
                        models::enums::DatabaseType::PostgreSQL => "Schema (optional)",
                        models::enums::DatabaseType::SQLite => "Database (read-only)",
                        models::enums::DatabaseType::MySQL
                        | models::enums::DatabaseType::MsSQL => "Database (optional)",
                        models::enums::DatabaseType::Redis
                        | models::enums::DatabaseType::MongoDB => "Database",
                    };

                    ui.add_space(4.0);
                    ui.label(target_label);
                    match state.db_type {
                        models::enums::DatabaseType::SQLite => {
                            let display = if target_text.is_empty() {
                                "[using connection default]"
                            } else {
                                target_text.as_str()
                            };
                            ui.label(display);
                        }
                        models::enums::DatabaseType::Redis
                        | models::enums::DatabaseType::MongoDB => {
                            // Nothing to edit; keep info only
                            let display = if target_text.is_empty() {
                                "[not applicable]"
                            } else {
                                target_text.as_str()
                            };
                            ui.label(display);
                        }
                        _ => {
                            if ui.text_edit_singleline(&mut target_text).changed() {
                                tabular.create_table_error = None;
                            }
                            let normalized = target_text.trim();
                            state.database_name = if normalized.is_empty() {
                                None
                            } else {
                                Some(normalized.to_string())
                            };
                        }
                    }

                    ui.add_space(6.0);
                    ui.label("Tip: names are quoted automatically when needed.");
                }
                models::structs::CreateTableWizardStep::Columns => {
                    ui.heading("Columns");
                    ui.label("Define each column, including data type and primary key settings.");
                    ui.add_space(4.0);

                    let mut remove_idx: Option<usize> = None;
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            egui::Grid::new("create_table_columns_grid")
                                .striped(true)
                                .num_columns(6)
                                .spacing([12.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("Name").strong());
                                    ui.label(egui::RichText::new("Type").strong());
                                    ui.label(egui::RichText::new("Allow NULL").strong());
                                    ui.label(egui::RichText::new("Default").strong());
                                    ui.label(egui::RichText::new("Primary Key").strong());
                                    ui.label(egui::RichText::new(" ").strong());
                                    ui.end_row();

                                    for (idx, column) in state.columns.iter_mut().enumerate() {
                                        if ui.text_edit_singleline(&mut column.name).changed() {
                                            tabular.create_table_error = None;
                                        }
                                        if ui
                                            .text_edit_singleline(&mut column.data_type)
                                            .changed()
                                        {
                                            tabular.create_table_error = None;
                                        }

                                        if ui.checkbox(&mut column.allow_null, "").changed() {
                                            if column.is_primary_key {
                                                column.allow_null = false;
                                            }
                                            tabular.create_table_error = None;
                                        }

                                        if ui
                                            .text_edit_singleline(&mut column.default_value)
                                            .changed()
                                        {
                                            tabular.create_table_error = None;
                                        }

                                        if ui.checkbox(&mut column.is_primary_key, "").changed() {
                                            column.allow_null = false;
                                            tabular.create_table_error = None;
                                        }

                                        if idx > 0 {
                                            if ui.button("🗑").clicked() {
                                                remove_idx = Some(idx);
                                            }
                                        } else {
                                            ui.label(" ");
                                        }
                                        ui.end_row();
                                    }
                                });
                        });

                    if let Some(idx) = remove_idx {
                        state.columns.remove(idx);
                        tabular.create_table_error = None;
                    }

                    ui.add_space(6.0);
                    if ui.button("➕ Add Column").clicked() {
                        let new_col = models::structs::TableColumnDefinition::blank(state.columns.len());
                        state.columns.push(new_col);
                        tabular.create_table_error = None;
                    }
                }
                models::structs::CreateTableWizardStep::Indexes => {
                    ui.heading("Indexes");
                    ui.label("Optionally add secondary indexes.");
                    ui.add_space(4.0);

                    let mut remove_idx: Option<usize> = None;
                    if state.indexes.is_empty() {
                        ui.label("No indexes defined.");
                    } else {
                        egui::Grid::new("create_table_indexes_grid")
                            .striped(true)
                            .num_columns(4)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("Name").strong());
                                ui.label(egui::RichText::new("Columns (comma-separated)").strong());
                                ui.label(egui::RichText::new("Unique").strong());
                                ui.label(egui::RichText::new(" ").strong());
                                ui.end_row();

                                for (idx, index_def) in state.indexes.iter_mut().enumerate() {
                                    if ui.text_edit_singleline(&mut index_def.name).changed() {
                                        tabular.create_table_error = None;
                                    }
                                    if ui
                                        .text_edit_singleline(&mut index_def.columns)
                                        .changed()
                                    {
                                        tabular.create_table_error = None;
                                    }
                                    if ui.checkbox(&mut index_def.unique, "").changed() {
                                        tabular.create_table_error = None;
                                    }
                                    if ui.button("🗑").clicked() {
                                        remove_idx = Some(idx);
                                    }
                                    ui.end_row();
                                }
                            });
                    }

                    if let Some(idx) = remove_idx {
                        state.indexes.remove(idx);
                        tabular.create_table_error = None;
                    }

                    ui.add_space(6.0);
                    if ui.button("➕ Add Index").clicked() {
                        let new_index =
                            models::structs::TableIndexDefinition::blank(state.indexes.len());
                        state.indexes.push(new_index);
                        tabular.create_table_error = None;
                    }
                }
                models::structs::CreateTableWizardStep::Review => {
                    ui.heading("Review");
                    ui.label("Preview the generated SQL before creating the table.");
                    ui.add_space(4.0);

                    match preview_result.as_ref() {
                        Some(Ok(sql)) => {
                            let mut preview_text = sql.clone();
                            ui.add(
                                egui::TextEdit::multiline(&mut preview_text)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_rows(12)
                                    .interactive(false),
                            );
                        }
                        Some(Err(err)) => {
                            ui.colored_label(egui::Color32::from_rgb(192, 57, 43), err);
                        }
                        None => {
                            ui.label("SQL preview will appear after completing the previous steps.");
                        }
                    }
                }
            }

            if let Some(err) = tabular.create_table_error.as_ref() {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(192, 57, 43), err);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    action = WizardAction::Cancel;
                }

                if current_step.previous().is_some() {
                    if ui.button("Back").clicked() {
                        action = WizardAction::Back;
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if current_step == models::structs::CreateTableWizardStep::Review {
                        let create_enabled = preview_result
                            .as_ref()
                            .map(|res| res.is_ok())
                            .unwrap_or(false);
                        if ui
                            .add_enabled(create_enabled, egui::Button::new("Create Table"))
                            .clicked()
                        {
                            action = WizardAction::Create;
                        }
                        if let Some(Ok(sql)) = preview_result.as_ref() {
                            if ui.button("Copy SQL").clicked() {
                                copy_preview = Some(sql.clone());
                            }
                        }
                    } else if ui.button("Next").clicked() {
                        action = WizardAction::Next;
                    }
                });
            });
        });

    if let Some(sql) = copy_preview {
        ctx.copy_text(sql);
    }

    if !keep_open {
        action = WizardAction::Cancel;
    }

    match action {
        WizardAction::Cancel => {
            tabular.create_table_wizard = None;
            tabular.create_table_error = None;
            tabular.show_create_table_dialog = false;
        }
        WizardAction::Back => {
            if let Some(state) = tabular.create_table_wizard.as_mut() {
                if let Some(prev) = state.current_step.previous() {
                    state.current_step = prev;
                }
            }
            tabular.create_table_error = None;
            tabular.show_create_table_dialog = true;
        }
        WizardAction::Next => {
            if let Some(mut state) = tabular.create_table_wizard.take() {
                let current_step = state.current_step;
                if let Some(err) = tabular.validate_create_table_step(&mut state, current_step) {
                    tabular.create_table_error = Some(err);
                } else {
                    tabular.create_table_error = None;
                    if let Some(next) = state.current_step.next() {
                        state.current_step = next;
                    }
                }
                tabular.create_table_wizard = Some(state);
            }
            tabular.show_create_table_dialog = true;
        }
        WizardAction::Create => {
            if let Some(state) = tabular.create_table_wizard.clone() {
                tabular.create_table_error = None;
                tabular.submit_create_table_wizard(state);
            }
        }
        WizardAction::None => {
            tabular.show_create_table_dialog = true;
        }
    }
}
