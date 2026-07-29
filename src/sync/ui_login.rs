/// Login / Register UI dialog for tabular cloud sync.
///
/// Shows login options (Google / GitHub OAuth) and account info when logged in.
/// Integrated into the Settings panel → "Sync & Account" tab.

use eframe::egui;
use crate::window_egui::Tabular;
use super::auth::OAuthProvider;

/// Render the full login panel (call from settings or floating dialog)
pub fn render_login_panel(tabular: &mut Tabular, ui: &mut egui::Ui) {
    match &tabular.sync_account {
        Some(account) => render_logged_in(tabular, ui),
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
        let url_edit = egui::TextEdit::singleline(server_url)
            .hint_text("https://your-tabular-server.com")
            .desired_width(f32::INFINITY);
        ui.add(url_edit);
        ui.add_space(12.0);

        // OAuth buttons
        ui.horizontal(|ui| {
            // Google
            let google_btn = egui::Button::new(
                egui::RichText::new("  Sign in with Google  ").size(14.0)
            )
            .min_size(egui::vec2(180.0, 36.0));

            if ui.add(google_btn).clicked() {
                start_oauth(tabular, OAuthProvider::Google);
            }

            ui.add_space(8.0);

            // GitHub
            let github_btn = egui::Button::new(
                egui::RichText::new("  Sign in with GitHub  ").size(14.0)
            )
            .min_size(egui::vec2(180.0, 36.0));

            if ui.add(github_btn).clicked() {
                start_oauth(tabular, OAuthProvider::GitHub);
            }
        });

        ui.add_space(8.0);

        // Pending login / manual token entry
        if tabular.sync_login_pending {
            ui.separator();
            ui.add_space(4.0);
            ui.label("🌐 Browser opened. After signing in, copy the token JSON from the browser:");
            ui.add_space(4.0);

            let token_edit = egui::TextEdit::multiline(&mut tabular.sync_token_input)
                .hint_text("Paste token JSON here: { \"access_token\": \"...\", \"refresh_token\": \"...\" }")
                .desired_width(f32::INFINITY)
                .desired_rows(4);
            ui.add(token_edit);

            ui.add_space(4.0);
            if ui.button("✅  Submit Token").clicked() {
                try_submit_token(tabular);
            }

            if ui.button("Cancel").clicked() {
                tabular.sync_login_pending = false;
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

        // Manual sync buttons
        ui.label(egui::RichText::new("Manual Sync").strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("🔗  Sync Connections").clicked() {
                tabular.sync_trigger_connections = true;
            }
            if ui.button("📜  Sync History").clicked() {
                tabular.sync_trigger_history = true;
            }
            if ui.button("💾  Sync Queries").clicked() {
                tabular.sync_trigger_queries = true;
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        // Logout
        if ui.button("🚪  Sign Out").clicked() {
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

    let _rx = super::auth::start_oauth_flow(&tabular.sync_server_url, provider);
    // In a future iteration: poll _rx for automatic token capture via local HTTP server
}

fn try_submit_token(tabular: &mut Tabular) {
    let input = tabular.sync_token_input.trim().to_string();

    // Try to parse as full token response JSON from server
    match serde_json::from_str::<serde_json::Value>(&input) {
        Ok(json) => {
            let access_token = json["access_token"].as_str().unwrap_or("").to_string();
            let refresh_token = json["refresh_token"].as_str().unwrap_or("").to_string();
            let expires_in = json["expires_in"].as_i64().unwrap_or(3600);
            let user_id = json["user"]["id"].as_str().unwrap_or("").to_string();
            let email = json["user"]["email"].as_str().unwrap_or("").to_string();
            let display_name = json["user"]["display_name"].as_str().map(|s| s.to_string());

            if access_token.is_empty() || email.is_empty() {
                tabular.sync_login_error = Some("Invalid token JSON — missing access_token or email".to_string());
                return;
            }

            let account = super::TabularAccount {
                user_id,
                email,
                display_name,
                avatar_url: None,
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
