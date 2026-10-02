//! Generator AI untuk alur bisnis flow card: dari kode di repository, AI
//! menelusuri apa yang dikerjakan tiap endpoint HTTP langkah demi langkah.
//!
//! Alurnya mengikuti [`crate::repo_endpoints`] dan headless supaya bisa diuji:
//! 1. Repository disiapkan dengan [`repo_scan::choose_source`] +
//!    [`repo_scan::resolve_repo`].
//! 2. Card yang alurnya masih segar (hash isi file sumber sama) dilewati.
//! 3. [`repo_scan::ai_workspace`] menentukan AI membaca repository sendiri
//!    atau hanya menerima cuplikan file route.
//! 4. Card dikirim per batch ([`FLOW_BATCH_SIZE`]) yang dikelompokkan per
//!    file sumber, beberapa batch sekaligus, dengan slot AI global yang sama
//!    dengan generate endpoint.
//! 5. Jawaban diurai [`parse_flows_reply`], lalu [`FlowMeta`] diisi termasuk
//!    `source_hash` untuk mendeteksi alur basi di generate berikutnya.
//!
//! Modul ini tidak boleh bergantung pada `window_egui`.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use serde_json::Value;

use crate::agent::harness::ProgressStatus;
use crate::ai_assistant::{ChatBackend, ChatWorkspace};
use crate::diagram_flow::{
    MAX_DETAIL_CHARS, MAX_SOURCE_FILES, MAX_STEP_COLUMNS, MAX_STEPS, MAX_TITLE_CHARS,
};
use crate::models::structs::{FlowMeta, FlowOp, FlowStep, FlowStepKind, FlowTarget};
use crate::repo_endpoints::{self, MAX_GLOBAL_AI_TURNS, MAX_PARALLEL_BATCHES, source_file};
use crate::repo_scan::{
    self, MAX_FILE_BYTES, PromptMode, RepoJobEvent, RepoScanError, RepoSource, step,
};

/// Card maksimum per giliran AI.
pub const FLOW_BATCH_SIZE: usize = 6;
/// Panjang maksimum ringkasan flow (karakter).
const MAX_SUMMARY_CHARS: usize = 200;
/// Panjang maksimum `condition`, `source`, nama resource dan kolom (karakter).
const MAX_SHORT_CHARS: usize = 160;
/// Panjang maksimum satu nama kolom (karakter).
const MAX_COLUMN_CHARS: usize = 64;
/// Batas panjang daftar tabel di system prompt (byte).
const MAX_TABLE_LIST_BYTES: usize = 20_000;

// ─── Antarmuka ──────────────────────────────────────────────────────────────

pub struct FlowScanInput {
    pub repo_path: Option<String>,
    pub repo_url: Option<String>,
    /// Judul group atau folder, untuk prompt dan log.
    pub scope_name: String,
    pub cards: Vec<FlowSeed>,
    /// Nama (id node) tabel diagram, tanpa tabel link database.
    pub tables: Vec<String>,
    pub backend: Option<ChatBackend>,
    pub backend_label: String,
    pub cache_root: PathBuf,
    /// Batch AI yang dikerjakan bersamaan (1..=[`MAX_PARALLEL_BATCHES`]).
    pub parallel: usize,
    /// `false` = lewati card yang `source_hash`-nya masih sama.
    pub force: bool,
}

/// Bahan satu flow untuk AI.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlowSeed {
    pub card_id: String,
    pub method: String,
    pub path: String,
    /// Lokasi definisi route (`path/file:line`).
    pub source: Option<String>,
    /// Tabel yang sudah tertaut ke endpoint ini (dari `endpoint_links`).
    pub known_tables: Vec<String>,
    pub previous: Option<FlowMeta>,
}

impl FlowSeed {
    /// File definisi route, tanpa nomor baris; kosong bila tidak diketahui.
    fn route_file(&self) -> &str {
        self.source.as_deref().map_or("", source_file)
    }
}

/// Alur hasil generate untuk satu card.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedFlow {
    pub card_id: String,
    pub summary: String,
    pub steps: Vec<FlowStep>,
    pub meta: FlowMeta,
}

#[derive(Clone, Debug, Default)]
pub struct FlowScanOutcome {
    pub flows: Vec<GeneratedFlow>,
    /// Card yang dilewati karena file sumbernya tidak berubah.
    pub skipped_fresh: usize,
    /// (card_id, pesan)
    pub failed: Vec<(String, String)>,
    pub note: Option<String>,
}

pub type FlowEvent = RepoJobEvent<FlowScanOutcome>;

/// Pegangan job generate alur yang berjalan.
pub struct FlowScanHandle {
    pub rx: mpsc::Receiver<FlowEvent>,
    cancel: Arc<AtomicBool>,
}

