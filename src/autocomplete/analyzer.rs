//! Analisis konteks kursor: klausa aktif, apa yang diharapkan di posisi kursor
//! (`Expect`), dan tabel/alias/CTE yang terlihat (scope).
//!
//! Semua fungsi di sini murni (tanpa akses `Tabular`/cache) sehingga mudah dites.
//! Parser sengaja toleran: SQL di posisi kursor hampir selalu belum lengkap.

use super::lexer::{Dialect, TokKind, Token, tokenize};

/// Klausa SQL tempat kursor berada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clause {
    StatementStart,
    SelectList,
    From,
    JoinOn,
    Using,
    Where,
    GroupPending,
    GroupBy,
    Having,
    OrderPending,
    OrderBy,
    Limit,
    Insert,
    InsertTarget,
    InsertColumns,
    Values,
    UpdateTarget,
    UpdateSet,
    Delete,
    With,
    SetOp,
    Other,
}

/// Jenis token yang diharapkan di posisi kursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Tidak ada completion (di dalam string, komentar, nama alias, dsb.).
    None,
    StatementStart,
    /// Nama tabel (setelah FROM/JOIN/INTO/UPDATE).
    Table,
    /// Tepat setelah nama tabel: alias atau klausa berikutnya.
    AfterTable {
        joined: bool,
        aliased: bool,
    },
    /// Awal ekspresi: kolom, fungsi, dsb.
    Column,
    /// Tepat setelah `ON` (atau `AND` di dalam ON).
    JoinCondition,
    /// Setelah operand kiri: operator pembanding.
    Operator,
    /// Setelah operator pembanding: nilai/kolom pembanding.
    Value,
    /// Kondisi/ekspresi sudah lengkap: AND/OR/klausa berikutnya.
    AfterExpr,
    /// Setelah satu item SELECT: AS / FROM.
    AfterSelectItem,
    /// Hanya keyword tertentu yang valid.
    Keywords(&'static [&'static str]),
    /// Konteks tidak dikenali: campuran keyword + tabel + kolom.
    Generic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Base,
    Cte,
    Derived,
}

/// Tabel yang terlihat di posisi kursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeTable {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
    pub kind: TableKind,
    /// 0 = query saat ini, 1 = query luar (correlated), dst.
    pub depth: usize,
    /// Index token nama tabel (untuk urutan).
    pub idx: usize,
    /// Kolom eksplisit untuk CTE/derived table.
    pub columns: Vec<String>,
    /// Sumber `SELECT *` untuk CTE/derived table (diekspansi lewat katalog).
    pub star_from: Vec<ScopeTable>,
    /// Tabel target INSERT/UPDATE/DELETE.
    pub dml_target: bool,
}

impl ScopeTable {
    /// Nama yang dipakai sebagai qualifier (alias bila ada).
    pub fn display(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColRef {
    pub qualifier: Option<String>,
    pub column: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectItem {
    /// Nama output (alias atau nama kolom), bila bisa ditentukan.
    pub name: Option<String>,
    /// Qualifier kolom sumber (`u` pada `u.name`).
    pub qualifier: Option<String>,
    /// Teks ekspresi apa adanya (tanpa alias).
    pub text: String,
    pub aggregate: bool,
    pub is_star: bool,
    /// Ekspresi punya alias eksplisit.
    pub has_alias: bool,
}

/// Hasil analisis konteks di posisi kursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    pub expect: Expect,
    pub clause: Clause,
    /// Kata yang sedang diketik (tanpa qualifier).
    pub partial: String,
    /// Segmen sebelum `partial`, mis. `["u"]` untuk `u.na|`.
    pub qualifier: Vec<String>,
    pub scope: Vec<ScopeTable>,
    pub ctes: Vec<ScopeTable>,
    pub select_items: Vec<SelectItem>,
    /// Kolom (lowercase, tanpa qualifier) yang sudah dipakai di daftar saat ini.
    pub used_columns: Vec<String>,
    /// Operand kiri untuk `Operator`/`Value`.
    pub lhs: Option<ColRef>,
    /// Posisi nilai di baris VALUES tanpa daftar kolom eksplisit.
    pub value_index: Option<usize>,
    pub join_target: Option<ScopeTable>,
    pub dml_target: Option<ScopeTable>,
    /// Nama fungsi (uppercase) bila kursor berada di argumen fungsi.
    pub in_function: Option<String>,
    /// Posisi boleh diisi subquery `(SELECT ...)`.
    pub allow_subquery: bool,
    /// Kursor tepat setelah `SELECT` (untuk DISTINCT/TOP).
    pub right_after_select: bool,
    /// Tabel yang diharapkan adalah target `JOIN` (bukan setelah koma/FROM).
    pub join_pending: bool,
    /// Index token awal kata di kursor; tabel dengan `idx` lebih kecil ditulis sebelum kursor.
    pub cursor_tok: usize,
}

impl Analysis {
    fn none() -> Self {
        Analysis {
            expect: Expect::None,
            clause: Clause::Other,
            partial: String::new(),
            qualifier: Vec::new(),
            scope: Vec::new(),
            ctes: Vec::new(),
            select_items: Vec::new(),
            used_columns: Vec::new(),
            lhs: None,
            value_index: None,
            join_target: None,
            dml_target: None,
            in_function: None,
            allow_subquery: false,
            right_after_select: false,
            join_pending: false,
            cursor_tok: 0,
        }
    }

    /// Nama tabel dasar (lowercase, unik) yang kolomnya dibutuhkan engine.
    pub fn referenced_tables(&self) -> Vec<String> {
        fn walk(t: &ScopeTable, out: &mut Vec<String>) {
            if t.kind == TableKind::Base {
                let n = t.name.to_ascii_lowercase();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            for s in &t.star_from {
                walk(s, out);
            }
        }
        let mut out: Vec<String> = Vec::new();
        for t in self.scope.iter().chain(self.ctes.iter()) {
            walk(t, &mut out);
        }
        // `users.` tanpa FROM: qualifier bisa berupa nama tabel langsung
        if let Some(q) = self.qualifier.last() {
            let q = q.to_ascii_lowercase();
            let is_alias = self.scope.iter().any(|t| {
                t.alias
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(&q))
            });
            if !is_alias && !out.contains(&q) {
                out.push(q);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Item: token pada satu level kurung, dengan grup kurung dan rantai nama diringkas.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Item {
    Name {
        parts: Vec<String>,
        first: usize,
        last: usize,
    },
    /// Nama langsung diikuti `(`: pemanggilan fungsi, `INSERT INTO t (...)`, `WITH x(a) AS`.
    Call {
        name: String,
        name_idx: usize,
        open: usize,
        close: Option<usize>,
    },
    Group {
        open: usize,
        close: Option<usize>,
    },
    Kw {
        up: String,
        idx: usize,
    },
    Op {
        text: String,
        idx: usize,
    },
    Comma(usize),
    Lit(usize),
}

impl Item {
    fn kw(&self) -> Option<&str> {
        match self {
            Item::Kw { up, .. } => Some(up.as_str()),
            _ => None,
        }
    }

    fn is_kw(&self, k: &str) -> bool {
        self.kw() == Some(k)
    }

    fn is_kw_any(&self, ks: &[&str]) -> bool {
        self.kw().is_some_and(|k| ks.contains(&k))
    }

    fn is_comma(&self) -> bool {
        matches!(self, Item::Comma(_))
    }

    /// Item yang bisa berdiri sebagai operand ekspresi.
    fn is_operand(&self) -> bool {
        match self {
            Item::Name { .. } | Item::Call { .. } | Item::Group { .. } | Item::Lit(_) => true,
            Item::Kw { up, .. } => matches!(
                up.as_str(),
                "NULL"
                    | "TRUE"
                    | "FALSE"
                    | "DEFAULT"
                    | "CURRENT_DATE"
                    | "CURRENT_TIME"
                    | "CURRENT_TIMESTAMP"
            ),
            _ => false,
        }
    }

    fn span(&self) -> (usize, usize) {
        match self {
            Item::Name { first, last, .. } => (*first, *last),
            Item::Call {
                name_idx,
                open,
                close,
                ..
            } => (*name_idx, close.unwrap_or(*open)),
            Item::Group { open, close } => (*open, close.unwrap_or(*open)),
            Item::Kw { idx, .. } | Item::Op { idx, .. } | Item::Comma(idx) | Item::Lit(idx) => {
                (*idx, *idx)
            }
        }
    }

    fn col_ref(&self) -> Option<ColRef> {
        match self {
            Item::Name { parts, .. } if parts.last().is_some_and(|p| p != "*") => {
                let column = parts.last()?.clone();
                let qualifier = if parts.len() >= 2 {
                    Some(parts[parts.len() - 2].clone())
                } else {
                    None
                };
                Some(ColRef { qualifier, column })
            }
            _ => None,
        }
    }
}

/// Keyword struktural — kata lain dianggap identifier (termasuk nama fungsi).
const RESERVED: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "STRAIGHT_JOIN",
    "ON",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "LIKE",
    "ILIKE",
    "RLIKE",
    "REGEXP",
    "SIMILAR",
    "BETWEEN",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "AS",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "OUTER",
    "CROSS",
    "NATURAL",
    "UNION",
    "INTERSECT",
    "EXCEPT",
    "MINUS",
    "ALL",
    "DISTINCT",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "WITH",
    "RECURSIVE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "EXISTS",
    "ASC",
    "DESC",
    "USING",
    "RETURNING",
    "TRUE",
    "FALSE",
    "NULLS",
    "OVER",
    "PARTITION",
    "LATERAL",
    "ANY",
    "SOME",
    "ESCAPE",
    "DEFAULT",
    "CREATE",
    "ALTER",
    "DROP",
    "TABLE",
    "TRUNCATE",
    "EXPLAIN",
    "FETCH",
    "TOP",
    "WINDOW",
    "QUALIFY",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "ONLY",
];

pub(crate) fn is_reserved(word: &str) -> bool {
    RESERVED.iter().any(|k| k.eq_ignore_ascii_case(word))
}

pub(crate) const AGGREGATES: &[&str] = &[
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
    "BOOL_AND",
    "BOOL_OR",
    "EVERY",
    "STDDEV",
    "VARIANCE",
    "LISTAGG",
    "JSON_ARRAYAGG",
    "JSON_OBJECTAGG",
];

const CMP_OPS: &[&str] = &["=", "<>", "!=", "<", ">", "<=", ">=", "<=>", "=="];
const ARITH_OPS: &[&str] = &[
    "+", "-", "*", "/", "%", "||", "::", "->", "->>", "#>", "#>>", "&", "|", "^",
];

/// Pasangan kurung: index `(` → index `)` (None bila belum ditutup).
fn match_parens(toks: &[Token]) -> Vec<Option<usize>> {
    let mut close = vec![None; toks.len()];
    let mut stack = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            TokKind::LParen => stack.push(i),
            TokKind::RParen => {
                if let Some(o) = stack.pop() {
                    close[o] = Some(i);
                }
            }
            _ => {}
        }
    }
    close
}

/// Bangun daftar item untuk token `[from, to)` pada level kurung terluar.
fn build_items(toks: &[Token], from: usize, to: usize, close: &[Option<usize>]) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut i = from;
    while i < to {
        let t = &toks[i];
        match t.kind {
            TokKind::LParen => {
                let c = close[i].filter(|c| *c < to);
                // Nama tepat sebelum `(` → pemanggilan fungsi
                let call = match items.last() {
                    Some(Item::Name { parts, first, last }) if *last + 1 == i => {
                        Some((parts.last().cloned().unwrap_or_default(), *first))
                    }
                    _ => None,
                };
                if let Some((name, name_idx)) = call {
                    items.pop();
                    items.push(Item::Call {
                        name,
                        name_idx,
                        open: i,
                        close: c,
                    });
                } else {
                    items.push(Item::Group { open: i, close: c });
                }
                match c {
                    Some(c) => i = c + 1,
                    None => break,
                }
            }
            TokKind::RParen | TokKind::Dot | TokKind::Semicolon | TokKind::Comment => i += 1,
            TokKind::Comma => {
                items.push(Item::Comma(i));
                i += 1;
            }
            TokKind::Number | TokKind::Str | TokKind::Param => {
                items.push(Item::Lit(i));
                i += 1;
            }
            TokKind::Op => {
                items.push(Item::Op {
                    text: t.text.clone(),
                    idx: i,
                });
                i += 1;
            }
            TokKind::Word if is_reserved(&t.text) => {
                items.push(Item::Kw {
                    up: t.text.to_ascii_uppercase(),
                    idx: i,
                });
                i += 1;
            }
            TokKind::Word | TokKind::QuotedIdent => {
                let first = i;
                let mut parts = vec![t.text.clone()];
                let mut j = i + 1;
                // Rantai hanya bersambung bila segmen menempel pada titik: `u. FROM`
                // (qualifier yang sedang diketik) tidak boleh menelan keyword berikutnya.
                while j + 1 < to
                    && toks[j].kind == TokKind::Dot
                    && toks[j + 1].start == toks[j].end
                    && (matches!(toks[j + 1].kind, TokKind::Word | TokKind::QuotedIdent)
                        || toks[j + 1].is_op("*"))
                {
                    parts.push(toks[j + 1].text.clone());
                    j += 2;
                }
                items.push(Item::Name {
                    parts,
                    first,
                    last: j - 1,
                });
                i = j;
            }
        }
    }
    items
}

fn last_index_where(items: &[Item], f: impl Fn(&Item) -> bool) -> Option<usize> {
    items.iter().rposition(f)
}

// ---------------------------------------------------------------------------
// Scope: tabel, alias, CTE, derived table.
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    sql: &'a str,
    toks: &'a [Token],
    close: &'a [Option<usize>],
    /// Range token yang diabaikan (kata yang sedang diketik).
    skip: std::ops::Range<usize>,
}

impl Ctx<'_> {
    fn skipped(&self, item: &Item) -> bool {
        let (a, b) = item.span();
        !self.skip.is_empty() && a < self.skip.end && b >= self.skip.start
    }

