//! Account and Cloud Sync UI components.
//!
//! Provides:
//! 1. `render_sync_panel`: Cloud Sync settings tab inside Preferences modal.
//! 2. `render_account_dialog`: Dedicated modal popup for account management, login/logout, and profile photo settings.
//! 3. `draw_circular_avatar`: Helper to render circular user avatars with image texture or initials fallback.

use super::auth::OAuthProvider;
use crate::rfd;
use crate::window_egui::{Tabular, style};
use eframe::egui;

/// Directly paint a circular avatar into any painter at center with radius using a circular fan mesh.
pub fn paint_circular_avatar(
    painter: &egui::Painter,
    tabular: &Tabular,
    center: egui::Pos2,
    radius: f32,
    email: &str,
    display_name: Option<&str>,
    is_hovered: bool,
    dark_mode: bool,
) {
    if let Some(ref texture) = tabular.avatar_texture {
        // Draw true circular mesh with texture mapping
        let n_points = 32;
        let mut mesh = egui::Mesh::with_texture(texture.id());
        let center_uv = egui::pos2(0.5, 0.5);
        mesh.vertices.push(egui::epaint::Vertex {
            pos: center,
            uv: center_uv,
            color: egui::Color32::WHITE,
        });

        for i in 0..=n_points {
            let angle = i as f32 * std::f32::consts::TAU / (n_points as f32);
            let (sin, cos) = angle.sin_cos();
            let p = center + egui::vec2(cos, sin) * radius;
            let uv = center_uv + egui::vec2(cos, sin) * 0.5;
            mesh.vertices.push(egui::epaint::Vertex {
                pos: p,
                uv,
                color: egui::Color32::WHITE,
            });
        }

        for i in 1..=(n_points as u32) {
            mesh.indices.push(0);
            mesh.indices.push(i);
            mesh.indices.push(i + 1);
        }

        painter.add(egui::Shape::mesh(mesh));

        let stroke_color = if is_hovered {
            egui::Color32::from_rgb(100, 160, 255)
        } else if dark_mode {
            egui::Color32::from_rgb(80, 85, 100)
        } else {
            egui::Color32::from_rgb(190, 195, 205)
        };
        painter.circle_stroke(center, radius - 0.5, egui::Stroke::new(1.5, stroke_color));
    } else {
        let bg_color = if is_hovered {
            egui::Color32::from_rgb(45, 95, 190)
        } else if dark_mode {
            egui::Color32::from_rgb(55, 65, 85)
        } else {
            egui::Color32::from_rgb(215, 225, 240)
        };

        painter.circle_filled(center, radius, bg_color);
        let stroke_color = if is_hovered {
            egui::Color32::WHITE
        } else if dark_mode {
            egui::Color32::from_rgb(75, 85, 110)
        } else {
            egui::Color32::from_rgb(185, 195, 215)
        };
        painter.circle_stroke(center, radius - 0.5, egui::Stroke::new(1.0, stroke_color));

        let initial = display_name
            .and_then(|n| n.trim().chars().next())
            .or_else(|| email.trim().chars().next())
            .unwrap_or('U')
            .to_uppercase()
            .to_string();

        let font_size = (radius * 0.92).max(11.0);
        let text_color = if dark_mode {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(30, 40, 60)
        };

        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(font_size),
            text_color,
        );
    }
}

/// Draw a circular avatar with either the loaded image texture or initials / user icon fallback.
pub fn draw_circular_avatar(
    ui: &mut egui::Ui,
    tabular: &Tabular,
    size: f32,
    email: &str,
    display_name: Option<&str>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    paint_circular_avatar(
        ui.painter(),
        tabular,
        rect.center(),
        size / 2.0,
        email,
        display_name,
        resp.hovered(),
        ui.visuals().dark_mode,
    );
    resp
}

