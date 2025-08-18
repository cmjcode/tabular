use semver::Version;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use log::{debug, error, info, warn};

const GITHUB_REPO: &str = "tabular-id/tabular";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    body: String,
    assets: Vec<GitHubAsset>,
    html_url: String,
    published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_notes: String,
    pub download_url: Option<String>,
    pub asset_name: Option<String>,
    pub release_url: String,
    pub published_at: Option<String>,
}

#[derive(Debug)]
pub enum UpdateError {
    NetworkError(String),
    ParseError(String),
    UnsupportedPlatform,
    UpdateFailed(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::NetworkError(msg) => write!(f, "Network error: {}", msg),
            UpdateError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            UpdateError::UnsupportedPlatform => write!(f, "Unsupported platform for auto-update"),
            UpdateError::UpdateFailed(msg) => write!(f, "Update failed: {}", msg),
        }
    }
}

impl Error for UpdateError {}

pub async fn check_for_updates() -> Result<UpdateInfo, UpdateError> {
    info!("Checking for updates from GitHub releases...");
    
    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);
    
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("User-Agent", format!("Tabular/{}", CURRENT_VERSION))
        .send()
        .await
        .map_err(|e| UpdateError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(UpdateError::NetworkError(format!(
            "GitHub API returned status: {}",
            response.status()
        )));
    }

    let release: GitHubRelease = response
        .json()
        .await
        .map_err(|e| UpdateError::ParseError(e.to_string()))?;

    debug!("Latest release: {}", release.tag_name);

    // Parse versions
    let current_version = Version::parse(CURRENT_VERSION)
        .map_err(|e| UpdateError::ParseError(format!("Invalid current version: {}", e)))?;
    
    // Remove 'v' prefix if present
    let latest_version_str = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name);
    let latest_version = Version::parse(latest_version_str)
        .map_err(|e| UpdateError::ParseError(format!("Invalid latest version: {}", e)))?;

    let update_available = latest_version > current_version;

    // Find appropriate asset for current platform
    let (download_url, asset_name) = if update_available {
        find_asset_for_platform(&release.assets)
    } else {
        (None, None)
    };

    Ok(UpdateInfo {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: latest_version.to_string(),
        update_available,
        release_notes: release.body,
        download_url,
        asset_name,
        release_url: release.html_url,
        published_at: release.published_at,
    })
}

fn find_asset_for_platform(assets: &[GitHubAsset]) -> (Option<String>, Option<String>) {
    let platform = get_platform_info();
    
    debug!("🔍 Searching for asset matching platform: {}", platform);
    debug!("📦 Available assets:");
    for asset in assets {
        debug!("  - {}", asset.name);
    }
    
    for asset in assets {
        let asset_name_lower = asset.name.to_lowercase();
        
        if platform.matches(&asset_name_lower) {
            debug!("✅ Found matching asset: {}", asset.name);
            return (Some(asset.browser_download_url.clone()), Some(asset.name.clone()));
        }
    }
    
    // Fallback: for macOS, try to find any .dmg file
    if platform.os == "macos" {
        for asset in assets {
            if asset.name.to_lowercase().ends_with(".dmg") {
                warn!("🔄 Using fallback .dmg asset: {}", asset.name);
                return (Some(asset.browser_download_url.clone()), Some(asset.name.clone()));
            }
        }
    }
    
    warn!("❌ No matching asset found for platform: {}", platform);
    (None, None)
}

#[derive(Debug)]
struct PlatformInfo {
    os: &'static str,
    arch: &'static str,
}

impl PlatformInfo {
    fn matches(&self, asset_name: &str) -> bool {
        let os_matches = match self.os {
            "macos" => {
                // More flexible matching for macOS
                asset_name.contains("macos") 
                || asset_name.contains("darwin") 
                || asset_name.ends_with(".dmg")
                || (asset_name.starts_with("tabular") && asset_name.ends_with(".dmg"))
            },
            "linux" => asset_name.contains("linux"),
            "windows" => asset_name.contains("windows") || asset_name.contains(".exe") || asset_name.contains(".msi"),
            _ => false,
        };
        
        // For macOS .dmg files, we don't require architecture-specific naming
        if self.os == "macos" && asset_name.ends_with(".dmg") {
            return true;
        }
        
        let arch_matches = match self.arch {
            "x86_64" => asset_name.contains("x86_64") || asset_name.contains("amd64"),
            "aarch64" => asset_name.contains("aarch64") || asset_name.contains("arm64"),
            _ => true, // Fallback to any architecture if not specified
        };
        
        os_matches && (arch_matches || asset_name.ends_with(".dmg"))
    }
}

