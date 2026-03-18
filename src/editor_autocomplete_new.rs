//! Temporary clean replacement for editor_autocomplete while original is corrupted.
use crate::cache_data::{get_columns_from_cache, get_tables_from_cache};
use crate::query_tools;
use crate::window_egui::Tabular;
use eframe::egui;

use std::collections::HashSet;

const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "TABLE", "DROP", "ALTER", "ADD", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "ON", "GROUP",
    "BY", "ORDER", "LIMIT", "OFFSET", "AND", "OR", "NOT", "NULL", "AS", "DISTINCT", "COUNT", "SUM",
    "AVG", "MIN", "MAX", "LIKE", "IN", "IS", "BETWEEN", "UNION", "ALL",
];

fn current_prefix(text: &str, cursor: usize) -> (String, usize) {
    if text.is_empty() {
        return (String::new(), cursor);
    }
    let bytes = text.as_bytes();
    let mut start = cursor.min(bytes.len());
    while start > 0 {
        let c = bytes[start - 1] as char;
        if c.is_alphanumeric() || matches!(c, '_' | ':' | '@' | '$' | '.') {
            start -= 1;
        } else {
            break;
        }
    }
    (text[start..cursor.min(text.len())].to_string(), start)
}

fn find_statement_bounds(text: &str, cursor: usize) -> (usize, usize) {
    if text.is_empty() {
        return (0, 0);
    }
    let bytes = text.as_bytes();
    let n = bytes.len();
    let cursor = cursor.min(n);

    // Scan backwards from cursor
    let mut start = cursor;
    while start > 0 {
        if bytes[start - 1] == b';' {
            break;
        }
        start -= 1;
    }

    // Scan forwards from cursor
    let mut end = cursor;
    while end < n {
        if bytes[end] == b';' {
            break;
        }
        end += 1;
    }

    (start, end)
}
fn active_connection_and_db(app: &Tabular) -> Option<(i64, String)> {
    app.query_tabs.get(app.active_tab_index).and_then(|tab| {
        tab.connection_id
            .map(|cid| (cid, tab.database_name.clone().unwrap_or_default()))
    })
}
fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn strip_wrapping_pair(s: &str) -> &str {
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        match (bytes[0], bytes[s.len() - 1]) {
            (b'"', b'"') | (b'`', b'`') | (b'[', b']') => return &s[1..s.len() - 1],
            _ => {}
        }
    }
    s
}

fn parse_table_name(sql: &str, mut idx: usize) -> Option<(usize, String)> {
    let bytes = sql.as_bytes();
    let len = bytes.len();
    while idx < len && bytes[idx].is_ascii_whitespace() {
        idx += 1;
    }
    if idx >= len {
        return None;
    }
    if bytes[idx] == b'(' {
        return None;
    }
    let start = idx;
    while idx < len {
        let b = bytes[idx];
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'"' | b'`' | b'[' | b']') {
            idx += 1;
        } else {
            break;
        }
    }
    if start == idx {
        return None;
    }
    let mut token = sql[start..idx].trim();
    token = token.trim_end_matches([',', ';']);
    if token.is_empty() {
        return None;
    }
    let mut final_seg = None;
    for seg in token.split('.') {
        let stripped = strip_wrapping_pair(seg.trim());
        if !stripped.is_empty() {
            final_seg = Some(strip_wrapping_pair(stripped));
        }
    }
    let final_name = final_seg?.trim();
    if final_name.is_empty() {
        return None;
    }
    Some((start, final_name.to_string()))
}

fn collect_table_hits(sql: &str) -> Vec<(usize, String)> {
    let lower = sql.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut hits = Vec::new();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..].starts_with(b"from")
            && (i == 0 || !is_word_char(bytes[i - 1]))
            && (i + 4 >= bytes.len() || !is_word_char(bytes[i + 4]))
        {
            if let Some((pos, name)) = parse_table_name(sql, i + 4) {
                hits.push((pos, name));
            }
            i += 4;
            continue;
        }
        if bytes[i..].starts_with(b"join")
            && (i == 0 || !is_word_char(bytes[i - 1]))
            && (i + 4 >= bytes.len() || !is_word_char(bytes[i + 4]))
        {
            if let Some((pos, name)) = parse_table_name(sql, i + 4) {
                hits.push((pos, name));
            }
            i += 4;
            continue;
        }
        i += 1;
    }
    hits
}

