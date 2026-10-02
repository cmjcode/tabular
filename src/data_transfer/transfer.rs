//! Transfer baris langsung antar koneksi (H8) dan lintas engine dengan
//! aproksimasi tipe (H9). Sumber dibaca per halaman lalu ditulis ke tujuan
//! sebagai `INSERT` multi-baris; tidak ada file perantara.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::catalog::{self, Endpoint};
use super::types::{SourceColumn, create_table_sql};
use super::values::{InsertLimits, ValueKind, build_insert_batches, kind_from_type};
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::{quote_ident, sql_literal};

#[derive(Clone, Debug)]
pub struct TransferOptions {
    /// Buat tabel tujuan bila belum ada (tipe diaproksimasi lintas engine).
    pub create_table: bool,
    /// Kosongkan tabel tujuan sebelum menyalin.
    pub clear_target: bool,
    /// Filter baris sumber (isi klausa `WHERE`, tanpa kata `WHERE`).
    pub where_clause: Option<String>,
    pub row_limit: Option<u64>,
    /// Baris per halaman baca.
    pub page_rows: u64,
    pub insert_limits: InsertLimits,
    /// Lanjut ke tabel berikutnya bila satu tabel gagal.
    pub continue_on_error: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            create_table: true,
            clear_target: false,
            where_clause: None,
            row_limit: None,
            page_rows: 2000,
            insert_limits: InsertLimits::default(),
            continue_on_error: true,
        }
    }
}

/// Satu pasangan tabel sumber -> tujuan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableTransfer {
    pub source_table: String,
    pub target_table: String,
}

/// Keadaan transfer yang dibaca UI tiap frame.
#[derive(Clone, Debug, Default)]
pub struct TransferProgress {
    pub current_table: String,
    pub tables_done: usize,
    pub tables_total: usize,
    pub rows_copied: u64,
    pub log: Vec<String>,
    pub finished: bool,
    pub error: Option<String>,
}

pub type ProgressHandle = Arc<Mutex<TransferProgress>>;

fn with_progress(progress: &ProgressHandle, f: impl FnOnce(&mut TransferProgress)) {
    let mut guard = progress
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard);
}

fn log_line(progress: &ProgressHandle, line: String) {
    log::debug!("[TRANSFER] {line}");
    with_progress(progress, |p| {
        p.log.push(line);
        // Log dibatasi supaya transfer panjang tidak menumpuk memori.
        if p.log.len() > 500 {
            p.log.drain(..100);
        }
    });
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferSummary {
    pub tables_copied: usize,
    pub rows_copied: u64,
    /// `(tabel sumber, pesan)` untuk tabel yang gagal.
    pub failed: Vec<(String, String)>,
}

/// Pasangan kolom sumber -> tujuan yang akan disalin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnPlan {
    pub source: SourceColumn,
    pub target_name: String,
    pub kind: ValueKind,
    /// Sumber boolean ke kolom tujuan non-boolean: tulis `1`/`0`.
    pub bool_as_number: bool,
}

/// Cocokkan kolom sumber dengan kolom tujuan (nama, tanpa membedakan huruf).
/// Mengembalikan rencana dan nama kolom sumber yang dilewati.
pub fn plan_columns(
    source: &[SourceColumn],
    target: &[SourceColumn],
) -> (Vec<ColumnPlan>, Vec<String>) {
    let mut plan = Vec::new();
    let mut skipped = Vec::new();
    for column in source {
        let Some(t) = target
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(&column.name))
        else {
            skipped.push(column.name.clone());
            continue;
        };
        let source_kind = kind_from_type(&column.data_type);
        let target_kind = kind_from_type(&t.data_type);
        let (kind, bool_as_number) = match (source_kind, target_kind) {
            (_, ValueKind::Bool) => (ValueKind::Bool, false),
            (ValueKind::Bool, _) => (ValueKind::Number, true),
            (ValueKind::Binary, _) => (ValueKind::Binary, false),
            (_, ValueKind::Number) => (ValueKind::Number, false),
            (kind, _) => (kind, false),
        };
        plan.push(ColumnPlan {
            source: column.clone(),
            target_name: t.name.clone(),
            kind,
            bool_as_number,
        });
    }
    (plan, skipped)
}

