//! Job background Integration Testing: run suite berurutan (dengan fase
//! cleanup), verifikasi sisa data di database, dan generate langkah dengan AI.
//!
//! Job dimiliki `Tabular`, bukan tab, sehingga tetap berjalan saat tab ditutup.
//! Keadaannya dicerminkan ke panel Background Tasks. Tab membaca hasil lewat
//! `suite_id`.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

use super::Tabular;
use super::background_tasks::{Snapshot, TaskId, TaskOwner};
use crate::agent::harness::{ProgressStatus, ProgressStep};
use crate::http_test_validate::{CodeSource, ValidateInput, ValidateOutcome};
use crate::http_tests::{
    self, DbCheckTarget, DbResidueCheck, HttpTestStep, HttpTestSuite, RunScope, StepGenHandle,
    StepGenMode, StepResult, TestEvent, TestGenHandle, TestRun, TestRunHandle,
};
use crate::repo_scan::RepoJobEvent;

/// Hasil generate langkah: langkah baru + variabel tambahan.
pub type GenResult = Result<(Vec<HttpTestStep>, Vec<(String, String)>), String>;

/// Run suite yang sedang berjalan.
pub struct RunJob {
    pub suite: HttpTestSuite,
    pub scope: RunScope,
    pub run_id: String,
    started_at: chrono::DateTime<chrono::Utc>,
    started: Instant,
    handle: TestRunHandle,
    /// Hasil per id langkah.
    pub results: HashMap<String, StepResult>,
    /// Indeks langkah (di `suite.steps`) yang sedang dikirim.
    pub current: Option<usize>,
    pub in_cleanup: bool,
    project_name: Option<String>,
    environment: Option<String>,
    db_targets: Vec<DbCheckTarget>,
    task: Option<TaskId>,
    steps: Vec<ProgressStep>,
    /// Selesai: ringkasan (Ok) atau pesan gagal (Err).
    finished: Option<Result<String, String>>,
}

impl RunJob {
    pub fn is_running(&self) -> bool {
        self.finished.is_none()
    }

    pub fn is_cancelling(&self) -> bool {
        self.handle.is_cancelling()
    }

    /// "Running 3/12: Create PO" atau "Cleaning up test data".
    pub fn progress_text(&self) -> String {
        let total = (0..self.suite.steps.len())
            .filter(|i| self.scope.includes(*i))
            .count();
        let done = self.results.len();
        let mut text = if self.in_cleanup {
            format!("Cleaning up test data ({done}/{total})")
        } else {
            format!("Running {done}/{total}")
        };
        if let Some(name) = self
            .current
            .and_then(|i| self.suite.steps.get(i))
            .map(|s| s.name.as_str())
        {
            text.push_str(&format!(": {name}"));
        }
        if self.is_cancelling() && !self.in_cleanup {
            text.push_str(" (stopping, cleanup will still run)");
        }
        text
    }
}

/// Verifikasi database setelah run.
struct DbCheckJob {
    suite_id: String,
    run: TestRun,
    rx: mpsc::Receiver<Vec<DbResidueCheck>>,
}

/// Generate langkah untuk suite yang ada.
pub struct GenJob {
    pub mode: StepGenMode,
    handle: StepGenHandle,
    pub started: Instant,
    pub status: String,
}

/// Generate suite baru untuk project.
pub struct SuiteGenJob {
    project_id: Option<String>,
    handle: TestGenHandle,
    pub started: Instant,
    pub status: String,
}

/// State Integration Testing milik `Tabular`.
#[derive(Default)]
pub struct IntegrationTestUi {
    pub runs: HashMap<String, RunJob>,
    db_checks: Vec<DbCheckJob>,
    pub gens: HashMap<String, GenJob>,
    pub suite_gen: Option<SuiteGenJob>,
    /// Hasil generate langkah yang belum diambil tab (per suite).
    pub gen_results: HashMap<String, GenResult>,
    /// Run terakhir yang selesai per suite.
    pub finished: HashMap<String, TestRun>,
    /// Naik setiap ada run tersimpan; tab memuat ulang riwayatnya.
    pub runs_rev: u64,
    /// Validasi (run nyata + perbaikan AI) yang sedang berjalan per suite.
    pub validations: HashMap<String, ValidateJob>,
    /// Hasil validasi yang belum diambil tab (per suite).
    pub validate_results: HashMap<String, Result<ValidateOutcome, String>>,
    /// `true` = jangan validasi otomatis setelah AI generate.
    pub skip_auto_validate: bool,
    /// Perbaikan kode API dengan AI yang sedang disiapkan (per suite).
    pub codefixes: HashMap<String, CodeFixJob>,
    /// Usulan perbaikan kode yang menunggu ditinjau (per suite).
    pub proposals: HashMap<String, crate::http_test_codefix::CodeFixProposal>,
    /// Cek build usulan (per suite).
    pub checks: HashMap<String, CheckJob>,
    /// Sidebar disembunyikan otomatis untuk tab ini (id tab, visibilitas
    /// sidebar sebelum disembunyikan).
    pub sidebar_auto: Option<(usize, bool)>,
}

