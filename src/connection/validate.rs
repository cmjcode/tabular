//! Validasi statement di server **tanpa eksekusi**, untuk diagnostik live
//! Query Editor (`query_diagnostics`).
//!
//! - PostgreSQL: Parse/Describe (prepared statement) di dalam transaksi
//!   `READ ONLY` dengan `lock_timeout`, lalu ROLLBACK. Parse analysis
//!   memeriksa tabel, kolom, tipe, dan GROUP BY tanpa menjalankan apa pun.
//! - MySQL: `COM_STMT_PREPARE`; database tab dipilih dengan `USE` (hook
//!   `after_release` pool mengembalikannya ke default).
//! - SQLite: `sqlite3_prepare`, tidak pernah di-*step*.
//! - MsSQL: batch `SET NOEXEC ON; <stmt>; SET NOEXEC OFF` (dikompilasi,
//!   tidak dijalankan). Teks yang memuat `NOEXEC` tidak pernah dikirim
//!   (`query_diagnostics::analyze` menandainya tidak bisa divalidasi). Karena
//!   *deferred name resolution*, tabel yang tidak ada tidak dilaporkan, jadi
//!   hasil valid hanya berarti "sintaks valid" ([`ServerVerdict::SyntaxValid`]).
//!
//! Hanya koneksi yang menganggur (atau koneksi baru bila pool belum penuh)
//! yang dipakai: validasi tidak pernah menunggu koneksi milik query user. Bila
//! melewati batas waktu, koneksinya dilepas dari pool dan ditutup.

use std::time::Duration;

use sqlx::{AssertSqlSafe, Executor, SqlSafeStr};

use crate::autocomplete::lexer::Dialect;
use crate::models::enums::DatabasePool;
use crate::query_diagnostics::locate::{
    ServerPosition, locate, mssql_error_line, tidy_server_message,
};
use crate::query_diagnostics::{DiagCode, DiagSource, ServerVerdict};
use crate::query_tools::{LintMessage, LintSeverity};

/// Batas waktu satu validasi (termasuk persiapan sesi).
const VALIDATE_TIMEOUT: Duration = Duration::from_secs(4);

/// Koneksi untuk validasi: yang sedang menganggur, atau koneksi baru bila
/// pool belum penuh. Tidak pernah menunggu koneksi yang dipakai query user.
async fn acquire_spare<DB: sqlx::Database>(
    pool: &sqlx::Pool<DB>,
) -> Option<sqlx::pool::PoolConnection<DB>> {
    if let Some(conn) = pool.try_acquire() {
        return Some(conn);
    }
    if pool.size() < pool.options().get_max_connections() {
        return tokio::time::timeout(Duration::from_millis(500), pool.acquire())
            .await
            .ok()?
            .ok();
    }
    None
}

/// Hasil satu percobaan validasi.
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationOutcome {
    Verdict(ServerVerdict),
    /// Tidak ada koneksi menganggur saat ini; coba lagi nanti.
    Busy,
}

/// Konteks tab tempat statement akan dijalankan.
#[derive(Debug, Clone, Default)]
pub struct ValidationTarget {
    /// Database tab (MySQL: dipilih dengan `USE`).
    pub database: Option<String>,
    /// Schema tab (PostgreSQL: `search_path`).
    pub schema: Option<String>,
}

fn server_error(
    stmt: &str,
    message: &str,
    pos: ServerPosition,
    hint: Option<String>,
    dialect: Dialect,
) -> ServerVerdict {
    let span = locate(stmt, message, &pos, dialect).unwrap_or_else(|| {
        let lead = stmt.len() - stmt.trim_start().len();
        let first = stmt[lead..]
            .find(char::is_whitespace)
            .map_or(stmt.len(), |p| lead + p);
        lead..first
    });
    let mut m = LintMessage {
        severity: LintSeverity::Error,
        message: tidy_server_message(message),
        span: Some(span),
        code: Some(DiagCode::Server),
        source: DiagSource::Server,
        ..Default::default()
    };
    m.hint = hint;
    ServerVerdict::Invalid(vec![m])
}

/// Validasi satu statement. `stmt` sudah tanpa `;` penutup.
pub async fn validate_statement(
    pool: &DatabasePool,
    target: &ValidationTarget,
    stmt: &str,
) -> ValidationOutcome {
    match pool {
        DatabasePool::PostgreSQL(p) => validate_postgres(p, target, stmt).await,
        DatabasePool::MySQL(p) => validate_mysql(p, target, stmt).await,
        DatabasePool::SQLite(p) => validate_sqlite(p, stmt).await,
        DatabasePool::MsSQL(p) => validate_mssql(p.clone(), stmt).await,
        _ => ValidationOutcome::Verdict(ServerVerdict::Unavailable),
    }
}

/// Kelas SQLSTATE yang berarti "tidak bisa menilai sekarang", bukan SQL salah.
fn pg_transient(code: &str) -> bool {
    ["08", "25", "40", "53", "54", "55", "57", "58", "XX"]
        .iter()
        .any(|c| code.starts_with(c))
}

