//! Titik akhir (koneksi + database) untuk transfer/compare/ekspor objek, dan
//! query katalog yang dibutuhkan: daftar tabel, kolom, primary key, halaman
//! data. Headless.

use super::cell_from_executor;
use super::types::SourceColumn;
use super::values::{ValueKind, kind_from_type, sql_value};
use crate::connection::pool::create_connection_pool_for_config;
use crate::models::enums::{DatabasePool, DatabaseType};
use crate::models::structs::ConnectionConfig;
use crate::schema_objects::exec::run_statements_in_database;
use crate::schema_objects::sql::{quote_ident, quote_qualified, split_qualified, sql_literal};
use crate::schema_objects::{ResultSet, run_in_database};

/// Engine yang bisa menjadi sumber/tujuan transfer SQL.
pub fn supports_sql(db: &DatabaseType) -> bool {
    matches!(
        db,
        DatabaseType::MySQL | DatabaseType::PostgreSQL | DatabaseType::SQLite | DatabaseType::MsSQL
    )
}

/// Satu sisi operasi: koneksi, pool (bila sudah ada), dan database aktif.
#[derive(Clone)]
pub struct Endpoint {
    pub conn: ConnectionConfig,
    pub pool: Option<DatabasePool>,
    pub database: Option<String>,
}

impl Endpoint {
    pub fn new(
        conn: ConnectionConfig,
        pool: Option<DatabasePool>,
        database: Option<String>,
    ) -> Self {
        let database = database.filter(|d| !d.trim().is_empty());
        Self {
            conn,
            pool,
            database,
        }
    }

    pub fn db_type(&self) -> &DatabaseType {
        &self.conn.connection_type
    }

    /// "Nama koneksi / database" untuk log dan judul.
    pub fn label(&self) -> String {
        match &self.database {
            Some(db) if !matches!(self.db_type(), DatabaseType::SQLite) => {
                format!("{} / {}", self.conn.name, db)
            }
            _ => self.conn.name.clone(),
        }
    }

    /// Pastikan ada pool yang menunjuk database yang benar. PostgreSQL terikat
    /// satu database per pool, jadi database lain memakai pool tersendiri yang
    /// hidup selama operasi (bukan satu pool per statement).
    pub async fn connect(mut self) -> Result<Self, String> {
        if !supports_sql(self.db_type()) {
            return Err(format!(
                "{} connections are not supported here",
                self.conn.connection_type.as_db_str()
            ));
        }
        if matches!(self.db_type(), DatabaseType::PostgreSQL)
            && let Some(db) = self.database.clone()
            && db != self.conn.database
        {
            self.conn.database = db;
            self.pool = Some(create_connection_pool_for_config(&self.conn).await?);
        } else if self.pool.is_none() {
            self.pool = Some(create_connection_pool_for_config(&self.conn).await?);
        }
        Ok(self)
    }

    /// Jalankan SQL dan kembalikan result set terakhir.
    pub async fn query(&self, sql: &str) -> Result<ResultSet, String> {
        let mut sets =
            run_in_database(&self.conn, self.pool.clone(), self.database.as_deref(), sql).await?;
        Ok(sets.pop().unwrap_or_default())
    }

    /// Jalankan statement yang sudah jadi, tanpa dipecah lagi.
    pub async fn execute(&self, statements: &[String]) -> Result<(), String> {
        if statements.is_empty() {
            return Ok(());
        }
        run_statements_in_database(
            &self.conn,
            self.pool.clone(),
            self.database.as_deref(),
            statements,
        )
        .await
        .map(|_| ())
    }

    /// Jalankan `statements` dalam satu transaksi di satu koneksi: semuanya
    /// tersimpan atau tidak sama sekali. Statement diambil satu per satu dari
    /// iterator, jadi pemanggil bisa menyusunnya sambil jalan tanpa menahan
    /// seluruh skrip di memori. Bila satu statement gagal transaksi dibatalkan
    /// dan sisanya tidak dijalankan. Mengembalikan jumlah statement yang
    /// dijalankan.
    ///
    /// Endpoint harus sudah tersambung ([`Endpoint::connect`]). Atomik hanya
    /// sejauh engine-nya transaksional: tabel MySQL MyISAM, dan DDL di MySQL
    /// (commit implisit), tidak ikut dibatalkan.
    pub async fn execute_atomic<I>(&self, statements: I) -> Result<usize, String>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
        I::IntoIter: Send,
    {
        let err = |e: sqlx::Error| e.to_string();
        let pool = self
            .pool
            .clone()
            .ok_or_else(|| "Connection is not open".to_string())?;
        let database = self
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());
        let mut statements = statements.into_iter();
        let mut done = 0usize;
        match pool {
            DatabasePool::MySQL(p) => {
                let mut c = p.acquire().await.map_err(err)?;
                if let Some(db) = database {
                    // `USE` ditolak protokol prepared statement; lihat `schema_objects::exec`.
                    let use_stmt = format!("USE `{}`", db.replace('`', "``"));
                    sqlx::raw_sql(sqlx::AssertSqlSafe(use_stmt))
                        .execute(&mut *c)
                        .await
                        .map_err(err)?;
                }
                let mut tx = sqlx::Connection::begin(&mut *c).await.map_err(err)?;
                for stmt in statements.by_ref() {
                    let run = sqlx::query(sqlx::AssertSqlSafe(stmt.as_ref().to_string()))
                        .execute(&mut *tx)
                        .await;
                    if let Err(e) = run {
                        undo_failed(tx.rollback().await);
                        return Err(e.to_string());
                    }
                    done += 1;
                }
                tx.commit().await.map_err(err)?;
            }
            DatabasePool::PostgreSQL(p) => {
                let mut tx = p.begin().await.map_err(err)?;
                for stmt in statements.by_ref() {
                    let run = sqlx::query(sqlx::AssertSqlSafe(stmt.as_ref().to_string()))
                        .execute(&mut *tx)
                        .await;
                    if let Err(e) = run {
                        undo_failed(tx.rollback().await);
                        return Err(e.to_string());
                    }
                    done += 1;
                }
                tx.commit().await.map_err(err)?;
            }
            DatabasePool::SQLite(p) => {
                let mut tx = p.begin().await.map_err(err)?;
                for stmt in statements.by_ref() {
                    let run = sqlx::query(sqlx::AssertSqlSafe(stmt.as_ref().to_string()))
                        .execute(&mut *tx)
                        .await;
                    if let Err(e) = run {
                        undo_failed(tx.rollback().await);
                        return Err(e.to_string());
                    }
                    done += 1;
                }
                tx.commit().await.map_err(err)?;
            }
            DatabasePool::MsSQL(p) => {
                let mut c = p.get().await.map_err(|e| e.to_string())?;
                let client = c
                    .client_mut()
                    .ok_or_else(|| "MsSQL pooled connection unavailable".to_string())?;
                if let Some(db) = database {
                    let use_sql = format!("USE [{}]", db.replace(']', "]]"));
                    client
                        .simple_query(use_sql.as_str())
                        .await
                        .map_err(|e| e.to_string())?;
                }
                crate::driver_mssql::run_query_multi(client, MSSQL_BEGIN).await?;
                for stmt in statements.by_ref() {
                    if let Err(e) =
                        crate::driver_mssql::run_query_multi(client, stmt.as_ref()).await
                    {
                        // Koneksi kembali ke pool; transaksi tidak boleh ikut terbawa.
                        let undo = crate::driver_mssql::run_query_multi(client, MSSQL_UNDO).await;
                        if let Err(r) = undo {
                            log::warn!("[TRANSFER] undoing the transaction failed: {r}");
                        }
                        return Err(e);
                    }
                    done += 1;
                }
                crate::driver_mssql::run_query_multi(client, MSSQL_COMMIT).await?;
            }
            _ => return Err("This connection type does not support SQL".to_string()),
        }
        Ok(done)
    }

    /// Nama tabel terkutip untuk endpoint ini.
    pub fn table_sql(&self, table: &str) -> String {
        quote_qualified(self.db_type(), table)
    }
}

