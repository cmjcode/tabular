use eframe::egui;
use log::error;

use crate::{editor, models, rfd, window_egui};

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
        let cursor_pos = cursor_range.primary.index.0;

        // Calculate cursor X position from actual text width
        let text_before_cursor = if cursor_pos <= text.len() {
            &text[..cursor_pos]
        } else {
            text
        };

        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let galley = ui.fonts_mut(|f| {
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
        window_egui::style::render_modal_backdrop(ctx, "about_dialog", tabular.show_about_dialog);
        // Load logo texture if not already loaded
        load_logo_texture(tabular, ctx);

        let mut should_check_updates = false;
        let mut close = false;

        egui::Window::new("about_dialog_window")
            .id(egui::Id::new("about_dialog_window"))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(420.0)
            .frame(window_egui::style::modal_window_frame(ctx))
            .show(ctx, |ui| {
                window_egui::style::render_modal_header(
                    ui,
                    "About Tabular",
                    &mut close,
                );
                ui.add_space(12.0);

                window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.add_space(6.0);

                        // App icon/logo - use actual logo if loaded, fallback to emoji
                        if let Some(logo_texture) = &tabular.logo_texture {
                            ui.add(
                                egui::Image::from_texture(logo_texture)
                                    .max_size(egui::vec2(140.0, 140.0)),
                            );
                        } else {
                            ui.label(egui::RichText::new("📊").size(48.0));
                        }
                        ui.add_space(8.0);

                        // App name and version
                        ui.label(egui::RichText::new("Tabular").size(24.0).strong());
                        ui.label(
                            egui::RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                .size(15.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.label(
                            egui::RichText::new("Built with ❤️ using Rust")
                                .size(13.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(10.0);

                        // Description
                        ui.label(
                            egui::RichText::new(
                                "Your SQL Editor, Forged with Rust: Fast, Safe, Efficient.",
                            )
                            .size(13.0),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("Credit : Pamungkas Jayuda, Mualip Suhal, Davin Adesta Putra, Mohamad Ardiansah Pratama")
                                .size(11.0)
                                .weak(),
                        );
                        ui.add_space(10.0);

                        // Update check button
                        if ui.button("🔄 Check for Updates").clicked() {
                            should_check_updates = true;
                        }
                        ui.add_space(8.0);

                        ui.hyperlink_to(
                            "https://github.com/tabular-id/tabular",
                            "https://github.com/tabular-id/tabular",
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("© 2025 PT. Vneu Teknologi Indonesia")
                                .size(10.0)
                                .color(egui::Color32::GRAY),
                        );
                        ui.add_space(4.0);
                    });
                });
            });

        if close {
            tabular.show_about_dialog = false;
        }

        if should_check_updates {
            tabular.check_for_updates(true); // Manual check from About dialog
        }
    }
}

pub(crate) fn render_error_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.show_error_message {
        window_egui::style::render_modal_backdrop(ctx, "error_dialog", tabular.show_error_message);
        let mut close = false;
        egui::Window::new("error_dialog_window")
            .id(egui::Id::new("error_dialog_window"))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(400.0)
            .frame(window_egui::style::modal_window_frame(ctx))
            .show(ctx, |ui| {
                window_egui::style::render_modal_header(
                    ui,
                    egui::RichText::new("⚠️ Error").color(window_egui::style::theme_danger(ctx)),
                    &mut close,
                );
                ui.add_space(12.0);

                window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(&tabular.error_message);
                });
            });

        if close {
            tabular.show_error_message = false;
            tabular.error_message.clear();
        }
    }
}

