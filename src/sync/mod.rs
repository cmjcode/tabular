/// Sync & Collaboration module for tabular-client.
///
/// All submodules compile normally; CRDT/WebSocket/crypto features
/// are only active when built with `--features collab`.
///
/// Offline-first: local SQLite remains the source of truth.
/// Sync runs as background tasks when server is reachable.

pub mod api_client;
pub mod auth;
pub mod crdt_editor;
pub mod sync_connections;
pub mod sync_history;
pub mod sync_queries;
pub mod ui_collab;
pub mod ui_login;

use serde::{Deserialize, Serialize};

// ─── Public top-level state types ────────────────────────────────────────────

/// Sync status shown in the UI
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SyncStatus {
    #[default]
    Offline,           // Server not configured or not reachable
    Syncing,           // Currently syncing
    Synced,            // Last sync successful
    Error(String),     // Last sync failed
}

impl SyncStatus {
    pub fn label(&self) -> &str {
        match self {
            SyncStatus::Offline  => "Offline",
            SyncStatus::Syncing  => "Syncing…",
            SyncStatus::Synced   => "Synced",
            SyncStatus::Error(_) => "Sync Error",
        }
    }

    pub fn is_online(&self) -> bool {
        !matches!(self, SyncStatus::Offline | SyncStatus::Error(_))
    }
}

/// Persisted account info (saved to keyring after login)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabularAccount {
    pub user_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub token_expires_at: i64, // Unix timestamp
}

impl TabularAccount {
    /// Check if access token is expired (with 60-second buffer)
    pub fn is_token_expired(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.token_expires_at < (now + 60)
    }
}

/// Info about a collab room
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CollabRoom {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub owner_id: String,
    pub created_at: String,
}

/// A member of a collab room (presence info)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMember {
    pub user_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub role: String,
    pub is_online: bool,
}
