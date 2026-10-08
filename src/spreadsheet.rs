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
                // Get old_val from all_table_data if available, otherwise fall back to current_table_data.
                // In server pagination mode, all_table_data may not contain the current page rows.
                let old_val = self
                    .get_all_table_data()
                    .get(row)
                    .and_then(|r| r.get(col))
                    .cloned()
                    .or_else(|| {
                        self.get_current_table_data()
                            .get(row)
                            .and_then(|r| r.get(col))
                            .cloned()
                    });

                // Salinan baris sebelum diubah: kunci WHERE saat simpan.
                let row_snapshot: Vec<String> = self
                    .get_all_table_data()
                    .get(row)
                    .cloned()
                    .or_else(|| self.get_current_table_data().get(row).cloned())
                    .unwrap_or_default();
                let maybe_old = old_val.clone();
                match maybe_old {
                    Some(ref old) if *old != new_val => {
                        // Update current_table_data
                        if let Some(r1) = self.get_current_table_data_mut().get_mut(row)
                            && let Some(c1) = r1.get_mut(col)
                        {
                            *c1 = new_val.clone();
                        }
                        // Update all_table_data
                        if let Some(r2) = self.get_all_table_data_mut().get_mut(row)
                            && let Some(c2) = r2.get_mut(col)
                        {
                            *c2 = new_val.clone();
                        }

                        // If this row is a freshly inserted row, update its pending InsertRow values instead of pushing an Update
                        let mut updated_insert_row = false;
                        let headers_len = self.get_current_table_headers().len();
                        {
                            let state = self.get_spreadsheet_state_mut();
                            for op in &mut state.pending_operations {
                                if let crate::models::structs::CellEditOperation::InsertRow {
                                    row_index,
                                    values,
                                } = op
                                    && *row_index == row
                                {
                                    // Ensure values vector has enough columns
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
                        }
                        // If not an InsertRow case, record as an Update operation
                        if !updated_insert_row {
                            let state = self.get_spreadsheet_state_mut();
                            state.pending_operations.push(
                                crate::models::structs::CellEditOperation::Update {
                                    row_index: row,
                                    col_index: col,
                                    old_value: old.clone(),
                                    new_value: new_val,
                                    row_values: row_snapshot,
                                },
                            );
                        }
                        self.get_spreadsheet_state_mut().is_dirty = true;
                    }
                    None => {
                        // If old_val is None (e.g., row not present in all_table_data in server pagination),
                        // still update visible data so the edit doesn't disappear. Skip recording pending op.
                        if let Some(r1) = self.get_current_table_data_mut().get_mut(row)
                            && let Some(c1) = r1.get_mut(col)
                        {
                            *c1 = new_val.clone();
                        }
                        if let Some(r2) = self.get_all_table_data_mut().get_mut(row)
                            && let Some(c2) = r2.get_mut(col)
                        {
                            *c2 = new_val.clone();
                        }
                    }
                    _ => { /* unchanged value, do nothing */ }
                }
            }
        }
    }

    fn spreadsheet_add_row(&mut self) {
        let new_row: Vec<String> = self
            .get_current_table_headers()
            .iter()
            .map(|_| String::new())
            .collect();
        let row_index = self.get_all_table_data().len();
        self.get_all_table_data_mut().push(new_row.clone());
        self.get_current_table_data_mut().push(new_row.clone());
        self.set_total_rows(self.get_total_rows().saturating_add(1));

        let state = self.get_spreadsheet_state_mut();
        state
            .pending_operations
            .push(crate::models::structs::CellEditOperation::InsertRow {
                row_index,
                values: new_row,
            });
        state.is_dirty = true;

        self.set_selected_row(Some(row_index));
        self.set_selected_cell(Some((row_index, 0)));
        self.set_table_recently_clicked(true);
        self.spreadsheet_start_cell_edit(row_index, 0);
    }

    fn spreadsheet_delete_selected_row(&mut self) {
        debug!(
            "🔥 spreadsheet_delete_selected_row called, selected_row: {:?}",
            self.get_selected_row()
        );

        if let Some(row) = self.get_selected_row() {
            // Get the row values BEFORE removing from any data structures
            let values = if let Some(values) = self.get_all_table_data().get(row).cloned() {
                values
            } else if let Some(values) = self.get_current_table_data().get(row).cloned() {
                values
            } else {
                debug!("🔥 Could not get values for row {}", row);
                return;
            };

            let state = self.get_spreadsheet_state_mut();
            state
                .pending_operations
                .push(crate::models::structs::CellEditOperation::DeleteRow {
                    row_index: row,
                    values: values.clone(),
                });
            state.is_dirty = true;

            // Now remove from data structures
            if row < self.get_current_table_data().len() {
                self.get_current_table_data_mut().remove(row);
            }
            if row < self.get_all_table_data().len() {
                self.get_all_table_data_mut().remove(row);
            }
            self.set_total_rows(self.get_total_rows().saturating_sub(1));
            self.set_selected_row(None);
            self.set_selected_cell(None);

            // Update tab state
            let current_data = self.get_current_table_data().clone();
            let all_data = self.get_all_table_data().clone();
            let total = self.get_total_rows();
            let idx = self.get_active_tab_index();

            if let Some(active_tab) = self.get_query_tabs_mut().get_mut(idx) {
                active_tab.result_rows = current_data;
                active_tab.result_all_rows = all_data;
                active_tab.total_rows = total;
            }
        } else {
            debug!("🔥 No row selected for deletion");
        }
    }

    fn spreadsheet_duplicate_selected_row(&mut self) {
        if let Some(selected_row_idx) = self.get_selected_row() {
            let current_len = self.get_current_table_data().len();
            if selected_row_idx >= current_len {
                return;
            }

            // Clone the row data
            let row_data = self.get_current_table_data()[selected_row_idx].clone();

            // Insert the duplicated row right after the selected row
            let insert_index = selected_row_idx + 1;

            // Insert into data structures
            self.get_current_table_data_mut()
                .insert(insert_index, row_data.clone());
            // Safe insert into all_table_data
            if insert_index <= self.get_all_table_data().len() {
                self.get_all_table_data_mut()
                    .insert(insert_index, row_data.clone());
            } else {
                self.get_all_table_data_mut().push(row_data.clone());
            }

            // Update total rows count
            self.set_total_rows(self.get_current_table_data().len());

            // Mark this row as newly created for highlighting
            self.get_newly_created_rows_mut().insert(insert_index);

            // Update indices in newly_created_rows for rows that shifted down
            // We need to collect first to avoid mutation issues
            let mut rows_to_shift = Vec::new();
            for &row_idx in self.get_newly_created_rows_mut().iter() {
                if row_idx > insert_index {
                    rows_to_shift.push(row_idx);
                }
            }

            for row_idx in rows_to_shift {
                self.get_newly_created_rows_mut().remove(&row_idx);
                self.get_newly_created_rows_mut().insert(row_idx + 1);
            }

            // Select the new duplicated row
            self.set_selected_row(Some(insert_index));
            self.set_selected_cell(Some((insert_index, 0)));

            // Mark spreadsheet as dirty
            let state = self.get_spreadsheet_state_mut();
            state.is_dirty = true;

            // Create an insert operation for tracking
            state
                .pending_operations
                .push(crate::models::structs::CellEditOperation::InsertRow {
                    row_index: insert_index,
                    values: row_data,
                });

            // Update tab state
            let current_data = self.get_current_table_data().clone();
            let all_data = self.get_all_table_data().clone();
            let total = self.get_total_rows();
            let idx = self.get_active_tab_index();

            if let Some(active_tab) = self.get_query_tabs_mut().get_mut(idx) {
                active_tab.result_rows = current_data;
                active_tab.result_all_rows = all_data;
                active_tab.total_rows = total;
            }

            debug!(
                "Row {} duplicated successfully. New row at index {}",
                selected_row_idx, insert_index
            );
        }
    }

    fn spreadsheet_extract_table_name(&self) -> Option<String> {
        debug!(
            "🔥 spreadsheet_extract_table_name called with current_table_name: '{}'",
            self.get_current_table_name()
        );

        if self.get_current_table_name().starts_with("Table: ") {
            let s = self.get_current_table_name().strip_prefix("Table: ")?;
            let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
            debug!("🔥 Extracted table name: {:?}", result);
            result
        } else {
            // Try to extract from active tab if it's a table browse tab
            if let Some(tab) = self.get_query_tabs().get(self.get_active_tab_index()) {
                debug!("🔥 Checking active tab title: '{}'", tab.title);
                if tab.title.starts_with("Table: ") {
                    let s = tab.title.strip_prefix("Table: ")?;
                    let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
                    debug!("🔥 Extracted table name from tab: {:?}", result);
                    return result;
                }
            }
            debug!("🔥 Table name does not start with 'Table: ' and no suitable tab found");
            None
        }
    }

    fn spreadsheet_extract_database_name(&self) -> Option<String> {
        if self.get_current_table_name().contains("(Database:")
            && let Some(start) = self.get_current_table_name().find("(Database:")
        {
            let after = &self.get_current_table_name()[start + "(Database:".len()..];
            if let Some(end) = after.find(')') {
                let name = after[..end].trim();
                if !name.is_empty() && !name.eq_ignore_ascii_case("unknown") {
                    return Some(name.to_string());
                }
            }
        }

        if let Some(tab) = self.get_query_tabs().get(self.get_active_tab_index())
            && let Some(db) = tab.database_name.clone()
            && !db.is_empty()
            && !db.eq_ignore_ascii_case("unknown")
        {
            return Some(db);
        }

        None
    }

    fn spreadsheet_quote_ident(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        ident: &str,
    ) -> String {
        match conn.connection_type {
            crate::models::enums::DatabaseType::MySQL => format!("`{}`", ident),
            crate::models::enums::DatabaseType::PostgreSQL => format!("\"{}\"", ident),
            crate::models::enums::DatabaseType::MsSQL => format!("[{}]", ident),
            crate::models::enums::DatabaseType::SQLite => format!("\"{}\"", ident),
            _ => ident.to_string(),
        }
    }

    // Quote a possibly schema-qualified table identifier appropriately per-DB.
    // Examples:
    // - MySQL: schema.table -> `schema`.`table`
    // - PostgreSQL: schema.table -> "schema"."table"
    // - MsSQL: schema.table -> [schema].[table]
    // - SQLite: table -> "table" (no schemas)
    fn spreadsheet_quote_table_ident(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        ident: &str,
    ) -> String {
        // If identifier already appears quoted for the target DB, return as-is
        let already_mysql = ident.contains('`');
        let already_pg_sqlite = ident.contains('"');
        let already_mssql = ident.contains('[') && ident.contains(']');

        match conn.connection_type {
            crate::models::enums::DatabaseType::MySQL => {
                if already_mysql {
                    return ident.to_string();
                }
                if ident.contains('.') {
                    ident
                        .split('.')
                        .map(|p| format!("`{}`", p))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    format!("`{}`", ident)
                }
            }
            crate::models::enums::DatabaseType::PostgreSQL
            | crate::models::enums::DatabaseType::SQLite => {
                if already_pg_sqlite {
                    return ident.to_string();
                }
                if ident.contains('.') {
                    ident
                        .split('.')
                        .map(|p| format!("\"{}\"", p))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    format!("\"{}\"", ident)
                }
            }
            crate::models::enums::DatabaseType::MsSQL => {
                if already_mssql {
                    return ident.to_string();
                }
                if ident.contains('.') {
                    ident
                        .split('.')
                        .map(|p| format!("[{}]", p.trim_matches(['[', ']'])))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    format!("[{}]", ident.trim_matches(['[', ']']))
                }
            }
            _ => ident.to_string(),
        }
    }

    fn spreadsheet_quote_value(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        v: &str,
    ) -> String {
        // NULL/kosong → NULL, nilai mentah (DEFAULT/NOW) apa adanya, sisanya
        // di-quote dengan escape sesuai dialek.
        crate::data_table::grid_model::quote_literal(&conn.connection_type, v)
    }

    fn spreadsheet_save_changes(&mut self);

    /// Jalankan statement simpan grid (satu transaksi bila koneksinya
    /// mendukung) dan tangani hasilnya.
    fn execute_spreadsheet_statements(
        &mut self,
        statements: Vec<crate::connection::atomic::TransactionalStatement>,
    );
}

