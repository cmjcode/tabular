//! Parameter query (`:nama`, `$1`, `?`) untuk editor.
//!
//! Modul ini headless: memindai placeholder di luar string/komentar, lalu
//! mengganti tiap placeholder berdasarkan posisinya dengan literal SQL yang
//! sudah di-escape sesuai dialek. Nilai tidak pernah ditempel mentah kecuali
//! pengguna memilih [`ParamKind::Raw`] secara eksplisit.

use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::sql_literal;
use std::ops::Range;

/// Satu kemunculan placeholder di teks SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    /// Rentang byte placeholder di SQL asal.
    pub range: Range<usize>,
    /// Nama tampilan: `:status`, `$1`, atau `? (Param 3)`.
    pub name: String,
}

/// Cara nilai parameter diterjemahkan menjadi SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParamKind {
    /// Tebak: `NULL`, angka, boolean, selain itu string.
    #[default]
    Auto,
    /// Selalu literal string.
    Text,
    /// Harus angka yang valid.
    Number,
    /// `NULL`, isi kolom nilai diabaikan.
    Null,
    /// Ditempel apa adanya (ekspresi SQL). Tidak di-escape.
    Raw,
}

impl ParamKind {
    pub const ALL: [ParamKind; 5] = [
        ParamKind::Auto,
        ParamKind::Text,
        ParamKind::Number,
        ParamKind::Null,
        ParamKind::Raw,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ParamKind::Auto => "Auto",
            ParamKind::Text => "Text",
            ParamKind::Number => "Number",
            ParamKind::Null => "NULL",
            ParamKind::Raw => "SQL expression",
        }
    }
}

/// Nilai yang diisi pengguna untuk satu nama parameter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParamValue {
    pub name: String,
    pub text: String,
    pub kind: ParamKind,
}

impl ParamValue {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: String::new(),
            kind: ParamKind::Auto,
        }
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Bila `sql[i..]` diawali tag dollar-quote PostgreSQL (`$$` atau `$tag$`),
/// kembalikan panjang tag pembuka.
fn dollar_tag_len(bytes: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if bytes.get(j).is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    while j < bytes.len() && is_ident_byte(bytes[j]) {
        j += 1;
    }
    (bytes.get(j) == Some(&b'$')).then(|| j + 1 - i)
}

/// Pindai semua placeholder di luar string, identifier berkutip, dan komentar.
pub fn scan(sql: &str) -> Vec<Placeholder> {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut out: Vec<Placeholder> = Vec::new();
    let mut unique: Vec<String> = Vec::new();
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();

        // Komentar baris.
        if (b == b'-' && next == Some(b'-')) || b == b'#' {
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Komentar blok.
        if b == b'/' && next == Some(b'*') {
            i += 2;
            while i < len && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(len);
            continue;
        }
        // String dan identifier berkutip.
        if b == b'\'' || b == b'"' || b == b'`' {
            let quote = b;
            i += 1;
            while i < len {
                if bytes[i] == b'\\' && quote != b'`' {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    if bytes.get(i + 1) == Some(&quote) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i = (i + 1).min(len);
            continue;
        }
        // Dollar-quote PostgreSQL atau parameter posisi `$1`.
        if b == b'$' {
            if let Some(tag_len) = dollar_tag_len(bytes, i) {
                let tag = &sql[i..i + tag_len];
                let body_start = i + tag_len;
                i = match sql[body_start..].find(tag) {
                    Some(pos) => body_start + pos + tag_len,
                    None => len,
                };
                continue;
            }
            if next.is_some_and(|c| c.is_ascii_digit()) {
                let start = i;
                i += 1;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                push(&mut out, &mut unique, start..i, sql[start..i].to_string());
                continue;
            }
            i += 1;
            continue;
        }
        // Cast PostgreSQL `::type`.
        if b == b':' && next == Some(b':') {
            i += 2;
            continue;
        }
        if b == b':' && next.is_some_and(|c| c.is_ascii_alphabetic() || c == b'_') {
            // `a[1:n]` bukan parameter: karakter sebelumnya bagian identifier/angka.
            let prev_is_ident = i > 0 && is_ident_byte(bytes[i - 1]);
            let start = i;
            i += 1;
            while i < len && is_ident_byte(bytes[i]) {
                i += 1;
            }
            if !prev_is_ident {
                push(&mut out, &mut unique, start..i, sql[start..i].to_string());
            }
            continue;
        }
        if b == b'?' {
            // Operator jsonb PostgreSQL `?|` dan `?&` bukan parameter.
            if matches!(next, Some(b'|') | Some(b'&')) {
                i += 2;
                continue;
            }
            let name = format!("? (Param {})", unique.len() + 1);
            push(&mut out, &mut unique, i..i + 1, name);
            i += 1;
            continue;
        }
        i += 1;
    }
    out
}

fn push(out: &mut Vec<Placeholder>, unique: &mut Vec<String>, range: Range<usize>, name: String) {
    if !unique.contains(&name) {
        unique.push(name.clone());
    }
    out.push(Placeholder { range, name });
}

/// Nama parameter unik sesuai urutan kemunculan.
pub fn names(sql: &str) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for p in scan(sql) {
        if !unique.contains(&p.name) {
            unique.push(p.name);
        }
    }
    unique
}

fn is_number(text: &str) -> bool {
    text.bytes().any(|b| b.is_ascii_digit())
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'-' | b'+'))
        && text.parse::<f64>().is_ok_and(|v| v.is_finite())
}

