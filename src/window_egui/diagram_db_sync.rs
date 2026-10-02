//! Sinkronisasi tab diagram dengan tabel `diagram_by_tabular` di database
//! target, yang menjadi penyimpanan utama diagram:
//!
//! - Save / autosave mengirim diagram di background dengan compare-and-swap
//!   (autosave menunggu [`DB_SAVE_DEBOUNCE`] tanpa perubahan).
//! - Saat diagram dibuka, salinan lokal dibandingkan dengan versi database
//!   (lihat [`crate::diagram_sync`]): perubahan satu sisi digabung otomatis,
//!   konflik ditampilkan di popup merge.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::diagram_storage::{DiagramRecord, DiagramStoreError};
use crate::diagram_sync::{self, Side, SyncBase, SyncPlan};
use crate::models::structs::{DiagramDbStatus, DiagramPendingMerge, DiagramState};

/// Jeda tanpa perubahan sebelum autosave mengirim diagram ke database.
pub const DB_SAVE_DEBOUNCE: Duration = Duration::from_secs(3);

/// Penyimpanan satu diagram ke database yang sedang berjalan.
pub struct DiagramDbSaveJob {
    conn_id: i64,
    db_name: String,
    /// State yang dikirim; menjadi base bila penyimpanan sukses.
    state: DiagramState,
    updated_by: String,
    /// Save manual: tampilkan toast hasilnya.
    announce: bool,
    rx: mpsc::Receiver<Result<i64, DiagramStoreError>>,
}

/// Hasil rekonsiliasi salinan lokal dengan versi database.
pub(crate) enum Reconciled {
    /// State lokal dipertahankan (mungkin ditandai perlu dikirim atau
    /// menunggu merge).
    Kept,
    /// State diganti hasil merge; isi link perlu dimaterialisasi ulang.
    Replaced(String),
}

fn synced(rec: &DiagramRecord) -> DiagramDbStatus {
    DiagramDbStatus::Synced {
        revision: rec.revision,
        updated_by: rec.updated_by.clone(),
        updated_at: rec.updated_at.clone(),
    }
}

/// Tandai state perlu segera dikirim ke database.
fn push_soon(state: &mut DiagramState) {
    state.db_dirty_since = Instant::now().checked_sub(DB_SAVE_DEBOUNCE);
}

/// Ganti isi `state` dengan hasil merge, pertahankan data skema live dan
/// status runtime tab.
fn adopt_merged(state: &mut DiagramState, merged: DiagramState, conn_id: i64, db_name: &str) {
    let old = std::mem::replace(state, merged);
    crate::diagram_schema::prepare_stored_state(state, conn_id, db_name);
    diagram_sync::adopt_schema_from(state, &old);
    state.schema_syncing = old.schema_syncing;
    state.db_dirty_since = old.db_dirty_since;
}

impl super::Tabular {
    fn diagram_base_path(&self, conn_id: i64, db_name: &str) -> Option<std::path::PathBuf> {
        let dir = self
            .get_diagram_path(conn_id, db_name)?
            .parent()?
            .to_path_buf();
        Some(dir.join(crate::diagram_storage::base_diagram_file_name(
            conn_id, db_name,
        )))
    }

    pub(crate) fn read_diagram_base(&self, conn_id: i64, db_name: &str) -> Option<SyncBase> {
        diagram_sync::read_base(&self.diagram_base_path(conn_id, db_name)?)
    }

    fn write_diagram_base(&self, conn_id: i64, db_name: &str, revision: i64, state: &DiagramState) {
        if let Some(path) = self.diagram_base_path(conn_id, db_name) {
            diagram_sync::write_base(
                &path,
                &SyncBase {
                    revision,
                    state: state.clone(),
                },
            );
        }
    }

    /// Nama yang dicatat di kolom `updated_by`: user OS, lalu user koneksi.
    fn diagram_db_user(&self, conn_id: i64) -> String {
        std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .ok()
            .filter(|u| !u.is_empty())
            .or_else(|| {
                self.connections
                    .iter()
                    .find(|c| c.id == Some(conn_id))
                    .map(|c| c.username.clone())
                    .filter(|u| !u.is_empty())
            })
            .unwrap_or_else(|| "tabular".to_string())
    }

    fn diagram_db_job_running(&self, conn_id: i64, db_name: &str) -> bool {
        self.diagram_db_save_jobs
            .iter()
            .any(|j| j.conn_id == conn_id && j.db_name == db_name)
    }

