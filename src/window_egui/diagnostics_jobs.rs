//! Diagnostik live Query Editor: debounce perubahan teks, analisis lokal di
//! thread background, validasi server (prepare tanpa eksekusi) lewat runtime
//! bersama, penanda error eksekusi terakhir, lalu penggabungan hasil yang
//! digambar editor (`diagnostics_ui`).
//!
//! Tidak ada I/O di render path: `tick` hanya membaca cache in-memory dan
//! memulai job; hasilnya datang lewat channel.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use eframe::egui;

use super::Tabular;
use crate::connection::validate::{ValidationOutcome, ValidationTarget, validate_statement};
use crate::models::enums::{DatabasePool, DatabaseType};
use crate::query_diagnostics::locate::{ServerPosition, locate, tidy_server_message};
use crate::query_diagnostics::{
    self as qd, Analysis, DiagCode, DiagSource, QueryLanguage, ServerVerdict, StatementInfo,
};
use crate::query_tools::{LintMessage, LintSeverity};

/// Jeda setelah ketikan terakhir sebelum analisis lokal.
const LOCAL_DEBOUNCE: Duration = Duration::from_millis(350);
/// Jeda tambahan sebelum statement dikirim ke server.
const SERVER_DEBOUNCE: Duration = Duration::from_millis(700);
const MAX_SERVER_IN_FLIGHT: usize = 2;
const SERVER_CACHE_LIMIT: usize = 512;
const BUSY_RETRY: Duration = Duration::from_millis(1500);

/// Kapan statement divalidasi di server saat mengetik.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerValidationMode {
    Off,
    /// Semua koneksi kecuali yang berlabel Production.
    #[default]
    NonProduction,
    Always,
}

impl ServerValidationMode {
    pub const ALL: [ServerValidationMode; 3] = [Self::NonProduction, Self::Always, Self::Off];

    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::NonProduction => "non_production",
            Self::Always => "always",
        }
    }

    pub fn parse(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|m| m.key() == key.trim())
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Never (local checks only)",
            Self::NonProduction => "Except Production connections",
            Self::Always => "All connections",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ContextKey {
    connection_id: Option<i64>,
    database: String,
    schema: String,
    lang: QueryLanguage,
}

enum DiagMsg {
    Local {
        generation: u64,
        analysis: Analysis,
        fingerprint: (usize, usize),
    },
    Server {
        key: ContextKey,
        hash: u64,
        outcome: ValidationOutcome,
    },
    Collections {
        connection_id: i64,
        collections: Vec<String>,
    },
}

pub struct DiagnosticsState {
    /// Diagnostik tab aktif yang digambar editor (rentang byte di teks editor).
    pub items: Vec<LintMessage>,
    /// Daftar Problems di bawah editor terbuka.
    pub panel_open: bool,
    /// Lokasi error eksekusi terakhir di teks editor.
    pub execution_span: Option<Range<usize>>,
    tab_id: Option<usize>,
    context: Option<ContextKey>,
    seen_text: String,
    seen_key: (u64, usize),
    last_change: Instant,
    dirty: bool,
    generation: u64,
    local_in_flight: bool,
    local: Vec<LintMessage>,
    statements: Vec<StatementInfo>,
    missing: Vec<String>,
    analyzed_fingerprint: (usize, usize),
    last_fingerprint_check: Instant,
    server: HashMap<(ContextKey, u64), ServerVerdict>,
    server_pending: HashSet<(ContextKey, u64)>,
    server_retry_at: Option<Instant>,
    server_active: bool,
    requested_tables: HashSet<(i64, String)>,
    collections: HashMap<i64, Vec<String>>,
    collections_requested: HashSet<i64>,
    needs_merge: bool,
    /// Kunci perhitungan penanda eksekusi terakhir (teks + error).
    exec_key: Option<(u64, usize, u64)>,
    tx: Sender<DiagMsg>,
    rx: Receiver<DiagMsg>,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            items: Vec::new(),
            panel_open: false,
            execution_span: None,
            tab_id: None,
            context: None,
            seen_text: String::new(),
            seen_key: (u64::MAX, usize::MAX),
            last_change: Instant::now(),
            dirty: false,
            generation: 0,
            local_in_flight: false,
            local: Vec::new(),
            statements: Vec::new(),
            missing: Vec::new(),
            analyzed_fingerprint: (0, 0),
            last_fingerprint_check: Instant::now(),
            server: HashMap::new(),
            server_pending: HashSet::new(),
            server_retry_at: None,
            server_active: false,
            requested_tables: HashSet::new(),
            collections: HashMap::new(),
            collections_requested: HashSet::new(),
            needs_merge: false,
            exec_key: None,
            tx,
            rx,
        }
    }
}

