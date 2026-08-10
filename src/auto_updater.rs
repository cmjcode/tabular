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
    /// Update completed. On macOS, contains the path to the staged helper script
    /// that should be executed on restart to apply the update.
    Completed(Option<PathBuf>),
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

        // staged_script carries back the macOS helper script path (None on other platforms)
        let staged_script: Option<PathBuf>;

        #[cfg(target_os = "macos")]
        {
            let staged = self.stage_macos_update(&content, update_info, &progress_cb).await?;
            staged_script = staged;
        }

        #[cfg(target_os = "linux")]
        {
            self.stage_linux_update(&content, update_info, &progress_cb).await?;
            staged_script = None;
        }

        #[cfg(target_os = "windows")]
        {
            self.stage_windows_update(&content, update_info, &progress_cb).await?;
            staged_script = None;
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            staged_script = None;
        }

        progress_cb(UpdateStage::Completed(staged_script));
        Ok(())
    }

    /// Relaunches Tabular application automatically.
    ///
    /// On macOS, if a staged update script path is provided, the helper script is
    /// executed first (it waits for this process to exit, replaces the app, and
    /// relaunches it). This avoids replacing the binary while it is running,
    /// which would trigger a code-signature kill (SIGKILL / Code Signature Invalid).
    pub fn restart_app(
        staged_script: Option<&PathBuf>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let current_exe = std::env::current_exe()?;
        info!("🔄 Restarting Tabular executable: {:?}", current_exe);

        #[cfg(target_os = "macos")]
        {
            // If a staged update script is available, run it and exit.
            // The script waits for us to quit, then replaces the .app and relaunches.
            if let Some(script_path) = staged_script {
                info!("🍏 Launching staged update helper script: {:?}", script_path);
                std::process::Command::new("bash")
                    .arg(script_path)
                    .spawn()?;
                std::process::exit(0);
            }

            // Normal relaunch (no staged update)
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

        #[cfg(not(target_os = "macos"))]
        let _ = staged_script; // unused on other platforms

        std::process::Command::new(&current_exe).spawn()?;
        std::process::exit(0);
    }
}

