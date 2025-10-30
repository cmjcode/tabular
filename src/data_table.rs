use eframe::egui;
use log::{debug, info};

use crate::{
    connection, driver_mssql, export, models, spreadsheet::SpreadsheetOperations, window_egui,
};

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
                            .hint_text("column = 'value' AND col2 > 0"),
                    );
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
                        ui.colored_label(egui::Color32::RED, "Unsaved changes (⌘S)");
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

            // Tata letak: sisakan ruang khusus pagination supaya tidak tertimpa / tersembunyi walau data kosong.
            // Pendekatan: alokasikan area scroll dengan tinggi pasti (avail - pagination_est) lalu render bar di bawahnya.
            let avail_h = ui.available_height();
            let pagination_height_est = ui.text_style_height(&egui::TextStyle::Body) + 14.0;
            let scroll_h = (avail_h - pagination_height_est).max(50.0);
            let (scroll_rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), scroll_h),
                egui::Sense::hover(),
            );
            let mut scroll_child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(scroll_rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            );
            // Defer refresh action to avoid mutable borrow inside UI closures
            let mut refresh_request_data = false;
            let _scroll_area_response = egui::ScrollArea::both()
                .id_salt("table_data_scroll")
                .auto_shrink([false, false])
                .show(&mut scroll_child, |ui| {
                    // Capture target rect of the selected cell during layout
                    let mut target_cell_rect: Option<egui::Rect> = None;
                    let grid_response = egui::Grid::new("table_data_grid")
                        .striped(true)
                        .spacing([0.0, 0.0])
                        .min_col_width(0.0)
                        .max_col_width(f32::INFINITY)
                        .show(ui, |ui| {
                            // Render No column header first (centered) - clicking here selects all rows
                            ui.allocate_ui_with_layout(
                                [60.0, ui.available_height().max(30.0)].into(),
                                egui::Layout::left_to_right(egui::Align::Center),
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
                                    // Draw text
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
                                    // Clickable overlay
                                    let resp =
                                        ui.allocate_response(rect.size(), egui::Sense::click());
                                    if resp.clicked() {
                                        select_all_rows_request = true;
                                    }
                                },
                            );

                            // Render enhanced headers with sort buttons and resize handles
                            for (col_index, header) in headers.iter().enumerate() {
                                // For error columns, use a larger default width but still allow resizing
                                let column_width = if Some(col_index) == error_column_index {
                                    // If this is the first time we see an error column, set a larger default width
                                    if get_column_width(tabular, col_index) <= 180.0 {
                                        // Default width
                                        set_column_width(tabular, col_index, 600.0); // Set larger default for error columns
                                    }
                                    get_column_width(tabular, col_index).max(100.0)
                                } else {
                                    get_column_width(tabular, col_index).max(30.0)
                                };
                                let available_height = ui.available_height().max(30.0);

                                ui.allocate_ui_with_layout(
                                    [column_width, available_height].into(),
                                    egui::Layout::left_to_right(egui::Align::Center),
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
                                        ui.horizontal(|ui| {
                                            let sort_button_width = 25.0;
                                            let text_area_width =
                                                ui.available_width() - sort_button_width;
                                            ui.allocate_ui_with_layout(
                                                [text_area_width, ui.available_height()].into(),
                                                egui::Layout::top_down(egui::Align::Center),
                                                |ui| {
                                                    ui.add(egui::Label::new(
                                                        egui::RichText::new(header)
                                                            .strong()
                                                            .size(14.0)
                                                            .color(if ui.visuals().dark_mode {
                                                                egui::Color32::from_rgb(
                                                                    220, 220, 255,
                                                                )
                                                            } else {
                                                                egui::Color32::from_rgb(60, 60, 120)
                                                            }),
                                                    ));
                                                },
                                            );
                                            let (sort_icon, is_active) =
                                                if current_sort_column == Some(col_index) {
                                                    if current_sort_ascending {
                                                        ("^", true)
                                                    } else {
                                                        ("v", true)
                                                    }
                                                } else {
                                                    ("-", false)
                                                };
                                            let sort_button = ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new(sort_icon)
                                                        .size(12.0)
                                                        .color(if is_active {
                                                            egui::Color32::from_rgb(100, 150, 255)
                                                        } else {
                                                            egui::Color32::GRAY
                                                        }),
                                                )
                                                .small()
                                                .fill(if is_active {
                                                    egui::Color32::from_rgba_unmultiplied(
                                                        100, 150, 255, 50,
                                                    )
                                                } else {
                                                    egui::Color32::TRANSPARENT
                                                }),
                                            );
                                            if sort_button.clicked() {
                                                let new_ascending =
                                                    if current_sort_column == Some(col_index) {
                                                        !current_sort_ascending
                                                    } else {
                                                        true
                                                    };
                                                sort_requests.push((col_index, new_ascending));
                                            }
                                            // Click on empty header area (excluding sort button) to multi-select columns
                                            // Avoid overlapping the sort button so it remains clickable even when selecting columns
                                            let header_click_rect = egui::Rect::from_min_max(
                                                rect.min,
                                                egui::pos2(
                                                    (rect.max.x - sort_button_width)
                                                        .max(rect.min.x),
                                                    rect.max.y,
                                                ),
                                            );
                                            let header_click_resp = ui.interact(
                                                header_click_rect,
                                                egui::Id::new(("col_hdr", col_index)),
                                                egui::Sense::click(),
                                            );
                                            if header_click_resp.clicked() {
                                                let modifiers = ui.input(|i| i.modifiers);
                                                col_sel_requests.push((col_index, modifiers));
                                            }
                                        });
                                        // Add resize handle for all columns, including the last (rightmost) one
                                        // so users can resize the final column as well.
                                        let handle_x = ui.max_rect().max.x;
                                        let handle_y = ui.max_rect().min.y;
                                        let handle_height = available_height;
                                        let resize_handle_rect = egui::Rect::from_min_size(
                                            egui::pos2(handle_x - 3.0, handle_y),
                                            egui::vec2(6.0, handle_height),
                                        );
                                        let resize_response = ui
                                            .allocate_rect(resize_handle_rect, egui::Sense::drag());

                                        // Always show a subtle resize indicator
                                        let indicator_color = if resize_response.hovered()
                                            || resize_response.dragged()
                                        {
                                            egui::Color32::from_rgba_unmultiplied(
                                                100, 150, 255, 200,
                                            )
                                        } else if ui.visuals().dark_mode {
                                            egui::Color32::from_rgba_unmultiplied(120, 120, 120, 80)
                                        } else {
                                            egui::Color32::from_rgba_unmultiplied(150, 150, 150, 60)
                                        };

                                        // Draw the resize handle with dotted pattern
                                        let center_x = handle_x - 1.5;
                                        let dot_size = 1.0;
                                        let dot_spacing = 4.0;
                                        let start_y = handle_y + 8.0;
                                        let end_y = handle_y + handle_height - 8.0;

                                        for y in (start_y as i32..end_y as i32)
                                            .step_by(dot_spacing as usize)
                                        {
                                            ui.painter().circle_filled(
                                                egui::pos2(center_x, y as f32),
                                                dot_size,
                                                indicator_color,
                                            );
                                        }

                                        if resize_response.hovered() {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::ResizeColumn);
                                        }
                                        if resize_response.dragged() {
                                            let delta_x = resize_response.drag_delta().x;
                                            let new_width = column_width + delta_x;
                                            deferred_width_updates.push((col_index, new_width));
                                        }
                                    },
                                );
                            }
                            ui.end_row();

                            // Render data rows with row numbers
                            let current_table_data = tabular.current_table_data.clone();
                            let selected_rows = tabular.selected_rows.clone();
                            let selected_row = tabular.selected_row;
                            let error_column_index = error_column_index;
                            let newly_created_rows = tabular.newly_created_rows.clone();

                            for (row_index, row) in current_table_data.iter().enumerate() {
                                let is_selected_row = selected_rows.contains(&row_index)
                                    || selected_row == Some(row_index);
                                let is_newly_created = newly_created_rows.contains(&row_index);

                                let row_color = if is_newly_created {
                                    // Green highlight for newly created/duplicated rows
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
                                ui.allocate_ui_with_layout(
                                    [60.0, ui.available_height().max(25.0)].into(),
                                    egui::Layout::top_down(egui::Align::Center),
                                    |ui| {
                                        let rect = ui.available_rect_before_wrap();
                                        if row_color != egui::Color32::TRANSPARENT {
                                            ui.painter().rect_filled(rect, 3.0, row_color);
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
                                    let cell_height = ui.available_height().max(25.0);
                                    ui.allocate_ui_with_layout(
                                        [column_width, cell_height].into(),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            let rect = ui.available_rect_before_wrap();
                                            if row_color != egui::Color32::TRANSPARENT
                                                || is_selected_col
                                            {
                                                ui.painter().rect_filled(rect, 3.0, row_color);
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
                                                    egui::Color32::from_rgb(255, 60, 0),
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
                                            // If this is the selected cell and a scroll has been requested,
                                            // remember the exact rect so we can scroll to it after the grid is laid out.
                                            if is_selected_cell && tabular.scroll_to_selected_cell {
                                                target_cell_rect = Some(rect);
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
                                            if tabular.is_table_browse_mode
                                                && cell_response.double_clicked()
                                            {
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
                                ui.end_row();
                            }
                        });
                    grid_response.response.context_menu(|ui| {
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

                    // After laying out the grid, perform the scroll to the selected cell (if requested)
                    if tabular.scroll_to_selected_cell {
                        if let Some(rect) = target_cell_rect {
                            log::debug!(
                                "🔎 scroll_to_rect -> min=({:.1},{:.1}) max=({:.1},{:.1})",
                                rect.min.x,
                                rect.min.y,
                                rect.max.x,
                                rect.max.y
                            );
                            // Center the cell in view for both axes
                            ui.scroll_to_rect(rect, Some(egui::Align::Center));
                        } else {
                            log::debug!(
                                "⚠️ scroll_to_selected_cell set but no target rect found this frame"
                            );
                        }
                    }
                });
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
            }
            // (Cell edit text updates already applied above before changing edit target)

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
            ui.colored_label(egui::Color32::RED, &tabular.current_table_name);
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
pub(crate) fn render_pagination_bar(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    // Tidak ada extra space supaya bar menempel konten / tepi bawah
    ui.horizontal(|ui| {
        let spacing = ui.spacing_mut();
        spacing.item_spacing.x = 4.0;
        spacing.button_padding = egui::vec2(6.0, 2.0);

        if tabular.use_server_pagination && tabular.actual_total_rows.is_some() {
            let actual_total = tabular.actual_total_rows.unwrap_or(0);
            if actual_total > 0 {
                let start_row = tabular.current_page * tabular.page_size + 1;
                let end_row = ((tabular.current_page + 1) * tabular.page_size).min(actual_total);
                ui.label(format!("Showing rows {}-{}", start_row, end_row));
            } else {
                ui.label("0 rows");
            }
            ui.colored_label(egui::Color32::GREEN, "📡 Server pagination");
        } else {
            ui.label(format!("Total rows: {}", tabular.total_rows));
            if !tabular.use_server_pagination {
                ui.colored_label(egui::Color32::YELLOW, "💾 Client pagination");
            }
        }
        ui.separator();

        // Page size selector
        ui.label("Rows per page:");
        let mut page_size_str = tabular.page_size.to_string();
        // Batasi lebar input supaya tidak mengembang mengisi bar dan membuat gap
        if ui
            .add(egui::TextEdit::singleline(&mut page_size_str).desired_width(60.0))
            .changed()
            && let Ok(new_size) = page_size_str.parse::<usize>()
            && new_size > 0
            && new_size <= 10000
        {
            tabular.set_page_size(new_size);
        }

        ui.separator();

        // Navigation buttons
        let has_data = if tabular.use_server_pagination {
            tabular.actual_total_rows.unwrap_or(0) > 0
        } else {
            tabular.total_rows > 0
        };
        let total_pages = if tabular.use_server_pagination {
            get_total_pages_server(tabular)
        } else {
            get_total_pages(tabular)
        };

        ui.add_enabled(
            has_data && tabular.current_page > 0,
            egui::Button::new("⏮ First"),
        )
        .clicked()
        .then(|| go_to_page(tabular, 0));
        ui.add_enabled(
            has_data && tabular.current_page > 0,
            egui::Button::new("◀ Prev"),
        )
        .clicked()
        .then(|| previous_page(tabular));
        ui.label(format!(
            "Page {} of {}",
            tabular.current_page + 1,
            total_pages.max(1)
        ));
        ui.add_enabled(
            has_data && tabular.current_page < total_pages.saturating_sub(1),
            egui::Button::new("Next ▶"),
        )
        .clicked()
        .then(|| next_page(tabular));
        ui.add_enabled(has_data && total_pages > 1, egui::Button::new("Last ⏭"))
            .clicked()
            .then(|| {
                let last_page = total_pages.saturating_sub(1);
                go_to_page(tabular, last_page);
            });

        ui.separator();
        if ui.button("Clear selection").clicked() {
            tabular.selected_rows.clear();
            tabular.selected_columns.clear();
            tabular.selected_row = None;
            tabular.selected_cell = None;
            tabular.last_clicked_row = None;
            tabular.last_clicked_column = None;
        }

        ui.label("Go to page:");
        let mut page_input = (tabular.current_page + 1).to_string();
        if ui
            .add_enabled(has_data, egui::TextEdit::singleline(&mut page_input))
            .changed()
            && let Ok(page_num) = page_input.parse::<usize>()
            && page_num > 0
        {
            go_to_page(tabular, page_num - 1);
        }
    });
}

pub(crate) fn sort_table_data(
    tabular: &mut window_egui::Tabular,
    column_index: usize,
    ascending: bool,
) {
    if column_index >= tabular.current_table_headers.len() || tabular.all_table_data.is_empty() {
        return;
    }

    // Update sort state
    tabular.sort_column = Some(column_index);
    tabular.sort_ascending = ascending;

    // Sort ALL the data (not just current page)
    tabular.all_table_data.sort_by(|a, b| {
        if column_index >= a.len() || column_index >= b.len() {
            return std::cmp::Ordering::Equal;
        }

        let cell_a = &a[column_index];
        let cell_b = &b[column_index];

        // Handle NULL or empty values (put them at the end)
        let comparison = match (cell_a.as_str(), cell_b.as_str()) {
            ("NULL", "NULL") | ("", "") => std::cmp::Ordering::Equal,
            ("NULL", _) | ("", _) => std::cmp::Ordering::Greater,
            (_, "NULL") | (_, "") => std::cmp::Ordering::Less,
            (a_val, b_val) => {
                // Try to parse as numbers first for better numeric sorting
                match (a_val.parse::<f64>(), b_val.parse::<f64>()) {
                    (Ok(num_a), Ok(num_b)) => num_a
                        .partial_cmp(&num_b)
                        .unwrap_or(std::cmp::Ordering::Equal),
                    _ => {
                        // Fall back to string comparison (case-insensitive)
                        a_val.to_lowercase().cmp(&b_val.to_lowercase())
                    }
                }
            }
        };

        if ascending {
            comparison
        } else {
            comparison.reverse()
        }
    });

    // Update current page data after sorting
    update_current_page_data(tabular);

    let sort_direction = if ascending {
        "^ ascending"
    } else {
        "v descending"
    };
    debug!(
        "✓ Sorted table by column '{}' in {} order ({} total rows)",
        tabular.current_table_headers[column_index],
        sort_direction,
        tabular.all_table_data.len()
    );
}

pub(crate) fn apply_sql_filter(tabular: &mut window_egui::Tabular) {
    // If no connection or table name available, can't apply filter
    let Some(connection_id) = tabular.current_connection_id else {
        return;
    };

    // Use the existing helper function to get clean table name
    let table_name = infer_current_table_name(tabular);

    // Skip if no table name
    if table_name.is_empty() {
        return;
    }

    // Get connection info
    let Some(connection) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .cloned()
    else {
        return;
    };

    // Get database name from active tab or connection
    let database_name = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .unwrap_or_else(|| connection.database.clone());

    // Build SQL query based on database type and filter
    let sql_query = if tabular.sql_filter_text.trim().is_empty() {
        // No filter - get all data
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                if database_name.is_empty() {
                    format!("SELECT * FROM `{}`", table_name)
                } else {
                    format!("USE `{}`;\nSELECT * FROM `{}`", database_name, table_name)
                }
            }
            models::enums::DatabaseType::PostgreSQL => {
                if database_name.is_empty() {
                    format!("SELECT * FROM \"{}\"", table_name)
                } else {
                    format!("SELECT * FROM \"{}\".\"{}\"", database_name, table_name)
                }
            }
            models::enums::DatabaseType::SQLite => {
                format!("SELECT * FROM `{}`", table_name)
            }
            models::enums::DatabaseType::MsSQL => {
                driver_mssql::build_mssql_select_query(database_name, table_name)
                    .replace("SELECT TOP 100 *", "SELECT *")
            }
            _ => return, // Other database types not supported for filtering
        }
    } else {
        // Apply WHERE clause filter
        match connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                if database_name.is_empty() {
                    format!(
                        "SELECT * FROM `{}` WHERE {}",
                        table_name, tabular.sql_filter_text
                    )
                } else {
                    format!(
                        "USE `{}`;\nSELECT * FROM `{}` WHERE {}",
                        database_name, table_name, tabular.sql_filter_text
                    )
                }
            }
            models::enums::DatabaseType::PostgreSQL => {
                if database_name.is_empty() {
                    format!(
                        "SELECT * FROM \"{}\" WHERE {}",
                        table_name, tabular.sql_filter_text
                    )
                } else {
                    format!(
                        "SELECT * FROM \"{}\".\"{}\" WHERE {}",
                        database_name, table_name, tabular.sql_filter_text
                    )
                }
            }
            models::enums::DatabaseType::SQLite => {
                format!(
                    "SELECT * FROM `{}` WHERE {}",
                    table_name, tabular.sql_filter_text
                )
            }
            models::enums::DatabaseType::MsSQL => {
                let base_query = driver_mssql::build_mssql_select_query(database_name, table_name)
                    .replace("SELECT TOP 100 *", "SELECT *");
                if base_query.contains("WHERE") {
                    format!("{} AND ({})", base_query, tabular.sql_filter_text)
                } else {
                    format!(
                        "{} WHERE {}",
                        base_query.trim_end_matches(';'),
                        tabular.sql_filter_text
                    )
                }
            }
            _ => return, // Other database types not supported for filtering
        }
    };

    debug!("🔍 Applying SQL filter: {}", sql_query);

    // If the filtered query doesn't specify pagination, enable server-side pagination automatically
    let upper = sql_query.to_uppercase();
    let has_pagination_clause = upper.contains(" LIMIT ")
        || upper.contains(" OFFSET ")
        || upper.contains(" FETCH ")
        || upper.contains(" TOP ");
    if !has_pagination_clause {
        // Use server pagination: set base query and execute first page only
        let base_query = sql_query.trim().trim_end_matches(';').to_string();
        tabular.use_server_pagination = true; // force server pagination for filtered browse
        tabular.current_base_query = base_query.clone();
        tabular.current_page = 0;
        tabular.actual_total_rows = Some(10_000); // assume total rows for paging (default 10k)
        // Persist into active tab for consistent paging
        if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
            tab.base_query = base_query;
            tab.current_page = tabular.current_page;
            tab.page_size = tabular.page_size;
        }
        debug!("🚀 Auto server pagination (filter): executing first page only");
        tabular.execute_paginated_query();
        return;
    }

    // Otherwise, fallback to client-side execution with auto LIMIT
    let final_query =
        crate::connection::add_auto_limit_if_needed(&sql_query, &connection.connection_type);
    debug!("🚀 Final query with auto-limit: {}", final_query);

    if let Some((headers, data)) =
        connection::execute_query_with_connection(tabular, connection_id, final_query)
    {
        tabular.current_table_headers = headers;
        tabular.current_table_data = data.clone();
        tabular.all_table_data = data;
        tabular.total_rows = tabular.all_table_data.len();
        tabular.current_page = 0;
        update_current_page_data(tabular);
        debug!(
            "✅ Filter applied successfully, {} rows returned",
            tabular.total_rows
        );
    } else {
        tabular.error_message =
            "Failed to apply filter. Please check your WHERE clause syntax.".to_string();
        tabular.show_error_message = true;
        debug!("❌ Failed to apply SQL filter");
    }
}

