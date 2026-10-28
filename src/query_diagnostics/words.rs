//! Daftar kata dan kemiripan nama untuk diagnostik: kata kunci yang bukan
//! kolom, awal statement yang sah, dan saran "Did you mean" berbasis jarak edit.

/// Kata (unquoted) yang tidak pernah dianggap nama kolom: kata kunci, literal,
/// fungsi tanpa kurung, satuan tanggal, dan nama tipe. Kolom bernama seperti
/// ini memang tidak diperiksa (lebih baik terlewat daripada salah tandai).
const NOT_COLUMNS: &[&str] = &[
    // literal & fungsi tanpa kurung
    "TRUE",
    "FALSE",
    "NULL",
    "UNKNOWN",
    "DEFAULT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "CURRENT_USER",
    "SESSION_USER",
    "SYSTEM_USER",
    "USER",
    "CURRENT_SCHEMA",
    "CURRENT_CATALOG",
    "CURRENT_ROLE",
    "SYSDATE",
    "SYSDATETIME",
    "GETDATE",
    "ROWNUM",
    "ROWID",
    "OID",
    "CTID",
    "XMIN",
    "XMAX",
    "_ROWID_",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    // kata kunci ekspresi/klausa
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "ROWS",
    "ROW",
    "RANGE",
    "GROUPS",
    "UNBOUNDED",
    "PRECEDING",
    "FOLLOWING",
    "CURRENT",
    "PARTITION",
    "OVER",
    "FILTER",
    "WITHIN",
    "DISTINCT",
    "ALL",
    "ANY",
    "SOME",
    "EXISTS",
    "BETWEEN",
    "SYMMETRIC",
    "LIKE",
    "ILIKE",
    "RLIKE",
    "REGEXP",
    "GLOB",
    "MATCH",
    "ESCAPE",
    "IS",
    "NOT",
    "AND",
    "OR",
    "XOR",
    "DIV",
    "MOD",
    "IN",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "INTERVAL",
    "ARRAY",
    "COLLATE",
    "BINARY",
    "AT",
    "ZONE",
    "TIME",
    "TIMESTAMP",
    "DATE",
    "LEADING",
    "TRAILING",
    "BOTH",
    "FOR",
    "OF",
    "NO",
    "ACTION",
    "CASCADE",
    "RESTRICT",
    "NOTHING",
    "DO",
    "CONFLICT",
    "DUPLICATE",
    "KEY",
    "IGNORE",
    "REPLACE",
    "ABORT",
    "FAIL",
    "ROLLBACK",
    "TOP",
    "PERCENT",
    "TIES",
    "ONLY",
    "NEXT",
    "VALUE",
    "OUTPUT",
    "INSERTED",
    "DELETED",
    "EXCLUDED",
    "NEW",
    "OLD",
    "TABLESAMPLE",
    "SYSTEM",
    "BERNOULLI",
    "REPEATABLE",
    "LOCK",
    "SHARE",
    "MODE",
    "NOWAIT",
    "SKIP",
    "LOCKED",
    "OFFSET",
    "LIMIT",
    "FETCH",
    "WITH",
    "TIES",
    "NOLOCK",
    "READPAST",
    "UPDLOCK",
    "HOLDLOCK",
    "ROWLOCK",
    "TABLOCK",
    "TABLOCKX",
    "XLOCK",
    "PAGLOCK",
    "INDEX",
    "FORCESEEK",
    "OPTION",
    "RECOMPILE",
    "MAXDOP",
    "HASH",
    "LOOP",
    "MERGE",
    "REMOTE",
    "SIMILAR",
    "TO",
    "UNNEST",
    "ORDINALITY",
    "LATERAL",
    "NATURAL",
    "USING",
    "ESCAPE",
    "UESCAPE",
    "SEPARATOR",
    "SEARCH",
    "DEPTH",
    "BREADTH",
    "CYCLE",
    "SET",
    "SIGNED",
    "UNSIGNED",
    "CHARACTER",
    "VARYING",
    "PRECISION",
    "ZEROFILL",
    "AUTO_INCREMENT",
    "IDENTITY",
    "ROLLUP",
    "CUBE",
    "GROUPING",
    "SETS",
    "STRAIGHT_JOIN",
    "SQL_CALC_FOUND_ROWS",
    "HIGH_PRIORITY",
    "LOW_PRIORITY",
    "DELAYED",
    "QUICK",
    "SQL_NO_CACHE",
    "SQL_CACHE",
    "SQL_BUFFER_RESULT",
    "SQL_SMALL_RESULT",
    "SQL_BIG_RESULT",
    "OUTFILE",
    "DUMPFILE",
    "MATERIALIZED",
    "RECURSIVE",
    "VIEW",
    "STORED",
    "VIRTUAL",
    "GENERATED",
    "ALWAYS",
    "OVERRIDING",
    "SYSTEM_TIME",
    "CONTAINED",
    "FROM",
    "PIVOT",
    "UNPIVOT",
    "APPLY",
    "OUTER",
    "CROSS",
    "INNER",
    "LEFT",
    "RIGHT",
    "FULL",
    "JOIN",
    "ON",
    "WHERE",
    "GROUP",
    "BY",
    "HAVING",
    "ORDER",
    "WINDOW",
    "QUALIFY",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "MINUS",
    "SELECT",
    "AS",
    "INTO",
    "VALUES",
    "RETURNING",
    "TABLE",
    "UPDATE",
    "DELETE",
    "INSERT",
    "OVERLAPS",
    "DAYOFWEEK",
    // satuan tanggal (EXTRACT, DATEADD, INTERVAL, TIMESTAMPDIFF, DATE_TRUNC)
    "YEAR",
    "YEARS",
    "QUARTER",
    "MONTH",
    "MONTHS",
    "WEEK",
    "WEEKS",
    "DAY",
    "DAYS",
    "HOUR",
    "HOURS",
    "MINUTE",
    "MINUTES",
    "SECOND",
    "SECONDS",
    "MILLISECOND",
    "MILLISECONDS",
    "MICROSECOND",
    "MICROSECONDS",
    "NANOSECOND",
    "EPOCH",
    "DOW",
    "DOY",
    "ISODOW",
    "ISOYEAR",
    "DECADE",
    "CENTURY",
    "MILLENNIUM",
    "TIMEZONE",
    "TIMEZONE_HOUR",
    "TIMEZONE_MINUTE",
    "YY",
    "YYYY",
    "QQ",
    "Q",
    "MM",
    "M",
    "DY",
    "Y",
    "DD",
    "D",
    "WK",
    "WW",
    "DW",
    "W",
    "HH",
    "MI",
    "N",
    "SS",
    "S",
    "MS",
    "MCS",
    "NS",
    "TZ",
    "TZOFFSET",
    "ISO_WEEK",
    "ISOWK",
    "ISOWW",
    "WEEKDAY",
    "DAYOFYEAR",
    "YEAR_MONTH",
    "DAY_HOUR",
    "DAY_MINUTE",
    "DAY_SECOND",
    "HOUR_MINUTE",
    "HOUR_SECOND",
    "MINUTE_SECOND",
    "SECOND_MICROSECOND",
    // nama tipe (CAST/CONVERT/::)
    "INT",
    "INTEGER",
    "BIGINT",
    "SMALLINT",
    "TINYINT",
    "MEDIUMINT",
    "INT2",
    "INT4",
    "INT8",
    "SERIAL",
    "BIGSERIAL",
    "SMALLSERIAL",
    "DECIMAL",
    "NUMERIC",
    "DEC",
    "FLOAT",
    "FLOAT4",
    "FLOAT8",
    "REAL",
    "DOUBLE",
    "MONEY",
    "SMALLMONEY",
    "BIT",
    "BOOL",
    "BOOLEAN",
    "CHAR",
    "NCHAR",
    "VARCHAR",
    "NVARCHAR",
    "VARCHAR2",
    "TEXT",
    "NTEXT",
    "TINYTEXT",
    "MEDIUMTEXT",
    "LONGTEXT",
    "CLOB",
    "BLOB",
    "TINYBLOB",
    "MEDIUMBLOB",
    "LONGBLOB",
    "BYTEA",
    "VARBINARY",
    "IMAGE",
    "DATETIME",
    "DATETIME2",
    "SMALLDATETIME",
    "DATETIMEOFFSET",
    "TIMESTAMPTZ",
    "TIMETZ",
    "JSON",
    "JSONB",
    "XML",
    "UUID",
    "UNIQUEIDENTIFIER",
    "INET",
    "CIDR",
    "MACADDR",
    "TSVECTOR",
    "TSQUERY",
    "REGCLASS",
    "REGTYPE",
    "OID",
    "CITEXT",
    "HSTORE",
    "POINT",
    "GEOMETRY",
    "GEOGRAPHY",
    "SQL_VARIANT",
    "HIERARCHYID",
    "ROWVERSION",
    "ENUM",
    "YEAR",
    "UTF8",
    "UTF8MB4",
    "LATIN1",
    "ASCII",
    "UNICODE",
    "MAX",
];

