//! Integration test HTTP API yang di-generate AI dari satu atau beberapa
//! folder HTTP API (bisa dari repository berbeda) lalu dijalankan di Tabular.
//!
//! Modul ini headless: model suite, penyimpanan `{app_data}/http_tests/`,
//! substitusi variabel `{{nama}}`, evaluasi assertion, analisis relasi antar
//! langkah, runner berurutan dengan fase cleanup yang selalu dijalankan,
//! riwayat run, serta prompt + parser AI. UI ada di
//! `window_egui::integration_test_ui`; laporan di [`crate::http_test_report`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http_collection::{HttpFolder, SavedRequest};
use crate::http_send::RequestSpec;
pub use crate::http_test_db::{DbExecutor, DbQueryStep, QueryOutput, StepKind};
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
    /// Angka sama dalam toleransi: nilai `"100"` (toleransi 0.001) atau
    /// `"100 +- 0.5"`. Untuk kolom decimal/float.
    Approx,
}

impl AssertOp {
    pub const ALL: [AssertOp; 10] = [
        AssertOp::Equals,
        AssertOp::NotEquals,
        AssertOp::Contains,
        AssertOp::NotContains,
        AssertOp::Exists,
        AssertOp::NotExists,
        AssertOp::LessThan,
        AssertOp::GreaterThan,
        AssertOp::Matches,
        AssertOp::Approx,
    ];

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
            AssertOp::Approx => "approx",
        }
    }

    pub fn parse(s: &str) -> Self {
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
            "approx" | "approximately" | "about" | "~" | "~=" | "close_to" => AssertOp::Approx,
            _ => AssertOp::Equals,
        }
    }
}

/// Pemeriksaan terhadap respons. `target`:
/// `status`, `time_ms`, `body`, `header:<Nama>`, atau `json:<path>`
/// (`$.data.items[0].id`). Langkah DB: `row_count`, `scalar`,
/// `cell:<baris>.<kolom>`, `column:<kolom>`. Langkah concurrent:
/// `count:<target>=<nilai>`, `any:<target>`, `all:<target>`.
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

/// Fase langkah. Urutan eksekusi: semua Setup, lalu Test, lalu Cleanup.
/// Cleanup tetap dijalankan walau langkah sebelumnya gagal atau run dibatalkan.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum StepPhase {
    Setup,
    #[default]
    Test,
    Cleanup,
}

impl StepPhase {
    pub const ALL: [StepPhase; 3] = [StepPhase::Setup, StepPhase::Test, StepPhase::Cleanup];

    pub fn label(self) -> &'static str {
        match self {
            StepPhase::Setup => "Setup",
            StepPhase::Test => "Test",
            StepPhase::Cleanup => "Cleanup",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "setup" | "arrange" | "before" | "precondition" => StepPhase::Setup,
            "cleanup" | "teardown" | "after" | "clean_up" | "clean-up" => StepPhase::Cleanup,
            _ => StepPhase::Test,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpTestStep {
    /// Id stabil langkah di dalam suite (rujukan `cleanup_for` dan tab HTTP).
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub phase: StepPhase,
    /// Group fungsional (mis. "Purchasing", "Plant"); kosong = tanpa group.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
    /// Untuk langkah Cleanup: id langkah yang datanya dihapus langkah ini.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_for: Option<String>,
    /// Catatan bebas (tujuan langkah, requirement yang diuji).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Jenis langkah; suite lama tanpa field ini = HTTP.
    #[serde(default, skip_serializing_if = "StepKind::is_http")]
    pub kind: StepKind,
    /// Request dengan variabel `{{nama}}` (URL, header, body, auth). Kosong
    /// untuk langkah DB.
    #[serde(default, skip_serializing_if = "request_is_blank")]
    pub request: SavedRequest,
    /// Query baca untuk langkah `kind = db_query`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<DbQueryStep>,
    /// Kirim request langkah ini beberapa kali serentak.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<Concurrency>,
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

fn default_one() -> u32 {
    1
}

/// Request tanpa isi (langkah DB): tidak ditulis ke file suite.
fn request_is_blank(r: &SavedRequest) -> bool {
    r.url.trim().is_empty()
        && r.body_text.is_empty()
        && r.headers.is_empty()
        && r.params.is_empty()
        && r.form_data.is_empty()
}

/// Batas burst serentak (bukan load test).
pub const MAX_CONCURRENCY: u32 = 50;
pub const MAX_ROUNDS: u32 = 20;

/// Kirim request langkah yang sama beberapa kali serentak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Concurrency {
    /// Jumlah request serentak, 2..=50.
    pub count: u32,
    /// Semua thread menunggu di barrier lalu mengirim bersamaan (default
    /// true). `false` = dikirim berurutan secepatnya.
    #[serde(default = "default_true")]
    pub barrier: bool,
    /// Ulangi seluruh burst sekian kali untuk menangkap race yang jarang.
    #[serde(default = "default_one")]
    pub rounds: u32,
}

impl Default for Concurrency {
    fn default() -> Self {
        Self {
            count: 5,
            barrier: true,
            rounds: 1,
        }
    }
}

impl Concurrency {
    pub fn count(&self) -> usize {
        self.count.clamp(2, MAX_CONCURRENCY) as usize
    }

    pub fn rounds(&self) -> usize {
        self.rounds.clamp(1, MAX_ROUNDS) as usize
    }
}

impl HttpTestStep {
    pub fn is_db(&self) -> bool {
        self.kind == StepKind::DbQuery
    }

    /// Langkah HTTP dengan burst serentak.
    pub fn concurrent(&self) -> Option<Concurrency> {
        self.concurrency.filter(|_| self.kind.is_http())
    }

    /// Label method untuk daftar dan laporan (`SQL` untuk langkah DB).
    pub fn method_label(&self) -> &'static str {
        if self.is_db() {
            "SQL"
        } else {
            self.request.method.label()
        }
    }

    /// Langkah HTTP yang (biasanya) mengubah data. Langkah DB selalu baca.
    pub fn mutates(&self) -> bool {
        self.kind.is_http() && is_mutating(&self.request.method)
    }
}

fn sanitize_step_id(id: &str) -> String {
    id.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(40)
        .collect()
}

/// Method yang (biasanya) mengubah data di server.
pub fn is_mutating(method: &HttpMethod) -> bool {
    !matches!(
        method,
        HttpMethod::GET | HttpMethod::HEAD | HttpMethod::OPTIONS
    )
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
    /// RFC 3339, perubahan terakhir.
    #[serde(default)]
    pub updated_at: String,
    /// Project pemilik suite (`Project::id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// Hentikan langkah Test berikutnya setelah satu langkah gagal. Cleanup
    /// tetap berjalan.
    #[serde(default)]
    pub stop_on_failure: bool,
    /// Kriteria lulus (exit criteria) dalam persen; `None` = 100%.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_pass_rate: Option<f64>,
    /// Hasil validasi terakhir (run nyata + perbaikan AI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<SuiteValidation>,
    /// Urutan group (mis. Purchasing → Plant → Sales). Group yang dipakai
    /// langkah tetapi belum terdaftar ditambahkan di belakang.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
}

/// Bukti bahwa suite pernah dijalankan dan lolos terhadap server sungguhan.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SuiteValidation {
    /// RFC 3339.
    pub at: String,
    #[serde(default)]
    pub environment: Option<String>,
    pub rounds: usize,
    pub passed: usize,
    pub total: usize,
    /// Langkah yang diperbaiki AI selama validasi.
    pub fixed: usize,
    /// [`suite_fingerprint`] saat validasi; beda = suite sudah diubah sesudahnya.
    pub fingerprint: String,
}

impl SuiteValidation {
    pub fn all_passed(&self) -> bool {
        self.total > 0 && self.passed == self.total
    }
}

impl HttpTestSuite {
    /// Validasi masih berlaku untuk definisi suite saat ini.
    pub fn validation_current(&self) -> Option<&SuiteValidation> {
        self.validation
            .as_ref()
            .filter(|v| v.fingerprint == suite_fingerprint(self))
    }

    /// Suite kosong baru untuk project `project_id`.
    pub fn new(name: &str, project_id: Option<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id: crate::http_collection::unique_id("suite"),
            name: name.trim().to_string(),
            project_id,
            created_at: now.clone(),
            updated_at: now,
            ..Default::default()
        }
    }

    /// Beri id unik pada langkah yang belum punya atau duplikat.
    pub fn ensure_ids(&mut self) {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut next = self.steps.len() + 1;
        for step in &mut self.steps {
            let id = sanitize_step_id(&step.id);
            if id.is_empty() || seen.contains(&id) {
                // Rujukan `cleanup_for` ke id lama yang duplikat tetap ke langkah pertama.
                loop {
                    let candidate = format!("s{next}");
                    next += 1;
                    if !seen.contains(&candidate) {
                        step.id = candidate;
                        break;
                    }
                }
            } else {
                step.id = id;
            }
            seen.insert(step.id.clone());
        }
        for step in &mut self.steps {
            if let Some(target) = &step.cleanup_for
                && !seen.contains(target)
            {
                step.cleanup_for = None;
            }
        }
    }

    /// Id langkah baru yang belum dipakai.
    pub fn next_step_id(&self) -> String {
        let mut n = self.steps.len() + 1;
        loop {
            let id = format!("s{n}");
            if !self.steps.iter().any(|s| s.id == id) {
                return id;
            }
            n += 1;
        }
    }

    pub fn step_index(&self, id: &str) -> Option<usize> {
        self.steps.iter().position(|s| s.id == id)
    }

    /// Nama group berurutan: `groups` lalu group langkah yang belum terdaftar.
    pub fn group_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for g in self
            .groups
            .iter()
            .chain(self.steps.iter().map(|s| &s.group))
        {
            let g = g.trim();
            if !g.is_empty() && !out.iter().any(|x| x == g) {
                out.push(g.to_string());
            }
        }
        out
    }

    /// Kunci urutan eksekusi: fase, lalu urutan group (Setup/Test). Langkah
    /// tanpa group berjalan lebih dulu di fasenya. Cleanup memakai urutan
    /// daftar (lihat [`auto_order_cleanup`]).
    fn order_key(&self, groups: &[String], step: &HttpTestStep) -> (StepPhase, usize) {
        let rank = if step.phase == StepPhase::Cleanup || step.group.trim().is_empty() {
            0
        } else {
            groups
                .iter()
                .position(|g| g == step.group.trim())
                .map(|p| p + 1)
                .unwrap_or(groups.len() + 1)
        };
        (step.phase, rank)
    }

    /// Urutkan langkah per fase dan group tanpa mengubah urutan di dalamnya.
    /// Daftar yang tampil = urutan eksekusi.
    pub fn normalize_order(&mut self) {
        let groups = self.group_names();
        self.groups = groups.clone();
        let mut steps = std::mem::take(&mut self.steps);
        steps.sort_by_key(|s| self.order_key(&groups, s));
        self.steps = steps;
    }

    /// Indeks langkah dalam urutan eksekusi.
    pub fn execution_order(&self) -> Vec<usize> {
        let groups = self.group_names();
        let mut idx: Vec<usize> = (0..self.steps.len()).collect();
        idx.sort_by_key(|&i| self.order_key(&groups, &self.steps[i]));
        idx
    }

    /// Ganti nama group di daftar dan di semua langkah.
    pub fn rename_group(&mut self, from: &str, to: &str) {
        let to = to.trim();
        for g in &mut self.groups {
            if g == from {
                *g = to.to_string();
            }
        }
        for s in &mut self.steps {
            if s.group == from {
                s.group = to.to_string();
            }
        }
        let names = self.group_names();
        self.groups = names;
    }

    /// Hapus group; langkahnya menjadi tanpa group.
    pub fn remove_group(&mut self, name: &str) {
        self.groups.retain(|g| g != name);
        for s in &mut self.steps {
            if s.group == name {
                s.group.clear();
            }
        }
    }

    /// Geser group satu posisi (`delta` = -1 naik, +1 turun).
    pub fn move_group(&mut self, name: &str, delta: isize) {
        self.groups = self.group_names();
        let Some(i) = self.groups.iter().position(|g| g == name) else {
            return;
        };
        let j = i as isize + delta;
        if j < 0 || j as usize >= self.groups.len() {
            return;
        }
        self.groups.swap(i, j as usize);
        self.normalize_order();
    }

    pub fn touch(&mut self) {
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    /// Pass rate minimum (persen) agar run dinyatakan lulus.
    pub fn pass_threshold(&self) -> f64 {
        self.min_pass_rate.unwrap_or(100.0).clamp(0.0, 100.0)
    }

    /// Jumlah langkah aktif yang mengubah data (bukan GET/HEAD/OPTIONS).
    pub fn mutating_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.enabled && s.mutates())
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

/// Nomor revisi penyimpanan suite. Naik setiap kali suite ditulis, supaya
/// tab yang membuka suite yang sama bisa memuat ulang dari disk.
static SUITES_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn suites_revision() -> u64 {
    SUITES_REV.load(Ordering::SeqCst)
}

