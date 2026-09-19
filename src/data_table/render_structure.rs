use super::{infer_current_table_name, load_structure_info_for_current_table};
use crate::{models, window_egui};
use eframe::egui;

/// Jalankan statement pengubah struktur (ADD COLUMN, DROP COLUMN, CREATE INDEX,
/// …) di latar belakang agar UI tidak freeze saat menunggu database (misalnya
/// MySQL menunggu metadata lock). `on_success` dipanggil jika statement sukses;
/// jika gagal, `error_prefix` ditambahkan di depan pesan error database.
/// Jika pool belum siap, statement diantrekan sampai koneksi terbentuk.
fn run_structure_statement(
    tabular: &mut window_egui::Tabular,
    conn_id: i64,
    stmt: String,
    error_prefix: &str,
    on_success: impl FnOnce(&mut window_egui::Tabular) + 'static,
) {
    let error_prefix = error_prefix.to_string();
    tabular.run_query_with_callback(conn_id, stmt, move |tabular, message| {
        if message.success {
            on_success(tabular);
        } else {
            let err = message
                .error
                .clone()
                .unwrap_or_else(|| "Unknown error".to_string());
            tabular.toasts.error(format!("{}: {}", error_prefix, err));
        }
    });
}

pub(crate) fn data_types_for_current_conn(
    tabular: &window_egui::Tabular,
) -> &'static [&'static str] {
    let conn = tabular
        .current_connection_id
        .and_then(|id| tabular.connections.iter().find(|c| c.id == Some(id)));
    match conn.map(|c| c.connection_type.clone()) {
        Some(models::enums::DatabaseType::PostgreSQL) => &[
            "varchar(255)",
            "text",
            "integer",
            "bigint",
            "smallint",
            "boolean",
            "timestamp with time zone",
            "timestamp",
            "date",
            "time",
            "numeric(10,2)",
            "double precision",
            "real",
            "jsonb",
            "json",
            "uuid",
            "serial",
            "bigserial",
            "bytea",
        ],
        Some(models::enums::DatabaseType::SQLite) => {
            &["TEXT", "INTEGER", "REAL", "BLOB", "NUMERIC"]
        }
        Some(models::enums::DatabaseType::MsSQL) => &[
            "nvarchar(255)",
            "varchar(255)",
            "ntext",
            "int",
            "bigint",
            "smallint",
            "bit",
            "datetime2",
            "date",
            "time",
            "decimal(18,2)",
            "float",
            "uniqueidentifier",
            "varbinary(max)",
        ],
        _ => &[
            "varchar(255)",
            "text",
            "longtext",
            "int",
            "bigint",
            "smallint",
            "tinyint(1)",
            "boolean",
            "datetime",
            "date",
            "timestamp",
            "decimal(10,2)",
            "float",
            "double",
            "json",
            "enum('a','b')",
        ],
    }
}

pub(crate) fn default_data_type_for_conn(tabular: &window_egui::Tabular) -> String {
    data_types_for_current_conn(tabular)
        .first()
        .unwrap_or(&"varchar(255)")
        .to_string()
}

pub(crate) fn trigger_background_structure_refresh(tabular: &mut window_egui::Tabular) {
    let Some(conn_id) = tabular.current_connection_id else {
        return;
    };
    let active_db = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .unwrap_or_default();
    let conn = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned();
    let Some(conn) = conn else {
        return;
    };
    let table = infer_current_table_name(tabular);
    if table.trim().is_empty() {
        return;
    }
    let database = if !active_db.is_empty() {
        active_db
    } else {
        conn.database.clone()
    };

    tabular.is_refreshing_structure = true;

    if let Some(sender) = &tabular.background_sender {
        let _ = sender.send(models::enums::BackgroundTask::FetchTableStructure {
            connection_id: conn_id,
            database_name: database,
            table_name: table,
        });
    }
}

pub(crate) fn trigger_drop_column(tabular: &mut window_egui::Tabular, col_name: &str) {
    let table_name = infer_current_table_name(tabular);
    if let Some(conn_id) = tabular.current_connection_id
        && let Some(conn) = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()
    {
        let stmt = match conn.connection_type {
            models::enums::DatabaseType::MySQL => {
                format!("ALTER TABLE `{}` DROP COLUMN `{}`;", table_name, col_name)
            }
            models::enums::DatabaseType::PostgreSQL => {
                format!(
                    "ALTER TABLE \"{}\" DROP COLUMN \"{}\";",
                    table_name, col_name
                )
            }
            models::enums::DatabaseType::MsSQL => {
                format!("ALTER TABLE [{}] DROP COLUMN [{}];", table_name, col_name)
            }
            models::enums::DatabaseType::SQLite => {
                format!(
                    "-- SQLite drop column requires table rebuild; not supported automatically. Consider manual migration for '{}'.",
                    col_name
                )
            }
            _ => "-- Drop column not supported for this database type".to_string(),
        };
        tabular.pending_drop_column_name = Some(col_name.to_string());
        tabular.pending_drop_column_stmt = Some(stmt.clone());
        let insertion = format!("\n{}", stmt);
        let pos = tabular.editor.text.len();
        tabular.editor.apply_single_replace(pos..pos, &insertion);
        tabular.cursor_position = pos + insertion.len();
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor.text.clone();
            tab.is_modified = true;
        }
    }
}

pub(crate) fn trigger_drop_index(tabular: &mut window_egui::Tabular, idx_name: &str) {
    let table_name = infer_current_table_name(tabular);
    if let Some(conn_id) = tabular.current_connection_id
        && let Some(conn) = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()
    {
        let stmt = match conn.connection_type {
            models::enums::DatabaseType::MySQL => {
                format!("ALTER TABLE `{}` DROP INDEX `{}`;", table_name, idx_name)
            }
            models::enums::DatabaseType::PostgreSQL => {
                format!("DROP INDEX IF EXISTS \"{}\";", idx_name)
            }
            models::enums::DatabaseType::MsSQL => {
                format!("DROP INDEX [{}] ON [{}];", idx_name, table_name)
            }
            models::enums::DatabaseType::SQLite => {
                format!("DROP INDEX IF EXISTS `{}`;", idx_name)
            }
            _ => "-- Drop index not supported for this database type".to_string(),
        };
        tabular.pending_drop_index_name = Some(idx_name.to_string());
        tabular.pending_drop_index_stmt = Some(stmt.clone());
        let insertion = format!("\n{}", stmt);
        let pos = tabular.editor.text.len();
        tabular.editor.apply_single_replace(pos..pos, &insertion);
        tabular.cursor_position = pos + insertion.len();
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor.text.clone();
            tab.is_modified = true;
        }
    }
}