// ─── Platform Specific: macOS ────────────────────────────────────────────────
// TIDAK DIUBAH — identik dengan implementasi sebelumnya
#[cfg(target_os = "macos")]
impl AutoUpdater {
    /// Stage a macOS update safely WITHOUT replacing the currently-running binary.
    ///
    /// # Why not replace in-place?
    /// macOS enforces code-signing at the page level. Overwriting the executable
    /// while it is mapped into memory causes the kernel to detect a signature
    /// mismatch and immediately sends SIGKILL (`Code Signature Invalid`).
    ///
    /// # Safe staged approach
    /// 1. Download & mount the DMG (unchanged).
    /// 2. Copy the new `.app` bundle to a **staging directory** in `$TMPDIR`.
    /// 3. Write a shell helper script that will:
    ///    - Wait briefly for this process to exit.
    ///    - Replace the installed `.app` with the staged one.
    ///    - Clear macOS quarantine attribute.
    ///    - Re-launch the app.
    /// 4. Returns the staged helper script path for `restart_app()` to use.
    async fn stage_macos_update<F>(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        info!("🍏 Staging macOS update (safe staged approach)...");
        progress_cb(UpdateStage::Applying);

        let asset_name = update_info
            .asset_name
            .as_deref()
            .unwrap_or("Tabular.dmg");

        let dmg_path = self.temp_dir.join(asset_name);
        fs::write(&dmg_path, content)?;

        let current_exe = std::env::current_exe()?;
        let exe_str = current_exe.to_string_lossy();

        // Determine the current .app bundle path (if running from inside a .app)
        let current_app_path: Option<PathBuf> =
            if let Some(app_pos) = exe_str.find(".app/Contents/MacOS/") {
                Some(PathBuf::from(&exe_str[..app_pos + 4]))
            } else {
                None
            };

        // Mount DMG via hdiutil
        let mount_point = self.temp_dir.join("mount");
        let _ = fs::remove_dir_all(&mount_point);
        fs::create_dir_all(&mount_point)?;

        debug!("Mounting DMG {:?} to {:?}", dmg_path, mount_point);
        let mount_output = std::process::Command::new("hdiutil")
            .args([
                "attach",
                "-nobrowse",
                "-mountpoint",
                mount_point.to_str().unwrap(),
                dmg_path.to_str().unwrap(),
            ])
            .output();

        let mounted = matches!(mount_output, Ok(ref o) if o.status.success());

        if mounted {
            debug!("DMG mounted. Looking for Tabular.app inside mount...");
            let mounted_app = mount_point.join("Tabular.app");

            if mounted_app.exists() {
                // Stage: copy the new .app to a temp location (NOT over the live install)
                let staging_dir = self.temp_dir.join("tabular_staged");
                let _ = fs::remove_dir_all(&staging_dir);
                fs::create_dir_all(&staging_dir)?;

                let staged_app = staging_dir.join("Tabular.app");
                info!("📋 Copying new .app to staging dir: {:?}", staged_app);

                // Use system cp -R for reliable deep-copy of .app bundle
                let cp_status = std::process::Command::new("cp")
                    .args(["-R", mounted_app.to_str().unwrap(), staged_app.to_str().unwrap()])
                    .status();

                let _ = std::process::Command::new("hdiutil")
                    .args(["detach", mount_point.to_str().unwrap()])
                    .output();
                let _ = fs::remove_file(&dmg_path);

                if cp_status.map_or(false, |s| s.success()) && staged_app.exists() {
                    // Determine install target (default to current .app location or /Applications)
                    let install_target = current_app_path
                        .as_deref()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| "/Applications/Tabular.app".to_string());

                    // Write a helper shell script that runs AFTER we exit
                    let helper_script_path = self.temp_dir.join("tabular_update_helper.sh");
                    let script_content = format!(
                        "#!/bin/bash\n\
                        # Tabular auto-update helper — runs after the app exits\n\
                        sleep 1\n\
                        # Replace installed .app with staged version\n\
                        rm -rf \"{install}\"\n\
                        cp -R \"{staged}\" \"{install}\"\n\
                        # Clear macOS quarantine attribute\n\
                        xattr -cr \"{install}\" 2>/dev/null || true\n\
                        # Remove this helper script\n\
                        rm -f \"{script}\"\n\
                        # Relaunch the app\n\
                        open \"{install}\"\n",
                        install = install_target,
                        staged = staged_app.to_string_lossy(),
                        script = helper_script_path.to_string_lossy(),
                    );

                    fs::write(&helper_script_path, script_content)?;

                    // Make the helper script executable
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&helper_script_path, fs::Permissions::from_mode(0o755))?;
                    }

                    info!("✅ Update staged. Helper script ready at {:?}", helper_script_path);
                    return Ok(Some(helper_script_path));
                } else {
                    warn!("❌ Failed to copy new .app to staging dir; falling back to Downloads DMG");
                }
            } else {
                let _ = std::process::Command::new("hdiutil")
                    .args(["detach", mount_point.to_str().unwrap()])
                    .output();
                let _ = fs::remove_file(&dmg_path);
                warn!("Tabular.app not found inside mounted DMG; falling back to Downloads");
            }
        } else {
            warn!("Failed to mount DMG; falling back to Downloads");
            let _ = fs::remove_file(&dmg_path);
        }

        // Fallback: save DMG to Downloads and open it so the user can update manually
        warn!("Staged update could not complete; placing DMG in Downloads for manual installation");
        let dmg_fallback_path = self.temp_dir.join(asset_name);
        if dmg_fallback_path.exists() {
            if let Some(downloads_dir) = dirs::download_dir() {
                let download_dmg = downloads_dir.join(asset_name);
                let _ = fs::copy(&dmg_fallback_path, &download_dmg);
                let _ = std::process::Command::new("open").arg(&download_dmg).spawn();
            }
            let _ = fs::remove_file(&dmg_fallback_path);
        }

        Ok(None)
    }
}

