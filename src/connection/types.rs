use crate::models;
use std::time::Instant;

/// Id sesi di sisi server (backend pid PostgreSQL / connection id MySQL) untuk
/// setiap job query yang sedang berjalan, dengan key job id. Dipakai supaya
/// permintaan cancel benar-benar menghentikan statement di server, bukan hanya
/// meninggalkannya di sisi klien.
pub type BackendPidRegistry = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, i64>>>;

/// Menghapus backend pid sebuah job dari registry saat job selesai atau
/// task-nya di-abort.
pub struct BackendPidGuard {
    registry: BackendPidRegistry,
    job_id: u64,
}

impl BackendPidGuard {
    pub fn register(registry: &BackendPidRegistry, job_id: u64, pid: i64) -> Self {
        // Registry ter-poison tetap dipakai: kalau dilewati, tombol cancel
        // tidak lagi bisa menghentikan query di server.
        super::pool::lock_or_recover(registry).insert(job_id, pid);
        Self {
            registry: registry.clone(),
            job_id,
        }
    }
}

impl Drop for BackendPidGuard {
    fn drop(&mut self) {
        super::pool::lock_or_recover(&self.registry).remove(&self.job_id);
    }
}

#[derive(Clone, Debug)]
pub struct QueryExecutionOptions {
    pub connection_id: i64,
    pub connection: models::structs::ConnectionConfig,
    pub query: String,
    pub selected_database: Option<String>,
    /// Schema aktif tab (PostgreSQL): diterapkan sebagai `search_path` pada
    /// koneksi yang menjalankan query.
    pub schema_name: Option<String>,
    pub use_server_pagination: bool,
    pub current_page: usize,
    pub page_size: usize,
    pub base_query: Option<String>,
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub save_to_history: bool,
    /// Kirim satu hasil per result set (MsSQL multi-SELECT). Dimatikan untuk
    /// job ber-callback dan agent yang mengharapkan tepat satu hasil.
    pub split_result_sets: bool,
    pub ast_enabled: bool,
    pub job_id: u64,
    /// Batalkan statement setelah durasi ini (None = tanpa batas).
    pub query_timeout: Option<std::time::Duration>,
    /// Berhenti membaca result set setelah jumlah baris ini.
    pub max_rows: usize,
    pub backend_pids: BackendPidRegistry,
    /// Pertahanan berlapis untuk jalur baca agent: statement dijalankan dalam
    /// transaksi read-only (PostgreSQL/MySQL) atau dengan `query_only`
    /// (SQLite), sehingga tulisan yang lolos dari classifier tetap ditolak
    /// server. Engine lain mengabaikan flag ini.
    pub read_only: bool,
}

/// Dipanggil dari task eksekusi setelah sebuah hasil masuk ke channel, supaya
/// UI (egui) bangun dan memprosesnya tanpa polling. Pemanggil headless
/// (agent, tes) memakai `None`.
pub type ResultWakeHook = std::sync::Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub struct QueryJob {
    pub job_id: u64,
    /// `QueryTab::id` milik tab yang menjalankan job ini (None jika tidak ada tab aktif).
    pub tab_id: Option<usize>,
    pub options: QueryExecutionOptions,
    pub connection_pool: models::enums::DatabasePool,
    pub started_at: Instant,
    /// Lihat [`ResultWakeHook`].
    pub on_result: Option<ResultWakeHook>,
}

#[derive(Clone, Debug)]
pub struct QueryJobStatus {
    pub job_id: u64,
    pub connection_id: i64,
    pub query_preview: String,
    pub started_at: Instant,
    pub completed: bool,
}