    fn group_starts_with_query(&self, open: usize) -> bool {
        self.toks
            .get(open + 1)
            .is_some_and(|t| t.is_kw("SELECT") || t.is_kw("WITH") || t.is_kw("VALUES"))
    }

    fn group_end(&self, open: usize) -> usize {
        self.close[open].unwrap_or(self.toks.len())
    }

    fn text(&self, a: usize, b: usize) -> String {
        let (s, e) = (self.toks[a].start, self.toks[b].end);
        self.sql.get(s..e).unwrap_or("").trim().to_string()
    }

    /// Nama-nama sederhana dipisah koma di dalam `(a, b, c)`.
    fn names_in(&self, from: usize, to: usize) -> Vec<String> {
        build_items(self.toks, from, to, self.close)
            .iter()
            .filter_map(|i| match i {
                Item::Name { parts, .. } => parts.last().cloned(),
                _ => None,
            })
            .collect()
    }

    /// Definisi CTE di awal region (`WITH a AS (...), b(x, y) AS (...)`).
    fn ctes(&self, from: usize, to: usize, known: &[ScopeTable]) -> Vec<ScopeTable> {
        let items = build_items(self.toks, from, to, self.close);
        let mut out: Vec<ScopeTable> = Vec::new();
        if !items.first().is_some_and(|i| i.is_kw("WITH")) {
            return out;
        }
        let mut k = 1;
        if items.get(k).is_some_and(|i| i.is_kw("RECURSIVE")) {
            k += 1;
        }
        while k < items.len() {
            let (name, idx, explicit_cols) = match &items[k] {
                Item::Name { parts, first, .. } => (
                    parts.last().cloned().unwrap_or_default(),
                    *first,
                    Vec::new(),
                ),
                Item::Call {
                    name,
                    name_idx,
                    open,
                    close: Some(c),
                } => (name.clone(), *name_idx, self.names_in(*open + 1, *c)),
                _ => break,
            };
            k += 1;
            if !items.get(k).is_some_and(|i| i.is_kw("AS")) {
                break;
            }
            k += 1;
            // `AS [NOT] MATERIALIZED (...)`
            while items.get(k).is_some_and(|i| {
                i.is_kw("NOT") || matches!(i, Item::Name { parts, .. } if parts[0].eq_ignore_ascii_case("MATERIALIZED"))
            }) {
                k += 1;
            }
            let Some(Item::Group { open, .. }) = items.get(k) else {
                break;
            };
            let body_end = self.group_end(*open);
            let mut visible: Vec<ScopeTable> = known.to_vec();
            visible.extend(out.iter().cloned());
            let (columns, star_from) = if explicit_cols.is_empty() {
                self.query_outputs(*open + 1, body_end, &visible)
            } else {
                (explicit_cols, Vec::new())
            };
            out.push(ScopeTable {
                schema: None,
                name,
                alias: None,
                kind: TableKind::Cte,
                depth: 0,
                idx,
                columns,
                star_from,
                dml_target: false,
            });
            k += 1;
            if items.get(k).is_some_and(|i| i.is_comma()) {
                k += 1;
            } else {
                break;
            }
        }
        out
    }