/// Kata yang boleh mengawali statement di salah satu database yang didukung.
const STATEMENT_STARTS: &[&str] = &[
    "SELECT",
    "WITH",
    "INSERT",
    "UPDATE",
    "DELETE",
    "MERGE",
    "UPSERT",
    "REPLACE",
    "VALUES",
    "TABLE",
    "CREATE",
    "ALTER",
    "DROP",
    "TRUNCATE",
    "RENAME",
    "COMMENT",
    "GRANT",
    "REVOKE",
    "DENY",
    "BEGIN",
    "START",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    "END",
    "ABORT",
    "SET",
    "RESET",
    "SHOW",
    "DESCRIBE",
    "DESC",
    "EXPLAIN",
    "ANALYZE",
    "ANALYSE",
    "VACUUM",
    "REINDEX",
    "CLUSTER",
    "CHECKPOINT",
    "DISCARD",
    "USE",
    "CALL",
    "EXEC",
    "EXECUTE",
    "PREPARE",
    "DEALLOCATE",
    "DECLARE",
    "FETCH",
    "MOVE",
    "CLOSE",
    "OPEN",
    "PRAGMA",
    "ATTACH",
    "DETACH",
    "COPY",
    "LOCK",
    "UNLOCK",
    "LISTEN",
    "NOTIFY",
    "UNLISTEN",
    "LOAD",
    "DO",
    "REFRESH",
    "SECURITY",
    "IMPORT",
    "REASSIGN",
    "IF",
    "WHILE",
    "PRINT",
    "RAISERROR",
    "THROW",
    "RETURN",
    "GO",
    "WAITFOR",
    "BACKUP",
    "RESTORE",
    "DBCC",
    "BULK",
    "REVERT",
    "RECONFIGURE",
    "SHUTDOWN",
    "KILL",
    "HANDLER",
    "OPTIMIZE",
    "REPAIR",
    "CHECK",
    "CHECKSUM",
    "FLUSH",
    "PURGE",
    "CHANGE",
    "INSTALL",
    "UNINSTALL",
    "DELIMITER",
    "SOURCE",
    "XA",
    "SIGNAL",
    "RESIGNAL",
    "GET",
    "ITERATE",
    "LEAVE",
    "REPEAT",
    "CASE",
    "BREAK",
    "CONTINUE",
    "GOTO",
    "ENABLE",
    "DISABLE",
    "SETUSER",
    "READTEXT",
    "WRITETEXT",
    "UPDATETEXT",
    "HELP",
    "BINLOG",
    "CACHE",
    "SUMMARIZE",
    "PIVOT",
    "FROM",
    "ELSE",
    "ELSEIF",
    "LOOP",
    "OUTPUT",
];

