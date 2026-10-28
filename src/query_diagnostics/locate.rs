//! Petakan pesan error server ke rentang teks statement.
//!
//! Server memberi lokasi dengan cara berbeda: PostgreSQL offset karakter,
//! MySQL `near '...' at line N`, MsSQL `line: N` dan nama objek dalam kutip,
//! SQLite hanya nama (`no such column: u.x`, `near "FORM": syntax error`).
//! Semua bentuk itu diubah menjadi rentang token di dalam statement.

use std::ops::Range;

use crate::autocomplete::lexer::{Dialect, TokKind, Token, tokenize};

/// Lokasi yang diketahui dari server, relatif terhadap statement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerPosition {
    /// Offset karakter (0-based) di statement.
    pub char_offset: Option<usize>,
    /// Nomor baris (1-based) di statement.
    pub line: Option<usize>,
}

/// Nomor baris dari pesan MsSQL (`... (line: 3)` atau `line: 3]`).
pub fn mssql_error_line(message: &str) -> Option<usize> {
    let idx = message.rfind("line: ")?;
    message[idx + "line: ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
        .filter(|n| *n > 0)
}

/// Nama yang disebut pesan error, urut dari yang paling spesifik.
pub fn names_in_message(message: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: &str| {
        let s = s.trim().trim_end_matches(['.', ',', ';']);
        if !s.is_empty() && s.len() <= 256 && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    };
    // SQLite: `no such column: u.x`, `no such table: t`, `ambiguous column name: id`
    for marker in [
        "no such column: ",
        "no such table: ",
        "ambiguous column name: ",
        "no such function: ",
        "has no column named ",
    ] {
        if let Some(idx) = message.find(marker) {
            let rest = &message[idx + marker.len()..];
            push(
                rest.split(|c: char| c.is_whitespace() || c == ',' || c == '(')
                    .next()
                    .unwrap_or(""),
            );
        }
    }
    // Teks dalam kutip: 'x', "x", `x`
    for q in ['\'', '"', '`'] {
        let mut rest = message;
        while let Some(a) = rest.find(q) {
            let after = &rest[a + 1..];
            let Some(b) = after.find(q) else {
                break;
            };
            push(&after[..b]);
            rest = &after[b + 1..];
        }
    }
    out
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, TokKind::Word | TokKind::QuotedIdent)
}

/// Rentang di `stmt` untuk error ini, atau `None` bila tidak bisa ditentukan.
pub fn locate(
    stmt: &str,
    message: &str,
    pos: &ServerPosition,
    dialect: Dialect,
) -> Option<Range<usize>> {
    let toks: Vec<Token> = tokenize(stmt, dialect)
        .into_iter()
        .filter(|t| t.kind != TokKind::Comment)
        .collect();
    if toks.is_empty() {
        return None;
    }
    if let Some(offset) = pos.char_offset {
        let byte = stmt
            .char_indices()
            .nth(offset)
            .map_or(stmt.len(), |(b, _)| b);
        return toks
            .iter()
            .find(|t| t.end > byte)
            .or(toks.last())
            .map(|t| t.start..t.end);
    }
    let line_range = pos.line.and_then(|line| line_bounds(stmt, line));
    let in_line = |t: &Token| {
        line_range
            .as_ref()
            .is_none_or(|r| t.start >= r.start && t.start < r.end)
    };

    // MySQL: `near 'FORM users' at line 1` → awal teks itu.
    if let Some(near) = mysql_near(message) {
        if near.is_empty() {
            return toks.last().map(|t| t.start..t.end);
        }
        let start = match &line_range {
            Some(r) => stmt[r.start..].find(&near).map(|p| r.start + p),
            None => stmt.find(&near),
        }
        .or_else(|| stmt.find(&near));
        if let Some(start) = start {
            return toks.iter().find(|t| t.end > start).map(|t| t.start..t.end);
        }
    }
    // SQLite `incomplete input` / PostgreSQL `at end of input`.
    let lower = message.to_ascii_lowercase();
    if lower.contains("incomplete input") || lower.contains("end of input") {
        return toks.last().map(|t| t.start..t.end);
    }
    for name in names_in_message(message) {
        let parts: Vec<&str> = name.split('.').collect();
        let last = parts.last().copied().unwrap_or(&name);
        // Rantai lengkap dulu (`u.x`), lalu segmen terakhirnya.
        for (i, t) in toks.iter().enumerate() {
            if !is_name(t) || !in_line(t) {
                continue;
            }
            if parts.len() >= 2 && t.text.eq_ignore_ascii_case(parts[parts.len() - 2]) {
                if toks.get(i + 1).is_some_and(|d| d.kind == TokKind::Dot)
                    && toks
                        .get(i + 2)
                        .is_some_and(|c| c.text.eq_ignore_ascii_case(last))
                {
                    return Some(toks[i + 2].start..toks[i + 2].end);
                }
            }
        }
        let hit = toks
            .iter()
            .filter(|t| in_line(t))
            .find(|t| (is_name(t) || t.kind == TokKind::Op) && t.text.eq_ignore_ascii_case(last))
            .or_else(|| {
                toks.iter()
                    .find(|t| is_name(t) && t.text.eq_ignore_ascii_case(last))
            });
        if let Some(t) = hit {
            return Some(t.start..t.end);
        }
    }
    // Hanya baris yang diketahui: tandai isi baris itu.
    if let Some(r) = line_range {
        let slice = &stmt[r.clone()];
        let lead = slice.len() - slice.trim_start().len();
        let trail = slice.len() - slice.trim_end().len();
        if lead + trail < slice.len() {
            return Some(r.start + lead..r.end - trail);
        }
    }
    None
}

