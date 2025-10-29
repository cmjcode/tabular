use super::{
    errors::QueryAstError,
    logical::{Expr, JoinKind, LogicalQueryPlan, SetOpKind, SortItem},
};
use sqlparser::ast as sq;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::collections::HashSet;

pub fn parse_single_select_to_plan(sql: &str) -> Result<LogicalQueryPlan, QueryAstError> {
    let dialect = GenericDialect {};
    let ast = Parser::parse_sql(&dialect, sql).map_err(|e| QueryAstError::Parse(e.to_string()))?;
    if ast.len() != 1 {
        return Err(QueryAstError::Unsupported("multi-statement"));
    }
    match &ast[0] {
        sq::Statement::Query(q) => convert_query(q, sql),
        _ => Err(QueryAstError::Unsupported("not a SELECT")),
    }
}

fn convert_query(q: &sq::Query, raw_sql: &str) -> Result<LogicalQueryPlan, QueryAstError> {
    let mut plan = convert_set_expr(&q.body, q, raw_sql)?;
    if let Some(with) = &q.with
        && !with.cte_tables.is_empty()
    {
        let mut ctes = Vec::new();
        for c in &with.cte_tables {
            ctes.push((c.alias.name.value.clone(), c.query.to_string()));
        }
        plan = LogicalQueryPlan::With {
            ctes,
            input: Box::new(plan),
        };
    }
    Ok(plan)
}

fn convert_set_expr(
    se: &sq::SetExpr,
    outer_q: &sq::Query,
    raw_sql: &str,
) -> Result<LogicalQueryPlan, QueryAstError> {
    match se {
        sq::SetExpr::Select(sel) => convert_select(sel, outer_q, raw_sql),
        // Newer sqlparser versions expose set ops via SetOperation { op, left, right, set_quantifier, .. }
        sq::SetExpr::SetOperation {
            op,
            left,
            right,
            set_quantifier,
            ..
        } => {
            let left_plan = convert_set_expr(left, outer_q, raw_sql)?;
            let right_plan = convert_set_expr(right, outer_q, raw_sql)?;
            use sq::SetOperator;
            let op_kind = match op {
                SetOperator::Union => {
                    if matches!(set_quantifier, sq::SetQuantifier::All) {
                        SetOpKind::UnionAll
                    } else {
                        SetOpKind::Union
                    }
                }
                _ => return Err(QueryAstError::Unsupported("only UNION supported")),
            };
            Ok(LogicalQueryPlan::SetOp {
                left: Box::new(left_plan),
                right: Box::new(right_plan),
                op: op_kind,
            })
        }
        // Some variants (Query) wrap another Query (e.g. parentheses); unwrap recursively
        sq::SetExpr::Query(q2) => convert_set_expr(&q2.body, q2, raw_sql),
        _ => Err(QueryAstError::Unsupported("unsupported set expr variant")),
    }
}

