//! Eksekusi sekumpulan statement DML dalam satu transaksi. Dipakai simpan
//! grid: semua berhasil atau tidak ada yang tersimpan, dan statement yang
//! diharapkan mengenai tepat satu baris (UPDATE/DELETE berkunci) menggagalkan
//! seluruh transaksi bila jumlah barisnya berbeda. Headless: hanya butuh pool.

use std::time::Duration;

use super::types::QueryExecutionError;
use crate::models::enums::DatabasePool;

/// Satu statement beserta jumlah baris yang harus terkena. `None` = tidak
/// diperiksa (INSERT).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionalStatement {
    pub sql: String,
    pub expected_rows: Option<u64>,
}

/// Pool yang bisa menjalankan [`execute_in_transaction`]. Plugin, Redis, dan
/// MongoDB tidak punya transaksi SQL di sini.
pub fn supports_transactions(pool: &DatabasePool) -> bool {
    matches!(
        pool,
        DatabasePool::MySQL(_)
            | DatabasePool::PostgreSQL(_)
            | DatabasePool::SQLite(_)
            | DatabasePool::MsSQL(_)
    )
}

/// Pesan error untuk statement ke-`index` yang gagal; transaksi sudah
/// di-rollback saat pesan ini dibuat.
fn statement_failed(index: usize, sql: &str, error: impl std::fmt::Display) -> QueryExecutionError {
    QueryExecutionError::Message(format!(
        "Statement {} failed, nothing was saved: {}\n{}",
        index + 1,
        error,
        sql
    ))
}

/// Periksa jumlah baris yang terkena terhadap harapan statement.
fn check_expected(
    index: usize,
    statement: &TransactionalStatement,
    affected: u64,
) -> Result<(), QueryExecutionError> {
    match statement.expected_rows {
        Some(expected) if affected != expected => Err(QueryExecutionError::Message(format!(
            "Statement {} matched {} row(s) instead of {}; nothing was saved. Check the key columns used in WHERE.\n{}",
            index + 1,
            affected,
            expected,
            statement.sql
        ))),
        _ => Ok(()),
    }
}

/// Transaksi sqlx: BEGIN, setiap statement, COMMIT. Error apa pun (termasuk
/// jumlah baris tak sesuai) me-rollback dan mengembalikan error-nya.
macro_rules! run_sqlx_transaction {
    ($pool:expr, $statements:expr, $label:literal) => {{
        let mut tx = $pool.begin().await.map_err(|e| {
            QueryExecutionError::from_sqlx_with_context(concat!($label, " connection error: "), e)
        })?;
        let mut total: u64 = 0;
        for (index, statement) in $statements.iter().enumerate() {
            let affected = match sqlx::query(sqlx::AssertSqlSafe(statement.sql.as_str()))
                .execute(&mut *tx)
                .await
            {
                Ok(result) => result.rows_affected(),
                Err(e) => {
                    let _ = tx.rollback().await;
                    return Err(statement_failed(index, &statement.sql, e));
                }
            };
            if let Err(e) = check_expected(index, statement, affected) {
                let _ = tx.rollback().await;
                return Err(e);
            }
            total += affected;
        }
        tx.commit().await.map_err(|e| {
            QueryExecutionError::from_sqlx_with_context(concat!($label, " commit error: "), e)
        })?;
        Ok(total)
    }};
}

async fn run_mssql_transaction(
    pool: &mssql_driver_pool::Pool,
    statements: &[TransactionalStatement],
) -> Result<u64, QueryExecutionError> {
    let mut conn = pool.get().await.map_err(|e| {
        QueryExecutionError::Connection(format!("SQL Server connection error: {}", e))
    })?;
    let client = conn.client_mut().ok_or_else(|| {
        QueryExecutionError::Connection("MsSQL pooled connection unavailable".to_string())
    })?;
    client
        .simple_query("BEGIN TRANSACTION")
        .await
        .map_err(|e| QueryExecutionError::Message(format!("BEGIN TRANSACTION failed: {}", e)))?;
    let mut total: u64 = 0;
    for (index, statement) in statements.iter().enumerate() {
        let outcome = match client.execute(&statement.sql, &[]).await {
            Ok(affected) => check_expected(index, statement, affected).map(|_| affected),
            Err(e) => Err(statement_failed(index, &statement.sql, e)),
        };
        match outcome {
            Ok(affected) => total += affected,
            Err(e) => {
                if let Err(rollback_err) = client.simple_query("ROLLBACK").await {
                    log::warn!(
                        "[GRID] ROLLBACK gagal setelah error simpan: {}",
                        rollback_err
                    );
                }
                return Err(e);
            }
        }
    }
    client
        .simple_query("COMMIT")
        .await
        .map_err(|e| QueryExecutionError::Message(format!("COMMIT failed: {}", e)))?;
    Ok(total)
}

