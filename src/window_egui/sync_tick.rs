/// Sync update loop — call from the egui `update()` frame to:
/// 1. Poll CRDT messages from the background WebSocket task
/// 2. Handle manual sync triggers
/// 3. Drain async sync result receivers
/// 4. Periodically auto-sync when online

use eframe::egui;
use log::{debug, info, warn};

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

    fn handle_sync_triggers(&mut self) {
        let account = match self.sync_account.clone() {
            Some(a) => a,
            None => return,
        };

        // Refresh token if expiring
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
                user_id, token.clone(), server.clone(), tx,
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
        if let Some(rx) = &self.sync_auth_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(token_resp) => {
                        info!("[sync] ✅ OAuth login completed automatically!");
                        let account = crate::sync::auth::token_to_account(&token_resp);
                        crate::sync::api_client::save_account(&account);
                        self.sync_account = Some(account.clone());
                        self.sync_login_pending = false;
                        self.sync_login_error = None;
                        self.sync_status = crate::sync::SyncStatus::Synced;
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
        }

        // Connections
        if let Some(rx) = &self.sync_connections_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(remote_conns) => {
                        info!("[sync] Received {} remote connections", remote_conns.len());
                        self.sync_status = crate::sync::SyncStatus::Synced;
                        // TODO: merge into local connection list
                    }
                    Err(e) => {
                        warn!("[sync] Connections sync error: {}", e);
                        self.sync_status = crate::sync::SyncStatus::Error(e);
                    }
                }
                self.sync_connections_receiver = None;
            }
        }

        // History push
        if let Some(rx) = &self.sync_history_push_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(inserted) => {
                        info!("[sync] Pushed {} history items", inserted);
                        self.sync_status = crate::sync::SyncStatus::Synced;
                    }
                    Err(e) => {
                        warn!("[sync] History push error: {}", e);
                        self.sync_status = crate::sync::SyncStatus::Error(e);
                    }
                }
                self.sync_history_push_receiver = None;
            }
        }

        // History pull
        if let Some(rx) = &self.sync_history_pull_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(n) => {
                        info!("[sync] Pulled {} history items", n);
                        self.sync_status = crate::sync::SyncStatus::Synced;
                    }
                    Err(e) => {
                        warn!("[sync] History pull error: {}", e);
                        self.sync_status = crate::sync::SyncStatus::Error(e);
                    }
                }
                self.sync_history_pull_receiver = None;
            }
        }

        // Queries push
        if let Some(rx) = &self.sync_queries_push_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(n) => {
                        info!("[sync] Pushed {} queries", n);
                        self.sync_status = crate::sync::SyncStatus::Synced;
                    }
                    Err(e) => {
                        warn!("[sync] Queries push error: {}", e);
                        self.sync_status = crate::sync::SyncStatus::Error(e);
                    }
                }
                self.sync_queries_push_receiver = None;
            }
        }

        // Queries pull
        if let Some(rx) = &self.sync_queries_pull_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(n) => {
                        info!("[sync] Pulled {} queries", n);
                        self.sync_status = crate::sync::SyncStatus::Synced;
                    }
                    Err(e) => {
                        warn!("[sync] Queries pull error: {}", e);
                        self.sync_status = crate::sync::SyncStatus::Error(e);
                    }
                }
                self.sync_queries_pull_receiver = None;
            }
        }
    }

    fn poll_collab_receivers(&mut self) {
        // Room list refresh
        if let Some(rx) = &self.collab_rooms_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(rooms) => {
                        self.collab_rooms = rooms;
                        info!("[sync] Refreshed {} rooms", self.collab_rooms.len());
                    }
                    Err(e) => {
                        warn!("[sync] Room list error: {}", e);
                    }
                }
                self.collab_rooms_receiver = None;
            }
        }

        // Room creation
        if let Some(rx) = &self.collab_room_create_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(room) => {
                        self.collab_rooms.push(room.clone());
                        self.toasts.info(format!("Room '{}' created!", room.name));
                    }
                    Err(e) => {
                        warn!("[sync] Room create error: {}", e);
                        self.toasts.info(format!("Failed to create room: {}", e));
                    }
                }
                self.collab_room_create_receiver = None;
            }
        }
    }

    /// Notify the CRDT engine when the editor text changes (call after each edit).
    pub fn notify_crdt_text_change(&mut self, old_text: String, new_text: String) {
        if let Some(crdt) = &self.crdt_state {
            if crdt.is_connected {
                crdt.on_local_change(old_text, new_text);
            }
        }
    }

    /// Notify the CRDT engine when the cursor moves.
    pub fn notify_crdt_cursor_move(&mut self, pos: usize) {
        if let Some(crdt) = &self.crdt_state {
            if crdt.is_connected {
                crdt.on_cursor_move(pos);
            }
        }
    }
}
