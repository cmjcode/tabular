//! Structure Compare & Sync: bandingkan definisi kolom dua tabel, lalu
//! susun skrip DDL yang membuat struktur tujuan sama dengan sumber.
//! Pembandingnya murni; hanya [`compare_table_structure`] dan
//! [`compare_database_structure`] yang menyentuh database.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use super::catalog::{self, Endpoint};
use super::compare::{self, CompareProgressHandle};
use super::types::{Logical, SourceColumn, create_table_sql, map_type, parse_type};
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::{quote_ident, quote_qualified};

/// Opsi perbandingan struktur tabel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StructureOptions {
    /// Nama kolom yang diabaikan (tidak dibandingkan).
    pub ignore_columns: Vec<String>,
    /// Abaikan perbedaan huruf besar/kecil pada nama kolom.
    pub case_insensitive_names: bool,
    /// Bandingkan nullability (NOT NULL / NULL).
    pub compare_nullability: bool,
}

impl Default for StructureOptions {
    fn default() -> Self {
        Self {
            ignore_columns: Vec::new(),
            case_insensitive_names: false,
            compare_nullability: true,
        }
    }
}

/// Jenis perbedaan kolom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnDiffKind {
    OnlyInSource,
    OnlyInTarget,
    Changed,
}

impl ColumnDiffKind {
    pub fn label(self) -> &'static str {
        match self {
            ColumnDiffKind::OnlyInSource => "Only in source",
            ColumnDiffKind::OnlyInTarget => "Only in target",
            ColumnDiffKind::Changed => "Changed",
        }
    }
}

/// Atribut kolom yang berubah.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnAttribute {
    Type,
    Nullable,
    PrimaryKey,
}

impl ColumnAttribute {
    pub fn label(self) -> &'static str {
        match self {
            ColumnAttribute::Type => "Type",
            ColumnAttribute::Nullable => "Nullable",
            ColumnAttribute::PrimaryKey => "Primary Key",
        }
    }
}

/// Satu perbedaan definisi kolom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnDiff {
    pub name: String,
    pub kind: ColumnDiffKind,
    pub source: Option<SourceColumn>,
    pub target: Option<SourceColumn>,
    pub changed: Vec<ColumnAttribute>,
}

/// Hasil perbandingan struktur antara dua tabel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StructureResult {
    pub diffs: Vec<ColumnDiff>,
    pub identical: usize,
    pub source_columns: usize,
    pub target_columns: usize,
}

impl StructureResult {
    /// `(hanya sumber, hanya tujuan, berubah)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let count = |k: ColumnDiffKind| self.diffs.iter().filter(|d| d.kind == k).count();
        (
            count(ColumnDiffKind::OnlyInSource),
            count(ColumnDiffKind::OnlyInTarget),
            count(ColumnDiffKind::Changed),
        )
    }
}

fn col_matches(src_name: &str, dst_name: &str, case_insensitive: bool) -> bool {
    if case_insensitive {
        src_name.eq_ignore_ascii_case(dst_name)
    } else {
        src_name == dst_name
    }
}

/// Bandingkan tipe kolom sumber dan target.
fn types_match(
    src_type: &str,
    dst_type: &str,
    src_db: &DatabaseType,
    dst_db: &DatabaseType,
) -> bool {
    let s_clean = src_type.trim();
    let d_clean = dst_type.trim();
    if src_db == dst_db {
        if s_clean.eq_ignore_ascii_case(d_clean) {
            return true;
        }
        let s_log = parse_type(src_db, s_clean);
        let d_log = parse_type(dst_db, d_clean);
        s_log != Logical::Unknown && s_log == d_log
    } else {
        let s_log = parse_type(src_db, s_clean);
        let d_log = parse_type(dst_db, d_clean);
        if s_log != Logical::Unknown && d_log != Logical::Unknown {
            s_log == d_log
        } else {
            s_clean.eq_ignore_ascii_case(d_clean)
        }
    }
}

