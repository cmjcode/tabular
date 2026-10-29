//! Validasi suite integration test terhadap server sungguhan, dengan perbaikan
//! otomatis oleh AI.
//!
//! Satu putaran: jalankan suite (cleanup tetap berjalan) → kumpulkan langkah
//! yang gagal beserta response server dan potongan kode handler dari repo →
//! minta AI memperbaiki langkah tersebut → jalankan lagi. Berhenti saat semua
//! lolos, AI tidak mengubah apa pun, atau batas putaran tercapai.
//!
//! Modul ini headless: dipakai tab Integration Testing lewat job background.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use serde_json::Value;

use crate::agent::harness::ProgressStatus;
use crate::http_tests::{
    self, FolderCatalog, HttpTestStep, HttpTestSuite, RunScope, StepOutcome, StepResult,
    SuiteValidation, TestRun,
};
use crate::repo_scan::{RepoJobEvent, step};

/// Batas putaran bawaan (run + perbaikan).
pub const DEFAULT_ROUNDS: usize = 3;
/// Batas karakter body response per langkah di prompt.
const MAX_RESPONSE_CHARS: usize = 1_500;
/// Batas karakter konteks kode per langkah.
const MAX_CODE_CHARS: usize = 7_000;
/// Batas ukuran file yang dibaca saat mencari handler.
const MAX_FILE_BYTES: u64 = 400 * 1024;

/// Lokasi kode route sebuah request katalog.
#[derive(Debug, Clone)]
pub struct CodeSource {
    pub root: PathBuf,
    /// `path/file:line` relatif terhadap `root`.
    pub source: String,
}

pub struct ValidateInput {
    pub backend: crate::ai_assistant::ChatBackend,
    pub backend_label: String,
    pub suite: HttpTestSuite,
    /// Variabel environment project (dengan secret).
    pub env_vars: HashMap<String, String>,
    pub environment: Option<String>,
    pub catalogs: Vec<FolderCatalog>,
    /// Lokasi kode per id request katalog (`HttpTestStep::request_id`).
    pub code: HashMap<String, CodeSource>,
    pub max_rounds: usize,
}

