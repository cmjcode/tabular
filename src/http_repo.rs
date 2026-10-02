//! UI penghubung folder HTTP API dengan repository kode dan diagram database:
//! - modal "Folder Repository" (URL git bersama + folder project personal);
//! - jendela "Generate Endpoints": kemajuan pemindaian [`crate::repo_endpoints`]
//!   dan review hasil, lalu request ditambahkan ke folder dan endpoint
//!   ditautkan ke tabel diagram yang group-nya memakai repository yang sama;
//! - jendela "Generate Integration Tests" dari satu atau beberapa folder;
//! - jendela "Integration Tests" untuk mengatur variabel dan menjalankan suite;
//! - popup daftar folder/group yang saling bertaut.
//!
//! Menu konteks sidebar dan diagram hanya punya `egui::Context`, jadi mereka
//! meminta aksi lewat [`request`]; aksi dijalankan di [`render`] yang
//! dipanggil tiap frame dengan akses ke `Tabular`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::agent::harness::{ProgressStatus, ProgressStep};
use crate::http_collection::{HttpFolder, save_workspaces};
use crate::http_tests::{HttpTestSuite, StepResult, TestEvent, TestGenHandle, TestRunHandle};
use crate::models::structs::{DiagramState, GroupRepoDraft};
use crate::repo_endpoints::{EndpointScanHandle, GeneratedEndpoint};
use crate::repo_links::{ApplyStats, EndpointTables, FolderRef, GroupRef, RepoIndex};
use crate::repo_scan::RepoJobEvent;
use crate::window_egui::{Tabular, style};

// ─── Aksi ───────────────────────────────────────────────────────────────────

/// Aksi dari menu konteks (sidebar HTTP API atau diagram).
#[derive(Clone, Debug)]
pub enum RepoAction {
    EditRepository {
        folder_id: String,
    },
    GenerateEndpoints {
        folder_id: String,
    },
    LinkEndpoints {
        folder_id: String,
    },
    ShowLinkedGroups {
        folder_id: String,
    },
    GenerateTests {
        folder_id: Option<String>,
    },
    OpenSuites,
    /// Dari diagram: folder HTTP API dengan repository `key`.
    ShowLinkedFolders {
        key: String,
        group_title: String,
    },
    /// Dari diagram: buka request tersimpan di HTTP client.
    OpenRequest {
        request_id: String,
        label: String,
    },
}

fn action_id() -> egui::Id {
    egui::Id::new("http_repo_pending_action")
}

/// Minta aksi dijalankan pada frame berikutnya.
pub fn request(ctx: &egui::Context, action: RepoAction) {
    ctx.data_mut(|d| d.insert_temp(action_id(), action));
    ctx.request_repaint();
}

// ─── State UI ───────────────────────────────────────────────────────────────

struct FolderRepoEditor {
    folder_id: String,
    folder_name: String,
    draft: GroupRepoDraft,
    had_repo: bool,
    auto_import_flexurio: bool,
}

/// Kemajuan job AI/pemindaian yang sedang ditampilkan.
#[derive(Default)]
struct JobProgress {
    steps: Vec<ProgressStep>,
    running: bool,
    started_at: Option<Instant>,
    last_activity_at: Option<Instant>,
    elapsed: Option<Duration>,
    error: Option<String>,
}

impl JobProgress {
    fn start() -> Self {
        Self {
            running: true,
            started_at: Some(Instant::now()),
            ..Default::default()
        }
    }

    fn step(&mut self, step: ProgressStep) {
        self.last_activity_at = Some(Instant::now());
        crate::window_egui::diagram::upsert_progress(&mut self.steps, step);
    }

    fn finish(&mut self, error: Option<String>) {
        self.running = false;
        self.elapsed = self.started_at.map(|t| t.elapsed());
        if error.is_none() {
            for s in &mut self.steps {
                if s.status == ProgressStatus::Active {
                    s.status = ProgressStatus::Done;
                }
            }
        }
        self.error = error;
    }

    fn show(&self, ui: &mut egui::Ui) {
        crate::diagram_repo::render_job_progress(
            ui,
            &self.steps,
            self.started_at,
            self.last_activity_at,
            self.elapsed,
            self.running,
        );
    }
}

struct EndpointRow {
    endpoint: GeneratedEndpoint,
    checked: bool,
    exists: bool,
}

struct EndpointWindow {
    folder_id: String,
    folder_name: String,
    key: Option<String>,
    linked_groups: usize,
    /// Kunci endpoint yang sudah ada di folder saat generate dimulai.
    existing: HashSet<String>,
    handle: Option<EndpointScanHandle>,
    progress: JobProgress,
    note: Option<String>,
    base_url: String,
    rows: Vec<EndpointRow>,
    filter: String,
    subfolders: bool,
    link_diagrams: bool,
    /// Setelah link, generate alur bisnis card di diagram yang sedang terbuka.
    also_flows: bool,
    selected: Option<usize>,
    files_scanned: usize,
    /// `false` = jendela disembunyikan user; job tetap berjalan di background
    /// dan jendela muncul lagi saat hasilnya siap.
    visible: bool,
    /// Batch AI paralel yang dipakai job ini.
    parallel: usize,
    /// Entri job ini di panel Background Processes.
    task_id: Option<u64>,
}

struct TestGenWindow {
    selected: HashSet<String>,
    instructions: String,
    handle: Option<TestGenHandle>,
    progress: Option<JobProgress>,
    /// `true` = jendela disembunyikan; job tetap berjalan di background.
    hidden: bool,
    /// Entri job ini di panel Background Processes.
    task_id: Option<u64>,
}

struct RunState {
    suite_id: String,
    handle: TestRunHandle,
    current: Option<usize>,
}

#[derive(Default)]
struct SuitesWindow {
    suites: Vec<HttpTestSuite>,
    selected: Option<String>,
    /// Hasil run terakhir per suite: indeks langkah → hasil.
    results: HashMap<String, Vec<Option<StepResult>>>,
    run: Option<RunState>,
    confirm_run: Option<String>,
    confirm_remove: Option<String>,
    expanded: HashSet<usize>,
    dirty: bool,
}

#[derive(Default)]
struct LinksPopup {
    title: String,
    groups: Vec<GroupRef>,
    folders: Vec<FolderRef>,
}

/// State semua jendela repository HTTP API; disimpan di `Tabular`.
#[derive(Default)]
pub struct HttpRepoUi {
    editor: Option<FolderRepoEditor>,
    /// Satu jendela (dan job) generate endpoint per folder; beberapa folder
    /// bisa berjalan bersamaan.
    endpoints: Vec<EndpointWindow>,
    /// Pilihan user untuk batch AI paralel per job (0 = bawaan).
    parallel_batches: usize,
    test_gen: Option<TestGenWindow>,
    suites: Option<SuitesWindow>,
    links: Option<LinksPopup>,
}

// ─── Menu sidebar ───────────────────────────────────────────────────────────

/// Item menu repository untuk folder HTTP API (menu konteks sidebar).
pub fn folder_menu_items(ui: &mut egui::Ui, folder: &HttpFolder) {
    use egui_icons::icons as i;
    ui.separator();
    let has_repo = folder.has_repository();
    let label = if has_repo {
        "Edit Repository…"
    } else {
        "Set Repository…"
    };
    let folder_id = folder.id.clone();
    if ui
        .button(format!("{} {label}", i::MDI_GIT.codepoint))
        .on_hover_text("Link a git repository or local project folder to this folder")
        .clicked()
    {
        request(
            ui.ctx(),
            RepoAction::EditRepository {
                folder_id: folder_id.clone(),
            },
        );
        ui.close();
    }
    if ui
        .button(format!(
            "{} Generate Endpoints from Repo (AI)",
            i::ICON_AUTO_AWESOME.codepoint
        ))
        .on_hover_text(
            "Find every endpoint in the repository and add it here with parameters, body, \
             auth and the tables it uses",
        )
        .clicked()
    {
        request(
            ui.ctx(),
            RepoAction::GenerateEndpoints {
                folder_id: folder_id.clone(),
            },
        );
        ui.close();
    }
    if has_repo
        && ui
            .button(format!(
                "{} Linked Diagram Groups…",
                i::ICON_SCHEMA.codepoint
            ))
            .on_hover_text("Diagram groups that use the same git repository")
            .clicked()
    {
        request(
            ui.ctx(),
            RepoAction::ShowLinkedGroups {
                folder_id: folder_id.clone(),
            },
        );
        ui.close();
    }
    if folder.all_requests().iter().any(|r| !r.tables.is_empty())
        && ui
            .button(format!(
                "{} Link Endpoints to Diagram Tables",
                i::MDI_LINK_VARIANT.codepoint
            ))
            .on_hover_text("Show these endpoints on the tables they use in linked diagrams")
            .clicked()
    {
        request(
            ui.ctx(),
            RepoAction::LinkEndpoints {
                folder_id: folder_id.clone(),
            },
        );
        ui.close();
    }
    if ui
        .button(format!(
            "{} Generate Integration Tests (AI)…",
            i::MDI_TEST_TUBE.codepoint
        ))
        .clicked()
    {
        request(
            ui.ctx(),
            RepoAction::GenerateTests {
                folder_id: Some(folder_id.clone()),
            },
        );
        ui.close();
    }
    #[cfg(not(target_os = "ios"))]
    if let Some(path) = folder
        .local_repo_path()
        .map(|p| crate::repo_scan::expand_home(&p))
        .filter(|p| p.is_dir())
        && ui
            .button(format!(
                "{} Open Folder Location",
                i::ICON_FOLDER_OPEN.codepoint
            ))
            .clicked()
    {
        if let Err(e) = crate::url_opener::open_folder(&path) {
            log::warn!("[HTTP_REPO] cannot open {}: {e}", path.display());
        }
        ui.close();
    }
}

