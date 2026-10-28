# ADR 0009: Diagnostik live di Query Editor

Tanggal: 2026-10-28 · Status: Diterima, diterapkan 2026-10-28

## Konteks

Kesalahan query (kolom/tabel tidak ada, sintaks, alias salah) baru terlihat setelah query
dijalankan, dan editor hanya menyediakan "Go to error" untuk PostgreSQL (offset) dan MySQL
(baris). Lint yang ada (`query_tools::lint_sql`, saran index) hanya muncul di toast setelah
eksekusi. Tabular sudah punya bahan untuk memeriksa lebih awal: lexer/analyzer autocomplete
(scope tabel, alias, CTE), cache tabel/kolom in-memory, `sqlparser` per dialek, dan pool
koneksi yang sudah terbuka.

## Keputusan

1. **Modul headless `query_diagnostics`** (tanpa `window_egui`) menghasilkan
   `LintMessage` yang diperluas dengan `code`, `source`, dan `suggestions`. Tipe lint lama
   dipakai ulang, tidak ada tipe diagnostik kedua.
2. **Empat lapis dengan tingkat keyakinan berbeda** (`DiagSource`):
   `Syntax` (leksikal pasti), `Parser` (sqlparser, dialek bisa lebih longgar), `Schema`
   (cache), `Server` (prepare), `Execution` (eksekusi terakhir). `Parser` dan `Schema`
   tentatif: bila validasi server aktif, keduanya ditahan sampai server menjawab, dan dibuang
   bila server menerima statement. Error parser lokal berlevel Warning.
3. **Presisi di atas cakupan.** Metadata yang belum ada = tidak ada tanda. CTE, derived
   table, fungsi tabel, `#temp`, tabel di schema non-default, dan objek yang dibuat skrip
   yang sama tidak diperiksa per kolom. Daftar tabel hanya dari `table_cache` (tabel + view),
   bukan fallback sidebar tree yang bisa belum memuat folder Views.
4. **Validasi server tanpa eksekusi** (`connection::validate`): PostgreSQL Parse di transaksi
   `READ ONLY` + `lock_timeout` lalu ROLLBACK; MySQL `COM_STMT_PREPARE`; SQLite prepare;
   MsSQL `SET NOEXEC ON`. Teks yang memuat `NOEXEC`/`PARSEONLY`/`FMTONLY` tidak pernah
   dikirim, karena T-SQL tidak wajib memakai `;` dan `SET NOEXEC OFF` di dalam statement bisa
   mematikan pengaman. Statement setelah DDL di skrip yang sama, teks bertemplate, dan
   statement > 64 KB tidak dikirim. MsSQL valid hanya berarti "sintaks valid"
   (`ServerVerdict::SyntaxValid`) karena deferred name resolution.
5. **Tidak merebut koneksi user**: hanya koneksi idle atau koneksi baru bila pool belum penuh
   (`acquire_spare`); bila sibuk, coba lagi 1,5 detik kemudian. Timeout 4 detik melepas
   koneksi dari pool.
6. **Default validasi server: kecuali koneksi Production** (label `connection_env`).
   Preferensi `diagnostics_server_validation` (`off`/`non_production`/`always`) dan
   `live_diagnostics`.
7. **Tidak ada I/O di render path**: `diagnostics_jobs::tick` men-debounce (350 ms lokal,
   700 ms server), menganalisis di thread latar, memvalidasi lewat runtime bersama, dan
   menggeser rentang lama (`rebase`) selama analisis baru berjalan. Penanda digambar di atas
   galley TextEdit; ikon gutter digambar dengan painter.
8. **Error eksekusi** dipetakan ke token lewat `query_diagnostics::locate` (offset PG, `near`
   MySQL, `line:` MsSQL, nama dalam pesan SQLite). "Go to error" memakai lokasi yang sama.

## Konsekuensi

- Cache skema yang basi bisa memberi tanda "tidak ada" pada objek baru; hint menyarankan
  refresh koneksi, dan validasi server (bila aktif) mengoreksinya.
- Prepared statement validasi tersimpan di cache statement koneksi pool (LRU sqlx).
- Analyzer autocomplete kini dipakai dua fitur; perubahan scope-nya harus menjaga test di
  `query_diagnostics::semantic`.
