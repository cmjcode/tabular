use crate::{connection, models, window_egui::Tabular};
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
            self.get_current_table_data_mut().insert(insert_index, row_data.clone());
            // Safe insert into all_table_data
            if insert_index <= self.get_all_table_data().len() {
                self.get_all_table_data_mut().insert(insert_index, row_data.clone());
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
                if row_idx >= insert_index && row_idx != insert_index {
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
            state.pending_operations.push(
                crate::models::structs::CellEditOperation::InsertRow {
                    row_index: insert_index,
                    values: row_data,
                },
            );

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
        println!(
            "🔥 spreadsheet_extract_table_name called with current_table_name: '{}'",
            self.get_current_table_name()
        );

        if self.get_current_table_name().starts_with("Table: ") {
            let s = self.get_current_table_name().strip_prefix("Table: ")?;
            let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
            println!("🔥 Extracted table name: {:?}", result);
            result
        } else {
            // Try to extract from active tab if it's a table browse tab
            if let Some(tab) = self.get_query_tabs().get(self.get_active_tab_index()) {
                println!("🔥 Checking active tab title: '{}'", tab.title);
                if tab.title.starts_with("Table: ") {
                    let s = tab.title.strip_prefix("Table: ")?;
                    let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
                    println!("🔥 Extracted table name from tab: {:?}", result);
                    return result;
                }
            }
            println!("🔥 Table name does not start with 'Table: ' and no suitable tab found");
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
        // Handle NULL values properly - don't quote them
        if v.is_empty() || v.eq_ignore_ascii_case("null") {
            return "NULL".to_string();
        }
        match conn.connection_type {
            crate::models::enums::DatabaseType::MySQL
            | crate::models::enums::DatabaseType::PostgreSQL
            | crate::models::enums::DatabaseType::MsSQL
            | crate::models::enums::DatabaseType::SQLite => format!("'{}'", v.replace("'", "''")),
            _ => format!("'{}'", v),
        }
    }





    fn spreadsheet_save_changes(&mut self);

    // This method needs to be implemented by the struct that implements this trait
    // It should execute the SQL and handle the response appropriately
    fn execute_spreadsheet_sql(&mut self, sql: String);
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

    fn execute_spreadsheet_sql(&mut self, sql: String) {
        if let Some(conn_id) = self.current_connection_id {
            if let Some((headers, data)) =
                connection::execute_query_with_connection(self, conn_id, sql)
            {
                // Detect error tables returned by executor (headers == ["Error"]) and treat as failure
                let is_error_table = headers.len() == 1 && headers[0].eq_ignore_ascii_case("error");
                if is_error_table {
                    let msg = data
                        .first()
                        .and_then(|r| r.first())
                        .cloned()
                        .unwrap_or_else(|| "Unknown query error".to_string());
                    debug!("❌ SQL execution returned error table: {}", msg);
                    self.error_message = msg;
                    self.show_error_message = true;
                    // Do NOT clear pending operations on failure
                } else {
                    debug!("🔥 SQL executed successfully, clearing pending operations");
                    self.spreadsheet_state.pending_operations.clear();
                    self.spreadsheet_state.is_dirty = false;

                    // Clear newly created rows highlight after successful save
                    self.newly_created_rows.clear();

                    // Refresh grid after save so inserted rows become visible
                    if self.is_table_browse_mode {
                        if self.use_server_pagination && !self.current_base_query.is_empty() {
                            // Re-run current page of the base query
                            self.execute_paginated_query();
                        } else {
                            // Client-side mode: simply re-sync current page slice
                            self.update_current_page_data();
                        }
                    }
                }
            } else {
                debug!("🔥 SQL execution failed");
                self.error_message = "Failed to save table changes".to_string();
                self.show_error_message = true;
            }
        }
    }

    fn reset_spreadsheet_state(&mut self) {
        *self.get_spreadsheet_state_mut() = crate::models::structs::SpreadsheetState::default();
    }

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





    fn spreadsheet_extract_table_name(&self) -> Option<String> {
        std::println!(
            "🔥 spreadsheet_extract_table_name called with current_table_name: '{}'",
            self.get_current_table_name()
        );

        if self.get_current_table_name().starts_with("Table: ") {
            let s = self.get_current_table_name().strip_prefix("Table: ")?;
            let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
            std::println!("🔥 Extracted table name: {:?}", result);
            result
        } else {
            // Try to extract from active tab if it's a table browse tab
            if let Some(tab) = self.get_query_tabs().get(self.get_active_tab_index()) {
                std::println!("🔥 Checking active tab title: '{}'", tab.title);
                if tab.title.starts_with("Table: ") {
                    let s = tab.title.strip_prefix("Table: ")?;
                    let result = Some(s.split(" (").next().unwrap_or("").trim().to_string());
                    std::println!("🔥 Extracted table name from tab: {:?}", result);
                    return result;
                }
            }
            std::println!("🔥 Table name does not start with 'Table: ' and no suitable tab found");
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
             std::println!("🔥 spreadsheet_build_where_clause: filtering for target_table='{}'", target_table);
             
             for (i, col_meta) in meta.iter().enumerate() {
                 let belongs_to_table = col_meta.table_name.as_deref().unwrap_or("") == target_table;
                 
                 if belongs_to_table && col_meta.is_primary_key {
                     if let Some(col_name) = headers.get(i) {
                         std::println!("🔥 Found matching PK: '{}' at index {}", col_name, i);
                         let id_name = col_meta.original_name.clone().unwrap_or(col_name.clone());
                         let mut val = row_data.get(i).cloned().unwrap_or_default();
                         if let Some(ov) = overrides {
                             if let Some(v) = ov.get(&col_name.to_lowercase()) {
                                 val = v.clone();
                             }
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
                     // std::println!("🔥 Skipping column '{}' (is_pk={}) for table match", col_meta.name, col_meta.is_primary_key);
                 }
             }
        } else {
             std::println!("🔥 spreadsheet_build_where_clause: NO metadata filtering (target={:?}, meta={})", target_table_for_update, metadata.is_some());
        }

        if where_parts.is_empty() {
             std::println!("🔥 spreadsheet_build_where_clause: where_parts was empty, using FALLBACK logic");
             for (i, header) in headers.iter().enumerate() {
                // NEW: Security check - if we have metadata, ensure this column belongs to target table
                // This prevents adding columns from joined tables (e.g. date_time) to the WHERE clause
                // when updating a specific table (e.g. user_data).
                if let Some(target) = target_table_for_update {
                    if let Some(meta) = metadata.as_ref() {
                        if let Some(col_meta) = meta.get(i) {
                             let tbl = col_meta.table_name.as_deref().unwrap_or("");
                             // Only skip if table name is explicitly known and differs from target.
                             // Use case-insensitive check to be safe.
                             if !tbl.is_empty() && !tbl.eq_ignore_ascii_case(target) {
                                 std::println!("🔥 Fallback skipping column '{}' because it belongs to table '{}' (target='{}')", header, tbl, target);
                                 continue;
                             }
                        }
                    }
                }

                if primary_keys.is_empty()
                    || primary_keys
                        .iter()
                        .any(|pk| pk.eq_ignore_ascii_case(header))
                {
                    if let Some(val_ref) = row_data.get(i) {
                         let mut val = val_ref.clone();
                         if let Some(ov) = overrides {
                             if let Some(v) = ov.get(&header.to_lowercase()) {
                                 val = v.clone();
                             }
                         }
                        let clause = if val.to_uppercase() == "NULL" {
                            format!("{} IS NULL", qt(header))
                        } else {
                            format!("{} = {}", qt(header), qv(&val))
                        };
                        where_parts.push(clause);
                    }
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
                     let clause = if first_value.is_empty() || first_value.eq_ignore_ascii_case("null") {
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
        // Handle NULL values properly - don't quote them
        if v.is_empty() || v.eq_ignore_ascii_case("null") {
            return "NULL".to_string();
        }
        match conn.connection_type {
            crate::models::enums::DatabaseType::MySQL
            | crate::models::enums::DatabaseType::PostgreSQL
            | crate::models::enums::DatabaseType::MsSQL
            | crate::models::enums::DatabaseType::SQLite => {
                std::format!("'{}'", v.replace("'", "''"))
            }
            _ => std::format!("'{}'", v),
        }
    }

    fn spreadsheet_generate_sql(&self) -> Option<String> {
        std::println!("🔥 spreadsheet_generate_sql called");

        let conn_id = self.get_current_connection_id()?;
        std::println!("🔥 Found connection ID: {}", conn_id);

        let conn = self
            .get_connections()
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()?;
        std::println!("🔥 Found connection config");

        let table = self.spreadsheet_extract_table_name();
        if let Some(t) = &table {
            std::println!("🔥 Extracted table name: {}", t);
        } else {
            std::println!("🔥 No global table name found - relying on column metadata");
        }

        let qt = |s: &str| self.spreadsheet_quote_ident(&conn, s);
        let qt_table = |s: &str| self.spreadsheet_quote_table_ident(&conn, s);
        let qv = |s: &str| self.spreadsheet_quote_value(&conn, s);

        let headers = self.get_current_table_headers();
        let all_rows = self.get_all_table_data();
        let current_rows = self.get_current_table_data();
        let state = self.get_spreadsheet_state();
        let metadata = self.get_current_column_metadata();
        if let Some(meta) = metadata {
            log::info!("🔥 metadata present with {} columns", meta.len());
            for (i, m) in meta.iter().enumerate() {
                log::info!("🔥 Col {}: name='{}', table='{:?}', orig='{:?}'", i, m.name, m.table_name, m.original_name);
            }
        } else {
            log::warn!("🔥 No metadata found in spreadsheet_generate_sql override");
        }

        // Determine derived primary keys from metadata if available
        let mut derived_pks = Vec::new();
        if let Some(meta) = metadata {
            for m in meta {
                if m.is_primary_key {
                    derived_pks.push(m.name.clone());
                    log::info!("🔥 Found PK from metadata: {}", m.name);
                }
            }
        }

        let pk_columns = if !derived_pks.is_empty() {
            &derived_pks
        } else {
            &state.primary_key_columns
        };
        let mut pk_overrides: HashMap<usize, HashMap<String, String>> = HashMap::new();
        if !pk_columns.is_empty() {
            for op in &state.pending_operations {
                if let crate::models::structs::CellEditOperation::Update {
                    row_index,
                    col_index,
                    old_value,
                    ..
                } = op
                    && let Some(col_name) = headers.get(*col_index)
                    && pk_columns
                        .iter()
                        .any(|pk| pk.eq_ignore_ascii_case(col_name))
                {
                    pk_overrides
                        .entry(*row_index)
                        .or_default()
                        .insert(col_name.to_lowercase(), old_value.clone());
                }
            }
        }

        let mut stmts: Vec<String> = Vec::new();
        std::println!(
            "🔥 Processing {} operations",
            state.pending_operations.len()
        );
        for op in &state.pending_operations {
            match op {
                crate::models::structs::CellEditOperation::Update {
                    row_index,
                    col_index,
                    old_value: _,
                    new_value,
                } => {
                    // Determine table name for this specific column
                    // Determine table name for this specific column
                    let col_meta = metadata.and_then(|m| m.get(*col_index));
                    let table_name_opt = col_meta
                        .and_then(|m| m.table_name.clone())
                        .or_else(|| table.clone());

                    let table_name_str = match table_name_opt {
                        Some(t) => t,
                        None => {
                             std::println!("🔥 Unable to determine table name for update at col {}", col_index);
                             continue;
                        }
                    };

                    // Determine column name (use original name if available to handle aliases)
                    let col_name_str = col_meta
                        .and_then(|m| m.original_name.clone())
                        .or_else(|| headers.get(*col_index).cloned());
                    
                    let col = match col_name_str {
                         Some(n) => n,
                         None => {
                             std::println!("🔥 Missing header for column index {}", col_index);
                             continue;
                         }
                    };
                    let row_data = current_rows
                        .get(*row_index)
                        .or_else(|| all_rows.get(*row_index));
                    let row_data = match row_data {
                        Some(r) => r,
                        None => {
                            std::println!("🔥 Missing row data at index {}", row_index);
                            continue;
                        }
                    };
                    let overrides = pk_overrides.get(row_index);
                    let where_clause = match self.spreadsheet_build_where_clause(
                        &conn, row_data, headers, pk_columns, overrides, Some(&table_name_str),
                    ) {
                        Some(clause) => clause,
                        None => {
                            std::println!("🔥 Unable to build WHERE clause for row {}", row_index);
                            continue;
                        }
                    };
                    let sql = std::format!(
                        "UPDATE {} SET {} = {} WHERE {}",
                        qt_table(&table_name_str),
                        qt(&col),
                        qv(new_value),
                        where_clause
                    );
                    stmts.push(sql);
                }

                crate::models::structs::CellEditOperation::InsertRow { row_index, values } => {
                    if headers.is_empty() {
                        std::println!("🔥 Skipping insert: no headers available");
                        continue;
                    }
                    let cols: Vec<String> = headers.iter().map(|c| qt(c)).collect();
                    // Prefer latest row data from all_table_data/current_table_data to avoid stale empty values
                    let latest_vals_src: Option<&Vec<String>> = self
                        .get_current_table_data()
                        .get(*row_index)
                        .or_else(|| self.get_all_table_data().get(*row_index));
                    let vals_vec: Vec<String> = if let Some(src) = latest_vals_src {
                        src.clone()
                    } else {
                        values.clone()
                    };
                    let vals: Vec<String> = vals_vec.iter().map(|v| qv(v)).collect();
                    let table_for_insert = match &table {
                         Some(t) => t,
                         None => {
                             std::println!("🔥 Skipping insert: no global table identified");
                             continue;
                         }
                    };
                    let sql = std::format!(
                        "INSERT INTO {} ({}) VALUES ({})",
                        qt_table(table_for_insert),
                        cols.join(", "),
                        vals.join(", ")
                    );
                    stmts.push(sql);
                }
                crate::models::structs::CellEditOperation::DeleteRow { row_index, values } => {
                    if values.is_empty() || headers.is_empty() {
                        continue;
                    }
                    let overrides = pk_overrides.get(row_index);
                    let where_clause = match self.spreadsheet_build_where_clause(
                        &conn, values, headers, pk_columns, overrides, None,
                    ) {
                        Some(clause) => clause,
                        None => {
                            std::println!(
                                "🔥 Unable to build DELETE WHERE clause for row {}",
                                row_index
                            );
                            continue;
                        }
                    };
                    let table_for_delete = match &table {
                         Some(t) => t,
                         None => {
                             std::println!("🔥 Skipping delete: no global table identified");
                             continue;
                         }
                    };
                    let sql =
                        std::format!("DELETE FROM {} WHERE {}", qt_table(table_for_delete), where_clause);
                    std::println!("🔥 Using DELETE WHERE clause: {}", where_clause);
                    stmts.push(sql);
                }
            }
        }
        if stmts.is_empty() {
            None
        } else {
            Some(stmts.join(";\n"))
        }
    }

    fn spreadsheet_save_changes(&mut self) {
        std::println!(
            "🔥 spreadsheet_save_changes called with {} pending operations",
            self.get_spreadsheet_state().pending_operations.len()
        );
        debug!(
            "🔥 spreadsheet_save_changes called with {} pending operations",
            self.get_spreadsheet_state().pending_operations.len()
        );

        if self.get_spreadsheet_state().pending_operations.is_empty() {
            std::println!("🔥 No pending operations to save");
            debug!("🔥 No pending operations to save");
            return;
        }
        if let Some(sql) = self.spreadsheet_generate_sql() {
            std::println!("🔥 Generated SQL: {}", sql);
            debug!("🔥 Generated SQL: {}", sql);
            if let Some(conn_id) = self.get_current_connection_id() {
                std::println!("🔥 Executing SQL with connection {}", conn_id);
                debug!("🔥 Executing SQL with connection {}", conn_id);

                // Execute without transaction wrapper to avoid MySQL prepared statement issues
                std::println!("🔥 Executing SQL: {}", sql);

                // Note: This is a bit tricky because we need to call connection::execute_query_with_connection
                // but this trait doesn't know about the full Tabular struct. We'll need to implement this
                // in the actual implementation of the trait.
                // in the actual implementation of the trait.
                self.execute_spreadsheet_sql(sql);
                // SUCCESS: User requested to stop editing cell on success
                self.spreadsheet_finish_cell_edit(false);
            } else {
                std::println!("🔥 No current connection ID");
                debug!("🔥 No current connection ID");
            }
        } else {
            std::println!("🔥 Failed to generate SQL");
            debug!("🔥 Failed to generate SQL");
        }
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
