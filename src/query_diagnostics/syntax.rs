//! Pemeriksaan sintaks satu statement: kesalahan leksikal yang pasti
//! (`DiagSource::Syntax`) dan error parser SQL per dialek (`DiagSource::Parser`).

use std::ops::Range;

use super::words;
use super::{DiagCode, DiagSource, SchemaSnapshot, diag};
use crate::autocomplete::lexer::{Dialect, TokKind, Token, tokenize};
use crate::query_tools::{LintMessage, LintSeverity};

/// Token statement dengan offset absolut terhadap `text`.
pub(crate) fn statement_tokens(text: &str, body: &Range<usize>, dialect: Dialect) -> Vec<Token> {
    let mut toks = tokenize(&text[body.clone()], dialect);
    for t in &mut toks {
        t.start += body.start;
        t.end += body.start;
    }
    toks
}

/// Rentang sampai akhir baris (untuk token yang tidak ditutup).
fn to_line_end(text: &str, start: usize, end: usize) -> Range<usize> {
    let line_end = text[start..end].find('\n').map_or(end, |p| start + p);
    start..line_end.max((start + 1).min(text.len()))
}

/// Periksa satu statement. `lexical_only` = hanya pemeriksaan leksikal
/// (teks bertemplate atau skrip prosedural).
pub fn check(
    text: &str,
    body: Range<usize>,
    dialect: Dialect,
    schema: &SchemaSnapshot,
    lexical_only: bool,
    out: &mut Vec<LintMessage>,
) {
    let all = statement_tokens(text, &body, dialect);
    if unterminated(text, &all, out) {
        return;
    }
    let toks: Vec<Token> = all
        .into_iter()
        .filter(|t| t.kind != TokKind::Comment)
        .collect();
    if toks.is_empty() {
        return;
    }
    let before = out.len();
    parens(&toks, out);
    if lexical_only {
        return;
    }
    commas(&toks, out);
    statement_start(&toks, out);
    let first = toks[0].text.to_ascii_uppercase();
    if super::is_dml_word(&first) {
        keyword_typos(&toks, schema, out);
    }
    // Error pasti atau salah ketik kata kunci sudah menjelaskan masalahnya;
    // error parser setelahnya hanya akibat lanjutan.
    let has_error = out[before..]
        .iter()
        .any(|m| m.severity == LintSeverity::Error || m.code == Some(DiagCode::KeywordTypo));
    if !has_error && super::is_dml_word(&first) {
        parse_with_sqlparser(text, &body, dialect, &toks, out);
    }
}

/// String, identifier ber-quote, atau komentar blok yang tidak ditutup.
fn unterminated(text: &str, toks: &[Token], out: &mut Vec<LintMessage>) -> bool {
    for t in toks.iter().filter(|t| !t.terminated) {
        let (code, message, hint) = match t.kind {
            TokKind::Str => (
                DiagCode::UnterminatedString,
                "Unterminated string literal",
                "Add the closing quote.",
            ),
            TokKind::QuotedIdent => (
                DiagCode::UnterminatedIdentifier,
                "Unterminated quoted identifier",
                "Add the closing quote or bracket.",
            ),
            TokKind::Comment => (
                DiagCode::UnterminatedComment,
                "Unterminated block comment",
                "Close the comment with */.",
            ),
            _ => continue,
        };
        let mut m = diag(
            LintSeverity::Error,
            code,
            DiagSource::Syntax,
            to_line_end(text, t.start, t.end),
            message,
        );
        m.hint = Some(hint.to_string());
        out.push(m);
        return true;
    }
    false
}

fn parens(toks: &[Token], out: &mut Vec<LintMessage>) {
    let mut stack: Vec<&Token> = Vec::new();
    for t in toks {
        match t.kind {
            TokKind::LParen => stack.push(t),
            TokKind::RParen if stack.pop().is_none() => {
                let mut m = diag(
                    LintSeverity::Error,
                    DiagCode::UnbalancedParen,
                    DiagSource::Syntax,
                    t.start..t.end,
                    "Unmatched closing parenthesis",
                );
                m.hint = Some("Remove it or add the opening \"(\".".to_string());
                out.push(m);
            }
            _ => {}
        }
    }
    for t in stack.into_iter().rev().take(3) {
        let mut m = diag(
            LintSeverity::Error,
            DiagCode::UnbalancedParen,
            DiagSource::Syntax,
            t.start..t.end,
            "Parenthesis is never closed",
        );
        m.hint = Some("Add the matching \")\".".to_string());
        out.push(m);
    }
}

