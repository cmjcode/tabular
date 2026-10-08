//! State dan aksi fitur data grid (bagian B checklist TablePro).
//!
//! UI-nya ada di `grid_ui.rs`; logika murni di `grid_model.rs`; persistensi
//! filter & highlight rule di `grid_prefs.rs`. Semua aksi yang mengubah grid
//! dijalankan setelah borrow render selesai lewat [`GridRequests`].

use super::grid_model::{self as gm, OpSummary};
use super::grid_prefs::{self, SavedFilter, SavedFilterPayload, TableKey};
use crate::models::enums::DatabaseType;
use crate::models::structs::{CellEditOperation, FilterCondition, FilterOperator, ForeignKey};
use crate::spreadsheet::SpreadsheetOperations;
use crate::window_egui::Tabular;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Umur maksimum peta FK per grid sebelum cache SQLite dibaca ulang. Cache FK
/// diisi task latar tanpa sinyal, jadi cukup dicek ulang sesekali, bukan
/// setiap frame.
const FK_CACHE_TTL: Duration = Duration::from_secs(2);

const UNDO_LIMIT: usize = 200;
const REWIND_LIMIT: usize = 50;
/// Kedalaman maksimum drill foreign key di viewer "Row as JSON".
pub const MAX_FK_DEPTH: usize = 5;
pub const FK_PICKER_LIMIT: usize = 100;

// ─── Undo / redo (B2) ──────────────────────────────────────────────────────

/// Perubahan data grid yang bisa dibalik. Antrean operasi pending disimpan
/// utuh sebagai snapshot di [`UndoEntry`], jadi di sini cukup data selnya.
#[derive(Clone, Debug)]
pub enum DataChange {
    Cell {
        row: usize,
        col: usize,
        before: String,
        after: String,
    },
    InsertRow {
        row: usize,
        values: Vec<String>,
    },
    RemoveRow {
        row: usize,
        values: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub label: String,
    pub ops_before: Vec<CellEditOperation>,
    pub ops_after: Vec<CellEditOperation>,
    pub changes: Vec<DataChange>,
}

// ─── Review SQL (B1) & rewind (B3) ─────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum ReviewKind {
    SaveChanges,
    Rewind { entry_id: u64 },
    Ddl,
}

#[derive(Clone, Debug)]
pub struct ChangeReview {
    pub kind: ReviewKind,
    pub title: String,
    pub summary: String,
    pub sql: String,
    pub warnings: Vec<String>,
    pub connection_id: i64,
}

/// SQL pembalik yang disiapkan sebelum simpan, dicatat ke riwayat bila
/// simpan sukses.
#[derive(Clone, Debug, Default)]
pub struct PendingRewind {
    pub sql: Option<String>,
    pub notes: Vec<String>,
    pub forward_sql: String,
    pub summary: OpSummary,
    pub table: String,
}

#[derive(Clone, Debug)]
pub struct RewindEntry {
    pub id: u64,
    pub committed_at: String,
    pub connection_id: i64,
    pub table: String,
    pub summary: OpSummary,
    pub forward_sql: String,
    pub rewind_sql: Option<String>,
    pub notes: Vec<String>,
    pub restored: bool,
}

// ─── Find, jump, kolom ─────────────────────────────────────────────────────

type FindCacheKey = (String, bool, usize, usize, usize, usize);

#[derive(Clone, Debug, Default)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub case_sensitive: bool,
    pub focus_request: bool,
    pub matches: Vec<(usize, usize)>,
    /// Himpunan `matches` untuk lookup O(1) saat menggambar sel; dibangun
    /// sekali ketika hasil dihitung, bukan setiap frame.
    pub match_set: Arc<HashSet<(usize, usize)>>,
    pub current: usize,
    cache_key: Option<FindCacheKey>,
    /// Pencarian sedang diterapkan sebagai filter server-side.
    pub server_filter_active: bool,
}

#[derive(Clone, Debug, Default)]
pub struct JumpState {
    pub open: bool,
    pub query: String,
    pub selected: usize,
    pub focus_request: bool,
    /// Tipe data per indeks kolom (diisi saat dialog dibuka).
    pub types: Vec<String>,
}

type InvisibleCacheKey = (usize, usize, usize, usize);

/// Peta indeks kolom → foreign key untuk grid aktif.
pub(crate) type FkByCol = HashMap<usize, ForeignKey>;

/// Kunci cache peta FK: (connection id, database, tabel, hash header + tabel
/// asal tiap kolom).
type FkCacheKey = (i64, String, String, u64);

#[derive(Clone, Debug)]
struct FkCacheEntry {
    key: FkCacheKey,
    checked_at: Instant,
    map: Arc<FkByCol>,
}

#[derive(Clone, Debug)]
pub enum MoveColumnStatus {
    Loading,
    Ready { definition: String },
    Unsupported(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct MoveColumnDialog {
    pub connection_id: i64,
    pub qualified: String,
    pub column: String,
    /// `None` = pindah ke posisi pertama.
    pub after: Option<String>,
    pub columns: Vec<String>,
    pub status: MoveColumnStatus,
    pub generation: u64,
}

// ─── Row as JSON (B10) ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum FkChild {
    Loading,
    Loaded(Box<RowJsonNode>),
    NotFound,
    Failed(String),
}

#[derive(Clone, Debug, Default)]
pub struct RowJsonNode {
    pub table: String,
    pub columns: Vec<String>,
    pub values: Vec<String>,
    /// Tabel asal per kolom (hasil query JOIN); kosong = pakai `table`.
    pub column_tables: Vec<Option<String>>,
    pub children: HashMap<usize, FkChild>,
}

impl RowJsonNode {
    pub fn column_table(&self, col: usize) -> &str {
        self.column_tables
            .get(col)
            .and_then(|t| t.as_deref())
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.table)
    }

    pub fn node_at_mut(&mut self, path: &[usize]) -> Option<&mut RowJsonNode> {
        let mut node = self;
        for col in path {
            node = match node.children.get_mut(col) {
                Some(FkChild::Loaded(child)) => child.as_mut(),
                _ => return None,
            };
        }
        Some(node)
    }

    pub fn node_at(&self, path: &[usize]) -> Option<&RowJsonNode> {
        let mut node = self;
        for col in path {
            node = match node.children.get(col) {
                Some(FkChild::Loaded(child)) => child.as_ref(),
                _ => return None,
            };
        }
        Some(node)
    }

    /// Objek JSON baris; FK yang sudah dimuat ikut sebagai objek bersarang
    /// dengan kunci `"<kolom> (<tabel>)"`.
    pub fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for (i, col) in self.columns.iter().enumerate() {
            let raw = self
                .values
                .get(i)
                .map(String::as_str)
                .unwrap_or(crate::models::structs::NULL_CELL);
            map.insert(col.clone(), json_scalar(raw));
            if let Some(FkChild::Loaded(child)) = self.children.get(&i) {
                map.insert(format!("{} ({})", col, child.table), child.to_json());
            }
        }
        serde_json::Value::Object(map)
    }
}

/// Nilai sel sebagai skalar JSON: NULL → null, angka kanonik → number.
pub fn json_scalar(raw: &str) -> serde_json::Value {
    if gm::is_null_cell(raw) {
        return serde_json::Value::Null;
    }
    let text = gm::display_value(raw);
    let leading_zero = text.len() > 1 && text.starts_with('0') && !text.starts_with("0.");
    let looks_numeric = !(text.is_empty() || leading_zero || text.starts_with('+'));
    if looks_numeric {
        if let Ok(i) = text.parse::<i64>() {
            return serde_json::Value::from(i);
        }
        if let Ok(f) = text.parse::<f64>()
            && f.is_finite()
            && let Some(n) = serde_json::Number::from_f64(f)
        {
            return serde_json::Value::Number(n);
        }
    }
    serde_json::Value::String(text.into_owned())
}

pub fn fk_for<'a>(fks: &'a [ForeignKey], table: &str, column: &str) -> Option<&'a ForeignKey> {
    fks.iter().find(|fk| {
        fk.column_name.eq_ignore_ascii_case(column)
            && (table.is_empty() || fk.table_name.eq_ignore_ascii_case(table))
    })
}

#[derive(Clone, Debug)]
pub struct RowJsonViewer {
    pub open: bool,
    pub generation: u64,
    pub connection_id: Option<i64>,
    pub db_type: Option<DatabaseType>,
    pub database: String,
    pub schema: Option<String>,
    pub fks: Arc<Vec<ForeignKey>>,
    pub root: RowJsonNode,
    pub show_raw_json: bool,
    pub row_label: String,
}

// ─── FK value picker (B11) ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FkPicker {
    pub row: usize,
    pub col: usize,
    pub column: String,
    pub fk: ForeignKey,
    pub qualified: String,
    pub db_type: DatabaseType,
    pub connection_id: i64,
    pub search: String,
    pub last_search: Option<String>,
    pub search_changed_at: Option<std::time::Instant>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub label_cols: Vec<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub generation: u64,
    pub focus_request: bool,
}