fn convert_select(
    sel: &sq::Select,
    q: &sq::Query,
    raw_sql: &str,
) -> Result<LogicalQueryPlan, QueryAstError> {
    if sel.from.is_empty() {
        return Err(QueryAstError::Unsupported("missing FROM"));
    }

    // FROM + JOIN chain
    let base = match &sel.from[0].relation {
        sq::TableFactor::Table { name, alias, .. } => {
            let mut scan = LogicalQueryPlan::table_scan(name.to_string());
            if let LogicalQueryPlan::TableScan { alias: a, .. } = &mut scan
                && let Some(a2) = alias
            {
                *a = Some(a2.name.value.clone());
            }
            scan
        }
        sq::TableFactor::Derived {
            subquery, alias, ..
        } => {
            let al = alias
                .as_ref()
                .map(|a| a.name.to_string())
                .unwrap_or_else(|| "subq".into());
            LogicalQueryPlan::subquery_scan(subquery.to_string(), al)
        }
        _ => return Err(QueryAstError::Unsupported("complex table ref")),
    };
    let mut plan = base;
    for join in &sel.from[0].joins {
        let right = match &join.relation {
            sq::TableFactor::Table { name, alias, .. } => {
                let mut scan = LogicalQueryPlan::table_scan(name.to_string());
                if let LogicalQueryPlan::TableScan { alias: a, .. } = &mut scan
                    && let Some(a2) = alias
                {
                    *a = Some(a2.name.value.clone());
                }
                scan
            }
            sq::TableFactor::Derived {
                subquery, alias, ..
            } => {
                let al = alias
                    .as_ref()
                    .map(|a| a.name.to_string())
                    .unwrap_or_else(|| "subq".into());
                LogicalQueryPlan::subquery_scan(subquery.to_string(), al)
            }
            _ => return Err(QueryAstError::Unsupported("complex join rel")),
        };
        let kind = match join.join_operator {
            sq::JoinOperator::Inner(_) => JoinKind::Inner,
            sq::JoinOperator::LeftOuter(_) => JoinKind::Left,
            sq::JoinOperator::RightOuter(_) => JoinKind::Right,
            sq::JoinOperator::FullOuter(_) => JoinKind::Full,
            _ => JoinKind::Inner,
        };
        let on_expr = match &join.join_operator {
            sq::JoinOperator::Inner(cond)
            | sq::JoinOperator::LeftOuter(cond)
            | sq::JoinOperator::RightOuter(cond)
            | sq::JoinOperator::FullOuter(cond) => match cond {
                sq::JoinConstraint::On(e) => Some(convert_expr(e)),
                _ => None,
            },
            _ => None,
        };
        plan = LogicalQueryPlan::Join {
            left: Box::new(plan),
            right: Box::new(right),
            on: on_expr,
            kind,
        };
    }

    // WHERE
    if let Some(pred) = &sel.selection {
        plan = LogicalQueryPlan::Filter {
            predicate: convert_expr(pred),
            input: Box::new(plan),
        };
    }

    // GROUP BY (primary attempt via AST)
    use sqlparser::ast::GroupByExpr;
    let mut group_added = false;
    if let GroupByExpr::Expressions(_mod, list) = &sel.group_by
        && !list.is_empty()
    {
        let mut gexprs = Vec::new();
        for item in list {
            let s = item.to_string();
            // Heuristic: simple identifier / dotted path -> Column; otherwise Raw
            if s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                && s.contains(|c: char| c.is_ascii_alphabetic())
            {
                gexprs.push(Expr::Column(s));
            } else {
                gexprs.push(Expr::Raw(s));
            }
        }
        plan = LogicalQueryPlan::Group {
            group_exprs: gexprs,
            input: Box::new(plan),
        };
        group_added = true;
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
                if let Some(idx) = tail.find(kw)
                    && idx < end_rel
                {
                    end_rel = idx;
                }
            }
            let original_slice = &raw_sql[start..start + end_rel];
            let candidates: Vec<Expr> = original_slice
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| Expr::Raw(s.to_string()))
                .collect();
            if !candidates.is_empty() {
                plan = LogicalQueryPlan::Group {
                    group_exprs: candidates,
                    input: Box::new(plan),
                };
            }
        }
    }

    // HAVING
    if let Some(having) = &sel.having {
        plan = LogicalQueryPlan::Having {
            predicate: convert_expr(having),
            input: Box::new(plan),
        };
    }

    // PROJECTION
    let mut exprs = Vec::new();
    for item in &sel.projection {
        match item {
            sq::SelectItem::Wildcard(_) => exprs.push(Expr::Star),
            sq::SelectItem::UnnamedExpr(e) => {
                let ce = convert_expr(e);
                if ce.is_simple_aggregate() {
                    let alias = match &ce {
                        Expr::FuncCall { name, args } => {
                            if args.is_empty() {
                                name.to_ascii_lowercase()
                            } else {
                                format!("{}_col", name.to_ascii_lowercase())
                            }
                        }
                        _ => "agg".into(),
                    };
                    exprs.push(Expr::Alias {
                        expr: Box::new(ce),
                        alias,
                    });
                } else {
                    exprs.push(ce);
                }
            }
            sq::SelectItem::ExprWithAlias { expr, alias } => exprs.push(Expr::Alias {
                expr: Box::new(convert_expr(expr)),
                alias: alias.to_string(),
            }),
            _ => return Err(QueryAstError::Unsupported("complex projection")),
        }
    }
    // Second pass aggregate aliasing
    let mut adjusted = Vec::with_capacity(exprs.len());
    for e in exprs.into_iter() {
        match &e {
            Expr::FuncCall { name, .. } => {
                let lname = name.to_ascii_lowercase();
                if matches!(lname.as_str(), "count" | "sum" | "avg" | "min" | "max") {
                    adjusted.push(Expr::Alias {
                        expr: Box::new(e),
                        alias: format!("{}_col", lname),
                    });
                } else {
                    adjusted.push(e);
                }
            }
            _ => adjusted.push(e),
        }
    }
    plan = LogicalQueryPlan::Projection {
        exprs: adjusted,
        input: Box::new(plan),
    };

    // DISTINCT
    if sel.distinct.is_some() {
        plan = LogicalQueryPlan::Distinct {
            input: Box::new(plan),
        };
    }

    // ORDER BY
    if let Some(ob) = &q.order_by
        && !ob.exprs.is_empty()
    {
        let mut items = Vec::new();
        for obe in &ob.exprs {
            items.push(SortItem {
                expr: convert_expr(&obe.expr),
                asc: obe.asc.unwrap_or(true),
            });
        }
        plan = LogicalQueryPlan::Sort {
            items,
            input: Box::new(plan),
        };
    }

    // LIMIT/OFFSET
    let (limit, offset) = extract_limit_offset(q)?;
    if let Some(l) = limit {
        plan = LogicalQueryPlan::Limit {
            limit: l,
            offset: offset.unwrap_or(0),
            input: Box::new(plan),
        };
    }

    // Final accurate correlation marking: traverse expressions finding subquery columns referencing outer aliases.
    let mut outer_aliases = HashSet::new();
    collect_table_aliases(&plan, &mut outer_aliases);
    mark_correlated(&mut plan, &outer_aliases);
    Ok(plan)
}

