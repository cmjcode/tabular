//! Account and Cloud Sync UI components.
//!
//! Provides:
//! 1. `render_sync_panel`: Cloud Sync settings tab inside Preferences modal.
//! 2. `render_account_dialog`: Dedicated modal popup for account management, login/logout, and profile photo settings.
//! 3. `draw_circular_avatar`: Helper to render circular user avatars with image texture or initials fallback.

use eframe::egui;
use crate::window_egui::{Tabular, style};
use super::auth::OAuthProvider;

/// Draw a circular avatar with either the loaded image texture or initials / user icon fallback.
pub fn draw_circular_avatar(
    ui: &mut egui::Ui,
    tabular: &Tabular,
    size: f32,
    email: &str,
    display_name: Option<&str>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
    let center = rect.center();
    let radius = size / 2.0;

    let dark = ui.visuals().dark_mode;

    if let Some(ref texture) = tabular.avatar_texture {
        // Draw circular image
        let corner_radius = egui::CornerRadius::same(radius as u8);
        ui.painter().rect_filled(rect, corner_radius, egui::Color32::BLACK);
        ui.painter().image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        // Subtle circular border
        let stroke_color = if resp.hovered() {
            style::theme_accent(ui.ctx())
        } else if dark {
            egui::Color32::from_rgb(70, 75, 90)
        } else {
            egui::Color32::from_rgb(200, 205, 215)
        };
        ui.painter().circle_stroke(center, radius - 0.5, egui::Stroke::new(1.5, stroke_color));
    } else {
        // Fallback: draw circular badge with initial letter or user icon
        let bg_color = if resp.hovered() {
            style::theme_accent(ui.ctx())
        } else if dark {
            egui::Color32::from_rgb(55, 65, 85)
        } else {
            egui::Color32::from_rgb(215, 225, 240)
        };

        ui.painter().circle_filled(center, radius, bg_color);
        let stroke_color = if resp.hovered() {
            egui::Color32::WHITE
        } else if dark {
            egui::Color32::from_rgb(75, 85, 110)
        } else {
            egui::Color32::from_rgb(185, 195, 215)
        };
        ui.painter().circle_stroke(center, radius - 0.5, egui::Stroke::new(1.0, stroke_color));

        // Initial character
        let initial = display_name
            .and_then(|n| n.trim().chars().next())
            .or_else(|| email.trim().chars().next())
            .unwrap_or('U')
            .to_uppercase()
            .to_string();

        let font_size = (size * 0.46).max(11.0);
        let text_color = if dark {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(30, 40, 60)
        };

        ui.painter().text(
            center,
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(font_size),
            text_color,
        );
    }

    resp
}

