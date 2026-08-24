use log::debug;
use crate::{connection, driver_mssql, models, window_egui};
use super::{update_current_page_data, infer_current_table_name};

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

pub fn build_where_from_visual_filter(
    filter: &models::structs::VisualFilterState,
    db_type: Option<&models::enums::DatabaseType>,
) -> String {
    let quote_ident = |col: &str| -> String {
        match db_type {
            Some(models::enums::DatabaseType::MySQL) => format!("`{}`", col.replace('`', "``")),
            Some(models::enums::DatabaseType::MsSQL) => format!("[{}]", col.trim_matches(['[', ']'])),
            _ => format!("\"{}\"", col.replace('"', "\"\"")),
        }
    };

    let quote_val = |val: &str| -> String {
        let v = val.trim();
        // If it parses as a pure integer or float, we can keep it unquoted for numeric comparisons
        if v.parse::<i64>().is_ok() || v.parse::<f64>().is_ok() {
            v.to_string()
        } else {
            match db_type {
                Some(models::enums::DatabaseType::MySQL) => {
                    format!("'{}'", v.replace('\\', "\\\\").replace('\'', "''"))
                }
                _ => format!("'{}'", v.replace('\'', "''")),
            }
        }
    };

    let escape_like_val = |val: &str| -> String {
        let v = val.replace('\'', "''");
        format!("'{}'", v)
    };

    let mut parts = Vec::new();

    for cond in &filter.conditions {
        let col = cond.column.trim();
        if col.is_empty() {
            continue;
        }

        let q_col = quote_ident(col);
        let val = cond.value.trim();

        let clause = match cond.operator {
            models::structs::FilterOperator::Equals => {
                format!("{} = {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::NotEquals => {
                format!("{} != {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::Contains => {
                let pattern = format!("%{}%", val.replace('%', "\\%").replace('_', "\\_"));
                format!("{} LIKE {}", q_col, escape_like_val(&pattern))
            }
            models::structs::FilterOperator::StartsWith => {
                let pattern = format!("{}%", val.replace('%', "\\%").replace('_', "\\_"));
                format!("{} LIKE {}", q_col, escape_like_val(&pattern))
            }
            models::structs::FilterOperator::EndsWith => {
                let pattern = format!("%{}", val.replace('%', "\\%").replace('_', "\\_"));
                format!("{} LIKE {}", q_col, escape_like_val(&pattern))
            }
            models::structs::FilterOperator::GreaterThan => {
                format!("{} > {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::LessThan => {
                format!("{} < {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::GreaterThanOrEqual => {
                format!("{} >= {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::LessThanOrEqual => {
                format!("{} <= {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::IsNull => {
                format!("{} IS NULL", q_col)
            }
            models::structs::FilterOperator::IsNotNull => {
                format!("{} IS NOT NULL", q_col)
            }
            models::structs::FilterOperator::In => {
                let items: Vec<String> = val
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| quote_val(s))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                format!("{} IN ({})", q_col, items.join(", "))
            }
        };

        parts.push(clause);
    }

    if parts.is_empty() {
        String::new()
    } else if parts.len() == 1 {
        parts.remove(0)
    } else {
        let glue = if filter.match_all { " AND " } else { " OR " };
        parts.join(glue)
    }
}

pub(crate) fn render_visual_filter_panel(tabular: &mut window_egui::Tabular, ui: &mut eframe::egui::Ui) {
    if !tabular.visual_filter.is_open {
        return;
    }

    let accent = crate::window_egui::style::theme_accent(ui.ctx());
    let muted = crate::window_egui::style::theme_muted_text(ui.ctx());
    let mut remove_idx = None;
    let mut apply_filter_now = false;
    let mut clear_all_now = false;

    eframe::egui::Frame::group(ui.style())
        .fill(ui.visuals().extreme_bg_color)
        .inner_margin(eframe::egui::Vec2::new(10.0, 8.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(eframe::egui::RichText::new("⊞ Visual Filters").strong().color(accent));
                ui.add_space(8.0);
                ui.label(eframe::egui::RichText::new("Match:").color(muted).small());

                if ui
                    .selectable_label(tabular.visual_filter.match_all, "ALL (AND)")
                    .clicked()
                {
                    tabular.visual_filter.match_all = true;
                }
                if ui
                    .selectable_label(!tabular.visual_filter.match_all, "ANY (OR)")
                    .clicked()
                {
                    tabular.visual_filter.match_all = false;
                }

                ui.add_space(8.0);
                if ui.button("+ Add Condition").clicked() {
                    let first_col = tabular
                        .current_table_headers
                        .first()
                        .cloned()
                        .unwrap_or_default();
                    tabular.visual_filter.conditions.push(models::structs::FilterCondition {
                        column: first_col,
                        operator: models::structs::FilterOperator::Equals,
                        value: String::new(),
                    });
                }

                if !tabular.visual_filter.conditions.is_empty() {
                    if ui.button("Clear All").clicked() {
                        clear_all_now = true;
                    }
                    if ui
                        .add(crate::window_egui::style::btn_primary_ctx(ui.ctx(), "Apply Filter"))
                        .clicked()
                    {
                        apply_filter_now = true;
                    }
                }
            });

            if tabular.visual_filter.conditions.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    eframe::egui::RichText::new("No visual filters configured. Click \"+ Add Condition\" to create a filter without writing SQL.")
                        .italics()
                        .color(muted)
                        .small(),
                );
            } else {
                ui.add_space(6.0);
                let headers = tabular.current_table_headers.clone();

                for (idx, cond) in tabular.visual_filter.conditions.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(eframe::egui::RichText::new(format!("{}.", idx + 1)).color(muted).small());

                        // Column selector
                        eframe::egui::ComboBox::from_id_salt(format!("filter_col_{}", idx))
                            .selected_text(if cond.column.is_empty() {
                                "Select Column"
                            } else {
                                &cond.column
                            })
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for h in &headers {
                                    ui.selectable_value(&mut cond.column, h.clone(), h);
                                }
                            });

                        // Operator selector
                        eframe::egui::ComboBox::from_id_salt(format!("filter_op_{}", idx))
                            .selected_text(cond.operator.label())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for op in models::structs::FilterOperator::all() {
                                    ui.selectable_value(&mut cond.operator, *op, op.label());
                                }
                            });

                        // Value field (hidden for IS NULL / IS NOT NULL)
                        let needs_val = !matches!(
                            cond.operator,
                            models::structs::FilterOperator::IsNull
                                | models::structs::FilterOperator::IsNotNull
                        );
                        if needs_val {
                            let hint = if cond.operator == models::structs::FilterOperator::In {
                                "val1, val2, val3"
                            } else {
                                "value..."
                            };
                            let resp = ui.add(
                                eframe::egui::TextEdit::singleline(&mut cond.value)
                                    .hint_text(hint)
                                    .desired_width(160.0),
                            );
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter)) {
                                apply_filter_now = true;
                            }
                        } else {
                            ui.label(eframe::egui::RichText::new("(no value needed)").color(muted).italics().small());
                        }

                        // Remove button
                        if ui.button("✖").on_hover_text("Remove condition").clicked() {
                            remove_idx = Some(idx);
                        }
                    });
                }
            }
        });

    if let Some(i) = remove_idx {
        if i < tabular.visual_filter.conditions.len() {
            tabular.visual_filter.conditions.remove(i);
        }
    }

    if clear_all_now {
        tabular.visual_filter.conditions.clear();
        tabular.sql_filter_text.clear();
        apply_sql_filter(tabular);
    }

    if apply_filter_now {
        let db_type = tabular.current_connection_id.and_then(|cid| {
            tabular.connections.iter().find(|c| c.id == Some(cid)).map(|c| &c.connection_type)
        });
        tabular.sql_filter_text = build_where_from_visual_filter(&tabular.visual_filter, db_type);
        apply_sql_filter(tabular);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_where_equals_and_like() {
        let mut filter = models::structs::VisualFilterState {
            conditions: vec![
                models::structs::FilterCondition {
                    column: "name".to_string(),
                    operator: models::structs::FilterOperator::Equals,
                    value: "John".to_string(),
                },
                models::structs::FilterCondition {
                    column: "age".to_string(),
                    operator: models::structs::FilterOperator::GreaterThan,
                    value: "25".to_string(),
                },
            ],
            match_all: true,
            is_open: true,
        };

        let where_clause = build_where_from_visual_filter(&filter, Some(&models::enums::DatabaseType::PostgreSQL));
        assert_eq!(where_clause, "\"name\" = 'John' AND \"age\" > 25");

        filter.match_all = false;
        let where_or = build_where_from_visual_filter(&filter, Some(&models::enums::DatabaseType::MySQL));
        assert_eq!(where_or, "`name` = 'John' OR `age` > 25");
    }

    #[test]
    fn test_build_where_null_and_in() {
        let filter = models::structs::VisualFilterState {
            conditions: vec![
                models::structs::FilterCondition {
                    column: "deleted_at".to_string(),
                    operator: models::structs::FilterOperator::IsNull,
                    value: "".to_string(),
                },
                models::structs::FilterCondition {
                    column: "status".to_string(),
                    operator: models::structs::FilterOperator::In,
                    value: "active, pending".to_string(),
                },
            ],
            match_all: true,
            is_open: true,
        };

        let clause = build_where_from_visual_filter(&filter, Some(&models::enums::DatabaseType::PostgreSQL));
        assert_eq!(clause, "\"deleted_at\" IS NULL AND \"status\" IN ('active', 'pending')");
    }
}