/// Open and sync inputs for the dedicated Account Dialog.
pub fn open_account_dialog(tabular: &mut Tabular) {
    tabular.sync_profile_inputs_from_account();
    tabular.show_account_dialog = true;
    // Populate the unblock list up front so the section is not empty the first
    // time someone opens it looking for a block they just made.
    if tabular.sync_account.is_some() {
        refresh_blocked_users(tabular);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Settings Panel Tab: Cloud Sync (render_sync_panel)
// ─────────────────────────────────────────────────────────────────────────────

/// Render the Cloud Sync panel inside the Settings / Preferences modal.
pub fn render_sync_panel(tabular: &mut Tabular, ui: &mut egui::Ui) {
    use crate::window_egui::preferences::{
        Tone, divider, hint, page_header, row, section, stacked, status,
    };

    page_header(
        ui,
        "Cloud Sync",
        "Synchronize connections and query history across devices and collaborate securely.",
    );

    section(ui, "Account", |ui| {
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
                    hint(ui, &account.email);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(style::btn_primary_ctx(ui.ctx(), "Manage Account"))
                        .clicked()
                    {
                        open_account_dialog(tabular);
                    }
                });
            } else {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Not signed in").strong().size(13.5));
                    hint(
                        ui,
                        "Tabular works fully offline. Sign in to enable cloud sync.",
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(style::btn_primary_ctx(ui.ctx(), "Sign In / Create Account"))
                        .clicked()
                    {
                        open_account_dialog(tabular);
                    }
                });
            }
        });
    });

    section(ui, "Server", |ui| {
        stacked(ui, "Sync server URL", None, |ui| {
            let resp = style::render_text_field(
                ui,
                egui::TextEdit::singleline(&mut tabular.sync_server_url)
                    .hint_text("https://api.tabular.id"),
                f32::INFINITY,
                None,
            );
            if resp.lost_focus() || resp.changed() {
                tabular.prefs_dirty = true;
            }
        });
        if !tabular.sync_server_url.trim().is_empty()
            && !is_server_url_acceptable(&tabular.sync_server_url)
        {
            status(
                ui,
                Tone::Warning,
                "⚠ Use https://. Plain http:// is only accepted for localhost.",
            );
        }
        divider(ui);

        let tone = match &tabular.sync_status {
            super::SyncStatus::Synced => Tone::Success,
            super::SyncStatus::Syncing => Tone::Warning,
            super::SyncStatus::Error(_) => Tone::Danger,
            super::SyncStatus::Offline => Tone::Muted,
        };
        let label = tabular.sync_status.label().to_string();
        row(ui, "Status", None, |ui| {
            let color = crate::window_egui::preferences::tone_color(ui.ctx(), tone);
            style::render_badge(ui, &label, color.gamma_multiply(0.18), color);
        });
        if let super::SyncStatus::Error(e) = &tabular.sync_status {
            status(ui, Tone::Danger, e.clone());
        }
    });

    if tabular.sync_account.is_some() {
        section(ui, "Manual Sync", |ui| {
            hint(
                ui,
                "Push and pull changes immediately instead of waiting for the next automatic sync.",
            );
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                if ui.add(style::btn_secondary("🔗  Connections")).clicked() {
                    tabular.sync_trigger_connections = true;
                }
                if ui.add(style::btn_secondary("📜  History")).clicked() {
                    tabular.sync_trigger_history = true;
                }
                if ui.add(style::btn_secondary("💾  Queries")).clicked() {
                    tabular.sync_trigger_queries = true;
                }
                if ui.add(style::btn_secondary("🌐  HTTP Requests")).clicked() {
                    tabular.sync_trigger_http = true;
                }
            });
        });

        section(ui, "End-to-End Encryption", |ui| {
            super::ui_vault_setup::render_vault_panel(tabular, ui);
        });
    }
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
    let is_logged_in = tabular.sync_account.is_some();

    if is_logged_in {
        let dialog_w = 580.0f32.min(screen_rect.width() - 32.0);
        let max_scroll_h = (screen_rect.height() - 140.0).max(250.0);

        egui::Window::new("account_profile_dialog")
            .id(egui::Id::new("account_profile_dialog"))
            .open(&mut open_flag)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(screen_rect.center())
            .min_width(dialog_w)
            .max_width(dialog_w)
            .default_width(dialog_w)
            .max_height(screen_rect.height() - 32.0)
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 20,
                        top: 16,
                        bottom: 18,
                    })
                    .shadow(egui::Shadow {
                        offset: [0, 16],
                        blur: 48,
                        spread: 4,
                        color: egui::Color32::from_black_alpha(200),
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if ctx.global_style().visuals.dark_mode {
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 25)
                        } else {
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 30)
                        },
                    )),
            )
            .show(ctx, |ui| {
                // Header row: Tabs on the left, Close (X) button on the right
                ui.horizontal(|ui| {
                    render_account_tab_bar(tabular, ui);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui_icons::icons::ICON_CLOSE
                                .rich_text()
                                .size(16.0)
                                .color(ui.visuals().weak_text_color()),
                        )
                        .frame(false);
                        if ui
                            .add(close_btn)
                            .on_hover_text("Close (Esc)")
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            tabular.show_account_dialog = false;
                        }
                    });
                });

                ui.add_space(14.0);

                // Scrollable main content with auto_shrink [false, true] to prevent infinite height expansion
                egui::ScrollArea::vertical()
                    .id_salt("account_dialog_content_scroll")
                    .max_height(max_scroll_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| match tabular.account_dialog_tab {
                        crate::window_egui::AccountDialogTab::Profile => {
                            render_account_profile_tab(tabular, ui);
                        }
                        crate::window_egui::AccountDialogTab::Security => {
                            render_account_security_tab(tabular, ui);
                        }
                    });

                // Fixed Bottom Action Bar — clean without separator or redundant close button
                if tabular.account_dialog_tab == crate::window_egui::AccountDialogTab::Profile {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let saving = tabular.profile_update_receiver.is_some();
                        if saving {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new("Saving changes…")
                                    .size(12.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_enabled_ui(!saving, |ui| {
                                if ui
                                    .add(style::btn_primary_ctx(
                                        ui.ctx(),
                                        if saving {
                                            "💾  Saving…"
                                        } else {
                                            "💾  Save Changes"
                                        },
                                    ))
                                    .clicked()
                                {
                                    save_profile(tabular);
                                }
                            });
                        });
                    });
                }
            });
    } else {
        let login_w = 400.0f32.min(screen_rect.width() - 32.0);
        let max_scroll_h = (screen_rect.height() - 120.0).max(200.0);

        egui::Window::new("account_login_dialog")
            .id(egui::Id::new("account_login_dialog"))
            .open(&mut open_flag)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(screen_rect.center())
            .min_width(login_w)
            .max_width(login_w)
            .default_width(login_w)
            .max_height(screen_rect.height() - 32.0)
            .frame(
                egui::Frame::window(&ctx.global_style())
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin {
                        left: 20,
                        right: 12,
                        top: 12,
                        bottom: 16,
                    })
                    .shadow(egui::Shadow {
                        offset: [0, 16],
                        blur: 48,
                        spread: 4,
                        color: egui::Color32::from_black_alpha(200),
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if ctx.global_style().visuals.dark_mode {
                            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 25)
                        } else {
                            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 30)
                        },
                    )),
            )
            .show(ctx, |ui| {
                // Header row: Title on the left, Close (X) button right in the top-right corner
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("Sign In to Tabular")
                            .size(17.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let close_btn = egui::Button::new(
                            egui_icons::icons::ICON_CLOSE
                                .rich_text()
                                .size(16.0)
                                .color(ui.visuals().weak_text_color()),
                        )
                        .frame(false);
                        if ui
                            .add(close_btn)
                            .on_hover_text("Close")
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            tabular.show_account_dialog = false;
                        }
                    });
                });

                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .id_salt("account_login_dialog_scroll")
                    .max_height(max_scroll_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        render_account_login_view(tabular, ui);
                    });
            });
    }

    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        tabular.show_account_dialog = false;
    }

    if !open_flag {
        tabular.show_account_dialog = false;
    }
}

