//! Sync Passphrase setup / unlock UI.
//!
//! This is the user-facing half of `sync::vault_crypto`: a passphrase
//! **separate from the OAuth login**, known only to the user, that unlocks
//! the key which encrypts connections & HTTP client secrets before they
//! ever leave this machine. tabular-server stores the wrapped bundle this
//! screen creates but can never derive the passphrase or the keys from it.
//!
//! Rendered inside Settings → Sync & Account, right below the account info,
//! whenever the user is signed in.

use eframe::egui;
use std::collections::HashMap;

use super::api_client::{ApiClient, PutVaultKeysReq};
use super::vault_crypto::{self, VaultKeyBundle};
use crate::window_egui::{Tabular, style};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum VaultStage {
    /// Haven't asked the server yet whether this account has a vault.
    #[default]
    Unknown,
    Checking,
    /// No vault on the server yet — first-run setup.
    NeedsCreate,
    /// Just created; the recovery code must be shown & acknowledged once.
    ShowRecoveryCode,
    /// A vault exists server-side but isn't unlocked in this session yet.
    Locked,
    /// User clicked "I forgot my passphrase" — show the recovery-code form instead.
    UseRecovery,
    Unlocked,
}

/// Kick off the "does this account have a vault yet?" check. Call once right
/// after a successful login (mirrors the other `sync_trigger_*` flags).
pub fn trigger_vault_check(tabular: &mut Tabular) {
    if tabular.vault_check_receiver.is_some() {
        return; // already in flight
    }
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };
    tabular.vault_stage = VaultStage::Checking;

    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    super::spawn_async(async move {
        let client = ApiClient::new(&server);
        let result = client
            .get_vault_keys(&account.access_token)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    tabular.vault_check_receiver = Some(rx);
}

/// Drain the async receivers. Call from the same per-frame tick that drains
/// the other `sync_*_receiver`s (`window_egui::sync_tick`).
pub fn drain_receivers(tabular: &mut Tabular) {
    drain_touch_id(tabular);
    if let Some(rx) = &tabular.vault_check_receiver
        && let Ok(result) = rx.try_recv()
    {
        tabular.vault_check_receiver = None;
        match result {
            Ok(Some(bundle)) => {
                tabular.vault_remote_bundle = Some(bundle);
                tabular.vault_stage = VaultStage::Locked;
            }
            Ok(None) => {
                tabular.vault_stage = VaultStage::NeedsCreate;
            }
            Err(e) => {
                log::warn!("[vault] Failed to check vault status: {}", e);
                tabular.vault_error = Some(e);
                tabular.vault_stage = VaultStage::Unknown;
            }
        }
    }

    if let Some(rx) = &tabular.vault_upload_receiver
        && let Ok(result) = rx.try_recv()
    {
        tabular.vault_upload_receiver = None;
        if let Err(e) = result {
            log::warn!("[vault] Failed to upload vault bundle: {}", e);
            tabular.vault_error = Some(format!("Failed to save vault to server: {e}"));
        }
    }
}

fn upload_bundle(tabular: &mut Tabular, bundle: VaultKeyBundle) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    super::spawn_async(async move {
        let client = ApiClient::new(&server);
        let req = PutVaultKeysReq::from(&bundle);
        let result = client
            .put_vault_keys(&account.access_token, &req)
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });
    tabular.vault_upload_receiver = Some(rx);
}

fn submit_create(tabular: &mut Tabular) {
    if tabular.vault_passphrase_input != tabular.vault_passphrase_confirm_input {
        tabular.vault_error = Some("Passphrases do not match".to_string());
        return;
    }
    match vault_crypto::create_vault(&tabular.vault_passphrase_input) {
        Ok((unlocked, bundle, recovery_code)) => {
            tabular.vault = Some(unlocked);
            tabular.vault_team_keys = HashMap::new();
            tabular.vault_recovery_code_display = Some(recovery_code);
            tabular.vault_error = None;
            tabular.vault_passphrase_input.clear();
            tabular.vault_passphrase_confirm_input.clear();
            tabular.vault_stage = VaultStage::ShowRecoveryCode;
            upload_bundle(tabular, bundle);
        }
        Err(e) => tabular.vault_error = Some(e),
    }
}

