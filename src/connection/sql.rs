use crate::models;
use log::debug;

/// Infer column origins (which table each column belongs to) from a SQL SELECT query.
pub(crate) fn infer_column_origins(query: &str) -> (Option<Vec<Option<String>>>, Vec<String>) {
    let dialect = sqlparser::dialect::MySqlDialect {};
    let ast = match sqlparser::parser::Parser::parse_sql(&dialect, query) {
        Ok(a) => a,
        Err(_) => return (None, Vec::new()),
    };

    let query_body = if let Some(sqlparser::ast::Statement::Query(q)) = ast.first() {
        &q.body
    } else {
        return (None, Vec::new());
    };

    if let sqlparser::ast::SetExpr::Select(select) = &**query_body {
        // 1. Build map of alias/table -> real_table_name
        let mut table_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut primary_table: Option<String> = None;
        let mut all_tables = Vec::new();

        for (i, table_with_join) in select.from.iter().enumerate() {
            let relation = &table_with_join.relation;
            if let sqlparser::ast::TableFactor::Table { name, alias, .. } = relation {
                let real_name = name.to_string();
                if i == 0 {
                    primary_table = Some(real_name.clone());
                }
                table_map.insert(real_name.clone(), real_name.clone());
                if !all_tables.contains(&real_name) {
                    all_tables.push(real_name.clone());
                }

                if let Some(a) = alias {
                    table_map.insert(a.name.value.clone(), real_name);
                }
            } else {
                // Derived tables etc not supported yet
            }
            for join in &table_with_join.joins {
                if let sqlparser::ast::TableFactor::Table { name, alias, .. } = &join.relation {
                    let real_name = name.to_string();
                    table_map.insert(real_name.clone(), real_name.clone());
                    if !all_tables.contains(&real_name) {
                        all_tables.push(real_name.clone());
                    }

                    if let Some(a) = alias {
                        table_map.insert(a.name.value.clone(), real_name);
                    }
                }
            }
        }

        // 2. Map projection items to tables
        let mut origins = Vec::new();
        for item in &select.projection {
            match item {
                sqlparser::ast::SelectItem::UnnamedExpr(expr)
                | sqlparser::ast::SelectItem::ExprWithAlias { expr, .. } => match expr {
                    sqlparser::ast::Expr::Identifier(_) => {
                        // Column without prefix. If only 1 table, assign it. Else ambiguous (None)
                        if table_map.len() == 1 {
                            origins.push(primary_table.clone());
                        } else {
                            origins.push(None);
                        }
                    }
                    sqlparser::ast::Expr::CompoundIdentifier(idents) => {
                        // t.col -> idents[0] is alias/table
                        if idents.len() >= 2 {
                            let prefix = idents[0].value.clone();
                            origins.push(table_map.get(&prefix).cloned());
                        } else {
                            origins.push(None);
                        }
                    }
                    _ => origins.push(None), // Functions, literals etc
                },
                sqlparser::ast::SelectItem::Wildcard(_opts) => {
                    origins.push(None);
                }
                sqlparser::ast::SelectItem::QualifiedWildcard(obj, _) => {
                    origins.push(table_map.get(&obj.to_string()).cloned());
                }
            }
        }
        (Some(origins), all_tables)
    } else {
        (None, Vec::new())
    }
}

fn keyword_in_sql(upper_sql: &str, keyword: &str) -> bool {
    let bytes = upper_sql.as_bytes();
    let key_bytes = keyword.as_bytes();
    let mut search_from = 0;
    while search_from + key_bytes.len() <= bytes.len() {
        if let Some(rel_pos) = upper_sql[search_from..].find(keyword) {
            let start = search_from + rel_pos;
            let end = start + key_bytes.len();

            let prev_is_ident = if start == 0 {
                false
            } else {
                let prev = bytes[start - 1];
                prev.is_ascii_alphanumeric() || prev == b'_'
            };
            let next_is_ident = match bytes.get(end) {
                Some(next) => next.is_ascii_alphanumeric() || *next == b'_',
                None => false,
            };
            if !prev_is_ident && !next_is_ident {
                return true;
            }
            search_from = end;
        } else {
            break;
        }
    }
    false
}

