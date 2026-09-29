//! Analisis satu statement SQL menjadi model alur data untuk fitur
//! "Show Query Diagram": tabel sumber, relasi join, kolom hasil, dan kolom
//! yang diubah. Modul ini headless (tanpa `window_egui`) sehingga bisa dites
//! dan dipakai ulang oleh agent.

pub mod build;
pub mod layout;
#[cfg(feature = "query_ast")]
mod parse;
pub mod prompt;
pub mod render;

use crate::models::enums::DatabaseType;

/// Id sumber khusus untuk baris `VALUES` pada INSERT.
pub const VALUES_ID: &str = "VALUES";

/// Jenis statement yang bisa digambarkan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Select,
    Insert,
    Update,
    Delete,
}

impl StatementKind {
    /// Label kata kunci SQL untuk badge UI.
    pub fn label(self) -> &'static str {
        match self {
            StatementKind::Select => "SELECT",
            StatementKind::Insert => "INSERT",
            StatementKind::Update => "UPDATE",
            StatementKind::Delete => "DELETE",
        }
    }

    /// Kalimat singkat (English, tampil di UI) tentang apa yang dilakukan.
    pub fn summary(self) -> &'static str {
        match self {
            StatementKind::Select => {
                "Reads rows from the source tables and returns a new result set."
            }
            StatementKind::Insert => "Adds new rows to the target table.",
            StatementKind::Update => "Changes column values of matching rows in the target table.",
            StatementKind::Delete => "Removes matching rows from the target table.",
        }
    }
}

/// Asal sebuah sumber data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Table,
    Cte,
    Subquery,
    Values,
}

/// Tabel (atau sumber lain) yang dibaca/ditulis statement.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceTable {
    /// Id unik di dalam model: alias bila ada, selain itu nama tabel.
    pub id: String,
    /// Nama tabel apa adanya (boleh diawali schema).
    pub table: String,
    pub alias: Option<String>,
    pub kind: SourceKind,
    /// Jenis join (mis. "LEFT JOIN"); `None` untuk tabel FROM pertama.
    pub join: Option<String>,
    /// Semua kolom yang ditampilkan: urutan skema bila skema diketahui,
    /// selain itu hanya kolom yang dipakai statement.
    pub columns: Vec<String>,
    /// Kolom yang benar-benar dipakai statement.
    pub used: Vec<String>,
    /// `SELECT *` / `t.*` mengambil semua kolom tabel ini.
    pub all_columns: bool,
}

impl SourceTable {
    pub(crate) fn new(id: String, table: String, alias: Option<String>, kind: SourceKind) -> Self {
        Self {
            id,
            table,
            alias,
            kind,
            join: None,
            columns: Vec::new(),
            used: Vec::new(),
            all_columns: false,
        }
    }

    /// Judul kartu: `tabel AS alias` atau hanya nama tabel.
    pub fn title(&self) -> String {
        match &self.alias {
            Some(a) if !a.eq_ignore_ascii_case(&self.table) => format!("{} AS {}", self.table, a),
            _ => self.table.clone(),
        }
    }

    /// Tandai kolom sebagai dipakai statement (sekaligus ditampilkan).
    pub(crate) fn push_used(&mut self, column: &str) {
        if column == "*" {
            return;
        }
        if !self.used.iter().any(|c| c.eq_ignore_ascii_case(column)) {
            self.used.push(column.to_string());
        }
        self.push_column(column);
    }

    pub fn is_used(&self, column: &str) -> bool {
        self.used.iter().any(|c| c.eq_ignore_ascii_case(column))
    }

    pub(crate) fn push_column(&mut self, column: &str) {
        if column == "*" {
            return;
        }
        if !self.columns.iter().any(|c| c.eq_ignore_ascii_case(column)) {
            self.columns.push(column.to_string());
        }
    }
}

/// Referensi `sumber.kolom`. `table` kosong berarti belum diketahui (kolom
/// tanpa kualifikasi); diselesaikan oleh [`QueryDiagramModel::finalize`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnRef {
    pub table: String,
    pub column: String,
}

impl ColumnRef {
    pub fn new(table: impl Into<String>, column: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            column: column.into(),
        }
    }
}

/// Relasi kolom antar dua sumber (kondisi `ON a.x = b.y`, `USING`, atau
/// kesetaraan di WHERE).
#[derive(Debug, Clone, PartialEq)]
pub struct JoinLink {
    pub left: ColumnRef,
    pub right: ColumnRef,
    pub join_type: String,
}

/// Kolom hasil SELECT (atau hasil SELECT sumber INSERT).
#[derive(Debug, Clone, PartialEq)]
pub struct OutputColumn {
    pub name: String,
    pub expr: String,
    pub sources: Vec<ColumnRef>,
    pub aggregate: bool,
}

/// Perubahan satu kolom target (SET pada UPDATE, kolom pada INSERT).
#[derive(Debug, Clone, PartialEq)]
pub struct Mutation {
    pub column: String,
    pub new_value: String,
    pub sources: Vec<ColumnRef>,
    /// Nilai tetap (literal/parameter), tidak membaca kolom apa pun.
    pub is_static: bool,
}

