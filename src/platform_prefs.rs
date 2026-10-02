//! Preferensi platform (M6–M8, M12): bahasa UI, perilaku update, toggle
//! jaringan, Touch ID, dan Handoff.
//!
//! Disimpan di `<data dir>/platform_prefs.json`, terpisah dari tabel
//! `preferences`, sehingga menambah opsi tidak menyentuh skema KV dan mode
//! headless (`tabular mcp`, CLI) bisa membacanya tanpa GUI. Nilainya dipegang
//! global karena kode headless (update checker, klien sync, AI, tile peta)
//! perlu membacanya tanpa `&Tabular`.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PlatformPrefs {
    /// Kode bahasa UI; kosong berarti ikut bahasa sistem.
    pub language: String,
    /// Unduh update di latar belakang begitu terdeteksi.
    pub update_auto_download: bool,
    /// Pasang update yang sudah diunduh saat aplikasi ditutup.
    pub update_install_on_quit: bool,
    /// Versi yang dipilih "Skip This Version".
    pub update_skipped_version: Option<String>,
    /// Izin per kategori jaringan (`privacy::NetCategory::key`); kunci yang
    /// tidak ada memakai default kategori.
    pub network: BTreeMap<String, bool>,
    /// Tawarkan Touch ID untuk membuka vault sync (macOS).
    pub biometric_unlock: bool,
    /// Umumkan tab aktif lewat Handoff (macOS).
    pub handoff_enabled: bool,
}

impl Default for PlatformPrefs {
    fn default() -> Self {
        Self {
            language: String::new(),
            update_auto_download: true,
            update_install_on_quit: false,
            update_skipped_version: None,
            network: BTreeMap::new(),
            biometric_unlock: false,
            handoff_enabled: false,
        }
    }
}

impl PlatformPrefs {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Nilai rusak jatuh ke default supaya preferensi lain tetap termuat.
    pub fn from_json(raw: &str) -> Self {
        serde_json::from_str(raw).unwrap_or_else(|e| {
            log::warn!("[PREFS] invalid platform_prefs JSON, using defaults: {e}");
            Self::default()
        })
    }
}

fn cell() -> &'static RwLock<PlatformPrefs> {
    static CELL: OnceLock<RwLock<PlatformPrefs>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(PlatformPrefs::default()))
}

/// Salinan preferensi saat ini (belum digabung dengan kebijakan terkelola).
pub fn current() -> PlatformPrefs {
    cell().read().map(|g| g.clone()).unwrap_or_default()
}

pub fn set(prefs: PlatformPrefs) {
    let language_changed = match cell().write() {
        Ok(mut g) => {
            let changed = g.language != prefs.language;
            *g = prefs;
            changed
        }
        Err(_) => false,
    };
    if language_changed {
        crate::i18n::set_language(&effective_language());
    }
}

fn file_path() -> std::path::PathBuf {
    crate::config::get_data_dir().join("platform_prefs.json")
}

/// Muat dari disk (dipanggil sekali saat startup, setelah data dir final).
pub fn load_from_disk() {
    let prefs = match std::fs::read_to_string(file_path()) {
        Ok(raw) => PlatformPrefs::from_json(&raw),
        Err(_) => PlatformPrefs::default(),
    };
    set(prefs);
    apply_language();
}

/// Tulis preferensi saat ini ke disk.
pub fn persist() {
    let path = file_path();
    if let Err(e) = std::fs::write(&path, current().to_json()) {
        log::warn!("[PREFS] cannot write {}: {e}", path.display());
    }
}

/// Terapkan bahasa efektif ke i18n (dipanggil sekali saat preferensi dimuat,
/// karena `set` hanya bereaksi pada perubahan).
pub fn apply_language() {
    crate::i18n::set_language(&effective_language());
}

/// Ubah sebagian preferensi di tempat.
pub fn update(f: impl FnOnce(&mut PlatformPrefs)) {
    let mut p = current();
    f(&mut p);
    set(p);
}

// ── Nilai efektif (preferensi pengguna + kebijakan terkelola) ───────────────

pub fn update_check_disabled_by_policy() -> bool {
    crate::managed_policy::current()
        .policy
        .disable_update_check
        .unwrap_or(false)
}

pub fn effective_auto_download() -> bool {
    crate::managed_policy::current()
        .policy
        .automatic_download
        .unwrap_or_else(|| current().update_auto_download)
}

pub fn effective_install_on_quit() -> bool {
    crate::managed_policy::current()
        .policy
        .install_on_quit
        .unwrap_or_else(|| current().update_install_on_quit)
}

pub fn effective_language() -> String {
    crate::managed_policy::current()
        .policy
        .language
        .clone()
        .unwrap_or_else(|| current().language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_and_partial_input() {
        let mut p = PlatformPrefs {
            language: "id".into(),
            ..Default::default()
        };
        p.network.insert("ai".into(), false);
        assert_eq!(PlatformPrefs::from_json(&p.to_json()), p);

        // Field yang belum ada di versi lama memakai default.
        let partial = PlatformPrefs::from_json(r#"{"language":"ko"}"#);
        assert_eq!(partial.language, "ko");
        assert!(partial.update_auto_download);

        assert_eq!(
            PlatformPrefs::from_json("garbage"),
            PlatformPrefs::default()
        );
    }
}