pub fn query_contains_pagination(sql: &str) -> bool {
    let upper = sql.to_uppercase();
    let upper_ref = upper.as_str();
    keyword_in_sql(upper_ref, "LIMIT")
        || keyword_in_sql(upper_ref, "OFFSET")
        || keyword_in_sql(upper_ref, "FETCH")
        || keyword_in_sql(upper_ref, "TOP")
        || upper_ref.contains("FETCH NEXT")
        || upper_ref.contains("FETCH FIRST")
        || upper_ref.contains("FETCH PRIOR")
        || upper_ref.contains("FETCH ROW")
        || upper_ref.contains("FETCH ROWS")
}

fn normalize_sql_token(token: &str) -> String {
    token
        .trim_matches(|c: char| {
            matches!(
                c,
                ',' | ';' | '(' | ')' | '`' | '"' | '\'' | '[' | ']' | '{' | '}'
            )
        })
        .to_uppercase()
}

fn is_reserved_after_from(token: &str) -> bool {
    matches!(
        token,
        "" | "WHERE"
            | "ORDER"
            | "GROUP"
            | "HAVING"
            | "LIMIT"
            | "OFFSET"
            | "FETCH"
            | "FOR"
            | "UNION"
            | "INTERSECT"
            | "EXCEPT"
            | "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "CROSS"
            | "ON"
            | "USING"
            | "WINDOW"
            | "TABLESAMPLE"
    )
}

fn contains_complex_keywords(tokens: &[String]) -> bool {
    for window in tokens.windows(2) {
        if window[0] == "GROUP" && window[1] == "BY" {
            return true;
        }
        if window[0] == "ORDER" && window[1] == "BY" {
            return true;
        }
    }

    tokens.iter().any(|tok| {
        matches!(
            tok.as_str(),
            "JOIN"
                | "WHERE"
                | "HAVING"
                | "UNION"
                | "INTERSECT"
                | "EXCEPT"
                | "WITH"
                | "LIMIT"
                | "OFFSET"
                | "FETCH"
                | "FOR"
                | "PIVOT"
                | "UNPIVOT"
                | "RETURNING"
        )
    })
}

fn select_has_alias_or_multiple_tables(raw_tokens: &[&str], normalized_tokens: &[String]) -> bool {
    let from_pos = normalized_tokens.iter().position(|tok| tok == "FROM");
    let Some(from_idx) = from_pos else {
        debug!(
            "🛑 select_has_alias: no FROM found in tokens: {:?}",
            normalized_tokens
        );
        return true; // Cannot determine target without FROM
    };

    if from_idx + 1 >= raw_tokens.len() {
        debug!("🛑 select_has_alias: missing table token after FROM");
        return true;
    }

    let table_token_raw = raw_tokens[from_idx + 1];
    let table_token_clean = table_token_raw
        .trim_matches(|c: char| matches!(c, ',' | ';' | '`' | '"' | '\'' | '[' | ']' | '{' | '}'))
        .trim();

    if table_token_clean.is_empty()
        || table_token_raw.contains(',')
        || table_token_raw.contains('(')
    {
        debug!(
            "🛑 select_has_alias: table segment unclear '{}', raw='{}'",
            table_token_clean, table_token_raw
        );
        return true;
    }

    if let Some(token_upper) = normalized_tokens.get(from_idx + 2) {
        if token_upper == "AS" {
            debug!("🛑 select_has_alias: alias via AS detected");
            return true;
        }
        if is_reserved_after_from(token_upper) {
            debug!(
                "✅ select_has_alias: reserved token '{}' stops scan",
                token_upper
            );
        } else {
            // Any non-reserved token after table indicates alias or additional tables
            debug!(
                "🛑 select_has_alias: token '{}' treated as alias/additional table",
                token_upper
            );
            return true;
        }
    }

    debug!(
        "✅ select_has_alias: no alias detected (tokens after FROM: {:?})",
        &normalized_tokens[from_idx..]
    );
    false
}

