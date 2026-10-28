//! Tes terhadap server database sungguhan. Dijalankan job CI `drivers`
//! (service PostgreSQL dan MySQL); tanpa variabel lingkungan berikut, setiap
//! tes langsung lulus tanpa melakukan apa pun:
//!
//! - `TABULAR_TEST_PG_URL`, mis. `postgres://postgres:postgres@127.0.0.1:5432/tabular`
//! - `TABULAR_TEST_MYSQL_URL`, mis. `mysql://root:root@127.0.0.1:3306/tabular`
//!
//! Yang diuji adalah jaminan yang membuat hasil Tabular bisa dipercaya: simpan
//! grid atomik (satu baris yang tidak cocok membatalkan semuanya) dan daftar
//! tabel untuk sidebar.

use std::sync::Arc;

use crate::connection::atomic::{TransactionalStatement, execute_in_transaction};
use crate::models::enums::{DatabasePool, DatabaseType};
use crate::models::structs::ConnectionConfig;

fn env_url(name: &str) -> Option<url::Url> {
    let raw = std::env::var(name).ok()?;
    match url::Url::parse(&raw) {
        Ok(url) => Some(url),
        Err(e) => panic!("{name} is not a valid URL: {e}"),
    }
}

/// Nama tabel unik per tes agar job paralel tidak bentrok.
fn temp_table(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or_default();
    format!("tabular_{prefix}_{}_{nanos}", std::process::id())
}

fn config_from(url: &url::Url, kind: DatabaseType) -> ConnectionConfig {
    ConnectionConfig {
        host: url.host_str().unwrap_or("127.0.0.1").to_string(),
        port: url.port().map(|p| p.to_string()).unwrap_or_default(),
        username: url.username().to_string(),
        password: url.password().unwrap_or_default().to_string(),
        database: url.path().trim_start_matches('/').to_string(),
        connection_type: kind,
        ..Default::default()
    }
}

fn stmt(sql: String, expected: Option<u64>) -> TransactionalStatement {
    TransactionalStatement {
        sql,
        expected_rows: expected,
    }
}

/// Simpan grid: UPDATE kedua tidak mengenai baris mana pun, jadi UPDATE
/// pertama harus ikut dibatalkan. Lalu simpan yang valid harus berhasil.
async fn assert_grid_save_is_atomic(
    pool: &DatabasePool,
    run: impl AsyncFn(&str) -> Vec<String>,
    table: &str,
) {
    let bad = [
        stmt(
            format!("UPDATE {table} SET name = 'changed' WHERE id = 1"),
            Some(1),
        ),
        stmt(
            format!("UPDATE {table} SET name = 'ghost' WHERE id = 999"),
            Some(1),
        ),
    ];
    let err = execute_in_transaction(pool, &bad, None)
        .await
        .expect_err("a statement that matches no row must fail the whole save");
    assert!(err.to_string().contains("nothing was saved"), "{err}");
    assert_eq!(
        run(&format!("SELECT name FROM {table} WHERE id = 1")).await,
        vec!["a".to_string()],
        "the first UPDATE must have been rolled back"
    );

    let good = [
        stmt(
            format!("UPDATE {table} SET name = 'x' WHERE id = 1"),
            Some(1),
        ),
        stmt(
            format!("UPDATE {table} SET name = 'y' WHERE id = 2"),
            Some(1),
        ),
    ];
    let affected = execute_in_transaction(pool, &good, None)
        .await
        .expect("a valid save must succeed");
    assert_eq!(affected, 2);
    assert_eq!(
        run(&format!("SELECT name FROM {table} ORDER BY id")).await,
        vec!["x".to_string(), "y".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_grid_save_and_table_listing() {
    let Some(url) = env_url("TABULAR_TEST_PG_URL") else {
        return;
    };
    let pg = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(url.as_str())
        .await
        .expect("connect to TABULAR_TEST_PG_URL");
    let table = temp_table("pg");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TABLE {table} (id INTEGER PRIMARY KEY, name VARCHAR(50))"
    )))
    .execute(&pg)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} VALUES (1, 'a'), (2, 'b')"
    )))
    .execute(&pg)
    .await
    .unwrap();

    let pool = DatabasePool::PostgreSQL(Arc::new(pg.clone()));
    let reader = pg.clone();
    let run = async move |sql: &str| -> Vec<String> {
        sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.to_string()))
            .fetch_all(&reader)
            .await
            .unwrap()
    };
    assert_grid_save_is_atomic(&pool, run, &table).await;

    let config = config_from(&url, DatabaseType::PostgreSQL);
    let tables = crate::driver_postgres::list_postgres_tables(&config, &config.database, "table")
        .await
        .expect("list tables");
    assert!(tables.contains(&table), "{table} missing from {tables:?}");

    sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
        .execute(&pg)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn mysql_grid_save_and_table_listing() {
    let Some(url) = env_url("TABULAR_TEST_MYSQL_URL") else {
        return;
    };
    let my = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(2)
        .connect(url.as_str())
        .await
        .expect("connect to TABULAR_TEST_MYSQL_URL");
    let table = temp_table("my");
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "CREATE TABLE {table} (id INTEGER PRIMARY KEY, name VARCHAR(50))"
    )))
    .execute(&my)
    .await
    .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} VALUES (1, 'a'), (2, 'b')"
    )))
    .execute(&my)
    .await
    .unwrap();

    let pool = DatabasePool::MySQL(Arc::new(my.clone()));
    let reader = my.clone();
    let run = async move |sql: &str| -> Vec<String> {
        sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.to_string()))
            .fetch_all(&reader)
            .await
            .unwrap()
    };
    assert_grid_save_is_atomic(&pool, run, &table).await;

    let database = url.path().trim_start_matches('/').to_string();
    let tables = crate::driver_mysql::list_mysql_tables(&my, &database, "table")
        .await
        .expect("list tables");
    assert!(tables.contains(&table), "{table} missing from {tables:?}");

    sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
        .execute(&my)
        .await
        .unwrap();
}