pub(crate) fn render_save_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if tabular.show_save_dialog {
        window_egui::style::render_modal_backdrop(ctx, "save_dialog", tabular.show_save_dialog);
        let mut close = false;
        let mut save_clicked = false;

        egui::Window::new("save_dialog_window")
            .id(egui::Id::new("save_dialog_window"))
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(480.0)
            .frame(window_egui::style::modal_window_frame(ctx))
            .show(ctx, |ui| {
                window_egui::style::render_modal_header(ui, "💾 Save Query", &mut close);
                ui.add_space(12.0);

                window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());

                    // Current save directory display
                    ui.label(egui::RichText::new("Save Location").strong().size(12.5));
                    ui.add_space(2.0);
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

                    ui.add_space(12.0);

                    // Filename input
                    ui.label(egui::RichText::new("Enter Filename:").strong().size(12.5));
                    ui.add_space(2.0);
                    let filename_resp = crate::window_egui::style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut tabular.save_filename)
                            .hint_text("e.g. query.sql")
                            .cursor_at_end(false),
                        f32::INFINITY,
                        None,
                    );
                    if filename_resp.clicked() || filename_resp.gained_focus() {
                        filename_resp.request_focus();
                        ui.ctx().request_repaint();
                    }
                });

                ui.add_space(14.0);

                // Action button: Save only (X on top-right handles cancel)
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_save = !tabular.save_filename.trim().is_empty();
                    ui.add_enabled_ui(can_save, |ui| {
                        if ui
                            .add(window_egui::style::btn_primary_ctx(ui.ctx(), "💾 Save"))
                            .clicked()
                        {
                            save_clicked = true;
                        }
                    });
                });
            });

        if save_clicked {
            if let Err(err) =
                editor::save_current_tab_with_name(tabular, tabular.save_filename.clone())
            {
                error!("Failed to save: {}", err);
            }
            tabular.show_save_dialog = false;
            tabular.save_filename.clear();
            tabular.save_directory.clear();
        } else if close {
            tabular.show_save_dialog = false;
            tabular.save_filename.clear();
            tabular.save_directory.clear();
        }
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
    window_egui::style::render_modal_backdrop(ctx, "index_dialog", tabular.show_index_dialog);
    egui::Window::new("generate_index_window")
        .id(egui::Id::new("generate_index_window"))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(480.0)
        .frame(window_egui::style::modal_window_frame(ctx))
        .open(&mut open_flag)
        .show(ctx, |ui| {
            window_egui::style::render_modal_header(
                ui,
                "Generate Query Index",
                &mut should_close,
            );
            ui.add_space(12.0);

            window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                // Fields - aligned using a two-column Grid
                egui::Grid::new("index_form_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                    ui.label("Index name:");
                    let name_resp = crate::window_egui::style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut working.index_name).cursor_at_end(false),
                        320.0,
                        None,
                    );
                    if name_resp.clicked() || name_resp.gained_focus() {
                        name_resp.request_focus();
                        ui.ctx().request_repaint();
                    }
                    ui.end_row();

                    ui.label("Columns:");
                    let cols_resp = crate::window_egui::style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut working.columns).cursor_at_end(false),
                        320.0,
                        None,
                    );
                    if cols_resp.clicked() || cols_resp.gained_focus() {
                        cols_resp.request_focus();
                        ui.ctx().request_repaint();
                    }
                    ui.end_row();

                    ui.label("Method:");
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
                            ui.label(
                                egui::RichText::new("Field: 1 for asc, -1 for desc")
                                    .italics()
                                    .color(egui::Color32::GRAY),
                            );
                            working.method = None;
                        }
                        crate::models::enums::DatabaseType::ApiHttp
                        | crate::models::enums::DatabaseType::Plugin(_) => {
                            ui.label(egui::RichText::new("N/A").italics().color(egui::Color32::GRAY));
                            working.method = None;
                        }
                    }
                    ui.end_row();

                    ui.label("Unique:");
                    ui.checkbox(&mut working.unique, "");
                    ui.end_row();
                });
            });

            // Construct preview SQL
            let sql_preview = {
                if let Some(conn) = tabular
                    .connections
                    .iter()
                    .find(|c| c.id == Some(working.connection_id))
                {
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
                                db = db,
                                name = working.index_name,
                                table = working.table_name,
                                cols = working.columns,
                                clustered = clustered
                            )
                        }
                        (crate::models::structs::IndexDialogMode::Edit, DatabaseType::MySQL) => {
                            let method = working.method.clone().unwrap_or("BTREE".to_string());
                            let idx = working
                                .existing_index_name
                                .clone()
                                .unwrap_or(working.index_name.clone());
                            format!(
                                "ALTER TABLE `{table}` DROP INDEX `{idx}`,\nADD {unique} INDEX `{new}` ({cols}) USING {method};",
                                table = working.table_name,
                                idx = idx,
                                unique = if working.unique { "UNIQUE" } else { "" },
                                new = working.index_name,
                                cols = working.columns,
                                method = method
                            )
                        }
                        (crate::models::structs::IndexDialogMode::Edit, DatabaseType::PostgreSQL) => {
                            let schema = working.database_name.clone().unwrap_or_else(|| "public".to_string());
                            let idx = working
                                .existing_index_name
                                .clone()
                                .unwrap_or(working.index_name.clone());
                            format!(
                                "ALTER INDEX \"{schema}\".\"{idx}\" RENAME TO \"{new}\";",
                                schema = schema,
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
                                "DROP INDEX IF EXISTS \"{idx}\";\nCREATE {unique} INDEX IF NOT EXISTS \"{new}\" ON \"{table}\"({cols});",
                                idx = idx,
                                unique = if working.unique { "UNIQUE" } else { "" },
                                new = working.index_name,
                                table = working.table_name,
                                cols = working.columns,
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
                        (_, DatabaseType::Plugin(_)) => {
                            "-- Index management is not available for this engine".to_string()
                        }
                        (crate::models::structs::IndexDialogMode::Create, DatabaseType::MongoDB) => {
                            let db = working
                                .database_name
                                .clone()
                                .unwrap_or_else(|| conn.database.clone());
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
                        (crate::models::structs::IndexDialogMode::Create, DatabaseType::ApiHttp)
                        | (crate::models::structs::IndexDialogMode::Edit, DatabaseType::ApiHttp)
                        | (crate::models::structs::IndexDialogMode::Create, DatabaseType::Redis) => {
                            "-- Not applicable for this connection type".to_string()
                        }
                    }
                } else {
                    "-- No connection selected".to_string()
                }
            };

            ui.add_space(10.0);
            window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(egui::RichText::new("SQL Preview").strong().size(12.0));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    ui.code(sql_preview.clone());
                });
            });

            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let big_btn = window_egui::style::btn_primary_ctx(ui.ctx(), "Open in Editor")
                    .min_size(egui::vec2(140.0, 30.0));
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
    let mut close = false;

    window_egui::style::render_modal_backdrop(
        ctx,
        "create_table_wizard",
        tabular.show_create_table_dialog,
    );

    egui::Window::new("create_table_wizard_window")
        .id(egui::Id::new("create_table_wizard_window"))
        .collapsible(false)
        .resizable(true)
        .title_bar(false)
        .default_width(680.0)
        .min_width(640.0)
        .min_height(420.0)
        .frame(window_egui::style::modal_window_frame(ctx))
        .open(&mut keep_open)
        .show(ctx, |ui| {
            window_egui::style::render_modal_header(ui, "Create Table Wizard", &mut close);
            ui.add_space(12.0);

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
                        .fill(window_egui::style::theme_accent(ui.ctx())),
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
                                        | models::enums::DatabaseType::MongoDB
                                        | models::enums::DatabaseType::ApiHttp
                                        | models::enums::DatabaseType::Plugin(_) => "Database",
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
                                            let db_response =
                                                crate::window_egui::style::render_text_field(
                                                    ui,
                                                    egui::TextEdit::singleline(&mut target_text)
                                                        .cursor_at_end(false),
                                                    db_field_width,
                                                    None,
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
                                ui.colored_label(window_egui::style::theme_danger(ui.ctx()), err);
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
                window_egui::style::theme_alert_frame(ui.ctx(), true).show(ui, |ui| {
                    ui.colored_label(window_egui::style::theme_danger(ui.ctx()), err);
                });
            }

            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if current_step.previous().is_some()
                    && ui
                        .add_sized(
                            egui::vec2(100.0, 30.0),
                            crate::window_egui::style::btn_secondary("Back"),
                        )
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
                            crate::window_egui::style::btn_primary_ctx(ui.ctx(), "Create Table")
                                .min_size(egui::vec2(110.0, 30.0));
                        if ui.add_enabled(create_enabled, create_button).clicked() {
                            action = WizardAction::Create;
                        }
                        if let Some(Ok(sql)) = preview_result.as_ref()
                            && ui
                                .add_sized(
                                    egui::vec2(100.0, 30.0),
                                    crate::window_egui::style::btn_secondary("Copy SQL"),
                                )
                                .clicked()
                        {
                            copy_preview = Some(sql.clone());
                        }
                    } else if ui
                        .add_sized(
                            egui::vec2(100.0, 30.0),
                            crate::window_egui::style::btn_primary_ctx(ui.ctx(), "Next"),
                        )
                        .clicked()
                    {
                        action = WizardAction::Next;
                    }
                });
            });
        });

    if let Some(sql) = copy_preview {
        ctx.copy_text(sql);
    }

    if !keep_open || close {
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

// ── CSV Import Wizard ─────────────────────────────────────────────────────────

/// Opsi baca file sesuai pilihan wizard. `sniff` = biarkan pembaca menebak
/// delimiter (saat file baru dipilih).
fn import_read_options(
    state: &crate::models::structs::CsvImportState,
    sniff: bool,
    max_rows: Option<usize>,
) -> crate::data_transfer::readers::ReadOptions {
    crate::data_transfer::readers::ReadOptions {
        kind: None,
        delimiter: (!sniff).then_some(state.delimiter as u8),
        has_header: state.has_header_row,
        encoding: state.source.encoding,
        sheet: state.source.sheet.clone(),
        passphrase: (!state.source.passphrase.is_empty()).then(|| state.source.passphrase.clone()),
        max_rows,
        // Teks NULL pilihan pengguna diterapkan saat menyusun `INSERT`
        // (`csv_quote_value`), untuk semua format, bukan hanya CSV.
        null_text: None,
    }
}

type PreviewResult =
    Result<crate::data_transfer::readers::LoadedFile, crate::data_transfer::readers::ReadError>;

/// Pembacaan pratinjau yang sedang berjalan di thread kerja. File terkompresi,
/// spreadsheet, dan Parquet harus dibaca utuh walau hanya lima baris yang
/// ditampilkan, jadi pembacaannya tidak boleh menahan frame UI.
struct ImportPreviewJob {
    path: std::path::PathBuf,
    /// File baru dipilih (bukan sekadar ganti opsi baca file yang sama).
    new_file: bool,
    result: Option<PreviewResult>,
}

type ImportPreviewHandle = std::sync::Arc<std::sync::Mutex<ImportPreviewJob>>;

fn import_preview_id() -> egui::Id {
    egui::Id::new("csv_import_preview_job")
}

fn import_job_id() -> egui::Id {
    egui::Id::new("csv_import_job")
}

fn lock_or_recover<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Mulai membaca pratinjau `path` di thread kerja. Pratinjau lama yang belum
/// selesai ditinggalkan: hasilnya tidak lagi dirujuk dan dibuang.
fn start_import_preview(
    ctx: &egui::Context,
    state: &mut crate::models::structs::CsvImportState,
    path: std::path::PathBuf,
    sniff: bool,
    new_file: bool,
) {
    let opts = import_read_options(state, sniff, Some(5));
    let handle: ImportPreviewHandle =
        std::sync::Arc::new(std::sync::Mutex::new(ImportPreviewJob {
            path: path.clone(),
            new_file,
            result: None,
        }));
    let worker = handle.clone();
    let repaint = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("tabular-import-preview".to_string())
        .spawn(move || {
            let result = crate::data_transfer::readers::read_file(&path, &opts);
            lock_or_recover(&worker).result = Some(result);
            repaint.request_repaint();
        });
    match spawned {
        Ok(_) => {
            ctx.data_mut(|d| d.insert_temp(import_preview_id(), handle));
            state.status = crate::models::structs::CsvImportStatus::Idle;
            state.progress_message = "Reading file...".to_string();
        }
        Err(e) => {
            log::warn!("[IMPORT] could not start the preview thread: {e}");
            state.status = crate::models::structs::CsvImportStatus::Failed(e.to_string());
            state.progress_message = format!("Could not read file: {e}");
        }
    }
}

/// True bila pratinjau masih dibaca.
fn import_preview_pending(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<ImportPreviewHandle>(import_preview_id()))
        .is_some()
}

