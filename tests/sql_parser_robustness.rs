//! Tes ketahanan untuk parser teks pengguna: masukan acak yang deterministik
//! (PRNG xorshift dengan seed tetap) tidak boleh membuat pemecah statement
//! panik, dan pemecah di editor tidak boleh menghilangkan isi selain spasi.
//!
//! Setiap masukan dijalankan di dalam `catch_unwind` supaya kegagalan
//! melaporkan teks pemicunya, bukan hanya pesan panik.

use std::panic::{AssertUnwindSafe, catch_unwind};

use tabular::connection::sql as exec_sql;
use tabular::models::enums::DatabaseType;
use tabular::query_tools::{
    self, duplicate_lines, find_statement_at_cursor, move_lines, split_statements,
    toggle_line_comments,
};

/// PRNG xorshift64*; cukup untuk masukan tes dan tidak butuh dependensi.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Potongan yang sering membuat pemecah statement salah langkah.
const TOKENS: &[&str] = &[
    "SELECT",
    "select",
    "INSERT INTO t VALUES",
    "UPDATE t SET a =",
    "FROM",
    "WHERE",
    "BEGIN",
    "END",
    "GO",
    "go 3",
    "DELIMITER //",
    "//",
    "CREATE FUNCTION f() RETURNS void AS",
    "LANGUAGE plpgsql",
    "1",
    "42",
    "a",
    "t",
    "x.y",
    "*",
    ",",
    "=",
    "+",
    " ",
    " ",
    "  ",
    "\t",
    "\n",
    "\n",
    "\r\n",
    "\r",
    ";",
    ";",
    ";;",
    "'",
    "''",
    "'a;b'",
    "'it''s'",
    "\"",
    "\"\"",
    "\"x;y\"",
    "`",
    "`a;b`",
    "[",
    "]",
    "[a;b]",
    "--",
    "-- c ;\n",
    "-",
    "#",
    "# c ;\n",
    "/*",
    "*/",
    "/* ; */",
    "/*/",
    "$$",
    "$tag$",
    "$1",
    "$",
    "$$ ; $$",
    "(",
    ")",
    "((",
    "))",
    "\\",
    "\\'",
    "\0",
    "é",
    "—",
    "İ",
    "ı",
    "ß",
    "😀",
    "👩‍👩‍👧",
    "e\u{301}",
    "\u{200B}",
    "\u{FEFF}",
    "\u{2028}",
    "\u{A0}",
    "日本語",
    "ﬁ",
    "E'\\n'",
    "N'x'",
    "X'00'",
    "0x1F",
    ":name",
    "@v",
    "{{KEY}}",
    "?",
];

fn random_sql(rng: &mut Rng) -> String {
    let mut out = String::new();
    let pieces = 1 + rng.below(40);
    for _ in 0..pieces {
        match rng.below(10) {
            // Sesekali karakter Unicode acak, termasuk yang lebar byte-nya beragam.
            0 => {
                let code = match rng.below(4) {
                    0 => rng.below(0x80) as u32,
                    1 => 0x80 + rng.below(0x780) as u32,
                    2 => 0x800 + rng.below(0xF000) as u32,
                    _ => 0x1_0000 + rng.below(0x1_0000) as u32,
                };
                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
            }
            _ => out.push_str(TOKENS[rng.below(TOKENS.len())]),
        }
    }
    out
}

fn inputs(seed: u64, count: usize) -> Vec<String> {
    let mut rng = Rng::new(seed);
    let mut all: Vec<String> = vec![
        String::new(),
        ";".into(),
        ";;;".into(),
        "'".into(),
        "\"".into(),
        "/*".into(),
        "--".into(),
        "$$".into(),
        "\0".into(),
        "é".into(),
        "😀;".into(),
        "SELECT 'é—İ😀' ; SELECT \"e\u{301}\" -- İ\n;".into(),
    ];
    all.extend((0..count).map(|_| random_sql(&mut rng)));
    all
}

/// Posisi kursor yang sah (batas karakter), termasuk awal dan akhir teks.
fn cursor(rng: &mut Rng, text: &str) -> usize {
    let mut boundaries: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    boundaries.push(text.len());
    boundaries[rng.below(boundaries.len())]
}