pub fn save_suite(suite: &HttpTestSuite) -> Result<(), String> {
    save_suite_in(&suites_dir(), suite)?;
    SUITES_REV.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// Muat satu suite dari disk.
pub fn load_suite(id: &str) -> Option<HttpTestSuite> {
    load_suite_in(&suites_dir(), id)
}

pub fn load_suite_in(dir: &std::path::Path, id: &str) -> Option<HttpTestSuite> {
    let path = dir.join(format!("{}.json", safe_file_id(id)));
    let bytes = std::fs::read(&path).ok()?;
    match serde_json::from_slice::<HttpTestSuite>(&bytes) {
        Ok(mut s) => {
            s.ensure_ids();
            Some(s)
        }
        Err(e) => {
            log::warn!("[HTTP_TESTS] unreadable suite {}: {e}", path.display());
            None
        }
    }
}

/// Suite milik project `project_id`, terbaru lebih dulu.
pub fn load_project_suites(project_id: &str) -> Vec<HttpTestSuite> {
    load_suites()
        .into_iter()
        .filter(|s| s.project_id.as_deref() == Some(project_id))
        .collect()
}

/// Tulis ulang request satu langkah (dari tab HTTP client) ke suite di disk.
/// Variabel `{{…}}` di request dipertahankan apa adanya.
pub fn update_step_request(
    suite_id: &str,
    step_id: &str,
    request: SavedRequest,
) -> Result<String, String> {
    let mut suite = load_suite(suite_id)
        .ok_or_else(|| "The integration test suite no longer exists".to_string())?;
    let step = suite
        .steps
        .iter_mut()
        .find(|s| s.id == step_id)
        .ok_or_else(|| "The test step no longer exists in its suite".to_string())?;
    let keep = step.request.clone();
    step.request = SavedRequest {
        id: keep.id,
        workspace_id: keep.workspace_id,
        folder_id: keep.folder_id,
        name: keep.name,
        description: keep.description,
        tables: keep.tables,
        source: keep.source,
        route: keep.route,
        ..request
    };
    let name = step.name.clone();
    suite.touch();
    save_suite(&suite)?;
    Ok(name)
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
            Ok(mut s) => {
                s.ensure_ids();
                out.push(s)
            }
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
        Ok(()) => {
            SUITES_REV.fetch_add(1, Ordering::SeqCst);
            let _ = std::fs::remove_dir_all(runs_dir().join(safe_file_id(id)));
            Ok(())
        }
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
    if let Some(name) = t.strip_prefix("cookie:") {
        return cookie_value(&resp.headers, name.trim());
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

/// Nilai cookie `name` dari header `Set-Cookie` response.
pub fn cookie_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
        .flat_map(|(_, v)| v.split('\n'))
        .filter_map(|line| line.split(';').next())
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| k.trim() == name)
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
}

/// Header response untuk ditampilkan: nilai rahasia disamarkan, nama cookie
/// dan atributnya tetap terlihat.
pub fn redact_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(k, v)| {
            let value = if k.eq_ignore_ascii_case("set-cookie") {
                v.split('\n')
                    .map(|line| {
                        let mut parts = line.splitn(2, ';');
                        let first = parts.next().unwrap_or("");
                        let rest = parts.next().map(|r| format!(";{r}")).unwrap_or_default();
                        match first.split_once('=') {
                            Some((n, _)) => {
                                format!("{}={}{rest}", n.trim(), crate::http_ai::REDACTED)
                            }
                            None => line.to_string(),
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else if sensitive_var(k) {
                crate::http_ai::REDACTED.to_string()
            } else {
                v.clone()
            };
            (k.clone(), value)
        })
        .collect()
}

/// Hasil satu assertion.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    evaluate_actual(read_target(resp, &a.target), a, vars)
}

/// Assertion atas hasil query langkah DB.
pub fn evaluate_query(
    out: &QueryOutput,
    a: &Assertion,
    vars: &HashMap<String, String>,
) -> AssertResult {
    evaluate_actual(
        crate::http_test_db::read_query_target(out, &a.target),
        a,
        vars,
    )
}

/// Bandingkan nilai aktual dengan assertion. Nilai harapan boleh berisi
/// variabel dan aritmetika sederhana (`{{a}} + {{b}}`).
pub fn evaluate_actual(
    actual: Option<String>,
    a: &Assertion,
    vars: &HashMap<String, String>,
) -> AssertResult {
    let expected = crate::http_test_db::expected_value(&a.value, vars);
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
        (AssertOp::Approx, Some(v)) => {
            let (want, tol) = crate::http_test_db::split_tolerance(&expected);
            matches!((as_number(v), as_number(&want)), (Some(x), Some(y)) if (x - y).abs() <= tol)
        }
    };
    AssertResult {
        assertion: a.clone(),
        passed,
        actual,
    }
}

// ─── Relasi antar langkah ───────────────────────────────────────────────────

/// Variabel `{{x}}` yang dibaca langkah (request + nilai assertion).
pub fn step_inputs(step: &HttpTestStep) -> Vec<String> {
    let r = &step.request;
    let mut texts: Vec<&str> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    if step.is_db() {
        if let Some(db) = &step.db {
            let var = db.connection_var.trim();
            if !var.is_empty() {
                out.push(var.to_string());
            }
            texts.push(&db.sql);
            if let Some(d) = &db.database {
                texts.push(d);
            }
        }
    } else {
        texts.extend([
            r.url.as_str(),
            &r.body_text,
            &r.bearer_token,
            &r.basic_user,
            &r.basic_pass,
            &r.api_key_name,
            &r.api_key_value,
        ]);
        for rows in [&r.params, &r.headers, &r.form_data] {
            for (k, v, enabled) in rows {
                if *enabled {
                    texts.push(k);
                    texts.push(v);
                }
            }
        }
    }
    for a in &step.assertions {
        texts.push(&a.value);
    }
    for t in texts {
        for v in unresolved_vars(t) {
            if !out.contains(&v) {
                out.push(v);
            }
        }
    }
    out
}

/// Variabel yang dihasilkan langkah lewat `extract`.
pub fn step_outputs(step: &HttpTestStep) -> Vec<String> {
    step.extract
        .iter()
        .map(|e| e.var.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Relasi satu langkah dengan langkah lain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StepDeps {
    pub uses: Vec<String>,
    pub provides: Vec<String>,
    /// Indeks langkah (di `suite.steps`) yang menyediakan variabel yang dipakai.
    pub depends_on: Vec<usize>,
    /// Indeks langkah yang memakai variabel dari langkah ini.
    pub used_by: Vec<usize>,
}

/// Relasi semua langkah menurut urutan eksekusi. Variabel diambil dari
/// penyedia terakhir sebelum langkah tersebut.
pub fn dependencies(suite: &HttpTestSuite) -> Vec<StepDeps> {
    let mut deps: Vec<StepDeps> = suite
        .steps
        .iter()
        .map(|s| StepDeps {
            uses: step_inputs(s),
            provides: step_outputs(s),
            ..Default::default()
        })
        .collect();
    let mut provider: HashMap<String, usize> = HashMap::new();
    for i in suite.execution_order() {
        let uses = deps[i].uses.clone();
        for v in &uses {
            if let Some(&p) = provider.get(v)
                && p != i
            {
                if !deps[i].depends_on.contains(&p) {
                    deps[i].depends_on.push(p);
                }
                if !deps[p].used_by.contains(&i) {
                    deps[p].used_by.push(i);
                }
            }
        }
        if suite.steps[i].enabled {
            for v in deps[i].provides.clone() {
                provider.insert(v, i);
            }
        }
    }
    deps
}

/// Langkah yang datanya dibersihkan oleh langkah Cleanup `idx`: `cleanup_for`
/// bila diisi, selain itu penyedia terakhir variabel yang dipakainya.
pub fn cleanup_target(suite: &HttpTestSuite, deps: &[StepDeps], idx: usize) -> Option<usize> {
    let step = suite.steps.get(idx)?;
    if step.phase != StepPhase::Cleanup {
        return None;
    }
    if let Some(t) = step
        .cleanup_for
        .as_deref()
        .and_then(|id| suite.step_index(id))
    {
        return Some(t);
    }
    deps.get(idx)?
        .depends_on
        .iter()
        .copied()
        .filter(|&p| suite.steps[p].phase != StepPhase::Cleanup && suite.steps[p].mutates())
        .max()
}

/// Langkah Setup/Test yang membuat data: method POST/PUT yang mengekstrak
/// variabel bukan rahasia (mis. id baru). Login yang hanya mengekstrak token
/// atau session tidak dihitung sebagai pembuat data.
pub fn creates_data(step: &HttpTestStep) -> bool {
    step.phase != StepPhase::Cleanup
        && step.kind.is_http()
        && matches!(step.request.method, HttpMethod::POST | HttpMethod::PUT)
        && step
            .extract
            .iter()
            .any(|e| !e.var.trim().is_empty() && !sensitive_var(&e.var))
}

/// Langkah yang ikut dijalankan bila hanya group `groups` yang dipilih:
/// langkah di group itu, semua langkah yang mereka butuhkan (transitif, dari
/// group mana pun), Setup tanpa group, dan cleanup untuk langkah yang ikut.
/// `groups` kosong = semua langkah.
pub fn select_for_groups(suite: &HttpTestSuite, groups: &[String]) -> Vec<bool> {
    let n = suite.steps.len();
    if groups.is_empty() {
        return vec![true; n];
    }
    let deps = dependencies(suite);
    let in_group = |s: &HttpTestStep| groups.iter().any(|g| g.trim() == s.group.trim());
    let mut include: Vec<bool> = suite
        .steps
        .iter()
        .map(|s| {
            (s.phase != StepPhase::Cleanup && in_group(s))
                || (s.phase == StepPhase::Setup && s.group.trim().is_empty())
        })
        .collect();
    // Tarik prasyarat secara transitif.
    let mut stack: Vec<usize> = (0..n).filter(|&i| include[i]).collect();
    while let Some(i) = stack.pop() {
        for &p in &deps[i].depends_on {
            if !include[p] && suite.steps[p].phase != StepPhase::Cleanup {
                include[p] = true;
                stack.push(p);
            }
        }
    }
    // Cleanup: milik langkah yang ikut, atau cleanup di group terpilih.
    for i in 0..n {
        if suite.steps[i].phase != StepPhase::Cleanup {
            continue;
        }
        include[i] = match cleanup_target(suite, &deps, i) {
            Some(t) => include[t],
            None => in_group(&suite.steps[i]) || suite.steps[i].group.trim().is_empty(),
        };
    }
    include
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LintLevel {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LintIssue {
    pub level: LintLevel,
    /// Indeks langkah di `suite.steps`, `None` = seluruh suite.
    pub step: Option<usize>,
    pub message: String,
}

/// Periksa urutan dan relasi langkah. `known_vars` = variabel environment
/// project yang tersedia saat run.
pub fn lint(
    suite: &HttpTestSuite,
    known_vars: &std::collections::HashSet<String>,
) -> Vec<LintIssue> {
    let mut out = Vec::new();
    let deps = dependencies(suite);
    let suite_vars: std::collections::HashSet<String> = suite
        .variables
        .iter()
        .map(|(k, _)| k.trim().to_string())
        .collect();
    let order = suite.execution_order();
    let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(p, &i)| (i, p)).collect();
    if suite.steps.iter().filter(|s| s.enabled).count() == 0 {
        out.push(LintIssue {
            level: LintLevel::Info,
            step: None,
            message: "The suite has no enabled steps".into(),
        });
    }
    for &i in &order {
        let step = &suite.steps[i];
        if !step.enabled {
            continue;
        }
        let d = &deps[i];
        for v in &d.uses {
            if v == "run_id" || suite_vars.contains(v) || known_vars.contains(v) {
                continue;
            }
            if d.depends_on.iter().any(|&p| deps[p].provides.contains(v)) {
                continue;
            }
            let later = order
                .iter()
                .find(|&&j| pos[&j] > pos[&i] && deps[j].provides.contains(v));
            let disabled = suite
                .steps
                .iter()
                .enumerate()
                .find(|(j, s)| !s.enabled && deps[*j].provides.contains(v));
            let message = if let Some(&j) = later {
                format!(
                    "Uses {{{{{v}}}}} which is only provided later by step {} '{}'. Move this step after it.",
                    pos[&j] + 1,
                    suite.steps[j].name
                )
            } else if let Some((j, s)) = disabled {
                format!(
                    "Uses {{{{{v}}}}} from disabled step {} '{}'",
                    pos[&j] + 1,
                    s.name
                )
            } else {
                format!("Uses {{{{{v}}}}} but no variable or earlier step provides it")
            };
            out.push(LintIssue {
                level: LintLevel::Error,
                step: Some(i),
                message,
            });
        }
        for v in &d.uses {
            let empty_suite_var = suite
                .variables
                .iter()
                .any(|(k, val)| k.trim() == v && val.is_empty());
            let provided = known_vars.contains(v)
                || d.depends_on.iter().any(|&p| deps[p].provides.contains(v));
            if empty_suite_var && !provided {
                out.push(LintIssue {
                    level: LintLevel::Warning,
                    step: Some(i),
                    message: format!(
                        "Variable {{{{{v}}}}} is empty; set it in Variables or the project environment"
                    ),
                });
            }
        }
        if step.assertions.is_empty() {
            out.push(LintIssue {
                level: LintLevel::Warning,
                step: Some(i),
                message: "No assertions: the step passes on any response".into(),
            });
        }
        if step.phase == StepPhase::Cleanup && step.kind.is_http() && !step.mutates() {
            out.push(LintIssue {
                level: LintLevel::Warning,
                step: Some(i),
                message: format!(
                    "Cleanup step uses {} which normally does not remove data",
                    step.request.method.label()
                ),
            });
        }
        for message in step_kind_issues(step) {
            out.push(LintIssue {
                level: LintLevel::Error,
                step: Some(i),
                message,
            });
        }
        if let Some(c) = step.concurrent()
            && (c.count < 2 || c.count > MAX_CONCURRENCY || c.rounds > MAX_ROUNDS)
        {
            out.push(LintIssue {
                level: LintLevel::Warning,
                step: Some(i),
                message: format!(
                    "Concurrency is limited to 2..={MAX_CONCURRENCY} requests and {MAX_ROUNDS} rounds; it runs as {} x {}",
                    c.count(),
                    c.rounds()
                ),
            });
        }
        if step.phase == StepPhase::Cleanup && cleanup_target(suite, &deps, i).is_none() {
            out.push(LintIssue {
                level: LintLevel::Info,
                step: Some(i),
                message: "Cleanup step is not linked to a step; it runs whenever data was changed"
                    .into(),
            });
        }
    }
    // Setiap langkah yang membuat data harus punya cleanup.
    let cleanups: Vec<(usize, usize)> = (0..suite.steps.len())
        .filter(|&i| suite.steps[i].enabled)
        .filter_map(|i| cleanup_target(suite, &deps, i).map(|t| (i, t)))
        .collect();
    for (i, step) in suite.steps.iter().enumerate() {
        if step.enabled && creates_data(step) && !cleanups.iter().any(|(_, t)| *t == i) {
            out.push(LintIssue {
                level: LintLevel::Warning,
                step: Some(i),
                message:
                    "Creates data but no cleanup step removes it; test data will be left behind"
                        .into(),
            });
        }
    }
    // Cleanup harus berjalan terbalik dari urutan pembuatan (hapus turunan dulu).
    for a in 0..cleanups.len() {
        for b in (a + 1)..cleanups.len() {
            let (ca, ta) = cleanups[a];
            let (cb, tb) = cleanups[b];
            let (first, first_t, second_t) = if pos[&ca] < pos[&cb] {
                (ca, ta, tb)
            } else {
                (cb, tb, ta)
            };
            if pos.get(&first_t) < pos.get(&second_t) {
                out.push(LintIssue {
                    level: LintLevel::Warning,
                    step: Some(first),
                    message: "Cleanup order: data created earlier is removed first; dependent data may block it. Use Auto-order cleanup."
                        .into(),
                });
            }
        }
    }
    out.sort_by_key(|i| {
        (
            i.level,
            i.step.map(|s| pos.get(&s).copied().unwrap_or(usize::MAX)),
        )
    });
    out.dedup();
    out
}

/// Target agregat langkah concurrent (`count:`, `any:`, `all:`).
pub fn is_aggregate_target(target: &str) -> bool {
    let t = target.trim();
    t.starts_with("count:") || t.starts_with("any:") || t.starts_with("all:")
}

/// Kesalahan konfigurasi khusus langkah DB / concurrent.
pub fn step_kind_issues(step: &HttpTestStep) -> Vec<String> {
    let mut out = Vec::new();
    if step.is_db() {
        let Some(db) = &step.db else {
            out.push("Database step has no query".into());
            return out;
        };
        if db.connection_var.trim().is_empty() {
            out.push("Database step needs a connection variable (e.g. db_main)".into());
        }
        if db.sql.trim().is_empty() {
            out.push("Database step has an empty SQL query".into());
        } else {
            // Variabel diganti nilai contoh hanya untuk memeriksa bentuk SQL.
            let dummy: HashMap<String, String> = unresolved_vars(&db.sql)
                .into_iter()
                .map(|v| (v, "1".to_string()))
                .collect();
            match crate::http_test_db::bind_vars(
                &db.sql,
                &dummy,
                crate::http_test_db::Placeholder::Question,
            ) {
                Err(e) => out.push(e),
                Ok(b) => {
                    if let Err(e) = crate::agent::bound_query::ensure_select_only(
                        &crate::models::enums::DatabaseType::MySQL,
                        &b.sql,
                    ) {
                        out.push(e.to_string());
                    }
                }
            }
        }
        for a in &step.assertions {
            if !crate::http_test_db::is_query_target(&a.target) {
                out.push(format!(
                    "Target '{}' does not apply to a database step; use row_count, scalar, cell:<row>.<column> or column:<column>",
                    a.target
                ));
            }
        }
    } else if step.concurrent().is_none() {
        for a in &step.assertions {
            if is_aggregate_target(&a.target) {
                out.push(format!(
                    "Target '{}' needs concurrency on this step",
                    a.target
                ));
            }
        }
    }
    out
}

/// Urutkan langkah Cleanup terbalik dari urutan pembuatan datanya (LIFO).
/// Cleanup tanpa target ditaruh paling akhir dengan urutan semula.
pub fn auto_order_cleanup(suite: &mut HttpTestSuite) {
    suite.normalize_order();
    let deps = dependencies(suite);
    let first_cleanup = suite
        .steps
        .iter()
        .position(|s| s.phase == StepPhase::Cleanup)
        .unwrap_or(suite.steps.len());
    let order = suite.execution_order();
    let pos: HashMap<usize, usize> = order.iter().enumerate().map(|(p, &i)| (i, p)).collect();
    let mut keyed: Vec<(usize, usize, HttpTestStep)> = suite.steps[first_cleanup..]
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let target = cleanup_target(suite, &deps, first_cleanup + n);
            // Target yang dieksekusi lebih akhir → dibersihkan lebih dulu.
            let key = target
                .map(|t| usize::MAX / 2 - pos[&t])
                .unwrap_or(usize::MAX / 2 + 1);
            (key, n, s.clone())
        })
        .collect();
    keyed.sort_by_key(|(k, n, _)| (*k, *n));
    suite.steps.truncate(first_cleanup);
    suite.steps.extend(keyed.into_iter().map(|(_, _, s)| s));
}

// ─── Runner ─────────────────────────────────────────────────────────────────

/// Status hasil langkah menurut kategori ISTQB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    Passed,
    /// Response diterima tetapi assertion gagal.
    Failed,
    /// Request tidak mendapat response (koneksi, timeout).
    Error,
    /// Tidak dijalankan karena prasyarat (variabel, setup) tidak terpenuhi.
    Blocked,
    /// Dinonaktifkan atau tidak perlu dijalankan.
    Skipped,
}

impl StepOutcome {
    pub fn label(self) -> &'static str {
        match self {
            StepOutcome::Passed => "Passed",
            StepOutcome::Failed => "Failed",
            StepOutcome::Error => "Error",
            StepOutcome::Blocked => "Blocked",
            StepOutcome::Skipped => "Skipped",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepResult {
    pub index: usize,
    #[serde(default)]
    pub step_id: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub phase: StepPhase,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub time_ms: u128,
    pub error: Option<String>,
    pub assertions: Vec<AssertResult>,
    /// Variabel hasil ekstraksi; nilai rahasia sudah disamarkan.
    pub extracted: Vec<(String, String)>,
    /// Variabel yang masih `{{…}}` saat request akan dikirim.
    pub unresolved: Vec<String>,
    /// Header response (nilai rahasia disamarkan).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers: Vec<(String, String)>,
    /// Potongan awal body respons (nilai rahasia disamarkan).
    pub body_preview: String,
    pub skipped: bool,
    #[serde(default)]
    pub blocked: bool,
    /// Alasan dilewati/diblokir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Jumlah percobaan ulang (hanya cleanup).
    #[serde(default)]
    pub retries: u8,
    #[serde(default, skip_serializing_if = "StepKind::is_http")]
    pub kind: StepKind,
    /// Detail langkah DB (SQL, contoh baris, polling).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db: Option<DbStepInfo>,
    /// Respons per request untuk langkah concurrent, urut waktu selesai.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<InstanceResult>,
}

/// Detail eksekusi langkah DB untuk laporan.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbStepInfo {
    pub connection: String,
    /// SQL dengan placeholder dan daftar nilai parameter (rahasia disamarkan).
    pub sql: String,
    pub columns: Vec<String>,
    /// Paling banyak [`crate::http_test_db::SAMPLE_ROWS`] baris.
    pub sample: Vec<Vec<String>>,
    pub row_count: usize,
    #[serde(default)]
    pub truncated: bool,
    /// Berapa kali query dijalankan (polling `wait_until_ms`).
    pub attempts: u32,
    /// Lama menunggu sampai lulus/timeout.
    #[serde(default)]
    pub waited_ms: u128,
    #[serde(default)]
    pub timed_out: bool,
}

/// Satu respons dari burst concurrent.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceResult {
    /// Putaran (1-based).
    pub round: u32,
    /// Nomor request di dalam putaran (1-based).
    pub index: u32,
    pub status: u16,
    pub time_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Potongan body (rahasia disamarkan).
    #[serde(default)]
    pub body_preview: String,
}

impl StepResult {
    pub fn passed(&self) -> bool {
        self.outcome() == StepOutcome::Passed
    }

    pub fn outcome(&self) -> StepOutcome {
        if self.skipped {
            StepOutcome::Skipped
        } else if self.blocked {
            StepOutcome::Blocked
        } else if self.error.is_some() {
            StepOutcome::Error
        } else if self.assertions.iter().all(|a| a.passed) {
            StepOutcome::Passed
        } else {
            StepOutcome::Failed
        }
    }

    /// Request benar-benar terkirim dan mendapat response.
    pub fn executed(&self) -> bool {
        !self.skipped && !self.blocked && self.error.is_none()
    }

    /// Server menerima request (2xx), jadi data yang dibuat kemungkinan ada.
    /// Request yang ditolak (4xx/5xx) tidak perlu dibersihkan.
    pub fn created(&self) -> bool {
        self.executed() && (200..300).contains(&self.status)
    }

    /// Server menerima minimal satu request burst (2xx).
    pub fn created_count(&self) -> usize {
        if self.instances.is_empty() {
            usize::from(self.created())
        } else {
            self.instances
                .iter()
                .filter(|i| i.error.is_none() && (200..300).contains(&i.status))
                .count()
        }
    }

    fn not_run(index: usize, step: &HttpTestStep, blocked: bool, note: String) -> Self {
        StepResult {
            index,
            step_id: step.id.clone(),
            name: step.name.clone(),
            group: step.group.clone(),
            phase: step.phase,
            kind: step.kind,
            method: step.method_label().to_string(),
            url: if step.is_db() {
                step.db
                    .as_ref()
                    .map(|d| format!("{{{{{}}}}}", d.connection_var))
                    .unwrap_or_default()
            } else {
                crate::http_ai::redact_url(&step.request.url)
            },
            skipped: !blocked,
            blocked,
            note: Some(note),
            ..Default::default()
        }
    }
}

/// Data uji yang mungkin tertinggal di server setelah run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Residue {
    /// Langkah yang membuat data.
    pub step_index: usize,
    pub step_name: String,
    pub method: String,
    pub url: String,
    /// Variabel hasil ekstraksi langkah pembuat (mis. id), disamarkan bila rahasia.
    pub ids: Vec<(String, String)>,
    pub reason: String,
}

