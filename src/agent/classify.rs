//! Klasifikasi statement untuk gerbang read-only agent.
//!
//! Agent hanya boleh menjalankan statement yang tidak mengubah data, skema,
//! atau state sesi. Klasifikasi di sini sengaja konservatif: statement yang
//! tidak dikenali dianggap **bukan** read-only, dan `EXPLAIN ANALYZE` diperiksa
//! berdasarkan statement di dalamnya karena ia benar-benar mengeksekusi query.
//!
//! Parser ini bukan parser SQL penuh. Ia hanya memindai kata kunci di luar
//! string literal, identifier berkutip, dan komentar, sambil melacak kedalaman
//! tanda kurung, sehingga `INSERT` di dalam subquery CTE tetap terdeteksi
//! tanpa terkecoh oleh teks di dalam literal.

use serde::Serialize;

use crate::models::enums::DatabaseType;

/// Jenis statement dari sudut pandang keamanan agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementKind {
    /// Hanya membaca: SELECT, SHOW, EXPLAIN (tanpa DML di dalamnya), dll.
    Read,
    /// Mengubah data: INSERT, UPDATE, DELETE, MERGE, TRUNCATE, CALL, dll.
    Write,
    /// Mengubah skema atau hak akses: CREATE, ALTER, DROP, GRANT, dll.
    Ddl,
    /// Mengubah state sesi/server: SET, USE, BEGIN, COMMIT, KILL, PRAGMA, dll.
    Admin,
    /// Tidak dikenali; diperlakukan sebagai tidak aman.
    Unknown,
}

impl StatementKind {
    pub fn is_read_only(self) -> bool {
        matches!(self, StatementKind::Read)
    }
}

/// Kata kunci pada kedalaman kurung tertentu. `depth == 0` berarti berada di
/// level teratas statement.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Keyword {
    word: String,
    depth: usize,
}

fn flush(word: &mut String, out: &mut Vec<Keyword>, depth: usize) {
    if !word.is_empty() {
        out.push(Keyword {
            word: word.to_ascii_uppercase(),
            depth,
        });
        word.clear();
    }
}