fn submit_unlock(tabular: &mut Tabular) {
    let bundle = match &tabular.vault_remote_bundle {
        Some(b) => b.clone(),
        None => {
            tabular.vault_error = Some("No vault loaded yet".to_string());
            return;
        }
    };
    match vault_crypto::unlock_with_passphrase(&bundle.into(), &tabular.vault_passphrase_input) {
        Ok(unlocked) => {
            tabular.vault = Some(unlocked);
            tabular.vault_error = None;
            remember_passphrase_for_biometrics(tabular);
            tabular.vault_passphrase_input.clear();
            tabular.vault_stage = VaultStage::Unlocked;
            tabular.sync_trigger_connections = true;
        }
        Err(_) => {
            tabular.vault_error = Some("Wrong passphrase".to_string());
            // Passphrase tersimpan sudah basi (diganti di perangkat lain).
            forget_biometric_passphrase(tabular);
        }
    }
}

fn submit_recovery_unlock(tabular: &mut Tabular) {
    let bundle = match &tabular.vault_remote_bundle {
        Some(b) => b.clone(),
        None => return,
    };
    match vault_crypto::unlock_with_recovery_code(
        &bundle.into(),
        &tabular.vault_recovery_code_input,
    ) {
        Ok(unlocked) => {
            tabular.vault_recovery_code_input.clear();
            tabular.vault_error = None;
            // Recovery-code unlock only recovers the AccountKey (see
            // vault_crypto docs) — immediately have the user set a fresh
            // passphrase, re-wrapping under a brand-new KEK + recovery code.
            if !tabular.vault_passphrase_input.is_empty()
                && tabular.vault_passphrase_input == tabular.vault_passphrase_confirm_input
            {
                match vault_crypto::rewrap_with_new_passphrase(
                    &unlocked,
                    &tabular.vault_passphrase_input,
                ) {
                    Ok(new_bundle) => {
                        tabular.vault = Some(unlocked);
                        tabular.vault_team_keys = HashMap::new();
                        remember_passphrase_for_biometrics(tabular);
                        tabular.vault_passphrase_input.clear();
                        tabular.vault_passphrase_confirm_input.clear();
                        tabular.vault_stage = VaultStage::Unlocked;
                        tabular.sync_trigger_connections = true;
                        upload_bundle(tabular, new_bundle);
                    }
                    Err(e) => tabular.vault_error = Some(e),
                }
            } else {
                tabular.vault_error =
                    Some("Recovery code accepted — now set a new Sync Passphrase (both fields above) to finish".to_string());
                tabular.vault = Some(unlocked);
            }
        }
        Err(_) => {
            tabular.vault_error = Some("Invalid recovery code".to_string());
        }
    }
}