impl DiagnosticsState {
    fn clear(&mut self) {
        self.items.clear();
        self.local.clear();
        self.statements.clear();
        self.missing.clear();
        self.execution_span = None;
        self.exec_key = None;
        self.tab_id = None;
        self.context = None;
        self.generation += 1;
        self.dirty = false;
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for m in &self.items {
            match m.severity {
                LintSeverity::Error => c.0 += 1,
                LintSeverity::Warning => c.1 += 1,
                LintSeverity::Info => c.2 += 1,
            }
        }
        c
    }

    /// Diagnostik pertama yang rentangnya memuat `pos`.
    pub fn at(&self, pos: usize) -> Option<&LintMessage> {
        self.items.iter().find(|m| {
            m.span
                .as_ref()
                .is_some_and(|r| r.start <= pos && pos <= r.end.max(r.start + 1))
        })
    }

    /// Lokasi berikutnya/sebelumnya dari `pos` (berputar).
    pub fn next_from(&self, pos: usize, forward: bool) -> Option<&LintMessage> {
        let with_span = || self.items.iter().filter(|m| m.span.is_some());
        let start = |m: &&LintMessage| m.span.as_ref().map_or(0, |r| r.start);
        if forward {
            with_span()
                .filter(|m| start(m) > pos)
                .min_by_key(start)
                .or_else(|| with_span().min_by_key(start))
        } else {
            with_span()
                .filter(|m| start(m) < pos)
                .max_by_key(start)
                .or_else(|| with_span().max_by_key(start))
        }
    }
}

/// Bahasa teks tab aktif, atau `None` bila tab ini bukan editor query.
fn language_for(tabular: &Tabular) -> Option<(QueryLanguage, Option<i64>)> {
    let tab = tabular.query_tabs.get(tabular.active_tab_index)?;
    if tab.http_client_state.is_some() || tab.diagram_state.is_some() {
        return None;
    }
    if let Some(path) = &tab.file_path
        && !path.to_ascii_lowercase().ends_with(".sql")
    {
        return None;
    }
    let cid = tab.connection_id.or(tabular.current_connection_id);
    let Some(conn) = cid.and_then(|id| tabular.connections.iter().find(|c| c.id == Some(id)))
    else {
        return Some((QueryLanguage::Sql(qd_dialect_generic()), None));
    };
    let lang = match conn.connection_type {
        DatabaseType::Redis => QueryLanguage::Redis,
        DatabaseType::MongoDB => QueryLanguage::Mongo,
        DatabaseType::ApiHttp => return None,
        _ => QueryLanguage::Sql(crate::editor_autocomplete_new::dialect_for(tabular, cid)?),
    };
    Some((lang, cid))
}

fn qd_dialect_generic() -> crate::autocomplete::lexer::Dialect {
    crate::autocomplete::lexer::Dialect::Generic
}

fn pool_for(tabular: &Tabular, cid: i64) -> Option<DatabasePool> {
    tabular.connection_pools.get(&cid).cloned().or_else(|| {
        crate::connection::pool::lock_or_recover(&tabular.shared_connection_pools)
            .get(&cid)
            .cloned()
    })
}

/// Validasi server aktif untuk koneksi ini (mode preferensi + label environment).
fn server_allowed(tabular: &Tabular, cid: Option<i64>, lang: QueryLanguage) -> bool {
    let Some(cid) = cid else {
        return false;
    };
    if !matches!(lang, QueryLanguage::Sql(_)) {
        return false;
    }
    let Some(conn) = tabular.connections.iter().find(|c| c.id == Some(cid)) else {
        return false;
    };
    if !matches!(
        conn.connection_type,
        DatabaseType::PostgreSQL | DatabaseType::MySQL | DatabaseType::SQLite | DatabaseType::MsSQL
    ) {
        return false;
    }
    match tabular.diagnostics_server_mode {
        ServerValidationMode::Off => false,
        ServerValidationMode::Always => true,
        ServerValidationMode::NonProduction => {
            tabular.connection_environment_by_id(cid)
                != Some(crate::connection_env::Environment::Production)
        }
    }
}

