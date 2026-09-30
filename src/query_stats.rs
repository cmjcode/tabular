//! Statistik eksekusi query di `connections.db` (checklist I1, I2).
//!
//! Headless: fungsi I/O menerima `&SqlitePool` dan data biasa. Setiap eksekusi
//! dari editor atau browse tabel dicatat satu baris di `query_stats` (durasi,
//! jumlah baris, breakdown waktu). Berbeda dengan `query_history` yang
//! di-dedupe per teks dan dibatasi 150 baris, tabel ini menyimpan setiap run
//! selama [`RETENTION_DAYS`] hari sehingga tren bisa dihitung.
//!
//! Agregasi (paling sering, paling lambat, makin lambat, load tabel per hari)
//! dihitung di Rust dari baris mentah supaya bisa diuji tanpa database.

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

use crate::connection::timing::QueryTiming;

/// Baris lebih tua dari ini dihapus saat pemangkasan.
pub const RETENTION_DAYS: i64 = 30;
/// Batas jumlah baris total; baris tertua dipangkas lebih dulu.
pub const MAX_ROWS: i64 = 50_000;
/// Teks query yang disimpan dipotong di panjang ini.
const MAX_QUERY_CHARS: usize = 4_000;
/// Jumlah sampel maksimum yang dimuat untuk agregasi.
const MAX_SAMPLES: i64 = 20_000;
/// Minimal run per query sebelum tren "makin lambat" dihitung.
pub const MIN_RUNS_FOR_TREND: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// Query yang dijalankan dari editor.
    Query,
    /// Halaman data saat membuka/browse tabel dari sidebar.
    TableLoad,
}

impl RunKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RunKind::Query => "query",
            RunKind::TableLoad => "table_load",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "table_load" {
            RunKind::TableLoad
        } else {
            RunKind::Query
        }
    }
}

/// Satu eksekusi yang akan dicatat.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionRecord {
    pub connection_id: i64,
    pub database_name: Option<String>,
    pub kind: RunKind,
    pub query_text: String,
    pub table_name: Option<String>,
    pub success: bool,
    pub duration_ms: f64,
    pub row_count: Option<i64>,
    pub timing: Option<QueryTiming>,
}

/// Satu baris yang dimuat kembali untuk agregasi.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub id: i64,
    pub connection_id: i64,
    pub database_name: Option<String>,
    pub kind: RunKind,
    pub fingerprint: String,
    pub query_text: String,
    pub table_name: Option<String>,
    pub success: bool,
    pub duration_ms: f64,
    pub row_count: Option<i64>,
    pub timing: Option<QueryTiming>,
    /// UTC, format `YYYY-MM-DD HH:MM:SS`.
    pub executed_at: String,
}

/// Normalisasi SQL untuk pengelompokan: komentar dibuang, literal string dan
/// angka jadi `?`, daftar `?, ?, ?` diringkas, spasi diringkas, huruf kecil,
/// tanpa titik koma di akhir.
pub fn normalize_sql(sql: &str) -> String {
    let chars: Vec<char> = sql.chars().collect();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let push_space = |out: &mut String| {
        if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '-' && next == Some('-') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            push_space(&mut out);
            continue;
        }
        if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
            push_space(&mut out);
            continue;
        }
        if c == '\'' {
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            out.push('?');
            continue;
        }
        if c.is_ascii_digit() {
            let prev_ident = out
                .chars()
                .last()
                .is_some_and(|p| p.is_alphanumeric() || p == '_' || p == '$');
            if !prev_ident {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                out.push('?');
                continue;
            }
        }
        if c.is_whitespace() {
            push_space(&mut out);
            i += 1;
            continue;
        }
        out.extend(c.to_lowercase());
        i += 1;
    }
    // Spasi di sebelah tanda baca dibuang: `a = 1` dan `a=1` satu grup.
    let trimmed: Vec<char> = out.trim().trim_end_matches(';').trim().chars().collect();
    let is_word =
        |c: char| c.is_alphanumeric() || matches!(c, '_' | '$' | '?' | '"' | '`' | ']' | '[');
    let mut s = String::with_capacity(trimmed.len());
    for (idx, &c) in trimmed.iter().enumerate() {
        if c == ' ' {
            let prev = idx.checked_sub(1).map(|j| trimmed[j]);
            let next = trimmed.get(idx + 1).copied();
            if !(prev.is_some_and(is_word) && next.is_some_and(is_word)) {
                continue;
            }
        }
        s.push(c);
    }
    while s.contains("?,?") {
        s = s.replace("?,?", "?");
    }
    s
}

