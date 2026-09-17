use crate::{connection, editor, models, sidebar_history};

impl super::Tabular {
    pub fn handle_query_result_message(&mut self, mut message: connection::QueryResultMessage) {
        self.prune_cancelled_jobs();
        self.jobs.handles.remove(&message.job_id);

        // Drop this job from its sequential-batch group (if any); the group
        // entry disappears once every member has reported a result.
        if let Some(pos) = self
            .jobs.batches
            .iter()
            .position(|(ids, _)| ids.contains(&message.job_id))
        {
            let ids = &mut self.jobs.batches[pos].0;
            ids.retain(|id| *id != message.job_id);
            if ids.is_empty() {
                self.jobs.batches.remove(pos);
            }
        }

        if self.jobs.cancelled.remove(&message.job_id).is_some() {
            self.jobs.paginated.remove(&message.job_id);
            if self.jobs.active.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }
            return;
        }

        if let Some(status) = self.jobs.active.get_mut(&message.job_id) {
            status.completed = true;
        }
        self.jobs.active.remove(&message.job_id);

        // Job ber-callback (structure editor, simpan spreadsheet, wizard, …)
        // menangani hasilnya sendiri, bukan lewat panel hasil tab.
        if let Some(callback) = self.jobs.callbacks.remove(&message.job_id) {
            callback(self, &message);
            if self.jobs.active.is_empty() {
                self.query_execution_in_progress = false;
                self.extend_query_icon_hold();
            }
            return;
        }

        let was_paginated = self.jobs.paginated.remove(&message.job_id);

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
            if self.jobs.active.is_empty() {
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

        // Simpan lokasi error untuk tombol "Go to error"; hapus saat query sukses.
        let active_tab_id = self.query_tabs.get(self.active_tab_index).map(|t| t.id);
        match (&message.error_location, active_tab_id) {
            (Some(location), Some(tab_id)) if !message.success => {
                self.last_error_location = Some((tab_id, location.clone()));
            }
            (_, Some(tab_id))
                if message.success
                    && self
                        .last_error_location
                        .as_ref()
                        .is_some_and(|(id, _)| *id == tab_id) =>
            {
                self.last_error_location = None;
            }
            _ => {}
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

        // Simpan hasil ke daftar multi-result. Hanya `all_rows` yang disimpan;
        // potongan halaman dibuat ulang saat result dipilih. Baris dari message
        // dipindahkan (bukan di-clone) ke tampilan, sehingga satu result set
        // cukup ada dua salinan: di daftar result dan di tampilan aktif.
        let rows = std::mem::take(&mut message.rows);
        let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) else {
            editor::process_query_result(self, &message.query, message.connection_id, Some((message.headers.clone(), rows)), message.column_metadata.clone());
            self.query_execution_in_progress = false;
            self.extend_query_icon_hold();
            return;
        };
        let new_index = active_tab.results.len();
        active_tab.results.push(models::structs::QueryResult {
            headers: message.headers.clone(),
            rows: Vec::new(),
            all_rows: rows.clone(),
            table_name: if message.success {
                format!("Result {}", new_index + 1)
            } else {
                "Error".to_string()
            },
            current_page: 0,
            page_size: self.page_size.max(1),
            total_rows: rows.len(),
            query_message: self.query_message.clone(),
            query_message_is_error: self.query_message_is_error,
            execution_time_ms: message.duration.as_millis(),
            column_metadata: message.column_metadata.clone(),
            explain_plan_json: None,
            pinned_columns: std::collections::HashSet::new(),
        });

        if new_index == 0 {
            active_tab.active_result_index = 0;
            editor::process_query_result(self, &message.query, message.connection_id, Some((message.headers.clone(), rows)), message.column_metadata.clone());
        } else if message.success {
            // Save query to history for multi-statement execution results (new_index > 0)
            sidebar_history::save_query_to_history(self, &message.query, message.connection_id);
        }

        // Baris hasil untuk tab aktif ada di state tampilan global; switch_to_tab
        // memindahkannya ke field tab saat berpindah, jadi tidak perlu di-clone ke sini.
        if let Some(active_tab) = self.query_tabs.get_mut(self.active_tab_index) {
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
            active_tab.total_rows = self.actual_total_rows.unwrap_or(self.total_rows);
        }
    }
    /// Offset byte lokasi error query terakhir di editor tab aktif, jika ada
    /// dan statement-nya masih ada di teks editor.
    pub fn error_location_in_editor(&self) -> Option<usize> {
        let (tab_id, location) = self.last_error_location.as_ref()?;
        let active_id = self.query_tabs.get(self.active_tab_index)?.id;
        if *tab_id != active_id {
            return None;
        }
        connection::sql::locate_error_in_text(&self.editor.text, location)
    }

