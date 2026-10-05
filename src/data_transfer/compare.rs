//! Data Compare & Sync (H11): bandingkan isi dua tabel per kunci, lalu susun
//! skrip yang membuat tujuan sama dengan sumber. Pembandingnya murni; hanya
//! [`compare_tables`] yang menyentuh database.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::catalog::{self, Endpoint};
use super::types::SourceColumn;
use super::values::{ValueKind, kind_from_type, sql_value};
use super::{NULL_MARKER, TableData};
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::quote_ident;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompareOptions {
    /// Kolom kunci; kosong = primary key tabel sumber.
    pub key_columns: Vec<String>,
    /// Filter baris kedua sisi (isi klausa `WHERE`).
    pub where_clause: Option<String>,
    /// Baris maksimum yang dibaca per sisi. `None` = tanpa batas: query
    /// dikirim tanpa klausa batas dan kedua tabel dimuat utuh ke memori.
    pub row_limit: Option<u64>,
    pub ignore_columns: Vec<String>,
    /// Abaikan spasi di akhir nilai (padding `CHAR`).
    pub trim_trailing_space: bool,
    /// Samakan bentuk angka, boolean, dan timestamp antar engine
    /// (`1.0` = `1`, `t` = `1`, `2026-01-02T03:04:05.000` = `2026-01-02 03:04:05`).
    pub tolerant_values: bool,
    pub case_insensitive: bool,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            key_columns: Vec::new(),
            where_clause: None,
            row_limit: Some(100_000),
            ignore_columns: Vec::new(),
            trim_trailing_space: true,
            tolerant_values: true,
            case_insensitive: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    OnlyInSource,
    OnlyInTarget,
    Changed,
}

impl DiffKind {
    pub fn label(self) -> &'static str {
        match self {
            DiffKind::OnlyInSource => "Only in source",
            DiffKind::OnlyInTarget => "Only in target",
            DiffKind::Changed => "Changed",
        }
    }
}

/// Satu baris yang berbeda. `source`/`target` sejajar dengan
/// [`CompareResult::columns`]; kosong bila baris tidak ada di sisi itu.
/// Sel NULL ditulis sebagai penanda teks untuk ditampilkan; nullness yang
/// sebenarnya ada di `source_nulls`/`target_nulls` (sejajar, `true` = NULL).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDiff {
    pub kind: DiffKind,
    pub source: Vec<String>,
    pub target: Vec<String>,
    pub source_nulls: Vec<bool>,
    pub target_nulls: Vec<bool>,
    /// Indeks kolom yang nilainya berbeda (hanya untuk `Changed`).
    pub changed: Vec<usize>,
}

impl RowDiff {
    /// Nilai sisi sumber dengan nullness (`None` = SQL NULL).
    pub fn source_value(&self, col: usize) -> Option<&str> {
        side_value(&self.source, &self.source_nulls, col)
    }

    /// Nilai sisi tujuan dengan nullness (`None` = SQL NULL).
    pub fn target_value(&self, col: usize) -> Option<&str> {
        side_value(&self.target, &self.target_nulls, col)
    }
}

fn side_value<'a>(cells: &'a [String], nulls: &[bool], col: usize) -> Option<&'a str> {
    let cell = cells.get(col)?;
    (!nulls.get(col).copied().unwrap_or(false)).then_some(cell.as_str())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompareResult {
    /// Kolom yang ada di kedua sisi dan tidak diabaikan (nama sisi sumber).
    pub columns: Vec<String>,
    /// Nama kolom yang sama di sisi tujuan.
    pub target_columns: Vec<String>,
    pub key_indices: Vec<usize>,
    pub diffs: Vec<RowDiff>,
    pub identical: usize,
    pub source_rows: usize,
    pub target_rows: usize,
    pub source_only_columns: Vec<String>,
    pub target_only_columns: Vec<String>,
    /// Baris dengan kunci kembar di satu sisi (hanya yang pertama dibandingkan).
    pub duplicate_keys: usize,
    /// Batas baris tercapai; perbandingan hanya atas sebagian data.
    pub truncated: bool,
}

impl CompareResult {
    /// `(hanya sumber, hanya tujuan, berubah)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let count = |k: DiffKind| self.diffs.iter().filter(|d| d.kind == k).count();
        (
            count(DiffKind::OnlyInSource),
            count(DiffKind::OnlyInTarget),
            count(DiffKind::Changed),
        )
    }
}

fn looks_like_timestamp(v: &str) -> bool {
    let b = v.as_bytes();
    v.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b' ' || b[10] == b'T')
        && b[13] == b':'
        && b[16] == b':'
        && b[..4].iter().all(u8::is_ascii_digit)
}

