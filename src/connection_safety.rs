//! Mode keamanan eksekusi per koneksi untuk query yang dijalankan manusia.
//!
//! Tiga mode:
//! - `Standard`: konfirmasi UPDATE/DELETE tanpa WHERE dan statement destruktif
//!   (DROP TABLE/DATABASE, TRUNCATE, ALTER ... DROP COLUMN, FLUSHALL).
//! - `ConfirmWrites`: seperti Standard, ditambah konfirmasi untuk setiap
//!   statement yang mengubah data atau skema. Default untuk koneksi Production.
//! - `ReadOnly`: hanya statement baca yang dijalankan; sisanya ditolak.
//!
//! Klasifikasi statement memakai `agent::classify` dan `safety_guard` yang
//! juga dipakai agent MCP, sehingga GUI dan agent menilai SQL dengan cara sama.
//!
//! Disimpan di tabel `connection_safety` (`connections.db`), terpisah dari
//! `connections` seperti `connection_environment`, jadi lokal per mesin.

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::agent::classify::{StatementKind, classify_query};
use crate::connection_env::Environment;
use crate::models::enums::DatabaseType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SafetyMode {
    Standard,
    ConfirmWrites,
    ReadOnly,
}

impl SafetyMode {
    pub const ALL: [SafetyMode; 3] = [
        SafetyMode::Standard,
        SafetyMode::ConfirmWrites,
        SafetyMode::ReadOnly,
    ];

    pub fn key(self) -> &'static str {
        match self {
            SafetyMode::Standard => "standard",
            SafetyMode::ConfirmWrites => "confirm_writes",
            SafetyMode::ReadOnly => "read_only",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| m.key().eq_ignore_ascii_case(key.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            SafetyMode::Standard => "Standard",
            SafetyMode::ConfirmWrites => "Confirm all writes",
            SafetyMode::ReadOnly => "Read-only",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            SafetyMode::Standard => {
                "Asks before UPDATE/DELETE without WHERE and before DROP, TRUNCATE or similar."
            }
            SafetyMode::ConfirmWrites => {
                "Also asks before every statement that changes data or schema."
            }
            SafetyMode::ReadOnly => "Only statements that read data are run; the rest are refused.",
        }
    }
}

/// Mode efektif: pilihan eksplisit, atau `ConfirmWrites` untuk Production.
pub fn effective(explicit: Option<SafetyMode>, env: Option<Environment>) -> SafetyMode {
    explicit.unwrap_or(if env == Some(Environment::Production) {
        SafetyMode::ConfirmWrites
    } else {
        SafetyMode::Standard
    })
}

/// Keputusan untuk satu query (bisa berisi beberapa statement).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Jalankan hanya setelah user mengonfirmasi. `reason` satu kalimat.
    Confirm {
        title: String,
        reason: String,
    },
    /// Jangan jalankan.
    Block {
        reason: String,
    },
}

/// Statement `ALTER TABLE ... DROP ...` (kolom, constraint, partisi).
fn is_destructive_alter(statement: &str) -> bool {
    let upper = statement.to_ascii_uppercase();
    let mut words = upper.split_whitespace();
    words.next() == Some("ALTER")
        && words.next() == Some("TABLE")
        && upper.split_whitespace().any(|w| w == "DROP")
}

/// Statement admin yang tetap boleh di koneksi read-only (tidak mengubah data).
fn is_harmless_admin(statement: &str) -> bool {
    let first = statement
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == ';')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    matches!(
        first.as_str(),
        "USE" | "BEGIN" | "START" | "COMMIT" | "ROLLBACK" | "END"
    )
}

fn kind_label(kind: StatementKind) -> &'static str {
    crate::agent::access::kind_label(kind)
}

/// Nilai query sesuai mode koneksi. Murni (tanpa I/O) agar mudah diuji.
pub fn assess(db: &DatabaseType, sql: &str, mode: SafetyMode) -> Verdict {
    let statements = classify_query(db, sql);
    if statements.is_empty() {
        return Verdict::Allow;
    }

    if mode == SafetyMode::ReadOnly
        && let Some((stmt, kind)) = statements.iter().find(|(stmt, kind)| {
            *kind != StatementKind::Read
                && !(*kind == StatementKind::Admin && is_harmless_admin(stmt))
        })
    {
        let preview: String = stmt.trim().chars().take(80).collect();
        return Verdict::Block {
            reason: format!(
                "This connection is read-only, so the {} statement was not run: {preview}",
                kind_label(*kind)
            ),
        };
    }

    // Redis/MongoDB tidak punya UPDATE/DELETE SQL; safety_guard hanya untuk SQL.
    let is_sql = !matches!(db, DatabaseType::Redis | DatabaseType::MongoDB);
    if is_sql && let Some(report) = crate::safety_guard::analyze_safety(sql) {
        let target = report
            .table_name
            .map(|t| format!(" on \"{t}\""))
            .unwrap_or_default();
        return Verdict::Confirm {
            title: format!("{} without WHERE", report.statement_type),
            reason: format!(
                "This {}{target} has no WHERE clause and will affect every row in the table.",
                report.statement_type
            ),
        };
    }

    if let Some((stmt, _)) = statements
        .iter()
        .find(|(stmt, _)| crate::agent::access::is_destructive(stmt) || is_destructive_alter(stmt))
    {
        let preview: String = stmt.trim().chars().take(80).collect();
        return Verdict::Confirm {
            title: "Destructive statement".to_string(),
            reason: format!("This statement permanently removes data or schema objects: {preview}"),
        };
    }

    if mode == SafetyMode::ConfirmWrites
        && let Some((_, kind)) = statements.iter().find(|(_, kind)| {
            matches!(
                kind,
                StatementKind::Write | StatementKind::Ddl | StatementKind::Unknown
            )
        })
    {
        return Verdict::Confirm {
            title: "Confirm change".to_string(),
            reason: format!(
                "This connection asks before every change. Run this {} statement?",
                kind_label(*kind)
            ),
        };
    }

    Verdict::Allow
}

