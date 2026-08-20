//! Zero-knowledge vault crypto for Tabular Cloud Sync.
//!
//! tabular-server never holds a key that can decrypt synced connection
//! credentials or HTTP client secrets. The only secret that can unlock them
//! is the user's **Sync Passphrase**, which never leaves this process:
//!
//! ```text
//! SyncPassphrase --Argon2id(salt)--> KEK
//!   KEK  --AES-256-GCM.wrap-->  AccountKey (random, encrypts the user's own data)
//!   KEK  --AES-256-GCM.wrap-->  X25519 private key (used to receive Team vault keys)
//! ```
//!
//! The server stores only: `salt`, KDF params, `wrapped_account_key`,
//! `x25519_public_key` (public, safe to share), `wrapped_x25519_private_key`,
//! and the same wrapped-by-recovery-code pair for the "forgot my passphrase"
//! fallback. All of it is opaque without the passphrase or recovery code.
//!
//! A Team gets one shared symmetric `TeamKey`; it is never stored in the
//! clear — it is "sealed" (anonymous public-key encryption, libsodium-style)
//! individually to each member's `x25519_public_key` so only that member's
//! private key can open it. See [`wrap_team_key`] / [`unwrap_team_key`].

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use crypto_box::aead::OsRng as SealOsRng;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Argon2id parameters used to derive the Key-Encryption-Key from the Sync
/// Passphrase. OWASP-recommended minimums for a KDF that must run on every
/// unlock on ordinary desktop hardware.
const ARGON2_M_COST_KIB: u32 = 19 * 1024; // 19 MiB
const ARGON2_T_COST: u32 = 2;
const ARGON2_P_COST: u32 = 1;
pub const KDF_ALGO: &str = "argon2id";
pub const KDF_PARAMS_JSON: &str = r#"{"m_cost":19456,"t_cost":2,"p_cost":1}"#;

/// A raw 256-bit symmetric key, scrubbed from memory on drop. Used for both
/// the per-user AccountKey and per-Team TeamKey.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SymKey(pub [u8; KEY_LEN]);

impl SymKey {
    pub fn generate() -> Self {
        let mut bytes = [0u8; KEY_LEN];
        rand::rng().fill(&mut bytes);
        SymKey(bytes)
    }

    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }

    pub fn from_base64(s: &str) -> Result<Self, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s)
            .map_err(|e| e.to_string())?;
        let arr: [u8; KEY_LEN] = bytes
            .try_into()
            .map_err(|_| "invalid key length".to_string())?;
        Ok(SymKey(arr))
    }
}

/// The fully-unlocked vault, held in memory only for the lifetime of the
/// session. Never persisted in this form.
#[derive(Clone, ZeroizeOnDrop)]
pub struct UnlockedVault {
    /// Encrypts/decrypts the user's own (non-Team-shared) connections & HTTP secrets.
    pub account_key: SymKey,
    /// This user's X25519 keypair — `secret` unseals TeamKeys granted to them,
    /// `public_bytes` is uploaded to the server so others can grant them one.
    #[zeroize(skip)]
    pub x25519_secret: crypto_box::SecretKey,
    pub x25519_public_bytes: [u8; 32],
}

/// Everything that gets uploaded to `PUT /api/v1/vault/keys`. Every field is
/// either public (the X25519 public key) or opaque ciphertext — the server
/// stores this as-is and can never derive `UnlockedVault` from it alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultKeyBundle {
    pub kdf_algo: String,
    pub kdf_params_json: String,
    pub salt: String,                        // base64
    pub wrapped_account_key: String,          // base64(nonce || ciphertext)
    pub x25519_public_key: String,            // base64
    pub wrapped_x25519_private_key: String,   // base64(nonce || ciphertext)
    pub recovery_salt: String,                // base64
    pub wrapped_account_key_recovery: String, // base64(nonce || ciphertext)
}

/// Recovery code shown to the user exactly once at vault-creation time.
/// 32 random bytes, rendered as groups of hex for easy transcription.
pub fn generate_recovery_code() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    let hex = hex::encode(bytes);
    hex.as_bytes()
        .chunks(4)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Derive the 256-bit Key-Encryption-Key from a passphrase (or recovery code)
/// and salt via Argon2id. Both use the same KDF; only the salt differs.
fn derive_kek(secret: &str, salt: &[u8]) -> Result<SymKey, String> {
    let params = Params::new(ARGON2_M_COST_KIB, ARGON2_T_COST, ARGON2_P_COST, Some(KEY_LEN))
        .map_err(|e| format!("argon2 params: {e}"))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; KEY_LEN];
    argon2
        .hash_password_into(secret.as_bytes(), salt, &mut out)
        .map_err(|e| format!("argon2 derive: {e}"))?;
    Ok(SymKey(out))
}