/// Kata kunci yang tidak boleh langsung didahului koma.
const NO_COMMA_BEFORE: &[&str] = &[
    "FROM",
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "LIMIT",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "RETURNING",
    "WINDOW",
    "OFFSET",
];

fn commas(toks: &[Token], out: &mut Vec<LintMessage>) {
    for (i, t) in toks.iter().enumerate() {
        if t.kind != TokKind::Comma {
            continue;
        }
        let next = toks.get(i + 1);
        let prev = i.checked_sub(1).and_then(|p| toks.get(p));
        let message = match next {
            None => Some("Trailing comma at the end of the statement".to_string()),
            Some(n) if n.kind == TokKind::Comma => Some("Duplicate comma".to_string()),
            Some(n) if n.kind == TokKind::RParen => Some("Trailing comma before \")\"".to_string()),
            Some(n)
                if n.kind == TokKind::Word
                    && NO_COMMA_BEFORE
                        .iter()
                        .any(|k| n.text.eq_ignore_ascii_case(k)) =>
            {
                Some(format!(
                    "Trailing comma before {}",
                    n.text.to_ascii_uppercase()
                ))
            }
            _ => None,
        }
        .or_else(|| match prev {
            Some(p) if p.kind == TokKind::LParen => {
                Some("Unexpected comma after \"(\"".to_string())
            }
            Some(p) if p.is_kw("SELECT") || p.is_kw("BY") || p.is_kw("SET") => Some(format!(
                "Unexpected comma after {}",
                p.text.to_ascii_uppercase()
            )),
            _ => None,
        });
        if let Some(message) = message {
            let mut m = diag(
                LintSeverity::Error,
                DiagCode::StrayComma,
                DiagSource::Syntax,
                t.start..t.end,
                message,
            );
            m.hint = Some("Remove the comma or add the missing item.".to_string());
            m.suggestions = vec![String::new()];
            out.push(m);
        }
    }
}

/// Kata pertama statement yang mirip kata kunci awal (`SELEC`, `UPDTE`).
fn statement_start(toks: &[Token], out: &mut Vec<LintMessage>) {
    let t = &toks[0];
    if t.kind != TokKind::Word || words::is_statement_start(&t.text) {
        return;
    }
    // Label MsSQL/MySQL (`retry:`) dan kata yang tidak mirip apa pun dilewati.
    if toks.get(1).is_some_and(|n| n.is_op(":")) {
        return;
    }
    if let Some(k) = words::closest_statement_start(&t.text) {
        let mut m = diag(
            LintSeverity::Error,
            DiagCode::UnknownStatement,
            DiagSource::Syntax,
            t.start..t.end,
            format!("Unknown statement \"{}\"", t.text),
        );
        m.hint = Some(format!("Did you mean {k}?"));
        m.suggestions = vec![k.to_string()];
        out.push(m);
    }
}

fn ends_value(t: &Token) -> bool {
    match t.kind {
        TokKind::Word => !words::is_not_column(&t.text),
        TokKind::QuotedIdent | TokKind::RParen | TokKind::Number | TokKind::Str => true,
        TokKind::Op => t.text == "*",
        _ => false,
    }
}

/// `SELECT * FORM users`, `FROM t WEHRE id = 1`, `ODER BY`.
fn keyword_typos(toks: &[Token], schema: &SchemaSnapshot, out: &mut Vec<LintMessage>) {
    for i in 1..toks.len() {
        let t = &toks[i];
        if t.kind != TokKind::Word || t.text.len() < 4 || words::is_not_column(&t.text) {
            continue;
        }
        let Some(k) = words::closest_keyword(&t.text, words::CLAUSE_KEYWORDS) else {
            continue;
        };
        // `orders`, `groups`, `limits`: nama yang diawali kata kunci lebih
        // mungkin nama tabel/kolom daripada salah ketik.
        if t.text.to_ascii_uppercase().starts_with(k) {
            continue;
        }
        let prev = &toks[i - 1];
        let next = toks.get(i + 1);
        let next_ok = next.is_some_and(|n| {
            (matches!(n.kind, TokKind::Word | TokKind::QuotedIdent)
                && (!words::is_not_column(&n.text) || n.is_kw("BY")))
                || (n.kind == TokKind::Op && n.text == "*")
        });
        if !ends_value(prev) || !next_ok || prev.kind == TokKind::Dot || schema.knows_name(&t.text)
        {
            continue;
        }
        let mut m = diag(
            LintSeverity::Warning,
            DiagCode::KeywordTypo,
            DiagSource::Syntax,
            t.start..t.end,
            format!("\"{}\" looks like a misspelled {k}", t.text),
        );
        m.hint = Some(format!("Did you mean {k}?"));
        m.suggestions = vec![k.to_string()];
        out.push(m);
    }
}