/// Ringkasan akhir satu run.
#[derive(Debug, Clone, Default)]
pub struct RunSummary {
    pub cancelled: bool,
    pub aborted: bool,
    pub results: Vec<StepResult>,
    pub residue: Vec<Residue>,
    /// Server tidak terjangkau sebelum langkah pertama; tidak ada yang dikirim.
    pub preflight_error: Option<String>,
}

#[derive(Debug)]
pub enum TestEvent {
    StepStarted(usize),
    StepFinished(Box<StepResult>),
    /// Fase cleanup dimulai (dipakai UI untuk label kemajuan).
    CleanupStarted,
    Finished(Box<RunSummary>),
}

pub struct TestRunHandle {
    pub rx: mpsc::Receiver<TestEvent>,
    cancel: Arc<AtomicBool>,
    abort: Arc<AtomicBool>,
}

impl TestRunHandle {
    /// Hentikan langkah Setup/Test; langkah Cleanup tetap dijalankan.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Hentikan seketika, termasuk cleanup (data uji bisa tertinggal).
    pub fn abort(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        self.abort.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelling(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// Body JSON dengan nilai kunci sensitif (token, password, …) disamarkan.
pub fn redact_body(body: &str) -> String {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(map) => {
                for (k, val) in map.iter_mut() {
                    if sensitive_var(k) {
                        if !val.is_null() {
                            *val = Value::String(crate::http_ai::REDACTED.to_string());
                        }
                    } else {
                        walk(val);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(walk),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(body) {
        Ok(mut v) => {
            walk(&mut v);
            serde_json::to_string_pretty(&v).unwrap_or_default()
        }
        Err(_) => body.to_string(),
    }
}

/// Nama variabel/kunci yang nilainya rahasia (token, password, …).
pub fn is_sensitive_var(name: &str) -> bool {
    sensitive_var(name)
}

fn sensitive_var(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    crate::http_client_widgets::is_sensitive_key(name)
        || [
            "token", "password", "passwd", "secret", "api_key", "apikey", "cookie", "session",
        ]
        .iter()
        .any(|s| lower.contains(s))
}

/// Nilai variabel untuk ditampilkan/disimpan: rahasia disamarkan.
pub fn display_var_value(name: &str, value: &str) -> String {
    if sensitive_var(name) && !value.is_empty() {
        crate::http_ai::REDACTED.to_string()
    } else {
        value.to_string()
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
    let unresolved: Vec<String> = step_inputs(&HttpTestStep {
        request: req.clone(),
        ..Default::default()
    });
    let spec = RequestSpec::from_saved(&req);
    let url = spec.full_url();
    let resp = send(spec);
    let mut result = StepResult {
        index,
        step_id: step.id.clone(),
        name: step.name.clone(),
        group: step.group.clone(),
        phase: step.phase,
        method: req.method.label().to_string(),
        url: crate::http_ai::redact_url(&url),
        status: resp.status,
        time_ms: resp.time_ms,
        error: resp.error.clone(),
        unresolved,
        body_preview: crate::repo_scan::truncate_chars(&redact_body(&resp.body), 2_000),
        response_headers: redact_headers(&resp.headers),
        ..Default::default()
    };
    if resp.error.is_some() {
        return result;
    }
    let mut missing_extracts = Vec::new();
    for ex in &step.extract {
        match read_target(&resp, &ex.from) {
            Some(v) => {
                vars.insert(ex.var.clone(), v.clone());
                result
                    .extracted
                    .push((ex.var.clone(), display_var_value(&ex.var, &v)));
            }
            None if !ex.var.trim().is_empty() => missing_extracts.push(ex),
            None => {}
        }
    }
    result.assertions = step
        .assertions
        .iter()
        .map(|a| evaluate(&resp, a, vars))
        .collect();
    // Extract yang tidak menemukan nilai membuat langkah gagal: langkah
    // berikutnya butuh nilainya, dan penyebabnya ada di langkah ini.
    for ex in missing_extracts {
        result.assertions.push(AssertResult {
            assertion: Assertion {
                target: ex.from.clone(),
                op: AssertOp::Exists,
                value: format!("(extract {{{{{}}}}})", ex.var),
            },
            passed: false,
            actual: None,
        });
    }
    result
}

// ─── Langkah DB ─────────────────────────────────────────────────────────────

/// Jalankan langkah DB: query baca, assertion atas hasilnya, dan polling
/// sampai lulus bila `wait_until_ms` diisi. `cancel` menghentikan polling.
pub fn run_db_step(
    index: usize,
    step: &HttpTestStep,
    vars: &mut HashMap<String, String>,
    db: Option<&dyn DbExecutor>,
    cancel: &AtomicBool,
) -> StepResult {
    let mut result = StepResult::not_run(index, step, false, String::new());
    result.skipped = false;
    result.note = None;
    let Some(cfg) = step.db.as_ref() else {
        result.error = Some("Database step has no query".into());
        return result;
    };
    let Some(db) = db else {
        result.error = Some("Database steps cannot run here: no database access".into());
        return result;
    };
    let connection = vars
        .get(cfg.connection_var.trim())
        .cloned()
        .unwrap_or_default();
    let req = crate::http_test_db::DbRequest {
        connection,
        database: cfg
            .database
            .as_deref()
            .map(|d| substitute(d, vars))
            .filter(|d| !d.trim().is_empty()),
        sql: cfg.sql.clone(),
        vars: vars.clone(),
    };
    let wait = cfg
        .wait_until_ms
        .unwrap_or(0)
        .min(crate::http_test_db::MAX_WAIT_MS);
    let poll = cfg.poll_ms.max(crate::http_test_db::MIN_POLL_MS);
    let started = std::time::Instant::now();
    let mut attempts = 0u32;
    let (resp, assertions) = loop {
        attempts += 1;
        let resp = match db.query(&req) {
            Ok(r) => r,
            Err(e) => {
                result.error = Some(e);
                result.time_ms = started.elapsed().as_millis();
                result.db = Some(DbStepInfo {
                    attempts,
                    ..Default::default()
                });
                return result;
            }
        };
        let assertions: Vec<AssertResult> = step
            .assertions
            .iter()
            .map(|a| evaluate_query(&resp.output, a, vars))
            .collect();
        let all_ok = assertions.iter().all(|a| a.passed);
        let elapsed = started.elapsed().as_millis() as u64;
        if all_ok || wait == 0 || elapsed + poll > wait || cancel.load(Ordering::SeqCst) {
            break (resp, assertions);
        }
        std::thread::sleep(std::time::Duration::from_millis(poll));
    };
    let out = &resp.output;
    result.time_ms = started.elapsed().as_millis();
    result.url = resp.connection_name.clone();
    result.assertions = assertions;
    let all_ok = result.assertions.iter().all(|a| a.passed);
    result.db = Some(DbStepInfo {
        connection: resp.connection_name.clone(),
        sql: resp.sql_display.clone(),
        columns: out.columns.clone(),
        sample: out
            .rows
            .iter()
            .take(crate::http_test_db::SAMPLE_ROWS)
            .cloned()
            .collect(),
        row_count: out.rows.len(),
        truncated: out.truncated,
        attempts,
        waited_ms: if wait > 0 { result.time_ms } else { 0 },
        timed_out: wait > 0 && !all_ok,
    });
    result.body_preview = format!("{} row(s)", out.rows.len());
    for ex in &step.extract {
        if ex.var.trim().is_empty() {
            continue;
        }
        match crate::http_test_db::read_query_target(out, &ex.from) {
            Some(v) => {
                vars.insert(ex.var.clone(), v.clone());
                result
                    .extracted
                    .push((ex.var.clone(), display_var_value(&ex.var, &v)));
            }
            None => result.assertions.push(AssertResult {
                assertion: Assertion {
                    target: ex.from.clone(),
                    op: AssertOp::Exists,
                    value: format!("(extract {{{{{}}}}})", ex.var),
                },
                passed: false,
                actual: None,
            }),
        }
    }
    result
}

// ─── Langkah concurrent ─────────────────────────────────────────────────────

/// Status cocok dengan pola `2xx`/`4xx` atau angka persis.
fn status_matches(status: &str, pattern: &str) -> bool {
    let p = pattern.trim().to_ascii_lowercase();
    if p.len() == 3 && p.ends_with("xx") {
        return status.trim().starts_with(&p[..1]);
    }
    status.trim() == p
}

/// Assertion atas kumpulan respons burst:
/// - `count:<target>=<nilai>`: jumlah respons yang targetnya bernilai itu
///   (`count:status=2xx`, `count:json:$.status=success`); tanpa `=` →
///   jumlah respons yang memiliki target;
/// - `any:<target>`: lulus bila minimal satu respons memenuhi;
/// - `all:<target>` atau tanpa prefix: lulus bila semua memenuhi.
pub fn evaluate_burst(
    resps: &[&HttpClientResponse],
    a: &Assertion,
    vars: &HashMap<String, String>,
) -> AssertResult {
    let t = a.target.trim();
    if let Some(cond) = t.strip_prefix("count:") {
        let (target, want) = match cond.split_once('=') {
            Some((tg, v)) => (tg.trim(), Some(substitute(v.trim(), vars))),
            None => (cond.trim(), None),
        };
        let is_status = target.eq_ignore_ascii_case("status");
        let n = resps
            .iter()
            .filter(|r| r.error.is_none())
            .filter(|r| match (read_target(r, target), &want) {
                (Some(v), Some(w)) if is_status => status_matches(&v, w),
                (Some(v), Some(w)) => {
                    v.trim() == w.trim()
                        || matches!((as_number(&v), as_number(w)), (Some(x), Some(y)) if x == y)
                }
                (Some(_), None) => true,
                (None, _) => false,
            })
            .count();
        let mut r = evaluate_actual(Some(n.to_string()), a, vars);
        r.assertion = a.clone();
        return r;
    }
    let (any, inner) = match t.strip_prefix("any:") {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix("all:").unwrap_or(t)),
    };
    let inner_assert = Assertion {
        target: inner.trim().to_string(),
        ..a.clone()
    };
    let each: Vec<AssertResult> = resps
        .iter()
        .map(|r| evaluate(r, &inner_assert, vars))
        .collect();
    let passed = if any {
        each.iter().any(|r| r.passed)
    } else {
        !each.is_empty() && each.iter().all(|r| r.passed)
    };
    let pick = if any {
        each.iter().find(|r| r.passed)
    } else {
        each.iter().find(|r| !r.passed)
    };
    let matching = each.iter().filter(|r| r.passed).count();
    let actual = pick
        .or(each.first())
        .and_then(|r| r.actual.clone())
        .map(|v| format!("{v} ({matching}/{} matched)", each.len()));
    AssertResult {
        assertion: a.clone(),
        passed,
        actual,
    }
}

/// Kirim `count` salinan request. Dengan `parallel`, semua thread menunggu di
/// barrier lalu mengirim bersamaan; tanpa itu (atau `barrier = false` tanpa
/// pengirim paralel) dikirim berurutan. Hasil urut waktu selesai.
fn send_burst(
    spec: &RequestSpec,
    count: usize,
    barrier: bool,
    send: &dyn Fn(RequestSpec) -> HttpClientResponse,
    parallel: Option<&(dyn Fn(RequestSpec) -> HttpClientResponse + Sync)>,
) -> Vec<(usize, HttpClientResponse)> {
    let Some(par) = parallel else {
        return (0..count).map(|k| (k, send(spec.clone()))).collect();
    };
    let gate = std::sync::Barrier::new(count);
    let done: std::sync::Mutex<Vec<(usize, HttpClientResponse)>> =
        std::sync::Mutex::new(Vec::with_capacity(count));
    std::thread::scope(|s| {
        for k in 0..count {
            let (gate, done, spec) = (&gate, &done, spec.clone());
            s.spawn(move || {
                if barrier {
                    gate.wait();
                }
                let resp = par(spec);
                if let Ok(mut d) = done.lock() {
                    d.push((k, resp));
                }
            });
        }
    });
    done.into_inner().unwrap_or_default()
}

/// Jalankan langkah HTTP sebagai burst serentak `rounds` kali. Assertion
/// dievaluasi per putaran; langkah lulus bila semua putaran lulus.
/// `extract` dari respons sukses pertama; tiap respons sukses ke-n juga
/// mengisi `{{var#n}}`.
fn run_concurrent_step(
    index: usize,
    step: &HttpTestStep,
    conc: Concurrency,
    vars: &mut HashMap<String, String>,
    io: &RunIo,
    cancel: &AtomicBool,
) -> (StepResult, Vec<HashMap<String, String>>) {
    let req = resolve_request(&step.request, vars);
    let spec = RequestSpec::from_saved(&req);
    let url = spec.full_url();
    let started = std::time::Instant::now();
    let mut result = StepResult::not_run(index, step, false, String::new());
    result.skipped = false;
    result.note = None;
    result.url = crate::http_ai::redact_url(&url);
    result.unresolved = step_inputs(&HttpTestStep {
        request: req.clone(),
        ..Default::default()
    });
    let mut chosen: Option<Vec<AssertResult>> = None;
    let mut successes: Vec<HttpClientResponse> = Vec::new();
    let mut first_status: Option<u16> = None;
    let mut all_errors = true;
    for round in 0..conc.rounds() {
        if round > 0 && cancel.load(Ordering::SeqCst) {
            break;
        }
        let burst = send_burst(&spec, conc.count(), conc.barrier, io.send, io.send_parallel);
        for (k, r) in &burst {
            result.instances.push(InstanceResult {
                round: round as u32 + 1,
                index: *k as u32 + 1,
                status: r.status,
                time_ms: r.time_ms,
                error: r.error.clone(),
                body_preview: crate::repo_scan::truncate_chars(&redact_body(&r.body), 300),
            });
            if r.error.is_none() {
                all_errors = false;
                first_status.get_or_insert(r.status);
            }
        }
        let refs: Vec<&HttpClientResponse> = burst.iter().map(|(_, r)| r).collect();
        let assertions: Vec<AssertResult> = step
            .assertions
            .iter()
            .map(|a| {
                let mut r = evaluate_burst(&refs, a, vars);
                if conc.rounds() > 1 {
                    r.actual = r.actual.map(|v| format!("{v}, round {}", round + 1));
                }
                r
            })
            .collect();
        // Simpan putaran gagal pertama; bila semua lulus, putaran terakhir.
        let failed = assertions.iter().any(|a| !a.passed);
        if chosen.is_none() && (failed || round + 1 == conc.rounds()) {
            chosen = Some(assertions);
        }
        for (_, r) in burst {
            if r.error.is_none() && (200..300).contains(&r.status) {
                successes.push(r);
            }
        }
    }
    result.assertions = chosen.unwrap_or_default();
    result.time_ms = started.elapsed().as_millis();
    result.status = successes
        .first()
        .map(|r| r.status)
        .or(first_status)
        .unwrap_or(0);
    if all_errors {
        result.error = result.instances.iter().find_map(|i| i.error.clone());
        return (result, Vec::new());
    }
    if let Some(first) = successes.first() {
        result.body_preview = crate::repo_scan::truncate_chars(&redact_body(&first.body), 2_000);
        result.response_headers = redact_headers(&first.headers);
    }
    // Extract: respons sukses pertama → `var`, respons sukses ke-n → `var#n`.
    let mut overlays: Vec<HashMap<String, String>> = Vec::new();
    for (n, r) in successes.iter().enumerate() {
        let mut overlay = HashMap::new();
        for ex in step.extract.iter().filter(|e| !e.var.trim().is_empty()) {
            if let Some(v) = read_target(r, &ex.from) {
                if n == 0 {
                    vars.insert(ex.var.clone(), v.clone());
                    result
                        .extracted
                        .push((ex.var.clone(), display_var_value(&ex.var, &v)));
                }
                vars.insert(format!("{}#{}", ex.var, n + 1), v.clone());
                overlay.insert(ex.var.clone(), v);
            }
        }
        overlays.push(overlay);
    }
    for ex in step.extract.iter().filter(|e| !e.var.trim().is_empty()) {
        if !vars.contains_key(&ex.var) {
            result.assertions.push(AssertResult {
                assertion: Assertion {
                    target: ex.from.clone(),
                    op: AssertOp::Exists,
                    value: format!("(extract {{{{{}}}}})", ex.var),
                },
                passed: false,
                actual: None,
            });
        }
    }
    (result, overlays)
}

/// Pengirim dan akses database untuk satu run.
pub struct RunIo<'a> {
    pub send: &'a dyn Fn(RequestSpec) -> HttpClientResponse,
    /// Pengirim yang aman dipakai banyak thread untuk burst serentak. `None`
    /// = burst dikirim berurutan.
    pub send_parallel: Option<&'a (dyn Fn(RequestSpec) -> HttpClientResponse + Sync)>,
    /// Akses database untuk langkah DB; `None` = langkah DB gagal (Error).
    pub db: Option<&'a dyn DbExecutor>,
}

impl<'a> RunIo<'a> {
    pub fn http(send: &'a dyn Fn(RequestSpec) -> HttpClientResponse) -> Self {
        Self {
            send,
            send_parallel: None,
            db: None,
        }
    }
}

/// Jalankan satu langkah sesuai jenisnya. Mengembalikan overlay variabel per
/// instance untuk langkah concurrent (dipakai cleanup per instance).
fn exec_step(
    i: usize,
    step: &HttpTestStep,
    vars: &mut HashMap<String, String>,
    io: &RunIo,
    cancel: &AtomicBool,
) -> (StepResult, Vec<HashMap<String, String>>) {
    if step.is_db() {
        return (run_db_step(i, step, vars, io.db, cancel), Vec::new());
    }
    if let Some(c) = step.concurrent() {
        return run_concurrent_step(i, step, c, vars, io, cancel);
    }
    (run_step(i, step, vars, io.send), Vec::new())
}

/// Variabel awal satu run: variabel environment (`base`), ditimpa variabel
/// suite yang tidak kosong, plus `run_id` unik.
pub fn initial_vars_with(
    suite: &HttpTestSuite,
    base: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut vars = base.clone();
    for (k, v) in &suite.variables {
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        // Variabel suite kosong (mis. token) tidak menimpa nilai environment.
        if v.is_empty() && vars.contains_key(k) {
            continue;
        }
        vars.insert(k.to_string(), v.clone());
    }
    vars.entry("run_id".to_string())
        .or_insert_with(|| chrono::Utc::now().timestamp_millis().to_string());
    vars
}

/// Variabel awal satu run tanpa environment project.
pub fn initial_vars(suite: &HttpTestSuite) -> HashMap<String, String> {
    initial_vars_with(suite, &HashMap::new())
}

/// Variabel yang belum terisi untuk langkah `step` dengan nilai `vars` saat ini.
fn missing_vars(step: &HttpTestStep, vars: &HashMap<String, String>) -> Vec<String> {
    step_inputs(step)
        .into_iter()
        .filter(|v| !vars.contains_key(v))
        .collect()
}

/// Variabel yang diambil dari cookie: nama variabel → nama cookie.
pub fn cookie_vars(suite: &HttpTestSuite) -> HashMap<String, String> {
    suite
        .steps
        .iter()
        .flat_map(|s| s.extract.iter())
        .filter_map(|e| {
            let name = e.from.trim().strip_prefix("cookie:")?.trim();
            (!name.is_empty() && !e.var.trim().is_empty())
                .then(|| (e.var.trim().to_string(), name.to_string()))
        })
        .collect()
}

/// API yang membaca token dari cookie: langkah dengan auth Bearer `{{v}}`,
/// di mana `v` diambil dari `cookie:<nama>`, juga mengirim header
/// `Cookie: <nama>={{v}}`. Langkah tanpa auth (uji negatif) tidak disentuh.
pub fn with_cookie_auth<'a>(
    step: &'a HttpTestStep,
    cookies: &HashMap<String, String>,
) -> std::borrow::Cow<'a, HttpTestStep> {
    use std::borrow::Cow;
    let r = &step.request;
    if !matches!(
        r.auth_type,
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer
    ) {
        return Cow::Borrowed(step);
    }
    let token = r.bearer_token.trim();
    let Some(var) = token
        .strip_prefix("{{")
        .and_then(|t| t.strip_suffix("}}"))
        .map(str::trim)
    else {
        return Cow::Borrowed(step);
    };
    let Some(cookie) = cookies.get(var) else {
        return Cow::Borrowed(step);
    };
    if r.headers
        .iter()
        .any(|(k, _, on)| *on && k.eq_ignore_ascii_case("cookie"))
    {
        return Cow::Borrowed(step);
    }
    let mut s = step.clone();
    s.request
        .headers
        .push(("Cookie".into(), format!("{cookie}={{{{{var}}}}}"), true));
    Cow::Owned(s)
}

/// Pilihan run: `include` = langkah yang ikut (lihat [`select_for_groups`]),
/// `None` = semua.
#[derive(Debug, Clone, Default)]
pub struct RunScope {
    pub include: Option<Vec<bool>>,
    /// Nama group yang dipilih (untuk laporan); kosong = semua.
    pub groups: Vec<String>,
    /// Langkah DB boleh membaca koneksi bertanda Production.
    pub allow_production: bool,
}

impl RunScope {
    pub fn groups(suite: &HttpTestSuite, groups: Vec<String>) -> Self {
        if groups.is_empty() {
            return Self::default();
        }
        Self {
            include: Some(select_for_groups(suite, &groups)),
            groups,
            allow_production: false,
        }
    }

    /// Ada langkah DB aktif yang ikut run ini.
    pub fn needs_db(&self, suite: &HttpTestSuite) -> bool {
        suite
            .steps
            .iter()
            .enumerate()
            .any(|(i, s)| s.enabled && s.is_db() && self.includes(i))
    }

    pub fn includes(&self, i: usize) -> bool {
        self.include
            .as_ref()
            .is_none_or(|m| m.get(i).copied().unwrap_or(false))
    }
}

/// Jalankan suite secara sinkron. Urutan: Setup → Test (per group) → Cleanup.
///
/// - Langkah dengan variabel yang belum terisi (mis. id dari langkah gagal)
///   ditandai Blocked dan tidak dikirim.
/// - Setup gagal memblokir langkah berikutnya; `stop_on_failure` memblokir
///   langkah Test berikutnya setelah satu kegagalan.
/// - `cancel` menghentikan Setup/Test, tetapi Cleanup **tetap** dijalankan
///   untuk langkah yang sudah membuat data. Hanya `abort` yang menghentikannya.
/// - Data yang tidak berhasil dibersihkan dicatat sebagai [`Residue`].
pub fn run_suite(
    suite: &HttpTestSuite,
    scope: &RunScope,
    vars: HashMap<String, String>,
    send: &dyn Fn(RequestSpec) -> HttpClientResponse,
    cancel: &AtomicBool,
    abort: &AtomicBool,
    emit: &mut dyn FnMut(TestEvent),
) -> RunSummary {
    run_suite_with(suite, scope, vars, &RunIo::http(send), cancel, abort, emit)
}

/// Seperti [`run_suite`] dengan pengirim paralel (langkah concurrent) dan
/// akses database (langkah DB).
pub fn run_suite_with(
    suite: &HttpTestSuite,
    scope: &RunScope,
    mut vars: HashMap<String, String>,
    io: &RunIo,
    cancel: &AtomicBool,
    abort: &AtomicBool,
    emit: &mut dyn FnMut(TestEvent),
) -> RunSummary {
    let deps = dependencies(suite);
    // Overlay variabel per instance sukses dari langkah concurrent.
    let mut overlays: HashMap<usize, Vec<HashMap<String, String>>> = HashMap::new();
    let cookies = cookie_vars(suite);
    let order: Vec<usize> = suite
        .execution_order()
        .into_iter()
        .filter(|&i| scope.includes(i))
        .collect();
    let mut results: Vec<Option<StepResult>> = vec![None; suite.steps.len()];
    let mut halt: Option<String> = None;
    let mut cancelled = false;
    let finish = |i: usize,
                  r: StepResult,
                  results: &mut Vec<Option<StepResult>>,
                  emit: &mut dyn FnMut(TestEvent)| {
        emit(TestEvent::StepFinished(Box::new(r.clone())));
        results[i] = Some(r);
    };

    for &i in order
        .iter()
        .filter(|&&i| suite.steps[i].phase != StepPhase::Cleanup)
    {
        let step = &suite.steps[i];
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
        }
        let r = if !step.enabled {
            StepResult::not_run(i, step, false, "Disabled".into())
        } else if cancelled {
            StepResult::not_run(i, step, false, "Run stopped before this step".into())
        } else if let Some(reason) = &halt {
            StepResult::not_run(i, step, true, reason.clone())
        } else {
            let missing = missing_vars(step, &vars);
            if !missing.is_empty() {
                let mut r = StepResult::not_run(
                    i,
                    step,
                    true,
                    format!("Missing variable(s): {}", missing.join(", ")),
                );
                r.unresolved = missing;
                r
            } else {
                emit(TestEvent::StepStarted(i));
                let (r, ov) =
                    exec_step(i, &with_cookie_auth(step, &cookies), &mut vars, io, cancel);
                if ov.len() > 1 {
                    overlays.insert(i, ov);
                }
                r
            }
        };
        if !r.skipped && !r.passed() && halt.is_none() {
            if step.phase == StepPhase::Setup {
                halt = Some(format!("Setup step '{}' did not pass", step.name));
            } else if suite.stop_on_failure {
                halt = Some(format!("Stopped after failure of '{}'", step.name));
            }
        }
        log::info!(
            "[HTTP_TESTS] {} step '{}' -> {} {}",
            suite.name,
            step.name,
            r.status,
            r.outcome().label()
        );
        finish(i, r, &mut results, emit);
    }

    // Fase cleanup: selalu dijalankan kecuali abort.
    emit(TestEvent::CleanupStarted);
    let any_mutation_ran = results
        .iter()
        .flatten()
        .any(|r| r.created() && suite.steps.get(r.index).is_some_and(HttpTestStep::mutates));
    let mut residue: Vec<Residue> = Vec::new();
    let residue_for = |t: usize, reason: String, results: &[Option<StepResult>]| -> Residue {
        let r = results[t].as_ref();
        Residue {
            step_index: t,
            step_name: suite.steps[t].name.clone(),
            method: suite.steps[t].method_label().to_string(),
            url: r.map(|r| r.url.clone()).unwrap_or_default(),
            ids: r.map(|r| r.extracted.clone()).unwrap_or_default(),
            reason,
        }
    };
    for &i in order
        .iter()
        .filter(|&&i| suite.steps[i].phase == StepPhase::Cleanup)
    {
        let step = &suite.steps[i];
        let target = cleanup_target(suite, &deps, i);
        let needed = match target {
            Some(t) => results[t].as_ref().is_some_and(StepResult::created),
            None => any_mutation_ran,
        };
        let r = if abort.load(Ordering::SeqCst) {
            if needed && let Some(t) = target {
                residue.push(residue_for(
                    t,
                    "Run aborted before cleanup".into(),
                    &results,
                ));
            }
            StepResult::not_run(i, step, true, "Run aborted; cleanup not executed".into())
        } else if !needed {
            StepResult::not_run(
                i,
                step,
                false,
                "Nothing to clean: the data was not created".into(),
            )
        } else if !step.enabled {
            if let Some(t) = target {
                residue.push(residue_for(
                    t,
                    format!("Cleanup step '{}' is disabled", step.name),
                    &results,
                ));
            }
            StepResult::not_run(i, step, false, "Disabled".into())
        } else {
            let missing = missing_vars(step, &vars);
            if !missing.is_empty() {
                if let Some(t) = target {
                    residue.push(residue_for(
                        t,
                        format!(
                            "Cleanup '{}' blocked: missing {}",
                            step.name,
                            missing.join(", ")
                        ),
                        &results,
                    ));
                }
                let mut r = StepResult::not_run(
                    i,
                    step,
                    true,
                    format!("Missing variable(s): {}", missing.join(", ")),
                );
                r.unresolved = missing;
                r
            } else {
                emit(TestEvent::StepStarted(i));
                let sent = with_cookie_auth(step, &cookies);
                // Target concurrent: cleanup dijalankan untuk setiap data yang
                // dibuat burst (variabel extract diganti nilai instance ke-n).
                let per_instance = target.and_then(|t| overlays.get(&t)).cloned();
                let no_retry_cancel = AtomicBool::new(false);
                let run_once = |vars: &mut HashMap<String, String>| {
                    let mut r = exec_step(i, &sent, vars, io, &no_retry_cancel).0;
                    // Satu kali coba ulang untuk gangguan jaringan sesaat.
                    if r.error.is_some() && !abort.load(Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        r = exec_step(i, &sent, vars, io, &no_retry_cancel).0;
                        r.retries = 1;
                    }
                    r
                };
                let r = match per_instance {
                    None => run_once(&mut vars),
                    Some(list) => {
                        let mut combined: Option<StepResult> = None;
                        for (n, overlay) in list.iter().enumerate() {
                            if abort.load(Ordering::SeqCst) {
                                break;
                            }
                            let mut v = vars.clone();
                            v.extend(overlay.clone());
                            let one = run_once(&mut v);
                            let ok = one.passed();
                            let instance = InstanceResult {
                                round: 1,
                                index: n as u32 + 1,
                                status: one.status,
                                time_ms: one.time_ms,
                                error: one.error.clone(),
                                body_preview: crate::repo_scan::truncate_chars(
                                    &one.body_preview,
                                    300,
                                ),
                            };
                            let c = combined.get_or_insert_with(|| one.clone());
                            c.instances.push(instance);
                            if !ok && c.passed() {
                                // Simpan rincian kegagalan pertama.
                                let keep = std::mem::take(&mut c.instances);
                                *c = one;
                                c.instances = keep;
                            }
                        }
                        combined.unwrap_or_else(|| {
                            StepResult::not_run(i, step, true, "Run aborted".into())
                        })
                    }
                };
                if !r.passed()
                    && let Some(t) = target
                {
                    let why = r
                        .error
                        .clone()
                        .unwrap_or_else(|| format!("HTTP {} / assertion failed", r.status));
                    residue.push(residue_for(
                        t,
                        format!("Cleanup '{}' failed: {why}", step.name),
                        &results,
                    ));
                }
                r
            }
        };
        finish(i, r, &mut results, emit);
    }
    // Data yang dibuat tanpa langkah cleanup sama sekali.
    let covered: std::collections::HashSet<usize> = (0..suite.steps.len())
        .filter_map(|i| cleanup_target(suite, &deps, i))
        .collect();
    for (t, step) in suite.steps.iter().enumerate() {
        if creates_data(step)
            && !covered.contains(&t)
            && results[t].as_ref().is_some_and(StepResult::created)
        {
            residue.push(residue_for(
                t,
                "No cleanup step removes this data".into(),
                &results,
            ));
        }
    }
    residue.sort_by_key(|r| r.step_index);
    residue.dedup_by_key(|r| r.step_index);

    RunSummary {
        cancelled,
        aborted: abort.load(Ordering::SeqCst),
        results: order.iter().filter_map(|&i| results[i].clone()).collect(),
        residue,
        preflight_error: None,
    }
}

/// Jalankan suite dengan pengirim HTTP nyata dan, bila ada langkah DB, sesi
/// database headless (dibuka sekali per run). Dipakai GUI, CLI, validasi,
/// dan MCP.
pub fn run_with_session(
    suite: &HttpTestSuite,
    scope: &RunScope,
    vars: HashMap<String, String>,
    cancel: &AtomicBool,
    abort: &AtomicBool,
    emit: &mut dyn FnMut(TestEvent),
) -> RunSummary {
    run_with_session_as(suite, scope, vars, None, cancel, abort, emit)
}

/// Seperti [`run_with_session`]; `agent_client` = nama klien MCP sehingga
/// koneksi yang diblokir untuk agent ditolak.
pub fn run_with_session_as(
    suite: &HttpTestSuite,
    scope: &RunScope,
    vars: HashMap<String, String>,
    agent_client: Option<&str>,
    cancel: &AtomicBool,
    abort: &AtomicBool,
    emit: &mut dyn FnMut(TestEvent),
) -> RunSummary {
    let send = crate::http_send::send_blocking;
    let executor = if scope.needs_db(suite) {
        match crate::http_test_db::SessionDbExecutor::open(scope.allow_production) {
            Ok(e) => Some(match agent_client {
                Some(c) => e.for_agent(c),
                None => e,
            }),
            Err(e) => {
                log::warn!("[HTTP_TESTS] database steps unavailable: {e}");
                None
            }
        }
    } else {
        None
    };
    if scope.needs_db(suite) {
        let checked = match &executor {
            None => Err("Database steps need access to Tabular's saved connections, which could not be opened".to_string()),
            Some(e) => db_preflight(suite, scope, &vars, e),
        };
        if let Err(e) = checked {
            return preflight_failed(suite, scope, e);
        }
    }
    let io = RunIo {
        send: &send,
        send_parallel: Some(&send),
        db: executor.as_ref().map(|e| e as &dyn DbExecutor),
    };
    run_suite_with(suite, scope, vars, &io, cancel, abort, emit)
}

/// Koneksi yang dirujuk langkah DB ada, boleh dibaca (Production), dan bisa
/// dijangkau (`SELECT 1`) sebelum langkah pertama dikirim.
pub fn db_preflight(
    suite: &HttpTestSuite,
    scope: &RunScope,
    vars: &HashMap<String, String>,
    db: &dyn DbExecutor,
) -> Result<(), String> {
    let mut seen: Vec<String> = Vec::new();
    for (i, step) in suite.steps.iter().enumerate() {
        if !step.enabled || !step.is_db() || !scope.includes(i) {
            continue;
        }
        let Some(cfg) = &step.db else { continue };
        // Variabel yang belum ada ditandai Blocked di langkahnya sendiri.
        let Some(value) = vars.get(cfg.connection_var.trim()) else {
            continue;
        };
        if seen.contains(value) {
            continue;
        }
        seen.push(value.clone());
        let probe = crate::http_test_db::DbRequest {
            connection: value.clone(),
            database: None,
            sql: "SELECT 1".into(),
            vars: HashMap::new(),
        };
        db.query(&probe).map_err(|e| {
            format!(
                "Database for {{{{{}}}}} is not usable: {e}",
                cfg.connection_var.trim()
            )
        })?;
    }
    Ok(())
}

/// Jalankan suite di thread background dengan variabel awal `vars`
/// (lihat [`initial_vars_with`]).
pub fn spawn_run(
    suite: HttpTestSuite,
    scope: RunScope,
    vars: HashMap<String, String>,
) -> TestRunHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let abort = Arc::new(AtomicBool::new(false));
    let (c, a) = (cancel.clone(), abort.clone());
    std::thread::spawn(move || {
        let mut emit = |e: TestEvent| {
            let _ = tx.send(e);
        };
        let summary = match preflight(&suite, &scope, &vars) {
            Ok(()) => run_with_session(&suite, &scope, vars, &c, &a, &mut emit),
            Err(e) => preflight_failed(&suite, &scope, e),
        };
        let _ = tx.send(TestEvent::Finished(Box::new(summary)));
    });
    TestRunHandle { rx, cancel, abort }
}

// ─── Pre-flight ─────────────────────────────────────────────────────────────

/// Origin `(scheme, host, port)` unik dari URL langkah yang ikut run, setelah
/// variabel diganti. URL yang masih berisi `{{…}}` dilewati.
pub fn run_origins(
    suite: &HttpTestSuite,
    scope: &RunScope,
    vars: &HashMap<String, String>,
) -> Vec<(String, String, u16)> {
    let mut out: Vec<(String, String, u16)> = Vec::new();
    for (i, step) in suite.steps.iter().enumerate() {
        if !step.enabled || !scope.includes(i) {
            continue;
        }
        let url = substitute(&step.request.url, vars);
        let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
            continue;
        };
        let Some(host) = parsed.host_str() else {
            continue;
        };
        let port = parsed.port_or_known_default().unwrap_or(80);
        let origin = (parsed.scheme().to_string(), host.to_string(), port);
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    out
}

/// Pastikan semua server tujuan bisa dihubungi (koneksi TCP, 3 detik).
pub fn preflight(
    suite: &HttpTestSuite,
    scope: &RunScope,
    vars: &HashMap<String, String>,
) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    let mut down = Vec::new();
    for (scheme, host, port) in run_origins(suite, scope, vars) {
        let label = format!("{scheme}://{host}:{port}");
        let addrs: Vec<std::net::SocketAddr> = match (host.as_str(), port).to_socket_addrs() {
            Ok(a) => a.collect(),
            Err(e) => {
                down.push(format!("{label} (cannot resolve host: {e})"));
                continue;
            }
        };
        let ok = addrs.iter().any(|a| {
            std::net::TcpStream::connect_timeout(a, std::time::Duration::from_secs(3)).is_ok()
        });
        if !ok {
            down.push(format!("{label} (connection refused or timed out)"));
        }
    }
    if down.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "API server not reachable: {}. Start the API, or check the base URL variables and the project environment.",
            down.join(", ")
        ))
    }
}