/// Validasi diagnostik: statement valid diterima, kolom salah ditandai di
/// token yang benar, dan DELETE yang divalidasi tidak menghapus apa pun.
async fn assert_validation_never_executes(
    pool: &DatabasePool,
    run: impl AsyncFn(&str) -> Vec<String>,
    table: &str,
    target: &crate::connection::validate::ValidationTarget,
) {
    use crate::connection::validate::{ValidationOutcome, validate_statement};
    use crate::query_diagnostics::ServerVerdict;

    let ok = validate_statement(pool, target, &format!("SELECT id, name FROM {table}")).await;
    assert_eq!(ok, ValidationOutcome::Verdict(ServerVerdict::Valid));

    let bad = format!("SELECT id, nmae FROM {table}");
    let ValidationOutcome::Verdict(ServerVerdict::Invalid(items)) =
        validate_statement(pool, target, &bad).await
    else {
        panic!("an unknown column must be reported");
    };
    assert_eq!(&bad[items[0].span.clone().unwrap()], "nmae", "{items:?}");

    let delete = validate_statement(pool, target, &format!("DELETE FROM {table}")).await;
    assert_eq!(delete, ValidationOutcome::Verdict(ServerVerdict::Valid));
    assert_eq!(
        run(&format!("SELECT name FROM {table} WHERE id = 2")).await,
        vec!["b".to_string()],
        "validating a DELETE must not delete anything"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn postgres_and_mysql_validation_prepares_without_running() {
    if let Some(url) = env_url("TABULAR_TEST_PG_URL") {
        let pg = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(url.as_str())
            .await
            .expect("connect to TABULAR_TEST_PG_URL");
        let table = temp_table("pgv");
        for sql in [
            format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, name VARCHAR(50))"),
            format!("INSERT INTO {table} VALUES (1, 'a'), (2, 'b')"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .execute(&pg)
                .await
                .unwrap();
        }
        let reader = pg.clone();
        let run = async move |sql: &str| -> Vec<String> {
            sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.to_string()))
                .fetch_all(&reader)
                .await
                .unwrap()
        };
        let pool = DatabasePool::PostgreSQL(Arc::new(pg.clone()));
        assert_validation_never_executes(&pool, run, &table, &Default::default()).await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
            .execute(&pg)
            .await
            .unwrap();
    }
    if let Some(url) = env_url("TABULAR_TEST_MYSQL_URL") {
        let my = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(2)
            .connect(url.as_str())
            .await
            .expect("connect to TABULAR_TEST_MYSQL_URL");
        let table = temp_table("myv");
        for sql in [
            format!("CREATE TABLE {table} (id INTEGER PRIMARY KEY, name VARCHAR(50))"),
            format!("INSERT INTO {table} VALUES (1, 'a'), (2, 'b')"),
        ] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                .execute(&my)
                .await
                .unwrap();
        }
        let reader = my.clone();
        let run = async move |sql: &str| -> Vec<String> {
            sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql.to_string()))
                .fetch_all(&reader)
                .await
                .unwrap()
        };
        let pool = DatabasePool::MySQL(Arc::new(my.clone()));
        let target = crate::connection::validate::ValidationTarget {
            database: Some(url.path().trim_start_matches('/').to_string()),
            schema: None,
        };
        assert_validation_never_executes(&pool, run, &table, &target).await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
            .execute(&my)
            .await
            .unwrap();
    }
}
