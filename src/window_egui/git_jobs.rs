//! State tab Git di `Tabular`, job latar (git CLI & API GitHub/GitLab), dan
//! polling hasilnya per frame.
//!
//! Semua operasi git/jaringan berjalan di thread terpisah dan mengirim
//! [`JobResult`] lewat satu channel; UI tidak pernah menunggu proses git.
//! State tab tengah ([`GitTabState`]) hanya berisi data polos karena
//! `QueryTab` harus `Clone`; hal runtime (stream AI, handle pembatalan, cache
//! markdown) disimpan di [`GitUiState::reviews`] per id tab.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::agent::harness::{AgentEvent, CancelHandle};
use crate::git::branch::BranchInfo;
use crate::git::diff::DiffRow;
use crate::git::log::CommitInfo;
use crate::git::repos::{GitRepoStore, LinkKind, LinkSource, RepoEntry};
use crate::git::review::{
    self, ChangedFile, MergeMethod, MergeRequest, Provider, ProviderAccess, Recommendation,
};
use crate::git::status::{FileChange, RepoStatus};
use crate::git::{GitError, diff, log as gitlog, ops, status};
use crate::window_egui::Tabular;

/// Batas baris diff yang dirender sebelum user meminta "Show all".
pub const MAX_DIFF_ROWS: usize = 20_000;

// ─── State tab tengah ────────────────────────────────────────────────────────

/// Asal diff file lokal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSource {
    Worktree,
    Staged,
    Untracked,
    Conflict,
}

/// Isi tab Git di area tengah.
#[derive(Debug, Clone)]
pub enum GitView {
    FileDiff {
        repo: PathBuf,
        path: String,
        orig: Option<String>,
        source: DiffSource,
    },
    Commit {
        repo: PathBuf,
        commit: CommitInfo,
    },
    MergeRequest {
        mr: MergeRequest,
    },
}

/// Konfirmasi aksi MR yang tidak bisa dibatalkan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrConfirm {
    Merge,
    Close,
}

#[derive(Debug, Clone)]
pub struct GitTabState {
    pub view: GitView,
    pub loading: bool,
    pub error: Option<String>,
    /// Diff yang sedang ditampilkan (file lokal, file commit, atau file MR).
    pub rows: Vec<DiffRow>,
    pub binary: bool,
    pub side_by_side: bool,
    pub show_all_rows: bool,
    /// Commit: pesan lengkap dan file berubah.
    pub commit_message: String,
    pub commit_files: Vec<FileChange>,
    /// MR: file berubah dan detail yang sudah dilengkapi.
    pub mr_files: Vec<ChangedFile>,
    pub file_filter: String,
    pub selected_file: Option<usize>,
    pub comment: String,
    pub comment_preview: bool,
    pub merge_method: MergeMethod,
    pub merge_message: String,
    pub confirm: Option<MrConfirm>,
    /// Aksi jaringan MR yang sedang berjalan (merge/close/comment).
    pub busy: Option<String>,
    pub show_ai: bool,
}

impl GitTabState {
    pub fn new(view: GitView) -> Self {
        Self {
            view,
            loading: true,
            error: None,
            rows: Vec::new(),
            binary: false,
            side_by_side: true,
            show_all_rows: false,
            commit_message: String::new(),
            commit_files: Vec::new(),
            mr_files: Vec::new(),
            file_filter: String::new(),
            selected_file: None,
            comment: String::new(),
            comment_preview: false,
            merge_method: MergeMethod::default(),
            merge_message: String::new(),
            confirm: None,
            busy: None,
            show_ai: true,
        }
    }

    fn set_patch(&mut self, patch: &str) {
        self.binary = diff::is_binary_patch(patch);
        self.rows = if self.binary {
            Vec::new()
        } else {
            diff::parse_patch(patch)
        };
        self.show_all_rows = false;
    }
}

// ─── State sidebar ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSubMenu {
    Changes,
    Branches,
    History,
    Review,
}

impl GitSubMenu {
    pub fn key(self) -> &'static str {
        match self {
            GitSubMenu::Changes => "Changes",
            GitSubMenu::Branches => "Branches",
            GitSubMenu::History => "History",
            GitSubMenu::Review => "Review",
        }
    }

    pub fn from_key(key: &str) -> Self {
        match key {
            "Branches" => GitSubMenu::Branches,
            "History" => GitSubMenu::History,
            "Review" => GitSubMenu::Review,
            _ => GitSubMenu::Changes,
        }
    }
}

/// Konfirmasi aksi destruktif di sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarConfirm {
    DiscardTracked(Vec<String>),
    DiscardUntracked(Vec<String>),
    RemoveBranch {
        name: String,
        force: bool,
    },
    /// Checkout saat ada perubahan belum di-commit.
    Checkout {
        name: String,
        remote: bool,
    },
    RemoveRepo(String),
}

/// Dialog clone repository.
#[derive(Debug, Clone, Default)]
pub struct CloneDialog {
    pub url: String,
    pub dest: String,
    /// Kunci entri asal (tautan diagram/API diisi folder hasil clone).
    pub entry_key: Option<String>,
}

/// Satu proses review AI untuk tab MR.
pub struct ReviewRun {
    pub text: String,
    pub rx: Option<mpsc::Receiver<AgentEvent>>,
    pub cancel: Option<CancelHandle>,
    pub error: Option<String>,
    pub recommendation: Option<Recommendation>,
    pub backend_label: String,
    pub status_line: String,
    pub started: Instant,
    pub md_cache: egui_commonmark::CommonMarkCache,
}

impl ReviewRun {
    pub fn is_running(&self) -> bool {
        self.rx.is_some()
    }
}