/// Model alur data satu statement.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryDiagramModel {
    pub kind: StatementKind,
    pub sql: String,
    /// Tabel yang ditulis (INSERT/UPDATE/DELETE).
    pub target: Option<SourceTable>,
    pub sources: Vec<SourceTable>,
    pub joins: Vec<JoinLink>,
    pub output: Vec<OutputColumn>,
    pub mutations: Vec<Mutation>,
    pub filter: Option<String>,
    pub filter_columns: Vec<ColumnRef>,
    /// Kolom lain yang dipakai (GROUP BY, ORDER BY, HAVING).
    pub referenced: Vec<ColumnRef>,
    pub group_by: Vec<String>,
    pub having: Option<String>,
    pub order_by: Vec<String>,
    pub limit: Option<String>,
    pub distinct: bool,
    /// Jumlah baris VALUES pada INSERT.
    pub values_rows: usize,
    /// Catatan tambahan (UNION, CTE, ON CONFLICT, ...), English.
    pub notes: Vec<String>,
}

impl QueryDiagramModel {
    pub(crate) fn empty(kind: StatementKind, sql: &str) -> Self {
        Self {
            kind,
            sql: sql.to_string(),
            target: None,
            sources: Vec::new(),
            joins: Vec::new(),
            output: Vec::new(),
            mutations: Vec::new(),
            filter: None,
            filter_columns: Vec::new(),
            referenced: Vec::new(),
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            distinct: false,
            values_rows: 0,
            notes: Vec::new(),
        }
    }

    /// Target lalu seluruh sumber.
    pub fn tables(&self) -> impl Iterator<Item = &SourceTable> {
        self.target.iter().chain(self.sources.iter())
    }

    pub fn table(&self, id: &str) -> Option<&SourceTable> {
        self.tables().find(|t| t.id == id)
    }

    fn table_mut(&mut self, id: &str) -> Option<&mut SourceTable> {
        if self.target.as_ref().is_some_and(|t| t.id == id) {
            return self.target.as_mut();
        }
        self.sources.iter_mut().find(|t| t.id == id)
    }

    /// Nama tabel fisik (tanpa CTE/subquery/VALUES) untuk konteks skema.
    pub fn physical_tables(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in self.tables() {
            if t.kind == SourceKind::Table && !out.iter().any(|o| o.eq_ignore_ascii_case(&t.table))
            {
                out.push(t.table.clone());
            }
        }
        out
    }

    /// Selesaikan kolom tanpa kualifikasi, isi daftar kolom tiap tabel, dan
    /// kembangkan `*` bila skema diketahui. `lookup` menerima nama tabel dan
    /// mengembalikan daftar kolomnya (dari cache), bila ada.
    pub fn finalize(&mut self, lookup: &dyn Fn(&str) -> Option<Vec<String>>) {
        let schemas: Vec<(String, Option<Vec<String>>)> = self
            .tables()
            .map(|t| {
                let cols = if t.kind == SourceKind::Table {
                    lookup(&t.table)
                } else {
                    None
                };
                (t.id.clone(), cols)
            })
            .collect();
        // Kolom INSERT tanpa daftar kolom (`#n`) dinamai dari urutan skema target.
        let target_schema = self
            .target
            .as_ref()
            .and_then(|t| schemas.iter().find(|(id, _)| *id == t.id))
            .and_then(|(_, c)| c.clone());
        for m in &mut self.mutations {
            let Some(n) = m
                .column
                .strip_prefix('#')
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            let name = n
                .checked_sub(1)
                .and_then(|i| target_schema.as_ref().and_then(|c| c.get(i).cloned()))
                .unwrap_or_else(|| format!("column {n}"));
            for s in &mut m.sources {
                if s.table == VALUES_ID && s.column == m.column {
                    s.column = name.clone();
                }
            }
            m.column = name;
        }
        let default_table = match self.kind {
            StatementKind::Select => self.sources.first().map(|t| t.id.clone()),
            _ => self
                .target
                .as_ref()
                .map(|t| t.id.clone())
                .or_else(|| self.sources.first().map(|t| t.id.clone())),
        };
        let known: Vec<(String, Vec<String>)> = self
            .tables()
            .map(|t| (t.id.clone(), t.columns.clone()))
            .collect();
        let resolve = |r: &mut ColumnRef| {
            if !r.table.is_empty() {
                return;
            }
            let hit = schemas.iter().find(|(_, cols)| {
                cols.as_ref()
                    .is_some_and(|c| c.iter().any(|x| x.eq_ignore_ascii_case(&r.column)))
            });
            let hit = hit.map(|(id, _)| id.clone()).or_else(|| {
                known
                    .iter()
                    .find(|(_, cols)| cols.iter().any(|x| x.eq_ignore_ascii_case(&r.column)))
                    .map(|(id, _)| id.clone())
            });
            if let Some(id) = hit.or_else(|| default_table.clone()) {
                r.table = id;
            }
        };

        for j in &mut self.joins {
            resolve(&mut j.left);
            resolve(&mut j.right);
        }
        for o in &mut self.output {
            o.sources.iter_mut().for_each(resolve);
        }
        for m in &mut self.mutations {
            m.sources.iter_mut().for_each(resolve);
        }
        self.filter_columns.iter_mut().for_each(resolve);
        self.referenced.iter_mut().for_each(resolve);
        // Relasi dengan kedua ujung di tabel yang sama bukan relasi antar tabel.
        self.joins.retain(|j| j.left.table != j.right.table);

        // Kumpulkan kolom per tabel.
        let mut refs: Vec<ColumnRef> = Vec::new();
        for j in &self.joins {
            refs.push(j.left.clone());
            refs.push(j.right.clone());
        }
        for o in &self.output {
            refs.extend(o.sources.iter().cloned());
        }
        for m in &self.mutations {
            refs.extend(m.sources.iter().cloned());
        }
        refs.extend(self.filter_columns.iter().cloned());
        refs.extend(self.referenced.iter().cloned());
        for r in &refs {
            if let Some(t) = self.table_mut(&r.table) {
                t.push_used(&r.column);
            }
        }
        if let Some(target) = self.target.as_mut() {
            for m in &self.mutations {
                target.push_used(&m.column);
            }
        }

        // Tampilkan semua kolom tabel sesuai urutan skema; kolom yang tidak
        // ada di skema (mis. cache usang) tetap ikut di akhir.
        for (id, cols) in &schemas {
            let Some(cols) = cols else { continue };
            let Some(t) = self.table_mut(id) else {
                continue;
            };
            let mut all = cols.clone();
            for c in &t.columns {
                if !all.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                    all.push(c.clone());
                }
            }
            t.columns = all;
        }
        let mut expanded: Vec<OutputColumn> = Vec::with_capacity(self.output.len());
        for o in std::mem::take(&mut self.output) {
            let star = o.sources.len() == 1 && o.sources[0].column == "*";
            if star
                && let Some((_, Some(cols))) =
                    schemas.iter().find(|(id, _)| *id == o.sources[0].table)
            {
                let tid = o.sources[0].table.clone();
                for c in cols {
                    expanded.push(OutputColumn {
                        name: c.clone(),
                        expr: format!("{tid}.{c}"),
                        sources: vec![ColumnRef::new(tid.clone(), c.clone())],
                        aggregate: false,
                    });
                }
            } else {
                expanded.push(o);
            }
        }
        self.output = expanded;
    }
}

