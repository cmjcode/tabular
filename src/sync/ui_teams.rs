//! Teams UI panel and sidebar section — manage Teams, members, and create rooms.

use super::api_client::{AddTeamMemberReq, CreateTeamReq, CreateTeamRoomReq, RemoteTeam};
use crate::window_egui::Tabular;
use eframe::egui;

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
    let state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
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

    if header_res.0.clicked()
        && !is_open_before
        && tabular.sync_account.is_some()
        && !session_expired
    {
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
            ui.label(
                egui::RichText::new("🔒 Login untuk mengakses Teams.")
                    .small()
                    .weak(),
            );
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
            egui::TextEdit::singleline(&mut tabular.new_team_name).hint_text("Team name…"),
        );

        let can_create = !tabular.new_team_name.trim().is_empty();
        let create_btn = egui::Button::new(egui::RichText::new("+").strong()).corner_radius(4.0);
        let create_resp = ui
            .add_enabled_ui(can_create, |ui| ui.add_sized([btn_width, 24.0], create_btn))
            .inner;

        let create_resp = if can_create {
            create_resp.on_hover_text("Create Team")
        } else {
            create_resp.on_hover_text("Ketik nama Team dulu")
        };

        if create_resp.clicked() {
            create_team(tabular);
        }

        if ui
            .add_sized(
                [refresh_width, 24.0],
                egui::Button::new(egui::RichText::new("🔄").small()).corner_radius(4.0),
            )
            .on_hover_text("Refresh Teams")
            .clicked()
        {
            refresh_teams(tabular);
        }
    });

    ui.add_space(6.0);

    // ── Team list tree ────────────────────────────────────────────────────
    if tabular.teams.is_empty() {
        ui.label(
            egui::RichText::new("Belum ada Team. Buat Team untuk berbagi folder & Room.")
                .small()
                .weak(),
        );
    } else {
        let teams = tabular.teams.clone();
        for team in &teams {
            let is_owner = team.owner_id == account.user_id;

            let team_header_text = if is_owner {
                format!("👥 {} (Owner)", team.name)
            } else {
                format!("👥 {}", team.name)
            };

            let team_node_id = ui.make_persistent_id(format!("team_node_{}", team.id));
            let team_state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                team_node_id,
                true,
            );
            let is_open_before = team_state.is_open();

            let team_res = team_state.show_header(ui, |ui| {
                ui.label(egui::RichText::new(team_header_text).strong().size(13.0));
                if is_owner {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("🗑").small()).frame(false))
                            .on_hover_text("Delete Team")
                            .clicked()
                        {
                            delete_team(tabular, &team.id);
                        }
                    });
                }
            }).body(|ui| {
                ui.indent(format!("team_body_{}", team.id), |ui| {
                    if let Some(desc) = &team.description
                        && !desc.is_empty()
                    {
                        ui.label(egui::RichText::new(desc).small().weak());
                        ui.add_space(2.0);
                    }

                    // ── 1. Members Sub-Tree Node ──────────────────────────────────
                    let member_count = tabular.team_members.get(&team.id).map(|m| m.len()).unwrap_or(0);
                    let members_header_title = format!("Members ({})", member_count);
                    let members_node_id = ui.make_persistent_id(format!("team_members_node_{}", team.id));
                    let members_state = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        members_node_id,
                        true,
                    );

                    let mut open_add_dialog = false;
                    members_state.show_header(ui, |ui| {
                        ui.label(egui::RichText::new(members_header_title).strong().small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new(egui::RichText::new("+").strong().small()).corner_radius(3.0))
                                .on_hover_text("Tambah Member (Popup)")
                                .clicked()
                            {
                                open_add_dialog = true;
                            }
                        });
                    }).body(|ui| {
                        ui.indent(format!("members_body_{}", team.id), |ui| {
                            if let Some(members) = tabular.team_members.get(&team.id).cloned() {
                                if members.is_empty() {
                                    ui.label(egui::RichText::new("Belum ada member.").small().weak());
                                } else {
                                    for m in &members {
                                        ui.horizontal(|ui| {
                                            let label = if let Some(un) = &m.username {
                                                format!("{} (@{})", m.display_name.as_deref().unwrap_or(&m.email), un)
                                            } else {
                                                m.display_name.as_deref().unwrap_or(&m.email).to_string()
                                            };
                                            ui.label(egui::RichText::new(format!("• {}", label)).small());
                                            ui.label(egui::RichText::new(format!("[{}]", m.role)).small().weak());

                                            if is_owner && m.user_id != account.user_id {
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.add(egui::Button::new(egui::RichText::new("🗑").small()).frame(false))
                                                        .on_hover_text("Remove member")
                                                        .clicked()
                                                    {
                                                        remove_team_member(tabular, &team.id, &m.user_id);
                                                    }
                                                });
                                            }
                                        });
                                    }
                                }
                            } else {
                                ui.label(egui::RichText::new("Loading members…").small().weak());
                            }
                        });
                    });

                    if open_add_dialog {
                        tabular.add_member_target_team_id = Some(team.id.clone());
                        tabular.add_member_identifier.clear();
                        tabular.show_add_member_dialog = true;
                    }

                    ui.add_space(4.0);

                    // ── 2. Shares Sub-Tree Node ───────────────────────────────────
                    let team_shares: Vec<_> = tabular.shared_folders_cache
                        .iter()
                        .filter(|sf| sf.team_id == team.id)
                        .cloned()
                        .collect();

                    let shares_header_title = format!("Shares ({})", team_shares.len());
                    let shares_node_id = ui.make_persistent_id(format!("team_shares_node_{}", team.id));
                    let shares_state = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        shares_node_id,
                        true,
                    );

                    let mut add_share_clicked = false;
                    shares_state.show_header(ui, |ui| {
                        ui.label(egui::RichText::new(shares_header_title).strong().small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new(egui::RichText::new("+").strong().small()).corner_radius(3.0))
                                .on_hover_text("Share Folder (Connection/Query/HTTP)")
                                .clicked()
                            {
                                add_share_clicked = true;
                            }
                        });
                    }).body(|ui| {
                        ui.indent(format!("shares_body_{}", team.id), |ui| {
                            if team_shares.is_empty() {
                                ui.label(egui::RichText::new("Belum ada folder Connection, Query, atau HTTP yang dibagikan.").small().weak());
                            } else {
                                for sf in &team_shares {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(format!("📁 [{}] {}", sf.resource_type.to_uppercase(), sf.folder_path)).small());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add(egui::Button::new(egui::RichText::new("🗑").small()).frame(false))
                                                .on_hover_text("Unshare folder")
                                                .clicked()
                                            {
                                                unshare_folder_action(tabular, &team.id, &sf.id);
                                            }
                                        });
                                    });
                                }
                            }
                        });
                    });

                    if add_share_clicked {
                        tabular.share_folder_selected_team_id = Some(team.id.clone());
                        tabular.show_share_folder_dialog = true;
                    }

                    ui.add_space(4.0);

                    // ── 3. Rooms Sub-Tree Node ────────────────────────────────────
                    let team_rooms: Vec<_> = tabular.collab_rooms
                        .iter()
                        .filter(|r| r.team_id.as_deref() == Some(&team.id))
                        .cloned()
                        .collect();

                    let rooms_header_title = format!("Rooms ({})", team_rooms.len());
                    let rooms_node_id = ui.make_persistent_id(format!("team_rooms_node_{}", team.id));
                    let rooms_state = egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(),
                        rooms_node_id,
                        true,
                    );

                    let mut add_room_clicked = false;
                    rooms_state.show_header(ui, |ui| {
                        ui.label(egui::RichText::new(rooms_header_title).strong().small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(egui::Button::new(egui::RichText::new("+").strong().small()).corner_radius(3.0))
                                .on_hover_text("Buat Room baru untuk Team ini")
                                .clicked()
                            {
                                add_room_clicked = true;
                            }
                        });
                    }).body(|ui| {
                        ui.indent(format!("rooms_body_{}", team.id), |ui| {
                            if team_rooms.is_empty() {
                                ui.label(egui::RichText::new("Belum ada Room untuk Team ini.").small().weak());
                            } else {
                                for r in &team_rooms {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(format!("☁ {}", r.name)).small());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add(egui::Button::new(egui::RichText::new("🗑").small()).frame(false))
                                                .on_hover_text("Delete room")
                                                .clicked()
                                            {
                                                super::ui_collab::delete_room(tabular, &r.id);
                                            }

                                            let is_current = tabular.crdt_state
                                                .as_ref()
                                                .map(|c| c.room_id == r.id)
                                                .unwrap_or(false);

                                            if is_current {
                                                ui.label(egui::RichText::new("● Connected").color(egui::Color32::from_rgb(72, 199, 116)).small());
                                            } else if ui.small_button("Join").clicked() {
                                                super::ui_collab::join_room(tabular, r);
                                            }
                                        });
                                    });
                                }
                            }
                        });
                    });

                    if add_room_clicked {
                        create_room_from_team(tabular, team);
                    }
                });
            });

            if team_res.0.clicked() && !is_open_before {
                refresh_team_members(tabular, &team.id);
                refresh_all_shared_folders(tabular);
            }

            ui.add_space(4.0);
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

    let req = CreateTeamReq {
        name,
        description: desc,
    };
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
        let result = client
            .delete_team(&token, &team_id_clone)
            .await
            .map(|_| team_id_clone);
        let _ = tx.send(result);
    });

    tabular.team_delete_receiver = Some(rx);
}