/// Ringkasan run yang tidak dijalankan karena pre-flight gagal.
pub fn preflight_failed(suite: &HttpTestSuite, scope: &RunScope, error: String) -> RunSummary {
    let results = suite
        .execution_order()
        .into_iter()
        .filter(|&i| scope.includes(i))
        .map(|i| StepResult::not_run(i, &suite.steps[i], true, error.clone()))
        .collect();
    log::warn!("[HTTP_TESTS] {}: {error}", suite.name);
    RunSummary {
        results,
        preflight_error: Some(error),
        ..Default::default()
    }
}

// ─── Kredensial akun uji ────────────────────────────────────────────────────

/// Variabel kredensial untuk akun yang dibuat suite sendiri (bukan rahasia
/// nyata), mis. `test_password`.
pub fn is_test_credential(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("password")
        && ["test", "new", "dummy", "temp", "sample", "qa"]
            .iter()
            .any(|p| n.contains(p))
}

/// Nilai kuat acak untuk kredensial akun uji.
pub fn generate_test_password() -> String {
    use rand::RngExt;
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut bytes = [0u8; 10];
    rand::rng().fill(&mut bytes);
    let tail: String = bytes
        .iter()
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect();
    format!("Tst!{tail}9a")
}

/// Isi variabel kredensial akun uji yang masih kosong. Mengembalikan namanya.
pub fn fill_test_credentials(vars: &mut [(String, String)]) -> Vec<String> {
    let mut filled = Vec::new();
    for (k, v) in vars.iter_mut() {
        if v.is_empty() && is_test_credential(k) {
            *v = generate_test_password();
            filled.push(k.clone());
        }
    }
    filled
}

