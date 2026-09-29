//! Analisis performa query berbasis index, tanpa menjalankan query.
//!
//! Untuk tiap statement (termasuk subquery) dikumpulkan kolom yang dipakai di
//! JOIN, WHERE, GROUP BY, dan ORDER BY, di-resolve ke tabel lewat scope
//! analyzer, lalu dicocokkan dengan index yang diketahui (kolom pertama index,
//! termasuk primary key). Hasilnya rekomendasi yang bisa langsung dipakai:
//! index komposit berurutan equality, sort, lalu range, index untuk kolom FK,
//! dan pola yang membuat index tidak terpakai (fungsi pada kolom, `LIKE '%…'`).
//!
//! Prinsip presisi: tabel yang metadata index-nya belum diketahui dilewati,
//! kolom yang tidak ada di tabel (bila daftar kolom diketahui) diabaikan.

use std::collections::HashSet;
use std::ops::Range;

use super::analyzer::{ScopeTable, TableKind, analyze, is_reserved};
use super::lexer::{Dialect, TokKind, Token, tokenize};
use crate::models::structs::ForeignKey;

/// Satu index tabel; `columns` berurutan sesuai definisi index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

/// Sumber metadata untuk advisor.
pub trait IndexCatalog {
    /// Index tabel; `None` bila belum diketahui (tabel dilewati, bukan ditebak).
    fn indexes(&self, table: &str) -> Option<&[IndexDef]>;
    /// Kolom tabel bila diketahui; dipakai untuk menolak referensi yang salah.
    fn columns(&self, _table: &str) -> Option<&[String]> {
        None
    }
    fn foreign_keys(&self) -> &[ForeignKey];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdviceLevel {
    Info,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexAdvice {
    pub level: AdviceLevel,
    pub table: String,
    pub message: String,
    pub hint: Option<String>,
    /// SQL siap pakai (mis. `CREATE INDEX ...`), bila ada.
    pub ddl: Option<String>,
    /// Rentang byte di teks asli yang memicu rekomendasi.
    pub span: Option<Range<usize>>,
}

/// Status index sebuah kolom yang dipakai query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColumnStatus {
    /// Kolom pertama primary key.
    PrimaryKey,
    /// Kolom pertama index `index` (bisa dipakai langsung).
    Indexed { index: String, unique: bool },
    /// Ada di index `index`, tetapi bukan kolom pertama: tidak bisa dipakai sendirian.
    NotLeading { index: String, position: usize },
    /// Foreign key ke `references`, tanpa index yang diawali kolom ini.
    ForeignKeyNoIndex { references: String },
    /// Tidak ada index sama sekali untuk kolom ini.
    NotIndexed,
    /// Metadata index tabel belum diketahui.
    Unknown,
}

impl ColumnStatus {
    /// Kolom bisa dilayani index.
    pub fn is_good(&self) -> bool {
        matches!(self, ColumnStatus::PrimaryKey | ColumnStatus::Indexed { .. })
    }
}

/// Satu pemakaian kolom di query beserta status index-nya.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnUsage {
    pub table: String,
    pub column: String,
    /// Tempat kolom dipakai: `JOIN`, `WHERE =`, `WHERE range`, `GROUP BY`, `ORDER BY`.
    pub used_in: &'static str,
    pub status: ColumnStatus,
    /// Kolom dibungkus fungsi / `LIKE '%…'` sehingga index tidak terpakai.
    pub defeated_by: Option<String>,
    pub span: Range<usize>,
}

/// Hasil analisis lengkap satu teks query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueryReport {
    pub usages: Vec<ColumnUsage>,
    pub advice: Vec<IndexAdvice>,
    /// Tabel yang metadata index-nya belum ada di cache (tidak dianalisis).
    pub missing_metadata: Vec<String>,
}

impl QueryReport {
    /// Tidak ada peringatan performa.
    pub fn is_optimal(&self) -> bool {
        !self.advice.iter().any(|a| a.level == AdviceLevel::Warning)
    }

    /// Ada sesuatu yang bisa ditampilkan.
    pub fn is_empty(&self) -> bool {
        self.usages.is_empty() && self.advice.is_empty() && self.missing_metadata.is_empty()
    }

    fn push_usage(&mut self, u: ColumnUsage) {
        let dup = self.usages.iter().any(|x| {
            x.table.eq_ignore_ascii_case(&u.table)
                && x.column.eq_ignore_ascii_case(&u.column)
                && x.used_in == u.used_in
        });
        if !dup {
            self.usages.push(u);
        }
    }
}