// Fetch structure (columns & indexes) metadata for current table for Structure tab.
pub(crate) fn load_structure_info_for_current_table(tabular: &mut window_egui::Tabular) {
    // Determine current target
    let Some(conn_id) = tabular.current_connection_id else {
        return;
    };
    let active_tab_db = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .unwrap_or_default();
    if let Some(conn) = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(conn_id))
        .cloned()
    {
        // Infer actual table name from current UI state (avoids using captions like "Query Results")
        let table_guess = infer_current_table_name(tabular);
        if table_guess.trim().is_empty() {
            // Nothing to load if we can't determine a concrete table
            return;
        }
        let database = if !active_tab_db.is_empty() {
            active_tab_db.clone()
        } else {
            conn.database.clone()
        };

        // Short-circuit: if target unchanged and relevant subview data is already loaded, do nothing
        let target = (conn_id, database.clone(), table_guess.clone());
        if !tabular.request_structure_refresh
            && tabular
                .last_structure_target
                .as_ref()
                .map(|t| t == &target)
                .unwrap_or(false)
        {
            match tabular.structure_sub_view {
                models::structs::StructureSubView::Columns
                    if !tabular.structure_columns.is_empty() =>
                {
                    debug!(
                        "✅ Structure (columns) already loaded in-memory for {}/{} (skip reload)",
                        database, table_guess
                    );
                    return;
                }
                models::structs::StructureSubView::Indexes
                    if !tabular.structure_indexes.is_empty() =>
                {
                    debug!(
                        "✅ Structure (indexes) already loaded in-memory for {}/{} (skip reload)",
                        database, table_guess
                    );
                    return;
                }
                _ => {}
            }
        }

        // Reset current in-memory structure before (re)loading
        tabular.structure_columns.clear();
        tabular.structure_indexes.clear();
        tabular.structure_selected_row = None;
        tabular.structure_selected_cell = None;
        tabular.structure_sel_anchor = None;

        // Branch: if user explicitly requested refresh, force live fetch and update cache
        if tabular.request_structure_refresh {
            if let Some(cols) = crate::connection::fetch_columns_from_database(
                conn_id,
                &database,
                &table_guess,
                &conn,
            ) {
                crate::cache_data::save_columns_to_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                    &cols,
                );
                info!(
                    "🔄 Manual refresh: loaded live structure from server for {}/{} ({} columns)",
                    database,
                    table_guess,
                    cols.len()
                );
                for (name, dtype) in cols {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name,
                            data_type: dtype,
                            ..Default::default()
                        });
                }
            }
        } else {
            // 1) Try to populate from cache immediately for instant UI
            let mut had_struct_cache = false;
            if let Some(cols) =
                crate::cache_data::get_columns_from_cache(tabular, conn_id, &database, &table_guess)
                && !cols.is_empty()
            {
                info!(
                    "📦 Showing cached structure for {}/{} ({} columns)",
                    database,
                    table_guess,
                    cols.len()
                );
                for (name, dtype) in cols {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name,
                            data_type: dtype,
                            ..Default::default()
                        });
                }
                had_struct_cache = true;
            }

            // 2) Only fetch live structure if no cache yet
            if !had_struct_cache
                && let Some(cols) = crate::connection::fetch_columns_from_database(
                    conn_id,
                    &database,
                    &table_guess,
                    &conn,
                )
            {
                // Keep cache updated with latest structure
                crate::cache_data::save_columns_to_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                    &cols,
                );
                info!(
                    "🌐 Loaded live structure from server for {}/{} ({} columns)",
                    database,
                    table_guess,
                    cols.len()
                );
                for (name, dtype) in cols {
                    tabular
                        .structure_columns
                        .push(models::structs::ColumnStructInfo {
                            name,
                            data_type: dtype,
                            ..Default::default()
                        });
                }
            }
        }

        // Detailed index metadata: only when Indexes subview is visible
        if tabular.structure_sub_view == models::structs::StructureSubView::Indexes {
            if tabular.request_structure_refresh {
                // Force live fetch and update cache
                let idx =
                    fetch_index_details_for_table(tabular, conn_id, &conn, &database, &table_guess);
                crate::cache_data::save_indexes_to_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                    &idx,
                );
                tabular.structure_indexes = idx;
            } else {
                // Try cache first for instant display
                if let Some(cached) = crate::cache_data::get_indexes_from_cache(
                    tabular,
                    conn_id,
                    &database,
                    &table_guess,
                ) {
                    if !cached.is_empty() {
                        tabular.structure_indexes = cached;
                    } else {
                        let idx = fetch_index_details_for_table(
                            tabular,
                            conn_id,
                            &conn,
                            &database,
                            &table_guess,
                        );
                        if !idx.is_empty() {
                            crate::cache_data::save_indexes_to_cache(
                                tabular,
                                conn_id,
                                &database,
                                &table_guess,
                                &idx,
                            );
                        }
                        tabular.structure_indexes = idx;
                    }
                } else {
                    let idx = fetch_index_details_for_table(
                        tabular,
                        conn_id,
                        &conn,
                        &database,
                        &table_guess,
                    );
                    if !idx.is_empty() {
                        crate::cache_data::save_indexes_to_cache(
                            tabular,
                            conn_id,
                            &database,
                            &table_guess,
                            &idx,
                        );
                    }
                    tabular.structure_indexes = idx;
                }
            }
        }

        // Remember last loaded structure target and clear refresh request
        tabular.last_structure_target = Some((conn_id, database, table_guess));
        tabular.request_structure_refresh = false;
    }
}

