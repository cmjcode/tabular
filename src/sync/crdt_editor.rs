//! CRDT Collaborative Editor using yrs (Yjs port for Rust).
//!
//! This module handles:
//! - Connecting to a tabular-server collab room via WebSocket
//! - Syncing the yrs Doc (Yjs document) with remote peers
//! - Applying remote updates to the local editor buffer
//! - Broadcasting local editor changes as CRDT updates
//! - Tracking remote user cursors (presence/awareness)

use std::sync::mpsc;

#[cfg(feature = "collab")]
use yrs::{Doc, GetString, ReadTxn, Text, Transact};

/// Represents another user editing in the same room
#[derive(Debug, Clone)]
pub struct CollabPeer {
    pub client_id: u64,
    pub display_name: String,
    pub cursor_pos: Option<usize>,
    pub color: eframe::egui::Color32,
}

/// Messages sent from the background WS task to the UI thread
#[derive(Debug)]
pub enum CrdtMessage {
    /// New text state after applying remote update
    TextUpdate(String),
    /// A peer joined/left or moved their cursor
    PeersUpdate(Vec<CollabPeer>),
    /// Connected to room successfully
    Connected,
    /// Disconnected from room
    Disconnected(String),
    /// Error message
    Error(String),
}

/// Commands sent from UI thread to the background WS task
#[derive(Debug)]
pub enum CrdtCommand {
    /// User typed: broadcast local text change
    LocalTextChanged { old: String, new: String },
    /// User moved cursor
    LocalCursorMoved { pos: usize },
    /// Disconnect from room
    Disconnect,
}

/// State of the CRDT editor (held in main Tabular struct)
pub struct CrdtEditorState {
    pub room_id: String,
    pub is_connected: bool,
    pub peers: Vec<CollabPeer>,
    /// Channel to send commands to the WS background task
    pub command_tx: mpsc::Sender<CrdtCommand>,
    /// Channel to receive updates from the WS background task
    pub message_rx: mpsc::Receiver<CrdtMessage>,
}

impl CrdtEditorState {
    /// Send a text change to the CRDT engine
    pub fn on_local_change(&self, old: String, new: String) {
        let _ = self.command_tx.send(CrdtCommand::LocalTextChanged { old, new });
    }

    /// Send cursor position update
    pub fn on_cursor_move(&self, pos: usize) {
        let _ = self.command_tx.send(CrdtCommand::LocalCursorMoved { pos });
    }

    /// Poll for incoming messages (non-blocking, call from UI frame)
    pub fn poll(&mut self) -> Vec<CrdtMessage> {
        let mut msgs = Vec::new();
        while let Ok(msg) = self.message_rx.try_recv() {
            msgs.push(msg);
        }
        msgs
    }

    pub fn disconnect(&self) {
        let _ = self.command_tx.send(CrdtCommand::Disconnect);
    }
}

/// Connect to a collab room.
/// Returns a CrdtEditorState that can be polled from the UI thread.
/// The WebSocket connection runs in a background tokio task.
pub fn connect_to_room(
    room_id: String,
    server_url: String,
    access_token: String,
    display_name: String,
) -> CrdtEditorState {
    let (command_tx, command_rx) = mpsc::channel::<CrdtCommand>();
    let (message_tx, message_rx) = mpsc::channel::<CrdtMessage>();

    let room_id_clone = room_id.clone();

    // Spawn the background WebSocket task
    super::spawn_async(async move {
        run_ws_session(
            room_id_clone,
            server_url,
            access_token,
            display_name,
            command_rx,
            message_tx,
        )
        .await;
    });

    CrdtEditorState {
        room_id,
        is_connected: false,
        peers: Vec::new(),
        command_tx,
        message_rx,
    }
}

