use crate::{driver_mssql, driver_mysql, driver_sqlite, models, window_egui::Tabular};
use log::debug;
use sqlx::mysql::MySqlConnection;
use sqlx::pool::PoolConnection;
use sqlx::{Column, Row, TypeInfo};
use std::time::Instant;

use super::sql::{
    infer_column_origins, infer_select_headers, is_comment_only_statement,
    is_simple_select_statement, query_contains_pagination, split_sql_statements,
    starts_with_ascii_ci, statement_returns_rows, strip_leading_sql_comments,
};
use super::types::{
    BackendPidGuard, QueryExecutionError, QueryExecutionOptions, QueryJob, QueryJobOutput,
    QueryPreparationError, QueryResultMessage,
};
use futures_util::TryStreamExt;

/// Membaca result set lewat stream dan berhenti setelah `$max` baris.
/// Menghasilkan `Result<(Vec<Row>, bool /* terpotong */), sqlx::Error>`.
macro_rules! fetch_rows_limited {
    ($query:expr, $executor:expr, $max:expr) => {
        async {
            super::timing::mark_statement_start();
            let mut stream = $query.fetch($executor);
            let mut rows = Vec::new();
            let mut truncated = false;
            while let Some(row) = stream.try_next().await? {
                if rows.is_empty() {
                    super::timing::mark_first_row();
                }
                if rows.len() >= $max {
                    truncated = true;
                    break;
                }
                rows.push(row);
            }
            super::timing::mark_fetch_end();
            Ok::<_, sqlx::Error>((rows, truncated))
        }
    };
}
// Dipakai juga oleh `session.rs` (mode manual-commit).
pub(crate) use fetch_rows_limited;

/// Berapa kali mencoba mendapatkan koneksi pool yang benar-benar hidup. Pool
/// tidak mengetes koneksi idle saat `acquire`, jadi koneksi yang sudah diputus
/// server (idle timeout, restart) baru ketahuan pada statement pertama.
const LIVE_CONNECTION_ATTEMPTS: usize = 3;

/// Koneksi yang dipakai bersama oleh semua statement dalam satu batch.
///
/// Editor memecah script menjadi satu job per statement. Tanpa ini setiap job
/// mengambil koneksi pool sendiri, sehingga `BEGIN; UPDATE …; ROLLBACK;` jatuh
/// di tiga koneksi berbeda: `BEGIN` langsung di-rollback saat koneksinya
/// kembali ke pool, `UPDATE` ter-commit sendiri, dan `SET`/`USE`/tabel temporer
/// tidak terlihat oleh statement berikutnya.
///
/// Koneksi hanya disimpan kembali setelah statement sukses. Saat gagal atau
/// timeout koneksi dilepas (hook `after_release` me-reset sesinya) dan sisa
/// batch memang tidak dijalankan.
#[derive(Default)]
pub(crate) struct BatchConnection {
    /// `connection_id` pemilik koneksi yang sedang dipegang.
    owner: Option<i64>,
    postgres: Option<PoolConnection<sqlx::Postgres>>,
    mysql: Option<PoolConnection<sqlx::MySql>>,
    sqlite: Option<PoolConnection<sqlx::Sqlite>>,
}

impl BatchConnection {
    /// Lepas semua koneksi bila job berikutnya milik koneksi lain.
    fn claim(&mut self, connection_id: i64) {
        if self.owner != Some(connection_id) {
            *self = Self {
                owner: Some(connection_id),
                ..Self::default()
            };
        }
    }
}

/// Ambil koneksi PostgreSQL yang terbukti hidup beserta backend pid-nya.
/// Koneksi idle yang ternyata sudah putus dibuang dan diganti, selama belum ada
/// statement user yang dijalankan (jadi aman diulang).
async fn acquire_live_postgres(
    pool: &sqlx::PgPool,
) -> Result<(PoolConnection<sqlx::Postgres>, Option<i32>), QueryExecutionError> {
    let mut last_error = String::new();
    for _ in 0..LIVE_CONNECTION_ATTEMPTS {
        let mut conn = pool.acquire().await.map_err(|e| {
            QueryExecutionError::from_sqlx_with_context("PostgreSQL connection error: ", e)
        })?;
        match sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
            .fetch_one(&mut *conn)
            .await
        {
            Ok(pid) => return Ok((conn, Some(pid))),
            Err(e) if super::types::is_connection_class(&e) => {
                log::warn!("[EXEC] Discarding dead PostgreSQL connection: {}", e);
                last_error = e.to_string();
                drop(conn.detach());
            }
            // Koneksinya hidup; hanya pid yang tidak terbaca (cancel jadi tidak
            // tersedia, query tetap bisa jalan).
            Err(e) => {
                log::warn!("[EXEC] Cannot read PostgreSQL backend pid: {}", e);
                return Ok((conn, None));
            }
        }
    }
    Err(QueryExecutionError::Connection(format!(
        "PostgreSQL connection error: {}",
        last_error
    )))
}

/// Ambil koneksi MySQL yang terbukti hidup beserta connection id-nya. Lihat
/// [`acquire_live_postgres`].
async fn acquire_live_mysql(
    pool: &sqlx::MySqlPool,
) -> Result<(PoolConnection<sqlx::MySql>, Option<u64>), QueryExecutionError> {
    let mut last_error = String::new();
    for _ in 0..LIVE_CONNECTION_ATTEMPTS {
        let mut conn = pool.acquire().await.map_err(|e| {
            QueryExecutionError::from_sqlx_with_context("MySQL connection error: ", e)
        })?;
        match sqlx::query_scalar::<_, u64>("SELECT CONNECTION_ID()")
            .fetch_one(&mut *conn)
            .await
        {
            Ok(pid) => return Ok((conn, Some(pid))),
            Err(e) if super::types::is_connection_class(&e) => {
                log::warn!("[EXEC] Discarding dead MySQL connection: {}", e);
                last_error = e.to_string();
                drop(conn.detach());
            }
            Err(e) => {
                log::warn!("[EXEC] Cannot read MySQL connection id: {}", e);
                return Ok((conn, None));
            }
        }
    }
    Err(QueryExecutionError::Connection(format!(
        "MySQL connection error: {}",
        last_error
    )))
}

/// Kode error MySQL (mis. 1295) dari error sqlx, bila berasal dari server.
fn mysql_error_number(e: &sqlx::Error) -> Option<u16> {
    match e {
        sqlx::Error::Database(db) => db
            .try_downcast_ref::<sqlx::mysql::MySqlDatabaseError>()
            .map(|mysql| mysql.number()),
        _ => None,
    }
}

/// ER_UNSUPPORTED_PS: statement tidak didukung protokol prepared statement
/// (`USE`, `BEGIN`, `PURGE BINARY LOGS`, `FLUSH`, …).
const MYSQL_ER_UNSUPPORTED_PS: u16 = 1295;

/// Baris, penanda terpotong, dan jumlah baris terdampak satu statement MySQL.
type MySqlStatementResult = (Vec<sqlx::mysql::MySqlRow>, bool, Option<u64>);

/// Jalankan satu statement MySQL. Server menolak sebagian statement di protokol
/// prepared statement dengan error 1295 pada tahap PREPARE — sebelum apa pun
/// dieksekusi — jadi statement itu aman diulang lewat protokol teks.
pub(super) async fn run_mysql_statement(
    conn: &mut MySqlConnection,
    sql: &str,
    returns_rows: bool,
    max_rows: usize,
) -> Result<MySqlStatementResult, sqlx::Error> {
    let prepared = if returns_rows {
        fetch_rows_limited!(sqlx::query(sqlx::AssertSqlSafe(sql)), &mut *conn, max_rows)
            .await
            .map(|(rows, truncated)| (rows, truncated, None))
    } else {
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&mut *conn)
            .await
            .map(|r| (Vec::new(), false, Some(r.rows_affected())))
    };
    match prepared {
        Err(e) if mysql_error_number(&e) == Some(MYSQL_ER_UNSUPPORTED_PS) => {
            if returns_rows {
                fetch_rows_limited!(
                    sqlx::raw_sql(sqlx::AssertSqlSafe(sql)),
                    &mut *conn,
                    max_rows
                )
                .await
                .map(|(rows, truncated)| (rows, truncated, None))
            } else {
                sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
                    .execute(&mut *conn)
                    .await
                    .map(|r| (Vec::new(), false, Some(r.rows_affected())))
            }
        }
        other => other,
    }
}

/// Nama database dari statement `USE db`, atau `None` bila bukan statement USE.
fn parse_use_statement(statement: &str) -> Option<String> {
    let body = strip_leading_sql_comments(statement);
    if !starts_with_ascii_ci(body, "use") {
        return None;
    }
    let rest = body.get(3..)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let name = rest
        .trim()
        .trim_end_matches(';')
        .trim()
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('[')
        .trim_matches(']')
        .trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Pegangan untuk menghentikan statement SQLite yang sedang berjalan lewat
/// `sqlite3_interrupt`. Men-drop future sqlx saja tidak menghentikan apa pun:
/// thread worker sqlx terus menjalankan statement sampai selesai, dan koneksi
/// (beserta lock-nya) tertahan selama itu.
#[derive(Clone, Copy)]
pub(super) struct SqliteInterruptHandle {
    raw: std::ptr::NonNull<libsqlite3_sys::sqlite3>,
}

// SAFETY: pointer ini hanya pernah diberikan ke `sqlite3_interrupt`, satu-satunya
// fungsi SQLite yang didokumentasikan boleh dipanggil dari thread mana pun saat
// thread lain sedang memakai koneksi yang sama. Pointer tidak pernah
// di-dereference di sisi Rust.
unsafe impl Send for SqliteInterruptHandle {}

impl SqliteInterruptHandle {
    /// Ambil handle mentah koneksi. `None` bila worker sqlx sudah mati.
    pub(super) async fn for_connection(conn: &mut sqlx::SqliteConnection) -> Option<Self> {
        match conn.lock_handle().await {
            Ok(mut locked) => Some(Self {
                raw: locked.as_raw_handle(),
            }),
            Err(e) => {
                log::warn!("[EXEC] Cannot obtain the SQLite handle for interrupts: {}", e);
                None
            }
        }
    }

    /// Minta SQLite membatalkan statement yang sedang berjalan.
    ///
    /// # Safety
    ///
    /// Koneksi asal handle ini harus masih terbuka. `sqlite3_interrupt` pada
    /// koneksi yang sudah (atau sedang) ditutup adalah undefined behavior.
    unsafe fn interrupt(self) {
        // SAFETY: dijamin pemanggil — koneksi masih terbuka, jadi pointer sah.
        unsafe { libsqlite3_sys::sqlite3_interrupt(self.raw.as_ptr()) };
    }
}

/// Meng-interrupt statement SQLite bila di-drop sebelum statement selesai:
/// saat task job di-abort (user menekan cancel) atau saat timeout.
///
/// INVARIAN: guard ini harus di-drop SEBELUM `PoolConnection` asal handle-nya.
/// Pemakai mendeklarasikannya setelah koneksi dan di dalam scope satu
/// statement, sehingga urutan drop Rust (kebalikan urutan deklarasi) menjamin
/// koneksi masih terbuka ketika `Drop` di bawah berjalan.
pub(super) struct SqliteStatementGuard {
    handle: Option<SqliteInterruptHandle>,
    finished: bool,
}

impl SqliteStatementGuard {
    pub(super) fn new(handle: Option<SqliteInterruptHandle>) -> Self {
        Self {
            handle,
            finished: false,
        }
    }

    /// Statement selesai dengan sendirinya; tidak ada yang perlu di-interrupt.
    pub(super) fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for SqliteStatementGuard {
    fn drop(&mut self) {
        if !self.finished
            && let Some(handle) = self.handle
        {
            // SAFETY: lihat INVARIAN di atas — koneksi asal `handle` masih
            // hidup karena dideklarasikan lebih dulu daripada guard ini, dan
            // koneksi itu tidak ditutup selama masih kita pegang.
            unsafe { handle.interrupt() };
        }
    }
}

/// Menjalankan future dengan batas waktu opsional. `Err(())` berarti timeout.
async fn run_with_timeout<F: std::future::Future>(
    timeout: Option<std::time::Duration>,
    fut: F,
) -> Result<F::Output, ()> {
    match timeout {
        Some(limit) => tokio::time::timeout(limit, fut).await.map_err(|_| ()),
        None => Ok(fut.await),
    }
}

/// Pesan error timeout yang konsisten untuk semua driver.
fn timeout_message(options: &QueryExecutionOptions) -> String {
    match options.query_timeout {
        Some(limit) => format!(
            "Query timed out after {}s and was cancelled. Adjust the limit in Settings → Performance → Query timeout.",
            limit.as_secs()
        ),
        None => "Query timed out".to_string(),
    }
}

/// Memecah query job menjadi statement dengan splitter yang paham quote,
/// dollar-quote, dan komentar; statement yang hanya berisi komentar dibuang.
fn job_statements(options: &QueryExecutionOptions) -> Vec<String> {
    let hash_is_comment = matches!(
        options.connection.connection_type,
        models::enums::DatabaseType::MySQL
    );
    split_sql_statements(&options.query, hash_is_comment)
        .into_iter()
        .filter(|s| !is_comment_only_statement(s))
        .collect()
}

/// Potong teks untuk pratinjau tanpa memotong di tengah karakter multibyte.
fn preview_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        format!("{}...", text.chars().take(max_chars).collect::<String>())
    } else {
        text.to_string()
    }
}

