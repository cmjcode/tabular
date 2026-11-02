//! Repository setup di GUI: dari satu folder git, bentuk project, koneksi
//! database per environment, folder HTTP API per service (berisi endpoint
//! hasil pemindaian route), dan group diagram per service; lalu jaga
//! semuanya tetap sinkron dengan repository lewat watcher latar.
//!
//! Discovery berjalan di thread ([`crate::repo_setup::discover`]); hasilnya
//! diterapkan di UI thread karena menyentuh state `Tabular`. Penerapan
//! bersifat idempoten: menjalankan ulang plan yang sama tidak membuat
//! duplikat. Item yang hilang dari repository hanya ditandai *stale*, tidak
//! pernah dihapus otomatis. Diagram hanya disimpan ke file lokal; penyimpanan
//! ke tabel `diagram_by_tabular` di database target tetap menunggu user.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;

use super::background_tasks::{Snapshot, TaskId, TaskOwner};
use crate::agent::harness::{ProgressStatus, ProgressStep};
use crate::models::enums::DatabaseType;
use crate::models::structs::DiagramState;
use crate::project::{self, EnvVar, Project, ProjectEnv};
use crate::repo_setup::diagram_groups::{self, GroupPatch, ServiceTables};
use crate::repo_setup::store::{
    RepoSetup, RepoSetupStore, TrackedConnection, TrackedDiagram, TrackedService,
};
use crate::repo_setup::{self, PlannedConnection, SetupPlan, diff, dsn_detect, env_files};
use crate::window_egui::Tabular;

/// Jeda setelah aplikasi dibuka sebelum watcher pertama kali memeriksa.
const STARTUP_DELAY: Duration = Duration::from_secs(20);
/// Watcher menilai jadwal paling sering sekali per jeda ini.
const POLL_EVERY: Duration = Duration::from_secs(15);
/// Pemeriksaan karena jendela kembali fokus paling sering sekali per menit.
const FOCUS_MIN_GAP: Duration = Duration::from_secs(60);
/// Batas tunggu skema diagram sebelum group per service dibatalkan.
const DIAGRAM_WAIT: Duration = Duration::from_secs(180);
/// Pemeriksaan latar yang tidak selesai selama ini dianggap macet (mis.
/// `git` menunggu `index.lock`) dan dilepas supaya watcher tetap jalan.
const WATCH_TIMEOUT: Duration = Duration::from_secs(300);
/// Jeda pemeriksaan diagram yang menunggu skema (membaca file/tab).
const PENDING_EVERY: Duration = Duration::from_secs(1);

enum SetupEvent {
    Planned {
        job: u64,
        plan: Box<SetupPlan>,
    },
    Unchanged {
        job: u64,
    },
    Failed {
        job: u64,
        error: String,
    },
    Tables {
        pending: u64,
        /// `None`: skema belum bisa dibaca, coba lagi nanti.
        outcome: Option<Box<TablesOutcome>>,
    },
}

/// Hasil pembentukan group per service yang dihitung di thread latar.
struct TablesOutcome {
    sync: diagram_groups::GroupSync,
    patch: GroupPatch,
    /// Tag `service|tabel` yang baru ditempatkan.
    newly_placed: Vec<String>,
}

/// Asal state diagram yang skemanya sudah tersedia.
enum DiagramSource {
    /// Salinan state tab diagram yang terbuka.
    Tab(Box<DiagramState>),
    /// File cache lokal; dibaca di thread karena bisa berukuran megabyte.
    File(PathBuf),
}

/// Satu discovery yang berjalan (manual atau watcher).
struct Job {
    id: u64,
    path: PathBuf,
    key: Option<String>,
    manual: bool,
    started: Instant,
    cancel: Arc<AtomicBool>,
    task: Option<TaskId>,
    title: String,
    subtitle: String,
    steps: Vec<ProgressStep>,
    result: Option<Result<String, String>>,
    /// Nama project pilihan user (dialog Add Connection > Git Folder).
    project: Option<String>,
}

/// Diagram yang menunggu skema untuk dibentuk group per service.
struct PendingDiagram {
    id: u64,
    key: String,
    conn_id: i64,
    db: String,
    /// `(nama service, folder absolut)`.
    services: Vec<(String, String)>,
    repo_url: Option<String>,
    since: Instant,
    scanning: bool,
    announce: bool,
}

pub struct RepoSetupState {
    pub store: RepoSetupStore,
    tx: mpsc::Sender<SetupEvent>,
    rx: mpsc::Receiver<SetupEvent>,
    jobs: Vec<Job>,
    pending: Vec<PendingDiagram>,
    next_id: u64,
    last_check: HashMap<String, Instant>,
    last_poll: Option<Instant>,
    last_pending_check: Option<Instant>,
    started: Instant,
    was_focused: bool,
    /// Plan setup manual yang menunggu persetujuan (mode review).
    pub review: Option<Box<SetupPlan>>,
    /// Nama project pilihan user untuk plan yang sedang ditinjau.
    review_project: Option<String>,
    /// Isi form "Git Folder" di dialog Add New Connection.
    pub git_form: GitFolderForm,
    /// Perubahan dari watcher yang menunggu persetujuan (mode review).
    pub changes: HashMap<String, Box<SetupPlan>>,
    /// Kunci repository yang jendela detailnya terbuka.
    pub details: Option<String>,
}

impl Default for RepoSetupState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            store: RepoSetupStore::default(),
            tx,
            rx,
            jobs: Vec::new(),
            pending: Vec::new(),
            next_id: 0,
            last_check: HashMap::new(),
            last_poll: None,
            last_pending_check: None,
            started: Instant::now(),
            was_focused: true,
            review: None,
            review_project: None,
            git_form: GitFolderForm::default(),
            changes: HashMap::new(),
            details: None,
        }
    }
}

impl RepoSetupState {
    fn next(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Repository `key` sudah di-setup.
    pub fn is_set_up(&self, key: &str) -> bool {
        self.store.get(key).is_some()
    }

    /// Ada discovery untuk `key` yang sedang berjalan.
    pub fn is_running(&self, key: &str) -> bool {
        self.jobs
            .iter()
            .any(|j| j.result.is_none() && j.key.as_deref() == Some(key))
    }

    /// Proses diagram per koneksi yang sedang berjalan, untuk indikator
    /// di sidebar: `(id koneksi, keterangan)`.
    pub fn diagram_activity(&self) -> impl Iterator<Item = (i64, String)> + '_ {
        self.pending.iter().map(|p| {
            let text = if p.scanning {
                format!("Grouping tables of '{}' by service", p.db)
            } else {
                format!("Waiting for the schema of '{}'", p.db)
            };
            (p.conn_id, text)
        })
    }

    fn save(&self) -> Option<String> {
        self.store
            .save()
            .err()
            .map(|e| format!("Could not save repository setup: {e}"))
    }
}

