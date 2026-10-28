//! Pemeriksaan semantik DML terhadap skema di cache: tabel/view tidak ada,
//! kolom tidak ada, qualifier/alias tidak dikenal, kolom ambigu, beda huruf
//! besar identifier PostgreSQL, jumlah nilai INSERT, dan jumlah kolom UNION.
//!
//! Scope tabel/alias/CTE berasal dari analyzer autocomplete
//! (`autocomplete::analyzer::analyze`), per cabang set-operation dan per
//! subquery. Subquery melihat scope luarnya (correlated reference).
//!
//! Prinsip presisi sama dengan `index_advisor`: tabel yang kolomnya belum
//! diketahui, CTE/derived table, objek yang dibuat di skrip yang sama, dan
//! tabel di schema non-default tidak diperiksa.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use super::syntax::statement_tokens;
use super::words;
use super::{DiagCode, DiagSource, SchemaSnapshot, diag};
use crate::autocomplete::analyzer::{ScopeTable, TableKind, analyze};
use crate::autocomplete::lexer::{Dialect, TokKind, Token, statement_bounds, tokenize};
use crate::query_tools::{LintMessage, LintSeverity};

/// Objek yang didefinisikan statement sebelumnya di teks editor yang sama.
#[derive(Clone, Debug, Default)]
pub struct ScriptState {
    /// Tabel/view yang dibuat (lowercase, tanpa schema).
    pub created: HashSet<String>,
    /// Kolom yang ditambahkan `ALTER TABLE ... ADD` (key tabel lowercase).
    pub added_columns: HashMap<String, Vec<String>>,
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, TokKind::Word | TokKind::QuotedIdent)
}

fn non_comment(toks: Vec<Token>) -> Vec<Token> {
    toks.into_iter()
        .filter(|t| t.kind != TokKind::Comment)
        .collect()
}

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

/// Index token terakhir rantai `a.b.c` yang dimulai di `i`.
fn chain_end(toks: &[Token], i: usize) -> usize {
    let mut j = i;
    while toks
        .get(j + 1)
        .is_some_and(|d| d.kind == TokKind::Dot && d.start == toks[j].end)
        && toks.get(j + 2).is_some_and(is_name)
    {
        j += 2;
    }
    j
}