/// Kegagalan analisis statement.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum QueryDiagramError {
    #[error("No SQL statement found. Place the cursor inside a query or select it first.")]
    Empty,
    #[error("Could not parse the statement: {0}")]
    Parse(String),
    #[error(
        "{0} statements are not supported yet. Show Query Diagram works with SELECT, INSERT, UPDATE and DELETE."
    )]
    Unsupported(String),
    #[error("Query diagrams are not available for {0} connections.")]
    UnsupportedDatabase(String),
    #[error("This build was compiled without SQL parsing support (feature `query_ast`).")]
    FeatureDisabled,
}

/// Apakah jenis database ini bisa dianalisis (hanya SQL relasional).
pub fn supports_database(db: &DatabaseType) -> bool {
    matches!(
        db,
        DatabaseType::MySQL | DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::MsSQL
    )
}

/// Analisis satu statement tanpa informasi skema.
pub fn analyze_statement(
    sql: &str,
    db: &DatabaseType,
) -> Result<QueryDiagramModel, QueryDiagramError> {
    analyze_with_schema(sql, db, &|_| None)
}

/// Analisis satu statement; `lookup` memberi daftar kolom tabel (dari cache)
/// untuk menyelesaikan kolom tanpa kualifikasi dan mengembangkan `*`.
pub fn analyze_with_schema(
    sql: &str,
    db: &DatabaseType,
    lookup: &dyn Fn(&str) -> Option<Vec<String>>,
) -> Result<QueryDiagramModel, QueryDiagramError> {
    if !supports_database(db) {
        return Err(QueryDiagramError::UnsupportedDatabase(format!("{db:?}")));
    }
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err(QueryDiagramError::Empty);
    }
    #[cfg(feature = "query_ast")]
    {
        let mut model = parse::parse_model(trimmed, db)?;
        model.finalize(lookup);
        Ok(model)
    }
    #[cfg(not(feature = "query_ast"))]
    {
        let _ = lookup;
        Err(QueryDiagramError::FeatureDisabled)
    }
}

/// Potong teks panjang menjadi maksimal `max` karakter (aman untuk UTF-8).
pub fn clip(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_is_utf8_safe() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("héllo wörld", 6), "héllo…");
        assert_eq!(clip("a\n   b", 10), "a b");
    }

    #[test]
    fn test_redis_is_rejected() {
        let err = analyze_statement("SELECT 1", &DatabaseType::Redis).unwrap_err();
        assert!(matches!(err, QueryDiagramError::UnsupportedDatabase(_)));
    }

    #[test]
    fn test_empty_statement() {
        let err = analyze_statement("  ; ", &DatabaseType::MySQL).unwrap_err();
        assert_eq!(err, QueryDiagramError::Empty);
    }
}