fn active_step(text: &str) -> ProgressStep {
    ProgressStep {
        step_index: Some(1),
        description: text.to_string(),
        detail: None,
        status: ProgressStatus::Active,
        tool_name: None,
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

// ─── Titik masuk ────────────────────────────────────────────────────────────

/// Isi mode "Git Folder" di dialog Add New Connection.
#[derive(Debug, Clone, Default)]
pub struct GitFolderForm {
    /// Mode Git Folder sedang dipilih (bukan koneksi database biasa).
    pub active: bool,
    pub path: String,
    /// Nama project: pilih yang ada atau ketik nama baru.
    pub project: String,
    pub error: Option<String>,
}

/// Validasi form; hasilnya `(folder, nama project)`.
fn validate_git_form(t: &Tabular) -> Result<(PathBuf, String), String> {
    let form = &t.repo_setup.git_form;
    let raw = form.path.trim();
    if raw.is_empty() {
        return Err("Choose the git folder".into());
    }
    let dir = crate::repo_scan::expand_home(raw);
    if !dir.is_dir() {
        return Err(format!("Folder {} does not exist", dir.display()));
    }
    if crate::git::ops::discover_root(&dir).is_err() {
        return Err(format!("{} is not inside a git repository", dir.display()));
    }
    let name = form.project.trim().to_string();
    if name.is_empty() {
        return Err("Pick a project or type a new project name".into());
    }
    let exists = t
        .projects
        .list
        .iter()
        .any(|p| p.name.eq_ignore_ascii_case(&name) && !p.is_read_only());
    if !exists {
        project::validate_name(&name, &t.projects.list, None).map_err(|e| e.to_string())?;
    }
    Ok((dir, name))
}

/// Gambar mode "Git Folder" di dialog Add New Connection: folder repository
/// dan nama project (yang ada atau baru). Mengembalikan `true` bila setup
/// dimulai sehingga dialog boleh ditutup.
pub fn render_git_folder_form(t: &mut Tabular, ui: &mut egui::Ui) -> bool {
    super::project_ui::ensure_loaded(t);
    let projects: Vec<String> = t
        .projects
        .list
        .iter()
        .filter(|p| !p.is_read_only())
        .map(|p| p.name.clone())
        .collect();
    if t.repo_setup.git_form.project.is_empty()
        && let Some(p) = super::project_ui::active(t)
    {
        t.repo_setup.git_form.project = p.name.clone();
    }
    let mut submit = false;
    let mut browse = false;
    {
        let form = &mut t.repo_setup.git_form;
        crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
            egui::Grid::new("git_folder_form")
                .num_columns(2)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Git Folder:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut form.path)
                                .hint_text("Folder inside a git repository")
                                .desired_width(200.0),
                        );
                        if ui
                            .button(egui_icons::icons::ICON_FOLDER_OPEN.codepoint)
                            .on_hover_text("Browse")
                            .clicked()
                        {
                            browse = true;
                        }
                    });
                    ui.end_row();

                    ui.label("Project Name:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut form.project)
                                .hint_text("Pick a project or type a new name")
                                .desired_width(200.0),
                        );
                        egui::ComboBox::from_id_salt("git_folder_form_project")
                            .selected_text("")
                            .width(24.0)
                            .show_ui(ui, |ui| {
                                if projects.is_empty() {
                                    ui.label("No projects yet");
                                }
                                for name in &projects {
                                    if ui
                                        .selectable_label(
                                            form.project.eq_ignore_ascii_case(name),
                                            name,
                                        )
                                        .clicked()
                                    {
                                        form.project = name.clone();
                                    }
                                }
                            });
                    });
                    ui.end_row();

                    ui.label("");
                    let typed = form.project.trim();
                    if typed.is_empty() {
                        ui.label("");
                    } else if projects.iter().any(|p| p.eq_ignore_ascii_case(typed)) {
                        ui.label(egui::RichText::new("Uses the existing project").weak());
                    } else {
                        ui.label(
                            egui::RichText::new("A new project with this name will be created")
                                .weak(),
                        );
                    }
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "Tabular reads .env, docker-compose and config files to create the database connections, \
                     scans the code for API endpoints (one folder per microservice), builds the diagram, \
                     and keeps everything in sync with git. Existing connections and folders are reused.",
                )
                .weak()
                .small(),
            );
            if let Some(e) = &form.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add(crate::diagram_repo::accent_button(ui, "Set Up"))
                .clicked()
            {
                submit = true;
            }
        });
    }
    if browse
        && let Some(dir) = crate::rfd::FileDialog::new()
            .set_title("Select a git repository")
            .pick_folder()
    {
        let form = &mut t.repo_setup.git_form;
        form.path = dir.to_string_lossy().to_string();
        form.error = None;
        if form.project.trim().is_empty() {
            let root = crate::git::ops::discover_root(&dir).unwrap_or(dir);
            if let Some(n) = root.file_name() {
                form.project = n.to_string_lossy().to_string();
            }
        }
    }
    if !submit {
        return false;
    }
    match validate_git_form(t) {
        Ok((dir, name)) => {
            t.repo_setup.git_form = GitFolderForm {
                active: true,
                ..Default::default()
            };
            start_in_project(t, &dir, true, Some(name));
            true
        }
        Err(e) => {
            t.repo_setup.git_form.error = Some(e);
            false
        }
    }
}

/// Pilih folder lalu jalankan setup penuh.
pub fn pick_and_start(t: &mut Tabular) {
    if let Some(dir) = crate::rfd::FileDialog::new()
        .set_title("Select a git repository to set up")
        .pick_folder()
    {
        start(t, &dir, true);
    }
}

/// Jalankan discovery untuk folder `dir` (di dalam repository git). Setup
/// manual tampil di panel Background Processes dan langsung diterapkan
/// (atau ditinjau dulu bila "Review before applying" aktif).
pub fn start(t: &mut Tabular, dir: &Path, manual: bool) {
    start_in_project(t, dir, manual, None);
}

/// Seperti [`start`], tetapi semuanya dibuat di project bernama `project`
/// (project yang ada dipakai; bila belum ada, dibuat dengan nama itu).
pub fn start_in_project(t: &mut Tabular, dir: &Path, manual: bool, project: Option<String>) {
    let project = project
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    if crate::git::ops::discover_root(dir).is_err() {
        t.toasts.error(format!(
            "{} is not inside a git repository. Use \"Init repository\" in the Git tab first.",
            dir.display()
        ));
        return;
    }
    let st = &mut t.repo_setup;
    if st
        .jobs
        .iter()
        .any(|j| j.result.is_none() && same_dir(&j.path, dir))
    {
        t.toasts
            .info("This repository is already being scanned. See Background Processes.");
        return;
    }
    let id = st.next();
    let cancel = Arc::new(AtomicBool::new(false));
    let tx = st.tx.clone();
    let path = dir.to_path_buf();
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let ev = match repo_setup::discover(&path, &flag) {
            Ok(plan) => SetupEvent::Planned {
                job: id,
                plan: Box::new(plan),
            },
            Err(e) => SetupEvent::Failed {
                job: id,
                error: e.to_string(),
            },
        };
        let _ = tx.send(ev);
    });
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    log::info!("[REPO_SETUP] scanning {}", dir.display());
    st.jobs.push(Job {
        id,
        path: dir.to_path_buf(),
        key: None,
        manual,
        started: Instant::now(),
        cancel,
        task: None,
        title: format!("Set up {name}"),
        subtitle: dir.display().to_string(),
        steps: vec![active_step(
            "Reading .env, docker-compose and config files, scanning routes",
        )],
        result: None,
        project,
    });
}

/// Jalankan ulang sinkronisasi repository yang sudah di-setup.
pub fn sync_now(t: &mut Tabular, key: &str) {
    let Some(path) = t.repo_setup.store.get(key).map(|s| PathBuf::from(&s.path)) else {
        return;
    };
    if !path.is_dir() {
        t.toasts
            .error(format!("Folder {} no longer exists", path.display()));
        return;
    }
    start(t, &path, true);
}

/// Buka jendela detail repository `key`.
pub fn open_details(t: &mut Tabular, key: &str) {
    t.repo_setup.details = Some(key.to_string());
}

/// Watcher: bandingkan sidik jari dulu, discovery hanya bila berubah.
fn start_check(t: &mut Tabular, setup: &RepoSetup) {
    let st = &mut t.repo_setup;
    let id = st.next();
    let cancel = Arc::new(AtomicBool::new(false));
    let tx = st.tx.clone();
    let path = PathBuf::from(&setup.path);
    let flag = cancel.clone();
    let previous = setup.fingerprint.clone();
    let watched = setup.watched_files.clone();
    let thread_path = path.clone();
    std::thread::spawn(move || {
        let fp = repo_setup::fingerprint::compute(&thread_path, &watched);
        let ev = if fp == previous {
            SetupEvent::Unchanged { job: id }
        } else {
            match repo_setup::discover(&thread_path, &flag) {
                Ok(plan) => SetupEvent::Planned {
                    job: id,
                    plan: Box::new(plan),
                },
                Err(e) => SetupEvent::Failed {
                    job: id,
                    error: e.to_string(),
                },
            }
        };
        let _ = tx.send(ev);
    });
    st.jobs.push(Job {
        id,
        path,
        key: Some(setup.key.clone()),
        manual: false,
        started: Instant::now(),
        cancel,
        task: None,
        title: format!("Sync {}", setup.name),
        subtitle: setup.path.clone(),
        steps: Vec::new(),
        result: None,
        project: None,
    });
}

// ─── Tick ───────────────────────────────────────────────────────────────────

