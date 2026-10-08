//! Saran AI inline ("ghost text", gaya Copilot) di editor SQL.
//!
//! Modul ini dibagi dua:
//! - Logika murni (kelayakan pemicu, pembatasan konteks, pembersihan respons,
//!   pemecahan kata untuk accept per kata, penanda staleness) yang bisa diuji
//!   tanpa UI.
//! - Perekat ke editor (`handle_keys_pre_render`, `update_after_render`) yang
//!   dipanggil dari `editor::render_advanced_editor`.
//!
//! Fitur ini default MATI karena mengirim teks editor ke provider AI. Hanya
//! backend HTTP API yang dipakai; bila target AI default adalah CLI agent, atau
//! belum ada key/provider, atau kategori jaringan AI diblokir privacy, tidak
//! ada request yang dikirim.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::text::CCursor;

use crate::window_egui::Tabular;

/// Jeda ketik sebelum request dikirim.
pub const DEBOUNCE: Duration = Duration::from_millis(600);
/// Batas teks sebelum kursor yang dikirim (byte).
pub const MAX_PREFIX_BYTES: usize = 4 * 1024;
/// Batas teks sesudah kursor yang dikirim (byte).
pub const MAX_SUFFIX_BYTES: usize = 1024;
/// Batas ringkasan skema di prompt (byte).
pub const MAX_SCHEMA_BYTES: usize = 3 * 1024;
/// Jumlah tabel maksimum di ringkasan skema.
const MAX_SCHEMA_TABLES: usize = 20;
/// Jumlah baris saran maksimum.
pub const MAX_LINES: usize = 6;
/// Panjang saran maksimum (byte).
pub const MAX_CHARS: usize = 800;
/// Jendela teks di sekitar kursor yang di-hash untuk penanda staleness.
const ANCHOR_WINDOW_BEFORE: usize = 8 * 1024;
const ANCHOR_WINDOW_AFTER: usize = 2 * 1024;

// ─── Logika murni ────────────────────────────────────────────────────────────

/// Penanda keadaan editor saat saran diminta/ditampilkan. Saran hanya berlaku
/// selama penanda ini sama dengan keadaan editor sekarang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhostAnchor {
    pub tab_index: usize,
    pub cursor: usize,
    pub text_len: usize,
    pub window_hash: u64,
}

impl GhostAnchor {
    /// Ambil penanda dari teks, posisi kursor (byte) dan indeks tab aktif.
    /// Hanya jendela di sekitar kursor yang di-hash supaya murah tiap frame.
    pub fn capture(text: &str, cursor: usize, tab_index: usize) -> Self {
        let cursor = floor_char_boundary(text, cursor);
        let start = floor_char_boundary(text, cursor.saturating_sub(ANCHOR_WINDOW_BEFORE));
        let end = ceil_char_boundary(text, cursor.saturating_add(ANCHOR_WINDOW_AFTER));
        let mut hasher = DefaultHasher::new();
        text[start..end].hash(&mut hasher);
        Self {
            tab_index,
            cursor,
            text_len: text.len(),
            window_hash: hasher.finish(),
        }
    }

    /// `true` bila keadaan editor sudah berubah sejak penanda ini diambil.
    pub fn is_stale(&self, current: &GhostAnchor) -> bool {
        self != current
    }
}

fn floor_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, idx: usize) -> usize {
    let mut i = idx.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Apakah saran boleh dipicu pada posisi ini: tanpa seleksi, tanpa popup
/// autocomplete, tanpa multi-kursor, kursor di akhir baris (atau hanya diikuti
/// spasi), dan ada isi sebelum kursor.
pub fn is_trigger_eligible(
    text: &str,
    cursor: usize,
    selection: (usize, usize),
    popup_open: bool,
    multi_cursor: bool,
) -> bool {
    if popup_open || multi_cursor || selection.0 != selection.1 {
        return false;
    }
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return false;
    }
    let rest_of_line = text[cursor..].split('\n').next().unwrap_or("");
    if !rest_of_line.trim().is_empty() {
        return false;
    }
    let before = &text[..cursor];
    if before.trim().is_empty() {
        return false;
    }
    // Jangan ganggu blok inline `--AI … --` yang punya alurnya sendiri.
    let line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line = before[line_start..].trim_start();
    if line.starts_with("--AI") || line.starts_with("-- AI") {
        return false;
    }
    true
}

