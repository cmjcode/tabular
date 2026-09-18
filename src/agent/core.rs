//! Lapisan headless untuk agent AI.
//!
//! Semua fungsi di sini **tidak** menyentuh `window_egui::Tabular`; mereka
//! bekerja langsung dengan `connections.db` (cache lokal Tabular) dan pool
//! driver. Dengan begitu server MCP (`tabular mcp`) bisa berjalan sebagai proses
//! terpisah tanpa GUI, dan di masa depan lapisan yang sama bisa dipakai GUI
//! untuk menampilkan permintaan agent.
//!
//! Prinsip keamanan:
//! - Agent hanya menerima ringkasan koneksi (id, nama, tipe, host). Password,
//!   kunci SSH, dan sertifikat tidak pernah diserialisasi keluar.
//! - Query yang bukan read-only ditolak sebelum menyentuh driver
//!   (lihat [`super::classify`]). Tidak ada opsi untuk membukanya dari sisi
//!   agent; menulis harus lewat GUI.
//! - Hasil dipotong sesuai [`AgentLimits`] supaya tidak meledakkan konteks
//!   model.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::connection::types::{QueryExecutionOptions, QueryJob};
use crate::models::enums::{DatabasePool, DatabaseType};
use crate::models::structs::ConnectionConfig;

use super::classify::{self, StatementKind};

/// Kesalahan lapisan agent. Pesannya ditujukan untuk dibaca model, jadi harus
/// menjelaskan apa yang bisa dilakukan selanjutnya.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("connection {0} not found; call list_connections for valid ids")]
    ConnectionNotFound(i64),
    #[error("connection {0} ({1}) does not support this operation")]
    Unsupported(i64, String),
    #[error("refused: {0}")]
    Refused(String),
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("query failed: {0}")]
    Query(String),
    #[error("local Tabular cache error: {0}")]
    Cache(#[from] sqlx::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Batas ukuran hasil yang dikirim ke agent.
#[derive(Debug, Clone)]
pub struct AgentLimits {
    /// Jumlah baris maksimum per `run_query` (bisa diturunkan per panggilan).
    pub max_rows: usize,
    /// Panjang maksimum satu sel; sisanya dipotong dengan penanda.
    pub max_cell_chars: usize,
    /// Perkiraan total byte hasil sebelum baris berikutnya dibuang.
    pub max_result_bytes: usize,
    /// Batas waktu satu statement di server.
    pub query_timeout: Duration,
    /// Jumlah tabel maksimum di `describe_schema`.
    pub max_schema_tables: usize,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_rows: 200,
            max_cell_chars: 500,
            max_result_bytes: 256 * 1024,
            query_timeout: Duration::from_secs(30),
            max_schema_tables: 40,
        }
    }
}