    /// Pindahkan kursor editor ke lokasi error query terakhir.
    pub fn jump_to_error_location(&mut self) {
        let Some(pos) = self.error_location_in_editor() else {
            self.toasts
                .info("The failing statement is no longer in the editor.");
            return;
        };
        let pos = pos.min(self.editor.text.len());
        self.multi_selection.clear();
        self.multi_selection.add_collapsed(pos);
        self.cursor_position = pos;
        self.selection_start = pos;
        self.selection_end = pos;
        self.selection_force_clear = true;
        self.pending_cursor_set = Some(pos);
        self.editor_focus_boost_frames = self.editor_focus_boost_frames.max(6);
    }

    /// True jika pool koneksi untuk `connection_id` sudah tersedia.
    pub fn connection_pool_ready(&self, connection_id: i64) -> bool {
        self.connection_pools.contains_key(&connection_id)
            || self
                .shared_connection_pools
                .lock()
                .map(|pools| pools.contains_key(&connection_id))
                .unwrap_or(false)
    }

    /// Jalankan query di latar belakang dan tampilkan hasilnya di tab aktif,
    /// sama seperti tombol Run. Jika pool belum siap, query diantrekan dan
    /// dijalankan otomatis begitu koneksi terbentuk. Pengganti pemanggilan
    /// `execute_query_with_connection` yang memblokir UI.
    pub fn run_query_for_active_tab(&mut self, connection_id: i64, sql: String) {
        if !self.connection_pool_ready(connection_id) {
            connection::ensure_background_pool_creation(self, connection_id);
            self.pool_wait_in_progress = true;
            self.pool_wait_connection_id = Some(connection_id);
            self.pool_wait_query = sql;
            self.pool_wait_started_at = Some(std::time::Instant::now());
            self.query_execution_in_progress = true;
            self.current_table_name = "Connecting… waiting for pool".to_string();
            return;
        }

        let job_id = self.jobs.allocate_id();
        let result = connection::prepare_query_job(self, connection_id, sql.clone(), job_id)
            .and_then(|job| connection::spawn_query_job(self, job, self.query_result_sender.clone()));
        match result {
            Ok(handle) => {
                self.jobs.active.insert(
                    job_id,
                    connection::QueryJobStatus {
                        job_id,
                        connection_id,
                        query_preview: sql.chars().take(80).collect(),
                        started_at: std::time::Instant::now(),
                        completed: false,
                    },
                );
                self.jobs.handles.insert(job_id, handle);
                self.query_execution_in_progress = true;
                self.current_table_name = "Running query…".to_string();
            }
            Err(err) => {
                log::warn!("Could not start query for active tab: {:?}", err);
                self.toasts
                    .error(format!("Query could not be started: {:?}", err));
                if self.jobs.active.is_empty() {
                    self.query_execution_in_progress = false;
                }
            }
        }
    }

    /// Jalankan query di latar belakang dan serahkan hasilnya ke `on_result`
    /// (sukses maupun gagal). Jika pool belum siap, query diantrekan sampai
    /// koneksi terbentuk, gagal, atau menunggu terlalu lama.
    pub fn run_query_with_callback(
        &mut self,
        connection_id: i64,
        sql: String,
        on_result: impl FnOnce(&mut super::Tabular, &connection::QueryResultMessage) + 'static,
    ) {
        let callback: super::QueryCallback = Box::new(on_result);
        if self.connection_pool_ready(connection_id) {
            self.spawn_callback_job(connection_id, sql, callback);
        } else {
            connection::ensure_background_pool_creation(self, connection_id);
            self.jobs.deferred_callbacks.push(super::DeferredCallbackQuery {
                connection_id,
                sql,
                callback,
                queued_at: std::time::Instant::now(),
            });
            self.query_execution_in_progress = true;
        }
    }

    fn spawn_callback_job(&mut self, connection_id: i64, sql: String, callback: super::QueryCallback) {
        let job_id = self.jobs.allocate_id();
        let result = connection::prepare_query_job(self, connection_id, sql.clone(), job_id)
            .and_then(|mut job| {
                job.options.save_to_history = false;
                connection::spawn_query_job(self, job, self.query_result_sender.clone())
            });
        match result {
            Ok(handle) => {
                self.jobs.active.insert(
                    job_id,
                    connection::QueryJobStatus {
                        job_id,
                        connection_id,
                        query_preview: sql.chars().take(80).collect(),
                        started_at: std::time::Instant::now(),
                        completed: false,
                    },
                );
                self.jobs.handles.insert(job_id, handle);
                self.jobs.callbacks.insert(job_id, callback);
                self.query_execution_in_progress = true;
                self.extend_query_icon_hold();
            }
            Err(err) => {
                let message = failed_query_message(
                    job_id,
                    connection_id,
                    &sql,
                    format!("Query could not be started: {:?}", err),
                );
                callback(self, &message);
            }
        }
    }