/// Error parser per dialek. Hanya untuk DML; dialek server bisa menerima
/// sintaks yang tidak dikenal parser, jadi tingkatnya Warning dan sumbernya
/// tentatif (ditahan sampai server memastikan bila validasi server aktif).
#[cfg(feature = "sqlparser")]
fn parse_with_sqlparser(
    text: &str,
    body: &Range<usize>,
    dialect: Dialect,
    toks: &[Token],
    out: &mut Vec<LintMessage>,
) {
    use sqlparser::dialect::{
        Dialect as SqDialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect,
        SQLiteDialect,
    };
    use sqlparser::parser::{Parser, ParserError};
    let sq: Box<dyn SqDialect> = match dialect {
        Dialect::Postgres => Box::new(PostgreSqlDialect {}),
        Dialect::MySql => Box::new(MySqlDialect {}),
        Dialect::Sqlite => Box::new(SQLiteDialect {}),
        Dialect::MsSql => Box::new(MsSqlDialect {}),
        Dialect::Generic => Box::new(GenericDialect {}),
    };
    let stmt = &text[body.clone()];
    let err = match Parser::parse_sql(sq.as_ref(), stmt) {
        Ok(_) => return,
        Err(ParserError::ParserError(s)) | Err(ParserError::TokenizerError(s)) => s,
        Err(ParserError::RecursionLimitExceeded) => return,
    };
    let (message, location) = split_location(&err);
    let pos = location.and_then(|(line, col)| line_col_to_offset(stmt, line, col));
    let span = match pos {
        Some(p) => token_at(toks, body.start + p),
        None => toks.last().map(|t| t.start..t.end),
    };
    let Some(span) = span else {
        return;
    };
    let mut m = diag(
        LintSeverity::Warning,
        DiagCode::SyntaxError,
        DiagSource::Parser,
        span,
        format!("Syntax error: {}", tidy_parser_message(&message)),
    );
    m.hint = Some(
        "Reported by the local SQL parser; the server may accept some dialect extensions."
            .to_string(),
    );
    out.push(m);
}

#[cfg(not(feature = "sqlparser"))]
fn parse_with_sqlparser(
    _text: &str,
    _body: &Range<usize>,
    _dialect: Dialect,
    _toks: &[Token],
    _out: &mut Vec<LintMessage>,
) {
}

/// `"... at Line: 2, Column: 5"` → (pesan, Some((2, 5))).
pub(crate) fn split_location(err: &str) -> (String, Option<(usize, usize)>) {
    let Some(idx) = err.rfind(" at Line: ") else {
        return (err.to_string(), None);
    };
    let rest = &err[idx + " at Line: ".len()..];
    let mut parts = rest.split(", Column: ");
    let line = parts.next().and_then(|l| l.trim().parse().ok());
    let col = parts
        .next()
        .and_then(|c| c.trim().split(|ch: char| !ch.is_ascii_digit()).next())
        .and_then(|c| c.parse().ok());
    match (line, col) {
        (Some(l), Some(c)) if l > 0 && c > 0 => (err[..idx].to_string(), Some((l, c))),
        _ => (err[..idx].to_string(), None),
    }
}

/// Baris/kolom (1-based, kolom dalam karakter) → offset byte.
pub(crate) fn line_col_to_offset(text: &str, line: usize, col: usize) -> Option<usize> {
    let mut offset = 0;
    for (i, piece) in text.split_inclusive('\n').enumerate() {
        if i + 1 == line {
            let byte = piece
                .char_indices()
                .nth(col.saturating_sub(1))
                .map_or(piece.trim_end_matches('\n').len(), |(b, _)| b);
            return Some(offset + byte);
        }
        offset += piece.len();
    }
    None
}

/// Token yang memuat (atau pertama setelah) posisi `pos`.
pub(crate) fn token_at(toks: &[Token], pos: usize) -> Option<Range<usize>> {
    toks.iter()
        .find(|t| t.end > pos)
        .or(toks.last())
        .map(|t| t.start..t.end)
}

