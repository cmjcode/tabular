//! A ready-to-open SQLite database seeded on first launch, iOS only.
//!
//! On iOS every `rfd` file dialog is a stub that returns `None` (see the `rfd`
//! shim in `lib.rs`), so there is no way to pick a `.db` file from disk.
//! Without a bundled database a first-time user — an App Review reviewer
//! included — lands in an app that cannot do anything until they type in
//! credentials for a remote server they may not have. Guideline 2.1 rejections
//! follow from exactly that.
//!
//! Desktop builds are untouched: they have a working file picker, and silently
//! adding a connection to someone's existing sidebar would be surprising.

#[cfg(target_os = "ios")]
use log::{info, warn};

/// Name shown in the sidebar, and the key used to avoid re-seeding.
#[cfg(target_os = "ios")]
const SAMPLE_CONNECTION_NAME: &str = "Sample Database";

/// Seed the sample database and register a connection for it, once.
///
/// Cheap and idempotent: seeding is skipped as soon as a connection named
/// `SAMPLE_CONNECTION_NAME` exists, so a user who deletes it does not get it
/// back on the next launch.
#[cfg(target_os = "ios")]
pub async fn ensure_sample_database(pool: &sqlx::SqlitePool) {
    let already_registered: Option<i64> =
        sqlx::query_scalar("SELECT id FROM connections WHERE name = ? LIMIT 1")
            .bind(SAMPLE_CONNECTION_NAME)
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if already_registered.is_some() {
        return;
    }

    let sample_dir = crate::config::get_data_dir().join("sample");
    let path = match write_sample_database(&sample_dir).await {
        Ok(p) => p,
        Err(e) => {
            warn!("Could not create the sample database: {e}");
            return;
        }
    };

    let insert = sqlx::query(
        "INSERT INTO connections \
         (name, host, port, username, password, database_name, connection_type, folder) \
         VALUES (?, '', '', '', '', ?, 'SQLite', NULL)",
    )
    .bind(SAMPLE_CONNECTION_NAME)
    .bind(&path)
    .execute(pool)
    .await;

    match insert {
        Ok(_) => info!("Seeded sample database at {path}"),
        Err(e) => warn!("Could not register the sample connection: {e}"),
    }
}

/// Write the sample SQLite file into `dir`, returning its path.
///
/// Kept platform-independent so the schema above is exercised by tests. The
/// caller picks the directory; production passes the app data directory rather
/// than `Documents/` directly, so it travels with the app's own storage and is
/// covered by the `ensure_app_directories` fallback when the configured
/// directory is not writable.
pub async fn write_sample_database(dir: &std::path::Path) -> Result<String, String> {
    use std::str::FromStr;

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    let file = dir.join("tabular_sample.db");
    let path = file.to_string_lossy().to_string();

    // A leftover file from a previous install would otherwise accumulate
    // duplicate rows each time the connection row is removed and re-seeded.
    if file.exists() {
        std::fs::remove_file(&file).map_err(|e| e.to_string())?;
    }

    let options =
        sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite://{path}?mode=rwc"))
            .map_err(|e| e.to_string())?;
    let sample = sqlx::SqlitePool::connect_with(options)
        .await
        .map_err(|e| e.to_string())?;

    for statement in SCHEMA {
        sqlx::query(*statement)
            .execute(&sample)
            .await
            .map_err(|e| {
                format!(
                    "{e} (while running: {})",
                    statement.lines().next().unwrap_or("")
                )
            })?;
    }

    sample.close().await;
    Ok(path)
}

