use crate::self_update::UpdateInfo;
use futures_util::StreamExt;
use log::{debug, info, warn};
use std::fs;
#[allow(unused_imports)]
use std::io::Cursor;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UpdateStage {
    Idle,
    Downloading { progress: f32, downloaded: u64, total: Option<u64> },
    Extracting,
    Applying,
    Completed,
    Failed(String),
}

#[derive(Clone)]
pub struct AutoUpdater {
    pub temp_dir: PathBuf,
}

impl AutoUpdater {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let temp_dir = std::env::temp_dir().join("tabular_update");
        fs::create_dir_all(&temp_dir)?;
        
        // Silently clean up any leftover .exe.old files from previous updates on Windows
        #[cfg(target_os = "windows")]
        {
            if let Ok(current_exe) = std::env::current_exe() {
                let old_exe = current_exe.with_extension("exe.old");
                if old_exe.exists() {
                    let _ = fs::remove_file(&old_exe);
                }
            }
        }

        Ok(AutoUpdater { temp_dir })
    }

    /// Download update using chunked HTTP stream and stage/install in-place
    pub async fn download_and_stage_update<F>(
        &self,
        update_info: &UpdateInfo,
        progress_cb: F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        let download_url = update_info
            .download_url
            .as_ref()
            .ok_or("No download URL available for this release")?;

        info!("🚀 Starting auto update download from: {}", download_url);
        progress_cb(UpdateStage::Downloading {
            progress: 0.0,
            downloaded: 0,
            total: None,
        });

        let client = reqwest::Client::builder()
            .user_agent(format!("Tabular/{}", env!("CARGO_PKG_VERSION")))
            .build()?;

        let response = client.get(download_url).send().await?;

        if !response.status().is_success() {
            let err_msg = format!("Download HTTP error: {}", response.status());
            progress_cb(UpdateStage::Failed(err_msg.clone()));
            return Err(err_msg.into());
        }

        let total_size = response.content_length();
        let mut downloaded: u64 = 0;
        let mut content = Vec::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res?;
            downloaded += chunk.len() as u64;
            content.extend_from_slice(&chunk);

            let progress = total_size.map_or(0.0, |total| {
                if total == 0 {
                    0.0
                } else {
                    (downloaded as f32 / total as f32).min(1.0)
                }
            });

            progress_cb(UpdateStage::Downloading {
                progress,
                downloaded,
                total: total_size,
            });
        }

        info!("📦 Update payload downloaded successfully ({} bytes)", content.len());
        progress_cb(UpdateStage::Extracting);

        #[cfg(target_os = "macos")]
        {
            self.stage_macos_update(&content, update_info, &progress_cb).await?;
        }

        #[cfg(target_os = "linux")]
        {
            self.stage_linux_update(&content, update_info, &progress_cb).await?;
        }

        #[cfg(target_os = "windows")]
        {
            self.stage_windows_update(&content, update_info, &progress_cb).await?;
        }

        progress_cb(UpdateStage::Completed);
        Ok(())
    }

    /// Relaunches Tabular application automatically
    pub fn restart_app() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let current_exe = std::env::current_exe()?;
        info!("🔄 Restarting Tabular executable: {:?}", current_exe);

        #[cfg(target_os = "macos")]
        {
            let exe_str = current_exe.to_string_lossy();
            if let Some(app_pos) = exe_str.find(".app/Contents/MacOS/") {
                let app_path = &exe_str[..app_pos + 4];
                info!("🍏 Relaunching macOS bundle via open: {}", app_path);
                std::process::Command::new("open")
                    .args(["-n", app_path])
                    .spawn()?;
                std::process::exit(0);
            }
        }

        std::process::Command::new(&current_exe).spawn()?;
        std::process::exit(0);
    }
}

// Platform Specific Implementation: macOS
#[cfg(target_os = "macos")]
impl AutoUpdater {
    async fn stage_macos_update<F>(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        info!("🍏 Staging macOS update...");
        progress_cb(UpdateStage::Applying);

        let asset_name = update_info
            .asset_name
            .as_deref()
            .unwrap_or("Tabular.dmg");

        let dmg_path = self.temp_dir.join(asset_name);
        fs::write(&dmg_path, content)?;

        let current_exe = std::env::current_exe()?;
        let exe_str = current_exe.to_string_lossy();
        let is_app_bundle = exe_str.contains(".app/Contents/MacOS/");

        // Mount DMG via hdiutil
        let mount_point = self.temp_dir.join("mount");
        let _ = fs::remove_dir_all(&mount_point);
        fs::create_dir_all(&mount_point)?;

        debug!("Mounting DMG {:?} to {:?}", dmg_path, mount_point);
        let mount_status = std::process::Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-mountpoint",
                mount_point.to_str().unwrap(),
                dmg_path.to_str().unwrap(),
            ])
            .output();

        let mounted = match mount_status {
            Ok(output) => output.status.success(),
            Err(_) => false,
        };

        if mounted {
            debug!("DMG mounted successfully. Finding extracted app or binary...");
            let mounted_app = mount_point.join("Tabular.app");

            if mounted_app.exists() && is_app_bundle {
                let app_pos = exe_str.find(".app/Contents/MacOS/").unwrap();
                let current_app_path = PathBuf::from(&exe_str[..app_pos + 4]);
                info!("Replacing macOS bundle at {:?}", current_app_path);

                // Copy binary inside .app bundle directly
                let mounted_binary = mounted_app.join("Contents/MacOS/tabular");
                if mounted_binary.exists() {
                    let res = fs::copy(&mounted_binary, &current_exe);
                    if res.is_ok() {
                        let _ = std::process::Command::new("hdiutil")
                            .args(["detach", mount_point.to_str().unwrap()])
                            .output();
                        let _ = fs::remove_file(&dmg_path);
                        return Ok(());
                    }
                }
            } else if mounted_app.exists() && !is_app_bundle {
                // Standalone binary mode
                let mounted_binary = mounted_app.join("Contents/MacOS/tabular");
                if mounted_binary.exists() {
                    let res = fs::copy(&mounted_binary, &current_exe);
                    if res.is_ok() {
                        let _ = std::process::Command::new("hdiutil")
                            .args(["detach", mount_point.to_str().unwrap()])
                            .output();
                        let _ = fs::remove_file(&dmg_path);
                        return Ok(());
                    }
                }
            }

            let _ = std::process::Command::new("hdiutil")
                .args(["detach", mount_point.to_str().unwrap()])
                .output();
        }

        // Fallback: If in-place replacement was blocked by permissions (SIP/System root), save to Downloads & open DMG
        warn!("In-place macOS bundle update not permitted; falling back to Downloads DMG setup");
        if let Some(downloads_dir) = dirs::download_dir() {
            let download_dmg = downloads_dir.join(asset_name);
            let _ = fs::copy(&dmg_path, &download_dmg);
            let _ = std::process::Command::new("open").arg(&download_dmg).spawn();
        }

        let _ = fs::remove_file(&dmg_path);
        Ok(())
    }
}