fn extract_limit_offset(q: &sq::Query) -> Result<(Option<u64>, Option<u64>), QueryAstError> {
    let limit = if let Some(l) = &q.limit {
        match l {
            sq::Expr::Value(sq::Value::Number(n, _)) => n.parse().ok(),
            _ => None,
        }
    } else {
        None
    };
    let offset = if let Some(o) = &q.offset {
        match &o.value {
            sq::Expr::Value(sq::Value::Number(n, _)) => n.parse().ok(),
            _ => None,
        }
    } else {
        None
    };
    Ok((limit, offset))
}

fn convert_expr(e: &sq::Expr) -> Expr {
    match e {
        sq::Expr::Subquery(sub) => Expr::Subquery {
            sql: sub.to_string(),
            correlated: false,
        },
        sq::Expr::Identifier(id) => Expr::Column(id.to_string()),
        sq::Expr::CompoundIdentifier(parts) => Expr::Column(
            parts
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join("."),
        ),
        sq::Expr::Value(sq::Value::Number(n, _)) => Expr::Number(n.clone()),
        sq::Expr::Value(sq::Value::SingleQuotedString(s)) => Expr::StringLiteral(s.clone()),
        sq::Expr::Value(sq::Value::Boolean(b)) => Expr::Boolean(*b),
        sq::Expr::Value(sq::Value::Null) => Expr::Null,
        sq::Expr::BinaryOp { left, op, right } => Expr::BinaryOp {
            left: Box::new(convert_expr(left)),
            op: op.to_string(),
            right: Box::new(convert_expr(right)),
        },
        sq::Expr::Function(func) => {
            let name = func.name.to_string();
            let mut out_args = Vec::new();
            use sqlparser::ast::FunctionArguments;
            match &func.args {
                FunctionArguments::None => {}
                FunctionArguments::List(list) => {
                    for a in &list.args {
                        match a {
                            sq::FunctionArg::Unnamed(sq::FunctionArgExpr::Expr(ex)) => {
                                out_args.push(convert_expr(ex))
                            }
                            sq::FunctionArg::Unnamed(sq::FunctionArgExpr::Wildcard) => {
                                out_args.push(Expr::Star)
                            }
                            _ => return Expr::Raw(e.to_string()),
                        }
                    }
                }
                other => {
                    return Expr::Raw(other.to_string());
                }
            }
            if let Some(sq::WindowType::WindowSpec(spec)) = &func.over {
                // proper window spec
                let partition_by = spec
                    .partition_by
                    .iter()
                    .map(convert_expr)
                    .collect::<Vec<_>>();
                let order_by = spec
                    .order_by
                    .iter()
                    .map(|obe| (convert_expr(&obe.expr), obe.asc.unwrap_or(true)))
                    .collect::<Vec<_>>();
                let frame = spec.window_frame.as_ref().map(|wf| format!("{:?}", wf));
                return Expr::WindowFunc {
                    name,
                    args: out_args,
                    partition_by,
                    order_by,
                    frame,
                };
            }
            Expr::FuncCall {
                name,
                args: out_args,
            }
        }
        sq::Expr::IsNull(inner) => Expr::IsNull {
            expr: Box::new(convert_expr(inner)),
            negated: false,
        },
        sq::Expr::IsNotNull(inner) => Expr::IsNull {
            expr: Box::new(convert_expr(inner)),
            negated: true,
        },
        sq::Expr::UnaryOp { op, expr } => {
            let op_str = op.to_string().to_uppercase();
            if op_str == "NOT" {
                Expr::Not(Box::new(convert_expr(expr)))
            } else {
                Expr::Raw(e.to_string())
            }
        }
        sq::Expr::Like {
            expr,
            pattern,
            negated,
            ..
        } => Expr::Like {
            expr: Box::new(convert_expr(expr)),
            pattern: Box::new(convert_expr(pattern)),
            negated: *negated,
        },
        sq::Expr::ILike {
            expr,
            pattern,
            negated,
            ..
        } => Expr::Like {
            expr: Box::new(convert_expr(expr)),
            pattern: Box::new(convert_expr(pattern)),
            negated: *negated,
        },
        sq::Expr::InList {
            expr,
            list,
            negated,
        } => {
            let l = list.iter().map(convert_expr).collect();
            Expr::InList {
                expr: Box::new(convert_expr(expr)),
                list: l,
                negated: *negated,
            }
        }
        sq::Expr::Case {
            operand,
            conditions,
            results,
            else_result,
        } => {
            let op = operand.as_ref().map(|o| Box::new(convert_expr(o)));
            let mut when_then = Vec::new();
            for (c, r) in conditions.iter().zip(results.iter()) {
                when_then.push((convert_expr(c), convert_expr(r)));
            }
            let else_expr = else_result.as_ref().map(|e2| Box::new(convert_expr(e2)));
            Expr::Case {
                operand: op,
                when_then,
                else_expr,
            }
        }
        sq::Expr::Nested(inner) => Expr::Raw(inner.to_string()),
        _ => Expr::Raw(e.to_string()),
    }
}

