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

// Pagination methods
