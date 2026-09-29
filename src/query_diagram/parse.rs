//! Pemetaan AST `sqlparser` ke [`QueryDiagramModel`].
//!
//! Referensi kolom di dalam ekspresi dikumpulkan lewat tokenizer (bukan
//! walker AST penuh) supaya tahan terhadap puluhan varian `Expr` dan
//! sintaks khusus dialek. Kondisi kesetaraan `a.x = b.y` dan subquery
//! (`IN`, `EXISTS`, skalar) dibaca dari AST.

use sqlparser::ast::{
    AssignmentTarget, BinaryOperator, Expr, FromTable, FunctionArg, FunctionArgExpr,
    FunctionArguments, GroupByExpr, Insert, JoinConstraint, JoinOperator, LimitClause, MergeAction,
    MergeClauseKind, MergeInsertKind, ObjectName, OnConflictAction, OnInsert, OrderByKind, Query,
    Select, SelectItem, SetExpr, Statement, TableFactor, TableObject, TableWithJoins,
    UpdateTableFromKind,
};
use sqlparser::dialect::{
    Dialect, GenericDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::{
    ColumnRef, JoinLink, Mutation, OutputColumn, QueryDiagramError, QueryDiagramModel, SetBranch,
    SourceKind, SourceTable, StatementKind, VALUES_ID, clip,
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

/// Batas kedalaman CTE/subquery yang diuraikan (mencegah rekursi tanpa akhir).
const MAX_DEPTH: usize = 3;

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
        Statement::CreateTable(ct) if ct.query.is_some() => StatementKind::Insert,
        Statement::Update(_) | Statement::Merge(_) => StatementKind::Update,
        Statement::Delete(_) | Statement::Truncate(_) => StatementKind::Delete,
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
        Statement::Query(q) => {
            a.query(q, true);
            if let Some(into) = a.select_into.take() {
                a.copy_output_into(&into, "SELECT INTO", &[]);
            }
        }
        Statement::CreateTable(ct) => {
            if let Some(q) = &ct.query {
                a.query(q, true);
                let names: Vec<String> = ct.columns.iter().map(|c| c.name.value.clone()).collect();
                a.copy_output_into(&object_name(&ct.name), "CREATE TABLE AS", &names);
            }
        }
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
                a.assignment(&asg.target, &asg.value, false);
            }
            if let Some(sel) = &up.selection {
                a.filter(sel);
            }
            a.order_by_exprs(up.order_by.iter().map(|o| &o.expr));
            if let Some(l) = &up.limit {
                a.model.limit = Some(l.to_string());
            }
            a.merge_update_target_alias();
        }
        Statement::Merge(m) => {
            a.model.verb = Some("MERGE INTO".to_string());
            a.factor(&m.table, None, true);
            let src = a.factor(&m.source, Some("USING".to_string()), false);
            a.links_from_expr(&m.on, "ON", src.as_deref());
            for clause in &m.clauses {
                let when = match clause.clause_kind {
                    MergeClauseKind::Matched => "WHEN MATCHED",
                    MergeClauseKind::NotMatchedBySource => "WHEN NOT MATCHED BY SOURCE",
                    #[allow(unreachable_patterns)]
                    _ => "WHEN NOT MATCHED",
                };
                if let Some(p) = &clause.predicate {
                    a.note(format!(
                        "{when} AND {}: the action runs only for those rows.",
                        clip(&p.to_string(), 80)
                    ));
                }
                match &clause.action {
                    MergeAction::Update(u) => {
                        for asg in &u.assignments {
                            a.assignment(&asg.target, &asg.value, false);
                        }
                    }
                    MergeAction::Insert(ins) => {
                        a.model.upsert_label = format!("{when} INSERT");
                        let cols: Vec<String> = ins.columns.iter().map(last_part).collect();
                        if let MergeInsertKind::Values(v) = &ins.kind
                            && let Some(first) = v.rows.first()
                        {
                            for (i, e) in first.content.iter().enumerate() {
                                let value = e.to_string();
                                let sources = a.column_refs(&value);
                                a.model.upserts.push(Mutation {
                                    column: cols
                                        .get(i)
                                        .cloned()
                                        .unwrap_or_else(|| format!("column {}", i + 1)),
                                    new_value: value,
                                    is_static: sources.is_empty(),
                                    sources,
                                });
                            }
                        }
                    }
                    MergeAction::Delete { .. } => {
                        a.note(format!(
                            "{when} THEN DELETE: those target rows are removed."
                        ));
                    }
                }
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
            // Target: tabel yang disebut sebelum FROM (multi-table MySQL /
            // SQL Server), selain itu tabel FROM pertama.
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
        Statement::Truncate(t) => {
            a.model.verb = Some("TRUNCATE".to_string());
            if let Some(first) = t.table_names.first() {
                let table = object_name(&first.name);
                let id = a.unique_id(short_name(&table));
                a.add_source(SourceTable::new(id, table, None, SourceKind::Table), true);
            }
            if t.table_names.len() > 1 {
                a.note(format!("{} tables are emptied.", t.table_names.len()));
            }
            a.note("TRUNCATE removes every row at once; it cannot be filtered and is usually not logged row by row.".to_string());
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

/// Klausa `OVER (...)` di teks ekspresi, bila ada (window function).
fn window_clause(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    let pos = upper.find(" OVER (").or_else(|| upper.find(" OVER("))?;
    Some(text[pos + 1..].trim().to_string())
}

/// Daun-daun UNION/INTERSECT/EXCEPT beserta operator yang mendahuluinya.
fn flatten_set<'a>(e: &'a SetExpr, op: String, out: &mut Vec<(String, &'a SetExpr)>) {
    match e {
        SetExpr::SetOperation {
            left,
            op: o,
            set_quantifier,
            right,
        } => {
            flatten_set(left, op, out);
            let label = format!("{o} {set_quantifier}").trim().to_string();
            flatten_set(right, label, out);
        }
        other => out.push((op, other)),
    }
}

/// Hasil analisis ringan query di dalam CTE, subquery, atau cabang UNION.
struct InnerResult {
    /// (nama kolom hasil, kolom sumber). `"*"` = semua kolom.
    outputs: Vec<(String, Vec<ColumnRef>)>,
    /// Sumber langsung yang ditambahkan query ini.
    ids: Vec<String>,
    filter: Option<String>,
}

struct Analyzer<'d> {
    model: QueryDiagramModel,
    dialect: &'d dyn Dialect,
    /// (nama CTE huruf kecil, nama asli, query)
    ctes: Vec<(String, String, Query)>,
    /// (kualifier huruf kecil, id sumber). Entri terakhir menang.
    names: Vec<(String, String)>,
    subquery_seq: usize,
    /// Cabang UNION yang sedang dianalisis.
    branch: usize,
    /// Kedalaman CTE/subquery yang sedang diuraikan.
    depth: usize,
    /// CTE yang sedang diuraikan (untuk CTE rekursif).
    expanding: Vec<String>,
    /// Target `SELECT ... INTO tabel`.
    select_into: Option<String>,
}

impl<'d> Analyzer<'d> {
    fn new(kind: StatementKind, sql: &str, dialect: &'d dyn Dialect) -> Self {
        Self {
            model: QueryDiagramModel::empty(kind, sql),
            dialect,
            ctes: Vec::new(),
            names: Vec::new(),
            subquery_seq: 0,
            branch: 0,
            depth: 0,
            expanding: Vec::new(),
            select_into: None,
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

    fn add_source(&mut self, mut src: SourceTable, as_target: bool) -> String {
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
            src.branch = self.branch;
            self.model.sources.push(src);
        }
        id
    }

    fn source_mut(&mut self, id: &str) -> Option<&mut SourceTable> {
        self.model.sources.iter_mut().find(|t| t.id == id)
    }

    fn factor(&mut self, f: &TableFactor, join: Option<String>, as_target: bool) -> Option<String> {
        match f {
            TableFactor::Table { name, alias, .. } => {
                let table = object_name(name);
                let alias = alias.as_ref().map(|a| a.name.value.clone());
                let short = short_name(&table).to_lowercase();
                let cte = self.ctes.iter().find(|(n, _, _)| *n == short).cloned();
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
                let id = self.add_source(src, as_target);
                if let Some((lower, name, q)) = cte
                    && !self.expanding.contains(&lower)
                {
                    self.expand_derived(&id, &q, &name, &lower);
                }
                Some(id)
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
                let id = self.add_source(src, as_target);
                let key = format!("__derived_{id}");
                self.expand_derived(&id, subquery, &id.clone(), &key);
                Some(id)
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

    /// Uraikan isi CTE/subquery `q` menjadi tabel-tabel yang mengisi kartu `id`.
    fn expand_derived(&mut self, id: &str, q: &Query, name: &str, key: &str) {
        if self.depth >= MAX_DEPTH {
            return;
        }
        self.expanding.push(key.to_string());
        self.depth += 1;
        let res = self.inner_query(q, &format!("in {name}"), Some(id), "JOIN");
        self.depth -= 1;
        self.expanding.pop();
        for (out, refs) in res.outputs {
            let to = if out == "*" {
                ColumnRef::new(id, "*")
            } else {
                if let Some(t) = self.source_mut(id) {
                    t.push_column(&out);
                }
                ColumnRef::new(id, out)
            };
            for r in refs {
                self.model.derived_links.push((r, to.clone()));
            }
        }
        if let Some(f) = res.filter {
            self.note(format!("{name} keeps only rows where {}.", clip(&f, 100)));
        }
    }

    /// Analisis ringan query bersarang: tabel FROM/JOIN, relasi, kondisi, dan
    /// kolom hasil. Alias di dalamnya hanya berlaku selama analisis ini.
    fn inner_query(
        &mut self,
        q: &Query,
        badge: &str,
        feeds: Option<&str>,
        link: &str,
    ) -> InnerResult {
        let mark = self.names.len();
        if let Some(with) = &q.with {
            for cte in &with.cte_tables {
                let name = cte.alias.name.value.clone();
                self.ctes
                    .push((name.to_lowercase(), name, (*cte.query).clone()));
            }
        }
        let mut body = q.body.as_ref();
        loop {
            match body {
                SetExpr::Query(inner) => body = inner.body.as_ref(),
                SetExpr::SetOperation { left, .. } => body = left.as_ref(),
                _ => break,
            }
        }
        let res = match body {
            SetExpr::Select(sel) => self.inner_select(sel, badge, feeds, link),
            _ => InnerResult {
                outputs: Vec::new(),
                ids: Vec::new(),
                filter: None,
            },
        };
        self.names.truncate(mark);
        res
    }

    fn inner_select(
        &mut self,
        sel: &Select,
        badge: &str,
        feeds: Option<&str>,
        link: &str,
    ) -> InnerResult {
        let before = self.model.sources.len();
        for twj in &sel.from {
            self.table_with_joins(twj, None, false);
        }
        let mut ids = Vec::new();
        for s in self.model.sources.iter_mut().skip(before) {
            if s.feeds.is_some() {
                continue; // milik CTE yang lebih dalam
            }
            s.feeds = feeds.map(str::to_string);
            if s.join.is_none() && s.badge.is_none() {
                s.badge = Some(badge.to_string());
            }
            ids.push(s.id.clone());
        }
        let single = (ids.len() == 1).then(|| ids[0].clone());
        let qualify = |mut refs: Vec<ColumnRef>| {
            if let Some(id) = &single {
                for r in &mut refs {
                    if r.table.is_empty() {
                        r.table = id.clone();
                    }
                }
            }
            refs
        };

        let filter = sel.selection.as_ref().map(|w| w.to_string());
        if let Some(w) = &sel.selection {
            let mut eqs = Vec::new();
            equalities(w, &mut eqs);
            for (l, r) in eqs {
                let (Some(cl), Some(cr)) = (self.as_column(l), self.as_column(r)) else {
                    continue;
                };
                let (cl, cr) = (qualify(vec![cl]).remove(0), qualify(vec![cr]).remove(0));
                if cl.table.is_empty() || cr.table.is_empty() || cl.table == cr.table {
                    continue;
                }
                self.model.joins.push(JoinLink {
                    left: cl,
                    right: cr,
                    join_type: link.to_string(),
                });
            }
            let refs = qualify(self.column_refs(&w.to_string()));
            self.model.referenced.extend(refs);
            self.walk_subqueries(w);
        }

        let mut outputs = Vec::new();
        for (i, item) in sel.projection.iter().enumerate() {
            match item {
                SelectItem::UnnamedExpr(e) => {
                    outputs.push((expr_name(e, i), qualify(self.column_refs(&e.to_string()))));
                }
                SelectItem::ExprWithAlias { expr, alias } => {
                    outputs.push((
                        alias.value.clone(),
                        qualify(self.column_refs(&expr.to_string())),
                    ));
                }
                SelectItem::ExprWithAliases { expr, aliases } => {
                    let name = aliases
                        .first()
                        .map(|a| a.value.clone())
                        .unwrap_or_else(|| expr_name(expr, i));
                    outputs.push((name, qualify(self.column_refs(&expr.to_string()))));
                }
                SelectItem::QualifiedWildcard(kind, _) => {
                    let text = kind.to_string();
                    let qual = text.trim_end_matches(".*");
                    if let Some(id) = self.resolve_qualifier(qual) {
                        outputs.push(("*".to_string(), vec![ColumnRef::new(id, "*")]));
                    }
                }
                SelectItem::Wildcard(_) => {
                    let refs = ids
                        .iter()
                        .map(|id| ColumnRef::new(id.clone(), "*"))
                        .collect();
                    outputs.push(("*".to_string(), refs));
                }
            }
        }
        if let GroupByExpr::Expressions(v, _) = &sel.group_by {
            for e in v {
                let refs = qualify(self.column_refs(&e.to_string()));
                self.model.referenced.extend(refs);
            }
        }
        InnerResult {
            outputs,
            ids,
            filter,
        }
    }

    /// Cari subquery di ekspresi. `IN`/`EXISTS` menambah tabel dan relasi;
    /// subquery skalar mengembalikan kolom sumbernya.
    fn walk_subqueries(&mut self, e: &Expr) -> Vec<ColumnRef> {
        if self.depth >= MAX_DEPTH {
            return Vec::new();
        }
        match e {
            Expr::InSubquery {
                expr,
                subquery,
                negated,
            } => {
                let label = if *negated { "NOT IN" } else { "IN" };
                self.depth += 1;
                let res = self.inner_query(subquery, &format!("{label} (subquery)"), None, label);
                self.depth -= 1;
                if let Some(outer) = self.as_column(expr)
                    && let Some((_, refs)) = res.outputs.first()
                    && let Some(r) = refs.first()
                    && r.column != "*"
                {
                    self.model.joins.push(JoinLink {
                        left: outer,
                        right: r.clone(),
                        join_type: label.to_string(),
                    });
                }
                self.walk_subqueries(expr)
            }
            Expr::Exists { subquery, negated } => {
                let label = if *negated { "NOT EXISTS" } else { "EXISTS" };
                self.depth += 1;
                self.inner_query(subquery, label, None, label);
                self.depth -= 1;
                Vec::new()
            }
            Expr::Subquery(q) => {
                self.depth += 1;
                let res = self.inner_query(q, "SCALAR SUBQUERY", None, "SUBQUERY");
                self.depth -= 1;
                res.outputs
                    .into_iter()
                    .next()
                    .map(|(_, r)| r)
                    .unwrap_or_default()
            }
            Expr::BinaryOp { left, right, .. } => {
                let mut out = self.walk_subqueries(left);
                out.extend(self.walk_subqueries(right));
                out
            }
            Expr::UnaryOp { expr, .. }
            | Expr::Nested(expr)
            | Expr::IsNull(expr)
            | Expr::IsNotNull(expr)
            | Expr::Cast { expr, .. } => self.walk_subqueries(expr),
            Expr::Between {
                expr, low, high, ..
            } => {
                let mut out = self.walk_subqueries(expr);
                out.extend(self.walk_subqueries(low));
                out.extend(self.walk_subqueries(high));
                out
            }
            Expr::InList { expr, list, .. } => {
                let mut out = self.walk_subqueries(expr);
                for x in list {
                    out.extend(self.walk_subqueries(x));
                }
                out
            }
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                let mut out = Vec::new();
                if let Some(o) = operand {
                    out.extend(self.walk_subqueries(o));
                }
                for c in conditions {
                    out.extend(self.walk_subqueries(&c.condition));
                    out.extend(self.walk_subqueries(&c.result));
                }
                if let Some(x) = else_result {
                    out.extend(self.walk_subqueries(x));
                }
                out
            }
            Expr::Function(f) => {
                let mut out = Vec::new();
                match &f.args {
                    FunctionArguments::List(list) => {
                        for arg in &list.args {
                            if let FunctionArg::Unnamed(FunctionArgExpr::Expr(x))
                            | FunctionArg::Named {
                                arg: FunctionArgExpr::Expr(x),
                                ..
                            }
                            | FunctionArg::ExprNamed {
                                arg: FunctionArgExpr::Expr(x),
                                ..
                            } = arg
                            {
                                out.extend(self.walk_subqueries(x));
                            }
                        }
                    }
                    FunctionArguments::Subquery(q) => {
                        self.depth += 1;
                        let res = self.inner_query(q, "SCALAR SUBQUERY", None, "SUBQUERY");
                        self.depth -= 1;
                        if let Some((_, r)) = res.outputs.into_iter().next() {
                            out.extend(r);
                        }
                    }
                    FunctionArguments::None => {}
                }
                out
            }
            _ => Vec::new(),
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
        self.walk_subqueries(e);
    }

    /// Satu `kolom = nilai` pada UPDATE/MERGE (atau upsert bila `upsert`).
    fn assignment(&mut self, target: &AssignmentTarget, value: &Expr, upsert: bool) {
        let cols: Vec<String> = match target {
            AssignmentTarget::ColumnName(n) => vec![last_part(n)],
            AssignmentTarget::Tuple(v) => v.iter().map(last_part).collect(),
        };
        let text = value.to_string();
        let mut sources = self.column_refs(&text);
        for r in self.walk_subqueries(value) {
            if !sources.contains(&r) {
                sources.push(r);
            }
        }
        for c in cols {
            let m = Mutation {
                column: c,
                new_value: text.clone(),
                is_static: sources.is_empty(),
                sources: sources.clone(),
            };
            if upsert {
                self.model.upserts.push(m);
            } else {
                self.model.mutations.push(m);
            }
        }
    }

    /// UPDATE gaya SQL Server: `UPDATE o SET ... FROM orders o JOIN ...`.
    /// Target yang hanya menyebut alias di FROM digabung dengan tabel itu.
    fn merge_update_target_alias(&mut self) {
        let Some(target) = &self.model.target else {
            return;
        };
        if target.alias.is_some() {
            return;
        }
        let name = target.table.to_lowercase();
        let target_id = target.id.clone();
        let Some(pos) = self.model.sources.iter().position(|s| {
            s.kind == SourceKind::Table
                && (s.alias.as_deref().map(str::to_lowercase) == Some(name.clone())
                    || (s.alias.is_none() && s.table.to_lowercase() == name))
        }) else {
            return;
        };
        let src = self.model.sources.remove(pos);
        let old_id = src.id.clone();
        if let Some(t) = self.model.target.as_mut() {
            t.table = src.table;
            t.alias = src.alias;
        }
        self.model.remap_table(&old_id, &target_id);
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
                    .push((name.to_lowercase(), name, (*cte.query).clone()));
            }
            if with.recursive {
                self.note("WITH RECURSIVE: the CTE repeats until no new rows are produced; only its first step is drawn.".to_string());
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
            SetExpr::SetOperation { .. } => self.set_operation(body, collect_output),
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
                            window: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// UNION / INTERSECT / EXCEPT: query pertama dianalisis penuh, cabang
    /// berikutnya menambah tabel dan mengisi kolom hasil sesuai posisi.
    fn set_operation(&mut self, body: &SetExpr, collect_output: bool) {
        let mut leaves = Vec::new();
        flatten_set(body, String::new(), &mut leaves);
        let Some((_, first)) = leaves.first() else {
            return;
        };
        self.set_expr(first, collect_output);
        let first_tables = self
            .model
            .sources
            .iter()
            .filter(|s| s.branch == 0 && s.feeds.is_none())
            .map(|s| s.title())
            .collect();
        self.model.branches.push(SetBranch {
            op: String::new(),
            tables: first_tables,
            filter: self.model.filter.clone(),
        });
        for (k, (op, leaf)) in leaves.iter().enumerate().skip(1) {
            self.branch = k;
            let badge = format!("{op} #{}", k + 1);
            let mark = self.names.len();
            let res = match leaf {
                SetExpr::Select(sel) => self.inner_select(sel, &badge, None, "JOIN"),
                SetExpr::Query(q) => self.inner_query(q, &badge, None, "JOIN"),
                _ => InnerResult {
                    outputs: Vec::new(),
                    ids: Vec::new(),
                    filter: None,
                },
            };
            self.names.truncate(mark);
            for (i, (_, refs)) in res.outputs.iter().enumerate() {
                if let Some(o) = self.model.output.get_mut(i) {
                    for r in refs {
                        if !o.sources.contains(r) {
                            o.sources.push(r.clone());
                        }
                    }
                }
            }
            let tables = res
                .ids
                .iter()
                .filter_map(|id| self.model.table(id).map(|t| t.title()))
                .collect();
            self.model.branches.push(SetBranch {
                op: op.clone(),
                tables,
                filter: res.filter,
            });
        }
        self.branch = 0;
    }

    fn select(&mut self, sel: &Select, collect_output: bool) {
        for twj in &sel.from {
            let first_join = if self.model.sources.iter().any(|s| s.branch == self.branch) {
                Some(",".to_string())
            } else {
                None
            };
            self.table_with_joins(twj, first_join, false);
        }
        self.model.distinct = sel.distinct.is_some();
        if let Some(top) = &sel.top {
            self.model.limit = Some(top.to_string());
        }
        if let Some(into) = &sel.into {
            self.select_into = Some(object_name(&into.name));
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
                    self.model.referenced.extend(refs.clone());
                    self.model.group_columns.push(refs);
                    self.model.group_by.push(text);
                }
            }
            GroupByExpr::All(_) => {
                self.model.group_by.push("ALL".to_string());
                self.model.group_columns.push(Vec::new());
            }
        }
        if let Some(h) = &sel.having {
            let text = h.to_string();
            let refs = self.column_refs(&text);
            self.model.referenced.extend(refs.clone());
            self.model.having_columns = refs;
            self.model.having = Some(text);
            self.walk_subqueries(h);
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
                            window: None,
                        });
                    }
                }
                SelectItem::Wildcard(_) => {
                    let ids: Vec<String> = self
                        .model
                        .sources
                        .iter()
                        .filter(|t| t.feeds.is_none() && t.branch == self.branch)
                        .map(|t| t.id.clone())
                        .collect();
                    for t in &mut self.model.sources {
                        if ids.contains(&t.id) {
                            t.all_columns = true;
                        }
                    }
                    for id in ids {
                        self.model.output.push(OutputColumn {
                            name: format!("{id}.*"),
                            expr: "*".to_string(),
                            sources: vec![ColumnRef::new(id, "*")],
                            aggregate: false,
                            window: None,
                        });
                    }
                }
            }
        }
    }

    fn output_expr(&mut self, name: String, e: &Expr) {
        let expr = e.to_string();
        let mut sources = self.column_refs(&expr);
        for r in self.walk_subqueries(e) {
            if !sources.contains(&r) {
                sources.push(r);
            }
        }
        let window = window_clause(&expr);
        let aggregate = window.is_none() && self.is_aggregate(&expr);
        self.model.output.push(OutputColumn {
            name,
            expr,
            sources,
            aggregate,
            window,
        });
    }

    /// `SELECT ... INTO t` / `CREATE TABLE t AS SELECT`: hasil SELECT disalin
    /// ke tabel baru, digambar seperti INSERT ... SELECT.
    fn copy_output_into(&mut self, table: &str, verb: &str, names: &[String]) {
        self.model.kind = StatementKind::Insert;
        self.model.verb = Some(verb.to_string());
        let id = self.unique_id(short_name(table));
        self.add_source(
            SourceTable::new(id, table.to_string(), None, SourceKind::Table),
            true,
        );
        let outputs = self.model.output.clone();
        for (i, out) in outputs.iter().enumerate() {
            if out.name.ends_with(".*") && names.get(i).is_none() {
                self.note(format!(
                    "All columns of {} are copied.",
                    out.name.trim_end_matches(".*")
                ));
                continue;
            }
            self.model.mutations.push(Mutation {
                column: names.get(i).cloned().unwrap_or_else(|| out.name.clone()),
                new_value: out.expr.clone(),
                sources: out.sources.clone(),
                is_static: out.sources.is_empty(),
            });
        }
    }

    fn insert(&mut self, ins: &Insert) {
        let table = match &ins.table {
            TableObject::TableName(n) => object_name(n),
            other => clip(&other.to_string(), 40),
        };
        if ins.replace_into {
            self.model.verb = Some("REPLACE INTO".to_string());
            self.note(
                "REPLACE INTO deletes an existing row with the same key, then inserts the new one."
                    .to_string(),
            );
        }
        if ins.ignore {
            self.note("INSERT IGNORE: rows that would break a unique key are skipped.".to_string());
        }
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
                self.assignment(&asg.target, &asg.value, false);
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
        match &ins.on {
            Some(OnInsert::DuplicateKeyUpdate(assigns)) => {
                self.model.upsert_label = "ON DUPLICATE KEY UPDATE".to_string();
                for asg in assigns {
                    self.assignment(&asg.target, &asg.value, true);
                }
            }
            Some(OnInsert::OnConflict(oc)) => match &oc.action {
                OnConflictAction::DoNothing => {
                    self.note(
                        "ON CONFLICT DO NOTHING: rows that already exist are skipped.".to_string(),
                    );
                }
                OnConflictAction::DoUpdate(du) => {
                    self.model.upsert_label = "ON CONFLICT DO UPDATE".to_string();
                    for asg in &du.assignments {
                        self.assignment(&asg.target, &asg.value, true);
                    }
                    if let Some(sel) = &du.selection {
                        self.note(format!(
                            "The conflict update only applies where {}.",
                            clip(&sel.to_string(), 80)
                        ));
                    }
                }
            },
            #[allow(unreachable_patterns)]
            Some(other) => self.note(format!("On conflict: {}", clip(&other.to_string(), 120))),
            None => {}
        }
        if ins.returning.is_some() {
            self.note("Returns the inserted rows (RETURNING).".to_string());
        }
    }

    /// Token tanpa spasi dan tanpa isi subquery `(SELECT ...)`.
    fn top_level_tokens(&self, text: &str) -> Vec<Token> {
        let Ok(tokens) = Tokenizer::new(self.dialect, text).tokenize() else {
            return Vec::new();
        };
        let tokens: Vec<Token> = tokens
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace(_)))
            .collect();
        let mut out = Vec::with_capacity(tokens.len());
        let mut i = 0;
        while i < tokens.len() {
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
            out.push(tokens[i].clone());
            i += 1;
        }
        out
    }

    /// Referensi kolom di sebuah potongan SQL (ekspresi), tanpa isi subquery.
    fn column_refs(&self, text: &str) -> Vec<ColumnRef> {
        let tokens = self.top_level_tokens(text);
        let mut out: Vec<ColumnRef> = Vec::new();
        let mut i = 0;
        while i < tokens.len() {
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

    /// Ekspresi memakai fungsi agregat di luar subquery.
    fn is_aggregate(&self, text: &str) -> bool {
        let tokens = self.top_level_tokens(text);
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
        let is_alias =
            |r: &ColumnRef| r.table.is_empty() && aliases.contains(&r.column.to_lowercase());
        self.model.referenced.retain(|r| !is_alias(r));
        self.model.having_columns.retain(|r| !is_alias(r));
        for g in &mut self.model.group_columns {
            g.retain(|r| !is_alias(r));
        }
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
        assert_eq!(m.group_columns, vec![vec![ColumnRef::new("u", "name")]]);
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
        assert_eq!(
            m.table("orders").and_then(|t| t.feeds.clone()).as_deref(),
            Some("r")
        );
    }

    fn pg(sql: &str) -> QueryDiagramModel {
        analyze_statement(sql, &DatabaseType::PostgreSQL).expect("statement valid")
    }

    fn ms(sql: &str) -> QueryDiagramModel {
        analyze_statement(sql, &DatabaseType::MsSQL).expect("statement valid")
    }

    #[test]
    fn test_in_subquery_adds_table_and_link() {
        let m = my(
            "SELECT u.name FROM users u WHERE u.id IN (SELECT o.user_id FROM orders o WHERE o.total > 100)",
        );
        let o = m.table("o").expect("tabel subquery ikut digambar");
        assert_eq!(o.badge.as_deref(), Some("IN (subquery)"));
        assert!(o.feeds.is_none());
        assert!(m.joins.iter().any(|j| j.join_type == "IN"
            && j.left == ColumnRef::new("u", "id")
            && j.right == ColumnRef::new("o", "user_id")));
        assert_eq!(m.filter_columns, vec![ColumnRef::new("u", "id")]);
        assert!(cols(&m, "o").contains(&"total".to_string()));
    }

    #[test]
    fn test_not_exists_correlated() {
        let m = my(
            "SELECT u.id FROM users u WHERE NOT EXISTS (SELECT 1 FROM bans b WHERE b.user_id = u.id)",
        );
        assert_eq!(
            m.table("b").and_then(|t| t.badge.clone()).as_deref(),
            Some("NOT EXISTS")
        );
        assert!(m.joins.iter().any(|j| j.join_type == "NOT EXISTS"));
    }

    #[test]
    fn test_scalar_subquery_in_select_is_not_aggregate() {
        let m = my(
            "SELECT u.name, (SELECT MAX(o.total) FROM orders o WHERE o.user_id = u.id) AS top FROM users u",
        );
        assert_eq!(m.output[1].sources, vec![ColumnRef::new("o", "total")]);
        assert!(!m.output[1].aggregate);
        assert_eq!(
            m.table("o").and_then(|t| t.badge.clone()).as_deref(),
            Some("SCALAR SUBQUERY")
        );
    }

    #[test]
    fn test_update_set_from_subquery_is_not_static() {
        let m = my(
            "UPDATE products p SET price = (SELECT AVG(h.price) FROM history h WHERE h.product_id = p.id)",
        );
        assert!(!m.mutations[0].is_static);
        assert_eq!(m.mutations[0].sources, vec![ColumnRef::new("h", "price")]);
    }

    #[test]
    fn test_cte_tables_feed_the_cte() {
        let m = my(
            "WITH recent AS (SELECT o.id, o.user_id FROM orders o WHERE o.total > 5) SELECT r.id FROM recent r",
        );
        assert_eq!(m.sources[0].kind, SourceKind::Cte);
        let o = m.table("o").expect("tabel di dalam CTE");
        assert_eq!(o.feeds.as_deref(), Some("r"));
        assert!(
            m.derived_links
                .contains(&(ColumnRef::new("o", "id"), ColumnRef::new("r", "id")))
        );
        assert_eq!(cols(&m, "r"), vec!["id", "user_id"]);
        // Alias di dalam CTE tidak bocor ke query luar.
        assert!(m.output[0].sources == vec![ColumnRef::new("r", "id")]);
    }

    #[test]
    fn test_derived_table_is_expanded() {
        let m = my("SELECT t.n FROM (SELECT u.name AS n FROM users u) t");
        assert!(
            m.derived_links
                .contains(&(ColumnRef::new("u", "name"), ColumnRef::new("t", "n")))
        );
        assert_eq!(
            m.table("u").and_then(|x| x.feeds.clone()).as_deref(),
            Some("t")
        );
    }

    #[test]
    fn test_recursive_cte_terminates() {
        let m = pg(
            "WITH RECURSIVE tree AS (SELECT id, parent_id FROM nodes UNION ALL \
                    SELECT n.id, n.parent_id FROM nodes n JOIN tree t ON n.parent_id = t.id) \
                    SELECT * FROM tree",
        );
        assert!(m.table("nodes").is_some());
        assert!(m.notes.iter().any(|n| n.contains("RECURSIVE")));
    }

    #[test]
    fn test_union_all_branches() {
        let m = my("SELECT id, name FROM users UNION ALL SELECT id, name FROM admins");
        assert_eq!(m.branches.len(), 2);
        assert_eq!(m.branches[1].op, "UNION ALL");
        let admins = m.table("admins").unwrap();
        assert_eq!(admins.branch, 1);
        assert_eq!(admins.badge.as_deref(), Some("UNION ALL #2"));
        assert!(m.output[0].sources.contains(&ColumnRef::new("users", "id")));
        assert!(
            m.output[0]
                .sources
                .contains(&ColumnRef::new("admins", "id"))
        );
    }

    #[test]
    fn test_window_function_is_not_aggregate() {
        let m = my("SELECT name, SUM(amount) OVER (PARTITION BY dept) AS s FROM emp");
        assert!(m.output[1].window.is_some());
        assert!(!m.output[1].aggregate);
    }

    #[test]
    fn test_upsert_mysql_and_postgres() {
        let m = my("INSERT INTO t (id, n) VALUES (1, 'a') ON DUPLICATE KEY UPDATE n = VALUES(n)");
        assert_eq!(m.upsert_label, "ON DUPLICATE KEY UPDATE");
        assert_eq!(m.upserts.len(), 1);
        let p = pg(
            "INSERT INTO t (id, n) VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET n = EXCLUDED.n",
        );
        assert_eq!(p.upsert_label, "ON CONFLICT DO UPDATE");
        assert_eq!(p.upserts[0].column, "n");
    }

    #[test]
    fn test_replace_into_and_truncate() {
        let m = my("REPLACE INTO t (id) VALUES (1)");
        assert_eq!(m.verb.as_deref(), Some("REPLACE INTO"));
        let t = my("TRUNCATE TABLE logs");
        assert_eq!(t.kind, StatementKind::Delete);
        assert_eq!(t.verb.as_deref(), Some("TRUNCATE"));
        assert_eq!(t.target.as_ref().map(|x| x.table.as_str()), Some("logs"));
    }

    #[test]
    fn test_select_into_and_create_table_as() {
        let m = ms("SELECT id, name INTO backup_users FROM users");
        assert_eq!(m.kind, StatementKind::Insert);
        assert_eq!(m.verb.as_deref(), Some("SELECT INTO"));
        assert_eq!(
            m.target.as_ref().map(|x| x.table.as_str()),
            Some("backup_users")
        );
        assert_eq!(m.mutations.len(), 2);
        let c = pg("CREATE TABLE top_users AS SELECT u.id FROM users u");
        assert_eq!(c.verb.as_deref(), Some("CREATE TABLE AS"));
        assert_eq!(c.mutations[0].sources, vec![ColumnRef::new("u", "id")]);
    }

    #[test]
    fn test_merge() {
        let m = ms(
            "MERGE INTO stock AS t USING incoming AS s ON t.sku = s.sku \
                    WHEN MATCHED THEN UPDATE SET t.qty = s.qty \
                    WHEN NOT MATCHED THEN INSERT (sku, qty) VALUES (s.sku, s.qty);",
        );
        assert_eq!(m.kind, StatementKind::Update);
        assert_eq!(m.verb.as_deref(), Some("MERGE INTO"));
        assert_eq!(m.target.as_ref().map(|x| x.id.as_str()), Some("t"));
        assert_eq!(m.joins.len(), 1);
        assert_eq!(m.mutations[0].sources, vec![ColumnRef::new("s", "qty")]);
        assert_eq!(m.upserts.len(), 2);
        assert!(m.upsert_label.contains("NOT MATCHED"));
    }

    #[test]
    fn test_mssql_update_alias_from() {
        let m = ms(
            "UPDATE o SET o.status = s.name FROM orders o JOIN statuses s ON s.id = o.status_id",
        );
        let t = m.target.as_ref().unwrap();
        assert_eq!(t.table, "orders");
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.mutations[0].sources, vec![ColumnRef::new("s", "name")]);
        assert!(
            m.joins
                .iter()
                .any(|j| j.left.table == t.id || j.right.table == t.id)
        );
    }
}
