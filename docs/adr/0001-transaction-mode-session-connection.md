# ADR 0001: Mode Transaksi via Koneksi Sesi Dedicated per Tab

Tanggal: 2026-06-13 · Status: Diterima

## Konteks

Eksekusi query Tabular meminjam koneksi dari pool per statement (PG/SQLite)
atau membuka koneksi baru per job (MySQL). Akibatnya tidak ada state sesi
yang menetap antar eksekusi: `BEGIN`/`COMMIT` manual, `SET @var`, dan
`USE db` tidak berlaku untuk statement berikutnya. DataGrip-style manual
commit membutuhkan satu koneksi yang dipegang selama transaksi.

## Keputusan

1. **Session task per query-tab (opt-in).** Saat user mengaktifkan toggle
   "Manual commit" pada sebuah tab, eksekusi tab itu dialihkan ke satu task
   tokio (`connection/session.rs`) yang memegang **satu koneksi dedicated**
   (diambil dari pool sqlx) selama sesi hidup. Komunikasi lewat
   `tokio::mpsc::UnboundedSender<SessionCommand>`:
   `Execute { job_id, sql }`, `Commit`, `Rollback`, `Close`.
2. **Transaksi dimulai lazily.** `BEGIN` (MySQL: `START TRANSACTION`)
   dikirim otomatis sebelum statement pertama ketika belum ada transaksi
   terbuka; `Commit`/`Rollback` menutupnya; statement berikutnya membuka
   transaksi baru. Ini meniru perilaku auto-BEGIN DataGrip.
3. **Hasil lewat jalur yang sudah ada.** Session task mengirim
   `QueryResultMessage` per command lewat `query_result_sender`, sehingga
   status job, panel hasil multi-result, dan pesan error memakai pipeline
   UI yang sama dengan eksekusi biasa.
4. **Cakupan engine bertahap.** MySQL, PostgreSQL, SQLite dulu (API sqlx
   seragam lewat `PoolConnection`). MsSQL menyusul (jalur tiberius/deadpool
   terpisah); MongoDB/Redis tidak relevan — toggle disembunyikan.
5. **Konversi hasil ringan.** Session memakai konverter row→string driver
   yang sudah ada (`convert_mysql_rows_to_table_data`,
   `convert_sqlite_rows_to_table_data`; PG memakai pola decode executor).
   Tanpa inferensi metadata/PK yang berat — grid hasil mode transaksi
   bersifat baca (edit inline butuh jalur pool terpisah yang justru tidak
   melihat transaksi yang sedang terbuka).

## Konsekuensi

- Satu koneksi pool tersita per tab yang mengaktifkan manual commit;
  sesi ditutup (rollback implisit) saat toggle dimatikan atau tab ditutup.
- Auto-pagination server dinonaktifkan untuk eksekusi dalam mode transaksi.
- Error mid-transaksi membiarkan transaksi terbuka (PG: aborted state)
  sampai user memilih Commit/Rollback — sama seperti psql/DataGrip.