// ─── Platform Specific: Linux ────────────────────────────────────────────────
// TIDAK DIUBAH — identik dengan implementasi sebelumnya
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

// ─── Platform Specific: Windows ──────────────────────────────────────────────
// DIPERBAIKI — dual-path: MSI silent install ATAU ZIP in-place + PowerShell helper
#[cfg(target_os = "windows")]
impl AutoUpdater {
    async fn stage_windows_update<F>(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        use crate::self_update::WindowsUpdateKind;

        info!("🪟 Staging Windows update...");
        progress_cb(UpdateStage::Applying);

        // ── Path A: MSI silent install ────────────────────────────────────
        if matches!(update_info.windows_update_kind, Some(WindowsUpdateKind::Msi)) {
            return self.apply_msi_update(content, update_info).await;
        }

        // ── Path B: ZIP / EXE in-place ────────────────────────────────────
        self.apply_zip_update(content, update_info, progress_cb).await
    }

    /// Jalankan MSI installer secara silent menggunakan msiexec.
    /// msiexec menangani semua kompleksitas UAC dan penggantian file secara aman.
    async fn apply_msi_update(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let asset_name = update_info
            .asset_name
            .as_deref()
            .unwrap_or("Tabular-update.msi");

        let msi_path = self.temp_dir.join(asset_name);
        info!("💾 Writing MSI to {:?}", msi_path);
        fs::write(&msi_path, content)?;

        info!("🔧 Running msiexec silent install: {:?}", msi_path);

        // msiexec /i <file> /qn /norestart
        //   /i        = install
        //   /qn       = quiet, no UI
        //   /norestart = tidak restart otomatis
        let status = std::process::Command::new("msiexec.exe")
            .args([
                "/i",
                msi_path.to_str().ok_or("MSI path tidak valid (non-UTF8)")?,
                "/qn",
                "/norestart",
                "/l*v",
                self.temp_dir.join("msi_install.log").to_str().unwrap_or("nul"),
            ])
            .status();

        // Bersihkan file MSI setelah selesai
        let _ = fs::remove_file(&msi_path);

        match status {
            Ok(s) if s.success() => {
                info!("✅ MSI silent install berhasil");
                Ok(())
            }
            Ok(s) => {
                // Exit code 3010 = success tapi perlu restart (reboot required)
                // Kita treat ini sebagai success; app akan restart sendiri via restart_app()
                let code = s.code().unwrap_or(-1);
                if code == 3010 {
                    info!("✅ MSI install selesai (exit 3010 = reboot disarankan, diabaikan)");
                    Ok(())
                } else {
                    let log_hint = self.temp_dir.join("msi_install.log");
                    Err(format!(
                        "msiexec gagal dengan exit code {}. Lihat log: {:?}",
                        code, log_hint
                    ).into())
                }
            }
            Err(e) => Err(format!("Gagal menjalankan msiexec: {}", e).into()),
        }
    }

