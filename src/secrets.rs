//! Secret storage for credentials (DB passwords, SSH credentials, AI API keys).
//!
//! Primary backend is the OS keychain: macOS/iOS Keychain, Windows Credential
//! Manager, and (opt-in via the `linux-keyring` cargo feature) the freedesktop
//! Secret Service. When the keychain is unavailable or fails, values fall back
//! to `secrets.enc` in the data directory, encrypted with ChaCha20-Poly1305
//! using a locally generated key in `secrets.key` (0600 on Unix).
//!
//! Database/preference rows then hold only [`SECRET_SENTINEL`]; legacy
//! plaintext rows are migrated lazily on load via [`resolve_stored`].
//!
//! Set `TABULAR_DISABLE_KEYRING=1` to force the encrypted-file backend
//! (useful for tests/CI where no keychain is available).

use log::warn;

/// Marker persisted in place of a secret that lives in the secret store.
pub const SECRET_SENTINEL: &str = "__tabular_secret__";

#[allow(dead_code)] // referenced only by keychain-enabled targets
const KEYRING_SERVICE: &str = "id.tabular.database";

/// Stable secret-store name for a connection credential field
/// (`field` is one of `password`, `ssh_password`, `ssh_private_key`).
pub fn connection_secret_name(connection_id: i64, field: &str) -> String {
    format!("conn:{}:{}", connection_id, field)
}

/// Store `value` under `name` and return the string to persist in the
/// database column: the sentinel when stored externally, or the raw value
/// when no secret backend succeeded (so credentials are never lost).
/// An empty value deletes any stored secret.
pub fn store_or_keep(name: &str, value: &str) -> String {
    if value.is_empty() {
        delete_secret(name);
        return String::new();
    }
    // Never store the sentinel itself (defensive: a round-tripped column).
    if value == SECRET_SENTINEL {
        return value.to_string();
    }
    if set_secret(name, value) {
        SECRET_SENTINEL.to_string()
    } else {
        warn!(
            "no secret backend available for '{}'; keeping value in local database",
            name
        );
        value.to_string()
    }
}

/// Resolve a column value read from disk into the real secret.
///
/// Returns `(real_value, column_rewrite)`. `column_rewrite` is `Some(new)`
/// when the column held legacy plaintext that has now been moved into the
/// secret store — the caller should rewrite the column with `new`.
pub fn resolve_stored(name: &str, stored: &str) -> (String, Option<String>) {
    if stored == SECRET_SENTINEL {
        match get_secret(name) {
            Some(v) => (v, None),
            None => {
                warn!("secret '{}' missing from keychain and fallback store", name);
                (String::new(), None)
            }
        }
    } else if stored.is_empty() {
        (String::new(), None)
    } else if set_secret(name, stored) {
        // Legacy plaintext, now migrated to the secret store.
        (stored.to_string(), Some(SECRET_SENTINEL.to_string()))
    } else {
        (stored.to_string(), None)
    }
}

/// Like [`resolve_stored`] but read-only: never migrates legacy plaintext.
/// For secondary single-row readers; migration belongs to the main loader.
pub fn resolve_readonly(name: &str, stored: &str) -> String {
    if stored == SECRET_SENTINEL {
        get_secret(name).unwrap_or_else(|| {
            warn!("secret '{}' missing from keychain and fallback store", name);
            String::new()
        })
    } else {
        stored.to_string()
    }
}

/// Remove all credential secrets belonging to a connection.
pub fn delete_connection_secrets(connection_id: i64) {
    for field in ["password", "ssh_password", "ssh_private_key"] {
        delete_secret(&connection_secret_name(connection_id, field));
    }
}

pub fn set_secret(name: &str, value: &str) -> bool {
    if keyring_enabled() && backend_keyring::set(name, value) {
        // Drop any stale fallback copy so reads can't return an old value.
        backend_file::delete(name);
        return true;
    }
    backend_file::set(name, value)
}

pub fn get_secret(name: &str) -> Option<String> {
    if keyring_enabled()
        && let Some(v) = backend_keyring::get(name)
    {
        return Some(v);
    }
    backend_file::get(name)
}

pub fn delete_secret(name: &str) {
    if keyring_enabled() {
        backend_keyring::delete(name);
    }
    backend_file::delete(name);
}

