# Rencana Teknis Implementasi: Drag & Drop Tab Editor & Fitur Pin Tab

Dokumen ini menjelaskan rancangan teknis dan arsitektur fitur **Drag & Drop Tab Editor** dan **Pin Tab** pada Tabular, termasuk perbaikan hasil adversarial code review.

---

## 1. Analisis Kebutuhan (Requirements Analysis)

### 1.1 Latar Belakang & Masalah
Saat bekerja dengan banyak tab query SQL, tabel basis data, dan endpoint HTTP, pengguna memerlukan mekanisme untuk:
1. Mengelompokkan tab-tab yang saling terkait secara bebas dengan menggeser/menyeret tab (drag-and-drop).
2. Menyematkan (pin) tab penting agar selalu berada di posisi kiri tab bar dan terlindungi dari penutupan massal atau tidak sengaja.
3. Menyediakan kontrol navigasi tab lengkap melalui menu konteks (klik kanan) dan shortcut mouse (middle-click).

### 1.2 Ruang Lingkup Fitur
1. **Interactive Drag-and-Drop Tab Reordering**:
   - Mendukung penyeretan tab horizontal secara visual.
   - Ghost badge melayang mengikuti posisi kursor dengan ikon status (📌 / 📑) dan judul tab.
   - Indikator garis vertikal interaktif dengan aksen warna tema menandai slot penyisipan tab target.
   - Pembatalan drag secara mulus melalui penekanan tombol `Escape` atau menyeret kursor keluar dari batas vertikal tab bar.
   - Deteksi tombol mouse primer (`PointerButton::Primary`) guna mencegah pemicuan drop prematur dari tombol mouse lain.

2. **Fitur Pin Tab (📌)**:
   - Status `is_pinned: bool` pada setiap objek `QueryTab`.
   - Tab yang disematkan terkumpul rapi di sebelah kiri tab bar sebelum tab-tab biasa (unpinned).
   - Tombol close ("×") digantikan ikon pin ("📌") pada tab yang dipin untuk mencegah penutupan tak sengaja.
   - Tombol quick-pin muncul saat hover pada tab unpinned.
   - Garis pemisah vertikal membedakan grup tab pinned dengan unpinned.
   - Sinkronisasi otomatis saat drag-and-drop: tab biasa yang diseret masuk ke area pinned otomatis menjadi pinned, dan sebaliknya.

3. **Menu Konteks & Aksi Tab Lengkap**:
   - 📌 **Pin Tab** / 📌 **Unpin Tab**
   - ⬅ **Move Tab Left** / ➡ **Move Tab Right**
   - ✕ **Close Tab**
   - **Close Other Tabs** (melindungi tab yang sedang dipin)
   - **Close Tabs to the Right** (melindungi tab yang sedang dipin)
   - Middle-click untuk menutup tab unpinned.

---

## 2. Perubahan Arsitektur & Struktur Berkas

### 2.1 Model Data (`src/models/structs.rs`)
- Menambahkan field `pub is_pinned: bool` pada struct `QueryTab`.
- Default bernilai `false` pada instansiasi tab baru.

### 2.2 Logika Tab Management (`src/editor.rs`)
- `move_tab(tabular: &mut Tabular, from: usize, to: usize)`:
  - Memindahkan posisi tab langsung pada vektor `query_tabs`.
  - Menyesuaikan `active_tab_index` secara matematis tanpa merusak fokus aktif pengguna.
  - Memperbarui status `is_pinned` saat tab melintasi batas pemisah pinned/unpinned.
- `reorder_tab(tabular: &mut Tabular, from: usize, insert_at: usize)`:
  - Mengonversi slot penyisipan UI (0..=n) ke indeks target pemindahan.
- `pin_tab(tabular: &mut Tabular, tab_index: usize)`:
  - Menandai tab sebagai pinned dan memindahkannya ke akhir grup pinned.
  - Menyesuaikan `active_tab_index` secara tepat tanpa dead code branch.
- `unpin_tab(tabular: &mut Tabular, tab_index: usize)`:
  - Melepas pin tab dan memindahkannya ke posisi setelah seluruh tab pinned yang tersisa.
  - Menyesuaikan `active_tab_index` secara tepat tanpa dead code branch.
- `toggle_pin_tab(tabular: &mut Tabular, tab_index: usize)`:
  - Toggle antara `pin_tab` dan `unpin_tab`.
- `close_other_tabs(tabular: &mut Tabular, keep_index: usize)`:
  - Menutup semua tab kecuali tab target dan seluruh tab yang berstatus pinned.
- `close_tabs_to_the_right(tabular: &mut Tabular, tab_index: usize)`:
  - Menutup seluruh tab unpinned di sebelah kanan `tab_index`.

### 2.3 Antarmuka GUI (`src/window_egui/app_impl.rs`)
- Menambahkan field `dragged_tab_index: Option<usize>` pada state `Tabular`.
- Render strip tab dengan alokasi drag & click (`egui::Sense::click_and_drag()`).
- Deteksi pelepasan tombol primer (`PointerButton::Primary`).
- Pengecekan batas vertikal (`is_within_tab_bar_y`) dengan toleransi margin 20px agar pengguna dapat membatalkan drag dengan menggeser pointer keluar tab bar.
- Repaint rendering loop saat mouse dilepaskan untuk menjamin responsivitas UI instan.

---

## 3. Hasil Perbaikan Adversarial Code Review

1. **Pembersihan Dead Code pada `pin_tab`**:
   - Menghilangkan cabang kontradiktif `else if tab_index < first_unpinned` di dalam blok `if tab_index > first_unpinned`.
2. **Pembersihan Dead Code pada `unpin_tab`**:
   - Menghilangkan cabang unreachable `else` di dalam blok `if tab_index < last_p`.
3. **Pembersihan Dead Logic pada `close_tabs_to_the_right`**:
   - Menghapus decrement indeks yang redundan (`tabular.active_tab_index >= i`) yang mustahil terpenuhi.
4. **Pencegahan Premature Drop**:
   - Mengganti `any_released()` dengan `button_released(egui::PointerButton::Primary)`.
5. **Validasi Batas Vertikal**:
   - Menambahkan `is_within_tab_bar_y` pada kalkulasi slot drop `candidate_insert_at`.
