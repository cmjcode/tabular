use eframe::egui;
use log::error;

use crate::{editor, models, window_egui};

/// Helper function to paint cursor for TextEdit fields (fix for egui singleline cursor bug)
fn paint_text_edit_cursor(
    ui: &egui::Ui,
    response: &egui::Response,
    text_edit_id: egui::Id,
    text: &str,
) {
    if let Some(text_state) = egui::TextEdit::load_state(ui.ctx(), text_edit_id)
        && let Some(cursor_range) = text_state.cursor.char_range()
    {
        let cursor_pos = cursor_range.primary.index;

        // Calculate cursor X position from actual text width
        let text_before_cursor = if cursor_pos <= text.len() {
            &text[..cursor_pos]
        } else {
            text
        };

        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let galley = ui.fonts(|f| {
            f.layout_no_wrap(
                text_before_cursor.to_string(),
                font_id,
                ui.visuals().text_color(),
            )
        });
        let text_width = galley.rect.width();

        // Position cursor in response rect
        let text_margin = 4.0; // TextEdit internal margin
        let caret_x = response.rect.min.x + text_margin + text_width;
        let caret_top = response.rect.min.y + 2.0;
        let caret_bottom = response.rect.max.y - 2.0;

        // Paint visible cursor
        let cursor_color = ui.visuals().text_cursor.stroke.color;
        let cursor_width = 2.0;
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(caret_x - cursor_width / 2.0, caret_top),
                egui::pos2(caret_x + cursor_width / 2.0, caret_bottom),
            ),
            0.0,
            cursor_color,
        );
    }
}

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
                    ui.label(
                        "Credit : Pamungkas Jayuda (https://github.com/Jayuda), Mualip Suhal (https://github.com/msuhal),  Davin Adesta Putra (https://github.com/Davin-adesta), Mohamad Ardiansah Pratama (https://github.com/ardiansyah20007) ",
                    );
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
                    let filename_resp = ui.add(
                        egui::TextEdit::singleline(&mut tabular.save_filename).cursor_at_end(false),
                    );
                    if filename_resp.clicked() || filename_resp.gained_focus() {
                        filename_resp.request_focus();
                        ui.ctx().request_repaint();
                    }

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
                    let name_resp = ui.add(
                        egui::TextEdit::singleline(&mut working.index_name)
                            .desired_width(360.0)
                            .cursor_at_end(false)
                    );
                    if name_resp.clicked() || name_resp.gained_focus() {
                        name_resp.request_focus();
                        ui.ctx().request_repaint();
                    }
                    ui.end_row();

                    ui.label("Columns");
                    let cols_resp = ui.add(
                        egui::TextEdit::singleline(&mut working.columns)
                            .desired_width(360.0)
                            .cursor_at_end(false)
                    );
                    if cols_resp.clicked() || cols_resp.gained_focus() {
                        cols_resp.request_focus();
                        ui.ctx().request_repaint();
                    }
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
                        crate::models::enums::DatabaseType::SQLite
                        | crate::models::enums::DatabaseType::Redis => {
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
                    let conn = tabular
                        .connections
                        .iter()
                        .find(|c| c.id == Some(working.connection_id));
                    if let Some(conn) = conn {
                        use crate::models::enums::DatabaseType;
                        match (working.mode.clone(), conn.connection_type.clone()) {
                            (crate::models::structs::IndexDialogMode::Create, DatabaseType::MySQL) => {
                                let method = working.method.clone().unwrap_or("BTREE".to_string());
                                format!(
                                    "CREATE {unique} INDEX `{name}` ON `{table}` ({cols}) USING {method};",
                                    unique = if working.unique { "UNIQUE" } else { "" },
                                    name = working.index_name,
                                    table = working.table_name,
                                    cols = working.columns,
                                    method = method
                                )
                            }
                            (crate::models::structs::IndexDialogMode::Create, DatabaseType::PostgreSQL) => {
                                let schema = working.database_name.clone().unwrap_or_else(|| "public".to_string());
                                let method = working.method.clone().unwrap_or("btree".to_string());
                                format!(
                                    "CREATE {unique} INDEX {name} ON \"{schema}\".\"{table}\" USING {method} ({cols});",
                                    unique = if working.unique { "UNIQUE" } else { "" },
                                    name = working.index_name,
                                    schema = schema,
                                    table = working.table_name,
                                    cols = working.columns,
                                    method = method
                                )
                            }
                            (crate::models::structs::IndexDialogMode::Create, DatabaseType::SQLite) => {
                                format!(
                                    "CREATE {unique} INDEX IF NOT EXISTS \"{name}\" ON \"{table}\"({cols});",
                                    unique = if working.unique { "UNIQUE" } else { "" },
                                    name = working.index_name,
                                    table = working.table_name,
                                    cols = working.columns,
                                )
                            }
                            (crate::models::structs::IndexDialogMode::Create, DatabaseType::MsSQL) => {
                                let db = working.database_name.clone().unwrap_or_else(|| conn.database.clone());
                                let clustered = working.method.clone().unwrap_or("NONCLUSTERED".to_string());
                                format!(
                                    "USE [{db}];\nCREATE {unique} {clustered} INDEX [{name}] ON [dbo].[{table}] ({cols});",
                                    unique = if working.unique { "UNIQUE" } else { "" },
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
                                let idx = working
                                    .existing_index_name
                                    .clone()
                                    .unwrap_or(working.index_name.clone());
                                let method = working.method.clone().unwrap_or("BTREE".to_string());
                                format!(
                                    "-- MySQL has no ALTER INDEX; typically DROP then CREATE\nALTER TABLE `{table}` DROP INDEX `{idx}`;\nCREATE {unique} INDEX `{name}` ON `{table}` ({cols}) USING {method};",
                                    unique = if working.unique { "UNIQUE" } else { "" },
                                    name = working.index_name,
                                    table = working.table_name,
                                    cols = working.columns,
                                    method = method,
                                    idx = idx,
                                )
                            }
                            (crate::models::structs::IndexDialogMode::Edit, DatabaseType::PostgreSQL) => {
                                let idx = working
                                    .existing_index_name
                                    .clone()
                                    .unwrap_or(working.index_name.clone());
                                format!(
                                    "-- PostgreSQL example edits\nALTER INDEX IF EXISTS \"{idx}\" RENAME TO \"{new}\";\n-- or REBUILD/SET options\n-- ALTER INDEX IF EXISTS \"{new}\" SET (fillfactor = 90);",
                                    idx = idx,
                                    new = working.index_name,
                                )
                            }
                            (crate::models::structs::IndexDialogMode::Edit, DatabaseType::SQLite) => {
                                let idx = working
                                    .existing_index_name
                                    .clone()
                                    .unwrap_or(working.index_name.clone());
                                format!(
                                    "-- SQLite has no ALTER INDEX; DROP and CREATE\nDROP INDEX IF EXISTS \"{idx}\";\nCREATE {unique} INDEX \"{name}\" ON \"{table}\"({cols});",
                                    unique = if working.unique { "UNIQUE" } else { "" },
                                    name = working.index_name,
                                    table = working.table_name,
                                    cols = working.columns,
                                    idx = idx,
                                )
                            }
                            (crate::models::structs::IndexDialogMode::Edit, DatabaseType::MsSQL) => {
                                let db = working.database_name.clone().unwrap_or_else(|| conn.database.clone());
                                let idx = working
                                    .existing_index_name
                                    .clone()
                                    .unwrap_or(working.index_name.clone());
                                format!(
                                    "USE [{db}];\nALTER INDEX [{idx}] ON [dbo].[{table}] REBUILD;\n-- To rename: EXEC sp_rename N'[dbo].[{idx}]', N'{new}', N'INDEX';",
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

                egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                    ui.code(sql_preview.clone());
                });

                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let big_btn = egui::Button::new(egui::RichText::new("Open in Editor").strong())
                        .min_size(egui::vec2(150.0, 30.0));
                    if ui.add(big_btn).clicked() {
                        let title = match working.mode {
                            crate::models::structs::IndexDialogMode::Create => {
                                format!("Create Index on {}", working.table_name)
                            }
                            crate::models::structs::IndexDialogMode::Edit => {
                                format!("Edit Index {}", working.index_name)
                            }
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

pub(crate) fn render_create_table_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
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

    let preview_result = tabular.create_table_wizard.as_ref().and_then(|state| {
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
        .default_width(680.0)
        .min_width(640.0)
        .min_height(420.0)
        .open(&mut keep_open)
        .show(ctx, |ui| {
            let Some(state) = tabular.create_table_wizard.as_mut() else {
                action = WizardAction::Cancel;
                ui.label("Wizard state unavailable.");
                return;
            };

            let current_step = state.current_step;
            let steps = models::structs::CreateTableWizardStep::all_steps();
            let Some(active_index) = steps.iter().position(|s| *s == current_step) else {
                return;
            };
            let total_steps = steps.len().max(1);
            let progress_fraction = (active_index + 1) as f32 / total_steps as f32;

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    for (idx, step) in steps.iter().enumerate() {
                        let active = idx == active_index;
                        let bullet = if active { "🔥" } else { "○" };
                        let label = format!("{} {}", bullet, step.title());
                        ui.label(egui::RichText::new(label).strong().color(if active {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().weak_text_color()
                        }));
                    }
                });
                ui.add(
                    egui::ProgressBar::new(progress_fraction)
                        .desired_width(ui.available_width())
                        .fill(egui::Color32::from_rgb(255, 21, 0)), // rgba(255, 21, 0, 1)
                );
            });

            ui.add_space(8.0);

            egui::Frame::group(ui.style())
                .inner_margin(egui::Vec2::new(12.0, 10.0))
                .corner_radius(egui::CornerRadius::same(8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Connection").strong());
                        ui.separator();
                        ui.label(connection_caption.clone());
                    });
                    ui.add_space(4.0);
                    let target_text = state.database_name.as_deref().unwrap_or("[default schema]");
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Target").strong());
                        ui.separator();
                        ui.label(target_text);
                    });
                });

            ui.add_space(12.0);

            match current_step {
                models::structs::CreateTableWizardStep::Basics => {
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Vec2::new(16.0, 14.0))
                        .corner_radius(egui::CornerRadius::same(10))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Table identity").strong().size(16.0));
                            ui.add_space(8.0);
                            egui::Grid::new("create_table_basics_grid")
                                .num_columns(2)
                                .spacing([18.0, 12.0])
                                .striped(false)
                                .show(ui, |ui| {
                                    ui.label("Table name");
                                    let field_width = ui.available_width();
                                    let text_edit_id = ui.id().with("table_name_field");
                                    let response = ui.add_sized(
                                        [field_width, 0.0],
                                        egui::TextEdit::singleline(&mut state.table_name)
                                            .cursor_at_end(false)
                                            .id(text_edit_id),
                                    );

                                    if response.clicked() || response.gained_focus() {
                                        ui.memory_mut(|mem| mem.request_focus(text_edit_id));
                                        if let Some(mut state_inner) =
                                            egui::TextEdit::load_state(ui.ctx(), text_edit_id)
                                        {
                                            use egui::text::{CCursor, CCursorRange};
                                            state_inner.cursor.set_char_range(Some(
                                                CCursorRange::one(CCursor::new(0)),
                                            ));
                                            state_inner.store(ui.ctx(), text_edit_id);
                                        }
                                        ui.ctx().request_repaint();
                                    }

                                    // Custom cursor painting
                                    paint_text_edit_cursor(
                                        ui,
                                        &response,
                                        text_edit_id,
                                        &state.table_name,
                                    );

                                    if response.changed() {
                                        tabular.create_table_error = None;
                                    }
                                    ui.end_row();

                                    let mut target_text =
                                        state.database_name.clone().unwrap_or_default();
                                    let target_label = match state.db_type {
                                        models::enums::DatabaseType::PostgreSQL => {
                                            "Schema (optional)"
                                        }
                                        models::enums::DatabaseType::SQLite => {
                                            "Database (read-only)"
                                        }
                                        models::enums::DatabaseType::MySQL
                                        | models::enums::DatabaseType::MsSQL => {
                                            "Database (optional)"
                                        }
                                        models::enums::DatabaseType::Redis
                                        | models::enums::DatabaseType::MongoDB => "Database",
                                    };
                                    ui.label(target_label);
                                    match state.db_type {
                                        models::enums::DatabaseType::SQLite => {
                                            let display = if target_text.is_empty() {
                                                "[using connection default]".to_string()
                                            } else {
                                                target_text.clone()
                                            };
                                            ui.label(display);
                                        }
                                        models::enums::DatabaseType::Redis
                                        | models::enums::DatabaseType::MongoDB => {
                                            let display = if target_text.is_empty() {
                                                "[not applicable]".to_string()
                                            } else {
                                                target_text.clone()
                                            };
                                            ui.label(display);
                                        }
                                        _ => {
                                            let db_field_width = ui.available_width();
                                            let db_response = ui.add_sized(
                                                [db_field_width, 0.0],
                                                egui::TextEdit::singleline(&mut target_text)
                                                    .cursor_at_end(false),
                                            );
                                            if db_response.clicked() || db_response.gained_focus() {
                                                db_response.request_focus();
                                                ui.ctx().request_repaint();
                                            }
                                            if db_response.changed() {
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
                                    ui.end_row();

                                    ui.label("Notes");
                                    ui.label(
                                        egui::RichText::new(
                                            "Names are quoted automatically when required.",
                                        )
                                        .color(ui.visuals().weak_text_color()),
                                    );
                                    ui.end_row();
                                });
                        });
                }
                models::structs::CreateTableWizardStep::Columns => {
                    ui.label(
                        egui::RichText::new("Define the structure of the table")
                            .strong()
                            .size(16.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(
                            "Set column data types, defaults and primary key flags.",
                        )
                        .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(12.0);

                    let mut remove_idx: Option<usize> = None;
                    let frame_width = ui.available_width();
                    let name_width = frame_width * 0.24;
                    let type_width = frame_width * 0.2;
                    let default_width = frame_width * 0.26;

                    egui::Frame::group(ui.style())
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Vec2::new(12.0, 10.0))
                        .show(ui, |ui| {
                            let inner_width = ui.available_width();
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .max_height(260.0)
                                .show(ui, |ui| {
                                    ui.set_width(inner_width - 8.0);
                                    egui::Grid::new("create_table_columns_grid")
                                        .striped(true)
                                        .num_columns(6)
                                        .spacing([20.0, 12.0])
                                        .min_row_height(28.0)
                                        .show(ui, |ui| {
                                            ui.label(egui::RichText::new("Name").strong());
                                            ui.label(egui::RichText::new("Type").strong());
                                            ui.label(egui::RichText::new("Allow NULL").strong());
                                            ui.label(egui::RichText::new("Default").strong());
                                            ui.label(egui::RichText::new("Primary Key").strong());
                                            ui.label(egui::RichText::new(" ").strong());
                                            ui.end_row();

                                            for (idx, column) in
                                                state.columns.iter_mut().enumerate()
                                            {
                                                // Column name field
                                                let name_id = ui.id().with(("col_name", idx));
                                                let name_resp = ui.add_sized(
                                                    [name_width, 0.0],
                                                    egui::TextEdit::singleline(&mut column.name)
                                                        .cursor_at_end(false)
                                                        .id(name_id),
                                                );
                                                if name_resp.clicked() || name_resp.gained_focus() {
                                                    ui.memory_mut(|mem| mem.request_focus(name_id));
                                                    ui.ctx().request_repaint();
                                                }
                                                paint_text_edit_cursor(
                                                    ui,
                                                    &name_resp,
                                                    name_id,
                                                    &column.name,
                                                );
                                                if name_resp.changed() {
                                                    tabular.create_table_error = None;
                                                }

                                                // Column type field
                                                let type_id = ui.id().with(("col_type", idx));
                                                let type_resp = ui.add_sized(
                                                    [type_width, 0.0],
                                                    egui::TextEdit::singleline(
                                                        &mut column.data_type,
                                                    )
                                                    .cursor_at_end(false)
                                                    .id(type_id),
                                                );
                                                if type_resp.clicked() || type_resp.gained_focus() {
                                                    ui.memory_mut(|mem| mem.request_focus(type_id));
                                                    ui.ctx().request_repaint();
                                                }
                                                paint_text_edit_cursor(
                                                    ui,
                                                    &type_resp,
                                                    type_id,
                                                    &column.data_type,
                                                );
                                                if type_resp.changed() {
                                                    tabular.create_table_error = None;
                                                }

                                                if ui.checkbox(&mut column.allow_null, "").changed()
                                                {
                                                    if column.is_primary_key {
                                                        column.allow_null = false;
                                                    }
                                                    tabular.create_table_error = None;
                                                }

                                                // Column default field
                                                let default_id = ui.id().with(("col_default", idx));
                                                let default_resp = ui.add_sized(
                                                    [default_width, 0.0],
                                                    egui::TextEdit::singleline(
                                                        &mut column.default_value,
                                                    )
                                                    .cursor_at_end(false)
                                                    .id(default_id),
                                                );
                                                if default_resp.clicked()
                                                    || default_resp.gained_focus()
                                                {
                                                    ui.memory_mut(|mem| {
                                                        mem.request_focus(default_id)
                                                    });
                                                    ui.ctx().request_repaint();
                                                }
                                                paint_text_edit_cursor(
                                                    ui,
                                                    &default_resp,
                                                    default_id,
                                                    &column.default_value,
                                                );
                                                if default_resp.changed() {
                                                    tabular.create_table_error = None;
                                                }

                                                if ui
                                                    .checkbox(&mut column.is_primary_key, "")
                                                    .changed()
                                                {
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
                        });

                    if let Some(idx) = remove_idx {
                        state.columns.remove(idx);
                        tabular.create_table_error = None;
                    }

                    ui.add_space(10.0);
                    if ui
                        .add_sized(egui::vec2(160.0, 32.0), egui::Button::new("➕ Add Column"))
                        .clicked()
                    {
                        let new_col =
                            models::structs::TableColumnDefinition::blank(state.columns.len());
                        state.columns.push(new_col);
                        tabular.create_table_error = None;
                    }
                }
                models::structs::CreateTableWizardStep::Indexes => {
                    ui.label(
                        egui::RichText::new("Optimize lookups with optional indexes")
                            .strong()
                            .size(16.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Specify additional indexes to speed up reads.")
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(12.0);

                    let mut remove_idx: Option<usize> = None;
                    egui::Frame::group(ui.style())
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Vec2::new(12.0, 10.0))
                        .show(ui, |ui| {
                            if state.indexes.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new("No secondary indexes defined yet.")
                                            .color(ui.visuals().weak_text_color()),
                                    );
                                });
                            } else {
                                let row_width = ui.available_width();
                                let name_width = row_width * 0.35;
                                let cols_width = row_width * 0.5;

                                egui::Grid::new("create_table_indexes_grid")
                                    .striped(true)
                                    .num_columns(4)
                                    .spacing([16.0, 10.0])
                                    .min_row_height(28.0)
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new("Name").strong());
                                        ui.label(
                                            egui::RichText::new("Columns (comma-separated)")
                                                .strong(),
                                        );
                                        ui.label(egui::RichText::new("Unique").strong());
                                        ui.label(egui::RichText::new(" ").strong());
                                        ui.end_row();

                                        for (idx, index_def) in state.indexes.iter_mut().enumerate()
                                        {
                                            // Index name field
                                            let name_id = ui.id().with(("idx_name", idx));
                                            let name_resp = ui.add_sized(
                                                [name_width, 0.0],
                                                egui::TextEdit::singleline(&mut index_def.name)
                                                    .cursor_at_end(false)
                                                    .id(name_id),
                                            );
                                            if name_resp.clicked() || name_resp.gained_focus() {
                                                ui.memory_mut(|mem| mem.request_focus(name_id));
                                                ui.ctx().request_repaint();
                                            }
                                            paint_text_edit_cursor(
                                                ui,
                                                &name_resp,
                                                name_id,
                                                &index_def.name,
                                            );
                                            if name_resp.changed() {
                                                tabular.create_table_error = None;
                                            }

                                            // Index columns field
                                            let cols_id = ui.id().with(("idx_cols", idx));
                                            let cols_resp = ui.add_sized(
                                                [cols_width, 0.0],
                                                egui::TextEdit::singleline(&mut index_def.columns)
                                                    .cursor_at_end(false)
                                                    .id(cols_id),
                                            );
                                            if cols_resp.clicked() || cols_resp.gained_focus() {
                                                ui.memory_mut(|mem| mem.request_focus(cols_id));
                                                ui.ctx().request_repaint();
                                            }
                                            paint_text_edit_cursor(
                                                ui,
                                                &cols_resp,
                                                cols_id,
                                                &index_def.columns,
                                            );
                                            if cols_resp.changed() {
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
                        });

                    if let Some(idx) = remove_idx {
                        state.indexes.remove(idx);
                        tabular.create_table_error = None;
                    }

                    ui.add_space(10.0);
                    if ui
                        .add_sized(egui::vec2(160.0, 32.0), egui::Button::new("➕ Add Index"))
                        .clicked()
                    {
                        let new_index =
                            models::structs::TableIndexDefinition::blank(state.indexes.len());
                        state.indexes.push(new_index);
                        tabular.create_table_error = None;
                    }
                }
                models::structs::CreateTableWizardStep::Review => {
                    ui.label(egui::RichText::new("Final review").strong().size(16.0));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Confirm the generated SQL before creating the table.")
                            .color(ui.visuals().weak_text_color()),
                    );
                    ui.add_space(12.0);

                    egui::Frame::group(ui.style())
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Vec2::new(12.0, 10.0))
                        .show(ui, |ui| match preview_result.as_ref() {
                            Some(Ok(sql)) => {
                                let mut preview_text = sql.clone();
                                ui.add(
                                    egui::TextEdit::multiline(&mut preview_text)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_rows(14)
                                        .interactive(false),
                                );
                            }
                            Some(Err(err)) => {
                                ui.colored_label(egui::Color32::from_rgb(192, 57, 43), err);
                            }
                            None => {
                                ui.label(
                                    "SQL preview will appear after completing the previous steps.",
                                );
                            }
                        });
                }
            }

            if let Some(err) = tabular.create_table_error.as_ref() {
                ui.add_space(10.0);
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgb(255, 235, 238))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(214, 48, 49)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Vec2::new(10.0, 8.0))
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::from_rgb(192, 57, 43), err);
                    });
            }

            ui.add_space(12.0);
            egui::Frame::group(ui.style())
                .inner_margin(egui::Vec2::new(14.0, 12.0))
                .corner_radius(egui::CornerRadius::same(10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .add_sized(egui::vec2(110.0, 32.0), egui::Button::new("Cancel"))
                            .clicked()
                        {
                            action = WizardAction::Cancel;
                        }

                        if current_step.previous().is_some()
                            && ui
                                .add_sized(egui::vec2(110.0, 32.0), egui::Button::new("Back"))
                                .clicked()
                        {
                            action = WizardAction::Back;
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if current_step == models::structs::CreateTableWizardStep::Review {
                                let create_enabled = preview_result
                                    .as_ref()
                                    .map(|res| res.is_ok())
                                    .unwrap_or(false);
                                let create_button =
                                    egui::Button::new(egui::RichText::new("Create Table").strong())
                                        .min_size(egui::vec2(110.0, 32.0));
                                if ui.add_enabled(create_enabled, create_button).clicked() {
                                    action = WizardAction::Create;
                                }
                                if let Some(Ok(sql)) = preview_result.as_ref()
                                    && ui
                                        .add_sized(
                                            egui::vec2(110.0, 32.0),
                                            egui::Button::new("Copy SQL"),
                                        )
                                        .clicked()
                                {
                                    copy_preview = Some(sql.clone());
                                }
                            } else if ui
                                .add_sized(egui::vec2(110.0, 32.0), egui::Button::new("Next"))
                                .clicked()
                            {
                                action = WizardAction::Next;
                            }
                        });
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
            if let Some(state) = tabular.create_table_wizard.as_mut()
                && let Some(prev) = state.current_step.previous()
            {
                state.current_step = prev;
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