#[cfg(feature = "collab")]
async fn run_ws_session(
    room_id: String,
    server_url: String,
    access_token: String,
    display_name: String,
    command_rx: mpsc::Receiver<CrdtCommand>,
    message_tx: mpsc::Sender<CrdtMessage>,
) {
    use tokio_tungstenite::{connect_async, tungstenite::Message};
    use futures_util::{SinkExt, StreamExt};
    use log::{info, warn};

    // Build WS URL: ws(s)://server/ws/collab/{room_id}?token=...
    let ws_url = server_url
        .replace("http://", "ws://")
        .replace("https://", "wss://");
    let ws_url = format!("{}/ws/collab/{}?token={}", ws_url, room_id, access_token);

    info!("🔌 [crdt] Connecting to room {} at {}", room_id, &ws_url[..ws_url.find('?').unwrap_or(ws_url.len())]);

    let (ws_stream, _) = match connect_async(&ws_url).await {
        Ok(s) => s,
        Err(e) => {
            warn!("❌ [crdt] WS connect failed: {}", e);
            let _ = message_tx.send(CrdtMessage::Error(e.to_string()));
            return;
        }
    };

    let _ = message_tx.send(CrdtMessage::Connected);
    info!("✅ [crdt] Connected to room {}", room_id);

    let doc = Doc::new();
    let text = doc.get_or_insert_text("content");
    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Receive loop
    loop {
        // Poll commands from UI (non-blocking)
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                CrdtCommand::LocalTextChanged { old, new } => {
                    // Compute diff and create CRDT ops
                    if diff_to_yjs_ops(&old, &new, &text, &doc).is_some() {
                        // Encode state update and send to server
                        let update = {
                            let txn = doc.transact();
                            txn.encode_state_as_update_v1(&Default::default())
                        };
                        if !update.is_empty() {
                            let mut msg = vec![1u8]; // type 1 = update
                            msg.extend_from_slice(&update);
                            let _ = ws_tx.send(Message::Binary(msg.into())).await;
                        }
                    }
                }
                CrdtCommand::LocalCursorMoved { pos } => {
                    // Send awareness update with cursor position
                    let awareness_json = serde_json::json!({
                        "cursor": pos,
                        "name": display_name,
                    });
                    let data = serde_json::to_vec(&awareness_json).unwrap_or_default();
                    let mut msg = vec![2u8]; // type 2 = awareness
                    msg.extend_from_slice(&data);
                    let _ = ws_tx.send(Message::Binary(msg.into())).await;
                }
                CrdtCommand::Disconnect => {
                    let _ = ws_tx.send(Message::Close(None)).await;
                    return;
                }
            }
        }

        // Poll WebSocket messages
        match tokio::time::timeout(
            std::time::Duration::from_millis(16),
            ws_rx.next(),
        ).await {
            Ok(Some(Ok(Message::Binary(data)))) => {
                if data.is_empty() { continue; }
                let msg_type = data[0];
                let payload = &data[1..];

                match msg_type {
                    0 | 1 => {
                        // CRDT update from server — apply to doc
                        if let Ok(update) = yrs::updates::decoder::Decode::decode_v1(payload) {
                            let applied = {
                                let mut txn = doc.transact_mut();
                                txn.apply_update(update).is_ok()
                            };
                            if applied {
                                let content = {
                                    let txn2 = doc.transact();
                                    text.get_string(&txn2)
                                };
                                let _ = message_tx.send(CrdtMessage::TextUpdate(content));
                            }
                        }
                    }
                    2 => {
                        // Awareness update from peer — parse cursor info
                        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(payload) {
                            let cid = doc.client_id();
                            let cid_u64: u64 = cid.get();
                            let peer = CollabPeer {
                                client_id: cid_u64,
                                display_name: json["name"].as_str().unwrap_or("Unknown").to_string(),
                                cursor_pos: json["cursor"].as_u64().map(|v| v as usize),
                                color: pick_peer_color(cid_u64),
                            };
                            let _ = message_tx.send(CrdtMessage::PeersUpdate(vec![peer]));
                        }
                    }
                    _ => {}
                }
            }
            Ok(Some(Ok(Message::Close(_)))) => {
                info!("🔌 [crdt] Server closed connection");
                let _ = message_tx.send(CrdtMessage::Disconnected("Server closed connection".to_string()));
                return;
            }
            Ok(Some(Err(e))) => {
                warn!("❌ [crdt] WS error: {}", e);
                let _ = message_tx.send(CrdtMessage::Disconnected(e.to_string()));
                return;
            }
            Ok(None) => {
                let _ = message_tx.send(CrdtMessage::Disconnected("Connection lost".to_string()));
                return;
            }
            Ok(Some(Ok(_))) => {} // Ignore text/ping/pong
            Err(_) => {} // Timeout — continue polling commands
        }
    }
}

#[cfg(not(feature = "collab"))]
async fn run_ws_session(
    _room_id: String,
    _server_url: String,
    _access_token: String,
    _display_name: String,
    _command_rx: mpsc::Receiver<CrdtCommand>,
    message_tx: mpsc::Sender<CrdtMessage>,
) {
    let _ = message_tx.send(CrdtMessage::Error(
        "Collab feature not compiled in. Build with --features collab".to_string(),
    ));
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Diff two strings and produce minimal Yjs Text ops.
/// Returns Some(()) if ops were applied, None if texts are equal.
#[cfg(feature = "collab")]
fn diff_to_yjs_ops(old: &str, new: &str, text: &yrs::TextRef, doc: &Doc) -> Option<()> {
    if old == new {
        return None;
    }

    // Find common prefix length
    let prefix_len = old
        .char_indices()
        .zip(new.char_indices())
        .take_while(|((_, oc), (_, nc))| oc == nc)
        .count();

    // Find common suffix length (excluding prefix)
    let old_suffix = &old[prefix_len..];
    let new_suffix = &new[prefix_len..];
    let suffix_len = old_suffix
        .char_indices()
        .rev()
        .zip(new_suffix.char_indices().rev())
        .take_while(|((_, oc), (_, nc))| oc == nc)
        .count();

    let delete_count = old_suffix.chars().count().saturating_sub(suffix_len);
    let insert_text = &new_suffix[..new_suffix.char_indices().nth(
        new_suffix.chars().count().saturating_sub(suffix_len)
    ).map(|(i, _)| i).unwrap_or(new_suffix.len())];

    let mut txn = doc.transact_mut();
    if delete_count > 0 {
        text.remove_range(&mut txn, prefix_len as u32, delete_count as u32);
    }
    if !insert_text.is_empty() {
        text.insert(&mut txn, prefix_len as u32, insert_text);
    }

    Some(())
}

/// Assign a deterministic color to a peer based on their client_id
pub fn pick_peer_color(client_id: u64) -> eframe::egui::Color32 {
    const COLORS: [(u8, u8, u8); 8] = [
        (99, 132, 255),   // Blue
        (255, 99, 132),   // Pink
        (54, 205, 143),   // Green
        (255, 205, 86),   // Yellow
        (153, 102, 255),  // Purple
        (255, 159, 64),   // Orange
        (50, 210, 210),   // Teal
        (255, 99, 255),   // Magenta
    ];
    let (r, g, b) = COLORS[client_id as usize % COLORS.len()];
    eframe::egui::Color32::from_rgb(r, g, b)
}
