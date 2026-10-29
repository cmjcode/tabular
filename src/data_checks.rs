//! Data check: kumpulan query invariant per project yang **seharusnya
//! mengembalikan 0 baris**. Baris yang muncul adalah bukti data rusak.
//!
//! Melengkapi suite integration test: suite menguji kode di Staging, data
//! check menjaga data di Production. Selalu read-only (lewat
//! [`crate::http_test_db`] dan gerbang `agent::bound_query`), dengan batas
//! baris dan timeout, sehingga aman untuk Production tanpa
//! `--allow-production`.
//!
//! Headless. Penyimpanan:
//! - definisi: `{app_data}/data_checks/<project_id>.json`;
//! - riwayat run: `{app_data}/data_checks/runs/<project_id>/<run_id>.json`
//!   (maksimal [`MAX_RUNS_PER_PROJECT`]), dipakai untuk tren.
//!
//! Dipakai oleh `tabular check` ([`crate::data_check_cli`]), tool MCP
//! `list_data_checks`/`run_data_checks`/`save_data_check`, dan panel Data
//! Health di GUI.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::http_test_db::{DbExecutor, DbRequest, QueryOutput, SAMPLE_ROWS};

// ─── Model ──────────────────────────────────────────────────────────────────

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    #[default]
    Warning,
    Info,
}

impl Severity {
    pub const ALL: [Severity; 3] = [Severity::Critical, Severity::Warning, Severity::Info];

    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "Critical",
            Severity::Warning => "Warning",
            Severity::Info => "Info",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "critical" | "high" | "error" | "blocker" => Severity::Critical,
            "info" | "low" | "notice" => Severity::Info,
            _ => Severity::Warning,
        }
    }
}

/// Satu invariant: SELECT yang hasil kosongnya berarti sehat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCheck {
    pub id: String,
    /// Mis. "Duplicate receipts per document".
    pub name: String,
    /// Aturan bisnis yang dijaga.
    #[serde(default)]
    pub description: String,
    /// Variabel environment project yang berisi koneksi (nama atau id),
    /// di-resolve per environment seperti langkah DB suite.
    pub connection_var: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Satu SELECT; hasil kosong = sehat. Boleh memakai `{{variabel}}`
    /// environment (diikat sebagai parameter).
    pub sql: String,
    #[serde(default)]
    pub severity: Severity,
    /// Toleransi jumlah baris bermasalah; default 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rows: Option<u32>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl DataCheck {
    pub fn new(name: &str) -> Self {
        Self {
            id: crate::http_collection::unique_id("chk"),
            name: name.trim().to_string(),
            description: String::new(),
            connection_var: "db_main".into(),
            database: None,
            sql: String::new(),
            severity: Severity::Warning,
            max_rows: None,
            enabled: true,
        }
    }

    pub fn tolerance(&self) -> usize {
        self.max_rows.unwrap_or(0) as usize
    }
}

/// Jadwal run otomatis per project (dijalankan GUI selama aplikasi terbuka).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckSchedule {
    pub enabled: bool,
    /// Jam harian `HH:MM` (waktu lokal), mis. `06:00`.
    #[serde(default = "default_daily_at")]
    pub daily_at: String,
    /// Environment project yang diperiksa (kosong = environment aktif).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

impl Default for CheckSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_at: default_daily_at(),
            environment: None,
        }
    }
}

fn default_daily_at() -> String {
    "06:00".into()
}

/// Isi file `data_checks/<project_id>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DataCheckSet {
    #[serde(default)]
    pub project_id: String,
    #[serde(default)]
    pub checks: Vec<DataCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<CheckSchedule>,
}

impl DataCheckSet {
    pub fn check(&self, key: &str) -> Option<&DataCheck> {
        let key = key.trim();
        self.checks.iter().find(|c| c.id == key).or_else(|| {
            self.checks
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(key))
        })
    }

    /// Tambah atau ganti (berdasarkan id, lalu nama) satu check. Mengembalikan id.
    pub fn upsert(&mut self, mut check: DataCheck) -> String {
        if check.id.trim().is_empty() {
            check.id = crate::http_collection::unique_id("chk");
        }
        let pos = self
            .checks
            .iter()
            .position(|c| c.id == check.id)
            .or_else(|| {
                self.checks
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&check.name))
            });
        match pos {
            Some(i) => {
                check.id = self.checks[i].id.clone();
                let id = check.id.clone();
                self.checks[i] = check;
                id
            }
            None => {
                let id = check.id.clone();
                self.checks.push(check);
                id
            }
        }
    }
}

