use super::{logical::{LogicalQueryPlan, Expr, SortItem, JoinKind}, errors::QueryAstError};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::ast as sq;

pub fn parse_single_select_to_plan(sql: &str) -> Result<LogicalQueryPlan, QueryAstError> {
    let dialect = GenericDialect {}; // Later choose based on DatabaseType
    let ast = Parser::parse_sql(&dialect, sql).map_err(|e| QueryAstError::Parse(e.to_string()))?;
    if ast.len() != 1 { return Err(QueryAstError::Unsupported("multi-statement")); }
    match &ast[0] {
        sq::Statement::Query(q) => convert_query(q, sql),
        _ => Err(QueryAstError::Unsupported("not a SELECT")),
    }
}

fn convert_query(q: &sq::Query, raw_sql: &str) -> Result<LogicalQueryPlan, QueryAstError> {
    let body = &q.body;
    match body.as_ref() {
        sq::SetExpr::Select(sel) => convert_select(sel, q, raw_sql),
        _ => Err(QueryAstError::Unsupported("unsupported set expr")),
    }
}

fn convert_select(sel: &sq::Select, q: &sq::Query, raw_sql: &str) -> Result<LogicalQueryPlan, QueryAstError> {
    if sel.from.is_empty() { return Err(QueryAstError::Unsupported("missing FROM")); }

    // FROM + JOIN chain
    let base = match &sel.from[0].relation {
        sq::TableFactor::Table { name, .. } => LogicalQueryPlan::table_scan(name.to_string()),
        sq::TableFactor::Derived { subquery, alias, .. } => {
            let al = alias.as_ref().map(|a| a.name.to_string()).unwrap_or_else(|| "subq".into());
            LogicalQueryPlan::subquery_scan(subquery.to_string(), al)
        }
        _ => return Err(QueryAstError::Unsupported("complex table ref")),
    };
    let mut plan = base;
    for join in &sel.from[0].joins {
        let right = match &join.relation {
            sq::TableFactor::Table { name, .. } => LogicalQueryPlan::table_scan(name.to_string()),
            sq::TableFactor::Derived { subquery, alias, .. } => {
                let al = alias.as_ref().map(|a| a.name.to_string()).unwrap_or_else(|| "subq".into());
                LogicalQueryPlan::subquery_scan(subquery.to_string(), al)
            }
            _ => return Err(QueryAstError::Unsupported("complex join rel")),
        };
        let kind = match join.join_operator { sq::JoinOperator::Inner(_) => JoinKind::Inner, sq::JoinOperator::LeftOuter(_) => JoinKind::Left, sq::JoinOperator::RightOuter(_) => JoinKind::Right, sq::JoinOperator::FullOuter(_) => JoinKind::Full, _ => JoinKind::Inner };
        let on_expr = match &join.join_operator { sq::JoinOperator::Inner(cond) | sq::JoinOperator::LeftOuter(cond) | sq::JoinOperator::RightOuter(cond) | sq::JoinOperator::FullOuter(cond) => match cond { sq::JoinConstraint::On(e) => Some(convert_expr(e)), _ => None }, _ => None };
        plan = LogicalQueryPlan::Join { left: Box::new(plan), right: Box::new(right), on: on_expr, kind };
    }

    // WHERE
    if let Some(pred) = &sel.selection { plan = LogicalQueryPlan::Filter { predicate: convert_expr(pred), input: Box::new(plan) }; }

    // GROUP BY (primary attempt via AST)
    use sqlparser::ast::GroupByExpr;
    let mut group_added = false;
    if let GroupByExpr::Expressions(_mod, list) = &sel.group_by {
        if !list.is_empty() {
            let mut gexprs = Vec::new();
            for item in list {
                let s = item.to_string();
                // Heuristic: simple identifier / dotted path -> Column; otherwise Raw
                if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c=='.') && s.contains(|c: char| c.is_ascii_alphabetic()) {
                    gexprs.push(Expr::Column(s));
                } else {
                    gexprs.push(Expr::Raw(s));
                }
            }
            plan = LogicalQueryPlan::Group { group_exprs: gexprs, input: Box::new(plan) };
            group_added = true;
        }
    }
    // Fallback heuristic: if parser gave us no group node but raw SQL contains GROUP BY
    if !group_added {
        let lower = raw_sql.to_ascii_lowercase();
        if let Some(gpos) = lower.find("group by ") {
            // slice after 'group by '
            let start = gpos + "group by ".len();
            // find earliest of having/order/limit/end
            let tail = &lower[start..];
            let mut end_rel = tail.len();
            for kw in [" having ", " order by ", " limit ", ";"] {
                if let Some(idx) = tail.find(kw) { if idx < end_rel { end_rel = idx; } }
            }
            let original_slice = &raw_sql[start..start+end_rel];
            let candidates: Vec<Expr> = original_slice.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| Expr::Raw(s.to_string()))
                .collect();
            if !candidates.is_empty() {
                plan = LogicalQueryPlan::Group { group_exprs: candidates, input: Box::new(plan) };
            }
        }
    }

    // HAVING
    if let Some(having) = &sel.having { plan = LogicalQueryPlan::Having { predicate: convert_expr(having), input: Box::new(plan) }; }

    // PROJECTION
    let mut exprs = Vec::new();
    for item in &sel.projection {
        match item {
            sq::SelectItem::Wildcard(_) => exprs.push(Expr::Star),
            sq::SelectItem::UnnamedExpr(e) => {
                let ce = convert_expr(e);
                if ce.is_simple_aggregate() {
                    let alias = match &ce { Expr::FuncCall { name, args } => { if args.is_empty() { name.to_ascii_lowercase() } else { format!("{}_col", name.to_ascii_lowercase()) } } _ => "agg".into() };
                    exprs.push(Expr::Alias { expr: Box::new(ce), alias });
                } else { exprs.push(ce); }
            }
            sq::SelectItem::ExprWithAlias { expr, alias } => exprs.push(Expr::Alias { expr: Box::new(convert_expr(expr)), alias: alias.to_string() }),
            _ => return Err(QueryAstError::Unsupported("complex projection")),
        }
    }
    // Second pass aggregate aliasing
    let mut adjusted = Vec::with_capacity(exprs.len());
    for e in exprs.into_iter() {
        match &e { Expr::FuncCall { name, .. } => { let lname = name.to_ascii_lowercase(); if matches!(lname.as_str(), "count" | "sum" | "avg" | "min" | "max") { adjusted.push(Expr::Alias { expr: Box::new(e), alias: format!("{}_col", lname) }); } else { adjusted.push(e); } } _ => adjusted.push(e) }
    }
    plan = LogicalQueryPlan::Projection { exprs: adjusted, input: Box::new(plan) };

    // DISTINCT
    if sel.distinct.is_some() { plan = LogicalQueryPlan::Distinct { input: Box::new(plan) }; }

    // ORDER BY
    if let Some(ob) = &q.order_by { if !ob.exprs.is_empty() { let mut items = Vec::new(); for obe in &ob.exprs { items.push(SortItem { expr: convert_expr(&obe.expr), asc: obe.asc.unwrap_or(true) }); } plan = LogicalQueryPlan::Sort { items, input: Box::new(plan) }; } }

    // LIMIT/OFFSET
    let (limit, offset) = extract_limit_offset(q)?;
    if let Some(l) = limit { plan = LogicalQueryPlan::Limit { limit: l, offset: offset.unwrap_or(0), input: Box::new(plan) }; }

    Ok(plan)
}