fn tables_near_cursor(sql: &str, cursor: usize) -> Vec<String> {
    let hits = collect_table_hits(sql);
    if hits.is_empty() {
        return Vec::new();
    }
    
    // Constrain to current statement to avoid pollution from other queries
    let (stmt_start, stmt_end) = find_statement_bounds(sql, cursor);
    
    let cursor = cursor.min(sql.len());
    let mut below: Vec<_> = hits
        .iter()
        .filter(|(pos, _)| *pos >= cursor && *pos < stmt_end)
        .cloned()
        .collect();
    below.sort_by_key(|(pos, _)| *pos);
    let mut above: Vec<_> = hits
        .iter()
        .filter(|(pos, _)| *pos < cursor && *pos >= stmt_start)
        .cloned()
        .collect();
    above.sort_by_key(|(pos, _)| cursor - *pos);
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for (_, name) in below.into_iter().chain(above.into_iter()) {
        if seen.insert(name.clone()) {
            result.push(name);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_statement_bounds() {
        let sql = "SELECT * FROM t1; SELECT * FROM t2 WHERE id = 1; INSERT INTO t3 VALUES(1)";
        //         01234567890123456 7890123456789012345678901234567 8901234567890123456789012
        //         0                 1                  2                  3                  4

        // Cursor in first statement
        assert_eq!(find_statement_bounds(sql, 5), (0, 16));
        
        // Cursor in second statement
        assert_eq!(find_statement_bounds(sql, 25), (17, 47)); // after first ; (16) to second ; (47)
        
        // Cursor in third statement
        assert_eq!(find_statement_bounds(sql, 60), (48, 73));
    }

    #[test]
    fn test_tables_near_cursor_isolation() {
        let sql = "SELECT * FROM users; SELECT * FROM orders WHERE user_id = 1";
        
        // Cursor in first query (at end of 'users')
        let tables1 = tables_near_cursor(sql, 19); 
        assert_eq!(tables1, vec!["users"]);

        // Cursor in second query (at 'orders')
        let tables2 = tables_near_cursor(sql, 40);
        assert_eq!(tables2, vec!["orders"]);
    }
}

fn extract_tables(sql: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, name) in collect_table_hits(sql) {
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlContext {
    AfterSelect,
    AfterFrom,
    AfterWhere,
    AfterJoinOn,
    General,
}
fn detect_ctx(sql: &str, cursor: usize) -> SqlContext {
    let slice = &sql[..cursor.min(sql.len())];
    let mut last: Option<SqlContext> = None;
    for tok in slice.split_whitespace() {
        match tok.to_ascii_uppercase().as_str() {
            "SELECT" => last = Some(SqlContext::AfterSelect),
            "FROM" | "JOIN" | "LEFT" | "RIGHT" | "INNER" | "OUTER" | "CROSS" | "NATURAL" => {
                last = Some(SqlContext::AfterFrom)
            }
            "WHERE" | "HAVING" => last = Some(SqlContext::AfterWhere),
            "AND" | "OR" => {
                // AND/OR inside a JOIN ON condition stays in AfterJoinOn context
                if last != Some(SqlContext::AfterJoinOn) {
                    last = Some(SqlContext::AfterWhere);
                }
            }
            "ON" => {
                // ON after a JOIN (AfterFrom) is a join condition clause
                if last == Some(SqlContext::AfterFrom) {
                    last = Some(SqlContext::AfterJoinOn);
                }
            }
            _ => {}
        }
    }
    last.unwrap_or(SqlContext::General)
}
fn get_cached_tables(app: &Tabular, cid: i64, db: &str) -> Option<Vec<String>> {
    let dbs = if db.is_empty() {
        app.database_cache.get(&cid).cloned().unwrap_or_default()
    } else {
        vec![db.to_string()]
    };
    let mut all = Vec::new();
    for d in dbs {
        for tt in ["table", "view"] {
            if let Some(mut ls) = get_tables_from_cache(app, cid, &d, tt) {
                all.append(&mut ls);
            }
        }
    }
    if all.is_empty() {
        None
    } else {
        all.sort_unstable();
        all.dedup();
        Some(all)
    }
}
fn get_all_tables(app: &Tabular) -> Vec<String> {
    let mut all = Vec::new();
    for (cid, dbs) in &app.database_cache {
        for d in dbs {
            for tt in ["table", "view"] {
                if let Some(mut ls) = get_tables_from_cache(app, *cid, d, tt) {
                    all.append(&mut ls);
                }
            }
        }
    }
    all.sort_unstable();
    all.dedup();
    all
}
fn get_cached_columns(
    app: &mut Tabular,
    cid: i64,
    db: &str,
    tables: Vec<String>,
) -> Option<Vec<String>> {
    if tables.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for t in tables {
        if let Some(cols) = get_columns_from_cache(app, cid, db, &t) {
            for (c, _) in cols {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
    }
    out.sort_unstable();
    if out.is_empty() { None } else { Some(out) }
}
fn add_keywords(out: &mut Vec<String>, pref: &str) {
    for kw in SQL_KEYWORDS {
        if kw.to_ascii_lowercase().starts_with(pref) {
            out.push((*kw).to_string());
        }
    }
}

/// Collect all ForeignKey instances from any loaded diagram state in the app.
fn collect_loaded_fks(app: &Tabular) -> Vec<crate::models::structs::ForeignKey> {
    app.query_tabs
        .iter()
        .filter_map(|tab| tab.diagram_state.as_ref())
        .flat_map(|ds| ds.nodes.iter())
        .flat_map(|n| n.foreign_keys.iter().cloned())
        .collect()
}

/// Parse alias → real-table-name mappings from a SQL string.
/// Handles `FROM table alias`, `FROM table AS alias`, `JOIN table alias`, etc.
/// Returns `HashMap<alias_lowercase, real_table_name_as_written>`.
fn collect_alias_map(sql: &str) -> std::collections::HashMap<String, String> {
    let bytes = sql.as_bytes();
    let lower = sql.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let len = bytes.len();
    let mut map = std::collections::HashMap::new();
    let mut i = 0;
    while i < len {
        let kw_end = if i + 4 <= len
            && (lb[i..i + 4] == *b"from" || lb[i..i + 4] == *b"join")
            && (i == 0 || !is_word_char(bytes[i - 1]))
            && (i + 4 >= len || !is_word_char(bytes[i + 4]))
        {
            Some(i + 4)
        } else {
            None
        };
        if let Some(mut j) = kw_end {
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Skip subqueries
            if j < len && bytes[j] == b'(' {
                i += 1;
                continue;
            }
            // Read table name (may include schema prefix and/or quotes)
            let tname_start = j;
            while j < len {
                let b = bytes[j];
                if b.is_ascii_alphanumeric()
                    || matches!(b, b'_' | b'.' | b'"' | b'`' | b'[' | b']')
                {
                    j += 1;
                } else {
                    break;
                }
            }
            if j == tname_start {
                i += 1;
                continue;
            }
            let raw_tname = &sql[tname_start..j];
            // Use only the last segment (drop schema prefix)
            let table_name: String = raw_tname
                .split('.')
                .last()
                .map(|s| strip_wrapping_pair(s).to_string())
                .unwrap_or_else(|| raw_tname.to_string());
            // Always map the table itself
            map.entry(table_name.to_ascii_lowercase())
                .or_insert(table_name.clone());
            // Skip whitespace
            while j < len && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            // Optional AS keyword
            if j + 2 <= len
                && lb[j..j + 2] == *b"as"
                && (j + 2 >= len || !is_word_char(bytes[j + 2]))
            {
                j += 2;
                while j < len && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
            }
            // Read alias (must be a word token and not a SQL keyword)
            if j < len && is_word_char(bytes[j]) {
                let alias_start = j;
                while j < len && is_word_char(bytes[j]) {
                    j += 1;
                }
                let alias = &sql[alias_start..j];
                let alias_upper = alias.to_ascii_uppercase();
                let is_kw = SQL_KEYWORDS
                    .iter()
                    .any(|kw| *kw == alias_upper.as_str());
                if !is_kw {
                    map.insert(alias.to_ascii_lowercase(), table_name.clone());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    map
}

/// Build join condition suggestions for `JOIN <table> ON` context.
/// Returns `table1.col = table2.col` style strings using FK data (when available)
/// and heuristic column-name matching as fallback.
fn suggest_join_conditions(
    app: &mut Tabular,
    cid: i64,
    db: &str,
    tables: &[String],
    alias_map: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    if tables.len() < 2 {
        return Vec::new();
    }

    // Build display name map: real_table_lowercase → alias (or table name if no alias)
    let mut real_to_display: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for t in tables {
        real_to_display
            .entry(t.to_ascii_lowercase())
            .or_insert(t.clone());
    }
    for (alias, real) in alias_map {
        let real_lower = real.to_ascii_lowercase();
        if alias != &real_lower {
            // True alias — use it as the display name
            real_to_display.insert(real_lower, alias.clone());
        }
    }
    let dn = |name: &str| -> String {
        real_to_display
            .get(&name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| name.to_string())
    };

    // Collect FK info from any open diagram state
    let fks = collect_loaded_fks(app);

    // Fetch columns for every table involved in the query
    let mut table_cols: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for t in tables {
        if let Some(cols) = get_cached_columns(app, cid, db, vec![t.clone()]) {
            table_cols.insert(t.clone(), cols);
        }
    }

    let mut suggestions: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let push = |s: String, seen: &mut std::collections::HashSet<String>, out: &mut Vec<String>| {
        let key = s.to_lowercase();
        if seen.insert(key) {
            out.push(s);
        }
    };

    // --- FK-based suggestions --------------------------------------------
    for fk in &fks {
        let fk_tbl = fk.table_name.to_lowercase();
        let fk_ref = fk.referenced_table_name.to_lowercase();
        for t1 in tables {
            for t2 in tables {
                if t1 == t2 {
                    continue;
                }
                let t1l = t1.to_lowercase();
                let t2l = t2.to_lowercase();
                let d1 = dn(t1);
                let d2 = dn(t2);
                if fk_tbl == t1l && fk_ref == t2l {
                    let cond = format!("{}.{} = {}.{}", d1, fk.column_name, d2, fk.referenced_column_name);
                    push(cond, &mut seen, &mut suggestions);
                } else if fk_tbl == t2l && fk_ref == t1l {
                    let cond = format!("{}.{} = {}.{}", d1, fk.referenced_column_name, d2, fk.column_name);
                    push(cond, &mut seen, &mut suggestions);
                }
            }
        }
    }

    // --- Heuristic column-matching ---------------------------------------
    for i in 0..tables.len() {
        for j in (i + 1)..tables.len() {
            let t1 = &tables[i];
            let t2 = &tables[j];
            let t1_cols = match table_cols.get(t1) {
                Some(c) => c,
                None => continue,
            };
            let t2_cols = match table_cols.get(t2) {
                Some(c) => c,
                None => continue,
            };

            // Pattern A: t1 has `<t2_name>_id` or `<t2_singular>_id`, t2 has `id`
            let t2l = t2.to_lowercase();
            let t2_singular = t2l.trim_end_matches('s').to_string();
            let d1 = dn(t1);
            let d2 = dn(t2);
            for col1 in t1_cols {
                let c1l = col1.to_lowercase();
                let is_fk_col = c1l == format!("{}_id", t2l)
                    || c1l == format!("{}_id", t2_singular)
                    || c1l == format!("{}id", t2l)
                    || c1l == format!("{}id", t2_singular);
                if is_fk_col && t2_cols.iter().any(|c| c.to_lowercase() == "id") {
                    let cond = format!("{}.{} = {}.id", d1, col1, d2);
                    push(cond, &mut seen, &mut suggestions);
                }
            }

            // Pattern B: t2 has `<t1_name>_id` or `<t1_singular>_id`, t1 has `id`
            let t1l = t1.to_lowercase();
            let t1_singular = t1l.trim_end_matches('s').to_string();
            for col2 in t2_cols {
                let c2l = col2.to_lowercase();
                let is_fk_col = c2l == format!("{}_id", t1l)
                    || c2l == format!("{}_id", t1_singular)
                    || c2l == format!("{}id", t1l)
                    || c2l == format!("{}id", t1_singular);
                if is_fk_col && t1_cols.iter().any(|c| c.to_lowercase() == "id") {
                    let cond = format!("{}.id = {}.{}", d1, d2, col2);
                    push(cond, &mut seen, &mut suggestions);
                }
            }

            // Pattern C: same column name in both tables (common join key)
            for col1 in t1_cols {
                for col2 in t2_cols {
                    if col1.to_lowercase() == col2.to_lowercase() {
                        let cond = format!("{}.{} = {}.{}", d1, col1, d2, col2);
                        push(cond, &mut seen, &mut suggestions);
                    }
                }
            }
        }
    }

    suggestions
}

pub fn build_suggestions(
    app: &mut Tabular,
    text: &str,
    cursor: usize,
    prefix: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let pl = prefix.to_ascii_lowercase();
    
    // Check for dot-based table access (e.g. "users.na")
    // If prefix contains '.', we try to split it into table_part + col_part
    if let Some((table_part, col_part)) = pl.split_once('.') {
        // Preserve the original-case prefix as typed by the user (for display in suggestions)
        let display_prefix = prefix.split_once('.').map(|(t, _)| t).unwrap_or(table_part);

        let conn_id = app.query_tabs.get(app.active_tab_index).and_then(|t| t.connection_id);
        let db = active_connection_and_db(app)
            .map(|(_, d)| d)
            .unwrap_or_default();

        if let Some(cid) = conn_id {
            // Build alias map and resolve table_part to real cached table name.
            // Priority: alias_map lookup → case-insensitive scope_table match
            let alias_map = collect_alias_map(text);
            let scope_tables = tables_near_cursor(text, cursor);
            let real_table = alias_map
                .get(table_part)
                .cloned()
                .or_else(|| {
                    scope_tables
                        .iter()
                        .find(|t| t.to_ascii_lowercase() == table_part)
                        .cloned()
                });
            let real_table_name = real_table.as_deref().unwrap_or(table_part);

            if let Some(all_cols) = get_cached_columns(app, cid, &db, vec![real_table_name.to_string()]) {
                // Collect FK info to rank FK-relevant columns first
                let fks = collect_loaded_fks(app);
                let real_tl = real_table_name.to_ascii_lowercase();

                // Other tables currently in the query (resolved to real names)
                let other_real: Vec<String> = scope_tables
                    .iter()
                    .filter_map(|t| {
                        if t.to_ascii_lowercase() == real_tl {
                            None
                        } else {
                            Some(
                                alias_map
                                    .get(&t.to_ascii_lowercase())
                                    .cloned()
                                    .unwrap_or_else(|| t.clone())
                                    .to_ascii_lowercase(),
                            )
                        }
                    })
                    .collect();

                // FK-priority set: columns of real_table that participate in a FK
                // with any other table in scope (either as source or as target)
                let priority_cols: std::collections::HashSet<String> = fks
                    .iter()
                    .filter_map(|fk| {
                        let ft = fk.table_name.to_ascii_lowercase();
                        let fr = fk.referenced_table_name.to_ascii_lowercase();
                        if ft == real_tl && other_real.iter().any(|o| *o == fr) {
                            Some(fk.column_name.to_ascii_lowercase())
                        } else if fr == real_tl && other_real.iter().any(|o| *o == ft) {
                            Some(fk.referenced_column_name.to_ascii_lowercase())
                        } else {
                            None
                        }
                    })
                    .collect();

                // Heuristic priority: columns named <other_table>_id
                let heuristic_priority: std::collections::HashSet<String> = all_cols
                    .iter()
                    .filter(|col| {
                        let cl = col.to_ascii_lowercase();
                        other_real.iter().any(|ot| {
                            let sing = ot.trim_end_matches('s').to_string();
                            cl == format!("{}_id", ot)
                                || cl == format!("{}_id", sing)
                                || cl == format!("{}id", ot)
                        })
                    })
                    .map(|c| c.to_ascii_lowercase())
                    .collect();

                let mut fk_sugg: Vec<String> = Vec::new();
                let mut reg_sugg: Vec<String> = Vec::new();
                for c in &all_cols {
                    if !c.to_ascii_lowercase().starts_with(col_part) {
                        continue;
                    }
                    let suggestion = format!("{}.{}", display_prefix, c);
                    let cl = c.to_ascii_lowercase();
                    if priority_cols.contains(&cl) || heuristic_priority.contains(&cl) {
                        fk_sugg.push(suggestion);
                    } else {
                        reg_sugg.push(suggestion);
                    }
                }
                out.extend(fk_sugg);
                out.extend(reg_sugg);
            }
        }

        return out;
    }

    let ctx = detect_ctx(text, cursor);
    let mut tables_in_scope = tables_near_cursor(text, cursor);
    let tables_all = extract_tables(text);
    if tables_in_scope.is_empty() {
        tables_in_scope = tables_all.clone();
    }
    let conn_id = app
        .query_tabs
        .get(app.active_tab_index)
        .and_then(|t| t.connection_id);
    let db = active_connection_and_db(app)
        .map(|(_, d)| d)
        .unwrap_or_default();
    match ctx {
        SqlContext::AfterSelect => {
            add_keywords(&mut out, &pl);
            if let Some(cid) = conn_id
                && let Some(cols) = get_cached_columns(app, cid, &db, tables_in_scope.clone())
            {
                for c in cols {
                    if c.to_ascii_lowercase().starts_with(&pl) {
                        out.push(c);
                    }
                }
            }
            if "*".starts_with(&pl) {
                out.push("*".into());
            }
        }
        SqlContext::AfterFrom => {
            add_keywords(&mut out, &pl);
            if let Some(cid) = conn_id {
                if let Some(ts) = get_cached_tables(app, cid, &db) {
                    for t in ts {
                        if t.to_ascii_lowercase().starts_with(&pl) {
                            out.push(t);
                        }
                    }
                }
            } else {
                for t in get_all_tables(app) {
                    if t.to_ascii_lowercase().starts_with(&pl) {
                        out.push(t);
                    }
                }
            }
        }
        SqlContext::AfterWhere => {
            add_keywords(&mut out, &pl);
            if let Some(cid) = conn_id
                && let Some(cols) = get_cached_columns(app, cid, &db, tables_in_scope.clone())
            {
                for c in cols {
                    if c.to_ascii_lowercase().starts_with(&pl) {
                        out.push(c);
                    }
                }
            }
        }
        SqlContext::AfterJoinOn => {
            add_keywords(&mut out, &pl);
            if let Some(cid) = conn_id {
                let alias_map = collect_alias_map(text);
                // Build real_lower → display_name map
                let mut real_to_display: std::collections::HashMap<String, String> =
                    std::collections::HashMap::new();
                for t in &tables_in_scope {
                    real_to_display
                        .entry(t.to_ascii_lowercase())
                        .or_insert(t.clone());
                }
                for (alias, real) in &alias_map {
                    let real_lower = real.to_ascii_lowercase();
                    if alias != &real_lower {
                        real_to_display.insert(real_lower, alias.clone());
                    }
                }
                // Suggest qualified `alias.column` names for all tables in scope
                for table in &tables_in_scope {
                    let display = real_to_display
                        .get(&table.to_ascii_lowercase())
                        .map(|s| s.as_str())
                        .unwrap_or(table.as_str());
                    if let Some(cols) = get_cached_columns(app, cid, &db, vec![table.clone()]) {
                        for col in &cols {
                            let qualified = format!("{}.{}", display, col);
                            if qualified.to_ascii_lowercase().starts_with(&pl)
                                || col.to_ascii_lowercase().starts_with(&pl)
                            {
                                out.push(qualified);
                            }
                        }
                    }
                }
                // Suggest heuristic / FK-based join conditions (formatted with aliases)
                let join_conds =
                    suggest_join_conditions(app, cid, &db, &tables_in_scope, &alias_map);
                for cond in join_conds {
                    if cond.to_ascii_lowercase().starts_with(&pl) || pl.is_empty() {
                        out.push(cond);
                    }
                }
            }
        }
        SqlContext::General => {
            add_keywords(&mut out, &pl);
            if let Some(cid) = conn_id {
                if let Some(ts) = get_cached_tables(app, cid, &db) {
                    for t in ts {
                        if t.to_ascii_lowercase().starts_with(&pl) {
                            out.push(t);
                        }
                    }
                }
                if let Some(cols) = get_cached_columns(app, cid, &db, tables_in_scope.clone()) {
                    for c in cols {
                        if c.to_ascii_lowercase().starts_with(&pl) {
                            out.push(c);
                        }
                    }
                }
            } else {
                for t in get_all_tables(app) {
                    if t.to_ascii_lowercase().starts_with(&pl) {
                        out.push(t);
                    }
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub fn update_autocomplete(app: &mut Tabular) {
    // Throttle autocomplete updates to avoid heavy work on every keystroke
    let now = std::time::Instant::now();
    if let Some(last) = app.autocomplete_last_update {
        let elapsed = now.saturating_duration_since(last);
        if elapsed < std::time::Duration::from_millis(app.autocomplete_debounce_ms) {
            return;
        }
    }
    app.autocomplete_last_update = Some(now);
    // Clone editor text first to avoid immutable + mutable borrow overlap
    let editor_text = app.editor.text.clone();
    let cursor = app.cursor_position.min(editor_text.len());
    let (pref, _) = current_prefix(&editor_text, cursor);

    // CRITICAL: Don't touch autocomplete state while typing - let text settle first
    // This prevents freeze and caret jumping by avoiding mid-keystroke state mutations

    let prev_char = editor_text[..cursor].chars().next_back();
    if matches!(prev_char, Some(';')) || matches!(prev_char, Some('*')) {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
        app.autocomplete_prefix.clear();
        app.last_autocomplete_trigger_len = 0;
        return;
    }

    if pref.is_empty() {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
        app.autocomplete_prefix.clear();
        app.last_autocomplete_trigger_len = 0;
        return;
    }

    let pre_prefix_char = if pref.len() <= cursor {
        editor_text[..cursor - pref.len()].chars().next_back()
    } else {
        None
    };
    let triggered_by_space = matches!(pre_prefix_char, Some(ch) if ch.is_whitespace());
    
    // Also trigger if the prefix contains a dot (e.g. "table."), implying user wants column suggestions
    let triggered_by_dot = pref.contains('.');
    let triggered_by_len = pref.len() >= 2;
    if !triggered_by_space && !triggered_by_len && !triggered_by_dot {
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
        app.autocomplete_prefix.clear();
        app.last_autocomplete_trigger_len = 0;
        return;
    }

    // Only rebuild if prefix length changed (avoid redundant calls)
    if app.last_autocomplete_trigger_len == pref.len()
        && app.show_autocomplete
        && app.autocomplete_prefix == pref
    {
        // Suggestions already up-to-date for this prefix
        return;
    }

    app.autocomplete_prefix = pref.clone();

    if app.last_autocomplete_trigger_len != pref.len() || !app.show_autocomplete {
        let suggestions = build_suggestions(app, &editor_text, cursor, &pref);
        if suggestions.is_empty() {
            app.show_autocomplete = false;
            app.autocomplete_payloads.clear();
        } else {
            app.show_autocomplete = true;
            let context = detect_ctx(&editor_text, cursor);
            let (cid, db) = app
                .query_tabs
                .get(app.active_tab_index)
                .and_then(|tab| {
                    tab.connection_id
                        .map(|c| (c, tab.database_name.clone().unwrap_or_default()))
                })
                .unwrap_or((0, String::new()));

            let tables_set: HashSet<String> = if cid != 0 {
                get_cached_tables(app, cid, &db)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            } else {
                get_all_tables(app).into_iter().collect()
            };
            // Reuse tables_in_scope from build_suggestions to avoid redundant cache lookups
            let tables_for_cols = tables_near_cursor(&editor_text, cursor);
            let cols_set: HashSet<String> = if cid != 0 && !tables_for_cols.is_empty() {
                get_cached_columns(app, cid, &db, tables_for_cols)
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            } else {
                HashSet::new()
            };
            let syntax_set: HashSet<String> = SQL_KEYWORDS
                .iter()
                .map(|s| s.to_string())
                .chain(std::iter::once("*".to_string()))
                .collect();

            let mut tables = Vec::new();
            let mut columns = Vec::new();
            let mut syntax = Vec::new();
            for s in suggestions.into_iter() {
                if tables_set.contains(&s) {
                    tables.push(s);
                } else if cols_set.contains(&s) {
                    columns.push(s);
                } else if syntax_set.contains(&s) {
                    syntax.push(s);
                } else {
                    match context {
                        SqlContext::AfterFrom => tables.push(s),
                        SqlContext::AfterSelect | SqlContext::AfterWhere | SqlContext::AfterJoinOn => columns.push(s),
                        SqlContext::General => syntax.push(s),
                    }
                }
            }
            tables.sort_unstable();
            tables.dedup();
            columns.sort_unstable();
            columns.dedup();
            syntax.sort_unstable();
            syntax.dedup();

            let mut ordered = Vec::new();
            let mut kinds = Vec::new();
            let mut notes = Vec::new();
            let mut payloads = Vec::new();
            let mut seen_labels: HashSet<String> = HashSet::new();
            let mut push_suggestion =
                |label: String,
                 kind: crate::models::enums::AutocompleteKind,
                 note: Option<String>,
                 payload: Option<String>| {
                    if seen_labels.insert(label.clone()) {
                        ordered.push(label);
                        kinds.push(kind);
                        notes.push(note);
                        payloads.push(payload);
                    }
                };

            for t in tables {
                let note = if db.is_empty() {
                    Some("table".to_string())
                } else {
                    Some(format!("db: {}", db))
                };
                push_suggestion(t, crate::models::enums::AutocompleteKind::Table, note, None);
            }

            for c in columns {
                // Join conditions contain '=' and get a distinct note
                let (kind, note) = if context == SqlContext::AfterJoinOn && c.contains('=') {
                    (crate::models::enums::AutocompleteKind::Column, Some("join".to_string()))
                } else {
                    (crate::models::enums::AutocompleteKind::Column, Some("column".to_string()))
                };
                push_suggestion(c, kind, note, None);
            }

            for param in query_tools::parameter_candidates(&pref) {
                push_suggestion(
                    param.label.to_string(),
                    crate::models::enums::AutocompleteKind::Parameter,
                    Some(param.note.to_string()),
                    Some(param.template.to_string()),
                );
            }

            for kw in syntax {
                let is_wc = kw == "*";
                push_suggestion(
                    kw,
                    crate::models::enums::AutocompleteKind::Syntax,
                    Some(if is_wc {
                        "wildcard".to_string()
                    } else {
                        "keyword".to_string()
                    }),
                    None,
                );
            }

            let snippet_context = match context {
                SqlContext::AfterSelect => query_tools::SnippetContext::SelectList,
                SqlContext::AfterFrom => query_tools::SnippetContext::FromClause,
                SqlContext::AfterWhere | SqlContext::AfterJoinOn => query_tools::SnippetContext::WhereClause,
                SqlContext::General => query_tools::SnippetContext::Any,
            };

            for snippet in query_tools::snippet_candidates(&pref, snippet_context) {
                push_suggestion(
                    snippet.label.to_string(),
                    crate::models::enums::AutocompleteKind::Snippet,
                    Some(snippet.note.to_string()),
                    Some(snippet.template.to_string()),
                );
            }

            app.autocomplete_suggestions = ordered;
            app.autocomplete_kinds = kinds;
            app.autocomplete_notes = notes;
            app.autocomplete_payloads = payloads;
            app.selected_autocomplete_index = 0;
        }
        app.last_autocomplete_trigger_len = pref.len();
    }
}

pub fn accept_current_suggestion(app: &mut Tabular) {
    if !app.show_autocomplete {
        return;
    }
    if let Some(display) = app
        .autocomplete_suggestions
        .get(app.selected_autocomplete_index)
        .cloned()
    {
        let cursor = app.cursor_position.min(app.editor.text.len());
        let (_pref, start) = current_prefix(&app.editor.text, cursor);
        let start_idx = start;
        let replacement = app
            .autocomplete_payloads
            .get(app.selected_autocomplete_index)
            .and_then(|p| p.clone())
            .unwrap_or_else(|| display.clone());
        app.editor
            .apply_single_replace(start_idx..cursor, &replacement);
        app.cursor_position = start_idx + replacement.len();
        app.multi_selection
            .set_primary_range(app.cursor_position, app.cursor_position);
        app.pending_cursor_set = Some(app.cursor_position);
        app.autocomplete_expected_cursor = Some(app.cursor_position);
        app.autocomplete_protection_frames = app.autocomplete_protection_frames.max(8);
        app.editor_focus_boost_frames = app.editor_focus_boost_frames.max(6);
        app.show_autocomplete = false;
        app.autocomplete_suggestions.clear();
        app.autocomplete_kinds.clear();
        app.autocomplete_notes.clear();
        app.autocomplete_payloads.clear();
    }
}

pub fn navigate(app: &mut Tabular, delta: i32) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let len = app.autocomplete_suggestions.len();
    if delta > 0 {
        app.selected_autocomplete_index = (app.selected_autocomplete_index + 1) % len;
    } else if app.selected_autocomplete_index == 0 {
        app.selected_autocomplete_index = len - 1;
    } else {
        app.selected_autocomplete_index -= 1;
    }
}

pub fn render_autocomplete(app: &mut Tabular, ui: &mut egui::Ui, pos: egui::Pos2) {
    if !app.show_autocomplete || app.autocomplete_suggestions.is_empty() {
        return;
    }
    let screen = ui.ctx().screen_rect();
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let small_font_id = egui::TextStyle::Small.resolve(ui.style());
    let heading_font_id = egui::TextStyle::Body.resolve(ui.style());
    let suggestions = app.autocomplete_suggestions.clone();
    let kinds = app.autocomplete_kinds.clone();
    let notes = app.autocomplete_notes.clone();
    let mut max_label_px: f32 = 0.0;
    let mut max_note_px: f32 = 0.0;
    let mut max_heading_px: f32 = 0.0;
    let mut note_count = 0usize;
    let mut group_count = 0usize;
    let mut last_kind: Option<crate::models::enums::AutocompleteKind> = None;
    ui.ctx().fonts(|f| {
        for (idx, s) in suggestions.iter().enumerate() {
            let g = f.layout_no_wrap(s.clone(), font_id.clone(), egui::Color32::WHITE);
            max_label_px = max_label_px.max(g.size().x);

            if let Some(Some(note)) = notes.get(idx) {
                let ng =
                    f.layout_no_wrap(note.clone(), small_font_id.clone(), egui::Color32::WHITE);
                max_note_px = max_note_px.max(ng.size().x);
                note_count += 1;
            }

            if let Some(&kind) = kinds.get(idx)
                && last_kind != Some(kind)
            {
                group_count += 1;
                last_kind = Some(kind);
                let heading = match kind {
                    crate::models::enums::AutocompleteKind::Table => "Tables",
                    crate::models::enums::AutocompleteKind::Column => "Columns",
                    crate::models::enums::AutocompleteKind::Syntax => "Syntax",
                    crate::models::enums::AutocompleteKind::Snippet => "Snippets",
                    crate::models::enums::AutocompleteKind::Parameter => "Parameters",
                };
                let hg = f.layout_no_wrap(
                    heading.to_string(),
                    heading_font_id.clone(),
                    egui::Color32::WHITE,
                );
                max_heading_px = max_heading_px.max(hg.size().x);
            }
        }
    });

    let base_width = max_label_px.max(max_heading_px) + max_note_px + 20.0;
    // Ensure we have enough width for the note
    let popup_w = (base_width + 48.0).clamp(300.0, (screen.width() - 32.0).max(300.0));

    let entry_count = suggestions.len() as f32;
    // Recalculate height estimate since items are now single rows
    let mut desired_h =
        entry_count * 22.0 + (group_count as f32) * 25.0 + 10.0; // Slightly taller rows for comfort

    if desired_h < 64.0 {
        desired_h = 64.0;
    }
    let screen_h = screen.height();
    let desired_cap = screen_h * 0.65;
    if desired_h > desired_cap {
        desired_h = desired_cap;
    }
    let margin = 8.0;
    let space_below = (screen.bottom() - pos.y - margin).max(0.0);
    let space_above = (pos.y - screen.top() - margin).max(0.0);
    let show_above = space_below + 2.0 < desired_h && space_above > space_below;
    let max_h = desired_h.min(if show_above { space_above } else { space_below });
    let mut popup_pos = pos;
    if show_above {
        popup_pos.y = (pos.y - max_h).max(screen.top());
    }
    if popup_pos.x + popup_w > screen.right() {
        popup_pos.x = (screen.right() - popup_w).max(screen.left());
    }
    
    // nice shadow and generic window styles
    // nice shadow and generic window styles
    egui::Area::new(egui::Id::new("autocomplete_popup"))
        .fixed_pos(popup_pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
             egui::Frame::popup(ui.style())
                .shadow(eframe::epaint::Shadow {
                    offset: [0, 8],
                    blur: 10,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(96),
                })
                .fill(ui.style().visuals.window_fill)
                .stroke(ui.style().visuals.window_stroke())

                .show(ui, |ui| {
                    ui.set_min_width(popup_w);
                    ui.set_max_width(popup_w);
                    ui.set_min_height(max_h);
                    let suggestions = suggestions.clone();
                    let kinds = kinds.clone();
                    let notes = notes.clone();
                    let mut last_kind = None;
                    
                    egui::ScrollArea::vertical()
                        .max_height(max_h)
                        .show(ui, |ui| {
                        
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                            for (i, s) in suggestions.iter().enumerate() {
                                if let Some(k) = kinds.get(i).copied()
                                    && last_kind != Some(k)
                                {
                                    last_kind = Some(k);
                                    let label = match k {
                                        crate::models::enums::AutocompleteKind::Table => "Tables",
                                        crate::models::enums::AutocompleteKind::Column => "Columns",
                                        crate::models::enums::AutocompleteKind::Syntax => "Syntax",
                                        crate::models::enums::AutocompleteKind::Snippet => {
                                            "Snippets"
                                        }
                                        crate::models::enums::AutocompleteKind::Parameter => {
                                            "Parameters"
                                        }
                                    };
                                    // Header grouping
                                    ui.allocate_ui(egui::vec2(ui.available_width(), 24.0), |ui| {
                                        ui.horizontal(|ui| {
                                             ui.add_space(8.0);
                                             ui.heading(egui::RichText::new(label).size(12.0).strong().color(ui.visuals().text_color().gamma_multiply(0.6)));
                                        });
                                    });
                                    ui.add(egui::Separator::default().spacing(0.0));
                                }
                                
                                let selected = i == app.selected_autocomplete_index;
                                
                                // Custom row rendering for "Smooth" look
                                let row_height = 22.0;
                                let available_width = ui.available_width();
                                let (rect, response) = ui.allocate_exact_size(egui::vec2(available_width, row_height), egui::Sense::click());
                                
                                if ui.is_rect_visible(rect) {
                                    let visuals = ui.style().interact_selectable(&response, selected);
                                    
                                    // Background
                                    if selected || response.hovered() {
                                        ui.painter().add(egui::Shape::rect_filled(
                                            rect,
                                            0.0,
                                            visuals.bg_fill
                                        ));
                                    }
                                    
                                    let text_color = if selected {
                                        visuals.text_color()
                                    } else {
                                        ui.visuals().text_color()
                                    };

                                    // Render content using Widgets to avoid FontId/Painter errors
                                    // Padding
                                    let content_rect = rect.shrink2(egui::vec2(8.0, 0.0));
                                    
                                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                                        ui.horizontal_centered(|ui| {
                                            // Main Label
                                            ui.label(egui::RichText::new(s).font(font_id.clone()).color(text_color));
                                            
                                            // Spacer
                                            ui.allocate_space(ui.available_size());
                                        });
                                    });

                                    // Note (Right aligned overlay)
                                    if let Some(note) = notes.get(i).and_then(|n| n.clone()) {
                                         let note_color = if selected {
                                            text_color.gamma_multiply(0.7)
                                        } else {
                                            ui.visuals().text_color().gamma_multiply(0.5)
                                        };
                                        
                                        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(egui::RichText::new(note).font(small_font_id.clone()).color(note_color));
                                            });
                                        });
                                    }
                                }

                                if response.clicked() {
                                    app.selected_autocomplete_index = i;
                                    accept_current_suggestion(app);
                                    let id = egui::Id::new("sql_editor");
                                    if let Some(mut state) =
                                        egui::text_edit::TextEditState::load(ui.ctx(), id)
                                    {
                                        use egui::text::{CCursor, CCursorRange};
                                        state.cursor.set_char_range(Some(CCursorRange::one(
                                            CCursor::new(app.cursor_position),
                                        )));
                                        state.store(ui.ctx(), id);
                                    }
                                    ui.memory_mut(|m| m.request_focus(egui::Id::new("sql_editor")));
                                    app.editor_focus_boost_frames =
                                        app.editor_focus_boost_frames.max(6);
                                }
                            }
                        });
                });
        });
}

pub fn trigger_manual(app: &mut Tabular) {
    update_autocomplete(app);
    if app.autocomplete_prefix.is_empty() {
        app.autocomplete_suggestions = SQL_KEYWORDS.iter().map(|s| s.to_string()).collect();
        app.autocomplete_suggestions.sort_unstable();
        app.selected_autocomplete_index = 0;
        app.show_autocomplete = true;
        app.autocomplete_kinds = vec![
            crate::models::enums::AutocompleteKind::Syntax;
            app.autocomplete_suggestions.len()
        ];
        app.autocomplete_notes =
            vec![Some("keyword".to_string()); app.autocomplete_suggestions.len()];
        app.autocomplete_payloads = vec![None; app.autocomplete_suggestions.len()];
    } else if app.autocomplete_suggestions.is_empty() {
        app.autocomplete_suggestions = SQL_KEYWORDS
            .iter()
            .filter(|k| {
                k.to_lowercase()
                    .starts_with(&app.autocomplete_prefix.to_ascii_lowercase())
            })
            .map(|s| s.to_string())
            .collect();
        if !app.autocomplete_suggestions.is_empty() {
            app.show_autocomplete = true;
            app.autocomplete_kinds = vec![
                crate::models::enums::AutocompleteKind::Syntax;
                app.autocomplete_suggestions.len()
            ];
            app.autocomplete_notes =
                vec![Some("keyword".to_string()); app.autocomplete_suggestions.len()];
            app.autocomplete_payloads = vec![None; app.autocomplete_suggestions.len()];
        }
    }
}