// Implement the SpreadsheetOperations trait for Tabular
impl SpreadsheetOperations for Tabular {
    fn get_spreadsheet_state(&self) -> &crate::models::structs::SpreadsheetState {
        &self.spreadsheet_state
    }

    fn get_spreadsheet_state_mut(&mut self) -> &mut crate::models::structs::SpreadsheetState {
        &mut self.spreadsheet_state
    }

    fn get_current_table_data(&self) -> &Vec<Vec<String>> {
        &self.current_table_data
    }

    fn get_current_table_data_mut(&mut self) -> &mut Vec<Vec<String>> {
        &mut self.current_table_data
    }

    fn get_all_table_data(&self) -> &Vec<Vec<String>> {
        &self.all_table_data
    }

    fn get_all_table_data_mut(&mut self) -> &mut Vec<Vec<String>> {
        &mut self.all_table_data
    }

    fn get_current_table_headers(&self) -> &Vec<String> {
        &self.current_table_headers
    }

    fn get_current_table_name(&self) -> &str {
        &self.current_table_name
    }

    fn get_current_column_metadata(&self) -> Option<&Vec<crate::models::structs::ColumnMetadata>> {
        self.current_column_metadata.as_ref()
    }

    fn get_query_tabs(&self) -> &Vec<models::structs::QueryTab> {
        &self.query_tabs
    }

    fn get_query_tabs_mut(&mut self) -> &mut Vec<models::structs::QueryTab> {
        &mut self.query_tabs
    }

    fn get_active_tab_index(&self) -> usize {
        self.active_tab_index
    }

    fn get_connections(&self) -> &Vec<models::structs::ConnectionConfig> {
        &self.connections
    }

    fn get_current_connection_id(&self) -> Option<i64> {
        self.current_connection_id
    }

    fn get_total_rows(&self) -> usize {
        self.total_rows
    }

    fn set_total_rows(&mut self, rows: usize) {
        self.total_rows = rows;
    }