/// Dipanggil tiap frame: terima hasil job, bentuk diagram yang menunggu,
/// jalankan watcher, cerminkan ke panel latar, dan gambar jendela.
pub fn tick(t: &mut Tabular, ctx: &egui::Context) {
    poll(t);
    process_pending(t);
    watch(t, ctx);
    mirror_tasks(t);
    render_review(t, ctx);
    render_details(t, ctx);
    let st = &t.repo_setup;
    let busy = st.jobs.iter().any(|j| j.result.is_none()) || st.pending.iter().any(|p| p.scanning);
    if busy {
        ctx.request_repaint_after(Duration::from_millis(250));
    } else if !st.pending.is_empty() {
        // Menunggu skema diagram: cukup dicek sesekali.
        ctx.request_repaint_after(PENDING_EVERY);
    }
}

fn poll(t: &mut Tabular) {
    let events: Vec<SetupEvent> = t.repo_setup.rx.try_iter().collect();
    for ev in events {
        match ev {
            SetupEvent::Planned { job, plan } => handle_plan(t, job, *plan),
            SetupEvent::Unchanged { job } => {
                t.repo_setup.jobs.retain(|j| j.id != job);
            }
            SetupEvent::Failed { job, error } => {
                let manual = t
                    .repo_setup
                    .jobs
                    .iter()
                    .find(|j| j.id == job)
                    .is_some_and(|j| j.manual);
                log::warn!("[REPO_SETUP] {error}");
                if manual {
                    if error != "Cancelled" {
                        t.toasts.error(error.clone());
                    }
                    finish_job(t, job, Err(error));
                } else {
                    t.repo_setup.jobs.retain(|j| j.id != job);
                }
            }
            SetupEvent::Tables { pending, outcome } => apply_tables(t, pending, outcome),
        }
    }
}

fn finish_job(t: &mut Tabular, job: u64, result: Result<String, String>) {
    if let Some(j) = t.repo_setup.jobs.iter_mut().find(|j| j.id == job) {
        let ok = result.is_ok();
        for s in &mut j.steps {
            s.status = if ok {
                ProgressStatus::Done
            } else {
                ProgressStatus::Error
            };
        }
        j.result = Some(result);
    }
}

fn handle_plan(t: &mut Tabular, job: u64, plan: SetupPlan) {
    let (manual, project) = match t.repo_setup.jobs.iter_mut().find(|j| j.id == job) {
        Some(j) => {
            j.key = Some(plan.key.clone());
            (j.manual, j.project.clone())
        }
        None => return,
    };
    rekey(t, &plan);
    let review = t.repo_setup.store.settings.review_before_apply;
    if manual {
        if review {
            finish_job(t, job, Ok("Waiting for your review".into()));
            t.repo_setup.review = Some(Box::new(plan));
            t.repo_setup.review_project = project;
            return;
        }
        let stats = apply_plan(t, &plan, true, project.as_deref());
        let summary = stats.summary(&plan.name);
        t.toasts.success(summary.clone());
        for w in &plan.warnings {
            t.toasts.info(w.clone());
        }
        finish_job(t, job, Ok(summary));
        return;
    }
    // Watcher.
    t.repo_setup.jobs.retain(|j| j.id != job);
    let Some(setup) = t.repo_setup.store.get(&plan.key).cloned() else {
        return;
    };
    if review {
        let d = diff::diff(&plan, &setup);
        let new_eps = count_new_endpoints(t, &plan, &setup);
        if d.is_empty() && new_eps == 0 {
            remember_fingerprint(t, &plan);
            return;
        }
        t.toasts.info(format!(
            "{} changed. Open Repository setup to review and apply the changes.",
            plan.name
        ));
        t.repo_setup
            .changes
            .insert(plan.key.clone(), Box::new(plan));
        return;
    }
    let stats = apply_plan(t, &plan, false, None);
    if stats.changed() {
        t.toasts.info(stats.summary(&plan.name));
    }
}

/// Kunci repository bisa berubah (remote baru ditambahkan, atau URL remote
/// diganti). Setup lama untuk folder yang sama dipindah ke kunci baru supaya
/// tidak tercatat dua kali.
fn rekey(t: &mut Tabular, plan: &SetupPlan) {
    let st = &mut t.repo_setup;
    if st.store.get(&plan.key).is_some() {
        return;
    }
    let Some(old) = st
        .store
        .setups
        .iter_mut()
        .find(|s| same_dir(Path::new(&s.path), &plan.root))
    else {
        return;
    };
    let old_key = std::mem::replace(&mut old.key, plan.key.clone());
    log::info!(
        "[REPO_SETUP] repository key changed: {old_key} -> {}",
        plan.key
    );
    if let Some(c) = st.changes.remove(&old_key) {
        st.changes.insert(plan.key.clone(), c);
    }
    if st.details.as_deref() == Some(old_key.as_str()) {
        st.details = Some(plan.key.clone());
    }
    for p in st.pending.iter_mut().filter(|p| p.key == old_key) {
        p.key = plan.key.clone();
    }
}

fn remember_fingerprint(t: &mut Tabular, plan: &SetupPlan) {
    if let Some(s) = t.repo_setup.store.get_mut(&plan.key) {
        s.fingerprint = plan.fingerprint.clone();
        s.watched_files = plan.watched_files.clone();
    }
    if let Some(e) = t.repo_setup.save() {
        t.toasts.error(e);
    }
}

fn watch(t: &mut Tabular, ctx: &egui::Context) {
    let focused = ctx.input(|i| i.focused);
    let st = &mut t.repo_setup;
    let regained = focused && !st.was_focused;
    st.was_focused = focused;
    if st.store.setups.is_empty() || st.started.elapsed() < STARTUP_DELAY {
        return;
    }
    if !regained && st.last_poll.is_some_and(|p| p.elapsed() < POLL_EVERY) {
        return;
    }
    st.last_poll = Some(Instant::now());
    st.jobs.retain(|j| {
        let stuck = !j.manual && j.result.is_none() && j.started.elapsed() > WATCH_TIMEOUT;
        if stuck {
            j.cancel.store(true, Ordering::SeqCst);
            log::warn!("[REPO_SETUP] background check of {} timed out", j.subtitle);
        }
        !stuck
    });
    // Satu pemeriksaan latar pada satu waktu.
    if st.jobs.iter().any(|j| !j.manual && j.result.is_none()) {
        return;
    }
    let interval = Duration::from_secs(u64::from(st.store.settings.interval_min) * 60);
    let due = st
        .store
        .setups
        .iter()
        .filter(|s| s.auto_sync && !st.changes.contains_key(&s.key))
        .filter(|s| !st.is_running(&s.key))
        .find(|s| {
            let last = st.last_check.get(&s.key);
            (regained && last.is_none_or(|l| l.elapsed() >= FOCUS_MIN_GAP))
                || (!interval.is_zero() && last.is_none_or(|l| l.elapsed() >= interval))
        })
        .cloned();
    if let Some(s) = due {
        st.last_check.insert(s.key.clone(), Instant::now());
        if Path::new(&s.path).is_dir() {
            start_check(t, &s);
        }
    }
}

fn mirror_tasks(t: &mut Tabular) {
    let bg = &mut t.background_tasks;
    let st = &mut t.repo_setup;
    let mut show: Option<String> = None;
    for job in st.jobs.iter_mut().filter(|j| j.manual) {
        let out = bg.mirror(
            &mut job.task,
            Snapshot {
                owner: TaskOwner::RepoSetup,
                title: &job.title,
                subtitle: &job.subtitle,
                steps: &job.steps,
                started_at: Some(job.started),
                last_activity_at: None,
                hidden: true,
                result: job.result.clone(),
            },
        );
        if out.show {
            show = job.key.clone();
        }
        if out.cancel {
            job.cancel.store(true, Ordering::SeqCst);
        }
    }
    // Job selesai dibuang setelah entrinya hilang dari panel.
    st.jobs
        .retain(|j| j.result.is_none() || (j.manual && j.task.is_some_and(|id| bg.contains(id))));
    if let Some(key) = show
        && st.store.get(&key).is_some()
    {
        st.details = Some(key);
    }
}

// ─── Penerapan plan ─────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy)]
struct Stats {
    conn_new: usize,
    conn_updated: usize,
    conn_stale: usize,
    needs_credentials: usize,
    folders_new: usize,
    endpoints_new: usize,
    services_stale: usize,
    diagrams: usize,
    project_new: bool,
}

impl Stats {
    fn changed(&self) -> bool {
        self.conn_new
            + self.conn_updated
            + self.conn_stale
            + self.folders_new
            + self.endpoints_new
            + self.services_stale
            > 0
    }