/// AES-256-GCM encrypt. Returns base64(nonce(12) || ciphertext).
fn aes_encrypt(key: &SymKey, plaintext: &[u8]) -> Result<String, String> {
    let cipher_key: Key<Aes256Gcm> = key.0.into();
    let cipher = Aes256Gcm::new(&cipher_key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill(&mut nonce_bytes);
    let nonce: Nonce<_> = nonce_bytes.into();

    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| e.to_string())?;

    let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::STANDARD.encode(combined))
}

/// AES-256-GCM decrypt of the format produced by [`aes_encrypt`].
fn aes_decrypt(key: &SymKey, encoded: &str) -> Result<Vec<u8>, String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    if data.len() < NONCE_LEN {
        return Err("ciphertext too short".to_string());
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);

    let cipher_key: Key<Aes256Gcm> = key.0.into();
    let cipher = Aes256Gcm::new(&cipher_key);
    let nonce: Nonce<_> = nonce_bytes
        .try_into()
        .map_err(|_| "invalid nonce length".to_string())?;

    cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| "decryption failed (wrong key or corrupted data)".to_string())
}

/// Encrypt an arbitrary JSON-serializable value with a [`SymKey`] (AccountKey
/// or TeamKey). Used for connection configs & HTTP request secrets.
pub fn encrypt_json<T: Serialize>(key: &SymKey, value: &T) -> Result<String, String> {
    let plaintext = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    aes_encrypt(key, &plaintext)
}

