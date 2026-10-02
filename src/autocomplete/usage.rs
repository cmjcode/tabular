//! Statistik pemakaian yang dipelajari dari riwayat query user.
//!
//! Murni dan deterministik: tiap query di-lex dan dianalisis dengan analyzer
//! yang sama dengan autocomplete, lalu dihitung tabel yang dipakai, kolom per
//! tabel (`alias.kolom` di-resolve lewat scope), dan pasangan kolom JOIN
//! (`a.x = b.y` antar tabel berbeda). Engine memakai angka ini untuk ranking.

use std::collections::HashMap;

use super::analyzer::{ScopeTable, TableKind, analyze, is_reserved};
use super::lexer::{Dialect, TokKind, Token, tokenize};

/// Pasangan join ternormalisasi: `(tabel_a, kolom_a, tabel_b, kolom_b)` dengan
/// `(tabel_a, kolom_a) <= (tabel_b, kolom_b)`; semua lowercase.
type JoinKey = (String, String, String, String);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageStats {
    tables: HashMap<String, u32>,
    columns: HashMap<(String, String), u32>,
    joins: HashMap<JoinKey, u32>,
}

fn join_key(t1: &str, c1: &str, t2: &str, c2: &str) -> JoinKey {
    let a = (t1.to_ascii_lowercase(), c1.to_ascii_lowercase());
    let b = (t2.to_ascii_lowercase(), c2.to_ascii_lowercase());
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    (x.0, x.1, y.0, y.1)
}

/// Nama tabel dasar untuk qualifier (alias atau nama tabel) di scope.
fn resolve<'s>(scope: &'s [ScopeTable], q: &str) -> Option<&'s ScopeTable> {
    scope
        .iter()
        .filter(|t| t.kind == TableKind::Base)
        .find(|t| {
            t.alias
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(q))
        })
        .or_else(|| {
            scope
                .iter()
                .filter(|t| t.kind == TableKind::Base)
                .find(|t| t.name.eq_ignore_ascii_case(q))
        })
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, TokKind::Word | TokKind::QuotedIdent)
}

