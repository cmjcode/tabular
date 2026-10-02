use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use log::error;
use serde::{Deserialize, Serialize};

use crate::models::enums::DatabaseType;
use crate::models::structs::ConnectionConfig;

// ─── Format & Configuration Enums ───────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BackupFormat {
    #[default]
    PlainSql,
    GzipSql,
    PostgresCustom,
    PostgresTar,
    PostgresDirectory,
    SqliteNative,
    /// Arsip `mongodump --archive --gzip`.
    MongoArchiveGzip,
    /// Arsip `mongodump --archive` tanpa kompresi.
    MongoArchive,
    /// `sqlpackage /Action:Export`: skema + data SQL Server.
    SqlServerBacpac,
    /// `sqlpackage /Action:Extract`: skema SQL Server saja.
    SqlServerDacpac,
}

impl BackupFormat {
    /// Semua format, dalam urutan tampil di dialog.
    pub const ALL: [BackupFormat; 10] = [
        BackupFormat::GzipSql,
        BackupFormat::PlainSql,
        BackupFormat::PostgresCustom,
        BackupFormat::PostgresTar,
        BackupFormat::PostgresDirectory,
        BackupFormat::SqliteNative,
        BackupFormat::MongoArchiveGzip,
        BackupFormat::MongoArchive,
        BackupFormat::SqlServerBacpac,
        BackupFormat::SqlServerDacpac,
    ];

    /// Format bawaan untuk engine.
    pub fn default_for(db_type: &DatabaseType) -> Self {
        match db_type {
            DatabaseType::SQLite => BackupFormat::SqliteNative,
            DatabaseType::MongoDB => BackupFormat::MongoArchiveGzip,
            DatabaseType::MsSQL => BackupFormat::SqlServerBacpac,
            _ => BackupFormat::GzipSql,
        }
    }

    /// Nama file tanpa ekstensi format backup mana pun (`db.sql.gz` -> `db`).
    pub fn strip_extension(file_name: &str) -> &str {
        let mut longest: Option<&str> = None;
        for fmt in Self::ALL {
            let ext = fmt.extension();
            if file_name.len() > ext.len() + 1
                && file_name.ends_with(ext)
                && file_name[..file_name.len() - ext.len()].ends_with('.')
                && longest.is_none_or(|l| ext.len() > l.len())
            {
                longest = Some(ext);
            }
        }
        match longest {
            Some(ext) => &file_name[..file_name.len() - ext.len() - 1],
            None => file_name.rsplit_once('.').map_or(file_name, |(stem, _)| stem),
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            BackupFormat::PlainSql => "sql",
            BackupFormat::GzipSql => "sql.gz",
            BackupFormat::PostgresCustom => "dump",
            BackupFormat::PostgresTar => "tar",
            BackupFormat::PostgresDirectory => "dir",
            BackupFormat::SqliteNative => "sqlite",
            BackupFormat::MongoArchiveGzip => "archive.gz",
            BackupFormat::MongoArchive => "archive",
            BackupFormat::SqlServerBacpac => "bacpac",
            BackupFormat::SqlServerDacpac => "dacpac",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            BackupFormat::PlainSql => "Plain SQL (.sql)",
            BackupFormat::GzipSql => "Compressed SQL (.sql.gz)",
            BackupFormat::PostgresCustom => "PostgreSQL Custom Archive (.dump)",
            BackupFormat::PostgresTar => "PostgreSQL Tar Archive (.tar)",
            BackupFormat::PostgresDirectory => "PostgreSQL Directory (.dir)",
            BackupFormat::SqliteNative => "SQLite Database File (.sqlite)",
            BackupFormat::MongoArchiveGzip => "MongoDB Archive, gzip (.archive.gz)",
            BackupFormat::MongoArchive => "MongoDB Archive (.archive)",
            BackupFormat::SqlServerBacpac => "SQL Server BACPAC, schema + data (.bacpac)",
            BackupFormat::SqlServerDacpac => "SQL Server DACPAC, schema only (.dacpac)",
        }
    }