// Execute a manual data refresh for current table and update row cache
pub(crate) fn refresh_current_table_data(tabular: &mut window_egui::Tabular) {
    // Stay in browse mode so spreadsheet shortcuts remain enabled after refreshes
    tabular.is_table_browse_mode = true;
    if tabular.use_server_pagination && !tabular.current_base_query.is_empty() {
        tabular.current_page = 0;
        info!("🔄 Manual refresh: server pagination first page reloaded");
        tabular.execute_paginated_query();
        return;
    }

    if let Some(conn_id) = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id)
    {
        let table = infer_current_table_name(tabular);
        if table.is_empty() {
            return;
        }
        let db_name = tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.database_name.clone())
            .unwrap_or_default();
        let db_type = tabular
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.connection_type.clone());
        if let Some(ct) = db_type {
            let query = match ct {
                models::enums::DatabaseType::MySQL => {
                    if db_name.is_empty() {
                        format!("SELECT * FROM `{}` LIMIT 100", table)
                    } else {
                        format!("USE `{}`;\nSELECT * FROM `{}` LIMIT 100", db_name, table)
                    }
                }
                models::enums::DatabaseType::PostgreSQL => {
                    if db_name.is_empty() {
                        format!("SELECT * FROM \"{}\" LIMIT 100", table)
                    } else {
                        format!("SELECT * FROM \"{}\".\"{}\" LIMIT 100", db_name, table)
                    }
                }
                models::enums::DatabaseType::SQLite => {
                    format!("SELECT * FROM `{}` LIMIT 100", table)
                }
                models::enums::DatabaseType::MsSQL => {
                    driver_mssql::build_mssql_select_query(db_name.clone(), table.clone())
                }
                _ => String::new(),
            };
            if !query.is_empty()
                && let Some((headers, data)) =
                    connection::execute_query_with_connection(tabular, conn_id, query)
            {
                tabular.current_table_headers = headers;
                tabular.current_table_data = data.clone();
                tabular.all_table_data = data;
                tabular.total_rows = tabular.all_table_data.len();
                tabular.current_page = 0;
                if let Some(active_tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
                    active_tab.result_headers = tabular.current_table_headers.clone();
                    active_tab.result_rows = tabular.current_table_data.clone();
                    active_tab.result_all_rows = tabular.all_table_data.clone();
                    active_tab.result_table_name = tabular.current_table_name.clone();
                    active_tab.is_table_browse_mode = true;
                    active_tab.current_page = tabular.current_page;
                    active_tab.page_size = tabular.page_size;
                    active_tab.total_rows = tabular.total_rows;
                }
                // Save refreshed first page to cache
                let snapshot: Vec<Vec<String>> =
                    tabular.all_table_data.iter().take(100).cloned().collect();
                let headers_clone = tabular.current_table_headers.clone();
                crate::cache_data::save_table_rows_to_cache(
                    tabular,
                    conn_id,
                    &db_name,
                    &table,
                    &headers_clone,
                    &snapshot,
                );
                info!(
                    "💾 Cached first 100 rows after manual refresh for {}/{}",
                    db_name, table
                );
            }
        }
    }
}

