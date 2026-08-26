use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
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
}

impl BackupFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            BackupFormat::PlainSql => "sql",
            BackupFormat::GzipSql => "sql.gz",
            BackupFormat::PostgresCustom => "dump",
            BackupFormat::PostgresTar => "tar",
            BackupFormat::PostgresDirectory => "dir",
            BackupFormat::SqliteNative => "sqlite",
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

// ─── Progress & State Tracking ─────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    Backup,
    Restore,
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
            current_stage: "Ready".to_string(),
            log_lines: VecDeque::with_capacity(100),
            max_log_lines: 200,
        }
    }

    pub fn start(&mut self, stage: impl Into<String>) {
        self.status = OperationStatus::Running;
        self.start_time = Some(Instant::now());
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
        self.status = OperationStatus::Completed;
        self.current_stage = "Completed successfully".to_string();
        let elapsed = self.start_time.map_or(0.0, |t| t.elapsed().as_secs_f64());
        self.append_log(format!(
            "[{}] {:?} finished in {:.2}s ({} bytes processed)",
            chrono::Local::now().format("%H:%M:%S"),
            self.operation_type,
            elapsed,
            self.bytes_processed
        ));
    }

    pub fn fail(&mut self, err: impl Into<String>) {
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
        self.status = OperationStatus::Cancelled;
        self.current_stage = "Cancelled by user".to_string();
        self.append_log(format!(
            "[{}] ⚠️ Operation was cancelled by user.",
            chrono::Local::now().format("%H:%M:%S")
        ));
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        let elapsed = self.start_time.map_or(0.0, |t| t.elapsed().as_secs_f64());
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
    pub fn find_binary(binary_name: &'static str, custom_path: Option<&Path>) -> Option<NativeBinaryInfo> {
        // 1. Check custom path override first
        if let Some(cp) = custom_path {
            if cp.is_file() {
                let ver = Self::query_version(cp);
                return Some(NativeBinaryInfo {
                    name: binary_name,
                    path: cp.to_path_buf(),
                    version: ver,
                });
            }
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

        if cfg!(target_os = "macos") {
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));

            if binary_name.starts_with("pg_") || binary_name == "psql" {
                for v in [17, 16, 15, 14, 13, 12] {
                    dirs.push(PathBuf::from(format!("/opt/homebrew/opt/postgresql@{}/bin", v)));
                    dirs.push(PathBuf::from(format!("/usr/local/opt/postgresql@{}/bin", v)));
                }
                dirs.push(PathBuf::from("/opt/homebrew/opt/libpq/bin"));
                dirs.push(PathBuf::from("/usr/local/opt/libpq/bin"));
                dirs.push(PathBuf::from("/Applications/Postgres.app/Contents/Versions/latest/bin"));
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
                    dirs.push(PathBuf::from(format!(r"C:\Program Files\PostgreSQL\{}\bin", v)));
                }
            } else if binary_name.starts_with("mysql") {
                for v in ["8.4", "8.0", "5.7"] {
                    dirs.push(PathBuf::from(format!(r"C:\Program Files\MySQL\MySQL Server {}\bin", v)));
                }
                for v in ["11.4", "10.11", "10.6"] {
                    dirs.push(PathBuf::from(format!(r"C:\Program Files\MariaDB {}\bin", v)));
                }
            }
        }

        dirs
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
                return Err(format!("Failed to open source SQLite database: {}", err_msg));
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
                return Err(format!("Failed to create destination SQLite file: {}", err_msg));
            }

            let main_db = b"main\0".as_ptr() as *const c_char;
            let p_backup = libsqlite3_sys::sqlite3_backup_init(p_dest, main_db, p_src, main_db);

            if p_backup.is_null() {
                let err_msg = Self::get_sqlite_errmsg(p_dest);
                libsqlite3_sys::sqlite3_close(p_dest);
                libsqlite3_sys::sqlite3_close(p_src);
                return Err(format!("Failed to initialize SQLite backup handle: {}", err_msg));
            }

            {
                let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
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
                let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
                    trk.bytes_processed = total_compressed_in;
                }
            }

            encoder
                .finish()
                .map_err(|e| format!("Failed to finalize gzip stream: {}", e))?;
            let _ = std::fs::remove_file(&temp_dest);
        }

        {
            let mut trk = tracker.lock().unwrap();
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
            .map_or(false, |ext| ext == "gz");

        let raw_source_path = if is_gzipped {
            let temp_uncompressed = dest_db_path.with_extension("tmp_restore_sqlite");
            {
                let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
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
                    let mut trk = tracker.lock().unwrap();
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
                DatabaseType::PostgreSQL => Self::run_postgres_dump(
                    &config_clone,
                    &options,
                    tracker.clone(),
                    cancel_token,
                ),
                DatabaseType::MySQL => Self::run_mysql_dump(
                    &config_clone,
                    &options,
                    tracker.clone(),
                    cancel_token,
                ),
                _ => {
                    let err = format!(
                        "Backup is not supported for {:?}",
                        config_clone.connection_type
                    );
                    let mut trk = tracker.lock().unwrap();
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
                DatabaseType::MySQL => Self::run_mysql_restore(
                    &config_clone,
                    &options,
                    tracker.clone(),
                    cancel_token,
                ),
                _ => {
                    let err = format!(
                        "Restore is not supported for {:?}",
                        config_clone.connection_type
                    );
                    let mut trk = tracker.lock().unwrap();
                    trk.fail(&err);
                    Err(err)
                }
            };

            if let Err(e) = res {
                error!("Restore job failed: {}", e);
            }
        });
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
                tracker.lock().unwrap().fail(&msg);
                msg
            })?;

        {
            let mut trk = tracker.lock().unwrap();
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
            BackupFormat::SqliteNative => {}
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
        let is_custom_format = options.source_file.extension().map_or(false, |ext| {
            ext == "dump" || ext == "pgdump" || ext == "tar" || ext == "dir"
        });

        if is_custom_format {
            let binary_info =
                BinaryDetector::find_binary("pg_restore", options.custom_binary_path.as_deref())
                    .ok_or_else(|| {
                        let msg = "pg_restore binary not found in PATH or standard directories.".to_string();
                        tracker.lock().unwrap().fail(&msg);
                        msg
                    })?;

            {
                let mut trk = tracker.lock().unwrap();
                trk.start("Spawning pg_restore process...");
                trk.append_log(format!(
                    "Using pg_restore: {}",
                    binary_info.path.display()
                ));
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
                        let msg = "psql binary not found in PATH or standard directories.".to_string();
                        tracker.lock().unwrap().fail(&msg);
                        msg
                    })?;

            {
                let mut trk = tracker.lock().unwrap();
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

            Self::feed_file_to_stdin(
                &mut child,
                &options.source_file,
                tracker,
                cancel_token,
            )?;
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
                tracker.lock().unwrap().fail(&msg);
                msg
            })?;

        {
            let mut trk = tracker.lock().unwrap();
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

        if options.single_transaction {
            cmd.arg("--single-transaction");
        }
        if options.include_triggers_routines {
            cmd.arg("--routines").arg("--triggers");
        }
        if options.clean_before_recreate {
            cmd.arg("--add-drop-table");
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
                    let msg = "mysql client binary not found in PATH or standard directories.".to_string();
                    tracker.lock().unwrap().fail(&msg);
                    msg
                })?;

        {
            let mut trk = tracker.lock().unwrap();
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

        Self::feed_file_to_stdin(
            &mut child,
            &options.source_file,
            tracker,
            cancel_token,
        )?;

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
                    let mut t = trk_stderr.lock().unwrap();
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
                let mut trk = tracker.lock().unwrap();
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
                let mut trk = tracker.lock().unwrap();
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
            let mut trk = tracker.lock().unwrap();
            trk.complete();
            Ok(())
        } else {
            let msg = format!("Process exited with status code {:?}", status.code());
            let mut trk = tracker.lock().unwrap();
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
                    let mut t = trk_stderr.lock().unwrap();
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
                let mut trk = tracker.lock().unwrap();
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
                let mut trk = tracker.lock().unwrap();
                trk.add_bytes(read_bytes as u64);
            }
        }

        out_file.flush().map_err(|e| format!("Flush error: {}", e))?;

        let status = child
            .wait()
            .map_err(|e| format!("Error waiting for process: {}", e))?;

        if status.success() {
            let mut trk = tracker.lock().unwrap();
            trk.complete();
            Ok(())
        } else {
            let msg = format!("Process exited with status code {:?}", status.code());
            let mut trk = tracker.lock().unwrap();
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
                    let mut t = trk_stderr.lock().unwrap();
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        let is_gzipped = source_file.extension().map_or(false, |ext| ext == "gz");
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
                let mut trk = tracker.lock().unwrap();
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
                let mut trk = tracker.lock().unwrap();
                trk.add_bytes(read_bytes as u64);
            }
        }

        drop(stdin); // Close stdin to signal EOF to database process

        let status = child
            .wait()
            .map_err(|e| format!("Error waiting for restore process: {}", e))?;

        if status.success() {
            let mut trk = tracker.lock().unwrap();
            trk.complete();
            Ok(())
        } else {
            let msg = format!("Restore process exited with code {:?}", status.code());
            let mut trk = tracker.lock().unwrap();
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
                    let mut t = trk_stderr.lock().unwrap();
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = std::fs::remove_file(target_file);
                let mut trk = tracker.lock().unwrap();
                trk.cancel();
                return Ok(());
            }

            if let Ok(metadata) = std::fs::metadata(target_file) {
                let mut trk = tracker.lock().unwrap();
                trk.bytes_processed = metadata.len();
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    if let Ok(metadata) = std::fs::metadata(target_file) {
                        let mut trk = tracker.lock().unwrap();
                        trk.bytes_processed = metadata.len();
                    }

                    if status.success() {
                        let mut trk = tracker.lock().unwrap();
                        trk.complete();
                        return Ok(());
                    } else {
                        let msg = format!("Process failed with exit code {:?}", status.code());
                        let mut trk = tracker.lock().unwrap();
                        trk.fail(&msg);
                        return Err(msg);
                    }
                }
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    let msg = format!("Error checking process status: {}", e);
                    let mut trk = tracker.lock().unwrap();
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
                    let mut t = trk_out.lock().unwrap();
                    t.append_log(format!("[stdout] {}", line));
                }
            });
        }

        if let Some(err_pipe) = stderr {
            let trk_err = tracker.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(err_pipe);
                for line in reader.lines().map_while(Result::ok) {
                    let mut t = trk_err.lock().unwrap();
                    t.append_log(format!("[stderr] {}", line));
                }
            });
        }

        loop {
            if cancel_token.load(Ordering::Relaxed) {
                let _ = child.kill();
                let mut trk = tracker.lock().unwrap();
                trk.cancel();
                return Ok(());
            }

            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        let mut trk = tracker.lock().unwrap();
                        trk.complete();
                        return Ok(());
                    } else {
                        let msg = format!("Process exited with status {:?}", status.code());
                        let mut trk = tracker.lock().unwrap();
                        trk.fail(&msg);
                        return Err(msg);
                    }
                }
                Ok(None) => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    let msg = format!("Error checking process status: {}", e);
                    let mut trk = tracker.lock().unwrap();
                    trk.fail(&msg);
                    return Err(msg);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let mut trk = tracker_arc.lock().unwrap();
            trk.start("Initiating dump...");
            trk.add_bytes(1024);
            trk.set_pages(10, 50);
            trk.append_log("Writing table schema...");
        }

        let snap = tracker_arc.lock().unwrap().snapshot();
        assert_eq!(snap.status, OperationStatus::Running);
        assert_eq!(snap.bytes_processed, 1024);
        assert_eq!(snap.pages_copied, 10);
        assert_eq!(snap.total_pages, 50);
        assert!(snap.log_lines.len() >= 2);

        {
            let mut trk = tracker_arc.lock().unwrap();
            trk.complete();
        }

        let snap_final = tracker_arc.lock().unwrap().snapshot();
        assert_eq!(snap_final.status, OperationStatus::Completed);
    }

    #[test]
    fn test_sqlite_backup_restore_roundtrip() {
        let temp_dir = std::env::temp_dir().join(format!("tabular_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
                 INSERT INTO users (name) VALUES ('Alice'), ('Bob'), ('Charlie');"
            ).unwrap();
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
        assert!(res_gz.is_ok(), "Gzip SQLite backup failed: {:?}", res_gz.err());
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
        assert!(res_restore.is_ok(), "SQLite restore failed: {:?}", res_restore.err());
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
}