    pub fn supported_for(&self, db_type: &DatabaseType) -> bool {
        match db_type {
            DatabaseType::PostgreSQL => matches!(
                self,
                BackupFormat::PlainSql
                    | BackupFormat::GzipSql
                    | BackupFormat::PostgresCustom
                    | BackupFormat::PostgresTar
                    | BackupFormat::PostgresDirectory
            ),
            DatabaseType::MySQL => {
                matches!(self, BackupFormat::PlainSql | BackupFormat::GzipSql)
            }
            DatabaseType::SQLite => {
                matches!(
                    self,
                    BackupFormat::SqliteNative | BackupFormat::GzipSql | BackupFormat::PlainSql
                )
            }
            DatabaseType::MongoDB => matches!(
                self,
                BackupFormat::MongoArchiveGzip | BackupFormat::MongoArchive
            ),
            DatabaseType::MsSQL => matches!(
                self,
                BackupFormat::SqlServerBacpac | BackupFormat::SqlServerDacpac
            ),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BackupContentScope {
    #[default]
    Both,
    SchemaOnly,
    DataOnly,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupOptions {
    pub database_name: String,
    pub target_file: PathBuf,
    pub format: BackupFormat,
    pub scope: BackupContentScope,
    pub selected_tables: Vec<String>,
    pub excluded_tables: Vec<String>,
    pub include_triggers_routines: bool,
    pub single_transaction: bool,
    pub clean_before_recreate: bool,
    pub no_owner: bool,
    pub no_privileges: bool,
    pub custom_binary_path: Option<PathBuf>,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            database_name: String::new(),
            target_file: PathBuf::new(),
            format: BackupFormat::GzipSql,
            scope: BackupContentScope::Both,
            selected_tables: Vec::new(),
            excluded_tables: Vec::new(),
            include_triggers_routines: true,
            single_transaction: true,
            clean_before_recreate: false,
            no_owner: true,
            no_privileges: false,
            custom_binary_path: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestoreOptions {
    pub target_database_name: String,
    pub source_file: PathBuf,
    pub clean_before_restore: bool,
    pub single_transaction: bool,
    pub stop_on_error: bool,
    pub data_only: bool,
    pub schema_only: bool,
    pub custom_binary_path: Option<PathBuf>,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            target_database_name: String::new(),
            source_file: PathBuf::new(),
            clean_before_restore: false,
            single_transaction: false,
            stop_on_error: true,
            data_only: false,
            schema_only: false,
            custom_binary_path: None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CopyDatabaseOptions {
    pub source_database_name: String,
    pub target_database_name: String,
    /// For SQLite: target file path on disk
    pub target_file: Option<PathBuf>,
    pub custom_binary_path: Option<PathBuf>,
    pub drop_target_if_exists: bool,
    pub include_routines: bool,
}

// ─── Progress & State Tracking ─────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    Backup,
    Restore,
    CopyDatabase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    Idle,
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    pub operation_type: OperationType,
    pub status: OperationStatus,
    pub database_name: String,
    pub file_path: PathBuf,
    pub bytes_processed: u64,
    pub pages_copied: usize,
    pub total_pages: usize,
    pub elapsed_secs: f64,
    pub bytes_per_sec: f64,
    pub current_stage: String,
    pub log_lines: Vec<String>,
}

pub struct ProgressTracker {
    operation_type: OperationType,
    status: OperationStatus,
    database_name: String,
    file_path: PathBuf,
    bytes_processed: u64,
    pages_copied: usize,
    total_pages: usize,
    start_time: Option<Instant>,
    end_time: Option<Instant>,
    current_stage: String,
    log_lines: VecDeque<String>,
    max_log_lines: usize,
}

impl ProgressTracker {
    pub fn new(op_type: OperationType, database_name: String, file_path: PathBuf) -> Self {
        Self {
            operation_type: op_type,
            status: OperationStatus::Idle,
            database_name,
            file_path,
            bytes_processed: 0,
            pages_copied: 0,
            total_pages: 0,
            start_time: None,
            end_time: None,
            current_stage: "Ready".to_string(),
            log_lines: VecDeque::with_capacity(100),
            max_log_lines: 200,
        }
    }

    pub fn start(&mut self, stage: impl Into<String>) {
        self.status = OperationStatus::Running;
        self.start_time = Some(Instant::now());
        self.end_time = None;
        self.current_stage = stage.into();
        self.append_log(format!(
            "[{}] Starting {:?} on database '{}'...",
            chrono::Local::now().format("%H:%M:%S"),
            self.operation_type,
            self.database_name
        ));
    }

    pub fn set_stage(&mut self, stage: impl Into<String>) {
        self.current_stage = stage.into();
    }

    pub fn add_bytes(&mut self, delta: u64) {
        self.bytes_processed = self.bytes_processed.saturating_add(delta);
    }

    pub fn set_pages(&mut self, copied: usize, total: usize) {
        self.pages_copied = copied;
        self.total_pages = total;
    }

    pub fn append_log(&mut self, line: impl Into<String>) {
        let l = line.into();
        if self.log_lines.len() >= self.max_log_lines {
            self.log_lines.pop_front();
        }
        self.log_lines.push_back(l);
    }

    pub fn complete(&mut self) {
        let end = Instant::now();
        self.end_time = Some(end);
        self.status = OperationStatus::Completed;
        self.current_stage = "Completed successfully".to_string();
        let elapsed = self.start_time.map_or(0.0, |t| (end - t).as_secs_f64());
        self.append_log(format!(
            "[{}] {:?} finished in {:.2}s ({} bytes processed)",
            chrono::Local::now().format("%H:%M:%S"),
            self.operation_type,
            elapsed,
            self.bytes_processed
        ));
    }

    pub fn fail(&mut self, err: impl Into<String>) {
        self.end_time = Some(Instant::now());
        let msg = err.into();
        self.status = OperationStatus::Failed(msg.clone());
        self.current_stage = format!("Failed: {}", msg);
        self.append_log(format!(
            "[{}] ❌ Error: {}",
            chrono::Local::now().format("%H:%M:%S"),
            msg
        ));
    }

    pub fn cancel(&mut self) {
        self.end_time = Some(Instant::now());
        self.status = OperationStatus::Cancelled;
        self.current_stage = "Cancelled by user".to_string();
        self.append_log(format!(
            "[{}] ⚠️ Operation was cancelled by user.",
            chrono::Local::now().format("%H:%M:%S")
        ));
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        let elapsed = match (self.start_time, self.end_time) {
            (Some(start), Some(end)) => (end - start).as_secs_f64(),
            (Some(start), None) => start.elapsed().as_secs_f64(),
            (None, _) => 0.0,
        };
        let speed = if elapsed > 0.05 {
            self.bytes_processed as f64 / elapsed
        } else {
            0.0
        };

        ProgressSnapshot {
            operation_type: self.operation_type,
            status: self.status.clone(),
            database_name: self.database_name.clone(),
            file_path: self.file_path.clone(),
            bytes_processed: self.bytes_processed,
            pages_copied: self.pages_copied,
            total_pages: self.total_pages,
            elapsed_secs: elapsed,
            bytes_per_sec: speed,
            current_stage: self.current_stage.clone(),
            log_lines: self.log_lines.iter().cloned().collect(),
        }
    }
}

// ─── Native Binary Detection ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeBinaryInfo {
    pub name: &'static str,
    pub path: PathBuf,
    pub version: Option<String>,
}

pub struct BinaryDetector;

impl BinaryDetector {
    /// Detect binary path for tools: pg_dump, pg_restore, mysqldump, mysql, psql
    pub fn find_binary(
        binary_name: &'static str,
        custom_path: Option<&Path>,
    ) -> Option<NativeBinaryInfo> {
        // 1. Check custom path override first
        if let Some(cp) = custom_path
            && cp.is_file()
        {
            let ver = Self::query_version(cp);
            return Some(NativeBinaryInfo {
                name: binary_name,
                path: cp.to_path_buf(),
                version: ver,
            });
        }

        // 2. Check PATH environment variable
        if let Ok(path_var) = std::env::var("PATH") {
            for dir in std::env::split_paths(&path_var) {
                let candidate = dir.join(binary_name);
                let exe_candidate = if cfg!(windows) {
                    dir.join(format!("{}.exe", binary_name))
                } else {
                    candidate.clone()
                };

                if exe_candidate.is_file() {
                    let ver = Self::query_version(&exe_candidate);
                    return Some(NativeBinaryInfo {
                        name: binary_name,
                        path: exe_candidate,
                        version: ver,
                    });
                }
            }
        }

        // 3. Fallback to platform-specific well-known directories
        let well_known_dirs = Self::get_known_directories(binary_name);
        for dir in well_known_dirs {
            let candidate = dir.join(binary_name);
            let exe_candidate = if cfg!(windows) {
                dir.join(format!("{}.exe", binary_name))
            } else {
                candidate
            };

            if exe_candidate.is_file() {
                let ver = Self::query_version(&exe_candidate);
                return Some(NativeBinaryInfo {
                    name: binary_name,
                    path: exe_candidate,
                    version: ver,
                });
            }
        }

        None
    }

    fn query_version(path: &Path) -> Option<String> {
        let output = Command::new(path).arg("--version").output().ok()?;
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !s.is_empty() {
                return Some(s);
            }
            let err_s = String::from_utf8_lossy(&output.stderr).trim().to_string();
            if !err_s.is_empty() {
                return Some(err_s);
            }
        }
        None
    }

    fn get_known_directories(binary_name: &str) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // `dotnet tool install -g microsoft.sqlpackage` memasang ke sini di
        // semua platform.
        if binary_name == "sqlpackage" {
            if let Some(home) = ::dirs::home_dir() {
                dirs.push(home.join(".dotnet").join("tools"));
            }
            if cfg!(target_os = "windows") {
                for v in [170, 160, 150] {
                    dirs.push(PathBuf::from(format!(
                        r"C:\Program Files\Microsoft SQL Server\{}\DAC\bin",
                        v
                    )));
                }
            }
        }
        if cfg!(target_os = "windows") && binary_name.starts_with("mongo") {
            dirs.push(PathBuf::from(r"C:\Program Files\MongoDB\Tools\100\bin"));
        }

        if cfg!(target_os = "macos") {
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));

            if binary_name.starts_with("pg_") || binary_name == "psql" {
                for v in [17, 16, 15, 14, 13, 12] {
                    dirs.push(PathBuf::from(format!(
                        "/opt/homebrew/opt/postgresql@{}/bin",
                        v
                    )));
                    dirs.push(PathBuf::from(format!(
                        "/usr/local/opt/postgresql@{}/bin",
                        v
                    )));
                }
                dirs.push(PathBuf::from("/opt/homebrew/opt/libpq/bin"));
                dirs.push(PathBuf::from("/usr/local/opt/libpq/bin"));
                dirs.push(PathBuf::from(
                    "/Applications/Postgres.app/Contents/Versions/latest/bin",
                ));
            } else if binary_name.starts_with("mysql") {
                dirs.push(PathBuf::from("/opt/homebrew/opt/mysql-client/bin"));
                dirs.push(PathBuf::from("/usr/local/opt/mysql-client/bin"));
                dirs.push(PathBuf::from("/usr/local/mysql/bin"));
            }
        } else if cfg!(target_os = "linux") {
            dirs.push(PathBuf::from("/usr/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));

            if binary_name.starts_with("pg_") || binary_name == "psql" {
                for v in [17, 16, 15, 14, 13, 12] {
                    dirs.push(PathBuf::from(format!("/usr/lib/postgresql/{}/bin", v)));
                }
            }
        } else if cfg!(target_os = "windows") {
            if binary_name.starts_with("pg_") || binary_name == "psql" {
                for v in [17, 16, 15, 14, 13, 12] {
                    dirs.push(PathBuf::from(format!(
                        r"C:\Program Files\PostgreSQL\{}\bin",
                        v
                    )));
                }
            } else if binary_name.starts_with("mysql") {
                for v in ["8.4", "8.0", "5.7"] {
                    dirs.push(PathBuf::from(format!(
                        r"C:\Program Files\MySQL\MySQL Server {}\bin",
                        v
                    )));
                }
                for v in ["11.4", "10.11", "10.6"] {
                    dirs.push(PathBuf::from(format!(
                        r"C:\Program Files\MariaDB {}\bin",
                        v
                    )));
                }
            }
        }

        dirs
    }
}

// ─── mysqldump Capability Detection ──────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct MysqldumpCapabilities {
    pub supports_gtid_purged: bool,
    pub supports_no_tablespaces: bool,
}

impl MysqldumpCapabilities {
    /// Detects mysqldump CLI capabilities across MySQL and MariaDB variants
    pub fn detect(binary_path: &Path, version_str: Option<&str>) -> Self {
        let is_mariadb = version_str
            .map(|v| v.to_lowercase().contains("mariadb"))
            .unwrap_or(false);

        let help_text = Command::new(binary_path)
            .arg("--help")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        let supports_gtid_purged = !is_mariadb && help_text.contains("--set-gtid-purged");
        let supports_no_tablespaces = help_text.contains("--no-tablespaces");

        Self {
            supports_gtid_purged,
            supports_no_tablespaces,
        }
    }
}

// ─── Pure-Rust SQLite Backup Engine ────────────────────────────────────────

pub struct SqliteBackupEngine;

impl SqliteBackupEngine {
    /// Performs an online SQLite backup using libsqlite3_sys C API
    pub fn backup(
        source_db_path: &Path,
        dest_db_path: &Path,
        compress_gzip: bool,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let temp_dest = if compress_gzip {
            dest_db_path.with_extension("tmp_backup_sqlite")
        } else {
            dest_db_path.to_path_buf()
        };

        if let Some(parent) = temp_dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let src_c_str = CString::new(source_db_path.to_string_lossy().as_bytes())
            .map_err(|e| format!("Invalid source path: {}", e))?;
        let dest_c_str = CString::new(temp_dest.to_string_lossy().as_bytes())
            .map_err(|e| format!("Invalid dest path: {}", e))?;

        unsafe {
            let mut p_src: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();
            let mut p_dest: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();

            // Open source in readonly mode
            let rc_src = libsqlite3_sys::sqlite3_open_v2(
                src_c_str.as_ptr(),
                &mut p_src,
                libsqlite3_sys::SQLITE_OPEN_READONLY,
                std::ptr::null(),
            );
            if rc_src != libsqlite3_sys::SQLITE_OK {
                let err_msg = Self::get_sqlite_errmsg(p_src);
                if !p_src.is_null() {
                    libsqlite3_sys::sqlite3_close(p_src);
                }
                return Err(format!(
                    "Failed to open source SQLite database: {}",
                    err_msg
                ));
            }

            // Open destination in readwrite | create mode
            let rc_dest = libsqlite3_sys::sqlite3_open_v2(
                dest_c_str.as_ptr(),
                &mut p_dest,
                libsqlite3_sys::SQLITE_OPEN_READWRITE | libsqlite3_sys::SQLITE_OPEN_CREATE,
                std::ptr::null(),
            );
            if rc_dest != libsqlite3_sys::SQLITE_OK {
                let err_msg = Self::get_sqlite_errmsg(p_dest);
                libsqlite3_sys::sqlite3_close(p_src);
                if !p_dest.is_null() {
                    libsqlite3_sys::sqlite3_close(p_dest);
                }
                return Err(format!(
                    "Failed to create destination SQLite file: {}",
                    err_msg
                ));
            }

            let main_db = c"main".as_ptr();
            let p_backup = libsqlite3_sys::sqlite3_backup_init(p_dest, main_db, p_src, main_db);

            if p_backup.is_null() {
                let err_msg = Self::get_sqlite_errmsg(p_dest);
                libsqlite3_sys::sqlite3_close(p_dest);
                libsqlite3_sys::sqlite3_close(p_src);
                return Err(format!(
                    "Failed to initialize SQLite backup handle: {}",
                    err_msg
                ));
            }

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.start("Copying SQLite database pages...");
            }

            let pages_per_step = 250;
            let mut done = false;

            while !done {
                if cancel_token.load(Ordering::Relaxed) {
                    libsqlite3_sys::sqlite3_backup_finish(p_backup);
                    libsqlite3_sys::sqlite3_close(p_dest);
                    libsqlite3_sys::sqlite3_close(p_src);
                    let _ = std::fs::remove_file(&temp_dest);
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.cancel();
                    return Ok(());
                }

                let rc = libsqlite3_sys::sqlite3_backup_step(p_backup, pages_per_step);
                let remaining = libsqlite3_sys::sqlite3_backup_remaining(p_backup);
                let pagecount = libsqlite3_sys::sqlite3_backup_pagecount(p_backup);

                let copied = if pagecount >= remaining {
                    (pagecount - remaining) as usize
                } else {
                    0
                };
                let total = pagecount.max(0) as usize;

                // Estimate page size ~4096 bytes for byte counter
                let bytes_est = (copied as u64) * 4096;

                {
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.set_pages(copied, total);
                    trk.bytes_processed = bytes_est;
                }

                if rc == libsqlite3_sys::SQLITE_DONE {
                    done = true;
                } else if rc == libsqlite3_sys::SQLITE_OK {
                    // Continue immediately
                } else if rc == libsqlite3_sys::SQLITE_BUSY || rc == libsqlite3_sys::SQLITE_LOCKED {
                    std::thread::sleep(Duration::from_millis(20));
                } else {
                    let err_msg = Self::get_sqlite_errmsg(p_dest);
                    libsqlite3_sys::sqlite3_backup_finish(p_backup);
                    libsqlite3_sys::sqlite3_close(p_dest);
                    libsqlite3_sys::sqlite3_close(p_src);
                    let _ = std::fs::remove_file(&temp_dest);
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.fail(format!("SQLite backup step error: {}", err_msg));
                    return Err(format!("SQLite backup failed: {}", err_msg));
                }
            }

            libsqlite3_sys::sqlite3_backup_finish(p_backup);
            libsqlite3_sys::sqlite3_close(p_dest);
            libsqlite3_sys::sqlite3_close(p_src);
        }

