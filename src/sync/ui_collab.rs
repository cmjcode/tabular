/// Collaboration panel UI — shows rooms, members, and presence.
///
/// Renders as a floating panel or a sidebar section when user is in collab mode.

use eframe::egui;
use crate::window_egui::Tabular;
use super::{CollabRoom, RoomMember, crdt_editor};

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

fn render_collab_content(tabular: &mut Tabular, ui: &mut egui::Ui) {
    if tabular.sync_account.is_none() {
        ui.vertical_centered(|ui| {
            ui.add_space(16.0);
            ui.label("🔒 Sign in to use collaboration features.");
            ui.add_space(8.0);
            if ui.button("Open Settings → Sync & Account").clicked() {
                tabular.show_settings_window = true;
            }
        });
        return;
    }

    // ── Current session ────────────────────────────────────────────────────
    if let Some(crdt) = &tabular.crdt_state {
        ui.horizontal(|ui| {
            ui.label("📡 Connected to room:");
            ui.strong(&crdt.room_id);
        });

        // Connected peers
        if !crdt.peers.is_empty() {
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Online now:").small());
            for peer in &crdt.peers {
                ui.horizontal(|ui| {
                    let dot = egui::RichText::new("●").color(peer.color).small();
                    ui.label(dot);
                    ui.label(&peer.display_name);
                    if let Some(pos) = peer.cursor_pos {
                        ui.label(egui::RichText::new(format!("(pos {})", pos)).small().weak());
                    }
                });
            }
        } else {
            ui.label(egui::RichText::new("You are the only one here.").small().weak());
        }

        ui.add_space(8.0);
        if ui.button("🚪  Leave Room").clicked() {
            if let Some(crdt) = &tabular.crdt_state {
                crdt.disconnect();
            }
            tabular.crdt_state = None;
        }

        ui.separator();
        ui.add_space(8.0);
    }

    // ── Room list ─────────────────────────────────────────────────────────
    ui.label(egui::RichText::new("Your Rooms").strong());
    ui.add_space(4.0);

    // New room button
    ui.horizontal(|ui| {
        let name_edit = egui::TextEdit::singleline(&mut tabular.new_collab_room_name)
            .hint_text("Room name…")
            .desired_width(180.0);
        ui.add(name_edit);
        if ui.button("➕ Create").clicked() {
            create_room(tabular);
        }
    });

    ui.add_space(8.0);

    // Refresh button
    if ui.small_button("🔄 Refresh rooms").clicked() {
        refresh_rooms(tabular);
    }

    ui.add_space(4.0);

    // Room list
    if tabular.collab_rooms.is_empty() {
        ui.label(egui::RichText::new("No rooms yet. Create one to start collaborating.").small().weak());
    } else {
        let rooms = tabular.collab_rooms.clone();
        for room in &rooms {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&room.name).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                if let Some(desc) = &room.description {
                    if !desc.is_empty() {
                        ui.label(egui::RichText::new(desc).small().weak());
                    }
                }
            });
            ui.add_space(2.0);
        }
    }
}

/// Inline sidebar section for collab — compact version
pub fn render_sidebar_collab_section(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new("🤝 Collaboration").strong().size(12.0));
    ui.add_space(2.0);

    if let Some(crdt) = &tabular.crdt_state {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("●").color(egui::Color32::from_rgb(72, 199, 116)));
            ui.label(egui::RichText::new(&crdt.room_id).small().strong());
        });
        let peer_count = crdt.peers.len();
        if peer_count > 0 {
            ui.label(egui::RichText::new(format!("{} other(s) online", peer_count)).small().weak());
        }
    } else {
        ui.label(egui::RichText::new("Not in a room").small().weak());
    }

    if ui.small_button("Open Collab Panel").clicked() {
        tabular.show_collab_panel = true;
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

fn refresh_rooms(tabular: &mut Tabular) {
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
