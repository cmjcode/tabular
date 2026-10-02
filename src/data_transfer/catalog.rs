//! Titik akhir (koneksi + database) untuk transfer/compare/ekspor objek, dan
//! query katalog yang dibutuhkan: daftar tabel, kolom, primary key, halaman
//! data. Headless.

use super::types::SourceColumn;
use super::values::{ValueKind, kind_from_type};
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

    /// Nama tabel terkutip untuk endpoint ini.
    pub fn table_sql(&self, table: &str) -> String {
        quote_qualified(self.db_type(), table)
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

/// `SELECT` satu halaman. `order_by` berisi ekspresi yang sudah dikutip; bila
/// kosong urutannya tidak dijamin stabil antar halaman.
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
    if let Some(w) = where_clause.map(str::trim).filter(|w| !w.is_empty()) {
        sql.push_str(&format!(" WHERE {w}"));
    }
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

/// Baris `SELECT` sering datang tanpa header bila hasilnya kosong; samakan
/// lebar baris dengan jumlah kolom yang diminta.
pub fn pad_rows(rows: &mut [Vec<String>], width: usize) {
    for row in rows {
        row.resize(width, super::NULL_MARKER.to_string());
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