    /// Ganti binary secara in-place dari ZIP archive.
    ///
    /// Strategi:
    /// 1. Extract ZIP ke temp dir
    /// 2. Jika exe di direktori user-writable → rename + copy langsung
    /// 3. Jika di direktori sistem (C:\Program Files) → spawn PowerShell helper
    ///    yang meminta elevasi UAC sekali, kemudian replace dan restart
    async fn apply_zip_update<F>(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        let current_exe = std::env::current_exe()?;

        let extract_dir = self.temp_dir.join("extracted");
        let _ = fs::remove_dir_all(&extract_dir);
        fs::create_dir_all(&extract_dir)?;

        // Unzip archive
        let mut zip_archive = zip::ZipArchive::new(Cursor::new(content))?;
        zip_archive.extract(&extract_dir)?;

        // Cari file .exe di dalam archive (rekursif satu level)
        let new_binary = Self::find_exe_in_dir(&extract_dir)?;
        info!("📦 Binary ditemukan di archive: {:?}", new_binary);

        // Deteksi apakah install dir membutuhkan admin (Program Files, dll.)
        let needs_elevation = Self::path_needs_elevation(&current_exe);

        if needs_elevation {
            info!("🔒 Direktori install memerlukan elevasi UAC — menggunakan PowerShell helper");
            self.apply_via_powershell_helper(&current_exe, &new_binary, update_info, progress_cb)?;
        } else {
            info!("✅ Direktori install dapat ditulis langsung — melakukan in-place replace");
            self.replace_exe_in_place(&current_exe, &new_binary)?;
            let _ = fs::remove_dir_all(&extract_dir);
        }

        Ok(())
    }