    fn summary(&self, name: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.project_new {
            parts.push("project created".into());
        }
        if self.conn_new > 0 {
            parts.push(format!("{} new connection(s)", self.conn_new));
        }
        if self.conn_updated > 0 {
            parts.push(format!("{} connection(s) updated", self.conn_updated));
        }
        if self.folders_new > 0 {
            parts.push(format!("{} API folder(s)", self.folders_new));
        }
        if self.endpoints_new > 0 {
            parts.push(format!("{} new endpoint(s)", self.endpoints_new));
        }
        if self.diagrams > 0 {
            parts.push(format!("{} diagram(s)", self.diagrams));
        }
        if self.conn_stale + self.services_stale > 0 {
            parts.push(format!(
                "{} item(s) no longer in the repository",
                self.conn_stale + self.services_stale
            ));
        }
        if self.needs_credentials > 0 {
            parts.push(format!(
                "{} connection(s) need credentials",
                self.needs_credentials
            ));
        }
        if parts.is_empty() {
            format!("{name} is up to date")
        } else {
            format!("{name}: {}", parts.join(", "))
        }
    }
}

/// Terapkan `plan`: project, koneksi, folder HTTP API, diagram, dan daftar
/// repository Git. Idempoten.
fn apply_plan(
    t: &mut Tabular,
    plan: &SetupPlan,
    manual: bool,
    project_name: Option<&str>,
) -> Stats {
    super::project_ui::ensure_loaded(t);
    let root_s = plan.root.to_string_lossy().to_string();
    let mut setup = t
        .repo_setup
        .store
        .get(&plan.key)
        .cloned()
        .unwrap_or_else(|| RepoSetup::new(&plan.key, &root_s, &plan.name));
    setup.path = root_s.clone();
    setup.name = plan.name.clone();
    let mut stats = Stats::default();

    // 1. Project (folder koneksi, workspace HTTP, environment + variabel).
    let (mut project, project_new) = prepare_project(t, plan, &setup, project_name);
    stats.project_new = project_new;
    merge_env_vars(&mut project, plan, &mut setup);
    crate::sidebar_database::save_connection_folder(t, &project.connection_folder);
    if project::ensure_http_workspace(&mut t.yaak_workspaces, &mut project)
        && let Err(e) = crate::http_collection::save_workspaces(&t.yaak_workspaces)
    {
        t.toasts.error(e);
    }

    // 2. Koneksi.
    let ids = apply_connections(t, plan, &mut setup, &project.connection_folder, &mut stats);
    assign_env_connections(t, &mut project, plan, &ids);
    project.touch();
    let pid = project.id.clone();
    let saved = super::project_ui::save_from_setup(t, project.clone());
    if saved && project_new && manual {
        super::project_ui::set_active(t, Some(pid.clone()));
    }
    if saved || !project_new {
        setup.project_id = Some(pid.clone());
    }

    // 3. HTTP API.
    if let Some(ws_id) = project.http_workspace_id.clone() {
        apply_services(t, plan, &mut setup, &ws_id, manual, &mut stats);
    }

    // 4. Diagram.
    schedule_diagrams(t, plan, &mut setup, &ids, manual, &mut stats);

    // 5. Tab Git.
    t.git.store.add_to_project(&root_s, Some(&pid));
    if let Err(e) = t.git.save_store() {
        t.toasts.error(e);
    }
    if t.git.repos_loaded {
        super::git_jobs::reload_repos(t);
    }

    setup.fingerprint = plan.fingerprint.clone();
    setup.watched_files = plan.watched_files.clone();
    setup.warnings = plan.warnings.clone();
    setup.last_sync_at = chrono::Utc::now().timestamp();
    setup.last_summary = stats.summary(&plan.name);
    let st = &mut t.repo_setup;
    st.changes.remove(&plan.key);
    match st.store.get_mut(&plan.key) {
        Some(slot) => *slot = setup,
        None => st.store.setups.push(setup),
    }
    if let Some(e) = st.save() {
        t.toasts.error(e);
    }
    log::info!("[REPO_SETUP] applied: {}", stats.summary(&plan.name));
    if stats.changed() {
        crate::engram::record(
            crate::engram::ActivityKind::Project,
            Some(&plan.name),
            format!(
                "Set up the repository {} in Tabular: {}",
                plan.root.display(),
                stats.summary(&plan.name)
            ),
        );
    }
    stats
}

fn prepare_project(
    t: &Tabular,
    plan: &SetupPlan,
    setup: &RepoSetup,
    project_name: Option<&str>,
) -> (Project, bool) {
    let list = &t.projects.list;
    // Nama project pilihan user menang atas semua tebakan.
    if let Some(name) = project_name {
        if let Some(p) = list
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) && !p.is_read_only())
        {
            return (p.clone(), false);
        }
        if project::validate_name(name, list, None).is_ok() {
            let mut p = Project::new(name);
            p.description = format!("Set up from {}", plan.root.display());
            return (p, true);
        }
        log::warn!("[REPO_SETUP] project name '{name}' is not valid; choosing automatically");
    }
    let by_key = |p: &&Project| {
        p.repo_url
            .as_deref()
            .and_then(crate::repo_scan::repo_key)
            .is_some_and(|k| k == plan.key)
    };
    // Urutan: project yang sudah tercatat untuk repository ini, lalu project
    // yang sedang dipilih user di switcher, lalu project dengan URL git yang
    // sama. Project baru hanya dibuat bila tidak ada satu pun.
    let found = setup
        .project_id
        .as_deref()
        .and_then(|id| list.iter().find(|p| p.id == id))
        .or_else(|| super::project_ui::active(t))
        .or_else(|| list.iter().find(by_key))
        .filter(|p| !p.is_read_only())
        .cloned();
    if let Some(p) = found {
        return (p, false);
    }
    let mut name = plan.name.clone();
    let mut n = 2;
    while project::validate_name(&name, list, None).is_err() && n < 100 {
        name = format!("{} {n}", plan.name.replace(['/', '\\'], "-"));
        n += 1;
    }
    let mut p = Project::new(&name);
    p.description = format!("Set up from {}", plan.root.display());
    (p, true)
}

fn value_hash(v: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(v.as_bytes()))
}

/// Variabel `.env` menjadi variabel environment project. Nilai rahasia
/// (dari nama atau isinya) disimpan di keychain. Variabel yang sudah ada
/// hanya ditimpa bila nilainya di repository berubah sejak impor terakhir,
/// jadi editan user dan pilihan "secret" dihormati.
fn merge_env_vars(p: &mut Project, plan: &SetupPlan, setup: &mut RepoSetup) {
    if p.repo_url.is_none() {
        p.repo_url = plan.remote_url.clone();
    }
    for (env, vars) in &plan.env_vars {
        let name = env_files::project_env_name(*env);
        let idx = match p
            .environments
            .iter()
            .position(|e| e.name.eq_ignore_ascii_case(name) || e.environment() == Some(*env))
        {
            Some(i) => i,
            None => {
                p.environments.push(ProjectEnv::new(name, Some(*env)));
                p.environments.len() - 1
            }
        };
        let pe = &mut p.environments[idx];
        for (k, v) in vars {
            if k.trim().is_empty() {
                continue;
            }
            let id = format!("{}|{k}", env.key());
            let hash = value_hash(v);
            let previous = setup.imported_vars.insert(id, hash.clone());
            match pe.variables.iter_mut().find(|x| x.key == *k) {
                // Belum pernah diimpor (variabel milik user) atau nilai di
                // repository tidak berubah: biarkan.
                Some(_) if previous.as_deref().is_none_or(|h| h == hash) => {}
                Some(var) if var.secret => var.pending_secret = Some(v.clone()),
                Some(var) => var.value = v.clone(),
                None => {
                    let secret = env_files::is_secret(k, v);
                    pe.variables.push(EnvVar {
                        key: k.clone(),
                        value: if secret { String::new() } else { v.clone() },
                        secret,
                        pending_secret: secret.then(|| v.clone()),
                    });
                }
            }
        }
    }
}