const MSSQL_BEGIN: &str = "BEGIN TRANSACTION";
const MSSQL_COMMIT: &str = "COMMIT TRANSACTION";
const MSSQL_UNDO: &str = concat!("IF @@TRANCOUNT > 0 ROLL", "BACK TRANSACTION");

fn undo_failed(result: Result<(), sqlx::Error>) {
    if let Err(e) = result {
        log::warn!("[TRANSFER] undoing the transaction failed: {e}");
    }
}

/// Query daftar database pengguna (tanpa database sistem). `None` untuk engine
/// yang hanya punya satu database per koneksi.
pub fn list_databases_sql(db: &DatabaseType) -> Option<&'static str> {
    match db {
        DatabaseType::MySQL => Some(
            "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN \
             ('information_schema', 'performance_schema', 'mysql', 'sys') ORDER BY SCHEMA_NAME",
        ),
        DatabaseType::PostgreSQL => Some(
            "SELECT datname::text FROM pg_database WHERE NOT datistemplate AND datallowconn \
             ORDER BY 1",
        ),
        DatabaseType::MsSQL => Some(
            "SELECT name FROM sys.databases WHERE name NOT IN \
             ('master', 'tempdb', 'model', 'msdb') ORDER BY name",
        ),
        _ => None,
    }
}

/// Daftar database koneksi; dijalankan di database bawaan koneksi, bukan di
/// `ep.database`, supaya tetap jalan saat database itu belum dipilih.
pub async fn list_databases(ep: &Endpoint) -> Result<Vec<String>, String> {
    let sql = list_databases_sql(ep.db_type())
        .ok_or_else(|| "This connection type has a single database".to_string())?;
    let mut sets = run_in_database(&ep.conn, ep.pool.clone(), None, sql).await?;
    Ok(sets.pop().unwrap_or_default().first_column())
}

/// Query daftar tabel (satu kolom: nama; berkualifikasi schema bila bukan
/// schema default).
pub fn list_tables_sql(db: &DatabaseType, database: Option<&str>) -> Option<String> {
    match db {
        DatabaseType::MySQL => Some(format!(
            "SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = {} \
             AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
            database
                .map(|d| sql_literal(db, d))
                .unwrap_or_else(|| "DATABASE()".to_string())
        )),
        DatabaseType::PostgreSQL => Some(
            "SELECT CASE WHEN table_schema = 'public' THEN table_name \
             ELSE table_schema || '.' || table_name END \
             FROM information_schema.tables WHERE table_type = 'BASE TABLE' \
             AND table_schema NOT IN ('pg_catalog', 'information_schema') ORDER BY 1"
                .to_string(),
        ),
        DatabaseType::SQLite => Some(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name"
                .to_string(),
        ),
        DatabaseType::MsSQL => Some(
            "SELECT s.name + '.' + t.name FROM sys.tables t \
             JOIN sys.schemas s ON s.schema_id = t.schema_id ORDER BY 1"
                .to_string(),
        ),
        _ => None,
    }
}

pub async fn list_tables(ep: &Endpoint) -> Result<Vec<String>, String> {
    let sql = list_tables_sql(ep.db_type(), ep.database.as_deref())
        .ok_or_else(|| "This connection type has no tables".to_string())?;
    Ok(ep.query(&sql).await?.first_column())
}

