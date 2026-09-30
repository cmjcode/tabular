//! Integration test HTTP API yang di-generate AI dari satu atau beberapa
//! folder HTTP API (bisa dari repository berbeda) lalu dijalankan di Tabular.
//!
//! Modul ini headless: model suite, penyimpanan `{app_data}/http_tests/`,
//! substitusi variabel `{{nama}}`, evaluasi assertion, prompt + parser AI,
//! dan runner di thread background. UI ada di [`crate::http_repo`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http_collection::{HttpFolder, SavedRequest};
use crate::http_send::RequestSpec;
use crate::models::structs::{HttpAuthType, HttpBodyType, HttpClientResponse, HttpMethod};

// ─── Model ──────────────────────────────────────────────────────────────────

/// Operator assertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AssertOp {
    #[default]
    Equals,
    NotEquals,
    Contains,
    NotContains,
    Exists,
    NotExists,
    LessThan,
    GreaterThan,
    Matches,
}

impl AssertOp {
    pub fn label(self) -> &'static str {
        match self {
            AssertOp::Equals => "equals",
            AssertOp::NotEquals => "not equals",
            AssertOp::Contains => "contains",
            AssertOp::NotContains => "not contains",
            AssertOp::Exists => "exists",
            AssertOp::NotExists => "not exists",
            AssertOp::LessThan => "<",
            AssertOp::GreaterThan => ">",
            AssertOp::Matches => "matches",
        }
    }

    fn parse(s: &str) -> Self {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "not_equals" | "ne" | "!=" => AssertOp::NotEquals,
            "contains" | "includes" => AssertOp::Contains,
            "not_contains" => AssertOp::NotContains,
            "exists" | "present" | "not_null" => AssertOp::Exists,
            "not_exists" | "absent" | "is_null" => AssertOp::NotExists,
            "less_than" | "lt" | "<" => AssertOp::LessThan,
            "greater_than" | "gt" | ">" => AssertOp::GreaterThan,
            "matches" | "regex" => AssertOp::Matches,
            _ => AssertOp::Equals,
        }
    }
}

/// Pemeriksaan terhadap respons. `target`:
/// `status`, `time_ms`, `body`, `header:<Nama>`, atau `json:<path>`
/// (`$.data.items[0].id`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Assertion {
    pub target: String,
    #[serde(default)]
    pub op: AssertOp,
    #[serde(default)]
    pub value: String,
}

impl Assertion {
    pub fn describe(&self) -> String {
        match self.op {
            AssertOp::Exists | AssertOp::NotExists => {
                format!("{} {}", self.target, self.op.label())
            }
            _ => format!("{} {} {}", self.target, self.op.label(), self.value),
        }
    }
}

/// Simpan nilai dari respons ke variabel untuk langkah berikutnya.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Extract {
    pub var: String,
    /// Sintaks sama dengan [`Assertion::target`].
    pub from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpTestStep {
    pub name: String,
    /// Request dengan variabel `{{nama}}` (URL, header, body, auth).
    pub request: SavedRequest,
    #[serde(default)]
    pub extract: Vec<Extract>,
    #[serde(default)]
    pub assertions: Vec<Assertion>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Id request asal di collection, bila ada.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpTestSuite {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Folder HTTP API sumber (id).
    #[serde(default)]
    pub source_folders: Vec<String>,
    /// Variabel awal: `(nama, nilai)`, mis. `base_url_shop`.
    #[serde(default)]
    pub variables: Vec<(String, String)>,
    #[serde(default)]
    pub steps: Vec<HttpTestStep>,
    /// RFC 3339.
    #[serde(default)]
    pub created_at: String,
}

impl HttpTestSuite {
    /// Jumlah langkah aktif yang mengubah data (bukan GET/HEAD/OPTIONS).
    pub fn mutating_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.enabled)
            .filter(|s| {
                !matches!(
                    s.request.method,
                    HttpMethod::GET | HttpMethod::HEAD | HttpMethod::OPTIONS
                )
            })
            .count()
    }
}

// ─── Penyimpanan ────────────────────────────────────────────────────────────

fn suites_dir() -> std::path::PathBuf {
    crate::directory::get_app_data_dir().join("http_tests")
}

fn safe_file_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn save_suite(suite: &HttpTestSuite) -> Result<(), String> {
    save_suite_in(&suites_dir(), suite)
}

