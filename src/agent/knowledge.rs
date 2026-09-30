//! Pengetahuan untuk agent di luar skema mentah:
//! - diagram Tabular: group (domain bisnis) beserta repository kodenya,
//!   relasi virtual, sticky note, dan database yang di-link;
//! - riwayat query user (pola join dan filter yang biasa dipakai);
//! - analisis alur data satu statement tanpa menjalankannya;
//! - pemakaian tabel di kode repository milik group diagram.
//!
//! Semua fungsi read-only terhadap database target. Data yang dibaca hanya
//! milik Tabular di mesin ini: file diagram, `connections.db`, dan folder
//! repository yang dipilih user di diagram.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;

use crate::diagram_notes::{parse_wikilinks, resolve_table};
use crate::models::enums::DatabaseType;
use crate::models::structs::{
    DiagramNode, DiagramState, FlowOp, FlowStepKind, FlowTarget, FlowTriggerKind, NoteAnchor,
    RelationOrigin,
};
use crate::query_diagram::{self, ColumnRef, QueryDiagramModel};

use super::core::{AgentError, HeadlessSession};

/// Panjang maksimum isi satu sticky note yang dikirim ke agent.
const MAX_NOTE_CHARS: usize = 4_000;
/// Jumlah maksimum sticky note per respons.
const MAX_NOTES: usize = 50;
/// Panjang maksimum satu query history yang dikirim ke agent.
const MAX_HISTORY_CHARS: usize = 2_000;
/// Batas kolom hasil `analyze_query` (SELECT * bisa sangat lebar).
const MAX_OUTPUT_COLUMNS: usize = 100;
/// Jumlah maksimum proses bisnis (flow card) per respons.
const MAX_FLOWS: usize = 40;
/// Batas waktu memuat diagram bersama dari tabel `diagram_by_tabular`.
const REMOTE_DIAGRAM_TIMEOUT: Duration = Duration::from_secs(10);
/// Batas waktu pencarian teks di satu repository.
const GREP_TIMEOUT: Duration = Duration::from_secs(90);