/// Fingerprint 16 digit hex dari [`normalize_sql`].
pub fn fingerprint(sql: &str) -> String {
    let digest = Sha256::digest(normalize_sql(sql).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Buang pembungkus identifier (`"x"`, `` `x` ``, `[x]`).
fn unquote_ident(s: &str) -> &str {
    let s = s.trim();
    for (open, close) in [('"', '"'), ('`', '`'), ('[', ']')] {
        if s.len() >= 2 && s.starts_with(open) && s.ends_with(close) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Nama tabel bila `sql` adalah `SELECT ... FROM <satu tabel>` tanpa JOIN,
/// subquery di FROM, CTE, atau daftar tabel. Qualifier (`db.schema.`) dibuang.
pub fn single_table_of_select(sql: &str) -> Option<String> {
    let is_select = normalize_sql(sql)
        .strip_prefix("select")
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
    if !is_select {
        return None;
    }
    // Cari FROM pertama di kedalaman kurung 0 pada teks asli (nama tabel
    // dipertahankan case-nya); literal string dilewati.
    let src: Vec<char> = sql.chars().collect();
    let mut depth = 0i32;
    let mut i = 0;
    let mut from_at = None;
    while i < src.len() {
        let c = src[i];
        match c {
            '\'' => {
                i += 1;
                while i < src.len() && src[i] != '\'' {
                    i += 1;
                }
            }
            '(' => depth += 1,
            ')' => depth -= 1,
            'f' | 'F'
                if depth == 0
                    && (i == 0 || !(src[i - 1].is_alphanumeric() || src[i - 1] == '_'))
                    && src.len() > i + 4
                    && src[i..i + 4]
                        .iter()
                        .collect::<String>()
                        .eq_ignore_ascii_case("from")
                    && src[i + 4].is_whitespace() =>
            {
                from_at = Some(i + 4);
                break;
            }
            _ => {}
        }
        i += 1;
    }
    let rest: String = src[from_at?..].iter().collect();
    let rest: Vec<char> = rest.trim_start().chars().collect();
    if rest.first() == Some(&'(') {
        return None;
    }
    // Identifier (boleh ber-qualifier dan ber-quote) sampai spasi/koma/titik koma.
    let mut end = 0;
    let mut quote: Option<char> = None;
    while end < rest.len() {
        let c = rest[end];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '`' => quote = Some(c),
            None if c == '[' => quote = Some(']'),
            None if c.is_whitespace() || c == ',' || c == ';' || c == ')' => break,
            None => {}
        }
        end += 1;
    }
    let ident: String = rest[..end].iter().collect();
    let tail: String = rest[end..].iter().collect::<String>().to_ascii_lowercase();
    // Koma setelah tabel (atau setelah alias) berarti daftar tabel.
    let tail_head = tail.split(';').next().unwrap_or("");
    let words: Vec<&str> = tail_head.split_whitespace().collect();
    let before_clause: Vec<&str> = words
        .iter()
        .take_while(|w| {
            !matches!(
                **w,
                "where" | "order" | "group" | "limit" | "offset" | "having" | "fetch" | "union"
            )
        })
        .copied()
        .collect();
    if words.contains(&"join") || before_clause.iter().any(|w| w.contains(',')) {
        return None;
    }
    let last = split_qualified(&ident).pop()?;
    let name = unquote_ident(&last).to_string();
    (!name.is_empty()).then_some(name)
}

/// Pecah `a.b."c.d"` per titik di luar quote.
fn split_qualified(ident: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in ident.chars() {
        match quote {
            Some(q) if c == q => {
                quote = None;
                cur.push(c);
            }
            Some(_) => cur.push(c),
            None if c == '.' => parts.push(std::mem::take(&mut cur)),
            None => {
                if c == '"' || c == '`' {
                    quote = Some(c);
                } else if c == '[' {
                    quote = Some(']');
                }
                cur.push(c);
            }
        }
    }
    parts.push(cur);
    parts
}

/// Membuat tabel bila belum ada. Aman dipanggil berulang.
pub async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS query_stats (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            connection_id INTEGER NOT NULL,
            database_name TEXT NULL,
            kind TEXT NOT NULL DEFAULT 'query',
            fingerprint TEXT NOT NULL,
            query_text TEXT NOT NULL,
            table_name TEXT NULL,
            success INTEGER NOT NULL DEFAULT 1,
            duration_ms REAL NOT NULL,
            row_count INTEGER NULL,
            wait_ms REAL NULL,
            server_ms REAL NULL,
            transfer_ms REAL NULL,
            client_ms REAL NULL,
            executed_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(pool)
    .await?;
    for ddl in [
        "CREATE INDEX IF NOT EXISTS idx_query_stats_conn_time ON query_stats (connection_id, executed_at)",
        "CREATE INDEX IF NOT EXISTS idx_query_stats_table ON query_stats (connection_id, table_name, executed_at)",
        "CREATE INDEX IF NOT EXISTS idx_query_stats_time ON query_stats (executed_at)",
    ] {
        sqlx::query(ddl).execute(pool).await?;
    }
    Ok(())
}

/// Mencatat satu eksekusi dan sesekali memangkas baris lama. Mengembalikan id baru.
pub async fn record(pool: &SqlitePool, rec: &ExecutionRecord) -> Result<i64, sqlx::Error> {
    ensure_table(pool).await?;
    let text: String = rec
        .query_text
        .trim()
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect();
    let t = rec.timing;
    let id = sqlx::query(
        "INSERT INTO query_stats (connection_id, database_name, kind, fingerprint, query_text,
            table_name, success, duration_ms, row_count, wait_ms, server_ms, transfer_ms, client_ms)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(rec.connection_id)
    .bind(rec.database_name.as_deref())
    .bind(rec.kind.as_str())
    .bind(fingerprint(&rec.query_text))
    .bind(&text)
    .bind(rec.table_name.as_deref())
    .bind(rec.success as i64)
    .bind(rec.duration_ms)
    .bind(rec.row_count)
    .bind(t.map(|t| t.wait_ms))
    .bind(t.map(|t| t.server_ms))
    .bind(t.map(|t| t.transfer_ms))
    .bind(t.map(|t| t.client_ms))
    .execute(pool)
    .await?
    .last_insert_rowid();
    if id % 200 == 0 {
        prune(pool).await?;
    }
    Ok(id)
}

/// Hapus baris di luar retensi dan di atas [`MAX_ROWS`].
pub async fn prune(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM query_stats WHERE executed_at < datetime('now', ?)")
        .bind(format!("-{RETENTION_DAYS} days"))
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM query_stats WHERE id <= (
            SELECT id FROM query_stats ORDER BY id DESC LIMIT 1 OFFSET ?
        )",
    )
    .bind(MAX_ROWS)
    .execute(pool)
    .await?;
    Ok(())
}

/// Hapus semua statistik (opsional per koneksi).
pub async fn clear(pool: &SqlitePool, connection_id: Option<i64>) -> Result<u64, sqlx::Error> {
    ensure_table(pool).await?;
    let res = match connection_id {
        Some(id) => {
            sqlx::query("DELETE FROM query_stats WHERE connection_id = ?")
                .bind(id)
                .execute(pool)
                .await?
        }
        None => sqlx::query("DELETE FROM query_stats").execute(pool).await?,
    };
    Ok(res.rows_affected())
}

fn row_to_sample(row: &sqlx::sqlite::SqliteRow) -> Result<Sample, sqlx::Error> {
    let wait: Option<f64> = row.try_get("wait_ms")?;
    let server: Option<f64> = row.try_get("server_ms")?;
    let transfer: Option<f64> = row.try_get("transfer_ms")?;
    let client: Option<f64> = row.try_get("client_ms")?;
    let timing = match (wait, server, transfer, client) {
        (Some(wait_ms), Some(server_ms), Some(transfer_ms), Some(client_ms)) => Some(QueryTiming {
            wait_ms,
            server_ms,
            transfer_ms,
            client_ms,
        }),
        _ => None,
    };
    let kind: String = row.try_get("kind")?;
    let success: i64 = row.try_get("success")?;
    Ok(Sample {
        id: row.try_get("id")?,
        connection_id: row.try_get("connection_id")?,
        database_name: row.try_get("database_name")?,
        kind: RunKind::parse(&kind),
        fingerprint: row.try_get("fingerprint")?,
        query_text: row.try_get("query_text")?,
        table_name: row.try_get("table_name")?,
        success: success != 0,
        duration_ms: row.try_get("duration_ms")?,
        row_count: row.try_get("row_count")?,
        timing,
        executed_at: row.try_get("executed_at")?,
    })
}

const SAMPLE_COLUMNS: &str = "id, connection_id, database_name, kind, fingerprint, query_text, \
    table_name, success, duration_ms, row_count, wait_ms, server_ms, transfer_ms, client_ms, \
    CAST(executed_at AS TEXT) AS executed_at";

/// Sampel `days` hari terakhir (terbaru dulu), opsional per koneksi dan jenis.
pub async fn load_samples(
    pool: &SqlitePool,
    connection_id: Option<i64>,
    kind: Option<RunKind>,
    days: i64,
) -> Result<Vec<Sample>, sqlx::Error> {
    ensure_table(pool).await?;
    let sql = format!(
        "SELECT {SAMPLE_COLUMNS} FROM query_stats
         WHERE executed_at >= datetime('now', ?)
           AND (? IS NULL OR connection_id = ?)
           AND (? IS NULL OR kind = ?)
         ORDER BY id DESC LIMIT ?"
    );
    let kind = kind.map(|k| k.as_str());
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(format!("-{} days", days.max(1)))
        .bind(connection_id)
        .bind(connection_id)
        .bind(kind)
        .bind(kind)
        .bind(MAX_SAMPLES)
        .fetch_all(pool)
        .await?;
    rows.iter().map(row_to_sample).collect()
}

/// Riwayat load satu tabel `days` hari terakhir (terbaru dulu). Nama tabel
/// dicocokkan tanpa membedakan huruf besar/kecil.
pub async fn table_load_history(
    pool: &SqlitePool,
    connection_id: i64,
    table_name: &str,
    days: i64,
) -> Result<Vec<Sample>, sqlx::Error> {
    ensure_table(pool).await?;
    let sql = format!(
        "SELECT {SAMPLE_COLUMNS} FROM query_stats
         WHERE connection_id = ? AND table_name = ? COLLATE NOCASE
           AND executed_at >= datetime('now', ?)
         ORDER BY id DESC LIMIT 500"
    );
    let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(connection_id)
        .bind(table_name)
        .bind(format!("-{} days", days.max(1)))
        .fetch_all(pool)
        .await?;
    rows.iter().map(row_to_sample).collect()
}

/// Tren durasi: median paruh awal vs paruh akhir run (urut waktu).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trend {
    pub earlier_ms: f64,
    pub recent_ms: f64,
}

impl Trend {
    pub fn ratio(&self) -> f64 {
        if self.earlier_ms <= 0.0 {
            return 1.0;
        }
        self.recent_ms / self.earlier_ms
    }

    /// Makin lambat: median terbaru >= 1.5x dan selisih >= 20 ms (hindari noise
    /// query yang sangat cepat).
    pub fn is_regression(&self) -> bool {
        self.ratio() >= 1.5 && self.recent_ms - self.earlier_ms >= 20.0
    }
}

/// Agregat per fingerprint.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryAggregate {
    pub fingerprint: String,
    /// Teks query dari run terbaru.
    pub query_text: String,
    pub connection_id: i64,
    pub runs: usize,
    pub failures: usize,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub total_ms: f64,
    pub last_run: String,
    pub trend: Option<Trend>,
}