// ─── State utama ───────────────────────────────────────────────────────────

#[derive(Default)]
pub struct GridExtState {
    // B1
    pub review: Option<ChangeReview>,
    pub skip_review: bool,
    // B2
    pub undo: Vec<UndoEntry>,
    pub redo: Vec<UndoEntry>,
    /// Nilai mentah (DEFAULT/NOW) sel yang sedang diedit; editor dimulai
    /// kosong dan nilai ini dipertahankan bila user tidak mengetik apa pun.
    pub raw_edit_origin: Option<String>,
    // B3
    pub rewind: Vec<RewindEntry>,
    pub show_rewind: bool,
    pub pending_rewind: Option<PendingRewind>,
    next_rewind_id: u64,
    // B4
    pub find: FindState,
    // B6 / B7
    pub active_key: Option<TableKey>,
    pub saved_filters: Vec<SavedFilter>,
    pub new_filter_name: String,
    pub new_filter_default: bool,
    pub highlight_rules: Vec<gm::HighlightRule>,
    pub show_rules_editor: bool,
    // B8
    pub hide_invisibles: bool,
    invisible_cache: Option<(InvisibleCacheKey, Arc<HashMap<usize, usize>>)>,
    fk_cache: Option<FkCacheEntry>,
    // B9 / B13 (per QueryTab::id)
    pub hidden_columns: HashMap<usize, HashSet<String>>,
    pub column_order: HashMap<usize, Vec<String>>,
    pub jump: JumpState,
    pub dragging_column: Option<usize>,
    pub move_column: Option<MoveColumnDialog>,
    // B10 / B11
    pub row_json: Option<RowJsonViewer>,
    pub fk_picker: Option<FkPicker>,
    next_generation: u64,
}

impl GridExtState {
    pub fn next_generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// Tutup overlay grid teratas (untuk Esc). `true` bila ada yang ditutup.
    pub fn close_topmost_overlay(&mut self) -> bool {
        if self.review.is_some() {
            self.review = None;
        } else if self.fk_picker.is_some() {
            self.fk_picker = None;
        } else if self.move_column.is_some() {
            self.move_column = None;
        } else if self.jump.open {
            self.jump.open = false;
        } else if self.row_json.as_ref().is_some_and(|v| v.open) {
            self.row_json = None;
        } else if self.show_rules_editor {
            self.show_rules_editor = false;
        } else if self.show_rewind {
            self.show_rewind = false;
        } else if self.find.open && !self.find.server_filter_active {
            self.find.open = false;
            self.find.matches.clear();
            self.find.match_set = Arc::default();
            self.find.cache_key = None;
        } else {
            return false;
        }
        true
    }
}

/// Aksi yang dikumpulkan selama render grid lalu dijalankan setelahnya.
#[derive(Default)]
pub struct GridRequests {
    pub filter_by: Option<(String, FilterOperator, String)>,
    pub set_value: Option<(usize, usize, String)>,
    pub open_row_json: Option<usize>,
    pub open_fk_picker: Option<(usize, usize, ForeignKey)>,
    pub hide_column: Option<String>,
    pub show_all_columns: bool,
    pub toggle_column: Option<String>,
    pub reorder: Option<(String, String, bool)>,
    pub reset_order: bool,
    pub move_column_physical: Option<String>,
    pub toggle_delete_rows: Option<Vec<usize>>,
    /// Baris yang diduplikasi (disisipkan tepat di bawahnya).
    pub duplicate_row: Option<usize>,
    pub undo: bool,
    pub redo: bool,
    pub open_find: bool,
    pub open_jump: bool,
    pub open_rules_editor: bool,
    pub rule_from_cell: Option<(String, String)>,
    pub review_save: bool,
    pub discard: bool,
}

// ─── Helper umum ───────────────────────────────────────────────────────────

pub(crate) fn active_tab_id(t: &Tabular) -> Option<usize> {
    t.query_tabs.get(t.active_tab_index).map(|tab| tab.id)
}

pub(crate) fn hidden_columns(t: &Tabular) -> HashSet<String> {
    active_tab_id(t)
        .and_then(|id| t.grid_ext.hidden_columns.get(&id).cloned())
        .unwrap_or_default()
}

pub(crate) fn column_order(t: &Tabular) -> Option<Vec<String>> {
    active_tab_id(t).and_then(|id| t.grid_ext.column_order.get(&id).cloned())
}

pub(crate) fn connection_db_type(t: &Tabular, connection_id: i64) -> Option<DatabaseType> {
    t.connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .map(|c| c.connection_type.clone())
}

/// Nama database aktif: dari tab, lalu dari konfigurasi koneksi.
pub(crate) fn active_database_name(t: &Tabular) -> String {
    let cid = t.current_connection_id;
    t.query_tabs
        .get(t.active_tab_index)
        .and_then(|x| x.database_name.clone())
        .filter(|d| !d.is_empty())
        .or_else(|| {
            cid.and_then(|cid| t.connections.iter().find(|c| c.id == Some(cid)))
                .map(|c| c.database.clone())
        })
        .unwrap_or_default()
}

fn active_schema_name(t: &Tabular) -> Option<String> {
    t.query_tabs
        .get(t.active_tab_index)
        .and_then(|x| x.schema_name.clone())
        .filter(|s| !s.is_empty())
}

/// Jalankan future SQLite lokal secara blocking (pola `cache_data`).
fn block_on<F: std::future::Future>(t: &Tabular, fut: F) -> Option<F::Output> {
    match t.runtime.clone() {
        Some(rt) => Some(rt.block_on(fut)),
        None => tokio::runtime::Runtime::new()
            .ok()
            .map(|rt| rt.block_on(fut)),
    }
}

/// Query lookup kecil (FK picker, Row JSON, DDL) tanpa mengganti panel hasil
/// dengan spinner eksekusi.
pub(crate) fn run_lookup_query(
    t: &mut Tabular,
    connection_id: i64,
    sql: String,
    on_result: impl FnOnce(&mut Tabular, &crate::connection::QueryResultMessage) + 'static,
) {
    let was_running = t.query_execution_in_progress;
    t.run_query_with_callback(connection_id, sql, on_result);
    if !was_running && t.jobs.deferred_callbacks.is_empty() {
        t.query_execution_in_progress = false;
    }
}

fn refresh_after_external_change(t: &mut Tabular) {
    if !t.is_table_browse_mode {
        return;
    }
    if t.use_server_pagination && !t.current_base_query.is_empty() {
        t.execute_paginated_query();
    } else {
        super::refresh_current_table_data(t);
    }
}

// ─── Operasi data + undo (B1, B2) ──────────────────────────────────────────

impl Tabular {
    /// Indeks `all_table_data` untuk baris tampil `row`. Pada paginasi sisi
    /// klien `current_table_data` hanya potongan halaman aktif.
    /// Tolak aksi yang menyusun ulang atau mengganti baris grid selagi ada edit
    /// yang belum disimpan. Indeks baris di antrean edit mengacu ke urutan
    /// tampilan saat ini; sort, ganti halaman, atau filter akan menggeser
    /// penanda sel dan membuat review menunjuk baris lain. Mengembalikan true
    /// bila aksi harus dibatalkan.
    pub(crate) fn grid_refuse_while_dirty(&mut self, action: &str) -> bool {
        if self.spreadsheet_state.pending_operations.is_empty() {
            return false;
        }
        self.toasts.warning(format!(
            "Save or discard the pending grid changes before {}.",
            action
        ));
        true
    }

    pub(crate) fn grid_all_index(&self, row: usize) -> usize {
        if !self.use_server_pagination && self.all_table_data.len() > self.current_table_data.len()
        {
            row + self.current_page * self.page_size
        } else {
            row
        }
    }

    pub(crate) fn grid_cell(&self, row: usize, col: usize) -> Option<String> {
        self.current_table_data
            .get(row)
            .and_then(|r| r.get(col))
            .or_else(|| {
                self.all_table_data
                    .get(self.grid_all_index(row))
                    .and_then(|r| r.get(col))
            })
            .cloned()
    }

    pub(crate) fn grid_set_cell_raw(&mut self, row: usize, col: usize, value: &str) {
        let abs = self.grid_all_index(row);
        if let Some(cell) = self
            .current_table_data
            .get_mut(row)
            .and_then(|r| r.get_mut(col))
        {
            *cell = value.to_string();
        }
        if let Some(cell) = self
            .all_table_data
            .get_mut(abs)
            .and_then(|r| r.get_mut(col))
        {
            *cell = value.to_string();
        }
    }

    pub(crate) fn grid_insert_row_raw(&mut self, row: usize, values: Vec<String>) {
        let abs = self.grid_all_index(row);
        let at = row.min(self.current_table_data.len());
        self.current_table_data.insert(at, values.clone());
        if abs <= self.all_table_data.len() {
            self.all_table_data.insert(abs, values);
        } else {
            self.all_table_data.push(values);
        }
        self.total_rows = self.total_rows.saturating_add(1);
    }

