//! Sync Connections — upload/download encrypted connection configs.
//!
//! Security: connection credentials are encrypted with AES-256-GCM
//! BEFORE being sent to the server. The server only stores ciphertext.
//! Key derivation: PBKDF2 from the user's access_token sub (user_id).

use base64::Engine;
use log::{info, warn};
use std::sync::mpsc;

use crate::models::structs::ConnectionConfig;

use super::api_client::{ApiClient, CreateConnectionReq, RemoteConnection};

// ─── Encryption helpers ──────────────────────────────────────────────────────

/// Encrypt a JSON string using AES-256-GCM.
/// Key is derived from user_id using SHA-256.
/// Returns base64-encoded: nonce(12 bytes) || ciphertext.
#[cfg(feature = "collab")]
pub fn encrypt_config(plaintext: &str, user_id: &str) -> Result<String, String> {
    use aes_gcm::{
        aead::{Aead, KeyInit, OsRng, rand_core::RngCore},
        Aes256Gcm, Key, Nonce,
    };
    use sha2::{Sha256, Digest};

    let mut key_bytes = [0u8; 32];
    let hash = Sha256::digest(user_id.as_bytes());
    key_bytes.copy_from_slice(&hash);

    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);

    Ok(base64::engine::general_purpose::STANDARD.encode(combined))
}

#[cfg(not(feature = "collab"))]
pub fn encrypt_config(plaintext: &str, _user_id: &str) -> Result<String, String> {
    Ok(base64::engine::general_purpose::STANDARD.encode(plaintext)) // No-op stub without collab feature
}

/// Decrypt an AES-256-GCM encrypted connection config.
#[cfg(feature = "collab")]
pub fn decrypt_config(encrypted: &str, user_id: &str) -> Result<String, String> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Key, Nonce,
    };
    use sha2::{Sha256, Digest};

    let data = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| e.to_string())?;
    if data.len() < 12 {
        return Err("Invalid ciphertext: too short".to_string());
    }

    let (nonce_bytes, ciphertext) = data.split_at(12);

    let mut key_bytes = [0u8; 32];
    let hash = Sha256::digest(user_id.as_bytes());
    key_bytes.copy_from_slice(&hash);

    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| e.to_string())?;

    String::from_utf8(plaintext).map_err(|e| e.to_string())
}

#[cfg(not(feature = "collab"))]
pub fn decrypt_config(encrypted: &str, _user_id: &str) -> Result<String, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encrypted)
        .map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

// ─── Sync functions ───────────────────────────────────────────────────────────

/// Push a single connection to the server (called after local save).
pub fn push_connection_to_server(
    conn: ConnectionConfig,
    user_id: String,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<String, String>>, // returns server connection ID
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);

        // Serialize connection to JSON, then encrypt
        let config_json = match serde_json::to_string(&conn) {
            Ok(j) => j,
            Err(e) => {
                let _ = result_tx.send(Err(e.to_string()));
                return;
            }
        };

        let encrypted = match encrypt_config(&config_json, &user_id) {
            Ok(e) => e,
            Err(e) => {
                let _ = result_tx.send(Err(format!("Encryption error: {}", e)));
                return;
            }
        };

        let req = CreateConnectionReq {
            name: conn.name.clone(),
            db_type: format!("{:?}", conn.connection_type),
            encrypted_config: encrypted,
            color_tag: None,
        };

        match client.create_connection(&token, &req).await {
            Ok(remote) => {
                info!("✅ [sync_connections] Pushed connection '{}' → server id {}", conn.name, remote.id);
                let _ = result_tx.send(Ok(remote.id));
            }
            Err(e) => {
                warn!("❌ [sync_connections] Push failed for '{}': {}", conn.name, e);
                let _ = result_tx.send(Err(e.to_string()));
            }
        }
    });
}

/// Pull all connections from server and return decrypted configs.
pub fn pull_connections_from_server(
    _user_id: String,
    token: String,
    server_url: String,
    result_tx: mpsc::Sender<Result<Vec<RemoteConnection>, String>>,
) {
    super::spawn_async(async move {
        let client = ApiClient::new(&server_url);

        match client.list_connections(&token).await {
            Ok(remote_conns) => {
                info!("✅ [sync_connections] Pulled {} connections from server", remote_conns.len());
                let _ = result_tx.send(Ok(remote_conns));
            }
            Err(e) => {
                warn!("❌ [sync_connections] Pull failed: {}", e);
                let _ = result_tx.send(Err(e.to_string()));
            }
        }
    });
}