pub fn save_suite_in(dir: &std::path::Path, suite: &HttpTestSuite) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", safe_file_id(&suite.id)));
    let json = serde_json::to_string_pretty(suite).map_err(|e| e.to_string())?;
    crate::directory::write_file_atomically(&path, json.as_bytes()).map_err(|e| {
        log::error!("[HTTP_TESTS] cannot save {}: {e}", path.display());
        format!("Could not save test suite '{}': {e}", suite.name)
    })
}

pub fn load_suites() -> Vec<HttpTestSuite> {
    load_suites_in(&suites_dir())
}

pub fn load_suites_in(dir: &std::path::Path) -> Vec<HttpTestSuite> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| serde_json::from_slice::<HttpTestSuite>(&b).map_err(|e| e.to_string()))
        {
            Ok(s) => out.push(s),
            Err(e) => log::warn!("[HTTP_TESTS] skip unreadable {}: {e}", path.display()),
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.name.cmp(&b.name)));
    out
}

/// Hapus file suite dari disk (file yang sudah tidak ada dianggap berhasil).
pub fn remove_suite(id: &str) -> Result<(), String> {
    let path = suites_dir().join(format!("{}.json", safe_file_id(id)));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not remove test suite: {e}")),
    }
}

// ─── Variabel & evaluasi ────────────────────────────────────────────────────

/// Ganti `{{nama}}` dengan nilai variabel; yang tidak dikenal dibiarkan.
pub fn substitute(text: &str, vars: &HashMap<String, String>) -> String {
    if !text.contains("{{") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                match vars.get(name) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Nama variabel `{{x}}` yang belum punya nilai.
pub fn unresolved_vars(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        let name = after[..end].trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
        rest = &after[end + 2..];
    }
    out
}

/// Request dengan semua variabel diganti.
pub fn resolve_request(req: &SavedRequest, vars: &HashMap<String, String>) -> SavedRequest {
    let sub = |s: &str| substitute(s, vars);
    let rows = |r: &[(String, String, bool)]| -> Vec<(String, String, bool)> {
        r.iter().map(|(k, v, e)| (sub(k), sub(v), *e)).collect()
    };
    SavedRequest {
        url: sub(&req.url),
        params: rows(&req.params),
        headers: rows(&req.headers),
        body_text: sub(&req.body_text),
        form_data: rows(&req.form_data),
        bearer_token: sub(&req.bearer_token),
        basic_user: sub(&req.basic_user),
        basic_pass: sub(&req.basic_pass),
        api_key_name: sub(&req.api_key_name),
        api_key_value: sub(&req.api_key_value),
        ..req.clone()
    }
}

/// Ambil nilai dari JSON dengan path `$.a.b[0].c`, `a.b.0.c`, atau `a[0]`.
pub fn json_lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let path = path.trim();
    let path = path.strip_prefix('$').unwrap_or(path);
    let mut cur = value;
    for raw in path.split('.') {
        if raw.is_empty() {
            continue;
        }
        // `items[0][1]` → "items", 0, 1
        let (key, mut idx_part) = match raw.find('[') {
            Some(i) => (&raw[..i], &raw[i..]),
            None => (raw, ""),
        };
        if !key.is_empty() {
            cur = match (cur, key.parse::<usize>()) {
                (Value::Array(a), Ok(i)) => a.get(i)?,
                _ => cur.get(key)?,
            };
        }
        while let Some(rest) = idx_part.strip_prefix('[') {
            let end = rest.find(']')?;
            let inner = rest[..end].trim().trim_matches(|c| c == '\'' || c == '"');
            cur = match inner.parse::<usize>() {
                Ok(i) => cur.get(i)?,
                Err(_) => cur.get(inner)?,
            };
            idx_part = &rest[end + 1..];
        }
    }
    Some(cur)
}

/// Nilai target dari respons. `None` = tidak ada (header/JSON tidak ditemukan).
pub fn read_target(resp: &HttpClientResponse, target: &str) -> Option<String> {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "status" {
        return Some(resp.status.to_string());
    }
    if lower == "time_ms" || lower == "time" {
        return Some(resp.time_ms.to_string());
    }
    if lower == "body" {
        return Some(resp.body.clone());
    }
    if let Some(name) = t
        .strip_prefix("header:")
        .or_else(|| t.strip_prefix("headers."))
    {
        return resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name.trim()))
            .map(|(_, v)| v.clone());
    }
    let path = t
        .strip_prefix("json:")
        .or_else(|| t.strip_prefix("body."))
        .or_else(|| t.starts_with('$').then_some(t))?;
    let json: Value = serde_json::from_str(&resp.body).ok()?;
    match json_lookup(&json, path)? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Hasil satu assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertResult {
    pub assertion: Assertion,
    pub passed: bool,
    pub actual: Option<String>,
}