/// Batas waktu membuka koneksi khusus untuk mengirim perintah cancel.
const CANCEL_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Minta server menghentikan statement yang sedang berjalan pada sesi `pid`.
/// Dipakai saat user menekan cancel atau saat timeout tercapai.
///
/// Perintah cancel dikirim lewat koneksi khusus yang dibuka dari opsi koneksi
/// pool, bukan lewat `pool.acquire()`: saat semua koneksi pool sedang dipakai
/// query lambat — persis saat cancel paling dibutuhkan — `acquire` hanya ikut
/// antre sampai timeout. Pool baru dipakai bila koneksi khusus gagal dibuka.
pub(crate) async fn cancel_backend_query(pool: models::enums::DatabasePool, pid: i64) {
    use sqlx::{ConnectOptions, Connection};

    match pool {
        models::enums::DatabasePool::PostgreSQL(pg) => {
            let options = pg.connect_options();
            let dedicated = tokio::time::timeout(CANCEL_CONNECT_TIMEOUT, options.connect()).await;
            let result = match dedicated {
                Ok(Ok(mut conn)) => {
                    let result = sqlx::query("SELECT pg_cancel_backend($1)")
                        .bind(pid as i32)
                        .execute(&mut conn)
                        .await;
                    if let Err(e) = conn.close().await {
                        debug!("[CANCEL] closing the cancel connection failed: {}", e);
                    }
                    result
                }
                _ => {
                    sqlx::query("SELECT pg_cancel_backend($1)")
                        .bind(pid as i32)
                        .execute(pg.as_ref())
                        .await
                }
            };
            if let Err(e) = result {
                log::warn!("[CANCEL] pg_cancel_backend({}) failed: {}", pid, e);
            }
        }
        models::enums::DatabasePool::MySQL(my) => {
            let kill = format!("KILL QUERY {}", pid);
            let options = my.connect_options();
            let dedicated = tokio::time::timeout(CANCEL_CONNECT_TIMEOUT, options.connect()).await;
            let result = match dedicated {
                Ok(Ok(mut conn)) => {
                    let result = sqlx::raw_sql(sqlx::AssertSqlSafe(kill.as_str()))
                        .execute(&mut conn)
                        .await;
                    if let Err(e) = conn.close().await {
                        debug!("[CANCEL] closing the cancel connection failed: {}", e);
                    }
                    result
                }
                _ => {
                    sqlx::raw_sql(sqlx::AssertSqlSafe(kill.as_str()))
                        .execute(my.as_ref())
                        .await
                }
            };
            if let Err(e) = result {
                log::warn!("[CANCEL] KILL QUERY {} failed: {}", pid, e);
            }
        }
        models::enums::DatabasePool::Plugin(plugin) => {
            // Untuk engine plugin, "pid" adalah job id (lihat execute_plugin_query_job).
            let result =
                crate::driver_api::run_blocking(move || plugin.session.cancel(pid as u64)).await;
            if let Err(e) = result {
                log::warn!("[CANCEL] plugin cancel for job {} failed: {}", pid, e);
            }
        }
        _ => {}
    }
}

pub(crate) fn prepare_query_job(
    tabular: &mut Tabular,
    connection_id: i64,
    query: String,
    job_id: u64,
) -> Result<QueryJob, QueryPreparationError> {
    let connection = tabular
        .connections
        .iter()
        .find(|c| c.id == Some(connection_id))
        .cloned()
        .ok_or(QueryPreparationError::ConnectionNotFound)?;

    // `{{KEY}}` diisi dari environment aktif project pemilik koneksi (hanya
    // variabel non-secret, karena teks query masuk riwayat).
    let project_vars =
        crate::window_egui::project_ui::sql_vars_for_connection(tabular, &connection);
    let query = crate::project::substitute(&query, &project_vars);

    let selected_database = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .filter(|s| !s.trim().is_empty());

    let dba_special_mode = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.dba_special_mode.clone());

    // Pool yang tunnel SSH-nya sudah mati (atau yang sudah ditutup) tidak akan
    // pernah berhasil lagi: buang dan mulai connect ulang di latar belakang,
    // supaya "try again in a moment" benar-benar berhasil pada percobaan
    // berikutnya.
    if super::pool::evict_unusable_pool(tabular, connection_id) {
        super::pool::ensure_background_pool_creation(tabular, connection_id);
        return Err(QueryPreparationError::PoolUnavailable);
    }

    let connection_pool = if let Some(pool) = tabular.connection_pools.get(&connection_id) {
        pool.clone()
    } else {
        super::pool::lock_or_recover(&tabular.shared_connection_pools)
            .get(&connection_id)
            .cloned()
            .ok_or(QueryPreparationError::PoolUnavailable)?
    };

    let base_query = if tabular.current_base_query.trim().is_empty() {
        None
    } else {
        Some(tabular.current_base_query.clone())
    };

    let options = QueryExecutionOptions {
        connection_id,
        connection,
        query,
        selected_database,
        schema_name: tabular
            .query_tabs
            .get(tabular.active_tab_index)
            .and_then(|t| t.schema_name.clone())
            .filter(|s| !s.trim().is_empty()),
        use_server_pagination: tabular.use_server_pagination,
        current_page: tabular.current_page,
        page_size: tabular.page_size,
        base_query,
        dba_special_mode,
        save_to_history: true,
        split_result_sets: true,
        ast_enabled: cfg!(feature = "query_ast"),
        job_id,
        query_timeout: (tabular.query_timeout_secs > 0)
            .then(|| std::time::Duration::from_secs(tabular.query_timeout_secs as u64)),
        max_rows: tabular.max_result_rows.max(1) as usize,
        backend_pids: tabular.jobs.backend_pids.clone(),
        read_only: false,
    };

    let tab_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .map(|t| t.id);

    Ok(QueryJob {
        job_id,
        tab_id,
        options,
        connection_pool,
        started_at: Instant::now(),
        on_result: tabular.result_wake_hook(),
    })
}

pub(crate) fn spawn_query_job(
    tabular: &mut Tabular,
    job: QueryJob,
    sender: std::sync::mpsc::Sender<QueryResultMessage>,
) -> Result<tokio::task::JoinHandle<()>, QueryPreparationError> {
    let runtime = tabular
        .runtime
        .clone()
        .ok_or(QueryPreparationError::RuntimeUnavailable)?;

    let wake = job.on_result.clone();
    let handle = runtime.spawn(async move {
        for result in execute_query_job_all(job).await {
            send_and_wake(&sender, result, wake.as_ref());
        }
    });

    Ok(handle)
}

/// Run a batch of statements **sequentially** on one task so script-like
/// input (`CREATE …; INSERT …; SELECT …`) executes in order instead of
/// racing on separate pool connections. Each statement reports its own
/// `QueryResultMessage`; after the first failure the remaining statements
/// are skipped (reported as errors) rather than executed.
pub(crate) fn spawn_query_job_batch(
    tabular: &mut Tabular,
    jobs: Vec<QueryJob>,
    sender: std::sync::mpsc::Sender<QueryResultMessage>,
) -> Result<tokio::task::JoinHandle<()>, QueryPreparationError> {
    let runtime = tabular
        .runtime
        .clone()
        .ok_or(QueryPreparationError::RuntimeUnavailable)?;

    let handle = runtime.spawn(async move {
        let mut previous_failed = false;
        // Satu koneksi untuk seluruh batch: transaksi, `USE`/`SET` dan tabel
        // temporer berlaku sampai statement terakhir.
        let mut batch_connection = BatchConnection::default();
        for mut job in jobs {
            let wake = job.on_result.clone();
            if previous_failed {
                send_and_wake(&sender, skipped_statement_message(&job), wake.as_ref());
                continue;
            }
            // Reset the clock so each statement reports its own duration,
            // not the time spent waiting behind earlier statements.
            job.started_at = Instant::now();
            for result in execute_query_job_all_in(job, &mut batch_connection).await {
                previous_failed |= !result.success;
                send_and_wake(&sender, result, wake.as_ref());
            }
        }
    });

    Ok(handle)
}

/// Kirim satu hasil ke UI lalu bangunkan UI-nya. egui hanya repaint saat ada
/// input, jadi tanpa hook ini hasil menunggu di channel sampai frame berikutnya.
pub(crate) fn send_and_wake(
    sender: &std::sync::mpsc::Sender<QueryResultMessage>,
    message: QueryResultMessage,
    wake: Option<&super::types::ResultWakeHook>,
) {
    if sender.send(message).is_ok()
        && let Some(wake) = wake
    {
        wake();
    }
}

fn skipped_statement_message(job: &QueryJob) -> QueryResultMessage {
    let message = "Skipped: a previous statement in this batch failed".to_string();
    QueryResultMessage {
        job_id: job.job_id,
        tab_id: job.tab_id,
        connection_id: job.options.connection_id,
        success: false,
        headers: vec!["Error".to_string()],
        rows: vec![vec![message.clone()]],
        error: Some(message),
        duration: std::time::Duration::ZERO,
        query: job.options.query.clone(),
        dba_special_mode: job.options.dba_special_mode.clone(),
        ast_debug_sql: None,
        ast_headers: None,
        affected_rows: None,
        column_metadata: None,
        truncated: false,
        error_location: None,
        timing: None,
    }
}

/// Seperti [`execute_query_job`], tetapi batch MsSQL yang menghasilkan beberapa
/// result set dikembalikan sebagai satu pesan per result set, sehingga tiap
/// `SELECT` di satu batch mendapat tab hasil sendiri.
pub(crate) async fn execute_query_job_all(job: QueryJob) -> Vec<QueryResultMessage> {
    execute_query_job_all_in(job, &mut BatchConnection::default()).await
}

/// [`execute_query_job_all`] dengan koneksi batch yang dibagi antar statement.
async fn execute_query_job_all_in(
    job: QueryJob,
    batch_connection: &mut BatchConnection,
) -> Vec<QueryResultMessage> {
    let is_mssql = job.options.connection.connection_type == models::enums::DatabaseType::MsSQL;
    if !(is_mssql && job.options.split_result_sets) {
        return vec![execute_query_job_in(job, batch_connection).await];
    }
    let models::enums::DatabasePool::MsSQL(pool) = job.connection_pool.clone() else {
        return vec![execute_query_job_in(job, batch_connection).await];
    };

    let query = job.options.query.trim().to_string();
    // Batas baris diterapkan saat membaca (bukan setelah semua baris
    // dikonversi), dan timeout membatalkan query di server lalu membuang
    // koneksinya.
    let outcome = driver_mssql::execute_query_multi_bounded(
        pool,
        &query,
        job.options.max_rows.max(1),
        job.options.query_timeout,
    )
    .await;
    let message_for = |headers: Vec<String>, rows: Vec<Vec<String>>| QueryResultMessage {
        job_id: job.job_id,
        tab_id: job.tab_id,
        connection_id: job.options.connection_id,
        success: true,
        headers,
        rows,
        error: None,
        duration: job.started_at.elapsed(),
        query: job.options.query.clone(),
        dba_special_mode: job.options.dba_special_mode.clone(),
        ast_debug_sql: None,
        ast_headers: None,
        affected_rows: None,
        column_metadata: None,
        truncated: false,
        error_location: None,
        timing: None,
    };
    match outcome {
        Ok(sets) if !sets.is_empty() => sets
            .into_iter()
            .map(|(headers, rows, truncated)| {
                let mut message = message_for(headers, rows);
                message.truncated = truncated;
                message
            })
            .collect(),
        Ok(_) => vec![message_for(Vec::new(), Vec::new())],
        Err(e) => {
            let (message, error_location) =
                describe_execution_error(mssql_execution_error(e), &job.options);
            let mut msg = message_for(vec!["Error".to_string()], vec![vec![message.clone()]]);
            msg.success = false;
            msg.error = Some(message);
            msg.error_location = error_location;
            vec![msg]
        }
    }
}

/// Petakan kegagalan eksekusi MsSQL ke error query bertipe.
fn mssql_execution_error(e: driver_mssql::MssqlExecError) -> QueryExecutionError {
    match e {
        driver_mssql::MssqlExecError::Timeout => QueryExecutionError::Timeout,
        driver_mssql::MssqlExecError::Connection(message) => {
            QueryExecutionError::Connection(format!("MsSQL connection error: {}", message))
        }
        driver_mssql::MssqlExecError::Query(message) => {
            QueryExecutionError::Message(format!("Query error: {}", message))
        }
    }
}

/// Jalankan satu job query sampai selesai. Dipakai GUI (via `spawn_query_job`)
/// dan lapisan headless `crate::agent`.
pub(crate) async fn execute_query_job(job: QueryJob) -> QueryResultMessage {
    execute_query_job_in(job, &mut BatchConnection::default()).await
}