async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS connection_safety (\
            connection_id INTEGER PRIMARY KEY, \
            mode TEXT NOT NULL)",
    )
    .execute(pool)
    .await
    .map(|_| ())
}

/// Semua mode eksplisit, per id koneksi.
pub async fn load_all(pool: &SqlitePool) -> Result<HashMap<i64, SafetyMode>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT connection_id, mode FROM connection_safety")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, key)| SafetyMode::parse(&key).map(|m| (id, m)))
        .collect())
}

/// Simpan atau hapus (`None`, kembali ke default) mode eksplisit.
pub async fn set(
    pool: &SqlitePool,
    connection_id: i64,
    mode: Option<SafetyMode>,
) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    match mode {
        Some(m) => {
            sqlx::query(
                "INSERT INTO connection_safety (connection_id, mode) VALUES (?, ?) \
                 ON CONFLICT(connection_id) DO UPDATE SET mode = excluded.mode",
            )
            .bind(connection_id)
            .bind(m.key())
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM connection_safety WHERE connection_id = ?")
                .bind(connection_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: DatabaseType = DatabaseType::PostgreSQL;

    #[test]
    fn production_defaults_to_confirm_writes() {
        assert_eq!(
            effective(None, Some(Environment::Production)),
            SafetyMode::ConfirmWrites
        );
        assert_eq!(effective(None, None), SafetyMode::Standard);
        assert_eq!(
            effective(Some(SafetyMode::ReadOnly), Some(Environment::Local)),
            SafetyMode::ReadOnly
        );
    }

    #[test]
    fn unsafe_dml_in_any_statement_needs_confirmation() {
        // Statement kedua yang berbahaya tidak boleh lolos.
        let v = assess(&PG, "SELECT 1; DELETE FROM users", SafetyMode::Standard);
        assert!(matches!(v, Verdict::Confirm { .. }), "{v:?}");
        assert_eq!(
            assess(&PG, "DELETE FROM users WHERE id = 1", SafetyMode::Standard),
            Verdict::Allow
        );
    }

    #[test]
    fn destructive_ddl_needs_confirmation() {
        for sql in [
            "DROP TABLE orders",
            "TRUNCATE orders",
            "ALTER TABLE orders DROP COLUMN note",
            "select 1; drop database shop",
        ] {
            let v = assess(&PG, sql, SafetyMode::Standard);
            assert!(matches!(v, Verdict::Confirm { .. }), "{sql}: {v:?}");
        }
        assert_eq!(
            assess(
                &PG,
                "ALTER TABLE orders ADD COLUMN note text",
                SafetyMode::Standard
            ),
            Verdict::Allow
        );
    }

    #[test]
    fn confirm_writes_asks_for_any_change_but_not_reads() {
        let v = assess(&PG, "INSERT INTO t VALUES (1)", SafetyMode::ConfirmWrites);
        assert!(matches!(v, Verdict::Confirm { .. }));
        assert_eq!(
            assess(&PG, "SELECT * FROM t", SafetyMode::ConfirmWrites),
            Verdict::Allow
        );
        assert_eq!(
            assess(&PG, "INSERT INTO t VALUES (1)", SafetyMode::Standard),
            Verdict::Allow
        );
    }

    #[test]
    fn read_only_blocks_writes_and_allows_reads() {
        assert!(matches!(
            assess(
                &PG,
                "SELECT 1; UPDATE t SET a = 1 WHERE id = 2",
                SafetyMode::ReadOnly
            ),
            Verdict::Block { .. }
        ));
        assert_eq!(
            assess(
                &PG,
                "SELECT * FROM t; EXPLAIN SELECT 1",
                SafetyMode::ReadOnly
            ),
            Verdict::Allow
        );
        assert_eq!(
            assess(
                &DatabaseType::MySQL,
                "USE shop; SELECT 1",
                SafetyMode::ReadOnly
            ),
            Verdict::Allow
        );
        // SELECT dengan efek samping bukan read.
        assert!(matches!(
            assess(&PG, "SELECT pg_terminate_backend(42)", SafetyMode::ReadOnly),
            Verdict::Block { .. }
        ));
    }

    #[test]
    fn keys_roundtrip() {
        for m in SafetyMode::ALL {
            assert_eq!(SafetyMode::parse(m.key()), Some(m));
        }
        assert_eq!(SafetyMode::parse("nope"), None);
    }

    #[tokio::test]
    async fn persists_and_clears_in_sqlite() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        set(&pool, 1, Some(SafetyMode::ReadOnly)).await.unwrap();
        set(&pool, 1, Some(SafetyMode::ConfirmWrites))
            .await
            .unwrap();
        assert_eq!(
            load_all(&pool).await.unwrap().get(&1),
            Some(&SafetyMode::ConfirmWrites)
        );
        set(&pool, 1, None).await.unwrap();
        assert!(load_all(&pool).await.unwrap().is_empty());
    }
}