    pub(crate) fn grid_remove_row_raw(&mut self, row: usize) -> Option<Vec<String>> {
        let abs = self.grid_all_index(row);
        let from_current =
            (row < self.current_table_data.len()).then(|| self.current_table_data.remove(row));
        let from_all = (abs < self.all_table_data.len()).then(|| self.all_table_data.remove(abs));
        if from_current.is_some() || from_all.is_some() {
            self.total_rows = self.total_rows.saturating_sub(1);
        }
        from_current.or(from_all)
    }

    pub(crate) fn grid_apply_change(&mut self, change: &DataChange, forward: bool) {
        match (change, forward) {
            (
                DataChange::Cell {
                    row, col, after, ..
                },
                true,
            ) => self.grid_set_cell_raw(*row, *col, after),
            (
                DataChange::Cell {
                    row, col, before, ..
                },
                false,
            ) => self.grid_set_cell_raw(*row, *col, before),
            (DataChange::InsertRow { row, values }, true)
            | (DataChange::RemoveRow { row, values }, false) => {
                self.grid_insert_row_raw(*row, values.clone())
            }
            (DataChange::InsertRow { row, .. }, false)
            | (DataChange::RemoveRow { row, .. }, true) => {
                self.grid_remove_row_raw(*row);
            }
        }
    }

    /// Catat satu langkah undo. `ops_before` adalah snapshot antrean sebelum
    /// perubahan; antrean sesudahnya diambil dari state saat ini.
    pub(crate) fn grid_record(
        &mut self,
        label: &str,
        ops_before: Vec<CellEditOperation>,
        changes: Vec<DataChange>,
    ) {
        let ops_after = self.spreadsheet_state.pending_operations.clone();
        if changes.is_empty() && ops_after == ops_before {
            return;
        }
        let history = &mut self.grid_ext;
        history.undo.push(UndoEntry {
            label: label.to_string(),
            ops_before,
            ops_after,
            changes,
        });
        if history.undo.len() > UNDO_LIMIT {
            history.undo.remove(0);
        }
        history.redo.clear();
    }

    fn grid_restore_ops(&mut self, ops: Vec<CellEditOperation>) {
        self.spreadsheet_state.is_dirty = !ops.is_empty();
        self.spreadsheet_state.pending_operations = ops;
    }

    pub(crate) fn grid_undo(&mut self) -> Option<String> {
        if self.spreadsheet_state.editing_cell.is_some() {
            self.spreadsheet_finish_cell_edit(true);
        }
        let entry = self.grid_ext.undo.pop()?;
        for change in entry.changes.iter().rev() {
            self.grid_apply_change(change, false);
        }
        self.grid_restore_ops(entry.ops_before.clone());
        let label = entry.label.clone();
        self.grid_ext.redo.push(entry);
        Some(label)
    }

    pub(crate) fn grid_redo(&mut self) -> Option<String> {
        if self.spreadsheet_state.editing_cell.is_some() {
            self.spreadsheet_finish_cell_edit(true);
        }
        let entry = self.grid_ext.redo.pop()?;
        for change in &entry.changes {
            self.grid_apply_change(change, true);
        }
        self.grid_restore_ops(entry.ops_after.clone());
        let label = entry.label.clone();
        self.grid_ext.undo.push(entry);
        Some(label)
    }

    pub(crate) fn grid_clear_history(&mut self) {
        self.grid_ext.undo.clear();
        self.grid_ext.redo.clear();
        self.grid_ext.raw_edit_origin = None;
    }

    /// Set nilai sel lewat jalur edit biasa (tercatat di antrean & undo).
    pub(crate) fn grid_set_cell_value(&mut self, row: usize, col: usize, value: String) {
        if self.spreadsheet_state.editing_cell.is_some() {
            self.spreadsheet_finish_cell_edit(true);
        }
        if gm::pending_deleted_rows(&self.spreadsheet_state.pending_operations).contains(&row) {
            self.toasts
                .warning("Row is marked for deletion; restore it before editing.");
            return;
        }
        // SQLite tidak menerima `SET kolom = DEFAULT` pada baris yang sudah ada.
        let is_new_row =
            gm::pending_inserted_rows(&self.spreadsheet_state.pending_operations).contains(&row);
        if gm::is_raw_default(&value)
            && !is_new_row
            && self
                .current_connection_id
                .and_then(|cid| connection_db_type(self, cid))
                .is_some_and(|db| !gm::default_allowed_in_update(&db))
        {
            self.toasts
                .warning("SQLite cannot set an existing row's column to DEFAULT");
            return;
        }
        self.spreadsheet_start_cell_edit(row, col);
        if self.spreadsheet_state.editing_cell != Some((row, col)) {
            return;
        }
        self.grid_ext.raw_edit_origin = None;
        self.spreadsheet_state.cell_edit_text = value;
        self.spreadsheet_finish_cell_edit(true);
    }

    /// Tandai/lepas tanda hapus pada baris. Baris baru (belum tersimpan)
    /// langsung dibuang dari grid.
    pub(crate) fn grid_toggle_delete_rows(&mut self, mut rows: Vec<usize>) {
        if self.spreadsheet_state.editing_cell.is_some() {
            self.spreadsheet_finish_cell_edit(true);
        }
        rows.sort_unstable();
        rows.dedup();
        let ops_before = self.spreadsheet_state.pending_operations.clone();
        let mut changes = Vec::new();
        let mut removed_any = false;
        let mut marked = 0usize;
        let mut restored = 0usize;
        for row in rows.into_iter().rev() {
            let ops = &self.spreadsheet_state.pending_operations;
            if gm::pending_inserted_rows(ops).contains(&row) {
                self.spreadsheet_state
                    .pending_operations
                    .retain(|op| gm::op_row(op) != row);
                if let Some(values) = self.grid_remove_row_raw(row) {
                    changes.push(DataChange::RemoveRow { row, values });
                }
                gm::shift_op_rows(&mut self.spreadsheet_state.pending_operations, row + 1, -1);
                removed_any = true;
            } else if gm::pending_deleted_rows(ops).contains(&row) {
                self.spreadsheet_state.pending_operations.retain(
                    |op| !matches!(op, CellEditOperation::DeleteRow { row_index, .. } if *row_index == row),
                );
                restored += 1;
            } else if let Some(values) = self
                .current_table_data
                .get(row)
                .or_else(|| self.all_table_data.get(row))
                .cloned()
            {
                self.spreadsheet_state
                    .pending_operations
                    .push(CellEditOperation::DeleteRow {
                        row_index: row,
                        values,
                    });
                marked += 1;
            }
        }
        self.spreadsheet_state.is_dirty = !self.spreadsheet_state.pending_operations.is_empty();
        if removed_any {
            self.selected_row = None;
            self.selected_cell = None;
            self.selected_rows.clear();
        }
        let label = if marked > 0 {
            "Delete row(s)"
        } else if restored > 0 {
            "Restore row(s)"
        } else {
            "Remove new row(s)"
        };
        self.grid_record(label, ops_before, changes);
    }

    /// Setelah simpan sukses: buang baris yang dihapus dari grid.
    pub(crate) fn grid_drop_saved_deleted_rows(&mut self, saved: &[CellEditOperation]) {
        let mut rows: Vec<usize> = gm::pending_deleted_rows(saved).into_iter().collect();
        rows.sort_unstable_by(|a, b| b.cmp(a));
        for row in rows {
            self.grid_remove_row_raw(row);
            gm::shift_op_rows(&mut self.spreadsheet_state.pending_operations, row + 1, -1);
        }
    }

    pub(crate) fn grid_push_rewind(&mut self, connection_id: i64, pending: PendingRewind) {
        let grid = &mut self.grid_ext;
        grid.next_rewind_id += 1;
        grid.rewind.insert(
            0,
            RewindEntry {
                id: grid.next_rewind_id,
                committed_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                connection_id,
                table: pending.table,
                summary: pending.summary,
                forward_sql: pending.forward_sql,
                rewind_sql: pending.sql,
                notes: pending.notes,
                restored: false,
            },
        );
        grid.rewind.truncate(REWIND_LIMIT);
    }
}

/// Batalkan semua perubahan pending: kembalikan data lewat undo stack, lalu
/// muat ulang tabel bila sedang browse.
pub(crate) fn discard_pending_changes(t: &mut Tabular) {
    if t.spreadsheet_state.editing_cell.is_some() {
        t.spreadsheet_finish_cell_edit(false);
    }
    while let Some(entry) = t.grid_ext.undo.pop() {
        for change in entry.changes.iter().rev() {
            t.grid_apply_change(change, false);
        }
    }
    t.reset_spreadsheet_state();
    refresh_after_external_change(t);
}

// ─── Simpan dengan review (B1) ─────────────────────────────────────────────