        // If compression is requested, stream-compress the backup file to destination
        if compress_gzip {
            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.set_stage("Compressing SQLite backup with gzip...");
                trk.append_log("Compressing raw database file to .gz archive...");
            }

            let input_file = File::open(&temp_dest)
                .map_err(|e| format!("Failed to open temp backup for compression: {}", e))?;
            let output_file = File::create(dest_db_path)
                .map_err(|e| format!("Failed to create destination gzip file: {}", e))?;

            let mut encoder = GzEncoder::new(output_file, Compression::default());
            let mut reader = BufReader::with_capacity(128 * 1024, input_file);
            let mut buffer = [0u8; 64 * 1024];
            let mut total_compressed_in = 0u64;

            loop {
                if cancel_token.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_file(&temp_dest);
                    let _ = std::fs::remove_file(dest_db_path);
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.cancel();
                    return Ok(());
                }

                let read_bytes = reader
                    .read(&mut buffer)
                    .map_err(|e| format!("Read error during compression: {}", e))?;
                if read_bytes == 0 {
                    break;
                }

                encoder
                    .write_all(&buffer[..read_bytes])
                    .map_err(|e| format!("Write error during compression: {}", e))?;
                total_compressed_in += read_bytes as u64;

                {
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.bytes_processed = total_compressed_in;
                }
            }

            encoder
                .finish()
                .map_err(|e| format!("Failed to finalize gzip stream: {}", e))?;
            let _ = std::fs::remove_file(&temp_dest);
        }

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.complete();
        }

        Ok(())
    }

    /// Restores a SQLite database from a backup file (.sqlite, .db, or .gz)
    pub fn restore(
        source_backup_path: &Path,
        dest_db_path: &Path,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let is_gzipped = source_backup_path
            .extension()
            .is_some_and(|ext| ext == "gz");

        let raw_source_path = if is_gzipped {
            let temp_uncompressed = dest_db_path.with_extension("tmp_restore_sqlite");
            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.start("Decompressing gzip archive...");
            }

            let input_file = File::open(source_backup_path)
                .map_err(|e| format!("Failed to open gzip source file: {}", e))?;
            let mut decoder = GzDecoder::new(input_file);
            let mut output_file = File::create(&temp_uncompressed)
                .map_err(|e| format!("Failed to create temp restore file: {}", e))?;

            let mut buffer = [0u8; 64 * 1024];
            let mut decompressed_bytes = 0u64;

            loop {
                if cancel_token.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_file(&temp_uncompressed);
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.cancel();
                    return Ok(());
                }

                let read_bytes = decoder
                    .read(&mut buffer)
                    .map_err(|e| format!("Error decompressing: {}", e))?;
                if read_bytes == 0 {
                    break;
                }

                output_file
                    .write_all(&buffer[..read_bytes])
                    .map_err(|e| format!("Error writing decompressed data: {}", e))?;
                decompressed_bytes += read_bytes as u64;

                {
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.bytes_processed = decompressed_bytes;
                }
            }

            temp_uncompressed
        } else {
            source_backup_path.to_path_buf()
        };

        // Online backup from raw source into destination database
        let res = Self::backup(
            &raw_source_path,
            dest_db_path,
            false,
            tracker.clone(),
            cancel_token,
        );

        if is_gzipped {
            let _ = std::fs::remove_file(&raw_source_path);
        }

        res
    }

    unsafe fn get_sqlite_errmsg(db: *mut libsqlite3_sys::sqlite3) -> String {
        if db.is_null() {
            return "Null SQLite handle".to_string();
        }
        unsafe {
            let msg_ptr = libsqlite3_sys::sqlite3_errmsg(db);
            if msg_ptr.is_null() {
                return "Unknown SQLite error".to_string();
            }
            CStr::from_ptr(msg_ptr).to_string_lossy().to_string()
        }
    }
}

// ─── MongoDB & SQL Server command lines ────────────────────────────────────

/// File konfigurasi sementara berisi password untuk `mongodump`/`mongorestore`
/// (`--config`), supaya password tidak muncul di daftar proses. Dihapus saat
/// di-drop.
struct MongoPasswordFile {
    path: PathBuf,
}

impl MongoPasswordFile {
    fn create(password: &str) -> Result<Option<Self>, String> {
        if password.is_empty() {
            return Ok(None);
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "tabular-mongo-{}-{}.yaml",
            std::process::id(),
            nanos
        ));
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut file = open
            .open(&path)
            .map_err(|e| format!("Failed to create MongoDB tools config: {}", e))?;
        // String YAML berkutip ganda: hanya `\` dan `"` yang perlu di-escape.
        let escaped = password.replace('\\', "\\\\").replace('"', "\\\"");
        let guard = Self { path };
        file.write_all(format!("password: \"{}\"\n", escaped).as_bytes())
            .map_err(|e| format!("Failed to write MongoDB tools config: {}", e))?;
        Ok(Some(guard))
    }
}

impl Drop for MongoPasswordFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn mongo_connection_args(config: &ConnectionConfig, password_file: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "--host".to_string(),
        config.host.clone(),
        "--port".to_string(),
        config.port.clone(),
    ];
    if !config.username.is_empty() {
        args.push("--username".to_string());
        args.push(config.username.clone());
        args.push("--authenticationDatabase".to_string());
        args.push("admin".to_string());
    }
    if let Some(path) = password_file {
        args.push("--config".to_string());
        args.push(path.display().to_string());
    }
    args
}

/// Argumen `mongodump` untuk satu database ke satu file arsip.
pub fn mongodump_args(
    config: &ConnectionConfig,
    options: &BackupOptions,
    password_file: Option<&Path>,
) -> Vec<String> {
    let mut args = mongo_connection_args(config, password_file);
    args.push("--db".to_string());
    args.push(options.database_name.clone());
    args.push(format!("--archive={}", options.target_file.display()));
    if options.format == BackupFormat::MongoArchiveGzip {
        args.push("--gzip".to_string());
    }
    // `--collection` hanya menerima satu nama; selebihnya lewat pengecualian.
    if let [only] = options.selected_tables.as_slice() {
        args.push("--collection".to_string());
        args.push(only.clone());
    } else {
        for collection in &options.excluded_tables {
            args.push("--excludeCollection".to_string());
            args.push(collection.clone());
        }
    }
    args
}

/// Argumen `mongorestore` dari file arsip. Bila nama database tujuan diisi,
/// semua namespace di arsip dipetakan ke database itu.
pub fn mongorestore_args(
    config: &ConnectionConfig,
    options: &RestoreOptions,
    password_file: Option<&Path>,
) -> Vec<String> {
    let mut args = mongo_connection_args(config, password_file);
    args.push(format!("--archive={}", options.source_file.display()));
    if options
        .source_file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gz"))
    {
        args.push("--gzip".to_string());
    }
    if options.clean_before_restore {
        args.push("--drop".to_string());
    }
    if options.stop_on_error {
        args.push("--stopOnError".to_string());
    }
    let target = options.target_database_name.trim();
    if !target.is_empty() {
        args.push("--nsFrom=$db$.$coll$".to_string());
        args.push(format!("--nsTo={}.$coll$", target));
    }
    args
}

fn sqlpackage_connection_args(
    config: &ConnectionConfig,
    side: &str,
    database: &str,
) -> Vec<String> {
    let server = if config.port.trim().is_empty() {
        config.host.clone()
    } else {
        format!("{},{}", config.host, config.port.trim())
    };
    let mut args = vec![
        format!("/{side}ServerName:{server}"),
        format!("/{side}DatabaseName:{database}"),
    ];
    // Tanpa username: autentikasi terintegrasi (Windows / Kerberos).
    if !config.username.is_empty() {
        args.push(format!("/{side}User:{}", config.username));
        args.push(format!("/{side}Password:{}", config.password));
    }
    let verify = config.ssl_enabled && config.ssl_verify_server;
    args.push(format!(
        "/{side}EncryptConnection:{}",
        if config.ssl_enabled { "True" } else { "False" }
    ));
    args.push(format!(
        "/{side}TrustServerCertificate:{}",
        if verify { "False" } else { "True" }
    ));
    args
}

/// Argumen `sqlpackage` untuk backup: BACPAC (skema + data) atau DACPAC
/// (skema saja).
pub fn sqlpackage_export_args(config: &ConnectionConfig, options: &BackupOptions) -> Vec<String> {
    let action = if options.format == BackupFormat::SqlServerDacpac {
        "Extract"
    } else {
        "Export"
    };
    let mut args = vec![
        format!("/Action:{action}"),
        format!("/TargetFile:{}", options.target_file.display()),
    ];
    args.extend(sqlpackage_connection_args(
        config,
        "Source",
        &options.database_name,
    ));
    args
}

/// Argumen `sqlpackage` untuk restore: `Import` untuk `.bacpac`, `Publish`
/// untuk `.dacpac`.
pub fn sqlpackage_import_args(config: &ConnectionConfig, options: &RestoreOptions) -> Vec<String> {
    let is_dacpac = options
        .source_file
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dacpac"));
    let mut args = vec![
        format!("/Action:{}", if is_dacpac { "Publish" } else { "Import" }),
        format!("/SourceFile:{}", options.source_file.display()),
    ];
    args.extend(sqlpackage_connection_args(
        config,
        "Target",
        &options.target_database_name,
    ));
    args
}

// ─── Native Process Backup & Restore Runner ────────────────────────────────

pub struct BackupRestoreRunner;

