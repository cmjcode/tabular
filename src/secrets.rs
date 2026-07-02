//! Secret storage for credentials (DB passwords, SSH credentials, AI API keys).
//!
//! All secrets live in `secrets.enc` (data directory), encrypted with
//! ChaCha20-Poly1305 under a single **master key**. The master key is held
//! in the OS keychain as ONE item (macOS/iOS Keychain, Windows Credential
//! Manager, Linux Secret Service via the `linux-keyring` feature) and is
//! cached in-process, so the keychain is touched at most once per run —
//! one permission prompt, not one per credential. Without a keychain the
//! key falls back to `secrets.key` on disk (0600 on Unix).
//!
//! An earlier layout stored each secret as its own keychain item, which on
//! unsigned dev builds triggered a permission popup per credential per
//! rebuild; [`get_secret`] migrates those items into `secrets.enc` and
//! deletes them.
//!
//! Database/preference rows hold only [`SECRET_SENTINEL`]; legacy plaintext
//! rows are migrated lazily on load via [`resolve_stored`].
//!
//! Debug builds never CREATE keychain items (unsigned dev binaries get a
//! new code identity per rebuild, so "Always Allow" never sticks); the
//! master key lives in `secrets.key` instead, and any keychain-held key or
//! legacy items are rescued to disk once, then removed from the keychain.
//! `TABULAR_DISABLE_KEYRING=1` skips the keychain entirely (tests/CI);
//! `TABULAR_FORCE_KEYRING=1` forces keychain use in debug builds.

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

/// Stable secret-store name for an HTTP client auth field
/// (`field` is one of `bearer_token`, `basic_pass`, `api_key_value`).
pub fn http_secret_name(connection_id: i64, field: &str) -> String {
    format!("http:{}:{}", connection_id, field)
}

/// Remove all HTTP auth secrets for a connection (call when deleting a connection).
pub fn delete_http_secrets(connection_id: i64) {
    for field in ["bearer_token", "basic_pass", "api_key_value"] {
        delete_secret(&http_secret_name(connection_id, field));
    }
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
    // Skip the write when unchanged — callers re-store on every save.
    if backend_file::get(name).as_deref() == Some(value) {
        return true;
    }
    backend_file::set(name, value)
}

pub fn get_secret(name: &str) -> Option<String> {
    if let Some(v) = backend_file::get(name) {
        return Some(v);
    }
    // Legacy per-secret keychain items (old layout). Only migrate in On
    // (signed release) builds — Rescue/debug builds change code identity
    // every rebuild, so attempting keychain access here would prompt on
    // every run even after migration.
    if keyring_mode() == KeyringMode::On
        && let Some(v) = backend_keyring::get(name)
    {
        if backend_file::set(name, &v) {
            backend_keyring::delete(name);
        }
        return Some(v);
    }
    None
}

pub fn delete_secret(name: &str) {
    backend_file::delete(name);
    if keyring_allowed() {
        // Drop any legacy per-secret keychain item too.
        backend_keyring::delete(name);
    }
}

#[derive(PartialEq, Clone, Copy)]
enum KeyringMode {
    /// Keychain holds the master key (signed/release builds).
    On,
    /// Debug builds: never CREATE keychain items — unsigned dev binaries get
    /// a new code identity every rebuild, so macOS "Always Allow" grants
    /// never stick and every run prompts again. Existing items are still
    /// read once to rescue them into the file-backed store, then deleted.
    Rescue,
    /// TABULAR_DISABLE_KEYRING=1: never touch the keychain (tests/CI).
    Off,
}

fn keyring_mode() -> KeyringMode {
    if matches!(
        std::env::var("TABULAR_DISABLE_KEYRING").ok().as_deref(),
        Some("1") | Some("true")
    ) {
        return KeyringMode::Off;
    }
    if matches!(
        std::env::var("TABULAR_FORCE_KEYRING").ok().as_deref(),
        Some("1") | Some("true")
    ) {
        return KeyringMode::On;
    }
    if cfg!(debug_assertions) {
        KeyringMode::Rescue
    } else {
        KeyringMode::On
    }
}

