use super::{errors::RewriteError, logical::LogicalQueryPlan};
use std::sync::Mutex;

// Track last applied rewrite rule names for debug panel
static LAST_RULES: once_cell::sync::Lazy<Mutex<Vec<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(Vec::new()));

pub fn take_last_rules() -> Vec<String> {
    LAST_RULES.lock().map(|v| v.clone()).unwrap_or_default()
}

#[derive(Clone, Copy, Debug)]
enum Rule {
    AutoLimit,
    PaginationLimit,
    FilterPushdown,
    MergeFilters,
    RemoveRedundantProjection,
    LimitIntoSubquery,
    AnnotateCorrelation,
    InlineSingleUseCte,
    ProjectionPrune,
}

impl Rule {
    fn name(&self) -> &'static str {
        match self {
            Rule::AutoLimit => "auto_limit",
            Rule::PaginationLimit => "pagination_limit",
            Rule::FilterPushdown => "filter_pushdown",
            Rule::MergeFilters => "merge_filters",
            Rule::RemoveRedundantProjection => "remove_redundant_projection",
            Rule::LimitIntoSubquery => "limit_into_subquery",
            Rule::AnnotateCorrelation => "annotate_correlation",
            Rule::InlineSingleUseCte => "inline_single_use_cte",
            Rule::ProjectionPrune => "projection_prune",
        }
    }
}

pub struct Pagination {
    pub page: u64,
    pub page_size: u64,
}

pub fn apply_basic_rewrites(
    plan: &mut LogicalQueryPlan,
    inject_auto_limit: bool,
    pagination: Option<Pagination>,
) -> Result<(), RewriteError> {
    let mut applied: Vec<String> = Vec::new();
    // Iterative fixed-point (small cap)
    for _iter in 0..4 {
        // small loop; rules idempotent
        let mut changed = false;
        if inject_auto_limit && !has_limit(plan) {
            let new = LogicalQueryPlan::Limit {
                limit: 1000,
                offset: 0,
                input: Box::new(plan.clone()),
            };
            *plan = new;
            applied.push(Rule::AutoLimit.name().into());
            changed = true;
        }
        if let Some(ref p) = pagination
            && replace_or_add_limit_record(plan, p.page_size, p.page * p.page_size)
        {
            applied.push(Rule::PaginationLimit.name().into());
            changed = true;
        }
        if pushdown_filters(plan) {
            applied.push(Rule::FilterPushdown.name().into());
            changed = true;
        }
        if merge_consecutive_filters(plan) {
            applied.push(Rule::MergeFilters.name().into());
            changed = true;
        }
        if remove_redundant_projection(plan) {
            applied.push(Rule::RemoveRedundantProjection.name().into());
            changed = true;
        }
        if projection_prune(plan) {
            applied.push(Rule::ProjectionPrune.name().into());
            changed = true;
        }
        if inline_single_use_ctes(plan) {
            applied.push(Rule::InlineSingleUseCte.name().into());
            changed = true;
        }
        if try_pushdown_limit_into_subquery(plan) {
            applied.push(Rule::LimitIntoSubquery.name().into());
            changed = true;
        }
        if annotate_correlation(plan) {
            applied.push(Rule::AnnotateCorrelation.name().into());
            changed = true;
        }
        if !changed {
            break;
        }
    }
    if let Ok(mut guard) = LAST_RULES.lock() {
        *guard = applied;
    }
    Ok(())
}

fn has_limit(plan: &LogicalQueryPlan) -> bool {
    match plan {
        LogicalQueryPlan::Limit { .. } => true,
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => has_limit(input),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => has_limit(left) || has_limit(right),
        LogicalQueryPlan::TableScan { .. } | LogicalQueryPlan::SubqueryScan { .. } => false,
    }
}