/// Bentuk kanonik nilai untuk dibandingkan. `None` = SQL NULL, yang tidak
/// pernah sama dengan teks apa pun (termasuk string `NULL`).
fn normalize(value: Option<&str>, opts: &CompareOptions) -> Option<String> {
    let value = value?;
    let mut v = if opts.trim_trailing_space {
        value.trim_end().to_string()
    } else {
        value.to_string()
    };
    if opts.tolerant_values {
        match v.to_ascii_lowercase().as_str() {
            "true" | "t" => return Some("1".to_string()),
            "false" | "f" => return Some("0".to_string()),
            _ => {}
        }
        if let Ok(d) = rust_decimal::Decimal::from_str(&v) {
            return Some(d.normalize().to_string());
        }
        if looks_like_timestamp(&v) {
            v.replace_range(10..11, " ");
            // Nol di belakang pecahan detik tidak bermakna; titiknya menahan
            // pemangkasan supaya nol milik detik (`…:00`) tetap utuh.
            if v.get(19..).is_some_and(|rest| rest.starts_with('.')) {
                v = v.trim_end_matches('0').trim_end_matches('.').to_string();
            }
        }
    }
    if opts.case_insensitive {
        v = v.to_lowercase();
    }
    Some(v)
}

fn position(headers: &[String], name: &str) -> Option<usize> {
    headers.iter().position(|h| h.eq_ignore_ascii_case(name))
}

/// Kunci baris yang sudah dinormalkan.
type Key = Vec<Option<String>>;

/// Hash 128-bit kunci. Indeks baris menyimpan hash ini, bukan salinan
/// kuncinya; tabrakan tetap ditangani dengan membandingkan kunci aslinya.
fn key_hash(key: &[Option<String>]) -> u128 {
    use std::hash::{Hash, Hasher};
    let half = |seed: u8| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        seed.hash(&mut hasher);
        key.hash(&mut hasher);
        hasher.finish()
    };
    (u128::from(half(0)) << 64) | u128::from(half(1))
}

/// Baris-baris dengan hash kunci yang sama. Hampir selalu satu; `Many` hanya
/// muncul saat hash bertabrakan.
enum Bucket {
    One(usize),
    Many(Vec<usize>),
}

impl Bucket {
    fn rows(&self) -> &[usize] {
        match self {
            Bucket::One(row) => std::slice::from_ref(row),
            Bucket::Many(rows) => rows,
        }
    }

    fn push(&mut self, row: usize) {
        match self {
            Bucket::One(first) => *self = Bucket::Many(vec![*first, row]),
            Bucket::Many(rows) => rows.push(row),
        }
    }

    fn replace(&mut self, old: usize, new: usize) {
        match self {
            Bucket::One(row) => *row = new,
            Bucket::Many(rows) => {
                if let Some(slot) = rows.iter_mut().find(|r| **r == old) {
                    *slot = new;
                }
            }
        }
    }
}

/// Bandingkan dua tabel di memori. `opts.key_columns` wajib diisi.
pub fn diff_tables(
    source: &TableData,
    target: &TableData,
    opts: &CompareOptions,
) -> Result<CompareResult, String> {
    diff_tables_with(source, target, opts, &key_hash)
}

