//! Provider kandidat + ranker.
//!
//! Menerima `Analysis` (hasil analyzer) dan `Catalog` (metadata tabel/kolom/FK)
//! lalu menghasilkan daftar `CompletionItem` yang sudah diurutkan. Murni: tidak
//! menyentuh `Tabular` sehingga bisa dites dengan katalog tiruan.

use super::analyzer::{
    AGGREGATES, Analysis, Clause, ColRef, Expect, ScopeTable, TableKind, is_reserved,
};
use super::lexer::Dialect;
use crate::models::enums::KeywordCasing;
use crate::models::structs::ForeignKey;

/// Penanda posisi kursor di dalam teks sisipan; dihapus saat diterapkan.
pub const CURSOR_MARK: char = '\u{1}';

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: Option<String>,
}

/// Sumber metadata untuk engine.
pub trait Catalog {
    /// Semua tabel/view di database aktif.
    fn tables(&self) -> &[String];
    /// Kolom tabel (urutan ordinal), `None` bila belum ter-cache.
    fn columns(&self, table: &str) -> Option<&[ColumnMeta]>;
    fn foreign_keys(&self) -> &[ForeignKey];
    /// Berapa kali label ini pernah dipilih (untuk ranking).
    fn usage(&self, _label: &str) -> u32 {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Table,
    Cte,
    Column,
    Alias,
    Keyword,
    Operator,
    Function,
    JoinCondition,
    Template,
    Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// Teks yang ditampilkan.
    pub label: String,
    /// Teks yang disisipkan (boleh berisi `CURSOR_MARK`).
    pub insert: String,
    pub kind: ItemKind,
    pub detail: Option<String>,
    pub score: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub dialect: Dialect,
    pub casing: KeywordCasing,
}

/// CamelHump + subsequence fuzzy match (gaya DataGrip). `Some(score)` bila
/// semua karakter `pref` muncul berurutan di `cand`; prefix persis menang besar,
/// kecocokan di batas kata (awal, setelah `_`/`.`, huruf kapital) bernilai lebih.
pub fn fuzzy_match(pref: &str, cand: &str) -> Option<i32> {
    let p: Vec<char> = pref
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if p.is_empty() {
        return Some(0);
    }
    let orig: Vec<char> = cand.chars().collect();
    let lower: Vec<char> = orig.iter().flat_map(|c| c.to_lowercase()).collect();
    if lower.starts_with(&p) {
        return Some(1000 - orig.len() as i32);
    }
    let mut pi = 0usize;
    let mut score = 0i32;
    for (idx, &ch) in lower.iter().enumerate() {
        if pi >= p.len() {
            break;
        }
        if ch == p[pi] {
            let prev_sep = idx == 0
                || orig
                    .get(idx - 1)
                    .is_some_and(|&c| c == '_' || c == '.' || c == ' ');
            let hump = orig.get(idx).is_some_and(|&c| c.is_uppercase());
            score += if prev_sep || hump { 10 } else { 1 };
            pi += 1;
        }
    }
    (pi == p.len()).then_some(score)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TypeClass {
    Num,
    Text,
    Time,
    Bool,
    Other,
}

fn classify(data_type: Option<&str>) -> TypeClass {
    let Some(t) = data_type else {
        return TypeClass::Other;
    };
    let t = t.to_ascii_lowercase();
    if t.contains("bool") || t == "tinyint(1)" || t == "bit" {
        TypeClass::Bool
    } else if ["date", "time", "year", "interval"]
        .iter()
        .any(|k| t.contains(k))
    {
        TypeClass::Time
    } else if [
        "int", "dec", "num", "float", "double", "real", "money", "serial",
    ]
    .iter()
    .any(|k| t.contains(k))
    {
        TypeClass::Num
    } else if [
        "char", "text", "string", "uuid", "enum", "clob", "json", "xml",
    ]
    .iter()
    .any(|k| t.contains(k))
    {
        TypeClass::Text
    } else {
        TypeClass::Other
    }
}

fn compatible(a: TypeClass, b: TypeClass) -> bool {
    a == TypeClass::Other || b == TypeClass::Other || a == b
}

/// Bentuk tunggal sederhana: `categories` → `category`, `users` → `user`.
fn singular(name: &str) -> String {
    let l = name.to_ascii_lowercase();
    if let Some(s) = l.strip_suffix("ies") {
        format!("{s}y")
    } else if l.ends_with("ses") || l.ends_with("xes") {
        l[..l.len() - 2].to_string()
    } else if let Some(s) = l.strip_suffix('s') {
        s.to_string()
    } else {
        l
    }
}

/// Apakah `col` tampak seperti FK ke tabel `table` (`user_id`, `userid`, `users_id`).
fn looks_like_fk_to(col: &str, table: &str) -> bool {
    let c = col.to_ascii_lowercase();
    let t = table.to_ascii_lowercase();
    let s = singular(&t);
    [
        format!("{s}_id"),
        format!("{t}_id"),
        format!("{s}id"),
        format!("{t}id"),
    ]
    .contains(&c)
}

fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 1).collect();
        out.push('…');
        out
    }
}

fn same_table(a: &ScopeTable, b: &ScopeTable) -> bool {
    a.idx == b.idx && a.depth == b.depth
}

const START_KW: &[&str] = &[
    "SELECT",
    "INSERT INTO",
    "UPDATE",
    "DELETE FROM",
    "WITH",
    "CREATE TABLE",
    "ALTER TABLE",
    "DROP TABLE",
    "TRUNCATE TABLE",
    "EXPLAIN",
];

const GENERIC_KW: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "ON",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "AS",
    "DISTINCT",
    "IN",
    "IS",
    "LIKE",
    "BETWEEN",
    "UNION",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "EXISTS",
    "CREATE TABLE",
    "ALTER TABLE",
    "DROP TABLE",
];

/// Alias pendek yang juga keyword/ambigu — jangan dipakai sebagai alias otomatis.
const BAD_ALIASES: &[&str] = &[
    "as", "on", "or", "in", "is", "by", "to", "do", "if", "at", "of",
];

fn functions(dialect: Dialect) -> Vec<&'static str> {
    let mut f = vec![
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
        "COALESCE",
        "NULLIF",
        "CAST",
        "LOWER",
        "UPPER",
        "TRIM",
        "LENGTH",
        "SUBSTRING",
        "REPLACE",
        "ROUND",
        "ABS",
        "CONCAT",
        "ROW_NUMBER",
        "RANK",
        "DENSE_RANK",
        "LAG",
        "LEAD",
    ];
    f.extend_from_slice(match dialect {
        Dialect::MySql => &[
            "IFNULL",
            "IF",
            "NOW",
            "CURDATE",
            "DATE_FORMAT",
            "DATE_ADD",
            "DATE_SUB",
            "DATEDIFF",
            "GROUP_CONCAT",
            "CONCAT_WS",
            "JSON_EXTRACT",
            "CHAR_LENGTH",
            "YEAR",
            "MONTH",
            "DAY",
            "UNIX_TIMESTAMP",
            "FROM_UNIXTIME",
            "GREATEST",
            "LEAST",
        ][..],
        Dialect::Postgres => &[
            "NOW",
            "DATE_TRUNC",
            "TO_CHAR",
            "TO_DATE",
            "STRING_AGG",
            "ARRAY_AGG",
            "JSON_AGG",
            "JSONB_BUILD_OBJECT",
            "EXTRACT",
            "AGE",
            "GENERATE_SERIES",
            "SPLIT_PART",
            "REGEXP_REPLACE",
            "GREATEST",
            "LEAST",
        ][..],
        Dialect::Sqlite => &[
            "IFNULL",
            "DATE",
            "DATETIME",
            "STRFTIME",
            "GROUP_CONCAT",
            "JULIANDAY",
            "INSTR",
            "SUBSTR",
            "TYPEOF",
        ][..],
        Dialect::MsSql => &[
            "ISNULL",
            "GETDATE",
            "DATEADD",
            "DATEDIFF",
            "FORMAT",
            "CONVERT",
            "STRING_AGG",
            "LEN",
            "CHARINDEX",
            "IIF",
        ][..],
        Dialect::Generic => &["IFNULL", "NOW", "GREATEST", "LEAST"][..],
    });
    f
}