fn keyring_allowed() -> bool {
    keyring_mode() != KeyringMode::Off
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

/// Encrypted store: `secrets.enc` is a JSON map of
/// `name -> hex(nonce || ciphertext)` under a ChaCha20-Poly1305 master key.
/// The master key lives in the OS keychain (one item, cached per process);
/// without a keychain it falls back to `secrets.key` on disk (0600).
mod backend_file {
    use chacha20poly1305::aead::{Aead, Generate};
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};
    use log::warn;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    const NONCE_LEN: usize = 12;
    const MASTER_KEY_NAME: &str = "master-key";

    // Cached for the whole process so the keychain prompts at most once.
    static MASTER_KEY: std::sync::OnceLock<Option<[u8; 32]>> = std::sync::OnceLock::new();

    fn key_path() -> PathBuf {
        // Key must live in the local dir (~/.tabular), NOT the custom data dir
        // which may be Google Drive / Dropbox / NFS. Cloud sync of the master
        // key causes conflicts and silent key rotation, breaking secrets.enc.
        crate::config::get_local_data_dir().join("secrets.key")
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

    fn master_key() -> Option<Key> {
        MASTER_KEY.get_or_init(resolve_master_key).map(Key::from)
    }

    fn read_key_file() -> Option<[u8; 32]> {
        let content = std::fs::read_to_string(key_path()).ok()?;
        let bytes = hex::decode(content.trim()).ok()?;
        if bytes.len() != 32 {
            warn!("secrets.key is malformed; ignoring it");
            return None;
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&bytes);
        Some(key_bytes)
    }

    /// Resolution order: on-disk `secrets.key` (always wins — works
    /// identically for debug and release builds sharing one data dir) →
    /// keychain item (Rescue mode persists it to disk and deletes the
    /// keychain copy) → freshly generated key.
    fn resolve_master_key() -> Option<[u8; 32]> {
        if let Some(key_bytes) = read_key_file() {
            return Some(key_bytes);
        }

        let mode = super::keyring_mode();
        if mode != super::KeyringMode::Off
            && let Some(hex_key) = super::backend_keyring::get(MASTER_KEY_NAME)
            && let Ok(bytes) = hex::decode(hex_key.trim())
            && bytes.len() == 32
        {
            let mut key_bytes = [0u8; 32];
            key_bytes.copy_from_slice(&bytes);
            if mode == super::KeyringMode::Rescue && write_key_file(&key_bytes) {
                // Dev builds: key now lives on disk; drop the keychain copy
                // so rebuilds never trigger another permission prompt.
                super::backend_keyring::delete(MASTER_KEY_NAME);
            }
            return Some(key_bytes);
        }

        let key_bytes = <[u8; 32]>::generate();
        if mode == super::KeyringMode::On
            && super::backend_keyring::set(MASTER_KEY_NAME, &hex::encode(key_bytes))
        {
            return Some(key_bytes);
        }
        if write_key_file(&key_bytes) {
            Some(key_bytes)
        } else {
            None
        }
    }

    fn write_key_file(key_bytes: &[u8; 32]) -> bool {
        let path = key_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, hex::encode(key_bytes)) {
            Ok(_) => {
                restrict_permissions(&path);
                true
            }
            Err(e) => {
                warn!("cannot create secrets.key: {}", e);
                false
            }
        }
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
        let key = master_key()?;
        let cipher = ChaCha20Poly1305::new(&key);
        let (nonce, ciphertext) = raw.split_at(NONCE_LEN);
        let mut nonce_bytes = [0u8; NONCE_LEN];
        nonce_bytes.copy_from_slice(nonce);
        let plain = cipher.decrypt(&Nonce::from(nonce_bytes), ciphertext).ok()?;
        String::from_utf8(plain).ok()
    }

    pub fn set(name: &str, value: &str) -> bool {
        let Some(key) = master_key() else {
            return false;
        };
        let cipher = ChaCha20Poly1305::new(&key);
        let nonce = Nonce::generate();
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

/// Guards the on-disk `secrets.enc` format across chacha20poly1305 upgrades:
/// blobs written by older crate versions must stay decryptable byte-for-byte.
#[cfg(test)]
mod crypto_compat_tests {
    use chacha20poly1305::aead::Aead;
    use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce};

    const TEST_KEY: [u8; 32] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f,
    ];
    const TEST_NONCE: [u8; 12] = [
        0x24, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x2b, 0x2c, 0x2d, 0x2e, 0x2f,
    ];
    const PLAINTEXT: &str = "tabular-secret-compat-v1";
    // Produced by chacha20poly1305 0.10.1 with the key/nonce above.
    const V0_10_CIPHERTEXT_HEX: &str =
        "8bb848cf25113cc188a120e59fc44390acce1f5f95a636e792c966a877c17b4d3ccc53f4922a2f89";

    #[test]
    fn decrypts_ciphertext_written_by_v0_10() {
        let cipher = ChaCha20Poly1305::new(&Key::from(TEST_KEY));
        let ciphertext = hex::decode(V0_10_CIPHERTEXT_HEX).unwrap();
        let plain = cipher
            .decrypt(&Nonce::from(TEST_NONCE), ciphertext.as_slice())
            .expect("ciphertext from previous crate version must decrypt");
        assert_eq!(plain, PLAINTEXT.as_bytes());
    }

    #[test]
    fn encryption_output_is_stable() {
        let cipher = ChaCha20Poly1305::new(&Key::from(TEST_KEY));
        let ciphertext = cipher
            .encrypt(&Nonce::from(TEST_NONCE), PLAINTEXT.as_bytes())
            .unwrap();
        assert_eq!(hex::encode(ciphertext), V0_10_CIPHERTEXT_HEX);
    }
}
