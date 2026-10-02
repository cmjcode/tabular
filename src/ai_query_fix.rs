//! Bantuan AI untuk editor SQL yang tidak butuh UI (headless):
//!
//! - K8 "Review with AI": menyusun prompt review untuk panel AI Assistant.
//! - K9 "Fix with AI": menyusun prompt perbaikan query yang gagal, mengambil
//!   statement hasil perbaikan dari jawaban model, dan membuat diff per baris
//!   antara SQL lama dan baru.
//!
//! Tidak ada yang dijalankan otomatis; UI hanya menampilkan diff lalu user
//! memilih Apply / Copy / Cancel.

use crate::models::enums::DatabaseType;

/// Batas SQL yang ikut ke prompt (byte).
pub const MAX_SQL_BYTES: usize = 16_000;
/// Batas pesan error yang ikut ke prompt (byte).
pub const MAX_ERROR_BYTES: usize = 4_000;
/// Batas sel matriks LCS; di atas ini diff jatuh ke "hapus semua + tambah semua".
const MAX_DIFF_CELLS: usize = 4_000_000;

/// Nama engine yang dipakai di prompt.
pub fn engine_label(db: &DatabaseType) -> &'static str {
    match db {
        DatabaseType::MySQL => "MySQL",
        DatabaseType::PostgreSQL => "PostgreSQL",
        DatabaseType::SQLite => "SQLite",
        DatabaseType::MsSQL => "SQL Server",
        DatabaseType::Redis => "Redis",
        DatabaseType::MongoDB => "MongoDB",
        DatabaseType::ApiHttp => "HTTP",
        DatabaseType::Plugin(_) => "SQL",
    }
}

/// Potong di batas karakter UTF-8; `true` bila ada yang dibuang.
fn clip(s: &str, max: usize) -> (&str, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

/// Pesan user untuk panel AI Assistant: review SQL dari sisi kebenaran,
/// performa, keamanan/risiko, dan gaya.
pub fn review_prompt(sql: &str, engine: Option<&str>) -> String {
    let (sql, clipped) = clip(sql.trim(), MAX_SQL_BYTES);
    let engine_line = match engine {
        Some(e) => format!("The target database is {e}.\n"),
        None => String::new(),
    };
    let clip_note = if clipped {
        "\n(The SQL was truncated because it is long.)\n"
    } else {
        ""
    };
    format!(
        "Review this SQL. {engine_line}\
         Cover, in this order, and keep each section short:\n\
         1. Correctness: logic errors, wrong joins or filters, NULL handling, edge cases.\n\
         2. Performance: indexes it needs, full scans, N+1 patterns, expensive functions on indexed columns.\n\
         3. Safety and risk: data-changing statements without WHERE, locking, injection risks, transaction concerns.\n\
         4. Style and readability.\n\
         If you suggest a rewrite, put it in one ```sql code block. Do not run anything.\n\n\
         ```sql\n{sql}\n```{clip_note}"
    )
}

/// System prompt untuk perbaikan query gagal: jawab hanya statement.
pub fn fix_system_prompt(engine: &str) -> String {
    format!(
        "You fix failing {engine} SQL statements. Reply with only the corrected statement in a \
         single ```sql code block and nothing else: no explanation, no comments outside the block. \
         Keep the author's intent, formatting, and aliases; change only what is needed to fix the \
         error. Use exact table and column names from the schema when it is given. Never add \
         statements that change data or schema that were not in the original."
    )
}

/// Pesan user untuk perbaikan: SQL gagal, pesan error, engine, dan skema.
pub fn fix_user_prompt(sql: &str, error: &str, engine: &str, schema: &str) -> String {
    let (sql, _) = clip(sql.trim(), MAX_SQL_BYTES);
    let (error, _) = clip(error.trim(), MAX_ERROR_BYTES);
    let mut out = format!(
        "This {engine} statement failed.\n\nStatement:\n```sql\n{sql}\n```\n\nError:\n{error}\n"
    );
    if !schema.trim().is_empty() {
        out.push_str("\nRelevant schema:\n");
        out.push_str(schema.trim());
        out.push('\n');
    }
    out.push_str("\nReturn the corrected statement only.");
    out
}

/// Ambil statement dari jawaban: blok ```sql pertama (atau blok tanpa bahasa);
/// bila tidak ada blok, seluruh teks. `None` bila kosong.
pub fn extract_fixed_sql(reply: &str) -> Option<String> {
    let mut in_block = false;
    let mut block = String::new();
    let mut found = false;
    for line in reply.lines() {
        let trimmed = line.trim();
        if !in_block && trimmed.starts_with("```") {
            let info = trimmed.trim_start_matches('`').trim().to_ascii_lowercase();
            if info.is_empty() || info.contains("sql") {
                in_block = true;
                found = true;
            }
            continue;
        }
        if in_block {
            if trimmed.starts_with("```") {
                break;
            }
            block.push_str(line);
            block.push('\n');
        }
    }
    let out = if found {
        block.trim().to_string()
    } else {
        reply.trim().to_string()
    };
    (!out.is_empty()).then_some(out)
}

/// Jenis baris pada diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Same,
    Removed,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

/// Diff per baris berbasis LCS antara `old` dan `new`.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let line = |kind, text: &str| DiffLine {
        kind,
        text: text.to_string(),
    };

    // Buang prefix & suffix yang sama supaya matriks kecil.
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let a_mid = &a[prefix..a.len() - suffix];
    let b_mid = &b[prefix..b.len() - suffix];

    let mut out: Vec<DiffLine> = a[..prefix]
        .iter()
        .map(|t| line(DiffKind::Same, t))
        .collect();

    let (n, m) = (a_mid.len(), b_mid.len());
    if n.saturating_mul(m) > MAX_DIFF_CELLS {
        out.extend(a_mid.iter().map(|t| line(DiffKind::Removed, t)));
        out.extend(b_mid.iter().map(|t| line(DiffKind::Added, t)));
    } else {
        // lcs[i][j] = panjang LCS a_mid[i..] dan b_mid[j..]
        let mut lcs = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i][j] = if a_mid[i] == b_mid[j] {
                    lcs[i + 1][j + 1] + 1
                } else {
                    lcs[i + 1][j].max(lcs[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a_mid[i] == b_mid[j] {
                out.push(line(DiffKind::Same, a_mid[i]));
                i += 1;
                j += 1;
            } else if lcs[i + 1][j] >= lcs[i][j + 1] {
                out.push(line(DiffKind::Removed, a_mid[i]));
                i += 1;
            } else {
                out.push(line(DiffKind::Added, b_mid[j]));
                j += 1;
            }
        }
        out.extend(a_mid[i..].iter().map(|t| line(DiffKind::Removed, t)));
        out.extend(b_mid[j..].iter().map(|t| line(DiffKind::Added, t)));
    }

    out.extend(
        a[a.len() - suffix..]
            .iter()
            .map(|t| line(DiffKind::Same, t)),
    );
    out
}