/// Item menu integration test untuk workspace HTTP API.
pub fn workspace_menu_items(ui: &mut egui::Ui) {
    use egui_icons::icons as i;
    ui.separator();
    if ui
        .button(format!(
            "{} Generate Integration Tests (AI)…",
            i::MDI_TEST_TUBE.codepoint
        ))
        .clicked()
    {
        request(ui.ctx(), RepoAction::GenerateTests { folder_id: None });
        ui.close();
    }
    if ui
        .button(format!(
            "{} Integration Tests…",
            i::ICON_CHECKLIST.codepoint
        ))
        .clicked()
    {
        request(ui.ctx(), RepoAction::OpenSuites);
        ui.close();
    }
}

// ─── Indeks & tautan diagram ────────────────────────────────────────────────

fn diagrams_dir(app: &Tabular) -> Option<std::path::PathBuf> {
    app.get_diagram_path(0, "_")
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
}

/// Indeks group diagram (tab terbuka lebih dulu, lalu file cache) dan folder
/// HTTP API yang punya repository.
fn build_index(app: &Tabular) -> RepoIndex {
    let mut idx = RepoIndex::default();
    for tab in &app.query_tabs {
        let (Some(conn), Some(db), Some(state)) = (
            tab.connection_id,
            tab.database_name.as_deref(),
            tab.diagram_state.as_ref(),
        ) else {
            continue;
        };
        if state.scoped_to.is_none() {
            idx.add_diagram(conn, db, state);
        }
    }
    if let Some(dir) = diagrams_dir(app) {
        for (conn, db, _, state) in crate::repo_links::load_diagram_files(&dir) {
            idx.add_diagram(conn, &db, &state);
        }
    }
    idx.add_workspaces(&app.yaak_workspaces);
    idx
}

fn has_key(state: &DiagramState, key: &str) -> bool {
    state
        .groups
        .iter()
        .any(|g| g.repo_key().as_deref() == Some(key))
}

#[derive(Default)]
struct LinkReport {
    diagrams: usize,
    stats: ApplyStats,
}

impl LinkReport {
    fn add(&mut self, s: ApplyStats) {
        self.stats.added += s.added;
        self.stats.updated += s.updated;
        self.stats.unresolved += s.unresolved;
        if s.changed() {
            self.diagrams += 1;
        }
    }

    fn summary(&self) -> String {
        let pairs = self.stats.added + self.stats.updated;
        if pairs == 0 {
            return String::new();
        }
        let mut s = format!(
            " Linked {pairs} endpoint-table pair(s) in {} diagram(s).",
            self.diagrams
        );
        if self.stats.unresolved > 0 {
            s.push_str(&format!(
                " {} table name(s) are not in those diagrams.",
                self.stats.unresolved
            ));
        }
        s
    }
}

/// Tautkan endpoint ke tabel semua diagram yang punya group dengan
/// repository `key`: tab yang terbuka diperbarui di memori lalu disimpan,
/// diagram lain ditulis langsung ke file cache lokalnya.
fn link_to_diagrams(app: &mut Tabular, key: &str, endpoints: &[EndpointTables]) -> LinkReport {
    let mut report = LinkReport::default();
    if endpoints.is_empty() {
        return report;
    }
    let mut done_paths: HashSet<std::path::PathBuf> = HashSet::new();
    let mut to_save: Vec<(i64, String, DiagramState)> = Vec::new();
    for tab in app.query_tabs.iter_mut() {
        let (Some(conn), Some(db)) = (tab.connection_id, tab.database_name.clone()) else {
            continue;
        };
        let Some(state) = tab.diagram_state.as_mut() else {
            continue;
        };
        if state.scoped_to.is_some() || !has_key(state, key) {
            continue;
        }
        let stats = crate::repo_links::apply_endpoint_links(state, Some(key), endpoints);
        let synced = crate::diagram_flow::sync_cards_from_links(state);
        report.add(stats);
        if stats.changed() || synced {
            to_save.push((conn, db, state.clone()));
        }
    }
    // File milik tab terbuka tidak ditulis ulang: tab yang menyimpannya.
    for tab in &app.query_tabs {
        if let (Some(conn), Some(db), Some(state)) = (
            tab.connection_id,
            tab.database_name.as_deref(),
            tab.diagram_state.as_ref(),
        ) && state.scoped_to.is_none()
            && let Some(p) = app.get_diagram_path(conn, db)
        {
            done_paths.insert(p);
        }
    }
    for (conn, db, state) in to_save {
        app.save_diagram(conn, &db, &state);
    }
    let Some(dir) = diagrams_dir(app) else {
        return report;
    };
    for (_, _, path, mut state) in crate::repo_links::load_diagram_files(&dir) {
        if done_paths.contains(&path) || !has_key(&state, key) {
            continue;
        }
        let stats = crate::repo_links::apply_endpoint_links(&mut state, Some(key), endpoints);
        let synced = crate::diagram_flow::sync_cards_from_links(&mut state);
        report.add(stats);
        if !stats.changed() && !synced {
            continue;
        }
        let written = serde_json::to_vec_pretty(&state)
            .map_err(|e| e.to_string())
            .and_then(|b| crate::diagram_view::write_atomic(&path, &b).map_err(|e| e.to_string()));
        if let Err(e) = written {
            log::warn!("[HTTP_REPO] cannot update diagram {}: {e}", path.display());
            app.toasts
                .error(format!("Could not update diagram {}: {e}", path.display()));
        }
    }
    report
}

/// Mulai generate alur bisnis untuk card repository `key` di tiap diagram
/// terbuka yang punya group dengan repository itu. Card yang file sumbernya
/// tidak berubah dilewati oleh job.
fn generate_flows_in_open_diagrams(app: &mut Tabular, key: &str) {
    let targets: Vec<(Option<i64>, Option<String>, Vec<String>)> = app
        .query_tabs
        .iter()
        .filter_map(|tab| {
            let state = tab.diagram_state.as_ref()?;
            if state.scoped_to.is_some() || !has_key(state, key) {
                return None;
            }
            let ids: Vec<String> = state
                .flow_cards
                .iter()
                .filter(|c| c.repo_key.as_deref() == Some(key))
                .map(|c| c.id.clone())
                .collect();
            (!ids.is_empty()).then(|| (tab.connection_id, tab.database_name.clone(), ids))
        })
        .collect();
    let mut seen = HashSet::new();
    for (conn_id, db_name, ids) in targets {
        // Diagram yang terbuka di beberapa tab cukup satu job.
        if seen.insert((conn_id, db_name.clone())) {
            app.start_flow_generation(conn_id, db_name, None, &ids, false);
        }
    }
}

/// Tampilkan folder di sidebar HTTP API (buka tab APIs, expand leluhurnya).
fn reveal_folder(app: &mut Tabular, folder_id: &str) {
    fn path_to(folders: &[HttpFolder], id: &str, out: &mut Vec<String>) -> bool {
        for f in folders {
            out.push(f.id.clone());
            if f.id == id || path_to(&f.children, id, out) {
                return true;
            }
            out.pop();
        }
        false
    }
    for ws in &app.yaak_workspaces {
        let mut path = Vec::new();
        if path_to(&ws.folders, folder_id, &mut path) {
            app.collection_expanded_folders.extend(path);
            app.selected_menu = "APIs".to_string();
            return;
        }
    }
}

fn open_group(app: &mut Tabular, g: &GroupRef) {
    app.show_database_diagram(g.conn_id, g.db_name.clone());
    if let Some(state) = app
        .query_tabs
        .iter_mut()
        .filter(|t| {
            t.connection_id == Some(g.conn_id) && t.database_name.as_deref() == Some(&g.db_name)
        })
        .filter_map(|t| t.diagram_state.as_mut())
        .find(|s| s.scoped_to.is_none())
    {
        state.focus_group = Some(g.group_id.clone());
        state.focus_table = None;
    }
}

type BackendChoice = (
    Option<crate::ai_assistant::ChatBackend>,
    String,
    Option<String>,
);

fn chat_backend(app: &Tabular) -> BackendChoice {
    let target = app.effective_chat_target();
    match crate::ai_assistant::backend_ready_for(app, target) {
        Ok(()) => (
            Some(crate::ai_assistant::chat_backend_for(app, target)),
            crate::ai_assistant::backend_label_for(app, target),
            None,
        ),
        Err(e) => (None, String::new(), Some(e)),
    }
}

/// Tautkan ulang semua request folder yang punya daftar tabel ke diagram.
fn link_folder(app: &mut Tabular, folder_id: &str) {
    let Some((_, folder)) =
        crate::http_collection::find_workspace_folder(&app.yaak_workspaces, folder_id)
    else {
        return;
    };
    let Some(key) = folder.repo_key() else {
        app.toasts
            .info("Set a git URL on this folder first so it can be matched to a diagram group");
        return;
    };
    let eps: Vec<EndpointTables> = folder
        .all_requests()
        .into_iter()
        .filter(|r| !r.tables.is_empty())
        .map(EndpointTables::from_request)
        .collect();
    let report = link_to_diagrams(app, &key, &eps);
    let summary = report.summary();
    if summary.is_empty() {
        app.toasts.info(
            "Nothing new to link. Endpoints link to diagrams whose groups use the same git URL.",
        );
    } else {
        app.toasts.success(summary.trim().to_string());
    }
}

// ─── Loop utama ─────────────────────────────────────────────────────────────