/// Asal diagram yang dibaca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagramSource {
    /// Cache JSON lokal yang juga dibuka GUI.
    LocalFile,
    /// Tabel `diagram_by_tabular` di database target (dibagikan ke tim).
    SharedTable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VirtualRelationInfo {
    pub child: String,
    pub child_column: String,
    pub parent: String,
    pub parent_column: String,
    /// `manual`, `inferred` (saran yang diterima user), atau `imported`.
    pub origin: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagramGroupInfo {
    pub id: String,
    pub title: String,
    pub tables: Vec<String>,
    /// URL git bersama (kredensial disamarkan).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    /// Folder project di komputer ini; bisa dibaca find_table_usages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_repo_path: Option<String>,
    pub note_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagramNoteInfo {
    pub id: String,
    pub title: String,
    /// `table` atau `group`.
    pub anchor_type: &'static str,
    /// Nama tabel atau judul group tempat note ditempel.
    pub anchor: String,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub updated_at: String,
    /// Tabel yang disebut lewat `[[tabel]]` di isi note.
    pub links: Vec<String>,
    /// Isi Markdown.
    pub body: String,
    pub truncated: bool,
}

/// Pemakaian satu tabel oleh sebuah proses bisnis.
#[derive(Debug, Clone, Serialize)]
pub struct FlowTableInfo {
    pub table: String,
    /// Operasi dari langkah (`read`, `insert`, ...). Kosong bila tabel hanya
    /// diketahui dari link endpoint, tanpa langkah hasil generate.
    pub ops: Vec<FlowOp>,
    /// Nomor langkah (mulai 1) yang menyentuh tabel ini.
    pub steps: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlowStepInfo {
    pub kind: FlowStepKind,
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// `{"kind": "table", "id": "<nama tabel>"}`, atau API luar / queue /
    /// cache / flow lain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<FlowTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<FlowOp>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// `path/file:line` di repository group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Satu proses bisnis (flow card di diagram): pemicu, tabel yang disentuh,
/// dan langkah berurutan hasil generate AI dari kode repository.
#[derive(Debug, Clone, Serialize)]
pub struct FlowInfo {
    pub id: String,
    /// `http`, `job`, `queue`, `cron`, `event`, atau `cli`.
    pub trigger: FlowTriggerKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// Path route, nama job, topik queue, atau ekspresi cron.
    pub target: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// File handler di repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub tables: Vec<FlowTableInfo>,
    /// Kosong bila alurnya belum di-generate.
    pub steps: Vec<FlowStepInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_at: Option<String>,
    /// Commit HEAD saat generate (7 karakter).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// AI hanya melihat cuplikan kode; langkahnya bisa kurang lengkap.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkedDatabaseInfo {
    pub connection_id: Option<i64>,
    pub connection_name: String,
    pub database: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagramDescription {
    pub connection_id: i64,
    pub database: String,
    /// `false` bila belum ada diagram untuk database ini.
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<DiagramSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub table_count: usize,
    pub groups: Vec<DiagramGroupInfo>,
    /// Tabel tanpa group (hanya saat tidak difilter).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ungrouped_tables: Vec<String>,
    pub virtual_relations: Vec<VirtualRelationInfo>,
    pub notes: Vec<DiagramNoteInfo>,
    pub total_notes: usize,
    /// Proses bisnis (flow card) yang menyentuh tabel dalam cakupan.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<FlowInfo>,
    #[serde(skip_serializing_if = "is_zero")]
    pub total_flows: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linked_databases: Vec<LinkedDatabaseInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryHistoryResult {
    pub results: Vec<crate::vector_index::HistoryHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyzedSource {
    pub table: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// `table`, `cte`, `subquery`, atau `values`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub used_columns: Vec<String>,
    pub all_columns: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalyzedOutput {
    pub name: String,
    pub expr: String,
    pub sources: Vec<String>,
    pub aggregate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryAnalysis {
    /// SELECT, INSERT, UPDATE, atau DELETE.
    pub statement: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub sources: Vec<AnalyzedSource>,
    /// `a.x = b.y (LEFT JOIN)`.
    pub joins: Vec<String>,
    pub output: Vec<AnalyzedOutput>,
    pub output_truncated: bool,
    /// `kolom = ekspresi` untuk INSERT/UPDATE (termasuk upsert).
    pub mutations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    pub filter_columns: Vec<String>,
    pub group_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub having: Option<String>,
    pub order_by: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    pub distinct: bool,
    pub notes: Vec<String>,
    /// Petunjuk optimasi heuristik (sama dengan panel Query Insight di GUI).
    pub hints: Vec<String>,
    /// Kolom join/filter tanpa index yang diawali kolom itu (menurut cache).
    pub unindexed_columns: Vec<String>,
    /// `true` bila kolom tabel diketahui dari cache skema.
    pub schema_resolved: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageHit {
    pub table: String,
    pub refs: usize,
    pub files: usize,
    pub score: f32,
    /// `path:line: snippet` (path relatif terhadap root repository).
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RepoUsage {
    /// Judul group diagram pemilik repository ini.
    pub groups: Vec<String>,
    pub root: String,
    /// `local_path` atau `cached_clone`.
    pub source: &'static str,
    pub files_scanned: usize,
    pub truncated: bool,
    pub hits: Vec<UsageHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableUsageReport {
    pub tables: Vec<String>,
    pub repositories: Vec<RepoUsage>,
    /// Tabel yang tidak ditemukan di repository mana pun.
    pub not_found: Vec<String>,
    /// Group dengan repository yang tidak bisa dibaca, beserta alasannya.
    pub skipped: Vec<String>,
}

fn origin_label(origin: &RelationOrigin) -> &'static str {
    match origin {
        RelationOrigin::Manual => "manual",
        RelationOrigin::Inferred => "inferred",
        RelationOrigin::Imported => "imported",
    }
}

/// Nama tabel yang tampil untuk node.
fn node_name(node: &DiagramNode) -> &str {
    if node.title.trim().is_empty() {
        &node.id
    } else {
        &node.title
    }
}

fn short_name(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn node_in_group(node: &DiagramNode, gid: &str) -> bool {
    node.group_ids.iter().any(|g| g == gid) || node.group_id.as_deref() == Some(gid)
}

fn clip_chars(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((cut, _)) => (format!("{}…", &text[..cut]), true),
        None => (text.to_string(), false),
    }
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Proses bisnis yang menyentuh tabel dalam cakupan (`None` = semua), urut
/// path lalu method. Id node tabel diganti nama tabelnya; `request_id`,
/// posisi dan kunci repository tidak ikut karena hanya berarti di GUI.
fn flow_infos(
    state: &DiagramState,
    scope_tables: Option<&HashSet<String>>,
    name_of: &dyn Fn(&str) -> String,
) -> Vec<FlowInfo> {
    let tables_by_card = crate::diagram_flow::tables_by_card(state);
    let mut cards: Vec<(&crate::models::structs::FlowCard, &[&str])> = state
        .flow_cards
        .iter()
        .filter_map(|c| {
            let tables = tables_by_card.get(c.id.as_str())?.as_slice();
            let in_scope = scope_tables.is_none_or(|s| tables.iter().any(|t| s.contains(*t)));
            in_scope.then_some((c, tables))
        })
        .collect();
    cards.sort_by_key(|(c, _)| {
        (
            c.trigger.target.clone(),
            crate::repo_links::method_rank(&c.trigger.method),
        )
    });
    cards
        .into_iter()
        .map(|(card, tables)| {
            let tables = crate::diagram_flow::table_uses(card, tables)
                .into_iter()
                .map(|u| FlowTableInfo {
                    table: name_of(&u.table),
                    ops: u.ops,
                    steps: u.steps.iter().map(|i| i + 1).collect(),
                })
                .collect();
            let steps = card
                .steps
                .iter()
                .map(|s| FlowStepInfo {
                    kind: s.kind,
                    title: s.title.clone(),
                    detail: s.detail.clone(),
                    target: s.target.as_ref().map(|t| match t {
                        FlowTarget::Table(id) => FlowTarget::Table(name_of(id)),
                        other => other.clone(),
                    }),
                    op: s.op,
                    columns: s.columns.clone(),
                    condition: s.condition.clone(),
                    source: s.source.clone(),
                })
                .collect();
            let meta = card.meta.as_ref();
            FlowInfo {
                id: card.id.clone(),
                trigger: card.trigger.kind,
                method: card.trigger.method.clone(),
                target: card.trigger.target.clone(),
                summary: card.summary.clone(),
                source: card.source.clone(),
                tables,
                steps,
                generated_at: meta
                    .map(|m| m.generated_at.clone())
                    .filter(|s| !s.is_empty()),
                commit: meta
                    .and_then(|m| m.commit.as_deref())
                    .filter(|c| !c.is_empty())
                    .map(|c| c.chars().take(7).collect()),
                partial: meta.is_some_and(|m| m.partial),
            }
        })
        .collect()
}

fn column_ref(r: &ColumnRef) -> String {
    if r.table.is_empty() {
        r.column.clone()
    } else {
        format!("{}.{}", r.table, r.column)
    }
}

/// Samarkan literal setelah `PASSWORD` / `IDENTIFIED BY` supaya kata sandi
/// yang pernah diketik user di editor tidak ikut terkirim ke agent.
pub fn redact_sql_secrets(sql: &str) -> String {
    let lower = sql.to_ascii_lowercase();
    let bytes = sql.as_bytes();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for keyword in ["identified by", "password"] {
        let mut from = 0;
        while let Some(pos) = lower[from..].find(keyword) {
            let mut i = from + pos + keyword.len();
            from = i;
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'=') {
                i += 1;
            }
            let Some(&quote) = bytes.get(i) else { break };
            if quote != b'\'' && quote != b'"' {
                continue;
            }
            if let Some(len) = sql[i + 1..].find(quote as char) {
                ranges.push((i + 1, i + 1 + len));
                from = i + 1 + len;
            }
        }
    }
    if ranges.is_empty() {
        return sql.to_string();
    }
    ranges.sort_unstable();
    let mut out = String::with_capacity(sql.len());
    let mut last = 0;
    for (start, end) in ranges {
        if start < last {
            continue;
        }
        out.push_str(&sql[last..start]);
        out.push_str("***");
        last = end;
    }
    out.push_str(&sql[last..]);
    out
}

/// Ringkasan diagram per tabel untuk memperkaya `describe_schema`.
#[derive(Debug, Default)]
pub struct DiagramContext {
    virtual_by_child: HashMap<String, Vec<VirtualRelationInfo>>,
    groups_by_table: HashMap<String, Vec<String>>,
    notes_by_table: HashMap<String, usize>,
}

impl DiagramContext {
    pub fn from_state(state: &DiagramState) -> Self {
        let mut ctx = Self::default();
        let names: HashMap<&str, &str> = state
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), node_name(n)))
            .collect();
        let name_of = |id: &str| names.get(id).copied().unwrap_or(id).to_string();

        for v in &state.virtual_relations {
            ctx.virtual_by_child
                .entry(name_of(&v.child).to_lowercase())
                .or_default()
                .push(VirtualRelationInfo {
                    child: name_of(&v.child),
                    child_column: v.child_column.clone(),
                    parent: name_of(&v.parent),
                    parent_column: v.parent_column.clone(),
                    origin: origin_label(&v.origin),
                });
        }
        for node in &state.nodes {
            let groups: Vec<String> = state
                .groups
                .iter()
                .filter(|g| node_in_group(node, &g.id))
                .map(|g| g.title.clone())
                .collect();
            if !groups.is_empty() {
                ctx.groups_by_table
                    .insert(node_name(node).to_lowercase(), groups);
            }
        }
        for note in &state.notes {
            let mut tables: HashSet<String> = HashSet::new();
            if let NoteAnchor::Table(id) = &note.anchor {
                tables.insert(name_of(id).to_lowercase());
            }
            for link in parse_wikilinks(&note.body) {
                if let Some(node) = resolve_table(&state.nodes, &link.target) {
                    tables.insert(node_name(node).to_lowercase());
                }
            }
            for t in tables {
                *ctx.notes_by_table.entry(t).or_default() += 1;
            }
        }
        ctx
    }

    fn lookup<'a, T>(map: &'a HashMap<String, T>, table: &str) -> Option<&'a T> {
        let lower = table.to_lowercase();
        map.get(&lower).or_else(|| {
            let short = short_name(&lower);
            map.iter()
                .find(|(k, _)| short_name(k) == short)
                .map(|(_, v)| v)
        })
    }

    /// (relasi virtual dengan tabel ini sebagai child, judul group, jumlah note).
    pub fn for_table(&self, table: &str) -> (Vec<VirtualRelationInfo>, Vec<String>, usize) {
        (
            Self::lookup(&self.virtual_by_child, table)
                .cloned()
                .unwrap_or_default(),
            Self::lookup(&self.groups_by_table, table)
                .cloned()
                .unwrap_or_default(),
            Self::lookup(&self.notes_by_table, table)
                .copied()
                .unwrap_or_default(),
        )
    }
}

/// Folder repository lokal per group, dibaca ulang tiap panggilan karena GUI
/// bisa mengubahnya selama proses MCP berjalan.
fn local_repo_paths(app_dir: &Path) -> crate::diagram_repo_paths::RepoPathStore {
    crate::diagram_repo_paths::RepoPathStore::load(
        app_dir.join(crate::diagram_repo_paths::FILE_NAME),
    )
}

impl HeadlessSession {
    /// Diagram dari cache JSON lokal (yang sama dengan GUI). Isi database
    /// yang di-link dibuang; hanya referensinya yang tersisa.
    pub(super) fn load_local_diagram(&self, id: i64, db: &str) -> Option<DiagramState> {
        let path = self
            .app_dir()
            .join("diagrams")
            .join(crate::diagram_storage::local_diagram_file_name(id, db));
        let text = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str::<DiagramState>(&text) {
            Ok(mut state) => {
                crate::diagram_links::strip_linked(&mut state);
                Some(state)
            }
            Err(e) => {
                log::warn!("[AGENT] cannot parse diagram {}: {e}", path.display());
                None
            }
        }
    }

    /// Diagram lokal, atau bila belum ada, diagram bersama dari tabel
    /// `diagram_by_tabular` di database target. Mengembalikan catatan bila
    /// diagram bersama gagal dimuat.
    async fn load_any_diagram(
        &self,
        id: i64,
        db: &str,
    ) -> (Option<(DiagramState, DiagramSource)>, Option<String>) {
        if let Some(state) = self.load_local_diagram(id, db) {
            return (Some((state, DiagramSource::LocalFile)), None);
        }
        let pool = match self.pool_for(id).await {
            Ok((_, pool)) => pool,
            Err(e) => return (None, Some(format!("shared diagram not checked: {e}"))),
        };
        let load = crate::diagram_storage::load_diagram_from_database(&pool, db, None);
        match tokio::time::timeout(REMOTE_DIAGRAM_TIMEOUT, load).await {
            Ok(Ok(Some(mut state))) => {
                crate::diagram_links::strip_linked(&mut state);
                (Some((state, DiagramSource::SharedTable)), None)
            }
            Ok(Ok(None)) => (None, None),
            Ok(Err(e)) => {
                log::warn!("[AGENT] shared diagram load failed: {e}");
                (None, Some(format!("shared diagram could not be read: {e}")))
            }
            Err(_) => (
                None,
                Some("shared diagram load timed out after 10s".to_string()),
            ),
        }
    }

    /// Group, relasi virtual, sticky note, dan link database dari diagram
    /// Tabular. `group` (id atau judul) dan `table` mempersempit hasil.
    pub async fn describe_diagram(
        &self,
        id: i64,
        database: Option<&str>,
        group: Option<&str>,
        table: Option<&str>,
    ) -> Result<DiagramDescription, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let (loaded, load_note) = self.load_any_diagram(id, &db).await;
        let Some((state, source)) = loaded else {
            return Ok(DiagramDescription {
                connection_id: id,
                database: db,
                found: false,
                source: None,
                title: None,
                table_count: 0,
                groups: Vec::new(),
                ungrouped_tables: Vec::new(),
                virtual_relations: Vec::new(),
                notes: Vec::new(),
                total_notes: 0,
                flows: Vec::new(),
                total_flows: 0,
                linked_databases: Vec::new(),
                note: Some(load_note.unwrap_or_else(|| {
                    "no Tabular diagram for this database yet; the user can open it from the \
                     sidebar (Show Diagram). describe_schema still works."
                        .to_string()
                })),
            });
        };
        let group = group.map(str::trim).filter(|s| !s.is_empty());
        let table = table.map(str::trim).filter(|s| !s.is_empty());
        let names: HashMap<&str, &str> = state
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), node_name(n)))
            .collect();
        let name_of = |id: &str| names.get(id).copied().unwrap_or(id).to_string();

        // Cakupan: id node dan id group yang relevan; None = semuanya.
        let mut scope_tables: Option<HashSet<String>> = None;
        let mut scope_groups: Option<HashSet<String>> = None;
        if let Some(g) = group {
            let found = state
                .groups
                .iter()
                .find(|x| x.id == g || x.title.eq_ignore_ascii_case(g))
                .ok_or_else(|| {
                    let titles: Vec<&str> = state.groups.iter().map(|x| x.title.as_str()).collect();
                    AgentError::Refused(format!(
                        "group \"{g}\" not found; groups in this diagram: {}",
                        if titles.is_empty() {
                            "(none)".to_string()
                        } else {
                            titles.join(", ")
                        }
                    ))
                })?;
            scope_groups = Some(HashSet::from([found.id.clone()]));
            scope_tables = Some(
                state
                    .nodes
                    .iter()
                    .filter(|n| node_in_group(n, &found.id))
                    .map(|n| n.id.clone())
                    .collect(),
            );
        }
        if let Some(t) = table {
            let node = resolve_table(&state.nodes, t).ok_or_else(|| {
                AgentError::Refused(format!(
                    "table \"{t}\" is not in the diagram; call describe_diagram without `table` to see what it contains"
                ))
            })?;
            let groups: HashSet<String> = state
                .groups
                .iter()
                .filter(|g| node_in_group(node, &g.id))
                .map(|g| g.id.clone())
                .collect();
            scope_groups = Some(match scope_groups {
                Some(prev) => prev.intersection(&groups).cloned().collect(),
                None => groups,
            });
            scope_tables = Some(HashSet::from([node.id.clone()]));
        }
        let table_in_scope = |id: &str| scope_tables.as_ref().is_none_or(|s| s.contains(id));
        let group_in_scope = |id: &str| scope_groups.as_ref().is_none_or(|s| s.contains(id));

        let repo_paths = local_repo_paths(self.app_dir());
        let note_count_for_group = |gid: &str| {
            state
                .notes
                .iter()
                .filter(|n| matches!(&n.anchor, NoteAnchor::Group(g) if g == gid))
                .count()
        };
        let groups: Vec<DiagramGroupInfo> = state
            .groups
            .iter()
            .filter(|g| group_in_scope(&g.id))
            .map(|g| {
                let mut tables: Vec<String> = state
                    .nodes
                    .iter()
                    .filter(|n| node_in_group(n, &g.id))
                    .map(|n| node_name(n).to_string())
                    .collect();
                tables.sort();
                DiagramGroupInfo {
                    id: g.id.clone(),
                    title: g.title.clone(),
                    tables,
                    repo_url: g.shared_repo_url().map(crate::repo_scan::redact),
                    local_repo_path: repo_paths.get(&g.id).map(str::to_string),
                    note_count: note_count_for_group(&g.id),
                }
            })
            .collect();

        let mut ungrouped_tables = Vec::new();
        if scope_tables.is_none() {
            ungrouped_tables = state
                .nodes
                .iter()
                .filter(|n| !state.groups.iter().any(|g| node_in_group(n, &g.id)))
                .map(|n| node_name(n).to_string())
                .collect();
            ungrouped_tables.sort();
        }

        let virtual_relations: Vec<VirtualRelationInfo> = state
            .virtual_relations
            .iter()
            .filter(|v| table_in_scope(&v.child) || table_in_scope(&v.parent))
            .map(|v| VirtualRelationInfo {
                child: name_of(&v.child),
                child_column: v.child_column.clone(),
                parent: name_of(&v.parent),
                parent_column: v.parent_column.clone(),
                origin: origin_label(&v.origin),
            })
            .collect();

        let group_titles: HashMap<&str, &str> = state
            .groups
            .iter()
            .map(|g| (g.id.as_str(), g.title.as_str()))
            .collect();
        let mut notes: Vec<(&crate::models::structs::DiagramNote, Vec<String>)> = state
            .notes
            .iter()
            .filter_map(|n| {
                let linked: Vec<&DiagramNode> = parse_wikilinks(&n.body)
                    .iter()
                    .filter_map(|l| resolve_table(&state.nodes, &l.target))
                    .collect();
                let relevant = match &n.anchor {
                    NoteAnchor::Table(id) => table_in_scope(id),
                    NoteAnchor::Group(gid) => group_in_scope(gid),
                } || linked
                    .iter()
                    .any(|node| scope_tables.as_ref().is_some_and(|s| s.contains(&node.id)));
                let mut links: Vec<String> = linked
                    .iter()
                    .map(|node| node_name(node).to_string())
                    .collect();
                links.dedup();
                relevant.then_some((n, links))
            })
            .collect();
        let total_notes = notes.len();
        notes.sort_by(|a, b| {
            b.0.pinned
                .cmp(&a.0.pinned)
                .then_with(|| b.0.updated_at.cmp(&a.0.updated_at))
        });
        let notes = notes
            .into_iter()
            .take(MAX_NOTES)
            .map(|(n, links)| {
                let (body, truncated) = clip_chars(&n.body, MAX_NOTE_CHARS);
                let (anchor_type, anchor) = match &n.anchor {
                    NoteAnchor::Table(id) => ("table", name_of(id)),
                    NoteAnchor::Group(gid) => (
                        "group",
                        group_titles
                            .get(gid.as_str())
                            .copied()
                            .unwrap_or(gid)
                            .to_string(),
                    ),
                };
                DiagramNoteInfo {
                    id: n.id.clone(),
                    title: n.title.clone(),
                    anchor_type,
                    anchor,
                    pinned: n.pinned,
                    author: n.author.clone(),
                    updated_at: n.updated_at.clone(),
                    links,
                    body,
                    truncated,
                }
            })
            .collect::<Vec<_>>();

        let mut flows = flow_infos(&state, scope_tables.as_ref(), &name_of);
        let total_flows = flows.len();
        flows.truncate(MAX_FLOWS);

        let mut clipped: Vec<String> = Vec::new();
        if total_notes > notes.len() {
            clipped.push(format!("showing {} of {total_notes} notes", notes.len()));
        }
        if total_flows > flows.len() {
            clipped.push(format!(
                "showing {} of {total_flows} business processes",
                flows.len()
            ));
        }
        let mut note = load_note;
        if !clipped.is_empty() {
            note = Some(format!(
                "{}; pass `table` or `group` to narrow",
                clipped.join(", ")
            ));
        }

        Ok(DiagramDescription {
            connection_id: id,
            database: db,
            found: true,
            source: Some(source),
            title: state.diagram_title.clone(),
            table_count: state.nodes.len(),
            groups,
            ungrouped_tables,
            virtual_relations,
            notes,
            total_notes,
            flows,
            total_flows,
            linked_databases: state
                .linked_databases
                .iter()
                .map(|l| LinkedDatabaseInfo {
                    connection_id: l.connection_id,
                    connection_name: l.connection_name.clone(),
                    database: l.database_name.clone(),
                })
                .collect(),
            note,
        })
    }

    /// Query yang pernah dijalankan user dan mirip dengan `question`.
    pub async fn search_query_history(
        &self,
        question: &str,
        connection_id: Option<i64>,
        limit: Option<usize>,
        include_agent: bool,
    ) -> Result<QueryHistoryResult, AgentError> {
        let question = question.trim();
        if question.is_empty() {
            return Err(AgentError::Refused(
                "pass `question` with table names or what the query should do".to_string(),
            ));
        }
        let pool = self.cache_pool();
        crate::vector_index::sync_history_embeddings(pool).await?;
        let mut hits = crate::vector_index::search_history_hits(
            pool,
            question,
            connection_id,
            include_agent,
            limit.unwrap_or(10).clamp(1, 50),
            crate::vector_index::HISTORY_MAX_DISTANCE,
        )
        .await?;
        for hit in &mut hits {
            let redacted = redact_sql_secrets(&hit.query_text);
            hit.query_text = clip_chars(&redacted, MAX_HISTORY_CHARS).0;
        }
        Ok(QueryHistoryResult {
            hint: hits.is_empty().then(|| {
                "no similar queries in the user's history; try table or column names as keywords"
                    .to_string()
            }),
            results: hits,
        })
    }

    /// Kolom tiap tabel dari `column_cache`, dikunci nama tabel lowercase.
    async fn cached_columns_for(
        &self,
        id: i64,
        db: &str,
        tables: &[String],
    ) -> HashMap<String, Vec<String>> {
        let mut out = HashMap::new();
        for table in tables {
            for name in [table.as_str(), short_name(table)] {
                let cols: Vec<(String,)> = sqlx::query_as(
                    "SELECT column_name FROM column_cache WHERE connection_id = ? \
                     AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE \
                     ORDER BY ordinal_position",
                )
                .bind(id)
                .bind(db)
                .bind(name)
                .fetch_all(self.cache_pool())
                .await
                .unwrap_or_default();
                if !cols.is_empty() {
                    out.insert(
                        table.to_lowercase(),
                        cols.into_iter().map(|(c,)| c).collect(),
                    );
                    break;
                }
            }
        }
        out
    }

    /// Kolom pertama tiap index tabel (lowercase); `None` bila tabel tidak
    /// punya data index di cache.
    async fn leading_index_columns(
        &self,
        id: i64,
        db: &str,
        table: &str,
    ) -> Option<HashSet<String>> {
        for name in [table, short_name(table)] {
            let rows: Vec<(String,)> = sqlx::query_as(
                "SELECT columns_json FROM index_cache WHERE connection_id = ? \
                 AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE",
            )
            .bind(id)
            .bind(db)
            .bind(name)
            .fetch_all(self.cache_pool())
            .await
            .unwrap_or_default();
            if !rows.is_empty() {
                return Some(
                    rows.into_iter()
                        .filter_map(|(json,)| {
                            serde_json::from_str::<Vec<String>>(&json)
                                .ok()?
                                .into_iter()
                                .next()
                        })
                        .map(|c| c.to_lowercase())
                        .collect(),
                );
            }
        }
        None
    }

    /// Analisis alur data satu statement tanpa menjalankannya.
    pub async fn analyze_query(
        &self,
        connection_id: Option<i64>,
        sql: &str,
        database: Option<&str>,
    ) -> Result<QueryAnalysis, AgentError> {
        let (db_type, cache_ctx) = match connection_id {
            Some(id) => {
                let conn = self.load_connection(id).await?;
                if !query_diagram::supports_database(&conn.connection_type) {
                    return Err(AgentError::Unsupported(
                        id,
                        conn.connection_type.display_name(),
                    ));
                }
                let db = self.resolve_database(&conn, database).await.ok();
                (conn.connection_type.clone(), db.map(|db| (id, db)))
            }
            None => (DatabaseType::PostgreSQL, None),
        };

        let first = query_diagram::analyze_statement(sql, &db_type)
            .map_err(|e| AgentError::Query(e.to_string()))?;
        let mut columns = HashMap::new();
        if let Some((id, db)) = &cache_ctx {
            columns = self
                .cached_columns_for(*id, db, &first.physical_tables())
                .await;
        }
        let schema_resolved = !columns.is_empty();
        let model = if schema_resolved {
            let lookup = |table: &str| {
                let lower = table.to_lowercase();
                columns.get(&lower).cloned().or_else(|| {
                    columns
                        .iter()
                        .find(|(k, _)| short_name(k) == short_name(&lower))
                        .map(|(_, v)| v.clone())
                })
            };
            query_diagram::analyze_with_schema(sql, &db_type, &lookup)
                .map_err(|e| AgentError::Query(e.to_string()))?
        } else {
            first
        };

        let mut unindexed = Vec::new();
        if let Some((id, db)) = &cache_ctx {
            unindexed = self.unindexed_columns(*id, db, &model).await;
        }
        Ok(summarize_model(&model, schema_resolved, unindexed))
    }

    /// Kolom join/filter pada tabel fisik yang tidak menjadi kolom pertama
    /// index mana pun.
    async fn unindexed_columns(&self, id: i64, db: &str, model: &QueryDiagramModel) -> Vec<String> {
        let mut used: Vec<(String, String, &'static str)> = Vec::new();
        let mut push = |r: &ColumnRef, why: &'static str| {
            if let Some(t) = model.table(&r.table)
                && t.kind == query_diagram::SourceKind::Table
                && !used
                    .iter()
                    .any(|(tb, c, _)| tb == &t.table && c.eq_ignore_ascii_case(&r.column))
            {
                used.push((t.table.clone(), r.column.clone(), why));
            }
        };
        for j in &model.joins {
            push(&j.left, "join");
            push(&j.right, "join");
        }
        for f in &model.filter_columns {
            push(f, "filter");
        }

        let mut cache: HashMap<String, Option<HashSet<String>>> = HashMap::new();
        let mut out = Vec::new();
        for (table, column, why) in used {
            if !cache.contains_key(&table) {
                let lead = self.leading_index_columns(id, db, &table).await;
                cache.insert(table.clone(), lead);
            }
            if let Some(Some(lead)) = cache.get(&table)
                && !lead.contains(&column.to_lowercase())
            {
                out.push(format!("{table}.{column} ({why})"));
            }
        }
        out
    }

    /// Cari pemakaian tabel di kode repository milik group diagram (folder
    /// lokal atau clone yang sudah ada di cache Tabular). Tidak pernah
    /// menjalankan git atau membaca folder lain.
    pub async fn find_table_usages(
        &self,
        id: i64,
        database: Option<&str>,
        tables: &[String],
        group: Option<&str>,
        max_evidence: Option<usize>,
    ) -> Result<TableUsageReport, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let (loaded, load_note) = self.load_any_diagram(id, &db).await;
        let Some((state, _)) = loaded else {
            return Err(AgentError::Refused(load_note.unwrap_or_else(|| {
                "no Tabular diagram for this database, so no repository is linked; the user can \
                 set one per group in the diagram (Group Repository)"
                    .to_string()
            })));
        };
        let group = group.map(str::trim).filter(|s| !s.is_empty());
        let groups: Vec<&crate::models::structs::DiagramGroup> = state
            .groups
            .iter()
            .filter(|g| group.is_none_or(|q| g.id == q || g.title.eq_ignore_ascii_case(q)))
            .collect();
        if let Some(q) = group
            && groups.is_empty()
        {
            return Err(AgentError::Refused(format!(
                "group \"{q}\" not found in the diagram"
            )));
        }

        let mut wanted: Vec<String> = tables
            .iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if wanted.is_empty() {
            if group.is_none() {
                return Err(AgentError::Refused(
                    "pass `tables`, or `group` to search all tables of that group".to_string(),
                ));
            }
            for g in &groups {
                for n in state.nodes.iter().filter(|n| node_in_group(n, &g.id)) {
                    wanted.push(node_name(n).to_string());
                }
            }
        }
        wanted.sort();
        wanted.dedup();
        if wanted.is_empty() {
            return Err(AgentError::Refused("the group has no tables".to_string()));
        }

        // Root repository unik → (judul group, sumber).
        let repo_paths = local_repo_paths(self.app_dir());
        let mut roots: Vec<(PathBuf, Vec<String>, &'static str)> = Vec::new();
        let mut skipped = Vec::new();
        for g in &groups {
            let local = repo_paths
                .get(&g.id)
                .map(crate::repo_scan::expand_home)
                .filter(|p| p.is_dir());
            let resolved = match local {
                Some(p) => Some((p, "local_path")),
                None => g
                    .shared_repo_url()
                    .and_then(crate::repo_scan::cached_clone_dir)
                    .map(|p| (p, "cached_clone")),
            };
            match resolved {
                Some((root, source)) => {
                    let root = root.canonicalize().unwrap_or(root);
                    match roots.iter_mut().find(|(r, _, _)| *r == root) {
                        Some((_, titles, _)) => titles.push(g.title.clone()),
                        None => roots.push((root, vec![g.title.clone()], source)),
                    }
                }
                None if g.shared_repo_url().is_some() || repo_paths.get(&g.id).is_some() => {
                    skipped.push(format!(
                        "{}: repository not available on this computer; the user can set a local \
                         folder or run \"Suggest tables\" once in the diagram to clone it",
                        g.title
                    ));
                }
                None => {}
            }
        }
        if roots.is_empty() {
            return Err(AgentError::Refused(format!(
                "no readable repository linked to {}; the user can link one in the diagram \
                 (group menu > Repository){}",
                group.map_or("any diagram group".to_string(), |g| format!(
                    "group \"{g}\""
                )),
                if skipped.is_empty() {
                    String::new()
                } else {
                    format!(". {}", skipped.join("; "))
                }
            )));
        }

        let max_evidence = max_evidence.unwrap_or(3).clamp(1, 5);
        let candidates: Vec<crate::repo_scan::Candidate> = wanted
            .iter()
            .map(|t| crate::repo_scan::Candidate {
                id: t.clone(),
                title: t.clone(),
            })
            .collect();
        let mut repositories = Vec::new();
        let mut found: HashSet<String> = HashSet::new();
        for (root, titles, source) in roots {
            let cancel = Arc::new(AtomicBool::new(false));
            let (cands, flag, dir) = (candidates.clone(), cancel.clone(), root.clone());
            let task = tokio::task::spawn_blocking(move || {
                crate::repo_scan::grep_tables(&dir, &cands, &flag)
            });
            let report = match tokio::time::timeout(GREP_TIMEOUT, task).await {
                Ok(Ok(Ok(r))) => r,
                Ok(Ok(Err(e))) => {
                    skipped.push(format!("{}: {e}", root.display()));
                    continue;
                }
                Ok(Err(e)) => {
                    skipped.push(format!("{}: scan crashed: {e}", root.display()));
                    continue;
                }
                Err(_) => {
                    cancel.store(true, Ordering::Relaxed);
                    skipped.push(format!("{}: scan timed out", root.display()));
                    continue;
                }
            };
            let hits: Vec<UsageHit> = report
                .hits
                .into_iter()
                .map(|h| {
                    found.insert(h.id.clone());
                    UsageHit {
                        table: h.title,
                        refs: h.refs,
                        files: h.files,
                        score: h.score,
                        evidence: h
                            .evidence
                            .into_iter()
                            .take(max_evidence)
                            .map(|e| format!("{}:{}: {}", e.path, e.line, e.snippet.trim()))
                            .collect(),
                    }
                })
                .collect();
            repositories.push(RepoUsage {
                groups: titles,
                root: root.display().to_string(),
                source,
                files_scanned: report.files_scanned,
                truncated: report.truncated,
                hits,
            });
        }
        let not_found = wanted
            .iter()
            .filter(|t| !found.contains(*t))
            .cloned()
            .collect();
        Ok(TableUsageReport {
            tables: wanted,
            repositories,
            not_found,
            skipped,
        })
    }
}

/// Ubah model query diagram menjadi ringkasan yang ringkas untuk agent.
fn summarize_model(
    model: &QueryDiagramModel,
    schema_resolved: bool,
    unindexed_columns: Vec<String>,
) -> QueryAnalysis {
    let sources = model
        .sources
        .iter()
        .map(|t| AnalyzedSource {
            table: t.table.clone(),
            alias: t
                .alias
                .clone()
                .filter(|a| !a.eq_ignore_ascii_case(&t.table)),
            kind: format!("{:?}", t.kind).to_lowercase(),
            join: t.join.clone(),
            role: t.badge.clone(),
            used_columns: t.used.clone(),
            all_columns: t.all_columns,
        })
        .collect();
    let output_truncated = model.output.len() > MAX_OUTPUT_COLUMNS;
    QueryAnalysis {
        statement: model.kind.label().to_string(),
        summary: model.kind.summary().to_string(),
        verb: model.verb.clone(),
        target: model.target.as_ref().map(|t| t.title()),
        sources,
        joins: model
            .joins
            .iter()
            .map(|j| {
                format!(
                    "{} = {} ({})",
                    column_ref(&j.left),
                    column_ref(&j.right),
                    j.join_type
                )
            })
            .collect(),
        output: model
            .output
            .iter()
            .take(MAX_OUTPUT_COLUMNS)
            .map(|o| AnalyzedOutput {
                name: o.name.clone(),
                expr: o.expr.clone(),
                sources: o.sources.iter().map(column_ref).collect(),
                aggregate: o.aggregate,
                window: o.window.clone(),
            })
            .collect(),
        output_truncated,
        mutations: model
            .mutations
            .iter()
            .chain(model.upserts.iter())
            .map(|m| format!("{} = {}", m.column, m.new_value))
            .collect(),
        filter: model.filter.clone(),
        filter_columns: model.filter_columns.iter().map(column_ref).collect(),
        group_by: model.group_by.clone(),
        having: model.having.clone(),
        order_by: model.order_by.clone(),
        limit: model.limit.clone(),
        distinct: model.distinct,
        notes: model.notes.clone(),
        hints: query_diagram::prompt::quick_hints(model),
        unindexed_columns,
        schema_resolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{
        DiagramGroup, DiagramNote, EndpointLink, FlowCard, FlowMeta, FlowStep, FlowTrigger,
        VirtualRelation,
    };
    use eframe::egui;

    fn node(id: &str, cols: &[&str], group: Option<&str>) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            pos: egui::pos2(0.0, 0.0),
            size: egui::vec2(200.0, 120.0),
            columns: cols.iter().map(|c| c.to_string()).collect(),
            foreign_keys: Vec::new(),
            group_ids: group.map(|g| vec![g.to_string()]).unwrap_or_default(),
            group_id: None,
            column_meta: Vec::new(),
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        }
    }

    fn note(id: &str, anchor: NoteAnchor, body: &str, pinned: bool) -> DiagramNote {
        DiagramNote {
            id: id.into(),
            anchor,
            title: format!("title {id}"),
            body: body.into(),
            color: crate::diagram_notes::NOTE_COLORS[0],
            offset: None,
            size: crate::diagram_notes::DEFAULT_NOTE_SIZE,
            pinned,
            author: None,
            created_at: String::new(),
            updated_at: id.into(),
        }
    }

    fn sample_state() -> DiagramState {
        DiagramState {
            nodes: vec![
                node("users", &["id", "email"], Some("g1")),
                node("orders", &["id", "user_id", "status"], Some("g1")),
                node("audit_log", &["id"], None),
            ],
            groups: vec![DiagramGroup {
                id: "g1".into(),
                title: "Billing".into(),
                color: egui::Color32::WHITE,
                manual_pos: None,
                repo_url: Some("https://user:secret@example.com/acme/billing.git".into()),
            }],
            virtual_relations: vec![VirtualRelation {
                child: "orders".into(),
                child_column: "user_id".into(),
                parent: "users".into(),
                parent_column: "id".into(),
                origin: RelationOrigin::Manual,
            }],
            notes: vec![
                note(
                    "n1",
                    NoteAnchor::Table("orders".into()),
                    "Status 3 = void. See [[users]].",
                    false,
                ),
                note(
                    "n2",
                    NoteAnchor::Group("g1".into()),
                    "Billing domain owns invoices.",
                    true,
                ),
                note(
                    "n3",
                    NoteAnchor::Table("audit_log".into()),
                    "Append only.",
                    false,
                ),
            ],
            flow_cards: vec![
                FlowCard {
                    id: "flw_1".into(),
                    trigger: FlowTrigger {
                        kind: FlowTriggerKind::Http,
                        method: "POST".into(),
                        target: "/orders".into(),
                    },
                    summary: "Creates an order".into(),
                    request_id: Some("req_local".into()),
                    pos: Some([10.0, 20.0]),
                    steps: vec![
                        FlowStep {
                            kind: FlowStepKind::Db,
                            title: "Load the customer".into(),
                            target: Some(FlowTarget::Table("users".into())),
                            op: Some(FlowOp::Read),
                            columns: vec!["id".into()],
                            source: Some("src/orders.ts:12".into()),
                            ..Default::default()
                        },
                        FlowStep {
                            kind: FlowStepKind::Db,
                            title: "Create the order".into(),
                            target: Some(FlowTarget::Table("orders".into())),
                            op: Some(FlowOp::Insert),
                            ..Default::default()
                        },
                    ],
                    meta: Some(FlowMeta {
                        commit: Some("a1b2c3d4e5f6".into()),
                        generated_at: "2026-09-30T10:00:00Z".into(),
                        backend: "Claude Code".into(),
                        partial: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                FlowCard {
                    id: "flw_2".into(),
                    trigger: FlowTrigger {
                        kind: FlowTriggerKind::Http,
                        method: "GET".into(),
                        target: "/audit".into(),
                    },
                    ..Default::default()
                },
            ],
            endpoint_links: vec![EndpointLink {
                table: "audit_log".into(),
                method: "GET".into(),
                path: "/audit".into(),
                summary: String::new(),
                request_id: None,
                repo_key: None,
                source: None,
            }],
            diagram_title: Some("Shop".into()),
            ..Default::default()
        }
    }

    #[test]
    fn flow_infos_use_table_names_and_skip_gui_fields() {
        let mut state = sample_state();
        // Id node berbeda dari judulnya: agent harus melihat nama tabel.
        state.nodes[0].title = "public.users".into();
        let name_of = |id: &str| {
            state
                .nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| node_name(n).to_string())
                .unwrap_or_else(|| id.to_string())
        };
        let all = flow_infos(&state, None, &name_of);
        let targets: Vec<&str> = all.iter().map(|f| f.target.as_str()).collect();
        assert_eq!(targets, vec!["/audit", "/orders"]);

        let post = &all[1];
        assert_eq!(post.tables.len(), 2);
        assert_eq!(post.tables[0].table, "public.users");
        assert_eq!(post.tables[0].ops, vec![FlowOp::Read]);
        assert_eq!(post.tables[0].steps, vec![1]);
        assert_eq!(
            post.steps[0].target,
            Some(FlowTarget::Table("public.users".into()))
        );
        assert_eq!(post.commit.as_deref(), Some("a1b2c3d"));
        assert!(post.partial);

        let json = serde_json::to_value(post).expect("json");
        assert_eq!(json["trigger"], "http");
        assert_eq!(json["steps"][1]["op"], "insert");
        assert_eq!(json["steps"][0]["target"]["kind"], "table");
        for gui_only in ["request_id", "pos", "repo_key", "collapsed", "meta"] {
            assert!(json.get(gui_only).is_none(), "{gui_only} leaked: {json}");
        }

        // Card tanpa langkah: tabel dari link, tanpa operasi, tanpa `partial`.
        let get = serde_json::to_value(&all[0]).expect("json");
        assert_eq!(get["tables"][0]["table"], "audit_log");
        assert_eq!(get["tables"][0]["ops"], serde_json::json!([]));
        assert!(get.get("partial").is_none(), "{get}");

        let scope = HashSet::from(["orders".to_string()]);
        let scoped = flow_infos(&state, Some(&scope), &name_of);
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, "flw_1");
    }

    #[test]
    fn diagram_context_maps_tables() {
        let ctx = DiagramContext::from_state(&sample_state());
        let (virt, groups, notes) = ctx.for_table("ORDERS");
        assert_eq!(virt.len(), 1);
        assert_eq!(virt[0].parent, "users");
        assert_eq!(virt[0].origin, "manual");
        assert_eq!(groups, vec!["Billing"]);
        assert_eq!(notes, 1);
        // Dengan prefix schema tetap cocok, dan note [[users]] terhitung.
        let (virt, _, notes) = ctx.for_table("public.users");
        assert!(virt.is_empty());
        assert_eq!(notes, 1);
        assert_eq!(ctx.for_table("nope"), (Vec::new(), Vec::new(), 0));
    }

    #[test]
    fn redacts_passwords_in_history() {
        assert_eq!(
            redact_sql_secrets("CREATE USER a IDENTIFIED BY 'hunter2'"),
            "CREATE USER a IDENTIFIED BY '***'"
        );
        assert_eq!(
            redact_sql_secrets("ALTER ROLE x WITH PASSWORD \"p@ss\"; SELECT password FROM t"),
            "ALTER ROLE x WITH PASSWORD \"***\"; SELECT password FROM t"
        );
        assert_eq!(redact_sql_secrets("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn clip_chars_is_utf8_safe() {
        assert_eq!(clip_chars("héllo", 2), ("hé…".to_string(), true));
        assert_eq!(clip_chars("hé", 5), ("hé".to_string(), false));
    }

    /// Cache in-memory minimal + satu koneksi SQLite (tidak pernah dibuka).
    async fn session_with_diagram() -> (HeadlessSession, std::path::PathBuf) {
        crate::vector_index::register_sqlite_vec();
        let cache = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("cache");
        sqlx::query(
            "CREATE TABLE connections (id INTEGER PRIMARY KEY, name TEXT, host TEXT, port TEXT, \
             username TEXT, password TEXT, database_name TEXT, connection_type TEXT, folder TEXT, \
             ssh_enabled INTEGER, ssh_host TEXT, ssh_port TEXT, ssh_username TEXT, ssh_auth_method TEXT, \
             ssh_private_key TEXT, ssh_password TEXT, ssh_accept_unknown_host_keys INTEGER, ssh_jump_host TEXT, \
             ssl_enabled INTEGER, ssl_ca_cert TEXT, ssl_client_cert TEXT, ssl_client_key TEXT, \
             ssl_key_passphrase TEXT, ssl_verify_server INTEGER); \
             CREATE TABLE column_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER, \
             database_name TEXT, table_name TEXT, column_name TEXT, data_type TEXT, ordinal_position INTEGER, \
             is_primary_key INTEGER); \
             CREATE TABLE index_cache (id INTEGER PRIMARY KEY AUTOINCREMENT, connection_id INTEGER, \
             database_name TEXT, table_name TEXT, index_name TEXT, method TEXT, is_unique INTEGER, columns_json TEXT); \
             CREATE TABLE query_history (id INTEGER PRIMARY KEY AUTOINCREMENT, query_text TEXT NOT NULL, \
             connection_id INTEGER NOT NULL, connection_name TEXT NOT NULL, executed_at DATETIME DEFAULT CURRENT_TIMESTAMP); \
             INSERT INTO connections (id, name, host, port, username, password, database_name, connection_type) \
             VALUES (3, 'shop', '/nonexistent/shop.db', '', '', '', 'main', 'SQLite'); \
             INSERT INTO column_cache (connection_id, database_name, table_name, column_name, data_type, ordinal_position) VALUES \
             (3, 'main', 'orders', 'id', 'int', 1), (3, 'main', 'orders', 'user_id', 'int', 2), \
             (3, 'main', 'orders', 'status', 'int', 3), (3, 'main', 'users', 'id', 'int', 1), \
             (3, 'main', 'users', 'email', 'text', 2); \
             INSERT INTO index_cache (connection_id, database_name, table_name, index_name, is_unique, columns_json) VALUES \
             (3, 'main', 'users', 'pk_users', 1, '[\"id\"]'), (3, 'main', 'orders', 'pk_orders', 1, '[\"id\"]');",
        )
        .execute(&cache)
        .await
        .expect("schema");

        let dir = std::env::temp_dir().join(format!(
            "tabular-knowledge-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("diagrams")).expect("dir");
        std::fs::write(
            dir.join("diagrams")
                .join(crate::diagram_storage::local_diagram_file_name(3, "main")),
            serde_json::to_string(&sample_state()).expect("json"),
        )
        .expect("write diagram");
        (HeadlessSession::new(cache).with_app_dir(&dir), dir)
    }

    #[tokio::test]
    async fn describe_diagram_filters_and_redacts() {
        let (session, dir) = session_with_diagram().await;

        let all = session.describe_diagram(3, None, None, None).await.unwrap();
        assert!(all.found);
        assert_eq!(all.source, Some(DiagramSource::LocalFile));
        assert_eq!(all.title.as_deref(), Some("Shop"));
        assert_eq!(all.groups.len(), 1);
        assert_eq!(all.groups[0].tables, vec!["orders", "users"]);
        assert_eq!(
            all.groups[0].repo_url.as_deref(),
            Some("https://***@example.com/acme/billing.git")
        );
        assert_eq!(all.ungrouped_tables, vec!["audit_log"]);
        assert_eq!(all.total_notes, 3);
        // Note pinned didahulukan.
        assert_eq!(all.notes[0].id, "n2");
        assert_eq!(all.total_flows, 2);

        let orders = session
            .describe_diagram(3, None, None, Some("orders"))
            .await
            .unwrap();
        assert_eq!(orders.virtual_relations.len(), 1);
        let ids: Vec<&str> = orders.notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["n2", "n1"]);
        assert!(orders.ungrouped_tables.is_empty());
        let flows: Vec<&str> = orders.flows.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(flows, vec!["flw_1"]);

        let users = session
            .describe_diagram(3, None, None, Some("users"))
            .await
            .unwrap();
        // n1 menyebut [[users]].
        assert!(users.notes.iter().any(|n| n.id == "n1"));
        assert!(!users.notes.iter().any(|n| n.id == "n3"));

        let err = session
            .describe_diagram(3, None, Some("Nope"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Billing"), "{err}");

        // Repository: tanpa folder lokal dan tanpa clone, ditolak dengan penjelasan.
        let err = session
            .find_table_usages(3, None, &["orders".to_string()], None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AgentError::Refused(_)), "{err}");

        // Dengan folder lokal, pemakaian tabel ditemukan.
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(
            repo.join("src/billing.py"),
            "rows = db.execute(\"SELECT * FROM orders WHERE user_id = ?\")\n",
        )
        .unwrap();
        std::fs::write(
            dir.join(crate::diagram_repo_paths::FILE_NAME),
            serde_json::json!({ "paths": { "g1": repo.to_string_lossy() } }).to_string(),
        )
        .unwrap();
        let report = session
            .find_table_usages(3, None, &[], Some("Billing"), None)
            .await
            .unwrap();
        assert_eq!(report.tables, vec!["orders", "users"]);
        assert_eq!(report.repositories.len(), 1);
        assert_eq!(report.repositories[0].source, "local_path");
        let hit = &report.repositories[0].hits[0];
        assert_eq!(hit.table, "orders");
        assert!(hit.evidence[0].starts_with("src/billing.py:1:"));
        assert_eq!(report.not_found, vec!["users"]);

        // describe_schema ikut membawa relasi virtual, group, index, dan notes.
        sqlx::query(
            "CREATE TABLE table_cache (connection_id INTEGER, database_name TEXT, table_name TEXT, table_type TEXT); \
             CREATE TABLE foreign_key_cache (connection_id INTEGER, database_name TEXT, table_name TEXT, \
             column_name TEXT, referenced_table_name TEXT, referenced_column_name TEXT); \
             INSERT INTO table_cache VALUES (3, 'main', 'orders', 'table'), (3, 'main', 'users', 'table');",
        )
        .execute(session.cache_pool())
        .await
        .unwrap();
        let schema = session.describe_schema(3, None, None, None).await.unwrap();
        assert!(
            schema
                .ddl
                .contains("-- VIRTUAL FK orders.user_id -> users.id (manual")
        );
        assert!(schema.ddl.contains("-- INDEX pk_orders (id) UNIQUE"));
        assert!(schema.ddl.contains("-- diagram groups: Billing"));
        let er = schema.to_er_model();
        assert!(
            er.relations
                .iter()
                .any(|r| r.child == "orders" && r.inferred)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn analyze_query_resolves_schema_and_flags_unindexed() {
        let (session, dir) = session_with_diagram().await;
        let res = session
            .analyze_query(
                Some(3),
                "SELECT u.email, o.status FROM orders o JOIN users u ON u.id = o.user_id WHERE o.status = 3",
                None,
            )
            .await
            .unwrap();
        assert_eq!(res.statement, "SELECT");
        assert!(res.schema_resolved);
        assert_eq!(res.sources.len(), 2);
        assert_eq!(res.joins.len(), 1);
        assert!(
            res.unindexed_columns
                .contains(&"orders.user_id (join)".to_string())
        );
        assert!(
            res.unindexed_columns
                .contains(&"orders.status (filter)".to_string())
        );
        assert!(
            !res.unindexed_columns
                .iter()
                .any(|c| c.starts_with("users.id"))
        );

        let err = session.analyze_query(None, "", None).await.unwrap_err();
        assert!(matches!(err, AgentError::Query(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn search_query_history_filters_agent_and_redacts() {
        let (session, dir) = session_with_diagram().await;
        for (q, name) in [
            ("SELECT * FROM invoices WHERE due_date < now()", "shop"),
            ("SELECT * FROM invoices WHERE due_date < now()", "shop"),
            ("SELECT id FROM invoices WHERE paid = 0", "shop (agent)"),
            ("ALTER USER bob IDENTIFIED BY 'invoice-secret'", "shop"),
        ] {
            sqlx::query("INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, 3, ?)")
                .bind(q)
                .bind(name)
                .execute(session.cache_pool())
                .await
                .unwrap();
        }
        let res = session
            .search_query_history("overdue invoices due date", Some(3), None, false)
            .await
            .unwrap();
        assert!(!res.results.is_empty());
        assert!(
            res.results
                .iter()
                .all(|h| !h.connection_name.ends_with("(agent)"))
        );
        // Duplikat digabung.
        let dup = res
            .results
            .iter()
            .filter(|h| h.query_text.contains("due_date"))
            .count();
        assert_eq!(dup, 1);
        assert!(
            res.results
                .iter()
                .all(|h| !h.query_text.contains("invoice-secret"))
        );

        let none = session
            .search_query_history("invoices", Some(99), None, true)
            .await
            .unwrap();
        assert!(none.results.is_empty() && none.hint.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