/// Modern pill tab bar switching between Profile and Security tabs.
fn render_account_tab_bar(tabular: &mut Tabular, ui: &mut egui::Ui) {
    use crate::window_egui::AccountDialogTab;
    let dark = ui.visuals().dark_mode;
    let accent = style::theme_accent(ui.ctx());

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        let tabs = [
            (AccountDialogTab::Profile, "👤  Profile & Info"),
            (AccountDialogTab::Security, "🛡️  Security & Privacy"),
        ];

        for (tab, label) in tabs {
            let is_selected = tabular.account_dialog_tab == tab;
            let bg_color = if is_selected {
                if dark {
                    egui::Color32::from_rgb(45, 50, 68)
                } else {
                    egui::Color32::from_rgb(228, 235, 248)
                }
            } else {
                egui::Color32::TRANSPARENT
            };

            let text_color = if is_selected {
                accent
            } else {
                ui.visuals().weak_text_color()
            };

            let stroke = if is_selected {
                egui::Stroke::new(1.0, accent)
            } else {
                egui::Stroke::NONE
            };

            let btn = egui::Button::new(
                egui::RichText::new(label)
                    .size(13.0)
                    .strong()
                    .color(text_color),
            )
            .fill(bg_color)
            .stroke(stroke)
            .corner_radius(egui::CornerRadius::same(6))
            .min_size(egui::vec2(165.0, 32.0));

            if ui.add(btn).clicked() {
                tabular.account_dialog_tab = tab;
            }
        }
    });
}