/// Dipanggil setiap frame editor dirender.
pub(crate) fn tick(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.live_diagnostics {
        if !tabular.diagnostics.items.is_empty() || tabular.diagnostics.tab_id.is_some() {
            tabular.diagnostics.clear();
        }
        return;
    }
    let Some((lang, cid)) = language_for(tabular) else {
        if tabular.diagnostics.tab_id.is_some() {
            tabular.diagnostics.clear();
        }
        return;
    };
    let Some(tab) = tabular.query_tabs.get(tabular.active_tab_index) else {
        return;
    };
    let tab_id = tab.id;
    let context = ContextKey {
        connection_id: cid,
        database: tab.database_name.clone().unwrap_or_default(),
        schema: tab.schema_name.clone().unwrap_or_default(),
        lang,
    };
    let now = Instant::now();

    drain(tabular);

    // Perubahan tab/konteks atau teks.
    let text_key = (tabular.editor.revision, tabular.editor.text.len());
    let d = &mut tabular.diagnostics;
    if d.tab_id != Some(tab_id) || d.context.as_ref() != Some(&context) {
        d.clear();
        d.tab_id = Some(tab_id);
        d.context = Some(context.clone());
        d.seen_text = tabular.editor.text.clone();
        d.seen_key = text_key;
        d.dirty = true;
        d.last_change = now.checked_sub(LOCAL_DEBOUNCE).unwrap_or(now);
        d.needs_merge = true;
    } else if text_key != d.seen_key {
        d.seen_key = text_key;
        if tabular.editor.text != d.seen_text {
            let new = tabular.editor.text.clone();
            qd::rebase(&d.seen_text, &new, &mut d.local);
            // Statement yang tersunting keluar dari daftar (hasil servernya tidak
            // berlaku); tanda tentatif di dalamnya ditahan sampai analisis baru.
            qd::rebase_statements(&d.seen_text, &new, &mut d.statements);
            d.seen_text = new;
            d.dirty = true;
            d.last_change = now;
            d.generation += 1;
            d.needs_merge = true;
        }
    }

    // Cache skema selesai dimuat setelah analisis terakhir: analisis ulang.
    if !d.dirty && now.duration_since(d.last_fingerprint_check) > Duration::from_millis(500) {
        d.last_fingerprint_check = now;
        let fp = crate::editor_autocomplete_new::diagnostics_schema_fingerprint(tabular);
        let d = &mut tabular.diagnostics;
        if fp != d.analyzed_fingerprint && matches!(lang, QueryLanguage::Sql(_)) {
            d.dirty = true;
        }
    }

    let d = &tabular.diagnostics;
    let since_change = now.duration_since(d.last_change);
    if d.dirty && !d.local_in_flight {
        if since_change >= LOCAL_DEBOUNCE {
            start_local(tabular, lang, cid, ctx);
        } else {
            ctx.request_repaint_after(LOCAL_DEBOUNCE - since_change);
        }
    }

    // Validasi server untuk statement yang hasil lokalnya sudah terkini.
    let allowed =
        server_allowed(tabular, cid, lang) && cid.and_then(|c| pool_for(tabular, c)).is_some();
    if tabular.diagnostics.server_active != allowed {
        tabular.diagnostics.server_active = allowed;
        tabular.diagnostics.needs_merge = true;
    }
    let d = &tabular.diagnostics;
    if allowed && !d.dirty && !d.local_in_flight && since_change >= SERVER_DEBOUNCE {
        start_server(tabular, &context, cid, ctx);
    }

    update_execution_marker(tabular, tab_id, lang);

    if tabular.diagnostics.needs_merge {
        merge(tabular, &context);
    }
    let d = &tabular.diagnostics;
    if d.local_in_flight || !d.server_pending.is_empty() {
        ctx.request_repaint_after(Duration::from_millis(150));
    }
}