pub fn render_vault_panel(tabular: &mut Tabular, ui: &mut egui::Ui) {
    if tabular.sync_account.is_none() {
        return;
    }

    // Judul section digambar oleh pemanggil (Preferences → Cloud Sync).
    crate::window_egui::preferences::hint(
        ui,
        "A Sync Passphrase, separate from your login, encrypts connections and HTTP client secrets before they leave this device. tabular-server only ever stores ciphertext it cannot read.",
    );
    ui.add_space(4.0);

    match tabular.vault_stage.clone() {
        VaultStage::Unknown => {
            trigger_vault_check(tabular);
        }
        VaultStage::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Checking vault status…");
            });
        }
        VaultStage::NeedsCreate => render_create_form(tabular, ui),
        VaultStage::ShowRecoveryCode => render_recovery_code_screen(tabular, ui),
        VaultStage::Locked => render_unlock_form(tabular, ui),
        VaultStage::UseRecovery => render_recovery_unlock_form(tabular, ui),
        VaultStage::Unlocked => {
            let msg = format!(
                "{} Vault unlocked. Sync is end-to-end encrypted.",
                egui_icons::icons::ICON_CHECK.codepoint
            );
            crate::window_egui::preferences::status(
                ui,
                crate::window_egui::preferences::Tone::Success,
                msg,
            );
            render_biometric_toggle(tabular, ui);
        }
    }

    if let Some(err) = tabular.vault_error.clone() {
        ui.add_space(4.0);
        let err_msg = format!("{} {}", egui_icons::icons::ICON_CLOSE.codepoint, err);
        crate::window_egui::preferences::status(
            ui,
            crate::window_egui::preferences::Tone::Danger,
            err_msg,
        );
    }
}

fn render_create_form(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.label("Create a Sync Passphrase to protect your synced data:");
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(&mut tabular.vault_passphrase_input)
            .password(true)
            .hint_text("At least 8 characters"),
        280.0,
        None,
    );
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(&mut tabular.vault_passphrase_confirm_input)
            .password(true)
            .hint_text("Confirm passphrase"),
        280.0,
        None,
    );
    ui.add_space(4.0);
    ui.small("⚠ We cannot recover this for you. You'll get a one-time recovery code after this step — save it somewhere safe.");
    ui.add_space(6.0);
    if ui
        .add(style::btn_primary_ctx(ui.ctx(), "🔐  Create Vault"))
        .clicked()
    {
        submit_create(tabular);
    }
}

fn render_recovery_code_screen(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let code = tabular
        .vault_recovery_code_display
        .clone()
        .unwrap_or_default();
    ui.colored_label(
        egui::Color32::from_rgb(255, 193, 7),
        "⚠ Save this recovery code now — it will not be shown again:",
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.monospace(&code);
        if ui.small_button("📋 Copy").clicked() {
            ui.ctx().copy_text(code.clone());
        }
    });
    ui.add_space(6.0);
    ui.checkbox(
        &mut tabular.vault_recovery_code_saved_confirmed,
        "I have saved this recovery code somewhere safe",
    );
    ui.add_space(4.0);
    ui.add_enabled_ui(tabular.vault_recovery_code_saved_confirmed, |ui| {
        if ui
            .add(style::btn_primary_ctx(ui.ctx(), "Continue"))
            .clicked()
        {
            tabular.vault_recovery_code_display = None;
            tabular.vault_recovery_code_saved_confirmed = false;
            tabular.vault_stage = VaultStage::Unlocked;
        }
    });
}