/// Helper to trigger file dialog for selecting a profile picture.
fn choose_avatar_file(tabular: &mut Tabular) {
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

/// Tab 1: Profile & Personal Information view.
fn render_account_profile_tab(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let dark = ui.visuals().dark_mode;
    let card_bg = if dark {
        egui::Color32::from_rgb(26, 28, 36)
    } else {
        egui::Color32::from_rgb(248, 250, 253)
    };
    let card_stroke = if dark {
        egui::Color32::from_rgb(46, 50, 64)
    } else {
        egui::Color32::from_rgb(222, 226, 235)
    };

    ui.vertical(|ui| {
        ui.add_space(2.0);

        // 1. Hero Profile Identity Card
        egui::Frame::new()
            .fill(card_bg)
            .stroke(egui::Stroke::new(1.0, card_stroke))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    // Left: Avatar with quick photo actions
                    ui.vertical(|ui| {
                        draw_circular_avatar(
                            ui,
                            tabular,
                            72.0,
                            &account.email,
                            if tabular.profile_display_name_input.is_empty() {
                                account.display_name.as_deref()
                            } else {
                                Some(&tabular.profile_display_name_input)
                            },
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    style::btn_secondary("📁 Change")
                                        .min_size(egui::vec2(60.0, 24.0)),
                                )
                                .on_hover_text("Choose an image from your computer")
                                .clicked()
                            {
                                choose_avatar_file(tabular);
                            }
                            if !tabular.profile_avatar_url_input.is_empty() {
                                if ui.button("🗑").on_hover_text("Remove photo").clicked() {
                                    tabular.profile_avatar_url_input.clear();
                                    tabular.avatar_texture = None;
                                    tabular.avatar_texture_url = None;
                                }
                            }
                        });
                    });

                    ui.add_space(16.0);

                    // Right: User Details & Badges
                    ui.vertical(|ui| {
                        let name = if !tabular.profile_display_name_input.is_empty() {
                            &tabular.profile_display_name_input
                        } else {
                            account.display_name.as_deref().unwrap_or(&account.email)
                        };
                        ui.label(egui::RichText::new(name).strong().size(18.0));
                        ui.add_space(3.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&account.email)
                                    .color(ui.visuals().weak_text_color())
                                    .size(13.0),
                            );
                            ui.add_space(6.0);

                            // Verified badge pill
                            let badge_bg = if dark {
                                egui::Color32::from_rgb(18, 56, 32)
                            } else {
                                egui::Color32::from_rgb(225, 248, 232)
                            };
                            let badge_fg = if dark {
                                egui::Color32::from_rgb(72, 199, 116)
                            } else {
                                egui::Color32::from_rgb(16, 130, 60)
                            };
                            egui::Frame::new()
                                .fill(badge_bg)
                                .corner_radius(egui::CornerRadius::same(10))
                                .inner_margin(egui::Margin::symmetric(8, 2))
                                .show(ui, |ui| {
                                    ui.label(
                                        egui::RichText::new("✓ Verified")
                                            .color(badge_fg)
                                            .size(11.0)
                                            .strong(),
                                    );
                                });
                        });

                        if let Some(ref handle) = account.username {
                            if !handle.is_empty() {
                                ui.add_space(3.0);
                                ui.label(
                                    egui::RichText::new(format!("@{}", handle))
                                        .color(style::theme_accent(ui.ctx()))
                                        .size(13.0)
                                        .strong(),
                                );
                            }
                        }

                        ui.add_space(8.0);

                        // User ID with copy button
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("User ID:")
                                    .size(11.5)
                                    .color(ui.visuals().weak_text_color()),
                            );
                            ui.label(
                                egui::RichText::new(&account.user_id)
                                    .monospace()
                                    .size(11.5)
                                    .color(ui.visuals().weak_text_color()),
                            );
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new("📋 Copy").size(10.5))
                                        .small(),
                                )
                                .clicked()
                            {
                                ui.ctx().copy_text(account.user_id.clone());
                                tabular.toasts.info("User ID copied to clipboard");
                            }
                        });
                    });
                });

                // Collapsible Image URL input
                ui.add_space(8.0);
                ui.collapsing("🔗 Custom Image URL or Base64", |ui| {
                    ui.horizontal(|ui| {
                        let avatar_w = ui.available_width() - 10.0;
                        let avatar_edit = style::render_text_field(
                            ui,
                            egui::TextEdit::singleline(&mut tabular.profile_avatar_url_input)
                                .hint_text("https://example.com/photo.png or data:image/..."),
                            avatar_w,
                            None,
                        );
                        if avatar_edit.changed() {
                            tabular.avatar_texture = None;
                            tabular.avatar_texture_url = None;
                        }
                    });
                });
            });

        ui.add_space(14.0);

        // 2. Personal Information Card
        egui::Frame::new()
            .fill(card_bg)
            .stroke(egui::Stroke::new(1.0, card_stroke))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                let field_w = (ui.available_width() - 130.0).max(280.0);

                ui.label(
                    egui::RichText::new("Personal Information")
                        .strong()
                        .size(14.0),
                );
                ui.add_space(2.0);
                ui.label(
                    egui::RichText::new("Update your personal details and public profile info.")
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(12.0);

                egui::Grid::new("account_info_form_grid")
                    .num_columns(2)
                    .spacing([18.0, 14.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Display Name:").strong().size(12.5));
                        style::render_text_field(
                            ui,
                            egui::TextEdit::singleline(&mut tabular.profile_display_name_input)
                                .hint_text("e.g. John Doe"),
                            field_w,
                            None,
                        );
                        ui.end_row();

                        ui.label(egui::RichText::new("Username:").strong().size(12.5));
                        ui.vertical(|ui| {
                            style::render_text_field(
                                ui,
                                egui::TextEdit::singleline(&mut tabular.profile_username_input)
                                    .hint_text("e.g. johndoe"),
                                field_w,
                                None,
                            );
                            ui.label(
                                egui::RichText::new("Used for team invites and mentions")
                                    .size(10.5)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                        ui.end_row();

                        ui.label(egui::RichText::new("Phone Number:").strong().size(12.5));
                        style::render_text_field(
                            ui,
                            egui::TextEdit::singleline(&mut tabular.profile_phone_input)
                                .hint_text("e.g. +62 812 3456 7890"),
                            field_w,
                            None,
                        );
                        ui.end_row();

                        ui.label(egui::RichText::new("Email Address:").strong().size(12.5));
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(&account.email).size(12.5));
                            ui.label(
                                egui::RichText::new("🔒 Linked to account")
                                    .size(11.0)
                                    .color(ui.visuals().weak_text_color()),
                            );
                        });
                        ui.end_row();
                    });
            });

        ui.add_space(10.0);
    });
}