/// Ambil hasil pratinjau bila sudah selesai dan terapkan ke state wizard:
/// format, encoding, sheet, dan mapping kolom.
fn poll_import_preview(ctx: &egui::Context, state: &mut crate::models::structs::CsvImportState) {
    let Some(handle) = ctx.data(|d| d.get_temp::<ImportPreviewHandle>(import_preview_id())) else {
        return;
    };
    let (path, new_file, result) = {
        let mut job = lock_or_recover(&handle);
        let Some(result) = job.result.take() else {
            return;
        };
        (job.path.clone(), job.new_file, result)
    };
    ctx.data_mut(|d| d.remove::<ImportPreviewHandle>(import_preview_id()));
    match apply_import_preview(state, result) {
        Ok(()) => {
            if new_file {
                state.file_path = Some(path);
            }
            state.status = crate::models::structs::CsvImportStatus::Idle;
            state.progress_message = String::new();
        }
        Err(e) => {
            // File terenkripsi tetap dipilih supaya passphrase bisa diisi.
            if new_file && state.source.needs_passphrase {
                state.file_path = Some(path);
            }
            state.status = crate::models::structs::CsvImportStatus::Failed(e.clone());
            state.progress_message = format!("Parse error: {}", e);
        }
    }
}

/// Terapkan hasil baca pratinjau (CSV/TSV, JSON, NDJSON, spreadsheet,
/// Parquet; juga terkompresi atau terenkripsi) ke state wizard.
fn apply_import_preview(
    state: &mut crate::models::structs::CsvImportState,
    result: PreviewResult,
) -> Result<(), String> {
    use crate::data_transfer::readers::{FileKind, ReadError};
    match result {
        Ok(loaded) => {
            let src = &mut state.source;
            src.needs_passphrase = false;
            src.is_delimited = loaded.kind == FileKind::Delimited;
            src.is_text = loaded.kind.is_text();
            src.named_columns = matches!(
                loaded.kind,
                FileKind::Json | FileKind::Ndjson | FileKind::Parquet
            );
            src.detected_encoding = loaded.encoding;
            src.sheets = loaded.sheets;
            src.sheet = loaded.sheet;
            let mut label = loaded.kind.label().to_string();
            if let Some(compression) = loaded.compression {
                label.push_str(&format!(", {compression}"));
            }
            if loaded.encrypted {
                label.push_str(", encrypted");
            }
            src.kind_label = label;
            if let Some(delimiter) = loaded.delimiter {
                state.delimiter = delimiter as char;
            }
            let named = state.has_header_row || state.source.named_columns;
            let table_cols = state.table_columns.clone();
            state.column_mappings =
                build_auto_mappings(&loaded.data.headers, &loaded.data.rows, named, &table_cols);
            state.preview_headers = loaded.data.headers;
            // Sel NULL tampil sebagai penanda teks; nullness yang sebenarnya
            // dibaca ulang dari file saat impor.
            state.preview_rows = loaded.data.rows;
            Ok(())
        }
        Err(e) => {
            if matches!(e, ReadError::NeedsPassphrase | ReadError::Decrypt(_)) {
                state.source.needs_passphrase = true;
            }
            Err(e.to_string())
        }
    }
}

/// Literal SQL untuk satu sel impor. `None` adalah NULL dari file (JSON
/// `null`, sel spreadsheet kosong, kolom yang tidak ada di baris). Teks hanya
/// menjadi NULL bila sama dengan `null_value` pilihan pengguna; dengan
/// `null_value` kosong itu berarti sel kosong. String `NULL` tetap string
/// kecuali pengguna mengisi `NULL` di "NULL representation".
fn csv_quote_value(
    v: Option<&str>,
    null_value: &str,
    db_type: &crate::models::enums::DatabaseType,
) -> String {
    let Some(v) = v else {
        return "NULL".to_string();
    };
    if v == null_value {
        return "NULL".to_string();
    }
    match db_type {
        crate::models::enums::DatabaseType::MySQL => {
            format!("'{}'", v.replace('\\', "\\\\").replace('\'', "''"))
        }
        _ => format!("'{}'", v.replace('\'', "''")),
    }
}

fn csv_quote_ident(s: &str, db_type: &crate::models::enums::DatabaseType) -> String {
    match db_type {
        crate::models::enums::DatabaseType::MySQL => format!("`{}`", s.replace('`', "``")),
        crate::models::enums::DatabaseType::MsSQL => format!("[{}]", s.trim_matches(['[', ']'])),
        _ => format!("\"{}\"", s.replace('"', "\"\"")),
    }
}

/// Keadaan impor yang dibaca dialog tiap frame.
#[derive(Default)]
struct ImportProgress {
    /// File sudah terbaca; baris sedang dikirim.
    inserting: bool,
    rows_done: usize,
    rows_total: usize,
    /// Koneksi tujuan; dipakai jalur lama walau dialog sudah ditutup.
    connection_id: i64,
    outcome: Option<Result<ImportOutcome, String>>,
}

type ImportHandle = std::sync::Arc<std::sync::Mutex<ImportProgress>>;

enum ImportOutcome {
    /// Semua baris tersimpan dalam satu transaksi.
    Imported(usize),
    /// Engine tanpa jalur transaksi headless (plugin): statement dijalankan
    /// lewat antrean query biasa di thread UI, tidak atomik.
    Legacy {
        statements: Vec<String>,
        rows: usize,
    },
}

/// Penyusun `INSERT` multi-baris untuk impor, satu statement per panggilan
/// `next`. Baris yang sudah ditulis dilepas dari memori, jadi skrip lengkap
/// tidak pernah ada di memori bersamaan dengan seluruh isi file.
struct InsertChunks {
    cells: Vec<Vec<Option<String>>>,
    pos: usize,
    /// Indeks kolom file yang dipakai.
    active: Vec<usize>,
    head: String,
    null_value: String,
    db_type: crate::models::enums::DatabaseType,
    limits: crate::data_transfer::values::InsertLimits,
    progress: Option<(ImportHandle, egui::Context)>,
}

impl InsertChunks {
    /// `None` bila tidak ada kolom yang dipetakan.
    fn new(
        table_name: &str,
        database_name: Option<&str>,
        db_type: &crate::models::enums::DatabaseType,
        column_mappings: &[crate::models::structs::CsvColumnMapping],
        cells: Vec<Vec<Option<String>>>,
        null_value: &str,
    ) -> Option<Self> {
        let active: Vec<(usize, &str)> = column_mappings
            .iter()
            .enumerate()
            .filter(|(_, m)| m.target_column != "__skip__" && !m.target_column.is_empty())
            .map(|(i, m)| (i, m.target_column.as_str()))
            .collect();
        if active.is_empty() {
            return None;
        }
        let full_table = match (db_type, database_name) {
            (crate::models::enums::DatabaseType::MySQL, Some(db)) => {
                format!(
                    "{}.{}",
                    csv_quote_ident(db, db_type),
                    csv_quote_ident(table_name, db_type)
                )
            }
            (crate::models::enums::DatabaseType::PostgreSQL, Some(schema)) => {
                format!(
                    "{}.{}",
                    csv_quote_ident(schema, db_type),
                    csv_quote_ident(table_name, db_type)
                )
            }
            (crate::models::enums::DatabaseType::MsSQL, Some(db)) => {
                format!("[{}].dbo.{}", db, csv_quote_ident(table_name, db_type))
            }
            _ => csv_quote_ident(table_name, db_type),
        };
        let col_list: String = active
            .iter()
            .map(|(_, col)| csv_quote_ident(col, db_type))
            .collect::<Vec<_>>()
            .join(", ");
        Some(Self {
            cells,
            pos: 0,
            active: active.iter().map(|(i, _)| *i).collect(),
            head: format!("INSERT INTO {} ({}) VALUES\n", full_table, col_list),
            null_value: null_value.to_string(),
            db_type: db_type.clone(),
            // Batas terbesar yang diterima semua engine (SQL Server: 1000 baris).
            limits: crate::data_transfer::values::InsertLimits::default().for_engine(db_type),
            progress: None,
        })
    }

    fn with_progress(mut self, handle: ImportHandle, ctx: egui::Context) -> Self {
        self.progress = Some((handle, ctx));
        self
    }
}

impl Iterator for InsertChunks {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        if self.pos >= self.cells.len() {
            return None;
        }
        let mut sql = self.head.clone();
        let mut rows = 0usize;
        while self.pos < self.cells.len() {
            let over_rows = self.limits.max_rows > 0 && rows >= self.limits.max_rows;
            let over_bytes =
                self.limits.max_bytes > 0 && rows > 0 && sql.len() >= self.limits.max_bytes;
            if over_rows || over_bytes {
                break;
            }
            let row = std::mem::take(&mut self.cells[self.pos]);
            self.pos += 1;
            let vals: Vec<String> = self
                .active
                .iter()
                .map(|ci| {
                    csv_quote_value(
                        row.get(*ci).and_then(|c| c.as_deref()),
                        &self.null_value,
                        &self.db_type,
                    )
                })
                .collect();
            if rows > 0 {
                sql.push_str(",\n");
            }
            sql.push('(');
            sql.push_str(&vals.join(", "));
            sql.push(')');
            rows += 1;
        }
        sql.push(';');
        if let Some((handle, ctx)) = &self.progress {
            let mut progress = lock_or_recover(handle);
            progress.inserting = true;
            // Statement ini baru akan dikirim: yang selesai adalah sebelumnya.
            progress.rows_done = self.pos - rows;
            drop(progress);
            ctx.request_repaint();
        }
        Some(sql)
    }
}