impl FlowScanHandle {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

// ─── Prompt ─────────────────────────────────────────────────────────────────

const FLOW_SCHEMA: &str = r#"{"flows":[{"id":"flw_3","summary":"Creates an order and reserves stock","files":["src/routes/orders.ts","src/services/order.ts"],"steps":[{"kind":"auth","title":"Verify bearer token","source":"src/middleware/auth.ts:12"},{"kind":"db","op":"read","table":"users","columns":["id","status"],"title":"Load the customer","source":"src/services/order.ts:40"},{"kind":"db","op":"insert","table":"orders","columns":["user_id","total"],"title":"Create the order","source":"src/services/order.ts:58"},{"kind":"queue","op":"publish","resource":"order.created","title":"Publish order.created","source":"src/services/order.ts:71"},{"kind":"respond","title":"Return 201 with the order"}]}]}"#;

/// Daftar tabel dipisah koma, dipotong di [`MAX_TABLE_LIST_BYTES`].
fn table_list(tables: &[String]) -> String {
    let mut list = String::new();
    for t in tables {
        if list.len() > MAX_TABLE_LIST_BYTES {
            list.push_str(", …");
            break;
        }
        if !list.is_empty() {
            list.push_str(", ");
        }
        list.push_str(t);
    }
    list
}

/// Prompt untuk satu batch card.
pub fn build_flow_prompts(
    scope_name: &str,
    tables: &[String],
    batch: &[FlowSeed],
    mode: PromptMode,
    repo_root: Option<&Path>,
    snippets: &str,
) -> (String, String) {
    let mut system = String::from(
        "You trace what each HTTP endpoint does, step by step, from its server source code, so \
         an architect can see the business process. Follow the handler into middleware, \
         services, repositories, ORM models, raw SQL, queue publishers and HTTP clients.\n",
    );
    system.push_str(&repo_endpoints::repo_location(mode, repo_root));
    system.push_str(&format!(
        "Rules:\n\
         - List steps in execution order, at most {MAX_STEPS} per endpoint. Merge trivial lines \
           into one step.\n\
         - `kind` is auth, validate, db, external, queue, cache, logic, branch or respond.\n\
         - A db step names exactly one table in `table`, with `op` read, insert, update, delete \
           or upsert, and the columns it filters or writes in `columns`.\n\
         - external, queue and cache steps put the service, topic or key pattern in `resource`, \
           with `op` call, publish or consume.\n\
         - `condition` says when a step runs if it is not always executed.\n\
         - `source` is `path/to/file:line` of the code for that step.\n\
         - `files` lists every file you read for this endpoint, relative to the repository \
           root.\n"
    ));
    if !tables.is_empty() {
        system.push_str(&format!(
            "- Use table names from this list when they match: {}\n",
            table_list(tables)
        ));
    }
    system.push_str(
        "- Never include secrets, tokens or personal data. Never create, modify or remove \
         files.\n\
         Reply with ONLY one JSON object, no prose and no markdown fence, shaped like this \
         example:\n",
    );
    system.push_str(FLOW_SCHEMA);
    system.push('\n');
    system.push_str(
        "Return one flow per endpoint in the list, using its id. Leave out an endpoint only \
         when its handler cannot be found.\n",
    );

    let mut user = format!("Scope: {scope_name}\n\nEndpoints to trace:\n");
    for seed in batch {
        user.push_str(&format!(
            "- {}: {} {}",
            seed.card_id, seed.method, seed.path
        ));
        if let Some(src) = seed.source.as_deref().filter(|s| !s.is_empty()) {
            user.push_str(&format!(" ({src})"));
        }
        if !seed.known_tables.is_empty() {
            user.push_str(&format!(" known tables: {}", seed.known_tables.join(", ")));
        }
        user.push('\n');
    }
    if !snippets.is_empty() {
        user.push_str("\nSource code:\n");
        user.push_str(snippets);
    }
    (system, user)
}

// ─── Parser ─────────────────────────────────────────────────────────────────

/// Satu flow dari jawaban AI, sebelum path file dinormalkan dan hash dihitung.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedFlow {
    pub card_id: String,
    pub summary: String,
    /// Path file seperti ditulis AI (belum dinormalkan).
    pub files: Vec<String>,
    pub steps: Vec<FlowStep>,
}

fn text(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| match v.get(*k)? {
            Value::String(s) => Some(s.trim().to_string()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn texts(v: &Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn step_kind(s: &str) -> FlowStepKind {
    match s.trim().to_ascii_lowercase().as_str() {
        "auth" | "authn" | "authz" | "authorization" | "authentication" => FlowStepKind::Auth,
        "validate" | "validation" => FlowStepKind::Validate,
        "db" | "database" | "sql" | "query" => FlowStepKind::Db,
        "external" | "http" | "api" => FlowStepKind::External,
        "queue" | "event" | "publish" => FlowStepKind::Queue,
        "cache" => FlowStepKind::Cache,
        "branch" | "condition" | "if" => FlowStepKind::Branch,
        "respond" | "response" | "return" => FlowStepKind::Respond,
        _ => FlowStepKind::Logic,
    }
}

fn step_op(s: &str) -> Option<FlowOp> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "read" | "select" | "get" | "find" | "query" => FlowOp::Read,
        "insert" | "create" => FlowOp::Insert,
        "update" => FlowOp::Update,
        "delete" | "remove" => FlowOp::Delete,
        "upsert" => FlowOp::Upsert,
        "call" | "request" => FlowOp::Call,
        "publish" | "emit" | "send" | "enqueue" => FlowOp::Publish,
        "consume" | "subscribe" | "receive" => FlowOp::Consume,
        _ => return None,
    })
}

/// Id node tabel diagram untuk nama `name` dari AI. Daftar kosong berarti
/// diagram belum punya tabel, jadi tidak ada yang cocok.
fn match_table(name: &str, tables: &[String]) -> Option<String> {
    if tables.is_empty() {
        return None;
    }
    repo_endpoints::filter_tables(&[name.to_string()], tables)
        .into_iter()
        .next()
}

/// Resource non-tabel menjadi target menurut jenis langkah atau operasinya.
fn resource_target(kind: FlowStepKind, op: Option<FlowOp>, resource: String) -> FlowTarget {
    match (kind, op) {
        (FlowStepKind::Queue, _) | (_, Some(FlowOp::Publish | FlowOp::Consume)) => {
            FlowTarget::Queue(resource)
        }
        (FlowStepKind::Cache, _) => FlowTarget::Cache(resource),
        _ => FlowTarget::External(resource),
    }
}

fn some_text(s: String, max: usize) -> Option<String> {
    (!s.is_empty()).then(|| repo_scan::truncate_chars(&s, max))
}

fn parse_step(v: &Value, tables: &[String]) -> Option<FlowStep> {
    let kind = step_kind(&text(v, &["kind", "type"]));
    let op = step_op(&text(v, &["op", "operation"]));
    let table = text(v, &["table"]);
    let resource = text(v, &["resource", "service", "topic"]);
    let mut detail = text(v, &["detail", "description"]);
    let mut target = None;
    if !table.is_empty() {
        match match_table(&table, tables) {
            Some(id) => target = Some(FlowTarget::Table(id)),
            None => {
                if !detail.is_empty() {
                    detail.push(' ');
                }
                detail.push_str(&format!("(table: {table})"));
            }
        }
    }
    if target.is_none() && !resource.is_empty() {
        target = Some(resource_target(
            kind,
            op,
            repo_scan::truncate_chars(&resource, MAX_SHORT_CHARS),
        ));
    }
    let mut title = text(v, &["title", "name"]);
    if title.is_empty() {
        // Tanpa judul, nama tabel atau resource masih cukup untuk ditampilkan.
        title = if !table.is_empty() { table } else { resource };
    }
    if title.is_empty() {
        return None;
    }
    let mut columns: Vec<String> = Vec::new();
    for c in texts(v, "columns") {
        let c = repo_scan::truncate_chars(&c, MAX_COLUMN_CHARS);
        if !columns.contains(&c) {
            columns.push(c);
        }
        if columns.len() >= MAX_STEP_COLUMNS {
            break;
        }
    }
    Some(FlowStep {
        kind,
        title: repo_scan::truncate_chars(&title, MAX_TITLE_CHARS),
        detail: some_text(detail, MAX_DETAIL_CHARS).unwrap_or_default(),
        target,
        op,
        columns,
        source: some_text(text(v, &["source", "location"]), MAX_SHORT_CHARS),
        condition: some_text(text(v, &["condition", "when"]), MAX_SHORT_CHARS),
    })
}

/// Urai jawaban AI: `{"flows": [...]}` atau langsung array flow. Flow dengan
/// id di luar `batch_ids`, duplikat, atau tanpa langkah dibuang; pemanggil
/// menganggap card yang tidak ada di hasil sebagai gagal. Nama tabel
/// dicocokkan ke `tables` (id node diagram).
pub fn parse_flows_reply(
    text_in: &str,
    batch_ids: &[&str],
    tables: &[String],
) -> Result<Vec<ParsedFlow>, RepoScanError> {
    let items = repo_scan::slice_between(text_in, '{', '}')
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("flows").and_then(Value::as_array).cloned())
        .or_else(|| {
            repo_scan::slice_between(text_in, '[', ']')
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|v| v.as_array().cloned())
        })
        .ok_or_else(|| {
            RepoScanError::Parse(format!(
                "expected a JSON object with \"flows\", got: {}",
                repo_scan::truncate_chars(text_in.trim(), 200)
            ))
        })?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for item in &items {
        let id = text(item, &["id", "card_id"]);
        if !batch_ids.contains(&id.as_str()) || seen.contains(&id) {
            continue;
        }
        let steps: Vec<FlowStep> = item
            .get("steps")
            .and_then(Value::as_array)
            .map(|s| {
                s.iter()
                    .filter_map(|v| parse_step(v, tables))
                    .take(MAX_STEPS)
                    .collect()
            })
            .unwrap_or_default();
        if steps.is_empty() {
            continue;
        }
        seen.insert(id.clone());
        out.push(ParsedFlow {
            card_id: id,
            summary: repo_scan::truncate_chars(&text(item, &["summary"]), MAX_SUMMARY_CHARS),
            files: texts(item, "files"),
            steps,
        });
    }
    Ok(out)
}