// ─── Riwayat run ────────────────────────────────────────────────────────────

/// Pemeriksaan sisa data uji langsung di database setelah cleanup.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbResidueCheck {
    pub connection: String,
    pub table: String,
    /// Baris yang masih memuat `run_id`; `None` bila tidak bisa diperiksa.
    pub rows: Option<u64>,
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Catatan satu run untuk laporan dan bukti validasi.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TestRun {
    pub run_id: String,
    pub suite_id: String,
    pub suite_name: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    #[serde(default)]
    pub environment: Option<String>,
    /// Group yang dipilih; kosong = seluruh suite.
    #[serde(default)]
    pub groups: Vec<String>,
    pub started_at: String,
    pub finished_at: String,
    #[serde(default)]
    pub duration_ms: u128,
    /// User OS yang menjalankan run.
    #[serde(default)]
    pub operator: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub app_version: String,
    /// SHA-256 definisi suite saat dijalankan (identitas konfigurasi yang diuji).
    #[serde(default)]
    pub suite_fingerprint: String,
    /// Origin server yang dihubungi.
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub aborted: bool,
    /// Hasil per langkah dalam urutan eksekusi.
    pub results: Vec<StepResult>,
    #[serde(default)]
    pub residue: Vec<Residue>,
    #[serde(default)]
    pub db_checks: Vec<DbResidueCheck>,
    #[serde(default)]
    pub pass_threshold: f64,
    /// Server tidak terjangkau; run dibatalkan sebelum langkah pertama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight_error: Option<String>,
}

/// Batas jumlah run tersimpan per suite.
const MAX_RUNS_PER_SUITE: usize = 50;

fn runs_dir() -> std::path::PathBuf {
    suites_dir().join("runs")
}

/// SHA-256 (hex) dari definisi suite.
pub fn suite_fingerprint(suite: &HttpTestSuite) -> String {
    use sha2::{Digest, Sha256};
    let json = serde_json::to_vec(&(&suite.steps, &suite.variables)).unwrap_or_default();
    hex::encode(Sha256::digest(&json))
}

/// Origin unik (`scheme://host`) dari URL hasil run.
pub fn run_targets(results: &[StepResult]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in results {
        let Some((scheme, rest)) = r.url.split_once("://") else {
            continue;
        };
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() || host.contains("{{") {
            continue;
        }
        let o = format!("{scheme}://{host}");
        if !out.contains(&o) {
            out.push(o);
        }
    }
    out
}

/// User OS dan nama host untuk jejak audit.
pub fn operator_and_host() -> (String, String) {
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(system_hostname);
    (user, host)
}

/// Nama host dari perintah `hostname` (macOS tidak mengekspor `HOSTNAME`).
#[cfg(not(target_os = "ios"))]
fn system_hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "ios")]
fn system_hostname() -> String {
    String::new()
}

impl TestRun {
    /// Bangun catatan run dari ringkasan runner.
    pub fn from_summary(
        suite: &HttpTestSuite,
        scope: &RunScope,
        run_id: String,
        started: chrono::DateTime<chrono::Utc>,
        summary: RunSummary,
    ) -> Self {
        let finished = chrono::Utc::now();
        let (operator, host) = operator_and_host();
        Self {
            run_id,
            suite_id: suite.id.clone(),
            suite_name: suite.name.clone(),
            project_id: suite.project_id.clone(),
            groups: scope.groups.clone(),
            started_at: started.to_rfc3339(),
            finished_at: finished.to_rfc3339(),
            duration_ms: (finished - started).num_milliseconds().max(0) as u128,
            operator,
            host,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            suite_fingerprint: suite_fingerprint(suite),
            targets: run_targets(&summary.results),
            cancelled: summary.cancelled,
            aborted: summary.aborted,
            results: summary.results,
            residue: summary.residue,
            preflight_error: summary.preflight_error,
            pass_threshold: suite.pass_threshold(),
            ..Default::default()
        }
    }
}

pub fn save_run(run: &TestRun) -> Result<(), String> {
    save_run_in(&runs_dir(), run)
}

pub fn save_run_in(dir: &std::path::Path, run: &TestRun) -> Result<(), String> {
    let dir = dir.join(safe_file_id(&run.suite_id));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.json", safe_file_id(&run.run_id)));
    let json = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    crate::directory::write_file_atomically(&path, json.as_bytes())
        .map_err(|e| format!("Could not save test run: {e}"))?;
    // Buang run tertua di atas batas (nama file = run_id berbasis waktu).
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|it| it.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().and_then(|e| e.to_str()) == Some("json"));
    if files.len() > MAX_RUNS_PER_SUITE {
        files.sort();
        for old in &files[..files.len() - MAX_RUNS_PER_SUITE] {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

/// Run suite, terbaru lebih dulu, maksimal `limit`.
pub fn load_runs(suite_id: &str, limit: usize) -> Vec<TestRun> {
    load_runs_in(&runs_dir(), suite_id, limit)
}

pub fn load_runs_in(dir: &std::path::Path, suite_id: &str, limit: usize) -> Vec<TestRun> {
    let Ok(entries) = std::fs::read_dir(dir.join(safe_file_id(suite_id))) else {
        return Vec::new();
    };
    let mut runs: Vec<TestRun> = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .filter_map(|e| {
            let bytes = std::fs::read(e.path()).ok()?;
            serde_json::from_slice::<TestRun>(&bytes)
                .map_err(|err| log::warn!("[HTTP_TESTS] skip run {}: {err}", e.path().display()))
                .ok()
        })
        .collect();
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    runs.truncate(limit);
    runs
}

// ─── Tab HTTP client untuk satu langkah ─────────────────────────────────────

/// Penanda bahwa tab HTTP client sedang mengedit satu langkah suite. Simpan
/// (Cmd+S) menulis request kembali ke langkah, bukan ke collection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestStepRef {
    pub suite_id: String,
    pub step_id: String,
    /// Label untuk banner, mis. "Procure to pay › Create PO".
    pub label: String,
    /// Variabel run (suite + environment + hasil run terakhir) untuk mengirim
    /// request dari tab tersebut.
    pub vars: HashMap<String, String>,
}

// ─── Verifikasi sisa data di database ───────────────────────────────────────

/// Koneksi database yang diperiksa (id lokal + nama untuk laporan).
#[derive(Debug, Clone)]
pub struct DbCheckTarget {
    pub connection_id: i64,
    pub connection_name: String,
}

/// Batas tabel dan kolom per pemeriksaan agar query tetap ringan.
const DB_CHECK_MAX_TABLES: usize = 40;
const DB_CHECK_MAX_COLUMNS: usize = 24;

fn quote_ident(kind: &str, name: &str) -> String {
    use crate::models::enums::DatabaseType;
    let db = match kind {
        "MySQL" => DatabaseType::MySQL,
        "MsSQL" => DatabaseType::MsSQL,
        _ => DatabaseType::PostgreSQL,
    };
    crate::schema_objects::sql::quote_ident(&db, name)
}

/// `schema.table` → identifier berkutip per bagian.
fn quote_table(kind: &str, table: &str) -> String {
    table
        .split('.')
        .map(|p| quote_ident(kind, p))
        .collect::<Vec<_>>()
        .join(".")
}

fn table_matches(cached: &str, wanted: &str) -> bool {
    let c = cached.to_ascii_lowercase();
    let w = wanted.trim().to_ascii_lowercase();
    c == w || c.rsplit('.').next() == Some(w.as_str()) || w.rsplit('.').next() == Some(c.as_str())
}

/// SQL hitung baris yang kolom teksnya memuat `run_id`.
pub fn residue_count_sql(
    kind: &str,
    table: &str,
    columns: &[String],
    run_id: &str,
) -> Option<String> {
    if columns.is_empty() || run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    let cond: Vec<String> = columns
        .iter()
        .map(|c| format!("{} LIKE '%{run_id}%'", quote_ident(kind, c)))
        .collect();
    Some(format!(
        "SELECT COUNT(*) AS n FROM {} WHERE {}",
        quote_table(kind, table),
        cond.join(" OR ")
    ))
}

/// Cari baris yang masih memuat `run_id` di tabel `tables` pada koneksi
/// `targets`. Hanya query baca (lewat gerbang read-only agent). Tabel yang
/// tidak ada di cache skema koneksi tersebut dilewati.
pub async fn check_db_residue(
    session: &crate::agent::core::HeadlessSession,
    targets: &[DbCheckTarget],
    tables: &[String],
    run_id: &str,
) -> Vec<DbResidueCheck> {
    let mut out = Vec::new();
    if tables.is_empty() || targets.is_empty() {
        return out;
    }
    let kinds: HashMap<i64, String> = session
        .list_connections()
        .await
        .map(|l| l.into_iter().map(|c| (c.id, c.kind)).collect())
        .unwrap_or_default();
    for t in targets {
        let Some(kind) = kinds.get(&t.connection_id).cloned() else {
            continue;
        };
        if !matches!(kind.as_str(), "MySQL" | "PostgreSQL" | "SQLite" | "MsSQL") {
            continue;
        }
        let cached: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT database_name, table_name FROM table_cache \
             WHERE connection_id = ? AND table_type = 'table'",
        )
        .bind(t.connection_id)
        .fetch_all(session.cache_pool())
        .await
        .unwrap_or_default();
        for (db, table) in cached
            .into_iter()
            .filter(|(_, name)| tables.iter().any(|w| table_matches(name, w)))
            .take(DB_CHECK_MAX_TABLES)
        {
            let cols: Vec<(String, String)> = sqlx::query_as(
                "SELECT column_name, data_type FROM column_cache \
                 WHERE connection_id = ? AND database_name = ? AND table_name = ? \
                 ORDER BY ordinal_position",
            )
            .bind(t.connection_id)
            .bind(&db)
            .bind(&table)
            .fetch_all(session.cache_pool())
            .await
            .unwrap_or_default();
            let text_cols: Vec<String> = cols
                .into_iter()
                .filter(|(_, ty)| {
                    let ty = ty.to_ascii_lowercase();
                    ["char", "text", "string", "clob"]
                        .iter()
                        .any(|k| ty.contains(k))
                })
                .map(|(c, _)| c)
                .take(DB_CHECK_MAX_COLUMNS)
                .collect();
            let mut check = DbResidueCheck {
                connection: t.connection_name.clone(),
                table: table.clone(),
                columns: text_cols.clone(),
                ..Default::default()
            };
            match residue_count_sql(&kind, &table, &text_cols, run_id) {
                None => check.error = Some("No text columns to search".into()),
                Some(sql) => match session
                    .run_query(t.connection_id, &sql, Some(&db), Some(1))
                    .await
                {
                    Ok(r) => {
                        check.rows = r
                            .rows
                            .first()
                            .and_then(|row| row.first())
                            .and_then(|v| v.trim().parse::<u64>().ok());
                        if check.rows.is_none() {
                            check.error = Some("Unexpected result".into());
                        }
                    }
                    Err(e) => check.error = Some(e.to_string()),
                },
            }
            out.push(check);
        }
    }
    out
}

/// Tabel yang disentuh langkah pengubah data yang benar-benar dijalankan.
pub fn touched_tables(suite: &HttpTestSuite, results: &[StepResult]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in results.iter().filter(|r| r.executed()) {
        let Some(step) = suite.steps.iter().find(|s| s.id == r.step_id) else {
            continue;
        };
        if !is_mutating(&step.request.method) {
            continue;
        }
        for t in &step.request.tables {
            if !out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                out.push(t.clone());
            }
        }
    }
    out
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

