//! Aproksimasi tipe kolom lintas engine (H9). Tipe sumber diurai menjadi
//! tipe logis, lalu ditulis ulang dalam dialek tujuan. Fungsi murni.

use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::quote_ident;

/// Tipe logis netral-engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Logical {
    Bool,
    SmallInt,
    Int,
    BigInt,
    /// `(precision, scale)` bila diketahui.
    Decimal(Option<(u32, u32)>),
    Float,
    Double,
    Char(Option<u32>),
    Varchar(Option<u32>),
    Text,
    Date,
    Time,
    Timestamp,
    TimestampTz,
    Binary,
    Json,
    Uuid,
    /// Tipe yang tidak dikenali (array, geometry, tipe buatan user, …).
    Unknown,
}

fn args_of(ty: &str) -> Vec<u32> {
    ty.split_once('(')
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(inner, _)| {
            inner
                .split(',')
                .filter_map(|p| p.trim().parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Urai nama tipe engine `src` menjadi tipe logis.
pub fn parse_type(src: &DatabaseType, ty: &str) -> Logical {
    let lower = ty.trim().to_ascii_lowercase();
    let unsigned = lower.contains("unsigned");
    let args = args_of(&lower);
    let base = lower
        .split('(')
        .next()
        .unwrap_or("")
        .replace(" unsigned", "")
        .replace(" zerofill", "");
    let base = base.trim();
    let len = args.first().copied();

    if lower.ends_with("[]") {
        return Logical::Unknown;
    }
    // `timestamp(6) with time zone`: presisi memisahkan nama dari akhirannya.
    if lower.starts_with("timestamp") && lower.contains("with time zone") {
        return Logical::TimestampTz;
    }
    if matches!(src, DatabaseType::SQLite) {
        // Aturan afinitas SQLite: nama tipe bebas, dicocokkan per substring.
        return if base.is_empty() {
            Logical::Text
        } else if base.contains("bool") {
            Logical::Bool
        } else if base.contains("int") {
            Logical::BigInt
        } else if base.contains("char") || base.contains("clob") || base.contains("text") {
            Logical::Text
        } else if base.contains("blob") {
            Logical::Binary
        } else if base.contains("real") || base.contains("floa") || base.contains("doub") {
            Logical::Double
        } else if base.contains("json") {
            Logical::Json
        } else if base == "date" {
            Logical::Date
        } else if base.contains("time") {
            Logical::Timestamp
        } else {
            Logical::Decimal(match args.as_slice() {
                [p, s] => Some((*p, *s)),
                _ => None,
            })
        };
    }

    match base {
        "bool" | "boolean" => Logical::Bool,
        "bit" if len.unwrap_or(1) == 1 => Logical::Bool,
        "bit" | "bit varying" | "varbit" => Logical::Binary,
        "tinyint" if len == Some(1) => Logical::Bool,
        "tinyint" | "smallint" | "int2" | "smallserial" | "year" => {
            if unsigned && base == "smallint" {
                Logical::Int
            } else {
                Logical::SmallInt
            }
        }
        "mediumint" | "int" | "integer" | "int4" | "serial" => {
            if unsigned {
                Logical::BigInt
            } else {
                Logical::Int
            }
        }
        "bigint" | "int8" | "bigserial" => Logical::BigInt,
        "decimal" | "numeric" | "dec" | "fixed" => Logical::Decimal(match args.as_slice() {
            [p, s] => Some((*p, *s)),
            [p] => Some((*p, 0)),
            _ => None,
        }),
        "money" | "smallmoney" => Logical::Decimal(Some((19, 4))),
        // `float` di SQL Server presisi ganda; di MySQL presisi tunggal.
        "float" if matches!(src, DatabaseType::MsSQL) => Logical::Double,
        "float" | "float4" | "real" => Logical::Float,
        "double" | "double precision" | "float8" => Logical::Double,
        "char" | "character" | "nchar" | "bpchar" => Logical::Char(len),
        "varchar" | "character varying" | "nvarchar" | "varchar2" => {
            if lower.contains("(max)") {
                Logical::Text
            } else {
                Logical::Varchar(len)
            }
        }
        "text" | "tinytext" | "mediumtext" | "longtext" | "ntext" | "clob" | "citext" | "xml"
        | "enum" | "set" | "name" | "inet" | "cidr" | "macaddr" | "interval" => Logical::Text,
        "date" => Logical::Date,
        "time" | "time without time zone" | "time with time zone" | "timetz" => Logical::Time,
        // `timestamp` di SQL Server adalah rowversion, bukan waktu.
        "timestamp" | "rowversion" if matches!(src, DatabaseType::MsSQL) => Logical::Binary,
        "datetime"
        | "datetime2"
        | "smalldatetime"
        | "timestamp"
        | "timestamp without time zone" => Logical::Timestamp,
        "timestamptz" | "timestamp with time zone" | "datetimeoffset" => Logical::TimestampTz,
        "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary" | "varbinary" | "bytea"
        | "image" => Logical::Binary,
        "json" | "jsonb" => Logical::Json,
        "uuid" | "uniqueidentifier" => Logical::Uuid,
        _ => Logical::Unknown,
    }
}

/// Tulis tipe logis dalam dialek `dst`. `key` = kolom ikut primary key, yang
/// di MySQL/SQL Server tidak boleh bertipe teks tanpa panjang.
pub fn render_type(dst: &DatabaseType, logical: &Logical, key: bool) -> String {
    match dst {
        DatabaseType::MySQL => match logical {
            Logical::Bool => "TINYINT(1)".into(),
            Logical::SmallInt => "SMALLINT".into(),
            Logical::Int => "INT".into(),
            Logical::BigInt => "BIGINT".into(),
            Logical::Decimal(Some((p, s))) => {
                let p = (*p).clamp(1, 65);
                format!("DECIMAL({},{})", p, (*s).min(30).min(p))
            }
            Logical::Decimal(None) => "DECIMAL(38,10)".into(),
            Logical::Float => "FLOAT".into(),
            Logical::Double => "DOUBLE".into(),
            Logical::Char(Some(n)) if *n <= 255 => format!("CHAR({n})"),
            Logical::Varchar(Some(n)) if *n <= 16_383 => format!("VARCHAR({n})"),
            Logical::Varchar(None) | Logical::Char(None) => "VARCHAR(255)".into(),
            Logical::Uuid => "CHAR(36)".into(),
            Logical::Date => "DATE".into(),
            Logical::Time => "TIME".into(),
            Logical::Timestamp | Logical::TimestampTz => "DATETIME(6)".into(),
            Logical::Binary if key => "VARBINARY(255)".into(),
            Logical::Binary => "LONGBLOB".into(),
            Logical::Json => "JSON".into(),
            _ if key => "VARCHAR(255)".into(),
            _ => "LONGTEXT".into(),
        },
        DatabaseType::MsSQL => match logical {
            Logical::Bool => "BIT".into(),
            Logical::SmallInt => "SMALLINT".into(),
            Logical::Int => "INT".into(),
            Logical::BigInt => "BIGINT".into(),
            Logical::Decimal(Some((p, s))) => {
                let p = (*p).clamp(1, 38);
                format!("DECIMAL({},{})", p, (*s).min(p))
            }
            Logical::Decimal(None) => "DECIMAL(38,10)".into(),
            Logical::Float => "REAL".into(),
            Logical::Double => "FLOAT".into(),
            Logical::Char(Some(n)) if *n <= 4000 => format!("NCHAR({n})"),
            Logical::Varchar(Some(n)) if *n <= 4000 => format!("NVARCHAR({n})"),
            Logical::Uuid => "UNIQUEIDENTIFIER".into(),
            Logical::Date => "DATE".into(),
            Logical::Time => "TIME".into(),
            Logical::Timestamp => "DATETIME2".into(),
            Logical::TimestampTz => "DATETIMEOFFSET".into(),
            Logical::Binary if key => "VARBINARY(450)".into(),
            Logical::Binary => "VARBINARY(MAX)".into(),
            _ if key => "NVARCHAR(450)".into(),
            _ => "NVARCHAR(MAX)".into(),
        },
        DatabaseType::SQLite => match logical {
            Logical::Bool | Logical::SmallInt | Logical::Int | Logical::BigInt => "INTEGER".into(),
            Logical::Decimal(_) => "NUMERIC".into(),
            Logical::Float | Logical::Double => "REAL".into(),
            Logical::Binary => "BLOB".into(),
            _ => "TEXT".into(),
        },
        // PostgreSQL dan engine lain yang berdialek serupa.
        _ => match logical {
            Logical::Bool => "BOOLEAN".into(),
            Logical::SmallInt => "SMALLINT".into(),
            Logical::Int => "INTEGER".into(),
            Logical::BigInt => "BIGINT".into(),
            Logical::Decimal(Some((p, s))) => format!("NUMERIC({},{})", (*p).max(1), s),
            Logical::Decimal(None) => "NUMERIC".into(),
            Logical::Float => "REAL".into(),
            Logical::Double => "DOUBLE PRECISION".into(),
            Logical::Char(Some(n)) => format!("CHAR({n})"),
            Logical::Varchar(Some(n)) => format!("VARCHAR({n})"),
            Logical::Varchar(None) | Logical::Char(None) => "VARCHAR".into(),
            Logical::Date => "DATE".into(),
            Logical::Time => "TIME".into(),
            Logical::Timestamp => "TIMESTAMP".into(),
            Logical::TimestampTz => "TIMESTAMPTZ".into(),
            Logical::Binary => "BYTEA".into(),
            Logical::Json => "JSONB".into(),
            Logical::Uuid => "UUID".into(),
            Logical::Text | Logical::Unknown => "TEXT".into(),
        },
    }
}

/// Tipe kolom di engine tujuan. Engine yang sama: tipe asli dipertahankan.
pub fn map_type(src: &DatabaseType, dst: &DatabaseType, ty: &str, key: bool) -> String {
    if src == dst && !ty.trim().is_empty() {
        return ty.trim().to_string();
    }
    render_type(dst, &parse_type(src, ty), key)
}

/// Kolom sumber yang dibutuhkan untuk membuat tabel tujuan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

/// `CREATE TABLE` di dialek `dst` untuk kolom dari engine `src`. `table_sql`
/// sudah dikutip. Tidak gagal bila tabel sudah ada. Auto-increment/identity
/// tidak dibawa: nilai kunci disalin apa adanya dari sumber.
pub fn create_table_sql(
    src: &DatabaseType,
    dst: &DatabaseType,
    table_sql: &str,
    columns: &[SourceColumn],
) -> String {
    let mut defs: Vec<String> = columns
        .iter()
        .map(|c| {
            format!(
                "  {} {}{}",
                quote_ident(dst, &c.name),
                map_type(src, dst, &c.data_type, c.primary_key),
                if c.nullable && !c.primary_key {
                    ""
                } else {
                    " NOT NULL"
                }
            )
        })
        .collect();
    let keys: Vec<String> = columns
        .iter()
        .filter(|c| c.primary_key)
        .map(|c| quote_ident(dst, &c.name))
        .collect();
    if !keys.is_empty() {
        defs.push(format!("  PRIMARY KEY ({})", keys.join(", ")));
    }
    let body = defs.join(",\n");
    match dst {
        DatabaseType::MsSQL => format!(
            "IF OBJECT_ID(N'{}', N'U') IS NULL\nCREATE TABLE {} (\n{}\n);",
            table_sql.replace('\'', "''"),
            table_sql,
            body
        ),
        _ => format!("CREATE TABLE IF NOT EXISTS {} (\n{}\n);", table_sql, body),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MY: DatabaseType = DatabaseType::MySQL;
    const PG: DatabaseType = DatabaseType::PostgreSQL;
    const MS: DatabaseType = DatabaseType::MsSQL;
    const LITE: DatabaseType = DatabaseType::SQLite;

    #[test]
    fn mysql_to_postgres() {
        assert_eq!(map_type(&MY, &PG, "tinyint(1)", false), "BOOLEAN");
        assert_eq!(map_type(&MY, &PG, "int(10) unsigned", false), "BIGINT");
        assert_eq!(map_type(&MY, &PG, "varchar(120)", false), "VARCHAR(120)");
        assert_eq!(map_type(&MY, &PG, "decimal(12,2)", false), "NUMERIC(12,2)");
        assert_eq!(map_type(&MY, &PG, "datetime", false), "TIMESTAMP");
        assert_eq!(map_type(&MY, &PG, "longblob", false), "BYTEA");
        assert_eq!(map_type(&MY, &PG, "enum('a','b')", false), "TEXT");
        assert_eq!(map_type(&MY, &PG, "json", false), "JSONB");
    }

    #[test]
    fn postgres_to_mysql_and_mssql() {
        assert_eq!(
            map_type(&PG, &MY, "character varying(50)", false),
            "VARCHAR(50)"
        );
        assert_eq!(map_type(&PG, &MY, "text", false), "LONGTEXT");
        assert_eq!(map_type(&PG, &MY, "text", true), "VARCHAR(255)");
        assert_eq!(
            map_type(&PG, &MY, "timestamp with time zone", false),
            "DATETIME(6)"
        );
        assert_eq!(map_type(&PG, &MY, "uuid", true), "CHAR(36)");
        assert_eq!(map_type(&PG, &MY, "integer[]", false), "LONGTEXT");
        assert_eq!(map_type(&PG, &MS, "boolean", false), "BIT");
        assert_eq!(map_type(&PG, &MS, "jsonb", false), "NVARCHAR(MAX)");
        assert_eq!(map_type(&PG, &MS, "bytea", false), "VARBINARY(MAX)");
        assert_eq!(
            map_type(&PG, &MS, "timestamp without time zone", false),
            "DATETIME2"
        );
    }

    #[test]
    fn mssql_quirks() {
        assert_eq!(map_type(&MS, &PG, "float", false), "DOUBLE PRECISION");
        assert_eq!(map_type(&MS, &PG, "timestamp", false), "BYTEA");
        assert_eq!(map_type(&MS, &PG, "nvarchar(max)", false), "TEXT");
        assert_eq!(map_type(&MS, &PG, "uniqueidentifier", false), "UUID");
        assert_eq!(map_type(&MS, &MY, "datetimeoffset", false), "DATETIME(6)");
    }

    #[test]
    fn sqlite_affinity_and_targets() {
        assert_eq!(map_type(&LITE, &PG, "INTEGER", false), "BIGINT");
        assert_eq!(map_type(&LITE, &PG, "VARCHAR(20)", false), "TEXT");
        assert_eq!(map_type(&LITE, &PG, "", false), "TEXT");
        assert_eq!(map_type(&LITE, &MY, "REAL", false), "DOUBLE");
        assert_eq!(map_type(&PG, &LITE, "numeric(10,2)", false), "NUMERIC");
        assert_eq!(map_type(&MY, &LITE, "bigint", false), "INTEGER");
    }

    #[test]
    fn same_engine_keeps_original_type() {
        assert_eq!(
            map_type(&PG, &PG, "geometry(Point,4326)", false),
            "geometry(Point,4326)"
        );
    }

    #[test]
    fn create_table_per_dialect() {
        let cols = vec![
            SourceColumn {
                name: "id".into(),
                data_type: "int(11)".into(),
                nullable: false,
                primary_key: true,
            },
            SourceColumn {
                name: "note".into(),
                data_type: "text".into(),
                nullable: true,
                primary_key: false,
            },
        ];
        let pg = create_table_sql(&MY, &PG, "\"t\"", &cols);
        assert_eq!(
            pg,
            "CREATE TABLE IF NOT EXISTS \"t\" (\n  \"id\" INTEGER NOT NULL,\n  \"note\" TEXT,\n  PRIMARY KEY (\"id\")\n);"
        );
        let ms = create_table_sql(&MY, &MS, "[dbo].[t]", &cols);
        assert!(
            ms.starts_with("IF OBJECT_ID(N'[dbo].[t]', N'U') IS NULL\nCREATE TABLE [dbo].[t] (")
        );
        assert!(ms.contains("[note] NVARCHAR(MAX)"));
    }
}