// ─── Hash sumber dan alur basi ──────────────────────────────────────────────

/// Path relatif `p` terhadap salah satu `roots`. Path absolut di luar root,
/// path yang naik (`..`), dan path kosong ditolak.
fn relative_path(roots: &[&Path], p: &str) -> Option<String> {
    let p = p.trim().trim_matches('`').trim();
    let rel = if Path::new(p).is_absolute() {
        let path = Path::new(p);
        let stripped = roots.iter().find_map(|r| path.strip_prefix(r).ok())?;
        stripped.to_string_lossy().replace('\\', "/")
    } else {
        p.replace('\\', "/")
    };
    let rel = rel
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
    (!rel.is_empty() && !rel.split('/').any(|seg| seg == "..")).then_some(rel)
}

/// Lokasi `file:line` dengan path file dibuat relatif; `None` bila file di
/// luar repository.
fn relative_source(roots: &[&Path], source: &str) -> Option<String> {
    let (file, line) = match source.rsplit_once(':') {
        Some((f, l)) if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit() || c == '-') => {
            (f, Some(l))
        }
        _ => (source, None),
    };
    let rel = relative_path(roots, file)?;
    Some(match line {
        Some(l) => format!("{rel}:{l}"),
        None => rel,
    })
}

/// md5 gabungan isi `files` (relatif ke `root`), tidak bergantung urutan.
/// Tiap file dibaca paling banyak [`MAX_FILE_BYTES`]; file yang hilang atau
/// path yang tidak aman ikut di-hash sebagai penanda "hilang".
pub fn source_hash(root: &Path, files: &[String]) -> String {
    let mut sorted: Vec<&str> = files.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let mut ctx = md5::Context::new();
    for file in sorted {
        ctx.consume(file.as_bytes());
        ctx.consume([0u8]);
        let safe = !file.split(['/', '\\']).any(|s| s == "..") && !Path::new(file).is_absolute();
        let mut body = Vec::new();
        let read = safe
            && std::fs::File::open(root.join(file))
                .and_then(|f| f.take(MAX_FILE_BYTES).read_to_end(&mut body))
                .is_ok();
        if read {
            ctx.consume((body.len() as u64).to_le_bytes());
            ctx.consume(&body);
        } else {
            ctx.consume(b"\x01missing");
        }
        ctx.consume([0u8]);
    }
    format!("{:x}", ctx.finalize())
}

/// Alur `previous` masih sesuai isi repository di `root`, jadi tidak perlu
/// di-generate ulang. Alur tanpa meta, hasil cuplikan (`partial`), atau tanpa
/// daftar file selalu dianggap basi.
pub fn is_fresh(root: &Path, previous: Option<&FlowMeta>) -> bool {
    previous.is_some_and(|m| {
        !m.partial
            && !m.source_files.is_empty()
            && !m.source_hash.is_empty()
            && source_hash(root, &m.source_files) == m.source_hash
    })
}

/// Pisahkan card yang perlu di-generate dari yang masih segar.
pub fn select_stale(root: &Path, seeds: &[FlowSeed], force: bool) -> (Vec<FlowSeed>, usize) {
    let mut todo = Vec::new();
    let mut fresh = 0;
    for seed in seeds {
        if !force && is_fresh(root, seed.previous.as_ref()) {
            fresh += 1;
        } else {
            todo.push(seed.clone());
        }
    }
    (todo, fresh)
}

/// Keterangan asal-usul yang sama untuk semua flow satu job.
struct MetaBase {
    commit: Option<String>,
    generated_at: String,
    backend: String,
    partial: bool,
}

