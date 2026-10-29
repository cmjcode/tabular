//! Langkah DB untuk integration test dan data check: query baca ke koneksi
//! database lalu assertion atas hasilnya.
//!
//! Headless. Isi modul:
//! - model [`DbQueryStep`] dan [`StepKind`];
//! - [`bind_vars`]: `{{x}}` di SQL menjadi placeholder parameter (`?`, `$1`,
//!   `@P1`); nilainya **tidak pernah** disambung ke teks SQL;
//! - target hasil query (`row_count`, `scalar`, `cell:<baris>.<kolom>`,
//!   `column:<kolom>`) dan evaluasi aritmetika sederhana untuk nilai assertion;
//! - [`DbExecutor`]: antarmuka sinkron yang dipakai runner, dengan
//!   implementasi nyata [`SessionDbExecutor`] di atas
//!   [`crate::agent::core::HeadlessSession`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::connection_env::Environment;

// ─── Model ──────────────────────────────────────────────────────────────────

/// Jenis langkah suite. Suite lama tanpa field `kind` = `Http`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    #[default]
    Http,
    DbQuery,
}

impl StepKind {
    pub fn is_http(&self) -> bool {
        *self == StepKind::Http
    }

    pub fn label(self) -> &'static str {
        match self {
            StepKind::Http => "HTTP request",
            StepKind::DbQuery => "Database query",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_")
            .as_str()
        {
            "db_query" | "db" | "database" | "sql" | "query" | "db_assert" | "db_assertion" => {
                StepKind::DbQuery
            }
            _ => StepKind::Http,
        }
    }
}

pub fn default_poll_ms() -> u64 {
    250
}

/// Query baca satu langkah DB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbQueryStep {
    /// Nama variabel environment yang berisi koneksi (nama atau id), mis.
    /// `db_main`. Bukan koneksi literal, agar suite yang sama jalan di
    /// Dev/Staging.
    pub connection_var: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    /// Satu SELECT. `{{x}}` diikat sebagai parameter, bukan disambung ke teks.
    pub sql: String,
    /// Ulangi query sampai semua assertion lulus atau batas waktu ini habis
    /// (untuk efek async seperti goroutine/queue setelah commit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_until_ms: Option<u64>,
    #[serde(default = "default_poll_ms")]
    pub poll_ms: u64,
}

impl Default for DbQueryStep {
    fn default() -> Self {
        Self {
            connection_var: "db_main".into(),
            database: None,
            sql: String::new(),
            wait_until_ms: None,
            poll_ms: default_poll_ms(),
        }
    }
}

/// Batas waktu tunggu polling (5 menit) dan jeda minimum.
pub const MAX_WAIT_MS: u64 = 300_000;
pub const MIN_POLL_MS: u64 = 50;
/// Baris maksimum yang diambil langkah DB / data check.
pub const DB_MAX_ROWS: usize = 1000;
/// Baris contoh yang disimpan di hasil run dan laporan.
pub const SAMPLE_ROWS: usize = 20;

// ─── Bind parameter ─────────────────────────────────────────────────────────

/// Gaya placeholder per engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    /// MySQL, SQLite.
    Question,
    /// PostgreSQL.
    Dollar,
    /// SQL Server.
    AtP,
}

impl Placeholder {
    /// Dari label jenis koneksi (`ConnectionSummary::kind`).
    pub fn for_kind(kind: &str) -> Self {
        match kind {
            "PostgreSQL" => Placeholder::Dollar,
            "MsSQL" => Placeholder::AtP,
            _ => Placeholder::Question,
        }
    }

