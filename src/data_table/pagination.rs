use eframe::egui;
use log::debug;
use crate::window_egui;
use super::clear_table_selection;

pub(crate) fn render_pagination_bar(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    // Execution time of the currently displayed result (read before the mutable
    // borrow taken by the closure below).
    let exec_ms = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.results.get(t.active_result_index))
        .map(|r| r.execution_time_ms)
        .filter(|ms| *ms > 0);

    let bg_color = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(22, 22, 26)
    } else {
        egui::Color32::from_rgb(245, 245, 250)
    };
    let stroke_color = if ui.visuals().dark_mode {
        egui::Color32::from_rgb(45, 45, 50)
    } else {
        egui::Color32::from_rgb(215, 215, 220)
    };

    egui::Frame::new()
        .fill(bg_color)
        .stroke(egui::Stroke::new(1.0, stroke_color))
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let spacing = ui.spacing_mut();
                spacing.item_spacing.x = 4.0;
                spacing.button_padding = egui::vec2(6.0, 3.0);

                if tabular.use_server_pagination && tabular.actual_total_rows.is_some() {
                    let actual_total = tabular.actual_total_rows.unwrap_or(0);
                    if actual_total > 0 {
                        let start_row = tabular.current_page * tabular.page_size + 1;
                        let end_row = ((tabular.current_page + 1) * tabular.page_size).min(actual_total);
                        ui.label(format!("Showing rows {}-{}", start_row, end_row));
                    } else {
                        ui.label("0 rows");
                    }
                    ui.colored_label(crate::window_egui::style::theme_success(ui.ctx()), "📡 Server pagination");
                } else {
                    ui.label(format!("Total rows: {}", tabular.total_rows));
                    if !tabular.use_server_pagination {
                        ui.colored_label(crate::window_egui::style::theme_warning(ui.ctx()), "💾 Client pagination");
                    }
                }

                // Execution time indicator
                if let Some(ms) = exec_ms {
                    ui.separator();
                    let label = if ms >= 1000 {
                        format!("⏱ {:.2} s", ms as f64 / 1000.0)
                    } else {
                        format!("⏱ {} ms", ms)
                    };
                    ui.label(egui::RichText::new(label).color(ui.visuals().weak_text_color()))
                        .on_hover_text("Query execution time");
                }

                // Grid Summary Bar (Sum, Avg, Count, Min, Max for selected cells)
                if let Some(summary) = super::selection::calculate_grid_summary(tabular) {
                    ui.separator();
                    let format_num = |v: f64| -> String {
                        if v.fract().abs() < 1e-6 {
                            format!("{:.0}", v)
                        } else {
                            format!("{:.2}", v)
                        }
                    };

                    if summary.numeric_count > 0 {
                        let summary_text = format!(
                            "∑ Sum: {}  |  x̅ Avg: {}  |  🔢 Count: {}  |  ⬇ Min: {}  |  ⬆ Max: {}",
                            format_num(summary.sum),
                            format_num(summary.avg),
                            summary.numeric_count,
                            format_num(summary.min),
                            format_num(summary.max)
                        );
                        ui.colored_label(
                            crate::window_egui::style::theme_accent(ui.ctx()),
                            egui::RichText::new(summary_text).strong().small(),
                        )
                        .on_hover_text(format!(
                            "Selection Summary ({} cells selected, {} numeric)",
                            summary.total_cells, summary.numeric_count
                        ));
                    } else {
                        ui.colored_label(
                            ui.visuals().weak_text_color(),
                            egui::RichText::new(format!("Selected: {} cells", summary.total_cells)).small(),
                        );
                    }
                }

                ui.separator();

                // Page size selector
                ui.label("Rows per page:");
                let mut page_size_str = tabular.page_size.to_string();
                // Batasi lebar input supaya tidak mengembang mengisi bar dan membuat gap
                if ui
                    .add(egui::TextEdit::singleline(&mut page_size_str).desired_width(50.0))
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
                    .add_enabled(has_data, egui::TextEdit::singleline(&mut page_input).desired_width(40.0))
                    .changed()
                    && let Ok(page_num) = page_input.parse::<usize>()
                    && page_num > 0
                {
                    go_to_page(tabular, page_num - 1);
                }

                // Embed 3 view buttons directly into the right side of the datatable footer bar
                render_footer_view_buttons(tabular, ui);
            });
        });
}

