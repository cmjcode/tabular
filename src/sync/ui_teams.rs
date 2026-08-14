//! Teams UI panel and sidebar section — manage Teams, members, and create rooms.

use eframe::egui;
use crate::window_egui::Tabular;
use super::api_client::{AddTeamMemberReq, CreateTeamReq, CreateTeamRoomReq, RemoteTeam};

/// Inline sidebar section for Teams — accordion version
pub fn render_sidebar_teams_section(tabular: &mut Tabular, ui: &mut egui::Ui) {
    ui.add_space(2.0);

    let session_expired = if let Some(err) = &tabular.sync_login_error {
        err.contains("401") || err.contains("Unauthorized") || err.contains("Session expired")
    } else {
        false
    };

    let count = tabular.teams.len();
    let header_title = if count > 0 {
        format!("👥 Teams ({})", count)
    } else {
        "👥 Teams".to_string()
    };

    let id = ui.make_persistent_id("sidebar_teams_accordion");
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let is_open_before = state.is_open();

    let header_res = state
        .show_header(ui, |ui| {
            ui.label(egui::RichText::new(header_title).strong().size(12.0));
        })
        .body_unindented(|ui| {
            ui.add_space(4.0);
            render_teams_content(tabular, ui);
            ui.add_space(4.0);
        });

    if header_res.0.clicked() && !is_open_before && tabular.sync_account.is_some() && !session_expired {
        refresh_teams(tabular);
    }
}

pub fn render_teams_content(tabular: &mut Tabular, ui: &mut egui::Ui) {
    let session_expired = if let Some(err) = &tabular.sync_login_error {
        err.contains("401") || err.contains("Unauthorized") || err.contains("Session expired")
    } else {
        false
    };

    let account = match &tabular.sync_account {
        Some(a) if !session_expired => a.clone(),
        _ => {
            ui.label(egui::RichText::new("🔒 Login untuk mengakses Teams.").small().weak());
            return;
        }
    };

    // ── Create & Refresh Team row ─────────────────────────────────────────
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.spacing_mut().button_padding = egui::vec2(2.0, 0.0);

        let avail_width = ui.available_width();
        let btn_width = 24.0;
        let refresh_width = 24.0;
        let input_width = (avail_width - btn_width - refresh_width - 16.0).max(40.0);

        ui.add_sized(
            [input_width, 24.0],
            egui::TextEdit::singleline(&mut tabular.new_team_name)
                .hint_text("Team name…")
        );

        let can_create = !tabular.new_team_name.trim().is_empty();
        let create_btn = egui::Button::new(egui::RichText::new("+").strong()).corner_radius(4.0);
        let create_resp = ui.add_enabled_ui(can_create, |ui| {
            ui.add_sized([btn_width, 24.0], create_btn)
        }).inner;

        let create_resp = if can_create {
            create_resp.on_hover_text("Create Team")
        } else {
            create_resp.on_hover_text("Ketik nama Team dulu")
        };

        if create_resp.clicked() {
            create_team(tabular);
        }

        if ui.add_sized(
            [refresh_width, 24.0],
            egui::Button::new(egui::RichText::new("🔄").small()).corner_radius(4.0)
        ).on_hover_text("Refresh Teams").clicked() {
            refresh_teams(tabular);
        }
    });

    ui.add_space(6.0);

    // ── Team list ─────────────────────────────────────────────────────────
    if tabular.teams.is_empty() {
        ui.label(egui::RichText::new("Belum ada Team. Buat Team untuk berbagi folder & Room.").small().weak());
    } else {
        let teams = tabular.teams.clone();
        for team in &teams {
            let is_owner = team.owner_id == account.user_id;

            ui.group(|ui| {
                ui.horizontal(|ui| {
                    let is_expanded = tabular.expanded_team_ids.contains(&team.id);
                    let toggle_symbol = if is_expanded { "⏷" } else { "⏸" };
                    if ui.selectable_label(is_expanded, format!("{} {}", toggle_symbol, team.name)).clicked() {
                        if is_expanded {
                            tabular.expanded_team_ids.remove(&team.id);
                        } else {
                            tabular.expanded_team_ids.insert(team.id.clone());
                            refresh_team_members(tabular, &team.id);
                        }
                    }

                    if is_owner {
                        ui.label(egui::RichText::new("(Owner)").small().weak());
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if is_owner && ui.add(egui::Button::new(egui::RichText::new("🗑").small()).frame(false))
                            .on_hover_text("Delete Team")
                            .clicked()
                        {
                            delete_team(tabular, &team.id);
                        }

                        if ui.button(egui::RichText::new("+ Room").small())
                            .on_hover_text("Buat Room dari Team ini")
                            .clicked()
                        {
                            create_room_from_team(tabular, team);
                        }
                    });
                });

                if let Some(desc) = &team.description
                    && !desc.is_empty()
                {
                    ui.label(egui::RichText::new(desc).small().weak());
                }

                // If expanded: show members & add member inputs
                if tabular.expanded_team_ids.contains(&team.id) {
                    ui.separator();

                    ui.label(egui::RichText::new("Members:").small().strong());

                    if let Some(members) = tabular.team_members.get(&team.id).cloned() {
                        for m in &members {
                            ui.horizontal(|ui| {
                                let label = if let Some(un) = &m.username {
                                    format!("{} (@{})", m.display_name.as_deref().unwrap_or(&m.email), un)
                                } else {
                                    m.display_name.as_deref().unwrap_or(&m.email).to_string()
                                };
                                ui.label(egui::RichText::new(format!("• {}", label)).small());
                                ui.label(egui::RichText::new(format!("[{}]", m.role)).small().weak());

                                if (is_owner && m.user_id != account.user_id)
                                    && ui.add(egui::Button::new(egui::RichText::new("×").small()).frame(false))
                                        .on_hover_text("Remove member")
                                        .clicked()
                                {
                                    remove_team_member(tabular, &team.id, &m.user_id);
                                }
                            });
                        }
                    } else {
                        ui.label(egui::RichText::new("Loading members…").small().weak());
                    }

                    ui.add_space(4.0);

                    // Add member row
                    let mut add_clicked = false;
                    let (mut identifier, mut type_idx, mut role_idx) = tabular
                        .team_add_member_inputs
                        .get(&team.id)
                        .cloned()
                        .unwrap_or_default();

                    ui.horizontal(|ui| {
                        ui.add_sized([100.0, 20.0], egui::TextEdit::singleline(&mut identifier).hint_text("Identifier…"));

                        let id_types = ["email", "username", "phone"];
                        egui::ComboBox::from_id_salt(format!("id_type_{}", team.id))
                            .selected_text(id_types[type_idx])
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut type_idx, 0, "email");
                                ui.selectable_value(&mut type_idx, 1, "username");
                                ui.selectable_value(&mut type_idx, 2, "phone");
                            });

                        let roles = ["member", "admin"];
                        egui::ComboBox::from_id_salt(format!("role_{}", team.id))
                            .selected_text(roles[role_idx])
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut role_idx, 0, "member");
                                ui.selectable_value(&mut role_idx, 1, "admin");
                            });

                        if ui.button("+ Add").clicked() {
                            add_clicked = true;
                        }
                    });

                    tabular.team_add_member_inputs.insert(team.id.clone(), (identifier, type_idx, role_idx));

                    if add_clicked {
                        add_team_member(tabular, &team.id);
                    }
                }
            });
            ui.add_space(2.0);
        }
    }
}

