use log::{debug, error, warn};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

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
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpdateError::NetworkError(msg) => {
                if msg.contains("403 Forbidden") {
                    write!(
                        f,
                        "GitHub API rate limit exceeded (403 Forbidden). You can check releases directly at https://github.com/tabular-id/tabular/releases"
                    )
                } else {
                    write!(f, "Network error: {}", msg)
                }
            }
            UpdateError::ParseError(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl Error for UpdateError {}

pub async fn check_for_updates() -> Result<UpdateInfo, UpdateError> {
    debug!("Checking for updates from GitHub releases...");

    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .user_agent(format!("Tabular/{}", CURRENT_VERSION))
        .build()
        .map_err(|e| UpdateError::NetworkError(e.to_string()))?;

    let mut request_builder = client
        .get(&url)
        .header("Accept", "application/vnd.github+json");

    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token_trimmed = token.trim();
        if !token_trimmed.is_empty() {
            request_builder = request_builder.header("Authorization", format!("Bearer {}", token_trimmed));
        }
    }

    let response = request_builder
        .send()
        .await
        .map_err(|e| UpdateError::NetworkError(e.to_string()))?;

    if response.status() == reqwest::StatusCode::FORBIDDEN
        || response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        warn!(
            "GitHub API rate limit or access restriction ({}); falling back to web redirect check...",
            response.status()
        );
        return check_for_updates_web_fallback().await;
    }

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
    let latest_version_str = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
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

pub async fn check_for_updates_web_fallback() -> Result<UpdateInfo, UpdateError> {
    debug!("Checking for updates via web redirect fallback...");
    let web_release_url = format!("https://github.com/{}/releases/latest", GITHUB_REPO);

    let client = reqwest::Client::builder()
        .user_agent(format!("Tabular/{}", CURRENT_VERSION))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| UpdateError::NetworkError(e.to_string()))?;

    let response = match client.head(&web_release_url).send().await {
        Ok(res) if res.status().is_redirection() => res,
        _ => client
            .get(&web_release_url)
            .send()
            .await
            .map_err(|e| UpdateError::NetworkError(e.to_string()))?,
    };

    let redirect_url = if response.status().is_redirection() {
        response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    } else {
        None
    };

    let tag_name = if let Some(ref loc) = redirect_url {
        loc.rsplit('/').next().unwrap_or("").to_string()
    } else {
        String::new()
    };

    if tag_name.is_empty() {
        return Err(UpdateError::NetworkError(
            "GitHub API rate limit exceeded (403 Forbidden). Could not determine latest version from web fallback.".to_string(),
        ));
    }

    debug!("Latest release tag from web fallback: {}", tag_name);

    let current_version = Version::parse(CURRENT_VERSION)
        .map_err(|e| UpdateError::ParseError(format!("Invalid current version: {}", e)))?;

    let latest_version_str = tag_name.strip_prefix('v').unwrap_or(&tag_name);
    let latest_version = Version::parse(latest_version_str)
        .map_err(|e| UpdateError::ParseError(format!("Invalid latest version tag '{}': {}", tag_name, e)))?;

    let update_available = latest_version > current_version;
    let release_url = if let Some(loc) = redirect_url {
        loc
    } else {
        format!("https://github.com/{}/releases/tag/{}", GITHUB_REPO, tag_name)
    };

    let release_notes = if update_available {
        "Note: GitHub API rate limit reached (403 Forbidden). Detailed release notes unavailable in app.\n\nPlease click 'View Release' to open GitHub and download the latest version.".to_string()
    } else {
        "You are on the latest version.".to_string()
    };

    Ok(UpdateInfo {
        current_version: CURRENT_VERSION.to_string(),
        latest_version: latest_version.to_string(),
        update_available,
        release_notes,
        download_url: None,
        asset_name: None,
        release_url,
        published_at: None,
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
            return (
                Some(asset.browser_download_url.clone()),
                Some(asset.name.clone()),
            );
        }
    }

    // Fallback: for macOS, try to find any .dmg file
    if platform.os == "macos" {
        for asset in assets {
            if asset.name.to_lowercase().ends_with(".dmg") {
                warn!("🔄 Using fallback .dmg asset: {}", asset.name);
                return (
                    Some(asset.browser_download_url.clone()),
                    Some(asset.name.clone()),
                );
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
            }
            "linux" => asset_name.contains("linux"),
            "windows" => {
                asset_name.contains("windows")
                    || asset_name.contains(".exe")
                    || asset_name.contains(".msi")
            }
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

pub fn open_url(url: &str) {
    debug!("Opening URL: {}", url);

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = std::process::Command::new("open").arg(url).status() {
            error!("Failed to open URL on macOS: {}", e);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Err(e) = std::process::Command::new("xdg-open").arg(url).status() {
            error!("Failed to open URL on Linux: {}", e);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Err(e) = std::process::Command::new("cmd").args(["/c", "start", url]).status() {
            error!("Failed to open URL on Windows: {}", e);
        }
    }
}

pub fn open_release_page(update_info: &UpdateInfo) {
    open_url(&update_info.release_url);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_matching() {
        let macos_arm = PlatformInfo {
            os: "macos",
            arch: "aarch64",
        };
        assert!(macos_arm.matches("tabular-0.3.0-macos-aarch64.dmg"));
        assert!(macos_arm.matches("tabular-darwin-arm64.tar.gz"));
        assert!(!macos_arm.matches("tabular-linux-x86_64.tar.gz"));

        let linux_x64 = PlatformInfo {
            os: "linux",
            arch: "x86_64",
        };
        assert!(linux_x64.matches("tabular-0.3.0-linux-x86_64.tar.gz"));
        assert!(!linux_x64.matches("tabular-0.3.0-windows-x86_64.zip"));
    }

    #[test]
    fn test_version_parsing() {
        let v1 = Version::parse("0.3.0").unwrap();
        let v2 = Version::parse("0.4.0").unwrap();
        assert!(v2 > v1);
    }

    #[test]
    fn test_update_error_formatting() {
        let err_403 = UpdateError::NetworkError("GitHub API returned status: 403 Forbidden".to_string());
        assert!(err_403.to_string().contains("403 Forbidden"));
        assert!(err_403.to_string().contains("https://github.com/tabular-id/tabular/releases"));

        let err_generic = UpdateError::NetworkError("Connection refused".to_string());
        assert_eq!(err_generic.to_string(), "Network error: Connection refused");
    }
}