/// Terjemahkan satu nilai menjadi potongan SQL.
pub fn render_value(db: &DatabaseType, value: &ParamValue) -> Result<String, String> {
    let text = value.text.trim();
    match value.kind {
        ParamKind::Null => Ok("NULL".to_string()),
        ParamKind::Text => Ok(sql_literal(db, &value.text)),
        ParamKind::Number => {
            if is_number(text) {
                Ok(text.to_string())
            } else {
                Err(format!("{}: \"{}\" is not a number", value.name, text))
            }
        }
        ParamKind::Raw => {
            if text.is_empty() {
                Err(format!("{}: SQL expression is empty", value.name))
            } else {
                Ok(text.to_string())
            }
        }
        ParamKind::Auto => {
            if text.is_empty() {
                return Err(format!(
                    "{}: value is empty (choose NULL or Text for an empty value)",
                    value.name
                ));
            }
            if text.eq_ignore_ascii_case("null") {
                return Ok("NULL".to_string());
            }
            if is_number(text) {
                return Ok(text.to_string());
            }
            if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") {
                let truthy = text.eq_ignore_ascii_case("true");
                let rendered = match (db, truthy) {
                    (DatabaseType::MsSQL, true) => "1",
                    (DatabaseType::MsSQL, false) => "0",
                    (_, true) => "TRUE",
                    (_, false) => "FALSE",
                };
                return Ok(rendered.to_string());
            }
            Ok(sql_literal(db, &value.text))
        }
    }
}

/// Ganti setiap placeholder dengan nilainya. Gagal bila ada nilai yang tidak
/// valid atau placeholder tanpa nilai, sehingga query setengah jadi tidak pernah
/// dikirim ke server.
pub fn substitute(sql: &str, db: &DatabaseType, values: &[ParamValue]) -> Result<String, String> {
    let mut out = String::with_capacity(sql.len() + 32);
    let mut last = 0;
    for p in scan(sql) {
        let value = values
            .iter()
            .find(|v| v.name == p.name)
            .ok_or_else(|| format!("{}: no value provided", p.name))?;
        out.push_str(&sql[last..p.range.start]);
        out.push_str(&render_value(db, value)?);
        last = p.range.end;
    }
    out.push_str(&sql[last..]);
    Ok(out)
}