/// Bandingkan definisi kolom dua tabel secara murni tanpa koneksi.
pub fn diff_structure(
    src_cols: &[SourceColumn],
    dst_cols: &[SourceColumn],
    src_db: &DatabaseType,
    dst_db: &DatabaseType,
    opts: &StructureOptions,
) -> StructureResult {
    let is_ignored = |name: &str| {
        opts.ignore_columns
            .iter()
            .any(|ig| ig.trim().eq_ignore_ascii_case(name.trim()))
    };

    let mut diffs = Vec::new();
    let mut identical = 0;
    let mut matched_target = vec![false; dst_cols.len()];

    for src in src_cols {
        if is_ignored(&src.name) {
            continue;
        }
        let hit = dst_cols.iter().enumerate().position(|(i, dst)| {
            !matched_target[i]
                && !is_ignored(&dst.name)
                && col_matches(&src.name, &dst.name, opts.case_insensitive_names)
        });

        match hit {
            Some(i) => {
                matched_target[i] = true;
                let dst = &dst_cols[i];
                let type_ok = types_match(&src.data_type, &dst.data_type, src_db, dst_db);
                let nullable_ok = if opts.compare_nullability {
                    src.nullable == dst.nullable
                } else {
                    true
                };
                let pk_ok = src.primary_key == dst.primary_key;

                if type_ok && nullable_ok && pk_ok {
                    identical += 1;
                } else {
                    let mut changed = Vec::new();
                    if !type_ok {
                        changed.push(ColumnAttribute::Type);
                    }
                    if !nullable_ok {
                        changed.push(ColumnAttribute::Nullable);
                    }
                    if !pk_ok {
                        changed.push(ColumnAttribute::PrimaryKey);
                    }
                    diffs.push(ColumnDiff {
                        name: src.name.clone(),
                        kind: ColumnDiffKind::Changed,
                        source: Some(src.clone()),
                        target: Some(dst.clone()),
                        changed,
                    });
                }
            }
            None => {
                diffs.push(ColumnDiff {
                    name: src.name.clone(),
                    kind: ColumnDiffKind::OnlyInSource,
                    source: Some(src.clone()),
                    target: None,
                    changed: Vec::new(),
                });
            }
        }
    }

    for (i, dst) in dst_cols.iter().enumerate() {
        if matched_target[i] || is_ignored(&dst.name) {
            continue;
        }
        diffs.push(ColumnDiff {
            name: dst.name.clone(),
            kind: ColumnDiffKind::OnlyInTarget,
            source: None,
            target: Some(dst.clone()),
            changed: Vec::new(),
        });
    }

    let source_columns = src_cols.iter().filter(|c| !is_ignored(&c.name)).count();
    let target_columns = dst_cols.iter().filter(|c| !is_ignored(&c.name)).count();

    StructureResult {
        diffs,
        identical,
        source_columns,
        target_columns,
    }
}

/// Bagian skrip sinkronisasi DDL yang diaktifkan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructureSyncParts {
    pub add_missing_columns: bool,
    pub alter_changed_columns: bool,
    pub drop_extra_columns: bool,
    pub create_missing_tables: bool,
}

impl Default for StructureSyncParts {
    fn default() -> Self {
        Self {
            add_missing_columns: true,
            alter_changed_columns: true,
            drop_extra_columns: false,
            create_missing_tables: true,
        }
    }
}

/// Menghitung statement yang dapat dieksekusi (mengabaikan komentar manual `--`).
pub fn count_executable_statements(statements: &[String]) -> usize {
    statements
        .iter()
        .filter(|s| !s.trim_start().starts_with("--"))
        .count()
}

