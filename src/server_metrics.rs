//! Metrik server real-time untuk tab "Dashboard" di DBA monitor (checklist I3).
//!
//! Headless: fungsi fetch menerima `&DatabasePool`, parsing dan perhitungan
//! laju (delta counter kumulatif per detik) murni sehingga bisa diuji tanpa
//! server. Didukung PostgreSQL, MySQL/MariaDB, dan SQL Server; engine lain
//! mengembalikan error yang jelas.
//!
//! Sumber data:
//! - PostgreSQL: `pg_stat_database` (transaksi, blks_hit/read), `pg_stat_activity`,
//!   slow query dari `pg_stat_statements` bila extension terpasang.
//! - MySQL: `SHOW GLOBAL STATUS` (Questions, Threads_*, InnoDB buffer pool, Bytes_*),
//!   slow query dari `performance_schema.events_statements_summary_by_digest`.
//! - SQL Server: `sys.dm_os_performance_counters`, `sys.dm_exec_requests`,
//!   slow query dari `sys.dm_exec_query_stats`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use sqlx::Row;

use crate::models::enums::{DatabasePool, DatabaseType};

/// Jumlah sampel yang disimpan untuk grafik (5 menit pada interval 1 detik).
pub const MAX_SAMPLES: usize = 300;
/// Slow query dimuat ulang paling cepat tiap interval ini.
pub const SLOW_REFRESH: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Nilai mentah dari server. Field kumulatif naik terus sejak server start;
/// field gauge adalah nilai saat ini.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Counters {
    /// Kumulatif: transaksi (PG), Questions (MySQL), batch request (MsSQL).
    pub statements_total: Option<f64>,
    /// Gauge: koneksi klien terbuka.
    pub connections: Option<f64>,
    /// Gauge: koneksi yang sedang menjalankan query.
    pub active: Option<f64>,
    pub max_connections: Option<f64>,
    /// Kumulatif: pembacaan buffer yang terlayani cache.
    pub cache_hits: Option<f64>,
    /// Kumulatif: pembacaan yang harus ke disk.
    pub cache_misses: Option<f64>,
    /// Rasio hit buffer (persen) bila server memberikannya langsung (MsSQL).
    pub cache_hit_ratio: Option<f64>,
    /// Kumulatif: byte diterima/dikirim server (MySQL).
    pub bytes_in: Option<f64>,
    pub bytes_out: Option<f64>,
}

/// Satu titik di grafik dashboard.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MetricsSample {
    /// Detik sejak dashboard mulai mengumpulkan.
    pub t_secs: f64,
    pub statements_per_sec: Option<f64>,
    pub connections: Option<f64>,
    pub active: Option<f64>,
    /// Persen hit buffer pada interval ini.
    pub cache_hit_pct: Option<f64>,
    pub bytes_in_per_sec: Option<f64>,
    pub bytes_out_per_sec: Option<f64>,
}

/// Statement paling lambat menurut statistik server.
#[derive(Debug, Clone, PartialEq)]
pub struct SlowStatement {
    pub query: String,
    pub calls: f64,
    pub avg_ms: f64,
    pub total_ms: f64,
}

/// Hasil fetch latar belakang yang dikirim ke UI.
#[derive(Debug, Clone)]
pub enum MetricsResult {
    Counters(Result<Counters, String>),
    Slow(Result<Vec<SlowStatement>, String>),
}

/// Label laju statement per engine.
pub fn statements_label(db_type: &DatabaseType) -> &'static str {
    match db_type {
        DatabaseType::PostgreSQL => "Transactions/s",
        DatabaseType::MsSQL => "Batch requests/s",
        _ => "Queries/s",
    }
}

pub fn is_supported(db_type: &DatabaseType) -> bool {
    matches!(
        db_type,
        DatabaseType::PostgreSQL | DatabaseType::MySQL | DatabaseType::MsSQL
    )
}