    fn get_selected_row(&self) -> Option<usize> {
        self.selected_row
    }

    fn set_selected_row(&mut self, row: Option<usize>) {
        self.selected_row = row;
    }

    fn get_selected_cell(&self) -> Option<(usize, usize)> {
        self.selected_cell
    }

    fn set_selected_cell(&mut self, cell: Option<(usize, usize)>) {
        self.selected_cell = cell;
    }

    fn set_table_recently_clicked(&mut self, clicked: bool) {
        self.table_recently_clicked = clicked;
    }

    fn get_use_server_pagination(&self) -> bool {
        self.use_server_pagination
    }

    fn get_current_base_query(&self) -> &str {
        &self.current_base_query
    }

    fn get_is_table_browse_mode(&self) -> bool {
        self.is_table_browse_mode
    }

    fn set_error_message(&mut self, message: String) {
        self.error_message = message;
    }

    fn set_show_error_message(&mut self, show: bool) {
        self.show_error_message = show;
    }

    fn get_newly_created_rows_mut(&mut self) -> &mut std::collections::HashSet<usize> {
        &mut self.newly_created_rows
    }

    fn execute_paginated_query(&mut self) {
        // Call the existing method
        self.execute_paginated_query();
    }

    fn update_current_page_data(&mut self) {
        // Update the current page data for client-side pagination
        let start = self.current_page * self.page_size;
        let end = std::cmp::min(start + self.page_size, self.all_table_data.len());
        if start < end && end <= self.all_table_data.len() {
            self.current_table_data = self.all_table_data[start..end].to_vec();
            self.total_rows = self.current_table_data.len();
        } else {
            self.current_table_data.clear();
            self.total_rows = 0;
        }
    }

    fn get_primary_keys_for_table(
        &self,
        connection_id: i64,
        database_name: &str,
        table_name: &str,
    ) -> Option<Vec<String>> {
        // Query PRIMARY KEY from index_cache (SQLite cache)
        if let Some(ref pool) = self.db_pool {
            let pool_clone = pool.clone();
            let db_name = database_name.to_string();
            let tbl_name = table_name.to_string();

            let fut = async move {
                // Query index_cache for PRIMARY index
                let row_opt = sqlx::query(
                    "SELECT columns_json FROM index_cache 
                     WHERE connection_id = ? AND database_name = ? AND table_name = ? AND index_name = 'PRIMARY'"
                )
                .bind(connection_id)
                .bind(&db_name)
                .bind(&tbl_name)
                .fetch_optional(pool_clone.as_ref())
                .await
                .map_err(|e| format!("Failed to query index_cache: {}", e))?;

                if let Some(row) = row_opt {
                    use sqlx::Row as _;
                    let columns_json: String = row
                        .try_get(0)
                        .map_err(|e| format!("Failed to get columns_json: {}", e))?;
                    let columns: Vec<String> = serde_json::from_str(&columns_json)
                        .map_err(|e| format!("Failed to parse columns_json: {}", e))?;
                    Ok::<Vec<String>, String>(columns)
                } else {
                    Ok::<Vec<String>, String>(Vec::new())
                }
            };

            let result: Result<Vec<String>, String> = if let Some(ref rt) = self.runtime {
                rt.block_on(fut)
            } else {
                tokio::runtime::Runtime::new().ok()?.block_on(fut)
            };

            match result {
                Ok(pks) if !pks.is_empty() => {
                    debug!(
                        "✅ Found {} primary key(s) from cache for {}.{}: {:?}",
                        pks.len(),
                        database_name,
                        table_name,
                        pks
                    );
                    Some(pks)
                }
                Ok(_) => {
                    debug!(
                        "⚠️ No primary key found in cache for {}.{}",
                        database_name, table_name
                    );
                    None
                }
                Err(e) => {
                    debug!("⚠️ Failed to get primary keys from cache: {}", e);
                    None
                }
            }
        } else {
            debug!("⚠️ No db_pool available");
            None
        }
    }

    fn execute_spreadsheet_statements(
        &mut self,
        statements: Vec<crate::connection::atomic::TransactionalStatement>,
    ) {
        let Some(conn_id) = self.current_connection_id else {
            return;
        };
        // Jumlah operasi yang ikut disimpan. Edit yang dibuat selama proses
        // simpan berjalan tidak boleh ikut terhapus saat simpan sukses.
        let submitted_ops = self.spreadsheet_state.pending_operations.len();
        let saved_ops_snapshot = self.spreadsheet_state.pending_operations.clone();
        // SQL pembalik (Data Rewind) disiapkan oleh commit_pending_changes.
        let rewind = self.grid_ext.pending_rewind.take();
        self.run_grid_save(
            conn_id,
            statements,
            Box::new(move |tabular, message| {
                if !message.success {
                    let msg = message
                        .error
                        .clone()
                        .unwrap_or_else(|| "Unknown query error".to_string());
                    debug!("❌ Spreadsheet save failed: {}", msg);
                    // Operasi tetap disimpan agar user bisa memperbaiki lalu mencoba lagi.
                    tabular
                        .toasts
                        .error(format!("Failed to save table changes: {}", msg));
                    return;
                }
                debug!("🔥 SQL executed successfully, clearing saved pending operations");
                let state = &mut tabular.spreadsheet_state;
                let saved = submitted_ops.min(state.pending_operations.len());
                // Antrean bisa berubah selama simpan berjalan (undo, hapus baris
                // baru). Buang prefiks hanya bila masih sama dengan yang dikirim.
                let queue_intact = state.pending_operations[..saved] == saved_ops_snapshot[..saved];
                if queue_intact {
                    state.pending_operations.drain(..saved);
                } else {
                    log::warn!("[GRID] antrean edit berubah selama simpan; muat ulang tabel");
                    state.pending_operations.clear();
                }
                state.is_dirty = !state.pending_operations.is_empty();
                if state.pending_operations.is_empty() {
                    // Clear newly created rows highlight after successful save
                    tabular.newly_created_rows.clear();
                }
                if queue_intact {
                    // Baris yang dihapus baru hilang dari grid setelah commit sukses.
                    tabular.grid_drop_saved_deleted_rows(&saved_ops_snapshot[..saved]);
                }
                // Commit adalah batas undo; pemulihan setelahnya lewat Data Rewind.
                tabular.grid_clear_history();
                if let Some(rewind) = rewind {
                    tabular.grid_push_rewind(conn_id, rewind);
                }
                match message.affected_rows {
                    Some(n) => tabular
                        .toasts
                        .success(format!("Saved changes ({} row(s) affected)", n)),
                    None => tabular.toasts.success("Saved changes"),
                }

                // Muat ulang dari server agar baris baru tampil dengan nilai
                // sebenarnya (DEFAULT, auto-increment) alih-alih placeholder.
                if tabular.is_table_browse_mode {
                    if tabular.use_server_pagination && !tabular.current_base_query.is_empty() {
                        tabular.execute_paginated_query();
                    } else {
                        crate::data_table::refresh_current_table_data(tabular);
                    }
                }
            }),
        );
    }