/// Ringkasan koneksi yang aman dibagikan ke agent (tanpa rahasia).
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionSummary {
    pub id: i64,
    pub name: String,
    /// Jenis database: MySQL, PostgreSQL, SQLite, MsSQL, Redis, MongoDB, ApiHttp.
    pub kind: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub folder: Option<String>,
    /// `false` untuk koneksi yang tidak bisa menjalankan query lewat agent.
    pub supports_query: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColumnDescription {
    pub name: String,
    pub data_type: String,
    pub primary_key: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForeignKeyDescription {
    pub column: String,
    pub references_table: String,
    pub references_column: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TableDescription {
    pub name: String,
    /// "table" atau "view".
    pub kind: String,
    pub columns: Vec<ColumnDescription>,
    pub foreign_keys: Vec<ForeignKeyDescription>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SchemaDescription {
    pub connection_id: i64,
    pub database: String,
    pub total_tables: usize,
    pub shown_tables: usize,
    /// `true` bila urutan tabel dipilih berdasarkan relevansi dengan pertanyaan.
    pub ranked_by_relevance: bool,
    pub tables: Vec<TableDescription>,
    /// Ringkasan DDL kompak (format yang sama dengan AI assistant di GUI).
    pub ddl: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentQueryResult {
    pub connection_id: i64,
    pub database: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
    /// `true` bila baris atau sel dipotong karena batas ukuran.
    pub truncated: bool,
    pub execution_ms: u128,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatementSafety {
    pub statement: String,
    pub kind: StatementKind,
    pub read_only: bool,
    /// Terisi bila UPDATE/DELETE tanpa WHERE terdeteksi.
    pub unsafe_dml: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintEntry {
    pub severity: String,
    pub message: String,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SafetyReport {
    pub read_only: bool,
    /// `true` bila agent boleh menjalankannya lewat `run_query`.
    pub allowed_for_agent: bool,
    pub statements: Vec<StatementSafety>,
    pub lints: Vec<LintEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplainResult {
    pub connection_id: i64,
    pub executed_sql: String,
    pub raw_plan: String,
    pub summary: Option<crate::query_profiler::ExplainSummary>,
    pub warnings: Vec<String>,
    pub plan: Option<crate::query_profiler::ExplainNode>,
}

/// Sesi headless: satu cache pool SQLite + pool driver per koneksi.
pub struct HeadlessSession {
    cache_pool: SqlitePool,
    pools: Mutex<HashMap<i64, DatabasePool>>,
    next_job_id: AtomicU64,
    pub limits: AgentLimits,
}

/// Buka `connections.db` milik Tabular dalam mode baca-tulis tanpa membuat
/// file baru: skema dibuat oleh GUI, jadi bila file belum ada berarti Tabular
/// belum pernah dijalankan di mesin ini.
pub async fn open_cache_pool() -> Result<SqlitePool, AgentError> {
    crate::directory::ensure_app_directories()?;
    let db_path = crate::directory::get_data_dir().join("connections.db");
    if !db_path.exists() {
        return Err(AgentError::Refused(format!(
            "no Tabular data at {}; open the Tabular app once and add a connection first",
            db_path.display()
        )));
    }
    let url = format!("sqlite://{}?mode=rw", db_path.to_string_lossy());
    let opts = sqlx::sqlite::SqliteConnectOptions::from_str(&url)?
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5));
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(opts)
        .await?;
    Ok(pool)
}

fn kind_label(t: &DatabaseType) -> &'static str {
    match t {
        DatabaseType::MySQL => "MySQL",
        DatabaseType::PostgreSQL => "PostgreSQL",
        DatabaseType::SQLite => "SQLite",
        DatabaseType::Redis => "Redis",
        DatabaseType::MsSQL => "MsSQL",
        DatabaseType::MongoDB => "MongoDB",
        DatabaseType::ApiHttp => "ApiHttp",
    }
}

fn supports_query(kind: &str) -> bool {
    matches!(kind, "MySQL" | "PostgreSQL" | "SQLite" | "MsSQL" | "Redis")
}

impl HeadlessSession {
    pub fn new(cache_pool: SqlitePool) -> Self {
        Self {
            cache_pool,
            pools: Mutex::new(HashMap::new()),
            next_job_id: AtomicU64::new(1),
            limits: AgentLimits::default(),
        }
    }

    pub fn cache_pool(&self) -> &SqlitePool {
        &self.cache_pool
    }

    /// Daftar koneksi tersimpan, tanpa kolom rahasia.
    pub async fn list_connections(&self) -> Result<Vec<ConnectionSummary>, AgentError> {
        let rows: Vec<(i64, String, String, String, String, String, String)> = sqlx::query_as(
            "SELECT id, name, COALESCE(host, ''), COALESCE(port, ''), COALESCE(database_name, ''), \
                    COALESCE(connection_type, ''), COALESCE(folder, '') \
             FROM connections ORDER BY name",
        )
        .fetch_all(&self.cache_pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(
                |(id, name, host, port, database, kind, folder)| ConnectionSummary {
                    id,
                    name,
                    supports_query: supports_query(&kind),
                    kind,
                    host,
                    port,
                    database,
                    folder: if folder.is_empty() {
                        None
                    } else {
                        Some(folder)
                    },
                },
            )
            .collect())
    }

    async fn load_connection(&self, id: i64) -> Result<ConnectionConfig, AgentError> {
        crate::connection::pool::load_connection_by_id(id, &self.cache_pool)
            .await
            .ok_or(AgentError::ConnectionNotFound(id))
    }

    /// Ambil (atau buat) pool driver untuk koneksi. Pool dipakai ulang selama
    /// proses hidup; SSH tunnel dan TLS ditangani oleh pembuat pool GUI.
    async fn pool_for(&self, id: i64) -> Result<(ConnectionConfig, DatabasePool), AgentError> {
        let conn = self.load_connection(id).await?;
        if matches!(conn.connection_type, DatabaseType::ApiHttp) {
            return Err(AgentError::Unsupported(
                id,
                kind_label(&conn.connection_type).to_string(),
            ));
        }
        let mut pools = self.pools.lock().await;
        if let Some(p) = pools.get(&id) {
            return Ok((conn, p.clone()));
        }
        let pool = crate::connection::pool::create_connection_pool_for_config(&conn)
            .await
            .map_err(AgentError::Connect)?;
        pools.insert(id, pool.clone());
        log::info!("[AGENT] opened pool for connection {id} ({})", conn.name);
        Ok((conn, pool))
    }

    /// Muat ulang cache skema (database, tabel, kolom, FK) dari server.
    pub async fn refresh_schema_cache(&self, id: i64) -> Result<usize, AgentError> {
        let (conn, pool) = self.pool_for(id).await?;
        let ok = crate::connection::metadata::cache::fetch_and_cache_all_data(
            id,
            &conn,
            &pool,
            &self.cache_pool,
        )
        .await;
        if !ok {
            return Err(AgentError::Query(
                "schema fetch failed; see Tabular log for details".to_string(),
            ));
        }
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM table_cache WHERE connection_id = ?")
                .bind(id)
                .fetch_one(&self.cache_pool)
                .await?;
        Ok(count.max(0) as usize)
    }

    async fn cached_databases(&self, id: i64) -> Result<Vec<String>, AgentError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT database_name FROM database_cache WHERE connection_id = ? ORDER BY database_name",
        )
        .bind(id)
        .fetch_all(&self.cache_pool)
        .await?;
        Ok(rows.into_iter().map(|(d,)| d).collect())
    }

    /// Daftar database/schema yang diketahui untuk koneksi. Bila cache kosong,
    /// diambil dari server dulu.
    pub async fn list_databases(&self, id: i64) -> Result<Vec<String>, AgentError> {
        let mut dbs = self.cached_databases(id).await?;
        if dbs.is_empty() {
            self.refresh_schema_cache(id).await?;
            dbs = self.cached_databases(id).await?;
        }
        Ok(dbs)
    }

    async fn resolve_database(
        &self,
        conn: &ConnectionConfig,
        requested: Option<&str>,
    ) -> Result<String, AgentError> {
        if let Some(db) = requested.map(str::trim).filter(|s| !s.is_empty()) {
            return Ok(db.to_string());
        }
        if !conn.database.trim().is_empty() {
            return Ok(conn.database.trim().to_string());
        }
        let id = conn.id.unwrap_or_default();
        self.list_databases(id)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                AgentError::Refused(
                    "no database known for this connection; pass `database` explicitly".to_string(),
                )
            })
    }

    async fn cached_tables(&self, id: i64, db: &str) -> Result<Vec<(String, String)>, AgentError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT table_name, table_type FROM table_cache \
             WHERE connection_id = ? AND database_name = ? COLLATE NOCASE \
               AND table_type IN ('table', 'view') \
             ORDER BY table_type, table_name",
        )
        .bind(id)
        .bind(db)
        .fetch_all(&self.cache_pool)
        .await?;
        Ok(rows)
    }

    /// Deskripsi skema untuk agent. Jika `question` diberikan dan tabel lebih
    /// banyak dari batas, tabel diurutkan berdasarkan relevansi memakai indeks
    /// vektor lokal (tanpa memanggil API eksternal).
    pub async fn describe_schema(
        &self,
        id: i64,
        database: Option<&str>,
        question: Option<&str>,
        max_tables: Option<usize>,
    ) -> Result<SchemaDescription, AgentError> {
        let conn = self.load_connection(id).await?;
        let db = self.resolve_database(&conn, database).await?;
        let max_tables = max_tables
            .unwrap_or(self.limits.max_schema_tables)
            .clamp(1, 500);

        let mut tables = self.cached_tables(id, &db).await?;
        let mut note = None;
        if tables.is_empty() {
            match self.refresh_schema_cache(id).await {
                Ok(_) => tables = self.cached_tables(id, &db).await?,
                Err(e) => note = Some(format!("schema cache empty and refresh failed: {e}")),
            }
        }

        let total = tables.len();
        let question = question.map(str::trim).filter(|q| !q.is_empty());
        let mut ranked = false;
        if let Some(q) = question
            && total > max_tables
        {
            match self.rank_tables(id, &db, q, total).await {
                Ok(order) if !order.is_empty() => {
                    let kinds: HashMap<String, String> = tables.iter().cloned().collect();
                    let mut picked: Vec<(String, String)> = Vec::new();
                    for name in order {
                        if let Some(kind) = kinds.get(&name)
                            && !picked.iter().any(|(n, _)| n == &name)
                        {
                            picked.push((name.clone(), kind.clone()));
                        }
                    }
                    for (name, kind) in tables {
                        if !picked.iter().any(|(n, _)| n == &name) {
                            picked.push((name, kind));
                        }
                    }
                    tables = picked;
                    ranked = true;
                }
                Ok(_) => {}
                Err(e) => log::warn!("[AGENT] relevance ranking failed: {e}"),
            }
        }

        let mut described = Vec::new();
        let mut ddl = format!("-- Database: {db}\n");
        for (name, kind) in tables.iter().take(max_tables) {
            let cols: Vec<(String, String, i64)> = sqlx::query_as(
                "SELECT column_name, data_type, COALESCE(is_primary_key, 0) FROM column_cache \
                 WHERE connection_id = ? AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE \
                 ORDER BY ordinal_position",
            )
            .bind(id)
            .bind(&db)
            .bind(name)
            .fetch_all(&self.cache_pool)
            .await?;
            let fks: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT column_name, referenced_table_name, referenced_column_name FROM foreign_key_cache \
                 WHERE connection_id = ? AND database_name = ? COLLATE NOCASE AND table_name = ? COLLATE NOCASE",
            )
            .bind(id)
            .bind(&db)
            .bind(name)
            .fetch_all(&self.cache_pool)
            .await
            .unwrap_or_default();

            ddl.push_str(&format!("-- {kind}: {name}\n"));
            if cols.is_empty() {
                ddl.push_str(&format!(
                    "-- {name}: columns not cached yet; call refresh_schema_cache\n\n"
                ));
            } else {
                let col_lines: Vec<String> = cols
                    .iter()
                    .map(|(c, t, pk)| {
                        if *pk != 0 {
                            format!("  {c} {t} PRIMARY KEY")
                        } else {
                            format!("  {c} {t}")
                        }
                    })
                    .collect();
                ddl.push_str(&format!(
                    "CREATE TABLE {name} (\n{}\n);\n",
                    col_lines.join(",\n")
                ));
                for (c, rt, rc) in &fks {
                    ddl.push_str(&format!("-- FK {name}.{c} -> {rt}.{rc}\n"));
                }
                ddl.push('\n');
            }

            described.push(TableDescription {
                name: name.clone(),
                kind: kind.clone(),
                columns: cols
                    .into_iter()
                    .map(|(c, t, pk)| ColumnDescription {
                        name: c,
                        data_type: t,
                        primary_key: pk != 0,
                    })
                    .collect(),
                foreign_keys: fks
                    .into_iter()
                    .map(|(c, rt, rc)| ForeignKeyDescription {
                        column: c,
                        references_table: rt,
                        references_column: rc,
                    })
                    .collect(),
            });
        }

        if total > max_tables {
            ddl.push_str(&format!(
                "-- ... and {} more tables (showing {})\n",
                total - max_tables,
                if ranked {
                    format!("the {max_tables} most relevant to the question")
                } else {
                    format!("first {max_tables}; pass `question` to rank by relevance")
                }
            ));
        }

        Ok(SchemaDescription {
            connection_id: id,
            database: db,
            total_tables: total,
            shown_tables: described.len(),
            ranked_by_relevance: ranked,
            tables: described,
            ddl,
            note,
        })
    }

    async fn rank_tables(
        &self,
        id: i64,
        db: &str,
        question: &str,
        limit: usize,
    ) -> Result<Vec<String>, sqlx::Error> {
        crate::vector_index::sync_schema_embeddings(&self.cache_pool, id, db).await?;
        let ranked =
            crate::vector_index::rank_tables(&self.cache_pool, id, db, question, limit).await?;
        Ok(ranked.into_iter().map(|(t, _)| t).collect())
    }

    /// Laporan keamanan tanpa menjalankan apa pun.
    pub async fn check_sql_safety(
        &self,
        id: Option<i64>,
        sql: &str,
    ) -> Result<SafetyReport, AgentError> {
        let db_type = match id {
            Some(id) => self.load_connection(id).await?.connection_type,
            None => DatabaseType::PostgreSQL,
        };
        Ok(build_safety_report(&db_type, sql))
    }

    /// Jalankan query read-only dan kembalikan hasil yang sudah dipotong.
    pub async fn run_query(
        &self,
        id: i64,
        sql: &str,
        database: Option<&str>,
        max_rows: Option<usize>,
    ) -> Result<AgentQueryResult, AgentError> {
        let conn = self.load_connection(id).await?;
        let report = build_safety_report(&conn.connection_type, sql);
        if !report.allowed_for_agent {
            let kinds: Vec<String> = report
                .statements
                .iter()
                .filter(|s| !s.read_only)
                .map(|s| format!("{:?}", s.kind).to_lowercase())
                .collect();
            return Err(AgentError::Refused(format!(
                "agent access is read-only; statement kind(s) {} are not allowed. \
                 Ask the user to run this in the Tabular app.",
                kinds.join(", ")
            )));
        }
        let (conn, pool) = self.pool_for(id).await?;
        let database = match conn.connection_type {
            DatabaseType::SQLite => None,
            _ => database
                .map(str::trim)
                .filter(|d| !d.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    let d = conn.database.trim();
                    (!d.is_empty()).then(|| d.to_string())
                }),
        };

        let max_rows = max_rows
            .unwrap_or(self.limits.max_rows)
            .clamp(1, self.limits.max_rows.max(1));
        let job_id = self.next_job_id.fetch_add(1, Ordering::Relaxed);
        let options = QueryExecutionOptions {
            connection_id: id,
            connection: conn.clone(),
            query: sql.to_string(),
            selected_database: database.clone(),
            schema_name: None,
            use_server_pagination: false,
            current_page: 0,
            page_size: max_rows,
            base_query: None,
            dba_special_mode: None,
            save_to_history: false,
            ast_enabled: false,
            job_id,
            query_timeout: Some(self.limits.query_timeout),
            max_rows: max_rows + 1,
            backend_pids: Default::default(),
        };
        let job = QueryJob {
            job_id,
            tab_id: None,
            options,
            connection_pool: pool,
            started_at: Instant::now(),
        };

        let msg = crate::connection::execute::execute_query_job(job).await;
        self.record_history(&conn, sql).await;

        if !msg.success {
            return Err(AgentError::Query(
                msg.error.unwrap_or_else(|| "unknown error".to_string()),
            ));
        }

        let mut result = AgentQueryResult {
            connection_id: id,
            database,
            columns: msg.headers,
            rows: msg.rows,
            row_count: 0,
            truncated: msg.truncated,
            execution_ms: msg.duration.as_millis(),
            warnings: Vec::new(),
        };
        if let Some(n) = msg.affected_rows {
            result
                .warnings
                .push(format!("driver reported {n} affected rows"));
        }
        truncate_result(&mut result, max_rows, &self.limits);
        Ok(result)
    }

    /// Jalankan EXPLAIN untuk statement read-only dan parse hasilnya dengan
    /// profiler yang sama seperti GUI.
    pub async fn explain_query(
        &self,
        id: i64,
        sql: &str,
        database: Option<&str>,
        analyze: bool,
    ) -> Result<ExplainResult, AgentError> {
        let conn = self.load_connection(id).await?;
        let stmt = crate::query_tools::statement_parser::split_statements(sql)
            .into_iter()
            .map(|s| s.text.trim().to_string())
            .find(|s| !s.is_empty())
            .ok_or_else(|| AgentError::Refused("empty statement".to_string()))?;
        if !classify::classify_sql_statement(&stmt).is_read_only() {
            return Err(AgentError::Refused(
                "explain_query only accepts read-only statements (EXPLAIN ANALYZE would execute writes)"
                    .to_string(),
            ));
        }
        let stripped = strip_leading_explain(&stmt);
        let prefix = match conn.connection_type {
            DatabaseType::PostgreSQL if analyze => "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) ",
            DatabaseType::PostgreSQL => "EXPLAIN (FORMAT JSON) ",
            DatabaseType::MySQL if analyze => "EXPLAIN ANALYZE ",
            DatabaseType::MySQL => "EXPLAIN FORMAT=JSON ",
            DatabaseType::SQLite => "EXPLAIN QUERY PLAN ",
            other => {
                return Err(AgentError::Unsupported(id, kind_label(&other).to_string()));
            }
        };
        let explain_sql = format!("{prefix}{stripped}");
        let result = self
            .run_query(id, &explain_sql, database, Some(self.limits.max_rows))
            .await?;

        let raw = if result.columns.len() == 1 {
            result
                .rows
                .iter()
                .map(|r| r.first().cloned().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            result
                .rows
                .iter()
                .map(|r| r.join(" | "))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let parsed = crate::query_profiler::parse_explain(&raw);
        let mut warnings = Vec::new();
        if let Some((root, _)) = &parsed {
            collect_warnings(root, &mut warnings);
        }
        Ok(ExplainResult {
            connection_id: id,
            executed_sql: explain_sql,
            raw_plan: raw,
            summary: parsed.as_ref().map(|(_, s)| s.clone()),
            warnings,
            plan: parsed.map(|(n, _)| n),
        })
    }

    /// Catat query agent ke history Tabular supaya user bisa mengaudit apa
    /// yang dijalankan agent. Kegagalan tidak menggagalkan query.
    async fn record_history(&self, conn: &ConnectionConfig, sql: &str) {
        let Some(id) = conn.id else { return };
        let trimmed = sql.trim();
        if trimmed.is_empty() {
            return;
        }
        let res = sqlx::query(
            "INSERT INTO query_history (query_text, connection_id, connection_name) VALUES (?, ?, ?)",
        )
        .bind(trimmed)
        .bind(id)
        .bind(format!("{} (agent)", conn.name))
        .execute(&self.cache_pool)
        .await;
        if let Err(e) = res {
            log::warn!("[AGENT] failed to record history: {e}");
        }
    }
}

/// Buang awalan `EXPLAIN ...` yang mungkin sudah ditulis agent supaya prefix
/// sesuai dialek bisa dipasang ulang.
fn strip_leading_explain(stmt: &str) -> String {
    let upper = stmt.to_ascii_uppercase();
    if !upper.starts_with("EXPLAIN") {
        return stmt.to_string();
    }
    // Cari statement starter pertama setelah EXPLAIN dan opsinya.
    for starter in ["SELECT", "WITH", "VALUES", "TABLE", "SHOW"] {
        if let Some(pos) = upper.find(starter)
            && pos > 0
        {
            return stmt[pos..].to_string();
        }
    }
    stmt.to_string()
}

fn collect_warnings(node: &crate::query_profiler::ExplainNode, out: &mut Vec<String>) {
    for w in &node.warnings {
        let mut line = format!("{}: {} — {}", node.node_type, w.title, w.description);
        if let Some(rec) = &w.recommendation {
            line.push_str(&format!(" (recommendation: {rec})"));
        }
        out.push(line);
    }
    for child in &node.children {
        collect_warnings(child, out);
    }
}

pub fn build_safety_report(db_type: &DatabaseType, sql: &str) -> SafetyReport {
    let parts = classify::classify_query(db_type, sql);
    let statements: Vec<StatementSafety> = parts
        .into_iter()
        .map(|(statement, kind)| {
            let unsafe_dml = crate::safety_guard::analyze_safety(&statement).map(|r| {
                format!(
                    "{} without WHERE on {}",
                    r.statement_type,
                    r.table_name.unwrap_or_else(|| "unknown table".to_string())
                )
            });
            StatementSafety {
                read_only: kind.is_read_only(),
                statement,
                kind,
                unsafe_dml,
            }
        })
        .collect();
    let read_only = !statements.is_empty() && statements.iter().all(|s| s.read_only);
    let lints = match db_type {
        DatabaseType::Redis | DatabaseType::MongoDB | DatabaseType::ApiHttp => Vec::new(),
        _ => crate::query_tools::lint_sql(sql)
            .into_iter()
            .map(|l| LintEntry {
                severity: format!("{:?}", l.severity).to_lowercase(),
                message: l.message,
                hint: l.hint,
            })
            .collect(),
    };
    SafetyReport {
        read_only,
        allowed_for_agent: read_only,
        statements,
        lints,
    }
}

/// Potong hasil sesuai batas baris, panjang sel, dan total byte.
pub fn truncate_result(result: &mut AgentQueryResult, max_rows: usize, limits: &AgentLimits) {
    if result.rows.len() > max_rows {
        result.rows.truncate(max_rows);
        result.truncated = true;
    }
    let mut bytes: usize = result.columns.iter().map(String::len).sum();
    let mut keep = 0;
    for row in result.rows.iter_mut() {
        for cell in row.iter_mut() {
            if cell.chars().count() > limits.max_cell_chars {
                let cut: String = cell.chars().take(limits.max_cell_chars).collect();
                *cell = format!("{cut}…[truncated]");
                result.truncated = true;
            }
            bytes += cell.len() + 4;
        }
        if bytes > limits.max_result_bytes && keep > 0 {
            break;
        }
        keep += 1;
    }
    if keep < result.rows.len() {
        result.rows.truncate(keep);
        result.truncated = true;
    }
    result.row_count = result.rows.len();
    if result.truncated {
        result.warnings.push(format!(
            "result truncated to {} rows / {} chars per cell; add LIMIT or narrow the query",
            result.rows.len(),
            limits.max_cell_chars
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rows: usize, cell_len: usize) -> AgentQueryResult {
        AgentQueryResult {
            connection_id: 1,
            database: None,
            columns: vec!["a".into(), "b".into()],
            rows: (0..rows)
                .map(|i| vec![i.to_string(), "x".repeat(cell_len)])
                .collect(),
            row_count: 0,
            truncated: false,
            execution_ms: 0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn truncate_by_rows() {
        let mut r = sample(10, 3);
        truncate_result(&mut r, 4, &AgentLimits::default());
        assert_eq!(r.rows.len(), 4);
        assert_eq!(r.row_count, 4);
        assert!(r.truncated);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn truncate_by_cell_length() {
        let mut r = sample(2, 50);
        let limits = AgentLimits {
            max_cell_chars: 10,
            ..Default::default()
        };
        truncate_result(&mut r, 10, &limits);
        assert_eq!(r.rows.len(), 2);
        assert!(r.rows[0][1].starts_with("xxxxxxxxxx…"));
        assert!(r.truncated);
    }

    #[test]
    fn truncate_by_bytes_keeps_at_least_one_row() {
        let mut r = sample(100, 100);
        let limits = AgentLimits {
            max_result_bytes: 50,
            ..Default::default()
        };
        truncate_result(&mut r, 100, &limits);
        assert_eq!(r.rows.len(), 1);
        assert!(r.truncated);
    }

    #[test]
    fn untouched_result_is_not_marked_truncated() {
        let mut r = sample(3, 3);
        truncate_result(&mut r, 10, &AgentLimits::default());
        assert_eq!(r.rows.len(), 3);
        assert!(!r.truncated);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn safety_report_flags_writes_and_unsafe_dml() {
        let pg = DatabaseType::PostgreSQL;
        let ok = build_safety_report(&pg, "SELECT 1");
        assert!(ok.allowed_for_agent);
        let bad = build_safety_report(&pg, "SELECT 1; DELETE FROM t");
        assert!(!bad.allowed_for_agent);
        assert_eq!(bad.statements.len(), 2);
        assert!(bad.statements[1].unsafe_dml.is_some());
        let redis = build_safety_report(&DatabaseType::Redis, "GET a");
        assert!(redis.allowed_for_agent);
        assert!(redis.lints.is_empty());
    }

    #[test]
    fn strip_explain_prefix() {
        assert_eq!(strip_leading_explain("SELECT 1"), "SELECT 1");
        assert_eq!(
            strip_leading_explain("EXPLAIN (ANALYZE) SELECT 1"),
            "SELECT 1"
        );
        assert_eq!(strip_leading_explain("explain select 2"), "select 2");
    }

    #[tokio::test]
    async fn run_query_refuses_writes_and_runs_reads_on_sqlite() {
        // Cache in-memory dengan skema minimal yang dibaca lapisan agent.
        let cache = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("cache");
        let db_file =
            std::env::temp_dir().join(format!("tabular-agent-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db_file);
        sqlx::query(
            "CREATE TABLE connections (id INTEGER PRIMARY KEY, name TEXT, host TEXT, port TEXT, \
             username TEXT, password TEXT, database_name TEXT, connection_type TEXT, folder TEXT, \
             ssh_enabled INTEGER, ssh_host TEXT, ssh_port TEXT, ssh_username TEXT, ssh_auth_method TEXT, \
             ssh_private_key TEXT, ssh_password TEXT, ssh_accept_unknown_host_keys INTEGER, ssh_jump_host TEXT, \
             ssl_enabled INTEGER, ssl_ca_cert TEXT, ssl_client_cert TEXT, ssl_client_key TEXT, \
             ssl_key_passphrase TEXT, ssl_verify_server INTEGER); \
             CREATE TABLE query_history (id INTEGER PRIMARY KEY AUTOINCREMENT, query_text TEXT NOT NULL, \
             connection_id INTEGER NOT NULL, connection_name TEXT NOT NULL, executed_at DATETIME DEFAULT CURRENT_TIMESTAMP);",
        )
        .execute(&cache)
        .await
        .expect("schema");
        sqlx::query(
            "INSERT INTO connections (id, name, host, port, username, password, database_name, connection_type, folder) \
             VALUES (7, 'local', ?, '', '', '', '', 'SQLite', NULL)",
        )
        .bind(db_file.to_string_lossy().to_string())
        .execute(&cache)
        .await
        .expect("insert");

        // Data target: file SQLite sungguhan.
        {
            let target = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&format!("sqlite://{}?mode=rwc", db_file.to_string_lossy()))
                .await
                .expect("target");
            sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO t (name) VALUES ('a'), ('b'), ('c')")
                .execute(&target)
                .await
                .expect("seed");
        }

        let session = HeadlessSession::new(cache.clone());
        let conns = session.list_connections().await.expect("list");
        assert_eq!(conns.len(), 1);
        assert_eq!(conns[0].id, 7);
        assert_eq!(conns[0].kind, "SQLite");

        let refused = session.run_query(7, "DELETE FROM t", None, None).await;
        assert!(
            matches!(refused, Err(AgentError::Refused(_))),
            "{refused:?}"
        );

        let res = session
            .run_query(7, "SELECT id, name FROM t ORDER BY id", None, Some(2))
            .await
            .expect("query");
        assert_eq!(res.columns, vec!["id", "name"]);
        assert_eq!(res.rows.len(), 2);
        assert!(res.truncated);

        // Query agent tercatat di history, ditandai "(agent)".
        let (name,): (String,) =
            sqlx::query_as("SELECT connection_name FROM query_history ORDER BY id DESC LIMIT 1")
                .fetch_one(&cache)
                .await
                .expect("history");
        assert_eq!(name, "local (agent)");

        let missing = session.run_query(99, "SELECT 1", None, None).await;
        assert!(matches!(missing, Err(AgentError::ConnectionNotFound(99))));

        let _ = std::fs::remove_file(&db_file);
    }
}