fn drain(tabular: &mut Tabular) {
    let d = &mut tabular.diagnostics;
    while let Ok(msg) = d.rx.try_recv() {
        match msg {
            DiagMsg::Local {
                generation,
                analysis,
                fingerprint,
            } => {
                d.local_in_flight = false;
                if generation == d.generation {
                    d.local = analysis.items;
                    d.statements = analysis.statements;
                    d.missing = analysis.missing_columns;
                    d.analyzed_fingerprint = fingerprint;
                    d.needs_merge = true;
                }
            }
            DiagMsg::Server { key, hash, outcome } => {
                d.server_pending.remove(&(key.clone(), hash));
                match outcome {
                    ValidationOutcome::Busy => {
                        d.server_retry_at = Some(Instant::now() + BUSY_RETRY)
                    }
                    ValidationOutcome::Verdict(v) => {
                        if d.server.len() >= SERVER_CACHE_LIMIT {
                            d.server.clear();
                        }
                        d.server.insert((key, hash), v);
                        d.needs_merge = true;
                    }
                }
            }
            DiagMsg::Collections {
                connection_id,
                collections,
            } => {
                d.collections.insert(connection_id, collections);
                d.dirty = true;
            }
        }
    }
}

fn start_local(tabular: &mut Tabular, lang: QueryLanguage, cid: Option<i64>, ctx: &egui::Context) {
    let missing = std::mem::take(&mut tabular.diagnostics.missing);
    let mut requested = std::mem::take(&mut tabular.diagnostics.requested_tables);
    let mut schema = match lang {
        QueryLanguage::Sql(_) => {
            crate::editor_autocomplete_new::diagnostics_snapshot(tabular, &missing, &mut requested)
        }
        _ => Default::default(),
    };
    tabular.diagnostics.requested_tables = requested;
    tabular.diagnostics.missing = missing;
    if lang == QueryLanguage::Mongo
        && let Some(cid) = cid
    {
        schema.collections = tabular.diagnostics.collections.get(&cid).cloned();
        request_collections(tabular, cid);
    }
    let fingerprint = crate::editor_autocomplete_new::diagnostics_schema_fingerprint(tabular);
    let d = &mut tabular.diagnostics;
    d.dirty = false;
    d.local_in_flight = true;
    let generation = d.generation;
    let text = tabular.editor.text.clone();
    let tx = d.tx.clone();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("query-diagnostics".into())
        .spawn(move || {
            let analysis = qd::analyze(&text, lang, &schema);
            let _ = tx.send(DiagMsg::Local {
                generation,
                analysis,
                fingerprint,
            });
            ctx.request_repaint();
        });
    if let Err(e) = spawned {
        log::warn!("[DIAG] cannot start the diagnostics thread: {e}");
        tabular.diagnostics.local_in_flight = false;
    }
}

fn request_collections(tabular: &mut Tabular, cid: i64) {
    if !tabular.diagnostics.collections_requested.insert(cid) {
        return;
    }
    let (Some(rt), Some(pool)) = (tabular.runtime.clone(), tabular.db_pool.clone()) else {
        return;
    };
    let tx = tabular.diagnostics.tx.clone();
    rt.spawn(async move {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT table_name FROM table_cache WHERE connection_id = ? AND table_type = 'collection'",
        )
        .bind(cid)
        .fetch_all(pool.as_ref())
        .await;
        match rows {
            Ok(rows) if !rows.is_empty() => {
                let _ = tx.send(DiagMsg::Collections {
                    connection_id: cid,
                    collections: rows.into_iter().map(|(c,)| c).collect(),
                });
            }
            Ok(_) => {}
            Err(e) => log::debug!("[DIAG] cannot read cached collections: {e}"),
        }
    });
}