/// Ambil kata kunci (huruf besar) di luar literal dan komentar.
fn scan_keywords(sql: &str) -> Vec<Keyword> {
    let bytes = sql.as_bytes();
    let mut out = Vec::new();
    let mut depth: usize = 0;
    let mut i = 0;
    let mut word = String::new();

    while i < bytes.len() {
        let c = bytes[i];
        // Komentar baris (`--` standar, `#` gaya MySQL)
        if (c == b'-' && bytes.get(i + 1) == Some(&b'-')) || c == b'#' {
            flush(&mut word, &mut out, depth);
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Komentar blok
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            flush(&mut word, &mut out, depth);
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            continue;
        }
        // Literal / identifier berkutip: ' " ` [ ]
        if c == b'\'' || c == b'"' || c == b'`' || c == b'[' {
            flush(&mut word, &mut out, depth);
            let close = if c == b'[' { b']' } else { c };
            i += 1;
            while i < bytes.len() {
                if bytes[i] == close {
                    // Kutip ganda sebagai escape ('' atau "")
                    if close != b']' && bytes.get(i + 1) == Some(&close) {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        // Dollar-quoted string PostgreSQL ($$ ... $$ atau $tag$ ... $tag$)
        if c == b'$' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if bytes.get(j) == Some(&b'$') {
                flush(&mut word, &mut out, depth);
                let tag = &sql[i..=j];
                let rest = &sql[j + 1..];
                if let Some(end) = rest.find(tag) {
                    i = j + 1 + end + tag.len();
                } else {
                    i = bytes.len();
                }
                continue;
            }
        }
        if c == b'(' {
            flush(&mut word, &mut out, depth);
            depth += 1;
            i += 1;
            continue;
        }
        if c == b')' {
            flush(&mut word, &mut out, depth);
            depth = depth.saturating_sub(1);
            i += 1;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == b'_' {
            word.push(c as char);
        } else {
            flush(&mut word, &mut out, depth);
        }
        i += 1;
    }
    flush(&mut word, &mut out, depth);
    out
}

const READ_STARTERS: &[&str] = &[
    "SELECT", "VALUES", "TABLE", "SHOW", "DESCRIBE", "DESC", "EXPLAIN", "WITH",
];
const WRITE_STARTERS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "TRUNCATE", "LOAD", "COPY", "CALL",
    "EXEC", "EXECUTE", "IMPORT", "BULK", "DO",
];
const DDL_STARTERS: &[&str] = &[
    "CREATE", "ALTER", "DROP", "RENAME", "COMMENT", "GRANT", "REVOKE", "REFRESH", "CLUSTER",
];
const ADMIN_STARTERS: &[&str] = &[
    "SET",
    "USE",
    "BEGIN",
    "START",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    "LOCK",
    "UNLOCK",
    "KILL",
    "VACUUM",
    "ANALYZE",
    "ANALYSE",
    "REINDEX",
    "ATTACH",
    "DETACH",
    "PRAGMA",
    "FLUSH",
    "RESET",
    "OPTIMIZE",
    "REPAIR",
    "CHECKPOINT",
    "LISTEN",
    "NOTIFY",
    "DISCARD",
    "DEALLOCATE",
    "PREPARE",
    "DECLARE",
    "FETCH",
    "CLOSE",
    "SHUTDOWN",
    "BACKUP",
    "RESTORE",
    "DBCC",
];

/// Kata opsi yang boleh muncul di antara `EXPLAIN` dan statement-nya.
const EXPLAIN_OPTIONS: &[&str] = &[
    "ANALYZE",
    "ANALYSE",
    "VERBOSE",
    "FORMAT",
    "JSON",
    "TEXT",
    "XML",
    "YAML",
    "TREE",
    "TRADITIONAL",
    "EXTENDED",
    "PARTITIONS",
    "QUERY",
    "PLAN",
    "COSTS",
    "BUFFERS",
    "TIMING",
    "SUMMARY",
    "SETTINGS",
    "WAL",
    "GENERIC_PLAN",
    "ON",
    "OFF",
    "TRUE",
    "FALSE",
];

fn kind_of_starter(word: &str) -> Option<StatementKind> {
    if READ_STARTERS.contains(&word) {
        Some(StatementKind::Read)
    } else if WRITE_STARTERS.contains(&word) {
        Some(StatementKind::Write)
    } else if DDL_STARTERS.contains(&word) {
        Some(StatementKind::Ddl)
    } else if ADMIN_STARTERS.contains(&word) {
        Some(StatementKind::Admin)
    } else {
        None
    }
}

/// Kata kunci yang, bila muncul di level teratas sebuah SELECT/WITH, membuat
/// statement tersebut menulis (SELECT ... INTO tabel_baru / OUTFILE, CTE
/// dengan DML di dalamnya).
const WRITE_INSIDE_READ: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "INTO", "OUTFILE", "DUMPFILE",
];

/// Klasifikasikan satu statement SQL.
pub fn classify_sql_statement(sql: &str) -> StatementKind {
    let keywords = scan_keywords(sql);
    if keywords.is_empty() {
        return StatementKind::Unknown;
    }

    if keywords[0].word == "EXPLAIN" {
        // EXPLAIN ANALYZE benar-benar mengeksekusi statement di dalamnya:
        // lewati kata opsi EXPLAIN (ANALYZE, VERBOSE, FORMAT, QUERY PLAN, ...)
        // lalu nilai statement pertama yang ditemukan.
        let inner = keywords
            .iter()
            .skip(1)
            .position(|k| {
                k.depth == 0
                    && !EXPLAIN_OPTIONS.contains(&k.word.as_str())
                    && kind_of_starter(&k.word).is_some()
            })
            .map(|p| p + 1);
        return match inner {
            Some(idx) => classify_from(&keywords, idx),
            None => StatementKind::Read,
        };
    }

    classify_from(&keywords, 0)
}

fn classify_from(keywords: &[Keyword], start_idx: usize) -> StatementKind {
    let start = &keywords[start_idx];
    match kind_of_starter(&start.word) {
        Some(StatementKind::Read) => {
            // SELECT ... INTO / CTE yang membungkus DML tetap dianggap menulis.
            // Untuk WITH, DML berada satu level kurung di dalam CTE.
            let max_depth = if start.word == "WITH" {
                start.depth + 1
            } else {
                start.depth
            };
            let writes = keywords[start_idx + 1..]
                .iter()
                .any(|k| k.depth <= max_depth && WRITE_INSIDE_READ.contains(&k.word.as_str()));
            if writes {
                StatementKind::Write
            } else {
                StatementKind::Read
            }
        }
        Some(kind) => kind,
        None => StatementKind::Unknown,
    }
}

/// Perintah Redis yang tidak mengubah data maupun konfigurasi server.
const REDIS_READ_COMMANDS: &[&str] = &[
    "GET",
    "MGET",
    "GETRANGE",
    "STRLEN",
    "EXISTS",
    "TYPE",
    "TTL",
    "PTTL",
    "KEYS",
    "SCAN",
    "DBSIZE",
    "RANDOMKEY",
    "HGET",
    "HMGET",
    "HGETALL",
    "HKEYS",
    "HVALS",
    "HLEN",
    "HEXISTS",
    "HSCAN",
    "HSTRLEN",
    "LRANGE",
    "LLEN",
    "LINDEX",
    "LPOS",
    "SMEMBERS",
    "SCARD",
    "SISMEMBER",
    "SMISMEMBER",
    "SRANDMEMBER",
    "SSCAN",
    "SDIFF",
    "SINTER",
    "SUNION",
    "ZRANGE",
    "ZRANGEBYSCORE",
    "ZREVRANGE",
    "ZREVRANGEBYSCORE",
    "ZRANGEBYLEX",
    "ZCARD",
    "ZCOUNT",
    "ZSCORE",
    "ZMSCORE",
    "ZRANK",
    "ZREVRANK",
    "ZSCAN",
    "ZLEXCOUNT",
    "XRANGE",
    "XREVRANGE",
    "XLEN",
    "XINFO",
    "PFCOUNT",
    "GEOPOS",
    "GEODIST",
    "GEOHASH",
    "GEOSEARCH",
    "BITCOUNT",
    "BITPOS",
    "GETBIT",
    "INFO",
    "PING",
    "ECHO",
    "TIME",
    "LASTSAVE",
    "OBJECT",
    "MEMORY",
    "DUMP",
    "JSON.GET",
    "JSON.MGET",
    "JSON.TYPE",
    "JSON.STRLEN",
    "JSON.ARRLEN",
    "JSON.OBJKEYS",
    "JSON.OBJLEN",
    "FT.SEARCH",
    "FT.INFO",
    "FT.AGGREGATE",
    "TS.GET",
    "TS.RANGE",
    "TS.MGET",
    "TS.MRANGE",
    "TS.INFO",
    "CLIENT",
    "CONFIG",
    "COMMAND",
    "SELECT",
];

/// Sub-perintah yang mengubah state walau perintah induknya ada di daftar baca.
fn redis_subcommand_is_write(cmd: &str, sub: Option<&str>) -> bool {
    let sub = sub.unwrap_or("").to_ascii_uppercase();
    match cmd {
        "CLIENT" => !matches!(
            sub.as_str(),
            "LIST" | "INFO" | "ID" | "GETNAME" | "TRACKINGINFO"
        ),
        "CONFIG" => sub != "GET",
        "MEMORY" => !matches!(sub.as_str(), "USAGE" | "STATS" | "DOCTOR" | "MALLOC-STATS"),
        _ => false,
    }
}

/// Klasifikasikan perintah Redis (baris perintah mentah, mis. `HGETALL user:1`).
pub fn classify_redis_command(command_line: &str) -> StatementKind {
    let mut parts = command_line.split_whitespace();
    let Some(cmd) = parts.next() else {
        return StatementKind::Unknown;
    };
    let cmd = cmd.to_ascii_uppercase();
    let sub = parts.next();
    if REDIS_READ_COMMANDS.contains(&cmd.as_str()) {
        if redis_subcommand_is_write(&cmd, sub) {
            StatementKind::Admin
        } else {
            StatementKind::Read
        }
    } else if matches!(
        cmd.as_str(),
        "FLUSHALL" | "FLUSHDB" | "SHUTDOWN" | "DEBUG" | "MIGRATE" | "REPLICAOF" | "SLAVEOF"
    ) {
        StatementKind::Admin
    } else {
        StatementKind::Write
    }
}

/// Klasifikasikan teks query sesuai jenis koneksi. Untuk SQL, teks bisa berisi
/// beberapa statement; hasilnya per statement.
pub fn classify_query(db_type: &DatabaseType, text: &str) -> Vec<(String, StatementKind)> {
    match db_type {
        DatabaseType::Redis => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| (l.to_string(), classify_redis_command(l)))
            .collect(),
        DatabaseType::MongoDB | DatabaseType::ApiHttp => {
            vec![(text.trim().to_string(), StatementKind::Unknown)]
        }
        _ => crate::query_tools::statement_parser::split_statements(text)
            .into_iter()
            .map(|span| {
                let stmt = span.text.trim().to_string();
                let kind = classify_sql_statement(&stmt);
                (stmt, kind)
            })
            .filter(|(stmt, _)| !stmt.is_empty())
            .collect(),
    }
}

/// `true` bila semua statement di `text` hanya membaca.
pub fn is_read_only(db_type: &DatabaseType, text: &str) -> bool {
    let parts = classify_query(db_type, text);
    !parts.is_empty() && parts.iter().all(|(_, k)| k.is_read_only())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(sql: &str) -> StatementKind {
        classify_sql_statement(sql)
    }

    #[test]
    fn plain_select_is_read() {
        assert_eq!(
            k("SELECT * FROM users WHERE name = 'x'"),
            StatementKind::Read
        );
        assert_eq!(k("  select 1"), StatementKind::Read);
        assert_eq!(k("SHOW TABLES"), StatementKind::Read);
        assert_eq!(k("DESCRIBE users"), StatementKind::Read);
    }

    #[test]
    fn dml_is_write() {
        assert_eq!(k("INSERT INTO t VALUES (1)"), StatementKind::Write);
        assert_eq!(k("update t set a = 1"), StatementKind::Write);
        assert_eq!(k("DELETE FROM t"), StatementKind::Write);
        assert_eq!(k("TRUNCATE t"), StatementKind::Write);
        assert_eq!(k("CALL do_thing()"), StatementKind::Write);
    }

    #[test]
    fn ddl_and_admin() {
        assert_eq!(k("CREATE TABLE t (id INT)"), StatementKind::Ddl);
        assert_eq!(k("DROP TABLE t"), StatementKind::Ddl);
        assert_eq!(k("GRANT ALL ON t TO bob"), StatementKind::Ddl);
        assert_eq!(k("SET search_path = public"), StatementKind::Admin);
        assert_eq!(k("PRAGMA journal_mode = WAL"), StatementKind::Admin);
        assert_eq!(k("BEGIN"), StatementKind::Admin);
    }

    #[test]
    fn literals_and_comments_do_not_fool_the_scanner() {
        assert_eq!(k("SELECT 'DELETE FROM t' AS s"), StatementKind::Read);
        assert_eq!(k("SELECT \"INSERT\" FROM t"), StatementKind::Read);
        assert_eq!(k("-- DROP TABLE t\nSELECT 1"), StatementKind::Read);
        assert_eq!(k("/* UPDATE */ SELECT 1"), StatementKind::Read);
        assert_eq!(k("SELECT $$INSERT INTO x$$"), StatementKind::Read);
        assert_eq!(k("SELECT [INSERT] FROM t"), StatementKind::Read);
        assert_eq!(k("SELECT 'it''s' FROM t"), StatementKind::Read);
    }

    #[test]
    fn select_into_and_cte_with_dml_are_writes() {
        assert_eq!(k("SELECT * INTO new_t FROM t"), StatementKind::Write);
        assert_eq!(
            k("SELECT * FROM t INTO OUTFILE '/tmp/x'"),
            StatementKind::Write
        );
        assert_eq!(
            k("WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d"),
            StatementKind::Write
        );
        assert_eq!(
            k("WITH c AS (SELECT 1) SELECT * FROM c"),
            StatementKind::Read
        );
        // Subquery biasa tidak dianggap menulis.
        assert_eq!(
            k("SELECT * FROM t WHERE id IN (SELECT id FROM u)"),
            StatementKind::Read
        );
    }

    #[test]
    fn explain_follows_inner_statement() {
        assert_eq!(k("EXPLAIN SELECT 1"), StatementKind::Read);
        assert_eq!(
            k("EXPLAIN (ANALYZE, FORMAT JSON) SELECT 1"),
            StatementKind::Read
        );
        assert_eq!(k("EXPLAIN ANALYZE DELETE FROM t"), StatementKind::Write);
        assert_eq!(
            k("EXPLAIN FORMAT=JSON UPDATE t SET a = 1"),
            StatementKind::Write
        );
        assert_eq!(k("EXPLAIN QUERY PLAN SELECT * FROM t"), StatementKind::Read);
        assert_eq!(k("EXPLAIN VERBOSE DROP TABLE t"), StatementKind::Ddl);
    }

    #[test]
    fn unknown_is_not_read_only() {
        assert_eq!(k("FROBNICATE"), StatementKind::Unknown);
        assert!(!StatementKind::Unknown.is_read_only());
        assert_eq!(k(""), StatementKind::Unknown);
    }

    #[test]
    fn redis_commands() {
        assert_eq!(classify_redis_command("GET a"), StatementKind::Read);
        assert_eq!(
            classify_redis_command("hgetall user:1"),
            StatementKind::Read
        );
        assert_eq!(classify_redis_command("SET a 1"), StatementKind::Write);
        assert_eq!(classify_redis_command("DEL a"), StatementKind::Write);
        assert_eq!(classify_redis_command("FLUSHALL"), StatementKind::Admin);
        assert_eq!(
            classify_redis_command("CONFIG GET maxmemory"),
            StatementKind::Read
        );
        assert_eq!(
            classify_redis_command("CONFIG SET maxmemory 1"),
            StatementKind::Admin
        );
    }

    #[test]
    fn batch_is_read_only_only_when_all_parts_are() {
        let pg = DatabaseType::PostgreSQL;
        assert!(is_read_only(&pg, "SELECT 1; SELECT 2;"));
        assert!(!is_read_only(&pg, "SELECT 1; DELETE FROM t;"));
        assert!(!is_read_only(&pg, "   "));
        assert!(is_read_only(&DatabaseType::Redis, "GET a\nHGETALL b"));
        assert!(!is_read_only(&DatabaseType::Redis, "GET a\nDEL b"));
        assert!(!is_read_only(&DatabaseType::MongoDB, "db.users.find({})"));
    }
}
