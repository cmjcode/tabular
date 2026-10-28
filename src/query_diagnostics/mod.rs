//! Diagnostik live untuk Query Editor: tanda di bawah teks untuk kesalahan
//! sebelum query dijalankan.
//!
//! Empat lapis, semuanya headless (data biasa masuk, `LintMessage` keluar):
//! 1. [`syntax`] — kesalahan leksikal pasti (string/komentar/kurung tidak
//!    ditutup, koma salah tempat, salah ketik kata kunci) dan parser SQL lokal.
//! 2. [`semantic`] — dicocokkan dengan skema di cache: tabel, kolom, alias,
//!    kolom ambigu, jumlah nilai INSERT, jumlah kolom UNION.
//! 3. Validasi server (`connection::validate`) — statement di-*prepare* tanpa
//!    dieksekusi; hasilnya digabung lewat [`merge`].
//! 4. Error eksekusi terakhir — dipetakan ke teks lewat [`locate`].
//!
//! Prinsip: lebih baik tidak menandai daripada salah menandai. Metadata yang
//! belum ada di cache berarti tidak ada tanda, dan hasil lokal yang bisa
//! keliru (`DiagSource::is_tentative`) ditahan sampai server memastikannya
//! bila validasi server aktif.
//!
//! MongoDB ([`mongo`]) dan Redis ([`redis`]) punya pemeriksa sendiri.

pub mod locate;
pub mod mongo;
pub mod redis;
pub mod semantic;
pub mod syntax;
mod words;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use crate::autocomplete::lexer::Dialect;
use crate::query_tools::{LintMessage, LintSeverity};

/// Asal satu diagnostik; menentukan seberapa yakin tanda itu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DiagSource {
    /// Saran gaya/performa (SELECT *, index).
    #[default]
    Lint,
    /// Kesalahan leksikal pasti (string/kurung tidak ditutup, koma).
    Syntax,
    /// Parser SQL lokal; dialek server bisa lebih longgar.
    Parser,
    /// Dicocokkan dengan skema di cache.
    Schema,
    /// Dilaporkan server saat prepare (tanpa eksekusi).
    Server,
    /// Error dari eksekusi terakhir.
    Execution,
}

impl DiagSource {
    /// Hasil lokal yang bisa keliru (dialek, cache basi); server menang.
    pub fn is_tentative(self) -> bool {
        matches!(self, DiagSource::Parser | DiagSource::Schema)
    }

    /// Hasil lokal yang dibatalkan bila server menyatakan statement valid.
    pub fn is_local_check(self) -> bool {
        matches!(
            self,
            DiagSource::Syntax | DiagSource::Parser | DiagSource::Schema
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            DiagSource::Lint => "Lint",
            DiagSource::Syntax => "Syntax",
            DiagSource::Parser => "Parser",
            DiagSource::Schema => "Schema",
            DiagSource::Server => "Server",
            DiagSource::Execution => "Last run",
        }
    }
}

/// Jenis kesalahan (untuk test, filter, dan quick fix).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagCode {
    UnterminatedString,
    UnterminatedIdentifier,
    UnterminatedComment,
    UnbalancedParen,
    StrayComma,
    UnknownStatement,
    KeywordTypo,
    SyntaxError,
    UnknownTable,
    UnknownColumn,
    UnknownQualifier,
    AliasRequired,
    AmbiguousColumn,
    IdentifierCase,
    ValueCountMismatch,
    DuplicateColumn,
    SetOpColumnCount,
    StringAsIdentifier,
    InvalidJson,
    InvalidQuery,
    UnknownOperator,
    UnknownCollection,
    UnknownCommand,
    UnsupportedCommand,
    WrongArgCount,
    Server,
    Execution,
}

/// Bahasa teks di editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryLanguage {
    Sql(Dialect),
    Mongo,
    Redis,
}

