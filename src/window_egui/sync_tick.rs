//! Sync update loop — call from the egui `update()` frame to:
//! 1. Poll CRDT messages from the background WebSocket task
//! 2. Handle manual sync triggers
//! 3. Drain async sync result receivers
//! 4. Periodically auto-sync when online

use eframe::egui;
use log::{info, warn};

/// Maximum number of consecutive token refresh failures before we stop retrying automatically.
const MAX_REFRESH_ATTEMPTS: u32 = 2;

impl super::Tabular {
    /// Call once per frame (from app_impl update) to drive the sync system.
    pub fn tick_sync(&mut self, ctx: &egui::Context) {
        // ── Render collab panel (floating window) ───────────────────────────
        crate::sync::ui_collab::render_collab_panel(self, ctx);

        // ── Poll CRDT messages ───────────────────────────────────────────────
        self.poll_crdt_messages(ctx);

        // ── Handle sync triggers ─────────────────────────────────────────────
        self.handle_sync_triggers();

        // ── Drain async receivers ─────────────────────────────────────────────
        self.drain_sync_receivers();

        // ── Poll room list receiver ───────────────────────────────────────────
        self.poll_collab_receivers();

        // ── Poll teams receivers ──────────────────────────────────────────────
        self.poll_teams_receivers();

        // ── Poll profile (username/phone) save receiver ────────────────────────
        self.poll_profile_receiver();
    }