struct Builder<'a> {
    a: &'a Analysis,
    cat: &'a dyn Catalog,
    o: Options,
    out: Vec<CompletionItem>,
}

impl<'a> Builder<'a> {
    // ----- utilitas -----

    fn kw(&self, s: &str) -> String {
        match self.o.casing {
            KeywordCasing::Upper | KeywordCasing::Preserve => s.to_ascii_uppercase(),
            KeywordCasing::Lower => s.to_ascii_lowercase(),
        }
    }

    /// Quote identifier bila perlu (karakter khusus, keyword, atau huruf besar di Postgres).
    fn ident(&self, name: &str) -> String {
        let plain = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        let pg_upper =
            self.o.dialect == Dialect::Postgres && name.chars().any(|c| c.is_ascii_uppercase());
        if plain && !is_reserved(name) && !pg_upper {
            name.to_string()
        } else {
            self.o.dialect.quote_ident(name)
        }
    }

    fn push(
        &mut self,
        label: String,
        insert: String,
        filter: &str,
        kind: ItemKind,
        detail: Option<String>,
        boost: i32,
    ) {
        let Some(fz) = fuzzy_match(&self.a.partial, filter) else {
            return;
        };
        let usage = (self.cat.usage(&label).min(6) as i32) * 15;
        self.out.push(CompletionItem {
            label,
            insert,
            kind,
            detail,
            score: fz + boost + usage,
        });
    }

    fn keyword(&mut self, s: &str, boost: i32) {
        if s == "=" {
            self.push(
                "=".into(),
                "= ".into(),
                "=",
                ItemKind::Operator,
                None,
                boost,
            );
            return;
        }
        let k = self.kw(s);
        self.push(
            k.clone(),
            format!("{k} "),
            &k,
            ItemKind::Keyword,
            Some("keyword".into()),
            boost,
        );
    }

    fn keywords(&mut self, list: &[&str], top: i32) {
        for (i, k) in list.iter().enumerate() {
            self.keyword(k, top - (i as i32) * 10);
        }
    }

