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

    let cache_key = format!("{}::{:?}::{:?}::{}", raw, db_type, pagination, inject_auto_limit);
    if let Some(entry) = PlanCache::global().get(&cache_key) { return Ok((entry.sql, entry.headers)); }

    let mut plan = parse_single_select_to_plan(raw)?;
    let pagination = pagination.map(|(page, size)| Pagination { page, page_size: size });
    apply_basic_rewrites(&mut plan, inject_auto_limit, pagination)?;
    let headers = infer_headers_from_plan(&plan);
    let sql = emit_sql(&plan, db_type)?;
    PlanCache::global().insert(cache_key, plan_cache::PlanEntry { plan: std::sync::Arc::new(plan), sql: sql.clone(), headers: headers.clone() });
    Ok((sql, headers))
}

#[cfg(feature = "query_ast")]
fn infer_headers_from_plan(plan: &LogicalQueryPlan) -> Vec<String> {
    use logical::LogicalQueryPlan as L;
    use logical::Expr as E;
    fn find_projection(p: &L) -> Option<&Vec<E>> { match p { L::Projection { exprs, .. } => Some(exprs), L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } => find_projection(input), L::Join { left, .. } => find_projection(left), L::TableScan { .. } => None } }
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