    fn poll_profile_receiver(&mut self) {
        if let Some(rx) = &self.profile_update_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(user) => {
                    if let Some(account) = &mut self.sync_account {
                        account.username = user.username.clone();
                        account.phone = user.phone.clone();
                        crate::sync::api_client::save_account(account);
                    }
                    self.profile_username_input = user.username.unwrap_or_default();
                    self.profile_phone_input = user.phone.unwrap_or_default();
                    self.toasts.info("Profile saved");
                }
                Err(e) => {
                    warn!("[sync] Profile update error: {}", e);
                    self.toasts.info(format!("Failed to save profile: {}", e));
                    self.check_401_error(&e);
                }
            }
            self.profile_update_receiver = None;
        }
    }

    fn poll_crdt_messages(&mut self, ctx: &egui::Context) {
        let messages = if let Some(crdt) = &mut self.crdt_state {
            crdt.poll()
        } else {
            return;
        };

        for msg in messages {
            use crate::sync::crdt_editor::CrdtMessage;
            match msg {
                CrdtMessage::Connected => {
                    if let Some(crdt) = &mut self.crdt_state {
                        crdt.is_connected = true;
                    }
                    self.toasts.info("Connected to collaboration room");
                    ctx.request_repaint();
                }
                CrdtMessage::Disconnected(reason) => {
                    if let Some(crdt) = &mut self.crdt_state {
                        crdt.is_connected = false;
                    }
                    warn!("[sync] Disconnected: {}", reason);
                    self.toasts.info(format!("Disconnected: {}", reason));
                    ctx.request_repaint();
                }
                CrdtMessage::TextUpdate(new_text) => {
                    // Apply remote text to the main editor buffer
                    // Only update if content actually changed (avoids re-triggering our own broadcast)
                    if self.editor.text != new_text {
                        self.editor.text = new_text.clone();
                        // Keep the cursor position valid
                        let text_len = self.editor.text.len();
                        if self.cursor_position > text_len {
                            self.cursor_position = text_len;
                        }
                        ctx.request_repaint();
                    }
                }
                CrdtMessage::PeersUpdate(peers) => {
                    if let Some(crdt) = &mut self.crdt_state {
                        crdt.peers = peers;
                    }
                    ctx.request_repaint();
                }
                CrdtMessage::Error(e) => {
                    warn!("[sync] CRDT error: {}", e);
                }
            }
        }
    }

    pub fn trigger_refresh_token(&mut self) {
        if self.sync_refresh_receiver.is_some() {
            return; // Refresh already in progress
        }
        // Stop retrying if we've already exhausted max attempts
        if self.sync_refresh_attempt_count >= MAX_REFRESH_ATTEMPTS {
            return;
        }
        let account = match &self.sync_account {
            Some(a) => a.clone(),
            None => return,
        };
        if account.refresh_token.is_empty() {
            warn!("[sync] Cannot refresh token: refresh_token is empty");
            self.sync_login_error = Some("Session expired. Please sign in again.".to_string());
            if !self.sync_session_expired_notified {
                self.sync_session_expired_notified = true;
                self.toasts
                    .warning("⚠️ Session expired. Please sign in again.");
            }
            return;
        }
        info!(
            "[sync] 🔄 Attempting automatic token refresh (attempt {}/{})...",
            self.sync_refresh_attempt_count + 1,
            MAX_REFRESH_ATTEMPTS
        );
        let server = self.sync_server_url.clone();
        let refresh_token = account.refresh_token.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.sync_refresh_receiver = Some(rx);
        crate::sync::spawn_async(async move {
            let client = crate::sync::api_client::ApiClient::new(&server);
            match client.refresh_token(&refresh_token).await {
                Ok(resp) => {
                    let updated = crate::sync::auth::token_to_account(&resp);
                    crate::sync::api_client::save_account(&updated);
                    let _ = tx.send(Ok(updated));
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                }
            }
        });
    }

    fn check_401_error(&mut self, err_msg: &str) {
        if err_msg.contains("401") || err_msg.contains("Unauthorized") {
            warn!("[sync] ⚠️ 401 Unauthorized detected. Triggering token refresh.");
            self.trigger_refresh_token();
        }
    }

    fn handle_sync_triggers(&mut self) {
        let account = match self.sync_account.clone() {
            Some(a) => a,
            None => return,
        };

        // Automatically refresh token if expired, but only if we haven't exhausted retries
        if account.is_token_expired() {
            if self.sync_refresh_attempt_count < MAX_REFRESH_ATTEMPTS {
                self.trigger_refresh_token();
            }
            return;
        }

        let token = account.access_token.clone();
        let server = self.sync_server_url.clone();

        // Connections sync
        if self.sync_trigger_connections {
            self.sync_trigger_connections = false;
            info!("[sync] Triggering connections sync");

            let (tx, rx) = std::sync::mpsc::channel();
            self.sync_connections_receiver = Some(rx);
            let user_id = account.user_id.clone();
            crate::sync::sync_connections::pull_connections_from_server(
                user_id,
                token.clone(),
                server.clone(),
                tx,
            );
        }

        // History push
        if self.sync_trigger_history {
            self.sync_trigger_history = false;
            info!("[sync] Triggering history sync");

            let (tx, rx) = std::sync::mpsc::channel();
            self.sync_queries_push_receiver = Some(rx);
            crate::sync::sync_queries::push_queries_to_server(token.clone(), server.clone(), tx);

            let (tx2, rx2) = std::sync::mpsc::channel();
            self.sync_queries_pull_receiver = Some(rx2);
            crate::sync::sync_queries::pull_queries_from_server(token.clone(), server.clone(), tx2);
        }

        // Queries sync
        if self.sync_trigger_queries {
            self.sync_trigger_queries = false;
            info!("[sync] Triggering queries sync");

            let (tx, rx) = std::sync::mpsc::channel();
            self.sync_queries_push_receiver = Some(rx);
            crate::sync::sync_queries::push_queries_to_server(token.clone(), server.clone(), tx);
        }
    }

    fn drain_sync_receivers(&mut self) {
        // OAuth automatic login callback
        if let Some(rx) = &self.sync_auth_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(token_resp) => {
                    info!("[sync] ✅ OAuth login completed automatically!");
                    let account = crate::sync::auth::token_to_account(&token_resp);
                    crate::sync::api_client::save_account(&account);
                    self.sync_account = Some(account.clone());
                    self.sync_login_pending = false;
                    self.sync_login_error = None;
                    self.sync_status = crate::sync::SyncStatus::Synced;
                    // Reset retry counters when user logs in fresh
                    self.sync_refresh_attempt_count = 0;
                    self.sync_session_expired_notified = false;
                    self.toasts.info(format!("Signed in as {}", account.email));

                    // Trigger automatic sync for connections, history, queries
                    self.sync_trigger_connections = true;
                    self.sync_trigger_history = true;
                    self.sync_trigger_queries = true;
                }
                Err(e) => {
                    warn!("[sync] ❌ OAuth login error: {}", e);
                    self.sync_login_error = Some(e);
                    self.sync_login_pending = false;
                }
            }
            self.sync_auth_receiver = None;
        }

        // Automatic Token Refresh Callback
        if let Some(rx) = &self.sync_refresh_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(updated) => {
                    info!("[sync] ✅ Access token refreshed automatically!");
                    self.sync_account = Some(updated);
                    self.sync_login_error = None;
                    self.sync_status = crate::sync::SyncStatus::Synced;
                    // Reset retry counters on success
                    self.sync_refresh_attempt_count = 0;
                    self.sync_session_expired_notified = false;

                    // Re-trigger sync with refreshed token
                    self.sync_trigger_connections = true;
                    self.sync_trigger_history = true;
                    self.sync_trigger_queries = true;
                    crate::sync::ui_collab::refresh_rooms(self);
                    crate::sync::ui_teams::refresh_teams(self);
                }
                Err(e) => {
                    self.sync_refresh_attempt_count += 1;
                    warn!(
                        "[sync] ❌ Automatic token refresh failed ({}/{}): {}.",
                        self.sync_refresh_attempt_count, MAX_REFRESH_ATTEMPTS, e
                    );
                    self.sync_login_error =
                        Some("Session expired. Please sign in again.".to_string());
                    // Only show the toast once — not on every retry
                    if !self.sync_session_expired_notified {
                        self.sync_session_expired_notified = true;
                        self.toasts
                            .warning("⚠️ Session expired. Please sign in again.");
                    }
                    if self.sync_refresh_attempt_count >= MAX_REFRESH_ATTEMPTS {
                        warn!(
                            "[sync] 🛑 Max refresh attempts reached. Stopping auto-retry. User must sign in manually."
                        );
                    }
                }
            }
            self.sync_refresh_receiver = None;
        }

        // Connections
        if let Some(rx) = &self.sync_connections_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(remote_conns) => {
                    info!("[sync] Received {} remote connections", remote_conns.len());
                    self.sync_status = crate::sync::SyncStatus::Synced;
                    // TODO: merge into local connection list
                }
                Err(e) => {
                    warn!("[sync] Connections sync error: {}", e);
                    self.sync_status = crate::sync::SyncStatus::Error(e.clone());
                    self.check_401_error(&e);
                }
            }
            self.sync_connections_receiver = None;
        }

        // History push
        if let Some(rx) = &self.sync_history_push_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(inserted) => {
                    info!("[sync] Pushed {} history items", inserted);
                    self.sync_status = crate::sync::SyncStatus::Synced;
                }
                Err(e) => {
                    warn!("[sync] History push error: {}", e);
                    self.sync_status = crate::sync::SyncStatus::Error(e.clone());
                    self.check_401_error(&e);
                }
            }
            self.sync_history_push_receiver = None;
        }

        // History pull
        if let Some(rx) = &self.sync_history_pull_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(n) => {
                    info!("[sync] Pulled {} history items", n);
                    self.sync_status = crate::sync::SyncStatus::Synced;
                }
                Err(e) => {
                    warn!("[sync] History pull error: {}", e);
                    self.sync_status = crate::sync::SyncStatus::Error(e.clone());
                    self.check_401_error(&e);
                }
            }
            self.sync_history_pull_receiver = None;
        }

        // Queries push
        if let Some(rx) = &self.sync_queries_push_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(n) => {
                    info!("[sync] Pushed {} queries", n);
                    self.sync_status = crate::sync::SyncStatus::Synced;
                }
                Err(e) => {
                    warn!("[sync] Queries push error: {}", e);
                    self.sync_status = crate::sync::SyncStatus::Error(e.clone());
                    self.check_401_error(&e);
                }
            }
            self.sync_queries_push_receiver = None;
        }

        // Queries pull
        if let Some(rx) = &self.sync_queries_pull_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(n) => {
                    info!("[sync] Pulled {} queries", n);
                    self.sync_status = crate::sync::SyncStatus::Synced;
                }
                Err(e) => {
                    warn!("[sync] Queries pull error: {}", e);
                    self.sync_status = crate::sync::SyncStatus::Error(e.clone());
                    self.check_401_error(&e);
                }
            }
            self.sync_queries_pull_receiver = None;
        }
    }

    fn poll_collab_receivers(&mut self) {
        // Room list refresh
        if let Some(rx) = &self.collab_rooms_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(rooms) => {
                    self.collab_rooms = rooms;
                    info!("[sync] Refreshed {} rooms", self.collab_rooms.len());
                }
                Err(e) => {
                    warn!("[sync] Room list error: {}", e);
                    self.check_401_error(&e.to_string());
                }
            }
            self.collab_rooms_receiver = None;
        }

        // Room creation
        if let Some(rx) = &self.collab_room_create_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(room) => {
                    self.collab_rooms.push(room.clone());
                    self.toasts.info(format!("Room '{}' created!", room.name));
                }
                Err(e) => {
                    warn!("[sync] Room create error: {}", e);
                    self.toasts.info(format!("Failed to create room: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.collab_room_create_receiver = None;
        }

        // Room deletion
        if let Some(rx) = &self.collab_room_delete_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(room_id) => {
                    self.collab_rooms.retain(|r| r.id != room_id);
                    self.toasts.info("Room deleted");
                }
                Err(e) => {
                    warn!("[sync] Room delete error: {}", e);
                    self.toasts.info(format!("Failed to delete room: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.collab_room_delete_receiver = None;
        }
    }

    fn poll_teams_receivers(&mut self) {
        // Teams list refresh
        if let Some(rx) = &self.teams_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(teams) => {
                    self.teams = teams;
                    info!("[sync] Refreshed {} teams", self.teams.len());
                }
                Err(e) => {
                    warn!("[sync] Teams list error: {}", e);
                    self.check_401_error(&e.to_string());
                }
            }
            self.teams_receiver = None;
        }

        // Team creation
        if let Some(rx) = &self.team_create_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(team) => {
                    self.toasts.info(format!("Team '{}' created!", team.name));
                    self.teams.push(team);
                }
                Err(e) => {
                    warn!("[sync] Team create error: {}", e);
                    self.toasts.info(format!("Failed to create team: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.team_create_receiver = None;
        }

        // Team deletion
        if let Some(rx) = &self.team_delete_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(team_id) => {
                    self.teams.retain(|t| t.id != team_id);
                    self.toasts.info("Team deleted");
                }
                Err(e) => {
                    warn!("[sync] Team delete error: {}", e);
                    self.toasts.info(format!("Failed to delete team: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.team_delete_receiver = None;
        }

        // Team members list
        if let Some(rx) = &self.team_members_receiver
            && let Ok((team_id, result)) = rx.try_recv()
        {
            match result {
                Ok(members) => {
                    self.team_members.insert(team_id, members);
                }
                Err(e) => {
                    warn!("[sync] Team members error: {}", e);
                    self.check_401_error(&e.to_string());
                }
            }
            self.team_members_receiver = None;
        }

        // Add/remove member result
        if let Some(rx) = &self.team_add_member_receiver
            && let Ok((_team_id, result)) = rx.try_recv()
        {
            match result {
                Ok(_) => {
                    self.toasts.info("Team member updated");
                    crate::sync::ui_teams::refresh_teams(self);
                }
                Err(e) => {
                    warn!("[sync] Team member error: {}", e);
                    self.toasts.info(format!("Failed to update member: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.team_add_member_receiver = None;
        }
    }

    /// Notify the CRDT engine when the editor text changes (call after each edit).
    pub fn notify_crdt_text_change(&mut self, old_text: String, new_text: String) {
        if let Some(crdt) = &self.crdt_state
            && crdt.is_connected
        {
            crdt.on_local_change(old_text, new_text);
        }
    }

    /// Notify the CRDT engine when the cursor moves.
    pub fn notify_crdt_cursor_move(&mut self, pos: usize) {
        if let Some(crdt) = &self.crdt_state
            && crdt.is_connected
        {
            crdt.on_cursor_move(pos);
        }
    }
}