/// Perbaikan kode yang sedang disiapkan AI.
pub struct CodeFixJob {
    pub suite_name: String,
    handle: crate::http_test_codefix::CodeFixHandle,
    pub started: Instant,
    pub status: String,
    task: Option<TaskId>,
    steps: Vec<ProgressStep>,
    finished: Option<Result<String, String>>,
}

impl CodeFixJob {
    pub fn is_running(&self) -> bool {
        self.finished.is_none()
    }
}

/// Cek build atas usulan perbaikan.
pub struct CheckJob {
    rx: mpsc::Receiver<Result<String, String>>,
    pub started: Instant,
    pub result: Option<Result<String, String>>,
}

/// Validasi suite yang sedang berjalan.
pub struct ValidateJob {
    pub suite_name: String,
    handle: crate::http_test_validate::ValidateHandle,
    pub started: Instant,
    pub status: String,
    task: Option<TaskId>,
    steps: Vec<ProgressStep>,
    finished: Option<Result<String, String>>,
}

impl ValidateJob {
    pub fn is_running(&self) -> bool {
        self.finished.is_none()
    }
}

// ─── Konteks project ────────────────────────────────────────────────────────

/// Project pemilik suite, atau project aktif bila suite belum terikat.
pub fn suite_project<'a>(
    t: &'a Tabular,
    suite: &HttpTestSuite,
) -> Option<&'a crate::project::Project> {
    match &suite.project_id {
        Some(id) => t.projects.list.iter().find(|p| &p.id == id),
        None => super::project_ui::active(t),
    }
}

/// Variabel environment project untuk suite (dengan secret dari keychain).
pub fn project_vars(t: &mut Tabular, suite: &HttpTestSuite) -> HashMap<String, String> {
    let ws = suite_project(t, suite).and_then(|p| p.http_workspace_id.clone());
    super::project_ui::http_vars_for_workspace(t, ws.as_deref())
}

