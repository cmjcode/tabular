use super::{
    errors::QueryAstError,
    logical::{Expr, LogicalQueryPlan, SortItem},
};
use crate::models::enums::DatabaseType;

pub mod dialect;
use dialect::{SqlDialect, get_dialect};

pub fn emit_sql(plan: &LogicalQueryPlan, db_type: &DatabaseType) -> Result<String, QueryAstError> {
    // If top-level is With and still has CTEs, emit a proper WITH clause wrapping emitted SELECT.
    if let LogicalQueryPlan::With { ctes, input } = plan
        && !ctes.is_empty()
    {
        let mut parts = Vec::new();
        for (name, sql) in ctes {
            // Ensure subquery body does not end with semicolon
            let body = sql.trim().trim_end_matches(';');
            parts.push(format!("{} AS ({})", name, body));
        }
        let rendered_inner = emit_sql(input, db_type)?; // recursive (will flatten below)
        return Ok(format!("WITH {} {}", parts.join(", "), rendered_inner));
    }
    // If top-level is a SetOp, emit recursively (each side may itself contain WITH already handled above)
    if let LogicalQueryPlan::SetOp { left, right, op } = plan {
        let left_sql = emit_sql(left, db_type)?;
        let right_sql = emit_sql(right, db_type)?;
        let op_str = match op {
            super::logical::SetOpKind::Union => "UNION",
            super::logical::SetOpKind::UnionAll => "UNION ALL",
        };
        return Ok(format!("({}) {} ({})", left_sql, op_str, right_sql));
    }
    let flat = flatten_plan(plan);
    let dialect = get_dialect(db_type);
    let mut emitter = FlatEmitter { dialect };
    emitter.emit(&flat)
}

#[derive(Debug, Default, Clone)]
struct FlatSelect {
    table: Option<String>,
    subquery: Option<(String, String)>, // (sql, alias)
    projection: Vec<Expr>,
    predicates: Vec<Expr>,
    sort: Vec<SortItem>,
    limit: Option<u64>,
    offset: Option<u64>,
    distinct: bool,
    group_exprs: Vec<Expr>,
    join: Option<(super::logical::JoinKind, String, Option<Expr>)>, // (kind, right_table, on expr)
    having: Option<Expr>,
}

fn flatten_plan(plan: &LogicalQueryPlan) -> FlatSelect {
    fn rec(node: &LogicalQueryPlan, acc: &mut FlatSelect) {
        match node {
            LogicalQueryPlan::TableScan { table, alias } => {
                acc.table = Some(match alias {
                    Some(a) => format!("{} {}", table, a),
                    None => table.clone(),
                });
            }
            LogicalQueryPlan::SubqueryScan { sql, alias, .. } => {
                acc.subquery = Some((sql.clone(), alias.clone()));
            }
            LogicalQueryPlan::Projection { exprs, input } => {
                acc.projection = exprs.clone();
                rec(input, acc);
            }
            LogicalQueryPlan::Distinct { input } => {
                acc.distinct = true;
                rec(input, acc);
            }
            LogicalQueryPlan::Filter { predicate, input } => {
                acc.predicates.push(predicate.clone());
                rec(input, acc);
            }
            LogicalQueryPlan::Sort { items, input } => {
                acc.sort = items.clone();
                rec(input, acc);
            }
            LogicalQueryPlan::Limit {
                limit,
                offset,
                input,
            } => {
                acc.limit = Some(*limit);
                acc.offset = Some(*offset);
                rec(input, acc);
            }
            LogicalQueryPlan::Group { group_exprs, input } => {
                acc.group_exprs = group_exprs.clone();
                rec(input, acc);
            }
            LogicalQueryPlan::Join {
                left,
                right,
                on,
                kind,
            } => {
                // assume left eventually becomes main table, right is simple table scan
                // Extract right table name if direct TableScan
                let right_table = match &**right {
                    LogicalQueryPlan::TableScan { table, alias } => match alias {
                        Some(a) => format!("{} {}", table, a),
                        None => table.clone(),
                    },
                    _ => "sub".into(),
                };
                acc.join = Some((*kind, right_table, on.clone()));
                rec(left, acc);
            }
            LogicalQueryPlan::Having { predicate, input } => {
                acc.having = Some(predicate.clone());
                rec(input, acc);
            }
            LogicalQueryPlan::With { input, .. } => {
                rec(input, acc);
            }
            LogicalQueryPlan::SetOp { .. } => { /* SetOp cannot be flattened into single SELECT; higher emit handles it */
            }
        }
    }
    let mut flat = FlatSelect::default();
    rec(plan, &mut flat);
    flat
}

