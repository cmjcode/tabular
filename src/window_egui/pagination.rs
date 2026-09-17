use log::{debug};
use crate::spreadsheet::SpreadsheetOperations;
use crate::{connection, models, data_table, driver_mssql};

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
    pub fn build_paginated_query(&self, offset: usize, limit: usize) -> String {
        // Get the base query from the active tab - NO fallback to global state
        let base_query = if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
            if tab.base_query.is_empty() {
                None
            } else {
                Some(&tab.base_query)
            }
        } else {
            None
        };

        debug!(
            "🔍 build_paginated_query: active_tab_index={}, base_query='{}'",
            self.active_tab_index,
            base_query.unwrap_or(&"<empty>".to_string())
        );

        let Some(base_query) = base_query else {
            debug!("❌ build_paginated_query: base_query is empty, returning empty string");
            return String::new();
        };

        // Get the database type from active tab's connection
        let connection_id = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|tab| tab.connection_id);

        let db_type = if let Some(connection_id) = connection_id {
            self.connections
                .iter()
                .find(|c| c.id == Some(connection_id))
                .map(|c| &c.connection_type)
                .unwrap_or(&models::enums::DatabaseType::MySQL)
        } else {
            &models::enums::DatabaseType::MySQL
        };

        // If base_query already contains a LIMIT clause, avoid appending another LIMIT/OFFSET
        let has_limit = {
            let upper = base_query.to_uppercase();
            upper.contains(" LIMIT ")
                || upper.ends_with(" LIMIT")
                || upper.contains("\nLIMIT ")
        };

        if has_limit {
            debug!(
                "🔍 build_paginated_query: base_query already has LIMIT, returning without pagination"
            );
            return base_query.clone();
        }

        match db_type {
            models::enums::DatabaseType::MySQL | models::enums::DatabaseType::SQLite => {
                format!("{} LIMIT {} OFFSET {}", base_query, limit, offset)
            }
            models::enums::DatabaseType::PostgreSQL => {
                format!("{} LIMIT {} OFFSET {}", base_query, limit, offset)
            }
            models::enums::DatabaseType::MsSQL => {
                // MsSQL requires ORDER BY for OFFSET/FETCH. Inject ORDER BY 1 if missing.
                // Handle optional leading USE statement separated by semicolon.
                let mut base = base_query.clone();
                debug!("🔍 MsSQL base query before processing: {}", base);

                let mut prefix = String::new();
                // Separate USE ...; prefix if present so pagination applies only to SELECT part
                if let Some(use_end) = base.find(";\nSELECT") {
                    // include the semicolon in prefix
                    prefix = base[..=use_end].to_string();
                    base = base[use_end + 2..].to_string(); // skip "\n" keeping SELECT...
                }

                // Trim and remove trailing semicolons/spaces
                let mut select_part = base.trim().trim_end_matches(';').to_string();
                debug!("🔍 MsSQL select part before TOP removal: {}", select_part);

                // Enhanced TOP removal using case-insensitive regex-like approach
                select_part = driver_mssql::sanitize_mssql_select_for_pagination(&select_part);
                debug!("🔍 MsSQL select part after TOP removal: {}", select_part);

                // Detect ORDER BY (case-insensitive)
                let has_order = select_part.to_lowercase().contains("order by");
                if !has_order {
                    select_part.push_str(" ORDER BY 1");
                }
                let effective_limit = if limit == 0 { 100 } else { limit }; // safety
                let mut final_query = format!(
                    "{}{} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY",
                    prefix, select_part, offset, effective_limit
                );
                // check if contain TOP 1000 than replace it
                final_query = final_query.replace("TOP 10000", "");
                debug!(" *** final_query *** : {}", final_query);

                debug!("🧪 MsSQL final paginated query: {}", final_query);
                final_query
            }
            _ => {
                // For Redis/MongoDB, return original query (these don't use SQL pagination)
                base_query.clone()
            }
        }
    }
    pub fn set_page_size(&mut self, new_size: usize) {
        if new_size > 0 {
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
                .is_some_and(|t| t.id == tab_id && t.base_query.trim().trim_end_matches(';') == base_query);
            match count {
                Some(total) if still_same_query => tabular.actual_total_rows = Some(total),
                Some(_) => {}
                None => tabular.toasts.error("Could not read the row count returned by the server"),
            }
        });
    }
    pub fn initialize_server_pagination(&mut self, base_query: String) {
        debug!(
            "🚀 Initializing server pagination with base query: {}",
            base_query
        );
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