pub(crate) fn render_footer_view_buttons(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui) {
    let executed = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.has_executed_query)
        .unwrap_or(false);
    let has_headers = !tabular.current_table_headers.is_empty();
    let has_message = !tabular.query_message.is_empty();
    let has_lint = !tabular.lint_messages.is_empty();

    if !executed && !has_headers && !has_message && !has_lint {
        return;
    }

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().button_padding = egui::vec2(6.0, 2.0);
        let button_height = 20.0;

        // Show Details (Lint Issue) Button
        if has_lint {
            let count = tabular.lint_messages.len();
            let lint_text_label = format!("⚠️ Details ({})", count);
            let is_lint_open = tabular.show_lint_panel;

            let lint_bg = if is_lint_open {
                window_egui::style::theme_warning(ui.ctx())
            } else if ui.visuals().dark_mode {
                egui::Color32::from_rgb(60, 45, 30)
            } else {
                egui::Color32::from_rgb(255, 243, 224)
            };
            let lint_text_color = if is_lint_open {
                egui::Color32::WHITE
            } else {
                window_egui::style::theme_warning(ui.ctx())
            };

            let lint_btn = egui::Button::new(
                egui::RichText::new(lint_text_label)
                    .small()
                    .strong()
                    .color(lint_text_color),
            )
            .fill(lint_bg)
            .corner_radius(egui::CornerRadius::same(4u8))
            .min_size(egui::vec2(0.0, button_height));

            if ui.add(lint_btn).clicked() {
                tabular.show_lint_panel = !tabular.show_lint_panel;
            }
        }

        // Messages Button
        if has_message {
            let is_msg_open = tabular.show_message_panel;
            let is_error = tabular.query_message_is_error;
            let messages_bg = if is_msg_open || is_error {
                if is_error {
                    window_egui::style::theme_danger(ui.ctx())
                } else {
                    window_egui::style::theme_accent(ui.ctx())
                }
            } else if ui.visuals().dark_mode {
                egui::Color32::from_rgb(45, 45, 50)
            } else {
                egui::Color32::from_rgb(225, 225, 230)
            };
            let messages_text_color = if is_msg_open || is_error {
                egui::Color32::WHITE
            } else {
                ui.visuals().text_color()
            };

            let msg_btn = egui::Button::new(
                egui::RichText::new("💬 Messages")
                    .small()
                    .strong()
                    .color(messages_text_color),
            )
            .fill(messages_bg)
            .corner_radius(egui::CornerRadius::same(4u8))
            .min_size(egui::vec2(0.0, button_height));

            if ui.add(msg_btn).clicked() {
                tabular.show_message_panel = !tabular.show_message_panel;
                tabular.message_shown_at = None;
            }
        }

        // Data Button
        let is_data = tabular.table_bottom_view == crate::models::structs::TableBottomView::Data
            && !tabular.show_message_panel
            && !tabular.show_lint_panel;
        let data_bg = if is_data {
            window_egui::style::theme_accent(ui.ctx())
        } else if ui.visuals().dark_mode {
            egui::Color32::from_rgb(45, 45, 50)
        } else {
            egui::Color32::from_rgb(225, 225, 230)
        };
        let data_text_color = if is_data {
            egui::Color32::WHITE
        } else {
            ui.visuals().text_color()
        };

        let data_btn = egui::Button::new(
            egui::RichText::new("📊 Data")
                .small()
                .strong()
                .color(data_text_color),
        )
        .fill(data_bg)
        .corner_radius(egui::CornerRadius::same(4u8))
        .min_size(egui::vec2(0.0, button_height));

        if ui.add(data_btn).clicked() {
            tabular.table_bottom_view = crate::models::structs::TableBottomView::Data;
            tabular.show_message_panel = false;
            tabular.show_lint_panel = false;
        }

        // Explain Button (shown if EXPLAIN output present)
        let has_explain = tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.explain_plan_json.as_ref())
            .is_some();
        if has_explain {
            let is_explain = tabular.table_bottom_view == crate::models::structs::TableBottomView::Explain
                && !tabular.show_message_panel
                && !tabular.show_lint_panel;
            let explain_bg = if is_explain {
                window_egui::style::theme_accent(ui.ctx())
            } else if ui.visuals().dark_mode {
                egui::Color32::from_rgb(45, 45, 50)
            } else {
                egui::Color32::from_rgb(225, 225, 230)
            };
            let explain_text_color = if is_explain {
                egui::Color32::WHITE
            } else {
                ui.visuals().text_color()
            };

            let explain_btn = egui::Button::new(
                egui::RichText::new("🔍 Explain")
                    .small()
                    .strong()
                    .color(explain_text_color),
            )
            .fill(explain_bg)
            .corner_radius(egui::CornerRadius::same(4u8))
            .min_size(egui::vec2(0.0, button_height));

            if ui.add(explain_btn).clicked() {
                tabular.table_bottom_view = crate::models::structs::TableBottomView::Explain;
                tabular.show_message_panel = false;
                tabular.show_lint_panel = false;
            }
        }
    });
}

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