struct FlatEmitter {
    dialect: Box<dyn SqlDialect>,
}

impl FlatEmitter {
    fn emit(&mut self, flat: &FlatSelect) -> Result<String, QueryAstError> {
        let proj_sql = if flat.projection.is_empty() {
            "*".to_string()
        } else {
            flat.projection
                .iter()
                .map(|e| self.emit_expr(e))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        };
        let from_clause = if let Some((sub_sql, alias)) = &flat.subquery {
            format!("({}) {}", sub_sql, self.quote_table(alias))
        } else {
            self.quote_table(&flat.table.clone().unwrap_or_else(|| "DUAL".to_string()))
        };
        let mut sql = if flat.distinct {
            format!(
                "SELECT {} {} FROM {}",
                self.dialect.emit_distinct(),
                proj_sql,
                from_clause
            )
        } else {
            format!("SELECT {} FROM {}", proj_sql, from_clause)
        };
        if let Some((kind, right_table, on)) = &flat.join {
            let join_kw = self.dialect.emit_join_kind(kind);
            sql.push_str(&format!(" {} {}", join_kw, self.quote_table(right_table)));
            if let Some(on_expr) = on {
                sql.push_str(&format!(" ON {}", self.emit_expr(on_expr)?));
            }
        }
        if !flat.predicates.is_empty() {
            let where_clause = flat
                .predicates
                .iter()
                .map(|p| self.emit_expr(p))
                .collect::<Result<Vec<_>, _>>()?
                .join(" AND ");
            sql.push_str(&format!(" WHERE {}", where_clause));
        }
        if !flat.group_exprs.is_empty() {
            let grp = flat
                .group_exprs
                .iter()
                .map(|g| self.emit_expr(g))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            sql.push_str(&format!(" GROUP BY {}", grp));
        }
        if let Some(h) = &flat.having {
            sql.push_str(&format!(" HAVING {}", self.emit_expr(h)?));
        }
        if !flat.sort.is_empty() {
            let order = flat
                .sort
                .iter()
                .map(|s| {
                    format!(
                        "{} {}",
                        self.emit_expr(&s.expr).unwrap_or_else(|_| "?".into()),
                        if s.asc { "ASC" } else { "DESC" }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!(" ORDER BY {}", order));
        }

        // Use dialect-specific LIMIT emission
        if let Some(l) = flat.limit {
            let offset = flat.offset.unwrap_or(0);
            let limit_clause = self.dialect.emit_limit(l, offset);

            // Special handling for MS SQL TOP (needs to be injected after SELECT)
            if self.dialect.db_type() == DatabaseType::MsSQL
                && offset == 0
                && !limit_clause.is_empty()
            {
                // Already handled by SELECT TOP injection in dialect
            } else if self.dialect.db_type() == DatabaseType::MsSQL && offset == 0 {
                // Inject TOP for MS SQL when no offset
                if sql.to_uppercase().starts_with("SELECT ") {
                    sql = sql.replacen("SELECT ", &format!("SELECT TOP {} ", l), 1);
                }
            } else {
                sql.push_str(&limit_clause);
            }
        }
        Ok(sql)
    }

    fn emit_expr(&mut self, expr: &Expr) -> Result<String, QueryAstError> {
        Ok(match expr {
            Expr::Column(c) => self.emit_column(c),
            Expr::StringLiteral(s) => self.dialect.quote_string(s),
            Expr::Number(n) => n.clone(),
            Expr::BinaryOp { left, op, right } => format!(
                "{} {} {}",
                self.emit_expr(left)?,
                op,
                self.emit_expr(right)?
            ),
            Expr::FuncCall { name, args } => {
                let args_sql = args
                    .iter()
                    .map(|a| self.emit_expr(a))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                format!("{}({})", name, args_sql)
            }
            Expr::Star => "*".into(),
            Expr::Alias { expr, alias } => {
                format!("{} AS {}", self.emit_expr(expr)?, self.quote_ident(alias))
            }
            Expr::Raw(r) => r.clone(),
            Expr::Null => self.dialect.emit_null(),
            Expr::Boolean(b) => self.dialect.emit_boolean(*b),
            Expr::Not(inner) => format!("NOT {}", self.emit_expr(inner)?),
            Expr::IsNull { expr, negated } => {
                if *negated {
                    format!("{} IS NOT NULL", self.emit_expr(expr)?)
                } else {
                    format!("{} IS NULL", self.emit_expr(expr)?)
                }
            }
            Expr::Like {
                expr,
                pattern,
                negated,
            } => {
                if *negated {
                    format!(
                        "{} NOT LIKE {}",
                        self.emit_expr(expr)?,
                        self.emit_expr(pattern)?
                    )
                } else {
                    format!(
                        "{} LIKE {}",
                        self.emit_expr(expr)?,
                        self.emit_expr(pattern)?
                    )
                }
            }
            Expr::InList {
                expr,
                list,
                negated,
            } => {
                let items = list
                    .iter()
                    .map(|e| self.emit_expr(e))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                if *negated {
                    format!("{} NOT IN ({})", self.emit_expr(expr)?, items)
                } else {
                    format!("{} IN ({})", self.emit_expr(expr)?, items)
                }
            }
            Expr::Case {
                operand,
                when_then,
                else_expr,
            } => {
                let mut s = String::from("CASE");
                if let Some(op) = operand {
                    s.push(' ');
                    s.push_str(&self.emit_expr(op)?);
                }
                for (w, t) in when_then {
                    s.push_str(&format!(
                        " WHEN {} THEN {}",
                        self.emit_expr(w)?,
                        self.emit_expr(t)?
                    ));
                }
                if let Some(e2) = else_expr {
                    s.push_str(&format!(" ELSE {}", self.emit_expr(e2)?));
                }
                s.push_str(" END");
                s
            }
            Expr::Subquery { sql, .. } => format!("({})", sql),
            Expr::WindowFunc {
                name,
                args,
                partition_by,
                order_by,
                frame,
            } => {
                let args_sql = args
                    .iter()
                    .map(|a| self.emit_expr(a))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ");
                let mut s = format!("{}({}) OVER (", name, args_sql);
                if !partition_by.is_empty() {
                    s.push_str("PARTITION BY ");
                    s.push_str(
                        &partition_by
                            .iter()
                            .map(|e| self.emit_expr(e))
                            .collect::<Result<Vec<_>, _>>()?
                            .join(", "),
                    );
                }
                if !order_by.is_empty() {
                    if !partition_by.is_empty() {
                        s.push(' ');
                    }
                    s.push_str("ORDER BY ");
                    s.push_str(
                        &order_by
                            .iter()
                            .map(|(e, asc)| {
                                format!(
                                    "{} {}",
                                    self.emit_expr(e).unwrap_or_else(|_| "?".into()),
                                    if *asc { "ASC" } else { "DESC" }
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                if let Some(f) = frame {
                    if !partition_by.is_empty() || !order_by.is_empty() {
                        s.push(' ');
                    }
                    s.push_str(f);
                }
                s.push(')');
                s
            }
        })
    }

    fn emit_column(&self, col: &str) -> String {
        if col.contains('.') {
            col.split('.')
                .map(|p| self.quote_ident(p))
                .collect::<Vec<_>>()
                .join(".")
        } else {
            self.quote_ident(col)
        }
    }

    fn quote_table(&self, t: &str) -> String {
        self.emit_column(t)
    }

    fn quote_ident(&self, ident: &str) -> String {
        self.dialect.quote_ident(ident)
    }
}