/// [`diff_tables`] dengan fungsi hash yang bisa diganti (tes tabrakan).
///
/// Kedua tabel hanya dibaca di tempat: tidak ada salinan baris terproyeksi,
/// dan indeks kunci berisi hash 128-bit + nomor baris, bukan kunci utuh.
/// Baris baru disalin hanya untuk yang masuk ke daftar perbedaan.
fn diff_tables_with(
    source: &TableData,
    target: &TableData,
    opts: &CompareOptions,
    hash: &dyn Fn(&[Option<String>]) -> u128,
) -> Result<CompareResult, String> {
    use std::collections::hash_map::Entry;

    if opts.key_columns.is_empty() {
        return Err("Choose at least one key column".to_string());
    }
    let ignored = |name: &str| {
        opts.ignore_columns
            .iter()
            .any(|c| c.eq_ignore_ascii_case(name))
    };
    let mut result = CompareResult {
        source_rows: source.rows.len(),
        target_rows: target.rows.len(),
        ..Default::default()
    };
    // (indeks sumber, indeks tujuan) untuk tiap kolom yang dibandingkan.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for (si, name) in source.headers.iter().enumerate() {
        match position(&target.headers, name) {
            Some(ti) if !ignored(name) => {
                pairs.push((si, ti));
                result.columns.push(name.clone());
                result.target_columns.push(target.headers[ti].clone());
            }
            Some(_) => {}
            None => result.source_only_columns.push(name.clone()),
        }
    }
    result.target_only_columns = target
        .headers
        .iter()
        .filter(|h| position(&source.headers, h).is_none())
        .cloned()
        .collect();
    for key in &opts.key_columns {
        let idx = position(&result.columns, key)
            .ok_or_else(|| format!("Key column \"{key}\" must exist on both sides"))?;
        result.key_indices.push(idx);
    }

    let key_indices = &result.key_indices;
    let source_key = |row: usize| -> Key {
        key_indices
            .iter()
            .map(|i| normalize(source.value(row, pairs[*i].0), opts))
            .collect()
    };
    let target_key = |row: usize| -> Key {
        key_indices
            .iter()
            .map(|i| normalize(target.value(row, pairs[*i].1), opts))
            .collect()
    };
    // Salinan baris untuk daftar perbedaan: teks tampilan + nullness.
    let project = |data: &TableData, row: usize, pick: &dyn Fn(&(usize, usize)) -> usize| {
        let mut cells = Vec::with_capacity(pairs.len());
        let mut nulls = Vec::with_capacity(pairs.len());
        for pair in &pairs {
            match data.value(row, pick(pair)) {
                Some(text) => {
                    cells.push(text.to_string());
                    nulls.push(false);
                }
                None => {
                    cells.push(NULL_MARKER.to_string());
                    nulls.push(true);
                }
            }
        }
        (cells, nulls)
    };

    // Indeks tujuan: per kunci, baris terakhir yang memilikinya.
    let mut target_index: HashMap<u128, Bucket> = HashMap::with_capacity(target.rows.len());
    let mut duplicates = 0usize;
    for row in 0..target.rows.len() {
        let key = target_key(row);
        match target_index.entry(hash(&key)) {
            Entry::Vacant(slot) => {
                slot.insert(Bucket::One(row));
            }
            Entry::Occupied(mut slot) => {
                let same = slot
                    .get()
                    .rows()
                    .iter()
                    .copied()
                    .find(|other| target_key(*other) == key);
                match same {
                    Some(old) => {
                        duplicates += 1;
                        slot.get_mut().replace(old, row);
                    }
                    None => slot.get_mut().push(row),
                }
            }
        }
    }

    let mut matched = vec![false; target.rows.len()];
    // Kunci sumber yang sudah terlihat: baris pertama per kunci yang dipakai.
    let mut seen_source: HashMap<u128, Bucket> = HashMap::with_capacity(source.rows.len());
    let mut diffs = Vec::new();
    let mut identical = 0usize;
    for row in 0..source.rows.len() {
        let key = source_key(row);
        let h = hash(&key);
        match seen_source.entry(h) {
            Entry::Vacant(slot) => {
                slot.insert(Bucket::One(row));
            }
            Entry::Occupied(mut slot) => {
                if slot
                    .get()
                    .rows()
                    .iter()
                    .any(|other| source_key(*other) == key)
                {
                    duplicates += 1;
                    continue;
                }
                slot.get_mut().push(row);
            }
        }
        let hit = target_index.get(&h).and_then(|bucket| {
            bucket
                .rows()
                .iter()
                .copied()
                .find(|other| target_key(*other) == key)
        });
        match hit {
            Some(ti) => {
                matched[ti] = true;
                let changed: Vec<usize> = pairs
                    .iter()
                    .enumerate()
                    .filter(|(_, (s, t))| {
                        normalize(source.value(row, *s), opts)
                            != normalize(target.value(ti, *t), opts)
                    })
                    .map(|(c, _)| c)
                    .collect();
                if changed.is_empty() {
                    identical += 1;
                } else {
                    let (src, source_nulls) = project(source, row, &|p| p.0);
                    let (dst, target_nulls) = project(target, ti, &|p| p.1);
                    diffs.push(RowDiff {
                        kind: DiffKind::Changed,
                        source: src,
                        target: dst,
                        source_nulls,
                        target_nulls,
                        changed,
                    });
                }
            }
            None => {
                let (src, source_nulls) = project(source, row, &|p| p.0);
                diffs.push(RowDiff {
                    kind: DiffKind::OnlyInSource,
                    source: src,
                    target: Vec::new(),
                    source_nulls,
                    target_nulls: Vec::new(),
                    changed: Vec::new(),
                });
            }
        }
    }
    // Baris tujuan yang kuncinya kembar tidak ikut indeks; yang dilaporkan
    // hanya baris terindeks yang tidak punya pasangan.
    let mut unmatched: Vec<usize> = target_index
        .values()
        .flat_map(|bucket| bucket.rows().iter().copied())
        .filter(|i| !matched[*i])
        .collect();
    unmatched.sort_unstable();
    for i in unmatched {
        let (dst, target_nulls) = project(target, i, &|p| p.1);
        diffs.push(RowDiff {
            kind: DiffKind::OnlyInTarget,
            source: Vec::new(),
            target: dst,
            source_nulls: Vec::new(),
            target_nulls,
            changed: Vec::new(),
        });
    }
    result.diffs = diffs;
    result.identical = identical;
    result.duplicate_keys = duplicates;
    Ok(result)
}

/// Bagian skrip sinkronisasi yang ikut dibuat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncParts {
    pub insert_missing: bool,
    pub update_changed: bool,
    pub delete_extra: bool,
}

impl Default for SyncParts {
    fn default() -> Self {
        Self {
            insert_missing: true,
            update_changed: true,
            delete_extra: false,
        }
    }
}