fn as_number(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

pub fn evaluate(
    resp: &HttpClientResponse,
    a: &Assertion,
    vars: &HashMap<String, String>,
) -> AssertResult {
    let actual = read_target(resp, &a.target);
    let expected = substitute(&a.value, vars);
    let passed = match (a.op, actual.as_deref()) {
        (AssertOp::Exists, v) => v.is_some(),
        (AssertOp::NotExists, v) => v.is_none(),
        (_, None) => false,
        (AssertOp::Equals, Some(v)) => match (as_number(v), as_number(&expected)) {
            (Some(x), Some(y)) => (x - y).abs() < f64::EPSILON,
            _ => v.trim() == expected.trim(),
        },
        (AssertOp::NotEquals, Some(v)) => match (as_number(v), as_number(&expected)) {
            (Some(x), Some(y)) => (x - y).abs() >= f64::EPSILON,
            _ => v.trim() != expected.trim(),
        },
        (AssertOp::Contains, Some(v)) => v.contains(&expected),
        (AssertOp::NotContains, Some(v)) => !v.contains(&expected),
        (AssertOp::LessThan, Some(v)) => {
            matches!((as_number(v), as_number(&expected)), (Some(x), Some(y)) if x < y)
        }
        (AssertOp::GreaterThan, Some(v)) => {
            matches!((as_number(v), as_number(&expected)), (Some(x), Some(y)) if x > y)
        }
        (AssertOp::Matches, Some(v)) => regex::Regex::new(&expected).is_ok_and(|r| r.is_match(v)),
    };
    AssertResult {
        assertion: a.clone(),
        passed,
        actual,
    }
}

// ─── Runner ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct StepResult {
    pub index: usize,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub time_ms: u128,
    pub error: Option<String>,
    pub assertions: Vec<AssertResult>,
    pub extracted: Vec<(String, String)>,
    /// Variabel yang masih `{{…}}` saat request dikirim.
    pub unresolved: Vec<String>,
    /// Potongan awal body respons.
    pub body_preview: String,
    pub skipped: bool,
}

impl StepResult {
    pub fn passed(&self) -> bool {
        !self.skipped && self.error.is_none() && self.assertions.iter().all(|a| a.passed)
    }
}

#[derive(Debug)]
pub enum TestEvent {
    StepStarted(usize),
    StepFinished(Box<StepResult>),
    Finished { cancelled: bool },
}

pub struct TestRunHandle {
    pub rx: mpsc::Receiver<TestEvent>,
    cancel: Arc<AtomicBool>,
}

impl TestRunHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Jalankan satu langkah dengan pengirim `send` (bisa diganti di test).
pub fn run_step(
    index: usize,
    step: &HttpTestStep,
    vars: &mut HashMap<String, String>,
    send: &dyn Fn(RequestSpec) -> HttpClientResponse,
) -> StepResult {
    let req = resolve_request(&step.request, vars);
    let mut unresolved = unresolved_vars(&req.url);
    for text in [&req.body_text, &req.bearer_token, &req.api_key_value] {
        for v in unresolved_vars(text) {
            if !unresolved.contains(&v) {
                unresolved.push(v);
            }
        }
    }
    let spec = RequestSpec::from_saved(&req);
    let url = spec.full_url();
    let resp = send(spec);
    let mut result = StepResult {
        index,
        name: step.name.clone(),
        method: req.method.label().to_string(),
        url: crate::http_ai::redact_url(&url),
        status: resp.status,
        time_ms: resp.time_ms,
        error: resp.error.clone(),
        unresolved,
        body_preview: crate::repo_scan::truncate_chars(&resp.body, 2_000),
        ..Default::default()
    };
    if resp.error.is_some() {
        return result;
    }
    for ex in &step.extract {
        if let Some(v) = read_target(&resp, &ex.from) {
            vars.insert(ex.var.clone(), v.clone());
            result.extracted.push((ex.var.clone(), v));
        }
    }
    result.assertions = step
        .assertions
        .iter()
        .map(|a| evaluate(&resp, a, vars))
        .collect();
    result
}

/// Variabel awal satu run: variabel suite + `run_id` unik.
pub fn initial_vars(suite: &HttpTestSuite) -> HashMap<String, String> {
    let mut vars: HashMap<String, String> = suite
        .variables
        .iter()
        .map(|(k, v)| (k.trim().to_string(), v.clone()))
        .collect();
    vars.entry("run_id".to_string())
        .or_insert_with(|| chrono::Utc::now().timestamp_millis().to_string());
    vars
}