/// Jalankan aksi langsung (dipanggil dari kode yang punya `Tabular`).
pub fn perform(app: &mut Tabular, action: RepoAction) {
    let mut ui = std::mem::take(&mut app.http_repo);
    ui.handle(app, action);
    app.http_repo = ui;
}

/// Jalankan aksi tertunda, poll job, dan gambar jendela. Dipanggil tiap frame.
pub fn render(app: &mut Tabular, ctx: &egui::Context) {
    let mut ui = std::mem::take(&mut app.http_repo);
    let pending = ctx.data_mut(|d| {
        let action = d.get_temp::<RepoAction>(action_id());
        d.remove::<RepoAction>(action_id());
        action
    });
    if let Some(action) = pending {
        ui.handle(app, action);
    }
    ui.poll(app, ctx);
    ui.sync_background_tasks(app);
    ui.render_editor(app, ctx);
    ui.render_endpoints(app, ctx);
    ui.render_test_gen(app, ctx);
    ui.render_suites(app, ctx);
    ui.render_links(app, ctx);
    app.http_repo = ui;
}

impl HttpRepoUi {
    fn handle(&mut self, app: &mut Tabular, action: RepoAction) {
        match action {
            RepoAction::EditRepository { folder_id } => self.open_editor(app, &folder_id),
            RepoAction::GenerateEndpoints { folder_id } => {
                self.start_endpoints(app, &folder_id, false)
            }
            RepoAction::LinkEndpoints { folder_id } => link_folder(app, &folder_id),
            RepoAction::ShowLinkedGroups { folder_id } => self.show_linked_groups(app, &folder_id),
            RepoAction::ShowLinkedFolders { key, group_title } => {
                let mut idx = RepoIndex::default();
                idx.add_workspaces(&app.yaak_workspaces);
                let folders: Vec<FolderRef> = idx.folders_for(&key).cloned().collect();
                match folders.len() {
                    0 => app.toasts.info(format!(
                        "No HTTP API folder uses this repository. In the APIs sidebar, \
                         right-click a folder and choose Set Repository… with the same git URL \
                         as {group_title}."
                    )),
                    1 => {
                        reveal_folder(app, &folders[0].folder_id);
                        app.toasts
                            .info(format!("HTTP API folder: {}", folders[0].folder_name));
                    }
                    _ => {
                        self.links = Some(LinksPopup {
                            title: format!("HTTP API folders linked to {group_title}"),
                            groups: Vec::new(),
                            folders,
                        })
                    }
                }
            }
            RepoAction::OpenRequest { request_id, label } => {
                match crate::http_collection::find_request(&app.yaak_workspaces, &request_id)
                    .cloned()
                {
                    Some(req) => {
                        if let Some(folder_id) = req.folder_id.clone() {
                            reveal_folder(app, &folder_id);
                        }
                        crate::sidebar_collection::apply_collection_request_to_active_tab(
                            app, &req,
                        );
                    }
                    None => app.toasts.info(format!(
                        "{label} is not in your HTTP API collections on this computer. \
                         Generate endpoints from the repository to add it."
                    )),
                }
            }
            RepoAction::GenerateTests { folder_id } => {
                self.test_gen = Some(TestGenWindow {
                    selected: folder_id.into_iter().collect(),
                    instructions: String::new(),
                    handle: None,
                    progress: None,
                    hidden: false,
                    task_id: None,
                });
            }
            RepoAction::OpenSuites => self.open_suites(None),
        }
    }

    fn show_linked_groups(&mut self, app: &mut Tabular, folder_id: &str) {
        let Some((_, folder)) =
            crate::http_collection::find_workspace_folder(&app.yaak_workspaces, folder_id)
        else {
            return;
        };
        let name = folder.name.clone();
        let Some(key) = folder.repo_key() else {
            app.toasts.info("Set a git URL on this folder first");
            return;
        };
        let idx = build_index(app);
        let groups: Vec<GroupRef> = idx.groups_for(&key).cloned().collect();
        match groups.len() {
            0 => app.toasts.info(format!(
                "No diagram group uses {key}. Set the same git URL on a group with Set \
                 Repository… in the database diagram."
            )),
            1 => open_group(app, &groups[0]),
            _ => {
                self.links = Some(LinksPopup {
                    title: format!("Diagram groups linked to {name}"),
                    groups,
                    folders: Vec::new(),
                })
            }
        }
    }

    fn open_editor(&mut self, app: &Tabular, folder_id: &str) {
        let Some((_, folder)) =
            crate::http_collection::find_workspace_folder(&app.yaak_workspaces, folder_id)
        else {
            return;
        };
        let is_empty = folder.all_requests().is_empty();
        self.editor = Some(FolderRepoEditor {
            folder_id: folder.id.clone(),
            folder_name: folder.name.clone(),
            draft: GroupRepoDraft {
                group_id: folder.id.clone(),
                path: folder.local_repo_path().unwrap_or_default(),
                url: folder.repo_url.clone().unwrap_or_default(),
                url_auto: false,
            },
            had_repo: folder.has_repository(),
            auto_import_flexurio: is_empty,
        });
    }

    fn open_suites(&mut self, select: Option<String>) {
        let win = self.suites.get_or_insert_with(SuitesWindow::default);
        if win.run.is_none() {
            win.suites = crate::http_tests::load_suites();
        }
        win.selected = select
            .or_else(|| win.selected.clone())
            .filter(|id| win.suites.iter().any(|s| &s.id == id))
            .or_else(|| win.suites.first().map(|s| s.id.clone()));
    }

    fn parallel(&self) -> usize {
        match self.parallel_batches {
            0 => crate::repo_endpoints::DEFAULT_PARALLEL_BATCHES,
            n => n.min(crate::repo_endpoints::MAX_PARALLEL_BATCHES),
        }
    }

    /// Mulai generate endpoint untuk folder. Job folder lain tidak diganggu.
    /// Bila job folder ini masih berjalan dan `restart` false, jendelanya
    /// hanya ditampilkan lagi.
    fn start_endpoints(&mut self, app: &mut Tabular, folder_id: &str, restart: bool) {
        if !restart
            && let Some(win) = self
                .endpoints
                .iter_mut()
                .find(|w| w.folder_id == folder_id && w.handle.is_some())
        {
            win.visible = true;
            return;
        }
        let Some((_, folder)) =
            crate::http_collection::find_workspace_folder(&app.yaak_workspaces, folder_id)
        else {
            return;
        };
        if !folder.has_repository() {
            self.open_editor(app, folder_id);
            app.toasts
                .info("Set the repository of this folder first, then generate endpoints.");
            return;
        }
        // Job lama folder yang sama digantikan; folder lain dibiarkan.
        self.endpoints.retain(|w| {
            let same = w.folder_id == folder_id;
            if same && let Some(h) = &w.handle {
                h.cancel();
            }
            !same
        });
        let parallel = self.parallel();
        let key = folder.repo_key();
        let repo_path = folder.local_repo_path();
        let repo_url = folder.repo_url.clone();
        let folder_name = folder.name.clone();
        let existing = crate::repo_endpoints::folder_request_keys(folder);
        let idx = build_index(app);
        let (tables, linked_groups) = key
            .as_deref()
            .map(|k| (idx.tables_for(k), idx.groups_for(k).count()))
            .unwrap_or_default();
        let (backend, backend_label, backend_note) = chat_backend(app);
        log::info!(
            "[HTTP_REPO] generating endpoints for '{folder_name}' ({} linked table(s), AI: {})",
            tables.len(),
            if backend.is_some() {
                backend_label.as_str()
            } else {
                "off"
            }
        );
        let handle =
            crate::repo_endpoints::spawn_endpoint_scan(crate::repo_endpoints::EndpointScanInput {
                repo_path,
                repo_url,
                folder_name: folder_name.clone(),
                tables,
                backend,
                backend_label,
                cache_root: crate::repo_scan::default_cache_root(),
                parallel,
            });
        self.endpoints.push(EndpointWindow {
            folder_id: folder_id.to_string(),
            folder_name,
            key,
            linked_groups,
            existing,
            handle: Some(handle),
            progress: JobProgress::start(),
            note: backend_note,
            base_url: String::new(),
            rows: Vec::new(),
            filter: String::new(),
            subfolders: true,
            link_diagrams: linked_groups > 0,
            also_flows: false,
            selected: None,
            files_scanned: 0,
            visible: true,
            parallel,
            task_id: None,
        });
    }

    /// Cerminkan job AI yang berjalan ke panel Background Processes dan
    /// jalankan permintaan panel (tampilkan, batal).
    fn sync_background_tasks(&mut self, app: &mut Tabular) {
        use crate::window_egui::background_tasks::{Snapshot, TaskOwner};

        let tasks = &mut app.background_tasks;
        let mut seen = Vec::new();
        self.endpoints.retain_mut(|win| {
            if win.handle.is_none() && win.task_id.is_none() {
                return true;
            }
            let title = format!("Generate endpoints: {}", win.folder_name);
            let out = tasks.mirror(
                &mut win.task_id,
                Snapshot {
                    owner: TaskOwner::HttpRepo,
                    title: &title,
                    subtitle: "",
                    steps: &win.progress.steps,
                    started_at: win.progress.started_at,
                    last_activity_at: win.progress.last_activity_at,
                    hidden: !win.visible,
                    // Jendela muncul lagi sendiri saat hasilnya siap.
                    result: win.handle.is_none().then(|| Ok(String::new())),
                },
            );
            seen.extend(win.task_id);
            if out.show {
                win.visible = true;
            }
            if out.cancel
                && let Some(h) = win.handle.take()
            {
                h.cancel();
                win.progress.finish(Some("Cancelled".into()));
                // Jendela tersembunyi yang dibatalkan tidak perlu muncul lagi.
                return win.visible;
            }
            true
        });
        let mut close_test_gen = false;
        if let Some(win) = self.test_gen.as_mut()
            && (win.handle.is_some() || win.task_id.is_some())
        {
            let (steps, started_at, last_activity_at) = match &win.progress {
                Some(p) => (p.steps.as_slice(), p.started_at, p.last_activity_at),
                None => (&[][..], None, None),
            };
            let out = tasks.mirror(
                &mut win.task_id,
                Snapshot {
                    owner: TaskOwner::HttpRepo,
                    title: "Generate integration tests",
                    subtitle: "",
                    steps,
                    started_at,
                    last_activity_at,
                    hidden: win.hidden,
                    result: win.handle.is_none().then(|| Ok(String::new())),
                },
            );
            seen.extend(win.task_id);
            if out.show {
                win.hidden = false;
            }
            if out.cancel
                && let Some(h) = win.handle.take()
            {
                h.cancel();
                if let Some(p) = win.progress.as_mut() {
                    p.finish(Some("Cancelled".into()));
                }
                close_test_gen = win.hidden;
            }
        }
        if close_test_gen {
            self.test_gen = None;
        }
        tasks.retain_owner(TaskOwner::HttpRepo, &seen);
    }