fn replace_or_add_limit_record(plan: &mut LogicalQueryPlan, limit: u64, offset: u64) -> bool {
    let mut changed = false;
    match plan {
        LogicalQueryPlan::Limit {
            limit: l,
            offset: o,
            ..
        } => {
            if *l != limit || *o != offset {
                *l = limit;
                *o = offset;
                changed = true;
            }
        }
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => {
            changed |= replace_or_add_limit_record(input, limit, offset);
        }
        LogicalQueryPlan::Join { left, .. } => {
            changed |= replace_or_add_limit_record(left, limit, offset);
        }
        LogicalQueryPlan::SetOp { left, .. } => {
            changed |= replace_or_add_limit_record(left, limit, offset);
        }
        LogicalQueryPlan::TableScan { .. } | LogicalQueryPlan::SubqueryScan { .. } => {
            let new = LogicalQueryPlan::Limit {
                limit,
                offset,
                input: Box::new(plan.clone()),
            };
            *plan = new;
            changed = true;
        }
    }
    changed
}

// Treat certain nodes as barriers (no pushdown across)
fn is_barrier(p: &LogicalQueryPlan) -> bool {
    use super::logical::LogicalQueryPlan as L;
    matches!(p, L::Group { .. } | L::Distinct { .. } | L::SetOp { .. })
}

fn pushdown_filters(plan: &mut LogicalQueryPlan) -> bool {
    use super::logical::LogicalQueryPlan as L;
    let mut changed = false;
    if let L::Projection { .. } = plan {
        // Look for Projection(Filter(inner)) pattern
        if let L::Projection {
            exprs,
            input: inner_box,
        } = plan
            && let L::Filter {
                predicate,
                input: filter_inner,
            } = &**inner_box
            && !contains_group_or_distinct(filter_inner)
            && !is_barrier(filter_inner)
        {
            let new_plan = L::Filter {
                predicate: predicate.clone(),
                input: Box::new(L::Projection {
                    exprs: exprs.clone(),
                    input: filter_inner.clone(),
                }),
            };
            *plan = new_plan;
            changed = true;
        }
    }
    changed
}

fn contains_group_or_distinct(plan: &LogicalQueryPlan) -> bool {
    use super::logical::LogicalQueryPlan as L;
    match plan {
        L::Group { .. } | L::Distinct { .. } => true,
        L::Projection { input, .. }
        | L::Filter { input, .. }
        | L::Sort { input, .. }
        | L::Limit { input, .. }
        | L::Having { input, .. }
        | L::With { input, .. } => contains_group_or_distinct(input),
        L::Join { left, right, .. } | L::SetOp { left, right, .. } => {
            contains_group_or_distinct(left) || contains_group_or_distinct(right)
        }
        L::TableScan { .. } | L::SubqueryScan { .. } => false,
    }
}

