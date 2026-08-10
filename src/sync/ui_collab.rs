//! Collaboration panel UI — shows rooms, members, and presence.
//!
//! Renders as a floating panel or a sidebar section when user is in collab mode.

use eframe::egui;
use crate::window_egui::Tabular;
use super::{CollabRoom, crdt_editor};

/// Render the collab panel (call from sidebar or floating window)
pub fn render_collab_panel(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_collab_panel {
        return;
    }

    egui::Window::new("☁  Collaboration")
        .id(egui::Id::new("collab_panel"))
        .resizable(true)
        .min_width(300.0)
        .default_width(350.0)
        .show(ctx, |ui| {
            render_collab_content(tabular, ui);
        });
}

pub fn render_collab_content(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let session_expired = if let Some(err) = &tabular.sync_login_error {
        err.contains("401") || err.contains("Unauthorized") || err.contains("Session expired")
    } else {
        false
    };

    if tabular.sync_account.is_none() || session_expired {
        ui.add_space(4.0);
        ui.group(|ui| {
            if session_expired {
                ui.label(egui::RichText::new("⚠️ Sesi Telah Berakhir (401)").small().strong().color(egui::Color32::from_rgb(255, 170, 0)));
                ui.label(egui::RichText::new("Sesi login Anda telah habis. Silakan login kembali untuk melanjutkan kolaborasi.").small().weak());
            } else {
                ui.label(egui::RichText::new("🔒 Belum Login").small().strong());
                ui.label(egui::RichText::new("Silakan login akun Tabular untuk menggunakan fitur kolaborasi.").small().weak());
            }
            ui.add_space(6.0);
            if ui.add(crate::window_egui::style::btn_primary_ctx(ui.ctx(), "🔑 Login Kembali")).clicked() {
                tabular.sync_login_pending = true;
                tabular.sync_login_error = None;
                tabular.sync_auth_receiver = Some(crate::sync::auth::start_oauth_flow(
                    &tabular.sync_server_url,
                    crate::sync::auth::OAuthProvider::Google,
                ));
            }
        });
        ui.add_space(4.0);
        return;
    }

    // ── Current session card ─────────────────────────────────────────────
    let mut disconnect_requested = false;
    if let Some(crdt) = &tabular.crdt_state {
        let room_id = crdt.room_id.clone();
        let peers = crdt.peers.clone();

        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("●").color(egui::Color32::from_rgb(72, 199, 116)).small());
                ui.label(egui::RichText::new(format!("Room: {}", room_id)).small().strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(crate::window_egui::style::btn_danger_ctx(ui.ctx(), "Leave")).clicked() {
                        disconnect_requested = true;
                    }
                });
            });

            if !peers.is_empty() {
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new("Members:").small().weak());
                    for peer in &peers {
                        let dot = egui::RichText::new("●").color(peer.color).small();
                        ui.label(dot);
                        ui.label(egui::RichText::new(&peer.display_name).small());
                    }
                });
            }
        });
        ui.add_space(6.0);
    }

    if disconnect_requested {
        if let Some(crdt) = &tabular.crdt_state {
            crdt.disconnect();
        }
        tabular.crdt_state = None;
    }

    // ── Create & Refresh room row ───────────────────────────────────────
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);

        let avail_width = ui.available_width();
        let btn_width = 24.0;
        let refresh_width = 24.0;
        let input_width = (avail_width - btn_width - refresh_width - 16.0).max(40.0);

        ui.add_sized(
            [input_width, 24.0],
            egui::TextEdit::singleline(&mut tabular.new_collab_room_name)
                .hint_text("Room name…")
        );

        if ui.add_sized(
            [btn_width, 24.0],
            egui::Button::new(egui::RichText::new("+").strong()).corner_radius(4.0)
        ).on_hover_text("Create room").clicked() {
            create_room(tabular);
        }

        if ui.add_sized(
            [refresh_width, 24.0],
            egui::Button::new(egui::RichText::new("🔄").small()).corner_radius(4.0)
        ).on_hover_text("Refresh room list").clicked() {
            refresh_rooms(tabular);
        }
    });

    ui.add_space(6.0);

    // ── Room list ─────────────────────────────────────────────────────────
    if tabular.collab_rooms.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new("No rooms yet. Create one to start collaborating.").small().weak());
    } else {
        let rooms = tabular.collab_rooms.clone();
        for room in &rooms {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&room.name).small().strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("🗑").small()).frame(false))
                            .on_hover_text("Delete room")
                            .clicked()
                        {
                            delete_room(tabular, &room.id);
                        }

                        let is_current = tabular.crdt_state
                            .as_ref()
                            .map(|c| c.room_id == room.id)
                            .unwrap_or(false);

                        if is_current {
                            ui.label(egui::RichText::new("● Connected").color(egui::Color32::from_rgb(72, 199, 116)).small());
                        } else if ui.small_button("Join").clicked() {
                            join_room(tabular, room);
                        }
                    });
                });
                if let Some(desc) = &room.description
                    && !desc.is_empty() {
                        ui.label(egui::RichText::new(desc).small().weak());
                    }
            });
            ui.add_space(2.0);
        }
    }
}