/// [`execute_query_job`] dengan koneksi batch yang dibagi antar statement.
async fn execute_query_job_in(
    job: QueryJob,
    batch_connection: &mut BatchConnection,
) -> QueryResultMessage {
    batch_connection.claim(job.options.connection_id);
    let start = job.started_at;
    let tab_id = job.tab_id;
    let connection_id = job.options.connection_id;
    let query = job.options.query.clone();
    let dba_special_mode = job.options.dba_special_mode.clone();

    let (outcome, timing) = super::timing::with_probe(start, async {
        match job.options.connection.connection_type {
            models::enums::DatabaseType::MySQL => {
                execute_mysql_query_job(
                    &job.options,
                    job.connection_pool.clone(),
                    batch_connection,
                )
                .await
            }
            models::enums::DatabaseType::PostgreSQL => {
                execute_postgres_query_job(
                    &job.options,
                    job.connection_pool.clone(),
                    batch_connection,
                )
                .await
            }
            models::enums::DatabaseType::SQLite => {
                execute_sqlite_query_job(
                    &job.options,
                    job.connection_pool.clone(),
                    batch_connection,
                )
                .await
            }
            models::enums::DatabaseType::Redis => {
                execute_redis_query_job(&job.options, job.connection_pool.clone()).await
            }
            models::enums::DatabaseType::MsSQL => {
                execute_mssql_query_job(&job.options, job.connection_pool.clone()).await
            }
            models::enums::DatabaseType::MongoDB => {
                execute_mongodb_query_job(&job.options, job.connection_pool.clone()).await
            }
            models::enums::DatabaseType::ApiHttp => Err(QueryExecutionError::Message(
                "API-HTTP connections do not support SQL queries".to_string(),
            )),
            models::enums::DatabaseType::Plugin(_) => {
                execute_plugin_query_job(&job.options, job.connection_pool.clone()).await
            }
        }
    })
    .await;

    match outcome {
        Ok(output) => QueryResultMessage {
            job_id: job.job_id,
            tab_id,
            connection_id,
            success: true,
            headers: output.headers.clone(),
            rows: output.rows.clone(),
            error: None,
            duration: start.elapsed(),
            query: query.clone(),
            dba_special_mode,
            ast_debug_sql: output.ast_debug_sql,
            ast_headers: output.ast_headers,
            affected_rows: output.affected_rows.map(|n| n as usize),
            column_metadata: output.column_metadata,
            truncated: output.truncated,
            error_location: None,
            timing,
        },
        Err(err) => {
            if err.is_connection() {
                log::warn!(
                    "[EXEC] Connection-class failure on connection {}: {}",
                    connection_id,
                    err
                );
            }
            let (message, error_location) = describe_execution_error(err, &job.options);
            QueryResultMessage {
                job_id: job.job_id,
                tab_id,
                connection_id,
                success: false,
                headers: vec!["Error".to_string()],
                rows: vec![vec![message.clone()]],
                error: Some(message),
                duration: start.elapsed(),
                query,
                dba_special_mode,
                ast_debug_sql: None,
                ast_headers: None,
                affected_rows: None,
                column_metadata: None,
                truncated: false,
                error_location,
                timing: None,
            }
        }
    }
}

fn describe_execution_error(
    err: QueryExecutionError,
    options: &QueryExecutionOptions,
) -> (String, Option<super::types::ErrorLocation>) {
    match err {
        QueryExecutionError::Message(msg) | QueryExecutionError::Connection(msg) => (msg, None),
        QueryExecutionError::Located(msg, location) => (msg, Some(location)),
        QueryExecutionError::Timeout => (timeout_message(options), None),
        QueryExecutionError::Cancelled => ("Query cancelled".to_string(), None),
    }
}

