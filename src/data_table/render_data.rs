use eframe::egui;
use crate::{export, spreadsheet::SpreadsheetOperations, window_egui};
use chrono::Timelike;
use super::{
    initialize_column_widths, get_column_width, set_column_width,
    refresh_current_table_data, infer_current_table_name,
    handle_row_click, handle_column_click,
    copy_selected_block_as_csv, copy_selected_rows_as_csv, copy_selected_columns_as_csv,
    apply_sql_filter, sort_table_data,
    render_pagination_bar,
};
use super::utils::parse_enum_values;

pub(crate) fn render_table_data(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    if !tabular.current_table_headers.is_empty() || !tabular.current_table_name.is_empty() {
        // This function now only renders DATA grid (toggle handled at higher level for table tabs)

        // Show grid whenever we have headers (even if 0 rows) so user sees column structure
        if !tabular.current_table_headers.is_empty() {
            // Toolbar: filter + spreadsheet actions (only in table browse mode)
            if tabular.is_table_browse_mode {
                ui.horizontal(|ui| {
                    // WHERE filter
                    ui.label("WHERE:");
                    let filter_response = ui.add_sized(
                        [ui.available_width() * 0.8, 25.0],
                        egui::TextEdit::singleline(&mut tabular.sql_filter_text)
                            .hint_text("column = 'value' AND col2 > 0")
                            .interactive(true),
                    );

                    if filter_response.has_focus() || filter_response.hovered() {
                        let visuals = ui.visuals();
                        let accent = if filter_response.has_focus() {
                            visuals.selection.stroke.color
                        } else {
                            visuals.widgets.hovered.bg_stroke.color
                        };
                        let rect = filter_response.rect.expand(2.0);
                        ui.painter().rect_stroke(
                            rect,
                            4.0,
                            egui::Stroke::new(1.6, accent),
                            egui::StrokeKind::Outside,
                        );
                    }

                    // Apply filter when:
                    // - Enter is pressed while the field has focus, or
                    // - The field loses focus (more forgiving than requiring `changed()`)
                    // This avoids cases where `lost_focus && changed` misses due to frame timing.
                    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (filter_response.has_focus() && enter_pressed)
                        || filter_response.lost_focus()
                    {
                        apply_sql_filter(tabular);
                    }
                    if ui.button("❌").on_hover_text("Clear filter").clicked() {
                        tabular.sql_filter_text.clear();
                        apply_sql_filter(tabular);
                    }
                    if tabular.spreadsheet_state.is_dirty {
                        ui.separator();
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 0, 0),
                            "Unsaved changes (⌘S)",
                        );
                    }
                });
                ui.separator();
            }

            // Store sort state locally to avoid borrowing issues
            let current_sort_column = tabular.sort_column;
            let current_sort_ascending = tabular.sort_ascending;
            let headers = tabular.current_table_headers.clone();
            let mut sort_requests = Vec::new();
            let mut row_sel_requests: Vec<(usize, egui::Modifiers)> = Vec::new();
            let mut col_sel_requests: Vec<(usize, egui::Modifiers)> = Vec::new();
            let mut cell_sel_requests: Vec<(usize, usize)> = Vec::new();
            let mut select_all_rows_request = false;
            // Defer actions that would mutate tabular during borrow of iter
            let mut start_edit_request: Option<(usize, usize)> = None;
            let mut cell_edit_text_update: Option<String> = None;
            // Defer any column width updates to avoid mut borrow in closures
            let mut deferred_width_updates: Vec<(usize, f32)> = Vec::new();
            // Defer delete-row action to avoid mutable borrow inside UI closures
            let mut delete_row_index_request: Option<usize> = None;
            let mut add_row_request: Option<usize> = None;
            let mut open_csv_import = false;

            // Ensure column widths are initialized
            if tabular.column_widths.len() != headers.len() {
                initialize_column_widths(tabular);
            }

            // If this is an error table (usually 1 column, header contains "error"), set error column width to max
            let mut error_column_index: Option<usize> = None;
            if headers.len() == 1 && headers[0].to_lowercase().contains("error") {
                error_column_index = Some(0);
            } else {
                // If there is a column named "error" (case-insensitive), set its width to max
                for (i, h) in headers.iter().enumerate() {
                    if h.to_lowercase().contains("error") {
                        error_column_index = Some(i);
                        break;
                    }
                }
            }

            // Tata letak: sticky header (32px) + data scroll + pagination bar.
            let avail_h = ui.available_height();
            let pagination_height_est = ui.text_style_height(&egui::TextStyle::Body) + 14.0;
            let total_h = (avail_h - pagination_height_est).max(50.0);
            let header_h = 32.0_f32;
            let data_h = (total_h - header_h).max(20.0);

            // ── Sticky header row ──────────────────────────────────────────────────
            let header_w = ui.available_width();
            let (header_alloc_rect, _) = ui.allocate_exact_size(
                egui::vec2(header_w, header_h),
                egui::Sense::hover(),
            );
            {
                let total_content_w: f32 = 60.0
                    + headers.iter().enumerate().map(|(i, _)| {
                        get_column_width(tabular, i).max(30.0)
                    }).sum::<f32>();
                let content_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        header_alloc_rect.min.x - tabular.data_scroll_x,
                        header_alloc_rect.min.y,
                    ),
                    egui::vec2(total_content_w.max(header_w), header_h),
                );
                let mut hdr_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(content_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                hdr_ui.set_clip_rect(header_alloc_rect);
                hdr_ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                // "No" header cell
                hdr_ui.allocate_ui_with_layout(
                    [60.0, header_h].into(),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        let rect = ui.available_rect_before_wrap();
                        let border_color = if ui.visuals().dark_mode {
                            egui::Color32::from_gray(60)
                        } else {
                            egui::Color32::from_gray(200)
                        };
                        let thin_stroke = egui::Stroke::new(0.5, border_color);
                        let hdr_fill = if ui.visuals().dark_mode {
                            egui::Color32::from_gray(40)
                        } else {
                            egui::Color32::from_gray(240)
                        };
                        ui.painter().rect_filled(rect, 0.0, hdr_fill);
                        ui.painter().line_segment([rect.left_top(), rect.right_top()], thin_stroke);
                        ui.painter().line_segment([rect.right_top(), rect.right_bottom()], thin_stroke);
                        ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], thin_stroke);
                        ui.painter().line_segment([rect.left_bottom(), rect.left_top()], thin_stroke);
                        let text_color = if ui.visuals().dark_mode {
                            egui::Color32::from_rgb(220, 220, 255)
                        } else {
                            egui::Color32::from_rgb(60, 60, 120)
                        };
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "No",
                            egui::FontId::proportional(14.0),
                            text_color,
                        );
                        let resp = ui.allocate_response(rect.size(), egui::Sense::click());
                        if resp.clicked() {
                            select_all_rows_request = true;
                        }
                    },
                );

                // Column header cells
                for (col_index, header) in headers.iter().enumerate() {
                    let column_width = if Some(col_index) == error_column_index {
                        if get_column_width(tabular, col_index) <= 180.0 {
                            set_column_width(tabular, col_index, 600.0);
                        }
                        get_column_width(tabular, col_index).max(100.0)
                    } else {
                        get_column_width(tabular, col_index).max(30.0)
                    };
                    hdr_ui.allocate_ui_with_layout(
                        [column_width, header_h].into(),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let rect = ui.available_rect_before_wrap();
                            let border_color = if ui.visuals().dark_mode {
                                egui::Color32::from_gray(60)
                            } else {
                                egui::Color32::from_gray(200)
                            };
                            let thin_stroke = egui::Stroke::new(0.5, border_color);
                            let hdr_fill = if ui.visuals().dark_mode {
                                egui::Color32::from_gray(40)
                            } else {
                                egui::Color32::from_gray(240)
                            };
                            ui.painter().rect_filled(rect, 0.0, hdr_fill);
                            ui.painter().line_segment([rect.left_top(), rect.right_top()], thin_stroke);
                            ui.painter().line_segment([rect.right_top(), rect.right_bottom()], thin_stroke);
                            ui.painter().line_segment([rect.right_bottom(), rect.left_bottom()], thin_stroke);
                            ui.painter().line_segment([rect.left_bottom(), rect.left_top()], thin_stroke);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let sort_button_width = 45.0;
                                let text_area_width = ui.available_width() - sort_button_width;
                                ui.allocate_ui_with_layout(
                                    [text_area_width, ui.available_height()].into(),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        ui.add(egui::Label::new(
                                            egui::RichText::new(header)
                                                .strong()
                                                .size(14.0)
                                                .color(if ui.visuals().dark_mode {
                                                    egui::Color32::from_rgb(220, 220, 255)
                                                } else {
                                                    egui::Color32::from_rgb(60, 60, 120)
                                                }),
                                        ));
                                    },
                                );
                                let (is_sorted_column, is_asc) =
                                    if current_sort_column == Some(col_index) {
                                        (true, current_sort_ascending)
                                    } else {
                                        (false, false)
                                    };
                                let icon_size = ui.available_height().min(sort_button_width) * 0.6;
                                let (response, painter) = ui.allocate_painter(
                                    egui::vec2(sort_button_width, ui.available_height()),
                                    egui::Sense::click(),
                                );
                                if response.hovered() {
                                    painter.rect_filled(
                                        response.rect.shrink(2.0),
                                        4.0,
                                        if ui.visuals().dark_mode {
                                            egui::Color32::from_white_alpha(10)
                                        } else {
                                            egui::Color32::from_black_alpha(10)
                                        },
                                    );
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                let icon_color = if is_sorted_column {
                                    egui::Color32::from_rgb(255, 0, 0)
                                } else if response.hovered() {
                                    ui.visuals().text_color()
                                } else {
                                    ui.visuals().text_color().gamma_multiply(0.3)
                                };
                                let center = response.rect.center();
                                let half_sz = icon_size * 0.35;
                                if is_sorted_column {
                                    if is_asc {
                                        painter.add(egui::Shape::convex_polygon(
                                            vec![
                                                center + egui::vec2(0.0, -half_sz),
                                                center + egui::vec2(-half_sz, half_sz),
                                                center + egui::vec2(half_sz, half_sz),
                                            ],
                                            icon_color,
                                            egui::Stroke::NONE,
                                        ));
                                    } else {
                                        painter.add(egui::Shape::convex_polygon(
                                            vec![
                                                center + egui::vec2(0.0, half_sz),
                                                center + egui::vec2(-half_sz, -half_sz),
                                                center + egui::vec2(half_sz, -half_sz),
                                            ],
                                            icon_color,
                                            egui::Stroke::NONE,
                                        ));
                                    }
                                } else {
                                    let dash_rect = egui::Rect::from_center_size(
                                        center,
                                        egui::vec2(icon_size * 0.6, icon_size * 0.15),
                                    );
                                    painter.rect_filled(dash_rect, 1.0, icon_color);
                                }
                                if response.clicked() {
                                    let new_ascending = if current_sort_column == Some(col_index) {
                                        !current_sort_ascending
                                    } else {
                                        true
                                    };
                                    sort_requests.push((col_index, new_ascending));
                                }
                                let header_click_rect = egui::Rect::from_min_max(
                                    rect.min,
                                    egui::pos2(
                                        (rect.max.x - sort_button_width).max(rect.min.x),
                                        rect.max.y,
                                    ),
                                );
                                let header_click_resp = ui.interact(
                                    header_click_rect,
                                    egui::Id::new(("col_hdr_s", col_index)),
                                    egui::Sense::click(),
                                );
                                if header_click_resp.clicked() {
                                    let modifiers = ui.input(|i| i.modifiers);
                                    col_sel_requests.push((col_index, modifiers));
                                }
                            });
                            // Resize handle
                            let handle_x = ui.max_rect().max.x;
                            let handle_y = ui.max_rect().min.y;
                            let resize_handle_rect = egui::Rect::from_min_size(
                                egui::pos2(handle_x - 8.0, handle_y),
                                egui::vec2(8.0, header_h),
                            );
                            let resize_response =
                                ui.allocate_rect(resize_handle_rect, egui::Sense::drag());
                            if resize_response.hovered() || resize_response.dragged() {
                                let indicator_color = egui::Color32::from_rgb(255, 0, 0);
                                let dot_size = 1.5;
                                let dot_spacing = 2.0_f32;
                                let start_y = handle_y + 2.0;
                                let end_y = handle_y + header_h - 2.0;
                                for y in (start_y as i32..end_y as i32).step_by(dot_spacing as usize) {
                                    ui.painter().circle_filled(
                                        egui::pos2(handle_x, y as f32),
                                        dot_size,
                                        indicator_color,
                                    );
                                }
                                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                            }
                            if resize_response.dragged() {
                                let new_width = column_width + resize_response.drag_delta().x;
                                deferred_width_updates.push((col_index, new_width));
                            }
                        },
                    );
                }
            }
            // ── Data scroll area ────────────────────────────────────────────────────
            let (data_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), data_h),
                egui::Sense::hover(),
            );
            let mut scroll_child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(data_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            // Defer refresh action to avoid mutable borrow inside UI closures
            let mut refresh_request_data = false;

            // Virtual scroll: only render rows visible in the viewport.
            // Previous frame's scroll offset drives row range — 1-frame lag is imperceptible.
            const ROW_HEIGHT: f32 = 25.0;
            let total_rows = tabular.current_table_data.len();
            let prev_scroll_y = tabular.data_scroll_y;
            let first_row = ((prev_scroll_y / ROW_HEIGHT) as usize).saturating_sub(3);
            let last_row = (((prev_scroll_y + data_h) / ROW_HEIGHT).ceil() as usize + 4).min(total_rows);

            // Pre-compute total content width (matches sticky header formula)
            let total_content_w: f32 = 60.0
                + headers.iter().enumerate()
                    .map(|(i, _)| get_column_width(tabular, i).max(30.0))
                    .sum::<f32>();

            let scroll_out = egui::ScrollArea::both()
                .id_salt("table_data_scroll")
                .horizontal_scroll_offset(tabular.data_scroll_x)
                .auto_shrink([false, false])
                .show(&mut scroll_child, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                    // Establish full content width so horizontal scrollbar is correct
                    ui.set_min_width(total_content_w);

                    // Clone data (borrow checker: tabular fields mutated inside loop)
                    let current_table_data = tabular.current_table_data.clone();
                    let selected_rows = tabular.selected_rows.clone();
                    let selected_row = tabular.selected_row;
                    let newly_created_rows = tabular.newly_created_rows.clone();

                    // Top spacer: allocate space for rows above the viewport
                    if first_row > 0 {
                        ui.add_space(first_row as f32 * ROW_HEIGHT);
                    }

                    for (row_index, row) in current_table_data
                        .iter()
                        .enumerate()
                        .take(last_row)
                        .skip(first_row)
                    {
                        let is_selected_row = selected_rows.contains(&row_index)
                            || selected_row == Some(row_index);
                        let is_newly_created = newly_created_rows.contains(&row_index);

                        let row_color = if is_newly_created {
                            if ui.visuals().dark_mode {
                                egui::Color32::from_rgba_unmultiplied(50, 200, 100, 40)
                            } else {
                                egui::Color32::from_rgba_unmultiplied(150, 255, 180, 100)
                            }
                        } else if is_selected_row {
                            if ui.visuals().dark_mode {
                                egui::Color32::from_rgba_unmultiplied(100, 150, 255, 30)
                            } else {
                                egui::Color32::from_rgba_unmultiplied(200, 220, 255, 80)
                            }
                        } else {
                            egui::Color32::TRANSPARENT
                        };

                        // Each row is a horizontal strip of fixed height
                        ui.allocate_ui_with_layout(
                            egui::vec2(total_content_w, ROW_HEIGHT),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                                let row_rect = ui.max_rect();

                                // Alternating stripe background
                                if row_index % 2 == 1 {
                                    let stripe = if ui.visuals().dark_mode {
                                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 8)
                                    } else {
                                        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 8)
                                    };
                                    ui.painter().rect_filled(row_rect, 0.0, stripe);
                                }
                                // Selection / new-row highlight
                                if row_color != egui::Color32::TRANSPARENT {
                                    ui.painter().rect_filled(row_rect, 3.0, row_color);
                                }

                                // ── Row number cell ──────────────────────────────────
                                ui.allocate_ui_with_layout(
                                    [60.0, ROW_HEIGHT].into(),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        let rect = ui.available_rect_before_wrap();
                                        let border_color = if ui.visuals().dark_mode {
                                            egui::Color32::from_gray(60)
                                        } else {
                                            egui::Color32::from_gray(200)
                                        };
                                        let thin_stroke = egui::Stroke::new(0.5, border_color);
                                        ui.painter().line_segment(
                                            [rect.left_top(), rect.right_top()],
                                            thin_stroke,
                                        );
                                        ui.painter().line_segment(
                                            [rect.right_top(), rect.right_bottom()],
                                            thin_stroke,
                                        );
                                        ui.painter().line_segment(
                                            [rect.right_bottom(), rect.left_bottom()],
                                            thin_stroke,
                                        );
                                        ui.painter().line_segment(
                                            [rect.left_bottom(), rect.left_top()],
                                            thin_stroke,
                                        );
                                        // Draw row number text centered
                                        let text_color = ui.visuals().text_color();
                                        ui.painter().text(
                                            rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            (row_index + 1).to_string(),
                                            egui::FontId::proportional(14.0),
                                            text_color,
                                        );
                                        // Clickable overlay for row selection
                                        let resp =
                                            ui.allocate_response(rect.size(), egui::Sense::click());
                                        if resp.clicked() {
                                            let modifiers = ui.input(|i| i.modifiers);
                                            row_sel_requests.push((row_index, modifiers));
                                        }
                                    },
                                );
                                for (col_index, cell) in row.iter().enumerate() {
                                    let is_selected_cell =
                                        tabular.selected_cell == Some((row_index, col_index));
                                    let is_selected_col =
                                        tabular.selected_columns.contains(&col_index);
                                    let column_width = if Some(col_index) == error_column_index {
                                        get_column_width(tabular, col_index).max(100.0)
                                    } else {
                                        get_column_width(tabular, col_index).max(50.0)
                                    };
                                    ui.allocate_ui_with_layout(
                                        [column_width, ROW_HEIGHT].into(),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let rect = ui.available_rect_before_wrap();
                                            // Column-selection overlay (row highlight already on row_rect)
                                            if is_selected_col {
                                                let overlay = if ui.visuals().dark_mode {
                                                    egui::Color32::from_rgba_unmultiplied(
                                                        100, 255, 150, 20,
                                                    )
                                                } else {
                                                    egui::Color32::from_rgba_unmultiplied(
                                                        100, 200, 150, 40,
                                                    )
                                                };
                                                ui.painter().rect_filled(rect, 0.0, overlay);
                                            }
                                            // Multi-cell block overlay (between anchor and current selected cell)
                                            if let (Some((ar, ac)), Some((br, bc))) =
                                                (tabular.table_sel_anchor, tabular.selected_cell)
                                            {
                                                let rmin = ar.min(br);
                                                let rmax = ar.max(br);
                                                let cmin = ac.min(bc);
                                                let cmax = ac.max(bc);
                                                if row_index >= rmin
                                                    && row_index <= rmax
                                                    && col_index >= cmin
                                                    && col_index <= cmax
                                                {
                                                    let sel_color = if ui.visuals().dark_mode {
                                                        egui::Color32::from_rgba_unmultiplied(
                                                            255, 120, 40, 36,
                                                        )
                                                    } else {
                                                        egui::Color32::from_rgba_unmultiplied(
                                                            255, 140, 60, 70,
                                                        )
                                                    };
                                                    ui.painter().rect_filled(rect, 0.0, sel_color);
                                                }
                                            }
                                            let border_color = if ui.visuals().dark_mode {
                                                egui::Color32::from_gray(60)
                                            } else {
                                                egui::Color32::from_gray(200)
                                            };
                                            let thin_stroke = egui::Stroke::new(0.5, border_color);
                                            ui.painter().line_segment(
                                                [rect.left_top(), rect.right_top()],
                                                thin_stroke,
                                            );
                                            ui.painter().line_segment(
                                                [rect.right_top(), rect.right_bottom()],
                                                thin_stroke,
                                            );
                                            ui.painter().line_segment(
                                                [rect.right_bottom(), rect.left_bottom()],
                                                thin_stroke,
                                            );
                                            ui.painter().line_segment(
                                                [rect.left_bottom(), rect.left_top()],
                                                thin_stroke,
                                            );
                                            if is_selected_cell {
                                                let stroke = egui::Stroke::new(
                                                    2.0,
                                                    egui::Color32::from_rgb(255, 0, 0),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    0.0,
                                                    egui::Color32::from_rgba_unmultiplied(
                                                        255, 60, 10, 20,
                                                    ),
                                                );
                                                ui.painter().line_segment(
                                                    [rect.left_top(), rect.right_top()],
                                                    stroke,
                                                );
                                                ui.painter().line_segment(
                                                    [rect.right_top(), rect.right_bottom()],
                                                    stroke,
                                                );
                                                ui.painter().line_segment(
                                                    [rect.right_bottom(), rect.left_bottom()],
                                                    stroke,
                                                );
                                                ui.painter().line_segment(
                                                    [rect.left_bottom(), rect.left_top()],
                                                    stroke,
                                                );
                                            }
                                            let max_chars =
                                                ((column_width / 8.0).floor() as usize).max(10);
                                            let display_text = if cell.chars().count() > max_chars {
                                                format!(
                                                    "{}...",
                                                    cell.chars()
                                                        .take(max_chars.saturating_sub(3))
                                                        .collect::<String>()
                                                )
                                            } else {
                                                cell.clone()
                                            };
                                            let cell_response = ui.allocate_response(
                                                rect.size(),
                                                egui::Sense::click_and_drag(),
                                            );
                                            
                                            // Log once per frame (approx) - using a simple counter or just log.
                                            // log::debug!("Rendering cell"); // Too spammy
                                            
                                            // DETACHED double click check
                                            cell_response.double_clicked();

                                            // ALLOW EDITING ALWAYS (for custom queries too)
                                            if cell_response.double_clicked() {
                                                // queue edit start to avoid mutable borrow inside iteration

                                                start_edit_request = Some((row_index, col_index));
                                            } else if cell_response.clicked() {

                                                let shift = ui.input(|i| i.modifiers.shift);
                                                if shift {
                                                    if tabular.table_sel_anchor.is_none() {
                                                        tabular.table_sel_anchor = Some(
                                                            tabular
                                                                .selected_cell
                                                                .unwrap_or((row_index, col_index)),
                                                        );
                                                    }
                                                    tabular.selected_cell =
                                                        Some((row_index, col_index));
                                                } else {
                                                    cell_sel_requests.push((row_index, col_index));
                                                    tabular.table_sel_anchor = None;
                                                }
                                            }
                                            // Attach hover text without moving away the response we keep using
                                            let mut cell_resp = cell_response;
                                            if cell.chars().count() > max_chars || !cell.is_empty()
                                            {
                                                cell_resp = cell_resp.on_hover_text(cell);
                                            }
                                            // Drag-to-select lifecycle
                                            if cell_resp.drag_started() {
                                                if tabular.table_sel_anchor.is_none() {
                                                    tabular.table_sel_anchor =
                                                        Some((row_index, col_index));
                                                }
                                                tabular.selected_cell =
                                                    Some((row_index, col_index));
                                                tabular.table_dragging = true;
                                            }
                                            if tabular.table_dragging
                                                && ui.input(|i| i.pointer.primary_down())
                                                && cell_resp.hovered()
                                            {
                                                tabular.selected_cell =
                                                    Some((row_index, col_index));
                                            }
                                            if tabular.table_dragging
                                                && !ui.input(|i| i.pointer.primary_down())
                                            {
                                                tabular.table_dragging = false;
                                            }
                                            // Check if this cell is being edited
                                            let is_editing_this_cell =
                                                tabular.spreadsheet_state.editing_cell
                                                    == Some((row_index, col_index));

                                            if is_editing_this_cell {
                                                // Show TextEdit overlay for editing
                                                let mut text_edit_rect = rect;
                                                text_edit_rect = text_edit_rect.shrink(2.0); // Small margin

                                                // Store cell edit text in a local variable to avoid borrow conflict
                                                let mut edit_text = tabular
                                                    .spreadsheet_state
                                                    .cell_edit_text
                                                    .clone();

                                                    ui.scope_builder(
                                                        egui::UiBuilder::new().max_rect(text_edit_rect),
                                                        |ui| {
                                                            let valid_options = tabular.spreadsheet_state.enum_options.clone();
                                                            // Determine if column is Date/DateTime
                                                            let mut is_date_type = false;
                                                            let mut is_datetime_type = false;
                                                            if let Some(meta) = &tabular.current_column_metadata
                                                                && let Some(col_meta) = meta.get(col_index)
                                                            {
                                                                let t = col_meta.type_name.to_uppercase();
                                                                if t.contains("DATE") && !t.contains("TIME") {
                                                                    is_date_type = true;
                                                                } else if t.contains("DATETIME") || t.contains("TIMESTAMP") {
                                                                    is_datetime_type = true;
                                                                }
                                                            }

                                                            if let Some(options) = valid_options {
                                                                // Render ComboBox for ENUM types
                                                                let mut current_val = edit_text.clone();
                                                                let combo = egui::ComboBox::from_id_salt("enum_combo")
                                                                    .selected_text(&current_val)
                                                                    .height(200.0)
                                                                    .show_ui(ui, |ui| {
                                                                        let mut changed = false;
                                                                        for opt in options {
                                                                             if ui.selectable_value(&mut current_val, opt.clone(), &opt).clicked() {
                                                                                 changed = true;
                                                                             }
                                                                        }
                                                                        changed
                                                                    });
                                                                
                                                                if combo.inner.unwrap_or(false) {
                                                                    edit_text = current_val;
                                                                }
                                                            } else if is_date_type {
                                                                // DATE Picker
                                                                ui.horizontal(|ui| {
                                                                    // jiff Date parses/prints ISO "%Y-%m-%d" natively
                                                                    let mut date_val: jiff::civil::Date = edit_text.parse()
                                                                        .unwrap_or_else(|_| jiff::Zoned::now().date());

                                                                    let changed = ui.add(
                                                                        egui_extras::DatePickerButton::new(&mut date_val)
                                                                            .id_salt("date_picker")
                                                                    ).changed();

                                                                    if changed {
                                                                        edit_text = date_val.to_string();
                                                                    }
                                                                });
                                                            } else if is_datetime_type {
                                                                // DATETIME Picker
                                                                ui.horizontal(|ui| {
                                                                    ui.spacing_mut().item_spacing.x = 4.0;
                                                                    // Parse as NaiveDateTime
                                                                    let mut dt_val = chrono::NaiveDateTime::parse_from_str(&edit_text, "%Y-%m-%d %H:%M:%S")
                                                                        .or_else(|_| chrono::NaiveDateTime::parse_from_str(&edit_text, "%Y-%m-%d %H:%M:%S%.f"))
                                                                        .unwrap_or_else(|_| chrono::Local::now().naive_local());
                                                                    
                                                                    let mut date_part = dt_val.date();
                                                                    // Bridge to jiff just for the picker; chrono stays the source of truth
                                                                    let mut jiff_date: jiff::civil::Date = date_part.to_string().parse()
                                                                        .unwrap_or_else(|_| jiff::Zoned::now().date());
                                                                    // Date Picker
                                                                    if ui.add(
                                                                        egui_extras::DatePickerButton::new(&mut jiff_date)
                                                                            .id_salt("datetime_date_picker")
                                                                    ).changed()
                                                                        && let Ok(new_date) =
                                                                            chrono::NaiveDate::parse_from_str(&jiff_date.to_string(), "%Y-%m-%d")
                                                                    {
                                                                        date_part = new_date;
                                                                        dt_val = chrono::NaiveDateTime::new(date_part, dt_val.time());
                                                                        edit_text = dt_val.format("%Y-%m-%d %H:%M:%S").to_string();
                                                                    }

                                                                    // Time Inputs (H/M/S)
                                                                    let mut h = dt_val.hour();
                                                                    let mut m = dt_val.minute();
                                                                    let mut s = dt_val.second();
                                                                    
                                                                    let dh = ui.add(egui::DragValue::new(&mut h).range(0..=23).suffix("h"));
                                                                    let dm = ui.add(egui::DragValue::new(&mut m).range(0..=59).suffix("m"));
                                                                    let ds = ui.add(egui::DragValue::new(&mut s).range(0..=59).suffix("s"));

                                                                    if (dh.changed() || dm.changed() || ds.changed())
                                                                        && let Some(new_time) =
                                                                            chrono::NaiveTime::from_hms_opt(h, m, s)
                                                                    {
                                                                        dt_val = chrono::NaiveDateTime::new(date_part, new_time);
                                                                        edit_text = dt_val
                                                                            .format("%Y-%m-%d %H:%M:%S")
                                                                            .to_string();
                                                                    }
                                                                });

                                                            } else {
                                                                // Render standard TextEdit
                                                                let text_edit = egui::TextEdit::singleline(
                                                                    &mut edit_text,
                                                                )
                                                                .desired_width(text_edit_rect.width())
                                                                .margin(egui::vec2(2.0, 2.0));

                                                                let response = ui.add(text_edit);

                                                                // Auto-focus the text edit when we start editing
                                                                if !response.has_focus() {
                                                                    response.request_focus();
                                                                }
                                                            }
                                                        },
                                                    );

                                                // Store the updated text to apply later
                                                cell_edit_text_update = Some(edit_text);
                                            } else {
                                                // Show normal cell text
                                                let text_pos = rect.left_top()
                                                    + egui::vec2(5.0, rect.height() * 0.5);
                                                ui.painter().text(
                                                    text_pos,
                                                    egui::Align2::LEFT_CENTER,
                                                    &display_text,
                                                    egui::FontId::default(),
                                                    if is_selected_cell {
                                                        if ui.visuals().dark_mode {
                                                            egui::Color32::WHITE
                                                        } else {
                                                            egui::Color32::BLACK
                                                        }
                                                    } else {
                                                        ui.visuals().text_color()
                                                    },
                                                );
                                            }
                                            cell_resp.context_menu(|ui| {
                                                ui.set_min_width(150.0);
                                                ui.vertical(|ui| {
                                                    if ui.button("🔄 Refresh Data").clicked() {
                                                        refresh_request_data = true;
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    if tabular.is_table_browse_mode
                                                        && ui.button("📋 Add New Row").clicked()
                                                    {
                                                        add_row_request = Some(0);
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    if ui.button("📋 Copy Cell Value").clicked() {
                                                        ui.ctx().copy_text(cell.clone());
                                                        ui.close();
                                                    }
                                                    if tabular.table_sel_anchor.is_some()
                                                        && tabular.selected_cell.is_some()
                                                        && ui
                                                            .button("📄 Copy Selection as CSV")
                                                            .clicked()
                                                    {
                                                        if let (Some(a), Some(b)) = (
                                                            tabular.table_sel_anchor,
                                                            tabular.selected_cell,
                                                        ) && let Some(csv) =
                                                            copy_selected_block_as_csv(
                                                                tabular, a, b,
                                                            )
                                                        {
                                                            ui.ctx().copy_text(csv);
                                                        }
                                                        ui.close();
                                                    }
                                                    if !tabular.selected_rows.is_empty()
                                                        && ui
                                                            .button("📄 Copy Selected Rows as CSV")
                                                            .clicked()
                                                    {
                                                        if let Some(csv) =
                                                            copy_selected_rows_as_csv(tabular)
                                                        {
                                                            ui.ctx().copy_text(csv);
                                                        }
                                                        ui.close();
                                                    }
                                                    if !tabular.selected_columns.is_empty()
                                                        && ui
                                                            .button(
                                                                "📄 Copy Selected Columns as CSV",
                                                            )
                                                            .clicked()
                                                    {
                                                        if let Some(csv) =
                                                            copy_selected_columns_as_csv(tabular)
                                                        {
                                                            ui.ctx().copy_text(csv);
                                                        }
                                                        ui.close();
                                                    }
                                                    if let Some(selected_row_idx) =
                                                        tabular.selected_row
                                                        && ui.button("📄 Copy Row as CSV").clicked()
                                                    {
                                                        if let Some(row_data) = tabular
                                                            .current_table_data
                                                            .get(selected_row_idx)
                                                        {
                                                            let csv_row = row_data
                                                                .iter()
                                                                .map(|cell| {
                                                                    if cell.contains(',')
                                                                        || cell.contains('"')
                                                                        || cell.contains('\n')
                                                                    {
                                                                        format!(
                                                                            "\"{}\"",
                                                                            cell.replace(
                                                                                '"', "\"\""
                                                                            )
                                                                        )
                                                                    } else {
                                                                        cell.clone()
                                                                    }
                                                                })
                                                                .collect::<Vec<_>>()
                                                                .join(",");
                                                            ui.ctx().copy_text(csv_row);
                                                        }
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    if ui.button("📄 Export to CSV").clicked() {
                                                        export::export_to_csv(
                                                            &tabular.all_table_data,
                                                            &tabular.current_table_headers,
                                                            &tabular.current_table_name,
                                                        );
                                                        ui.close();
                                                    }
                                                    if ui.button("📊 Export to XLSX").clicked() {
                                                        export::export_to_xlsx(
                                                            &tabular.all_table_data,
                                                            &tabular.current_table_headers,
                                                            &tabular.current_table_name,
                                                        );
                                                        ui.close();
                                                    }
                                                    if ui.button("🧾 Export to JSON").clicked() {
                                                        export::export_to_json(
                                                            &tabular.all_table_data,
                                                            &tabular.current_table_headers,
                                                            &tabular.current_table_name,
                                                        );
                                                        ui.close();
                                                    }
                                                    if ui.button("📝 Export to Markdown").clicked()
                                                    {
                                                        export::export_to_markdown(
                                                            &tabular.all_table_data,
                                                            &tabular.current_table_headers,
                                                            &tabular.current_table_name,
                                                        );
                                                        ui.close();
                                                    }
                                                    if ui
                                                        .button("🛢 Export as SQL INSERTs")
                                                        .clicked()
                                                    {
                                                        let db_type = tabular
                                                            .current_connection_id
                                                            .and_then(|cid| {
                                                                tabular
                                                                    .connections
                                                                    .iter()
                                                                    .find(|c| c.id == Some(cid))
                                                            })
                                                            .map(|c| c.connection_type.clone());
                                                        export::export_to_sql_inserts(
                                                            &tabular.all_table_data,
                                                            &tabular.current_table_headers,
                                                            &tabular.current_table_name,
                                                            db_type.as_ref(),
                                                        );
                                                        ui.close();
                                                    }
                                                    if tabular.is_table_browse_mode
                                                        && ui.button("📥 Import CSV...").clicked()
                                                    {
                                                        open_csv_import = true;
                                                        ui.close();
                                                    }
                                                    ui.separator();
                                                    if tabular.is_table_browse_mode
                                                        && ui.button("🗑 Delete this Row").clicked()
                                                    {
                                                        // Defer the actual deletion until after the grid borrow ends
                                                        delete_row_index_request = Some(row_index);
                                                        ui.close();
                                                    }
                                                });
                                            });
                                        },
                                    );
                                }
                            }, // end row allocate_ui_with_layout
                        ); // end row wrapper
                    } // end for row_index in first_row..last_row

                    // Bottom spacer: allocate space for rows below the viewport
                    if last_row < total_rows {
                        ui.add_space((total_rows - last_row) as f32 * ROW_HEIGHT);
                    }

                    // Context menu on the scroll area background
                    let bg_resp = ui.interact(
                        ui.min_rect(),
                        egui::Id::new("table_data_bg"),
                        egui::Sense::hover(),
                    );
                    bg_resp.context_menu(|ui| {
                        ui.set_min_width(150.0);
                        ui.vertical(|ui| {
                            if ui.button("🔄 Refresh Data").clicked() {
                                refresh_request_data = true;
                                ui.close();
                            }
                            if tabular.table_sel_anchor.is_some()
                                && tabular.selected_cell.is_some()
                                && ui.button("📋 Copy Selection as CSV").clicked()
                            {
                                if let (Some(a), Some(b)) =
                                    (tabular.table_sel_anchor, tabular.selected_cell)
                                    && let Some(csv) = copy_selected_block_as_csv(tabular, a, b)
                                {
                                    ui.ctx().copy_text(csv);
                                }
                                ui.close();
                            }
                            if ui.button("📄 Export to CSV").clicked() {
                                export::export_to_csv(
                                    &tabular.all_table_data,
                                    &tabular.current_table_headers,
                                    &tabular.current_table_name,
                                );
                                ui.close();
                            }
                            if ui.button("📊 Export to XLSX").clicked() {
                                export::export_to_xlsx(
                                    &tabular.all_table_data,
                                    &tabular.current_table_headers,
                                    &tabular.current_table_name,
                                );
                                ui.close();
                            }
                            if ui.button("🧾 Export to JSON").clicked() {
                                export::export_to_json(
                                    &tabular.all_table_data,
                                    &tabular.current_table_headers,
                                    &tabular.current_table_name,
                                );
                                ui.close();
                            }
                            if ui.button("📝 Export to Markdown").clicked() {
                                export::export_to_markdown(
                                    &tabular.all_table_data,
                                    &tabular.current_table_headers,
                                    &tabular.current_table_name,
                                );
                                ui.close();
                            }
                            if ui.button("🛢 Export as SQL INSERTs").clicked() {
                                let db_type = tabular
                                    .current_connection_id
                                    .and_then(|cid| {
                                        tabular.connections.iter().find(|c| c.id == Some(cid))
                                    })
                                    .map(|c| c.connection_type.clone());
                                export::export_to_sql_inserts(
                                    &tabular.all_table_data,
                                    &tabular.current_table_headers,
                                    &tabular.current_table_name,
                                    db_type.as_ref(),
                                );
                                ui.close();
                            }
                            if tabular.is_table_browse_mode
                                && ui.button("📥 Import CSV...").clicked()
                            {
                                open_csv_import = true;
                                ui.close();
                            }
                            ui.separator();
                            if !tabular.selected_rows.is_empty()
                                && ui.button("📋 Copy Selected Rows as CSV").clicked()
                            {
                                if let Some(csv) = copy_selected_rows_as_csv(tabular) {
                                    ui.ctx().copy_text(csv);
                                }
                                ui.close();
                            }
                            if !tabular.selected_columns.is_empty()
                                && ui.button("📋 Copy Selected Columns as CSV").clicked()
                            {
                                if let Some(csv) = copy_selected_columns_as_csv(tabular) {
                                    ui.ctx().copy_text(csv);
                                }
                                ui.close();
                            }
                        });
                    });

                    // Scroll to selected cell — computed geometrically so it works
                    // even when the target cell is outside the rendered viewport.
                    if tabular.scroll_to_selected_cell
                        && let Some((sel_row, sel_col)) = tabular.selected_cell {
                            let col_x: f32 = 60.0
                                + (0..sel_col)
                                    .map(|i| get_column_width(tabular, i).max(50.0))
                                    .sum::<f32>();
                            let col_w = get_column_width(tabular, sel_col).max(50.0);
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(col_x, sel_row as f32 * ROW_HEIGHT),
                                egui::vec2(col_w, ROW_HEIGHT),
                            );
                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                        }
                });
            // Sync scroll offsets: x for sticky header, y for next-frame virtual scroll
            tabular.data_scroll_x = scroll_out.state.offset.x;
            tabular.data_scroll_y = scroll_out.state.offset.y;
            // Execute deferred refresh after UI borrows are released
            if refresh_request_data {
                refresh_current_table_data(tabular);
            }
            // If editing a cell, support keyboard-only editing/navigation
            if let Some((erow, ecol)) = tabular.spreadsheet_state.editing_cell {
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let right = ui.input(|i| i.key_pressed(egui::Key::ArrowRight));
                let left = ui.input(|i| i.key_pressed(egui::Key::ArrowLeft));
                let down = ui.input(|i| i.key_pressed(egui::Key::ArrowDown));
                let up = ui.input(|i| i.key_pressed(egui::Key::ArrowUp));

                // Enter/Escape behavior (commit/cancel)
                if enter {
                    // Apply in-flight text from the overlay before committing
                    if let Some(new_text) = cell_edit_text_update.take() {
                        tabular.spreadsheet_state.cell_edit_text = new_text;
                    }
                    tabular.spreadsheet_finish_cell_edit(true);
                } else if esc {
                    tabular.spreadsheet_finish_cell_edit(false);
                }

                // Arrow key navigation while editing: commit current and move edit focus
                let mut target: Option<(usize, usize)> = None;
                if right {
                    if let Some(row_vec) = tabular.current_table_data.get(erow)
                        && ecol + 1 < row_vec.len()
                    {
                        target = Some((erow, ecol + 1));
                    }
                } else if left {
                    if ecol > 0 {
                        target = Some((erow, ecol - 1));
                    }
                } else if down {
                    if erow + 1 < tabular.current_table_data.len()
                        && let Some(next_row) = tabular.current_table_data.get(erow + 1)
                    {
                        let tcol = ecol.min(next_row.len().saturating_sub(1));
                        target = Some((erow + 1, tcol));
                    }
                } else if up
                    && erow > 0
                    && let Some(prev_row) = tabular.current_table_data.get(erow - 1)
                {
                    let tcol = ecol.min(prev_row.len().saturating_sub(1));
                    target = Some((erow - 1, tcol));
                }

                if let Some((tr, tc)) = target {
                    // Apply in-flight overlay text and commit current edit before moving
                    if let Some(new_text) = cell_edit_text_update.take() {
                        tabular.spreadsheet_state.cell_edit_text = new_text;
                    }
                    tabular.spreadsheet_finish_cell_edit(true);
                    tabular.selected_row = Some(tr);
                    tabular.selected_cell = Some((tr, tc));
                    tabular.table_recently_clicked = true;
                    tabular.scroll_to_selected_cell = true;
                    tabular.spreadsheet_start_cell_edit(tr, tc);
                }
            }
            // First, apply any live text changes captured from the TextEdit overlay
            // so that if we switch to a different cell, the previous cell's final text is preserved.
            if let Some(new_text) = cell_edit_text_update.take() {
                tabular.spreadsheet_state.cell_edit_text = new_text;
            }

            // Apply deferred selection changes after UI borrow ends
            if select_all_rows_request {
                if tabular.selected_rows.len() == tabular.current_table_data.len() {
                    tabular.selected_rows.clear();
                } else {
                    tabular.selected_rows.clear();
                    for r in 0..tabular.current_table_data.len() {
                        tabular.selected_rows.insert(r);
                    }
                }
            }
            for (row_idx, modifiers) in row_sel_requests {
                handle_row_click(tabular, row_idx, modifiers);
            }
            for (col_idx, modifiers) in col_sel_requests {
                handle_column_click(tabular, col_idx, modifiers);
                tabular.table_sel_anchor = None;
                tabular.table_dragging = false;
            }

            // Handle right-click context menu for selected row
            if tabular.is_table_browse_mode && tabular.selected_row.is_some() {
                // Check for right-click separately to avoid conflict with any_click detection
                let (should_show_menu, pointer_pos) = ui.input(|i| {
                    (
                        i.pointer.secondary_clicked(),
                        i.pointer.hover_pos().unwrap_or(egui::Pos2::ZERO),
                    )
                });

                if should_show_menu && !tabular.show_row_context_menu {
                    tabular.show_row_context_menu = true;
                    tabular.context_menu_row = tabular.selected_row;
                    tabular.context_menu_just_opened = true;
                    tabular.context_menu_pos = pointer_pos; // Save the position when menu opens
                }
            }
            if let Some((r, c)) = cell_sel_requests.last().copied() {
                tabular.selected_row = Some(r);
                tabular.selected_cell = Some((r, c));
                tabular.table_sel_anchor = None;
                tabular.table_dragging = false;
                tabular.table_recently_clicked = true; // Mark that table was clicked
            }
            if let Some((r, c)) = start_edit_request.take() {
                // If we're switching from one editing cell to another, commit the previous edit first
                if tabular.spreadsheet_state.editing_cell.is_some()
                    && tabular.spreadsheet_state.editing_cell != Some((r, c))
                {
                    tabular.scroll_to_selected_cell = true;
                    tabular.spreadsheet_finish_cell_edit(true);
                }
                tabular.selected_row = Some(r);
                tabular.selected_cell = Some((r, c));
                tabular.table_recently_clicked = true;
                
                tabular.spreadsheet_start_cell_edit(r, c);

                // Fetch ENUM options if applicable
                tabular.spreadsheet_state.enum_options = None;
                if let Some(conn_id) = tabular.current_connection_id {
                     // Check if we have precise metadata for this column (from query result)
                     // This allows ENUM lookup even for complex queries or when table name isn't in the tab title
                     let mut type_might_be_enum = false;
                     let table_name = if let Some(meta) = &tabular.current_column_metadata
                        && let Some(col_meta) = meta.get(c)
                     {
                         if let Some(t_name) = &col_meta.table_name && !t_name.is_empty() {
                             // Check type name if available
                             let t_type = col_meta.type_name.to_lowercase();
                             if t_type.contains("enum") || t_type.contains("set") {
                                 type_might_be_enum = true;
                             }
                             // If basic type is unknown or weird, we might want to check anyway? 
                             // But usually sqlx gives "VARCHAR" for enums in MySQL sometimes? 
                             // Wait, if sqlx gives "VARCHAR", then we WON'T detect it here, and we WON'T trigger cache miss.
                             // That means we might miss ENUM dropdowns.
                             // BUT user specifically said: "Harusnya ini muncul kalo ada kolom yang di cache belum jelas tipedatanya"
                             // (This should appear if there is a column in cache with unclear datatype).
                             // If sqlx says "ENUM", we MUST check cache.
                             // If sqlx says "VARCHAR", we ignore. That seems safe for now to stop the annoyance.
                             
                             t_name.clone()
                         } else {
                             // fallback
                             infer_current_table_name(tabular)
                         }
                     } else {
                         infer_current_table_name(tabular)
                     };
                     
                     // If we inferred table name but skipped metadata check (e.g. no metadata), assume we might need to check?
                     // No, let's be conservative. If we don't know it's an ENUM, don't popup.
                     // Exception: If we have NO metadata (e.g. browsing a table directly?), we rely on cache entirely.
                     // In table browse mode, we usually don't have types in `current_column_metadata`? 
                     // Actually `execute_mysql_query_job` populates it.
                     // Let's assume `type_might_be_enum` is the gate.
                     
                     if !table_name.is_empty() && type_might_be_enum {
                         let clean_table = table_name.trim_matches(|c| c == '`' || c == '"' || c == '\'');
                         let db_name = tabular.query_tabs.get(tabular.active_tab_index)
                                         .and_then(|t| t.database_name.clone())
                                         .unwrap_or_default();
                         if let (Some(cols), Some(col_name)) = (crate::cache_data::get_columns_from_cache(tabular, conn_id, &db_name, clean_table), tabular.current_table_headers.get(c)) {
                                 if cols.is_empty() {
                                     tabular.cache_miss_request = Some((conn_id, db_name.clone(), clean_table.to_string()));
                                 } else {
                                     if let Some((_, type_str)) = cols.iter().find(|(name, _)| name == col_name) {
                                         let lower_type = type_str.to_lowercase();
                                         if lower_type.starts_with("enum") || lower_type.starts_with("set") {
                                              tabular.spreadsheet_state.enum_options = parse_enum_values(type_str);
                                         }
                                     }
                                 }
                         } else {
                             tabular.cache_miss_request = Some((conn_id, db_name.clone(), clean_table.to_string()));
                         }
                     }
                }
            }
            // (Cell edit text updates already applied above before changing edit target)

            // Open CSV import dialog for the current table
            if open_csv_import
                && let Some(conn_id) = tabular.current_connection_id
                    && let Some(conn) = tabular.connections.iter().find(|c| c.id == Some(conn_id)) {
                        let db_type = conn.connection_type.clone();
                        // Extract bare table name (strip "Table: " prefix if present)
                        let raw = tabular.current_table_name.trim();
                        let table_name = raw.strip_prefix("Table:").map(str::trim).unwrap_or(raw).to_string();
                        // Use current database from cache_miss_request context or best-effort
                        // Walk items_tree recursively to find the database_name for this table
                        fn find_db_name(
                            nodes: &[crate::models::structs::TreeNode],
                            conn_id: i64,
                            table: &str,
                        ) -> Option<String> {
                            for n in nodes {
                                if n.connection_id == Some(conn_id)
                                    && n.table_name.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(table))
                                    && n.database_name.is_some()
                                {
                                    return n.database_name.clone();
                                }
                                if let Some(found) = find_db_name(&n.children, conn_id, table) {
                                    return Some(found);
                                }
                            }
                            None
                        }
                        let database_name: Option<String> =
                            find_db_name(&tabular.items_tree, conn_id, &table_name);
                        let table_cols: Vec<String> = database_name.as_deref()
                            .and_then(|db| crate::cache_data::get_columns_from_cache(tabular, conn_id, db, &table_name))
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(name, _)| name)
                            .collect();
                        tabular.csv_import_state = Some(crate::models::structs::CsvImportState {
                            connection_id: conn_id,
                            database_name,
                            table_name,
                            db_type,
                            file_path: None,
                            delimiter: ',',
                            has_header_row: true,
                            null_value: String::new(),
                            preview_headers: vec![],
                            preview_rows: vec![],
                            table_columns: table_cols,
                            column_mappings: vec![],
                            status: crate::models::structs::CsvImportStatus::Idle,
                            progress_message: String::new(),
                        });
                        tabular.show_csv_import_dialog = true;
                    }

            // Perform deferred delete after UI borrows are released
            if let Some(ri) = delete_row_index_request.take() {
                // Ensure the row intended for deletion is selected, then delete
                tabular.selected_row = Some(ri);
                tabular.spreadsheet_delete_selected_row();
            }

            if let Some(_ri) = add_row_request.take() {
                tabular.spreadsheet_add_row();
            }

            for (column_index, ascending) in sort_requests {
                sort_table_data(tabular, column_index, ascending);
            }
            // Apply any deferred column width updates now
            for (ci, w) in deferred_width_updates {
                set_column_width(tabular, ci, w);
            }

            // Reset scroll request flag after attempting scroll inside the ScrollArea
            if tabular.scroll_to_selected_cell {
                tabular.scroll_to_selected_cell = false;
            }
            // If there are no rows, display an explicit message under the header grid
            // if tabular.current_table_data.is_empty() {
            //     ui.add_space(4.0);
            //     ui.label(egui::RichText::new("0 rows").italics().weak());
            // }

            // (Pagination dipindahkan & kini dirender terpisah secara universal di akhir fungsi)
        } else if tabular.current_table_name.starts_with("Failed") {
            ui.colored_label(
                egui::Color32::from_rgb(255, 0, 0),
                &tabular.current_table_name,
            );
        } else {
            // Tampilkan header & pagination walaupun tidak ada data
            // Ambil header dari tab aktif bila current_table_headers kosong
            if tabular.current_table_headers.is_empty()
                && let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index)
                && !tab.result_headers.is_empty()
            {
                tabular.current_table_headers = tab.result_headers.clone();
            }

            if !tabular.current_table_headers.is_empty() {
                // Render grid header tanpa rows
                egui::ScrollArea::both().show(ui, |ui| {
                    egui::Grid::new("empty_result_headers")
                        .striped(true)
                        .show(ui, |ui| {
                            for h in &tabular.current_table_headers {
                                ui.label(egui::RichText::new(h).strong());
                            }
                            ui.end_row();
                        });
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("0 rows").italics().weak());
                });
            } else {
                // Fallback asli kalau benar-benar tidak ada header
                ui.label("No data available - No Header available");
            }
        }
        // Pagination universal: jika belum ada header sama sekali tampilkan placeholder info sebelum bar
        if tabular.current_table_headers.is_empty() {
            ui.label(
                egui::RichText::new("No columns loaded yet")
                    .italics()
                    .weak(),
            );
        }
        // Pagination bar sticky: sudah di luar area scroll jadi otomatis menempel bawah container
        render_pagination_bar(tabular, ui);
    }
}

// Helper baru: render pagination bar (dipakai baik ada data maupun kosong)