impl fmt::Display for PlatformInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os, self.arch)
    }
}

fn get_platform_info() -> PlatformInfo {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };
    
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    };
    
    PlatformInfo { os, arch }
}

pub async fn download_and_install_update(update_info: &UpdateInfo) -> Result<(), UpdateError> {
    let download_url = update_info.download_url.as_ref()
        .ok_or(UpdateError::UnsupportedPlatform)?;
    
    info!("🚀 Starting update process...");
    info!("📥 Downloading update from: {}", download_url);
    info!("📦 Asset name: {:?}", update_info.asset_name);
    info!("🎯 Target platform: {}", target_triple());
    
    // Download the file manually instead of using self_update crate
    let client = reqwest::Client::new();
    let response = client
        .get(download_url)
        .header("User-Agent", format!("Tabular/{}", CURRENT_VERSION))
        .send()
        .await
        .map_err(|e| UpdateError::NetworkError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(UpdateError::NetworkError(format!(
            "Download failed with status: {}",
            response.status()
        )));
    }

    // Get the file content
    let content = response
        .bytes()
        .await
        .map_err(|e| UpdateError::UpdateFailed(format!("Failed to read download content: {}", e)))?;

    info!("📦 Downloaded {} bytes", content.len());

    // For macOS DMG files, we'll just save it to Downloads and notify the user
    #[cfg(target_os = "macos")]
    {
        let asset_name = update_info.asset_name.as_ref()
            .ok_or(UpdateError::UpdateFailed("No asset name available".to_string()))?;
        
        // Save to Downloads folder
        let downloads_dir = dirs::download_dir()
            .ok_or(UpdateError::UpdateFailed("Could not find Downloads directory".to_string()))?;
        
        let file_path = downloads_dir.join(asset_name);
        
        std::fs::write(&file_path, &content)
            .map_err(|e| UpdateError::UpdateFailed(format!("Failed to save file: {}", e)))?;
        
        info!("✅ Update downloaded to: {}", file_path.display());
        info!("� Please manually install the downloaded DMG file");
        
        // On macOS, open the Downloads folder to show the file
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(&file_path)
            .spawn();
        
        Ok(())
    }
    
    #[cfg(not(target_os = "macos"))]
    {
        // For other platforms, we could implement automatic installation later
        Err(UpdateError::UnsupportedPlatform)
    }
}

fn target_triple() -> &'static str {
    if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "aarch64-pc-windows-msvc"
    } else {
        "unknown"
    }
}

pub fn open_release_page(update_info: &UpdateInfo) {
    let url = &update_info.release_url;
    info!("Opening release page: {}", url);
    
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(url)
            .spawn()
            .unwrap_or_else(|e| {
                error!("Failed to open URL on macOS: {}", e);
                std::process::exit(1);
            }).wait();
    }
    
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .unwrap_or_else(|e| {
                error!("Failed to open URL on Linux: {}", e);
                std::process::exit(1);
            });
    }
    
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", url])
            .spawn()
            .unwrap_or_else(|e| {
                error!("Failed to open URL on Windows: {}", e);
                std::process::exit(1);
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_matching() {
        let macos_arm = PlatformInfo { os: "macos", arch: "aarch64" };
        assert!(macos_arm.matches("tabular-0.3.0-macos-aarch64.dmg"));
        assert!(macos_arm.matches("tabular-darwin-arm64.tar.gz"));
        assert!(!macos_arm.matches("tabular-linux-x86_64.tar.gz"));
        
        let linux_x64 = PlatformInfo { os: "linux", arch: "x86_64" };
        assert!(linux_x64.matches("tabular-0.3.0-linux-x86_64.tar.gz"));
        assert!(!linux_x64.matches("tabular-0.3.0-windows-x86_64.zip"));
    }

    #[test]
    fn test_version_parsing() {
        let v1 = Version::parse("0.3.0").unwrap();
        let v2 = Version::parse("0.4.0").unwrap();
        assert!(v2 > v1);
    }
}