/// Kata kunci klausa yang sering salah ketik (`FORM`, `WEHRE`, …).
pub(crate) const CLAUSE_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "GROUP", "ORDER", "HAVING", "LIMIT", "JOIN", "INNER", "LEFT",
    "RIGHT", "OUTER", "VALUES", "INTO", "UNION", "DISTINCT", "OFFSET", "BETWEEN",
];

fn contains(list: &[&str], word: &str) -> bool {
    list.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Kata unquoted ini bukan nama kolom (kata kunci, literal, tipe, satuan).
pub fn is_not_column(word: &str) -> bool {
    contains(NOT_COLUMNS, word) || crate::autocomplete::analyzer::is_reserved(word)
}

pub fn is_statement_start(word: &str) -> bool {
    contains(STATEMENT_STARTS, word)
}

/// Awal statement yang paling mirip (untuk `SELEC` → `SELECT`).
pub fn closest_statement_start(word: &str) -> Option<&'static str> {
    closest_keyword(word, STATEMENT_STARTS)
}

/// Kata kunci dari `list` yang berjarak edit kecil dari `word`.
pub fn closest_keyword(word: &str, list: &[&'static str]) -> Option<&'static str> {
    if word.len() < 3 {
        return None;
    }
    let up = word.to_ascii_uppercase();
    let limit = if up.len() <= 4 { 1 } else { 2 };
    list.iter()
        .filter(|k| k.len() >= 4 || up.len() >= 4)
        .map(|k| (edit_distance(&up, k), *k))
        .filter(|(d, _)| *d > 0 && *d <= limit)
        .min_by_key(|(d, k)| (*d, k.len().abs_diff(up.len())))
        .map(|(_, k)| k)
}

/// Jarak Damerau-Levenshtein (transposisi dihitung satu langkah), tanpa beda huruf besar.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<char> = b.chars().map(|c| c.to_ascii_lowercase()).collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
}

/// Saran nama paling mirip dari `candidates` (maksimal `max`), terbaik dulu.
/// Memakai jarak edit dan, untuk nama panjang, kecocokan fuzzy autocomplete.
pub fn suggest(word: &str, candidates: &[String], max: usize) -> Vec<String> {
    let w = word.to_ascii_lowercase();
    if w.is_empty() {
        return Vec::new();
    }
    let limit = match w.chars().count() {
        0..=3 => 1,
        4..=6 => 2,
        _ => 3,
    };
    let mut scored: Vec<(usize, i32, &String)> = candidates
        .iter()
        .filter_map(|c| {
            let lc = c.to_ascii_lowercase();
            if lc == w {
                return None;
            }
            let dist = edit_distance(&w, &lc);
            // `usr_id` → `user_id`, `custmer` → `customer`: jarak kecil.
            // `name` → `customer_name`: salah satu memuat yang lain.
            let contained = w.len() >= 3 && (lc.contains(&w) || w.contains(&lc));
            if dist <= limit || contained {
                let fuzzy = crate::autocomplete::fuzzy_match(&w, &lc).unwrap_or(0);
                Some((if contained { dist.min(limit) } else { dist }, -fuzzy, c))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.len().cmp(&b.2.len()))
    });
    let mut out: Vec<String> = Vec::new();
    for (_, _, c) in scored {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(c)) {
            out.push(c.clone());
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_counts_transpositions_once() {
        assert_eq!(edit_distance("FORM", "FROM"), 1);
        assert_eq!(edit_distance("selec", "SELECT"), 1);
        assert_eq!(edit_distance("abc", "abc"), 0);
    }

    #[test]
    fn suggests_close_names_first() {
        let cols = vec![
            "customer_id".to_string(),
            "customer_name".to_string(),
            "created_at".to_string(),
        ];
        assert_eq!(suggest("custmer_id", &cols, 3)[0], "customer_id");
        assert!(suggest("zzz", &cols, 3).is_empty());
    }

    #[test]
    fn typo_of_statement_start_is_found() {
        assert_eq!(closest_statement_start("SELCT"), Some("SELECT"));
        assert_eq!(closest_statement_start("UPDTE"), Some("UPDATE"));
        assert_eq!(closest_statement_start("customer"), None);
    }
}