impl UsageStats {
    pub fn from_queries<'a>(queries: impl IntoIterator<Item = &'a str>, dialect: Dialect) -> Self {
        let mut s = UsageStats::default();
        for q in queries {
            s.learn(q, dialect);
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    pub fn table(&self, table: &str) -> u32 {
        self.tables
            .get(&table.to_ascii_lowercase())
            .copied()
            .unwrap_or(0)
    }

    pub fn column(&self, table: &str, column: &str) -> u32 {
        self.columns
            .get(&(table.to_ascii_lowercase(), column.to_ascii_lowercase()))
            .copied()
            .unwrap_or(0)
    }

    pub fn join(&self, t1: &str, c1: &str, t2: &str, c2: &str) -> u32 {
        self.joins
            .get(&join_key(t1, c1, t2, c2))
            .copied()
            .unwrap_or(0)
    }

    /// Pasangan kolom `(kolom_di_a, kolom_di_b)` yang pernah dipakai untuk
    /// menggabungkan tabel `a` dan `b`, beserta frekuensinya.
    pub fn joins_between(&self, a: &str, b: &str) -> Vec<(String, String, u32)> {
        let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
        let mut out: Vec<(String, String, u32)> = self
            .joins
            .iter()
            .filter_map(|((t1, c1, t2, c2), &n)| {
                if *t1 == a && *t2 == b {
                    Some((c1.clone(), c2.clone(), n))
                } else if *t1 == b && *t2 == a {
                    Some((c2.clone(), c1.clone(), n))
                } else {
                    None
                }
            })
            .collect();
        out.sort_by(|x, y| y.2.cmp(&x.2).then_with(|| x.0.cmp(&y.0)));
        out
    }

    /// Join yang pernah dipakai dari `table`: `(tabel_lain, kolom_lain, kolom_di_table, n)`.
    pub fn joins_of(&self, table: &str) -> Vec<(String, String, String, u32)> {
        let t = table.to_ascii_lowercase();
        let mut out: Vec<(String, String, String, u32)> = self
            .joins
            .iter()
            .filter_map(|((t1, c1, t2, c2), &n)| {
                if *t1 == t && *t2 != t {
                    Some((t2.clone(), c2.clone(), c1.clone(), n))
                } else if *t2 == t && *t1 != t {
                    Some((t1.clone(), c1.clone(), c2.clone(), n))
                } else {
                    None
                }
            })
            .collect();
        out.sort_by(|x, y| y.3.cmp(&x.3).then_with(|| x.0.cmp(&y.0)));
        out
    }

    /// Pelajari satu teks query (boleh berisi beberapa statement).
    pub fn learn(&mut self, sql: &str, dialect: Dialect) {
        let toks: Vec<Token> = tokenize(sql, dialect)
            .into_iter()
            .filter(|t| t.kind != TokKind::Comment)
            .collect();
        for stmt in toks.split(|t| t.kind == TokKind::Semicolon) {
            if let (Some(first), Some(last)) = (stmt.first(), stmt.last()) {
                self.learn_statement(&sql[first.start..last.end], stmt, dialect);
            }
        }
    }

    fn learn_statement(&mut self, text: &str, toks: &[Token], dialect: Dialect) {
        // Spasi penutup: kursor di luar token terakhir (mis. angka `LIMIT 10`)
        let padded = format!("{text} ");
        let scope: Vec<ScopeTable> = analyze(&padded, padded.len(), dialect)
            .scope
            .into_iter()
            .filter(|t| t.depth == 0)
            .collect();
        let bases: Vec<&ScopeTable> = scope.iter().filter(|t| t.kind == TableKind::Base).collect();
        if bases.is_empty() {
            return;
        }
        for t in &bases {
            *self.tables.entry(t.name.to_ascii_lowercase()).or_default() += 1;
        }
        let single = (bases.len() == 1).then(|| bases[0].name.to_ascii_lowercase());
        let scope_names: Vec<String> = scope
            .iter()
            .flat_map(|t| [Some(t.name.clone()), t.alias.clone()])
            .flatten()
            .map(|n| n.to_ascii_lowercase())
            .collect();

        // Referensi kolom: (index token pertama, index token terakhir, tabel, kolom)
        let mut refs: Vec<(usize, usize, String, String)> = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            let t = &toks[i];
            let next_dot = toks.get(i + 1).is_some_and(|n| n.kind == TokKind::Dot);
            let prev_dot = i > 0 && toks[i - 1].kind == TokKind::Dot;
            if is_name(t) && next_dot && toks.get(i + 2).is_some_and(is_name) {
                // `q.kol` (untuk `schema.tabel.kol` ambil dua segmen terakhir)
                let mut j = i;
                while toks.get(j + 3).is_some_and(|n| n.kind == TokKind::Dot)
                    && toks.get(j + 4).is_some_and(is_name)
                {
                    j += 2;
                }
                let col_tok = &toks[j + 2];
                let followed_by_call = toks.get(j + 3).is_some_and(|n| n.kind == TokKind::LParen);
                if !followed_by_call && let Some(st) = resolve(&scope, &toks[j].text) {
                    refs.push((
                        i,
                        j + 2,
                        st.name.to_ascii_lowercase(),
                        col_tok.text.to_ascii_lowercase(),
                    ));
                }
                i = j + 3;
                continue;
            }
            if let Some(tbl) = single.as_ref()
                && is_name(t)
                && !prev_dot
                && !next_dot
                && !toks.get(i + 1).is_some_and(|n| n.kind == TokKind::LParen)
                && !(t.kind == TokKind::Word && is_reserved(&t.text))
                && !(i > 0 && toks[i - 1].is_kw("AS"))
                && !scope_names.contains(&t.text.to_ascii_lowercase())
            {
                refs.push((i, i, tbl.clone(), t.text.to_ascii_lowercase()));
            }
            i += 1;
        }

        for (_, _, t, c) in &refs {
            *self.columns.entry((t.clone(), c.clone())).or_default() += 1;
        }
        // `a.x = b.y` antar tabel berbeda → pasangan join
        for w in refs.windows(2) {
            let (l, r) = (&w[0], &w[1]);
            let eq_between = r.0 == l.1 + 2 && toks[l.1 + 1].is_op("=");
            if eq_between && l.2 != r.2 {
                *self
                    .joins
                    .entry(join_key(&l.2, &l.3, &r.2, &r.3))
                    .or_default() += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learns_tables_columns_and_joins() {
        let s = UsageStats::from_queries(
            [
                "SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id WHERE o.status = 'paid'",
                "select * from orders where status = 'new' limit 10",
                "SELECT count(*) FROM orders o JOIN users u ON u.id = o.user_id; SELECT 1",
            ],
            Dialect::Postgres,
        );
        assert_eq!(s.table("orders"), 3);
        assert_eq!(s.table("USERS"), 2);
        assert_eq!(s.column("orders", "status"), 2);
        assert_eq!(s.column("users", "name"), 1);
        // arah join tidak berpengaruh
        assert_eq!(s.join("users", "id", "orders", "user_id"), 2);
        assert_eq!(
            s.joins_between("orders", "users"),
            vec![("user_id".to_string(), "id".to_string(), 2)]
        );
        // fungsi dan keyword bukan kolom
        assert_eq!(s.column("orders", "count"), 0);
        assert_eq!(s.column("orders", "limit"), 0);
    }

    #[test]
    fn unqualified_columns_only_with_single_table() {
        let s = UsageStats::from_queries(
            ["SELECT id FROM users u JOIN orders o ON o.user_id = u.id"],
            Dialect::Postgres,
        );
        // ambigu (dua tabel) → tidak dihitung
        assert_eq!(s.column("users", "id") + s.column("orders", "id"), 1);
        let s = UsageStats::from_queries(["SELECT email FROM users AS x"], Dialect::Postgres);
        assert_eq!(s.column("users", "email"), 1);
        assert_eq!(s.column("users", "x"), 0);
    }

    #[test]
    fn garbage_input_does_not_panic() {
        let s = UsageStats::from_queries(
            [
                "",
                ";;",
                "SELECT 'unterminated",
                "FROM . . JOIN ON = ;",
                "é.ü = ö.ä",
            ],
            Dialect::MySql,
        );
        assert!(s.table("x") == 0);
    }
}