fn tidy_parser_message(msg: &str) -> String {
    let m = msg.trim();
    let m = m
        .strip_prefix("Expected: ")
        .map_or_else(|| m.to_string(), |rest| format!("expected {rest}"));
    match m.rsplit_once(", found: ") {
        Some((head, "EOF")) => format!("{head}, but the statement ended"),
        Some((head, found)) => format!("{head}, found \"{found}\""),
        None => m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sql: &str, dialect: Dialect) -> Vec<LintMessage> {
        let mut out = Vec::new();
        check(
            sql,
            0..sql.len(),
            dialect,
            &SchemaSnapshot::default(),
            false,
            &mut out,
        );
        out
    }

    fn codes(sql: &str) -> Vec<DiagCode> {
        run(sql, Dialect::Postgres)
            .into_iter()
            .filter_map(|m| m.code)
            .collect()
    }

    #[test]
    fn unterminated_tokens_are_errors() {
        assert_eq!(
            codes("SELECT 'abc FROM t"),
            vec![DiagCode::UnterminatedString]
        );
        assert_eq!(
            codes("SELECT \"abc FROM t"),
            vec![DiagCode::UnterminatedIdentifier]
        );
        assert_eq!(
            codes("SELECT 1 /* note"),
            vec![DiagCode::UnterminatedComment]
        );
    }

    #[test]
    fn unbalanced_parens_point_at_the_paren() {
        let sql = "SELECT COUNT(id FROM t";
        let out = run(sql, Dialect::Postgres);
        assert_eq!(out[0].code, Some(DiagCode::UnbalancedParen));
        assert_eq!(&sql[out[0].span.clone().unwrap()], "(");
        assert_eq!(codes("SELECT 1)"), vec![DiagCode::UnbalancedParen]);
    }

    #[test]
    fn stray_commas_are_found() {
        assert_eq!(codes("SELECT a, b, FROM t"), vec![DiagCode::StrayComma]);
        assert_eq!(codes("SELECT a,, b FROM t"), vec![DiagCode::StrayComma]);
        assert_eq!(
            codes("INSERT INTO t (a, b,) VALUES (1, 2)"),
            vec![DiagCode::StrayComma]
        );
        assert!(run("SELECT a, b FROM t LIMIT 10, 20", Dialect::MySql).is_empty());
    }

    #[test]
    fn statement_and_keyword_typos_are_suggested() {
        let out = run("SELEC * FROM t", Dialect::Postgres);
        assert_eq!(out[0].code, Some(DiagCode::UnknownStatement));
        assert_eq!(out[0].suggestions, vec!["SELECT".to_string()]);
        let out = run("SELECT * FORM users", Dialect::Postgres);
        assert!(
            out.iter().any(|m| m.code == Some(DiagCode::KeywordTypo)
                && m.suggestions == vec!["FROM".to_string()])
        );
    }

    #[test]
    fn valid_sql_has_no_diagnostics() {
        for sql in [
            "SELECT a, b FROM t WHERE a = 1 ORDER BY b DESC",
            "WITH c AS (SELECT id FROM t) SELECT * FROM c",
            "INSERT INTO t (a, b) VALUES (1, 'x'), (2, 'y') ON CONFLICT (a) DO NOTHING",
            "UPDATE t SET a = a + 1 WHERE b IS NOT NULL",
            "SELECT $$it's; fine$$, x::text FROM t",
        ] {
            assert!(
                run(sql, Dialect::Postgres).is_empty(),
                "{sql}: {:?}",
                run(sql, Dialect::Postgres)
            );
        }
        assert!(run("SELECT TOP 10 [a] FROM dbo.t WITH (NOLOCK)", Dialect::MsSql).is_empty());
        assert!(run("SELECT `a` FROM t LIMIT 5, 10", Dialect::MySql).is_empty());
    }

    #[cfg(feature = "sqlparser")]
    #[test]
    fn parser_error_points_at_the_offending_token() {
        let sql = "SELECT a FROM t WHERE";
        let out = run(sql, Dialect::Postgres);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code, Some(DiagCode::SyntaxError));
        assert_eq!(out[0].source, DiagSource::Parser);
    }

    #[test]
    fn location_is_parsed_from_parser_messages() {
        let (msg, loc) =
            split_location("Expected: an expression, found: FROM at Line: 2, Column: 8");
        assert_eq!(msg, "Expected: an expression, found: FROM");
        assert_eq!(loc, Some((2, 8)));
        assert_eq!(line_col_to_offset("ab\ncdef", 2, 3), Some(5));
        assert_eq!(
            tidy_parser_message("Expected: an expression, found: FROM"),
            "expected an expression, found \"FROM\""
        );
    }
}