/// Open and sync inputs for the dedicated Account Dialog.
pub fn open_account_dialog(tabular: &mut Tabular) {
    if let Some(account) = &tabular.sync_account {
        tabular.profile_display_name_input = account.display_name.clone().unwrap_or_default();
        tabular.profile_avatar_url_input = account.avatar_url.clone().unwrap_or_default();
        tabular.profile_username_input = account.username.clone().unwrap_or_default();
        tabular.profile_phone_input = account.phone.clone().unwrap_or_default();
    }
    tabular.show_account_dialog = true;
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Settings Panel Tab: Cloud Sync (render_sync_panel)
// ─────────────────────────────────────────────────────────────────────────────

/// Render the Cloud Sync panel inside the Settings / Preferences modal.
pub fn render_sync_panel(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.add_space(6.0);
        ui.heading("☁ Cloud Synchronization");
        ui.add_space(4.0);
        ui.label("Synchronize database connections, query history, and collaborate securely across devices.");
        ui.add_space(10.0);

        // Account status card
        let dark = ui.visuals().dark_mode;
        let card_bg = if dark {
            egui::Color32::from_rgb(32, 34, 42)
        } else {
            egui::Color32::from_rgb(245, 247, 250)
        };
        let card_stroke = if dark {
            egui::Color32::from_rgb(52, 56, 68)
        } else {
            egui::Color32::from_rgb(220, 224, 232)
        };

        egui::Frame::new()
            .fill(card_bg)
            .stroke(egui::Stroke::new(1.0, card_stroke))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(ref account) = tabular.sync_account {
                        draw_circular_avatar(
                            ui,
                            tabular,
                            36.0,
                            &account.email,
                            account.display_name.as_deref(),
                        );
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            let name = account.display_name.as_deref().unwrap_or(&account.email);
                            ui.label(egui::RichText::new(name).strong().size(13.5));
                            ui.label(egui::RichText::new(&account.email).color(ui.visuals().weak_text_color()).size(11.5));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(style::btn_primary_ctx(ui.ctx(), "👤 Manage Account")).clicked() {
                                open_account_dialog(tabular);
                            }
                        });
                    } else {
                        ui.label(egui::RichText::new("⚙").size(24.0));
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            ui.label(egui::RichText::new("Offline Mode (No Account)").strong().size(13.0));
                            ui.label(egui::RichText::new("Tabular works fully offline. Sign in to enable cloud sync.").color(ui.visuals().weak_text_color()).size(11.5));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(style::btn_primary_ctx(ui.ctx(), "👤 Sign In / Create Account")).clicked() {
                                open_account_dialog(tabular);
                            }
                        });
                    }
                });
            });

        ui.add_space(10.0);

        // Server URL input
        ui.label(egui::RichText::new("Sync Server URL:").strong());
        let server_url = &mut tabular.sync_server_url;
        let url_resp = ui.add(
            egui::TextEdit::singleline(server_url)
                .hint_text("https://api.tabular.id")
                .desired_width(f32::INFINITY),
        );
        if url_resp.lost_focus() || url_resp.changed() {
            tabular.prefs_dirty = true;
        }
        if !tabular.sync_server_url.trim().is_empty() && !is_server_url_acceptable(&tabular.sync_server_url) {
            ui.colored_label(
                egui::Color32::from_rgb(255, 193, 7),
                "⚠ Use https:// — plain http:// is only accepted for localhost",
            );
        }

        ui.add_space(8.0);

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

        if tabular.sync_account.is_some() {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // Manual sync buttons
            ui.label(egui::RichText::new("Manual Sync Actions").strong());
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add(style::btn_secondary("🔗  Sync Connections")).clicked() {
                    tabular.sync_trigger_connections = true;
                }
                if ui.add(style::btn_secondary("📜  Sync History")).clicked() {
                    tabular.sync_trigger_history = true;
                }
                if ui.add(style::btn_secondary("💾  Sync Queries")).clicked() {
                    tabular.sync_trigger_queries = true;
                }
                if ui.add(style::btn_secondary("🌐  Sync HTTP Requests")).clicked() {
                    tabular.sync_trigger_http = true;
                }
            });

            super::ui_vault_setup::render_vault_panel(tabular, ui);
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Dedicated Account & Profile Modal Dialog (render_account_dialog)
// ─────────────────────────────────────────────────────────────────────────────

/// Render the dedicated Account & Profile dialog.
pub fn render_account_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_account_dialog {
        return;
    }

    style::render_modal_backdrop(ctx, "account_dialog", tabular.show_account_dialog);

    let mut open_flag = true;
    let screen_rect = ctx.content_rect();
    let max_dialog_w = (screen_rect.width() - 32.0).min(520.0).max(360.0);
    let max_dialog_h = (screen_rect.height() - 40.0).min(640.0).max(320.0);

    egui::Window::new("👤 Account & Profile")
        .open(&mut open_flag)
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .fixed_pos(screen_rect.center())
        .max_width(max_dialog_w)
        .default_width(max_dialog_w)
        .max_height(max_dialog_h)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("account_dialog_scroll")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if tabular.sync_account.is_some() {
                        render_account_profile_view(tabular, ui);
                    } else {
                        render_account_login_view(tabular, ui);
                    }
                });
        });

    if !open_flag {
        tabular.show_account_dialog = false;
    }
}

