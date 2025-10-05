use super::{logical::LogicalQueryPlan, errors::RewriteError};

pub struct Pagination { pub page: u64, pub page_size: u64 }

pub fn apply_basic_rewrites(plan: &mut LogicalQueryPlan, inject_auto_limit: bool, pagination: Option<Pagination>) -> Result<(), RewriteError> {
    // Walk to find existing limit
    if inject_auto_limit && !has_limit(plan) {
        // Wrap in default LIMIT 1000
        let new = LogicalQueryPlan::Limit { limit: 1000, offset: 0, input: Box::new(plan.clone()) };
        *plan = new;
    }
    if let Some(p) = pagination {
        // Replace/append limit
        replace_or_add_limit(plan, p.page_size, p.page * p.page_size);
    }
    // Simple predicate pushdown: Filter directly above Projection/TableScan chain without Group/Distinct/Join
    pushdown_filters(plan);
    merge_consecutive_filters(plan);
    remove_redundant_projection(plan);
    try_pushdown_limit_into_subquery(plan);
    Ok(())
}

fn has_limit(plan: &LogicalQueryPlan) -> bool { match plan { LogicalQueryPlan::Limit { .. } => true, LogicalQueryPlan::Projection { input, .. } | LogicalQueryPlan::Filter { input, .. } | LogicalQueryPlan::Sort { input, .. } | LogicalQueryPlan::Distinct { input } | LogicalQueryPlan::Group { input, .. } | LogicalQueryPlan::Having { input, .. } => has_limit(input), LogicalQueryPlan::Join { left, right, .. } => has_limit(left) || has_limit(right), LogicalQueryPlan::TableScan { .. } | LogicalQueryPlan::SubqueryScan { .. } => false } }

fn replace_or_add_limit(plan: &mut LogicalQueryPlan, limit: u64, offset: u64) {
    match plan {
        LogicalQueryPlan::Limit { limit: l, offset: o, .. } => { *l = limit; *o = offset; }
    LogicalQueryPlan::Projection { input, .. } | LogicalQueryPlan::Filter { input, .. } | LogicalQueryPlan::Sort { input, .. } | LogicalQueryPlan::Distinct { input } | LogicalQueryPlan::Group { input, .. } | LogicalQueryPlan::Having { input, .. } => replace_or_add_limit(input, limit, offset),
    LogicalQueryPlan::Join { left, .. } => replace_or_add_limit(left, limit, offset),
    LogicalQueryPlan::TableScan { .. } | LogicalQueryPlan::SubqueryScan { .. } => { let new = LogicalQueryPlan::Limit { limit, offset, input: Box::new(plan.clone())}; *plan = new; }
    }
}

fn pushdown_filters(plan: &mut LogicalQueryPlan) {
    use super::logical::LogicalQueryPlan as L;
    match plan {
    L::Projection { .. } => {
            // Look for Projection(Filter(inner)) pattern
            if let L::Projection { exprs, input: inner_box } = plan {
                if let L::Filter { predicate, input: filter_inner } = &**inner_box {
                    if !contains_group_or_distinct(filter_inner) {
                        let new_plan = L::Filter {
                            predicate: predicate.clone(),
                            input: Box::new(L::Projection { exprs: exprs.clone(), input: filter_inner.clone() })
                        };
                        *plan = new_plan;
                    }
                }
            }
        }
        _ => {}
    }
}

fn contains_group_or_distinct(plan: &LogicalQueryPlan) -> bool {
    use super::logical::LogicalQueryPlan as L;
    match plan {
        L::Group { .. } | L::Distinct { .. } => true,
        L::Projection { input, .. } | L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Having { input, .. } => contains_group_or_distinct(input),
        L::Join { left, right, .. } => contains_group_or_distinct(left) || contains_group_or_distinct(right),
    L::TableScan { .. } | L::SubqueryScan { .. } => false,
    }
}

// Merge Filter(Filter(X)) -> single Filter with AND predicate (best-effort on Raw join)
fn merge_consecutive_filters(plan: &mut LogicalQueryPlan) {
    use super::logical::LogicalQueryPlan as L;
    match plan {
        L::Filter { predicate, input } => {
            merge_consecutive_filters(input);
            if let L::Filter { predicate: inner_pred, input: inner_input } = &mut **input {
                // Combine by creating Raw binary AND expression for now
                let combined = super::logical::Expr::Raw(format!("({}) AND ({})", display_expr(predicate), display_expr(inner_pred)));
                *predicate = combined;
                *input = inner_input.clone();
            }
        }
        L::Limit { input, .. } => { merge_consecutive_filters(input); }
        L::Projection { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } => merge_consecutive_filters(input),
        L::Join { left, right, .. } => { merge_consecutive_filters(left); merge_consecutive_filters(right); }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
    }
}

fn display_expr(e: &super::logical::Expr) -> String { match e { super::logical::Expr::Raw(s) => s.clone(), _ => format!("{:?}", e) } }

// Remove Projection that is identity (all columns *) directly above another Projection
fn remove_redundant_projection(plan: &mut LogicalQueryPlan) {
    use super::logical::{LogicalQueryPlan as L, Expr};
    match plan {
        L::Projection { exprs, input } => {
            remove_redundant_projection(input);
            if exprs.len()==1 && matches!(exprs[0], Expr::Star) && matches!(&**input, L::Projection { .. }) {
                if let L::Projection { exprs: inner_exprs, input: inner_input } = &**input {
                    *exprs = inner_exprs.clone();
                    *input = inner_input.clone();
                }
            }
        }
        L::Filter { input, .. } | L::Sort { input, .. } | L::Limit { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } => remove_redundant_projection(input),
        L::Join { left, right, .. } => { remove_redundant_projection(left); remove_redundant_projection(right); }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
    }
}

// Attempt: push outer LIMIT into SubqueryScan if subquery lacks its own LIMIT and offset=0
fn try_pushdown_limit_into_subquery(plan: &mut LogicalQueryPlan) {
    use super::logical::LogicalQueryPlan as L;
    match plan {
        L::Limit { limit, offset, input } if *offset==0 => {
            if let L::Projection { input: inner2, .. } | L::Distinct { input: inner2 } | L::Sort { input: inner2, .. } = &mut **input {
                // Recurse first
                try_pushdown_limit_into_subquery(inner2);
            }
            if let L::SubqueryScan { sql, alias } = &mut **input {
                if !sql.to_ascii_lowercase().contains(" limit ") {
                    sql.push_str(&format!(" LIMIT {}", limit));
                }
            }
        }
        L::Limit { input, .. } => { try_pushdown_limit_into_subquery(input); }
        L::Projection { input, .. } | L::Filter { input, .. } | L::Sort { input, .. } | L::Distinct { input } | L::Group { input, .. } | L::Having { input, .. } => try_pushdown_limit_into_subquery(input),
        L::Join { left, right, .. } => { try_pushdown_limit_into_subquery(left); try_pushdown_limit_into_subquery(right); }
        L::TableScan { .. } | L::SubqueryScan { .. } => {}
    }
}
