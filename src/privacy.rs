//! Inventaris koneksi keluar + toggle (M12).
//!
//! Setiap fitur yang menghubungi server di luar database pengguna terdaftar
//! sebagai [`NetCategory`]. Titik panggil jaringan memanggil [`check`] sebelum
//! mengirim request: hasilnya dicatat di log memori (tampil di Preferences >
//! Privacy) dan request dibatalkan bila kategori dimatikan pengguna atau oleh
//! kebijakan terkelola.
//!
//! Log hanya menyimpan host + path (tanpa query string) supaya token atau
//! API key di URL tidak ikut tercatat.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetCategory {
    UpdateCheck,
    UpdateDownload,
    CloudSync,
    Ai,
    MapTiles,
    Handoff,
}

impl NetCategory {
    pub const ALL: [NetCategory; 6] = [
        NetCategory::UpdateCheck,
        NetCategory::UpdateDownload,
        NetCategory::CloudSync,
        NetCategory::Ai,
        NetCategory::MapTiles,
        NetCategory::Handoff,
    ];

    /// Kunci stabil untuk preferensi dan kebijakan terkelola.
    pub fn key(self) -> &'static str {
        match self {
            NetCategory::UpdateCheck => "update_check",
            NetCategory::UpdateDownload => "update_download",
            NetCategory::CloudSync => "cloud_sync",
            NetCategory::Ai => "ai",
            NetCategory::MapTiles => "map_tiles",
            NetCategory::Handoff => "handoff",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NetCategory::UpdateCheck => "Update check",
            NetCategory::UpdateDownload => "Update download",
            NetCategory::CloudSync => "Cloud Sync & Teams",
            NetCategory::Ai => "AI Assistant (API)",
            NetCategory::MapTiles => "Map tiles",
            NetCategory::Handoff => "Handoff",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            NetCategory::UpdateCheck => {
                "Asks GitHub for the latest release, at most once a day or when you click Check for Updates. Sends the app version in the User-Agent."
            }
            NetCategory::UpdateDownload => {
                "Downloads the release archive from GitHub when an update is available."
            }
            NetCategory::CloudSync => {
                "Talks to the sync server only while you are signed in. Connection secrets are end-to-end encrypted before upload."
            }
            NetCategory::Ai => {
                "Sends your prompt, and the schema or SQL you attach, to the AI provider you configured. CLI agents run as separate programs and are not covered here."
            }
            NetCategory::MapTiles => {
                "Downloads OpenStreetMap tiles for the Map view of geometry columns. Tiles are cached locally."
            }
            NetCategory::Handoff => {
                "Shares the active connection name and query with your other Apple devices through Handoff (iCloud account, Bluetooth)."
            }
        }
    }

    /// Endpoint yang dihubungi (untuk ditampilkan; bukan allowlist).
    pub fn endpoints(self) -> &'static [&'static str] {
        match self {
            NetCategory::UpdateCheck => &["api.github.com", "github.com"],
            NetCategory::UpdateDownload => &["github.com", "objects.githubusercontent.com"],
            NetCategory::CloudSync => &["api.tabular.id (or your custom sync server)"],
            NetCategory::Ai => &[
                "api.openai.com",
                "api.groq.com",
                "models.inference.ai.azure.com",
                "api.anthropic.com",
                "generativelanguage.googleapis.com",
                "api.x.ai",
                "openrouter.ai",
                "localhost (Ollama, llama.cpp, MLX)",
                "your custom base URL",
            ],
            NetCategory::MapTiles => &["tile.openstreetmap.org"],
            NetCategory::Handoff => &["Apple Handoff (local devices)"],
        }
    }

    /// Nilai awal bila pengguna belum pernah mengubah toggle. Handoff mati
    /// secara default karena ikut membagikan teks query.
    pub fn default_allowed(self) -> bool {
        !matches!(self, NetCategory::Handoff)
    }

    /// Kategori yang hanya relevan di platform tertentu.
    pub fn available(self) -> bool {
        match self {
            NetCategory::Handoff => cfg!(target_os = "macos"),
            NetCategory::UpdateCheck | NetCategory::UpdateDownload => {
                crate::self_update::SELF_UPDATE_SUPPORTED
            }
            _ => true,
        }
    }
}