/// Validasi definisi sebelum disimpan atau dijalankan.
pub fn validate(check: &DataCheck) -> Result<(), String> {
    if check.name.trim().is_empty() {
        return Err("A data check needs a name".into());
    }
    if check.connection_var.trim().is_empty() {
        return Err("A data check needs a connection variable (e.g. db_main)".into());
    }
    let step = crate::http_tests::HttpTestStep {
        kind: crate::http_tests::StepKind::DbQuery,
        db: Some(crate::http_tests::DbQueryStep {
            connection_var: check.connection_var.clone(),
            sql: check.sql.clone(),
            ..Default::default()
        }),
        ..Default::default()
    };
    match crate::http_tests::step_kind_issues(&step)
        .into_iter()
        .next()
    {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ─── Penyimpanan ────────────────────────────────────────────────────────────

pub const MAX_RUNS_PER_PROJECT: usize = 100;

pub fn checks_dir() -> PathBuf {
    crate::directory::get_app_data_dir().join("data_checks")
}

fn safe_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn load_set(project_id: &str) -> DataCheckSet {
    load_set_in(&checks_dir(), project_id)
}

pub fn load_set_in(dir: &Path, project_id: &str) -> DataCheckSet {
    let path = dir.join(format!("{}.json", safe_id(project_id)));
    let mut set = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<DataCheckSet>(&bytes).unwrap_or_else(|e| {
            log::warn!("[DATA_CHECKS] cannot read {}: {e}", path.display());
            DataCheckSet::default()
        }),
        Err(_) => DataCheckSet::default(),
    };
    set.project_id = project_id.to_string();
    set
}

pub fn save_set(set: &DataCheckSet) -> Result<(), String> {
    save_set_in(&checks_dir(), set)
}

pub fn save_set_in(dir: &Path, set: &DataCheckSet) -> Result<(), String> {
    if set.project_id.trim().is_empty() {
        return Err("Data checks need a project".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not save data checks: {e}"))?;
    let path = dir.join(format!("{}.json", safe_id(&set.project_id)));
    let json = serde_json::to_string_pretty(set).map_err(|e| e.to_string())?;
    crate::directory::write_file_atomically(&path, json.as_bytes())
        .map_err(|e| format!("Could not save data checks: {e}"))
}

// ─── Hasil ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// Baris bermasalah tidak melebihi toleransi.
    Healthy,
    /// Ada baris bermasalah: data rusak.
    Failing,
    /// Query tidak bisa dijalankan (koneksi, SQL, variabel).
    Error,
    /// Dinonaktifkan.
    Skipped,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            CheckStatus::Healthy => "Healthy",
            CheckStatus::Failing => "Failing",
            CheckStatus::Error => "Error",
            CheckStatus::Skipped => "Skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckResult {
    pub check_id: String,
    pub name: String,
    pub severity: Severity,
    pub status: CheckStatus,
    #[serde(default)]
    pub connection: String,
    /// Jumlah baris bermasalah; `None` bila query gagal.
    pub row_count: Option<usize>,
    /// Lebih dari batas baris; `row_count` adalah batas bawah.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub tolerance: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    /// Paling banyak [`SAMPLE_ROWS`] baris.
    #[serde(default)]
    pub sample: Vec<Vec<String>>,
    #[serde(default)]
    pub time_ms: u128,
    /// SQL dengan nilai parameter (rahasia disamarkan).
    #[serde(default)]
    pub sql: String,
    /// Jumlah baris pada run sebelumnya (tren).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_rows: Option<usize>,
}

impl CheckResult {
    /// Tren dibanding run sebelumnya: `+3`, `-2`, `=`.
    pub fn trend(&self) -> Option<String> {
        let (now, before) = (self.row_count?, self.previous_rows?);
        Some(match now.cmp(&before) {
            std::cmp::Ordering::Greater => format!("+{}", now - before),
            std::cmp::Ordering::Less => format!("-{}", before - now),
            std::cmp::Ordering::Equal => "=".into(),
        })
    }
}

/// Satu run data check.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CheckRun {
    pub run_id: String,
    pub project_id: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub environment: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    #[serde(default)]
    pub operator: String,
    #[serde(default)]
    pub host: String,
    pub results: Vec<CheckResult>,
}

impl CheckRun {
    /// Ada check Critical yang gagal atau tidak bisa diperiksa.
    pub fn critical_failed(&self) -> bool {
        self.results.iter().any(|r| {
            r.severity == Severity::Critical
                && matches!(r.status, CheckStatus::Failing | CheckStatus::Error)
        })
    }

    pub fn count(&self, status: CheckStatus) -> usize {
        self.results.iter().filter(|r| r.status == status).count()
    }

    /// `FAILING` (ada Critical gagal), `WARNINGS`, atau `HEALTHY`.
    pub fn verdict(&self) -> &'static str {
        if self.critical_failed() {
            "FAILING"
        } else if self
            .results
            .iter()
            .any(|r| matches!(r.status, CheckStatus::Failing | CheckStatus::Error))
        {
            "WARNINGS"
        } else {
            "HEALTHY"
        }
    }

    /// Check yang berubah dari sehat (run sebelumnya) menjadi gagal.
    pub fn regressions(&self, previous: Option<&CheckRun>) -> Vec<&CheckResult> {
        let Some(prev) = previous else {
            return Vec::new();
        };
        self.results
            .iter()
            .filter(|r| r.status == CheckStatus::Failing)
            .filter(|r| {
                prev.results
                    .iter()
                    .find(|p| p.check_id == r.check_id)
                    .is_some_and(|p| p.status == CheckStatus::Healthy)
            })
            .collect()
    }
}

