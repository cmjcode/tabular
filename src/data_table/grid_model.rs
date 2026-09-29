//! Logika murni fitur data grid (bagian B checklist TablePro).
//!
//! Semua fungsi di sini tidak menyentuh `Tabular` maupun egui sehingga mudah
//! diuji: nilai SQL mentah (DEFAULT/NOW), penanda karakter tak terlihat,
//! evaluasi highlight rule, pencarian di hasil, pembangun WHERE pencarian
//! server-side, SQL lookup foreign key, urutan kolom, dan helper antrean edit.

use crate::models::enums::DatabaseType;
use crate::models::structs::{CellEditOperation, FilterOperator};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

// ─── Nilai SQL mentah (B12) ────────────────────────────────────────────────

/// Penanda di awal nilai sel yang berarti "ekspresi SQL, jangan di-quote".
/// Karakter kontrol ini tidak mungkin diketik user di editor sel.
pub const RAW_SQL_MARK: char = '\u{1}';

pub const RAW_DEFAULT: &str = "DEFAULT";
pub const RAW_NOW: &str = "CURRENT_TIMESTAMP";
pub const RAW_EMPTY_STRING: &str = "''";

pub fn raw_sql_value(expr: &str) -> String {
    format!("{RAW_SQL_MARK}{expr}")
}

/// Ekspresi SQL bila `value` adalah nilai mentah.
pub fn as_raw_sql(value: &str) -> Option<&str> {
    value.strip_prefix(RAW_SQL_MARK)
}

pub fn is_raw_default(value: &str) -> bool {
    as_raw_sql(value) == Some(RAW_DEFAULT)
}

/// Teks yang ditampilkan di grid untuk sebuah nilai sel.
pub fn display_value(value: &str) -> Cow<'_, str> {
    match as_raw_sql(value) {
        Some(RAW_NOW) => Cow::Borrowed("NOW()"),
        Some(expr) => Cow::Borrowed(expr),
        None => Cow::Borrowed(value),
    }
}

/// Pilihan cepat "Set Value" di context menu sel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickValue {
    Null,
    EmptyString,
    Default,
    Now,
    Uuid,
}

impl QuickValue {
    pub fn label(self) -> &'static str {
        match self {
            QuickValue::Null => "NULL",
            QuickValue::EmptyString => "Empty string ('')",
            QuickValue::Default => "DEFAULT",
            QuickValue::Now => "NOW() (current timestamp)",
            QuickValue::Uuid => "UUID (random v4)",
        }
    }

    /// Nilai sel yang disimpan di grid untuk pilihan ini.
    pub fn cell_value(self) -> String {
        match self {
            QuickValue::Null => "NULL".to_string(),
            QuickValue::EmptyString => raw_sql_value(RAW_EMPTY_STRING),
            QuickValue::Default => raw_sql_value(RAW_DEFAULT),
            QuickValue::Now => raw_sql_value(RAW_NOW),
            QuickValue::Uuid => new_uuid_v4(),
        }
    }
}

/// `UPDATE ... SET c = DEFAULT` tidak didukung SQLite; di INSERT kolom DEFAULT
/// cukup dihilangkan dari daftar kolom sehingga berlaku untuk semua engine.
pub fn default_allowed_in_update(db: &DatabaseType) -> bool {
    !matches!(db, DatabaseType::SQLite)
}

/// UUID v4 acak dalam format kanonik (lowercase, dengan tanda hubung).
pub fn new_uuid_v4() -> String {
    use rand::RngExt;
    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Quote literal string sesuai dialek. `NULL` / kosong menjadi `NULL`
/// (konvensi grid yang sudah ada), nilai mentah dikeluarkan apa adanya.
pub fn quote_literal(db: &DatabaseType, value: &str) -> String {
    if let Some(expr) = as_raw_sql(value) {
        return expr.to_string();
    }
    if value.is_empty() || value.eq_ignore_ascii_case("null") {
        return "NULL".to_string();
    }
    quote_text_literal(db, value)
}

/// Quote teks tanpa interpretasi NULL (dipakai untuk pola pencarian).
fn quote_text_literal(db: &DatabaseType, value: &str) -> String {
    match db {
        // MySQL memperlakukan backslash sebagai escape secara default.
        DatabaseType::MySQL => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''")),
        DatabaseType::MsSQL => format!("N'{}'", value.replace('\'', "''")),
        _ => format!("'{}'", value.replace('\'', "''")),
    }
}

// ─── Karakter tak terlihat (B8) ────────────────────────────────────────────

/// Simbol pengganti untuk karakter yang tidak terlihat. Semua simbol berada di
/// blok Latin-1 agar tersedia di font Proportional maupun Monospace egui.
pub fn invisible_marker(c: char) -> Option<&'static str> {
    match c {
        '\t' => Some("»"),
        '\n' => Some("¶"),
        '\r' => Some("¬"),
        '\u{a0}' | '\u{202f}' | '\u{2007}' => Some("·"),
        '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}' | '\u{feff}' | '\u{00ad}' => Some("¦"),
        c if c.is_control() && c != RAW_SQL_MARK => Some("¤"),
        _ => None,
    }
}

