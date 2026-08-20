//! One-time migration helpers for pre-vault (`crypto_version = 0`) synced
//! connection rows.
//!
//! Before the vault (`sync::vault_crypto`) existed, a connection's
//! `encrypted_config` was produced by whichever of two schemes the client
//! happened to be built with:
//! 1. Default builds: `base64(plaintext JSON)` — no encryption at all.
//! 2. `--features collab` builds: AES-256-GCM keyed by `SHA256(user_id)` —
//!    "encrypted", but the key was derivable by the server itself.
//!
//! `crypto_version = 0` doesn't distinguish which scheme produced a given
//! row, so migration tries both. This is intentionally the *only* place
//! either legacy scheme still exists in the codebase — used exclusively to
//! read old rows once so they can be re-encrypted under the real vault key
//! and marked `crypto_version = 1`, never for anything new.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use sha2::{Digest, Sha256};

/// Best-effort decrypt of a `crypto_version = 0` connection's `encrypted_config`.
/// Tries the SHA256(user_id)-keyed AES-256-GCM scheme first, then falls back
/// to plain base64. Returns `None` if neither scheme produces valid UTF-8
/// (the row is unreadable and migration should skip it, not merge garbage).
pub fn legacy_decrypt_best_effort(stored: &str, user_id: &str) -> Option<String> {
    legacy_aes_gcm_decrypt(stored, user_id).or_else(|| {
        base64::engine::general_purpose::STANDARD
            .decode(stored)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    })
}

fn legacy_aes_gcm_decrypt(encrypted: &str, user_id: &str) -> Option<String> {
    let data = base64::engine::general_purpose::STANDARD.decode(encrypted).ok()?;
    if data.len() < 12 {
        return None;
    }
    let (nonce_bytes, ciphertext) = data.split_at(12);

    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(&Sha256::digest(user_id.as_bytes()));
    let key: Key<Aes256Gcm> = key_bytes.into();
    let cipher = Aes256Gcm::new(&key);
    let nonce: Nonce<_> = nonce_bytes.try_into().ok()?;

    let plaintext = cipher.decrypt(&nonce, ciphertext).ok()?;
    String::from_utf8(plaintext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_base64_no_op_scheme() {
        let plaintext = r#"{"name":"legacy-plain"}"#;
        let stored = base64::engine::general_purpose::STANDARD.encode(plaintext);
        assert_eq!(legacy_decrypt_best_effort(&stored, "some-user-id").as_deref(), Some(plaintext));
    }

    #[test]
    fn decrypts_sha256_user_id_keyed_scheme() {
        let plaintext = r#"{"name":"legacy-collab"}"#;
        let user_id = "user-123";

        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&Sha256::digest(user_id.as_bytes()));
        let key: Key<Aes256Gcm> = key_bytes.into();
        let cipher = Aes256Gcm::new(&key);
        let nonce_bytes = [7u8; 12];
        let nonce: Nonce<_> = nonce_bytes.into();
        let ciphertext = cipher.encrypt(&nonce, plaintext.as_bytes()).unwrap();
        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);
        let stored = base64::engine::general_purpose::STANDARD.encode(combined);

        assert_eq!(legacy_decrypt_best_effort(&stored, user_id).as_deref(), Some(plaintext));
    }

    #[test]
    fn unreadable_row_returns_none() {
        assert!(legacy_decrypt_best_effort("not even base64!!", "user-id").is_none());
    }
}