fn keyring_enabled() -> bool {
    !matches!(
        std::env::var("TABULAR_DISABLE_KEYRING").ok().as_deref(),
        Some("1") | Some("true")
    )
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    all(target_os = "linux", feature = "linux-keyring")
))]
mod backend_keyring {
    use super::KEYRING_SERVICE;
    use log::debug;

    pub fn get(name: &str) -> Option<String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, name).ok()?;
        match entry.get_password() {
            Ok(v) => Some(v),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                debug!("keyring get '{}' failed: {}", name, e);
                None
            }
        }
    }

    pub fn set(name: &str, value: &str) -> bool {
        match keyring::Entry::new(KEYRING_SERVICE, name) {
            Ok(entry) => match entry.set_password(value) {
                Ok(()) => true,
                Err(e) => {
                    debug!("keyring set '{}' failed: {}", name, e);
                    false
                }
            },
            Err(e) => {
                debug!("keyring entry '{}' failed: {}", name, e);
                false
            }
        }
    }

    pub fn delete(name: &str) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, name) {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    all(target_os = "linux", feature = "linux-keyring")
)))]
mod backend_keyring {
    pub fn get(_name: &str) -> Option<String> {
        None
    }
    pub fn set(_name: &str, _value: &str) -> bool {
        false
    }
    pub fn delete(_name: &str) {}
}

/// Encrypted-file fallback: `secrets.enc` is a JSON map of
/// `name -> hex(nonce || ciphertext)` under a ChaCha20-Poly1305 key stored
/// in `secrets.key`. Protects at rest against casual file reads/backups;
/// the key lives next to the data, so the OS keychain remains preferred.
mod backend_file {
    use chacha20poly1305::aead::{Aead, OsRng};
    use chacha20poly1305::{AeadCore, ChaCha20Poly1305, Key, KeyInit, Nonce};
    use log::warn;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const NONCE_LEN: usize = 12;

    fn key_path() -> PathBuf {
        crate::config::get_data_dir().join("secrets.key")
    }

    fn store_path() -> PathBuf {
        crate::config::get_data_dir().join("secrets.enc")
    }

    fn restrict_permissions(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
    }

    fn load_or_create_key() -> Option<Key> {
        let path = key_path();
        if let Ok(content) = std::fs::read_to_string(&path) {
            match hex::decode(content.trim()) {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut key_bytes = [0u8; 32];
                    key_bytes.copy_from_slice(&bytes);
                    return Some(Key::from(key_bytes));
                }
                _ => {
                    warn!("secrets.key is malformed; fallback secret store unavailable");
                    return None;
                }
            }
        }
        let key = ChaCha20Poly1305::generate_key(&mut OsRng);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, hex::encode(key)) {
            warn!("cannot create secrets.key: {}", e);
            return None;
        }
        restrict_permissions(&path);
        Some(key)
    }

    fn load_entries() -> HashMap<String, String> {
        std::fs::read_to_string(store_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save_entries(entries: &HashMap<String, String>) -> bool {
        let path = store_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(entries) {
            Ok(json) => {
                let ok = std::fs::write(&path, json).is_ok();
                if ok {
                    restrict_permissions(&path);
                }
                ok
            }
            Err(_) => false,
        }
    }

    pub fn get(name: &str) -> Option<String> {
        let blob = load_entries().remove(name)?;
        let raw = hex::decode(blob).ok()?;
        if raw.len() <= NONCE_LEN {
            return None;
        }
        let key = load_or_create_key()?;
        let cipher = ChaCha20Poly1305::new(&key);
        let (nonce, ciphertext) = raw.split_at(NONCE_LEN);
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(nonce);
        let plain = cipher.decrypt(&Nonce::from(nonce_bytes), ciphertext).ok()?;
        String::from_utf8(plain).ok()
    }

    pub fn set(name: &str, value: &str) -> bool {
        let Some(key) = load_or_create_key() else {
            return false;
        };
        let cipher = ChaCha20Poly1305::new(&key);
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let Ok(ciphertext) = cipher.encrypt(&nonce, value.as_bytes()) else {
            return false;
        };
        let mut blob = nonce.to_vec();
        blob.extend_from_slice(&ciphertext);
        let mut entries = load_entries();
        entries.insert(name.to_string(), hex::encode(blob));
        save_entries(&entries)
    }

    pub fn delete(name: &str) {
        let mut entries = load_entries();
        if entries.remove(name).is_some() {
            let _ = save_entries(&entries);
        }
    }
}