/// Jalankan `statements` dalam satu transaksi di `pool`; mengembalikan total
/// baris yang terkena. Melewati `timeout` menggagalkan dengan
/// [`QueryExecutionError::Timeout`]; transaksi sqlx yang di-drop otomatis
/// di-rollback saat koneksinya kembali ke pool.
pub async fn execute_in_transaction(
    pool: &DatabasePool,
    statements: &[TransactionalStatement],
    timeout: Option<Duration>,
) -> Result<u64, QueryExecutionError> {
    let run = async {
        match pool {
            DatabasePool::MySQL(p) => run_sqlx_transaction!(p, statements, "MySQL"),
            DatabasePool::PostgreSQL(p) => run_sqlx_transaction!(p, statements, "PostgreSQL"),
            DatabasePool::SQLite(p) => run_sqlx_transaction!(p, statements, "SQLite"),
            DatabasePool::MsSQL(p) => run_mssql_transaction(p, statements).await,
            _ => Err(QueryExecutionError::Message(
                "Transactional save is not supported for this connection type".to_string(),
            )),
        }
    };
    match timeout {
        Some(limit) => tokio::time::timeout(limit, run)
            .await
            .map_err(|_| QueryExecutionError::Timeout)?,
        None => run.await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn memory_pool() -> DatabasePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("pool sqlite memori");
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'b')")
            .execute(&pool)
            .await
            .unwrap();
        DatabasePool::SQLite(Arc::new(pool))
    }

    async fn names(pool: &DatabasePool) -> Vec<String> {
        let DatabasePool::SQLite(p) = pool else {
            unreachable!()
        };
        sqlx::query_scalar::<_, String>("SELECT name FROM t ORDER BY id")
            .fetch_all(p.as_ref())
            .await
            .unwrap()
    }

    fn stmt(sql: &str, expected: Option<u64>) -> TransactionalStatement {
        TransactionalStatement {
            sql: sql.to_string(),
            expected_rows: expected,
        }
    }

    #[tokio::test]
    async fn semua_tersimpan_bila_sukses() {
        let pool = memory_pool().await;
        let total = execute_in_transaction(
            &pool,
            &[
                stmt("UPDATE t SET name = 'x' WHERE id = 1", Some(1)),
                stmt("DELETE FROM t WHERE id = 2", Some(1)),
                stmt("INSERT INTO t (name) VALUES ('new')", None),
            ],
            None,
        )
        .await
        .unwrap();
        assert_eq!(total, 3);
        assert_eq!(names(&pool).await, ["x", "b", "new"]);
    }

    #[tokio::test]
    async fn jumlah_baris_salah_membatalkan_semua() {
        let pool = memory_pool().await;
        let err = execute_in_transaction(
            &pool,
            &[
                stmt("UPDATE t SET name = 'x' WHERE id = 1", Some(1)),
                // WHERE tanpa kunci mengenai dua baris: harus rollback,
                // termasuk UPDATE pertama yang sudah berhasil.
                stmt("DELETE FROM t WHERE name = 'b'", Some(1)),
            ],
            None,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("matched 2 row(s)"), "{err}");
        assert_eq!(names(&pool).await, ["a", "b", "b"]);
    }

    #[tokio::test]
    async fn error_sql_membatalkan_semua() {
        let pool = memory_pool().await;
        let err = execute_in_transaction(
            &pool,
            &[
                stmt("UPDATE t SET name = 'x' WHERE id = 1", Some(1)),
                stmt("UPDATE nope SET name = 'x'", Some(1)),
            ],
            None,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().starts_with("Statement 2 failed"), "{err}");
        assert_eq!(names(&pool).await, ["a", "b", "b"]);
    }
}
