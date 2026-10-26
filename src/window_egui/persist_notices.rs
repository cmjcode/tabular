//! Menampilkan kegagalan penyimpanan dari kode headless ke user.
//!
//! Modul headless (mis. `secrets`) tidak punya akses ke toast; mereka
//! mengantrekan kejadian, dan fungsi di sini mengubahnya menjadi toast satu
//! kali per sesi untuk tiap jenis/kunci.

use super::Tabular;
use crate::secrets::SecretNotice;

impl Tabular {
    /// Ambil kejadian penyimpanan (umum dan secret) yang tertunda lalu tampilkan
    /// sebagai toast. Murah
    /// dipanggil tiap frame (satu lock + cek kosong).
    pub(crate) fn surface_secret_notices(&mut self) {
        for notice in crate::persist_notice::take() {
            match notice.level {
                crate::persist_notice::Level::Warning => {
                    self.toasts.warning_once(&notice.key, notice.message)
                }
                crate::persist_notice::Level::Error => {
                    self.toasts.error_once(&notice.key, notice.message)
                }
            }
        }
        for notice in crate::secrets::take_notices() {
            match notice {
                SecretNotice::PlaintextFallback { name } => {
                    self.toasts.warning_once(
                        "secret-plaintext-fallback",
                        format!(
                            "The system keychain and the encrypted secret store are not available, so a credential ({}) was saved unencrypted in the local database. Check the keychain permissions, then save the connection again.",
                            self.describe_secret(&name)
                        ),
                    );
                }
                SecretNotice::Missing { name } => {
                    let what = self.describe_secret(&name);
                    self.toasts.warning_once(
                        &format!("secret-missing:{name}"),
                        format!(
                            "The saved {what} could not be found in the secret store, so it was left empty. Enter it again in Edit Connection."
                        ),
                    );
                }
            }
        }
    }

    /// Nama secret internal (`conn:12:password`) menjadi teks yang dimengerti
    /// user, mis. `password of connection "prod-db"`.
    fn describe_secret(&self, name: &str) -> String {
        let mut parts = name.splitn(3, ':');
        let (kind, id, field) = (parts.next(), parts.next(), parts.next());
        let field_label = match field {
            Some("password") | Some("basic_pass") => "password",
            Some("ssh_password") => "SSH password",
            Some("ssh_private_key") => "SSH private key",
            Some("bearer_token") => "bearer token",
            Some("api_key_value") => "API key",
            Some(other) => other,
            None => "credential",
        };
        let conn_name = id
            .and_then(|id| id.parse::<i64>().ok())
            .and_then(|id| self.connections.iter().find(|c| c.id == Some(id)))
            .map(|c| c.name.clone());
        match (kind, conn_name) {
            (Some("conn") | Some("http"), Some(conn)) => {
                format!("{field_label} of connection \"{conn}\"")
            }
            _ => format!("{field_label} ({name})"),
        }
    }
}