/// Tab 2: Security, Blocked Users, and Danger Zone view.
fn render_account_security_tab(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let dark = ui.visuals().dark_mode;
    let card_bg = if dark {
        egui::Color32::from_rgb(26, 28, 36)
    } else {
        egui::Color32::from_rgb(248, 250, 253)
    };
    let card_stroke = if dark {
        egui::Color32::from_rgb(46, 50, 64)
    } else {
        egui::Color32::from_rgb(222, 226, 235)
    };

    ui.vertical(|ui| {
        ui.add_space(2.0);

        // 1. Active Session Card
        egui::Frame::new()
            .fill(card_bg)
            .stroke(egui::Stroke::new(1.0, card_stroke))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Active Session").strong().size(14.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(style::btn_danger_ctx(ui.ctx(), "🚪  Sign Out"))
                            .clicked()
                        {
                            do_logout(tabular);
                        }
                    });
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Manage your current session and connection to Tabular Cloud Sync.")
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(10.0);

                ui.horizontal(|ui| {
                    ui.label("Signed in as:");
                    ui.label(egui::RichText::new(&account.email).strong());
                });
            });

        ui.add_space(14.0);

        // 2. Blocked Users Card (App Store Guideline 1.2)
        egui::Frame::new()
            .fill(card_bg)
            .stroke(egui::Stroke::new(1.0, card_stroke))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let count_text = if tabular.blocked_users.is_empty() {
                        "🚫 Blocked Users".to_string()
                    } else {
                        format!("🚫 Blocked Users ({})", tabular.blocked_users.len())
                    };
                    ui.label(egui::RichText::new(count_text).strong().size(14.0));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(style::btn_secondary("🔄 Refresh")).clicked() {
                            refresh_blocked_users(tabular);
                        }
                    });
                });

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "You can block or unblock collaborators from the Teams member list.",
                    )
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(10.0);

                if tabular.blocked_users.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "You have not blocked anyone. Block a person from the Teams member list.",
                        )
                        .size(11.5)
                        .color(ui.visuals().weak_text_color()),
                    );
                } else {
                    let blocked = tabular.blocked_users.clone();
                    for b in &blocked {
                        ui.horizontal(|ui| {
                            let label = b.display_name.clone().unwrap_or_else(|| b.email.clone());
                            ui.label(egui::RichText::new(format!("• {}", label)).size(12.5));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(egui::RichText::new("Unblock").small()).clicked() {
                                    do_unblock_user(tabular, &b.id);
                                }
                            });
                        });
                    }
                }
            });

        ui.add_space(14.0);

        // 3. Danger Zone Card (App Store Guideline 5.1.1(v))
        let danger_bg = if dark {
            egui::Color32::from_rgb(38, 20, 20)
        } else {
            egui::Color32::from_rgb(255, 244, 244)
        };
        let danger_stroke = if dark {
            egui::Color32::from_rgb(90, 36, 36)
        } else {
            egui::Color32::from_rgb(240, 190, 190)
        };
        egui::Frame::new()
            .fill(danger_bg)
            .stroke(egui::Stroke::new(1.0, danger_stroke))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    egui::RichText::new("⚠️ Danger Zone")
                        .strong()
                        .size(14.0)
                        .color(egui::Color32::from_rgb(220, 70, 70)),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "Deleting your account permanently erases your synced connections, saved queries, \
                         query history, HTTP requests and vault keys from the server, and removes any team \
                         you own for its other members. This action cannot be undone.",
                    )
                    .size(11.5)
                    .color(ui.visuals().weak_text_color()),
                );
                ui.add_space(10.0);
                if ui.add(style::btn_danger_ctx(ui.ctx(), "🗑  Delete Account")).clicked() {
                    tabular.show_delete_account_dialog = true;
                    tabular.delete_account_confirm_input.clear();
                    tabular.delete_account_error = None;
                }
            });

        ui.add_space(10.0);
    });
}

