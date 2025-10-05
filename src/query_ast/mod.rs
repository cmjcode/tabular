//! Experimental query AST & logical plan layer (Phase 1)
//!
//! Feature flag: `query_ast`
//! Current scope: single SELECT parsing (no joins/subqueries/aggregation yet) + emission for MySQL/Postgres/SQLite.
//! Fallback: caller should ignore errors and use legacy raw query path.

#[cfg(feature = "query_ast")]
pub mod ast;
#[cfg(feature = "query_ast")]
pub mod logical;
#[cfg(feature = "query_ast")]
pub mod parser;
#[cfg(feature = "query_ast")]
pub mod emitter;
#[cfg(feature = "query_ast")]
pub mod rewrite;
#[cfg(feature = "query_ast")]
pub mod errors;
#[cfg(feature = "query_ast")]
pub mod plan_cache;

#[cfg(feature = "query_ast")]
pub use errors::*;
#[cfg(feature = "query_ast")]
pub use logical::*;

#[cfg(feature = "query_ast")]
use crate::models::enums::DatabaseType;

#[cfg(feature = "query_ast")]
/// Compile raw SQL (expected single SELECT) into (emitted SQL, inferred headers)
/// Headers inference: projection columns / alias; Star => returns empty (caller may fallback to DESCRIBE/LIMIT 0)
pub fn compile_single_select(
    raw: &str,
    db_type: &DatabaseType,
    pagination: Option<(u64,u64)>, // (page, page_size)
    inject_auto_limit: bool,
) -> Result<(String, Vec<String>), QueryAstError> {
    use parser::parse_single_select_to_plan;
    use rewrite::{apply_basic_rewrites, Pagination};
    use emitter::emit_sql;
    use plan_cache::PlanCache;
    use std::hash::{Hasher, Hash};
    use std::collections::hash_map::DefaultHasher;

    fn canonicalize_space(s: &str) -> String {
        let mut out = s.trim().trim_end_matches(';').to_string();
        out = out.split_whitespace().collect::<Vec<_>>().join(" "); out
    }
    // Basic structural fingerprint: lowercase, remove redundant spaces and quote style agnostic
    fn structural_fingerprint(s: &str) -> u64 {
        let norm = canonicalize_space(&s.to_ascii_lowercase().replace('`', "\"").replace('[', "\"").replace(']', "\""));
        let mut hasher = DefaultHasher::new(); norm.hash(&mut hasher); hasher.finish()
    }
    let canon = canonicalize_space(raw);
    let fp = structural_fingerprint(raw);
    let cache_key = format!("fp{}::{:?}::{:?}::{}", fp, db_type, pagination, inject_auto_limit);
    if let Some(entry) = PlanCache::global().get(&cache_key) { return Ok((entry.sql, entry.headers)); }

    let mut working_sql = raw.to_string();
    // Very simple CTE inlining (Phase A): if WITH cte AS (sub) SELECT ... ; only support single simple CTE referenced once
    if working_sql.trim_start().to_ascii_lowercase().starts_with("with ") {
        if let Some(as_pos) = working_sql.to_ascii_lowercase().find(" as (") {
            // Extract cte name
            let with_body = &working_sql[4..as_pos];
            let cte_name = with_body.trim().trim_matches(|c: char| c==',' );
            if let Some(close_paren) = working_sql[as_pos..].find(')') {
                let subquery_start = as_pos + 5; // len(" as (")
                let subquery_sql = working_sql[subquery_start..as_pos+close_paren].trim();
                // Remaining after )
                let after = &working_sql[as_pos+close_paren+1..];
                // Replace first occurrence of cte_name in FROM with (subquery) alias
                let lowered_after = after.to_ascii_lowercase();
                if lowered_after.contains(&format!(" {} ", cte_name.to_ascii_lowercase())) {
                    let replaced_after = after.replacen(cte_name, &format!("({})", subquery_sql), 1);
                    working_sql = replaced_after;
                } else {
                    // If not referenced, fallback to original SELECT after CTE list (strip WITH ...)
                    working_sql = after.to_string();
                }
            }
        }
    }
    let mut plan = parse_single_select_to_plan(&working_sql)?;
    let pagination = pagination.map(|(page, size)| Pagination { page, page_size: size });
    apply_basic_rewrites(&mut plan, inject_auto_limit, pagination)?;
    let headers = infer_headers_from_plan(&plan);
    let sql = emit_sql(&plan, db_type)?;
    PlanCache::global().insert(cache_key, plan_cache::PlanEntry { plan: std::sync::Arc::new(plan), sql: sql.clone(), headers: headers.clone() });
    Ok((sql, headers))
}

