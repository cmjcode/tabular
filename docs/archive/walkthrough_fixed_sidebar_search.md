# Walkthrough: Perbaikan Sidebar Search & Preservasi Konten Folder

Dokumen ini mendokumentasikan implementasi dan penyelesaian perbaikan fitur **Sidebar Search** di Tabular. Perbaikan ini memastikan bahwa ketika pencarian di sidebar mencocokkan sebuah folder/koleksi/database, seluruh konten di dalam folder tersebut (subfolder bersarang, tabel, view, query files, dan request HTTP) tetap dimunculkan serta di-expand secara rekursif.

---

## 1. Masalah & Temuan Review Critic yang Diselesaikan

Berdasarkan tinjauan Adversarial Critic dan instruksi user:
1. **HTTP Collections**: Subfolder pada HTTP Collection otomatis tertutup (*collapsed*) dan kontennya tersembunyi saat parent folder atau workspace cocok dengan kata kunci search.
2. **Database Nodes**: `NodeType::Database` sebelumnya tidak terdaftar dalam `NodeType::is_folder()`, sehingga ketika pengguna mencari nama database, seluruh tabel dan view di bawahnya terhapus/hilang dari hasil filter.
3. **Subfolder Bersarang (Nested Subfolders)**: Subfolder bersarang tidak mengalami auto-expand rekursif pada Saved Queries (`filtered_queries_tree`) maupun Connection Tree (`filter_node_with_like_search`). Hanya folder tingkat pertama yang terbuka, sementara subfolder di dalamnya tetap tertutup.
4. **Inkonsistensi Whitespace & Ketiadaan `.trim()`**: Ketiadaan `.trim()` pada pemrosesan query pencarian riwayat (`sidebar_history.rs`) menyebabkan desinkronisasi status pencarian dengan UI toggle di `app_impl.rs`.

---

## 2. Rincian Perubahan Kode

### 2.1 `src/models/enums.rs`
- Menambahkan variant `NodeType::Database` ke dalam fungsi `NodeType::is_folder()`.
- Dengan ini, node Database diperlakukan sebagai kontainer/folder sehingga saat node Database cocok dengan filter, seluruh hirarki anak di bawahnya dipertahankan.

### 2.2 `src/models/structs.rs`
- Menambahkan metode baru `TreeNode::expand_all_folders(&mut self)`:
  ```rust
  pub fn expand_all_folders(&mut self) {
      if self.node_type.is_folder() {
          self.is_expanded = true;
      }
      for child in &mut self.children {
          child.expand_all_folders();
      }
  }
  ```
- Memastikan ekspansi terjadi secara mendalam (rekursif) ke seluruh level subfolder anak, bukan hanya node terluar.

### 2.3 `src/window_egui/search.rs`
- Pada `filter_node_with_like_search`, ketika sebuah folder (termasuk `NodeType::Database`, `CustomFolder`, dll) cocok dengan teks pencarian:
  - Seluruh node anak di-clone dan dipertahankan.
  - Memanggil `filtered_node.expand_all_folders()` untuk membuka folder utama beserta semua subfoldernya.
- Pada `update_all_database_search_results`, melakukan `.trim()` pada `database_search_text` saat mengupdate `history_search_text`.
- Menambahkan unit test:
  - `test_filter_node_with_like_search_folder_preserves_children`: Menguji folder custom mempertahankan semua koneksi anak saat folder dicari.
  - `test_filter_node_database_preserves_tables_and_expands_folders`: Menguji node `Database` mempertahankan `TablesFolder` dan `ViewsFolder` serta auto-expand saat database dicari.
  - `test_filter_node_nested_subfolders_recursive_expand`: Menguji ekspansi rekursif subfolder bertingkat (`Servers -> Regional -> Europe`).

### 2.4 `src/sidebar_query.rs`
- Pada `filter_queries_tree`, saat `node.node_type.is_folder()` cocok dengan teks pencarian:
  - Memanggil `filtered_node.expand_all_folders()`.
- Menambahkan unit test:
  - `test_filter_queries_tree_nested_subfolders_recursive_expand`: Menguji folder query bertingkat (`Finance -> 2026 Reports -> Monthly Report.sql`) ter-expand otomatis secara rekursif saat parent folder dicari.

### 2.5 `src/sidebar_history.rs`
- Pada `filter_history_tree`:
  - Menambahkan `.trim()` pada `tabular.history_search_text.trim()`.
  - Jika query kosong atau hanya berisi spasi whitespace, `filtered_history_tree` dikosongkan secara konsisten.
- Menambahkan unit test pengujian whitespace, query berjarak (`"  users  "`), dan query kosong (`"   "`).

### 2.6 `src/sidebar_collection.rs`
- Menambahkan parameter `parent_matched: bool` pada `render_folder_node`.
- Menambahkan deteksi `ws_matches` pada level workspace HTTP Collection.
- Saat parent folder atau workspace cocok dengan kata kunci pencarian:
  - `folder_matches` menjadi `true` secara propagatif untuk semua subfolder di bawahnya.
  - `is_expanded` menjadi `true` untuk semua subfolder turunan.
  - Seluruh requests dan child folders tidak di-skip, sehingga tetap ditampilkan utuh kepada pengguna.
- Menambahkan unit test:
  - `test_folder_and_workspace_has_match`: Menguji pencarian pada level workspace, parent folder, subfolder, serta endpoint URL request.

---

## 3. Matriks Pengujian & Verifikasi

| Komponen Sidebar | Skenario Pengujian | Hasil yang Diharapkan | Status |
| :--- | :--- | :--- | :--- |
| **Database Connection Tree** | Cari nama folder custom (misal `"Production"`) | Folder terbuka, semua koneksi di dalamnya tetap muncul | Terverifikasi (`search.rs`) |
| **Database & Schema Tree** | Cari nama database (misal `"ecommerce"`) | Database terbuka, `TablesFolder`, `ViewsFolder`, serta tabel/view di dalamnya tetap muncul | Terverifikasi (`search.rs`) |
| **Database Connection Tree** | Subfolder bertingkat (`Servers -> Regional -> Europe`) | Seluruh tingkatan subfolder ter-expand otomatis | Terverifikasi (`search.rs`) |
| **Saved Queries Tree** | Subfolder bertingkat (`Finance -> 2026 Reports`) | Seluruh tingkatan subfolder query ter-expand otomatis | Terverifikasi (`sidebar_query.rs`) |
| **Query History** | Pencarian dengan whitespace / spasi trailing (`"  users  "`, `"   "`) | Ter-trim dengan konsisten, tidak ada desinkronisasi UI tree | Terverifikasi (`sidebar_history.rs`) |
| **HTTP API Collection** | Cari nama folder parent atau workspace | Subfolder tidak tertutup, seluruh endpoint/request anak tetap dimunculkan | Terverifikasi (`sidebar_collection.rs`) |

---

## 4. Kesimpulan

Semua catatan kritik review telah diperbaiki tuntas di seluruh 4 domain sidebar (Database Tree, Saved Queries, History, dan HTTP Collections) dengan penanganan rekursif yang konsisten, penanganan whitespace yang aman, serta cakupan unit tests komprehensif.
