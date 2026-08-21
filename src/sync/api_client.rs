//! REST API client for tabular-server.
//! All network calls are async and return Results.
//! The caller is responsible for scheduling them on the Tokio runtime.

use serde::{Deserialize, Serialize};
use reqwest::Client;

use super::TabularAccount;

#[derive(Debug, Clone)]
pub struct ApiClient {
    pub server_url: String,
    pub http: Client,
}

impl ApiClient {
    pub fn new(server_url: &str) -> Self {
        // Lets a server operator gate out clients older than the release that
        // introduced end-to-end vault encryption (see tabular-server's
        // MIN_CLIENT_VERSION / middleware::version_gate) — a client this old
        // doesn't send this header at all, which the gate treats as "too old".
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = reqwest::header::HeaderValue::from_str(env!("CARGO_PKG_VERSION")) {
            headers.insert("X-Tabular-Client-Version", v);
        }

        ApiClient {
            server_url: server_url.trim_end_matches('/').to_string(),
            http: Client::builder()
                .user_agent(concat!("tabular-client/", env!("CARGO_PKG_VERSION")))
                .default_headers(headers)
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.server_url, path)
    }

    /// Refresh the access token using the stored refresh token.
    pub async fn refresh_token(&self, refresh_token: &str) -> anyhow::Result<TokenResponse> {
        let resp = self
            .http
            .post(self.url("/api/v1/auth/refresh"))
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await?
            .error_for_status()?
            .json::<ApiWrapper<TokenResponse>>()
            .await?;
        Ok(resp.data)
    }