fn runs_dir() -> PathBuf {
    checks_dir().join("runs")
}

pub fn save_run(run: &CheckRun) -> Result<(), String> {
    save_run_in(&runs_dir(), run)
}

pub fn save_run_in(dir: &Path, run: &CheckRun) -> Result<(), String> {
    let dir = dir.join(safe_id(&run.project_id));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not save data check run: {e}"))?;
    let path = dir.join(format!("{}.json", safe_id(&run.run_id)));
    let json = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    crate::directory::write_file_atomically(&path, json.as_bytes())
        .map_err(|e| format!("Could not save data check run: {e}"))?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().and_then(|e| e.to_str()) == Some("json"));
    if files.len() > MAX_RUNS_PER_PROJECT {
        files.sort();
        for old in &files[..files.len() - MAX_RUNS_PER_PROJECT] {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

/// Run project, terbaru lebih dulu.
pub fn load_runs(project_id: &str, limit: usize) -> Vec<CheckRun> {
    load_runs_in(&runs_dir(), project_id, limit)
}

pub fn load_runs_in(dir: &Path, project_id: &str, limit: usize) -> Vec<CheckRun> {
    let Ok(entries) = std::fs::read_dir(dir.join(safe_id(project_id))) else {
        return Vec::new();
    };
    let mut runs: Vec<CheckRun> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs.truncate(limit);
    runs
}

// ─── Eksekusi ───────────────────────────────────────────────────────────────

fn blank_result(check: &DataCheck, status: CheckStatus) -> CheckResult {
    CheckResult {
        check_id: check.id.clone(),
        name: check.name.clone(),
        severity: check.severity,
        status,
        connection: String::new(),
        row_count: None,
        truncated: false,
        tolerance: check.tolerance(),
        error: None,
        columns: Vec::new(),
        sample: Vec::new(),
        time_ms: 0,
        sql: String::new(),
        previous_rows: None,
    }
}

/// Hasil satu check dari hasil query `(koneksi, sql tampil, output)`.
pub fn evaluate(
    check: &DataCheck,
    outcome: Result<(String, String, QueryOutput), String>,
    previous: Option<&CheckRun>,
) -> CheckResult {
    let mut r = blank_result(check, CheckStatus::Error);
    r.previous_rows = previous
        .and_then(|p| p.results.iter().find(|x| x.check_id == check.id))
        .and_then(|x| x.row_count);
    match outcome {
        Err(e) => r.error = Some(e),
        Ok((connection, sql, out)) => {
            r.connection = connection;
            r.sql = sql;
            r.row_count = Some(out.rows.len());
            r.truncated = out.truncated;
            r.time_ms = out.time_ms;
            r.status = if out.rows.len() > check.tolerance() {
                CheckStatus::Failing
            } else {
                CheckStatus::Healthy
            };
            r.columns = out.columns;
            r.sample = out.rows.into_iter().take(SAMPLE_ROWS).collect();
        }
    }
    r
}

/// Pilihan run.
#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    pub project_id: String,
    pub project_name: String,
    pub environment: Option<String>,
    /// Variabel environment (koneksi + nilai untuk `{{x}}`).
    pub vars: HashMap<String, String>,
    /// Id/nama check yang dijalankan; kosong = semua (yang nonaktif dilewati).
    pub only: Vec<String>,
}

/// Jalankan check secara berurutan dengan `db`.
pub fn run_checks(
    set: &DataCheckSet,
    req: &RunRequest,
    db: &dyn DbExecutor,
    previous: Option<&CheckRun>,
    progress: &mut dyn FnMut(&CheckResult),
) -> CheckRun {
    let started = chrono::Utc::now();
    let (operator, host) = crate::http_tests::operator_and_host();
    let selected = |c: &DataCheck| {
        req.only.is_empty()
            || req
                .only
                .iter()
                .any(|k| k.trim() == c.id || k.trim().eq_ignore_ascii_case(&c.name))
    };
    let mut results = Vec::new();
    for check in set.checks.iter().filter(|c| selected(c)) {
        let r = if !check.enabled && req.only.is_empty() {
            let mut r = blank_result(check, CheckStatus::Skipped);
            r.error = Some("Disabled".into());
            r
        } else if let Err(e) = validate(check) {
            evaluate(check, Err(e), previous)
        } else {
            match req.vars.get(check.connection_var.trim()) {
                None => evaluate(
                    check,
                    Err(format!(
                        "Variable {{{{{}}}}} is not set in environment {}",
                        check.connection_var.trim(),
                        req.environment.as_deref().unwrap_or("(none)")
                    )),
                    previous,
                ),
                Some(conn) => {
                    let q = DbRequest {
                        connection: conn.clone(),
                        database: check.database.clone().filter(|d| !d.trim().is_empty()),
                        sql: check.sql.clone(),
                        vars: req.vars.clone(),
                    };
                    let res = db
                        .query(&q)
                        .map(|r| (r.connection_name, r.sql_display, r.output));
                    evaluate(check, res, previous)
                }
            }
        };
        log::info!(
            "[DATA_CHECKS] {} '{}' -> {} ({:?} rows)",
            req.project_name,
            check.name,
            r.status.label(),
            r.row_count
        );
        progress(&r);
        results.push(r);
    }
    CheckRun {
        run_id: started.format("%Y%m%dT%H%M%S%3fZ").to_string(),
        project_id: req.project_id.clone(),
        project_name: req.project_name.clone(),
        environment: req.environment.clone(),
        started_at: started.to_rfc3339(),
        finished_at: chrono::Utc::now().to_rfc3339(),
        operator,
        host,
        results,
    }
}

/// Jalankan dengan sesi database nyata (selalu read-only; Production boleh
/// karena tidak ada tulisan) lalu simpan riwayatnya. `agent_client` diisi
/// bila dipanggil lewat MCP (koneksi `Blocked` ditolak). Gagal menyimpan
/// riwayat dilaporkan sebagai error, bukan diabaikan.
pub fn run_and_save(
    set: &DataCheckSet,
    req: &RunRequest,
    agent_client: Option<&str>,
    progress: &mut dyn FnMut(&CheckResult),
) -> Result<CheckRun, String> {
    let mut exec = crate::http_test_db::SessionDbExecutor::open(true)?;
    if let Some(c) = agent_client {
        exec = exec.for_agent(c);
    }
    let previous = load_runs(&req.project_id, 1).into_iter().next();
    let run = run_checks(set, req, &exec, previous.as_ref(), progress);
    save_run(&run)?;
    Ok(run)
}

// ─── Jadwal ─────────────────────────────────────────────────────────────────

/// Jam `HH:MM` → menit sejak tengah malam.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Run terjadwal jatuh tempo: jam jadwal hari ini sudah lewat dan run
/// terakhir terjadi sebelum jam itu. `now` dan `last` dalam waktu lokal.
pub fn schedule_due(
    schedule: &CheckSchedule,
    now: chrono::NaiveDateTime,
    last: Option<chrono::NaiveDateTime>,
) -> bool {
    if !schedule.enabled {
        return false;
    }
    let Some(minutes) = parse_hhmm(&schedule.daily_at) else {
        return false;
    };
    let Some(slot) = now.date().and_hms_opt(minutes / 60, minutes % 60, 0) else {
        return false;
    };
    now >= slot && last.is_none_or(|l| l < slot)
}

// ─── Laporan ────────────────────────────────────────────────────────────────

fn esc_md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn rows_text(r: &CheckResult) -> String {
    match r.row_count {
        Some(n) if r.truncated => format!("{n}+"),
        Some(n) => n.to_string(),
        None => "-".into(),
    }
}

pub fn to_markdown(run: &CheckRun, checks: &DataCheckSet) -> String {
    let mut o = String::new();
    let _ = writeln!(o, "# Data health: {}", run.project_name);
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "- Verdict: **{}** ({} healthy, {} failing, {} error, {} skipped)",
        run.verdict(),
        run.count(CheckStatus::Healthy),
        run.count(CheckStatus::Failing),
        run.count(CheckStatus::Error),
        run.count(CheckStatus::Skipped)
    );
    let _ = writeln!(
        o,
        "- Environment: {}",
        run.environment.as_deref().unwrap_or("-")
    );
    let _ = writeln!(
        o,
        "- Run: {} ({} to {})",
        run.run_id, run.started_at, run.finished_at
    );
    let _ = writeln!(o, "- Operator: {}@{}", run.operator, run.host);
    let _ = writeln!(o);
    let _ = writeln!(
        o,
        "| Check | Severity | Status | Rows | Trend | Connection |"
    );
    let _ = writeln!(o, "|---|---|---|---|---|---|");
    for r in &run.results {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} | {} |",
            esc_md(&r.name),
            r.severity.label(),
            r.status.label(),
            rows_text(r),
            r.trend().unwrap_or_else(|| "-".into()),
            esc_md(&r.connection)
        );
    }
    for r in run
        .results
        .iter()
        .filter(|r| matches!(r.status, CheckStatus::Failing | CheckStatus::Error))
    {
        let _ = writeln!(o);
        let _ = writeln!(o, "## {} ({})", r.name, r.status.label());
        if let Some(d) = checks
            .check(&r.check_id)
            .map(|c| c.description.trim())
            .filter(|d| !d.is_empty())
        {
            let _ = writeln!(o, "{d}");
            let _ = writeln!(o);
        }
        if let Some(e) = &r.error {
            let _ = writeln!(o, "Error: {}", esc_md(e));
        }
        if !r.sql.is_empty() {
            let _ = writeln!(o, "```sql\n{}\n```", r.sql);
        }
        if !r.columns.is_empty() && !r.sample.is_empty() {
            let header: Vec<String> = r.columns.iter().map(|c| esc_md(c)).collect();
            let _ = writeln!(o, "| {} |", header.join(" | "));
            let _ = writeln!(o, "|{}", "---|".repeat(r.columns.len()));
            for row in &r.sample {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| esc_md(&crate::repo_scan::truncate_chars(c, 60)))
                    .collect();
                let _ = writeln!(o, "| {} |", cells.join(" | "));
            }
            if r.row_count.unwrap_or(0) > r.sample.len() {
                let _ = writeln!(
                    o,
                    "\nShowing {} of {} row(s).",
                    r.sample.len(),
                    rows_text(r)
                );
            }
        }
    }
    o
}