pub(crate) fn is_simple_select_statement(stmt: &str) -> bool {
    let raw_tokens: Vec<&str> = stmt.split_whitespace().collect();
    if raw_tokens.is_empty() {
        return false;
    }

    let normalized_tokens: Vec<String> = raw_tokens
        .iter()
        .map(|tok| normalize_sql_token(tok))
        .filter(|tok| !tok.is_empty())
        .collect();

    debug!(
        "🔍 is_simple_select_statement tokens raw={:?}, normalized={:?}",
        raw_tokens, normalized_tokens
    );

    if normalized_tokens.first().map(|tok| tok.as_str()) != Some("SELECT") {
        return false;
    }

    if contains_complex_keywords(&normalized_tokens) {
        debug!("🛑 is_simple_select_statement: complex keyword detected");
        return false;
    }

    if normalized_tokens.len() != 4 {
        debug!(
            "🛑 is_simple_select_statement: expected 4 tokens, got {}",
            normalized_tokens.len()
        );
        return false;
    }

    if normalized_tokens.get(1).map(|tok| tok.as_str()) != Some("*") {
        debug!("🛑 is_simple_select_statement: only SELECT * patterns are eligible");
        return false;
    }

    let result = !select_has_alias_or_multiple_tables(&raw_tokens, &normalized_tokens);
    debug!(
        "✅ is_simple_select_statement result={} for stmt='{}'",
        result, stmt
    );
    result
}

pub fn should_enable_auto_pagination(sql: &str) -> bool {
    if query_contains_pagination(sql) {
        debug!("🛑 should_enable_auto_pagination: pagination clause already present");
        return false;
    }

    let mut simple_select_count = 0;
    for stmt in sql.split(';') {
        let trimmed = stmt.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.trim_start().starts_with(['-', '#']) {
            continue;
        }

        if trimmed.to_uppercase().starts_with("SELECT") {
            let is_simple = is_simple_select_statement(trimmed);
            debug!(
                "🔍 should_enable_auto_pagination: stmt='{}' is_simple={}",
                trimmed, is_simple
            );
            if !is_simple {
                debug!("🛑 should_enable_auto_pagination: statement not simple enough");
                return false;
            }
            simple_select_count += 1;
            if simple_select_count > 1 {
                debug!("🛑 should_enable_auto_pagination: more than one SELECT statement");
                return false;
            }
        }
    }

    let enable = simple_select_count == 1;
    if enable {
        debug!(
            "✅ should_enable_auto_pagination: enabling for sql='{}'",
            sql
        );
    } else {
        debug!("ℹ️ should_enable_auto_pagination: no eligible SELECT statements detected");
    }
    enable
}