/// Jalankan suite berurutan di thread background. Langkah yang dinonaktifkan
/// dilewati; langkah yang gagal tidak menghentikan suite (variabel hasil
/// ekstraksinya saja yang tidak terisi).
pub fn spawn_run(suite: HttpTestSuite) -> TestRunHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let mut vars = initial_vars(&suite);
        let send = crate::http_send::send_blocking;
        for (i, step) in suite.steps.iter().enumerate() {
            if flag.load(Ordering::SeqCst) {
                let _ = tx.send(TestEvent::Finished { cancelled: true });
                return;
            }
            if !step.enabled {
                let _ = tx.send(TestEvent::StepFinished(Box::new(StepResult {
                    index: i,
                    name: step.name.clone(),
                    method: step.request.method.label().to_string(),
                    skipped: true,
                    ..Default::default()
                })));
                continue;
            }
            let _ = tx.send(TestEvent::StepStarted(i));
            let result = run_step(i, step, &mut vars, &send);
            log::info!(
                "[HTTP_TESTS] {} step {} '{}' -> {} ({})",
                suite.name,
                i + 1,
                step.name,
                result.status,
                if result.passed() { "pass" } else { "fail" }
            );
            let _ = tx.send(TestEvent::StepFinished(Box::new(result)));
        }
        let _ = tx.send(TestEvent::Finished { cancelled: false });
    });
    TestRunHandle { rx, cancel }
}

// ─── Generate dengan AI ─────────────────────────────────────────────────────

/// Folder sumber untuk AI: nama, variabel base URL, dan request-nya.
#[derive(Debug, Clone)]
pub struct FolderCatalog {
    pub folder_id: String,
    pub folder_name: String,
    /// Nama variabel base URL, mis. `base_url_shop_api`.
    pub base_var: String,
    pub base_url: String,
    pub requests: Vec<SavedRequest>,
}

/// Nama variabel aman dari nama folder.
pub fn base_var_name(folder_name: &str) -> String {
    let slug: String = folder_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if slug.is_empty() {
        "base_url".to_string()
    } else {
        format!("base_url_{slug}")
    }
}

/// Origin (`scheme://host:port`) yang paling sering dipakai request folder.
pub fn common_origin(requests: &[&SavedRequest]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for r in requests {
        let url = r.url.trim();
        let Some((scheme, rest)) = url.split_once("://") else {
            continue;
        };
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() {
            continue;
        }
        let origin = format!("{scheme}://{host}");
        match counts.iter_mut().find(|(o, _)| *o == origin) {
            Some((_, c)) => *c += 1,
            None => counts.push((origin, 1)),
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(o, _)| o)
        .unwrap_or_else(|| "http://localhost:3000".to_string())
}

impl FolderCatalog {
    pub fn from_folder(folder: &HttpFolder) -> Self {
        let reqs = folder.all_requests();
        Self {
            folder_id: folder.id.clone(),
            folder_name: folder.name.clone(),
            base_var: base_var_name(&folder.name),
            base_url: common_origin(&reqs),
            requests: reqs.into_iter().cloned().collect(),
        }
    }
}

/// Batas byte katalog endpoint di prompt.
const CATALOG_BYTES: usize = 120_000;

/// Katalog request untuk prompt: tanpa nilai auth, body diringkas dan
/// literal rahasia disamarkan.
pub fn catalog_text(catalogs: &[FolderCatalog]) -> String {
    let mut out = String::new();
    for c in catalogs {
        out.push_str(&format!(
            "\n## Folder \"{}\" (base URL variable `{{{{{}}}}}` = {})\n",
            c.folder_name, c.base_var, c.base_url
        ));
        for r in &c.requests {
            if out.len() > CATALOG_BYTES {
                out.push_str("… (catalog truncated)\n");
                return out;
            }
            let path = r
                .route
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| crate::http_collection::extract_endpoint_url(&r.url));
            out.push_str(&format!(
                "- id={} {} {} — {}\n",
                r.id,
                r.method.label(),
                path,
                r.display_name()
            ));
            if !matches!(
                r.auth_type,
                HttpAuthType::NoAuth | HttpAuthType::InheritParent
            ) {
                out.push_str(&format!("  auth: {:?}\n", r.auth_type));
            }
            let q: Vec<&str> = r.params.iter().map(|(k, _, _)| k.as_str()).collect();
            if !q.is_empty() {
                out.push_str(&format!("  query: {}\n", q.join(", ")));
            }
            if !r.tables.is_empty() {
                out.push_str(&format!("  tables: {}\n", r.tables.join(", ")));
            }
            if !r.body_text.trim().is_empty() {
                let body = crate::repo_endpoints::redact_code_secrets(&r.body_text);
                out.push_str(&format!(
                    "  body example: {}\n",
                    crate::repo_scan::truncate_chars(&body.replace('\n', " "), 600)
                ));
            }
        }
    }
    out
}

