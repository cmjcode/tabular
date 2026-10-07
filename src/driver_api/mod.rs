//! Lapisan driver engine database yang bisa dipasang sebagai plugin (ADR 0002).
//!
//! Engine builtin (PostgreSQL, MySQL, SQLite, SQL Server, MongoDB, Redis) tetap
//! memakai jalurnya sendiri. Engine baru masuk lewat trait [`EngineDriver`]
//! dan dijalankan oleh salah satu host: Wasm ([`wasm_host`]) untuk engine
//! berbasis HTTP, atau sidecar ([`sidecar`]) untuk protokol native.
//!
//! Modul ini headless: tidak boleh bergantung pada `window_egui`, dan tidak
//! pernah menerima `ConnectionConfig` di batas plugin. Plugin hanya melihat
//! [`ConnectParams`] milik koneksinya sendiri.

pub mod cache;
pub mod connect;
pub mod http;
pub mod manifest;
pub mod presets;
pub mod query;
pub mod registry;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub mod sidecar;
pub mod sqlite_adapter;
pub mod wasm_host;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Error domain driver plugin.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum DriverError {
    #[error("database driver '{0}' is not installed")]
    NotInstalled(String),
    #[error("connection failed: {0}")]
    Connect(String),
    #[error("{0}")]
    Query(String),
    #[error("operation not supported by this engine: {0}")]
    Unsupported(String),
    #[error("plugin error: {0}")]
    Plugin(String),
    #[error("plugin protocol error: {0}")]
    Protocol(String),
    #[error("permission denied: {0}")]
    Permission(String),
    #[error("query cancelled")]
    Cancelled,
}

pub type DriverResult<T> = Result<T, DriverError>;

/// Bahasa query yang diterima engine; menentukan mode editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum QueryLanguage {
    #[default]
    Sql,
    Json,
    Text,
}

/// Fitur yang didukung engine. UI menyembunyikan fitur yang tidak ada di sini,
/// sehingga fitur khusus engine builtin tidak bocor ke engine plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EngineCapabilities {
    /// Engine punya daftar database yang bisa dipilih.
    pub databases: bool,
    /// Engine punya schema di dalam database (seperti PostgreSQL).
    pub schemas: bool,
    pub query_language: QueryLanguage,
    /// Dialect untuk highlight/autocomplete, mis. `"clickhouse"`, `"ansi"`.
    pub sql_dialect: Option<String>,
    /// Engine bisa membatalkan query yang sedang berjalan.
    pub cancel: bool,
    /// Host boleh membuka SSH tunnel lalu memberi `localhost:port` ke plugin.
    pub ssh_tunnel: bool,
    /// Form koneksi menampilkan pengaturan TLS.
    pub tls: bool,
    /// Template query pratinjau tabel. Placeholder: `{table}` (sudah dikutip),
    /// `{raw_table}`, `{database}`, `{limit}`. Default: `SELECT * FROM {table}
    /// LIMIT {limit}` untuk engine SQL.
    pub preview_template: Option<String>,
}

impl Default for EngineCapabilities {
    fn default() -> Self {
        Self {
            databases: true,
            schemas: false,
            query_language: QueryLanguage::Sql,
            sql_dialect: None,
            cancel: false,
            ssh_tunnel: true,
            tls: true,
            preview_template: None,
        }
    }
}

/// Field standar `ConnectionConfig` yang dipakai engine di form koneksi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StandardFields {
    pub host: bool,
    pub port: bool,
    pub username: bool,
    pub password: bool,
    pub database: bool,
}

impl Default for StandardFields {
    fn default() -> Self {
        Self {
            host: true,
            port: true,
            username: true,
            password: true,
            database: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OptionKind {
    #[default]
    Text,
    /// Disimpan di secret store, bukan di `connections.db`.
    Secret,
    Number,
    Bool,
    Select,
}

/// Field tambahan khusus engine di form koneksi, disimpan di
/// `ConnectionConfig::plugin_options`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionField {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: OptionKind,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
    #[serde(default)]
    pub required: bool,
}

/// Deskripsi engine: dipakai registry, form koneksi, dan sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineDescriptor {
    /// Id stabil, huruf kecil, mis. `"clickhouse"`. Disimpan sebagai
    /// `plugin:<id>` di kolom `connections.connection_type`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub default_port: Option<u16>,
    #[serde(default)]
    pub standard_fields: StandardFields,
    #[serde(default)]
    pub options: Vec<OptionField>,
    #[serde(default)]
    pub capabilities: EngineCapabilities,
}

impl EngineDescriptor {
    /// Id valid: 1-48 karakter `[a-z0-9_-]`, diawali huruf.
    pub fn is_valid_id(id: &str) -> bool {
        let mut chars = id.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
            && id.len() <= 48
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    }
}

/// Pengaturan TLS yang diteruskan ke plugin.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsParams {
    pub enabled: bool,
    pub verify_server: bool,
    pub ca_cert: String,
    pub client_cert: String,
    pub client_key: String,
}

/// Parameter koneksi yang dikirim ke plugin. Hanya berisi data koneksi itu
/// sendiri; host/port sudah diganti ke ujung tunnel SSH bila tunnel aktif.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectParams {
    pub host: String,
    pub port: Option<u16>,
    pub username: String,
    pub password: String,
    pub database: String,
    pub tls: TlsParams,
    pub options: BTreeMap<String, String>,
}