/// Hasil job latar.
pub enum JobResult {
    Status {
        key: String,
        res: Result<RepoStatus, GitError>,
    },
    Branches {
        key: String,
        res: Result<Vec<BranchInfo>, GitError>,
    },
    Log {
        key: String,
        skip: usize,
        res: Result<Vec<CommitInfo>, GitError>,
    },
    /// Operasi yang mengubah repository selesai.
    Op {
        label: String,
        res: Result<String, GitError>,
    },
    Cloned {
        entry_key: Option<String>,
        dest: PathBuf,
        res: Result<(), GitError>,
    },
    Mrs {
        results: Vec<(Option<Provider>, Result<Vec<MergeRequest>, GitError>)>,
    },
    MrDetails {
        tab_id: usize,
        res: Result<(MergeRequest, Vec<ChangedFile>), GitError>,
    },
    MrAction {
        tab_id: usize,
        action: MrConfirm,
        res: Result<(), GitError>,
    },
    Comment {
        tab_id: usize,
        res: Result<(), GitError>,
    },
    Patch {
        tab_id: usize,
        res: Result<String, GitError>,
    },
    CommitDetail {
        tab_id: usize,
        res: Result<(String, Vec<FileChange>), GitError>,
    },
    CommitMessage(Result<String, String>),
}

pub struct GitUiState {
    pub store: GitRepoStore,
    pub sub: GitSubMenu,
    pub repos: Vec<RepoEntry>,
    pub repos_loaded: bool,
    pub status: Option<RepoStatus>,
    pub status_error: Option<String>,
    pub status_loading: bool,
    pub branches: Vec<BranchInfo>,
    pub commits: Vec<CommitInfo>,
    pub commits_done: bool,
    pub commits_loading: bool,
    pub commit_message: String,
    pub amend: bool,
    pub new_branch: String,
    pub branch_filter: String,
    pub history_filter: String,
    /// Operasi yang mengubah repository sedang berjalan.
    pub busy: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub confirm: Option<SidebarConfirm>,
    pub clone_dialog: Option<CloneDialog>,
    pub last_error: Option<String>,
    // Merge Review
    pub mrs: Vec<MergeRequest>,
    pub mr_errors: Vec<String>,
    pub mrs_loading: bool,
    pub mrs_loaded_at: Option<Instant>,
    /// Hanya MR repository aktif (bukan semua yang relevan untuk user).
    pub mrs_active_repo: bool,
    pub mr_filter: String,
    pub commit_ai_busy: bool,
    pub reviews: HashMap<usize, ReviewRun>,
    /// Cache markdown untuk deskripsi MR dan preview komentar.
    pub md_cache: egui_commonmark::CommonMarkCache,
    /// Draft token di Preferences (tidak pernah dirender ulang dari keychain).
    pub github_token_draft: String,
    pub gitlab_token_draft: String,
    /// Token & URL provider yang sudah dibaca dari keychain; `None` = baca ulang.
    access_cache: Option<ProviderAccess>,
    tx: mpsc::Sender<JobResult>,
    rx: mpsc::Receiver<JobResult>,
    pending: usize,
    was_focused: bool,
}

impl Default for GitUiState {
    fn default() -> Self {
        Self::new()
    }
}

impl GitUiState {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            store: GitRepoStore::load(GitRepoStore::default_file()),
            sub: GitSubMenu::Changes,
            repos: Vec::new(),
            repos_loaded: false,
            status: None,
            status_error: None,
            status_loading: false,
            branches: Vec::new(),
            commits: Vec::new(),
            commits_done: false,
            commits_loading: false,
            commit_message: String::new(),
            amend: false,
            new_branch: String::new(),
            branch_filter: String::new(),
            history_filter: String::new(),
            busy: None,
            cancel: Arc::new(AtomicBool::new(false)),
            confirm: None,
            clone_dialog: None,
            last_error: None,
            mrs: Vec::new(),
            mr_errors: Vec::new(),
            mrs_loading: false,
            mrs_loaded_at: None,
            mrs_active_repo: false,
            mr_filter: String::new(),
            commit_ai_busy: false,
            reviews: HashMap::new(),
            md_cache: Default::default(),
            github_token_draft: String::new(),
            gitlab_token_draft: String::new(),
            access_cache: None,
            tx,
            rx,
            pending: 0,
            was_focused: true,
        }
    }

    pub fn active(&self) -> Option<&RepoEntry> {
        let key = self.store.active.as_deref()?;
        self.repos.iter().find(|r| r.key == key)
    }

    pub fn active_key(&self) -> Option<String> {
        self.active().map(|r| r.key.clone())
    }

    /// Working tree repository aktif, bila ada di komputer ini.
    pub fn active_path(&self) -> Option<PathBuf> {
        self.active().and_then(|r| r.path.clone())
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    pub fn has_pending(&self) -> bool {
        self.pending > 0 || self.reviews.values().any(ReviewRun::is_running)
    }

    fn spawn(&mut self, job: impl FnOnce() -> JobResult + Send + 'static) {
        self.pending += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }

    pub fn save_store(&self) -> Result<(), String> {
        self.store.save().map_err(|e| {
            log::warn!("[GIT] cannot save {}: {e}", crate::git::repos::FILE_NAME);
            format!("Cannot save Git settings: {e}")
        })
    }

    /// Akses provider (token dari keychain), di-cache supaya keychain tidak
    /// dibaca setiap frame.
    pub fn provider_access(&mut self) -> ProviderAccess {
        if self.access_cache.is_none() {
            self.access_cache = Some(ProviderAccess::load(&self.store.settings.gitlab_url));
        }
        self.access_cache.clone().unwrap_or_default()
    }

    /// Baca ulang token/URL pada pemakaian berikutnya.
    pub fn invalidate_access(&mut self) {
        self.access_cache = None;
    }
}

// ─── Daftar repository ──────────────────────────────────────────────────────

fn collect_http_folders(
    folders: &[crate::http_collection::HttpFolder],
    workspace: &str,
    out: &mut Vec<LinkSource>,
) {
    for f in folders {
        if f.has_repository() {
            out.push(LinkSource {
                kind: LinkKind::HttpFolder,
                id: f.id.clone(),
                label: format!("{workspace} / {}", f.name),
                url: f.shared_repo_url().map(str::to_string),
                path: f.local_repo_path(),
            });
        }
        collect_http_folders(&f.children, workspace, out);
    }
}

