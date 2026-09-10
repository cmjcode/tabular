# Walkthrough: Fitur Drag & Drop Tab Editor dan Pin Tab

Dokumen ini mendokumentasikan implementasi dan verifikasi fitur **Drag & Drop Tab Editor** dan **Pin Tab** pada Tabular, termasuk perbaikan menyeluruh dari adversarial code review.

---

## 1. Ringkasan Fitur

1. **Interactive Drag-and-Drop Tab Reordering**:
   - Pengguna dapat menggeser posisi tab editor secara horizontal.
   - Dilengkapi feedback visual interaktif: ghost badge melayang mengikuti kursor, ikon status (📌 / 📑), serta garis indikator penyisipan vertikal dengan warna aksen tema.
   - Pengguna dapat membatalkan aksi seret tab dengan menekan tombol `Escape` atau menggeser kursor keluar dari batas vertikal tab bar strip.
   - Deteksi tombol mouse primer (`PointerButton::Primary`) memastikan tidak terjadi drop prematur saat tombol mouse lain dilepas.

2. **Fitur Pin Tab (📌)**:
   - Tab penting dapat disematkan (pinned) sehingga selalu berada di sisi kiri tab bar.
   - Mencegah penutupan tab secara tidak sengaja: tombol close ("×") digantikan oleh ikon pin ("📌").
   - Quick-pin button muncul saat hover pada tab unpinned.
   - Garis pembatas visual memisahkan kelompok tab pinned dan unpinned.
   - Sinkronisasi otomatis batasan pinned/unpinned saat tab diseret melintasi batas pemisah.

3. **Menu Konteks Navigasi Lengkap**:
   - Menu klik kanan pada setiap tab:
     - 📌 **Pin Tab** / 📌 **Unpin Tab**
     - ⬅ **Move Tab Left** / ➡ **Move Tab Right**
     - ✕ **Close Tab**
     - **Close Other Tabs** (melindungi tab yang sedang dipin)
     - **Close Tabs to the Right** (melindungi tab yang sedang dipin)
   - Dukungan tombol tengah mouse (middle-click) untuk menutup tab biasa secara cepat.

---

## 2. Perbaikan Berdasarkan Adversarial Code Review

| Masalah | Letak Berkas | Tindakan Perbaikan |
| :--- | :--- | :--- |
| **Dead Code pada `pin_tab`** | `src/editor.rs:355` | Menghapus cabang `else if tab_index < first_unpinned` di dalam blok `if tab_index > first_unpinned`. |
| **Dead Code pada `unpin_tab`** | `src/editor.rs:381` | Menghapus cabang `else` yang tidak terjangkau di dalam blok `if tab_index < last_p`. |
| **Dead Logic pada `close_tabs_to_the_right`** | `src/editor.rs:448` | Menghapus decrement indeks `active_tab_index >= i` yang tidak pernah terjadi karena tab aktif telah berpindah ke `tab_index < i`. |
| **Premature Drop pada `any_released()`** | `src/window_egui/app_impl.rs:2076` | Mengganti ke `inp.pointer.button_released(egui::PointerButton::Primary)`. |
| **Ketiadaan Validasi Batas Vertikal** | `src/window_egui/app_impl.rs:2424` | Menambahkan validasi `is_within_tab_bar_y` sehingga drag dapat dibatalkan jika pointer keluar tab bar. |

---

## 3. Hasil Pengujian Unit

Pengujian unit di `src/editor.rs` dan `src/models/structs.rs` mencakup skenario:
1. `test_query_tab_pinning`: Pengujian properti `is_pinned` pada struct `QueryTab`.
2. `test_move_tab_and_active_index`: Pengujian pemindahan tab dan pembaruan otomatis indeks tab aktif.
3. `test_reorder_tab_with_insert_slots`: Pengujian pemetaan slot drop ke indeks tab.
4. `test_pin_and_unpin_tab`: Pengujian transisi status pin dan pergeseran ke grup tab pinned.
5. `test_pin_tab_shifts_active_index_correctly`: Pengujian pergeseran indeks tab aktif saat tab di sebelah kanan di-pin.
6. `test_unpin_tab_shifts_active_index_correctly`: Pengujian pergeseran indeks tab aktif saat tab di sebelah kiri di-unpin.
7. `test_close_other_tabs_protects_pinned`: Pengujian perlindungan tab pinned dari operasi penutupan tab lainnya.
8. `test_close_tabs_to_the_right`: Pengujian penutupan tab unpinned di sebelah kanan dengan perlindungan tab pinned.
9. `test_close_tabs_to_the_right_active_tab_switch`: Pengujian pengalihan tab aktif ke target sebelum penutupan tab kanan.