/// Query kolom tabel. Hasil selain SQLite: nama, tipe lengkap, `YES`/`NO`
/// nullable, `PRI` bila primary key. SQLite memakai `PRAGMA table_info`.
pub fn columns_sql(db: &DatabaseType, database: Option<&str>, table: &str) -> Option<String> {
    match db {
        DatabaseType::MySQL => {
            let (schema, name) = split_qualified(table, database.unwrap_or(""));
            Some(format!(
                "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_KEY \
                 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = {} AND TABLE_NAME = {} \
                 ORDER BY ORDINAL_POSITION",
                if schema.is_empty() {
                    "DATABASE()".to_string()
                } else {
                    sql_literal(db, &schema)
                },
                sql_literal(db, &name)
            ))
        }
        DatabaseType::PostgreSQL => Some(format!(
            "SELECT a.attname::text, format_type(a.atttypid, a.atttypmod), \
             CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END, \
             CASE WHEN EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = a.attrelid \
               AND i.indisprimary AND a.attnum = ANY(i.indkey)) THEN 'PRI' ELSE '' END \
             FROM pg_attribute a WHERE a.attrelid = {}::regclass AND a.attnum > 0 \
             AND NOT a.attisdropped ORDER BY a.attnum",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::MsSQL => Some(format!(
            "SELECT c.name, \
             CASE WHEN t.name IN ('varchar', 'char', 'varbinary', 'binary') THEN t.name + '(' + \
               CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length AS varchar(10)) END + ')' \
             WHEN t.name IN ('nvarchar', 'nchar') THEN t.name + '(' + \
               CASE WHEN c.max_length = -1 THEN 'max' ELSE CAST(c.max_length / 2 AS varchar(10)) END + ')' \
             WHEN t.name IN ('decimal', 'numeric') THEN t.name + '(' + \
               CAST(c.precision AS varchar(10)) + ',' + CAST(c.scale AS varchar(10)) + ')' \
             ELSE t.name END, \
             CASE WHEN c.is_nullable = 1 THEN 'YES' ELSE 'NO' END, \
             CASE WHEN EXISTS (SELECT 1 FROM sys.index_columns ic \
               JOIN sys.indexes i ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
               WHERE i.is_primary_key = 1 AND ic.object_id = c.object_id \
               AND ic.column_id = c.column_id) THEN 'PRI' ELSE '' END \
             FROM sys.columns c JOIN sys.types t ON t.user_type_id = c.user_type_id \
             WHERE c.object_id = OBJECT_ID({}) ORDER BY c.column_id",
            sql_literal(db, &quote_qualified(db, table))
        )),
        DatabaseType::SQLite => Some(format!(
            "PRAGMA table_info({})",
            quote_ident(db, &split_qualified(table, "").1)
        )),
        _ => None,
    }
}

/// Urai hasil [`columns_sql`].
pub fn parse_columns(db: &DatabaseType, set: &ResultSet) -> Vec<SourceColumn> {
    let cell = |row: &Vec<String>, i: usize| row.get(i).cloned().unwrap_or_default();
    set.rows
        .iter()
        .map(|row| {
            if matches!(db, DatabaseType::SQLite) {
                // cid, name, type, notnull, dflt_value, pk
                let pk = cell(row, 5);
                SourceColumn {
                    name: cell(row, 1),
                    data_type: match cell(row, 2).as_str() {
                        "NULL" => String::new(),
                        t => t.to_string(),
                    },
                    nullable: cell(row, 3) != "1",
                    primary_key: pk != "0" && pk != "NULL" && !pk.is_empty(),
                }
            } else {
                SourceColumn {
                    name: cell(row, 0),
                    data_type: cell(row, 1),
                    nullable: !cell(row, 2).eq_ignore_ascii_case("NO"),
                    primary_key: cell(row, 3).eq_ignore_ascii_case("PRI"),
                }
            }
        })
        .filter(|c| !c.name.is_empty())
        .collect()
}

pub async fn fetch_columns(ep: &Endpoint, table: &str) -> Result<Vec<SourceColumn>, String> {
    let sql = columns_sql(ep.db_type(), ep.database.as_deref(), table)
        .ok_or_else(|| "This connection type has no table columns".to_string())?;
    let columns = parse_columns(ep.db_type(), &ep.query(&sql).await?);
    if columns.is_empty() {
        return Err(format!("Table \"{table}\" not found or has no columns"));
    }
    Ok(columns)
}

/// Ekspresi SELECT untuk satu kolom. BLOB SQLite dibaca sebagai heksadesimal
/// karena driver hanya menampilkan ukurannya.
pub fn select_expr(db: &DatabaseType, column: &SourceColumn) -> String {
    let ident = quote_ident(db, &column.name);
    if matches!(db, DatabaseType::SQLite) && kind_from_type(&column.data_type) == ValueKind::Binary
    {
        format!("CASE WHEN {ident} IS NULL THEN NULL ELSE '0x' || hex({ident}) END AS {ident}")
    } else {
        ident
    }
}

/// Klausa `WHERE` dari filter pengguna (boleh kosong).
fn where_sql(where_clause: Option<&str>) -> String {
    match where_clause.map(str::trim).filter(|w| !w.is_empty()) {
        Some(w) => format!(" WHERE {w}"),
        None => String::new(),
    }
}

/// `SELECT` satu halaman dengan `OFFSET`. `order_by` berisi ekspresi yang
/// sudah dikutip; bila kosong urutannya tidak dijamin stabil antar halaman.
/// Biayanya O(offset) per halaman; pakai [`TablePager`] yang memilih keyset
/// bila tabel punya primary key.
pub fn select_page_sql(
    db: &DatabaseType,
    table_sql: &str,
    select_list: &[String],
    where_clause: Option<&str>,
    order_by: &[String],
    limit: u64,
    offset: u64,
) -> String {
    let mut sql = format!("SELECT {} FROM {}", select_list.join(", "), table_sql);
    sql.push_str(&where_sql(where_clause));
    match db {
        DatabaseType::MsSQL => {
            let order = if order_by.is_empty() {
                "(SELECT NULL)".to_string()
            } else {
                order_by.join(", ")
            };
            sql.push_str(&format!(
                " ORDER BY {order} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY"
            ));
        }
        _ => {
            if !order_by.is_empty() {
                sql.push_str(&format!(" ORDER BY {}", order_by.join(", ")));
            }
            sql.push_str(&format!(" LIMIT {limit} OFFSET {offset}"));
        }
    }
    sql
}

/// `SELECT` terurut tanpa `OFFSET`. `limit = None` berarti semua baris (tanpa
/// klausa batas sama sekali, bukan angka sentinel).
pub fn select_ordered_sql(
    db: &DatabaseType,
    table_sql: &str,
    select_list: &[String],
    where_clause: Option<&str>,
    order_by: &[String],
    limit: Option<u64>,
) -> String {
    let top = match (db, limit) {
        (DatabaseType::MsSQL, Some(n)) => format!("TOP ({n}) "),
        _ => String::new(),
    };
    let mut sql = format!("SELECT {top}{} FROM {}", select_list.join(", "), table_sql);
    sql.push_str(&where_sql(where_clause));
    if !order_by.is_empty() {
        sql.push_str(&format!(" ORDER BY {}", order_by.join(", ")));
    }
    if let Some(n) = limit
        && !matches!(db, DatabaseType::MsSQL)
    {
        sql.push_str(&format!(" LIMIT {n}"));
    }
    sql
}

/// Predikat "kunci lebih besar dari `last`" untuk keyset pagination.
/// `key_sql` sudah dikutip, `last` sudah berupa literal SQL. Kunci majemuk
/// memakai perbandingan row value; SQL Server tidak punya itu, jadi
/// dijabarkan menjadi rantai `OR`/`AND` yang setara.
pub fn keyset_predicate(db: &DatabaseType, key_sql: &[String], last: &[String]) -> String {
    match (key_sql.len(), db) {
        (0, _) => String::new(),
        (1, _) => format!("{} > {}", key_sql[0], last[0]),
        (n, DatabaseType::MsSQL) => (0..n)
            .map(|i| {
                let mut terms: Vec<String> = (0..i)
                    .map(|j| format!("{} = {}", key_sql[j], last[j]))
                    .collect();
                terms.push(format!("{} > {}", key_sql[i], last[i]));
                format!("({})", terms.join(" AND "))
            })
            .collect::<Vec<_>>()
            .join(" OR "),
        _ => format!("({}) > ({})", key_sql.join(", "), last.join(", ")),
    }
}

/// `SELECT` satu halaman keyset: baris setelah kunci `last` (atau dari awal
/// bila `None`), urut menurut kunci. Biayanya tidak bergantung pada posisi
/// halaman, dan baris yang sudah ada tidak terlewat atau terulang walau
/// tabel berubah selama dibaca.
pub fn select_keyset_sql(
    db: &DatabaseType,
    table_sql: &str,
    select_list: &[String],
    where_clause: Option<&str>,
    key_sql: &[String],
    last: Option<&[String]>,
    limit: u64,
) -> String {
    let predicate = last
        .map(|last| keyset_predicate(db, key_sql, last))
        .unwrap_or_default();
    let filter = match (where_clause.map(str::trim).filter(|w| !w.is_empty()), last) {
        (Some(w), Some(_)) => format!(" WHERE ({w}) AND ({predicate})"),
        (Some(w), None) => format!(" WHERE {w}"),
        (None, Some(_)) => format!(" WHERE {predicate}"),
        (None, None) => String::new(),
    };
    let order = key_sql.join(", ");
    let list = select_list.join(", ");
    match db {
        DatabaseType::MsSQL => {
            format!("SELECT TOP ({limit}) {list} FROM {table_sql}{filter} ORDER BY {order}")
        }
        _ => format!("SELECT {list} FROM {table_sql}{filter} ORDER BY {order} LIMIT {limit}"),
    }
}

fn base_type(type_name: &str) -> String {
    let t = type_name.trim().to_ascii_lowercase();
    t.split(['(', ' ']).next().unwrap_or("").to_string()
}

/// True bila nilai kolom bertipe ini kembali dari eksekutor sebagai teks
/// yang, ditulis ulang sebagai literal, sama persis dengan nilai aslinya dan
/// dibandingkan dengan aturan yang sama seperti `ORDER BY` kolomnya. Hanya
/// tipe seperti itu yang aman jadi kunci keyset: float dan timestamp bisa
/// kehilangan presisi di tampilan, dan `varchar` SQL Server dibandingkan
/// dengan aturan lain begitu bertemu literal `N'..'`.
pub fn keyset_safe_type(db: &DatabaseType, type_name: &str) -> bool {
    if type_name.trim().eq_ignore_ascii_case("tinyint(1)") {
        return false;
    }
    let base = base_type(type_name);
    let exact_number = matches!(
        base.as_str(),
        "int"
            | "integer"
            | "tinyint"
            | "smallint"
            | "mediumint"
            | "bigint"
            | "int2"
            | "int4"
            | "int8"
            | "serial"
            | "bigserial"
            | "smallserial"
            | "decimal"
            | "numeric"
    );
    let unicode_text = matches!(
        base.as_str(),
        "nvarchar" | "nchar" | "uuid" | "uniqueidentifier"
    );
    let text = matches!(
        base.as_str(),
        "char" | "character" | "varchar" | "text" | "bpchar" | "citext"
    );
    exact_number || unicode_text || (text && !matches!(db, DatabaseType::MsSQL))
}

/// True bila kolom bertipe ini boleh masuk `ORDER BY` di dialek `db`.
/// MySQL dan SQLite mengurutkan tipe apa pun; untuk PostgreSQL dan SQL Server
/// hanya tipe yang pasti punya urutan yang diikutkan (`json`, `xml`, dan
/// `text`/`image` SQL Server ditolak engine).
fn orderable_type(db: &DatabaseType, type_name: &str) -> bool {
    if matches!(db, DatabaseType::MySQL | DatabaseType::SQLite) {
        return true;
    }
    let base = base_type(type_name);
    match kind_from_type(type_name) {
        ValueKind::Number | ValueKind::Bool => true,
        ValueKind::Binary => base != "image",
        ValueKind::Text => {
            matches!(
                base.as_str(),
                "char"
                    | "character"
                    | "varchar"
                    | "nvarchar"
                    | "nchar"
                    | "bpchar"
                    | "citext"
                    | "uuid"
                    | "uniqueidentifier"
                    | "date"
                    | "time"
                    | "timetz"
                    | "timestamp"
                    | "timestamptz"
                    | "datetime"
                    | "datetime2"
                    | "smalldatetime"
                    | "datetimeoffset"
                    | "interval"
            ) || (matches!(db, DatabaseType::PostgreSQL)
                && matches!(base.as_str(), "text" | "jsonb"))
        }
    }
}

/// Dari mana nullness satu kolom hasil diketahui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NullSource {
    /// Kolom `NOT NULL`: teks apa pun adalah nilai.
    Never,
    /// Indeks kolom indikator (`1` = NULL) di baris hasil.
    Flag(usize),
    /// Hanya penanda teks eksekutor; aman untuk angka dan boolean yang
    /// tidak mungkin bertuliskan `NULL`.
    Marker,
}