impl BackupRestoreRunner {
    /// Launches a background backup operation for Postgres, MySQL, or SQLite
    pub fn run_backup(
        config: &ConnectionConfig,
        options: BackupOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) {
        let config_clone = config.clone();
        std::thread::spawn(move || {
            let res = match config_clone.connection_type {
                DatabaseType::SQLite => {
                    let source_path = PathBuf::from(&config_clone.database);
                    let compress = matches!(options.format, BackupFormat::GzipSql);
                    SqliteBackupEngine::backup(
                        &source_path,
                        &options.target_file,
                        compress,
                        tracker.clone(),
                        cancel_token,
                    )
                }
                DatabaseType::PostgreSQL => {
                    Self::run_postgres_dump(&config_clone, &options, tracker.clone(), cancel_token)
                }
                DatabaseType::MySQL => {
                    Self::run_mysql_dump(&config_clone, &options, tracker.clone(), cancel_token)
                }
                DatabaseType::MongoDB => {
                    Self::run_mongodump(&config_clone, &options, tracker.clone(), cancel_token)
                }
                DatabaseType::MsSQL => Self::run_sqlpackage(
                    sqlpackage_export_args(&config_clone, &options),
                    options.custom_binary_path.as_deref(),
                    tracker.clone(),
                    cancel_token,
                ),
                _ => {
                    let err = format!(
                        "Backup is not supported for {:?}",
                        config_clone.connection_type
                    );
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.fail(&err);
                    Err(err)
                }
            };

            if let Err(e) = res {
                error!("Backup job failed: {}", e);
            }
        });
    }

    /// Launches a background restore operation for Postgres, MySQL, or SQLite
    pub fn run_restore(
        config: &ConnectionConfig,
        options: RestoreOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) {
        let config_clone = config.clone();
        std::thread::spawn(move || {
            let res = match config_clone.connection_type {
                DatabaseType::SQLite => {
                    let dest_path = PathBuf::from(&config_clone.database);
                    SqliteBackupEngine::restore(
                        &options.source_file,
                        &dest_path,
                        tracker.clone(),
                        cancel_token,
                    )
                }
                DatabaseType::PostgreSQL => Self::run_postgres_restore(
                    &config_clone,
                    &options,
                    tracker.clone(),
                    cancel_token,
                ),
                DatabaseType::MySQL => {
                    Self::run_mysql_restore(&config_clone, &options, tracker.clone(), cancel_token)
                }
                DatabaseType::MongoDB => {
                    Self::run_mongorestore(&config_clone, &options, tracker.clone(), cancel_token)
                }
                DatabaseType::MsSQL => Self::run_sqlpackage(
                    sqlpackage_import_args(&config_clone, &options),
                    options.custom_binary_path.as_deref(),
                    tracker.clone(),
                    cancel_token,
                ),
                _ => {
                    let err = format!(
                        "Restore is not supported for {:?}",
                        config_clone.connection_type
                    );
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.fail(&err);
                    Err(err)
                }
            };

            if let Err(e) = res {
                error!("Restore job failed: {}", e);
            }
        });
    }

    /// Launches a background copy database operation for Postgres, MySQL, or SQLite
    pub fn run_copy_database(
        config: &ConnectionConfig,
        options: CopyDatabaseOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) {
        let config_clone = config.clone();
        std::thread::spawn(move || {
            let res = match config_clone.connection_type {
                DatabaseType::SQLite => {
                    let source_path = PathBuf::from(&config_clone.database);
                    let target_path = options.target_file.clone().unwrap_or_else(|| {
                        PathBuf::from(format!("{}_copy.sqlite", config_clone.database))
                    });
                    if options.drop_target_if_exists && target_path.exists() {
                        let _ = std::fs::remove_file(&target_path);
                    }
                    if let Some(parent) = target_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    SqliteBackupEngine::backup(
                        &source_path,
                        &target_path,
                        false,
                        tracker.clone(),
                        cancel_token,
                    )
                }
                DatabaseType::PostgreSQL => {
                    Self::run_postgres_copy(&config_clone, &options, tracker.clone(), cancel_token)
                }
                DatabaseType::MySQL => {
                    Self::run_mysql_copy(&config_clone, &options, tracker.clone(), cancel_token)
                }
                _ => {
                    let err = format!(
                        "Copy database is not supported for {:?}",
                        config_clone.connection_type
                    );
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.fail(&err);
                    Err(err)
                }
            };

            if let Err(e) = res {
                error!("Copy database job failed: {}", e);
            }
        });
    }

    // ─── MongoDB & SQL Server (mongodump / mongorestore / sqlpackage) ───────

    fn require_binary(
        name: &'static str,
        custom: Option<&Path>,
        hint: &str,
        tracker: &Arc<Mutex<ProgressTracker>>,
    ) -> Result<NativeBinaryInfo, String> {
        BinaryDetector::find_binary(name, custom).ok_or_else(|| {
            let msg = format!("{name} binary not found in PATH or standard directories. {hint}");
            tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fail(&msg);
            msg
        })
    }

    fn announce(tracker: &Arc<Mutex<ProgressTracker>>, binary: &NativeBinaryInfo) {
        let mut trk = tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        trk.start(format!("Spawning {} process...", binary.name));
        trk.append_log(format!(
            "Using {}: {} ({})",
            binary.name,
            binary.path.display(),
            binary.version.as_deref().unwrap_or("unknown version")
        ));
    }

    fn run_mongodump(
        config: &ConnectionConfig,
        options: &BackupOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let binary = Self::require_binary(
            "mongodump",
            options.custom_binary_path.as_deref(),
            "Install MongoDB Database Tools.",
            &tracker,
        )?;
        Self::announce(&tracker, &binary);
        if let Some(parent) = options.target_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let password_file = MongoPasswordFile::create(&config.password)?;
        let mut cmd = Command::new(&binary.path);
        cmd.args(mongodump_args(
            config,
            options,
            password_file.as_ref().map(|f| f.path.as_path()),
        ));
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn mongodump: {}", e))?;
        Self::monitor_process_with_file_growth(
            &mut child,
            &options.target_file,
            tracker,
            cancel_token,
        )
    }

    fn run_mongorestore(
        config: &ConnectionConfig,
        options: &RestoreOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let binary = Self::require_binary(
            "mongorestore",
            options.custom_binary_path.as_deref(),
            "Install MongoDB Database Tools.",
            &tracker,
        )?;
        Self::announce(&tracker, &binary);
        let password_file = MongoPasswordFile::create(&config.password)?;
        let mut cmd = Command::new(&binary.path);
        cmd.args(mongorestore_args(
            config,
            options,
            password_file.as_ref().map(|f| f.path.as_path()),
        ));
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn mongorestore: {}", e))?;
        Self::monitor_process_simple(&mut child, tracker, cancel_token)
    }

    fn run_sqlpackage(
        args: Vec<String>,
        custom_binary: Option<&Path>,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let binary = Self::require_binary(
            "sqlpackage",
            custom_binary,
            "Install it with `dotnet tool install -g microsoft.sqlpackage`.",
            &tracker,
        )?;
        Self::announce(&tracker, &binary);
        let mut cmd = Command::new(&binary.path);
        cmd.args(args);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn sqlpackage: {}", e))?;
        Self::monitor_process_simple(&mut child, tracker, cancel_token)
    }

    // ─── PostgreSQL DUMP Runner ─────────────────────────────────────────────

    fn run_postgres_dump(
        config: &ConnectionConfig,
        options: &BackupOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let binary_info = BinaryDetector::find_binary("pg_dump", options.custom_binary_path.as_deref())
            .ok_or_else(|| {
                let msg = "pg_dump binary not found in PATH or standard directories. Please install PostgreSQL client tools.".to_string();
                tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fail(&msg);
                msg
            })?;

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.start("Spawning pg_dump process...");
            trk.append_log(format!(
                "Using pg_dump: {} ({})",
                binary_info.path.display(),
                binary_info.version.as_deref().unwrap_or("unknown version")
            ));
        }

        if let Some(parent) = options.target_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut cmd = Command::new(&binary_info.path);

        // Connection arguments
        cmd.arg("-h").arg(&config.host);
        cmd.arg("-p").arg(&config.port);
        if !config.username.is_empty() {
            cmd.arg("-U").arg(&config.username);
        }
        cmd.arg("-d").arg(&options.database_name);

        // Security: Pass password strictly via environment variable
        if !config.password.is_empty() {
            cmd.env("PGPASSWORD", &config.password);
        }

        // Content Scope
        match options.scope {
            BackupContentScope::SchemaOnly => {
                cmd.arg("--schema-only");
            }
            BackupContentScope::DataOnly => {
                cmd.arg("--data-only");
            }
            BackupContentScope::Both => {}
        }

        // Table filters
        for tbl in &options.selected_tables {
            cmd.arg("-t").arg(tbl);
        }
        for tbl in &options.excluded_tables {
            cmd.arg("-T").arg(tbl);
        }

        // Options
        if options.clean_before_recreate {
            cmd.arg("--clean").arg("--if-exists");
        }
        if options.no_owner {
            cmd.arg("--no-owner");
        }
        if options.no_privileges {
            cmd.arg("--no-privileges");
        }

        let is_piped_gzip = options.format == BackupFormat::GzipSql;

        match options.format {
            BackupFormat::PostgresCustom => {
                cmd.arg("-F").arg("c");
                cmd.arg("-f").arg(&options.target_file);
            }
            BackupFormat::PostgresTar => {
                cmd.arg("-F").arg("t");
                cmd.arg("-f").arg(&options.target_file);
            }
            BackupFormat::PostgresDirectory => {
                cmd.arg("-F").arg("d");
                cmd.arg("-f").arg(&options.target_file);
            }
            BackupFormat::PlainSql => {
                cmd.arg("-F").arg("p");
                cmd.arg("-f").arg(&options.target_file);
            }
            BackupFormat::GzipSql => {
                cmd.arg("-F").arg("p");
                // Stream output to stdout for compression pipe
            }
            // Format engine lain tidak ditawarkan untuk PostgreSQL.
            BackupFormat::SqliteNative
            | BackupFormat::MongoArchiveGzip
            | BackupFormat::MongoArchive
            | BackupFormat::SqlServerBacpac
            | BackupFormat::SqlServerDacpac => {}
        }