// Basic projection pruning: remove unused expressions from Projection if never referenced upstream.
// We perform a single bottom-up pass collecting needed column names (heuristic: raw column tokens and aliases).
fn projection_prune(plan: &mut LogicalQueryPlan) -> bool {
    use super::logical::{Expr, LogicalQueryPlan as L};
    let mut changed = false;
    // Collect required column/alias names from parent contexts (simple heuristic through recursion)
    fn collect_needed(p: &L, needed: &mut std::collections::HashSet<String>) {
        match p {
            L::Filter { predicate, input } => {
                collect_expr_cols(predicate, needed);
                collect_needed(input, needed);
            }
            L::Sort { items, input } => {
                for it in items {
                    collect_expr_cols(&it.expr, needed);
                }
                collect_needed(input, needed);
            }
            L::Limit { input, .. } => collect_needed(input, needed),
            L::Having { predicate, input } => {
                collect_expr_cols(predicate, needed);
                collect_needed(input, needed);
            }
            L::Group { group_exprs, input } => {
                for g in group_exprs {
                    collect_expr_cols(g, needed);
                }
                collect_needed(input, needed);
            }
            L::Projection { input, .. } => {
                collect_needed(input, needed);
            }
            L::Distinct { input } | L::With { input, .. } => collect_needed(input, needed),
            L::Join {
                left, right, on, ..
            } => {
                if let Some(o) = on {
                    collect_expr_cols(o, needed);
                }
                collect_needed(left, needed);
                collect_needed(right, needed);
            }
            L::SetOp { left, right, .. } => {
                collect_needed(left, needed);
                collect_needed(right, needed);
            }
            L::TableScan { .. } | L::SubqueryScan { .. } => {}
        }
    }
    fn collect_expr_cols(e: &Expr, out: &mut std::collections::HashSet<String>) {
        use Expr::*;
        match e {
            Column(c) => {
                out.insert(c.split('.').next_back().unwrap_or(c).to_ascii_lowercase());
            }
            Alias { expr, alias } => {
                out.insert(alias.to_ascii_lowercase());
                collect_expr_cols(expr, out);
            }
            BinaryOp { left, right, .. } => {
                collect_expr_cols(left, out);
                collect_expr_cols(right, out);
            }
            FuncCall { args, .. } => {
                for a in args {
                    collect_expr_cols(a, out);
                }
            }
            Not(inner) => collect_expr_cols(inner, out),
            IsNull { expr, .. } => collect_expr_cols(expr, out),
            Like { expr, pattern, .. } => {
                collect_expr_cols(expr, out);
                collect_expr_cols(pattern, out);
            }
            InList { expr, list, .. } => {
                collect_expr_cols(expr, out);
                for l in list {
                    collect_expr_cols(l, out);
                }
            }
            Case {
                operand,
                when_then,
                else_expr,
            } => {
                if let Some(o) = operand {
                    collect_expr_cols(o, out);
                }
                for (w, t) in when_then {
                    collect_expr_cols(w, out);
                    collect_expr_cols(t, out);
                }
                if let Some(e2) = else_expr {
                    collect_expr_cols(e2, out);
                }
            }
            WindowFunc {
                args,
                partition_by,
                order_by,
                ..
            } => {
                for a in args {
                    collect_expr_cols(a, out);
                }
                for p in partition_by {
                    collect_expr_cols(p, out);
                }
                for (o, _) in order_by {
                    collect_expr_cols(o, out);
                }
            }
            Subquery { .. } | Star | StringLiteral(_) | Number(_) | Raw(_) | Null | Boolean(_) => {}
        }
    }
    // Recursive pruning
    fn recurse(
        p: &mut L,
        changed: &mut bool,
        needed_parent: &std::collections::HashSet<String>,
        is_root: bool,
    ) {
        match p {
            L::Projection { exprs, input } => {
                // Do NOT prune at the root projection: keep user-selected columns intact.
                // Also, if there is no explicit need from parent contexts, keep the full projection.
                if !is_root && !needed_parent.is_empty() {
                    // Determine which expressions are referenced by parent contexts (needed_parent)
                    let mut kept = Vec::with_capacity(exprs.len());
                    for e in exprs.iter() {
                        let keep = match e {
                            Expr::Alias { alias, .. } => {
                                needed_parent.contains(&alias.to_ascii_lowercase())
                            }
                            Expr::Column(c) => needed_parent.contains(
                                &c.split('.').next_back().unwrap_or(c).to_ascii_lowercase(),
                            ),
                            Expr::Star => true, // cannot prune * safely
                            _ => true,          // keep complex expressions conservatively
                        };
                        if keep {
                            kept.push(e.clone());
                        }
                    }
                    if kept.len() != exprs.len() {
                        *exprs = kept;
                        *changed = true;
                    }
                }
                // Build next needed set from current projection outputs (all aliases/columns that remain)
                let mut next_needed = std::collections::HashSet::new();
                for e in exprs.iter() {
                    match e {
                        Expr::Alias { alias, .. } => {
                            next_needed.insert(alias.to_ascii_lowercase());
                        }
                        Expr::Column(c) => {
                            next_needed
                                .insert(c.split('.').next_back().unwrap_or(c).to_ascii_lowercase());
                        }
                        Expr::Star => { /* wildcard ensures everything */ }
                        _ => {}
                    }
                }
                recurse(input, changed, &next_needed, false);
            }
            L::Filter { input, .. }
            | L::Sort { input, .. }
            | L::Limit { input, .. }
            | L::Distinct { input }
            | L::Group { input, .. }
            | L::Having { input, .. }
            | L::With { input, .. } => recurse(input, changed, needed_parent, is_root),
            L::Join { left, right, .. } | L::SetOp { left, right, .. } => {
                recurse(left, changed, needed_parent, is_root);
                recurse(right, changed, needed_parent, is_root);
            }
            L::TableScan { .. } | L::SubqueryScan { .. } => {}
        }
    }
    // First accumulate global needed set from top-level (root consumers); start with empty then treat root as consumer of all projection outputs (so prune only unreachable nested projections)
    let mut needed = std::collections::HashSet::new();
    collect_needed(plan, &mut needed);
    // Root needed set = everything referenced above base projections
    // Root call: mark as root to avoid pruning top-level projection
    recurse(plan, &mut changed, &needed, true);
    changed
}