    /// Kolom output sebuah query (untuk CTE / derived table).
    fn query_outputs(
        &self,
        from: usize,
        to: usize,
        ctes: &[ScopeTable],
    ) -> (Vec<String>, Vec<ScopeTable>) {
        let mut cols = Vec::new();
        let mut star_from = Vec::new();
        let mut inner: Option<Vec<ScopeTable>> = None;
        for it in self.select_items(from, to) {
            if it.is_star {
                let tables = inner.get_or_insert_with(|| self.tables(from, to, 0, ctes));
                for t in tables.iter() {
                    let matches = match &it.qualifier {
                        Some(q) => {
                            t.display().eq_ignore_ascii_case(q) || t.name.eq_ignore_ascii_case(q)
                        }
                        None => true,
                    };
                    if matches {
                        star_from.push(t.clone());
                    }
                }
            } else if let Some(n) = it.name {
                cols.push(n);
            }
        }
        (cols, star_from)
    }

    /// Item SELECT dari query di region `[from, to)`.
    fn select_items(&self, from: usize, to: usize) -> Vec<SelectItem> {
        let items = build_items(self.toks, from, to, self.close);
        // Lewati WITH ... di depan: cari SELECT pertama di level ini.
        let Some(sel) = items.iter().position(|i| i.is_kw("SELECT")) else {
            return Vec::new();
        };
        let end = items[sel + 1..]
            .iter()
            .position(|i| {
                i.is_kw_any(&[
                    "FROM", "INTO", "WHERE", "GROUP", "ORDER", "LIMIT", "UNION", "HAVING",
                ])
            })
            .map(|p| sel + 1 + p)
            .unwrap_or(items.len());
        let mut out = Vec::new();
        for seg in items[sel + 1..end].split(|i| i.is_comma()) {
            let mut seg: Vec<&Item> = seg.iter().filter(|i| !self.skipped(i)).collect();
            while seg
                .first()
                .is_some_and(|i| i.is_kw_any(&["DISTINCT", "ALL"]))
            {
                seg.remove(0);
            }
            if seg.first().is_some_and(|i| i.is_kw("TOP")) {
                seg.drain(..seg.len().min(2));
            }
            if seg.is_empty() {
                continue;
            }
            let (a, _) = seg[0].span();
            let (_, b) = seg[seg.len() - 1].span();
            let aggregate = (a..=b).any(|k| {
                self.toks[k].kind == TokKind::Word
                    && AGGREGATES
                        .iter()
                        .any(|g| self.toks[k].text.eq_ignore_ascii_case(g))
                    && self
                        .toks
                        .get(k + 1)
                        .is_some_and(|t| t.kind == TokKind::LParen)
            });
            let n = seg.len();
            let alias = match seg.last() {
                Some(Item::Name { parts, .. }) if n >= 2 && parts.len() == 1 => {
                    let prev = seg[n - 2];
                    (prev.is_kw("AS") || prev.is_operand()).then(|| parts[0].clone())
                }
                _ => None,
            };
            let expr_end = match &alias {
                Some(_) if seg[n - 2].is_kw("AS") => n - 2,
                Some(_) => n - 1,
                None => n,
            };
            let mut item = SelectItem {
                name: None,
                qualifier: None,
                text: String::new(),
                aggregate,
                is_star: false,
                has_alias: alias.is_some(),
            };
            if expr_end > 0 {
                let (ea, _) = seg[0].span();
                let (_, eb) = seg[expr_end - 1].span();
                item.text = self.text(ea, eb);
            }
            if let Some(Item::Name { parts, .. }) = seg.first() {
                if parts.len() >= 2 {
                    item.qualifier = Some(parts[parts.len() - 2].clone());
                }
            }
            if alias.is_some() {
                item.name = alias;
            } else if n == 1 {
                match seg[0] {
                    Item::Name { parts, .. } if parts.last().is_some_and(|p| p == "*") => {
                        item.is_star = true
                    }
                    Item::Name { parts, .. } => item.name = parts.last().cloned(),
                    Item::Op { text, .. } if text == "*" => item.is_star = true,
                    _ => {}
                }
            }
            out.push(item);
        }
        out
    }

