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
pub mod executor;
#[cfg(feature = "query_ast")]
pub mod executors;

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

    #[cfg(feature = "query_ast")]
    fn hash_expr(e: &logical::Expr, h: &mut impl Hasher) {
        use logical::Expr as E;
        std::mem::discriminant(e).hash(h);
        match e {
            E::Column(c)|E::StringLiteral(c)|E::Number(c)|E::Raw(c) => c.to_ascii_lowercase().hash(h),
            E::BinaryOp { left, op, right } => { op.to_ascii_lowercase().hash(h); hash_expr(left,h); hash_expr(right,h); }
            E::FuncCall { name, args } => { name.to_ascii_lowercase().hash(h); for a in args { hash_expr(a,h);} }
            E::Alias { expr, alias } => { alias.to_ascii_lowercase().hash(h); hash_expr(expr,h); }
            E::Null => {},
            E::Boolean(b)=> b.hash(h),
            E::Not(inner)=> hash_expr(inner,h),
            E::IsNull { expr, negated } => { let _ = *negated; hash_expr(expr,h); }
            E::Like { expr, pattern, negated } => { (*negated as u8).hash(h); hash_expr(expr,h); hash_expr(pattern,h); }
            E::InList { expr, list, negated } => { (*negated as u8).hash(h); hash_expr(expr,h); for i in list { hash_expr(i,h);} }
            E::Case { operand, when_then, else_expr } => { if let Some(o)=operand { hash_expr(o,h);} for (w,t) in when_then { hash_expr(w,h); hash_expr(t,h);} if let Some(e2)=else_expr { hash_expr(e2,h);} }
            E::Subquery { sql, correlated } => { sql.trim().to_ascii_lowercase().hash(h); correlated.hash(h); }
            E::WindowFunc { name, args, partition_by, order_by, frame } => {
                name.to_ascii_lowercase().hash(h);
                for a in args { hash_expr(a,h); }
                for p in partition_by { hash_expr(p,h); }
                for (o,asc) in order_by { hash_expr(o,h); asc.hash(h); }
                if let Some(f)=frame { f.to_ascii_lowercase().hash(h); }
            }
            E::Star => {"*".hash(h);}    }
    }

    #[cfg(feature = "query_ast")]
    fn hash_plan(p: &LogicalQueryPlan, h: &mut impl Hasher) {
        use logical::LogicalQueryPlan as L;
        std::mem::discriminant(p).hash(h);
        match p {
            L::Projection { exprs, input } => { for e in exprs { hash_expr(e,h);} hash_plan(input,h); }
            L::Distinct { input } => hash_plan(input,h),
            L::Filter { predicate, input } => { hash_expr(predicate,h); hash_plan(input,h); }
            L::Sort { items, input } => { for it in items { hash_expr(&it.expr,h); it.asc.hash(h);} hash_plan(input,h); }
            L::Limit { limit, offset, input } => { limit.hash(h); offset.hash(h); hash_plan(input,h); }
            L::Group { group_exprs, input } => { for g in group_exprs { hash_expr(g,h);} hash_plan(input,h); }
            L::Join { left, right, on, kind } => { (*kind as u8).hash(h); if let Some(o)=on { hash_expr(o,h);} hash_plan(left,h); hash_plan(right,h);} 
            L::Having { predicate, input } => { hash_expr(predicate,h); hash_plan(input,h);} 
            L::With { ctes, input } => { for (n,s) in ctes { n.to_ascii_lowercase().hash(h); s.to_ascii_lowercase().hash(h);} hash_plan(input,h); }
            L::SetOp { left, right, op } => { (*op as u8).hash(h); hash_plan(left,h); hash_plan(right,h); }
            L::TableScan { table, alias } => { table.to_ascii_lowercase().hash(h); if let Some(a)=alias { a.to_ascii_lowercase().hash(h);} } 
            L::SubqueryScan { sql, alias, correlated } => { sql.trim().to_ascii_lowercase().hash(h); alias.to_ascii_lowercase().hash(h); correlated.hash(h);} 
        }
    }

    fn canonicalize_space(s: &str) -> String {
        let mut out = s.trim().trim_end_matches(';').to_string();
        out = out.split_whitespace().collect::<Vec<_>>().join(" "); out
    }
    // Basic structural fingerprint: lowercase, remove redundant spaces and quote style agnostic
    fn structural_fingerprint(s: &str) -> u64 {
        let norm = canonicalize_space(&s.to_ascii_lowercase().replace(['`', '[', ']'], "\""));
        let mut hasher = DefaultHasher::new(); norm.hash(&mut hasher); hasher.finish()
    }
    // let _canon = canonicalize_space(raw); // reserved for future debugging
    let fp_struct = structural_fingerprint(raw);
    // We'll compute precise logical hash after parsing; initial quick key for early hit
    let pre_key = format!("pre{}::{:?}::{:?}::{}", fp_struct, db_type, pagination, inject_auto_limit);
    if let Some(entry) = PlanCache::global().get(&pre_key) { return Ok((entry.sql, entry.headers)); }

    let mut working_sql = raw.to_string();
    // Very simple CTE inlining (Phase A): if WITH cte AS (sub) SELECT ... ; only support single simple CTE referenced once
    if working_sql.trim_start().to_ascii_lowercase().starts_with("with ")
        && let Some(as_pos) = working_sql.to_ascii_lowercase().find(" as (") {
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
    let mut plan = parse_single_select_to_plan(&working_sql)?;
    // Compute precise hash of plan shape + expressions
    let mut hasher = DefaultHasher::new();
    hash_plan(&plan, &mut hasher);
    let logical_fp = hasher.finish();
    let cache_key = format!("plan{}::{:?}::{:?}::{}", logical_fp, db_type, pagination, inject_auto_limit);
    if let Some(entry) = PlanCache::global().get(&cache_key) { return Ok((entry.sql, entry.headers)); }
    let pagination = pagination.map(|(page, size)| Pagination { page, page_size: size });
    apply_basic_rewrites(&mut plan, inject_auto_limit, pagination)?;
    // Extract remaining CTE names if any after rewrites (for debug UI)
    let mut remaining_ctes: Option<Vec<String>> = None;
    if let logical::LogicalQueryPlan::With { ctes, .. } = &plan && !ctes.is_empty() { remaining_ctes = Some(ctes.iter().map(|(n,_)| n.clone()).collect()); }
    let headers = infer_headers_from_plan(&plan);
    let sql = emit_sql(&plan, db_type)?;
    // (Optionally we could store remaining_ctes inside PlanEntry in future)
    PlanCache::global().insert(cache_key.clone(), plan_cache::PlanEntry { plan: std::sync::Arc::new(plan), sql: sql.clone(), headers: headers.clone() });
    // Hook: store debug info into thread-local so UI can pick it up (simple static slot)
    STORE_DEBUG.with(|slot| { *slot.borrow_mut() = Some((logical_fp, cache_key.clone(), remaining_ctes)); });
    Ok((sql, headers))
}