        cmd.stdout(if is_piped_gzip {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn pg_dump: {}", e))?;

        if is_piped_gzip {
            Self::pipe_stdout_to_gzip(
                &mut child,
                &options.target_file,
                tracker.clone(),
                cancel_token,
            )?;
        } else {
            Self::monitor_process_with_file_growth(
                &mut child,
                &options.target_file,
                tracker.clone(),
                cancel_token,
            )?;
        }

        Ok(())
    }

    // ─── PostgreSQL RESTORE Runner ──────────────────────────────────────────

    fn run_postgres_restore(
        config: &ConnectionConfig,
        options: &RestoreOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let is_custom_format = options
            .source_file
            .extension()
            .is_some_and(|ext| ext == "dump" || ext == "pgdump" || ext == "tar" || ext == "dir");

        if is_custom_format {
            let binary_info =
                BinaryDetector::find_binary("pg_restore", options.custom_binary_path.as_deref())
                    .ok_or_else(|| {
                        let msg = "pg_restore binary not found in PATH or standard directories."
                            .to_string();
                        tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .fail(&msg);
                        msg
                    })?;

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.start("Spawning pg_restore process...");
                trk.append_log(format!("Using pg_restore: {}", binary_info.path.display()));
            }

            let mut cmd = Command::new(&binary_info.path);
            cmd.arg("-h").arg(&config.host);
            cmd.arg("-p").arg(&config.port);
            if !config.username.is_empty() {
                cmd.arg("-U").arg(&config.username);
            }
            cmd.arg("-d").arg(&options.target_database_name);

            if !config.password.is_empty() {
                cmd.env("PGPASSWORD", &config.password);
            }

            if options.clean_before_restore {
                cmd.arg("--clean").arg("--if-exists");
            }
            if options.single_transaction {
                cmd.arg("-1");
            }
            if options.data_only {
                cmd.arg("--data-only");
            }
            if options.schema_only {
                cmd.arg("--schema-only");
            }
            cmd.arg("--no-owner");
            cmd.arg(&options.source_file);

            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            let mut child = cmd
                .spawn()
                .map_err(|e| format!("Failed to spawn pg_restore: {}", e))?;

            Self::monitor_process_simple(&mut child, tracker, cancel_token)?;
        } else {
            // Plain SQL or .sql.gz restore using psql
            let binary_info =
                BinaryDetector::find_binary("psql", options.custom_binary_path.as_deref())
                    .ok_or_else(|| {
                        let msg =
                            "psql binary not found in PATH or standard directories.".to_string();
                        tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .fail(&msg);
                        msg
                    })?;

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.start("Spawning psql restore process...");
                trk.append_log(format!("Using psql: {}", binary_info.path.display()));
            }

            let mut cmd = Command::new(&binary_info.path);
            cmd.arg("-h").arg(&config.host);
            cmd.arg("-p").arg(&config.port);
            if !config.username.is_empty() {
                cmd.arg("-U").arg(&config.username);
            }
            cmd.arg("-d").arg(&options.target_database_name);
            if options.stop_on_error {
                cmd.arg("-v").arg("ON_ERROR_STOP=1");
            }
            if options.single_transaction {
                cmd.arg("-1");
            }

            if !config.password.is_empty() {
                cmd.env("PGPASSWORD", &config.password);
            }

            cmd.stdin(Stdio::piped());
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());

            let mut child = cmd
                .spawn()
                .map_err(|e| format!("Failed to spawn psql: {}", e))?;

