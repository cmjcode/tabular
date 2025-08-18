use std::path::{Path, PathBuf};
use std::process::Command;
use log::info;
use crate::self_update::UpdateInfo;

#[derive(Clone)]
pub struct AutoUpdater {
    current_exe_path: PathBuf,
    temp_dir: PathBuf,
}

impl AutoUpdater {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let current_exe_path = std::env::current_exe()?;
        let temp_dir = std::env::temp_dir().join("tabular_update");
        
        // Create temp directory if it doesn't exist
        std::fs::create_dir_all(&temp_dir)?;
        
        Ok(AutoUpdater {
            current_exe_path,
            temp_dir,
        })
    }
    
    /// Download and prepare update, then schedule replacement on next restart
    pub async fn download_and_stage_update(&self, update_info: &UpdateInfo) -> Result<(), Box<dyn std::error::Error>> {
        let download_url = update_info.download_url.as_ref()
            .ok_or("No download URL available")?;
        
        info!("🚀 Starting staged update process...");
        info!("📥 Downloading from: {}", download_url);
        
        // Download the update
        let client = reqwest::Client::new();
        let response = client
            .get(download_url)
            .header("User-Agent", format!("Tabular/{}", env!("CARGO_PKG_VERSION")))
            .send()
            .await?;
        
        if !response.status().is_success() {
            return Err(format!("Download failed: {}", response.status()).into());
        }
        
        let content = response.bytes().await?;
        info!("📦 Downloaded {} bytes", content.len());
        
        #[cfg(target_os = "macos")]
        {
            self.stage_macos_update(&content, update_info).await?;
        }
        
        #[cfg(target_os = "linux")]
        {
            self.stage_linux_update(&content, update_info).await?;
        }
        
        #[cfg(target_os = "windows")]
        {
            self.stage_windows_update(&content, update_info).await?;
        }
        
        Ok(())
    }
    
    #[cfg(target_os = "macos")]
    async fn stage_macos_update(&self, content: &[u8], update_info: &UpdateInfo) -> Result<(), Box<dyn std::error::Error>> {
        let asset_name = update_info.asset_name.as_ref()
            .ok_or("No asset name available")?;
        
        if asset_name.ends_with(".dmg") {
            // Handle DMG files
            self.handle_dmg_update(content, asset_name).await
        } else if asset_name.ends_with(".tar.gz") {
            // Handle tar.gz app bundles (if we provide them)
            self.handle_dmg_update(content, asset_name).await
        } else {
            // Try to extract binary from DMG or handle as direct binary
            self.handle_dmg_update(content, asset_name).await
        }
    }
    
    #[cfg(target_os = "macos")]
    async fn handle_dmg_update(&self, content: &[u8], asset_name: &str) -> Result<(), Box<dyn std::error::Error>> {
        let dmg_path = self.temp_dir.join(asset_name);
        std::fs::write(&dmg_path, content)?;
        
        info!("📀 DMG saved to: {}", dmg_path.display());
        
        // For now, just save to Downloads and open it like before
        // This is more reliable than trying to programmatically mount and extract
        let downloads_dir = dirs::download_dir()
            .ok_or("Could not find Downloads directory")?;
        
        let download_dmg_path = downloads_dir.join(asset_name);
        std::fs::copy(&dmg_path, &download_dmg_path)?;
        
        info!("✅ Update downloaded to: {}", download_dmg_path.display());
        info!("🚀 Opening DMG file for installation...");
        
        // Open the DMG file directly so user can install it
        let _ = std::process::Command::new("open")
            .arg(&download_dmg_path)
            .spawn();
        
        // Clean up temp file
        let _ = std::fs::remove_file(&dmg_path);
        
        Ok(())
    }
    
    #[cfg(target_os = "macos")]
    fn parse_mount_point_from_plist(&self, plist_output: &str) -> Result<String, Box<dyn std::error::Error>> {
        // Try to find mount point in plist output
        // Look for mount-point key in the plist
        for line in plist_output.lines() {
            if line.contains("<string>/Volumes/") {
                if let Some(start) = line.find("/Volumes/") {
                    if let Some(end) = line[start..].find("</string>") {
                        let mount_point = &line[start..start + end];
                        return Ok(mount_point.to_string());
                    }
                }
            }
        }
        
        // Fallback: try without plist flag
        info!("🔄 Plist parsing failed, trying fallback method...");
        self.mount_dmg_fallback(&std::env::temp_dir().join("tabular_update").join("Tabular-0.3.2.dmg"))
    }
    
    #[cfg(target_os = "macos")]
    fn mount_dmg_fallback(&self, dmg_path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
        // Unmount any existing mount first
        let _ = Command::new("hdiutil")
            .args(["detach", "/Volumes/Tabular"])
            .output();
        
        // Mount without plist
        let mount_output = Command::new("hdiutil")
            .args(["attach", "-nobrowse", "-quiet"])
            .arg(dmg_path)
            .output()?;
        
        if !mount_output.status.success() {
            let stderr = String::from_utf8_lossy(&mount_output.stderr);
            return Err(format!("Fallback mount failed: {}", stderr).into());
        }
        
        let mount_info = String::from_utf8_lossy(&mount_output.stdout);
        info!("📋 Fallback mount output: {}", mount_info);
        
        // Parse the traditional output format
        for line in mount_info.lines() {
            if line.contains("/Volumes/") {
                // Extract the mount point (last column)
                if let Some(mount_point) = line.split_whitespace().last() {
                    if mount_point.starts_with("/Volumes/") {
                        return Ok(mount_point.to_string());
                    }
                }
            }
        }
        
        // Final fallback: check if standard location exists
        let standard_mount = "/Volumes/Tabular";
        if std::path::Path::new(standard_mount).exists() {
            return Ok(standard_mount.to_string());
        }
        
        Err("Could not find mount point even with fallback".into())
    }
    
    #[cfg(target_os = "macos")]
    fn find_app_in_mount(&self, mount_point: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let mount_path = Path::new(mount_point);
        
        // Look for .app bundle
        for entry in std::fs::read_dir(mount_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("app") {
                return Ok(path);
            }
        }
        
        Err("No .app bundle found in DMG".into())
    }
    
    #[cfg(target_os = "macos")]
    fn create_macos_update_script(&self, staged_app: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let current_app_bundle = self.find_current_app_bundle()?;
        let script_path = self.temp_dir.join("update_tabular.sh");
        
        let script_content = format!(r#"#!/bin/bash
# Tabular Auto-Update Script
set -e

echo "🔄 Starting Tabular update..."

# Wait a moment for the current app to fully quit
sleep 2

# Backup current version
BACKUP_DIR="/tmp/tabular_backup_$(date +%s)"
echo "📦 Backing up current version to $BACKUP_DIR"
cp -R "{}" "$BACKUP_DIR"

# Remove old version
echo "🗑️ Removing old version..."
rm -rf "{}"

# Install new version
echo "📥 Installing new version..."
cp -R "{}" "{}"

# Set proper permissions
echo "🔒 Setting permissions..."
chmod -R 755 "{}"

# Clean up
echo "🧹 Cleaning up..."
rm -rf "{}"
rm "$0"

echo "✅ Update completed successfully!"

# Try to launch the new version
echo "🚀 Launching updated Tabular..."
open "{}"
"#, 
            current_app_bundle.display(),
            current_app_bundle.display(),
            staged_app.display(),
            current_app_bundle.display(),
            current_app_bundle.display(),
            self.temp_dir.display(),
            current_app_bundle.display()
        );
        
        std::fs::write(&script_path, script_content)?;
        
        // Make script executable
        Command::new("chmod")
            .args(["+x"])
            .arg(&script_path)
            .output()?;
        
        // Create a launch agent to run the script after quit
        self.create_launch_agent(&script_path)?;
        
        Ok(())
    }
    
    #[cfg(target_os = "macos")]
    fn find_current_app_bundle(&self) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let exe_path = &self.current_exe_path;
        
        // Navigate up from executable to find .app bundle
        let mut current = exe_path.as_path();
        while let Some(parent) = current.parent() {
            if parent.extension().and_then(|s| s.to_str()) == Some("app") {
                return Ok(parent.to_path_buf());
            }
            current = parent;
        }
        
        Err("Could not find current app bundle".into())
    }
    
    #[cfg(target_os = "macos")]
    fn create_launch_agent(&self, script_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let home_dir = dirs::home_dir().ok_or("Could not find home directory")?;
        let launch_agents_dir = home_dir.join("Library/LaunchAgents");
        std::fs::create_dir_all(&launch_agents_dir)?;
        
        let plist_path = launch_agents_dir.join("com.tabular.updater.plist");
        
        let plist_content = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.tabular.updater</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LaunchOnlyOnce</key>
    <true/>
</dict>
</plist>"#, script_path.display());
        
        std::fs::write(&plist_path, plist_content)?;
        
        // Load the launch agent
        Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&plist_path)
            .output()?;
        
        info!("📅 Scheduled update script to run on next login");
        Ok(())
    }
    
    fn copy_dir_all(&self, src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            if ty.is_dir() {
                self.copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
            } else {
                std::fs::copy(&entry.path(), &dst.join(entry.file_name()))?;
            }
        }
        Ok(())
    }
    
    /// Get update status (simplified version)
    pub fn get_update_status(&self) -> (bool, bool, Option<String>) {
        // For simplified version, we don't track complex status
        // Return (downloading: false, installed: false, error: None)
        (false, false, None)
    }

    /// Clear error state (no-op in simplified version)
    pub fn clear_error(&self) {
        // No-op since we don't track status anymore
    }
}

