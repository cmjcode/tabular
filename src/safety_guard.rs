//! Safety Guard module for Tabular.
//! Detects unsafe SQL operations (e.g. UPDATE or DELETE without a WHERE clause)
//! to prevent accidental mass modifications of database tables.

use log::warn;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsafeDmlReport {
    pub statement_type: &'static str, // "UPDATE" or "DELETE"
    pub table_name: Option<String>,
    pub snippet: String,
}

/// Analyze a SQL query to check if it contains unsafe DML statements (UPDATE or DELETE without a WHERE clause).
pub fn analyze_safety(sql: &str) -> Option<UnsafeDmlReport> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Split query by semicolon into separate statements
    let statements = split_statements_ignore_quotes(trimmed);

    for stmt in statements {
        let clean = strip_sql_comments(&stmt);
        let upper = clean.trim().to_ascii_uppercase();

        let stmt_type = if upper.starts_with("DELETE") {
            Some("DELETE")
        } else if upper.starts_with("UPDATE") {
            Some("UPDATE")
        } else {
            None
        };

        if let Some(kind) = stmt_type {
            // Check if top-level WHERE keyword exists (not inside subqueries or quotes)
            if !has_top_level_where(&clean) {
                let table_name = extract_target_table(&clean, kind);
                warn!("⚠️ Safety Guard: Unsafe {} detected without WHERE clause (table: {:?})", kind, table_name);
                return Some(UnsafeDmlReport {
                    statement_type: kind,
                    table_name,
                    snippet: stmt.chars().take(120).collect(),
                });
            }
        }
    }

    None
}

/// Check if a SQL statement contains a top-level WHERE clause.
fn has_top_level_where(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    let mut depth = 0;
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if b == b'(' {
            depth += 1;
            i += 1;
            continue;
        }
        if b == b')' {
            if depth > 0 {
                depth -= 1;
            }
            i += 1;
            continue;
        }

        if depth == 0 && (b.is_ascii_alphanumeric() || b == b'_') {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &sql[start..i];
            if word.eq_ignore_ascii_case("WHERE") {
                return true;
            }
            continue;
        }
        i += 1;
    }

    false
}

/// Extract target table name from DELETE FROM <table> or UPDATE <table>
fn extract_target_table(sql: &str, kind: &str) -> Option<String> {
    let tokens: Vec<&str> = sql.split_whitespace().collect();
    if kind == "DELETE" {
        // DELETE FROM <table> or DELETE <table>
        let mut iter = tokens.iter();
        while let Some(tok) = iter.next() {
            if tok.eq_ignore_ascii_case("DELETE") {
                if let Some(next) = iter.next() {
                    if next.eq_ignore_ascii_case("FROM") {
                        if let Some(tbl) = iter.next() {
                            return Some(clean_table_name(tbl));
                        }
                    } else {
                        return Some(clean_table_name(next));
                    }
                }
            }
        }
    } else if kind == "UPDATE" {
        // UPDATE <table> SET ...
        let mut iter = tokens.iter();
        while let Some(tok) = iter.next() {
            if tok.eq_ignore_ascii_case("UPDATE") {
                if let Some(tbl) = iter.next() {
                    return Some(clean_table_name(tbl));
                }
            }
        }
    }
    None
}

fn clean_table_name(raw: &str) -> String {
    raw.trim_matches(|c| c == ';' || c == '"' || c == '`' || c == '[' || c == ']' || c == '\'')
        .to_string()
}

fn strip_sql_comments(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut i = 0;

    while i < n {
        // Line comment --
        if bytes[i] == b'-' && i + 1 < n && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment /* */
        if bytes[i] == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        result.push(bytes[i] as char);
        i += 1;
    }

    result
}

fn split_statements_ignore_quotes(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;

    for c in sql.chars() {
        match c {
            '\'' if !in_double_quote && !in_backtick => {
                in_single_quote = !in_single_quote;
                current.push(c);
            }
            '"' if !in_single_quote && !in_backtick => {
                in_double_quote = !in_double_quote;
                current.push(c);
            }
            '`' if !in_single_quote && !in_double_quote => {
                in_backtick = !in_backtick;
                current.push(c);
            }
            ';' if !in_single_quote && !in_double_quote && !in_backtick => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    statements.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        statements.push(trimmed);
    }

    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unsafe_delete_without_where() {
        let sql = "DELETE FROM users;";
        let report = analyze_safety(sql);
        assert!(report.is_some());
        let r = report.unwrap();
        assert_eq!(r.statement_type, "DELETE");
        assert_eq!(r.table_name.as_deref(), Some("users"));
    }

    #[test]
    fn test_safe_delete_with_where() {
        let sql = "DELETE FROM users WHERE id = 42;";
        assert!(analyze_safety(sql).is_none());
    }

    #[test]
    fn test_unsafe_update_without_where() {
        let sql = "UPDATE orders SET status = 'cancelled';";
        let report = analyze_safety(sql);
        assert!(report.is_some());
        let r = report.unwrap();
        assert_eq!(r.statement_type, "UPDATE");
        assert_eq!(r.table_name.as_deref(), Some("orders"));
    }

    #[test]
    fn test_safe_update_with_where() {
        let sql = "UPDATE orders SET status = 'cancelled' WHERE total = 0;";
        assert!(analyze_safety(sql).is_none());
    }
}