    fn poll(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        let mut busy = false;
        for win in self.endpoints.iter_mut() {
            let was_running = win.handle.is_some();
            busy |= poll_endpoints(win);
            if was_running && win.handle.is_none() && !win.visible {
                // Job di background selesai: tampilkan hasilnya.
                win.visible = true;
                match &win.progress.error {
                    Some(e) => app.toasts.error(format!(
                        "Generating endpoints for '{}' failed: {e}",
                        win.folder_name
                    )),
                    None => app.toasts.success(format!(
                        "{} endpoint(s) ready to review for '{}'",
                        win.rows.len(),
                        win.folder_name
                    )),
                }
            }
        }

        let mut generated: Option<Vec<HttpTestSuite>> = None;
        if let Some(win) = self.test_gen.as_mut()
            && let (Some(handle), Some(progress)) = (win.handle.as_ref(), win.progress.as_mut())
        {
            busy = true;
            let mut finished = false;
            loop {
                match handle.rx.try_recv() {
                    Ok(RepoJobEvent::Progress(step)) => progress.step(step),
                    Ok(RepoJobEvent::Activity) => progress.last_activity_at = Some(Instant::now()),
                    Ok(RepoJobEvent::Finished(result)) => {
                        match result {
                            Ok(suites) => {
                                progress.finish(None);
                                generated = Some(suites);
                            }
                            Err(e) => progress.finish(Some(e)),
                        }
                        finished = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        progress.finish(Some("Test generation stopped unexpectedly".into()));
                        finished = true;
                        break;
                    }
                }
            }
            if finished {
                win.handle = None;
                if win.hidden {
                    // Gagal di background: munculkan lagi jendelanya.
                    win.hidden = false;
                    if let Some(e) = &progress.error {
                        app.toasts
                            .error(format!("Generating integration tests failed: {e}"));
                    }
                }
            }
        }
        if let Some(suites) = generated {
            let mut saved = 0;
            for s in &suites {
                match crate::http_tests::save_suite(s) {
                    Ok(()) => saved += 1,
                    Err(e) => app.toasts.error(e),
                }
            }
            let steps: usize = suites.iter().map(|s| s.steps.len()).sum();
            app.toasts.success(format!(
                "Generated {saved} integration test suite(s) with {steps} step(s)"
            ));
            self.test_gen = None;
            self.open_suites(suites.first().map(|s| s.id.clone()));
        }

        if let Some(win) = self.suites.as_mut()
            && let Some(run) = win.run.as_mut()
        {
            busy = true;
            let results = win.results.entry(run.suite_id.clone()).or_default();
            let mut done = None;
            loop {
                match run.handle.rx.try_recv() {
                    Ok(TestEvent::StepStarted(i)) => run.current = Some(i),
                    Ok(TestEvent::StepFinished(r)) => {
                        let i = r.index;
                        if results.len() <= i {
                            results.resize(i + 1, None);
                        }
                        results[i] = Some(*r);
                    }
                    Ok(TestEvent::Finished { cancelled }) => {
                        done = Some(cancelled);
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        done = Some(true);
                        break;
                    }
                }
            }
            if let Some(cancelled) = done {
                let finished: Vec<&StepResult> = results.iter().flatten().collect();
                let passed = finished.iter().filter(|r| r.passed()).count();
                let failed = finished
                    .iter()
                    .filter(|r| !r.passed() && !r.skipped)
                    .count();
                let msg = format!(
                    "Test run {}: {passed} passed, {failed} failed",
                    if cancelled { "stopped" } else { "finished" }
                );
                if failed > 0 || cancelled {
                    app.toasts.error(msg);
                } else {
                    app.toasts.success(msg);
                }
                win.run = None;
            }
        }
        if busy {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }
}

/// Terima event job generate endpoint. `true` = job masih berjalan.
fn poll_endpoints(win: &mut EndpointWindow) -> bool {
    let Some(handle) = win.handle.as_ref() else {
        return false;
    };
    let mut finished = false;
    loop {
        match handle.rx.try_recv() {
            Ok(RepoJobEvent::Progress(step)) => win.progress.step(step),
            Ok(RepoJobEvent::Activity) => win.progress.last_activity_at = Some(Instant::now()),
            Ok(RepoJobEvent::Finished(result)) => {
                match result {
                    Ok(outcome) => {
                        win.base_url = outcome.base_url;
                        win.files_scanned = outcome.files_scanned;
                        win.note = match (win.note.take(), outcome.note) {
                            (Some(a), Some(b)) => Some(format!("{a} {b}")),
                            (a, b) => b.or(a),
                        };
                        let existing = &win.existing;
                        win.rows = outcome
                            .endpoints
                            .into_iter()
                            .map(|endpoint| {
                                let exists = existing.contains(&endpoint.key());
                                EndpointRow {
                                    endpoint,
                                    checked: !exists,
                                    exists,
                                }
                            })
                            .collect();
                        win.rows.sort_by(|a, b| {
                            let rank = |r: &EndpointRow| {
                                (
                                    r.endpoint.path.clone(),
                                    crate::repo_links::method_rank(&r.endpoint.method),
                                )
                            };
                            rank(a).cmp(&rank(b))
                        });
                        win.progress.finish(None);
                    }
                    Err(e) => win.progress.finish(Some(e)),
                }
                finished = true;
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                win.progress
                    .finish(Some("Endpoint generation stopped unexpectedly".into()));
                finished = true;
                break;
            }
        }
    }
    if finished {
        win.handle = None;
    }
    !finished
}

/// Warna method HTTP (juga dipakai flow card di diagram).
pub(crate) fn method_color(method: &str) -> egui::Color32 {
    match method.to_ascii_uppercase().as_str() {
        "GET" => egui::Color32::from_rgb(76, 175, 80),
        "POST" => egui::Color32::from_rgb(255, 167, 38),
        "PUT" => egui::Color32::from_rgb(66, 165, 245),
        "PATCH" => egui::Color32::from_rgb(171, 71, 188),
        "DELETE" => egui::Color32::from_rgb(239, 83, 80),
        _ => egui::Color32::from_rgb(158, 158, 158),
    }
}

/// Chip method HTTP berwarna.
pub fn method_chip(ui: &mut egui::Ui, method: &str) -> egui::Response {
    let color = method_color(method);
    egui::Frame::NONE
        .fill(color.linear_multiply(0.18))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(4, 1))
        .show(ui, |ui| {
            ui.set_min_width(46.0);
            ui.label(
                egui::RichText::new(method)
                    .family(egui::FontFamily::Monospace)
                    .size(10.5)
                    .strong()
                    .color(color),
            )
        })
        .inner
}

fn small_weak(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).small().weak());
}

// ─── Jendela ────────────────────────────────────────────────────────────────