/// Statement yang membuat tabel tujuan sama dengan sumber, satu elemen per
/// statement (nilai boleh berisi baris baru). `target_sql` sudah dikutip;
/// `kinds` sejajar dengan `result.columns`.
pub fn sync_statements(
    result: &CompareResult,
    db: &DatabaseType,
    target_sql: &str,
    kinds: &[ValueKind],
    parts: SyncParts,
) -> Vec<String> {
    let kind = |i: usize| kinds.get(i).copied().unwrap_or_default();
    let col = |i: usize| quote_ident(db, &result.target_columns[i]);
    // Baris tujuan dikenali lewat nilai kuncinya di sisi tujuan.
    let where_key = |diff: &RowDiff| -> String {
        result
            .key_indices
            .iter()
            .map(|i| match diff.target_value(*i) {
                None => format!("{} IS NULL", col(*i)),
                Some(v) => format!("{} = {}", col(*i), sql_value(db, Some(v), kind(*i))),
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let column_list = (0..result.columns.len())
        .map(col)
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = Vec::new();
    for diff in &result.diffs {
        match diff.kind {
            DiffKind::OnlyInSource if parts.insert_missing => {
                let values: Vec<String> = (0..diff.source.len())
                    .map(|i| sql_value(db, diff.source_value(i), kind(i)))
                    .collect();
                out.push(format!(
                    "INSERT INTO {target_sql} ({column_list}) VALUES ({});",
                    values.join(", ")
                ));
            }
            DiffKind::Changed if parts.update_changed => {
                let sets: Vec<String> = diff
                    .changed
                    .iter()
                    .map(|i| {
                        format!(
                            "{} = {}",
                            col(*i),
                            sql_value(db, diff.source_value(*i), kind(*i))
                        )
                    })
                    .collect();
                out.push(format!(
                    "UPDATE {target_sql} SET {} WHERE {};",
                    sets.join(", "),
                    where_key(diff)
                ));
            }
            DiffKind::OnlyInTarget if parts.delete_extra => {
                out.push(format!(
                    "DELETE FROM {target_sql} WHERE {};",
                    where_key(diff)
                ));
            }
            _ => {}
        }
    }
    out
}

/// [`sync_statements`] sebagai satu skrip untuk ditinjau di editor.
pub fn sync_script(
    result: &CompareResult,
    db: &DatabaseType,
    target_sql: &str,
    kinds: &[ValueKind],
    parts: SyncParts,
) -> String {
    let mut script = sync_statements(result, db, target_sql, kinds, parts).join("\n");
    if !script.is_empty() {
        script.push('\n');
    }
    script
}

/// Hasil perbandingan dua tabel di database, beserta yang dibutuhkan untuk
/// menyusun skrip sinkronisasi.
#[derive(Clone, Debug)]
pub struct CompareOutcome {
    pub result: CompareResult,
    pub target_db: DatabaseType,
    pub target_sql: String,
    pub kinds: Vec<ValueKind>,
    /// Kunci yang dipakai (dari opsi atau primary key sumber).
    pub key_columns: Vec<String>,
}

async fn fetch_side(
    ep: &Endpoint,
    table: &str,
    opts: &CompareOptions,
) -> Result<(TableData, Vec<SourceColumn>, bool), String> {
    let columns = catalog::fetch_columns(ep, table).await?;
    // Indikator NULL ikut di-SELECT, jadi string `NULL` tidak tertukar NULL.
    let select = catalog::PageSelect::new(ep.db_type(), &columns, &[]);
    let order: Vec<String> = columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| quote_ident(ep.db_type(), &c.name))
        .collect();
    // Satu baris ekstra untuk tahu apakah batas memotong data. Tanpa batas,
    // query tidak diberi klausa batas sama sekali.
    let sql = catalog::select_ordered_sql(
        ep.db_type(),
        &ep.table_sql(table),
        &select.select_list,
        opts.where_clause.as_deref(),
        &order,
        opts.row_limit.map(|limit| limit.saturating_add(1)),
    );
    let mut rows = ep.query(&sql).await?.rows;
    let truncated = match opts.row_limit {
        Some(limit) => {
            let over = rows.len() as u64 > limit;
            rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
            over
        }
        None => false,
    };
    let headers = columns.iter().map(|c| c.name.clone()).collect();
    let data = TableData::from_cells(headers, select.decode(rows));
    Ok((data, columns, truncated))
}

/// Baca kedua tabel dan bandingkan.
pub async fn compare_tables(
    source: Endpoint,
    target: Endpoint,
    source_table: &str,
    target_table: &str,
    opts: &CompareOptions,
) -> Result<CompareOutcome, String> {
    let source = source.connect().await.map_err(|e| format!("Source: {e}"))?;
    let target = target.connect().await.map_err(|e| format!("Target: {e}"))?;
    compare_connected(&source, &target, source_table, target_table, opts).await
}

/// [`compare_tables`] untuk endpoint yang sudah tersambung.
async fn compare_connected(
    source: &Endpoint,
    target: &Endpoint,
    source_table: &str,
    target_table: &str,
    opts: &CompareOptions,
) -> Result<CompareOutcome, String> {
    let (src_data, src_cols, src_truncated) = fetch_side(source, source_table, opts)
        .await
        .map_err(|e| format!("Source: {e}"))?;
    let (dst_data, dst_cols, dst_truncated) = fetch_side(target, target_table, opts)
        .await
        .map_err(|e| format!("Target: {e}"))?;

    let mut opts = opts.clone();
    if opts.key_columns.is_empty() {
        opts.key_columns = src_cols
            .iter()
            .filter(|c| c.primary_key)
            .map(|c| c.name.clone())
            .collect();
        if opts.key_columns.is_empty() {
            return Err(
                "Source table has no primary key; choose the key columns to match rows on"
                    .to_string(),
            );
        }
    }
    let mut result = diff_tables(&src_data, &dst_data, &opts)?;
    result.truncated = src_truncated || dst_truncated;
    let kinds = result
        .target_columns
        .iter()
        .map(|name| {
            dst_cols
                .iter()
                .find(|c| &c.name == name)
                .map(|c| kind_from_type(&c.data_type))
                .unwrap_or_default()
        })
        .collect();
    Ok(CompareOutcome {
        result,
        target_db: target.db_type().clone(),
        target_sql: target.table_sql(target_table),
        kinds,
        key_columns: opts.key_columns,
    })
}

/// Pasangan tabel sumber/tujuan untuk perbandingan seluruh database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TablePairing {
    /// `(tabel sumber, tabel tujuan)`.
    pub pairs: Vec<(String, String)>,
    pub source_only: Vec<String>,
    pub target_only: Vec<String>,
}

/// Nama tanpa schema, huruf kecil (`dbo.Orders` -> `orders`).
fn bare_name(table: &str) -> String {
    table.rsplit('.').next().unwrap_or(table).to_lowercase()
}

/// Pasangkan tabel menurut nama, tidak peka huruf. Nama yang tidak cocok
/// persis dicocokkan tanpa schema (`dbo.orders` dengan `orders`) selama nama
/// itu tidak ambigu di kedua sisi, supaya antar engine tetap berpasangan.
pub fn pair_tables(source: &[String], target: &[String]) -> TablePairing {
    let mut pairing = TablePairing::default();
    let mut taken = vec![false; target.len()];
    let mut unmatched = Vec::new();
    for name in source {
        let hit = target
            .iter()
            .enumerate()
            .position(|(i, t)| !taken[i] && t.eq_ignore_ascii_case(name));
        match hit {
            Some(i) => {
                taken[i] = true;
                pairing.pairs.push((name.clone(), target[i].clone()));
            }
            None => unmatched.push(name.clone()),
        }
    }
    for name in unmatched {
        let bare = bare_name(&name);
        let same_in_source = source.iter().filter(|s| bare_name(s) == bare).count();
        let candidates: Vec<usize> = (0..target.len())
            .filter(|i| !taken[*i] && bare_name(&target[*i]) == bare)
            .collect();
        if same_in_source == 1 && candidates.len() == 1 {
            taken[candidates[0]] = true;
            pairing.pairs.push((name, target[candidates[0]].clone()));
        } else {
            pairing.source_only.push(name);
        }
    }
    pairing.pairs.sort_by_key(|pair| pair.0.to_lowercase());
    pairing.target_only = target
        .iter()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(name, _)| name.clone())
        .collect();
    pairing
}

