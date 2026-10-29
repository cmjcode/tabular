//! Logika tool MCP untuk integration test dan data check (F4):
//! `list_test_suites`, `run_test_suite`, `get_test_run`, `list_data_checks`,
//! `run_data_checks`, `save_data_check`.
//!
//! Headless; `mcp.rs` hanya membungkus fungsi di sini menjadi tool, termasuk
//! persetujuan user untuk `run_test_suite` (membuat data di server) dan
//! `save_data_check`. Run dikerjakan di thread biasa karena runner suite
//! sinkron dan memakai [`crate::runtime::block_on`].

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use super::core::{AgentError, HeadlessSession};
use crate::connection_env::Environment;
use crate::data_checks::{self, DataCheck, Severity};
use crate::http_test_report::{self as report, ReportFormat, ReportInput};
use crate::http_tests::{self, HttpTestSuite, RunScope, TestRun};
use crate::project::Project;

// ─── Tipe hasil ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SuiteValidationInfo {
    pub at: String,
    pub environment: Option<String>,
    pub passed: usize,
    pub total: usize,
    /// `false` bila suite diubah setelah validasi.
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SuiteSummary {
    pub id: String,
    pub name: String,
    pub project: Option<String>,
    pub steps: usize,
    pub db_steps: usize,
    pub concurrent_steps: usize,
    pub groups: Vec<String>,
    pub last_validation: Option<SuiteValidationInfo>,
    /// Verdict run terakhir, bila ada.
    pub last_run: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SuiteList {
    pub suites: Vec<SuiteSummary>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct StepSummary {
    pub id: String,
    pub name: String,
    pub phase: String,
    pub kind: String,
    pub outcome: String,
    /// HTTP status (0 untuk langkah DB atau tanpa respons).
    pub status: u16,
    pub time_ms: u128,
    /// Assertion gagal, error, atau catatan.
    pub details: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SuiteRunReport {
    pub run_id: String,
    pub suite_id: String,
    pub suite: String,
    pub environment: Option<String>,
    /// PASS, PASS WITH DEVIATIONS, FAIL, ...
    pub verdict: String,
    pub passed: usize,
    pub failed: usize,
    pub error: usize,
    pub blocked: usize,
    pub skipped: usize,
    /// `true` bila semua data uji terhapus.
    pub data_clean: bool,
    pub steps: Vec<StepSummary>,
    /// Laporan HTML lengkap di disk.
    pub report_path: Option<String>,
    pub preflight_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct RequestInfo {
    pub method: String,
    pub url: String,
    /// Header dengan nilai rahasia disamarkan.
    pub headers: Vec<(String, String)>,
    /// Body dengan nilai rahasia disamarkan (masih berisi `{{variabel}}`).
    pub body: String,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct StepDetail {
    pub id: String,
    pub name: String,
    pub phase: String,
    pub outcome: String,
    /// Definisi request dari suite (langkah HTTP).
    pub request: Option<RequestInfo>,
    /// SQL dengan nilai parameter (langkah DB).
    pub sql: Option<String>,
    /// URL yang benar-benar dikirim (rahasia disamarkan) atau nama koneksi.
    pub sent_to: String,
    pub status: u16,
    pub time_ms: u128,
    pub error: Option<String>,
    pub failed_assertions: Vec<String>,
    pub response_headers: Vec<(String, String)>,
    pub response_body: String,
    /// Langkah DB: kolom dan baris contoh.
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Langkah concurrent: `putaran#nomor=status`.
    pub burst: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct TestRunDetail {
    pub run_id: String,
    pub suite_id: String,
    pub suite: String,
    pub environment: Option<String>,
    pub started_at: String,
    pub verdict: String,
    pub steps: Vec<StepDetail>,
    pub residue: Vec<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DataCheckInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub severity: String,
    pub enabled: bool,
    pub connection_var: String,
    pub sql: String,
    pub last_status: Option<String>,
    pub last_rows: Option<usize>,
    pub last_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DataCheckList {
    pub project: String,
    pub checks: Vec<DataCheckInfo>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DataCheckOutcome {
    pub id: String,
    pub name: String,
    pub severity: String,
    pub status: String,
    pub connection: String,
    pub rows: Option<usize>,
    pub truncated: bool,
    /// Perubahan jumlah baris dibanding run sebelumnya (`+3`, `-1`, `=`).
    pub trend: Option<String>,
    pub error: Option<String>,
    pub columns: Vec<String>,
    /// Paling banyak 20 baris contoh.
    pub sample: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct DataCheckRunReport {
    pub run_id: String,
    pub project: String,
    pub environment: Option<String>,
    /// HEALTHY, WARNINGS, atau FAILING (ada Critical gagal).
    pub verdict: String,
    pub checks: Vec<DataCheckOutcome>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SavedDataCheck {
    pub project: String,
    pub id: String,
    pub name: String,
    /// `true` bila menggantikan check yang sudah ada.
    pub replaced: bool,
}

/// Definisi check dari agent (`save_data_check`).
#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct DataCheckInput {
    /// Existing check id to replace; omit to create a new check.
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    /// The business rule the check guards, in one sentence.
    #[serde(default)]
    pub description: String,
    /// One SELECT that returns the rows violating the rule; empty result = healthy.
    pub sql: String,
    /// Environment variable that holds the connection, e.g. "db_main".
    #[serde(default)]
    pub connection_var: Option<String>,
    /// critical, warning (default) or info.
    #[serde(default)]
    pub severity: Option<String>,
    /// Rows tolerated before the check fails (default 0).
    #[serde(default)]
    pub max_rows: Option<u32>,
}

/// Run suite yang sudah disiapkan, menunggu persetujuan bila perlu.
pub struct PreparedSuiteRun {
    pub suite: HttpTestSuite,
    pub scope: RunScope,
    pub vars: HashMap<String, String>,
    pub project_name: Option<String>,
    pub environment: Option<String>,
    /// Alasan persetujuan user diperlukan; `None` = environment aman.
    pub needs_approval: Option<String>,
}

// ─── Fungsi bantu ───────────────────────────────────────────────────────────

fn find_suite(key: &str) -> Result<HttpTestSuite, AgentError> {
    if let Some(s) = http_tests::load_suite(key.trim()) {
        return Ok(s);
    }
    let mut matches: Vec<HttpTestSuite> = http_tests::load_suites()
        .into_iter()
        .filter(|s| s.name.eq_ignore_ascii_case(key.trim()))
        .collect();
    match matches.len() {
        0 => Err(AgentError::Refused(format!(
            "test suite '{key}' not found; call list_test_suites"
        ))),
        1 => Ok(matches.remove(0)),
        n => Err(AgentError::Refused(format!(
            "{n} suites are named '{key}'; use the suite id"
        ))),
    }
}

type EnvChoice = (Option<String>, Option<Environment>, HashMap<String, String>);

/// Environment terpilih: (nama, jenis, variabel dengan secret).
fn env_of(project: Option<&Project>, wanted: Option<&str>) -> Result<EnvChoice, AgentError> {
    let Some(p) = project else {
        return Ok((None, None, HashMap::new()));
    };
    let env = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(name) => Some(
            p.environments
                .iter()
                .find(|e| e.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| {
                    AgentError::Refused(format!(
                        "environment '{name}' not found in project {}",
                        p.name
                    ))
                })?,
        ),
        None => p.active_environment(),
    };
    Ok(match env {
        Some(e) => (
            Some(e.name.clone()),
            e.environment(),
            p.env_vars_with(&e.name, crate::secrets::get_secret),
        ),
        None => (None, None, HashMap::new()),
    })
}

fn step_summary(r: &http_tests::StepResult) -> StepSummary {
    StepSummary {
        id: r.step_id.clone(),
        name: r.name.clone(),
        phase: r.phase.label().to_string(),
        kind: if r.kind.is_http() { "http" } else { "db_query" }.to_string(),
        outcome: r.outcome().label().to_string(),
        status: r.status,
        time_ms: r.time_ms,
        details: report::step_details_text(r),
    }
}

fn reports_dir() -> std::path::PathBuf {
    crate::directory::get_app_data_dir()
        .join("http_tests")
        .join("reports")
}

/// Run suite secara sinkron (di thread pemanggil) lalu simpan run dan
/// laporan HTML-nya.
pub fn execute_suite_run(
    prepared: PreparedSuiteRun,
    client: &str,
    cancel: &AtomicBool,
) -> SuiteRunReport {
    let PreparedSuiteRun {
        mut suite,
        scope,
        vars,
        project_name,
        environment,
        ..
    } = prepared;
    http_tests::fill_test_credentials(&mut suite.variables);
    let run_id = vars.get("run_id").cloned().unwrap_or_default();
    let started = chrono::Utc::now();
    let abort = AtomicBool::new(false);
    let summary = match http_tests::preflight(&suite, &scope, &vars) {
        Ok(()) => http_tests::run_with_session_as(
            &suite,
            &scope,
            vars,
            Some(client),
            cancel,
            &abort,
            &mut |_| {},
        ),
        Err(e) => http_tests::preflight_failed(&suite, &scope, e),
    };
    let mut run = TestRun::from_summary(&suite, &scope, run_id, started, summary);
    run.project_name = project_name;
    run.environment = environment;
    if let Err(e) = http_tests::save_run(&run) {
        log::warn!("[AGENT] {e}");
    }
    let history = http_tests::load_runs(&suite.id, 20);
    let html = ReportFormat::Html.render(&ReportInput {
        suite: &suite,
        run: &run,
        history: &history,
        catalog: &[],
    });
    let dir = reports_dir();
    let report_path = std::fs::create_dir_all(&dir)
        .and_then(|_| {
            let path = dir.join(report::report_file_name(&run, ReportFormat::Html));
            std::fs::write(&path, html).map(|_| path)
        })
        .map_err(|e| log::warn!("[AGENT] cannot write test report: {e}"))
        .ok()
        .map(|p| p.display().to_string());
    let st = report::compute(&run);
    SuiteRunReport {
        run_id: run.run_id.clone(),
        suite_id: suite.id.clone(),
        suite: suite.name.clone(),
        environment: run.environment.clone(),
        verdict: st.verdict().label().to_string(),
        passed: st.tests.passed,
        failed: st.tests.failed,
        error: st.tests.error,
        blocked: st.tests.blocked,
        skipped: st.tests.skipped,
        data_clean: st.data_clean(),
        steps: run.results.iter().map(step_summary).collect(),
        report_path,
        preflight_error: run.preflight_error.clone(),
    }
}

/// Jalankan `f` di thread biasa dan tunggu hasilnya secara async.
pub async fn on_thread<T, F>(f: F) -> Result<T, AgentError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.await
        .map_err(|_| AgentError::Query("the worker thread stopped unexpectedly".into()))
}

impl HeadlessSession {
    pub async fn list_test_suites(&self, project: Option<&str>) -> Result<SuiteList, AgentError> {
        let projects = crate::project::load_all(self.app_dir());
        let pid = match project.map(str::trim).filter(|p| !p.is_empty()) {
            Some(k) => Some(self.find_project(k)?.id),
            None => None,
        };
        let suites = http_tests::load_suites()
            .into_iter()
            .filter(|s| pid.is_none() || s.project_id == pid)
            .map(|s| {
                let last_run = http_tests::load_runs(&s.id, 1)
                    .first()
                    .map(|r| report::compute(r).verdict().label().to_string());
                SuiteSummary {
                    project: s
                        .project_id
                        .as_deref()
                        .and_then(|id| projects.iter().find(|p| p.id == id))
                        .map(|p| p.name.clone()),
                    steps: s.steps.len(),
                    db_steps: s.steps.iter().filter(|x| x.is_db()).count(),
                    concurrent_steps: s.steps.iter().filter(|x| x.concurrent().is_some()).count(),
                    groups: s.group_names(),
                    last_validation: s.validation.as_ref().map(|v| SuiteValidationInfo {
                        at: v.at.clone(),
                        environment: v.environment.clone(),
                        passed: v.passed,
                        total: v.total,
                        current: s.validation_current().is_some(),
                    }),
                    last_run,
                    id: s.id,
                    name: s.name,
                }
            })
            .collect();
        Ok(SuiteList { suites })
    }

    /// Siapkan run: suite, environment, dan keputusan persetujuan.
    /// Development/Testing/Local/Staging tidak perlu persetujuan; Production
    /// wajib `allow_production` **dan** persetujuan user; environment tanpa
    /// jenis perlu persetujuan.
    pub fn prepare_suite_run(
        &self,
        suite_key: &str,
        env: Option<&str>,
        groups: Vec<String>,
        allow_production: bool,
    ) -> Result<PreparedSuiteRun, AgentError> {
        let suite = find_suite(suite_key)?;
        let project = suite
            .project_id
            .as_deref()
            .and_then(|id| self.find_project(id).ok());
        let (env_name, kind, env_vars) = env_of(project.as_ref(), env)?;
        let needs_approval = match kind {
            Some(Environment::Production) => {
                if !allow_production {
                    return Err(AgentError::Refused(format!(
                        "environment '{}' is Production; integration tests create and delete data. \
                         Pass allow_production=true and the user will be asked to approve.",
                        env_name.unwrap_or_default()
                    )));
                }
                Some("integration test run on a Production environment".to_string())
            }
            Some(_) => None,
            None => Some(format!(
                "integration test run on environment '{}' whose kind is not set",
                env_name.as_deref().unwrap_or("(none)")
            )),
        };
        let known = suite.group_names();
        if let Some(g) = groups.iter().find(|g| !known.contains(g)) {
            return Err(AgentError::Refused(format!(
                "unknown group '{g}'; groups in this suite: {}",
                known.join(", ")
            )));
        }
        let mut scope = RunScope::groups(&suite, groups);
        scope.allow_production = allow_production && kind == Some(Environment::Production);
        let vars = http_tests::initial_vars_with(&suite, &env_vars);
        Ok(PreparedSuiteRun {
            suite,
            scope,
            vars,
            project_name: project.map(|p| p.name),
            environment: env_name,
            needs_approval,
        })
    }

    pub async fn get_test_run(
        &self,
        run_id: &str,
        suite: Option<&str>,
    ) -> Result<TestRunDetail, AgentError> {
        let suites: Vec<HttpTestSuite> = match suite {
            Some(k) => vec![find_suite(k)?],
            None => http_tests::load_suites(),
        };
        for s in &suites {
            let Some(run) = http_tests::load_runs(&s.id, 50)
                .into_iter()
                .find(|r| r.run_id == run_id.trim())
            else {
                continue;
            };
            let steps = run
                .results
                .iter()
                .map(|r| {
                    let def = s.steps.iter().find(|x| x.id == r.step_id);
                    let request = def.filter(|d| d.kind.is_http()).map(|d| RequestInfo {
                        method: d.request.method.label().to_string(),
                        url: crate::http_ai::redact_url(&d.request.url),
                        headers: http_tests::redact_headers(
                            &d.request
                                .headers
                                .iter()
                                .filter(|(k, _, on)| *on && !k.is_empty())
                                .map(|(k, v, _)| (k.clone(), v.clone()))
                                .collect::<Vec<_>>(),
                        ),
                        body: http_tests::redact_body(&d.request.body_text),
                    });
                    StepDetail {
                        id: r.step_id.clone(),
                        name: r.name.clone(),
                        phase: r.phase.label().to_string(),
                        outcome: r.outcome().label().to_string(),
                        request,
                        sql: r.db.as_ref().map(|d| d.sql.clone()),
                        sent_to: r.url.clone(),
                        status: r.status,
                        time_ms: r.time_ms,
                        error: r.error.clone(),
                        failed_assertions: r
                            .assertions
                            .iter()
                            .filter(|a| !a.passed)
                            .map(|a| {
                                format!(
                                    "expected {}, actual {}",
                                    a.assertion.describe(),
                                    a.actual.as_deref().unwrap_or("(missing)")
                                )
                            })
                            .collect(),
                        response_headers: r.response_headers.clone(),
                        response_body: r.body_preview.clone(),
                        columns: r.db.as_ref().map(|d| d.columns.clone()).unwrap_or_default(),
                        rows: r.db.as_ref().map(|d| d.sample.clone()).unwrap_or_default(),
                        burst: r
                            .instances
                            .iter()
                            .map(|i| format!("{}#{}={}", i.round, i.index, i.status))
                            .collect(),
                    }
                })
                .collect();
            return Ok(TestRunDetail {
                verdict: report::compute(&run).verdict().label().to_string(),
                run_id: run.run_id.clone(),
                suite_id: s.id.clone(),
                suite: s.name.clone(),
                environment: run.environment.clone(),
                started_at: run.started_at.clone(),
                residue: run
                    .residue
                    .iter()
                    .map(|r| format!("{} ({} {}): {}", r.step_name, r.method, r.url, r.reason))
                    .collect(),
                steps,
            });
        }
        Err(AgentError::Refused(format!(
            "test run '{run_id}' not found; run ids come from run_test_suite"
        )))
    }

    pub async fn list_data_checks(&self, project: &str) -> Result<DataCheckList, AgentError> {
        let p = self.find_project(project)?;
        let set = data_checks::load_set(&p.id);
        let last = data_checks::load_runs(&p.id, 1).into_iter().next();
        let checks = set
            .checks
            .iter()
            .map(|c| {
                let r = last
                    .as_ref()
                    .and_then(|run| run.results.iter().find(|x| x.check_id == c.id));
                DataCheckInfo {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    description: c.description.clone(),
                    severity: c.severity.label().to_string(),
                    enabled: c.enabled,
                    connection_var: c.connection_var.clone(),
                    sql: c.sql.clone(),
                    last_status: r.map(|x| x.status.label().to_string()),
                    last_rows: r.and_then(|x| x.row_count),
                    last_run_at: r.and(last.as_ref().map(|l| l.started_at.clone())),
                }
            })
            .collect();
        Ok(DataCheckList {
            project: p.name,
            checks,
        })
    }

    /// Jalankan data check (read-only) atas nama `client`. Hasilnya sama
    /// dengan `tabular check run` karena memakai jalur yang sama.
    pub async fn run_data_checks(
        &self,
        client: &str,
        project: &str,
        env: Option<&str>,
        only: Vec<String>,
    ) -> Result<DataCheckRunReport, AgentError> {
        let p = self.find_project(project)?;
        let set = data_checks::load_set(&p.id);
        if set.checks.is_empty() {
            return Err(AgentError::Refused(format!(
                "project {} has no data checks; add one with save_data_check",
                p.name
            )));
        }
        if let Some(k) = only.iter().find(|k| set.check(k).is_none()) {
            return Err(AgentError::Refused(format!(
                "data check '{k}' not found; call list_data_checks"
            )));
        }
        let (env_name, _, vars) = env_of(Some(&p), env)?;
        let req = data_checks::RunRequest {
            project_id: p.id.clone(),
            project_name: p.name.clone(),
            environment: env_name,
            vars,
            only,
        };
        let client = client.to_string();
        let run =
            on_thread(move || data_checks::run_and_save(&set, &req, Some(&client), &mut |_| {}))
                .await?
                .map_err(AgentError::Query)?;
        Ok(DataCheckRunReport {
            verdict: run.verdict().to_string(),
            run_id: run.run_id.clone(),
            project: run.project_name.clone(),
            environment: run.environment.clone(),
            checks: run
                .results
                .iter()
                .map(|r| DataCheckOutcome {
                    id: r.check_id.clone(),
                    name: r.name.clone(),
                    severity: r.severity.label().to_string(),
                    status: r.status.label().to_string(),
                    connection: r.connection.clone(),
                    rows: r.row_count,
                    truncated: r.truncated,
                    trend: r.trend(),
                    error: r.error.clone(),
                    columns: r.columns.clone(),
                    sample: r.sample.clone(),
                })
                .collect(),
        })
    }

    /// Validasi definisi check dari agent; belum menyimpan.
    pub fn prepare_data_check(
        &self,
        project: &str,
        input: &DataCheckInput,
    ) -> Result<(Project, DataCheck), AgentError> {
        let p = self.find_project(project)?;
        if p.is_read_only() {
            return Err(AgentError::Refused(format!(
                "project {} is shared with you as viewer; data checks cannot be changed",
                p.name
            )));
        }
        let mut c = DataCheck::new(&input.name);
        if let Some(id) = input.id.as_deref().filter(|i| !i.trim().is_empty()) {
            c.id = id.trim().to_string();
        }
        c.description = input.description.trim().to_string();
        c.sql = input.sql.trim().to_string();
        if let Some(v) = input
            .connection_var
            .as_deref()
            .filter(|v| !v.trim().is_empty())
        {
            c.connection_var = v.trim().trim_matches(['{', '}']).to_string();
        }
        c.severity = input
            .severity
            .as_deref()
            .map(Severity::parse)
            .unwrap_or_default();
        c.max_rows = input.max_rows;
        data_checks::validate(&c).map_err(AgentError::Refused)?;
        Ok((p, c))
    }

    /// Simpan check yang sudah disetujui user.
    pub fn commit_data_check(
        &self,
        project: &Project,
        check: DataCheck,
    ) -> Result<SavedDataCheck, AgentError> {
        let mut set = data_checks::load_set(&project.id);
        let existed = set.check(&check.id).is_some() || set.check(&check.name).is_some();
        let name = check.name.clone();
        let id = set.upsert(check);
        data_checks::save_set(&set).map_err(AgentError::Refused)?;
        Ok(SavedDataCheck {
            project: project.name.clone(),
            id,
            name,
            replaced: existed,
        })
    }
}
