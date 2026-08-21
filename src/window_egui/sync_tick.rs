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

        // ── Render share folder dialog & add member dialog & delete team dialog ────
        crate::sync::ui_teams::render_share_folder_dialog(self, ctx);
        crate::sync::ui_teams::render_add_member_dialog(self, ctx);
        crate::sync::ui_teams::render_delete_team_dialog(self, ctx);

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

        // ── Vault (E2E encryption) setup/unlock UI receivers ────────────────────
        crate::sync::ui_vault_setup::drain_receivers(self);
        self.poll_vault_team_keys_receiver();
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

            self.maybe_unlock_team_keys();
            self.push_all_connections(&token, &server);

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

        // History push + pull — needs the vault unlocked since query_text is
        // end-to-end encrypted; silently deferred otherwise. (Previously this
        // block mistakenly called sync_queries instead of sync_history — real
        // query history sync was never wired up until now.)
        if self.sync_trigger_history {
            self.sync_trigger_history = false;
            if let (Some(vault), Some(db_pool)) = (self.vault.clone(), self.db_pool.clone()) {
                info!("[sync] Triggering history sync");

                let (tx, rx) = std::sync::mpsc::channel();
                self.sync_history_push_receiver = Some(rx);
                crate::sync::sync_history::push_history_to_server(
                    self.history_items.clone(),
                    vault.account_key.clone(),
                    token.clone(),
                    server.clone(),
                    tx,
                );

                let (tx2, rx2) = std::sync::mpsc::channel();
                self.sync_history_pull_receiver = Some(rx2);
                crate::sync::sync_history::pull_history_from_server(
                    vault.account_key.clone(),
                    token.clone(),
                    server.clone(),
                    db_pool,
                    tx2,
                );
            } else {
                info!("[sync] Deferring history sync — vault is locked or local DB not ready");
            }
        }

        // Queries sync (push + pull) — needs the vault unlocked since
        // query_text is end-to-end encrypted; silently deferred otherwise.
        if self.sync_trigger_queries {
            self.sync_trigger_queries = false;
            if let Some(vault) = self.vault.clone() {
                info!("[sync] Triggering queries sync");
                let team_keys = self.vault_team_keys.clone();
                let shared_folders = self.shared_folders_cache.clone();

                let (tx, rx) = std::sync::mpsc::channel();
                self.sync_queries_push_receiver = Some(rx);
                crate::sync::sync_queries::push_queries_to_server(
                    vault.account_key.clone(),
                    team_keys.clone(),
                    shared_folders.clone(),
                    token.clone(),
                    server.clone(),
                    tx,
                );

                let (tx2, rx2) = std::sync::mpsc::channel();
                self.sync_queries_pull_receiver = Some(rx2);
                crate::sync::sync_queries::pull_queries_from_server(
                    vault.account_key.clone(),
                    team_keys,
                    shared_folders,
                    token.clone(),
                    server.clone(),
                    tx2,
                );
            } else {
                info!("[sync] Deferring queries sync — vault is locked");
            }
        }

        // HTTP requests sync (push then pull) — needs the vault unlocked since
        // headers/body/auth are end-to-end encrypted; silently deferred otherwise.
        if self.sync_trigger_http {
            self.sync_trigger_http = false;
            if let Some(vault) = self.vault.clone() {
                info!("[sync] Triggering HTTP requests sync");
                let team_keys = self.vault_team_keys.clone();
                let shared_folders = self.shared_folders_cache.clone();

                let (tx, rx) = std::sync::mpsc::channel();
                self.sync_http_push_receiver = Some(rx);
                crate::sync::sync_http_requests::push_http_requests_to_server(
                    vault.account_key.clone(),
                    team_keys.clone(),
                    shared_folders.clone(),
                    token.clone(),
                    server.clone(),
                    tx,
                );

                let (tx2, rx2) = std::sync::mpsc::channel();
                self.sync_http_pull_receiver = Some(rx2);
                crate::sync::sync_http_requests::pull_http_requests_from_server(
                    vault.account_key.clone(),
                    team_keys,
                    shared_folders,
                    token.clone(),
                    server.clone(),
                    tx2,
                );
            } else {
                info!("[sync] Deferring HTTP requests sync — vault is locked");
            }
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
                    crate::sync::ui_vault_setup::trigger_vault_check(self);

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
                    self.merge_remote_connections(remote_conns);
                }
                Err(e) => {
                    warn!("[sync] Connections sync error: {}", e);
                    self.sync_status = crate::sync::SyncStatus::Error(e.clone());
                    self.check_401_error(&e);
                }
            }
            self.sync_connections_receiver = None;
        }

        // Connections push
        if let Some(rx) = &self.sync_connections_push_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(n) if n > 0 => info!("[sync] Pushed {} new connection(s) to server", n),
                Ok(_) => {}
                Err(e) => {
                    warn!("[sync] Connections push error: {}", e);
                    self.check_401_error(&e);
                }
            }
            self.sync_connections_push_receiver = None;
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
                    if n > 0 {
                        // Refresh the in-memory list + sidebar tree so newly
                        // pulled items show up without an app restart.
                        crate::sidebar_history::load_query_history(self);
                    }
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

        // HTTP requests push
        if let Some(rx) = &self.sync_http_push_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(n) => info!("[sync] Pushed {} HTTP request(s)", n),
                Err(e) => {
                    warn!("[sync] HTTP requests push error: {}", e);
                    self.check_401_error(&e);
                }
            }
            self.sync_http_push_receiver = None;
        }

        // HTTP requests pull
        if let Some(rx) = &self.sync_http_pull_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(n) => info!("[sync] Pulled {} HTTP request(s)", n),
                Err(e) => {
                    warn!("[sync] HTTP requests pull error: {}", e);
                    self.check_401_error(&e);
                }
            }
            self.sync_http_pull_receiver = None;
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
                    self.teams = teams.clone();
                    info!("[sync] Refreshed {} teams", self.teams.len());
                    if let Some(pool) = self.db_pool.clone() {
                        crate::sync::spawn_async(async move {
                            crate::sync::sync_teams_cache::save_teams_cache(pool.as_ref(), &teams).await;
                        });
                    }
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
                    if let Some(pool) = self.db_pool.clone() {
                        let t_clone = team.clone();
                        crate::sync::spawn_async(async move {
                            crate::sync::sync_teams_cache::save_single_team_cache(pool.as_ref(), &t_clone).await;
                        });
                    }
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
                    if let Some(pool) = self.db_pool.clone() {
                        let t_id = team_id.clone();
                        crate::sync::spawn_async(async move {
                            crate::sync::sync_teams_cache::delete_team_cache(pool.as_ref(), &t_id).await;
                        });
                    }
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
                    self.team_members.insert(team_id.clone(), members.clone());
                    if let Some(pool) = self.db_pool.clone() {
                        let t_id = team_id.clone();
                        crate::sync::spawn_async(async move {
                            crate::sync::sync_teams_cache::save_team_members_cache(pool.as_ref(), &t_id, &members).await;
                        });
                    }
                }
                Err(e) => {
                    warn!("[sync] Team members error: {}", e);
                    self.check_401_error(&e.to_string());
                }
            }
            self.team_members_receiver = None;
        }

        // Add member team receiver
        if let Some(rx) = &self.team_add_member_receiver
            && let Ok((team_id, result)) = rx.try_recv()
        {
            match result {
                Ok(()) => {
                    self.toasts.info("Member added to team!");
                    crate::sync::ui_teams::refresh_team_members(self, &team_id);
                }
                Err(e) => {
                    warn!("[sync] Add member error: {}", e);
                    self.toasts.info(format!("Failed to add member: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.team_add_member_receiver = None;
        }

        // Add member user search autocomplete
        if let Some(rx) = &self.add_member_search_receiver
            && let Ok(result) = rx.try_recv()
        {
            self.add_member_search_in_progress = false;
            match result {
                Ok(users) => {
                    self.add_member_search_results = users;
                }
                Err(e) => {
                    warn!("[sync] User search error: {}", e);
                }
            }
            self.add_member_search_receiver = None;
        }

        // Share/Unshare folder result
        if let Some(rx) = &self.share_folder_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(_) => {
                    self.toasts.info("Folder share updated");
                    crate::sync::ui_teams::refresh_all_shared_folders(self);
                }
                Err(e) => {
                    warn!("[sync] Share folder error: {}", e);
                    self.toasts.info(format!("Failed to update share: {}", e));
                    self.check_401_error(&e.to_string());
                }
            }
            self.share_folder_receiver = None;
        }

        // Shared folders cache refresh
        if let Some(rx) = &self.shared_folders_receiver
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(folders) => {
                    self.shared_folders_cache = folders.clone();
                    info!("[sync] Refreshed {} shared folders", self.shared_folders_cache.len());
                    if let Some(pool) = self.db_pool.clone() {
                        crate::sync::spawn_async(async move {
                            crate::sync::sync_teams_cache::save_shared_folders_cache(pool.as_ref(), &folders).await;
                        });
                    }
                }
                Err(e) => {
                    warn!("[sync] Shared folders list error: {}", e);
                    self.check_401_error(&e.to_string());
                }
            }
            self.shared_folders_receiver = None;
        }
    }

    /// Push local connections the server doesn't have yet (matched by
    /// name + folder_path). Skips any connection whose folder is Team-shared
    /// but whose Team key isn't unlocked locally yet, and any connection type
    /// not synced to the cloud is still sent — the whole config is opaque
    /// ciphertext to the server either way.
    fn push_all_connections(&mut self, token: &str, server: &str) {
        if self.sync_connections_push_receiver.is_some() {
            return; // already in flight
        }
        let vault = match self.vault.clone() {
            Some(v) => v,
            None => return, // nothing to encrypt with yet
        };
        if self.connections.is_empty() {
            return;
        }

        let team_keys = self.vault_team_keys.clone();
        let shared_folders = self.shared_folders_cache.clone();
        let connections = self.connections.clone();
        let token = token.to_string();
        let server = server.to_string();

        let (tx, rx) = std::sync::mpsc::channel();
        self.sync_connections_push_receiver = Some(rx);

        crate::sync::spawn_async(async move {
            let client = crate::sync::api_client::ApiClient::new(&server);
            let remote = match client.list_connections(&token).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };
            let existing: std::collections::HashSet<(String, String)> =
                remote.iter().map(|r| (r.name.clone(), r.folder_path.clone())).collect();

            let mut pushed = 0usize;
            for conn in connections {
                let folder_path = conn.folder.clone().filter(|f| !f.trim().is_empty()).unwrap_or_else(|| "/".to_string());
                if existing.contains(&(conn.name.clone(), folder_path.clone())) {
                    continue;
                }
                let key = match crate::sync::vault_sync::resolve_key_for_folder(
                    &vault.account_key,
                    &team_keys,
                    &shared_folders,
                    "connection",
                    &folder_path,
                ) {
                    Some(k) => k.clone(),
                    None => continue, // Team key not unlocked yet — try again next tick
                };

                let encrypted = match crate::sync::vault_crypto::encrypt_json(&key, &conn) {
                    Ok(e) => e,
                    Err(e) => {
                        log::warn!("[sync] Failed to encrypt connection '{}': {}", conn.name, e);
                        continue;
                    }
                };
                let req = crate::sync::api_client::CreateConnectionReq {
                    name: conn.name.clone(),
                    db_type: format!("{:?}", conn.connection_type),
                    encrypted_config: encrypted,
                    color_tag: None,
                    folder_path: Some(folder_path),
                    crypto_version: 1,
                };
                match client.create_connection(&token, &req).await {
                    Ok(_) => pushed += 1,
                    Err(e) => log::warn!("[sync] Failed to push connection '{}': {}", conn.name, e),
                }
            }
            let _ = tx.send(Ok(pushed));
        });
    }

    /// Decrypt & merge connections pulled from the server into the local
    /// connections list. Each row is decrypted with the AccountKey (personal
    /// folders) or the owning Team's key (Team-shared folders) — resolved via
    /// `vault_sync::resolve_key_for_folder`. Rows this device can't decrypt
    /// yet (vault locked, Team key not granted, or pre-E2E legacy ciphertext)
    /// are skipped rather than guessed at.
    fn merge_remote_connections(&mut self, remote_conns: Vec<crate::sync::api_client::RemoteConnection>) {
        let vault = match self.vault.clone() {
            Some(v) => v,
            None => {
                info!("[sync] Vault locked — deferring connection decrypt until unlocked");
                return;
            }
        };
        let my_user_id = self.sync_account.as_ref().map(|a| a.user_id.clone());
        let token = self.sync_account.as_ref().map(|a| a.access_token.clone());
        let server = self.sync_server_url.clone();

        let existing: std::collections::HashSet<(String, Option<String>)> = self
            .connections
            .iter()
            .map(|c| (c.name.clone(), c.folder.clone()))
            .collect();

        let mut added = 0usize;
        for remote in remote_conns {
            let key = match crate::sync::vault_sync::resolve_key_for_folder(
                &vault.account_key,
                &self.vault_team_keys,
                &self.shared_folders_cache,
                "connection",
                &remote.folder_path,
            ) {
                Some(k) => k.clone(),
                None => {
                    info!("[sync] Skipping Team-shared connection '{}': Team key not unlocked yet", remote.name);
                    continue;
                }
            };

            let mut conn: crate::models::structs::ConnectionConfig = if remote.crypto_version >= 1 {
                match crate::sync::vault_crypto::decrypt_json(&key, &remote.encrypted_config) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("[sync] Failed to decrypt connection '{}': {}", remote.name, e);
                        continue;
                    }
                }
            } else {
                // Legacy (pre-vault) row — best-effort decrypt with the old
                // scheme(s), then queue a re-upload under the real vault key
                // so it migrates for good instead of staying legacy forever.
                let plaintext = match (&my_user_id, crate::sync::legacy_crypto::legacy_decrypt_best_effort(
                    &remote.encrypted_config,
                    my_user_id.as_deref().unwrap_or(""),
                )) {
                    (Some(_), Some(p)) => p,
                    _ => {
                        warn!("[sync] Could not decrypt legacy connection '{}' with any known scheme — skipping", remote.name);
                        continue;
                    }
                };
                match serde_json::from_str::<crate::models::structs::ConnectionConfig>(&plaintext) {
                    Ok(c) => {
                        if let Some(token) = &token {
                            crate::sync::sync_connections::migrate_legacy_connection(
                                remote.id.clone(),
                                c.clone(),
                                key.clone(),
                                token.clone(),
                                server.clone(),
                            );
                        }
                        c
                    }
                    Err(e) => {
                        warn!("[sync] Legacy connection '{}' decrypted but wasn't valid JSON: {}", remote.name, e);
                        continue;
                    }
                }
            };

            if existing.contains(&(conn.name.clone(), conn.folder.clone())) {
                continue; // already present locally — no merge/conflict resolution for connections yet
            }

            conn.id = None; // let the local DB assign a fresh id
            if crate::sidebar_database::save_connection_to_database(self, &conn) {
                added += 1;
            }
        }

        if added > 0 {
            info!("[sync] Merged {} new connection(s) from server", added);
            self.toasts.info(format!("Synced {} connection(s) from cloud", added));
            crate::sidebar_database::load_connections(self);
        }
    }

    /// Opportunistically unlock any Team vault keys we don't have yet (piggy-backs
    /// on the connections-sync cadence rather than polling every frame), and grant
    /// pending members a key for any Team whose key we already hold.
    fn poll_vault_team_keys_receiver(&mut self) {
        if let Some(rx) = &self.vault_team_keys_receiver
            && let Ok(unlocked) = rx.try_recv()
        {
            self.vault_team_keys_receiver = None;
            if !unlocked.is_empty() {
                info!("[sync] Unsealed {} Team vault key(s)", unlocked.len());
                for (team_id, team_key) in unlocked {
                    self.vault_team_keys.insert(team_id.clone(), team_key.clone());

                    let account = match &self.sync_account {
                        Some(a) => a.clone(),
                        None => continue,
                    };
                    let server = self.sync_server_url.clone();
                    crate::sync::spawn_async(async move {
                        let client = crate::sync::api_client::ApiClient::new(&server);
                        if let Err(e) = crate::sync::vault_sync::grant_pending_team_key_envelopes(
                            &client,
                            &account.access_token,
                            &team_id,
                            &team_key,
                        )
                        .await
                        {
                            warn!("[sync] Failed to grant pending Team {} key envelopes: {}", team_id, e);
                        }
                    });
                }
                // Re-pull connections now that we may be able to decrypt more of them.
                self.sync_trigger_connections = true;
            }
        }

        if let Some(rx) = &self.vault_team_bootstrap_receiver
            && let Ok((team_id, result)) = rx.try_recv()
        {
            self.vault_team_bootstrap_receiver = None;
            match result {
                Ok(key) => {
                    info!("[sync] Team {} vault key ready", team_id);
                    self.vault_team_keys.insert(team_id, key);
                    self.sync_trigger_connections = true;
                    self.sync_trigger_http = true;
                }
                Err(e) => warn!("[sync] Failed to bootstrap Team {} vault key: {}", team_id, e),
            }
        }
    }

    /// Kick off unsealing Team vault keys for any Team we belong to but don't
    /// have a key for yet. Called opportunistically alongside connections sync.
    fn maybe_unlock_team_keys(&mut self) {
        if self.vault_team_keys_receiver.is_some() {
            return; // already in flight
        }
        let vault = match self.vault.clone() {
            Some(v) => v,
            None => return,
        };
        let missing: Vec<String> = self
            .teams
            .iter()
            .map(|t| t.id.clone())
            .filter(|id| !self.vault_team_keys.contains_key(id))
            .collect();
        if missing.is_empty() {
            return;
        }
        let account = match &self.sync_account {
            Some(a) => a.clone(),
            None => return,
        };
        let server = self.sync_server_url.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.vault_team_keys_receiver = Some(rx);
        crate::sync::spawn_async(async move {
            let client = crate::sync::api_client::ApiClient::new(&server);
            let unlocked = crate::sync::vault_sync::unlock_all_team_keys(&client, &account.access_token, &vault, &missing).await;
            let _ = tx.send(unlocked);
        });
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