/// Skema yang diketahui untuk koneksi/database tab, diambil dari cache
/// in-memory. Semua pencarian tidak peka huruf besar kecuali disebut lain.
#[derive(Clone, Debug, Default)]
pub struct SchemaSnapshot {
    /// Tabel dan view di database aktif; `None` = belum diketahui (tidak ada
    /// tanda "tabel tidak ada").
    pub tables: Option<Vec<String>>,
    /// Kolom per tabel (key lowercase), hanya tabel yang sudah dimuat.
    pub columns: HashMap<String, Vec<String>>,
    /// Qualifier yang sama dengan "tanpa qualifier" (schema default,
    /// database aktif). Tabel ber-qualifier lain tidak diperiksa.
    pub default_schemas: Vec<String>,
    /// Collection MongoDB yang diketahui; `None` = belum diketahui.
    pub collections: Option<Vec<String>>,
}

impl SchemaSnapshot {
    /// Nama tabel persis seperti di cache, bila ada.
    pub fn find_table(&self, name: &str) -> Option<&str> {
        self.tables
            .as_ref()?
            .iter()
            .find(|t| t.eq_ignore_ascii_case(name) || last_part(t).eq_ignore_ascii_case(name))
            .map(String::as_str)
    }

    pub fn columns_of(&self, table: &str) -> Option<&[String]> {
        self.columns
            .get(&table.to_ascii_lowercase())
            .or_else(|| self.columns.get(&last_part(table).to_ascii_lowercase()))
            .filter(|c| !c.is_empty())
            .map(Vec::as_slice)
    }

    pub fn is_default_schema(&self, schema: &str) -> bool {
        self.default_schemas
            .iter()
            .any(|s| s.eq_ignore_ascii_case(schema))
    }

    /// Nama ini dikenal sebagai tabel atau kolom mana pun (untuk menahan
    /// tanda salah ketik kata kunci pada nama yang sah).
    pub fn knows_name(&self, name: &str) -> bool {
        self.find_table(name).is_some()
            || self
                .columns
                .values()
                .any(|cols| cols.iter().any(|c| c.eq_ignore_ascii_case(name)))
    }
}

fn last_part(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Satu statement di teks editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatementInfo {
    /// Rentang byte isi statement (tanpa spasi tepi dan `;`).
    pub span: Range<usize>,
    /// Hash teks statement (kunci cache validasi server).
    pub hash: u64,
    /// Boleh dikirim ke server untuk di-prepare.
    pub validatable: bool,
}

/// Hasil analisis lokal satu teks.
#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub items: Vec<LintMessage>,
    pub statements: Vec<StatementInfo>,
    /// Tabel yang dirujuk tetapi kolomnya belum ada di cache; UI memuatnya
    /// di background lalu menganalisis ulang.
    pub missing_columns: Vec<String>,
}

/// Teks yang lebih besar dari ini tidak dianalisis (dump/skrip migrasi besar).
pub const MAX_ANALYZED_BYTES: usize = 512 * 1024;
/// Statement yang lebih besar dari ini tidak dikirim ke server.
pub const MAX_VALIDATED_BYTES: usize = 64 * 1024;