/// Nama variabel environment project (tanpa nilai) untuk lint.
pub fn project_var_names(t: &Tabular, suite: &HttpTestSuite) -> std::collections::HashSet<String> {
    suite_project(t, suite)
        .and_then(|p| p.active_environment())
        .map(|e| {
            e.variables
                .iter()
                .map(|v| v.key.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Folder request workspace HTTP project (untuk AI dan cakupan). Tanpa
/// workspace project: semua workspace.
pub fn project_catalogs(t: &Tabular, suite: &HttpTestSuite) -> Vec<http_tests::FolderCatalog> {
    let ws_id = suite_project(t, suite).and_then(|p| p.http_workspace_id.clone());
    let mut out = Vec::new();
    for ws in &t.yaak_workspaces {
        if ws_id.as_deref().is_some_and(|id| id != ws.id) {
            continue;
        }
        for f in &ws.folders {
            let c = http_tests::FolderCatalog::from_folder(f);
            if !c.requests.is_empty() {
                out.push(c);
            }
        }
    }
    out
}

/// Semua request katalog (datar).
pub fn project_requests(
    t: &Tabular,
    suite: &HttpTestSuite,
) -> Vec<crate::http_collection::SavedRequest> {
    project_catalogs(t, suite)
        .into_iter()
        .flat_map(|c| c.requests)
        .collect()
}

fn db_targets(t: &Tabular, suite: &HttpTestSuite) -> Vec<DbCheckTarget> {
    let Some(p) = suite_project(t, suite) else {
        return Vec::new();
    };
    p.member_connections(&t.connections)
        .into_iter()
        .filter_map(|c| {
            Some(DbCheckTarget {
                connection_id: c.id?,
                connection_name: c.name.clone(),
            })
        })
        .collect()
}

// ─── Run ────────────────────────────────────────────────────────────────────

/// Mulai run suite (atau sebagian group-nya). Gagal bila suite ini sedang berjalan.
pub fn start_run(t: &mut Tabular, mut suite: HttpTestSuite, scope: RunScope) -> Result<(), String> {
    if is_validating(t, &suite.id) {
        return Err("The suite is being validated; wait for it to finish".into());
    }
    // Kredensial akun uji yang kosong (mis. test_password) diisi dan disimpan.
    let filled = http_tests::fill_test_credentials(&mut suite.variables);
    if !filled.is_empty() {
        if let Err(e) = http_tests::save_suite(&suite) {
            t.toasts.error(e);
        }
        t.toasts.info(format!(
            "Generated a value for {} (account created by the test)",
            filled.join(", ")
        ));
    }
    if t.integration_tests
        .runs
        .get(&suite.id)
        .is_some_and(RunJob::is_running)
    {
        return Err("This suite is already running".into());
    }
    let env_vars = project_vars(t, &suite);
    let vars = http_tests::initial_vars_with(&suite, &env_vars);
    let run_id = vars.get("run_id").cloned().unwrap_or_default();
    let project = suite_project(t, &suite);
    let project_name = project.map(|p| p.name.clone());
    let environment = project
        .and_then(|p| p.active_environment())
        .map(|e| e.name.clone());
    let targets = db_targets(t, &suite);
    log::info!(
        "[INTEGRATION_TEST] start '{}' run {run_id} groups={:?} env={:?}",
        suite.name,
        scope.groups,
        environment
    );
    let handle = http_tests::spawn_run(suite.clone(), scope.clone(), vars);
    t.integration_tests.runs.insert(
        suite.id.clone(),
        RunJob {
            suite,
            scope,
            run_id,
            started_at: chrono::Utc::now(),
            started: Instant::now(),
            handle,
            results: HashMap::new(),
            current: None,
            in_cleanup: false,
            project_name,
            environment,
            db_targets: targets,
            task: None,
            steps: Vec::new(),
            finished: None,
        },
    );
    Ok(())
}

/// Hentikan langkah Test; cleanup tetap dijalankan.
pub fn stop_run(t: &mut Tabular, suite_id: &str) {
    if let Some(job) = t.integration_tests.runs.get(suite_id) {
        job.handle.cancel();
    }
}

/// Hentikan seketika termasuk cleanup.
pub fn abort_run(t: &mut Tabular, suite_id: &str) {
    if let Some(job) = t.integration_tests.runs.get(suite_id) {
        job.handle.abort();
    }
}

// ─── Generate ───────────────────────────────────────────────────────────────

/// Backend AI aktif, atau pesan kenapa belum siap.
fn backend(t: &Tabular) -> Result<(crate::ai_assistant::ChatBackend, String), String> {
    let target = t.effective_chat_target();
    crate::ai_assistant::backend_ready_for(t, target)?;
    Ok((
        crate::ai_assistant::chat_backend_for(t, target),
        crate::ai_assistant::backend_label_for(t, target),
    ))
}

const NO_API_MESSAGE: &str = "The project has no HTTP API requests yet. Generate endpoints from the repository or add requests to the project's API workspace first.";

/// Generate langkah tambahan (atau cleanup) untuk suite.
pub fn start_step_gen(
    t: &mut Tabular,
    suite: &HttpTestSuite,
    instructions: String,
    mode: StepGenMode,
) -> Result<(), String> {
    if t.integration_tests.gens.contains_key(&suite.id) {
        return Err("The AI is already working on this suite".into());
    }
    let (backend, label) = backend(t)?;
    let catalogs = project_catalogs(t, suite);
    if catalogs.is_empty() && mode == StepGenMode::Append {
        return Err(NO_API_MESSAGE.into());
    }
    let handle = http_tests::spawn_step_generate(
        backend,
        label.clone(),
        catalogs,
        suite.clone(),
        instructions,
        mode,
    );
    t.integration_tests.gens.insert(
        suite.id.clone(),
        GenJob {
            mode,
            handle,
            started: Instant::now(),
            status: format!("Asking {label}"),
        },
    );
    Ok(())
}

/// Generate suite baru dari seluruh API project aktif.
pub fn start_suite_gen(t: &mut Tabular, instructions: String) -> Result<(), String> {
    if t.integration_tests.suite_gen.is_some() {
        return Err("The AI is already designing a suite".into());
    }
    let (backend, label) = backend(t)?;
    let project_id = super::project_ui::active(t).map(|p| p.id.clone());
    let probe = HttpTestSuite {
        project_id: project_id.clone(),
        ..Default::default()
    };
    let catalogs = project_catalogs(t, &probe);
    if catalogs.is_empty() {
        return Err(NO_API_MESSAGE.into());
    }
    let handle = http_tests::spawn_generate(backend, label.clone(), catalogs, instructions);
    t.integration_tests.suite_gen = Some(SuiteGenJob {
        project_id,
        handle,
        started: Instant::now(),
        status: format!("Asking {label}"),
    });
    Ok(())
}

// ─── Validasi ───────────────────────────────────────────────────────────────

pub fn is_validating(t: &Tabular, suite_id: &str) -> bool {
    t.integration_tests
        .validations
        .get(suite_id)
        .is_some_and(ValidateJob::is_running)
}

/// Lokasi kode route untuk request katalog yang dipakai langkah suite.
fn code_sources(t: &Tabular, suite: &HttpTestSuite) -> HashMap<String, CodeSource> {
    fn walk(
        folder: &crate::http_collection::HttpFolder,
        root: Option<std::path::PathBuf>,
        wanted: &std::collections::HashSet<&str>,
        out: &mut HashMap<String, CodeSource>,
    ) {
        let root = folder
            .local_repo_path()
            .map(std::path::PathBuf::from)
            .filter(|p| p.is_dir())
            .or(root);
        if let Some(root) = &root {
            for r in &folder.requests {
                if let Some(src) = r.source.as_ref().filter(|_| wanted.contains(r.id.as_str())) {
                    out.insert(
                        r.id.clone(),
                        CodeSource {
                            root: root.clone(),
                            source: src.clone(),
                        },
                    );
                }
            }
        }
        for c in &folder.children {
            walk(c, root.clone(), wanted, out);
        }
    }
    let wanted: std::collections::HashSet<&str> = suite
        .steps
        .iter()
        .filter_map(|s| s.request_id.as_deref())
        .collect();
    let mut out = HashMap::new();
    if wanted.is_empty() {
        return out;
    }
    for ws in &t.yaak_workspaces {
        for f in &ws.folders {
            walk(f, None, &wanted, &mut out);
        }
    }
    out
}

/// Jalankan suite terhadap server dan biarkan AI memperbaiki langkah yang
/// gagal (beberapa putaran). Ditolak untuk environment Production.
pub fn start_validation(t: &mut Tabular, suite: &HttpTestSuite) -> Result<(), String> {
    if is_validating(t, &suite.id) {
        return Err("This suite is already being validated".into());
    }
    if t.integration_tests
        .runs
        .get(&suite.id)
        .is_some_and(RunJob::is_running)
    {
        return Err("The suite is running; wait for it to finish".into());
    }
    if suite.steps.iter().all(|s| !s.enabled) {
        return Err("The suite has no enabled steps to validate".into());
    }
    let project = suite_project(t, suite);
    let env = project.and_then(|p| p.active_environment());
    if env.and_then(|e| e.environment()) == Some(crate::connection_env::Environment::Production) {
        return Err(
            "The active environment is Production. Validation sends real requests that create and delete data; switch the project to Development or Staging first."
                .into(),
        );
    }
    let environment = env.map(|e| e.name.clone());
    let (backend, label) = backend(t)?;
    let input = ValidateInput {
        backend,
        backend_label: label.clone(),
        suite: suite.clone(),
        env_vars: project_vars(t, suite),
        environment,
        catalogs: project_catalogs(t, suite),
        code: code_sources(t, suite),
        max_rounds: crate::http_test_validate::DEFAULT_ROUNDS,
    };
    log::info!(
        "[INTEGRATION_TEST] validating '{}' with {label}",
        suite.name
    );
    let handle = crate::http_test_validate::spawn_validate(input);
    t.integration_tests.validations.insert(
        suite.id.clone(),
        ValidateJob {
            suite_name: suite.name.clone(),
            handle,
            started: Instant::now(),
            status: "Starting".into(),
            task: None,
            steps: Vec::new(),
            finished: None,
        },
    );
    Ok(())
}

/// Validasi otomatis setelah AI generate (bila tidak dimatikan user).
pub fn auto_validate(t: &mut Tabular, suite: &HttpTestSuite) {
    if t.integration_tests.skip_auto_validate {
        return;
    }
    if let Err(e) = start_validation(t, suite) {
        t.toasts
            .warning(format!("'{}' was not validated: {e}", suite.name));
    }
}

pub fn cancel_validation(t: &mut Tabular, suite_id: &str) {
    if let Some(job) = t.integration_tests.validations.get(suite_id) {
        job.handle.cancel();
    }
}

fn poll_validations(t: &mut Tabular) {
    let mut done: Vec<(String, Result<ValidateOutcome, String>)> = Vec::new();
    for (suite_id, job) in t.integration_tests.validations.iter_mut() {
        if !job.is_running() {
            continue;
        }
        loop {
            match job.handle.rx.try_recv() {
                Ok(RepoJobEvent::Progress(step)) => {
                    job.status = match &step.detail {
                        Some(d) => format!("{} ({d})", step.description),
                        None => step.description.clone(),
                    };
                    job.steps = vec![step];
                }
                Ok(RepoJobEvent::Activity) => {}
                Ok(RepoJobEvent::Finished(result)) => {
                    done.push((suite_id.clone(), result));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    done.push((
                        suite_id.clone(),
                        Err("Validation stopped unexpectedly".into()),
                    ));
                    break;
                }
            }
        }
    }
    for (suite_id, result) in done {
        let msg = match &result {
            Ok(o) => {
                let (passed, total) = crate::http_test_validate::counts(&o.run);
                let mut m = format!(
                    "Validated '{}': {passed}/{total} step(s) pass{}",
                    o.suite.name,
                    o.run
                        .environment
                        .as_deref()
                        .map(|e| format!(" on {e}"))
                        .unwrap_or_default()
                );
                if o.fixed > 0 {
                    m.push_str(&format!(
                        ". AI fixed {} step(s) in {} round(s)",
                        o.fixed,
                        o.rounds.len()
                    ));
                }
                if !o.remaining.is_empty() {
                    m.push_str(&format!(". {} still need attention", o.remaining.len()));
                }
                Ok(m)
            }
            Err(e) => Err(e.clone()),
        };
        match &msg {
            Ok(m) if result.as_ref().is_ok_and(|o| o.remaining.is_empty()) => {
                t.toasts.success(m.clone())
            }
            Ok(m) => t.toasts.warning(m.clone()),
            Err(e) => t.toasts.error(format!("Validation failed: {e}")),
        }
        if let Some(job) = t.integration_tests.validations.get_mut(&suite_id) {
            job.finished = Some(msg);
        }
        if let Ok(o) = &result {
            let mut run = o.run.clone();
            run.project_name = suite_project(t, &o.suite).map(|p| p.name.clone());
            if let Err(e) = http_tests::save_run(&run) {
                log::warn!("[INTEGRATION_TEST] cannot save validation run: {e}");
            }
            t.integration_tests.finished.insert(suite_id.clone(), run);
            t.integration_tests.runs_rev += 1;
        }
        let tab_open = t.query_tabs.iter().any(|tab| {
            tab.integration_test_state
                .as_ref()
                .is_some_and(|s| s.suite_id() == suite_id)
        });
        if tab_open {
            t.integration_tests
                .validate_results
                .insert(suite_id, result);
        } else if let Ok(o) = result
            && let Err(e) = http_tests::save_suite(&o.suite)
        {
            t.toasts.error(e);
        }
    }
}

// ─── Perbaikan kode API ─────────────────────────────────────────────────────

pub fn is_fixing_code(t: &Tabular, suite_id: &str) -> bool {
    t.integration_tests
        .codefixes
        .get(suite_id)
        .is_some_and(CodeFixJob::is_running)
}

/// Folder repo lokal pertama dari folder API project (cadangan bila langkah
/// tidak punya lokasi kode).
fn project_repo_fallback(t: &Tabular, suite: &HttpTestSuite) -> Option<std::path::PathBuf> {
    let ws_id = suite_project(t, suite).and_then(|p| p.http_workspace_id.clone());
    t.yaak_workspaces
        .iter()
        .filter(|ws| ws_id.as_deref().is_none_or(|id| id == ws.id))
        .flat_map(|ws| ws.folders.iter())
        .filter_map(|f| f.local_repo_path())
        .map(std::path::PathBuf::from)
        .find(|p| p.is_dir())
}

/// Minta AI memperbaiki kode API agar langkah `step_ids` (yang gagal di `run`)
/// bekerja. Perubahan disiapkan di branch terpisah untuk ditinjau.
pub fn start_code_fix(
    t: &mut Tabular,
    suite: &HttpTestSuite,
    run: &TestRun,
    step_ids: &[String],
    note: String,
) -> Result<(), String> {
    use crate::http_test_codefix::{CodeFixInput, CodeFixTarget};
    if is_fixing_code(t, &suite.id) {
        return Err("The AI is already preparing a code fix for this suite".into());
    }
    if t.integration_tests.proposals.contains_key(&suite.id) {
        return Err("Review, apply or discard the pending code fix first".into());
    }
    let code = code_sources(t, suite);
    let targets: Vec<CodeFixTarget> = step_ids
        .iter()
        .filter_map(|id| {
            let step = suite.steps.iter().find(|s| &s.id == id)?;
            let result = run.results.iter().find(|r| &r.step_id == id)?;
            matches!(
                result.outcome(),
                http_tests::StepOutcome::Failed | http_tests::StepOutcome::Error
            )
            .then(|| CodeFixTarget {
                step_id: id.clone(),
                step_name: step.name.clone(),
                request: step.request.clone(),
                assertions: step.assertions.iter().map(|a| a.describe()).collect(),
                result: result.clone(),
                code: step.request_id.as_ref().and_then(|r| code.get(r)).cloned(),
            })
        })
        .collect();
    if targets.is_empty() {
        return Err("Select a step that failed in the last run".into());
    }
    let repo = crate::http_test_codefix::pick_repo(&targets, project_repo_fallback(t, suite))
        .ok_or_else(|| {
            "No local repository is linked to the project's API folder. Right-click the folder and choose Folder Repository… to set it."
                .to_string()
        })?;
    let (backend, label) = backend(t)?;
    log::info!(
        "[INTEGRATION_TEST] code fix for '{}' ({} step(s)) in {}",
        suite.name,
        targets.len(),
        repo.display()
    );
    let handle = crate::http_test_codefix::spawn_code_fix(CodeFixInput {
        backend,
        backend_label: label,
        repo_root: repo,
        suite_name: suite.name.clone(),
        targets,
        note,
    });
    t.integration_tests.codefixes.insert(
        suite.id.clone(),
        CodeFixJob {
            suite_name: suite.name.clone(),
            handle,
            started: Instant::now(),
            status: "Starting".into(),
            task: None,
            steps: Vec::new(),
            finished: None,
        },
    );
    Ok(())
}

pub fn cancel_code_fix(t: &mut Tabular, suite_id: &str) {
    if let Some(job) = t.integration_tests.codefixes.get(suite_id) {
        job.handle.cancel();
    }
}

/// Keputusan user atas usulan perbaikan kode.
pub enum ProposalAction {
    Apply,
    KeepBranch,
    Discard,
    Check,
}

pub fn resolve_proposal(t: &mut Tabular, suite_id: &str, action: ProposalAction) {
    use crate::http_test_codefix as cf;
    if let ProposalAction::Check = action {
        if let Some(p) = t.integration_tests.proposals.get(suite_id) {
            let rx = cf::spawn_check(p);
            t.integration_tests.checks.insert(
                suite_id.to_string(),
                CheckJob {
                    rx,
                    started: Instant::now(),
                    result: None,
                },
            );
        }
        return;
    }
    let Some(p) = t.integration_tests.proposals.remove(suite_id) else {
        return;
    };
    t.integration_tests.checks.remove(suite_id);
    match action {
        ProposalAction::Apply => match cf::apply_to_working_copy(&p) {
            Ok(()) => t.toasts.success(format!(
                "Applied the fix to {} ({} file(s), not committed). Restart the API server, then run the suite again.",
                p.repo.display(),
                p.files.len()
            )),
            Err(e) => {
                t.toasts.error(e);
                // Usulan tetap bisa ditinjau atau dibuang.
                t.integration_tests
                    .proposals
                    .insert(suite_id.to_string(), p);
            }
        },
        ProposalAction::KeepBranch => match cf::keep_branch(&p) {
            Ok(branch) => t.toasts.success(format!(
                "Saved the fix as branch {branch} in {}. Merge it from the Git client when ready.",
                p.repo.display()
            )),
            Err(e) => {
                t.toasts.error(e);
                t.integration_tests
                    .proposals
                    .insert(suite_id.to_string(), p);
            }
        },
        ProposalAction::Discard => {
            if let Err(e) = cf::discard(&p) {
                t.toasts.warning(format!("Discarded, but cleanup reported: {e}"));
            } else {
                t.toasts.info("Code fix discarded");
            }
        }
        ProposalAction::Check => {}
    }
}

fn poll_code_fixes(t: &mut Tabular) {
    let mut done: Vec<(
        String,
        Result<crate::http_test_codefix::CodeFixProposal, String>,
    )> = Vec::new();
    for (suite_id, job) in t.integration_tests.codefixes.iter_mut() {
        if !job.is_running() {
            continue;
        }
        loop {
            match job.handle.rx.try_recv() {
                Ok(RepoJobEvent::Progress(step)) => {
                    job.status = match &step.detail {
                        Some(d) => format!("{} ({d})", step.description),
                        None => step.description.clone(),
                    };
                    job.steps = vec![step];
                }
                Ok(RepoJobEvent::Activity) => {}
                Ok(RepoJobEvent::Finished(result)) => {
                    done.push((suite_id.clone(), result));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    done.push((
                        suite_id.clone(),
                        Err("Code fix stopped unexpectedly".into()),
                    ));
                    break;
                }
            }
        }
    }
    for (suite_id, result) in done {
        let msg = match &result {
            Ok(p) => Ok(format!(
                "Code fix ready for review: {} file(s) in {}",
                p.files.len(),
                p.repo.display()
            )),
            Err(e) => Err(e.clone()),
        };
        match &msg {
            Ok(m) => t.toasts.success(m.clone()),
            Err(e) => t.toasts.error(format!("Code fix: {e}")),
        }
        if let Some(job) = t.integration_tests.codefixes.get_mut(&suite_id) {
            job.finished = Some(msg);
        }
        if let Ok(p) = result {
            t.integration_tests.proposals.insert(suite_id.clone(), p);
            super::integration_test_ui::open_suite(t, &suite_id);
        }
    }
    for job in t.integration_tests.checks.values_mut() {
        if job.result.is_none() {
            match job.rx.try_recv() {
                Ok(r) => job.result = Some(r),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    job.result = Some(Err("The check stopped unexpectedly".into()))
                }
            }
        }
    }
}

pub fn cancel_gen(t: &mut Tabular, suite_id: &str) {
    if let Some(job) = t.integration_tests.gens.remove(suite_id) {
        job.handle.cancel();
    }
}

pub fn cancel_suite_gen(t: &mut Tabular) {
    if let Some(job) = t.integration_tests.suite_gen.take() {
        job.handle.cancel();
    }
}

// ─── Tick ───────────────────────────────────────────────────────────────────

/// Dipanggil setiap frame dari `app_impl`.
pub fn tick(t: &mut Tabular, ctx: &egui::Context) {
    let ui = &t.integration_tests;
    if ui.runs.is_empty()
        && ui.db_checks.is_empty()
        && ui.gens.is_empty()
        && ui.suite_gen.is_none()
        && ui.validations.is_empty()
        && ui.codefixes.is_empty()
        && ui.checks.is_empty()
    {
        return;
    }
    poll_runs(t);
    poll_db_checks(t);
    poll_gens(t);
    poll_validations(t);
    poll_code_fixes(t);
    mirror_tasks(t);
    let ui = &t.integration_tests;
    if ui.runs.values().any(RunJob::is_running)
        || !ui.db_checks.is_empty()
        || !ui.gens.is_empty()
        || ui.suite_gen.is_some()
        || ui.validations.values().any(ValidateJob::is_running)
        || ui.codefixes.values().any(CodeFixJob::is_running)
        || ui.checks.values().any(|c| c.result.is_none())
    {
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

fn active_step(text: String) -> Vec<ProgressStep> {
    vec![ProgressStep {
        step_index: None,
        description: text,
        detail: None,
        status: ProgressStatus::Active,
        tool_name: None,
    }]
}

fn poll_runs(t: &mut Tabular) {
    let mut done: Vec<(String, TestRun, Vec<DbCheckTarget>, Vec<String>)> = Vec::new();
    for (suite_id, job) in t.integration_tests.runs.iter_mut() {
        if !job.is_running() {
            continue;
        }
        let mut finished_run = None;
        loop {
            match job.handle.rx.try_recv() {
                Ok(TestEvent::StepStarted(i)) => job.current = Some(i),
                Ok(TestEvent::StepFinished(r)) => {
                    job.current = None;
                    job.results.insert(r.step_id.clone(), *r);
                }
                Ok(TestEvent::CleanupStarted) => job.in_cleanup = true,
                Ok(TestEvent::Finished(summary)) => {
                    let mut run = TestRun::from_summary(
                        &job.suite,
                        &job.scope,
                        job.run_id.clone(),
                        job.started_at,
                        *summary,
                    );
                    run.project_name = job.project_name.clone();
                    run.environment = job.environment.clone();
                    finished_run = Some(run);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    job.finished = Some(Err("The test run stopped unexpectedly".into()));
                    break;
                }
            }
        }
        if let Some(run) = finished_run {
            job.in_cleanup = false;
            job.current = None;
            let tables = http_tests::touched_tables(&job.suite, &run.results);
            job.steps = active_step("Verifying the database is clean".into());
            done.push((suite_id.clone(), run, job.db_targets.clone(), tables));
        } else if job.is_running() {
            job.steps = active_step(job.progress_text());
        }
    }
    for (suite_id, run, targets, tables) in done {
        if !tables.is_empty() && !targets.is_empty() && !run.aborted {
            let rx = spawn_db_check(targets, tables, run.run_id.clone());
            t.integration_tests
                .db_checks
                .push(DbCheckJob { suite_id, run, rx });
        } else {
            finalize(t, &suite_id, run);
        }
    }
}

/// Jalankan pemeriksaan sisa data di thread terpisah dengan runtime sendiri.
fn spawn_db_check(
    targets: Vec<DbCheckTarget>,
    tables: Vec<String>,
    run_id: String,
) -> mpsc::Receiver<Vec<DbResidueCheck>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let checks = match crate::runtime::shared() {
            Some(rt) => rt.block_on(async {
                match crate::agent::core::open_cache_pool().await {
                    Ok(pool) => {
                        let session = crate::agent::core::HeadlessSession::new(pool);
                        http_tests::check_db_residue(&session, &targets, &tables, &run_id).await
                    }
                    Err(e) => {
                        log::warn!("[INTEGRATION_TEST] db check unavailable: {e}");
                        Vec::new()
                    }
                }
            }),
            None => {
                log::warn!("[INTEGRATION_TEST] db check runtime unavailable");
                Vec::new()
            }
        };
        let _ = tx.send(checks);
    });
    rx
}

fn poll_db_checks(t: &mut Tabular) {
    let mut ready = Vec::new();
    t.integration_tests
        .db_checks
        .retain_mut(|job| match job.rx.try_recv() {
            Ok(checks) => {
                job.run.db_checks = checks;
                ready.push((job.suite_id.clone(), std::mem::take(&mut job.run)));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                ready.push((job.suite_id.clone(), std::mem::take(&mut job.run)));
                false
            }
        });
    for (suite_id, run) in ready {
        finalize(t, &suite_id, run);
    }
}

/// Simpan run, kabarkan hasilnya, dan tandai job selesai.
fn finalize(t: &mut Tabular, suite_id: &str, run: TestRun) {
    use crate::http_test_report::Verdict;
    if let Err(e) = http_tests::save_run(&run) {
        t.toasts.error(e);
    }
    if let Some(e) = &run.preflight_error {
        t.toasts.error(format!("{}: not run. {e}", run.suite_name));
        if let Some(job) = t.integration_tests.runs.get_mut(suite_id) {
            job.finished = Some(Err(e.clone()));
        }
        t.integration_tests
            .finished
            .insert(suite_id.to_string(), run);
        t.integration_tests.runs_rev += 1;
        return;
    }
    let stats = crate::http_test_report::compute(&run);
    let mut msg = format!(
        "{}: {}. {} passed, {} failed, {} blocked.",
        run.suite_name,
        stats.verdict().label(),
        stats.tests.passed,
        stats.tests.failed + stats.tests.error,
        stats.tests.blocked
    );
    if stats.data_clean() {
        msg.push_str(" Test data cleaned up.");
    } else {
        msg.push_str(&format!(
            " {} item(s) of test data may be left behind.",
            stats.residue_items + stats.db_residue_rows as usize
        ));
    }
    log::info!("[INTEGRATION_TEST] {msg}");
    match stats.verdict() {
        Verdict::Pass => t.toasts.success(msg.clone()),
        Verdict::PassWithDeviations => t.toasts.warning(msg.clone()),
        _ => t.toasts.error(msg.clone()),
    }
    if let Some(job) = t.integration_tests.runs.get_mut(suite_id) {
        job.finished = Some(match stats.verdict() {
            Verdict::Pass | Verdict::PassWithDeviations => Ok(msg),
            _ => Err(msg),
        });
    }
    t.integration_tests
        .finished
        .insert(suite_id.to_string(), run);
    t.integration_tests.runs_rev += 1;
}

fn poll_gens(t: &mut Tabular) {
    let mut done: Vec<(String, GenResult)> = Vec::new();
    for (suite_id, job) in t.integration_tests.gens.iter_mut() {
        loop {
            match job.handle.rx.try_recv() {
                Ok(RepoJobEvent::Progress(step)) => {
                    job.status = step.detail.unwrap_or(step.description);
                }
                Ok(RepoJobEvent::Activity) => {}
                Ok(RepoJobEvent::Finished(result)) => {
                    done.push((suite_id.clone(), result));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    done.push((
                        suite_id.clone(),
                        Err("The AI stopped without a reply".into()),
                    ));
                    break;
                }
            }
        }
    }
    for (suite_id, result) in done {
        t.integration_tests.gens.remove(&suite_id);
        let tab_open = t.query_tabs.iter().any(|tab| {
            tab.integration_test_state
                .as_ref()
                .is_some_and(|s| s.suite_id() == suite_id)
        });
        if tab_open {
            t.integration_tests.gen_results.insert(suite_id, result);
            continue;
        }
        // Tidak ada tab terbuka: gabungkan langsung ke suite di disk.
        match result {
            Ok((steps, vars)) => match http_tests::load_suite(&suite_id) {
                Some(mut suite) => {
                    let n = http_tests::merge_generated_steps(&mut suite, steps, vars);
                    match http_tests::save_suite(&suite) {
                        Ok(()) => {
                            t.toasts
                                .success(format!("Added {n} AI step(s) to '{}'", suite.name));
                            auto_validate(t, &suite);
                        }
                        Err(e) => t.toasts.error(e),
                    }
                }
                None => t.toasts.warning("The suite no longer exists"),
            },
            Err(e) => t.toasts.error(format!("AI step generation failed: {e}")),
        }
    }

    let mut suites_done = None;
    if let Some(job) = t.integration_tests.suite_gen.as_mut() {
        loop {
            match job.handle.rx.try_recv() {
                Ok(RepoJobEvent::Progress(step)) => {
                    job.status = step.detail.unwrap_or(step.description);
                }
                Ok(RepoJobEvent::Activity) => {}
                Ok(RepoJobEvent::Finished(result)) => {
                    suites_done = Some((job.project_id.clone(), result));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    suites_done = Some((
                        job.project_id.clone(),
                        Err("The AI stopped without a reply".into()),
                    ));
                    break;
                }
            }
        }
    }
    if let Some((project_id, result)) = suites_done {
        t.integration_tests.suite_gen = None;
        match result {
            Ok(mut suites) => {
                let mut first = None;
                let mut saved = Vec::new();
                for s in &mut suites {
                    s.project_id = project_id.clone();
                    match http_tests::save_suite(s) {
                        Ok(()) => {
                            first.get_or_insert_with(|| s.id.clone());
                            saved.push(s.clone());
                        }
                        Err(e) => t.toasts.error(e),
                    }
                }
                for s in &saved {
                    auto_validate(t, s);
                }
                let steps: usize = suites.iter().map(|s| s.steps.len()).sum();
                t.toasts.success(format!(
                    "AI designed {} suite(s) with {steps} step(s)",
                    suites.len()
                ));
                if let Some(id) = first {
                    super::integration_test_ui::open_suite(t, &id);
                }
            }
            Err(e) => t.toasts.error(format!("AI suite generation failed: {e}")),
        }
    }
}

fn mirror_tasks(t: &mut Tabular) {
    let bg = &mut t.background_tasks;
    let mut show: Option<String> = None;
    for (suite_id, job) in t.integration_tests.runs.iter_mut() {
        let out = bg.mirror(
            &mut job.task,
            Snapshot {
                owner: TaskOwner::IntegrationTest,
                title: &job.suite.name,
                subtitle: "Integration test run",
                steps: &job.steps,
                started_at: Some(job.started),
                last_activity_at: None,
                hidden: true,
                result: job.finished.clone(),
            },
        );
        if out.show {
            show = Some(suite_id.clone());
        }
        if out.cancel {
            job.handle.cancel();
        }
    }
    for (suite_id, job) in t.integration_tests.validations.iter_mut() {
        let title = format!("Validate {}", job.suite_name);
        let out = bg.mirror(
            &mut job.task,
            Snapshot {
                owner: TaskOwner::IntegrationTest,
                title: &title,
                subtitle: "Run against the server and fix failing steps with AI",
                steps: &job.steps,
                started_at: Some(job.started),
                last_activity_at: None,
                hidden: true,
                result: job.finished.clone(),
            },
        );
        if out.show {
            show = Some(suite_id.clone());
        }
        if out.cancel {
            job.handle.cancel();
        }
    }
    t.integration_tests
        .validations
        .retain(|_, j| j.is_running() || j.task.is_some_and(|id| bg.contains(id)));
    for (suite_id, job) in t.integration_tests.codefixes.iter_mut() {
        let title = format!("Fix API code: {}", job.suite_name);
        let out = bg.mirror(
            &mut job.task,
            Snapshot {
                owner: TaskOwner::IntegrationTest,
                title: &title,
                subtitle: "AI code fix in a separate git branch",
                steps: &job.steps,
                started_at: Some(job.started),
                last_activity_at: None,
                hidden: true,
                result: job.finished.clone(),
            },
        );
        if out.show {
            show = Some(suite_id.clone());
        }
        if out.cancel {
            job.handle.cancel();
        }
    }
    t.integration_tests
        .codefixes
        .retain(|_, j| j.is_running() || j.task.is_some_and(|id| bg.contains(id)));
    // Run selesai dibuang setelah entrinya hilang dari panel.
    t.integration_tests
        .runs
        .retain(|_, j| j.is_running() || j.task.is_some_and(|id| bg.contains(id)));
    if let Some(id) = show {
        super::integration_test_ui::open_suite(t, &id);
    }
}