/// Aturan bersama semua prompt integration test.
const TEST_RULES: &str = "Rules:\n\
- Order matters: each step may only use variables provided by an earlier step's `extract` \
  (ids, tokens) or by `variables`. Chain the steps so data flows from one to the next.\n\
- Give every step a short unique `id` (s1, s2, ...), a `phase` and, when the system has \
  business areas, a `group` (for example an ERP: \"Purchasing\", \"Inventory\", \"Plant\", \
  \"Sales\", \"Finance\"). Groups run in the order they first appear.\n\
- `phase` is `setup` (login, master data the tests need), `test` (the behaviour under test, \
  including negative cases such as validation errors, not found and unauthorized) or \
  `cleanup`.\n\
- Every step that creates data MUST have a `cleanup` step that removes it, with \
  `cleanup_for` set to the id of the creating step. Put cleanup steps last, in reverse order \
  of creation (remove dependent records first). Test data must never be left behind.\n\
- Make created data recognisable and unique: include `{{run_id}}` in names, codes and \
  emails (for example \"PO-TEST-{{run_id}}\").\n\
- Use `{{base_url_…}}` variables from the catalog for every URL; never hardcode hosts.\n\
- Auth tokens are variables such as `{{token}}`; add them to `variables` with an empty value \
  unless a login step extracts them.\n\
- Some APIs return the auth token in a cookie (`Set-Cookie`). Extract it with \
  `cookie:<name>`; steps with auth `bearer` and `{{token}}` then also send it as a cookie.\n\
- Assertion and extract `target` is `status`, `time_ms`, `body`, `header:<Name>`, \
  `cookie:<name>` or `json:$.path.to[0].field`. `op` is equals, not_equals, contains, not_contains, exists, \
  not_exists, less_than, greater_than or matches (regex). Every step needs at least one \
  assertion.\n\
- `request_id` refers to the catalog id the step is based on, when there is one.\n\
- Bugs where the HTTP response looks right but the data is wrong need two more tools:\n\
  * Concurrent step: add `\"concurrency\":{\"count\":5,\"barrier\":true,\"rounds\":3}` to send \
    the same request N times at once (double submit, race conditions; count 2..50). Its \
    assertions run over all responses: `count:status=2xx` (also `count:status=409`, \
    `count:json:$.status=success`), `any:<target>`, `all:<target>`; a plain target means all.\n\
  * Database step: `{\"id\":\"s5\",\"name\":\"Balance updated\",\"phase\":\"test\",\
    \"kind\":\"db_query\",\"db\":{\"connection_var\":\"db_main\",\"sql\":\"SELECT qty FROM \
    stocks WHERE id = {{stock_id}}\",\"wait_until_ms\":5000},\"assert\":[{\"target\":\"scalar\",\
    \"op\":\"approx\",\"value\":\"{{qty_before}} + {{qty_in}}\"}]}`. One SELECT only; \
    `{{x}}` values are bound as parameters (do not quote them for numbers). Targets: \
    `row_count`, `scalar`, `cell:<row>.<column>`, `column:<column>`; `extract` works the same. \
    `wait_until_ms` polls until the assertions pass (async writes). Values may use simple \
    arithmetic with variables. `connection_var` names a project environment variable holding \
    the connection; add it to `variables` with an empty value.\n\
- Never include real secrets.\n";

const STEP_EXAMPLE: &str = "{\"id\":\"s2\",\"name\":\"Create purchase order\",\"phase\":\"test\",\
\"group\":\"Purchasing\",\"request_id\":\"sr_1\",\"method\":\"POST\",\
\"url\":\"{{base_url_erp}}/purchase-orders\",\"headers\":{\"Content-Type\":\"application/json\"},\
\"query\":{},\"auth\":\"bearer\",\"body\":{\"number\":\"PO-TEST-{{run_id}}\",\
\"vendor_id\":\"{{vendor_id}}\"},\"extract\":{\"po_id\":\"json:$.id\"},\
\"assert\":[{\"target\":\"status\",\"op\":\"equals\",\"value\":\"201\"},\
{\"target\":\"json:$.id\",\"op\":\"exists\"}]},\
{\"id\":\"s9\",\"name\":\"Delete purchase order\",\"phase\":\"cleanup\",\"group\":\"Purchasing\",\
\"cleanup_for\":\"s2\",\"method\":\"DELETE\",\"url\":\"{{base_url_erp}}/purchase-orders/{{po_id}}\",\
\"auth\":\"bearer\",\"assert\":[{\"target\":\"status\",\"op\":\"less_than\",\"value\":\"300\"}]}";