/// Daftar `SELECT` untuk kolom data, ditambah yang dibutuhkan untuk membaca
/// hasilnya dengan benar: indikator NULL untuk kolom teks nullable (eksekutor
/// tidak membedakan SQL NULL dari string `NULL`) dan kolom kunci yang tidak
/// ikut di daftar data.
#[derive(Clone, Debug)]
pub struct PageSelect {
    pub select_list: Vec<String>,
    nulls: Vec<NullSource>,
    /// Posisi tiap kolom kunci di baris hasil.
    key_positions: Vec<usize>,
}

impl PageSelect {
    pub fn new(db: &DatabaseType, columns: &[SourceColumn], key: &[SourceColumn]) -> Self {
        let mut select_list: Vec<String> = columns.iter().map(|c| select_expr(db, c)).collect();
        let mut nulls = Vec::with_capacity(columns.len());
        for (i, column) in columns.iter().enumerate() {
            let kind = kind_from_type(&column.data_type);
            if !column.nullable {
                nulls.push(NullSource::Never);
            } else if matches!(kind, ValueKind::Number | ValueKind::Bool) {
                nulls.push(NullSource::Marker);
            } else {
                nulls.push(NullSource::Flag(select_list.len()));
                select_list.push(format!(
                    "CASE WHEN {} IS NULL THEN 1 ELSE 0 END AS {}",
                    quote_ident(db, &column.name),
                    quote_ident(db, &format!("tabular_null_{i}"))
                ));
            }
        }
        let mut key_positions = Vec::with_capacity(key.len());
        for k in key {
            match columns.iter().position(|c| c.name == k.name) {
                Some(i) => key_positions.push(i),
                None => {
                    key_positions.push(select_list.len());
                    select_list.push(quote_ident(db, &k.name));
                }
            }
        }
        Self {
            select_list,
            nulls,
            key_positions,
        }
    }