/// Hasil satu tabel dalam perbandingan database; gagal per tabel (mis. tanpa
/// primary key) tidak menghentikan tabel lain.
#[derive(Clone, Debug)]
pub struct TableComparison {
    pub source_table: String,
    pub target_table: String,
    pub outcome: Result<CompareOutcome, String>,
}

impl TableComparison {
    /// Kedua sisi terbaca dan ada baris yang berbeda.
    pub fn differs(&self) -> bool {
        self.outcome
            .as_ref()
            .is_ok_and(|o| !o.result.diffs.is_empty())
    }
}

#[derive(Clone, Debug, Default)]
pub struct DatabaseComparison {
    pub tables: Vec<TableComparison>,
    /// Tabel yang hanya ada di satu sisi (tidak dibandingkan).
    pub source_only: Vec<String>,
    pub target_only: Vec<String>,
    /// Dihentikan pengguna; `tables` hanya berisi yang sempat dibandingkan.
    pub cancelled: bool,
}

/// Keadaan perbandingan database yang dibaca UI tiap frame.
#[derive(Clone, Debug, Default)]
pub struct CompareProgress {
    pub current_table: String,
    pub tables_done: usize,
    pub tables_total: usize,
}

pub type CompareProgressHandle = Arc<Mutex<CompareProgress>>;

pub fn with_progress(progress: &CompareProgressHandle, f: impl FnOnce(&mut CompareProgress)) {
    let mut guard = progress
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard);
}