    fn reset_spreadsheet_state(&mut self) {
        *self.get_spreadsheet_state_mut() = crate::models::structs::SpreadsheetState::default();
        self.grid_clear_history();
    }

    fn spreadsheet_start_cell_edit(&mut self, row: usize, col: usize) {
        // Baris yang ditandai hapus tidak bisa diedit sampai dipulihkan.
        if crate::data_table::grid_model::pending_deleted_rows(
            &self.spreadsheet_state.pending_operations,
        )
        .contains(&row)
        {
            return;
        }
        if let Some(val) = self
            .get_current_table_data()
            .get(row)
            .and_then(|r| r.get(col))
            .cloned()
        {
            // Nilai mentah (DEFAULT/NOW) tidak diedit sebagai teks: editor
            // dimulai kosong dan nilai asal dipakai lagi bila tidak diketik.
            let text = if crate::data_table::grid_model::as_raw_sql(&val).is_some() {
                self.grid_ext.raw_edit_origin = Some(val);
                String::new()
            } else {
                self.grid_ext.raw_edit_origin = None;
                val
            };
            let state = self.get_spreadsheet_state_mut();
            state.editing_cell = Some((row, col));
            state.cell_edit_text = text;
        }
    }

    fn spreadsheet_finish_cell_edit(&mut self, save: bool) {
        if let Some(origin) = self.grid_ext.raw_edit_origin.take()
            && self.spreadsheet_state.cell_edit_text.is_empty()
        {
            self.spreadsheet_state.cell_edit_text = origin;
        }
        let editing_cell = self.get_spreadsheet_state().editing_cell;
        // Snapshot untuk undo: nilai sel dan antrean sebelum edit diterapkan.
        let undo_snapshot = editing_cell.and_then(|(r, c)| {
            self.grid_cell(r, c).map(|before| {
                (
                    r,
                    c,
                    before,
                    self.spreadsheet_state.pending_operations.clone(),
                )
            })
        });
        self.spreadsheet_finish_cell_edit_inner(save, editing_cell);
        if save && let Some((row, col, before, ops_before)) = undo_snapshot {
            let after = self.grid_cell(row, col).unwrap_or_default();
            if after != before {
                self.grid_record(
                    "Edit cell",
                    ops_before,
                    vec![crate::data_table::grid_state::DataChange::Cell {
                        row,
                        col,
                        before,
                        after,
                    }],
                );
            }
        }
    }

    fn spreadsheet_add_row(&mut self) {
        if self.spreadsheet_state.editing_cell.is_some() {
            self.spreadsheet_finish_cell_edit(true);
        }
        let ops_before = self.spreadsheet_state.pending_operations.clone();
        // Indeks mengikuti baris yang tampil agar edit sel baru cocok dengan
        // operasi InsertRow-nya.
        let row_index = self.current_table_data.len();
        let values: Vec<String> = self
            .current_table_headers
            .iter()
            .map(|_| {
                crate::data_table::grid_model::raw_sql_value(
                    crate::data_table::grid_model::RAW_DEFAULT,
                )
            })
            .collect();
        self.grid_insert_row_raw(row_index, values.clone());
        self.spreadsheet_state.pending_operations.push(
            crate::models::structs::CellEditOperation::InsertRow {
                row_index,
                values: values.clone(),
            },
        );
        self.spreadsheet_state.is_dirty = true;
        self.grid_record(
            "Add row",
            ops_before,
            vec![crate::data_table::grid_state::DataChange::InsertRow {
                row: row_index,
                values,
            }],
        );

        self.selected_row = Some(row_index);
        self.selected_cell = Some((row_index, 0));
        self.table_recently_clicked = true;
        self.scroll_to_selected_cell = true;
        self.spreadsheet_start_cell_edit(row_index, 0);
    }

    fn spreadsheet_delete_selected_row(&mut self) {
        let mut rows: Vec<usize> = self.selected_rows.iter().copied().collect();
        if rows.is_empty()
            && let Some(row) = self.selected_row
        {
            rows.push(row);
        }
        if !rows.is_empty() {
            self.grid_toggle_delete_rows(rows);
        }
    }