/// Ganti kemunculan pertama `original` di `text` dengan `fixed`. `None` bila
/// statement asli sudah tidak ada di teks (mis. editor sudah diubah).
pub fn replace_statement(text: &str, original: &str, fixed: &str) -> Option<String> {
    let original = original.trim();
    if original.is_empty() {
        return None;
    }
    let pos = text.find(original)?;
    Some(format!(
        "{}{}{}",
        &text[..pos],
        fixed,
        &text[pos + original.len()..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(diff: &[DiffLine]) -> Vec<String> {
        diff.iter()
            .map(|d| {
                let p = match d.kind {
                    DiffKind::Same => ' ',
                    DiffKind::Removed => '-',
                    DiffKind::Added => '+',
                };
                format!("{p}{}", d.text)
            })
            .collect()
    }

    #[test]
    fn diff_marks_changed_lines_only() {
        let old = "SELECT id,\n  nme\nFROM users\nWHERE id = 1";
        let new = "SELECT id,\n  name\nFROM users\nWHERE id = 1";
        assert_eq!(
            render(&line_diff(old, new)),
            [
                " SELECT id,",
                "-  nme",
                "+  name",
                " FROM users",
                " WHERE id = 1"
            ]
        );
    }

    #[test]
    fn diff_handles_insertions_deletions_and_empty() {
        assert_eq!(render(&line_diff("a\nc", "a\nb\nc")), [" a", "+b", " c"]);
        assert_eq!(render(&line_diff("a\nb\nc", "a\nc")), [" a", "-b", " c"]);
        assert_eq!(render(&line_diff("", "x")), ["+x"]);
        assert_eq!(render(&line_diff("x", "")), ["-x"]);
        assert!(line_diff("", "").is_empty());
        assert_eq!(render(&line_diff("same", "same")), [" same"]);
        assert_eq!(
            render(&line_diff("a\nb\nc\nd", "a\nx\nc\ny")),
            [" a", "-b", "+x", " c", "-d", "+y"]
        );
    }

    #[test]
    fn extract_prefers_sql_block() {
        let reply =
            "Here you go:\n```sql\nSELECT name FROM users;\n```\nThe column was misspelled.";
        assert_eq!(
            extract_fixed_sql(reply).as_deref(),
            Some("SELECT name FROM users;")
        );
        assert_eq!(
            extract_fixed_sql("```\nSELECT 1\n```").as_deref(),
            Some("SELECT 1")
        );
        assert_eq!(
            extract_fixed_sql("  SELECT 2;  ").as_deref(),
            Some("SELECT 2;")
        );
        assert_eq!(extract_fixed_sql("```sql\n```"), None);
        assert_eq!(extract_fixed_sql("   "), None);
    }

    #[test]
    fn replace_statement_only_first_occurrence() {
        let text = "SELECT 1;\nSELECT nme FROM t;\nSELECT nme FROM t;";
        let out = replace_statement(text, "SELECT nme FROM t;", "SELECT name FROM t;");
        assert_eq!(
            out.as_deref(),
            Some("SELECT 1;\nSELECT name FROM t;\nSELECT nme FROM t;")
        );
        assert_eq!(replace_statement(text, "DELETE FROM x", "y"), None);
        assert_eq!(replace_statement(text, "  ", "y"), None);
    }

    #[test]
    fn prompts_carry_context() {
        let p = fix_user_prompt(
            "SELECT nme FROM users",
            "column \"nme\" does not exist",
            "PostgreSQL",
            "- users(id int, name text)",
        );
        assert!(p.contains("SELECT nme FROM users"));
        assert!(p.contains("column \"nme\" does not exist"));
        assert!(p.contains("PostgreSQL"));
        assert!(p.contains("users(id int, name text)"));
        assert!(!fix_user_prompt("x", "e", "MySQL", "").contains("Relevant schema"));
        assert!(fix_system_prompt("MySQL").contains("only the corrected statement"));

        let r = review_prompt("SELECT * FROM t", Some("MySQL"));
        for needle in [
            "Correctness",
            "Performance",
            "Safety",
            "Style",
            "MySQL",
            "SELECT * FROM t",
        ] {
            assert!(r.contains(needle), "missing {needle}");
        }
        let long = "x".repeat(MAX_SQL_BYTES + 10);
        assert!(review_prompt(&long, None).contains("truncated"));
    }
}