impl HttpRepoUi {
    fn render_editor(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        let Some(mut ed) = self.editor.take() else {
            return;
        };
        let mut close = false;
        let mut save = false;
        let mut generate_after = false;
        let mut import_flexurio_after = false;
        let mut remove = false;
        let (clone, info) = crate::diagram_repo::RepoCloneState::poll(
            ctx,
            &format!("http:{}", ed.folder_id),
            &mut ed.draft,
        );
        if let Some(msg) = info {
            app.toasts.success(msg);
        }

        let folder_path = crate::repo_scan::expand_home(ed.draft.path.trim());
        let flexurio_path_opt = crate::flexurio_import::detect_flexurio_config(&folder_path);
        let is_flexurio = flexurio_path_opt.is_some();

        style::render_modal_backdrop(ctx, "http_folder_repo_backdrop", true);
        let win_w = (ctx.content_rect().width() - 48.0).clamp(340.0, 580.0);
        egui::Window::new("Folder Repository")
            .title_bar(false)
            .frame(style::modal_window_frame(ctx))
            .collapsible(false)
            .resizable(false)
            .fixed_size(egui::vec2(win_w, 0.0))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(win_w);
                style::render_modal_header(
                    ui,
                    format!("Repository for {}", ed.folder_name),
                    &mut close,
                );
                ui.label(
                    egui::RichText::new(
                        "Link the code of this API. Generate Endpoints reads it to create every \
                         request with its parameters, body and auth. Diagram groups with the \
                         same git URL are linked to this folder, so endpoints appear on the \
                         tables they use. The project folder is personal and stays on this \
                         computer.",
                    )
                    .weak()
                    .small(),
                );
                ui.add_space(8.0);
                let fields = crate::diagram_repo::render_repo_fields(
                    ui,
                    &mut ed.draft,
                    &clone,
                    "this HTTP collection",
                );
                if is_flexurio {
                    ui.add_space(4.0);
                    ui.checkbox(
                        &mut ed.auto_import_flexurio,
                        egui::RichText::new("Import Flexurio routes into this folder on Save").small(),
                    );
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ed.had_repo
                        && ui
                            .add(egui::Button::new("Remove").min_size(egui::vec2(0.0, 28.0)))
                            .on_hover_text("Unlink the folder and URL from this HTTP folder")
                            .clicked()
                    {
                        remove = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let valid = fields.valid(&clone);
                        if is_flexurio {
                            if ui
                                .add_enabled(
                                    valid,
                                    crate::diagram_repo::accent_button(
                                        ui,
                                        "⚡ Save & Import Flexurio",
                                    ),
                                )
                                .on_hover_text(
                                    "Save repository and import all routes and endpoints directly from config/routes.json into this folder",
                                )
                                .clicked()
                            {
                                save = true;
                                import_flexurio_after = true;
                            }
                        }
                        if ui
                            .add_enabled(
                                valid,
                                if is_flexurio {
                                    egui::Button::new("Scan Endpoints…").min_size(egui::vec2(0.0, 28.0))
                                } else {
                                    crate::diagram_repo::accent_button(ui, "Save & Generate Endpoints")
                                },
                            )
                            .clicked()
                        {
                            save = true;
                            generate_after = true;
                        }
                        if ui
                            .add_enabled(
                                valid,
                                egui::Button::new("Save").min_size(egui::vec2(0.0, 28.0)),
                            )
                            .clicked()
                        {
                            save = true;
                            if is_flexurio && ed.auto_import_flexurio {
                                import_flexurio_after = true;
                            }
                        }
                        if ui
                            .add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 28.0)))
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
            });
        if close || save || remove {
            clone.finish(ctx);
        }
        if save || remove {
            let clean = |v: &str| {
                (!remove)
                    .then(|| v.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            if let Err(e) = crate::diagram_repo_paths::set_http_folder_repo_path(
                &ed.folder_id,
                clean(&ed.draft.path).as_deref(),
            ) {
                app.toasts.error(e);
                return;
            }
            let url = clean(&ed.draft.url);
            let mut changed = false;
            for ws in app.yaak_workspaces.iter_mut() {
                if let Some(f) =
                    crate::http_collection::find_folder_mut(&mut ws.folders, &ed.folder_id)
                    && f.repo_url != url
                {
                    f.repo_url = url.clone();
                    changed = true;
                }
            }
            if changed && let Err(e) = save_workspaces(&app.yaak_workspaces) {
                app.toasts.error(e);
            }
            if import_flexurio_after && let Some(cfg_file) = &flexurio_path_opt {
                match crate::flexurio_import::import_flexurio_into_folder(
                    &mut app.yaak_workspaces,
                    &ed.folder_id,
                    cfg_file,
                ) {
                    Ok(res) => {
                        if let Err(e) = save_workspaces(&app.yaak_workspaces) {
                            app.toasts.error(e);
                        }
                        app.toasts.success(format!(
                            "Imported {} endpoints across {} routes from Flexurio into '{}'",
                            res.total_requests, res.total_routes, ed.folder_name
                        ));
                        for w in res.warnings {
                            app.toasts.warning(w);
                        }
                    }
                    Err(e) => {
                        app.toasts.error(format!("Flexurio import failed: {e}"));
                    }
                }
            } else if generate_after {
                self.start_endpoints(app, &ed.folder_id, true);
            } else {
                app.toasts.success(if remove {
                    "Repository unlinked from the folder"
                } else {
                    "Folder repository saved"
                });
            }
            return;
        }
        if !close {
            self.editor = Some(ed);
        }
    }

    fn render_endpoints(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        let windows = std::mem::take(&mut self.endpoints);
        let mut keep = Vec::with_capacity(windows.len());
        for win in windows {
            if let Some(win) = self.render_endpoint_window(app, ctx, win) {
                keep.push(win);
            }
        }
        // Job yang dimulai selama render (Rescan) sudah masuk `self.endpoints`.
        keep.append(&mut self.endpoints);
        self.endpoints = keep;
    }

    /// Gambar satu jendela generate endpoint. `None` = jendela ditutup.
    fn render_endpoint_window(
        &mut self,
        app: &mut Tabular,
        ctx: &egui::Context,
        mut win: EndpointWindow,
    ) -> Option<EndpointWindow> {
        if !win.visible {
            return Some(win);
        }
        let mut open = true;
        let mut add = false;
        let mut rescan = false;
        let mut cancel = false;
        let mut background = false;
        let mut parallel = self.parallel();
        // Tinggi dikunci (hanya lebar yang bisa diubah) supaya jendela tidak memanjang
        // sampai setinggi layar; tetap muat di layar kecil.
        let height = (ctx.content_rect().height() - 120.0).clamp(240.0, 580.0);
        // Lebar maksimum 50% window utama supaya jendela tidak melebar sampai penuh.
        let max_width = (ctx.content_rect().width() * 0.5).max(360.0);
        egui::Window::new(format!("Generate Endpoints · {}", win.folder_name))
            .id(egui::Id::new((
                "http_repo_endpoints_window",
                &win.folder_id,
            )))
            .open(&mut open)
            .collapsible(true)
            .resizable([true, false])
            .default_size(egui::vec2(max_width.min(780.0), height))
            .max_width(max_width)
            .min_height(height)
            .max_height(height)
            .show(ctx, |ui| {
                if win.progress.running {
                    small_weak(
                        ui,
                        format!(
                            "{} AI batch(es) in parallel. Process it in the background to keep \
                             working; this window opens again when the endpoints are ready.",
                            win.parallel
                        ),
                    );
                    ui.add_space(4.0);
                    // Baris langkah yang panjang digulung, bukan melebarkan jendela.
                    egui::ScrollArea::both()
                        .id_salt("http_repo_endpoints_progress")
                        .max_height((ui.available_height() - 40.0).max(80.0))
                        .auto_shrink([false, true])
                        .show(ui, |ui| win.progress.show(ui));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        if ui
                            .button("Process in Background")
                            .on_hover_text(
                                "Hide this window and keep working. Follow it in Background \
                                 Processes at the bottom of the sidebar.",
                            )
                            .clicked()
                        {
                            background = true;
                        }
                    });
                    return;
                }
                if let Some(e) = &win.progress.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                    ui.add_space(6.0);
                    egui::CollapsingHeader::new("Details")
                        .default_open(false)
                        .show(ui, |ui| {
                            egui::ScrollArea::both()
                                .id_salt("http_repo_endpoints_details")
                                .max_height((ui.available_height() - 40.0).max(80.0))
                                .auto_shrink([false, true])
                                .show(ui, |ui| win.progress.show(ui));
                        });
                    ui.horizontal(|ui| {
                        if ui.button("Retry").clicked() {
                            rescan = true;
                        }
                        parallel_control(ui, &mut parallel);
                    });
                    return;
                }
                endpoints_results(ui, &mut win, &mut add, &mut rescan, &mut parallel);
            });
        self.parallel_batches = parallel;
        if cancel && let Some(h) = win.handle.take() {
            h.cancel();
            win.progress.finish(Some("Cancelled".into()));
        }
        if background && win.handle.is_some() {
            win.visible = false;
            return Some(win);
        }
        if !open {
            if win.handle.is_some() {
                win.visible = false;
                app.toasts.info(format!(
                    "Still generating endpoints for '{}' in the background",
                    win.folder_name
                ));
                return Some(win);
            }
            return None;
        }
        if rescan {
            let folder_id = win.folder_id.clone();
            self.start_endpoints(app, &folder_id, true);
            return None;
        }
        if add {
            add_endpoints(app, &win);
            return None;
        }
        Some(win)
    }

    fn render_test_gen(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        if self.test_gen.as_ref().is_some_and(|w| w.hidden) {
            return; // berjalan di background; dibuka lagi dari panel sidebar
        }
        let Some(mut win) = self.test_gen.take() else {
            return;
        };
        let mut open = true;
        let mut generate = false;
        let mut cancel = false;
        let folders: Vec<(String, String, String, usize, bool)> =
            crate::http_collection::all_folders(&app.yaak_workspaces)
                .into_iter()
                .map(|(ws, f)| {
                    (
                        f.id.clone(),
                        ws.name.clone(),
                        f.name.clone(),
                        f.all_requests().len(),
                        f.has_repository(),
                    )
                })
                .collect();
        let running = win.progress.as_ref().is_some_and(|p| p.running);
        egui::Window::new("Generate Integration Tests")
            .id(egui::Id::new("http_repo_test_gen"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(560.0, 520.0))
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Pick one or more HTTP API folders; they can come from different \
                         repositories. The AI designs end-to-end flows across them, with \
                         variables, extracted ids and assertions. Nothing is sent until you run \
                         a suite.",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .id_salt("http_repo_test_gen_folders")
                    .max_height(240.0)
                    .show(ui, |ui| {
                        if folders.is_empty() {
                            small_weak(ui, "No HTTP API folders yet.");
                        }
                        for (id, ws, name, count, repo) in &folders {
                            let mut on = win.selected.contains(id);
                            ui.add_enabled_ui(!running && *count > 0, |ui| {
                                let git = if *repo {
                                    format!("  {}", egui_icons::icons::MDI_GIT.codepoint)
                                } else {
                                    String::new()
                                };
                                let text = format!("{ws} / {name}  ({count} request(s)){git}");
                                if ui.checkbox(&mut on, text).changed() {
                                    if on {
                                        win.selected.insert(id.clone());
                                    } else {
                                        win.selected.remove(id);
                                    }
                                }
                            });
                        }
                    });
                ui.add_space(6.0);
                ui.label("Focus (optional)");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::multiline(&mut win.instructions)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text(
                            "e.g. checkout flow from cart to payment, include auth failures",
                        ),
                );
                ui.add_space(6.0);
                if let Some(p) = &win.progress {
                    p.show(ui);
                    if let Some(e) = &p.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if running {
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                        if ui
                            .button("Process in Background")
                            .on_hover_text(
                                "Hide this window and keep working. Follow it in Background \
                                 Processes at the bottom of the sidebar.",
                            )
                            .clicked()
                        {
                            win.hidden = true;
                        }
                    } else if ui
                        .add_enabled(
                            !win.selected.is_empty(),
                            crate::diagram_repo::accent_button(ui, "Generate Tests"),
                        )
                        .clicked()
                    {
                        generate = true;
                    }
                    if ui.button("Open Integration Tests…").clicked() {
                        request(ui.ctx(), RepoAction::OpenSuites);
                    }
                });
            });
        if cancel && let Some(h) = win.handle.take() {
            h.cancel();
            if let Some(p) = win.progress.as_mut() {
                p.finish(Some("Cancelled".into()));
            }
        }
        if !open {
            if let Some(h) = win.handle.take() {
                h.cancel();
            }
            return;
        }
        if generate {
            let (backend, label, note) = chat_backend(app);
            match backend {
                None => app.toasts.error(note.unwrap_or_else(|| {
                    "No AI backend is ready. Configure one in the AI Assistant panel.".into()
                })),
                Some(backend) => {
                    let catalogs: Vec<crate::http_tests::FolderCatalog> =
                        crate::http_collection::all_folders(&app.yaak_workspaces)
                            .into_iter()
                            .filter(|(_, f)| win.selected.contains(&f.id))
                            .map(|(_, f)| crate::http_tests::FolderCatalog::from_folder(f))
                            .collect();
                    win.progress = Some(JobProgress::start());
                    win.handle = Some(crate::http_tests::spawn_generate(
                        backend,
                        label,
                        catalogs,
                        win.instructions.clone(),
                    ));
                }
            }
        }
        self.test_gen = Some(win);
    }

    fn render_suites(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        let Some(mut win) = self.suites.take() else {
            return;
        };
        let mut open = true;
        let mut start_run: Option<String> = None;
        let mut open_request: Option<crate::http_collection::SavedRequest> = None;
        egui::Window::new("Integration Tests")
            .id(egui::Id::new("http_repo_suites"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(880.0, 600.0))
            .show(ctx, |ui| {
                if win.suites.is_empty() {
                    small_weak(
                        ui,
                        "No integration tests yet. Right-click an HTTP API folder and choose \
                         Generate Integration Tests (AI)….",
                    );
                    return;
                }
                ui.horizontal_top(|ui| {
                    suites_list(ui, &mut win);
                    ui.separator();
                    ui.vertical(|ui| suite_detail(ui, &mut win, &mut start_run, &mut open_request));
                });
            });

        if let Some(id) = confirm_run_dialog(ctx, &mut win) {
            start_run = Some(id);
        }
        confirm_remove_dialog(app, ctx, &mut win);

        // Simpan perubahan (nama, variabel, langkah aktif) saat input dilepas.
        if win.dirty && !ctx.egui_wants_keyboard_input() {
            for s in &win.suites {
                if let Err(e) = crate::http_tests::save_suite(s) {
                    app.toasts.error(e);
                    break;
                }
            }
            win.dirty = false;
        }
        if let Some(id) = start_run
            && let Some(suite) = win.suites.iter().find(|s| s.id == id).cloned()
        {
            if win.dirty {
                if let Err(e) = crate::http_tests::save_suite(&suite) {
                    app.toasts.error(e);
                }
                win.dirty = false;
            }
            win.results
                .insert(id.clone(), vec![None; suite.steps.len()]);
            win.expanded.clear();
            win.run = Some(RunState {
                suite_id: id,
                handle: crate::http_tests::spawn_run(suite),
                current: None,
            });
        }
        if let Some(req) = open_request {
            let vars = win
                .selected
                .as_ref()
                .and_then(|id| win.suites.iter().find(|s| &s.id == id))
                .map(crate::http_tests::initial_vars)
                .unwrap_or_default();
            let resolved = crate::http_tests::resolve_request(&req, &vars);
            crate::sidebar_collection::apply_collection_request_to_active_tab(app, &resolved);
        }
        if !open {
            if let Some(run) = win.run.take() {
                run.handle.cancel();
            }
            return;
        }
        self.suites = Some(win);
    }

    fn render_links(&mut self, app: &mut Tabular, ctx: &egui::Context) {
        let Some(popup) = self.links.take() else {
            return;
        };
        let mut open = true;
        let mut open_group_ref: Option<GroupRef> = None;
        let mut reveal: Option<String> = None;
        // Sertakan id dan nama koneksi: dua koneksi bisa menunjuk database yang sama.
        let group_labels: Vec<String> = popup
            .groups
            .iter()
            .map(|g| {
                let conn_name = app
                    .connections
                    .iter()
                    .find(|c| c.id == Some(g.conn_id))
                    .map(|c| c.name.as_str())
                    .unwrap_or("unknown connection");
                format!(
                    "#{} {} · {} · {}",
                    g.conn_id, conn_name, g.db_name, g.group_title
                )
            })
            .collect();
        egui::Window::new(&popup.title)
            .id(egui::Id::new("http_repo_links_popup"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                for (g, label) in popup.groups.iter().zip(&group_labels) {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        if ui.small_button("Open diagram").clicked() {
                            open_group_ref = Some(g.clone());
                        }
                    });
                }
                for f in &popup.folders {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} / {}", f.workspace_name, f.folder_name));
                        if ui.small_button("Show").clicked() {
                            reveal = Some(f.folder_id.clone());
                        }
                    });
                }
            });
        if let Some(g) = open_group_ref {
            open_group(app, &g);
            return;
        }
        if let Some(id) = reveal {
            reveal_folder(app, &id);
            return;
        }
        if open {
            self.links = Some(popup);
        }
    }
}