pub(crate) fn render_structure_view(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    let table_name = infer_current_table_name(tabular);
    let is_cols = tabular.structure_sub_view == models::structs::StructureSubView::Columns;
    let is_idx = tabular.structure_sub_view == models::structs::StructureSubView::Indexes;
    let metrics =
        crate::window_egui::device_profile::DeviceUiMetrics::compute(ui.ctx(), tabular.ui_mode);
    let tab_h = if metrics.is_touch { 38.0 } else { 28.0 };
    let tab_size = egui::vec2(if metrics.is_touch { 115.0 } else { 95.0 }, tab_h);

    // Top action bar - Balanced & Clean UI style
    ui.horizontal(|ui| {
        ui.spacing_mut().button_padding = if metrics.is_touch {
            egui::vec2(14.0, 8.0)
        } else {
            egui::vec2(10.0, 4.0)
        };
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 4.0);

        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "{} Structure: {}",
                egui_icons::icons::ICON_BUILD.codepoint,
                if table_name.is_empty() {
                    "-"
                } else {
                    &table_name
                }
            ))
            .strong()
            .size(if metrics.is_touch { 16.0 } else { 14.5 }),
        );
        ui.add_space(8.0);

        // Subview Tabs (Columns vs Indexes) - Styled identical to Data / Structure tabs
        let col_tab_title = format!("{} Columns", egui_icons::icons::ICON_VIEW_COLUMN.codepoint);
        if crate::window_egui::style::render_custom_tab(ui, &col_tab_title, is_cols, tab_size)
            .clicked()
        {
            tabular.structure_sub_view = models::structs::StructureSubView::Columns;
            tabular.structure_sel_anchor = None;
            tabular.structure_selected_cell = None;
            tabular.structure_selected_row = None;
        }

        let idx_tab_title = format!("{} Indexes", egui_icons::icons::ICON_TAG.codepoint);
        if crate::window_egui::style::render_custom_tab(ui, &idx_tab_title, is_idx, tab_size)
            .clicked()
        {
            tabular.structure_sub_view = models::structs::StructureSubView::Indexes;
            if tabular.structure_indexes.is_empty() {
                load_structure_info_for_current_table(tabular);
            }
            tabular.structure_sel_anchor = None;
            tabular.structure_selected_cell = None;
            tabular.structure_selected_row = None;
        }

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        // Refresh Button & Loading Indicator
        if tabular.is_refreshing_structure {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(15.0));
                ui.label(
                    egui::RichText::new("Refreshing...")
                        .size(13.5)
                        .italics()
                        .color(ui.visuals().text_color()),
                );
            });
        } else if ui
            .add(crate::window_egui::style::btn_secondary(format!(
                "{} Refresh",
                egui_icons::icons::ICON_REFRESH.codepoint
            )))
            .on_hover_text("Fetch latest structure in background")
            .clicked()
        {
            trigger_background_structure_refresh(tabular);
        }

        if is_cols {
            if ui
                .add(crate::window_egui::style::btn_primary_ctx(
                    ui.ctx(),
                    format!("{} Add Column", egui_icons::icons::ICON_ADD.codepoint),
                ))
                .on_hover_text("Add a new column")
                .clicked()
                && !tabular.adding_column
            {
                tabular.adding_column = true;
                if tabular.new_column_type.trim().is_empty() {
                    tabular.new_column_type = default_data_type_for_conn(tabular);
                }
                tabular.new_column_name.clear();
                tabular.new_column_default.clear();
                tabular.new_column_comment.clear();
                tabular.new_column_nullable = true;
            }

            let sel_col = tabular
                .structure_selected_row
                .and_then(|r| tabular.structure_columns.get(r))
                .cloned();
            let edit_enabled = sel_col.is_some() && !tabular.editing_column;
            if ui
                .add_enabled(
                    edit_enabled,
                    crate::window_egui::style::btn_secondary(format!(
                        "{} Edit Column",
                        egui_icons::icons::ICON_EDIT.codepoint
                    )),
                )
                .on_hover_text("Edit selected column")
                .clicked()
                && let Some(col) = &sel_col
            {
                tabular.editing_column = true;
                tabular.edit_column_original_name = col.name.clone();
                tabular.edit_column_name = col.name.clone();
                tabular.edit_column_type = col.data_type.clone();
                tabular.edit_column_nullable = col.nullable.unwrap_or(true);
                tabular.edit_column_default = col.default_value.clone().unwrap_or_default();
                tabular.edit_column_comment = col.comment.clone().unwrap_or_default();
            }

            let drop_enabled = sel_col.is_some();
            if ui
                .add_enabled(
                    drop_enabled,
                    crate::window_egui::style::btn_danger_ctx(
                        ui.ctx(),
                        format!("{} Drop Column", egui_icons::icons::ICON_DELETE.codepoint),
                    ),
                )
                .on_hover_text("Drop selected column")
                .clicked()
                && let Some(col) = &sel_col
            {
                trigger_drop_column(tabular, &col.name);
            }
        } else if is_idx {
            if ui
                .add(crate::window_egui::style::btn_primary_ctx(
                    ui.ctx(),
                    format!("{} Add Index", egui_icons::icons::ICON_ADD.codepoint),
                ))
                .on_hover_text("Create new index")
                .clicked()
                && !tabular.adding_index
            {
                start_inline_add_index(tabular);
            }

            let sel_idx = tabular
                .structure_selected_row
                .and_then(|r| tabular.structure_indexes.get(r))
                .cloned();
            let drop_enabled = sel_idx.is_some();
            if ui
                .add_enabled(
                    drop_enabled,
                    crate::window_egui::style::btn_danger_ctx(
                        ui.ctx(),
                        format!("{} Drop Index", egui_icons::icons::ICON_DELETE.codepoint),
                    ),
                )
                .on_hover_text("Drop selected index")
                .clicked()
                && let Some(idx) = &sel_idx
            {
                trigger_drop_index(tabular, &idx.name);
            }
        }
    });

    ui.separator();
    ui.add_space(2.0);
    match tabular.structure_sub_view {
        models::structs::StructureSubView::Columns => {
            render_structure_columns_editor(tabular, ui);
        }
        models::structs::StructureSubView::Indexes => {
            // Headers: No | index_name | algorithm | unique | columns | actions
            let headers = [
                "#",
                "index_name",
                "algorithm",
                "unique",
                "columns",
                "actions",
            ];
            if tabular.structure_idx_col_widths.len() != headers.len() {
                tabular.structure_idx_col_widths = vec![40.0, 200.0, 120.0, 70.0, 260.0, 120.0];
            }
            let mut widths = tabular.structure_idx_col_widths.clone();
            for w in widths.iter_mut() {
                *w = w.clamp(40.0, 800.0);
            }
            let dark = ui.visuals().dark_mode;
            let border = if dark {
                egui::Color32::from_rgb(55, 59, 74)
            } else {
                egui::Color32::from_rgb(203, 213, 225)
            };
            let stroke = egui::Stroke::new(0.5, border);
            let metrics = crate::window_egui::device_profile::DeviceUiMetrics::compute(
                ui.ctx(),
                tabular.ui_mode,
            );
            let row_h = metrics.table_row_height;
            let header_h = metrics.table_row_height + 4.0;
            egui::ScrollArea::both()
                                .id_salt("struct_idx_inline")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    // Header
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        for (i, h) in headers.iter().enumerate() {
                                            let w = widths[i];
                                            let (rect, resp) = ui.allocate_exact_size(
                                                egui::vec2(w, header_h),
                                                egui::Sense::click(),
                                            );
                                            let (h_bg, h_text_col) = crate::window_egui::style::table_header_colors(dark, i == 0);
                                            ui.painter().rect_filled(rect, 0.0, h_bg);
                                            ui.painter().rect_stroke(
                                                rect,
                                                0.0,
                                                stroke,
                                                egui::StrokeKind::Outside,
                                            );
                                            ui.painter().text(
                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                egui::Align2::LEFT_CENTER,
                                                *h,
                                                egui::FontId::proportional(13.0),
                                                h_text_col,
                                            );
                                            let handle = egui::Rect::from_min_max(
                                                egui::pos2(rect.max.x - 4.0, rect.min.y),
                                                rect.max,
                                            );
                                            let rh = ui.interact(
                                                handle,
                                                egui::Id::new(("struct_idx_inline", "resize", i)),
                                                egui::Sense::drag(),
                                            );
                                            if rh.dragged() {
                                                widths[i] =
                                                    (widths[i] + rh.drag_delta().x).clamp(40.0, 800.0);
                                                ui.ctx().request_repaint();
                                            }
                                            if rh.hovered() {
                                                ui.painter().rect_filled(
                                                    handle,
                                                    0.0,
                                                    egui::Color32::from_gray(80),
                                                );
                                            }
                                            resp.context_menu(|ui| {
                                                if ui.button(format!("{} Add Index", egui_icons::icons::ICON_ADD.codepoint)).clicked() {
                                                    if !tabular.adding_index {
                                                        start_inline_add_index(tabular);
                                                    }
                                                    ui.close();
                                                }
                                                if ui.button(format!("{} Refresh", egui_icons::icons::ICON_REFRESH.codepoint)).clicked() {
                                                    tabular.request_structure_refresh = true;
                                                    load_structure_info_for_current_table(tabular);
                                                    ui.close();
                                                }
                                            });
                                        }
                                    });
                                    ui.add_space(2.0);
                                    // Existing indexes rows
                                    let existing_indexes = tabular.structure_indexes.clone();
                                    for (idx, ix) in existing_indexes.iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            let is_pk = ix.name.eq_ignore_ascii_case("PRIMARY")
                                                || (ix.unique && ix.name.to_ascii_lowercase().contains("primary"));
                                            let values = [
                                                (idx + 1).to_string(),
                                                ix.name.clone(),
                                                ix.method.clone().unwrap_or_default(),
                                                if ix.unique {
                                                    "YES".to_string()
                                                } else {
                                                    "NO".to_string()
                                                },
                                                if ix.columns.is_empty() {
                                                    String::new()
                                                } else {
                                                    ix.columns.join(",")
                                                },
                                                String::new(), // actions placeholder
                                            ];
                                            // Defer selected cell border, and draw multi-selection overlay per cell
                                            let mut selected_cell_rect: Option<egui::Rect> = None;
                                            for (i, val) in values.iter().enumerate() {
                                                let w = widths[i];
                                                let (rect, resp) = ui.allocate_exact_size(
                                                    egui::vec2(w, row_h),
                                                    egui::Sense::click_and_drag(),
                                                );
                                                // Alternating row bg
                                                if idx % 2 == 1 {
                                                    let bg = if dark {
                                                        egui::Color32::from_rgb(26, 29, 38)
                                                    } else {
                                                        egui::Color32::from_rgb(245, 247, 250)
                                                    };
                                                    ui.painter().rect_filled(rect, 0.0, bg);
                                                }
                                                // Selection highlight (row / cell)
                                                let is_row_selected =
                                                    tabular.structure_selected_row == Some(idx);
                                                let is_cell_selected = tabular
                                                    .structure_selected_cell
                                                    == Some((idx, i));
                                                if let (Some(a), Some(b)) = (
                                                    tabular.structure_sel_anchor,
                                                    tabular.structure_selected_cell,
                                                ) {
                                                    let (ar, ac) = a;
                                                    let (br, bc) = b;
                                                    let rmin = ar.min(br);
                                                    let rmax = ar.max(br);
                                                    let cmin = ac.min(bc);
                                                    let cmax = ac.max(bc);
                                                    if idx >= rmin
                                                        && idx <= rmax
                                                        && i >= cmin
                                                        && i <= cmax
                                                    {
                                                        let sel = if dark {
                                                            egui::Color32::from_rgba_unmultiplied(
                                                                255, 80, 20, 28,
                                                            )
                                                        } else {
                                                            egui::Color32::from_rgba_unmultiplied(
                                                                255, 120, 40, 60,
                                                            )
                                                        };
                                                        ui.painter().rect_filled(rect, 0.0, sel);
                                                    }
                                                }
                                                if is_row_selected {
                                                    let sel = if dark {
                                                        egui::Color32::from_rgba_unmultiplied(
                                                            100, 150, 255, 30,
                                                        )
                                                    } else {
                                                        egui::Color32::from_rgba_unmultiplied(
                                                            200, 220, 255, 80,
                                                        )
                                                    };
                                                    ui.painter().rect_filled(rect, 0.0, sel);
                                                }
                                                // Base grid stroke first, so the selected outline can be drawn last
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    0.0,
                                                    stroke,
                                                    egui::StrokeKind::Outside,
                                                );
                                                if is_cell_selected {
                                                    selected_cell_rect = Some(rect);
                                                }
                                                match i {
                                                    0 => {
                                                        // # Row number
                                                        let row_num_col = crate::window_egui::style::table_row_number_color(dark);
                                                        ui.painter().text(
                                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                                            egui::Align2::LEFT_CENTER,
                                                            val,
                                                            egui::FontId::monospace(12.0),
                                                            row_num_col,
                                                        );
                                                    }
                                                    1 => {
                                                        // index_name
                                                        let idx_col = crate::window_egui::style::index_name_color(&ix.name, ix.unique, dark);
                                                        let icon_cp = if is_pk {
                                                            egui_icons::icons::ICON_KEY.codepoint
                                                        } else {
                                                            egui_icons::icons::ICON_TAG.codepoint
                                                        };
                                                        let mut job = egui::text::LayoutJob::default();
                                                        job.append(
                                                            &format!("{} ", icon_cp),
                                                            0.0,
                                                            egui::TextFormat {
                                                                color: idx_col,
                                                                font_id: egui::FontId::proportional(12.0),
                                                                ..Default::default()
                                                            },
                                                        );
                                                        job.append(
                                                            &ix.name,
                                                            0.0,
                                                            egui::TextFormat {
                                                                color: idx_col,
                                                                font_id: egui::FontId::proportional(13.0),
                                                                ..Default::default()
                                                            },
                                                        );
                                                        let galley = ui.painter().layout_job(job);
                                                        ui.painter().galley(
                                                            rect.left_center() + egui::vec2(6.0, -galley.size().y * 0.5),
                                                            galley,
                                                            egui::Color32::WHITE,
                                                        );
                                                    }
                                                    2 => {
                                                        // algorithm
                                                        let method = ix.method.as_deref().unwrap_or("");
                                                        if method.is_empty() {
                                                            let muted = if dark {
                                                                egui::Color32::from_rgb(100, 116, 139)
                                                            } else {
                                                                egui::Color32::from_rgb(148, 163, 184)
                                                            };
                                                            ui.painter().text(
                                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                "-",
                                                                egui::FontId::proportional(13.0),
                                                                muted,
                                                            );
                                                        } else {
                                                            let algo_col = crate::window_egui::style::index_algorithm_color(dark);
                                                            ui.painter().text(
                                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                method,
                                                                egui::FontId::monospace(12.5),
                                                                algo_col,
                                                            );
                                                        }
                                                    }
                                                    3 => {
                                                        // unique
                                                        if ix.unique {
                                                            let yes_col = if dark {
                                                                egui::Color32::from_rgb(52, 211, 153)
                                                            } else {
                                                                egui::Color32::from_rgb(5, 150, 105)
                                                            };
                                                            ui.painter().text(
                                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                "YES",
                                                                egui::FontId::proportional(13.0),
                                                                yes_col,
                                                            );
                                                        } else {
                                                            let no_col = if dark {
                                                                egui::Color32::from_rgb(148, 163, 184)
                                                            } else {
                                                                egui::Color32::from_rgb(100, 116, 139)
                                                            };
                                                            ui.painter().text(
                                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                "NO",
                                                                egui::FontId::proportional(13.0),
                                                                no_col,
                                                            );
                                                        }
                                                    }
                                                    4 => {
                                                        // columns
                                                        if ix.columns.is_empty() {
                                                            let muted = if dark {
                                                                egui::Color32::from_rgb(100, 116, 139)
                                                            } else {
                                                                egui::Color32::from_rgb(148, 163, 184)
                                                            };
                                                            ui.painter().text(
                                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                "-",
                                                                egui::FontId::proportional(13.0),
                                                                muted,
                                                            );
                                                        } else {
                                                            let mut job = egui::text::LayoutJob::default();
                                                            let col_color = crate::window_egui::style::column_name_color(dark, false);
                                                            let comma_color = if dark {
                                                                egui::Color32::from_rgb(100, 116, 139)
                                                            } else {
                                                                egui::Color32::from_rgb(148, 163, 184)
                                                            };
                                                            for (c_idx, col_part) in ix.columns.iter().enumerate() {
                                                                if c_idx > 0 {
                                                                    job.append(
                                                                        ", ",
                                                                        0.0,
                                                                        egui::TextFormat {
                                                                            color: comma_color,
                                                                            font_id: egui::FontId::proportional(13.0),
                                                                            ..Default::default()
                                                                        },
                                                                    );
                                                                }
                                                                job.append(
                                                                    col_part,
                                                                    0.0,
                                                                    egui::TextFormat {
                                                                        color: col_color,
                                                                        font_id: egui::FontId::proportional(13.0),
                                                                        ..Default::default()
                                                                    },
                                                                );
                                                            }
                                                            let galley = ui.painter().layout_job(job);
                                                            ui.painter().galley(
                                                                rect.left_center() + egui::vec2(6.0, -galley.size().y * 0.5),
                                                                galley,
                                                                egui::Color32::WHITE,
                                                            );
                                                        }
                                                    }
                                                    _ => {
                                                        if !val.is_empty() {
                                                            let txt_col = if dark {
                                                                egui::Color32::LIGHT_GRAY
                                                            } else {
                                                                egui::Color32::BLACK
                                                            };
                                                            ui.painter().text(
                                                                rect.left_center() + egui::vec2(6.0, 0.0),
                                                                egui::Align2::LEFT_CENTER,
                                                                val,
                                                                egui::FontId::proportional(13.0),
                                                                txt_col,
                                                            );
                                                        }
                                                    }
                                                }
                                                if resp.clicked() {
                                                    let shift = ui.input(|i| i.modifiers.shift);
                                                    tabular.structure_selected_row = Some(idx);
                                                    tabular.structure_selected_cell = Some((idx, i));
                                                    if !shift || tabular.structure_sel_anchor.is_none()
                                                    {
                                                        tabular.structure_sel_anchor = Some((idx, i));
                                                    }
                                                    // use same focus flag so global arrow handling prefers tables/structure over editor
                                                    tabular.table_recently_clicked = true;
                                                }
                                                if resp.drag_started() {
                                                    tabular.structure_dragging = true;
                                                    if tabular.structure_sel_anchor.is_none() {
                                                        tabular.structure_sel_anchor = Some((idx, i));
                                                    }
                                                    tabular.structure_selected_row = Some(idx);
                                                    tabular.structure_selected_cell = Some((idx, i));
                                                }
                                                if tabular.structure_dragging
                                                    && ui.input(|inp| inp.pointer.primary_down())
                                                    && resp.hovered()
                                                {
                                                    tabular.structure_selected_row = Some(idx);
                                                    tabular.structure_selected_cell = Some((idx, i));
                                                }
                                                if tabular.structure_dragging
                                                    && !ui.input(|inp| inp.pointer.primary_down())
                                                {
                                                    tabular.structure_dragging = false;
                                                }
                                                resp.context_menu(|ui| {
                                                    // Copy helpers
                                                    if ui.button(format!("{} Copy Cell Value", egui_icons::icons::ICON_CONTENT_COPY.codepoint)).clicked() {
                                                        ui.ctx().copy_text(val.clone());
                                                        ui.close();
                                                    }
                                                    if ui.button(format!("{} Copy Selection as CSV", egui_icons::icons::ICON_DESCRIPTION.codepoint)).clicked()
                                                    {
                                                        if let (Some(a), Some(b)) = (
                                                            tabular.structure_sel_anchor,
                                                            tabular.structure_selected_cell,
                                                        ) {
                                                            let (ar, ac) = a;
                                                            let (br, bc) = b;
                                                            let rmin = ar.min(br);
                                                            let rmax = ar.max(br);
                                                            let cmin = ac.min(bc);
                                                            let cmax = ac.max(bc);
                                                            let mut out = String::new();
                                                            for r in rmin..=rmax {
                                                                if let Some(row) =
                                                                    tabular.structure_indexes.get(r)
                                                                {
                                                                    let rowvals = [
                                                                        (r + 1).to_string(),
                                                                        row.name.clone(),
                                                                        row.method.clone().unwrap_or_default(),
                                                                        if row.unique {
                                                                            "YES".to_string()
                                                                        } else {
                                                                            "NO".to_string()
                                                                        },
                                                                        if row.columns.is_empty() {
                                                                            String::new()
                                                                        } else {
                                                                            row.columns.join(",")
                                                                        },
                                                                        String::new(),
                                                                    ];
                                                                    let mut fields: Vec<String> = Vec::new();
                                                                    for c in cmin..=cmax {
                                                                        let v = rowvals
                                                                            .get(c)
                                                                            .cloned()
                                                                            .unwrap_or_default();
                                                                        let q = if v.contains(',')
                                                                            || v.contains('"')
                                                                            || v.contains('\n')
                                                                        {
                                                                            format!(
                                                                                "\"{}\"",
                                                                                v.replace(
                                                                                    '"',
                                                                                    "\"\"",
                                                                                )
                                                                            )
                                                                        } else {
                                                                            v
                                                                        };
                                                                        fields.push(q);
                                                                    }
                                                                    out.push_str(&fields.join(","));
                                                                    out.push('\n');
                                                                }
                                                            }
                                                            if !out.is_empty() {
                                                                ui.ctx().copy_text(out);
                                                            }
                                                        }
                                                        ui.close();
                                                    }
                                                    if ui.button(format!("{} Copy Row as CSV", egui_icons::icons::ICON_DESCRIPTION.codepoint)).clicked() {
                                                        let csv_row = values
                                                            .iter()
                                                            .map(|v| {
                                                                if v.contains(',')
                                                                    || v.contains('"')
                                                                    || v.contains('\n')
                                                                {
                                                                    format!(
                                                                        "\"{}\"",
                                                                        v.replace(
                                                                            '"',
                                                                            "\"\"",
                                                                        ),
                                                                    )
                                                                } else {
                                                                    v.clone()
                                                                }
                                                            })
                                                            .collect::<Vec<_>>()
                                                            .join(",");
                                                        ui.ctx().copy_text(csv_row);
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    if ui.button(format!("{} Add Index", egui_icons::icons::ICON_ADD.codepoint)).clicked() {
                                                        if !tabular.adding_index {
                                                            start_inline_add_index(tabular);
                                                        }
                                                        ui.close();
                                                    }
                                                    if ui.button(format!("{} Refresh", egui_icons::icons::ICON_REFRESH.codepoint)).clicked() {
                                                        tabular.request_structure_refresh = true;
                                                        load_structure_info_for_current_table(tabular);
                                                        ui.close();
                                                    }
                                                    if ui.button(format!("{} Drop Index", egui_icons::icons::ICON_DELETE.codepoint)).clicked() {
                                                        if let Some(conn_id) =
                                                            tabular.current_connection_id
                                                            && let Some(conn) = tabular
                                                                .connections
                                                                .iter()
                                                                .find(|c| c.id == Some(conn_id))
                                                                .cloned()
                                                        {
                                                            let table_name =
                                                                infer_current_table_name(tabular);
                                                            let drop_stmt = match conn.connection_type
                                                            {
                                                                models::enums::DatabaseType::MySQL => {
                                                                    format!(
                                                                        "ALTER TABLE `{}` DROP INDEX `{}`;",
                                                                        table_name, ix.name
                                                                    )
                                                                }
                                                                models::enums::DatabaseType::MsSQL => {
                                                                    format!(
                                                                        "DROP INDEX [{}] ON [{}];",
                                                                        ix.name, table_name
                                                                    )
                                                                }
                                                                models::enums::DatabaseType::PostgreSQL => {
                                                                    format!(
                                                                        "DROP INDEX IF EXISTS \"{}\";",
                                                                        ix.name
                                                                    )
                                                                }
                                                                models::enums::DatabaseType::SQLite => {
                                                                    format!(
                                                                        "DROP INDEX IF EXISTS `{}`;",
                                                                        ix.name
                                                                    )
                                                                }
                                                                models::enums::DatabaseType::MongoDB => {
                                                                    format!(
                                                                        "-- MongoDB drop index '{}' (executed via driver)",
                                                                        ix.name
                                                                    )
                                                                }
                                                                _ => {
                                                                    "-- Drop index not supported for this database type"
                                                                        .to_string()
                                                                }
                                                            };
                                                            tabular.pending_drop_index_name =
                                                                Some(ix.name.clone());
                                                            tabular.pending_drop_index_stmt =
                                                                Some(drop_stmt.clone());
                                                            // Append the generated SQL to the editor via rope edit
                                                            let insertion =
                                                                format!("\n{}", drop_stmt);
                                                            let pos = tabular.editor.text.len();
                                                            tabular.editor.apply_single_replace(
                                                                pos..pos,
                                                                &insertion,
                                                            );
                                                            tabular.cursor_position =
                                                                pos + insertion.len();
                                                            if let Some(tab) = tabular
                                                                .query_tabs
                                                                .get_mut(tabular.active_tab_index)
                                                            {
                                                                tab.content =
                                                                    tabular.editor.text.clone();
                                                                tab.is_modified = true;
                                                            }
                                                        }
                                                        ui.close();
                                                    }
                                                });
                                            }
                                            // Paint selected cell border last to ensure right edge stays visible
                                            if let Some(rect) = selected_cell_rect {
                                                let stroke =
                                                    egui::Stroke::new(
                                                        2.0,
                                                        egui::Color32::from_rgb(255, 0, 0),
                                                    );
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    0.0,
                                                    stroke,
                                                    egui::StrokeKind::Outside,
                                                );
                                            }
                                        });
                                    }
                                    // Inline add new index row (editable fields like add column)
                                    if tabular.adding_index {
                                        let edit_row_bg = if dark {
                                            egui::Color32::from_rgb(32, 36, 46)
                                        } else {
                                            egui::Color32::from_rgb(238, 244, 255)
                                        };
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;

                                            for (i, &w) in widths.iter().enumerate().take(6) {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    egui::vec2(w, row_h),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(rect, 0.0, edit_row_bg);
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    0.0,
                                                    stroke,
                                                    egui::StrokeKind::Outside,
                                                );

                                                let mut child_ui = ui.new_child(
                                                    egui::UiBuilder::new()
                                                        .max_rect(rect)
                                                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                                );
                                                child_ui.spacing_mut().item_spacing.x = 2.0;
                                                // Compact widget padding so ComboBox/Button controls
                                                // fit inside the fixed row height instead of the
                                                // app-wide toolbar-sized padding (12,7).
                                                child_ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                                                // Safety net: never let a control paint outside its cell or outside visible area.
                                                child_ui.set_clip_rect(rect.intersect(ui.clip_rect()));

                                                match i {
                                                    0 => {
                                                        let idx_txt = format!("{}", tabular.structure_indexes.len() + 1);
                                                        let txt_col = if dark {
                                                            egui::Color32::LIGHT_GRAY
                                                        } else {
                                                            egui::Color32::BLACK
                                                        };
                                                        child_ui.add_space(6.0);
                                                        child_ui.label(
                                                            egui::RichText::new(idx_txt)
                                                                .size(13.0)
                                                                .color(txt_col),
                                                        );
                                                    }
                                                    1 => {
                                                        child_ui.add_space(3.0);
                                                        if tabular.new_index_name.is_empty() {
                                                            tabular.new_index_name = format!(
                                                                "idx_{}_col",
                                                                infer_current_table_name(tabular)
                                                            );
                                                        }
                                                        let text_w = (w - 8.0).max(20.0);
                                                        child_ui.add(
                                                            egui::TextEdit::singleline(&mut tabular.new_index_name)
                                                                .desired_width(text_w),
                                                        );
                                                    }
                                                    2 => {
                                                        child_ui.add_space(3.0);
                                                        let algos = ["", "btree", "hash", "gin", "gist"];
                                                        let combo_w = (w
                                                            - 8.0
                                                            - 2.0 * child_ui.spacing().button_padding.x)
                                                            .max(20.0);
                                                        egui::ComboBox::from_id_salt("new_index_algo")
                                                            .selected_text(
                                                                if tabular.new_index_method.is_empty() {
                                                                    "(auto)"
                                                                } else {
                                                                    &tabular.new_index_method
                                                                },
                                                            )
                                                            .width(combo_w)
                                                            .show_ui(&mut child_ui, |ui| {
                                                                for a in algos {
                                                                    if ui
                                                                        .selectable_label(
                                                                            tabular.new_index_method == a,
                                                                            if a.is_empty() {
                                                                                "(auto)"
                                                                            } else {
                                                                                a
                                                                            },
                                                                        )
                                                                        .clicked()
                                                                    {
                                                                        tabular.new_index_method = a.to_string();
                                                                    }
                                                                }
                                                            });
                                                    }
                                                    3 => {
                                                        child_ui.add_space(3.0);
                                                        let combo_w = (w
                                                            - 8.0
                                                            - 2.0 * child_ui.spacing().button_padding.x)
                                                            .max(20.0);
                                                        egui::ComboBox::from_id_salt("new_index_unique")
                                                            .selected_text(if tabular.new_index_unique {
                                                                "YES"
                                                            } else {
                                                                "NO"
                                                            })
                                                            .width(combo_w)
                                                            .show_ui(&mut child_ui, |ui| {
                                                                if ui
                                                                    .selectable_label(
                                                                        tabular.new_index_unique,
                                                                        "YES",
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    tabular.new_index_unique = true;
                                                                }
                                                                if ui
                                                                    .selectable_label(
                                                                        !tabular.new_index_unique,
                                                                        "NO",
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    tabular.new_index_unique = false;
                                                                }
                                                            });
                                                    }
                                                    4 => {
                                                        child_ui.add_space(3.0);
                                                        if tabular.new_index_columns.is_empty() {
                                                            tabular.new_index_columns = "col1".to_string();
                                                        }
                                                        let text_w = (w - 8.0).max(20.0);
                                                        child_ui.add(
                                                            egui::TextEdit::singleline(&mut tabular.new_index_columns)
                                                                .desired_width(text_w),
                                                        );
                                                    }
                                                    5 => {
                                                        child_ui.add_space(3.0);
                                                        child_ui.spacing_mut().item_spacing.x = 4.0;
                                                        child_ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                                                        let save_enabled =
                                                            !tabular.new_index_name.trim().is_empty()
                                                                && !tabular.new_index_columns.trim().is_empty();
                                                        if child_ui
                                                            .add_enabled(
                                                                save_enabled,
                                                                crate::window_egui::style::btn_primary_ctx(
                                                                    child_ui.ctx(),
                                                                    "Save",
                                                                ),
                                                            )
                                                            .clicked()
                                                        {
                                                            commit_new_index(tabular);
                                                        }
                                                        if child_ui
                                                            .add(crate::window_egui::style::btn_secondary("Cancel"))
                                                            .clicked()
                                                        {
                                                            tabular.adding_index = false;
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        });
                                    }
                                });
            tabular.structure_idx_col_widths = widths;
        }
    }
}

