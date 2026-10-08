use crate::spreadsheet::SpreadsheetOperations;
use crate::{connection, data_table, driver_mssql, models};
use log::debug;

impl super::Tabular {
    pub fn execute_paginated_query(&mut self) {
        debug!("🔥 Starting execute_paginated_query()");
        self.query_execution_in_progress = true;
        self.extend_query_icon_hold();
        // Note: is_table_browse_mode is NOT set here - it should only be true when browsing tables via sidebar
        // Use connection from active tab, not global current_connection_id
        let connection_id = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|tab| tab.connection_id);

        debug!(
            "🔥 execute_paginated_query: active_tab_index={}, connection_id={:?}",
            self.active_tab_index, connection_id
        );

        if let Some(connection_id) = connection_id {
            // Check if connection pool is being created to avoid infinite retry loops
            if self.pending_connection_pools.contains(&connection_id) {
                debug!(
                    "⏳ Connection pool creation in progress for connection {}, skipping pagination for now",
                    connection_id
                );
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
                return;
            }

            let offset = self.current_page * self.page_size;
            debug!(
                "🔥 About to build paginated query with offset={}, page_size={}, connection_id={}",
                offset, self.page_size, connection_id
            );
            let paginated_query = self.build_paginated_query(offset, self.page_size);
            debug!("🔥 Built paginated query: {}", paginated_query);

            let job_id = self.jobs.allocate_id();

            match connection::prepare_query_job(
                self,
                connection_id,
                paginated_query.clone(),
                job_id,
            ) {
                Ok(mut job) => {
                    job.options.save_to_history = false;
                    let status = connection::QueryJobStatus {
                        job_id,
                        connection_id,
                        query_preview: paginated_query.chars().take(80).collect(),
                        started_at: std::time::Instant::now(),
                        completed: false,
                    };
                    self.jobs.active.insert(job_id, status);
                    self.jobs.paginated.insert(job_id);

                    match connection::spawn_query_job(self, job, self.query_result_sender.clone()) {
                        Ok(handle) => {
                            self.jobs.handles.insert(job_id, handle);
                            self.current_table_name =
                                format!("Loading page {}…", self.current_page.saturating_add(1));
                            return;
                        }
                        Err(err) => {
                            debug!(
                                "⚠️ Failed to spawn paginated query job {:?}. Falling back to sync execution.",
                                err
                            );
                            self.jobs.active.remove(&job_id);
                            self.jobs.paginated.remove(&job_id);
                        }
                    }
                }
                Err(err) => {
                    debug!(
                        "⚠️ Failed to prepare paginated query job: {:?}. Falling back to sync execution.",
                        err
                    );
                }
            }

            // Job tidak bisa dimulai sekarang (biasanya pool belum siap). Jangan
            // jatuh ke eksekusi sinkron yang memblokir UI: antrekan halaman ini
            // dan terapkan hasilnya begitu koneksi siap.
            self.run_query_with_callback(connection_id, paginated_query, |tabular, message| {
                if message.success {
                    tabular.apply_paginated_query_result(message);
                } else {
                    tabular.toasts.error(format!(
                        "Failed to load page: {}",
                        message.error.clone().unwrap_or_default()
                    ));
                }
            });
            return;
        } else {
            debug!("🔥 No connection_id available in active tab for paginated query");
        }

