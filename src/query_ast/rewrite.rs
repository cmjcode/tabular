use super::{logical::{LogicalQueryPlan, Expr}, errors::{RewriteError}};

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
    Ok(())
}

fn has_limit(plan: &LogicalQueryPlan) -> bool { match plan { LogicalQueryPlan::Limit { .. } => true, LogicalQueryPlan::Projection { input, .. } | LogicalQueryPlan::Filter { input, .. } | LogicalQueryPlan::Sort { input, .. } | LogicalQueryPlan::Distinct { input } | LogicalQueryPlan::Group { input, .. } | LogicalQueryPlan::Having { input, .. } => has_limit(input), LogicalQueryPlan::Join { left, right, .. } => has_limit(left) || has_limit(right), LogicalQueryPlan::TableScan { .. } => false } }

fn replace_or_add_limit(plan: &mut LogicalQueryPlan, limit: u64, offset: u64) {
    match plan {
        LogicalQueryPlan::Limit { limit: l, offset: o, .. } => { *l = limit; *o = offset; }
    LogicalQueryPlan::Projection { input, .. } | LogicalQueryPlan::Filter { input, .. } | LogicalQueryPlan::Sort { input, .. } | LogicalQueryPlan::Distinct { input } | LogicalQueryPlan::Group { input, .. } | LogicalQueryPlan::Having { input, .. } => replace_or_add_limit(input, limit, offset),
        LogicalQueryPlan::Join { left, .. } => replace_or_add_limit(left, limit, offset),
        LogicalQueryPlan::TableScan { .. } => { let new = LogicalQueryPlan::Limit { limit, offset, input: Box::new(plan.clone())}; *plan = new; }
    }
}