#[derive(Debug, Clone, Default)]
pub struct RoundInfo {
    pub round: usize,
    pub passed: usize,
    pub total: usize,
    /// Langkah yang diubah AI setelah putaran ini.
    pub fixed: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ValidateOutcome {
    /// Suite setelah perbaikan, dengan `validation` terisi.
    pub suite: HttpTestSuite,
    /// Run putaran terakhir (untuk riwayat dan laporan).
    pub run: TestRun,
    pub rounds: Vec<RoundInfo>,
    pub fixed: usize,
    /// Langkah yang masih gagal beserta alasannya.
    pub remaining: Vec<String>,
    /// Variabel kredensial akun uji yang diisi otomatis.
    pub filled_vars: Vec<String>,
}

pub type ValidateEvent = RepoJobEvent<ValidateOutcome>;

pub struct ValidateHandle {
    pub rx: mpsc::Receiver<ValidateEvent>,
    cancel: Arc<AtomicBool>,
}

impl ValidateHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

// ─── Konteks kode ───────────────────────────────────────────────────────────

fn re(p: &str) -> regex::Regex {
    regex::Regex::new(p).expect("regex statis valid")
}

/// Nama handler dari baris route, mis. `post(auth_routes::register))` →
/// (`auth_routes`, `register`).
pub fn handler_name(route_line: &str) -> Option<(Option<String>, String)> {
    const SKIP: &[&str] = &[
        "get", "post", "put", "patch", "delete", "head", "options", "route", "async", "move",
        "handler", "router", "app", "Router", "new",
    ];
    let path = re(r"([A-Za-z_][A-Za-z0-9_]*(?:(?:::|\.)[A-Za-z_][A-Za-z0-9_]*)*)\s*[),;]");
    let mut best: Option<(Option<String>, String)> = None;
    for cap in path.captures_iter(route_line) {
        let Some(full) = cap.get(1) else { continue };
        let parts: Vec<&str> = full
            .as_str()
            .split([':', '.'])
            .filter(|p| !p.is_empty())
            .collect();
        let Some(name) = parts.last() else { continue };
        if SKIP.contains(name) {
            continue;
        }
        let hint = (parts.len() > 1).then(|| parts[parts.len() - 2].to_string());
        best = Some((hint, name.to_string()));
    }
    best
}

fn lines_window(text: &str, start: usize, count: usize) -> String {
    text.lines()
        .skip(start)
        .take(count)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Cari definisi yang cocok dengan `pattern` di `files`. Path yang memuat
/// `hint` didahulukan. Mengembalikan (path relatif, baris awal, potongan).
fn find_definition(
    root: &Path,
    files: &[PathBuf],
    pattern: &regex::Regex,
    hint: Option<&str>,
    lines: usize,
) -> Option<(String, usize, String)> {
    let mut ordered: Vec<&PathBuf> = files.iter().collect();
    if let Some(h) = hint {
        let h = h.to_ascii_lowercase();
        ordered.sort_by_key(|p| !p.to_string_lossy().to_ascii_lowercase().contains(&h));
    }
    for path in ordered {
        if std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(m) = pattern.find(&text) {
            let line = text[..m.start()].matches('\n').count();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            return Some((rel, line + 1, lines_window(&text, line, lines)));
        }
    }
    None
}

/// Potongan kode untuk satu route: baris route, fungsi handler, dan tipe body
/// request yang dipakai handler. Rahasia literal disamarkan.
pub fn code_context(root: &Path, source: &str, files: &[PathBuf]) -> Option<String> {
    let (file, line) = source.rsplit_once(':')?;
    let line: usize = line.trim().parse().ok()?;
    let text = std::fs::read_to_string(root.join(file)).ok()?;
    let start = line.saturating_sub(4);
    let mut out = format!(
        "// {file}:{line} (route)\n{}\n",
        lines_window(&text, start, 7)
    );
    let route_line = text.lines().nth(line.saturating_sub(1)).unwrap_or("");
    if let Some((hint, name)) = handler_name(route_line) {
        let name_re = regex::escape(&name);
        let def = re(&format!(
            r"(?m)\b(?:fn|def|function|func)\s+(?:\([^)]*\)\s*)?{name_re}\b|\b(?:const|let|var)\s+{name_re}\s*=\s*(?:async\s*)?\("
        ));
        if let Some((rel, at, body)) = find_definition(root, files, &def, hint.as_deref(), 80) {
            out.push_str(&format!("\n// {rel}:{at} (handler {name})\n{body}\n"));
            let types = re(
                r"(?:Json|Form|Query|ValidatedJson|Valid)<\s*([A-Z][A-Za-z0-9_]*)|@RequestBody\s+(?:@\w+\s+)?([A-Z]\w*)|:\s*([A-Z]\w*(?:Request|Req|Dto|DTO|Input|Payload|Body|Schema|Params))\b",
            );
            let mut seen: Vec<String> = Vec::new();
            for cap in types.captures_iter(&body) {
                let Some(t) = cap.get(1).or(cap.get(2)).or(cap.get(3)) else {
                    continue;
                };
                let t = t.as_str().to_string();
                if seen.contains(&t) || seen.len() >= 3 {
                    continue;
                }
                seen.push(t.clone());
                let tre = re(&format!(
                    r"(?m)\b(?:struct|class|interface|type|enum|record)\s+{}\b",
                    regex::escape(&t)
                ));
                if let Some((rel, at, body)) = find_definition(root, files, &tre, None, 45) {
                    out.push_str(&format!("\n// {rel}:{at} (type {t})\n{body}\n"));
                }
            }
        }
    }
    let out = crate::repo_endpoints::redact_code_secrets(&out);
    Some(crate::repo_scan::truncate_chars(&out, MAX_CODE_CHARS))
}

// ─── Prompt perbaikan ───────────────────────────────────────────────────────

/// Langkah yang perlu diperbaiki: gagal assertion atau tanpa response.
pub fn failing_steps(run: &TestRun) -> Vec<&StepResult> {
    run.results
        .iter()
        .filter(|r| matches!(r.outcome(), StepOutcome::Failed | StepOutcome::Error))
        .collect()
}

fn describe_step(step: &HttpTestStep) -> String {
    if let (true, Some(db)) = (step.is_db(), &step.db) {
        let mut o = format!(
            "id={} name=\"{}\" phase={} kind=db_query connection_var={}{}\nsql:\n{}\n",
            step.id,
            step.name,
            step.phase.label().to_ascii_lowercase(),
            db.connection_var,
            db.wait_until_ms
                .map(|w| format!(" wait_until_ms={w}"))
                .unwrap_or_default(),
            db.sql.trim()
        );
        for e in &step.extract {
            o.push_str(&format!("extract {} = {}\n", e.var, e.from));
        }
        for a in &step.assertions {
            o.push_str(&format!("assert {}\n", a.describe()));
        }
        return o;
    }
    let mut o = describe_http_step(step);
    if let Some(c) = step.concurrent() {
        o.push_str(&format!(
            "concurrency: {} requests at once x {} round(s), barrier {}\n",
            c.count(),
            c.rounds(),
            c.barrier
        ));
    }
    o
}

fn describe_http_step(step: &HttpTestStep) -> String {
    let r = &step.request;
    let mut o = format!(
        "id={} name=\"{}\" phase={} group=\"{}\"{}\n{} {}\n",
        step.id,
        step.name,
        step.phase.label().to_ascii_lowercase(),
        step.group,
        step.cleanup_for
            .as_deref()
            .map(|c| format!(" cleanup_for={c}"))
            .unwrap_or_default(),
        r.method.label(),
        r.url
    );
    for (k, v, on) in &r.headers {
        if *on && !k.is_empty() {
            let v = if crate::http_client_widgets::is_sensitive_key(k) {
                crate::http_ai::REDACTED
            } else {
                v.as_str()
            };
            o.push_str(&format!("header {k}: {v}\n"));
        }
    }
    let q: Vec<String> = r
        .params
        .iter()
        .filter(|(k, _, on)| *on && !k.is_empty())
        .map(|(k, v, _)| format!("{k}={v}"))
        .collect();
    if !q.is_empty() {
        o.push_str(&format!("query: {}\n", q.join("&")));
    }
    o.push_str(&format!("auth: {:?}\n", r.auth_type));
    if !r.body_text.trim().is_empty() {
        o.push_str(&format!("body:\n{}\n", r.body_text.trim()));
    }
    for e in &step.extract {
        o.push_str(&format!("extract {} = {}\n", e.var, e.from));
    }
    for a in &step.assertions {
        o.push_str(&format!("assert {}\n", a.describe()));
    }
    o
}

/// Prompt perbaikan untuk langkah yang gagal.
pub fn build_repair_prompts(
    suite: &HttpTestSuite,
    run: &TestRun,
    contexts: &HashMap<String, String>,
    catalogs: &[FolderCatalog],
) -> (String, String) {
    let system = String::from(
        "You fix failing HTTP integration test steps. Each step was sent to the real API server; \
         you get the request, the server's response and the source code of the handler. \
         Make every failing step correct for this API:\n\
         - Use the exact field names, types and required fields from the error message and the \
           code (for example a missing field `nama` means the body must contain `nama`).\n\
         - Fix the URL, method, query, headers, body, auth or extract paths when they are wrong. \
           Extract paths must match the real response JSON. A failed `(extract …)` check means \
           the value was not found in the response.\n\
         - When the token comes back in a cookie (see the response headers), extract it with \
           `cookie:<name>` in the login step. Steps with auth `bearer` and `{{token}}` then send \
           the cookie automatically.\n\
         - When several steps fail for the same reason, fix all of them in this reply. A step \
           can also fail because an earlier step failed (for example a list that should contain \
           created data); fix the earlier step.\n\
         - Fix an assertion only when the API behaviour is clearly correct and the expectation is \
           wrong (for example 200 instead of 201). Never weaken a test that reveals a real bug; \
           leave such a step unchanged and explain it in `notes`.\n\
         - Keep using `{{variables}}` and `{{run_id}}` for unique data; never hardcode hosts or \
           secrets. Variables for test accounts may get example values in `variables`.\n\
         - If data must exist first (for example a parent record), you may add a new setup step \
           with a new id, its extract, and a cleanup step for it.\n\
         - Database steps (`kind: db_query`) check data with one SELECT; fix the SQL (table or \
           column names, joins) when it errors, but never relax the invariant it checks: a wrong \
           balance or a duplicate row is a real bug. Return them as \
           {\"id\":\"…\",\"kind\":\"db_query\",\"db\":{\"connection_var\":\"db_main\",\"sql\":\"SELECT …\"},\"assert\":[…]}.\n\
         - Concurrent steps (`concurrency`) send the same request at once; a `count:status=2xx` \
           failure there usually means a race condition in the API, which is a real bug.\n\
         Reply with ONLY one JSON object, no prose and no markdown fence:\n\
         {\"steps\":[{\"id\":\"s1\",\"name\":\"…\",\"phase\":\"setup|test|cleanup\",\"group\":\"…\",\
         \"cleanup_for\":\"…\",\"method\":\"POST\",\"url\":\"{{base_url_x}}/path\",\"headers\":{},\
         \"query\":{},\"auth\":\"bearer|basic|api_key|none\",\"body\":{},\"extract\":{\"var\":\"json:$.id\"},\
         \"assert\":[{\"target\":\"status\",\"op\":\"equals\",\"value\":\"201\"}]}],\
         \"variables\":{},\"notes\":\"…\"}\n\
         Return each changed step completely, with the same id. Omit steps you do not change.\n",
    );
    let mut user = String::from("Suite outline (execution order):\n");
    user.push_str(&http_tests::suite_outline(suite));
    let vars: Vec<String> = suite
        .variables
        .iter()
        .map(|(k, v)| {
            format!(
                "{k} = {}",
                if v.is_empty() {
                    "(empty)".to_string()
                } else {
                    http_tests::display_var_value(k, v)
                }
            )
        })
        .collect();
    if !vars.is_empty() {
        user.push_str(&format!("\nVariables: {}\n", vars.join(", ")));
    }
    user.push_str("\n# Failing steps\n");
    let deps = http_tests::dependencies(suite);
    for r in failing_steps(run) {
        let Some(step) = suite.steps.iter().find(|s| s.id == r.step_id) else {
            continue;
        };
        // Langkah penyedia variabel: cara nilainya diambil dan header response-nya
        // (mis. cookie token dari langkah login).
        if let Some(i) = suite.step_index(&step.id) {
            for &p in &deps[i].depends_on {
                let prov = &suite.steps[p];
                let Some(pr) = run.results.iter().find(|x| x.step_id == prov.id) else {
                    continue;
                };
                user.push_str(&format!(
                    "\n(step {} \"{}\" provides values used below: {}; result HTTP {})\n",
                    prov.id,
                    prov.name,
                    prov.extract
                        .iter()
                        .map(|e| format!("{} = {}", e.var, e.from))
                        .collect::<Vec<_>>()
                        .join(", "),
                    pr.status
                ));
                if !pr.response_headers.is_empty() {
                    let h: Vec<String> = pr
                        .response_headers
                        .iter()
                        .map(|(k, v)| format!("{k}: {v}"))
                        .collect();
                    user.push_str(&format!("its response headers:\n{}\n", h.join("\n")));
                }
            }
        }
        user.push_str(&format!("\n## Step {}\n", step.id));
        user.push_str(&describe_step(step));
        match (&r.db, &r.error) {
            (_, Some(e)) if step.is_db() => user.push_str(&format!("result: query failed ({e})\n")),
            (Some(db), None) => {
                user.push_str(&format!(
                    "result: {} row(s) after {} attempt(s){}\ncolumns: {}\n",
                    db.row_count,
                    db.attempts,
                    if db.timed_out {
                        ", waited until timeout"
                    } else {
                        ""
                    },
                    db.columns.join(", ")
                ));
                for row in db.sample.iter().take(5) {
                    user.push_str(&format!("row: {}\n", row.join(" | ")));
                }
            }
            (_, Some(e)) => {
                user.push_str(&format!("sent to: {} {}\n", r.method, r.url));
                user.push_str(&format!("result: no response ({e})\n"));
            }
            (None, None) => {
                user.push_str(&format!("sent to: {} {}\n", r.method, r.url));
                user.push_str(&format!("result: HTTP {}\n", r.status));
                if !r.instances.is_empty() {
                    let statuses: Vec<String> = r
                        .instances
                        .iter()
                        .map(|i| format!("r{}#{}={}", i.round, i.index, i.status))
                        .collect();
                    user.push_str(&format!("burst responses: {}\n", statuses.join(" ")));
                }
            }
        }
        for a in r.assertions.iter().filter(|a| !a.passed) {
            user.push_str(&format!(
                "failed: {} (actual {})\n",
                a.assertion.describe(),
                a.actual.as_deref().unwrap_or("missing")
            ));
        }
        if !r.response_headers.is_empty() {
            let h: Vec<String> = r
                .response_headers
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect();
            user.push_str(&format!("response headers:\n{}\n", h.join("\n")));
        }
        if !r.body_preview.trim().is_empty() {
            user.push_str(&format!(
                "response body:\n{}\n",
                crate::repo_scan::truncate_chars(r.body_preview.trim(), MAX_RESPONSE_CHARS)
            ));
        }
        if let Some(code) = contexts.get(&step.id) {
            user.push_str(&format!("source code:\n{code}\n"));
        }
    }
    let catalog = http_tests::catalog_text(catalogs);
    if !catalog.trim().is_empty() {
        user.push_str("\n# Endpoint catalog (for reference)\n");
        user.push_str(&crate::repo_scan::truncate_chars(&catalog, 30_000));
    }
    (system, user)
}

// ─── Menerapkan perbaikan ───────────────────────────────────────────────────

/// Gabungkan langkah hasil perbaikan ke langkah asal: field yang tidak disebut
/// AI (`raw` tidak punya kuncinya) dipertahankan.
fn merge_step(keep: &HttpTestStep, mut next: HttpTestStep, raw: &Value) -> HttpTestStep {
    let has = |k: &str| raw.get(k).is_some();
    next.enabled = keep.enabled;
    // Perbaikan parsial langkah DB (mis. hanya assertion) tetap langkah DB.
    if keep.is_db() && !has("db") && !has("sql") && !has("kind") {
        next.kind = keep.kind;
        next.db = keep.db.clone();
    }
    if !has("concurrency") {
        next.concurrency = keep.concurrency;
    }
    if !has("phase") {
        next.phase = keep.phase;
    }
    if !has("group") {
        next.group = keep.group.clone();
    }
    if !has("cleanup_for") {
        next.cleanup_for = keep.cleanup_for.clone();
    }
    if next.notes.is_empty() {
        next.notes = keep.notes.clone();
    }
    if !has("request_id") {
        next.request_id = keep.request_id.clone();
    }
    if !has("name") {
        next.name = keep.name.clone();
    }
    if !has("extract") {
        next.extract = keep.extract.clone();
    }
    if !has("assert") && !has("assertions") {
        next.assertions = keep.assertions.clone();
    }
    let r = &mut next.request;
    let k = &keep.request;
    if !has("url") {
        r.url = k.url.clone();
    }
    if !has("method") {
        r.method = k.method.clone();
    }
    if !has("body") {
        r.body_type = k.body_type.clone();
        r.body_text = k.body_text.clone();
    }
    if !has("headers") {
        r.headers = k.headers.clone();
    }
    if !has("query") && !has("params") {
        r.params = k.params.clone();
    }
    if !has("auth") {
        r.auth_type = k.auth_type.clone();
        r.bearer_token = k.bearer_token.clone();
        r.basic_user = k.basic_user.clone();
        r.basic_pass = k.basic_pass.clone();
        r.api_key_name = k.api_key_name.clone();
        r.api_key_value = k.api_key_value.clone();
        r.api_key_in_header = k.api_key_in_header;
    }
    if r.tables.is_empty() {
        r.tables = k.tables.clone();
    }
    if r.route.is_none() {
        r.route = k.route.clone();
    }
    if r.source.is_none() {
        r.source = k.source.clone();
    }
    r.id = k.id.clone();
    r.workspace_id = k.workspace_id.clone();
    r.folder_id = k.folder_id.clone();
    next
}

/// Terapkan jawaban AI ke suite. Field yang tidak disebut AI dipertahankan.
/// Langkah dengan id baru ditambahkan. Mengembalikan jumlah langkah berubah.
pub fn apply_repairs(
    suite: &mut HttpTestSuite,
    text: &str,
    catalogs: &[FolderCatalog],
) -> Result<usize, String> {
    let v: Value = crate::repo_scan::slice_between(text, '{', '}')
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(|| {
            format!(
                "AI reply is not JSON: {}",
                crate::repo_scan::truncate_chars(text.trim(), 200)
            )
        })?;
    let raw_steps = v
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut changed = 0;
    let mut new_steps: Vec<HttpTestStep> = Vec::new();
    for raw in &raw_steps {
        // `url` boleh tidak ada pada perbaikan; isi sementara agar parser menerima.
        let mut probe = raw.clone();
        if probe.get("url").is_none()
            && let Value::Object(m) = &mut probe
        {
            m.insert("url".into(), Value::String("{{_}}".into()));
        }
        let Some(parsed) = http_tests::parse_step_value(&probe, catalogs) else {
            continue;
        };
        match suite.steps.iter_mut().find(|s| s.id == parsed.id) {
            Some(orig) => {
                let before = serde_json::to_string(&*orig).unwrap_or_default();
                *orig = merge_step(orig, parsed, raw);
                if serde_json::to_string(&*orig).unwrap_or_default() != before {
                    changed += 1;
                }
            }
            None if raw.get("url").is_some() => new_steps.push(parsed),
            None => {}
        }
    }
    // Variabel: tambah yang baru, isi yang masih kosong bila bukan rahasia.
    if let Some(Value::Object(map)) = v.get("variables") {
        for (k, val) in map {
            let val = match val {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            let secret = http_tests::display_var_value(k, "x") != "x";
            let val = if secret && !http_tests::is_test_credential(k) {
                String::new()
            } else {
                val
            };
            match suite.variables.iter_mut().find(|(x, _)| x == k) {
                Some((_, cur)) if cur.is_empty() && !val.is_empty() => *cur = val,
                Some(_) => {}
                None if k != "run_id" => suite.variables.push((k.clone(), val)),
                None => {}
            }
        }
        http_tests::fill_test_credentials(&mut suite.variables);
    }
    if !new_steps.is_empty() {
        changed += http_tests::merge_generated_steps(suite, new_steps, Vec::new());
    } else {
        suite.ensure_ids();
        suite.normalize_order();
    }
    if changed > 0 {
        suite.touch();
    }
    Ok(changed)
}

// ─── Job ────────────────────────────────────────────────────────────────────

/// (lolos, dihitung) tanpa langkah yang dilewati.
pub fn counts(run: &TestRun) -> (usize, usize) {
    let considered: Vec<&StepResult> = run
        .results
        .iter()
        .filter(|r| r.outcome() != StepOutcome::Skipped)
        .collect();
    let passed = considered.iter().filter(|r| r.passed()).count();
    (passed, considered.len())
}

/// Workspace AI read-only di `root` (Claude Code / Gemini CLI); backend lain
/// tidak membaca repo.
pub fn read_only_workspace(
    backend: &crate::ai_assistant::ChatBackend,
    root: &Path,
) -> crate::ai_assistant::ChatWorkspace {
    workspace_for(backend, &[root.to_path_buf()])
}

/// Workspace AI: agent CLI tertentu boleh membaca repo (read-only).
fn workspace_for(
    backend: &crate::ai_assistant::ChatBackend,
    roots: &[PathBuf],
) -> crate::ai_assistant::ChatWorkspace {
    use crate::config::{AiBackend, CliAgentKind};
    let Some(root) = roots.first().filter(|_| roots.len() == 1) else {
        return Default::default();
    };
    match (&backend.backend, &backend.cli.kind) {
        (AiBackend::Cli, CliAgentKind::ClaudeCode) => crate::ai_assistant::ChatWorkspace {
            cwd: Some(root.clone()),
            allowed_tools: ["Read", "Grep", "Glob", "LS"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            without_mcp: true,
        },
        (AiBackend::Cli, CliAgentKind::GeminiCli) => crate::ai_assistant::ChatWorkspace {
            cwd: Some(root.clone()),
            ..Default::default()
        },
        _ => Default::default(),
    }
}

/// Jalankan validasi di thread background.
pub fn spawn_validate(input: ValidateInput) -> ValidateHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let result = validate(input, &tx, &flag);
        if let Err(e) = &result {
            log::warn!("[HTTP_TESTS] validation failed: {e}");
        }
        let _ = tx.send(RepoJobEvent::Finished(result));
    });
    ValidateHandle { rx, cancel }
}

fn validate(
    input: ValidateInput,
    tx: &mpsc::Sender<ValidateEvent>,
    cancel: &AtomicBool,
) -> Result<ValidateOutcome, String> {
    let ValidateInput {
        backend,
        backend_label,
        suite,
        env_vars,
        environment,
        catalogs,
        code,
        max_rounds,
    } = input;
    let roots: Vec<PathBuf> = {
        let mut r: Vec<PathBuf> = code.values().map(|c| c.root.clone()).collect();
        r.sort();
        r.dedup();
        r
    };
    let workspace = workspace_for(&backend, &roots);
    let mut ask = |system: String, user: String| -> Result<String, String> {
        crate::repo_scan::ask_ai(&backend, system, user, workspace.clone(), tx, cancel)
            .map_err(|e| e.to_string())
    };
    run_validation(
        ValidationPlan {
            suite,
            env_vars,
            environment,
            catalogs,
            code,
            max_rounds,
            ai_label: backend_label,
        },
        &mut ask,
        tx,
        cancel,
    )
}

/// Bahan validasi tanpa backend AI (AI dipanggil lewat `ask`).
pub struct ValidationPlan {
    pub suite: HttpTestSuite,
    pub env_vars: HashMap<String, String>,
    pub environment: Option<String>,
    pub catalogs: Vec<FolderCatalog>,
    pub code: HashMap<String, CodeSource>,
    pub max_rounds: usize,
    pub ai_label: String,
}

/// Loop validasi. `ask(system, user)` mengembalikan jawaban AI.
pub fn run_validation(
    plan: ValidationPlan,
    ask: &mut dyn FnMut(String, String) -> Result<String, String>,
    tx: &mpsc::Sender<ValidateEvent>,
    cancel: &AtomicBool,
) -> Result<ValidateOutcome, String> {
    let ValidationPlan {
        mut suite,
        env_vars,
        environment,
        catalogs,
        code,
        max_rounds,
        ai_label: backend_label,
    } = plan;
    let max_rounds = max_rounds.max(1);
    // Lanjut melewati batas selama jumlah langkah lolos masih bertambah.
    let hard_max = max_rounds * 2;
    let mut prev_passed: Option<usize> = None;
    let mut outcome = ValidateOutcome {
        filled_vars: http_tests::fill_test_credentials(&mut suite.variables),
        ..Default::default()
    };
    let scope = RunScope::default();
    let no_abort = AtomicBool::new(false);
    let mut file_cache: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut idx: u64 = 1;
    for round in 1..=hard_max {
        if cancel.load(Ordering::SeqCst) {
            return Err("Validation cancelled".into());
        }
        let vars = http_tests::initial_vars_with(&suite, &env_vars);
        let run_id = vars.get("run_id").cloned().unwrap_or_default();
        let label = format!("Round {round}: running the suite against the server");
        let _ = tx.send(step(idx, label.clone(), None, ProgressStatus::Active));
        http_tests::preflight(&suite, &scope, &vars)?;
        let started = chrono::Utc::now();
        let summary =
            http_tests::run_with_session(&suite, &scope, vars, cancel, &no_abort, &mut |_| {});
        let mut run = TestRun::from_summary(&suite, &scope, run_id, started, summary);
        run.environment = environment.clone();
        let (passed, total) = counts(&run);
        let failing: Vec<String> = failing_steps(&run)
            .iter()
            .map(|r| r.step_id.clone())
            .collect();
        let _ = tx.send(step(
            idx,
            label,
            Some(format!("{passed}/{total} passed")),
            if failing.is_empty() {
                ProgressStatus::Done
            } else {
                ProgressStatus::Error
            },
        ));
        idx += 1;
        outcome.rounds.push(RoundInfo {
            round,
            passed,
            total,
            fixed: 0,
        });
        let clean = run.residue.is_empty();
        outcome.run = run;
        let improving = prev_passed.is_none_or(|p| passed > p);
        prev_passed = Some(passed);
        let out_of_rounds = round >= hard_max || (round >= max_rounds && !improving);
        if (failing.is_empty() && clean) || out_of_rounds || cancel.load(Ordering::SeqCst) {
            break;
        }
        // Konteks kode untuk langkah yang gagal.
        let mut contexts: HashMap<String, String> = HashMap::new();
        for id in &failing {
            let Some(src) = suite
                .steps
                .iter()
                .find(|s| &s.id == id)
                .and_then(|s| s.request_id.as_ref())
                .and_then(|rid| code.get(rid))
            else {
                continue;
            };
            let files = file_cache.entry(src.root.clone()).or_insert_with(|| {
                crate::repo_scan::list_repo_files(&src.root, cancel)
                    .map(|(f, _)| f)
                    .unwrap_or_default()
            });
            if let Some(ctx) = code_context(&src.root, &src.source, files) {
                contexts.insert(id.clone(), ctx);
            }
        }
        let label = format!(
            "Round {round}: asking {backend_label} to fix {} step(s)",
            failing.len().max(1)
        );
        let _ = tx.send(step(idx, label.clone(), None, ProgressStatus::Active));
        let (system, user) = build_repair_prompts(&suite, &outcome.run, &contexts, &catalogs);
        let reply = ask(system, user)?;
        let fixed = apply_repairs(&mut suite, &reply, &catalogs)?;
        let _ = tx.send(step(
            idx,
            label,
            Some(format!("{fixed} step(s) changed")),
            ProgressStatus::Done,
        ));
        idx += 1;
        if let Some(last) = outcome.rounds.last_mut() {
            last.fixed = fixed;
        }
        outcome.fixed += fixed;
        if fixed == 0 {
            break;
        }
    }
    let (passed, total) = counts(&outcome.run);
    outcome.remaining = failing_steps(&outcome.run)
        .iter()
        .map(|r| {
            format!(
                "{}: {}",
                r.name,
                crate::http_test_report::step_details_text(r)
            )
        })
        .chain(
            outcome
                .run
                .residue
                .iter()
                .map(|r| format!("{} left behind: {}", r.step_name, r.reason)),
        )
        .collect();
    suite.validation = Some(SuiteValidation {
        at: chrono::Utc::now().to_rfc3339(),
        environment,
        rounds: outcome.rounds.len(),
        passed,
        total,
        fixed: outcome.fixed,
        fingerprint: http_tests::suite_fingerprint(&suite),
    });
    suite.touch();
    outcome.suite = suite;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http_collection::SavedRequest;
    use crate::http_tests::{Assertion, StepPhase};
    use crate::models::structs::HttpMethod;

    #[test]
    fn finds_handler_in_route_lines() {
        assert_eq!(
            handler_name(r#"        .route("/api/auth/register", post(auth_routes::register))"#),
            Some((Some("auth_routes".into()), "register".into()))
        );
        assert_eq!(
            handler_name("router.post('/users', userController.create);"),
            Some((Some("userController".into()), "create".into()))
        );
        assert_eq!(
            handler_name(r#"app.get("/health", health)"#),
            Some((None, "health".into()))
        );
    }

    #[test]
    fn builds_code_context_with_handler_and_body_type() {
        let dir = std::env::temp_dir().join(format!(
            "tabular-validate-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        ));
        std::fs::create_dir_all(dir.join("src/auth_routes")).expect("dir");
        std::fs::write(
            dir.join("src/main.rs"),
            "fn main() {\n    let app = Router::new()\n        .route(\"/api/auth/register\", post(auth_routes::register));\n}\n",
        )
        .expect("main");
        std::fs::write(
            dir.join("src/auth_routes/mod.rs"),
            "pub async fn register(State(s): State<App>, Json(req): Json<RegisterRequest>) -> Result<Json<Value>> {\n    todo!()\n}\n\n#[derive(Deserialize)]\npub struct RegisterRequest {\n    pub nama: String,\n    pub email: String,\n}\n",
        )
        .expect("mod");
        let files = vec![dir.join("src/main.rs"), dir.join("src/auth_routes/mod.rs")];
        let ctx = code_context(&dir, "src/main.rs:3", &files).expect("context");
        assert!(ctx.contains("handler register"), "{ctx}");
        assert!(ctx.contains("type RegisterRequest"), "{ctx}");
        assert!(ctx.contains("pub nama: String"), "{ctx}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn suite() -> HttpTestSuite {
        let mut s = HttpTestSuite {
            variables: vec![
                ("test_password".into(), String::new()),
                ("token".into(), String::new()),
            ],
            steps: vec![HttpTestStep {
                id: "s1".into(),
                name: "Register".into(),
                phase: StepPhase::Setup,
                group: "Auth".into(),
                request: SavedRequest {
                    method: HttpMethod::POST,
                    url: "{{base}}/api/auth/register".into(),
                    body_text: r#"{"name":"x"}"#.into(),
                    tables: vec!["users".into()],
                    ..Default::default()
                },
                assertions: vec![Assertion {
                    target: "status".into(),
                    value: "201".into(),
                    ..Default::default()
                }],
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        s.ensure_ids();
        s
    }

    #[test]
    fn repairs_keep_unmentioned_fields_and_fill_test_passwords() {
        let mut s = suite();
        let reply = r#"{"steps":[{"id":"s1",
            "body":{"nama":"Budi {{run_id}}","email":"b{{run_id}}@example.com","password":"{{test_password}}"}}],
            "variables":{"test_password":"","token":"leak"}}"#;
        let n = apply_repairs(&mut s, reply, &[]).expect("apply");
        assert_eq!(n, 1);
        let st = &s.steps[0];
        assert!(st.request.body_text.contains("\"nama\""));
        assert_eq!(st.request.url, "{{base}}/api/auth/register");
        assert_eq!(st.request.method, HttpMethod::POST);
        assert_eq!(st.phase, StepPhase::Setup);
        assert_eq!(st.group, "Auth");
        assert_eq!(st.assertions[0].value, "201");
        assert_eq!(st.request.tables, vec!["users"]);
        let pw = &s
            .variables
            .iter()
            .find(|(k, _)| k == "test_password")
            .expect("pw")
            .1;
        assert!(!pw.is_empty());
        let tok = &s
            .variables
            .iter()
            .find(|(k, _)| k == "token")
            .expect("tok")
            .1;
        assert!(tok.is_empty(), "AI must not fill real secrets");
        assert_eq!(
            apply_repairs(&mut s, r#"{"steps":[],"notes":"ok"}"#, &[]).expect("apply"),
            0
        );
    }

    #[test]
    fn repair_prompt_contains_server_error_and_code() {
        let s = suite();
        let run = TestRun {
            results: vec![StepResult {
                step_id: "s1".into(),
                name: "Register".into(),
                method: "POST".into(),
                url: "http://h/api/auth/register".into(),
                status: 422,
                body_preview: "missing field `nama`".into(),
                assertions: vec![crate::http_tests::AssertResult {
                    assertion: s.steps[0].assertions[0].clone(),
                    passed: false,
                    actual: Some("422".into()),
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut ctx = HashMap::new();
        ctx.insert("s1".to_string(), "pub nama: String".to_string());
        let (_, user) = build_repair_prompts(&s, &run, &ctx, &[]);
        assert!(user.contains("missing field `nama`"));
        assert!(user.contains("HTTP 422"));
        assert!(user.contains("pub nama: String"));
        assert!(user.contains("test_password = (empty)"));
    }

    /// Server mini yang meniru API nyata: register wajib `nama`, token dikirim
    /// lewat cookie `dp-token`, `/me` butuh cookie itu.
    fn spawn_api() -> (u16, Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log2 = log.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body) = loop {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break (String::new(), String::new());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let head = text[..end].to_string();
                        let len: usize = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0);
                        while buf.len() < end + 4 + len {
                            let n = stream.read(&mut chunk).unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                        }
                        let body = String::from_utf8_lossy(&buf[end + 4..]).to_string();
                        break (head, body);
                    }
                };
                let line = head.lines().next().unwrap_or("").to_string();
                log2.lock().expect("log").push(line.clone());
                let cookie_ok = head.lines().any(|l| {
                    l.to_ascii_lowercase().starts_with("cookie:") && l.contains("dp-token=TOK")
                });
                let (status, extra, payload) = if line.starts_with("POST /api/auth/register") {
                    if body.contains("\"nama\"") {
                        (
                            "201 Created",
                            "Set-Cookie: dp-token=TOK; HttpOnly; Path=/\r\n",
                            r#"{"id":"u1","email":"x"}"#.to_string(),
                        )
                    } else {
                        ("422 Unprocessable Entity", "", "Failed to deserialize the JSON body into the target type: missing field `nama`".to_string())
                    }
                } else if line.starts_with("GET /api/auth/me") {
                    if cookie_ok {
                        ("200 OK", "", r#"{"id":"u1"}"#.to_string())
                    } else {
                        (
                            "401 Unauthorized",
                            "",
                            r#"{"error":"unauthorized"}"#.to_string(),
                        )
                    }
                } else if line.starts_with("DELETE /api/users/u1") {
                    ("204 No Content", "", String::new())
                } else {
                    ("404 Not Found", "", "{}".to_string())
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (port, log)
    }

    fn api_step(id: &str, phase: StepPhase, method: HttpMethod, path: &str) -> HttpTestStep {
        HttpTestStep {
            id: id.into(),
            name: id.into(),
            phase,
            request: SavedRequest {
                method,
                url: format!("{{{{base}}}}{path}"),
                ..Default::default()
            },
            assertions: vec![Assertion {
                target: "status".into(),
                op: crate::http_tests::AssertOp::LessThan,
                value: "300".into(),
            }],
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn validation_loop_fixes_steps_against_a_real_server() {
        let (port, log) = spawn_api();
        let mut register = api_step(
            "s1",
            StepPhase::Setup,
            HttpMethod::POST,
            "/api/auth/register",
        );
        register.request.body_type = crate::models::structs::HttpBodyType::Json;
        register.request.body_text =
            r#"{"name":"Budi {{run_id}}","password":"{{test_password}}"}"#.into();
        register.request.headers = vec![("Content-Type".into(), "application/json".into(), true)];
        register.extract = vec![
            crate::http_tests::Extract {
                var: "user_id".into(),
                from: "json:$.user.id".into(),
            },
            crate::http_tests::Extract {
                var: "token".into(),
                from: "json:$.token".into(),
            },
        ];
        let mut me = api_step("s2", StepPhase::Test, HttpMethod::GET, "/api/auth/me");
        me.request.auth_type = crate::models::structs::HttpAuthType::BearerToken;
        me.request.bearer_token = "{{token}}".into();
        let mut cleanup = api_step(
            "c1",
            StepPhase::Cleanup,
            HttpMethod::DELETE,
            "/api/users/{{user_id}}",
        );
        cleanup.cleanup_for = Some("s1".into());
        let mut suite = HttpTestSuite {
            variables: vec![
                ("base".into(), format!("http://127.0.0.1:{port}")),
                ("test_password".into(), String::new()),
            ],
            steps: vec![register, me, cleanup],
            ..Default::default()
        };
        suite.ensure_ids();

        let mut prompts: Vec<String> = Vec::new();
        let mut ask = |_system: String, user: String| -> Result<String, String> {
            prompts.push(user.clone());
            if user.contains("missing field `nama`") {
                Ok(r#"{"steps":[{"id":"s1",
                    "body":{"nama":"Budi {{run_id}}","email":"b{{run_id}}@example.com","password":"{{test_password}}"},
                    "extract":{"user_id":"json:$.id","token":"cookie:dp-token"}}]}"#.into())
            } else if user.contains("HTTP 401") {
                Ok(r#"{"steps":[{"id":"s2","auth":"none","headers":{"Cookie":"dp-token={{token}}"}}]}"#.into())
            } else {
                Ok(r#"{"steps":[]}"#.into())
            }
        };
        let (tx, _rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let outcome = run_validation(
            ValidationPlan {
                suite,
                env_vars: HashMap::new(),
                environment: Some("Development".into()),
                catalogs: Vec::new(),
                code: HashMap::new(),
                max_rounds: 3,
                ai_label: "test".into(),
            },
            &mut ask,
            &tx,
            &cancel,
        )
        .expect("validation");

        // Putaran 1: register 422 → AI memperbaiki body dan extract cookie.
        // Putaran 2: /me memakai Bearer {{token}} dan otomatis ikut mengirim
        // cookie dp-token, jadi lolos tanpa perbaikan kedua.
        assert_eq!(outcome.rounds.len(), 2, "{:?}", outcome.rounds);
        assert_eq!(outcome.fixed, 1);
        assert!(outcome.remaining.is_empty(), "{:?}", outcome.remaining);
        assert_eq!(outcome.filled_vars, vec!["test_password"]);
        let v = outcome
            .suite
            .validation_current()
            .expect("validation recorded");
        assert!(v.all_passed());
        assert_eq!(v.environment.as_deref(), Some("Development"));
        assert!(outcome.run.residue.is_empty());
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("missing field `nama`"));
        // Data uji dibersihkan pada putaran yang membuatnya.
        let deletes = log
            .lock()
            .expect("log")
            .iter()
            .filter(|l| l.starts_with("DELETE /api/users/u1"))
            .count();
        assert_eq!(deletes, 1);
    }

    #[test]
    fn validation_stops_early_when_server_is_down() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .expect("port");
        let mut s = suite();
        s.variables
            .push(("base".into(), format!("http://127.0.0.1:{port}")));
        let mut asked = false;
        let mut ask = |_: String, _: String| -> Result<String, String> {
            asked = true;
            Ok("{}".into())
        };
        let (tx, _rx) = mpsc::channel();
        let err = run_validation(
            ValidationPlan {
                suite: s,
                env_vars: HashMap::new(),
                environment: None,
                catalogs: Vec::new(),
                code: HashMap::new(),
                max_rounds: 3,
                ai_label: "test".into(),
            },
            &mut ask,
            &tx,
            &AtomicBool::new(false),
        )
        .expect_err("server down");
        assert!(err.contains("not reachable"), "{err}");
        assert!(!asked);
    }
}
