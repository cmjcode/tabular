/// REST API client for tabular-server.
/// All network calls are async and return Results.
/// The caller is responsible for scheduling them on the Tokio runtime.

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
        ApiClient {
            server_url: server_url.trim_end_matches('/').to_string(),
            http: Client::builder()
                .user_agent("tabular-client/0.10")
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

    pub async fn delete_saved_query(&self, token: &str, id: &str) -> anyhow::Result<()> {
        self.http
            .delete(self.url(&format!("/api/v1/queries/{}", id)))
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteConnection {
    pub id: String,
    pub name: String,
    pub db_type: String,
    pub encrypted_config: String,
    pub color_tag: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct CreateConnectionReq {
    pub name: String,
    pub db_type: String,
    pub encrypted_config: String,
    pub color_tag: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteHistoryItem {
    pub id: String,
    pub connection_name: String,
    pub query_text: String,
    pub executed_at: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryPushItem {
    pub connection_name: String,
    pub query_text: String,
    pub executed_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RemoteSavedQuery {
    pub id: String,
    pub name: String,
    pub folder_path: String,
    pub query_text: String,
    pub connection_name: Option<String>,
    pub client_checksum: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Serialize)]
pub struct CreateQueryReq {
    pub name: String,
    pub folder_path: Option<String>,
    pub query_text: String,
    pub connection_name: Option<String>,
    pub client_checksum: Option<String>,
}