/// Data impor yang dibawa ke task latar; tidak menyentuh state GUI.
struct ImportJob {
    path: std::path::PathBuf,
    read_opts: crate::data_transfer::readers::ReadOptions,
    table_name: String,
    database_name: Option<String>,
    db_type: crate::models::enums::DatabaseType,
    mappings: Vec<crate::models::structs::CsvColumnMapping>,
    null_value: String,
    endpoint: crate::data_transfer::catalog::Endpoint,
}

/// Baca seluruh file dan impor isinya. Pembacaan dan penyusunan SQL berjalan
/// di luar thread UI. Untuk engine SQL bawaan semua `INSERT` dijalankan
/// dalam satu transaksi: bila satu baris ditolak, tidak ada yang tersimpan.
async fn run_import(
    job: ImportJob,
    handle: ImportHandle,
    ctx: egui::Context,
) -> Result<ImportOutcome, String> {
    let ImportJob {
        path,
        read_opts,
        table_name,
        database_name,
        db_type,
        mappings,
        null_value,
        endpoint,
    } = job;
    let cells = tokio::task::spawn_blocking(move || {
        crate::data_transfer::readers::read_file(&path, &read_opts)
            .map(|loaded| loaded.data.into_cells())
            .map_err(|e| format!("Failed to read file: {e}"))
    })
    .await
    .map_err(|e| format!("Failed to read file: {e}"))??;
    let total = cells.len();
    lock_or_recover(&handle).rows_total = total;
    ctx.request_repaint();
    let chunks = InsertChunks::new(
        &table_name,
        database_name.as_deref(),
        &db_type,
        &mappings,
        cells,
        &null_value,
    )
    .filter(|_| total > 0)
    .ok_or_else(|| "No data or all columns skipped.".to_string())?;

    if !crate::data_transfer::catalog::supports_sql(&db_type) {
        return Ok(ImportOutcome::Legacy {
            statements: chunks.collect(),
            rows: total,
        });
    }
    let endpoint = endpoint.connect().await?;
    endpoint
        .execute_atomic(chunks.with_progress(handle, ctx))
        .await
        .map_err(|e| format!("{e}. The transaction was rolled back; no rows were imported."))?;
    Ok(ImportOutcome::Imported(total))
}

/// Jalur lama untuk engine tanpa transaksi headless: satu job query per
/// statement lewat antrean biasa. Statement yang gagal menghentikan sisanya,
/// tetapi yang sudah jalan tidak dibatalkan.
fn spawn_legacy_import_jobs(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    batches: Vec<String>,
    total_rows: usize,
) -> Result<String, String> {
    let batch_count = batches.len();
    let mut jobs = Vec::new();
    for (i, sql) in batches.into_iter().enumerate() {
        let job_id = tabular.jobs.allocate_id();
        let job = crate::connection::prepare_query_job(tabular, connection_id, sql, job_id)
            .map_err(|e| format!("Failed to prepare batch: {:?}", e))?;
        tabular.jobs.active.insert(
            job_id,
            crate::connection::QueryJobStatus {
                job_id,
                connection_id,
                query_preview: format!("CSV import batch {}/{}", i + 1, batch_count),
                started_at: std::time::Instant::now(),
                completed: false,
            },
        );
        jobs.push(job);
    }
    let sender = tabular.query_result_sender.clone();
    crate::connection::spawn_query_job_batch(tabular, jobs, sender)
        .map_err(|e| format!("Failed to start import: {:?}", e))?;
    Ok(format!(
        "Importing {} rows in {} batch(es) (not transactional on this engine)...",
        total_rows, batch_count
    ))
}

/// Kumpulkan data impor dari state wizard dan jalankan di runtime.
fn start_import(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    let Some(state) = tabular.csv_import_state.as_ref() else {
        return;
    };
    let Some(path) = state.file_path.clone() else {
        return;
    };
    let db_type = state.db_type.clone();
    // Nama tabel di `INSERT` sudah berkualifikasi. Database aktif tab dipakai
    // seperti jalur query biasa; untuk PostgreSQL `database_name` wizard
    // adalah schema, bukan database, jadi tidak dipakai di sini.
    let tab_database = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .filter(|d| !d.trim().is_empty());
    let database = match db_type {
        crate::models::enums::DatabaseType::PostgreSQL => tab_database,
        crate::models::enums::DatabaseType::SQLite => None,
        _ => tab_database.or_else(|| state.database_name.clone()),
    };
    let endpoint =
        tabular.transfer_endpoint(state.connection_id, database.as_deref().unwrap_or(""));
    let Some(endpoint) = endpoint else {
        let state = tabular.csv_import_state.as_mut().unwrap();
        state.status =
            crate::models::structs::CsvImportStatus::Failed("Connection not found".to_string());
        state.progress_message = "Connection not found".to_string();
        return;
    };
    let job = ImportJob {
        path,
        read_opts: import_read_options(state, false, None),
        table_name: state.table_name.clone(),
        database_name: state.database_name.clone(),
        db_type,
        mappings: state.column_mappings.clone(),
        null_value: state.null_value.clone(),
        endpoint,
    };
    let handle: ImportHandle = std::sync::Arc::new(std::sync::Mutex::new(ImportProgress {
        connection_id: state.connection_id,
        ..Default::default()
    }));
    ctx.data_mut(|d| d.insert_temp(import_job_id(), handle.clone()));
    let worker_ctx = ctx.clone();
    tabular.get_runtime().spawn(async move {
        let outcome = run_import(job, handle.clone(), worker_ctx.clone()).await;
        lock_or_recover(&handle).outcome = Some(outcome);
        worker_ctx.request_repaint();
    });
    let state = tabular.csv_import_state.as_mut().unwrap();
    state.status = crate::models::structs::CsvImportStatus::Importing;
    state.progress_message = "Reading file...".to_string();
}

/// Ambil kemajuan/hasil impor latar. Dipanggil tiap frame, juga saat dialog
/// sudah ditutup: impor tetap selesai dan hasilnya dilaporkan lewat toast.
fn poll_import_job(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    use crate::models::structs::CsvImportStatus;
    let Some(handle) = ctx.data(|d| d.get_temp::<ImportHandle>(import_job_id())) else {
        return;
    };
    let (connection_id, outcome, inserting, done, total) = {
        let mut progress = lock_or_recover(&handle);
        (
            progress.connection_id,
            progress.outcome.take(),
            progress.inserting,
            progress.rows_done,
            progress.rows_total,
        )
    };
    let Some(outcome) = outcome else {
        if let Some(state) = tabular.csv_import_state.as_mut() {
            state.status = CsvImportStatus::Importing;
            state.progress_message = if inserting {
                format!("Importing... {done} of {total} rows")
            } else {
                "Reading file...".to_string()
            };
        }
        return;
    };
    ctx.data_mut(|d| d.remove::<ImportHandle>(import_job_id()));
    let (status, message) = match outcome {
        Ok(ImportOutcome::Imported(rows)) => (
            CsvImportStatus::Done(rows),
            format!("Imported {rows} rows in one transaction."),
        ),
        Ok(ImportOutcome::Legacy { statements, rows }) => {
            match spawn_legacy_import_jobs(tabular, connection_id, statements, rows) {
                Ok(message) => (CsvImportStatus::Importing, message),
                Err(e) => (CsvImportStatus::Failed(e.clone()), e),
            }
        }
        Err(e) => (
            CsvImportStatus::Failed(e.clone()),
            format!("Import failed: {e}"),
        ),
    };
    match tabular.csv_import_state.as_mut() {
        Some(state) => {
            state.status = status;
            state.progress_message = message;
        }
        None => match status {
            CsvImportStatus::Failed(_) => {
                log::warn!("[IMPORT] {message}");
                tabular.toasts.error(message);
            }
            _ => tabular.toasts.success(message),
        },
    }
}