/// Karakter tak terlihat yang patut diwaspadai (newline dianggap wajar).
pub fn is_suspicious_invisible(c: char) -> bool {
    c != '\n' && invisible_marker(c).is_some()
}

pub fn has_invisible(text: &str) -> bool {
    text.chars().any(|c| invisible_marker(c).is_some())
}

/// Pecah teks menjadi potongan (teks, apakah_penanda) untuk dirender dengan
/// warna berbeda.
pub fn segment_invisibles(text: &str) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for c in text.chars() {
        match invisible_marker(c) {
            Some(marker) => out.push((marker.to_string(), true)),
            None => match out.last_mut() {
                Some((buf, false)) => buf.push(c),
                _ => out.push((c.to_string(), false)),
            },
        }
    }
    out
}

/// Indeks kolom yang memuat karakter tak terlihat mencurigakan, beserta
/// jumlah sel yang terdampak.
pub fn columns_with_suspicious_invisibles(rows: &[Vec<String>]) -> HashMap<usize, usize> {
    let mut out: HashMap<usize, usize> = HashMap::new();
    for row in rows {
        for (ci, cell) in row.iter().enumerate() {
            if as_raw_sql(cell).is_none() && cell.chars().any(is_suspicious_invisible) {
                *out.entry(ci).or_default() += 1;
            }
        }
    }
    out
}

// ─── Highlight rules (B7) ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HighlightScope {
    #[default]
    Row,
    Cell,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HighlightColor {
    Red,
    Orange,
    #[default]
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Gray,
}

impl HighlightColor {
    pub fn all() -> &'static [HighlightColor] {
        &[
            HighlightColor::Red,
            HighlightColor::Orange,
            HighlightColor::Yellow,
            HighlightColor::Green,
            HighlightColor::Teal,
            HighlightColor::Blue,
            HighlightColor::Purple,
            HighlightColor::Gray,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            HighlightColor::Red => "Red",
            HighlightColor::Orange => "Orange",
            HighlightColor::Yellow => "Yellow",
            HighlightColor::Green => "Green",
            HighlightColor::Teal => "Teal",
            HighlightColor::Blue => "Blue",
            HighlightColor::Purple => "Purple",
            HighlightColor::Gray => "Gray",
        }
    }

    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            HighlightColor::Red => (239, 68, 68),
            HighlightColor::Orange => (249, 115, 22),
            HighlightColor::Yellow => (234, 179, 8),
            HighlightColor::Green => (34, 197, 94),
            HighlightColor::Teal => (20, 184, 166),
            HighlightColor::Blue => (59, 130, 246),
            HighlightColor::Purple => (168, 85, 247),
            HighlightColor::Gray => (120, 120, 130),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HighlightRule {
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub column: String,
    pub operator: FilterOperator,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub color: HighlightColor,
    #[serde(default)]
    pub scope: HighlightScope,
}

fn default_true() -> bool {
    true
}

impl HighlightRule {
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            enabled: true,
            column: column.into(),
            operator: FilterOperator::Equal,
            value: String::new(),
            color: HighlightColor::default(),
            scope: HighlightScope::default(),
        }
    }
}

/// Operator yang bisa dievaluasi di sisi klien untuk highlight rule.
pub fn highlight_operators() -> &'static [FilterOperator] {
    &[
        FilterOperator::Equal,
        FilterOperator::NotEqual,
        FilterOperator::Contains,
        FilterOperator::StartsWith,
        FilterOperator::EndsWith,
        FilterOperator::Like,
        FilterOperator::GreaterThan,
        FilterOperator::LessThan,
        FilterOperator::GreaterThanOrEqual,
        FilterOperator::LessThanOrEqual,
        FilterOperator::Between,
        FilterOperator::In,
        FilterOperator::IsNull,
        FilterOperator::IsNotNull,
    ]
}

pub fn is_null_cell(cell: &str) -> bool {
    cell.eq_ignore_ascii_case("null")
}