pub(crate) fn render_structure_columns_editor(
    tabular: &mut window_egui::Tabular,
    ui: &mut egui::Ui,
) {
    let headers = [
        "#",
        "column_name",
        "data_type",
        "nullable",
        "default",
        "extra",
        "description",
        "actions",
    ];
    if tabular.structure_col_widths.len() != headers.len() {
        tabular.structure_col_widths = vec![40.0, 180.0, 160.0, 90.0, 140.0, 110.0, 200.0, 130.0];
    }
    let mut widths = tabular.structure_col_widths.clone();
    for w in widths.iter_mut() {
        *w = w.clamp(40.0, 600.0);
    }
    let dark = ui.visuals().dark_mode;
    let border = if dark {
        egui::Color32::from_rgb(55, 59, 74)
    } else {
        egui::Color32::from_rgb(203, 213, 225)
    };
    let stroke = egui::Stroke::new(0.5, border);
    let metrics =
        crate::window_egui::device_profile::DeviceUiMetrics::compute(ui.ctx(), tabular.ui_mode);
    let row_h = metrics.table_row_height;
    let header_h = metrics.table_row_height + 4.0;

    egui::ScrollArea::both()
        .id_salt("struct_cols_inline")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // HEADER
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (i, h) in headers.iter().enumerate() {
                    let w = widths[i];
                    // Make header cells clickable so we can attach context menu (right-click)
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(w, header_h), egui::Sense::click());
                    let (h_bg, h_text_col) =
                        crate::window_egui::style::table_header_colors(dark, i == 0);
                    ui.painter().rect_filled(rect, 0.0, h_bg);
                    ui.painter()
                        .rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
                    ui.painter().text(
                        rect.left_center() + egui::vec2(6.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        *h,
                        egui::FontId::proportional(13.0),
                        h_text_col,
                    );
                    // simple resize region
                    let handle = egui::Rect::from_min_max(
                        egui::pos2(rect.max.x - 4.0, rect.min.y),
                        rect.max,
                    );
                    let rh = ui.interact(
                        handle,
                        egui::Id::new(("struct_cols_inline", "resize", i)),
                        egui::Sense::drag(),
                    );
                    if rh.dragged() {
                        widths[i] = (widths[i] + rh.drag_delta().x).clamp(40.0, 600.0);
                        ui.ctx().request_repaint();
                    }
                    if rh.hovered() {
                        ui.painter()
                            .rect_filled(handle, 0.0, egui::Color32::from_gray(80));
                    }
                    // Context menu on any header cell
                    resp.context_menu(|ui| {
                        if ui
                            .button(format!(
                                "{} Refresh",
                                egui_icons::icons::ICON_REFRESH.codepoint
                            ))
                            .clicked()
                        {
                            tabular.request_structure_refresh = true;
                            load_structure_info_for_current_table(tabular);
                            ui.close();
                        }
                        if ui
                            .button(format!(
                                "{} Add Column",
                                egui_icons::icons::ICON_ADD.codepoint
                            ))
                            .clicked()
                        {
                            if !tabular.adding_column {
                                // initialize add column row
                                tabular.adding_column = true;
                                if tabular.new_column_type.trim().is_empty() {
                                    tabular.new_column_type = "varchar(255)".to_string();
                                }
                                tabular.new_column_name.clear();
                                tabular.new_column_default.clear();
                                tabular.new_column_comment.clear();
                                tabular.new_column_nullable = true;
                            }
                            ui.close();
                        }
                    });
                }
            });
            ui.add_space(2.0);

            // EXISTING ROWS (clone to avoid simultaneous mutable borrow when using context menu actions)
            let existing_cols = tabular.structure_columns.clone();
            if tabular.editing_column
                && !existing_cols
                    .iter()
                    .any(|c| c.name == tabular.edit_column_original_name)
            {
                tabular.editing_column = false;
                tabular.edit_column_comment.clear();
            }

            for (idx, col) in existing_cols.iter().enumerate() {
                let is_editing_this_row =
                    tabular.editing_column && tabular.edit_column_original_name == col.name;

                if is_editing_this_row {
                    let edit_row_bg = if dark {
                        egui::Color32::from_rgb(32, 36, 46)
                    } else {
                        egui::Color32::from_rgb(238, 244, 255)
                    };
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;

                        for (i, &w) in widths.iter().enumerate().take(8) {
                            let (rect, _) =
                                ui.allocate_exact_size(egui::vec2(w, row_h), egui::Sense::hover());
                            ui.painter().rect_filled(rect, 0.0, edit_row_bg);
                            ui.painter()
                                .rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);

                            let mut child_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            );
                            child_ui.spacing_mut().item_spacing.x = 2.0;
                            child_ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                            child_ui.set_clip_rect(rect.intersect(ui.clip_rect()));

                            match i {
                                0 => {
                                    let txt_col = if dark {
                                        egui::Color32::LIGHT_GRAY
                                    } else {
                                        egui::Color32::BLACK
                                    };
                                    child_ui.add_space(6.0);
                                    child_ui.label(
                                        egui::RichText::new(egui_icons::icons::ICON_EDIT.codepoint)
                                            .size(12.0)
                                            .color(txt_col),
                                    );
                                }
                                1 => {
                                    child_ui.add_space(3.0);
                                    let text_w = (w - 8.0).max(20.0);
                                    child_ui.add(
                                        egui::TextEdit::singleline(&mut tabular.edit_column_name)
                                            .desired_width(text_w),
                                    );
                                }
                                2 => {
                                    child_ui.add_space(3.0);
                                    let picker_w = 30.0;
                                    let text_w = (w - picker_w - 8.0).max(20.0);

                                    child_ui.add(
                                        egui::TextEdit::singleline(&mut tabular.edit_column_type)
                                            .desired_width(text_w),
                                    );

                                    let types = data_types_for_current_conn(tabular);
                                    egui::ComboBox::from_id_salt("edit_col_type_picker")
                                        .selected_text("")
                                        .width(14.0)
                                        .show_ui(&mut child_ui, |ui| {
                                            for t in types {
                                                if ui
                                                    .selectable_label(
                                                        tabular.edit_column_type == *t,
                                                        *t,
                                                    )
                                                    .clicked()
                                                {
                                                    tabular.edit_column_type = t.to_string();
                                                }
                                            }
                                        });
                                }
                                3 => {
                                    child_ui.add_space(3.0);
                                    let combo_w =
                                        (w - 8.0 - 2.0 * child_ui.spacing().button_padding.x)
                                            .max(20.0);
                                    egui::ComboBox::from_id_salt("edit_col_nullable")
                                        .selected_text(if tabular.edit_column_nullable {
                                            "YES"
                                        } else {
                                            "NO"
                                        })
                                        .width(combo_w)
                                        .show_ui(&mut child_ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    tabular.edit_column_nullable,
                                                    "YES",
                                                )
                                                .clicked()
                                            {
                                                tabular.edit_column_nullable = true;
                                            }
                                            if ui
                                                .selectable_label(
                                                    !tabular.edit_column_nullable,
                                                    "NO",
                                                )
                                                .clicked()
                                            {
                                                tabular.edit_column_nullable = false;
                                            }
                                        });
                                }
                                4 => {
                                    child_ui.add_space(3.0);
                                    let text_w = (w - 8.0).max(20.0);
                                    child_ui.add(
                                        egui::TextEdit::singleline(
                                            &mut tabular.edit_column_default,
                                        )
                                        .desired_width(text_w)
                                        .hint_text("NULL"),
                                    );
                                }
                                5 => {
                                    child_ui.add_space(6.0);
                                    let muted = if dark {
                                        egui::Color32::from_rgb(100, 116, 139)
                                    } else {
                                        egui::Color32::from_rgb(148, 163, 184)
                                    };
                                    let extra_txt = col.extra.as_deref().unwrap_or("-");
                                    child_ui.label(
                                        egui::RichText::new(extra_txt).size(13.0).color(muted),
                                    );
                                }
                                6 => {
                                    child_ui.add_space(3.0);
                                    let text_w = (w - 8.0).max(20.0);
                                    child_ui.add(
                                        egui::TextEdit::singleline(
                                            &mut tabular.edit_column_comment,
                                        )
                                        .desired_width(text_w)
                                        .hint_text("Description"),
                                    );
                                }
                                7 => {
                                    child_ui.add_space(3.0);
                                    child_ui.spacing_mut().item_spacing.x = 4.0;
                                    child_ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                                    let save_enabled = !tabular.edit_column_name.trim().is_empty()
                                        && !tabular.edit_column_type.trim().is_empty();
                                    if child_ui
                                        .add_enabled(
                                            save_enabled,
                                            crate::window_egui::style::btn_primary_ctx(
                                                child_ui.ctx(),
                                                "Save",
                                            ),
                                        )
                                        .clicked()
                                    {
                                        commit_edit_column(tabular);
                                    }
                                    if child_ui
                                        .add(crate::window_egui::style::btn_secondary("Cancel"))
                                        .clicked()
                                    {
                                        tabular.editing_column = false;
                                        tabular.edit_column_comment.clear();
                                    }
                                }
                                _ => {}
                            }
                        }
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let is_pk = tabular.structure_indexes.iter().any(|ix| {
                            (ix.name.eq_ignore_ascii_case("PRIMARY")
                                || (ix.unique && ix.name.to_ascii_lowercase().contains("primary")))
                                && ix.columns.iter().any(|c| c.eq_ignore_ascii_case(&col.name))
                        });
                        let values = [
                            (idx + 1).to_string(),
                            col.name.clone(),
                            col.data_type.clone(),
                            col.nullable
                                .map(|b| if b { "YES" } else { "NO" })
                                .unwrap_or("?")
                                .to_string(),
                            col.default_value.clone().unwrap_or_default(),
                            col.extra.clone().unwrap_or_default(),
                            col.comment.clone().unwrap_or_default(),
                            String::new(), // index 7: actions
                        ];
                        // Defer selected cell border so it paints last for this row
                        let mut selected_cell_rect: Option<egui::Rect> = None;
                        for (i, val) in values.iter().enumerate() {
                            let w = widths[i];
                            let (rect, resp) = if i == 7 {
                                ui.allocate_exact_size(egui::vec2(w, row_h), egui::Sense::hover())
                            } else {
                                ui.allocate_exact_size(
                                    egui::vec2(w, row_h),
                                    egui::Sense::click_and_drag(),
                                )
                            };
                            if idx % 2 == 1 {
                                let bg = if dark {
                                    egui::Color32::from_rgb(26, 29, 38)
                                } else {
                                    egui::Color32::from_rgb(245, 247, 250)
                                };
                                ui.painter().rect_filled(rect, 0.0, bg);
                            }
                            // Selection highlight
                            let is_row_selected = tabular.structure_selected_row == Some(idx);
                            let is_cell_selected =
                                tabular.structure_selected_cell == Some((idx, i));
                            // Multi-selection block highlight (Structure)
                            if let (Some(a), Some(b)) = (
                                tabular.structure_sel_anchor,
                                tabular.structure_selected_cell,
                            ) {
                                let (ar, ac) = a;
                                let (br, bc) = b;
                                let rmin = ar.min(br);
                                let rmax = ar.max(br);
                                let cmin = ac.min(bc);
                                let cmax = ac.max(bc);
                                if idx >= rmin && idx <= rmax && i >= cmin && i <= cmax {
                                    let sel = if dark {
                                        egui::Color32::from_rgba_unmultiplied(255, 80, 20, 28)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(255, 120, 40, 60)
                                    };
                                    ui.painter().rect_filled(rect, 0.0, sel);
                                }
                            }
                            if is_row_selected {
                                let sel = if dark {
                                    egui::Color32::from_rgba_unmultiplied(100, 150, 255, 30)
                                } else {
                                    egui::Color32::from_rgba_unmultiplied(200, 220, 255, 80)
                                };
                                ui.painter().rect_filled(rect, 0.0, sel);
                            }
                            // Base grid stroke first
                            ui.painter()
                                .rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
                            // Defer selected outline to avoid being overdrawn by neighbor cells
                            if is_cell_selected {
                                selected_cell_rect = Some(rect);
                            }
                            match i {
                                0 => {
                                    // # Row number
                                    let row_num_col =
                                        crate::window_egui::style::table_row_number_color(dark);
                                    ui.painter().text(
                                        rect.left_center() + egui::vec2(6.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        val,
                                        egui::FontId::monospace(12.0),
                                        row_num_col,
                                    );
                                }
                                1 => {
                                    // column_name
                                    let col_color =
                                        crate::window_egui::style::column_name_color(dark, is_pk);
                                    let icon_cp = if is_pk {
                                        egui_icons::icons::ICON_KEY.codepoint
                                    } else {
                                        egui_icons::icons::ICON_VIEW_COLUMN.codepoint
                                    };
                                    let mut job = egui::text::LayoutJob::default();
                                    job.append(
                                        &format!("{} ", icon_cp),
                                        0.0,
                                        egui::TextFormat {
                                            color: col_color,
                                            font_id: egui::FontId::proportional(12.0),
                                            ..Default::default()
                                        },
                                    );
                                    job.append(
                                        &col.name,
                                        0.0,
                                        egui::TextFormat {
                                            color: col_color,
                                            font_id: egui::FontId::proportional(13.0),
                                            ..Default::default()
                                        },
                                    );
                                    let galley = ui.painter().layout_job(job);
                                    ui.painter().galley(
                                        rect.left_center()
                                            + egui::vec2(6.0, -galley.size().y * 0.5),
                                        galley,
                                        egui::Color32::WHITE,
                                    );
                                }
                                2 => {
                                    // data_type
                                    let type_color = crate::window_egui::style::sql_type_color(
                                        &col.data_type,
                                        dark,
                                    );
                                    ui.painter().text(
                                        rect.left_center() + egui::vec2(6.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        val,
                                        egui::FontId::monospace(12.5),
                                        type_color,
                                    );
                                }
                                3 => {
                                    // nullable
                                    let null_col =
                                        crate::window_egui::style::nullable_badge_color(val, dark);
                                    let display_val = if val == "?" { "-" } else { val.as_str() };
                                    ui.painter().text(
                                        rect.left_center() + egui::vec2(6.0, 0.0),
                                        egui::Align2::LEFT_CENTER,
                                        display_val,
                                        egui::FontId::proportional(13.0),
                                        null_col,
                                    );
                                }
                                4 => {
                                    // default_value
                                    if val.is_empty() || val.eq_ignore_ascii_case("NULL") {
                                        let muted = if dark {
                                            egui::Color32::from_rgb(100, 116, 139)
                                        } else {
                                            egui::Color32::from_rgb(148, 163, 184)
                                        };
                                        ui.painter().text(
                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            "NULL",
                                            egui::FontId::proportional(12.0),
                                            muted,
                                        );
                                    } else {
                                        let (def_col, _) =
                                            crate::window_egui::style::table_cell_style(
                                                val,
                                                Some(&col.data_type),
                                                dark,
                                            );
                                        ui.painter().text(
                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            val,
                                            egui::FontId::proportional(13.0),
                                            def_col,
                                        );
                                    }
                                }
                                5 => {
                                    // extra
                                    if val.is_empty() {
                                        let muted = if dark {
                                            egui::Color32::from_rgb(100, 116, 139)
                                        } else {
                                            egui::Color32::from_rgb(148, 163, 184)
                                        };
                                        ui.painter().text(
                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            "-",
                                            egui::FontId::proportional(13.0),
                                            muted,
                                        );
                                    } else {
                                        let extra_col =
                                            crate::window_egui::style::extra_info_color(val, dark);
                                        ui.painter().text(
                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            val,
                                            egui::FontId::proportional(13.0),
                                            extra_col,
                                        );
                                    }
                                }
                                6 => {
                                    // description
                                    if val.is_empty() {
                                        let muted = if dark {
                                            egui::Color32::from_rgb(100, 116, 139)
                                        } else {
                                            egui::Color32::from_rgb(148, 163, 184)
                                        };
                                        ui.painter().text(
                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            "-",
                                            egui::FontId::proportional(13.0),
                                            muted,
                                        );
                                    } else {
                                        let desc_col =
                                            crate::window_egui::style::column_description_color(
                                                dark,
                                            );
                                        ui.painter().text(
                                            rect.left_center() + egui::vec2(6.0, 0.0),
                                            egui::Align2::LEFT_CENTER,
                                            val,
                                            egui::FontId::proportional(12.5),
                                            desc_col,
                                        );
                                    }
                                }
                                7 => {
                                    // actions: Quick Edit & Drop buttons (hanya render jika berada di dalam area tampak)
                                    let visible_rect = rect.intersect(ui.clip_rect());
                                    if visible_rect.is_positive() {
                                        let mut child_ui = ui.new_child(
                                            egui::UiBuilder::new().max_rect(rect).layout(
                                                egui::Layout::left_to_right(egui::Align::Center),
                                            ),
                                        );
                                        child_ui.spacing_mut().item_spacing.x = 4.0;
                                        child_ui.spacing_mut().button_padding =
                                            egui::vec2(5.0, 1.5);
                                        child_ui.set_clip_rect(visible_rect);
                                        child_ui.add_space(4.0);

                                        let edit_btn = child_ui.add(
                                            egui::Button::new(
                                                egui::RichText::new(format!(
                                                    "{} Edit",
                                                    egui_icons::icons::ICON_EDIT.codepoint
                                                ))
                                                .size(11.5),
                                            )
                                            .corner_radius(4.0),
                                        );
                                        if edit_btn.on_hover_text("Edit this column").clicked() {
                                            tabular.editing_column = true;
                                            tabular.edit_column_original_name = col.name.clone();
                                            tabular.edit_column_name = col.name.clone();
                                            tabular.edit_column_type = col.data_type.clone();
                                            tabular.edit_column_nullable =
                                                col.nullable.unwrap_or(true);
                                            tabular.edit_column_default =
                                                col.default_value.clone().unwrap_or_default();
                                            tabular.edit_column_comment =
                                                col.comment.clone().unwrap_or_default();
                                        }

                                        let del_btn = child_ui.add(
                                            egui::Button::new(
                                                egui::RichText::new(format!(
                                                    "{} Drop",
                                                    egui_icons::icons::ICON_DELETE.codepoint
                                                ))
                                                .size(11.5)
                                                .color(crate::window_egui::style::theme_danger(
                                                    child_ui.ctx(),
                                                )),
                                            )
                                            .corner_radius(4.0),
                                        );
                                        if del_btn.on_hover_text("Drop this column").clicked() {
                                            trigger_drop_column(tabular, &col.name);
                                        }
                                    }
                                }
                                _ => {}
                            }
                            if i < 7 {
                                if resp.clicked() {
                                    let shift = ui.input(|i| i.modifiers.shift);
                                    tabular.structure_selected_row = Some(idx);
                                    tabular.structure_selected_cell = Some((idx, i));
                                    if !shift || tabular.structure_sel_anchor.is_none() {
                                        tabular.structure_sel_anchor = Some((idx, i));
                                    }
                                    tabular.table_recently_clicked = true;
                                }
                                if resp.double_clicked() {
                                    tabular.editing_column = true;
                                    tabular.edit_column_original_name = col.name.clone();
                                    tabular.edit_column_name = col.name.clone();
                                    tabular.edit_column_type = col.data_type.clone();
                                    tabular.edit_column_nullable = col.nullable.unwrap_or(true);
                                    tabular.edit_column_default =
                                        col.default_value.clone().unwrap_or_default();
                                    tabular.edit_column_comment =
                                        col.comment.clone().unwrap_or_default();
                                }
                                // Drag-to-select: when user drags over cells, extend the selection to current cell
                                if resp.drag_started() {
                                    tabular.structure_dragging = true;
                                    if tabular.structure_sel_anchor.is_none() {
                                        tabular.structure_sel_anchor = Some((idx, i));
                                    }
                                    tabular.structure_selected_row = Some(idx);
                                    tabular.structure_selected_cell = Some((idx, i));
                                }
                                // While dragging, update selection when hovering over any cell
                                if tabular.structure_dragging
                                    && ui.input(|inp| inp.pointer.primary_down())
                                    && resp.hovered()
                                {
                                    tabular.structure_selected_row = Some(idx);
                                    tabular.structure_selected_cell = Some((idx, i));
                                }
                                // End drag when primary is released anywhere
                                if tabular.structure_dragging
                                    && !ui.input(|inp| inp.pointer.primary_down())
                                {
                                    tabular.structure_dragging = false;
                                }
                                // Context menu on every cell
                                resp.context_menu(|ui| {
                                    if ui
                                        .button(format!(
                                            "{} Copy Cell Value",
                                            egui_icons::icons::ICON_CONTENT_COPY.codepoint
                                        ))
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(val.clone());
                                        ui.close();
                                    }
                                    if ui
                                        .button(format!(
                                            "{} Copy Selection as CSV",
                                            egui_icons::icons::ICON_DESCRIPTION.codepoint
                                        ))
                                        .clicked()
                                    {
                                        if let (Some(a), Some(b)) = (
                                            tabular.structure_sel_anchor,
                                            tabular.structure_selected_cell,
                                        ) {
                                            let (ar, ac) = a;
                                            let (br, bc) = b;
                                            let rmin = ar.min(br);
                                            let rmax = ar.max(br);
                                            let cmin = ac.min(bc);
                                            let cmax = ac.max(bc);
                                            let mut out = String::new();
                                            for r in rmin..=rmax {
                                                if let Some(row) = tabular.structure_columns.get(r)
                                                {
                                                    let rowvals = [
                                                        (r + 1).to_string(),
                                                        row.name.clone(),
                                                        row.data_type.clone(),
                                                        row.nullable
                                                            .map(|b| if b { "YES" } else { "NO" })
                                                            .unwrap_or("?")
                                                            .to_string(),
                                                        row.default_value
                                                            .clone()
                                                            .unwrap_or_default(),
                                                        row.extra.clone().unwrap_or_default(),
                                                        row.comment.clone().unwrap_or_default(),
                                                        String::new(),
                                                    ];
                                                    let mut fields: Vec<String> = Vec::new();
                                                    for c in cmin..=cmax {
                                                        let v = rowvals
                                                            .get(c)
                                                            .cloned()
                                                            .unwrap_or_default();
                                                        let quoted = if v.contains(',')
                                                            || v.contains('"')
                                                            || v.contains('\n')
                                                        {
                                                            format!(
                                                                "\"{}\"",
                                                                v.replace('"', "\"\"")
                                                            )
                                                        } else {
                                                            v
                                                        };
                                                        fields.push(quoted);
                                                    }
                                                    out.push_str(&fields.join(","));
                                                    out.push('\n');
                                                }
                                            }
                                            if !out.is_empty() {
                                                ui.ctx().copy_text(out);
                                            }
                                        }
                                        ui.close();
                                    }
                                    if ui
                                        .button(format!(
                                            "{} Copy Row as CSV",
                                            egui_icons::icons::ICON_DESCRIPTION.codepoint
                                        ))
                                        .clicked()
                                    {
                                        let csv_row = values
                                            .iter()
                                            .take(7)
                                            .map(|v| {
                                                if v.contains(',')
                                                    || v.contains('"')
                                                    || v.contains('\n')
                                                {
                                                    format!("\"{}\"", v.replace('"', "\"\""))
                                                } else {
                                                    v.clone()
                                                }
                                            })
                                            .collect::<Vec<_>>()
                                            .join(",");
                                        ui.ctx().copy_text(csv_row);
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui
                                        .button(format!(
                                            "{} Refresh",
                                            egui_icons::icons::ICON_REFRESH.codepoint
                                        ))
                                        .clicked()
                                    {
                                        tabular.request_structure_refresh = true;
                                        load_structure_info_for_current_table(tabular);
                                        crate::sidebar_database::refresh_connections_tree(tabular);
                                        ui.close();
                                    }
                                    if ui
                                        .button(format!(
                                            "{} Add Column",
                                            egui_icons::icons::ICON_ADD.codepoint
                                        ))
                                        .clicked()
                                    {
                                        if !tabular.adding_column {
                                            tabular.adding_column = true;
                                            if tabular.new_column_type.trim().is_empty() {
                                                tabular.new_column_type =
                                                    default_data_type_for_conn(tabular);
                                            }
                                            tabular.new_column_name.clear();
                                            tabular.new_column_default.clear();
                                            tabular.new_column_comment.clear();
                                            tabular.new_column_nullable = true;
                                        }
                                        ui.close();
                                    }
                                    if ui
                                        .button(format!(
                                            "{} Edit Column",
                                            egui_icons::icons::ICON_EDIT.codepoint
                                        ))
                                        .clicked()
                                    {
                                        tabular.editing_column = true;
                                        tabular.edit_column_original_name = col.name.clone();
                                        tabular.edit_column_name = col.name.clone();
                                        tabular.edit_column_type = col.data_type.clone();
                                        tabular.edit_column_nullable = col.nullable.unwrap_or(true);
                                        tabular.edit_column_default =
                                            col.default_value.clone().unwrap_or_default();
                                        tabular.edit_column_comment =
                                            col.comment.clone().unwrap_or_default();
                                        ui.close();
                                    }
                                    if ui
                                        .button(format!(
                                            "{} Drop Column",
                                            egui_icons::icons::ICON_DELETE.codepoint
                                        ))
                                        .clicked()
                                    {
                                        trigger_drop_column(tabular, &col.name);
                                        ui.close();
                                    }
                                });
                            }
                        }
                        // Draw the selected cell outline last (on top)
                        if let Some(rect) = selected_cell_rect {
                            let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 0, 0));
                            ui.painter()
                                .rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
                        }
                    });
                }
            }

            // NEW COLUMN ROW (editable)
            if tabular.adding_column {
                let edit_row_bg = if dark {
                    egui::Color32::from_rgb(32, 36, 46)
                } else {
                    egui::Color32::from_rgb(238, 244, 255)
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;

                    for (i, &w) in widths.iter().enumerate().take(8) {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(w, row_h), egui::Sense::hover());
                        ui.painter().rect_filled(rect, 0.0, edit_row_bg);
                        ui.painter()
                            .rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);

                        let mut child_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        child_ui.spacing_mut().item_spacing.x = 2.0;
                        // Compact widget padding so ComboBox/Button controls fit inside the
                        // fixed row height instead of the app-wide toolbar-sized padding (12,7).
                        child_ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                        // Safety net: never let a control paint outside its cell or outside visible area.
                        child_ui.set_clip_rect(rect.intersect(ui.clip_rect()));

                        match i {
                            0 => {
                                let idx_txt = format!("{}", tabular.structure_columns.len() + 1);
                                let txt_col = if dark {
                                    egui::Color32::LIGHT_GRAY
                                } else {
                                    egui::Color32::BLACK
                                };
                                child_ui.add_space(6.0);
                                child_ui
                                    .label(egui::RichText::new(idx_txt).size(13.0).color(txt_col));
                            }
                            1 => {
                                child_ui.add_space(3.0);
                                let text_w = (w - 8.0).max(20.0);
                                child_ui.add(
                                    egui::TextEdit::singleline(&mut tabular.new_column_name)
                                        .desired_width(text_w)
                                        .hint_text("column_name"),
                                );
                            }
                            2 => {
                                child_ui.add_space(3.0);
                                let picker_w = 30.0;
                                let text_w = (w - picker_w - 8.0).max(20.0);

                                child_ui.add(
                                    egui::TextEdit::singleline(&mut tabular.new_column_type)
                                        .desired_width(text_w),
                                );

                                let types = data_types_for_current_conn(tabular);
                                egui::ComboBox::from_id_salt("new_col_type_picker")
                                    .selected_text("")
                                    .width(14.0)
                                    .show_ui(&mut child_ui, |ui| {
                                        for t in types {
                                            if ui
                                                .selectable_label(tabular.new_column_type == *t, *t)
                                                .clicked()
                                            {
                                                tabular.new_column_type = t.to_string();
                                            }
                                        }
                                    });
                            }
                            3 => {
                                child_ui.add_space(3.0);
                                let combo_w =
                                    (w - 8.0 - 2.0 * child_ui.spacing().button_padding.x).max(20.0);
                                egui::ComboBox::from_id_salt("new_col_nullable")
                                    .selected_text(if tabular.new_column_nullable {
                                        "YES"
                                    } else {
                                        "NO"
                                    })
                                    .width(combo_w)
                                    .show_ui(&mut child_ui, |ui| {
                                        if ui
                                            .selectable_label(tabular.new_column_nullable, "YES")
                                            .clicked()
                                        {
                                            tabular.new_column_nullable = true;
                                        }
                                        if ui
                                            .selectable_label(!tabular.new_column_nullable, "NO")
                                            .clicked()
                                        {
                                            tabular.new_column_nullable = false;
                                        }
                                    });
                            }
                            4 => {
                                child_ui.add_space(3.0);
                                let text_w = (w - 8.0).max(20.0);
                                child_ui.add(
                                    egui::TextEdit::singleline(&mut tabular.new_column_default)
                                        .desired_width(text_w)
                                        .hint_text("NULL"),
                                );
                            }
                            5 => {
                                child_ui.add_space(6.0);
                                let muted = if dark {
                                    egui::Color32::from_rgb(100, 116, 139)
                                } else {
                                    egui::Color32::from_rgb(148, 163, 184)
                                };
                                child_ui.label(egui::RichText::new("-").size(13.0).color(muted));
                            }
                            6 => {
                                child_ui.add_space(3.0);
                                let text_w = (w - 8.0).max(20.0);
                                child_ui.add(
                                    egui::TextEdit::singleline(&mut tabular.new_column_comment)
                                        .desired_width(text_w)
                                        .hint_text("Description"),
                                );
                            }
                            7 => {
                                child_ui.add_space(3.0);
                                child_ui.spacing_mut().item_spacing.x = 4.0;
                                child_ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
                                let save_enabled = !tabular.new_column_name.trim().is_empty();
                                if child_ui
                                    .add_enabled(
                                        save_enabled,
                                        crate::window_egui::style::btn_primary_ctx(
                                            child_ui.ctx(),
                                            "Save",
                                        ),
                                    )
                                    .clicked()
                                {
                                    commit_new_column(tabular);
                                }
                                if child_ui
                                    .add(crate::window_egui::style::btn_secondary("Cancel"))
                                    .clicked()
                                {
                                    tabular.adding_column = false;
                                    tabular.new_column_comment.clear();
                                }
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
    tabular.structure_col_widths = widths;
}

pub(crate) fn commit_edit_column(tabular: &mut window_egui::Tabular) {
    if !tabular.editing_column {
        return;
    }
    let Some(conn_id) = tabular.current_connection_id else {
        tabular.editing_column = false;
        tabular.edit_column_comment.clear();
        return;
    };
    let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    else {
        tabular.editing_column = false;
        tabular.edit_column_comment.clear();
        return;
    };
    let table_name = infer_current_table_name(tabular);
    if table_name.is_empty() {
        tabular.editing_column = false;
        tabular.edit_column_comment.clear();
        return;
    }

    let old = tabular.edit_column_original_name.trim();
    let new_name = tabular.edit_column_name.trim();
    let new_type = tabular.edit_column_type.trim();
    let nullable = tabular.edit_column_nullable;
    let def = tabular.edit_column_default.trim();
    let comment = tabular.edit_column_comment.trim();

    let mut stmts: Vec<String> = Vec::new();
    match conn.connection_type {
        models::enums::DatabaseType::MySQL => {
            // Build complete column definition with type, nullable, default, and comment
            let mut column_def = new_type.to_string();
            if !nullable {
                column_def.push_str(" NOT NULL");
            }
            if !def.is_empty() {
                let upper = def.to_uppercase();
                let is_numeric = def.chars().all(|c| c.is_ascii_digit());
                let is_func = matches!(
                    upper.as_str(),
                    "CURRENT_TIMESTAMP" | "NOW()" | "CURRENT_DATE"
                );
                if is_numeric || is_func {
                    column_def.push_str(&format!(" DEFAULT {}", def));
                } else {
                    column_def.push_str(&format!(" DEFAULT '{}'", def.replace('\'', "''")));
                }
            }
            column_def.push_str(&format!(" COMMENT '{}'", comment.replace('\'', "''")));

            // MySQL supports CHANGE to rename+modify; use MODIFY if name unchanged
            let stmt = if old != new_name {
                format!(
                    "ALTER TABLE `{}` CHANGE `{}` `{}` {};",
                    table_name, old, new_name, column_def
                )
            } else {
                format!(
                    "ALTER TABLE `{}` MODIFY `{}` {};",
                    table_name, new_name, column_def
                )
            };
            stmts.push(stmt);
        }
        models::enums::DatabaseType::PostgreSQL => {
            if old != new_name {
                stmts.push(format!(
                    "ALTER TABLE \"{}\" RENAME COLUMN \"{}\" TO \"{}\";",
                    table_name, old, new_name
                ));
            }
            if !new_type.is_empty() {
                stmts.push(format!(
                    "ALTER TABLE \"{}\" ALTER COLUMN \"{}\" TYPE {};",
                    table_name, new_name, new_type
                ));
            }
            stmts.push(format!(
                "ALTER TABLE \"{}\" ALTER COLUMN \"{}\" {} NOT NULL;",
                table_name,
                new_name,
                if nullable { "DROP" } else { "SET" }
            ));
            if def.is_empty() {
                stmts.push(format!(
                    "ALTER TABLE \"{}\" ALTER COLUMN \"{}\" DROP DEFAULT;",
                    table_name, new_name
                ));
            } else {
                stmts.push(format!(
                    "ALTER TABLE \"{}\" ALTER COLUMN \"{}\" SET DEFAULT {};",
                    table_name, new_name, def
                ));
            }
            let pg_table = if table_name.contains('.') {
                table_name
                    .split('.')
                    .map(|p| format!("\"{}\"", p.trim_matches('"')))
                    .collect::<Vec<_>>()
                    .join(".")
            } else {
                format!("\"{}\"", table_name.trim_matches('"'))
            };
            if comment.is_empty() {
                stmts.push(format!(
                    "COMMENT ON COLUMN {}.\"{}\" IS NULL;",
                    pg_table, new_name
                ));
            } else {
                stmts.push(format!(
                    "COMMENT ON COLUMN {}.\"{}\" IS '{}';",
                    pg_table,
                    new_name,
                    comment.replace('\'', "''")
                ));
            }
        }
        models::enums::DatabaseType::MsSQL => {
            if old != new_name {
                stmts.push(format!(
                    "EXEC sp_rename '{}.{}', '{}', 'COLUMN';",
                    table_name, old, new_name
                ));
            }
            if !new_type.is_empty() {
                stmts.push(format!(
                    "ALTER TABLE [{}] ALTER COLUMN [{}] {}{};",
                    table_name,
                    new_name,
                    new_type,
                    if nullable { "" } else { " NOT NULL" }
                ));
            }
            if !def.is_empty() {
                // Note: Default constraints require named constraints; here we set default at column level (may require manual constraint handling)
                stmts.push(
                    "-- You may need to drop existing DEFAULT constraint before setting a new one"
                        .to_string(),
                );
            }
            let (schema_name, tbl) = if let Some((s, t)) = table_name.split_once('.') {
                (
                    s.trim_matches(|c| matches!(c, '[' | ']')),
                    t.trim_matches(|c| matches!(c, '[' | ']')),
                )
            } else {
                ("dbo", table_name.trim_matches(|c| matches!(c, '[' | ']')))
            };
            if comment.is_empty() {
                stmts.push(format!(
                    "IF EXISTS (SELECT 1 FROM fn_listextendedproperty('MS_Description', 'SCHEMA', '{}', 'TABLE', '{}', 'COLUMN', '{}')) \
                     EXEC sp_dropextendedproperty @name = N'MS_Description', @level0type = N'SCHEMA', @level0name = '{}', @level1type = N'TABLE', @level1name = '{}', @level2type = N'COLUMN', @level2name = '{}';",
                    schema_name, tbl, new_name, schema_name, tbl, new_name
                ));
            } else {
                stmts.push(format!(
                    "IF NOT EXISTS (SELECT 1 FROM fn_listextendedproperty('MS_Description', 'SCHEMA', '{}', 'TABLE', '{}', 'COLUMN', '{}')) \
                     EXEC sp_addextendedproperty @name = N'MS_Description', @value = N'{}', @level0type = N'SCHEMA', @level0name = '{}', @level1type = N'TABLE', @level1name = '{}', @level2type = N'COLUMN', @level2name = '{}'; \
                     ELSE \
                     EXEC sp_updateextendedproperty @name = N'MS_Description', @value = N'{}', @level0type = N'SCHEMA', @level0name = '{}', @level1type = N'TABLE', @level1name = '{}', @level2type = N'COLUMN', @level2name = '{}';",
                    schema_name, tbl, new_name, comment.replace('\'', "''"), schema_name, tbl, new_name, comment.replace('\'', "''"), schema_name, tbl, new_name
                ));
            }
        }
        models::enums::DatabaseType::SQLite => {
            stmts.push(format!("-- SQLite column edit requires table rebuild; consider manual migration for column '{}'.", old));
        }
        _ => {
            stmts.push("-- Edit column not supported".to_string());
        }
    }

    let full = stmts.join("\n");
    if !full.is_empty() {
        let insertion = format!("\n{}", full);
        let pos = tabular.editor.text.len();
        tabular.editor.apply_single_replace(pos..pos, &insertion);
        tabular.cursor_position = pos + insertion.len();
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.content = tabular.editor.text.clone();
            tab.is_modified = true;
        }
    }
    tabular.editing_column = false;
    tabular.edit_column_comment.clear();
    // Execute in the background so a slow/locked ALTER doesn't freeze the UI.
    run_structure_statement(tabular, conn_id, full, "Failed to edit column", |tabular| {
        tabular.request_structure_refresh = true;
        load_structure_info_for_current_table(tabular);
        crate::sidebar_database::refresh_connections_tree(tabular);
    });
}

pub(crate) fn render_drop_column_confirmation(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
) {
    if tabular.pending_drop_column_name.is_none() || tabular.pending_drop_column_stmt.is_none() {
        return;
    }
    let col_name = tabular.pending_drop_column_name.clone().unwrap();
    let stmt = tabular.pending_drop_column_stmt.clone().unwrap();
    let mut close = false;
    crate::window_egui::style::render_modal_backdrop(ctx, "drop_column_backdrop", true);

    egui::Window::new("Drop Column?")
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_width(440.0)
        .show(ctx, |ui| {
            crate::window_egui::style::render_modal_header(ui, "Drop Column?", &mut close);
            ui.add_space(8.0);

            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.label(format!("Column: {}", col_name));
                ui.add_space(4.0);
                ui.code(&stmt);
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let confirm_btn = egui::Button::new(
                        egui::RichText::new("Confirm").color(egui::Color32::WHITE),
                    )
                    .fill(crate::window_egui::style::theme_danger(ui.ctx()));

                    if ui.add(confirm_btn).clicked() {
                        if let Some(conn_id) = tabular.current_connection_id
                            && !stmt.starts_with("--")
                        {
                            let victim = col_name.clone();
                            run_structure_statement(
                                tabular,
                                conn_id,
                                stmt.clone(),
                                "Failed to drop column",
                                move |tabular| {
                                    tabular.structure_columns.retain(|it| it.name != victim);
                                    tabular.request_structure_refresh = true;
                                    load_structure_info_for_current_table(tabular);
                                    crate::sidebar_database::refresh_connections_tree(tabular);
                                },
                            );
                        }
                        tabular.pending_drop_column_name = None;
                        tabular.pending_drop_column_stmt = None;
                    }
                });
            });
        });

    if close {
        tabular.pending_drop_column_name = None;
        tabular.pending_drop_column_stmt = None;
    }
}