pub(crate) fn render_csv_import_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    // Impor latar dipantau walau dialog sudah ditutup.
    poll_import_job(tabular, ctx);
    if !tabular.show_csv_import_dialog {
        return;
    }
    let Some(state) = tabular.csv_import_state.as_mut() else {
        tabular.show_csv_import_dialog = false;
        return;
    };
    poll_import_preview(ctx, state);
    let preview_pending = import_preview_pending(ctx);

    window_egui::style::render_modal_backdrop(
        ctx,
        "csv_import_modal",
        tabular.show_csv_import_dialog,
    );

    let table_name = tabular
        .csv_import_state
        .as_ref()
        .unwrap()
        .table_name
        .clone();
    let title = format!("Import Data into \"{}\"", table_name);
    let mut open_flag = tabular.show_csv_import_dialog;
    let mut should_close = false;
    let mut trigger_file_pick = false;
    let mut trigger_import = false;
    let mut redelimit = false;
    let mut reset_file = false;
    let mut auto_match_all = false;
    let mut skip_all = false;

    egui::Window::new("csv_import_dialog_window")
        .id(egui::Id::new("csv_import_dialog_window"))
        .collapsible(false)
        .resizable(true)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(740.0)
        .min_width(580.0)
        .max_width(960.0)
        .default_height(560.0)
        .min_height(380.0)
        .max_height(780.0)
        .frame(window_egui::style::modal_window_frame(ctx))
        .open(&mut open_flag)
        .show(ctx, |ui| {
            window_egui::style::render_modal_header(ui, &title, &mut should_close);
            ui.add_space(12.0);

            let state = tabular.csv_import_state.as_mut().unwrap();
            let accent = window_egui::style::theme_accent(ctx);
            let muted = window_egui::style::theme_muted_text(ctx);
            let is_dark = ui.visuals().dark_mode;
            let card_bg = if is_dark {
                egui::Color32::from_rgb(26, 28, 38)
            } else {
                egui::Color32::from_rgb(246, 248, 252)
            };
            let border_color = if is_dark {
                egui::Color32::from_rgb(45, 49, 66)
            } else {
                egui::Color32::from_rgb(220, 226, 236)
            };

            // ── Header Description ───────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(egui_icons::icons::ICON_DOWNLOAD.codepoint).size(20.0));
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Import Data into Table: {}", table_name))
                            .strong()
                            .size(15.0),
                    );
                    ui.label(
                        egui::RichText::new("Choose a CSV, JSON, Excel, or Parquet file, configure parsing options, and review the column mapping.")
                            .small()
                            .color(muted),
                    );
                });
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── Scrollable Body Area ─────────────────────────────────────────
            egui::ScrollArea::vertical()
                .id_salt("csv_import_main_scroll")
                .max_height(ui.available_height() - 55.0)
                .show(ui, |ui| {
                    // ── STEP 1: File Selection ───────────────────────────────
                    if let Some(path) = &state.file_path {
                        // File Loaded Card
                        egui::Frame::group(ui.style())
                            .fill(card_bg)
                            .stroke(egui::Stroke::new(1.0, border_color))
                            .corner_radius(8.0)
                            .inner_margin(egui::Vec2::new(14.0, 12.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(egui_icons::icons::ICON_DESCRIPTION.codepoint).size(24.0));
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        let file_name = path
                                            .file_name()
                                            .unwrap_or_default()
                                            .to_string_lossy()
                                            .into_owned();
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(file_name).strong().size(14.0));
                                            if !state.preview_rows.is_empty() {
                                                let tag_bg = if is_dark {
                                                    egui::Color32::from_rgb(30, 50, 40)
                                                } else {
                                                    egui::Color32::from_rgb(225, 245, 230)
                                                };
                                                let tag_fg = window_egui::style::theme_success(ctx);
                                                egui::Frame::new()
                                                    .fill(tag_bg)
                                                    .corner_radius(4.0)
                                                    .inner_margin(egui::Vec2::new(6.0, 2.0))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            egui::RichText::new(format!("{} rows previewed", state.preview_rows.len()))
                                                                .color(tag_fg)
                                                                .small()
                                                                .strong(),
                                                        );
                                                    });
                                            }
                                        });
                                        ui.label(
                                            egui::RichText::new(path.to_string_lossy())
                                                .color(muted)
                                                .small(),
                                        );
                                    });

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button(format!("{} Remove", egui_icons::icons::ICON_CLOSE.codepoint)).on_hover_text("Deselect this file").clicked() {
                                            reset_file = true;
                                        }
                                        ui.add_space(4.0);
                                        if ui.button(format!("{} Change File...", egui_icons::icons::ICON_REFRESH.codepoint)).clicked() {
                                            trigger_file_pick = true;
                                        }
                                    });
                                });
                            });
                    } else {
                        // Empty Dropzone Area
                        egui::Frame::group(ui.style())
                            .fill(card_bg)
                            .stroke(egui::Stroke::new(1.2, border_color))
                            .corner_radius(8.0)
                            .inner_margin(egui::Vec2::new(20.0, 20.0))
                            .show(ui, |ui| {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new(egui_icons::icons::ICON_FOLDER.codepoint).size(32.0));
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new("Choose a File to Import")
                                            .strong()
                                            .size(15.0),
                                    );
                                    ui.add_space(2.0);
                                    ui.label(
                                        egui::RichText::new("CSV/TSV, JSON, NDJSON, Excel/ODS, Parquet; also .gz/.zip/.zst and encrypted .enc exports.")
                                            .color(muted)
                                            .small(),
                                    );
                                    ui.add_space(10.0);
                                    if ui
                                        .add(window_egui::style::btn_primary_ctx(ctx, format!("{}  Browse File...", egui_icons::icons::ICON_FOLDER_OPEN.codepoint)))
                                        .clicked()
                                    {
                                        trigger_file_pick = true;
                                    }
                                    ui.add_space(6.0);
                                });
                            });
                    }
                    ui.add_space(10.0);

                    // ── STEP 2: Parsing Options ──────────────────────────────
                    egui::Frame::group(ui.style())
                        .fill(card_bg)
                        .stroke(egui::Stroke::new(1.0, border_color))
                        .corner_radius(8.0)
                        .inner_margin(egui::Vec2::new(14.0, 12.0))
                        .show(ui, |ui| {
                            // Selebar kartu lain, berapa pun lebar isinya.
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("{} Parsing Options", egui_icons::icons::ICON_SETTINGS.codepoint)).strong());
                            });
                            ui.add_space(8.0);

                            if state.file_path.is_none() || state.source.is_delimited {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Delimiter:").color(muted));
                                ui.add_space(4.0);

                                for (ch, label, icon) in [
                                    (',', "Comma", ","),
                                    (';', "Semicolon", ";"),
                                    ('\t', "Tab", "\\t"),
                                    ('|', "Pipe", "|"),
                                ] {
                                    let is_selected = state.delimiter == ch;
                                    let pill_bg = if is_selected {
                                        if is_dark {
                                            egui::Color32::from_rgb(45, 55, 80)
                                        } else {
                                            egui::Color32::from_rgb(215, 230, 250)
                                        }
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                    let pill_stroke = if is_selected {
                                        egui::Stroke::new(
                                            1.2,
                                            if is_dark {
                                                egui::Color32::from_rgb(90, 130, 220)
                                            } else {
                                                egui::Color32::from_rgb(50, 100, 200)
                                            },
                                        )
                                    } else {
                                        egui::Stroke::new(1.0, border_color)
                                    };
                                    let pill_text_color = if is_selected {
                                        if is_dark {
                                            egui::Color32::WHITE
                                        } else {
                                            egui::Color32::from_rgb(20, 45, 100)
                                        }
                                    } else {
                                        ui.visuals().text_color()
                                    };

                                    let btn = egui::Button::new(
                                        egui::RichText::new(format!("{} {}", icon, label))
                                            .color(pill_text_color)
                                            .size(12.0),
                                    )
                                    .fill(pill_bg)
                                    .stroke(pill_stroke)
                                    .corner_radius(6.0);

                                    if ui.add(btn).clicked() && state.delimiter != ch {
                                        state.delimiter = ch;
                                        if state.file_path.is_some() {
                                            redelimit = true;
                                        }
                                    }
                                    ui.add_space(2.0);
                                }
                            });
                            }

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .checkbox(&mut state.has_header_row, "First row contains column names")
                                    .changed()
                                    && state.file_path.is_some()
                                {
                                    redelimit = true;
                                }
                                ui.add_space(16.0);
                                ui.label(egui::RichText::new("NULL representation:").color(muted))
                                    .on_hover_text(
                                        "Cells equal to this text are imported as SQL NULL. \
                                         Leave it empty to import empty cells as NULL; the text \
                                         NULL is then kept as a string. Type NULL here to import \
                                         it as SQL NULL instead.",
                                    );
                                crate::window_egui::style::render_text_field(
                                    ui,
                                    egui::TextEdit::singleline(&mut state.null_value)
                                        .hint_text("empty cells"),
                                    110.0,
                                    None,
                                );
                            });

                            // Encoding (format teks) dan sheet (spreadsheet).
                            let show_encoding = state.file_path.is_none() || state.source.is_text;
                            if show_encoding || state.source.sheets.len() > 1 {
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    if show_encoding {
                                        ui.label(egui::RichText::new("Encoding:").color(muted));
                                        let auto_label = match state.source.detected_encoding {
                                            Some(enc) if state.source.encoding.is_none() => {
                                                format!("Auto ({})", enc.label())
                                            }
                                            _ => "Auto-detect".to_string(),
                                        };
                                        let selected = state
                                            .source
                                            .encoding
                                            .map(|enc| enc.label().to_string())
                                            .unwrap_or_else(|| auto_label.clone());
                                        let before = state.source.encoding;
                                        egui::ComboBox::from_id_salt("csv_import_encoding_combo")
                                            .selected_text(selected)
                                            .width(150.0)
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    &mut state.source.encoding,
                                                    None,
                                                    "Auto-detect",
                                                );
                                                for enc in crate::data_transfer::encoding::TextEncoding::ALL {
                                                    ui.selectable_value(
                                                        &mut state.source.encoding,
                                                        Some(enc),
                                                        enc.label(),
                                                    );
                                                }
                                            });
                                        if state.source.encoding != before && state.file_path.is_some() {
                                            redelimit = true;
                                        }
                                        ui.add_space(16.0);
                                    }
                                    if state.source.sheets.len() > 1 {
                                        ui.label(egui::RichText::new("Sheet:").color(muted));
                                        let before = state.source.sheet.clone();
                                        egui::ComboBox::from_id_salt("csv_import_sheet_combo")
                                            .selected_text(before.clone().unwrap_or_default())
                                            .width(160.0)
                                            .show_ui(ui, |ui| {
                                                for sheet in state.source.sheets.clone() {
                                                    ui.selectable_value(
                                                        &mut state.source.sheet,
                                                        Some(sheet.clone()),
                                                        sheet,
                                                    );
                                                }
                                            });
                                        if state.source.sheet != before {
                                            redelimit = true;
                                        }
                                    }
                                });
                            }
                            if state.source.needs_passphrase {
                                ui.add_space(6.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("Passphrase:").color(muted));
                                    crate::window_egui::style::render_text_field(
                                        ui,
                                        egui::TextEdit::singleline(&mut state.source.passphrase)
                                            .password(true)
                                            .hint_text("Encrypted export passphrase"),
                                        220.0,
                                        None,
                                    );
                                    if ui.button("Unlock").clicked() {
                                        redelimit = true;
                                    }
                                });
                            }
                            if !state.source.kind_label.is_empty() {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(format!("Detected: {}", state.source.kind_label))
                                        .small()
                                        .color(muted),
                                );
                            }
                        });
                    ui.add_space(10.0);

                    // ── STEP 3: Column Mapping ───────────────────────────────
                    if !state.column_mappings.is_empty() {
                        egui::Frame::group(ui.style())
                            .fill(card_bg)
                            .stroke(egui::Stroke::new(1.0, border_color))
                            .corner_radius(8.0)
                            .inner_margin(egui::Vec2::new(14.0, 12.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(format!("{} Column Mapping", egui_icons::icons::ICON_SHUFFLE.codepoint)).strong());
                                    ui.add_space(6.0);
                                    let mapped_count = state
                                        .column_mappings
                                        .iter()
                                        .filter(|m| m.target_column != "__skip__")
                                        .count();
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "({}/{} columns mapped)",
                                            mapped_count,
                                            state.column_mappings.len()
                                        ))
                                        .small()
                                        .color(muted),
                                    );

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("Skip All").on_hover_text("Ignore all columns").clicked() {
                                            skip_all = true;
                                        }
                                        ui.add_space(4.0);
                                        if ui.button("Auto-Match").on_hover_text("Match columns by name").clicked() {
                                            auto_match_all = true;
                                        }
                                    });
                                });
                                ui.add_space(8.0);

                                let scroll_h = (state.column_mappings.len().min(8) as f32 * 32.0 + 10.0).max(80.0);
                                egui::ScrollArea::vertical()
                                    .id_salt("csv_mapping_scroll_area")
                                    .max_height(scroll_h)
                                    .show(ui, |ui| {
                                        egui::Grid::new("csv_mapping_grid_modern")
                                            .num_columns(4)
                                            .spacing([12.0, 6.0])
                                            .striped(true)
                                            .min_col_width(90.0)
                                            .show(ui, |ui| {
                                                ui.label(egui::RichText::new("Source Column").strong().small().color(muted));
                                                ui.label(egui::RichText::new("").small());
                                                ui.label(egui::RichText::new("Target Table Column").strong().small().color(muted));
                                                ui.label(egui::RichText::new("Sample Preview").strong().small().color(muted));
                                                ui.end_row();

                                                for (i, mapping) in state.column_mappings.iter_mut().enumerate() {
                                                    // Source header
                                                    ui.horizontal(|ui| {
                                                        egui::Frame::new()
                                                            .fill(if is_dark { egui::Color32::from_rgb(36, 40, 54) } else { egui::Color32::from_rgb(230, 235, 245) })
                                                            .corner_radius(4.0)
                                                            .inner_margin(egui::Vec2::new(6.0, 2.0))
                                                            .show(ui, |ui| {
                                                                ui.label(egui::RichText::new(&mapping.csv_header).monospace().small().strong());
                                                            });
                                                    });

                                                    ui.label(
                                                        egui::RichText::new(egui_icons::icons::ICON_ARROW_FORWARD.codepoint)
                                                            .color(muted)
                                                            .small(),
                                                    );

                                                    // Target column dropdown
                                                    let mut sel = mapping.target_column.clone();
                                                    let is_skipped = sel == "__skip__";
                                                    let display_text = if is_skipped {
                                                        "(skip column)".to_string()
                                                    } else {
                                                        sel.clone()
                                                    };
                                                    let text_col = if is_skipped {
                                                        muted
                                                    } else {
                                                        window_egui::style::theme_success(ctx)
                                                    };

                                                    egui::ComboBox::from_id_salt(egui::Id::new(("csv_map_target", i)))
                                                        .selected_text(egui::RichText::new(display_text).color(text_col))
                                                        .width(180.0)
                                                        .show_ui(ui, |ui| {
                                                            ui.selectable_value(
                                                                &mut sel,
                                                                "__skip__".to_string(),
                                                                egui::RichText::new("(skip column)").color(muted),
                                                            );
                                                            for col in &state.table_columns {
                                                                ui.selectable_value(&mut sel, col.clone(), col.as_str());
                                                            }
                                                        });
                                                    mapping.target_column = sel;

                                                    // Sample value
                                                    let preview_val = state
                                                        .preview_rows
                                                        .first()
                                                        .and_then(|r| r.get(i))
                                                        .map(String::as_str)
                                                        .unwrap_or("");
                                                    ui.label(
                                                        egui::RichText::new(if preview_val.is_empty() {
                                                            "NULL / empty"
                                                        } else {
                                                            preview_val
                                                        })
                                                        .small()
                                                        .italics()
                                                        .color(muted),
                                                    );
                                                    ui.end_row();
                                                }
                                            });
                                    });
                            });
                        ui.add_space(10.0);

                        // ── STEP 4: Live Data Preview ────────────────────────
                        egui::Frame::group(ui.style())
                            .fill(card_bg)
                            .stroke(egui::Stroke::new(1.0, border_color))
                            .corner_radius(8.0)
                            .inner_margin(egui::Vec2::new(14.0, 10.0))
                            .show(ui, |ui| {
                                ui.collapsing(
                                    egui::RichText::new(format!(
                                        "{} Data Preview (showing {} sample rows)",
                                        egui_icons::icons::ICON_VISIBILITY.codepoint,
                                        state.preview_rows.len()
                                    ))
                                        .strong()
                                        .small(),
                                    |ui| {
                                        ui.add_space(4.0);
                                        egui::ScrollArea::horizontal()
                                            .id_salt("csv_preview_table_hscroll")
                                            .max_height(140.0)
                                            .show(ui, |ui| {
                                                egui::Grid::new("csv_preview_table_grid")
                                                    .spacing([14.0, 4.0])
                                                    .striped(true)
                                                    .show(ui, |ui| {
                                                        ui.label(egui::RichText::new("#").strong().small().color(muted));
                                                        for h in &state.preview_headers {
                                                            ui.label(egui::RichText::new(h).strong().small().monospace());
                                                        }
                                                        if !state.preview_headers.is_empty() {
                                                            ui.end_row();
                                                        }
                                                        for (r_idx, row) in state.preview_rows.iter().enumerate() {
                                                            ui.label(egui::RichText::new(format!("{}", r_idx + 1)).small().color(muted));
                                                            for cell in row {
                                                                ui.label(egui::RichText::new(cell).small());
                                                            }
                                                            ui.end_row();
                                                        }
                                                    });
                                            });
                                    },
                                );
                            });
                        ui.add_space(6.0);
                    }
                });

            // ── Footer: Status Bar + Action Buttons ───────────────────────────
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                // Status message on the left
                if !state.progress_message.is_empty() {
                    let importing =
                        state.status == crate::models::structs::CsvImportStatus::Importing;
                    let (icon, color) = match &state.status {
                        crate::models::structs::CsvImportStatus::Failed(_) => (
                            Some(egui_icons::icons::ICON_ERROR),
                            window_egui::style::theme_danger(ui.ctx()),
                        ),
                        crate::models::structs::CsvImportStatus::Done(_) => (
                            Some(egui_icons::icons::ICON_CHECK_CIRCLE),
                            window_egui::style::theme_success(ui.ctx()),
                        ),
                        crate::models::structs::CsvImportStatus::Importing => (None, accent),
                        _ => (None, muted),
                    };
                    // File sedang dibaca atau baris sedang dikirim di latar.
                    if importing || preview_pending {
                        ui.spinner();
                    }
                    if let Some(icon) = icon {
                        ui.label(egui::RichText::new(icon.codepoint).small().color(color));
                    }
                    ui.label(
                        egui::RichText::new(&state.progress_message)
                            .small()
                            .strong()
                            .color(color),
                    );
                }

                // Buttons aligned to the right
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let has_valid_mapping = state
                        .column_mappings
                        .iter()
                        .any(|m| m.target_column != "__skip__");
                    let can_import = state.file_path.is_some()
                        && has_valid_mapping
                        && !preview_pending
                        && state.status != crate::models::structs::CsvImportStatus::Importing;

                    let import_text = if state.status == crate::models::structs::CsvImportStatus::Importing {
                        "Importing...".to_string()
                    } else if !state.preview_rows.is_empty() {
                        format!("Import Data ({})", state.preview_rows.len())
                    } else {
                        "Import Data".to_string()
                    };

                    let import_btn = window_egui::style::btn_primary_ctx(ctx, import_text);
                    if ui.add_enabled(can_import, import_btn).clicked() {
                        trigger_import = true;
                    }
                });
            });
            ui.add_space(2.0);
        });

    if reset_file {
        ctx.data_mut(|d| d.remove::<ImportPreviewHandle>(import_preview_id()));
        let state = tabular.csv_import_state.as_mut().unwrap();
        state.file_path = None;
        state.source = Default::default();
        state.preview_headers.clear();
        state.preview_rows.clear();
        state.column_mappings.clear();
        state.status = crate::models::structs::CsvImportStatus::Idle;
        state.progress_message.clear();
    }

    if auto_match_all {
        let state = tabular.csv_import_state.as_mut().unwrap();
        let table_cols = state.table_columns.clone();
        for mapping in &mut state.column_mappings {
            if let Some(matched) = table_cols
                .iter()
                .find(|c| c.eq_ignore_ascii_case(&mapping.csv_header))
            {
                mapping.target_column = matched.clone();
            }
        }
    }

    if skip_all {
        let state = tabular.csv_import_state.as_mut().unwrap();
        for mapping in &mut state.column_mappings {
            mapping.target_column = "__skip__".to_string();
        }
    }

    // ── File pick (outside closure) ────────────────────────────────────────
    if redelimit
        && let Some(state) = tabular.csv_import_state.as_mut()
        && let Some(path) = state.file_path.clone()
    {
        start_import_preview(ctx, state, path, false, false);
    }

    if trigger_file_pick
        && let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Data files (CSV, JSON, Excel, Parquet)",
                crate::data_transfer::readers::FILE_EXTENSIONS,
            )
            .add_filter("All files", &["*"])
            .pick_file()
        && let Some(state) = tabular.csv_import_state.as_mut()
    {
        // File baru: format, sheet, dan passphrase file lama tidak berlaku.
        state.source = Default::default();
        state.preview_headers.clear();
        state.preview_rows.clear();
        state.column_mappings.clear();
        start_import_preview(ctx, state, path, true, true);
    }

    if trigger_import {
        start_import(tabular, ctx);
    }

    if !open_flag || should_close {
        // Pratinjau yang masih dibaca tidak punya dialog untuk menerimanya.
        ctx.data_mut(|d| d.remove::<ImportPreviewHandle>(import_preview_id()));
    }
    if !open_flag || should_close {
        tabular.show_csv_import_dialog = false;
        tabular.csv_import_state = None;
    }
}