fn start_server(
    tabular: &mut Tabular,
    context: &ContextKey,
    cid: Option<i64>,
    ctx: &egui::Context,
) {
    let now = Instant::now();
    let d = &tabular.diagnostics;
    if d.server_retry_at.is_some_and(|t| t > now) || d.server_pending.len() >= MAX_SERVER_IN_FLIGHT
    {
        return;
    }
    let Some(cid) = cid else {
        return;
    };
    let cursor = tabular.cursor_position;
    // Statement di kursor dulu, lalu urutan teks.
    let mut todo: Vec<&StatementInfo> = d
        .statements
        .iter()
        .filter(|s| s.validatable)
        .filter(|s| {
            let key = (context.clone(), s.hash);
            !d.server.contains_key(&key) && !d.server_pending.contains(&key)
        })
        .collect();
    if todo.is_empty() {
        return;
    }
    todo.sort_by_key(|s| !(s.span.start <= cursor && cursor <= s.span.end));
    let Some(pool) = pool_for(tabular, cid) else {
        return;
    };
    let Some(rt) = tabular.runtime.clone() else {
        return;
    };
    let slots = MAX_SERVER_IN_FLIGHT - d.server_pending.len();
    let jobs: Vec<(u64, String)> = todo
        .into_iter()
        .take(slots)
        .filter_map(|s| {
            tabular
                .editor
                .text
                .get(s.span.clone())
                .map(|t| (s.hash, t.to_string()))
        })
        .collect();
    let target = ValidationTarget {
        database: Some(context.database.clone()).filter(|s| !s.is_empty()),
        schema: Some(context.schema.clone()).filter(|s| !s.is_empty()),
    };
    for (hash, stmt) in jobs {
        let key = context.clone();
        tabular
            .diagnostics
            .server_pending
            .insert((key.clone(), hash));
        let tx = tabular.diagnostics.tx.clone();
        let pool = pool.clone();
        let target = target.clone();
        let ctx = ctx.clone();
        rt.spawn(async move {
            let outcome = validate_statement(&pool, &target, &stmt).await;
            let _ = tx.send(DiagMsg::Server { key, hash, outcome });
            ctx.request_repaint();
        });
    }
}

/// Penanda error dari eksekusi terakhir di tab ini, bila statement-nya masih ada.
fn update_execution_marker(tabular: &mut Tabular, tab_id: usize, lang: QueryLanguage) {
    let error_hash = tabular.last_error_location.as_ref().map_or(0, |(id, loc)| {
        qd::hash_text(&format!(
            "{id}\u{0}{}\u{0}{:?}\u{0}{:?}\u{0}{}",
            loc.statement,
            loc.char_offset,
            loc.line,
            tabular.last_error_message.as_deref().unwrap_or("")
        ))
    });
    let key = (
        tabular.editor.revision,
        tabular.editor.text.len(),
        error_hash,
    );
    if tabular.diagnostics.exec_key == Some(key) {
        return;
    }
    tabular.diagnostics.exec_key = Some(key);
    let span = (|| {
        let (err_tab, loc) = tabular.last_error_location.as_ref()?;
        if *err_tab != tab_id {
            return None;
        }
        let text = &tabular.editor.text;
        let start_loc = crate::connection::types::ErrorLocation {
            statement: loc.statement.clone(),
            char_offset: Some(0),
            line: None,
        };
        let base = crate::connection::sql::locate_error_in_text(text, &start_loc)?;
        let end = if text[base..].starts_with(loc.statement.trim()) {
            base + loc.statement.trim().len()
        } else {
            qd::statement_spans(&text[base..], dialect_of(lang))
                .first()
                .map_or(text.len(), |r| base + r.end)
        };
        let stmt = text.get(base..end)?;
        let message = tabular.last_error_message.clone().unwrap_or_default();
        let pos = ServerPosition {
            char_offset: loc.char_offset,
            line: loc.line,
        };
        let rel = locate(stmt, &message, &pos, dialect_of(lang)).or_else(|| {
            let first_line = stmt.find('\n').unwrap_or(stmt.len());
            (first_line > 0).then_some(0..first_line)
        })?;
        Some((base + rel.start..base + rel.end, message))
    })();
    let d = &mut tabular.diagnostics;
    let new_span = span.as_ref().map(|(r, _)| r.clone());
    if new_span != d.execution_span {
        d.execution_span = new_span;
        d.needs_merge = true;
    }
}

fn dialect_of(lang: QueryLanguage) -> crate::autocomplete::lexer::Dialect {
    match lang {
        QueryLanguage::Sql(d) => d,
        _ => crate::autocomplete::lexer::Dialect::Generic,
    }
}