pub fn refresh_team_members(tabular: &mut Tabular, team_id: &str) {
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
        let result = client
            .remove_team_member(&token, &team_id_clone, &user_id_clone)
            .await;
        let _ = tx.send((team_id_clone, result));
    });

    tabular.team_add_member_receiver = Some(rx);
}

fn create_room_from_team(tabular: &mut Tabular, team: &RemoteTeam) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let base_name = team.name.clone();
    let existing_names: Vec<String> = tabular
        .collab_rooms
        .iter()
        .filter(|r| r.team_id.as_deref() == Some(&team.id))
        .map(|r| r.name.clone())
        .collect();

    let mut room_name = base_name.clone();
    let mut counter = 2;
    while existing_names.contains(&room_name) {
        room_name = format!("{} {}", base_name, counter);
        counter += 1;
    }

    let req = CreateTeamRoomReq {
        name: Some(room_name),
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

pub fn render_share_folder_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_share_folder_dialog {
        return;
    }

    let preset_target = tabular.share_folder_target.clone();

    let mut close_requested = false;
    let mut share_clicked = false;
    let mut unshare_id: Option<(String, String)> = None;

    let resource_types = ["connection", "query", "http"];
    let resource_labels = ["Connection", "Query", "HTTP Request"];

    egui::Window::new("🤝 Share Folder to Team")
        .id(egui::Id::new("share_folder_dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(360.0);

            if let Some((res_type, folder_path)) = &preset_target {
                ui.label(
                    egui::RichText::new(format!("Share {} folder:", res_type.to_uppercase())).strong(),
                );
                ui.label(
                    egui::RichText::new(folder_path)
                        .monospace()
                        .color(egui::Color32::from_rgb(100, 180, 255)),
                );
            } else {
                ui.label(egui::RichText::new("Tipe Resource:").small().strong());
                egui::ComboBox::from_id_salt("share_folder_dialog_res_type")
                    .selected_text(resource_labels[tabular.share_folder_type_idx.min(2)])
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut tabular.share_folder_type_idx, 0, "Connection");
                        ui.selectable_value(&mut tabular.share_folder_type_idx, 1, "Query");
                        ui.selectable_value(&mut tabular.share_folder_type_idx, 2, "HTTP Request");
                    });

                ui.add_space(4.0);
                ui.label(egui::RichText::new("Folder Path:").small().strong());

                let active_res_type = resource_types[tabular.share_folder_type_idx.min(2)];
                let available_folders = get_available_folder_suggestions(tabular, active_res_type);

                let selected_folder_text = if tabular.share_folder_path_input.trim().is_empty() {
                    "Pilih Folder dari daftar...".to_string()
                } else {
                    tabular.share_folder_path_input.clone()
                };

                egui::ComboBox::from_id_salt("share_folder_path_combobox")
                    .selected_text(&selected_folder_text)
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        if available_folders.is_empty() {
                            ui.label(egui::RichText::new("Belum ada folder lokal").small().weak());
                        } else {
                            for folder in &available_folders {
                                let is_selected = tabular.share_folder_path_input == *folder;
                                if ui.selectable_label(is_selected, folder).clicked() {
                                    tabular.share_folder_path_input = folder.clone();
                                }
                            }
                        }
                    });

                ui.add_space(2.0);
                ui.add_sized(
                    [ui.available_width(), 24.0],
                    egui::TextEdit::singleline(&mut tabular.share_folder_path_input)
                        .hint_text("Atau ketik folder baru (cth: /Production)"),
                );
            }

            ui.add_space(8.0);

            if tabular.teams.is_empty() {
                ui.label(
                    egui::RichText::new("Anda belum memiliki atau bergabung di Team manapun.")
                        .weak()
                        .small(),
                );
            } else {
                ui.label(egui::RichText::new("Pilih Team:").small().strong());

                let selected_team_id = tabular
                    .share_folder_selected_team_id
                    .clone()
                    .unwrap_or_else(|| tabular.teams[0].id.clone());

                let selected_team_name = tabular
                    .teams
                    .iter()
                    .find(|t| t.id == selected_team_id)
                    .map(|t| t.name.as_str())
                    .unwrap_or("Pilih Team...");

                egui::ComboBox::from_id_salt("share_target_team")
                    .selected_text(selected_team_name)
                    .show_ui(ui, |ui| {
                        for team in &tabular.teams {
                            if ui
                                .selectable_value(
                                    &mut tabular.share_folder_selected_team_id,
                                    Some(team.id.clone()),
                                    &team.name,
                                )
                                .clicked()
                            {}
                        }
                    });

                ui.add_space(6.0);

                if ui
                    .add(crate::window_egui::style::btn_primary_ctx(
                        ui.ctx(),
                        "🤝 Share to Team",
                    ))
                    .clicked()
                {
                    share_clicked = true;
                }
            }

            ui.separator();
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Shared dengan Team:").small().strong());

            let (active_res_type, active_folder_path) = match &preset_target {
                Some((rt, fp)) => (rt.clone(), fp.clone()),
                None => (
                    resource_types[tabular.share_folder_type_idx.min(2)].to_string(),
                    tabular.share_folder_path_input.trim().to_string(),
                ),
            };

            let current_shares: Vec<_> = tabular
                .shared_folders_cache
                .iter()
                .filter(|sf| {
                    if active_folder_path.is_empty() {
                        true
                    } else {
                        sf.resource_type == active_res_type && sf.folder_path == active_folder_path
                    }
                })
                .cloned()
                .collect();

            if current_shares.is_empty() {
                ui.label(
                    egui::RichText::new("Belum ada folder yang dibagikan.")
                        .small()
                        .weak(),
                );
            } else {
                for sf in &current_shares {
                    let team_name = tabular
                        .teams
                        .iter()
                        .find(|t| t.id == sf.team_id)
                        .map(|t| t.name.as_str())
                        .unwrap_or(&sf.team_id);

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "• [{}] {} ➔ {}",
                                sf.resource_type.to_uppercase(),
                                sf.folder_path,
                                team_name
                            ))
                            .small(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new("🗑").small())
                                        .frame(false),
                                )
                                .on_hover_text("Unshare folder")
                                .clicked()
                            {
                                unshare_id = Some((sf.team_id.clone(), sf.id.clone()));
                            }
                        });
                    });
                }
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Tutup").clicked() {
                        close_requested = true;
                    }
                });
            });
        });

    if close_requested {
        tabular.show_share_folder_dialog = false;
        tabular.share_folder_target = None;
    }

    if share_clicked && !tabular.teams.is_empty() {
        let (res_type, folder_path) = match &preset_target {
            Some((rt, fp)) => (rt.clone(), fp.clone()),
            None => (
                resource_types[tabular.share_folder_type_idx.min(2)].to_string(),
                tabular.share_folder_path_input.trim().to_string(),
            ),
        };

        if folder_path.is_empty() {
            tabular.toasts.warning("Folder path tidak boleh kosong");
        } else {
            let team_id = tabular
                .share_folder_selected_team_id
                .clone()
                .unwrap_or_else(|| tabular.teams[0].id.clone());

            share_folder_action(tabular, &team_id, &res_type, &folder_path);
            if preset_target.is_none() {
                tabular.share_folder_path_input.clear();
            }
        }
    }

    if let Some((team_id, folder_id)) = unshare_id {
        unshare_folder_action(tabular, &team_id, &folder_id);
    }
}

