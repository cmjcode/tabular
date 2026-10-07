use super::{infer_current_table_name, update_current_page_data};
use crate::{driver_mssql, models, window_egui};
use log::debug;

pub use crate::models::structs::SqlValue;

/// Kunci urut satu sel untuk sort di sisi klien. Urutan total (ascending):
/// angka (via `f64::total_cmp`, termasuk inf/NaN), lalu teks tanpa membedakan
/// huruf besar/kecil, lalu NULL/kosong di akhir. Descending membalik semuanya,
/// sehingga NULL berada di awal, sama seperti perilaku sebelumnya.
#[derive(Debug, Clone)]
pub(crate) enum SortKey {
    Num(f64),
    Str(String),
    Null,
}

impl SortKey {
    /// `None` = baris tidak memiliki kolom tersebut; diperlakukan seperti NULL.
    pub(crate) fn from_cell(cell: Option<&String>) -> Self {
        match cell.map(String::as_str) {
            None | Some("NULL") | Some("") => SortKey::Null,
            Some(value) => match value.parse::<f64>() {
                Ok(num) => SortKey::Num(num),
                Err(_) => SortKey::Str(value.to_lowercase()),
            },
        }
    }

    fn rank(&self) -> u8 {
        match self {
            SortKey::Num(_) => 0,
            SortKey::Str(_) => 1,
            SortKey::Null => 2,
        }
    }
}

