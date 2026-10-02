//! Probe waktu per job query (checklist I2): membagi durasi total menjadi
//! tunggu koneksi, server (sampai baris pertama), transfer (baris pertama
//! sampai stream habis), dan proses klien (konversi baris, sisa overhead).
//!
//! Probe disimpan di `task_local` tokio sehingga driver cukup memanggil
//! `mark_*` di jalur fetch tanpa mengubah signature. Di luar
//! [`with_probe`] semua `mark_*` tidak berefek (mis. dipanggil dari agent).
//! Hanya statement terakhir yang mengembalikan baris yang diukur; statement
//! tanpa result set (`execute`) tidak menghasilkan breakdown.

use std::cell::RefCell;
use std::future::Future;
use std::time::Instant;

/// Rincian durasi satu eksekusi, dalam milidetik.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct QueryTiming {
    /// Mengambil koneksi dari pool, `SET search_path`, statement sebelumnya.
    pub wait_ms: f64,
    /// Statement dikirim sampai baris pertama tiba (eksekusi di server).
    pub server_ms: f64,
    /// Baris pertama sampai stream selesai (transfer jaringan + decode driver).
    pub transfer_ms: f64,
    /// Setelah stream selesai: konversi ke tabel string dan overhead lain.
    pub client_ms: f64,
}

impl QueryTiming {
    pub fn total_ms(&self) -> f64 {
        self.wait_ms + self.server_ms + self.transfer_ms + self.client_ms
    }
}

#[derive(Default)]
struct Probe {
    statement_start: Option<Instant>,
    first_row: Option<Instant>,
    fetch_end: Option<Instant>,
}

tokio::task_local! {
    static PROBE: RefCell<Probe>;
}

fn ms(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.0
}

/// Hitung breakdown dari titik waktu yang tercatat. `None` bila statement
/// yang mengembalikan baris tidak pernah selesai di-fetch.
fn compute(
    start: Instant,
    statement_start: Option<Instant>,
    first_row: Option<Instant>,
    fetch_end: Option<Instant>,
    end: Instant,
) -> Option<QueryTiming> {
    let statement_start = statement_start?;
    let fetch_end = fetch_end?;
    // Result set kosong: seluruh waktu fetch dianggap waktu server.
    let first_row = first_row.unwrap_or(fetch_end);
    Some(QueryTiming {
        wait_ms: ms(start, statement_start),
        server_ms: ms(statement_start, first_row),
        transfer_ms: ms(first_row, fetch_end),
        client_ms: ms(fetch_end, end),
    })
}

/// Jalankan `fut` dengan probe aktif; kembalikan hasilnya plus breakdown waktu
/// relatif terhadap `start` (awal job).
pub(crate) async fn with_probe<F: Future>(
    start: Instant,
    fut: F,
) -> (F::Output, Option<QueryTiming>) {
    PROBE
        .scope(RefCell::new(Probe::default()), async move {
            let out = fut.await;
            let timing = PROBE.with(|p| {
                let p = p.borrow();
                compute(
                    start,
                    p.statement_start,
                    p.first_row,
                    p.fetch_end,
                    Instant::now(),
                )
            });
            (out, timing)
        })
        .await
}

/// Statement yang mengembalikan baris mulai di-fetch. Mereset titik sebelumnya
/// sehingga hanya statement terakhir yang tercatat.
pub(crate) fn mark_statement_start() {
    let _ = PROBE.try_with(|p| {
        let mut p = p.borrow_mut();
        p.statement_start = Some(Instant::now());
        p.first_row = None;
        p.fetch_end = None;
    });
}

pub(crate) fn mark_first_row() {
    let _ = PROBE.try_with(|p| {
        let mut p = p.borrow_mut();
        if p.first_row.is_none() {
            p.first_row = Some(Instant::now());
        }
    });
}

pub(crate) fn mark_fetch_end() {
    let _ = PROBE.try_with(|p| p.borrow_mut().fetch_end = Some(Instant::now()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn compute_membagi_durasi_per_fase() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let t = compute(t0, Some(at(10)), Some(at(110)), Some(at(160)), at(170)).unwrap();
        assert!((t.wait_ms - 10.0).abs() < 0.5);
        assert!((t.server_ms - 100.0).abs() < 0.5);
        assert!((t.transfer_ms - 50.0).abs() < 0.5);
        assert!((t.client_ms - 10.0).abs() < 0.5);
        assert!((t.total_ms() - 170.0).abs() < 0.5);
    }

    #[test]
    fn result_kosong_dihitung_sebagai_waktu_server() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let t = compute(t0, Some(at(5)), None, Some(at(45)), at(46)).unwrap();
        assert!((t.server_ms - 40.0).abs() < 0.5);
        assert_eq!(t.transfer_ms, 0.0);
    }

    #[test]
    fn tanpa_fetch_tidak_ada_breakdown() {
        let t0 = Instant::now();
        assert!(compute(t0, None, None, None, t0).is_none());
        assert!(compute(t0, Some(t0), None, None, t0).is_none());
    }

    #[tokio::test]
    async fn probe_mencatat_statement_terakhir() {
        let start = Instant::now();
        let ((), timing) = with_probe(start, async {
            mark_statement_start();
            mark_first_row();
            mark_fetch_end();
            mark_statement_start();
            tokio::time::sleep(Duration::from_millis(5)).await;
            mark_first_row();
            mark_fetch_end();
        })
        .await;
        let timing = timing.expect("breakdown tersedia");
        assert!(timing.server_ms >= 4.0);
    }

    #[test]
    fn mark_di_luar_probe_tidak_panic() {
        mark_statement_start();
        mark_first_row();
        mark_fetch_end();
    }
}
