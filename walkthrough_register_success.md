# Walkthrough: Auto-Load Account Information on OAuth Login

Dokumen ini mendokumentasikan hasil perbaikan, implementasi, dan verifikasi fitur **Auto-Load Account Information** sesaat setelah pengguna berhasil login via OAuth (Google / GitHub) di Tabular.

---

## 1. Ringkasan Perbaikan

Sebelum perbaikan ini, ketika pengguna login melalui Google atau GitHub:
1. Otentikasi browser berhasil dan foto avatar pengguna muncul.
2. Namun isian pada bagian **Account Information** (`Display Name`, `Username`, `Phone Number`) tetap kosong.
3. Pengguna harus menutup modal dialog profil dan membukanya kembali agar data akun tersebut muncul.

Setelah perbaikan ini:
1. Begitu callback otentikasi OAuth selesai di latar belakang (`drain_sync_receivers`), seluruh field Account Information (`Display Name`, `Username`, `Phone Number`, dan URL gambar profil) langsung terisi secara otomatis tanpa perlu menutup dan membuka kembali modal dialog.
2. Form buffer dikelola secara terpusat melalui fungsi helper `sync_profile_inputs_from_account(&mut self)`.
3. Input form aman dari *state clobbering* dan *race condition* karena mutasi buffer form hanya dipicu oleh event login/dialog dan tidak diutak-atik saat render loop frame ataupun background token refresh.

---

## 2. Berkas yang Dimodifikasi

| Berkas | Jenis Perubahan | Deskripsi |
| :--- | :--- | :--- |
| `src/window_egui/sync_tick.rs` | Perbaikan Logika & Concurrency | Memanggil `sync_profile_inputs_from_account()` saat event OAuth login sukses; mengisolasi background token refresh agar tidak menimpa buffer formulir aktif pengguna; menambahkan helper `sync_profile_inputs_from_account()`. |
| `src/sync/ui_login.rs` | UI State Management | Mengintegrasikan pemanggilan `sync_profile_inputs_from_account()` pada `open_account_dialog` dan `try_submit_token`; menghapus logika auto-load di dalam per-frame render loop `render_account_profile_view` guna mencegah terkuncinya input saat pengguna mengetik/menghapus teks. |
| `src/window_egui/init.rs` | Sinkronisasi State | Menggunakan `sync_profile_inputs_from_account()` saat memuat akun dari cache SQLite saat inisialisasi aplikasi. |
| `src/window_egui/app_impl.rs` | Sinkronisasi State | Menggunakan `sync_profile_inputs_from_account()` saat akun terdeteksi dari background receiver startup. |
| `src/sync/auth.rs` | Unit Test | Menambahkan unit test `test_parse_poll_completed_with_account_information` dan assertion untuk kelengkapan atribut profil. |
| `test_api.sh` | Integrasi Pengujian | Skrip cURL terotomatisasi untuk memverifikasi endpoint API sinkronisasi tanpa toleransi error palsu `404`. |
| `implementation_plan.md` | Pemulihan Repositori | Dipulihkan dari `origin/main` untuk menjaga riwayat fitur ekspor/impor ZIP. |
| `walkthrough.md` | Pemulihan Repositori | Dipulihkan dari `origin/main` untuk menjaga riwayat fitur ekspor/impor ZIP. |
| `implementation_plan_register_success.md` | Dokumentasi | Rencana teknis implementasi fitur auto-load profil akun. |
| `walkthrough_register_success.md` | Dokumentasi | Berkas laporan verifikasi ini. |

---

## 3. Hasil Pengujian & Verifikasi

### 3.1 Unit Testing (`cargo test --lib -- sync::`)
Pengujian unit test Rust dijalankan di dalam container runner `tabular-test-runner:latest`:
```
running 13 tests
test sync::auth::tests::test_token_to_account_conversion ... ok
test sync::legacy_crypto::tests::unreadable_row_returns_none ... ok
test sync::auth::tests::test_parse_poll_completed_response ... ok
test sync::auth::tests::test_parse_poll_completed_with_account_information ... ok
test sync::legacy_crypto::tests::decrypts_base64_no_op_scheme ... ok
test sync::legacy_crypto::tests::decrypts_sha256_user_id_keyed_scheme ... ok
test sync::vault_crypto::tests::encrypt_decrypt_json_roundtrip ... ok
test sync::vault_crypto::tests::team_key_seal_unseal_roundtrip ... ok
test sync::vault_crypto::tests::wrong_passphrase_fails ... ok
test sync::vault_crypto::tests::wrong_recovery_code_fails ... ok
test sync::vault_crypto::tests::create_and_unlock_roundtrip ... ok
test sync::vault_crypto::tests::recovery_code_unlocks_account_key ... ok
test sync::vault_crypto::tests::team_key_wrong_recipient_fails ... ok

test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 180 filtered out; finished in 3.13s
```
Semua 13 tests lulus 100%.

### 3.2 Endpoint Integration Test (`bash test_api.sh`)
Pengujian integrasi endpoint otentikasi dan akun terhadap server sinkronisasi:
```
======================================================
    Tabular API & Account Sync Integration Tests     
======================================================
Target Server: https://api.tabular.id

[1/5] Checking Server Health...
  [PASS] GET /health (HTTP 200)

[2/5] Testing OAuth Ticket Poll Endpoint...
  [PASS] POST /api/v1/auth/ticket/poll (HTTP 200)

[3/5] Testing Token Refresh Endpoint...
  [PASS] POST /api/v1/auth/refresh (HTTP 401)

[4/5] Testing Profile Update Endpoint...
  [PASS] PUT /api/v1/users/me (HTTP 401)

[5/5] Testing User Search Endpoint...
  [PASS] GET /api/v1/users/search (HTTP 401)

======================================================
Test Summary: 5 Passed, 0 Failed
======================================================
All API endpoint tests succeeded!
```
Semua assertions berstatus PASS dan tidak ada status 404 yang ditoleransi.

---

## 4. Evaluasi Adversarial Critic & Audit Keamanan

1. **Anti-Clobbering / User Input Lock Prevention**:
   - Tidak ada sinkronisasi di dalam `render_account_profile_view`. Pengguna dapat mengedit atau menghapus teks di formulir (misalnya dengan backspace) secara leluasa tanpa khawatir buffer tertimpa ulang secara per-frame.
2. **Concurrency & Race Condition Safety**:
   - Handler refresh token di latar belakang (`sync_refresh_receiver`) tidak menyentuh buffer input form interaktif pengguna, mencegah hilangnya data pengguna yang belum disimpan saat token kadaluwarsa diperbarui di background.
3. **Integritas Repositori**:
   - `walkthrough.md` dan `implementation_plan.md` asli tetap utuh dari `origin/main`.
   - File mode skrip utilitas tidak mengalami perubahan permissions yang tidak diinginkan.
   - Tidak ada polusi dokumen pada `README.md`.
