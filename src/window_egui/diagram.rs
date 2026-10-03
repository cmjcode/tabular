use crate::models;
use eframe::egui;

/// Pengambilan skema satu database (conn, db) yang sedang berjalan di
/// background untuk diagram ERD.
pub struct DiagramSchemaJob {
    conn_id: i64,
    db_name: String,
    rx: std::sync::mpsc::Receiver<Result<crate::diagram_schema::SchemaSnapshot, String>>,
}

/// Tab diagram fokus (tabel + relasinya) yang dibuka sebelum diagram
/// database-nya ada di cache; diisi saat skema (conn, db) selesai diambil.
pub struct DiagramFocusRequest {
    conn_id: i64,
    db_name: String,
    table: String,
    /// [`models::structs::QueryTab::id`] tab placeholder.
    tab_id: usize,
}

/// Pemindaian repository untuk saran tabel sebuah group diagram.
pub struct DiagramRepoScanJob {
    conn_id: Option<i64>,
    db_name: Option<String>,
    group_id: String,
    handle: crate::repo_scan::ScanHandle,
}

/// Nomor urut simpan diagram (global, naik terus).
static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Nomor urut simpan terbaru per file. Tulisan yang lebih tua dari ini
/// dibuang supaya layout lama tidak menimpa layout baru.
static LATEST_SAVE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, u64>>,
> = std::sync::OnceLock::new();