pub fn build_test_prompts(catalogs: &[FolderCatalog], instructions: &str) -> (String, String) {
    let system = format!(
        "You design end-to-end integration tests for HTTP APIs. The APIs may come from several \
         services; design flows that cross them when the data connects (ids created by one \
         service used by another, shared tables).\n{TEST_RULES}\
         Reply with ONLY one JSON object, no prose and no markdown fence:\n\
         {{\"suites\":[{{\"name\":\"Procure to pay\",\"description\":\"…\",\"variables\":\
         {{\"token\":\"\"}},\"steps\":[{STEP_EXAMPLE}]}}]}}\n\
         The variable `{{{{run_id}}}}` is always available and unique per run.\n"
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

/// Mode generate langkah untuk suite yang sudah ada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepGenMode {
    /// Tambah langkah baru (mis. satu group) di belakang langkah yang ada.
    Append,
    /// Buat langkah cleanup untuk data yang belum dibersihkan.
    Cleanup,
}

/// Ringkasan suite untuk prompt: langkah, variabel yang tersedia, group.
pub fn suite_outline(suite: &HttpTestSuite) -> String {
    let mut out = format!("Suite \"{}\"\n", suite.name);
    let groups = suite.group_names();
    if !groups.is_empty() {
        out.push_str(&format!("Groups in order: {}\n", groups.join(", ")));
    }
    let vars: Vec<&str> = suite.variables.iter().map(|(k, _)| k.as_str()).collect();
    if !vars.is_empty() {
        out.push_str(&format!("Variables: {}\n", vars.join(", ")));
    }
    for i in suite.execution_order() {
        let s = &suite.steps[i];
        out.push_str(&format!(
            "- id={} [{}{}] {} {} — {}",
            s.id,
            s.phase.label().to_ascii_lowercase(),
            if s.group.is_empty() {
                String::new()
            } else {
                format!(", group {}", s.group)
            },
            s.method_label(),
            match (&s.db, s.is_db()) {
                (Some(db), true) => format!(
                    "{{{{{}}}}} {}",
                    db.connection_var,
                    db.sql.split_whitespace().collect::<Vec<_>>().join(" ")
                ),
                _ => s.request.url.clone(),
            },
            s.name
        ));
        if let Some(c) = s.concurrent() {
            out.push_str(&format!(" (concurrent {} x {})", c.count(), c.rounds()));
        }
        let outs = step_outputs(s);
        if !outs.is_empty() {
            out.push_str(&format!(" (extracts {})", outs.join(", ")));
        }
        if let Some(t) = &s.cleanup_for {
            out.push_str(&format!(" (cleans {t})"));
        }
        out.push('\n');
    }
    out
}

/// Prompt untuk menambah langkah ke suite yang ada.
pub fn build_step_prompts(
    catalogs: &[FolderCatalog],
    suite: &HttpTestSuite,
    instructions: &str,
    mode: StepGenMode,
) -> (String, String) {
    let task = match mode {
        StepGenMode::Append => {
            "Add new steps to an existing integration test suite. Reuse variables and ids the \
             existing steps already extract instead of creating the same data again. New step \
             ids must not collide with existing ids."
        }
        StepGenMode::Cleanup => {
            "Add only the missing `cleanup` steps for the existing suite: every step that \
             creates data and has no cleanup yet needs one, linked with `cleanup_for`. Do not \
             repeat existing steps."
        }
    };
    let system = format!(
        "You design end-to-end integration tests for HTTP APIs. {task}\n{TEST_RULES}\
         Reply with ONLY one JSON object, no prose and no markdown fence:\n\
         {{\"variables\":{{}},\"steps\":[{STEP_EXAMPLE}]}}\n\
         The variable `{{{{run_id}}}}` is always available and unique per run.\n"
    );
    let mut user = String::from("Existing suite:\n");
    user.push_str(&suite_outline(suite));
    user.push_str("\nEndpoint catalog:\n");
    user.push_str(&catalog_text(catalogs));
    if !instructions.trim().is_empty() {
        user.push_str("\nRequest: ");
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

/// Urai satu objek langkah JSON (format jawaban AI).
pub fn parse_step_value(v: &Value, catalogs: &[FolderCatalog]) -> Option<HttpTestStep> {
    parse_step(v, catalogs)
}

fn parse_step(v: &Value, catalogs: &[FolderCatalog]) -> Option<HttpTestStep> {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if StepKind::parse(&s("kind")) == StepKind::DbQuery || v.get("db").is_some_and(Value::is_object)
    {
        return parse_db_step(v);
    }
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
    let phase = StepPhase::parse(&s("phase"));
    Some(HttpTestStep {
        id: sanitize_step_id(&s("id")),
        name: if req.name.is_empty() {
            format!("{} {}", req.method.label(), req.url)
        } else {
            req.name.clone()
        },
        phase,
        group: s("group"),
        cleanup_for: Some(sanitize_step_id(&s("cleanup_for")))
            .filter(|x| !x.is_empty() && phase == StepPhase::Cleanup),
        notes: s("notes"),
        request: req,
        extract: parse_extracts(v),
        assertions: parse_assertions(v),
        enabled: true,
        request_id,
        concurrency: parse_concurrency(v),
        ..Default::default()
    })
}

/// `extract` sebagai objek `{var: from}` atau array `[{var, from}]`.
fn parse_extracts(v: &Value) -> Vec<Extract> {
    if let Some(items) = v.get("extract").and_then(Value::as_array) {
        return items
            .iter()
            .filter_map(|e| {
                Some(Extract {
                    var: e.get("var")?.as_str()?.trim().to_string(),
                    from: e.get("from")?.as_str()?.trim().to_string(),
                })
            })
            .collect();
    }
    obj_pairs(v.get("extract"))
        .into_iter()
        .map(|(var, from)| Extract { var, from })
        .collect()
}

fn parse_assertions(v: &Value) -> Vec<Assertion> {
    v.get("assert")
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
        .unwrap_or_default()
}

fn parse_concurrency(v: &Value) -> Option<Concurrency> {
    let c = v.get("concurrency")?;
    let num = |k: &str| c.get(k).and_then(Value::as_u64).map(|n| n as u32);
    let count = num("count").or_else(|| c.as_u64().map(|n| n as u32))?;
    Some(Concurrency {
        count: count.clamp(2, MAX_CONCURRENCY),
        barrier: c.get("barrier").and_then(Value::as_bool).unwrap_or(true),
        rounds: num("rounds").unwrap_or(1).clamp(1, MAX_ROUNDS),
    })
}

/// Langkah DB dari jawaban AI: `{"kind":"db_query","db":{...}}`, atau
/// `sql`/`connection_var` langsung di langkah.
fn parse_db_step(v: &Value) -> Option<HttpTestStep> {
    let s = |o: &Value, k: &str| {
        o.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let db = v.get("db").unwrap_or(v);
    let sql = s(db, "sql");
    if sql.is_empty() {
        return None;
    }
    let connection_var = Some(s(db, "connection_var"))
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "db_main".into());
    let phase = StepPhase::parse(&s(v, "phase"));
    let name = s(v, "name");
    Some(HttpTestStep {
        id: sanitize_step_id(&s(v, "id")),
        name: if name.is_empty() {
            "Check database".into()
        } else {
            name
        },
        phase,
        group: s(v, "group"),
        notes: s(v, "notes"),
        kind: StepKind::DbQuery,
        db: Some(DbQueryStep {
            connection_var: connection_var.trim_matches(['{', '}']).to_string(),
            database: Some(s(db, "database")).filter(|d| !d.is_empty()),
            sql,
            wait_until_ms: db.get("wait_until_ms").and_then(Value::as_u64),
            poll_ms: db
                .get("poll_ms")
                .and_then(Value::as_u64)
                .unwrap_or_else(crate::http_test_db::default_poll_ms),
        }),
        extract: parse_extracts(v),
        assertions: parse_assertions(v),
        enabled: true,
        ..Default::default()
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
        fill_test_credentials(&mut variables);
        let mut suite = HttpTestSuite {
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
            updated_at: now.clone(),
            ..Default::default()
        };
        suite.ensure_ids();
        suite.normalize_order();
        out.push(suite);
    }
    if out.is_empty() {
        return Err("The AI returned no test steps".into());
    }
    Ok(out)
}

/// Urai jawaban AI berisi `steps` (dan opsional `variables`) untuk suite yang ada.
pub fn parse_steps_reply(
    text: &str,
    catalogs: &[FolderCatalog],
) -> Result<(Vec<HttpTestStep>, Vec<(String, String)>), String> {
    let v: Value = crate::repo_scan::slice_between(text, '{', '}')
        .and_then(|s| serde_json::from_str(s).ok())
        .ok_or_else(|| {
            format!(
                "AI reply is not JSON: {}",
                crate::repo_scan::truncate_chars(text.trim(), 200)
            )
        })?;
    let steps_v = v
        .get("steps")
        .and_then(Value::as_array)
        .or_else(|| {
            v.get("suites")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(|s| s.get("steps"))
                .and_then(Value::as_array)
        })
        .ok_or_else(|| "AI reply has no \"steps\"".to_string())?;
    let steps: Vec<HttpTestStep> = steps_v
        .iter()
        .filter_map(|s| parse_step(s, catalogs))
        .collect();
    if steps.is_empty() {
        return Err("The AI returned no test steps".into());
    }
    let mut vars = obj_pairs(v.get("variables"));
    for (k, val) in vars.iter_mut() {
        if sensitive_var(k) {
            val.clear();
        }
    }
    vars.retain(|(k, _)| k != "run_id");
    fill_test_credentials(&mut vars);
    Ok((steps, vars))
}

/// Gabungkan langkah hasil AI ke suite. Id yang bentrok diganti, rujukan
/// `cleanup_for` ikut dipetakan. Mengembalikan jumlah langkah yang ditambah.
pub fn merge_generated_steps(
    suite: &mut HttpTestSuite,
    steps: Vec<HttpTestStep>,
    vars: Vec<(String, String)>,
) -> usize {
    let mut remap: HashMap<String, String> = HashMap::new();
    let mut added = Vec::new();
    for mut step in steps {
        let wanted = step.id.clone();
        if wanted.is_empty()
            || suite.steps.iter().any(|s| s.id == wanted)
            || added.iter().any(|s: &HttpTestStep| s.id == wanted)
        {
            let mut n = suite.steps.len() + added.len() + 1;
            loop {
                let id = format!("s{n}");
                if !suite.steps.iter().any(|s| s.id == id) && !added.iter().any(|s| s.id == id) {
                    step.id = id;
                    break;
                }
                n += 1;
            }
            if !wanted.is_empty() {
                remap.insert(wanted, step.id.clone());
            }
        }
        added.push(step);
    }
    for step in &mut added {
        if let Some(t) = step.cleanup_for.clone()
            && let Some(new) = remap.get(&t)
        {
            step.cleanup_for = Some(new.clone());
        }
    }
    let count = added.len();
    suite.steps.extend(added);
    for (k, v) in vars {
        if !suite.variables.iter().any(|(x, _)| *x == k) {
            suite.variables.push((k, v));
        }
    }
    suite.ensure_ids();
    suite.normalize_order();
    suite.touch();
    count
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

pub type StepGenEvent = crate::repo_scan::RepoJobEvent<(Vec<HttpTestStep>, Vec<(String, String)>)>;

pub struct StepGenHandle {
    pub rx: mpsc::Receiver<StepGenEvent>,
    cancel: Arc<AtomicBool>,
}

impl StepGenHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Generate langkah tambahan untuk suite yang ada di background.
pub fn spawn_step_generate(
    backend: crate::ai_assistant::ChatBackend,
    backend_label: String,
    catalogs: Vec<FolderCatalog>,
    suite: HttpTestSuite,
    instructions: String,
    mode: StepGenMode,
) -> StepGenHandle {
    use crate::agent::harness::ProgressStatus;
    use crate::repo_scan::{RepoJobEvent, step};
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let label = match mode {
            StepGenMode::Append => format!("Asking {backend_label} for new test steps"),
            StepGenMode::Cleanup => format!("Asking {backend_label} for cleanup steps"),
        };
        let _ = tx.send(step(1, label.clone(), None, ProgressStatus::Active));
        let (system, user) = build_step_prompts(&catalogs, &suite, &instructions, mode);
        let result = crate::repo_scan::ask_ai(
            &backend,
            system,
            user,
            crate::ai_assistant::ChatWorkspace::default(),
            &tx,
            &flag,
        )
        .map_err(|e| e.to_string())
        .and_then(|text| parse_steps_reply(&text, &catalogs))
        .map(|(mut steps, vars)| {
            if mode == StepGenMode::Cleanup {
                steps.retain(|s| s.phase == StepPhase::Cleanup);
            }
            (steps, vars)
        });
        let (status, detail) = match &result {
            Ok((s, _)) => (ProgressStatus::Done, format!("{} step(s)", s.len())),
            Err(e) => (ProgressStatus::Error, e.clone()),
        };
        let _ = tx.send(step(1, label, Some(detail), status));
        if let Err(e) = &result {
            log::warn!("[HTTP_TESTS] step generation failed: {e}");
        }
        let _ = tx.send(RepoJobEvent::Finished(result));
    });
    StepGenHandle { rx, cancel }
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
            ..Default::default()
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

    // ─── Relasi, group, cleanup ─────────────────────────────────────────────

    fn step(
        id: &str,
        group: &str,
        phase: StepPhase,
        method: HttpMethod,
        url: &str,
    ) -> HttpTestStep {
        HttpTestStep {
            id: id.into(),
            name: id.into(),
            group: group.into(),
            phase,
            request: SavedRequest {
                method,
                url: url.into(),
                ..Default::default()
            },
            assertions: vec![Assertion {
                target: "status".into(),
                op: AssertOp::LessThan,
                value: "300".into(),
            }],
            enabled: true,
            ..Default::default()
        }
    }

    fn extract(mut s: HttpTestStep, var: &str) -> HttpTestStep {
        s.extract.push(Extract {
            var: var.into(),
            from: "json:$.id".into(),
        });
        s
    }

    /// ERP mini: login (setup) → Purchasing (vendor, PO) → Plant (receipt
    /// memakai PO) → cleanup.
    fn erp_suite() -> HttpTestSuite {
        let mut s = HttpTestSuite {
            id: "erp".into(),
            name: "ERP".into(),
            groups: vec!["Purchasing".into(), "Plant".into()],
            variables: vec![("base".into(), "http://erp".into())],
            steps: vec![
                {
                    let mut l = step(
                        "login",
                        "",
                        StepPhase::Setup,
                        HttpMethod::POST,
                        "{{base}}/login",
                    );
                    l.extract.push(Extract {
                        var: "token".into(),
                        from: "json:$.token".into(),
                    });
                    l
                },
                extract(
                    step(
                        "vendor",
                        "Purchasing",
                        StepPhase::Test,
                        HttpMethod::POST,
                        "{{base}}/vendors?t={{token}}",
                    ),
                    "vendor_id",
                ),
                extract(
                    step(
                        "po",
                        "Purchasing",
                        StepPhase::Test,
                        HttpMethod::POST,
                        "{{base}}/po?v={{vendor_id}}",
                    ),
                    "po_id",
                ),
                extract(
                    step(
                        "receipt",
                        "Plant",
                        StepPhase::Test,
                        HttpMethod::POST,
                        "{{base}}/receipts?po={{po_id}}",
                    ),
                    "receipt_id",
                ),
                {
                    let mut c = step(
                        "del_receipt",
                        "Plant",
                        StepPhase::Cleanup,
                        HttpMethod::DELETE,
                        "{{base}}/receipts/{{receipt_id}}",
                    );
                    c.cleanup_for = Some("receipt".into());
                    c
                },
                {
                    let mut c = step(
                        "del_po",
                        "Purchasing",
                        StepPhase::Cleanup,
                        HttpMethod::DELETE,
                        "{{base}}/po/{{po_id}}",
                    );
                    c.cleanup_for = Some("po".into());
                    c
                },
                {
                    let mut c = step(
                        "del_vendor",
                        "Purchasing",
                        StepPhase::Cleanup,
                        HttpMethod::DELETE,
                        "{{base}}/vendors/{{vendor_id}}",
                    );
                    c.cleanup_for = Some("vendor".into());
                    c
                },
            ],
            ..Default::default()
        };
        s.ensure_ids();
        s
    }

    fn ok_json(id: &str) -> HttpClientResponse {
        resp(201, &format!(r#"{{"id":"{id}","token":"secret-token"}}"#))
    }

    fn run_with(
        suite: &HttpTestSuite,
        scope: &RunScope,
        send: &dyn Fn(RequestSpec) -> HttpClientResponse,
        cancel_after: Option<usize>,
        abort: bool,
    ) -> (RunSummary, Vec<String>) {
        let cancel = AtomicBool::new(false);
        let abort_flag = AtomicBool::new(abort);
        let mut order = Vec::new();
        let mut done = 0usize;
        let mut emit = |e: TestEvent| {
            if let TestEvent::StepFinished(r) = e {
                order.push(r.step_id.clone());
                done += 1;
                if cancel_after == Some(done) {
                    cancel.store(true, Ordering::SeqCst);
                }
            }
        };
        let s = run_suite(
            suite,
            scope,
            initial_vars(suite),
            send,
            &cancel,
            &abort_flag,
            &mut emit,
        );
        (s, order)
    }

    #[test]
    fn login_that_only_extracts_a_token_does_not_create_data() {
        let suite = erp_suite();
        assert!(!creates_data(&suite.steps[0]));
        assert!(creates_data(&suite.steps[1]));
    }

    #[test]
    fn executes_setup_then_groups_then_cleanup_lifo() {
        let suite = erp_suite();
        let send = |spec: RequestSpec| ok_json(&spec.url.len().to_string());
        let (summary, order) = run_with(&suite, &RunScope::default(), &send, None, false);
        assert_eq!(
            order,
            vec![
                "login",
                "vendor",
                "po",
                "receipt",
                "del_receipt",
                "del_po",
                "del_vendor"
            ]
        );
        assert!(
            summary.results.iter().all(|r| r.passed()),
            "{:?}",
            summary.results
        );
        assert!(summary.residue.is_empty());
        // Token hasil login disamarkan di hasil maupun body.
        let login = &summary.results[0];
        assert_eq!(login.extracted[0].1, crate::http_ai::REDACTED);
        assert!(!login.body_preview.contains("secret-token"));
    }

    #[test]
    fn failed_step_blocks_dependents_but_cleanup_still_runs() {
        let suite = erp_suite();
        // PO gagal (500) → receipt diblokir; vendor tetap dibersihkan.
        let send = |spec: RequestSpec| {
            if spec.url.contains("/po?") {
                resp(500, "{}")
            } else {
                ok_json("7")
            }
        };
        let (summary, _) = run_with(&suite, &RunScope::default(), &send, None, false);
        let by = |id: &str| summary.results.iter().find(|r| r.step_id == id).expect(id);
        assert_eq!(by("po").outcome(), StepOutcome::Failed);
        assert_eq!(by("receipt").outcome(), StepOutcome::Blocked);
        assert_eq!(by("del_receipt").outcome(), StepOutcome::Skipped);
        // PO ditolak server (500) → tidak ada data yang perlu dibersihkan.
        assert_eq!(by("del_po").outcome(), StepOutcome::Skipped);
        assert_eq!(by("del_vendor").outcome(), StepOutcome::Passed);
        assert!(summary.residue.is_empty(), "{:?}", summary.residue);
    }

    #[test]
    fn cancel_stops_tests_but_cleans_up_created_data() {
        let suite = erp_suite();
        let send = |_spec: RequestSpec| ok_json("9");
        // Batal setelah 2 langkah (login, vendor).
        let (summary, order) = run_with(&suite, &RunScope::default(), &send, Some(2), false);
        assert!(summary.cancelled);
        assert!(order.contains(&"del_vendor".to_string()));
        let by = |id: &str| summary.results.iter().find(|r| r.step_id == id).expect(id);
        assert_eq!(by("po").outcome(), StepOutcome::Skipped);
        assert_eq!(by("del_vendor").outcome(), StepOutcome::Passed);
        assert_eq!(by("del_po").outcome(), StepOutcome::Skipped);
        assert!(summary.residue.is_empty());
    }

    #[test]
    fn abort_skips_cleanup_and_reports_residue() {
        let suite = erp_suite();
        let send = |_spec: RequestSpec| ok_json("9");
        let cancel = AtomicBool::new(false);
        let abort = AtomicBool::new(false);
        let mut n = 0;
        let mut emit = |e: TestEvent| {
            if let TestEvent::CleanupStarted = e {
                abort.store(true, Ordering::SeqCst);
            }
            n += 1;
        };
        let summary = run_suite(
            &suite,
            &RunScope::default(),
            initial_vars(&suite),
            &send,
            &cancel,
            &abort,
            &mut emit,
        );
        assert!(summary.aborted);
        assert_eq!(summary.residue.len(), 3);
    }

    #[test]
    fn group_scope_pulls_in_dependencies_and_their_cleanup() {
        let suite = erp_suite();
        let include = select_for_groups(&suite, &["Plant".to_string()]);
        let picked: Vec<&str> = suite
            .steps
            .iter()
            .zip(&include)
            .filter(|(_, inc)| **inc)
            .map(|(s, _)| s.id.as_str())
            .collect();
        // Plant butuh po → vendor → login (setup).
        for id in [
            "login",
            "vendor",
            "po",
            "receipt",
            "del_receipt",
            "del_po",
            "del_vendor",
        ] {
            assert!(picked.contains(&id), "{id} missing from {picked:?}");
        }
        let purchasing = select_for_groups(&suite, &["Purchasing".to_string()]);
        let picked: Vec<&str> = suite
            .steps
            .iter()
            .zip(&purchasing)
            .filter(|(_, inc)| **inc)
            .map(|(s, _)| s.id.as_str())
            .collect();
        assert!(!picked.contains(&"receipt"));
        assert!(!picked.contains(&"del_receipt"));
        assert!(picked.contains(&"del_po"));

        let send = |_spec: RequestSpec| ok_json("1");
        let scope = RunScope::groups(&suite, vec!["Purchasing".into()]);
        let (summary, order) = run_with(&suite, &scope, &send, None, false);
        assert_eq!(order, vec!["login", "vendor", "po", "del_po", "del_vendor"]);
        assert!(summary.residue.is_empty());
    }

    #[test]
    fn lint_reports_order_missing_cleanup_and_cleanup_order() {
        let mut suite = erp_suite();
        let known = std::collections::HashSet::new();
        assert!(
            lint(&suite, &known)
                .iter()
                .all(|i| i.level != LintLevel::Error)
        );
        // Pindahkan receipt ke group pertama → memakai po_id sebelum disediakan.
        suite.steps[3].group = "Purchasing".into();
        suite.steps.swap(2, 3);
        let issues = lint(&suite, &known);
        assert!(
            issues
                .iter()
                .any(|i| i.level == LintLevel::Error && i.message.contains("only provided later"))
        );
        // Hapus cleanup vendor → peringatan data tertinggal.
        let mut suite = erp_suite();
        suite.steps.retain(|s| s.id != "del_vendor");
        assert!(
            lint(&suite, &known)
                .iter()
                .any(|i| i.message.starts_with("Creates data"))
        );
        // Cleanup urutan terbalik → peringatan, lalu auto-order memperbaikinya.
        let mut suite = erp_suite();
        let n = suite.steps.len();
        suite.steps[n - 3..].reverse();
        assert!(
            lint(&suite, &known)
                .iter()
                .any(|i| i.message.starts_with("Cleanup order"))
        );
        auto_order_cleanup(&mut suite);
        let tail: Vec<&str> = suite.steps[n - 3..].iter().map(|s| s.id.as_str()).collect();
        assert_eq!(tail, vec!["del_receipt", "del_po", "del_vendor"]);
        assert!(
            !lint(&suite, &known)
                .iter()
                .any(|i| i.message.starts_with("Cleanup order"))
        );
    }

    #[test]
    fn dependencies_and_cleanup_targets() {
        let suite = erp_suite();
        let deps = dependencies(&suite);
        let idx = |id: &str| suite.step_index(id).expect(id);
        assert_eq!(deps[idx("po")].depends_on, vec![idx("vendor")]);
        assert_eq!(
            deps[idx("vendor")].used_by,
            vec![idx("po"), idx("del_vendor")]
        );
        let mut auto = suite.clone();
        let c = auto.step_index("del_po").expect("del_po");
        auto.steps[c].cleanup_for = None;
        let deps = dependencies(&auto);
        assert_eq!(cleanup_target(&auto, &deps, c), Some(idx("po")));
    }

    #[test]
    fn groups_order_execution_and_can_be_renamed_or_moved() {
        let mut suite = erp_suite();
        suite.move_group("Plant", -1);
        let order: Vec<&str> = suite
            .execution_order()
            .into_iter()
            .map(|i| suite.steps[i].id.as_str())
            .collect();
        assert_eq!(&order[..4], &["login", "receipt", "vendor", "po"]);
        suite.rename_group("Plant", "Warehouse");
        assert_eq!(suite.group_names(), vec!["Warehouse", "Purchasing"]);
        suite.remove_group("Warehouse");
        assert!(suite.steps.iter().all(|s| s.group != "Warehouse"));
    }

    #[test]
    fn ensure_ids_fixes_duplicates_and_dangling_cleanup() {
        let mut suite = HttpTestSuite {
            steps: vec![
                HttpTestStep {
                    id: "a".into(),
                    ..Default::default()
                },
                HttpTestStep {
                    id: "a".into(),
                    ..Default::default()
                },
                HttpTestStep {
                    id: String::new(),
                    cleanup_for: Some("zzz".into()),
                    phase: StepPhase::Cleanup,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        suite.ensure_ids();
        let ids: std::collections::HashSet<&str> =
            suite.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), 3);
        assert_eq!(suite.steps[2].cleanup_for, None);
    }

    #[test]
    fn merges_ai_steps_with_id_remap() {
        let mut suite = erp_suite();
        let mut new_step = step(
            "po",
            "Sales",
            StepPhase::Test,
            HttpMethod::POST,
            "{{base}}/so",
        );
        new_step.extract.push(Extract {
            var: "so_id".into(),
            from: "json:$.id".into(),
        });
        let mut clean = step(
            "x",
            "Sales",
            StepPhase::Cleanup,
            HttpMethod::DELETE,
            "{{base}}/so/{{so_id}}",
        );
        clean.cleanup_for = Some("po".into());
        let n = merge_generated_steps(
            &mut suite,
            vec![new_step, clean],
            vec![("so_token".into(), "".into())],
        );
        assert_eq!(n, 2);
        let created = suite
            .steps
            .iter()
            .find(|s| s.request.url.ends_with("/so"))
            .expect("so");
        assert_ne!(created.id, "po");
        let cleanup = suite
            .steps
            .iter()
            .find(|s| s.request.url.contains("{{so_id}}"))
            .expect("cleanup");
        assert_eq!(cleanup.cleanup_for.as_deref(), Some(created.id.as_str()));
        assert!(suite.group_names().contains(&"Sales".to_string()));
    }

    #[test]
    fn parses_phase_group_and_cleanup_from_ai() {
        let reply = r#"{"suites":[{"name":"P2P","steps":[
          {"id":"s1","name":"Create PO","phase":"test","group":"Purchasing","method":"POST","url":"{{b}}/po","extract":{"po_id":"json:$.id"}},
          {"id":"s2","name":"Delete PO","phase":"teardown","group":"Purchasing","cleanup_for":"s1","method":"DELETE","url":"{{b}}/po/{{po_id}}"}
        ]}]}"#;
        let suites = parse_tests_reply(reply, &[]).expect("parse");
        let s = &suites[0];
        assert_eq!(s.steps[0].group, "Purchasing");
        assert_eq!(s.steps[1].phase, StepPhase::Cleanup);
        assert_eq!(s.steps[1].cleanup_for.as_deref(), Some("s1"));
        let (steps, _) = parse_steps_reply(r#"{"steps":[{"url":"{{b}}/x","method":"GET"}]}"#, &[])
            .expect("steps");
        assert_eq!(steps.len(), 1);
    }

    #[test]
    fn residue_sql_quotes_identifiers_and_validates_run_id() {
        let cols = vec!["code".to_string(), "na\"me".to_string()];
        let sql = residue_count_sql("PostgreSQL", "public.orders", &cols, "1700").expect("sql");
        assert_eq!(
            sql,
            "SELECT COUNT(*) AS n FROM \"public\".\"orders\" WHERE \"code\" LIKE '%1700%' OR \"na\"\"me\" LIKE '%1700%'"
        );
        assert!(residue_count_sql("MySQL", "t", &cols, "1'; drop").is_none());
        assert!(residue_count_sql("MySQL", "t", &[], "1").is_none());
        assert!(
            residue_count_sql("MySQL", "t", &cols, "17")
                .expect("sql")
                .contains("`code`")
        );
        assert!(table_matches("public.orders", "orders"));
    }

    #[test]
    fn saves_runs_and_prunes_history() {
        let dir = std::env::temp_dir().join(format!(
            "tabular-http-runs-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        ));
        for i in 0..(MAX_RUNS_PER_SUITE + 3) {
            let run = TestRun {
                run_id: format!("{:05}", i),
                suite_id: "s/1".into(),
                started_at: format!("2026-10-15T00:{:02}:{:02}Z", i / 60, i % 60),
                ..Default::default()
            };
            save_run_in(&dir, &run).expect("save");
        }
        let runs = load_runs_in(&dir, "s/1", 100);
        assert_eq!(runs.len(), MAX_RUNS_PER_SUITE);
        assert_eq!(runs[0].run_id, format!("{:05}", MAX_RUNS_PER_SUITE + 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preflight_reports_unreachable_server_without_sending() {
        // Port yang baru dilepas hampir pasti tidak didengar.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .expect("port");
        let mut suite = erp_suite();
        suite.variables = vec![("base".into(), format!("http://127.0.0.1:{port}"))];
        let vars = initial_vars(&suite);
        let err = preflight(&suite, &RunScope::default(), &vars).expect_err("down");
        assert!(err.contains(&format!("127.0.0.1:{port}")), "{err}");
        let summary = preflight_failed(&suite, &RunScope::default(), err.clone());
        assert_eq!(summary.preflight_error.as_deref(), Some(err.as_str()));
        assert!(
            summary
                .results
                .iter()
                .all(|r| r.outcome() == StepOutcome::Blocked)
        );
        // Server yang hidup lolos.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let live = listener.local_addr().expect("addr").port();
        suite.variables = vec![("base".into(), format!("http://127.0.0.1:{live}"))];
        assert!(preflight(&suite, &RunScope::default(), &initial_vars(&suite)).is_ok());
    }

    #[test]
    fn fills_only_test_account_credentials() {
        let mut vars = vec![
            ("test_password".to_string(), String::new()),
            ("password".to_string(), String::new()),
            ("token".to_string(), String::new()),
            ("new_user_password".to_string(), "keep".to_string()),
        ];
        let filled = fill_test_credentials(&mut vars);
        assert_eq!(filled, vec!["test_password"]);
        assert!(vars[0].1.len() >= 12);
        assert!(vars[1].1.is_empty() && vars[2].1.is_empty());
        assert_eq!(vars[3].1, "keep");
    }

    #[test]
    fn lint_warns_about_empty_variables() {
        let mut suite = erp_suite();
        suite.variables.push(("api_secret".into(), String::new()));
        suite.steps[1].request.url.push_str("&k={{api_secret}}");
        let issues = lint(&suite, &std::collections::HashSet::new());
        assert!(
            issues
                .iter()
                .any(|i| i.message.contains("{{api_secret}} is empty"))
        );
        let known: std::collections::HashSet<String> = ["api_secret".to_string()].into();
        assert!(
            !lint(&suite, &known)
                .iter()
                .any(|i| i.message.contains("is empty"))
        );
    }

    #[test]
    fn extracts_cookies_and_redacts_them_in_headers() {
        let mut r = resp(200, r#"{"id":"u1"}"#);
        r.headers.push((
            "set-cookie".into(),
            "dp-token=abc.def; HttpOnly; Path=/; Max-Age=60".into(),
        ));
        r.headers
            .push(("set-cookie".into(), "other=1; Path=/".into()));
        assert_eq!(
            read_target(&r, "cookie:dp-token").as_deref(),
            Some("abc.def")
        );
        assert_eq!(read_target(&r, "cookie:other").as_deref(), Some("1"));
        assert_eq!(read_target(&r, "cookie:none"), None);
        let h = redact_headers(&r.headers);
        let cookie = &h.iter().find(|(k, _)| k == "set-cookie").expect("cookie").1;
        assert!(cookie.starts_with("dp-token="));
        assert!(cookie.contains("HttpOnly"));
        assert!(!cookie.contains("abc.def"));
    }

    #[test]
    fn missing_extract_fails_the_step_so_ai_sees_the_body() {
        let step = HttpTestStep {
            id: "s1".into(),
            name: "register".into(),
            request: SavedRequest {
                method: HttpMethod::POST,
                url: "http://h/register".into(),
                ..Default::default()
            },
            extract: vec![Extract {
                var: "user_id".into(),
                from: "json:$.user.id".into(),
            }],
            assertions: vec![Assertion {
                target: "status".into(),
                op: AssertOp::Equals,
                value: "200".into(),
            }],
            enabled: true,
            ..Default::default()
        };
        let mut v = HashMap::new();
        let send = |_spec: RequestSpec| resp(200, r#"{"id":"u1"}"#);
        let r = run_step(0, &step, &mut v, &send);
        assert_eq!(r.outcome(), StepOutcome::Failed);
        let failed = r.assertions.iter().find(|a| !a.passed).expect("failed");
        assert_eq!(failed.assertion.target, "json:$.user.id");
        assert!(failed.assertion.value.contains("user_id"));
        assert!(!v.contains_key("user_id"));
    }

    #[test]
    fn bearer_token_from_cookie_is_also_sent_as_cookie() {
        let mut suite = erp_suite();
        suite.steps[0].extract = vec![Extract {
            var: "token".into(),
            from: "cookie:dp-token".into(),
        }];
        let mut me = step("me", "", StepPhase::Test, HttpMethod::GET, "{{base}}/me");
        me.request.auth_type = HttpAuthType::BearerToken;
        me.request.bearer_token = "{{token}}".into();
        let negative = step("anon", "", StepPhase::Test, HttpMethod::GET, "{{base}}/me");
        suite.steps.push(me.clone());
        suite.steps.push(negative.clone());
        let cookies = cookie_vars(&suite);
        assert_eq!(cookies.get("token").map(String::as_str), Some("dp-token"));
        let sent = with_cookie_auth(&me, &cookies);
        assert!(
            sent.request
                .headers
                .iter()
                .any(|(k, v, on)| *on && k == "Cookie" && v == "dp-token={{token}}")
        );
        // Langkah tanpa auth (uji negatif) tidak mendapat cookie.
        let anon = with_cookie_auth(&negative, &cookies);
        assert!(anon.request.headers.is_empty());
    }

    // ── Langkah DB & concurrent ─────────────────────────────────────────

    use crate::http_test_db::{DbRequest, DbResponse};
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    /// Executor palsu: mengembalikan hasil berurutan per panggilan.
    struct FakeDb {
        calls: AtomicUsize,
        outputs: Vec<Result<QueryOutput, String>>,
        seen: Mutex<Vec<DbRequest>>,
    }

    impl FakeDb {
        fn new(outputs: Vec<Result<QueryOutput, String>>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                outputs,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl DbExecutor for FakeDb {
        fn query(&self, req: &DbRequest) -> Result<DbResponse, String> {
            self.seen.lock().unwrap().push(req.clone());
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            let out = self.outputs[n.min(self.outputs.len() - 1)].clone()?;
            Ok(DbResponse {
                connection_name: "staging db".into(),
                sql_display: req.sql.clone(),
                output: out,
            })
        }
    }

    fn scalar(v: &str) -> Result<QueryOutput, String> {
        Ok(QueryOutput {
            columns: vec!["qty_balance".into()],
            rows: vec![vec![v.into()]],
            ..Default::default()
        })
    }

    fn db_step(sql: &str, wait: Option<u64>, assertions: Vec<Assertion>) -> HttpTestStep {
        HttpTestStep {
            id: "d1".into(),
            name: "Balance".into(),
            kind: StepKind::DbQuery,
            db: Some(DbQueryStep {
                connection_var: "db_main".into(),
                sql: sql.into(),
                wait_until_ms: wait,
                poll_ms: 10,
                ..Default::default()
            }),
            assertions,
            enabled: true,
            ..Default::default()
        }
    }

    fn assertion(target: &str, op: AssertOp, value: &str) -> Assertion {
        Assertion {
            target: target.into(),
            op,
            value: value.into(),
        }
    }

    #[test]
    fn db_step_polls_until_the_async_write_lands() {
        let fake = FakeDb::new(vec![scalar("100"), scalar("100"), scalar("120")]);
        let step = db_step(
            "SELECT qty_balance FROM kandang_stocks WHERE id = {{id}}",
            Some(2_000),
            vec![assertion(
                "scalar",
                AssertOp::Approx,
                "{{saldo_awal}} + {{qty_kirim}}",
            )],
        );
        let mut v = vars(&[
            ("db_main", "staging db"),
            ("id", "1"),
            ("saldo_awal", "100"),
            ("qty_kirim", "20"),
        ]);
        let r = run_db_step(0, &step, &mut v, Some(&fake), &AtomicBool::new(false));
        assert!(r.passed(), "{:?}", r.assertions);
        let info = r.db.expect("db info");
        assert_eq!(info.attempts, 3);
        assert!(!info.timed_out);
        assert_eq!(info.connection, "staging db");
        // Executor menerima SQL mentah + variabel; pengikatan terjadi di sana.
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen[0].connection, "staging db");
        assert!(seen[0].sql.contains("{{id}}"));
    }

    #[test]
    fn db_step_times_out_and_fails() {
        let fake = FakeDb::new(vec![scalar("100")]);
        let step = db_step(
            "SELECT 1",
            Some(60),
            vec![assertion("scalar", AssertOp::Equals, "120")],
        );
        let mut v = vars(&[("db_main", "x")]);
        let r = run_db_step(0, &step, &mut v, Some(&fake), &AtomicBool::new(false));
        assert_eq!(r.outcome(), StepOutcome::Failed);
        let info = r.db.expect("db info");
        assert!(info.timed_out);
        assert!(info.attempts >= 2);
    }

    #[test]
    fn db_step_extracts_and_reports_errors() {
        let fake = FakeDb::new(vec![scalar("4750")]);
        let mut step = db_step("SELECT 1", None, Vec::new());
        step.extract = vec![Extract {
            var: "saldo_awal".into(),
            from: "scalar".into(),
        }];
        let mut v = vars(&[("db_main", "x")]);
        let r = run_db_step(0, &step, &mut v, Some(&fake), &AtomicBool::new(false));
        assert!(r.passed());
        assert_eq!(v.get("saldo_awal").map(String::as_str), Some("4750"));

        let failing = FakeDb::new(vec![Err("refused: only SELECT".into())]);
        let r = run_db_step(0, &step, &mut v, Some(&failing), &AtomicBool::new(false));
        assert_eq!(r.outcome(), StepOutcome::Error);
        // Tanpa akses database langkah DB tidak lulus diam-diam.
        let r = run_db_step(0, &step, &mut v, None, &AtomicBool::new(false));
        assert_eq!(r.outcome(), StepOutcome::Error);
    }

    #[test]
    fn missing_connection_variable_blocks_the_db_step() {
        let suite = HttpTestSuite {
            steps: vec![db_step(
                "SELECT 1",
                None,
                vec![assertion("row_count", AssertOp::Equals, "1")],
            )],
            ..Default::default()
        };
        let fake = FakeDb::new(vec![scalar("1")]);
        let send = |_s: RequestSpec| resp(200, "{}");
        let io = RunIo {
            send: &send,
            send_parallel: None,
            db: Some(&fake),
        };
        let (c, a) = (AtomicBool::new(false), AtomicBool::new(false));
        let s = run_suite_with(
            &suite,
            &RunScope::default(),
            initial_vars(&suite),
            &io,
            &c,
            &a,
            &mut |_| {},
        );
        assert_eq!(s.results[0].outcome(), StepOutcome::Blocked);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn burst_aggregates_count_any_all() {
        let ok = resp(201, r#"{"status":"success","id":1}"#);
        let dup = resp(
            409,
            r#"{"status":"error","message":"already been received"}"#,
        );
        let dup2 = resp(
            409,
            r#"{"status":"error","message":"already been received"}"#,
        );
        let all = vec![&ok, &dup, &dup2];
        let v = HashMap::new();
        let eval = |t: &str, op: AssertOp, val: &str| {
            evaluate_burst(&all, &assertion(t, op, val), &v).passed
        };
        assert!(eval("count:status=2xx", AssertOp::Equals, "1"));
        assert!(eval("count:status=409", AssertOp::Equals, "2"));
        assert!(eval("count:status=4xx", AssertOp::Equals, "2"));
        assert!(eval("count:json:$.status=success", AssertOp::Equals, "1"));
        assert!(eval("count:json:$.id", AssertOp::Equals, "1"));
        assert!(eval(
            "any:json:$.message",
            AssertOp::Contains,
            "already been received"
        ));
        assert!(!eval("all:status", AssertOp::LessThan, "300"));
        assert!(!eval("status", AssertOp::LessThan, "300"));
        assert!(eval("all:status", AssertOp::LessThan, "500"));
    }

    #[test]
    fn concurrent_step_sends_burst_through_barrier_and_detects_double_submit() {
        let step = HttpTestStep {
            id: "terima".into(),
            name: "Receive the same delivery 5x".into(),
            request: SavedRequest {
                method: HttpMethod::POST,
                url: "http://h/stock-ins".into(),
                ..Default::default()
            },
            concurrency: Some(Concurrency {
                count: 5,
                barrier: true,
                rounds: 3,
            }),
            extract: vec![Extract {
                var: "stock_in_id".into(),
                from: "json:$.id".into(),
            }],
            assertions: vec![assertion("count:status=2xx", AssertOp::Equals, "1")],
            enabled: true,
            ..Default::default()
        };
        let suite = HttpTestSuite {
            steps: vec![step],
            ..Default::default()
        };
        // Server bug: dua dari lima request lolos di putaran kedua.
        let n = AtomicUsize::new(0);
        let buggy = |_s: RequestSpec| {
            let k = n.fetch_add(1, Ordering::SeqCst);
            if k == 0 || k == 5 || k == 6 {
                resp(201, &format!(r#"{{"id":{k}}}"#))
            } else {
                resp(409, r#"{"message":"already been received"}"#)
            }
        };
        let io = RunIo {
            send: &buggy,
            send_parallel: Some(&buggy),
            db: None,
        };
        let (c, a) = (AtomicBool::new(false), AtomicBool::new(false));
        let s = run_suite_with(
            &suite,
            &RunScope::default(),
            initial_vars(&suite),
            &io,
            &c,
            &a,
            &mut |_| {},
        );
        let r = &s.results[0];
        assert_eq!(n.load(Ordering::SeqCst), 15);
        assert_eq!(r.instances.len(), 15);
        assert_eq!(r.outcome(), StepOutcome::Failed);
        assert!(
            r.assertions[0]
                .actual
                .as_deref()
                .unwrap()
                .contains("round 2")
        );
        assert_eq!(r.created_count(), 3);

        // Server benar: tepat satu sukses per putaran.
        let m = AtomicUsize::new(0);
        let fixed = |_s: RequestSpec| {
            if m.fetch_add(1, Ordering::SeqCst).is_multiple_of(5) {
                resp(201, r#"{"id":1}"#)
            } else {
                resp(409, "{}")
            }
        };
        let io = RunIo {
            send: &fixed,
            send_parallel: None,
            db: None,
        };
        let s = run_suite_with(
            &suite,
            &RunScope::default(),
            initial_vars(&suite),
            &io,
            &c,
            &a,
            &mut |_| {},
        );
        assert!(s.results[0].passed(), "{:?}", s.results[0].assertions);
    }

    #[test]
    fn cleanup_runs_once_per_created_instance() {
        let create = HttpTestStep {
            id: "c".into(),
            name: "Create".into(),
            request: SavedRequest {
                method: HttpMethod::POST,
                url: "http://h/items".into(),
                ..Default::default()
            },
            concurrency: Some(Concurrency {
                count: 3,
                barrier: false,
                rounds: 1,
            }),
            extract: vec![Extract {
                var: "item_id".into(),
                from: "json:$.id".into(),
            }],
            assertions: vec![assertion("all:status", AssertOp::LessThan, "300")],
            enabled: true,
            ..Default::default()
        };
        let delete = HttpTestStep {
            id: "d".into(),
            name: "Delete".into(),
            phase: StepPhase::Cleanup,
            cleanup_for: Some("c".into()),
            request: SavedRequest {
                method: HttpMethod::DELETE,
                url: "http://h/items/{{item_id}}".into(),
                ..Default::default()
            },
            assertions: vec![assertion("status", AssertOp::Equals, "204")],
            enabled: true,
            ..Default::default()
        };
        let suite = HttpTestSuite {
            steps: vec![create, delete],
            ..Default::default()
        };
        let n = AtomicUsize::new(0);
        let deleted = Mutex::new(Vec::new());
        let send = |s: RequestSpec| {
            if s.url.contains("/items/") {
                deleted.lock().unwrap().push(s.url.clone());
                resp(204, "")
            } else {
                let k = n.fetch_add(1, Ordering::SeqCst);
                resp(201, &format!(r#"{{"id":"i{k}"}}"#))
            }
        };
        let io = RunIo {
            send: &send,
            send_parallel: None,
            db: None,
        };
        let (c, a) = (AtomicBool::new(false), AtomicBool::new(false));
        let s = run_suite_with(
            &suite,
            &RunScope::default(),
            initial_vars(&suite),
            &io,
            &c,
            &a,
            &mut |_| {},
        );
        let mut urls = deleted.lock().unwrap().clone();
        urls.sort();
        assert_eq!(
            urls,
            vec![
                "http://h/items/i0",
                "http://h/items/i1",
                "http://h/items/i2"
            ]
        );
        let cleanup = s.results.iter().find(|r| r.step_id == "d").unwrap();
        assert!(cleanup.passed());
        assert_eq!(cleanup.instances.len(), 3);
        assert!(s.residue.is_empty());
    }

    #[test]
    fn old_suites_load_and_save_without_new_fields() {
        let request = serde_json::to_value(SavedRequest {
            id: "r1".into(),
            url: "{{base}}/ping".into(),
            ..Default::default()
        })
        .unwrap();
        // Bentuk file suite sebelum fitur DB/concurrent.
        let json = serde_json::json!({"id": "suite_1", "name": "Old", "steps": [{
            "id": "s1", "name": "Ping", "request": request,
            "assertions": [{"target": "status", "op": "equals", "value": "200"}]}]});
        let suite: HttpTestSuite = serde_json::from_value(json).expect("old suite");
        assert_eq!(suite.steps[0].kind, StepKind::Http);
        assert!(suite.steps[0].db.is_none() && suite.steps[0].concurrency.is_none());
        let out = serde_json::to_string(&suite).unwrap();
        assert!(!out.contains("\"kind\""));
        assert!(!out.contains("\"db\""));
        assert!(!out.contains("\"concurrency\""));
        // Langkah DB tidak menulis request kosong.
        let db = db_step("SELECT 1", None, Vec::new());
        let out = serde_json::to_value(&db).unwrap();
        assert!(out.get("request").is_none());
        assert_eq!(out["kind"], "db_query");
        let back: HttpTestStep = serde_json::from_value(out).unwrap();
        assert!(back.is_db());
    }

    #[test]
    fn lint_flags_db_and_aggregate_mistakes() {
        let bad_sql = db_step(
            "DELETE FROM t WHERE id = {{id}}",
            None,
            vec![assertion("status", AssertOp::Equals, "200")],
        );
        let issues = step_kind_issues(&bad_sql);
        assert!(issues.iter().any(|m| m.contains("SELECT")), "{issues:?}");
        assert!(issues.iter().any(|m| m.contains("does not apply")));
        let aggregate_without_burst = HttpTestStep {
            assertions: vec![assertion("count:status=2xx", AssertOp::Equals, "1")],
            ..Default::default()
        };
        assert!(!step_kind_issues(&aggregate_without_burst).is_empty());
        let ok = db_step(
            "SELECT id FROM t WHERE code = '{{code}}'",
            None,
            vec![assertion("row_count", AssertOp::Equals, "0")],
        );
        assert!(step_kind_issues(&ok).is_empty());
    }

    #[test]
    fn ai_reply_parses_db_and_concurrent_steps() {
        let v: Value = serde_json::json!({
            "id": "s5", "name": "Balance", "phase": "test", "kind": "db_query",
            "db": {"connection_var": "{{db_main}}", "sql": "SELECT 1", "wait_until_ms": 5000},
            "assert": [{"target": "scalar", "op": "approx", "value": "1"}],
            "extract": [{"var": "a", "from": "scalar"}]
        });
        let s = parse_step_value(&v, &[]).expect("db step");
        assert!(s.is_db());
        let db = s.db.unwrap();
        assert_eq!(db.connection_var, "db_main");
        assert_eq!(db.wait_until_ms, Some(5000));
        assert_eq!(s.assertions[0].op, AssertOp::Approx);
        assert_eq!(s.extract[0].var, "a");
        let v: Value = serde_json::json!({
            "id": "s2", "name": "Dup", "method": "POST", "url": "{{base}}/x",
            "concurrency": {"count": 500, "rounds": 3},
            "assert": [{"target": "count:status=2xx", "op": "equals", "value": "1"}]
        });
        let s = parse_step_value(&v, &[]).expect("http step");
        let c = s.concurrency.unwrap();
        assert_eq!((c.count, c.rounds, c.barrier), (MAX_CONCURRENCY, 3, true));
    }
}