/// Catat tabel/kolom yang dibuat statement DDL supaya statement berikutnya
/// di skrip yang sama tidak ditandai salah.
pub fn record_definitions(stmt: &str, dialect: Dialect, script: &mut ScriptState) {
    let toks = non_comment(tokenize(stmt, dialect));
    let kw = |i: usize, k: &str| toks.get(i).is_some_and(|t| t.is_kw(k));
    let name_at = |i: usize| -> Option<(String, usize)> {
        let t = toks.get(i).filter(|t| is_name(t))?;
        let _ = t;
        let j = chain_end(&toks, i);
        Some((toks[j].text.to_ascii_lowercase(), j))
    };
    let skip_if_exists = |mut i: usize, not: bool| {
        if kw(i, "IF") && (!not || kw(i + 1, "NOT")) {
            i += if not { 3 } else { 2 };
        }
        i
    };
    if kw(0, "CREATE") {
        let mut i = 1;
        while toks.get(i).is_some_and(|t| {
            [
                "OR",
                "REPLACE",
                "GLOBAL",
                "LOCAL",
                "TEMP",
                "TEMPORARY",
                "UNLOGGED",
                "MATERIALIZED",
                "RECURSIVE",
                "SECURE",
                "FOREIGN",
                "VIRTUAL",
            ]
            .iter()
            .any(|k| t.is_kw(k))
        }) {
            i += 1;
        }
        if kw(i, "TABLE") || kw(i, "VIEW") {
            if let Some((name, _)) = name_at(skip_if_exists(i + 1, true)) {
                script.created.insert(name);
            }
        }
    } else if kw(0, "ALTER") && kw(1, "TABLE") {
        let mut i = skip_if_exists(2, false);
        if kw(i, "ONLY") {
            i += 1;
        }
        let Some((table, mut j)) = name_at(i) else {
            return;
        };
        j += 1;
        while j < toks.len() {
            if kw(j, "ADD") {
                let mut k = j + 1;
                if kw(k, "COLUMN") {
                    k += 1;
                }
                k = skip_if_exists(k, true);
                if let Some(t) = toks.get(k).filter(|t| is_name(t))
                    && !words::is_not_column(&t.text)
                {
                    script
                        .added_columns
                        .entry(table.clone())
                        .or_default()
                        .push(t.text.clone());
                }
            } else if kw(j, "RENAME") {
                // `RENAME TO baru` / `RENAME COLUMN a TO b`
                if let Some(to) = (j + 1..toks.len().min(j + 5)).find(|&k| kw(k, "TO"))
                    && let Some(t) = toks.get(to + 1).filter(|t| is_name(t))
                {
                    if kw(j + 1, "TO") {
                        script.created.insert(t.text.to_ascii_lowercase());
                    } else {
                        script
                            .added_columns
                            .entry(table.clone())
                            .or_default()
                            .push(t.text.clone());
                    }
                }
            }
            j += 1;
        }
    } else if kw(0, "RENAME") && kw(1, "TABLE") {
        for (i, t) in toks.iter().enumerate() {
            if t.is_kw("TO")
                && let Some((name, _)) = name_at(i + 1)
            {
                script.created.insert(name);
            }
        }
    } else if kw(0, "SELECT") && dialect != Dialect::MySql {
        // `SELECT ... INTO tabel` (PostgreSQL / MsSQL)
        let mut depth = 0i32;
        for (i, t) in toks.iter().enumerate() {
            match t.kind {
                TokKind::LParen => depth += 1,
                TokKind::RParen => depth -= 1,
                _ if depth == 0 && t.is_kw("INTO") => {
                    let mut k = i + 1;
                    while toks.get(k).is_some_and(|t| {
                        t.is_kw("TEMP")
                            || t.is_kw("TEMPORARY")
                            || t.is_kw("UNLOGGED")
                            || t.is_kw("TABLE")
                    }) {
                        k += 1;
                    }
                    if let Some((name, _)) = name_at(k) {
                        script.created.insert(name);
                    }
                    break;
                }
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Select,
    Insert,
    Update,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Section {
    None,
    Select,
    From,
    Cond,
    /// Setelah `INSERT INTO`: nama tabel lalu daftar kolom.
    InsertTarget,
    /// `ON CONFLICT ... DO UPDATE SET`, `ON DUPLICATE KEY UPDATE`, `SET` (MySQL).
    InsertTail,
    Values,
    Skip,
}

/// Satu tabel di scope beserta lokasi namanya di teks.
#[derive(Clone, Debug)]
struct Entry {
    table: ScopeTable,
    /// Rentang byte segmen terakhir nama tabel.
    span: Range<usize>,
    quoted: bool,
    /// Nama yang ditulis dengan qualifier schema non-default, tabel sementara
    /// `#tmp`, atau fungsi tabel: kolomnya tidak diperiksa.
    opaque: bool,
}

impl Entry {
    fn display(&self) -> String {
        match &self.table.alias {
            Some(a) => format!("{} ({a})", self.table.name),
            None => self.table.name.clone(),
        }
    }
}

#[derive(Default)]
struct Frame {
    entries: Vec<Entry>,
}

enum ColMatch {
    Exact,
    /// Hanya beda huruf besar (PostgreSQL); berisi nama sebenarnya.
    Case(String),
    No,
}

struct Ctx<'a> {
    text: &'a str,
    dialect: Dialect,
    schema: &'a SchemaSnapshot,
    script: &'a ScriptState,
    out: &'a mut Vec<LintMessage>,
    missing: &'a mut BTreeSet<String>,
    kind: Kind,
    /// Statement memakai `NATURAL JOIN` / `JOIN ... USING`: kolom gabungan
    /// tidak ambigu, jadi pemeriksaan ambigu dimatikan.
    merges_columns: bool,
    /// Nama output/alias yang boleh dirujuk tanpa qualifier (ORDER BY, dsb.).
    aliases: HashSet<String>,
}

/// Qualifier khusus yang bukan tabel di FROM.
const PSEUDO_QUALIFIERS: &[&str] = &["excluded", "new", "old", "inserted", "deleted"];

/// Periksa satu statement (rentang isi `body` di `text`).
pub fn check(
    text: &str,
    body: Range<usize>,
    dialect: Dialect,
    schema: &SchemaSnapshot,
    script: &ScriptState,
    out: &mut Vec<LintMessage>,
    missing: &mut BTreeSet<String>,
) {
    let toks = non_comment(statement_tokens(text, &body, dialect));
    let mut start = 0;
    // `EXPLAIN [ANALYZE] [VERBOSE] [(opsi)] SELECT ...`
    if toks
        .first()
        .is_some_and(|t| t.is_kw("EXPLAIN") || t.is_kw("DESCRIBE") || t.is_kw("DESC"))
    {
        let mut depth = 0;
        start = toks.len();
        for (i, t) in toks.iter().enumerate().skip(1) {
            match t.kind {
                TokKind::LParen => depth += 1,
                TokKind::RParen => depth -= 1,
                TokKind::Word if depth == 0 && super::is_dml_word(&t.text.to_ascii_uppercase()) => {
                    start = i;
                    break;
                }
                _ => {}
            }
        }
    }
    let Some(first) = toks.get(start) else {
        return;
    };
    let kind = match first.text.to_ascii_uppercase().as_str() {
        "SELECT" | "WITH" => Kind::Select,
        "INSERT" | "REPLACE" => Kind::Insert,
        "UPDATE" => Kind::Update,
        "DELETE" => Kind::Delete,
        _ => return,
    };
    let toks = &toks[start..];
    // `WITH ... INSERT/UPDATE/DELETE` (CTE di depan DML)
    let kind = if kind == Kind::Select && first.is_kw("WITH") {
        main_keyword(toks).unwrap_or(kind)
    } else {
        kind
    };
    let merges_columns = toks.iter().any(|t| t.is_kw("NATURAL") || t.is_kw("USING"));
    let aliases = collect_aliases(toks);
    let mut ctx = Ctx {
        text,
        dialect,
        schema,
        script,
        out,
        missing,
        kind,
        merges_columns,
        aliases,
    };
    let ctes = cte_names(toks);
    ctx.block(toks, &[], &ctes);
}

/// Kata kunci utama statement yang diawali `WITH`.
fn main_keyword(toks: &[Token]) -> Option<Kind> {
    let mut depth = 0i32;
    for t in toks.iter().skip(1) {
        match t.kind {
            TokKind::LParen => depth += 1,
            TokKind::RParen => depth -= 1,
            TokKind::Word if depth == 0 => match t.text.to_ascii_uppercase().as_str() {
                "SELECT" => return Some(Kind::Select),
                "INSERT" => return Some(Kind::Insert),
                "UPDATE" => return Some(Kind::Update),
                "DELETE" => return Some(Kind::Delete),
                _ => {}
            },
            _ => {}
        }
    }
    None
}

fn ends_value(t: &Token) -> bool {
    match t.kind {
        TokKind::Word => !words::is_not_column(&t.text),
        TokKind::QuotedIdent | TokKind::RParen | TokKind::Number | TokKind::Str => true,
        _ => false,
    }
}

/// Nama setelah `AS` dan alias implisit (`expr nama`), lowercase.
fn collect_aliases(toks: &[Token]) -> HashSet<String> {
    let mut out = HashSet::new();
    for i in 1..toks.len() {
        let t = &toks[i];
        if !is_name(t)
            || toks
                .get(i + 1)
                .is_some_and(|n| n.kind == TokKind::Dot || n.kind == TokKind::LParen)
        {
            continue;
        }
        let prev = &toks[i - 1];
        if prev.is_kw("AS")
            || (ends_value(prev) && !(t.kind == TokKind::Word && words::is_not_column(&t.text)))
        {
            out.insert(t.text.to_ascii_lowercase());
        }
    }
    out
}

/// Nama CTE yang didefinisikan di awal `toks` (`WITH a AS (...), b(x) AS (...)`).
fn cte_names(toks: &[Token]) -> Vec<String> {
    let mut out = Vec::new();
    if !toks.first().is_some_and(|t| t.is_kw("WITH")) {
        return out;
    }
    let mut i = 1;
    if toks.get(i).is_some_and(|t| t.is_kw("RECURSIVE")) {
        i += 1;
    }
    while let Some(t) = toks.get(i).filter(|t| is_name(t)) {
        let name = t.text.clone();
        let mut j = i + 1;
        if toks.get(j).is_some_and(|t| t.kind == TokKind::LParen) {
            j = matching_paren(toks, j).map_or(toks.len(), |c| c + 1);
        }
        if !toks.get(j).is_some_and(|t| t.is_kw("AS")) {
            break;
        }
        out.push(name);
        j += 1;
        while toks
            .get(j)
            .is_some_and(|t| t.is_kw("NOT") || t.is_kw("MATERIALIZED"))
        {
            j += 1;
        }
        let Some(close) = toks
            .get(j)
            .filter(|t| t.kind == TokKind::LParen)
            .and_then(|_| matching_paren(toks, j))
        else {
            break;
        };
        if toks
            .get(close + 1)
            .is_some_and(|t| t.kind == TokKind::Comma)
        {
            i = close + 2;
        } else {
            break;
        }
    }
    out
}

/// Jumlah item SELECT level teratas; `None` bila memakai `*` atau tidak jelas.
fn select_item_count(toks: &[Token]) -> Option<(usize, usize)> {
    let mut depth = 0i32;
    let sel = toks.iter().enumerate().position(|(i, t)| {
        let d = toks[..i].iter().fold(0i32, |d, t| match t.kind {
            TokKind::LParen => d + 1,
            TokKind::RParen => d - 1,
            _ => d,
        });
        d == 0 && t.is_kw("SELECT")
    })?;
    let mut i = sel + 1;
    // DISTINCT [ON (...)] / ALL / TOP n [PERCENT] [WITH TIES]
    loop {
        match toks.get(i) {
            Some(t) if t.is_kw("DISTINCT") || t.is_kw("ALL") => {
                i += 1;
                if toks.get(i).is_some_and(|t| t.is_kw("ON")) {
                    i = matching_paren(toks, i + 1)? + 1;
                }
            }
            Some(t) if t.is_kw("TOP") => {
                i += 1;
                if toks.get(i).is_some_and(|t| t.kind == TokKind::LParen) {
                    i = matching_paren(toks, i)? + 1;
                } else {
                    i += 1;
                }
                while toks
                    .get(i)
                    .is_some_and(|t| t.is_kw("PERCENT") || t.is_kw("WITH") || t.is_kw("TIES"))
                {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    let mut count = 1usize;
    let mut empty = true;
    while let Some(t) = toks.get(i) {
        match t.kind {
            TokKind::LParen => depth += 1,
            TokKind::RParen => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            TokKind::Comma if depth == 0 => count += 1,
            TokKind::Word
                if depth == 0
                    && [
                        "FROM",
                        "INTO",
                        "WHERE",
                        "GROUP",
                        "ORDER",
                        "LIMIT",
                        "UNION",
                        "EXCEPT",
                        "INTERSECT",
                        "MINUS",
                        "HAVING",
                        "WINDOW",
                        "OFFSET",
                        "FETCH",
                        "FOR",
                    ]
                    .iter()
                    .any(|k| t.is_kw(k)) =>
            {
                break;
            }
            TokKind::Op if t.text == "*" && depth == 0 => {
                let prev = &toks[i - 1];
                if prev.kind == TokKind::Comma
                    || prev.kind == TokKind::Dot
                    || i == sel + 1
                    || prev.is_kw("DISTINCT")
                    || prev.is_kw("ALL")
                    || prev.kind == TokKind::RParen
                        && toks[..i].iter().rev().any(|t| t.is_kw("TOP"))
                {
                    return None;
                }
            }
            _ => {}
        }
        empty = false;
        i += 1;
    }
    (!empty).then_some((count, sel))
}

impl Ctx<'_> {
    fn push(
        &mut self,
        code: DiagCode,
        span: Range<usize>,
        message: String,
        hint: Option<String>,
        suggestions: Vec<String>,
    ) {
        let mut m = diag(LintSeverity::Error, code, DiagSource::Schema, span, message);
        m.hint = hint;
        m.suggestions = suggestions;
        self.out.push(m);
    }

    /// Satu query (mungkin berisi UNION/INTERSECT/EXCEPT) beserta subquery-nya.
    fn block(&mut self, toks: &[Token], parents: &[&Frame], ctes: &[String]) {
        if toks.is_empty() {
            return;
        }
        let mut ctes = ctes.to_vec();
        ctes.extend(cte_names(toks));
        // Pecah di set operation level teratas.
        let mut branches: Vec<(usize, usize)> = Vec::new();
        let mut ops: Vec<usize> = Vec::new();
        let mut depth = 0i32;
        let mut start = 0;
        for (i, t) in toks.iter().enumerate() {
            match t.kind {
                TokKind::LParen => depth += 1,
                TokKind::RParen => depth -= 1,
                TokKind::Word
                    if depth == 0
                        && (t.is_kw("UNION")
                            || t.is_kw("INTERSECT")
                            || t.is_kw("EXCEPT")
                            || (t.is_kw("MINUS") && self.dialect != Dialect::Postgres)) =>
                {
                    branches.push((start, i));
                    ops.push(i);
                    start = i + 1;
                    while toks
                        .get(start)
                        .is_some_and(|t| t.is_kw("ALL") || t.is_kw("DISTINCT"))
                    {
                        start += 1;
                    }
                }
                _ => {}
            }
        }
        branches.push((start, toks.len()));
        let mut counts = Vec::new();
        for &(a, b) in &branches {
            let branch = &toks[a..b];
            let frame = self.frame(branch, &ctes);
            self.walk(branch, &frame, parents, &ctes);
            counts.push(select_item_count(branch).map(|(n, _)| n));
        }
        if branches.len() > 1 && counts.iter().all(Option::is_some) {
            let first = counts[0].unwrap_or(0);
            for (k, c) in counts.iter().enumerate().skip(1) {
                let c = c.unwrap_or(0);
                if c != first {
                    let op = &toks[ops[k - 1]];
                    self.push(
                        DiagCode::SetOpColumnCount,
                        op.start..op.end,
                        format!(
                            "Each {} query must return the same number of columns: the first returns {first}, this one returns {c}",
                            op.text.to_ascii_uppercase()
                        ),
                        Some("Add or remove columns so every branch matches.".to_string()),
                        Vec::new(),
                    );
                    break;
                }
            }
        }
    }

    /// Scope level teratas satu cabang query, dan cek tabel yang tidak ada.
    fn frame(&mut self, toks: &[Token], ctes: &[String]) -> Frame {
        let (Some(f), Some(l)) = (toks.first(), toks.last()) else {
            return Frame::default();
        };
        let base = f.start;
        let padded = format!("{} ", &self.text[f.start..l.end]);
        let analysis = analyze(&padded, padded.len(), self.dialect);
        let (s0, s1) = statement_bounds(&padded, padded.len(), self.dialect);
        let mut local = non_comment(tokenize(&padded[s0..s1], self.dialect));
        for t in &mut local {
            t.start += s0 + base;
            t.end += s0 + base;
        }
        let mut frame = Frame::default();
        for st in analysis.scope.into_iter().filter(|t| t.depth == 0) {
            let Some(first) = local.get(st.idx) else {
                continue;
            };
            let j = chain_end(&local, st.idx);
            let name_tok = &local[j];
            // `x IS DISTINCT FROM y` bukan klausa FROM.
            if st.idx >= 2 && local[st.idx - 1].is_kw("FROM") && local[st.idx - 2].is_kw("DISTINCT")
            {
                continue;
            }
            // `INSERT INTO t (a, b)`: kurung setelah target adalah daftar kolom.
            let fn_call =
                !st.dml_target && local.get(j + 1).is_some_and(|t| t.kind == TokKind::LParen);
            let temp = self.text[..first.start].ends_with('#')
                || name_tok.text.starts_with('#')
                || name_tok.text.starts_with('@');
            let foreign_schema = st
                .schema
                .as_deref()
                .is_some_and(|s| !self.schema.is_default_schema(s));
            frame.entries.push(Entry {
                span: name_tok.start..name_tok.end,
                quoted: name_tok.kind == TokKind::QuotedIdent,
                opaque: fn_call || temp || foreign_schema || st.kind != TableKind::Base,
                table: st,
            });
        }
        let aliases: Vec<String> = frame
            .entries
            .iter()
            .filter_map(|e| e.table.alias.clone())
            .collect();
        for k in 0..frame.entries.len() {
            let e = frame.entries[k].clone();
            if e.opaque || e.table.kind != TableKind::Base {
                continue;
            }
            let lname = e.table.name.to_ascii_lowercase();
            // `UPDATE x SET ... FROM t x` (MsSQL): target berupa alias.
            if aliases
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&e.table.name))
                || ctes.iter().any(|c| c.eq_ignore_ascii_case(&e.table.name))
                || self.script.created.contains(&lname)
                || is_system_table(&lname, self.dialect)
            {
                frame.entries[k].opaque = true;
                continue;
            }
            self.check_table(&e);
        }
        frame
    }

    fn check_table(&mut self, e: &Entry) {
        let Some(tables) = self.schema.tables.as_ref() else {
            return;
        };
        let name = &e.table.name;
        match self.schema.find_table(name) {
            Some(actual) => {
                let actual = actual.rsplit('.').next().unwrap_or(actual).to_string();
                if let ColMatch::Case(real) =
                    self.match_ident(std::slice::from_ref(&actual), name, e.quoted)
                {
                    self.push(
                        DiagCode::IdentifierCase,
                        e.span.clone(),
                        format!("Table \"{name}\" does not exist; it is named \"{real}\" (PostgreSQL folds unquoted names to lower case)"),
                        Some(format!("Write it as \"{real}\" with double quotes.")),
                        vec![format!("\"{}\"", real.replace('"', "\"\""))],
                    );
                } else if self.schema.columns_of(&actual).is_none() {
                    self.missing.insert(actual);
                }
            }
            None => {
                let suggestions = words::suggest(name, tables, 3);
                let hint = suggestions
                    .first()
                    .map(|s| format!("Did you mean {s}?"))
                    .or_else(|| {
                        Some(
                            "Refresh the connection if the table was created recently.".to_string(),
                        )
                    });
                self.push(
                    DiagCode::UnknownTable,
                    e.span.clone(),
                    format!("Table or view \"{name}\" does not exist"),
                    hint,
                    suggestions,
                );
            }
        }
    }

    /// Kolom tabel di scope; `None` = tidak diketahui (jangan periksa).
    fn columns(&mut self, e: &Entry, ctes: &[String]) -> Option<Vec<String>> {
        if e.opaque
            || e.table.kind != TableKind::Base
            || ctes.iter().any(|c| c.eq_ignore_ascii_case(&e.table.name))
        {
            return None;
        }
        let lname = e.table.name.to_ascii_lowercase();
        if self.script.created.contains(&lname) {
            return None;
        }
        match self.schema.columns_of(&e.table.name) {
            Some(cols) => {
                let mut cols = cols.to_vec();
                if let Some(added) = self.script.added_columns.get(&lname) {
                    cols.extend(added.iter().cloned());
                }
                Some(cols)
            }
            None => {
                if let Some(actual) = self.schema.find_table(&e.table.name) {
                    self.missing.insert(actual.to_string());
                }
                None
            }
        }
    }

    /// Cocokkan identifier dengan daftar nama sesuai aturan huruf besar dialek.
    fn match_ident(&self, names: &[String], written: &str, quoted: bool) -> ColMatch {
        if self.dialect != Dialect::Postgres {
            return if names.iter().any(|n| n.eq_ignore_ascii_case(written)) {
                ColMatch::Exact
            } else {
                ColMatch::No
            };
        }
        let folded = if quoted {
            written.to_string()
        } else {
            written.to_lowercase()
        };
        if names.contains(&folded) {
            return ColMatch::Exact;
        }
        match names.iter().find(|n| n.eq_ignore_ascii_case(written)) {
            Some(real) => ColMatch::Case(real.clone()),
            None => ColMatch::No,
        }
    }

    fn walk(&mut self, toks: &[Token], frame: &Frame, parents: &[&Frame], ctes: &[String]) {
        let mut section = Section::None;
        let mut depth = 0usize;
        let mut target_seen = false;
        let mut insert_cols: Option<(usize, Range<usize>)> = None;
        let mut value_rows: Vec<(usize, Range<usize>)> = Vec::new();
        let mut insert_select = false;
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            match t.kind {
                TokKind::LParen => {
                    let next = toks.get(i + 1);
                    let close = matching_paren(toks, i).unwrap_or(toks.len() - 1);
                    if next.is_some_and(|n| n.is_kw("SELECT") || n.is_kw("WITH")) {
                        let mut chain: Vec<&Frame> = vec![frame];
                        chain.extend_from_slice(parents);
                        self.block(&toks[i + 1..close.min(toks.len())], &chain, ctes);
                        i = close + 1;
                        continue;
                    }
                    if section == Section::InsertTarget && target_seen && depth == 0 {
                        let n = self.insert_column_list(&toks[i + 1..close], frame, ctes);
                        insert_cols = Some((n, t.start..toks[close].end));
                        section = Section::None;
                        i = close + 1;
                        continue;
                    }
                    if section == Section::Values && depth == 0 {
                        value_rows.push((
                            group_item_count(&toks[i + 1..close]),
                            t.start..toks[close].end,
                        ));
                        i = close + 1;
                        continue;
                    }
                    depth += 1;
                    i += 1;
                    continue;
                }
                TokKind::RParen => {
                    depth = depth.saturating_sub(1);
                    i += 1;
                    continue;
                }
                TokKind::Word if depth == 0 => {
                    let up = t.text.to_ascii_uppercase();
                    let next_is = |k: &str| toks.get(i + 1).is_some_and(|n| n.is_kw(k));
                    let new_section = match up.as_str() {
                        "SELECT" => {
                            if self.kind == Kind::Insert && target_seen {
                                insert_select = true;
                            }
                            Some(Section::Select)
                        }
                        "FROM" | "JOIN" => Some(Section::From),
                        "UPDATE" if self.kind == Kind::Insert => Some(Section::InsertTail),
                        "UPDATE" => Some(Section::From),
                        "INTO" if self.kind == Kind::Insert => Some(Section::InsertTarget),
                        "INTO" => Some(Section::Skip),
                        "WHERE" | "HAVING" | "QUALIFY" | "WINDOW" => Some(Section::Cond),
                        "RETURNING" | "OUTPUT" if self.kind == Kind::Insert => {
                            Some(Section::InsertTail)
                        }
                        "RETURNING" | "OUTPUT" => Some(Section::Cond),
                        "ON" if next_is("CONFLICT") || next_is("DUPLICATE") => {
                            Some(Section::InsertTail)
                        }
                        "ON" => Some(Section::Cond),
                        "SET" if self.kind == Kind::Insert => Some(Section::InsertTail),
                        "SET" => Some(Section::Cond),
                        "GROUP" | "ORDER" if next_is("BY") => Some(Section::Cond),
                        "VALUES" if self.kind == Kind::Insert => Some(Section::Values),
                        "VALUES" => Some(Section::Skip),
                        "LIMIT" | "OFFSET" | "FETCH" => Some(Section::Skip),
                        "USING" if toks.get(i + 1).is_some_and(|n| n.kind == TokKind::LParen) => {
                            i = matching_paren(toks, i + 1).map_or(toks.len(), |c| c + 1);
                            continue;
                        }
                        "USING" => Some(Section::From),
                        _ => None,
                    };
                    if let Some(s) = new_section {
                        section = s;
                        i += if (up == "GROUP" || up == "ORDER") && next_is("BY") {
                            2
                        } else {
                            1
                        };
                        continue;
                    }
                }
                _ => {}
            }
            if section == Section::InsertTarget && is_name(t) && !target_seen && !t.is_kw("TABLE") {
                target_seen = true;
                i = chain_end(toks, i) + 1;
                continue;
            }
            if matches!(
                section,
                Section::Select | Section::Cond | Section::InsertTail
            ) && is_name(t)
            {
                i = self.check_ref(toks, i, frame, parents, ctes, section);
                continue;
            }
            i += 1;
        }
        if self.kind == Kind::Insert {
            self.check_insert_counts(toks, frame, ctes, insert_cols, &value_rows, insert_select);
        }
    }

    fn target<'f>(&self, frame: &'f Frame) -> Option<&'f Entry> {
        frame.entries.iter().find(|e| e.table.dml_target)
    }

    /// Daftar kolom `INSERT INTO t (a, b)`: cek nama dan duplikat; kembalikan jumlahnya.
    fn insert_column_list(&mut self, toks: &[Token], frame: &Frame, ctes: &[String]) -> usize {
        let target = self.target(frame).cloned();
        let cols = match &target {
            Some(e) => self.columns(e, ctes),
            None => None,
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut count = 0;
        for t in toks.iter().filter(|t| is_name(t)) {
            count += 1;
            let key = t.text.to_ascii_lowercase();
            if !seen.insert(key) {
                self.push(
                    DiagCode::DuplicateColumn,
                    t.start..t.end,
                    format!("Column \"{}\" is listed more than once", t.text),
                    Some("Remove the duplicate column.".to_string()),
                    Vec::new(),
                );
                continue;
            }
            if let (Some(cols), Some(e)) = (&cols, &target) {
                self.report_column(t, cols, e);
            }
        }
        count
    }

    fn report_column(&mut self, t: &Token, cols: &[String], e: &Entry) {
        match self.match_ident(cols, &t.text, t.kind == TokKind::QuotedIdent) {
            ColMatch::Exact => {}
            ColMatch::Case(real) => self.case_error(t, &real, "Column"),
            ColMatch::No => {
                let suggestions = words::suggest(&t.text, cols, 3);
                let hint = suggestions.first().map(|s| format!("Did you mean {s}?"));
                self.push(
                    DiagCode::UnknownColumn,
                    t.start..t.end,
                    format!(
                        "Column \"{}\" does not exist in \"{}\"",
                        t.text,
                        e.display()
                    ),
                    hint,
                    suggestions,
                );
            }
        }
    }

    fn case_error(&mut self, t: &Token, real: &str, what: &str) {
        self.push(
            DiagCode::IdentifierCase,
            t.start..t.end,
            format!(
                "{what} \"{}\" does not exist; it is named \"{real}\" (PostgreSQL folds unquoted names to lower case)",
                t.text
            ),
            Some(format!("Write it as \"{real}\" with double quotes.")),
            vec![format!("\"{}\"", real.replace('"', "\"\""))],
        );
    }

    /// Satu referensi nama mulai token `i`; kembalikan index lanjutan.
    fn check_ref(
        &mut self,
        toks: &[Token],
        i: usize,
        frame: &Frame,
        parents: &[&Frame],
        ctes: &[String],
        section: Section,
    ) -> usize {
        let t = &toks[i];
        let j = chain_end(toks, i);
        let after = j + 1;
        let star = toks.get(after).is_some_and(|d| d.kind == TokKind::Dot)
            && toks.get(after + 1).is_some_and(|s| s.is_op("*"));
        let next = if star {
            toks.get(after + 2)
        } else {
            toks.get(after)
        };
        let resume = if star { after + 2 } else { after };
        // Fungsi (`f(`, `schema.f(`): argumennya diperiksa sebagai token biasa.
        if next.is_some_and(|n| n.kind == TokKind::LParen) {
            return resume;
        }
        let prev = i.checked_sub(1).map(|p| &toks[p]);
        let skip_prev = prev.is_some_and(|p| {
            p.kind == TokKind::Dot
                || p.kind == TokKind::Param
                || (p.kind == TokKind::Op
                    && (p.text.contains('@')
                        || p.text.contains(':')
                        || p.text == "#"
                        || p.text == "->"
                        || p.text == "->>"))
                || [
                    "AS",
                    "COLLATE",
                    "OVER",
                    "WINDOW",
                    "FOR",
                    "INTERVAL",
                    "CHARACTER",
                    "CHARSET",
                    "SEPARATOR",
                    "TYPE",
                    "INTO",
                ]
                .iter()
                .any(|k| p.is_kw(k))
        });
        if skip_prev
            || self.text[..t.start].ends_with('#')
            || next.is_some_and(|n| n.is_op("=>") || n.is_op(":="))
        {
            return resume;
        }
        if j == i && !star {
            // Nama tunggal.
            if t.kind == TokKind::Word && words::is_not_column(&t.text) {
                return resume;
            }
            if next.is_some_and(|n| n.kind == TokKind::Str) {
                return resume; // literal bertipe: DATE '2024-01-01'
            }
            if prev.is_some_and(ends_value) {
                return resume; // alias implisit
            }
            let lower = t.text.to_ascii_lowercase();
            if self.aliases.contains(&lower) && t.kind == TokKind::Word {
                return resume;
            }
            if self.dialect == Dialect::MsSql
                && section == Section::Select
                && next.is_some_and(|n| n.is_op("="))
                && prev.is_some_and(|p| p.kind == TokKind::Comma || p.is_kw("SELECT"))
            {
                return resume; // `SELECT alias = expr`
            }
            if frame.entries.iter().any(|e| {
                e.table.name.eq_ignore_ascii_case(&t.text)
                    || e.table
                        .alias
                        .as_deref()
                        .is_some_and(|a| a.eq_ignore_ascii_case(&t.text))
            }) {
                return resume; // nilai baris PostgreSQL: SELECT t FROM t
            }
            self.check_unqualified(t, frame, parents, ctes, section);
            return resume;
        }
        // Rantai: qualifier = segmen sebelum terakhir (atau terakhir untuk `q.*`).
        let (q_idx, col_idx) = if star { (j, None) } else { (j - 2, Some(j)) };
        let q = &toks[q_idx];
        if PSEUDO_QUALIFIERS
            .iter()
            .any(|p| p.eq_ignore_ascii_case(&q.text))
        {
            return resume;
        }
        let parts = (j - i) / 2 + 1;
        let mut found: Option<Entry> = None;
        for f in std::iter::once(frame).chain(parents.iter().copied()) {
            match resolve_qualifier(f, &q.text) {
                Resolved::Entry(e) => {
                    found = Some(e.clone());
                    break;
                }
                Resolved::AliasRequired(e) => {
                    if let Some(alias) = e.table.alias.clone() {
                        self.push(
                            DiagCode::AliasRequired,
                            q.start..q.end,
                            format!("\"{}\" has the alias \"{alias}\" in this query; refer to it as {alias}", e.table.name),
                            Some(format!("Replace {} with {alias}.", q.text)),
                            vec![alias],
                        );
                    }
                    return resume;
                }
                Resolved::None => {}
            }
        }
        let Some(entry) = found else {
            // `schema.table.col` tanpa tabel itu di FROM: terlalu banyak bentuk sah.
            if parts >= 3 || (star && parts >= 2) {
                return resume;
            }
            let mut names: Vec<String> = Vec::new();
            for f in std::iter::once(frame).chain(parents.iter().copied()) {
                for e in &f.entries {
                    names.push(
                        e.table
                            .alias
                            .clone()
                            .unwrap_or_else(|| e.table.name.clone()),
                    );
                }
            }
            if names.is_empty() {
                return resume;
            }
            let suggestions = words::suggest(&q.text, &names, 3);
            let is_table = self.schema.find_table(&q.text).is_some()
                || ctes.iter().any(|c| c.eq_ignore_ascii_case(&q.text));
            let message = if is_table {
                format!(
                    "Table \"{}\" is not in the FROM clause of this query",
                    q.text
                )
            } else {
                format!("Unknown table or alias \"{}\"", q.text)
            };
            let hint = suggestions
                .first()
                .map(|s| format!("Did you mean {s}?"))
                .or_else(|| Some(format!("Tables in scope: {}.", names.join(", "))));
            self.push(
                DiagCode::UnknownQualifier,
                q.start..q.end,
                message,
                hint,
                suggestions,
            );
            return resume;
        };
        let Some(col_idx) = col_idx else {
            return resume;
        };
        if let Some(cols) = self.columns(&entry, ctes) {
            self.report_column(&toks[col_idx], &cols, &entry);
        }
        resume
    }

    fn check_unqualified(
        &mut self,
        t: &Token,
        frame: &Frame,
        parents: &[&Frame],
        ctes: &[String],
        section: Section,
    ) {
        let visible: Vec<Entry> = frame
            .entries
            .iter()
            .filter(|e| match (self.kind, section) {
                (Kind::Insert, Section::InsertTail) => e.table.dml_target,
                (Kind::Insert, _) => !e.table.dml_target,
                _ => true,
            })
            .cloned()
            .collect();
        if visible.is_empty() {
            return;
        }
        let quoted = t.kind == TokKind::QuotedIdent;
        let mut owners: Vec<Entry> = Vec::new();
        let mut case_hit: Option<String> = None;
        let mut all_cols: Vec<String> = Vec::new();
        for e in &visible {
            let Some(cols) = self.columns(e, ctes) else {
                return;
            };
            match self.match_ident(&cols, &t.text, quoted) {
                ColMatch::Exact => owners.push(e.clone()),
                ColMatch::Case(real) => case_hit = Some(real),
                ColMatch::No => {}
            }
            all_cols.extend(cols);
        }
        if owners.len() > 1 {
            if self.merges_columns {
                return;
            }
            let quals: Vec<String> = owners
                .iter()
                .map(|e| {
                    format!(
                        "{}.{}",
                        e.table.alias.as_deref().unwrap_or(&e.table.name),
                        t.text
                    )
                })
                .collect();
            self.push(
                DiagCode::AmbiguousColumn,
                t.start..t.end,
                format!(
                    "Column \"{}\" is ambiguous: it exists in {}",
                    t.text,
                    owners
                        .iter()
                        .map(Entry::display)
                        .collect::<Vec<_>>()
                        .join(" and ")
                ),
                Some("Qualify it with a table name or alias.".to_string()),
                quals,
            );
            return;
        }
        if !owners.is_empty() {
            return;
        }
        if let Some(real) = case_hit {
            self.case_error(t, &real, "Column");
            return;
        }
        // Referensi ke query luar (correlated subquery).
        for f in parents {
            for e in &f.entries {
                let Some(cols) = self.columns(e, ctes) else {
                    return;
                };
                if !matches!(self.match_ident(&cols, &t.text, quoted), ColMatch::No) {
                    return;
                }
            }
        }
        let suggestions = words::suggest(&t.text, &all_cols, 3);
        let hint = suggestions.first().map(|s| format!("Did you mean {s}?"));
        let place = if visible.len() == 1 {
            format!("\"{}\"", visible[0].display())
        } else {
            format!(
                "any of {}",
                visible
                    .iter()
                    .map(|e| format!("\"{}\"", e.display()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.push(
            DiagCode::UnknownColumn,
            t.start..t.end,
            format!("Column \"{}\" does not exist in {place}", t.text),
            hint,
            suggestions,
        );
    }

    fn check_insert_counts(
        &mut self,
        toks: &[Token],
        frame: &Frame,
        ctes: &[String],
        insert_cols: Option<(usize, Range<usize>)>,
        value_rows: &[(usize, Range<usize>)],
        insert_select: bool,
    ) {
        let target = self.target(frame).cloned();
        let (expected, explicit) = match insert_cols {
            Some((n, _)) => (n, true),
            None => match target.as_ref().and_then(|e| self.columns(e, ctes)) {
                Some(cols) => (cols.len(), false),
                None => return,
            },
        };
        if expected == 0 {
            return;
        }
        // Tanpa daftar kolom: PostgreSQL mengisi sisanya dengan DEFAULT, MsSQL
        // melewati kolom IDENTITY/computed — hanya "terlalu banyak" yang pasti salah.
        let lenient = !explicit
            && matches!(
                self.dialect,
                Dialect::Postgres | Dialect::MsSql | Dialect::Generic
            );
        let wrong = |got: usize| {
            if lenient {
                got > expected
            } else {
                got != expected
            }
        };
        let table = target.map(|e| e.table.name).unwrap_or_default();
        let describe = |got: usize| {
            if explicit {
                format!("INSERT lists {expected} column(s) but {got} value(s) are supplied")
            } else {
                format!(
                    "Table \"{table}\" has {expected} column(s) but {got} value(s) are supplied"
                )
            }
        };
        for (got, span) in value_rows {
            if wrong(*got) {
                self.push(
                    DiagCode::ValueCountMismatch,
                    span.clone(),
                    describe(*got),
                    Some("Make the number of values match the column list.".to_string()),
                    Vec::new(),
                );
                return;
            }
        }
        if insert_select && let Some((got, sel)) = select_item_count(toks) {
            if wrong(got) {
                let t = &toks[sel];
                self.push(
                    DiagCode::ValueCountMismatch,
                    t.start..t.end,
                    describe(got).replace("value(s) are supplied", "column(s) are selected"),
                    Some("Make the SELECT list match the INSERT column list.".to_string()),
                    Vec::new(),
                );
            }
        }
    }
}

/// Jumlah item dipisah koma di level teratas sebuah grup kurung.
fn group_item_count(toks: &[Token]) -> usize {
    if toks.is_empty() {
        return 0;
    }
    let mut depth = 0i32;
    let mut n = 1;
    for t in toks {
        match t.kind {
            TokKind::LParen => depth += 1,
            TokKind::RParen => depth -= 1,
            TokKind::Comma if depth == 0 => n += 1,
            _ => {}
        }
    }
    n
}

enum Resolved<'f> {
    Entry(&'f Entry),
    /// Ditulis dengan nama tabel padahal tabel itu punya alias.
    AliasRequired(&'f Entry),
    None,
}

fn resolve_qualifier<'f>(frame: &'f Frame, q: &str) -> Resolved<'f> {
    if let Some(e) = frame.entries.iter().find(|e| {
        e.table
            .alias
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(q))
    }) {
        return Resolved::Entry(e);
    }
    if let Some(e) = frame
        .entries
        .iter()
        .find(|e| e.table.alias.is_none() && e.table.name.eq_ignore_ascii_case(q))
    {
        return Resolved::Entry(e);
    }
    match frame
        .entries
        .iter()
        .find(|e| e.table.name.eq_ignore_ascii_case(q))
    {
        // Tabel yang sama bisa muncul dua kali (dengan dan tanpa alias):
        // hanya salah bila tidak ada kemunculan tanpa alias.
        Some(e) if !e.opaque => Resolved::AliasRequired(e),
        Some(e) => Resolved::Entry(e),
        None => Resolved::None,
    }
}

/// Tabel/katalog sistem yang tidak ada di cache tabel user.
fn is_system_table(lname: &str, dialect: Dialect) -> bool {
    match dialect {
        Dialect::Postgres => lname.starts_with("pg_") || lname.starts_with("information_schema"),
        Dialect::Sqlite => lname.starts_with("sqlite_") || lname.starts_with("pragma_"),
        Dialect::MySql => lname == "dual",
        Dialect::MsSql => lname.starts_with("sys") || lname.starts_with("information_schema"),
        Dialect::Generic => {
            lname == "dual" || lname.starts_with("pg_") || lname.starts_with("sqlite_")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> SchemaSnapshot {
        let mut columns = HashMap::new();
        columns.insert(
            "users".to_string(),
            vec![
                "id".to_string(),
                "name".to_string(),
                "email".to_string(),
                "created_at".to_string(),
            ],
        );
        columns.insert(
            "orders".to_string(),
            vec![
                "id".to_string(),
                "user_id".to_string(),
                "total".to_string(),
                "status".to_string(),
            ],
        );
        columns.insert(
            "Accounts".to_string().to_ascii_lowercase(),
            vec!["Id".to_string(), "userName".to_string()],
        );
        SchemaSnapshot {
            tables: Some(vec!["users".into(), "orders".into(), "Accounts".into()]),
            columns,
            default_schemas: vec!["public".into()],
            collections: None,
        }
    }

    fn run_with(sql: &str, dialect: Dialect) -> Vec<LintMessage> {
        let mut out = Vec::new();
        let mut missing = BTreeSet::new();
        check(
            sql,
            0..sql.len(),
            dialect,
            &schema(),
            &ScriptState::default(),
            &mut out,
            &mut missing,
        );
        out
    }

    fn run(sql: &str) -> Vec<LintMessage> {
        run_with(sql, Dialect::Postgres)
    }

    fn found(sql: &str) -> Vec<(DiagCode, String)> {
        run(sql)
            .into_iter()
            .map(|m| (m.code.unwrap(), sql[m.span.unwrap()].to_string()))
            .collect()
    }

    #[test]
    fn unknown_table_is_marked_with_suggestion() {
        let out = run("SELECT id FROM usres");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code, Some(DiagCode::UnknownTable));
        assert_eq!(
            out[0].suggestions.first().map(String::as_str),
            Some("users")
        );
    }

    #[test]
    fn unknown_columns_qualified_and_unqualified() {
        assert_eq!(
            found("SELECT nmae FROM users"),
            vec![(DiagCode::UnknownColumn, "nmae".into())]
        );
        assert_eq!(
            found("SELECT u.emial FROM users u"),
            vec![(DiagCode::UnknownColumn, "emial".into())]
        );
        let out = run("SELECT u.emial FROM users u");
        assert_eq!(
            out[0].suggestions.first().map(String::as_str),
            Some("email")
        );
        assert_eq!(
            found("SELECT u.id FROM users u JOIN orders o ON o.user_id = u.id WHERE o.totl > 1"),
            vec![(DiagCode::UnknownColumn, "totl".into())]
        );
    }

    #[test]
    fn unknown_alias_and_alias_required() {
        assert_eq!(
            found("SELECT x.id FROM users u"),
            vec![(DiagCode::UnknownQualifier, "x".into())]
        );
        assert_eq!(
            found("SELECT users.id FROM users u"),
            vec![(DiagCode::AliasRequired, "users".into())]
        );
    }

    #[test]
    fn ambiguous_column_needs_a_qualifier() {
        let out = run("SELECT id FROM users u JOIN orders o ON o.user_id = u.id");
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].code, Some(DiagCode::AmbiguousColumn));
        assert!(run("SELECT id FROM users JOIN orders USING (id)").is_empty());
    }

    #[test]
    fn postgres_identifier_case_is_checked() {
        assert_eq!(
            found("SELECT username FROM \"Accounts\""),
            vec![(DiagCode::IdentifierCase, "username".into())]
        );
        assert_eq!(
            found("SELECT \"userName\" FROM Accounts"),
            vec![(DiagCode::IdentifierCase, "Accounts".into())]
        );
        assert!(run_with("SELECT username FROM Accounts", Dialect::MySql).is_empty());
    }

    #[test]
    fn aliases_functions_and_keywords_are_not_columns() {
        for sql in [
            "SELECT count(*) AS n FROM users ORDER BY n DESC",
            "SELECT name total FROM users ORDER BY total",
            "SELECT EXTRACT(YEAR FROM created_at), CAST(id AS TEXT) FROM users",
            "SELECT id FROM users WHERE created_at > CURRENT_DATE - INTERVAL '1 day'",
            "SELECT id FROM users WHERE name IS DISTINCT FROM email",
            "SELECT u.* , o.total FROM users u LEFT JOIN orders o ON o.user_id = u.id",
            "SELECT id, DATE '2024-01-01' FROM users",
            "WITH c AS (SELECT id, name FROM users) SELECT c.whatever FROM c",
            "SELECT id FROM users WHERE id IN (SELECT user_id FROM orders WHERE orders.user_id = users.id)",
            "SELECT id FROM generate_series(1, 3) g(id)",
            "SELECT id FROM audit.logs",
            "SELECT * FROM pg_stat_activity",
            "SELECT @@version",
        ] {
            assert!(run(sql).is_empty(), "{sql}: {:?}", run(sql));
        }
    }

    #[test]
    fn correlated_subquery_sees_outer_tables() {
        assert!(run("SELECT name FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.id AND email IS NOT NULL)").is_empty());
        assert_eq!(
            found(
                "SELECT name FROM users u WHERE EXISTS (SELECT 1 FROM orders o WHERE o.user_id = u.idd)"
            ),
            vec![(DiagCode::UnknownColumn, "idd".into())]
        );
    }

    #[test]
    fn insert_columns_and_value_counts() {
        assert_eq!(
            found("INSERT INTO users (id, nme) VALUES (1, 'a')"),
            vec![(DiagCode::UnknownColumn, "nme".into())]
        );
        assert_eq!(
            found("INSERT INTO users (id, name) VALUES (1)"),
            vec![(DiagCode::ValueCountMismatch, "(1)".into())]
        );
        assert_eq!(
            found("INSERT INTO users (id, id) VALUES (1, 2)"),
            vec![(DiagCode::DuplicateColumn, "id".into())]
        );
        // PostgreSQL: kolom sisa diisi DEFAULT
        assert!(run("INSERT INTO users VALUES (1, 'a')").is_empty());
        assert!(!run_with("INSERT INTO users VALUES (1, 'a')", Dialect::MySql).is_empty());
        assert!(run("INSERT INTO users (id, name) SELECT id, status FROM orders").is_empty());
        assert_eq!(
            found("INSERT INTO users (id, name) SELECT id FROM orders"),
            vec![(DiagCode::ValueCountMismatch, "SELECT".into())]
        );
        assert!(run("INSERT INTO users (id, name) VALUES (1, 'a') ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name").is_empty());
    }

    #[test]
    fn union_branches_must_match() {
        assert_eq!(
            found("SELECT id, name FROM users UNION ALL SELECT id FROM orders"),
            vec![(DiagCode::SetOpColumnCount, "UNION".into())]
        );
        assert!(run("SELECT id, name FROM users UNION SELECT id, status FROM orders").is_empty());
        // tiap cabang punya scope sendiri
        assert!(run("SELECT email FROM users UNION SELECT status FROM orders").is_empty());
    }

    #[test]
    fn update_and_delete_targets() {
        assert_eq!(
            found("UPDATE users SET nam = 'x' WHERE id = 1"),
            vec![(DiagCode::UnknownColumn, "nam".into())]
        );
        assert_eq!(
            found("DELETE FROM orders WHERE stauts = 'x'"),
            vec![(DiagCode::UnknownColumn, "stauts".into())]
        );
        assert!(
            run_with(
                "UPDATE x SET x.total = 1 FROM orders x WHERE x.id = 1",
                Dialect::MsSql
            )
            .is_empty()
        );
    }

    #[test]
    fn unknown_metadata_means_no_marks() {
        let mut out = Vec::new();
        let mut missing = BTreeSet::new();
        let snap = SchemaSnapshot {
            tables: Some(vec!["users".into()]),
            ..Default::default()
        };
        check(
            "SELECT whatever FROM users",
            0..26,
            Dialect::Postgres,
            &snap,
            &ScriptState::default(),
            &mut out,
            &mut missing,
        );
        assert!(out.is_empty());
        assert!(missing.contains("users"));
        let mut out = Vec::new();
        check(
            "SELECT a FROM nope",
            0..18,
            Dialect::Postgres,
            &SchemaSnapshot::default(),
            &ScriptState::default(),
            &mut out,
            &mut missing,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn script_definitions_are_respected() {
        let mut script = ScriptState::default();
        record_definitions(
            "CREATE TEMP TABLE scratch (a int)",
            Dialect::Postgres,
            &mut script,
        );
        record_definitions(
            "ALTER TABLE users ADD COLUMN nickname text",
            Dialect::Postgres,
            &mut script,
        );
        let mut out = Vec::new();
        let mut missing = BTreeSet::new();
        let sql = "SELECT a FROM scratch; SELECT nickname FROM users";
        check(
            sql,
            0..21,
            Dialect::Postgres,
            &schema(),
            &script,
            &mut out,
            &mut missing,
        );
        check(
            sql,
            23..sql.len(),
            Dialect::Postgres,
            &schema(),
            &script,
            &mut out,
            &mut missing,
        );
        assert!(out.is_empty(), "{out:?}");
    }
}