type StoreDebugType = Option<(u64,String,Option<Vec<String>>)>;

thread_local! { static STORE_DEBUG: std::cell::RefCell<StoreDebugType> = const { std::cell::RefCell::new(None) }; }

#[cfg(feature="query_ast")]
pub fn take_last_debug() -> Option<(u64,String,Option<Vec<String>>)> { let mut out=None; STORE_DEBUG.with(|s| { out = s.borrow_mut().take(); }); out }

#[cfg(feature = "query_ast")]
pub fn cache_stats() -> (u64,u64) {
    use plan_cache::PlanCache; PlanCache::global().stats()
}

#[cfg(feature = "query_ast")]
pub fn last_rewrite_rules() -> Vec<String> { crate::query_ast::rewrite::take_last_rules() }

#[cfg(feature = "query_ast")]
pub fn plan_structural_hash(raw: &str, db_type: &DatabaseType, pagination: Option<(u64,u64)>, inject_auto_limit: bool) -> Result<(u64,String), QueryAstError> {
    use parser::parse_single_select_to_plan; use std::collections::hash_map::DefaultHasher; use std::hash::{Hasher,Hash};
    let plan = parse_single_select_to_plan(raw)?; let mut hasher = DefaultHasher::new();
    fn hash_expr(e:&logical::Expr,h:&mut impl Hasher){ use logical::Expr as E; std::mem::discriminant(e).hash(h); match e { E::Column(c)|E::StringLiteral(c)|E::Number(c)|E::Raw(c)=>c.to_ascii_lowercase().hash(h), E::BinaryOp{left,op,right}=>{op.to_ascii_lowercase().hash(h); hash_expr(left,h); hash_expr(right,h);} , E::FuncCall{name,args}=>{name.to_ascii_lowercase().hash(h); for a in args { hash_expr(a,h);} }, E::Alias{expr,alias}=>{alias.to_ascii_lowercase().hash(h); hash_expr(expr,h);}, E::Null=>{}, E::Boolean(b)=>b.hash(h), E::Not(i)=>hash_expr(i,h), E::IsNull{expr,negated}=>{negated.hash(h); hash_expr(expr,h);} , E::Like{expr,pattern,negated}=>{negated.hash(h); hash_expr(expr,h); hash_expr(pattern,h);} , E::InList{expr,list,negated}=>{negated.hash(h); hash_expr(expr,h); for i in list { hash_expr(i,h);} }, E::Case{operand,when_then,else_expr}=>{ if let Some(o)=operand { hash_expr(o,h);} for (w,t) in when_then { hash_expr(w,h); hash_expr(t,h);} if let Some(e2)=else_expr { hash_expr(e2,h);} }, E::Subquery{sql,correlated}=>{sql.trim().to_ascii_lowercase().hash(h); correlated.hash(h);} , E::WindowFunc{name,args,partition_by,order_by,frame}=>{name.to_ascii_lowercase().hash(h); for a in args { hash_expr(a,h);} for p in partition_by { hash_expr(p,h);} for (o,asc) in order_by { hash_expr(o,h); asc.hash(h);} if let Some(f)=frame { f.to_ascii_lowercase().hash(h);} }, E::Star=>{"*".hash(h);} }
    }
    fn hash_plan(p:&LogicalQueryPlan,h:&mut impl Hasher){ use logical::LogicalQueryPlan as L; std::mem::discriminant(p).hash(h); match p { L::Projection{exprs,input}=>{for e in exprs { hash_expr(e,h);} hash_plan(input,h);} , L::Distinct{input}|L::Group{input,..}|L::Filter{input,..}|L::Sort{input,..}|L::Limit{input,..}|L::Having{input,..}|L::With{input,..}=>hash_plan(input,h), L::Join{left,right,on,kind}=>{ (*kind as u8).hash(h); if let Some(o)=on { hash_expr(o,h);} hash_plan(left,h); hash_plan(right,h);} , L::SetOp { left, right, op }=>{ (*op as u8).hash(h); hash_plan(left,h); hash_plan(right,h);} , L::TableScan{table,alias}=>{table.to_ascii_lowercase().hash(h); if let Some(a)=alias { a.to_ascii_lowercase().hash(h);} }, L::SubqueryScan{sql,alias,correlated}=>{sql.trim().to_ascii_lowercase().hash(h); alias.to_ascii_lowercase().hash(h); correlated.hash(h);} }
    }
    hash_plan(&plan,&mut hasher); let structural = hasher.finish();
    let cache_key = format!("{:x}::{:?}::{:?}::{}", structural, db_type, pagination, inject_auto_limit);
    Ok((structural, cache_key))
}

