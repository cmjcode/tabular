//! OAuth 2.0 authentication for the desktop client.
//!
//! Flow:
//! 1. User clicks "Login with Google/GitHub"
//! 2. We open a local HTTP server on a random port
//! 3. We redirect the user's browser to the tabular-server /auth/login/{provider}
//!    endpoint (which in turn redirects to the OAuth provider)
//! 4. After the user consents, the provider redirects to tabular-server's callback,
//!    which returns a JSON response with access_token + refresh_token
//!
//! For the desktop flow (PKCE-less via server), we use a "relay" approach:
//! - tabular-server handles the OAuth dance and emits tokens
//! - The client polls a one-time token endpoint, or the server redirects
//!   to a custom deeplink: tabular://auth?access_token=...&refresh_token=...
//!
//! Simple implementation: the client opens the browser to the server's login URL
//! and shows a "paste token" dialog OR we use a local callback server.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
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

/// Helper to decode %XX encoded URL components
fn url_decode(input: &str) -> String {
    let mut decoded = String::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                decoded.push(val as char);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            decoded.push(' ');
            i += 1;
            continue;
        }
        decoded.push(bytes[i] as char);
        i += 1;
    }
    decoded
}

/// Initiate OAuth login:
/// 1. Bind local TCP listener on 127.0.0.1:0 (ephemeral port assigned by OS)
/// 2. Open the browser to `{server_url}/api/v1/auth/login/{provider}?port={port}`
/// 3. The server handles OAuth and redirects/fetches to local callback
/// 4. The client thread receives the tokens automatically without manual copy-paste
pub fn start_oauth_flow(
    server_url: &str,
    provider: OAuthProvider,
) -> mpsc::Receiver<Result<TokenResponse, String>> {
    let (tx, rx) = mpsc::channel();

    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            warn!("Failed to bind local loopback listener: {}", e);
            let url = format!("{}/api/v1/auth/login/{}", server_url.trim_end_matches('/'), provider.path());
            let _ = open_url(&url);
            return rx;
        }
    };

    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(_) => 0,
    };

    // Spawn loopback listener in background thread
    thread::spawn(move || {
        info!("🔑 Waiting for OAuth loopback callback on 127.0.0.1:{}", port);
        let start_time = std::time::Instant::now();

        while start_time.elapsed() < Duration::from_secs(180) {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut buf = [0u8; 8192];
                let n = match stream.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => continue,
                };

                let request_str = String::from_utf8_lossy(&buf[..n]);

                // Handle CORS OPTIONS preflight
                if request_str.starts_with("OPTIONS") {
                    let cors_resp = "HTTP/1.1 204 No Content\r\n\
                                     Access-Control-Allow-Origin: *\r\n\
                                     Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                                     Access-Control-Allow-Headers: *\r\n\
                                     Connection: close\r\n\r\n";
                    let _ = stream.write_all(cors_resp.as_bytes());
                    let _ = stream.flush();
                    continue;
                }

                // Extract token JSON payload
                let token_json_opt = if request_str.starts_with("POST") {
                    request_str.split("\r\n\r\n").nth(1).map(|s| s.trim().to_string())
                } else if request_str.starts_with("GET") {
                    if let Some(pos) = request_str.find("token=") {
                        let query_part = &request_str[pos + 6..];
                        let end_pos = query_part.find(' ').unwrap_or(query_part.len());
                        Some(url_decode(&query_part[..end_pos]))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let http_resp = "HTTP/1.1 200 OK\r\n\
                                 Content-Type: text/html\r\n\
                                 Access-Control-Allow-Origin: *\r\n\
                                 Connection: close\r\n\r\n\
                                 <!DOCTYPE html><html><body style='font-family:sans-serif;text-align:center;padding:40px;background:#0f172a;color:#fff;'>\
                                 <h2 style='color:#38bdf8;'>Sign in successful!</h2><p>You can close this tab and return to Tabular.</p></body></html>";
                let _ = stream.write_all(http_resp.as_bytes());
                let _ = stream.flush();

                if let Some(json_str) = token_json_opt {
                    match serde_json::from_str::<TokenResponse>(&json_str) {
                        Ok(token_resp) => {
                            info!("✅ Received valid token response via loopback HTTP");
                            let _ = tx.send(Ok(token_resp));
                            return;
                        }
                        Err(e) => {
                            warn!("❌ Failed to parse TokenResponse from loopback: {}", e);
                            let _ = tx.send(Err(format!("Invalid token JSON: {}", e)));
                            return;
                        }
                    }
                }
            }
        }
        let _ = tx.send(Err("Authentication timed out after 3 minutes".to_string()));
    });

    let url = format!("{}/api/v1/auth/login/{}?port={}", server_url.trim_end_matches('/'), provider.path(), port);
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