fn build_auto_mappings(
    headers: &[String],
    preview: &[Vec<String>],
    has_header_row: bool,
    table_cols: &[String],
) -> Vec<crate::models::structs::CsvColumnMapping> {
    if has_header_row {
        headers
            .iter()
            .map(|h| {
                let target = table_cols
                    .iter()
                    .find(|c| c.to_lowercase() == h.to_lowercase())
                    .cloned()
                    .unwrap_or_else(|| "__skip__".to_string());
                crate::models::structs::CsvColumnMapping {
                    csv_header: h.clone(),
                    target_column: target,
                }
            })
            .collect()
    } else {
        let ncols = preview.first().map(|r| r.len()).unwrap_or(0);
        (0..ncols)
            .map(|i| {
                let target = table_cols
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| "__skip__".to_string());
                crate::models::structs::CsvColumnMapping {
                    csv_header: format!("col_{}", i + 1),
                    target_column: target,
                }
            })
            .collect()
    }
}

pub(crate) fn render_parameter_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if !tabular.show_parameter_dialog {
        return;
    }
    window_egui::style::render_modal_backdrop(
        ctx,
        "parameter_dialog",
        tabular.show_parameter_dialog,
    );

    let mut execute_clicked = false;
    let mut close = false;

    egui::Window::new("parameter_bindings_window")
        .id(egui::Id::new("parameter_bindings_window"))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(480.0)
        .frame(window_egui::style::modal_window_frame(ctx))
        .show(ctx, |ui| {
            window_egui::style::render_modal_header(ui, "Parameter Bindings Required", &mut close);
            ui.add_space(12.0);

            window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    egui::RichText::new("Enter parameter values for this query:")
                        .size(12.0)
                        .weak(),
                );
                ui.add_space(8.0);

                egui::Grid::new("parameter_input_grid")
                    .num_columns(2)
                    .spacing([14.0, 10.0])
                    .show(ui, |ui| {
                        for (param_name, val) in &mut tabular.parameter_inputs {
                            ui.label(
                                egui::RichText::new(param_name.as_str())
                                    .monospace()
                                    .strong(),
                            );
                            crate::window_egui::style::render_text_field(
                                ui,
                                egui::TextEdit::singleline(val).hint_text("Enter value..."),
                                260.0,
                                None,
                            );
                            ui.end_row();
                        }
                    });
            });

            ui.add_space(14.0);

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(window_egui::style::btn_primary_ctx(
                            ui.ctx(),
                            "🚀 Run Query",
                        ))
                        .clicked()
                    {
                        execute_clicked = true;
                    }
                });
            });
        });

    if close {
        tabular.show_parameter_dialog = false;
    } else if execute_clicked {
        tabular.show_parameter_dialog = false;
        let query = tabular.parameter_dialog_query.clone();
        let substituted = editor::substitute_query_parameters(&query, &tabular.parameter_inputs);
        editor::execute_query_bypass_checks(tabular, substituted);
    }
}

