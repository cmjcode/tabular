use eframe::egui;
use crate::{models, window_egui};
use super::{load_structure_info_for_current_table, infer_current_table_name};

pub(crate) fn render_structure_view(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    let avail = ui.available_size();

    ui.horizontal(|ui| {
    let toggle_width = 20.0;
    let toggle_height = 80.0;

    ui.add_space(4.0);

        ui.scope(|ui| {
            let accent_col = window_egui::style::theme_accent(ui.ctx());
            let mut style = ui.style().as_ref().clone();
            style.visuals.selection.bg_fill = accent_col;
            style.visuals.selection.stroke.color = accent_col;
            style.visuals.widgets.active.bg_fill = accent_col;
            style.visuals.widgets.active.weak_bg_fill = accent_col;
            ui.set_style(style);

            ui.set_min_width(toggle_width);
            ui.set_min_height(avail.y);
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                let default_text = ui.visuals().widgets.inactive.fg_stroke.color;

                let active_cols =
                    tabular.structure_sub_view == models::structs::StructureSubView::Columns;
                     let draw_vertical_toggle = |ui: &mut egui::Ui,
                                                            label: &str,
                                                            active: bool|
                      -> egui::Response {
                          let button_size = egui::vec2(toggle_width, toggle_height);
                    let (rect, response) =
                        ui.allocate_exact_size(button_size, egui::Sense::click());

                    let mut bg = if active {
                        window_egui::style::theme_accent(ui.ctx())
                    } else {
                        ui.visuals().widgets.inactive.bg_fill
                    };
                    if response.hovered() && !active {
                        bg = bg.gamma_multiply(1.12);
                    }

                    let stroke_color = if active {
                        window_egui::style::theme_accent(ui.ctx())
                    } else {
                        ui.visuals().widgets.inactive.bg_stroke.color
                    };
                    let stroke = egui::Stroke::new(1.0, stroke_color);

                    let painter = ui.painter();
                    let rounding = 2.0;
                    painter.rect_filled(rect, rounding, bg);
                    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Outside);

                    let text_color = if active {
                        egui::Color32::WHITE
                    } else {
                        default_text
                    };
                    let font_id = ui.style().text_styles[&egui::TextStyle::Button].clone();
                    let galley = painter.layout_no_wrap(label.to_owned(), font_id, text_color);
                    let size = galley.rect.size();
                    let pos =
                        rect.center() + egui::vec2(-size.y * 0.5, size.x * 0.5);
                    let mut text_shape = egui::epaint::TextShape::new(pos, galley, text_color);
                    text_shape.angle = -std::f32::consts::FRAC_PI_2;
                    painter.add(text_shape);

                    response
                };

                let cols_resp = draw_vertical_toggle(ui, "☰ Columns", active_cols);
                if cols_resp.clicked() {
                    tabular.structure_sub_view = models::structs::StructureSubView::Columns;
                    tabular.structure_sel_anchor = None;
                    tabular.structure_selected_cell = None;
                    tabular.structure_selected_row = None;
                }

                ui.add_space(4.0);

                let active_idx =
                    tabular.structure_sub_view == models::structs::StructureSubView::Indexes;
                let idx_resp = draw_vertical_toggle(ui, "📈 Indexes", active_idx);
                if idx_resp.clicked() {
                    tabular.structure_sub_view = models::structs::StructureSubView::Indexes;
                    load_structure_info_for_current_table(tabular);
                    tabular.structure_sel_anchor = None;
                    tabular.structure_selected_cell = None;
                    tabular.structure_selected_row = None;
                }

                ui.add_space(ui.available_height());
            });
        });

        let remaining = ui.available_size();
        let content_size = egui::vec2(remaining.x.max(0.0), avail.y);
        ui.allocate_ui_with_layout(content_size, egui::Layout::top_down(egui::Align::LEFT), |ui| {
            egui::ScrollArea::both()
                .id_salt("structure_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
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
                                tabular.structure_idx_col_widths =
                                    vec![40.0, 200.0, 120.0, 70.0, 260.0, 120.0];
                            }
                            let mut widths = tabular.structure_idx_col_widths.clone();
                            for w in widths.iter_mut() {
                                *w = w.clamp(40.0, 800.0);
                            }
                            let dark = ui.visuals().dark_mode;
                            let border = if dark {
                                egui::Color32::from_gray(55)
                            } else {
                                egui::Color32::from_gray(190)
                            };
                            let stroke = egui::Stroke::new(0.5, border);
                            let header_text_col = if dark {
                                egui::Color32::from_rgb(220, 220, 255)
                            } else {
                                egui::Color32::from_rgb(60, 60, 120)
                            };
                            let header_bg = if dark {
                                egui::Color32::from_rgb(30, 30, 30)
                            } else {
                                egui::Color32::from_gray(240)
                            };
                            let row_h = 26.0f32;
                            let header_h = 30.0f32;
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
                                            ui.painter().rect_filled(rect, 0.0, header_bg);
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
                                                header_text_col,
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
                                                if ui.button("➕ Add Index").clicked() {
                                                    if !tabular.adding_index {
                                                        start_inline_add_index(tabular);
                                                    }
                                                    ui.close();
                                                }
                                                if ui.button("🔄 Refresh").clicked() {
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
                                                        egui::Color32::from_rgb(40, 40, 40)
                                                    } else {
                                                        egui::Color32::from_rgb(250, 250, 250)
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
                                                    if ui.button("📋 Copy Cell Value").clicked() {
                                                        ui.ctx().copy_text(val.clone());
                                                        ui.close();
                                                    }
                                                    if ui.button("📄 Copy Selection as CSV").clicked()
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
                                                    if ui.button("📄 Copy Row as CSV").clicked() {
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
                                                    if ui.button("➕ Add Index").clicked() {
                                                        if !tabular.adding_index {
                                                            start_inline_add_index(tabular);
                                                        }
                                                        ui.close();
                                                    }
                                                    if ui.button("🔄 Refresh").clicked() {
                                                        tabular.request_structure_refresh = true;
                                                        load_structure_info_for_current_table(tabular);
                                                        ui.close();
                                                    }
                                                    if ui.button("❌ Drop Index").clicked() {
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
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 0.0;
                                            // #
                                            let (rect_no, _) = ui.allocate_exact_size(
                                                egui::vec2(widths[0], row_h),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_stroke(
                                                rect_no,
                                                0.0,
                                                stroke,
                                                egui::StrokeKind::Outside,
                                            );
                                            let idx_txt =
                                                format!("{}", tabular.structure_indexes.len() + 1);
                                            let txt_col = if dark {
                                                egui::Color32::LIGHT_GRAY
                                            } else {
                                                egui::Color32::BLACK
                                            };
                                            ui.painter().text(
                                                rect_no.left_center() + egui::vec2(6.0, 0.0),
                                                egui::Align2::LEFT_CENTER,
                                                idx_txt,
                                                egui::FontId::proportional(13.0),
                                                txt_col,
                                            );
                                            // index_name
                                            let w_name = widths[1];
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(w_name, row_h),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    ui.set_min_width(w_name - 8.0);
                                                    ui.add_space(4.0);
                                                    if tabular.new_index_name.is_empty() {
                                                        tabular.new_index_name = format!(
                                                            "idx_{}_col",
                                                            infer_current_table_name(tabular)
                                                        );
                                                    }
                                                    ui.text_edit_singleline(&mut tabular.new_index_name);
                                                },
                                            );
                                            // algorithm
                                            let w_alg = widths[2];
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(w_alg, row_h),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    let algos = ["", "btree", "hash", "gin", "gist"];
                                                    egui::ComboBox::from_id_salt("new_index_algo")
                                                        .selected_text(if tabular
                                                            .new_index_method
                                                            .is_empty()
                                                        {
                                                            "(auto)"
                                                        } else {
                                                            &tabular.new_index_method
                                                        })
                                                        .show_ui(ui, |ui| {
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
                                                                    tabular.new_index_method =
                                                                        a.to_string();
                                                                }
                                                            }
                                                        });
                                                },
                                            );
                                            // unique
                                            let w_unique = widths[3];
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(w_unique, row_h),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    egui::ComboBox::from_id_salt("new_index_unique")
                                                        .selected_text(if tabular.new_index_unique {
                                                            "YES"
                                                        } else {
                                                            "NO"
                                                        })
                                                        .show_ui(ui, |ui| {
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
                                                },
                                            );
                                            // columns
                                            let w_cols = widths[4];
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(w_cols, row_h),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    ui.set_min_width(w_cols - 8.0);
                                                    ui.add_space(4.0);
                                                    if tabular.new_index_columns.is_empty() {
                                                        tabular.new_index_columns = "col1".to_string();
                                                    }
                                                    ui.text_edit_singleline(&mut tabular.new_index_columns);
                                                },
                                            );
                                            // actions
                                            let w_act = widths[5];
                                            ui.allocate_ui_with_layout(
                                                egui::vec2(w_act, row_h),
                                                egui::Layout::left_to_right(egui::Align::Center),
                                                |ui| {
                                                    let save_enabled =
                                                        !tabular.new_index_name.trim().is_empty()
                                                            && !tabular.new_index_columns.trim().is_empty();
                                                    if ui
                                                        .add_enabled(
                                                            save_enabled,
                                                            egui::Button::new("Save"),
                                                        )
                                                        .clicked()
                                                    {
                                                        commit_new_index(tabular);
                                                    }
                                                    if ui.button("Cancel").clicked() {
                                                        tabular.adding_index = false;
                                                    }
                                                },
                                            );
                                        });
                                    }
                                });
                            tabular.structure_idx_col_widths = widths;
                        }
                    }
                });
        });
    });
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
    ];
    if tabular.structure_col_widths.len() != headers.len() {
        tabular.structure_col_widths = vec![40.0, 180.0, 160.0, 90.0, 160.0, 120.0];
    }
    let mut widths = tabular.structure_col_widths.clone();
    for w in widths.iter_mut() {
        *w = w.clamp(40.0, 600.0);
    }
    let dark = ui.visuals().dark_mode;
    let border = if dark {
        egui::Color32::from_gray(55)
    } else {
        egui::Color32::from_gray(190)
    };
    let stroke = egui::Stroke::new(0.5, border);
    let header_text_col = if dark {
        egui::Color32::from_rgb(220, 220, 255)
    } else {
        egui::Color32::from_rgb(60, 60, 120)
    };
    let header_bg = if dark {
        egui::Color32::from_rgb(30, 30, 30)
    } else {
        egui::Color32::from_gray(240)
    };
    let row_h = 26.0f32;
    let header_h = 30.0f32;

    egui::ScrollArea::both().id_salt("struct_cols_inline").auto_shrink([false,false]).show(ui, |ui| {
            // HEADER
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (i,h) in headers.iter().enumerate() {
                    let w = widths[i];
                    // Make header cells clickable so we can attach context menu (right-click)
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, header_h), egui::Sense::click());
                    ui.painter().rect_filled(rect, 0.0, header_bg);
                    ui.painter().rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
                    ui.painter().text(rect.left_center() + egui::vec2(6.0,0.0), egui::Align2::LEFT_CENTER, *h, egui::FontId::proportional(13.0), header_text_col);
                    // simple resize region
                    let handle = egui::Rect::from_min_max(egui::pos2(rect.max.x - 4.0, rect.min.y), rect.max);
                    let rh = ui.interact(handle, egui::Id::new(("struct_cols_inline","resize",i)), egui::Sense::drag());
                    if rh.dragged() { widths[i] = (widths[i] + rh.drag_delta().x).clamp(40.0, 600.0); ui.ctx().request_repaint(); }
                    if rh.hovered() { ui.painter().rect_filled(handle, 0.0, egui::Color32::from_gray(80)); }
                    // Context menu on any header cell
                    resp.context_menu(|ui| {
                        if ui.button("🔄 Refresh").clicked() { tabular.request_structure_refresh = true; load_structure_info_for_current_table(tabular); ui.close(); }
                        if ui.button("➕ Add Column").clicked() {
                            if !tabular.adding_column { // initialize add column row
                                tabular.adding_column = true;
                                if tabular.new_column_type.trim().is_empty() { tabular.new_column_type = "varchar(255)".to_string(); }
                                tabular.new_column_name.clear();
                                tabular.new_column_default.clear();
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
            for (idx,col) in existing_cols.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let values = [
                        (idx+1).to_string(),
                        col.name.clone(),
                        col.data_type.clone(),
                        col.nullable.map(|b| if b {"YES"} else {"NO"}).unwrap_or("?").to_string(),
                        col.default_value.clone().unwrap_or_default(),
                        col.extra.clone().unwrap_or_default(),
                    ];
                    // Defer selected cell border so it paints last for this row
                    let mut selected_cell_rect: Option<egui::Rect> = None;
                    for (i,val) in values.iter().enumerate() {
                        let w = widths[i];
                        // All cells clickable for context menu
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w,row_h), egui::Sense::click_and_drag());
                        if idx %2 ==1 { let bg = if dark { egui::Color32::from_rgb(40,40,40) } else { egui::Color32::from_rgb(250,250,250) }; ui.painter().rect_filled(rect,0.0,bg);}
                        // Selection highlight
                        let is_row_selected = tabular.structure_selected_row == Some(idx);
                        let is_cell_selected = tabular.structure_selected_cell == Some((idx, i));
                        // Multi-selection block highlight (Structure)
                        if let (Some(a), Some(b)) = (tabular.structure_sel_anchor, tabular.structure_selected_cell) {
                            let (ar, ac) = a; let (br, bc) = b;
                            let rmin = ar.min(br); let rmax = ar.max(br);
                            let cmin = ac.min(bc); let cmax = ac.max(bc);
                            if idx >= rmin && idx <= rmax && i >= cmin && i <= cmax {
                                let sel = if dark { egui::Color32::from_rgba_unmultiplied(255,80,20,28) } else { egui::Color32::from_rgba_unmultiplied(255,120,40,60) };
                                ui.painter().rect_filled(rect, 0.0, sel);
                            }
                        }
                        if is_row_selected {
                            let sel = if dark { egui::Color32::from_rgba_unmultiplied(100,150,255,30) } else { egui::Color32::from_rgba_unmultiplied(200,220,255,80) };
                            ui.painter().rect_filled(rect, 0.0, sel);
                        }
                        // Base grid stroke first
                        ui.painter().rect_stroke(rect,0.0,stroke, egui::StrokeKind::Outside);
                        // Defer selected outline to avoid being overdrawn by neighbor cells
                        if is_cell_selected { selected_cell_rect = Some(rect); }
                        let txt_col = if dark { egui::Color32::LIGHT_GRAY } else { egui::Color32::BLACK };
                        ui.painter().text(rect.left_center()+egui::vec2(6.0,0.0), egui::Align2::LEFT_CENTER, val, egui::FontId::proportional(13.0), txt_col);
                        if resp.clicked() {
                            let shift = ui.input(|i| i.modifiers.shift);
                            tabular.structure_selected_row = Some(idx);
                            tabular.structure_selected_cell = Some((idx, i));
                            if !shift || tabular.structure_sel_anchor.is_none() { tabular.structure_sel_anchor = Some((idx, i)); }
                            tabular.table_recently_clicked = true;
                        }
                        // Drag-to-select: when user drags over cells, extend the selection to current cell
                        if resp.drag_started() {
                            tabular.structure_dragging = true;
                            if tabular.structure_sel_anchor.is_none() { tabular.structure_sel_anchor = Some((idx, i)); }
                            tabular.structure_selected_row = Some(idx);
                            tabular.structure_selected_cell = Some((idx, i));
                        }
                        // While dragging, update selection when hovering over any cell
                        if tabular.structure_dragging && ui.input(|inp| inp.pointer.primary_down()) && resp.hovered() {
                            tabular.structure_selected_row = Some(idx);
                            tabular.structure_selected_cell = Some((idx, i));
                        }
                        // End drag when primary is released anywhere
                        if tabular.structure_dragging && !ui.input(|inp| inp.pointer.primary_down()) { tabular.structure_dragging = false; }
                        // Context menu on every cell
                        resp.context_menu(|ui| {
                            if ui.button("📋 Copy Cell Value").clicked() {
                                ui.ctx().copy_text(val.clone());
                                ui.close();
                            }
                            if ui.button("📄 Copy Selection as CSV").clicked() {
                                if let (Some(a), Some(b)) = (tabular.structure_sel_anchor, tabular.structure_selected_cell) {
                                    let (ar, ac) = a; let (br, bc) = b;
                                    let rmin = ar.min(br); let rmax = ar.max(br);
                                    let cmin = ac.min(bc); let cmax = ac.max(bc);
                                    let mut out = String::new();
                                    for r in rmin..=rmax {
                                        // rebuild row values from current row (values corresponds to idx row)
                                        // we need from tabular.structure_columns for other rows
                                        if let Some(row) = tabular.structure_columns.get(r) {
                                            let rowvals = [
                                                (r+1).to_string(),
                                                row.name.clone(),
                                                row.data_type.clone(),
                                                row.nullable.map(|b| if b {"YES"} else {"NO"}).unwrap_or("?").to_string(),
                                                row.default_value.clone().unwrap_or_default(),
                                                row.extra.clone().unwrap_or_default(),
                                            ];
                                            let mut fields: Vec<String> = Vec::new();
                                            for c in cmin..=cmax {
                                                let v = rowvals.get(c).cloned().unwrap_or_default();
                                                let quoted = if v.contains(',') || v.contains('"') || v.contains('\n') { format!("\"{}\"", v.replace('"', "\"\"")) } else { v };
                                                fields.push(quoted);
                                            }
                                            out.push_str(&fields.join(",")); out.push('\n');
                                        }
                                    }
                                    if !out.is_empty() { ui.ctx().copy_text(out); }
                                }
                                ui.close();
                            }
                            if ui.button("📄 Copy Row as CSV").clicked() {
                                let csv_row = values.iter().map(|v| {
                                    if v.contains(',') || v.contains('"') || v.contains('\n') { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.clone() }
                                }).collect::<Vec<_>>().join(",");
                                ui.ctx().copy_text(csv_row);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("🔄 Refresh").clicked() { tabular.request_structure_refresh = true; load_structure_info_for_current_table(tabular); ui.close(); }
                            if ui.button("➕ Add Column").clicked() {
                                if !tabular.adding_column {
                                    tabular.adding_column = true;
                                    if tabular.new_column_type.trim().is_empty() { tabular.new_column_type = "varchar(255)".to_string(); }
                                    tabular.new_column_name.clear();
                                    tabular.new_column_default.clear();
                                    tabular.new_column_nullable = true;
                                }
                                ui.close();
                            }
                            if ui.button("☑ Edit Column").clicked() {
                                tabular.editing_column = true;
                                tabular.edit_column_original_name = col.name.clone();
                                tabular.edit_column_name = col.name.clone();
                                tabular.edit_column_type = col.data_type.clone();
                                tabular.edit_column_nullable = col.nullable.unwrap_or(true);
                                tabular.edit_column_default = col.default_value.clone().unwrap_or_default();
                                ui.close();
                            }
                            if ui.button("🗑 Drop Column").clicked() {
                                let table_name = infer_current_table_name(tabular);
                                if let Some(conn_id) = tabular.current_connection_id && let Some(conn) = tabular.connections.iter().find(|c| c.id==Some(conn_id)).cloned() {
                                    let stmt = match conn.connection_type {
                                        models::enums::DatabaseType::MySQL => format!("ALTER TABLE `{}` DROP COLUMN `{}`;", table_name, col.name),
                                        models::enums::DatabaseType::PostgreSQL => format!("ALTER TABLE \"{}\" DROP COLUMN \"{}\";", table_name, col.name),
                                        models::enums::DatabaseType::MsSQL => format!("ALTER TABLE [{}] DROP COLUMN [{}];", table_name, col.name),
                                        models::enums::DatabaseType::SQLite => format!("-- SQLite drop column requires table rebuild; not supported automatically. Consider manual migration for '{}'.", col.name),
                                        _ => "-- Drop column not supported for this database type".to_string(),
                                    };
                                    tabular.pending_drop_column_name = Some(col.name.clone());
                                    tabular.pending_drop_column_stmt = Some(stmt.clone());
                                    // Append the generated SQL to the editor via rope edit
                                    let insertion = format!("\n{}", stmt);
                                    let pos = tabular.editor.text.len();
                                    tabular.editor.apply_single_replace(pos..pos, &insertion);
                                    tabular.cursor_position = pos + insertion.len();
                                    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                                        tab.content = tabular.editor.text.clone();
                                        tab.is_modified = true;
                                    }
                                }
                                ui.close();
                            }
                        });
                    }
                    // Draw the selected cell outline last (on top)
                    if let Some(rect) = selected_cell_rect {
                        let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 0, 0));
                        ui.painter().rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Outside);
                    }
                });
            }

            // NEW COLUMN ROW (editable)
            if tabular.adding_column {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // #
                    let (rect_no,_) = ui.allocate_exact_size(egui::vec2(widths[0], row_h), egui::Sense::hover());
                    ui.painter().rect_stroke(rect_no,0.0,stroke, egui::StrokeKind::Outside);
                    let idx_txt = format!("{}", tabular.structure_columns.len()+1);
                    let txt_col = if dark { egui::Color32::LIGHT_GRAY } else { egui::Color32::BLACK }; ui.painter().text(rect_no.left_center()+egui::vec2(6.0,0.0), egui::Align2::LEFT_CENTER, idx_txt, egui::FontId::proportional(13.0), txt_col);
                    // Name
                    let w_name = widths[1];
                    ui.allocate_ui_with_layout(egui::vec2(w_name,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_name-8.0);
                        ui.add_space(4.0);
                        ui.text_edit_singleline(&mut tabular.new_column_name);
                    });
                    // Type combobox
                    // Type combobox / editable
                    let w_type = widths[2];
                    ui.allocate_ui_with_layout(egui::vec2(w_type,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_type-8.0);
                        let types = ["varchar(255)", "bigint", "int", "text", "longtext", "datetime", "date", "float", "double", "boolean", "enum('a','b')"];
                        
                        // Use a horizontal layout for text edit + picker button
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut tabular.new_column_type).desired_width(w_type - 24.0));
                            
                            egui::ComboBox::from_id_salt("new_col_type_picker")
                                .selected_text("")
                                .width(16.0)
                                .show_ui(ui, |ui| {
                                    for t in types {
                                        if ui.selectable_label(tabular.new_column_type == t, t).clicked() {
                                            tabular.new_column_type = t.to_string();
                                        }
                                    }
                                });
                        });
                    });
                    // Nullable
                    let w_null = widths[3];
                    ui.allocate_ui_with_layout(egui::vec2(w_null,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("new_col_nullable").selected_text(if tabular.new_column_nullable {"YES"} else {"NO"}).show_ui(ui, |ui| {
                            if ui.selectable_label(tabular.new_column_nullable, "YES").clicked() { tabular.new_column_nullable = true; }
                            if ui.selectable_label(!tabular.new_column_nullable, "NO").clicked() { tabular.new_column_nullable = false; }
                        });
                    });
                    // Default
                    let w_def = widths[4];
                    ui.allocate_ui_with_layout(egui::vec2(w_def,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_def-8.0);
                        ui.add_space(4.0);
                        ui.text_edit_singleline(&mut tabular.new_column_default);
                    });
                    // Extra (save/cancel buttons)
                    let w_extra = widths[5];
                    ui.allocate_ui_with_layout(egui::vec2(w_extra,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let save_enabled = !tabular.new_column_name.trim().is_empty();
                        if ui.add_enabled(save_enabled, egui::Button::new("Save")).clicked() { commit_new_column(tabular); }
                        if ui.button("Cancel").clicked() { tabular.adding_column = false; }
                    });
                });
            }

            // EDIT COLUMN ROW (editable)
            if tabular.editing_column {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // # (blank)
                    let (rect_no,_) = ui.allocate_exact_size(egui::vec2(widths[0], row_h), egui::Sense::hover());
                    ui.painter().rect_stroke(rect_no,0.0,stroke, egui::StrokeKind::Outside);
                    // Name
                    let w_name = widths[1];
                    ui.allocate_ui_with_layout(egui::vec2(w_name,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_name-8.0);
                        ui.add_space(4.0);
                        ui.text_edit_singleline(&mut tabular.edit_column_name);
                    });
                    // Type
                    let w_type = widths[2];
                    ui.allocate_ui_with_layout(egui::vec2(w_type,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_type-8.0);
                        ui.text_edit_singleline(&mut tabular.edit_column_type);
                    });
                    // Nullable
                    let w_null = widths[3];
                    ui.allocate_ui_with_layout(egui::vec2(w_null,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        egui::ComboBox::from_id_salt("edit_col_nullable").selected_text(if tabular.edit_column_nullable {"YES"} else {"NO"}).show_ui(ui, |ui| {
                            if ui.selectable_label(tabular.edit_column_nullable, "YES").clicked() { tabular.edit_column_nullable = true; }
                            if ui.selectable_label(!tabular.edit_column_nullable, "NO").clicked() { tabular.edit_column_nullable = false; }
                        });
                    });
                    // Default
                    let w_def = widths[4];
                    ui.allocate_ui_with_layout(egui::vec2(w_def,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_def-8.0);
                        ui.add_space(4.0);
                        ui.text_edit_singleline(&mut tabular.edit_column_default);
                    });
                    // Actions
                    let w_extra = widths[5];
                    ui.allocate_ui_with_layout(egui::vec2(w_extra,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let save_enabled = !tabular.edit_column_name.trim().is_empty() && !tabular.edit_column_type.trim().is_empty();
                        if ui.add_enabled(save_enabled, egui::Button::new("Save")).clicked() { commit_edit_column(tabular); }
                        if ui.button("Cancel").clicked() { tabular.editing_column = false; }
                    });
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
        return;
    };
    let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    else {
        tabular.editing_column = false;
        return;
    };
    let table_name = infer_current_table_name(tabular);
    if table_name.is_empty() {
        tabular.editing_column = false;
        return;
    }

    let old = tabular.edit_column_original_name.trim();
    let new_name = tabular.edit_column_name.trim();
    let new_type = tabular.edit_column_type.trim();
    let nullable = tabular.edit_column_nullable;
    let def = tabular.edit_column_default.trim();

    let mut stmts: Vec<String> = Vec::new();
    match conn.connection_type {
        models::enums::DatabaseType::MySQL => {
            // Build complete column definition with type, nullable, and default
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
                    column_def.push_str(&format!(" DEFAULT '{}'", def.replace("'", "''")));
                }
            }

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
    // Execute sequentially
    if let Some((headers, data)) =
        crate::connection::execute_query_with_connection(tabular, conn_id, full.clone())
    {
        let is_error = headers.first().map(|h| h == "Error").unwrap_or(false);
        if is_error {
            if let Some(row) = data.first()
                && let Some(err) = row.first()
            {
                tabular.error_message = format!("Gagal edit kolom: {}", err);
                tabular.show_error_message = true;
            }
        } else {
            load_structure_info_for_current_table(tabular);
        }
    }
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
    egui::Window::new("Konfirmasi Drop Column")
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .fixed_size(egui::vec2(440.0, 170.0))
        .show(ctx, |ui| {
            ui.label(format!("Column: {}", col_name));
            ui.add_space(4.0);
            ui.code(&stmt);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    tabular.pending_drop_column_name = None;
                    tabular.pending_drop_column_stmt = None;
                }
                if ui
                    .button(
                        egui::RichText::new("Confirm").color(egui::Color32::from_rgb(255, 0, 0)),
                    )
                    .clicked()
                {
                    if let Some(conn_id) = tabular.current_connection_id
                        && !stmt.starts_with("--")
                    {
                        let _ = crate::connection::execute_query_with_connection(
                            tabular,
                            conn_id,
                            stmt.clone(),
                        );
                    }
                    let victim = col_name.clone();
                    tabular.structure_columns.retain(|it| it.name != victim);
                    load_structure_info_for_current_table(tabular);
                    tabular.pending_drop_column_name = None;
                    tabular.pending_drop_column_stmt = None;
                }
            });
        });
}