/// Posisi error dari PostgreSQL (field `position`, dalam karakter, 1-based).
fn postgres_error_location(
    err: &sqlx::Error,
    statement: &str,
) -> Option<super::types::ErrorLocation> {
    let sqlx::Error::Database(db_err) = err else {
        return None;
    };
    let pg = db_err.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()?;
    match pg.position()? {
        sqlx::postgres::PgErrorPosition::Original(position) => Some(super::types::ErrorLocation {
            statement: statement.to_string(),
            char_offset: Some(position.saturating_sub(1)),
            line: None,
        }),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-driver async execution helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn execute_mysql_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
    batch_connection: &mut BatchConnection,
) -> Result<QueryJobOutput, QueryExecutionError> {
    debug!(
        "[async] Executing MySQL query (conn_id={})",
        options.connection_id
    );

    let mysql_pool = match &pool {
        models::enums::DatabasePool::MySQL(mysql_pool) => mysql_pool.clone(),
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for MySQL".to_string(),
            ));
        }
    };

    let statements_owned = job_statements(options);
    let statements_raw: Vec<&str> = statements_owned.iter().map(|s| s.as_str()).collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = {
        let allow_ast_rewrite = options.ast_enabled
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
            && is_simple_select_statement(statements_raw[0]);

        if allow_ast_rewrite {
            let should_paginate =
                options.use_server_pagination && !query_contains_pagination(statements_raw[0]);
            let pagination_opt = if should_paginate {
                Some((options.current_page as u64, options.page_size as u64))
            } else {
                None
            };
            let inject_auto_limit = should_paginate;
            match crate::query_ast::compile_single_select(
                statements_raw[0],
                &options.connection.connection_type,
                pagination_opt,
                inject_auto_limit,
            ) {
                Ok((new_sql, hdrs)) => {
                    if !hdrs.is_empty() {
                        inferred_headers_from_ast = Some(hdrs.clone());
                        ast_headers = Some(hdrs.clone());
                    }
                    vec![new_sql]
                }
                Err(_) => statements_raw.iter().map(|s| s.to_string()).collect(),
            }
        } else {
            statements_raw.iter().map(|s| s.to_string()).collect()
        }
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    let default_db = if let Some(db) = &options.selected_database {
        if db.trim().is_empty() {
            options.connection.database.clone()
        } else {
            db.clone()
        }
    } else {
        options.connection.database.clone()
    };

    debug!(
        "[mysql] selected_database={:?}, default_db={}",
        options.selected_database, default_db
    );

    let replication_status_mode = matches!(
        options.dba_special_mode,
        Some(models::enums::DBASpecialMode::ReplicationStatus)
    );
    let master_status_mode = matches!(
        options.dba_special_mode,
        Some(models::enums::DBASpecialMode::MasterStatus)
    );

    let mut ast_debug_sql: Option<String> = None;
    #[cfg(feature = "query_ast")]
    {
        if let Some(sql) = statements.first()
            && statements.len() == 1
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
        {
            ast_debug_sql = Some(sql.clone());
        }
    }

    let mut last_error: Option<String> = None;
    let mut failing_stmt_preview: Option<String> = None;
    let mut error_location: Option<super::types::ErrorLocation> = None;
    // True bila kegagalannya di koneksi (bukan di statement).
    let mut connection_failed = false;

    // Blok berlabel: `break 'job` keluar ke penanganan error di bawah tanpa
    // mengembalikan koneksi ke `batch_connection`.
    'job: {
        // Job berjalan di koneksi pool, bukan `MySqlConnection::connect` baru
        // per job: pool sudah membawa pengaturan SSL, batas waktu connect
        // (`acquire_timeout`), dan SET sesi dari `after_connect`. Koneksi baru
        // per job mengabaikan SSL, tidak punya batas waktu connect, dan
        // memaksa `sql_mode` yang berbeda dari server.
        let (mut conn, pid) = match batch_connection.mysql.take() {
            // Statement berikutnya dari batch yang sama: lanjut di koneksi
            // yang sama, termasuk database hasil `USE` sebelumnya.
            Some(conn) => {
                let mut conn = conn;
                let pid = sqlx::query_scalar::<_, u64>("SELECT CONNECTION_ID()")
                    .fetch_one(&mut *conn)
                    .await
                    .ok();
                (conn, pid)
            }
            None => {
                let (mut conn, pid) = match acquire_live_mysql(mysql_pool.as_ref()).await {
                    Ok(acquired) => acquired,
                    Err(e) => {
                        connection_failed = true;
                        last_error = Some(e.to_string());
                        break 'job;
                    }
                };
                // Database aktif tab bisa berbeda dari database default pool.
                // Hook `after_release` pool mengembalikannya ke default.
                if !default_db.trim().is_empty() {
                    let use_stmt = format!("USE `{}`", default_db.trim().replace('`', "``"));
                    if let Err(e) = sqlx::raw_sql(sqlx::AssertSqlSafe(use_stmt))
                        .execute(&mut *conn)
                        .await
                    {
                        connection_failed = super::types::is_connection_class(&e);
                        last_error = Some(format!(
                            "Cannot switch to database '{}': {}",
                            default_db.trim(),
                            e
                        ));
                        break 'job;
                    }
                }
                (conn, pid)
            }
        };

        // Catat connection id supaya cancel/timeout bisa mengirim KILL QUERY.
        let _pid_guard = pid.map(|pid| {
            BackendPidGuard::register(&options.backend_pids, options.job_id, pid as i64)
        });

        // Jalur baca agent: seluruh job berjalan dalam transaksi read-only,
        // sehingga tulisan yang lolos dari classifier ditolak server (1792).
        // Engine kompatibel-MySQL yang tidak mengenal sintaks ini tetap
        // dilayani: classifier adalah gerbang utamanya, ini lapis kedua.
        let mut read_only_tx_open = false;
        if options.read_only {
            match sqlx::raw_sql("START TRANSACTION READ ONLY")
                .execute(&mut *conn)
                .await
            {
                Ok(_) => read_only_tx_open = true,
                Err(e) if super::types::is_connection_class(&e) => {
                    connection_failed = true;
                    last_error = Some(format!("Cannot start a read-only transaction: {}", e));
                    break 'job;
                }
                Err(e) => log::warn!(
                    "[EXEC] MySQL server rejected START TRANSACTION READ ONLY; running without it: {}",
                    e
                ),
            }
        }
        // True bila koneksi tidak boleh kembali ke pool (query masih berjalan).
        let mut conn_in_flight = false;

        let mut final_headers: Vec<String> = Vec::new();
        let mut final_data: Vec<Vec<String>> = Vec::new();
        let mut final_column_metadata: Option<Vec<models::structs::ColumnMetadata>> = None;
        let mut final_affected: Option<u64> = None;
        let mut final_truncated = false;
        let mut execution_success = true;

        for (idx, statement) in statements_ref.iter().enumerate() {
            let trimmed = statement.trim();
            if is_comment_only_statement(trimmed) {
                continue;
            }

            debug!("[mysql] about to run statement[{}]: {:?}", idx + 1, trimmed);

            let upper = strip_leading_sql_comments(trimmed).to_uppercase();

            let is_admin_command = {
                upper.starts_with("PURGE BINARY LOGS")
                    || upper.starts_with("PURGE MASTER LOGS")
                    || upper.starts_with("RESET MASTER")
                    || upper.starts_with("RESET SLAVE")
                    || upper.starts_with("RESET REPLICA")
                    || upper.starts_with("CHANGE MASTER")
                    || upper.starts_with("CHANGE REPLICATION SOURCE")
                    || upper.starts_with("FLUSH")
            };

            if let Some(db_name) = parse_use_statement(trimmed) {
                // `USE` dijalankan di koneksi yang sama lewat protokol teks
                // (protokol prepared statement menolaknya dengan error 1295).
                let use_stmt = format!("USE `{}`", db_name.replace('`', "``"));
                if let Err(e) = sqlx::raw_sql(sqlx::AssertSqlSafe(use_stmt))
                    .execute(&mut *conn)
                    .await
                {
                    connection_failed = super::types::is_connection_class(&e);
                    last_error = Some(format!("USE failed: {}", e));
                    failing_stmt_preview.get_or_insert_with(|| preview_text(trimmed, 200));
                    execution_success = false;
                    break;
                }
                continue;
            }

            let returns_rows = statement_returns_rows(trimmed);
            let query_result = run_with_timeout(
                options.query_timeout,
                run_mysql_statement(&mut conn, trimmed, returns_rows, options.max_rows),
            )
            .await;

            match query_result {
                Ok(Ok((rows, truncated, affected))) => {
                    if idx == statements_ref.len() - 1 {
                        final_affected = affected;
                        final_truncated = truncated;
                        if !rows.is_empty() {
                            final_headers = rows[0]
                                .columns()
                                .iter()
                                .map(|c| c.name().to_string())
                                .collect();

                            let mut meta_vec = Vec::new();
                            let mut inferred_table_name = None;
                            if let Ok(ast) = sqlparser::parser::Parser::parse_sql(
                                &sqlparser::dialect::MySqlDialect {},
                                trimmed,
                            ) && let Some(sqlparser::ast::Statement::Query(q)) = ast.first()
                                && let sqlparser::ast::SetExpr::Select(select) = &*q.body
                                && let Some(table_with_joins) = select.from.first()
                                && let sqlparser::ast::TableFactor::Table { name, .. } =
                                    &table_with_joins.relation
                            {
                                inferred_table_name = Some(name.to_string());
                                log::debug!("🔥 Inferred table name: {}", name);
                            } else {
                                log::warn!("🔥 Failed to infer table name from query: {}", trimmed);
                            }

                            let mut unique_tables = std::collections::HashSet::new();
                            for _col in rows[0].columns() {
                                let t_name = String::new();
                                if !t_name.is_empty() {
                                    unique_tables.insert(t_name.clone());
                                }
                            }
                            if let Some(t) = &inferred_table_name {
                                unique_tables.insert(t.clone());
                            }

                            let mut table_pks: std::collections::HashMap<
                                String,
                                std::collections::HashSet<String>,
                            > = std::collections::HashMap::new();

                            let data_dir = crate::directory::get_data_dir();
                            let db_path = data_dir.join("connections.db");
                            let cache_conn_str =
                                format!("sqlite://{}?mode=ro", db_path.to_string_lossy());

                            match sqlx::sqlite::SqlitePool::connect(&cache_conn_str).await {
                                Ok(cache_pool) => {
                                    for table_full_name in &unique_tables {
                                        let parts: Vec<&str> = table_full_name.split('.').collect();
                                        let (target_db, target_table) = if parts.len() >= 2 {
                                            (parts[0], parts[1])
                                        } else {
                                            (default_db.as_str(), table_full_name.as_str())
                                        };

                                        let query = "SELECT columns_json FROM index_cache \
                                             WHERE connection_id = ? \
                                             AND database_name = ? \
                                             AND table_name LIKE ? \
                                             AND index_name = 'PRIMARY'";

                                        let result: Result<Option<(String,)>, _> =
                                            sqlx::query_as(query)
                                                .bind(options.connection.id.unwrap_or(0))
                                                .bind(target_db)
                                                .bind(target_table)
                                                .fetch_optional(&cache_pool)
                                                .await;

                                        match result {
                                            Ok(Some((json_str,))) => {
                                                if let Ok(cols) =
                                                    serde_json::from_str::<Vec<String>>(&json_str)
                                                    && !cols.is_empty()
                                                {
                                                    let pks: std::collections::HashSet<String> =
                                                        cols.into_iter()
                                                            .map(|s| s.to_lowercase())
                                                            .collect();
                                                    debug!(
                                                        "Found cached PKs for '{}': {:?}",
                                                        table_full_name, pks
                                                    );
                                                    table_pks.insert(
                                                        table_full_name.to_lowercase(),
                                                        pks,
                                                    );
                                                }
                                            }
                                            Ok(None) => {
                                                debug!(
                                                    "No cached PK found for '{}' (db={}, tbl={})",
                                                    table_full_name, target_db, target_table
                                                );
                                            }
                                            Err(e) => {
                                                debug!(
                                                    "Error fetching PK from cache for '{}': {}",
                                                    table_full_name, e
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    debug!(
                                        "Failed to connect to local cache at {}: {}",
                                        db_path.display(),
                                        e
                                    );
                                }
                            }

                            let (inferred_origins, involved_tables) = infer_column_origins(trimmed);

                            let mut expanded_schema: Vec<(String, String)> = Vec::new();

                            let exact_match_possible = if let Some(origins) = &inferred_origins {
                                origins.len() == rows[0].columns().len()
                                    && origins.iter().all(|o| o.is_some())
                            } else {
                                false
                            };

                            if !exact_match_possible && !involved_tables.is_empty() {
                                log::debug!(
                                    "🔥 Fetching ordered schema for involved tables: {:?}",
                                    involved_tables
                                );
                                for table in &involved_tables {
                                    let col_query = format!("SHOW COLUMNS FROM {}", table);
                                    if let Ok(col_rows) =
                                        sqlx::query(sqlx::AssertSqlSafe(col_query.as_str()))
                                            .fetch_all(&mut *conn)
                                            .await
                                    {
                                        for row in col_rows {
                                            if let Ok(col_name) = row.try_get::<String, _>("Field")
                                            {
                                                expanded_schema.push((col_name, table.clone()));
                                            }
                                        }
                                    }
                                }
                            }

                            let use_fine_grained = if let Some(origins) = &inferred_origins {
                                origins.len() == rows[0].columns().len()
                            } else {
                                false
                            };

                            if use_fine_grained {
                                log::debug!("🔥 Using fine-grained column table inference");
                            }

                            for (i, col) in rows[0].columns().iter().enumerate() {
                                let type_info = col.type_info();
                                let t_name = String::new();

                                log::debug!(
                                    "🔥 [debug] inferring table for col '{}': t_name='{}', use_fine_grained={}, involved_tables={:?}, expanded_len={}",
                                    col.name(),
                                    t_name,
                                    use_fine_grained,
                                    involved_tables,
                                    expanded_schema.len()
                                );

                                let table_name = if !t_name.is_empty() {
                                    Some(t_name.clone())
                                } else {
                                    let ast_name = if use_fine_grained {
                                        inferred_origins
                                            .as_ref()
                                            .and_then(|o| o.get(i).cloned().flatten())
                                    } else {
                                        None
                                    };

                                    if ast_name.is_some() {
                                        ast_name
                                    } else if involved_tables.len() == 1 {
                                        Some(involved_tables[0].clone())
                                    } else if expanded_schema.len() == rows[0].columns().len() {
                                        Some(expanded_schema[i].1.clone())
                                    } else {
                                        None
                                    }
                                };

                                let is_pk = if let Some(t) = &table_name {
                                    let key = t.to_lowercase();
                                    if let Some(pks) = table_pks.get(&key) {
                                        pks.contains(&col.name().to_lowercase())
                                    } else if let Some(simple_name) = key.split('.').next_back()
                                        && let Some(pks) = table_pks.get(simple_name)
                                    {
                                        pks.contains(&col.name().to_lowercase())
                                    } else if let Some((_k, pks)) = table_pks
                                        .iter()
                                        .find(|(k, _)| k.ends_with(&format!(".{}", key)))
                                    {
                                        pks.contains(&col.name().to_lowercase())
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                };

                                if let Some(final_t) = &table_name {
                                    log::debug!("🔥 [debug] -> Resolved table: {}", final_t);
                                } else {
                                    log::debug!("🔥 [debug] -> Resolved table: NONE");
                                }

                                meta_vec.push(models::structs::ColumnMetadata {
                                    name: col.name().to_string(),
                                    type_name: type_info.name().to_string(),
                                    table_name,
                                    original_name: Some(col.name().to_string()),
                                    is_primary_key: is_pk,
                                });
                            }
                            final_column_metadata = Some(meta_vec);

                            final_data = driver_mysql::convert_mysql_rows_to_table_data(rows);

                            if replication_status_mode || master_status_mode {
                                let version_str = match sqlx::query("SELECT VERSION() AS v")
                                    .fetch_one(&mut *conn)
                                    .await
                                {
                                    Ok(vrow) => vrow.try_get::<String, _>("v").unwrap_or_default(),
                                    Err(_) => String::new(),
                                };
                                let is_mariadb = version_str.to_lowercase().contains("mariadb");

                                if replication_status_mode
                                    && final_data.is_empty()
                                    && let Ok(fallback_rows) =
                                        sqlx::query("SHOW SLAVE STATUS").fetch_all(&mut *conn).await
                                    && !fallback_rows.is_empty()
                                {
                                    final_headers = fallback_rows[0]
                                        .columns()
                                        .iter()
                                        .map(|c| c.name().to_string())
                                        .collect();
                                    final_data = driver_mysql::convert_mysql_rows_to_table_data(
                                        fallback_rows,
                                    );
                                }

                                if !final_headers.is_empty() && !final_data.is_empty() {
                                    let header_index = |name: &str| {
                                        final_headers
                                            .iter()
                                            .position(|h| h.eq_ignore_ascii_case(name))
                                    };
                                    let first = &final_data[0];
                                    let mut summary: Vec<(String, String)> = Vec::new();

                                    if replication_status_mode {
                                        if let Some(idx) = header_index("Replica_IO_Running")
                                            .or_else(|| header_index("Slave_IO_Running"))
                                        {
                                            summary.push(("IO Thread".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Replica_SQL_Running")
                                            .or_else(|| header_index("Slave_SQL_Running"))
                                        {
                                            summary.push(("SQL Thread".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Seconds_Behind_Source")
                                            .or_else(|| header_index("Seconds_Behind_Master"))
                                        {
                                            summary.push((
                                                "Seconds Behind".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) = header_index("Channel_Name") {
                                            summary.push(("Channel".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Retrieved_Gtid_Set") {
                                            summary.push((
                                                "Retrieved GTID".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) = header_index("Executed_Gtid_Set") {
                                            summary
                                                .push(("Executed GTID".into(), first[idx].clone()));
                                        }
                                    }

                                    if master_status_mode {
                                        if let Some(idx) = header_index("File") {
                                            summary.push((
                                                "Binary Log File".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                        if let Some(idx) = header_index("Position") {
                                            summary.push(("Position".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Binlog_Do_DB") {
                                            summary
                                                .push(("Binlog Do DB".into(), first[idx].clone()));
                                        }
                                        if let Some(idx) = header_index("Binlog_Ignore_DB") {
                                            summary.push((
                                                "Binlog Ignore DB".into(),
                                                first[idx].clone(),
                                            ));
                                        }
                                    }

                                    if !summary.is_empty() {
                                        let mut summary_table: Vec<Vec<String>> = summary
                                            .into_iter()
                                            .map(|(metric, value)| vec![metric, value])
                                            .collect();
                                        summary_table.push(vec![
                                            "Server Version".into(),
                                            version_str.clone(),
                                        ]);
                                        summary_table.push(vec![
                                            "Engine".into(),
                                            if is_mariadb {
                                                "MariaDB".into()
                                            } else {
                                                "MySQL".into()
                                            },
                                        ]);
                                        final_headers = vec!["Metric".into(), "Value".into()];
                                        final_data = summary_table;
                                    }
                                }
                            }
                        } else {
                            #[cfg(feature = "query_ast")]
                            if final_headers.is_empty()
                                && ast_debug_sql.is_some()
                                && let Some(hh) = inferred_headers_from_ast.clone()
                                && !hh.is_empty()
                            {
                                final_headers = hh;
                            }

                            if final_headers.is_empty()
                                && trimmed.to_uppercase().starts_with("SELECT")
                            {
                                let inferred = infer_select_headers(trimmed);
                                if !inferred.is_empty() {
                                    final_headers = inferred;
                                }
                            }

                            final_data = Vec::new();

                            // Perintah admin tanpa result set (dijalankan lewat
                            // protokol teks) tetap memberi konfirmasi.
                            if is_admin_command && final_headers.is_empty() {
                                final_headers = vec!["Status".to_string()];
                                final_data =
                                    vec![vec!["Command executed successfully".to_string()]];
                            }
                        }
                    }
                }
                Ok(Err(e)) => {
                    connection_failed = super::types::is_connection_class(&e);
                    let err_str = e.to_string();

                    if is_admin_command
                        && (err_str.contains("1295")
                            || err_str.contains("prepared statement protocol"))
                    {
                        debug!(
                            "Admin command executed successfully (error 1295 expected for prepared statements)"
                        );
                        if idx == statements_ref.len() - 1 {
                            final_headers = vec!["Status".to_string()];
                            final_data = vec![vec!["Command executed successfully".to_string()]];
                        }
                    } else {
                        if failing_stmt_preview.is_none() {
                            failing_stmt_preview = Some(preview_text(trimmed, 200));
                        }
                        if let Some(line) = super::sql::mysql_error_line(&err_str) {
                            error_location = Some(super::types::ErrorLocation {
                                statement: trimmed.to_string(),
                                char_offset: None,
                                line: Some(line),
                            });
                        }
                        if err_str.contains("1146")
                            || err_str.to_lowercase().contains("doesn't exist")
                        {
                            let mut hint = String::new();
                            hint.push_str(
                                "Hint: Check the database/schema qualifier in your SQL. ",
                            );
                            hint.push_str(&format!(
                                "Current default database is '{}'. If your query references a different schema (e.g., 'foxlogger' vs actual '{}'), it can fail even if SELECT * FROM table works in the default DB. ",
                                default_db, default_db
                            ));
                            hint.push_str("Try removing the schema prefix or replacing it with the selected/default database, or switch the active database in the tab. ");
                            hint.push_str("Also, on case-sensitive MySQL servers (lower_case_table_names=0), using backticks requires exact table name casing. If unquoted works but \"`name`\" fails, check SHOW TABLES for the exact case and match it.");
                            last_error = Some(format!("{}\n\n{}", err_str, hint));
                        } else {
                            last_error = Some(err_str);
                        }
                        execution_success = false;
                        break;
                    }
                }
                Err(_) => {
                    // Future yang di-drop tidak menghentikan query di server,
                    // jadi kirim KILL QUERY lewat koneksi lain dari pool.
                    let pid = super::pool::lock_or_recover(&options.backend_pids)
                        .get(&options.job_id)
                        .copied();
                    if let Some(pid) = pid {
                        cancel_backend_query(pool.clone(), pid).await;
                    }
                    // Respons query yang dibatalkan belum dibaca dari soket.
                    conn_in_flight = true;
                    last_error = Some(timeout_message(options));
                    failing_stmt_preview.get_or_insert_with(|| preview_text(trimmed, 200));
                    execution_success = false;
                    break;
                }
            }
        }

        // Akhiri transaksi read-only. Bila gagal, `after_release` pool tetap
        // me-rollback saat koneksi dilepas.
        if read_only_tx_open
            && !conn_in_flight
            && let Err(e) = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await
        {
            log::warn!("[EXEC] MySQL read-only transaction could not be closed: {}", e);
        }

        if conn_in_flight {
            // Tutup koneksinya; jangan dikembalikan ke pool dengan respons
            // tertunda yang akan dibaca pemakai berikutnya.
            drop(conn.detach());
        } else if execution_success {
            // Statement berikutnya di batch yang sama memakai koneksi ini.
            batch_connection.mysql = Some(conn);
        }

        if execution_success {
            if !final_headers.is_empty() {
                debug!(
                    "[mysql] final headers ({}): {:?}",
                    final_headers.len(),
                    final_headers
                );
            } else {
                debug!(
                    "[mysql] final headers are empty (rows: {})",
                    final_data.len()
                );
            }
            return Ok(QueryJobOutput {
                headers: final_headers,
                rows: final_data,
                ast_debug_sql,
                ast_headers,
                column_metadata: final_column_metadata,
                affected_rows: final_affected,
                truncated: final_truncated,
            });
        }

        // Statement gagal atau timeout. Jangan diulang: statement sebelumnya
        // (atau statement yang timeout itu sendiri) mungkin sudah berefek,
        // sehingga retry bisa menjalankan DML dua kali. Yang diulang hanya
        // pengambilan koneksi, sebelum statement user mana pun dijalankan
        // (lihat `acquire_live_mysql`).
    }

    let mut final_err = last_error.unwrap_or_else(|| "Unknown MySQL error".to_string());
    if let Some(stmt) = failing_stmt_preview {
        final_err = format!("{}\n\nFailed statement (preview): {}", final_err, stmt);
    }
    Err(match error_location {
        Some(location) => QueryExecutionError::Located(final_err, location),
        None if connection_failed => QueryExecutionError::Connection(final_err),
        None => QueryExecutionError::Message(final_err),
    })
}

async fn execute_postgres_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
    batch_connection: &mut BatchConnection,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let pg_pool = match pool {
        models::enums::DatabasePool::PostgreSQL(pg) => pg,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for PostgreSQL".to_string(),
            ));
        }
    };

    let statements_owned = job_statements(options);
    let statements_raw: Vec<&str> = statements_owned.iter().map(|s| s.as_str()).collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    let mut ast_debug_sql: Option<String> = None;

    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = {
        let allow_ast_rewrite = options.ast_enabled
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
            && is_simple_select_statement(statements_raw[0]);

        if allow_ast_rewrite {
            let should_paginate =
                options.use_server_pagination && !query_contains_pagination(statements_raw[0]);
            let pagination_opt = if should_paginate {
                Some((options.current_page as u64, options.page_size as u64))
            } else {
                None
            };
            let inject_auto_limit = should_paginate;
            match crate::query_ast::compile_single_select(
                statements_raw[0],
                &options.connection.connection_type,
                pagination_opt,
                inject_auto_limit,
            ) {
                Ok((new_sql, hdrs)) => {
                    if !hdrs.is_empty() {
                        inferred_headers_from_ast = Some(hdrs.clone());
                        ast_headers = Some(hdrs.clone());
                    }
                    ast_debug_sql = Some(new_sql.clone());
                    vec![new_sql]
                }
                Err(_) => statements_raw.iter().map(|s| s.to_string()).collect(),
            }
        } else {
            statements_raw.iter().map(|s| s.to_string()).collect()
        }
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    // Semua statement dalam job memakai satu koneksi yang sama, sehingga SET /
    // search_path dan statement berikutnya konsisten, dan backend pid-nya
    // diketahui untuk keperluan cancel.
    let (mut conn, backend_pid) = match batch_connection.postgres.take() {
        // Statement berikutnya dari batch yang sama: lanjut di koneksi (dan
        // transaksi) yang sama.
        Some(conn) => {
            let mut conn = conn;
            let pid = sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()")
                .fetch_one(&mut *conn)
                .await
                .ok();
            (conn, pid)
        }
        None => acquire_live_postgres(pg_pool.as_ref()).await?,
    };
    let _pid_guard = backend_pid
        .map(|pid| BackendPidGuard::register(&options.backend_pids, options.job_id, pid as i64));

    // Terapkan schema aktif tab di koneksi ini juga. `SET search_path` yang
    // dijalankan terpisah bisa mendarat di koneksi pool lain dan tidak berefek.
    if let Some(schema) = options.schema_name.as_deref() {
        let set_path = format!(
            "SET search_path TO \"{}\", public",
            schema.replace('"', "\"\"")
        );
        if let Err(e) = sqlx::query(sqlx::AssertSqlSafe(set_path.as_str()))
            .execute(&mut *conn)
            .await
        {
            return Err(QueryExecutionError::Message(format!(
                "Cannot switch to schema '{}': {}",
                schema, e
            )));
        }
    }

    // Jalur baca agent: seluruh job berjalan dalam transaksi read-only,
    // sehingga tulisan yang lolos dari classifier ditolak server (25006). Pada
    // jalur error koneksi dilepas dan `after_release` me-rollback transaksinya.
    // Engine kompatibel-PostgreSQL yang tidak mengenal sintaks ini tetap
    // dilayani: classifier adalah gerbang utamanya, ini lapis kedua.
    let mut read_only_tx_open = false;
    if options.read_only {
        match sqlx::raw_sql("BEGIN READ ONLY").execute(&mut *conn).await {
            Ok(_) => read_only_tx_open = true,
            Err(e) if super::types::is_connection_class(&e) => {
                return Err(QueryExecutionError::from_sqlx_with_context(
                    "Cannot start a read-only transaction: ",
                    e,
                ));
            }
            Err(e) => log::warn!(
                "[EXEC] PostgreSQL server rejected BEGIN READ ONLY; running without it: {}",
                e
            ),
        }
    }

    let mut final_headers = Vec::new();
    let mut final_data = Vec::new();
    let mut final_affected: Option<u64> = None;
    let mut final_truncated = false;

    for (i, statement) in statements_ref.iter().enumerate() {
        let trimmed = statement.trim();
        if is_comment_only_statement(trimmed) {
            continue;
        }

        let returns_rows = statement_returns_rows(trimmed);
        let result = run_with_timeout(options.query_timeout, async {
            if returns_rows {
                fetch_rows_limited!(
                    sqlx::query(sqlx::AssertSqlSafe(trimmed)),
                    &mut *conn,
                    options.max_rows
                )
                .await
                .map(|(rows, truncated)| (rows, truncated, None))
            } else {
                sqlx::query(sqlx::AssertSqlSafe(trimmed))
                    .execute(&mut *conn)
                    .await
                    .map(|r| (Vec::new(), false, Some(r.rows_affected())))
            }
        })
        .await;

        match result {
            Ok(Ok((rows, truncated, affected))) => {
                if i == statements_ref.len() - 1 {
                    final_affected = affected;
                    final_truncated = truncated;
                    if !rows.is_empty() {
                        final_headers = rows[0]
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        final_data =
                            crate::driver_postgres::convert_postgres_rows_to_table_data(rows);
                    } else {
                        #[cfg(feature = "query_ast")]
                        if final_headers.is_empty()
                            && let Some(hh) = inferred_headers_from_ast.clone()
                            && !hh.is_empty()
                        {
                            final_headers = hh;
                        }
                        if final_headers.is_empty()
                            && strip_leading_sql_comments(trimmed)
                                .to_uppercase()
                                .starts_with("SELECT")
                        {
                            let inferred = infer_select_headers(trimmed);
                            if !inferred.is_empty() {
                                final_headers = inferred;
                            }
                        }
                        final_data = Vec::new();
                    }
                }
            }
            Ok(Err(e)) => {
                return Err(match postgres_error_location(&e, trimmed) {
                    Some(location) => {
                        QueryExecutionError::Located(format!("PostgreSQL error: {}", e), location)
                    }
                    None => QueryExecutionError::from_sqlx_with_context("PostgreSQL error: ", e),
                });
            }
            Err(_) => {
                // Drop future tidak menghentikan query di server; kirim
                // pg_cancel_backend lewat koneksi lain dari pool.
                let pid = super::pool::lock_or_recover(&options.backend_pids)
                    .get(&options.job_id)
                    .copied();
                // Koneksi ini masih menunggu hasil query yang dibatalkan;
                // lepaskan (tutup) supaya tidak dikembalikan ke pool.
                drop(conn.detach());
                if let Some(pid) = pid {
                    cancel_backend_query(
                        models::enums::DatabasePool::PostgreSQL(pg_pool.clone()),
                        pid,
                    )
                    .await;
                }
                return Err(QueryExecutionError::Timeout);
            }
        }
    }

    // Akhiri transaksi read-only. Koneksi yang gagal di-rollback tidak
    // disimpan: saat dilepas, `after_release` me-reset sesinya.
    if read_only_tx_open
        && let Err(e) = sqlx::raw_sql("ROLLBACK").execute(&mut *conn).await
    {
        return Err(QueryExecutionError::from_sqlx_with_context(
            "Cannot close the read-only transaction: ",
            e,
        ));
    }

    // Statement berikutnya di batch yang sama memakai koneksi ini. Pada jalur
    // error di atas koneksi dilepas, dan `after_release` me-reset sesinya.
    batch_connection.postgres = Some(conn);

    Ok(QueryJobOutput {
        headers: final_headers,
        rows: final_data,
        ast_debug_sql,
        ast_headers,
        column_metadata: None,
        affected_rows: final_affected,
        truncated: final_truncated,
    })
}

/// Jalankan job di engine plugin. Engine SQL dipecah per statement seperti
/// driver builtin; engine non-SQL menerima seluruh teks sebagai satu perintah.
/// Hasil statement terakhir yang ditampilkan.
async fn execute_plugin_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let models::enums::DatabasePool::Plugin(plugin_pool) = pool else {
        return Err(QueryExecutionError::Message(
            "Invalid pool type for plugin engine".to_string(),
        ));
    };
    let statements =
        if plugin_pool.capabilities.query_language == crate::driver_api::QueryLanguage::Sql {
            job_statements(options)
        } else {
            vec![options.query.trim().to_string()]
        };
    // Job id dicatat sebagai "pid" supaya tombol cancel diteruskan ke plugin.
    let _pid_guard = plugin_pool.capabilities.cancel.then(|| {
        BackendPidGuard::register(&options.backend_pids, options.job_id, options.job_id as i64)
    });

    let mut output = QueryJobOutput {
        headers: Vec::new(),
        rows: Vec::new(),
        ast_debug_sql: None,
        ast_headers: None,
        column_metadata: None,
        affected_rows: None,
        truncated: false,
    };
    for statement in statements {
        let request = crate::driver_api::ExecuteRequest {
            query: statement,
            database: options.selected_database.clone(),
            schema: options.schema_name.clone(),
            max_rows: options.max_rows,
            job_id: options.job_id,
        };
        let session = plugin_pool.session.clone();
        let outcome = run_with_timeout(
            options.query_timeout,
            crate::driver_api::run_blocking(move || session.execute(&request)),
        )
        .await;
        match outcome {
            Ok(Ok(result)) => {
                let affected = result.affected_rows;
                let (headers, rows, truncated) = result.into_table_rows(options.max_rows);
                output.headers = headers;
                output.rows = rows;
                output.affected_rows = affected;
                output.truncated = truncated;
            }
            Ok(Err(e)) => return Err(QueryExecutionError::Message(e.to_string())),
            Err(()) => {
                if plugin_pool.capabilities.cancel {
                    let session = plugin_pool.session.clone();
                    let job_id = options.job_id;
                    let _ = crate::driver_api::run_blocking(move || session.cancel(job_id)).await;
                }
                return Err(QueryExecutionError::Message(timeout_message(options)));
            }
        }
    }
    Ok(output)
}

async fn execute_sqlite_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
    batch_connection: &mut BatchConnection,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let sqlite_pool = match pool {
        models::enums::DatabasePool::SQLite(p) => p,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for SQLite".to_string(),
            ));
        }
    };

    let statements_owned = job_statements(options);
    let statements_raw: Vec<&str> = statements_owned.iter().map(|s| s.as_str()).collect();

    #[cfg(feature = "query_ast")]
    let mut inferred_headers_from_ast: Option<Vec<String>> = None;
    let mut ast_headers: Option<Vec<String>> = None;
    let mut ast_debug_sql: Option<String> = None;

    #[cfg(feature = "query_ast")]
    let statements: Vec<String> = {
        let allow_ast_rewrite = options.ast_enabled
            && statements_raw.len() == 1
            && statements_raw[0]
                .trim_start()
                .to_uppercase()
                .starts_with("SELECT")
            && is_simple_select_statement(statements_raw[0]);

        if allow_ast_rewrite {
            let should_paginate =
                options.use_server_pagination && !query_contains_pagination(statements_raw[0]);
            let pagination_opt = if should_paginate {
                Some((options.current_page as u64, options.page_size as u64))
            } else {
                None
            };
            let inject_auto_limit = should_paginate;
            match crate::query_ast::compile_single_select(
                statements_raw[0],
                &options.connection.connection_type,
                pagination_opt,
                inject_auto_limit,
            ) {
                Ok((new_sql, hdrs)) => {
                    if !hdrs.is_empty() {
                        inferred_headers_from_ast = Some(hdrs.clone());
                        ast_headers = Some(hdrs.clone());
                    }
                    ast_debug_sql = Some(new_sql.clone());
                    vec![new_sql]
                }
                Err(_) => statements_raw.iter().map(|s| s.to_string()).collect(),
            }
        } else {
            statements_raw.iter().map(|s| s.to_string()).collect()
        }
    };
    #[cfg(not(feature = "query_ast"))]
    let statements: Vec<String> = statements_raw.iter().map(|s| s.to_string()).collect();
    #[cfg(feature = "query_ast")]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();
    #[cfg(not(feature = "query_ast"))]
    let statements_ref: Vec<&str> = statements.iter().map(|s| s.as_str()).collect();

    // Satu koneksi untuk seluruh job. Menjalankan tiap statement langsung di
    // pool bisa mendarat di koneksi berbeda: `BEGIN` di koneksi A, `INSERT` di
    // koneksi B (autocommit), `COMMIT` di koneksi C (error), sementara koneksi
    // A kembali ke pool sambil menahan transaksi.
    //
    // URUTAN DEKLARASI PENTING: `conn` harus dideklarasikan sebelum
    // `SqliteStatementGuard` mana pun, supaya saat future ini di-drop (cancel)
    // guard di-drop lebih dulu selagi koneksi masih terbuka.
    let mut conn = match batch_connection.sqlite.take() {
        Some(conn) => conn,
        None => sqlite_pool.acquire().await.map_err(|e| {
            QueryExecutionError::from_sqlx_with_context("SQLite connection error: ", e)
        })?,
    };
    let interrupt = SqliteInterruptHandle::for_connection(&mut conn).await;

    // Jalur baca agent: `query_only` membuat SQLite menolak setiap tulisan yang
    // lolos dari classifier. Dimatikan lagi di SEMUA jalur keluar di bawah;
    // bila future ini di-drop di tengah jalan, `after_release` pool yang
    // mematikannya (lihat `reset_sqlite_session`).
    if options.read_only {
        sqlx::raw_sql("PRAGMA query_only=ON")
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                QueryExecutionError::from_sqlx_with_context("Cannot enable read-only mode: ", e)
            })?;
    }

    let mut final_headers = Vec::new();
    let mut final_data = Vec::new();
    let mut final_affected: Option<u64> = None;
    let mut final_truncated = false;

    for (i, statement) in statements_ref.iter().enumerate() {
        let trimmed = statement.trim();
        if is_comment_only_statement(trimmed) {
            continue;
        }

        let returns_rows = statement_returns_rows(trimmed);
        // Bila future ini di-drop di tengah statement (cancel) atau timeout
        // tercapai, guard memanggil `sqlite3_interrupt` sehingga worker sqlx
        // berhenti dan koneksi cepat kembali ke pool.
        let statement_guard = SqliteStatementGuard::new(interrupt);
        let result = run_with_timeout(options.query_timeout, async {
            if returns_rows {
                fetch_rows_limited!(
                    sqlx::query(sqlx::AssertSqlSafe(trimmed)),
                    &mut *conn,
                    options.max_rows
                )
                .await
                .map(|(rows, truncated)| (rows, truncated, None))
            } else {
                sqlx::query(sqlx::AssertSqlSafe(trimmed))
                    .execute(&mut *conn)
                    .await
                    .map(|r| (Vec::new(), false, Some(r.rows_affected())))
            }
        })
        .await;
        if result.is_ok() {
            // Selesai (sukses atau error SQL): tidak ada yang perlu dihentikan.
            statement_guard.finish();
        } else {
            // Timeout: drop guard = interrupt, selagi `conn` masih hidup.
            drop(statement_guard);
        }

        match result {
            Ok(Ok((rows, truncated, affected))) => {
                if i == statements_ref.len() - 1 {
                    final_affected = affected;
                    final_truncated = truncated;
                    if !rows.is_empty() {
                        final_headers = rows[0]
                            .columns()
                            .iter()
                            .map(|c| c.name().to_string())
                            .collect();
                        final_data = driver_sqlite::convert_sqlite_rows_to_table_data(rows);
                    } else {
                        #[cfg(feature = "query_ast")]
                        if final_headers.is_empty()
                            && let Some(hh) = inferred_headers_from_ast.clone()
                            && !hh.is_empty()
                        {
                            final_headers = hh;
                        }
                        if final_headers.is_empty()
                            && strip_leading_sql_comments(trimmed)
                                .to_uppercase()
                                .starts_with("SELECT")
                        {
                            let inferred = infer_select_headers(trimmed);
                            if !inferred.is_empty() {
                                final_headers = inferred;
                            }
                        }
                        final_data = Vec::new();
                    }
                }
            }
            Ok(Err(e)) => {
                if options.read_only {
                    drop(end_sqlite_read_only(conn).await);
                }
                return Err(QueryExecutionError::from_sqlx_with_context(
                    "SQLite error: ",
                    e,
                ));
            }
            Err(_) => {
                if options.read_only {
                    drop(end_sqlite_read_only(conn).await);
                }
                return Err(QueryExecutionError::Timeout);
            }
        }
    }

    let conn = if options.read_only {
        match end_sqlite_read_only(conn).await {
            Some(conn) => conn,
            None => {
                return Err(QueryExecutionError::Message(
                    "SQLite connection could not leave read-only mode and was closed".to_string(),
                ));
            }
        }
    } else {
        conn
    };

    // Statement berikutnya di batch yang sama memakai koneksi ini. Pada jalur
    // error di atas koneksi dilepas, dan `after_release` me-rollback transaksi
    // yang tertinggal.
    batch_connection.sqlite = Some(conn);

    Ok(QueryJobOutput {
        headers: final_headers,
        rows: final_data,
        ast_debug_sql,
        ast_headers,
        column_metadata: None,
        affected_rows: final_affected,
        truncated: final_truncated,
    })
}

/// Matikan `PRAGMA query_only` setelah job read-only. Koneksi yang tidak bisa
/// dikembalikan ke mode tulis ditutup (bukan dikembalikan ke pool), supaya
/// pemakai berikutnya tidak mewarisi koneksi yang menolak semua tulisan.
async fn end_sqlite_read_only(
    mut conn: PoolConnection<sqlx::Sqlite>,
) -> Option<PoolConnection<sqlx::Sqlite>> {
    let reset = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        sqlx::raw_sql("PRAGMA query_only=OFF").execute(&mut *conn),
    )
    .await;
    match reset {
        Ok(Ok(_)) => Some(conn),
        Ok(Err(e)) => {
            log::warn!("[EXEC] SQLite query_only could not be turned off: {}", e);
            drop(conn.detach());
            None
        }
        Err(_) => {
            log::warn!("[EXEC] SQLite query_only reset timed out; closing the connection");
            drop(conn.detach());
            None
        }
    }
}

/// Batas waktu satu perintah Redis interaktif.
const REDIS_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Batas waktu total pemindaian `KEYS` (beberapa putaran `SCAN`).
const REDIS_SCAN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Error Redis dengan teks asli dari server/driver (mis. `WRONGTYPE`,
/// `NOAUTH`), bukan pesan generik "timed out or failed".
fn redis_command_error(command: &str, e: redis::RedisError) -> QueryExecutionError {
    let message = format!("Redis {} failed: {}", command, e);
    if e.is_io_error() || e.is_connection_dropped() || e.is_connection_refusal() {
        QueryExecutionError::Connection(message)
    } else {
        QueryExecutionError::Message(message)
    }
}

fn redis_command_timeout(command: &str) -> QueryExecutionError {
    QueryExecutionError::Message(format!(
        "Redis {} timed out after {}s",
        command,
        REDIS_COMMAND_TIMEOUT.as_secs()
    ))
}

/// Kumpulkan key yang cocok dengan `pattern` lewat `SCAN` bertahap, paling
/// banyak `max_keys`. Mengembalikan `(keys, terpotong)`.
async fn scan_redis_keys(
    connection: &mut redis::aio::ConnectionManager,
    pattern: &str,
    max_keys: usize,
) -> Result<(Vec<String>, bool), redis::RedisError> {
    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cursor: u64 = 0;
    loop {
        let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(1000)
            .query_async(connection)
            .await?;
        for key in batch {
            // SCAN boleh mengembalikan key yang sama lebih dari sekali.
            if !seen.insert(key.clone()) {
                continue;
            }
            if keys.len() >= max_keys {
                return Ok((keys, true));
            }
            keys.push(key);
        }
        cursor = next_cursor;
        if cursor == 0 {
            return Ok((keys, false));
        }
    }
}

async fn execute_redis_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    use redis::AsyncCommands;

    let redis_manager = match pool {
        models::enums::DatabasePool::Redis(manager) => manager,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for Redis".to_string(),
            ));
        }
    };

    let command_line = options.query.trim();
    if command_line.is_empty() {
        return Err(QueryExecutionError::Message(
            "Empty Redis command".to_string(),
        ));
    }

    // Jangan `SELECT` pada manager bersama: semua clone-nya berbagi satu
    // soket, jadi `SELECT 3` di sini mengganti db untuk tab lain dan worker
    // sidebar, dan reconnect diam-diam kembali ke db 0. Job yang menunjuk db
    // tertentu memakai manager khusus yang indeks db-nya ada di info koneksi;
    // kegagalan membukanya dilaporkan, tidak diabaikan.
    let requested_db = options
        .selected_database
        .as_deref()
        .and_then(crate::driver_redis::parse_redis_db_index)
        .or_else(|| crate::driver_redis::parse_redis_db_index(&options.connection.database));
    let mut connection = match requested_db {
        Some(db_index) => super::pool::redis_manager_for_db(&options.connection, db_index)
            .await
            .map_err(|e| {
                QueryExecutionError::Connection(format!(
                    "Cannot open Redis database {}: {}",
                    db_index, e
                ))
            })?,
        None => redis_manager.as_ref().clone(),
    };

    debug!("[async] Executing Redis command: {}", command_line);

    let parts: Vec<&str> = command_line.split_whitespace().collect();
    if parts.is_empty() {
        return Err(QueryExecutionError::Message(
            "Empty Redis command".to_string(),
        ));
    }

    let command = parts[0].to_uppercase();
    match command.as_str() {
        "GET" => {
            if parts.len() != 2 {
                return Err(QueryExecutionError::Message(
                    "GET requires exactly one key".to_string(),
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                connection.get::<&str, Option<String>>(parts[1]),
            )
            .await
            {
                Ok(Ok(Some(value))) => Ok(QueryJobOutput {
                    headers: vec!["Key".to_string(), "Value".to_string()],
                    rows: vec![vec![parts[1].to_string(), value]],
                    ast_debug_sql: None,
                    ast_headers: None,
                    column_metadata: None,
                    affected_rows: None,
                    truncated: false,
                }),
                Ok(Ok(None)) => Ok(QueryJobOutput {
                    headers: vec!["Key".to_string(), "Value".to_string()],
                    rows: vec![vec![parts[1].to_string(), "NULL".to_string()]],
                    ast_debug_sql: None,
                    ast_headers: None,
                    column_metadata: None,
                    affected_rows: None,
                    truncated: false,
                }),
                Ok(Err(e)) => Err(redis_command_error("GET", e)),
                Err(_) => Err(redis_command_timeout("GET")),
            }
        }
        "KEYS" => {
            if parts.len() != 2 {
                return Err(QueryExecutionError::Message(
                    "KEYS requires exactly one pattern".to_string(),
                ));
            }
            // `KEYS pattern` memblokir server (single-thread) selama memindai
            // seluruh keyspace dan mengembalikan semua key sekaligus. Hasil
            // yang sama dikumpulkan lewat `SCAN` bertahap, dibatasi `max_rows`.
            let max_keys = options.max_rows.max(1);
            match tokio::time::timeout(
                REDIS_SCAN_TIMEOUT,
                scan_redis_keys(&mut connection, parts[1], max_keys),
            )
            .await
            {
                Ok(Ok((keys, truncated))) => {
                    let table_data: Vec<Vec<String>> = keys.into_iter().map(|k| vec![k]).collect();
                    Ok(QueryJobOutput {
                        headers: vec!["Key".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                        column_metadata: None,
                        affected_rows: None,
                        truncated,
                    })
                }
                Ok(Err(e)) => Err(redis_command_error("KEYS", e)),
                Err(_) => Err(redis_command_timeout("KEYS")),
            }
        }
        "SCAN" => {
            if parts.len() < 2 {
                return Err(QueryExecutionError::Message(
                    "SCAN requires cursor parameter".to_string(),
                ));
            }
            let cursor = parts[1];
            let mut match_pattern = "*";
            let mut count: i64 = 10;
            let mut idx = 2;
            while idx < parts.len() {
                match parts[idx].to_uppercase().as_str() {
                    "MATCH" => {
                        if idx + 1 < parts.len() {
                            match_pattern = parts[idx + 1];
                            idx += 2;
                        } else {
                            return Err(QueryExecutionError::Message(
                                "MATCH requires a pattern".to_string(),
                            ));
                        }
                    }
                    "COUNT" => {
                        if idx + 1 < parts.len() {
                            if let Ok(parsed) = parts[idx + 1].parse::<i64>() {
                                count = parsed;
                                idx += 2;
                            } else {
                                return Err(QueryExecutionError::Message(
                                    "COUNT must be a number".to_string(),
                                ));
                            }
                        } else {
                            return Err(QueryExecutionError::Message(
                                "COUNT requires a number".to_string(),
                            ));
                        }
                    }
                    other => {
                        return Err(QueryExecutionError::Message(format!(
                            "Unknown SCAN parameter: {}",
                            other
                        )));
                    }
                }
            }

            let mut cmd = redis::cmd("SCAN");
            cmd.arg(cursor);
            if match_pattern != "*" {
                cmd.arg("MATCH").arg(match_pattern);
            }
            cmd.arg("COUNT").arg(count);

            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                cmd.query_async::<(String, Vec<String>)>(&mut connection),
            )
            .await
            {
                Ok(Ok((next_cursor, keys))) => {
                    let mut table_data = Vec::new();
                    if keys.is_empty() {
                        table_data.push(vec![
                            "Info".to_string(),
                            format!("No keys found matching pattern: {}", match_pattern),
                        ]);
                        table_data.push(vec!["Cursor".to_string(), next_cursor.clone()]);
                        table_data.push(vec![
                            "Suggestion".to_string(),
                            "Try different pattern or use 'SCAN 0 COUNT 100' to see all keys"
                                .to_string(),
                        ]);
                        if match_pattern != "*"
                            && let Ok((_, sample_keys)) = redis::cmd("SCAN")
                                .arg("0")
                                .arg("COUNT")
                                .arg("10")
                                .query_async::<(String, Vec<String>)>(&mut connection)
                                .await
                            && !sample_keys.is_empty()
                        {
                            table_data.push(vec!["Sample Keys Found".to_string(), "".to_string()]);
                            for (i, key) in sample_keys.iter().take(5).enumerate() {
                                table_data.push(vec![format!("Sample {}", i + 1), key.clone()]);
                            }
                        }
                    } else {
                        table_data.push(vec!["CURSOR".to_string(), next_cursor]);
                        for key in keys {
                            table_data.push(vec!["KEY".to_string(), key]);
                        }
                    }
                    Ok(QueryJobOutput {
                        headers: vec!["Type".to_string(), "Value".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
                    })
                }
                Ok(Err(e)) => Err(redis_command_error("SCAN", e)),
                Err(_) => Err(redis_command_timeout("SCAN")),
            }
        }
        "INFO" => {
            let section = if parts.len() > 1 { parts[1] } else { "default" };
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                redis::cmd("INFO")
                    .arg(section)
                    .query_async::<String>(&mut connection),
            )
            .await
            {
                Ok(Ok(info_result)) => {
                    let mut table_data = Vec::new();
                    for line in info_result.lines() {
                        if line.trim().is_empty() || line.starts_with('#') {
                            continue;
                        }
                        if let Some((key, value)) = line.split_once(':') {
                            table_data.push(vec![key.to_string(), value.to_string()]);
                        }
                    }
                    Ok(QueryJobOutput {
                        headers: vec!["Property".to_string(), "Value".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
                    })
                }
                Ok(Err(e)) => Err(redis_command_error("INFO", e)),
                Err(_) => Err(redis_command_timeout("INFO")),
            }
        }
        "HGETALL" => {
            if parts.len() != 2 {
                return Err(QueryExecutionError::Message(
                    "HGETALL requires exactly one key".to_string(),
                ));
            }
            match tokio::time::timeout(
                std::time::Duration::from_secs(10),
                redis::cmd("HGETALL")
                    .arg(parts[1])
                    .query_async::<Vec<String>>(&mut connection),
            )
            .await
            {
                Ok(Ok(hash_data)) => {
                    let mut table_data = Vec::new();
                    for chunk in hash_data.chunks(2) {
                        if chunk.len() == 2 {
                            table_data.push(vec![chunk[0].clone(), chunk[1].clone()]);
                        }
                    }
                    if table_data.is_empty() {
                        table_data.push(vec![
                            "No data".to_string(),
                            "Hash is empty or key does not exist".to_string(),
                        ]);
                    }
                    Ok(QueryJobOutput {
                        headers: vec!["Field".to_string(), "Value".to_string()],
                        rows: table_data,
                        ast_debug_sql: None,
                        ast_headers: None,
                        column_metadata: None,
                        affected_rows: None,
                        truncated: false,
                    })
                }
                Ok(Err(e)) => Err(redis_command_error("HGETALL", e)),
                Err(_) => Err(redis_command_timeout("HGETALL")),
            }
        }
        _ => Err(QueryExecutionError::Message(format!(
            "Unsupported Redis command: {}",
            parts[0]
        ))),
    }
}

async fn execute_mssql_query_job(
    options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    let config = match pool {
        models::enums::DatabasePool::MsSQL(cfg) => cfg,
        _ => {
            return Err(QueryExecutionError::Message(
                "Invalid pool type for MsSQL".to_string(),
            ));
        }
    };

    let query_str = options.query.trim().to_string();
    if query_str.is_empty() {
        return Err(QueryExecutionError::Message(
            "Empty MsSQL query".to_string(),
        ));
    }

    // Sama seperti engine lain: berhenti di `max_rows`, tandai terpotong, dan
    // hormati batas waktu query (versi lama tidak punya keduanya di jalur ini).
    match driver_mssql::execute_query_bounded(
        config.clone(),
        &query_str,
        options.max_rows.max(1),
        options.query_timeout,
    )
    .await
    {
        Ok((headers, rows, truncated)) => Ok(QueryJobOutput {
            headers,
            rows,
            ast_debug_sql: None,
            ast_headers: None,
            column_metadata: None,
            affected_rows: None,
            truncated,
        }),
        Err(e) => Err(mssql_execution_error(e)),
    }
}

async fn execute_mongodb_query_job(
    _options: &QueryExecutionOptions,
    pool: models::enums::DatabasePool,
) -> Result<QueryJobOutput, QueryExecutionError> {
    match pool {
        models::enums::DatabasePool::MongoDB(_) => Ok(QueryJobOutput {
            headers: vec!["Info".to_string()],
            rows: vec![vec![
                "MongoDB query execution is not supported. Use tree to browse collections."
                    .to_string(),
            ]],
            ast_debug_sql: None,
            ast_headers: None,
            column_metadata: None,
            affected_rows: None,
            truncated: false,
        }),
        _ => Err(QueryExecutionError::Message(
            "Invalid pool type for MongoDB".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    async fn sqlite_job(query: &str, max_rows: usize) -> QueryResultMessage {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        for stmt in [
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO t (name) VALUES ('a'), ('b;c'), ('d')",
        ] {
            sqlx::query(stmt).execute(&pool).await.expect("seed");
        }
        let connection = models::structs::ConnectionConfig {
            connection_type: models::enums::DatabaseType::SQLite,
            ..Default::default()
        };
        let job = QueryJob {
            job_id: 7,
            tab_id: Some(3),
            options: QueryExecutionOptions {
                connection_id: 1,
                connection,
                query: query.to_string(),
                selected_database: None,
                schema_name: None,
                use_server_pagination: false,
                current_page: 0,
                page_size: 100,
                base_query: None,
                dba_special_mode: None,
                save_to_history: false,
                split_result_sets: false,
                ast_enabled: false,
                job_id: 7,
                query_timeout: None,
                max_rows,
                backend_pids: Default::default(),
                read_only: false,
            },
            connection_pool: models::enums::DatabasePool::SQLite(Arc::new(pool)),
            started_at: Instant::now(),
            on_result: None,
        };
        execute_query_job(job).await
    }

    /// Job SQLite di atas pool yang sudah ada, supaya beberapa job bisa
    /// berbagi pool yang sama.
    fn sqlite_job_on(
        pool: &Arc<sqlx::SqlitePool>,
        query: &str,
        timeout: Option<std::time::Duration>,
    ) -> QueryJob {
        QueryJob {
            job_id: 11,
            tab_id: Some(5),
            options: QueryExecutionOptions {
                connection_id: 3,
                connection: models::structs::ConnectionConfig {
                    connection_type: models::enums::DatabaseType::SQLite,
                    ..Default::default()
                },
                query: query.to_string(),
                selected_database: None,
                schema_name: None,
                use_server_pagination: false,
                current_page: 0,
                page_size: 100,
                base_query: None,
                dba_special_mode: None,
                save_to_history: false,
                split_result_sets: false,
                ast_enabled: false,
                job_id: 11,
                query_timeout: timeout,
                max_rows: 100,
                backend_pids: Default::default(),
                read_only: false,
            },
            connection_pool: models::enums::DatabasePool::SQLite(pool.clone()),
            started_at: Instant::now(),
            on_result: None,
        }
    }

    /// Database SQLite berbasis file sementara dengan pool multi-koneksi dan
    /// hook `after_release` aplikasi. File dihapus saat di-drop.
    struct TempSqlite {
        path: std::path::PathBuf,
        pool: Arc<sqlx::SqlitePool>,
    }

    impl TempSqlite {
        async fn new(name: &str, max_connections: u32) -> Self {
            let path = std::env::temp_dir().join(format!(
                "tabular_exec_{}_{}.sqlite",
                std::process::id(),
                name
            ));
            let _ = std::fs::remove_file(&path);
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                // Gagal cepat bila masih ada lock tertinggal, bukan menunggu.
                .busy_timeout(std::time::Duration::from_millis(500));
            let pool = crate::connection::pool::sqlite_pool_options()
                .min_connections(0)
                .max_connections(max_connections)
                .connect_with(options)
                .await
                .expect("temp sqlite");
            sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)")
                .execute(&pool)
                .await
                .expect("seed");
            Self {
                path,
                pool: Arc::new(pool),
            }
        }

        async fn run(&self, query: &str) -> QueryResultMessage {
            execute_query_job(sqlite_job_on(&self.pool, query, None)).await
        }

        async fn count(&self) -> String {
            let msg = self.run("SELECT count(*) FROM t").await;
            assert!(msg.success, "{:?}", msg.error);
            msg.rows[0][0].clone()
        }
    }

    impl Drop for TempSqlite {
        fn drop(&mut self) {
            for suffix in ["", "-wal", "-shm", "-journal"] {
                let mut file = self.path.clone().into_os_string();
                file.push(suffix);
                let _ = std::fs::remove_file(file);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_only_job_rejects_writes_and_leaves_the_connection_writable() {
        // Satu koneksi di pool: job berikutnya pasti memakai koneksi yang sama,
        // jadi `query_only` yang tertinggal akan langsung ketahuan.
        let db = TempSqlite::new("read_only_job", 1).await;
        let read_only = |query: &str| {
            let mut job = sqlite_job_on(&db.pool, query, None);
            job.options.read_only = true;
            job
        };

        // Tulisan langsung ditolak engine.
        let msg = execute_query_job(read_only("INSERT INTO t (name) VALUES ('x')")).await;
        assert!(!msg.success, "write must fail in a read-only job");
        assert!(
            msg.error.as_deref().unwrap_or_default().contains("readonly"),
            "{:?}",
            msg.error
        );

        // Tulisan yang diselundupkan di belakang SELECT juga ditolak.
        let msg =
            execute_query_job(read_only("SELECT 1; DELETE FROM t; SELECT count(*) FROM t")).await;
        assert!(!msg.success, "smuggled write must fail");

        // Baca tetap jalan.
        let msg = execute_query_job(read_only("SELECT count(*) FROM t")).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.rows[0][0], "0");

        // Job biasa sesudahnya (jalur sukses maupun gagal di atas) bisa menulis.
        let msg = db.run("INSERT INTO t (name) VALUES ('y')").await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(db.count().await, "1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn read_only_job_dropped_midway_is_reset_by_the_pool_hook() {
        let db = TempSqlite::new("read_only_dropped", 1).await;
        // Tiru job read-only yang di-drop sebelum sempat mematikan `query_only`.
        {
            let mut conn = db.pool.acquire().await.expect("acquire");
            sqlx::raw_sql("PRAGMA query_only=ON")
                .execute(&mut *conn)
                .await
                .expect("pragma");
        }
        let msg = db.run("INSERT INTO t (name) VALUES ('z')").await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(db.count().await, "1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn transaction_script_runs_on_a_single_connection() {
        // Dengan beberapa koneksi di pool, statement yang dijalankan terpisah
        // di pool akan mendarat di koneksi berbeda dan COMMIT gagal.
        let db = TempSqlite::new("tx_single_conn", 3).await;
        let msg = db
            .run("BEGIN; INSERT INTO t (name) VALUES ('a'); INSERT INTO t (name) VALUES ('b'); COMMIT;")
            .await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(db.count().await, "2");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn open_transaction_does_not_leak_into_the_pool() {
        let db = TempSqlite::new("tx_leak", 2).await;
        // BEGIN tanpa COMMIT: transaksi tulis masih terbuka saat job selesai.
        let msg = db.run("BEGIN; INSERT INTO t (name) VALUES ('lost')").await;
        assert!(msg.success, "{:?}", msg.error);

        // Job berikutnya tidak boleh terkena "database is locked", di koneksi
        // mana pun ia mendarat.
        for name in ["x", "y", "z"] {
            let msg = db
                .run(&format!("INSERT INTO t (name) VALUES ('{name}')"))
                .await;
            assert!(msg.success, "{:?}", msg.error);
        }
        // Baris dari transaksi yang ditinggalkan ikut ter-rollback, dan koneksi
        // bekasnya bisa memulai transaksi baru.
        assert_eq!(db.count().await, "3");
        let msg = db.run("BEGIN; DELETE FROM t; COMMIT").await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(db.count().await, "0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batch_statements_share_one_connection() {
        // Editor mengirim script sebagai satu job per statement.
        let db = TempSqlite::new("batch_shared", 3).await;
        let mut batch = BatchConnection::default();
        for stmt in [
            "BEGIN",
            "INSERT INTO t (name) VALUES ('a')",
            "INSERT INTO t (name) VALUES ('b')",
            "ROLLBACK",
        ] {
            let msg = execute_query_job_in(sqlite_job_on(&db.pool, stmt, None), &mut batch).await;
            assert!(msg.success, "{stmt}: {:?}", msg.error);
        }
        drop(batch);
        // ROLLBACK membatalkan kedua INSERT: semuanya berjalan di satu koneksi.
        assert_eq!(db.count().await, "0");

        let mut batch = BatchConnection::default();
        for stmt in ["BEGIN", "INSERT INTO t (name) VALUES ('c')", "COMMIT"] {
            let msg = execute_query_job_in(sqlite_job_on(&db.pool, stmt, None), &mut batch).await;
            assert!(msg.success, "{stmt}: {:?}", msg.error);
        }
        drop(batch);
        assert_eq!(db.count().await, "1");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_batch_statement_releases_the_connection() {
        let db = TempSqlite::new("batch_failed", 2).await;
        let mut batch = BatchConnection::default();
        for stmt in ["BEGIN", "INSERT INTO t (name) VALUES ('a')"] {
            let msg = execute_query_job_in(sqlite_job_on(&db.pool, stmt, None), &mut batch).await;
            assert!(msg.success, "{stmt}: {:?}", msg.error);
        }
        let msg = execute_query_job_in(
            sqlite_job_on(&db.pool, "INSERT INTO missing_table VALUES (1)", None),
            &mut batch,
        )
        .await;
        assert!(!msg.success);
        // Koneksi tidak disimpan lagi, jadi transaksi yang terbuka di-rollback.
        assert!(batch.sqlite.is_none());
        drop(batch);
        assert_eq!(db.count().await, "0");
    }

    /// Query yang tidak pernah selesai sendiri.
    const ENDLESS_QUERY: &str =
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c";

    #[tokio::test(flavor = "multi_thread")]
    async fn timed_out_statement_is_interrupted_and_frees_the_connection() {
        // Satu koneksi saja: bila statement tidak di-interrupt, job berikutnya
        // tidak akan pernah mendapat koneksi.
        let db = TempSqlite::new("timeout_interrupt", 1).await;
        let msg = execute_query_job(sqlite_job_on(
            &db.pool,
            ENDLESS_QUERY,
            Some(std::time::Duration::from_millis(300)),
        ))
        .await;
        assert!(!msg.success);
        assert!(
            msg.error.clone().unwrap_or_default().contains("timed out"),
            "{:?}",
            msg.error
        );

        let next = tokio::time::timeout(std::time::Duration::from_secs(8), db.run("SELECT 1"))
            .await
            .expect("connection must be released after the interrupt");
        assert!(next.success, "{:?}", next.error);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_job_interrupts_the_running_statement() {
        let db = TempSqlite::new("cancel_interrupt", 1).await;
        let task = tokio::spawn(execute_query_job(sqlite_job_on(
            &db.pool,
            ENDLESS_QUERY,
            None,
        )));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // Sama seperti tombol cancel di UI: task di-abort.
        task.abort();
        assert!(task.await.is_err());

        let next = tokio::time::timeout(std::time::Duration::from_secs(8), db.run("SELECT 1"))
            .await
            .expect("connection must be released after cancel");
        assert!(next.success, "{:?}", next.error);
    }

    /// Indeks db aktif sebuah koneksi Redis menurut server (`CLIENT INFO`).
    async fn redis_current_db(conn: &mut redis::aio::ConnectionManager) -> String {
        let info: String = redis::cmd("CLIENT")
            .arg("INFO")
            .query_async(conn)
            .await
            .expect("CLIENT INFO");
        info.split_whitespace()
            .find_map(|field| field.strip_prefix("db="))
            .unwrap_or("?")
            .to_string()
    }

    /// Uji integrasi terhadap Redis sungguhan. Hanya perintah baca (`CLIENT
    /// INFO`, `INFO`, `SCAN`); tidak menulis key apa pun. Dijalankan manual:
    /// `TABULAR_TEST_REDIS=127.0.0.1:6379 cargo test --lib redis_live -- --ignored`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs a live Redis; set TABULAR_TEST_REDIS=host:port"]
    async fn redis_live_job_db_selection_does_not_leak_to_the_shared_manager() {
        let Ok(target) = std::env::var("TABULAR_TEST_REDIS") else {
            return;
        };
        let (host, port) = target.rsplit_once(':').expect("TABULAR_TEST_REDIS=host:port");

        let shared = crate::driver_redis::open_redis_manager(host, port, "", "", None)
            .await
            .expect("shared manager");
        let mut observer = shared.clone();
        assert_eq!(redis_current_db(&mut observer).await, "0");

        // Job yang menunjuk db3, lewat jalur eksekusi sungguhan.
        let pool: Arc<sqlx::SqlitePool> = Arc::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .connect_lazy("sqlite::memory:")
                .expect("lazy pool"),
        );
        let mut job = sqlite_job_on(&pool, "INFO server", None);
        job.options.connection = models::structs::ConnectionConfig {
            connection_type: models::enums::DatabaseType::Redis,
            host: host.to_string(),
            port: port.to_string(),
            ..Default::default()
        };
        job.options.selected_database = Some("db3".to_string());
        job.connection_pool = models::enums::DatabasePool::Redis(Arc::new(shared.clone()));
        let msg = execute_query_job(job).await;
        assert!(msg.success, "{:?}", msg.error);
        assert!(!msg.rows.is_empty());

        // Manager bersama (dipakai tab lain dan sidebar) tetap di db 0.
        assert_eq!(redis_current_db(&mut observer).await, "0");

        // Manager khusus db benar-benar berada di db itu.
        let mut db3 = crate::driver_redis::open_redis_manager(host, port, "", "", Some(3))
            .await
            .expect("db3 manager");
        assert_eq!(redis_current_db(&mut db3).await, "3");

        // Pengganti KEYS: pemindaian SCAN sampai kursor habis.
        let (keys, truncated) =
            scan_redis_keys(&mut db3, "tabular-live-test-no-such-key-*", 10)
                .await
                .expect("scan");
        assert!(keys.is_empty());
        assert!(!truncated);

        // Error server diteruskan apa adanya, bukan "timed out or failed".
        let mut job = sqlite_job_on(&pool, "HGETALL", None);
        job.options.connection.connection_type = models::enums::DatabaseType::Redis;
        job.connection_pool = models::enums::DatabasePool::Redis(Arc::new(shared));
        let msg = execute_query_job(job).await;
        assert!(!msg.success);
        assert_eq!(
            msg.error.as_deref(),
            Some("HGETALL requires exactly one key")
        );
    }

    #[test]
    fn use_statement_is_recognised_without_slicing_into_characters() {
        assert_eq!(parse_use_statement("USE mydb"), Some("mydb".to_string()));
        assert_eq!(
            parse_use_statement("-- pilih\nuse `my db`;"),
            Some("my db".to_string())
        );
        assert_eq!(parse_use_statement("USE\t[dbo]"), Some("dbo".to_string()));
        assert_eq!(parse_use_statement("USE café"), Some("café".to_string()));
        assert_eq!(parse_use_statement("USER()"), None);
        assert_eq!(parse_use_statement("SELECT 1"), None);
        assert_eq!(parse_use_statement("USE"), None);
        assert_eq!(parse_use_statement("USE   "), None);
        // `ſ` (long s) menjadi `S` saat di-uppercase; dulu ini lolos sebagai
        // "USE" lalu dipotong dengan offset byte yang salah.
        assert_eq!(parse_use_statement("uſe x"), None);
        for input in crate::connection::sql::odd_sql_inputs() {
            let _ = parse_use_statement(&input);
            let _ = preview_text(&input, 3);
        }
    }

    #[tokio::test]
    async fn typed_errors_render_user_facing_messages() {
        let pool: Arc<sqlx::SqlitePool> = Arc::new(
            sqlx::sqlite::SqlitePoolOptions::new()
                .connect_lazy("sqlite::memory:")
                .expect("lazy pool"),
        );
        let mut job = sqlite_job_on(&pool, "SELECT 1", Some(std::time::Duration::from_secs(7)));
        let (message, location) =
            describe_execution_error(QueryExecutionError::Timeout, &job.options);
        assert!(message.contains("timed out after 7s"), "{message}");
        assert!(location.is_none());

        job.options.query_timeout = None;
        let (message, _) = describe_execution_error(
            QueryExecutionError::Connection("PostgreSQL connection error: reset".into()),
            &job.options,
        );
        assert_eq!(message, "PostgreSQL connection error: reset");

        let mapped = mssql_execution_error(driver_mssql::MssqlExecError::Connection("x".into()));
        assert!(mapped.is_connection());
        assert!(matches!(
            mssql_execution_error(driver_mssql::MssqlExecError::Timeout),
            QueryExecutionError::Timeout
        ));
        assert!(matches!(
            mssql_execution_error(driver_mssql::MssqlExecError::Query("bad".into())),
            QueryExecutionError::Message(m) if m == "Query error: bad"
        ));
    }

    /// Job yang sama tetapi lewat `DatabasePool::Plugin` (adapter SQLite di
    /// balik trait driver), untuk memastikan jalur generik plugin.
    async fn plugin_job(query: &str, max_rows: usize) -> QueryResultMessage {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite");
        for stmt in [
            "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)",
            "INSERT INTO t (name) VALUES ('a'), (NULL), ('d')",
        ] {
            sqlx::query(stmt).execute(&pool).await.expect("seed");
        }
        let session = crate::driver_api::sqlite_adapter::SqliteSession::from_pool(
            pool,
            tokio::runtime::Handle::current(),
        );
        let plugin_pool = crate::driver_api::PluginPool {
            engine_id: "sqlite-adapter".into(),
            capabilities: Default::default(),
            session: Arc::new(session),
        };
        let connection = models::structs::ConnectionConfig {
            connection_type: models::enums::DatabaseType::Plugin("sqlite-adapter".into()),
            ..Default::default()
        };
        let job = QueryJob {
            job_id: 8,
            tab_id: Some(4),
            options: QueryExecutionOptions {
                connection_id: 2,
                connection,
                query: query.to_string(),
                selected_database: None,
                schema_name: None,
                use_server_pagination: false,
                current_page: 0,
                page_size: 100,
                base_query: None,
                dba_special_mode: None,
                save_to_history: false,
                ast_enabled: false,
                job_id: 8,
                query_timeout: None,
                max_rows,
                backend_pids: Default::default(),
                split_result_sets: false,
                read_only: false,
            },
            connection_pool: models::enums::DatabasePool::Plugin(Arc::new(plugin_pool)),
            started_at: Instant::now(),
            on_result: None,
        };
        execute_query_job(job).await
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_engine_runs_statements_and_maps_nulls() {
        let msg = plugin_job(
            "UPDATE t SET name = 'z' WHERE id = 3; SELECT id, name FROM t ORDER BY id",
            2,
        )
        .await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.headers, vec!["id", "name"]);
        assert_eq!(
            msg.rows,
            vec![
                vec!["1".to_string(), "a".to_string()],
                vec!["2".to_string(), "NULL".to_string()]
            ]
        );
        assert!(msg.truncated);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_engine_reports_errors_and_affected_rows() {
        let msg = plugin_job("UPDATE t SET name = 'q' WHERE id > 1", 10).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.affected_rows, Some(2));

        let msg = plugin_job("SELECT * FROM missing_table", 10).await;
        assert!(!msg.success);
        assert!(msg.error.unwrap_or_default().contains("missing_table"));
    }

    #[tokio::test]
    async fn statement_with_leading_comment_is_executed() {
        let msg = sqlite_job("-- ambil semua\nSELECT name FROM t ORDER BY id", 100).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.tab_id, Some(3));
        assert_eq!(msg.rows.len(), 3);
        assert_eq!(msg.affected_rows, None);
    }

    #[tokio::test]
    async fn semicolon_inside_string_is_not_split() {
        let msg = sqlite_job("SELECT id FROM t WHERE name = 'b;c'", 100).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.rows, vec![vec!["2".to_string()]]);
    }

    #[tokio::test]
    async fn update_reports_driver_affected_rows() {
        let msg = sqlite_job("UPDATE t SET name = 'z' WHERE id >= 2", 100).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.affected_rows, Some(2));
        assert!(msg.rows.is_empty());
    }

    #[tokio::test]
    async fn result_set_is_truncated_at_row_limit() {
        let msg = sqlite_job("SELECT * FROM t", 2).await;
        assert!(msg.success, "{:?}", msg.error);
        assert_eq!(msg.rows.len(), 2);
        assert!(msg.truncated);
    }

    #[tokio::test]
    async fn sql_error_is_reported_as_failure() {
        let msg = sqlite_job("SELECT * FROM missing_table", 100).await;
        assert!(!msg.success);
        assert!(msg.error.unwrap_or_default().contains("missing_table"));
    }
}
