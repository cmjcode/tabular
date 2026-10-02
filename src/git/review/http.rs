//! Klien HTTP blocking kecil untuk API GitHub/GitLab.

use std::time::Duration;

use serde::de::DeserializeOwned;

use super::super::GitError;

const TIMEOUT: Duration = Duration::from_secs(30);
/// Batas panjang body error yang ikut ke pesan.
const MAX_ERROR_BODY: usize = 300;

pub fn client() -> Result<reqwest::blocking::Client, GitError> {
    reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("Tabular/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| GitError::Network(e.to_string()))
}

/// Kirim request dan parse JSON; status non-2xx menjadi [`GitError::Http`].
pub fn send_json<T: DeserializeOwned>(
    req: reqwest::blocking::RequestBuilder,
) -> Result<T, GitError> {
    let text = send_text(req)?;
    serde_json::from_str(&text).map_err(|e| GitError::Parse(e.to_string()))
}

/// Kirim request, kembalikan body teks bila sukses.
pub fn send_text(req: reqwest::blocking::RequestBuilder) -> Result<String, GitError> {
    let resp = req.send().map_err(|e| GitError::Network(without_url(&e)))?;
    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| GitError::Network(without_url(&e)))?;
    if status.is_success() {
        return Ok(body);
    }
    Err(GitError::Http {
        status: status.as_u16(),
        body: error_message(&body),
    })
}

/// Ambil `message` dari body JSON error bila ada, dipotong.
fn error_message(body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("message").or_else(|| v.get("error")).map(|m| {
                m.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| m.to_string())
            })
        })
        .unwrap_or_else(|| body.trim().to_string());
    let mut end = msg.len().min(MAX_ERROR_BODY);
    while !msg.is_char_boundary(end) {
        end -= 1;
    }
    msg[..end].to_string()
}

/// Pesan error reqwest tanpa URL (URL GitLab bisa memuat query sensitif).
fn without_url(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    if let Some(url) = e.url() {
        s = s.replace(url.as_str(), "<url>");
    }
    s
}

/// Encode segmen path (`group/sub/app` → `group%2Fsub%2Fapp`).
pub fn encode_segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_body_uses_message_field() {
        assert_eq!(
            error_message(r#"{"message":"Bad credentials"}"#),
            "Bad credentials"
        );
        assert_eq!(error_message(r#"{"message":["x"]}"#), r#"["x"]"#);
        assert_eq!(error_message("plain"), "plain");
        assert_eq!(error_message(&"é".repeat(400)).len(), 300);
    }

    #[test]
    fn encodes_nested_project_path() {
        assert_eq!(encode_segment("grp/sub app"), "grp%2Fsub%20app");
    }
}