fn commit_new_column(tabular: &mut window_egui::Tabular) {
    if !tabular.adding_column {
        return;
    }
    let Some(conn_id) = tabular.current_connection_id else {
        tabular.adding_column = false;
        tabular.new_column_comment.clear();
        return;
    };
    let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    else {
        tabular.adding_column = false;
        tabular.new_column_comment.clear();
        return;
    };
    // Strip any identifier-quote characters the inferred name may already carry
    // (e.g. when the active query quoted the table as `` `branches` ``, the
    // origin-inference in `sql.rs` used to leak the backticks into the plain
    // name). Without this, wrapping it in backticks again below produces a
    // doubled backtick and a MySQL 1064 syntax error.
    let table_name = infer_current_table_name(tabular)
        .trim_matches(|c| matches!(c, '`' | '"' | '[' | ']'))
        .to_string();
    if table_name.is_empty() {
        // Inform user explicitly
        tabular.toasts.error("Cannot add column: table name not found (open the table data or select a table first).".to_string());
        return;
    }
    let col_name = tabular
        .new_column_name
        .trim()
        .trim_matches(|c| matches!(c, '`' | '"' | '[' | ']'));
    if col_name.is_empty() {
        return;
    }

    // Build DEFAULT clause; allow numeric and common time keywords without quotes
    let mut default_clause = String::new();
    if !tabular.new_column_default.trim().is_empty() {
        let d = tabular.new_column_default.trim();
        let upper = d.to_uppercase();
        let is_numeric = d.chars().all(|c| c.is_ascii_digit());
        let is_func = matches!(
            upper.as_str(),
            "CURRENT_TIMESTAMP" | "NOW()" | "GETDATE()" | "CURRENT_DATE"
        );
        if is_numeric || is_func {
            default_clause = format!(" DEFAULT {}", d);
        } else {
            default_clause = format!(" DEFAULT '{}'", d.replace('\'', "''"));
        }
    }
    let null_clause = if tabular.new_column_nullable {
        ""
    } else {
        " NOT NULL"
    };

    let comment = tabular.new_column_comment.trim();
    let mut stmts = Vec::new();
    match conn.connection_type {
        models::enums::DatabaseType::MySQL => {
            let comment_clause = if !comment.is_empty() {
                format!(" COMMENT '{}'", comment.replace('\'', "''"))
            } else {
                String::new()
            };
            stmts.push(format!(
                "ALTER TABLE `{}` ADD COLUMN `{}` {}{}{}{};",
                table_name,
                col_name,
                tabular.new_column_type,
                null_clause,
                default_clause,
                comment_clause
            ));
        }
        models::enums::DatabaseType::PostgreSQL => {
            stmts.push(format!(
                "ALTER TABLE \"{}\" ADD COLUMN \"{}\" {}{}{};",
                table_name, col_name, tabular.new_column_type, null_clause, default_clause
            ));
            if !comment.is_empty() {
                let pg_table = if table_name.contains('.') {
                    table_name
                        .split('.')
                        .map(|p| format!("\"{}\"", p.trim_matches('"')))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    format!("\"{}\"", table_name.trim_matches('"'))
                };
                stmts.push(format!(
                    "COMMENT ON COLUMN {}.\"{}\" IS '{}';",
                    pg_table,
                    col_name,
                    comment.replace('\'', "''")
                ));
            }
        }
        models::enums::DatabaseType::MsSQL => {
            stmts.push(format!(
                "ALTER TABLE [{}] ADD [{}] {}{}{};",
                table_name, col_name, tabular.new_column_type, null_clause, default_clause
            ));
            if !comment.is_empty() {
                let (schema_name, tbl) = if let Some((s, t)) = table_name.split_once('.') {
                    (
                        s.trim_matches(|c| matches!(c, '[' | ']')),
                        t.trim_matches(|c| matches!(c, '[' | ']')),
                    )
                } else {
                    ("dbo", table_name.trim_matches(|c| matches!(c, '[' | ']')))
                };
                stmts.push(format!(
                    "EXEC sp_addextendedproperty @name = N'MS_Description', @value = N'{}', @level0type = N'SCHEMA', @level0name = '{}', @level1type = N'TABLE', @level1name = '{}', @level2type = N'COLUMN', @level2name = '{}';",
                    comment.replace('\'', "''"),
                    schema_name,
                    tbl,
                    col_name
                ));
            }
        }
        models::enums::DatabaseType::SQLite => {
            stmts.push(format!(
                "ALTER TABLE `{}` ADD COLUMN `{}` {}{}{};",
                table_name, col_name, tabular.new_column_type, null_clause, default_clause
            ));
        }
        _ => {
            stmts.push("-- Add column not supported for this database type".to_string());
        }
    }

    let stmt = stmts.join("\n");

    // Append to editor for visibility via rope edit
    let insertion = if stmt.starts_with("--") {
        stmt.clone()
    } else {
        format!("\n{}", stmt)
    };
    let pos = tabular.editor.text.len();
    tabular.editor.apply_single_replace(pos..pos, &insertion);
    tabular.cursor_position = pos + insertion.len();
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = tabular.editor.text.clone();
        tab.is_modified = true;
    }

    // Reset UI state
    tabular.adding_column = false;
    tabular.new_column_default.clear();
    tabular.new_column_name.clear();
    tabular.new_column_comment.clear();

    // Execute in the background and refresh structure on success, so a
    // slow/locked ALTER TABLE doesn't freeze the UI thread.
    if !stmt.starts_with("--") {
        run_structure_statement(tabular, conn_id, stmt, "Failed to add column", |tabular| {
            // Reload from source to ensure correct view
            tabular.request_structure_refresh = true;
            load_structure_info_for_current_table(tabular);
            crate::sidebar_database::refresh_connections_tree(tabular);
        });
    }
}