pub fn share_folder_action(
    tabular: &mut Tabular,
    team_id: &str,
    resource_type: &str,
    folder_path: &str,
) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let req = super::api_client::ShareFolderReq {
        resource_type: resource_type.to_string(),
        folder_path: folder_path.to_string(),
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let team_id_clone = team_id.to_string();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client
            .share_folder(&token, &team_id_clone, &req)
            .await
            .map(|_| ());
        let _ = tx.send(result);
    });

    tabular.share_folder_receiver = Some(rx);

    // Bootstrap (or fetch) this Team's vault key, grant it to every current
    // member, and re-encrypt anything already synced to this folder under the
    // personal AccountKey so it moves onto the Team key instead of staying
    // owner-only-readable forever.
    if let Some(vault) = tabular.vault.clone() {
        let team_id_owned = team_id.to_string();
        let my_user_id = account.user_id.clone();
        let token2 = account.access_token.clone();
        let server2 = tabular.sync_server_url.clone();
        let resource_type_owned = resource_type.to_string();
        let folder_path_owned = folder_path.to_string();

        // Local connections in this folder — captured now (sync, on the UI
        // thread) since the spawned task below can't safely borrow `tabular`.
        let matching_connections: Vec<crate::models::structs::ConnectionConfig> = if resource_type == "connection" {
            tabular
                .connections
                .iter()
                .filter(|c| {
                    c.folder
                        .clone()
                        .filter(|f| !f.trim().is_empty())
                        .unwrap_or_else(|| "/".to_string())
                        == folder_path
                })
                .cloned()
                .collect()
        } else {
            Vec::new()
        };

        let (tx2, rx2) = std::sync::mpsc::channel();
        super::spawn_async(async move {
            let client = super::api_client::ApiClient::new(&server2);
            let mut team_keys = std::collections::HashMap::new();
            let result = async {
                let key = super::vault_sync::ensure_own_team_key(
                    &client, &token2, &my_user_id, &vault, &team_id_owned, &mut team_keys,
                )
                .await?;
                super::vault_sync::grant_pending_team_key_envelopes(&client, &token2, &team_id_owned, &key).await?;
                Ok::<_, anyhow::Error>(key)
            }
            .await
            .map_err(|e| e.to_string());

            if let Ok(key) = &result {
                match resource_type_owned.as_str() {
                    "connection" => super::sync_connections::reencrypt_folder_to_server(
                        matching_connections,
                        key.clone(),
                        folder_path_owned.clone(),
                        token2.clone(),
                        server2.clone(),
                    ),
                    "http" => super::sync_http_requests::reencrypt_folder_to_server(
                        key.clone(),
                        folder_path_owned.clone(),
                        token2.clone(),
                        server2.clone(),
                    ),
                    _ => {}
                }
            }

            let _ = tx2.send((team_id_owned, result));
        });
        tabular.vault_team_bootstrap_receiver = Some(rx2);
    }
}

