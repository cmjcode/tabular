# Implementation Plan: Auto-Load Account Information on OAuth Login

Dokumen ini menjelaskan rencana teknis perbaikan dan implementasi pemuatan otomatis data **Account Information** (`display_name`, `username`, `phone`, `avatar_url`) sesaat setelah pengguna berhasil login melalui penyedia OAuth (Google / GitHub).

---

## 1. Analisis Masalah (Problem Statement)

### 1.1 Perilaku Sebelum Perbaikan
1. Pengguna membuka modal dialog akun (`👤 Account & Profile`) di Tabular.
2. Pengguna memilih login via OAuth: "Sign in with Google" atau "Sign in with GitHub".
3. Browser sistem terbuka, pengguna menyelesaikan otentikasi OAuth, dan token berhasil diproses oleh Tabular melalui ticket polling di latar belakang.
4. Tabular menerima payload token dan user profile (`TokenResponse` -> `RemoteUser`), lalu menyimpan kredensial ke `sync_account`.
5. Foto profil pengguna langsung muncul di avatar, namun formulir **Account Information** (Display Name, Username, Phone Number) tetap kosong.
6. Data formulir baru muncul jika pengguna menutup modal dialog (`show_account_dialog = false`) kemudian membukanya kembali dari menu/sidebar.

### 1.2 Akar Masalah Teknis
- Pengisian buffer formulir UI (`profile_display_name_input`, `profile_avatar_url_input`, `profile_username_input`, `profile_phone_input`) sebelumnya hanya dipanggil secara pasif di dalam fungsi `open_account_dialog()`.
- Karena modal dialog sudah berada dalam keadaan terbuka (`show_account_dialog = true`) saat proses OAuth dimulai dan selesai, fungsi `open_account_dialog()` tidak pernah dipanggil kembali saat transisi dari `render_account_login_view` ke `render_account_profile_view`.
- Handler OAuth di `drain_sync_receivers()` (`src/window_egui/sync_tick.rs`) hanya memperbarui `self.sync_account` tanpa menyinkronkan buffer string formulir UI.

---

## 2. Desain Solusi & Rencana Perubahan

### 2.1 Sinkronisasi Terpusat (`sync_profile_inputs_from_account`)
Tambahkan method utilitas pada struct `Tabular`:
```rust
impl super::Tabular {
    pub fn sync_profile_inputs_from_account(&mut self) {
        if let Some(account) = &self.sync_account {
            self.profile_display_name_input = account.display_name.clone().unwrap_or_default();
            self.profile_avatar_url_input = account.avatar_url.clone().unwrap_or_default();
            self.profile_username_input = account.username.clone().unwrap_or_default();
            self.profile_phone_input = account.phone.clone().unwrap_or_default();
            if self.avatar_texture_url != account.avatar_url {
                self.avatar_texture = None;
                self.avatar_texture_url = None;
            }
        } else {
            self.profile_display_name_input.clear();
            self.profile_avatar_url_input.clear();
            self.profile_username_input.clear();
            self.profile_phone_input.clear();
            self.avatar_texture = None;
            self.avatar_texture_url = None;
        }
    }
}
```

### 2.2 Integrasi Titik Panggilan Event-Driven
Panggil `sync_profile_inputs_from_account()` pada setiap transisi status akun diskrit:
1. **OAuth Login Success** (`src/window_egui/sync_tick.rs:360`): Sesaat setelah token OAuth diterima dan disimpan.
2. **Manual Token Submission** (`src/sync/ui_login.rs:649`): Saat pengguna menempel token JSON secara manual.
3. **Profile Save Success** (`src/window_egui/sync_tick.rs:60`): Setelah API response sukses memperbarui profil pengguna.
4. **App Initialization & Background Load** (`src/window_egui/init.rs` dan `src/window_egui/app_impl.rs`): Saat akun dimuat dari cache SQLite lokal saat startup.
5. **Open Dialog** (`src/sync/ui_login.rs:130`): Saat dialog dibuka kembali.

### 2.3 Pencegahan Masalah State Clobbering & Concurrency Safety
- **Tidak Memutasi State di Render Loop**: Jangan pernah melakukan auto-load di dalam fungsi render `render_account_profile_view` (egui per-frame render loop) untuk mencegah penimpaan input pengguna yang sedang mengetik atau menghapus teks.
- **Isolasi Background Token Refresh**: Pada callback refresh token otomatis di latar belakang, jangan mengubah buffer input formulir UI `profile_*_input` pengguna; hanya perbarui `self.sync_account` dan invalidasi cache avatar jika avatar URL berubah di server.
- **Zero Tolerance API Error**: Skrip pengujian integrasi `test_api.sh` menolak kode status 404 (Not Found) untuk menghindari kegagalan diam-diam (*silent failure*).

---

## 3. Rencana Verifikasi

1. **Unit Testing**:
   - Menjalankan unit tests parser autentikasi: `test_token_to_account_conversion`, `test_parse_poll_completed_response`, dan `test_parse_poll_completed_with_account_information`.
2. **Kompilasi & Test Suite**:
   - Memastikan `cargo test --lib -- sync::` berhasil 100% tanpa error.
3. **API Integration Test**:
   - Menjalankan `bash test_api.sh` untuk memastikan seluruh rute endpoint otentikasi dan profil merespons dengan kode status yang tepat.