fn assign_env_connections(
    t: &Tabular,
    p: &mut Project,
    plan: &SetupPlan,
    ids: &HashMap<String, i64>,
) {
    for c in &plan.connections {
        let Some(id) = ids.get(&c.identity()) else {
            continue;
        };
        let Some(conn) = t.connections.iter().find(|k| k.id == Some(*id)) else {
            continue;
        };
        // Koneksi di luar folder project (diadopsi) tidak dicatat.
        if !p.owns_connection(conn) {
            continue;
        }
        let name = env_files::project_env_name(c.environment);
        let idx = match p.environments.iter().position(|e| {
            e.name.eq_ignore_ascii_case(name) || e.environment() == Some(c.environment)
        }) {
            Some(i) => i,
            None => {
                p.environments
                    .push(ProjectEnv::new(name, Some(c.environment)));
                p.environments.len() - 1
            }
        };
        let env = &mut p.environments[idx];
        if !env.connections.iter().any(|n| n == &conn.name) {
            env.connections.push(conn.name.clone());
        }
    }
}

/// Buat/perbarui koneksi. Mengembalikan identitas plan → id koneksi.
fn apply_connections(
    t: &mut Tabular,
    plan: &SetupPlan,
    setup: &mut RepoSetup,
    folder: &str,
    stats: &mut Stats,
) -> HashMap<String, i64> {
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut to_insert: Vec<&PlannedConnection> = Vec::new();
    let mut adopted: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in &plan.connections {
        let identity = c.identity();
        let tracked = setup
            .connections
            .iter()
            .find(|x| x.identity == identity)
            .cloned();
        let own = tracked
            .as_ref()
            .filter(|x| !x.adopted)
            .and_then(|x| x.connection_id)
            .filter(|id| t.connections.iter().any(|k| k.id == Some(*id)));
        match own {
            Some(id) => {
                if tracked.as_ref().is_some_and(|x| x.digest != c.digest())
                    && update_connection(t, id, c)
                {
                    stats.conn_updated += 1;
                }
                ids.insert(identity, id);
            }
            // Koneksi user untuk server/database yang sama dipakai ulang,
            // tetapi tidak pernah diubah.
            None => match crate::deeplink::find_matching_connection(&t.connections, &c.dsn) {
                Some(id) => {
                    adopted.insert(identity.clone());
                    ids.insert(identity, id);
                }
                None => to_insert.push(c),
            },
        }
    }
    if !to_insert.is_empty() {
        for c in &to_insert {
            let mut cfg = c.dsn.to_connection_config(Some(&c.name));
            if dsn_detect::looks_placeholder(&cfg.password) {
                cfg.password.clear();
            }
            cfg.folder = Some(folder.to_string());
            if !crate::sidebar_database::save_connection_to_database(t, &cfg) {
                log::warn!("[REPO_SETUP] could not save connection '{}'", c.name);
            }
        }
        let known: std::collections::HashSet<i64> =
            t.connections.iter().filter_map(|k| k.id).collect();
        crate::sidebar_database::load_connections(t);
        for c in &to_insert {
            // Hanya baris yang baru dibuat (id belum dikenal sebelum insert).
            let id = t
                .connections
                .iter()
                .filter(|k| k.id.is_some_and(|id| !known.contains(&id)))
                .find(|k| {
                    k.name == c.name
                        && k.folder.as_deref() == Some(folder)
                        && k.host == c.dsn.host
                        && k.database == c.dsn.database
                })
                .and_then(|k| k.id);
            if let Some(id) = id {
                ids.insert(c.identity(), id);
                stats.conn_new += 1;
            }
        }
    }

    let mut tracked_new: Vec<TrackedConnection> = Vec::new();
    for c in &plan.connections {
        let identity = c.identity();
        let id = ids.get(&identity).copied();
        let is_adopted = adopted.contains(&identity)
            || setup
                .connections
                .iter()
                .any(|x| x.identity == identity && x.adopted);
        if let Some(id) = id
            && !is_adopted
            && t.platform_ui.connection_envs.get(&id) != Some(&c.environment)
        {
            t.set_connection_environment(id, Some(c.environment));
        }
        if c.needs_credentials {
            stats.needs_credentials += 1;
        }
        let name = id
            .and_then(|id| t.connections.iter().find(|k| k.id == Some(id)))
            .map(|k| k.name.clone())
            .unwrap_or_else(|| c.name.clone());
        tracked_new.push(TrackedConnection {
            identity,
            connection_id: id,
            name,
            environment: c.environment.key().to_string(),
            digest: c.digest(),
            needs_credentials: c.needs_credentials,
            adopted: is_adopted,
            stale: false,
        });
    }
    for old in &setup.connections {
        if !tracked_new.iter().any(|n| n.identity == old.identity) {
            let mut o = old.clone();
            if !o.stale {
                stats.conn_stale += 1;
            }
            o.stale = true;
            tracked_new.push(o);
        }
    }
    setup.connections = tracked_new;
    ids
}

/// Perbarui koneksi `id` dengan nilai baru dari repository. Password yang
/// kosong/placeholder di repository tidak menimpa password yang sudah diisi.
fn update_connection(t: &mut Tabular, id: i64, c: &PlannedConnection) -> bool {
    let Some(old) = t.connections.iter().find(|k| k.id == Some(id)).cloned() else {
        return false;
    };
    let mut cfg = old.clone();
    cfg.connection_type = c.dsn.db_type.clone();
    cfg.host = c.dsn.host.clone();
    cfg.port = c.dsn.port.clone();
    cfg.username = c.dsn.username.clone();
    cfg.database = c.dsn.database.clone();
    cfg.ssl_enabled = c.dsn.ssl;
    if !c.dsn.password.is_empty() && !dsn_detect::looks_placeholder(&c.dsn.password) {
        cfg.password = c.dsn.password.clone();
    }
    let same = cfg.connection_type == old.connection_type
        && cfg.host == old.host
        && cfg.port == old.port
        && cfg.username == old.username
        && cfg.database == old.database
        && cfg.ssl_enabled == old.ssl_enabled
        && cfg.password == old.password;
    if same {
        return false;
    }
    if !crate::sidebar_database::update_connection_in_database(t, &cfg) {
        log::warn!("[REPO_SETUP] could not update connection {id}");
        return false;
    }
    if let Some(slot) = t.connections.iter_mut().find(|k| k.id == Some(id)) {
        *slot = cfg;
    }
    // Pool lama menunjuk server/kredensial lama.
    t.disconnect_connection(id);
    crate::sidebar_database::refresh_connections_tree(t);
    true
}

fn service_dir(plan: &SetupPlan, rel: &str) -> PathBuf {
    if rel.is_empty() {
        plan.root.clone()
    } else {
        plan.root.join(rel)
    }
}

