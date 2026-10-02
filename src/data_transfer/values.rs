//! Literal SQL dari sel teks, penyusun batch `INSERT`, dan inferensi tipe
//! kolom untuk data dari file. Fungsi murni tanpa I/O.

use super::{TableData, is_null_cell};
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::sql_literal;

/// Cara sebuah nilai ditulis sebagai literal SQL.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ValueKind {
    #[default]
    Text,
    Number,
    Bool,
    Binary,
}

/// Tebak [`ValueKind`] dari nama tipe kolom engine mana pun.
pub fn kind_from_type(type_name: &str) -> ValueKind {
    let t = type_name.trim().to_ascii_lowercase();
    let base = t.split(['(', ' ']).next().unwrap_or("");
    if t == "tinyint(1)" || matches!(base, "bool" | "boolean" | "bit") {
        return ValueKind::Bool;
    }
    if matches!(
        base,
        "bytea"
            | "blob"
            | "tinyblob"
            | "mediumblob"
            | "longblob"
            | "binary"
            | "varbinary"
            | "image"
    ) {
        return ValueKind::Binary;
    }
    if matches!(
        base,
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
            | "float"
            | "float4"
            | "float8"
            | "double"
            | "real"
            | "money"
            | "smallmoney"
    ) {
        return ValueKind::Number;
    }
    ValueKind::Text
}

fn is_plain_number(v: &str) -> bool {
    !v.is_empty()
        && v.bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
        && v.parse::<f64>().is_ok_and(f64::is_finite)
}