fn merge(tabular: &mut Tabular, context: &ContextKey) {
    let d = &mut tabular.diagnostics;
    d.needs_merge = false;
    let server = &d.server;
    let mut items = qd::merge(
        &d.local,
        &d.statements,
        |hash| server.get(&(context.clone(), hash)),
        d.server_active,
    );
    if let Some(span) = d.execution_span.clone() {
        let overlaps = items.iter().any(|m| {
            m.source == DiagSource::Server
                && m.span
                    .as_ref()
                    .is_some_and(|r| r.start < span.end && span.start < r.end)
        });
        if !overlaps {
            let message = tabular
                .last_error_message
                .as_deref()
                .map(tidy_server_message)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "The last run failed here".to_string());
            items.push(LintMessage {
                severity: LintSeverity::Error,
                message,
                span: Some(span),
                hint: Some("Reported by the server when this query last ran.".to_string()),
                code: Some(DiagCode::Execution),
                source: DiagSource::Execution,
                ..Default::default()
            });
            items.sort_by_key(|m| m.span.as_ref().map_or(usize::MAX, |r| r.start));
        }
    }
    let len = tabular.editor.text.len();
    items.retain(|m| {
        m.span.as_ref().is_none_or(|r| {
            r.end <= len
                && tabular.editor.text.is_char_boundary(r.start)
                && tabular.editor.text.is_char_boundary(r.end)
        })
    });
    tabular.diagnostics.items = items;
}

/// Ganti teks diagnostik di `pos` dengan saran pertamanya (quick fix).
/// Mengembalikan true bila teks diubah.
pub(crate) fn apply_quick_fix(tabular: &mut Tabular, item: &LintMessage, suggestion: &str) -> bool {
    let Some(span) = item.span.clone() else {
        return false;
    };
    let text = &tabular.editor.text;
    if span.end > text.len()
        || !text.is_char_boundary(span.start)
        || !text.is_char_boundary(span.end)
    {
        return false;
    }
    let mut new = String::with_capacity(text.len() + suggestion.len());
    new.push_str(&text[..span.start]);
    new.push_str(suggestion);
    new.push_str(&text[span.end..]);
    tabular.editor.set_text(new);
    let pos = span.start + suggestion.len();
    tabular.cursor_position = pos;
    tabular.selection_start = pos;
    tabular.selection_end = pos;
    tabular.multi_selection.clear();
    tabular.multi_selection.add_collapsed(pos);
    tabular.pending_cursor_set = Some(pos);
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = tabular.editor.text.clone();
        tab.is_modified = true;
    }
    true
}

/// Pindahkan kursor ke diagnostik berikutnya/sebelumnya.
pub(crate) fn goto_problem(tabular: &mut Tabular, forward: bool) {
    let pos = tabular.cursor_position;
    let Some(target) = tabular
        .diagnostics
        .next_from(pos, forward)
        .and_then(|m| m.span.clone())
    else {
        tabular.toasts.info("No problems in this query.");
        return;
    };
    move_cursor(tabular, target.start);
}

pub(crate) fn move_cursor(tabular: &mut Tabular, pos: usize) {
    let pos = pos.min(tabular.editor.text.len());
    tabular.multi_selection.clear();
    tabular.multi_selection.add_collapsed(pos);
    tabular.cursor_position = pos;
    tabular.selection_start = pos;
    tabular.selection_end = pos;
    tabular.selection_force_clear = true;
    tabular.pending_cursor_set = Some(pos);
    tabular.editor_focus_boost_frames = tabular.editor_focus_boost_frames.max(4);
}

/// Quick fix untuk diagnostik di kursor (saran pertama).
pub(crate) fn quick_fix_at_cursor(tabular: &mut Tabular) {
    let pos = tabular.cursor_position;
    let Some(item) = tabular
        .diagnostics
        .items
        .iter()
        .find(|m| {
            !m.suggestions.is_empty()
                && m.span
                    .as_ref()
                    .is_some_and(|r| r.start <= pos && pos <= r.end)
        })
        .cloned()
    else {
        tabular.toasts.info("No quick fix at the cursor.");
        return;
    };
    let suggestion = item.suggestions[0].clone();
    apply_quick_fix(tabular, &item, &suggestion);
}