// Detailed index metadata loader per database
fn fetch_index_details_for_table(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    connection: &models::structs::ConnectionConfig,
    database_name: &str,
    table_name: &str,
) -> Vec<models::structs::IndexStructInfo> {
    match connection.connection_type {
        models::enums::DatabaseType::MySQL => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                    if let Some(models::enums::DatabasePool::MySQL(mysql_pool)) = crate::connection::get_or_create_connection_pool(tabular, connection_id).await {
                        let q = r#"SELECT INDEX_NAME, GROUP_CONCAT(COLUMN_NAME ORDER BY SEQ_IN_INDEX) AS COLS, MIN(NON_UNIQUE) AS NON_UNIQUE, GROUP_CONCAT(DISTINCT INDEX_TYPE) AS TYPES FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? GROUP BY INDEX_NAME ORDER BY INDEX_NAME"#;
                        match sqlx::query(q).bind(database_name).bind(table_name).fetch_all(mysql_pool.as_ref()).await {
                            Ok(rows) => { use sqlx::Row; rows.into_iter().map(|r| {
                                let name: String = r.get("INDEX_NAME");
                                let cols_str: Option<String> = r.try_get("COLS").ok();
                                let non_unique: Option<i64> = r.try_get("NON_UNIQUE").ok();
                                let types: Option<String> = r.try_get("TYPES").ok();
                                let columns = cols_str.unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                                let unique = matches!(non_unique, Some(0));
                                let method = types.and_then(|t| t.split(',').next().map(|m| m.trim().to_string())).filter(|s| !s.is_empty());
                                models::structs::IndexStructInfo { name, method, unique, columns }
                            }).collect() }
                            Err(_) => Vec::new(),
                        }
                    } else { Vec::new() }
                })
        }
        models::enums::DatabaseType::PostgreSQL => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                    if let Some(models::enums::DatabasePool::PostgreSQL(pg_pool)) = crate::connection::get_or_create_connection_pool(tabular, connection_id).await {
                        let q = r#"SELECT idx.relname AS index_name, pg_get_indexdef(i.indexrelid) AS index_def, i.indisunique AS is_unique FROM pg_class t JOIN pg_index i ON t.oid = i.indrelid JOIN pg_class idx ON idx.oid = i.indexrelid JOIN pg_namespace n ON n.oid = t.relnamespace WHERE t.relname = $1 AND n.nspname='public' ORDER BY idx.relname"#;
                        match sqlx::query(q).bind(table_name).fetch_all(pg_pool.as_ref()).await {
                            Ok(rows) => { use sqlx::Row; rows.into_iter().map(|r| {
                                let name: String = r.get("index_name");
                                let def: String = r.get("index_def");
                                let unique: bool = r.get("is_unique");
                                let method = def.split(" USING ").nth(1).and_then(|rest| rest.split_whitespace().next()).and_then(|m| if m.starts_with('('){None}else{Some(m.trim_matches('(').trim_matches(')').to_string())});
                                let columns: Vec<String> = if let Some(start) = def.rfind('(') { if let Some(end_rel) = def[start+1..].find(')') { def[start+1..start+1+end_rel].split(',').map(|s| s.trim().trim_matches('"').to_string()).filter(|s| !s.is_empty()).collect() } else { Vec::new() } } else { Vec::new() };
                                models::structs::IndexStructInfo { name, method, unique, columns }
                            }).collect() }
                            Err(_) => Vec::new(),
                        }
                    } else { Vec::new() }
                })
        }
        models::enums::DatabaseType::MsSQL => {
            use tiberius::{AuthMethod, Config};
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let host = connection.host.clone();
            let port: u16 = connection.port.parse().unwrap_or(1433);
            let user = connection.username.clone();
            let pass = connection.password.clone();
            let db = database_name.to_string();
            let tbl = table_name.to_string();
            let rt_res = tokio::runtime::Runtime::new().unwrap().block_on(async move {
                    let mut config = Config::new(); config.host(host.clone()); config.port(port); config.authentication(AuthMethod::sql_server(user.clone(), pass.clone())); config.trust_cert(); if !db.is_empty() { config.database(db.clone()); }
                    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.map_err(|e| e.to_string())?; tcp.set_nodelay(true).map_err(|e| e.to_string())?;
                    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await.map_err(|e| e.to_string())?;
                    let parse = |name: &str| -> (Option<String>, String) { if let Some((s,t)) = name.split_once('.') { (Some(s.trim_matches(['[',']']).to_string()), t.trim_matches(['[',']']).to_string()) } else { (None, name.trim_matches(['[',']']).to_string()) } };
                    let (_schema_opt, table_only) = parse(&tbl);
                    let q = format!("SELECT i.name AS index_name, i.is_unique, i.type_desc, STUFF((SELECT ','+c.name FROM sys.index_columns ic2 JOIN sys.columns c ON c.object_id=ic2.object_id AND c.column_id=ic2.column_id WHERE ic2.object_id=i.object_id AND ic2.index_id=i.index_id ORDER BY ic2.key_ordinal FOR XML PATH(''), TYPE).value('.','NVARCHAR(MAX)'),1,1,'') AS columns FROM sys.indexes i INNER JOIN sys.objects o ON o.object_id=i.object_id WHERE o.name='{}' AND i.name IS NOT NULL ORDER BY i.name", table_only.replace("'","''"));
                    let mut stream = client.simple_query(q).await.map_err(|e| e.to_string())?; use futures_util::TryStreamExt; use tiberius::QueryItem; let mut list = Vec::new();
                    while let Some(item) = stream.try_next().await.map_err(|e| e.to_string())? { if let QueryItem::Row(r) = item { let name: Option<&str> = r.get(0); let is_unique: Option<bool> = r.get(1); let type_desc: Option<&str> = r.get(2); let cols: Option<&str> = r.get(3); if let Some(nm)=name { list.push(models::structs::IndexStructInfo { name: nm.to_string(), method: type_desc.map(|s| s.to_string()), unique: is_unique.unwrap_or(false), columns: cols.unwrap_or("").split(',').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect() }); } } }
                    Ok::<_, String>(list)
                });
            rt_res.unwrap_or_default()
        }
        models::enums::DatabaseType::SQLite => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Some(models::enums::DatabasePool::SQLite(sqlite_pool)) =
                    crate::connection::get_or_create_connection_pool(tabular, connection_id).await
                {
                    use sqlx::Row;
                    let list_query =
                        format!("PRAGMA index_list('{}')", table_name.replace("'", "''"));
                    match sqlx::query(&list_query)
                        .fetch_all(sqlite_pool.as_ref())
                        .await
                    {
                        Ok(rows) => {
                            let mut infos = Vec::new();
                            for r in rows {
                                let name_opt: Option<String> = r.try_get("name").ok().flatten();
                                let unique_flag: Option<i64> = r.try_get("unique").ok().flatten();
                                if let Some(nm) = name_opt {
                                    let info_q =
                                        format!("PRAGMA index_info('{}')", nm.replace("'", "''"));
                                    let mut cols_vec = Vec::new();
                                    if let Ok(crows) =
                                        sqlx::query(&info_q).fetch_all(sqlite_pool.as_ref()).await
                                    {
                                        for cr in crows {
                                            if let Ok(Some(coln)) =
                                                cr.try_get::<Option<String>, _>("name")
                                            {
                                                cols_vec.push(coln);
                                            }
                                        }
                                    }
                                    infos.push(models::structs::IndexStructInfo {
                                        name: nm,
                                        method: None,
                                        unique: matches!(unique_flag, Some(0)),
                                        columns: cols_vec,
                                    });
                                }
                            }
                            infos
                        }
                        Err(_) => Vec::new(),
                    }
                } else {
                    Vec::new()
                }
            })
        }
        models::enums::DatabaseType::MongoDB => {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                if let Some(models::enums::DatabasePool::MongoDB(client)) =
                    crate::connection::get_or_create_connection_pool(tabular, connection_id).await
                {
                    match client
                        .database(database_name)
                        .collection::<mongodb::bson::Document>(table_name)
                        .list_index_names()
                        .await
                    {
                        Ok(names) => names
                            .into_iter()
                            .map(|n| models::structs::IndexStructInfo {
                                name: n,
                                method: None,
                                unique: false,
                                columns: Vec::new(),
                            })
                            .collect(),
                        Err(_) => Vec::new(),
                    }
                } else {
                    Vec::new()
                }
            })
        }
        _ => Vec::new(),
    }
}