// Platform Specific Implementation: Linux
#[cfg(target_os = "linux")]
impl AutoUpdater {
    async fn stage_linux_update<F>(
        &self,
        content: &[u8],
        _update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        info!("🐧 Staging Linux update...");
        progress_cb(UpdateStage::Applying);

        let current_exe = std::env::current_exe()?;
        let extract_dir = self.temp_dir.join("extracted");
        let _ = fs::remove_dir_all(&extract_dir);
        fs::create_dir_all(&extract_dir)?;

        // Decompress tar.gz archive
        let tar_gz = flate2::read::GzDecoder::new(Cursor::new(content));
        let mut archive = tar::Archive::new(tar_gz);
        archive.unpack(&extract_dir)?;

        // Find binary inside extracted output directory
        let mut extracted_binary: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(&extract_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                    if file_name.starts_with("tabular") {
                        extracted_binary = Some(path);
                        break;
                    }
                }
            }
        }

        let new_binary = extracted_binary
            .ok_or("Could not find extracted binary in update archive")?;

        // Set executable permissions (0755)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755));
        }

        // Try replacing binary in-place
        info!("Replacing Linux binary at {:?}", current_exe);
        if fs::rename(&new_binary, &current_exe).is_err() {
            // Atomic rename failed (e.g. cross-filesystem boundary), try copy + remove
            let temp_old_exe = current_exe.with_extension("old");
            let _ = fs::rename(&current_exe, &temp_old_exe);
            if fs::copy(&new_binary, &current_exe).is_ok() {
                let _ = fs::remove_file(&temp_old_exe);
            } else {
                // Rollback if copy fails
                let _ = fs::rename(&temp_old_exe, &current_exe);
                warn!("Linux in-place replacement failed (permission denied); saving to Downloads");
                if let Some(downloads_dir) = dirs::download_dir() {
                    let dest = downloads_dir.join("tabular-latest");
                    let _ = fs::copy(&new_binary, &dest);
                }
            }
        }

        let _ = fs::remove_dir_all(&extract_dir);
        Ok(())
    }
}

// Platform Specific Implementation: Windows
#[cfg(target_os = "windows")]
impl AutoUpdater {
    async fn stage_windows_update<F>(
        &self,
        content: &[u8],
        _update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        info!("🪟 Staging Windows update...");
        progress_cb(UpdateStage::Applying);

        let current_exe = std::env::current_exe()?;
        let extract_dir = self.temp_dir.join("extracted");
        let _ = fs::remove_dir_all(&extract_dir);
        fs::create_dir_all(&extract_dir)?;

        // Unzip windows archive using zip crate
        let mut zip_archive = zip::ZipArchive::new(Cursor::new(content))?;
        zip_archive.extract(&extract_dir)?;

        let mut extracted_binary: Option<PathBuf> = None;
        if let Ok(entries) = fs::read_dir(&extract_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                    if file_name.ends_with(".exe") {
                        extracted_binary = Some(path);
                        break;
                    }
                }
            }
        }

        let new_binary = extracted_binary
            .ok_or("Could not find extracted tabular.exe in Windows zip archive")?;

        let old_exe = current_exe.with_extension("exe.old");
        let _ = fs::remove_file(&old_exe);

        info!("Renaming active Windows executable {:?} -> {:?}", current_exe, old_exe);
        if fs::rename(&current_exe, &old_exe).is_ok() {
            if fs::copy(&new_binary, &current_exe).is_ok() {
                info!("Windows executable replaced successfully in-place!");
                let _ = fs::remove_dir_all(&extract_dir);
                return Ok(());
            } else {
                // Rollback if copy failed
                let _ = fs::rename(&old_exe, &current_exe);
            }
        }

        // Fallback: Write background batch script to swap files on process exit
        warn!("Direct Windows rename failed; creating background update launcher script");
        let bat_script_path = self.temp_dir.join("update_swap.bat");
        let bat_content = format!(
            "@echo off\r\ntimeout /t 1 /nobreak > NUL\r\ncopy /y \"{}\" \"{}\"\r\nstart \"\" \"{}\"\r\ndel \"%~f0\"\r\n",
            new_binary.to_string_lossy(),
            current_exe.to_string_lossy(),
            current_exe.to_string_lossy()
        );

        fs::write(&bat_script_path, bat_content)?;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", &bat_script_path.to_string_lossy()])
            .spawn();

        Ok(())
    }
}
