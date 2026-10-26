use crate::self_update::UpdateInfo;
use futures_util::StreamExt;
use log::info;
// Every `warn!` here lives in the macOS-only DMG staging path, so importing it
// unconditionally makes the iOS build fail `clippy -D warnings`.
#[cfg(target_os = "macos")]
use log::{debug, warn};
use std::fs;
#[allow(unused_imports)]
use std::io::Cursor;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Path executable terpasang, dicatat sekali sebelum update mengganti file-nya.
static INSTALL_EXE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Path executable Tabular yang terpasang.
///
/// Di Linux `current_exe()` membaca `/proc/self/exe`; setelah binary diganti
/// hasilnya menjadi `.../tabular (deleted)` (atau path `.old`), sehingga restart
/// gagal. Karena itu path dicatat saat updater dibuat (startup) dan di-cache.
pub fn install_exe_path() -> Option<PathBuf> {
    INSTALL_EXE
        .get_or_init(|| std::env::current_exe().ok().map(strip_deleted_suffix))
        .clone()
}

/// Buang akhiran ` (deleted)` yang ditambahkan kernel Linux pada `/proc/self/exe`.
fn strip_deleted_suffix(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
        Some(stripped) => PathBuf::from(stripped),
        None => path,
    }
}

/// Kutip path untuk string literal PowerShell bertanda kutip tunggal.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn ps_quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

/// Script helper Windows untuk update MSI. Dijalankan TANPA elevasi setelah
/// Tabular keluar: menunggu proses lama selesai, menjalankan `msiexec /passive`
/// (msiexec sendiri yang memunculkan prompt UAC untuk paket per-machine), lalu
/// membuka ulang Tabular kecuali dipanggil dengan `-NoRelaunch`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_msi_helper_script(
    pid: u32,
    msi: &Path,
    log: &Path,
    current_exe: &Path,
    is_msi_install: bool,
) -> String {
    r#"# Tabular Windows auto-update helper (MSI)
param([switch]$NoRelaunch)
$ErrorActionPreference = 'Continue'
Wait-Process -Id __PID__ -Timeout 60 -ErrorAction SilentlyContinue
$msi = '__MSI__'
$log = '__LOG__'
$target = '__EXE__'
if (-not $__IS_MSI__) { $target = Join-Path $env:ProgramFiles 'Tabular\Tabular.exe' }
$argList = '/i "' + $msi + '" /passive /norestart /l*v "' + $log + '"'
Start-Process -FilePath 'msiexec.exe' -ArgumentList $argList -Wait
Remove-Item -LiteralPath $msi -Force -ErrorAction SilentlyContinue
if (-not $NoRelaunch -and (Test-Path -LiteralPath $target)) { Start-Process -FilePath $target }
Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue
"#
    .replace("__PID__", &pid.to_string())
    .replace("__MSI__", &ps_quote(msi))
    .replace("__LOG__", &ps_quote(log))
    .replace("__EXE__", &ps_quote(current_exe))
    .replace("__IS_MSI__", if is_msi_install { "true" } else { "false" })
}

/// Script helper Windows untuk update ZIP di folder yang butuh admin. Setelah
/// Tabular keluar, salin binary baru; bila ditolak, ulangi lewat PowerShell
/// ber-elevasi (`-Verb RunAs`, satu prompt UAC), lalu buka ulang Tabular.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_copy_helper_script(pid: u32, new_binary: &Path, current_exe: &Path) -> String {
    r#"# Tabular Windows auto-update helper (ZIP)