/// Heksadesimal dari tampilan biner driver (`0x..` MySQL/MsSQL, `\x..` PostgreSQL).
fn binary_hex(v: &str) -> Option<&str> {
    let hex = v.strip_prefix("0x").or_else(|| v.strip_prefix("\\x"))?;
    (hex.len() % 2 == 0 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// Literal SQL untuk satu sel sesuai dialek `db`.
pub fn sql_value(db: &DatabaseType, value: &str, kind: ValueKind) -> String {
    if is_null_cell(value) {
        return "NULL".to_string();
    }
    match kind {
        ValueKind::Number => {
            if value.is_empty() {
                "NULL".to_string()
            } else if is_plain_number(value) {
                value.to_string()
            } else {
                sql_literal(db, value)
            }
        }
        ValueKind::Bool => {
            let truthy = match value.to_ascii_lowercase().as_str() {
                "" => return "NULL".to_string(),
                "true" | "t" | "1" | "yes" | "y" => true,
                "false" | "f" | "0" | "no" | "n" => false,
                _ => return sql_literal(db, value),
            };
            match (db, truthy) {
                (DatabaseType::PostgreSQL, true) => "TRUE".to_string(),
                (DatabaseType::PostgreSQL, false) => "FALSE".to_string(),
                (_, true) => "1".to_string(),
                (_, false) => "0".to_string(),
            }
        }
        ValueKind::Binary => match binary_hex(value) {
            Some("") => "''".to_string(),
            Some(hex) => match db {
                DatabaseType::PostgreSQL => format!("'\\x{hex}'"),
                DatabaseType::SQLite => format!("X'{hex}'"),
                _ => format!("0x{hex}"),
            },
            None => sql_literal(db, value),
        },
        ValueKind::Text => sql_literal(db, value),
    }
}

/// Batas ukuran satu statement `INSERT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InsertLimits {
    /// Baris maksimum per statement (0 = tanpa batas baris).
    pub max_rows: usize,
    /// Ukuran maksimum statement dalam byte (0 = tanpa batas ukuran).
    pub max_bytes: usize,
}

impl Default for InsertLimits {
    fn default() -> Self {
        Self {
            max_rows: 500,
            max_bytes: 1024 * 1024,
        }
    }
}

impl InsertLimits {
    /// SQL Server menolak lebih dari 1000 baris dalam satu `VALUES`.
    pub fn for_engine(self, db: &DatabaseType) -> Self {
        if matches!(db, DatabaseType::MsSQL) && (self.max_rows == 0 || self.max_rows > 1000) {
            Self {
                max_rows: 1000,
                ..self
            }
        } else {
            self
        }
    }
}

/// Susun statement `INSERT` multi-baris. `table_sql` dan `columns_sql` sudah
/// dikutip pemanggil. `source_cols[i]` adalah indeks sel sumber untuk kolom
/// ke-`i`; `kinds[i]` jenis literalnya (default teks).
pub fn build_insert_batches(
    db: &DatabaseType,
    table_sql: &str,
    columns_sql: &[String],
    rows: &[Vec<String>],
    source_cols: &[usize],
    kinds: &[ValueKind],
    limits: InsertLimits,
) -> Vec<String> {
    if rows.is_empty() || source_cols.is_empty() {
        return Vec::new();
    }
    let limits = limits.for_engine(db);
    let head = format!(
        "INSERT INTO {} ({}) VALUES\n",
        table_sql,
        columns_sql.join(", ")
    );
    let mut batches = Vec::new();
    let mut current = String::new();
    let mut current_rows = 0usize;
    for row in rows {
        let values: Vec<String> = source_cols
            .iter()
            .enumerate()
            .map(|(i, src)| {
                let cell = row
                    .get(*src)
                    .map(String::as_str)
                    .unwrap_or(super::NULL_MARKER);
                sql_value(db, cell, kinds.get(i).copied().unwrap_or_default())
            })
            .collect();
        let tuple = format!("({})", values.join(", "));
        let over_rows = limits.max_rows > 0 && current_rows >= limits.max_rows;
        // +3 untuk ",\n" pemisah dan ";" penutup.
        let over_bytes = limits.max_bytes > 0
            && current_rows > 0
            && current.len() + tuple.len() + 3 > limits.max_bytes;
        if over_rows || over_bytes {
            current.push(';');
            batches.push(std::mem::take(&mut current));
            current_rows = 0;
        }
        if current_rows == 0 {
            current.push_str(&head);
        } else {
            current.push_str(",\n");
        }
        current.push_str(&tuple);
        current_rows += 1;
    }
    if current_rows > 0 {
        current.push(';');
        batches.push(current);
    }
    batches
}

/// Tipe kolom hasil inferensi dari data teks (file CSV/JSON/XLSX).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferredType {
    Integer,
    Real,
    Boolean,
    Date,
    Timestamp,
    Text,
}

impl InferredType {
    pub fn kind(self) -> ValueKind {
        match self {
            InferredType::Integer | InferredType::Real => ValueKind::Number,
            InferredType::Boolean => ValueKind::Bool,
            _ => ValueKind::Text,
        }
    }

    /// Tipe kolom untuk `CREATE TABLE` di engine tujuan.
    pub fn sql_type(self, db: &DatabaseType) -> &'static str {
        match (self, db) {
            (InferredType::Integer, DatabaseType::SQLite) => "INTEGER",
            (InferredType::Integer, _) => "BIGINT",
            (InferredType::Real, DatabaseType::SQLite) => "REAL",
            (InferredType::Real, DatabaseType::MySQL) => "DOUBLE",
            (InferredType::Real, DatabaseType::MsSQL) => "FLOAT",
            (InferredType::Real, _) => "DOUBLE PRECISION",
            (InferredType::Boolean, DatabaseType::SQLite) => "INTEGER",
            (InferredType::Boolean, DatabaseType::MySQL) => "TINYINT(1)",
            (InferredType::Boolean, DatabaseType::MsSQL) => "BIT",
            (InferredType::Boolean, _) => "BOOLEAN",
            (InferredType::Date, DatabaseType::SQLite) => "TEXT",
            (InferredType::Date, _) => "DATE",
            (InferredType::Timestamp, DatabaseType::SQLite) => "TEXT",
            (InferredType::Timestamp, DatabaseType::MySQL) => "DATETIME",
            (InferredType::Timestamp, DatabaseType::MsSQL) => "DATETIME2",
            (InferredType::Timestamp, _) => "TIMESTAMP",
            (InferredType::Text, DatabaseType::MsSQL) => "NVARCHAR(MAX)",
            (InferredType::Text, _) => "TEXT",
        }
    }
}