/// Modal that gates account deletion behind typing the account's own email.
///
/// Rendered from the top-level frame loop rather than nested inside the account
/// dialog, so it survives that dialog being closed underneath it.
pub fn render_delete_account_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_delete_account_dialog {
        return;
    }

    let Some(account) = tabular.sync_account.clone() else {
        // Signed out from another surface while the modal was open.
        tabular.show_delete_account_dialog = false;
        return;
    };

    let in_progress = tabular.delete_account_receiver.is_some();

    egui::Window::new("Delete Account")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.add_space(4.0);

            ui.label(
                egui::RichText::new("⚠  This permanently deletes your Tabular account")
                    .strong()
                    .color(egui::Color32::from_rgb(220, 90, 90)),
            );
            ui.add_space(8.0);

            ui.label("The following is erased from the server and cannot be recovered:");
            ui.add_space(4.0);
            for line in [
                "• Synced database connections",
                "• Saved queries and query history",
                "• Saved HTTP requests",
                "• Vault keys — encrypted credentials become unrecoverable",
                "• Teams you own, including for their other members",
            ] {
                ui.label(egui::RichText::new(line).size(12.0));
            }

            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "Your databases themselves are untouched — this only removes what Tabular \
                     stores for your account. Local data on this device is cleared too.",
                )
                .size(11.0)
                .color(ui.visuals().weak_text_color()),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(8.0);

            ui.label(format!("Type {} to confirm:", account.email));
            ui.add_space(4.0);
            // Nonaktifkan input saat proses hapus akun sedang berjalan
            ui.add_enabled_ui(!in_progress, |ui| {
                style::render_text_field(
                    ui,
                    egui::TextEdit::singleline(&mut tabular.delete_account_confirm_input)
                        .hint_text(account.email.clone()),
                    f32::INFINITY,
                    None,
                );
            });

            let confirmed = tabular.delete_account_confirm_input.trim() == account.email;

            if let Some(err) = &tabular.delete_account_error {
                ui.add_space(6.0);
                ui.colored_label(egui::Color32::from_rgb(255, 100, 100), err);
            }

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!in_progress, |ui| {
                    if ui.add(style::btn_secondary("Cancel")).clicked() {
                        tabular.show_delete_account_dialog = false;
                        tabular.delete_account_confirm_input.clear();
                        tabular.delete_account_error = None;
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(confirmed && !in_progress, |ui| {
                        let label = if in_progress {
                            "Deleting…"
                        } else {
                            "🗑  Delete My Account"
                        };
                        if ui.add(style::btn_danger_ctx(ui.ctx(), label)).clicked() {
                            do_delete_account(tabular);
                        }
                    });
                });
            });

            ui.add_space(4.0);
        });
}

