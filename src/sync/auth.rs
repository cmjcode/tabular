/// OAuth 2.0 authentication for the desktop client.
///
/// Flow:
/// 1. User clicks "Login with Google/GitHub"
/// 2. We open a local HTTP server on a random port
/// 3. We redirect the user's browser to the tabular-server /auth/login/{provider}
///    endpoint (which in turn redirects to the OAuth provider)
/// 4. After the user consents, the provider redirects to tabular-server's callback,
///    which returns a JSON response with access_token + refresh_token
///
/// For the desktop flow (PKCE-less via server), we use a "relay" approach:
/// - tabular-server handles the OAuth dance and emits tokens
/// - The client polls a one-time token endpoint, or the server redirects
///   to a custom deeplink: tabular://auth?access_token=...&refresh_token=...
///
/// Simple implementation: the client opens the browser to the server's login URL
/// and shows a "paste token" dialog OR we use a local callback server.

use std::sync::mpsc;
use std::time::Duration;
use log::{info, warn};

use super::{TabularAccount, api_client::TokenResponse};

/// OAuth provider choice
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthProvider {
    Google,
    GitHub,
}

impl OAuthProvider {
    pub fn label(&self) -> &str {
        match self {
            OAuthProvider::Google => "Google",
            OAuthProvider::GitHub => "GitHub",
        }
    }

    pub fn path(&self) -> &str {
        match self {
            OAuthProvider::Google => "google",
            OAuthProvider::GitHub => "github",
        }
    }
}

/// Result returned asynchronously after OAuth completes
#[derive(Debug)]
pub struct AuthResult {
    pub account: TabularAccount,
}

/// Initiate OAuth login:
/// 1. Open the browser to `{server_url}/api/v1/auth/login/{provider}`
/// 2. The server handles the full OAuth dance and at the end returns tokens as JSON
///    OR redirects to tabular://auth callback.
///
/// For simplicity, we open the URL and return a channel.
/// The caller polls the channel while showing a "waiting for browser" dialog.
/// The user must copy the token JSON from the browser into the app.
/// (A more elegant solution would use a local HTTP server callback.)
pub fn start_oauth_flow(
    server_url: &str,
    provider: OAuthProvider,
) -> mpsc::Receiver<Result<TokenResponse, String>> {
    let url = format!("{}/api/v1/auth/login/{}", server_url.trim_end_matches('/'), provider.path());
    let (tx, rx) = mpsc::channel();

    // Open browser
    if let Err(e) = open_url(&url) {
        warn!("Failed to open browser: {}", e);
    }

    info!("Opened OAuth URL: {}", url);
    rx
}

/// Open a URL in the system default browser
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Err("Cannot open browser on this platform".to_string())
    }
}

/// Convert a TokenResponse (from server) into a TabularAccount for local storage
pub fn token_to_account(resp: &TokenResponse) -> TabularAccount {
    let expires_at = chrono::Utc::now().timestamp() + resp.expires_in;
    TabularAccount {
        user_id: resp.user.id.clone(),
        email: resp.user.email.clone(),
        display_name: resp.user.display_name.clone(),
        avatar_url: resp.user.avatar_url.clone(),
        access_token: resp.access_token.clone(),
        refresh_token: resp.refresh_token.clone(),
        token_expires_at: expires_at,
    }
}

/// Try to refresh the access token using the stored refresh token.
/// Returns updated account on success.
pub async fn refresh_if_needed(
    account: &TabularAccount,
    server_url: &str,
) -> Option<TabularAccount> {
    if !account.is_token_expired() {
        return None; // Still valid
    }

    let client = super::api_client::ApiClient::new(server_url);
    match client.refresh_token(&account.refresh_token).await {
        Ok(resp) => {
            info!("✅ Access token refreshed for {}", account.email);
            let updated = token_to_account(&resp);
            super::api_client::save_account(&updated);
            Some(updated)
        }
        Err(e) => {
            warn!("❌ Token refresh failed: {}", e);
            None
        }
    }
}
