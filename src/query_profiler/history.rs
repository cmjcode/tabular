//! Riwayat plan EXPLAIN per query di `connections.db` (checklist C3).
//!
//! Headless: semua fungsi menerima `&SqlitePool`. Setiap fingerprint menyimpan maksimal
//! `MAX_UNPINNED_PER_QUERY` plan tidak di-pin; plan yang di-pin tidak pernah dipangkas.

use sqlx::{Row, SqlitePool};

/// Batas plan tidak di-pin per (koneksi, fingerprint).
pub const MAX_UNPINNED_PER_QUERY: i64 = 20;

/// Satu plan tersimpan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanSnapshot {
    pub id: i64,
    pub connection_id: i64,
    pub query_hash: String,
    pub raw_plan: String,
    pub captured_at: String,
    pub pinned: bool,
    pub total_cost: f64,
    pub duration_ms: Option<f64>,
}

/// Membuat tabel bila belum ada. Aman dipanggil berulang.
pub async fn ensure_table(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS explain_plan_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            connection_id INTEGER NOT NULL,
            query_hash TEXT NOT NULL,
            query_text TEXT NOT NULL,
            raw_plan TEXT NOT NULL,
            total_cost REAL NOT NULL DEFAULT 0,
            duration_ms REAL NULL,
            pinned INTEGER NOT NULL DEFAULT 0,
            captured_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_explain_plan_history_key
         ON explain_plan_history (connection_id, query_hash, captured_at)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Menyimpan plan baru. Plan yang identik dengan plan terakhir untuk query yang sama
/// tidak diduplikasi; id plan terakhir dikembalikan.
pub async fn record_plan(
    pool: &SqlitePool,
    connection_id: i64,
    query_text: &str,
    raw_plan: &str,
) -> Result<i64, sqlx::Error> {
    ensure_table(pool).await?;
    let query_hash = super::compare::query_fingerprint(query_text);
    let (total_cost, duration_ms) = match super::parse_explain(raw_plan) {
        Some((_, summary)) => (
            summary.total_cost,
            (summary.total_duration_ms > 0.0).then_some(summary.total_duration_ms),
        ),
        None => (0.0, None),
    };

    let last = sqlx::query(
        "SELECT id, raw_plan FROM explain_plan_history
         WHERE connection_id = ? AND query_hash = ?
         ORDER BY id DESC LIMIT 1",
    )
    .bind(connection_id)
    .bind(&query_hash)
    .fetch_optional(pool)
    .await?;
    if let Some(row) = last {
        let last_raw: String = row.try_get("raw_plan")?;
        if last_raw == raw_plan {
            return row.try_get("id");
        }
    }

    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let id = sqlx::query(
        "INSERT INTO explain_plan_history
            (connection_id, query_hash, query_text, raw_plan, total_cost, duration_ms, captured_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(connection_id)
    .bind(&query_hash)
    .bind(query_text)
    .bind(raw_plan)
    .bind(total_cost)
    .bind(duration_ms)
    .bind(&now)
    .execute(pool)
    .await?
    .last_insert_rowid();

    sqlx::query(
        "DELETE FROM explain_plan_history
         WHERE connection_id = ? AND query_hash = ? AND pinned = 0 AND id NOT IN (
            SELECT id FROM explain_plan_history
            WHERE connection_id = ? AND query_hash = ? AND pinned = 0
            ORDER BY id DESC LIMIT ?
         )",
    )
    .bind(connection_id)
    .bind(&query_hash)
    .bind(connection_id)
    .bind(&query_hash)
    .bind(MAX_UNPINNED_PER_QUERY)
    .execute(pool)
    .await?;

    Ok(id)
}

/// Daftar plan untuk satu query, terbaru dulu.
pub async fn list_plans(
    pool: &SqlitePool,
    connection_id: i64,
    query_hash: &str,
) -> Result<Vec<PlanSnapshot>, sqlx::Error> {
    ensure_table(pool).await?;
    let rows = sqlx::query(
        "SELECT id, connection_id, query_hash, raw_plan, captured_at, pinned, total_cost, duration_ms
         FROM explain_plan_history
         WHERE connection_id = ? AND query_hash = ?
         ORDER BY id DESC",
    )
    .bind(connection_id)
    .bind(query_hash)
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|r| {
            Ok(PlanSnapshot {
                id: r.try_get("id")?,
                connection_id: r.try_get("connection_id")?,
                query_hash: r.try_get("query_hash")?,
                raw_plan: r.try_get("raw_plan")?,
                captured_at: r.try_get("captured_at")?,
                pinned: r.try_get::<i64, _>("pinned")? != 0,
                total_cost: r.try_get("total_cost")?,
                duration_ms: r.try_get("duration_ms")?,
            })
        })
        .collect()
}

/// Mengubah status pin sebuah plan.
pub async fn set_pinned(pool: &SqlitePool, id: i64, pinned: bool) -> Result<(), sqlx::Error> {
    ensure_table(pool).await?;
    sqlx::query("UPDATE explain_plan_history SET pinned = ? WHERE id = ?")
        .bind(pinned as i64)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Cache riwayat plan untuk query yang sedang ditampilkan, dipegang oleh aplikasi.
#[derive(Debug, Clone, Default)]
pub struct ExplainHistoryCache {
    /// (connection_id, query_hash) yang isinya sedang di-cache.
    pub key: Option<(i64, String)>,
    pub snapshots: Vec<PlanSnapshot>,
}

impl ExplainHistoryCache {
    /// Id snapshot terbaru yang isinya sama dengan plan yang sedang ditampilkan.
    pub fn current_id(&self, raw_plan: &str) -> Option<i64> {
        self.snapshots
            .iter()
            .find(|s| s.raw_plan == raw_plan)
            .map(|s| s.id)
    }

    /// Memaksa muat ulang pada render berikutnya.
    pub fn invalidate(&mut self) {
        self.key = None;
    }
}

/// Memilih baseline default: plan yang di-pin (terbaru), selain itu plan sebelum `current_id`.
pub fn default_baseline(snapshots: &[PlanSnapshot], current_id: Option<i64>) -> Option<i64> {
    let others = snapshots.iter().filter(|s| Some(s.id) != current_id);
    let mut pinned = others.clone().filter(|s| s.pinned);
    if let Some(p) = pinned.next() {
        return Some(p.id);
    }
    others.map(|s| s.id).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        // Satu koneksi: setiap koneksi `:memory:` adalah database terpisah.
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    const PLAN_A: &str = r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"t","Total Cost":100.0,"Plan Rows":10}}]"#;
    const PLAN_B: &str = r#"[{"Plan":{"Node Type":"Index Scan","Relation Name":"t","Total Cost":8.0,"Plan Rows":10}}]"#;

    #[tokio::test]
    async fn record_list_pin_dan_dedup() {
        let p = pool().await;
        let a = record_plan(&p, 1, "EXPLAIN (FORMAT JSON) SELECT * FROM t", PLAN_A)
            .await
            .unwrap();
        // Plan identik berturut-turut tidak diduplikasi.
        let a2 = record_plan(&p, 1, "explain select * from t", PLAN_A)
            .await
            .unwrap();
        assert_eq!(a, a2);
        let b = record_plan(&p, 1, "EXPLAIN SELECT * FROM t", PLAN_B)
            .await
            .unwrap();
        assert_ne!(a, b);

        let hash = crate::query_profiler::compare::query_fingerprint("select * from t");
        let list = list_plans(&p, 1, &hash).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, b);
        assert_eq!(list[0].total_cost, 8.0);
        assert_eq!(default_baseline(&list, Some(b)), Some(a));

        set_pinned(&p, a, true).await.unwrap();
        let list = list_plans(&p, 1, &hash).await.unwrap();
        assert!(list.iter().any(|s| s.id == a && s.pinned));
        // Koneksi lain terpisah.
        assert!(list_plans(&p, 2, &hash).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn plan_unpinned_dipangkas_tapi_pin_dipertahankan() {
        let p = pool().await;
        let first = record_plan(&p, 1, "select 1", "plan-0").await.unwrap();
        set_pinned(&p, first, true).await.unwrap();
        for i in 1..=(MAX_UNPINNED_PER_QUERY + 5) {
            record_plan(&p, 1, "select 1", &format!("plan-{i}"))
                .await
                .unwrap();
        }
        let hash = crate::query_profiler::compare::query_fingerprint("select 1");
        let list = list_plans(&p, 1, &hash).await.unwrap();
        assert_eq!(list.len() as i64, MAX_UNPINNED_PER_QUERY + 1);
        assert!(list.iter().any(|s| s.id == first));
    }
}