/// Pilihan jumlah batch AI paralel; berlaku untuk generate berikutnya.
fn parallel_control(ui: &mut egui::Ui, parallel: &mut usize) {
    ui.label("Parallel AI batches");
    ui.add(egui::DragValue::new(parallel).range(1..=crate::repo_endpoints::MAX_PARALLEL_BATCHES))
        .on_hover_text(
            "How many batches of 20 endpoints the AI documents at the same time on the next \
             run. Higher is faster but uses more of your AI quota; all folders together run at \
             most 6 AI turns at once.",
        );
}

fn endpoints_results(
    ui: &mut egui::Ui,
    win: &mut EndpointWindow,
    add: &mut bool,
    rescan: &mut bool,
    parallel: &mut usize,
) {
    if let Some(note) = &win.note {
        ui.label(
            egui::RichText::new(format!(
                "{} {note}",
                egui_icons::icons::ICON_WARNING.codepoint
            ))
            .small()
            .color(ui.visuals().warn_fg_color),
        );
    }
    let total = win.rows.len();
    let ai = win.rows.iter().filter(|r| r.endpoint.from_ai).count();
    let exists = win.rows.iter().filter(|r| r.exists).count();
    small_weak(
        ui,
        format!(
            "{total} endpoint(s) in {} file(s) · {ai} documented by AI · {} from text search \
             only · {exists} already in this folder",
            win.files_scanned,
            total - ai
        ),
    );
    egui::CollapsingHeader::new("Progress")
        .id_salt("http_repo_ep_progress")
        .default_open(false)
        .show(ui, |ui| win.progress.show(ui));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Base URL");
        ui.add(
            egui::TextEdit::singleline(&mut win.base_url)
                .desired_width(260.0)
                .hint_text("http://localhost:3000"),
        );
        ui.checkbox(&mut win.subfolders, "Group into sub-folders by resource");
    });
    let label = if win.linked_groups > 0 {
        format!(
            "Link endpoints to diagram tables ({} linked group(s))",
            win.linked_groups
        )
    } else {
        "Link endpoints to diagram tables (no diagram group uses this repository yet)".to_string()
    };
    ui.add_enabled(
        win.key.is_some(),
        egui::Checkbox::new(&mut win.link_diagrams, label),
    );
    if !cfg!(target_os = "ios") {
        ui.indent("http_repo_also_flows", |ui| {
            ui.add_enabled(
                win.key.is_some() && win.link_diagrams,
                egui::Checkbox::new(&mut win.also_flows, "Also generate business process"),
            )
            .on_hover_text(
                "After linking, trace each endpoint step by step with AI and show it as a \
                 process card in the diagrams that are open now. Other diagrams only get the \
                 links; generate their process from the diagram later.",
            );
        });
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        style::render_search_field(ui, &mut win.filter, "Filter by path, name or table…", 280.0);
        if ui.small_button("Select all").clicked() {
            win.rows.iter_mut().for_each(|r| r.checked = true);
        }
        if ui.small_button("Select none").clicked() {
            win.rows.iter_mut().for_each(|r| r.checked = false);
        }
        if ui.small_button("New only").clicked() {
            win.rows.iter_mut().for_each(|r| r.checked = !r.exists);
        }
    });
    ui.separator();
    let filter = win.filter.to_lowercase();
    let accent = style::theme_accent(ui.ctx());
    // Footer & detail digambar dari bawah lebih dulu, lalu daftar mengisi sisa ruang persis.
    // Menebak tinggi footer membuat konten sedikit meluap tiap frame sehingga jendela
    // resizable terus memanjang sampai setinggi layar.
    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        ui.horizontal(|ui| {
            let n = win.rows.iter().filter(|r| r.checked).count();
            if ui.button("Rescan").clicked() {
                *rescan = true;
            }
            parallel_control(ui, parallel);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        n > 0 && !win.base_url.trim().is_empty(),
                        crate::diagram_repo::accent_button(ui, format!("Add {n} endpoint(s)")),
                    )
                    .clicked()
                {
                    *add = true;
                }
            });
        });
        ui.separator();
        if let Some(ep) = win
            .selected
            .and_then(|i| win.rows.get(i))
            .map(|r| &r.endpoint)
        {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 180.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("http_repo_ep_detail")
                        .max_height(180.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| endpoint_details(ui, ep));
                },
            );
            ui.separator();
        }
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("http_repo_ep_list")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if win.rows.is_empty() {
                        small_weak(ui, "No endpoints were found.");
                    }
                    for (i, row) in win.rows.iter_mut().enumerate() {
                        let ep = &row.endpoint;
                        if !filter.is_empty()
                            && !ep.path.to_lowercase().contains(&filter)
                            && !ep.name.to_lowercase().contains(&filter)
                            && !ep.method.to_lowercase().contains(&filter)
                            && !ep.tables.iter().any(|t| t.to_lowercase().contains(&filter))
                        {
                            continue;
                        }
                        let resp = ui
                            .horizontal(|ui| {
                                ui.checkbox(&mut row.checked, "");
                                method_chip(ui, &ep.method);
                                ui.label(
                                    egui::RichText::new(&ep.path)
                                        .family(egui::FontFamily::Monospace)
                                        .size(12.0),
                                );
                                if !ep.name.is_empty() {
                                    small_weak(ui, &ep.name);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let (tag, color) = if ep.from_ai {
                                            ("AI", ui.visuals().hyperlink_color)
                                        } else {
                                            ("TEXT", ui.visuals().weak_text_color())
                                        };
                                        ui.label(
                                            egui::RichText::new(tag).small().strong().color(color),
                                        );
                                        if row.exists {
                                            ui.label(
                                            egui::RichText::new("exists")
                                                .small()
                                                .color(ui.visuals().warn_fg_color),
                                        )
                                        .on_hover_text(
                                            "Checked rows that already exist are updated in place",
                                        );
                                        }
                                        if !ep.tables.is_empty() {
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "{} {}",
                                                    egui_icons::icons::ICON_TABLE.codepoint,
                                                    ep.tables.join(", ")
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                        }
                                    },
                                );
                            })
                            .response
                            .interact(egui::Sense::click());
                        if resp.clicked() {
                            win.selected = if win.selected == Some(i) {
                                None
                            } else {
                                Some(i)
                            };
                        }
                        if win.selected == Some(i) {
                            ui.painter().rect_stroke(
                                resp.rect.expand(1.0),
                                3.0,
                                egui::Stroke::new(1.0, accent),
                                egui::StrokeKind::Outside,
                            );
                        }
                    }
                });
        });
    });
}