param([switch]$NoRelaunch)
$ErrorActionPreference = 'Continue'
Wait-Process -Id __PID__ -Timeout 60 -ErrorAction SilentlyContinue
$new = '__NEW__'
$cur = '__CUR__'
try {
    Copy-Item -LiteralPath $new -Destination $cur -Force -ErrorAction Stop
} catch {
    $cmd = "Copy-Item -LiteralPath '" + $new.Replace("'", "''") + "' -Destination '" + $cur.Replace("'", "''") + "' -Force"
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($cmd))
    Start-Process -FilePath 'powershell.exe' -Verb RunAs -WindowStyle Hidden -Wait -ArgumentList ('-NoProfile -ExecutionPolicy Bypass -EncodedCommand ' + $encoded)
}
if (-not $NoRelaunch) { Start-Process -FilePath $cur }
Remove-Item -LiteralPath $MyInvocation.MyCommand.Path -Force -ErrorAction SilentlyContinue
"#
    .replace("__PID__", &pid.to_string())
    .replace("__NEW__", &ps_quote(new_binary))
    .replace("__CUR__", &ps_quote(current_exe))
}

/// Jalankan script helper PowerShell tanpa jendela console.
#[cfg(target_os = "windows")]
fn spawn_windows_helper(script: &Path, no_relaunch: bool) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
    ])
    .arg(script);
    if no_relaunch {
        cmd.arg("-NoRelaunch");
    }
    cmd.creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ())
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UpdateStage {
    Idle,
    Downloading {
        progress: f32,
        downloaded: u64,
        total: Option<u64>,
    },
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
        // Catat path executable sekarang, sebelum update sempat menggantinya.
        let _ = install_exe_path();

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

        crate::privacy::check(crate::privacy::NetCategory::UpdateDownload, download_url)?;
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

        info!(
            "📦 Update payload downloaded successfully ({} bytes)",
            content.len()
        );
        progress_cb(UpdateStage::Extracting);

        // staged_script carries back the macOS helper script path (None on other platforms)
        let staged_script: Option<PathBuf>;

        #[cfg(target_os = "macos")]
        {
            let staged = self
                .stage_macos_update(&content, update_info, &progress_cb)
                .await?;
            staged_script = staged;
        }

        #[cfg(target_os = "linux")]
        {
            self.stage_linux_update(&content, update_info, &progress_cb)
                .await?;
            staged_script = None;
        }

        #[cfg(target_os = "windows")]
        {
            staged_script = self
                .stage_windows_update(&content, update_info, &progress_cb)
                .await?;
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
        let current_exe =
            install_exe_path().ok_or("Could not determine the Tabular executable path")?;
        info!("🔄 Restarting Tabular executable: {:?}", current_exe);

        // Windows: helper menunggu proses ini keluar, memasang update, lalu
        // membuka ulang Tabular. Jangan spawn exe lama di sini — instance baru
        // akan mengunci file yang hendak diganti.
        #[cfg(target_os = "windows")]
        if let Some(script_path) = staged_script {
            info!("🪟 Launching staged update helper: {:?}", script_path);
            spawn_windows_helper(script_path, false)?;
            std::process::exit(0);
        }

        #[cfg(target_os = "macos")]
        {
            // If a staged update script is available, run it and exit.
            // The script waits for us to quit, then replaces the .app and relaunches.
            if let Some(script_path) = staged_script {
                info!(
                    "🍏 Launching staged update helper script: {:?}",
                    script_path
                );
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

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = staged_script; // unused on other platforms

        std::process::Command::new(&current_exe).spawn()?;
        std::process::exit(0);
    }
}

impl AutoUpdater {
    /// Pasang update yang sudah di-stage saat aplikasi ditutup (M8), tanpa
    /// membuka ulang. Linux (dan ZIP portable Windows) sudah mengganti binary
    /// saat staging; macOS dan MSI/ZIP-admin Windows menjalankan helper di sini.
    pub fn install_on_quit(staged_script: Option<&PathBuf>) {
        #[cfg(target_os = "windows")]
        if let Some(script) = staged_script {
            info!("🪟 Installing staged update on quit: {:?}", script);
            if let Err(e) = spawn_windows_helper(script, true) {
                log::warn!("Failed to launch update helper on quit: {}", e);
            }
        }
        #[cfg(target_os = "macos")]
        if let Some(script) = staged_script {
            info!("🍏 Installing staged update on quit: {:?}", script);
            if let Err(e) = std::process::Command::new("bash")
                .arg(script)
                .arg("--no-relaunch")
                .spawn()
            {
                warn!("Failed to launch update helper on quit: {}", e);
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = staged_script;
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

        let asset_name = update_info.asset_name.as_deref().unwrap_or("Tabular.dmg");

        let dmg_path = self.temp_dir.join(asset_name);
        fs::write(&dmg_path, content)?;

        let current_exe = std::env::current_exe()?;
        let exe_str = current_exe.to_string_lossy();

        // Determine the current .app bundle path (if running from inside a .app)
        let current_app_path: Option<PathBuf> = exe_str
            .find(".app/Contents/MacOS/")
            .map(|app_pos| PathBuf::from(&exe_str[..app_pos + 4]));

        // Mount DMG via hdiutil
        let mount_point = self.temp_dir.join("mount");
        let _ = fs::remove_dir_all(&mount_point);
        fs::create_dir_all(&mount_point)?;

        debug!("Mounting DMG {:?} to {:?}", dmg_path, mount_point);
        let mount_output = std::process::Command::new("hdiutil")
            // Path diteruskan sebagai `OsStr`: tidak panic bila bukan UTF-8.
            .args(["attach", "-nobrowse", "-mountpoint"])
            .arg(&mount_point)
            .arg(&dmg_path)
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
                    .arg("-R")
                    .arg(&mounted_app)
                    .arg(&staged_app)
                    .status();

                let _ = std::process::Command::new("hdiutil")
                    .arg("detach")
                    .arg(&mount_point)
                    .output();
                let _ = fs::remove_file(&dmg_path);

                if cp_status.is_ok_and(|s| s.success()) && staged_app.exists() {
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
                        # Relaunch the app (dilewati untuk install-saat-keluar)\n\
                        if [ \"$1\" != \"--no-relaunch\" ]; then open \"{install}\"; fi\n",
                        install = install_target,
                        staged = staged_app.to_string_lossy(),
                        script = helper_script_path.to_string_lossy(),
                    );

                    fs::write(&helper_script_path, script_content)?;

                    // Make the helper script executable
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(
                            &helper_script_path,
                            fs::Permissions::from_mode(0o755),
                        )?;
                    }

                    info!(
                        "✅ Update staged. Helper script ready at {:?}",
                        helper_script_path
                    );
                    return Ok(Some(helper_script_path));
                } else {
                    warn!(
                        "❌ Failed to copy new .app to staging dir; falling back to Downloads DMG"
                    );
                }
            } else {
                let _ = std::process::Command::new("hdiutil")
                    .arg("detach")
                    .arg(&mount_point)
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
                let _ = std::process::Command::new("open")
                    .arg(&download_dmg)
                    .spawn();
            }
            let _ = fs::remove_file(&dmg_fallback_path);
        }

        Ok(None)
    }
}

// ─── Platform Specific: Linux ────────────────────────────────────────────────
#[cfg(target_os = "linux")]
impl AutoUpdater {
    /// Ganti binary Linux. Binary yang sedang berjalan boleh diganti dengan
    /// `rename` (inode lama tetap dipakai proses ini sampai keluar).
    ///
    /// Instalasi lewat .deb/AUR ada di `/usr/bin` (milik root): penggantian
    /// diulang lewat `pkexec` (prompt polkit). Bila tetap gagal, error
    /// dikembalikan — tidak lagi dilaporkan sukses padahal binary tidak berubah.
    async fn stage_linux_update<F>(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        info!("🐧 Staging Linux update...");
        progress_cb(UpdateStage::Applying);

        let current_exe =
            install_exe_path().ok_or("Could not determine the Tabular executable path")?;
        let extract_dir = self.temp_dir.join("extracted");
        let _ = fs::remove_dir_all(&extract_dir);
        fs::create_dir_all(&extract_dir)?;

        let tar_gz = flate2::read::GzDecoder::new(Cursor::new(content));
        let mut archive = tar::Archive::new(tar_gz);
        archive.unpack(&extract_dir)?;

        let new_binary = Self::find_linux_binary(&extract_dir)
            .ok_or("Could not find the Tabular binary in the update archive")?;

        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755))?;
        }

        info!("Replacing Linux binary at {:?}", current_exe);
        let result = match Self::replace_linux_binary(&new_binary, &current_exe) {
            Ok(()) => Ok(()),
            Err(e) => {
                log::warn!(
                    "[UPDATE] Direct replace failed ({}); retrying with pkexec",
                    e
                );
                Self::replace_linux_binary_elevated(&new_binary, &current_exe).map_err(|reason| {
                    Self::linux_manual_fallback(&new_binary, &current_exe, update_info, &reason)
                })
            }
        };

        let _ = fs::remove_dir_all(&extract_dir);
        result.map_err(Into::into)
    }

    /// Cari binary `tabular*` di root archive (atau satu level di bawahnya).
    fn find_linux_binary(dir: &Path) -> Option<PathBuf> {
        let is_binary = |p: &Path| {
            p.is_file()
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("tabular"))
                    .unwrap_or(false)
        };
        let entries: Vec<PathBuf> = fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        if let Some(p) = entries.iter().find(|p| is_binary(p)) {
            return Some(p.clone());
        }
        entries.iter().filter(|p| p.is_dir()).find_map(|sub| {
            fs::read_dir(sub)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| is_binary(p))
        })
    }

    /// Salin binary baru ke folder tujuan lalu `rename` atomik menimpa yang lama.
    /// Folder sama => tidak ada masalah lintas filesystem (/tmp sering tmpfs).
    fn replace_linux_binary(new_binary: &Path, target: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let dir = target
            .parent()
            .ok_or_else(|| std::io::Error::other("executable has no parent directory"))?;
        let staged = dir.join(".tabular-update.new");
        fs::copy(new_binary, &staged)?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
        if let Err(e) = fs::rename(&staged, target) {
            let _ = fs::remove_file(&staged);
            return Err(e);
        }
        Ok(())
    }

    /// Ulangi penggantian sebagai root lewat `pkexec` (dialog polkit).
    fn replace_linux_binary_elevated(new_binary: &Path, target: &Path) -> Result<(), String> {
        let script = r#"set -e; tmp="$(dirname "$2")/.tabular-update.new"; install -m 0755 "$1" "$tmp"; mv -f "$tmp" "$2""#;
        let status = std::process::Command::new("pkexec")
            .args(["/bin/sh", "-c", script, "sh"])
            .arg(new_binary)
            .arg(target)
            .status();
        match status {
            Ok(s) if s.success() => {
                info!("✅ Linux binary replaced via pkexec");
                Ok(())
            }
            Ok(s) if matches!(s.code(), Some(126) | Some(127)) => {
                Err("administrator authorization was cancelled or denied".to_string())
            }
            Ok(s) => Err(format!("pkexec exited with status {:?}", s.code())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err("pkexec is not installed".to_string())
            }
            Err(e) => Err(format!("failed to run pkexec: {}", e)),
        }
    }

    /// Simpan binary baru di Downloads dan susun pesan error yang jelas.
    fn linux_manual_fallback(
        new_binary: &Path,
        target: &Path,
        update_info: &UpdateInfo,
        reason: &str,
    ) -> String {
        let saved = dirs::download_dir().and_then(|dir| {
            let dest = dir.join(format!("tabular-{}", update_info.latest_version));
            fs::copy(new_binary, &dest).ok().map(|_| dest)
        });
        let mut msg = format!(
            "Could not replace {} ({}). If Tabular was installed with a package manager, update it there (e.g. apt or your AUR helper).",
            target.display(),
            reason
        );
        if let Some(dest) = saved {
            msg.push_str(&format!(
                " The new version was saved to {}.",
                dest.display()
            ));
        }
        msg
    }
}