/// Jalankan `check` untuk tiap masukan dan kumpulkan yang panik atau gagal.
fn run_all(inputs: &[String], check: impl Fn(&str) -> Result<(), String>) {
    let mut failures = Vec::new();
    for sql in inputs {
        match catch_unwind(AssertUnwindSafe(|| check(sql))) {
            Ok(Ok(())) => {}
            Ok(Err(reason)) => failures.push(format!("{reason}: {sql:?}")),
            Err(_) => failures.push(format!("panicked: {sql:?}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} inputs failed; first cases:\n{}",
        failures.len(),
        inputs.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Sifat yang dijamin `split_statements`: rentang menaik, tidak tumpang
/// tindih, di batas karakter; `text` adalah isi rentang tanpa spasi tepi; dan
/// yang tertinggal di luar rentang hanya spasi.
fn check_split(sql: &str) -> Result<(), String> {
    let statements = split_statements(sql);
    let mut covered_to = 0usize;
    for s in &statements {
        let r = s.range.clone();
        if r.start < covered_to || r.start > r.end || r.end > sql.len() {
            return Err(format!("range {r:?} out of order or out of bounds"));
        }
        if !sql.is_char_boundary(r.start) || !sql.is_char_boundary(r.end) {
            return Err(format!("range {r:?} splits a character"));
        }
        if !sql[covered_to..r.start].trim().is_empty() {
            return Err(format!("content lost before {r:?}"));
        }
        if s.text.is_empty() || s.text != sql[r.clone()].trim() {
            return Err(format!("text does not match range {r:?}"));
        }
        if s.line_range.0 == 0 || s.line_range.0 > s.line_range.1 {
            return Err(format!("bad line range {:?}", s.line_range));
        }
        covered_to = r.end;
    }
    if !sql[covered_to..].trim().is_empty() {
        return Err("content lost after the last statement".to_string());
    }
    if statements.is_empty() != sql.trim().is_empty() {
        return Err("statement count disagrees with empty input".to_string());
    }
    Ok(())
}

#[test]
fn split_statements_never_panics_and_keeps_all_content() {
    run_all(&inputs(0x5EED_0001, 4000), check_split);
}

#[test]
fn find_statement_at_cursor_never_panics() {
    let all = inputs(0x5EED_0002, 2000);
    let rng = std::cell::RefCell::new(Rng::new(0xC0FFEE));
    run_all(&all, |sql| {
        let mut rng = rng.borrow_mut();
        let has_statement = !split_statements(sql).is_empty();
        let positions = [
            0,
            cursor(&mut rng, sql),
            cursor(&mut rng, sql),
            sql.len(),
            sql.len() + 7,
            usize::MAX,
        ];
        for pos in positions {
            let found = find_statement_at_cursor(sql, pos);
            if found.is_some() != has_statement {
                return Err(format!("cursor {pos}: found = {}", found.is_some()));
            }
        }
        Ok(())
    });
}

#[test]
fn executor_statement_helpers_never_panic() {
    run_all(&inputs(0x5EED_0003, 3000), |sql| {
        for hash_is_comment in [false, true] {
            let parts = exec_sql::split_sql_statements(sql, hash_is_comment);
            // Pemecah eksekutor tidak boleh menciptakan isi baru.
            let total: usize = parts.iter().map(String::len).sum();
            if total > sql.len() {
                return Err(format!(
                    "split_sql_statements returned {total} bytes from {}",
                    sql.len()
                ));
            }
        }
        let _ = exec_sql::split_mssql_go_batches(sql);
        let _ = exec_sql::strip_leading_sql_comments(sql);
        let _ = exec_sql::is_comment_only_statement(sql);
        let _ = exec_sql::statement_returns_rows(sql);
        let _ = exec_sql::query_contains_pagination(sql);
        let _ = exec_sql::should_enable_auto_pagination(sql);
        for db in [
            DatabaseType::MySQL,
            DatabaseType::PostgreSQL,
            DatabaseType::SQLite,
            DatabaseType::MsSQL,
        ] {
            let _ = exec_sql::add_auto_limit_if_needed(sql, &db);
        }
        Ok(())
    });
}

#[test]
fn lint_and_format_never_panic() {
    run_all(&inputs(0x5EED_0004, 1500), |sql| {
        let _ = query_tools::lint_sql(sql);
        let _ = query_tools::format_sql(sql);
        Ok(())
    });
}

#[test]
fn line_actions_never_panic_on_valid_cursor_positions() {
    let all = inputs(0x5EED_0005, 2000);
    let rng = std::cell::RefCell::new(Rng::new(0x0BAD_C0DE));
    run_all(&all, |sql| {
        let mut rng = rng.borrow_mut();
        let (a, b) = (cursor(&mut rng, sql), cursor(&mut rng, sql));
        for (start, end) in [(a, b), (b, a), (a, a), (0, sql.len())] {
            let _ = toggle_line_comments(sql, start, end);
            let _ = duplicate_lines(sql, start, end);
            let _ = move_lines(sql, start, end, true);
            let _ = move_lines(sql, start, end, false);
        }
        Ok(())
    });
}