// ─── Actions ──────────────────────────────────────────────────────────────────

pub fn refresh_teams(tabular: &mut Tabular) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.list_teams(&token).await;
        let _ = tx.send(result);
    });

    tabular.teams_receiver = Some(rx);
}

fn create_team(tabular: &mut Tabular) {
    let name = tabular.new_team_name.trim().to_string();
    if name.is_empty() {
        tabular.toasts.warning("Nama Team tidak boleh kosong");
        return;
    }

    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let desc = if tabular.new_team_desc.trim().is_empty() {
        None
    } else {
        Some(tabular.new_team_desc.trim().to_string())
    };

    let req = CreateTeamReq { name, description: desc };
    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.create_team(&token, &req).await;
        let _ = tx.send(result);
    });

    tabular.new_team_name.clear();
    tabular.new_team_desc.clear();
    tabular.team_create_receiver = Some(rx);
}

fn delete_team(tabular: &mut Tabular, team_id: &str) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let team_id_clone = team_id.to_string();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.delete_team(&token, &team_id_clone).await.map(|_| team_id_clone);
        let _ = tx.send(result);
    });

    tabular.team_delete_receiver = Some(rx);
}

fn refresh_team_members(tabular: &mut Tabular, team_id: &str) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let team_id_clone = team_id.to_string();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.list_team_members(&token, &team_id_clone).await;
        let _ = tx.send((team_id_clone, result));
    });

    tabular.team_members_receiver = Some(rx);
}

fn add_team_member(tabular: &mut Tabular, team_id: &str) {
    let (identifier, type_idx, role_idx) = match tabular.team_add_member_inputs.get(team_id) {
        Some(tuple) => tuple.clone(),
        None => return,
    };

    let identifier = identifier.trim().to_string();
    if identifier.is_empty() {
        tabular.toasts.warning("Isi identifier member terlebih dahulu");
        return;
    }

    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let id_types = ["email", "username", "phone"];
    let roles = ["member", "admin"];

    let req = AddTeamMemberReq {
        identifier,
        identifier_type: id_types[type_idx].to_string(),
        role: Some(roles[role_idx].to_string()),
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let team_id_clone = team_id.to_string();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.add_team_member(&token, &team_id_clone, &req).await;
        let _ = tx.send((team_id_clone, result));
    });

    if let Some(tuple) = tabular.team_add_member_inputs.get_mut(team_id) {
        tuple.0.clear();
    }

    tabular.team_add_member_receiver = Some(rx);
}

fn remove_team_member(tabular: &mut Tabular, team_id: &str, user_id: &str) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();

    let team_id_clone = team_id.to_string();
    let user_id_clone = user_id.to_string();
    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.remove_team_member(&token, &team_id_clone, &user_id_clone).await;
        let _ = tx.send((team_id_clone, result));
    });

    tabular.team_add_member_receiver = Some(rx);
}

fn create_room_from_team(tabular: &mut Tabular, team: &RemoteTeam) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let req = CreateTeamRoomReq {
        name: Some(team.name.clone()),
        description: team.description.clone(),
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let team_id = team.id.clone();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client.create_team_room(&token, &team_id, &req).await;
        let _ = tx.send(result);
    });

    tabular.collab_room_create_receiver = Some(rx);
}
