//! Lexer SQL yang toleran terhadap teks setengah jadi.
//!
//! Tidak pernah gagal: string/komentar/identifier ber-quote yang belum ditutup
//! diperlakukan memanjang sampai akhir teks dan ditandai `terminated = false`.
//! Offset `start`/`end` adalah offset byte ke teks asli.

/// Dialek SQL — hanya memengaruhi aturan quoting dan beberapa daftar fungsi.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dialect {
    #[default]
    Generic,
    MySql,
    Postgres,
    Sqlite,
    MsSql,
}

impl Dialect {
    /// Bungkus identifier dengan quote yang sesuai dialek.
    pub fn quote_ident(self, name: &str) -> String {
        match self {
            Dialect::MySql => format!("`{}`", name.replace('`', "``")),
            Dialect::MsSql => format!("[{}]", name.replace(']', "]]")),
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    /// Kata tanpa quote: keyword atau identifier.
    Word,
    /// Identifier ber-quote (`"x"`, `` `x` ``, `[x]`); `text` sudah tanpa quote.
    QuotedIdent,
    Str,
    Number,
    /// Parameter bind: `:name`, `@name`, `$1`, `?`.
    Param,
    Op,
    Comma,
    Dot,
    LParen,
    RParen,
    Semicolon,
    Comment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
    /// Teks token; untuk `QuotedIdent` berisi nama tanpa quote.
    pub text: String,
    /// `false` bila string/komentar/quote belum ditutup.
    pub terminated: bool,
}

impl Token {
    /// Cek apakah token adalah kata `kw` (case-insensitive).
    pub fn is_kw(&self, kw: &str) -> bool {
        self.kind == TokKind::Word && self.text.eq_ignore_ascii_case(kw)
    }

    pub fn is_op(&self, op: &str) -> bool {
        self.kind == TokKind::Op && self.text == op
    }
}

fn is_word_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_word_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Pecah `sql` menjadi token. Komentar ikut dikembalikan (kind `Comment`)
/// supaya pemanggil bisa tahu apakah kursor berada di dalam komentar.
pub fn tokenize(sql: &str, dialect: Dialect) -> Vec<Token> {
    let mut out = Vec::new();
    scan(sql, dialect, |kind, start, end, terminated| {
        out.push(Token {
            kind,
            start,
            end,
            text: token_text(sql, kind, start, end, terminated),
            terminated,
        });
        true
    });
    out
}

/// Teks token dari posisinya; identifier ber-quote di-unescape.
fn token_text(sql: &str, kind: TokKind, start: usize, end: usize, terminated: bool) -> String {
    match kind {
        TokKind::Comment => String::new(),
        TokKind::QuotedIdent => {
            let open = sql.as_bytes()[start];
            let q = if open == b'[' { ']' } else { open as char };
            let body_end = if terminated { end - 1 } else { end };
            sql[start + 1..body_end].replace(&format!("{q}{q}"), &q.to_string())
        }
        _ => sql[start..end].to_string(),
    }
}

/// Rentang byte statement (dipisah `;`) yang memuat `cursor`. Tanpa alokasi per
/// token, jadi murah untuk dokumen besar. Lexer tidak menyimpan state antar
/// token, sehingga melex ulang rentang ini memberi token yang identik.
pub fn statement_bounds(sql: &str, cursor: usize, dialect: Dialect) -> (usize, usize) {
    let mut s0 = 0;
    let mut s1 = sql.len();
    scan(sql, dialect, |kind, _start, end, _| {
        if kind != TokKind::Semicolon {
            return true;
        }
        if end <= cursor {
            s0 = end;
            true
        } else {
            s1 = end - 1;
            false
        }
    });
    (s0, s1)
}

/// Inti lexer: panggil `emit(kind, start, end, terminated)` per token;
/// berhenti bila `emit` mengembalikan `false`.
fn scan(sql: &str, dialect: Dialect, mut emit: impl FnMut(TokKind, usize, usize, bool) -> bool) {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    macro_rules! push {
        ($kind:expr, $start:expr, $end:expr, $closed:expr) => {
            if !emit($kind, $start, $end, $closed) {
                return;
            }
        };
    }

    while i < n {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;

        // Komentar baris: `--` dan (MySQL) `#`
        if (b == b'-' && i + 1 < n && bytes[i + 1] == b'-')
            || (b == b'#' && dialect == Dialect::MySql)
        {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            push!(TokKind::Comment, start, i, true);
            continue;
        }
        // Komentar blok
        if b == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            i += 2;
            let mut closed = false;
            while i + 1 < n {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                i = n;
            }
            push!(TokKind::Comment, start, i, closed);
            continue;
        }

        // String literal ('...' dengan escape '' dan, untuk MySQL, backslash)
        if b == b'\'' || (b == b'"' && dialect == Dialect::MySql) {
            let q = b;
            i += 1;
            let mut closed = false;
            while i < n {
                if bytes[i] == b'\\' && dialect == Dialect::MySql {
                    i += 2;
                    continue;
                }
                if bytes[i] == q {
                    if i + 1 < n && bytes[i + 1] == q {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            let i2 = i.min(n);
            push!(TokKind::Str, start, i2, closed);
            i = i2;
            continue;
        }

        // Identifier ber-quote
        let close_quote = match b {
            b'"' => Some(b'"'),
            b'`' => Some(b'`'),
            b'[' if matches!(dialect, Dialect::MsSql | Dialect::Sqlite | Dialect::Generic) => {
                Some(b']')
            }
            _ => None,
        };
        if let Some(cq) = close_quote {
            i += 1;
            let mut closed = false;
            while i < n {
                if bytes[i] == cq {
                    if i + 1 < n && bytes[i + 1] == cq {
                        i += 2;
                        continue;
                    }
                    closed = true;
                    break;
                }
                i += 1;
            }
            if closed {
                i += 1;
            }
            push!(TokKind::QuotedIdent, start, i, closed);
            continue;
        }

        // Dollar-quoting PostgreSQL: $tag$ ... $tag$ (bukan $1)
        if b == b'$'
            && dialect == Dialect::Postgres
            && !(i + 1 < n && bytes[i + 1].is_ascii_digit())
        {
            let mut j = i + 1;
            while j < n && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j < n && bytes[j] == b'$' {
                let tag = &sql[i..=j];
                let body = j + 1;
                let (end, closed) = match sql[body..].find(tag) {
                    Some(p) => (body + p + tag.len(), true),
                    None => (n, false),
                };
                push!(TokKind::Str, start, end, closed);
                i = end;
                continue;
            }
        }

        // Parameter bind
        if (b == b':'
            && i + 1 < n
            && is_word_start(bytes[i + 1])
            && !(i > 0 && bytes[i - 1] == b':'))
            || (b == b'@' && i + 1 < n && (is_word_start(bytes[i + 1]) || bytes[i + 1] == b'@'))
            || (b == b'$' && i + 1 < n && bytes[i + 1].is_ascii_digit())
        {
            i += 1;
            while i < n && (is_word_cont(bytes[i]) || bytes[i] == b'@') {
                i += 1;
            }
            push!(TokKind::Param, start, i, true);
            continue;
        }
        if b == b'?' {
            i += 1;
            push!(TokKind::Param, start, i, true);
            continue;
        }

        // Angka (termasuk desimal dan eksponen)
        if b.is_ascii_digit() || (b == b'.' && i + 1 < n && bytes[i + 1].is_ascii_digit()) {
            i += 1;
            while i < n
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                // eksponen bertanda: 1e-5
                if (bytes[i] == b'e' || bytes[i] == b'E')
                    && i + 1 < n
                    && (bytes[i + 1] == b'-' || bytes[i + 1] == b'+')
                {
                    i += 1;
                }
                i += 1;
            }
            push!(TokKind::Number, start, i, true);
            continue;
        }

        // Kata
        if is_word_start(b) {
            i += 1;
            while i < n && is_word_cont(bytes[i]) {
                i += 1;
            }
            // jaga batas char UTF-8
            while i < n && !sql.is_char_boundary(i) {
                i += 1;
            }
            push!(TokKind::Word, start, i, true);
            continue;
        }

        match b {
            b',' => push!(TokKind::Comma, start, start + 1, true),
            b'.' => push!(TokKind::Dot, start, start + 1, true),
            b'(' => push!(TokKind::LParen, start, start + 1, true),
            b')' => push!(TokKind::RParen, start, start + 1, true),
            b';' => push!(TokKind::Semicolon, start, start + 1, true),
            _ => {
                // Operator multi-karakter lebih dulu
                const MULTI: &[&str] = &[
                    "->>", "<=>", "#>>", "<>", "<=", ">=", "!=", "||", "::", "->", ":=", "==",
                    "#>", "@>", "<@",
                ];
                let rest = &sql[i..];
                let len = MULTI
                    .iter()
                    .find(|op| rest.starts_with(**op))
                    .map(|op| op.len())
                    .unwrap_or_else(|| rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1));
                push!(TokKind::Op, start, start + len, true);
                i = start + len;
                continue;
            }
        }
        i = start + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(sql: &str, d: Dialect) -> Vec<TokKind> {
        tokenize(sql, d).into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn basic_select() {
        let toks = tokenize(
            "SELECT u.id, 'x' FROM users u WHERE a >= 1.5",
            Dialect::Generic,
        );
        let texts: Vec<&str> = toks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "SELECT", "u", ".", "id", ",", "'x'", "FROM", "users", "u", "WHERE", "a", ">=",
                "1.5"
            ]
        );
    }

    #[test]
    fn quoting_per_dialect() {
        // MySQL: "..." adalah string, `...` identifier
        assert_eq!(
            kinds("\"a\" `b`", Dialect::MySql),
            vec![TokKind::Str, TokKind::QuotedIdent]
        );
        // Postgres: "..." identifier
        assert_eq!(
            kinds("\"a\"", Dialect::Postgres),
            vec![TokKind::QuotedIdent]
        );
        // MSSQL: [a b]
        let t = tokenize("[my table]", Dialect::MsSql);
        assert_eq!(t[0].kind, TokKind::QuotedIdent);
        assert_eq!(t[0].text, "my table");
    }

    #[test]
    fn unterminated_and_comments() {
        let t = tokenize("SELECT 'abc", Dialect::Generic);
        assert_eq!(t[1].kind, TokKind::Str);
        assert!(!t[1].terminated);
        let t = tokenize("a -- c\n b /* x", Dialect::Generic);
        assert_eq!(
            t.iter().map(|t| t.kind).collect::<Vec<_>>(),
            vec![
                TokKind::Word,
                TokKind::Comment,
                TokKind::Word,
                TokKind::Comment
            ]
        );
        assert!(!t[3].terminated);
    }

    #[test]
    fn statement_bounds_skip_semicolons_in_strings_and_comments() {
        let sql = "SELECT ';' FROM a; -- x;\nSELECT 1 /* ; */ FROM b; SELECT 2";
        let cur = sql.find("FROM b").unwrap();
        let (s0, s1) = statement_bounds(sql, cur, Dialect::Postgres);
        assert_eq!(&sql[s0..s1], " -- x;\nSELECT 1 /* ; */ FROM b");
        // kursor di statement terakhir tanpa `;` penutup
        let (s0, s1) = statement_bounds(sql, sql.len(), Dialect::Postgres);
        assert_eq!(&sql[s0..s1], " SELECT 2");
        // dollar-quote Postgres berisi `;`
        let pg = "DO $$ BEGIN x; END $$; SELECT";
        let (s0, _) = statement_bounds(pg, pg.len(), Dialect::Postgres);
        assert_eq!(&pg[s0..], " SELECT");
    }

    #[test]
    fn tokenize_slice_matches_full_tokenize() {
        let sql = "SELECT \"a\"\"b\", [x]; SELECT `q`, 'it''s' FROM t WHERE c = :p; SELECT 1";
        for d in [Dialect::Postgres, Dialect::MySql, Dialect::MsSql] {
            let full = tokenize(sql, d);
            let cur = sql.find("FROM t").unwrap();
            let (s0, s1) = statement_bounds(sql, cur, d);
            let part: Vec<Token> = tokenize(&sql[s0..s1], d)
                .into_iter()
                .map(|mut t| {
                    t.start += s0;
                    t.end += s0;
                    t
                })
                .collect();
            let expected: Vec<Token> = full
                .into_iter()
                .filter(|t| t.start >= s0 && t.end <= s1)
                .collect();
            assert_eq!(part, expected, "{d:?}");
        }
    }

    #[test]
    fn params_and_casts() {
        let t = tokenize("a = :id AND b::text = $1 AND c = @v", Dialect::Postgres);
        let params: Vec<&str> = t
            .iter()
            .filter(|t| t.kind == TokKind::Param)
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(params, vec![":id", "$1", "@v"]);
        assert!(t.iter().any(|t| t.is_op("::")));
    }
}