#[cfg(feature = "query_ast")]
pub fn debug_plan(raw: &str, db_type: &DatabaseType) -> Result<String, QueryAstError> {
    use parser::parse_single_select_to_plan;
    let plan = parse_single_select_to_plan(raw)?;
    fn fmt(plan: &LogicalQueryPlan, indent: usize, out: &mut String) {
        use logical::LogicalQueryPlan as L;
        let pad = "  ".repeat(indent);
        match plan {
            L::TableScan { table, alias } => { out.push_str(&format!("{}TableScan({} alias={:?})\n", pad, table, alias)); }
            L::SubqueryScan { alias, .. } => { out.push_str(&format!("{}SubqueryScan(alias={})\n", pad, alias)); }
            L::Projection { exprs, input } => { out.push_str(&format!("{}Projection {:?}\n", pad, exprs.len())); fmt(input, indent+1, out); }
            L::Distinct { input } => { out.push_str(&format!("{}Distinct\n", pad)); fmt(input, indent+1, out); }
            L::Filter { predicate, input } => { out.push_str(&format!("{}Filter {:?}\n", pad, predicate)); fmt(input, indent+1, out); }
            L::Sort { items, input } => { out.push_str(&format!("{}Sort {:?}\n", pad, items.len())); fmt(input, indent+1, out); }
            L::Limit { limit, offset, input } => { out.push_str(&format!("{}Limit limit={} offset={}\n", pad, limit, offset)); fmt(input, indent+1, out); }
            L::Group { group_exprs, input } => { out.push_str(&format!("{}Group {:?}\n", pad, group_exprs.len())); fmt(input, indent+1, out); }
            L::Join { left, right, .. } => { out.push_str(&format!("{}Join\n", pad)); fmt(left, indent+1, out); fmt(right, indent+1, out); }
            L::Having { predicate, input } => { out.push_str(&format!("{}Having {:?}\n", pad, predicate)); fmt(input, indent+1, out); }
            L::With { ctes, input } => { out.push_str(&format!("{}With ctes={:?}\n", pad, ctes.iter().map(|(n,_)| n).collect::<Vec<_>>())); fmt(input, indent+1, out); }
            L::SetOp { left, right, op } => { out.push_str(&format!("{}SetOp {:?}\n", pad, op)); fmt(left, indent+1, out); fmt(right, indent+1, out); }
        }
    }
    let mut s = String::new();
    s.push_str(&format!("-- debug plan for {:?}\n", db_type));
    fmt(&plan, 0, &mut s);
    Ok(s)
}