/// Kumpulkan semua pemakai repository: group diagram (tab terbuka dan file
/// tersimpan), folder HTTP API, project, dan folder yang ditambahkan di Git.
pub fn collect_sources(t: &Tabular) -> Vec<LinkSource> {
    let mut out = Vec::new();
    let mut seen_groups = std::collections::HashSet::new();
    let mut add_state =
        |db: &str, state: &crate::models::structs::DiagramState, out: &mut Vec<LinkSource>| {
            for g in &state.groups {
                if !g.has_repository() || !seen_groups.insert(g.id.clone()) {
                    continue;
                }
                out.push(LinkSource {
                    kind: LinkKind::Diagram,
                    id: g.id.clone(),
                    label: format!("{} ({db})", g.title),
                    url: g.shared_repo_url().map(str::to_string),
                    path: g.local_repo_path(),
                });
            }
        };
    for tab in &t.query_tabs {
        if let (Some(db), Some(state)) = (tab.database_name.as_deref(), tab.diagram_state.as_ref())
        {
            add_state(db, state, &mut out);
        }
    }
    if let Some(dir) = t
        .get_diagram_path(0, "_")
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        for (_, db, _, state) in crate::repo_links::load_diagram_files(&dir) {
            add_state(&db, &state, &mut out);
        }
    }
    for ws in &t.yaak_workspaces {
        collect_http_folders(&ws.folders, &ws.name, &mut out);
    }
    for p in &t.projects.list {
        if let Some(url) = p.repo_url.as_deref().filter(|u| !u.trim().is_empty()) {
            out.push(LinkSource {
                kind: LinkKind::Project,
                id: p.id.clone(),
                label: p.name.clone(),
                url: Some(url.to_string()),
                path: None,
            });
        }
    }
    for m in &t.git.store.repos {
        out.push(LinkSource {
            kind: LinkKind::Manual,
            id: m.path.clone(),
            label: m.path.clone(),
            url: None,
            path: Some(m.path.clone()),
        });
    }
    out
}

/// Susun ulang daftar repository dan pilih repository aktif.
pub fn reload_repos(t: &mut Tabular) {
    let sources = collect_sources(t);
    let repos = crate::git::repos::merge(&sources, crate::repo_scan::git_remote_url);
    let git = &mut t.git;
    git.repos = repos;
    git.repos_loaded = true;
    let active_ok = git
        .store
        .active
        .as_deref()
        .is_some_and(|k| git.repos.iter().any(|r| r.key == k));
    if !active_ok {
        let first = git
            .repos
            .iter()
            .find(|r| r.path.is_some())
            .map(|r| r.key.clone());
        git.store.active = first;
    }
    refresh_all(t);
}

pub fn select_repo(t: &mut Tabular, key: &str) {
    if t.git.store.active.as_deref() == Some(key) {
        return;
    }
    t.git.store.active = Some(key.to_string());
    if let Err(e) = t.git.save_store() {
        t.toasts.error(e);
    }
    let git = &mut t.git;
    git.status = None;
    git.status_error = None;
    git.branches.clear();
    git.commits.clear();
    git.commits_done = false;
    git.commit_message.clear();
    git.amend = false;
    if git.mrs_active_repo {
        git.mrs.clear();
        git.mrs_loaded_at = None;
    }
    refresh_all(t);
}

