//! Pemetaan AST `sqlparser` ke [`QueryDiagramModel`].
//!
//! Referensi kolom di dalam ekspresi dikumpulkan lewat tokenizer (bukan
//! walker AST penuh) supaya tahan terhadap puluhan varian `Expr` dan
//! sintaks khusus dialek. Hanya kondisi kesetaraan `a.x = b.y` yang dibaca
//! dari AST untuk membentuk relasi join.

use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FromTable, GroupByExpr, Insert, JoinConstraint,
    JoinOperator, LimitClause, ObjectName, OrderByKind, Query, Select, SelectItem, SetExpr,
    Statement, TableFactor, TableObject, TableWithJoins, UpdateTableFromKind,
};
use sqlparser::dialect::{
    Dialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::{
    ColumnRef, JoinLink, Mutation, OutputColumn, QueryDiagramError, QueryDiagramModel, SourceKind,
    SourceTable, StatementKind, VALUES_ID, clip,
};
use crate::models::enums::DatabaseType;

/// Kata kunci yang bisa muncul sebagai kata tunggal di ekspresi tetapi
/// bukan nama kolom.
const EXPR_KEYWORDS: &[&str] = &[
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "TRUE",
    "FALSE",
    "AS",
    "DISTINCT",
    "INTERVAL",
    "EXISTS",
    "SELECT",
    "FROM",
    "WHERE",
    "ON",
    "ASC",
    "DESC",
    "ALL",
    "ANY",
    "SOME",
    "ESCAPE",
    "COLLATE",
    "OVER",
    "PARTITION",
    "BY",
    "ORDER",
    "ROWS",
    "RANGE",
    "UNBOUNDED",
    "PRECEDING",
    "FOLLOWING",
    "CURRENT",
    "ROW",
    "FILTER",
    "WITHIN",
    "GROUP",
    "DIV",
    "MOD",
    "XOR",
    "REGEXP",
    "RLIKE",
    "SIMILAR",
    "TO",
    "AT",
    "TIME",
    "ZONE",
    "NULLS",
    "FIRST",
    "LAST",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "DEFAULT",
    "UNKNOWN",
];

/// Fungsi agregat yang menandai kolom hasil sebagai ringkasan.
const AGGREGATES: &[&str] = &[
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "GROUP_CONCAT",
    "STRING_AGG",
    "ARRAY_AGG",
    "JSON_AGG",
    "JSONB_AGG",
    "JSON_ARRAYAGG",
    "JSON_OBJECTAGG",
    "STDDEV",
    "VARIANCE",
    "BOOL_AND",
    "BOOL_OR",
    "LISTAGG",
    "COUNT_BIG",
];

pub(super) fn parse_model(
    sql: &str,
    db: &DatabaseType,
) -> Result<QueryDiagramModel, QueryDiagramError> {
    let dialect: Box<dyn Dialect> = match db {
        DatabaseType::MySQL => Box::new(MySqlDialect {}),
        DatabaseType::PostgreSQL => Box::new(PostgreSqlDialect {}),
        DatabaseType::SQLite => Box::new(SQLiteDialect {}),
        DatabaseType::MsSQL => Box::new(MsSqlDialect {}),
        _ => Box::new(GenericDialect {}),
    };
    let statements = match Parser::parse_sql(dialect.as_ref(), sql) {
        Ok(s) => s,
        Err(first) => Parser::parse_sql(&GenericDialect {}, sql).map_err(|_| {
            log::debug!("[QUERY_DIAGRAM] parse gagal: {first}");
            QueryDiagramError::Parse(first.to_string())
        })?,
    };
    let Some(stmt) = statements.first() else {
        return Err(QueryDiagramError::Empty);
    };

    let kind = match stmt {
        Statement::Query(_) => StatementKind::Select,
        Statement::Insert(_) => StatementKind::Insert,
        Statement::Update(_) => StatementKind::Update,
        Statement::Delete(_) => StatementKind::Delete,
        _ => {
            let word = sql
                .split_whitespace()
                .next()
                .unwrap_or("This")
                .to_uppercase();
            return Err(QueryDiagramError::Unsupported(word));
        }
    };
    let mut a = Analyzer::new(kind, sql, dialect.as_ref());
    match stmt {
        Statement::Query(q) => a.query(q, true),
        Statement::Insert(ins) => a.insert(ins),
        Statement::Update(up) => {
            a.table_with_joins(&up.table, None, true);
            if let Some(from) = &up.from {
                let (UpdateTableFromKind::BeforeSet(v) | UpdateTableFromKind::AfterSet(v)) = from;
                for twj in v {
                    a.table_with_joins(twj, Some("FROM".to_string()), false);
                }
            }
            for asg in &up.assignments {
                let cols: Vec<String> = match &asg.target {
                    AssignmentTarget::ColumnName(n) => vec![last_part(n)],
                    AssignmentTarget::Tuple(v) => v.iter().map(last_part).collect(),
                };
                let value = asg.value.to_string();
                let sources = a.column_refs(&value);
                for c in cols {
                    a.model.mutations.push(Mutation {
                        column: c,
                        new_value: value.clone(),
                        is_static: sources.is_empty(),
                        sources: sources.clone(),
                    });
                }
            }
            if let Some(sel) = &up.selection {
                a.filter(sel);
            }
            a.order_by_exprs(up.order_by.iter().map(|o| &o.expr));
            if let Some(l) = &up.limit {
                a.model.limit = Some(l.to_string());
            }
        }
        Statement::Delete(del) => {
            let (FromTable::WithFromKeyword(v) | FromTable::WithoutKeyword(v)) = &del.from;
            for twj in v {
                a.table_with_joins(twj, None, false);
            }
            if let Some(using) = &del.using {
                for twj in using {
                    a.table_with_joins(twj, Some("USING".to_string()), false);
                }
            }
            // Target: tabel yang disebut sebelum FROM (multi-table MySQL),
            // selain itu tabel FROM pertama.
            let wanted = del.tables.first().map(last_part);
            let idx = wanted
                .and_then(|w| {
                    a.model.sources.iter().position(|s| {
                        s.id.eq_ignore_ascii_case(&w)
                            || short_name(&s.table).eq_ignore_ascii_case(&w)
                    })
                })
                .unwrap_or(0);
            if idx < a.model.sources.len() {
                let mut t = a.model.sources.remove(idx);
                t.join = None;
                a.model.target = Some(t);
            }
            if del.tables.len() > 1 {
                a.note(format!(
                    "Rows are removed from {} tables.",
                    del.tables.len()
                ));
            }
            if let Some(sel) = &del.selection {
                a.filter(sel);
            }
            a.order_by_exprs(del.order_by.iter().map(|o| &o.expr));
            if let Some(l) = &del.limit {
                a.model.limit = Some(l.to_string());
            }
        }
        _ => {}
    }
    if statements.len() > 1 {
        a.note(format!(
            "Only the first of {} statements is shown.",
            statements.len()
        ));
    }
    a.drop_output_aliases();
    Ok(a.model)
}

fn last_part(name: &ObjectName) -> String {
    name.0
        .last()
        .and_then(|p| p.as_ident())
        .map(|i| i.value.clone())
        .unwrap_or_else(|| name.to_string())
}

fn object_name(name: &ObjectName) -> String {
    name.0
        .iter()
        .map(|p| {
            p.as_ident()
                .map(|i| i.value.clone())
                .unwrap_or_else(|| p.to_string())
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn short_name(table: &str) -> &str {
    table.rsplit('.').next().unwrap_or(table)
}

fn join_parts(op: &JoinOperator) -> (&'static str, Option<&JoinConstraint>) {
    match op {
        JoinOperator::Join(c) => ("JOIN", Some(c)),
        JoinOperator::Inner(c) => ("INNER JOIN", Some(c)),
        JoinOperator::Left(c) | JoinOperator::LeftOuter(c) => ("LEFT JOIN", Some(c)),
        JoinOperator::Right(c) | JoinOperator::RightOuter(c) => ("RIGHT JOIN", Some(c)),
        JoinOperator::FullOuter(c) => ("FULL JOIN", Some(c)),
        JoinOperator::CrossJoin(c) => ("CROSS JOIN", Some(c)),
        JoinOperator::Semi(c) | JoinOperator::LeftSemi(c) | JoinOperator::RightSemi(c) => {
            ("SEMI JOIN", Some(c))
        }
        JoinOperator::Anti(c) | JoinOperator::LeftAnti(c) | JoinOperator::RightAnti(c) => {
            ("ANTI JOIN", Some(c))
        }
        JoinOperator::CrossApply => ("CROSS APPLY", None),
        JoinOperator::OuterApply => ("OUTER APPLY", None),
        JoinOperator::AsOf { constraint, .. } => ("ASOF JOIN", Some(constraint)),
        JoinOperator::StraightJoin(c) => ("STRAIGHT_JOIN", Some(c)),
        #[allow(unreachable_patterns)]
        _ => ("JOIN", None),
    }
}

/// Kumpulkan pasangan `kiri = kanan` dari rangkaian AND.
fn equalities<'e>(e: &'e Expr, out: &mut Vec<(&'e Expr, &'e Expr)>) {
    match e {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            equalities(left, out);
            equalities(right, out);
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } => out.push((left, right)),
        Expr::Nested(inner) => equalities(inner, out),
        _ => {}
    }
}

/// Nama kolom-kolom hasil sebuah query (untuk subquery / CTE).
fn projection_names(q: &Query) -> Vec<String> {
    let SetExpr::Select(sel) = q.body.as_ref() else {
        return Vec::new();
    };
    sel.projection
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            SelectItem::UnnamedExpr(e) => Some(expr_name(e, i)),
            SelectItem::ExprWithAlias { alias, .. } => Some(alias.value.clone()),
            SelectItem::ExprWithAliases { aliases, expr } => Some(
                aliases
                    .first()
                    .map(|a| a.value.clone())
                    .unwrap_or_else(|| expr_name(expr, i)),
            ),
            _ => None,
        })
        .collect()
}