fn esc_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn to_html(run: &CheckRun, checks: &DataCheckSet) -> String {
    let mut o = String::new();
    let _ = write!(
        o,
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Data health: {}</title>\
         <style>body{{font-family:-apple-system,Segoe UI,sans-serif;margin:24px;color:#222}}\
         table{{border-collapse:collapse;margin:8px 0 16px}}td,th{{border:1px solid #ccc;padding:4px 8px;font-size:13px;text-align:left}}\
         th{{background:#f3f3f3}}.Failing{{color:#b00020;font-weight:600}}.Error{{color:#b26a00;font-weight:600}}\
         .Healthy{{color:#1b7f3b}}pre{{background:#f6f6f6;padding:8px;overflow:auto}}</style></head><body>",
        esc_html(&run.project_name)
    );
    let _ = write!(
        o,
        "<h1>Data health: {}</h1><p><strong>{}</strong> &middot; environment {} &middot; {} &middot; {}@{}</p>",
        esc_html(&run.project_name),
        run.verdict(),
        esc_html(run.environment.as_deref().unwrap_or("-")),
        esc_html(&run.started_at),
        esc_html(&run.operator),
        esc_html(&run.host)
    );
    o.push_str("<table><tr><th>Check</th><th>Severity</th><th>Status</th><th>Rows</th><th>Trend</th><th>Connection</th></tr>");
    for r in &run.results {
        let _ = write!(
            o,
            "<tr><td>{}</td><td>{}</td><td class=\"{}\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc_html(&r.name),
            r.severity.label(),
            r.status.label(),
            r.status.label(),
            rows_text(r),
            esc_html(&r.trend().unwrap_or_else(|| "-".into())),
            esc_html(&r.connection)
        );
    }
    o.push_str("</table>");
    for r in run
        .results
        .iter()
        .filter(|r| matches!(r.status, CheckStatus::Failing | CheckStatus::Error))
    {
        let _ = write!(
            o,
            "<h2>{} <span class=\"{}\">{}</span></h2>",
            esc_html(&r.name),
            r.status.label(),
            r.status.label()
        );
        if let Some(d) = checks
            .check(&r.check_id)
            .map(|c| c.description.trim())
            .filter(|d| !d.is_empty())
        {
            let _ = write!(o, "<p>{}</p>", esc_html(d));
        }
        if let Some(e) = &r.error {
            let _ = write!(o, "<p class=\"Error\">{}</p>", esc_html(e));
        }
        if !r.sql.is_empty() {
            let _ = write!(o, "<pre>{}</pre>", esc_html(&r.sql));
        }
        if !r.sample.is_empty() {
            o.push_str("<table><tr>");
            for c in &r.columns {
                let _ = write!(o, "<th>{}</th>", esc_html(c));
            }
            o.push_str("</tr>");
            for row in &r.sample {
                o.push_str("<tr>");
                for c in row {
                    let _ = write!(
                        o,
                        "<td>{}</td>",
                        esc_html(&crate::repo_scan::truncate_chars(c, 120))
                    );
                }
                o.push_str("</tr>");
            }
            o.push_str("</table>");
        }
    }
    o.push_str("</body></html>\n");
    o
}

fn esc_xml(s: &str) -> String {
    s.chars()
        .filter(|c| matches!(c, '\t' | '\n' | '\r') || !c.is_control())
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn to_junit(run: &CheckRun) -> String {
    let tests = run.results.len();
    let failures = run.count(CheckStatus::Failing);
    let errors = run.count(CheckStatus::Error);
    let skipped = run.count(CheckStatus::Skipped);
    let mut o = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = writeln!(
        o,
        "<testsuites name=\"Tabular data checks\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\">"
    );
    let _ = writeln!(
        o,
        "  <testsuite name=\"{}\" tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" timestamp=\"{}\">",
        esc_xml(&format!(
            "{} ({})",
            run.project_name,
            run.environment.as_deref().unwrap_or("-")
        )),
        esc_xml(&run.started_at)
    );
    for r in &run.results {
        let _ = write!(
            o,
            "    <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\"",
            r.severity.label(),
            esc_xml(&r.name),
            r.time_ms as f64 / 1000.0
        );
        match r.status {
            CheckStatus::Healthy => o.push_str("/>\n"),
            CheckStatus::Skipped => o.push_str("><skipped/></testcase>\n"),
            CheckStatus::Failing => {
                let _ = writeln!(
                    o,
                    "><failure message=\"{} row(s) violate the rule (tolerance {})\"/></testcase>",
                    rows_text(r),
                    r.tolerance
                );
            }
            CheckStatus::Error => {
                let _ = writeln!(
                    o,
                    "><error message=\"{}\"/></testcase>",
                    esc_xml(r.error.as_deref().unwrap_or("error"))
                );
            }
        }
    }
    o.push_str("  </testsuite>\n</testsuites>\n");
    o
}

pub fn to_json(run: &CheckRun) -> String {
    serde_json::to_string_pretty(run).unwrap_or_default()
}

/// Render sesuai ekstensi file: `.html` (default), `.md`, `.json`, `.xml` (JUnit).
pub fn render_for_path(path: &str, run: &CheckRun, checks: &DataCheckSet) -> String {
    match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("md") | Some("markdown") => to_markdown(run, checks),
        Some("json") => to_json(run),
        Some("xml") => to_junit(run),
        _ => to_html(run, checks),
    }
}

// ─── Check awal ─────────────────────────────────────────────────────────────

fn starter(id: &str, name: &str, severity: Severity, description: &str, sql: &str) -> DataCheck {
    DataCheck {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        connection_var: "db_main".into(),
        database: None,
        sql: sql.into(),
        severity,
        max_rows: None,
        enabled: true,
    }
}

/// Lima invariant stok kandang (MySQL) dari audit VOLTUNES 2026-10-29.
/// Titik awal: sesuaikan dengan aturan bisnis project sebelum dipakai.
pub fn voltunes_stock_checks() -> Vec<DataCheck> {
    vec![
        starter(
            "chk_dup_receipt",
            "Duplicate receipts per document (30 days)",
            Severity::Critical,
            "A warehouse delivery note or a stock mutation may be received only once. More than one active stock-in for the same document detail means it was received twice (double submit).",
            "SELECT 'warehouse_stock_out' AS source, warehouse_stock_out_detail_id AS document_detail_id,\n       COUNT(*) AS receipts, SUM(total_in) AS total_in, MIN(created_at) AS first_at, MAX(created_at) AS last_at\nFROM kandang_stock_ins\nWHERE deleted_at IS NULL AND warehouse_stock_out_detail_id IS NOT NULL\n  AND created_at >= NOW() - INTERVAL 30 DAY\nGROUP BY warehouse_stock_out_detail_id\nHAVING COUNT(*) > 1\nUNION ALL\nSELECT 'mutation', kandang_stock_mutation_detail_id,\n       COUNT(*), SUM(total_in), MIN(created_at), MAX(created_at)\nFROM kandang_stock_ins\nWHERE deleted_at IS NULL AND kandang_stock_mutation_detail_id IS NOT NULL\n  AND created_at >= NOW() - INTERVAL 30 DAY\nGROUP BY kandang_stock_mutation_detail_id\nHAVING COUNT(*) > 1",
        ),
        starter(
            "chk_balance_vs_tx",
            "Stock balance differs from transactions",
            Severity::Critical,
            "kandang_stocks.qty_balance must equal active stock-ins minus active stock-outs for the same project, coop and product (stock-switch outs excluded).",
            "SELECT ks.kandang_project_id, ks.kandang_id, ks.product_id, ks.qty_balance,\n       COALESCE(i.total_in, 0) - COALESCE(o.total_out, 0) AS qty_from_transactions\nFROM kandang_stocks ks\nLEFT JOIN (\n    SELECT kandang_project_id, kandang_id, product_id, SUM(total_in) AS total_in\n    FROM kandang_stock_ins WHERE deleted_at IS NULL\n    GROUP BY kandang_project_id, kandang_id, product_id\n) i ON i.kandang_project_id = ks.kandang_project_id AND i.kandang_id = ks.kandang_id AND i.product_id = ks.product_id\nLEFT JOIN (\n    SELECT kandang_project_id, kandang_id, product_id, SUM(total_out) AS total_out\n    FROM kandang_stock_outs WHERE deleted_at IS NULL AND type <> 'stock-switch'\n    GROUP BY kandang_project_id, kandang_id, product_id\n) o ON o.kandang_project_id = ks.kandang_project_id AND o.kandang_id = ks.kandang_id AND o.product_id = ks.product_id\nWHERE ks.deleted_at IS NULL\n  AND ABS(ks.qty_balance - (COALESCE(i.total_in, 0) - COALESCE(o.total_out, 0))) > 0.001",
        ),
        starter(
            "chk_orphan_stock_out",
            "Active stock-out from a deleted stock-in",
            Severity::Critical,
            "Deleting a receipt must also remove the stock-outs drawn from it. An active stock-out whose source stock-in is deleted is orphaned.",
            "SELECT so.id AS stock_out_id, so.kandang_stock_in_id, so.total_out, so.type,\n       si.deleted_at AS stock_in_deleted_at\nFROM kandang_stock_outs so\nJOIN kandang_stock_ins si ON si.id = so.kandang_stock_in_id\nWHERE si.deleted_at IS NOT NULL AND so.deleted_at IS NULL",
        ),
        starter(
            "chk_overdrawn_stock_in",
            "Stock-in used beyond its quantity",
            Severity::Warning,
            "A receipt cannot be drawn down beyond what came in. Checked for active projects only.",
            "SELECT ksi.id AS kandang_stock_in_id, ksi.kandang_project_id, ksi.product_id, ksi.total_in,\n       COALESCE(agg.total_out, 0) AS total_out\nFROM kandang_stock_ins ksi\nJOIN kandang_projects kp ON kp.id = ksi.kandang_project_id AND kp.status = 'active'\nLEFT JOIN (\n    SELECT kandang_stock_in_id, SUM(total_out) AS total_out\n    FROM kandang_stock_outs WHERE deleted_at IS NULL\n    GROUP BY kandang_stock_in_id\n) agg ON agg.kandang_stock_in_id = ksi.id\nWHERE ksi.deleted_at IS NULL\n  AND COALESCE(agg.total_out, 0) > ksi.total_in + 0.001",
        ),
        starter(
            "chk_negative_balance",
            "Negative stock balance",
            Severity::Warning,
            "Stock per coop, project and product can never go below zero. Checked for active projects only.",
            "SELECT ks.kandang_project_id, ks.kandang_id, ks.product_id, ks.qty_balance\nFROM kandang_stocks ks\nJOIN kandang_projects kp ON kp.id = ks.kandang_project_id AND kp.status = 'active'\nWHERE ks.deleted_at IS NULL AND ks.qty_balance < -0.001",
        ),
    ]
}

// ─── Usulan AI ──────────────────────────────────────────────────────────────

/// Prompt "Generate with AI": aturan bisnis + skema → satu SELECT pelanggar.
pub fn build_generate_prompts(rule: &str, schema: &str, dialect: &str) -> (String, String) {
    let system = format!(
        "You write data integrity checks for a {dialect} database. A check is ONE read-only \
         SELECT that returns the rows that VIOLATE a business rule; an empty result means the \
         data is healthy. Return identifying columns (ids, dates, amounts) so a person can \
         investigate. Exclude soft-deleted rows when the schema has deleted_at. Never write \
         INSERT/UPDATE/DELETE/DDL. Reply with ONLY one JSON object, no prose and no markdown \
         fence: {{\"name\":\"short title\",\"description\":\"the rule in one sentence\",\
         \"severity\":\"critical|warning|info\",\"sql\":\"SELECT ...\"}}"
    );
    let user = format!(
        "Business rule: {}\n\nSchema:\n{}",
        rule.trim(),
        schema.trim()
    );
    (system, user)
}

/// Urai jawaban AI menjadi check (belum disimpan; user harus menyetujui).
pub fn parse_generated(text: &str, connection_var: &str) -> Result<DataCheck, String> {
    let v: serde_json::Value = crate::repo_scan::slice_between(text, '{', '}')
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(|| "AI reply is not JSON".to_string())?;
    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let mut check = DataCheck::new(&s("name"));
    check.description = s("description");
    check.severity = Severity::parse(&s("severity"));
    check.sql = s("sql");
    check.connection_var = connection_var.to_string();
    if check.name.is_empty() {
        check.name = "Generated check".into();
    }
    validate(&check)?;
    Ok(check)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_test_db::DbResponse;

    struct Fake(Vec<(&'static str, Result<QueryOutput, String>)>);

    impl DbExecutor for Fake {
        fn query(&self, req: &DbRequest) -> Result<DbResponse, String> {
            let out = self
                .0
                .iter()
                .find(|(k, _)| req.sql.contains(k))
                .map(|(_, v)| v.clone())
                .unwrap_or(Ok(QueryOutput::default()))?;
            Ok(DbResponse {
                connection_name: req.connection.clone(),
                sql_display: req.sql.clone(),
                output: out,
            })
        }
    }

    fn rows(n: usize) -> QueryOutput {
        QueryOutput {
            columns: vec!["id".into()],
            rows: (0..n).map(|i| vec![i.to_string()]).collect(),
            ..Default::default()
        }
    }

    fn set() -> DataCheckSet {
        let mut a = DataCheck::new("Dup receipts");
        a.id = "a".into();
        a.severity = Severity::Critical;
        a.sql = "SELECT id FROM dup".into();
        let mut b = DataCheck::new("Negative");
        b.id = "b".into();
        b.sql = "SELECT id FROM neg".into();
        b.max_rows = Some(2);
        let mut c = DataCheck::new("Off");
        c.id = "c".into();
        c.sql = "SELECT id FROM off".into();
        c.enabled = false;
        DataCheckSet {
            project_id: "p1".into(),
            checks: vec![a, b, c],
            schedule: None,
        }
    }

    fn req() -> RunRequest {
        RunRequest {
            project_id: "p1".into(),
            project_name: "VOLTUNES".into(),
            environment: Some("Production".into()),
            vars: HashMap::from([("db_main".to_string(), "prod db".to_string())]),
            only: Vec::new(),
        }
    }

    #[test]
    fn runs_checks_with_tolerance_trend_and_verdict() {
        let fake = Fake(vec![("dup", Ok(rows(3))), ("neg", Ok(rows(2)))]);
        let first = run_checks(&set(), &req(), &fake, None, &mut |_| {});
        assert_eq!(first.results[0].status, CheckStatus::Failing);
        assert_eq!(first.results[0].connection, "prod db");
        assert_eq!(first.results[1].status, CheckStatus::Healthy);
        assert_eq!(first.results[2].status, CheckStatus::Skipped);
        assert!(first.critical_failed());
        assert_eq!(first.verdict(), "FAILING");

        let fake = Fake(vec![("dup", Ok(rows(5))), ("neg", Ok(rows(4)))]);
        let second = run_checks(&set(), &req(), &fake, Some(&first), &mut |_| {});
        assert_eq!(second.results[0].trend().as_deref(), Some("+2"));
        let reg = second.regressions(Some(&first));
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].check_id, "b");
        let md = to_markdown(&second, &set());
        assert!(
            md.contains("| Dup receipts | Critical | Failing | 5 | +2 |"),
            "{md}"
        );
        assert!(to_junit(&second).contains("failures=\"2\""));
        assert!(to_html(&second, &set()).contains("FAILING"));
    }

    #[test]
    fn missing_connection_variable_and_writes_are_errors() {
        let mut s = set();
        s.checks[1].sql = "DELETE FROM neg".into();
        let r = RunRequest {
            project_id: "p1".into(),
            only: vec!["a".into(), "Negative".into()],
            ..Default::default()
        };
        let run = run_checks(&s, &r, &Fake(Vec::new()), None, &mut |_| {});
        assert_eq!(run.results.len(), 2);
        assert!(run.results.iter().all(|r| r.status == CheckStatus::Error));
        assert!(run.results[0].error.as_deref().unwrap().contains("db_main"));
        assert!(run.results[1].error.as_deref().unwrap().contains("SELECT"));
    }

    #[test]
    fn storage_round_trip_and_upsert() {
        let dir = std::env::temp_dir().join(format!("tabular_checks_{}", std::process::id()));
        let mut s = set();
        save_set_in(&dir, &s).unwrap();
        assert_eq!(load_set_in(&dir, "p1"), s);
        let mut renamed = DataCheck::new("negative");
        renamed.sql = "SELECT 1".into();
        assert_eq!(s.upsert(renamed), "b");
        assert_eq!(s.checks.len(), 3);
        let run = CheckRun {
            run_id: "r1".into(),
            project_id: "p1".into(),
            started_at: "2026-10-29T06:00:00Z".into(),
            ..Default::default()
        };
        save_run_in(&dir.join("runs"), &run).unwrap();
        assert_eq!(load_runs_in(&dir.join("runs"), "p1", 5).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn voltunes_starters_are_valid_selects() {
        let checks = voltunes_stock_checks();
        assert_eq!(checks.len(), 5);
        assert_eq!(
            checks
                .iter()
                .filter(|c| c.severity == Severity::Critical)
                .count(),
            3
        );
        for c in &checks {
            assert!(validate(c).is_ok(), "{}: {:?}", c.name, validate(c));
        }
    }

    #[test]
    fn schedule_is_due_once_per_day_after_the_slot() {
        let s = CheckSchedule {
            enabled: true,
            daily_at: "06:00".into(),
            environment: None,
        };
        let at = |d: u32, h: u32, m: u32| {
            chrono::NaiveDate::from_ymd_opt(2026, 10, d)
                .unwrap()
                .and_hms_opt(h, m, 0)
                .unwrap()
        };
        assert!(!schedule_due(&s, at(29, 5, 59), None));
        assert!(schedule_due(&s, at(29, 6, 1), None));
        assert!(schedule_due(&s, at(29, 6, 1), Some(at(28, 6, 0))));
        assert!(!schedule_due(&s, at(29, 9, 0), Some(at(29, 6, 2))));
        let off = CheckSchedule {
            enabled: false,
            ..s.clone()
        };
        assert!(!schedule_due(&off, at(29, 7, 0), None));
    }

    #[test]
    fn parses_ai_generated_check() {
        let reply = r#"Here: {"name":"Double receipt","description":"once","severity":"critical","sql":"SELECT id FROM t GROUP BY id HAVING COUNT(*) > 1"}"#;
        let c = parse_generated(reply, "db_main").unwrap();
        assert_eq!(c.severity, Severity::Critical);
        assert!(parse_generated(r#"{"name":"x","sql":"DELETE FROM t"}"#, "db_main").is_err());
    }
}