/// Inline sidebar section for collab — accordion version without vertical guide line
pub fn render_sidebar_collab_section(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.add_space(2.0);

    let session_expired = if let Some(err) = &tabular.sync_login_error {
        err.contains("401") || err.contains("Unauthorized") || err.contains("Session expired")
    } else {
        false
    };

    let header_title = if session_expired {
        "☁ Collaboration (⚠️ Sesi Expired)".to_string()
    } else if let Some(crdt) = &tabular.crdt_state {
        format!("☁ Collaboration (● {})", crdt.room_id)
    } else {
        "☁ Collaboration".to_string()
    };

    let header_text = if session_expired {
        egui::RichText::new(header_title).strong().size(12.0).color(egui::Color32::from_rgb(255, 170, 0))
    } else {
        egui::RichText::new(header_title).strong().size(12.0)
    };

    let id = ui.make_persistent_id("sidebar_collab_accordion");
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, session_expired);
    let is_open_before = state.is_open();

    let header_res = state
        .show_header(ui, |ui| {
            ui.label(header_text);
        })
        .body_unindented(|ui| {
            ui.add_space(4.0);
            render_collab_content(tabular, ui);
            ui.add_space(4.0);
        });

    if header_res.0.clicked() && !is_open_before {
        refresh_rooms(tabular);
    }
}

// ─── Actions ──────────────────────────────────────────────────────────────────

fn join_room(tabular: &mut Tabular, room: &CollabRoom) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    // Disconnect from current room if any
    if let Some(old) = &tabular.crdt_state {
        old.disconnect();
    }

    let display_name = account.display_name
        .clone()
        .unwrap_or_else(|| account.email.clone());

    let crdt_state = crdt_editor::connect_to_room(
        room.id.clone(),
        tabular.sync_server_url.clone(),
        account.access_token.clone(),
        display_name,
    );

    tabular.crdt_state = Some(crdt_state);
    tabular.toasts.info(format!("Joined room: {}", room.name));
}

fn create_room(tabular: &mut Tabular) {
    let name = tabular.new_collab_room_name.trim().to_string();
    if name.is_empty() {
        return;
    }

    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let name_clone = name.clone();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.create_room(&token, &name_clone, None).await;
        let _ = tx.send(result);
    });

    tabular.new_collab_room_name.clear();
    tabular.collab_room_create_receiver = Some(rx);
}

fn delete_room(tabular: &mut Tabular, room_id: &str) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let room_id_clone = room_id.to_string();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.delete_room(&token, &room_id_clone).await.map(|_| room_id_clone);
        let _ = tx.send(result);
    });

    tabular.collab_room_delete_receiver = Some(rx);
}

pub fn refresh_rooms(tabular: &mut Tabular) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.list_rooms(&token).await;
        let _ = tx.send(result);
    });

    tabular.collab_rooms_receiver = Some(rx);
}