fn add_endpoints(app: &mut Tabular, win: &EndpointWindow) {
    let Some((ws, _)) =
        crate::http_collection::find_workspace_folder(&app.yaak_workspaces, &win.folder_id)
    else {
        app.toasts.error("The HTTP folder no longer exists");
        return;
    };
    let ws_id = ws.id.clone();
    let eps: Vec<GeneratedEndpoint> = win
        .rows
        .iter()
        .filter(|r| r.checked)
        .map(|r| r.endpoint.clone())
        .collect();
    let stats = match crate::repo_endpoints::add_endpoints_to_folder(
        &mut app.yaak_workspaces,
        &ws_id,
        &win.folder_id,
        &eps,
        &win.base_url,
        win.subfolders,
    ) {
        Ok(s) => s,
        Err(e) => {
            app.toasts.error(e);
            return;
        }
    };
    if let Err(e) = save_workspaces(&app.yaak_workspaces) {
        app.toasts.error(e);
    }
    let mut msg = format!(
        "Added {} and updated {} endpoint(s) in '{}'.",
        stats.added, stats.updated, win.folder_name
    );
    if win.link_diagrams
        && let Some(key) = win.key.as_deref()
    {
        let links: Vec<EndpointTables> = stats
            .requests
            .iter()
            .filter(|r| !r.tables.is_empty())
            .map(EndpointTables::from_request)
            .collect();
        let report = link_to_diagrams(app, key, &links);
        msg.push_str(&report.summary());
        if win.also_flows {
            generate_flows_in_open_diagrams(app, key);
        }
    }
    reveal_folder(app, &win.folder_id);
    log::info!("[HTTP_REPO] {msg}");
    app.toasts.success(msg);
}

fn suites_list(ui: &mut egui::Ui, win: &mut SuitesWindow) {
    ui.vertical(|ui| {
        ui.set_width(220.0);
        egui::ScrollArea::vertical()
            .id_salt("http_repo_suite_list")
            .show(ui, |ui| {
                let mut pick = None;
                for s in &win.suites {
                    let (pass, fail) = win.results.get(&s.id).map_or((0, 0), |r| {
                        let done: Vec<&StepResult> = r.iter().flatten().collect();
                        (
                            done.iter().filter(|x| x.passed()).count(),
                            done.iter().filter(|x| !x.passed() && !x.skipped).count(),
                        )
                    });
                    let mut text = format!("{}\n{} step(s)", s.name, s.steps.len());
                    if pass + fail > 0 {
                        text.push_str(&format!(" · {pass} passed · {fail} failed"));
                    }
                    let selected = win.selected.as_deref() == Some(s.id.as_str());
                    if ui.selectable_label(selected, text).clicked() {
                        pick = Some(s.id.clone());
                    }
                }
                if let Some(id) = pick {
                    win.selected = Some(id);
                    win.expanded.clear();
                }
            });
    });
}

fn step_icon(ui: &mut egui::Ui, running_here: bool, result: Option<&StepResult>) {
    use egui_icons::icons as i;
    if running_here && result.is_none() {
        ui.add(egui::Spinner::new().size(12.0));
        return;
    }
    let (glyph, color) = match result {
        Some(r) if r.skipped => (i::ICON_SKIP_NEXT.codepoint, ui.visuals().weak_text_color()),
        Some(r) if r.passed() => (
            i::ICON_CHECK_CIRCLE.codepoint,
            egui::Color32::from_rgb(76, 175, 80),
        ),
        Some(_) => (i::ICON_CANCEL.codepoint, ui.visuals().error_fg_color),
        None => (
            i::ICON_RADIO_BUTTON_UNCHECKED.codepoint,
            ui.visuals().weak_text_color(),
        ),
    };
    ui.label(egui::RichText::new(glyph).color(color));
}

fn suite_detail(
    ui: &mut egui::Ui,
    win: &mut SuitesWindow,
    start_run: &mut Option<String>,
    open_request: &mut Option<crate::http_collection::SavedRequest>,
) {
    use egui_icons::icons as i;
    let Some(idx) = win
        .selected
        .as_ref()
        .and_then(|id| win.suites.iter().position(|s| &s.id == id))
    else {
        small_weak(ui, "Select a suite.");
        return;
    };
    let running_id = win.run.as_ref().map(|r| r.suite_id.clone());
    let current_step = win.run.as_ref().and_then(|r| r.current);
    let suite_id = win.suites[idx].id.clone();
    let is_running = running_id.as_deref() == Some(suite_id.as_str());
    let results = win.results.get(&suite_id).cloned().unwrap_or_default();
    let mut stop = false;
    let mut confirm_run = None;
    let mut confirm_remove = None;
    let mut dirty = false;
    let suite = &mut win.suites[idx];
    ui.horizontal(|ui| {
        if ui
            .add(
                egui::TextEdit::singleline(&mut suite.name)
                    .desired_width((ui.available_width() - 200.0).max(120.0))
                    .font(egui::TextStyle::Heading),
            )
            .changed()
        {
            dirty = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if is_running {
                if ui
                    .button(format!("{} Stop", i::ICON_STOP.codepoint))
                    .clicked()
                {
                    stop = true;
                }
            } else if ui
                .add_enabled(
                    running_id.is_none(),
                    crate::diagram_repo::accent_button(
                        ui,
                        format!("{} Run", i::ICON_PLAY_ARROW.codepoint),
                    ),
                )
                .clicked()
            {
                if suite.mutating_steps() > 0 {
                    confirm_run = Some(suite_id.clone());
                } else {
                    *start_run = Some(suite_id.clone());
                }
            }
            if ui
                .add_enabled(!is_running, egui::Button::new(i::ICON_DELETE.codepoint))
                .on_hover_text("Remove this suite")
                .clicked()
            {
                confirm_remove = Some(suite_id.clone());
            }
        });
    });
    if !suite.description.is_empty() {
        small_weak(ui, &suite.description);
    }
    ui.add_space(4.0);
    egui::CollapsingHeader::new(format!("Variables ({})", suite.variables.len()))
        .id_salt(("suite_vars", &suite_id))
        .default_open(true)
        .show(ui, |ui| {
            let mut remove_var = None;
            egui::Grid::new(("suite_vars_grid", &suite_id))
                .num_columns(3)
                .spacing([6.0, 4.0])
                .show(ui, |ui| {
                    for (i, (k, v)) in suite.variables.iter_mut().enumerate() {
                        dirty |= ui
                            .add(egui::TextEdit::singleline(k).desired_width(180.0))
                            .changed();
                        let secret = ["token", "password", "secret", "key"]
                            .iter()
                            .any(|s| k.to_ascii_lowercase().contains(s));
                        dirty |= ui
                            .add(
                                egui::TextEdit::singleline(v)
                                    .password(secret)
                                    .desired_width(300.0),
                            )
                            .changed();
                        if ui.small_button("✖").clicked() {
                            remove_var = Some(i);
                        }
                        ui.end_row();
                    }
                });
            if let Some(i) = remove_var {
                suite.variables.remove(i);
                dirty = true;
            }
            ui.horizontal(|ui| {
                if ui.small_button("➕ Add variable").clicked() {
                    suite.variables.push((String::new(), String::new()));
                    dirty = true;
                }
                small_weak(ui, "{{run_id}} is set automatically for each run.");
            });
        });
    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt(("suite_steps", &suite_id))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, step) in suite.steps.iter_mut().enumerate() {
                let result = results.get(i).and_then(|r| r.as_ref());
                let resp = ui
                    .horizontal(|ui| {
                        dirty |= ui
                            .add_enabled(!is_running, egui::Checkbox::new(&mut step.enabled, ""))
                            .changed();
                        step_icon(ui, is_running && current_step == Some(i), result);
                        small_weak(ui, format!("{}.", i + 1));
                        method_chip(ui, step.request.method.label());
                        ui.label(egui::RichText::new(&step.name).strong());
                        ui.label(
                            egui::RichText::new(&step.request.url)
                                .family(egui::FontFamily::Monospace)
                                .size(11.0)
                                .weak(),
                        );
                        if let Some(r) = result.filter(|r| !r.skipped) {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    small_weak(ui, format!("{} ms", r.time_ms));
                                    let status = if r.error.is_some() {
                                        "error".to_string()
                                    } else {
                                        r.status.to_string()
                                    };
                                    ui.label(egui::RichText::new(status).small().strong());
                                },
                            );
                        }
                    })
                    .response
                    .interact(egui::Sense::click());
                if resp.clicked() && !win.expanded.remove(&i) {
                    win.expanded.insert(i);
                }
                if win.expanded.contains(&i) {
                    ui.indent(("step_detail", i), |ui| {
                        step_details(ui, step, result);
                        if ui.small_button("Open in HTTP client").clicked() {
                            *open_request = Some(step.request.clone());
                        }
                    });
                    ui.add_space(4.0);
                }
            }
        });
    if stop && let Some(run) = &win.run {
        run.handle.cancel();
    }
    if confirm_run.is_some() {
        win.confirm_run = confirm_run;
    }
    if confirm_remove.is_some() {
        win.confirm_remove = confirm_remove;
    }
    win.dirty |= dirty;
}

