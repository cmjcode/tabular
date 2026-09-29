use crate::{models, window_egui::Tabular};
use log::debug;
use std::collections::HashMap;

// This trait provides spreadsheet functionality that can be implemented by any struct
// that has the necessary data fields to support spreadsheet operations
pub trait SpreadsheetOperations {
    // Access to required data fields
    fn get_spreadsheet_state(&self) -> &crate::models::structs::SpreadsheetState;
    fn get_spreadsheet_state_mut(&mut self) -> &mut crate::models::structs::SpreadsheetState;
    fn get_current_table_data(&self) -> &Vec<Vec<String>>;
    fn get_current_table_data_mut(&mut self) -> &mut Vec<Vec<String>>;
    fn get_all_table_data(&self) -> &Vec<Vec<String>>;
    fn get_all_table_data_mut(&mut self) -> &mut Vec<Vec<String>>;
    fn get_current_table_headers(&self) -> &Vec<String>;
    fn get_current_table_name(&self) -> &str;
    fn get_query_tabs(&self) -> &Vec<models::structs::QueryTab>;
    fn get_query_tabs_mut(&mut self) -> &mut Vec<models::structs::QueryTab>;
    fn get_active_tab_index(&self) -> usize;
    fn get_connections(&self) -> &Vec<models::structs::ConnectionConfig>;
    fn get_current_connection_id(&self) -> Option<i64>;
    fn get_total_rows(&self) -> usize;
    fn set_total_rows(&mut self, rows: usize);
    fn get_selected_row(&self) -> Option<usize>;
    fn set_selected_row(&mut self, row: Option<usize>);
    fn get_selected_cell(&self) -> Option<(usize, usize)>;
    fn set_selected_cell(&mut self, cell: Option<(usize, usize)>);
    fn set_table_recently_clicked(&mut self, clicked: bool);
    fn get_use_server_pagination(&self) -> bool;
    fn get_current_base_query(&self) -> &str;
    fn get_is_table_browse_mode(&self) -> bool;
    fn set_error_message(&mut self, message: String);
    fn set_show_error_message(&mut self, show: bool);
    fn get_newly_created_rows_mut(&mut self) -> &mut std::collections::HashSet<usize>;
    fn get_current_column_metadata(&self) -> Option<&Vec<crate::models::structs::ColumnMetadata>>;

    // Methods that need to be implemented by the parent struct
    fn execute_paginated_query(&mut self);
    fn update_current_page_data(&mut self);

    // Method to get primary keys - can be overridden by implementors
    fn get_primary_keys_for_table(
        &self,
        _connection_id: i64,
        _database_name: &str,
        _table_name: &str,
    ) -> Option<Vec<String>> {
        // Default implementation returns None
        // Actual implementations should override this
        None
    }

    // Clear spreadsheet editing state (pending ops, active edit, etc.)
    fn reset_spreadsheet_state(&mut self) {
        *self.get_spreadsheet_state_mut() = crate::models::structs::SpreadsheetState::default();
    }

    // Begin: Spreadsheet helpers
    fn spreadsheet_build_where_clause(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        row_data: &[String],
        headers: &[String],
        primary_keys: &[String],
        overrides: Option<&std::collections::HashMap<String, String>>,
        target_table_for_update: Option<&str>,
    ) -> Option<String>;

    fn spreadsheet_generate_sql(&self) -> Option<String>;

    fn spreadsheet_row_where_all_columns(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        row_index: usize,
    ) -> Option<String>;

    fn spreadsheet_start_cell_edit(&mut self, row: usize, col: usize) {
        if let Some(val) = self
            .get_current_table_data()
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
        {
            let state = self.get_spreadsheet_state_mut();
            state.editing_cell = Some((row, col));
            state.cell_edit_text = val;
        }
    }

    fn spreadsheet_finish_cell_edit(&mut self, save: bool) {
        let editing_cell = self.get_spreadsheet_state().editing_cell;
        if let Some((row, col)) = editing_cell {
            let new_val = self.get_spreadsheet_state().cell_edit_text.clone();
            self.get_spreadsheet_state_mut().cell_edit_text.clear();
            self.get_spreadsheet_state_mut().editing_cell = None;

            if save {
                // Nilai lama dari baris yang tampil; indeks all_table_data
                // disesuaikan dengan halaman aktif oleh grid_cell.
                match self.grid_cell(row, col) {
                    Some(old) if old != new_val => {
                        self.grid_set_cell_raw(row, col, &new_val);

                        // If this row is a freshly inserted row, update its pending InsertRow values instead of pushing an Update
                        let mut updated_insert_row = false;
                        let headers_len = self.get_current_table_headers().len();
                        for op in &mut self.spreadsheet_state.pending_operations {
                            if let crate::models::structs::CellEditOperation::InsertRow {
                                row_index,
                                values,
                            } = op
                                && *row_index == row
                            {
                                if values.len() < headers_len {
                                    values.resize(headers_len, String::new());
                                }
                                if col < values.len() {
                                    values[col] = new_val.clone();
                                }
                                updated_insert_row = true;
                                break;
                            }
                        }
                        // If not an InsertRow case, record as an Update operation
                        if !updated_insert_row {
                            self.spreadsheet_state.pending_operations.push(
                                crate::models::structs::CellEditOperation::Update {
                                    row_index: row,
                                    col_index: col,
                                    old_value: old,
                                    new_value: new_val,
                                },
                            );
                        }
                        self.spreadsheet_state.is_dirty = true;
                    }
                    None => {
                        // Baris tidak ditemukan: tetap tampilkan editnya tanpa operasi pending.
                        self.grid_set_cell_raw(row, col, &new_val);
                    }
                    _ => { /* unchanged value, do nothing */ }
                }
            }
        }
    }
}