/// Susun statement DDL untuk membuat tabel tujuan memiliki struktur kolom yang sama dengan sumber.
pub fn structure_sync_statements(
    result: &StructureResult,
    source_db: &DatabaseType,
    target_db: &DatabaseType,
    target_sql: &str,
    parts: StructureSyncParts,
) -> Vec<String> {
    let mut out = Vec::new();

    for diff in &result.diffs {
        match diff.kind {
            ColumnDiffKind::OnlyInSource if parts.add_missing_columns => {
                let Some(src) = &diff.source else { continue };
                let col_ident = quote_ident(target_db, &src.name);
                let mapped_type = map_type(source_db, target_db, &src.data_type, src.primary_key);
                let null_sql = if src.nullable && !src.primary_key {
                    ""
                } else {
                    " NOT NULL"
                };

                if matches!(target_db, DatabaseType::MsSQL) {
                    out.push(format!(
                        "ALTER TABLE {target_sql} ADD {col_ident} {mapped_type}{null_sql};"
                    ));
                } else {
                    out.push(format!(
                        "ALTER TABLE {target_sql} ADD COLUMN {col_ident} {mapped_type}{null_sql};"
                    ));
                }
            }
            ColumnDiffKind::OnlyInTarget if parts.drop_extra_columns => {
                let Some(dst) = &diff.target else { continue };
                let col_ident = quote_ident(target_db, &dst.name);
                out.push(format!("ALTER TABLE {target_sql} DROP COLUMN {col_ident};"));
            }
            ColumnDiffKind::Changed if parts.alter_changed_columns => {
                let (Some(src), Some(dst)) = (&diff.source, &diff.target) else {
                    continue;
                };
                let col_ident = quote_ident(target_db, &dst.name);
                let mapped_type = map_type(source_db, target_db, &src.data_type, src.primary_key);
                let null_sql = if src.nullable && !src.primary_key {
                    " NULL"
                } else {
                    " NOT NULL"
                };

                if diff.changed.contains(&ColumnAttribute::PrimaryKey) {
                    out.push(format!(
                        "-- manual: primary key change for column {} on {target_sql};",
                        dst.name
                    ));
                }

                let type_changed = diff.changed.contains(&ColumnAttribute::Type);
                let null_changed = diff.changed.contains(&ColumnAttribute::Nullable);

                if type_changed || null_changed {
                    match target_db {
                        DatabaseType::SQLite => {
                            out.push(format!(
                                "-- manual: SQLite does not support ALTER COLUMN for {} on {target_sql};",
                                dst.name
                            ));
                        }
                        DatabaseType::PostgreSQL => {
                            if type_changed {
                                out.push(format!(
                                    "ALTER TABLE {target_sql} ALTER COLUMN {col_ident} TYPE {mapped_type};"
                                ));
                            }
                            if null_changed {
                                if src.nullable {
                                    out.push(format!(
                                        "ALTER TABLE {target_sql} ALTER COLUMN {col_ident} DROP NOT NULL;"
                                    ));
                                } else {
                                    out.push(format!(
                                        "ALTER TABLE {target_sql} ALTER COLUMN {col_ident} SET NOT NULL;"
                                    ));
                                }
                            }
                        }
                        DatabaseType::MySQL => {
                            let null_clause = if src.nullable && !src.primary_key {
                                ""
                            } else {
                                " NOT NULL"
                            };
                            out.push(format!(
                                "ALTER TABLE {target_sql} MODIFY COLUMN {col_ident} {mapped_type}{null_clause};"
                            ));
                        }
                        DatabaseType::MsSQL => {
                            out.push(format!(
                                "ALTER TABLE {target_sql} ALTER COLUMN {col_ident} {mapped_type}{null_sql};"
                            ));
                        }
                        _ => {
                            let null_clause = if src.nullable && !src.primary_key {
                                ""
                            } else {
                                " NOT NULL"
                            };
                            out.push(format!(
                                "ALTER TABLE {target_sql} ALTER COLUMN {col_ident} {mapped_type}{null_clause};"
                            ));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    out
}

/// [`structure_sync_statements`] sebagai satu skrip teks untuk ditinjau di editor.
pub fn structure_sync_script(
    result: &StructureResult,
    source_db: &DatabaseType,
    target_db: &DatabaseType,
    target_sql: &str,
    parts: StructureSyncParts,
) -> String {
    let mut script =
        structure_sync_statements(result, source_db, target_db, target_sql, parts).join("\n");
    if !script.is_empty() {
        script.push('\n');
    }
    script
}

/// Hasil perbandingan struktur satu tabel yang siap ditampilkan atau disinkronkan.
#[derive(Clone, Debug)]
pub struct TableStructureOutcome {
    pub result: StructureResult,
    pub source_db: DatabaseType,
    pub target_db: DatabaseType,
    pub target_sql: String,
    pub source_table: String,
    pub target_table: String,
    pub source_columns: Vec<SourceColumn>,
    pub target_columns: Vec<SourceColumn>,
}

/// Bandingkan struktur dua tabel dari endpoint yang belum terhubung.
pub async fn compare_table_structure(
    source: Endpoint,
    target: Endpoint,
    source_table: &str,
    target_table: &str,
    opts: &StructureOptions,
) -> Result<TableStructureOutcome, String> {
    let source = source.connect().await.map_err(|e| format!("Source: {e}"))?;
    let target = target.connect().await.map_err(|e| format!("Target: {e}"))?;
    compare_table_structure_connected(&source, &target, source_table, target_table, opts).await
}

/// Bandingkan struktur dua tabel dari endpoint yang sudah tersambung.
pub async fn compare_table_structure_connected(
    source: &Endpoint,
    target: &Endpoint,
    source_table: &str,
    target_table: &str,
    opts: &StructureOptions,
) -> Result<TableStructureOutcome, String> {
    let src_cols = catalog::fetch_columns(source, source_table)
        .await
        .map_err(|e| format!("Source: {e}"))?;
    let dst_cols = catalog::fetch_columns(target, target_table)
        .await
        .map_err(|e| format!("Target: {e}"))?;

    let result = diff_structure(
        &src_cols,
        &dst_cols,
        source.db_type(),
        target.db_type(),
        opts,
    );

    Ok(TableStructureOutcome {
        result,
        source_db: source.db_type().clone(),
        target_db: target.db_type().clone(),
        target_sql: target.table_sql(target_table),
        source_table: source_table.to_string(),
        target_table: target_table.to_string(),
        source_columns: src_cols,
        target_columns: dst_cols,
    })
}

/// Hasil perbandingan satu tabel dalam perbandingan database struktur.
#[derive(Clone, Debug)]
pub struct TableStructureComparison {
    pub source_table: String,
    pub target_table: String,
    pub outcome: Result<TableStructureOutcome, String>,
}

impl TableStructureComparison {
    pub fn differs(&self) -> bool {
        self.outcome
            .as_ref()
            .is_ok_and(|o| !o.result.diffs.is_empty())
    }
}

/// Hasil perbandingan struktur seluruh database.
#[derive(Clone, Debug, Default)]
pub struct DatabaseStructureComparison {
    pub tables: Vec<TableStructureComparison>,
    /// Tabel yang hanya ada di sumber (dengan definisinya untuk CREATE TABLE).
    pub source_only: Vec<(String, Vec<SourceColumn>)>,
    /// Tabel yang hanya ada di tujuan.
    pub target_only: Vec<String>,
    pub source_db: Option<DatabaseType>,
    pub target_db: Option<DatabaseType>,
    pub cancelled: bool,
}

/// Bandingkan struktur semua tabel yang ada di kedua database.
pub async fn compare_database_structure(
    source: Endpoint,
    target: Endpoint,
    opts: &StructureOptions,
    progress: CompareProgressHandle,
    cancel: Arc<AtomicBool>,
) -> Result<DatabaseStructureComparison, String> {
    let source = source.connect().await.map_err(|e| format!("Source: {e}"))?;
    let target = target.connect().await.map_err(|e| format!("Target: {e}"))?;
    let source_tables = catalog::list_tables(&source)
        .await
        .map_err(|e| format!("Source: {e}"))?;
    let target_tables = catalog::list_tables(&target)
        .await
        .map_err(|e| format!("Target: {e}"))?;
    if source_tables.is_empty() && target_tables.is_empty() {
        return Err("No tables found on either side; choose the databases to compare".to_string());
    }

    let pairing = compare::pair_tables(&source_tables, &target_tables);
    compare::with_progress(&progress, |p| {
        p.tables_total = pairing.pairs.len() + pairing.source_only.len();
    });

    let mut out = DatabaseStructureComparison {
        source_db: Some(source.db_type().clone()),
        target_db: Some(target.db_type().clone()),
        target_only: pairing.target_only,
        ..Default::default()
    };

    for (source_table, target_table) in pairing.pairs {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        compare::with_progress(&progress, |p| p.current_table = source_table.clone());
        let outcome =
            compare_table_structure_connected(&source, &target, &source_table, &target_table, opts)
                .await;
        if let Err(e) = &outcome {
            log::debug!("[TRANSFER] compare structure {source_table} failed: {e}");
        }
        out.tables.push(TableStructureComparison {
            source_table,
            target_table,
            outcome,
        });
        compare::with_progress(&progress, |p| p.tables_done += 1);
    }

    for src_table in pairing.source_only {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        compare::with_progress(&progress, |p| p.current_table = src_table.clone());
        let cols = catalog::fetch_columns(&source, &src_table)
            .await
            .unwrap_or_default();
        out.source_only.push((src_table, cols));
        compare::with_progress(&progress, |p| p.tables_done += 1);
    }

    Ok(out)
}

/// Susun statement DDL untuk perbandingan seluruh database.
pub fn database_structure_sync_statements(
    db: &DatabaseStructureComparison,
    selected: Option<usize>,
    parts: StructureSyncParts,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(i) = selected {
        if let Some(t) = db.tables.get(i)
            && let Ok(o) = &t.outcome
        {
            out.extend(structure_sync_statements(
                &o.result,
                &o.source_db,
                &o.target_db,
                &o.target_sql,
                parts,
            ));
        }
        return out;
    }

    let default_db = DatabaseType::SQLite;
    let source_db = db.source_db.as_ref().unwrap_or(&default_db);
    let target_db = db.target_db.as_ref().unwrap_or(&default_db);

    if parts.create_missing_tables {
        for (name, cols) in &db.source_only {
            if cols.is_empty() {
                out.push(format!(
                    "-- manual: cannot create table {name} because source columns are unknown;"
                ));
            } else {
                let target_sql = quote_qualified(target_db, name);
                out.push(create_table_sql(source_db, target_db, &target_sql, cols));
            }
        }
    }

    for t in &db.tables {
        if let Ok(o) = &t.outcome {
            out.extend(structure_sync_statements(
                &o.result,
                &o.source_db,
                &o.target_db,
                &o.target_sql,
                parts,
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, data_type: &str, nullable: bool, primary_key: bool) -> SourceColumn {
        SourceColumn {
            name: name.to_string(),
            data_type: data_type.to_string(),
            nullable,
            primary_key,
        }
    }

    #[test]
    fn diff_structure_detects_missing_and_extra_columns() {
        let src = vec![
            col("id", "int", false, true),
            col("name", "varchar(50)", true, false),
            col("created_at", "timestamp", true, false),
        ];
        let dst = vec![
            col("id", "int", false, true),
            col("extra_col", "text", true, false),
        ];
        let opts = StructureOptions::default();
        let r = diff_structure(
            &src,
            &dst,
            &DatabaseType::PostgreSQL,
            &DatabaseType::PostgreSQL,
            &opts,
        );

        assert_eq!(r.identical, 1);
        assert_eq!(r.counts(), (2, 1, 0));

        let src_only: Vec<_> = r
            .diffs
            .iter()
            .filter(|d| d.kind == ColumnDiffKind::OnlyInSource)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(src_only, vec!["name", "created_at"]);

        let dst_only: Vec<_> = r
            .diffs
            .iter()
            .filter(|d| d.kind == ColumnDiffKind::OnlyInTarget)
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(dst_only, vec!["extra_col"]);
    }

    #[test]
    fn diff_structure_detects_type_nullability_and_pk_changes() {
        let src = vec![
            col("id", "int", false, true),
            col("title", "varchar(100)", false, false),
            col("body", "text", true, false),
            col("is_active", "boolean", true, false),
        ];
        let dst = vec![
            col("id", "int", false, false),            // PK beda
            col("title", "varchar(50)", false, false), // Tipe beda
            col("body", "text", false, false),         // Nullable beda
            col("is_active", "boolean", true, false),  // Sama
        ];
        let opts = StructureOptions::default();
        let r = diff_structure(
            &src,
            &dst,
            &DatabaseType::PostgreSQL,
            &DatabaseType::PostgreSQL,
            &opts,
        );

        assert_eq!(r.identical, 1);
        assert_eq!(r.counts(), (0, 0, 3));

        let id_diff = r.diffs.iter().find(|d| d.name == "id").unwrap();
        assert_eq!(id_diff.changed, vec![ColumnAttribute::PrimaryKey]);

        let title_diff = r.diffs.iter().find(|d| d.name == "title").unwrap();
        assert_eq!(title_diff.changed, vec![ColumnAttribute::Type]);

        let body_diff = r.diffs.iter().find(|d| d.name == "body").unwrap();
        assert_eq!(body_diff.changed, vec![ColumnAttribute::Nullable]);
    }

    #[test]
    fn diff_structure_respects_ignore_columns_and_case_insensitive() {
        let src = vec![
            col("ID", "INTEGER", false, true),
            col("secret_token", "text", true, false),
        ];
        let dst = vec![
            col("id", "integer", false, true),
            col("secret_token", "varchar(20)", false, false),
        ];
        let opts = StructureOptions {
            ignore_columns: vec!["secret_token".to_string()],
            case_insensitive_names: true,
            compare_nullability: true,
        };
        let r = diff_structure(
            &src,
            &dst,
            &DatabaseType::SQLite,
            &DatabaseType::SQLite,
            &opts,
        );

        assert_eq!(r.identical, 1);
        assert_eq!(r.diffs.len(), 0);
        assert_eq!(r.source_columns, 1);
        assert_eq!(r.target_columns, 1);
    }

    #[test]
    fn diff_structure_normalizes_types_across_engines() {
        let src = vec![col("val", "int4", true, false)];
        let dst = vec![col("val", "INT", true, false)];
        let opts = StructureOptions::default();
        let r = diff_structure(
            &src,
            &dst,
            &DatabaseType::PostgreSQL,
            &DatabaseType::MySQL,
            &opts,
        );
        assert_eq!(r.identical, 1);
        assert_eq!(r.diffs.len(), 0);
    }

    #[test]
    fn structure_sync_statements_generates_per_dialect() {
        let result = StructureResult {
            diffs: vec![
                ColumnDiff {
                    name: "new_col".to_string(),
                    kind: ColumnDiffKind::OnlyInSource,
                    source: Some(col("new_col", "varchar(60)", true, false)),
                    target: None,
                    changed: Vec::new(),
                },
                ColumnDiff {
                    name: "old_col".to_string(),
                    kind: ColumnDiffKind::OnlyInTarget,
                    source: None,
                    target: Some(col("old_col", "text", true, false)),
                    changed: Vec::new(),
                },
                ColumnDiff {
                    name: "changed_col".to_string(),
                    kind: ColumnDiffKind::Changed,
                    source: Some(col("changed_col", "text", false, false)),
                    target: Some(col("changed_col", "varchar(50)", true, false)),
                    changed: vec![ColumnAttribute::Type, ColumnAttribute::Nullable],
                },
            ],
            identical: 0,
            source_columns: 2,
            target_columns: 2,
        };

        let parts = StructureSyncParts {
            add_missing_columns: true,
            alter_changed_columns: true,
            drop_extra_columns: true,
            create_missing_tables: true,
        };

        // PostgreSQL
        let pg_stmts = structure_sync_statements(
            &result,
            &DatabaseType::PostgreSQL,
            &DatabaseType::PostgreSQL,
            "\"users\"",
            parts,
        );
        assert_eq!(
            pg_stmts,
            vec![
                "ALTER TABLE \"users\" ADD COLUMN \"new_col\" varchar(60);".to_string(),
                "ALTER TABLE \"users\" DROP COLUMN \"old_col\";".to_string(),
                "ALTER TABLE \"users\" ALTER COLUMN \"changed_col\" TYPE text;".to_string(),
                "ALTER TABLE \"users\" ALTER COLUMN \"changed_col\" SET NOT NULL;".to_string(),
            ]
        );

        // MySQL
        let mysql_stmts = structure_sync_statements(
            &result,
            &DatabaseType::PostgreSQL,
            &DatabaseType::MySQL,
            "`users`",
            parts,
        );
        assert_eq!(
            mysql_stmts,
            vec![
                "ALTER TABLE `users` ADD COLUMN `new_col` VARCHAR(60);".to_string(),
                "ALTER TABLE `users` DROP COLUMN `old_col`;".to_string(),
                "ALTER TABLE `users` MODIFY COLUMN `changed_col` LONGTEXT NOT NULL;".to_string(),
            ]
        );

        // SQLite: ALTER COLUMN becomes a manual comment
        let sqlite_stmts = structure_sync_statements(
            &result,
            &DatabaseType::SQLite,
            &DatabaseType::SQLite,
            "\"users\"",
            parts,
        );
        assert_eq!(
            sqlite_stmts,
            vec![
                "ALTER TABLE \"users\" ADD COLUMN \"new_col\" varchar(60);".to_string(),
                "ALTER TABLE \"users\" DROP COLUMN \"old_col\";".to_string(),
                "-- manual: SQLite does not support ALTER COLUMN for changed_col on \"users\";"
                    .to_string(),
            ]
        );
        assert_eq!(count_executable_statements(&sqlite_stmts), 2);
    }

    #[tokio::test]
    async fn compare_table_structure_works_with_sqlite_memory() {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool_src = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let pool_dst = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();

        sqlx::query("CREATE TABLE t1 (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL);")
            .execute(&pool_src)
            .await
            .unwrap();

        sqlx::query("CREATE TABLE t2 (id INTEGER PRIMARY KEY, name TEXT, extra TEXT);")
            .execute(&pool_dst)
            .await
            .unwrap();

        let ep_src = Endpoint::new(
            crate::models::structs::ConnectionConfig {
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(crate::models::enums::DatabasePool::SQLite(Arc::new(
                pool_src,
            ))),
            None,
        );
        let ep_dst = Endpoint::new(
            crate::models::structs::ConnectionConfig {
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(crate::models::enums::DatabasePool::SQLite(Arc::new(
                pool_dst,
            ))),
            None,
        );

        let opts = StructureOptions::default();
        let outcome = compare_table_structure_connected(&ep_src, &ep_dst, "t1", "t2", &opts)
            .await
            .unwrap();

        assert_eq!(outcome.result.identical, 1); // id
        let (src_only, dst_only, changed) = outcome.result.counts();
        assert_eq!(src_only, 1); // score
        assert_eq!(dst_only, 1); // extra
        assert_eq!(changed, 1); // name (nullable differs)
    }
}