/// ⌘S / tombol Save: siapkan SQL lalu tampilkan dialog review (atau langsung
/// simpan bila user memilih melewati review).
pub(crate) fn request_save_review(t: &mut Tabular) {
    if t.spreadsheet_state.editing_cell.is_some() {
        t.spreadsheet_finish_cell_edit(true);
    }
    if t.spreadsheet_state.pending_operations.is_empty() {
        t.toasts.info("No pending changes to save");
        return;
    }
    let Some(connection_id) = t.current_connection_id else {
        t.toasts.error("No active connection for this result");
        return;
    };
    t.spreadsheet_ensure_primary_keys();
    let Some(sql) = t.spreadsheet_generate_sql() else {
        t.toasts.error(
            "Could not build SQL for the pending changes (table name or key columns unknown).",
        );
        return;
    };
    if t.grid_ext.skip_review {
        commit_pending_changes(t);
        return;
    }
    let summary = gm::summarize_ops(&t.spreadsheet_state.pending_operations);
    let mut warnings = Vec::new();
    let has_meta_pk = t
        .current_column_metadata
        .as_ref()
        .is_some_and(|m| m.iter().any(|c| c.is_primary_key));
    if t.spreadsheet_state.primary_key_columns.is_empty() && !has_meta_pk {
        warnings.push(
            "No primary key detected: rows are matched on all column values, which can affect duplicate rows."
                .to_string(),
        );
    }
    if t.query_tabs
        .get(t.active_tab_index)
        .is_some_and(|tab| tab.tx_mode)
    {
        warnings.push(
            "Manual commit is on for this tab, but grid changes are saved on a separate connection and committed immediately (outside the open transaction)."
                .to_string(),
        );
    }
    t.grid_ext.review = Some(ChangeReview {
        kind: ReviewKind::SaveChanges,
        title: "Review changes".to_string(),
        summary: summary.describe(),
        sql: format!("{};", sql),
        warnings,
        connection_id,
    });
}

/// Simpan antrean edit ke database dan siapkan entri rewind.
pub(crate) fn commit_pending_changes(t: &mut Tabular) {
    t.spreadsheet_ensure_primary_keys();
    let forward_sql = t.spreadsheet_generate_sql().unwrap_or_default();
    let mut pending = t.spreadsheet_generate_rewind();
    pending.forward_sql = forward_sql;
    t.grid_ext.pending_rewind = Some(pending);
    t.spreadsheet_save_changes();
    // Bila eksekusi tidak jadi dimulai, rewind yang disiapkan tidak berlaku.
    t.grid_ext.pending_rewind = None;
}

pub(crate) fn confirm_review(t: &mut Tabular) {
    let Some(review) = t.grid_ext.review.take() else {
        return;
    };
    match review.kind {
        ReviewKind::SaveChanges => commit_pending_changes(t),
        ReviewKind::Rewind { entry_id } => {
            t.run_query_with_callback(review.connection_id, review.sql, move |t, msg| {
                if !msg.success {
                    t.toasts.error(format!(
                        "Restore failed: {}",
                        msg.error.clone().unwrap_or_default()
                    ));
                    return;
                }
                if let Some(entry) = t.grid_ext.rewind.iter_mut().find(|e| e.id == entry_id) {
                    entry.restored = true;
                }
                t.toasts.success("Previous values restored");
                if t.is_table_browse_mode {
                    refresh_after_external_change(t);
                } else {
                    t.toasts.info("Re-run the query to see the restored values");
                }
            });
        }
        ReviewKind::Ddl => {
            t.run_query_with_callback(review.connection_id, review.sql, |t, msg| {
                if !msg.success {
                    t.toasts.error(format!(
                        "Statement failed: {}",
                        msg.error.clone().unwrap_or_default()
                    ));
                    return;
                }
                t.toasts.success("Table altered");
                let key = t.grid_ext.active_key.clone();
                if let Some(key) = key {
                    t.cache_miss_request = Some((key.connection_id, key.database, key.table));
                }
                refresh_after_external_change(t);
            });
        }
    }
}

pub(crate) fn open_rewind_review(t: &mut Tabular, entry_id: u64) {
    let Some(entry) = t.grid_ext.rewind.iter().find(|e| e.id == entry_id).cloned() else {
        return;
    };
    let Some(sql) = entry.rewind_sql.clone() else {
        t.toasts.warning("This commit has no reversible statements");
        return;
    };
    let mut warnings = entry.notes.clone();
    warnings.push(
        "Rows changed by others since this commit will be overwritten with the values from before the commit."
            .to_string(),
    );
    t.grid_ext.review = Some(ChangeReview {
        kind: ReviewKind::Rewind { entry_id },
        title: "Restore previous values".to_string(),
        summary: format!(
            "Undo commit on {} ({})",
            entry.table,
            entry.summary.describe()
        ),
        sql: format!("{};", sql),
        warnings,
        connection_id: entry.connection_id,
    });
}

// ─── Konteks tabel: saved filter & highlight rule (B6, B7) ─────────────────

pub(crate) fn current_table_key(t: &mut Tabular) -> Option<TableKey> {
    if !t.is_table_browse_mode {
        return None;
    }
    let connection_id = t.current_connection_id?;
    let table = super::infer_current_table_name(t);
    if table.is_empty() {
        return None;
    }
    Some(TableKey {
        connection_id,
        database: active_database_name(t),
        table,
    })
}

/// Dipanggil tiap frame dari render grid; memuat preferensi tabel ketika
/// tabel yang tampil berganti dan menerapkan saved filter default.
pub(crate) fn sync_table_context(t: &mut Tabular) {
    if t.query_execution_in_progress {
        return;
    }
    let key = current_table_key(t);
    if key == t.grid_ext.active_key {
        return;
    }
    t.grid_ext.active_key = key.clone();
    t.grid_ext.saved_filters.clear();
    t.grid_ext.highlight_rules.clear();
    t.grid_ext.find.server_filter_active = false;
    let (Some(key), Some(pool)) = (key, t.db_pool.clone()) else {
        return;
    };
    let loaded = block_on(t, async {
        (
            grid_prefs::load_saved_filters(&pool, &key).await,
            grid_prefs::load_highlight_rules(&pool, &key).await,
        )
    });
    let Some((filters, rules)) = loaded else {
        return;
    };
    match rules {
        Ok(rules) => t.grid_ext.highlight_rules = rules,
        Err(e) => log::warn!("[GRID] gagal memuat highlight rules: {}", e),
    }
    match filters {
        Ok(filters) => {
            let default = filters.iter().find(|f| f.is_default).cloned();
            t.grid_ext.saved_filters = filters;
            if let Some(filter) = default
                && t.sql_filter_text.trim().is_empty()
                && t.visual_filter.conditions.is_empty()
            {
                t.toasts
                    .info(format!("Applied default filter \"{}\"", filter.name));
                apply_saved_filter(t, &filter.payload);
            }
        }
        Err(e) => log::warn!("[GRID] gagal memuat saved filters: {}", e),
    }
}

pub(crate) fn apply_saved_filter(t: &mut Tabular, payload: &SavedFilterPayload) {
    t.visual_filter.conditions = payload.conditions.clone();
    t.visual_filter.match_all = payload.match_all;
    t.visual_filter.group = payload.group;
    t.sql_filter_text = if payload.where_text.trim().is_empty() {
        let db_type = t
            .current_connection_id
            .and_then(|cid| connection_db_type(t, cid));
        super::build_where_from_visual_filter(&t.visual_filter, db_type.as_ref())
    } else {
        payload.where_text.clone()
    };
    super::apply_sql_filter(t);
}

fn reload_saved_filters(t: &mut Tabular) {
    let (Some(key), Some(pool)) = (t.grid_ext.active_key.clone(), t.db_pool.clone()) else {
        return;
    };
    if let Some(Ok(filters)) = block_on(t, grid_prefs::load_saved_filters(&pool, &key)) {
        t.grid_ext.saved_filters = filters;
    }
}

pub(crate) fn save_current_filter(t: &mut Tabular, name: &str, as_default: bool) {
    let name = name.trim();
    if name.is_empty() {
        t.toasts.warning("Enter a name for the filter");
        return;
    }
    let (Some(key), Some(pool)) = (t.grid_ext.active_key.clone(), t.db_pool.clone()) else {
        t.toasts
            .warning("Saved filters are available when browsing a table");
        return;
    };
    let payload = SavedFilterPayload {
        conditions: t.visual_filter.conditions.clone(),
        match_all: t.visual_filter.match_all,
        group: t.visual_filter.group,
        where_text: t.sql_filter_text.clone(),
    };
    let json = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    let result = block_on(
        t,
        grid_prefs::upsert_entry(
            &pool,
            &key,
            grid_prefs::KIND_FILTER,
            name,
            &json,
            as_default,
        ),
    );
    match result {
        Some(Ok(())) => {
            t.toasts.success(format!("Filter \"{}\" saved", name));
            reload_saved_filters(t);
        }
        Some(Err(e)) => t.toasts.error(format!("Could not save filter: {}", e)),
        None => {}
    }
}

