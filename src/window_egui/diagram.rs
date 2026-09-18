use crate::models;
use eframe::egui;

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
            let mut open = true;
            let mut confirmed = false;
            let mut should_close = false;

            egui::Window::new("Metadata Missing")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Metadata for table '{}' is not in cache.",
                        table_name
                    ));
                    ui.label("Would you like to fetch it now?");
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Fetch Metadata").clicked() {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            should_close = true;
                        }
                    });
                });

            if should_close {
                open = false;
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
            } else if !open {
                self.cache_miss_request = None;
            }
        }
    }
    pub fn save_diagram(&self, conn_id: i64, db_name: &str, state: &models::structs::DiagramState) {
        let Some(path) = self.get_diagram_path(conn_id, db_name) else {
            return;
        };
        // Tulis atomik supaya layout lama tidak rusak bila app crash saat menyimpan.
        let result = serde_json::to_vec_pretty(state)
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
            DiagramAction::OpenAddTablesModal => {
                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                    if let Some(st) = &mut tab.diagram_state {
                        st.show_add_tables_modal = true;
                    }
                }
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
        self.save_diagram(cid, &db, state);

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
        self.save_diagram(cid, &db, state);

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

        let state_clone = state.clone();
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
                    current_state.groups = loaded_state.groups;
                    current_state.virtual_relations = loaded_state.virtual_relations;
                    current_state.pan = loaded_state.pan;
                    current_state.zoom = loaded_state.zoom;
                    current_state.show_grid = loaded_state.show_grid;

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

        let model = crate::diagram_mermaid::ErModel::from_diagram(state);
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

    pub fn get_tables_for_dialog(&mut self, conn_id: i64, db_name: &str) -> Vec<String> {
        let mut tables = Vec::new();

        // 1. Coba ambil dari local SQLite cache `table_cache` jika ada
        if let Some(pool) = &self.db_pool {
            let res: Result<Vec<(String,)>, _> = futures::executor::block_on(async {
                sqlx::query_as(
                    "SELECT DISTINCT table_name FROM table_cache WHERE connection_id = ? AND database_name = ? AND table_type IN ('table', 'view', 'BASE TABLE') ORDER BY table_name"
                )
                .bind(conn_id)
                .bind(db_name)
                .fetch_all(pool.as_ref())
                .await
            });
            if let Ok(rows) = res {
                tables = rows.into_iter().map(|(r,)| r).collect();
            }
        }

        // 2. Jika cache kosong, fetch langsung dari driver database
        if tables.is_empty() {
            let db_type = self
                .connections
                .iter()
                .find(|c| c.id == Some(conn_id))
                .map(|c| c.connection_type.clone());

            match db_type {
                Some(models::enums::DatabaseType::MySQL) => {
                    if let Some(t) = crate::driver_mysql::fetch_tables_from_mysql_connection(
                        self, conn_id, db_name, "table",
                    ) {
                        tables = t;
                    }
                }
                Some(models::enums::DatabaseType::PostgreSQL) => {
                    if let Some(t) = crate::driver_postgres::fetch_tables_from_postgres_connection(
                        self, conn_id, db_name, "BASE TABLE",
                    ) {
                        tables = t;
                    }
                }
                Some(models::enums::DatabaseType::SQLite) => {
                    if let Some(t) = crate::driver_sqlite::fetch_tables_from_sqlite_connection(
                        self, conn_id, "table",
                    ) {
                        tables = t;
                    }
                }
                Some(models::enums::DatabaseType::MsSQL) => {
                    if let Some(t) = crate::driver_mssql::fetch_tables_from_mssql_connection(
                        self, conn_id, db_name, "table",
                    ) {
                        tables = t;
                    }
                }
                _ => {}
            }
        }

        tables
    }

    pub fn render_add_tables_dialog(&mut self, ctx: &egui::Context) {
        let is_modal_active = self
            .query_tabs
            .get(self.active_tab_index)
            .and_then(|t| t.diagram_state.as_ref())
            .map(|s| s.show_add_tables_modal)
            .unwrap_or(false);
        if !is_modal_active {
            return;
        }

        // 1. Inisialisasi default connection & database bila belum diset
        if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
            if let Some(st) = &mut tab.diagram_state {
                if st.add_tables_selected_conn.is_none() {
                    if let Some(first_conn) = self.connections.first() {
                        st.add_tables_selected_conn = first_conn.id;
                        st.add_tables_selected_db = Some(first_conn.database.clone());
                    }
                }
            }
        }

        // 2. Ambil parameter terpilih untuk cek apakah tabel perlu dimuat
        let (selected_conn_id, selected_db, needs_fetch) = {
            if let Some(tab) = self.query_tabs.get(self.active_tab_index) {
                if let Some(st) = &tab.diagram_state {
                    (
                        st.add_tables_selected_conn,
                        st.add_tables_selected_db.clone().unwrap_or_default(),
                        st.add_tables_selection.is_empty(),
                    )
                } else {
                    return;
                }
            } else {
                return;
            }
        };

        // 3. Muat daftar tabel jika belum ada (self dipinjam &mut secara eksklusif)
        if needs_fetch {
            if let Some(cid) = selected_conn_id {
                let fetched_tables = self.get_tables_for_dialog(cid, &selected_db);
                if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                    if let Some(st) = &mut tab.diagram_state {
                        let existing: std::collections::HashSet<String> = st
                            .nodes
                            .iter()
                            .filter(|n| n.database_name.as_deref() == Some(&selected_db))
                            .map(|n| n.title.clone())
                            .collect();
                        st.add_tables_selection = fetched_tables
                            .into_iter()
                            .filter(|t| !existing.contains(t))
                            .map(|t| (t, false))
                            .collect();
                    }
                }
            }
        }

        // 4. Salin opsi koneksi agar tidak meminjam self.connections saat render modal
        let conn_options: Vec<(i64, String, String)> = self
            .connections
            .iter()
            .filter_map(|c| c.id.map(|id| (id, c.name.clone(), c.database.clone())))
            .collect();

        let mut is_open = true;
        let mut user_cancelled = false;
        let mut do_add: Option<(i64, String, Vec<String>)> = None;
        let mut conn_changed_to: Option<(i64, String)> = None;
        let mut db_name_changed = false;

        egui::Window::new("Add Tables from Database")
            .open(&mut is_open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(480.0, 520.0))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) else {
                    return;
                };
                let Some(diagram_state) = &mut tab.diagram_state else {
                    return;
                };

                ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);

                ui.label(
                    egui::RichText::new("Select a connection and database to import tables into the current diagram:")
                        .weak()
                        .size(12.0),
                );

                ui.add_space(4.0);

                // 1. Connection Selector
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Connection:").strong().size(12.5));
                    let curr_conn_id = diagram_state.add_tables_selected_conn;
                    let curr_name = curr_conn_id
                        .and_then(|cid| conn_options.iter().find(|(id, _, _)| *id == cid))
                        .map(|(_, name, _)| name.clone())
                        .unwrap_or_else(|| "Select connection".to_string());

                    egui::ComboBox::from_id_salt("add_tables_conn_combo")
                        .selected_text(curr_name)
                        .show_ui(ui, |ui| {
                            for (cid, cname, cdb) in &conn_options {
                                let is_selected = Some(*cid) == curr_conn_id;
                                if ui.selectable_label(is_selected, cname).clicked() && !is_selected {
                                    conn_changed_to = Some((*cid, cdb.clone()));
                                }
                            }
                        });
                });

                if let Some((cid, cdb)) = conn_changed_to {
                    diagram_state.add_tables_selected_conn = Some(cid);
                    diagram_state.add_tables_selected_db = Some(cdb);
                    diagram_state.add_tables_selection.clear();
                }

                // 2. Database Name Selector / Input
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Database:").strong().size(12.5));
                    let mut db_val = diagram_state.add_tables_selected_db.clone().unwrap_or_default();
                    let resp = ui.add(egui::TextEdit::singleline(&mut db_val).hint_text("database name"));
                    if resp.changed() {
                        diagram_state.add_tables_selected_db = Some(db_val);
                        db_name_changed = true;
                    }
                });

                if db_name_changed {
                    diagram_state.add_tables_selection.clear();
                }

                ui.separator();

                // 3. Search & Batch Selection
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Search Tables:").size(12.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut diagram_state.add_tables_search)
                            .hint_text("Filter tables…")
                            .desired_width(180.0),
                    );

                    if ui.button("Select All").clicked() {
                        let filter = diagram_state.add_tables_search.to_lowercase();
                        for (tbl, sel) in &mut diagram_state.add_tables_selection {
                            if filter.is_empty() || tbl.to_lowercase().contains(&filter) {
                                *sel = true;
                            }
                        }
                    }
                    if ui.button("Deselect All").clicked() {
                        for (_, sel) in &mut diagram_state.add_tables_selection {
                            *sel = false;
                        }
                    }
                });

                let filter = diagram_state.add_tables_search.to_lowercase();
                let selected_count = diagram_state.add_tables_selection.iter().filter(|(_, s)| *s).count();

                // 4. Scrollable Tables List
                ui.group(|ui| {
                    egui::ScrollArea::vertical()
                        .max_height(260.0)
                        .show(ui, |ui| {
                            if diagram_state.add_tables_selection.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new("No tables found or all tables already added.").weak());
                                    ui.add_space(20.0);
                                });
                            } else {
                                for (tbl, is_checked) in &mut diagram_state.add_tables_selection {
                                    if !filter.is_empty() && !tbl.to_lowercase().contains(&filter) {
                                        continue;
                                    }
                                    ui.checkbox(is_checked, tbl.as_str());
                                }
                            }
                        });
                });

                ui.add_space(8.0);

                // 5. Actions Footer
                ui.horizontal(|ui| {
                    let can_add = selected_count > 0 && diagram_state.add_tables_selected_conn.is_some();
                    let btn = egui::Button::new(
                        egui::RichText::new(format!("Add {} Selected Table(s)", selected_count))
                            .strong(),
                    );
                    if ui.add_enabled(can_add, btn).clicked() {
                        if let Some(cid) = diagram_state.add_tables_selected_conn {
                            let chosen: Vec<String> = diagram_state
                                .add_tables_selection
                                .iter()
                                .filter(|(_, s)| *s)
                                .map(|(t, _)| t.clone())
                                .collect();
                            let db = diagram_state.add_tables_selected_db.clone().unwrap_or_default();
                            do_add = Some((cid, db, chosen));
                        }
                    }

                    if ui.button("Cancel").clicked() {
                        user_cancelled = true;
                    }
                });
            });

        if !is_open || user_cancelled {
            if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                if let Some(st) = &mut tab.diagram_state {
                    st.show_add_tables_modal = false;
                    st.add_tables_selection.clear();
                }
            }
        }

        if let Some((cid, db, chosen_tables)) = do_add {
            self.add_tables_to_active_diagram(cid, db, chosen_tables);
            if let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) {
                if let Some(st) = &mut tab.diagram_state {
                    st.show_add_tables_modal = false;
                    st.add_tables_selection.clear();
                }
            }
        }
    }

    pub fn add_tables_to_active_diagram(
        &mut self,
        conn_id: i64,
        db_name: String,
        table_names: Vec<String>,
    ) {
        if table_names.is_empty() {
            return;
        }

        let conn_name = self
            .connections
            .iter()
            .find(|c| c.id == Some(conn_id))
            .map(|c| c.name.clone());

        let mut fks = Vec::new();
        let mut columns_map = std::collections::HashMap::new();

        if let Some(rt) = self.runtime.clone() {
            rt.block_on(async {
                let _ = crate::connection::pool_if_connected_or_start(self, conn_id).await;
                fks = crate::connection::get_foreign_keys(self, conn_id, &db_name).await;

                if let Some(pool_enum) = self.connection_pools.get(&conn_id).cloned() {
                    match pool_enum {
                        models::enums::DatabasePool::MySQL(p) => {
                            if let Ok(cols) =
                                crate::driver_mysql::fetch_mysql_columns(&p, &db_name).await
                            {
                                columns_map = cols;
                            }
                        }
                        models::enums::DatabasePool::PostgreSQL(p) => {
                            if let Ok(cols) =
                                crate::driver_postgres::fetch_postgres_columns(&p).await
                            {
                                columns_map = cols;
                            }
                        }
                        models::enums::DatabasePool::SQLite(p) => {
                            if let Ok(cols) =
                                crate::driver_sqlite::fetch_sqlite_columns(&p).await
                            {
                                columns_map = cols;
                            }
                        }
                        _ => {}
                    }
                }
            });
        }

        let Some(tab) = self.query_tabs.get_mut(self.active_tab_index) else {
            return;
        };
        let Some(diagram_state) = &mut tab.diagram_state else {
            return;
        };

        let group_id = format!("group_{}", db_name.replace(' ', "_"));
        let group_title = format!("DB: {}", db_name);
        let group_exists = diagram_state
            .groups
            .iter()
            .any(|g| g.id == group_id || g.title == group_title);
        if !group_exists {
            let color_idx = diagram_state.groups.len() % crate::diagram_view::GROUP_COLORS.len();
            diagram_state.groups.push(models::structs::DiagramGroup {
                id: group_id.clone(),
                title: group_title,
                color: crate::diagram_view::GROUP_COLORS[color_idx],
                manual_pos: None,
            });
        }

        let max_x = diagram_state
            .nodes
            .iter()
            .map(|n| n.pos.x + n.size.x)
            .fold(0.0f32, |acc, x| acc.max(x));
        let start_x = if max_x > 0.0 { max_x + 80.0 } else { 100.0 };
        let mut curr_y = 100.0f32;

        let mut added_count = 0;
        for table in table_names {
            if diagram_state
                .nodes
                .iter()
                .any(|n| n.title == table && n.database_name.as_deref() == Some(&db_name))
            {
                continue;
            }

            let unique_id = if diagram_state.nodes.iter().any(|n| n.id == table) {
                format!("{}::{}", db_name, table)
            } else {
                table.clone()
            };

            let cols = columns_map.get(&table).cloned().unwrap_or_default();
            let col_names: Vec<String> = cols.iter().map(|c| c.name.clone()).collect();
            let table_fks: Vec<models::structs::ForeignKey> = fks
                .iter()
                .filter(|fk| fk.table_name == table)
                .cloned()
                .collect();

            let mut node = models::structs::DiagramNode {
                id: unique_id,
                title: table,
                pos: eframe::egui::pos2(start_x, curr_y),
                size: eframe::egui::vec2(220.0, 160.0),
                columns: col_names,
                foreign_keys: table_fks,
                group_ids: vec![group_id.clone()],
                group_id: Some(group_id.clone()),
                column_meta: cols,
                detached: false,
                database_name: Some(db_name.clone()),
                connection_id: Some(conn_id),
                connection_name: conn_name.clone(),
            };
            node.ensure_groups_migrated();

            curr_y += 240.0;
            diagram_state.nodes.push(node);
            added_count += 1;
        }

        for fk in &fks {
            let has_source = diagram_state.nodes.iter().any(|n| n.title == fk.table_name);
            let has_target = diagram_state.nodes.iter().any(|n| n.title == fk.referenced_table_name);
            if has_source && has_target {
                let already_edge = diagram_state.edges.iter().any(|e| {
                    (e.source == fk.table_name && e.target == fk.referenced_table_name)
                        || (e.source.ends_with(&format!("::{}", fk.table_name))
                            && e.target.ends_with(&format!("::{}", fk.referenced_table_name)))
                });
                if !already_edge {
                    diagram_state.edges.push(models::structs::DiagramEdge {
                        source: fk.table_name.clone(),
                        target: fk.referenced_table_name.clone(),
                        label: format!("{} -> {}", fk.column_name, fk.referenced_column_name),
                    });
                }
            }
        }

        if diagram_state.prevent_overlap {
            crate::diagram_view::resolve_node_overlaps(&mut diagram_state.nodes, 20.0);
        }

        diagram_state.save_requested = true;
        self.toasts.success(format!(
            "Added {} table(s) from database '{}' to diagram",
            added_count, db_name
        ));
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
            state.clone(),
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