// Merge Filter(Filter(X)) -> single Filter with AND predicate (best-effort on Raw join)
fn merge_consecutive_filters(plan: &mut LogicalQueryPlan) -> bool {
    use super::logical::LogicalQueryPlan as L;
    let mut changed = false;
    match plan {
        L::Filter { predicate, input } => {
            merge_consecutive_filters(input);
            if let L::Filter {
                predicate: inner_pred,
                input: inner_input,
            } = &mut **input
            {
                // Combine by creating Raw binary AND expression for now
                let combined = super::logical::Expr::Raw(format!(
                    "({}) AND ({})",
                    display_expr(predicate),
                    display_expr(inner_pred)
                ));
                *predicate = combined;
                *input = inner_input.clone();
                changed = true;
            }
        }
        L::Projection { input, .. }
        | L::Sort { input, .. }
        | L::Limit { input, .. }
        | L::Distinct { input }
        | L::Group { input, .. }
        | L::Having { input, .. }
        | L::With { input, .. } => {
            merge_consecutive_filters(input);
        }
        L::Join { left, right, .. } | L::SetOp { left, right, .. } => {
            merge_consecutive_filters(left);
            merge_consecutive_filters(right);
        }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
    }
    changed
}

fn display_expr(e: &super::logical::Expr) -> String {
    match e {
        super::logical::Expr::Raw(s) => s.clone(),
        _ => format!("{:?}", e),
    }
}

// Remove Projection that is identity (all columns *) directly above another Projection
fn remove_redundant_projection(plan: &mut LogicalQueryPlan) -> bool {
    use super::logical::{Expr, LogicalQueryPlan as L};
    let mut changed = false;
    match plan {
        L::Projection { exprs, input } => {
            remove_redundant_projection(input);
            if exprs.len() == 1
                && matches!(exprs[0], Expr::Star)
                && matches!(&**input, L::Projection { .. })
                && let L::Projection {
                    exprs: inner_exprs,
                    input: inner_input,
                } = &**input
            {
                *exprs = inner_exprs.clone();
                *input = inner_input.clone();
                changed = true;
            }
        }
        L::Filter { input, .. }
        | L::Sort { input, .. }
        | L::Limit { input, .. }
        | L::Distinct { input }
        | L::Group { input, .. }
        | L::Having { input, .. }
        | L::With { input, .. } => {
            remove_redundant_projection(input);
        }
        L::Join { left, right, .. } | L::SetOp { left, right, .. } => {
            remove_redundant_projection(left);
            remove_redundant_projection(right);
        }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
    }
    changed
}