async fn validate_postgres(
    pool: &sqlx::PgPool,
    target: &ValidationTarget,
    stmt: &str,
) -> ValidationOutcome {
    let Some(mut conn) = acquire_spare(pool).await else {
        return ValidationOutcome::Busy;
    };
    let schema = target.schema.clone().filter(|s| !s.trim().is_empty());
    let sql = stmt.to_string();
    let work = async {
        // Transaksi read-only: lapis kedua, Parse sendiri tidak mengeksekusi.
        sqlx::raw_sql("BEGIN READ ONLY; SET LOCAL lock_timeout = '1500ms'; SET LOCAL statement_timeout = '3000ms'")
            .execute(&mut *conn)
            .await?;
        if let Some(schema) = &schema {
            let set_path = format!(
                "SET LOCAL search_path TO \"{}\", public",
                schema.replace('"', "\"\"")
            );
            sqlx::raw_sql(AssertSqlSafe(set_path))
                .execute(&mut *conn)
                .await?;
        }
        let result = (&mut *conn)
            .prepare(AssertSqlSafe(sql).into_sql_str())
            .await
            .map(|_| ());
        let _ = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await;
        Ok::<_, sqlx::Error>(result)
    };
    match tokio::time::timeout(VALIDATE_TIMEOUT, work).await {
        Err(_) => {
            drop(conn.detach());
            ValidationOutcome::Verdict(ServerVerdict::Unavailable)
        }
        Ok(Err(e)) => {
            log::debug!("[DIAG] PostgreSQL validation setup failed: {e}");
            ValidationOutcome::Verdict(ServerVerdict::Unavailable)
        }
        Ok(Ok(Ok(()))) => ValidationOutcome::Verdict(ServerVerdict::Valid),
        Ok(Ok(Err(sqlx::Error::Database(db)))) => {
            let code = db.code().map(|c| c.to_string()).unwrap_or_default();
            if pg_transient(&code) {
                return ValidationOutcome::Verdict(ServerVerdict::Unavailable);
            }
            let pg = db.try_downcast_ref::<sqlx::postgres::PgDatabaseError>();
            let char_offset = pg.and_then(|pg| match pg.position() {
                Some(sqlx::postgres::PgErrorPosition::Original(p)) => Some(p.saturating_sub(1)),
                _ => None,
            });
            let hint = pg.and_then(|pg| pg.hint().or(pg.detail()).map(str::to_string));
            ValidationOutcome::Verdict(server_error(
                stmt,
                db.message(),
                ServerPosition {
                    char_offset,
                    line: None,
                },
                hint,
                Dialect::Postgres,
            ))
        }
        Ok(Ok(Err(e))) => {
            log::debug!("[DIAG] PostgreSQL validation failed: {e}");
            ValidationOutcome::Verdict(ServerVerdict::Unavailable)
        }
    }
}

/// Error MySQL yang bukan kesalahan SQL: statement tidak bisa di-prepare
/// (1295), lock wait timeout (1205), deadlock (1213), server pergi.
fn mysql_transient(code: &str) -> bool {
    matches!(
        code,
        "1295" | "1205" | "1213" | "2006" | "2013" | "1317" | "3024"
    )
}

async fn validate_mysql(
    pool: &sqlx::MySqlPool,
    target: &ValidationTarget,
    stmt: &str,
) -> ValidationOutcome {
    let Some(mut conn) = acquire_spare(pool).await else {
        return ValidationOutcome::Busy;
    };
    let database = target.database.clone().filter(|s| !s.trim().is_empty());
    let sql = stmt.to_string();
    let work = async {
        if let Some(db) = &database {
            let use_stmt = format!("USE `{}`", db.trim().replace('`', "``"));
            sqlx::raw_sql(AssertSqlSafe(use_stmt))
                .execute(&mut *conn)
                .await?;
        }
        Ok::<_, sqlx::Error>(
            (&mut *conn)
                .prepare(AssertSqlSafe(sql).into_sql_str())
                .await
                .map(|_| ()),
        )
    };
    match tokio::time::timeout(VALIDATE_TIMEOUT, work).await {
        Err(_) => {
            drop(conn.detach());
            ValidationOutcome::Verdict(ServerVerdict::Unavailable)
        }
        Ok(Err(_)) => ValidationOutcome::Verdict(ServerVerdict::Unavailable),
        Ok(Ok(Ok(()))) => ValidationOutcome::Verdict(ServerVerdict::Valid),
        Ok(Ok(Err(sqlx::Error::Database(db)))) => {
            let code = db
                .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
                .map(|e| e.number().to_string())
                .unwrap_or_default();
            if mysql_transient(&code) {
                return ValidationOutcome::Verdict(ServerVerdict::Unavailable);
            }
            let message = db.message().to_string();
            let line = crate::connection::sql::mysql_error_line(&message);
            ValidationOutcome::Verdict(server_error(
                stmt,
                &message,
                ServerPosition {
                    char_offset: None,
                    line,
                },
                None,
                Dialect::MySql,
            ))
        }
        Ok(Ok(Err(_))) => ValidationOutcome::Verdict(ServerVerdict::Unavailable),
    }
}

