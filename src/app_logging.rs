//! Logging aplikasi ke file dan pencatatan crash.
//!
//! Sebelumnya crate `log` dikompilasi dengan `max_level_off`, sehingga semua
//! `log::*` hilang total dan crash di mesin user tidak meninggalkan jejak.
//! Modul ini:
//! - menulis log ke `<data_dir>/logs/tabular.log` (sekaligus ke stderr), dengan
//!   rotasi berbasis ukuran saat startup dan selama sesi berjalan;
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

/// Ukuran file diperiksa setiap sekian penulisan, bukan tiap baris.
const ROTATE_CHECK_EVERY: u32 = 256;

static LOG_FILE: OnceLock<Mutex<LogSink>> = OnceLock::new();

/// File log aktif beserta state rotasinya. Selalu diakses di bawah mutex
/// `LOG_FILE`, jadi rotasi tidak pernah balapan dengan penulisan.
struct LogSink {
    file: Option<std::fs::File>,
    path: PathBuf,
    max_bytes: u64,
    writes_since_check: u32,
}

impl LogSink {
    fn open(path: PathBuf, max_bytes: u64) -> Self {
        let mut sink = Self {
            file: None,
            path,
            max_bytes,
            writes_since_check: 0,
        };
        sink.rotate_if_needed();
        if sink.file.is_none() {
            sink.file = sink.open_append();
        }
        sink
    }

    fn open_append(&self) -> Option<std::fs::File> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok()
    }

    fn rotated_path(&self) -> PathBuf {
        let mut name = self
            .path
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        name.push(".1");
        self.path.with_file_name(name)
    }

    /// Rotasi ke `<nama>.1` bila file aktif melewati batas, lalu buka ulang.
    /// Ukuran dibaca dari path (bukan handle) supaya proses lain yang menulis
    /// file yang sama (GUI vs `tabular mcp`) ikut terhitung dan ikut pindah ke
    /// file baru setelah dirotasi proses lain.
    fn rotate_if_needed(&mut self) {
        let too_big = std::fs::metadata(&self.path)
            .map(|m| m.len() > self.max_bytes)
            .unwrap_or(false);
        if too_big {
            // Tutup handle dulu: di Windows file terbuka tidak bisa di-rename.
            self.file = None;
            let _ = std::fs::rename(&self.path, self.rotated_path());
            self.file = self.open_append();
        } else if self.file.is_some() && !self.path.exists() {
            // File sudah dirotasi/dihapus proses lain: pindah ke file baru.
            self.file = self.open_append();
        }
    }

    fn write(&mut self, buf: &[u8]) {
        self.writes_since_check += 1;
        if self.writes_since_check >= ROTATE_CHECK_EVERY {
            self.writes_since_check = 0;
            self.rotate_if_needed();
        }
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(buf);
        }
    }

    fn flush(&mut self) {
        if let Some(file) = self.file.as_mut() {
            let _ = file.flush();
        }
    }
}

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
            && let Ok(mut sink) = lock.lock()
        {
            sink.write(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(lock) = LOG_FILE.get()
            && let Ok(mut sink) = lock.lock()
        {
            sink.flush();
        }
        std::io::stderr().flush()
    }
}

/// Inisialisasi logger global. Aman dipanggil lebih dari sekali; hanya
/// panggilan pertama yang berpengaruh. Harus dipanggil setelah
/// `config::init_data_dir()` supaya folder log berada di data dir yang benar.
pub fn init() {
    // Membuka file sekaligus merotasi file lama yang terlalu besar.
    let _ = LOG_FILE.set(Mutex::new(LogSink::open(log_file_path(), MAX_LOG_BYTES)));

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
        // RUST_LOG hanya dihormati jika berisi spesifikasi level yang valid
        // (mis. "debug" atau "tabular=debug"); nilai lain diabaikan.
        let verbose_from_env = std::env::var("RUST_LOG")
            .is_ok_and(|v| v.contains('=') || v.parse::<log::LevelFilter>().is_ok());
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

#[cfg(test)]
mod rotation_tests {
    use super::*;

    fn tmp_log(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tabular-log-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("tabular.log")
    }

    #[test]
    fn oversized_log_is_rotated_at_open() {
        let path = tmp_log("open");
        std::fs::write(&path, vec![b'x'; 200]).expect("seed");
        let mut sink = LogSink::open(path.clone(), 100);
        sink.write(b"fresh\n");
        sink.flush();
        assert_eq!(std::fs::read_to_string(&path).expect("log"), "fresh\n");
        assert_eq!(
            std::fs::metadata(sink.rotated_path())
                .expect("rotated")
                .len(),
            200
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn log_is_rotated_during_the_session() {
        let path = tmp_log("session");
        let mut sink = LogSink::open(path.clone(), 1024);
        let line = [b'a'; 64];
        // Cukup banyak baris untuk melewati batas dan beberapa kali pengecekan.
        for _ in 0..(ROTATE_CHECK_EVERY * 3) {
            sink.write(&line);
        }
        sink.flush();
        let rotated = sink.rotated_path();
        assert!(rotated.exists(), "log must rotate while running");
        let active = std::fs::metadata(&path).expect("active log").len();
        // File aktif tidak tumbuh tanpa batas: paling banyak satu jendela
        // pengecekan di atas batas.
        assert!(
            active <= 1024 + u64::from(ROTATE_CHECK_EVERY) * 64,
            "{active}"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn writer_follows_a_log_rotated_by_another_process() {
        let path = tmp_log("external");
        let mut sink = LogSink::open(path.clone(), 1024 * 1024);
        sink.write(b"before\n");
        std::fs::rename(&path, sink.rotated_path()).expect("external rotate");
        for _ in 0..ROTATE_CHECK_EVERY {
            sink.write(b"after\n");
        }
        sink.flush();
        assert!(
            std::fs::read_to_string(&path)
                .expect("new log")
                .contains("after")
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