    /// Jumlah kolom data (tanpa indikator dan kunci tambahan).
    pub fn width(&self) -> usize {
        self.nulls.len()
    }

    fn is_null(&self, row: &[String], col: usize) -> bool {
        let Some(cell) = row.get(col) else {
            return true;
        };
        let by_marker = || cell_from_executor(cell).is_none();
        match self.nulls.get(col) {
            Some(NullSource::Never) => false,
            Some(NullSource::Flag(flag)) => match row.get(*flag).map(String::as_str) {
                Some("1") => true,
                Some("0") => false,
                _ => by_marker(),
            },
            // Kolom kunci tambahan (di luar kolom data) juga lewat penanda.
            Some(NullSource::Marker) | None => by_marker(),
        }
    }

    /// Nilai kunci baris mentah; `None` bila ada bagian kunci yang NULL.
    fn key_values(&self, row: &[String]) -> Option<Vec<String>> {
        self.key_positions
            .iter()
            .map(|p| (!self.is_null(row, *p)).then(|| row[*p].clone()))
            .collect()
    }

    /// Ubah baris mentah eksekutor menjadi sel ber-nullness eksplisit, hanya
    /// kolom data, selebar [`PageSelect::width`].
    pub fn decode(&self, rows: Vec<Vec<String>>) -> Vec<Vec<Option<String>>> {
        let width = self.width();
        rows.into_iter()
            .map(|mut row| {
                let nulls: Vec<bool> = (0..width).map(|c| self.is_null(&row, c)).collect();
                row.truncate(width);
                let mut out: Vec<Option<String>> = row
                    .into_iter()
                    .zip(&nulls)
                    .map(|(cell, null)| (!null).then_some(cell))
                    .collect();
                out.resize(width, None);
                out
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingMode {
    /// `WHERE kunci > terakhir ORDER BY kunci LIMIT n`.
    Keyset,
    /// `ORDER BY ... LIMIT n OFFSET m`.
    Offset,
}

/// Pembaca tabel per halaman. Dengan primary key yang tipenya aman
/// ([`keyset_safe_type`]) halaman diambil secara keyset. Selain itu jatuh ke
/// `OFFSET`: urut menurut primary key bila ada, atau menurut semua kolom yang
/// bisa diurutkan bila tabel tidak punya kunci. Kasus terakhir tidak punya
/// urutan yang dijamin unik, jadi [`TablePager::warning`] terisi.
#[derive(Clone, Debug)]
pub struct TablePager {
    db: DatabaseType,
    table_sql: String,
    where_clause: Option<String>,
    select: PageSelect,
    mode: PagingMode,
    key_sql: Vec<String>,
    key_kinds: Vec<ValueKind>,
    order_by: Vec<String>,
    last_key: Option<Vec<String>>,
    fetched: u64,
    warning: Option<String>,
}

impl TablePager {
    /// `columns` = kolom yang dikembalikan; `table_columns` = semua kolom
    /// tabel (untuk primary key dan urutan). `table_sql` sudah dikutip.
    pub fn new(
        db: &DatabaseType,
        table_sql: &str,
        columns: &[SourceColumn],
        table_columns: &[SourceColumn],
        where_clause: Option<&str>,
    ) -> Self {
        let key: Vec<SourceColumn> = table_columns
            .iter()
            .filter(|c| c.primary_key)
            .cloned()
            .collect();
        let quoted = |cols: &[SourceColumn]| -> Vec<String> {
            cols.iter().map(|c| quote_ident(db, &c.name)).collect()
        };
        let keyset = !key.is_empty() && key.iter().all(|c| keyset_safe_type(db, &c.data_type));
        let (mode, order_by, warning) = if keyset {
            (PagingMode::Keyset, quoted(&key), None)
        } else if !key.is_empty() {
            (PagingMode::Offset, quoted(&key), None)
        } else {
            let orderable: Vec<SourceColumn> = table_columns
                .iter()
                .filter(|c| orderable_type(db, &c.data_type))
                .cloned()
                .collect();
            let warning = format!(
                "{table_sql} has no primary key: rows are read with OFFSET in column order, so \
                 the result may skip or duplicate rows if the table changes while it is being read"
            );
            (PagingMode::Offset, quoted(&orderable), Some(warning))
        };
        Self {
            db: db.clone(),
            table_sql: table_sql.to_string(),
            where_clause: where_clause
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(str::to_string),
            select: PageSelect::new(db, columns, if keyset { &key } else { &[] }),
            mode,
            key_sql: quoted(&key),
            key_kinds: key.iter().map(|c| kind_from_type(&c.data_type)).collect(),
            order_by,
            last_key: None,
            fetched: 0,
            warning,
        }
    }

    pub fn mode(&self) -> PagingMode {
        self.mode
    }

    /// Peringatan untuk laporan pengguna bila hasil bisa tidak konsisten.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// Query halaman berikutnya, paling banyak `limit` baris.
    pub fn next_sql(&self, limit: u64) -> String {
        match self.mode {
            PagingMode::Keyset => select_keyset_sql(
                &self.db,
                &self.table_sql,
                &self.select.select_list,
                self.where_clause.as_deref(),
                &self.key_sql,
                self.last_key.as_deref(),
                limit,
            ),
            PagingMode::Offset => select_page_sql(
                &self.db,
                &self.table_sql,
                &self.select.select_list,
                self.where_clause.as_deref(),
                &self.order_by,
                limit,
                self.fetched,
            ),
        }
    }

    /// Terima baris mentah hasil [`TablePager::next_sql`]: majukan posisi dan
    /// kembalikan sel ber-nullness eksplisit.
    pub fn accept(&mut self, rows: Vec<Vec<String>>) -> Result<Vec<Vec<Option<String>>>, String> {
        self.fetched += rows.len() as u64;
        if self.mode == PagingMode::Keyset
            && let Some(last) = rows.last()
        {
            match self.select.key_values(last) {
                Some(values) => {
                    let literals: Vec<String> = values
                        .iter()
                        .zip(&self.key_kinds)
                        .map(|(v, kind)| sql_value(&self.db, Some(v), *kind))
                        .collect();
                    // Kunci yang tidak maju berarti halaman yang sama akan
                    // terbaca terus; lebih baik gagal daripada menggandakan.
                    if self.last_key.as_ref() == Some(&literals) {
                        return Err(format!(
                            "keyset pagination made no progress on {}",
                            self.table_sql
                        ));
                    }
                    self.last_key = Some(literals);
                }
                None => {
                    // Kunci NULL (mungkin di SQLite) tidak punya "sesudahnya":
                    // lanjut dengan OFFSET di urutan kunci yang sama.
                    log::warn!(
                        "[TRANSFER] {} has a NULL primary key value; falling back to OFFSET paging",
                        self.table_sql
                    );
                    self.mode = PagingMode::Offset;
                }
            }
        }
        Ok(self.select.decode(rows))
    }

    /// Ambil halaman berikutnya dari `ep`. Halaman kosong = selesai.
    pub async fn next_page(
        &mut self,
        ep: &Endpoint,
        limit: u64,
    ) -> Result<Vec<Vec<Option<String>>>, String> {
        let sql = self.next_sql(limit);
        let rows = ep.query(&sql).await?.rows;
        self.accept(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_sql_per_dialect() {
        let cols = vec!["\"a\"".to_string(), "\"b\"".to_string()];
        assert_eq!(
            select_page_sql(
                &DatabaseType::PostgreSQL,
                "\"t\"",
                &cols,
                Some(" a > 1 "),
                &["\"a\"".to_string()],
                100,
                200
            ),
            "SELECT \"a\", \"b\" FROM \"t\" WHERE a > 1 ORDER BY \"a\" LIMIT 100 OFFSET 200"
        );
        assert_eq!(
            select_page_sql(&DatabaseType::MsSQL, "[t]", &cols, None, &[], 50, 0),
            "SELECT \"a\", \"b\" FROM [t] ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 50 ROWS ONLY"
        );
    }

    fn col(name: &str, ty: &str, nullable: bool, pk: bool) -> SourceColumn {
        SourceColumn {
            name: name.to_string(),
            data_type: ty.to_string(),
            nullable,
            primary_key: pk,
        }
    }

    #[test]
    fn keyset_sql_per_dialect() {
        let list = vec!["a".to_string(), "b".to_string()];
        let one = vec!["a".to_string()];
        let two = vec!["a".to_string(), "b".to_string()];
        let last1 = vec!["5".to_string()];
        let last2 = vec!["5".to_string(), "'x'".to_string()];
        let pg = DatabaseType::PostgreSQL;
        let ms = DatabaseType::MsSQL;

        // Halaman pertama: tanpa predikat kunci.
        assert_eq!(
            select_keyset_sql(&pg, "t", &list, None, &one, None, 10),
            "SELECT a, b FROM t ORDER BY a LIMIT 10"
        );
        assert_eq!(
            select_keyset_sql(&ms, "t", &list, Some("b > 0"), &one, None, 10),
            "SELECT TOP (10) a, b FROM t WHERE b > 0 ORDER BY a"
        );
        // Kunci tunggal, digabung dengan filter pengguna.
        assert_eq!(
            select_keyset_sql(
                &pg,
                "t",
                &list,
                Some("b > 0 OR b IS NULL"),
                &one,
                Some(&last1),
                10
            ),
            "SELECT a, b FROM t WHERE (b > 0 OR b IS NULL) AND (a > 5) ORDER BY a LIMIT 10"
        );
        // Kunci majemuk: row value, kecuali SQL Server.
        for db in [pg.clone(), DatabaseType::MySQL, DatabaseType::SQLite] {
            assert_eq!(
                select_keyset_sql(&db, "t", &list, None, &two, Some(&last2), 3),
                "SELECT a, b FROM t WHERE (a, b) > (5, 'x') ORDER BY a, b LIMIT 3"
            );
        }
        assert_eq!(
            select_keyset_sql(&ms, "t", &list, None, &two, Some(&last2), 3),
            "SELECT TOP (3) a, b FROM t WHERE (a > 5) OR (a = 5 AND b > 'x') ORDER BY a, b"
        );
        let three = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let last3 = vec!["1".to_string(), "2".to_string(), "3".to_string()];
        assert_eq!(
            keyset_predicate(&ms, &three, &last3),
            "(a > 1) OR (a = 1 AND b > 2) OR (a = 1 AND b = 2 AND c > 3)"
        );
    }

    #[test]
    fn ordered_sql_without_a_limit_has_no_sentinel() {
        let list = vec!["a".to_string()];
        let order = vec!["a".to_string()];
        let my = DatabaseType::MySQL;
        let ms = DatabaseType::MsSQL;
        assert_eq!(
            select_ordered_sql(&my, "t", &list, None, &order, None),
            "SELECT a FROM t ORDER BY a"
        );
        assert_eq!(
            select_ordered_sql(&my, "t", &list, Some("a > 1"), &order, Some(7)),
            "SELECT a FROM t WHERE a > 1 ORDER BY a LIMIT 7"
        );
        assert_eq!(
            select_ordered_sql(&ms, "t", &list, None, &order, Some(7)),
            "SELECT TOP (7) a FROM t ORDER BY a"
        );
        assert_eq!(
            select_ordered_sql(&ms, "t", &list, None, &[], None),
            "SELECT a FROM t"
        );
    }

    #[test]
    fn pager_uses_keyset_with_quoting_and_typed_literals() {
        let columns = vec![
            col("id", "bigint", false, true),
            col("code", "varchar(10)", false, true),
            col("note", "text", true, false),
        ];
        let my = DatabaseType::MySQL;
        let mut pager = TablePager::new(&my, "`db`.`t`", &columns, &columns, Some(" note <> '' "));
        assert_eq!(pager.mode(), PagingMode::Keyset);
        assert!(pager.warning().is_none());
        assert_eq!(
            pager.next_sql(2),
            "SELECT `id`, `code`, `note`, CASE WHEN `note` IS NULL THEN 1 ELSE 0 END AS \
             `tabular_null_2` FROM `db`.`t` WHERE note <> '' ORDER BY `id`, `code` LIMIT 2"
        );
        let cells = pager
            .accept(vec![
                vec!["1".into(), "a".into(), "NULL".into(), "0".into()],
                vec!["2".into(), "it's\\".into(), "NULL".into(), "1".into()],
            ])
            .unwrap();
        // Indikator memisahkan string `NULL` dari SQL NULL.
        assert_eq!(cells[0][2].as_deref(), Some("NULL"));
        assert_eq!(cells[1][2], None);
        assert_eq!(cells[1].len(), 3);
        let next = pager.next_sql(2);
        assert!(
            next.ends_with(
                "WHERE (note <> '') AND ((`id`, `code`) > (2, 'it''s\\\\')) \
                 ORDER BY `id`, `code` LIMIT 2"
            ),
            "{next}"
        );
        // Halaman yang sama dua kali: berhenti, jangan menggandakan baris.
        let again = vec![vec!["2".into(), "it's\\".into(), "x".into(), "0".into()]];
        assert!(pager.accept(again).unwrap_err().contains("no progress"));

        let ms = DatabaseType::MsSQL;
        let mut pager = TablePager::new(&ms, "[dbo].[t]", &columns[2..], &columns[..1], None);
        // Kolom kunci di luar daftar data ikut di-SELECT untuk halaman berikutnya.
        assert_eq!(
            pager.next_sql(5),
            "SELECT TOP (5) [note], CASE WHEN [note] IS NULL THEN 1 ELSE 0 END AS \
             [tabular_null_0], [id] FROM [dbo].[t] ORDER BY [id]"
        );
        let cells = pager
            .accept(vec![vec!["n".into(), "0".into(), "41".into()]])
            .unwrap();
        assert_eq!(cells, vec![vec![Some("n".to_string())]]);
        assert!(pager.next_sql(5).contains("WHERE [id] > 41 ORDER BY [id]"));
    }

    #[test]
    fn pager_falls_back_to_offset_and_warns_without_a_key() {
        let columns = vec![
            col("a", "integer", true, false),
            col("doc", "json", true, false),
            col("b", "text", true, false),
        ];
        let pg = DatabaseType::PostgreSQL;
        let mut pager = TablePager::new(&pg, "\"t\"", &columns, &columns, None);
        assert_eq!(pager.mode(), PagingMode::Offset);
        assert!(pager.warning().unwrap().contains("no primary key"));
        // `json` tidak punya urutan di PostgreSQL, jadi tidak ikut ORDER BY.
        let sql = pager.next_sql(10);
        assert!(
            sql.ends_with("FROM \"t\" ORDER BY \"a\", \"b\" LIMIT 10 OFFSET 0"),
            "{sql}"
        );
        let row: Vec<String> = ["1", "{}", "x", "0", "0"].map(String::from).to_vec();
        pager.accept(vec![row]).unwrap();
        assert!(pager.next_sql(10).ends_with("LIMIT 10 OFFSET 1"));

        // SQL Server: `text` tidak bisa diurutkan; MySQL mengurutkan semuanya.
        let ms = TablePager::new(&DatabaseType::MsSQL, "[t]", &columns, &columns, None);
        assert!(
            ms.next_sql(10)
                .ends_with("ORDER BY [a] OFFSET 0 ROWS FETCH NEXT 10 ROWS ONLY")
        );
        let my = TablePager::new(&DatabaseType::MySQL, "`t`", &columns, &columns, None);
        assert!(
            my.next_sql(10)
                .contains("ORDER BY `a`, `doc`, `b` LIMIT 10")
        );

        // Primary key dengan tipe yang tidak aman untuk keyset: OFFSET urut
        // kunci, tanpa peringatan karena urutannya unik.
        let ts = vec![
            col("at", "timestamp", false, true),
            col("v", "text", true, false),
        ];
        let pager = TablePager::new(&pg, "\"t\"", &ts, &ts, None);
        assert_eq!(pager.mode(), PagingMode::Offset);
        assert!(pager.warning().is_none());
        assert!(
            pager
                .next_sql(4)
                .contains("ORDER BY \"at\" LIMIT 4 OFFSET 0")
        );
        assert!(!keyset_safe_type(&DatabaseType::MsSQL, "varchar(20)"));
        assert!(keyset_safe_type(&DatabaseType::MsSQL, "nvarchar(20)"));
        assert!(keyset_safe_type(&DatabaseType::MySQL, "int(11) unsigned"));
        assert!(!keyset_safe_type(&DatabaseType::MySQL, "tinyint(1)"));
        assert!(!keyset_safe_type(&pg, "double precision"));
    }

    #[test]
    fn pager_switches_to_offset_when_a_key_value_is_null() {
        let columns = vec![
            col("k", "TEXT", true, true),
            col("v", "INTEGER", true, false),
        ];
        let db = DatabaseType::SQLite;
        let mut pager = TablePager::new(&db, "\"t\"", &columns, &columns, None);
        assert_eq!(pager.mode(), PagingMode::Keyset);
        // Kolom kunci nullable punya indikator: NULL sungguhan, bukan string.
        pager
            .accept(vec![vec!["NULL".into(), "1".into(), "1".into()]])
            .unwrap();
        assert_eq!(pager.mode(), PagingMode::Offset);
        assert!(
            pager
                .next_sql(2)
                .ends_with("ORDER BY \"k\" LIMIT 2 OFFSET 1")
        );

        let mut pager = TablePager::new(&db, "\"t\"", &columns, &columns, None);
        pager
            .accept(vec![vec!["NULL".into(), "1".into(), "0".into()]])
            .unwrap();
        assert_eq!(pager.mode(), PagingMode::Keyset);
        assert!(pager.next_sql(2).contains("WHERE \"k\" > 'NULL' ORDER BY"));
    }

    #[tokio::test]
    async fn execute_atomic_undoes_everything_on_failure() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let ep = Endpoint::new(
            ConnectionConfig {
                connection_type: DatabaseType::SQLite,
                ..Default::default()
            },
            Some(DatabasePool::SQLite(std::sync::Arc::new(pool))),
            None,
        );
        ep.query("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .await
            .unwrap();
        let bad = vec![
            "INSERT INTO t VALUES (1, 'a')".to_string(),
            "INSERT INTO t VALUES (2, 'b')".to_string(),
            "INSERT INTO t VALUES (3, NULL)".to_string(),
            "INSERT INTO t VALUES (4, 'never runs')".to_string(),
        ];
        assert!(ep.execute_atomic(&bad).await.is_err());
        // Gagal di tengah: tidak ada baris yang tertinggal, dan koneksi tunggal
        // pool kembali tanpa transaksi terbuka.
        let count = ep.query("SELECT COUNT(*) FROM t").await.unwrap();
        assert_eq!(count.first_value(), Some("0"));

        let good = bad[..2].iter().cloned();
        assert_eq!(ep.execute_atomic(good).await.unwrap(), 2);
        let count = ep.query("SELECT COUNT(*) FROM t").await.unwrap();
        assert_eq!(count.first_value(), Some("2"));
    }

    #[test]
    fn parses_sqlite_pragma_and_information_schema_rows() {
        let pragma = ResultSet {
            headers: vec![],
            rows: vec![
                vec![
                    "0".into(),
                    "id".into(),
                    "INTEGER".into(),
                    "1".into(),
                    "NULL".into(),
                    "1".into(),
                ],
                vec![
                    "1".into(),
                    "body".into(),
                    "".into(),
                    "0".into(),
                    "NULL".into(),
                    "0".into(),
                ],
            ],
        };
        let cols = parse_columns(&DatabaseType::SQLite, &pragma);
        assert_eq!(cols.len(), 2);
        assert!(cols[0].primary_key && !cols[0].nullable);
        assert!(!cols[1].primary_key && cols[1].nullable);

        let info = ResultSet {
            headers: vec![],
            rows: vec![vec![
                "id".into(),
                "int(11)".into(),
                "NO".into(),
                "PRI".into(),
            ]],
        };
        let cols = parse_columns(&DatabaseType::MySQL, &info);
        assert_eq!(cols[0].data_type, "int(11)");
        assert!(cols[0].primary_key && !cols[0].nullable);
    }

    #[test]
    fn sqlite_blob_columns_are_selected_as_hex() {
        let blob = SourceColumn {
            name: "data".into(),
            data_type: "BLOB".into(),
            nullable: true,
            primary_key: false,
        };
        assert!(select_expr(&DatabaseType::SQLite, &blob).contains("hex(\"data\")"));
        assert_eq!(select_expr(&DatabaseType::PostgreSQL, &blob), "\"data\"");
    }

    #[test]
    fn mysql_columns_sql_uses_database_or_current() {
        let sql = columns_sql(&DatabaseType::MySQL, Some("shop"), "orders").unwrap();
        assert!(sql.contains("TABLE_SCHEMA = 'shop' AND TABLE_NAME = 'orders'"));
        let sql = columns_sql(&DatabaseType::MySQL, None, "orders").unwrap();
        assert!(sql.contains("TABLE_SCHEMA = DATABASE()"));
    }
}