pub(crate) fn render_unsafe_dml_dialog(tabular: &mut window_egui::Tabular, ctx: &egui::Context) {
    if !tabular.show_unsafe_dml_dialog {
        return;
    }
    window_egui::style::render_modal_backdrop(
        ctx,
        "unsafe_dml_dialog",
        tabular.show_unsafe_dml_dialog,
    );

    let mut confirm_clicked = false;
    let mut close = false;

    egui::Window::new("unsafe_dml_dialog_window")
        .id(egui::Id::new("unsafe_dml_dialog_window"))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(500.0)
        .frame(window_egui::style::modal_window_frame(ctx))
        .show(ctx, |ui| {
            window_egui::style::render_modal_header(
                ui,
                egui::RichText::new("⚠️ Unsafe Statement").color(window_egui::style::theme_danger(ctx)),
                &mut close,
            );
            ui.add_space(12.0);

            window_egui::style::modal_card_frame(ctx).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!(
                        "This {} statement has NO WHERE clause!",
                        tabular.unsafe_dml_type
                    ))
                    .color(window_egui::style::theme_danger(ctx))
                    .strong()
                    .size(14.0),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Running it will change or delete EVERY row in the target table. Are you sure you want to continue?",
                    )
                    .size(12.0)
                    .weak(),
                );
                ui.add_space(10.0);

                egui::Frame::new()
                    .fill(if ui.visuals().dark_mode {
                        egui::Color32::from_rgb(18, 20, 26)
                    } else {
                        egui::Color32::from_rgb(240, 242, 246)
                    })
                    .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&tabular.unsafe_dml_query)
                                .monospace()
                                .size(11.5),
                        );
                    });
            });

            ui.add_space(14.0);

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let run_btn = egui::Button::new(
                        egui::RichText::new("Yes, Run It")
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(window_egui::style::theme_danger(ctx));
                    if ui.add(run_btn).clicked() {
                        confirm_clicked = true;
                    }
                });
            });
        });

    if close {
        tabular.show_unsafe_dml_dialog = false;
    } else if confirm_clicked {
        tabular.show_unsafe_dml_dialog = false;
        let query = tabular.unsafe_dml_query.clone();
        editor::execute_query_bypass_checks(tabular, query);
    }
}