pub fn build_test_prompts(catalogs: &[FolderCatalog], instructions: &str) -> (String, String) {
    let system = String::from(
        "You design end-to-end integration tests for HTTP APIs. The APIs may come from several \
         services; design flows that cross them when the data connects (ids created by one \
         service used by another, shared tables).\n\
         Rules:\n\
         - Build realistic scenarios: create data, read it back, update, list/search, negative \
           cases (validation errors, not found, unauthorized) and clean up what the suite created.\n\
         - Use `{{base_url_…}}` variables from the catalog for every URL; never hardcode hosts.\n\
         - Use `extract` to pass values (ids, tokens) between steps as `{{variable}}`.\n\
         - Auth tokens are variables such as `{{token}}`; add them to `variables` with an empty \
           value unless a login step extracts them.\n\
         - Assertion `target` is `status`, `time_ms`, `body`, `header:<Name>` or \
           `json:$.path.to[0].field`. `op` is equals, not_equals, contains, not_contains, \
           exists, not_exists, less_than, greater_than or matches (regex).\n\
         - `request_id` refers to the catalog id the step is based on, when there is one.\n\
         - Never include real secrets.\n\
         Reply with ONLY one JSON object, no prose and no markdown fence:\n\
         {\"suites\":[{\"name\":\"Checkout flow\",\"description\":\"…\",\"variables\":\
         {\"token\":\"\"},\"steps\":[{\"name\":\"Create user\",\"request_id\":\"sr_1\",\
         \"method\":\"POST\",\"url\":\"{{base_url_shop}}/users\",\"headers\":\
         {\"Content-Type\":\"application/json\"},\"query\":{},\"auth\":\"bearer\",\
         \"body\":{\"email\":\"test+{{run_id}}@example.com\"},\"extract\":{\"user_id\":\
         \"json:$.id\"},\"assert\":[{\"target\":\"status\",\"op\":\"equals\",\"value\":\"201\"},\
         {\"target\":\"json:$.id\",\"op\":\"exists\"}]}]}]}\n\
         The variable `{{run_id}}` is always available and unique per run.\n",
    );
    let mut user = String::from("Endpoint catalog:\n");
    user.push_str(&catalog_text(catalogs));
    if !instructions.trim().is_empty() {
        user.push_str("\nFocus: ");
        user.push_str(instructions.trim());
        user.push('\n');
    }
    (system, user)
}

