use crate::{connection, editor, models, sidebar_history};

impl super::Tabular {
    pub fn handle_query_result_message(&mut self, message: connection::QueryResultMessage) {
        self.prune_cancelled_jobs();
        self.active_query_handles.remove(&message.job_id);

        // Drop this job from its sequential-batch group (if any); the group
        // entry disappears once every member has reported a result.
        if let Some(pos) = self
            .query_job_batches
            .iter()
            .position(|(ids, _)| ids.contains(&message.job_id))
        {
            let ids = &mut self.query_job_batches[pos].0;
            ids.retain(|id| *id != message.job_id);
            if ids.is_empty() {
                self.query_job_batches.remove(pos);
            }
        }

        if self.cancelled_query_jobs.remove(&message.job_id).is_some() {
            self.pending_paginated_jobs.remove(&message.job_id);
            if self.active_query_jobs.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }
            return;
        }

        if let Some(status) = self.active_query_jobs.get_mut(&message.job_id) {
            status.completed = true;
        }
        self.active_query_jobs.remove(&message.job_id);

        // Structure-editor statements (Add/Drop Column, …) run through this
        // same job pipeline but drive their own success/error handling
        // instead of the query-tab/result-panel flow below — see
        // `PendingStructureJob`.
        if let Some(job) = self.pending_structure_jobs.remove(&message.job_id) {
            if message.success {
                (job.on_success)(self);
            } else {
                let err = message
                    .error
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string());
                self.error_message = format!("{}: {}", job.error_prefix, err);
                self.show_error_message = true;
            }
            if self.active_query_jobs.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }
            return;
        }

        let was_paginated = self.pending_paginated_jobs.remove(&message.job_id);

        if message.truncated {
            self.toasts.warning(format!(
                "Result truncated to the first {} rows. Add a LIMIT or raise “Max rows per result” in Settings → Performance.",
                message.rows.len()
            ));
        }

        // User bisa saja pindah tab selama query berjalan. Hasil tab aktif
        // disimpan di state tampilan global, sedangkan tab lain di field
        // miliknya sendiri. Jadi hasil untuk tab di latar belakang ditulis
        // langsung ke tab tersebut, tanpa menimpa data yang sedang tampil.
        if let Some(origin_idx) = message
            .tab_id
            .and_then(|id| self.query_tabs.iter().position(|t| t.id == id))
            && origin_idx != self.active_tab_index
        {
            self.apply_result_to_background_tab(origin_idx, &message, was_paginated);
            if self.active_query_jobs.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }
            return;
        }

        if let Some(ast_sql) = message.ast_debug_sql.clone() {
            self.last_compiled_sql = Some(ast_sql);
        }
        if let Some(ast_headers) = message.ast_headers.clone() {
            self.last_compiled_headers = ast_headers;
        }

        // Update query message panel
        if message.success {
            self.query_message = describe_query_outcome(&message);
            self.query_message_is_error = false;
            // Auto-switch to Data tab to show results
            self.table_bottom_view = models::structs::TableBottomView::Data;
        } else {
            let error_msg = message.error.clone().unwrap_or_else(|| "Unknown error".to_string());
            self.query_message = format!("Error: {}", error_msg);
            self.query_message_is_error = true;
            // Keep Data view active in bottom panel
            self.table_bottom_view = models::structs::TableBottomView::Data;
        }
        self.show_message_panel = true;
        self.message_shown_at = Some(std::time::Instant::now());

        // Update active tab message
        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.query_message = self.query_message.clone();
            active_tab.query_message_is_error = self.query_message_is_error;
        }

        if was_paginated && message.success {
            self.apply_paginated_query_result(&message);
            return;
        }

        // Store result in multi-tab result list
        let mut result_obj = models::structs::QueryResult {
            headers: message.headers.clone(),
            rows: message.rows.clone(),
            all_rows: message.rows.clone(),
            table_name: if message.success {
                format!("Result {}", self.next_query_job_id) // Placeholder, updated below
            } else {
                "Error".to_string()
            },
            current_page: 0,
            page_size: 500, // Default for now
            total_rows: message.rows.len(),
            query_message: self.query_message.clone(),
            query_message_is_error: self.query_message_is_error,
            execution_time_ms: message.duration.as_millis(),
            column_metadata: message.column_metadata.clone(),
            explain_plan_json: None,
            pinned_columns: std::collections::HashSet::new(),
        };

        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            // Determine index
            let new_index = active_tab.results.len();
            result_obj.table_name = format!("Result {}", new_index + 1);
            
            // If it's an error and we have results, maybe keep the error in a separate Result tab?
            // For now, simple append.
            active_tab.results.push(result_obj.clone());

            // Logic to auto-switch logic:
            // If this is the FIRST result, or if we are actively viewing the "latest" result (potentially),
            // update the viewport.
            // For simplicity: If this is the first result (index 0), switch to it.
            // Or if the user hasn't manually switched to another result yet.
            if new_index == 0 {
                active_tab.active_result_index = 0;
                editor::process_query_result(self, &message.query, message.connection_id, Some((message.headers.clone(), message.rows.clone())), message.column_metadata.clone());
            } else {
                // Save query to history for multi-statement execution results (new_index > 0)
                if message.success {
                    sidebar_history::save_query_to_history(self, &message.query, message.connection_id);
                }
            }
        } else {
             // Fallback for no active tab? Should not happen.
             editor::process_query_result(self, &message.query, message.connection_id, Some((message.headers.clone(), message.rows.clone())), message.column_metadata.clone());
        }

        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.result_headers = self.current_table_headers.clone();
            active_tab.result_rows = self.current_table_data.clone();
            active_tab.result_all_rows = self.current_table_data.clone();
            active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
            active_tab.current_page = self.current_page;
            active_tab.page_size = self.page_size;
            active_tab.is_table_browse_mode = self.is_table_browse_mode;
            active_tab.base_query = self.current_base_query.clone();
            active_tab.result_table_name = self.current_table_name.clone();
        }

        self.query_execution_in_progress = false;
        self.extend_query_icon_hold();
    }
    pub fn set_active_tab_connection_with_database(
        &mut self,
        connection_id: Option<i64>,
        database_name: Option<String>,
    ) {
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.connection_id = connection_id;
            tab.database_name = database_name;
        }

        // Eagerly open the connection pool when a connection is assigned to the active tab.
        // This restores previous behavior where opening a query file (with embedded connection_id)
        // would ensure the connection is ready before the user executes a query.
        if let Some(cid) = connection_id {
            // Update global current_connection_id so other components (e.g. spreadsheet) pick it up
            self.current_connection_id = Some(cid);

            // Open the pool in the background. This used to `block_on` the
            // quick-attempt path, which parked the UI thread inside the connect
            // itself — a slow server froze the whole app on connection select.
            // `ensure_background_pool_creation` is a no-op when a pool already
            // exists or one is already being created.
            crate::connection::ensure_background_pool_creation(self, cid);
        }
    }
    /// Menyimpan hasil query yang selesai ke tab yang sedang tidak ditampilkan.
    /// Data masuk ke field hasil milik tab tersebut, lalu `switch_to_tab`
    /// menukarnya ke tampilan saat user kembali ke tab itu.
    fn apply_result_to_background_tab(
        &mut self,
        tab_index: usize,
        message: &connection::QueryResultMessage,
        was_paginated: bool,
    ) {
        let query_message = describe_query_outcome(message);
        let tab_title;
        {
            let Some(tab) = self.query_tabs.get_mut(tab_index) else {
                return;
            };
            tab_title = tab.title.clone();
            tab.has_executed_query = true;
            tab.query_message = query_message.clone();
            tab.query_message_is_error = !message.success;

            if !(was_paginated && message.success) {
                let new_index = tab.results.len();
                tab.results.push(models::structs::QueryResult {
                    headers: message.headers.clone(),
                    rows: message.rows.clone(),
                    all_rows: message.rows.clone(),
                    table_name: if message.success {
                        format!("Result {}", new_index + 1)
                    } else {
                        "Error".to_string()
                    },
                    current_page: 0,
                    page_size: tab.page_size.max(1),
                    total_rows: message.rows.len(),
                    query_message: query_message.clone(),
                    query_message_is_error: !message.success,
                    execution_time_ms: message.duration.as_millis(),
                    column_metadata: message.column_metadata.clone(),
                    explain_plan_json: None,
                    pinned_columns: std::collections::HashSet::new(),
                });
                if new_index > 0 {
                    // Statement berikutnya dalam batch hanya menambah tab hasil.
                    tab.active_result_index = tab.active_result_index.min(new_index);
                }
            }

            let is_primary = was_paginated || tab.results.len() <= 1;
            if is_primary {
                tab.active_result_index = 0;
                tab.result_headers = message.headers.clone();
                tab.result_all_rows = message.rows.clone();
                tab.result_rows = message.rows.clone();
                tab.result_column_metadata = message.column_metadata.clone();
                tab.total_rows = message.rows.len();
                if !was_paginated {
                    tab.current_page = 0;
                }
                tab.result_table_name = if !message.success {
                    "Error".to_string()
                } else if message.rows.is_empty() {
                    "Query executed successfully (no results)".to_string()
                } else {
                    format!("Query Results ({} rows)", message.rows.len())
                };
            }
        }

        if message.success && !was_paginated {
            sidebar_history::save_query_to_history(self, &message.query, message.connection_id);
        }

        let summary = format!("“{}” finished: {}", tab_title, query_message);
        if message.success {
            self.toasts.info(summary);
        } else {
            self.toasts.error(summary);
        }
    }
    pub fn apply_paginated_query_result(&mut self, message: &connection::QueryResultMessage) {
        self.current_table_headers = message.headers.clone();
        self.current_table_data = message.rows.clone();
        self.all_table_data = self.current_table_data.clone();
        self.total_rows = self.current_table_data.len();

        if self.total_rows == 0 {
            self.current_table_name = format!(
                "Query Results (page {} empty)",
                self.current_page.saturating_add(1)
            );
        } else {
            self.current_table_name = format!(
                "Query Results (page {} showing {} rows)",
                self.current_page.saturating_add(1),
                self.current_table_data.len()
            );
        }

        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
            active_tab.result_headers = self.current_table_headers.clone();
            active_tab.result_rows = self.current_table_data.clone();
            active_tab.result_all_rows = self.current_table_data.clone();
            active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
        }
    }
    /// Kirim perintah cancel ke server (pg_cancel_backend / KILL QUERY) untuk
    /// job yang backend pid-nya sudah tercatat. `abort()` pada task saja hanya
    /// menghentikan penantian di klien, query tetap berjalan di server.
    fn cancel_queries_on_server(&self, job_ids: &[u64]) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        for job_id in job_ids {
            let pid = self
                .query_backend_pids
                .lock()
                .ok()
                .and_then(|m| m.get(job_id).copied());
            let pool = self
                .active_query_jobs
                .get(job_id)
                .and_then(|status| self.connection_pools.get(&status.connection_id).cloned());
            if let (Some(pid), Some(pool)) = (pid, pool) {
                runtime.spawn(connection::execute::cancel_backend_query(pool, pid));
            }
        }
    }

    pub fn cancel_active_query_job(&mut self, job_id: u64) -> bool {
        self.prune_cancelled_jobs();

        let mut server_side_ids = vec![job_id];
        if let Some((ids, _)) = self
            .query_job_batches
            .iter()
            .find(|(ids, _)| ids.contains(&job_id))
        {
            server_side_ids.extend(ids.iter().copied().filter(|id| *id != job_id));
        }
        self.cancel_queries_on_server(&server_side_ids);

        let preview_text = self
            .active_query_jobs
            .get(&job_id)
            .map(|status| status.query_preview.replace('\n', " "));

        let mut cancelled = false;
        if let Some(handle) = self.active_query_handles.remove(&job_id) {
            handle.abort();
            cancelled = true;
        }

        // A sequential batch runs on one task: cancelling any member job
        // aborts the entire batch and cleans up the sibling statements.
        if let Some(pos) = self
            .query_job_batches
            .iter()
            .position(|(ids, _)| ids.contains(&job_id))
        {
            let (member_ids, abort) = self.query_job_batches.remove(pos);
            abort.abort();
            cancelled = true;
            for member in member_ids {
                if member != job_id {
                    self.active_query_jobs.remove(&member);
                    self.active_query_handles.remove(&member);
                    self.cancelled_query_jobs
                        .insert(member, std::time::Instant::now());
                }
            }
        }

        let had_status = self.active_query_jobs.remove(&job_id).is_some();
        let was_paginated = self.pending_paginated_jobs.remove(&job_id);

        if had_status || was_paginated || cancelled {
            self.cancelled_query_jobs
                .insert(job_id, std::time::Instant::now());

            if self.active_query_jobs.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }

            if !was_paginated {
                if let Some(preview) = preview_text.filter(|p| !p.is_empty()) {
                    let truncated: String = if preview.chars().count() > 80 {
                        preview.chars().take(80).collect::<String>() + "…"
                    } else {
                        preview
                    };
                    self.toasts.info(format!("Query cancelled: {}", truncated.trim()));
                } else {
                    self.toasts.info("Query cancelled.");
                }
                self.current_table_name = "Query cancelled".to_string();
            }

            true
        } else {
            false
        }
    }
    pub fn cancel_all_active_query_jobs(&mut self) {
        let job_ids: Vec<u64> = self.active_query_jobs.keys().cloned().collect();
        for job_id in job_ids {
            self.cancel_active_query_job(job_id);
        }
        self.active_query_jobs.clear();
        self.active_query_handles.clear();
        self.query_job_batches.clear();
        self.query_execution_in_progress = false;
        self.current_table_name = "All queries cancelled".to_string();
        self.extend_query_icon_hold();
    }
    pub fn prune_cancelled_jobs(&mut self) {
        let now = std::time::Instant::now();
        let ttl = std::time::Duration::from_secs(30);
        self.cancelled_query_jobs
            .retain(|_, timestamp| now.duration_since(*timestamp) < ttl);
    }
    pub(crate) fn extend_query_icon_hold(&mut self) {
        self.query_icon_hold_until =
            Some(std::time::Instant::now() + std::time::Duration::from_millis(900));
    }
}

/// Baris status untuk query yang selesai: jumlah baris yang dikembalikan untuk
/// result set, jumlah baris terdampak untuk perubahan data (dari driver), atau
/// teks error.
pub(crate) fn describe_query_outcome(message: &connection::QueryResultMessage) -> String {
    if !message.success {
        return format!(
            "Error: {}",
            message.error.as_deref().unwrap_or("Unknown error")
        );
    }
    let duration_ms = message.duration.as_millis();
    let count = match message.affected_rows {
        Some(n) => format!("{} row(s) affected", n),
        None if message.truncated => format!("first {} row(s) returned (truncated)", message.rows.len()),
        None => format!("{} row(s) returned", message.rows.len()),
    };
    format!(
        "Query executed successfully in {}.{:03}s • {}",
        duration_ms / 1000,
        duration_ms % 1000,
        count
    )
}
