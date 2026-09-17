//! Logging aplikasi ke file dan pencatatan crash.
//!
//! Sebelumnya crate `log` dikompilasi dengan `max_level_off`, sehingga semua
//! `log::*` hilang total dan crash di mesin user tidak meninggalkan jejak.
//! Modul ini:
//! - menulis log ke `<data_dir>/logs/tabular.log` (sekaligus ke stderr), dengan
//!   rotasi sederhana saat startup;
//! - memasang panic hook yang menyimpan `crash-<waktu>.log` berisi pesan,
//!   lokasi, dan backtrace;
//! - menyediakan ringkasan diagnostik untuk laporan bug.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Ukuran maksimum `tabular.log` sebelum dirotasi ke `tabular.log.1`.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// Jumlah crash report yang disimpan; yang lebih lama dihapus.
const MAX_CRASH_REPORTS: usize = 10;

static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

/// Folder tempat log dan crash report disimpan.
pub fn logs_dir() -> PathBuf {
    crate::config::get_data_dir().join("logs")
}

/// Path file log aktif.
pub fn log_file_path() -> PathBuf {
    logs_dir().join("tabular.log")
}

/// Writer yang meneruskan setiap baris log ke stderr dan ke file log.
struct TeeWriter;

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Some(lock) = LOG_FILE.get()
            && let Ok(mut guard) = lock.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(lock) = LOG_FILE.get()
            && let Ok(mut guard) = lock.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.flush();
        }
        std::io::stderr().flush()
    }
}

/// Buka file log (append) setelah merotasi file lama yang terlalu besar.
fn open_log_file() -> Option<std::fs::File> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = log_file_path();
    if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(&path, dir.join("tabular.log.1"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// Inisialisasi logger global. Aman dipanggil lebih dari sekali; hanya
/// panggilan pertama yang berpengaruh. Harus dipanggil setelah
/// `config::init_data_dir()` supaya folder log berada di data dir yang benar.
pub fn init() {
    let _ = LOG_FILE.set(Mutex::new(open_log_file()));

    let result = env_logger::Builder::from_default_env()
        .filter_module("tabular", log::LevelFilter::Debug)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .format_timestamp_millis()
        .target(env_logger::Target::Pipe(Box::new(TeeWriter)))
        .is_test(false)
        .try_init();

    if result.is_ok() {
        // Filter per modul mengizinkan Debug, tetapi level global default
        // Info agar log tidak berisik; "Enable Debug Logging" menaikkannya.
        let verbose_from_env = std::env::var_os("RUST_LOG").is_some();
        if !verbose_from_env {
            log::set_max_level(log::LevelFilter::Info);
        }
    }
    log::info!(
        "Tabular {} starting on {} {} (data dir: {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::config::get_data_dir().display()
    );
}

/// Aktifkan atau matikan log level debug saat runtime (preferensi user).
pub fn set_verbose(enabled: bool) {
    log::set_max_level(if enabled {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });
}

/// Pasang panic hook yang menyimpan crash report lalu meneruskan ke hook
/// bawaan (yang mencetak pesan ke stderr).
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current();
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let report = format!(
            "Tabular crash report\n\
             ====================\n\
             Time     : {}\n\
             Version  : {}\n\
             Platform : {} {}\n\
             Thread   : {}\n\
             Location : {}\n\
             Message  : {}\n\n\
             Backtrace:\n{}\n",
            chrono::Local::now().to_rfc3339(),
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            thread.name().unwrap_or("<unnamed>"),
            location,
            payload,
            backtrace
        );
        log::error!("PANIC at {}: {}", location, payload);
        if let Some(path) = write_crash_report(&report) {
            eprintln!("Tabular crash report saved to {}", path.display());
        }
        default_hook(info);
    }));
}

fn write_crash_report(report: &str) -> Option<PathBuf> {
    let dir = logs_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!("crash-{}.log", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let path = dir.join(name);
    std::fs::write(&path, report).ok()?;
    prune_crash_reports();
    Some(path)
}

/// Daftar crash report, terbaru lebih dulu.
pub fn crash_reports() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(logs_dir())
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("crash-") && n.ends_with(".log"))
                })
                .collect()
        })
        .unwrap_or_default();
    // Nama file memuat timestamp yang bisa diurutkan secara leksikal.
    files.sort();
    files.reverse();
    files
}

fn prune_crash_reports() {
    for old in crash_reports().into_iter().skip(MAX_CRASH_REPORTS) {
        let _ = std::fs::remove_file(old);
    }
}

/// Crash report yang belum pernah diberitahukan ke user. Penanda disimpan di
/// `logs/.last_seen_crash` agar notifikasi hanya muncul sekali per crash.
pub fn take_unseen_crash_report() -> Option<PathBuf> {
    let latest = crash_reports().into_iter().next()?;
    let marker = logs_dir().join(".last_seen_crash");
    let latest_name = latest.file_name()?.to_string_lossy().to_string();
    let seen = std::fs::read_to_string(&marker).unwrap_or_default();
    if seen.trim() == latest_name {
        return None;
    }
    let _ = std::fs::write(&marker, &latest_name);
    Some(latest)
}

/// Ringkasan lingkungan untuk ditempel di laporan bug. Tidak memuat data
/// koneksi, query, atau kredensial.
pub fn diagnostics_report() -> String {
    let crashes = crash_reports();
    let log_tail = std::fs::read_to_string(log_file_path())
        .map(|content| {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(40);
            lines[start..].join("\n")
        })
        .unwrap_or_else(|_| "<log file not available>".to_string());
    format!(
        "Tabular diagnostics\n\
         Version  : {}\n\
         Platform : {} {}\n\
         Data dir : {}\n\
         Log file : {}\n\
         Crash reports: {}{}\n\n\
         Last log lines:\n{}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crate::config::get_data_dir().display(),
        log_file_path().display(),
        crashes.len(),
        crashes
            .first()
            .map(|p| format!(" (latest: {})", p.display()))
            .unwrap_or_default(),
        log_tail
    )
}