/// Koneksi yang selalu dimulai langsung oleh pengguna; ditampilkan di halaman
/// Privacy untuk kelengkapan, tanpa toggle.
pub const USER_INITIATED: &[(&str, &str)] = &[
    (
        "Database connections",
        "Only the servers you add, directly or through your SSH tunnel / proxy.",
    ),
    (
        "HTTP Client",
        "Requests you send from the HTTP client tab, to the URLs you type.",
    ),
    (
        "External CLI agents",
        "claude / gemini / agy run as separate programs with their own network access.",
    ),
    (
        "Links you open",
        "Release pages, documentation and sign-in pages open in your browser.",
    ),
];

/// Apakah kategori diizinkan (kebijakan terkelola menang atas preferensi).
pub fn allowed(cat: NetCategory) -> bool {
    if blocked_by_policy(cat) {
        return false;
    }
    crate::platform_prefs::current()
        .network
        .get(cat.key())
        .copied()
        .unwrap_or_else(|| cat.default_allowed())
}

pub fn blocked_by_policy(cat: NetCategory) -> bool {
    crate::managed_policy::current().network_disabled(cat.key())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub time: String,
    pub category: NetCategory,
    pub target: String,
    pub allowed: bool,
}

const LOG_CAPACITY: usize = 200;

fn log_buf() -> &'static Mutex<VecDeque<Entry>> {
    static LOG: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();
    LOG.get_or_init(|| Mutex::new(VecDeque::with_capacity(LOG_CAPACITY)))
}

/// Host + path saja; query string, fragmen, dan kredensial di URL dibuang.
pub fn redact_target(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => format!("{}{}", u.host_str().unwrap_or_default(), u.path()),
        Err(_) => url
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .rsplit('@')
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Catat lalu putuskan: `Ok(())` bila boleh dikirim, `Err` berisi pesan untuk
/// pengguna bila diblokir.
pub fn check(cat: NetCategory, url: &str) -> Result<(), String> {
    let ok = allowed(cat);
    record(cat, url, ok);
    if ok {
        Ok(())
    } else if blocked_by_policy(cat) {
        Err(format!(
            "{} is disabled by your organization's policy.",
            cat.label()
        ))
    } else {
        Err(format!(
            "{} is turned off in Preferences > Privacy.",
            cat.label()
        ))
    }
}

pub fn record(cat: NetCategory, url: &str, allowed: bool) {
    let entry = Entry {
        time: chrono::Local::now().format("%H:%M:%S").to_string(),
        category: cat,
        target: redact_target(url),
        allowed,
    };
    if !allowed {
        log::info!(
            "[PRIVACY] blocked {} request to {}",
            cat.key(),
            entry.target
        );
    }
    if let Ok(mut buf) = log_buf().lock() {
        if buf.len() >= LOG_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry);
    }
}

/// Log terbaru, paling baru di depan.
pub fn recent() -> Vec<Entry> {
    log_buf()
        .lock()
        .map(|b| b.iter().rev().cloned().collect())
        .unwrap_or_default()
}

pub fn clear_log() {
    if let Ok(mut b) = log_buf().lock() {
        b.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_drops_query_and_credentials() {
        assert_eq!(
            redact_target("https://user:pw@api.example.com/v1/x?key=secret#f"),
            "api.example.com/v1/x"
        );
        assert_eq!(redact_target("not a url?token=1"), "not a url");
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<_> = NetCategory::ALL.iter().map(|c| c.key()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), NetCategory::ALL.len());
    }

    #[test]
    fn check_respects_preference_and_logs() {
        crate::platform_prefs::update(|p| {
            p.network.insert(NetCategory::MapTiles.key().into(), false);
        });
        assert!(check(NetCategory::MapTiles, "https://tile.openstreetmap.org/1/1/1.png").is_err());
        let hit = recent()
            .into_iter()
            .find(|e| e.category == NetCategory::MapTiles)
            .expect("logged");
        assert!(!hit.allowed);
        crate::platform_prefs::update(|p| {
            p.network.remove(NetCategory::MapTiles.key());
        });
        assert!(allowed(NetCategory::MapTiles));
        assert!(!NetCategory::Handoff.default_allowed());
    }
}