pub fn hash_text(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Buat satu diagnostik.
pub(crate) fn diag(
    severity: LintSeverity,
    code: DiagCode,
    source: DiagSource,
    span: Range<usize>,
    message: impl Into<String>,
) -> LintMessage {
    LintMessage {
        severity,
        message: message.into(),
        span: Some(span),
        code: Some(code),
        source,
        ..Default::default()
    }
}

/// Teks memakai variabel template (`{{KEY}}`, `${var}`) yang baru diganti
/// saat eksekusi: tidak bisa diparse atau dikirim ke server apa adanya.
pub fn has_template_vars(text: &str) -> bool {
    text.contains("{{") || text.contains("${")
}

/// Kata pertama statement (uppercase), melewati komentar.
pub(crate) fn first_word(stmt: &str, dialect: Dialect) -> String {
    use crate::autocomplete::lexer::{TokKind, tokenize};
    tokenize(stmt, dialect)
        .into_iter()
        .find(|t| t.kind != TokKind::Comment)
        .filter(|t| t.kind == TokKind::Word)
        .map(|t| t.text.to_ascii_uppercase())
        .unwrap_or_default()
}

/// Statement yang mengubah definisi objek (setelahnya, statement lain di
/// skrip yang sama bisa merujuk objek baru yang belum ada di server).
pub(crate) fn is_ddl_word(word: &str) -> bool {
    matches!(
        word,
        "CREATE"
            | "ALTER"
            | "DROP"
            | "RENAME"
            | "TRUNCATE"
            | "DECLARE"
            | "EXEC"
            | "EXECUTE"
            | "CALL"
            | "DO"
            | "BEGIN"
            | "IF"
            | "SOURCE"
            | "LOAD"
            | "IMPORT"
            | "ATTACH"
    )
}

pub(crate) fn is_dml_word(word: &str) -> bool {
    matches!(
        word,
        "SELECT" | "WITH" | "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "MERGE" | "VALUES"
    )
}

/// Rentang isi tiap statement. MsSQL juga dipisah pada baris `GO`.
pub fn statement_spans(text: &str, dialect: Dialect) -> Vec<Range<usize>> {
    let hash_comment = dialect == Dialect::MySql;
    let mut out = Vec::new();
    for span in crate::connection::sql::split_sql_spans(text, hash_comment) {
        if dialect != Dialect::MsSql {
            out.push(span.body);
            continue;
        }
        // Baris `GO` (boleh diikuti angka) memisah batch MsSQL.
        let body = span.body.clone();
        let mut start = body.start;
        let mut offset = body.start;
        for line in text[body.clone()].split_inclusive('\n') {
            let trimmed = line.trim();
            let is_go = trimmed.eq_ignore_ascii_case("GO")
                || (trimmed.len() > 2
                    && trimmed[..2].eq_ignore_ascii_case("GO")
                    && trimmed[2..].trim().chars().all(|c| c.is_ascii_digit()));
            if is_go {
                push_trimmed(text, start..offset, &mut out);
                start = offset + line.len();
            }
            offset += line.len();
        }
        push_trimmed(text, start..body.end, &mut out);
    }
    out
}

fn push_trimmed(text: &str, range: Range<usize>, out: &mut Vec<Range<usize>>) {
    if range.start >= range.end {
        return;
    }
    let slice = &text[range.clone()];
    let lead = slice.len() - slice.trim_start().len();
    let trail = slice.len() - slice.trim_end().len();
    if lead + trail < slice.len() {
        out.push(range.start + lead..range.end - trail);
    }
}

/// Analisis lokal satu teks editor.
pub fn analyze(text: &str, lang: QueryLanguage, schema: &SchemaSnapshot) -> Analysis {
    if text.len() > MAX_ANALYZED_BYTES || text.trim().is_empty() {
        return Analysis::default();
    }
    match lang {
        QueryLanguage::Mongo => Analysis {
            items: mongo::check(text, schema),
            ..Default::default()
        },
        QueryLanguage::Redis => Analysis {
            items: redis::check(text),
            ..Default::default()
        },
        QueryLanguage::Sql(dialect) => analyze_sql(text, dialect, schema),
    }
}

fn analyze_sql(text: &str, dialect: Dialect, schema: &SchemaSnapshot) -> Analysis {
    let mut out = Analysis::default();
    // Skrip prosedural MySQL (`DELIMITER $$`) memakai pemisah lain: hanya
    // pemeriksaan leksikal yang aman.
    let procedural = text.lines().any(|l| {
        l.trim_start()
            .to_ascii_uppercase()
            .starts_with("DELIMITER ")
    });
    let templated = has_template_vars(text);
    let mut script = semantic::ScriptState::default();
    let mut missing = std::collections::BTreeSet::new();
    let mut ddl_seen = false;
    for span in statement_spans(text, dialect) {
        let stmt = &text[span.clone()];
        let word = first_word(stmt, dialect);
        let before = out.items.len();
        syntax::check(
            text,
            span.clone(),
            dialect,
            schema,
            procedural || templated,
            &mut out.items,
        );
        let syntax_errors = out.items[before..]
            .iter()
            .any(|m| m.severity == LintSeverity::Error);
        if !procedural && !templated && !syntax_errors {
            semantic::check(
                text,
                span.clone(),
                dialect,
                schema,
                &script,
                &mut out.items,
                &mut missing,
            );
        }
        semantic::record_definitions(stmt, dialect, &mut script);

        let upper = stmt.to_ascii_uppercase();
        let validatable = !procedural
            && !templated
            && !ddl_seen
            && is_dml_word(&word)
            && stmt.len() <= MAX_VALIDATED_BYTES
            // `SET NOEXEC OFF` di dalam teks bisa mematikan pengaman validasi MsSQL.
            && !(dialect == Dialect::MsSql
                && (upper.contains("NOEXEC") || upper.contains("PARSEONLY") || upper.contains("FMTONLY")))
            // Tabel sementara hidup di sesi lain, server validasi tidak melihatnya.
            && !(dialect == Dialect::MsSql && stmt.contains('#'))
            // `SELECT ... INTO` membuat tabel.
            && !(word == "SELECT" && upper.split_whitespace().any(|w| w == "INTO") && dialect != Dialect::MySql);
        if is_ddl_word(&word) || (word == "SELECT" && validatable_into(&upper, dialect)) {
            ddl_seen = true;
        }
        out.statements.push(StatementInfo {
            span: span.clone(),
            hash: hash_text(stmt),
            validatable,
        });
    }
    out.missing_columns = missing.into_iter().collect();
    out
}

/// `SELECT ... INTO tabel` (PG/MsSQL) membuat tabel baru.
fn validatable_into(upper: &str, dialect: Dialect) -> bool {
    dialect != Dialect::MySql && upper.split_whitespace().any(|w| w == "INTO")
}

/// Hasil validasi server satu statement.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerVerdict {
    /// Server menerima statement (prepare berhasil).
    Valid,
    /// Sintaks diterima, tetapi server tidak memeriksa nama objek (MsSQL
    /// `NOEXEC` dengan deferred name resolution): hanya hasil sintaks lokal
    /// yang dibatalkan.
    SyntaxValid,
    /// Server menolak; rentang relatif terhadap awal statement.
    Invalid(Vec<LintMessage>),
    /// Statement tidak bisa divalidasi (tidak didukung, koneksi gagal):
    /// hasil lokal dipakai apa adanya.
    Unavailable,
}