// ─── Platform Specific: Windows ──────────────────────────────────────────────
// MSI (instalasi Program Files) atau ZIP (portable). Untuk MSI dan ZIP di
// folder admin, update dipasang oleh helper PowerShell SETELAH Tabular keluar
// (dipicu "Restart Now" atau install-saat-keluar), karena file yang sedang
// berjalan tidak bisa ditimpa dan `msiexec /qn` tanpa elevasi pasti gagal.
#[cfg(target_os = "windows")]
impl AutoUpdater {
    /// Mengembalikan path script helper bila update baru terpasang saat restart.
    async fn stage_windows_update<F>(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        progress_cb: &F,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: Fn(UpdateStage) + Send + Sync + 'static,
    {
        use crate::self_update::WindowsUpdateKind;

        info!("🪟 Staging Windows update...");
        progress_cb(UpdateStage::Applying);

        let current_exe =
            install_exe_path().ok_or("Could not determine the Tabular executable path")?;

        match update_info.windows_update_kind {
            Some(WindowsUpdateKind::Msi) => self
                .stage_msi_update(content, update_info, &current_exe)
                .map(Some),
            _ => self.apply_zip_update(content, update_info, &current_exe),
        }
    }

    /// Simpan MSI dan tulis helper yang menjalankan `msiexec` setelah keluar.
    fn stage_msi_update(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        current_exe: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        let asset_name = update_info
            .asset_name
            .as_deref()
            .unwrap_or("Tabular-update.msi");
        let msi_path = self.temp_dir.join(asset_name);
        info!("💾 Writing MSI to {:?}", msi_path);
        fs::write(&msi_path, content)?;

        let script_path = self.temp_dir.join("tabular_update_helper.ps1");
        let script = windows_msi_helper_script(
            std::process::id(),
            &msi_path,
            &self.temp_dir.join("msi_install.log"),
            current_exe,
            crate::self_update::is_windows_system_install(&current_exe.to_string_lossy()),
        );
        fs::write(&script_path, script)
            .map_err(|e| format!("Failed to write the update helper script: {}", e))?;
        info!("✅ MSI update staged; helper at {:?}", script_path);
        Ok(script_path)
    }

    /// Update dari ZIP (atau EXE tunggal).
    ///
    /// - Folder bisa ditulis (portable): exe lama di-rename ke `.exe.old`, exe
    ///   baru disalin; selesai saat staging, restart cukup membuka exe.
    /// - Folder butuh admin: tulis helper yang menyalin setelah Tabular keluar.
    fn apply_zip_update(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
        current_exe: &Path,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
        use crate::self_update::WindowsUpdateKind;

        let extract_dir = self.temp_dir.join("extracted");
        let _ = fs::remove_dir_all(&extract_dir);
        fs::create_dir_all(&extract_dir)?;

        let new_binary = if update_info.windows_update_kind == Some(WindowsUpdateKind::Exe) {
            let path = extract_dir.join("tabular.exe");
            fs::write(&path, content)?;
            path
        } else {
            let mut zip_archive = zip::ZipArchive::new(Cursor::new(content))?;
            zip_archive.extract(&extract_dir)?;
            Self::find_exe_in_dir(&extract_dir)?
        };
        info!("📦 Binary ditemukan di archive: {:?}", new_binary);

        if Self::path_needs_elevation(current_exe) {
            info!("🔒 Direktori install memerlukan elevasi — helper dipasang saat restart");
            let script_path = self.temp_dir.join("tabular_update_helper.ps1");
            let script = windows_copy_helper_script(std::process::id(), &new_binary, current_exe);
            fs::write(&script_path, script)
                .map_err(|e| format!("Failed to write the update helper script: {}", e))?;
            Ok(Some(script_path))
        } else {
            info!("✅ Direktori install dapat ditulis langsung — melakukan in-place replace");
            Self::replace_exe_in_place(current_exe, &new_binary)?;
            let _ = fs::remove_dir_all(&extract_dir);
            Ok(None)
        }
    }

    /// Cari file .exe di root direktori, lalu satu level di bawahnya.
    fn find_exe_in_dir(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
        let is_exe = |p: &Path| {
            p.is_file()
                && p.extension()
                    .map(|e| e.eq_ignore_ascii_case("exe"))
                    .unwrap_or(false)
        };
        let entries: Vec<PathBuf> = fs::read_dir(dir)?.flatten().map(|e| e.path()).collect();
        if let Some(p) = entries.iter().find(|p| is_exe(p)) {
            return Ok(p.clone());
        }
        entries
            .iter()
            .filter(|p| p.is_dir())
            .find_map(|sub| {
                fs::read_dir(sub)
                    .ok()?
                    .flatten()
                    .map(|e| e.path())
                    .find(|p| is_exe(p))
            })
            .ok_or_else(|| "No .exe file found in the ZIP archive".into())
    }

    /// Apakah folder exe butuh hak administrator untuk ditulis?
    fn path_needs_elevation(exe_path: &Path) -> bool {
        if crate::self_update::is_windows_system_install(&exe_path.to_string_lossy()) {
            return true;
        }
        // Verifikasi langsung dengan menulis file uji.
        match exe_path.parent() {
            Some(parent) => {
                let test_file = parent.join(".tabular_write_test");
                if fs::write(&test_file, b"test").is_ok() {
                    let _ = fs::remove_file(&test_file);
                    false
                } else {
                    true
                }
            }
            None => false,
        }
    }

    /// Ganti exe portable: Windows boleh me-rename exe yang sedang berjalan
    /// (tapi tidak menimpanya). `.exe.old` dibersihkan di `AutoUpdater::new()`.
    fn replace_exe_in_place(
        current_exe: &Path,
        new_binary: &Path,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let old_exe = current_exe.with_extension("exe.old");
        let _ = fs::remove_file(&old_exe);

        info!("🔄 Rename {:?} → {:?}", current_exe, old_exe);
        fs::rename(current_exe, &old_exe)
            .map_err(|e| format!("Failed to rename the old executable: {}", e))?;

        info!("📋 Copy binary baru {:?} → {:?}", new_binary, current_exe);
        if let Err(e) = fs::copy(new_binary, current_exe) {
            log::warn!("Copy gagal ({}), rolling back...", e);
            let _ = fs::rename(&old_exe, current_exe);
            return Err(format!("Failed to copy the new binary: {}", e).into());
        }

        info!("✅ Binary berhasil diganti in-place (Windows portable)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_linux_deleted_suffix() {
        assert_eq!(
            strip_deleted_suffix(PathBuf::from("/usr/bin/tabular (deleted)")),
            PathBuf::from("/usr/bin/tabular")
        );
        assert_eq!(
            strip_deleted_suffix(PathBuf::from("/opt/tabular")),
            PathBuf::from("/opt/tabular")
        );
    }

    #[test]
    fn msi_helper_quotes_paths_and_waits_for_pid() {
        let script = windows_msi_helper_script(
            4242,
            Path::new(r"C:\Users\O'Neil\AppData\Local\Temp\tabular_update\Tabular-1.9.0.msi"),
            Path::new(r"C:\Temp\msi.log"),
            Path::new(r"C:\Program Files\Tabular\Tabular.exe"),
            true,
        );
        assert!(script.contains("Wait-Process -Id 4242"));
        assert!(script.contains(r"$msi = 'C:\Users\O''Neil\AppData"));
        assert!(script.contains(r"$target = 'C:\Program Files\Tabular\Tabular.exe'"));
        assert!(script.contains("if (-not $true)"));
        assert!(script.contains("/passive"));
        assert!(!script.contains("/qn"));
        assert!(!script.contains("__"));
    }

    #[test]
    fn copy_helper_fills_all_placeholders() {
        let script = windows_copy_helper_script(
            7,
            Path::new(r"C:\Temp\extracted\tabular.exe"),
            Path::new(r"C:\Program Files\Tabular\Tabular.exe"),
        );
        assert!(script.contains("Wait-Process -Id 7"));
        assert!(script.contains("-Verb RunAs"));
        assert!(!script.contains("__"));
    }
}