fn line_bounds(stmt: &str, line: usize) -> Option<Range<usize>> {
    let mut offset = 0;
    for (i, piece) in stmt.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            return Some(offset..offset + piece.trim_end_matches('\n').len());
        }
        offset += piece.len();
    }
    None
}

/// Teks setelah `near '` pada pesan MySQL/MariaDB (tanpa kutip penutup).
fn mysql_near(message: &str) -> Option<String> {
    let idx = message.find("near '")?;
    let rest = &message[idx + "near '".len()..];
    // Kutip penutup adalah yang diikuti " at line" (isi bisa memuat kutip).
    let end = rest.rfind("' at line").or_else(|| rest.rfind('\''))?;
    let near = &rest[..end];
    // MySQL memotong teks panjang; cukup awalnya untuk dicari.
    Some(
        near.chars()
            .take(40)
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

/// Bersihkan prefiks driver dari pesan server untuk tooltip.
pub fn tidy_server_message(message: &str) -> String {
    let mut m = message.trim();
    for prefix in [
        "Query error: ",
        "PostgreSQL error: ",
        "error returned from database: ",
        "Error: ",
    ] {
        if let Some(rest) = m.strip_prefix(prefix) {
            m = rest.trim_start();
        }
    }
    // MsSQL: `server error 207 (severity 16, state 1): Invalid column name 'x'. (line: 2)`
    let m = if let Some(rest) = m.strip_prefix("server error ") {
        rest.split_once("): ").map_or(m, |(_, msg)| msg)
    } else {
        m
    };
    // Lokasi MsSQL di akhir pesan tidak berguna di tooltip.
    let m = match m.rfind(" (line: ").or_else(|| m.rfind(" [line: ")) {
        Some(i) => &m[..i],
        None => m,
    };
    // MySQL: `1054 (42S22): Unknown column ...`
    let m = match m.split_once("): ") {
        Some((code, rest))
            if code.len() <= 16 && code.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
        {
            rest
        }
        _ => m,
    };
    m.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(stmt: &str, msg: &str, pos: ServerPosition, d: Dialect) -> Option<String> {
        locate(stmt, msg, &pos, d).map(|r| stmt[r].to_string())
    }

    #[test]
    fn postgres_offset_points_at_the_token() {
        let stmt = "SELECT nmae FROM users";
        let pos = ServerPosition {
            char_offset: Some(7),
            line: None,
        };
        assert_eq!(
            at(
                stmt,
                "column \"nmae\" does not exist",
                pos,
                Dialect::Postgres
            )
            .as_deref(),
            Some("nmae")
        );
    }

    #[test]
    fn mysql_near_text_is_found() {
        let stmt = "SELECT *\nFORM users";
        let msg =
            "You have an error in your SQL syntax; check the manual near 'FORM users' at line 2";
        let pos = ServerPosition {
            char_offset: None,
            line: Some(2),
        };
        assert_eq!(at(stmt, msg, pos, Dialect::MySql).as_deref(), Some("FORM"));
        let msg = "1054 (42S22): Unknown column 'u.nmae' in 'field list'";
        assert_eq!(
            at(
                "SELECT u.nmae FROM users u",
                msg,
                ServerPosition::default(),
                Dialect::MySql
            )
            .as_deref(),
            Some("nmae")
        );
    }

    #[test]
    fn sqlite_and_mssql_names_are_found() {
        assert_eq!(
            at(
                "SELECT x FROM t",
                "no such column: x",
                ServerPosition::default(),
                Dialect::Sqlite
            )
            .as_deref(),
            Some("x")
        );
        assert_eq!(
            at(
                "SELECT * FORM t",
                "near \"FORM\": syntax error",
                ServerPosition::default(),
                Dialect::Sqlite
            )
            .as_deref(),
            Some("FORM")
        );
        let msg = "server error 207 (severity 16, state 1): Invalid column name 'nmae'. (line: 2)";
        assert_eq!(mssql_error_line(msg), Some(2));
        let pos = ServerPosition {
            char_offset: None,
            line: mssql_error_line(msg),
        };
        assert_eq!(
            at("SELECT id\n, nmae FROM t", msg, pos, Dialect::MsSql).as_deref(),
            Some("nmae")
        );
        assert_eq!(tidy_server_message(msg), "Invalid column name 'nmae'.");
    }

    #[test]
    fn line_only_marks_the_line() {
        let pos = ServerPosition {
            char_offset: None,
            line: Some(2),
        };
        assert_eq!(
            at(
                "SELECT 1\n  WHERE x\n",
                "Incorrect syntax.",
                pos,
                Dialect::MsSql
            )
            .as_deref(),
            Some("WHERE x")
        );
    }
}