/// Lengkapi hasil AI: path file dan `source` langkah dibuat relatif, daftar
/// file dibatasi ke file yang ada di `root`, lalu `source_hash` dihitung.
fn finalize_flow(
    parsed: ParsedFlow,
    seed: &FlowSeed,
    root: &Path,
    roots: &[&Path],
    base: &MetaBase,
) -> GeneratedFlow {
    let mut steps = parsed.steps;
    for s in &mut steps {
        s.source = s.source.take().and_then(|src| relative_source(roots, &src));
    }
    let mut files: Vec<String> = Vec::new();
    let candidates = std::iter::once(seed.route_file().to_string())
        .chain(parsed.files)
        .chain(
            steps
                .iter()
                .filter_map(|s| s.source.as_deref().map(|src| source_file(src).to_string())),
        );
    for f in candidates {
        if let Some(rel) = relative_path(roots, &f)
            && !files.contains(&rel)
            && root.join(&rel).is_file()
        {
            files.push(rel);
        }
        if files.len() >= MAX_SOURCE_FILES {
            break;
        }
    }
    files.sort();
    let source_hash = if files.is_empty() {
        String::new()
    } else {
        source_hash(root, &files)
    };
    GeneratedFlow {
        card_id: parsed.card_id,
        summary: parsed.summary,
        steps,
        meta: FlowMeta {
            commit: base.commit.clone(),
            generated_at: base.generated_at.clone(),
            backend: base.backend.clone(),
            partial: base.partial,
            source_files: files,
            source_hash,
        },
    }
}

/// Commit HEAD pendek untuk ditampilkan; `None` bila bukan repository git.
fn head_commit(root: &Path) -> Option<String> {
    let out = crate::git::cli::run_text(root, &["rev-parse", "HEAD"]).ok()?;
    let sha = out.trim();
    (sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit())).then(|| sha.to_string())
}

// ─── Job background ─────────────────────────────────────────────────────────

/// Satu giliran AI: `(system, user, workspace, step_offset)` → teks jawaban.
/// Disuntikkan supaya job bisa diuji tanpa AI sungguhan.
type AskFn<'a> =
    dyn Fn(String, String, ChatWorkspace, u64) -> Result<String, RepoScanError> + Sync + 'a;

pub fn spawn_flow_scan(input: FlowScanInput) -> FlowScanHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let result = match input.backend.clone() {
            Some(backend) => {
                let ask = |system: String, user: String, ws: ChatWorkspace, offset: u64| {
                    repo_scan::ask_ai_with_offset(&backend, system, user, ws, &tx, &flag, offset)
                };
                run_flow_scan(&input, &backend, &tx, &flag, &ask)
            }
            None => Err(RepoScanError::Ai(
                "No AI backend is ready. Configure one in AI settings first.".into(),
            )),
        };
        let result = result.map_err(|e| match e {
            RepoScanError::Cancelled => "Cancelled".to_string(),
            other => {
                log::warn!("[DIAGRAM_FLOW] generation failed: {other}");
                other.to_string()
            }
        });
        let _ = tx.send(RepoJobEvent::Finished(result));
    });
    FlowScanHandle { rx, cancel }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), RepoScanError> {
    if cancel.load(Ordering::SeqCst) {
        Err(RepoScanError::Cancelled)
    } else {
        Ok(())
    }
}