    /// Tabel yang dirujuk langsung oleh query di region `[from, to)`.
    fn tables(&self, from: usize, to: usize, depth: usize, ctes: &[ScopeTable]) -> Vec<ScopeTable> {
        let items = build_items(self.toks, from, to, self.close);
        let mut out: Vec<ScopeTable> = Vec::new();
        let mut expecting_table = false;
        let mut expecting_alias = false;
        let mut dml_next = false;
        let mut in_from = false;
        let mut prev_kw: Option<String> = None;
        let base = |schema: Option<String>, name: String, idx: usize, dml: bool| ScopeTable {
            schema,
            name,
            alias: None,
            kind: TableKind::Base,
            depth,
            idx,
            columns: Vec::new(),
            star_from: Vec::new(),
            dml_target: dml,
        };
        for it in &items {
            if self.skipped(it) {
                expecting_alias = false;
                continue;
            }
            if expecting_alias {
                match it {
                    Item::Kw { up, .. } if up == "AS" => continue,
                    Item::Name { parts, .. } if parts.len() == 1 => {
                        if let Some(t) = out.last_mut() {
                            t.alias = Some(parts[0].clone());
                        }
                        expecting_alias = false;
                        continue;
                    }
                    _ => expecting_alias = false,
                }
            }
            if expecting_table {
                match it {
                    Item::Kw { up, .. } if up == "LATERAL" || up == "ONLY" => continue,
                    Item::Name { parts, first, .. } if parts.last().is_some_and(|p| p != "*") => {
                        let name = parts.last().cloned().unwrap_or_default();
                        let schema = (parts.len() >= 2).then(|| parts[parts.len() - 2].clone());
                        let cte = schema
                            .is_none()
                            .then(|| ctes.iter().find(|c| c.name.eq_ignore_ascii_case(&name)))
                            .flatten();
                        out.push(match cte {
                            Some(c) => ScopeTable {
                                depth,
                                idx: *first,
                                alias: None,
                                dml_target: dml_next,
                                ..c.clone()
                            },
                            None => base(schema, name, *first, dml_next),
                        });
                        expecting_table = false;
                        expecting_alias = true;
                        dml_next = false;
                        continue;
                    }
                    // `INSERT INTO t (a, b)` → t adalah tabel target, bukan fungsi
                    Item::Call { name, name_idx, .. } if dml_next => {
                        out.push(base(None, name.clone(), *name_idx, true));
                        expecting_table = false;
                        dml_next = false;
                        continue;
                    }
                    Item::Group { open, .. } if self.group_starts_with_query(*open) => {
                        let end = self.group_end(*open);
                        let (columns, star_from) = self.query_outputs(*open + 1, end, ctes);
                        out.push(ScopeTable {
                            kind: TableKind::Derived,
                            columns,
                            star_from,
                            ..base(None, String::new(), *open, false)
                        });
                        expecting_table = false;
                        expecting_alias = true;
                        continue;
                    }
                    // Fungsi tabel, mis. generate_series(...) / UNNEST(...)
                    Item::Call { name, name_idx, .. } => {
                        out.push(ScopeTable {
                            kind: TableKind::Derived,
                            ..base(None, name.to_ascii_lowercase(), *name_idx, false)
                        });
                        expecting_table = false;
                        expecting_alias = true;
                        continue;
                    }
                    _ => expecting_table = false,
                }
            }
            match it {
                Item::Kw { up, .. } => {
                    match up.as_str() {
                        "FROM" => {
                            expecting_table = true;
                            in_from = true;
                            dml_next = prev_kw.as_deref() == Some("DELETE");
                        }
                        "JOIN" | "STRAIGHT_JOIN" => {
                            expecting_table = true;
                            in_from = true;
                        }
                        "UPDATE" => {
                            expecting_table = true;
                            dml_next = true;
                            in_from = true;
                        }
                        "INTO" if prev_kw.as_deref() == Some("INSERT") => {
                            expecting_table = true;
                            dml_next = true;
                        }
                        "ON" | "USING" | "LEFT" | "RIGHT" | "INNER" | "OUTER" | "FULL"
                        | "CROSS" | "NATURAL" | "AND" | "OR" | "NOT" | "AS" | "IS" | "NULL"
                        | "LIKE" | "IN" | "BETWEEN" => {}
                        _ => in_from = false,
                    }
                    prev_kw = Some(up.clone());
                }
                Item::Comma(_) if in_from => expecting_table = true,
                _ => {}
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Mesin kondisi: WHERE / ON / HAVING / SET / CASE WHEN.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Cond {
    Start,
    Lhs(Option<ColRef>),
    LhsArith,
    Op(Option<ColRef>),
    Rhs,
    Is,
    IsNot,
    Not,
    In,
    BetweenPending(Option<ColRef>),
    Between1(Option<ColRef>),
    Exists,
}

/// Cari END pasangan CASE di `seg[k]`; kembalikan index END.
fn matching_end(seg: &[&Item], k: usize) -> Option<usize> {
    let mut depth = 0;
    for (j, it) in seg.iter().enumerate().skip(k) {
        if it.is_kw("CASE") {
            depth += 1;
        } else if it.is_kw("END") {
            depth -= 1;
            if depth == 0 {
                return Some(j);
            }
        }
    }
    None
}

fn cond_state(seg: &[&Item]) -> Cond {
    let mut st = Cond::Start;
    let mut lhs_keep: Option<ColRef> = None;
    let mut k = 0;
    while k < seg.len() {
        let it = seg[k];
        if it.is_kw("CASE") {
            // CASE ... END lengkap diperlakukan sebagai satu operand
            let Some(e) = matching_end(seg, k) else {
                return Cond::Start;
            };
            k = e + 1;
            st = match st {
                Cond::Start | Cond::LhsArith => Cond::Lhs(None),
                Cond::Op(_) => Cond::Rhs,
                Cond::BetweenPending(l) => Cond::Between1(l),
                s => s,
            };
            continue;
        }
        st = match (st, it) {
            (Cond::Between1(l), i) if i.is_kw("AND") => Cond::Op(l),
            (_, i)
                if i.is_kw_any(&["AND", "OR", "WHERE", "ON", "HAVING", "WHEN", "THEN", "ELSE"]) =>
            {
                Cond::Start
            }
            (Cond::Start, i) if i.is_kw("NOT") => Cond::Start,
            (Cond::Start, i) if i.is_kw("EXISTS") => Cond::Exists,
            (Cond::Exists, Item::Group { .. }) => Cond::Rhs,
            (Cond::Lhs(_) | Cond::Rhs, i) if i.is_kw("IS") => Cond::Is,
            (Cond::Is, i) if i.is_kw("NOT") => Cond::IsNot,
            (Cond::Is | Cond::IsNot, i) if i.is_kw_any(&["NULL", "TRUE", "FALSE", "UNKNOWN"]) => {
                Cond::Rhs
            }
            (Cond::Lhs(l), i) if i.is_kw("NOT") => {
                lhs_keep = l;
                Cond::Not
            }
            (Cond::Lhs(l), i) if i.is_kw_any(&["LIKE", "ILIKE", "RLIKE", "REGEXP", "SIMILAR"]) => {
                Cond::Op(l)
            }
            (Cond::Not, i) if i.is_kw_any(&["LIKE", "ILIKE", "RLIKE", "REGEXP", "SIMILAR"]) => {
                Cond::Op(lhs_keep.take())
            }
            (Cond::Lhs(l), Item::Op { text, .. }) if CMP_OPS.contains(&text.as_str()) => {
                Cond::Op(l)
            }
            (Cond::Lhs(_) | Cond::Not, i) if i.is_kw("IN") => Cond::In,
            (Cond::In, Item::Group { .. }) => Cond::Rhs,
            (Cond::Lhs(l), i) if i.is_kw("BETWEEN") => Cond::BetweenPending(l),
            (Cond::Not, i) if i.is_kw("BETWEEN") => Cond::BetweenPending(lhs_keep.take()),
            (Cond::BetweenPending(l), i) if i.is_operand() => Cond::Between1(l),
            (Cond::Op(l), i) if i.is_kw_any(&["ANY", "ALL", "SOME"]) => Cond::Op(l),
            (Cond::Op(_), i) if i.is_operand() => Cond::Rhs,
            (Cond::Rhs, Item::Op { text, .. }) if ARITH_OPS.contains(&text.as_str()) => {
                Cond::Op(None)
            }
            (Cond::Lhs(_), Item::Op { text, .. }) if ARITH_OPS.contains(&text.as_str()) => {
                Cond::LhsArith
            }
            (Cond::LhsArith, i) if i.is_operand() => Cond::Lhs(None),
            (Cond::Start, i) if i.is_operand() => Cond::Lhs(i.col_ref()),
            (s, _) => s,
        };
        k += 1;
    }
    st
}

/// Jika segmen berada di dalam CASE yang belum ditutup, kembalikan
/// sub-segmen setelah WHEN/THEN/ELSE terakhir beserta keyword-nya.
fn open_case<'a>(seg: &[&'a Item]) -> Option<(Vec<&'a Item>, &'static str)> {
    let mut stack: Vec<(usize, Option<(usize, &'static str)>)> = Vec::new();
    for (k, it) in seg.iter().enumerate() {
        match it.kw() {
            Some("CASE") => stack.push((k, None)),
            Some("END") => {
                stack.pop();
            }
            Some(kw @ ("WHEN" | "THEN" | "ELSE")) => {
                if let Some(top) = stack.last_mut() {
                    let kw: &'static str = match kw {
                        "WHEN" => "WHEN",
                        "THEN" => "THEN",
                        _ => "ELSE",
                    };
                    top.1 = Some((k, kw));
                }
            }
            _ => {}
        }
    }
    let (case_at, last) = *stack.last()?;
    Some(match last {
        Some((k, kw)) => (seg[k + 1..].to_vec(), kw),
        None => (seg[case_at + 1..].to_vec(), "CASE"),
    })
}

pub(crate) const CASE_AFTER_VALUE: &[&str] = &["WHEN", "ELSE", "END"];
pub(crate) const CASE_AFTER_COND: &[&str] = &["THEN", "AND", "OR"];

fn case_expect(sub: &[&Item], kw: &str) -> (Expect, Option<ColRef>) {
    if kw == "CASE" {
        return (Expect::Keywords(&["WHEN"]), None);
    }
    match (kw, cond_state(sub)) {
        (_, Cond::Start) | (_, Cond::LhsArith) => (Expect::Column, None),
        (_, Cond::Op(l)) | (_, Cond::BetweenPending(l)) => (Expect::Value, l),
        ("WHEN", Cond::Lhs(l)) => (Expect::Operator, l),
        ("WHEN", _) => (Expect::Keywords(CASE_AFTER_COND), None),
        _ => (Expect::Keywords(CASE_AFTER_VALUE), None),
    }
}

pub(crate) const IS_TAIL: &[&str] = &["NULL", "NOT NULL", "TRUE", "FALSE", "DISTINCT FROM"];
pub(crate) const IS_NOT_TAIL: &[&str] = &["NULL", "TRUE", "FALSE", "DISTINCT FROM"];
pub(crate) const NOT_TAIL: &[&str] = &["IN", "LIKE", "BETWEEN", "ILIKE", "EXISTS"];

fn cond_expect(seg: &[&Item], start: Expect) -> (Expect, Option<ColRef>) {
    if let Some((sub, kw)) = open_case(seg) {
        return case_expect(&sub, kw);
    }
    match cond_state(seg) {
        Cond::Start => (start, None),
        Cond::Lhs(l) => (Expect::Operator, l),
        Cond::LhsArith => (Expect::Column, None),
        Cond::Op(l) | Cond::BetweenPending(l) => (Expect::Value, l),
        Cond::Rhs => (Expect::AfterExpr, None),
        Cond::Is => (Expect::Keywords(IS_TAIL), None),
        Cond::IsNot => (Expect::Keywords(IS_NOT_TAIL), None),
        Cond::Not => (Expect::Keywords(NOT_TAIL), None),
        Cond::Between1(_) => (Expect::Keywords(&["AND"]), None),
        Cond::In | Cond::Exists => (Expect::None, None),
    }
}

// ---------------------------------------------------------------------------
// Klausa.
// ---------------------------------------------------------------------------

/// Tentukan klausa aktif dari item query; kembalikan
/// (klausa, index item awal segmen, index token keyword klausa).
fn clause_scan(items: &[Item]) -> (Clause, usize, Option<usize>) {
    let mut clause = Clause::StatementStart;
    let mut start = 0;
    let mut kw_tok = None;
    for (k, it) in items.iter().enumerate() {
        let Item::Kw { up, idx } = it else { continue };
        let next = match up.as_str() {
            "SELECT" => Some(Clause::SelectList),
            "FROM" | "JOIN" | "STRAIGHT_JOIN" => Some(Clause::From),
            "ON" if clause == Clause::From => Some(Clause::JoinOn),
            "USING" if clause == Clause::From => Some(Clause::Using),
            "WHERE" => Some(Clause::Where),
            "GROUP" => Some(Clause::GroupPending),
            "ORDER" => Some(Clause::OrderPending),
            "BY" if clause == Clause::GroupPending => Some(Clause::GroupBy),
            "BY" if clause == Clause::OrderPending => Some(Clause::OrderBy),
            "HAVING" => Some(Clause::Having),
            "LIMIT" | "OFFSET" | "FETCH" => Some(Clause::Limit),
            "INSERT" if matches!(clause, Clause::StatementStart | Clause::With) => {
                Some(Clause::Insert)
            }
            "INTO" if clause == Clause::Insert => Some(Clause::InsertTarget),
            "INTO" => Some(Clause::Other),
            "VALUES" => Some(Clause::Values),
            "UPDATE" => Some(Clause::UpdateTarget),
            "SET" if matches!(clause, Clause::UpdateTarget | Clause::From | Clause::JoinOn) => {
                Some(Clause::UpdateSet)
            }
            "DELETE" => Some(Clause::Delete),
            "WITH" if k == 0 => Some(Clause::With),
            "UNION" | "INTERSECT" | "EXCEPT" | "MINUS" => Some(Clause::SetOp),
            "RETURNING" => Some(Clause::SelectList),
            "CREATE" | "ALTER" | "DROP" | "TRUNCATE" => Some(Clause::Other),
            _ => None,
        };
        if let Some(c) = next {
            clause = c;
            start = k + 1;
            kw_tok = Some(*idx);
        }
    }
    (clause, start, kw_tok)
}

pub(crate) const JOIN_TAIL: &[&str] = &["JOIN", "OUTER JOIN"];
pub(crate) const JOIN_ONLY: &[&str] = &["JOIN"];
pub(crate) const AFTER_GROUP_ITEM: &[&str] = &["HAVING", "ORDER BY", "LIMIT", "WITH ROLLUP"];
pub(crate) const AFTER_ORDER_ITEM: &[&str] = &[
    "ASC",
    "DESC",
    "NULLS FIRST",
    "NULLS LAST",
    "LIMIT",
    "OFFSET",
];
pub(crate) const AFTER_ORDER_DIR: &[&str] = &["NULLS FIRST", "NULLS LAST", "LIMIT", "OFFSET"];
pub(crate) const INSERT_AFTER_COLS: &[&str] = &["VALUES", "SELECT"];
pub(crate) const SET_OP_TAIL: &[&str] = &["ALL", "SELECT"];
pub(crate) const WITH_AFTER_CTE: &[&str] = &["SELECT", "INSERT INTO", "UPDATE", "DELETE FROM"];

fn from_expect(items: &[Item]) -> Expect {
    let b = last_index_where(items, |i| {
        i.is_kw_any(&["FROM", "JOIN", "STRAIGHT_JOIN"]) || i.is_comma()
    });
    let joined = b.is_some_and(|b| items[b].is_kw_any(&["JOIN", "STRAIGHT_JOIN"]));
    let seg = &items[b.map(|b| b + 1).unwrap_or(0)..];
    let Some(last) = seg.last() else {
        return Expect::Table;
    };
    if last.is_kw_any(&["LEFT", "RIGHT", "FULL"]) {
        return Expect::Keywords(JOIN_TAIL);
    }
    if last.is_kw_any(&["INNER", "CROSS", "OUTER", "NATURAL"]) {
        return Expect::Keywords(JOIN_ONLY);
    }
    if last.is_kw("AS") {
        return Expect::None;
    }
    if last.is_kw_any(&["LATERAL", "ONLY"]) {
        return Expect::Table;
    }
    Expect::AfterTable {
        joined,
        aliased: seg.len() >= 2,
    }
}

/// Segmen setelah koma terakhir.
fn after_last_comma(items: &[Item]) -> &[Item] {
    match last_index_where(items, |i| i.is_comma()) {
        Some(c) => &items[c + 1..],
        None => items,
    }
}

fn list_expect(seg: &[Item], clause: Clause) -> Expect {
    let Some(last) = seg.last() else {
        return Expect::Column;
    };
    if matches!(last, Item::Op { .. }) {
        return Expect::Column;
    }
    match clause {
        Clause::OrderBy if last.is_kw_any(&["ASC", "DESC"]) => Expect::Keywords(AFTER_ORDER_DIR),
        Clause::OrderBy if last.is_kw("NULLS") => Expect::Keywords(&["FIRST", "LAST"]),
        Clause::OrderBy => Expect::Keywords(AFTER_ORDER_ITEM),
        _ => Expect::Keywords(AFTER_GROUP_ITEM),
    }
}

fn select_expect(seg: &[&Item]) -> (Expect, Option<ColRef>) {
    let mut seg: Vec<&Item> = seg.to_vec();
    while seg
        .first()
        .is_some_and(|i| i.is_kw_any(&["DISTINCT", "ALL"]))
    {
        seg.remove(0);
    }
    if seg.first().is_some_and(|i| i.is_kw("TOP")) {
        if seg.len() == 1 {
            return (Expect::None, None);
        }
        seg.drain(..2);
    }
    if let Some((sub, kw)) = open_case(&seg) {
        return case_expect(&sub, kw);
    }
    let Some(last) = seg.last() else {
        return (Expect::Column, None);
    };
    if last.is_kw("AS") {
        return (Expect::None, None);
    }
    if let Item::Op { text, .. } = last {
        if !(text == "*" && seg.len() == 1) {
            return (Expect::Column, None);
        }
    }
    (Expect::AfterSelectItem, None)
}

fn with_expect(items: &[Item]) -> Expect {
    let b = last_index_where(items, |i| i.is_comma() || i.is_kw("WITH"));
    let mut seg = &items[b.map(|b| b + 1).unwrap_or(0)..];
    if seg.first().is_some_and(|i| i.is_kw("RECURSIVE")) {
        seg = &seg[1..];
    }
    match seg {
        [] => Expect::None,
        [Item::Name { .. }] | [Item::Call { .. }] => Expect::Keywords(&["AS"]),
        [.., last] if last.is_kw("AS") => Expect::None,
        [.., Item::Group { close: Some(_), .. }] => Expect::Keywords(WITH_AFTER_CTE),
        _ => Expect::None,
    }
}

// ---------------------------------------------------------------------------
// Entry point.
// ---------------------------------------------------------------------------

/// Jenis frame kurung di sekitar kursor (di dalam satu query).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameKind {
    Func,
    InsertCols,
    InList,
    ValuesRow,
    UsingCols,
    Window,
    SubqueryPending,
    Expr,
}

/// Analisis konteks di posisi `cursor` (offset byte) pada `sql`.
pub fn analyze(sql: &str, cursor: usize, dialect: Dialect) -> Analysis {
    let mut cursor = cursor.min(sql.len());
    while cursor > 0 && !sql.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let all = tokenize(sql, dialect);

    // Kursor di dalam string, komentar, angka, parameter, atau quote yang belum ditutup.
    for t in &all {
        let inside = match t.kind {
            TokKind::Str => t.start < cursor && (cursor < t.end || !t.terminated),
            TokKind::Comment => {
                let line = !sql[t.start..].starts_with("/*");
                t.start < cursor && (cursor < t.end || !t.terminated || (line && cursor <= t.end))
            }
            TokKind::Number | TokKind::Param => t.start < cursor && cursor <= t.end,
            TokKind::QuotedIdent => !t.terminated && t.start < cursor,
            _ => false,
        };
        if inside {
            return Analysis::none();
        }
    }

    let toks_all: Vec<Token> = all
        .into_iter()
        .filter(|t| t.kind != TokKind::Comment)
        .collect();
    // Batasi ke statement aktif (dipisah `;`)
    let mut s0 = 0;
    let mut s1 = toks_all.len();
    for (i, t) in toks_all.iter().enumerate() {
        if t.kind == TokKind::Semicolon {
            if t.end <= cursor {
                s0 = i + 1;
            } else {
                s1 = i;
                break;
            }
        }
    }
    let toks: Vec<Token> = toks_all[s0..s1].to_vec();

    // Kata yang sedang diketik + rantai qualifier
    let mut anchor = toks
        .iter()
        .position(|t| t.start >= cursor)
        .unwrap_or(toks.len());
    let mut partial = String::new();
    let mut has_partial = false;
    if anchor > 0 {
        let t = &toks[anchor - 1];
        if t.start < cursor && t.end >= cursor {
            match t.kind {
                TokKind::Word => {
                    partial = sql[t.start..cursor].to_string();
                    anchor -= 1;
                    has_partial = true;
                }
                TokKind::QuotedIdent if t.end == cursor => {
                    partial = t.text.clone();
                    anchor -= 1;
                    has_partial = true;
                }
                _ => {}
            }
        }
    }
    let mut chain_start = anchor;
    let mut qualifier: Vec<String> = Vec::new();
    while chain_start >= 2
        && toks[chain_start - 1].kind == TokKind::Dot
        && matches!(
            toks[chain_start - 2].kind,
            TokKind::Word | TokKind::QuotedIdent
        )
    {
        qualifier.insert(0, toks[chain_start - 2].text.clone());
        chain_start -= 2;
    }
    let skip_end = anchor + usize::from(has_partial);
    let close = match_parens(&toks);
    let ctx = Ctx {
        sql,
        toks: &toks,
        close: &close,
        skip: chain_start..skip_end,
    };

    // Kurung yang membungkus kursor
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in toks[..chain_start].iter().enumerate() {
        match t.kind {
            TokKind::LParen => stack.push(i),
            TokKind::RParen => {
                stack.pop();
            }
            _ => {}
        }
    }
    let is_query_open = |j: usize| j + 1 < chain_start && ctx.group_starts_with_query(j);
    // Region query dari dalam ke luar: (open, start, end)
    let mut regions: Vec<(Option<usize>, usize, usize)> = stack
        .iter()
        .rev()
        .filter(|&&j| is_query_open(j))
        .map(|&j| (Some(j), j + 1, ctx.group_end(j)))
        .collect();
    regions.push((None, 0, toks.len()));
    let (q_open, qs, qe) = regions[0];

    // CTE: dari region terluar ke dalam supaya definisi dalam menimpa yang luar
    let mut ctes: Vec<ScopeTable> = Vec::new();
    for &(_, rs, re) in regions.iter().rev() {
        for c in ctx.ctes(rs, re, &ctes) {
            ctes.retain(|x| !x.name.eq_ignore_ascii_case(&c.name));
            ctes.push(c);
        }
    }

    // Scope: query saat ini (depth 0) lalu query luar
    let mut scope: Vec<ScopeTable> = Vec::new();
    for (depth, &(_, rs, re)) in regions.iter().enumerate() {
        scope.extend(ctx.tables(rs, re, depth, &ctes));
    }

    let mut a = Analysis::none();
    a.partial = partial;
    a.qualifier = qualifier;
    a.cursor_tok = chain_start;
    a.ctes = ctes;
    a.select_items = ctx.select_items(qs, qe);
    a.dml_target = scope.iter().find(|t| t.depth == 0 && t.dml_target).cloned();

    // Frame kurung non-query di dalam query aktif
    let frames: Vec<usize> = stack
        .iter()
        .copied()
        .filter(|&j| q_open.is_none_or(|q| j > q))
        .collect();
    let clause_end = frames.first().copied().unwrap_or(chain_start);
    let q_items = build_items(&toks, qs, clause_end, &close);
    let (clause, seg_start, clause_kw_tok) = clause_scan(&q_items);
    a.clause = clause;
    let clause_items = &q_items[seg_start.min(q_items.len())..];

    if clause == Clause::JoinOn {
        let on_tok = clause_kw_tok.unwrap_or(0);
        a.join_target = scope
            .iter()
            .filter(|t| t.depth == 0 && t.idx < on_tok)
            .max_by_key(|t| t.idx)
            .cloned();
    }
    a.scope = scope;

    if let Some(&f) = frames.last() {
        analyze_frame(&mut a, &ctx, f, &frames, qs, chain_start, &q_items);
        return a;
    }

    // --- Kursor langsung di level query ---
    let refs: Vec<&Item> = clause_items.iter().collect();
    let (expect, lhs) = match clause {
        Clause::StatementStart if clause_items.is_empty() => (Expect::StatementStart, None),
        Clause::StatementStart => (Expect::Generic, None),
        Clause::SelectList => {
            a.right_after_select = clause_items.is_empty();
            a.used_columns = a
                .select_items
                .iter()
                .filter(|s| !s.has_alias)
                .filter_map(|s| s.name.as_ref().map(|n| n.to_ascii_lowercase()))
                .collect();
            let seg: Vec<&Item> = after_last_comma(clause_items).iter().collect();
            select_expect(&seg)
        }
        Clause::From => {
            a.join_pending = last_index_where(&q_items, |i| {
                i.is_kw_any(&["FROM", "JOIN", "STRAIGHT_JOIN"]) || i.is_comma()
            })
            .is_some_and(|b| q_items[b].is_kw_any(&["JOIN", "STRAIGHT_JOIN"]));
            (from_expect(&q_items), None)
        }
        Clause::JoinOn => cond_expect(&refs, Expect::JoinCondition),
        Clause::Where | Clause::Having => cond_expect(&refs, Expect::Column),
        Clause::GroupPending | Clause::OrderPending if clause_items.is_empty() => {
            (Expect::Keywords(&["BY"]), None)
        }
        Clause::GroupBy | Clause::OrderBy => {
            a.used_columns = names_of(clause_items);
            (list_expect(after_last_comma(clause_items), clause), None)
        }
        Clause::Limit if !clause_items.is_empty() => (Expect::Keywords(&["OFFSET"]), None),
        Clause::Insert if clause_items.is_empty() => (Expect::Keywords(&["INTO"]), None),
        Clause::InsertTarget => match clause_items {
            [] => (Expect::Table, None),
            [Item::Name { .. }] => (
                Expect::AfterTable {
                    joined: false,
                    aliased: false,
                },
                None,
            ),
            [Item::Call { .. }] => (Expect::Keywords(INSERT_AFTER_COLS), None),
            _ => (Expect::None, None),
        },
        Clause::UpdateTarget => match clause_items {
            [] => (Expect::Table, None),
            [Item::Name { .. }] => (
                Expect::AfterTable {
                    joined: false,
                    aliased: false,
                },
                None,
            ),
            [Item::Name { .. }, ..] => (Expect::Keywords(&["SET"]), None),
            _ => (Expect::None, None),
        },
        Clause::UpdateSet => {
            a.used_columns = clause_items
                .split(|i| i.is_comma())
                .filter_map(|s| match s.first() {
                    Some(Item::Name { parts, .. }) => parts.last().map(|p| p.to_ascii_lowercase()),
                    _ => None,
                })
                .collect();
            let seg: Vec<&Item> = after_last_comma(clause_items).iter().collect();
            match cond_state(&seg) {
                Cond::Start => (Expect::Column, None),
                Cond::Lhs(_) => (Expect::Keywords(&["="]), None),
                Cond::Op(l) => (Expect::Value, l),
                Cond::Rhs => (Expect::AfterExpr, None),
                _ => (Expect::None, None),
            }
        }
        Clause::Delete if clause_items.is_empty() => (Expect::Keywords(&["FROM"]), None),
        Clause::With => (with_expect(&q_items), None),
        Clause::SetOp => match clause_items {
            [] => (Expect::Keywords(SET_OP_TAIL), None),
            [i] if i.is_kw("ALL") => (Expect::Keywords(&["SELECT"]), None),
            _ => (Expect::None, None),
        },
        Clause::Other | Clause::InsertColumns => (Expect::Generic, None),
        _ => (Expect::None, None),
    };
    a.expect = expect;
    a.lhs = lhs;
    a
}

/// Analisis bila kursor berada di dalam kurung non-query (argumen fungsi,
/// daftar kolom INSERT, IN (...), VALUES (...), dsb.).
fn analyze_frame(
    a: &mut Analysis,
    ctx: &Ctx,
    f: usize,
    frames: &[usize],
    qs: usize,
    chain_start: usize,
    q_items: &[Item],
) {
    let toks = ctx.toks;
    let parent_start = if frames.len() >= 2 {
        frames[frames.len() - 2] + 1
    } else {
        qs
    };
    let parent_items = build_items(toks, parent_start, f, ctx.close);
    let frame_items = build_items(toks, f + 1, chain_start, ctx.close);
    let prev_tok = f.checked_sub(1).map(|p| &toks[p]);
    let clause = a.clause;
    let kind = match prev_tok {
        Some(p) if p.is_kw("IN") => FrameKind::InList,
        Some(p) if p.is_kw("VALUES") => FrameKind::ValuesRow,
        Some(p) if p.kind == TokKind::Comma && clause == Clause::Values => FrameKind::ValuesRow,
        Some(p) if p.is_kw("USING") => FrameKind::UsingCols,
        Some(p) if p.is_kw("OVER") => FrameKind::Window,
        Some(p)
            if matches!(p.kind, TokKind::Word | TokKind::QuotedIdent)
                && clause == Clause::InsertTarget =>
        {
            FrameKind::InsertCols
        }
        Some(p) if p.kind == TokKind::Word && !is_reserved(&p.text) => FrameKind::Func,
        Some(p) if p.kind == TokKind::QuotedIdent => FrameKind::Func,
        Some(p)
            if [
                "FROM", "JOIN", "EXISTS", "AS", "ANY", "ALL", "SOME", "LATERAL",
            ]
            .iter()
            .any(|k| p.is_kw(k)) =>
        {
            FrameKind::SubqueryPending
        }
        _ => FrameKind::Expr,
    };
    let seg = after_last_comma(&frame_items);
    match kind {
        FrameKind::Func => {
            a.in_function = prev_tok.map(|p| p.text.to_ascii_uppercase());
            let mut s: Vec<&Item> = seg.iter().collect();
            while s.first().is_some_and(|i| i.is_kw_any(&["DISTINCT", "ALL"])) {
                s.remove(0);
            }
            let (e, l) = cond_expect(&s, Expect::Column);
            a.expect = match e {
                Expect::Column | Expect::Value | Expect::Keywords(_) => e,
                _ => Expect::None,
            };
            a.lhs = l;
        }
        FrameKind::InsertCols => {
            a.clause = Clause::InsertColumns;
            a.used_columns = ctx
                .names_in(f + 1, chain_start)
                .iter()
                .map(|s| s.to_ascii_lowercase())
                .collect();
            a.expect = if seg.is_empty() {
                Expect::Column
            } else {
                Expect::None
            };
        }
        FrameKind::InList => {
            if seg.is_empty() {
                a.expect = Expect::Value;
                a.allow_subquery = frame_items.is_empty();
                // operand kiri: <kolom> [NOT] IN (
                let mut k = parent_items.len();
                if k > 0 && parent_items[k - 1].is_kw("IN") {
                    k -= 1;
                    if k > 0 && parent_items[k - 1].is_kw("NOT") {
                        k -= 1;
                    }
                    if k > 0 {
                        a.lhs = parent_items[k - 1].col_ref();
                    }
                }
            }
        }
        FrameKind::ValuesRow => {
            a.clause = Clause::Values;
            if seg.is_empty() {
                a.expect = Expect::Value;
                let pos = frame_items.iter().filter(|i| i.is_comma()).count();
                let cols = explicit_insert_columns(ctx, q_items);
                match cols.get(pos) {
                    Some(c) => {
                        a.lhs = Some(ColRef {
                            qualifier: None,
                            column: c.clone(),
                        })
                    }
                    None if cols.is_empty() => a.value_index = Some(pos),
                    None => {}
                }
            }
        }
        FrameKind::UsingCols => {
            a.expect = if seg.is_empty() {
                Expect::Column
            } else {
                Expect::None
            };
        }
        FrameKind::Window => {
            a.expect = match frame_items.last() {
                None => Expect::Keywords(&["PARTITION BY", "ORDER BY"]),
                Some(i) if i.is_kw_any(&["PARTITION", "ORDER"]) => Expect::Keywords(&["BY"]),
                Some(i) if i.is_kw("BY") || i.is_comma() => Expect::Column,
                Some(_) => Expect::Keywords(&["ORDER BY", "ASC", "DESC", "ROWS BETWEEN"]),
            };
        }
        FrameKind::SubqueryPending => {
            if frame_items.is_empty() {
                a.expect = Expect::Keywords(&["SELECT"]);
            }
        }
        FrameKind::Expr => {
            // Ekspresi berkurung: mesin kondisi dengan klausa induk
            let start = if clause == Clause::JoinOn {
                Expect::JoinCondition
            } else {
                Expect::Column
            };
            let refs: Vec<&Item> = frame_items.iter().collect();
            let (e, l) = cond_expect(&refs, start);
            a.allow_subquery = frame_items.is_empty();
            a.expect = match e {
                Expect::AfterExpr => Expect::Keywords(&["AND", "OR"]),
                other => other,
            };
            a.lhs = l;
        }
    }
}

/// Kolom eksplisit pada `INSERT INTO t (a, b, c)`.
fn explicit_insert_columns(ctx: &Ctx, q_items: &[Item]) -> Vec<String> {
    let Some(into) = q_items.iter().position(|i| i.is_kw("INTO")) else {
        return Vec::new();
    };
    match q_items.get(into + 1) {
        Some(Item::Call {
            open,
            close: Some(c),
            ..
        }) => ctx.names_in(*open + 1, *c),
        _ => Vec::new(),
    }
}

fn names_of(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Name { parts, .. } => parts.last().map(|p| p.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Analisis dengan `|` sebagai penanda posisi kursor.
    fn at(sql_with_cursor: &str) -> Analysis {
        let cursor = sql_with_cursor.find('|').expect("penanda kursor");
        let sql = sql_with_cursor.replacen('|', "", 1);
        analyze(&sql, cursor, Dialect::Generic)
    }

    fn scope_names(a: &Analysis) -> Vec<(String, Option<String>, usize)> {
        a.scope
            .iter()
            .map(|t| (t.name.clone(), t.alias.clone(), t.depth))
            .collect()
    }

    #[test]
    fn statement_start() {
        assert_eq!(at("|").expect, Expect::StatementStart);
        assert_eq!(at("sel|").expect, Expect::StatementStart);
        assert_eq!(at("sel|").partial, "sel");
    }

    #[test]
    fn select_list_positions() {
        assert_eq!(at("SELECT |").expect, Expect::Column);
        assert!(at("SELECT |").right_after_select);
        assert_eq!(at("SELECT a, |").expect, Expect::Column);
        assert_eq!(at("SELECT a |").expect, Expect::AfterSelectItem);
        assert_eq!(at("SELECT a fr|").expect, Expect::AfterSelectItem);
        assert_eq!(at("SELECT a AS |").expect, Expect::None);
        assert_eq!(at("SELECT a + |").expect, Expect::Column);
        assert_eq!(at("SELECT DISTINCT |").expect, Expect::Column);
    }

    #[test]
    fn from_and_join_positions() {
        assert_eq!(at("SELECT * FROM |").expect, Expect::Table);
        assert_eq!(at("SELECT * FROM us|").expect, Expect::Table);
        assert_eq!(
            at("SELECT * FROM users |").expect,
            Expect::AfterTable {
                joined: false,
                aliased: false
            }
        );
        assert_eq!(
            at("SELECT * FROM users u |").expect,
            Expect::AfterTable {
                joined: false,
                aliased: true
            }
        );
        assert_eq!(
            at("SELECT * FROM users u LEFT |").expect,
            Expect::Keywords(JOIN_TAIL)
        );
        assert_eq!(at("SELECT * FROM users u JOIN |").expect, Expect::Table);
        assert_eq!(
            at("SELECT * FROM users u JOIN orders o |").expect,
            Expect::AfterTable {
                joined: true,
                aliased: true
            }
        );
        assert_eq!(at("SELECT * FROM a, |").expect, Expect::Table);
        assert_eq!(at("SELECT * FROM users AS |").expect, Expect::None);
    }

    #[test]
    fn join_on_positions() {
        let a = at("SELECT * FROM users u JOIN orders o ON |");
        assert_eq!(a.expect, Expect::JoinCondition);
        assert_eq!(a.clause, Clause::JoinOn);
        assert_eq!(
            a.join_target.as_ref().map(|t| t.name.as_str()),
            Some("orders")
        );
        assert_eq!(
            at("SELECT * FROM users u JOIN orders o ON o.user_id |").expect,
            Expect::Operator
        );
        assert_eq!(
            at("SELECT * FROM users u JOIN orders o ON o.user_id = |").expect,
            Expect::Value
        );
        assert_eq!(
            at("SELECT * FROM users u JOIN orders o ON o.user_id = u.id |").expect,
            Expect::AfterExpr
        );
        assert_eq!(
            at("SELECT * FROM users u JOIN orders o ON o.a = u.b AND |").expect,
            Expect::JoinCondition
        );
        // JOIN berikutnya menggeser join_target
        let a = at("SELECT * FROM a JOIN b ON a.x = b.x JOIN c ON |");
        assert_eq!(a.join_target.as_ref().map(|t| t.name.as_str()), Some("c"));
    }

    #[test]
    fn where_positions() {
        assert_eq!(at("SELECT * FROM t WHERE |").expect, Expect::Column);
        let a = at("SELECT * FROM t WHERE t.name |");
        assert_eq!(a.expect, Expect::Operator);
        assert_eq!(
            a.lhs,
            Some(ColRef {
                qualifier: Some("t".into()),
                column: "name".into()
            })
        );
        let a = at("SELECT * FROM t WHERE name = |");
        assert_eq!(a.expect, Expect::Value);
        assert_eq!(a.lhs.as_ref().map(|l| l.column.as_str()), Some("name"));
        assert_eq!(
            at("SELECT * FROM t WHERE a = 1 |").expect,
            Expect::AfterExpr
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a = 1 AND |").expect,
            Expect::Column
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a IS |").expect,
            Expect::Keywords(IS_TAIL)
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a NOT |").expect,
            Expect::Keywords(NOT_TAIL)
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a BETWEEN 1 |").expect,
            Expect::Keywords(&["AND"])
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a BETWEEN 1 AND |").expect,
            Expect::Value
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a BETWEEN 1 AND 2 |").expect,
            Expect::AfterExpr
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a IN (1, 2) |").expect,
            Expect::AfterExpr
        );
        assert_eq!(
            at("SELECT * FROM t WHERE a NOT LIKE |").expect,
            Expect::Value
        );
        // string/komentar diabaikan
        assert_eq!(
            at("SELECT * FROM t WHERE a = 'x' -- AND\n AND |").expect,
            Expect::Column
        );
        assert_eq!(at("SELECT * FROM t WHERE a = 'ab|c'").expect, Expect::None);
        assert_eq!(at("SELECT * FROM t -- komentar |").expect, Expect::None);
    }

    #[test]
    fn in_list_and_subqueries() {
        let a = at("SELECT * FROM t WHERE status IN (|");
        assert_eq!(a.expect, Expect::Value);
        assert!(a.allow_subquery);
        assert_eq!(a.lhs.as_ref().map(|l| l.column.as_str()), Some("status"));
        assert_eq!(
            at("SELECT * FROM t WHERE EXISTS (|").expect,
            Expect::Keywords(&["SELECT"])
        );
        // subquery punya klausa sendiri; scope luar tetap terlihat (depth 1)
        let a = at("SELECT * FROM users u WHERE u.id IN (SELECT o.user_id FROM orders o WHERE |)");
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(
            scope_names(&a),
            vec![
                ("orders".into(), Some("o".into()), 0),
                ("users".into(), Some("u".into()), 1)
            ]
        );
        // setelah subquery ditutup, klausa luar kembali
        assert_eq!(
            at("SELECT * FROM (SELECT x FROM t) sub WHERE |").expect,
            Expect::Column
        );
    }

    #[test]
    fn group_order_limit() {
        assert_eq!(
            at("SELECT a FROM t GROUP |").expect,
            Expect::Keywords(&["BY"])
        );
        assert_eq!(at("SELECT a FROM t GROUP BY |").expect, Expect::Column);
        assert_eq!(
            at("SELECT a FROM t GROUP BY a |").expect,
            Expect::Keywords(AFTER_GROUP_ITEM)
        );
        assert_eq!(
            at("SELECT a FROM t ORDER BY a |").expect,
            Expect::Keywords(AFTER_ORDER_ITEM)
        );
        assert_eq!(
            at("SELECT a FROM t ORDER BY a DESC |").expect,
            Expect::Keywords(AFTER_ORDER_DIR)
        );
        assert_eq!(at("SELECT a FROM t ORDER BY a, |").expect, Expect::Column);
        assert_eq!(at("SELECT a FROM t LIMIT 10|").expect, Expect::None);
        assert_eq!(
            at("SELECT a FROM t LIMIT 10 |").expect,
            Expect::Keywords(&["OFFSET"])
        );
    }

    #[test]
    fn scope_aliases_after_cursor() {
        // tabel yang ditulis setelah kursor tetap masuk scope
        let a = at("SELECT | FROM users u JOIN orders AS o ON o.user_id = u.id");
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(
            scope_names(&a),
            vec![
                ("users".into(), Some("u".into()), 0),
                ("orders".into(), Some("o".into()), 0)
            ]
        );
        // kata yang sedang diketik tidak dianggap tabel
        assert!(at("SELECT * FROM us|").scope.is_empty());
        // schema.table dan comma join
        let a = at("SELECT * FROM public.users u, orders WHERE |");
        assert_eq!(a.scope[0].schema.as_deref(), Some("public"));
        assert_eq!(a.scope[1].name, "orders");
    }

    #[test]
    fn qualifier_chain() {
        let a = at("SELECT u.na| FROM users u");
        assert_eq!(a.qualifier, vec!["u".to_string()]);
        assert_eq!(a.partial, "na");
        assert_eq!(a.expect, Expect::Column);
        let a = at("SELECT * FROM users u WHERE u.|");
        assert_eq!(a.qualifier, vec!["u".to_string()]);
        assert_eq!(a.expect, Expect::Column);
        let a = at("SELECT * FROM public.|");
        assert_eq!(a.expect, Expect::Table);
        assert_eq!(a.qualifier, vec!["public".to_string()]);
    }

    #[test]
    fn cte_and_derived_tables() {
        let a = at("WITH act AS (SELECT id, name AS nm, count(*) c FROM users) SELECT | FROM act");
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(a.scope[0].kind, TableKind::Cte);
        assert_eq!(a.scope[0].columns, vec!["id", "nm", "c"]);
        let a = at("SELECT d.| FROM (SELECT u.*, 1 AS one FROM users u) d");
        assert_eq!(a.scope[0].kind, TableKind::Derived);
        assert_eq!(a.scope[0].alias.as_deref(), Some("d"));
        assert_eq!(a.scope[0].columns, vec!["one"]);
        assert_eq!(a.scope[0].star_from[0].name, "users");
        assert_eq!(at("WITH x |").expect, Expect::Keywords(&["AS"]));
        assert_eq!(
            at("WITH x AS (SELECT 1) |").expect,
            Expect::Keywords(WITH_AFTER_CTE)
        );
        let a = at("WITH x(a, b) AS (SELECT 1, 2) SELECT | FROM x");
        assert_eq!(a.scope[0].columns, vec!["a", "b"]);
    }

    #[test]
    fn insert_positions() {
        assert_eq!(at("INSERT |").expect, Expect::Keywords(&["INTO"]));
        assert_eq!(at("INSERT INTO |").expect, Expect::Table);
        assert_eq!(
            at("INSERT INTO users |").expect,
            Expect::AfterTable {
                joined: false,
                aliased: false
            }
        );
        let a = at("INSERT INTO users (id, |");
        assert_eq!(a.clause, Clause::InsertColumns);
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(a.used_columns, vec!["id"]);
        assert_eq!(
            a.dml_target.as_ref().map(|t| t.name.as_str()),
            Some("users")
        );
        assert_eq!(
            at("INSERT INTO users (id, name) |").expect,
            Expect::Keywords(INSERT_AFTER_COLS)
        );
        let a = at("INSERT INTO users (id, name) VALUES (1, |");
        assert_eq!(a.expect, Expect::Value);
        assert_eq!(a.lhs.as_ref().map(|l| l.column.as_str()), Some("name"));
        assert_eq!(at("INSERT INTO users VALUES (1, 2, |").value_index, Some(2));
    }

    #[test]
    fn update_delete_positions() {
        assert_eq!(at("UPDATE |").expect, Expect::Table);
        assert_eq!(
            at("UPDATE users |").expect,
            Expect::AfterTable {
                joined: false,
                aliased: false
            }
        );
        let a = at("UPDATE users SET |");
        assert_eq!(a.clause, Clause::UpdateSet);
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(
            a.dml_target.as_ref().map(|t| t.name.as_str()),
            Some("users")
        );
        assert_eq!(
            at("UPDATE users SET name |").expect,
            Expect::Keywords(&["="])
        );
        let a = at("UPDATE users SET name = 'x', |");
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(a.used_columns, vec!["name"]);
        assert_eq!(
            at("UPDATE users SET name = 'x' |").expect,
            Expect::AfterExpr
        );
        assert_eq!(at("DELETE |").expect, Expect::Keywords(&["FROM"]));
        let a = at("DELETE FROM users WHERE |");
        assert_eq!(a.expect, Expect::Column);
        assert!(a.dml_target.is_some());
    }

    #[test]
    fn case_and_functions() {
        assert_eq!(at("SELECT CASE |").expect, Expect::Keywords(&["WHEN"]));
        assert_eq!(at("SELECT CASE WHEN |").expect, Expect::Column);
        assert_eq!(at("SELECT CASE WHEN a |").expect, Expect::Operator);
        assert_eq!(
            at("SELECT CASE WHEN a = 1 |").expect,
            Expect::Keywords(CASE_AFTER_COND)
        );
        assert_eq!(
            at("SELECT CASE WHEN a = 1 THEN 'x' |").expect,
            Expect::Keywords(CASE_AFTER_VALUE)
        );
        assert_eq!(
            at("SELECT CASE WHEN a = 1 THEN 'x' END |").expect,
            Expect::AfterSelectItem
        );
        let a = at("SELECT count(|");
        assert_eq!(a.expect, Expect::Column);
        assert_eq!(a.in_function.as_deref(), Some("COUNT"));
        assert_eq!(at("SELECT coalesce(a, |").expect, Expect::Column);
        assert_eq!(
            at("SELECT * FROM t WHERE lower(name) |").expect,
            Expect::Operator
        );
        assert_eq!(
            at("SELECT * FROM t WHERE (a = 1 OR |").expect,
            Expect::Column
        );
    }

    #[test]
    fn select_items_and_statement_isolation() {
        let a = at("SELECT u.name, count(*) AS total FROM users u GROUP BY |");
        assert_eq!(a.select_items.len(), 2);
        assert_eq!(a.select_items[0].name.as_deref(), Some("name"));
        assert_eq!(a.select_items[0].text, "u.name");
        assert!(!a.select_items[0].aggregate);
        assert_eq!(a.select_items[1].name.as_deref(), Some("total"));
        assert!(a.select_items[1].aggregate);
        // statement lain tidak mencemari scope
        let a = at("SELECT * FROM a; SELECT * FROM b WHERE |; SELECT * FROM c");
        assert_eq!(scope_names(&a), vec![("b".into(), None, 0)]);
    }
}