fn median(sorted: &[f64]) -> f64 {
    match sorted.len() {
        0 => 0.0,
        n if n % 2 == 1 => sorted[n / 2],
        n => (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0,
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn sorted_copy(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

/// Tren dari durasi berurutan waktu (lama ke baru). `None` bila run kurang dari
/// [`MIN_RUNS_FOR_TREND`].
pub fn trend_of(chronological: &[f64]) -> Option<Trend> {
    if chronological.len() < MIN_RUNS_FOR_TREND {
        return None;
    }
    let mid = chronological.len() / 2;
    Some(Trend {
        earlier_ms: median(&sorted_copy(&chronological[..mid])),
        recent_ms: median(&sorted_copy(&chronological[chronological.len() - mid..])),
    })
}

/// Kelompokkan sampel per (koneksi, fingerprint). Durasi rata-rata/p95/tren
/// hanya dari run yang sukses; run gagal dihitung di `failures`.
pub fn aggregate(samples: &[Sample]) -> Vec<QueryAggregate> {
    let mut groups: HashMap<(i64, &str), Vec<&Sample>> = HashMap::new();
    for s in samples {
        groups
            .entry((s.connection_id, s.fingerprint.as_str()))
            .or_default()
            .push(s);
    }
    let mut out: Vec<QueryAggregate> = groups
        .into_values()
        .map(|mut runs| {
            // Urut kronologis: id naik.
            runs.sort_by_key(|s| s.id);
            let latest = runs[runs.len() - 1];
            let ok: Vec<f64> = runs
                .iter()
                .filter(|s| s.success)
                .map(|s| s.duration_ms)
                .collect();
            let sorted = sorted_copy(&ok);
            let total: f64 = ok.iter().sum();
            QueryAggregate {
                fingerprint: latest.fingerprint.clone(),
                query_text: latest.query_text.clone(),
                connection_id: latest.connection_id,
                runs: runs.len(),
                failures: runs.iter().filter(|s| !s.success).count(),
                avg_ms: if ok.is_empty() {
                    0.0
                } else {
                    total / ok.len() as f64
                },
                p95_ms: percentile(&sorted, 95.0),
                max_ms: sorted.last().copied().unwrap_or(0.0),
                total_ms: total,
                last_run: latest.executed_at.clone(),
                trend: trend_of(&ok),
            }
        })
        .collect();
    out.sort_by(|a, b| b.runs.cmp(&a.runs).then(a.fingerprint.cmp(&b.fingerprint)));
    out
}

/// Urutkan untuk tampilan "Most Run": jumlah run terbanyak.
pub fn most_run(aggs: &[QueryAggregate], limit: usize) -> Vec<QueryAggregate> {
    let mut v = aggs.to_vec();
    v.sort_by(|a, b| b.runs.cmp(&a.runs).then(b.total_ms.total_cmp(&a.total_ms)));
    v.truncate(limit);
    v
}

/// Urutkan untuk tampilan "Slowest": rata-rata durasi tertinggi.
pub fn slowest(aggs: &[QueryAggregate], limit: usize) -> Vec<QueryAggregate> {
    let mut v: Vec<_> = aggs
        .iter()
        .filter(|a| a.runs > a.failures)
        .cloned()
        .collect();
    v.sort_by(|a, b| b.avg_ms.total_cmp(&a.avg_ms));
    v.truncate(limit);
    v
}

/// Urutkan untuk tampilan "Getting Slower": hanya regresi, rasio terbesar dulu.
pub fn increasingly_slow(aggs: &[QueryAggregate], limit: usize) -> Vec<QueryAggregate> {
    let mut v: Vec<_> = aggs
        .iter()
        .filter(|a| a.trend.is_some_and(|t| t.is_regression()))
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        let ra = a.trend.map(|t| t.ratio()).unwrap_or(0.0);
        let rb = b.trend.map(|t| t.ratio()).unwrap_or(0.0);
        rb.total_cmp(&ra)
    });
    v.truncate(limit);
    v
}

/// Ringkasan load per tabel.
#[derive(Debug, Clone, PartialEq)]
pub struct TableAggregate {
    pub connection_id: i64,
    pub database_name: Option<String>,
    pub table_name: String,
    pub loads: usize,
    pub avg_ms: f64,
    pub max_ms: f64,
    pub last_ms: f64,
    pub last_run: String,
}

/// Kelompokkan sampel ber-`table_name` per (koneksi, database, tabel).
pub fn aggregate_tables(samples: &[Sample]) -> Vec<TableAggregate> {
    type Key = (i64, Option<String>, String);
    let mut groups: HashMap<Key, Vec<&Sample>> = HashMap::new();
    for s in samples.iter().filter(|s| s.success) {
        if let Some(t) = &s.table_name {
            groups
                .entry((s.connection_id, s.database_name.clone(), t.to_lowercase()))
                .or_default()
                .push(s);
        }
    }
    let mut out: Vec<TableAggregate> = groups
        .into_values()
        .map(|mut runs| {
            runs.sort_by_key(|s| s.id);
            let latest = runs[runs.len() - 1];
            let total: f64 = runs.iter().map(|s| s.duration_ms).sum();
            TableAggregate {
                connection_id: latest.connection_id,
                database_name: latest.database_name.clone(),
                table_name: latest.table_name.clone().unwrap_or_default(),
                loads: runs.len(),
                avg_ms: total / runs.len() as f64,
                max_ms: runs.iter().map(|s| s.duration_ms).fold(0.0, f64::max),
                last_ms: latest.duration_ms,
                last_run: latest.executed_at.clone(),
            }
        })
        .collect();
    out.sort_by(|a, b| b.loads.cmp(&a.loads).then(b.avg_ms.total_cmp(&a.avg_ms)));
    out
}

/// Satu hari di riwayat load.
#[derive(Debug, Clone, PartialEq)]
pub struct DayBucket {
    pub date: chrono::NaiveDate,
    pub loads: usize,
    pub avg_ms: f64,
    pub max_ms: f64,
}

/// Parse `executed_at` (UTC) menjadi waktu lokal.
pub fn parse_utc(executed_at: &str) -> Option<chrono::DateTime<chrono::Local>> {
    let naive = chrono::NaiveDateTime::parse_from_str(executed_at, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(naive.and_utc().with_timezone(&chrono::Local))
}

/// Bucket per hari lokal untuk `days` hari yang berakhir di `today` (inklusif,
/// terlama dulu). Hari tanpa load tetap muncul dengan `loads = 0`.
pub fn daily_buckets(samples: &[Sample], days: i64, today: chrono::NaiveDate) -> Vec<DayBucket> {
    let days = days.max(1);
    let mut per_day: HashMap<chrono::NaiveDate, Vec<f64>> = HashMap::new();
    for s in samples.iter().filter(|s| s.success) {
        if let Some(local) = parse_utc(&s.executed_at) {
            per_day
                .entry(local.date_naive())
                .or_default()
                .push(s.duration_ms);
        }
    }
    (0..days)
        .rev()
        .map(|back| {
            let date = today - chrono::Duration::days(back);
            let values = per_day.get(&date).map(Vec::as_slice).unwrap_or(&[]);
            DayBucket {
                date,
                loads: values.len(),
                avg_ms: if values.is_empty() {
                    0.0
                } else {
                    values.iter().sum::<f64>() / values.len() as f64
                },
                max_ms: values.iter().copied().fold(0.0, f64::max),
            }
        })
        .collect()
}

/// Format durasi singkat untuk UI: `850 ms`, `1.24 s`, `2m 05s`.
pub fn format_ms(ms: f64) -> String {
    if ms < 1.0 {
        format!("{ms:.2} ms")
    } else if ms < 1_000.0 {
        format!("{ms:.0} ms")
    } else if ms < 60_000.0 {
        format!("{:.2} s", ms / 1_000.0)
    } else {
        let secs = (ms / 1_000.0).round() as u64;
        format!("{}m {:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: i64, fp: &str, ms: f64) -> Sample {
        Sample {
            id,
            connection_id: 1,
            database_name: None,
            kind: RunKind::Query,
            fingerprint: fp.into(),
            query_text: format!("select {fp}"),
            table_name: None,
            success: true,
            duration_ms: ms,
            row_count: None,
            timing: None,
            executed_at: "2026-09-30 10:00:00".into(),
        }
    }

    #[test]
    fn normalisasi_mengganti_literal_dan_meringkas_spasi() {
        let a = normalize_sql("SELECT *  FROM users WHERE id = 42 AND name = 'O''Brien';");
        assert_eq!(a, "select*from users where id=? and name=?");
        let b = normalize_sql("select * from users -- komentar\n where id = 7 and name='x'");
        assert_eq!(a, b);
        assert_eq!(
            fingerprint("select 1 from t2 where x in (1, 2, 3)"),
            fingerprint("SELECT 9 FROM t2 WHERE x IN (4)")
        );
        // Angka dalam identifier bukan literal.
        assert!(normalize_sql("select col1 from t2").contains("col1"));
        assert_ne!(
            fingerprint("select a from t"),
            fingerprint("select b from t")
        );
    }

    #[test]
    fn tabel_tunggal_dari_select() {
        let t = single_table_of_select;
        assert_eq!(
            t("SELECT * FROM users LIMIT 100 OFFSET 0").as_deref(),
            Some("users")
        );
        assert_eq!(
            t("select * from `shop`.`Orders` where id > 3").as_deref(),
            Some("Orders")
        );
        assert_eq!(
            t("SELECT * FROM \"public\".\"line items\" u").as_deref(),
            Some("line items")
        );
        assert_eq!(
            t("SELECT TOP 100 * FROM [dbo].[Customers]").as_deref(),
            Some("Customers")
        );
        assert_eq!(
            t("select (select max(x) from b) from a").as_deref(),
            Some("a")
        );
        assert_eq!(t("select * from a where x in (1,2)").as_deref(), Some("a"));
        assert_eq!(t("select * from a join b on a.id=b.id"), None);
        assert_eq!(t("select * from a, b"), None);
        assert_eq!(t("select * from a x, b y"), None);
        assert_eq!(t("select * from (select 1) x"), None);
        assert_eq!(t("update a set x = 1"), None);
        assert_eq!(t("with c as (select 1) select * from c"), None);
        assert_eq!(t("select 'from x' as s"), None);
    }

    #[test]
    fn agregasi_menghitung_run_rata_rata_dan_gagal() {
        let mut s = vec![
            sample(1, "a", 10.0),
            sample(2, "a", 30.0),
            sample(3, "b", 500.0),
        ];
        let mut failed = sample(4, "a", 1.0);
        failed.success = false;
        s.push(failed);
        let aggs = aggregate(&s);
        let a = aggs.iter().find(|x| x.fingerprint == "a").unwrap();
        assert_eq!(a.runs, 3);
        assert_eq!(a.failures, 1);
        assert!((a.avg_ms - 20.0).abs() < 1e-9);
        assert_eq!(a.max_ms, 30.0);
        assert_eq!(most_run(&aggs, 10)[0].fingerprint, "a");
        assert_eq!(slowest(&aggs, 10)[0].fingerprint, "b");
    }

    #[test]
    fn tren_mendeteksi_query_yang_makin_lambat() {
        assert!(trend_of(&[1.0; 5]).is_none());
        let t = trend_of(&[100.0, 110.0, 90.0, 300.0, 320.0, 310.0]).unwrap();
        assert_eq!(t.earlier_ms, 100.0);
        assert_eq!(t.recent_ms, 310.0);
        assert!(t.is_regression());
        // Cepat tetapi rasio besar: selisih < 20 ms bukan regresi.
        let fast = trend_of(&[1.0, 1.0, 1.0, 5.0, 5.0, 5.0]).unwrap();
        assert!(!fast.is_regression());

        let mut s: Vec<Sample> = [100.0, 100.0, 100.0, 400.0, 400.0, 400.0]
            .iter()
            .enumerate()
            .map(|(i, ms)| sample(i as i64 + 1, "slow", *ms))
            .collect();
        s.extend((10..16).map(|i| sample(i, "steady", 50.0)));
        let aggs = aggregate(&s);
        let reg = increasingly_slow(&aggs, 10);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0].fingerprint, "slow");
    }

    #[test]
    fn bucket_harian_mengisi_hari_kosong() {
        let today = chrono::Local::now().date_naive();
        let now_utc = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        let mut a = sample(1, "t", 100.0);
        a.executed_at = now_utc.clone();
        let mut b = sample(2, "t", 300.0);
        b.executed_at = now_utc;
        let buckets = daily_buckets(&[a, b], 7, today);
        assert_eq!(buckets.len(), 7);
        assert_eq!(buckets[6].date, today);
        assert_eq!(buckets[6].loads, 2);
        assert_eq!(buckets[6].avg_ms, 200.0);
        assert_eq!(buckets[6].max_ms, 300.0);
        assert_eq!(buckets[0].loads, 0);
    }

    #[test]
    fn agregasi_tabel_per_nama_tanpa_case() {
        let mut a = sample(1, "x", 10.0);
        a.table_name = Some("Users".into());
        let mut b = sample(2, "y", 30.0);
        b.table_name = Some("users".into());
        let aggs = aggregate_tables(&[a, b, sample(3, "z", 5.0)]);
        assert_eq!(aggs.len(), 1);
        assert_eq!(aggs[0].loads, 2);
        assert_eq!(aggs[0].last_ms, 30.0);
    }

    #[test]
    fn format_ms_ringkas() {
        assert_eq!(format_ms(850.0), "850 ms");
        assert_eq!(format_ms(1240.0), "1.24 s");
        assert_eq!(format_ms(125_000.0), "2m 05s");
    }

    async fn memory_pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn record_dan_load_bolak_balik() {
        let pool = memory_pool().await;
        let rec = ExecutionRecord {
            connection_id: 7,
            database_name: Some("shop".into()),
            kind: RunKind::TableLoad,
            query_text: "SELECT * FROM orders LIMIT 100".into(),
            table_name: Some("orders".into()),
            success: true,
            duration_ms: 42.5,
            row_count: Some(100),
            timing: Some(QueryTiming {
                wait_ms: 1.0,
                server_ms: 30.0,
                transfer_ms: 10.0,
                client_ms: 1.5,
            }),
        };
        record(&pool, &rec).await.unwrap();
        record(
            &pool,
            &ExecutionRecord {
                connection_id: 8,
                kind: RunKind::Query,
                table_name: None,
                timing: None,
                ..rec.clone()
            },
        )
        .await
        .unwrap();

        let all = load_samples(&pool, None, None, 7).await.unwrap();
        assert_eq!(all.len(), 2);
        let only7 = load_samples(&pool, Some(7), Some(RunKind::TableLoad), 7)
            .await
            .unwrap();
        assert_eq!(only7.len(), 1);
        assert_eq!(only7[0].timing.unwrap().server_ms, 30.0);
        assert_eq!(only7[0].row_count, Some(100));
        assert!(parse_utc(&only7[0].executed_at).is_some());

        let hist = table_load_history(&pool, 7, "ORDERS", 7).await.unwrap();
        assert_eq!(hist.len(), 1);

        assert_eq!(clear(&pool, Some(8)).await.unwrap(), 1);
        prune(&pool).await.unwrap();
        assert_eq!(load_samples(&pool, None, None, 7).await.unwrap().len(), 1);
    }
}