/// Infer column headers from a SELECT statement when no rows are returned.
/// This is a best-effort parser handling simple SELECT lists (supports aliases, functions, qualified names).
pub(crate) fn infer_select_headers(statement: &str) -> Vec<String> {
    let lower = statement.to_lowercase();
    let select_pos = match lower.find("select") {
        Some(p) => p,
        None => return Vec::new(),
    };
    // Find the matching FROM outside parentheses
    let mut depth = 0usize;
    let mut from_pos: Option<usize> = None;
    for (i, ch) in statement.chars().enumerate().skip(select_pos + 6) {
        // after 'select'
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && i + 4 <= statement.len() && lower[i..].starts_with("from") {
            from_pos = Some(i);
            break;
        }
    }
    let from_pos = match from_pos {
        Some(p) => p,
        None => return Vec::new(),
    };
    let select_list = &statement[select_pos + 6..from_pos];
    // Split by commas at top level (ignore commas inside parentheses)
    let mut headers = Vec::new();
    let mut current = String::new();
    depth = 0;
    for ch in select_list.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if depth == 0 => {
                let h = extract_alias_or_name(&current);
                if !h.is_empty() {
                    headers.push(h);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        let h = extract_alias_or_name(&current);
        if !h.is_empty() {
            headers.push(h);
        }
    }
    headers
}

fn extract_alias_or_name(fragment: &str) -> String {
    let frag = fragment.trim();
    if frag.is_empty() {
        return String::new();
    }
    let lower = frag.to_lowercase();
    if let Some(as_pos) = lower.rfind(" as ") {
        // alias with AS
        let alias = frag[as_pos + 4..].trim();
        return clean_identifier(alias);
    }
    // Alias without AS: take last token after space if it is not a function call
    let tokens: Vec<&str> = frag.split_whitespace().collect();
    if tokens.len() > 1 {
        let last = tokens.last().unwrap();
        // Avoid returning keywords or expressions
        if !last.contains('(') && !["distinct"].contains(&last.to_lowercase().as_str()) {
            return clean_identifier(last);
        }
    }
    // Otherwise, strip qualification
    if let Some(idx) = frag.rfind('.') {
        return clean_identifier(&frag[idx + 1..]);
    }
    clean_identifier(frag)
}

fn clean_identifier(id: &str) -> String {
    id.trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('[')
        .trim_matches(']')
        .to_string()
}

/// Add an auto LIMIT/TOP clause to a SELECT query if one is not already present.
pub fn add_auto_limit_if_needed(query: &str, db_type: &models::enums::DatabaseType) -> String {
    let trimmed_query = query.trim();

    // Don't add LIMIT/TOP if the entire query already has LIMIT/TOP/OFFSET/FETCH
    if query_contains_pagination(trimmed_query) {
        return trimmed_query.to_string();
    }

    // Only operate on simple SELECT queries
    let upper_query = trimmed_query.to_uppercase();
    if !upper_query.starts_with("SELECT") {
        return trimmed_query.to_string();
    }

    match db_type {
        models::enums::DatabaseType::MsSQL => {
            // Insert TOP 5000 after SELECT if no TOP present
            if upper_query.starts_with("SELECT") {
                if let Some(rest) = trimmed_query.get(6..) {
                    return format!("SELECT TOP 5000{}", rest);
                }
            }
            trimmed_query.to_string()
        }
        _ => {
            // MySQL/PostgreSQL/SQLite/MongoDB/Redis: append LIMIT 5000
            format!("{} LIMIT 5000", trimmed_query)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_select_allows_auto_pagination() {
        assert!(should_enable_auto_pagination("SELECT * FROM users"));
        assert!(should_enable_auto_pagination(
            "USE mydb; SELECT * FROM `users`"
        ));
        assert!(should_enable_auto_pagination(
            "SELECT * FROM schema.table_name"
        ));
    }

    #[test]
    fn select_with_alias_skips_auto_pagination() {
        assert!(!should_enable_auto_pagination("SELECT * FROM users u"));
        assert!(!should_enable_auto_pagination("SELECT * FROM `users` AS u"));
        assert!(!should_enable_auto_pagination("SELECT column1 FROM users"));
    }

    #[test]
    fn select_with_join_or_filters_skips_auto_pagination() {
        assert!(!should_enable_auto_pagination(
            "SELECT * FROM users JOIN orders ON users.id = orders.user_id"
        ));
        assert!(!should_enable_auto_pagination(
            "SELECT * FROM users WHERE users.active = 1"
        ));
    }

    #[test]
    fn multiple_selects_do_not_auto_paginate() {
        assert!(!should_enable_auto_pagination(
            "SELECT * FROM users; SELECT * FROM orders"
        ));
    }
}