/// Simpan diagram ke `path` di thread background. Isi kontainer link database
/// tidak disimpan, hanya referensinya. Penulisan atomik dan berurutan: bila
/// dua simpan berjalan bersamaan, hanya yang terbaru yang menulis.
pub(crate) fn save_diagram_file_async(
    path: std::path::PathBuf,
    state: models::structs::DiagramState,
) -> std::thread::JoinHandle<()> {
    let seq = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let latest = LATEST_SAVE.get_or_init(Default::default);
    match latest.lock() {
        Ok(mut map) => {
            map.insert(path.clone(), seq);
        }
        Err(e) => log::warn!("[DIAGRAM] save sequence lock poisoned: {e}"),
    }
    std::thread::spawn(move || {
        let state = crate::diagram_links::persistable(&state);
        let bytes = match serde_json::to_vec_pretty(&state) {
            Ok(b) => b,
            Err(e) => {
                log::error!("[DIAGRAM] Failed to serialize diagram {:?}: {}", path, e);
                return;
            }
        };
        // Lock ditahan selama menulis supaya dua penulis tidak berselang-seling.
        let map = match latest.lock() {
            Ok(m) => m,
            Err(e) => {
                log::error!("[DIAGRAM] save sequence lock poisoned: {e}");
                return;
            }
        };
        if map.get(&path) != Some(&seq) {
            log::debug!("[DIAGRAM] Skipping stale save of {:?}", path);
            return;
        }
        // Tulis atomik supaya layout lama tidak rusak bila app crash saat menyimpan.
        match crate::diagram_view::write_atomic(&path, &bytes) {
            Ok(()) => log::debug!("Diagram layout saved to {:?}", path),
            Err(e) => log::error!("Failed to save diagram {:?}: {}", path, e),
        }
    })
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
        path.push(crate::diagram_storage::local_diagram_file_name(
            conn_id, db_name,
        ));
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
        // Serialisasi + tulis file di background: diagram besar bisa makan
        // puluhan ms dan membuat UI tersendat setiap kali drag dilepas.
        save_diagram_file_async(path, state.clone());
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
        // Tab subset hanya tampilan: aksi yang menyimpan, memuat, atau
        // me-link akan menimpa diagram database dengan isi subset.
        let modifies_source = matches!(
            action,
            DiagramAction::Save
                | DiagramAction::SaveToVault
                | DiagramAction::SyncWithDatabase
                | DiagramAction::OpenLinkDatabaseModal
                | DiagramAction::RelinkDatabase(_)
                | DiagramAction::SuggestGroupTables(_)
                | DiagramAction::GenerateFlows { .. }
        );
        if modifies_source && state.scoped_to.is_some() {
            self.toasts.info(
                "This tab shows only a subset of tables and is not saved. Use the full diagram tab instead.",
            );
            return;
        }
        match action {
            DiagramAction::OpenFocusInNewTab(table) => {
                self.open_focus_subset_tab(conn_id, db_name, state, &table)
            }
            DiagramAction::OpenGroupInNewTab(group_id) => {
                self.open_group_subset_tab(conn_id, db_name, state, &group_id)
            }
            DiagramAction::OpenFlowInNewTab(card_id) => {
                self.open_flow_subset_tab(conn_id, db_name, state, &card_id)
            }
            DiagramAction::ConfigureTableGroups => {
                let cid = conn_id.or_else(|| state.nodes.iter().find_map(|n| n.connection_id));
                let db = db_name.or_else(|| state.nodes.iter().find_map(|n| n.database_name.clone()));
                if let (Some(cid), Some(db)) = (cid, db) {
                    self.open_table_group_dialog(cid, db);
                } else {
                    self.toasts.warning("Cannot configure table groups: no connection or database selected");
                }
            }
            DiagramAction::ReloadSchema => {
                let cid = conn_id.or_else(|| state.nodes.iter().find_map(|n| n.connection_id));
                let db = db_name.or_else(|| state.nodes.iter().find_map(|n| n.database_name.clone()));
                if let (Some(cid), Some(db)) = (cid, db) {
                    self.request_diagram_schema(cid, &db);
                }
            }
            DiagramAction::Save => self.save_diagram_with_defaults(conn_id, db_name, state),
            DiagramAction::Info(msg) => self.toasts.success(msg),
            DiagramAction::Error(msg) => self.toasts.error(msg),
            DiagramAction::SaveToVault => self.save_diagram_to_vault(conn_id, db_name, state),
            DiagramAction::SyncWithDatabase => {
                if let (Some(cid), Some(db)) = (conn_id, db_name.as_deref()) {
                    self.sync_diagram_with_database(cid, db);
                }
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
            DiagramAction::SuggestGroupTables(group_id) => {
                self.start_group_table_scan(conn_id, db_name, &group_id);
            }
            DiagramAction::GenerateFlows {
                group_id,
                card_ids,
                force,
            } => {
                self.start_flow_generation(conn_id, db_name, group_id.as_deref(), &card_ids, force)
            }
            DiagramAction::OpenEndpointRequest { request_id, label } => match request_id {
                Some(request_id) => crate::http_repo::perform(
                    self,
                    crate::http_repo::RepoAction::OpenRequest { request_id, label },
                ),
                None => self.toasts.info(format!(
                    "{label} has no saved request. Generate endpoints from the repository of the \
                     linked HTTP API folder."
                )),
            },
            DiagramAction::ShowLinkedHttpFolders(group_id) => {
                let Some(group) = state.groups.iter().find(|g| g.id == group_id) else {
                    return;
                };
                match group.repo_key() {
                    Some(key) => crate::http_repo::perform(
                        self,
                        crate::http_repo::RepoAction::ShowLinkedFolders {
                            key,
                            group_title: group.title.clone(),
                        },
                    ),
                    None => self
                        .toasts
                        .info("Set a git repository for this group first (Set Repository…)"),
                }
            }
        }
    }

    /// State diagram penuh milik (conn, db) yang sedang terbuka.
    pub(crate) fn diagram_state_for_mut(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<&str>,
    ) -> Option<&mut models::structs::DiagramState> {
        self.query_tabs
            .iter_mut()
            .filter(|t| t.connection_id == conn_id && t.database_name.as_deref() == db_name)
            .filter_map(|t| t.diagram_state.as_mut())
            .find(|s| s.scoped_to.is_none())
    }

    /// Mulai pemindaian repository group `group_id` dan buka jendela saran.
    fn start_group_table_scan(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        group_id: &str,
    ) {
        // Satu pemindaian per group; permintaan baru menggantikan yang lama.
        self.diagram_repo_scan_jobs.retain(|job| {
            let same = job.group_id == group_id && job.conn_id == conn_id && job.db_name == db_name;
            if same {
                job.handle.cancel();
            }
            !same
        });

        let target = self.effective_chat_target();
        let (backend, backend_label, backend_note) =
            match crate::ai_assistant::backend_ready_for(self, target) {
                Ok(()) => (
                    Some(crate::ai_assistant::chat_backend_for(self, target)),
                    crate::ai_assistant::backend_label_for(self, target),
                    None,
                ),
                Err(e) => (None, String::new(), Some(e)),
            };

        let Some(state) = self.diagram_state_for_mut(conn_id, db_name.as_deref()) else {
            self.toasts.error("Diagram tab is no longer open");
            return;
        };
        let Some(group) = state.groups.iter().find(|g| g.id == group_id) else {
            self.toasts.error("Group not found");
            return;
        };
        if !group.has_repository() {
            state.group_repo_editor = Some(models::structs::GroupRepoDraft {
                group_id: group_id.to_string(),
                ..Default::default()
            });
            return;
        }
        let repo_path = group.local_repo_path();
        let repo_url = group.repo_url.clone();
        let group_title = group.title.clone();

        // Tabel milik database yang di-link mengikuti diagram sumbernya.
        let candidates: Vec<crate::repo_scan::Candidate> = state
            .nodes
            .iter()
            .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
            .map(|n| crate::repo_scan::Candidate {
                id: n.id.clone(),
                title: n.title.clone(),
            })
            .collect();
        if candidates.is_empty() {
            self.toasts.info("This diagram has no tables to suggest");
            return;
        }
        let members: Vec<String> = state
            .nodes
            .iter()
            .filter(|n| n.is_in_group(group_id))
            .map(|n| n.title.clone())
            .collect();

        state.group_table_suggestions = Some(models::structs::GroupTableSuggestions {
            group_id: group_id.to_string(),
            group_title: group_title.clone(),
            running: true,
            note: backend_note,
            started_at: Some(std::time::Instant::now()),
            ..Default::default()
        });

        log::info!(
            "[DIAGRAM] scanning repository for group '{group_title}' ({} candidate table(s), AI: {})",
            candidates.len(),
            if backend.is_some() {
                backend_label.as_str()
            } else {
                "off"
            }
        );
        let handle = crate::repo_scan::spawn_scan(crate::repo_scan::ScanInput {
            repo_path,
            repo_url,
            group_title,
            members,
            candidates,
            backend,
            backend_label,
            cache_root: crate::repo_scan::default_cache_root(),
        });
        self.diagram_repo_scan_jobs.push(DiagramRepoScanJob {
            conn_id,
            db_name,
            group_id: group_id.to_string(),
            handle,
        });
    }

    /// Terima kemajuan dan hasil pemindaian repository. Dipanggil tiap frame.
    pub fn poll_diagram_repo_scan_jobs(&mut self, ctx: &egui::Context) {
        if self.diagram_repo_scan_jobs.is_empty() {
            return;
        }
        let jobs = std::mem::take(&mut self.diagram_repo_scan_jobs);
        let mut keep = Vec::with_capacity(jobs.len());
        for job in jobs {
            let Some(state) = self.diagram_state_for_mut(job.conn_id, job.db_name.as_deref())
            else {
                job.handle.cancel(); // tab ditutup
                continue;
            };
            // Jendela ditutup atau sudah berganti group: hentikan job.
            let Some(sugg) = state
                .group_table_suggestions
                .as_mut()
                .filter(|s| s.group_id == job.group_id)
            else {
                job.handle.cancel();
                continue;
            };
            if sugg.cancel_requested {
                job.handle.cancel();
            }
            let mut finished = false;
            loop {
                match job.handle.rx.try_recv() {
                    Ok(crate::repo_scan::ScanEvent::Progress(step)) => {
                        sugg.last_activity_at = Some(std::time::Instant::now());
                        upsert_progress(&mut sugg.progress, step);
                    }
                    Ok(crate::repo_scan::ScanEvent::Activity) => {
                        sugg.last_activity_at = Some(std::time::Instant::now());
                    }
                    Ok(crate::repo_scan::ScanEvent::Finished(result)) => {
                        sugg.running = false;
                        sugg.elapsed = sugg.started_at.map(|t| t.elapsed());
                        match result {
                            Ok(outcome) => {
                                // Tool agent yang tidak melaporkan hasil dianggap selesai.
                                for s in &mut sugg.progress {
                                    if s.status == crate::agent::harness::ProgressStatus::Active {
                                        s.status = crate::agent::harness::ProgressStatus::Done;
                                    }
                                }
                                sugg.items = outcome.items;
                                sugg.unknown = outcome.unknown;
                                sugg.note = match (sugg.note.take(), outcome.note) {
                                    (Some(a), Some(b)) => Some(format!("{a} {b}")),
                                    (a, b) => b.or(a),
                                };
                            }
                            Err(e) => sugg.error = Some(e),
                        }
                        finished = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        sugg.running = false;
                        sugg.elapsed = sugg.started_at.map(|t| t.elapsed());
                        sugg.error = Some("Repository scan stopped unexpectedly".to_string());
                        finished = true;
                        break;
                    }
                }
            }
            if !finished {
                keep.push(job);
            }
        }
        // Job yang dimulai selama polling (tidak mungkin sekarang) tetap dipertahankan.
        keep.append(&mut self.diagram_repo_scan_jobs);
        self.diagram_repo_scan_jobs = keep;
        if !self.diagram_repo_scan_jobs.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Save: simpan ke cache lokal, kirim ke tabel `diagram_by_tabular` di
    /// database target (penyimpanan utama), dan ke vault Obsidian bila aktif.
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

        self.save_diagram_and_propagate(cid, &db, state);
        self.start_diagram_db_save(cid, &db, true);
        if self.obsidian_root().is_some() {
            self.save_diagram_to_vault(Some(cid), Some(db), state);
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
    pub(crate) fn request_diagram_schema(&mut self, conn_id: i64, db_name: &str) {
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
        state.db_status = models::structs::DiagramDbStatus::Checking;

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

    /// Tampilkan diagram penuh (conn, db): pindah ke tab yang sudah terbuka,
    /// atau buka tab baru.
    pub fn show_database_diagram(&mut self, conn_id: i64, db_name: String) {
        match self
            .query_tabs
            .iter()
            .position(|t| Self::is_diagram_host_tab(t, conn_id, &db_name))
        {
            Some(idx) => crate::editor::switch_to_tab(self, idx),
            None => self.open_database_diagram(conn_id, db_name),
        }
    }

    /// Buka tab baru berisi `table` dan semua tabel yang berelasi dengannya.
    /// Sumber diagram: tab diagram penuh yang terbuka, lalu cache lokal. Bila
    /// keduanya belum memuat tabel itu, tab placeholder dibuka dan diisi
    /// setelah skema live selesai diambil di background.
    pub fn open_table_focus_diagram(&mut self, conn_id: i64, db_name: String, table: String) {
        let open_host = self
            .query_tabs
            .iter()
            .find(|t| Self::is_diagram_host_tab(t, conn_id, &db_name))
            .and_then(|t| t.diagram_state.clone());
        let source = match open_host {
            Some(s) => Some(s),
            None => self.load_prepared_diagram(conn_id, &db_name).map(|mut s| {
                self.materialize_links(&mut s, None);
                s
            }),
        };
        if let Some(src) = source
            && let Some(id) = crate::diagram_view::find_table_id(&src, &table)
        {
            self.open_focus_subset_tab(Some(conn_id), Some(db_name), &src, &id);
            return;
        }

        let placeholder = models::structs::DiagramState {
            scoped_to: Some(table.clone()),
            schema_syncing: true,
            ..Default::default()
        };
        self.open_subset_tab(
            Some(conn_id),
            Some(db_name.clone()),
            format!("Diagram: {table} + related"),
            placeholder,
        );
        if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
            self.diagram_focus_requests.push(DiagramFocusRequest {
                conn_id,
                db_name: db_name.clone(),
                table,
                tab_id: tab.id,
            });
        }
        self.request_diagram_schema(conn_id, &db_name);
    }

    /// Ambil request fokus yang menunggu skema (conn, db).
    fn take_focus_requests(&mut self, conn_id: i64, db_name: &str) -> Vec<DiagramFocusRequest> {
        let (done, rest) = std::mem::take(&mut self.diagram_focus_requests)
            .into_iter()
            .partition(|r| r.conn_id == conn_id && r.db_name == db_name);
        self.diagram_focus_requests = rest;
        done
    }

    /// Isi tab placeholder fokus setelah skema (conn, db) tersedia.
    fn resolve_focus_requests(
        &mut self,
        conn_id: i64,
        db_name: &str,
        snapshot: &crate::diagram_schema::SchemaSnapshot,
        conn_name: Option<&str>,
    ) {
        let requests = self.take_focus_requests(conn_id, db_name);
        if requests.is_empty() {
            return;
        }
        let open_host = self
            .query_tabs
            .iter()
            .find(|t| Self::is_diagram_host_tab(t, conn_id, db_name))
            .and_then(|t| t.diagram_state.clone());
        let source = match open_host {
            Some(s) => s,
            None => {
                let mut st = self.build_schema_source(conn_id, db_name, snapshot, conn_name);
                self.materialize_links(&mut st, None);
                st
            }
        };
        for req in requests {
            let Some(tab) = self.query_tabs.iter_mut().find(|t| t.id == req.tab_id) else {
                continue; // tab sudah ditutup user
            };
            match crate::diagram_view::find_table_id(&source, &req.table) {
                Some(id) => {
                    let subset = crate::diagram_view::focus_subset_state(&source, &id);
                    log::info!(
                        "[DIAGRAM] focus '{}' ready with {} table(s)",
                        req.table,
                        subset.nodes.len()
                    );
                    tab.diagram_state = Some(subset);
                }
                None => {
                    if let Some(st) = tab.diagram_state.as_mut() {
                        st.schema_syncing = false;
                    }
                    self.toasts.warning(format!(
                        "Table '{}' not found in the diagram of '{db_name}'",
                        req.table
                    ));
                }
            }
        }
    }

    /// Skema live gagal diambil: hentikan indikator sync tab fokus terkait.
    fn fail_focus_requests(&mut self, conn_id: i64, db_name: &str, error: &str) {
        let requests = self.take_focus_requests(conn_id, db_name);
        for req in &requests {
            if let Some(st) = self
                .query_tabs
                .iter_mut()
                .find(|t| t.id == req.tab_id)
                .and_then(|t| t.diagram_state.as_mut())
            {
                st.schema_syncing = false;
            }
        }
        if let Some(req) = requests.first() {
            self.toasts.warning(format!(
                "Could not load diagram for '{}': {error}",
                req.table
            ));
        }
    }

    /// State diagram (conn, db) dari layout bersama / cache lokal yang
    /// digabung dengan skema live, lalu disimpan agar pembukaan berikutnya
    /// instan. Dipakai saat tab diagram database tersebut tidak terbuka.
    fn build_schema_source(
        &mut self,
        conn_id: i64,
        db_name: &str,
        snapshot: &crate::diagram_schema::SchemaSnapshot,
        conn_name: Option<&str>,
    ) -> models::structs::DiagramState {
        use crate::diagram_schema::{merge_schema, prepare_stored_state};
        let shared = snapshot
            .shared
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(Option::as_ref)
            .map(|rec| rec.state.clone());
        let mut st = match shared {
            Some(mut shared) => {
                prepare_stored_state(&mut shared, conn_id, db_name);
                shared
            }
            None => self
                .load_prepared_diagram(conn_id, db_name)
                .unwrap_or_default(),
        };
        merge_schema(&mut st, snapshot, conn_id, db_name, conn_name);
        if !st.nodes.is_empty() {
            self.save_diagram(conn_id, db_name, &st);
        }
        st
    }

    /// Sumber untuk tab subset: bila `state` sendiri sudah subset, pakai tab
    /// diagram penuh milik (conn, db) agar tabel/relasi di luar subset ikut.
    fn subset_source<'a>(
        &'a self,
        conn_id: Option<i64>,
        db_name: Option<&str>,
        state: &'a models::structs::DiagramState,
    ) -> &'a models::structs::DiagramState {
        let host = match (state.scoped_to.is_some(), conn_id, db_name) {
            (true, Some(cid), Some(db)) => self
                .query_tabs
                .iter()
                .find(|t| Self::is_diagram_host_tab(t, cid, db))
                .and_then(|t| t.diagram_state.as_ref()),
            _ => None,
        };
        host.unwrap_or(state)
    }

    /// Buka tab diagram baru berisi `subset` (sudah ditandai `scoped_to`).
    fn open_subset_tab(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        title: String,
        subset: models::structs::DiagramState,
    ) {
        crate::editor::create_new_tab_with_connection_and_database(
            self,
            title,
            String::new(),
            conn_id,
            db_name,
        );
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            tab.diagram_state = Some(subset);
        }
        self.table_bottom_view = models::structs::TableBottomView::Query;
    }

    /// Buka tab baru berisi `table` dan tabel yang berelasi dengannya saja,
    /// sudah ditata otomatis. Bila dipanggil dari tab subset, sumbernya
    /// diambil dari tab diagram penuh agar relasi di luar subset ikut.
    fn open_focus_subset_tab(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
        table: &str,
    ) {
        let source = self.subset_source(conn_id, db_name.as_deref(), state);
        let mut subset = crate::diagram_view::focus_subset_state(source, table);
        crate::diagram_view::auto_arrange(&mut subset);
        let title = subset
            .nodes
            .iter()
            .find(|n| n.id == table)
            .map_or(table, |n| n.title.as_str())
            .to_string();
        let count = subset.nodes.len();
        self.open_subset_tab(
            conn_id,
            db_name,
            format!("Diagram: {title} + related"),
            subset,
        );
        log::info!("[DIAGRAM] opened '{title}' with {count} related table(s) in a new tab");
    }

    /// Buka tab baru berisi seluruh tabel anggota group `group_id`, sudah
    /// ditata otomatis (Auto Arrange).
    fn open_group_subset_tab(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
        group_id: &str,
    ) {
        let source = self.subset_source(conn_id, db_name.as_deref(), state);
        // Tab subset hanya membawa group-nya sendiri; bila group tidak ada
        // di tab penuh (mis. sudah dihapus), pakai state tab ini.
        let subset = crate::diagram_view::group_subset_state(source, group_id)
            .or_else(|| crate::diagram_view::group_subset_state(state, group_id));
        let Some(mut subset) = subset else {
            self.toasts.info("This group has no tables to open");
            return;
        };
        crate::diagram_view::auto_arrange(&mut subset);
        let title = subset
            .groups
            .first()
            .map_or(group_id, |g| g.title.as_str())
            .to_string();
        let count = subset.nodes.len();
        self.open_subset_tab(conn_id, db_name, format!("Diagram: {title}"), subset);
        log::info!("[DIAGRAM] opened group '{title}' with {count} table(s) in a new tab");
    }

    /// Buka tab baru berisi flow card endpoint `card_id` beserta semua tabel
    /// yang tertaut dengannya, sudah ditata otomatis (Auto Arrange).
    fn open_flow_subset_tab(
        &mut self,
        conn_id: Option<i64>,
        db_name: Option<String>,
        state: &models::structs::DiagramState,
        card_id: &str,
    ) {
        let source = self.subset_source(conn_id, db_name.as_deref(), state);
        let subset = crate::diagram_view::flow_subset_state(source, card_id)
            .or_else(|| crate::diagram_view::flow_subset_state(state, card_id));
        let Some(mut subset) = subset else {
            self.toasts
                .info("This endpoint is not linked to any table in the diagram");
            return;
        };
        crate::diagram_view::auto_arrange(&mut subset);
        let title = subset.flow_cards.first().map_or_else(
            || card_id.to_string(),
            |c| format!("{} {}", c.trigger.method, c.trigger.target),
        );
        let count = subset.nodes.len();
        self.open_subset_tab(conn_id, db_name, format!("Diagram: {title}"), subset);
        log::info!("[DIAGRAM] opened endpoint '{title}' with {count} linked table(s) in a new tab");
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

    /// Tab diagram penuh milik (conn, db). Tab subset (`scoped_to`) tidak
    /// termasuk: tidak menerima sinkron skema dan bukan sumber link.
    pub(crate) fn is_diagram_host_tab(
        tab: &models::structs::QueryTab,
        conn_id: i64,
        db_name: &str,
    ) -> bool {
        tab.diagram_state
            .as_ref()
            .is_some_and(|s| s.scoped_to.is_none())
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
        use crate::diagram_schema::merge_schema;

        let conn_name = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.name.clone());
        let host_tabs: Vec<usize> = (0..self.query_tabs.len())
            .filter(|&i| Self::is_diagram_host_tab(&self.query_tabs[i], conn_id, db_name))
            .collect();

        let mut source: Option<models::structs::DiagramState> = None;
        let mut messages: Vec<String> = Vec::new();
        for i in host_tabs {
            let Some(mut state) = self.query_tabs[i].diagram_state.take() else {
                continue;
            };
            // Bandingkan salinan lokal dengan versi di `diagram_by_tabular`.
            let mut replaced = false;
            if state.pending_merge.is_none() {
                match &snapshot.shared {
                    Some(Ok(Some(rec))) => {
                        if let crate::window_egui::diagram_db_sync::Reconciled::Replaced(msg) =
                            self.reconcile_diagram(&mut state, conn_id, db_name, rec.clone())
                        {
                            replaced = true;
                            messages.push(msg);
                        }
                    }
                    Some(Ok(None)) => {
                        state.db_status = models::structs::DiagramDbStatus::NotInDatabase;
                    }
                    Some(Err(e)) => {
                        log::warn!("[DIAGRAM_DB] reading diagram of '{db_name}' failed: {e}");
                        state.db_status = models::structs::DiagramDbStatus::LocalOnly(e.clone());
                    }
                    None => {}
                }
            }
            merge_schema(&mut state, snapshot, conn_id, db_name, conn_name.as_deref());
            if replaced {
                // Isi link ikut terbuang saat state diganti.
                self.materialize_links(&mut state, None);
            }
            state.schema_syncing = false;
            self.save_diagram(conn_id, db_name, &state);
            source.get_or_insert_with(|| crate::diagram_links::persistable(&state));
            self.query_tabs[i].diagram_state = Some(state);
        }
        for msg in messages {
            self.toasts.info(msg);
        }

        self.resolve_focus_requests(conn_id, db_name, snapshot, conn_name.as_deref());

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
            // Tab database sumber tidak terbuka: bangun dari layout bersama /
            // cache lokal.
            None => self.build_schema_source(conn_id, db_name, snapshot, conn_name.as_deref()),
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
        self.fail_focus_requests(conn_id, db_name, &error);
        let mut was_syncing = false;
        for tab in &mut self.query_tabs {
            if Self::is_diagram_host_tab(tab, conn_id, db_name)
                && let Some(st) = tab.diagram_state.as_mut()
            {
                was_syncing |= st.schema_syncing;
                st.schema_syncing = false;
                // Database tidak terjangkau: versi di `diagram_by_tabular`
                // belum bisa dibandingkan.
                if matches!(
                    st.db_status,
                    models::structs::DiagramDbStatus::Checking
                        | models::structs::DiagramDbStatus::Unknown
                ) {
                    st.db_status = models::structs::DiagramDbStatus::LocalOnly(error.clone());
                }
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
        let existing = self
            .query_tabs
            .iter()
            .position(|t| Self::is_diagram_host_tab(t, cid, &link.database_name));
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
                        c.connection_type.display_name(),
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
        st.force_save = true;
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
}

/// Perbarui langkah dengan nomor yang sama (Active → Done/Error) atau tambahkan
/// langkah baru di akhir.
pub(crate) fn upsert_progress(
    steps: &mut Vec<crate::agent::harness::ProgressStep>,
    step: crate::agent::harness::ProgressStep,
) {
    if let Some(idx) = step.step_index
        && let Some(existing) = steps
            .iter_mut()
            .rev()
            .find(|s| s.step_index == Some(idx) && s.tool_name == step.tool_name)
    {
        *existing = step;
        return;
    }
    steps.push(step);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn async_save_keeps_latest_layout() {
        let dir = std::env::temp_dir().join(format!(
            "tabular_diagram_save_{}_{}",
            std::process::id(),
            SAVE_SEQ.load(std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("layout.json");

        let mut old = crate::diagram_lod::synthetic_state(200, 4, 400, 100);
        old.diagram_title = Some("old".into());
        let new = models::structs::DiagramState {
            diagram_title: Some("new".into()),
            ..Default::default()
        };

        let h1 = save_diagram_file_async(path.clone(), old);
        let h2 = save_diagram_file_async(path.clone(), new);
        h1.join().expect("first save thread");
        h2.join().expect("second save thread");

        let saved: models::structs::DiagramState =
            serde_json::from_slice(&std::fs::read(&path).expect("read saved diagram"))
                .expect("parse saved diagram");
        assert_eq!(saved.diagram_title.as_deref(), Some("new"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
