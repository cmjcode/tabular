# ADR 0002: Engine Database Baru sebagai Plugin, Engine yang Ada Tetap Builtin

Tanggal: 2026-09-29 · Status: Diterima, diterapkan 2026-09-30 (registry publik belum)

## Konteks

Bagian L di `docs/TABLEPRO_GAP_CHECKLIST.md` berisi 17+ engine baru (ClickHouse,
Elasticsearch, Oracle, Snowflake, DuckDB, dan lainnya). Menambahkan semuanya
sebagai kode builtin membuat crate membengkak, menambah dependensi yang
berisiko bentrok (DuckDB membawa SQLite sendiri, sedangkan `libsqlite3-sys`
dipin 0.37), dan memaksa rilis app untuk setiap perbaikan driver.

`DatabaseType` adalah enum tertutup dengan sekitar 869 titik `match` di 67
file. Runtime plugin `wasmi` yang ada (`src/plugin_runtime/`) hanya dipakai
untuk export dan ORM: sekali jalan, tanpa state, tanpa I/O jaringan.

## Keputusan

1. **Engine yang sudah ada tetap builtin**: PostgreSQL, MySQL, SQLite, SQL
   Server, MongoDB, Redis. Fitur dalamnya (DBA monitor, user manager, backup,
   structure editor, profiler, sesi transaksi ADR 0001) tidak dipindah ke
   balik API plugin. Alasannya: overhead serialisasi dan interpreter, biaya
   menulis ulang ratusan titik `match`, SQLite wajib untuk `connections.db`,
   dan iOS tetap butuh semuanya dibundel.
2. **Engine baru dibuat sebagai plugin**, lewat satu trait `EngineDriver`
   (`src/driver_api/`) dengan dua host:
   - **Wasm** (`wasmi`) untuk engine berbasis HTTP. Host yang menjalankan
     request HTTP dan mem-parse format respons standar secara native; plugin
     menyusun request dan logika engine. Akses jaringan dibatasi allowlist
     host di manifest.
   - **Sidecar** (proses terpisah, JSON-RPC lewat stdio, desktop saja) untuk
     protokol biner atau library native: Oracle, Cassandra, Kafka, DuckDB.
     Binary sidecar tidak ter-sandbox, jadi wajib persetujuan eksplisit per
     hash SHA-256.
3. **Driver builtin juga mengimplementasikan `EngineDriver` di dalam proses**
   (adapter tanpa IPC). SQLite menjadi adapter pertama sekaligus fixture test,
   untuk membuktikan API cukup sebelum ABI Wasm dibekukan.
4. **Varian engine di atas driver builtin** (MariaDB, TiDB, CockroachDB,
   Redshift, dsb., item L2/L3) cukup berupa preset, bukan plugin.
5. **Tipe koneksi yang tidak dikenal tidak pernah dipaksa menjadi tipe lain.**
   Sebelumnya enam titik parsing `connections.connection_type` jatuh ke
   `SQLite` untuk nilai tak dikenal, dan tiga di antaranya juga salah
   memetakan `ApiHttp` menjadi SQLite. Sekarang semua lewat
   `DatabaseType::from_db_str`, dan koneksi bertipe tak dikenal dilewati
   dengan log `warn` tanpa mengubah barisnya.

## Fase

| Fase | Isi | Usaha |
|---|---|---|
| 0 | `from_db_str`/`as_db_str`, hapus fallback ke SQLite | S (selesai) |
| 1 | Trait `EngineDriver`, `EngineCapabilities`, `EngineDescriptor`, `DriverRegistry`, varian `DatabaseType::Plugin(String)` dan `DatabasePool::Plugin`, adapter SQLite | M |
| 2 | Jalur generik: form koneksi dinamis, pool, eksekusi ke `QueryJobOutput`, pohon sidebar, cache metadata, MCP read-only; fitur dalam disembunyikan per capability | L |
| 3 | Host Wasm ABI `tabular-driver-v1` (HTTP lewat host, secret per koneksi, fuel & batas memori), manifest `plugin.toml`, SDK guest, benchmark 10k/100k baris | L |
| 4 | Host sidecar (JSON-RPC stdio, restart/backoff, persetujuan per hash) | M |
| 6 | UI Plugin Manager (tab Database Drivers), install dari file | M |
| 7 | Plugin referensi: ClickHouse (Wasm), DuckDB (sidecar) | M per plugin |

Fase 5 dari rancangan awal (cargo feature untuk SQL Server, MongoDB, Redis)
dibatalkan karena engine yang ada tetap builtin penuh.

## Implementasi

| Bagian | Lokasi |
|---|---|
| Trait, tipe protokol, error | `src/driver_api/mod.rs` |
| Registry, preset L2/L3 | `src/driver_api/registry.rs`, `presets.rs` |
| Manifest, install/uninstall, enable, persetujuan sidecar | `src/driver_api/manifest.rs` (`<data_dir>/plugins/drivers/`) |
| Host Wasm (ABI v1, pool 4 instance/sesi, fuel, 256 MiB) | `src/driver_api/wasm_host.rs` |
| HTTP host + allowlist + parser tabel native | `src/driver_api/http.rs` |
| Host sidecar (JSON-RPC stdio, restart) | `src/driver_api/sidecar.rs` |
| Koneksi, opsi rahasia, cache metadata, helper SQL | `src/driver_api/{connect,cache,query}.rs` |
| Adapter SQLite in-process (fixture) | `src/driver_api/sqlite_adapter.rs` |
| Form koneksi, pohon sidebar, tab Database Drivers | `src/window_egui/plugin_{connection_form,tree}.rs`, `src/plugin_runtime/drivers_ui.rs` |
| SDK guest + plugin referensi | `plugins/sdk`, `plugins/examples/{clickhouse,duckdb-sidecar}` |

Kolom baru `connections.plugin_options` (JSON). Tipe koneksi plugin disimpan sebagai
`plugin:<id>`. Agent/MCP memakai jalur eksekusi yang sama; engine plugin non-SQL
diklasifikasi `Unknown` sehingga ditolak (read-only fail closed).

## Belum diputuskan

- Registry publik bertanda tangan (ed25519) atau cukup install dari file dulu.
  Usulan: install dari file dulu, registry setelah ABI v1 stabil.
- `wasmtime` (JIT) untuk desktop hanya bila benchmark Fase 3 menunjukkan
  parsing di host dan format biner belum cukup.

## Konsekuensi

- Plugin engine hanya mendapat fitur generik (koneksi, pohon objek, query,
  grid, chart, export). Fitur dalam muncul hanya bila capability-nya ada.
- Koneksi plugin yang dibuka di perangkat tanpa plugin itu tetap tersimpan dan
  tampil sebagai "Driver not installed" (setelah Fase 1).
- Versi app sebelum Fase 0 akan membaca koneksi plugin sebagai SQLite. Fase 0
  harus dirilis setidaknya satu versi sebelum plugin driver pertama.