pub fn unshare_folder_action(tabular: &mut Tabular, team_id: &str, folder_id: &str) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let (tx, rx) = std::sync::mpsc::channel();

    let team_id_clone = team_id.to_string();
    let folder_id_clone = folder_id.to_string();
    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let result = client
            .unshare_folder(&token, &team_id_clone, &folder_id_clone)
            .await;
        let _ = tx.send(result);
    });

    tabular.share_folder_receiver = Some(rx);
}

pub fn refresh_all_shared_folders(tabular: &mut Tabular) {
    let account = match &tabular.sync_account {
        Some(a) => a.clone(),
        None => return,
    };

    let token = account.access_token.clone();
    let server = tabular.sync_server_url.clone();
    let teams = tabular.teams.clone();

    if teams.is_empty() {
        return;
    }

    let (tx, rx) = std::sync::mpsc::channel();

    super::spawn_async(async move {
        let client = super::api_client::ApiClient::new(&server);
        let mut all_shared = Vec::new();
        for t in &teams {
            if let Ok(folders) = client.list_shared_folders(&token, &t.id).await {
                all_shared.extend(folders);
            }
        }
        let _ = tx.send(Ok(all_shared));
    });

    tabular.shared_folders_receiver = Some(rx);
}

pub fn render_add_member_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_add_member_dialog {
        return;
    }

    let team_id = match &tabular.add_member_target_team_id {
        Some(id) => id.clone(),
        None => {
            tabular.show_add_member_dialog = false;
            return;
        }
    };

    let team_name = tabular
        .teams
        .iter()
        .find(|t| t.id == team_id)
        .map(|t| t.name.clone())
        .unwrap_or_else(|| "Team".to_string());

    let mut close_requested = false;
    let mut add_clicked = false;
    let mut search_triggered_query: Option<String> = None;

    egui::Window::new(format!("👥 Add Member to {}", team_name))
        .id(egui::Id::new("add_team_member_dialog"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(380.0);
            ui.spacing_mut().item_spacing.y = 8.0;

            ui.label(
                egui::RichText::new("Cari Member (Min. 5 Karakter Email / Name / Phone):")
                    .small()
                    .strong(),
            );

            ui.add_sized(
                [ui.available_width(), 26.0],
                egui::TextEdit::singleline(&mut tabular.add_member_identifier)
                    .hint_text("Ketik min. 5 karakter untuk mencari…"),
            );

            let trimmed_input = tabular.add_member_identifier.trim().to_string();
            let char_count = trimmed_input.chars().count();

            // Trigger search if >= 5 chars and query changed
            if char_count >= 5
                && trimmed_input != tabular.add_member_search_query
                && !tabular.add_member_search_in_progress
            {
                search_triggered_query = Some(trimmed_input.clone());
            }

            // Role selection row (Tipe dropdown is removed)
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Role:").small().strong());
                let roles = ["member", "admin"];
                egui::ComboBox::from_id_salt("add_member_dialog_role")
                    .selected_text(roles[tabular.add_member_role_idx])
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut tabular.add_member_role_idx, 0, "member");
                        ui.selectable_value(&mut tabular.add_member_role_idx, 1, "admin");
                    });
            });

            // Autocomplete Candidate Dropdown / Box
            if char_count < 5 {
                ui.label(
                    egui::RichText::new(format!(
                        "Ketik {} karakter lagi untuk mencari…",
                        5 - char_count
                    ))
                    .small()
                    .weak(),
                );
            } else if tabular.add_member_search_in_progress {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new("Mencari member di server…")
                            .small()
                            .weak(),
                    );
                });
            } else if !tabular.add_member_search_results.is_empty() {
                ui.group(|ui| {
                    ui.set_max_height(140.0);
                    ui.label(
                        egui::RichText::new("Hasil Pencarian (Pilih Pengguna):")
                            .small()
                            .strong(),
                    );
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let candidates = tabular.add_member_search_results.clone();
                        for u in &candidates {
                            let display_name = u.display_name.as_deref().unwrap_or(&u.email);
                            let username_or_phone =
                                u.phone.as_deref().or(u.username.as_deref()).unwrap_or("");

                            let item_text = if username_or_phone.is_empty() {
                                format!("👤 {} | ✉️ {}", display_name, u.email)
                            } else {
                                format!(
                                    "👤 {} | ✉️ {} | 📞 {}",
                                    display_name, u.email, username_or_phone
                                )
                            };

                            let is_selected = tabular.add_member_identifier == u.email;
                            if ui
                                .selectable_label(
                                    is_selected,
                                    egui::RichText::new(item_text).small(),
                                )
                                .clicked()
                            {
                                tabular.add_member_identifier = u.email.clone();
                            }
                        }
                    });
                });
            } else if char_count >= 5 {
                ui.label(
                    egui::RichText::new("Tidak ada pengguna yang cocok.")
                        .small()
                        .weak(),
                );
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_add = !tabular.add_member_identifier.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_add,
                            crate::window_egui::style::btn_primary_ctx(
                                ui.ctx(),
                                "➕ Tambah Member",
                            ),
                        )
                        .clicked()
                    {
                        add_clicked = true;
                    }

                    if ui.button("Batal").clicked() {
                        close_requested = true;
                    }
                });
            });
        });

    if let Some(query) = search_triggered_query {
        if let Some(account) = &tabular.sync_account {
            tabular.add_member_search_query = query.clone();
            tabular.add_member_search_in_progress = true;

            let token = account.access_token.clone();
            let server = tabular.sync_server_url.clone();
            let (tx, rx) = std::sync::mpsc::channel();

            super::spawn_async(async move {
                let client = super::api_client::ApiClient::new(&server);
                let result = client.search_users(&token, &query).await;
                let _ = tx.send(result);
            });

            tabular.add_member_search_receiver = Some(rx);
        }
    }

    if close_requested {
        tabular.show_add_member_dialog = false;
        tabular.add_member_target_team_id = None;
        tabular.add_member_identifier.clear();
        tabular.add_member_search_results.clear();
        tabular.add_member_search_query.clear();
        tabular.add_member_search_in_progress = false;
    }

    if add_clicked {
        let identifier = tabular.add_member_identifier.trim().to_string();
        if !identifier.is_empty() {
            let roles = ["member", "admin"];

            let req = AddTeamMemberReq {
                identifier,
                identifier_type: "auto".to_string(),
                role: Some(roles[tabular.add_member_role_idx].to_string()),
            };

            if let Some(account) = &tabular.sync_account {
                let token = account.access_token.clone();
                let server = tabular.sync_server_url.clone();
                let (tx, rx) = std::sync::mpsc::channel();

                let team_id_clone = team_id.clone();
                super::spawn_async(async move {
                    let client = super::api_client::ApiClient::new(&server);
                    let result = client.add_team_member(&token, &team_id_clone, &req).await;
                    let _ = tx.send((team_id_clone, result));
                });

                tabular.team_add_member_receiver = Some(rx);
            }
        }

        tabular.show_add_member_dialog = false;
        tabular.add_member_target_team_id = None;
        tabular.add_member_identifier.clear();
        tabular.add_member_search_results.clear();
        tabular.add_member_search_query.clear();
        tabular.add_member_search_in_progress = false;
    }
}