/// Folder HTTP API per service; hanya endpoint baru yang ditambahkan, jadi
/// endpoint yang sudah didokumentasikan (AI atau manual) tidak ditimpa.
fn apply_services(
    t: &mut Tabular,
    plan: &SetupPlan,
    setup: &mut RepoSetup,
    ws_id: &str,
    manual: bool,
    stats: &mut Stats,
) {
    use crate::http_collection::{find_folder, find_folder_mut};
    use crate::repo_endpoints::{
        GeneratedEndpoint, add_endpoints_to_folder, folder_request_keys, saved_request_key,
        to_saved_request,
    };
    let mut tracked_new: Vec<TrackedService> = Vec::new();
    let mut fresh_folders: Vec<String> = Vec::new();
    for s in &plan.services {
        let prev = setup.services.iter().find(|x| x.rel_dir == s.rel_dir);
        let abs = service_dir(plan, &s.rel_dir).to_string_lossy().to_string();
        let folders = t
            .yaak_workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .map(|w| w.folders.clone())
            .unwrap_or_default();
        // Folder yang sudah ada dipakai ulang: yang tercatat, yang namanya
        // sama, atau (aplikasi tunggal) yang sudah menunjuk repository ini.
        let single = plan.services.len() == 1;
        let folder_id = prev
            .and_then(|p| p.folder_id.clone())
            .filter(|id| find_folder(&folders, id).is_some())
            .or_else(|| {
                folders
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case(&s.name))
                    .map(|f| f.id.clone())
            })
            .or_else(|| {
                single
                    .then(|| {
                        folders
                            .iter()
                            .find(|f| f.repo_key().as_deref() == Some(plan.key.as_str()))
                            .map(|f| f.id.clone())
                    })
                    .flatten()
            });
        let folder_id = match folder_id {
            Some(id) => id,
            None => match crate::http_collection::create_folder_in_workspace(
                &mut t.yaak_workspaces,
                ws_id,
                None,
                &s.name,
            ) {
                Some(f) => {
                    stats.folders_new += 1;
                    f.id
                }
                None => continue,
            },
        };
        if let Some(ws) = t.yaak_workspaces.iter_mut().find(|w| w.id == ws_id)
            && let Some(f) = find_folder_mut(&mut ws.folders, &folder_id)
            && f.shared_repo_url().is_none()
        {
            f.repo_url = plan.remote_url.clone();
        }
        if crate::diagram_repo_paths::http_folder_repo_path(&folder_id).as_deref() != Some(&abs)
            && let Err(e) =
                crate::diagram_repo_paths::set_http_folder_repo_path(&folder_id, Some(&abs))
        {
            log::warn!("[REPO_SETUP] {e}");
        }
        let existing = t
            .yaak_workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .and_then(|w| find_folder(&w.folders, &folder_id))
            .map(folder_request_keys)
            .unwrap_or_default();
        let new_eps: Vec<GeneratedEndpoint> = s
            .endpoints
            .iter()
            .filter(|e| !existing.contains(&saved_request_key(&to_saved_request(e, &s.base_url))))
            .cloned()
            .collect();
        if !new_eps.is_empty() {
            match add_endpoints_to_folder(
                &mut t.yaak_workspaces,
                ws_id,
                &folder_id,
                &new_eps,
                &s.base_url,
                true,
            ) {
                Ok(added) => stats.endpoints_new += added.added,
                Err(e) => log::warn!("[REPO_SETUP] {e}"),
            }
        }
        // Folder yang masih punya endpoint tanpa daftar tabel (baru ditemukan,
        // atau setup sebelumnya tanpa AI) didokumentasikan pada setup manual.
        let undocumented = t
            .yaak_workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .and_then(|w| find_folder(&w.folders, &folder_id))
            .is_some_and(|f| f.all_requests().iter().any(|r| r.tables.is_empty()));
        if manual && undocumented {
            fresh_folders.push(folder_id.clone());
        }
        let total = t
            .yaak_workspaces
            .iter()
            .find(|w| w.id == ws_id)
            .and_then(|w| find_folder(&w.folders, &folder_id))
            .map(|f| f.all_requests().len())
            .unwrap_or_default();
        tracked_new.push(TrackedService {
            name: s.name.clone(),
            rel_dir: s.rel_dir.clone(),
            folder_id: Some(folder_id),
            endpoints: total,
            stale: false,
        });
    }
    for old in &setup.services {
        if !tracked_new.iter().any(|n| n.rel_dir == old.rel_dir) {
            let mut o = old.clone();
            if !o.stale {
                stats.services_stale += 1;
            }
            o.stale = true;
            tracked_new.push(o);
        }
    }
    setup.services = tracked_new;
    if let Err(e) = crate::http_collection::save_workspaces(&t.yaak_workspaces) {
        t.toasts.error(e);
    }
    // Dokumentasi AI (parameter, body, tabel) berjalan di background lalu
    // langsung diterapkan, ditautkan ke tabel diagram, dan alur bisnisnya
    // dibuat, supaya hasil setup langsung bisa dipakai.
    if t.repo_setup.store.settings.ai_enrich && !fresh_folders.is_empty() {
        let target = t.effective_chat_target();
        match crate::ai_assistant::backend_ready_for(t, target) {
            Ok(()) => {
                for folder_id in fresh_folders {
                    crate::http_repo::perform(
                        t,
                        crate::http_repo::RepoAction::AutoDocumentEndpoints { folder_id },
                    );
                }
            }
            Err(e) => t.toasts.info(format!(
                "Endpoints were added without AI documentation: {e}"
            )),
        }
    }
}

/// Daftarkan diagram yang dibentuk group per service; tab dibuka pada setup
/// manual bila preferensi "open diagrams" aktif.
fn schedule_diagrams(
    t: &mut Tabular,
    plan: &SetupPlan,
    setup: &mut RepoSetup,
    ids: &HashMap<String, i64>,
    manual: bool,
    stats: &mut Stats,
) {
    let open = manual && t.repo_setup.store.settings.open_diagrams;
    for target in plan.diagram_targets() {
        let Some(&conn_id) = ids.get(&target.identity()) else {
            continue;
        };
        let db = if target.dsn.db_type == DatabaseType::SQLite {
            "main".to_string()
        } else {
            target.dsn.database.clone()
        };
        if db.trim().is_empty() {
            continue;
        }
        let services = diagram_services(plan, target);
        if t.repo_setup
            .pending
            .iter()
            .any(|p| p.conn_id == conn_id && p.db == db)
        {
            continue;
        }
        let id = t.repo_setup.next();
        t.repo_setup.pending.push(PendingDiagram {
            id,
            key: plan.key.clone(),
            conn_id,
            db: db.clone(),
            services,
            repo_url: plan.remote_url.clone(),
            since: Instant::now(),
            scanning: false,
            announce: manual,
        });
        if !setup
            .diagrams
            .iter()
            .any(|d| d.connection_id == conn_id && d.database == db)
        {
            setup.diagrams.push(TrackedDiagram {
                connection_id: conn_id,
                database: db.clone(),
                groups: 0,
                tables: 0,
                placed: Vec::new(),
            });
        }
        stats.diagrams += 1;
        if open {
            t.show_database_diagram(conn_id, db);
        }
    }
}

/// Service yang memakai database `target`.
fn diagram_services(plan: &SetupPlan, target: &PlannedConnection) -> Vec<(String, String)> {
    let tid = dsn_detect::server_identity(&target.dsn);
    let entry = |name: &str, rel: &str| {
        (
            name.to_string(),
            service_dir(plan, rel).to_string_lossy().to_string(),
        )
    };
    let users: Vec<(String, String)> = plan
        .services
        .iter()
        .filter(|s| {
            plan.connections.iter().any(|c| {
                c.service.as_deref() == Some(s.name.as_str())
                    && dsn_detect::server_identity(&c.dsn) == tid
            })
        })
        .map(|s| entry(&s.name, &s.rel_dir))
        .collect();
    if !users.is_empty() {
        return users;
    }
    if plan.services.is_empty() {
        return vec![entry(&plan.name, "")];
    }
    plan.services
        .iter()
        .map(|s| entry(&s.name, &s.rel_dir))
        .collect()
}

/// Diagram (conn, db) yang skemanya sudah tersedia: tab terbuka yang
/// selesai sinkron, atau file cache lokal. Tidak mem-parse apa pun di sini:
/// file diagram besar dibaca di thread latar.
fn diagram_source(t: &Tabular, conn_id: i64, db: &str) -> Option<DiagramSource> {
    if let Some(tab) = t
        .query_tabs
        .iter()
        .find(|q| Tabular::is_diagram_host_tab(q, conn_id, db))
    {
        let s = tab.diagram_state.as_ref()?;
        return (!s.schema_syncing && !s.nodes.is_empty())
            .then(|| DiagramSource::Tab(Box::new(s.clone())));
    }
    let path = t.get_diagram_path(conn_id, db)?;
    path.exists().then_some(DiagramSource::File(path))
}

fn read_diagram(path: &Path) -> Option<DiagramState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice::<DiagramState>(&bytes).ok()
}

/// Tabel yang pernah ditempatkan di diagram (conn, db) repository `key`.
fn placed_tables(t: &Tabular, key: &str, conn_id: i64, db: &str) -> Vec<String> {
    t.repo_setup
        .store
        .get(key)
        .and_then(|s| {
            s.diagrams
                .iter()
                .find(|d| d.connection_id == conn_id && d.database == db)
        })
        .map(|d| d.placed.clone())
        .unwrap_or_default()
}

