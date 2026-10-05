//! Data Files (H1): buka CSV/TSV/JSON/XLSX/Parquet (juga terkompresi atau
//! terenkripsi) sebagai tabel tanpa mengimpornya ke database milik pengguna.
//!
//! File dimuat ke satu database SQLite lokal ("workspace") di data dir
//! aplikasi; tiap file menjadi satu tabel, sehingga grid, filter, chart,
//! ekspor, dan JOIN antar file memakai mesin yang sama dengan koneksi biasa.
//! SQLite bawaan dipakai supaya tidak ada engine kedua di binary.
//!
//! Asal tiap tabel dicatat di file JSON di samping workspace (bukan di dalam
//! database), sehingga workspace hanya berisi tabel milik pengguna.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use super::TableData;
use super::readers::{self, FileKind, ReadError, ReadOptions};
use super::values::{InferredType, InsertLimits, build_insert_batches, infer_types};
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::quote_ident;

/// Nama koneksi SQLite yang mewakili workspace di sidebar.
pub const WORKSPACE_CONNECTION_NAME: &str = "Data Files";

/// Asal satu tabel workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub source_path: String,
    pub kind: String,
    pub rows: usize,
    /// Waktu muat terakhir, RFC 3339 UTC.
    pub loaded_at: String,
}

/// Daftar tabel workspace yang berasal dari file: nama tabel -> asal.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub tables: BTreeMap<String, RegistryEntry>,
}

fn registry_path(workspace: &Path) -> PathBuf {
    workspace.with_extension("json")
}

/// Baca daftar tabel; file yang hilang atau rusak dianggap kosong.
pub fn read_registry(workspace: &Path) -> Registry {
    std::fs::read_to_string(registry_path(workspace))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_registry(workspace: &Path, registry: &Registry) -> Result<(), DataFileError> {
    let text = serde_json::to_string_pretty(registry).map_err(ws)?;
    std::fs::write(registry_path(workspace), text).map_err(ws)
}

/// Lokasi database workspace.
pub fn workspace_path() -> PathBuf {
    crate::config::get_data_dir().join("data_files.sqlite")
}

#[derive(Debug, thiserror::Error)]
pub enum DataFileError {
    #[error(transparent)]
    Read(#[from] ReadError),
    #[error("Data Files workspace: {0}")]
    Workspace(String),
}

impl DataFileError {
    /// File terenkripsi dan passphrase belum (atau salah) diberikan.
    pub fn needs_passphrase(&self) -> bool {
        matches!(
            self,
            DataFileError::Read(ReadError::NeedsPassphrase | ReadError::Decrypt(_))
        )
    }
}

fn ws(e: impl std::fmt::Display) -> DataFileError {
    DataFileError::Workspace(e.to_string())
}

/// Tabel yang baru dimuat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedTable {
    pub table: String,
    pub rows: usize,
    pub columns: usize,
    pub source: PathBuf,
    pub kind: FileKind,
}

pub async fn open_workspace(path: &Path) -> Result<SqlitePool, DataFileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(ws)?;
    }
    // `filename` (bukan URL) supaya path berisi spasi, `#`, `?`, atau
    // backslash Windows tidak salah diurai.
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        // GUI bisa sedang membaca workspace lewat pool koneksinya sendiri.
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(ws)?;
    Ok(pool)
}

/// Nama tabel untuk `source`: `base`, kecuali nama itu sudah dipakai file
/// lain atau tabel buatan pengguna, maka `base_2`, `base_3`, … File yang sama
/// selalu mendapat nama yang sama sehingga membukanya lagi memuat ulang
/// tabelnya.
async fn resolve_table_name(
    pool: &SqlitePool,
    registry: &Registry,
    base: &str,
    source: &str,
) -> Result<String, DataFileError> {
    let mut candidate = base.to_string();
    let mut n = 2;
    loop {
        let taken = match registry.tables.get(&candidate) {
            Some(entry) => entry.source_path != source,
            // Tidak tercatat: bebas kecuali pengguna membuat tabel bernama itu.
            None => {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ? COLLATE NOCASE",
                )
                .bind(&candidate)
                .fetch_one(pool)
                .await
                .map_err(ws)?
                    > 0
            }
        };
        if !taken {
            return Ok(candidate);
        }
        candidate = format!("{base}_{n}");
        n += 1;
    }
}