pub(crate) fn remove_saved_filter(t: &mut Tabular, name: &str) {
    let (Some(key), Some(pool)) = (t.grid_ext.active_key.clone(), t.db_pool.clone()) else {
        return;
    };
    if let Some(Err(e)) = block_on(
        t,
        grid_prefs::remove_entry(&pool, &key, grid_prefs::KIND_FILTER, name),
    ) {
        t.toasts.error(format!("Could not remove filter: {}", e));
    }
    reload_saved_filters(t);
}

pub(crate) fn set_filter_default(t: &mut Tabular, name: &str, is_default: bool) {
    let Some(filter) = t
        .grid_ext
        .saved_filters
        .iter()
        .find(|f| f.name == name)
        .cloned()
    else {
        return;
    };
    let (Some(key), Some(pool)) = (t.grid_ext.active_key.clone(), t.db_pool.clone()) else {
        return;
    };
    let json = serde_json::to_string(&filter.payload).unwrap_or_else(|_| "{}".to_string());
    if let Some(Err(e)) = block_on(
        t,
        grid_prefs::upsert_entry(
            &pool,
            &key,
            grid_prefs::KIND_FILTER,
            name,
            &json,
            is_default,
        ),
    ) {
        t.toasts.error(format!("Could not update filter: {}", e));
    }
    reload_saved_filters(t);
}

pub(crate) fn persist_highlight_rules(t: &mut Tabular) {
    let (Some(key), Some(pool)) = (t.grid_ext.active_key.clone(), t.db_pool.clone()) else {
        return;
    };
    let rules = t.grid_ext.highlight_rules.clone();
    if let Some(Err(e)) = block_on(t, grid_prefs::save_highlight_rules(&pool, &key, &rules)) {
        t.toasts
            .error(format!("Could not save highlight rules: {}", e));
    }
}

// ─── Find in results (B4) ──────────────────────────────────────────────────

pub(crate) fn refresh_find_matches(t: &mut Tabular) {
    if !t.grid_ext.find.open || t.grid_ext.find.query.is_empty() {
        if !t.grid_ext.find.matches.is_empty() {
            t.grid_ext.find.matches.clear();
            t.grid_ext.find.match_set = Arc::default();
        }
        t.grid_ext.find.cache_key = None;
        return;
    }
    let hidden = hidden_columns(t);
    let key: FindCacheKey = (
        t.grid_ext.find.query.clone(),
        t.grid_ext.find.case_sensitive,
        t.current_table_data.as_ptr() as usize,
        t.current_table_data.len(),
        hidden.len(),
        t.spreadsheet_state.pending_operations.len() + t.grid_ext.undo.len() * 7,
    );
    if t.grid_ext.find.cache_key.as_ref() == Some(&key) {
        return;
    }
    let headers = &t.current_table_headers;
    let matches = gm::find_matches(
        &t.current_table_data,
        &t.grid_ext.find.query,
        t.grid_ext.find.case_sensitive,
        |ci| headers.get(ci).is_some_and(|h| hidden.contains(h)),
    );
    let find = &mut t.grid_ext.find;
    find.current = find.current.min(matches.len().saturating_sub(1));
    find.match_set = Arc::new(matches.iter().copied().collect());
    find.matches = matches;
    find.cache_key = Some(key);
}

/// Pindah ke hasil berikutnya (`delta` = 1) / sebelumnya (-1).
pub(crate) fn find_step(t: &mut Tabular, delta: isize) {
    let len = t.grid_ext.find.matches.len();
    if len == 0 {
        return;
    }
    let cur = t.grid_ext.find.current as isize;
    let next = (cur + delta).rem_euclid(len as isize) as usize;
    t.grid_ext.find.current = next;
    select_find_match(t);
}

pub(crate) fn select_find_match(t: &mut Tabular) {
    if let Some(&(r, c)) = t.grid_ext.find.matches.get(t.grid_ext.find.current) {
        t.selected_row = Some(r);
        t.selected_cell = Some((r, c));
        t.table_sel_anchor = None;
        t.scroll_to_selected_cell = true;
    }
}

pub(crate) fn search_all_rows_on_server(t: &mut Tabular) {
    let Some(cid) = t.current_connection_id else {
        return;
    };
    let Some(db_type) = connection_db_type(t, cid) else {
        return;
    };
    let needle = t.grid_ext.find.query.clone();
    let Some(where_sql) = gm::build_search_all_where(&t.current_table_headers, &needle, &db_type)
    else {
        t.toasts.info("Type something to search for");
        return;
    };
    t.visual_filter.conditions.clear();
    t.sql_filter_text = where_sql;
    t.grid_ext.find.server_filter_active = true;
    super::apply_sql_filter(t);
}

pub(crate) fn clear_server_search(t: &mut Tabular) {
    t.grid_ext.find.server_filter_active = false;
    t.sql_filter_text.clear();
    super::apply_sql_filter(t);
}

// ─── Karakter tak terlihat (B8) ────────────────────────────────────────────

pub(crate) fn invisible_columns(t: &mut Tabular) -> Arc<HashMap<usize, usize>> {
    let key: InvisibleCacheKey = (
        t.current_table_data.as_ptr() as usize,
        t.current_table_data.len(),
        t.current_table_headers.len(),
        t.spreadsheet_state.pending_operations.len() + t.grid_ext.undo.len() * 7,
    );
    if let Some((cached_key, cols)) = &t.grid_ext.invisible_cache
        && *cached_key == key
    {
        return Arc::clone(cols);
    }
    let cols = Arc::new(gm::columns_with_suspicious_invisibles(
        &t.current_table_data,
    ));
    t.grid_ext.invisible_cache = Some((key, Arc::clone(&cols)));
    cols
}

// ─── Peta foreign key per kolom ────────────────────────────────────────────

/// Petakan tiap header ke FK-nya. `column_table(i)` mengembalikan tabel asal
/// kolom ke-`i` bila metadata hasil query menyediakannya; kalau tidak,
/// `fallback_table` dipakai (kosong = cocokkan nama kolom saja).
pub(crate) fn map_foreign_keys_to_columns<'a>(
    headers: &[String],
    column_table: impl Fn(usize) -> Option<&'a str>,
    fallback_table: &str,
    fks: &[ForeignKey],
) -> FkByCol {
    let mut out = FkByCol::new();
    if fks.is_empty() {
        return out;
    }
    for (i, h) in headers.iter().enumerate() {
        let table_hint = column_table(i)
            .filter(|t| !t.is_empty())
            .unwrap_or(fallback_table);
        if let Some(fk) = fks.iter().find(|fk| {
            (fk.table_name.eq_ignore_ascii_case(table_hint) || table_hint.is_empty())
                && fk.column_name.eq_ignore_ascii_case(h)
        }) {
            out.insert(i, fk.clone());
        }
    }
    out
}

