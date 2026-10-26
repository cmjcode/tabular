//! Antrean kegagalan penyimpanan dari kode headless/latar untuk ditampilkan UI.
//!
//! Kode yang berjalan di runtime latar atau tidak memegang `&mut Tabular`
//! tidak bisa memunculkan toast. Ia memanggil [`report`]; GUI mengambil
//! antrean lewat [`take`] setiap frame dan menampilkannya sekali per `key`.
//! Proses tanpa GUI (`tabular mcp`) tidak pernah mengambil antrean; ukurannya
//! dibatasi sehingga tidak tumbuh tanpa batas.

use std::sync::{Mutex, PoisonError};

/// Tingkat keparahan notifikasi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Warning,
    Error,
}

/// Satu kejadian yang perlu diketahui user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Kunci deduplikasi: notifikasi dengan kunci sama tampil sekali per sesi.
    pub key: String,
    pub level: Level,
    pub message: String,
}

const MAX_PENDING: usize = 64;

static PENDING: Mutex<Vec<Notice>> = Mutex::new(Vec::new());

/// Catat kegagalan penyimpanan untuk ditampilkan ke user. Juga ditulis ke log.
pub fn report(key: &str, level: Level, message: impl Into<String>) {
    let message = message.into();
    match level {
        Level::Warning => log::warn!("[PERSIST] {key}: {message}"),
        Level::Error => log::error!("[PERSIST] {key}: {message}"),
    }
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    if pending.len() < MAX_PENDING && !pending.iter().any(|n| n.key == key) {
        pending.push(Notice {
            key: key.to_string(),
            level,
            message,
        });
    }
}

/// Ambil dan kosongkan antrean.
pub fn take() -> Vec<Notice> {
    let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
    std::mem::take(&mut *pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_dedups_by_key_and_take_drains() {
        // Kunci unik agar tidak bentrok dengan tes lain yang berjalan paralel.
        let key = "test-persist-notice-dedup";
        report(key, Level::Error, "first");
        report(key, Level::Error, "second");
        let mine: Vec<_> = take().into_iter().filter(|n| n.key == key).collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].message, "first");
        assert!(take().iter().all(|n| n.key != key));
    }
}