/// Gabungkan hasil lokal dengan hasil server.
///
/// `verdict(hash)` mengembalikan hasil server untuk statement itu bila sudah
/// ada. `server_active` = validasi server berjalan untuk tab ini; statement
/// yang masih menunggu hasilnya menahan diagnostik lokal yang tentatif.
pub fn merge<'a>(
    local: &[LintMessage],
    statements: &[StatementInfo],
    verdict: impl Fn(u64) -> Option<&'a ServerVerdict>,
    server_active: bool,
) -> Vec<LintMessage> {
    let mut out = Vec::new();
    let stmt_of = |pos: usize| {
        statements
            .iter()
            .find(|s| s.span.start <= pos && pos <= s.span.end)
    };
    let mut server_items: Vec<LintMessage> = Vec::new();
    for s in statements.iter().filter(|s| s.validatable) {
        if let Some(ServerVerdict::Invalid(items)) = verdict(s.hash) {
            for item in items {
                let mut item = item.clone();
                item.span = item.span.map(|r| {
                    (r.start + s.span.start).min(s.span.end)..(r.end + s.span.start).min(s.span.end)
                });
                server_items.push(item);
            }
        }
    }
    for item in local {
        let Some(span) = item.span.clone() else {
            out.push(item.clone());
            continue;
        };
        let Some(stmt) = stmt_of(span.start) else {
            // Statement-nya sedang diedit: hasil tentatif menunggu analisis baru.
            if !(server_active && item.source.is_tentative()) {
                out.push(item.clone());
            }
            continue;
        };
        if !stmt.validatable {
            out.push(item.clone());
            continue;
        }
        if !item.source.is_local_check() {
            out.push(item.clone());
            continue;
        }
        match verdict(stmt.hash) {
            Some(ServerVerdict::Valid) => {}
            Some(ServerVerdict::SyntaxValid) => {
                if item.source == DiagSource::Schema {
                    out.push(item.clone());
                }
            }
            Some(ServerVerdict::Invalid(_)) => {
                let overlaps = server_items.iter().any(|s| {
                    s.span.as_ref().is_some_and(|r| {
                        r.start < span.end.max(span.start + 1)
                            && span.start < r.end.max(r.start + 1)
                    })
                });
                if !overlaps {
                    out.push(item.clone());
                }
            }
            Some(ServerVerdict::Unavailable) => out.push(item.clone()),
            None if server_active && item.source.is_tentative() => {}
            None => out.push(item.clone()),
        }
    }
    out.extend(server_items);
    out.sort_by_key(|m| m.span.as_ref().map_or(usize::MAX, |r| r.start));
    out
}

