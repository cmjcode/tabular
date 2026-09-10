# Implementation Plan: Perbaikan Sidebar Search & Preservasi Konten Folder

Dokumen ini menjelaskan rencana teknis perbaikan dan penyempurnaan fitur **Sidebar Search** pada aplikasi Tabular. Perbaikan ini memastikan bahwa pencarian pada sidebar mempertahankan seluruh konten di dalam folder yang cocok (subfolder bersarang, tabel, view, query files, dan request HTTP) serta meng-expand seluruh folder turunan secara rekursif.

---

## 1. Analisis Masalah (Problem Statement)

### 1.1 Masalah pada Fitur Pencarian Sidebar Sebelumnya
1. **HTTP Collections**: Subfolder pada koleksi HTTP otomatis tertutup (*collapsed*) dan item request di dalamnya tersembunyi ketika parent folder atau workspace cocok dengan kueri pencarian.
2. **Database Nodes**: Tipe `NodeType::Database` belum terdaftar dalam fungsi `NodeType::is_folder()`. Akibatnya, saat pengguna mencari nama database, seluruh tabel dan view di dalamnya terfilter keluar (hilang dari hasil pencarian).
3. **Subfolder Bersarang (Nested Subfolders)**: Subfolder bertingkat tidak mengalami auto-expand rekursif pada Saved Queries (`filtered_queries_tree`) maupun Connection Tree (`filter_node_with_like_search`). Hanya folder tingkat pertama yang terbuka, sementara subfolder di dalamnya tetap tertutup.
4. **Inkonsistensi Whitespace & Ketiadaan `.trim()`**: Ketiadaan `.trim()` pada pemrosesan pencarian riwayat query (`sidebar_history.rs`) menyebabkan pencarian dengan spasi awal/akhir tidak menemukan hasil dan menyebabkan desinkronisasi status pencarian dengan UI toggle di `app_impl.rs`.

---

## 2. Desain Solusi & Rencana Perubahan

### 2.1 Preservasi Node Database sebagai Folder (`src/models/enums.rs`)
- Menambahkan variant `NodeType::Database` ke dalam fungsi pembantu `NodeType::is_folder()`.
- Hal ini memastikan bahwa node database diperlakukan sebagai kontainer/folder sehingga saat node database cocok dengan kueri pencarian, seluruh hirarki anak di bawahnya (`TablesFolder`, `ViewsFolder`, tabel, dan view) dipertahankan utuh.

### 2.2 Ekspansi Rekursif Subfolder (`src/models/structs.rs`)
- Menambahkan method rekursif `expand_all_folders(&mut self)` pada struct `TreeNode`:
  ```rust
  impl TreeNode {
      pub fn expand_all_folders(&mut self) {
          if self.node_type.is_folder() {
              self.is_expanded = true;
          }
          for child in &mut self.children {
              child.expand_all_folders();
          }
      }
  }
  ```
- Memastikan bahwa saat sebuah folder lolos pencarian, seluruh subfolder turunan di dalamnya otomatis terbuka (`is_expanded = true`) sampai tingkat terdalam.

### 2.3 Perbaikan Filter Connection & Database Tree (`src/window_egui/search.rs`)
- Pada fungsi `filter_node_with_like_search`:
  - Ketika sebuah folder (kategori, folder koneksi, atau database) cocok dengan teks pencarian, seluruh node anak di-clone dan dipertahankan.
  - Memanggil `filtered_node.expand_all_folders()` untuk membuka folder utama beserta semua subfoldernya.
- Pada `update_all_database_search_results`:
  - Melakukan `.trim()` pada teks pencarian database saat memperbarui `history_search_text`.

### 2.4 Perbaikan Saved Queries Tree (`src/sidebar_query.rs`)
- Pada fungsi `filter_queries_tree`:
  - Saat `node.node_type.is_folder()` cocok dengan teks pencarian, seluruh struktur folder anak dipertahankan dan dilakukan `filtered_node.expand_all_folders()`.

### 2.5 Normalisasi Whitespace pada History Search (`src/sidebar_history.rs`)
- Pada fungsi `filter_history_tree`:
  - Melakukan `.trim()` pada `tabular.history_search_text.trim()`.
  - Jika kueri kosong atau hanya whitespace, kosongkan `filtered_history_tree` secara deterministik dan konsisten dengan status UI.

### 2.6 Propagasi Pencocokan Workspace & Parent pada HTTP Collection (`src/sidebar_collection.rs`)
- Menambahkan parameter `parent_matched: bool` pada `render_folder_node`.
- Mendeteksi kecocokan pada level workspace (`ws_matches`).
- Jika parent folder atau workspace cocok dengan kata kunci:
  - Propagasi status kecocokan ke seluruh subfolder turunan.
  - Set `is_expanded = true` secara rekursif pada folder-folder turunan.
  - Tampilkan seluruh requests dan child folders tanpa di-filter keluar.

---

## 3. Rencana Pengujian (Testing Plan)

1. **Unit Tests - Search (`src/window_egui/search.rs`)**:
   - `test_filter_node_with_like_search_folder_preserves_children`: Memastikan koneksi di bawah folder tetap muncul saat nama folder dicari.
   - `test_filter_node_database_preserves_tables_and_expands_folders`: Memastikan tabel dan view di bawah database tetap muncul dan terbuka.
   - `test_filter_node_nested_subfolders_recursive_expand`: Memastikan subfolder bertingkat (`Servers -> Regional -> Europe`) ter-expand secara rekursif.

2. **Unit Tests - Saved Queries (`src/sidebar_query.rs`)**:
   - `test_filter_queries_tree_nested_subfolders_recursive_expand`: Memastikan hierarki query file bertingkat terbuka utuh saat parent folder dicari.

3. **Unit Tests - History Search (`src/sidebar_history.rs`)**:
   - Pengujian kueri whitespace (`"  users  "` dan `"   "`).

4. **Unit Tests - HTTP Collections (`src/sidebar_collection.rs`)**:
   - `test_folder_and_workspace_has_match`: Memastikan pencarian nama workspace dan parent folder memunculkan seluruh request di dalamnya.

5. **Kompilasi & Regresi**:
   - Menjalankan `cargo test` untuk memverifikasi seluruh test suite lulus tanpa error.
