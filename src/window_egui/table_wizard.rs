use crate::{connection, models};

impl super::Tabular {
    pub fn open_create_table_wizard(&mut self, connection_id: i64, database_name: Option<String>) {
        let connection = match self
            .connections
            .iter()
            .find(|conn| conn.id == Some(connection_id))
            .cloned()
        {
            Some(conn) => conn,
            None => {
                self.error_message = format!(
                    "Connection {} tidak ditemukan untuk Create Table.",
                    connection_id
                );
                self.show_error_message = true;
                return;
            }
        };

        match connection.connection_type {
            models::enums::DatabaseType::Redis | models::enums::DatabaseType::MongoDB => {
                self.error_message =
                    "Create Table tidak tersedia untuk jenis database ini.".to_string();
                self.show_error_message = true;
                return;
            }
            _ => {}
        }

        let mut target_db = database_name.filter(|s| !s.trim().is_empty());
        if target_db.is_none() {
            let trimmed = connection.database.trim();
            if !trimmed.is_empty() {
                target_db = Some(trimmed.to_string());
            }
        }

        let mut state = models::structs::CreateTableWizardState::new(
            connection_id,
            connection.connection_type.clone(),
            target_db,
        );

        if let Some(first_column) = state.columns.first_mut()
            && first_column.data_type.is_empty()
        {
            first_column.data_type = match connection.connection_type {
                models::enums::DatabaseType::PostgreSQL => "SERIAL".to_string(),
                models::enums::DatabaseType::SQLite => "INTEGER".to_string(),
                models::enums::DatabaseType::MySQL => "INT".to_string(),
                models::enums::DatabaseType::MsSQL => "INT".to_string(),
                _ => String::new(),
            };
        }

        self.current_connection_id = Some(connection_id);
        self.create_table_wizard = Some(state);
        self.create_table_error = None;
        self.show_create_table_dialog = true;
    }
    pub fn quote_identifier(&self, ident: &str, db_type: &models::enums::DatabaseType) -> String {
        let mut parts: Vec<String> = Vec::new();
        for part in ident.split('.') {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                continue;
            }
            let quoted = match db_type {
                models::enums::DatabaseType::MySQL => {
                    if trimmed.starts_with('`') && trimmed.ends_with('`') {
                        trimmed.to_string()
                    } else {
                        format!("`{}`", trimmed.replace('`', "``"))
                    }
                }
                models::enums::DatabaseType::PostgreSQL | models::enums::DatabaseType::SQLite => {
                    if trimmed.starts_with('"') && trimmed.ends_with('"') {
                        trimmed.to_string()
                    } else {
                        format!("\"{}\"", trimmed.replace('"', "\"\""))
                    }
                }
                models::enums::DatabaseType::MsSQL => {
                    if trimmed.starts_with('[') && trimmed.ends_with(']') {
                        trimmed.to_string()
                    } else {
                        format!("[{}]", trimmed.replace(']', "]]"))
                    }
                }
                _ => trimmed.to_string(),
            };
            parts.push(quoted);
        }