/// Bandingkan secara numerik bila keduanya angka, selain itu leksikografis
/// case-insensitive (tanggal ISO tetap terurut benar).
fn compare_values(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

fn values_equal(a: &str, b: &str) -> bool {
    compare_values(a, b) == std::cmp::Ordering::Equal
}

/// Pencocokan pola LIKE (`%`, `_`) case-insensitive.
pub fn like_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    // Pemrograman dinamis sederhana; pola dan sel pendek.
    let mut dp = vec![vec![false; t.len() + 1]; p.len() + 1];
    dp[0][0] = true;
    for i in 1..=p.len() {
        if p[i - 1] == '%' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=p.len() {
        for j in 1..=t.len() {
            dp[i][j] = match p[i - 1] {
                '%' => dp[i - 1][j] || dp[i][j - 1],
                '_' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == t[j - 1],
            };
        }
    }
    dp[p.len()][t.len()]
}

fn split_range(value: &str) -> Option<(&str, &str)> {
    for sep in [" AND ", " and ", ","] {
        if let Some((a, b)) = value.split_once(sep) {
            return Some((a.trim(), b.trim()));
        }
    }
    None
}

/// Apakah nilai sel memenuhi operator + nilai pembanding.
pub fn rule_matches(op: FilterOperator, expected: &str, cell: &str) -> bool {
    let cell = display_value(cell);
    let cell = cell.as_ref();
    let expected = expected.trim();
    let null = is_null_cell(cell);
    match op {
        FilterOperator::IsNull => null,
        FilterOperator::IsNotNull => !null,
        _ if null => false,
        FilterOperator::Equal | FilterOperator::Equals => values_equal(cell, expected),
        FilterOperator::NotEqual | FilterOperator::NotEquals => !values_equal(cell, expected),
        FilterOperator::Contains => cell.to_lowercase().contains(&expected.to_lowercase()),
        FilterOperator::StartsWith => cell.to_lowercase().starts_with(&expected.to_lowercase()),
        FilterOperator::EndsWith => cell.to_lowercase().ends_with(&expected.to_lowercase()),
        FilterOperator::Like | FilterOperator::ILike => like_matches(expected, cell),
        FilterOperator::GreaterThan => compare_values(cell, expected).is_gt(),
        FilterOperator::LessThan => compare_values(cell, expected).is_lt(),
        FilterOperator::GreaterThanOrEqual => compare_values(cell, expected).is_ge(),
        FilterOperator::LessThanOrEqual => compare_values(cell, expected).is_le(),
        FilterOperator::Between => match split_range(expected) {
            Some((lo, hi)) => compare_values(cell, lo).is_ge() && compare_values(cell, hi).is_le(),
            None => values_equal(cell, expected),
        },
        FilterOperator::In => expected
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .any(|v| values_equal(cell, v)),
    }
}

/// Rule aktif yang kolomnya ada di header, dengan indeks kolomnya.
pub fn resolve_rules<'a>(
    rules: &'a [HighlightRule],
    headers: &[String],
) -> Vec<(usize, &'a HighlightRule)> {
    rules
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| {
            headers
                .iter()
                .position(|h| h.eq_ignore_ascii_case(&r.column))
                .map(|i| (i, r))
        })
        .collect()
}

/// Warna baris dan warna per sel untuk satu baris. Rule pertama yang cocok
/// menang untuk tiap target.
pub fn evaluate_row_highlight(
    resolved: &[(usize, &HighlightRule)],
    row: &[String],
) -> (Option<HighlightColor>, Vec<(usize, HighlightColor)>) {
    let mut row_color = None;
    let mut cells: Vec<(usize, HighlightColor)> = Vec::new();
    for (col, rule) in resolved {
        let Some(cell) = row.get(*col) else { continue };
        if !rule_matches(rule.operator, &rule.value, cell) {
            continue;
        }
        match rule.scope {
            HighlightScope::Row => {
                if row_color.is_none() {
                    row_color = Some(rule.color);
                }
            }
            HighlightScope::Cell => {
                if !cells.iter().any(|(c, _)| c == col) {
                    cells.push((*col, rule.color));
                }
            }
        }
    }
    (row_color, cells)
}

// ─── Find in results (B4) ──────────────────────────────────────────────────

/// Posisi (baris, kolom) sel yang memuat `needle`, urut baris lalu kolom.
pub fn find_matches(
    rows: &[Vec<String>],
    needle: &str,
    case_sensitive: bool,
    skip_column: impl Fn(usize) -> bool,
) -> Vec<(usize, usize)> {
    if needle.is_empty() {
        return Vec::new();
    }
    let needle_cmp = if case_sensitive {
        needle.to_string()
    } else {
        needle.to_lowercase()
    };
    let mut out = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        for (ci, cell) in row.iter().enumerate() {
            if skip_column(ci) {
                continue;
            }
            let shown = display_value(cell);
            let hit = if case_sensitive {
                shown.contains(needle_cmp.as_str())
            } else {
                shown.to_lowercase().contains(needle_cmp.as_str())
            };
            if hit {
                out.push((ri, ci));
            }
        }
    }
    out
}

/// Escape pola LIKE dengan karakter escape `!` (didukung klausa `ESCAPE`
/// di keempat engine SQL).
fn escape_like(db: &DatabaseType, needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 4);
    for c in needle.chars() {
        match c {
            '!' | '%' | '_' => {
                out.push('!');
                out.push(c);
            }
            '[' if matches!(db, DatabaseType::MsSQL) => out.push_str("!["),
            _ => out.push(c),
        }
    }
    out
}

/// Ekspresi teks sebuah kolom untuk perbandingan LIKE lintas tipe.
fn text_cast(db: &DatabaseType, quoted_col: &str) -> String {
    match db {
        DatabaseType::PostgreSQL => format!("CAST({quoted_col} AS TEXT)"),
        DatabaseType::MySQL => format!("CAST({quoted_col} AS CHAR)"),
        DatabaseType::MsSQL => format!("CAST({quoted_col} AS NVARCHAR(MAX))"),
        _ => format!("CAST({quoted_col} AS TEXT)"),
    }
}