// Attempt: push outer LIMIT into SubqueryScan if subquery lacks its own LIMIT and offset=0
fn try_pushdown_limit_into_subquery(plan: &mut LogicalQueryPlan) -> bool {
    use super::logical::LogicalQueryPlan as L;
    let mut changed = false;
    match plan {
        L::Limit {
            limit,
            offset,
            input,
        } if *offset == 0 => {
            if let L::Projection { input: inner2, .. }
            | L::Distinct { input: inner2 }
            | L::Sort { input: inner2, .. } = &mut **input
            {
                // Recurse first
                changed |= try_pushdown_limit_into_subquery(inner2);
            }
            if let L::SubqueryScan {
                sql,
                alias: _,
                correlated,
            } = &mut **input
                && !*correlated
                && !sql.to_ascii_lowercase().contains(" limit ")
            {
                sql.push_str(&format!(" LIMIT {}", limit));
                changed = true;
            }
        }
        L::Limit { input, .. } => {
            changed |= try_pushdown_limit_into_subquery(input);
        }
        L::Projection { input, .. }
        | L::Filter { input, .. }
        | L::Sort { input, .. }
        | L::Distinct { input }
        | L::Group { input, .. }
        | L::Having { input, .. }
        | L::With { input, .. } => {
            changed |= try_pushdown_limit_into_subquery(input);
        }
        L::Join { left, right, .. } => {
            changed |= try_pushdown_limit_into_subquery(left);
            changed |= try_pushdown_limit_into_subquery(right);
        }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
        L::SetOp {
            left: _left,
            right: _right,
            op: _op,
        } => todo!(),
    }
    changed
}