fn digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

fn looks_like_date(v: &str) -> bool {
    let b = v.as_bytes();
    v.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && digits(&v[..4], 4)
        && digits(&v[5..7], 2)
        && digits(&v[8..], 2)
}

fn looks_like_timestamp(v: &str) -> bool {
    if v.len() < 16 || !v.is_char_boundary(10) || !v.is_char_boundary(11) {
        return false;
    }
    let sep = v.as_bytes()[10];
    let time = &v[11..];
    looks_like_date(&v[..10])
        && (sep == b' ' || sep == b'T')
        && time.len() >= 5
        && time.is_char_boundary(5)
        && digits(&time[..2], 2)
        && time.as_bytes()[2] == b':'
        && digits(&time[3..5], 2)
}

/// Inferensi tipe per kolom. Sel kosong dan NULL diabaikan; kolom tanpa nilai
/// atau dengan nilai campuran menjadi `Text`. Angka berawalan nol (`007`,
/// kode pos, nomor telepon) dipertahankan sebagai teks.
pub fn infer_types(data: &TableData) -> Vec<InferredType> {
    (0..data.headers.len())
        .map(|col| {
            let mut seen = false;
            let (mut int, mut real, mut boolean, mut date, mut ts) = (true, true, true, true, true);
            for row in &data.rows {
                let v = row.get(col).map(|s| s.trim()).unwrap_or("");
                if v.is_empty() || is_null_cell(v) {
                    continue;
                }
                seen = true;
                let unsigned = v.trim_start_matches(['-', '+']);
                let padded =
                    unsigned.len() > 1 && unsigned.starts_with('0') && !unsigned.starts_with("0.");
                int &= !padded && v.parse::<i64>().is_ok();
                real &= !padded && is_plain_number(v);
                boolean &= matches!(v.to_ascii_lowercase().as_str(), "true" | "false");
                date &= looks_like_date(v);
                ts &= looks_like_timestamp(v);
                if !(int || real || boolean || date || ts) {
                    break;
                }
            }
            match (seen, int, real, boolean, date, ts) {
                (false, ..) => InferredType::Text,
                (_, true, ..) => InferredType::Integer,
                (_, _, true, ..) => InferredType::Real,
                (_, _, _, true, ..) => InferredType::Boolean,
                (_, _, _, _, true, _) => InferredType::Date,
                (_, _, _, _, _, true) => InferredType::Timestamp,
                _ => InferredType::Text,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(data: &[&[&str]]) -> Vec<Vec<String>> {
        data.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    #[test]
    fn literals_follow_kind_and_dialect() {
        let pg = DatabaseType::PostgreSQL;
        let my = DatabaseType::MySQL;
        assert_eq!(sql_value(&pg, "NULL", ValueKind::Text), "NULL");
        assert_eq!(sql_value(&pg, "42", ValueKind::Number), "42");
        assert_eq!(sql_value(&pg, "", ValueKind::Number), "NULL");
        assert_eq!(sql_value(&pg, "1; DROP", ValueKind::Number), "'1; DROP'");
        assert_eq!(sql_value(&pg, "t", ValueKind::Bool), "TRUE");
        assert_eq!(sql_value(&my, "false", ValueKind::Bool), "0");
        assert_eq!(sql_value(&my, "a\\b'c", ValueKind::Text), "'a\\\\b''c'");
        assert_eq!(
            sql_value(&DatabaseType::MsSQL, "é", ValueKind::Text),
            "N'é'"
        );
    }

    #[test]
    fn binary_literals_per_engine() {
        assert_eq!(
            sql_value(&DatabaseType::PostgreSQL, "0xDEADBEEF", ValueKind::Binary),
            "'\\xDEADBEEF'"
        );
        assert_eq!(
            sql_value(&DatabaseType::MySQL, "\\xdead", ValueKind::Binary),
            "0xdead"
        );
        assert_eq!(
            sql_value(&DatabaseType::SQLite, "0x00ff", ValueKind::Binary),
            "X'00ff'"
        );
        // Bukan heksadesimal: diperlakukan sebagai teks.
        assert_eq!(
            sql_value(&DatabaseType::MySQL, "hello", ValueKind::Binary),
            "'hello'"
        );
    }

    #[test]
    fn insert_batches_respect_row_and_byte_limits() {
        let data = rows(&[&["1", "a"], &["2", "b"], &["3", "c"]]);
        let cols = vec!["\"id\"".to_string(), "\"name\"".to_string()];
        let kinds = [ValueKind::Number, ValueKind::Text];
        let by_rows = build_insert_batches(
            &DatabaseType::PostgreSQL,
            "\"t\"",
            &cols,
            &data,
            &[0, 1],
            &kinds,
            InsertLimits {
                max_rows: 2,
                max_bytes: 0,
            },
        );
        assert_eq!(by_rows.len(), 2);
        assert_eq!(
            by_rows[0],
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES\n(1, 'a'),\n(2, 'b');"
        );
        assert_eq!(
            by_rows[1],
            "INSERT INTO \"t\" (\"id\", \"name\") VALUES\n(3, 'c');"
        );

        // Batas byte lebih kecil dari satu baris: tetap satu baris per statement.
        let by_bytes = build_insert_batches(
            &DatabaseType::PostgreSQL,
            "\"t\"",
            &cols,
            &data,
            &[0, 1],
            &kinds,
            InsertLimits {
                max_rows: 0,
                max_bytes: 10,
            },
        );
        assert_eq!(by_bytes.len(), 3);
        assert!(by_bytes.iter().all(|s| s.ends_with(';')));
    }

    #[test]
    fn mssql_caps_rows_per_statement() {
        let limits = InsertLimits {
            max_rows: 5000,
            max_bytes: 0,
        }
        .for_engine(&DatabaseType::MsSQL);
        assert_eq!(limits.max_rows, 1000);
    }

    #[test]
    fn infers_column_types() {
        let data = TableData::new(
            ["i", "r", "b", "d", "ts", "zip", "mixed", "empty"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            rows(&[
                &[
                    "1",
                    "1.5",
                    "true",
                    "2026-01-02",
                    "2026-01-02 03:04:05",
                    "00123",
                    "1",
                    "",
                ],
                &[
                    "-2",
                    "2",
                    "FALSE",
                    "2026-12-31",
                    "2026-01-02T03:04",
                    "04567",
                    "x",
                    "NULL",
                ],
                &["", "NULL", "true", "", "", "", "", ""],
            ]),
        );
        assert_eq!(
            infer_types(&data),
            vec![
                InferredType::Integer,
                InferredType::Real,
                InferredType::Boolean,
                InferredType::Date,
                InferredType::Timestamp,
                InferredType::Text,
                InferredType::Text,
                InferredType::Text,
            ]
        );
    }

    #[test]
    fn kinds_from_engine_type_names() {
        assert_eq!(kind_from_type("tinyint(1)"), ValueKind::Bool);
        assert_eq!(kind_from_type("INT(11) unsigned"), ValueKind::Number);
        assert_eq!(kind_from_type("numeric(10,2)"), ValueKind::Number);
        assert_eq!(kind_from_type("bytea"), ValueKind::Binary);
        assert_eq!(kind_from_type("varbinary(16)"), ValueKind::Binary);
        assert_eq!(kind_from_type("character varying(20)"), ValueKind::Text);
        assert_eq!(kind_from_type("timestamp with time zone"), ValueKind::Text);
    }
}
