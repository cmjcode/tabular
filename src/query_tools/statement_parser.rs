use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlStatementSpan {
    pub text: String,
    pub range: Range<usize>,
    pub line_range: (usize, usize),
}

/// Splits SQL string into discrete statements respecting comments and quoted literals.
pub fn split_statements(sql: &str) -> Vec<SqlStatementSpan> {
    let mut statements = Vec::new();
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let len = chars.len();

    if len == 0 {
        return statements;
    }

    let mut stmt_start_char_idx = 0;
    let mut i = 0;

    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while i < len {
        let (byte_idx, ch) = chars[i];
        let next_ch = if i + 1 < len { Some(chars[i + 1].1) } else { None };

        if in_single_quote {
            if ch == '\'' {
                if next_ch == Some('\'') {
                    // Escaped single quote: ''
                    i += 2;
                    continue;
                } else {
                    in_single_quote = false;
                }
            }
            i += 1;
            continue;
        }

        if in_double_quote {
            if ch == '"' {
                if next_ch == Some('"') {
                    i += 2;
                    continue;
                } else {
                    in_double_quote = false;
                }
            }
            i += 1;
            continue;
        }

        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }

        if in_block_comment {
            if ch == '*' && next_ch == Some('/') {
                in_block_comment = false;
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }

        // Check for starting comments
        if ch == '-' && next_ch == Some('-') {
            in_line_comment = true;
            i += 2;
            continue;
        }

        if ch == '/' && next_ch == Some('*') {
            in_block_comment = true;
            i += 2;
            continue;
        }

        if ch == '\'' {
            in_single_quote = true;
            i += 1;
            continue;
        }

        if ch == '"' {
            in_double_quote = true;
            i += 1;
            continue;
        }

        if ch == ';' {
            let start_byte = chars[stmt_start_char_idx].0;
            let end_byte = byte_idx + ch.len_utf8();
            let raw_text = &sql[start_byte..end_byte];
            let trimmed = raw_text.trim();

            if !trimmed.is_empty() {
                let start_line = sql[..start_byte].chars().filter(|&c| c == '\n').count() + 1;
                let end_line = sql[..end_byte].chars().filter(|&c| c == '\n').count() + 1;

                statements.push(SqlStatementSpan {
                    text: trimmed.to_string(),
                    range: start_byte..end_byte,
                    line_range: (start_line, end_line),
                });
            }

            stmt_start_char_idx = i + 1;
        }

        i += 1;
    }

    // Trailing statement without trailing semicolon
    if stmt_start_char_idx < len {
        let start_byte = chars[stmt_start_char_idx].0;
        let end_byte = sql.len();
        let raw_text = &sql[start_byte..end_byte];
        let trimmed = raw_text.trim();

        if !trimmed.is_empty() {
            let start_line = sql[..start_byte].chars().filter(|&c| c == '\n').count() + 1;
            let end_line = sql[..end_byte].chars().filter(|&c| c == '\n').count() + 1;

            statements.push(SqlStatementSpan {
                text: trimmed.to_string(),
                range: start_byte..end_byte,
                line_range: (start_line, end_line),
            });
        }
    }

    statements
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
