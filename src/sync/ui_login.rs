//! Login / Register UI dialog for tabular cloud sync.
//!
//! Shows login options (Google / GitHub OAuth) and account info when logged in.
//! Integrated into the Settings panel → "Sync & Account" tab.

use eframe::egui;
use crate::window_egui::{Tabular, style};
use super::auth::OAuthProvider;

/// Render the full login panel (call from settings or floating dialog)
pub fn render_login_panel(tabular: &mut Tabular, ui: &mut egui::Ui) {
    match &tabular.sync_account {
        Some(_account) => render_logged_in(tabular, ui),
        None => render_login_form(tabular, ui),
    }
}

fn render_login_form(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.add_space(8.0);

        // Header
        ui.heading("☁  Tabular Cloud Sync");
        ui.add_space(4.0);
        ui.label("Sign in to sync your connections, history, and queries across devices and collaborate in real-time.");
        ui.add_space(12.0);

        // Server URL input
        ui.label("Server URL:");
        let server_url = &mut tabular.sync_server_url;
        let url_resp = ui.add(
            egui::TextEdit::singleline(server_url)
                .hint_text("https://api.tabular.id")
                .desired_width(f32::INFINITY),
        );
        if url_resp.lost_focus() || url_resp.changed() {
            tabular.prefs_dirty = true;
        }
        ui.add_space(12.0);

        // OAuth buttons
        ui.horizontal(|ui| {
            // Google
            let google_btn = style::btn_primary_ctx(
                ui.ctx(),
                "  Sign in with Google  "
            ).min_size(egui::vec2(180.0, 36.0));

            if ui.add(google_btn).clicked() {
                start_oauth(tabular, OAuthProvider::Google);
            }

            ui.add_space(8.0);

            // GitHub
            let github_btn = style::btn_primary_ctx(
                ui.ctx(),
                "  Sign in with GitHub  "
            ).min_size(egui::vec2(180.0, 36.0));

            if ui.add(github_btn).clicked() {
                start_oauth(tabular, OAuthProvider::GitHub);
            }
        });

        ui.add_space(8.0);

        // Pending login / automatic loopback listener / manual fallback
        if tabular.sync_login_pending {
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("🌐 Opening browser... Complete sign-in in your browser.");
            });
            ui.add_space(4.0);

            ui.collapsing("Enter token manually (fallback)", |ui| {
                ui.label("If browser redirect does not complete automatically, copy the token JSON from your browser:");
                ui.add_space(4.0);

                let token_edit = egui::TextEdit::multiline(&mut tabular.sync_token_input)
                    .hint_text("Paste token JSON here: { \"access_token\": \"...\", \"refresh_token\": \"...\" }")
                    .desired_width(f32::INFINITY)
                    .desired_rows(3);
                ui.add(token_edit);

                ui.add_space(4.0);
                if ui.add(style::btn_primary_ctx(ui.ctx(), "✅  Submit Token")).clicked() {
                    try_submit_token(tabular);
                }
            });

            ui.add_space(4.0);
            if ui.add(style::btn_secondary("Cancel")).clicked() {
                tabular.sync_login_pending = false;
                tabular.sync_auth_receiver = None;
                tabular.sync_token_input.clear();
            }
        }

        // Error display
        if let Some(err) = &tabular.sync_login_error.clone() {
            ui.add_space(4.0);
            ui.colored_label(egui::Color32::from_rgb(255, 80, 80), format!("❌ {}", err));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.small("Your connection credentials are encrypted locally before being sent to the server.");
    });
}