/// Fire the DELETE and let `poll_delete_account_receiver` finish the teardown.
///
/// The local wipe deliberately waits for the server to confirm: clearing first
/// would throw away the very token needed to authenticate the request, leaving
/// a live server-side account behind if it failed.
fn do_delete_account(tabular: &mut Tabular) {
    let Some(account) = tabular.sync_account.clone() else {
        return;
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    tabular.delete_account_error = None;

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client
            .delete_account(&token)
            .await
            .map(|resp| resp.email)
            .map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    tabular.delete_account_receiver = Some(rx);
}

/// Helper to render an OAuth provider tile button with icon on top and small label underneath.
fn render_oauth_tile(
    ui: &mut egui::Ui,
    icon: egui_icons::MaterialIcon,
    label: &str,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let accent = style::theme_accent(ui.ctx());
        let is_hovered = response.hovered();
        let is_pressed = response.is_pointer_button_down_on();

        let bg_fill = if is_pressed {
            accent.gamma_multiply(0.8)
        } else if is_hovered {
            accent.gamma_multiply(0.9)
        } else {
            accent
        };

        ui.painter().rect_filled(rect, 6.0, bg_fill);

        let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        child_ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            ui.add(egui::Label::new(
                icon.rich_text().size(20.0).color(egui::Color32::WHITE),
            ));
            ui.add_space(2.0);
            ui.add(egui::Label::new(
                egui::RichText::new(label)
                    .size(10.0)
                    .strong()
                    .color(egui::Color32::WHITE),
            ));
        });
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Render logged-out login / create account view.
fn render_account_login_view(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new("Connect your account to sync connections, queries, and collaborate in real-time.")
                    .size(12.0)
                    .color(ui.visuals().weak_text_color()),
            )
            .wrap(),
        );
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new("💡 Note: An account is completely optional. Tabular is offline-first and fully functional without login.")
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            )
            .wrap(),
        );
        ui.add_space(14.0);

        // Ensure default sync server url is set even when input is hidden
        if tabular.sync_server_url.trim().is_empty() {
            tabular.sync_server_url = "https://api.tabular.id".to_string();
        }

        // OAuth buttons: Apple, Google, GitHub side-by-side in one row with increased height and label
        let total_spacing = 8.0 * 2.0;
        let btn_w = ((ui.available_width() - total_spacing) / 3.0).max(60.0);
        let btn_size = egui::vec2(btn_w, 54.0);

        ui.horizontal(|ui| {
            // 1. Apple
            let apple_resp = render_oauth_tile(
                ui,
                egui_icons::icons::ICON_APPLE,
                "Sign with Apple",
                btn_size,
            );
            if apple_resp.on_hover_text("Sign in with Apple").clicked() {
                start_oauth(tabular, OAuthProvider::Apple);
            }

            ui.add_space(8.0);

            // 2. Google
            let google_resp = render_oauth_tile(
                ui,
                egui_icons::icons::ICON_GOOGLE,
                "Sign with Google",
                btn_size,
            );
            if google_resp.on_hover_text("Sign in with Google").clicked() {
                start_oauth(tabular, OAuthProvider::Google);
            }

            ui.add_space(8.0);

            // 3. GitHub
            let github_resp = render_oauth_tile(
                ui,
                egui_icons::icons::ICON_GITHUB,
                "Sign with GitHub",
                btn_size,
            );
            if github_resp.on_hover_text("Sign in with GitHub").clicked() {
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

            // Desktop only. On iOS sign-in always completes over HTTPS ticket
            // polling, so this escape hatch is never needed there — and asking
            // an App Store reviewer to paste raw token JSON reads as an
            // unfinished developer screen.
            if !cfg!(target_os = "ios") {
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
            }

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
        ui.add(
            egui::Label::new(
                egui::RichText::new("🔒 Your connection credentials remain encrypted locally before being sent to the server.")
                    .size(11.0)
                    .color(ui.visuals().weak_text_color()),
            )
            .wrap(),
        );
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
        tabular.sync_server_url = "https://api.tabular.id".to_string();
    }
    if !is_server_url_acceptable(&tabular.sync_server_url) {
        tabular.sync_login_error = Some(
            "Server URL must use https:// (plain http:// is only allowed for localhost/127.0.0.1)"
                .to_string(),
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
                tabular.sync_login_error =
                    Some("Invalid token JSON — missing access_token or email".to_string());
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
            tabular.sync_account = Some(account);
            tabular.sync_profile_inputs_from_account();

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

    wipe_local_session(tabular);
}

/// Drop every trace of the signed-in account from this device.
///
/// Shared by sign-out and account deletion. For deletion this is the second
/// half of the operation — Guideline 5.1.1(v) expects the app to be back in its
/// signed-out state once the server confirms, with no stale vault material left
/// behind that could still decrypt a cached payload.
pub fn wipe_local_session(tabular: &mut Tabular) {
    super::api_client::clear_account();
    tabular.sync_account = None;
    tabular.sync_status = super::SyncStatus::Offline;
    tabular.crdt_state = None;

    tabular.sync_profile_inputs_from_account();

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

/// Fetch the blocked-user list for the unblock UI (App Store Guideline 1.2).
pub fn refresh_blocked_users(tabular: &mut Tabular) {
    let Some(account) = tabular.sync_account.clone() else {
        return;
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.list_blocks(&token).await.map_err(|e| e.to_string());
        let _ = tx.send(result);
    });

    tabular.blocked_users_receiver = Some(rx);
}

/// Lift a block, then refresh the list so the row disappears.
fn do_unblock_user(tabular: &mut Tabular, user_id: &str) {
    let Some(account) = tabular.sync_account.clone() else {
        return;
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let target = user_id.to_string();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let _ = client.unblock_user(&token, &target).await;
    });

    // Optimistic: the row goes now, and the next refresh reconciles if the
    // request turned out to fail.
    tabular.blocked_users.retain(|b| b.id != user_id);
}