#[cfg(target_os = "linux")]
impl AutoUpdater {
    async fn stage_linux_update(&self, content: &[u8], update_info: &UpdateInfo) -> Result<(), Box<dyn std::error::Error>> {
        info!("🐧 Processing Linux update...");
        
        // Save to downloads folder
        let downloads_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        
        // Determine filename based on asset name
        let filename = if let Some(asset_name) = &update_info.asset_name {
            if asset_name.contains("appimage") || asset_name.contains("AppImage") {
                format!("Tabular-{}.AppImage", update_info.version)
            } else if asset_name.ends_with(".tar.gz") {
                format!("tabular-{}.tar.gz", update_info.version)
            } else {
                format!("tabular-{}", update_info.version)
            }
        } else {
            format!("tabular-{}", update_info.version)
        };
        
        let file_path = downloads_dir.join(&filename);
        
        info!("💾 Saving Linux binary to: {}", file_path.display());
        std::fs::write(&file_path, content)?;
        
        // Make executable if it's a binary
        if !filename.ends_with(".tar.gz") {
            Command::new("chmod")
                .args(["+x"])
                .arg(&file_path)
                .output()?;
        }
        
        // Open file manager to show the downloaded file
        info!("📂 Opening file manager...");
        let _result = Command::new("xdg-open")
            .arg(&downloads_dir)
            .spawn()
            .or_else(|_| {
                // Fallback for GNOME
                Command::new("nautilus")
                    .arg(&downloads_dir)
                    .spawn()
            })
            .or_else(|_| {
                // Fallback for KDE
                Command::new("dolphin")
                    .arg(&downloads_dir)
                    .spawn()
            })
            .or_else(|_| {
                // Fallback for generic file manager
                Command::new("thunar")
                    .arg(&downloads_dir)
                    .spawn()
            });
        
        info!("✅ Linux update downloaded! Check Downloads folder.");
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl AutoUpdater {
    async fn stage_windows_update(&self, content: &[u8], update_info: &UpdateInfo) -> Result<(), Box<dyn std::error::Error>> {
        info!("🪟 Processing Windows update...");
        
        // Save to downloads folder
        let downloads_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("C:\\"));
        
        // Determine filename based on asset name
        let filename = if let Some(asset_name) = &update_info.asset_name {
            if asset_name.ends_with(".msi") {
                format!("Tabular-{}.msi", update_info.version)
            } else if asset_name.ends_with(".exe") {
                format!("Tabular-{}.exe", update_info.version)
            } else {
                format!("Tabular-{}.exe", update_info.version)
            }
        } else {
            format!("Tabular-{}.exe", update_info.version)
        };
        
        let file_path = downloads_dir.join(&filename);
        
        info!("💾 Saving Windows installer to: {}", file_path.display());
        std::fs::write(&file_path, content)?;
        
        // Open the installer automatically
        info!("🚀 Opening installer...");
        Command::new("cmd")
            .args(["/C", "start", "", &file_path.to_string_lossy()])
            .spawn()?;
        
        info!("✅ Windows installer opened!");
        Ok(())
    }
}