fn delta_rate(prev: Option<f64>, cur: Option<f64>, secs: f64) -> Option<f64> {
    let (p, c) = (prev?, cur?);
    // Counter turun berarti server restart atau statistik di-reset.
    (secs > 0.0 && c >= p).then(|| (c - p) / secs)
}

/// Hitung satu sampel dari dua pembacaan berurutan.
pub fn compute_sample(
    prev: &Counters,
    cur: &Counters,
    elapsed_secs: f64,
    t_secs: f64,
) -> MetricsSample {
    let hit_pct = match (
        delta_rate(prev.cache_hits, cur.cache_hits, 1.0),
        delta_rate(prev.cache_misses, cur.cache_misses, 1.0),
    ) {
        (Some(h), Some(m)) if h + m > 0.0 => Some(h / (h + m) * 100.0),
        // Tidak ada pembacaan di interval ini: pakai rasio kumulatif.
        _ => cumulative_hit_pct(cur),
    };
    MetricsSample {
        t_secs,
        statements_per_sec: delta_rate(prev.statements_total, cur.statements_total, elapsed_secs),
        connections: cur.connections,
        active: cur.active,
        cache_hit_pct: cur.cache_hit_ratio.or(hit_pct),
        bytes_in_per_sec: delta_rate(prev.bytes_in, cur.bytes_in, elapsed_secs),
        bytes_out_per_sec: delta_rate(prev.bytes_out, cur.bytes_out, elapsed_secs),
    }
}

fn cumulative_hit_pct(c: &Counters) -> Option<f64> {
    let (h, m) = (c.cache_hits?, c.cache_misses?);
    (h + m > 0.0).then(|| h / (h + m) * 100.0)
}

fn num(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok()
}

/// Petakan pasangan (nama, nilai) hasil query metrik ke [`Counters`].
pub fn counters_from_pairs(db_type: &DatabaseType, pairs: &[(String, String)]) -> Counters {
    let mut c = Counters::default();
    let mut ratio: Option<f64> = None;
    let mut ratio_base: Option<f64> = None;
    for (k, v) in pairs {
        let value = num(v);
        match (db_type, k.trim().to_ascii_lowercase().as_str()) {
            (DatabaseType::MySQL, "questions") => c.statements_total = value,
            (DatabaseType::MySQL, "threads_connected") => c.connections = value,
            (DatabaseType::MySQL, "threads_running") => c.active = value,
            // read_requests mencakup pembacaan dari disk; hit = requests - reads.
            (DatabaseType::MySQL, "innodb_buffer_pool_read_requests") => c.cache_hits = value,
            (DatabaseType::MySQL, "innodb_buffer_pool_reads") => c.cache_misses = value,
            (DatabaseType::MySQL, "bytes_received") => c.bytes_in = value,
            (DatabaseType::MySQL, "bytes_sent") => c.bytes_out = value,
            (DatabaseType::MsSQL, "batch requests/sec") => c.statements_total = value,
            (DatabaseType::MsSQL, "user connections") => c.connections = value,
            (DatabaseType::MsSQL, "buffer cache hit ratio") => ratio = value,
            (DatabaseType::MsSQL, "buffer cache hit ratio base") => ratio_base = value,
            (_, "statements") => c.statements_total = value,
            (_, "connections") => c.connections = value,
            (_, "active") => c.active = value,
            (_, "max_connections") => c.max_connections = value,
            (_, "cache_hits") => c.cache_hits = value,
            (_, "cache_misses") => c.cache_misses = value,
            _ => {}
        }
    }
    if *db_type == DatabaseType::MySQL
        && let (Some(req), Some(disk)) = (c.cache_hits, c.cache_misses)
    {
        c.cache_hits = Some((req - disk).max(0.0));
    }
    if let (Some(r), Some(b)) = (ratio, ratio_base)
        && b > 0.0
    {
        c.cache_hit_ratio = Some((r / b * 100.0).clamp(0.0, 100.0));
    }
    c
}