#[cfg(feature = "query_ast")]
pub fn cache_stats() -> (u64,u64) {
    use plan_cache::PlanCache; PlanCache::global().stats()
}

#[cfg(feature = "query_ast")]
pub fn debug_plan(raw: &str, db_type: &DatabaseType) -> Result<String, QueryAstError> {
    use parser::parse_single_select_to_plan;
    let plan = parse_single_select_to_plan(raw)?;
    fn fmt(plan: &LogicalQueryPlan, indent: usize, out: &mut String) {
        use logical::LogicalQueryPlan as L;
        let pad = "  ".repeat(indent);
        match plan {
            L::TableScan { table } => { out.push_str(&format!("{}TableScan({})\n", pad, table)); }
            L::SubqueryScan { alias, .. } => { out.push_str(&format!("{}SubqueryScan(alias={})\n", pad, alias)); }
            L::Projection { exprs, input } => { out.push_str(&format!("{}Projection {:?}\n", pad, exprs.len())); fmt(input, indent+1, out); }
            L::Distinct { input } => { out.push_str(&format!("{}Distinct\n", pad)); fmt(input, indent+1, out); }
            L::Filter { predicate, input } => { out.push_str(&format!("{}Filter {:?}\n", pad, predicate)); fmt(input, indent+1, out); }
            L::Sort { items, input } => { out.push_str(&format!("{}Sort {:?}\n", pad, items.len())); fmt(input, indent+1, out); }
            L::Limit { limit, offset, input } => { out.push_str(&format!("{}Limit limit={} offset={}\n", pad, limit, offset)); fmt(input, indent+1, out); }
            L::Group { group_exprs, input } => { out.push_str(&format!("{}Group {:?}\n", pad, group_exprs.len())); fmt(input, indent+1, out); }
            L::Join { left, right, .. } => { out.push_str(&format!("{}Join\n", pad)); fmt(left, indent+1, out); fmt(right, indent+1, out); }
            L::Having { predicate, input } => { out.push_str(&format!("{}Having {:?}\n", pad, predicate)); fmt(input, indent+1, out); }
        }
    }
    let mut s = String::new();
    s.push_str(&format!("-- debug plan for {:?}\n", db_type));
    fmt(&plan, 0, &mut s);
    Ok(s)
}

#[cfg(feature = "query_ast")]
fn infer_headers_from_plan(plan: &LogicalQueryPlan) -> Vec<String> {
    use logical::LogicalQueryPlan as L;
    use logical::Expr as E;
    fn find_projection(p: &L) -> Option<&Vec<E>> { match p { L::Projection { exprs, .. } => Some(exprs), L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } => find_projection(input), L::Join { left, .. } => find_projection(left), L::TableScan { .. } | L::SubqueryScan { .. } => None } }
    if let Some(exprs) = find_projection(plan) {
        let mut out = Vec::new();
        for e in exprs {
            match e {
                E::Alias { alias, .. } => out.push(alias.clone()),
                E::Column(c) => out.push(c.split('.').last().unwrap_or(c).to_string()),
                E::FuncCall { name, .. } => out.push(name.clone()),
                E::Star => return Vec::new(), // unknown until runtime
                E::Number(n) => out.push(n.clone()),
                E::StringLiteral(_) => out.push("literal".to_string()),
                E::Boolean(_) => out.push("bool".to_string()),
                E::Null => out.push("null".to_string()),
                E::Not(_) => out.push("not".to_string()),
                E::IsNull { .. } => out.push("is_null".to_string()),
                E::Like { .. } => out.push("like".to_string()),
                E::InList { .. } => out.push("in_list".to_string()),
                E::Case { .. } => out.push("case".to_string()),
                E::BinaryOp { .. } | E::Raw(_) => out.push("expr".to_string()),
                E::Subquery(_) => out.push("subquery".to_string()),
            }
        }
        out
    } else { Vec::new() }
}

#[cfg(not(feature = "query_ast"))]
pub fn compile_single_select(
    _raw: &str,
    _db_type: &crate::models::enums::DatabaseType,
    _pagination: Option<(u64,u64)>,
    _inject_auto_limit: bool,
) -> Result<(String, Vec<String>), ()> { Err(()) }