/// Peta FK per kolom untuk grid aktif. Hasilnya di-cache per (koneksi,
/// database, tabel, header) sehingga cache SQLite tidak dibaca setiap frame;
/// entri dicek ulang setelah [`FK_CACHE_TTL`] karena cache FK diisi task latar.
pub(crate) fn fk_by_column(
    t: &mut Tabular,
    headers: &[String],
    database: &str,
    table: &str,
) -> Arc<FkByCol> {
    use std::hash::{Hash, Hasher};
    let Some(cid) = t.current_connection_id else {
        t.grid_ext.fk_cache = None;
        return Arc::default();
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    headers.hash(&mut hasher);
    if let Some(meta) = &t.current_column_metadata {
        for m in meta {
            m.table_name.hash(&mut hasher);
        }
    }
    let key: FkCacheKey = (
        cid,
        database.to_string(),
        table.to_string(),
        hasher.finish(),
    );
    if let Some(entry) = &t.grid_ext.fk_cache
        && entry.key == key
        && entry.checked_at.elapsed() < FK_CACHE_TTL
    {
        return Arc::clone(&entry.map);
    }
    let fks =
        crate::cache_data::get_foreign_keys_from_cache_shared(t, cid, database).unwrap_or_default();
    let meta = t.current_column_metadata.as_deref();
    let map = Arc::new(map_foreign_keys_to_columns(
        headers,
        |i| {
            meta.and_then(|m| m.get(i))
                .and_then(|c| c.table_name.as_deref())
        },
        table,
        &fks,
    ));
    t.grid_ext.fk_cache = Some(FkCacheEntry {
        key,
        checked_at: Instant::now(),
        map: Arc::clone(&map),
    });
    map
}

/// Buang peta FK grid (mis. setelah refresh koneksi) agar dibaca ulang,
/// beserta memo proses `cache_data` milik koneksi itu.
pub(crate) fn invalidate_fk_cache(t: &mut Tabular, connection_id: i64) {
    t.grid_ext.fk_cache = None;
    crate::cache_data::invalidate_foreign_key_memo(connection_id);
}

// ─── Kolom: sembunyi, urutan, jump (B9, B13) ───────────────────────────────

pub(crate) fn set_column_hidden(t: &mut Tabular, column: &str, hidden: bool) {
    let Some(id) = active_tab_id(t) else {
        return;
    };
    let visible_count = t.current_table_headers.len()
        - t.grid_ext
            .hidden_columns
            .get(&id)
            .map(|s| s.len())
            .unwrap_or(0);
    let set = t.grid_ext.hidden_columns.entry(id).or_default();
    if hidden {
        if visible_count <= 1 {
            t.toasts.warning("At least one column must stay visible");
            return;
        }
        set.insert(column.to_string());
        t.pinned_columns.remove(column);
    } else {
        set.remove(column);
    }
    t.grid_ext.find.cache_key = None;
}

pub(crate) fn show_all_columns(t: &mut Tabular) {
    if let Some(id) = active_tab_id(t) {
        t.grid_ext.hidden_columns.remove(&id);
        t.grid_ext.find.cache_key = None;
    }
}

pub(crate) fn reorder_column(t: &mut Tabular, moving: &str, target: &str, after: bool) {
    let Some(id) = active_tab_id(t) else {
        return;
    };
    let headers = t.current_table_headers.clone();
    let order = t.grid_ext.column_order.entry(id).or_default();
    gm::move_column(order, &headers, moving, target, after);
}

pub(crate) fn reset_column_order(t: &mut Tabular) {
    if let Some(id) = active_tab_id(t) {
        t.grid_ext.column_order.remove(&id);
    }
}

/// Tipe data tiap kolom hasil: dari metadata query, atau dari cache kolom
/// tabel yang sedang di-browse.
pub(crate) fn column_types(t: &Tabular) -> Vec<String> {
    let headers = &t.current_table_headers;
    if let Some(meta) = &t.current_column_metadata
        && meta.len() == headers.len()
    {
        return meta.iter().map(|m| m.type_name.clone()).collect();
    }
    let cached = t.grid_ext.active_key.as_ref().and_then(|key| {
        crate::cache_data::get_columns_from_cache(t, key.connection_id, &key.database, &key.table)
    });
    match cached {
        Some(cols) => headers
            .iter()
            .map(|h| {
                cols.iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case(h))
                    .map(|(_, ty)| ty.clone())
                    .unwrap_or_default()
            })
            .collect(),
        None => vec![String::new(); headers.len()],
    }
}

/// Indeks kolom hasil fuzzy search untuk dialog Jump to Column.
pub(crate) fn jump_candidates(t: &Tabular) -> Vec<usize> {
    let query = t.grid_ext.jump.query.trim();
    let mut scored: Vec<(usize, i32)> = t
        .current_table_headers
        .iter()
        .enumerate()
        .filter_map(|(i, h)| gm::fuzzy_score(query, h).map(|s| (i, s)))
        .collect();
    if !query.is_empty() {
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    }
    scored.into_iter().map(|(i, _)| i).collect()
}

pub(crate) fn jump_to_column(t: &mut Tabular, col: usize) {
    let Some(name) = t.current_table_headers.get(col).cloned() else {
        return;
    };
    set_column_hidden(t, &name, false);
    let row = t
        .selected_cell
        .map(|(r, _)| r)
        .or(t.selected_row)
        .unwrap_or(0)
        .min(t.current_table_data.len().saturating_sub(1));
    t.selected_row = Some(row);
    t.selected_cell = Some((row, col));
    t.table_sel_anchor = None;
    t.scroll_to_selected_cell = true;
    t.table_recently_clicked = true;
    t.grid_ext.jump.open = false;
}

/// Buka dialog pindah kolom fisik (MySQL: `MODIFY COLUMN ... AFTER`).
pub(crate) fn open_move_column_dialog(t: &mut Tabular, column: String) {
    let Some(key) = t.grid_ext.active_key.clone() else {
        t.toasts
            .info("Open the table from the sidebar to change its column order");
        return;
    };
    let db_type = connection_db_type(t, key.connection_id);
    let generation = t.grid_ext.next_generation();
    let columns = t.current_table_headers.clone();
    let db = db_type.clone().unwrap_or(DatabaseType::MySQL);
    let qualified = gm::qualified_table(
        &db,
        Some(&key.database),
        active_schema_name(t).as_deref(),
        &key.table,
    );
    let status = if db_type == Some(DatabaseType::MySQL) {
        MoveColumnStatus::Loading
    } else {
        MoveColumnStatus::Unsupported(
            "Moving a column inside an existing table is only supported on MySQL/MariaDB (ALTER TABLE … MODIFY COLUMN … AFTER). PostgreSQL, SQLite and SQL Server need a full table rebuild, which Tabular does not perform automatically. Drag the column header to reorder this grid view instead."
                .to_string(),
        )
    };
    let fetch = matches!(status, MoveColumnStatus::Loading);
    let prev = columns
        .iter()
        .position(|c| c == &column)
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| columns.get(i).cloned());
    t.grid_ext.move_column = Some(MoveColumnDialog {
        connection_id: key.connection_id,
        qualified: qualified.clone(),
        column: column.clone(),
        after: prev,
        columns,
        status,
        generation,
    });
    if !fetch {
        return;
    }
    run_lookup_query(
        t,
        key.connection_id,
        format!("SHOW CREATE TABLE {}", qualified),
        move |t, msg| {
            let Some(dialog) = t.grid_ext.move_column.as_mut() else {
                return;
            };
            if dialog.generation != generation {
                return;
            }
            dialog.status = if !msg.success {
                MoveColumnStatus::Failed(msg.error.clone().unwrap_or_default())
            } else {
                let ddl = msg
                    .rows
                    .first()
                    .and_then(|r| r.get(1))
                    .cloned()
                    .unwrap_or_default();
                match gm::extract_mysql_column_definition(&ddl, &column) {
                    Some(definition) => MoveColumnStatus::Ready { definition },
                    None => MoveColumnStatus::Failed(format!(
                        "Column `{}` not found in SHOW CREATE TABLE output",
                        column
                    )),
                }
            };
        },
    );
}

pub(crate) fn review_move_column(t: &mut Tabular) {
    let Some(dialog) = t.grid_ext.move_column.take() else {
        return;
    };
    let MoveColumnStatus::Ready { definition } = &dialog.status else {
        t.grid_ext.move_column = Some(dialog);
        return;
    };
    let sql =
        gm::build_mysql_move_column_sql(&dialog.qualified, definition, dialog.after.as_deref());
    let position = match &dialog.after {
        Some(c) => format!("after `{}`", c),
        None => "to the first position".to_string(),
    };
    t.grid_ext.review = Some(ChangeReview {
        kind: ReviewKind::Ddl,
        title: "Move column".to_string(),
        summary: format!("Move `{}` {}", dialog.column, position),
        sql: format!("{};", sql),
        warnings: vec![
            "MySQL may rebuild the table to apply this change; on large tables it can take a while and lock writes."
                .to_string(),
        ],
        connection_id: dialog.connection_id,
    });
}

// ─── Row as JSON (B10) ─────────────────────────────────────────────────────

pub(crate) fn open_row_json(t: &mut Tabular, row: usize) {
    let Some(values) = t.current_table_data.get(row).cloned() else {
        return;
    };
    let connection_id = t.current_connection_id;
    let database = active_database_name(t);
    let table = super::infer_current_table_name(t);
    let column_tables: Vec<Option<String>> = t
        .current_column_metadata
        .as_ref()
        .map(|meta| meta.iter().map(|m| m.table_name.clone()).collect())
        .unwrap_or_default();
    let fks = connection_id
        .and_then(|cid| crate::cache_data::get_foreign_keys_from_cache_shared(t, cid, &database))
        .unwrap_or_default();
    let generation = t.grid_ext.next_generation();
    t.grid_ext.row_json = Some(RowJsonViewer {
        open: true,
        generation,
        connection_id,
        db_type: connection_id.and_then(|cid| connection_db_type(t, cid)),
        database,
        schema: active_schema_name(t),
        fks,
        root: RowJsonNode {
            table,
            columns: t.current_table_headers.clone(),
            values,
            column_tables,
            children: HashMap::new(),
        },
        show_raw_json: false,
        row_label: format!("Row {}", row + 1),
    });
}