/// Muat `data` sebagai tabel `base_name` (diganti bila sudah ada). Tipe kolom
/// diinferensi dari isi supaya urut, agregat, dan chart bekerja pada angka.
pub async fn load_table(
    pool: &SqlitePool,
    workspace: &Path,
    base_name: &str,
    data: &TableData,
    source: &Path,
    kind: FileKind,
) -> Result<LoadedTable, DataFileError> {
    let db = DatabaseType::SQLite;
    let source_key = source.to_string_lossy().into_owned();
    // Lembar kerja spreadsheet memakai sumber `path#sheet` supaya tiap sheet
    // punya tabel sendiri.
    let mut registry = read_registry(workspace);
    let table = resolve_table_name(pool, &registry, base_name, &source_key).await?;
    let table_sql = quote_ident(&db, &table);
    let types = infer_types(data);
    let column_defs: Vec<String> = data
        .headers
        .iter()
        .zip(&types)
        .map(|(h, t)| format!("{} {}", quote_ident(&db, h), t.sql_type(&db)))
        .collect();
    if column_defs.is_empty() {
        return Err(DataFileError::Read(ReadError::Parse(
            "file has no columns".to_string(),
        )));
    }
    let columns: Vec<String> = data.headers.iter().map(|h| quote_ident(&db, h)).collect();
    let kinds: Vec<_> = types.iter().copied().map(InferredType::kind).collect();
    let source_cols: Vec<usize> = (0..data.headers.len()).collect();
    let inserts = build_insert_batches(
        &db,
        &table_sql,
        &columns,
        data,
        &source_cols,
        &kinds,
        InsertLimits::default(),
    );

    let mut tx = pool.begin().await.map_err(ws)?;
    let mut statements = vec![
        format!("DROP TABLE IF EXISTS {table_sql}"),
        format!("CREATE TABLE {table_sql} ({})", column_defs.join(", ")),
    ];
    statements.extend(inserts);
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut *tx)
            .await
            .map_err(ws)?;
    }
    tx.commit().await.map_err(ws)?;
    registry.tables.insert(
        table.clone(),
        RegistryEntry {
            source_path: source_key,
            kind: kind.label().to_string(),
            rows: data.rows.len(),
            loaded_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    write_registry(workspace, &registry)?;

    Ok(LoadedTable {
        table,
        rows: data.rows.len(),
        columns: data.headers.len(),
        source: source.to_path_buf(),
        kind,
    })
}

fn sheet_suffix(sheet: &str) -> String {
    let cleaned: String = sheet
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    cleaned.trim_matches('_').to_string()
}

/// Baca `file` dan muat ke workspace di `workspace`. Spreadsheet dengan
/// beberapa sheet menghasilkan satu tabel per sheet (kecuali `opts.sheet`
/// menunjuk satu sheet).
pub async fn load_file(
    workspace: &Path,
    file: &Path,
    opts: &ReadOptions,
) -> Result<Vec<LoadedTable>, DataFileError> {
    let read = {
        let file = file.to_path_buf();
        let opts = opts.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<readers::LoadedFile>, ReadError> {
            let first = readers::read_file(&file, &opts)?;
            let mut all = Vec::new();
            if opts.sheet.is_none() && first.sheets.len() > 1 {
                let others: Vec<String> = first.sheets[1..].to_vec();
                all.push(first);
                for sheet in others {
                    let sheet_opts = ReadOptions {
                        sheet: Some(sheet),
                        ..opts.clone()
                    };
                    all.push(readers::read_file(&file, &sheet_opts)?);
                }
            } else {
                all.push(first);
            }
            Ok(all)
        })
        .await
        .map_err(ws)??
    };

    let pool = open_workspace(workspace).await?;
    let base = readers::table_name_for(file);
    let multi = read.len() > 1;
    let mut loaded = Vec::new();
    for item in &read {
        let (name, source) = match (&item.sheet, multi) {
            (Some(sheet), true) => (
                format!("{base}_{}", sheet_suffix(sheet)),
                PathBuf::from(format!("{}#{}", file.display(), sheet)),
            ),
            _ => (base.clone(), file.to_path_buf()),
        };
        // Sheet kosong tidak dijadikan tabel.
        if item.data.headers.is_empty() {
            continue;
        }
        loaded.push(load_table(&pool, workspace, &name, &item.data, &source, item.kind).await?);
    }
    pool.close().await;
    if loaded.is_empty() {
        return Err(DataFileError::Read(ReadError::Parse(
            "file has no data".to_string(),
        )));
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tabular_datafiles_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn loads_csv_as_typed_table_and_reloads_same_file() {
        let dir = temp_dir("csv");
        let file = dir.join("sales 2026.csv");
        std::fs::write(&file, "id,amount,city\n1,10.5,Jakarta\n2,,Bandung\n").unwrap();
        let workspace = dir.join("ws.sqlite");

        let loaded = load_file(&workspace, &file, &ReadOptions::default())
            .await
            .unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].table, "sales_2026");
        assert_eq!(loaded[0].rows, 2);

        let pool = open_workspace(&workspace).await.unwrap();
        let (sum, count): (f64, i64) =
            sqlx::query_as("SELECT SUM(amount), COUNT(amount) FROM sales_2026")
                .fetch_one(&pool)
                .await
                .unwrap();
        // Kolom angka bertipe REAL dan sel kosong menjadi NULL.
        assert_eq!((sum, count), (10.5, 1));
        let (ty,): (String,) =
            sqlx::query_as("SELECT type FROM pragma_table_info('sales_2026') WHERE name = 'id'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(ty, "INTEGER");
        pool.close().await;

        // File yang sama dimuat ulang ke tabel yang sama.
        std::fs::write(&file, "id,amount,city\n9,1,Medan\n").unwrap();
        let again = load_file(&workspace, &file, &ReadOptions::default())
            .await
            .unwrap();
        assert_eq!(again[0].table, "sales_2026");
        assert_eq!(again[0].rows, 1);

        // File lain dengan nama dasar sama mendapat tabel sendiri.
        let other_dir = dir.join("other");
        std::fs::create_dir_all(&other_dir).unwrap();
        let other = other_dir.join("sales 2026.csv");
        std::fs::write(&other, "x\n1\n").unwrap();
        let third = load_file(&workspace, &other, &ReadOptions::default())
            .await
            .unwrap();
        assert_eq!(third[0].table, "sales_2026_2");

        // Asal tabel dicatat di samping workspace, bukan sebagai tabel.
        let registry = read_registry(&workspace);
        assert_eq!(registry.tables.len(), 2);
        assert_eq!(registry.tables["sales_2026"].rows, 1);
        assert_eq!(registry.tables["sales_2026"].kind, "Delimited text");
        let pool = open_workspace(&workspace).await.unwrap();
        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        let names: Vec<&str> = tables.iter().map(|t| t.0.as_str()).collect();
        assert_eq!(names, vec!["sales_2026", "sales_2026_2"]);

        // Tabel buatan pengguna dengan nama yang sama tidak ditimpa.
        sqlx::query("CREATE TABLE mine (v TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let own = dir.join("mine.csv");
        std::fs::write(&own, "a\n1\n").unwrap();
        let fourth = load_file(&workspace, &own, &ReadOptions::default())
            .await
            .unwrap();
        assert_eq!(fourth[0].table, "mine_2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn encrypted_file_reports_missing_passphrase() {
        let dir = temp_dir("enc");
        let file = dir.join("secret.csv.enc");
        let sealed = super::super::encrypt::encrypt_bytes("pw", b"a,b\n1,2\n").unwrap();
        std::fs::write(&file, sealed).unwrap();
        let workspace = dir.join("ws.sqlite");

        let err = load_file(&workspace, &file, &ReadOptions::default())
            .await
            .unwrap_err();
        assert!(err.needs_passphrase());

        let opts = ReadOptions {
            passphrase: Some("pw".to_string()),
            ..Default::default()
        };
        let loaded = load_file(&workspace, &file, &opts).await.unwrap();
        assert_eq!(loaded[0].table, "secret");
        assert_eq!(loaded[0].columns, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