/// Bandingkan semua tabel yang ada di kedua database, satu per satu dengan
/// opsi yang sama. `opts.key_columns` kosong berarti primary key tiap tabel.
pub async fn compare_databases(
    source: Endpoint,
    target: Endpoint,
    opts: &CompareOptions,
    progress: CompareProgressHandle,
    cancel: Arc<AtomicBool>,
) -> Result<DatabaseComparison, String> {
    let source = source.connect().await.map_err(|e| format!("Source: {e}"))?;
    let target = target.connect().await.map_err(|e| format!("Target: {e}"))?;
    let source_tables = catalog::list_tables(&source)
        .await
        .map_err(|e| format!("Source: {e}"))?;
    let target_tables = catalog::list_tables(&target)
        .await
        .map_err(|e| format!("Target: {e}"))?;
    if source_tables.is_empty() && target_tables.is_empty() {
        return Err("No tables found on either side; choose the databases to compare".to_string());
    }
    let pairing = pair_tables(&source_tables, &target_tables);
    with_progress(&progress, |p| p.tables_total = pairing.pairs.len());

    let mut out = DatabaseComparison {
        source_only: pairing.source_only,
        target_only: pairing.target_only,
        ..Default::default()
    };
    for (source_table, target_table) in pairing.pairs {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        with_progress(&progress, |p| p.current_table = source_table.clone());
        let outcome = compare_connected(&source, &target, &source_table, &target_table, opts).await;
        if let Err(e) = &outcome {
            log::debug!("[TRANSFER] compare {source_table} failed: {e}");
        }
        out.tables.push(TableComparison {
            source_table,
            target_table,
            outcome,
        });
        with_progress(&progress, |p| p.tables_done += 1);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(headers: &[&str], rows: &[&[&str]]) -> TableData {
        TableData::new(
            headers.iter().map(|s| s.to_string()).collect(),
            rows.iter()
                .map(|r| r.iter().map(|s| s.to_string()).collect())
                .collect(),
        )
    }

    fn keyed(key: &str) -> CompareOptions {
        CompareOptions {
            key_columns: vec![key.to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn pairs_tables_by_name_then_by_unqualified_name() {
        let names = |list: &[&str]| -> Vec<String> { list.iter().map(|s| s.to_string()).collect() };
        let source = names(&["orders", "Users", "audit.log", "sales.log", "only_here"]);
        let target = names(&["dbo.orders", "users", "log", "extra"]);
        let pairing = pair_tables(&source, &target);
        assert_eq!(
            pairing.pairs,
            vec![
                ("orders".to_string(), "dbo.orders".to_string()),
                ("Users".to_string(), "users".to_string()),
            ]
        );
        // `log` ambigu di sumber (dua schema), jadi tidak dipasangkan.
        assert_eq!(
            pairing.source_only,
            names(&["audit.log", "sales.log", "only_here"])
        );
        assert_eq!(pairing.target_only, names(&["log", "extra"]));
    }

    #[test]
    fn finds_missing_extra_and_changed_rows() {
        let src = table(
            &["id", "name", "qty"],
            &[&["1", "a", "5"], &["2", "b", "6"], &["3", "c", "7"]],
        );
        let dst = table(
            &["ID", "name", "qty"],
            &[&["1", "a", "5"], &["2", "B", "6"], &["4", "d", "8"]],
        );
        let r = diff_tables(&src, &dst, &keyed("id")).unwrap();
        assert_eq!(r.identical, 1);
        assert_eq!(r.counts(), (1, 1, 1));
        let changed = r
            .diffs
            .iter()
            .find(|d| d.kind == DiffKind::Changed)
            .unwrap();
        assert_eq!(changed.changed, vec![1]);
        assert_eq!(changed.source[1], "b");
        assert_eq!(changed.target[1], "B");
        assert_eq!(r.target_columns[0], "ID");
    }

    #[test]
    fn tolerant_values_ignore_engine_formatting() {
        let src = table(
            &["id", "price", "ok", "at", "pad", "n"],
            &[&[
                "1",
                "10.50",
                "t",
                "2026-01-02T03:04:00.000000",
                "ab  ",
                "NULL",
            ]],
        );
        let dst = table(
            &["id", "price", "ok", "at", "pad", "n"],
            &[&["1.0", "10.5", "1", "2026-01-02 03:04:00", "ab", "NULL"]],
        );
        let r = diff_tables(&src, &dst, &keyed("id")).unwrap();
        assert_eq!(r.identical, 1, "{:?}", r.diffs);

        let strict = CompareOptions {
            tolerant_values: false,
            trim_trailing_space: false,
            ..keyed("id")
        };
        let src = table(&["id", "v"], &[&["1", "10.50"]]);
        let dst = table(&["id", "v"], &[&["1", "10.5"]]);
        assert_eq!(
            diff_tables(&src, &dst, &strict).unwrap().counts(),
            (0, 0, 1)
        );
    }

    #[test]
    fn null_differs_from_the_text_null_free_value_and_columns_can_be_ignored() {
        let src = table(&["id", "v", "ts"], &[&["1", "NULL", "x"]]);
        let dst = table(&["id", "v", "ts"], &[&["1", "", "y"]]);
        let mut opts = keyed("id");
        assert_eq!(
            diff_tables(&src, &dst, &opts).unwrap().diffs[0].changed,
            vec![1, 2]
        );
        opts.ignore_columns = vec!["TS".to_string(), "v".to_string()];
        assert_eq!(diff_tables(&src, &dst, &opts).unwrap().identical, 1);
    }

    fn explicit(headers: &[&str], rows: &[&[Option<&str>]]) -> TableData {
        TableData::from_cells(
            headers.iter().map(|s| s.to_string()).collect(),
            rows.iter()
                .map(|r| r.iter().map(|c| c.map(str::to_string)).collect())
                .collect(),
        )
    }

    #[test]
    fn explicit_null_differs_from_the_string_null() {
        // Sumber punya string `NULL`, tujuan SQL NULL (dan sebaliknya).
        let src = explicit(
            &["id", "v"],
            &[
                &[Some("1"), Some("NULL")],
                &[Some("2"), None],
                &[Some("3"), None],
                &[Some("NULL"), Some("k")],
            ],
        );
        let dst = explicit(
            &["id", "v"],
            &[
                &[Some("1"), None],
                &[Some("2"), Some("NULL")],
                &[Some("3"), None],
                &[None, Some("k")],
            ],
        );
        let r = diff_tables(&src, &dst, &keyed("id")).unwrap();
        assert_eq!(r.identical, 1);
        // Kunci string `NULL` dan kunci NULL adalah dua baris berbeda.
        assert_eq!(r.counts(), (1, 1, 2));
        let script = sync_script(
            &r,
            &DatabaseType::PostgreSQL,
            "\"t\"",
            &[ValueKind::Text, ValueKind::Text],
            SyncParts {
                delete_extra: true,
                ..Default::default()
            },
        );
        assert_eq!(
            script,
            "UPDATE \"t\" SET \"v\" = 'NULL' WHERE \"id\" = '1';\n\
             UPDATE \"t\" SET \"v\" = NULL WHERE \"id\" = '2';\n\
             INSERT INTO \"t\" (\"id\", \"v\") VALUES ('NULL', 'k');\n\
             DELETE FROM \"t\" WHERE \"id\" IS NULL;\n"
        );
        let changed = &r.diffs[0];
        assert_eq!(changed.source_value(1), Some("NULL"));
        assert_eq!(changed.target_value(1), None);
        // Teks tampilan tetap memakai penanda di kedua sisi.
        assert_eq!(changed.source[1], "NULL");
        assert_eq!(changed.target[1], "NULL");
    }

    #[test]
    fn duplicate_keys_and_hash_collisions_do_not_change_the_result() {
        let src = table(
            &["id", "grp", "v"],
            &[
                &["1", "a", "first"],
                &["1", "a", "second source duplicate"],
                &["2", "a", "x"],
                &["1", "b", "y"],
                &["9", "z", "only source"],
            ],
        );
        let dst = table(
            &["id", "grp", "v"],
            &[
                &["1", "a", "older target duplicate"],
                &["2", "a", "x"],
                &["1", "a", "first"],
                &["1.0", "b", "changed"],
                &["7", "q", "only target"],
                &["7", "q", "only target, last wins"],
            ],
        );
        let opts = CompareOptions {
            key_columns: vec!["id".to_string(), "grp".to_string()],
            ..Default::default()
        };
        let real = diff_tables(&src, &dst, &opts).unwrap();
        assert_eq!(real.duplicate_keys, 3);
        assert_eq!(real.identical, 2);
        assert_eq!(real.counts(), (1, 1, 1));
        // Baris sumber pertama dan baris tujuan terakhir per kunci yang dipakai.
        let only_target = real
            .diffs
            .iter()
            .find(|d| d.kind == DiffKind::OnlyInTarget)
            .unwrap();
        assert_eq!(only_target.target[2], "only target, last wins");
        let changed = real
            .diffs
            .iter()
            .find(|d| d.kind == DiffKind::Changed)
            .unwrap();
        assert_eq!(changed.source[2], "y");
        assert_eq!(changed.target[2], "changed");

        // Semua kunci jatuh ke satu ember (tabrakan total), lalu ke dua ember:
        // hasilnya harus sama persis karena kunci asli tetap dibandingkan.
        let one_bucket = diff_tables_with(&src, &dst, &opts, &|_| 7).unwrap();
        assert_eq!(one_bucket, real);
        let two_buckets = diff_tables_with(&src, &dst, &opts, &|key| key_hash(key) % 2).unwrap();
        assert_eq!(two_buckets, real);
        assert_ne!(
            key_hash(&[Some("a".into()), None]),
            key_hash(&[None, Some("a".into())])
        );
    }

    #[test]
    fn reports_one_sided_columns_and_duplicate_keys() {
        let src = table(
            &["id", "a", "only_src"],
            &[&["1", "x", "s"], &["1", "y", "s"]],
        );
        let dst = table(&["id", "a", "only_dst"], &[&["1", "x", "d"]]);
        let r = diff_tables(&src, &dst, &keyed("id")).unwrap();
        assert_eq!(r.source_only_columns, vec!["only_src"]);
        assert_eq!(r.target_only_columns, vec!["only_dst"]);
        assert_eq!(r.duplicate_keys, 1);
        assert_eq!(r.identical, 1);
        assert!(diff_tables(&src, &dst, &keyed("missing")).is_err());
        assert!(diff_tables(&src, &dst, &CompareOptions::default()).is_err());
    }

    #[test]
    fn sync_script_makes_target_match_source() {
        let src = table(
            &["id", "name"],
            &[&["1", "a"], &["2", "it's"], &["3", "NULL"]],
        );
        let dst = table(&["id", "name"], &[&["1", "old"], &["9", "gone"]]);
        let r = diff_tables(&src, &dst, &keyed("id")).unwrap();
        let kinds = [ValueKind::Number, ValueKind::Text];
        let all = SyncParts {
            delete_extra: true,
            ..Default::default()
        };
        let script = sync_script(&r, &DatabaseType::PostgreSQL, "\"t\"", &kinds, all);
        assert_eq!(
            script,
            "UPDATE \"t\" SET \"name\" = 'a' WHERE \"id\" = 1;\n\
             INSERT INTO \"t\" (\"id\", \"name\") VALUES (2, 'it''s');\n\
             INSERT INTO \"t\" (\"id\", \"name\") VALUES (3, NULL);\n\
             DELETE FROM \"t\" WHERE \"id\" = 9;\n"
        );
        let inserts_only = SyncParts {
            insert_missing: true,
            update_changed: false,
            delete_extra: false,
        };
        let script = sync_script(&r, &DatabaseType::PostgreSQL, "\"t\"", &kinds, inserts_only);
        assert_eq!(script.lines().count(), 2);
    }

    #[tokio::test]
    async fn compares_every_table_of_two_databases() {
        use crate::models::enums::DatabasePool;
        use crate::models::structs::ConnectionConfig;
        async fn database(sql: &str) -> Endpoint {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            let ep = Endpoint::new(
                ConnectionConfig {
                    connection_type: DatabaseType::SQLite,
                    ..Default::default()
                },
                Some(DatabasePool::SQLite(Arc::new(pool))),
                None,
            );
            ep.query(sql).await.unwrap();
            ep
        }
        let source = database(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, v TEXT); \
             CREATE TABLE orders (id INTEGER PRIMARY KEY, v TEXT); \
             CREATE TABLE notes (v TEXT); \
             CREATE TABLE src_only (id INTEGER PRIMARY KEY); \
             INSERT INTO users VALUES (1,'a'),(2,'b'); \
             INSERT INTO orders VALUES (1,'x'),(2,'y');",
        )
        .await;
        let target = database(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, v TEXT); \
             CREATE TABLE orders (id INTEGER PRIMARY KEY, v TEXT); \
             CREATE TABLE notes (v TEXT); \
             CREATE TABLE dst_only (id INTEGER PRIMARY KEY); \
             INSERT INTO users VALUES (1,'a'),(2,'b'); \
             INSERT INTO orders VALUES (1,'x');",
        )
        .await;
        let progress = CompareProgressHandle::default();
        let db = compare_databases(
            source,
            target,
            &CompareOptions::default(),
            progress.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
        let by_name = |name: &str| db.tables.iter().find(|t| t.source_table == name).unwrap();
        assert_eq!(db.tables.len(), 3);
        assert!(!by_name("users").differs());
        assert!(by_name("orders").differs());
        // Tanpa primary key: gagal untuk tabel itu saja.
        assert!(by_name("notes").outcome.is_err());
        assert_eq!(db.source_only, vec!["src_only"]);
        assert_eq!(db.target_only, vec!["dst_only"]);
        assert!(!db.cancelled);
        assert_eq!(progress.lock().unwrap().tables_done, 3);
    }

    #[tokio::test]
    async fn compares_sqlite_tables_using_primary_key() {
        use crate::models::enums::DatabasePool;
        use crate::models::structs::ConnectionConfig;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let ep = Endpoint::new(
            ConnectionConfig {
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(DatabasePool::SQLite(std::sync::Arc::new(pool))),
            None,
        );
        ep.query(
            "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT); \
             CREATE TABLE b (id INTEGER PRIMARY KEY, v TEXT); \
             INSERT INTO a VALUES (1,'x'),(2,'y'),(3,'z'); \
             INSERT INTO b VALUES (1,'x'),(2,'changed');",
        )
        .await
        .unwrap();
        let outcome = compare_tables(ep.clone(), ep.clone(), "a", "b", &CompareOptions::default())
            .await
            .unwrap();
        assert_eq!(outcome.key_columns, vec!["id"]);
        assert_eq!(outcome.result.counts(), (1, 0, 1));
        let statements = sync_statements(
            &outcome.result,
            &outcome.target_db,
            &outcome.target_sql,
            &outcome.kinds,
            SyncParts::default(),
        );
        assert_eq!(statements.len(), 2);
        ep.execute(&statements).await.unwrap();
        let again = compare_tables(ep.clone(), ep.clone(), "a", "b", &CompareOptions::default())
            .await
            .unwrap();
        assert_eq!(again.result.counts(), (0, 0, 0));
        assert_eq!(again.result.identical, 3);
        assert!(!again.result.truncated);

        let limited = CompareOptions {
            row_limit: Some(2),
            ..Default::default()
        };
        let partial = compare_tables(ep.clone(), ep, "a", "b", &limited)
            .await
            .unwrap();
        assert!(partial.result.truncated);
        assert_eq!(partial.result.source_rows, 2);
    }
}