fn collect_table_aliases(plan: &LogicalQueryPlan, out: &mut HashSet<String>) {
    match plan {
        LogicalQueryPlan::TableScan { table, alias } => {
            if let Some(a) = alias {
                out.insert(a.to_ascii_lowercase());
            } else {
                out.insert(
                    table
                        .split('.')
                        .next_back()
                        .unwrap_or(table)
                        .to_ascii_lowercase(),
                );
            }
        }
        LogicalQueryPlan::SubqueryScan { alias, .. } => {
            out.insert(alias.to_ascii_lowercase());
        }
        LogicalQueryPlan::Projection { input, .. }
        | LogicalQueryPlan::Filter { input, .. }
        | LogicalQueryPlan::Sort { input, .. }
        | LogicalQueryPlan::Limit { input, .. }
        | LogicalQueryPlan::Distinct { input }
        | LogicalQueryPlan::Group { input, .. }
        | LogicalQueryPlan::Having { input, .. }
        | LogicalQueryPlan::With { input, .. } => collect_table_aliases(input, out),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => {
            collect_table_aliases(left, out);
            collect_table_aliases(right, out);
        }
    }
}

fn mark_correlated(plan: &mut LogicalQueryPlan, outer: &HashSet<String>) {
    match plan {
        LogicalQueryPlan::SubqueryScan {
            sql, correlated, ..
        } => {
            if !*correlated {
                let lower = sql.to_ascii_lowercase();
                if outer.iter().any(|a| lower.contains(&format!("{}.", a))) {
                    *correlated = true;
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
        | LogicalQueryPlan::With { input, .. } => mark_correlated(input, outer),
        LogicalQueryPlan::Join { left, right, .. }
        | LogicalQueryPlan::SetOp { left, right, .. } => {
            mark_correlated(left, outer);
            mark_correlated(right, outer);
        }
        LogicalQueryPlan::TableScan { .. } => {}
    }
}
