//! Kebijakan terkelola (M9, fondasi A3): pengaturan yang dipaksakan admin
//! lewat configuration profile (MDM) atau file kebijakan sistem.
//!
//! Sumber, dibaca sekali saat pertama dipakai (yang pertama ditemukan menang):
//! 1. `TABULAR_POLICY_FILE` (JSON) — untuk pengujian dan deployment skrip.
//! 2. macOS: `/Library/Managed Preferences/<user>/id.tabular.database.plist`,
//!    lalu `/Library/Managed Preferences/id.tabular.database.plist` (dipasang
//!    oleh MDM), lalu `/Library/Application Support/Tabular/policy.json`.
//! 3. Linux: `/etc/tabular/policy.json`.
//! 4. Windows: `%ProgramData%\Tabular\policy.json`.
//!
//! Kunci (JSON maupun plist): `disableUpdateCheck`, `automaticDownload`,
//! `installOnQuit`, `minimumVersion`, `disabledNetworkCategories` (array kunci
//! kategori dari `privacy.rs`), `language`. Nilai yang ada selalu mengalahkan
//! pilihan pengguna dan kontrolnya dikunci di UI.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ManagedPolicy {
    /// Matikan pengecekan update otomatis maupun manual.
    pub disable_update_check: Option<bool>,
    /// Paksa unduh update di latar belakang (true) atau hanya notifikasi (false).
    pub automatic_download: Option<bool>,
    /// Paksa update yang sudah diunduh dipasang saat aplikasi ditutup.
    pub install_on_quit: Option<bool>,
    /// Versi minimum yang diizinkan organisasi; di bawahnya UI menampilkan peringatan.
    pub minimum_version: Option<String>,
    /// Kategori jaringan yang dimatikan paksa (lihat `privacy::NetCategory::key`).
    pub disabled_network_categories: Vec<String>,
    /// Bahasa UI yang dipaksakan (kode seperti "id", "ko").
    pub language: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LoadedPolicy {
    pub policy: ManagedPolicy,
    /// File asal kebijakan; `None` berarti tidak ada kebijakan terkelola.
    pub source: Option<PathBuf>,
}

impl LoadedPolicy {
    pub fn is_managed(&self) -> bool {
        self.source.is_some()
    }

    pub fn network_disabled(&self, key: &str) -> bool {
        self.policy
            .disabled_network_categories
            .iter()
            .any(|k| k.eq_ignore_ascii_case(key))
    }

    /// `true` bila versi berjalan lebih rendah dari `minimumVersion`.
    pub fn below_minimum_version(&self) -> bool {
        let Some(min) = self.policy.minimum_version.as_deref() else {
            return false;
        };
        match (
            semver::Version::parse(min.trim().trim_start_matches('v')),
            semver::Version::parse(env!("CARGO_PKG_VERSION")),
        ) {
            (Ok(min), Ok(cur)) => cur < min,
            _ => false,
        }
    }
}

/// Kebijakan aktif (di-cache untuk seluruh proses).
pub fn current() -> &'static LoadedPolicy {
    static POLICY: OnceLock<LoadedPolicy> = OnceLock::new();
    POLICY.get_or_init(|| {
        let loaded = load_from_candidates(&candidate_paths());
        if let Some(src) = &loaded.source {
            log::info!("[POLICY] managed policy loaded from {}", src.display());
        }
        loaded
    })
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(p) = std::env::var("TABULAR_POLICY_FILE")
        && !p.trim().is_empty()
    {
        paths.push(PathBuf::from(p));
    }
    #[cfg(target_os = "macos")]
    {
        let bundle = "id.tabular.database.plist";
        if let Ok(user) = std::env::var("USER") {
            paths.push(
                Path::new("/Library/Managed Preferences")
                    .join(user)
                    .join(bundle),
            );
        }
        paths.push(Path::new("/Library/Managed Preferences").join(bundle));
        paths.push(PathBuf::from(
            "/Library/Application Support/Tabular/policy.json",
        ));
    }
    #[cfg(target_os = "linux")]
    paths.push(PathBuf::from("/etc/tabular/policy.json"));
    #[cfg(target_os = "windows")]
    if let Ok(pd) = std::env::var("ProgramData") {
        paths.push(Path::new(&pd).join("Tabular").join("policy.json"));
    }
    paths
}

fn load_from_candidates(paths: &[PathBuf]) -> LoadedPolicy {
    for path in paths {
        if !path.is_file() {
            continue;
        }
        match read_policy(path) {
            Ok(policy) => {
                return LoadedPolicy {
                    policy,
                    source: Some(path.clone()),
                };
            }
            Err(e) => log::warn!("[POLICY] ignoring {}: {e}", path.display()),
        }
    }
    LoadedPolicy::default()
}

fn read_policy(path: &Path) -> Result<ManagedPolicy, String> {
    let is_plist = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("plist"));
    let json = if is_plist {
        plist_to_json(path)?
    } else {
        std::fs::read_to_string(path).map_err(|e| e.to_string())?
    };
    parse_policy_json(&json)
}

/// Plist terkelola biasanya biner; `plutil` bawaan macOS mengubahnya ke JSON
/// tanpa perlu crate plist tambahan.
#[cfg(target_os = "macos")]
fn plist_to_json(path: &Path) -> Result<String, String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn plist_to_json(_path: &Path) -> Result<String, String> {
    Err("plist policies are only supported on macOS".to_string())
}

pub fn parse_policy_json(json: &str) -> Result<ManagedPolicy, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_keys_and_ignores_unknown() {
        let p = parse_policy_json(
            r#"{"disableUpdateCheck": true, "automaticDownload": false,
                "disabledNetworkCategories": ["ai", "map_tiles"],
                "language": "id", "somethingElse": 1}"#,
        )
        .unwrap();
        assert_eq!(p.disable_update_check, Some(true));
        assert_eq!(p.automatic_download, Some(false));
        assert_eq!(p.install_on_quit, None);
        assert_eq!(p.language.as_deref(), Some("id"));
        let loaded = LoadedPolicy {
            policy: p,
            source: Some("x".into()),
        };
        assert!(loaded.network_disabled("AI"));
        assert!(!loaded.network_disabled("cloud_sync"));
    }

    #[test]
    fn minimum_version_comparison() {
        let mk = |v: &str| LoadedPolicy {
            policy: ManagedPolicy {
                minimum_version: Some(v.into()),
                ..Default::default()
            },
            source: Some("x".into()),
        };
        assert!(mk("999.0.0").below_minimum_version());
        assert!(!mk("0.0.1").below_minimum_version());
        assert!(!mk("not-a-version").below_minimum_version());
    }

    #[test]
    fn first_existing_candidate_wins_and_bad_files_are_skipped() {
        let dir = std::env::temp_dir().join(format!("tabular-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.json");
        let good = dir.join("good.json");
        std::fs::write(&bad, "{not json").unwrap();
        std::fs::write(&good, r#"{"installOnQuit": true}"#).unwrap();
        let loaded = load_from_candidates(&[dir.join("missing.json"), bad, good.clone()]);
        assert_eq!(loaded.source.as_deref(), Some(good.as_path()));
        assert_eq!(loaded.policy.install_on_quit, Some(true));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