// Kesetaraan mengikuti `Ord` (NaN == NaN) agar konsisten dengan urutan total.
impl PartialEq for SortKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for SortKey {}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (SortKey::Num(a), SortKey::Num(b)) => a.total_cmp(b),
            (SortKey::Str(a), SortKey::Str(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Sorts the loaded table data in-memory by the specified column index
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

    // Sort ALL the data (not just current page). Kunci dihitung sekali per
    // baris (bukan parse + lowercase di setiap perbandingan) dan urutannya
    // total, jadi `sort` tidak bisa panik karena komparator tidak konsisten.
    // Pengurutan stabil: baris dengan kunci sama mempertahankan urutan semula.
    if ascending {
        tabular
            .all_table_data
            .sort_by_cached_key(|row| SortKey::from_cell(row.get(column_index)));
    } else {
        tabular
            .all_table_data
            .sort_by_cached_key(|row| std::cmp::Reverse(SortKey::from_cell(row.get(column_index))));
    }

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

/// Quotes and escapes a database identifier safely according to the specific database dialect
#[allow(dead_code)]
pub fn quote_identifier(col: &str, db_type: &models::enums::DatabaseType) -> String {
    let clean = col.trim();
    match db_type {
        models::enums::DatabaseType::MySQL => {
            let unquoted = clean.trim_matches('`');
            let escaped = unquoted.replace('`', "``");
            format!("`{}`", escaped)
        }
        models::enums::DatabaseType::MsSQL => {
            let unbracketed = clean.trim_start_matches('[').trim_end_matches(']');
            let escaped = unbracketed.replace(']', "]]");
            format!("[{}]", escaped)
        }
        models::enums::DatabaseType::PostgreSQL | models::enums::DatabaseType::SQLite => {
            let unquoted = clean.trim_matches('"');
            let escaped = unquoted.replace('"', "\"\"");
            format!("\"{}\"", escaped)
        }
        _ => {
            let escaped = clean.replace('"', "\"\"");
            format!("\"{}\"", escaped)
        }
    }
}

/// Parses a user-entered filter value string into a typed `SqlValue`
#[allow(dead_code)]
pub fn parse_sql_param_value(val: &str) -> SqlValue {
    let v = val.trim();
    if v.eq_ignore_ascii_case("null") {
        SqlValue::Null
    } else if v.eq_ignore_ascii_case("true") {
        SqlValue::Boolean(true)
    } else if v.eq_ignore_ascii_case("false") {
        SqlValue::Boolean(false)
    } else if let Ok(i) = v.parse::<i64>() {
        SqlValue::Integer(i)
    } else if let Ok(f) = v.parse::<f64>() {
        SqlValue::Number(f)
    } else {
        SqlValue::Text(v.to_string())
    }
}

/// Builds a parameterized server-side SQL WHERE clause and its corresponding parameters vector.
///
/// Uses `$1, $2, ...` for PostgreSQL, `@p1, @p2, ...` for MsSQL, and `?` for MySQL/SQLite.
#[allow(dead_code)]
pub fn build_server_side_where_clause(
    conditions: &[models::structs::FilterCondition],
    db_type: &models::enums::DatabaseType,
) -> (String, Vec<SqlValue>) {
    build_server_side_where_clause_with_group(
        conditions,
        models::structs::FilterGroup::And,
        db_type,
    )
}

/// Builds a parameterized server-side SQL WHERE clause with an explicit `FilterGroup` (AND / OR).
#[allow(dead_code)]
pub fn build_server_side_where_clause_with_group(
    conditions: &[models::structs::FilterCondition],
    group: models::structs::FilterGroup,
    db_type: &models::enums::DatabaseType,
) -> (String, Vec<SqlValue>) {
    let mut parts = Vec::new();
    let mut params = Vec::new();
    let mut param_index = 1;

    for cond in conditions {
        let col = cond.column.trim();
        if col.is_empty() {
            continue;
        }

        let q_col = quote_identifier(col, db_type);
        let val = cond.value.trim();

        let make_placeholder = |idx: usize| -> String {
            match db_type {
                models::enums::DatabaseType::PostgreSQL => format!("${}", idx),
                models::enums::DatabaseType::MsSQL => format!("@p{}", idx),
                _ => "?".to_string(),
            }
        };

        match cond.operator {
            models::structs::FilterOperator::Equal | models::structs::FilterOperator::Equals => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(parse_sql_param_value(val));
                parts.push(format!("{} = {}", q_col, ph));
            }
            models::structs::FilterOperator::NotEqual
            | models::structs::FilterOperator::NotEquals => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(parse_sql_param_value(val));
                parts.push(format!("{} != {}", q_col, ph));
            }
            models::structs::FilterOperator::GreaterThan => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(parse_sql_param_value(val));
                parts.push(format!("{} > {}", q_col, ph));
            }
            models::structs::FilterOperator::LessThan => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(parse_sql_param_value(val));
                parts.push(format!("{} < {}", q_col, ph));
            }
            models::structs::FilterOperator::GreaterThanOrEqual => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(parse_sql_param_value(val));
                parts.push(format!("{} >= {}", q_col, ph));
            }
            models::structs::FilterOperator::LessThanOrEqual => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(parse_sql_param_value(val));
                parts.push(format!("{} <= {}", q_col, ph));
            }
            models::structs::FilterOperator::Like => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(SqlValue::Text(val.to_string()));
                parts.push(format!("{} LIKE {}", q_col, ph));
            }
            models::structs::FilterOperator::ILike => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(SqlValue::Text(val.to_string()));
                match db_type {
                    models::enums::DatabaseType::PostgreSQL => {
                        parts.push(format!("{} ILIKE {}", q_col, ph));
                    }
                    models::enums::DatabaseType::MySQL | models::enums::DatabaseType::SQLite => {
                        parts.push(format!("LOWER({}) LIKE LOWER({})", q_col, ph));
                    }
                    _ => {
                        parts.push(format!("{} LIKE {}", q_col, ph));
                    }
                }
            }
            models::structs::FilterOperator::Contains => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(SqlValue::Text(format!("%{}%", val)));
                match db_type {
                    models::enums::DatabaseType::PostgreSQL => {
                        parts.push(format!("{} ILIKE {}", q_col, ph));
                    }
                    models::enums::DatabaseType::MySQL | models::enums::DatabaseType::SQLite => {
                        parts.push(format!("LOWER({}) LIKE LOWER({})", q_col, ph));
                    }
                    _ => {
                        parts.push(format!("{} LIKE {}", q_col, ph));
                    }
                }
            }
            models::structs::FilterOperator::StartsWith => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(SqlValue::Text(format!("{}%", val)));
                match db_type {
                    models::enums::DatabaseType::PostgreSQL => {
                        parts.push(format!("{} ILIKE {}", q_col, ph));
                    }
                    models::enums::DatabaseType::MySQL | models::enums::DatabaseType::SQLite => {
                        parts.push(format!("LOWER({}) LIKE LOWER({})", q_col, ph));
                    }
                    _ => {
                        parts.push(format!("{} LIKE {}", q_col, ph));
                    }
                }
            }
            models::structs::FilterOperator::EndsWith => {
                let ph = make_placeholder(param_index);
                param_index += 1;
                params.push(SqlValue::Text(format!("%{}", val)));
                match db_type {
                    models::enums::DatabaseType::PostgreSQL => {
                        parts.push(format!("{} ILIKE {}", q_col, ph));
                    }
                    models::enums::DatabaseType::MySQL | models::enums::DatabaseType::SQLite => {
                        parts.push(format!("LOWER({}) LIKE LOWER({})", q_col, ph));
                    }
                    _ => {
                        parts.push(format!("{} LIKE {}", q_col, ph));
                    }
                }
            }
            models::structs::FilterOperator::IsNull => {
                parts.push(format!("{} IS NULL", q_col));
            }
            models::structs::FilterOperator::IsNotNull => {
                parts.push(format!("{} IS NOT NULL", q_col));
            }
            models::structs::FilterOperator::Between => {
                let (v1, v2) = if let Some(ref val2) = cond.value2 {
                    (val, val2.trim())
                } else if val.contains(" AND ") {
                    let s: Vec<&str> = val.split(" AND ").collect();
                    (s[0].trim(), s[1].trim())
                } else if val.contains(" and ") {
                    let s: Vec<&str> = val.split(" and ").collect();
                    (s[0].trim(), s[1].trim())
                } else if val.contains(',') {
                    let s: Vec<&str> = val.split(',').collect();
                    (s[0].trim(), s.get(1).map(|x| x.trim()).unwrap_or_default())
                } else {
                    (val, "")
                };

                if !v1.is_empty() && !v2.is_empty() {
                    let ph1 = make_placeholder(param_index);
                    param_index += 1;
                    let ph2 = make_placeholder(param_index);
                    param_index += 1;
                    params.push(parse_sql_param_value(v1));
                    params.push(parse_sql_param_value(v2));
                    parts.push(format!("{} BETWEEN {} AND {}", q_col, ph1, ph2));
                } else if !v1.is_empty() {
                    let ph = make_placeholder(param_index);
                    param_index += 1;
                    params.push(parse_sql_param_value(v1));
                    parts.push(format!("{} = {}", q_col, ph));
                }
            }
            models::structs::FilterOperator::In => {
                let items: Vec<&str> = val
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if items.is_empty() {
                    continue;
                }
                let mut phs = Vec::new();
                for item in items {
                    let ph = make_placeholder(param_index);
                    param_index += 1;
                    params.push(parse_sql_param_value(item));
                    phs.push(ph);
                }
                parts.push(format!("{} IN ({})", q_col, phs.join(", ")));
            }
        }
    }

    if parts.is_empty() {
        (String::new(), params)
    } else {
        let glue = match group {
            models::structs::FilterGroup::Or => " OR ",
            models::structs::FilterGroup::And => " AND ",
        };
        (parts.join(glue), params)
    }
}

