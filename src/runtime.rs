//! Satu runtime tokio bersama untuk seluruh proses.
//!
//! Sebelumnya banyak modul membuat `Runtime::new()` sendiri per panggilan
//! (metadata, cache, HTTP, sync): setiap panggilan memulai thread worker baru,
//! pool yang dibuat di runtime sementara rusak saat runtime itu di-drop, dan
//! `Runtime::new()` di dalam konteks tokio langsung panic. Sekarang semua kode
//! memakai [`shared`], [`block_on`], atau [`spawn`].
//!
//! Pengecualian yang disengaja: titik masuk CLI (`tabular mcp`, `tabular test`)
//! boleh membangun runtime sendiri karena prosesnya terpisah dari GUI.

use std::future::Future;
use std::sync::{Arc, OnceLock};

use tokio::runtime::{Handle, Runtime, RuntimeFlavor};

static SHARED: OnceLock<Option<Arc<Runtime>>> = OnceLock::new();

/// Runtime bersama (multi-thread). `None` hanya bila OS menolak membuat thread;
/// kegagalan itu dicatat sekali.
pub fn shared() -> Option<Arc<Runtime>> {
    SHARED
        .get_or_init(|| {
            // Satu-satunya tempat runtime aplikasi dibuat.
            #[allow(clippy::disallowed_methods)]
            let mut builder = tokio::runtime::Builder::new_multi_thread();
            match builder
                .enable_all()
                .worker_threads(4)
                .thread_name("tabular-rt")
                .build()
            {
                Ok(rt) => Some(Arc::new(rt)),
                Err(e) => {
                    log::error!("[RUNTIME] cannot start the shared tokio runtime: {e}");
                    None
                }
            }
        })
        .clone()
}

/// Jalankan `fut` sampai selesai dari konteks apa pun:
/// - thread biasa: di runtime bersama;
/// - worker runtime multi-thread: lewat `block_in_place` (tanpa panic);
/// - runtime current-thread: di thread terpisah, karena thread itu tidak boleh
///   diblokir.
///
/// Mengembalikan `None` bila runtime bersama tidak bisa dibuat.
pub fn block_on<F>(fut: F) -> Option<F::Output>
where
    F: Future + Send,
    F::Output: Send,
{
    if let Ok(handle) = Handle::try_current() {
        if handle.runtime_flavor() == RuntimeFlavor::MultiThread {
            return Some(tokio::task::block_in_place(|| handle.block_on(fut)));
        }
        return std::thread::scope(|scope| {
            scope
                .spawn(|| shared().map(|rt| rt.block_on(fut)))
                .join()
                .ok()
                .flatten()
        });
    }
    shared().map(|rt| rt.block_on(fut))
}

/// Jalankan `fut` di latar pada runtime bersama. `false` bila runtime tidak ada.
pub fn spawn<F>(fut: F) -> bool
where
    F: Future<Output = ()> + Send + 'static,
{
    match shared() {
        Some(rt) => {
            rt.spawn(fut);
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn answer() -> u32 {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        42
    }

    #[test]
    fn block_on_from_plain_thread() {
        assert_eq!(block_on(answer()), Some(42));
    }

    #[test]
    fn block_on_inside_multi_thread_worker_does_not_panic() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let got = rt.block_on(async { tokio::spawn(async { block_on(answer()) }).await });
        assert_eq!(got.unwrap(), Some(42));
    }

    #[test]
    fn block_on_inside_current_thread_runtime_does_not_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(rt.block_on(async { block_on(answer()) }), Some(42));
    }

    #[test]
    fn block_on_from_spawn_blocking() {
        let rt = shared().unwrap();
        let got = rt.block_on(async { tokio::task::spawn_blocking(|| block_on(answer())).await });
        assert_eq!(got.unwrap(), Some(42));
    }

    #[test]
    fn shared_runtime_is_one_instance() {
        let a = shared().unwrap();
        let b = shared().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