        self.query_execution_in_progress = false;
        self.extend_query_icon_hold();
    }
    /// Query untuk halaman `offset..offset+limit` dari `base_query` tab aktif.
    ///
    /// Query tanpa `ORDER BY` di level teratas diberi `ORDER BY <primary key>`
    /// (dari cache index) supaya urutan halaman stabil: tanpa itu server bebas
    /// mengembalikan urutan berbeda tiap halaman, sehingga baris bisa terulang
    /// atau hilang saat berpindah halaman. Deteksi `LIMIT`/`ORDER BY` memakai
    /// token level teratas, jadi subquery, literal, dan kolom bernama `offset`
    /// tidak mengecoh.
    pub fn build_paginated_query(&mut self, offset: usize, limit: usize) -> String {
        let Some(base_query) = self
            .query_tabs
            .get(self.active_tab_index)
            .map(|tab| tab.base_query.clone())
            .filter(|q| !q.trim().is_empty())
        else {
            debug!("build_paginated_query: base_query is empty, returning empty string");
            return String::new();
        };

        let connection_id = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|tab| tab.connection_id);
        let db_type = connection_id
            .and_then(|id| self.connections.iter().find(|c| c.id == Some(id)))
            .map(|c| c.connection_type.clone())
            .unwrap_or(models::enums::DatabaseType::MySQL);

        // Prefiks `USE db;` (MySQL/MsSQL) dipisah agar klausa paginasi hanya
        // menempel pada SELECT-nya.
        let is_mysql = matches!(db_type, models::enums::DatabaseType::MySQL);
        let statements = connection::split_sql_statements(&base_query, is_mysql);
        let (prefix, select_part) = match statements.as_slice() {
            [first, .., last] if connection::sql::starts_with_ascii_ci(first.trim(), "USE") => (
                format!("{};\n", first.trim().trim_end_matches(';')),
                last.trim().trim_end_matches(';').to_string(),
            ),
            _ => (
                String::new(),
                base_query.trim().trim_end_matches(';').to_string(),
            ),
        };

        if ["LIMIT", "OFFSET", "FETCH"]
            .iter()
            .any(|kw| connection::sql::has_top_level_keyword(&select_part, kw))
        {
            debug!("build_paginated_query: base_query already paginated, returned as is");
            return base_query;
        }

        let has_order_by = connection::sql::has_top_level_keyword(&select_part, "ORDER BY");
        // Kolom sort yang dipilih di header grid menang atas primary key.
        let sort_order_by = self.sort_column.and_then(|col| {
            let name = self.current_table_headers.get(col)?;
            Some(format!(
                " ORDER BY {} {}",
                quote_ident_for(&db_type, name),
                if self.sort_ascending { "ASC" } else { "DESC" }
            ))
        });
        let order_by = if has_order_by {
            None
        } else {
            sort_order_by.or_else(|| {
                connection_id.and_then(|id| self.paginated_order_by(id, &db_type, &select_part))
            })
        };

        match db_type {
            models::enums::DatabaseType::MySQL
            | models::enums::DatabaseType::SQLite
            | models::enums::DatabaseType::PostgreSQL => {
                format!(
                    "{}{}{} LIMIT {} OFFSET {}",
                    prefix,
                    select_part,
                    order_by.unwrap_or_default(),
                    limit,
                    offset
                )
            }
            models::enums::DatabaseType::MsSQL => {
                // OFFSET/FETCH butuh ORDER BY; tanpa PK yang diketahui pakai
                // `ORDER BY 1` (kolom pertama).
                let select_part = driver_mssql::sanitize_mssql_select_for_pagination(&select_part);
                let order = if has_order_by {
                    String::new()
                } else {
                    order_by.unwrap_or_else(|| " ORDER BY 1".to_string())
                };
                let effective_limit = if limit == 0 { 100 } else { limit };
                let final_query = format!(
                    "{}{}{} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                    prefix, select_part, order, offset, effective_limit
                );
                debug!("MsSQL paginated query: {}", final_query);
                final_query
            }
            _ => {
                // Redis/MongoDB tidak memakai paginasi SQL.
                base_query
            }
        }
    }

    /// ` ORDER BY pk1, pk2` dari primary key (cache index) tabel tunggal yang
    /// dibaca `select_part`; `None` bila query bukan SELECT satu tabel atau
    /// PK-nya belum ada di cache. Hanya membaca cache lokal, tidak ke server.
    fn paginated_order_by(
        &mut self,
        connection_id: i64,
        db_type: &models::enums::DatabaseType,
        select_part: &str,
    ) -> Option<String> {
        let table = connection::sql::single_table_of_select(select_part)?;
        let parts = connection::sql::split_qualified_ident(&table);
        let (qualifier, table_name) = match parts.as_slice() {
            [t] => (None, t.clone()),
            [q, .., t] => (Some(q.clone()), t.clone()),
            [] => return None,
        };
        let tab_database = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|t| t.database_name.clone())
            .filter(|d| !d.trim().is_empty())
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|c| c.id == Some(connection_id))
                    .map(|c| c.database.clone())
            })
            .unwrap_or_default();
        let mut candidates: Vec<String> = Vec::new();
        if let Some(q) = qualifier {
            candidates.push(q);
        }
        if !candidates.iter().any(|c| c == &tab_database) {
            candidates.push(tab_database);
        }
        let pks = candidates.iter().find_map(|db| {
            crate::cache_data::get_primary_keys_from_cache(self, connection_id, db, &table_name)
                .filter(|cols| !cols.is_empty())
        })?;
        let quoted: Vec<String> = pks
            .iter()
            .map(|col| quote_ident_for(db_type, col))
            .collect();
        Some(format!(" ORDER BY {}", quoted.join(", ")))
    }
    pub fn set_page_size(&mut self, new_size: usize) {
        if new_size > 0 {
            if self.grid_refuse_while_dirty("changing the page size") {
                return;
            }
            // Check if we have a base query in the active tab for server-side pagination
            let has_base_query = self
                .query_tabs
                .get(self.active_tab_index)
                .map(|tab| !tab.base_query.is_empty())
                .unwrap_or(false);

            self.page_size = new_size;
            if self.use_server_pagination && has_base_query {
                // Reset to first page and re-execute query
                self.current_page = 0;
                self.execute_paginated_query();
            } else {
                // Client-side pagination
                self.current_page = 0;
                self.update_current_page_data();
            }
            data_table::clear_table_selection(self);
        }
    }
    /// Total baris untuk server pagination. `COUNT(*)` tidak dijalankan otomatis
    /// karena bisa sangat mahal di tabel besar; sebelumnya fungsi ini
    /// mengembalikan angka palsu 10.000 yang membuat navigasi halaman
    /// menyesatkan. Total kini `None` (belum diketahui) sampai user menekan
    /// "Count rows" (lihat `request_total_row_count`).
    pub fn execute_count_query(&mut self) -> Option<usize> {
        None
    }

    /// Hitung total baris query paginasi aktif di latar belakang.
    pub fn request_total_row_count(&mut self) {
        let Some(tab) = self.query_tabs.get(self.active_tab_index) else {
            return;
        };
        let (Some(connection_id), tab_id) = (tab.connection_id, tab.id) else {
            return;
        };
        let base_query = tab.base_query.trim().trim_end_matches(';').to_string();
        if base_query.is_empty() {
            return;
        }
        let count_sql = format!("SELECT COUNT(*) FROM ({}) AS tabular_row_count", base_query);
        self.run_query_with_callback(connection_id, count_sql, move |tabular, message| {
            if !message.success {
                tabular.toasts.error(format!(
                    "Could not count rows: {}",
                    message.error.clone().unwrap_or_default()
                ));
                return;
            }
            let count = message
                .rows
                .first()
                .and_then(|row| row.first())
                .and_then(|value| value.trim().parse::<usize>().ok());
            let still_same_query = tabular
                .query_tabs
                .get(tabular.active_tab_index)
                .is_some_and(|t| {
                    t.id == tab_id && t.base_query.trim().trim_end_matches(';') == base_query
                });
            match count {
                Some(total) if still_same_query => tabular.actual_total_rows = Some(total),
                Some(_) => {}
                None => tabular
                    .toasts
                    .error("Could not read the row count returned by the server"),
            }
        });
    }
    pub fn initialize_server_pagination(&mut self, base_query: String) {
        debug!(
            "🚀 Initializing server pagination with base query: {}",
            base_query
        );
        // Sort header milik tabel sebelumnya tidak boleh terbawa.
        self.sort_column = None;
        self.current_base_query = base_query.clone();
        self.current_page = 0;

        // Also save the base query to the active tab
        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.base_query = base_query;
        }

        // Execute count query to get total rows (now using default assumption)
        if let Some(total) = self.execute_count_query() {
            debug!("✅ Count query successful, total rows: {}", total);
            self.actual_total_rows = Some(total);
        } else {
            debug!("❌ Count query failed, no total available");
            self.actual_total_rows = None;
        }

        // Execute first page
        debug!("📄 Executing first page query...");
        self.execute_paginated_query();
        debug!(
            "🏁 Server pagination initialization complete. actual_total_rows: {:?}",
            self.actual_total_rows
        );
        debug!(
            "🎯 Ready for pagination with {} total pages",
            data_table::get_total_pages(self)
        );
    }
}

/// Quote identifier kolom sesuai dialek untuk klausa ORDER BY paginasi.
fn quote_ident_for(db_type: &models::enums::DatabaseType, name: &str) -> String {
    match db_type {
        models::enums::DatabaseType::MySQL => format!("`{}`", name.replace('`', "``")),
        models::enums::DatabaseType::MsSQL => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}