async fn validate_sqlite(pool: &sqlx::SqlitePool, stmt: &str) -> ValidationOutcome {
    let Some(mut conn) = acquire_spare(pool).await else {
        return ValidationOutcome::Busy;
    };
    let sql = stmt.to_string();
    let work = async {
        (&mut *conn)
            .prepare(AssertSqlSafe(sql).into_sql_str())
            .await
            .map(|_| ())
    };
    match tokio::time::timeout(VALIDATE_TIMEOUT, work).await {
        Err(_) => {
            drop(conn.detach());
            ValidationOutcome::Verdict(ServerVerdict::Unavailable)
        }
        Ok(Ok(())) => ValidationOutcome::Verdict(ServerVerdict::Valid),
        Ok(Err(sqlx::Error::Database(db))) => {
            let message = db.message().to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("database is locked") || lower.contains("busy") {
                return ValidationOutcome::Verdict(ServerVerdict::Unavailable);
            }
            ValidationOutcome::Verdict(server_error(
                stmt,
                &message,
                ServerPosition::default(),
                None,
                Dialect::Sqlite,
            ))
        }
        Ok(Err(_)) => ValidationOutcome::Verdict(ServerVerdict::Unavailable),
    }
}

/// Batch MsSQL yang hanya dikompilasi. Baris pertama adalah `SET NOEXEC ON`,
/// jadi nomor baris error dikurangi satu untuk sampai ke statement.
pub(crate) fn mssql_noexec_batch(stmt: &str) -> String {
    format!("SET NOEXEC ON;\n{stmt}\n;\nSET NOEXEC OFF;")
}

async fn validate_mssql(
    pool: std::sync::Arc<mssql_driver_pool::Pool>,
    stmt: &str,
) -> ValidationOutcome {
    let upper = stmt.to_ascii_uppercase();
    if upper.contains("NOEXEC") || upper.contains("PARSEONLY") || upper.contains("FMTONLY") {
        return ValidationOutcome::Verdict(ServerVerdict::Unavailable);
    }
    let batch = mssql_noexec_batch(stmt);
    let run = crate::driver_mssql::execute_query_bounded(
        pool,
        &batch,
        1,
        Some(Duration::from_secs(3)),
        None,
    );
    match tokio::time::timeout(VALIDATE_TIMEOUT, run).await {
        Err(_) => ValidationOutcome::Busy,
        Ok(Ok(_)) => ValidationOutcome::Verdict(ServerVerdict::SyntaxValid),
        Ok(Err(crate::driver_mssql::MssqlExecError::Query(message))) => {
            let line = mssql_error_line(&message)
                .and_then(|l| l.checked_sub(1))
                .filter(|l| *l > 0);
            ValidationOutcome::Verdict(server_error(
                stmt,
                &message,
                ServerPosition {
                    char_offset: None,
                    line,
                },
                None,
                Dialect::MsSql,
            ))
        }
        Ok(Err(_)) => ValidationOutcome::Verdict(ServerVerdict::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_diagnostics::ServerVerdict;

    async fn sqlite_pool(max: u32) -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(max)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        sqlx::raw_sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO users (name) VALUES ('a')")
            .execute(&pool)
            .await
            .expect("seed");
        wait_idle(&pool).await;
        pool
    }

    /// Pengembalian koneksi ke pool berjalan async.
    async fn wait_idle(pool: &sqlx::SqlitePool) {
        for _ in 0..200 {
            if pool.num_idle() > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn sqlite_prepare_reports_unknown_columns_without_running_anything() {
        let pool = sqlite_pool(1).await;
        let ok = validate_sqlite(&pool, "SELECT id, name FROM users").await;
        assert_eq!(ok, ValidationOutcome::Verdict(ServerVerdict::Valid));

        let stmt = "SELECT id, nmae FROM users";
        wait_idle(&pool).await;
        let ValidationOutcome::Verdict(ServerVerdict::Invalid(items)) =
            validate_sqlite(&pool, stmt).await
        else {
            panic!("expected an error");
        };
        assert_eq!(&stmt[items[0].span.clone().unwrap()], "nmae");
        assert_eq!(items[0].source, DiagSource::Server);

        // Validasi DELETE tidak menghapus apa pun.
        wait_idle(&pool).await;
        let del = validate_sqlite(&pool, "DELETE FROM users").await;
        assert_eq!(del, ValidationOutcome::Verdict(ServerVerdict::Valid));
        wait_idle(&pool).await;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn busy_pool_is_not_waited_for() {
        let pool = sqlite_pool(1).await;
        let _held = pool.acquire().await.unwrap();
        assert_eq!(
            validate_sqlite(&pool, "SELECT 1").await,
            ValidationOutcome::Busy
        );
    }

    #[test]
    fn mssql_batch_line_numbers_skip_the_noexec_line() {
        let batch = mssql_noexec_batch("SELECT a\nFROM t");
        assert!(batch.starts_with("SET NOEXEC ON;\nSELECT a\n"));
        assert!(batch.trim_end().ends_with("SET NOEXEC OFF;"));
    }
}