fn run_flow_scan(
    input: &FlowScanInput,
    backend: &ChatBackend,
    tx: &mpsc::Sender<FlowEvent>,
    cancel: &AtomicBool,
    ask: &AskFn<'_>,
) -> Result<FlowScanOutcome, RepoScanError> {
    check_cancel(cancel)?;
    let source = repo_scan::choose_source(input.repo_path.as_deref(), input.repo_url.as_deref())?;
    let shown = match &source {
        RepoSource::Local(p) => p.display().to_string(),
        RepoSource::Remote(u) => repo_scan::redact(u),
    };
    let prep = match source {
        RepoSource::Remote(_) => "Cloning or updating repository",
        RepoSource::Local(_) => "Opening local folder",
    };
    let _ = tx.send(step(1, prep, Some(shown.clone()), ProgressStatus::Active));
    let resolved = match repo_scan::resolve_repo(&source, &input.cache_root, cancel) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(step(1, prep, Some(e.to_string()), ProgressStatus::Error));
            return Err(e);
        }
    };
    let _ = tx.send(step(1, prep, Some(shown), ProgressStatus::Done));
    check_cancel(cancel)?;

    let checking = "Checking which processes changed";
    let _ = tx.send(step(2, checking, None, ProgressStatus::Active));
    let (todo, skipped_fresh) = select_stale(&resolved.root, &input.cards, input.force);
    let _ = tx.send(step(
        2,
        checking,
        Some(format!(
            "{} to generate, {skipped_fresh} unchanged",
            todo.len()
        )),
        ProgressStatus::Done,
    ));
    log::info!(
        "[DIAGRAM_FLOW] {}: {} card(s) to generate, {skipped_fresh} unchanged",
        input.scope_name,
        todo.len()
    );
    if todo.is_empty() {
        return Ok(FlowScanOutcome {
            skipped_fresh,
            note: (skipped_fresh > 0)
                .then(|| "Every business process is up to date with the source code.".into()),
            ..Default::default()
        });
    }

    let (mode, workspace) =
        repo_scan::ai_workspace(backend, &resolved, &input.cache_root, tx, cancel, 3)?;
    let repo_root = workspace.cwd.clone();
    let partial = mode == PromptMode::Snippets;
    let base = MetaBase {
        commit: head_commit(&resolved.root),
        generated_at: chrono::Utc::now().to_rfc3339(),
        backend: input.backend_label.clone(),
        partial,
    };

    let batches =
        repo_endpoints::batches_by_file(&todo, FLOW_BATCH_SIZE, |s| s.route_file().to_string());
    let total = batches.len();
    let workers = input
        .parallel
        .clamp(1, MAX_PARALLEL_BATCHES)
        .min(total.max(1));
    let overview = format!(
        "Tracing {} endpoint(s) in {total} batch(es) with {}",
        todo.len(),
        input.backend_label
    );
    let _ = tx.send(step(
        4,
        overview.clone(),
        Some(format!("{workers} in parallel")),
        ProgressStatus::Active,
    ));
    let results = repo_endpoints::run_pool(total, workers, cancel, |i| {
        let batch = &batches[i];
        let idx = 10 + i as u64;
        let label = format!("Batch {}/{}", i + 1, total);
        let _slot = repo_endpoints::acquire_ai_slot(MAX_GLOBAL_AI_TURNS, cancel)?;
        let _ = tx.send(step(
            idx,
            label.clone(),
            Some(format!("{} endpoint(s)", batch.len())),
            ProgressStatus::Active,
        ));
        let snippets = if partial {
            let files: Vec<&str> = batch.iter().map(FlowSeed::route_file).collect();
            repo_endpoints::file_snippets(&resolved.root, &files)
        } else {
            String::new()
        };
        let (system, user) = build_flow_prompts(
            &input.scope_name,
            &input.tables,
            batch,
            mode,
            repo_root.as_deref().filter(|_| !partial),
            &snippets,
        );
        let ids: Vec<&str> = batch.iter().map(|s| s.card_id.as_str()).collect();
        // Langkah agent tiap batch diberi rentang nomor sendiri.
        let offset = 1_000 * (i as u64 + 1);
        let result = ask(system, user, workspace.clone(), offset)
            .and_then(|t| parse_flows_reply(&t, &ids, &input.tables));
        match &result {
            Ok(flows) => {
                let _ = tx.send(step(
                    idx,
                    label,
                    Some(format!("{} of {} traced", flows.len(), batch.len())),
                    ProgressStatus::Done,
                ));
            }
            Err(RepoScanError::Cancelled) => {}
            Err(e) => {
                log::warn!("[DIAGRAM_FLOW] batch {} failed: {e}", i + 1);
                let _ = tx.send(step(idx, label, Some(e.to_string()), ProgressStatus::Error));
            }
        }
        result
    });
    check_cancel(cancel)?;

    let roots: Vec<&Path> = std::iter::once(resolved.root.as_path())
        .chain(repo_root.as_deref())
        .collect();
    let mut outcome = FlowScanOutcome {
        skipped_fresh,
        ..Default::default()
    };
    for (i, result) in results {
        let batch = &batches[i];
        match result {
            Ok(parsed) => {
                let mut parsed = parsed;
                for seed in batch {
                    match parsed.iter().position(|p| p.card_id == seed.card_id) {
                        Some(pos) => outcome.flows.push(finalize_flow(
                            parsed.swap_remove(pos),
                            seed,
                            &resolved.root,
                            &roots,
                            &base,
                        )),
                        None => outcome.failed.push((
                            seed.card_id.clone(),
                            "The AI returned no steps for this endpoint.".into(),
                        )),
                    }
                }
            }
            Err(RepoScanError::Cancelled) => return Err(RepoScanError::Cancelled),
            Err(e) => {
                let msg = e.to_string();
                outcome
                    .failed
                    .extend(batch.iter().map(|s| (s.card_id.clone(), msg.clone())));
            }
        }
    }
    let _ = tx.send(step(
        4,
        overview,
        Some(format!(
            "{} traced, {} failed",
            outcome.flows.len(),
            outcome.failed.len()
        )),
        if outcome.flows.is_empty() {
            ProgressStatus::Error
        } else {
            ProgressStatus::Done
        },
    ));
    let mut notes: Vec<String> = Vec::new();
    if partial {
        notes.push(
            "The AI saw the route files only (it could not open the repository), so these \
             processes are partial."
                .into(),
        );
    }
    if !outcome.failed.is_empty() {
        notes.push(format!(
            "{} endpoint(s) could not be traced.",
            outcome.failed.len()
        ));
    }
    outcome.note = Some(notes.join(" ")).filter(|n| !n.is_empty());
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::Mutex;
    use std::time::Duration;

    use crate::agent::harness;
    use crate::config::{AiBackend, CliAgentKind};

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tabular-flow-gen-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("create temp repo");
            Self(dir)
        }

        fn write(&self, rel: &str, body: &str) {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(p, body).expect("write file");
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn seed(id: &str, method: &str, path: &str, source: &str) -> FlowSeed {
        FlowSeed {
            card_id: id.into(),
            method: method.into(),
            path: path.into(),
            source: Some(source.into()),
            ..Default::default()
        }
    }

    fn backend(kind: AiBackend, cli: CliAgentKind, model: &str) -> ChatBackend {
        ChatBackend {
            target: match kind {
                AiBackend::Api => crate::config::ChatTarget::Api,
                AiBackend::Cli => crate::config::ChatTarget::Cli(cli),
            },
            backend: kind,
            provider: crate::config::AiProvider::OpenAI,
            api_key: String::new(),
            model: String::new(),
            base_url: String::new(),
            cli: harness::CliAgentConfig {
                kind: cli,
                model: model.into(),
                ..Default::default()
            },
            mcp_available: false,
            notes_enabled: false,
            notes_writable: false,
        }
    }

    const REPLY: &str = r#"```json
{"flows":[
 {"id":"flw_1","summary":"Creates an order","files":["src/routes/orders.ts","./src/services/order.ts","../etc/passwd"],
  "steps":[
   {"kind":"auth","title":"Verify token","source":"src/middleware/auth.ts:12"},
   {"kind":"db","op":"read","table":"Users","columns":["id","status","id"],"title":"Load customer"},
   {"kind":"db","op":"insert","table":"ledger","title":"Write ledger"},
   {"kind":"queue","op":"publish","resource":"order.created","title":"Publish event"},
   {"kind":"cache","resource":"cart:{id}","title":"Clear cart"},
   {"kind":"external","op":"call","resource":"Stripe API","title":"Charge card","condition":"if total > 0"},
   {"kind":"mystery","op":"teleport","title":"Something new"},
   {"kind":"respond"}
  ]},
 {"id":"flw_9","summary":"Not in this batch","steps":[{"kind":"logic","title":"x"}]},
 {"id":"flw_2","summary":"Empty","steps":[]},
 {"id":"flw_1","summary":"Duplicate","steps":[{"kind":"logic","title":"dup"}]}
]}
```"#;

    #[test]
    fn parses_fenced_reply_and_maps_targets() {
        let tables = names(&["users", "orders"]);
        let flows = parse_flows_reply(REPLY, &["flw_1", "flw_2"], &tables).expect("parse");
        // flw_9 bukan anggota batch, flw_2 tanpa langkah, duplikat flw_1 dibuang.
        assert_eq!(flows.len(), 1);
        let f = &flows[0];
        assert_eq!(f.card_id, "flw_1");
        assert_eq!(f.summary, "Creates an order");
        assert_eq!(f.files.len(), 3);
        // Langkah respond tanpa judul maupun target dibuang.
        assert_eq!(f.steps.len(), 7);

        let s = &f.steps;
        assert_eq!(s[0].kind, FlowStepKind::Auth);
        assert_eq!(s[0].source.as_deref(), Some("src/middleware/auth.ts:12"));
        // Nama tabel dicocokkan tanpa membedakan huruf besar-kecil; kolom unik.
        assert_eq!(s[1].target, Some(FlowTarget::Table("users".into())));
        assert_eq!(s[1].op, Some(FlowOp::Read));
        assert_eq!(s[1].columns, names(&["id", "status"]));
        // Tabel yang tidak ada di diagram: tanpa target, nama masuk detail.
        assert_eq!(s[2].target, None);
        assert!(s[2].detail.contains("ledger"), "{}", s[2].detail);
        assert_eq!(s[3].target, Some(FlowTarget::Queue("order.created".into())));
        assert_eq!(s[4].target, Some(FlowTarget::Cache("cart:{id}".into())));
        assert_eq!(s[5].target, Some(FlowTarget::External("Stripe API".into())));
        assert_eq!(s[5].condition.as_deref(), Some("if total > 0"));
        // Jenis dan operasi tak dikenal.
        assert_eq!(s[6].kind, FlowStepKind::Logic);
        assert_eq!(s[6].op, None);
    }

    #[test]
    fn parses_bare_array_and_rejects_prose() {
        let reply = r#"Here you go: [{"id":"flw_4","steps":[{"kind":"db","op":"delete","table":"orders","title":"Remove"}]}]"#;
        let flows = parse_flows_reply(reply, &["flw_4"], &names(&["orders"])).expect("parse");
        assert_eq!(flows[0].steps[0].op, Some(FlowOp::Delete));
        assert!(matches!(
            parse_flows_reply("I could not find it.", &["flw_4"], &[]),
            Err(RepoScanError::Parse(_))
        ));
        // Diagram tanpa tabel: tidak ada target tabel sama sekali.
        let flows = parse_flows_reply(reply, &["flw_4"], &[]).expect("parse");
        assert_eq!(flows[0].steps[0].target, None);
    }

    #[test]
    fn enforces_size_limits() {
        let long = "x".repeat(1_000);
        let steps: Vec<String> = (0..40)
            .map(|i| {
                let cols: Vec<String> = (0..30).map(|c| format!("\"c{c}\"")).collect();
                format!(
                    r#"{{"kind":"db","title":"{long}{i}","detail":"{long}","condition":"{long}","columns":[{}]}}"#,
                    cols.join(",")
                )
            })
            .collect();
        let reply = format!(
            r#"{{"flows":[{{"id":"flw_1","summary":"{long}","steps":[{}]}}]}}"#,
            steps.join(",")
        );
        let flows = parse_flows_reply(&reply, &["flw_1"], &[]).expect("parse");
        let f = &flows[0];
        assert_eq!(f.steps.len(), MAX_STEPS);
        assert!(f.summary.chars().count() <= MAX_SUMMARY_CHARS + 1);
        for s in &f.steps {
            assert!(s.title.chars().count() <= MAX_TITLE_CHARS + 1);
            assert!(s.detail.chars().count() <= MAX_DETAIL_CHARS + 1);
            assert!(
                s.condition
                    .as_ref()
                    .is_some_and(|c| c.chars().count() <= MAX_SHORT_CHARS + 1)
            );
            assert_eq!(s.columns.len(), MAX_STEP_COLUMNS);
        }
    }

    #[test]
    fn prompts_list_batch_tables_and_snippets() {
        let mut a = seed("flw_1", "POST", "/orders", "src/routes/orders.ts:31");
        a.known_tables = names(&["orders"]);
        let b = FlowSeed {
            card_id: "flw_2".into(),
            method: "GET".into(),
            path: "/health".into(),
            ..Default::default()
        };
        let (system, user) = build_flow_prompts(
            "Shop API",
            &names(&["orders", "users"]),
            &[a, b],
            PromptMode::Snippets,
            None,
            "\n=== src/routes/orders.ts ===\n    1 router.post()\n",
        );
        assert!(system.contains("at most 25 per endpoint"));
        assert!(system.contains("when they match: orders, users"));
        assert!(system.contains("You cannot open files"));
        assert!(system.contains(r#""flows""#));
        assert!(user.contains("Scope: Shop API"));
        assert!(
            user.contains("- flw_1: POST /orders (src/routes/orders.ts:31) known tables: orders")
        );
        assert!(user.contains("- flw_2: GET /health\n"));
        assert!(user.contains("Source code:"));

        let root = Path::new("/tmp/repo");
        let (system, user) =
            build_flow_prompts("S", &[], &[], PromptMode::ReadRepo, Some(root), "");
        assert!(system.contains("/tmp/repo"));
        assert!(!system.contains("when they match"));
        assert!(!user.contains("Source code:"));
    }

    #[test]
    fn normalizes_paths_against_roots() {
        let root = Path::new("/work/repo");
        let copy = Path::new("/cache/copy");
        let roots = [root, copy];
        assert_eq!(
            relative_path(&roots, "./src/a.ts").as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            relative_path(&roots, "/work/repo/src/a.ts").as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(
            relative_path(&roots, "/cache/copy/b.rs").as_deref(),
            Some("b.rs")
        );
        assert_eq!(relative_path(&roots, "/etc/passwd"), None);
        assert_eq!(relative_path(&roots, "../x"), None);
        assert_eq!(relative_path(&roots, "  "), None);
        assert_eq!(
            relative_source(&roots, "/work/repo/src/a.ts:12").as_deref(),
            Some("src/a.ts:12")
        );
        assert_eq!(
            relative_source(&roots, "src/a.ts").as_deref(),
            Some("src/a.ts")
        );
        assert_eq!(relative_source(&roots, "/home/me/x.ts:3"), None);
    }

    #[test]
    fn hash_tracks_content_missing_files_and_ignores_order() {
        let repo = TempRepo::new("hash");
        repo.write("a.ts", "one");
        repo.write("b.ts", "two");
        let files = names(&["a.ts", "b.ts"]);
        let h = source_hash(&repo.0, &files);
        assert_eq!(h, source_hash(&repo.0, &names(&["b.ts", "a.ts", "a.ts"])));
        repo.write("b.ts", "two!");
        let changed = source_hash(&repo.0, &files);
        assert_ne!(h, changed);
        std::fs::remove_file(repo.0.join("b.ts")).expect("remove");
        assert_ne!(changed, source_hash(&repo.0, &files));
        // Path yang keluar dari repository tidak dibaca.
        assert_eq!(
            source_hash(&repo.0, &names(&["../x"])),
            source_hash(&repo.0, &names(&["../x"]))
        );
    }

    #[test]
    fn selects_stale_cards() {
        let repo = TempRepo::new("stale");
        repo.write("src/a.ts", "a");
        let files = names(&["src/a.ts"]);
        let fresh_meta = FlowMeta {
            source_files: files.clone(),
            source_hash: source_hash(&repo.0, &files),
            ..Default::default()
        };
        let mut fresh = seed("flw_1", "GET", "/a", "src/a.ts:1");
        fresh.previous = Some(fresh_meta.clone());
        let mut partial = seed("flw_2", "GET", "/b", "src/a.ts:2");
        partial.previous = Some(FlowMeta {
            partial: true,
            ..fresh_meta.clone()
        });
        let never = seed("flw_3", "GET", "/c", "src/a.ts:3");
        let mut changed = seed("flw_4", "GET", "/d", "src/a.ts:4");
        changed.previous = Some(FlowMeta {
            source_hash: "0".repeat(32),
            ..fresh_meta
        });
        let seeds = vec![fresh, partial, never, changed];

        let (todo, skipped) = select_stale(&repo.0, &seeds, false);
        assert_eq!(skipped, 1);
        let ids: Vec<&str> = todo.iter().map(|s| s.card_id.as_str()).collect();
        assert_eq!(ids, ["flw_2", "flw_3", "flw_4"]);

        let (todo, skipped) = select_stale(&repo.0, &seeds, true);
        assert_eq!((todo.len(), skipped), (4, 0));

        // File berubah membuat card yang tadinya segar menjadi basi.
        repo.write("src/a.ts", "a changed");
        let (todo, skipped) = select_stale(&repo.0, &seeds[..1], false);
        assert_eq!((todo.len(), skipped), (1, 0));
    }

    fn fixture_repo() -> TempRepo {
        let repo = TempRepo::new("job");
        repo.write(
            "src/routes/orders.ts",
            "router.post('/orders', auth, create);\nrouter.get('/orders/:id', show);\n",
        );
        repo.write(
            "src/services/order.ts",
            "export async function create() { await db.insert('orders'); }\n",
        );
        repo
    }

    fn input(repo: &TempRepo, cache: &TempRepo, cards: Vec<FlowSeed>) -> FlowScanInput {
        FlowScanInput {
            repo_path: Some(repo.0.to_string_lossy().to_string()),
            repo_url: None,
            scope_name: "Shop".into(),
            cards,
            tables: names(&["orders", "users"]),
            backend: None,
            backend_label: "Test API".into(),
            cache_root: cache.0.clone(),
            parallel: 2,
            force: false,
        }
    }

    const JOB_REPLY: &str = r#"{"flows":[{"id":"flw_1","summary":"Creates an order","files":["src/services/order.ts","missing.ts"],"steps":[{"kind":"db","op":"insert","table":"orders","title":"Insert order","source":"src/services/order.ts:1"}]}]}"#;

    #[test]
    fn job_generates_skips_fresh_and_reports_failures() {
        let repo = fixture_repo();
        let cache = TempRepo::new("job-cache");
        let files = names(&["src/routes/orders.ts"]);
        let mut fresh = seed("flw_3", "DELETE", "/orders/{id}", "src/routes/orders.ts:3");
        fresh.previous = Some(FlowMeta {
            source_files: files.clone(),
            source_hash: source_hash(&repo.0, &files),
            ..Default::default()
        });
        let cards = vec![
            seed("flw_1", "POST", "/orders", "src/routes/orders.ts:1"),
            seed("flw_2", "GET", "/orders/{id}", "src/routes/orders.ts:2"),
            fresh,
        ];
        let prompts: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let ask = |_s: String, user: String, _ws: ChatWorkspace, _o: u64| {
            prompts.lock().expect("lock").push(user);
            Ok(JOB_REPLY.to_string())
        };
        let (tx, rx) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let be = backend(AiBackend::Api, CliAgentKind::ClaudeCode, "");
        let out = run_flow_scan(&input(&repo, &cache, cards), &be, &tx, &cancel, &ask)
            .expect("job succeeds");

        assert_eq!(out.skipped_fresh, 1);
        assert_eq!(out.flows.len(), 1);
        assert_eq!(out.failed.len(), 1);
        assert_eq!(out.failed[0].0, "flw_2");
        let f = &out.flows[0];
        assert_eq!(f.card_id, "flw_1");
        assert_eq!(f.steps[0].target, Some(FlowTarget::Table("orders".into())));
        // Backend API hanya menerima cuplikan: alurnya parsial.
        assert!(f.meta.partial);
        assert_eq!(f.meta.backend, "Test API");
        assert!(!f.meta.generated_at.is_empty());
        // File route selalu ikut; file yang tidak ada dibuang.
        assert_eq!(
            f.meta.source_files,
            names(&["src/routes/orders.ts", "src/services/order.ts"])
        );
        assert_eq!(
            f.meta.source_hash,
            source_hash(&repo.0, &f.meta.source_files)
        );
        assert!(out.note.as_deref().is_some_and(|n| n.contains("partial")));

        // Satu batch (file route sama) berisi cuplikan kode yang sudah diredaksi.
        let prompts = prompts.into_inner().expect("lock");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("=== src/routes/orders.ts ==="));
        assert!(!prompts[0].contains("flw_3"));
        drop(tx);
        let steps: Vec<FlowEvent> = rx.try_iter().collect();
        assert!(steps.iter().any(|e| matches!(e, RepoJobEvent::Progress(_))));
    }

    #[test]
    fn job_with_nothing_stale_skips_ai() {
        let repo = fixture_repo();
        let cache = TempRepo::new("fresh-cache");
        let files = names(&["src/routes/orders.ts"]);
        let mut s = seed("flw_1", "POST", "/orders", "src/routes/orders.ts:1");
        s.previous = Some(FlowMeta {
            source_files: files.clone(),
            source_hash: source_hash(&repo.0, &files),
            ..Default::default()
        });
        let ask =
            |_: String, _: String, _: ChatWorkspace, _: u64| -> Result<String, RepoScanError> {
                panic!("AI must not be asked")
            };
        let (tx, _rx) = mpsc::channel();
        let be = backend(AiBackend::Api, CliAgentKind::ClaudeCode, "");
        let out = run_flow_scan(
            &input(&repo, &cache, vec![s]),
            &be,
            &tx,
            &AtomicBool::new(false),
            &ask,
        )
        .expect("job succeeds");
        assert_eq!((out.flows.len(), out.skipped_fresh), (0, 1));
        assert!(out.note.is_some());
    }

    #[test]
    fn failed_batch_marks_its_cards_but_keeps_others() {
        let repo = fixture_repo();
        let cache = TempRepo::new("fail-cache");
        repo.write("src/routes/users.ts", "router.get('/users', list);\n");
        let cards = vec![
            seed("flw_1", "POST", "/orders", "src/routes/orders.ts:1"),
            seed("flw_5", "GET", "/users", "src/routes/users.ts:1"),
        ];
        // File berbeda tetapi muat satu batch; paksa dua batch dengan 7 card.
        let mut cards = cards;
        for n in 10..16 {
            cards.push(seed(
                &format!("flw_{n}"),
                "GET",
                "/users",
                "src/routes/users.ts:1",
            ));
        }
        let ask = |_: String, user: String, _: ChatWorkspace, _: u64| {
            if user.contains("flw_1:") {
                Ok(JOB_REPLY.to_string())
            } else {
                Err(RepoScanError::Ai("model overloaded".into()))
            }
        };
        let (tx, _rx) = mpsc::channel();
        let be = backend(AiBackend::Api, CliAgentKind::ClaudeCode, "");
        let out = run_flow_scan(
            &input(&repo, &cache, cards),
            &be,
            &tx,
            &AtomicBool::new(false),
            &ask,
        )
        .expect("job succeeds");
        assert_eq!(out.flows.len(), 1);
        assert!(out.failed.iter().any(|(_, m)| m.contains("overloaded")));
        assert!(!out.failed.iter().any(|(id, _)| id == "flw_1"));
    }

    #[test]
    fn cancellation_stops_the_job() {
        let repo = fixture_repo();
        let cache = TempRepo::new("cancel-cache");
        let be = backend(AiBackend::Api, CliAgentKind::ClaudeCode, "");
        let cards = || vec![seed("flw_1", "POST", "/orders", "src/routes/orders.ts:1")];
        let (tx, _rx) = mpsc::channel();

        // Dibatalkan sebelum mulai: AI tidak pernah dipanggil.
        let never =
            |_: String, _: String, _: ChatWorkspace, _: u64| -> Result<String, RepoScanError> {
                panic!("AI must not be asked")
            };
        let cancel = AtomicBool::new(true);
        assert!(matches!(
            run_flow_scan(&input(&repo, &cache, cards()), &be, &tx, &cancel, &never),
            Err(RepoScanError::Cancelled)
        ));

        // Dibatalkan saat AI bekerja: hasil parsial tidak dikembalikan.
        let cancel = AtomicBool::new(false);
        let ask = |_: String, _: String, _: ChatWorkspace, _: u64| {
            cancel.store(true, Ordering::SeqCst);
            Ok(JOB_REPLY.to_string())
        };
        assert!(matches!(
            run_flow_scan(&input(&repo, &cache, cards()), &be, &tx, &cancel, &ask),
            Err(RepoScanError::Cancelled)
        ));
    }

    #[test]
    fn spawn_without_backend_finishes_with_error() {
        let repo = fixture_repo();
        let cache = TempRepo::new("spawn-cache");
        let handle = spawn_flow_scan(input(&repo, &cache, vec![]));
        match handle.rx.recv_timeout(Duration::from_secs(10)) {
            Ok(RepoJobEvent::Finished(Err(e))) => assert!(e.contains("No AI backend"), "{e}"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    #[ignore = "memanggil CLI Claude Code sungguhan"]
    fn real_claude_traces_business_process() {
        let repo = TempRepo::new("real");
        repo.write(
            "src/routes/orders.js",
            "const express = require('express');\n\
             const { requireUser } = require('../middleware/auth');\n\
             const orders = require('../services/orders');\n\
             const router = express.Router();\n\
             router.post('/orders', requireUser, async (req, res) => {\n\
             \x20 const order = await orders.create(req.user.id, req.body.items);\n\
             \x20 res.status(201).json(order);\n\
             });\n\
             module.exports = router;\n",
        );
        repo.write(
            "src/middleware/auth.js",
            "exports.requireUser = (req, res, next) => {\n\
             \x20 if (!req.headers.authorization) return res.status(401).end();\n\
             \x20 next();\n};\n",
        );
        repo.write(
            "src/services/orders.js",
            "const db = require('../db');\n\
             exports.create = async (userId, items) => {\n\
             \x20 const user = await db.query('SELECT id, status FROM users WHERE id = $1', [userId]);\n\
             \x20 const { rows } = await db.query('INSERT INTO orders (user_id, total) VALUES ($1, $2) RETURNING *', [userId, items.length]);\n\
             \x20 return rows[0];\n};\n",
        );
        let git = |args: &[&str]| {
            let _ = Command::new("git")
                .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
                .args(args)
                .current_dir(&repo.0)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        };
        git(&["init", "--quiet"]);
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "fixture"]);
        let cache = TempRepo::new("real-cache");
        let mut inp = input(
            &repo,
            &cache,
            vec![seed("flw_1", "POST", "/orders", "src/routes/orders.js:5")],
        );
        inp.backend = Some(backend(AiBackend::Cli, CliAgentKind::ClaudeCode, "haiku"));
        inp.backend_label = "Claude Code".into();
        let handle = spawn_flow_scan(inp);
        let out = loop {
            match handle.rx.recv_timeout(Duration::from_secs(600)) {
                Ok(RepoJobEvent::Progress(p)) => {
                    eprintln!("[progress] {} {:?}", p.description, p.detail)
                }
                Ok(RepoJobEvent::Activity) => {}
                Ok(RepoJobEvent::Finished(r)) => break r.expect("generation succeeds"),
                Err(e) => panic!("generation did not finish: {e}"),
            }
        };
        eprintln!("{out:#?}");
        assert!(out.failed.is_empty(), "{:?}", out.failed);
        let f = &out.flows[0];
        assert!(!f.meta.partial);
        assert!(f.meta.commit.is_some());
        assert!(!f.meta.source_hash.is_empty());
        let tables: Vec<&str> = f
            .steps
            .iter()
            .filter_map(|s| s.target.as_ref()?.table())
            .collect();
        assert!(tables.contains(&"orders"), "{tables:?}");
        assert!(tables.contains(&"users"), "{tables:?}");
        assert!(
            f.steps.iter().any(|s| s.kind == FlowStepKind::Auth),
            "{:?}",
            f.steps
        );
    }
}