/// Satu suntingan teks: bagian `[prefix, old_edit_end)` di teks lama diganti.
struct Edit {
    prefix: usize,
    old_edit_end: usize,
    delta: isize,
    touches_left: bool,
    touches_right: bool,
    new_len: usize,
}

impl Edit {
    fn between(old: &str, new: &str) -> Self {
        let prefix = old
            .bytes()
            .zip(new.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let max_suffix = old.len().min(new.len()) - prefix;
        let suffix = old
            .bytes()
            .rev()
            .zip(new.bytes().rev())
            .take(max_suffix)
            .take_while(|(a, b)| a == b)
            .count();
        let old_edit_end = old.len() - suffix;
        let new_edit_end = new.len() - suffix;
        let ident = |b: Option<&u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        // Token yang bersentuhan dengan suntingan ikut berubah (mis. huruf
        // ditambahkan di ujung nama kolom).
        let touches_left = ident(new.as_bytes().get(prefix)) || ident(old.as_bytes().get(prefix));
        let touches_right = prefix < new_edit_end.max(old_edit_end)
            && (new_edit_end > 0 && ident(new.as_bytes().get(new_edit_end - 1))
                || old_edit_end > 0 && ident(old.as_bytes().get(old_edit_end - 1)));
        Edit {
            prefix,
            old_edit_end,
            delta: new.len() as isize - old.len() as isize,
            touches_left,
            touches_right,
            new_len: new.len(),
        }
    }

    /// Rentang di teks baru, atau `None` bila rentang itu ikut tersunting.
    fn map(&self, span: &Range<usize>, new: &str) -> Option<Range<usize>> {
        if span.end < self.prefix || (span.end == self.prefix && !self.touches_left) {
            return Some(span.clone());
        }
        if span.start > self.old_edit_end
            || (span.start == self.old_edit_end && !self.touches_right)
        {
            let shift = |p: usize| (p as isize + self.delta).max(0) as usize;
            let (s, e) = (shift(span.start), shift(span.end).min(self.new_len));
            if s <= e && new.is_char_boundary(s) && new.is_char_boundary(e) {
                return Some(s..e);
            }
        }
        None
    }
}

/// Geser rentang diagnostik dari `old` ke `new` setelah satu suntingan;
/// diagnostik yang menyentuh bagian yang diubah dibuang.
pub fn rebase(old: &str, new: &str, items: &mut Vec<LintMessage>) {
    if old == new {
        return;
    }
    let edit = Edit::between(old, new);
    items.retain_mut(|m| {
        let Some(span) = m.span.clone() else {
            return true;
        };
        match edit.map(&span, new) {
            Some(r) => {
                m.span = Some(r);
                true
            }
            None => false,
        }
    });
}

/// Sama dengan [`rebase`] untuk rentang statement: statement yang tersunting
/// dibuang (hasil servernya tidak berlaku lagi), sisanya digeser.
pub fn rebase_statements(old: &str, new: &str, statements: &mut Vec<StatementInfo>) {
    if old == new {
        return;
    }
    let edit = Edit::between(old, new);
    statements.retain_mut(|s| match edit.map(&s.span, new) {
        Some(r) => {
            s.span = r;
            true
        }
        None => false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(span: Range<usize>, source: DiagSource) -> LintMessage {
        diag(
            LintSeverity::Error,
            DiagCode::UnknownColumn,
            source,
            span,
            "x",
        )
    }

    #[test]
    fn rebase_shifts_spans_after_the_edit_and_drops_touched_ones() {
        let old = "SELECT a FROM t WHERE b = 1";
        let new = "SELECT a, c FROM t WHERE b = 1";
        let mut items = vec![
            item(0..6, DiagSource::Syntax),
            item(7..8, DiagSource::Schema),
            item(22..23, DiagSource::Schema),
        ];
        rebase(old, new, &mut items);
        let spans: Vec<_> = items.iter().map(|m| m.span.clone().unwrap()).collect();
        assert_eq!(spans, vec![0..6, 7..8, 25..26]);
        assert_eq!(&new[25..26], "b");
    }

    #[test]
    fn rebase_drops_the_word_being_typed() {
        let mut items = vec![item(7..8, DiagSource::Schema)];
        rebase("SELECT a FROM t", "SELECT ab FROM t", &mut items);
        assert!(items.is_empty());
        let mut items = vec![item(7..8, DiagSource::Schema)];
        rebase("SELECT a FROM t", "SELECT xa FROM t", &mut items);
        assert!(items.is_empty());
    }

    #[test]
    fn rebase_statements_keeps_untouched_statements() {
        let old = "SELECT 1; SELECT 2";
        let new = "SELECT 10; SELECT 2";
        let mut stmts = vec![
            StatementInfo {
                span: 0..8,
                hash: 1,
                validatable: true,
            },
            StatementInfo {
                span: 10..18,
                hash: 2,
                validatable: true,
            },
        ];
        rebase_statements(old, new, &mut stmts);
        assert_eq!(stmts.len(), 1);
        assert_eq!(&new[stmts[0].span.clone()], "SELECT 2");
    }

    #[test]
    fn go_lines_split_mssql_batches() {
        let text = "SELECT 1\nGO\nSELECT 2\ngo 2\nSELECT 3";
        let spans = statement_spans(text, Dialect::MsSql);
        let parts: Vec<&str> = spans.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(parts, vec!["SELECT 1", "SELECT 2", "SELECT 3"]);
    }

    #[test]
    fn server_verdict_overrides_tentative_local_results() {
        let stmts = vec![StatementInfo {
            span: 0..20,
            hash: 1,
            validatable: true,
        }];
        let local = vec![item(7..8, DiagSource::Schema), item(0..3, DiagSource::Lint)];
        // Menunggu server: yang tentatif ditahan, lint tetap.
        let pending = merge(&local, &stmts, |_| None, true);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].source, DiagSource::Lint);
        // Server valid: hasil lokal dibuang.
        let valid = ServerVerdict::Valid;
        assert_eq!(merge(&local, &stmts, |_| Some(&valid), true).len(), 1);
        // Server menolak: error server menggantikan yang bertumpuk.
        let invalid = ServerVerdict::Invalid(vec![item(7..8, DiagSource::Server)]);
        let merged = merge(&local, &stmts, |_| Some(&invalid), true);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|m| m.source == DiagSource::Server));
        // Validasi server mati: semua hasil lokal tampil.
        assert_eq!(merge(&local, &stmts, |_| None, false).len(), 2);
    }

    #[test]
    fn ddl_makes_later_statements_local_only() {
        let text = "CREATE TABLE t (a int);\nSELECT a FROM t;";
        let a = analyze(
            text,
            QueryLanguage::Sql(Dialect::Postgres),
            &SchemaSnapshot::default(),
        );
        assert_eq!(a.statements.len(), 2);
        assert!(!a.statements[0].validatable);
        assert!(!a.statements[1].validatable);
    }
}