/// Kerja berat pembentukan group per service (baca diagram, cari tabel di
/// kode, susun ulang posisi); dijalankan di thread latar.
fn build_groups(
    source: DiagramSource,
    services: Vec<(String, String)>,
    repo_url: Option<String>,
    placed: Vec<String>,
) -> Option<TablesOutcome> {
    let state = match source {
        DiagramSource::Tab(s) => *s,
        DiagramSource::File(path) => read_diagram(&path)?,
    };
    if state.nodes.is_empty() {
        return None;
    }
    let candidates = diagram_groups::candidates(&state);
    let cancel = AtomicBool::new(false);
    let mut services: Vec<ServiceTables> = services
        .into_iter()
        .map(|(service, dir)| {
            let report = crate::repo_scan::grep_tables(Path::new(&dir), &candidates, &cancel)
                .unwrap_or_default();
            ServiceTables {
                service,
                dir,
                table_ids: diagram_groups::confident_ids(&report),
            }
        })
        .collect();
    // Tabel yang pernah ditempatkan tidak ditambahkan lagi: bila user
    // mengeluarkannya dari group, keputusan itu bertahan.
    let mut newly_placed: Vec<String> = Vec::new();
    for s in &mut services {
        s.table_ids.retain(|id| {
            let tag = format!("{}|{id}", s.service);
            let fresh = !placed.contains(&tag);
            if fresh {
                newly_placed.push(tag);
            }
            fresh
        });
    }
    let (sync, patch) = GroupPatch::compute(state, repo_url.as_deref(), &services);
    Some(TablesOutcome {
        sync,
        patch,
        newly_placed,
    })
}

fn process_pending(t: &mut Tabular) {
    if t.repo_setup.pending.is_empty()
        || t.repo_setup
            .last_pending_check
            .is_some_and(|at| at.elapsed() < PENDING_EVERY)
    {
        return;
    }
    t.repo_setup.last_pending_check = Some(Instant::now());
    let mut expired: Vec<u64> = Vec::new();
    let mut ready: Vec<(u64, DiagramSource)> = Vec::new();
    for p in t.repo_setup.pending.iter() {
        if p.scanning {
            // Thread pencarian yang tidak pernah melapor (panic) dilepas.
            if p.since.elapsed() > DIAGRAM_WAIT * 3 {
                expired.push(p.id);
            }
            continue;
        }
        match diagram_source(t, p.conn_id, &p.db) {
            Some(source) => ready.push((p.id, source)),
            None if p.since.elapsed() > DIAGRAM_WAIT => expired.push(p.id),
            None => {}
        }
    }
    for id in expired {
        if let Some(p) = t.repo_setup.pending.iter().find(|p| p.id == id) {
            log::info!(
                "[REPO_SETUP] schema of '{}' not available; diagram groups skipped",
                p.db
            );
            if p.announce {
                t.toasts.info(format!(
                    "Diagram groups for '{}' were skipped: the database did not respond. Use Sync now later.",
                    p.db
                ));
            }
        }
        t.repo_setup.pending.retain(|p| p.id != id);
    }
    for (id, source) in ready {
        let Some(p) = t.repo_setup.pending.iter().find(|p| p.id == id) else {
            continue;
        };
        let placed = placed_tables(t, &p.key, p.conn_id, &p.db);
        let Some(p) = t.repo_setup.pending.iter_mut().find(|p| p.id == id) else {
            continue;
        };
        p.scanning = true;
        let services = p.services.clone();
        let repo_url = p.repo_url.clone();
        let tx = t.repo_setup.tx.clone();
        std::thread::spawn(move || {
            let outcome = build_groups(source, services, repo_url, placed).map(Box::new);
            let _ = tx.send(SetupEvent::Tables {
                pending: id,
                outcome,
            });
        });
    }
}

