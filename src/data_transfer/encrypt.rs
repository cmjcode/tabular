//! Ekspor terenkripsi: AES-256-GCM dengan kunci dari passphrase (Argon2id,
//! parameter yang sama dengan vault sync di `sync::vault_crypto`).
//!
//! Format file (biner):
//!
//! ```text
//! "TABENC01" (8) | salt (16) | nonce (12) | ciphertext + tag GCM (16)
//! ```
//!
//! Header (magic + salt) ikut diautentikasi sebagai AAD, jadi file yang
//! header-nya diubah gagal didekripsi.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngExt;

use crate::sync::vault_crypto::derive_kek;

pub const MAGIC: &[u8; 8] = b"TABENC01";
/// Akhiran yang ditambahkan ke nama file terenkripsi (`users.csv.enc`).
pub const ENCRYPTED_EXTENSION: &str = "enc";

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = MAGIC.len() + SALT_LEN;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EncryptError {
    #[error("Passphrase must not be empty")]
    EmptyPassphrase,
    #[error("File is not a Tabular encrypted export")]
    NotEncrypted,
    #[error("Encrypted file is truncated")]
    Truncated,
    #[error("Wrong passphrase or corrupted file")]
    WrongPassphrase,
    #[error("Encryption failed: {0}")]
    Crypto(String),
}

/// True bila `bytes` diawali magic file terenkripsi Tabular.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn cipher_for(passphrase: &str, salt: &[u8]) -> Result<Aes256Gcm, EncryptError> {
    let key = derive_kek(passphrase, salt).map_err(EncryptError::Crypto)?;
    let cipher_key: Key<Aes256Gcm> = key.0.into();
    Ok(Aes256Gcm::new(&cipher_key))
}

/// Enkripsi `plaintext` dengan `passphrase`. Salt dan nonce acak per file.
pub fn encrypt_bytes(passphrase: &str, plaintext: &[u8]) -> Result<Vec<u8>, EncryptError> {
    if passphrase.is_empty() {
        return Err(EncryptError::EmptyPassphrase);
    }
    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rng().fill(&mut salt);
    rand::rng().fill(&mut nonce_bytes);

    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + plaintext.len() + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);

    let cipher = cipher_for(passphrase, &salt)?;
    let nonce: Nonce<_> = nonce_bytes.into();
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &out[..HEADER_LEN],
            },
        )
        .map_err(|e| EncryptError::Crypto(e.to_string()))?;
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Dekripsi file hasil [`encrypt_bytes`].
pub fn decrypt_bytes(passphrase: &str, data: &[u8]) -> Result<Vec<u8>, EncryptError> {
    if !is_encrypted(data) {
        return Err(EncryptError::NotEncrypted);
    }
    if passphrase.is_empty() {
        return Err(EncryptError::EmptyPassphrase);
    }
    if data.len() < HEADER_LEN + NONCE_LEN + 16 {
        return Err(EncryptError::Truncated);
    }
    let (header, rest) = data.split_at(HEADER_LEN);
    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);
    let cipher = cipher_for(passphrase, &header[MAGIC.len()..])?;
    let nonce: Nonce<_> = nonce_bytes
        .try_into()
        .map_err(|_| EncryptError::Truncated)?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: header,
            },
        )
        .map_err(|_| EncryptError::WrongPassphrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_random_salt() {
        let a = encrypt_bytes("s3cret", b"id,name\n1,a\n").unwrap();
        let b = encrypt_bytes("s3cret", b"id,name\n1,a\n").unwrap();
        assert!(is_encrypted(&a));
        assert_ne!(a, b, "salt/nonce must differ per file");
        assert_eq!(decrypt_bytes("s3cret", &a).unwrap(), b"id,name\n1,a\n");
    }

    #[test]
    fn wrong_passphrase_and_tampering_fail() {
        let mut data = encrypt_bytes("right", b"payload").unwrap();
        assert_eq!(
            decrypt_bytes("wrong", &data),
            Err(EncryptError::WrongPassphrase)
        );
        // Ubah satu byte salt: header ikut diautentikasi.
        data[MAGIC.len()] ^= 0x01;
        assert_eq!(
            decrypt_bytes("right", &data),
            Err(EncryptError::WrongPassphrase)
        );
    }

    #[test]
    fn rejects_plain_and_truncated_input() {
        assert_eq!(
            decrypt_bytes("x", b"id,name"),
            Err(EncryptError::NotEncrypted)
        );
        assert_eq!(decrypt_bytes("x", MAGIC), Err(EncryptError::Truncated));
        assert_eq!(encrypt_bytes("", b"a"), Err(EncryptError::EmptyPassphrase));
    }
}