const PG_COUNTERS_SQL: &str = "\
SELECT 'statements' AS k, CAST(COALESCE(SUM(xact_commit + xact_rollback), 0) AS TEXT) AS v FROM pg_stat_database
UNION ALL SELECT 'cache_hits', CAST(COALESCE(SUM(blks_hit), 0) AS TEXT) FROM pg_stat_database
UNION ALL SELECT 'cache_misses', CAST(COALESCE(SUM(blks_read), 0) AS TEXT) FROM pg_stat_database
UNION ALL SELECT 'connections', CAST(COUNT(*) AS TEXT) FROM pg_stat_activity WHERE backend_type = 'client backend'
UNION ALL SELECT 'active', CAST(COUNT(*) AS TEXT) FROM pg_stat_activity
    WHERE backend_type = 'client backend' AND state = 'active' AND pid <> pg_backend_pid()
UNION ALL SELECT 'max_connections', setting FROM pg_settings WHERE name = 'max_connections'";

const MYSQL_STATUS_SQL: &str = "SHOW GLOBAL STATUS WHERE Variable_name IN ('Questions', \
    'Threads_connected', 'Threads_running', 'Innodb_buffer_pool_read_requests', \
    'Innodb_buffer_pool_reads', 'Bytes_received', 'Bytes_sent')";
const MYSQL_MAX_CONN_SQL: &str = "SHOW VARIABLES LIKE 'max_connections'";

const MSSQL_COUNTERS_SQL: &str = "\
SELECT RTRIM(counter_name) AS k, CAST(cntr_value AS VARCHAR(40)) AS v
FROM sys.dm_os_performance_counters
WHERE (object_name LIKE '%SQL Statistics%' AND counter_name = 'Batch Requests/sec')
   OR (object_name LIKE '%General Statistics%' AND counter_name = 'User Connections')
   OR (object_name LIKE '%Buffer Manager%'
       AND counter_name IN ('Buffer cache hit ratio', 'Buffer cache hit ratio base'))
UNION ALL
SELECT 'active', CAST(COUNT(*) AS VARCHAR(40)) FROM sys.dm_exec_requests
WHERE session_id <> @@SPID AND session_id > 50 AND status IN ('running', 'runnable', 'suspended')
UNION ALL
SELECT 'max_connections', CAST(@@MAX_CONNECTIONS AS VARCHAR(40))";

const PG_SLOW_SQL: &str = "\
SELECT query, CAST(calls AS TEXT), CAST(mean_exec_time AS TEXT), CAST(total_exec_time AS TEXT)
FROM pg_stat_statements
WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY mean_exec_time DESC LIMIT 25";
/// PostgreSQL < 13 memakai nama kolom lama.
const PG_SLOW_SQL_LEGACY: &str = "\
SELECT query, CAST(calls AS TEXT), CAST(mean_time AS TEXT), CAST(total_time AS TEXT)
FROM pg_stat_statements
WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
ORDER BY mean_time DESC LIMIT 25";

/// Timer performance_schema dalam picodetik; dibagi 1e9 menjadi milidetik.
const MYSQL_SLOW_SQL: &str = "\
SELECT CAST(DIGEST_TEXT AS CHAR), CAST(COUNT_STAR AS CHAR),
       CAST(AVG_TIMER_WAIT / 1000000000 AS CHAR), CAST(SUM_TIMER_WAIT / 1000000000 AS CHAR)
FROM performance_schema.events_statements_summary_by_digest
WHERE DIGEST_TEXT IS NOT NULL
ORDER BY AVG_TIMER_WAIT DESC LIMIT 25";

const MSSQL_SLOW_SQL: &str = "\
SELECT TOP 25
  CAST(SUBSTRING(st.text, (qs.statement_start_offset / 2) + 1,
    ((CASE qs.statement_end_offset WHEN -1 THEN DATALENGTH(st.text)
      ELSE qs.statement_end_offset END - qs.statement_start_offset) / 2) + 1) AS NVARCHAR(4000)),
  CAST(qs.execution_count AS VARCHAR(40)),
  CAST(qs.total_elapsed_time / 1000.0 / qs.execution_count AS VARCHAR(40)),
  CAST(qs.total_elapsed_time / 1000.0 AS VARCHAR(40))