/// Satu kondisi "kolom mengandung teks" case-insensitive.
fn contains_condition(db: &DatabaseType, column: &str, needle: &str) -> String {
    let q = super::quote_identifier(column, db);
    let pattern = quote_text_literal(db, &format!("%{}%", escape_like(db, needle)));
    match db {
        DatabaseType::PostgreSQL => format!("{} ILIKE {} ESCAPE '!'", text_cast(db, &q), pattern),
        _ => format!(
            "LOWER({}) LIKE LOWER({}) ESCAPE '!'",
            text_cast(db, &q),
            pattern
        ),
    }
}

/// WHERE untuk "Search All Rows": teks dicari di semua kolom di server.
pub fn build_search_all_where(
    columns: &[String],
    needle: &str,
    db: &DatabaseType,
) -> Option<String> {
    let needle = needle.trim();
    if needle.is_empty() || columns.is_empty() {
        return None;
    }
    let parts: Vec<String> = columns
        .iter()
        .filter(|c| !c.trim().is_empty())
        .map(|c| contains_condition(db, c, needle))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(format!("({})", parts.join(" OR ")))
    }
}

// ─── Lookup foreign key (B10, B11) ─────────────────────────────────────────

/// Nama tabel lengkap untuk query lookup.
pub fn qualified_table(
    db: &DatabaseType,
    database: Option<&str>,
    schema: Option<&str>,
    table: &str,
) -> String {
    let q = |s: &str| super::quote_identifier(s, db);
    let database = database.filter(|d| !d.trim().is_empty());
    let schema = schema.filter(|s| !s.trim().is_empty());
    match db {
        DatabaseType::MySQL => match database {
            Some(d) => format!("{}.{}", q(d), q(table)),
            None => q(table),
        },
        DatabaseType::PostgreSQL => match schema {
            Some(s) => format!("{}.{}", q(s), q(table)),
            None => q(table),
        },
        DatabaseType::MsSQL => match (database, schema) {
            (Some(d), Some(s)) => format!("{}.{}.{}", q(d), q(s), q(table)),
            (Some(d), None) => format!("{}..{}", q(d), q(table)),
            (None, Some(s)) => format!("{}.{}", q(s), q(table)),
            (None, None) => q(table),
        },
        _ => q(table),
    }
}

fn select_with_limit(
    db: &DatabaseType,
    from: &str,
    where_sql: &str,
    order: &str,
    limit: usize,
) -> String {
    let where_part = if where_sql.is_empty() {
        String::new()
    } else {
        format!(" WHERE {where_sql}")
    };
    let order_part = if order.is_empty() {
        String::new()
    } else {
        format!(" ORDER BY {order}")
    };
    match db {
        DatabaseType::MsSQL => format!("SELECT TOP {limit} * FROM {from}{where_part}{order_part}"),
        _ => format!("SELECT * FROM {from}{where_part}{order_part} LIMIT {limit}"),
    }
}

/// Ambil baris induk yang dirujuk sebuah nilai foreign key.
pub fn build_fk_row_lookup_sql(
    db: &DatabaseType,
    qualified: &str,
    column: &str,
    value: &str,
) -> String {
    let q = super::quote_identifier(column, db);
    let cond = if is_null_cell(value) {
        format!("{q} IS NULL")
    } else {
        format!("{q} = {}", quote_literal(db, value))
    };
    select_with_limit(db, qualified, &cond, "", 1)
}

/// Daftar kandidat nilai untuk FK picker, disaring `needle` pada kolom kunci
/// dan kolom label.
pub fn build_fk_picker_sql(
    db: &DatabaseType,
    qualified: &str,
    key_column: &str,
    search_columns: &[String],
    needle: &str,
    limit: usize,
) -> String {
    let mut cols: Vec<String> = vec![key_column.to_string()];
    for c in search_columns {
        if !cols.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            cols.push(c.clone());
        }
    }
    let where_sql = build_search_all_where(&cols, needle, db).unwrap_or_default();
    let order = super::quote_identifier(key_column, db);
    select_with_limit(db, qualified, &where_sql, &order, limit)
}

/// Pilih kolom label yang paling deskriptif untuk FK picker.
pub fn pick_label_columns(headers: &[String], key_column: &str, max: usize) -> Vec<String> {
    const PREFERRED: &[&str] = &[
        "name",
        "title",
        "label",
        "display_name",
        "full_name",
        "username",
        "email",
        "code",
        "slug",
        "description",
    ];
    let mut out: Vec<String> = Vec::new();
    for pref in PREFERRED {
        if let Some(h) = headers.iter().find(|h| h.eq_ignore_ascii_case(pref)) {
            if !h.eq_ignore_ascii_case(key_column) && !out.contains(h) {
                out.push(h.clone());
            }
        }
    }
    for pref in PREFERRED {
        for h in headers {
            let lower = h.to_lowercase();
            if lower.contains(pref) && !h.eq_ignore_ascii_case(key_column) && !out.contains(h) {
                out.push(h.clone());
            }
        }
    }
    for h in headers {
        if !h.eq_ignore_ascii_case(key_column) && !out.contains(h) {
            out.push(h.clone());
        }
    }
    out.truncate(max);
    out
}