fn render_unlock_form(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.label("Enter your Sync Passphrase to unlock end-to-end encrypted sync on this device:");
    let resp = style::render_text_field(
        ui,
        egui::TextEdit::singleline(&mut tabular.vault_passphrase_input)
            .password(true)
            .hint_text("Sync Passphrase"),
        280.0,
        None,
    );
    let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add(style::btn_primary_ctx(ui.ctx(), "🔓  Unlock"))
            .clicked()
            || submit
        {
            submit_unlock(tabular);
        }
        if ui.add(style::btn_secondary("Forgot passphrase?")).clicked() {
            tabular.vault_error = None;
            tabular.vault_stage = VaultStage::UseRecovery;
        }
        render_touch_id_button(tabular, ui);
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Touch ID (M7)
//
// Passphrase disimpan di OS keychain (lewat `secrets.rs`) hanya bila pengguna
// mengaktifkan opsi ini, lalu dibaca kembali setelah LocalAuthentication
// berhasil. Catatan keamanan: item keychain tidak diikat ke biometrik
// (crate `keyring` tidak mendukung access control), jadi Touch ID berfungsi
// sebagai gerbang di Tabular; perlindungan item tetap dari login keychain.
// ─────────────────────────────────────────────────────────────────────────────

fn biometric_secret_name(tabular: &Tabular) -> Option<String> {
    tabular
        .sync_account
        .as_ref()
        .map(|a| format!("vault:passphrase:{}", a.user_id))
}

/// Touch ID tersedia di perangkat ini (hasil di-cache per sesi).
fn biometric_supported(tabular: &mut Tabular) -> bool {
    #[cfg(target_os = "macos")]
    {
        *tabular
            .platform_ui
            .touch_id_available
            .get_or_insert_with(crate::platform_macos::biometric_available)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = tabular;
        false
    }
}

fn biometric_enabled(tabular: &mut Tabular) -> bool {
    crate::platform_prefs::current().biometric_unlock && biometric_supported(tabular)
}

fn remember_passphrase_for_biometrics(tabular: &mut Tabular) {
    if !biometric_enabled(tabular) || tabular.vault_passphrase_input.is_empty() {
        return;
    }
    if let Some(name) = biometric_secret_name(tabular) {
        let ok = crate::secrets::set_secret(&name, &tabular.vault_passphrase_input);
        if !ok {
            log::warn!("[vault] could not store passphrase for Touch ID");
        }
        tabular.platform_ui.touch_id_secret_saved = Some(ok);
    }
}

fn forget_biometric_passphrase(tabular: &mut Tabular) {
    if let Some(name) = biometric_secret_name(tabular) {
        crate::secrets::delete_secret(&name);
    }
    tabular.platform_ui.touch_id_secret_saved = Some(false);
}

fn biometric_secret_saved(tabular: &mut Tabular) -> bool {
    if let Some(saved) = tabular.platform_ui.touch_id_secret_saved {
        return saved;
    }
    let saved = biometric_secret_name(tabular)
        .and_then(|n| crate::secrets::get_secret(&n))
        .is_some();
    tabular.platform_ui.touch_id_secret_saved = Some(saved);
    saved
}

fn render_touch_id_button(tabular: &mut Tabular, ui: &mut egui::Ui) {
    if !biometric_enabled(tabular) {
        return;
    }
    if !biometric_secret_saved(tabular) {
        return;
    }
    let waiting = tabular.platform_ui.touch_id_rx.is_some();
    let label = format!(
        "{}  {}",
        egui_icons::icons::ICON_FINGERPRINT.codepoint,
        crate::i18n::tr("Unlock with Touch ID")
    );
    if ui
        .add_enabled(!waiting, style::btn_secondary(label))
        .clicked()
    {
        #[cfg(target_os = "macos")]
        {
            tabular.platform_ui.touch_id_rx = Some(crate::platform_macos::authenticate(
                "unlock your Tabular sync vault",
            ));
        }
    }
    if waiting {
        ui.spinner();
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }
}

/// Terima hasil Touch ID lalu buka vault dengan passphrase tersimpan.
fn drain_touch_id(tabular: &mut Tabular) {
    let Some(rx) = &tabular.platform_ui.touch_id_rx else {
        return;
    };
    let result = match rx.try_recv() {
        Ok(r) => r,
        Err(std::sync::mpsc::TryRecvError::Empty) => return,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            Err("Touch ID did not respond".to_string())
        }
    };
    tabular.platform_ui.touch_id_rx = None;
    match result {
        Ok(()) => {
            let stored =
                biometric_secret_name(tabular).and_then(|n| crate::secrets::get_secret(&n));
            match stored {
                Some(passphrase) => {
                    tabular.vault_passphrase_input = passphrase;
                    submit_unlock(tabular);
                }
                None => {
                    tabular.vault_error =
                        Some("No saved passphrase; unlock once with your passphrase".into())
                }
            }
        }
        Err(e) => tabular.vault_error = Some(e),
    }
}