impl std::fmt::Debug for ConnectParams {
    // Password dan opsi disembunyikan supaya tidak bocor ke log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectParams")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("database", &self.database)
            .field("tls_enabled", &self.tls.enabled)
            .field("option_keys", &self.options.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    #[default]
    Table,
    View,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    #[serde(default)]
    pub kind: TableKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    #[serde(default)]
    pub data_type: String,
    #[serde(default = "default_true")]
    pub nullable: bool,
    #[serde(default)]
    pub primary_key: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub query: String,
    #[serde(default)]
    pub database: Option<String>,
    #[serde(default)]
    pub schema: Option<String>,
    /// Plugin berhenti membaca setelah jumlah baris ini dan menandai `truncated`.
    pub max_rows: usize,
    pub job_id: u64,
}

/// Hasil satu statement. `None` di sel berarti NULL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecuteOutput {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub affected_rows: Option<u64>,
    pub truncated: bool,
}

impl ExecuteOutput {
    /// Ubah ke bentuk grid Tabular (`"NULL"` untuk nilai kosong, sama dengan
    /// konverter driver builtin) dan terapkan batas baris host.
    pub fn into_table_rows(self, max_rows: usize) -> (Vec<String>, Vec<Vec<String>>, bool) {
        let mut truncated = self.truncated;
        let mut rows: Vec<Vec<String>> = self
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|cell| cell.unwrap_or_else(|| "NULL".to_string()))
                    .collect()
            })
            .collect();
        if max_rows > 0 && rows.len() > max_rows {
            rows.truncate(max_rows);
            truncated = true;
        }
        (self.headers, rows, truncated)
    }
}

/// Driver untuk satu engine. Semua method bersifat blocking: host memanggilnya
/// dari `tokio::task::spawn_blocking` (lihat [`run_blocking`]).
pub trait EngineDriver: Send + Sync {
    fn descriptor(&self) -> &EngineDescriptor;
    fn connect(&self, params: ConnectParams) -> DriverResult<Arc<dyn EngineSession>>;
}

/// Satu koneksi terbuka ke engine. Harus aman dipakai paralel dari beberapa
/// thread; `cancel` dipanggil ketika `execute` lain sedang berjalan.
pub trait EngineSession: Send + Sync {
    fn list_databases(&self) -> DriverResult<Vec<String>>;
    fn list_schemas(&self, database: Option<&str>) -> DriverResult<Vec<String>>;
    fn list_tables(
        &self,
        database: Option<&str>,
        schema: Option<&str>,
    ) -> DriverResult<Vec<TableInfo>>;
    fn list_columns(
        &self,
        database: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> DriverResult<Vec<ColumnInfo>>;
    fn execute(&self, request: &ExecuteRequest) -> DriverResult<ExecuteOutput>;
    fn cancel(&self, _job_id: u64) -> DriverResult<()> {
        Err(DriverError::Unsupported("cancel".to_string()))
    }
    fn close(&self) {}
}

/// Pool untuk `DatabasePool::Plugin`: satu sesi engine plugin yang sudah
/// terhubung, dipakai bersama oleh semua tab koneksi itu.
pub struct PluginPool {
    pub engine_id: String,
    pub capabilities: EngineCapabilities,
    pub session: Arc<dyn EngineSession>,
}

impl Drop for PluginPool {
    fn drop(&mut self) {
        self.session.close();
    }
}

impl std::fmt::Debug for PluginPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginPool")
            .field("engine_id", &self.engine_id)
            .finish_non_exhaustive()
    }
}

/// Jalankan operasi driver yang blocking di thread pool tokio.
pub async fn run_blocking<T, F>(f: F) -> DriverResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> DriverResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| DriverError::Plugin(format!("driver task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_id_validation() {
        assert!(EngineDescriptor::is_valid_id("clickhouse"));
        assert!(EngineDescriptor::is_valid_id("my-engine_2"));
        assert!(!EngineDescriptor::is_valid_id(""));
        assert!(!EngineDescriptor::is_valid_id("2fast"));
        assert!(!EngineDescriptor::is_valid_id("ClickHouse"));
        assert!(!EngineDescriptor::is_valid_id("../evil"));
        assert!(!EngineDescriptor::is_valid_id(&"a".repeat(49)));
    }

    #[test]
    fn output_maps_null_and_applies_row_cap() {
        let out = ExecuteOutput {
            headers: vec!["a".into()],
            rows: vec![vec![Some("1".into())], vec![None], vec![Some("3".into())]],
            affected_rows: None,
            truncated: false,
        };
        let (headers, rows, truncated) = out.into_table_rows(2);
        assert_eq!(headers, vec!["a"]);
        assert_eq!(rows, vec![vec!["1".to_string()], vec!["NULL".to_string()]]);
        assert!(truncated);
    }

    #[test]
    fn connect_params_debug_hides_secrets() {
        let mut options = BTreeMap::new();
        options.insert("api_key".to_string(), "sk-live-123".to_string());
        let p = ConnectParams {
            password: "hunter2".into(),
            options,
            ..Default::default()
        };
        let dbg = format!("{p:?}");
        assert!(!dbg.contains("hunter2"));
        assert!(!dbg.contains("sk-live-123"));
        assert!(dbg.contains("api_key"));
    }

    #[test]
    fn descriptor_parses_with_defaults() {
        let d: EngineDescriptor =
            serde_json::from_str(r#"{"id":"clickhouse","name":"ClickHouse"}"#).unwrap();
        assert!(d.standard_fields.host);
        assert_eq!(d.capabilities.query_language, QueryLanguage::Sql);
        assert!(d.options.is_empty());
    }
}