fn value_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn obj_pairs(v: Option<&Value>) -> Vec<(String, String)> {
    match v {
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| (k.clone(), value_string(v)))
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| {
                let k = i
                    .get("name")
                    .or_else(|| i.get("key"))?
                    .as_str()?
                    .to_string();
                let v = i.get("value").map(value_string).unwrap_or_default();
                Some((k, v))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn parse_method(s: &str) -> HttpMethod {
    match s.trim().to_ascii_uppercase().as_str() {
        "POST" => HttpMethod::POST,
        "PUT" => HttpMethod::PUT,
        "DELETE" => HttpMethod::DELETE,
        "PATCH" => HttpMethod::PATCH,
        "HEAD" => HttpMethod::HEAD,
        "OPTIONS" => HttpMethod::OPTIONS,
        _ => HttpMethod::GET,
    }
}

fn parse_step(v: &Value, catalogs: &[FolderCatalog]) -> Option<HttpTestStep> {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let url = s("url");
    if url.is_empty() {
        return None;
    }
    let request_id = Some(s("request_id")).filter(|x| !x.is_empty());
    let base = request_id
        .as_deref()
        .and_then(|id| {
            catalogs
                .iter()
                .flat_map(|c| c.requests.iter())
                .find(|r| r.id == id)
        })
        .cloned();
    let mut req = SavedRequest {
        name: s("name"),
        url,
        method: parse_method(&s("method")),
        ..Default::default()
    };
    // Jenis auth dari request asal (tanpa nilai rahasianya).
    if let Some(b) = &base {
        req.auth_type = b.auth_type.clone();
        req.api_key_name = b.api_key_name.clone();
        req.api_key_in_header = b.api_key_in_header;
        req.tables = b.tables.clone();
        req.route = b.route.clone();
    }
    match s("auth").to_ascii_lowercase().as_str() {
        "bearer" => req.auth_type = HttpAuthType::BearerToken,
        "basic" => req.auth_type = HttpAuthType::BasicAuth,
        "api_key" => req.auth_type = HttpAuthType::ApiKey,
        "none" => req.auth_type = HttpAuthType::NoAuth,
        _ => {}
    }
    match req.auth_type {
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            req.bearer_token = "{{token}}".into();
        }
        HttpAuthType::BasicAuth => {
            req.basic_user = "{{username}}".into();
            req.basic_pass = "{{password}}".into();
        }
        HttpAuthType::ApiKey => {
            if req.api_key_name.is_empty() {
                req.api_key_name = "X-API-Key".into();
                req.api_key_in_header = true;
            }
            req.api_key_value = "{{api_key}}".into();
        }
        _ => {}
    }
    req.headers = obj_pairs(v.get("headers"))
        .into_iter()
        .map(|(k, v)| (k, v, true))
        .collect();
    req.params = obj_pairs(v.get("query").or_else(|| v.get("params")))
        .into_iter()
        .map(|(k, v)| (k, v, true))
        .collect();
    match v.get("body") {
        None | Some(Value::Null) => req.body_type = HttpBodyType::NoBody,
        Some(Value::String(text)) if text.trim().is_empty() => req.body_type = HttpBodyType::NoBody,
        Some(Value::String(text)) => {
            req.body_type = if serde_json::from_str::<Value>(text).is_ok() {
                HttpBodyType::Json
            } else {
                HttpBodyType::OtherText
            };
            req.body_text = text.clone();
        }
        Some(other) => {
            req.body_type = HttpBodyType::Json;
            req.body_text = serde_json::to_string_pretty(other).unwrap_or_default();
        }
    }
    req.form_data = vec![(String::new(), String::new(), true)];
    let extract = obj_pairs(v.get("extract"))
        .into_iter()
        .map(|(var, from)| Extract { var, from })
        .collect();
    let assertions = v
        .get("assert")
        .or_else(|| v.get("assertions"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|a| {
                    let target = a.get("target")?.as_str()?.trim().to_string();
                    Some(Assertion {
                        target,
                        op: AssertOp::parse(a.get("op").and_then(Value::as_str).unwrap_or("")),
                        value: a.get("value").map(value_string).unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(HttpTestStep {
        name: if req.name.is_empty() {
            format!("{} {}", req.method.label(), req.url)
        } else {
            req.name.clone()
        },
        request: req,
        extract,
        assertions,
        enabled: true,
        request_id,
    })
}

/// Urai jawaban AI menjadi suite. Variabel base URL tiap folder ditambahkan
/// otomatis bila belum ada.
pub fn parse_tests_reply(
    text: &str,
    catalogs: &[FolderCatalog],
) -> Result<Vec<HttpTestSuite>, String> {
    let v: Value = crate::repo_scan::slice_between(text, '{', '}')
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(|| {
            format!(
                "AI reply is not JSON: {}",
                crate::repo_scan::truncate_chars(text.trim(), 200)
            )
        })?;
    let suites = v
        .get("suites")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| v.get("steps").map(|_| vec![v.clone()]))
        .ok_or_else(|| "AI reply has no \"suites\"".to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = Vec::new();
    for sv in suites {
        let steps: Vec<HttpTestStep> = sv
            .get("steps")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| parse_step(s, catalogs)).collect())
            .unwrap_or_default();
        if steps.is_empty() {
            continue;
        }
        let mut variables = obj_pairs(sv.get("variables"));
        for c in catalogs {
            if !variables.iter().any(|(k, _)| *k == c.base_var) {
                variables.insert(0, (c.base_var.clone(), c.base_url.clone()));
            }
        }
        // Nilai rahasia dari AI tidak dipercaya: user mengisinya sendiri.
        for (k, v) in variables.iter_mut() {
            let lower = k.to_ascii_lowercase();
            if ["token", "password", "secret", "api_key"]
                .iter()
                .any(|s| lower.contains(s))
            {
                v.clear();
            }
        }
        variables.retain(|(k, _)| k != "run_id");
        out.push(HttpTestSuite {
            id: crate::http_collection::unique_id("suite"),
            name: sv
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("Integration test")
                .to_string(),
            description: sv
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            source_folders: catalogs.iter().map(|c| c.folder_id.clone()).collect(),
            variables,
            steps,
            created_at: now.clone(),
        });
    }
    if out.is_empty() {
        return Err("The AI returned no test steps".into());
    }
    Ok(out)
}

pub type TestGenEvent = crate::repo_scan::RepoJobEvent<Vec<HttpTestSuite>>;

pub struct TestGenHandle {
    pub rx: mpsc::Receiver<TestGenEvent>,
    cancel: Arc<AtomicBool>,
}

impl TestGenHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Generate suite di background dengan backend AI yang dipilih.
pub fn spawn_generate(
    backend: crate::ai_assistant::ChatBackend,
    backend_label: String,
    catalogs: Vec<FolderCatalog>,
    instructions: String,
) -> TestGenHandle {
    use crate::agent::harness::ProgressStatus;
    use crate::repo_scan::{RepoJobEvent, step};
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let label = format!("Asking {backend_label} to design integration tests");
        let total: usize = catalogs.iter().map(|c| c.requests.len()).sum();
        let _ = tx.send(step(
            1,
            label.clone(),
            Some(format!(
                "{} endpoint(s) from {} folder(s)",
                total,
                catalogs.len()
            )),
            ProgressStatus::Active,
        ));
        let (system, user) = build_test_prompts(&catalogs, &instructions);
        let result = crate::repo_scan::ask_ai(
            &backend,
            system,
            user,
            crate::ai_assistant::ChatWorkspace::default(),
            &tx,
            &flag,
        )
        .map_err(|e| e.to_string())
        .and_then(|text| parse_tests_reply(&text, &catalogs));
        let status = if result.is_ok() {
            ProgressStatus::Done
        } else {
            ProgressStatus::Error
        };
        let detail = match &result {
            Ok(s) => format!(
                "{} suite(s), {} step(s)",
                s.len(),
                s.iter().map(|x| x.steps.len()).sum::<usize>()
            ),
            Err(e) => e.clone(),
        };
        let _ = tx.send(step(1, label, Some(detail), status));
        if let Err(e) = &result {
            log::warn!("[HTTP_TESTS] generation failed: {e}");
        }
        let _ = tx.send(RepoJobEvent::Finished(result));
    });
    TestGenHandle { rx, cancel }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, body: &str) -> HttpClientResponse {
        HttpClientResponse {
            status,
            status_text: String::new(),
            body: body.to_string(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            time_ms: 42,
            size_bytes: body.len(),
            error: None,
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn substitutes_known_variables_only() {
        let v = vars(&[("base", "http://h"), ("id", "7")]);
        assert_eq!(substitute("{{base}}/u/{{ id }}", &v), "http://h/u/7");
        assert_eq!(substitute("{{missing}}/x", &v), "{{missing}}/x");
        assert_eq!(substitute("a {{ unclosed", &v), "a {{ unclosed");
        assert_eq!(unresolved_vars("{{a}} {{b}} {{a}}"), vec!["a", "b"]);
    }

    #[test]
    fn looks_up_json_paths() {
        let v: Value = serde_json::json!({"data": {"items": [{"id": 5}, {"id": 6}]}, "ok": true});
        assert_eq!(
            json_lookup(&v, "$.data.items[1].id"),
            Some(&serde_json::json!(6))
        );
        assert_eq!(
            json_lookup(&v, "data.items.0.id"),
            Some(&serde_json::json!(5))
        );
        assert_eq!(json_lookup(&v, "$.ok"), Some(&serde_json::json!(true)));
        assert_eq!(json_lookup(&v, "$.nope"), None);
    }

    #[test]
    fn evaluates_assertions() {
        let r = resp(201, r#"{"id": 9, "email": "a@b.c", "tags": []}"#);
        let v = vars(&[("expected", "9")]);
        let check = |target: &str, op: AssertOp, value: &str| {
            evaluate(
                &r,
                &Assertion {
                    target: target.into(),
                    op,
                    value: value.into(),
                },
                &v,
            )
            .passed
        };
        assert!(check("status", AssertOp::Equals, "201"));
        assert!(!check("status", AssertOp::Equals, "200"));
        assert!(check("json:$.id", AssertOp::Equals, "{{expected}}"));
        assert!(check("json:$.email", AssertOp::Contains, "@"));
        assert!(check("json:$.missing", AssertOp::NotExists, ""));
        assert!(check("header:content-type", AssertOp::Contains, "json"));
        assert!(check("time_ms", AssertOp::LessThan, "1000"));
        assert!(check("json:$.email", AssertOp::Matches, r"^\w+@"));
        assert!(!check("json:$.missing", AssertOp::Equals, ""));
    }

    #[test]
    fn run_step_extracts_and_substitutes() {
        let step = HttpTestStep {
            name: "get".into(),
            request: SavedRequest {
                url: "{{base}}/users/{{id}}".into(),
                bearer_token: "{{token}}".into(),
                ..Default::default()
            },
            extract: vec![Extract {
                var: "email".into(),
                from: "json:$.email".into(),
            }],
            assertions: vec![Assertion {
                target: "status".into(),
                op: AssertOp::Equals,
                value: "200".into(),
            }],
            enabled: true,
            request_id: None,
        };
        let mut v = vars(&[("base", "http://h"), ("id", "3")]);
        let seen = std::cell::RefCell::new(String::new());
        let send = |spec: RequestSpec| {
            *seen.borrow_mut() = spec.url.clone();
            resp(200, r#"{"email":"x@y.z"}"#)
        };
        let r = run_step(0, &step, &mut v, &send);
        assert_eq!(*seen.borrow(), "http://h/users/3");
        assert!(r.passed());
        assert_eq!(v.get("email").map(String::as_str), Some("x@y.z"));
        assert_eq!(r.unresolved, vec!["token"]);
    }

    #[test]
    fn parses_ai_suites_and_fills_base_vars() {
        let cat = FolderCatalog {
            folder_id: "f1".into(),
            folder_name: "Shop API".into(),
            base_var: base_var_name("Shop API"),
            base_url: "http://localhost:3000".into(),
            requests: vec![SavedRequest {
                id: "sr_1".into(),
                method: HttpMethod::POST,
                url: "http://localhost:3000/users".into(),
                api_key_name: "X-Key".into(),
                auth_type: HttpAuthType::ApiKey,
                tables: vec!["users".into()],
                ..Default::default()
            }],
        };
        assert_eq!(cat.base_var, "base_url_shop_api");
        let reply = r#"```json
{"suites":[{"name":"Users","variables":{"token":"real-secret","run_id":"x"},"steps":[
 {"name":"Create","request_id":"sr_1","method":"post","url":"{{base_url_shop_api}}/users",
  "body":{"email":"a@b.c"},"extract":{"uid":"json:$.id"},
  "assert":[{"target":"status","op":"equals","value":201},{"target":"json:$.id","op":"exists"}]},
 {"method":"GET"}
]},{"name":"Empty","steps":[]}]}
```"#;
        let suites = parse_tests_reply(reply, std::slice::from_ref(&cat)).expect("parse");
        assert_eq!(suites.len(), 1);
        let s = &suites[0];
        assert_eq!(s.steps.len(), 1);
        assert_eq!(
            s.variables[0],
            (
                "base_url_shop_api".to_string(),
                "http://localhost:3000".to_string()
            )
        );
        assert!(
            s.variables
                .iter()
                .any(|(k, v)| k == "token" && v.is_empty())
        );
        assert!(!s.variables.iter().any(|(k, _)| k == "run_id"));
        let step = &s.steps[0];
        assert_eq!(step.request.method, HttpMethod::POST);
        assert_eq!(step.request.body_type, HttpBodyType::Json);
        assert_eq!(step.request.api_key_name, "X-Key");
        assert_eq!(step.request.api_key_value, "{{api_key}}");
        assert_eq!(step.request.tables, vec!["users"]);
        assert_eq!(step.assertions[0].value, "201");
        assert_eq!(step.assertions[1].op, AssertOp::Exists);
        assert_eq!(step.extract[0].var, "uid");
        assert_eq!(s.mutating_steps(), 1);
        assert!(parse_tests_reply("nope", &[]).is_err());
    }

    #[test]
    fn saves_and_loads_suites() {
        let dir = std::env::temp_dir().join(format!(
            "tabular-http-tests-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        ));
        let suite = HttpTestSuite {
            id: "suite/1".into(),
            name: "S".into(),
            steps: vec![HttpTestStep {
                name: "a".into(),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        save_suite_in(&dir, &suite).expect("save");
        let loaded = load_suites_in(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "suite/1");
        assert_eq!(loaded[0].steps.len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn common_origin_picks_most_used_host() {
        let a = SavedRequest {
            url: "http://a:1/x".into(),
            ..Default::default()
        };
        let b = SavedRequest {
            url: "https://b/y?q".into(),
            ..Default::default()
        };
        assert_eq!(common_origin(&[&a, &b, &b]), "https://b");
        assert_eq!(common_origin(&[]), "http://localhost:3000");
    }

    #[test]
    fn initial_vars_adds_run_id() {
        let s = HttpTestSuite {
            variables: vec![(" base ".into(), "x".into())],
            ..Default::default()
        };
        let v = initial_vars(&s);
        assert_eq!(v.get("base").map(String::as_str), Some("x"));
        assert!(v.contains_key("run_id"));
    }
}