/// Tambah folder lokal (harus berada di dalam repository git).
pub fn add_folder(t: &mut Tabular, dir: &Path) {
    let root = match ops::discover_root(dir) {
        Ok(r) => r,
        Err(GitError::NotARepo(_)) | Err(GitError::Command { .. }) => {
            t.toasts.error(format!(
                "{} is not inside a git repository. Use \"Init repository\" to create one.",
                dir.display()
            ));
            return;
        }
        Err(e) => {
            t.toasts.error(e.to_string());
            return;
        }
    };
    let root_s = root.to_string_lossy().to_string();
    t.git.store.add(&root_s);
    reload_repos(t);
    let key = t
        .git
        .repos
        .iter()
        .find(|r| r.path.as_ref().is_some_and(|p| same_dir(p, &root)))
        .map(|r| r.key.clone());
    if let Some(k) = key {
        select_repo(t, &k);
    }
    if let Err(e) = t.git.save_store() {
        t.toasts.error(e);
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

pub fn init_repo(t: &mut Tabular, dir: PathBuf) {
    match ops::init(&dir) {
        Ok(()) => {
            t.toasts
                .success(format!("Initialized git repository in {}", dir.display()));
            add_folder(t, &dir);
        }
        Err(e) => t.toasts.error(e.to_string()),
    }
}

pub fn remove_repo(t: &mut Tabular, key: &str) {
    let Some(entry) = t.git.repos.iter().find(|r| r.key == key).cloned() else {
        return;
    };
    for link in entry.links.iter().filter(|l| l.kind == LinkKind::Manual) {
        t.git.store.remove(&link.id);
    }
    if t.git.store.active.as_deref() == Some(key) {
        t.git.store.active = None;
    }
    if let Err(e) = t.git.save_store() {
        t.toasts.error(e);
    }
    reload_repos(t);
}

/// Pakai folder repository ini untuk group diagram / folder API tertaut yang
/// belum punya folder di komputer ini.
pub fn link_folder_to_items(t: &mut Tabular, key: &str) {
    let Some(entry) = t.git.repos.iter().find(|r| r.key == key).cloned() else {
        return;
    };
    let Some(path) = entry.path.clone() else {
        return;
    };
    match crate::git::repos::assign_folder(&entry, &path) {
        Ok(0) => {}
        Ok(n) => {
            t.toasts
                .success(format!("Project folder set for {n} linked item(s)"));
            reload_repos(t);
        }
        Err(e) => t.toasts.error(e),
    }
}

// ─── Refresh status/branch/log ──────────────────────────────────────────────

pub fn refresh_all(t: &mut Tabular) {
    refresh_status(t);
    refresh_branches(t);
    refresh_log(t, true);
}

pub fn refresh_status(t: &mut Tabular) {
    let git = &mut t.git;
    let (Some(key), Some(path)) = (git.active_key(), git.active_path()) else {
        git.status = None;
        return;
    };
    git.status_loading = true;
    git.spawn(move || JobResult::Status {
        key,
        res: status::read(&path),
    });
}

pub fn refresh_branches(t: &mut Tabular) {
    let git = &mut t.git;
    let (Some(key), Some(path)) = (git.active_key(), git.active_path()) else {
        return;
    };
    git.spawn(move || JobResult::Branches {
        key,
        res: crate::git::branch::list(&path),
    });
}

/// Muat riwayat; `reset` memulai dari commit terbaru.
pub fn refresh_log(t: &mut Tabular, reset: bool) {
    let git = &mut t.git;
    let (Some(key), Some(path)) = (git.active_key(), git.active_path()) else {
        return;
    };
    if git.commits_loading {
        return;
    }
    let skip = if reset { 0 } else { git.commits.len() };
    git.commits_loading = true;
    git.spawn(move || JobResult::Log {
        key,
        skip,
        res: gitlog::page(&path, None, skip, gitlog::PAGE_SIZE),
    });
}

// ─── Operasi yang mengubah repository ───────────────────────────────────────

/// Jalankan operasi di repository aktif. Hanya satu operasi pada satu waktu.
pub fn run_op(
    t: &mut Tabular,
    label: &str,
    op: impl FnOnce(&Path, &AtomicBool) -> Result<String, GitError> + Send + 'static,
) {
    let git = &mut t.git;
    let Some(path) = git.active_path() else {
        t.toasts
            .error("Select a repository with a local folder first");
        return;
    };
    if git.is_busy() {
        t.toasts.info("Another git operation is still running");
        return;
    }
    git.busy = Some(label.to_string());
    git.last_error = None;
    git.cancel.store(false, Ordering::SeqCst);
    let cancel = git.cancel.clone();
    let label = label.to_string();
    log::info!("[GIT] {label} in {}", path.display());
    git.spawn(move || JobResult::Op {
        res: op(&path, &cancel),
        label,
    });
}

pub fn cancel_op(t: &mut Tabular) {
    t.git.cancel.store(true, Ordering::SeqCst);
}

pub fn stage(t: &mut Tabular, paths: Vec<String>) {
    run_op(t, "Stage", move |p, _| {
        ops::stage(p, &paths).map(|_| String::new())
    });
}

pub fn unstage(t: &mut Tabular, paths: Vec<String>) {
    let has_head = t.git.status.as_ref().is_some_and(|s| s.head_oid.is_some());
    run_op(t, "Unstage", move |p, _| {
        ops::unstage(p, &paths, has_head).map(|_| String::new())
    });
}

pub fn commit(t: &mut Tabular) {
    let msg = t.git.commit_message.trim().to_string();
    let amend = t.git.amend;
    if msg.is_empty() && !amend {
        t.toasts.error("Enter a commit message");
        return;
    }
    let st = t.git.status.clone().unwrap_or_default();
    let stage_all = st.staged.is_empty() && !amend;
    if stage_all && st.unstaged.is_empty() && st.untracked.is_empty() {
        t.toasts.info("Nothing to commit");
        return;
    }
    run_op(t, "Commit", move |p, _| {
        if stage_all {
            // Seperti VS Code: tanpa file staged, commit semua perubahan.
            ops::stage_all(p)?;
        }
        let msg = if msg.is_empty() {
            gitlog::message(p, "HEAD")?
        } else {
            msg
        };
        ops::commit(p, &msg, amend)
    });
}

pub fn fetch(t: &mut Tabular) {
    run_op(t, "Fetch", |p, c| ops::fetch(p, c).map(|_| String::new()));
}

pub fn pull(t: &mut Tabular) {
    let rebase = t.git.store.settings.pull_rebase;
    run_op(t, "Pull", move |p, c| {
        ops::pull(p, rebase, c).map(|_| String::new())
    });
}

pub fn push(t: &mut Tabular) {
    let Some(st) = t.git.status.clone() else {
        return;
    };
    let Some(branch) = st.branch.clone() else {
        t.toasts.error("Cannot push a detached HEAD");
        return;
    };
    let has_upstream = st.upstream.is_some();
    run_op(t, "Push", move |p, c| {
        ops::push(p, &branch, has_upstream, c).map(|_| String::new())
    });
}

pub fn checkout(t: &mut Tabular, name: String, remote: bool) {
    run_op(t, "Checkout", move |p, _| {
        if remote {
            ops::checkout_remote(p, &name)
        } else {
            ops::checkout(p, &name)
        }
        .map(|_| String::new())
    });
}

pub fn create_branch(t: &mut Tabular) {
    let name = t.git.new_branch.trim().to_string();
    let Some(path) = t.git.active_path() else {
        return;
    };
    if !ops::is_valid_branch_name(&path, &name) {
        t.toasts
            .error(format!("\"{name}\" is not a valid branch name"));
        return;
    }
    t.git.new_branch.clear();
    run_op(t, "Create branch", move |p, _| {
        ops::create_branch(p, &name, true).map(|_| String::new())
    });
}

pub fn confirm_sidebar(t: &mut Tabular, c: SidebarConfirm) {
    match c {
        SidebarConfirm::DiscardTracked(paths) => run_op(t, "Discard changes", move |p, _| {
            ops::discard_tracked(p, &paths).map(|_| String::new())
        }),
        SidebarConfirm::DiscardUntracked(paths) => {
            run_op(t, "Discard untracked files", move |p, _| {
                ops::discard_untracked(p, &paths).map(|_| String::new())
            })
        }
        SidebarConfirm::RemoveBranch { name, force } => run_op(t, "Remove branch", move |p, _| {
            ops::remove_branch(p, &name, force).map(|_| String::new())
        }),
        SidebarConfirm::Checkout { name, remote } => checkout(t, name, remote),
        SidebarConfirm::RemoveRepo(key) => remove_repo(t, &key),
    }
}

pub fn start_clone(t: &mut Tabular, dlg: CloneDialog) {
    let url = dlg.url.trim().to_string();
    let dest = crate::repo_scan::expand_home(dlg.dest.trim());
    if url.is_empty() || dlg.dest.trim().is_empty() {
        t.toasts
            .error("Enter a repository URL and a destination folder");
        return;
    }
    if crate::repo_scan::has_embedded_credentials(&url) {
        t.toasts.error(
            "Remove the credentials from the URL; git uses your credential helper or SSH key.",
        );
        return;
    }
    let git = &mut t.git;
    if git.is_busy() {
        t.toasts.info("Another git operation is still running");
        return;
    }
    git.busy = Some("Clone".to_string());
    git.cancel.store(false, Ordering::SeqCst);
    let cancel = git.cancel.clone();
    let entry_key = dlg.entry_key.clone();
    log::info!(
        "[GIT] clone {} into {}",
        crate::repo_scan::redact(&url),
        dest.display()
    );
    git.spawn(move || JobResult::Cloned {
        res: ops::clone(&url, &dest, &cancel),
        entry_key,
        dest,
    });
}

// ─── AI pesan commit ────────────────────────────────────────────────────────

pub fn generate_commit_message(t: &mut Tabular) {
    let Some(path) = t.git.active_path() else {
        return;
    };
    let target = t.effective_default_target();
    if let Err(e) = crate::ai_assistant::backend_ready_for(t, target) {
        t.toasts.error(format!("AI is not configured: {e}"));
        return;
    }
    let cfg = crate::ai_assistant::chat_backend_for(t, target);
    let branch = t.git.status.as_ref().and_then(|s| s.branch.clone());
    t.git.commit_ai_busy = true;
    t.git.spawn(move || {
        let res = (|| {
            let mut staged = diff::staged_all(&path).map_err(|e| e.to_string())?;
            if staged.trim().is_empty() {
                // Tanpa file staged, commit akan memakai semua perubahan.
                staged = crate::git::cli::run_text(
                    &path,
                    &["diff", "--no-color", "--no-ext-diff", "HEAD"],
                )
                .map_err(|e| e.to_string())?;
            }
            if staged.trim().is_empty() {
                return Err("No changes to describe".to_string());
            }
            let rx = crate::ai_assistant::request_text(
                &cfg,
                review::prompt::commit_message_system_prompt(),
                review::prompt::commit_message_user_prompt(&staged, branch.as_deref()),
            );
            rx.recv()
                .map_err(|_| "AI backend stopped without a reply".to_string())?
                .map(|m| review::prompt::clean_commit_message(&m))
        })();
        JobResult::CommitMessage(res)
    });
}

// ─── Tab tengah ─────────────────────────────────────────────────────────────

fn find_tab(t: &Tabular, pred: impl Fn(&GitTabState) -> bool) -> Option<usize> {
    t.query_tabs
        .iter()
        .position(|tab| tab.git_state.as_ref().is_some_and(&pred))
}

fn tab_by_id(t: &mut Tabular, tab_id: usize) -> Option<&mut GitTabState> {
    t.query_tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .and_then(|tab| tab.git_state.as_mut())
}

/// Buka tab Git baru (atau pakai ulang tab `reuse`) dan kembalikan id-nya.
fn open_tab(t: &mut Tabular, reuse: Option<usize>, title: String, state: GitTabState) -> usize {
    if let Some(idx) = reuse {
        crate::editor::switch_to_tab(t, idx);
        if let Some(tab) = t.query_tabs.get_mut(idx) {
            tab.title = title;
            tab.git_state = Some(state);
            return tab.id;
        }
    }
    crate::editor::create_new_tab(t, title, String::new());
    let idx = t.active_tab_index;
    match t.query_tabs.get_mut(idx) {
        Some(tab) => {
            tab.git_state = Some(state);
            tab.is_table_browse_mode = false;
            tab.id
        }
        None => 0,
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Buka diff file lokal. Satu tab "preview" dipakai ulang seperti VS Code.
pub fn open_file_diff(t: &mut Tabular, change: &FileChange, source: DiffSource) {
    let Some(repo) = t.git.active_path() else {
        return;
    };
    let reuse = find_tab(t, |s| matches!(s.view, GitView::FileDiff { .. }));
    let suffix = match source {
        DiffSource::Staged => " (Index)",
        DiffSource::Untracked => " (Untracked)",
        DiffSource::Conflict => " (Conflict)",
        DiffSource::Worktree => "",
    };
    let title = format!(
        "{} {}{suffix}",
        egui_icons::icons::ICON_DIFFERENCE.codepoint,
        file_name(&change.path)
    );
    let view = GitView::FileDiff {
        repo: repo.clone(),
        path: change.path.clone(),
        orig: change.orig_path.clone(),
        source,
    };
    let tab_id = open_tab(t, reuse, title, GitTabState::new(view));
    let path = change.path.clone();
    let orig = change.orig_path.clone();
    t.git.spawn(move || JobResult::Patch {
        tab_id,
        res: match source {
            DiffSource::Staged => diff::staged(&repo, &path, orig.as_deref()),
            DiffSource::Untracked => diff::untracked(&repo, &path),
            DiffSource::Worktree | DiffSource::Conflict => diff::worktree(&repo, &path),
        },
    });
}

/// Buka detail commit (pesan + daftar file) di tab tengah.
pub fn open_commit(t: &mut Tabular, commit: &CommitInfo) {
    let Some(repo) = t.git.active_path() else {
        return;
    };
    let reuse = find_tab(t, |s| matches!(s.view, GitView::Commit { .. }));
    let title = format!(
        "{} {}",
        egui_icons::icons::ICON_COMMIT.codepoint,
        commit.short
    );
    let view = GitView::Commit {
        repo: repo.clone(),
        commit: commit.clone(),
    };
    let tab_id = open_tab(t, reuse, title, GitTabState::new(view));
    let hash = commit.hash.clone();
    let parent = commit.parents.first().cloned();
    t.git.spawn(move || JobResult::CommitDetail {
        tab_id,
        res: gitlog::message(&repo, &hash)
            .and_then(|m| gitlog::commit_files(&repo, &hash, parent.as_deref()).map(|f| (m, f))),
    });
}

/// Muat diff satu file di tab commit.
pub fn load_commit_file(t: &mut Tabular, tab_id: usize, index: usize) {
    let Some(st) = t
        .query_tabs
        .iter_mut()
        .find(|tab| tab.id == tab_id)
        .and_then(|tab| tab.git_state.as_mut())
    else {
        return;
    };
    load_commit_file_for(&mut t.git, tab_id, st, index);
}

/// Seperti [`load_commit_file`] untuk state yang sedang dirender.
pub fn load_commit_file_for(
    git: &mut GitUiState,
    tab_id: usize,
    st: &mut GitTabState,
    index: usize,
) {
    let GitView::Commit { repo, commit } = &st.view else {
        return;
    };
    let Some(file) = st.commit_files.get(index).cloned() else {
        return;
    };
    let (repo, hash, parent) = (
        repo.clone(),
        commit.hash.clone(),
        commit.parents.first().cloned(),
    );
    st.selected_file = Some(index);
    st.loading = true;
    st.error = None;
    st.rows.clear();
    git.spawn(move || JobResult::Patch {
        tab_id,
        res: diff::commit_file(
            &repo,
            &hash,
            parent.as_deref(),
            &file.path,
            file.orig_path.as_deref(),
        ),
    });
}

/// Buka merge request; satu tab per MR.
pub fn open_merge_request(t: &mut Tabular, mr: &MergeRequest) {
    let key = mr.key();
    if let Some(idx) = find_tab(
        t,
        |s| matches!(&s.view, GitView::MergeRequest { mr: m } if m.key() == key),
    ) {
        crate::editor::switch_to_tab(t, idx);
        return;
    }
    let title = format!(
        "{} {} {}",
        egui_icons::icons::ICON_SOURCE_PULL.codepoint,
        mr.display_number(),
        short_title(&mr.title, 28)
    );
    let tab_id = open_tab(
        t,
        None,
        title,
        GitTabState::new(GitView::MergeRequest { mr: mr.clone() }),
    );
    with_tab_state(t, tab_id, |t, st| reload_merge_request(t, tab_id, st));
}

pub fn reload_merge_request(t: &mut Tabular, tab_id: usize, st: &mut GitTabState) {
    let GitView::MergeRequest { mr } = &st.view else {
        return;
    };
    let access = t.git.provider_access();
    let mr = mr.clone();
    st.loading = true;
    st.error = None;
    t.git.spawn(move || JobResult::MrDetails {
        tab_id,
        res: review::load_details(&access, &mr),
    });
}

/// Jalankan `f` dengan state tab `tab_id` yang dikeluarkan sementara dari tab.
pub fn with_tab_state(
    t: &mut Tabular,
    tab_id: usize,
    f: impl FnOnce(&mut Tabular, &mut GitTabState),
) {
    let Some(mut st) = t
        .query_tabs
        .iter_mut()
        .find(|q| q.id == tab_id)
        .and_then(|q| q.git_state.take())
    else {
        return;
    };
    f(t, &mut st);
    if let Some(tab) = t.query_tabs.iter_mut().find(|q| q.id == tab_id) {
        tab.git_state = Some(st);
    }
}

fn short_title(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

/// Pilih file di tab MR dan tampilkan diff-nya (tanpa jaringan: patch sudah ada).
pub fn select_mr_file(st: &mut GitTabState, index: usize) {
    let patch = st.mr_files.get(index).and_then(|f| f.patch.clone());
    st.selected_file = Some(index);
    match patch {
        Some(p) => st.set_patch(&p),
        None => {
            st.rows.clear();
            st.binary = true;
        }
    }
}

pub fn mr_action(t: &mut Tabular, tab_id: usize, st: &mut GitTabState, action: MrConfirm) {
    let GitView::MergeRequest { mr } = &st.view else {
        return;
    };
    let access = t.git.provider_access();
    let (mr, method, message) = (mr.clone(), st.merge_method, st.merge_message.clone());
    st.busy = Some(match action {
        MrConfirm::Merge => "Merging…".to_string(),
        MrConfirm::Close => "Closing…".to_string(),
    });
    log::info!(
        "[GIT] {:?} {} {}",
        action,
        mr.repo_full_name,
        mr.display_number()
    );
    t.git.spawn(move || JobResult::MrAction {
        tab_id,
        action,
        res: match action {
            MrConfirm::Merge => review::merge(&access, &mr, method, &message),
            MrConfirm::Close => review::close(&access, &mr),
        },
    });
}

pub fn post_comment(t: &mut Tabular, tab_id: usize, st: &mut GitTabState, body: String) {
    let GitView::MergeRequest { mr } = &st.view else {
        return;
    };
    let access = t.git.provider_access();
    let mr = mr.clone();
    st.busy = Some("Posting comment…".to_string());
    t.git.spawn(move || JobResult::Comment {
        tab_id,
        res: review::comment(&access, &mr, &body),
    });
}

// ─── Merge Review: daftar & AI ──────────────────────────────────────────────

/// Muat daftar MR sesuai mode (semua yang relevan untuk user, atau repository aktif).
pub fn load_mrs(t: &mut Tabular) {
    let access = t.git.provider_access();
    let git = &mut t.git;
    if git.mrs_loading {
        return;
    }
    if access.github_token.is_none() && access.gitlab_token.is_none() {
        git.mrs.clear();
        git.mr_errors.clear();
        git.mrs_loaded_at = Some(Instant::now());
        return;
    }
    let include_closed = git.store.settings.show_closed;
    let remote = if git.mrs_active_repo {
        match git.active() {
            Some(entry) => {
                match review::remote_from_key(&entry.key, &access.gitlab_host()) {
                    Some(r) => Some(r),
                    None => {
                        git.mrs.clear();
                        git.mr_errors =
                        vec!["The active repository is not hosted on GitHub or the configured GitLab.".to_string()];
                        git.mrs_loaded_at = Some(Instant::now());
                        return;
                    }
                }
            }
            None => {
                git.mrs.clear();
                git.mr_errors = vec!["Select a repository first.".to_string()];
                return;
            }
        }
    } else {
        None
    };
    git.mrs_loading = true;
    git.spawn(move || {
        let results = match remote {
            Some(r) => vec![(
                Some(r.provider),
                review::list_repo(&access, &r, include_closed),
            )],
            None => review::list_mine(&access)
                .into_iter()
                .map(|(p, r)| (Some(p), r))
                .collect(),
        };
        JobResult::Mrs { results }
    });
}

/// Mulai review AI untuk tab MR memakai backend AI Tabular.
pub fn start_review(t: &mut Tabular, tab_id: usize, st: &mut GitTabState) {
    let GitView::MergeRequest { mr } = &st.view else {
        return;
    };
    if st.mr_files.is_empty() {
        t.toasts.info("Wait until the changed files are loaded");
        return;
    }
    let target = t.effective_default_target();
    if let Err(e) = crate::ai_assistant::backend_ready_for(t, target) {
        t.toasts.error(format!("AI is not configured: {e}"));
        return;
    }
    let cfg = crate::ai_assistant::chat_backend_for(t, target);
    let backend_label = crate::ai_assistant::backend_label_for(t, target);
    let language = t.git.store.settings.review_language.clone();
    let system = review::prompt::review_system_prompt(&language);
    let user = review::prompt::review_user_prompt(mr, &st.mr_files);
    st.show_ai = true;
    cancel_review(t, tab_id);
    // Review tidak butuh tool atau MCP: seluruh diff ada di prompt.
    let workspace = crate::ai_assistant::ChatWorkspace {
        cwd: None,
        allowed_tools: Vec::new(),
        without_mcp: true,
    };
    let (rx, cancel, error) =
        match crate::ai_assistant::start_chat_in(&cfg, system, user, None, workspace) {
            Ok((rx, cancel)) => (Some(rx), cancel, None),
            Err(e) => (None, None, Some(e)),
        };
    log::info!(
        "[GIT] AI review of {} {} via {backend_label}",
        mr.repo_full_name,
        mr.display_number()
    );
    t.git.reviews.insert(
        tab_id,
        ReviewRun {
            text: String::new(),
            status_line: if rx.is_some() {
                "Starting…".to_string()
            } else {
                String::new()
            },
            rx,
            cancel,
            error,
            recommendation: None,
            backend_label,
            started: Instant::now(),
            md_cache: Default::default(),
        },
    );
}

pub fn cancel_review(t: &mut Tabular, tab_id: usize) {
    if let Some(run) = t.git.reviews.get_mut(&tab_id) {
        if let Some(c) = run.cancel.take() {
            c.cancel();
        }
        if run.rx.take().is_some() {
            run.status_line = "Cancelled".to_string();
        }
    }
}

fn poll_reviews(git: &mut GitUiState) {
    for run in git.reviews.values_mut() {
        let Some(rx) = &run.rx else {
            continue;
        };
        let mut finished = false;
        loop {
            match rx.try_recv() {
                Ok(AgentEvent::TextDelta(d)) => run.text.push_str(&d),
                Ok(AgentEvent::Progress(p)) => run.status_line = p.description,
                Ok(AgentEvent::ToolUse(name)) => run.status_line = format!("Using {name}…"),
                Ok(AgentEvent::Session(_)) => {}
                Ok(AgentEvent::Done { text, .. }) => {
                    if run.text.trim().is_empty() {
                        run.text = text;
                    }
                    run.status_line = format!("Done in {}s", run.started.elapsed().as_secs());
                    finished = true;
                    break;
                }
                Ok(AgentEvent::Error(e)) => {
                    run.error = Some(e);
                    run.status_line.clear();
                    finished = true;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if run.text.trim().is_empty() && run.error.is_none() {
                        run.error = Some("AI backend stopped without a reply".to_string());
                    }
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            run.rx = None;
            run.cancel = None;
            run.recommendation = review::prompt::parse_recommendation(&run.text);
        }
    }
}

// ─── Polling per frame ──────────────────────────────────────────────────────

fn describe(e: &GitError) -> String {
    match e {
        GitError::Auth { action, detail } => format!(
            "git {action} needs credentials. Configure a credential helper or SSH key in a terminal, then try again.\n{detail}"
        ),
        other => other.to_string(),
    }
}

/// Terapkan hasil job dan jalankan refresh berkala. Dipanggil sekali per frame.
pub fn poll(t: &mut Tabular, ctx: &egui::Context) {
    let mut results = Vec::new();
    while let Ok(r) = t.git.rx.try_recv() {
        t.git.pending = t.git.pending.saturating_sub(1);
        results.push(r);
    }
    for r in results {
        apply(t, r);
    }
    poll_reviews(&mut t.git);

    // Hentikan review milik tab yang sudah ditutup.
    let live: std::collections::HashSet<usize> = t.query_tabs.iter().map(|q| q.id).collect();
    t.git.reviews.retain(|id, run| {
        let keep = live.contains(id);
        if !keep && let Some(c) = run.cancel.take() {
            c.cancel();
        }
        keep
    });

    // Refresh status saat jendela kembali fokus (file mungkin diubah di luar).
    let focused = ctx.input(|i| i.focused);
    if focused && !t.git.was_focused && t.selected_menu == "Git" && !t.git.is_busy() {
        refresh_status(t);
    }
    t.git.was_focused = focused;

    // Auto-refresh daftar MR setelah tab Git pernah dibuka.
    let minutes = t.git.store.settings.auto_refresh_min;
    if minutes > 0
        && t.git.repos_loaded
        && !t.git.mrs_loading
        && t.git
            .mrs_loaded_at
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(u64::from(minutes) * 60))
    {
        load_mrs(t);
    }

    if t.git.has_pending() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn apply(t: &mut Tabular, r: JobResult) {
    let active = t.git.active_key();
    match r {
        JobResult::Status { key, res } => {
            if active.as_deref() != Some(&key) {
                return;
            }
            let git = &mut t.git;
            git.status_loading = false;
            match res {
                Ok(s) => {
                    git.status = Some(s);
                    git.status_error = None;
                }
                Err(e) => {
                    git.status = None;
                    git.status_error = Some(describe(&e));
                }
            }
        }
        JobResult::Branches { key, res } => {
            if active.as_deref() != Some(&key) {
                return;
            }
            match res {
                Ok(b) => t.git.branches = b,
                Err(e) => log::warn!("[GIT] branch list failed: {e}"),
            }
        }
        JobResult::Log { key, skip, res } => {
            t.git.commits_loading = false;
            if active.as_deref() != Some(&key) {
                return;
            }
            match res {
                Ok(page) => {
                    let git = &mut t.git;
                    git.commits_done = page.len() < gitlog::PAGE_SIZE;
                    if skip == 0 {
                        git.commits = page;
                    } else {
                        git.commits.extend(page);
                    }
                }
                Err(e) => log::warn!("[GIT] log failed: {e}"),
            }
        }
        JobResult::Op { label, res } => {
            t.git.busy = None;
            match res {
                Ok(_) => {
                    if label == "Commit" {
                        t.git.commit_message.clear();
                        t.git.amend = false;
                    }
                    if matches!(
                        label.as_str(),
                        "Push" | "Pull" | "Fetch" | "Commit" | "Checkout"
                    ) {
                        t.toasts.success(format!("{label} completed"));
                    }
                }
                Err(GitError::Cancelled) => t.toasts.info(format!("{label} cancelled")),
                Err(e) => {
                    let msg = describe(&e);
                    log::warn!("[GIT] {label} failed: {msg}");
                    t.git.last_error = Some(msg.clone());
                    t.toasts.error(format!("{label} failed: {msg}"));
                }
            }
            refresh_all(t);
        }
        JobResult::Cloned {
            entry_key,
            dest,
            res,
        } => {
            t.git.busy = None;
            match res {
                Ok(()) => {
                    t.toasts.success(format!("Cloned into {}", dest.display()));
                    t.git.store.add(&dest.to_string_lossy());
                    if let Err(e) = t.git.save_store() {
                        t.toasts.error(e);
                    }
                    reload_repos(t);
                    // Isi folder untuk group diagram / folder API yang menunjuk URL ini.
                    let key = entry_key.or_else(|| {
                        t.git
                            .repos
                            .iter()
                            .find(|r| r.path.as_ref().is_some_and(|p| same_dir(p, &dest)))
                            .map(|r| r.key.clone())
                    });
                    if let Some(k) = key {
                        link_folder_to_items(t, &k);
                        select_repo(t, &k);
                    }
                }
                Err(GitError::Cancelled) => t.toasts.info("Clone cancelled"),
                Err(e) => t.toasts.error(format!("Clone failed: {}", describe(&e))),
            }
        }
        JobResult::Mrs { results } => {
            let git = &mut t.git;
            git.mrs_loading = false;
            git.mrs_loaded_at = Some(Instant::now());
            git.mrs.clear();
            git.mr_errors.clear();
            for (provider, res) in results {
                match res {
                    Ok(list) => git.mrs.extend(list),
                    Err(e) => {
                        let label = provider.map(Provider::label).unwrap_or("Review");
                        log::warn!("[GIT] {label} merge requests failed: {e}");
                        git.mr_errors.push(format!("{label}: {e}"));
                    }
                }
            }
        }
        JobResult::MrDetails { tab_id, res } => {
            let Some(st) = tab_by_id(t, tab_id) else {
                return;
            };
            st.loading = false;
            match res {
                Ok((mr, files)) => {
                    st.view = GitView::MergeRequest { mr };
                    st.mr_files = files;
                    st.error = None;
                    if !st.mr_files.is_empty() {
                        select_mr_file(st, 0);
                    }
                }
                Err(e) => st.error = Some(e.to_string()),
            }
        }
        JobResult::MrAction {
            tab_id,
            action,
            res,
        } => {
            let mut ok = false;
            if let Some(st) = tab_by_id(t, tab_id) {
                st.busy = None;
                st.confirm = None;
                match &res {
                    Ok(()) => {
                        ok = true;
                        if let GitView::MergeRequest { mr } = &mut st.view {
                            mr.state = match action {
                                MrConfirm::Merge => review::MrState::Merged,
                                MrConfirm::Close => review::MrState::Closed,
                            };
                        }
                    }
                    Err(e) => st.error = Some(e.to_string()),
                }
            }
            match res {
                Ok(()) => t.toasts.success(match action {
                    MrConfirm::Merge => "Merge request merged",
                    MrConfirm::Close => "Merge request closed",
                }),
                Err(e) => t.toasts.error(format!("Action failed: {e}")),
            }
            if ok {
                load_mrs(t);
            }
        }
        JobResult::Comment { tab_id, res } => {
            let ok = res.is_ok();
            if let Some(st) = tab_by_id(t, tab_id) {
                st.busy = None;
                if ok {
                    st.comment.clear();
                    st.comment_preview = false;
                }
            }
            match res {
                Ok(()) => t.toasts.success("Comment posted"),
                Err(e) => t.toasts.error(format!("Comment failed: {e}")),
            }
        }
        JobResult::Patch { tab_id, res } => {
            let Some(st) = tab_by_id(t, tab_id) else {
                return;
            };
            st.loading = false;
            match res {
                Ok(p) => {
                    st.error = None;
                    st.set_patch(&p);
                }
                Err(e) => st.error = Some(e.to_string()),
            }
        }
        JobResult::CommitDetail { tab_id, res } => {
            let mut first = false;
            if let Some(st) = tab_by_id(t, tab_id) {
                st.loading = false;
                match res {
                    Ok((msg, files)) => {
                        st.commit_message = msg;
                        first = !files.is_empty();
                        st.commit_files = files;
                    }
                    Err(e) => st.error = Some(e.to_string()),
                }
            }
            if first {
                load_commit_file(t, tab_id, 0);
            }
        }
        JobResult::CommitMessage(res) => {
            t.git.commit_ai_busy = false;
            match res {
                Ok(m) if !m.is_empty() => t.git.commit_message = m,
                Ok(_) => t.toasts.info("AI returned an empty commit message"),
                Err(e) => t.toasts.error(format!("Commit message: {e}")),
            }
        }
    }
}
