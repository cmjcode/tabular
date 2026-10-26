use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlStatementSpan {
    pub text: String,
    pub range: Range<usize>,
    pub line_range: (usize, usize),
}

/// Splits SQL string into discrete statements respecting comments and quoted literals.
pub fn split_statements(sql: &str) -> Vec<SqlStatementSpan> {
    // Memakai lexer bersama agar editor, eksekusi, dan safety guard selalu
    // memecah skrip dengan cara yang sama.
    // Segmen yang hanya berisi `;` atau komentar tetap dikembalikan supaya
    // tidak ada teks di luar rentang mana pun.
    crate::connection::sql::split_sql_segments(sql, false)
        .into_iter()
        .filter(|seg| !sql[seg.raw.clone()].trim().is_empty())
        .map(|seg| {
            let range = seg.raw;
            let start_line = sql[..range.start].matches('\n').count() + 1;
            let end_line = sql[..range.end].matches('\n').count() + 1;
            SqlStatementSpan {
                text: sql[range.clone()].trim().to_string(),
                range,
                line_range: (start_line, end_line),
            }
        })
        .collect()
}

/// Finds the SQL statement span containing or closest to the given cursor position.
pub fn find_statement_at_cursor(sql: &str, cursor_pos: usize) -> Option<SqlStatementSpan> {
    let statements = split_statements(sql);
    if statements.is_empty() {
        return None;
    }

    // Direct containment: cursor within span.range (inclusive of boundary)
    for stmt in &statements {
        if cursor_pos >= stmt.range.start && cursor_pos <= stmt.range.end {
            return Some(stmt.clone());
        }
    }

    // If cursor is before first statement
    if cursor_pos < statements[0].range.start {
        return Some(statements[0].clone());
    }

    // If cursor is in whitespace between statements, pick previous statement if closer or next statement
    for i in 0..statements.len() - 1 {
        if cursor_pos > statements[i].range.end && cursor_pos < statements[i + 1].range.start {
            return Some(statements[i].clone());
        }
    }

    // Otherwise return the last statement
    statements.last().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_lexer_with_execution_for_dollar_quotes_and_backticks() {
        // Dulu pemecah ini tidak mengenal dollar-quote dan backtick sehingga
        // "statement di kursor" berbeda dengan yang dieksekusi.
        let sql = "CREATE FUNCTION f() RETURNS int AS $$ SELECT 1; $$ LANGUAGE sql;\nSELECT `a;b` FROM t;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].text.ends_with("LANGUAGE sql;"));
        assert_eq!(stmts[1].text, "SELECT `a;b` FROM t;");
        assert_eq!(stmts[1].line_range, (1, 2));
    }

    #[test]
    fn test_split_simple_statements() {
        let sql = "SELECT 1;\nSELECT 2;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].text, "SELECT 1;");
        assert_eq!(stmts[1].text, "SELECT 2;");
    }

    #[test]
    fn test_semicolon_inside_string_and_comment() {
        let sql = "SELECT 'hello; world' AS greeting; -- comment with ; semicolon\nSELECT /* comment ; */ 42;";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].text, "SELECT 'hello; world' AS greeting;");
        assert!(stmts[1].text.contains("42;"));
    }

    #[test]
    fn test_statement_without_trailing_semicolon() {
        let sql = "SELECT 1;\nSELECT 2";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0].text, "SELECT 1;");
        assert_eq!(stmts[1].text, "SELECT 2");
    }

    #[test]
    fn test_find_statement_at_cursor() {
        let sql = "SELECT 1;\n\nSELECT 2;\n\nSELECT 3;";
        // Cursor inside first statement
        let stmt1 = find_statement_at_cursor(sql, 4).unwrap();
        assert_eq!(stmt1.text, "SELECT 1;");

        // Cursor inside second statement
        let stmt2 = find_statement_at_cursor(sql, 14).unwrap();
        assert_eq!(stmt2.text, "SELECT 2;");

        // Cursor at the end of sql
        let stmt3 = find_statement_at_cursor(sql, sql.len()).unwrap();
        assert_eq!(stmt3.text, "SELECT 3;");
    }
}