fn normalize_bool(cell: &mut String) {
    match cell.to_ascii_lowercase().as_str() {
        "true" | "t" | "yes" | "y" => *cell = "1".to_string(),
        "false" | "f" | "no" | "n" => *cell = "0".to_string(),
        _ => {}
    }
}

/// SQL Server menolak `INSERT` eksplisit ke kolom identity kecuali
/// `IDENTITY_INSERT` dinyalakan untuk tabel itu.
async fn mssql_has_identity(dst: &Endpoint, table_sql: &str) -> bool {
    if !matches!(dst.db_type(), DatabaseType::MsSQL) {
        return false;
    }
    let sql = format!(
        "SELECT COUNT(*) FROM sys.identity_columns WHERE object_id = OBJECT_ID({})",
        sql_literal(dst.db_type(), table_sql)
    );
    dst.query(&sql)
        .await
        .ok()
        .and_then(|set| set.first_value().map(|v| v != "0"))
        .unwrap_or(false)
}

async fn transfer_one(
    src: &Endpoint,
    dst: &Endpoint,
    pair: &TableTransfer,
    opts: &TransferOptions,
    progress: &ProgressHandle,
    cancel: &AtomicBool,
) -> Result<u64, String> {
    let source_columns = catalog::fetch_columns(src, &pair.source_table).await?;
    let target_sql = dst.table_sql(&pair.target_table);
    if opts.create_table {
        let ddl = create_table_sql(src.db_type(), dst.db_type(), &target_sql, &source_columns);
        dst.execute(&[ddl])
            .await
            .map_err(|e| format!("create table failed: {e}"))?;
    }
    let target_columns = catalog::fetch_columns(dst, &pair.target_table)
        .await
        .map_err(|e| format!("target table: {e}"))?;
    let (plan, skipped) = plan_columns(&source_columns, &target_columns);
    if plan.is_empty() {
        return Err("source and target tables have no column in common".to_string());
    }
    if !skipped.is_empty() {
        log_line(
            progress,
            format!(
                "{}: skipped columns missing in target: {}",
                pair.source_table,
                skipped.join(", ")
            ),
        );
    }
    if opts.clear_target {
        dst.execute(&[format!("DELETE FROM {target_sql}")])
            .await
            .map_err(|e| format!("clearing target failed: {e}"))?;
    }

    let select_list: Vec<String> = plan
        .iter()
        .map(|p| catalog::select_expr(src.db_type(), &p.source))
        .collect();
    // Urut menurut primary key supaya halaman stabil; tanpa PK urutan engine.
    let order_by: Vec<String> = plan
        .iter()
        .filter(|p| p.source.primary_key)
        .map(|p| quote_ident(src.db_type(), &p.source.name))
        .collect();
    let target_cols: Vec<String> = plan
        .iter()
        .map(|p| quote_ident(dst.db_type(), &p.target_name))
        .collect();
    let kinds: Vec<ValueKind> = plan.iter().map(|p| p.kind).collect();
    let source_cols: Vec<usize> = (0..plan.len()).collect();
    let identity = mssql_has_identity(dst, &target_sql).await;
    let source_sql = src.table_sql(&pair.source_table);
    let page = opts.page_rows.max(1);

    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let limit = match opts.row_limit {
            Some(max) if copied >= max => break,
            Some(max) => page.min(max - copied),
            None => page,
        };
        let sql = catalog::select_page_sql(
            src.db_type(),
            &source_sql,
            &select_list,
            opts.where_clause.as_deref(),
            &order_by,
            limit,
            copied,
        );
        let mut rows = src
            .query(&sql)
            .await
            .map_err(|e| format!("reading source failed: {e}"))?
            .rows;
        if rows.is_empty() {
            break;
        }
        catalog::pad_rows(&mut rows, plan.len());
        for (i, p) in plan.iter().enumerate() {
            if p.bool_as_number {
                for row in &mut rows {
                    normalize_bool(&mut row[i]);
                }
            }
        }
        let fetched = rows.len() as u64;
        let mut statements = build_insert_batches(
            dst.db_type(),
            &target_sql,
            &target_cols,
            &rows,
            &source_cols,
            &kinds,
            opts.insert_limits,
        );
        if identity {
            statements.insert(0, format!("SET IDENTITY_INSERT {target_sql} ON"));
            statements.push(format!("SET IDENTITY_INSERT {target_sql} OFF"));
        }
        dst.execute(&statements)
            .await
            .map_err(|e| format!("writing target failed after {copied} rows: {e}"))?;
        copied += fetched;
        with_progress(progress, |p| p.rows_copied += fetched);
        if fetched < limit {
            break;
        }
    }
    Ok(copied)
}