/// Toggle Touch ID saat vault terbuka (hanya bila perangkat mendukung).
fn render_biometric_toggle(tabular: &mut Tabular, ui: &mut egui::Ui) {
    if !biometric_supported(tabular) {
        return;
    }
    let mut on = crate::platform_prefs::current().biometric_unlock;
    ui.add_space(6.0);
    if crate::window_egui::preferences::toggle_row(
        ui,
        &mut on,
        crate::i18n::tr("Offer Touch ID to unlock the vault"),
        Some(crate::i18n::tr(
            "Stores your Sync Passphrase in the macOS Keychain. Touch ID is asked before Tabular reads it.",
        )),
    ) {
        crate::platform_prefs::update(|p| p.biometric_unlock = on);
        crate::platform_prefs::persist();
        if !on {
            forget_biometric_passphrase(tabular);
        }
    }
    if on && !biometric_secret_saved(tabular) {
        crate::window_egui::preferences::hint(
            ui,
            "Your passphrase is saved for Touch ID the next time you unlock with it.",
        );
    }
}

fn render_recovery_unlock_form(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.label("Enter your recovery code, then set a new Sync Passphrase:");
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(&mut tabular.vault_recovery_code_input)
            .hint_text("Recovery code"),
        340.0,
        None,
    );
    ui.add_space(4.0);
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(&mut tabular.vault_passphrase_input)
            .password(true)
            .hint_text("New Sync Passphrase"),
        280.0,
        None,
    );
    style::render_text_field(
        ui,
        egui::TextEdit::singleline(&mut tabular.vault_passphrase_confirm_input)
            .password(true)
            .hint_text("Confirm new passphrase"),
        280.0,
        None,
    );
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui
            .add(style::btn_primary_ctx(ui.ctx(), "Recover & Reset"))
            .clicked()
        {
            submit_recovery_unlock(tabular);
        }
        if ui.add(style::btn_secondary("Back")).clicked() {
            tabular.vault_error = None;
            tabular.vault_stage = VaultStage::Locked;
        }
    });
    ui.add_space(4.0);
    ui.small("Recovering only restores access to your own data — Team-shared items stay locked until you rejoin/re-share.");
}

/// Buka popup unlock vault. Dipakai oleh aksi sync yang butuh vault terbuka,
/// supaya user tidak perlu mencari form passphrase di Preferences.
pub fn open_vault_unlock_dialog(tabular: &mut Tabular) {
    tabular.vault_error = None;
    tabular.vault_passphrase_input.clear();
    tabular.show_vault_unlock_dialog = true;
}

/// Popup "Unlock Vault". Isinya sama dengan panel vault di Preferences
/// (unlock, buat vault baru, atau recovery), lalu menutup sendiri begitu
/// vault terbuka.
pub fn render_vault_unlock_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_vault_unlock_dialog {
        return;
    }
    if tabular.sync_account.is_none() {
        // Logout dari tempat lain saat popup masih terbuka.
        tabular.show_vault_unlock_dialog = false;
        return;
    }

    let mut close = false;
    style::render_modal_backdrop(ctx, "vault_unlock_backdrop", true);

    egui::Window::new("Unlock Vault")
        .title_bar(false)
        .frame(style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_min_width(400.0);
            style::render_modal_header(ui, "Unlock Vault to Sync", &mut close);
            ui.add_space(8.0);
            render_vault_panel(tabular, ui);
        });

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    // Tahap ShowRecoveryCode juga sudah punya vault, tapi kode recovery harus
    // dikonfirmasi dulu; tunggu sampai stage benar-benar Unlocked.
    let unlocked = tabular.vault.is_some() && tabular.vault_stage == VaultStage::Unlocked;
    if unlocked {
        tabular.show_vault_unlock_dialog = false;
        tabular.toasts.success("Vault unlocked");
    } else if close {
        tabular.show_vault_unlock_dialog = false;
        tabular.vault_passphrase_input.clear();
    }
}