    fn render(self, n: usize) -> String {
        match self {
            Placeholder::Question => "?".into(),
            Placeholder::Dollar => format!("${n}"),
            Placeholder::AtP => format!("@P{n}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SqlParam {
    Int(i64),
    Float(f64),
    Text(String),
}

impl SqlParam {
    /// Nilai di luar tanda kutip: angka murni diikat sebagai angka (agar
    /// perbandingan dengan kolom numerik berlaku di PostgreSQL), sisanya teks.
    fn infer(value: &str) -> Self {
        let v = value.trim();
        let digits = v.strip_prefix('-').unwrap_or(v);
        if !digits.is_empty()
            && digits.len() <= 18
            && digits.bytes().all(|b| b.is_ascii_digit())
            && let Ok(i) = v.parse::<i64>()
        {
            return SqlParam::Int(i);
        }
        if let Some((a, b)) = digits.split_once('.')
            && !a.is_empty()
            && !b.is_empty()
            && a.bytes().all(|c| c.is_ascii_digit())
            && b.bytes().all(|c| c.is_ascii_digit())
            && let Ok(f) = v.parse::<f64>()
        {
            return SqlParam::Float(f);
        }
        SqlParam::Text(value.to_string())
    }

    fn shown(&self, var_names: &[String]) -> String {
        let secret = var_names
            .iter()
            .any(|n| crate::http_tests::is_sensitive_var(n));
        match self {
            _ if secret => crate::http_ai::REDACTED.to_string(),
            SqlParam::Int(i) => i.to_string(),
            SqlParam::Float(f) => f.to_string(),
            SqlParam::Text(s) => {
                let cut: String = s.chars().take(200).collect();
                format!("'{}'", cut.replace('\'', "''"))
            }
        }
    }
}

/// SQL siap dijalankan: placeholder + nilai terikat.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoundSql {
    pub sql: String,
    pub params: Vec<SqlParam>,
    /// SQL dengan daftar nilai parameter (rahasia disamarkan) untuk laporan
    /// dan riwayat query.
    pub display: String,
}

fn read_var(name: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    vars.get(name)
        .cloned()
        .ok_or_else(|| format!("Missing variable {{{{{name}}}}}"))
}

/// Ubah `{{x}}` di SQL menjadi placeholder parameter.
///
/// - `{{x}}` di luar string: satu parameter (angka murni diikat sebagai angka).
/// - String `'…{{x}}…'`: seluruh literal menjadi satu parameter teks berisi
///   isi literal dengan variabel diganti, mis. `'%{{kode}}%'`.
/// - `{{x}}` di identifier berkutip (`"…"`, `` `…` ``, `[…]`) ditolak.
/// - Komentar dibiarkan apa adanya.
pub fn bind_vars(
    sql: &str,
    vars: &HashMap<String, String>,
    style: Placeholder,
) -> Result<BoundSql, String> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(sql.len());
    let mut params: Vec<SqlParam> = Vec::new();
    let mut names: Vec<Vec<String>> = Vec::new();
    let backslash_escapes = style == Placeholder::Question;
    let starts = |i: usize, s: &str| -> bool {
        let s: Vec<char> = s.chars().collect();
        i + s.len() <= n && chars[i..i + s.len()] == s[..]
    };
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if starts(i, "--") {
            while i < n && chars[i] != '\n' {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if starts(i, "/*") {
            let end = (i + 2..n.saturating_sub(1))
                .find(|&j| chars[j] == '*' && chars[j + 1] == '/')
                .map(|j| j + 2)
                .unwrap_or(n);
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        if starts(i, "{{") {
            let Some(close) = (i + 2..n.saturating_sub(1)).find(|&j| starts(j, "}}")) else {
                return Err("Unclosed {{ in SQL".into());
            };
            let name: String = chars[i + 2..close]
                .iter()
                .collect::<String>()
                .trim()
                .to_string();
            let value = read_var(&name, vars)?;
            params.push(SqlParam::infer(&value));
            names.push(vec![name]);
            out.push_str(&style.render(params.len()));
            i = close + 2;
            continue;
        }
        if c == '\'' {
            // Literal string sampai kutip penutup ('' = kutip di dalam).
            let mut j = i + 1;
            let mut content = String::new();
            let mut closed = false;
            while j < n {
                let d = chars[j];
                if backslash_escapes && d == '\\' && j + 1 < n {
                    content.push(d);
                    content.push(chars[j + 1]);
                    j += 2;
                    continue;
                }
                if d == '\'' {
                    if j + 1 < n && chars[j + 1] == '\'' {
                        content.push_str("''");
                        j += 2;
                        continue;
                    }
                    closed = true;
                    break;
                }
                content.push(d);
                j += 1;
            }
            if !closed {
                return Err("Unclosed string literal in SQL".into());
            }
            if content.contains("{{") {
                let used = crate::http_tests::unresolved_vars(&content);
                for v in &used {
                    read_var(v, vars)?;
                }
                let mut value = content.replace("''", "'");
                if backslash_escapes {
                    value = value.replace("\\'", "'").replace("\\\\", "\\");
                }
                let value = crate::http_tests::substitute(&value, vars);
                params.push(SqlParam::Text(value));
                names.push(used);
                out.push_str(&style.render(params.len()));
            } else {
                out.extend(&chars[i..=j]);
            }
            i = j + 1;
            continue;
        }
        if c == '"' || c == '`' || (c == '[' && style == Placeholder::AtP) {
            let close_ch = if c == '[' { ']' } else { c };
            let end = (i + 1..n).find(|&j| chars[j] == close_ch).unwrap_or(n - 1);
            let ident: String = chars[i..=end].iter().collect();
            if ident.contains("{{") {
                return Err(format!(
                    "Variables cannot be used as identifiers ({ident}); use them as values only"
                ));
            }
            out.push_str(&ident);
            i = end + 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    let mut display = out.clone();
    if !params.is_empty() {
        let shown: Vec<String> = params
            .iter()
            .zip(&names)
            .enumerate()
            .map(|(k, (p, ns))| format!("{} = {}", style.render(k + 1), p.shown(ns)))
            .collect();
        display.push_str(&format!("\n-- params: {}", shown.join(", ")));
    }
    Ok(BoundSql {
        sql: out,
        params,
        display,
    })
}

// ─── Hasil query & target ───────────────────────────────────────────────────

/// Hasil satu query langkah DB / data check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Baris melebihi [`DB_MAX_ROWS`] dan dibuang.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub time_ms: u128,
}

/// Target yang membaca hasil query, bukan respons HTTP.
pub fn is_query_target(target: &str) -> bool {
    let t = target.trim().to_ascii_lowercase();
    t == "row_count"
        || t == "rows"
        || t == "scalar"
        || t.starts_with("cell:")
        || t.starts_with("column:")
}

fn column_index(out: &QueryOutput, col: &str) -> Option<usize> {
    let col = col.trim();
    out.columns
        .iter()
        .position(|c| c.eq_ignore_ascii_case(col))
        .or_else(|| col.parse::<usize>().ok())
}

fn cell_value(v: &str) -> Option<String> {
    (v != crate::models::structs::NULL_DISPLAY).then(|| v.to_string())
}

/// Nilai target dari hasil query. `None` = tidak ada (baris/kolom tidak ada,
/// atau NULL).
pub fn read_query_target(out: &QueryOutput, target: &str) -> Option<String> {
    let t = target.trim();
    let lower = t.to_ascii_lowercase();
    if lower == "row_count" || lower == "rows" {
        return Some(out.rows.len().to_string());
    }
    if lower == "scalar" {
        return out.rows.first()?.first().and_then(|v| cell_value(v));
    }
    if let Some(rest) = t.strip_prefix("cell:") {
        let (row, col) = rest.split_once('.')?;
        let row: usize = row.trim().parse().ok()?;
        let idx = column_index(out, col)?;
        return out.rows.get(row)?.get(idx).and_then(|v| cell_value(v));
    }
    if let Some(col) = t.strip_prefix("column:") {
        let idx = column_index(out, col)?;
        return Some(
            out.rows
                .iter()
                .filter_map(|r| r.get(idx))
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    None
}

// ─── Aritmetika nilai assertion ─────────────────────────────────────────────

struct ArithParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl ArithParser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            self.ws();
            match self.s.get(self.i) {
                Some(b'+') => {
                    self.i += 1;
                    v += self.term()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => return Some(v),
            }
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        loop {
            self.ws();
            match self.s.get(self.i) {
                Some(b'*') => {
                    self.i += 1;
                    v *= self.factor()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let d = self.factor()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                _ => return Some(v),
            }
        }
    }

    fn factor(&mut self) -> Option<f64> {
        self.ws();
        match self.s.get(self.i)? {
            b'-' => {
                self.i += 1;
                Some(-self.factor()?)
            }
            b'+' => {
                self.i += 1;
                self.factor()
            }
            b'(' => {
                self.i += 1;
                let v = self.expr()?;
                self.ws();
                if self.s.get(self.i) != Some(&b')') {
                    return None;
                }
                self.i += 1;
                Some(v)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.')
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i])
                    .ok()?
                    .parse()
                    .ok()
            }
        }
    }
}

/// Evaluasi ekspresi `+ - * /` dengan kurung atas angka desimal. `None`
/// bila bukan ekspresi angka yang valid (mis. teks biasa).
pub fn eval_arith(expr: &str) -> Option<f64> {
    let mut p = ArithParser {
        s: expr.trim().as_bytes(),
        i: 0,
    };
    if p.s.is_empty() {
        return None;
    }
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

/// Format angka hasil aritmetika tanpa `.0` yang tidak perlu.
pub fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.10}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Nilai harapan assertion: variabel diganti, lalu bila hasilnya ekspresi
/// aritmetika (`{{a}} + {{b}}`) dievaluasi menjadi angka.
pub fn expected_value(raw: &str, vars: &HashMap<String, String>) -> String {
    let text = crate::http_tests::substitute(raw, vars);
    let t = text.trim();
    if t.parse::<f64>().is_ok() {
        return text;
    }
    let (body, tol) = match t.rsplit_once("+-") {
        Some((b, tol)) if tol.trim().parse::<f64>().is_ok() => (b.trim(), Some(tol.trim())),
        _ => (t, None),
    };
    // Operator wajib diapit spasi, supaya tanggal (`2026-10-29`) atau kode
    // (`10/2026`) tidak dihitung.
    let looks_arith = [" + ", " - ", " * ", " / "]
        .iter()
        .any(|op| body.contains(op))
        && body
            .chars()
            .all(|c| c.is_ascii_digit() || " .+-*/()".contains(c));
    match looks_arith.then(|| eval_arith(body)).flatten() {
        Some(v) => match tol {
            Some(tol) => format!("{} +- {tol}", format_number(v)),
            None => format_number(v),
        },
        None => text,
    }
}

/// Toleransi default operator `approx`.
pub const DEFAULT_TOLERANCE: f64 = 0.001;

/// Pecah nilai `approx`: `"100 +- 0.5"` / `"100 ± 0.5"` → (100, 0.5).
pub fn split_tolerance(expected: &str) -> (String, f64) {
    for sep in ["+/-", "+-", "±"] {
        if let Some((v, tol)) = expected.rsplit_once(sep)
            && let Ok(t) = tol.trim().parse::<f64>()
        {
            return (v.trim().to_string(), t.abs());
        }
    }
    (expected.trim().to_string(), DEFAULT_TOLERANCE)
}

// ─── Eksekusi ───────────────────────────────────────────────────────────────

/// Permintaan query dari runner.
#[derive(Debug, Clone)]
pub struct DbRequest {
    /// Nilai variabel `connection_var`: nama atau id koneksi.
    pub connection: String,
    pub database: Option<String>,
    /// SQL mentah dengan `{{x}}`; diikat oleh executor sesuai engine.
    pub sql: String,
    pub vars: HashMap<String, String>,
}

/// Hasil eksekusi beserta identitas koneksi.
#[derive(Debug, Clone, Default)]
pub struct DbResponse {
    pub connection_name: String,
    /// SQL dengan nilai parameter (untuk laporan).
    pub sql_display: String,
    pub output: QueryOutput,
}

/// Antarmuka sinkron yang dipakai runner (bisa diganti di test).
pub trait DbExecutor: Sync {
    fn query(&self, req: &DbRequest) -> Result<DbResponse, String>;
}

/// Koneksi tersimpan yang bisa dirujuk langkah DB.
#[derive(Debug, Clone)]
pub struct KnownConnection {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub environment: Option<Environment>,
}

/// Cari koneksi berdasarkan id atau nama (tanpa beda huruf besar).
pub fn resolve_connection<'a>(
    value: &str,
    known: &'a [KnownConnection],
) -> Result<&'a KnownConnection, String> {
    let v = value.trim();
    if v.is_empty() {
        return Err("the connection variable is empty".into());
    }
    if let Ok(id) = v.parse::<i64>()
        && let Some(c) = known.iter().find(|c| c.id == id)
    {
        return Ok(c);
    }
    known
        .iter()
        .find(|c| c.name == v)
        .or_else(|| known.iter().find(|c| c.name.eq_ignore_ascii_case(v)))
        .ok_or_else(|| format!("connection '{v}' not found in Tabular"))
}

/// Tolak koneksi Production kecuali run mengizinkannya.
pub fn check_production(conn: &KnownConnection, allow_production: bool) -> Result<(), String> {
    if conn.environment == Some(Environment::Production) && !allow_production {
        return Err(format!(
            "connection '{}' is marked Production; allow Production for this run to query it",
            conn.name
        ));
    }
    Ok(())
}

/// Executor nyata: sesi headless + koneksi tersimpan, dijalankan lewat
/// runtime bersama ([`crate::runtime::block_on`]).
pub struct SessionDbExecutor {
    session: crate::agent::core::HeadlessSession,
    known: Vec<KnownConnection>,
    allow_production: bool,
    /// Nama klien MCP bila dijalankan agent: koneksi `Blocked` ditolak.
    agent_client: Option<String>,
}

impl SessionDbExecutor {
    /// Buka `connections.db` dan muat daftar koneksi beserta environment-nya.
    pub fn open(allow_production: bool) -> Result<Self, String> {
        crate::runtime::block_on(async {
            let pool = crate::agent::core::open_cache_pool()
                .await
                .map_err(|e| e.to_string())?;
            Self::from_session(
                crate::agent::core::HeadlessSession::new(pool),
                allow_production,
            )
            .await
        })
        .ok_or_else(|| "async runtime unavailable".to_string())?
    }

    pub async fn from_session(
        session: crate::agent::core::HeadlessSession,
        allow_production: bool,
    ) -> Result<Self, String> {
        let known = known_connections(&session).await?;
        Ok(Self {
            session,
            known,
            allow_production,
            agent_client: None,
        })
    }

    /// Tandai eksekusi atas nama klien agent (cek akses koneksi).
    pub fn for_agent(mut self, client: &str) -> Self {
        self.agent_client = Some(client.to_string());
        self
    }

    pub fn known(&self) -> &[KnownConnection] {
        &self.known
    }

    /// Versi async dari [`DbExecutor::query`] untuk pemanggil yang sudah di
    /// dalam runtime (MCP, data check).
    pub async fn query_async(&self, req: &DbRequest) -> Result<DbResponse, String> {
        let conn = resolve_connection(&req.connection, &self.known)?;
        check_production(conn, self.allow_production)?;
        if let Some(client) = &self.agent_client {
            self.session
                .ensure_access(client, conn.id)
                .await
                .map_err(|e| e.to_string())?;
        }
        let bound = bind_vars(&req.sql, &req.vars, Placeholder::for_kind(&conn.kind))?;
        let r = self
            .session
            .run_bound_query(conn.id, &bound, req.database.as_deref(), DB_MAX_ROWS)
            .await
            .map_err(|e| e.to_string())?;
        Ok(DbResponse {
            connection_name: conn.name.clone(),
            sql_display: bound.display,
            output: QueryOutput {
                columns: r.columns,
                rows: r.rows,
                truncated: r.truncated,
                time_ms: r.execution_ms,
            },
        })
    }
}

impl DbExecutor for SessionDbExecutor {
    fn query(&self, req: &DbRequest) -> Result<DbResponse, String> {
        crate::runtime::block_on(self.query_async(req))
            .unwrap_or_else(|| Err("async runtime unavailable".into()))
    }
}

/// Koneksi tersimpan + environment efektifnya (tanda eksplisit atau nama).
pub async fn known_connections(
    session: &crate::agent::core::HeadlessSession,
) -> Result<Vec<KnownConnection>, String> {
    let list = session
        .list_connections()
        .await
        .map_err(|e| e.to_string())?;
    let marks = crate::connection_env::load_all(session.cache_pool())
        .await
        .unwrap_or_default();
    Ok(list
        .into_iter()
        .map(|c| KnownConnection {
            environment: crate::connection_env::effective(marks.get(&c.id).copied(), &c.name),
            id: c.id,
            name: c.name,
            kind: c.kind,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn injection_value_is_bound_not_spliced() {
        let v = vars(&[("name", "' OR 1=1 --"), ("id", "42")]);
        let b = bind_vars(
            "SELECT * FROM t WHERE name = {{name}} AND id = {{id}}",
            &v,
            Placeholder::Dollar,
        )
        .expect("bind");
        assert_eq!(b.sql, "SELECT * FROM t WHERE name = $1 AND id = $2");
        assert_eq!(
            b.params,
            vec![SqlParam::Text("' OR 1=1 --".into()), SqlParam::Int(42)]
        );
        assert!(!b.sql.contains("OR 1=1"));
        // Literal berkutip juga menjadi satu parameter.
        let b = bind_vars("SELECT 1 WHERE a = '{{name}}'", &v, Placeholder::Question).unwrap();
        assert_eq!(b.sql, "SELECT 1 WHERE a = ?");
        assert_eq!(b.params, vec![SqlParam::Text("' OR 1=1 --".into())]);
    }

    #[test]
    fn literals_comments_and_styles() {
        let v = vars(&[("k", "AB"), ("x", "1.5")]);
        let b = bind_vars(
            "SELECT '{{k}}%' AS a, 'it''s' AS b -- {{zzz}}\nFROM t WHERE p = {{x}}",
            &v,
            Placeholder::AtP,
        )
        .unwrap();
        assert_eq!(
            b.sql,
            "SELECT @P1 AS a, 'it''s' AS b -- {{zzz}}\nFROM t WHERE p = @P2"
        );
        assert_eq!(
            b.params,
            vec![SqlParam::Text("AB%".into()), SqlParam::Float(1.5)]
        );
        assert!(b.display.contains("@P1 = 'AB%'"));
        assert!(bind_vars("SELECT \"{{k}}\" FROM t", &v, Placeholder::Dollar).is_err());
        assert!(bind_vars("SELECT {{missing}}", &v, Placeholder::Dollar).is_err());
        assert!(bind_vars("SELECT 'open", &v, Placeholder::Dollar).is_err());
    }

    #[test]
    fn secret_params_are_redacted_in_display() {
        let v = vars(&[("api_token", "s3cr3t")]);
        let b = bind_vars(
            "SELECT 1 WHERE t = {{api_token}}",
            &v,
            Placeholder::Question,
        )
        .unwrap();
        assert!(!b.display.contains("s3cr3t"));
    }

    #[test]
    fn query_targets() {
        let out = QueryOutput {
            columns: vec!["id".into(), "qty_balance".into()],
            rows: vec![
                vec!["1".into(), "9500".into()],
                vec!["2".into(), crate::models::structs::NULL_DISPLAY.into()],
            ],
            ..Default::default()
        };
        assert_eq!(read_query_target(&out, "row_count").as_deref(), Some("2"));
        assert_eq!(read_query_target(&out, "scalar").as_deref(), Some("1"));
        assert_eq!(
            read_query_target(&out, "cell:0.qty_balance").as_deref(),
            Some("9500")
        );
        assert_eq!(read_query_target(&out, "cell:0.1").as_deref(), Some("9500"));
        assert_eq!(read_query_target(&out, "cell:1.qty_balance"), None);
        assert_eq!(read_query_target(&out, "cell:5.id"), None);
        assert_eq!(read_query_target(&out, "column:id").as_deref(), Some("1,2"));
        let empty = QueryOutput::default();
        assert_eq!(read_query_target(&empty, "scalar"), None);
        assert_eq!(read_query_target(&empty, "row_count").as_deref(), Some("0"));
    }

    #[test]
    fn arithmetic_in_expected_values() {
        assert_eq!(eval_arith("4750 + 4750"), Some(9500.0));
        assert_eq!(eval_arith("(2 + 3) * 4 - 10 / 4"), Some(17.5));
        assert_eq!(eval_arith("-3 + 1"), Some(-2.0));
        assert_eq!(eval_arith("1 / 0"), None);
        assert_eq!(eval_arith("abc"), None);
        let v = vars(&[("saldo_awal", "100.5"), ("qty_kirim", "20")]);
        assert_eq!(
            expected_value("{{saldo_awal}} + {{qty_kirim}}", &v),
            "120.5"
        );
        assert_eq!(
            expected_value("{{saldo_awal}} + {{qty_kirim}} +- 0.5", &v),
            "120.5 +- 0.5"
        );
        assert_eq!(expected_value("-5", &v), "-5");
        assert_eq!(expected_value("2026-10-29", &v), "2026-10-29");
        assert_eq!(expected_value("10/2026", &v), "10/2026");
        assert_eq!(expected_value("(1 + 2) * 3", &v), "9");
        assert_eq!(expected_value("ok", &v), "ok");
        assert_eq!(split_tolerance("100 +- 0.5"), ("100".into(), 0.5));
        assert_eq!(split_tolerance("100").1, DEFAULT_TOLERANCE);
    }

    #[test]
    fn connection_resolution_and_production_guard() {
        let known = vec![
            KnownConnection {
                id: 7,
                name: "voltunes staging".into(),
                kind: "MySQL".into(),
                environment: Some(Environment::Staging),
            },
            KnownConnection {
                id: 9,
                name: "Voltunes PROD".into(),
                kind: "MySQL".into(),
                environment: Some(Environment::Production),
            },
        ];
        assert_eq!(resolve_connection("7", &known).unwrap().id, 7);
        assert_eq!(resolve_connection("voltunes prod", &known).unwrap().id, 9);
        assert!(resolve_connection("nope", &known).is_err());
        assert!(resolve_connection(" ", &known).is_err());
        let prod = resolve_connection("9", &known).unwrap();
        assert!(check_production(prod, false).is_err());
        assert!(check_production(prod, true).is_ok());
        assert!(check_production(&known[0], false).is_ok());
    }

    #[test]
    fn step_kind_serde_is_backward_compatible() {
        assert_eq!(StepKind::parse("db_query"), StepKind::DbQuery);
        assert_eq!(StepKind::parse("SQL"), StepKind::DbQuery);
        assert_eq!(StepKind::parse("whatever"), StepKind::Http);
        let s: DbQueryStep =
            serde_json::from_str(r#"{"connection_var":"db_main","sql":"SELECT 1"}"#).unwrap();
        assert_eq!(s.poll_ms, 250);
        assert_eq!(s.wait_until_ms, None);
    }
}