/// Salin beberapa tabel dari `src` ke `dst`. Kedua endpoint disambungkan di
/// sini. `progress` diperbarui sepanjang jalan dan ditandai selesai di akhir,
/// juga bila gagal.
pub async fn transfer_tables(
    src: Endpoint,
    dst: Endpoint,
    tables: Vec<TableTransfer>,
    opts: TransferOptions,
    progress: ProgressHandle,
    cancel: Arc<AtomicBool>,
) -> Result<TransferSummary, String> {
    with_progress(&progress, |p| p.tables_total = tables.len());
    let result = run_transfer(src, dst, &tables, &opts, &progress, &cancel).await;
    with_progress(&progress, |p| {
        p.finished = true;
        p.current_table.clear();
        if let Err(e) = &result {
            p.error = Some(e.clone());
        }
    });
    result
}

async fn run_transfer(
    src: Endpoint,
    dst: Endpoint,
    tables: &[TableTransfer],
    opts: &TransferOptions,
    progress: &ProgressHandle,
    cancel: &AtomicBool,
) -> Result<TransferSummary, String> {
    if tables.is_empty() {
        return Err("No tables selected".to_string());
    }
    let src = src.connect().await.map_err(|e| format!("Source: {e}"))?;
    let dst = dst.connect().await.map_err(|e| format!("Target: {e}"))?;
    log_line(
        progress,
        format!("Transfer {} -> {}", src.label(), dst.label()),
    );
    let mut summary = TransferSummary::default();
    for pair in tables {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".to_string());
        }
        with_progress(progress, |p| p.current_table = pair.source_table.clone());
        match transfer_one(&src, &dst, pair, opts, progress, cancel).await {
            Ok(rows) => {
                summary.tables_copied += 1;
                summary.rows_copied += rows;
                log_line(
                    progress,
                    format!(
                        "{} -> {}: {} rows",
                        pair.source_table, pair.target_table, rows
                    ),
                );
            }
            Err(e) if e == "cancelled" => return Err("Cancelled".to_string()),
            Err(e) => {
                log_line(progress, format!("{}: FAILED: {}", pair.source_table, e));
                if !opts.continue_on_error {
                    return Err(format!("{}: {}", pair.source_table, e));
                }
                summary.failed.push((pair.source_table.clone(), e));
            }
        }
        with_progress(progress, |p| p.tables_done += 1);
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::enums::DatabasePool;
    use crate::models::structs::ConnectionConfig;

    async fn sqlite_endpoint(name: &str) -> Endpoint {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        Endpoint::new(
            ConnectionConfig {
                name: name.to_string(),
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(DatabasePool::SQLite(Arc::new(pool))),
            None,
        )
    }

    fn handles() -> (ProgressHandle, Arc<AtomicBool>) {
        (
            Arc::new(Mutex::new(TransferProgress::default())),
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn pair(source: &str, target: &str) -> TableTransfer {
        TableTransfer {
            source_table: source.to_string(),
            target_table: target.to_string(),
        }
    }

    #[tokio::test]
    async fn copies_rows_and_creates_target_table() {
        let src = sqlite_endpoint("src").await;
        let dst = sqlite_endpoint("dst").await;
        src.query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, note TEXT, pic BLOB); \
             INSERT INTO users VALUES (1, 'it''s; ann', NULL, X'00FF'), (2, 'bob', 'x', NULL), \
             (3, 'cy', '', NULL);",
        )
        .await
        .unwrap();
        let (progress, cancel) = handles();
        let opts = TransferOptions {
            page_rows: 2,
            ..Default::default()
        };
        let summary = transfer_tables(
            src,
            dst.clone(),
            vec![pair("users", "people")],
            opts,
            progress.clone(),
            cancel,
        )
        .await
        .unwrap();
        assert_eq!(summary.rows_copied, 3);
        assert_eq!(summary.tables_copied, 1);
        assert!(summary.failed.is_empty());

        let rows = dst
            // `hex(NULL)` di SQLite adalah string kosong, jadi NULL diperiksa eksplisit.
            .query("SELECT id, name, note, CASE WHEN pic IS NULL THEN NULL ELSE hex(pic) END FROM people ORDER BY id")
            .await
            .unwrap()
            .rows;
        assert_eq!(rows[0], vec!["1", "it's; ann", "NULL", "00FF"]);
        assert_eq!(rows[1], vec!["2", "bob", "x", "NULL"]);
        assert_eq!(rows[2][2], "");
        let p = progress.lock().unwrap();
        assert!(p.finished && p.error.is_none());
        assert_eq!(p.rows_copied, 3);
    }

    #[tokio::test]
    async fn respects_filter_limit_and_clear_target() {
        let src = sqlite_endpoint("src").await;
        let dst = sqlite_endpoint("dst").await;
        src.query(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); \
             INSERT INTO t VALUES (1,'a'),(2,'b'),(3,'c'),(4,'d');",
        )
        .await
        .unwrap();
        dst.query(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t VALUES (99,'old');",
        )
        .await
        .unwrap();
        let (progress, cancel) = handles();
        let opts = TransferOptions {
            create_table: false,
            clear_target: true,
            where_clause: Some("id > 1".to_string()),
            row_limit: Some(2),
            page_rows: 1,
            ..Default::default()
        };
        let summary = transfer_tables(
            src,
            dst.clone(),
            vec![pair("t", "t")],
            opts,
            progress,
            cancel,
        )
        .await
        .unwrap();
        assert_eq!(summary.rows_copied, 2);
        let ids = dst.query("SELECT id FROM t ORDER BY id").await.unwrap();
        assert_eq!(ids.first_column(), vec!["2", "3"]);
    }

    #[tokio::test]
    async fn failed_table_is_reported_and_others_continue() {
        let src = sqlite_endpoint("src").await;
        let dst = sqlite_endpoint("dst").await;
        src.query("CREATE TABLE ok (id INTEGER); INSERT INTO ok VALUES (1);")
            .await
            .unwrap();
        let (progress, cancel) = handles();
        let summary = transfer_tables(
            src,
            dst,
            vec![pair("missing", "missing"), pair("ok", "ok")],
            TransferOptions::default(),
            progress,
            cancel,
        )
        .await
        .unwrap();
        assert_eq!(summary.tables_copied, 1);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0].0, "missing");
    }

    #[test]
    fn column_plan_matches_by_name_and_picks_literal_kind() {
        let col = |name: &str, ty: &str| SourceColumn {
            name: name.to_string(),
            data_type: ty.to_string(),
            nullable: true,
            primary_key: false,
        };
        let source = vec![
            col("ID", "int(11)"),
            col("active", "tinyint(1)"),
            col("flag", "tinyint(1)"),
            col("gone", "text"),
        ];
        let target = vec![
            col("id", "integer"),
            col("active", "boolean"),
            col("flag", "integer"),
        ];
        let (plan, skipped) = plan_columns(&source, &target);
        assert_eq!(skipped, vec!["gone"]);
        assert_eq!(plan[0].target_name, "id");
        assert_eq!(plan[1].kind, ValueKind::Bool);
        assert!(plan[2].bool_as_number);
        assert_eq!(plan[2].kind, ValueKind::Number);
    }
}
