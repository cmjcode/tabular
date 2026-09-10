# Rencana Teknis Implementasi: Drag & Drop Tab Editor dan Pin Tab

Dokumen ini menjelaskan rencana teknis menyeluruh untuk menambahkan fitur **Drag & Drop Tab Editor** (reordering & visual feedback) dan **Pin Tab** (penyematan tab editor di sisi kiri dengan proteksi penutupan) pada aplikasi Tabular.

---

## 1. Analisis Kebutuhan (Requirements Analysis)

### 1.1 Latar Belakang & Tujuan
Saat bekerja dengan banyak tab query SQL, tabel basis data, atau API HTTP, tab editor sering kali menumpuk tanpa pengelompokan yang jelas. Pengguna membutuhkan:
1. Kemampuan untuk **menyeret dan menggeser tab** (drag-and-drop horizontal) secara interaktif untuk mengatur urutan dan mengelompokkan tab yang saling berhubungan.
2. Fitur **Pin Tab (📌)** untuk menandai tab-tab penting (koneksi aktif, query pemantauan, atau referensi) agar selalu berada di posisi paling kiri dan tidak tertutup secara tidak sengaja.
3. Menu konteks navigasi tab yang lengkap (Pin/Unpin, Move Left/Right, Close, Close Others, Close to the Right).

### 1.2 Batasan Arsitektural & Invarian
1. **Invarian Urutan Tab (Pinned vs Unpinned)**:
   - Seluruh tab yang memiliki status `is_pinned == true` harus selalu berada di sebelah kiri tab-tab yang `is_pinned == false`.
   - Garis pembatas visual (vertical divider) memisahkan kelompok tab pinned dari kelompok tab unpinned.
   - Jika pengguna menyeret tab unpinned ke dalam zona pinned, tab tersebut otomatis menjadi pinned.
   - Jika pengguna menyeret tab pinned ke dalam zona unpinned, tab tersebut otomatis menjadi unpinned.
2. **Integritas Sesi Transaksi & Sumber Daya**:
   - Tab yang ditutup melalui *Close Other Tabs* atau *Close Tabs to the Right* harus memanggil `session.close()` jika memiliki manual-commit transaction handle (`SessionHandle`), mencegah kebocoran sesi koneksi.
3. **Pembaruan Sinkron Indeks Tab Aktif (`active_tab_index`)**:
   - Pemindahan tab ke kanan maupun ke kiri harus memperbarui `active_tab_index` secara presisi tanpa out-of-bounds index atau desinkronisasi konten editor.
4. **Resilience & Liveness Drag State**:
   - Drag state (`dragged_tab_index`) harus aman dari kondisi stuck: pembatalan via tombol `Escape`, pelepasan pointer primer (`button_released(PointerButton::Primary)`), liveness check saat pointer tidak lagi ditekan (`!primary_down`), serta toleransi batas vertikal (`is_within_tab_bar_y`).

---

## 2. Desain Solusi Teknis & Struktur Data

### 2.1 Model Data Tab (`src/models/structs.rs`)
Menambahkan atribut `is_pinned` pada struct `QueryTab`:
```rust
pub struct QueryTab {
    pub id: usize,
    pub title: String,
    pub content: String,
    pub file_path: Option<String>,
    pub is_saved: bool,
    pub is_modified: bool,
    pub is_pinned: bool,
    ...
}
```

### 2.2 Field State pada `Tabular` (`src/window_egui/mod.rs` & `init.rs`)
Menambahkan field state penanda tab yang sedang diseret:
```rust
pub struct Tabular {
    ...
    pub dragged_tab_index: Option<usize>,
    ...
}
```

### 2.3 Operasi Tab Editor (`src/editor.rs`)
Implementasi fungsi-fungsi manipulasi tab:
- `move_tab(tabular, from, to)`: Memindahkan posisi tab secara langsung dengan sinkronisasi `is_pinned` saat melewati batas dan penyesuaian `active_tab_index`.
- `reorder_tab(tabular, from, insert_at)`: Menghitung target indeks dari slot drop penyisipan kursor.
- `pin_tab(tabular, tab_index)`: Menyematkan tab dan memindahkannya ke akhir kelompok pinned.
- `unpin_tab(tabular, tab_index)`: Melepas sematan tab dan memindahkannya ke awal kelompok unpinned.
- `toggle_pin_tab(tabular, tab_index)`: Toggle antara pin dan unpin.
- `close_other_tabs(tabular, keep_index)`: Menutup seluruh tab lain kecuali tab yang dipilih dan semua tab pinned.
- `close_tabs_to_the_right(tabular, tab_index)`: Menutup tab unpinned di sebelah kanan indeks target.

### 2.4 Antarmuka Pengguna & Interaksi (`src/window_egui/app_impl.rs`)
1. **Drag Detection**:
   - Menggunakan `allocate_exact_size` dengan `Sense::click_and_drag()`.
   - Mengaktifkan drag hanya saat pointer primer ditekan (`drag_started_by(PointerButton::Primary)`).
2. **Visual Feedback**:
   - Kursor berubah menjadi `egui::CursorIcon::Grabbing`.
   - Floating ghost badge melayang mengikuti posisi pointer dengan ikon (📌 / 📑) dan judul tab.
   - Garis indikator penyisipan vertikal berwarna aksen tema dengan aksen cap atas & bawah di antara slot drop.
3. **Context Menu**:
   - Menu klik kanan pada setiap tab: Pin/Unpin Tab, Move Left/Right, Close Tab, Close Other Tabs, Close Tabs to the Right.
4. **Proteksi & Tombol Aksi**:
   - Tombol close ("×") digantikan oleh ikon pin ("📌") pada tab pinned.
   - Quick-pin button muncul saat hover pada tab unpinned.
   - Middle-click menutup tab biasa secara cepat tanpa menutup tab pinned.

---

## 3. Rencana Pengujian & Validasi

### 3.1 Unit Test Skenario
1. `test_query_tab_pinning`: Pengujian inisialisasi default `is_pinned`.
2. `test_move_tab_and_active_index`: Pengujian pergeseran `active_tab_index` saat tab aktif dipindah atau tab lain digeser.
3. `test_reorder_tab_with_insert_slots`: Pengujian pemetaan slot penyisipan kursor ke indeks target.
4. `test_pin_and_unpin_tab`: Pengujian transisi status pin dan perpindahan grup tab.
5. `test_pin_tab_shifts_active_index_correctly`: Pengujian pergeseran indeks tab aktif saat tab di kanannya di-pin.
6. `test_unpin_tab_shifts_active_index_correctly`: Pengujian pergeseran indeks tab aktif saat tab di kirinya di-unpin.
7. `test_close_other_tabs_protects_pinned`: Pengujian retensi tab pinned saat operasi close-others dipanggil.
8. `test_close_tabs_to_the_right`: Pengujian retensi tab pinned saat operasi close-tabs-to-right dipanggil.
9. `test_close_tabs_to_the_right_active_tab_switch`: Pengujian pengalihan tab aktif ke target sebelum penutupan tab kanan.
10. `test_move_tab_crossing_pinned_boundary_both_ways`: Pengujian transisi otomatis status pin dua arah saat melintasi batas pemisah.
11. `test_tab_bounds_safety`: Pengujian ketahanan dan ketiadaan panic saat input indeks melebihi batas (out-of-bounds).
