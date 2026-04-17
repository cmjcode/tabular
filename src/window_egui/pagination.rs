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
            let prev_headers = self.current_table_headers.clone();
            let requested_page = self.current_page;

            let job_id = self.next_query_job_id;
            self.next_query_job_id = self.next_query_job_id.wrapping_add(1);

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
                    self.active_query_jobs.insert(job_id, status);
                    self.pending_paginated_jobs.insert(job_id);

                    match connection::spawn_query_job(self, job, self.query_result_sender.clone()) {
                        Ok(handle) => {
                            self.active_query_handles.insert(job_id, handle);
                            self.current_table_name =
                                format!("Loading page {}…", self.current_page.saturating_add(1));
                            return;
                        }
                        Err(err) => {
                            debug!(
                                "⚠️ Failed to spawn paginated query job {:?}. Falling back to sync execution.",
                                err
                            );
                            self.active_query_jobs.remove(&job_id);
                            self.pending_paginated_jobs.remove(&job_id);
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

            if let Some((headers, data)) =
                connection::execute_query_with_connection(self, connection_id, paginated_query)
            {
                debug!(
                    "[execute_paginated_query] got result: rows={}, cols={}",
                    data.len(),
                    headers.len()
                );
                // If we navigated past the last page (offset beyond available rows), keep previous headers and revert page
                if data.is_empty() && offset > 0 {
                    // Heuristic: previous page had < page_size rows or actual_total_rows known and offset >= actual_total_rows
                    let past_end = if let Some(total) = self.actual_total_rows {
                        offset >= total
                    } else {
                        self.current_page > 0 && self.total_rows < self.page_size
                    };
                    if past_end {
                        debug!(
                            "🔙 Requested page {} out of range (offset {}), reverting to previous page",
                            requested_page + 1,
                            offset
                        );
                        // Revert page index
                        if requested_page > 0 {
                            self.current_page = requested_page - 1;
                        }
                        // Keep previous headers and data (do not overwrite)
                        self.current_table_headers = prev_headers;
                        // No further sync needed
                        self.query_execution_in_progress = false;
                        self.extend_query_icon_hold();
                        return;
                    }
                }

                // Normal assignment (including empty last page that is valid)
                self.current_table_headers = if headers.is_empty() {
                    if !prev_headers.is_empty() {
                        prev_headers
                    } else {
                        headers
                    }
                } else {
                    headers
                };
                debug!(
                    "[execute_paginated_query] assigning to current_table: rows={}, cols={}",
                    self.current_table_data.len(),
                    self.current_table_headers.len()
                );
                self.current_table_data = data;
                // For server pagination, total_rows represents current page row count only (used for UI row count display)
                self.total_rows = self.current_table_data.len();
                // Sync ke tab aktif agar mode table tab (tanpa editor) bisa menampilkan Data
                if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
                    debug!(
                        "[execute_paginated_query] sync to tab {}: rows={} cols={}",
                        self.active_tab_index,
                        self.current_table_data.len(),
                        self.current_table_headers.len()
                    );
                    active_tab.result_headers = self.current_table_headers.clone();
                    active_tab.result_rows = self.current_table_data.clone();
                    active_tab.result_all_rows = self.current_table_data.clone(); // single page snapshot
                    active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
                    active_tab.current_page = self.current_page;
                    active_tab.page_size = self.page_size;
                    // Note: is_table_browse_mode is not forced here - it inherits from self
                    active_tab.is_table_browse_mode = self.is_table_browse_mode;
                }

                // Save this first page into row cache (only when on first page)
                if self.current_page == 0 {
                    // Determine database and table names for cache key
                    let db_name = self
                        .query_tabs
                        .get(self.active_tab_index)
                        .and_then(|t| t.database_name.clone())
                        .unwrap_or_default();
                    let table = data_table::infer_current_table_name(self);
                    if !db_name.is_empty() && !table.is_empty() {
                        let snapshot: Vec<Vec<String>> =
                            self.current_table_data.iter().take(100).cloned().collect();
                        let headers_clone = self.current_table_headers.clone();
                        crate::cache_data::save_table_rows_to_cache(
                            self,
                            connection_id,
                            &db_name,
                            &table,
                            &headers_clone,
                            &snapshot,
                        );
                        debug!(
                            "💾 Cached first 100 rows (server pagination) for {}/{}",
                            db_name, table
                        );
                    }
                }
            }
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
    pub fn execute_count_query(&mut self) -> Option<usize> {
        // For large tables, we don't want to run actual count queries as they can be very slow
        // or cause timeouts. Instead, we assume a reasonable default size for pagination.
        // This prevents the server from being overwhelmed by expensive COUNT(*) operations.

        debug!("📊 Using default row count assumption for large table pagination");
        debug!("✅ Assuming table has data with default pagination size of 10,000 rows");

        // Return a reasonable default that enables pagination
        // This allows users to navigate through pages without expensive count operations
        Some(10000)
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