/// Nama index yang lazim dipakai untuk primary key di tiap database.
fn is_primary_index(ix: &IndexDef) -> bool {
    let n = ix.name.to_ascii_lowercase();
    n == "primary" || n.ends_with("_pkey") || n.starts_with("pk_") || n.starts_with("pk__")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Eq,
    Range,
    Group,
    Order,
}

#[derive(Clone, Debug)]
struct ColUse {
    table: ScopeTable,
    column: String,
    role: Role,
    span: Range<usize>,
}

#[derive(Clone, Debug)]
struct JoinUse {
    left: (ScopeTable, String),
    right: (ScopeTable, String),
    span: Range<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Cond,
    Group,
    Order,
}

/// Fungsi yang hasilnya lebih baik ditulis ulang sebagai rentang pada kolom.
const RANGE_REWRITE_FUNCS: &[&str] = &[
    "DATE",
    "YEAR",
    "MONTH",
    "DAY",
    "DATE_TRUNC",
    "DATE_FORMAT",
    "TO_CHAR",
    "EXTRACT",
    "CAST",
    "CONVERT",
    "STRFTIME",
    "DATEPART",
];

/// Rekomendasi untuk semua statement di `sql`.
pub fn advise(sql: &str, dialect: Dialect, cat: &dyn IndexCatalog) -> Vec<IndexAdvice> {
    report(sql, dialect, cat).advice
}

/// Analisis lengkap: status index tiap kolom yang dipakai + rekomendasi.
pub fn report(sql: &str, dialect: Dialect, cat: &dyn IndexCatalog) -> QueryReport {
    let toks: Vec<Token> = tokenize(sql, dialect)
        .into_iter()
        .filter(|t| t.kind != TokKind::Comment)
        .collect();
    let mut rep = QueryReport::default();
    for stmt in toks.split(|t| t.kind == TokKind::Semicolon) {
        Block::new(sql, stmt, dialect, cat).run(&mut rep);
    }
    let mut seen = HashSet::new();
    rep.advice
        .retain(|a| seen.insert((a.table.clone(), a.message.clone())));
    rep
}

struct Block<'a> {
    sql: &'a str,
    toks: &'a [Token],
    dialect: Dialect,
    cat: &'a dyn IndexCatalog,
    scope: Vec<ScopeTable>,
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, TokKind::Word | TokKind::QuotedIdent)
}

fn is_cmp(t: &Token) -> bool {
    t.kind == TokKind::Op && matches!(t.text.as_str(), "=" | "<" | ">" | "<=" | ">=")
}