pub(crate) fn start_inline_add_index(tabular: &mut window_egui::Tabular) {
    tabular.adding_index = true;
    tabular.new_index_unique = false;
    tabular.new_index_method.clear();
    // Prefill columns using first column(s) from structure view if available
    if !tabular.structure_columns.is_empty() {
        let first_cols: Vec<String> = tabular
            .structure_columns
            .iter()
            .take(2)
            .map(|c| c.name.clone())
            .collect();
        tabular.new_index_columns = first_cols.join(",");
    } else {
        tabular.new_index_columns.clear();
    }
    let t = infer_current_table_name(tabular);
    tabular.new_index_name = if t.is_empty() {
        "idx_new_col".to_string()
    } else {
        format!("idx_{}_col", t)
    };
}

fn commit_new_index(tabular: &mut window_egui::Tabular) {
    if !tabular.adding_index {
        return;
    }
    let Some(conn_id) = tabular.current_connection_id else {
        tabular.adding_index = false;
        return;
    };
    let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    else {
        tabular.adding_index = false;
        return;
    };
    let table_name = infer_current_table_name(tabular);
    if table_name.is_empty() {
        // Don't silently fail; tell user
        tabular.toasts.error("Cannot create index: table name not found (open the table data or select a table first).".to_string());
        return;
    }
    let idx_name = tabular.new_index_name.trim();
    if idx_name.is_empty() {
        return;
    }
    let cols_raw = tabular.new_index_columns.trim();
    if cols_raw.is_empty() {
        return;
    }
    let cols: Vec<String> = cols_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if cols.is_empty() {
        return;
    }
    let method = tabular.new_index_method.trim();
    // Prepare index creation statement buffer when user requests creating an index.
    let stmt = match conn.connection_type {
        models::enums::DatabaseType::MySQL => {
            // ALTER TABLE add index for consistency (so it can run with other alters)
            let algo_clause = if method.is_empty() {
                "".to_string()
            } else {
                format!(" USING {}", method.to_uppercase())
            };
            let unique = if tabular.new_index_unique {
                "UNIQUE "
            } else {
                ""
            };
            format!(
                "ALTER TABLE `{}` ADD {}INDEX `{}` ({}){};",
                table_name,
                unique,
                idx_name,
                cols.iter()
                    .map(|c| format!("`{}`", c))
                    .collect::<Vec<_>>()
                    .join(", "),
                algo_clause
            )
        }
        models::enums::DatabaseType::PostgreSQL => {
            let unique = if tabular.new_index_unique {
                "UNIQUE "
            } else {
                ""
            };
            let using_clause = if method.is_empty() {
                "".to_string()
            } else {
                format!(" USING {}", method)
            };
            format!(
                "CREATE {}INDEX \"{}\" ON \"{}\"{} ({});",
                unique,
                idx_name,
                table_name,
                using_clause,
                cols.iter()
                    .map(|c| format!("\"{}\"", c))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        models::enums::DatabaseType::MsSQL => {
            let unique = if tabular.new_index_unique {
                "UNIQUE "
            } else {
                ""
            };
            // SQL Server: CREATE [UNIQUE] [NONCLUSTERED] INDEX idx ON table(col,...)
            format!(
                "CREATE {}INDEX [{}] ON [{}] ({});",
                unique,
                idx_name,
                table_name,
                cols.join(", ")
            )
        }
        models::enums::DatabaseType::SQLite => {
            let unique = if tabular.new_index_unique {
                "UNIQUE "
            } else {
                ""
            };
            format!(
                "CREATE {}INDEX IF NOT EXISTS `{}` ON `{}` ({});",
                unique,
                idx_name,
                table_name,
                cols.iter()
                    .map(|c| format!("`{}`", c))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        _ => "-- Create index not supported for this database type".to_string(),
    };
    let insertion = if stmt.starts_with("--") {
        stmt.clone()
    } else {
        format!("\n{}", stmt)
    };
    let pos = tabular.editor.text.len();
    tabular.editor.apply_single_replace(pos..pos, &insertion);
    tabular.cursor_position = pos + insertion.len();
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = tabular.editor.text.clone();
        tab.is_modified = true;
    }
    // Append optimistic row
    tabular
        .structure_indexes
        .push(models::structs::IndexStructInfo {
            name: idx_name.to_string(),
            method: if method.is_empty() {
                None
            } else {
                Some(method.to_string())
            },
            unique: tabular.new_index_unique,
            columns: cols.clone(),
        });
    // Reset state before execution to avoid double firing on UI re-render
    tabular.adding_index = false;
    tabular.new_index_columns.clear();
    tabular.new_index_method.clear();
    tabular.new_index_name.clear();
    // Auto execute in the background and refresh, so a slow/locked CREATE
    // INDEX doesn't freeze the UI thread.
    if !stmt.starts_with("--") {
        run_structure_statement(tabular, conn_id, stmt, "CREATE INDEX failed", |tabular| {
            tabular.request_structure_refresh = true;
            load_structure_info_for_current_table(tabular);
        });
    }
}

pub(crate) fn render_drop_index_confirmation(
    tabular: &mut window_egui::Tabular,
    ctx: &egui::Context,
) {
    if tabular.pending_drop_index_name.is_none() || tabular.pending_drop_index_stmt.is_none() {
        return;
    }
    let idx_name = tabular.pending_drop_index_name.clone().unwrap();
    let stmt = tabular.pending_drop_index_stmt.clone().unwrap();
    let mut close = false;
    crate::window_egui::style::render_modal_backdrop(ctx, "drop_index_backdrop", true);

    egui::Window::new("Drop Index?")
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_width(420.0)
        .show(ctx, |ui| {
            crate::window_egui::style::render_modal_header(ui, "Drop Index?", &mut close);
            ui.add_space(8.0);

            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.label(format!("Index: {}", idx_name));
                ui.add_space(4.0);
                ui.code(&stmt);
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let confirm_btn = egui::Button::new(
                        egui::RichText::new("Confirm").color(egui::Color32::WHITE),
                    )
                    .fill(crate::window_egui::style::theme_danger(ui.ctx()));

                    if ui.add(confirm_btn).clicked() {
                        if let Some(conn_id) = tabular.current_connection_id
                            && !stmt.starts_with("--")
                        {
                            let victim = idx_name.clone();
                            run_structure_statement(
                                tabular,
                                conn_id,
                                stmt.clone(),
                                "Failed to drop index",
                                move |tabular| {
                                    tabular.structure_indexes.retain(|it| it.name != victim);
                                    tabular.request_structure_refresh = true;
                                    load_structure_info_for_current_table(tabular);
                                },
                            );
                        }
                        tabular.pending_drop_index_name = None;
                        tabular.pending_drop_index_stmt = None;
                    }
                });
            });
        });

    if close {
        tabular.pending_drop_index_name = None;
        tabular.pending_drop_index_stmt = None;
    }
}

// Handle directory picker dialog