FROM sys.dm_exec_query_stats qs
CROSS APPLY sys.dm_exec_sql_text(qs.sql_handle) st
ORDER BY qs.total_elapsed_time / qs.execution_count DESC";

fn pg_cell(row: &sqlx::postgres::PgRow, idx: usize) -> String {
    row.try_get::<String, _>(idx).unwrap_or_default()
}

fn mysql_cell(row: &sqlx::mysql::MySqlRow, idx: usize) -> String {
    if let Ok(s) = row.try_get::<String, _>(idx) {
        return s;
    }
    // MySQL mengembalikan sebagian kolom SHOW sebagai VARBINARY.
    row.try_get::<Vec<u8>, _>(idx)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Jalankan query dan kembalikan semua kolom sebagai teks (timeout 5 detik).
async fn fetch_text_rows(pool: &DatabasePool, sql: &str) -> Result<Vec<Vec<String>>, String> {
    let timed_out = || format!("Timed out after {}s", FETCH_TIMEOUT.as_secs());
    match pool {
        DatabasePool::PostgreSQL(pg) => {
            let fut = sqlx::query(sqlx::AssertSqlSafe(sql.to_string())).fetch_all(&**pg);
            let rows = tokio::time::timeout(FETCH_TIMEOUT, fut)
                .await
                .map_err(|_| timed_out())?
                .map_err(|e| e.to_string())?;
            Ok(rows
                .iter()
                .map(|r| (0..r.len()).map(|i| pg_cell(r, i)).collect())
                .collect())
        }
        DatabasePool::MySQL(my) => {
            let fut = sqlx::query(sqlx::AssertSqlSafe(sql.to_string())).fetch_all(&**my);
            let rows = tokio::time::timeout(FETCH_TIMEOUT, fut)
                .await
                .map_err(|_| timed_out())?
                .map_err(|e| e.to_string())?;
            Ok(rows
                .iter()
                .map(|r| (0..r.len()).map(|i| mysql_cell(r, i)).collect())
                .collect())
        }
        DatabasePool::MsSQL(ms) => {
            let fut = crate::driver_mssql::execute_query_multi(ms.clone(), sql);
            let sets = tokio::time::timeout(FETCH_TIMEOUT, fut)
                .await
                .map_err(|_| timed_out())??;
            Ok(sets.into_iter().flat_map(|(_, rows)| rows).collect())
        }
        _ => {
            Err("Server metrics are available for PostgreSQL, MySQL/MariaDB and SQL Server".into())
        }
    }
}

fn pairs(rows: Vec<Vec<String>>) -> Vec<(String, String)> {
    rows.into_iter()
        .filter_map(|mut r| {
            if r.len() < 2 {
                return None;
            }
            let v = r.swap_remove(1);
            let k = r.swap_remove(0);
            Some((k, v))
        })
        .collect()
}

/// Baca counter server saat ini.
pub async fn fetch_counters(
    pool: &DatabasePool,
    db_type: &DatabaseType,
) -> Result<Counters, String> {
    let rows = match db_type {
        DatabaseType::PostgreSQL => fetch_text_rows(pool, PG_COUNTERS_SQL).await?,
        DatabaseType::MySQL => {
            let mut rows = fetch_text_rows(pool, MYSQL_STATUS_SQL).await?;
            match fetch_text_rows(pool, MYSQL_MAX_CONN_SQL).await {
                Ok(extra) => rows.extend(extra),
                Err(e) => log::debug!("[DBA-METRICS] max_connections unavailable: {e}"),
            }
            rows
        }
        DatabaseType::MsSQL => fetch_text_rows(pool, MSSQL_COUNTERS_SQL).await?,
        _ => return Err("Server metrics are not available for this engine".into()),
    };
    Ok(counters_from_pairs(db_type, &pairs(rows)))
}

/// Petakan baris (query, calls, avg_ms, total_ms) ke [`SlowStatement`].
pub fn slow_from_rows(rows: Vec<Vec<String>>) -> Vec<SlowStatement> {
    rows.into_iter()
        .filter(|r| r.len() >= 4)
        .map(|r| SlowStatement {
            query: r[0].split_whitespace().collect::<Vec<_>>().join(" "),
            calls: num(&r[1]).unwrap_or(0.0),
            avg_ms: num(&r[2]).unwrap_or(0.0),
            total_ms: num(&r[3]).unwrap_or(0.0),
        })
        .collect()
}

/// Statement paling lambat (rata-rata) menurut statistik server.
pub async fn fetch_slow_statements(
    pool: &DatabasePool,
    db_type: &DatabaseType,
) -> Result<Vec<SlowStatement>, String> {
    let rows = match db_type {
        DatabaseType::PostgreSQL => match fetch_text_rows(pool, PG_SLOW_SQL).await {
            Ok(rows) => rows,
            Err(e) if e.contains("mean_exec_time") => {
                fetch_text_rows(pool, PG_SLOW_SQL_LEGACY).await?
            }
            Err(e) if e.contains("pg_stat_statements") => {
                return Err(
                    "pg_stat_statements is not installed. Run CREATE EXTENSION pg_stat_statements \
                     (it must also be listed in shared_preload_libraries)."
                        .into(),
                );
            }
            Err(e) => return Err(e),
        },
        DatabaseType::MySQL => fetch_text_rows(pool, MYSQL_SLOW_SQL)
            .await
            .map_err(|e| format!("performance_schema statement digests are unavailable: {e}"))?,
        DatabaseType::MsSQL => fetch_text_rows(pool, MSSQL_SLOW_SQL).await?,
        _ => return Err("Slow statement statistics are not available for this engine".into()),
    };
    Ok(slow_from_rows(rows))
}

/// State dashboard per tab DBA monitor.
#[derive(Debug, Clone, Default)]
pub struct DashboardState {
    pub started: Option<Instant>,
    /// Pembacaan terakhir untuk menghitung delta.
    pub last: Option<(Instant, Counters)>,
    pub samples: VecDeque<MetricsSample>,
    pub error: Option<String>,
    pub counters_loading: bool,
    pub slow: Vec<SlowStatement>,
    pub slow_error: Option<String>,
    pub slow_loading: bool,
    pub slow_loaded_at: Option<Instant>,
}

impl DashboardState {
    /// Terima pembacaan baru pada waktu `now`; sampel pertama hanya menjadi
    /// titik acuan untuk laju.
    pub fn push_counters(&mut self, now: Instant, counters: Counters) {
        let started = *self.started.get_or_insert(now);
        if let Some((prev_at, prev)) = &self.last {
            let elapsed = now.saturating_duration_since(*prev_at).as_secs_f64();
            let t = now.saturating_duration_since(started).as_secs_f64();
            self.samples
                .push_back(compute_sample(prev, &counters, elapsed, t));
            while self.samples.len() > MAX_SAMPLES {
                self.samples.pop_front();
            }
        }
        self.last = Some((now, counters));
        self.error = None;
    }

    pub fn latest_counters(&self) -> Option<&Counters> {
        self.last.as_ref().map(|(_, c)| c)
    }

    pub fn slow_due(&self, now: Instant) -> bool {
        !self.slow_loading
            && self
                .slow_loaded_at
                .is_none_or(|at| now.saturating_duration_since(at) >= SLOW_REFRESH)
    }
}

/// Format byte per detik: `12.3 KB/s`.
pub fn format_rate_bytes(bps: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = bps.max(0.0);
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn counter_mysql_dari_show_global_status() {
        let c = counters_from_pairs(
            &DatabaseType::MySQL,
            &kv(&[
                ("Questions", "1000"),
                ("Threads_connected", "12"),
                ("Threads_running", "3"),
                ("Innodb_buffer_pool_read_requests", "900"),
                ("Innodb_buffer_pool_reads", "100"),
                ("Bytes_received", "2048"),
                ("max_connections", "151"),
            ]),
        );
        assert_eq!(c.statements_total, Some(1000.0));
        assert_eq!(c.connections, Some(12.0));
        assert_eq!(c.active, Some(3.0));
        assert_eq!(c.max_connections, Some(151.0));
        assert_eq!(c.cache_hits, Some(800.0));
        assert_eq!(c.cache_misses, Some(100.0));
        assert_eq!(c.bytes_in, Some(2048.0));
    }

    #[test]
    fn counter_mssql_menghitung_rasio_dari_base() {
        let c = counters_from_pairs(
            &DatabaseType::MsSQL,
            &kv(&[
                ("Batch Requests/sec", "5000"),
                ("User Connections", "20"),
                ("Buffer cache hit ratio", "95"),
                ("Buffer cache hit ratio base", "100"),
                ("active", "2"),
                ("max_connections", "32767"),
            ]),
        );
        assert_eq!(c.statements_total, Some(5000.0));
        assert_eq!(c.connections, Some(20.0));
        assert_eq!(c.cache_hit_ratio, Some(95.0));
        assert_eq!(c.active, Some(2.0));
        assert_eq!(c.max_connections, Some(32767.0));
    }

    #[test]
    fn sampel_menghitung_laju_dan_hit_ratio() {
        let prev = Counters {
            statements_total: Some(100.0),
            cache_hits: Some(1000.0),
            cache_misses: Some(0.0),
            bytes_out: Some(0.0),
            ..Default::default()
        };
        let cur = Counters {
            statements_total: Some(300.0),
            cache_hits: Some(1090.0),
            cache_misses: Some(10.0),
            bytes_out: Some(4096.0),
            connections: Some(5.0),
            ..Default::default()
        };
        let s = compute_sample(&prev, &cur, 2.0, 10.0);
        assert_eq!(s.statements_per_sec, Some(100.0));
        assert_eq!(s.cache_hit_pct, Some(90.0));
        assert_eq!(s.bytes_out_per_sec, Some(2048.0));
        assert_eq!(s.connections, Some(5.0));
        assert_eq!(s.t_secs, 10.0);
    }

    #[test]
    fn counter_turun_dianggap_reset() {
        let prev = Counters {
            statements_total: Some(500.0),
            ..Default::default()
        };
        let cur = Counters {
            statements_total: Some(10.0),
            ..Default::default()
        };
        assert_eq!(
            compute_sample(&prev, &cur, 1.0, 1.0).statements_per_sec,
            None
        );
    }

    #[test]
    fn dashboard_menyimpan_sampel_terbatas() {
        let mut d = DashboardState::default();
        let t0 = Instant::now();
        for i in 0..(MAX_SAMPLES + 10) {
            d.push_counters(
                t0 + Duration::from_secs(i as u64),
                Counters {
                    statements_total: Some(i as f64 * 10.0),
                    ..Default::default()
                },
            );
        }
        assert_eq!(d.samples.len(), MAX_SAMPLES);
        assert_eq!(d.samples.back().unwrap().statements_per_sec, Some(10.0));
        assert!(d.slow_due(t0));
    }

    #[test]
    fn slow_dari_baris_teks() {
        let rows = vec![vec![
            "SELECT *\n  FROM t".to_string(),
            "12".into(),
            "3.5".into(),
            "42".into(),
        ]];
        let s = slow_from_rows(rows);
        assert_eq!(s[0].query, "SELECT * FROM t");
        assert_eq!(s[0].calls, 12.0);
        assert_eq!(s[0].avg_ms, 3.5);
    }

    #[test]
    fn format_laju_byte() {
        assert_eq!(format_rate_bytes(512.0), "512 B/s");
        assert_eq!(format_rate_bytes(2048.0), "2.0 KB/s");
    }
}