/// Potong konteks: maksimal [`MAX_PREFIX_BYTES`] sebelum kursor dan
/// [`MAX_SUFFIX_BYTES`] sesudahnya, selalu di batas karakter.
pub fn bounded_context(text: &str, cursor: usize) -> (&str, &str) {
    let cursor = floor_char_boundary(text, cursor);
    let start = ceil_char_boundary(text, cursor.saturating_sub(MAX_PREFIX_BYTES));
    let end = floor_char_boundary(text, cursor.saturating_add(MAX_SUFFIX_BYTES));
    (&text[start..cursor], &text[cursor..end.max(cursor)])
}

/// Potong string di batas karakter tanpa melebihi `max` byte.
pub fn truncate_bytes(s: &str, max: usize) -> &str {
    &s[..floor_char_boundary(s, max)]
}

/// Susun system prompt dan user prompt untuk satu permintaan completion.
pub fn build_prompts(prefix: &str, suffix: &str, engine: &str, schema: &str) -> (String, String) {
    let system = format!(
        "You are an inline SQL code-completion engine inside a database client. \
         Continue the user's SQL exactly at the cursor. Reply with ONLY the raw text to insert \
         at the cursor: no explanations, no markdown, no code fences, and do not repeat any text \
         that is already before the cursor. Keep it short (at most {MAX_LINES} lines), usually \
         finishing the current clause or statement. If no useful completion exists, reply with \
         an empty message."
    );
    let mut user = String::new();
    let engine = engine.trim();
    if !engine.is_empty() {
        user.push_str(&format!("Database engine: {engine}\n"));
    }
    let schema = schema.trim();
    if !schema.is_empty() {
        user.push_str("Schema (partial):\n");
        user.push_str(schema);
        user.push('\n');
    }
    user.push_str(
        "\nText before the cursor is inside <prefix>, text after the cursor is inside <suffix>. \
         Output only the completion to insert between them.\n",
    );
    user.push_str("<prefix>");
    user.push_str(prefix);
    user.push_str("</prefix>\n<suffix>");
    user.push_str(suffix);
    user.push_str("</suffix>");
    (system, user)
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Ambil isi blok kode berpagar pertama bila ada; selain itu kembalikan apa adanya.
fn strip_fences(raw: &str) -> String {
    let Some(open) = raw.find("```") else {
        return raw.to_string();
    };
    let after_open = &raw[open + 3..];
    // Lewati label bahasa (```sql) sampai akhir baris pembuka.
    let body_start = after_open
        .find('\n')
        .map(|p| p + 1)
        .unwrap_or(after_open.len());
    let body = &after_open[body_start..];
    let body = match body.find("```") {
        Some(close) => &body[..close],
        None => body,
    };
    body.trim_end_matches(['\n', '\r']).to_string()
}

/// Bersihkan respons model menjadi teks yang siap disisipkan di kursor.
/// Membuang pagar kode, tag prompt, prefix yang diulang, baris yang sudah ada
/// di suffix, lalu membatasi jumlah baris/panjang. `None` bila hasilnya kosong.
pub fn clean_completion(raw: &str, prefix: &str, suffix: &str) -> Option<String> {
    let mut s = strip_fences(raw);
    for tag in [
        "<prefix>",
        "</prefix>",
        "<suffix>",
        "</suffix>",
        "<cursor>",
        "<CURSOR>",
    ] {
        s = s.replace(tag, "");
    }
    let mut s = s.trim_end().to_string();

    let line_start = prefix.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let current_line = &prefix[line_start..];
    let current_trimmed = current_line.trim_start();

    // Model mengulang baris saat ini (mis. "SELECT * FR" -> "SELECT * FROM t").
    if !current_trimmed.is_empty() {
        let lead = s.len() - s.trim_start().len();
        if s[lead..].starts_with(current_trimmed) {
            s = s[lead + current_trimmed.len()..].to_string();
        } else {
            // Model mengulang potongan kata terakhir (mis. "us" -> "users").
            let word_start = current_line
                .char_indices()
                .rev()
                .take_while(|(_, c)| is_ident_char(*c))
                .last()
                .map(|(i, _)| i);
            if let Some(ws) = word_start {
                let word = &current_line[ws..];
                if s.starts_with(word) && s.len() > word.len() {
                    s = s[word.len()..].to_string();
                }
            }
        }
    }

    // Hindari spasi ganda bila prefix sudah diakhiri spasi.
    if prefix.ends_with([' ', '\t']) {
        s = s.trim_start_matches([' ', '\t']).to_string();
    }

    // Buang baris terakhir yang sama persis dengan baris berikutnya di suffix.
    if let Some(next_line) = suffix.lines().map(str::trim).find(|l| !l.is_empty()) {
        while let Some(last_nl) = s.rfind('\n') {
            if s[last_nl + 1..].trim() == next_line {
                s.truncate(last_nl);
                s = s.trim_end().to_string();
            } else {
                break;
            }
        }
    }

    // Batasi jumlah baris dan panjang.
    let mut out = String::new();
    for (i, line) in s.split('\n').enumerate() {
        if i >= MAX_LINES {
            break;
        }
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end_matches('\r'));
    }
    let out = truncate_bytes(&out, MAX_CHARS).trim_end().to_string();
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Pecah saran menjadi "kata berikutnya" dan sisanya, untuk accept per kata
/// (Cmd/Ctrl+→). Spasi di depan ikut kata; berhenti setelah baris baru +
/// indentasinya.
pub fn split_next_word(s: &str) -> (&str, &str) {
    let mut end = 0;
    let mut chars = s.char_indices().peekable();
    // Spasi di depan (termasuk satu baris baru beserta indentasinya).
    while let Some(&(i, c)) = chars.peek() {
        if c == '\n' {
            end = i + 1;
            chars.next();
            while let Some(&(j, c2)) = chars.peek() {
                if c2 == ' ' || c2 == '\t' {
                    end = j + c2.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            return (&s[..end], &s[end..]);
        }
        if c.is_whitespace() {
            end = i + c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    match chars.peek() {
        Some(&(_, c)) if is_ident_char(c) => {
            while let Some(&(i, c)) = chars.peek() {
                if is_ident_char(c) {
                    end = i + c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
        Some(&(i, c)) => {
            end = i + c.len_utf8();
        }
        None => {}
    }
    (&s[..end], &s[end..])
}

/// Baris pertama saran (untuk digambar inline) dan jumlah baris tambahan.
pub fn first_line_and_extra(s: &str) -> (&str, usize) {
    let mut lines = s.split('\n');
    let first = lines.next().unwrap_or("");
    (first, lines.count())
}

// ─── State ───────────────────────────────────────────────────────────────────

/// Request yang sedang berjalan beserta keadaan editor saat dikirim.
pub struct PendingGhost {
    pub rx: mpsc::Receiver<Result<String, String>>,
    pub anchor: GhostAnchor,
    pub prefix: String,
    pub suffix: String,
}

/// Saran yang siap ditampilkan.
#[derive(Debug, Clone)]
pub struct GhostSuggestion {
    pub text: String,
    pub anchor: GhostAnchor,
}

/// State ghost text milik editor (satu untuk seluruh aplikasi; saran terikat
/// pada tab lewat [`GhostAnchor::tab_index`]).
/// Ringkasan skema yang sudah dibangun untuk satu (koneksi, database).
pub struct SchemaContextCache {
    pub connection_id: i64,
    pub database: String,
    pub text: String,
    pub built_at: Instant,
}

/// Masa berlaku ringkasan skema sebelum dibangun ulang.
const SCHEMA_CACHE_TTL: Duration = Duration::from_secs(120);

#[derive(Default)]
pub struct GhostState {
    /// Ringkasan skema terakhir; dibangun di runtime latar belakang supaya
    /// UI thread tidak menunggu query cache SQLite setiap request.
    pub schema_cache: Option<SchemaContextCache>,
    /// Pembangunan skema yang sedang berjalan: (koneksi, database, hasil).
    pub schema_rx: Option<mpsc::Receiver<(i64, String, String)>>,
    /// Penanda keadaan editor pada frame sebelumnya.
    pub last_anchor: Option<GhostAnchor>,
    /// Waktu ketikan terakhir; `Some` berarti pemicu sedang menunggu debounce.
    pub armed_at: Option<Instant>,
    /// Hanya satu request boleh berjalan.
    pub pending: Option<PendingGhost>,
    pub suggestion: Option<GhostSuggestion>,
}

impl GhostState {
    pub fn clear(&mut self) {
        self.armed_at = None;
        self.suggestion = None;
    }
}

// ─── Perekat editor ──────────────────────────────────────────────────────────

fn current_anchor(tabular: &Tabular) -> GhostAnchor {
    GhostAnchor::capture(
        &tabular.editor.text,
        tabular.cursor_position,
        tabular.active_tab_index,
    )
}

/// Saran yang berlaku untuk keadaan editor saat ini, bila ada.
fn visible_suggestion(tabular: &Tabular) -> Option<&GhostSuggestion> {
    if !tabular.ai_inline_suggestions
        || tabular.show_autocomplete
        || !tabular.multi_selection.is_empty()
        || tabular.selection_start != tabular.selection_end
    {
        return None;
    }
    let s = tabular.ghost.suggestion.as_ref()?;
    (!s.anchor.is_stale(&current_anchor(tabular))).then_some(s)
}

/// Sisipkan teks di kursor lewat buffer editor (ikut undo), lalu sinkronkan
/// kursor egui dan isi tab.
fn insert_at_cursor(tabular: &mut Tabular, ctx: &egui::Context, editor_id: egui::Id, s: &str) {
    let pos = floor_char_boundary(&tabular.editor.text, tabular.cursor_position);
    tabular.editor.apply_single_replace(pos..pos, s);
    let new_pos = pos + s.len();
    tabular.cursor_position = new_pos;
    tabular.selection_start = new_pos;
    tabular.selection_end = new_pos;
    tabular.selected_text.clear();
    let ci = tabular.editor.text[..new_pos].chars().count();
    crate::editor_state_adapter::EditorStateAdapter::set_single(ctx, editor_id, ci);
    if let Some(tab) = tabular.query_tabs.get_mut(tabular.active_tab_index) {
        tab.content = tabular.editor.text.clone();
        tab.is_modified = true;
    }
    ctx.memory_mut(|m| m.request_focus(editor_id));
    ctx.request_repaint();
}

/// Tangani Tab / Cmd|Ctrl+→ / Esc untuk ghost text SEBELUM TextEdit dirender,
/// supaya event tersebut tidak diproses editor. Mengembalikan `true` bila teks
/// berubah (pemanggil sebaiknya scroll ke kursor).
pub(crate) fn handle_keys_pre_render(
    tabular: &mut Tabular,
    ui: &egui::Ui,
    editor_id: egui::Id,
) -> bool {
    if visible_suggestion(tabular).is_none() || !ui.memory(|m| m.has_focus(editor_id)) {
        return false;
    }

    let (tab, word, esc) = ui.input(|i| {
        let mut tab = false;
        let mut word = false;
        let mut esc = false;
        for ev in &i.events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            {
                match key {
                    egui::Key::Tab if modifiers.is_none() => tab = true,
                    egui::Key::ArrowRight
                        if modifiers.command && !modifiers.shift && !modifiers.alt =>
                    {
                        word = true
                    }
                    egui::Key::Escape => esc = true,
                    _ => {}
                }
            }
        }
        (tab, word, esc)
    });
    if !(tab || word || esc) {
        return false;
    }

    // Buang event yang kita tangani agar TextEdit tidak ikut memprosesnya.
    ui.ctx().input_mut(|ri| {
        ri.events.retain(|e| match e {
            egui::Event::Key {
                key: egui::Key::Tab,
                ..
            } => !tab,
            egui::Event::Key {
                key: egui::Key::ArrowRight,
                modifiers,
                ..
            } if modifiers.command => !word,
            egui::Event::Key {
                key: egui::Key::Escape,
                ..
            } => !esc,
            _ => true,
        })
    });

    if esc && !tab && !word {
        log::debug!("[AI] Inline suggestion dismissed (Esc)");
        tabular.ghost.suggestion = None;
        return false;
    }

    let Some(sugg) = tabular.ghost.suggestion.take() else {
        return false;
    };
    if tab {
        log::debug!(
            "[AI] Inline suggestion accepted ({} bytes)",
            sugg.text.len()
        );
        insert_at_cursor(tabular, ui.ctx(), editor_id, &sugg.text);
    } else {
        let (w, rest) = split_next_word(&sugg.text);
        let (w, rest) = (w.to_string(), rest.to_string());
        insert_at_cursor(tabular, ui.ctx(), editor_id, &w);
        if !rest.is_empty() {
            let anchor = current_anchor(tabular);
            tabular.ghost.suggestion = Some(GhostSuggestion { text: rest, anchor });
            // Accept per kata bukan "ketikan": jangan picu request baru.
            tabular.ghost.last_anchor = Some(anchor);
        }
    }
    tabular.ghost.armed_at = None;
    true
}

/// Backend HTTP API yang siap dipakai, atau `None` bila target default adalah
/// CLI agent, belum dikonfigurasi, atau jaringan AI diblokir.
fn ready_api_backend(tabular: &Tabular) -> Option<crate::ai_assistant::ChatBackend> {
    let target = tabular.effective_default_target();
    if target != crate::config::ChatTarget::Api {
        return None;
    }
    crate::ai_assistant::backend_ready_for(tabular, target).ok()?;
    if !crate::privacy::allowed(crate::privacy::NetCategory::Ai) {
        return None;
    }
    Some(crate::ai_assistant::chat_backend_for(tabular, target))
}

fn engine_label(tabular: &Tabular) -> String {
    let conn_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id)
        .or(tabular.current_connection_id);
    conn_id
        .and_then(|id| tabular.connections.iter().find(|c| c.id == Some(id)))
        .map(|c| format!("{:?}", c.connection_type))
        .unwrap_or_default()
}

/// (koneksi, database) yang dipakai ringkasan skema: koneksi aktif dan
/// database tab aktif, atau database pertama yang diketahui.
fn schema_target(tabular: &Tabular) -> Option<(i64, String)> {
    let conn_id = tabular.current_connection_id?;
    let db = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| {
            tabular
                .database_cache
                .get(&conn_id)
                .and_then(|dbs| dbs.first().cloned())
        })?;
    Some((conn_id, db))
}

/// Ringkasan skema dari cache bila masih berlaku; kalau tidak, bangun di
/// latar belakang dan kembalikan yang lama (atau kosong). Request saat ini
/// tetap dikirim tanpa menunggu.
fn schema_context(tabular: &mut Tabular) -> String {
    let Some((conn_id, db)) = schema_target(tabular) else {
        return String::new();
    };
    let cached = tabular
        .ghost
        .schema_cache
        .as_ref()
        .filter(|c| c.connection_id == conn_id && c.database == db);
    let fresh = cached.is_some_and(|c| c.built_at.elapsed() < SCHEMA_CACHE_TTL);
    let text = cached.map(|c| c.text.clone()).unwrap_or_default();
    if !fresh
        && tabular.ghost.schema_rx.is_none()
        && let (Some(pool), Some(rt)) = (tabular.db_pool.clone(), tabular.runtime.clone())
    {
        let (tx, rx) = mpsc::channel();
        tabular.ghost.schema_rx = Some(rx);
        let db_for_task = db.clone();
        rt.spawn(async move {
            let text = crate::ai_assistant::build_schema_context_async(
                &pool,
                conn_id,
                &db_for_task,
                MAX_SCHEMA_TABLES,
            )
            .await;
            let _ = tx.send((conn_id, db_for_task, text));
        });
    }
    text
}

/// Ambil hasil pembangunan skema latar belakang bila sudah ada.
fn poll_schema(tabular: &mut Tabular) {
    let Some(rx) = tabular.ghost.schema_rx.as_ref() else {
        return;
    };
    match rx.try_recv() {
        Ok((connection_id, database, text)) => {
            tabular.ghost.schema_rx = None;
            tabular.ghost.schema_cache = Some(SchemaContextCache {
                connection_id,
                database,
                text,
                built_at: Instant::now(),
            });
        }
        Err(mpsc::TryRecvError::Empty) => {}
        Err(mpsc::TryRecvError::Disconnected) => tabular.ghost.schema_rx = None,
    }
}

fn fire_request(tabular: &mut Tabular, backend: crate::ai_assistant::ChatBackend) {
    let anchor = current_anchor(tabular);
    let (prefix, suffix) = bounded_context(&tabular.editor.text, anchor.cursor);
    let (prefix, suffix) = (prefix.to_string(), suffix.to_string());
    let engine = engine_label(tabular);
    let schema_full = schema_context(tabular);
    let schema = truncate_bytes(&schema_full, MAX_SCHEMA_BYTES);
    let (system, user) = build_prompts(&prefix, &suffix, &engine, schema);
    log::debug!(
        "[AI] Inline suggestion request (prefix {} B, suffix {} B, schema {} B)",
        prefix.len(),
        suffix.len(),
        schema.len()
    );
    let rx = crate::ai_assistant::request_ai_suggestion(
        backend.provider,
        backend.api_key,
        backend.model,
        backend.base_url,
        system,
        user,
    );
    tabular.ghost.pending = Some(PendingGhost {
        rx,
        anchor,
        prefix,
        suffix,
    });
}

/// Panggil setelah TextEdit dirender dan kursor tersinkron: lacak perubahan,
/// ambil respons, picu request setelah debounce, dan gambar ghost text.
pub(crate) fn update_after_render(
    tabular: &mut Tabular,
    ui: &egui::Ui,
    galley: &std::sync::Arc<egui::Galley>,
    galley_pos: egui::Pos2,
    clip_rect: egui::Rect,
    font_id: egui::FontId,
    has_focus: bool,
) {
    if !tabular.ai_inline_suggestions {
        if tabular.ghost.suggestion.is_some() || tabular.ghost.armed_at.is_some() {
            tabular.ghost.clear();
        }
        tabular.ghost.last_anchor = None;
        // Request yang sudah berjalan dibiarkan selesai lalu dibuang.
        poll_pending(tabular, None);
        return;
    }

    poll_schema(tabular);
    let anchor = current_anchor(tabular);
    if tabular.ghost.last_anchor != Some(anchor) {
        let typed = tabular
            .ghost
            .last_anchor
            .is_some_and(|p| p.tab_index == anchor.tab_index && p.text_len != anchor.text_len);
        tabular.ghost.last_anchor = Some(anchor);
        if tabular
            .ghost
            .suggestion
            .as_ref()
            .is_some_and(|s| s.anchor.is_stale(&anchor))
        {
            tabular.ghost.suggestion = None;
        }
        // Hanya ketikan yang memicu; gerak kursor/pindah tab membatalkan pemicu.
        tabular.ghost.armed_at = typed.then(Instant::now);
    }

    poll_pending(tabular, Some(anchor));

    if let Some(armed) = tabular.ghost.armed_at {
        let elapsed = armed.elapsed();
        if elapsed < DEBOUNCE {
            ui.ctx().request_repaint_after(DEBOUNCE - elapsed);
        } else if tabular.ghost.pending.is_some() {
            // Tunggu request sebelumnya selesai (hanya satu yang boleh berjalan).
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        } else {
            tabular.ghost.armed_at = None;
            let eligible = has_focus
                && tabular.ghost.suggestion.is_none()
                && is_trigger_eligible(
                    &tabular.editor.text,
                    tabular.cursor_position,
                    (tabular.selection_start, tabular.selection_end),
                    tabular.show_autocomplete,
                    !tabular.multi_selection.is_empty(),
                );
            if eligible && let Some(backend) = ready_api_backend(tabular) {
                fire_request(tabular, backend);
            }
        }
    }
    if tabular.ghost.pending.is_some() {
        // Poll hasil request tanpa menunggu input berikutnya.
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    if has_focus {
        paint_ghost(tabular, ui, galley, galley_pos, clip_rect, font_id);
    }
}

/// Ambil hasil request bila sudah ada. Respons dibuang bila keadaan editor
/// berubah sejak request dikirim (`current == None` berarti selalu buang).
fn poll_pending(tabular: &mut Tabular, current: Option<GhostAnchor>) {
    let Some(pending) = tabular.ghost.pending.as_ref() else {
        return;
    };
    let result = match pending.rx.try_recv() {
        Ok(r) => r,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => Err("channel closed".to_string()),
    };
    let Some(pending) = tabular.ghost.pending.take() else {
        return;
    };
    match result {
        Ok(raw) => {
            if current.is_none_or(|c| pending.anchor.is_stale(&c)) {
                log::debug!("[AI] Inline suggestion dropped (stale)");
                return;
            }
            match clean_completion(&raw, &pending.prefix, &pending.suffix) {
                Some(text) => {
                    tabular.ghost.suggestion = Some(GhostSuggestion {
                        text,
                        anchor: pending.anchor,
                    });
                }
                None => log::debug!("[AI] Inline suggestion empty after cleanup"),
            }
        }
        Err(e) => log::debug!("[AI] Inline suggestion failed: {e}"),
    }
}

fn paint_ghost(
    tabular: &Tabular,
    ui: &egui::Ui,
    galley: &std::sync::Arc<egui::Galley>,
    galley_pos: egui::Pos2,
    clip_rect: egui::Rect,
    font_id: egui::FontId,
) {
    let Some(sugg) = visible_suggestion(tabular) else {
        return;
    };
    let cursor = sugg.anchor.cursor.min(tabular.editor.text.len());
    let ci = tabular.editor.text[..cursor].chars().count();
    let caret = galley
        .pos_from_cursor(CCursor::new(ci))
        .translate(galley_pos.to_vec2());

    let (first, extra) = first_line_and_extra(&sugg.text);
    let color = ui.visuals().weak_text_color().gamma_multiply(0.85);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &first.replace('\t', "    "),
        0.0,
        egui::TextFormat {
            font_id: font_id.clone(),
            color,
            italics: true,
            ..Default::default()
        },
    );
    if extra > 0 {
        let label = if extra == 1 {
            "+1 line (Tab to accept)".to_string()
        } else {
            format!("+{extra} lines (Tab to accept)")
        };
        job.append(
            &label,
            if first.trim().is_empty() { 0.0 } else { 12.0 },
            egui::TextFormat {
                font_id: egui::FontId::proportional((font_id.size * 0.8).max(9.0)),
                color: color.gamma_multiply(0.7),
                italics: true,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
    }
    let ghost_galley = ui.fonts_mut(|f| f.layout_job(job));
    let painter = ui
        .painter()
        .with_clip_rect(clip_rect.intersect(ui.clip_rect()));
    painter.galley(caret.left_top() + egui::vec2(1.0, 0.0), ghost_galley, color);
}

// ─── Preferensi ──────────────────────────────────────────────────────────────

impl Tabular {
    /// Section "Inline Suggestions" di halaman Preferences → AI Assistant.
    pub(crate) fn render_ai_inline_suggestions_pref(&mut self, ui: &mut egui::Ui) {
        use crate::window_egui::preferences::{hint, section, toggle_row};
        section(ui, "Inline Suggestions", |ui| {
            if toggle_row(
                ui,
                &mut self.ai_inline_suggestions,
                "Inline AI suggestions",
                Some(
                    "Show ghost-text completions in the SQL editor when you pause typing. \
                     Sends the text around the cursor and a compact schema summary to the \
                     configured API provider.",
                ),
            ) {
                if !self.ai_inline_suggestions {
                    self.ghost.clear();
                }
                self.prefs_dirty = true;
                self.try_save_prefs();
            }
            if self.ai_inline_suggestions {
                let target = self.effective_default_target();
                if target != crate::config::ChatTarget::Api {
                    hint(
                        ui,
                        "Inactive: the default AI target is a CLI agent. Inline suggestions only use the API provider.",
                    );
                } else if let Err(e) = crate::ai_assistant::backend_ready_for(self, target) {
                    hint(ui, format!("Inactive: {e}"));
                } else if !crate::privacy::allowed(crate::privacy::NetCategory::Ai) {
                    hint(
                        ui,
                        "Inactive: AI network access is blocked in Privacy settings.",
                    );
                }
            }
            hint(
                ui,
                "Tab accepts, Cmd/Ctrl+Right accepts the next word, Esc dismisses.",
            );
        });
    }
}

// ─── Tes ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligible_at_end_of_line_without_selection() {
        let t = "SELECT * FROM ";
        assert!(is_trigger_eligible(
            t,
            t.len(),
            (t.len(), t.len()),
            false,
            false
        ));
    }

    #[test]
    fn eligible_when_only_whitespace_follows_on_line() {
        let t = "SELECT *   \nFROM x";
        assert!(is_trigger_eligible(t, 8, (8, 8), false, false));
    }

    #[test]
    fn not_eligible_mid_line_or_with_selection_or_popup() {
        let t = "SELECT id FROM users";
        let n = t.len();
        assert!(!is_trigger_eligible(t, 6, (6, 6), false, false));
        assert!(!is_trigger_eligible(t, n, (0, 6), false, false));
        assert!(!is_trigger_eligible(t, n, (n, n), true, false));
        assert!(!is_trigger_eligible(t, n, (n, n), false, true));
    }

    #[test]
    fn not_eligible_on_empty_buffer_bad_boundary_or_ai_block() {
        assert!(!is_trigger_eligible("   \n ", 5, (5, 5), false, false));
        let t = "SELECT 'é'";
        assert!(!is_trigger_eligible(t, 9, (9, 9), false, false));
        let t = "--AI list users";
        let n = t.len();
        assert!(!is_trigger_eligible(t, n, (n, n), false, false));
    }

    #[test]
    fn bounded_context_limits_and_respects_char_boundaries() {
        let text = format!("{}|{}", "é".repeat(5000), "x".repeat(3000));
        let cursor = text.find('|').unwrap_or(0);
        let (p, s) = bounded_context(&text, cursor);
        assert!(p.len() <= MAX_PREFIX_BYTES && !p.is_empty());
        assert!(s.len() <= MAX_SUFFIX_BYTES);
        assert!(s.starts_with('|'));
        assert!(text[..cursor].ends_with(p));
    }

    #[test]
    fn prompt_contains_prefix_suffix_engine_and_schema() {
        let (sys, user) = build_prompts("SELECT * FR", "\n;", "Postgres", "-- Table: users");
        assert!(sys.contains("no code fences"));
        assert!(user.contains("<prefix>SELECT * FR</prefix>"));
        assert!(user.contains("<suffix>\n;</suffix>"));
        assert!(user.contains("Database engine: Postgres"));
        assert!(user.contains("-- Table: users"));
    }

    #[test]
    fn clean_strips_fences() {
        let raw = "```sql\nOM users\nWHERE id = 1\n```";
        assert_eq!(
            clean_completion(raw, "SELECT * FR", "").as_deref(),
            Some("OM users\nWHERE id = 1")
        );
    }

    #[test]
    fn clean_strips_echoed_current_line() {
        assert_eq!(
            clean_completion("SELECT * FROM users;", "SELECT * FR", "").as_deref(),
            Some("OM users;")
        );
        assert_eq!(
            clean_completion("  SELECT * FROM users", "x;\n  SELECT * FR", "").as_deref(),
            Some("OM users")
        );
    }

    #[test]
    fn clean_strips_echoed_partial_word() {
        assert_eq!(
            clean_completion("users WHERE", "SELECT * FROM us", "").as_deref(),
            Some("ers WHERE")
        );
    }

    #[test]
    fn clean_avoids_double_space_and_duplicate_suffix_line() {
        assert_eq!(
            clean_completion(" users", "SELECT * FROM ", "").as_deref(),
            Some("users")
        );
        assert_eq!(
            clean_completion("users\nLIMIT 10;", "SELECT * FROM ", "\nLIMIT 10;\n").as_deref(),
            Some("users")
        );
    }

    #[test]
    fn clean_caps_lines_and_rejects_empty() {
        let raw = (0..20)
            .map(|i| format!("-- l{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = clean_completion(&raw, "SELECT 1;\n", "").unwrap_or_default();
        assert_eq!(out.lines().count(), MAX_LINES);
        assert_eq!(clean_completion("```\n```", "SELECT ", ""), None);
        assert_eq!(clean_completion("   ", "SELECT ", ""), None);
    }

    #[test]
    fn split_next_word_variants() {
        assert_eq!(split_next_word("OM users"), ("OM", " users"));
        assert_eq!(split_next_word(" users WHERE"), (" users", " WHERE"));
        assert_eq!(split_next_word(", name"), (",", " name"));
        assert_eq!(split_next_word("\n  WHERE x"), ("\n  ", "WHERE x"));
        assert_eq!(split_next_word("ümlaut x"), ("ümlaut", " x"));
        assert_eq!(split_next_word(""), ("", ""));
    }

    #[test]
    fn first_line_and_extra_counts() {
        assert_eq!(first_line_and_extra("a"), ("a", 0));
        assert_eq!(first_line_and_extra("a\nb\nc"), ("a", 2));
    }

    #[test]
    fn anchor_detects_edits_caret_moves_and_tab_switch() {
        let t = "SELECT * FROM users";
        let a = GhostAnchor::capture(t, t.len(), 0);
        assert!(!a.is_stale(&GhostAnchor::capture(t, t.len(), 0)));
        assert!(a.is_stale(&GhostAnchor::capture(t, 3, 0)));
        assert!(a.is_stale(&GhostAnchor::capture(t, t.len(), 1)));
        let t2 = "SELECT * FROM userz";
        assert!(a.is_stale(&GhostAnchor::capture(t2, t2.len(), 0)));
    }

    #[test]
    fn anchor_clamps_cursor_to_char_boundary() {
        let a = GhostAnchor::capture("é", 1, 0);
        assert_eq!(a.cursor, 0);
    }
}