#[cfg(test)]
mod csv_import_tests {
    use super::*;
    use crate::data_transfer::catalog::Endpoint;
    use crate::models::enums::{DatabasePool, DatabaseType};
    use crate::models::structs::{ConnectionConfig, CsvColumnMapping};

    fn mapping(pairs: &[(&str, &str)]) -> Vec<CsvColumnMapping> {
        pairs
            .iter()
            .map(|(header, target)| CsvColumnMapping {
                csv_header: header.to_string(),
                target_column: target.to_string(),
            })
            .collect()
    }

    fn cells(rows: &[&[Option<&str>]]) -> Vec<Vec<Option<String>>> {
        rows.iter()
            .map(|r| r.iter().map(|c| c.map(str::to_string)).collect())
            .collect()
    }

    #[test]
    fn null_text_is_a_string_unless_the_user_chose_it() {
        let pg = DatabaseType::PostgreSQL;
        // Bawaan dialog (`null_value` kosong): sel kosong = NULL, teks `NULL` = string.
        assert_eq!(csv_quote_value(Some("NULL"), "", &pg), "'NULL'");
        assert_eq!(csv_quote_value(Some(""), "", &pg), "NULL");
        assert_eq!(csv_quote_value(None, "", &pg), "NULL");
        // Pilihan eksplisit: `NULL` jadi SQL NULL, sel kosong jadi string kosong.
        assert_eq!(csv_quote_value(Some("NULL"), "NULL", &pg), "NULL");
        assert_eq!(csv_quote_value(Some(""), "NULL", &pg), "''");
        assert_eq!(csv_quote_value(None, "NULL", &pg), "NULL");
        assert_eq!(
            csv_quote_value(Some("a\\b'c"), "", &DatabaseType::MySQL),
            "'a\\\\b''c'"
        );
    }

    #[test]
    fn insert_chunks_follow_mapping_and_row_limit() {
        let maps = mapping(&[("a", "id"), ("b", "__skip__"), ("c", "note")]);
        let data = cells(&[
            &[Some("1"), Some("x"), Some("NULL")],
            &[Some("2"), Some("y"), None],
            &[Some("3")],
        ]);
        let mut chunks =
            InsertChunks::new("t", Some("shop"), &DatabaseType::MySQL, &maps, data, "").unwrap();
        chunks.limits.max_rows = 2;
        let statements: Vec<String> = chunks.collect();
        assert_eq!(
            statements,
            vec![
                "INSERT INTO `shop`.`t` (`id`, `note`) VALUES\n('1', 'NULL'),\n('2', NULL);",
                "INSERT INTO `shop`.`t` (`id`, `note`) VALUES\n('3', NULL);",
            ]
        );
        // SQL Server dibatasi 1000 baris per `VALUES`; engine lain 500.
        let skip_all = mapping(&[("a", "__skip__")]);
        assert!(
            InsertChunks::new("t", None, &DatabaseType::MsSQL, &skip_all, vec![], "").is_none()
        );
        let ms = InsertChunks::new("t", None, &DatabaseType::MsSQL, &maps, vec![], "").unwrap();
        assert!(ms.limits.max_rows <= 1000);
        assert_eq!(ms.count(), 0);
    }

    async fn sqlite_endpoint() -> Endpoint {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        Endpoint::new(
            ConnectionConfig {
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(DatabasePool::SQLite(std::sync::Arc::new(pool))),
            None,
        )
    }

    fn job(path: &std::path::Path, endpoint: &Endpoint, null_value: &str) -> ImportJob {
        ImportJob {
            path: path.to_path_buf(),
            read_opts: Default::default(),
            table_name: "people".to_string(),
            database_name: None,
            db_type: DatabaseType::SQLite,
            mappings: mapping(&[("id", "id"), ("name", "name")]),
            null_value: null_value.to_string(),
            endpoint: endpoint.clone(),
        }
    }

    #[tokio::test]
    async fn import_is_atomic_and_keeps_the_text_null() {
        let dir = std::env::temp_dir().join(format!("tabular_import_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let endpoint = sqlite_endpoint().await;
        endpoint
            .query("CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL)")
            .await
            .unwrap();
        let ctx = egui::Context::default();

        // 1200 baris = tiga statement (500 + 500 + 200); baris 900 kosong dan
        // ditolak NOT NULL.
        let mut csv = String::from("id,name\n");
        for i in 1..=1200 {
            let name = match i {
                900 => String::new(),
                7 => "NULL".to_string(),
                _ => format!("n{i}"),
            };
            csv.push_str(&format!("{i},{name}\n"));
        }
        let bad = dir.join("bad.csv");
        std::fs::write(&bad, &csv).unwrap();
        let handle = ImportHandle::default();
        let err = run_import(job(&bad, &endpoint, ""), handle.clone(), ctx.clone())
            .await
            .err()
            .expect("NOT NULL violation fails the import");
        assert!(err.contains("rolled back"), "{err}");
        // Statement pertama (500 baris) sudah jalan sebelum yang kedua gagal,
        // tetapi tidak ada yang tertinggal.
        let count = endpoint.query("SELECT COUNT(*) FROM people").await.unwrap();
        assert_eq!(count.first_value(), Some("0"));
        assert_eq!(lock_or_recover(&handle).rows_total, 1200);

        let good = dir.join("good.csv");
        std::fs::write(&good, csv.replace("900,\n", "900,ok\n")).unwrap();
        let outcome = run_import(
            job(&good, &endpoint, ""),
            ImportHandle::default(),
            ctx.clone(),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, ImportOutcome::Imported(1200)));
        let count = endpoint.query("SELECT COUNT(*) FROM people").await.unwrap();
        assert_eq!(count.first_value(), Some("1200"));
        // Teks `NULL` di CSV tersimpan sebagai string.
        let name = endpoint
            .query("SELECT name, name IS NULL FROM people WHERE id = 7")
            .await
            .unwrap();
        assert_eq!(name.rows[0], vec!["NULL", "0"]);

        // Semua kolom dilewati atau file kosong: tidak ada yang dijalankan.
        let mut skipped = job(&good, &endpoint, "");
        skipped.mappings = mapping(&[("id", "__skip__"), ("name", "__skip__")]);
        let err = run_import(skipped, ImportHandle::default(), ctx)
            .await
            .err();
        assert_eq!(err.as_deref(), Some("No data or all columns skipped."));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