fn extract_limit_offset(q: &sq::Query) -> Result<(Option<u64>, Option<u64>), QueryAstError> {
    let limit = if let Some(l) = &q.limit { match l { sq::Expr::Value(sq::Value::Number(n, _)) => n.parse().ok(), _ => None } } else { None };
    let offset = if let Some(o) = &q.offset { match &o.value { sq::Expr::Value(sq::Value::Number(n, _)) => n.parse().ok(), _ => None } } else { None };
    Ok((limit, offset))
}

fn convert_expr(e: &sq::Expr) -> Expr {
    match e {
    sq::Expr::Subquery(sub) => Expr::Subquery(sub.to_string()),
        sq::Expr::Identifier(id) => Expr::Column(id.to_string()),
        sq::Expr::CompoundIdentifier(parts) => Expr::Column(parts.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".")),
        sq::Expr::Value(sq::Value::Number(n, _)) => Expr::Number(n.clone()),
        sq::Expr::Value(sq::Value::SingleQuotedString(s)) => Expr::StringLiteral(s.clone()),
        sq::Expr::Value(sq::Value::Boolean(b)) => Expr::Boolean(*b),
        sq::Expr::Value(sq::Value::Null) => Expr::Null,
        sq::Expr::BinaryOp { left, op, right } => Expr::BinaryOp { left: Box::new(convert_expr(left)), op: op.to_string(), right: Box::new(convert_expr(right)) },
        sq::Expr::Function(func) => {
            let name = func.name.to_string();
            let mut out_args = Vec::new();
            use sqlparser::ast::FunctionArguments;
            match &func.args {
                FunctionArguments::None => {}
                FunctionArguments::List(list) => {
                    for a in &list.args {
                        match a {
                            sq::FunctionArg::Unnamed(sq::FunctionArgExpr::Expr(ex)) => out_args.push(convert_expr(ex)),
                            sq::FunctionArg::Unnamed(sq::FunctionArgExpr::Wildcard) => out_args.push(Expr::Star),
                            _ => return Expr::Raw(e.to_string()),
                        }
                    }
                }
                other => { return Expr::Raw(other.to_string()); }
            }
            Expr::FuncCall { name, args: out_args }
        }
        sq::Expr::IsNull(inner) => Expr::IsNull { expr: Box::new(convert_expr(inner)), negated: false },
        sq::Expr::IsNotNull(inner) => Expr::IsNull { expr: Box::new(convert_expr(inner)), negated: true },
        sq::Expr::UnaryOp { op, expr } => {
            let op_str = op.to_string().to_uppercase();
            if op_str == "NOT" { Expr::Not(Box::new(convert_expr(expr))) } else { Expr::Raw(e.to_string()) }
        }
        sq::Expr::Like { expr, pattern, negated, .. } => Expr::Like { expr: Box::new(convert_expr(expr)), pattern: Box::new(convert_expr(pattern)), negated: *negated },
        sq::Expr::ILike { expr, pattern, negated, .. } => Expr::Like { expr: Box::new(convert_expr(expr)), pattern: Box::new(convert_expr(pattern)), negated: *negated },
        sq::Expr::InList { expr, list, negated } => {
            let l = list.iter().map(convert_expr).collect();
            Expr::InList { expr: Box::new(convert_expr(expr)), list: l, negated: *negated }
        }
        sq::Expr::Case { operand, conditions, results, else_result } => {
            let op = operand.as_ref().map(|o| Box::new(convert_expr(o)));
            let mut when_then = Vec::new();
            for (c, r) in conditions.iter().zip(results.iter()) { when_then.push((convert_expr(c), convert_expr(r))); }
            let else_expr = else_result.as_ref().map(|e2| Box::new(convert_expr(e2)));
            Expr::Case { operand: op, when_then, else_expr }
        }
        sq::Expr::Nested(inner) => Expr::Raw(inner.to_string()),
        _ => Expr::Raw(e.to_string()),
    }
}
