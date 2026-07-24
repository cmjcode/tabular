use eframe::egui;
use crate::window_egui;

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

#[derive(Debug, Clone, Default)]
pub(crate) struct GridSummary {
    pub total_cells: usize,
    pub numeric_count: usize,
    pub sum: f64,
    pub avg: f64,
    pub min: f64,
    pub max: f64,
}

pub(crate) fn get_selected_subtable(
    tabular: &window_egui::Tabular,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    if tabular.current_table_data.is_empty() || tabular.current_table_headers.is_empty() {
        return None;
    }

    // 1. Check block selection (table_sel_anchor + selected_cell)
    if let (Some((ar, ac)), Some((br, bc))) = (tabular.table_sel_anchor, tabular.selected_cell) {
        let rmin = ar.min(br);
        let rmax = ar.max(br).min(tabular.current_table_data.len().saturating_sub(1));
        let cmin = ac.min(bc);
        let cmax = ac.max(bc).min(tabular.current_table_headers.len().saturating_sub(1));

        if rmin <= rmax && cmin <= cmax {
            let headers = tabular.current_table_headers[cmin..=cmax].to_vec();
            let mut rows = Vec::new();
            for r in rmin..=rmax {
                if let Some(row) = tabular.current_table_data.get(r) {
                    let sub_row = row[cmin..=cmax].to_vec();
                    rows.push(sub_row);
                }
            }
            return Some((headers, rows));
        }
    }

    // 2. Check row selection
    if !tabular.selected_rows.is_empty() {
        let headers = tabular.current_table_headers.clone();
        let mut rows = Vec::new();
        for (r_idx, row) in tabular.current_table_data.iter().enumerate() {
            if tabular.selected_rows.contains(&r_idx) {
                rows.push(row.clone());
            }
        }
        if !rows.is_empty() {
            return Some((headers, rows));
        }
    }

    // 3. Check column selection
    if !tabular.selected_columns.is_empty() {
        let mut col_indices: Vec<usize> = tabular.selected_columns.iter().copied().collect();
        col_indices.sort_unstable();
        col_indices.retain(|&c| c < tabular.current_table_headers.len());

        if !col_indices.is_empty() {
            let headers: Vec<String> = col_indices
                .iter()
                .map(|&c| tabular.current_table_headers[c].clone())
                .collect();
            let mut rows = Vec::new();
            for row in &tabular.current_table_data {
                let sub_row: Vec<String> = col_indices
                    .iter()
                    .map(|&c| row.get(c).cloned().unwrap_or_default())
                    .collect();
                rows.push(sub_row);
            }
            return Some((headers, rows));
        }
    }

    // 4. Check single cell selection
    if let Some((r, c)) = tabular.selected_cell {
        if r < tabular.current_table_data.len() && c < tabular.current_table_headers.len() {
            let headers = vec![tabular.current_table_headers[c].clone()];
            let val = tabular.current_table_data[r].get(c).cloned().unwrap_or_default();
            let rows = vec![vec![val]];
            return Some((headers, rows));
        }
    }

    None
}

pub(crate) fn calculate_grid_summary(tabular: &window_egui::Tabular) -> Option<GridSummary> {
    let (_, rows) = get_selected_subtable(tabular)?;
    if rows.is_empty() {
        return None;
    }

    let mut summary = GridSummary::default();
    let mut min_val = f64::INFINITY;
    let mut max_val = f64::NEG_INFINITY;

    for row in &rows {
        for cell in row {
            summary.total_cells += 1;
            let clean = cell.replace(',', "").trim().to_string();
            if let Ok(num) = clean.parse::<f64>() {
                if !num.is_nan() {
                    summary.numeric_count += 1;
                    summary.sum += num;
                    if num < min_val {
                        min_val = num;
                    }
                    if num > max_val {
                        max_val = num;
                    }
                }
            }
        }
    }

    if summary.total_cells == 0 {
        return None;
    }

    if summary.numeric_count > 0 {
        summary.avg = summary.sum / (summary.numeric_count as f64);
        summary.min = min_val;
        summary.max = max_val;
    }

    Some(summary)
}

pub(crate) fn copy_selected_as_sql_inserts(
    tabular: &window_egui::Tabular,
    db_type: Option<&crate::models::enums::DatabaseType>,
) -> Option<String> {
    let (headers, rows) = get_selected_subtable(tabular)?;
    if rows.is_empty() || headers.is_empty() {
        return None;
    }
    Some(crate::export::build_sql_inserts(
        &rows,
        &headers,
        &tabular.current_table_name,
        db_type,
    ))
}

pub(crate) fn copy_selected_as_markdown(tabular: &window_egui::Tabular) -> Option<String> {
    let (headers, rows) = get_selected_subtable(tabular)?;
    if rows.is_empty() || headers.is_empty() {
        return None;
    }
    Some(crate::export::build_markdown(&rows, &headers))
}

pub(crate) fn export_selected_to_sql_inserts(
    tabular: &window_egui::Tabular,
    db_type: Option<&crate::models::enums::DatabaseType>,
) {
    if let Some((headers, rows)) = get_selected_subtable(tabular) {
        crate::export::export_to_sql_inserts(&rows, &headers, &tabular.current_table_name, db_type);
    }
}

pub(crate) fn export_selected_to_markdown(tabular: &window_egui::Tabular) {
    if let Some((headers, rows)) = get_selected_subtable(tabular) {
        crate::export::export_to_markdown(&rows, &headers, &tabular.current_table_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_tabular() -> window_egui::Tabular {
        let mut tab = window_egui::Tabular::default();
        tab.current_table_headers = vec!["id".to_string(), "val".to_string(), "name".to_string()];
        tab.current_table_data = vec![
            vec!["1".to_string(), "10.5".to_string(), "Alice".to_string()],
            vec!["2".to_string(), "20.0".to_string(), "Bob".to_string()],
            vec!["3".to_string(), "30.5".to_string(), "Charlie".to_string()],
        ];
        tab
    }

    #[test]
    fn test_grid_summary_block_selection() {
        let mut tab = create_test_tabular();
        tab.table_sel_anchor = Some((0, 0));
        tab.selected_cell = Some((1, 1)); // block 0..=1 rows, 0..=1 cols

        let summary = calculate_grid_summary(&tab).expect("Should compute summary");
        assert_eq!(summary.total_cells, 4);
        assert_eq!(summary.numeric_count, 4);
        assert_eq!(summary.sum, 1.0 + 10.5 + 2.0 + 20.0); // 33.5
        assert_eq!(summary.min, 1.0);
        assert_eq!(summary.max, 20.0);
        assert_eq!(summary.avg, 33.5 / 4.0);
    }

    #[test]
    fn test_selection_export_sql_and_markdown() {
        let mut tab = create_test_tabular();
        tab.selected_rows.insert(0);

        let sql = copy_selected_as_sql_inserts(&tab, None).expect("SQL string");
        assert!(sql.contains("INSERT INTO"));
        assert!(sql.contains("('1', '10.5', 'Alice')"));

        let md = copy_selected_as_markdown(&tab).expect("MD string");
        assert!(md.contains("| id | val | name |"));
        assert!(md.contains("| 1 | 10.5 | Alice |"));
    }
}