/// Tabel fisik yang dibaca langsung oleh query (dangkal).
fn tables_in_query(q: &Query) -> Vec<String> {
    let SetExpr::Select(sel) = q.body.as_ref() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for twj in &sel.from {
        let factors = std::iter::once(&twj.relation).chain(twj.joins.iter().map(|j| &j.relation));
        for f in factors {
            if let TableFactor::Table { name, .. } = f {
                out.push(object_name(name));
            }
        }
    }
    out
}

fn expr_name(e: &Expr, index: usize) -> String {
    match e {
        Expr::Identifier(i) => i.value.clone(),
        Expr::CompoundIdentifier(parts) => parts
            .last()
            .map(|p| p.value.clone())
            .unwrap_or_else(|| format!("expr{}", index + 1)),
        other => clip(&other.to_string(), 32),
    }
}

struct Analyzer<'d> {
    model: QueryDiagramModel,
    dialect: &'d dyn Dialect,
    /// (nama CTE huruf kecil, tabel yang dibacanya)
    ctes: Vec<(String, Vec<String>)>,
    /// (kualifier huruf kecil, id sumber). Entri terakhir menang.
    names: Vec<(String, String)>,
    subquery_seq: usize,
}

impl<'d> Analyzer<'d> {
    fn new(kind: StatementKind, sql: &str, dialect: &'d dyn Dialect) -> Self {
        Self {
            model: QueryDiagramModel::empty(kind, sql),
            dialect,
            ctes: Vec::new(),
            names: Vec::new(),
            subquery_seq: 0,
        }
    }

