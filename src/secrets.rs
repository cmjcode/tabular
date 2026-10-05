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

    fn master_key() -> Option<Key> {
        MASTER_KEY.get_or_init(resolve_master_key).map(Key::from)
    }

    fn read_key_file() -> Option<[u8; 32]> {
        let path = key_path();
        let key = parse_key_file(&path);
        if key.is_none() && path.exists() {
            warn!("secrets.key is malformed; ignoring it");
        }
        key
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
            if mode == super::KeyringMode::Rescue
                && let Some(on_disk) = persist_key_file(&key_path(), &key_bytes)
            {
                if on_disk == key_bytes {
                    // Dev builds: key now lives on disk; drop the keychain copy
                    // so rebuilds never trigger another permission prompt.
                    super::backend_keyring::delete(MASTER_KEY_NAME);
                }
                // On-disk key always wins (see resolution order above).
                return Some(on_disk);
            }
            return Some(key_bytes);
        }

        let key_bytes = <[u8; 32]>::generate();
        if mode == super::KeyringMode::On
            && super::backend_keyring::set(MASTER_KEY_NAME, &hex::encode(key_bytes))
        {
            return Some(key_bytes);
        }
        persist_key_file(&key_path(), &key_bytes)
    }

    fn parse_key_file(path: &Path) -> Option<[u8; 32]> {
        let content = std::fs::read_to_string(path).ok()?;
        let bytes = hex::decode(content.trim()).ok()?;
        <[u8; 32]>::try_from(bytes.as_slice()).ok()
    }

    /// Simpan master key ke `path` dan kembalikan key yang akhirnya berlaku di
    /// disk. File dibuat dengan `create_new` + mode 0600 sejak awal (tidak ada
    /// jendela waktu terbaca user lain), dan bila proses lain (GUI vs
    /// `tabular mcp`) lebih dulu membuatnya, key milik proses itu yang dipakai
    /// supaya dua proses tidak mengenkripsi `secrets.enc` dengan key berbeda.
    fn persist_key_file(path: &Path, key_bytes: &[u8; 32]) -> Option<[u8; 32]> {
        use std::io::Write;

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(mut file) => {
                let written = file
                    .write_all(hex::encode(key_bytes).as_bytes())
                    .and_then(|_| file.sync_all());
                match written {
                    Ok(()) => Some(*key_bytes),
                    Err(e) => {
                        warn!("cannot write secrets.key: {}", e);
                        // Jangan tinggalkan key setengah jadi di disk.
                        let _ = std::fs::remove_file(path);
                        None
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Proses lain mungkin sedang menulisnya: tunggu sebentar.
                for _ in 0..10 {
                    if let Some(existing) = parse_key_file(path) {
                        return Some(existing);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                // Benar-benar rusak: sisihkan, lalu tulis key baru secara atomik.
                warn!("secrets.key is malformed; replacing it");
                let _ = crate::directory::quarantine_corrupt_file(path);
                match crate::directory::write_file_atomically_with_mode(
                    path,
                    hex::encode(key_bytes).as_bytes(),
                    Some(0o600),
                ) {
                    Ok(()) => Some(*key_bytes),
                    Err(e) => {
                        warn!("cannot create secrets.key: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                warn!("cannot create secrets.key: {}", e);
                None
            }
        }
    }

    #[derive(Debug)]
    pub(super) enum StoreError {
        /// Isi `secrets.enc` tidak bisa dibaca sebagai peta JSON.
        Corrupt(String),
        Io(std::io::Error),
    }

    impl std::fmt::Display for StoreError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                StoreError::Corrupt(msg) => write!(f, "secret store is corrupt: {}", msg),
                StoreError::Io(e) => write!(f, "secret store I/O error: {}", e),
            }
        }
    }

    /// Penanda versi file di disk (mtime + ukuran) untuk mendeteksi perubahan
    /// oleh proses lain tanpa membaca ulang isinya.
    type FileStamp = Option<(std::time::SystemTime, u64)>;

    struct CachedEntries {
        path: PathBuf,
        stamp: FileStamp,
        entries: HashMap<String, String>,
    }

    /// Dipegang selama baca-ubah-tulis supaya dua thread tidak saling menimpa.
    static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    static ENTRIES_CACHE: std::sync::Mutex<Option<CachedEntries>> = std::sync::Mutex::new(None);

    fn lock_cache() -> std::sync::MutexGuard<'static, Option<CachedEntries>> {
        ENTRIES_CACHE.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn file_stamp(path: &Path) -> FileStamp {
        let meta = std::fs::metadata(path).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// Baca peta dari disk. File yang belum ada = peta kosong; file yang ada
    /// tetapi tidak bisa di-parse = `Corrupt` (BUKAN peta kosong, supaya
    /// penulisan berikutnya tidak menghapus semua rahasia).
    fn read_store(path: &Path) -> Result<HashMap<String, String>, StoreError> {
        match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| StoreError::Corrupt(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(StoreError::Io(e)),
        }
    }

    fn write_store(path: &Path, entries: &HashMap<String, String>) -> Result<(), StoreError> {
        let json = serde_json::to_vec_pretty(entries)
            .map_err(|e| StoreError::Io(std::io::Error::other(e)))?;
        // File sementara + fsync + rename, dibuat 0600 sejak awal.
        crate::directory::write_file_atomically_with_mode(path, &json, Some(0o600))
            .map_err(StoreError::Io)
    }

    /// Kunci antar-proses (GUI vs `tabular mcp`) lewat file kunci di samping
    /// store. Usaha terbaik: filesystem yang tidak mendukung lock tetap jalan
    /// hanya dengan kunci dalam-proses.
    fn lock_store_file(path: &Path) -> Option<std::fs::File> {
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let lock_path = path.with_file_name(format!(".{}.lock", file_name));
        if let Some(parent) = lock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;
        match file.lock() {
            Ok(()) => Some(file),
            Err(e) => {
                log::debug!("secret store file lock unavailable: {}", e);
                None
            }
        }
    }

    /// Baca-ubah-tulis `path` di bawah satu kunci. Isi SELALU dibaca ulang dari
    /// disk di dalam kunci (tidak dari cache) sehingga key yang ditulis proses
    /// lain tidak hilang. `mutate` mengembalikan `true` bila ada perubahan.
    /// File korup dipindah ke `secrets.enc.corrupt-<timestamp>` dan operasi
    /// gagal; tidak ada penulisan diam-diam di atasnya.
    pub(super) fn update_store_at(
        path: &Path,
        mutate: impl FnOnce(&mut HashMap<String, String>) -> bool,
    ) -> Result<(HashMap<String, String>, FileStamp), StoreError> {
        let _guard = STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _file_lock = lock_store_file(path);

        let mut entries = match read_store(path) {
            Ok(entries) => entries,
            Err(StoreError::Corrupt(first)) => {
                // Versi lama menulis tanpa rename atomik: beri kesempatan
                // penulis itu selesai sebelum memutuskan file benar-benar rusak.
                std::thread::sleep(std::time::Duration::from_millis(50));
                match read_store(path) {
                    Ok(entries) => entries,
                    Err(StoreError::Corrupt(_)) => {
                        warn!(
                            "secrets.enc is corrupt ({}); preserving it and refusing to overwrite",
                            first
                        );
                        let _ = crate::directory::quarantine_corrupt_file(path);
                        return Err(StoreError::Corrupt(first));
                    }
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };
        if mutate(&mut entries) {
            write_store(path, &entries)?;
        }
        Ok((entries, file_stamp(path)))
    }

    fn update_store(mutate: impl FnOnce(&mut HashMap<String, String>) -> bool) -> bool {
        let path = store_path();
        match update_store_at(&path, mutate) {
            Ok((entries, stamp)) => {
                *lock_cache() = Some(CachedEntries {
                    path,
                    stamp,
                    entries,
                });
                true
            }
            Err(e) => {
                warn!("{}", e);
                // Cache tidak lagi bisa dipercaya.
                *lock_cache() = None;
                false
            }
        }
    }

    /// Blob terenkripsi untuk `name`. Cache hanya dipakai selama file di disk
    /// belum berubah (mtime + ukuran), jadi perubahan dari proses lain terbaca.
    fn stored_blob(name: &str) -> Option<String> {
        let path = store_path();
        let stamp = file_stamp(&path);
        {
            let cache = lock_cache();
            if let Some(cached) = cache.as_ref()
                && cached.path == path
                && cached.stamp == stamp
            {
                return cached.entries.get(name).cloned();
            }
        }
        match read_store(&path) {
            Ok(entries) => {
                let blob = entries.get(name).cloned();
                *lock_cache() = Some(CachedEntries {
                    path,
                    stamp,
                    entries,
                });
                blob
            }
            Err(e) => {
                // Jalur baca tidak memindahkan file; itu tugas jalur tulis.
                warn!("{}", e);
                None
            }
        }
    }

    pub fn get(name: &str) -> Option<String> {
        let blob = stored_blob(name)?;
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
        let blob = hex::encode(blob);
        update_store(|entries| {
            entries.insert(name.to_string(), blob);
            true
        })
    }

    pub fn delete(name: &str) {
        // Tidak ada file = tidak ada yang dihapus; jangan membuat file kosong.
        if !store_path().exists() {
            return;
        }
        let _ = update_store(|entries| entries.remove(name).is_some());
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn tmp_dir(tag: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "tabular-secrets-{}-{}-{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            dir
        }

        fn put(path: &Path, name: &str, blob: &str) -> Result<HashMap<String, String>, StoreError> {
            update_store_at(path, |entries| {
                entries.insert(name.to_string(), blob.to_string());
                true
            })
            .map(|(entries, _)| entries)
        }

        #[test]
        fn missing_file_is_an_empty_store() {
            let dir = tmp_dir("missing");
            let path = dir.join("secrets.enc");
            assert!(read_store(&path).expect("missing is fine").is_empty());
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn corrupt_file_is_preserved_and_not_overwritten() {
            let dir = tmp_dir("corrupt");
            let path = dir.join("secrets.enc");
            std::fs::write(&path, "{\"conn:1:password\": \"abc").expect("write garbage");

            assert!(matches!(read_store(&path), Err(StoreError::Corrupt(_))));
            let result = put(&path, "conn:2:password", "ff");
            assert!(matches!(result, Err(StoreError::Corrupt(_))));

            // Tidak ada file baru yang ditulis di atasnya, dan isi lama utuh.
            assert!(!path.exists());
            let backups: Vec<PathBuf> = std::fs::read_dir(&dir)
                .expect("read dir")
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().starts_with("secrets.enc.corrupt-"))
                        .unwrap_or(false)
                })
                .collect();
            assert_eq!(backups.len(), 1);
            assert_eq!(
                std::fs::read_to_string(&backups[0]).expect("backup"),
                "{\"conn:1:password\": \"abc"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn atomic_write_roundtrip() {
            let dir = tmp_dir("roundtrip");
            let path = dir.join("secrets.enc");
            put(&path, "a", "01").expect("first");
            put(&path, "b", "02").expect("second");
            let entries = read_store(&path).expect("read");
            assert_eq!(entries.get("a").map(String::as_str), Some("01"));
            assert_eq!(entries.get("b").map(String::as_str), Some("02"));
            // Tidak ada file sementara yang tertinggal.
            let leftovers = std::fs::read_dir(&dir)
                .expect("read dir")
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                .count();
            assert_eq!(leftovers, 0);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }

        /// Dua penulis (GUI dan `tabular mcp`) masing-masing memegang salinan
        /// lama isi file. Karena tiap penulisan membaca ulang disk di dalam
        /// kunci, key milik penulis lain tidak ikut terhapus.
        #[test]
        fn sequential_writers_with_stale_caches_keep_all_keys() {
            let dir = tmp_dir("stale");
            let path = dir.join("secrets.enc");
            put(&path, "shared", "00").expect("seed");

            // Keduanya "memuat cache" pada titik ini.
            let stale_a = read_store(&path).expect("a loads");
            let stale_b = read_store(&path).expect("b loads");
            assert_eq!(stale_a.len(), 1);
            assert_eq!(stale_b.len(), 1);

            put(&path, "from_a", "0a").expect("a writes");
            put(&path, "from_b", "0b").expect("b writes");
            // Penghapusan oleh A tidak boleh menghidupkan lagi/menghapus key B.
            update_store_at(&path, |entries| entries.remove("shared").is_some())
                .expect("a deletes");

            let entries = read_store(&path).expect("final");
            assert_eq!(entries.get("from_a").map(String::as_str), Some("0a"));
            assert_eq!(entries.get("from_b").map(String::as_str), Some("0b"));
            assert!(!entries.contains_key("shared"));
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn concurrent_writers_do_not_lose_keys() {
            let dir = tmp_dir("threads");
            let path = dir.join("secrets.enc");
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let path = path.clone();
                    std::thread::spawn(move || {
                        for j in 0..5 {
                            put(&path, &format!("k{i}_{j}"), "00").expect("put");
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().expect("writer thread");
            }
            assert_eq!(read_store(&path).expect("final").len(), 40);
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn key_file_is_created_private_and_first_writer_wins() {
            let dir = tmp_dir("key");
            let path = dir.join("secrets.key");
            let first = [7u8; 32];
            let second = [9u8; 32];
            assert_eq!(persist_key_file(&path, &first), Some(first));
            // Proses kedua harus memakai key yang sudah ada, bukan menimpanya.
            assert_eq!(persist_key_file(&path, &second), Some(first));
            assert_eq!(parse_key_file(&path), Some(first));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn malformed_key_file_is_replaced_but_preserved() {
            let dir = tmp_dir("badkey");
            let path = dir.join("secrets.key");
            std::fs::write(&path, "not-hex").expect("write");
            let key = [3u8; 32];
            assert_eq!(persist_key_file(&path, &key), Some(key));
            assert_eq!(parse_key_file(&path), Some(key));
            let preserved = std::fs::read_dir(&dir)
                .expect("read dir")
                .flatten()
                .filter(|e| e.file_name().to_string_lossy().contains(".corrupt-"))
                .count();
            assert_eq!(preserved, 1);
            let _ = std::fs::remove_dir_all(&dir);
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
