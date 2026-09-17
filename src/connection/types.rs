use crate::models;
use std::time::Instant;

/// Id sesi di sisi server (backend pid PostgreSQL / connection id MySQL) untuk
/// setiap job query yang sedang berjalan, dengan key job id. Dipakai supaya
/// permintaan cancel benar-benar menghentikan statement di server, bukan hanya
/// meninggalkannya di sisi klien.
pub type BackendPidRegistry =
    std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, i64>>>;

/// Menghapus backend pid sebuah job dari registry saat job selesai atau
/// task-nya di-abort.
pub struct BackendPidGuard {
    registry: BackendPidRegistry,
    job_id: u64,
}

impl BackendPidGuard {
    pub fn register(registry: &BackendPidRegistry, job_id: u64, pid: i64) -> Self {
        if let Ok(mut map) = registry.lock() {
            map.insert(job_id, pid);
        }
        Self {
            registry: registry.clone(),
            job_id,
        }
    }
}

impl Drop for BackendPidGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = self.registry.lock() {
            map.remove(&self.job_id);
        }
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
    pub ast_enabled: bool,
    pub job_id: u64,
    /// Batalkan statement setelah durasi ini (None = tanpa batas).
    pub query_timeout: Option<std::time::Duration>,
    /// Berhenti membaca result set setelah jumlah baris ini.
    pub max_rows: usize,
    pub backend_pids: BackendPidRegistry,
}

#[derive(Clone)]
pub struct QueryJob {
    pub job_id: u64,
    /// `QueryTab::id` milik tab yang menjalankan job ini (None jika tidak ada tab aktif).
    pub tab_id: Option<usize>,
    pub options: QueryExecutionOptions,
    pub connection_pool: models::enums::DatabasePool,
    pub started_at: Instant,
}

#[derive(Clone, Debug)]
pub struct QueryJobStatus {
    pub job_id: u64,
    pub connection_id: i64,
    pub query_preview: String,
    pub started_at: Instant,
    pub completed: bool,
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

#[derive(Debug)]
pub enum QueryExecutionError {
    Message(String),
}