/// Decrypt a payload produced by [`encrypt_json`].
pub fn decrypt_json<T: for<'de> Deserialize<'de>>(key: &SymKey, encoded: &str) -> Result<T, String> {
    let bytes = aes_decrypt(key, encoded)?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// Encrypt a plain string (used for the three legacy HTTP-request JSON blobs,
/// which are already-serialized JSON strings rather than typed values).
pub fn encrypt_str(key: &SymKey, plaintext: &str) -> Result<String, String> {
    aes_encrypt(key, plaintext.as_bytes())
}

/// Decrypt a payload produced by [`encrypt_str`].
pub fn decrypt_str(key: &SymKey, encoded: &str) -> Result<String, String> {
    let bytes = aes_decrypt(key, encoded)?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Create a brand-new vault: generates the AccountKey and X25519 keypair,
/// wraps both under the passphrase-derived KEK *and* under a freshly
/// generated recovery code, and returns everything needed to unlock plus
/// the bundle to upload to the server.
///
/// The recovery code is returned once — the caller MUST show it to the user
/// and get explicit confirmation it was saved; it cannot be recovered later.
pub fn create_vault(passphrase: &str) -> Result<(UnlockedVault, VaultKeyBundle, String), String> {
    if passphrase.chars().count() < 8 {
        return Err("Sync Passphrase must be at least 8 characters".to_string());
    }

    let account_key = SymKey::generate();
    let x25519_secret = crypto_box::SecretKey::generate(&mut SealOsRng);
    let x25519_public = x25519_secret.public_key();
    let x25519_public_bytes = *x25519_public.as_bytes();

    let mut salt = [0u8; 16];
    rand::rng().fill(&mut salt);
    let kek = derive_kek(passphrase, &salt)?;

    let recovery_code = generate_recovery_code();
    let mut recovery_salt = [0u8; 16];
    rand::rng().fill(&mut recovery_salt);
    let recovery_kek = derive_kek(&normalize_recovery_code(&recovery_code), &recovery_salt)?;

    let wrapped_account_key = aes_encrypt(&kek, &account_key.0)?;
    let wrapped_account_key_recovery = aes_encrypt(&recovery_kek, &account_key.0)?;
    let wrapped_x25519_private_key = aes_encrypt(&kek, x25519_secret.to_bytes().as_slice())?;

    let bundle = VaultKeyBundle {
        kdf_algo: KDF_ALGO.to_string(),
        kdf_params_json: KDF_PARAMS_JSON.to_string(),
        salt: base64::engine::general_purpose::STANDARD.encode(salt),
        wrapped_account_key,
        x25519_public_key: base64::engine::general_purpose::STANDARD.encode(x25519_public_bytes),
        wrapped_x25519_private_key,
        recovery_salt: base64::engine::general_purpose::STANDARD.encode(recovery_salt),
        wrapped_account_key_recovery,
    };

    let unlocked = UnlockedVault {
        account_key,
        x25519_secret,
        x25519_public_bytes,
    };

    Ok((unlocked, bundle, recovery_code))
}

/// Unlock an existing vault bundle (fetched from the server) using the
/// user's Sync Passphrase.
pub fn unlock_with_passphrase(bundle: &VaultKeyBundle, passphrase: &str) -> Result<UnlockedVault, String> {
    let salt = base64::engine::general_purpose::STANDARD
        .decode(&bundle.salt)
        .map_err(|e| e.to_string())?;
    let kek = derive_kek(passphrase, &salt)?;
    unlock_with_kek(bundle, &kek)
}

/// Unlock an existing vault bundle using the one-time recovery code shown at
/// vault-creation time (fallback when the passphrase is forgotten).
pub fn unlock_with_recovery_code(bundle: &VaultKeyBundle, recovery_code: &str) -> Result<UnlockedVault, String> {
    let salt = base64::engine::general_purpose::STANDARD
        .decode(&bundle.recovery_salt)
        .map_err(|e| e.to_string())?;
    let kek = derive_kek(&normalize_recovery_code(recovery_code), &salt)?;
    let account_key_bytes = aes_decrypt(&kek, &bundle.wrapped_account_key_recovery)?;
    let account_key = SymKey(
        account_key_bytes
            .try_into()
            .map_err(|_| "invalid account key length".to_string())?,
    );

    // The recovery path only re-derives the AccountKey (that's the whole point
    // of "forgot my passphrase" — regain access to your own data). The
    // X25519 keypair stays wrapped under the passphrase KEK only, so a
    // recovery-code unlock cannot itself unseal Team vault keys; the caller
    // should prompt to also set a new passphrase (re-wrapping everything via
    // [`rewrap_with_new_passphrase`]) once this succeeds.
    let x25519_secret = crypto_box::SecretKey::generate(&mut SealOsRng);
    let x25519_public_bytes = *x25519_secret.public_key().as_bytes();

    Ok(UnlockedVault {
        account_key,
        x25519_secret,
        x25519_public_bytes,
    })
}

fn unlock_with_kek(bundle: &VaultKeyBundle, kek: &SymKey) -> Result<UnlockedVault, String> {
    let account_key_bytes = aes_decrypt(kek, &bundle.wrapped_account_key)?;
    let account_key = SymKey(
        account_key_bytes
            .try_into()
            .map_err(|_| "invalid account key length".to_string())?,
    );

    let x25519_secret_bytes = aes_decrypt(kek, &bundle.wrapped_x25519_private_key)?;
    let x25519_arr: [u8; 32] = x25519_secret_bytes
        .try_into()
        .map_err(|_| "invalid x25519 secret key length".to_string())?;
    let x25519_secret = crypto_box::SecretKey::from_bytes(x25519_arr);
    let x25519_public_bytes = *x25519_secret.public_key().as_bytes();

    let expected_pub = base64::engine::general_purpose::STANDARD
        .decode(&bundle.x25519_public_key)
        .map_err(|e| e.to_string())?;
    if expected_pub != x25519_public_bytes {
        return Err("vault integrity check failed: public key mismatch".to_string());
    }

    Ok(UnlockedVault {
        account_key,
        x25519_secret,
        x25519_public_bytes,
    })
}

/// Re-wrap the vault under a new passphrase (used after a recovery-code
/// unlock, or an explicit "change passphrase" action). Keeps the same
/// AccountKey and X25519 keypair — only the KEK wrapping changes — so
/// nothing that was already encrypted with them needs re-encrypting.
pub fn rewrap_with_new_passphrase(
    vault: &UnlockedVault,
    new_passphrase: &str,
) -> Result<VaultKeyBundle, String> {
    if new_passphrase.chars().count() < 8 {
        return Err("Sync Passphrase must be at least 8 characters".to_string());
    }

    let mut salt = [0u8; 16];
    rand::rng().fill(&mut salt);
    let kek = derive_kek(new_passphrase, &salt)?;

    let recovery_code = generate_recovery_code();
    let mut recovery_salt = [0u8; 16];
    rand::rng().fill(&mut recovery_salt);
    let recovery_kek = derive_kek(&normalize_recovery_code(&recovery_code), &recovery_salt)?;

    let wrapped_account_key = aes_encrypt(&kek, &vault.account_key.0)?;
    let wrapped_account_key_recovery = aes_encrypt(&recovery_kek, &vault.account_key.0)?;
    let wrapped_x25519_private_key = aes_encrypt(&kek, vault.x25519_secret.to_bytes().as_slice())?;

    Ok(VaultKeyBundle {
        kdf_algo: KDF_ALGO.to_string(),
        kdf_params_json: KDF_PARAMS_JSON.to_string(),
        salt: base64::engine::general_purpose::STANDARD.encode(salt),
        wrapped_account_key,
        x25519_public_key: base64::engine::general_purpose::STANDARD.encode(vault.x25519_public_bytes),
        wrapped_x25519_private_key,
        recovery_salt: base64::engine::general_purpose::STANDARD.encode(recovery_salt),
        wrapped_account_key_recovery,
    })
}

// ─── Team vault key sharing (sealed boxes) ────────────────────────────────────

/// Seal a Team's symmetric key so only the holder of `recipient_public_key`
/// can open it (anonymous public-key encryption — the server relays this
/// blob but can never decrypt it). Returns base64.
pub fn wrap_team_key(recipient_public_key_b64: &str, team_key: &SymKey) -> Result<String, String> {
    let pub_bytes = base64::engine::general_purpose::STANDARD
        .decode(recipient_public_key_b64)
        .map_err(|e| e.to_string())?;
    let pub_arr: [u8; 32] = pub_bytes
        .try_into()
        .map_err(|_| "invalid public key length".to_string())?;
    let public_key = crypto_box::PublicKey::from_bytes(pub_arr);

    let sealed = public_key
        .seal(&mut SealOsRng, &team_key.0)
        .map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(sealed))
}

/// Open a Team key envelope sealed by [`wrap_team_key`] using our own
/// X25519 private key.
pub fn unwrap_team_key(vault: &UnlockedVault, sealed_b64: &str) -> Result<SymKey, String> {
    let sealed = base64::engine::general_purpose::STANDARD
        .decode(sealed_b64)
        .map_err(|e| e.to_string())?;
    let opened = vault
        .x25519_secret
        .unseal(&sealed)
        .map_err(|e| e.to_string())?;
    let arr: [u8; KEY_LEN] = opened
        .try_into()
        .map_err(|_| "invalid team key length".to_string())?;
    Ok(SymKey(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_unlock_roundtrip() {
        let (vault, bundle, _recovery_code) = create_vault("correct horse battery staple").unwrap();
        let reopened = unlock_with_passphrase(&bundle, "correct horse battery staple").unwrap();
        assert_eq!(vault.account_key.0, reopened.account_key.0);
        assert_eq!(vault.x25519_public_bytes, reopened.x25519_public_bytes);
    }

    #[test]
    fn wrong_passphrase_fails() {
        let (_vault, bundle, _recovery_code) = create_vault("correct horse battery staple").unwrap();
        assert!(unlock_with_passphrase(&bundle, "wrong passphrase entirely").is_err());
    }

    #[test]
    fn recovery_code_unlocks_account_key() {
        let (vault, bundle, recovery_code) = create_vault("correct horse battery staple").unwrap();
        let recovered = unlock_with_recovery_code(&bundle, &recovery_code).unwrap();
        assert_eq!(vault.account_key.0, recovered.account_key.0);
    }

    #[test]
    fn wrong_recovery_code_fails() {
        let (_vault, bundle, _recovery_code) = create_vault("correct horse battery staple").unwrap();
        let bogus = generate_recovery_code();
        assert!(unlock_with_recovery_code(&bundle, &bogus).is_err());
    }

    #[test]
    fn encrypt_decrypt_json_roundtrip() {
        let key = SymKey::generate();
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Payload {
            host: String,
            password: String,
        }
        let original = Payload {
            host: "db.internal".to_string(),
            password: "hunter2".to_string(),
        };
        let ciphertext = encrypt_json(&key, &original).unwrap();
        assert!(!ciphertext.contains("hunter2"), "plaintext must not leak into ciphertext");
        let decrypted: Payload = decrypt_json(&key, &ciphertext).unwrap();
        assert_eq!(original, decrypted);
    }

    #[test]
    fn team_key_seal_unseal_roundtrip() {
        let (vault, _bundle, _rc) = create_vault("team member passphrase!").unwrap();
        let recipient_pub_b64 = base64::engine::general_purpose::STANDARD.encode(vault.x25519_public_bytes);

        let team_key = SymKey::generate();
        let sealed = wrap_team_key(&recipient_pub_b64, &team_key).unwrap();
        let opened = unwrap_team_key(&vault, &sealed).unwrap();
        assert_eq!(team_key.0, opened.0);
    }

    #[test]
    fn team_key_wrong_recipient_fails() {
        let (_vault_a, _bundle_a, _rc_a) = create_vault("member A passphrase!!").unwrap();
        let (vault_b, _bundle_b, _rc_b) = create_vault("member B passphrase!!").unwrap();

        let team_key = SymKey::generate();
        // Seal to a throwaway key that is neither A nor B.
        let (other, _bundle_other, _rc_other) = create_vault("unrelated passphrase!!").unwrap();
        let other_pub_b64 = base64::engine::general_purpose::STANDARD.encode(other.x25519_public_bytes);
        let sealed = wrap_team_key(&other_pub_b64, &team_key).unwrap();

        // B (not the intended recipient) must not be able to open it.
        assert!(unwrap_team_key(&vault_b, &sealed).is_err());
    }
}