/// Konfirmasi sebelum request yang mengubah data dikirim. Mengembalikan id
/// suite yang boleh dijalankan.
fn confirm_run_dialog(ctx: &egui::Context, win: &mut SuitesWindow) -> Option<String> {
    let id = win.confirm_run.clone()?;
    let Some(suite) = win.suites.iter().find(|s| s.id == id) else {
        win.confirm_run = None;
        return None;
    };
    let mut decided: Option<bool> = None;
    let targets: Vec<String> = suite
        .variables
        .iter()
        .filter(|(k, _)| k.starts_with("base_url"))
        .map(|(k, v)| format!("{k} = {v}"))
        .collect();
    style::render_modal_backdrop(ctx, "http_repo_confirm_run", true);
    egui::Window::new("Run integration tests?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!(
                "This run sends {} request(s) that create, change or remove data.",
                suite.mutating_steps()
            ));
            if !targets.is_empty() {
                ui.label("Target servers:");
                for t in &targets {
                    ui.label(egui::RichText::new(t).family(egui::FontFamily::Monospace));
                }
            }
            ui.label(
                egui::RichText::new("Use a development or test environment, not production.")
                    .color(ui.visuals().warn_fg_color),
            );
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                if ui
                    .add(crate::diagram_repo::accent_button(ui, "Run"))
                    .clicked()
                {
                    decided = Some(true);
                }
            });
        });
    match decided {
        Some(yes) => {
            win.confirm_run = None;
            yes.then_some(id)
        }
        None => None,
    }
}

fn confirm_remove_dialog(app: &mut Tabular, ctx: &egui::Context, win: &mut SuitesWindow) {
    let Some(id) = win.confirm_remove.clone() else {
        return;
    };
    let mut decided: Option<bool> = None;
    let name = win
        .suites
        .iter()
        .find(|s| s.id == id)
        .map(|s| s.name.clone())
        .unwrap_or_default();
    egui::Window::new("Remove test suite?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.label(format!("Remove \"{name}\"? This cannot be undone."));
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
                if ui.button("Remove").clicked() {
                    decided = Some(true);
                }
            });
        });
    let Some(yes) = decided else {
        return;
    };
    win.confirm_remove = None;
    if !yes {
        return;
    }
    match crate::http_tests::remove_suite(&id) {
        Ok(()) => {
            win.suites.retain(|s| s.id != id);
            win.results.remove(&id);
            win.selected = win.suites.first().map(|s| s.id.clone());
        }
        Err(e) => app.toasts.error(e),
    }
}

fn param_lines(ui: &mut egui::Ui, title: &str, params: &[crate::repo_endpoints::ParamSpec]) {
    if params.is_empty() {
        return;
    }
    ui.label(egui::RichText::new(title).small().strong());
    for p in params {
        let mut line = format!("  {}", p.name);
        if !p.example.is_empty() {
            line.push_str(&format!(" = {}", p.example));
        }
        if p.required {
            line.push_str("  (required)");
        }
        if !p.description.is_empty() {
            line.push_str(&format!("  — {}", p.description));
        }
        ui.label(
            egui::RichText::new(line)
                .family(egui::FontFamily::Monospace)
                .size(11.0),
        );
    }
}

fn code_block(ui: &mut egui::Ui, title: &str, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    ui.label(egui::RichText::new(title).small().strong());
    ui.label(
        egui::RichText::new(crate::repo_scan::truncate_chars(text, 4_000))
            .family(egui::FontFamily::Monospace)
            .size(11.0),
    );
}

fn endpoint_details(ui: &mut egui::Ui, ep: &GeneratedEndpoint) {
    ui.horizontal(|ui| {
        method_chip(ui, &ep.method);
        ui.label(
            egui::RichText::new(&ep.path)
                .family(egui::FontFamily::Monospace)
                .strong(),
        );
        if !ep.source.is_empty() {
            small_weak(ui, &ep.source);
        }
    });
    if !ep.description.is_empty() {
        ui.label(&ep.description);
    }
    let mut facts = Vec::new();
    if !ep.auth.is_empty() {
        facts.push(format!("auth: {}", ep.auth));
    }
    if !ep.body_type.is_empty() {
        facts.push(format!("body: {}", ep.body_type));
    }
    if !ep.status_codes.is_empty() {
        facts.push(format!("status: {}", ep.status_codes.join(", ")));
    }
    if !ep.tables.is_empty() {
        facts.push(format!("tables: {}", ep.tables.join(", ")));
    }
    if !facts.is_empty() {
        small_weak(ui, facts.join(" · "));
    }
    param_lines(ui, "Path parameters", &ep.path_params);
    param_lines(ui, "Query parameters", &ep.query_params);
    param_lines(ui, "Headers", &ep.headers);
    param_lines(ui, "Form fields", &ep.form_fields);
    code_block(ui, "Body example", &ep.body_example);
    code_block(ui, "Response example", &ep.response_example);
}

fn step_details(
    ui: &mut egui::Ui,
    step: &crate::http_tests::HttpTestStep,
    result: Option<&StepResult>,
) {
    if !step.request.body_text.trim().is_empty() {
        code_block(ui, "Body", &step.request.body_text);
    }
    if !step.extract.is_empty() {
        ui.label(egui::RichText::new("Extract").small().strong());
        for e in &step.extract {
            let value = result
                .and_then(|r| r.extracted.iter().find(|(k, _)| *k == e.var))
                .map(|(_, v)| format!(" = {}", crate::repo_scan::truncate_chars(v, 80)))
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!("  {} from {}{value}", e.var, e.from))
                    .family(egui::FontFamily::Monospace)
                    .size(11.0),
            );
        }
    }
    ui.label(egui::RichText::new("Assertions").small().strong());
    if step.assertions.is_empty() {
        small_weak(ui, "  (none)");
    }
    for (i, a) in step.assertions.iter().enumerate() {
        let res = result.and_then(|r| r.assertions.get(i));
        let (mark, color) = match res {
            Some(r) if r.passed => ("✅", egui::Color32::from_rgb(76, 175, 80)),
            Some(_) => ("❌", ui.visuals().error_fg_color),
            None => ("•", ui.visuals().weak_text_color()),
        };
        let actual = res
            .filter(|r| !r.passed)
            .and_then(|r| r.actual.as_ref())
            .map(|v| format!("  (actual: {})", crate::repo_scan::truncate_chars(v, 80)))
            .unwrap_or_default();
        ui.label(
            egui::RichText::new(format!("  {mark} {}{actual}", a.describe()))
                .size(11.5)
                .color(color),
        );
    }
    if let Some(r) = result {
        if let Some(e) = &r.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        if !r.unresolved.is_empty() {
            ui.label(
                egui::RichText::new(format!(
                    "Variables without a value: {}",
                    r.unresolved.join(", ")
                ))
                .small()
                .color(ui.visuals().warn_fg_color),
            );
        }
        if !r.url.is_empty() {
            small_weak(ui, format!("{} {}", r.method, r.url));
        }
        code_block(ui, "Response", &r.body_preview);
    }
}