        if parts.is_empty() {
            ident.trim().to_string()
        } else {
            parts.join(".")
        }
    }
    pub fn generate_create_table_sql(
        &self,
        state: &models::structs::CreateTableWizardState,
    ) -> Result<String, String> {
        use models::enums::DatabaseType;

        if state.table_name.trim().is_empty() {
            return Err("Please describe the table name.".to_string());
        }

        if matches!(state.db_type, DatabaseType::Redis | DatabaseType::MongoDB) {
            return Err("Create table is not available for this database type.".to_string());
        }

        if state.columns.is_empty() {
            return Err("Please add at least one column.".to_string());
        }

        let mut column_defs: Vec<String> = Vec::new();
        let mut pk_columns: Vec<String> = Vec::new();

        for column in &state.columns {
            let name_trim = column.name.trim();
            if name_trim.is_empty() {
                return Err("Each column must have a name.".to_string());
            }
            if column.data_type.trim().is_empty() {
                return Err(format!("Column '{}' does not have a data type.", name_trim));
            }

            let mut pieces = vec![
                self.quote_identifier(name_trim, &state.db_type),
                column.data_type.trim().to_string(),
            ];

            if !column.allow_null {
                pieces.push("NOT NULL".to_string());
            }
            if !column.default_value.trim().is_empty() {
                pieces.push(format!("DEFAULT {}", column.default_value.trim()));
            }

            column_defs.push(pieces.join(" "));

            if column.is_primary_key {
                pk_columns.push(self.quote_identifier(name_trim, &state.db_type));
            }
        }

        if !pk_columns.is_empty() {
            column_defs.push(format!("PRIMARY KEY ({})", pk_columns.join(", ")));
        }

        let mut statements: Vec<String> = Vec::new();
        let table_identifier = match state.db_type {
            DatabaseType::PostgreSQL => {
                let schema = state
                    .database_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("public");
                format!(
                    "{}.{}",
                    self.quote_identifier(schema, &state.db_type),
                    self.quote_identifier(state.table_name.trim(), &state.db_type)
                )
            }
            DatabaseType::SQLite => self.quote_identifier(state.table_name.trim(), &state.db_type),
            DatabaseType::MySQL => {
                if let Some(db) = state
                    .database_name
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    statements.push(format!("USE `{}`;", db));
                }
                self.quote_identifier(state.table_name.trim(), &state.db_type)
            }
            DatabaseType::MsSQL => {
                if let Some(db) = state
                    .database_name
                    .as_ref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                {
                    statements.push(format!("USE [{}];", db));
                }
                self.quote_identifier(state.table_name.trim(), &state.db_type)
            }
            DatabaseType::Redis | DatabaseType::MongoDB | DatabaseType::ApiHttp => {
                return Err("Create table is not available for this database type.".to_string());
            }
        };

        let create_stmt = format!(
            "CREATE TABLE {} (\n    {}\n);",
            table_identifier,
            column_defs.join(",\n    ")
        );
        statements.push(create_stmt);

        for index in &state.indexes {
            let name_trim = index.name.trim();
            if name_trim.is_empty() {
                continue;
            }
            let columns: Vec<&str> = index
                .columns
                .split(',')
                .map(|c| c.trim())
                .filter(|c| !c.is_empty())
                .collect();
            if columns.is_empty() {
                continue;
            }
            if matches!(state.db_type, DatabaseType::Redis | DatabaseType::MongoDB) {
                continue;
            }

            let quoted_cols = columns
                .iter()
                .map(|c| self.quote_identifier(c, &state.db_type))
                .collect::<Vec<_>>()
                .join(", ");
            let quoted_index_name = self.quote_identifier(name_trim, &state.db_type);
            let prefix = if index.unique { "UNIQUE " } else { "" };
            statements.push(format!(
                "CREATE {}INDEX {} ON {} ({});",
                prefix, quoted_index_name, table_identifier, quoted_cols
            ));
        }

        Ok(statements.join("\n"))
    }
    pub fn validate_create_table_step(
        &self,
        state: &mut models::structs::CreateTableWizardState,
        step: models::structs::CreateTableWizardStep,
    ) -> Option<String> {
        use models::structs::CreateTableWizardStep as Step;
        match step {
            Step::Basics => {
                if state.table_name.trim().is_empty() {
                    return Some("Table name must be provided.".to_string());
                }
                if matches!(
                    state.db_type,
                    models::enums::DatabaseType::Redis | models::enums::DatabaseType::MongoDB
                ) {
                    return Some(
                        "Create table is not available for this database type.".to_string(),
                    );
                }
                None
            }
            Step::Columns => {
                if state.columns.is_empty() {
                    return Some("Please add at least one column.".to_string());
                }
                let mut seen = std::collections::HashSet::new();
                for (idx, column) in state.columns.iter_mut().enumerate() {
                    let name_trim = column.name.trim();
                    if name_trim.is_empty() {
                        return Some(format!("Column {} does not have a name.", idx + 1));
                    }
                    let key = name_trim.to_lowercase();
                    if !seen.insert(key) {
                        return Some(format!("Column name '{}' is duplicated.", name_trim));
                    }
                    if column.data_type.trim().is_empty() {
                        return Some(format!("Column '{}' does not have a data type.", name_trim));
                    }
                    if column.is_primary_key {
                        column.allow_null = false;
                    }
                }
                None
            }
            Step::Indexes => {
                for (idx, index) in state.indexes.iter().enumerate() {
                    let name_trim = index.name.trim();
                    let has_columns = index.columns.split(',').any(|c| !c.trim().is_empty());
                    if name_trim.is_empty() && has_columns {
                        return Some(format!("Index {} requires a name.", idx + 1));
                    }
                    if !name_trim.is_empty() && !has_columns {
                        return Some(format!("Index '{}' requires columns.", name_trim));
                    }
                }
                None
            }
            Step::Review => self.generate_create_table_sql(state).err(),
        }
    }
    pub fn submit_create_table_wizard(&mut self, state: models::structs::CreateTableWizardState) {
        match self.generate_create_table_sql(&state) {
            Ok(sql) => {
                let execution = crate::connection::execute_query_with_connection(
                    self,
                    state.connection_id,
                    sql,
                );
                let (success, message) = match execution {
                    Some((headers, rows)) => {
                        let is_error = headers.first().map(|h| h == "Error").unwrap_or(false);
                        if is_error {
                            let msg = rows
                                .first()
                                .and_then(|row| row.first())
                                .cloned()
                                .unwrap_or_else(|| "Failed to create table.".to_string());
                            (false, Some(msg))
                        } else {
                            (true, None)
                        }
                    }
                    None => (
                        false,
                        Some("Failed to execute CREATE TABLE command.".to_string()),
                    ),
                };

                if success {
                    self.create_table_error = None;
                    self.create_table_wizard = None;
                    self.show_create_table_dialog = false;
                    self.error_message = format!(
                        "Table '{}' has been created successfully.",
                        state.table_name.trim()
                    );
                    self.show_error_message = true;
                    self.refresh_connection(state.connection_id);
                } else {
                    let msg = message.unwrap_or_else(|| "Failed to create table.".to_string());
                    self.create_table_error = Some(msg.clone());
                    self.error_message = msg;
                    self.show_error_message = true;
                    self.create_table_wizard = Some(state);
                    self.show_create_table_dialog = true;
                }
            }
            Err(err) => {
                self.create_table_error = Some(err.clone());
                self.create_table_wizard = Some(state);
                self.show_create_table_dialog = true;
            }
        }
    }
}