/// Render logged-in account profile view with picture setter and form fields.
fn render_account_profile_view(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    ui.vertical(|ui| {
        ui.add_space(4.0);

        // Header with Avatar & Details
        ui.horizontal(|ui| {
            draw_circular_avatar(
                ui,
                tabular,
                64.0,
                &account.email,
                if tabular.profile_display_name_input.is_empty() {
                    account.display_name.as_deref()
                } else {
                    Some(&tabular.profile_display_name_input)
                },
            );

            ui.add_space(12.0);
            ui.vertical(|ui| {
                let name = if !tabular.profile_display_name_input.is_empty() {
                    &tabular.profile_display_name_input
                } else {
                    account.display_name.as_deref().unwrap_or(&account.email)
                };
                ui.label(egui::RichText::new(name).strong().size(16.0));
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&account.email).color(ui.visuals().weak_text_color()).size(12.0));
                    ui.label(egui::RichText::new("✓ Verified").color(egui::Color32::from_rgb(0, 180, 80)).size(11.0));
                });
                if let Some(ref handle) = account.username {
                    if !handle.is_empty() {
                        ui.label(egui::RichText::new(format!("@{}", handle)).color(style::theme_accent(ui.ctx())).size(12.0));
                    }
                }
            });
        });

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(8.0);

        // Profile Picture Configuration
        ui.label(egui::RichText::new("Profile Picture").strong().size(13.0));
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            let avatar_edit = ui.add(
                egui::TextEdit::singleline(&mut tabular.profile_avatar_url_input)
                    .hint_text("Image URL or select local file")
                    .desired_width(ui.available_width() - 170.0),
            );
            if avatar_edit.changed() {
                // Invalidate cached texture so it reloads
                tabular.avatar_texture = None;
                tabular.avatar_texture_url = None;
            }

            if ui.add(style::btn_secondary("📁 Choose File")).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif"])
                    .pick_file()
                {
                    if let Ok(bytes) = std::fs::read(&path) {
                        use base64::Engine;
                        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
                        let mime = match ext.to_lowercase().as_str() {
                            "jpg" | "jpeg" => "image/jpeg",
                            "webp" => "image/webp",
                            "gif" => "image/gif",
                            _ => "image/png",
                        };
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        tabular.profile_avatar_url_input = format!("data:{};base64,{}", mime, b64);
                        tabular.avatar_texture = None;
                        tabular.avatar_texture_url = None;
                    }
                }
            }

            if !tabular.profile_avatar_url_input.is_empty() {
                if ui.button("🗑").on_hover_text("Clear Photo").clicked() {
                    tabular.profile_avatar_url_input.clear();
                    tabular.avatar_texture = None;
                    tabular.avatar_texture_url = None;
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // Account Information Form
        ui.label(egui::RichText::new("Account Information").strong().size(13.0));
        ui.add_space(4.0);

        egui::Grid::new("account_info_form_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Display Name:");
                ui.add(
                    egui::TextEdit::singleline(&mut tabular.profile_display_name_input)
                        .hint_text("e.g. John Doe")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();

                ui.label("Username:");
                ui.add(
                    egui::TextEdit::singleline(&mut tabular.profile_username_input)
                        .hint_text("e.g. johndoe (used for team invites)")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();

                ui.label("Phone Number:");
                ui.add(
                    egui::TextEdit::singleline(&mut tabular.profile_phone_input)
                        .hint_text("e.g. +6281234567890")
                        .desired_width(f32::INFINITY),
                );
                ui.end_row();

                ui.label("Email:");
                ui.label(egui::RichText::new(&account.email).color(ui.visuals().weak_text_color()));
                ui.end_row();

                ui.label("User ID:");
                ui.label(egui::RichText::new(&account.user_id).color(ui.visuals().weak_text_color()).size(11.0));
                ui.end_row();
            });

        ui.add_space(14.0);

        // Action Buttons
        let saving = tabular.profile_update_receiver.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!saving, |ui| {
                if ui.add(style::btn_primary_ctx(ui.ctx(), if saving { "💾  Saving Changes…" } else { "💾  Save Changes" })).clicked() {
                    save_profile(tabular);
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(style::btn_danger_ctx(ui.ctx(), "🚪  Sign Out")).clicked() {
                    do_logout(tabular);
                }
            });
        });

        ui.add_space(6.0);
    });
}