/// Builds an inline, safely escaped SQL WHERE clause from visual filter state for SQL query execution and UI preview.
pub fn build_where_from_visual_filter(
    filter: &models::structs::VisualFilterState,
    db_type: Option<&models::enums::DatabaseType>,
) -> String {
    let quote_ident = |col: &str| -> String {
        let clean = col.trim();
        match db_type {
            Some(models::enums::DatabaseType::MySQL) => {
                let unquoted = clean.trim_matches('`');
                format!("`{}`", unquoted.replace('`', "``"))
            }
            Some(models::enums::DatabaseType::MsSQL) => {
                let unbracketed = clean.trim_start_matches('[').trim_end_matches(']');
                format!("[{}]", unbracketed.replace(']', "]]"))
            }
            _ => {
                let unquoted = clean.trim_matches('"');
                format!("\"{}\"", unquoted.replace('"', "\"\""))
            }
        }
    };

    let quote_val = |val: &str| -> String {
        let v = val.trim();
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
            models::structs::FilterOperator::Equal | models::structs::FilterOperator::Equals => {
                format!("{} = {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::NotEqual
            | models::structs::FilterOperator::NotEquals => {
                format!("{} != {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::Like => {
                format!("{} LIKE {}", q_col, quote_val(val))
            }
            models::structs::FilterOperator::ILike => match db_type {
                Some(models::enums::DatabaseType::PostgreSQL) => {
                    format!("{} ILIKE {}", q_col, quote_val(val))
                }
                Some(models::enums::DatabaseType::MySQL)
                | Some(models::enums::DatabaseType::SQLite) => {
                    format!("LOWER({}) LIKE LOWER({})", q_col, quote_val(val))
                }
                _ => format!("{} LIKE {}", q_col, quote_val(val)),
            },
            models::structs::FilterOperator::Between => {
                if let Some(ref v2) = cond.value2 {
                    format!(
                        "{} BETWEEN {} AND {}",
                        q_col,
                        quote_val(val),
                        quote_val(v2.trim())
                    )
                } else if val.contains(" AND ") || val.contains(" and ") {
                    let parts: Vec<&str> = if val.contains(" AND ") {
                        val.split(" AND ").collect()
                    } else {
                        val.split(" and ").collect()
                    };
                    if parts.len() == 2 {
                        format!(
                            "{} BETWEEN {} AND {}",
                            q_col,
                            quote_val(parts[0].trim()),
                            quote_val(parts[1].trim())
                        )
                    } else {
                        format!("{} = {}", q_col, quote_val(val))
                    }
                } else if val.contains(',') {
                    let parts: Vec<&str> = val.split(',').collect();
                    if parts.len() == 2 {
                        format!(
                            "{} BETWEEN {} AND {}",
                            q_col,
                            quote_val(parts[0].trim()),
                            quote_val(parts[1].trim())
                        )
                    } else {
                        format!("{} = {}", q_col, quote_val(val))
                    }
                } else {
                    format!("{} = {}", q_col, quote_val(val))
                }
            }
            models::structs::FilterOperator::Contains => {
                let pattern = format!("%{}%", val.replace('%', "\\%").replace('_', "\\_"));
                match db_type {
                    Some(models::enums::DatabaseType::PostgreSQL) => {
                        format!("{} ILIKE {}", q_col, escape_like_val(&pattern))
                    }
                    Some(models::enums::DatabaseType::MySQL)
                    | Some(models::enums::DatabaseType::SQLite) => {
                        format!("LOWER({}) LIKE LOWER({})", q_col, escape_like_val(&pattern))
                    }
                    _ => format!("{} LIKE {}", q_col, escape_like_val(&pattern)),
                }
            }
            models::structs::FilterOperator::StartsWith => {
                let pattern = format!("{}%", val.replace('%', "\\%").replace('_', "\\_"));
                match db_type {
                    Some(models::enums::DatabaseType::PostgreSQL) => {
                        format!("{} ILIKE {}", q_col, escape_like_val(&pattern))
                    }
                    Some(models::enums::DatabaseType::MySQL)
                    | Some(models::enums::DatabaseType::SQLite) => {
                        format!("LOWER({}) LIKE LOWER({})", q_col, escape_like_val(&pattern))
                    }
                    _ => format!("{} LIKE {}", q_col, escape_like_val(&pattern)),
                }
            }
            models::structs::FilterOperator::EndsWith => {
                let pattern = format!("%{}", val.replace('%', "\\%").replace('_', "\\_"));
                match db_type {
                    Some(models::enums::DatabaseType::PostgreSQL) => {
                        format!("{} ILIKE {}", q_col, escape_like_val(&pattern))
                    }
                    Some(models::enums::DatabaseType::MySQL)
                    | Some(models::enums::DatabaseType::SQLite) => {
                        format!("LOWER({}) LIKE LOWER({})", q_col, escape_like_val(&pattern))
                    }
                    _ => format!("{} LIKE {}", q_col, escape_like_val(&pattern)),
                }
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
                    .map(&quote_val)
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
        let glue = match filter.group {
            models::structs::FilterGroup::Or => " OR ",
            models::structs::FilterGroup::And => {
                if filter.match_all {
                    " AND "
                } else {
                    " OR "
                }
            }
        };
        parts.join(glue)
    }
}

/// Applies the current SQL WHERE filter to the active table and fetches fresh paginated rows from the database.
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
        tabular.actual_total_rows = None; // total belum diketahui sampai user menekan Count rows
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

    tabular.run_query_with_callback(connection_id, final_query, |tabular, message| {
        if !message.success {
            debug!("❌ Failed to apply SQL filter");
            tabular.toasts.error(format!(
                "Failed to apply filter: {}",
                message.error.clone().unwrap_or_default()
            ));
            return;
        }
        tabular.current_table_headers = message.headers.clone();
        tabular.current_table_data = message.rows.clone();
        tabular.all_table_data = message.rows.clone();
        tabular.total_rows = tabular.all_table_data.len();
        tabular.current_page = 0;
        update_current_page_data(tabular);
        debug!(
            "✅ Filter applied successfully, {} rows returned",
            tabular.total_rows
        );
    });
}

/// Renders the modular visual filter builder bar and condition rows directly above the data grid
pub(crate) fn render_visual_filter_panel(
    tabular: &mut window_egui::Tabular,
    ui: &mut eframe::egui::Ui,
) {
    if !tabular.visual_filter.is_open {
        return;
    }

    let accent = crate::window_egui::style::theme_accent(ui.ctx());
    let muted = crate::window_egui::style::theme_muted_text(ui.ctx());
    let is_dark = ui.visuals().dark_mode;
    let mut remove_idx = None;
    let mut apply_filter_now = false;
    let mut clear_all_now = false;
    let cond_count = tabular.visual_filter.conditions.len();

    let bg_color = if is_dark {
        eframe::egui::Color32::from_rgb(22, 24, 30)
    } else {
        eframe::egui::Color32::from_rgb(245, 247, 250)
    };

    let border_color = if is_dark {
        eframe::egui::Color32::from_rgb(45, 49, 62)
    } else {
        eframe::egui::Color32::from_rgb(220, 226, 235)
    };

    eframe::egui::Frame::group(ui.style())
        .fill(bg_color)
        .stroke(eframe::egui::Stroke::new(1.0, border_color))
        .corner_radius(6.0)
        .inner_margin(eframe::egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            // Header Row: Controls, Match Mode, Add/Clear/Apply buttons
            ui.horizontal(|ui| {
                ui.label(
                    eframe::egui::RichText::new("⊞ Filter Builder")
                        .strong()
                        .color(accent),
                );

                if cond_count > 0 {
                    let badge_text = format!("{} active", cond_count);
                    ui.label(
                        eframe::egui::RichText::new(badge_text)
                            .small()
                            .color(if is_dark { eframe::egui::Color32::from_rgb(140, 180, 255) } else { eframe::egui::Color32::from_rgb(30, 80, 200) }),
                    );
                }

                ui.add_space(8.0);
                ui.label(eframe::egui::RichText::new("Match:").color(muted).small());

                if ui
                    .selectable_label(tabular.visual_filter.match_all, "ALL (AND)")
                    .on_hover_text("All conditions must match (AND logic)")
                    .clicked()
                {
                    tabular.visual_filter.match_all = true;
                    tabular.visual_filter.group = models::structs::FilterGroup::And;
                }
                if ui
                    .selectable_label(!tabular.visual_filter.match_all, "ANY (OR)")
                    .on_hover_text("Any condition may match (OR logic)")
                    .clicked()
                {
                    tabular.visual_filter.match_all = false;
                    tabular.visual_filter.group = models::structs::FilterGroup::Or;
                }

                ui.add_space(8.0);
                if ui
                    .button("+ Filter")
                    .on_hover_text("Add a new filter condition row")
                    .clicked()
                {
                    let first_col = tabular
                        .current_table_headers
                        .first()
                        .cloned()
                        .unwrap_or_default();
                    tabular.visual_filter.conditions.push(models::structs::FilterCondition::new(
                        first_col,
                        models::structs::FilterOperator::Equal,
                        String::new(),
                    ));
                }

                if cond_count > 0 {
                    if ui
                        .add(crate::window_egui::style::btn_secondary("Clear All"))
                        .on_hover_text("Clear all filter conditions and reset table")
                        .clicked()
                    {
                        clear_all_now = true;
                    }

                    if ui
                        .add(crate::window_egui::style::btn_primary_ctx(ui.ctx(), "Apply Filter"))
                        .on_hover_text("Execute server-side query with active conditions")
                        .clicked()
                    {
                        apply_filter_now = true;
                    }
                }
            });

            // Body: List of filter rows or empty state
            if tabular.visual_filter.conditions.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    eframe::egui::RichText::new("No visual filters configured. Click \"+ Filter\" to build server-side conditions without writing SQL.")
                        .italics()
                        .color(muted)
                        .small(),
                );
            } else {
                ui.add_space(6.0);
                let headers = tabular.current_table_headers.clone();

                for (idx, cond) in tabular.visual_filter.conditions.iter_mut().enumerate() {
                    let row_bg = if is_dark {
                        if idx % 2 == 0 {
                            eframe::egui::Color32::from_rgb(28, 30, 38)
                        } else {
                            eframe::egui::Color32::from_rgb(24, 26, 33)
                        }
                    } else if idx % 2 == 0 {
                        eframe::egui::Color32::from_rgb(255, 255, 255)
                    } else {
                        eframe::egui::Color32::from_rgb(240, 243, 248)
                    };

                    eframe::egui::Frame::new()
                        .fill(row_bg)
                        .corner_radius(4.0)
                        .inner_margin(eframe::egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    eframe::egui::RichText::new(format!("{}.", idx + 1))
                                        .color(muted)
                                        .small(),
                                );

                                // Column selector dropdown
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

                                // Operator selector dropdown
                                eframe::egui::ComboBox::from_id_salt(format!("filter_op_{}", idx))
                                    .selected_text(cond.operator.label())
                                    .width(160.0)
                                    .show_ui(ui, |ui| {
                                        for op in models::structs::FilterOperator::all() {
                                            ui.selectable_value(&mut cond.operator, *op, op.label());
                                        }
                                    });

                                // Value fields
                                match cond.operator {
                                    models::structs::FilterOperator::IsNull | models::structs::FilterOperator::IsNotNull => {
                                        ui.label(
                                            eframe::egui::RichText::new("(no value needed)")
                                                .color(muted)
                                                .italics()
                                                .small(),
                                        );
                                    }
                                    models::structs::FilterOperator::Between => {
                                        let v2_ref = cond.value2.get_or_insert_with(String::new);
                                        let r1 = window_egui::style::render_text_field(
                                            ui,
                                            eframe::egui::TextEdit::singleline(&mut cond.value)
                                                .hint_text("From (min)"),
                                            100.0,
                                            None,
                                        );
                                        ui.label(eframe::egui::RichText::new("and").color(muted).small());
                                        let r2 = window_egui::style::render_text_field(
                                            ui,
                                            eframe::egui::TextEdit::singleline(v2_ref)
                                                .hint_text("To (max)"),
                                            100.0,
                                            None,
                                        );
                                        if (r1.lost_focus() || r2.lost_focus()) && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter)) {
                                            apply_filter_now = true;
                                        }
                                    }
                                    models::structs::FilterOperator::In => {
                                        let resp = window_egui::style::render_text_field(
                                            ui,
                                            eframe::egui::TextEdit::singleline(&mut cond.value)
                                                .hint_text("val1, val2, val3..."),
                                            180.0,
                                            None,
                                        );
                                        if resp.lost_focus() && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter)) {
                                            apply_filter_now = true;
                                        }
                                    }
                                    _ => {
                                        let hint = match cond.operator {
                                            models::structs::FilterOperator::Contains => "%value%",
                                            models::structs::FilterOperator::StartsWith => "value%",
                                            models::structs::FilterOperator::EndsWith => "%value",
                                            models::structs::FilterOperator::Like => "pattern (e.g. A_%)",
                                            models::structs::FilterOperator::ILike => "pattern (case-insensitive)",
                                            _ => "value...",
                                        };
                                        let resp = window_egui::style::render_text_field(
                                            ui,
                                            eframe::egui::TextEdit::singleline(&mut cond.value)
                                                .hint_text(hint),
                                            160.0,
                                            None,
                                        );
                                        if resp.lost_focus() && ui.input(|i| i.key_pressed(eframe::egui::Key::Enter)) {
                                            apply_filter_now = true;
                                        }
                                    }
                                }

                                ui.add_space(4.0);
                                // Remove condition button
                                if ui.button("✖").on_hover_text("Remove this condition").clicked() {
                                    remove_idx = Some(idx);
                                }
                            });
                        });
                    ui.add_space(2.0);
                }

                // Live WHERE preview
                let db_type = tabular.current_connection_id.and_then(|cid| {
                    tabular.connections.iter().find(|c| c.id == Some(cid)).map(|c| &c.connection_type)
                });
                let generated_sql = build_where_from_visual_filter(&tabular.visual_filter, db_type);
                if !generated_sql.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(eframe::egui::RichText::new("WHERE:").strong().small().color(muted));
                        ui.label(
                            eframe::egui::RichText::new(&generated_sql)
                                .monospace()
                                .small()
                                .color(if is_dark { eframe::egui::Color32::from_rgb(180, 220, 255) } else { eframe::egui::Color32::from_rgb(20, 70, 160) }),
                        );
                        if ui.small_button("📋").on_hover_text("Copy generated WHERE clause").clicked() {
                            ui.ctx().copy_text(generated_sql);
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
            tabular
                .connections
                .iter()
                .find(|c| c.id == Some(cid))
                .map(|c| &c.connection_type)
        });
        tabular.sql_filter_text = build_where_from_visual_filter(&tabular.visual_filter, db_type);
        apply_sql_filter(tabular);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::DatabaseType;
    use crate::models::structs::{FilterCondition, FilterGroup, FilterOperator, VisualFilterState};

    #[test]
    fn test_quote_identifier_safety() {
        assert_eq!(
            quote_identifier("user_name", &DatabaseType::MySQL),
            "`user_name`"
        );
        assert_eq!(
            quote_identifier("user`name", &DatabaseType::MySQL),
            "`user``name`"
        );
        assert_eq!(
            quote_identifier("user_name", &DatabaseType::PostgreSQL),
            "\"user_name\""
        );
        assert_eq!(
            quote_identifier("user\"name", &DatabaseType::PostgreSQL),
            "\"user\"\"name\""
        );
        assert_eq!(
            quote_identifier("user_name", &DatabaseType::SQLite),
            "\"user_name\""
        );
        assert_eq!(
            quote_identifier("user_name", &DatabaseType::MsSQL),
            "[user_name]"
        );
        assert_eq!(
            quote_identifier("user]name", &DatabaseType::MsSQL),
            "[user]]name]"
        );
    }

    #[test]
    fn test_build_server_side_where_clause_postgres() {
        let conditions = vec![
            FilterCondition::new("age", FilterOperator::GreaterThan, "25"),
            FilterCondition::new("status", FilterOperator::Equal, "active"),
            FilterCondition::new("email", FilterOperator::Like, "%@example.com"),
        ];

        let (where_clause, params) =
            build_server_side_where_clause(&conditions, &DatabaseType::PostgreSQL);
        assert_eq!(
            where_clause,
            "\"age\" > $1 AND \"status\" = $2 AND \"email\" LIKE $3"
        );
        assert_eq!(params.len(), 3);
        assert_eq!(params[0], SqlValue::Integer(25));
        assert_eq!(params[1], SqlValue::Text("active".to_string()));
        assert_eq!(params[2], SqlValue::Text("%@example.com".to_string()));
    }

    #[test]
    fn test_build_server_side_where_clause_mysql() {
        let conditions = vec![
            FilterCondition::new("category_id", FilterOperator::Equal, "10"),
            FilterCondition::new("deleted_at", FilterOperator::IsNull, ""),
            FilterCondition::new("title", FilterOperator::Contains, "rust"),
        ];

        let (where_clause, params) =
            build_server_side_where_clause(&conditions, &DatabaseType::MySQL);
        assert_eq!(
            where_clause,
            "`category_id` = ? AND `deleted_at` IS NULL AND LOWER(`title`) LIKE LOWER(?)"
        );
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], SqlValue::Integer(10));
        assert_eq!(params[1], SqlValue::Text("%rust%".to_string()));
    }

    #[test]
    fn test_build_server_side_where_clause_mssql() {
        let conditions = vec![
            FilterCondition::new("price", FilterOperator::LessThanOrEqual, "99.99"),
            FilterCondition::new("is_active", FilterOperator::Equal, "true"),
        ];

        let (where_clause, params) =
            build_server_side_where_clause(&conditions, &DatabaseType::MsSQL);
        assert_eq!(where_clause, "[price] <= @p1 AND [is_active] = @p2");
        assert_eq!(params.len(), 2);
        assert_eq!(params[0], SqlValue::Number(99.99));
        assert_eq!(params[1], SqlValue::Boolean(true));
    }

    #[test]
    fn test_build_server_side_where_clause_between_and_in() {
        let conditions = vec![
            FilterCondition::between("created_at", "2026-01-01", "2026-12-31"),
            FilterCondition::new("role", FilterOperator::In, "admin, manager, dev"),
        ];

        let (where_clause, params) =
            build_server_side_where_clause(&conditions, &DatabaseType::PostgreSQL);
        assert_eq!(
            where_clause,
            "\"created_at\" BETWEEN $1 AND $2 AND \"role\" IN ($3, $4, $5)"
        );
        assert_eq!(params.len(), 5);
        assert_eq!(params[0], SqlValue::Text("2026-01-01".to_string()));
        assert_eq!(params[1], SqlValue::Text("2026-12-31".to_string()));
        assert_eq!(params[2], SqlValue::Text("admin".to_string()));
        assert_eq!(params[3], SqlValue::Text("manager".to_string()));
        assert_eq!(params[4], SqlValue::Text("dev".to_string()));
    }

    #[test]
    fn test_build_server_side_where_clause_or_group() {
        let conditions = vec![
            FilterCondition::new("status", FilterOperator::Equal, "failed"),
            FilterCondition::new("status", FilterOperator::Equal, "error"),
        ];

        let (where_clause, params) = build_server_side_where_clause_with_group(
            &conditions,
            FilterGroup::Or,
            &DatabaseType::SQLite,
        );
        assert_eq!(where_clause, "\"status\" = ? OR \"status\" = ?");
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_build_where_equals_and_like() {
        let mut filter = VisualFilterState {
            conditions: vec![
                FilterCondition::new("name", FilterOperator::Equals, "John"),
                FilterCondition::new("age", FilterOperator::GreaterThan, "25"),
            ],
            match_all: true,
            is_open: true,
            group: FilterGroup::And,
            pinned_columns: Default::default(),
        };

        let where_clause = build_where_from_visual_filter(&filter, Some(&DatabaseType::PostgreSQL));
        assert_eq!(where_clause, "\"name\" = 'John' AND \"age\" > 25");

        filter.match_all = false;
        let where_or = build_where_from_visual_filter(&filter, Some(&DatabaseType::MySQL));
        assert_eq!(where_or, "`name` = 'John' OR `age` > 25");
    }

    #[test]
    fn test_build_where_null_and_in() {
        let filter = VisualFilterState {
            conditions: vec![
                FilterCondition::new("deleted_at", FilterOperator::IsNull, ""),
                FilterCondition::new("status", FilterOperator::In, "active, pending"),
            ],
            match_all: true,
            is_open: true,
            group: FilterGroup::And,
            pinned_columns: Default::default(),
        };

        let clause = build_where_from_visual_filter(&filter, Some(&DatabaseType::PostgreSQL));
        assert_eq!(
            clause,
            "\"deleted_at\" IS NULL AND \"status\" IN ('active', 'pending')"
        );
    }

    fn sort_key(value: &str) -> SortKey {
        SortKey::from_cell(Some(&value.to_string()))
    }

    fn sorted_column(values: &[&str], ascending: bool) -> Vec<String> {
        let mut t = crate::window_egui::Tabular::new();
        t.current_table_headers = vec!["v".to_string(), "pos".to_string()];
        t.all_table_data = values
            .iter()
            .enumerate()
            .map(|(i, v)| vec![v.to_string(), i.to_string()])
            .collect();
        t.page_size = values.len().max(1);
        t.current_page = 0;
        sort_table_data(&mut t, 0, ascending);
        t.all_table_data.iter().map(|r| r[0].clone()).collect()
    }

    #[test]
    fn sort_numeric_column_is_numeric_not_lexical() {
        assert_eq!(
            sorted_column(&["10", "9", "-1.5", "100", "2"], true),
            vec!["-1.5", "2", "9", "10", "100"]
        );
        assert_eq!(
            sorted_column(&["10", "9", "-1.5", "100", "2"], false),
            vec!["100", "10", "9", "2", "-1.5"]
        );
    }

    #[test]
    fn sort_places_nulls_last_ascending_and_first_descending() {
        assert_eq!(
            sorted_column(&["b", "NULL", "a", "", "C"], true),
            vec!["a", "b", "C", "NULL", ""]
        );
        assert_eq!(
            sorted_column(&["b", "NULL", "a", "", "C"], false),
            vec!["NULL", "", "C", "b", "a"]
        );
    }

    #[test]
    fn sort_mixed_numeric_and_text_puts_numbers_before_text() {
        assert_eq!(
            sorted_column(&["abc", "10", "2", "Abd", "1e3", "x9"], true),
            vec!["2", "10", "1e3", "abc", "Abd", "x9"]
        );
    }

    #[test]
    fn sort_handles_nan_and_infinity_strings() {
        assert_eq!(
            sorted_column(&["NaN", "inf", "1", "-inf", "NULL", "0"], true),
            vec!["-inf", "0", "1", "inf", "NaN", "NULL"]
        );
        assert_eq!(sort_key("NaN"), sort_key("nan"));
    }

    #[test]
    fn sort_is_stable_for_equal_keys() {
        let mut t = crate::window_egui::Tabular::new();
        t.current_table_headers = vec!["v".to_string(), "pos".to_string()];
        t.all_table_data = ["b", "A", "a", "B", "a"]
            .iter()
            .enumerate()
            .map(|(i, v)| vec![v.to_string(), i.to_string()])
            .collect();
        t.page_size = 10;
        sort_table_data(&mut t, 0, true);
        let order: Vec<&str> = t.all_table_data.iter().map(|r| r[1].as_str()).collect();
        assert_eq!(order, vec!["1", "2", "4", "0", "3"]);
        sort_table_data(&mut t, 0, false);
        let order: Vec<&str> = t.all_table_data.iter().map(|r| r[1].as_str()).collect();
        assert_eq!(order, vec!["0", "3", "1", "2", "4"]);
    }

    #[test]
    fn sort_rows_missing_the_column_are_treated_as_null() {
        let mut t = crate::window_egui::Tabular::new();
        t.current_table_headers = vec!["a".to_string(), "b".to_string()];
        t.all_table_data = vec![
            vec!["x".to_string()],
            vec!["y".to_string(), "2".to_string()],
            vec!["z".to_string(), "1".to_string()],
        ];
        t.page_size = 10;
        sort_table_data(&mut t, 1, true);
        let order: Vec<&str> = t.all_table_data.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(order, vec!["z", "y", "x"]);
    }

    #[test]
    fn sort_key_order_is_total_and_transitive() {
        // Nilai pseudo-acak deterministik (LCG) yang mencampur angka, teks,
        // NULL, kosong, NaN, dan tak hingga.
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as u32
        };
        let special = [
            "NULL", "", "NaN", "inf", "-inf", "-0", "0", "1e2", "abc", "ABC",
        ];
        let values: Vec<String> = (0..300)
            .map(|_| {
                let r = next();
                match r % 5 {
                    0 => special[(r / 5) as usize % special.len()].to_string(),
                    1 => ((r / 5) as i64 % 2000 - 1000).to_string(),
                    2 => format!("{:.2}", ((r / 5) % 10_000) as f64 / 37.0 - 100.0),
                    3 => format!("item{}", (r / 5) % 50),
                    _ => format!("{}x", (r / 5) % 50),
                }
            })
            .collect();
        let keys: Vec<SortKey> = values.iter().map(|v| SortKey::from_cell(Some(v))).collect();

        use std::cmp::Ordering;
        for a in &keys {
            assert_eq!(a.cmp(a), Ordering::Equal);
            for b in &keys {
                // Antisimetri.
                assert_eq!(a.cmp(b), b.cmp(a).reverse());
            }
        }
        // Transitivitas pada sampel tripel (seluruh 300^3 terlalu banyak).
        for i in (0..keys.len()).step_by(3) {
            for j in (0..keys.len()).step_by(5) {
                for k in (0..keys.len()).step_by(7) {
                    let (a, b, c) = (&keys[i], &keys[j], &keys[k]);
                    if a.cmp(b) != Ordering::Greater && b.cmp(c) != Ordering::Greater {
                        assert_ne!(a.cmp(c), Ordering::Greater, "{a:?} {b:?} {c:?}");
                    }
                }
            }
        }

        // Mengurutkan dengan kunci ini tidak panik dan hasilnya terurut.
        let mut sorted = keys.clone();
        sorted.sort();
        assert!(sorted.windows(2).all(|w| w[0] <= w[1]));
    }
}