    fn note(&mut self, text: String) {
        if !self.model.notes.contains(&text) {
            self.model.notes.push(text);
        }
    }

    fn unique_id(&self, base: &str) -> String {
        let taken = |id: &str| self.model.tables().any(|t| t.id.eq_ignore_ascii_case(id));
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base}_{n}"))
            .find(|c| !taken(c))
            .unwrap_or_else(|| base.to_string())
    }

    fn register(&mut self, qualifier: &str, id: &str) {
        self.names.push((qualifier.to_lowercase(), id.to_string()));
    }

    fn resolve_qualifier(&self, q: &str) -> Option<String> {
        let q = q.to_lowercase();
        self.names
            .iter()
            .rev()
            .find(|(k, _)| *k == q)
            .map(|(_, id)| id.clone())
    }

    fn add_source(&mut self, src: SourceTable, as_target: bool) -> String {
        let id = src.id.clone();
        self.register(&id, &id);
        if src.kind == SourceKind::Table {
            self.register(&src.table, &id);
            let short = short_name(&src.table).to_string();
            self.register(&short, &id);
        }
        if as_target {
            self.model.target = Some(src);
        } else {
            self.model.sources.push(src);
        }
        id
    }

    fn factor(&mut self, f: &TableFactor, join: Option<String>, as_target: bool) -> Option<String> {
        match f {
            TableFactor::Table { name, alias, .. } => {
                let table = object_name(name);
                let alias = alias.as_ref().map(|a| a.name.value.clone());
                let short = short_name(&table).to_lowercase();
                let cte = self.ctes.iter().find(|(n, _)| *n == short).cloned();
                let kind = if cte.is_some() {
                    SourceKind::Cte
                } else {
                    SourceKind::Table
                };
                let base = alias
                    .clone()
                    .unwrap_or_else(|| short_name(&table).to_string());
                let id = self.unique_id(&base);
                let mut src = SourceTable::new(id, table, alias, kind);
                src.join = join;
                if let Some((name, reads)) = cte
                    && !reads.is_empty()
                {
                    self.note(format!("CTE `{name}` reads from {}.", reads.join(", ")));
                }
                Some(self.add_source(src, as_target))
            }
            TableFactor::Derived {
                subquery, alias, ..
            } => {
                self.subquery_seq += 1;
                let alias = alias.as_ref().map(|a| a.name.value.clone());
                let base = alias
                    .clone()
                    .unwrap_or_else(|| format!("subquery{}", self.subquery_seq));
                let id = self.unique_id(&base);
                let mut src = SourceTable::new(
                    id.clone(),
                    "(subquery)".to_string(),
                    Some(id.clone()),
                    SourceKind::Subquery,
                );
                src.join = join;
                let reads = tables_in_query(subquery);
                if !reads.is_empty() {
                    self.note(format!("Subquery `{id}` reads from {}.", reads.join(", ")));
                }
                for c in projection_names(subquery) {
                    src.push_column(&c);
                }
                Some(self.add_source(src, as_target))
            }
            TableFactor::NestedJoin {
                table_with_joins, ..
            } => self.table_with_joins(table_with_joins, join, as_target),
            other => {
                self.subquery_seq += 1;
                let id = self.unique_id(&format!("source{}", self.subquery_seq));
                let mut src =
                    SourceTable::new(id, clip(&other.to_string(), 40), None, SourceKind::Subquery);
                src.join = join;
                Some(self.add_source(src, as_target))
            }
        }
    }

    /// Tambah tabel dan join-nya. Mengembalikan id tabel pertama.
    fn table_with_joins(
        &mut self,
        twj: &TableWithJoins,
        first_join: Option<String>,
        target_first: bool,
    ) -> Option<String> {
        let first = self.factor(&twj.relation, first_join, target_first);
        for j in &twj.joins {
            let (label, constraint) = join_parts(&j.join_operator);
            let id = self.factor(&j.relation, Some(label.to_string()), false);
            match constraint {
                Some(JoinConstraint::On(e)) => self.links_from_expr(e, label, id.as_deref()),
                Some(JoinConstraint::Using(cols)) => {
                    if let (Some(l), Some(r)) = (first.clone(), id.clone()) {
                        for c in cols {
                            let c = last_part(c);
                            self.model.joins.push(JoinLink {
                                left: ColumnRef::new(l.clone(), c.clone()),
                                right: ColumnRef::new(r.clone(), c),
                                join_type: label.to_string(),
                            });
                        }
                    }
                }
                Some(JoinConstraint::Natural) => {
                    self.note(format!(
                        "{label} is NATURAL: columns with the same name are matched."
                    ));
                }
                _ => {}
            }
        }
        first
    }

    fn as_column(&self, e: &Expr) -> Option<ColumnRef> {
        match e {
            Expr::Identifier(i) => Some(ColumnRef::new("", i.value.clone())),
            Expr::CompoundIdentifier(parts) if parts.len() >= 2 => {
                let col = parts[parts.len() - 1].value.clone();
                let qual = &parts[parts.len() - 2].value;
                let id = self.resolve_qualifier(qual).or_else(|| {
                    let full: Vec<&str> = parts[..parts.len() - 1]
                        .iter()
                        .map(|p| p.value.as_str())
                        .collect();
                    self.resolve_qualifier(&full.join("."))
                })?;
                Some(ColumnRef::new(id, col))
            }
            Expr::Nested(inner) => self.as_column(inner),
            _ => None,
        }
    }

    /// Bentuk relasi dari kesetaraan kolom; kolom lain di kondisi yang sama
    /// dicatat sebagai kolom yang dipakai.
    fn links_from_expr(&mut self, e: &Expr, label: &str, new_id: Option<&str>) {
        let mut eqs = Vec::new();
        equalities(e, &mut eqs);
        for (l, r) in eqs {
            let (Some(cl), Some(cr)) = (self.as_column(l), self.as_column(r)) else {
                continue;
            };
            if cl.table.is_empty() && cr.table.is_empty() {
                continue;
            }
            if cl.table == cr.table {
                continue;
            }
            let (left, right) = if new_id.is_some_and(|n| n == cl.table) {
                (cr, cl)
            } else {
                (cl, cr)
            };
            let dup = self.model.joins.iter().any(|j| {
                (j.left == left && j.right == right) || (j.left == right && j.right == left)
            });
            if !dup {
                self.model.joins.push(JoinLink {
                    left,
                    right,
                    join_type: label.to_string(),
                });
            }
        }
        let refs = self.column_refs(&e.to_string());
        self.model.referenced.extend(refs);
    }

    fn filter(&mut self, e: &Expr) {
        let text = e.to_string();
        self.model.filter_columns = self.column_refs(&text);
        self.model.filter = Some(text);
        // Join gaya lama / UPDATE ... FROM / DELETE ... USING menaruh relasi di WHERE.
        if self.model.tables().count() > 1 {
            let before = self.model.referenced.len();
            self.links_from_expr(e, "WHERE", None);
            self.model.referenced.truncate(before);
        }
    }

    fn order_by_exprs<'e>(&mut self, exprs: impl Iterator<Item = &'e Expr>) {
        for e in exprs {
            let text = e.to_string();
            let refs = self.column_refs(&text);
            self.model.referenced.extend(refs);
            self.model.order_by.push(text);
        }
    }

    fn query(&mut self, q: &Query, collect_output: bool) {
        if let Some(with) = &q.with {
            for cte in &with.cte_tables {
                let name = cte.alias.name.value.clone();
                self.ctes
                    .push((name.to_lowercase(), tables_in_query(&cte.query)));
            }
        }
        self.set_expr(&q.body, collect_output);
        if let Some(ob) = &q.order_by
            && let OrderByKind::Expressions(v) = &ob.kind
        {
            self.order_by_exprs(v.iter().map(|o| &o.expr));
        }
        match &q.limit_clause {
            Some(LimitClause::LimitOffset { limit: Some(l), .. }) => {
                self.model.limit = Some(l.to_string());
            }
            Some(LimitClause::OffsetCommaLimit { limit, .. }) => {
                self.model.limit = Some(limit.to_string());
            }
            _ => {}
        }
        if let Some(f) = &q.fetch {
            self.model.limit = Some(f.to_string());
        }
    }

    fn set_expr(&mut self, body: &SetExpr, collect_output: bool) {
        match body {
            SetExpr::Select(sel) => self.select(sel, collect_output),
            SetExpr::Query(q) => self.query(q, collect_output),
            SetExpr::SetOperation { left, op, .. } => {
                self.note(format!(
                    "Combined with another query using {op}; only the first query is drawn."
                ));
                self.set_expr(left, collect_output);
            }
            SetExpr::Values(v) => {
                self.model.values_rows = v.rows.len();
                let src = SourceTable::new(
                    VALUES_ID.to_string(),
                    "VALUES".to_string(),
                    None,
                    SourceKind::Values,
                );
                self.add_source(src, false);
                if let Some(first) = v.rows.first() {
                    for (i, e) in first.content.iter().enumerate() {
                        let name = format!("column{}", i + 1);
                        self.model.output.push(OutputColumn {
                            name: name.clone(),
                            expr: e.to_string(),
                            sources: vec![ColumnRef::new(VALUES_ID, name)],
                            aggregate: false,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn select(&mut self, sel: &Select, collect_output: bool) {
        for twj in &sel.from {
            let first_join = if self.model.sources.is_empty() {
                None
            } else {
                Some(",".to_string())
            };
            self.table_with_joins(twj, first_join, false);
        }
        self.model.distinct = sel.distinct.is_some();
        if let Some(top) = &sel.top {
            self.model.limit = Some(top.to_string());
        }
        if collect_output {
            self.projection(&sel.projection);
        }
        if let Some(w) = &sel.selection {
            self.filter(w);
        }
        match &sel.group_by {
            GroupByExpr::Expressions(v, _) => {
                for e in v {
                    let text = e.to_string();
                    let refs = self.column_refs(&text);
                    self.model.referenced.extend(refs);
                    self.model.group_by.push(text);
                }
            }
            GroupByExpr::All(_) => self.model.group_by.push("ALL".to_string()),
        }
        if let Some(h) = &sel.having {
            let text = h.to_string();
            let refs = self.column_refs(&text);
            self.model.referenced.extend(refs);
            self.model.having = Some(text);
        }
    }

    fn projection(&mut self, items: &[SelectItem]) {
        for (i, item) in items.iter().enumerate() {
            match item {
                SelectItem::UnnamedExpr(e) => self.output_expr(expr_name(e, i), e),
                SelectItem::ExprWithAlias { expr, alias } => {
                    self.output_expr(alias.value.clone(), expr)
                }
                SelectItem::ExprWithAliases { expr, aliases } => {
                    let name = aliases
                        .first()
                        .map(|a| a.value.clone())
                        .unwrap_or_else(|| expr_name(expr, i));
                    self.output_expr(name, expr);
                }
                SelectItem::QualifiedWildcard(kind, _) => {
                    let text = kind.to_string();
                    let qual = text.trim_end_matches(".*");
                    let id = self.resolve_qualifier(qual).or_else(|| {
                        self.resolve_qualifier(qual.rsplit('.').next().unwrap_or(qual))
                    });
                    if let Some(id) = id {
                        if let Some(t) = self.model.sources.iter_mut().find(|t| t.id == id) {
                            t.all_columns = true;
                        }
                        self.model.output.push(OutputColumn {
                            name: format!("{id}.*"),
                            expr: text.clone(),
                            sources: vec![ColumnRef::new(id, "*")],
                            aggregate: false,
                        });
                    }
                }
                SelectItem::Wildcard(_) => {
                    let ids: Vec<String> =
                        self.model.sources.iter().map(|t| t.id.clone()).collect();
                    for t in &mut self.model.sources {
                        t.all_columns = true;
                    }
                    for id in ids {
                        self.model.output.push(OutputColumn {
                            name: format!("{id}.*"),
                            expr: "*".to_string(),
                            sources: vec![ColumnRef::new(id, "*")],
                            aggregate: false,
                        });
                    }
                }
            }
        }
    }

    fn output_expr(&mut self, name: String, e: &Expr) {
        let expr = e.to_string();
        let sources = self.column_refs(&expr);
        let aggregate = self.is_aggregate(&expr);
        self.model.output.push(OutputColumn {
            name,
            expr,
            sources,
            aggregate,
        });
    }

    fn insert(&mut self, ins: &Insert) {
        let table = match &ins.table {
            TableObject::TableName(n) => object_name(n),
            other => clip(&other.to_string(), 40),
        };
        let alias = ins.table_alias.as_ref().map(|a| a.alias.value.clone());
        let base = alias
            .clone()
            .unwrap_or_else(|| short_name(&table).to_string());
        let id = self.unique_id(&base);
        let target = SourceTable::new(id, table, alias, SourceKind::Table);
        self.add_source(target, true);
        let cols: Vec<String> = ins.columns.iter().map(last_part).collect();
        let col_name = |i: usize| {
            cols.get(i)
                .cloned()
                .unwrap_or_else(|| format!("#{}", i + 1))
        };

        if !ins.assignments.is_empty() {
            for asg in &ins.assignments {
                let names: Vec<String> = match &asg.target {
                    AssignmentTarget::ColumnName(n) => vec![last_part(n)],
                    AssignmentTarget::Tuple(v) => v.iter().map(last_part).collect(),
                };
                let value = asg.value.to_string();
                let sources = self.column_refs(&value);
                for c in names {
                    self.model.mutations.push(Mutation {
                        column: c,
                        new_value: value.clone(),
                        is_static: sources.is_empty(),
                        sources: sources.clone(),
                    });
                }
            }
        } else if let Some(src) = &ins.source {
            if let SetExpr::Values(v) = src.body.as_ref() {
                self.model.values_rows = v.rows.len();
                let values = SourceTable::new(
                    VALUES_ID.to_string(),
                    "VALUES".to_string(),
                    None,
                    SourceKind::Values,
                );
                self.add_source(values, false);
                if let Some(first) = v.rows.first() {
                    for (i, e) in first.content.iter().enumerate() {
                        let c = col_name(i);
                        self.model.mutations.push(Mutation {
                            column: c.clone(),
                            new_value: e.to_string(),
                            sources: vec![ColumnRef::new(VALUES_ID, c)],
                            is_static: true,
                        });
                    }
                }
            } else {
                self.query(src, true);
                let outputs = self.model.output.clone();
                for (i, out) in outputs.iter().enumerate() {
                    let c = cols.get(i).cloned().unwrap_or_else(|| {
                        if out.name.ends_with(".*") {
                            format!("#{}", i + 1)
                        } else {
                            out.name.clone()
                        }
                    });
                    self.model.mutations.push(Mutation {
                        column: c,
                        new_value: out.expr.clone(),
                        sources: out.sources.clone(),
                        is_static: out.sources.is_empty(),
                    });
                }
            }
        }
        if let Some(on) = &ins.on {
            self.note(format!("On conflict: {}", clip(&on.to_string(), 120)));
        }
        if ins.returning.is_some() {
            self.note("Returns the inserted rows (RETURNING).".to_string());
        }
    }

    /// Referensi kolom di sebuah potongan SQL (ekspresi).
    fn column_refs(&self, text: &str) -> Vec<ColumnRef> {
        let tokens: Vec<Token> = match Tokenizer::new(self.dialect, text).tokenize() {
            Ok(t) => t
                .into_iter()
                .filter(|t| !matches!(t, Token::Whitespace(_)))
                .collect(),
            Err(_) => return Vec::new(),
        };
        let mut out: Vec<ColumnRef> = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
            // Lewati subquery "(SELECT ...)": kolomnya milik scope lain.
            if matches!(tokens[i], Token::LParen)
                && matches!(tokens.get(i + 1), Some(Token::Word(w)) if w.value.eq_ignore_ascii_case("select"))
            {
                let mut depth = 0i32;
                while i < tokens.len() {
                    match tokens[i] {
                        Token::LParen => depth += 1,
                        Token::RParen => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
                continue;
            }
            let Token::Word(first) = &tokens[i] else {
                i += 1;
                continue;
            };
            let mut parts = vec![first.clone()];
            let mut j = i + 1;
            let mut wildcard = false;
            while j + 1 < tokens.len() && tokens[j] == Token::Period {
                match &tokens[j + 1] {
                    Token::Word(w) => {
                        parts.push(w.clone());
                        j += 2;
                    }
                    Token::Mul => {
                        wildcard = true;
                        j += 2;
                        break;
                    }
                    _ => break,
                }
            }
            let prev = if i > 0 { tokens.get(i - 1) } else { None };
            let next = tokens.get(j);
            i = j;
            if wildcard || matches!(next, Some(Token::LParen)) {
                continue;
            }
            match prev {
                Some(Token::Word(p)) if p.value.eq_ignore_ascii_case("as") => continue,
                Some(
                    Token::SingleQuotedString(_)
                    | Token::Number(..)
                    | Token::DoubleColon
                    | Token::Colon
                    | Token::AtSign,
                ) => continue,
                _ => {}
            }
            let r = if parts.len() == 1 {
                let p = &parts[0];
                if matches!(next, Some(Token::SingleQuotedString(_))) {
                    continue;
                }
                if p.quote_style.is_none()
                    && EXPR_KEYWORDS
                        .iter()
                        .any(|k| k.eq_ignore_ascii_case(&p.value))
                {
                    continue;
                }
                ColumnRef::new("", p.value.clone())
            } else {
                let col = parts[parts.len() - 1].value.clone();
                let qual = &parts[parts.len() - 2].value;
                let id = self.resolve_qualifier(qual).or_else(|| {
                    let full: Vec<&str> = parts[..parts.len() - 1]
                        .iter()
                        .map(|p| p.value.as_str())
                        .collect();
                    self.resolve_qualifier(&full.join("."))
                });
                match id {
                    Some(id) => ColumnRef::new(id, col),
                    None => continue,
                }
            };
            if !out.contains(&r) {
                out.push(r);
            }
        }
        out
    }

    fn is_aggregate(&self, text: &str) -> bool {
        let Ok(tokens) = Tokenizer::new(self.dialect, text).tokenize() else {
            return false;
        };
        let tokens: Vec<Token> = tokens
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace(_)))
            .collect();
        tokens.windows(2).any(|w| {
            matches!((&w[0], &w[1]), (Token::Word(f), Token::LParen)
                if AGGREGATES.iter().any(|a| a.eq_ignore_ascii_case(&f.value)))
        })
    }

    /// ORDER BY/GROUP BY/HAVING boleh memakai alias kolom hasil; alias itu
    /// bukan kolom tabel sehingga tidak boleh masuk kartu sumber.
    fn drop_output_aliases(&mut self) {
        let aliases: Vec<String> = self
            .model
            .output
            .iter()
            .filter(|o| {
                !o.expr.eq_ignore_ascii_case(&o.name) && !o.expr.ends_with(&format!(".{}", o.name))
            })
            .map(|o| o.name.to_lowercase())
            .collect();
        self.model
            .referenced
            .retain(|r| !(r.table.is_empty() && aliases.contains(&r.column.to_lowercase())));
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn my(sql: &str) -> QueryDiagramModel {
        analyze_statement(sql, &DatabaseType::MySQL).expect("statement valid")
    }

    fn cols(m: &QueryDiagramModel, id: &str) -> Vec<String> {
        m.table(id).map(|t| t.columns.clone()).unwrap_or_default()
    }

    #[test]
    fn test_select_join_group_by() {
        let m = my("SELECT u.name, COUNT(o.id) AS total FROM users u \
                    LEFT JOIN orders o ON o.user_id = u.id \
                    WHERE u.active = 1 GROUP BY u.name ORDER BY total DESC LIMIT 10");
        assert_eq!(m.kind, StatementKind::Select);
        assert_eq!(m.sources.len(), 2);
        assert_eq!(m.sources[1].join.as_deref(), Some("LEFT JOIN"));
        assert_eq!(m.joins.len(), 1);
        let j = &m.joins[0];
        assert_eq!(j.left, ColumnRef::new("u", "id"));
        assert_eq!(j.right, ColumnRef::new("o", "user_id"));
        assert_eq!(m.output.len(), 2);
        assert!(m.output[1].aggregate);
        assert_eq!(m.output[1].sources, vec![ColumnRef::new("o", "id")]);
        assert_eq!(m.limit.as_deref(), Some("10"));
        assert_eq!(m.group_by, vec!["u.name".to_string()]);
        // Alias `total` di ORDER BY bukan kolom tabel.
        assert!(!cols(&m, "u").contains(&"total".to_string()));
        assert!(cols(&m, "u").contains(&"active".to_string()));
        assert_eq!(m.filter_columns, vec![ColumnRef::new("u", "active")]);
    }

    #[test]
    fn test_select_star_expands_with_schema() {
        let lookup = |t: &str| (t == "users").then(|| vec!["id".to_string(), "email".to_string()]);
        let m =
            analyze_with_schema("SELECT * FROM users", &DatabaseType::PostgreSQL, &lookup).unwrap();
        let names: Vec<&str> = m.output.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, vec!["id", "email"]);
        assert_eq!(cols(&m, "users"), vec!["id", "email"]);
    }

    #[test]
    fn test_unqualified_column_resolves_by_schema() {
        let lookup = |t: &str| match t {
            "a" => Some(vec!["id".to_string()]),
            "b" => Some(vec!["a_id".to_string(), "price".to_string()]),
            _ => None,
        };
        let m = analyze_with_schema(
            "SELECT price FROM a JOIN b ON b.a_id = a.id",
            &DatabaseType::MySQL,
            &lookup,
        )
        .unwrap();
        assert_eq!(m.output[0].sources, vec![ColumnRef::new("b", "price")]);
    }

    #[test]
    fn test_implicit_join_in_where() {
        let m = my("SELECT a.x FROM a, b WHERE a.id = b.a_id AND b.flag = 'y'");
        assert_eq!(m.joins.len(), 1);
        assert_eq!(m.joins[0].join_type, "WHERE");
    }

    #[test]
    fn test_insert_values() {
        let m = my("INSERT INTO users (name, email) VALUES ('a', 'a@x'), ('b', 'b@x')");
        assert_eq!(m.kind, StatementKind::Insert);
        assert_eq!(m.target.as_ref().map(|t| t.table.as_str()), Some("users"));
        assert_eq!(m.values_rows, 2);
        assert_eq!(m.mutations.len(), 2);
        assert_eq!(m.mutations[0].column, "name");
        assert_eq!(m.mutations[0].new_value, "'a'");
        assert_eq!(cols(&m, "users"), vec!["name", "email"]);
    }

    #[test]
    fn test_insert_values_without_columns_uses_schema_order() {
        let lookup = |t: &str| (t == "t").then(|| vec!["id".to_string(), "name".to_string()]);
        let m = analyze_with_schema(
            "INSERT INTO t VALUES (1, 'x')",
            &DatabaseType::SQLite,
            &lookup,
        )
        .unwrap();
        let names: Vec<&str> = m.mutations.iter().map(|x| x.column.as_str()).collect();
        assert_eq!(names, vec!["id", "name"]);
        let without =
            analyze_statement("INSERT INTO t VALUES (1, 'x')", &DatabaseType::SQLite).unwrap();
        assert_eq!(without.mutations[1].column, "column 2");
    }

    #[test]
    fn test_insert_select() {
        let m = my(
            "INSERT INTO archive (id, total) SELECT o.id, o.amount FROM orders o WHERE o.year < 2020",
        );
        assert_eq!(m.mutations.len(), 2);
        assert_eq!(m.mutations[1].column, "total");
        assert_eq!(m.mutations[1].sources, vec![ColumnRef::new("o", "amount")]);
        assert_eq!(m.sources.len(), 1);
    }

    #[test]
    fn test_update_static() {
        let m =
            my("UPDATE products SET price = price * 1.1, status = 'sale' WHERE category_id = 3");
        assert_eq!(m.kind, StatementKind::Update);
        assert_eq!(m.mutations.len(), 2);
        assert!(!m.mutations[0].is_static);
        assert_eq!(
            m.mutations[0].sources,
            vec![ColumnRef::new("products", "price")]
        );
        assert!(m.mutations[1].is_static);
        assert!(cols(&m, "products").contains(&"category_id".to_string()));
    }

    #[test]
    fn test_update_from_other_table_postgres() {
        let m = analyze_statement(
            "UPDATE orders o SET status = s.name FROM statuses s WHERE s.id = o.status_id",
            &DatabaseType::PostgreSQL,
        )
        .unwrap();
        assert_eq!(m.target.as_ref().map(|t| t.id.as_str()), Some("o"));
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.mutations[0].sources, vec![ColumnRef::new("s", "name")]);
        assert_eq!(m.joins.len(), 1);
    }

    #[test]
    fn test_update_join_mysql() {
        let m = my("UPDATE orders o JOIN users u ON u.id = o.user_id SET o.email = u.email");
        assert_eq!(m.target.as_ref().map(|t| t.id.as_str()), Some("o"));
        assert_eq!(m.joins.len(), 1);
        assert_eq!(m.mutations[0].column, "email");
    }

    #[test]
    fn test_delete_with_where() {
        let m = my("DELETE FROM sessions WHERE expires_at < NOW()");
        assert_eq!(m.kind, StatementKind::Delete);
        assert_eq!(
            m.target.as_ref().map(|t| t.table.as_str()),
            Some("sessions")
        );
        assert!(m.sources.is_empty());
        assert_eq!(
            m.filter_columns,
            vec![ColumnRef::new("sessions", "expires_at")]
        );
    }

    #[test]
    fn test_delete_using() {
        let m = analyze_statement(
            "DELETE FROM orders o USING users u WHERE u.id = o.user_id AND u.banned",
            &DatabaseType::PostgreSQL,
        )
        .unwrap();
        assert_eq!(m.target.as_ref().map(|t| t.id.as_str()), Some("o"));
        assert_eq!(m.joins.len(), 1);
    }

    #[test]
    fn test_mssql_brackets_and_top() {
        let m = analyze_statement(
            "SELECT TOP 5 [u].[name] FROM [dbo].[users] AS [u]",
            &DatabaseType::MsSQL,
        )
        .unwrap();
        assert_eq!(m.sources[0].table, "dbo.users");
        assert_eq!(m.output[0].sources, vec![ColumnRef::new("u", "name")]);
        assert!(m.limit.as_deref().unwrap_or("").contains('5'));
    }

    #[test]
    fn test_subquery_in_where_is_not_a_column() {
        let m = my("SELECT id FROM users WHERE id IN (SELECT user_id FROM orders)");
        assert_eq!(m.filter_columns, vec![ColumnRef::new("users", "id")]);
    }

    #[test]
    fn test_unsupported_statement() {
        let err = analyze_statement("CREATE TABLE t (id int)", &DatabaseType::MySQL).unwrap_err();
        assert_eq!(err, QueryDiagramError::Unsupported("CREATE".to_string()));
    }

    #[test]
    fn test_cte_is_marked() {
        let m = my("WITH recent AS (SELECT * FROM orders) SELECT r.id FROM recent r");
        assert_eq!(m.sources[0].kind, SourceKind::Cte);
        assert!(m.notes.iter().any(|n| n.contains("orders")));
    }
}