/// Index token `)` pasangan `(` di `open`.
fn matching_paren(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.kind {
            TokKind::LParen => depth += 1,
            TokKind::RParen => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

impl<'a> Block<'a> {
    fn new(sql: &'a str, toks: &'a [Token], dialect: Dialect, cat: &'a dyn IndexCatalog) -> Self {
        let scope = match (toks.first(), toks.last()) {
            (Some(f), Some(l)) => {
                let padded = format!("{} ", &sql[f.start..l.end]);
                analyze(&padded, padded.len(), dialect)
                    .scope
                    .into_iter()
                    .filter(|t| t.depth == 0)
                    .collect()
            }
            _ => Vec::new(),
        };
        Block {
            sql,
            toks,
            dialect,
            cat,
            scope,
        }
    }

    fn bases(&self) -> Vec<&ScopeTable> {
        self.scope
            .iter()
            .filter(|t| t.kind == TableKind::Base)
            .collect()
    }

    fn resolve(&self, q: &str) -> Option<&ScopeTable> {
        let found = self
            .scope
            .iter()
            .find(|t| {
                t.alias
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(q))
            })
            .or_else(|| {
                self.scope
                    .iter()
                    .find(|t| t.alias.is_none() && t.name.eq_ignore_ascii_case(q))
            })?;
        (found.kind == TableKind::Base).then_some(found)
    }

    fn column_exists(&self, table: &ScopeTable, col: &str) -> bool {
        self.cat
            .columns(&table.name)
            .is_none_or(|cols| cols.iter().any(|c| c.eq_ignore_ascii_case(col)))
    }

    fn scope_names(&self) -> Vec<String> {
        self.scope
            .iter()
            .flat_map(|t| [Some(t.name.clone()), t.alias.clone()])
            .flatten()
            .map(|n| n.to_ascii_lowercase())
            .collect()
    }

    /// Referensi kolom di `i`: `(tabel, kolom, index token terakhir)`.
    fn column_ref(&self, i: usize) -> Option<(ScopeTable, String, usize)> {
        let toks = self.toks;
        let t = toks.get(i)?;
        if !is_name(t) || (i > 0 && toks[i - 1].kind == TokKind::Dot) {
            return None;
        }
        let next = toks.get(i + 1);
        if next.is_some_and(|n| n.kind == TokKind::Dot) {
            // `q.kol` atau `schema.tabel.kol`: ambil dua segmen terakhir
            let mut j = i;
            while toks.get(j + 1).is_some_and(|n| n.kind == TokKind::Dot)
                && toks.get(j + 2).is_some_and(is_name)
            {
                j += 2;
            }
            if j == i || toks.get(j + 1).is_some_and(|n| n.kind == TokKind::LParen) {
                return None;
            }
            let table = self.resolve(&toks[j - 2].text)?.clone();
            let col = toks[j].text.clone();
            return self.column_exists(&table, &col).then_some((table, col, j));
        }
        if next.is_some_and(|n| n.kind == TokKind::LParen)
            || (t.kind == TokKind::Word && is_reserved(&t.text))
            || (i > 0 && toks[i - 1].is_kw("AS"))
            || self.scope_names().contains(&t.text.to_ascii_lowercase())
        {
            return None;
        }
        let bases = self.bases();
        let table = if bases.len() == 1 {
            bases[0].clone()
        } else {
            // Beberapa tabel: hanya bila kolom itu unik di antara tabel yang diketahui
            let owners: Vec<&ScopeTable> = bases
                .into_iter()
                .filter(|b| {
                    self.cat
                        .columns(&b.name)
                        .is_some_and(|cols| cols.iter().any(|c| c.eq_ignore_ascii_case(&t.text)))
                })
                .collect();
            if owners.len() != 1 {
                return None;
            }
            owners[0].clone()
        };
        self.column_exists(&table, &t.text)
            .then(|| (table, t.text.clone(), i))
    }

    fn run(&self, rep: &mut QueryReport) {
        if self.toks.is_empty() || self.bases().is_empty() && !self.has_subquery() {
            return;
        }
        let toks = self.toks;
        let mut uses: Vec<ColUse> = Vec::new();
        let mut joins: Vec<JoinUse> = Vec::new();
        let mut section = Section::None;
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            // Subquery dianalisis sebagai blok sendiri
            if t.kind == TokKind::LParen
                && toks
                    .get(i + 1)
                    .is_some_and(|n| n.is_kw("SELECT") || n.is_kw("WITH"))
            {
                let close = matching_paren(toks, i).unwrap_or(toks.len());
                let inner = &toks[i + 1..close.min(toks.len())];
                Block::new(self.sql, inner, self.dialect, self.cat).run(rep);
                i = close + 1;
                continue;
            }
            if t.kind == TokKind::Word {
                let next_by = toks.get(i + 1).is_some_and(|n| n.is_kw("BY"));
                let kw = t.text.to_ascii_uppercase();
                let new_section = match kw.as_str() {
                    "WHERE" | "ON" => Some(Section::Cond),
                    "GROUP" if next_by => Some(Section::Group),
                    "ORDER" if next_by => Some(Section::Order),
                    "SELECT" | "FROM" | "JOIN" | "HAVING" | "LIMIT" | "OFFSET" | "SET"
                    | "VALUES" | "UNION" | "EXCEPT" | "INTERSECT" | "RETURNING" | "USING"
                    | "WINDOW" | "FETCH" => Some(Section::None),
                    _ => None,
                };
                if let Some(s) = new_section {
                    section = s;
                    i += if next_by { 2 } else { 1 };
                    continue;
                }
            }
            match section {
                Section::Cond => {
                    if let Some(next) = self.condition_at(i, &mut uses, &mut joins, rep) {
                        i = next;
                        continue;
                    }
                }
                Section::Group | Section::Order => {
                    if let Some((table, column, end)) = self.column_ref(i) {
                        let role = if section == Section::Group {
                            Role::Group
                        } else {
                            Role::Order
                        };
                        uses.push(ColUse {
                            table,
                            column,
                            role,
                            span: t.start..toks[end].end,
                        });
                        i = end + 1;
                        continue;
                    }
                }
                Section::None => {}
            }
            i += 1;
        }
        self.evaluate(&uses, &joins, rep);
    }

    fn has_subquery(&self) -> bool {
        self.toks
            .windows(2)
            .any(|w| w[0].kind == TokKind::LParen && (w[1].is_kw("SELECT") || w[1].is_kw("WITH")))
    }

    /// Satu predikat di WHERE/ON mulai token `i`; mengembalikan index lanjutan.
    fn condition_at(
        &self,
        i: usize,
        uses: &mut Vec<ColUse>,
        joins: &mut Vec<JoinUse>,
        rep: &mut QueryReport,
    ) -> Option<usize> {
        let toks = self.toks;
        let t = &toks[i];
        // `FUNC(kolom) <op> ...` → index pada kolom tidak terpakai
        if t.kind == TokKind::Word
            && toks.get(i + 1).is_some_and(|n| n.kind == TokKind::LParen)
            && let Some(close) = matching_paren(toks, i + 1)
            && toks.get(close + 1).is_some_and(|n| {
                is_cmp(n) || n.is_kw("LIKE") || n.is_kw("IN") || n.is_kw("BETWEEN")
            })
            && let Some((table, column, _)) = (i + 2..close).find_map(|k| self.column_ref(k))
        {
            let span = t.start..toks[close].end;
            rep.push_usage(ColumnUsage {
                table: table.name.clone(),
                column: column.clone(),
                used_in: "WHERE",
                status: self.status(&table, &column),
                defeated_by: Some(format!("wrapped in {}()", t.text.to_ascii_uppercase())),
                span: span.clone(),
            });
            rep.advice
                .push(self.function_advice(&t.text, &table, &column, span));
            return Some(close + 1);
        }

        let (table, column, end) = self.column_ref(i)?;
        let span_start = t.start;
        let op = toks.get(end + 1)?;
        let role = if op.is_op("=") {
            // `a.x = b.y` antar tabel → join
            if let Some((rt, rc, rend)) = self.column_ref(end + 2) {
                if !rt.name.eq_ignore_ascii_case(&table.name)
                    || rt.alias.as_deref() != table.alias.as_deref()
                {
                    joins.push(JoinUse {
                        left: (table, column),
                        right: (rt, rc),
                        span: span_start..toks[rend].end,
                    });
                }
                return Some(rend + 1);
            }
            Some(Role::Eq)
        } else if is_cmp(op) || op.is_kw("BETWEEN") {
            Some(Role::Range)
        } else if op.is_kw("IN") {
            Some(Role::Eq)
        } else if op.is_kw("LIKE") || op.is_kw("ILIKE") {
            match toks.get(end + 2) {
                Some(p) if p.kind == TokKind::Str && p.text.starts_with("'%") => {
                    rep.push_usage(ColumnUsage {
                        table: table.name.clone(),
                        column: column.clone(),
                        used_in: "WHERE LIKE",
                        status: self.status(&table, &column),
                        defeated_by: Some("leading % wildcard".into()),
                        span: span_start..p.end,
                    });
                    rep.advice
                        .push(self.wildcard_advice(&table, &column, span_start..p.end));
                    None
                }
                _ => Some(Role::Range),
            }
        } else {
            None
        };
        if let Some(role) = role {
            uses.push(ColUse {
                table,
                column,
                role,
                span: span_start..toks[end].end,
            });
        }
        Some(end + 1)
    }

    // ----- penilaian -----

    fn indexes(&self, table: &ScopeTable) -> Option<&[IndexDef]> {
        self.cat.indexes(&table.name)
    }

    /// Ada index yang kolom pertamanya `col` (index bisa dipakai untuk kolom ini).
    fn leading(&self, table: &ScopeTable, col: &str) -> Option<bool> {
        let idx = self.indexes(table)?;
        Some(idx.iter().any(|ix| {
            ix.columns
                .first()
                .is_some_and(|c| c.eq_ignore_ascii_case(col))
        }))
    }

    /// Status index kolom untuk laporan.
    fn status(&self, table: &ScopeTable, col: &str) -> ColumnStatus {
        let Some(idx) = self.indexes(table) else {
            return ColumnStatus::Unknown;
        };
        let lead = |ix: &&IndexDef| {
            ix.columns
                .first()
                .is_some_and(|c| c.eq_ignore_ascii_case(col))
        };
        if let Some(ix) = idx.iter().filter(lead).find(|ix| is_primary_index(ix)) {
            let _ = ix;
            return ColumnStatus::PrimaryKey;
        }
        if let Some(ix) = idx.iter().find(lead) {
            return ColumnStatus::Indexed {
                index: ix.name.clone(),
                unique: ix.unique,
            };
        }
        if let Some(fk) = self.is_fk(table, col) {
            return ColumnStatus::ForeignKeyNoIndex {
                references: fk.referenced_table_name.clone(),
            };
        }
        for ix in idx {
            if let Some(p) = ix
                .columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(col))
            {
                return ColumnStatus::NotLeading {
                    index: ix.name.clone(),
                    position: p + 1,
                };
            }
        }
        ColumnStatus::NotIndexed
    }

    fn record_usages(&self, uses: &[ColUse], joins: &[JoinUse], rep: &mut QueryReport) {
        let mut note_missing = |t: &ScopeTable, rep: &mut QueryReport| {
            if self.indexes(t).is_none()
                && !rep
                    .missing_metadata
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&t.name))
            {
                rep.missing_metadata.push(t.name.clone());
            }
        };
        for j in joins {
            for (t, c) in [&j.left, &j.right] {
                note_missing(t, rep);
                rep.push_usage(ColumnUsage {
                    table: t.name.clone(),
                    column: c.clone(),
                    used_in: "JOIN",
                    status: self.status(t, c),
                    defeated_by: None,
                    span: j.span.clone(),
                });
            }
        }
        for u in uses {
            note_missing(&u.table, rep);
            let used_in = match u.role {
                Role::Eq => "WHERE =",
                Role::Range => "WHERE range",
                Role::Group => "GROUP BY",
                Role::Order => "ORDER BY",
            };
            rep.push_usage(ColumnUsage {
                table: u.table.name.clone(),
                column: u.column.clone(),
                used_in,
                status: self.status(&u.table, &u.column),
                defeated_by: None,
                span: u.span.clone(),
            });
        }
    }

    fn is_fk(&self, table: &ScopeTable, col: &str) -> Option<&ForeignKey> {
        self.cat.foreign_keys().iter().find(|fk| {
            fk.table_name.eq_ignore_ascii_case(&table.name)
                && fk.column_name.eq_ignore_ascii_case(col)
        })
    }

    fn evaluate(&self, uses: &[ColUse], joins: &[JoinUse], rep: &mut QueryReport) {
        self.record_usages(uses, joins, rep);
        // 1. Kolom JOIN tanpa index
        for j in joins {
            for (side, other) in [(&j.left, &j.right), (&j.right, &j.left)] {
                let (table, col) = side;
                if self.leading(table, col) != Some(false) {
                    continue;
                }
                let other_name = &other.0.name;
                let (message, hint) = match self.is_fk(table, col) {
                    Some(fk) => (
                        format!(
                            "Foreign key `{}.{col}` used in JOIN has no index.",
                            table.name
                        ),
                        format!(
                            "Joining from `{}` scans `{}` for every row. Foreign keys are not indexed automatically in PostgreSQL/SQLite/SQL Server.",
                            fk.referenced_table_name, table.name
                        ),
                    ),
                    None => (
                        format!("JOIN column `{}.{col}` has no index.", table.name),
                        format!(
                            "Each lookup from `{other_name}` into `{}` becomes a full scan.",
                            table.name
                        ),
                    ),
                };
                rep.advice.push(IndexAdvice {
                    level: AdviceLevel::Warning,
                    table: table.name.clone(),
                    message,
                    hint: Some(hint),
                    ddl: Some(self.create_index(table, std::slice::from_ref(col))),
                    span: Some(j.span.clone()),
                });
            }
        }

        // 2. Filter WHERE/ON per tabel: index komposit equality, sort, lalu range
        let mut tables: Vec<&ScopeTable> = Vec::new();
        for u in uses {
            if !tables
                .iter()
                .any(|t| t.name.eq_ignore_ascii_case(&u.table.name))
            {
                tables.push(&u.table);
            }
        }
        let order_tables: HashSet<String> = uses
            .iter()
            .filter(|u| u.role == Role::Order)
            .map(|u| u.table.name.to_ascii_lowercase())
            .collect();
        for table in tables {
            if self.indexes(table).is_none() {
                continue;
            }
            let of = |role: Role| -> Vec<&ColUse> {
                let mut v: Vec<&ColUse> = Vec::new();
                for u in uses
                    .iter()
                    .filter(|u| u.role == role && u.table.name.eq_ignore_ascii_case(&table.name))
                {
                    if !v.iter().any(|x| x.column.eq_ignore_ascii_case(&u.column)) {
                        v.push(u);
                    }
                }
                v
            };
            let (eq, range, group, order) = (
                of(Role::Eq),
                of(Role::Range),
                of(Role::Group),
                of(Role::Order),
            );
            let filters: Vec<&ColUse> = eq.iter().chain(range.iter()).copied().collect();
            if !filters.is_empty() {
                if filters
                    .iter()
                    .any(|u| self.leading(table, &u.column) == Some(true))
                {
                    continue;
                }
                let mut cols: Vec<String> = eq.iter().map(|u| u.column.clone()).collect();
                // ORDER BY ikut hanya bila seluruhnya dari tabel ini
                if order_tables.len() == 1 && !order.is_empty() {
                    for u in &order {
                        if !cols.iter().any(|c| c.eq_ignore_ascii_case(&u.column)) {
                            cols.push(u.column.clone());
                        }
                    }
                }
                if let Some(r) = range.first()
                    && !cols.iter().any(|c| c.eq_ignore_ascii_case(&r.column))
                {
                    cols.push(r.column.clone());
                }
                cols.truncate(4);
                let list = cols.join(", ");
                let hint = if cols.len() > 1 {
                    "Composite index ordered equality, then sort, then range columns, so one index serves the whole filter."
                } else {
                    "Without an index the database scans the whole table for this filter."
                };
                rep.advice.push(IndexAdvice {
                    level: AdviceLevel::Warning,
                    table: table.name.clone(),
                    message: format!("No index supports the filter on `{}` ({list}).", table.name),
                    hint: Some(hint.into()),
                    ddl: Some(self.create_index(table, &cols)),
                    span: Some(filters[0].span.clone()),
                });
                continue;
            }
            // 3. GROUP BY tanpa filter pada tabel ini
            if !group.is_empty()
                && group
                    .iter()
                    .all(|u| self.leading(table, &u.column) == Some(false))
            {
                let cols: Vec<String> = group.iter().take(4).map(|u| u.column.clone()).collect();
                rep.advice.push(IndexAdvice {
                    level: AdviceLevel::Info,
                    table: table.name.clone(),
                    message: format!(
                        "GROUP BY on `{}` ({}) has no supporting index.",
                        table.name,
                        cols.join(", ")
                    ),
                    hint: Some(
                        "An index on the grouped columns lets the database group without sorting the whole table."
                            .into(),
                    ),
                    ddl: Some(self.create_index(table, &cols)),
                    span: Some(group[0].span.clone()),
                });
            }
        }
    }

    fn function_advice(
        &self,
        func: &str,
        table: &ScopeTable,
        column: &str,
        span: Range<usize>,
    ) -> IndexAdvice {
        let f = func.to_ascii_uppercase();
        let call = self.sql[span.clone()].to_string();
        let (hint, ddl) = if RANGE_REWRITE_FUNCS.contains(&f.as_str()) {
            (
                format!(
                    "Compare the raw column with a range instead, e.g. `{column} >= '2024-01-01' AND {column} < '2024-02-01'`."
                ),
                None,
            )
        } else if self.dialect == Dialect::MsSql {
            (
                format!("Add a persisted computed column for `{call}` and index it."),
                None,
            )
        } else {
            let expr = format!("{f}({})", self.ident(column));
            let body = if self.dialect == Dialect::MySql {
                format!("({expr})")
            } else {
                expr
            };
            let name = index_name(&table.name, &[f.to_ascii_lowercase(), column.to_string()]);
            (
                "Create an expression index that matches the function call.".into(),
                Some(format!(
                    "CREATE INDEX {} ON {} ({body});",
                    self.ident(&name),
                    self.table_ident(table)
                )),
            )
        };
        IndexAdvice {
            level: AdviceLevel::Warning,
            table: table.name.clone(),
            message: format!(
                "`{call}` wraps `{}.{column}`, so an index on the column can't be used.",
                table.name
            ),
            hint: Some(hint),
            ddl,
            span: Some(span),
        }
    }

    fn wildcard_advice(&self, table: &ScopeTable, column: &str, span: Range<usize>) -> IndexAdvice {
        let (hint, ddl) = match self.dialect {
            Dialect::Postgres => (
                "Leading `%` forces a full scan. A trigram index supports it (requires `CREATE EXTENSION pg_trgm`).",
                Some(format!(
                    "CREATE INDEX {} ON {} USING gin ({} gin_trgm_ops);",
                    self.ident(&index_name(
                        &table.name,
                        &[column.to_string(), "trgm".into()]
                    )),
                    self.table_ident(table),
                    self.ident(column)
                )),
            ),
            Dialect::MySql => (
                "Leading `%` forces a full scan. Use a FULLTEXT index with MATCH … AGAINST, or search by prefix.",
                None,
            ),
            _ => (
                "Leading `%` forces a full scan. Search by prefix (`'abc%'`) when possible.",
                None,
            ),
        };
        IndexAdvice {
            level: AdviceLevel::Warning,
            table: table.name.clone(),
            message: format!(
                "`LIKE '%…'` on `{}.{column}` can't use a B-tree index.",
                table.name
            ),
            hint: Some(hint.into()),
            ddl,
            span: Some(span),
        }
    }

    // ----- DDL -----

    fn ident(&self, name: &str) -> String {
        let plain = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && !is_reserved(name);
        if plain {
            name.to_string()
        } else {
            self.dialect.quote_ident(name)
        }
    }

    fn table_ident(&self, t: &ScopeTable) -> String {
        match &t.schema {
            Some(s) => format!("{}.{}", self.ident(s), self.ident(&t.name)),
            None => self.ident(&t.name),
        }
    }

    fn create_index(&self, table: &ScopeTable, cols: &[String]) -> String {
        let name = index_name(&table.name, cols);
        let list = cols
            .iter()
            .map(|c| self.ident(c))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "CREATE INDEX {} ON {} ({list});",
            self.ident(&name),
            self.table_ident(table)
        )
    }
}

