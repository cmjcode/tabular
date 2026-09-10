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
use rand::RngExt;

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
/// 1. Generate a cryptographically random session ticket
/// 2. Optionally bind local TCP listener on 127.0.0.1:0 if permitted by OS / App Sandbox
/// 3. Open browser to `{server_url}/api/v1/auth/login/{provider}?ticket={ticket}[&port={port}]`
/// 4. Simultaneously poll `{server_url}/api/v1/auth/ticket/poll` and listen on loopback
/// 5. Works seamlessly in macOS App Sandbox / TestFlight, behind firewalls, and across all browsers (Safari/Chrome)
pub fn start_oauth_flow(
    server_url: &str,
    provider: OAuthProvider,
) -> mpsc::Receiver<Result<TokenResponse, String>> {
    let (tx, rx) = mpsc::channel();

    // 1. Generate random session ticket (32 hex characters)
    let mut ticket_bytes = [0u8; 16];
    rand::rng().fill(&mut ticket_bytes);
    let ticket = hex::encode(ticket_bytes);

    // 2. Best-effort loopback listener (allowed in dev/unrestricted, but blocked by App Sandbox without server entitlement)
    let listener_res = TcpListener::bind("127.0.0.1:0");
    let (listener_opt, port_opt) = match listener_res {
        Ok(l) => {
            let port = l.local_addr().ok().map(|a| a.port());
            let _ = l.set_nonblocking(true);
            (Some(l), port)
        }
        Err(e) => {
            info!("Local loopback listener not available (App Sandbox or restricted network): {}", e);
            (None, None)
        }
    };

    // 3. Build login URL with ticket and optional port
    let mut url = format!(
        "{}/api/v1/auth/login/{}?ticket={}",
        server_url.trim_end_matches('/'),
        provider.path(),
        ticket
    );
    if let Some(port) = port_opt {
        url.push_str(&format!("&port={}", port));
    }

    // 4. Open the browser
    if let Err(e) = open_url(&url) {
        warn!("Failed to open browser: {}", e);
    }
    info!("Opened OAuth URL: {}", url);

    // 5. Spawn background worker to await authentication via HTTPS polling or loopback callback
    let server_url_owned = server_url.trim_end_matches('/').to_string();
    thread::spawn(move || {
        info!("🔑 Waiting for OAuth authentication (ticket: {}, loopback port: {:?})", ticket, port_opt);
        let start_time = std::time::Instant::now();
        let poll_url = format!("{}/api/v1/auth/ticket/poll", server_url_owned);

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok();

        let mut last_poll = std::time::Instant::now() - Duration::from_secs(5);

        while start_time.elapsed() < Duration::from_secs(180) {
            // A. Ticket Polling via HTTPS (100% compatible with App Sandbox & Safari)
            if last_poll.elapsed() >= Duration::from_millis(1500) {
                last_poll = std::time::Instant::now();
                if let Some(ref http) = client {
                    let body = serde_json::json!({ "ticket": ticket });
                    if let Ok(resp) = http.post(&poll_url).json(&body).send() {
                        if resp.status().is_success() {
                            if let Ok(json) = resp.json::<serde_json::Value>() {
                                let data = json.get("data").unwrap_or(&json);
                                let status = data.get("status").and_then(|s| s.as_str()).unwrap_or("");
                                if status == "completed" {
                                    if let Some(token_val) = data.get("token") {
                                        match serde_json::from_value::<TokenResponse>(token_val.clone()) {
                                            Ok(token_resp) => {
                                                info!("✅ Received valid token response via ticket polling");
                                                let _ = tx.send(Ok(token_resp));
                                                return;
                                            }
                                            Err(e) => {
                                                warn!("❌ Failed to parse TokenResponse from ticket poll: {}", e);
                                                let _ = tx.send(Err(format!("Invalid token JSON from poll: {}", e)));
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // B. Optional Loopback HTTP Server Check
            if let Some(ref listener) = listener_opt {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                    let mut buf = [0u8; 8192];
                    if let Ok(n) = stream.read(&mut buf) {
                        if n > 0 {
                            let request_str = String::from_utf8_lossy(&buf[..n]);

                            if request_str.starts_with("OPTIONS") {
                                let cors_resp = "HTTP/1.1 204 No Content\r\n\
                                                 Access-Control-Allow-Origin: *\r\n\
                                                 Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                                                 Access-Control-Allow-Headers: *\r\n\
                                                 Connection: close\r\n\r\n";
                                let _ = stream.write_all(cors_resp.as_bytes());
                                let _ = stream.flush();
                            } else {
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
                    }
                }
            }

            thread::sleep(Duration::from_millis(200));
        }

        let _ = tx.send(Err("Authentication timed out after 3 minutes".to_string()));
    });

    rx
}

/// Open a URL in the system default browser
fn open_url(_url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(_url)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", _url])
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(_url)
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
        username: resp.user.username.clone(),
        phone: resp.user.phone.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::api_client::RemoteUser;

    #[test]
    fn test_token_to_account_conversion() {
        let resp = TokenResponse {
            access_token: "test_access_token".to_string(),
            refresh_token: "test_refresh_token".to_string(),
            expires_in: 3600,
            user: RemoteUser {
                id: "usr-42".to_string(),
                email: "dev@tabular.id".to_string(),
                display_name: Some("Dev User".to_string()),
                avatar_url: None,
                username: Some("devuser".to_string()),
                phone: Some("+123456789".to_string()),
            },
        };

        let account = token_to_account(&resp);
        assert_eq!(account.user_id, "usr-42");
        assert_eq!(account.email, "dev@tabular.id");
        assert_eq!(account.display_name, Some("Dev User".to_string()));
        assert_eq!(account.avatar_url, None);
        assert_eq!(account.access_token, "test_access_token");
        assert_eq!(account.refresh_token, "test_refresh_token");
        assert_eq!(account.username, Some("devuser".to_string()));
        assert_eq!(account.phone, Some("+123456789".to_string()));
        assert!(!account.is_token_expired());
    }

    #[test]
    fn test_parse_poll_completed_response() {
        let json_data = serde_json::json!({
            "success": true,
            "data": {
                "status": "completed",
                "token": {
                    "access_token": "poll_access",
                    "refresh_token": "poll_refresh",
                    "expires_in": 7200,
                    "user": {
                        "id": "u-99",
                        "email": "poll@tabular.id"
                    }
                }
            }
        });

        let data = json_data.get("data").unwrap();
        let status = data.get("status").and_then(|s| s.as_str()).unwrap();
        assert_eq!(status, "completed");

        let token_val = data.get("token").unwrap();
        let token_resp: TokenResponse = serde_json::from_value(token_val.clone()).expect("parse TokenResponse");
        assert_eq!(token_resp.access_token, "poll_access");
        assert_eq!(token_resp.user.email, "poll@tabular.id");
    }

    #[test]
    fn test_parse_poll_completed_with_account_information() {
        let json_data = serde_json::json!({
            "success": true,
            "data": {
                "status": "completed",
                "token": {
                    "access_token": "acc_access",
                    "refresh_token": "acc_refresh",
                    "expires_in": 3600,
                    "user": {
                        "id": "u-100",
                        "email": "alice@tabular.id",
                        "display_name": "Alice Wonderland",
                        "avatar_url": "https://example.com/alice.png",
                        "username": "alicew",
                        "phone": "+628123456789"
                    }
                }
            }
        });

        let data = json_data.get("data").unwrap();
        let token_val = data.get("token").unwrap();
        let token_resp: TokenResponse = serde_json::from_value(token_val.clone()).expect("parse TokenResponse");
        let account = token_to_account(&token_resp);

        assert_eq!(account.user_id, "u-100");
        assert_eq!(account.email, "alice@tabular.id");
        assert_eq!(account.display_name.as_deref(), Some("Alice Wonderland"));
        assert_eq!(account.avatar_url.as_deref(), Some("https://example.com/alice.png"));
        assert_eq!(account.username.as_deref(), Some("alicew"));
        assert_eq!(account.phone.as_deref(), Some("+628123456789"));
    }
}