// ─── Kolom: urutan, sembunyi, fuzzy jump (B9, B13) ─────────────────────────

/// Urutan tampil indeks kolom: kolom pinned dulu, lalu sisanya. Bila ada
/// urutan kustom, kedua kelompok mengikuti urutan itu. Kolom tersembunyi
/// dibuang.
pub fn display_order(
    headers: &[String],
    pinned: &HashSet<String>,
    hidden: &HashSet<String>,
    custom_order: Option<&[String]>,
) -> Vec<usize> {
    let mut base: Vec<usize> = (0..headers.len()).collect();
    if let Some(order) = custom_order {
        let rank = |i: &usize| {
            order
                .iter()
                .position(|n| n == &headers[*i])
                .unwrap_or(order.len() + *i)
        };
        base.sort_by_key(rank);
    }
    let visible = |i: &&usize| !hidden.contains(&headers[**i]);
    let mut out: Vec<usize> = base
        .iter()
        .filter(visible)
        .filter(|i| pinned.contains(&headers[**i]))
        .copied()
        .collect();
    out.extend(
        base.iter()
            .filter(visible)
            .filter(|i| !pinned.contains(&headers[**i])),
    );
    out
}

/// Pindahkan kolom `moving` ke posisi sebelum/sesudah `target` dalam urutan
/// nama kolom. Urutan kosong diisi dari `headers` terlebih dulu.
pub fn move_column(
    order: &mut Vec<String>,
    headers: &[String],
    moving: &str,
    target: &str,
    after: bool,
) {
    if order.len() != headers.len() || headers.iter().any(|h| !order.contains(h)) {
        let mut rebuilt: Vec<String> = order
            .iter()
            .filter(|n| headers.contains(n))
            .cloned()
            .collect();
        for h in headers {
            if !rebuilt.contains(h) {
                rebuilt.push(h.clone());
            }
        }
        *order = rebuilt;
    }
    if moving == target {
        return;
    }
    let Some(from) = order.iter().position(|n| n == moving) else {
        return;
    };
    let item = order.remove(from);
    let Some(to) = order.iter().position(|n| n == target) else {
        order.insert(from, item);
        return;
    };
    let insert_at = if after { to + 1 } else { to };
    order.insert(insert_at, item);
}

/// Skor fuzzy subsequence (lebih tinggi lebih baik), `None` bila tidak cocok.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    let q: Vec<char> = query.trim().to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let c: Vec<char> = candidate.to_lowercase().chars().collect();
    let mut score = 0i32;
    let mut qi = 0usize;
    let mut prev_match: Option<usize> = None;
    for (ci, ch) in c.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            score += 1;
            if prev_match.is_some_and(|p| p + 1 == ci) {
                score += 5;
            }
            if ci == 0 || matches!(c[ci - 1], '_' | ' ' | '.' | '-') {
                score += 8;
            }
            prev_match = Some(ci);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    let joined: String = q.iter().collect();
    let lower: String = c.iter().collect();
    if lower == joined {
        score += 100;
    } else if lower.starts_with(&joined) {
        score += 50;
    } else if lower.contains(&joined) {
        score += 25;
    }
    Some(score - (c.len() as i32 - q.len() as i32).max(0) / 4)
}

/// Ambil definisi kolom dari `SHOW CREATE TABLE` MySQL, tanpa koma penutup.
pub fn extract_mysql_column_definition(create_sql: &str, column: &str) -> Option<String> {
    let prefix = format!("`{}`", column.replace('`', "``"));
    create_sql.lines().map(str::trim).find_map(|line| {
        line.starts_with(&prefix)
            .then(|| line.trim_end_matches(',').trim().to_string())
    })
}

/// `ALTER TABLE ... MODIFY COLUMN ... FIRST|AFTER` untuk memindah kolom fisik.
pub fn build_mysql_move_column_sql(
    qualified_table: &str,
    definition: &str,
    after: Option<&str>,
) -> String {
    let position = match after {
        Some(col) => format!("AFTER `{}`", col.replace('`', "``")),
        None => "FIRST".to_string(),
    };
    format!("ALTER TABLE {qualified_table} MODIFY COLUMN {definition} {position}")
}

// ─── Antrean edit (B1, B2) ─────────────────────────────────────────────────

fn op_row_mut(op: &mut CellEditOperation) -> &mut usize {
    match op {
        CellEditOperation::Update { row_index, .. }
        | CellEditOperation::InsertRow { row_index, .. }
        | CellEditOperation::DeleteRow { row_index, .. } => row_index,
    }
}

pub fn op_row(op: &CellEditOperation) -> usize {
    match op {
        CellEditOperation::Update { row_index, .. }
        | CellEditOperation::InsertRow { row_index, .. }
        | CellEditOperation::DeleteRow { row_index, .. } => *row_index,
    }
}

