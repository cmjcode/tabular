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

// Fetch structure (columns & indexes) metadata for current table for Structure tab.