fn apply_tables(t: &mut Tabular, pending: u64, outcome: Option<Box<TablesOutcome>>) {
    let Some(idx) = t.repo_setup.pending.iter().position(|p| p.id == pending) else {
        return;
    };
    let Some(outcome) = outcome else {
        // Skema belum terbaca (file kosong/rusak): tunggu giliran berikutnya.
        t.repo_setup.pending[idx].scanning = false;
        return;
    };
    let p = t.repo_setup.pending.remove(idx);
    let TablesOutcome {
        sync: result,
        patch,
        newly_placed,
    } = *outcome;
    let mut patched_in_tab = false;
    if !patch.is_empty() {
        let host = t
            .query_tabs
            .iter()
            .position(|q| Tabular::is_diagram_host_tab(q, p.conn_id, &p.db));
        if let Some(i) = host {
            patched_in_tab = true;
            let snapshot = t.query_tabs[i].diagram_state.as_mut().map(|state| {
                patch.apply(state);
                state.clone()
            });
            if let Some(state) = snapshot {
                t.save_diagram(p.conn_id, &p.db, &state);
            }
        } else if let Some(path) = t.get_diagram_path(p.conn_id, &p.db) {
            // Tab tidak terbuka: baca ulang file terbaru, tempel patch, dan
            // tulis di thread supaya UI tidak tersendat.
            std::thread::spawn(move || {
                let Some(mut state) = read_diagram(&path) else {
                    log::warn!("[REPO_SETUP] cannot read diagram {}", path.display());
                    return;
                };
                patch.apply(&mut state);
                let state = crate::diagram_links::persistable(&state);
                let written = serde_json::to_vec_pretty(&state)
                    .map_err(|e| e.to_string())
                    .and_then(|b| {
                        crate::diagram_view::write_atomic(&path, &b).map_err(|e| e.to_string())
                    });
                if let Err(e) = written {
                    log::warn!("[REPO_SETUP] cannot update diagram {}: {e}", path.display());
                }
            });
        }
    }
    for (gid, dir) in &result.groups {
        if crate::diagram_repo_paths::local_repo_path(gid).is_none()
            && let Err(e) = crate::diagram_repo_paths::set_local_repo_path(gid, Some(dir))
        {
            log::warn!("[REPO_SETUP] {e}");
        }
    }
    if let Some(setup) = t.repo_setup.store.get_mut(&p.key)
        && let Some(d) = setup
            .diagrams
            .iter_mut()
            .find(|d| d.connection_id == p.conn_id && d.database == p.db)
    {
        d.groups = result.groups.len();
        d.tables += result.tables_added;
        d.placed.extend(newly_placed);
    }
    if let Some(e) = t.repo_setup.save() {
        t.toasts.error(e);
    }
    if result.changed() {
        t.toasts.info(format!(
            "Diagram '{}': {} group(s), {} table(s) placed by service",
            p.db,
            result.groups.len(),
            result.tables_added
        ));
    }
    // Group baru: endpoint yang sudah punya tabel langsung ditautkan. Hanya
    // bila patch masuk ke tab terbuka; patch file di thread bisa balapan
    // dengan tulisan tautan, dan dokumentasi AI menautkan lagi saat selesai.
    if patched_in_tab {
        let folders: Vec<String> = t
            .repo_setup
            .store
            .get(&p.key)
            .map(|s| {
                s.services
                    .iter()
                    .filter_map(|x| x.folder_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        for folder_id in folders {
            let summary = crate::http_repo::relink_folder(t, &folder_id);
            if !summary.is_empty() {
                t.toasts.success(summary);
            }
        }
    }
}

/// Endpoint plan yang belum ada di folder HTTP tercatat (mode review).
fn count_new_endpoints(t: &Tabular, plan: &SetupPlan, setup: &RepoSetup) -> usize {
    use crate::repo_endpoints::{folder_request_keys, saved_request_key, to_saved_request};
    plan.services
        .iter()
        .map(|s| {
            let folder = setup
                .services
                .iter()
                .find(|x| x.rel_dir == s.rel_dir)
                .and_then(|x| x.folder_id.as_deref())
                .and_then(|id| {
                    crate::http_collection::find_workspace_folder(&t.yaak_workspaces, id)
                        .map(|(_, f)| f)
                });
            let keys = folder.map(folder_request_keys).unwrap_or_default();
            s.endpoints
                .iter()
                .filter(|e| !keys.contains(&saved_request_key(&to_saved_request(e, &s.base_url))))
                .count()
        })
        .sum()
}

// ─── Jendela ────────────────────────────────────────────────────────────────

fn env_badge(ui: &mut egui::Ui, key: &str) {
    match crate::connection_env::Environment::parse(key) {
        Some(e) => {
            ui.label(
                egui::RichText::new(e.short())
                    .small()
                    .strong()
                    .color(e.color()),
            );
        }
        None => {
            ui.label("");
        }
    }
}

fn plan_body(ui: &mut egui::Ui, plan: &SetupPlan) {
    ui.label(egui::RichText::new(plan.root.display().to_string()).weak());
    ui.add_space(6.0);
    ui.strong(format!("Database connections ({})", plan.connections.len()));
    egui::Grid::new("repo_setup_plan_conns")
        .striped(true)
        .num_columns(3)
        .show(ui, |ui| {
            for c in &plan.connections {
                ui.label(&c.name);
                env_badge(ui, c.environment.key());
                let mut src = format!("{} ({})", c.source, c.key);
                if c.needs_credentials {
                    src.push_str(", needs credentials");
                }
                if let Some(n) = &c.note {
                    src.push_str(&format!(", {n}"));
                }
                ui.label(egui::RichText::new(src).weak());
                ui.end_row();
            }
        });
    ui.add_space(6.0);
    ui.strong(if plan.is_microservices() {
        format!("Microservices ({})", plan.services.len())
    } else {
        format!("API ({})", plan.services.len())
    });
    egui::Grid::new("repo_setup_plan_services")
        .striped(true)
        .num_columns(3)
        .show(ui, |ui| {
            for s in &plan.services {
                ui.label(&s.name);
                ui.label(format!("{} endpoint(s)", s.endpoints.len()));
                ui.label(
                    egui::RichText::new(format!(
                        "{} {}",
                        if s.rel_dir.is_empty() {
                            "."
                        } else {
                            &s.rel_dir
                        },
                        s.base_url
                    ))
                    .weak(),
                );
                ui.end_row();
            }
        });
    let targets = plan.diagram_targets();
    if !targets.is_empty() {
        ui.add_space(6.0);
        ui.strong("Diagrams");
        for c in targets {
            ui.label(format!("{} (one group per service)", c.name));
        }
    }
    for w in &plan.warnings {
        ui.colored_label(ui.visuals().warn_fg_color, w);
    }
}

fn render_review(t: &mut Tabular, ctx: &egui::Context) {
    let Some(plan) = t.repo_setup.review.as_ref() else {
        return;
    };
    let mut open = true;
    let mut apply = false;
    let mut cancel = false;
    egui::Window::new(format!("Set up {}", plan.name))
        .id(egui::Id::new("repo_setup_review"))
        .open(&mut open)
        .collapsible(false)
        .default_width(620.0)
        .show(ctx, |ui| {
            ui.label("Tabular will create or update the following:");
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| plan_body(ui, plan));
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add(crate::diagram_repo::accent_button(ui, "Apply"))
                    .clicked()
                {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    if apply {
        if let Some(plan) = t.repo_setup.review.take() {
            let project = t.repo_setup.review_project.take();
            let stats = apply_plan(t, &plan, true, project.as_deref());
            t.toasts.success(stats.summary(&plan.name));
        }
    } else if cancel || !open {
        t.repo_setup.review = None;
        t.repo_setup.review_project = None;
    }
}

enum DetailAction {
    Sync,
    ApplyChanges,
    ToggleAuto(bool),
    ForgetStale,
    Remove,
}

fn render_details(t: &mut Tabular, ctx: &egui::Context) {
    let Some(key) = t.repo_setup.details.clone() else {
        return;
    };
    let Some(setup) = t.repo_setup.store.get(&key).cloned() else {
        t.repo_setup.details = None;
        return;
    };
    let running = t.repo_setup.is_running(&key);
    let pending_change = t.repo_setup.changes.get(&key).cloned();
    let project_name = setup
        .project_id
        .as_deref()
        .and_then(|id| t.projects.list.iter().find(|p| p.id == id))
        .map(|p| p.name.clone());
    let mut open = true;
    let mut action: Option<DetailAction> = None;
    egui::Window::new(format!("Repository setup: {}", setup.name))
        .id(egui::Id::new("repo_setup_details"))
        .open(&mut open)
        .collapsible(false)
        .default_width(620.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(&setup.path).weak());
            if let Some(name) = &project_name {
                ui.label(format!("Project: {name}"));
            }
            if setup.last_sync_at > 0
                && let Some(at) = chrono::DateTime::from_timestamp(setup.last_sync_at, 0)
            {
                let local = at.with_timezone(&chrono::Local);
                ui.label(format!("Last sync: {}", local.format("%Y-%m-%d %H:%M")));
            }
            if !setup.last_summary.is_empty() {
                ui.label(egui::RichText::new(&setup.last_summary).weak());
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let mut auto = setup.auto_sync;
                if ui
                    .checkbox(&mut auto, "Keep in sync with the repository")
                    .on_hover_text(
                        "Check for git changes in the background and update connections, API folders and diagrams",
                    )
                    .changed()
                {
                    action = Some(DetailAction::ToggleAuto(auto));
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!running, egui::Button::new(if running { "Syncing…" } else { "Sync now" }))
                    .clicked()
                {
                    action = Some(DetailAction::Sync);
                }
                if pending_change.is_some()
                    && ui
                        .add(crate::diagram_repo::accent_button(ui, "Apply detected changes"))
                        .clicked()
                {
                    action = Some(DetailAction::ApplyChanges);
                }
                if setup.stale_count() > 0 && ui.button("Forget missing items").clicked() {
                    action = Some(DetailAction::ForgetStale);
                }
                if ui
                    .button("Stop managing")
                    .on_hover_text("Keep the connections, API folders and diagrams, but stop syncing them")
                    .clicked()
                {
                    action = Some(DetailAction::Remove);
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                if let Some(plan) = &pending_change {
                    ui.strong("Detected changes (not applied yet)");
                    plan_body(ui, plan);
                    ui.separator();
                }
                ui.strong(format!("Database connections ({})", setup.connections.len()));
                egui::Grid::new("repo_setup_conns")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        for c in &setup.connections {
                            ui.label(&c.name);
                            env_badge(ui, &c.environment);
                            let source = c.identity.split_once('|').map(|(_, s)| s).unwrap_or(&c.identity);
                            if c.stale {
                                ui.colored_label(ui.visuals().warn_fg_color, format!("{source}: no longer in the repository"));
                            } else if c.needs_credentials {
                                ui.colored_label(ui.visuals().warn_fg_color, format!("{source}: fill in the credentials"));
                            } else {
                                ui.label(egui::RichText::new(source).weak());
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.strong(format!("API folders ({})", setup.services.len()));
                egui::Grid::new("repo_setup_services")
                    .striped(true)
                    .num_columns(3)
                    .show(ui, |ui| {
                        for s in &setup.services {
                            ui.label(&s.name);
                            ui.label(format!("{} request(s)", s.endpoints));
                            let dir = if s.rel_dir.is_empty() { "." } else { s.rel_dir.as_str() };
                            if s.stale {
                                ui.colored_label(ui.visuals().warn_fg_color, format!("{dir}: no longer in the repository"));
                            } else {
                                ui.label(egui::RichText::new(dir).weak());
                            }
                            ui.end_row();
                        }
                    });
                if !setup.diagrams.is_empty() {
                    ui.add_space(6.0);
                    ui.strong("Diagrams");
                    for d in &setup.diagrams {
                        ui.label(format!("{}: {} group(s), {} table(s) placed", d.database, d.groups, d.tables));
                    }
                }
                for w in &setup.warnings {
                    ui.colored_label(ui.visuals().warn_fg_color, w);
                }
            });
        });
    if !open {
        t.repo_setup.details = None;
    }
    match action {
        Some(DetailAction::Sync) => sync_now(t, &key),
        Some(DetailAction::ApplyChanges) => {
            if let Some(plan) = t.repo_setup.changes.remove(&key) {
                let stats = apply_plan(t, &plan, false, None);
                t.toasts.success(stats.summary(&plan.name));
            }
        }
        Some(DetailAction::ToggleAuto(v)) => {
            if let Some(s) = t.repo_setup.store.get_mut(&key) {
                s.auto_sync = v;
            }
            if let Some(e) = t.repo_setup.save() {
                t.toasts.error(e);
            }
        }
        Some(DetailAction::ForgetStale) => {
            if let Some(s) = t.repo_setup.store.get_mut(&key) {
                s.connections.retain(|c| !c.stale);
                s.services.retain(|c| !c.stale);
            }
            if let Some(e) = t.repo_setup.save() {
                t.toasts.error(e);
            }
        }
        Some(DetailAction::Remove) => {
            t.repo_setup.store.remove(&key);
            t.repo_setup.changes.remove(&key);
            t.repo_setup.details = None;
            if let Some(e) = t.repo_setup.save() {
                t.toasts.error(e);
            }
            t.toasts
                .info("Repository setup removed. Connections, API folders and diagrams were kept.");
        }
        None => {}
    }
}