/// Render logged-out login / create account view.
fn render_account_login_view(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.add_space(6.0);
        ui.heading("👤 Sign In to Tabular");
        ui.add_space(4.0);
        ui.label("Connect your account to sync connections, queries, and collaborate in real-time.");
        ui.small("💡 Note: An account is completely optional. Tabular is offline-first and fully functional without login.");
        ui.add_space(12.0);

        // Server URL input
        ui.label(egui::RichText::new("Server URL:").strong());
        let server_url = &mut tabular.sync_server_url;
        let url_resp = ui.add(
            egui::TextEdit::singleline(server_url)
                .hint_text("https://api.tabular.id")
                .desired_width(f32::INFINITY),
        );
        if url_resp.lost_focus() || url_resp.changed() {
            tabular.prefs_dirty = true;
        }
        if !tabular.sync_server_url.trim().is_empty() && !is_server_url_acceptable(&tabular.sync_server_url) {
            ui.colored_label(
                egui::Color32::from_rgb(255, 193, 7),
                "⚠ Use https:// — plain http:// is only accepted for localhost",
            );
        }
        ui.add_space(12.0);

        // OAuth buttons
        ui.horizontal(|ui| {
            let google_btn = style::btn_primary_ctx(
                ui.ctx(),
                "  Sign in with Google  "
            ).min_size(egui::vec2(160.0, 36.0));

            if ui.add(google_btn).clicked() {
                start_oauth(tabular, OAuthProvider::Google);
            }

            ui.add_space(8.0);

            let github_btn = style::btn_primary_ctx(
                ui.ctx(),
                "  Sign in with GitHub  "
            ).min_size(egui::vec2(160.0, 36.0));

            if ui.add(github_btn).clicked() {
                start_oauth(tabular, OAuthProvider::GitHub);
            }
        });

        ui.add_space(8.0);

        // Pending login status / Manual fallback
        if tabular.sync_login_pending {
            ui.separator();
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("🌐 Opening browser... Complete sign-in in your browser.");
            });
            ui.add_space(4.0);

            ui.collapsing("Enter token manually (fallback)", |ui| {
                ui.label("If browser redirect does not complete automatically, paste the token JSON:");
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

        ui.add_space(10.0);
        ui.separator();
        ui.small("Your connection credentials remain encrypted locally before being sent to the server.");
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Actions & Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn is_server_url_acceptable(url: &str) -> bool {
    let trimmed = url.trim();
    if let Some(rest) = trimmed.strip_prefix("https://") {
        return !rest.is_empty();
    }
    if let Some(rest) = trimmed.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        return host == "localhost" || host == "127.0.0.1" || host == "::1";
    }
    false
}

fn start_oauth(tabular: &mut Tabular, provider: OAuthProvider) {
    if tabular.sync_server_url.trim().is_empty() {
        tabular.sync_login_error = Some("Please enter a server URL first".to_string());
        return;
    }
    if !is_server_url_acceptable(&tabular.sync_server_url) {
        tabular.sync_login_error = Some(
            "Server URL must use https:// (plain http:// is only allowed for localhost/127.0.0.1)".to_string(),
        );
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

    match serde_json::from_str::<serde_json::Value>(&input) {
        Ok(json) => {
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
                display_name: display_name.clone(),
                avatar_url: avatar_url.clone(),
                username: username.clone(),
                phone: phone.clone(),
                access_token,
                refresh_token,
                token_expires_at: chrono::Utc::now().timestamp() + expires_in,
            };

            super::api_client::save_account(&account);
            tabular.profile_display_name_input = display_name.unwrap_or_default();
            tabular.profile_avatar_url_input = avatar_url.unwrap_or_default();
            tabular.profile_username_input = username.unwrap_or_default();
            tabular.profile_phone_input = phone.unwrap_or_default();
            tabular.avatar_texture = None;
            tabular.avatar_texture_url = None;

            tabular.sync_account = Some(account);
            tabular.sync_login_pending = false;
            tabular.sync_login_error = None;
            tabular.sync_token_input.clear();
            tabular.sync_status = super::SyncStatus::Synced;
            super::ui_vault_setup::trigger_vault_check(tabular);
        }
        Err(e) => {
            tabular.sync_login_error = Some(format!("Invalid JSON: {}", e));
        }
    }
}

pub fn save_profile(tabular: &mut Tabular) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let display_name = tabular.profile_display_name_input.trim().to_string();
    let avatar_url = tabular.profile_avatar_url_input.trim().to_string();
    let username = tabular.profile_username_input.trim().to_string();
    let phone = tabular.profile_phone_input.trim().to_string();

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client
            .update_profile(
                &token,
                Some(display_name.as_str()),
                Some(avatar_url.as_str()),
                Some(username.as_str()),
                Some(phone.as_str()),
            )
            .await
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    tabular.profile_update_receiver = Some(rx);
}

pub fn do_logout(tabular: &mut Tabular) {
    if let Some(account) = &tabular.sync_account.clone() {
        let token = account.access_token.clone();
        let refresh = account.refresh_token.clone();
        let server = tabular.sync_server_url.clone();

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

    tabular.avatar_texture = None;
    tabular.avatar_texture_url = None;
    tabular.profile_display_name_input.clear();
    tabular.profile_avatar_url_input.clear();
    tabular.profile_username_input.clear();
    tabular.profile_phone_input.clear();

    tabular.vault = None;
    tabular.vault_team_keys.clear();
    tabular.vault_stage = super::ui_vault_setup::VaultStage::Unknown;
    tabular.vault_remote_bundle = None;
    tabular.vault_passphrase_input.clear();
    tabular.vault_passphrase_confirm_input.clear();
    tabular.vault_recovery_code_input.clear();
    tabular.vault_recovery_code_display = None;
    tabular.vault_error = None;
}