#[cfg(feature = "query_ast")]
pub fn plan_metrics(raw: &str) -> Result<(usize,usize,usize,usize,usize), QueryAstError> { // (nodes, depth, subqueries_total, subqueries_correlated, windows)
    use parser::parse_single_select_to_plan; let plan = parse_single_select_to_plan(raw)?; 
    fn walk(p: &LogicalQueryPlan, depth: usize, stats: &mut (usize,usize,usize,usize,usize)) { stats.0+=1; stats.1=stats.1.max(depth); use logical::LogicalQueryPlan as L; match p { L::Projection { input, .. } | L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } | L::With { input, .. } => walk(input, depth+1, stats), L::Join { left, right, .. } => { walk(left, depth+1, stats); walk(right, depth+1, stats); }, L::SetOp { left, right, .. } => { walk(left, depth+1, stats); walk(right, depth+1, stats); }, L::TableScan { .. } | L::SubqueryScan { .. } => {} }
    }
    fn count_expr(e: &logical::Expr, subs: &mut usize, correlated: &mut usize, wins: &mut usize) { use logical::Expr as E; match e { E::Subquery { correlated: c, .. } => { *subs+=1; if *c { *correlated+=1; } }, E::WindowFunc { .. } => *wins+=1, E::Alias { expr, .. } => count_expr(expr, subs, correlated,wins), E::BinaryOp { left, right, .. } => { count_expr(left,subs,correlated,wins); count_expr(right,subs,correlated,wins); }, E::FuncCall { args, .. } => { for a in args { count_expr(a,subs,correlated,wins);} }, E::Case { when_then, operand, else_expr } => { if let Some(o)=operand { count_expr(o,subs,correlated,wins);} for (w,t) in when_then { count_expr(w,subs,correlated,wins); count_expr(t,subs,correlated,wins);} if let Some(e2)=else_expr { count_expr(e2,subs,correlated,wins);} }, E::InList { expr, list, .. } => { count_expr(expr,subs,correlated,wins); for l in list { count_expr(l,subs,correlated,wins);} }, E::Like { expr, pattern, .. } => { count_expr(expr,subs,correlated,wins); count_expr(pattern,subs,correlated,wins);} , E::Not(inner)=> count_expr(inner,subs,correlated,wins), E::IsNull { expr, .. } => count_expr(expr,subs,correlated,wins), _ => {} }
    }
    // Drill down to find projection expressions for counting subqueries/windows
    fn collect_projection(p:&LogicalQueryPlan, out:&mut Vec<logical::Expr>) { use logical::LogicalQueryPlan as L; match p { L::Projection { exprs, input } => { out.extend(exprs.clone()); collect_projection(input,out);} L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } | L::With { input, .. } => collect_projection(input,out), L::Join { left, right, .. } | L::SetOp { left, right, .. } => { collect_projection(left,out); collect_projection(right,out);} L::TableScan { .. } | L::SubqueryScan { .. } => {} } }
    let mut stats=(0,0,0,0,0); walk(&plan,0,&mut stats); let mut exprs=Vec::new(); collect_projection(&plan,&mut exprs); for e in &exprs { count_expr(e,&mut stats.2,&mut stats.3,&mut stats.4);} Ok(stats)
}

#[cfg(feature = "query_ast")]
fn infer_headers_from_plan(plan: &LogicalQueryPlan) -> Vec<String> {
    use logical::LogicalQueryPlan as L;
    use logical::Expr as E;
    fn find_projection(p: &L) -> Option<&Vec<E>> { match p { L::Projection { exprs, .. } => Some(exprs), L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } | L::With { input, .. } => find_projection(input), L::Join { left, .. } | L::SetOp { left, .. } => find_projection(left), L::TableScan { .. } | L::SubqueryScan { .. } => None } }
    if let Some(exprs) = find_projection(plan) {
        let mut out = Vec::new();
        for e in exprs {
            match e {
                E::Alias { alias, .. } => out.push(alias.clone()),
                E::Column(c) => out.push(c.split('.').next_back().unwrap_or(c).to_string()),
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
                E::Subquery { .. } => out.push("subquery".to_string()),
                E::WindowFunc { name, .. } => out.push(name.to_ascii_lowercase()),
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