    /// Logout (revoke refresh token)
    pub async fn logout(&self, refresh_token: &str, access_token: &str) -> anyhow::Result<()> {
        self.http
            .post(self.url("/api/v1/auth/logout"))
            .bearer_auth(access_token)
            .json(&serde_json::json!({ "refresh_token": refresh_token }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    // ── Connections ──────────────────────────────────────────────────────────

    pub async fn list_connections(&self, token: &str) -> anyhow::Result<Vec<RemoteConnection>> {
        let resp = self.http
            .get(self.url("/api/v1/connections"))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteConnection>>>().await?;
        Ok(resp.data)
    }

    pub async fn create_connection(
        &self,
        token: &str,
        req: &CreateConnectionReq,
    ) -> anyhow::Result<RemoteConnection> {
        let resp = self.http
            .post(self.url("/api/v1/connections"))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteConnection>>().await?;
        Ok(resp.data)
    }

    pub async fn update_connection(
        &self,
        token: &str,
        id: &str,
        req: &serde_json::Value,
    ) -> anyhow::Result<RemoteConnection> {
        let resp = self.http
            .put(self.url(&format!("/api/v1/connections/{}", id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteConnection>>().await?;
        Ok(resp.data)
    }

    pub async fn delete_connection(&self, token: &str, id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/connections/{}", id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    // ── History ──────────────────────────────────────────────────────────────

    pub async fn fetch_history(
        &self,
        token: &str,
        since: Option<&str>,
    ) -> anyhow::Result<Vec<RemoteHistoryItem>> {
        let mut url = self.url("/api/v1/history?limit=500");
        if let Some(s) = since {
            url.push_str(&format!("&since={}", s));
        }
        let resp: serde_json::Value = self.http
            .get(&url)
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json().await?;
        let items: Vec<RemoteHistoryItem> = serde_json::from_value(resp["data"].clone())?;
        Ok(items)
    }

    pub async fn push_history(
        &self,
        token: &str,
        items: Vec<HistoryPushItem>,
    ) -> anyhow::Result<u64> {
        let resp: serde_json::Value = self.http
            .post(self.url("/api/v1/history"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "items": items }))
            .send().await?.error_for_status()?
            .json().await?;
        Ok(resp["inserted"].as_u64().unwrap_or(0))
    }

    // ── Saved Queries ────────────────────────────────────────────────────────

    pub async fn list_queries(&self, token: &str) -> anyhow::Result<Vec<RemoteSavedQuery>> {
        let resp = self.http
            .get(self.url("/api/v1/queries"))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteSavedQuery>>>().await?;
        Ok(resp.data)
    }

    pub async fn create_saved_query(
        &self,
        token: &str,
        req: &CreateQueryReq,
    ) -> anyhow::Result<RemoteSavedQuery> {
        let resp = self.http
            .post(self.url("/api/v1/queries"))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteSavedQuery>>().await?;
        Ok(resp.data)
    }

    /// Used to migrate a legacy (`crypto_version = 0`) query in place to E2E
    /// encryption, and to re-encrypt under a Team key when its folder becomes
    /// Team-shared — both cases update the existing row instead of creating a
    /// duplicate (the server's `create_saved_query` always inserts a new id).
    pub async fn update_saved_query(
        &self,
        token: &str,
        id: &str,
        req: &UpdateQueryReq,
    ) -> anyhow::Result<RemoteSavedQuery> {
        let resp = self.http
            .put(self.url(&format!("/api/v1/queries/{}", id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteSavedQuery>>().await?;
        Ok(resp.data)
    }

    pub async fn delete_saved_query(&self, token: &str, id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/queries/{}", id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    // ── HTTP Requests ────────────────────────────────────────────────────────

    pub async fn list_http_requests(&self, token: &str) -> anyhow::Result<Vec<RemoteHttpRequest>> {
        let resp = self.http
            .get(self.url("/api/v1/http-requests"))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteHttpRequest>>>().await?;
        Ok(resp.data)
    }

    pub async fn create_http_request(
        &self,
        token: &str,
        req: &CreateHttpRequestReq,
    ) -> anyhow::Result<RemoteHttpRequest> {
        let resp = self.http
            .post(self.url("/api/v1/http-requests"))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteHttpRequest>>().await?;
        Ok(resp.data)
    }

    pub async fn update_http_request(
        &self,
        token: &str,
        id: &str,
        req: &UpdateHttpRequestReq,
    ) -> anyhow::Result<RemoteHttpRequest> {
        let resp = self.http
            .put(self.url(&format!("/api/v1/http-requests/{}", id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteHttpRequest>>().await?;
        Ok(resp.data)
    }

    pub async fn delete_http_request(&self, token: &str, id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/http-requests/{}", id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    // ── Collab Rooms ─────────────────────────────────────────────────────────

    pub async fn list_rooms(&self, token: &str) -> anyhow::Result<Vec<super::CollabRoom>> {
        let resp = self.http
            .get(self.url("/api/v1/collab/rooms"))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<super::CollabRoom>>>().await?;
        Ok(resp.data)
    }

    pub async fn create_room(
        &self,
        token: &str,
        name: &str,
        description: Option<&str>,
    ) -> anyhow::Result<super::CollabRoom> {
        let resp = self.http
            .post(self.url("/api/v1/collab/rooms"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "name": name, "description": description }))
            .send().await?.error_for_status()?
            .json::<ApiWrapper<super::CollabRoom>>().await?;
        Ok(resp.data)
    }

    pub async fn delete_room(&self, token: &str, room_id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/collab/rooms/{}", room_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn list_team_rooms(&self, token: &str, team_id: &str) -> anyhow::Result<Vec<super::CollabRoom>> {
        let resp = self.http
            .get(self.url(&format!("/api/v1/teams/{}/rooms", team_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<super::CollabRoom>>>().await?;
        Ok(resp.data)
    }

    pub async fn create_team_room(
        &self,
        token: &str,
        team_id: &str,
        req: &CreateTeamRoomReq,
    ) -> anyhow::Result<super::CollabRoom> {
        let resp = self.http
            .post(self.url(&format!("/api/v1/teams/{}/rooms", team_id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<super::CollabRoom>>().await?;
        Ok(resp.data)
    }

    // ── Teams ─────────────────────────────────────────────────────────────────

    pub async fn list_teams(&self, token: &str) -> anyhow::Result<Vec<RemoteTeam>> {
        let resp = self.http
            .get(self.url("/api/v1/teams"))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteTeam>>>().await?;
        Ok(resp.data)
    }

    pub async fn create_team(&self, token: &str, req: &CreateTeamReq) -> anyhow::Result<RemoteTeam> {
        let resp = self.http
            .post(self.url("/api/v1/teams"))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteTeam>>().await?;
        Ok(resp.data)
    }

    pub async fn delete_team(&self, token: &str, team_id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/teams/{}", team_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn list_team_members(&self, token: &str, team_id: &str) -> anyhow::Result<Vec<RemoteTeamMember>> {
        let resp = self.http
            .get(self.url(&format!("/api/v1/teams/{}/members", team_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteTeamMember>>>().await?;
        Ok(resp.data)
    }

    pub async fn add_team_member(&self, token: &str, team_id: &str, req: &AddTeamMemberReq) -> anyhow::Result<()> {
        self.http
            .post(self.url(&format!("/api/v1/teams/{}/members", team_id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn remove_team_member(&self, token: &str, team_id: &str, user_id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/teams/{}/members/{}", team_id, user_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn list_shared_folders(&self, token: &str, team_id: &str) -> anyhow::Result<Vec<RemoteSharedFolder>> {
        let resp = self.http
            .get(self.url(&format!("/api/v1/teams/{}/shared-folders", team_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteSharedFolder>>>().await?;
        Ok(resp.data)
    }

    pub async fn share_folder(&self, token: &str, team_id: &str, req: &ShareFolderReq) -> anyhow::Result<RemoteSharedFolder> {
        let resp = self.http
            .post(self.url(&format!("/api/v1/teams/{}/shared-folders", team_id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteSharedFolder>>().await?;
        Ok(resp.data)
    }

    pub async fn unshare_folder(&self, token: &str, team_id: &str, folder_id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/teams/{}/shared-folders/{}", team_id, folder_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?;
        Ok(())
    }

    // ── User profile ─────────────────────────────────────────────────────────

    /// PUT /api/v1/users/me — `None` leaves a field untouched, `Some("")` clears it.
    pub async fn update_profile(
        &self,
        token: &str,
        display_name: Option<&str>,
        avatar_url: Option<&str>,
        username: Option<&str>,
        phone: Option<&str>,
    ) -> anyhow::Result<RemoteUser> {
        let resp = self.http
            .put(self.url("/api/v1/users/me"))
            .bearer_auth(token)
            .json(&serde_json::json!({
                "display_name": display_name,
                "avatar_url": avatar_url,
                "username": username,
                "phone": phone,
            }))
            .send().await?.error_for_status()?
            .json::<ApiWrapper<RemoteUser>>().await?;
        Ok(resp.data)
    }

    /// GET /api/v1/users/search?q= — exact match on email, username, or phone.
    pub async fn search_users(&self, token: &str, q: &str) -> anyhow::Result<Vec<RemoteUser>> {
        let url = format!("{}?q={}", self.url("/api/v1/users/search"), percent_encode(q));
        let resp = self.http
            .get(&url)
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<RemoteUser>>>().await?;
        Ok(resp.data)
    }

    // ── Vault Keys (E2E) ─────────────────────────────────────────────────────
    // The server only ever stores/returns opaque ciphertext + the public half
    // of the X25519 keypair here — see sync/vault_crypto.rs for what actually
    // unlocks this bundle (the Sync Passphrase, never sent over the wire).

    /// `None` when the caller has never created a vault yet (fresh account).
    pub async fn get_vault_keys(&self, token: &str) -> anyhow::Result<Option<RemoteVaultKeys>> {
        let resp = self.http
            .get(self.url("/api/v1/vault/keys"))
            .bearer_auth(token)
            .send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let wrapped = resp.error_for_status()?.json::<ApiWrapper<RemoteVaultKeys>>().await?;
        Ok(Some(wrapped.data))
    }

    /// Create or rotate the caller's own vault key bundle.
    pub async fn put_vault_keys(&self, token: &str, req: &PutVaultKeysReq) -> anyhow::Result<()> {
        self.http
            .put(self.url("/api/v1/vault/keys"))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?;
        Ok(())
    }

    /// Bulk-fetch X25519 public keys for the given user ids (used to grant a
    /// Team vault key to fellow members — the server never sees the key itself).
    pub async fn list_public_keys(&self, token: &str, ids: &[String]) -> anyhow::Result<Vec<PublicKeyEntry>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}?ids={}", self.url("/api/v1/users/public-keys"), percent_encode(&ids.join(",")));
        let resp = self.http
            .get(&url)
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<PublicKeyEntry>>>().await?;
        Ok(resp.data)
    }

    // ── Team Vault Key Envelopes ─────────────────────────────────────────────

    /// `None` when this team has no vault key yet, or the caller hasn't been
    /// granted one yet (waiting on another online member's client to grant it).
    pub async fn get_my_key_envelope(&self, token: &str, team_id: &str) -> anyhow::Result<Option<RemoteKeyEnvelope>> {
        let resp = self.http
            .get(self.url(&format!("/api/v1/teams/{}/key-envelopes/me", team_id)))
            .bearer_auth(token)
            .send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let wrapped = resp.error_for_status()?.json::<ApiWrapper<RemoteKeyEnvelope>>().await?;
        Ok(Some(wrapped.data))
    }

    /// Team members who don't have a key envelope yet, with the public key
    /// needed to seal one for them.
    pub async fn list_pending_key_grants(&self, token: &str, team_id: &str) -> anyhow::Result<Vec<PendingKeyGrant>> {
        let resp = self.http
            .get(self.url(&format!("/api/v1/teams/{}/key-envelopes/pending", team_id)))
            .bearer_auth(token)
            .send().await?.error_for_status()?
            .json::<ApiWrapper<Vec<PendingKeyGrant>>>().await?;
        Ok(resp.data)
    }

    /// Upload one or more sealed Team-key envelopes (granting members access).
    pub async fn put_key_envelopes(&self, token: &str, team_id: &str, req: &PutKeyEnvelopesReq) -> anyhow::Result<()> {
        self.http
            .post(self.url(&format!("/api/v1/teams/{}/key-envelopes", team_id)))
            .bearer_auth(token)
            .json(req)
            .send().await?.error_for_status()?;
        Ok(())
    }

    // ── Health check ─────────────────────────────────────────────────────────

    pub async fn health_check(&self) -> bool {
        self.http
            .get(self.url("/health"))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}

/// Minimal query-string value encoder (search identifiers are email/username/phone —
/// only space and the URL-reserved chars need escaping).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ─── Persisted account helpers ────────────────────────────────────────────────

/// Save account to encrypted file store
pub fn save_account(account: &TabularAccount) {
    if let Ok(json) = serde_json::to_string(account) {
        crate::secrets::set_secret("sync:account", &json);
    }
}

/// Load account from encrypted file store
pub fn load_account() -> Option<TabularAccount> {
    let json = crate::secrets::get_secret("sync:account")?;
    serde_json::from_str(&json).ok()
}

/// Clear account from encrypted store (on logout)
pub fn clear_account() {
    crate::secrets::delete_secret("sync:account");
}

// ─── Request / response types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ApiWrapper<T> {
    pub success: bool,
    pub data: T,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: RemoteUser,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RemoteUser {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteConnection {
    pub id: String,
    pub name: String,
    pub db_type: String,
    pub encrypted_config: String,
    pub color_tag: Option<String>,
    #[serde(default)]
    pub folder_path: String,
    /// 0 = legacy (base64 no-op or SHA256(user_id)-keyed — untrusted), 1 = AccountKey/TeamKey AES-256-GCM.
    #[serde(default)]
    pub crypto_version: i32,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct CreateConnectionReq {
    pub name: String,
    pub db_type: String,
    pub encrypted_config: String,
    pub color_tag: Option<String>,
    pub folder_path: Option<String>,
    pub crypto_version: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteHistoryItem {
    pub id: String,
    pub connection_name: String,
    pub query_text: String,
    #[serde(default)]
    pub client_checksum: Option<String>,
    /// 0 = legacy plaintext (untrusted), 1 = AccountKey AES-256-GCM ciphertext.
    #[serde(default)]
    pub crypto_version: i32,
    pub executed_at: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryPushItem {
    pub connection_name: String,
    pub query_text: String,
    pub executed_at: String,
    pub client_checksum: Option<String>,
    pub crypto_version: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteSavedQuery {
    pub id: String,
    pub name: String,
    pub folder_path: String,
    pub query_text: String,
    pub connection_name: Option<String>,
    pub client_checksum: Option<String>,
    /// 0 = legacy plaintext (untrusted), 1 = AccountKey/TeamKey AES-256-GCM ciphertext.
    #[serde(default)]
    pub crypto_version: i32,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct CreateQueryReq {
    pub name: String,
    pub folder_path: Option<String>,
    pub query_text: String,
    pub connection_name: Option<String>,
    pub client_checksum: Option<String>,
    pub crypto_version: i32,
}

#[derive(Debug, Serialize, Default)]
pub struct UpdateQueryReq {
    pub name: Option<String>,
    pub folder_path: Option<String>,
    pub query_text: Option<String>,
    pub connection_name: Option<String>,
    pub client_checksum: Option<String>,
    pub crypto_version: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteHttpRequest {
    pub id: String,
    pub user_id: String,
    pub workspace_name: String,
    pub folder_path: String,
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers_json: Option<String>,
    pub body_json: Option<String>,
    pub auth_json: Option<String>,
    pub client_checksum: Option<String>,
    pub updated_at: String,
    #[serde(default)]
    pub access: String,
    /// 0 = legacy plaintext JSON (untrusted), 1 = AccountKey/TeamKey AES-256-GCM ciphertext.
    #[serde(default)]
    pub crypto_version: i32,
}

#[derive(Debug, Serialize)]
pub struct CreateHttpRequestReq {
    pub workspace_name: String,
    pub folder_path: Option<String>,
    pub name: String,
    pub method: String,
    pub url: String,
    pub headers_json: Option<String>,
    pub body_json: Option<String>,
    pub auth_json: Option<String>,
    pub client_checksum: Option<String>,
    pub crypto_version: i32,
}

#[derive(Debug, Serialize)]
pub struct UpdateHttpRequestReq {
    pub workspace_name: Option<String>,
    pub folder_path: Option<String>,
    pub name: Option<String>,
    pub method: Option<String>,
    pub url: Option<String>,
    pub headers_json: Option<String>,
    pub body_json: Option<String>,
    pub auth_json: Option<String>,
    pub client_checksum: Option<String>,
    pub crypto_version: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct CreateTeamRoomReq {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteTeam {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteTeamMember {
    pub user_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub username: Option<String>,
    pub phone: Option<String>,
    pub role: String,
    pub joined_at: String,
}

#[derive(Debug, Serialize)]
pub struct CreateTeamReq {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AddTeamMemberReq {
    pub identifier: String,
    pub identifier_type: String,
    pub role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteSharedFolder {
    pub id: String,
    pub team_id: String,
    pub resource_type: String,
    pub folder_path: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct ShareFolderReq {
    pub resource_type: String,
    pub folder_path: String,
}

// ─── Vault Keys (E2E) ──────────────────────────────────────────────────────

/// Opaque wrapped-key bundle as stored server-side — see `sync::vault_crypto`
/// for what unlocks it. None of these fields mean anything without the
/// user's Sync Passphrase or recovery code.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteVaultKeys {
    pub kdf_algo: String,
    pub kdf_params_json: String,
    pub salt: String,
    pub wrapped_account_key: String,
    pub x25519_public_key: String,
    pub wrapped_x25519_private_key: String,
    pub recovery_salt: String,
    pub wrapped_account_key_recovery: String,
}

#[derive(Debug, Serialize)]
pub struct PutVaultKeysReq {
    pub kdf_algo: String,
    pub kdf_params_json: String,
    pub salt: String,
    pub wrapped_account_key: String,
    pub x25519_public_key: String,
    pub wrapped_x25519_private_key: String,
    pub recovery_salt: String,
    pub wrapped_account_key_recovery: String,
}

impl From<&crate::sync::vault_crypto::VaultKeyBundle> for PutVaultKeysReq {
    fn from(b: &crate::sync::vault_crypto::VaultKeyBundle) -> Self {
        PutVaultKeysReq {
            kdf_algo: b.kdf_algo.clone(),
            kdf_params_json: b.kdf_params_json.clone(),
            salt: b.salt.clone(),
            wrapped_account_key: b.wrapped_account_key.clone(),
            x25519_public_key: b.x25519_public_key.clone(),
            wrapped_x25519_private_key: b.wrapped_x25519_private_key.clone(),
            recovery_salt: b.recovery_salt.clone(),
            wrapped_account_key_recovery: b.wrapped_account_key_recovery.clone(),
        }
    }
}

impl From<RemoteVaultKeys> for crate::sync::vault_crypto::VaultKeyBundle {
    fn from(r: RemoteVaultKeys) -> Self {
        crate::sync::vault_crypto::VaultKeyBundle {
            kdf_algo: r.kdf_algo,
            kdf_params_json: r.kdf_params_json,
            salt: r.salt,
            wrapped_account_key: r.wrapped_account_key,
            x25519_public_key: r.x25519_public_key,
            wrapped_x25519_private_key: r.wrapped_x25519_private_key,
            recovery_salt: r.recovery_salt,
            wrapped_account_key_recovery: r.wrapped_account_key_recovery,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublicKeyEntry {
    pub id: String,
    pub x25519_public_key: String,
}

// ─── Team Vault Key Envelopes ───────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteKeyEnvelope {
    pub team_id: String,
    pub user_id: String,
    pub wrapped_team_key: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct KeyEnvelopeItemReq {
    pub user_id: String,
    pub wrapped_team_key: String,
}

#[derive(Debug, Serialize)]
pub struct PutKeyEnvelopesReq {
    pub envelopes: Vec<KeyEnvelopeItemReq>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PendingKeyGrant {
    pub user_id: String,
    pub x25519_public_key: String,
}

/// Helper to load an image from HTTP/HTTPS URL, data URI (base64), or local path
/// and decode it into an egui::ColorImage.
pub async fn fetch_image_as_color_image(url_or_path: &str) -> Result<eframe::egui::ColorImage, String> {
    let url_or_path = url_or_path.trim();
    if url_or_path.is_empty() {
        return Err("Empty image URL or path".to_string());
    }

    let bytes = if url_or_path.starts_with("http://") || url_or_path.starts_with("https://") {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client.get(url_or_path).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP error {}", resp.status()));
        }
        resp.bytes().await.map_err(|e| e.to_string())?.to_vec()
    } else if let Some(base64_data) = url_or_path.strip_prefix("data:image/") {
        if let Some((_, b64)) = base64_data.split_once(";base64,") {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(b64.trim())
                .map_err(|e| format!("Base64 decode error: {}", e))?
        } else {
            return Err("Invalid data URI format".to_string());
        }
    } else {
        // Local file path
        std::fs::read(url_or_path).map_err(|e| format!("Failed to read file: {}", e))?
    };

    let image = image::load_from_memory(&bytes).map_err(|e| format!("Image decode error: {}", e))?;
    let rgba = image.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let pixels = rgba.as_flat_samples();
    Ok(eframe::egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice()))
}

