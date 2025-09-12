use crate::models;
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

        // Use only the first column (usually primary key like RecID) for WHERE clause
        if let (Some(first_header), Some(first_value)) =
            (self.get_current_table_headers().first(), row.first())
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