/// Muat baris induk untuk kolom FK `col` pada node di `path`.
pub(crate) fn expand_row_json_fk(t: &mut Tabular, path: Vec<usize>, col: usize) {
    let Some(viewer) = t.grid_ext.row_json.as_mut() else {
        return;
    };
    if path.len() >= MAX_FK_DEPTH {
        return;
    }
    let (Some(cid), Some(db_type)) = (viewer.connection_id, viewer.db_type.clone()) else {
        return;
    };
    let generation = viewer.generation;
    let Some(node) = viewer.root.node_at(&path) else {
        return;
    };
    let Some(column) = node.columns.get(col).cloned() else {
        return;
    };
    let value = node.values.get(col).cloned().unwrap_or_default();
    let Some(fk) = fk_for(&viewer.fks, node.column_table(col), &column).cloned() else {
        return;
    };
    let qualified = gm::qualified_table(
        &db_type,
        Some(&viewer.database),
        viewer.schema.as_deref(),
        &fk.referenced_table_name,
    );
    let sql = gm::build_fk_row_lookup_sql(&db_type, &qualified, &fk.referenced_column_name, &value);
    if let Some(node) = viewer.root.node_at_mut(&path) {
        node.children.insert(col, FkChild::Loading);
    }
    let ref_table = fk.referenced_table_name.clone();
    run_lookup_query(t, cid, sql, move |t, msg| {
        let Some(viewer) = t.grid_ext.row_json.as_mut() else {
            return;
        };
        if viewer.generation != generation {
            return;
        }
        let Some(node) = viewer.root.node_at_mut(&path) else {
            return;
        };
        let child = if !msg.success {
            FkChild::Failed(msg.error.clone().unwrap_or_default())
        } else if let Some(row) = msg.rows.first() {
            FkChild::Loaded(Box::new(RowJsonNode {
                table: ref_table,
                columns: msg.headers.clone(),
                values: row.clone(),
                column_tables: Vec::new(),
                children: HashMap::new(),
            }))
        } else {
            FkChild::NotFound
        };
        node.children.insert(col, child);
    });
}

// ─── FK value picker (B11) ─────────────────────────────────────────────────

pub(crate) fn open_fk_picker(t: &mut Tabular, row: usize, col: usize, fk: ForeignKey) {
    let Some(cid) = t.current_connection_id else {
        return;
    };
    let Some(db_type) = connection_db_type(t, cid) else {
        return;
    };
    let qualified = gm::qualified_table(
        &db_type,
        Some(&active_database_name(t)),
        active_schema_name(t).as_deref(),
        &fk.referenced_table_name,
    );
    let generation = t.grid_ext.next_generation();
    t.grid_ext.fk_picker = Some(FkPicker {
        row,
        col,
        column: t
            .current_table_headers
            .get(col)
            .cloned()
            .unwrap_or_default(),
        fk,
        qualified,
        db_type,
        connection_id: cid,
        search: String::new(),
        last_search: None,
        search_changed_at: None,
        headers: Vec::new(),
        rows: Vec::new(),
        label_cols: Vec::new(),
        loading: false,
        error: None,
        generation,
        focus_request: true,
    });
    fk_picker_fetch(t);
}

pub(crate) fn fk_picker_fetch(t: &mut Tabular) {
    let generation = t.grid_ext.next_generation();
    let Some(picker) = t.grid_ext.fk_picker.as_mut() else {
        return;
    };
    picker.generation = generation;
    picker.loading = true;
    picker.error = None;
    picker.last_search = Some(picker.search.clone());
    picker.search_changed_at = None;
    let sql = gm::build_fk_picker_sql(
        &picker.db_type,
        &picker.qualified,
        &picker.fk.referenced_column_name,
        &picker.label_cols,
        &picker.search,
        FK_PICKER_LIMIT,
    );
    let cid = picker.connection_id;
    run_lookup_query(t, cid, sql, move |t, msg| {
        let Some(picker) = t.grid_ext.fk_picker.as_mut() else {
            return;
        };
        if picker.generation != generation {
            return;
        }
        picker.loading = false;
        if !msg.success {
            picker.error = Some(msg.error.clone().unwrap_or_default());
            return;
        }
        if picker.label_cols.is_empty() {
            picker.label_cols =
                gm::pick_label_columns(&msg.headers, &picker.fk.referenced_column_name, 2);
        }
        picker.headers = msg.headers.clone();
        picker.rows = msg.rows.clone();
    });
}

pub(crate) fn fk_picker_choose(t: &mut Tabular, value: String) {
    let Some(picker) = t.grid_ext.fk_picker.take() else {
        return;
    };
    t.grid_set_cell_value(picker.row, picker.col, value);
}

// ─── Eksekusi request dari render ──────────────────────────────────────────