    /// Dipanggil setiap frame: jalankan query ber-callback yang pool-nya
    /// sudah siap, atau gagalkan jika koneksi error / menunggu lebih dari 60 detik.
    pub fn process_deferred_callback_queries(&mut self) {
        if self.jobs.deferred_callbacks.is_empty() {
            return;
        }
        let queued = std::mem::take(&mut self.jobs.deferred_callbacks);
        for item in queued {
            if self.connection_pool_ready(item.connection_id) {
                self.spawn_callback_job(item.connection_id, item.sql, item.callback);
            } else if let Some(err) = self.connection_errors.get(&item.connection_id).cloned() {
                let message = failed_query_message(
                    0,
                    item.connection_id,
                    &item.sql,
                    format!("Connection failed: {}", err),
                );
                (item.callback)(self, &message);
            } else if item.queued_at.elapsed() > std::time::Duration::from_secs(60) {
                let message = failed_query_message(
                    0,
                    item.connection_id,
                    &item.sql,
                    "Timed out waiting for the database connection".to_string(),
                );
                (item.callback)(self, &message);
            } else {
                self.jobs.deferred_callbacks.push(item);
            }
        }
        if self.jobs.deferred_callbacks.is_empty() && self.jobs.active.is_empty() {
            self.query_execution_in_progress = false;
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
                .jobs.backend_pids
                .lock()
                .ok()
                .and_then(|m| m.get(job_id).copied());
            let pool = self
                .jobs.active
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
            .jobs.batches
            .iter()
            .find(|(ids, _)| ids.contains(&job_id))
        {
            server_side_ids.extend(ids.iter().copied().filter(|id| *id != job_id));
        }
        self.cancel_queries_on_server(&server_side_ids);

        let preview_text = self
            .jobs.active
            .get(&job_id)
            .map(|status| status.query_preview.replace('\n', " "));

        let mut cancelled = false;
        if let Some(handle) = self.jobs.handles.remove(&job_id) {
            handle.abort();
            cancelled = true;
        }

        // A sequential batch runs on one task: cancelling any member job
        // aborts the entire batch and cleans up the sibling statements.
        if let Some(pos) = self
            .jobs.batches
            .iter()
            .position(|(ids, _)| ids.contains(&job_id))
        {
            let (member_ids, abort) = self.jobs.batches.remove(pos);
            abort.abort();
            cancelled = true;
            for member in member_ids {
                if member != job_id {
                    self.jobs.active.remove(&member);
                    self.jobs.handles.remove(&member);
                    self.jobs.cancelled
                        .insert(member, std::time::Instant::now());
                }
            }
        }

        let had_status = self.jobs.active.remove(&job_id).is_some();
        let was_paginated = self.jobs.paginated.remove(&job_id);

        if had_status || was_paginated || cancelled {
            self.jobs.cancelled
                .insert(job_id, std::time::Instant::now());

            if self.jobs.active.is_empty() {
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
        let job_ids: Vec<u64> = self.jobs.active.keys().cloned().collect();
        for job_id in job_ids {
            self.cancel_active_query_job(job_id);
        }
        self.jobs.active.clear();
        self.jobs.handles.clear();
        self.jobs.batches.clear();
        self.query_execution_in_progress = false;
        self.current_table_name = "All queries cancelled".to_string();
        self.extend_query_icon_hold();
    }
    pub fn prune_cancelled_jobs(&mut self) {
        let now = std::time::Instant::now();
        let ttl = std::time::Duration::from_secs(30);
        self.jobs.cancelled
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

/// Pesan hasil gagal untuk query yang tidak sempat dijalankan.
pub(crate) fn failed_query_message(
    job_id: u64,
    connection_id: i64,
    sql: &str,
    error: String,
) -> connection::QueryResultMessage {
    connection::QueryResultMessage {
        job_id,
        tab_id: None,
        connection_id,
        success: false,
        headers: vec!["Error".to_string()],
        rows: vec![vec![error.clone()]],
        error: Some(error),
        duration: std::time::Duration::ZERO,
        query: sql.to_string(),
        dba_special_mode: None,
        ast_debug_sql: None,
        ast_headers: None,
        affected_rows: None,
        column_metadata: None,
        truncated: false,
        error_location: None,
    }
}