// Correlation annotation heuristic: mark SubqueryScan as correlated if its sql text references any table alias found in ancestor chain.
fn annotate_correlation(plan: &mut LogicalQueryPlan) -> bool {
    let mut aliases = Vec::new();
    collect_aliases(plan, &mut aliases);
    let mut changed = false;
    annotate(plan, &aliases, &mut changed);
    changed
}
fn collect_aliases(plan: &LogicalQueryPlan, out: &mut Vec<String>) {
    match plan {
        LogicalQueryPlan::TableScan { table, alias } => {
            if let Some(a) = alias {
                out.push(a.clone());
            } else {
                out.push(table.clone());
            }
        }
        LogicalQueryPlan::SubqueryScan { alias, .. } => {
            out.push(alias.clone());
        }
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Limit { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => collect_aliases(input, out),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => {
            collect_aliases(left, out);
            collect_aliases(right, out);
        }
    }
}
fn annotate(plan: &mut LogicalQueryPlan, aliases: &[String], changed: &mut bool) {
    match plan {
        LogicalQueryPlan::SubqueryScan {
            sql, correlated, ..
        } => {
            if !*correlated && is_correlated_subquery(sql, aliases) {
                *correlated = true;
                *changed = true;
            }
        }
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Limit { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => annotate(input, aliases, changed),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => {
            annotate(left, aliases, changed);
            annotate(right, aliases, changed);
        }
        LogicalQueryPlan::TableScan { .. } => {}
    }
}

// Structured correlated subquery detection: parse subquery SQL, collect referenced identifiers, compare to outer alias list.
fn is_correlated_subquery(sql: &str, outer_aliases: &[String]) -> bool {
    use sqlparser::ast as sq;
    use sqlparser::dialect::GenericDialect;
    use sqlparser::parser::Parser;
    let dialect = GenericDialect {};
    let parsed = match Parser::parse_sql(&dialect, sql) {
        Ok(p) => p,
        Err(_) => return false,
    }; // if it doesn't parse cleanly, fallback to false (avoid false positives)
    if parsed.len() != 1 {
        return false;
    }
    let stmt = &parsed[0];
    let query = match stmt {
        sq::Statement::Query(q) => q,
        _ => return false,
    };
    let mut local_aliases = std::collections::HashSet::new();
    // Collect local table aliases/names from this subquery's FROM clause(s)
    if let sq::SetExpr::Select(sel) = query.body.as_ref() {
        for table_with_joins in &sel.from {
            collect_local_aliases_from_factor(&table_with_joins.relation, &mut local_aliases);
            for j in &table_with_joins.joins {
                collect_local_aliases_from_factor(&j.relation, &mut local_aliases);
            }
        }
    }
    // Traverse expressions & gather referenced root identifiers
    let mut referenced_roots = std::collections::HashSet::new();
    collect_ident_roots_stmt(stmt, &mut referenced_roots);
    // Outer alias match if referenced root is in outer & NOT shadowed by local alias
    referenced_roots.iter().any(|r| {
        outer_aliases.iter().any(|oa| oa.eq_ignore_ascii_case(r))
            && !local_aliases.contains(&r.to_ascii_lowercase())
    })
}

fn collect_local_aliases_from_factor(
    f: &sqlparser::ast::TableFactor,
    out: &mut std::collections::HashSet<String>,
) {
    use sqlparser::ast::TableFactor as TF;
    match f {
        TF::Table { name, alias, .. } => {
            if let Some(a) = alias {
                out.insert(a.name.value.to_ascii_lowercase());
            } else {
                out.insert(
                    name.0
                        .last()
                        .map(|id| id.value.to_ascii_lowercase())
                        .unwrap_or_default(),
                );
            }
        }
        TF::Derived {
            subquery, alias, ..
        } => {
            if let Some(a) = alias {
                out.insert(a.name.value.to_ascii_lowercase());
            } else {
                // derived w/o alias not referenceable; skip
                // optionally parse inside? not needed for alias listing
                let _ = subquery;
            }
        }
        _ => {}
    }
}

fn collect_ident_roots_stmt(
    stmt: &sqlparser::ast::Statement,
    out: &mut std::collections::HashSet<String>,
) {
    use sqlparser::ast as sq;
    if let sq::Statement::Query(q) = stmt {
        collect_ident_roots_query(q, out);
    }
}
fn collect_ident_roots_query(
    q: &sqlparser::ast::Query,
    out: &mut std::collections::HashSet<String>,
) {
    use sqlparser::ast as sq;
    if let sq::SetExpr::Select(sel) = q.body.as_ref() {
        for proj in &sel.projection {
            match proj {
                sq::SelectItem::UnnamedExpr(e) => collect_ident_roots_expr(e, out),
                sq::SelectItem::ExprWithAlias { expr, .. } => collect_ident_roots_expr(expr, out),
                sq::SelectItem::QualifiedWildcard(obj, _) => {
                    if let Some(id) = obj.0.first() {
                        out.insert(id.value.to_ascii_lowercase());
                    }
                }
                sq::SelectItem::Wildcard(_) => {}
            }
        }
        if let Some(sel_expr) = &sel.selection {
            collect_ident_roots_expr(sel_expr, out);
        }
        match &sel.group_by {
            sq::GroupByExpr::Expressions(exprs, _) => {
                for g in exprs {
                    collect_ident_roots_expr(g, out);
                }
            }
            sq::GroupByExpr::All(_) => {}
        }
        if let Some(h) = &sel.having {
            collect_ident_roots_expr(h, out);
        }
        for fw in &sel.from {
            collect_ident_roots_factor(&fw.relation, out);
            for j in &fw.joins {
                match &j.join_operator {
                    sq::JoinOperator::Inner(constraint)
                    | sq::JoinOperator::LeftOuter(constraint)
                    | sq::JoinOperator::RightOuter(constraint)
                    | sq::JoinOperator::FullOuter(constraint) => {
                        if let sq::JoinConstraint::On(e) = constraint {
                            collect_ident_roots_expr(e, out);
                        }
                    }
                    _ => {}
                }
                collect_ident_roots_factor(&j.relation, out);
            }
        }
    }
    if let Some(ob) = &q.order_by {
        for obe in &ob.exprs {
            collect_ident_roots_expr(&obe.expr, out);
        }
    }
    if let Some(limit) = &q.limit {
        collect_ident_roots_expr(limit, out);
    }
    if let Some(offset) = &q.offset {
        collect_ident_roots_expr(&offset.value, out);
    }
}
fn collect_ident_roots_factor(
    f: &sqlparser::ast::TableFactor,
    out: &mut std::collections::HashSet<String>,
) {
    use sqlparser::ast::TableFactor as TF;
    match f {
        TF::Table { .. } => {}
        TF::Derived { subquery, .. } => {
            if let sqlparser::ast::SetExpr::Select(_sel) = subquery.body.as_ref() {
                // recurse minimal
                let stmt = sqlparser::ast::Statement::Query(subquery.clone());
                collect_ident_roots_stmt(&stmt, out);
            }
        }
        _ => {}
    }
}
fn collect_ident_roots_expr(e: &sqlparser::ast::Expr, out: &mut std::collections::HashSet<String>) {
    use sqlparser::ast as sq;
    match e {
        sq::Expr::Identifier(id) => {
            out.insert(id.value.to_ascii_lowercase());
        }
        sq::Expr::CompoundIdentifier(ids) => {
            if let Some(first) = ids.first() {
                out.insert(first.value.to_ascii_lowercase());
            }
        }
        sq::Expr::BinaryOp { left, right, .. } => {
            collect_ident_roots_expr(left, out);
            collect_ident_roots_expr(right, out);
        }
        sq::Expr::UnaryOp { expr, .. } => collect_ident_roots_expr(expr, out),
        sq::Expr::Function(f) => {
            if let sq::FunctionArguments::List(args_list) = &f.args {
                for arg in &args_list.args {
                    if let sq::FunctionArg::Unnamed(sq::FunctionArgExpr::Expr(ex)) = arg {
                        collect_ident_roots_expr(ex, out);
                    }
                }
            }
            if let Some(sq::WindowType::WindowSpec(spec)) = &f.over {
                for p in &spec.partition_by {
                    collect_ident_roots_expr(p, out);
                }
                for o in &spec.order_by {
                    collect_ident_roots_expr(&o.expr, out);
                }
            }
        }
        sq::Expr::Nested(inner) => collect_ident_roots_expr(inner, out),
        sq::Expr::Like { expr, pattern, .. } | sq::Expr::ILike { expr, pattern, .. } => {
            collect_ident_roots_expr(expr, out);
            collect_ident_roots_expr(pattern, out);
        }
        sq::Expr::InList { expr, list, .. } => {
            collect_ident_roots_expr(expr, out);
            for l in list {
                collect_ident_roots_expr(l, out);
            }
        }
        sq::Expr::IsNull(inner) | sq::Expr::IsNotNull(inner) => {
            collect_ident_roots_expr(inner, out)
        }
        sq::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            if let Some(o) = operand {
                collect_ident_roots_expr(o, out);
            }
            for c in conditions {
                collect_ident_roots_expr(c, out);
            }
            for r in results {
                collect_ident_roots_expr(r, out);
            }
            if let Some(er) = else_result {
                collect_ident_roots_expr(er, out);
            }
        }
        sq::Expr::Between {
            expr, low, high, ..
        } => {
            collect_ident_roots_expr(expr, out);
            collect_ident_roots_expr(low, out);
            collect_ident_roots_expr(high, out);
        }
        sq::Expr::Subquery(sub) => {
            let stmt = sqlparser::ast::Statement::Query(sub.clone());
            collect_ident_roots_stmt(&stmt, out);
        }
        _ => {}
    }
}

// Inline single-use CTEs: For With { ctes, input } count textual references of each CTE name (case-insensitive whole word) in emitted subtree SQL forms (approximation via Raw string occurrences in SubqueryScan sql + TableScan table). If count==1 -> replace occurrences by subquery SQL, drop from ctes list.
fn inline_single_use_ctes(plan: &mut LogicalQueryPlan) -> bool {
    use LogicalQueryPlan as L;
    let mut changed = false;
    match plan {
        L::With { ctes, input } => {
            if ctes.is_empty() {
                return false;
            }
            // Build lowercase names
            let names: Vec<String> = ctes.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
            let mut counts = vec![0usize; names.len()];
            count_cte_refs(input, &names, &mut counts);
            let mut to_inline = Vec::new();
            for (idx, c) in counts.iter().enumerate() {
                if *c == 1 {
                    to_inline.push(idx);
                }
            }
            if !to_inline.is_empty() {
                // Apply replacements inside subtree
                for idx in to_inline.iter().rev() {
                    // reverse so index stable when removing
                    let (name, sql) = ctes[*idx].clone();
                    inline_cte_in_subtree(input, &name, &sql);
                    ctes.remove(*idx);
                    changed = true;
                }
                if ctes.is_empty() {
                    // collapse With wrapper
                    let inner = std::mem::replace(
                        input,
                        Box::new(L::TableScan {
                            table: "__dummy__".into(),
                            alias: None,
                        }),
                    );
                    *plan = *inner; // replace whole plan with inner plan
                }
            }
        }
        L::Projection { input, .. }
        | L::Filter { input, .. }
        | L::Sort { input, .. }
        | L::Limit { input, .. }
        | L::Distinct { input }
        | L::Group { input, .. }
        | L::Having { input, .. } => {
            changed |= inline_single_use_ctes(input);
        }
        L::Join { left, right, .. } | L::SetOp { left, right, .. } => {
            changed |= inline_single_use_ctes(left);
            changed |= inline_single_use_ctes(right);
        }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
    }
    changed
}

fn count_cte_refs(plan: &LogicalQueryPlan, names: &[String], counts: &mut [usize]) {
    match plan {
        LogicalQueryPlan::TableScan { table, alias } => {
            let tgt = alias.as_ref().unwrap_or(table).to_ascii_lowercase();
            for (i, n) in names.iter().enumerate() {
                if tgt == *n {
                    counts[i] += 1;
                }
            }
        }
        LogicalQueryPlan::SubqueryScan { sql, .. } => {
            let lower = sql.to_ascii_lowercase();
            for (i, n) in names.iter().enumerate() {
                if lower
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .any(|tok| tok == n)
                {
                    counts[i] += 1;
                }
            }
        }
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Limit { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => count_cte_refs(input, names, counts),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => {
            count_cte_refs(left, names, counts);
            count_cte_refs(right, names, counts);
        }
    }
}

fn inline_cte_in_subtree(plan: &mut LogicalQueryPlan, name: &str, sql: &str) {
    match plan {
        LogicalQueryPlan::TableScan { table, alias } => {
            let alias_name = alias.clone();
            if alias_name
                .clone()
                .unwrap_or_else(|| table.clone())
                .eq_ignore_ascii_case(name)
            {
                // Replace TableScan with SubqueryScan of CTE body; keep existing alias if present else name
                let final_alias = alias_name.unwrap_or_else(|| name.to_string());
                *plan = LogicalQueryPlan::SubqueryScan {
                    sql: sql.to_string(),
                    alias: final_alias,
                    correlated: false,
                };
            }
        }
        LogicalQueryPlan::SubqueryScan { sql: inner_sql, .. } => {
            // Replace textual references of name with (sql) - simplistic, only if appears as whole token
            let tokens: Vec<&str> = inner_sql.split_whitespace().collect();
            if tokens.iter().any(|t| t.eq_ignore_ascii_case(name)) {
                *inner_sql = inner_sql.replace(name, &format!("({})", sql));
            }
        }
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Limit { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => inline_cte_in_subtree(input, name, sql),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => {
            inline_cte_in_subtree(left, name, sql);
            inline_cte_in_subtree(right, name, sql);
        }
    }
}
