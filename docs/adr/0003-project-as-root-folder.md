# ADR 0003: Project sebagai Root Folder Lintas Tool

Tanggal: 2026-09-30 · Status: Diterima, diterapkan 2026-09-30

## Konteks

Connections, Queries, dan HTTP Client masing-masing punya sistem folder sendiri:
tabel `connection_folders` di `connections.db`, direktori di bawah `query/`, dan
`HttpWorkspace` di `http_collections/`. Satu aplikasi yang sama tersebar di tiga
pohon tanpa relasi. Environment juga terpisah: tanda environment per koneksi
(`connection_env`) dan `YaakEnvironment` per workspace HTTP yang tidak pernah
dipakai saat request dikirim. Agent AI tidak punya tempat untuk menyimpan
pengetahuan per aplikasi.

## Keputusan

1. **Project adalah root folder, bukan tabel relasi.** Keanggotaan dihitung dari
   path: koneksi milik project bila `folder` sama dengan `connection_folder` atau
   subfoldernya, file query bila ada di bawah `query/{query_folder}`, request HTTP
   bila ada di workspace `http_workspace_id`. Folder lama bisa diangkat menjadi
   project tanpa migrasi data, dan menghapus project tidak menghapus isinya.
2. **Manifest per project** di `<data dir>/projects/<id>/project.json`, headless
   (`src/project.rs`) supaya bisa dipakai GUI, MCP, dan test.
3. **Environment milik project.** Variabel dipakai lewat `{{KEY}}`. HTTP memakai
   semua variabel termasuk secret; SQL hanya non-secret karena teks query masuk
   riwayat. Nilai secret ada di keychain dengan nama `project:<id>:<env>:<key>`.
   Koneksi per environment dicatat dengan nama, bukan id, karena id lokal per mesin.
4. **Memory agent per project** berupa file Markdown (`src/project_memory.rs`),
   ikut dibagikan bersama project, dan disensor dari nilai secret sebelum ditulis.
5. **Share memakai `team_shared_folders` yang sudah ada.** Satu project dibagikan
   sebagai empat share: `connection`, `query`, `http`, dan `project` (manifest).
   Share folder sekarang mencakup subfolder (pencocokan prefix di server dan klien).
   Manifest disimpan terenkripsi di tabel `projects` server.

## Konsekuensi

- Rename project harus ikut me-rename folder dan membagikan ulang, karena share
  dikunci pada path. Project yang dibagikan hanya bisa di-rename pemiliknya.
- Pencocokan prefix mengubah arti share folder yang sudah ada: subfoldernya kini
  ikut terbagi. Ini sesuai harapan user, tapi berlaku juga untuk share lama.
- Sinkronisasi query sebelumnya mencatat semua file sebagai folder `/`. Setelah
  perbaikan, baris lama dengan isi sama dipindah ke folder yang benar saat push.
- Merge project memakai `updated_at` terbaru; tidak ada merge per field.
- `YaakEnvironment` dibiarkan untuk import Yaak/Postman; environment project yang
  dipakai saat kirim.