/// Lokasi error SQL di dalam statement yang gagal, relatif terhadap teks
/// statement itu sendiri (bukan terhadap seluruh isi editor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorLocation {
    /// Teks statement yang dikirim ke server.
    pub statement: String,
    /// Offset karakter (0-based) di dalam statement, jika server memberikannya.
    pub char_offset: Option<usize>,
    /// Nomor baris (1-based) di dalam statement, jika hanya baris yang diketahui.
    pub line: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct QueryResultMessage {
    pub job_id: u64,
    /// `QueryTab::id` milik tab yang menjalankan job; hasil dikirim ke tab ini.
    pub tab_id: Option<usize>,
    pub connection_id: i64,
    pub success: bool,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub error: Option<String>,
    pub duration: std::time::Duration,
    pub query: String,
    pub dba_special_mode: Option<models::enums::DBASpecialMode>,
    pub ast_debug_sql: Option<String>,
    pub ast_headers: Option<Vec<String>>,
    pub affected_rows: Option<usize>, // Number of affected rows for INSERT/UPDATE/DELETE
    pub column_metadata: Option<Vec<models::structs::ColumnMetadata>>,
    /// True jika result set dipotong karena mencapai batas baris.
    pub truncated: bool,
    /// Posisi error di statement (untuk tombol "Go to error").
    pub error_location: Option<ErrorLocation>,
    /// Rincian waktu (tunggu/server/transfer/klien) bila driver mengukurnya.
    pub timing: Option<super::timing::QueryTiming>,
}

#[derive(Debug, Clone)]
pub struct QueryJobOutput {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub ast_debug_sql: Option<String>,
    pub ast_headers: Option<Vec<String>>,
    pub column_metadata: Option<Vec<models::structs::ColumnMetadata>>,
    /// Jumlah baris terdampak dari driver jika statement terakhir mengubah data.
    pub affected_rows: Option<u64>,
    pub truncated: bool,
}

#[derive(Debug)]
pub enum QueryPreparationError {
    ConnectionNotFound,
    PoolUnavailable,
    RuntimeUnavailable,
    UnsupportedDatabase,
}

#[derive(Debug, thiserror::Error)]
pub enum QueryExecutionError {
    #[error("{0}")]
    Message(String),
    /// Error yang posisinya di dalam statement diketahui.
    #[error("{0}")]
    Located(String, ErrorLocation),
    /// Koneksi ke server gagal atau putus (jaringan, TLS, pool habis/tertutup).
    /// Pool koneksi ini sebaiknya dibuang dan dibuat ulang.
    #[error("{0}")]
    Connection(String),
    /// Statement melewati batas waktu query.
    #[error("Query timed out")]
    Timeout,
    /// Job dibatalkan sebelum selesai.
    #[error("Query cancelled")]
    Cancelled,
}

impl QueryExecutionError {
    /// Petakan error sqlx: kegagalan kelas koneksi menjadi [`Self::Connection`],
    /// sisanya (error SQL dari server, decode, dll.) menjadi [`Self::Message`].
    pub fn from_sqlx(e: sqlx::Error) -> Self {
        Self::from_sqlx_with_context("", e)
    }

    /// Seperti [`Self::from_sqlx`], dengan awalan pesan (mis. `"PostgreSQL error: "`).
    pub fn from_sqlx_with_context(prefix: &str, e: sqlx::Error) -> Self {
        let message = format!("{prefix}{e}");
        if is_connection_class(&e) {
            Self::Connection(message)
        } else {
            Self::Message(message)
        }
    }

    /// True jika error ini menandakan pool/koneksi tidak bisa dipakai lagi.
    pub fn is_connection(&self) -> bool {
        matches!(self, Self::Connection(_))
    }
}

/// True untuk error sqlx yang berarti koneksinya sendiri bermasalah, bukan
/// statement-nya.
pub(crate) fn is_connection_class(e: &sqlx::Error) -> bool {
    matches!(
        e,
        sqlx::Error::Io(_)
            | sqlx::Error::Tls(_)
            | sqlx::Error::PoolTimedOut
            | sqlx::Error::PoolClosed
            | sqlx::Error::WorkerCrashed
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_class_sqlx_errors_map_to_connection() {
        let io = sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset by peer",
        ));
        let mapped = QueryExecutionError::from_sqlx_with_context("PostgreSQL error: ", io);
        assert!(mapped.is_connection());
        assert!(mapped.to_string().starts_with("PostgreSQL error: "));
        assert!(mapped.to_string().contains("reset by peer"));

        for e in [sqlx::Error::PoolTimedOut, sqlx::Error::PoolClosed] {
            assert!(QueryExecutionError::from_sqlx(e).is_connection());
        }
    }

    #[test]
    fn statement_errors_stay_plain_messages() {
        let mapped = QueryExecutionError::from_sqlx(sqlx::Error::RowNotFound);
        assert!(!mapped.is_connection());
        assert!(matches!(mapped, QueryExecutionError::Message(_)));
        assert_eq!(QueryExecutionError::Timeout.to_string(), "Query timed out");
    }
}