    /// Cari file .exe pertama yang ditemukan di dalam direktori (tidak rekursif).
    fn find_exe_in_dir(dir: &std::path::Path) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        // Cari di root directory dulu
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                    if name.ends_with(".exe") {
                        return Ok(path);
                    }
                }
            }
        }

        // Cari satu level lebih dalam (zip mungkin punya subdirektori)
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Ok(sub_entries) = fs::read_dir(&path) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.is_file() {
                                let name = sub_path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                                if name.ends_with(".exe") {
                                    return Ok(sub_path);
                                }
                            }
                        }
                    }
                }
            }
        }

        Err("Tidak ada file .exe ditemukan di dalam ZIP archive".into())
    }

    /// Periksa apakah path memerlukan hak administrator untuk ditulis.
    /// Path di bawah Program Files, Windows, atau ProgramData dianggap perlu elevasi.
    fn path_needs_elevation(exe_path: &std::path::Path) -> bool {
        let path_str = exe_path.to_string_lossy().to_lowercase();

        // Cek apakah ada di direktori sistem Windows
        let system_prefixes = [
            "c:\\program files",
            "c:\\program files (x86)",
            "c:\\windows",
            "c:\\programdata",
        ];

        for prefix in &system_prefixes {
            if path_str.starts_with(prefix) {
                return true;
            }
        }

        // Coba tulis file dummy untuk memverifikasi izin secara langsung
        if let Some(parent) = exe_path.parent() {
            let test_file = parent.join(".tabular_write_test");
            if fs::write(&test_file, b"test").is_ok() {
                let _ = fs::remove_file(&test_file);
                return false; // Bisa ditulis — tidak perlu elevasi
            } else {
                return true; // Tidak bisa ditulis — perlu elevasi
            }
        }

        false
    }

    /// Replace exe langsung tanpa elevasi (untuk portable install).
    ///
    /// Windows memperbolehkan rename file yang sedang berjalan (tidak bisa overwrite),
    /// sehingga kita rename yang lama ke .old, copy yang baru, lalu cleanup .old saat restart.
    fn replace_exe_in_place(
        &self,
        current_exe: &std::path::Path,
        new_binary: &std::path::Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let old_exe = current_exe.with_extension("exe.old");

        // Hapus .old dari run sebelumnya jika ada
        let _ = fs::remove_file(&old_exe);

        info!("🔄 Rename {:?} → {:?}", current_exe, old_exe);
        fs::rename(current_exe, &old_exe)
            .map_err(|e| format!("Gagal rename exe lama: {}", e))?;

        info!("📋 Copy binary baru {:?} → {:?}", new_binary, current_exe);
        if let Err(e) = fs::copy(new_binary, current_exe) {
            // Rollback: kembalikan exe lama
            warn!("Copy gagal ({}), rolling back...", e);
            let _ = fs::rename(&old_exe, current_exe);
            return Err(format!("Gagal copy binary baru: {}", e).into());
        }

        info!("✅ Binary berhasil diganti in-place (Windows portable)");
        // .old akan dibersihkan saat startup berikutnya di AutoUpdater::new()
        Ok(())
    }

    /// Spawn PowerShell helper yang meminta elevasi UAC untuk mengganti binary
    /// di direktori sistem (C:\Program Files).
    ///
    /// Script PowerShell:
    /// 1. Tunggu proses Tabular lama selesai
    /// 2. Rename exe lama → .old
    /// 3. Copy binary baru → nama exe asli
    /// 4. Jalankan instance baru
    /// 5. Hapus .old dan script itu sendiri
    fn apply_via_powershell_helper<F>(
        &self,
        current_exe: &std::path::Path,
        new_binary: &std::path::Path,
        _update_info: &UpdateInfo,
        _progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        let current_pid = std::process::id();
        let old_exe = current_exe.with_extension("exe.old");
        let script_path = self.temp_dir.join("tabular_update_helper.ps1");

        // Escape path untuk PowerShell (ganti backslash ganda dan kutip)
        let cur_exe_str = current_exe.to_string_lossy().replace('\'', "''");
        let old_exe_str = old_exe.to_string_lossy().replace('\'', "''");
        let new_bin_str = new_binary.to_string_lossy().replace('\'', "''");
        let script_str = script_path.to_string_lossy().replace('\'', "''");

        let ps_script = format!(
            r#"# Tabular Windows Auto-Update Helper
# Dijalankan dengan elevasi UAC setelah proses utama keluar

$ErrorActionPreference = 'Stop'

# 1. Tunggu proses lama selesai (maks 30 detik)
$pid_old = {pid}
$waited = 0
while ($waited -lt 30) {{
    $proc = Get-Process -Id $pid_old -ErrorAction SilentlyContinue
    if ($null -eq $proc) {{ break }}
    Start-Sleep -Milliseconds 500
    $waited += 0.5
}}

# 2. Rename exe lama agar bisa ditulis
$oldPath = '{old_exe}'
$curPath = '{cur_exe}'
$newBin  = '{new_bin}'

if (Test-Path $oldPath) {{ Remove-Item -Force $oldPath }}
Rename-Item -Path $curPath -NewName $oldPath -Force

# 3. Copy binary baru ke lokasi asli
Copy-Item -Path $newBin -Destination $curPath -Force

# 4. Hapus .old
if (Test-Path $oldPath) {{ Remove-Item -Force $oldPath -ErrorAction SilentlyContinue }}

# 5. Jalankan versi baru
Start-Process -FilePath $curPath

# 6. Hapus script ini
Remove-Item -Path '{script}' -Force -ErrorAction SilentlyContinue
"#,
            pid = current_pid,
            cur_exe = cur_exe_str,
            old_exe = old_exe_str,
            new_bin = new_bin_str,
            script = script_str,
        );

        fs::write(&script_path, ps_script.as_bytes())
            .map_err(|e| format!("Gagal menulis PowerShell helper script: {}", e))?;

        info!("🚀 Spawning PowerShell helper dengan UAC elevation: {:?}", script_path);

        // Start-Process dengan -Verb RunAs meminta elevasi UAC
        // -WindowStyle Hidden agar tidak muncul jendela console
        let status = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy", "Bypass",
                "-Command",
                &format!(
                    "Start-Process powershell -Verb RunAs -WindowStyle Hidden -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File \"{}\"'",
                    script_path.to_string_lossy()
                ),
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                info!("✅ PowerShell helper berhasil di-spawn, menunggu proses selesai...");
                Ok(())
            }
            Ok(s) => Err(format!(
                "PowerShell helper gagal di-spawn (exit code: {:?}). \
                 Coba update manual dari halaman release GitHub.",
                s.code()
            ).into()),
            Err(e) => Err(format!("Gagal menjalankan PowerShell: {}", e).into()),
        }
    }
}