fn render_logged_in(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let account = tabular.sync_account.clone().unwrap();

    ui.vertical(|ui| {
        ui.add_space(8.0);
        ui.heading("☁  Tabular Cloud Sync");
        ui.add_space(8.0);

        // User info
        ui.horizontal(|ui| {
            ui.label("✅ Signed in as:");
            ui.strong(&account.email);
        });
        if let Some(name) = &account.display_name {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.label(name);
            });
        }
        ui.add_space(4.0);

        // Sync status
        let status_label = tabular.sync_status.label();
        let status_color = match &tabular.sync_status {
            super::SyncStatus::Synced  => egui::Color32::from_rgb(72, 199, 116),
            super::SyncStatus::Syncing => egui::Color32::from_rgb(255, 213, 0),
            super::SyncStatus::Error(_) => egui::Color32::from_rgb(255, 80, 80),
            super::SyncStatus::Offline => egui::Color32::GRAY,
        };
        ui.horizontal(|ui| {
            ui.label("Sync status:");
            ui.colored_label(status_color, status_label);
        });

        if let super::SyncStatus::Error(e) = &tabular.sync_status {
            ui.colored_label(egui::Color32::from_rgb(255, 80, 80), format!("  {}", e));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Profile (username / phone) — used later to add you to a Team by handle
        ui.label(egui::RichText::new("Profile").strong());
        ui.small("Username and phone let teammates add you to a Team without knowing your email.");
        ui.add_space(4.0);
        egui::Grid::new("profile_fields_grid")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Username:");
                ui.add(
                    egui::TextEdit::singleline(&mut tabular.profile_username_input)
                        .hint_text("e.g. jane_doe")
                        .desired_width(220.0),
                );
                ui.end_row();

                ui.label("Phone:");
                ui.add(
                    egui::TextEdit::singleline(&mut tabular.profile_phone_input)
                        .hint_text("e.g. +6281234567890")
                        .desired_width(220.0),
                );
                ui.end_row();
            });
        ui.add_space(4.0);
        let saving = tabular.profile_update_receiver.is_some();
        ui.add_enabled_ui(!saving, |ui| {
            if ui.add(style::btn_secondary(if saving { "Saving…" } else { "💾  Save Profile" })).clicked() {
                save_profile(tabular);
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Manual sync buttons
        ui.label(egui::RichText::new("Manual Sync").strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add(style::btn_secondary("🔗  Sync Connections")).clicked() {
                tabular.sync_trigger_connections = true;
            }
            if ui.add(style::btn_secondary("📜  Sync History")).clicked() {
                tabular.sync_trigger_history = true;
            }
            if ui.add(style::btn_secondary("💾  Sync Queries")).clicked() {
                tabular.sync_trigger_queries = true;
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // Logout
        if ui.add(style::btn_danger_ctx(ui.ctx(), "🚪  Sign Out")).clicked() {
            do_logout(tabular);
        }
    });
}

// ─── Actions ──────────────────────────────────────────────────────────────────

fn start_oauth(tabular: &mut Tabular, provider: OAuthProvider) {
    if tabular.sync_server_url.trim().is_empty() {
        tabular.sync_login_error = Some("Please enter a server URL first".to_string());
        return;
    }
    tabular.sync_login_error = None;
    tabular.sync_login_pending = true;
    tabular.sync_token_input.clear();

    let rx = super::auth::start_oauth_flow(&tabular.sync_server_url, provider);
    tabular.sync_auth_receiver = Some(rx);
}

fn try_submit_token(tabular: &mut Tabular) {
    let input = tabular.sync_token_input.trim().to_string();

    // Try to parse as full token response JSON from server
    match serde_json::from_str::<serde_json::Value>(&input) {
        Ok(json) => {
            // Support both wrapped API envelope {"success": true, "data": {...}} and raw token JSON
            let root = if json.get("data").is_some_and(|d| d.is_object()) {
                &json["data"]
            } else {
                &json
            };

            let access_token = root["access_token"].as_str().unwrap_or("").to_string();
            let refresh_token = root["refresh_token"].as_str().unwrap_or("").to_string();
            let expires_in = root["expires_in"].as_i64().unwrap_or(3600);
            let user_id = root["user"]["id"].as_str().unwrap_or("").to_string();
            let email = root["user"]["email"].as_str().unwrap_or("").to_string();
            let display_name = root["user"]["display_name"].as_str().map(|s| s.to_string());
            let avatar_url = root["user"]["avatar_url"].as_str().map(|s| s.to_string());
            let username = root["user"]["username"].as_str().map(|s| s.to_string());
            let phone = root["user"]["phone"].as_str().map(|s| s.to_string());

            if access_token.is_empty() || email.is_empty() {
                tabular.sync_login_error = Some("Invalid token JSON — missing access_token or email".to_string());
                return;
            }

            let account = super::TabularAccount {
                user_id,
                email,
                display_name,
                avatar_url,
                username,
                phone,
                access_token,
                refresh_token,
                token_expires_at: chrono::Utc::now().timestamp() + expires_in,
            };

            super::api_client::save_account(&account);
            tabular.sync_account = Some(account);
            tabular.sync_login_pending = false;
            tabular.sync_login_error = None;
            tabular.sync_token_input.clear();
            tabular.sync_status = super::SyncStatus::Synced;
        }
        Err(e) => {
            tabular.sync_login_error = Some(format!("Invalid JSON: {}", e));
        }
    }
}

fn save_profile(tabular: &mut Tabular) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let username = tabular.profile_username_input.trim().to_string();
    let phone = tabular.profile_phone_input.trim().to_string();

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client
            .update_profile(&token, Some(username.as_str()), Some(phone.as_str()))
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    tabular.profile_update_receiver = Some(rx);
}

fn do_logout(tabular: &mut Tabular) {
    if let Some(account) = &tabular.sync_account.clone() {
        let token = account.access_token.clone();
        let refresh = account.refresh_token.clone();
        let server = tabular.sync_server_url.clone();

        // Fire-and-forget logout request
        if let Some(rt) = &tabular.runtime {
            rt.spawn(async move {
                let client = super::api_client::ApiClient::new(&server);
                let _ = client.logout(&refresh, &token).await;
            });
        }
    }

    super::api_client::clear_account();
    tabular.sync_account = None;
    tabular.sync_status = super::SyncStatus::Offline;
    tabular.crdt_state = None;
}
