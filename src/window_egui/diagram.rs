use crate::models;
use eframe::egui;

/// Pengambilan skema satu database (conn, db) yang sedang berjalan di
/// background untuk diagram ERD.
pub struct DiagramSchemaJob {
    conn_id: i64,
    db_name: String,
    rx: std::sync::mpsc::Receiver<Result<crate::diagram_schema::SchemaSnapshot, String>>,
}

impl super::Tabular {
    pub fn get_diagram_path(&self, conn_id: i64, db_name: &str) -> Option<std::path::PathBuf> {
        let mut path = if !self.data_directory.is_empty() {
            std::path::PathBuf::from(&self.data_directory).join("diagrams")
        } else if let Some(config_dir) = dirs::data_local_dir() {
            config_dir.join("tabular").join("diagrams")
        } else {
            return None;
        };

        let _ = std::fs::create_dir_all(&path);
        // Sanitize filename
        let safe_db_name: String = db_name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        path.push(format!("conn_{}_{}.json", conn_id, safe_db_name));
        log::debug!(
            "get_diagram_path: inputs=({}, '{}') -> path={:?}",
            conn_id,
            db_name,
            path
        );
        Some(path)
    }
    pub fn render_cache_miss_dialog(&mut self, ctx: &egui::Context) {
        if let Some((conn_id, db_name, table_name)) = &self.cache_miss_request {
            crate::window_egui::style::render_modal_backdrop(
                ctx,
                "cache_miss_backdrop",
                self.cache_miss_request.is_some(),
            );

            let mut should_close = false;
            let mut confirmed = false;

            egui::Window::new("Metadata Missing")
                .title_bar(false)
                .frame(crate::window_egui::style::modal_window_frame(ctx))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .default_width(360.0)
                .show(ctx, |ui| {
                    crate::window_egui::style::render_modal_header(
                        ui,
                        "Metadata Missing",
                        &mut should_close,
                    );
                    ui.add_space(8.0);

                    crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                        ui.label(format!(
                            "Metadata for table '{}' is not in cache.",
                            table_name
                        ));
                        ui.label("Would you like to fetch it now?");
                    });

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let fetch_btn = egui::Button::new(
                                egui::RichText::new("Fetch Metadata")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(crate::window_egui::style::theme_accent(ui.ctx()));

                            if ui.add(fetch_btn).clicked() {
                                confirmed = true;
                            }
                        });
                    });
                });

            if should_close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.cache_miss_request = None;
                return;
            }

            if confirmed {
                // Trigger background fetch
                let conn_id = *conn_id;
                let db = db_name.clone();
                let table = table_name.clone();

                // We can use existing function connection::fetch_columns_from_database
                // avoiding async generic hell by doing it in the background thread if possible,
                // or just spawning a tokio task here since we have runtime.
                // or just spawning a tokio task here since we have runtime.
                if let Some(rt) = self.runtime.clone()
                    && let Some(conn_config) = self
                        .connections
                        .iter()
                        .find(|c| c.id == Some(conn_id))
                        .cloned()
                {
                    let pool_clone = self.db_pool.clone();
                    rt.spawn(async move {
                         // Fetch columns
                         if let Some(cols) = crate::connection::fetch_columns_from_database(
                             conn_id,
                             &db,
                             &table,
                             &conn_config,
                         ) {
                             // This requires a mutable Tabular reference to save to cache, which we don't have easily in async.
                             // But `save_columns_to_cache` mainly needs db_pool.
                             // Let's manually call sqlx logic or refactor save_columns_to_cache to not need Tabular.
                             // Attempting direct sqlx insert matching cache_data logic:
                             if let Some(pool) = pool_clone {
                                 // Copy-paste save logic or make it cleaner later. 
                                 // For now, let's just use the existing function if we can refactor it.
                                 // Refactoring `save_columns_to_cache` to take `&SqlitePool` is best.
                                 // But I cannot refactor it right now easily without breaking other calls.
                                 // So I will implement a raw SQL insert here for the fix.

                                 log::debug!("Loading columns for {}...", table);
                                 // CLEAR
                                 let _ = sqlx::query("DELETE FROM column_cache WHERE connection_id = ? AND database_name = ? AND table_name = ?")
                                     .bind(conn_id)
                                     .bind(&db)
                                     .bind(&table)
                                     .execute(pool.as_ref())
                                     .await;

                                 // INSERT
                                 for (i, (cname, ctype)) in cols.iter().enumerate() {
                                     let _ = sqlx::query("INSERT OR REPLACE INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES (?, ?, ?, ?, ?, ?)")
                                         .bind(conn_id)
                                         .bind(&db)
                                         .bind(&table)
                                         .bind(cname)
                                         .bind(ctype)
                                         .bind(i as i64)
                                         .execute(pool.as_ref())
                                         .await;
                                 }
                                 log::debug!("Columns loaded for {}", table);
                             }
                         }
                     });
                }

                self.cache_miss_request = None;
            }
        }
    }
    pub fn save_diagram(&self, conn_id: i64, db_name: &str, state: &models::structs::DiagramState) {
        let Some(path) = self.get_diagram_path(conn_id, db_name) else {
            return;
        };
        // Isi kontainer link database tidak disimpan; hanya referensinya.
        let state = crate::diagram_links::persistable(state);
        // Tulis atomik supaya layout lama tidak rusak bila app crash saat menyimpan.
        let result = serde_json::to_vec_pretty(&state)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                crate::diagram_view::write_atomic(&path, &bytes).map_err(|e| e.to_string())
            });
        match result {
            Ok(()) => log::debug!("Diagram layout saved to {:?}", path),
            Err(e) => log::error!("Failed to save diagram {:?}: {}", path, e),
        }
    }
    /// Jalankan aksi toolbar diagram yang butuh state aplikasi.
    pub fn handle_diagram_action(
        &mut self,
        action: crate::diagram_view::DiagramAction,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
    ) {
        use crate::diagram_view::DiagramAction;
        match action {
            DiagramAction::Save => self.save_diagram_with_defaults(conn_id, db_name, state),
            DiagramAction::Info(msg) => self.toasts.success(msg),
            DiagramAction::Error(msg) => self.toasts.error(msg),
            DiagramAction::SaveToVault => self.save_diagram_to_vault(conn_id, db_name, state),
            DiagramAction::SaveToDatabase => self.save_diagram_to_db(conn_id, db_name, state),
            DiagramAction::LoadFromDatabase => {
                self.load_diagram_from_db_and_apply(conn_id, db_name)
            }
            DiagramAction::OpenLinkDatabaseModal => self.open_link_database_modal(None),
            DiagramAction::RelinkDatabase(link_id) => self.open_link_database_modal(Some(link_id)),
            DiagramAction::OpenLinkedDiagram(link_id) => self.open_linked_source_diagram(&link_id),
            DiagramAction::RefreshLinks(only) => {
                // Hasil datang dari background; kegagalan dilaporkan oleh
                // `fail_diagram_links` saat tiba.
                self.refresh_diagram_links(self.active_tab_index, only.as_deref());
                self.toasts.info("Refreshing linked databases…");
            }
            DiagramAction::SyncToServer => {
                self.sync_diagram_to_server(conn_id, db_name, state);
            }
        }
    }

    /// Simpan diagram ke disk lokal dan secara default otomatis ke vault Obsidian (bila diaktifkan).
    pub fn save_diagram_with_defaults(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
    ) {
        let Some(cid) = conn_id else {
            self.toasts.error("No active connection for diagram save");
            return;
        };
        let db = db_name.unwrap_or_else(|| "default".to_string());

        // 1. Simpan layout ke cache JSON lokal
        self.save_diagram_and_propagate(cid, &db, state);

        // 2. Default: simpan juga ke Obsidian vault jika vault aktif
        if self.obsidian_root().is_some() {
            self.save_diagram_to_vault(Some(cid), Some(db), state);
        } else {
            self.toasts.success("Diagram layout saved");
        }
    }

    /// Simpan diagram ke tabel `diagram_by_tabular` di database target dan cache lokal.
    pub fn save_diagram_to_db(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
    ) {
        let Some(cid) = conn_id else {
            self.toasts.error("No active connection for diagram save");
            return;
        };
        let db = db_name.unwrap_or_else(|| "default".to_string());

        // Simpan juga ke cache disk lokal segera
        self.save_diagram_and_propagate(cid, &db, state);

        let pool_opt = self.connection_pools.get(&cid).cloned().or_else(|| {
            self.shared_connection_pools
                .lock()
                .ok()
                .and_then(|p| p.get(&cid).cloned())
        });

        let Some(pool) = pool_opt else {
            self.toasts
                .error("Database connection pool not ready. Reconnect and try again.");
            return;
        };

        let Some(rt) = self.runtime.clone() else {
            self.toasts.error("Tokio runtime unavailable");
            return;
        };

        let state_clone = crate::diagram_links::persistable(state);
        let db_clone = db.clone();

        let save_res = rt.block_on(async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                crate::diagram_storage::save_diagram_to_database(
                    &pool,
                    &db_clone,
                    &state_clone,
                    None,
                    None,
                ),
            )
            .await
        });

        match save_res {
            Ok(Ok(())) => {
                self.toasts
                    .success("Diagram saved to table 'diagram_by_tabular' in database");
            }
            Ok(Err(e)) => {
                log::error!("[DIAGRAM_DB] Failed to save diagram to database: {e}");
                self.toasts.error(format!("Save to database failed: {e}"));
            }
            Err(_) => {
                log::error!("[DIAGRAM_DB] Save diagram to database timed out");
                self.toasts.error("Save to database timed out (10s)");
            }
        }
    }

    /// Muat ulang diagram dari tabel `diagram_by_tabular` di database target.
    pub fn load_diagram_from_db_and_apply(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
    ) {
        let Some(cid) = conn_id else {
            self.toasts.error("No active connection for diagram load");
            return;
        };
        let db = db_name.unwrap_or_else(|| "default".to_string());

        let pool_opt = self.connection_pools.get(&cid).cloned().or_else(|| {
            self.shared_connection_pools
                .lock()
                .ok()
                .and_then(|p| p.get(&cid).cloned())
        });

        let Some(pool) = pool_opt else {
            self.toasts
                .error("Database connection pool not ready. Reconnect and try again.");
            return;
        };

        let Some(rt) = self.runtime.clone() else {
            self.toasts.error("Tokio runtime unavailable");
            return;
        };

        let db_clone = db.clone();
        let load_res = rt.block_on(async move {
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                crate::diagram_storage::load_diagram_from_database(&pool, &db_clone, None),
            )
            .await
        });

        match load_res {
            Ok(Ok(Some(loaded_state))) => {
                let mut state_to_cache = None;
                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index)
                    && let Some(current_state) = &mut tab.diagram_state
                {
                    crate::diagram_links::strip_linked(current_state);
                    current_state.groups = loaded_state.groups;
                    current_state.virtual_relations = loaded_state.virtual_relations;
                    current_state.linked_databases = loaded_state.linked_databases;
                    current_state.pan = loaded_state.pan;
                    current_state.zoom = loaded_state.zoom;
                    current_state.show_grid = loaded_state.show_grid;
                    current_state.prevent_overlap = loaded_state.prevent_overlap;
                    current_state.show_relations = loaded_state.show_relations;

                    for node in &mut current_state.nodes {
                        if let Some(ln) = loaded_state.nodes.iter().find(|n| n.id == node.id) {
                            node.pos = ln.pos;
                            node.size = ln.size;
                            node.group_ids = ln.group_ids.clone();
                            node.group_id = ln.group_id.clone();
                            node.detached = ln.detached;
                        }
                    }

                    for ln in loaded_state.nodes {
                        if ln.detached && !current_state.nodes.iter().any(|n| n.id == ln.id) {
                            current_state.nodes.push(ln);
                        }
                    }

                    state_to_cache = Some(current_state.clone());
                }

                if let Some(st) = state_to_cache {
                    self.save_diagram(cid, &db, &st);
                }
                self.refresh_diagram_links(self.active_tab_index, None);
                self.toasts
                    .success("Diagram loaded from table 'diagram_by_tabular'");
            }
            Ok(Ok(None)) => {
                self.toasts
                    .warning("Table 'diagram_by_tabular' not found or empty in database");
            }
            Ok(Err(e)) => {
                log::error!("[DIAGRAM_DB] Failed to load diagram from database: {e}");
                self.toasts.error(format!("Load from database failed: {e}"));
            }
            Err(_) => {
                self.toasts.error("Load from database timed out (10s)");
            }
        }
    }

    /// Simpan skema diagram sebagai catatan Mermaid di vault Obsidian, supaya
    /// agent AI bisa me-recall-nya lewat `search_notes` / `read_note`.
    fn save_diagram_to_vault(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
    ) {
        let Some(root) = self.obsidian_root() else {
            self.toasts.warning(
                "Enable an Obsidian vault in Settings > AI Assistant > Memory to save diagrams as AI memory",
            );
            return;
        };
        let conn_name = conn_id
            .and_then(|id| self.connections.iter().find(|c| c.id == Some(id)))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Connection".to_string());
        let db = db_name.unwrap_or_else(|| "default".to_string());

        let model = crate::diagram_mermaid::ErModel::from_diagram(
            &crate::diagram_links::persistable(state),
        );
        let body = crate::diagram_mermaid::schema_note_markdown(
            &format!("Schema: {db} ({conn_name})"),
            &model,
        );
        let result = crate::obsidian::save_schema_note(
            &root,
            &format!("{conn_name} - {db}"),
            &body,
            &[("connection", &conn_name), ("database", &db)],
        );
        match result {
            Ok(path) => {
                log::info!("[OBSIDIAN] saved schema note: {path}");
                self.toasts
                    .success(format!("Schema saved to vault: {path}"));
                if self.ai_obsidian_index_receiver.is_none() {
                    self.start_obsidian_index();
                }
            }
            Err(e) => {
                log::warn!("[OBSIDIAN] schema note save failed: {e}");
                self.toasts.error(format!("Save to vault failed: {e}"));
            }
        }
    }

    pub fn load_diagram(
        &self,
        conn_id: i64,
        db_name: &str,
    ) -> Option<models::structs::DiagramState> {
        if let Some(path) = self.get_diagram_path(conn_id, db_name)
            && path.exists()
        {
            match std::fs::File::open(&path) {
                Ok(file) => {
                    let reader = std::io::BufReader::new(file);
                    match serde_json::from_reader(reader) {
                        Ok(state) => {
                            log::debug!("Diagram layout loaded from {:?}", path);
                            return Some(state);
                        }
                        Err(e) => log::error!("Failed to deserialize diagram state: {}", e),
                    }
                }
                Err(e) => log::error!("Failed to open diagram file {:?}: {}", path, e),
            }
            None
        } else {
            None
        }
    }

    /// Muat diagram dari cache JSON lokal dan rapikan untuk dipakai.
    fn load_prepared_diagram(
        &self,
        conn_id: i64,
        db_name: &str,
    ) -> Option<models::structs::DiagramState> {
        let mut state = self.load_diagram(conn_id, db_name)?;
        crate::diagram_schema::prepare_stored_state(&mut state, conn_id, db_name);
        Some(state)
    }

    /// Mulai ambil skema live (conn, db) di background. Job untuk database
    /// yang sama tidak diduplikasi; hasilnya diproses di
    /// [`Self::poll_diagram_schema_jobs`].
    fn request_diagram_schema(&mut self, conn_id: i64, db_name: &str) {
        if self
            .diagram_schema_jobs
            .iter()
            .any(|j| j.conn_id == conn_id && j.db_name == db_name)
        {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.diagram_schema_jobs.push(DiagramSchemaJob {
            conn_id,
            db_name: db_name.to_string(),
            rx,
        });

        let Some(conn) = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .cloned()
        else {
            let _ = tx.send(Err("Connection not found".to_string()));
            return;
        };
        let Some(rt) = self.runtime.clone() else {
            let _ = tx.send(Err("Tokio runtime unavailable".to_string()));
            return;
        };
        // Tidak pernah dial di UI thread: pool yang belum siap hanya dipicu
        // pembuatannya, lalu ditunggu oleh task background.
        let pool = rt.block_on(crate::connection::pool_if_connected_or_start(self, conn_id));
        if pool.is_none()
            && let Some(err) = self.connection_errors.get(&conn_id)
        {
            let _ = tx.send(Err(err.clone()));
            return;
        }
        let req = crate::diagram_schema::SchemaFetchRequest {
            conn,
            db_name: db_name.to_string(),
            pool,
            shared_pools: self.shared_connection_pools.clone(),
            cache_pool: self.db_pool.clone(),
        };
        rt.spawn(async move {
            let _ = tx.send(crate::diagram_schema::fetch_schema_snapshot(req).await);
        });
    }

    /// Buka tab diagram untuk satu database. Layout tersimpan tampil
    /// seketika; skema live disinkronkan di background.
    pub fn open_database_diagram(&mut self, conn_id: i64, db_name: String) {
        let started = std::time::Instant::now();
        let cached = self.load_prepared_diagram(conn_id, &db_name);
        let from_cache = cached.as_ref().is_some_and(|s| !s.nodes.is_empty());
        let mut state = cached.unwrap_or_default();
        self.materialize_links(&mut state, None);
        state.schema_syncing = true;
        state.layout_baseline = Some(crate::diagram_schema::layout_fingerprint(&state));

        let title = format!("Diagram: {}", db_name);
        crate::editor::create_new_tab_with_connection_and_database(
            self,
            title,
            String::new(), // No query content
            Some(conn_id),
            Some(db_name.clone()),
        );

        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.diagram_state = Some(state);
        }
        self.table_bottom_view = models::structs::TableBottomView::Query;
        self.request_diagram_schema(conn_id, &db_name);
        log::info!(
            "[DIAGRAM_PERF] opened diagram '{db_name}' from {} in {:?}",
            if from_cache {
                "local cache"
            } else {
                "empty state"
            },
            started.elapsed()
        );
    }

    /// Terima hasil pengambilan skema yang sudah selesai. Dipanggil tiap frame.
    pub fn poll_diagram_schema_jobs(&mut self, ctx: &egui::Context) {
        if self.diagram_schema_jobs.is_empty() {
            return;
        }
        let mut done = Vec::new();
        self.diagram_schema_jobs
            .retain(|job| match job.rx.try_recv() {
                Ok(result) => {
                    done.push((job.conn_id, job.db_name.clone(), result));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done.push((
                        job.conn_id,
                        job.db_name.clone(),
                        Err("Schema fetch was interrupted".to_string()),
                    ));
                    false
                }
            });
        for (conn_id, db_name, result) in done {
            match result {
                Ok(snapshot) => self.apply_diagram_schema(conn_id, &db_name, &snapshot),
                Err(e) => self.fail_diagram_schema(conn_id, &db_name, e),
            }
        }
        if !self.diagram_schema_jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn is_diagram_host_tab(tab: &models::structs::QueryTab, conn_id: i64, db_name: &str) -> bool {
        tab.diagram_state.is_some()
            && tab.connection_id == Some(conn_id)
            && tab.database_name.as_deref() == Some(db_name)
    }

    fn links_to(link: &models::structs::LinkedDatabase, conn_id: i64, db_name: &str) -> bool {
        link.connection_id == Some(conn_id) && link.database_name == db_name
    }

    /// Terapkan skema live (conn, db) ke tab diagram database tersebut dan
    /// ke semua diagram yang me-link database tersebut.
    fn apply_diagram_schema(
        &mut self,
        conn_id: i64,
        db_name: &str,
        snapshot: &crate::diagram_schema::SchemaSnapshot,
    ) {
        use crate::diagram_schema::{layout_fingerprint, merge_schema, prepare_stored_state};

        let conn_name = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.name.clone());
        let host_tabs: Vec<usize> = (0..self.query_tabs.len())
            .filter(|&i| Self::is_diagram_host_tab(&self.query_tabs[i], conn_id, db_name))
            .collect();

        let mut shared_applied = false;
        let mut source: Option<models::structs::DiagramState> = None;
        for i in host_tabs {
            let Some(mut state) = self.query_tabs[i].diagram_state.take() else {
                continue;
            };
            // Layout bersama hanya menggantikan cache bila user belum
            // mengedit apa pun sejak tab dibuka.
            let untouched = state
                .layout_baseline
                .is_some_and(|b| b == layout_fingerprint(&state));
            let replaced = match snapshot.shared_state.clone() {
                Some(mut shared) if untouched => {
                    prepare_stored_state(&mut shared, conn_id, db_name);
                    state = shared;
                    true
                }
                Some(_) => {
                    log::info!(
                        "[DIAGRAM_DB] shared layout of '{db_name}' skipped: diagram was edited before it arrived"
                    );
                    false
                }
                None => false,
            };
            merge_schema(&mut state, snapshot, conn_id, db_name, conn_name.as_deref());
            if replaced {
                // Isi link ikut terbuang saat state diganti.
                self.materialize_links(&mut state, None);
                shared_applied = true;
            }
            state.schema_syncing = false;
            state.layout_baseline = None;
            self.save_diagram(conn_id, db_name, &state);
            source.get_or_insert_with(|| crate::diagram_links::persistable(&state));
            self.query_tabs[i].diagram_state = Some(state);
        }
        if shared_applied {
            self.toasts.info(format!(
                "Diagram loaded from table `diagram_by_tabular` in {db_name}"
            ));
        }

        let linked = self.query_tabs.iter().any(|t| {
            t.diagram_state.as_ref().is_some_and(|s| {
                s.linked_databases
                    .iter()
                    .any(|l| Self::links_to(l, conn_id, db_name))
            })
        });
        if !linked {
            return;
        }
        let source = match source {
            Some(s) => s,
            None => {
                // Tab database sumber tidak terbuka: bangun dari layout
                // bersama / cache lokal, lalu simpan supaya pembukaan
                // berikutnya instan.
                let mut st = match snapshot.shared_state.clone() {
                    Some(mut shared) => {
                        prepare_stored_state(&mut shared, conn_id, db_name);
                        shared
                    }
                    None => self
                        .load_prepared_diagram(conn_id, db_name)
                        .unwrap_or_default(),
                };
                merge_schema(&mut st, snapshot, conn_id, db_name, conn_name.as_deref());
                if !st.nodes.is_empty() {
                    self.save_diagram(conn_id, db_name, &st);
                }
                st
            }
        };
        if source.nodes.is_empty() {
            self.fail_diagram_links(
                conn_id,
                db_name,
                format!("No tables found in '{db_name}' (database offline or empty)"),
            );
            return;
        }
        for tab in &mut self.query_tabs {
            let Some(st) = tab.diagram_state.as_mut() else {
                continue;
            };
            let ids: Vec<String> = st
                .linked_databases
                .iter()
                .filter(|l| Self::links_to(l, conn_id, db_name))
                .map(|l| l.link_id.clone())
                .collect();
            for id in ids {
                crate::diagram_links::apply_link(st, &id, &source);
            }
        }
    }

    /// Pengambilan skema (conn, db) gagal: tampilan cache dipertahankan.
    fn fail_diagram_schema(&mut self, conn_id: i64, db_name: &str, error: String) {
        log::warn!("[DIAGRAM] schema of '{db_name}' not refreshed: {error}");
        let mut was_syncing = false;
        for tab in &mut self.query_tabs {
            if Self::is_diagram_host_tab(tab, conn_id, db_name)
                && let Some(st) = tab.diagram_state.as_mut()
            {
                was_syncing |= st.schema_syncing;
                st.schema_syncing = false;
                st.layout_baseline = None;
            }
        }
        if was_syncing {
            self.toasts.warning(format!(
                "Could not refresh schema of '{db_name}' (showing saved diagram): {error}"
            ));
        }
        self.fail_diagram_links(conn_id, db_name, error);
    }

    /// Tandai link ke (conn, db) yang belum termuat sebagai gagal.
    fn fail_diagram_links(&mut self, conn_id: i64, db_name: &str, error: String) {
        let mut failed = false;
        for tab in &mut self.query_tabs {
            let Some(st) = tab.diagram_state.as_mut() else {
                continue;
            };
            let ids: Vec<String> = st
                .linked_databases
                .iter()
                .filter(|l| Self::links_to(l, conn_id, db_name))
                .filter(|l| l.status != models::structs::LinkStatus::Loaded)
                .map(|l| l.link_id.clone())
                .collect();
            for id in ids {
                crate::diagram_links::mark_link_failed(st, &id, error.clone());
                failed = true;
            }
        }
        if failed {
            self.toasts.warning(format!(
                "Could not load linked database '{db_name}': {error}"
            ));
        }
    }

    /// Resolusi koneksi sebuah link: id + nama dulu, lalu nama saja. Id
    /// koneksi lokal tidak portabel antar mesin, jadi id yang cocok tapi
    /// namanya beda dianggap koneksi lain.
    fn resolve_link_connection(
        &self,
        link: &models::structs::LinkedDatabase,
    ) -> Option<(i64, String)> {
        let by_id = self.connections.iter().find(|c| {
            c.id.is_some()
                && c.id == link.connection_id
                && (link.connection_name.is_empty() || c.name == link.connection_name)
        });
        let by_name = || {
            (!link.connection_name.is_empty())
                .then(|| {
                    self.connections
                        .iter()
                        .find(|c| c.name == link.connection_name)
                })
                .flatten()
        };
        by_id
            .or_else(by_name)
            .and_then(|c| c.id.map(|id| (id, c.name.clone())))
    }

    /// Materialisasi isi kontainer link database (`only` = satu link saja)
    /// tanpa memblokir UI. Tab diagram sumber yang sedang terbuka dipakai
    /// langsung karena paling baru; selain itu cache lokal ditampilkan dulu
    /// lalu skema live diambil di background. Link yang gagal dimuat tampil
    /// sebagai placeholder; relasi lintas database ke link tersebut tetap
    /// disimpan.
    pub fn materialize_links(
        &mut self,
        state: &mut models::structs::DiagramState,
        only: Option<&str>,
    ) {
        let links: Vec<models::structs::LinkedDatabase> = state
            .linked_databases
            .iter()
            .filter(|l| only.is_none_or(|id| l.link_id == id))
            .cloned()
            .collect();
        for link in links {
            let Some((cid, name)) = self.resolve_link_connection(&link) else {
                let e = format!(
                    "Connection '{}' not found on this machine",
                    link.connection_name
                );
                log::warn!(
                    "[DIAGRAM_LINK] {}/{} not loaded: {e}",
                    link.connection_name,
                    link.database_name
                );
                crate::diagram_links::mark_link_failed(state, &link.link_id, e);
                continue;
            };
            if let Some(l) = state
                .linked_databases
                .iter_mut()
                .find(|l| l.link_id == link.link_id)
            {
                l.connection_id = Some(cid);
                l.connection_name = name;
            }
            let open_source = self.query_tabs.iter().find_map(|t| {
                Self::is_diagram_host_tab(t, cid, &link.database_name)
                    .then_some(t.diagram_state.as_ref())
                    .flatten()
            });
            if let Some(open) = open_source {
                let source = crate::diagram_links::persistable(open);
                crate::diagram_links::apply_link(state, &link.link_id, &source);
                continue;
            }
            if let Some(cached) = self
                .load_prepared_diagram(cid, &link.database_name)
                .filter(|s| !s.nodes.is_empty())
            {
                crate::diagram_links::apply_link(state, &link.link_id, &cached);
            }
            self.request_diagram_schema(cid, &link.database_name);
        }
    }

    /// Muat ulang link database pada diagram di tab `tab_idx`.
    pub fn refresh_diagram_links(&mut self, tab_idx: usize, only: Option<&str>) {
        let Some(mut state) = self
            .query_tabs
            .get_mut(tab_idx)
            .and_then(|t| t.diagram_state.take())
        else {
            return;
        };
        self.materialize_links(&mut state, only);
        if let Some(tab) = self.query_tabs.get_mut(tab_idx) {
            tab.diagram_state = Some(state);
        }
    }

    /// Terapkan diagram (conn, db) yang baru disimpan ke semua diagram
    /// gabungan yang me-link database tersebut.
    pub fn propagate_diagram_to_links(
        &mut self,
        conn_id: i64,
        db_name: &str,
        state: &models::structs::DiagramState,
    ) {
        let source = crate::diagram_links::persistable(state);
        for tab in &mut self.query_tabs {
            let Some(st) = tab.diagram_state.as_mut() else {
                continue;
            };
            let ids: Vec<String> = st
                .linked_databases
                .iter()
                .filter(|l| l.connection_id == Some(conn_id) && l.database_name == db_name)
                .map(|l| l.link_id.clone())
                .collect();
            for id in ids {
                crate::diagram_links::apply_link(st, &id, &source);
            }
        }
    }

    /// Simpan diagram ke cache lokal lalu perbarui diagram gabungan yang
    /// me-link database ini.
    pub fn save_diagram_and_propagate(
        &mut self,
        conn_id: i64,
        db_name: &str,
        state: &models::structs::DiagramState,
    ) {
        self.save_diagram(conn_id, db_name, state);
        self.propagate_diagram_to_links(conn_id, db_name, state);
    }

    /// Buka dialog Link Database; `relink` = ganti koneksi link yang ada.
    fn open_link_database_modal(&mut self, relink: Option<String>) {
        let preset = relink.as_deref().and_then(|id| {
            let link = self
                .query_tabs
                .get(self.active_tab_index)?
                .diagram_state
                .as_ref()?
                .linked_databases
                .iter()
                .find(|l| l.link_id == id)?
                .clone();
            let cid = self
                .resolve_link_connection(&link)
                .map(|(cid, _)| cid)
                .or(link.connection_id);
            Some((cid, link.database_name))
        });
        let Some(st) = self
            .query_tabs
            .get_mut(self.active_tab_index)
            .and_then(|t| t.diagram_state.as_mut())
        else {
            return;
        };
        st.show_link_modal = true;
        st.link_modal_relink = relink;
        st.link_modal_db_options.clear();
        st.link_modal_db_options_for = None;
        match preset {
            Some((cid, db)) => {
                st.link_modal_conn = cid;
                st.link_modal_db = db;
            }
            None => {
                st.link_modal_conn = None;
                st.link_modal_db.clear();
            }
        }
    }

    /// Buka (atau pindah ke) tab diagram sumber sebuah link.
    fn open_linked_source_diagram(&mut self, link_id: &str) {
        let Some(link) = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|t| t.diagram_state.as_ref())
            .and_then(|st| st.linked_databases.iter().find(|l| l.link_id == link_id))
            .cloned()
        else {
            return;
        };
        let Some((cid, _)) = self.resolve_link_connection(&link) else {
            self.toasts.error(format!(
                "Connection '{}' not found. Use Relink to choose another connection.",
                link.connection_name
            ));
            return;
        };
        let existing = self.query_tabs.iter().position(|t| {
            t.diagram_state.is_some()
                && t.connection_id == Some(cid)
                && t.database_name.as_deref() == Some(link.database_name.as_str())
        });
        match existing {
            Some(idx) => crate::editor::switch_to_tab(self, idx),
            None => self.open_database_diagram(cid, link.database_name),
        }
    }

    /// Daftar database sebuah koneksi untuk dialog Link Database: cache memori,
    /// lalu cache SQLite lokal, terakhir query langsung (timeout 10 detik).
    /// `force` melewati cache. Schema sistem disembunyikan.
    fn databases_for_link_dialog(&mut self, conn_id: i64, force: bool) -> Vec<String> {
        const SYSTEM_DBS: &[&str] = &[
            "information_schema",
            "performance_schema",
            "mysql",
            "sys",
            "master",
            "tempdb",
            "model",
            "msdb",
        ];
        let mut dbs = if force {
            None
        } else {
            self.database_cache
                .get(&conn_id)
                .filter(|d| !d.is_empty())
                .cloned()
                .or_else(|| {
                    crate::cache_data::get_databases_from_cache(self, conn_id)
                        .filter(|d| !d.is_empty())
                })
        };
        if dbs.is_none()
            && let Some(rt) = self.runtime.clone()
        {
            let fetched = rt.block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    crate::connection::metadata::fetch_databases_from_connection_async(
                        self, conn_id,
                    ),
                )
                .await
            });
            match fetched {
                Ok(Some(list)) if !list.is_empty() => dbs = Some(list),
                Ok(_) => log::warn!("[DIAGRAM_LINK] no databases returned for conn {conn_id}"),
                Err(_) => self
                    .toasts
                    .warning("Loading the database list timed out (10s)"),
            }
        }
        let mut dbs = dbs.unwrap_or_default();
        if !dbs.is_empty() {
            self.database_cache.insert(conn_id, dbs.clone());
        }
        dbs.retain(|d| !SYSTEM_DBS.contains(&d.to_lowercase().as_str()));
        // Fallback: database default koneksi (mis. SQLite tanpa daftar database).
        if dbs.is_empty()
            && let Some(c) = self.connections.iter().find(|c| c.id == Some(conn_id))
            && !c.database.is_empty()
        {
            dbs.push(c.database.clone());
        }
        dbs.sort_by_key(|d| d.to_lowercase());
        dbs.dedup();
        dbs
    }

    pub fn render_link_database_dialog(&mut self, ctx: &egui::Context) {
        use models::enums::DatabaseType;

        let tab_idx = self.active_tab_index;
        let Some(tab) = self.query_tabs.get(tab_idx) else {
            return;
        };
        let Some(st) = tab.diagram_state.as_ref() else {
            return;
        };
        if !st.show_link_modal {
            return;
        }
        let host = (tab.connection_id, tab.database_name.clone());
        let relink = st.link_modal_relink.clone();
        let existing: Vec<(Option<i64>, String, String)> = st
            .linked_databases
            .iter()
            .map(|l| (l.connection_id, l.database_name.clone(), l.link_id.clone()))
            .collect();
        // Hanya koneksi relasional yang punya diagram.
        let conn_options: Vec<(i64, String, String, String)> = self
            .connections
            .iter()
            .filter(|c| {
                matches!(
                    c.connection_type,
                    DatabaseType::MySQL
                        | DatabaseType::PostgreSQL
                        | DatabaseType::SQLite
                        | DatabaseType::MsSQL
                )
            })
            .filter_map(|c| {
                let host_label = if c.host.is_empty() {
                    c.database.clone()
                } else if c.port.is_empty() {
                    c.host.clone()
                } else {
                    format!("{}:{}", c.host, c.port)
                };
                c.id.map(|id| {
                    (
                        id,
                        c.name.clone(),
                        format!("{:?}", c.connection_type),
                        host_label,
                    )
                })
            })
            .collect();

        // Database yang tidak bisa dipilih: milik diagram ini atau sudah di-link.
        let unavailable = |cid: i64, db: &str| -> Option<&'static str> {
            if host.0 == Some(cid) && host.1.as_deref() == Some(db) {
                Some("this diagram")
            } else if existing.iter().any(|(c, d, id)| {
                *c == Some(cid) && d == db && relink.as_deref() != Some(id.as_str())
            }) {
                Some("already linked")
            } else {
                None
            }
        };

        // 1. Pilihan koneksi awal: koneksi diagram ini, atau koneksi pertama.
        let (selected_conn, loaded_for, reload) = {
            let st = tab.diagram_state.as_ref().expect("checked above");
            (
                st.link_modal_conn,
                st.link_modal_db_options_for,
                st.link_modal_db_reload,
            )
        };
        let selected_conn = selected_conn
            .filter(|cid| conn_options.iter().any(|(id, ..)| id == cid))
            .or_else(|| {
                host.0
                    .filter(|cid| conn_options.iter().any(|(id, ..)| id == cid))
                    .or_else(|| conn_options.first().map(|(id, ..)| *id))
            });

        // 2. Muat daftar database bila koneksi berganti / diminta reload.
        let fresh_options = match selected_conn {
            Some(cid) if loaded_for != Some(cid) || reload => {
                Some(self.databases_for_link_dialog(cid, reload))
            }
            _ => None,
        };

        let Some(st) = self
            .query_tabs
            .get_mut(tab_idx)
            .and_then(|t| t.diagram_state.as_mut())
        else {
            return;
        };
        st.link_modal_conn = selected_conn;
        st.link_modal_db_reload = false;
        if let Some(options) = fresh_options {
            st.link_modal_db_options = options;
            st.link_modal_db_options_for = selected_conn;
            // Pertahankan pilihan yang masih valid, selain itu pilih database
            // pertama yang tersedia.
            let keep = selected_conn.is_some_and(|cid| {
                st.link_modal_db_options.contains(&st.link_modal_db)
                    && unavailable(cid, &st.link_modal_db).is_none()
            });
            if !keep {
                st.link_modal_db = selected_conn
                    .and_then(|cid| {
                        st.link_modal_db_options
                            .iter()
                            .find(|d| unavailable(cid, d).is_none())
                            .cloned()
                    })
                    .unwrap_or_default();
            }
        }

        let mut cancelled = false;
        let mut confirm: Option<(i64, String, String)> = None;
        let title = if relink.is_some() {
            "Relink Database"
        } else {
            "Link Database"
        };
        const FIELD_WIDTH: f32 = 300.0;

        crate::window_egui::style::render_modal_backdrop(
            ctx,
            "link_database_backdrop",
            st.show_link_modal,
        );

        egui::Window::new(title)
            .title_bar(false)
            .frame(crate::window_egui::style::modal_window_frame(ctx))
            .collapsible(false)
            .resizable(false)
            .default_width(450.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                crate::window_egui::style::render_modal_header(ui, title, &mut cancelled);
                ui.add_space(8.0);

                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
                    ui.label(
                        egui::RichText::new(if relink.is_some() {
                            "Point this linked database at another connection. \
                             Relations to its tables are kept."
                        } else {
                            "All tables of the selected database appear in their own container \
                             and follow that database's diagram."
                        })
                        .weak(),
                    );
                    ui.add_space(8.0);

                    egui::Grid::new("link_db_grid")
                        .num_columns(2)
                        .spacing([12.0, 10.0])
                        .min_col_width(80.0)
                        .show(ui, |ui| {
                            // Connection
                            ui.label("Connection");
                            let curr = st.link_modal_conn;
                            let curr_opt = curr
                                .and_then(|cid| conn_options.iter().find(|(id, ..)| *id == cid));
                            let selected_text = match curr_opt {
                                Some((_, name, kind, _)) => format!("{name}  ·  {kind}"),
                                None => "Select connection".to_string(),
                            };
                            let combo = egui::ComboBox::from_id_salt("link_db_conn_combo")
                                .width(FIELD_WIDTH)
                                .selected_text(selected_text)
                                .show_ui(ui, |ui| {
                                    if conn_options.is_empty() {
                                        ui.label(
                                            egui::RichText::new("No relational connections").weak(),
                                        );
                                    }
                                    for (cid, cname, kind, host_label) in &conn_options {
                                        let selected = Some(*cid) == curr;
                                        let text = format!("{cname}  ·  {kind}");
                                        let resp = ui
                                            .selectable_label(selected, text)
                                            .on_hover_text(host_label);
                                        if resp.clicked() && !selected {
                                            st.link_modal_conn = Some(*cid);
                                            st.link_modal_db.clear();
                                        }
                                    }
                                });
                            if let Some((.., host_label)) = curr_opt {
                                combo.response.on_hover_text(host_label);
                            }
                            ui.end_row();

                            // Database
                            ui.label("Database");
                            ui.horizontal(|ui| {
                                let cid = st.link_modal_conn;
                                let loading = cid != st.link_modal_db_options_for;
                                let selected_text = if loading {
                                    egui::RichText::new("Loading…").weak()
                                } else if st.link_modal_db.is_empty() {
                                    egui::RichText::new("Select database").weak()
                                } else {
                                    egui::RichText::new(st.link_modal_db.clone())
                                };
                                let reload_width = 28.0;
                                egui::ComboBox::from_id_salt("link_db_db_combo")
                                    .width(FIELD_WIDTH - reload_width - 8.0)
                                    .height(320.0)
                                    .selected_text(selected_text)
                                    .show_ui(ui, |ui| {
                                        if st.link_modal_db_options.is_empty() {
                                            ui.label(
                                                egui::RichText::new("No databases found").weak(),
                                            );
                                        }
                                        let Some(cid) = cid else {
                                            return;
                                        };
                                        for db in &st.link_modal_db_options {
                                            let selected = *db == st.link_modal_db;
                                            match unavailable(cid, db) {
                                                Some(reason) => {
                                                    ui.add_enabled(
                                                        false,
                                                        egui::Button::selectable(
                                                            false,
                                                            format!("{db}  —  {reason}"),
                                                        ),
                                                    );
                                                }
                                                None => {
                                                    if ui.selectable_label(selected, db).clicked() {
                                                        st.link_modal_db = db.clone();
                                                    }
                                                }
                                            }
                                        }
                                    });
                                if ui
                                    .add_sized(
                                        [reload_width, ui.spacing().interact_size.y],
                                        egui::Button::new(
                                            egui_icons::icons::ICON_REFRESH.codepoint,
                                        ),
                                    )
                                    .on_hover_text("Reload database list from the server")
                                    .clicked()
                                {
                                    st.link_modal_db_reload = true;
                                }
                            });
                            ui.end_row();
                        });
                });

                let db = st.link_modal_db.trim().to_string();
                let ready = st
                    .link_modal_conn
                    .is_some_and(|cid| !db.is_empty() && unavailable(cid, &db).is_none());

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if relink.is_some() {
                            "Relink"
                        } else {
                            "Link Database"
                        };
                        let primary = egui::Button::new(
                            egui::RichText::new(label)
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(crate::window_egui::style::theme_accent(ui.ctx()))
                        .min_size(egui::vec2(110.0, 0.0));
                        if ui.add_enabled(ready, primary).clicked()
                            && let Some(cid) = st.link_modal_conn
                        {
                            let name = conn_options
                                .iter()
                                .find(|(id, ..)| *id == cid)
                                .map(|(_, n, ..)| n.clone())
                                .unwrap_or_default();
                            confirm = Some((cid, name, db.clone()));
                        }
                    });
                });
            });

        if cancelled || confirm.is_some() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            st.show_link_modal = false;
            st.link_modal_relink = None;
        }
        let Some((cid, name, db)) = confirm else {
            return;
        };

        let link_id = match relink {
            Some(id) => {
                if let Some(l) = st.linked_databases.iter_mut().find(|l| l.link_id == id) {
                    l.connection_id = Some(cid);
                    l.connection_name = name;
                    l.database_name = db.clone();
                }
                id
            }
            None => {
                let id = crate::diagram_links::new_link_id(&st.linked_databases);
                let colors = crate::diagram_view::GROUP_COLORS;
                st.linked_databases.push(models::structs::LinkedDatabase {
                    link_id: id.clone(),
                    connection_id: Some(cid),
                    connection_name: name,
                    database_name: db.clone(),
                    offset: crate::diagram_links::next_link_offset(st),
                    color: colors[(st.linked_databases.len() * 3 + 5) % colors.len()],
                    status: models::structs::LinkStatus::Pending,
                });
                id
            }
        };

        self.refresh_diagram_links(tab_idx, Some(&link_id));

        let Some(st) = self
            .query_tabs
            .get_mut(tab_idx)
            .and_then(|t| t.diagram_state.as_mut())
        else {
            return;
        };
        // Simpan daftar link (isi kontainernya sendiri tidak ikut disimpan).
        st.save_requested = true;
        let status = st
            .linked_databases
            .iter()
            .find(|l| l.link_id == link_id)
            .map(|l| l.status.clone());
        let tables = st
            .nodes
            .iter()
            .filter(|n| crate::diagram_links::link_id_of(&n.id) == Some(link_id.as_str()))
            .count();
        match status {
            Some(models::structs::LinkStatus::Failed(e)) => self
                .toasts
                .warning(format!("Linked '{db}', but it could not be loaded: {e}")),
            _ => self
                .toasts
                .success(format!("Linked database '{db}' ({tables} tables)")),
        };
    }

    pub fn sync_diagram_to_server(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
    ) {
        if self.sync_account.is_none() {
            self.toasts
                .warning("Please sign in to Tabular to sync diagrams to cloud");
            crate::sync::ui_login::open_account_dialog(self);
            return;
        }

        if self.vault.is_none() {
            self.toasts
                .warning("Vault is locked. Please unlock your vault to sync.");
            crate::sync::ui_login::open_account_dialog(self);
            return;
        }

        let account = self.sync_account.as_ref().unwrap();
        let vault = self.vault.as_ref().unwrap();

        let db = db_name.unwrap_or_else(|| "default".to_string());
        let diag_title = state.diagram_title.clone().unwrap_or_else(|| {
            if let Some(cid) = conn_id {
                let conn_name = self
                    .connections
                    .iter()
                    .find(|c| c.id == Some(cid))
                    .map(|c| c.name.clone())
                    .unwrap_or_else(|| "diagram".to_string());
                format!("{}_{}", conn_name, db)
            } else {
                format!("diagram_{}", db)
            }
        });

        let (tx, rx) = std::sync::mpsc::channel();
        let token = account.access_token.clone();
        let server_url = self.sync_server_url.clone();
        let team_keys = self.vault_team_keys.clone();
        let shared_folders = self.shared_folders_cache.clone();

        crate::sync::sync_diagrams::push_single_diagram(
            diag_title.clone(),
            crate::diagram_links::persistable(state),
            vault.account_key.clone(),
            team_keys,
            shared_folders,
            token,
            server_url,
            tx,
        );

        self.toasts
            .info(format!("Syncing diagram '{}' to cloud…", diag_title));

        if let Some(rt) = &self.runtime {
            rt.spawn(async move {
                match rx.recv() {
                    Ok(Ok(remote_id)) => {
                        log::info!(
                            "[SYNC] Diagram '{}' synced successfully (id: {})",
                            diag_title,
                            remote_id
                        );
                    }
                    Ok(Err(e)) => {
                        log::error!("[SYNC] Diagram sync failed: {}", e);
                    }
                    Err(_) => {}
                }
            });
        }
    }
}