    fn fks(&self) -> &'a [ForeignKey] {
        self.cat.foreign_keys()
    }

    fn depth0(&self) -> impl Iterator<Item = &'a ScopeTable> + use<'a> {
        self.a.scope.iter().filter(|t| t.depth == 0)
    }

    /// Tabel di query saat ini yang ditulis sebelum kursor.
    fn tables_before_cursor(&self) -> Vec<&'a ScopeTable> {
        let c = self.a.cursor_tok;
        self.depth0().filter(|t| t.idx < c).collect()
    }

    fn current_table(&self) -> Option<&'a ScopeTable> {
        self.tables_before_cursor()
            .into_iter()
            .max_by_key(|t| t.idx)
    }

    fn uses_aliases(&self) -> bool {
        self.depth0().any(|t| t.alias.is_some())
    }

    /// Alias singkat dari nama tabel: `order_items` → `oi`, `users` → `u`.
    fn gen_alias(&self, table: &str) -> String {
        let mut initials = String::new();
        let mut prev_sep = true;
        let mut prev_lower = false;
        for c in table.chars() {
            if matches!(c, '_' | '-' | ' ') {
                prev_sep = true;
                continue;
            }
            if c.is_ascii_alphabetic() && (prev_sep || (c.is_ascii_uppercase() && prev_lower)) {
                initials.push(c.to_ascii_lowercase());
            }
            prev_lower = c.is_ascii_lowercase();
            prev_sep = false;
        }
        if initials.is_empty() {
            initials = table
                .chars()
                .take(1)
                .collect::<String>()
                .to_ascii_lowercase();
        }
        let taken: Vec<String> = self
            .a
            .scope
            .iter()
            .flat_map(|t| {
                [
                    Some(t.name.to_ascii_lowercase()),
                    t.alias.as_ref().map(|a| a.to_ascii_lowercase()),
                ]
            })
            .flatten()
            .collect();
        let ok =
            |s: &str| !is_reserved(s) && !BAD_ALIASES.contains(&s) && !taken.iter().any(|t| t == s);
        if ok(&initials) {
            return initials;
        }
        (1..100)
            .map(|n| format!("{initials}{n}"))
            .find(|s| ok(s))
            .unwrap_or(initials)
    }

    fn table_columns(&self, t: &ScopeTable) -> Vec<ColumnMeta> {
        match t.kind {
            TableKind::Base => self
                .cat
                .columns(&t.name)
                .map(|c| c.to_vec())
                .unwrap_or_default(),
            TableKind::Cte | TableKind::Derived => {
                let mut v: Vec<ColumnMeta> = t
                    .columns
                    .iter()
                    .map(|c| ColumnMeta {
                        name: c.clone(),
                        data_type: None,
                    })
                    .collect();
                for s in &t.star_from {
                    for c in self.table_columns(s) {
                        if !v.iter().any(|x| x.name.eq_ignore_ascii_case(&c.name)) {
                            v.push(c);
                        }
                    }
                }
                v
            }
        }
    }

    /// Cari tabel scope untuk qualifier (alias, lalu nama tabel, lalu CTE).
    fn resolve(&self, q: &str) -> Option<ScopeTable> {
        let scope = &self.a.scope;
        scope
            .iter()
            .filter(|t| {
                t.alias
                    .as_deref()
                    .is_some_and(|a| a.eq_ignore_ascii_case(q))
            })
            .min_by_key(|t| t.depth)
            .or_else(|| {
                scope
                    .iter()
                    .filter(|t| t.name.eq_ignore_ascii_case(q))
                    .min_by_key(|t| t.depth)
            })
            .cloned()
            .or_else(|| {
                self.a
                    .ctes
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(q))
                    .cloned()
            })
    }

    fn fk_from(&self, table: &str, col: &str) -> Option<&'a ForeignKey> {
        self.fks().iter().find(|fk| {
            fk.table_name.eq_ignore_ascii_case(table) && fk.column_name.eq_ignore_ascii_case(col)
        })
    }

    fn is_referenced(&self, table: &str, col: &str) -> bool {
        self.fks().iter().any(|fk| {
            fk.referenced_table_name.eq_ignore_ascii_case(table)
                && fk.referenced_column_name.eq_ignore_ascii_case(col)
        })
    }

    fn column_detail(&self, t: &ScopeTable, c: &ColumnMeta) -> String {
        let mut d = c.data_type.clone().unwrap_or_else(|| "column".into());
        let owner = if t.kind == TableKind::Base {
            t.name.as_str()
        } else {
            t.display()
        };
        if !owner.is_empty() {
            d.push_str(" · ");
            d.push_str(owner);
        }
        if t.kind == TableKind::Base {
            if let Some(fk) = self.fk_from(&t.name, &c.name) {
                d.push_str(&format!(" · FK→{}", fk.referenced_table_name));
            } else if c.name.eq_ignore_ascii_case("id") || self.is_referenced(&t.name, &c.name) {
                d.push_str(" · PK");
            }
        }
        d
    }

    /// Kolom operand kiri (`lhs`) atau kolom target VALUES.
    fn lhs_meta(&self) -> Option<(ScopeTable, ColumnMeta)> {
        if let Some(i) = self.a.value_index {
            let t = self.a.dml_target.clone()?;
            let c = self.table_columns(&t).get(i).cloned()?;
            return Some((t, c));
        }
        let ColRef { qualifier, column } = self.a.lhs.as_ref()?;
        let find = |t: &ScopeTable| {
            self.table_columns(t)
                .into_iter()
                .find(|c| c.name.eq_ignore_ascii_case(column))
        };
        if let Some(q) = qualifier {
            let t = self.resolve(q)?;
            let c = find(&t)?;
            return Some((t, c));
        }
        let cands: Vec<ScopeTable> = match &self.a.dml_target {
            Some(t) => vec![t.clone()],
            None => self.a.scope.clone(),
        };
        cands.into_iter().find_map(|t| find(&t).map(|c| (t, c)))
    }

    /// Apakah `table.col` adalah pasangan join dari operand kiri (FK atau `<tabel>_id` ↔ `id`).
    fn is_fk_partner(&self, lhs: Option<&ColRef>, table: &ScopeTable, col: &str) -> bool {
        let Some(l) = lhs else { return false };
        let Some(lt) = l.qualifier.as_deref().and_then(|q| self.resolve(q)) else {
            return false;
        };
        if same_table(&lt, table) {
            return false;
        }
        let (ln, rn) = (lt.name.as_str(), table.name.as_str());
        let by_fk = self.fks().iter().any(|fk| {
            (fk.table_name.eq_ignore_ascii_case(ln)
                && fk.column_name.eq_ignore_ascii_case(&l.column)
                && fk.referenced_table_name.eq_ignore_ascii_case(rn)
                && fk.referenced_column_name.eq_ignore_ascii_case(col))
                || (fk.referenced_table_name.eq_ignore_ascii_case(ln)
                    && fk.referenced_column_name.eq_ignore_ascii_case(&l.column)
                    && fk.table_name.eq_ignore_ascii_case(rn)
                    && fk.column_name.eq_ignore_ascii_case(col))
        });
        by_fk
            || (looks_like_fk_to(&l.column, rn) && col.eq_ignore_ascii_case("id"))
            || (l.column.eq_ignore_ascii_case("id") && looks_like_fk_to(col, ln))
    }

    // ----- provider -----

    fn all_tables(&mut self, boost: i32) {
        let tables: Vec<String> = self.cat.tables().to_vec();
        for t in tables {
            let insert = self.ident(&t);
            self.push(
                t.clone(),
                insert,
                &t,
                ItemKind::Table,
                Some("table".into()),
                boost,
            );
        }
    }

    /// `alias.|` / `tabel.|` / `schema.|`.
    fn member(&mut self) {
        let q = self.a.qualifier.last().cloned().unwrap_or_default();
        if self.a.expect == Expect::Table {
            self.all_tables(300);
            return;
        }
        let target = self.resolve(&q).or_else(|| {
            self.cat.columns(&q).map(|_| ScopeTable {
                schema: None,
                name: q.clone(),
                alias: None,
                kind: TableKind::Base,
                depth: 0,
                idx: usize::MAX,
                columns: Vec::new(),
                star_from: Vec::new(),
                dml_target: false,
            })
        });
        let Some(t) = target else {
            // qualifier tak dikenal → kemungkinan nama schema
            self.all_tables(150);
            return;
        };
        let suffix = if self.a.clause == Clause::UpdateSet {
            " = "
        } else {
            ""
        };
        let lhs_type = self
            .lhs_meta()
            .map(|(_, c)| classify(c.data_type.as_deref()));
        let lhs = self.a.lhs.clone();
        for (i, c) in self.table_columns(&t).into_iter().enumerate() {
            let mut boost = 500 - (i as i32).min(50);
            if self.a.expect == Expect::Value {
                if let Some(lt) = lhs_type {
                    boost += if compatible(lt, classify(c.data_type.as_deref())) {
                        100
                    } else {
                        -150
                    };
                }
                if self.is_fk_partner(lhs.as_ref(), &t, &c.name) {
                    boost += 300;
                }
            } else if self.a.clause == Clause::JoinOn
                && (c.name.eq_ignore_ascii_case("id") || self.fk_from(&t.name, &c.name).is_some())
            {
                boost += 80;
            }
            if self.a.used_columns.contains(&c.name.to_ascii_lowercase()) {
                boost -= 200;
            }
            let detail = self.column_detail(&t, &c);
            let insert = format!("{}{suffix}", self.ident(&c.name));
            self.push(
                c.name.clone(),
                insert,
                &c.name,
                ItemKind::Column,
                Some(detail),
                boost,
            );
        }
        if self.a.clause == Clause::SelectList && self.a.expect != Expect::Value {
            self.push(
                "*".into(),
                "*".into(),
                "*",
                ItemKind::Keyword,
                Some("all columns".into()),
                150,
            );
        }
    }

    /// Setelah FROM / JOIN / INTO / UPDATE.
    fn table_expect(&mut self) {
        if !self.a.qualifier.is_empty() {
            self.all_tables(300);
            return;
        }
        let ctes = self.a.ctes.clone();
        for c in &ctes {
            let insert = self.ident(&c.name);
            self.push(
                c.name.clone(),
                insert,
                &c.name,
                ItemKind::Cte,
                Some("CTE".into()),
                450,
            );
        }
        // Setelah JOIN: tabel yang punya FK ke tabel sebelumnya + kondisi ON siap pakai
        let mut related: Vec<String> = Vec::new();
        let prev = self.tables_before_cursor();
        if self.a.join_pending && !prev.is_empty() {
            let use_alias = self.uses_aliases();
            let on = self.kw("ON");
            for s in prev.iter().filter(|t| t.kind == TableKind::Base) {
                let ds = s.display().to_string();
                for fk in self.fks() {
                    // (tabel lain, kolom di tabel lain, kolom di tabel scope)
                    let (other, other_col, scope_col) =
                        if fk.table_name.eq_ignore_ascii_case(&s.name) {
                            (
                                &fk.referenced_table_name,
                                &fk.referenced_column_name,
                                &fk.column_name,
                            )
                        } else if fk.referenced_table_name.eq_ignore_ascii_case(&s.name) {
                            (&fk.table_name, &fk.column_name, &fk.referenced_column_name)
                        } else {
                            continue;
                        };
                    let tname = self.ident(other);
                    let alias = if use_alias {
                        self.gen_alias(other)
                    } else {
                        tname.clone()
                    };
                    let head = if alias == tname {
                        tname.clone()
                    } else {
                        format!("{tname} {alias}")
                    };
                    let text = format!(
                        "{head} {on} {alias}.{} = {ds}.{}",
                        self.ident(other_col),
                        self.ident(scope_col)
                    );
                    let detail = format!("FK join → {}", s.name);
                    self.push(
                        text.clone(),
                        text,
                        other,
                        ItemKind::JoinCondition,
                        Some(detail),
                        420,
                    );
                    if !related.iter().any(|r| r.eq_ignore_ascii_case(other)) {
                        related.push(other.clone());
                    }
                }
            }
        }
        let tables: Vec<String> = self.cat.tables().to_vec();
        for t in tables {
            let rel = related.iter().any(|r| r.eq_ignore_ascii_case(&t));
            let insert = self.ident(&t);
            let detail = if rel { "table · FK related" } else { "table" };
            self.push(
                t.clone(),
                insert,
                &t,
                ItemKind::Table,
                Some(detail.into()),
                if rel { 380 } else { 250 },
            );
        }
    }

    /// Kondisi join antara `target` dan tabel lain (FK, lalu heuristik nama).
    fn join_conditions(
        &self,
        target: &ScopeTable,
        others: &[&ScopeTable],
    ) -> Vec<(String, i32, &'static str)> {
        let mut out: Vec<(String, i32, &'static str)> = Vec::new();
        let td = target.display().to_string();
        let tcols = self.table_columns(target);
        let has_id = |cols: &[ColumnMeta]| cols.iter().any(|c| c.name.eq_ignore_ascii_case("id"));
        for o in others.iter().filter(|o| !same_table(o, target)) {
            let od = o.display().to_string();
            let cond =
                |a: &str, b: &str| format!("{td}.{} = {od}.{}", self.ident(a), self.ident(b));
            for fk in self.fks() {
                if fk.table_name.eq_ignore_ascii_case(&target.name)
                    && fk.referenced_table_name.eq_ignore_ascii_case(&o.name)
                {
                    out.push((cond(&fk.column_name, &fk.referenced_column_name), 600, "FK"));
                } else if fk.table_name.eq_ignore_ascii_case(&o.name)
                    && fk.referenced_table_name.eq_ignore_ascii_case(&target.name)
                {
                    out.push((cond(&fk.referenced_column_name, &fk.column_name), 600, "FK"));
                }
            }
            let ocols = self.table_columns(o);
            for c in &tcols {
                if looks_like_fk_to(&c.name, &o.name) && has_id(&ocols) {
                    out.push((cond(&c.name, "id"), 500, "by name"));
                }
            }
            for c in &ocols {
                if looks_like_fk_to(&c.name, &target.name) && has_id(&tcols) {
                    out.push((cond("id", &c.name), 500, "by name"));
                }
            }
            for c in &tcols {
                let l = c.name.to_ascii_lowercase();
                let keyish = l != "id"
                    && (l.ends_with("id")
                        || l.ends_with("_key")
                        || l.ends_with("_code")
                        || l.ends_with("_no"));
                if keyish && ocols.iter().any(|x| x.name.eq_ignore_ascii_case(&c.name)) {
                    out.push((cond(&c.name, &c.name), 400, "same name"));
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        out.retain(|(c, _, _)| seen.insert(c.to_ascii_lowercase()));
        out
    }

    /// Tepat setelah nama tabel.
    fn after_table(&mut self, joined: bool, aliased: bool) {
        let cur = self.current_table();
        match self.a.clause {
            Clause::InsertTarget => {
                if let Some(t) = self.a.dml_target.clone().or_else(|| cur.cloned()) {
                    let cols = self.table_columns(&t);
                    if !cols.is_empty() {
                        let list = cols
                            .iter()
                            .map(|c| self.ident(&c.name))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let values = self.kw("VALUES");
                        let insert = format!("({list}) {values} ({CURSOR_MARK})");
                        let label = truncate_label(&format!("({list}) {values} (…)"), 60);
                        self.push(
                            label,
                            insert,
                            "",
                            ItemKind::Template,
                            Some("all columns".into()),
                            400,
                        );
                    }
                }
                self.keywords(&["VALUES", "SELECT"], 300);
                if self.o.dialect != Dialect::MySql {
                    self.keyword("DEFAULT VALUES", 100);
                }
            }
            Clause::UpdateTarget => {
                self.keyword("SET", 400);
                if let Some(t) = cur.filter(|_| !aliased) {
                    let al = self.gen_alias(&t.name);
                    self.push(
                        al.clone(),
                        format!("{al} "),
                        &al,
                        ItemKind::Alias,
                        Some("alias".into()),
                        150,
                    );
                }
            }
            _ => {
                if !aliased {
                    if let Some(t) = cur.filter(|t| t.kind != TableKind::Derived) {
                        let al = self.gen_alias(&t.name);
                        self.push(
                            al.clone(),
                            format!("{al} "),
                            &al,
                            ItemKind::Alias,
                            Some("alias".into()),
                            200,
                        );
                    }
                    self.keyword("AS", 110);
                }
                let base = if joined { 250 } else { 350 };
                if joined {
                    self.keyword("ON", 460);
                    if let Some(t) = cur {
                        let others: Vec<&ScopeTable> = self
                            .tables_before_cursor()
                            .into_iter()
                            .filter(|o| o.idx < t.idx)
                            .collect();
                        let on = self.kw("ON");
                        for (cond, boost, why) in self.join_conditions(t, &others) {
                            let text = format!("{on} {cond}");
                            self.push(
                                text.clone(),
                                text,
                                &on,
                                ItemKind::JoinCondition,
                                Some(why.into()),
                                boost - 100,
                            );
                        }
                    }
                    self.keyword("USING", 150);
                }
                self.keyword("WHERE", base);
                let joins: &[&str] = match self.o.dialect {
                    Dialect::MySql | Dialect::Sqlite => &[
                        "JOIN",
                        "LEFT JOIN",
                        "INNER JOIN",
                        "RIGHT JOIN",
                        "CROSS JOIN",
                    ],
                    _ => &[
                        "JOIN",
                        "LEFT JOIN",
                        "INNER JOIN",
                        "RIGHT JOIN",
                        "FULL JOIN",
                        "CROSS JOIN",
                    ],
                };
                self.keywords(joins, base - 40);
                self.keywords(&["GROUP BY", "ORDER BY"], base - 80);
                if self.o.dialect != Dialect::MsSql {
                    self.keyword("LIMIT", base - 110);
                }
                self.keywords(&["HAVING", "UNION", "UNION ALL"], 60);
            }
        }
    }

    /// Kolom semua tabel scope. `qualify_always` memaksa `alias.kolom`.
    fn scope_columns(&mut self, base: i32, qualify_always: bool, lhs_type: Option<TypeClass>) {
        let scope: Vec<ScopeTable> = self.a.scope.clone();
        let multi = scope.iter().filter(|t| t.depth == 0).count() > 1;
        let per_table: Vec<Vec<ColumnMeta>> = scope.iter().map(|t| self.table_columns(t)).collect();
        let mut count: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (t, cols) in scope.iter().zip(&per_table) {
            if t.depth == 0 {
                for c in cols {
                    *count.entry(c.name.to_ascii_lowercase()).or_default() += 1;
                }
            }
        }
        let lhs = self.a.lhs.clone();
        let lhs_table = lhs
            .as_ref()
            .and_then(|l| l.qualifier.as_deref())
            .and_then(|q| self.resolve(q));
        let join_target = self.a.join_target.clone();
        let suffix = if self.a.clause == Clause::UpdateSet {
            " = "
        } else {
            ""
        };
        for (t, cols) in scope.iter().zip(per_table) {
            let q = t.display().to_string();
            for (i, c) in cols.into_iter().enumerate() {
                let lower = c.name.to_ascii_lowercase();
                let used = self.a.used_columns.contains(&lower);
                if used && matches!(self.a.clause, Clause::InsertColumns | Clause::UpdateSet) {
                    continue;
                }
                let ambiguous = count.get(&lower).copied().unwrap_or(0) > 1;
                let qualify = !q.is_empty() && (qualify_always || ambiguous || t.depth > 0);
                let mut boost = base - (i as i32).min(40);
                if t.depth > 0 {
                    boost -= 150;
                }
                if used {
                    boost -= 220;
                }
                if let Some(lt) = lhs_type {
                    boost += if compatible(lt, classify(c.data_type.as_deref())) {
                        120
                    } else {
                        -120
                    };
                }
                if let Some(l) = &lhs {
                    let lhs_here = match &lhs_table {
                        Some(lt) => same_table(lt, t),
                        None => l.qualifier.is_none() && !multi,
                    };
                    // `a = a` tidak berguna
                    if lhs_here && l.column.eq_ignore_ascii_case(&c.name) {
                        boost -= 400;
                    }
                    if self.is_fk_partner(Some(l), t, &c.name) {
                        boost += 350;
                    }
                }
                if join_target.as_ref().is_some_and(|jt| same_table(jt, t)) {
                    boost += 40;
                }
                let label = if qualify {
                    format!("{q}.{}", c.name)
                } else {
                    c.name.clone()
                };
                let insert = if qualify {
                    format!("{q}.{}{suffix}", self.ident(&c.name))
                } else {
                    format!("{}{suffix}", self.ident(&c.name))
                };
                let detail = self.column_detail(t, &c);
                self.push(
                    label,
                    insert,
                    &c.name,
                    ItemKind::Column,
                    Some(detail),
                    boost,
                );
            }
        }
    }

    /// Alias/nama tabel di scope (untuk mengetik `u` lalu `.`).
    fn scope_aliases(&mut self, boost: i32) {
        if self.a.partial.is_empty() {
            return;
        }
        let scope: Vec<ScopeTable> = self.depth0().cloned().collect();
        for t in scope {
            let d = t.display().to_string();
            if d.is_empty() {
                continue;
            }
            let detail = if t.alias.is_some() {
                format!("alias · {}", t.name)
            } else {
                "table".into()
            };
            self.push(
                d.clone(),
                d.clone(),
                &d,
                ItemKind::Alias,
                Some(detail),
                boost,
            );
        }
    }

    fn functions(&mut self, agg_boost: i32, other_boost: i32, allow_agg: bool) {
        if self.a.partial.is_empty() {
            return;
        }
        for f in functions(self.o.dialect) {
            let is_agg = AGGREGATES.contains(&f);
            if is_agg && !allow_agg {
                continue;
            }
            let name = self.kw(f);
            let insert = if ["ROW_NUMBER", "RANK", "DENSE_RANK"].contains(&f) {
                format!("{name}() {} ({CURSOR_MARK})", self.kw("OVER"))
            } else {
                format!("{name}({CURSOR_MARK})")
            };
            let (boost, note) = if is_agg {
                (agg_boost, "aggregate")
            } else {
                (other_boost, "function")
            };
            self.push(
                format!("{name}(…)"),
                insert,
                &name,
                ItemKind::Function,
                Some(note.into()),
                boost,
            );
        }
    }

    fn column_expect(&mut self) {
        if !self.a.qualifier.is_empty() {
            self.member();
            return;
        }
        let clause = self.a.clause;
        if matches!(clause, Clause::InsertColumns | Clause::UpdateSet) {
            let Some(t) = self.a.dml_target.clone() else {
                self.scope_columns(400, false, None);
                return;
            };
            let cols = self.table_columns(&t);
            let remaining: Vec<&ColumnMeta> = cols
                .iter()
                .filter(|c| !self.a.used_columns.contains(&c.name.to_ascii_lowercase()))
                .collect();
            if clause == Clause::InsertColumns && self.a.partial.is_empty() && remaining.len() > 1 {
                let list = remaining
                    .iter()
                    .map(|c| self.ident(&c.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                let label = truncate_label(&list, 60);
                self.push(
                    label,
                    list,
                    "",
                    ItemKind::Template,
                    Some("all remaining columns".into()),
                    300,
                );
            }
            let suffix = if clause == Clause::UpdateSet {
                " = "
            } else {
                ""
            };
            for (i, c) in remaining.into_iter().enumerate() {
                let insert = format!("{}{suffix}", self.ident(&c.name));
                let detail = self.column_detail(&t, c);
                self.push(
                    c.name.clone(),
                    insert,
                    &c.name,
                    ItemKind::Column,
                    Some(detail),
                    400 - i as i32,
                );
            }
            return;
        }

        // Argumen COUNT(...)
        if self.a.in_function.as_deref() == Some("COUNT") && self.a.partial.is_empty() {
            self.push(
                "*".into(),
                "*".into(),
                "*",
                ItemKind::Keyword,
                Some("all rows".into()),
                700,
            );
            self.keyword("DISTINCT", 450);
        }

        match clause {
            Clause::SelectList => {
                if self.a.in_function.is_none()
                    && (self.a.right_after_select || self.a.partial.is_empty())
                {
                    self.push(
                        "*".into(),
                        "*".into(),
                        "*",
                        ItemKind::Keyword,
                        Some("all columns".into()),
                        450,
                    );
                    self.expand_star_template();
                }
                if self.a.right_after_select {
                    self.keyword("DISTINCT", 250);
                    if self.o.dialect == Dialect::MsSql {
                        self.keyword("TOP", 120);
                    }
                }
                self.scope_columns(400, false, None);
                self.scope_aliases(150);
                self.functions(260, 160, true);
                if !self.a.partial.is_empty() {
                    self.keywords(&["CASE", "NOT", "NULL", "EXISTS"], 60);
                }
            }
            Clause::GroupBy => {
                let used = &self.a.used_columns;
                let non_agg: Vec<String> = self
                    .a
                    .select_items
                    .iter()
                    .filter(|s| !s.aggregate && !s.is_star && !s.text.is_empty())
                    .map(|s| s.text.clone())
                    .filter(|t| {
                        !used.contains(&t.rsplit('.').next().unwrap_or(t).to_ascii_lowercase())
                    })
                    .collect();
                if non_agg.len() > 1 && self.a.partial.is_empty() {
                    let all = non_agg.join(", ");
                    let label = truncate_label(&all, 60);
                    self.push(
                        label,
                        all,
                        "",
                        ItemKind::Template,
                        Some("all non-aggregated".into()),
                        600,
                    );
                }
                for (i, t) in non_agg.iter().enumerate() {
                    let filter = t.rsplit('.').next().unwrap_or(t).to_string();
                    self.push(
                        t.clone(),
                        t.clone(),
                        &filter,
                        ItemKind::Column,
                        Some("from SELECT".into()),
                        500 - i as i32,
                    );
                }
                self.scope_columns(350, false, None);
                self.scope_aliases(120);
                self.functions(0, 100, false);
            }
            Clause::OrderBy => {
                let aliases: Vec<String> = self
                    .a
                    .select_items
                    .iter()
                    .filter(|s| s.has_alias)
                    .filter_map(|s| s.name.clone())
                    .collect();
                for al in aliases {
                    let ins = self.ident(&al);
                    self.push(
                        al.clone(),
                        ins,
                        &al,
                        ItemKind::Alias,
                        Some("select alias".into()),
                        480,
                    );
                }
                self.scope_columns(380, false, None);
                self.scope_aliases(120);
                self.functions(200, 100, true);
            }
            Clause::Having => {
                let aggs: Vec<String> = self
                    .a
                    .select_items
                    .iter()
                    .filter(|s| s.aggregate && !s.text.is_empty())
                    .map(|s| s.text.clone())
                    .collect();
                for (i, t) in aggs.iter().enumerate() {
                    let detail = Some("aggregate from SELECT".to_string());
                    self.push(
                        t.clone(),
                        t.clone(),
                        t,
                        ItemKind::Column,
                        detail,
                        480 - i as i32,
                    );
                }
                self.functions(420, 100, true);
                self.scope_columns(300, false, None);
                self.scope_aliases(120);
            }
            Clause::JoinOn => {
                self.scope_columns(400, true, None);
                self.scope_aliases(150);
                self.functions(0, 80, false);
            }
            _ => {
                let multi_scope = self.depth0().count() > 1;
                self.scope_columns(400, false, None);
                self.scope_aliases(if multi_scope { 200 } else { 120 });
                let agg_ok = clause != Clause::Where;
                self.functions(if agg_ok { 150 } else { 0 }, 120, agg_ok);
                if !self.a.partial.is_empty() {
                    self.keywords(&["NOT", "EXISTS", "CASE", "NULL"], 60);
                }
            }
        }
        if self.a.allow_subquery {
            self.keyword("SELECT", 300);
        }
    }

    /// `* → id, name, …`: ekspansi semua kolom scope.
    fn expand_star_template(&mut self) {
        let scope: Vec<ScopeTable> = self.depth0().cloned().collect();
        let multi = scope.len() > 1;
        let mut parts = Vec::new();
        for t in &scope {
            for c in self.table_columns(t) {
                let col = self.ident(&c.name);
                parts.push(if multi && !t.display().is_empty() {
                    format!("{}.{col}", t.display())
                } else {
                    col
                });
            }
        }
        if parts.len() < 2 {
            return;
        }
        let all = parts.join(", ");
        let label = truncate_label(&format!("* → {all}"), 60);
        self.push(
            label,
            all,
            "",
            ItemKind::Template,
            Some("expand all columns".into()),
            200,
        );
    }

    fn join_condition_expect(&mut self) {
        if !self.a.qualifier.is_empty() {
            self.member();
            return;
        }
        if let Some(t) = self.a.join_target.clone() {
            let others: Vec<ScopeTable> =
                self.depth0().filter(|o| o.idx < t.idx).cloned().collect();
            let refs: Vec<&ScopeTable> = others.iter().collect();
            for (cond, boost, why) in self.join_conditions(&t, &refs) {
                let filter = cond.clone();
                self.push(
                    cond.clone(),
                    cond,
                    &filter,
                    ItemKind::JoinCondition,
                    Some(why.into()),
                    boost,
                );
            }
        }
        self.scope_columns(300, true, None);
        self.scope_aliases(150);
        if !self.a.partial.is_empty() {
            self.keywords(&["NOT", "EXISTS"], 40);
        }
    }

    fn operator_expect(&mut self) {
        let ty = self
            .lhs_meta()
            .map(|(_, c)| classify(c.data_type.as_deref()))
            .unwrap_or(TypeClass::Other);
        let symbols: &[&str] = if ty == TypeClass::Bool {
            &["=", "<>"]
        } else {
            &["=", "<>", "<", ">", "<=", ">="]
        };
        for (i, op) in symbols.iter().enumerate() {
            self.push(
                (*op).into(),
                format!("{op} "),
                op,
                ItemKind::Operator,
                None,
                450 - i as i32 * 5,
            );
        }
        let textual = matches!(ty, TypeClass::Text | TypeClass::Other);
        let ranged = matches!(ty, TypeClass::Num | TypeClass::Time | TypeClass::Other);
        let mut word_ops: Vec<(&str, String, i32)> = vec![
            ("IN (…)", format!("{} ({CURSOR_MARK})", self.kw("IN")), 380),
            ("IS NULL", format!("{} ", self.kw("IS NULL")), 370),
            ("IS NOT NULL", format!("{} ", self.kw("IS NOT NULL")), 365),
            (
                "NOT IN (…)",
                format!("{} ({CURSOR_MARK})", self.kw("NOT IN")),
                300,
            ),
        ];
        if textual {
            word_ops.push(("LIKE", format!("{} '{CURSOR_MARK}'", self.kw("LIKE")), 390));
            word_ops.push((
                "NOT LIKE",
                format!("{} '{CURSOR_MARK}'", self.kw("NOT LIKE")),
                280,
            ));
            if self.o.dialect == Dialect::Postgres {
                word_ops.push((
                    "ILIKE",
                    format!("{} '{CURSOR_MARK}'", self.kw("ILIKE")),
                    360,
                ));
            }
        }
        if ranged {
            let ins = format!("{} {CURSOR_MARK} {} ", self.kw("BETWEEN"), self.kw("AND"));
            word_ops.push(("BETWEEN … AND …", ins, 320));
        }
        if ty == TypeClass::Bool {
            word_ops.push(("IS TRUE", format!("{} ", self.kw("IS TRUE")), 360));
            word_ops.push(("IS FALSE", format!("{} ", self.kw("IS FALSE")), 355));
        }
        for (label, insert, boost) in word_ops {
            let l = self.kw(label);
            self.push(
                l.clone(),
                insert,
                &l,
                ItemKind::Operator,
                Some("operator".into()),
                boost,
            );
        }
    }

    fn value_expect(&mut self) {
        if !self.a.qualifier.is_empty() {
            self.member();
            return;
        }
        let meta = self.lhs_meta();
        let ty = meta.as_ref().map(|(_, c)| classify(c.data_type.as_deref()));
        match ty {
            Some(TypeClass::Bool) => {
                for (i, v) in ["TRUE", "FALSE"].iter().enumerate() {
                    let k = self.kw(v);
                    self.push(
                        k.clone(),
                        k,
                        v,
                        ItemKind::Value,
                        Some("boolean".into()),
                        460 - i as i32 * 5,
                    );
                }
            }
            Some(TypeClass::Time) => {
                for (i, v) in ["CURRENT_DATE", "CURRENT_TIMESTAMP"].iter().enumerate() {
                    let k = self.kw(v);
                    self.push(
                        k.clone(),
                        k,
                        v,
                        ItemKind::Value,
                        Some("date/time".into()),
                        440 - i as i32 * 5,
                    );
                }
                let now = match self.o.dialect {
                    Dialect::MsSql => Some("GETDATE()"),
                    Dialect::MySql | Dialect::Postgres => Some("NOW()"),
                    _ => None,
                };
                if let Some(n) = now {
                    let k = self.kw(n);
                    self.push(
                        k.clone(),
                        k,
                        n,
                        ItemKind::Function,
                        Some("date/time".into()),
                        420,
                    );
                }
            }
            Some(TypeClass::Text) if self.a.partial.is_empty() => {
                let ins = format!("'{CURSOR_MARK}'");
                self.push(
                    "'…'".into(),
                    ins,
                    "",
                    ItemKind::Value,
                    Some("text literal".into()),
                    360,
                );
            }
            _ => {}
        }
        if self.a.clause == Clause::Values {
            self.keywords(&["NULL", "DEFAULT"], 200);
            return;
        }
        if self.a.allow_subquery {
            self.keyword("SELECT", 320);
        }
        let lhs_type = ty.filter(|t| *t != TypeClass::Other);
        let qualify = self.depth0().count() > 1;
        self.scope_columns(250, qualify, lhs_type);
        self.scope_aliases(120);
        self.functions(0, 100, false);
    }

    fn after_expr(&mut self) {
        let limit = self.o.dialect != Dialect::MsSql;
        match self.a.clause {
            Clause::JoinOn => {
                self.keywords(&["AND", "OR"], 420);
                self.keyword("WHERE", 400);
                self.keywords(&["JOIN", "LEFT JOIN", "INNER JOIN"], 330);
                self.keywords(&["GROUP BY", "ORDER BY"], 260);
                if limit {
                    self.keyword("LIMIT", 200);
                }
            }
            Clause::Having => {
                self.keywords(&["AND", "OR", "ORDER BY"], 400);
                if limit {
                    self.keyword("LIMIT", 300);
                }
            }
            Clause::UpdateSet => self.keyword("WHERE", 400),
            _ => {
                self.keywords(&["AND", "OR"], 420);
                self.keywords(&["GROUP BY", "ORDER BY"], 330);
                if limit {
                    self.keyword("LIMIT", 300);
                }
                self.keywords(&["UNION", "UNION ALL"], 100);
            }
        }
    }

    fn generic(&mut self) {
        if !self.a.partial.is_empty() {
            self.keywords(GENERIC_KW, 100);
        }
        self.all_tables(150);
        self.scope_columns(120, false, None);
    }

    fn run(mut self) -> Vec<CompletionItem> {
        match self.a.expect.clone() {
            Expect::None => {}
            Expect::StatementStart => self.keywords(START_KW, 400),
            Expect::Table => self.table_expect(),
            Expect::AfterTable { .. } if !self.a.qualifier.is_empty() => self.member(),
            Expect::AfterTable { joined, aliased } => self.after_table(joined, aliased),
            Expect::Column => self.column_expect(),
            Expect::JoinCondition => self.join_condition_expect(),
            Expect::Operator => self.operator_expect(),
            Expect::Value => self.value_expect(),
            Expect::AfterExpr => self.after_expr(),
            Expect::AfterSelectItem => {
                self.keyword("FROM", 450);
                self.keyword("AS", 250);
            }
            Expect::Keywords(list) => self.keywords(list, 450),
            Expect::Generic => self.generic(),
        }
        let mut out = self.out;
        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.label.len().cmp(&b.label.len()))
                .then_with(|| a.label.cmp(&b.label))
        });
        let mut seen = std::collections::HashSet::new();
        out.retain(|i| seen.insert(i.label.to_ascii_lowercase()));
        out.truncate(200);
        out
    }
}

/// Hasilkan kandidat completion terurut untuk hasil analisis `a`.
pub fn complete(a: &Analysis, cat: &dyn Catalog, opts: Options) -> Vec<CompletionItem> {
    Builder {
        a,
        cat,
        o: opts,
        out: Vec::new(),
    }
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autocomplete::analyzer::analyze;

    struct Mock {
        tables: Vec<String>,
        cols: Vec<(String, Vec<ColumnMeta>)>,
        fks: Vec<ForeignKey>,
    }

    impl Catalog for Mock {
        fn tables(&self) -> &[String] {
            &self.tables
        }
        fn columns(&self, table: &str) -> Option<&[ColumnMeta]> {
            self.cols
                .iter()
                .find(|(t, _)| t.eq_ignore_ascii_case(table))
                .map(|(_, c)| c.as_slice())
        }
        fn foreign_keys(&self) -> &[ForeignKey] {
            &self.fks
        }
    }

    fn col(name: &str, ty: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            data_type: Some(ty.into()),
        }
    }

    fn fk(t: &str, c: &str, rt: &str, rc: &str) -> ForeignKey {
        ForeignKey {
            constraint_name: format!("fk_{t}_{c}"),
            table_name: t.into(),
            column_name: c.into(),
            referenced_table_name: rt.into(),
            referenced_column_name: rc.into(),
        }
    }

    fn mock() -> Mock {
        Mock {
            tables: vec![
                "users".into(),
                "orders".into(),
                "order_items".into(),
                "products".into(),
                "audit_log".into(),
            ],
            cols: vec![
                (
                    "users".into(),
                    vec![
                        col("id", "int"),
                        col("name", "varchar"),
                        col("email", "varchar"),
                        col("active", "boolean"),
                        col("created_at", "timestamp"),
                    ],
                ),
                (
                    "orders".into(),
                    vec![
                        col("id", "int"),
                        col("user_id", "int"),
                        col("total", "decimal"),
                        col("status", "varchar"),
                        col("created_at", "timestamp"),
                    ],
                ),
                (
                    "order_items".into(),
                    vec![
                        col("id", "int"),
                        col("order_id", "int"),
                        col("product_id", "int"),
                        col("qty", "int"),
                    ],
                ),
                (
                    "products".into(),
                    vec![
                        col("id", "int"),
                        col("title", "varchar"),
                        col("price", "decimal"),
                    ],
                ),
            ],
            fks: vec![
                fk("orders", "user_id", "users", "id"),
                fk("order_items", "order_id", "orders", "id"),
                fk("order_items", "product_id", "products", "id"),
            ],
        }
    }

    fn run(sql_with_cursor: &str) -> Vec<CompletionItem> {
        let cursor = sql_with_cursor.find('|').unwrap();
        let sql = sql_with_cursor.replacen('|', "", 1);
        let a = analyze(&sql, cursor, Dialect::Postgres);
        complete(
            &a,
            &mock(),
            Options {
                dialect: Dialect::Postgres,
                casing: KeywordCasing::Upper,
            },
        )
    }

    fn labels(items: &[CompletionItem]) -> Vec<String> {
        items.iter().map(|i| i.label.clone()).collect()
    }

    fn top(sql: &str, n: usize) -> Vec<String> {
        labels(&run(sql)).into_iter().take(n).collect()
    }

    #[test]
    fn fuzzy_prefix_beats_subsequence() {
        assert!(
            fuzzy_match("cust", "customer_name").unwrap()
                > fuzzy_match("cnm", "customer_name").unwrap()
        );
        assert!(fuzzy_match("cnm", "customer_name").is_some());
        assert!(fuzzy_match("zzz", "customer_name").is_none());
        assert_eq!(fuzzy_match("", "x"), Some(0));
    }

    #[test]
    fn from_suggests_tables_not_columns() {
        let items = run("SELECT * FROM |");
        assert!(
            items
                .iter()
                .all(|i| matches!(i.kind, ItemKind::Table | ItemKind::Cte))
        );
        assert!(labels(&items).contains(&"users".to_string()));
    }

    #[test]
    fn after_from_table_suggests_clauses_and_alias() {
        let l = labels(&run("SELECT * FROM users |"));
        assert!(l.contains(&"WHERE".to_string()));
        assert!(l.contains(&"LEFT JOIN".to_string()));
        assert!(l.contains(&"u".to_string()), "alias otomatis: {l:?}");
        assert!(
            !l.contains(&"email".to_string()),
            "kolom tidak valid di sini"
        );
        assert_eq!(top("SELECT * FROM users w|", 1), vec!["WHERE"]);
    }

    #[test]
    fn join_table_uses_foreign_keys() {
        let items = run("SELECT * FROM users u JOIN |");
        assert_eq!(items[0].kind, ItemKind::JoinCondition);
        assert_eq!(items[0].label, "orders o ON o.user_id = u.id");
        let l = labels(&items);
        let pos_orders = l.iter().position(|x| x == "orders").unwrap();
        let pos_audit = l.iter().position(|x| x == "audit_log").unwrap();
        assert!(pos_orders < pos_audit);
        // setelah koma bukan JOIN → tidak ada kombinasi ON
        assert!(
            run("SELECT * FROM users u, |")
                .iter()
                .all(|i| i.kind != ItemKind::JoinCondition)
        );
    }

    #[test]
    fn after_join_table_offers_on_with_condition() {
        let l = labels(&run("SELECT * FROM users u JOIN orders o |"));
        assert_eq!(l[0], "ON o.user_id = u.id", "{l:?}");
        assert!(l.contains(&"ON".to_string()));
    }

    #[test]
    fn on_clause_prefers_fk_condition_then_qualified_columns() {
        let items = run("SELECT * FROM users u JOIN orders o ON |");
        assert_eq!(items[0].label, "o.user_id = u.id");
        assert!(
            items
                .iter()
                .filter(|i| i.kind == ItemKind::Column)
                .all(|i| i.label.contains('.'))
        );
        // join 3 tabel: kondisi hanya untuk tabel yang baru di-join
        let items =
            run("SELECT * FROM users u JOIN orders o ON o.user_id = u.id JOIN order_items oi ON |");
        assert_eq!(items[0].label, "oi.order_id = o.id");
        assert!(!labels(&items).iter().any(|l| l == "o.user_id = u.id"));
    }

    #[test]
    fn on_value_prefers_fk_partner() {
        assert_eq!(
            top("SELECT * FROM users u JOIN orders o ON o.user_id = |", 1),
            vec!["u.id"]
        );
    }

    #[test]
    fn where_columns_then_operators_then_values() {
        let l = labels(&run("SELECT * FROM users WHERE |"));
        assert!(l[..5].contains(&"id".to_string()), "{l:?}");
        let ops = labels(&run("SELECT * FROM users WHERE email |"));
        assert_eq!(ops[0], "=");
        assert!(ops.contains(&"LIKE".to_string()));
        assert!(ops.contains(&"IS NULL".to_string()));
        let ops = labels(&run("SELECT * FROM users WHERE active |"));
        assert!(!ops.contains(&"LIKE".to_string()));
        assert_eq!(
            top("SELECT * FROM users WHERE active = |", 2),
            vec!["TRUE", "FALSE"]
        );
        assert_eq!(
            top("SELECT * FROM users WHERE created_at > |", 1),
            vec!["CURRENT_DATE"]
        );
        let after = labels(&run("SELECT * FROM users WHERE id = 1 |"));
        assert_eq!(&after[..2], &["AND".to_string(), "OR".to_string()]);
        // mengetik "li" setelah kolom → LIKE
        assert_eq!(top("SELECT * FROM users WHERE email li|", 1), vec!["LIKE"]);
    }

    #[test]
    fn select_list_qualifies_ambiguous_columns() {
        let l = labels(&run(
            "SELECT | FROM users u JOIN orders o ON o.user_id = u.id",
        ));
        assert_eq!(l[0], "*");
        assert!(l.contains(&"u.id".to_string()) && l.contains(&"o.id".to_string()));
        assert!(
            l.contains(&"email".to_string()),
            "kolom unik tanpa qualifier"
        );
        assert!(l.contains(&"total".to_string()));
        // kolom yang sudah dipilih turun peringkat
        let l = labels(&run("SELECT email, | FROM users"));
        let pos = |c: &str| l.iter().position(|x| x == c).unwrap();
        assert!(pos("email") > pos("name"), "{l:?}");
    }

    #[test]
    fn member_access_by_alias_cte_and_derived() {
        let l = labels(&run("SELECT o.| FROM orders o"));
        assert_eq!(&l[..2], &["id".to_string(), "user_id".to_string()]);
        let l = labels(&run(
            "WITH t AS (SELECT id, name AS nm FROM users) SELECT t.| FROM t",
        ));
        assert_eq!(&l[..2], &["id".to_string(), "nm".to_string()]);
        let l = labels(&run(
            "SELECT d.| FROM (SELECT u.*, 1 AS one FROM users u) d",
        ));
        assert!(l.contains(&"one".to_string()) && l.contains(&"email".to_string()));
        assert_eq!(top("SELECT users.em|", 1), vec!["email"]);
    }

    #[test]
    fn group_by_and_order_by_use_select_list() {
        let items = run("SELECT u.name, u.email, count(*) AS n FROM users u GROUP BY |");
        assert_eq!(items[0].kind, ItemKind::Template);
        assert_eq!(items[0].insert, "u.name, u.email");
        assert_eq!(
            top("SELECT u.name, count(*) AS n FROM users u ORDER BY |", 1),
            vec!["n"]
        );
        assert_eq!(
            top("SELECT name, count(*) FROM users GROUP BY name HAVING |", 1),
            vec!["count(*)"]
        );
    }

    #[test]
    fn insert_and_update() {
        let items = run("INSERT INTO users |");
        assert_eq!(items[0].kind, ItemKind::Template);
        assert!(
            items[0]
                .insert
                .starts_with("(id, name, email, active, created_at) VALUES (")
        );
        let l = labels(&run("INSERT INTO users (id, name, |"));
        assert!(!l.contains(&"id".to_string()) && !l.contains(&"name".to_string()));
        assert!(l.contains(&"email".to_string()));
        let items = run("UPDATE users SET |");
        assert_eq!(
            items.iter().find(|i| i.label == "email").unwrap().insert,
            "email = "
        );
        assert_eq!(
            top("INSERT INTO users (id, active) VALUES (1, |", 2),
            vec!["TRUE", "FALSE"]
        );
        assert_eq!(top("UPDATE users SET name = 'x' |", 1), vec!["WHERE"]);
    }

    #[test]
    fn keyword_positions_are_narrow() {
        assert_eq!(top("SELECT a FROM t GROUP |", 1), vec!["BY"]);
        assert_eq!(top("SELECT id |", 1), vec!["FROM"]);
        assert_eq!(top("sel|", 1), vec!["SELECT"]);
        assert!(run("SELECT a AS |").is_empty());
        assert!(run("SELECT * FROM users WHERE name = 'jo|'").is_empty());
    }

    #[test]
    fn identifiers_are_quoted_when_needed() {
        let cat = Mock {
            tables: vec!["Order Details".into(), "select".into(), "Users".into()],
            cols: vec![],
            fks: vec![],
        };
        let a = analyze("SELECT * FROM ", 14, Dialect::Postgres);
        let items = complete(
            &a,
            &cat,
            Options {
                dialect: Dialect::Postgres,
                casing: KeywordCasing::Upper,
            },
        );
        let ins: Vec<&str> = items.iter().map(|i| i.insert.as_str()).collect();
        assert!(ins.contains(&"\"Order Details\""));
        assert!(ins.contains(&"\"select\""));
        assert!(
            ins.contains(&"\"Users\""),
            "Postgres: huruf besar wajib di-quote"
        );
        let a = analyze("SELECT * FROM ", 14, Dialect::MySql);
        let items = complete(
            &a,
            &cat,
            Options {
                dialect: Dialect::MySql,
                casing: KeywordCasing::Lower,
            },
        );
        assert!(items.iter().any(|i| i.insert == "`Order Details`"));
        assert!(items.iter().any(|i| i.insert == "Users"));
    }
}