/// `idx_<tabel>_<kolom...>`, hanya `[a-z0-9_]`, maksimal 60 karakter
/// (batas nama identifier PostgreSQL 63, MySQL 64).
fn index_name(table: &str, cols: &[String]) -> String {
    let raw = format!("idx_{table}_{}", cols.join("_")).to_ascii_lowercase();
    let mut name: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    name.truncate(60);
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cat {
        idx: Vec<(&'static str, Vec<IndexDef>)>,
        cols: Vec<(&'static str, Vec<String>)>,
        fks: Vec<ForeignKey>,
    }

    impl IndexCatalog for Cat {
        fn indexes(&self, table: &str) -> Option<&[IndexDef]> {
            self.idx
                .iter()
                .find(|(t, _)| t.eq_ignore_ascii_case(table))
                .map(|(_, v)| v.as_slice())
        }
        fn columns(&self, table: &str) -> Option<&[String]> {
            self.cols
                .iter()
                .find(|(t, _)| t.eq_ignore_ascii_case(table))
                .map(|(_, v)| v.as_slice())
        }
        fn foreign_keys(&self) -> &[ForeignKey] {
            &self.fks
        }
    }

    fn ix(name: &str, cols: &[&str]) -> IndexDef {
        IndexDef {
            name: name.into(),
            columns: cols.iter().map(|c| c.to_string()).collect(),
            unique: false,
        }
    }

    fn cols(v: &[&str]) -> Vec<String> {
        v.iter().map(|c| c.to_string()).collect()
    }

    fn cat() -> Cat {
        Cat {
            idx: vec![
                (
                    "users",
                    vec![ix("users_pkey", &["id"]), ix("users_email", &["email"])],
                ),
                ("orders", vec![ix("orders_pkey", &["id"])]),
                ("products", vec![ix("products_pkey", &["id"])]),
            ],
            cols: vec![
                (
                    "users",
                    cols(&["id", "name", "email", "active", "created_at"]),
                ),
                (
                    "orders",
                    cols(&["id", "user_id", "total", "status", "created_at"]),
                ),
                ("products", cols(&["id", "title", "price"])),
            ],
            fks: vec![ForeignKey {
                constraint_name: "fk".into(),
                table_name: "orders".into(),
                column_name: "user_id".into(),
                referenced_table_name: "users".into(),
                referenced_column_name: "id".into(),
            }],
        }
    }

    fn run(sql: &str) -> Vec<IndexAdvice> {
        advise(sql, Dialect::Postgres, &cat())
    }

    fn ddls(a: &[IndexAdvice]) -> Vec<String> {
        a.iter().filter_map(|x| x.ddl.clone()).collect()
    }

    #[test]
    fn unindexed_fk_in_join_is_flagged() {
        let a = run("SELECT * FROM users u JOIN orders o ON o.user_id = u.id");
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].message.contains("Foreign key `orders.user_id`"));
        assert_eq!(
            a[0].ddl.as_deref(),
            Some("CREATE INDEX idx_orders_user_id ON orders (user_id);")
        );
        // sudah ter-index → tidak ada saran
        let mut c = cat();
        c.idx[1]
            .1
            .push(ix("orders_user", &["user_id", "created_at"]));
        assert!(
            advise(
                "SELECT * FROM users u JOIN orders o ON o.user_id = u.id",
                Dialect::Postgres,
                &c
            )
            .is_empty()
        );
    }

    #[test]
    fn composite_index_follows_equality_sort_range() {
        let a = run(
            "SELECT * FROM orders WHERE status = 'paid' AND created_at >= '2024-01-01' AND total > 10 ORDER BY created_at DESC",
        );
        assert_eq!(
            ddls(&a),
            vec!["CREATE INDEX idx_orders_status_created_at ON orders (status, created_at);"]
        );
        let a =
            run("SELECT * FROM orders o WHERE o.status IN ('a','b') AND o.total BETWEEN 1 AND 5");
        assert_eq!(
            ddls(&a),
            vec!["CREATE INDEX idx_orders_status_total ON orders (status, total);"]
        );
    }

    #[test]
    fn indexed_filters_and_pk_lookups_are_quiet() {
        assert!(run("SELECT * FROM users WHERE email = 'a@b.c'").is_empty());
        assert!(run("SELECT * FROM orders WHERE id = 5").is_empty());
        // salah satu filter sudah ter-index → index itu bisa dipakai
        assert!(run("SELECT * FROM users WHERE email = 'x' AND active = true").is_empty());
    }

    #[test]
    fn unknown_index_metadata_or_columns_are_skipped() {
        // tabel tanpa metadata index: tidak menebak
        assert!(run("SELECT * FROM audit_log WHERE actor = 'x'").is_empty());
        // kolom tidak ada di tabel: tidak dilaporkan
        assert!(run("SELECT * FROM orders WHERE nonexistent = 1").is_empty());
        // tanpa WHERE tidak ada yang disarankan
        assert!(run("SELECT * FROM orders").is_empty());
    }

    #[test]
    fn function_on_column_and_leading_wildcard() {
        let a = run("SELECT * FROM users WHERE LOWER(email) = 'a@b.c'");
        assert_eq!(a.len(), 1, "{a:?}");
        assert!(a[0].message.contains("`LOWER(email)` wraps `users.email`"));
        assert_eq!(
            a[0].ddl.as_deref(),
            Some("CREATE INDEX idx_users_lower_email ON users (LOWER(email));")
        );
        let a = run("SELECT * FROM orders WHERE DATE(created_at) = '2024-01-01'");
        assert!(a[0].hint.as_deref().unwrap().contains("range"));
        assert!(a[0].ddl.is_none());

        let a = run("SELECT * FROM users WHERE name LIKE '%jo%'");
        assert!(a[0].message.contains("LIKE '%…'"));
        assert!(a[0].ddl.as_deref().unwrap().contains("gin_trgm_ops"));
        // prefix LIKE memakai index biasa
        let a = run("SELECT * FROM users WHERE name LIKE 'jo%'");
        assert_eq!(
            ddls(&a),
            vec!["CREATE INDEX idx_users_name ON users (name);"]
        );
    }

    #[test]
    fn group_by_subquery_and_multiple_statements() {
        let a = run("SELECT status, count(*) FROM orders GROUP BY status");
        assert_eq!(a[0].level, AdviceLevel::Info);
        assert_eq!(
            a[0].ddl.as_deref(),
            Some("CREATE INDEX idx_orders_status ON orders (status);")
        );
        let a = run(
            "SELECT * FROM users WHERE id IN (SELECT user_id FROM orders WHERE status = 'x'); SELECT * FROM products WHERE price > 5",
        );
        let d = ddls(&a);
        assert!(
            d.contains(&"CREATE INDEX idx_orders_status ON orders (status);".to_string()),
            "{d:?}"
        );
        assert!(
            d.contains(&"CREATE INDEX idx_products_price ON products (price);".to_string()),
            "{d:?}"
        );
    }

    #[test]
    fn unqualified_column_with_several_tables_needs_unique_owner() {
        // `status` hanya ada di orders → tetap ter-resolve
        let a = run("SELECT * FROM users u JOIN orders o ON o.user_id = u.id WHERE status = 'x'");
        assert!(
            ddls(&a).contains(&"CREATE INDEX idx_orders_status ON orders (status);".to_string())
        );
        // `created_at` ada di dua tabel → ambigu, dilewati
        let a =
            run("SELECT * FROM users u JOIN orders o ON o.user_id = u.id WHERE created_at > now()");
        assert!(!a.iter().any(|x| x.message.contains("created_at")), "{a:?}");
    }

    #[test]
    fn identifiers_are_quoted_per_dialect() {
        let mut c = cat();
        c.idx.push(("Order Details", vec![]));
        let a = advise(
            "SELECT * FROM [Order Details] WHERE Qty = 1",
            Dialect::MsSql,
            &c,
        );
        assert_eq!(
            a[0].ddl.as_deref(),
            Some("CREATE INDEX idx_order_details_qty ON [Order Details] ([Qty]);")
        );
    }

    #[test]
    fn garbage_does_not_panic() {
        for s in [
            "",
            ";",
            "WHERE",
            "SELECT * FROM t WHERE (",
            "SELECT ( SELECT",
            "ON a.b = ",
            "SELECT * FROM users WHERE LOWER(",
        ] {
            let _ = run(s);
        }
    }
}