    /// Kirim diagram (conn, db) yang sedang terbuka ke database sekarang.
    /// `announce` = Save manual (toast hasil, dicoba lagi meski sebelumnya gagal).
    pub fn start_diagram_db_save(&mut self, conn_id: i64, db_name: &str, announce: bool) {
        let running = self.diagram_db_job_running(conn_id, db_name);
        let Some(st) = self.diagram_state_for_mut(Some(conn_id), Some(db_name)) else {
            return;
        };
        if let Some(pending) = st.pending_merge.as_mut() {
            if announce {
                pending.visible = true;
                self.toasts
                    .info("Resolve the differences with the database version first");
            }
            return;
        }
        if running {
            // Kirim lagi begitu penyimpanan yang berjalan selesai.
            push_soon(st);
            return;
        }
        st.db_dirty_since = None;
        st.db_status = DiagramDbStatus::Saving;
        let state = crate::diagram_links::persistable(st);

        let expected = self.read_diagram_base(conn_id, db_name).map(|b| b.revision);
        let updated_by = self.diagram_db_user(conn_id);
        let Some(rt) = self.runtime.clone() else {
            self.finish_diagram_db_save(
                conn_id,
                db_name,
                state,
                &updated_by,
                announce,
                Err(DiagramStoreError::Db(
                    "Tokio runtime unavailable".to_string(),
                )),
            );
            return;
        };
        // Tidak pernah dial di UI thread: pool yang belum siap hanya dipicu
        // pembuatannya lalu ditunggu di task background.
        let pool = rt.block_on(crate::connection::pool_if_connected_or_start(self, conn_id));
        let shared = self.shared_connection_pools.clone();
        let (tx, rx) = mpsc::channel();
        let (db, to_save, who) = (db_name.to_string(), state.clone(), updated_by.clone());
        rt.spawn(async move {
            let pool = match pool {
                Some(p) => Some(p),
                None => crate::diagram_schema::wait_for_pool(conn_id, &shared).await,
            };
            let result = match pool {
                None => Err(DiagramStoreError::Db(
                    "database connection is not ready".to_string(),
                )),
                Some(pool) => tokio::time::timeout(
                    crate::diagram_storage::DB_TIMEOUT,
                    crate::diagram_storage::save_diagram_record(
                        &pool, &db, &to_save, expected, &who,
                    ),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(DiagramStoreError::Db(
                        "saving to the database timed out".to_string(),
                    ))
                }),
            };
            let _ = tx.send(result);
        });
        self.diagram_db_save_jobs.push(DiagramDbSaveJob {
            conn_id,
            db_name: db_name.to_string(),
            state,
            updated_by,
            announce,
            rx,
        });
    }

    fn finish_diagram_db_save(
        &mut self,
        conn_id: i64,
        db_name: &str,
        state: DiagramState,
        updated_by: &str,
        announce: bool,
        result: Result<i64, DiagramStoreError>,
    ) {
        match result {
            Ok(revision) => {
                self.write_diagram_base(conn_id, db_name, revision, &state);
                log::info!("[DIAGRAM_DB] '{db_name}' saved at revision {revision}");
                if let Some(st) = self.diagram_state_for_mut(Some(conn_id), Some(db_name)) {
                    st.db_status = DiagramDbStatus::Synced {
                        revision,
                        updated_by: Some(updated_by.to_string()),
                        updated_at: Some(
                            chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                        ),
                    };
                }
                if announce {
                    self.toasts.success(format!(
                        "Diagram saved to `diagram_by_tabular` in {db_name}"
                    ));
                }
            }
            Err(DiagramStoreError::Conflict(remote)) => {
                log::info!(
                    "[DIAGRAM_DB] '{db_name}' changed in the database (revision {}), reconciling",
                    remote.revision
                );
                self.reconcile_open_diagram(conn_id, db_name, *remote);
            }
            Err(e) => {
                log::warn!("[DIAGRAM_DB] saving '{db_name}' failed: {e}");
                let msg = e.to_string();
                let was_failing = self
                    .diagram_state_for_mut(Some(conn_id), Some(db_name))
                    .map(|st| {
                        let was = matches!(st.db_status, DiagramDbStatus::LocalOnly(_));
                        st.db_status = DiagramDbStatus::LocalOnly(msg.clone());
                        // Perubahan tetap harus dikirim saat database kembali.
                        st.db_dirty_since.get_or_insert_with(Instant::now);
                        was
                    })
                    .unwrap_or(false);
                if announce || !was_failing {
                    self.toasts.warning(format!(
                        "Diagram saved locally only. Database save failed: {msg}"
                    ));
                }
            }
        }
    }

    /// Terima hasil penyimpanan dan jalankan autosave ke database yang sudah
    /// melewati jeda. Dipanggil tiap frame.
    pub fn poll_diagram_db_jobs(&mut self, ctx: &egui::Context) {
        let jobs = std::mem::take(&mut self.diagram_db_save_jobs);
        let mut keep = Vec::with_capacity(jobs.len());
        let mut finished = Vec::new();
        for job in jobs {
            match job.rx.try_recv() {
                Ok(result) => finished.push((job, result)),
                Err(mpsc::TryRecvError::Empty) => keep.push(job),
                Err(mpsc::TryRecvError::Disconnected) => finished.push((
                    job,
                    Err(DiagramStoreError::Db(
                        "save task stopped unexpectedly".to_string(),
                    )),
                )),
            }
        }
        self.diagram_db_save_jobs = keep;
        for (job, result) in finished {
            self.finish_diagram_db_save(
                job.conn_id,
                &job.db_name,
                job.state,
                &job.updated_by,
                job.announce,
                result,
            );
        }

        // Autosave ke database. Diagram yang gagal disimpan (LocalOnly) atau
        // menunggu merge tidak dicoba otomatis; Save manual mencoba lagi.
        let mut waiting = false;
        let due: Vec<(i64, String)> = self
            .query_tabs
            .iter()
            .filter_map(|t| {
                let st = t.diagram_state.as_ref()?;
                let since = st.db_dirty_since?;
                if st.scoped_to.is_some()
                    || st.pending_merge.is_some()
                    || matches!(st.db_status, DiagramDbStatus::LocalOnly(_))
                {
                    return None;
                }
                if since.elapsed() < DB_SAVE_DEBOUNCE {
                    waiting = true;
                    return None;
                }
                Some((t.connection_id?, t.database_name.clone()?))
            })
            .collect();
        for (conn_id, db_name) in due {
            if !self.diagram_db_job_running(conn_id, &db_name) {
                self.start_diagram_db_save(conn_id, &db_name, false);
            }
        }
        if waiting || !self.diagram_db_save_jobs.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Bandingkan `state` (salinan lokal) dengan versi database `rec`.
    /// Perubahan satu sisi digabung otomatis; konflik disimpan sebagai
    /// `pending_merge` untuk popup merge.
    pub(crate) fn reconcile_diagram(
        &self,
        state: &mut DiagramState,
        conn_id: i64,
        db_name: &str,
        rec: DiagramRecord,
    ) -> Reconciled {
        let base = self.read_diagram_base(conn_id, db_name);
        let base_state = base.as_ref().map(|b| &b.state);
        let local = crate::diagram_links::persistable(state);
        let plan = diagram_sync::classify(&local, base_state, &rec.state);
        log::info!(
            "[DIAGRAM_SYNC] '{db_name}': {plan:?} (base rev {:?}, database rev {})",
            base.as_ref().map(|b| b.revision),
            rec.revision
        );
        match plan {
            SyncPlan::UpToDate => {
                self.write_diagram_base(conn_id, db_name, rec.revision, &rec.state);
                state.db_status = synced(&rec);
                state.db_dirty_since = None;
                Reconciled::Kept
            }
            SyncPlan::PushLocal => {
                // Isi database sama dengan base: cukup perbarui revision base
                // lalu kirim perubahan lokal.
                self.write_diagram_base(conn_id, db_name, rec.revision, &rec.state);
                state.db_status = synced(&rec);
                push_soon(state);
                Reconciled::Kept
            }
            SyncPlan::TakeRemote | SyncPlan::Merge => {
                let result = diagram_sync::three_way_merge(base_state, &local, &rec.state);
                if !result.conflicts.is_empty() {
                    log::info!(
                        "[DIAGRAM_SYNC] '{db_name}': {} conflict(s), asking the user",
                        result.conflicts.len()
                    );
                    state.db_status = DiagramDbStatus::MergePending;
                    state.pending_merge = Some(Box::new(DiagramPendingMerge {
                        result,
                        remote: rec,
                        visible: true,
                    }));
                    return Reconciled::Kept;
                }
                match diagram_sync::resolve(&result) {
                    Ok(merged) => {
                        let push = !diagram_sync::same_content(&merged, &rec.state);
                        adopt_merged(state, merged, conn_id, db_name);
                        self.write_diagram_base(conn_id, db_name, rec.revision, &rec.state);
                        state.db_status = synced(&rec);
                        state.pending_merge = None;
                        if push {
                            push_soon(state);
                        } else {
                            state.db_dirty_since = None;
                        }
                        let who = rec
                            .updated_by
                            .as_deref()
                            .map(|u| format!(" by {u}"))
                            .unwrap_or_default();
                        Reconciled::Replaced(if plan == SyncPlan::TakeRemote {
                            format!("Diagram updated from the database (changed{who})")
                        } else {
                            format!(
                                "Merged {} change(s) from the database{who} with your local changes",
                                result.auto_merged
                            )
                        })
                    }
                    Err(e) => {
                        log::error!("[DIAGRAM_SYNC] merge of '{db_name}' failed: {e}");
                        state.db_status = DiagramDbStatus::LocalOnly(e);
                        Reconciled::Kept
                    }
                }
            }
        }
    }

    fn host_diagram_tab(&self, conn_id: i64, db_name: &str) -> Option<usize> {
        self.query_tabs
            .iter()
            .position(|t| Self::is_diagram_host_tab(t, conn_id, db_name))
    }

    /// Rekonsiliasi tab diagram (conn, db) yang terbuka dengan `rec`.
    fn reconcile_open_diagram(&mut self, conn_id: i64, db_name: &str, rec: DiagramRecord) {
        let Some(idx) = self.host_diagram_tab(conn_id, db_name) else {
            return;
        };
        let Some(mut st) = self.query_tabs[idx].diagram_state.take() else {
            return;
        };
        let outcome = self.reconcile_diagram(&mut st, conn_id, db_name, rec);
        if let Reconciled::Replaced(_) = outcome {
            self.materialize_links(&mut st, None);
        }
        let snapshot = st.clone();
        self.query_tabs[idx].diagram_state = Some(snapshot.clone());
        if let Reconciled::Replaced(msg) = outcome {
            self.save_diagram_and_propagate(conn_id, db_name, &snapshot);
            self.toasts.info(msg);
        }
    }

    /// Bandingkan ulang diagram (conn, db) dengan database: skema live dan
    /// diagram tersimpan diambil lagi, hasilnya diproses seperti saat dibuka.
    pub fn sync_diagram_with_database(&mut self, conn_id: i64, db_name: &str) {
        if let Some(st) = self.diagram_state_for_mut(Some(conn_id), Some(db_name)) {
            if st.pending_merge.is_some() {
                if let Some(p) = st.pending_merge.as_mut() {
                    p.visible = true;
                }
                return;
            }
            st.db_status = DiagramDbStatus::Checking;
            st.schema_syncing = true;
        }
        self.request_diagram_schema(conn_id, db_name);
    }

    /// Terapkan keputusan popup merge pada tab `tab_idx`. `all` memaksa
    /// semua konflik ke satu sisi; `None` = pilihan per item.
    pub(crate) fn apply_diagram_merge(&mut self, tab_idx: usize, all: Option<Side>) {
        let Some(tab) = self.query_tabs.get_mut(tab_idx) else {
            return;
        };
        let (Some(conn_id), Some(db_name)) = (tab.connection_id, tab.database_name.clone()) else {
            return;
        };
        let Some(mut st) = tab.diagram_state.take() else {
            return;
        };
        let Some(pending) = st.pending_merge.take() else {
            self.query_tabs[tab_idx].diagram_state = Some(st);
            return;
        };

        // Hitung ulang terhadap state lokal terkini supaya edit yang dibuat
        // setelah popup ditutup ikut, lalu pakai pilihan user yang ada.
        let base = self.read_diagram_base(conn_id, &db_name);
        let local = crate::diagram_links::persistable(&st);
        let mut result = diagram_sync::three_way_merge(
            base.as_ref().map(|b| &b.state),
            &local,
            &pending.remote.state,
        );
        for c in &mut result.conflicts {
            c.choice = all.unwrap_or_else(|| {
                pending
                    .result
                    .conflicts
                    .iter()
                    .find(|p| p.field == c.field && p.key == c.key)
                    .map_or(Side::Local, |p| p.choice)
            });
        }

        match diagram_sync::resolve(&result) {
            Ok(merged) => {
                let rec = &pending.remote;
                let push = !diagram_sync::same_content(&merged, &rec.state);
                adopt_merged(&mut st, merged, conn_id, &db_name);
                self.write_diagram_base(conn_id, &db_name, rec.revision, &rec.state);
                st.db_status = synced(rec);
                if push {
                    push_soon(&mut st);
                } else {
                    st.db_dirty_since = None;
                }
                self.materialize_links(&mut st, None);
                let snapshot = st.clone();
                self.query_tabs[tab_idx].diagram_state = Some(st);
                self.save_diagram_and_propagate(conn_id, &db_name, &snapshot);
                self.toasts.success(if push {
                    "Merge applied. Saving the result to the database…"
                } else {
                    "Diagram updated to the database version"
                });
            }
            Err(e) => {
                log::error!("[DIAGRAM_SYNC] applying merge of '{db_name}' failed: {e}");
                st.pending_merge = Some(pending);
                self.query_tabs[tab_idx].diagram_state = Some(st);
                self.toasts.error(format!("Merge failed: {e}"));
            }
        }
    }
}