/// Geser indeks baris operasi setelah baris disisipkan (`delta` positif) atau
/// dibuang (`delta` negatif) di posisi `from`.
pub fn shift_op_rows(ops: &mut [CellEditOperation], from: usize, delta: isize) {
    for op in ops.iter_mut() {
        let row = op_row_mut(op);
        if *row >= from {
            *row = row.saturating_add_signed(delta);
        }
    }
}

pub fn pending_deleted_rows(ops: &[CellEditOperation]) -> HashSet<usize> {
    ops.iter()
        .filter_map(|op| match op {
            CellEditOperation::DeleteRow { row_index, .. } => Some(*row_index),
            _ => None,
        })
        .collect()
}

pub fn pending_inserted_rows(ops: &[CellEditOperation]) -> HashSet<usize> {
    ops.iter()
        .filter_map(|op| match op {
            CellEditOperation::InsertRow { row_index, .. } => Some(*row_index),
            _ => None,
        })
        .collect()
}

/// Sel yang sudah diubah beserta nilai aslinya (nilai lama pertama).
pub fn pending_updated_cells(ops: &[CellEditOperation]) -> HashMap<(usize, usize), String> {
    let mut out: HashMap<(usize, usize), String> = HashMap::new();
    for op in ops {
        if let CellEditOperation::Update {
            row_index,
            col_index,
            old_value,
            ..
        } = op
        {
            out.entry((*row_index, *col_index))
                .or_insert_with(|| old_value.clone());
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpSummary {
    pub updates: usize,
    pub inserts: usize,
    pub deletes: usize,
}

impl OpSummary {
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.updates > 0 {
            parts.push(format!("{} updated cell(s)", self.updates));
        }
        if self.inserts > 0 {
            parts.push(format!("{} new row(s)", self.inserts));
        }
        if self.deletes > 0 {
            parts.push(format!("{} deleted row(s)", self.deletes));
        }
        if parts.is_empty() {
            "no changes".to_string()
        } else {
            parts.join(", ")
        }
    }
}

pub fn summarize_ops(ops: &[CellEditOperation]) -> OpSummary {
    let mut s = OpSummary::default();
    let mut cells: HashSet<(usize, usize)> = HashSet::new();
    for op in ops {
        match op {
            CellEditOperation::Update {
                row_index,
                col_index,
                ..
            } => {
                if cells.insert((*row_index, *col_index)) {
                    s.updates += 1;
                }
            }
            CellEditOperation::InsertRow { .. } => s.inserts += 1,
            CellEditOperation::DeleteRow { .. } => s.deletes += 1,
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn nilai_mentah_tidak_di_quote() {
        let db = DatabaseType::PostgreSQL;
        assert_eq!(quote_literal(&db, &raw_sql_value(RAW_DEFAULT)), "DEFAULT");
        assert_eq!(
            quote_literal(&db, &raw_sql_value(RAW_NOW)),
            "CURRENT_TIMESTAMP"
        );
        assert_eq!(quote_literal(&db, &raw_sql_value(RAW_EMPTY_STRING)), "''");
        assert_eq!(quote_literal(&db, "NULL"), "NULL");
        assert_eq!(quote_literal(&db, "O'Brien"), "'O''Brien'");
        assert_eq!(
            quote_literal(&DatabaseType::MySQL, "a\\"),
            "'a\\\\'",
            "backslash MySQL harus di-escape"
        );
        assert_eq!(display_value(&raw_sql_value(RAW_NOW)), "NOW()");
        assert!(is_raw_default(&QuickValue::Default.cell_value()));
    }

    #[test]
    fn uuid_v4_berformat_kanonik() {
        let u = new_uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(u, new_uuid_v4());
    }

    #[test]
    fn penanda_karakter_tak_terlihat() {
        let segs = segment_invisibles("a\tb\u{200b}c");
        assert_eq!(
            segs,
            vec![
                ("a".to_string(), false),
                ("»".to_string(), true),
                ("b".to_string(), false),
                ("¦".to_string(), true),
                ("c".to_string(), false),
            ]
        );
        assert!(!is_suspicious_invisible('\n'));
        assert!(is_suspicious_invisible('\u{a0}'));
        assert_eq!(
            "x\r\n\u{feff}"
                .chars()
                .filter(|c| is_suspicious_invisible(*c))
                .count(),
            2
        );
        let rows = vec![s(&["ok", "x\u{a0}y"]), s(&["a\tb", "fine"])];
        let cols = columns_with_suspicious_invisibles(&rows);
        assert_eq!(cols.get(&0), Some(&1));
        assert_eq!(cols.get(&1), Some(&1));
        // Penanda nilai mentah tidak dianggap karakter tersembunyi.
        assert!(columns_with_suspicious_invisibles(&[vec![raw_sql_value("DEFAULT")]]).is_empty());
    }

    #[test]
    fn evaluasi_operator_rule() {
        assert!(rule_matches(FilterOperator::GreaterThan, "10", "11.5"));
        assert!(!rule_matches(FilterOperator::GreaterThan, "10", "9"));
        assert!(rule_matches(FilterOperator::Equal, "ACTIVE", "active"));
        assert!(rule_matches(FilterOperator::Contains, "err", "Some ERROR"));
        assert!(rule_matches(FilterOperator::IsNull, "", "NULL"));
        assert!(!rule_matches(FilterOperator::Equal, "NULL", "NULL"));
        assert!(rule_matches(FilterOperator::Between, "1, 5", "3"));
        assert!(rule_matches(FilterOperator::In, "a, b", "B"));
        assert!(rule_matches(FilterOperator::Like, "a_c%", "abcdef"));
        assert!(!rule_matches(FilterOperator::Like, "a_c", "abcd"));
        assert!(rule_matches(
            FilterOperator::LessThan,
            "2024-01-01",
            "2023-12-31 23:00:00"
        ));
    }

    #[test]
    fn highlight_baris_dan_sel() {
        let headers = s(&["id", "status", "amount"]);
        let mut row_rule = HighlightRule::new("STATUS");
        row_rule.value = "failed".into();
        row_rule.color = HighlightColor::Red;
        let mut cell_rule = HighlightRule::new("amount");
        cell_rule.operator = FilterOperator::GreaterThan;
        cell_rule.value = "100".into();
        cell_rule.scope = HighlightScope::Cell;
        cell_rule.color = HighlightColor::Green;
        let mut disabled = HighlightRule::new("id");
        disabled.enabled = false;
        let rules = vec![row_rule, cell_rule, disabled];
        let resolved = resolve_rules(&rules, &headers);
        assert_eq!(resolved.len(), 2);
        let (row, cells) = evaluate_row_highlight(&resolved, &s(&["1", "failed", "250"]));
        assert_eq!(row, Some(HighlightColor::Red));
        assert_eq!(cells, vec![(2, HighlightColor::Green)]);
        let (row, cells) = evaluate_row_highlight(&resolved, &s(&["2", "ok", "5"]));
        assert!(row.is_none() && cells.is_empty());
    }

    #[test]
    fn rule_bisa_diserialisasi() {
        let rule = HighlightRule::new("x");
        let json = serde_json::to_string(&vec![rule.clone()]).unwrap();
        let back: Vec<HighlightRule> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, vec![rule]);
    }

    #[test]
    fn find_di_hasil() {
        let rows = vec![s(&["Alice", "alpha"]), s(&["Bob", "ALPHABET"])];
        assert_eq!(
            find_matches(&rows, "alp", false, |_| false),
            vec![(0, 1), (1, 1)]
        );
        assert_eq!(find_matches(&rows, "alp", true, |_| false), vec![(0, 1)]);
        assert_eq!(find_matches(&rows, "a", false, |c| c == 1), vec![(0, 0)]);
        assert!(find_matches(&rows, "", false, |_| false).is_empty());
    }

    #[test]
    fn where_pencarian_semua_kolom() {
        let cols = s(&["id", "name"]);
        let pg = build_search_all_where(&cols, "50%_o'k", &DatabaseType::PostgreSQL).unwrap();
        assert_eq!(
            pg,
            "(CAST(\"id\" AS TEXT) ILIKE '%50!%!_o''k%' ESCAPE '!' OR CAST(\"name\" AS TEXT) ILIKE '%50!%!_o''k%' ESCAPE '!')"
        );
        let my = build_search_all_where(&cols, "x", &DatabaseType::MySQL).unwrap();
        assert!(my.contains("LOWER(CAST(`id` AS CHAR)) LIKE LOWER('%x%') ESCAPE '!'"));
        let ms = build_search_all_where(&cols, "[a]", &DatabaseType::MsSQL).unwrap();
        assert!(ms.contains("CAST([id] AS NVARCHAR(MAX))"));
        assert!(ms.contains("N'%![a]%'"));
        assert!(build_search_all_where(&cols, "  ", &DatabaseType::SQLite).is_none());
    }

    #[test]
    fn sql_lookup_foreign_key() {
        let q = qualified_table(&DatabaseType::MySQL, Some("shop"), None, "users");
        assert_eq!(q, "`shop`.`users`");
        assert_eq!(
            build_fk_row_lookup_sql(&DatabaseType::MySQL, &q, "id", "7"),
            "SELECT * FROM `shop`.`users` WHERE `id` = '7' LIMIT 1"
        );
        let ms = qualified_table(&DatabaseType::MsSQL, Some("db"), None, "t");
        assert_eq!(ms, "[db]..[t]");
        assert_eq!(
            build_fk_row_lookup_sql(&DatabaseType::MsSQL, &ms, "id", "7"),
            "SELECT TOP 1 * FROM [db]..[t] WHERE [id] = N'7'"
        );
        let pg = qualified_table(&DatabaseType::PostgreSQL, Some("app"), Some("sales"), "t");
        assert_eq!(pg, "\"sales\".\"t\"");
        let picker =
            build_fk_picker_sql(&DatabaseType::SQLite, "\"t\"", "id", &s(&["name"]), "", 50);
        assert_eq!(picker, "SELECT * FROM \"t\" ORDER BY \"id\" LIMIT 50");
        let picker = build_fk_picker_sql(
            &DatabaseType::SQLite,
            "\"t\"",
            "id",
            &s(&["name"]),
            "bo",
            50,
        );
        assert!(picker.contains("WHERE (LOWER(CAST(\"id\" AS TEXT))"));
        assert!(picker.ends_with("ORDER BY \"id\" LIMIT 50"));
    }

    #[test]
    fn kolom_label_fk_picker() {
        let headers = s(&["id", "created_at", "email", "full_name"]);
        assert_eq!(
            pick_label_columns(&headers, "id", 2),
            s(&["full_name", "email"])
        );
        let headers = s(&["id", "x", "y"]);
        assert_eq!(pick_label_columns(&headers, "id", 2), s(&["x", "y"]));
    }

    #[test]
    fn urutan_kolom_pin_hide_custom() {
        let headers = s(&["a", "b", "c", "d"]);
        let pinned: HashSet<String> = ["c".to_string()].into();
        let hidden: HashSet<String> = ["b".to_string()].into();
        assert_eq!(
            display_order(&headers, &pinned, &hidden, None),
            vec![2, 0, 3]
        );
        let custom = s(&["d", "c", "b", "a"]);
        assert_eq!(
            display_order(&headers, &HashSet::new(), &HashSet::new(), Some(&custom)),
            vec![3, 2, 1, 0]
        );
    }

    #[test]
    fn pindah_kolom() {
        let headers = s(&["a", "b", "c"]);
        let mut order = Vec::new();
        move_column(&mut order, &headers, "a", "c", true);
        assert_eq!(order, s(&["b", "c", "a"]));
        move_column(&mut order, &headers, "a", "b", false);
        assert_eq!(order, s(&["a", "b", "c"]));
    }

    #[test]
    fn skor_fuzzy() {
        assert!(fuzzy_score("cid", "customer_id").is_some());
        assert!(fuzzy_score("xyz", "customer_id").is_none());
        let exact = fuzzy_score("email", "email").unwrap();
        let partial = fuzzy_score("email", "email_verified_at").unwrap();
        assert!(exact > partial);
        // Huruf di awal kata (c-ustomer_i-d) lebih bernilai daripada sebaran acak.
        assert!(fuzzy_score("cid", "customer_id") > fuzzy_score("cid", "city_count_id"));
    }

    #[test]
    fn definisi_kolom_mysql() {
        let ddl = "CREATE TABLE `t` (\n  `id` int NOT NULL AUTO_INCREMENT,\n  `name` varchar(20) DEFAULT 'x' COMMENT 'n',\n  PRIMARY KEY (`id`)\n)";
        let def = extract_mysql_column_definition(ddl, "name").unwrap();
        assert_eq!(def, "`name` varchar(20) DEFAULT 'x' COMMENT 'n'");
        assert_eq!(
            build_mysql_move_column_sql("`t`", &def, None),
            "ALTER TABLE `t` MODIFY COLUMN `name` varchar(20) DEFAULT 'x' COMMENT 'n' FIRST"
        );
        assert!(build_mysql_move_column_sql("`t`", &def, Some("id")).ends_with("AFTER `id`"));
        assert!(extract_mysql_column_definition(ddl, "nope").is_none());
    }

    #[test]
    fn geser_indeks_operasi() {
        let mut ops = vec![
            CellEditOperation::Update {
                row_index: 1,
                col_index: 0,
                old_value: "a".into(),
                new_value: "b".into(),
            },
            CellEditOperation::DeleteRow {
                row_index: 3,
                values: vec![],
            },
        ];
        shift_op_rows(&mut ops, 2, 1);
        assert_eq!(op_row(&ops[0]), 1);
        assert_eq!(op_row(&ops[1]), 4);
        shift_op_rows(&mut ops, 2, -1);
        assert_eq!(op_row(&ops[1]), 3);
        assert_eq!(pending_deleted_rows(&ops), [3].into());
    }

    #[test]
    fn ringkasan_operasi() {
        let ops = vec![
            CellEditOperation::Update {
                row_index: 0,
                col_index: 0,
                old_value: "a".into(),
                new_value: "b".into(),
            },
            CellEditOperation::Update {
                row_index: 0,
                col_index: 0,
                old_value: "b".into(),
                new_value: "c".into(),
            },
            CellEditOperation::InsertRow {
                row_index: 5,
                values: vec![],
            },
        ];
        let sum = summarize_ops(&ops);
        assert_eq!(
            sum,
            OpSummary {
                updates: 1,
                inserts: 1,
                deletes: 0
            }
        );
        assert_eq!(
            pending_updated_cells(&ops).get(&(0, 0)),
            Some(&"a".to_string())
        );
        assert_eq!(pending_inserted_rows(&ops), [5].into());
    }
}