pub(crate) fn apply_grid_requests(t: &mut Tabular, req: GridRequests) {
    if req.undo {
        match t.grid_undo() {
            Some(label) => t.toasts.info(format!("Undo: {}", label)),
            None => t.toasts.info("Nothing to undo"),
        }
    }
    if req.redo {
        match t.grid_redo() {
            Some(label) => t.toasts.info(format!("Redo: {}", label)),
            None => t.toasts.info("Nothing to redo"),
        }
    }
    if let Some((row, col, value)) = req.set_value {
        t.grid_set_cell_value(row, col, value);
    }
    if let Some(rows) = req.toggle_delete_rows {
        t.grid_toggle_delete_rows(rows);
    }
    if let Some(row) = req.duplicate_row {
        t.selected_row = Some(row);
        t.spreadsheet_duplicate_selected_row();
    }
    if let Some((column, op, value)) = req.filter_by {
        let condition = if matches!(op, FilterOperator::IsNull | FilterOperator::IsNotNull) {
            FilterCondition::new(column, op, String::new())
        } else {
            FilterCondition::new(column, op, value)
        };
        t.visual_filter.conditions.push(condition);
        t.visual_filter.is_open = true;
        let db_type = t
            .current_connection_id
            .and_then(|cid| connection_db_type(t, cid));
        t.sql_filter_text =
            super::build_where_from_visual_filter(&t.visual_filter, db_type.as_ref());
        super::apply_sql_filter(t);
    }
    if let Some(row) = req.open_row_json {
        open_row_json(t, row);
    }
    if let Some((row, col, fk)) = req.open_fk_picker {
        open_fk_picker(t, row, col, fk);
    }
    if let Some(column) = req.hide_column {
        set_column_hidden(t, &column, true);
    }
    if let Some(column) = req.toggle_column {
        let hidden = hidden_columns(t).contains(&column);
        set_column_hidden(t, &column, !hidden);
    }
    if req.show_all_columns {
        show_all_columns(t);
    }
    if let Some((moving, target, after)) = req.reorder {
        reorder_column(t, &moving, &target, after);
    }
    if req.reset_order {
        reset_column_order(t);
    }
    if let Some(column) = req.move_column_physical {
        open_move_column_dialog(t, column);
    }
    if req.open_find {
        t.grid_ext.find.open = true;
        t.grid_ext.find.focus_request = true;
    }
    if req.open_jump {
        let types = column_types(t);
        t.grid_ext.jump = JumpState {
            open: true,
            focus_request: true,
            types,
            ..Default::default()
        };
    }
    if let Some((column, value)) = req.rule_from_cell {
        let mut rule = gm::HighlightRule::new(column);
        if gm::is_null_cell(&value) {
            rule.operator = FilterOperator::IsNull;
        } else {
            rule.value = gm::display_value(&value).into_owned();
        }
        t.grid_ext.highlight_rules.push(rule);
        t.grid_ext.show_rules_editor = true;
        persist_highlight_rules(t);
    }
    if req.open_rules_editor {
        t.grid_ext.show_rules_editor = true;
    }
    if req.review_save {
        request_save_review(t);
    }
    if req.discard {
        discard_pending_changes(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabular_with_rows() -> Tabular {
        let mut t = Tabular::default();
        t.current_table_headers = vec!["id".into(), "name".into()];
        t.current_table_data = vec![
            vec!["1".into(), "a".into()],
            vec!["2".into(), "b".into()],
            vec!["3".into(), "c".into()],
        ];
        t.all_table_data = t.current_table_data.clone();
        t.total_rows = 3;
        t
    }

    #[test]
    fn edit_sel_bisa_di_undo_dan_redo() {
        let mut t = tabular_with_rows();
        t.spreadsheet_start_cell_edit(1, 1);
        t.spreadsheet_state.cell_edit_text = "B".into();
        t.spreadsheet_finish_cell_edit(true);
        assert_eq!(t.current_table_data[1][1], "B");
        assert_eq!(t.spreadsheet_state.pending_operations.len(), 1);

        assert!(t.grid_undo().is_some());
        assert_eq!(t.current_table_data[1][1], "b");
        assert!(t.spreadsheet_state.pending_operations.is_empty());
        assert!(!t.spreadsheet_state.is_dirty);

        assert!(t.grid_redo().is_some());
        assert_eq!(t.current_table_data[1][1], "B");
        assert_eq!(t.spreadsheet_state.pending_operations.len(), 1);
    }

    #[test]
    fn hapus_baris_ditandai_lalu_bisa_dipulihkan() {
        let mut t = tabular_with_rows();
        t.grid_toggle_delete_rows(vec![0, 2]);
        assert_eq!(t.current_table_data.len(), 3, "baris lama hanya ditandai");
        assert_eq!(
            gm::pending_deleted_rows(&t.spreadsheet_state.pending_operations),
            [0, 2].into()
        );
        t.grid_toggle_delete_rows(vec![2]);
        assert_eq!(
            gm::pending_deleted_rows(&t.spreadsheet_state.pending_operations),
            [0].into()
        );
        t.grid_undo();
        assert_eq!(
            gm::pending_deleted_rows(&t.spreadsheet_state.pending_operations),
            [0, 2].into()
        );
    }

    #[test]
    fn baris_baru_memakai_default_dan_bisa_dibuang() {
        let mut t = tabular_with_rows();
        t.spreadsheet_add_row();
        t.spreadsheet_finish_cell_edit(false);
        assert_eq!(t.current_table_data.len(), 4);
        assert!(gm::is_raw_default(&t.current_table_data[3][0]));
        assert_eq!(
            gm::pending_inserted_rows(&t.spreadsheet_state.pending_operations),
            [3].into()
        );

        // Edit sel DEFAULT tanpa mengetik apa pun tidak mengubah nilainya.
        t.spreadsheet_start_cell_edit(3, 1);
        assert_eq!(t.spreadsheet_state.cell_edit_text, "");
        t.spreadsheet_finish_cell_edit(true);
        assert!(gm::is_raw_default(&t.current_table_data[3][1]));

        t.grid_toggle_delete_rows(vec![3]);
        assert_eq!(t.current_table_data.len(), 3);
        assert!(t.spreadsheet_state.pending_operations.is_empty());
        t.grid_undo();
        assert_eq!(t.current_table_data.len(), 4);
        t.grid_undo();
        assert_eq!(t.current_table_data.len(), 3, "undo penambahan baris");
    }

    #[test]
    fn duplikat_baris_menggeser_operasi() {
        let mut t = tabular_with_rows();
        t.grid_toggle_delete_rows(vec![2]);
        t.selected_row = Some(0);
        t.spreadsheet_duplicate_selected_row();
        assert_eq!(t.current_table_data.len(), 4);
        assert_eq!(
            gm::pending_deleted_rows(&t.spreadsheet_state.pending_operations),
            [3].into(),
            "baris yang ditandai hapus ikut bergeser"
        );
        assert_eq!(
            gm::pending_inserted_rows(&t.spreadsheet_state.pending_operations),
            [1].into()
        );
        t.grid_undo();
        assert_eq!(t.current_table_data.len(), 3);
        assert_eq!(
            gm::pending_deleted_rows(&t.spreadsheet_state.pending_operations),
            [2].into()
        );
    }

    #[test]
    fn discard_mengembalikan_data() {
        let mut t = tabular_with_rows();
        t.grid_set_cell_value(0, 1, "zzz".into());
        t.grid_toggle_delete_rows(vec![1]);
        discard_pending_changes(&mut t);
        assert_eq!(t.current_table_data[0][1], "a");
        assert!(t.spreadsheet_state.pending_operations.is_empty());
        assert!(t.grid_ext.undo.is_empty());
    }

    #[test]
    fn find_dan_navigasi() {
        let mut t = tabular_with_rows();
        t.grid_ext.find.open = true;
        t.grid_ext.find.query = "B".into();
        refresh_find_matches(&mut t);
        assert_eq!(t.grid_ext.find.matches, vec![(1, 1)]);
        find_step(&mut t, 1);
        assert_eq!(t.selected_cell, Some((1, 1)));
    }

    fn sqlite_tabular() -> Tabular {
        let mut t = tabular_with_rows();
        t.connections
            .push(crate::models::structs::ConnectionConfig {
                id: Some(1),
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            });
        t.current_connection_id = Some(1);
        t.current_table_name = "Table: t".into();
        t.spreadsheet_state.primary_key_columns = vec!["id".into()];
        t
    }

    #[test]
    fn rewind_memakai_nilai_asli_setelah_edit_berulang() {
        let mut t = sqlite_tabular();
        t.grid_set_cell_value(0, 1, "x".into());
        t.grid_set_cell_value(0, 1, gm::QuickValue::Now.cell_value());
        t.grid_set_cell_value(0, 1, "y".into());
        t.spreadsheet_add_row();
        t.spreadsheet_finish_cell_edit(false);
        let rewind = t.spreadsheet_generate_rewind();
        let sql = rewind.sql.expect("ada SQL pembalik");
        assert_eq!(sql, "UPDATE \"t\" SET \"name\" = 'a' WHERE \"id\" = '1'");
        assert!(
            rewind.notes.iter().any(|n| n.contains("New row 4")),
            "insert dengan kunci DEFAULT tidak bisa dibalik: {:?}",
            rewind.notes
        );
    }

    #[test]
    fn edit_di_halaman_kedua_mengenai_baris_yang_benar() {
        let mut t = tabular_with_rows();
        t.all_table_data.push(vec!["4".into(), "d".into()]);
        t.page_size = 2;
        t.current_page = 1;
        t.use_server_pagination = false;
        t.current_table_data = t.all_table_data[2..4].to_vec();
        t.grid_set_cell_value(0, 1, "C".into());
        assert_eq!(t.all_table_data[2][1], "C");
        assert_eq!(t.all_table_data[0][1], "a");
        match &t.spreadsheet_state.pending_operations[0] {
            CellEditOperation::Update { old_value, .. } => assert_eq!(old_value, "c"),
            other => panic!("operasi tak terduga: {:?}", other),
        }
        t.grid_undo();
        assert_eq!(t.all_table_data[2][1], "c");
    }

    #[test]
    fn json_baris_dengan_fk_bersarang() {
        let mut root = RowJsonNode {
            table: "orders".into(),
            columns: vec!["id".into(), "customer_id".into(), "note".into()],
            values: vec![
                "10".into(),
                "7".into(),
                crate::models::structs::NULL_CELL.into(),
            ],
            ..Default::default()
        };
        root.children.insert(
            1,
            FkChild::Loaded(Box::new(RowJsonNode {
                table: "customers".into(),
                columns: vec!["id".into(), "name".into()],
                values: vec!["7".into(), "Ann".into()],
                ..Default::default()
            })),
        );
        let json = root.to_json();
        assert_eq!(json["id"], 10);
        assert!(json["note"].is_null());
        assert_eq!(json["customer_id (customers)"]["name"], "Ann");
        assert!(root.node_at(&[1]).is_some());
        assert!(root.node_at(&[0]).is_none());
        assert_eq!(json_scalar("007"), serde_json::Value::String("007".into()));
    }

    fn fk(table: &str, column: &str, ref_table: &str) -> ForeignKey {
        ForeignKey {
            constraint_name: format!("fk_{table}_{column}"),
            table_name: table.to_string(),
            column_name: column.to_string(),
            referenced_table_name: ref_table.to_string(),
            referenced_column_name: "id".to_string(),
        }
    }

    #[test]
    fn peta_fk_per_kolom_memakai_tabel_asal_atau_fallback() {
        let headers: Vec<String> = vec!["id".into(), "User_ID".into(), "city_id".into()];
        let fks = vec![
            fk("orders", "user_id", "users"),
            fk("addresses", "city_id", "cities"),
        ];

        // Tanpa metadata: tabel fallback menentukan FK mana yang cocok.
        let map = map_foreign_keys_to_columns(&headers, |_| None, "orders", &fks);
        assert_eq!(map.len(), 1);
        assert_eq!(map[&1].referenced_table_name, "users");

        // Metadata kolom menimpa fallback (hasil JOIN); kosong = pakai fallback.
        let tables = [Some(""), Some("orders"), Some("ADDRESSES")];
        let map = map_foreign_keys_to_columns(&headers, |i| tables[i], "orders", &fks);
        assert_eq!(map.len(), 2);
        assert_eq!(map[&2].referenced_table_name, "cities");

        // Tabel tidak diketahui: cocokkan nama kolom saja.
        let map = map_foreign_keys_to_columns(&headers, |_| None, "", &fks);
        assert_eq!(map.len(), 2);

        assert!(map_foreign_keys_to_columns(&headers, |_| None, "orders", &[]).is_empty());
    }

    #[test]
    fn himpunan_hasil_find_mengikuti_daftar_matches() {
        let mut t = tabular_with_rows();
        t.grid_ext.find.open = true;
        t.grid_ext.find.query = "b".into();
        refresh_find_matches(&mut t);
        assert_eq!(t.grid_ext.find.matches, vec![(1, 1)]);
        assert!(t.grid_ext.find.match_set.contains(&(1, 1)));
        assert_eq!(t.grid_ext.find.match_set.len(), 1);
        // Cache masih berlaku: himpunan yang sama dipakai ulang, tidak dibangun lagi.
        let before = Arc::clone(&t.grid_ext.find.match_set);
        refresh_find_matches(&mut t);
        assert!(Arc::ptr_eq(&before, &t.grid_ext.find.match_set));

        t.grid_ext.find.query.clear();
        refresh_find_matches(&mut t);
        assert!(t.grid_ext.find.match_set.is_empty());
    }

    #[test]
    fn tanpa_koneksi_peta_fk_kosong() {
        let mut t = tabular_with_rows();
        t.current_connection_id = None;
        let headers = t.current_table_headers.clone();
        assert!(fk_by_column(&mut t, &headers, "db", "users").is_empty());
    }
}