            Self::feed_file_to_stdin(&mut child, &options.source_file, tracker, cancel_token)?;
        }

        Ok(())
    }

    // ─── MySQL DUMP Runner ──────────────────────────────────────────────────

    fn run_mysql_dump(
        config: &ConnectionConfig,
        options: &BackupOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let binary_info = BinaryDetector::find_binary("mysqldump", options.custom_binary_path.as_deref())
            .ok_or_else(|| {
                let msg = "mysqldump binary not found in PATH or standard directories. Please install MySQL client tools.".to_string();
                tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fail(&msg);
                msg
            })?;

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.start("Spawning mysqldump process...");
            trk.append_log(format!(
                "Using mysqldump: {} ({})",
                binary_info.path.display(),
                binary_info.version.as_deref().unwrap_or("unknown version")
            ));
        }

        if let Some(parent) = options.target_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut cmd = Command::new(&binary_info.path);

        cmd.arg("-h").arg(&config.host);
        cmd.arg("-P").arg(&config.port);
        if !config.username.is_empty() {
            cmd.arg("-u").arg(&config.username);
        }

        // Security: Pass password via MYSQL_PWD environment variable
        if !config.password.is_empty() {
            cmd.env("MYSQL_PWD", &config.password);
        }

        let caps = MysqldumpCapabilities::detect(&binary_info.path, binary_info.version.as_deref());

        if options.single_transaction {
            cmd.arg("--single-transaction");
        }
        if options.include_triggers_routines {
            cmd.arg("--routines").arg("--triggers");
        }
        if options.clean_before_recreate {
            cmd.arg("--add-drop-table");
        }
        if caps.supports_gtid_purged {
            cmd.arg("--set-gtid-purged=OFF");
        }
        if caps.supports_no_tablespaces {
            cmd.arg("--no-tablespaces");
        }

        match options.scope {
            BackupContentScope::SchemaOnly => {
                cmd.arg("--no-data");
            }
            BackupContentScope::DataOnly => {
                cmd.arg("--no-create-info");
            }
            BackupContentScope::Both => {}
        }

        cmd.arg(&options.database_name);

        for tbl in &options.selected_tables {
            cmd.arg(tbl);
        }

        for tbl in &options.excluded_tables {
            cmd.arg(format!("--ignore-table={}.{}", options.database_name, tbl));
        }

        let is_piped_gzip = options.format == BackupFormat::GzipSql;

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn mysqldump: {}", e))?;

        if is_piped_gzip {
            Self::pipe_stdout_to_gzip(
                &mut child,
                &options.target_file,
                tracker.clone(),
                cancel_token,
            )?;
        } else {
            Self::pipe_stdout_to_file(
                &mut child,
                &options.target_file,
                tracker.clone(),
                cancel_token,
            )?;
        }

        Ok(())
    }

    // ─── MySQL RESTORE Runner ───────────────────────────────────────────────

    fn run_mysql_restore(
        config: &ConnectionConfig,
        options: &RestoreOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let binary_info =
            BinaryDetector::find_binary("mysql", options.custom_binary_path.as_deref())
                .ok_or_else(|| {
                    let msg = "mysql client binary not found in PATH or standard directories."
                        .to_string();
                    tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .fail(&msg);
                    msg
                })?;

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.start("Spawning mysql client restore process...");
            trk.append_log(format!("Using mysql: {}", binary_info.path.display()));
        }

        let mut cmd = Command::new(&binary_info.path);
        cmd.arg("-h").arg(&config.host);
        cmd.arg("-P").arg(&config.port);
        if !config.username.is_empty() {
            cmd.arg("-u").arg(&config.username);
        }
        cmd.arg("-D").arg(&options.target_database_name);

        if !config.password.is_empty() {
            cmd.env("MYSQL_PWD", &config.password);
        }

        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn mysql: {}", e))?;

        Self::feed_file_to_stdin(&mut child, &options.source_file, tracker, cancel_token)?;

        Ok(())
    }

    // ─── Stream Utilities ───────────────────────────────────────────────────

    /// Reads stdout from child and writes compressed gzip stream to output file
    fn pipe_stdout_to_gzip(
        child: &mut Child,
        target_file: &Path,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let stderr = child.stderr.take();

        // Spawn stderr logging thread
        if let Some(err_pipe) = stderr {
            let trk_stderr = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_stderr
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        let out_file = File::create(target_file)
            .map_err(|e| format!("Failed to create output file: {}", e))?;
        let mut encoder = GzEncoder::new(out_file, Compression::default());
        let mut buffer = [0u8; 64 * 1024];

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = std::fs::remove_file(target_file);
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.cancel();
                return Ok(());
            }

            let read_bytes = stdout
                .read(&mut buffer)
                .map_err(|e| format!("Error reading dump stream: {}", e))?;
            if read_bytes == 0 {
                break;
            }

            encoder
                .write_all(&buffer[..read_bytes])
                .map_err(|e| format!("Error writing compressed stream: {}", e))?;

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.add_bytes(read_bytes as u64);
            }
        }

        encoder
            .finish()
            .map_err(|e| format!("Failed to finalize gzip archive: {}", e))?;

        let status = child
            .wait()
            .map_err(|e| format!("Error waiting for process: {}", e))?;

        if status.success() {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.complete();
            Ok(())
        } else {
            let msg = format!("Process exited with status code {:?}", status.code());
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.fail(&msg);
            Err(msg)
        }
    }

    /// Reads stdout from child and writes directly to output file
    fn pipe_stdout_to_file(
        child: &mut Child,
        target_file: &Path,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut stdout = child.stdout.take().ok_or("Failed to capture stdout")?;
        let stderr = child.stderr.take();

        if let Some(err_pipe) = stderr {
            let trk_stderr = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_stderr
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        let mut out_file = File::create(target_file)
            .map_err(|e| format!("Failed to create output file: {}", e))?;
        let mut buffer = [0u8; 64 * 1024];

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = std::fs::remove_file(target_file);
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.cancel();
                return Ok(());
            }

            let read_bytes = stdout
                .read(&mut buffer)
                .map_err(|e| format!("Error reading dump stream: {}", e))?;
            if read_bytes == 0 {
                break;
            }

            out_file
                .write_all(&buffer[..read_bytes])
                .map_err(|e| format!("Error writing dump file: {}", e))?;

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.add_bytes(read_bytes as u64);
            }
        }

        out_file
            .flush()
            .map_err(|e| format!("Flush error: {}", e))?;

        let status = child
            .wait()
            .map_err(|e| format!("Error waiting for process: {}", e))?;

        if status.success() {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.complete();
            Ok(())
        } else {
            let msg = format!("Process exited with status code {:?}", status.code());
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.fail(&msg);
            Err(msg)
        }
    }

    /// Feeds file (raw or decompressed if .gz) into child stdin
    fn feed_file_to_stdin(
        child: &mut Child,
        source_file: &Path,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut stdin = child.stdin.take().ok_or("Failed to open child stdin")?;
        let stderr = child.stderr.take();

        if let Some(err_pipe) = stderr {
            let trk_stderr = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_stderr
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        let is_gzipped = source_file.extension().is_some_and(|ext| ext == "gz");
        let file = File::open(source_file)
            .map_err(|e| format!("Failed to open source file for restore: {}", e))?;

        let mut reader: Box<dyn Read> = if is_gzipped {
            Box::new(GzDecoder::new(file))
        } else {
            Box::new(BufReader::with_capacity(128 * 1024, file))
        };

        let mut buffer = [0u8; 64 * 1024];

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.cancel();
                return Ok(());
            }

            let read_bytes = reader
                .read(&mut buffer)
                .map_err(|e| format!("Read error during restore stream: {}", e))?;
            if read_bytes == 0 {
                break;
            }

            stdin
                .write_all(&buffer[..read_bytes])
                .map_err(|e| format!("Write error to database stdin: {}", e))?;

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.add_bytes(read_bytes as u64);
            }
        }

        drop(stdin); // Close stdin to signal EOF to database process

        let status = child
            .wait()
            .map_err(|e| format!("Error waiting for restore process: {}", e))?;

        if status.success() {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.complete();
            Ok(())
        } else {
            let msg = format!("Restore process exited with code {:?}", status.code());
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.fail(&msg);
            Err(msg)
        }
    }

    /// Monitors a background process while tracking file size growth on disk
    fn monitor_process_with_file_growth(
        child: &mut Child,
        target_file: &Path,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let stderr = child.stderr.take();
        if let Some(err_pipe) = stderr {
            let trk_stderr = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_stderr
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = std::fs::remove_file(target_file);
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.cancel();
                return Ok(());
            }

            if let Ok(metadata) = std::fs::metadata(target_file) {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.bytes_processed = metadata.len();
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Ok(metadata) = std::fs::metadata(target_file) {
                        let mut trk = tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        trk.bytes_processed = metadata.len();
                    }

                    if status.success() {
                        let mut trk = tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        trk.complete();
                        return Ok(());
                    } else {
                        let msg = format!("Process failed with exit code {:?}", status.code());
                        let mut trk = tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        trk.fail(&msg);
                        return Err(msg);
                    }
                }
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    let msg = format!("Error checking process status: {}", e);
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.fail(&msg);
                    return Err(msg);
                }
            }
        }
    }

    /// Simple process monitor that captures stdout/stderr and waits for completion
    fn monitor_process_simple(
        child: &mut Child,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(out_pipe) = stdout {
            let trk_out = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(out_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_out
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[stdout] {}", line));
                }
            });
        }

        if let Some(err_pipe) = stderr {
            let trk_err = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_err
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.cancel();
                return Ok(());
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        let mut trk = tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        trk.complete();
                        return Ok(());
                    } else {
                        let msg = format!("Process exited with status {:?}", status.code());
                        let mut trk = tracker
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        trk.fail(&msg);
                        return Err(msg);
                    }
                }
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    let msg = format!("Error checking process status: {}", e);
                    let mut trk = tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    trk.fail(&msg);
                    return Err(msg);
                }
            }
        }
    }

    // ─── PostgreSQL COPY Runner ─────────────────────────────────────────────

    fn run_postgres_copy(
        config: &ConnectionConfig,
        options: &CopyDatabaseOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let pg_dump_info = BinaryDetector::find_binary("pg_dump", options.custom_binary_path.as_deref())
            .ok_or_else(|| {
                let msg = "pg_dump binary not found in PATH or standard directories. Please install PostgreSQL client tools.".to_string();
                tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fail(&msg);
                msg
            })?;

        let psql_info = BinaryDetector::find_binary("psql", options.custom_binary_path.as_deref())
            .ok_or_else(|| {
                let msg = "psql binary not found in PATH or standard directories. Please install PostgreSQL client tools.".to_string();
                tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fail(&msg);
                msg
            })?;

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.start("Creating target database...");
            trk.append_log(format!(
                "Using pg_dump: {} and psql: {}",
                pg_dump_info.path.display(),
                psql_info.path.display()
            ));
        }

        // 1. Create target database using psql
        let mut create_cmd = Command::new(&psql_info.path);
        create_cmd.arg("-h").arg(&config.host);
        create_cmd.arg("-p").arg(&config.port);
        if !config.username.is_empty() {
            create_cmd.arg("-U").arg(&config.username);
        }
        let conn_db = if !config.database.is_empty() {
            &config.database
        } else {
            &options.source_database_name
        };
        create_cmd.arg("-d").arg(conn_db);
        if !config.password.is_empty() {
            create_cmd.env("PGPASSWORD", &config.password);
        }
        let sql_escaped = options.target_database_name.replace('"', "\"\"");

        // If requested, drop existing target database first
        if options.drop_target_if_exists {
            let mut drop_cmd = Command::new(&psql_info.path);
            drop_cmd.arg("-h").arg(&config.host);
            drop_cmd.arg("-p").arg(&config.port);
            if !config.username.is_empty() {
                drop_cmd.arg("-U").arg(&config.username);
            }
            drop_cmd.arg("-d").arg(conn_db);
            if !config.password.is_empty() {
                drop_cmd.env("PGPASSWORD", &config.password);
            }
            drop_cmd
                .arg("-c")
                .arg(format!("DROP DATABASE IF EXISTS \"{}\";", sql_escaped));
            let _ = drop_cmd.output();
        }

        create_cmd
            .arg("-c")
            .arg(format!("CREATE DATABASE \"{}\";", sql_escaped));

        create_cmd.stdout(Stdio::piped());
        create_cmd.stderr(Stdio::piped());

        let out = create_cmd
            .output()
            .map_err(|e| format!("Failed to spawn psql to create database: {}", e))?;

        if !out.status.success() {
            let err_msg = String::from_utf8_lossy(&out.stderr).to_string();
            let mut msg = format!(
                "Failed to create database '{}': {}",
                options.target_database_name,
                err_msg.trim()
            );
            if err_msg.contains("already exists") {
                msg.push_str(
                    ". Tip: enable 'Overwrite target database if it already exists' to replace it.",
                );
            }
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.fail(&msg);
            return Err(msg);
        }

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.append_log(format!(
                "Target database '{}' created successfully.",
                options.target_database_name
            ));
            trk.current_stage = "Streaming schema and data from source to target...".to_string();
        }

        // 2. Dump from source
        let mut dump_cmd = Command::new(&pg_dump_info.path);
        dump_cmd.arg("-h").arg(&config.host);
        dump_cmd.arg("-p").arg(&config.port);
        if !config.username.is_empty() {
            dump_cmd.arg("-U").arg(&config.username);
        }
        dump_cmd.arg("-d").arg(&options.source_database_name);
        dump_cmd.arg("-F").arg("p");
        dump_cmd.arg("--no-owner");
        if !config.password.is_empty() {
            dump_cmd.env("PGPASSWORD", &config.password);
        }
        dump_cmd.stdout(Stdio::piped());
        dump_cmd.stderr(Stdio::piped());

        let mut dump_child = dump_cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn pg_dump: {}", e))?;

        // 3. Restore into target
        let mut restore_cmd = Command::new(&psql_info.path);
        restore_cmd.arg("-h").arg(&config.host);
        restore_cmd.arg("-p").arg(&config.port);
        if !config.username.is_empty() {
            restore_cmd.arg("-U").arg(&config.username);
        }
        restore_cmd.arg("-d").arg(&options.target_database_name);
        restore_cmd.arg("-v").arg("ON_ERROR_STOP=1");
        if !config.password.is_empty() {
            restore_cmd.env("PGPASSWORD", &config.password);
        }
        restore_cmd.stdin(Stdio::piped());
        restore_cmd.stdout(Stdio::piped());
        restore_cmd.stderr(Stdio::piped());

        let mut restore_child = restore_cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn psql for restore: {}", e))?;

        Self::pipe_dump_to_restore(&mut dump_child, &mut restore_child, tracker, cancel_token)
    }

    // ─── MySQL COPY Runner ──────────────────────────────────────────────────

    fn run_mysql_copy(
        config: &ConnectionConfig,
        options: &CopyDatabaseOptions,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mysqldump_info =
            BinaryDetector::find_binary("mysqldump", options.custom_binary_path.as_deref())
                .ok_or_else(|| {
                    let msg = "mysqldump binary not found in PATH or standard directories. Please install MySQL client tools.".to_string();
                    tracker.lock().unwrap_or_else(std::sync::PoisonError::into_inner).fail(&msg);
                    msg
                })?;

        let mysql_info =
            BinaryDetector::find_binary("mysql", options.custom_binary_path.as_deref())
                .ok_or_else(|| {
                    let msg = "mysql client binary not found in PATH or standard directories."
                        .to_string();
                    tracker
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .fail(&msg);
                    msg
                })?;

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.start("Creating target database...");
            trk.append_log(format!(
                "Using mysqldump: {} and mysql: {}",
                mysqldump_info.path.display(),
                mysql_info.path.display()
            ));
        }

        // 1. Create target database using mysql
        let sql_escaped = options.target_database_name.replace('`', "``");

        // If requested, drop existing target database first
        if options.drop_target_if_exists {
            let mut drop_cmd = Command::new(&mysql_info.path);
            drop_cmd.arg("-h").arg(&config.host);
            drop_cmd.arg("-P").arg(&config.port);
            if !config.username.is_empty() {
                drop_cmd.arg("-u").arg(&config.username);
            }
            if !config.password.is_empty() {
                drop_cmd.env("MYSQL_PWD", &config.password);
            }
            drop_cmd
                .arg("-e")
                .arg(format!("DROP DATABASE IF EXISTS `{}`;", sql_escaped));
            let _ = drop_cmd.output();
        }

        let mut create_cmd = Command::new(&mysql_info.path);
        create_cmd.arg("-h").arg(&config.host);
        create_cmd.arg("-P").arg(&config.port);
        if !config.username.is_empty() {
            create_cmd.arg("-u").arg(&config.username);
        }
        if !config.password.is_empty() {
            create_cmd.env("MYSQL_PWD", &config.password);
        }
        create_cmd
            .arg("-e")
            .arg(format!("CREATE DATABASE `{}`;", sql_escaped));

        create_cmd.stdout(Stdio::piped());
        create_cmd.stderr(Stdio::piped());

        let out = create_cmd
            .output()
            .map_err(|e| format!("Failed to spawn mysql to create database: {}", e))?;

        if !out.status.success() {
            let err_msg = String::from_utf8_lossy(&out.stderr).to_string();
            let mut msg = format!(
                "Failed to create database '{}': {}",
                options.target_database_name,
                err_msg.trim()
            );
            if err_msg.contains("database exists") || err_msg.contains("1007") {
                msg.push_str(
                    ". Tip: enable 'Overwrite target database if it already exists' to replace it.",
                );
            }
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.fail(&msg);
            return Err(msg);
        }

        {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.append_log(format!(
                "Target database '{}' created successfully.",
                options.target_database_name
            ));
            trk.current_stage = "Streaming schema and data from source to target...".to_string();
        }

        // 2. Dump from source
        let caps =
            MysqldumpCapabilities::detect(&mysqldump_info.path, mysqldump_info.version.as_deref());

        let mut dump_cmd = Command::new(&mysqldump_info.path);
        dump_cmd.arg("-h").arg(&config.host);
        dump_cmd.arg("-P").arg(&config.port);
        if !config.username.is_empty() {
            dump_cmd.arg("-u").arg(&config.username);
        }
        if !config.password.is_empty() {
            dump_cmd.env("MYSQL_PWD", &config.password);
        }

        // Consistent non-locking snapshot and streaming
        dump_cmd.arg("--single-transaction");
        dump_cmd.arg("--quick");

        // Prevent GTID_PURGED from breaking restore on running MySQL instances (ERROR 3546)
        if caps.supports_gtid_purged {
            dump_cmd.arg("--set-gtid-purged=OFF");
        }
        // Avoid requiring PROCESS privilege for tablespaces in MySQL 8+
        if caps.supports_no_tablespaces {
            dump_cmd.arg("--no-tablespaces");
        }
        if options.include_routines {
            dump_cmd.arg("--routines");
        }

        dump_cmd.arg(&options.source_database_name);
        dump_cmd.stdout(Stdio::piped());
        dump_cmd.stderr(Stdio::piped());

        let mut dump_child = dump_cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn mysqldump: {}", e))?;

        // 3. Restore into target
        let mut restore_cmd = Command::new(&mysql_info.path);
        restore_cmd.arg("-h").arg(&config.host);
        restore_cmd.arg("-P").arg(&config.port);
        if !config.username.is_empty() {
            restore_cmd.arg("-u").arg(&config.username);
        }
        if !config.password.is_empty() {
            restore_cmd.env("MYSQL_PWD", &config.password);
        }
        restore_cmd.arg("-D").arg(&options.target_database_name);
        restore_cmd.stdin(Stdio::piped());
        restore_cmd.stdout(Stdio::piped());
        restore_cmd.stderr(Stdio::piped());

        let mut restore_child = restore_cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn mysql for restore: {}", e))?;

        Self::pipe_dump_to_restore(&mut dump_child, &mut restore_child, tracker, cancel_token)
    }

    /// Pipes stdout from dump_child directly into stdin of restore_child, tracking progress
    fn pipe_dump_to_restore(
        dump_child: &mut Child,
        restore_child: &mut Child,
        tracker: Arc<Mutex<ProgressTracker>>,
        cancel_token: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let mut dump_stdout = dump_child
            .stdout
            .take()
            .ok_or("Failed to capture dump stdout")?;
        let mut restore_stdin = restore_child
            .stdin
            .take()
            .ok_or("Failed to open restore stdin")?;
        let dump_stderr = dump_child.stderr.take();
        let restore_stderr = restore_child.stderr.take();

        if let Some(err_pipe) = dump_stderr {
            let trk_stderr = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_stderr
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[dump] {}", line));
                }
            });
        }

        if let Some(err_pipe) = restore_stderr {
            let trk_stderr = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_stderr
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    t.append_log(format!("[restore] {}", line));
                }
            });
        }

        let mut buffer = [0u8; 64 * 1024];

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = dump_child.kill();
                let _ = restore_child.kill();
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.cancel();
                return Ok(());
            }

            let read_bytes = dump_stdout
                .read(&mut buffer)
                .map_err(|e| format!("Error reading from dump process: {}", e))?;
            if read_bytes == 0 {
                break;
            }

            if let Err(e) = restore_stdin.write_all(&buffer[..read_bytes]) {
                let _ = dump_child.kill();

                // Give stderr reader thread a short window to capture error output from restore process
                std::thread::sleep(std::time::Duration::from_millis(150));
                let _ = restore_child.kill();

                // Look for actual error line from restore output
                let mut detailed_err: Option<String> = None;
                if let Ok(trk) = tracker.lock() {
                    for line in trk.log_lines.iter().rev() {
                        if line.contains("[restore] ERROR") || line.contains("[restore]") {
                            let clean = line.trim();
                            if !clean.is_empty() && clean != "[restore]" {
                                detailed_err = Some(clean.to_string());
                                break;
                            }
                        }
                    }
                }

                let msg = if let Some(restore_err) = detailed_err {
                    format!("Restore process failed: {}", restore_err)
                } else {
                    format!("Pipe to restore process broke: {}", e)
                };

                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.fail(&msg);
                return Err(msg);
            }

            {
                let mut trk = tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                trk.add_bytes(read_bytes as u64);
            }
        }

        // Close restore stdin to signal EOF to the restore process
        drop(restore_stdin);

        let dump_status = dump_child
            .wait()
            .map_err(|e| format!("Error waiting for dump process: {}", e))?;

        let restore_status = restore_child
            .wait()
            .map_err(|e| format!("Error waiting for restore process: {}", e))?;

        if dump_status.success() && restore_status.success() {
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.complete();
            Ok(())
        } else {
            let msg = format!(
                "Copy failed: dump exit status {:?}, restore exit status {:?}",
                dump_status.code(),
                restore_status.code()
            );
            let mut trk = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.fail(&msg);
            Err(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_char;

    #[test]
    fn test_backup_format_properties() {
        assert_eq!(BackupFormat::PlainSql.extension(), "sql");
        assert_eq!(BackupFormat::GzipSql.extension(), "sql.gz");
        assert_eq!(BackupFormat::PostgresCustom.extension(), "dump");
        assert_eq!(BackupFormat::PostgresTar.extension(), "tar");
        assert_eq!(BackupFormat::SqliteNative.extension(), "sqlite");

        assert!(BackupFormat::PostgresCustom.supported_for(&DatabaseType::PostgreSQL));
        assert!(!BackupFormat::PostgresCustom.supported_for(&DatabaseType::MySQL));
        assert!(BackupFormat::GzipSql.supported_for(&DatabaseType::MySQL));
        assert!(BackupFormat::SqliteNative.supported_for(&DatabaseType::SQLite));
    }

    #[test]
    fn test_progress_tracker_lifecycle() {
        let tracker = ProgressTracker::new(
            OperationType::Backup,
            "test_db".to_string(),
            PathBuf::from("/tmp/test.sql"),
        );
        let tracker_arc = Arc::new(Mutex::new(tracker));

        {
            let mut trk = tracker_arc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.start("Initiating dump...");
            trk.add_bytes(1024);
            trk.set_pages(10, 50);
            trk.append_log("Writing table schema...");
        }

        let snap = tracker_arc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot();
        assert_eq!(snap.status, OperationStatus::Running);
        assert_eq!(snap.bytes_processed, 1024);
        assert_eq!(snap.pages_copied, 10);
        assert_eq!(snap.total_pages, 50);
        assert!(snap.log_lines.len() >= 2);

        {
            let mut trk = tracker_arc
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            trk.complete();
        }

        let snap_final = tracker_arc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .snapshot();
        assert_eq!(snap_final.status, OperationStatus::Completed);
    }

    #[test]
    fn test_sqlite_backup_restore_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tabular_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        let src_db = temp_dir.join("source.db");
        let backup_raw = temp_dir.join("backup.sqlite");
        let backup_gz = temp_dir.join("backup.sqlite.gz");
        let restored_db = temp_dir.join("restored.db");

        // 1. Create source SQLite DB with test table & data using C API
        let src_c = CString::new(src_db.to_str().unwrap()).unwrap();
        unsafe {
            let mut db: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();
            let rc = libsqlite3_sys::sqlite3_open(src_c.as_ptr(), &mut db);
            assert_eq!(rc, libsqlite3_sys::SQLITE_OK);

            let sql = CString::new(
                "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT); \
                 INSERT INTO users (name) VALUES ('Alice'), ('Bob'), ('Charlie');",
            )
            .unwrap();
            let mut errmsg: *mut c_char = std::ptr::null_mut();
            let exec_rc = libsqlite3_sys::sqlite3_exec(
                db,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                &mut errmsg,
            );
            assert_eq!(exec_rc, libsqlite3_sys::SQLITE_OK);
            libsqlite3_sys::sqlite3_close(db);
        }

        // 2. Backup to uncompressed sqlite file
        let cancel_token = Arc::new(AtomicBool::new(false));
        let tracker = Arc::new(Mutex::new(ProgressTracker::new(
            OperationType::Backup,
            "main".to_string(),
            backup_raw.clone(),
        )));

        let res = SqliteBackupEngine::backup(
            &src_db,
            &backup_raw,
            false,
            tracker.clone(),
            cancel_token.clone(),
        );
        assert!(res.is_ok(), "Raw SQLite backup failed: {:?}", res.err());
        assert!(backup_raw.is_file());

        // 3. Backup with gzip compression
        let tracker_gz = Arc::new(Mutex::new(ProgressTracker::new(
            OperationType::Backup,
            "main".to_string(),
            backup_gz.clone(),
        )));
        let res_gz = SqliteBackupEngine::backup(
            &src_db,
            &backup_gz,
            true,
            tracker_gz.clone(),
            cancel_token.clone(),
        );
        assert!(
            res_gz.is_ok(),
            "Gzip SQLite backup failed: {:?}",
            res_gz.err()
        );
        assert!(backup_gz.is_file());

        // 4. Restore from gzip backup to new database
        let tracker_restore = Arc::new(Mutex::new(ProgressTracker::new(
            OperationType::Restore,
            "main".to_string(),
            backup_gz.clone(),
        )));
        let res_restore = SqliteBackupEngine::restore(
            &backup_gz,
            &restored_db,
            tracker_restore.clone(),
            cancel_token.clone(),
        );
        assert!(
            res_restore.is_ok(),
            "SQLite restore failed: {:?}",
            res_restore.err()
        );
        assert!(restored_db.is_file());

        // 5. Verify restored data
        let restore_c = CString::new(restored_db.to_str().unwrap()).unwrap();
        unsafe {
            let mut db: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();
            let rc = libsqlite3_sys::sqlite3_open(restore_c.as_ptr(), &mut db);
            assert_eq!(rc, libsqlite3_sys::SQLITE_OK);

            let sql = CString::new("SELECT COUNT(*) FROM users;").unwrap();
            let mut stmt: *mut libsqlite3_sys::sqlite3_stmt = std::ptr::null_mut();
            let prep_rc = libsqlite3_sys::sqlite3_prepare_v2(
                db,
                sql.as_ptr(),
                -1,
                &mut stmt,
                std::ptr::null_mut(),
            );
            assert_eq!(prep_rc, libsqlite3_sys::SQLITE_OK);

            let step_rc = libsqlite3_sys::sqlite3_step(stmt);
            assert_eq!(step_rc, libsqlite3_sys::SQLITE_ROW);
            let count = libsqlite3_sys::sqlite3_column_int(stmt, 0);
            assert_eq!(count, 3);

            libsqlite3_sys::sqlite3_finalize(stmt);
            libsqlite3_sys::sqlite3_close(db);
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_copy_database_options_default() {
        let opts = CopyDatabaseOptions::default();
        assert!(opts.source_database_name.is_empty());
        assert!(opts.target_database_name.is_empty());
        assert!(opts.target_file.is_none());
        assert!(opts.custom_binary_path.is_none());
        assert!(!opts.drop_target_if_exists);
        assert!(!opts.include_routines);
    }

    #[test]
    fn test_sqlite_copy_database_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!(
            "tabular_test_copy_{}",
            Instant::now().elapsed().as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        let source_db = temp_dir.join("source.sqlite");
        let target_db = temp_dir.join("target_copy.sqlite");

        // 1. Populate source database
        let source_c = CString::new(source_db.to_str().unwrap()).unwrap();
        unsafe {
            let mut db: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();
            let rc = libsqlite3_sys::sqlite3_open(source_c.as_ptr(), &mut db);
            assert_eq!(rc, libsqlite3_sys::SQLITE_OK);

            let sql = CString::new(
                "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT); \
                 INSERT INTO items (name) VALUES ('Item A'), ('Item B'), ('Item C'), ('Item D');",
            )
            .unwrap();
            let exec_rc = libsqlite3_sys::sqlite3_exec(
                db,
                sql.as_ptr(),
                None,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert_eq!(exec_rc, libsqlite3_sys::SQLITE_OK);
            libsqlite3_sys::sqlite3_close(db);
        }

        // 2. Perform copy database operation
        let tracker = Arc::new(Mutex::new(ProgressTracker::new(
            OperationType::CopyDatabase,
            "source → target".to_string(),
            target_db.clone(),
        )));
        let cancel_token = Arc::new(AtomicBool::new(false));

        let res = SqliteBackupEngine::backup(
            &source_db,
            &target_db,
            false,
            tracker.clone(),
            cancel_token,
        );
        assert!(res.is_ok(), "SQLite copy failed: {:?}", res.err());
        assert!(
            target_db.is_file(),
            "Target copied database file does not exist"
        );

        // 3. Verify copied database contents
        let target_c = CString::new(target_db.to_str().unwrap()).unwrap();
        unsafe {
            let mut db: *mut libsqlite3_sys::sqlite3 = std::ptr::null_mut();
            let rc = libsqlite3_sys::sqlite3_open(target_c.as_ptr(), &mut db);
            assert_eq!(rc, libsqlite3_sys::SQLITE_OK);

            let sql = CString::new("SELECT COUNT(*) FROM items;").unwrap();
            let mut stmt: *mut libsqlite3_sys::sqlite3_stmt = std::ptr::null_mut();
            let prep_rc = libsqlite3_sys::sqlite3_prepare_v2(
                db,
                sql.as_ptr(),
                -1,
                &mut stmt,
                std::ptr::null_mut(),
            );
            assert_eq!(prep_rc, libsqlite3_sys::SQLITE_OK);

            let step_rc = libsqlite3_sys::sqlite3_step(stmt);
            assert_eq!(step_rc, libsqlite3_sys::SQLITE_ROW);
            let count = libsqlite3_sys::sqlite3_column_int(stmt, 0);
            assert_eq!(count, 4);

            libsqlite3_sys::sqlite3_finalize(stmt);
            libsqlite3_sys::sqlite3_close(db);
        }

        // Verify tracker state
        let snap = tracker.lock().unwrap().snapshot();
        assert_eq!(snap.operation_type, OperationType::CopyDatabase);
        assert_eq!(snap.status, OperationStatus::Completed);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_progress_tracker_freezes_elapsed_and_speed_on_complete() {
        let mut tracker = ProgressTracker::new(
            OperationType::Backup,
            "test_db".to_string(),
            PathBuf::from("/tmp/test.sql"),
        );
        tracker.start("Processing");
        tracker.add_bytes(100_000);
        std::thread::sleep(std::time::Duration::from_millis(60));
        tracker.complete();

        let snap1 = tracker.snapshot();
        assert_eq!(snap1.status, OperationStatus::Completed);
        assert!(snap1.elapsed_secs > 0.0);
        assert!(snap1.bytes_per_sec > 0.0);

        std::thread::sleep(std::time::Duration::from_millis(50));
        let snap2 = tracker.snapshot();
        assert_eq!(snap1.elapsed_secs, snap2.elapsed_secs);
        assert_eq!(snap1.bytes_per_sec, snap2.bytes_per_sec);
    }

    fn mongo_config() -> ConnectionConfig {
        ConnectionConfig {
            connection_type: DatabaseType::MongoDB,
            host: "db.local".to_string(),
            port: "27017".to_string(),
            username: "root".to_string(),
            password: "p\"w".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_formats_per_engine_and_extension_stripping() {
        assert_eq!(
            BackupFormat::default_for(&DatabaseType::MongoDB),
            BackupFormat::MongoArchiveGzip
        );
        assert_eq!(
            BackupFormat::default_for(&DatabaseType::MsSQL),
            BackupFormat::SqlServerBacpac
        );
        assert!(BackupFormat::MongoArchive.supported_for(&DatabaseType::MongoDB));
        assert!(!BackupFormat::GzipSql.supported_for(&DatabaseType::MongoDB));
        assert!(BackupFormat::SqlServerDacpac.supported_for(&DatabaseType::MsSQL));
        assert!(!BackupFormat::SqlServerBacpac.supported_for(&DatabaseType::PostgreSQL));

        assert_eq!(BackupFormat::strip_extension("shop_2026.sql.gz"), "shop_2026");
        assert_eq!(BackupFormat::strip_extension("shop.archive.gz"), "shop");
        assert_eq!(BackupFormat::strip_extension("shop.v2.bacpac"), "shop.v2");
        assert_eq!(BackupFormat::strip_extension("shop.unknown"), "shop");
        assert_eq!(BackupFormat::strip_extension("shop"), "shop");
    }

    #[test]
    fn test_mongodump_args_keep_password_off_command_line() {
        let options = BackupOptions {
            database_name: "shop".to_string(),
            target_file: PathBuf::from("/tmp/shop.archive.gz"),
            format: BackupFormat::MongoArchiveGzip,
            excluded_tables: vec!["logs".to_string()],
            ..Default::default()
        };
        let args = mongodump_args(&mongo_config(), &options, Some(Path::new("/tmp/cfg.yaml")));
        assert_eq!(
            args,
            vec![
                "--host",
                "db.local",
                "--port",
                "27017",
                "--username",
                "root",
                "--authenticationDatabase",
                "admin",
                "--config",
                "/tmp/cfg.yaml",
                "--db",
                "shop",
                "--archive=/tmp/shop.archive.gz",
                "--gzip",
                "--excludeCollection",
                "logs",
            ]
        );
        assert!(!args.iter().any(|a| a.contains("p\"w")));

        let single = BackupOptions {
            selected_tables: vec!["orders".to_string()],
            format: BackupFormat::MongoArchive,
            ..options
        };
        let args = mongodump_args(&mongo_config(), &single, None);
        assert!(args.windows(2).any(|w| w == ["--collection", "orders"]));
        assert!(!args.contains(&"--gzip".to_string()));
    }

    #[test]
    fn test_mongo_password_file_is_private_and_removed() {
        let file = MongoPasswordFile::create("p\"w\\x").unwrap().unwrap();
        let path = file.path.clone();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "password: \"p\\\"w\\\\x\"\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        drop(file);
        assert!(!path.exists());
        assert!(MongoPasswordFile::create("").unwrap().is_none());
    }

    #[test]
    fn test_mongorestore_args_remap_namespace() {
        let options = RestoreOptions {
            target_database_name: "shop_copy".to_string(),
            source_file: PathBuf::from("/tmp/shop.archive.gz"),
            clean_before_restore: true,
            ..Default::default()
        };
        let args = mongorestore_args(&mongo_config(), &options, None);
        assert!(args.contains(&"--archive=/tmp/shop.archive.gz".to_string()));
        assert!(args.contains(&"--gzip".to_string()));
        assert!(args.contains(&"--drop".to_string()));
        assert!(args.contains(&"--stopOnError".to_string()));
        assert!(args.contains(&"--nsTo=shop_copy.$coll$".to_string()));
    }

    #[test]
    fn test_sqlpackage_args() {
        let config = ConnectionConfig {
            connection_type: DatabaseType::MsSQL,
            host: "sql.local".to_string(),
            port: "1433".to_string(),
            username: "sa".to_string(),
            password: "secret".to_string(),
            ..Default::default()
        };
        let backup = BackupOptions {
            database_name: "shop".to_string(),
            target_file: PathBuf::from("/tmp/shop.bacpac"),
            format: BackupFormat::SqlServerBacpac,
            ..Default::default()
        };
        assert_eq!(
            sqlpackage_export_args(&config, &backup),
            vec![
                "/Action:Export",
                "/TargetFile:/tmp/shop.bacpac",
                "/SourceServerName:sql.local,1433",
                "/SourceDatabaseName:shop",
                "/SourceUser:sa",
                "/SourcePassword:secret",
                "/SourceEncryptConnection:False",
                "/SourceTrustServerCertificate:True",
            ]
        );
        let dacpac = BackupOptions {
            format: BackupFormat::SqlServerDacpac,
            ..backup
        };
        assert_eq!(sqlpackage_export_args(&config, &dacpac)[0], "/Action:Extract");

        let verified = ConnectionConfig {
            ssl_enabled: true,
            ssl_verify_server: true,
            username: String::new(),
            ..config
        };
        let restore = RestoreOptions {
            target_database_name: "shop2".to_string(),
            source_file: PathBuf::from("/tmp/shop.dacpac"),
            ..Default::default()
        };
        let args = sqlpackage_import_args(&verified, &restore);
        assert_eq!(args[0], "/Action:Publish");
        assert!(args.contains(&"/TargetDatabaseName:shop2".to_string()));
        assert!(args.contains(&"/TargetEncryptConnection:True".to_string()));
        assert!(args.contains(&"/TargetTrustServerCertificate:False".to_string()));
        // Tanpa username: autentikasi terintegrasi, tidak ada argumen password.
        assert!(!args.iter().any(|a| a.contains("Password")));
    }
}