    fn spreadsheet_duplicate_selected_row(&mut self) {
        if self.spreadsheet_state.editing_cell.is_some() {
            self.spreadsheet_finish_cell_edit(true);
        }
        let Some(selected) = self.selected_row else {
            return;
        };
        let Some(mut values) = self.current_table_data.get(selected).cloned() else {
            return;
        };
        // Kolom primary key diisi DEFAULT agar salinan tidak bentrok kunci.
        // Di mode browse metadata kosong, jadi PK dibaca dari cache lokal
        // (tanpa query ke server di thread UI).
        let pk_from_meta: Vec<usize> = self
            .current_column_metadata
            .as_ref()
            .map(|meta| {
                meta.iter()
                    .enumerate()
                    .filter(|(_, m)| m.is_primary_key)
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        let mut pk_names = self.spreadsheet_state.primary_key_columns.clone();
        if pk_from_meta.is_empty()
            && pk_names.is_empty()
            && let (Some(conn_id), Some(table)) = (
                self.current_connection_id,
                self.spreadsheet_extract_table_name(),
            )
        {
            let db = self.spreadsheet_extract_database_name().unwrap_or_default();
            pk_names = crate::cache_data::get_primary_keys_from_cache(self, conn_id, &db, &table)
                .unwrap_or_default();
        }
        for (i, header) in self.current_table_headers.iter().enumerate() {
            let is_pk = pk_from_meta.contains(&i)
                || pk_names.iter().any(|pk| pk.eq_ignore_ascii_case(header));
            if is_pk && let Some(v) = values.get_mut(i) {
                *v = crate::data_table::grid_model::raw_sql_value(
                    crate::data_table::grid_model::RAW_DEFAULT,
                );
            }
        }
        let ops_before = self.spreadsheet_state.pending_operations.clone();
        let insert_index = selected + 1;
        crate::data_table::grid_model::shift_op_rows(
            &mut self.spreadsheet_state.pending_operations,
            insert_index,
            1,
        );
        self.grid_insert_row_raw(insert_index, values.clone());
        self.spreadsheet_state.pending_operations.push(
            crate::models::structs::CellEditOperation::InsertRow {
                row_index: insert_index,
                values: values.clone(),
            },
        );
        self.spreadsheet_state.is_dirty = true;
        self.grid_record(
            "Duplicate row",
            ops_before,
            vec![crate::data_table::grid_state::DataChange::InsertRow {
                row: insert_index,
                values,
            }],
        );
        self.selected_row = Some(insert_index);
        self.selected_cell = Some((insert_index, 0));
        self.selected_rows.clear();
        self.scroll_to_selected_cell = true;
    }

    fn spreadsheet_extract_table_name(&self) -> Option<String> {
        debug!(
            "🔥 spreadsheet_extract_table_name called with current_table_name: '{}'",
            self.get_current_table_name()
        );

        if self.get_current_table_name().starts_with("Table: ") {
            let s = self.get_current_table_name().strip_prefix("Table: ")?;
            let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
            debug!("🔥 Extracted table name: {:?}", result);
            result
        } else {
            // Try to extract from active tab if it's a table browse tab
            if let Some(tab) = self.get_query_tabs().get(self.get_active_tab_index()) {
                debug!("🔥 Checking active tab title: '{}'", tab.title);
                if tab.title.starts_with("Table: ") {
                    let s = tab.title.strip_prefix("Table: ")?;
                    let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
                    debug!("🔥 Extracted table name from tab: {:?}", result);
                    return result;
                }
            }
            debug!("🔥 Table name does not start with 'Table: ' and no suitable tab found");
            None
        }
    }

    fn spreadsheet_extract_database_name(&self) -> Option<String> {
        if self.get_current_table_name().contains("(Database:")
            && let Some(start) = self.get_current_table_name().find("(Database:")
        {
            let after = &self.get_current_table_name()[start + "(Database:".len()..];
            if let Some(end) = after.find(')') {
                let name = after[..end].trim();
                if !name.is_empty() && !name.eq_ignore_ascii_case("unknown") {
                    return Some(name.to_string());
                }
            }
        }

        if let Some(tab) = self.get_query_tabs().get(self.get_active_tab_index())
            && let Some(db) = tab.database_name.clone()
            && !db.is_empty()
            && !db.eq_ignore_ascii_case("unknown")
        {
            return Some(db);
        }

        None
    }

    fn spreadsheet_build_where_clause(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        row_data: &[String],
        headers: &[String],
        primary_keys: &[String],
        overrides: Option<&HashMap<String, String>>,
        target_table_for_update: Option<&str>,
    ) -> Option<String> {
        let qt = |s: &str| self.spreadsheet_quote_ident(conn, s);
        let qv = |s: &str| self.spreadsheet_quote_value(conn, s);
        let metadata = self.get_current_column_metadata();

        let mut where_parts = Vec::new();
        // If we have a target table, we ONLY want to scope the WHERE clause to PKs of that table.
        let use_metadata_filtering = target_table_for_update.is_some() && metadata.is_some();

        if use_metadata_filtering {
            let target_table = target_table_for_update.unwrap();
            let meta = metadata.as_ref().unwrap();
            debug!(
                "🔥 spreadsheet_build_where_clause: filtering for target_table='{}'",
                target_table
            );

            for (i, col_meta) in meta.iter().enumerate() {
                let belongs_to_table = col_meta.table_name.as_deref().unwrap_or("") == target_table;

                if belongs_to_table && col_meta.is_primary_key {
                    if let Some(col_name) = headers.get(i) {
                        debug!("🔥 Found matching PK: '{}' at index {}", col_name, i);
                        let id_name = col_meta.original_name.clone().unwrap_or(col_name.clone());
                        let mut val = row_data.get(i).cloned().unwrap_or_default();
                        if let Some(ov) = overrides
                            && let Some(v) = ov.get(&col_name.to_lowercase())
                        {
                            val = v.clone();
                        }

                        let clause = if val.to_uppercase() == "NULL" {
                            format!("{} IS NULL", qt(&id_name))
                        } else {
                            format!("{} = {}", qt(&id_name), qv(&val))
                        };
                        where_parts.push(clause);
                    }
                } else if belongs_to_table {
                    // Debug why non-PK was skipped
                    // debug!("🔥 Skipping column '{}' (is_pk={}) for table match", col_meta.name, col_meta.is_primary_key);
                }
            }
        } else {
            debug!(
                "🔥 spreadsheet_build_where_clause: NO metadata filtering (target={:?}, meta={})",
                target_table_for_update,
                metadata.is_some()
            );
        }

        if where_parts.is_empty() {
            debug!(
                "🔥 spreadsheet_build_where_clause: where_parts was empty, using FALLBACK logic"
            );
            for (i, header) in headers.iter().enumerate() {
                // NEW: Security check - if we have metadata, ensure this column belongs to target table
                // This prevents adding columns from joined tables (e.g. date_time) to the WHERE clause
                // when updating a specific table (e.g. user_data).
                if let Some(target) = target_table_for_update
                    && let Some(meta) = metadata.as_ref()
                    && let Some(col_meta) = meta.get(i)
                {
                    let tbl = col_meta.table_name.as_deref().unwrap_or("");
                    // Only skip if table name is explicitly known and differs from target.
                    // Use case-insensitive check to be safe.
                    if !tbl.is_empty() && !tbl.eq_ignore_ascii_case(target) {
                        debug!(
                            "🔥 Fallback skipping column '{}' because it belongs to table '{}' (target='{}')",
                            header, tbl, target
                        );
                        continue;
                    }
                }

                if (primary_keys.is_empty()
                    || primary_keys
                        .iter()
                        .any(|pk| pk.eq_ignore_ascii_case(header)))
                    && let Some(val_ref) = row_data.get(i)
                {
                    let mut val = val_ref.clone();
                    if let Some(ov) = overrides
                        && let Some(v) = ov.get(&header.to_lowercase())
                    {
                        val = v.clone();
                    }
                    let col_name_for_where = metadata
                        .as_ref()
                        .and_then(|meta| meta.get(i))
                        .and_then(|m| m.original_name.clone())
                        .unwrap_or_else(|| header.clone());
                    let clause = if val.to_uppercase() == "NULL" {
                        format!("{} IS NULL", qt(&col_name_for_where))
                    } else {
                        format!("{} = {}", qt(&col_name_for_where), qv(&val))
                    };
                    where_parts.push(clause);
                }
            }
        }

        if where_parts.is_empty() {
            // Second fallback logic (implicit ID detection from old code)
            if primary_keys.is_empty()
                && let (Some(first_header), Some(first_value)) = (headers.first(), row_data.first())
            {
                let lower = first_header.to_lowercase();
                if lower.contains("id") || lower.contains("recid") || lower == "pk" {
                    let clause = if crate::data_table::grid_model::is_null_cell(first_value) {
                        format!("{} IS NULL", qt(first_header))
                    } else {
                        format!("{} = {}", qt(first_header), qv(first_value))
                    };
                    return Some(clause);
                }
            }
            None
        } else {
            Some(where_parts.join(" AND "))
        }
    }

    fn spreadsheet_quote_ident(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        ident: &str,
    ) -> String {
        match conn.connection_type {
            crate::models::enums::DatabaseType::MySQL => std::format!("`{}`", ident),
            crate::models::enums::DatabaseType::PostgreSQL => std::format!("\"{}\"", ident),
            crate::models::enums::DatabaseType::MsSQL => std::format!("[{}]", ident),
            crate::models::enums::DatabaseType::SQLite => std::format!("\"{}\"", ident),
            _ => ident.to_string(),
        }
    }

    fn spreadsheet_quote_table_ident(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        ident: &str,
    ) -> String {
        // If identifier already appears quoted for the target DB, return as-is
        let already_mysql = ident.contains('`');
        let already_pg_sqlite = ident.contains('"');
        let already_mssql = ident.contains('[') && ident.contains(']');

        match conn.connection_type {
            crate::models::enums::DatabaseType::MySQL => {
                if already_mysql {
                    return ident.to_string();
                }
                if ident.contains('.') {
                    ident
                        .split('.')
                        .map(|p| std::format!("`{}`", p))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    std::format!("`{}`", ident)
                }
            }
            crate::models::enums::DatabaseType::PostgreSQL
            | crate::models::enums::DatabaseType::SQLite => {
                if already_pg_sqlite {
                    return ident.to_string();
                }
                if ident.contains('.') {
                    ident
                        .split('.')
                        .map(|p| std::format!("\"{}\"", p))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    std::format!("\"{}\"", ident)
                }
            }
            crate::models::enums::DatabaseType::MsSQL => {
                if already_mssql {
                    return ident.to_string();
                }
                if ident.contains('.') {
                    ident
                        .split('.')
                        .map(|p| std::format!("[{}]", p.trim_matches(['[', ']'])))
                        .collect::<Vec<_>>()
                        .join(".")
                } else {
                    std::format!("[{}]", ident.trim_matches(['[', ']']))
                }
            }
            _ => ident.to_string(),
        }
    }

    fn spreadsheet_quote_value(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        v: &str,
    ) -> String {
        // NULL/kosong → NULL, nilai mentah (DEFAULT/NOW) apa adanya, sisanya
        // di-quote dengan escape sesuai dialek.
        crate::data_table::grid_model::quote_literal(&conn.connection_type, v)
    }

    fn spreadsheet_generate_sql(&self) -> Option<String> {
        let statements = self.spreadsheet_generate_statements();
        if statements.is_empty() {
            None
        } else {
            Some(
                statements
                    .into_iter()
                    .map(|s| s.sql)
                    .collect::<Vec<_>>()
                    .join(";\n"),
            )
        }
    }

    fn spreadsheet_save_changes(&mut self) {
        debug!(
            "🔥 spreadsheet_save_changes called with {} pending operations",
            self.get_spreadsheet_state().pending_operations.len()
        );
        debug!(
            "spreadsheet_save_changes called with {} pending operations",
            self.get_spreadsheet_state().pending_operations.len()
        );

        if self.get_spreadsheet_state().pending_operations.is_empty() {
            debug!("No pending operations to save");
            return;
        }

        // Ensure primary key columns are available before generating SQL.
        self.spreadsheet_ensure_primary_keys();

        let statements = self.spreadsheet_generate_statements();
        if statements.is_empty() {
            debug!("Failed to generate SQL");
            return;
        }
        if self.get_current_connection_id().is_none() {
            debug!("No current connection ID");
            return;
        }
        self.execute_spreadsheet_statements(statements);
        self.spreadsheet_finish_cell_edit(false);
    }

    // Override to use primary keys from cache
    fn spreadsheet_row_where_all_columns(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        row_index: usize,
    ) -> Option<String> {
        let row = self
            .get_current_table_data()
            .get(row_index)
            .or_else(|| self.get_all_table_data().get(row_index))?;
        let headers = self.get_current_table_headers();
        let pk_columns = &self.get_spreadsheet_state().primary_key_columns;
        self.spreadsheet_build_where_clause(conn, row, headers, pk_columns, None, None)
    }
}

impl Tabular {
    /// Isi `primary_key_columns` sebelum SQL simpan dibuat. Di mode browse
    /// `current_column_metadata` kosong, jadi PK diambil dari cache lalu,
    /// bila belum ada, langsung dari database.
    /// Statement DML untuk antrean edit grid, urut sesuai antrean. Semua edit
    /// pada satu baris digabung menjadi satu `UPDATE ... SET a = .., b = ..`
    /// supaya WHERE (dari nilai asli baris) tetap cocok setelah kolom pertama
    /// berubah, juga pada tabel tanpa primary key. `expected_rows` = 1 untuk
    /// UPDATE/DELETE: eksekusi transaksional membatalkan semuanya bila sebuah
    /// statement mengenai 0 atau lebih dari 1 baris.
    pub(crate) fn spreadsheet_generate_statements(
        &self,
    ) -> Vec<crate::connection::atomic::TransactionalStatement> {
        use crate::connection::atomic::TransactionalStatement;
        use crate::models::structs::CellEditOperation;

        let Some(conn_id) = self.get_current_connection_id() else {
            return Vec::new();
        };
        let Some(conn) = self
            .get_connections()
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()
        else {
            return Vec::new();
        };
        let table = self.spreadsheet_extract_table_name();
        let qt = |s: &str| self.spreadsheet_quote_ident(&conn, s);
        let qt_table = |s: &str| self.spreadsheet_quote_table_ident(&conn, s);
        let qv = |s: &str| self.spreadsheet_quote_value(&conn, s);

        let headers = self.get_current_table_headers();
        let all_rows = self.get_all_table_data();
        let current_rows = self.get_current_table_data();
        let state = self.get_spreadsheet_state();
        let metadata = self.get_current_column_metadata();

        // Primary key: metadata hasil query, lalu index_cache, lalu state.
        let mut derived_pks: Vec<String> = metadata
            .map(|meta| {
                meta.iter()
                    .filter(|m| m.is_primary_key)
                    .map(|m| m.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        if derived_pks.is_empty()
            && let Some(ref tbl) = table
            && let Some(db) = self.spreadsheet_extract_database_name()
            && let Some(pks) = self.get_primary_keys_for_table(conn_id, &db, tbl)
            && !pks.is_empty()
        {
            derived_pks = pks;
        }
        let pk_columns: &[String] = if derived_pks.is_empty() {
            &state.primary_key_columns
        } else {
            &derived_pks
        };

        // Nilai asli per (baris, kolom): old_value dari edit pertama.
        let mut row_overrides: HashMap<usize, HashMap<String, String>> = HashMap::new();
        for op in &state.pending_operations {
            if let CellEditOperation::Update {
                row_index,
                col_index,
                old_value,
                ..
            } = op
                && let Some(col_name) = headers.get(*col_index)
            {
                row_overrides
                    .entry(*row_index)
                    .or_default()
                    .entry(col_name.to_lowercase())
                    .or_insert_with(|| old_value.clone());
            }
        }

        let mut stmts: Vec<TransactionalStatement> = Vec::new();
        let mut updated_rows: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for op in &state.pending_operations {
            match op {
                CellEditOperation::Update {
                    row_index,
                    row_values,
                    ..
                } => {
                    if !updated_rows.insert(*row_index) {
                        continue;
                    }
                    // Nilai akhir tiap kolom yang diubah pada baris ini.
                    let mut set_cols: Vec<(usize, String)> = Vec::new();
                    for later in &state.pending_operations {
                        if let CellEditOperation::Update {
                            row_index: r,
                            col_index: c,
                            new_value,
                            ..
                        } = later
                            && r == row_index
                        {
                            match set_cols.iter_mut().find(|(ci, _)| ci == c) {
                                Some(slot) => slot.1 = new_value.clone(),
                                None => set_cols.push((*c, new_value.clone())),
                            }
                        }
                    }
                    // Baris asli dari snapshot edit pertama (nilai sebelum
                    // edit), bukan dari posisi baris di grid saat ini.
                    let snapshot = (!row_values.is_empty()).then_some(row_values);
                    let Some(row_data) = snapshot
                        .or_else(|| current_rows.get(*row_index))
                        .or_else(|| all_rows.get(*row_index))
                    else {
                        debug!("[GRID] Missing row data at index {}", row_index);
                        continue;
                    };
                    let overrides = row_overrides.get(row_index);

                    // Query multi-tabel: kolom dikelompokkan per tabel asal.
                    let mut per_table: Vec<(String, Vec<String>)> = Vec::new();
                    for (col_index, new_value) in &set_cols {
                        let col_meta = metadata.and_then(|m| m.get(*col_index));
                        let Some(table_name) = col_meta
                            .and_then(|m| m.table_name.clone())
                            .or_else(|| table.clone())
                        else {
                            debug!(
                                "[GRID] Unable to determine table name for update at col {}",
                                col_index
                            );
                            continue;
                        };
                        let Some(col) = col_meta
                            .and_then(|m| m.original_name.clone())
                            .or_else(|| headers.get(*col_index).cloned())
                        else {
                            continue;
                        };
                        let assignment = format!("{} = {}", qt(&col), qv(new_value));
                        match per_table.iter_mut().find(|(t, _)| *t == table_name) {
                            Some((_, assignments)) => assignments.push(assignment),
                            None => per_table.push((table_name, vec![assignment])),
                        }
                    }
                    for (table_name, assignments) in per_table {
                        let Some(where_clause) = self.spreadsheet_build_where_clause(
                            &conn,
                            row_data,
                            headers,
                            pk_columns,
                            overrides,
                            Some(&table_name),
                        ) else {
                            debug!("[GRID] Unable to build WHERE clause for row {}", row_index);
                            continue;
                        };
                        stmts.push(TransactionalStatement {
                            sql: format!(
                                "UPDATE {} SET {} WHERE {}",
                                qt_table(&table_name),
                                assignments.join(", "),
                                where_clause
                            ),
                            expected_rows: Some(1),
                        });
                    }
                }
                CellEditOperation::InsertRow { row_index, values } => {
                    if headers.is_empty() {
                        continue;
                    }
                    // Nilai terbaru dari grid (edit setelah baris dibuat).
                    let latest = current_rows
                        .get(*row_index)
                        .or_else(|| all_rows.get(*row_index))
                        .unwrap_or(values);
                    // Kolom bernilai DEFAULT dihilangkan dari INSERT sehingga
                    // default/auto-increment server berlaku (juga di SQLite yang
                    // tidak mengenal DEFAULT di VALUES).
                    let (cols, vals): (Vec<String>, Vec<String>) = headers
                        .iter()
                        .enumerate()
                        .filter_map(|(i, h)| {
                            let v = latest.get(i).map(String::as_str).unwrap_or("");
                            (!crate::data_table::grid_model::is_raw_default(v))
                                .then(|| (qt(h), qv(v)))
                        })
                        .unzip();
                    let Some(table_for_insert) = table.as_ref() else {
                        debug!("[GRID] Skipping insert: no table identified");
                        continue;
                    };
                    let sql = if cols.is_empty() {
                        match conn.connection_type {
                            crate::models::enums::DatabaseType::MySQL => {
                                format!("INSERT INTO {} () VALUES ()", qt_table(table_for_insert))
                            }
                            _ => {
                                format!("INSERT INTO {} DEFAULT VALUES", qt_table(table_for_insert))
                            }
                        }
                    } else {
                        format!(
                            "INSERT INTO {} ({}) VALUES ({})",
                            qt_table(table_for_insert),
                            cols.join(", "),
                            vals.join(", ")
                        )
                    };
                    stmts.push(TransactionalStatement {
                        sql,
                        expected_rows: None,
                    });
                }
                CellEditOperation::DeleteRow { row_index, values } => {
                    if values.is_empty() || headers.is_empty() {
                        continue;
                    }
                    let overrides = row_overrides.get(row_index);
                    let Some(where_clause) = self.spreadsheet_build_where_clause(
                        &conn, values, headers, pk_columns, overrides, None,
                    ) else {
                        debug!(
                            "[GRID] Unable to build DELETE WHERE clause for row {}",
                            row_index
                        );
                        continue;
                    };
                    let Some(table_for_delete) = table.as_ref() else {
                        debug!("[GRID] Skipping delete: no table identified");
                        continue;
                    };
                    stmts.push(TransactionalStatement {
                        sql: format!(
                            "DELETE FROM {} WHERE {}",
                            qt_table(table_for_delete),
                            where_clause
                        ),
                        expected_rows: Some(1),
                    });
                }
            }
        }
        stmts
    }

    pub(crate) fn spreadsheet_ensure_primary_keys(&mut self) {
        if !self.spreadsheet_state.primary_key_columns.is_empty() {
            return;
        }
        let conn_id_opt = self.current_connection_id;
        let tbl_opt = self.spreadsheet_extract_table_name();
        let db_str = self.spreadsheet_extract_database_name().unwrap_or_default();

        if let (Some(conn_id), Some(ref tbl)) = (conn_id_opt, tbl_opt) {
            // 1. Try index_cache first (fastest, no network round-trip)
            let mut pks =
                crate::cache_data::get_primary_keys_from_cache(self, conn_id, &db_str, tbl)
                    .unwrap_or_default();
            // 2. Cache miss → query the live database directly
            if pks.is_empty()
                && let Some(conn) = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(conn_id))
                    .cloned()
            {
                pks = self.fetch_primary_key_columns_for_table(conn_id, &conn, &db_str, tbl);
            }
            if !pks.is_empty() {
                debug!("Pre-loaded PKs for '{}': {:?}", tbl, pks);
                self.spreadsheet_state.primary_key_columns = pks;
            } else {
                debug!(
                    "Warning: could not determine PKs for table '{}' — WHERE clause will use all columns",
                    tbl
                );
            }
        }
    }

    /// Kolom primary key efektif: dari metadata hasil query, bila tidak ada
    /// dari `primary_key_columns`.
    fn spreadsheet_effective_pk_columns(&self) -> Vec<String> {
        let from_meta: Vec<String> = self
            .current_column_metadata
            .as_ref()
            .map(|meta| {
                meta.iter()
                    .filter(|m| m.is_primary_key)
                    .map(|m| m.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        if from_meta.is_empty() {
            self.spreadsheet_state.primary_key_columns.clone()
        } else {
            from_meta
        }
    }

    /// WHERE yang mengidentifikasi `row` (nilai sesudah commit). `None` bila
    /// baris tidak bisa diidentifikasi dengan aman, mis. kunci belum
    /// diketahui atau nilainya masih ekspresi mentah (DEFAULT/NOW).
    fn spreadsheet_rewind_where(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        row: &[String],
        pk_columns: &[String],
        table: &str,
    ) -> Option<String> {
        use crate::data_table::grid_model as gm;
        let headers = &self.current_table_headers;
        let unusable = |v: &str| gm::as_raw_sql(v).is_some();
        if pk_columns.is_empty() {
            // Tanpa PK WHERE memakai semua kolom; nilai mentah tidak bisa dicocokkan.
            if row.iter().any(|v| unusable(v)) {
                return None;
            }
        } else {
            for pk in pk_columns {
                let value = headers
                    .iter()
                    .position(|h| h.eq_ignore_ascii_case(pk))
                    .and_then(|i| row.get(i))?;
                if unusable(value) || value.is_empty() || gm::is_null_cell(value) {
                    return None;
                }
            }
        }
        self.spreadsheet_build_where_clause(conn, row, headers, pk_columns, None, Some(table))
    }

    /// Data Rewind (B3): SQL yang membalik antrean pending setelah di-commit.
    /// Dibuat sebelum simpan, saat data grid masih mencerminkan hasil commit.
    pub(crate) fn spreadsheet_generate_rewind(
        &self,
    ) -> crate::data_table::grid_state::PendingRewind {
        use crate::data_table::grid_model as gm;
        use crate::models::structs::CellEditOperation;
        use std::collections::BTreeMap;

        let ops = &self.spreadsheet_state.pending_operations;
        let table = self.spreadsheet_extract_table_name();
        let mut out = crate::data_table::grid_state::PendingRewind {
            summary: gm::summarize_ops(ops),
            table: table.clone().unwrap_or_else(|| "query result".to_string()),
            ..Default::default()
        };
        let Some(conn) = self
            .current_connection_id
            .and_then(|cid| self.connections.iter().find(|c| c.id == Some(cid)))
            .cloned()
        else {
            return out;
        };
        let headers = &self.current_table_headers;
        let metadata = self.current_column_metadata.as_ref();
        let pk_columns = self.spreadsheet_effective_pk_columns();
        let row_now = |r: usize| {
            self.current_table_data
                .get(r)
                .or_else(|| self.all_table_data.get(self.grid_all_index(r)))
                .cloned()
        };
        let mut stmts: Vec<String> = Vec::new();
        let mut restores_empty = false;

        // 1. Sel yang diubah: satu UPDATE per (baris, tabel) yang mengembalikan
        //    semua kolomnya ke nilai asli (nilai lama pertama). WHERE memakai
        //    nilai sesudah commit sehingga perubahan PK pun ikut kembali.
        let mut per_row: BTreeMap<(usize, String), Vec<(String, String)>> = BTreeMap::new();
        for ((row_index, col_index), original) in gm::pending_updated_cells(ops) {
            let col_meta = metadata.and_then(|m| m.get(col_index));
            let Some(table_name) = col_meta
                .and_then(|m| m.table_name.clone())
                .filter(|t| !t.is_empty())
                .or_else(|| table.clone())
            else {
                continue;
            };
            let Some(col) = col_meta
                .and_then(|m| m.original_name.clone())
                .or_else(|| headers.get(col_index).cloned())
            else {
                continue;
            };
            restores_empty |= original.is_empty();
            per_row
                .entry((row_index, table_name))
                .or_default()
                .push((col, original));
        }
        for ((row_index, table_name), mut cols) in per_row {
            cols.sort();
            let Some(row) = row_now(row_index) else {
                continue;
            };
            match self.spreadsheet_rewind_where(&conn, &row, &pk_columns, &table_name) {
                Some(where_clause) => {
                    let sets: Vec<String> = cols
                        .iter()
                        .map(|(c, v)| {
                            format!(
                                "{} = {}",
                                self.spreadsheet_quote_ident(&conn, c),
                                self.spreadsheet_quote_value(&conn, v)
                            )
                        })
                        .collect();
                    stmts.push(format!(
                        "UPDATE {} SET {} WHERE {}",
                        self.spreadsheet_quote_table_ident(&conn, &table_name),
                        sets.join(", "),
                        where_clause
                    ));
                }
                None => out.notes.push(format!(
                    "Row {}: it cannot be identified safely (no key, or a key/value set to DEFAULT/NOW()), so its changes cannot be reverted.",
                    row_index + 1
                )),
            }
        }

        // 2. Baris baru: dihapus lagi berdasarkan PK yang nilainya diketahui.
        for op in ops {
            let CellEditOperation::InsertRow { row_index, .. } = op else {
                continue;
            };
            let (Some(table_name), Some(row)) = (table.as_ref(), row_now(*row_index)) else {
                continue;
            };
            let where_clause = if pk_columns.is_empty() {
                None
            } else {
                self.spreadsheet_rewind_where(&conn, &row, &pk_columns, table_name)
            };
            match where_clause {
                Some(w) => stmts.push(format!(
                    "DELETE FROM {} WHERE {}",
                    self.spreadsheet_quote_table_ident(&conn, table_name),
                    w
                )),
                None => out.notes.push(format!(
                    "New row {}: its key is generated by the database, so the insert cannot be reverted automatically.",
                    row_index + 1
                )),
            }
        }

        // 3. Baris terhapus: disisipkan kembali dengan nilai aslinya.
        for op in ops {
            let CellEditOperation::DeleteRow { values, .. } = op else {
                continue;
            };
            let Some(table_name) = table.as_ref() else {
                continue;
            };
            restores_empty |= values.iter().any(String::is_empty);
            let (cols, vals): (Vec<String>, Vec<String>) = headers
                .iter()
                .zip(values.iter())
                .map(|(h, v)| {
                    (
                        self.spreadsheet_quote_ident(&conn, h),
                        self.spreadsheet_quote_value(&conn, v),
                    )
                })
                .unzip();
            if !cols.is_empty() {
                stmts.push(format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    self.spreadsheet_quote_table_ident(&conn, table_name),
                    cols.join(", "),
                    vals.join(", ")
                ));
            }
        }
        if restores_empty {
            out.notes.push(
                "Empty strings are written back as NULL (the grid does not distinguish them)."
                    .to_string(),
            );
        }
        if ops
            .iter()
            .any(|op| matches!(op, CellEditOperation::DeleteRow { .. }))
        {
            out.notes.push(
                "Re-inserting deleted rows writes every column; identity or generated columns may reject it."
                    .to_string(),
            );
        }
        out.sql = (!stmts.is_empty()).then(|| stmts.join(";\n"));
        out
    }

    /// Isi `spreadsheet_finish_cell_edit`; pembungkusnya menambahkan
    /// pencatatan undo.
    fn spreadsheet_finish_cell_edit_inner(
        &mut self,
        save: bool,
        editing_cell: Option<(usize, usize)>,
    ) {
        let Some((row, col)) = editing_cell else {
            return;
        };
        let new_val = self.spreadsheet_state.cell_edit_text.clone();
        self.spreadsheet_state.cell_edit_text.clear();
        self.spreadsheet_state.editing_cell = None;
        if !save {
            return;
        }
        // Nilai lama dari baris yang tampil; indeks all_table_data
        // disesuaikan dengan halaman aktif oleh grid_cell.
        match self.grid_cell(row, col) {
            Some(old) if old != new_val => {
                // Salinan baris sebelum diubah: kunci WHERE saat simpan.
                let row_snapshot: Vec<String> = self
                    .current_table_data
                    .get(row)
                    .cloned()
                    .unwrap_or_default();
                self.grid_set_cell_raw(row, col, &new_val);
                // Baris baru: perbarui nilai InsertRow-nya, bukan menambah Update.
                let headers_len = self.current_table_headers.len();
                let mut updated_insert_row = false;
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
                if !updated_insert_row {
                    self.spreadsheet_state.pending_operations.push(
                        crate::models::structs::CellEditOperation::Update {
                            row_index: row,
                            col_index: col,
                            old_value: old,
                            new_value: new_val,
                            row_values: row_snapshot,
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