/// Ada nilai yang ditempel mentah, jadi hasil substitusi perlu dinilai ulang
/// oleh mode keamanan koneksi.
pub fn has_raw(values: &[ParamValue]) -> bool {
    values.iter().any(|v| v.kind == ParamKind::Raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn val(name: &str, text: &str, kind: ParamKind) -> ParamValue {
        ParamValue {
            name: name.to_string(),
            text: text.to_string(),
            kind,
        }
    }

    #[test]
    fn names_in_order_and_unique() {
        let sql =
            "SELECT * FROM users WHERE status = :status AND id = $1 AND name = ? AND s2 = :status";
        assert_eq!(names(sql), vec![":status", "$1", "? (Param 3)"]);
    }

    #[test]
    fn ignores_strings_comments_casts_and_dollar_quotes() {
        let sql = "SELECT ':a', \"b:c\", x::text, $$ :d ? $$, $fn$ :e $fn$ -- :f\n/* :g ? */ FROM t WHERE k = :real";
        assert_eq!(names(sql), vec![":real"]);
    }

    #[test]
    fn ignores_array_slice_and_jsonb_operators() {
        assert!(names("SELECT a[1:n], j ?| array['a'], j ?& array['b'] FROM t").is_empty());
    }

    #[test]
    fn string_values_are_escaped() {
        let out = substitute(
            "SELECT * FROM t WHERE name = :n",
            &DatabaseType::PostgreSQL,
            &[val(":n", "x'; DROP TABLE t; --", ParamKind::Auto)],
        )
        .unwrap();
        assert_eq!(out, "SELECT * FROM t WHERE name = 'x''; DROP TABLE t; --'");
    }

    #[test]
    fn prefix_names_do_not_collide() {
        let out = substitute(
            "SELECT :id, :id2",
            &DatabaseType::SQLite,
            &[
                val(":id", "1", ParamKind::Auto),
                val(":id2", "abc", ParamKind::Auto),
            ],
        )
        .unwrap();
        assert_eq!(out, "SELECT 1, 'abc'");
    }

    #[test]
    fn placeholder_text_inside_literal_is_untouched() {
        let out = substitute(
            "SELECT ':id' AS label, :id",
            &DatabaseType::MySQL,
            &[val(":id", "7", ParamKind::Auto)],
        )
        .unwrap();
        assert_eq!(out, "SELECT ':id' AS label, 7");
    }

    #[test]
    fn positional_question_marks_are_distinct() {
        let out = substitute(
            "SELECT ?, ?",
            &DatabaseType::MsSQL,
            &[
                val("? (Param 1)", "a", ParamKind::Auto),
                val("? (Param 2)", "true", ParamKind::Auto),
            ],
        )
        .unwrap();
        assert_eq!(out, "SELECT N'a', 1");
    }

    #[test]
    fn kinds_are_respected() {
        let db = DatabaseType::PostgreSQL;
        let r = |text: &str, kind| render_value(&db, &val(":a", text, kind));
        assert_eq!(r("42", ParamKind::Text).unwrap(), "'42'");
        assert_eq!(r("", ParamKind::Text).unwrap(), "''");
        assert_eq!(r("x", ParamKind::Null).unwrap(), "NULL");
        assert_eq!(r("-1.5e3", ParamKind::Number).unwrap(), "-1.5e3");
        assert!(r("1 OR 1=1", ParamKind::Number).is_err());
        assert_eq!(r("now()", ParamKind::Raw).unwrap(), "now()");
        assert!(r("", ParamKind::Auto).is_err());
        assert_eq!(r("NaN", ParamKind::Auto).unwrap(), "'NaN'");
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(substitute("SELECT :a", &DatabaseType::SQLite, &[]).is_err());
    }

    #[test]
    fn mysql_backslash_is_escaped() {
        let out = render_value(&DatabaseType::MySQL, &val(":a", "a\\'b", ParamKind::Text)).unwrap();
        assert_eq!(out, "'a\\\\''b'");
    }
}
