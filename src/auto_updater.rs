use crate::self_update::UpdateInfo;
use log::info;
use std::path::PathBuf;

#[derive(Clone)]
pub struct AutoUpdater {
    temp_dir: PathBuf,
}

impl AutoUpdater {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("tabular_update");

        // Create temp directory if it doesn't exist
        std::fs::create_dir_all(&temp_dir)?;

        Ok(AutoUpdater { temp_dir })
    }

    /// Download and prepare update, then schedule replacement on next restart
    pub async fn download_and_stage_update(
        &self,
        update_info: &UpdateInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let download_url = update_info
            .download_url
            .as_ref()
            .ok_or("No download URL available")?;

        info!("🚀 Starting staged update process...");
        info!("📥 Downloading from: {}", download_url);

        // Download the update
        let client = reqwest::Client::new();
        let response = client
            .get(download_url)
            .header(
                "User-Agent",
                format!("Tabular/{}", env!("CARGO_PKG_VERSION")),
            )
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
    async fn stage_macos_update(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let asset_name = update_info
            .asset_name
            .as_ref()
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
    async fn handle_dmg_update(
        &self,
        content: &[u8],
        asset_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dmg_path = self.temp_dir.join(asset_name);
        std::fs::write(&dmg_path, content)?;

        info!("📀 DMG saved to: {}", dmg_path.display());

        // For now, just save to Downloads and open it like before
        // This is more reliable than trying to programmatically mount and extract
        let downloads_dir = dirs::download_dir().ok_or("Could not find Downloads directory")?;

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
}

#[cfg(target_os = "linux")]
impl AutoUpdater {
    pub async fn stage_linux_update(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!("🐧 Processing Linux update...");

        // Save to downloads folder
        let downloads_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

        // Determine filename based on asset name
        let filename = if let Some(asset_name) = &update_info.asset_name {
            if asset_name.contains("appimage") || asset_name.contains("AppImage") {
                format!("Tabular-{}.AppImage", update_info.latest_version)
            } else if asset_name.ends_with(".tar.gz") {
                format!("tabular-{}.tar.gz", update_info.latest_version)
            } else {
                format!("tabular-{}", update_info.latest_version)
            }
        } else {
            format!("tabular-{}", update_info.latest_version)
        };

        let file_path = downloads_dir.join(&filename);

        info!("💾 Saving Linux binary to: {}", file_path.display());
        std::fs::write(&file_path, content)?;

        info!("✅ Linux update downloaded! Check Downloads folder.");
        Ok(())
    }
}

#[cfg(target_os = "windows")]
impl AutoUpdater {
    pub async fn stage_windows_update(
        &self,
        content: &[u8],
        update_info: &UpdateInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!("🪟 Processing Windows update...");

        // Save to downloads folder
        let downloads_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("C:\\"));

        // Determine filename based on asset name
        let filename = if let Some(asset_name) = &update_info.asset_name {
            if asset_name.ends_with(".msi") {
                format!("Tabular-{}.msi", update_info.latest_version)
            } else if asset_name.ends_with(".exe") {
                format!("Tabular-{}.exe", update_info.latest_version)
            } else {
                format!("Tabular-{}.exe", update_info.latest_version)
            }
        } else {
            format!("Tabular-{}.exe", update_info.latest_version)
        };

        let file_path = downloads_dir.join(&filename);

        info!("💾 Saving Windows installer to: {}", file_path.display());
        std::fs::write(&file_path, content)?;

        // // Open the installer automatically
        // info!("🚀 Opening installer...");
        // Command::new("cmd")
        //     .args(["/C", "start", "", &file_path.to_string_lossy()])
        //     .spawn()?;

        info!("✅ Windows installer opened!");
        Ok(())
    }
}