pub(crate) fn render_structure_view(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    let avail = ui.available_size();

    ui.horizontal(|ui| {
        let toggle_width = 120.0;

        ui.scope(|ui| {
            let mut style = ui.style().as_ref().clone();
            style.visuals.selection.bg_fill = egui::Color32::from_rgb(255, 13, 0);
            style.visuals.selection.stroke.color = egui::Color32::from_rgb(255, 13, 0);
            style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(255, 13, 0);
            style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(255, 13, 0);
            ui.set_style(style);

            ui.set_min_width(toggle_width);
            ui.set_min_height(avail.y);

            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.add_space(6.0);
                let default_text = ui.visuals().widgets.inactive.fg_stroke.color;

                let active_cols =
                    tabular.structure_sub_view == models::structs::StructureSubView::Columns;
                let draw_vertical_toggle = |ui: &mut egui::Ui,
                                             label: &str,
                                             active: bool|
                 -> egui::Response {
                    let button_size = egui::vec2(toggle_width, toggle_width);
                    let (rect, response) =
                        ui.allocate_exact_size(button_size, egui::Sense::click());

                    let mut bg = if active {
                        egui::Color32::from_rgb(255, 13, 0)
                    } else {
                        ui.visuals().widgets.inactive.bg_fill
                    };
                    if response.hovered() && !active {
                        bg = bg.gamma_multiply(1.12);
                    }

                    let stroke_color = if active {
                        egui::Color32::from_rgb(255, 13, 0)
                    } else {
                        ui.visuals().widgets.inactive.bg_stroke.color
                    };
                    let stroke = egui::Stroke::new(1.0, stroke_color);

                    let painter = ui.painter();
                    let rounding = 6.0;
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

                let cols_resp = draw_vertical_toggle(ui, "Columns", active_cols);
                if cols_resp.clicked() {
                    tabular.structure_sub_view = models::structs::StructureSubView::Columns;
                    tabular.structure_sel_anchor = None;
                    tabular.structure_selected_cell = None;
                    tabular.structure_selected_row = None;
                }

                ui.add_space(4.0);

                let active_idx =
                    tabular.structure_sub_view == models::structs::StructureSubView::Indexes;
                let idx_resp = draw_vertical_toggle(ui, "Indexes", active_idx);
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

        ui.separator();

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
                                                        egui::Color32::from_rgb(255, 60, 0),
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
                            if ui.button("✏️ Edit Column").clicked() {
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
                        let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 60, 0));
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
                    let w_type = widths[2];
                    ui.allocate_ui_with_layout(egui::vec2(w_type,row_h), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_width(w_type-8.0);
                        let types = ["varchar(255)", "bigint", "int", "text", "longtext", "datetime", "date", "float", "double", "boolean"];
                        egui::ComboBox::from_id_salt("new_col_type").selected_text(&tabular.new_column_type).show_ui(ui, |ui| {
                            for t in types { if ui.selectable_label(tabular.new_column_type==t, t).clicked() { tabular.new_column_type = t.to_string(); } }
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
                    .button(egui::RichText::new("Confirm").color(egui::Color32::RED))
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

pub(crate) fn infer_current_table_name(tabular: &mut window_egui::Tabular) -> String {
    // Priority 1: if current_table_name starts with "Table:" extract
    if tabular.current_table_name.starts_with("Table:") {
        let after = tabular
            .current_table_name
            .split_once(':')
            .map(|x| x.1)
            .unwrap_or("")
            .trim();
        let mut cut = after.to_string();
        if let Some(p) = cut.find('(') {
            cut = cut[..p].trim().to_string();
        }
        if !cut.is_empty() {
            return cut;
        }
    }
    // Priority 2: active tab title pattern
    let ttitle = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.title.clone())
        .unwrap_or_default();
    let mut table_guess = if ttitle.contains(':') {
        ttitle.split(':').nth(1).unwrap_or("").trim().to_string()
    } else {
        String::new()
    };
    if let Some(p) = table_guess.find('(') {
        table_guess = table_guess[..p].trim().to_string();
    }
    table_guess
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
                    .button(egui::RichText::new("Confirm").color(egui::Color32::RED))
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

pub(crate) fn clear_table_selection(tabular: &mut window_egui::Tabular) {
    tabular.selected_row = None;
    tabular.selected_cell = None;
    tabular.selected_rows.clear();
    tabular.selected_columns.clear();
    tabular.last_clicked_row = None;
    tabular.last_clicked_column = None;
    // Also clear multi-cell block state for Data grid
    tabular.table_sel_anchor = None;
    tabular.table_dragging = false;
}

pub(crate) fn handle_row_click(
    tabular: &mut window_egui::Tabular,
    row_index: usize,
    modifiers: egui::Modifiers,
) {
    // Treat selection within current page only
    if modifiers.command {
        // Cmd/Ctrl toggles
        if tabular.selected_rows.contains(&row_index) {
            tabular.selected_rows.remove(&row_index);
        } else {
            tabular.selected_rows.insert(row_index);
        }
        tabular.last_clicked_row = Some(row_index);
    } else if modifiers.shift && tabular.last_clicked_row.is_some() {
        let last = tabular.last_clicked_row.unwrap();
        let (start, end) = if row_index <= last {
            (row_index, last)
        } else {
            (last, row_index)
        };
        for r in start..=end {
            tabular.selected_rows.insert(r);
        }
    } else {
        // No modifier: if only this row is selected, toggle it off; otherwise select only this
        if tabular.selected_rows.len() == 1 && tabular.selected_rows.contains(&row_index) {
            tabular.selected_rows.clear();
            tabular.selected_row = None;
            tabular.last_clicked_row = None;
        } else {
            tabular.selected_rows.clear();
            tabular.selected_rows.insert(row_index);
            tabular.last_clicked_row = Some(row_index);
        }
    }
    // Align single-selection marker with set state
    tabular.selected_row = if tabular.selected_rows.len() == 1 {
        tabular.selected_rows.iter().copied().next()
    } else {
        None
    };
    tabular.selected_cell = None;
}

pub(crate) fn handle_column_click(
    tabular: &mut window_egui::Tabular,
    col_index: usize,
    modifiers: egui::Modifiers,
) {
    if modifiers.command {
        // Cmd/Ctrl toggles
        if tabular.selected_columns.contains(&col_index) {
            tabular.selected_columns.remove(&col_index);
        } else {
            tabular.selected_columns.insert(col_index);
        }
        tabular.last_clicked_column = Some(col_index);
    } else if modifiers.shift && tabular.last_clicked_column.is_some() {
        let last = tabular.last_clicked_column.unwrap();
        let (start, end) = if col_index <= last {
            (col_index, last)
        } else {
            (last, col_index)
        };
        for c in start..=end {
            tabular.selected_columns.insert(c);
        }
    } else {
        // No modifier: if only this column is selected, toggle it off; otherwise select only this
        if tabular.selected_columns.len() == 1 && tabular.selected_columns.contains(&col_index) {
            tabular.selected_columns.clear();
            tabular.last_clicked_column = None;
        } else {
            tabular.selected_columns.clear();
            tabular.selected_columns.insert(col_index);
            tabular.last_clicked_column = Some(col_index);
        }
    }
    tabular.selected_cell = None;
}

pub(crate) fn copy_selected_rows_as_csv(tabular: &mut window_egui::Tabular) -> Option<String> {
    if tabular.selected_rows.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for (idx, row) in tabular.current_table_data.iter().enumerate() {
        if tabular.selected_rows.contains(&idx) {
            let line = row
                .iter()
                .map(|cell| {
                    if cell.contains(',') || cell.contains('"') || cell.contains('\n') {
                        format!("\"{}\"", cell.replace('"', "\"\""))
                    } else {
                        cell.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            lines.push(line);
        }
    }
    Some(lines.join("\n"))
}

pub(crate) fn copy_selected_columns_as_csv(tabular: &mut window_egui::Tabular) -> Option<String> {
    if tabular.selected_columns.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    // header first
    let mut header = Vec::new();
    for (i, h) in tabular.current_table_headers.iter().enumerate() {
        if tabular.selected_columns.contains(&i) {
            header.push(h.clone());
        }
    }
    if !header.is_empty() {
        lines.push(header.join(","));
    }
    for row in &tabular.current_table_data {
        let mut cols = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            if tabular.selected_columns.contains(&i) {
                if cell.contains(',') || cell.contains('"') || cell.contains('\n') {
                    cols.push(format!("\"{}\"", cell.replace('"', "\"\"")));
                } else {
                    cols.push(cell.clone());
                }
            }
        }
        lines.push(cols.join(","));
    }
    Some(lines.join("\n"))
}

/// Build CSV for a rectangular block selection in the Data grid (inclusive bounds).
/// Returns None if the selection is invalid or outside the current page.
pub(crate) fn copy_selected_block_as_csv(
    tabular: &mut window_egui::Tabular,
    a: (usize, usize),
    b: (usize, usize),
) -> Option<String> {
    let (ar, ac) = a;
    let (br, bc) = b;
    let rmin = ar.min(br);
    let rmax = ar.max(br);
    let cmin = ac.min(bc);
    let cmax = ac.max(bc);
    if rmax >= tabular.current_table_data.len() {
        return None;
    }
    if tabular.current_table_data.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    for r in rmin..=rmax {
        if let Some(row) = tabular.current_table_data.get(r) {
            let mut cols: Vec<String> = Vec::new();
            for c in cmin..=cmax {
                if let Some(val) = row.get(c) {
                    if val.contains(',') || val.contains('"') || val.contains('\n') {
                        cols.push(format!("\"{}\"", val.replace('"', "\"\"")));
                    } else {
                        cols.push(val.clone());
                    }
                } else {
                    cols.push(String::new());
                }
            }
            lines.push(cols.join(","));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

// Pagination methods
pub fn update_pagination_data(tabular: &mut window_egui::Tabular, all_data: Vec<Vec<String>>) {
    debug!("=== UPDATE_PAGINATION_DATA DEBUG ===");
    debug!("Received data rows: {}", all_data.len());
    if !all_data.is_empty() {
        debug!("First row sample: {:?}", &all_data[0]);
    }

    tabular.all_table_data = all_data;
    tabular.total_rows = tabular.all_table_data.len();
    tabular.current_page = 0; // Reset to first page

    debug!(
        "After assignment - all_table_data.len(): {}",
        tabular.all_table_data.len()
    );
    debug!("After assignment - total_rows: {}", tabular.total_rows);
    debug!("====================================");

    update_current_page_data(tabular);

    // Initialize column widths when new data is loaded
    initialize_column_widths(tabular);
}

// Column width management methods
pub(crate) fn initialize_column_widths(tabular: &mut window_egui::Tabular) {
    let num_columns = tabular.current_table_headers.len();
    if num_columns > 0 {
        // Calculate initial column width based on available space
        let base_width = 180.0; // Base width per column
        tabular.column_widths = vec![base_width; num_columns];
    } else {
        tabular.column_widths.clear();
    }
}

pub(crate) fn get_column_width(tabular: &window_egui::Tabular, column_index: usize) -> f32 {
    tabular
        .column_widths
        .get(column_index)
        .copied()
        .unwrap_or(180.0)
        .max(tabular.min_column_width)
}

pub(crate) fn set_column_width(
    tabular: &mut window_egui::Tabular,
    column_index: usize,
    width: f32,
) {
    if column_index < tabular.column_widths.len() {
        // Only enforce minimum width, allow unlimited maximum width
        let safe_width = width.max(tabular.min_column_width);
        // Ensure we never have invalid floating point values
        let final_width = if safe_width.is_finite() && safe_width > 0.0 {
            safe_width
        } else {
            tabular.min_column_width
        };
        tabular.column_widths[column_index] = final_width;
    }
}

pub(crate) fn update_current_page_data(tabular: &mut window_egui::Tabular) {
    let start_index = tabular.current_page * tabular.page_size;
    let end_index =
        ((tabular.current_page + 1) * tabular.page_size).min(tabular.all_table_data.len());

    if start_index < tabular.all_table_data.len() {
        tabular.current_table_data = tabular.all_table_data[start_index..end_index].to_vec();
    } else {
        tabular.current_table_data.clear();
    }
}

pub(crate) fn next_page(tabular: &mut window_egui::Tabular) {
    // Check if we have a base query in the active tab for server-side pagination
    let has_base_query = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|tab| !tab.base_query.is_empty())
        .unwrap_or(false);

    if tabular.use_server_pagination && has_base_query {
        // Server-side pagination
        let total_pages = get_total_pages_server(tabular);
        if tabular.current_page < total_pages.saturating_sub(1) {
            tabular.current_page += 1;
            tabular.execute_paginated_query();
            clear_table_selection(tabular);
        }
    } else {
        // Client-side pagination (original behavior)
        let max_page = (tabular.total_rows.saturating_sub(1)) / tabular.page_size;
        if tabular.current_page < max_page {
            tabular.current_page += 1;
            update_current_page_data(tabular);
            clear_table_selection(tabular);
        }
    }
}

pub(crate) fn previous_page(tabular: &mut window_egui::Tabular) {
    // Check if we have a base query in the active tab for server-side pagination
    let has_base_query = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|tab| !tab.base_query.is_empty())
        .unwrap_or(false);

    if tabular.use_server_pagination && has_base_query {
        // Server-side pagination
        if tabular.current_page > 0 {
            tabular.current_page -= 1;
            tabular.execute_paginated_query();
            clear_table_selection(tabular);
        }
    } else {
        // Client-side pagination (original behavior)
        if tabular.current_page > 0 {
            tabular.current_page -= 1;
            update_current_page_data(tabular);
            clear_table_selection(tabular);
        }
    }
}

pub(crate) fn go_to_page(tabular: &mut window_egui::Tabular, page: usize) {
    // Check if we have a base query in the active tab for server-side pagination
    let has_base_query = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|tab| !tab.base_query.is_empty())
        .unwrap_or(false);

    if tabular.use_server_pagination && has_base_query {
        // Server-side pagination
        let total_pages = get_total_pages_server(tabular);
        if page < total_pages {
            tabular.current_page = page;
            tabular.execute_paginated_query();
            clear_table_selection(tabular);
        }
    } else {
        // Client-side pagination (original behavior)
        let max_page = (tabular.total_rows.saturating_sub(1)) / tabular.page_size;
        if page <= max_page {
            tabular.current_page = page;
            update_current_page_data(tabular);
            clear_table_selection(tabular);
        }
    }
}

pub(crate) fn get_total_pages_server(tabular: &mut window_egui::Tabular) -> usize {
    // Avoid division by zero if page_size was restored as 0 from an older tab/session
    let ps = if tabular.page_size == 0 {
        100
    } else {
        tabular.page_size
    };
    if let Some(actual_total) = tabular.actual_total_rows {
        actual_total.div_ceil(ps) // Ceiling division
    } else {
        1
    }
}

pub(crate) fn get_total_pages(tabular: &window_egui::Tabular) -> usize {
    if tabular.total_rows == 0 {
        // Return 1 page if we have headers (table structure exists) but no data
        if !tabular.current_table_headers.is_empty() {
            1
        } else {
            0
        }
    } else if tabular.page_size == 0 {
        1 // Avoid division by zero, fallback to 1 page
    } else {
        tabular.total_rows.div_ceil(tabular.page_size)
    }
}
