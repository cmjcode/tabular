use crate::{connection, models, window_egui::Tabular};
use log::debug;

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
        println!(
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
                println!("🔥 Could not get values for row {}", row);
                debug!("🔥 Could not get values for row {}", row);
                return;
            };

            println!(
                "🔥 Adding DeleteRow operation for row {} with {} values: {:?}",
                row,
                values.len(),
                values
            );
            debug!(
                "🔥 Adding DeleteRow operation for row {} with {} values",
                row,
                values.len()
            );

            let state = self.get_spreadsheet_state_mut();
            state
                .pending_operations
                .push(crate::models::structs::CellEditOperation::DeleteRow {
                    row_index: row,
                    values,
                });
            state.is_dirty = true;

            println!(
                "🔥 Now have {} pending operations, is_dirty: {}",
                state.pending_operations.len(),
                state.is_dirty
            );
            debug!(
                "🔥 Now have {} pending operations, is_dirty: {}",
                state.pending_operations.len(),
                state.is_dirty
            );

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
        } else {
            println!("🔥 No row selected for deletion");
            debug!("🔥 No row selected for deletion");
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

    fn spreadsheet_row_where_all_columns(
        &self,
        conn: &crate::models::structs::ConnectionConfig,
        row_index: usize,
    ) -> Option<String> {
        let row = self.get_current_table_data().get(row_index)?;
        let headers = self.get_current_table_headers();

        // This method needs to be implemented by the parent struct to access cache
        // For now, fall back to first column if not overridden
        // Use only the first column (usually primary key like RecID) for WHERE clause
        if let (Some(first_header), Some(first_value)) =
            (headers.first(), row.first())
        {
            let lhs = self.spreadsheet_quote_ident(conn, first_header);
            let rhs = self.spreadsheet_quote_value(conn, first_value);
            Some(format!("{} = {}", lhs, rhs))
        } else {
            None
        }
    }

    fn spreadsheet_generate_sql(&self) -> Option<String> {
        println!("🔥 spreadsheet_generate_sql called");

        let conn_id = self.get_current_connection_id()?;
        println!("🔥 Found connection ID: {}", conn_id);

        let conn = self
            .get_connections()
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()?;
        println!("🔥 Found connection config");

        let table = self.spreadsheet_extract_table_name()?;
        println!("🔥 Extracted table name: {}", table);

        let qt = |s: &str| self.spreadsheet_quote_ident(&conn, s);
        let qt_table = |s: &str| self.spreadsheet_quote_table_ident(&conn, s);
        let qv = |s: &str| self.spreadsheet_quote_value(&conn, s);

        let mut stmts: Vec<String> = Vec::new();
        println!(
            "🔥 Processing {} operations",
            self.get_spreadsheet_state().pending_operations.len()
        );
        for op in &self.get_spreadsheet_state().pending_operations {
            match op {
                crate::models::structs::CellEditOperation::Update {
                    row_index,
                    col_index,
                    old_value: _,
                    new_value,
                } => {
                    let col = self.get_current_table_headers().get(*col_index)?;
                    let where_clause = self.spreadsheet_row_where_all_columns(&conn, *row_index)?;
                    let sql = format!(
                        "UPDATE {} SET {} = {} WHERE {}",
                        qt_table(&table),
                        qt(col),
                        qv(new_value),
                        where_clause
                    );
                    stmts.push(sql);
                }

                crate::models::structs::CellEditOperation::InsertRow { row_index, values } => {
                    let cols: Vec<String> = self
                        .get_current_table_headers()
                        .iter()
                        .map(|c| qt(c))
                        .collect();
                    // Prefer latest row data from all_table_data/current_table_data to avoid stale empty values
                    let latest_vals_src: Option<&Vec<String>> = self
                        .get_all_table_data()
                        .get(*row_index)
                        .or_else(|| self.get_current_table_data().get(*row_index));
                    let vals_vec: Vec<String> = if let Some(src) = latest_vals_src {
                        src.clone()
                    } else {
                        values.clone()
                    };
                    let vals: Vec<String> = vals_vec.iter().map(|v| qv(v)).collect();
                    let sql = format!(
                        "INSERT INTO {} ({}) VALUES ({})",
                        qt_table(&table),
                        cols.join(", "),
                        vals.join(", ")
                    );
                    stmts.push(sql);
                }
                crate::models::structs::CellEditOperation::DeleteRow {
                    row_index: _,
                    values,
                } => {
                    // Use a smarter WHERE clause - prefer just the first column if it looks like a primary key
                    if values.is_empty() || self.get_current_table_headers().is_empty() {
                        continue;
                    }

                    let first_header = &self.get_current_table_headers()[0];
                    let first_value = &values[0];

                    // If the first column looks like a primary key (RecID, ID, etc.), use just that
                    if first_header.to_lowercase().contains("id")
                        || first_header.to_lowercase().contains("recid")
                        || first_header.to_lowercase() == "pk"
                    {
                        let where_clause = format!("{} = {}", qt(first_header), qv(first_value));
                        let sql =
                            format!("DELETE FROM {} WHERE {}", qt_table(&table), where_clause);
                        println!("🔥 Using primary key WHERE: {}", where_clause);
                        stmts.push(sql);
                    } else {
                        // Fallback to all columns if no obvious primary key
                        if values.len() != self.get_current_table_headers().len() {
                            continue;
                        }
                        let parts: Vec<String> = self
                            .get_current_table_headers()
                            .iter()
                            .zip(values.iter())
                            .map(|(col, v)| format!("{} = {}", qt(col), qv(v)))
                            .collect();
                        let where_clause = parts.join(" AND ");
                        let sql =
                            format!("DELETE FROM {} WHERE {}", qt_table(&table), where_clause);
                        println!(
                            "🔥 Using full row WHERE (no obvious PK): {} columns",
                            parts.len()
                        );
                        stmts.push(sql);
                    }
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
        println!(
            "🔥 spreadsheet_save_changes called with {} pending operations",
            self.get_spreadsheet_state().pending_operations.len()
        );
        debug!(
            "🔥 spreadsheet_save_changes called with {} pending operations",
            self.get_spreadsheet_state().pending_operations.len()
        );

        if self.get_spreadsheet_state().pending_operations.is_empty() {
            println!("🔥 No pending operations to save");
            debug!("🔥 No pending operations to save");
            return;
        }
        if let Some(sql) = self.spreadsheet_generate_sql() {
            println!("🔥 Generated SQL: {}", sql);
            debug!("🔥 Generated SQL: {}", sql);
            if let Some(conn_id) = self.get_current_connection_id() {
                println!("🔥 Executing SQL with connection {}", conn_id);
                debug!("🔥 Executing SQL with connection {}", conn_id);

                // Execute without transaction wrapper to avoid MySQL prepared statement issues
                println!("🔥 Executing SQL: {}", sql);

                // Note: This is a bit tricky because we need to call connection::execute_query_with_connection
                // but this trait doesn't know about the full Tabular struct. We'll need to implement this
                // in the actual implementation of the trait.
                self.execute_spreadsheet_sql(sql);
            } else {
                println!("🔥 No current connection ID");
                debug!("🔥 No current connection ID");
            }
        } else {
            println!("🔥 Failed to generate SQL");
            debug!("🔥 Failed to generate SQL");
        }
    }

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

    fn get_query_tabs(&self) -> &Vec<models::structs::QueryTab> {
        &self.query_tabs
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
                    let columns_json: String = row.try_get(0).map_err(|e| format!("Failed to get columns_json: {}", e))?;
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
                    debug!("✅ Found {} primary key(s) from cache for {}.{}: {:?}", pks.len(), database_name, table_name, pks);
                    Some(pks)
                }
                Ok(_) => {
                    debug!("⚠️ No primary key found in cache for {}.{}", database_name, table_name);
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
        std::println!(
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
                std::println!("🔥 Could not get values for row {}", row);
                debug!("🔥 Could not get values for row {}", row);
                return;
            };

            std::println!(
                "🔥 Adding DeleteRow operation for row {} with {} values: {:?}",
                row,
                values.len(),
                values
            );
            debug!(
                "🔥 Adding DeleteRow operation for row {} with {} values",
                row,
                values.len()
            );

            let state = self.get_spreadsheet_state_mut();
            state
                .pending_operations
                .push(crate::models::structs::CellEditOperation::DeleteRow {
                    row_index: row,
                    values,
                });
            state.is_dirty = true;

            std::println!(
                "🔥 Now have {} pending operations, is_dirty: {}",
                state.pending_operations.len(),
                state.is_dirty
            );
            debug!(
                "🔥 Now have {} pending operations, is_dirty: {}",
                state.pending_operations.len(),
                state.is_dirty
            );

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
        } else {
            std::println!("🔥 No row selected for deletion");
            debug!("🔥 No row selected for deletion");
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

        let table = self.spreadsheet_extract_table_name()?;
        std::println!("🔥 Extracted table name: {}", table);

        let qt = |s: &str| self.spreadsheet_quote_ident(&conn, s);
        let qt_table = |s: &str| self.spreadsheet_quote_table_ident(&conn, s);
        let qv = |s: &str| self.spreadsheet_quote_value(&conn, s);

        let mut stmts: Vec<String> = Vec::new();
        std::println!(
            "🔥 Processing {} operations",
            self.get_spreadsheet_state().pending_operations.len()
        );
        for op in &self.get_spreadsheet_state().pending_operations {
            match op {
                crate::models::structs::CellEditOperation::Update {
                    row_index,
                    col_index,
                    old_value: _,
                    new_value,
                } => {
                    let col = self.get_current_table_headers().get(*col_index)?;
                    let where_clause = self.spreadsheet_row_where_all_columns(&conn, *row_index)?;
                    let sql = std::format!(
                        "UPDATE {} SET {} = {} WHERE {}",
                        qt_table(&table),
                        qt(col),
                        qv(new_value),
                        where_clause
                    );
                    stmts.push(sql);
                }

                crate::models::structs::CellEditOperation::InsertRow { row_index, values } => {
                    let cols: Vec<String> = self
                        .get_current_table_headers()
                        .iter()
                        .map(|c| qt(c))
                        .collect();
                    // Prefer latest row data from all_table_data/current_table_data to avoid stale empty values
                    let latest_vals_src: Option<&Vec<String>> = self
                        .get_all_table_data()
                        .get(*row_index)
                        .or_else(|| self.get_current_table_data().get(*row_index));
                    let vals_vec: Vec<String> = if let Some(src) = latest_vals_src {
                        src.clone()
                    } else {
                        values.clone()
                    };
                    let vals: Vec<String> = vals_vec.iter().map(|v| qv(v)).collect();
                    let sql = std::format!(
                        "INSERT INTO {} ({}) VALUES ({})",
                        qt_table(&table),
                        cols.join(", "),
                        vals.join(", ")
                    );
                    stmts.push(sql);
                }
                crate::models::structs::CellEditOperation::DeleteRow {
                    row_index: _,
                    values,
                } => {
                    // Use a smarter WHERE clause - prefer just the first column if it looks like a primary key
                    if values.is_empty() || self.get_current_table_headers().is_empty() {
                        continue;
                    }

                    let first_header = &self.get_current_table_headers()[0];
                    let first_value = &values[0];

                    // If the first column looks like a primary key (RecID, ID, etc.), use just that
                    if first_header.to_lowercase().contains("id")
                        || first_header.to_lowercase().contains("recid")
                        || first_header.to_lowercase() == "pk"
                    {
                        let where_clause =
                            std::format!("{} = {}", qt(first_header), qv(first_value));
                        let sql =
                            std::format!("DELETE FROM {} WHERE {}", qt_table(&table), where_clause);
                        std::println!("🔥 Using primary key WHERE: {}", where_clause);
                        stmts.push(sql);
                    } else {
                        // Fallback to all columns if no obvious primary key
                        if values.len() != self.get_current_table_headers().len() {
                            continue;
                        }
                        let parts: Vec<String> = self
                            .get_current_table_headers()
                            .iter()
                            .zip(values.iter())
                            .map(|(col, v)| std::format!("{} = {}", qt(col), qv(v)))
                            .collect();
                        let where_clause = parts.join(" AND ");
                        let sql =
                            std::format!("DELETE FROM {} WHERE {}", qt_table(&table), where_clause);
                        std::println!(
                            "🔥 Using full row WHERE (no obvious PK): {} columns",
                            parts.len()
                        );
                        stmts.push(sql);
                    }
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
                self.execute_spreadsheet_sql(sql);
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
        let row = self.get_current_table_data().get(row_index)?;
        let headers = self.get_current_table_headers();
        
        // Try to get primary keys from cache
        let conn_id = conn.id?;
        let database_name = self.query_tabs.get(self.active_tab_index)?.database_name.as_ref()?;
        
        // Extract clean table name (without "Table: " prefix)
        let table_name = self.spreadsheet_extract_table_name()?;
        
        // Get primary keys from cache using the trait method
        let primary_keys = self.get_primary_keys_for_table(conn_id, database_name, &table_name);
        
        debug!("🔍 Primary keys from cache for table {}: {:?}", table_name, primary_keys);
        
        if let Some(pk_cols) = primary_keys {
            if !pk_cols.is_empty() {
                // Build WHERE clause using all primary key columns
                let mut conditions = Vec::new();
                for pk_col in &pk_cols {
                    if let Some(col_idx) = headers.iter().position(|h| h == pk_col)
                        && let Some(value) = row.get(col_idx) {
                            let lhs = self.spreadsheet_quote_ident(conn, pk_col);
                            let rhs = self.spreadsheet_quote_value(conn, value);
                            conditions.push(std::format!("{} = {}", lhs, rhs));
                        }
                }
                
                if !conditions.is_empty() {
                    debug!("✅ Using primary key WHERE clause: {}", conditions.join(" AND "));
                    return Some(conditions.join(" AND "));
                } else {
                    debug!("⚠️ Primary keys found but no matching columns in headers");
                }
            }
        } else {
            debug!("⚠️ No primary keys found in cache for table {}", table_name);
        }
        
        // Fallback: use first column if it looks like an ID
        if let (Some(first_header), Some(first_value)) = (headers.first(), row.first())
            && first_header.to_lowercase().contains("id") {
                debug!("⚠️ Falling back to first column (looks like ID): {}", first_header);
                let lhs = self.spreadsheet_quote_ident(conn, first_header);
                let rhs = self.spreadsheet_quote_value(conn, first_value);
                return Some(std::format!("{} = {}", lhs, rhs));
            }
        
        // Last resort: use all columns for WHERE clause
        debug!("⚠️ No primary key found, using all columns for WHERE clause");
        let mut conditions = Vec::new();
        for (idx, header) in headers.iter().enumerate() {
            if let Some(value) = row.get(idx) {
                let lhs = self.spreadsheet_quote_ident(conn, header);
                let rhs = self.spreadsheet_quote_value(conn, value);
                conditions.push(std::format!("{} = {}", lhs, rhs));
            }
        }
        
        if !conditions.is_empty() {
            Some(conditions.join(" AND "))
        } else {
            None
        }
    }

}