fn commit_new_column(tabular: &mut window_egui::Tabular) {
    if !tabular.adding_column {
        return;
    }
    let Some(conn_id) = tabular.current_connection_id else {
        tabular.adding_column = false;
        return;
    };
    let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    else {
        tabular.adding_column = false;
        return;
    };
    let table_name = infer_current_table_name(tabular);
    if table_name.is_empty() {
        // Inform user explicitly
        tabular.error_message = "Gagal menambah kolom: nama tabel tidak ditemukan (buka data table atau klik tabel dulu).".to_string();
        tabular.show_error_message = true;
        return;
    }
    let col_name = tabular.new_column_name.trim();
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
            default_clause = format!(" DEFAULT '{}'", d.replace("'", "''"));
        }
    }
    let null_clause = if tabular.new_column_nullable {
        ""
    } else {
        " NOT NULL"
    };

    // DB-specific quoting for identifiers
    let stmt = match conn.connection_type {
        models::enums::DatabaseType::MySQL => format!(
            "ALTER TABLE `{}` ADD COLUMN `{}` {}{}{};",
            table_name, col_name, tabular.new_column_type, null_clause, default_clause
        ),
        models::enums::DatabaseType::PostgreSQL => format!(
            "ALTER TABLE \"{}\" ADD COLUMN \"{}\" {}{}{};",
            table_name, col_name, tabular.new_column_type, null_clause, default_clause
        ),
        models::enums::DatabaseType::MsSQL => format!(
            "ALTER TABLE [{}] ADD [{}] {}{}{};",
            table_name, col_name, tabular.new_column_type, null_clause, default_clause
        ),
        models::enums::DatabaseType::SQLite => format!(
            "ALTER TABLE `{}` ADD COLUMN `{}` {}{}{};",
            table_name, col_name, tabular.new_column_type, null_clause, default_clause
        ),
        _ => "-- Add column not supported for this database type".to_string(),
    };

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

    // Execute and refresh structure on success
    if !stmt.starts_with("--")
        && let Some((headers, data)) =
            crate::connection::execute_query_with_connection(tabular, conn_id, stmt.clone())
    {
        let is_error = headers.first().map(|h| h == "Error").unwrap_or(false);
        if is_error {
            if let Some(first_row) = data.first()
                && let Some(err) = first_row.first()
            {
                tabular.error_message = format!("Gagal menambah kolom: {}", err);
                tabular.show_error_message = true;
            }
        } else {
            // Reload from source to ensure correct view
            load_structure_info_for_current_table(tabular);
        }
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
        tabular.error_message = "Gagal membuat index: nama tabel tidak ditemukan (buka data table atau klik tabel dulu).".to_string();
        tabular.show_error_message = true;
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
    // Auto execute and refresh
    if !stmt.starts_with("--")
        && let Some((headers, data)) =
            crate::connection::execute_query_with_connection(tabular, conn_id, stmt.clone())
    {
        let is_error = headers.first().map(|h| h == "Error").unwrap_or(false);
        if is_error {
            if let Some(first_row) = data.first()
                && let Some(err) = first_row.first()
            {
                tabular.error_message = format!("Gagal CREATE INDEX: {}", err);
                tabular.show_error_message = true;
            }
        } else {
            load_structure_info_for_current_table(tabular);
        }
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
    egui::Window::new("Konfirmasi Drop Index")
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .fixed_size(egui::vec2(420.0, 170.0))
        .show(ctx, |ui| {
            ui.label(format!("Index: {}", idx_name));
            ui.add_space(4.0);
            ui.code(&stmt);
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    tabular.pending_drop_index_name = None;
                    tabular.pending_drop_index_stmt = None;
                }
                if ui
                    .button(
                        egui::RichText::new("Confirm").color(egui::Color32::from_rgb(255, 0, 0)),
                    )
                    .clicked()
                {
                    if let Some(conn_id) = tabular.current_connection_id
                        && !stmt.starts_with("--")
                    {
                        let _ = crate::connection::execute_query_with_connection(
                            tabular,
                            conn_id,
                            stmt.clone(),
                        );
                    }
                    let victim = idx_name.clone();
                    tabular.structure_indexes.retain(|it| it.name != victim);
                    load_structure_info_for_current_table(tabular);
                    tabular.pending_drop_index_name = None;
                    tabular.pending_drop_index_stmt = None;
                }
            });
        });
}

// Handle directory picker dialog

