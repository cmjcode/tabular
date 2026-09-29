//! Manifest, instalasi, dan pemuatan plugin driver dari disk.
//!
//! Tata letak: `<data_dir>/plugins/drivers/<engine-id>/manifest.json` plus
//! file entry (`.wasm` atau executable sidecar) di folder yang sama. Status
//! enable/disable dan persetujuan sidecar disimpan di
//! `<data_dir>/plugins/drivers/state.json`.

use super::registry::{self, DriverSource, RegisteredDriver};
use super::{DriverError, DriverResult, EngineDescriptor};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub const DRIVER_ABI: &str = "tabular-driver-v1";
const MANIFEST_FILE: &str = "manifest.json";
const STATE_FILE: &str = "state.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriverKind {
    Wasm,
    Sidecar,
}

/// Path entry: satu path untuk semua platform, atau per `"<os>-<arch>"`
/// (mis. `"macos-aarch64"`, `"linux-x86_64"`, `"windows-x86_64"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EntrySpec {
    Single(String),
    PerPlatform(BTreeMap<String, String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    /// Host yang boleh dihubungi plugin Wasm. `{connection.host}` = host koneksi.
    pub http_hosts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverManifest {
    pub abi: String,
    pub kind: DriverKind,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    pub entry: EntrySpec,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub permissions: Permissions,
    pub engine: EngineDescriptor,
}

pub fn current_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

impl DriverManifest {
    pub fn parse(json: &str) -> DriverResult<Self> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|e| DriverError::Protocol(format!("invalid manifest.json: {e}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> DriverResult<()> {
        if self.abi != DRIVER_ABI {
            return Err(DriverError::Protocol(format!(
                "unsupported driver ABI '{}' (expected '{DRIVER_ABI}')",
                self.abi
            )));
        }
        if !EngineDescriptor::is_valid_id(&self.engine.id) {
            return Err(DriverError::Protocol(format!(
                "invalid engine id '{}'",
                self.engine.id
            )));
        }
        if crate::models::enums::DatabaseType::is_builtin_id(&self.engine.id) {
            return Err(DriverError::Protocol(format!(
                "engine id '{}' is reserved for a builtin driver",
                self.engine.id
            )));
        }
        if self.engine.name.trim().is_empty() {
            return Err(DriverError::Protocol("engine name is empty".into()));
        }
        Ok(())
    }

    /// Path entry relatif untuk platform ini, sudah dicek tidak keluar folder.
    pub fn entry_relative(&self) -> DriverResult<PathBuf> {
        let raw = match &self.entry {
            EntrySpec::Single(p) => p.clone(),
            EntrySpec::PerPlatform(map) => {
                map.get(&current_platform()).cloned().ok_or_else(|| {
                    DriverError::Unsupported(format!(
                        "no build of this driver for platform {}",
                        current_platform()
                    ))
                })?
            }
        };
        let path = PathBuf::from(&raw);
        let safe = !raw.is_empty()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !safe {
            return Err(DriverError::Permission(format!(
                "entry path '{raw}' must stay inside the plugin folder"
            )));
        }
        Ok(path)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DriversState {
    pub disabled: BTreeSet<String>,
    /// engine id -> SHA-256 executable sidecar yang sudah disetujui pengguna.
    pub approved_sidecars: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverStatus {
    Loaded,
    Disabled,
    /// Sidecar baru atau binary-nya berubah; perlu persetujuan eksplisit.
    NeedsApproval,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct InstalledDriver {
    pub dir: PathBuf,
    pub manifest: DriverManifest,
    pub entry_path: PathBuf,
    pub sha256: String,
    pub status: DriverStatus,
}

pub fn drivers_dir() -> PathBuf {
    crate::config::get_data_dir().join("plugins").join("drivers")
}

pub fn read_state(dir: &Path) -> DriversState {
    std::fs::read_to_string(dir.join(STATE_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn write_state(dir: &Path, state: &DriversState) -> DriverResult<()> {
    std::fs::create_dir_all(dir).map_err(|e| DriverError::Plugin(e.to_string()))?;
    let json =
        serde_json::to_string_pretty(state).map_err(|e| DriverError::Plugin(e.to_string()))?;
    std::fs::write(dir.join(STATE_FILE), json).map_err(|e| DriverError::Plugin(e.to_string()))
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(hex::encode(sha2::Sha256::digest(&bytes)))
}

/// Baca satu folder plugin tanpa mendaftarkannya.
pub fn inspect(plugin_dir: &Path) -> DriverResult<(DriverManifest, PathBuf, String)> {
    let json = std::fs::read_to_string(plugin_dir.join(MANIFEST_FILE))
        .map_err(|e| DriverError::Protocol(format!("cannot read manifest.json: {e}")))?;
    let manifest = DriverManifest::parse(&json)?;
    let entry_path = plugin_dir.join(manifest.entry_relative()?);
    let sha256 = sha256_file(&entry_path).map_err(|e| {
        DriverError::Protocol(format!(
            "cannot read entry '{}': {e}",
            entry_path.display()
        ))
    })?;
    Ok((manifest, entry_path, sha256))
}

fn build_driver(
    manifest: &DriverManifest,
    entry_path: &Path,
) -> DriverResult<(Arc<dyn super::EngineDriver>, DriverSource)> {
    match manifest.kind {
        DriverKind::Wasm => {
            let bytes = std::fs::read(entry_path)
                .map_err(|e| DriverError::Plugin(format!("cannot read wasm: {e}")))?;
            let driver = super::wasm_host::WasmDriver::load(
                &bytes,
                manifest.engine.clone(),
                manifest.permissions.http_hosts.clone(),
            )?;
            Ok((
                Arc::new(driver),
                DriverSource::Wasm {
                    path: entry_path.to_path_buf(),
                },
            ))
        }
        DriverKind::Sidecar => {
            #[cfg(not(target_os = "ios"))]
            {
                let driver = super::sidecar::SidecarDriver::new(
                    entry_path.to_path_buf(),
                    manifest.args.clone(),
                    manifest.engine.clone(),
                );
                Ok((
                    Arc::new(driver),
                    DriverSource::Sidecar {
                        path: entry_path.to_path_buf(),
                    },
                ))
            }
            #[cfg(target_os = "ios")]
            {
                Err(DriverError::Unsupported(
                    "sidecar drivers are not available on iOS".into(),
                ))
            }
        }
    }
}

/// Pindai `dir`, daftarkan driver yang boleh dimuat, dan kembalikan status
/// semua plugin yang ditemukan.
pub fn load_from(dir: &Path) -> Vec<InstalledDriver> {
    let state = read_state(dir);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let plugin_dir = entry.path();
        if !plugin_dir.is_dir() {
            continue;
        }
        let (manifest, entry_path, sha256) = match inspect(&plugin_dir) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[DRIVER-PLUGIN] Skipping {}: {}", plugin_dir.display(), e);
                continue;
            }
        };
        let id = manifest.engine.id.clone();
        let status = if state.disabled.contains(&id) {
            DriverStatus::Disabled
        } else if manifest.kind == DriverKind::Sidecar
            && state.approved_sidecars.get(&id) != Some(&sha256)
        {
            DriverStatus::NeedsApproval
        } else {
            match build_driver(&manifest, &entry_path).and_then(|(driver, source)| {
                registry::register(RegisteredDriver {
                    driver,
                    source,
                    version: manifest.version.clone(),
                })
            }) {
                Ok(()) => DriverStatus::Loaded,
                Err(e) => {
                    log::warn!("[DRIVER-PLUGIN] Failed to load driver '{}': {}", id, e);
                    DriverStatus::Failed(e.to_string())
                }
            }
        };
        if status != DriverStatus::Loaded {
            registry::unregister(&id);
        }
        found.push(InstalledDriver {
            dir: plugin_dir,
            manifest,
            entry_path,
            sha256,
            status,
        });
    }
    found.sort_by(|a, b| a.manifest.engine.name.cmp(&b.manifest.engine.name));
    found
}

/// Muat semua driver terpasang dari folder standar.
pub fn load_installed() -> Vec<InstalledDriver> {
    if cfg!(target_os = "ios") {
        return Vec::new();
    }
    let found = load_from(&drivers_dir());
    let loaded = found
        .iter()
        .filter(|d| d.status == DriverStatus::Loaded)
        .count();
    log::info!(
        "[DRIVER-PLUGIN] {} driver plugin(s) found, {} loaded",
        found.len(),
        loaded
    );
    found
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Salin folder plugin (berisi `manifest.json`) ke folder driver. Plugin
/// dengan id yang sama diganti. Tidak mendaftarkan driver; panggil
/// [`load_from`] sesudahnya.
pub fn install_from_dir(src: &Path, drivers_root: &Path) -> DriverResult<DriverManifest> {
    let (manifest, _, _) = inspect(src)?;
    let target = drivers_root.join(&manifest.engine.id);
    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| DriverError::Plugin(format!("cannot replace old version: {e}")))?;
    }
    copy_dir(src, &target).map_err(|e| DriverError::Plugin(format!("copy failed: {e}")))?;
    #[cfg(unix)]
    if manifest.kind == DriverKind::Sidecar {
        use std::os::unix::fs::PermissionsExt;
        let entry = target.join(manifest.entry_relative()?);
        if let Ok(meta) = std::fs::metadata(&entry) {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o755);
            let _ = std::fs::set_permissions(&entry, perms);
        }
    }
    registry::unregister(&manifest.engine.id);
    log::info!(
        "[DRIVER-PLUGIN] Installed driver '{}' {}",
        manifest.engine.id,
        manifest.version
    );
    Ok(manifest)
}

/// Hapus plugin `id` dari folder driver. Koneksi yang memakainya tetap ada
/// dan tampil sebagai "Driver not installed".
pub fn uninstall(id: &str, drivers_root: &Path) -> DriverResult<()> {
    if !EngineDescriptor::is_valid_id(id) {
        return Err(DriverError::Permission(format!("invalid engine id '{id}'")));
    }
    registry::unregister(id);
    let target = drivers_root.join(id);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| DriverError::Plugin(e.to_string()))?;
    }
    let mut state = read_state(drivers_root);
    state.disabled.remove(id);
    state.approved_sidecars.remove(id);
    write_state(drivers_root, &state)
}

pub fn set_enabled(id: &str, enabled: bool, drivers_root: &Path) -> DriverResult<()> {
    let mut state = read_state(drivers_root);
    if enabled {
        state.disabled.remove(id);
    } else {
        state.disabled.insert(id.to_string());
        registry::unregister(id);
    }
    write_state(drivers_root, &state)
}

/// Setujui executable sidecar dengan hash tertentu. Bila binary berubah,
/// hash tidak cocok lagi dan persetujuan harus diulang.
pub fn approve_sidecar(id: &str, sha256: &str, drivers_root: &Path) -> DriverResult<()> {
    let mut state = read_state(drivers_root);
    state
        .approved_sidecars
        .insert(id.to_string(), sha256.to_string());
    write_state(drivers_root, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(id: &str, kind: &str, entry: &str) -> String {
        format!(
            r#"{{"abi":"{DRIVER_ABI}","kind":"{kind}","version":"0.1.0","entry":"{entry}",
               "permissions":{{"http_hosts":["{{connection.host}}"]}},
               "engine":{{"id":"{id}","name":"Test {id}"}}}}"#
        )
    }

    #[test]
    fn parses_and_validates_manifest() {
        let m = DriverManifest::parse(&manifest_json("clickhouse", "wasm", "driver.wasm")).unwrap();
        assert_eq!(m.kind, DriverKind::Wasm);
        assert_eq!(m.entry_relative().unwrap(), PathBuf::from("driver.wasm"));
        assert_eq!(m.permissions.http_hosts, vec!["{connection.host}"]);

        assert!(DriverManifest::parse(&manifest_json("mysql", "wasm", "x.wasm")).is_err());
        let bad_abi = manifest_json("x", "wasm", "x.wasm").replace(DRIVER_ABI, "v0");
        assert!(DriverManifest::parse(&bad_abi).is_err());
    }

    #[test]
    fn rejects_entry_paths_outside_plugin_dir() {
        for entry in ["../evil", "/usr/bin/evil", "a/../../b", ""] {
            let m = DriverManifest::parse(&manifest_json("x", "sidecar", entry)).unwrap();
            assert!(m.entry_relative().is_err(), "{entry} should be rejected");
        }
        let m = DriverManifest::parse(&manifest_json("x", "sidecar", "bin/x")).unwrap();
        assert!(m.entry_relative().is_ok());
    }

    #[test]
    fn per_platform_entry_selects_current_platform() {
        let json = format!(
            r#"{{"abi":"{DRIVER_ABI}","kind":"sidecar","version":"1","entry":{{"{}":"bin/here","plan9-mips":"bin/other"}},
               "engine":{{"id":"x","name":"X"}}}}"#,
            current_platform()
        );
        let m = DriverManifest::parse(&json).unwrap();
        assert_eq!(m.entry_relative().unwrap(), PathBuf::from("bin/here"));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tabular-driver-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sidecar_needs_approval_until_hash_matches() {
        let root = temp_dir("approval");
        let plugin = root.join("src-plugin");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("manifest.json"),
            manifest_json("fake-sidecar", "sidecar", "run.sh"),
        )
        .unwrap();
        std::fs::write(plugin.join("run.sh"), "#!/bin/sh\n").unwrap();

        let drivers = root.join("drivers");
        install_from_dir(&plugin, &drivers).unwrap();
        let found = load_from(&drivers);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].status, DriverStatus::NeedsApproval);

        approve_sidecar("fake-sidecar", &found[0].sha256, &drivers).unwrap();
        let found = load_from(&drivers);
        assert_eq!(found[0].status, DriverStatus::Loaded);

        // Binary berubah: persetujuan lama tidak berlaku.
        std::fs::write(
            drivers.join("fake-sidecar/run.sh"),
            "#!/bin/sh\necho changed\n",
        )
        .unwrap();
        let found = load_from(&drivers);
        assert_eq!(found[0].status, DriverStatus::NeedsApproval);
        assert!(!registry::is_installed("fake-sidecar"));

        set_enabled("fake-sidecar", false, &drivers).unwrap();
        assert_eq!(load_from(&drivers)[0].status, DriverStatus::Disabled);

        uninstall("fake-sidecar", &drivers).unwrap();
        assert!(load_from(&drivers).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