// ─── Folder Suggestion Helpers ────────────────────────────────────────────────

fn get_available_folder_suggestions(tabular: &Tabular, resource_type: &str) -> Vec<String> {
    let mut folders = std::collections::BTreeSet::new();

    match resource_type {
        "connection" => {
            for f in &tabular.connection_folders {
                if !f.trim().is_empty() {
                    folders.insert(f.clone());
                }
            }
            for conn in &tabular.connections {
                if let Some(f) = &conn.folder {
                    if !f.trim().is_empty() {
                        folders.insert(f.clone());
                    }
                }
            }
        }
        "query" => {
            let query_dir = crate::directory::get_query_dir();
            collect_query_subfolders(&query_dir, &query_dir, &mut folders);
        }
        "http" => {
            for ws in &tabular.yaak_workspaces {
                collect_http_subfolders(&ws.folders, &mut folders);
            }
        }
        _ => {}
    }

    for sf in &tabular.shared_folders_cache {
        if sf.resource_type == resource_type && !sf.folder_path.trim().is_empty() {
            folders.insert(sf.folder_path.clone());
        }
    }

    folders.into_iter().collect()
}

fn collect_query_subfolders(
    base_dir: &std::path::Path,
    current_dir: &std::path::Path,
    out: &mut std::collections::BTreeSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(current_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(rel) = path.strip_prefix(base_dir) {
                    let rel_str = rel.to_string_lossy().to_string();
                    if !rel_str.is_empty() {
                        out.insert(format!("/{}", rel_str.replace('\\', "/")));
                    }
                }
                collect_query_subfolders(base_dir, &path, out);
            }
        }
    }
}

fn collect_http_subfolders(
    folders: &[crate::http_collection::HttpFolder],
    out: &mut std::collections::BTreeSet<String>,
) {
    for f in folders {
        if !f.name.trim().is_empty() {
            let name = f.name.trim_start_matches('/');
            out.insert(format!("/{}", name));
        }
        collect_http_subfolders(&f.children, out);
    }
}