/// Small, obviously-fictional dataset: enough rows to make sorting, filtering
/// and joins meaningful in a demo, few enough to stay instant on a phone.
///
/// Not gated behind `target_os = "ios"` even though only iOS seeds it: gating
/// it would mean this SQL is never executed by any test on any platform, and a
/// typo here would first surface as a broken sidebar entry in front of an App
/// Review tester.
pub const SCHEMA: &[&str] = &[
    "CREATE TABLE customers (
        id        INTEGER PRIMARY KEY,
        name      TEXT NOT NULL,
        email     TEXT NOT NULL,
        city      TEXT NOT NULL,
        joined_on TEXT NOT NULL
    )",
    "CREATE TABLE products (
        id       INTEGER PRIMARY KEY,
        name     TEXT NOT NULL,
        category TEXT NOT NULL,
        price    REAL NOT NULL,
        stock    INTEGER NOT NULL
    )",
    "CREATE TABLE orders (
        id          INTEGER PRIMARY KEY,
        customer_id INTEGER NOT NULL REFERENCES customers(id),
        product_id  INTEGER NOT NULL REFERENCES products(id),
        quantity    INTEGER NOT NULL,
        ordered_on  TEXT NOT NULL,
        status      TEXT NOT NULL
    )",
    "INSERT INTO customers (id, name, email, city, joined_on) VALUES
        (1, 'Amelia Hartono',  'amelia@example.com',  'Jakarta',   '2024-01-15'),
        (2, 'Bagus Prakoso',   'bagus@example.com',   'Bandung',   '2024-02-03'),
        (3, 'Citra Wijaya',    'citra@example.com',   'Surabaya',  '2024-02-27'),
        (4, 'Deni Kurniawan',  'deni@example.com',    'Medan',     '2024-03-11'),
        (5, 'Eka Lestari',     'eka@example.com',     'Semarang',  '2024-04-02'),
        (6, 'Farah Nuraini',   'farah@example.com',   'Makassar',  '2024-05-19'),
        (7, 'Gilang Saputra',  'gilang@example.com',  'Denpasar',  '2024-06-08'),
        (8, 'Hana Pertiwi',    'hana@example.com',    'Yogyakarta','2024-07-21')",
    "INSERT INTO products (id, name, category, price, stock) VALUES
        (1, 'Mechanical Keyboard',      'Peripherals', 129.00, 42),
        (2, 'USB-C Hub',                'Peripherals',  49.50, 130),
        (3, '27-inch 4K Monitor',       'Displays',    389.00, 18),
        (4, 'Noise-Cancelling Headset', 'Audio',       199.99, 27),
        (5, 'Portable SSD 1TB',         'Storage',     114.75, 64),
        (6, 'Laptop Stand',             'Accessories',  39.00, 95),
        (7, 'Webcam 1080p',             'Peripherals',  74.25, 51)",
    "INSERT INTO orders (id, customer_id, product_id, quantity, ordered_on, status) VALUES
        (1, 1, 3, 1, '2024-08-01', 'shipped'),
        (2, 1, 2, 2, '2024-08-01', 'shipped'),
        (3, 2, 1, 1, '2024-08-04', 'pending'),
        (4, 3, 5, 3, '2024-08-09', 'shipped'),
        (5, 4, 4, 1, '2024-08-12', 'cancelled'),
        (6, 5, 6, 2, '2024-08-15', 'delivered'),
        (7, 6, 7, 1, '2024-08-18', 'pending'),
        (8, 2, 5, 1, '2024-08-22', 'delivered'),
        (9, 7, 1, 2, '2024-09-02', 'shipped'),
        (10, 8, 3, 1, '2024-09-07', 'pending')",
];

/// No-op everywhere except iOS — desktop has a working file picker.
#[cfg(not(target_os = "ios"))]
pub async fn ensure_sample_database(_pool: &sqlx::SqlitePool) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp directory per test — these run in parallel in one process.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("tabular_sample_{tag}_{unique}"))
    }

    /// The whole point of ungating `SCHEMA`: prove every statement executes.
    #[tokio::test]
    async fn schema_executes_and_populates() {
        let dir = temp_dir("schema");
        let path = write_sample_database(&dir)
            .await
            .expect("schema should run");

        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{path}"))
            .await
            .expect("sample db should open");

        // Literal SQL per table: sqlx 0.9 only accepts `&'static str` without
        // an explicit `AssertSqlSafe`, and there is no reason to reach for that
        // in a test over a fixed list.
        let checks: [(&str, &str, i64); 3] = [
            ("customers", "SELECT COUNT(*) FROM customers", 8),
            ("products", "SELECT COUNT(*) FROM products", 7),
            ("orders", "SELECT COUNT(*) FROM orders", 10),
        ];
        for (table, sql, expected) in checks {
            let count: i64 = sqlx::query_scalar(sql)
                .fetch_one(&pool)
                .await
                .unwrap_or_else(|e| panic!("{table} should be queryable: {e}"));
            assert_eq!(count, expected, "row count for {table}");
        }

        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reviewer's first action is a join across the sample tables, so make
    /// sure the foreign keys actually line up rather than merely parsing.
    #[tokio::test]
    async fn foreign_keys_resolve() {
        let dir = temp_dir("joins");
        let path = write_sample_database(&dir)
            .await
            .expect("schema should run");

        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{path}"))
            .await
            .expect("sample db should open");

        let orphans: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orders o
             LEFT JOIN customers c ON c.id = o.customer_id
             LEFT JOIN products  p ON p.id = o.product_id
             WHERE c.id IS NULL OR p.id IS NULL",
        )
        .fetch_one(&pool)
        .await
        .expect("join should run");

        assert_eq!(
            orphans, 0,
            "every order should point at a real customer and product"
        );

        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Seeding twice must not double the rows — `ensure_sample_database` deletes
    /// and rewrites the file rather than appending to it.
    #[tokio::test]
    async fn rewriting_does_not_duplicate_rows() {
        let dir = temp_dir("rewrite");
        write_sample_database(&dir).await.expect("first write");
        let path = write_sample_database(&dir).await.expect("second write");

        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{path}"))
            .await
            .expect("sample db should open");

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customers")
            .fetch_one(&pool)
            .await
            .expect("count should run");
        assert_eq!(count, 8, "a second write should replace, not append");

        pool.close().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
