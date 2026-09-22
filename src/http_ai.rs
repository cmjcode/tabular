//! Bantuan AI untuk REST client: menjelaskan response, membuat body JSON dari
//! instruksi, dan menyusun request dari bahasa natural (dijawab model dalam
//! bentuk perintah cURL lalu diterapkan lewat `curl_import`).
//!
//! Modul ini headless: hanya membangun prompt dan mem-parse jawaban. Eksekusi
//! dilakukan UI lewat `ai_assistant::request_text`, sehingga backend apa pun
//! (API key, `agy`, Claude Code, Gemini CLI) bisa dipakai.
//!
//! Semua secret (auth, header/param/field sensitif, userinfo di URL, cookie)
//! diganti `REDACTED` sebelum masuk prompt.

use crate::http_client_widgets::is_sensitive_key;
use crate::models::structs::{HttpAuthType, HttpBodyType, HttpClientState};
use std::sync::{Arc, Mutex, mpsc};

/// Pengganti nilai secret di prompt.
pub const REDACTED: &str = "«redacted»";

/// Batas body request/response yang ikut ke prompt.
const MAX_BODY_BYTES: usize = 8 * 1024;

/// Jenis bantuan AI yang diminta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpAiTask {
    #[default]
    BuildRequest,
    GenerateBody,
    ExplainResponse,
}

impl HttpAiTask {
    pub fn label(self) -> &'static str {
        match self {
            HttpAiTask::BuildRequest => "Build request",
            HttpAiTask::GenerateBody => "Generate body",
            HttpAiTask::ExplainResponse => "Explain response",
        }
    }
}

type ReplyReceiver = Arc<Mutex<mpsc::Receiver<Result<String, String>>>>;

/// State transient bantuan AI per tab HTTP (tidak disimpan ke disk).
#[derive(Debug, Clone, Default)]
pub struct HttpAiState {
    /// Bar prompt di bawah URL bar sedang terbuka.
    pub bar_open: bool,
    /// Tugas yang dipilih di bar prompt (BuildRequest / GenerateBody).
    pub task: HttpAiTask,
    pub prompt: String,
    /// Permintaan yang sedang berjalan beserta channel jawabannya.
    pub pending: Option<(HttpAiTask, ReplyReceiver)>,
    /// Penjelasan response terakhir (Markdown).
    pub explanation: Option<String>,
    pub error: Option<String>,
}

impl HttpAiState {
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    pub fn start(&mut self, task: HttpAiTask, rx: mpsc::Receiver<Result<String, String>>) {
        self.error = None;
        if task == HttpAiTask::ExplainResponse {
            self.explanation = None;
        }
        self.pending = Some((task, Arc::new(Mutex::new(rx))));
    }

    /// Ambil jawaban bila sudah tiba (non-blocking).
    pub fn poll(&mut self) -> Option<(HttpAiTask, Result<String, String>)> {
        let (task, rx) = self.pending.as_ref()?;
        let task = *task;
        let reply = match rx.try_lock().ok()?.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("AI backend stopped without a reply".to_string())
            }
        };
        self.pending = None;
        Some((task, reply))
    }
}

// ─── Penyamaran ─────────────────────────────────────────────────────────────

/// Samarkan userinfo (`user:pass@`) dan nilai query param sensitif di URL.
pub fn redact_url(url: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (url, None),
    };

    let mut out = match base.find("://") {
        Some(scheme_end) => {
            let after = &base[scheme_end + 3..];
            let host_end = after.find('/').unwrap_or(after.len());
            match after[..host_end].rfind('@') {
                Some(at) => format!(
                    "{}{}@{}",
                    &base[..scheme_end + 3],
                    REDACTED,
                    &after[at + 1..]
                ),
                None => base.to_string(),
            }
        }
        None => base.to_string(),
    };

    if let Some(q) = query {
        let pairs: Vec<String> = q
            .split('&')
            .map(|pair| match pair.split_once('=') {
                Some((k, _)) if is_sensitive_key(k) => format!("{k}={REDACTED}"),
                _ => pair.to_string(),
            })
            .collect();
        out.push('?');
        out.push_str(&pairs.join("&"));
    }
    out
}

/// Samarkan nilai field sensitif di dalam JSON secara rekursif.
fn redact_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if is_sensitive_key(k) && !v.is_object() && !v.is_array() {
                    *v = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_json(v);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact_json),
        _ => {}
    }
}

/// Body teks yang aman dikirim: JSON disamarkan per field; lainnya apa adanya.
fn redact_body(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body.trim()) {
        Ok(mut v) => {
            redact_json(&mut v);
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| body.to_string())
        }
        Err(_) => body.to_string(),
    }
}

/// Potong di batas karakter dengan penanda jumlah byte yang dibuang.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [truncated {} bytes]", &s[..end], s.len() - end)
}

fn kv_lines(rows: &[(String, String, bool)]) -> String {
    rows.iter()
        .filter(|(k, _, en)| *en && !k.trim().is_empty())
        .map(|(k, v, _)| {
            let v = if is_sensitive_key(k) {
                REDACTED
            } else {
                v.as_str()
            };
            format!("  {k}: {v}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn auth_summary(state: &HttpClientState) -> String {
    match state.auth_type {
        HttpAuthType::NoAuth => "none".to_string(),
        HttpAuthType::InheritParent => "inherited from collection".to_string(),
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            format!("Bearer token ({REDACTED})")
        }
        HttpAuthType::BasicAuth => format!("Basic auth (user and password {REDACTED})"),
        HttpAuthType::ApiKey => format!(
            "API key `{}` in {} ({REDACTED})",
            state.api_key_name,
            if state.api_key_in_header {
                "header"
            } else {
                "query"
            }
        ),
        ref other => format!("{other:?} (not configured here)"),
    }
}

/// Ringkasan request yang aman dikirim ke model.
pub fn describe_request(state: &HttpClientState) -> String {
    let mut out = format!(
        "{} {}\n",
        state.method.label(),
        redact_url(state.url.trim())
    );
    let params = kv_lines(&state.params);
    if !params.is_empty() {
        out.push_str(&format!("Query params:\n{params}\n"));
    }
    let headers = kv_lines(&state.headers);
    if !headers.is_empty() {
        out.push_str(&format!("Headers:\n{headers}\n"));
    }
    out.push_str(&format!("Auth: {}\n", auth_summary(state)));

    match state.body_type {
        HttpBodyType::NoBody => out.push_str("Body: none\n"),
        HttpBodyType::UrlEncoded | HttpBodyType::MultiPart => {
            out.push_str(&format!(
                "Body ({:?} form fields):\n{}\n",
                state.body_type,
                kv_lines(&state.form_data)
            ));
        }
        HttpBodyType::BinaryFile => out.push_str("Body: binary file\n"),
        _ if state.body_text.trim().is_empty() => {
            out.push_str(&format!("Body ({:?}): empty\n", state.body_type));
        }
        _ => {
            out.push_str(&format!(
                "Body ({:?}):\n{}\n",
                state.body_type,
                truncate(&redact_body(&state.body_text), MAX_BODY_BYTES)
            ));
        }
    }
    out
}

/// Ringkasan response yang aman dikirim ke model.
pub fn describe_response(state: &HttpClientState) -> String {
    if let Some(err) = &state.response_error {
        return format!("The request failed before a response arrived: {err}\n");
    }
    let mut out = format!(
        "Status: {} {}\n",
        state.response_status.unwrap_or(0),
        state.response_status_text
    );
    if let Some(ms) = state.response_time_ms {
        out.push_str(&format!("Time: {ms} ms\n"));
    }
    if !state.response_headers.is_empty() {
        out.push_str("Headers:\n");
        for (k, v) in &state.response_headers {
            let v = if is_sensitive_key(k) {
                REDACTED
            } else {
                v.as_str()
            };
            out.push_str(&format!("  {k}: {v}\n"));
        }
    }
    if state.response_body.is_empty() {
        out.push_str("Body: empty\n");
    } else {
        out.push_str(&format!(
            "Body:\n{}\n",
            truncate(&redact_body(&state.response_body), MAX_BODY_BYTES)
        ));
    }
    out
}

// ─── Prompt ─────────────────────────────────────────────────────────────────

const SYSTEM_BASE: &str = "You are an HTTP/REST API assistant embedded in the Tabular desktop client. \
Values shown as «redacted» are secrets that were removed on purpose: never ask for them and never \
invent real-looking replacements. Do not use tools; answer directly from the information given.";

/// Pasangan (system prompt, user prompt) untuk tugas `task`.
/// `instruction` diabaikan untuk `ExplainResponse`.
pub fn build_prompts(
    task: HttpAiTask,
    state: &HttpClientState,
    instruction: &str,
) -> (String, String) {
    let request = describe_request(state);
    match task {
        HttpAiTask::ExplainResponse => (
            format!(
                "{SYSTEM_BASE}\n\nExplain the HTTP response to a developer in concise Markdown: \
what the status means, what the body contains (its shape and key fields), and, for 4xx/5xx or \
network errors, the most likely causes and concrete fixes to the request. At most ~200 words."
            ),
            format!(
                "Request:\n{request}\nResponse:\n{}",
                describe_response(state)
            ),
        ),
        HttpAiTask::GenerateBody => (
            format!(
                "{SYSTEM_BASE}\n\nReply with ONLY the request body as valid JSON. No prose, no \
explanations, no Markdown code fences."
            ),
            format!(
                "Current request:\n{request}\nWrite a JSON request body for this instruction: {}",
                instruction.trim()
            ),
        ),
        HttpAiTask::BuildRequest => (
            format!(
                "{SYSTEM_BASE}\n\nReply with ONLY one curl command that performs the request, \
nothing else. Use -X for the method, -H for headers and --data-raw for a JSON body. Do not add \
Authorization, Cookie or other credential headers: authentication is configured separately in \
the client. Keep the current scheme and host unless the instruction names another one."
            ),
            format!(
                "Current request:\n{request}\nBuild the request for this instruction: {}",
                instruction.trim()
            ),
        ),
    }
}

// ─── Parsing jawaban ────────────────────────────────────────────────────────

/// Ambil isi code fence pertama bila ada; kalau tidak, kembalikan teks utuh.
fn strip_code_fences(reply: &str) -> &str {
    let Some(start) = reply.find("```") else {
        return reply.trim();
    };
    let after = &reply[start + 3..];
    // Lewati label bahasa (```json / ```bash).
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
    let body = &after[body_start..];
    match body.find("```") {
        Some(end) => body[..end].trim(),
        None => body.trim(),
    }
}

/// JSON pertama di jawaban model, diformat rapi. Teks sebelum/sesudahnya diabaikan.
pub fn extract_json(reply: &str) -> Result<String, String> {
    let text = strip_code_fences(reply);
    let start = text
        .find(['{', '['])
        .ok_or_else(|| "The AI reply did not contain JSON".to_string())?;
    let value = serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<serde_json::Value>()
        .next()
        .ok_or_else(|| "The AI reply did not contain JSON".to_string())?
        .map_err(|e| format!("The AI reply is not valid JSON: {e}"))?;
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

/// Perintah curl pertama di jawaban model (termasuk baris lanjutan `\`).
pub fn extract_curl(reply: &str) -> Option<String> {
    let text = strip_code_fences(reply);
    let lines: Vec<&str> = text.lines().collect();
    let first = lines.iter().position(|l| {
        let t = l.trim_start().trim_start_matches("$ ");
        t.starts_with("curl ") || t == "curl"
    })?;
    let mut out = Vec::new();
    for line in &lines[first..] {
        if line.trim().is_empty() {
            break;
        }
        out.push(line.trim_start().trim_start_matches("$ "));
        if !line.trim_end().ends_with('\\') {
            break;
        }
    }
    Some(out.join("\n"))
}

/// Terapkan curl hasil AI ke state tanpa menghapus konfigurasi auth yang
/// sudah ada, dan buang header yang berisi placeholder `REDACTED`.
pub fn apply_generated_curl(
    state: &mut HttpClientState,
    curl: &str,
) -> Result<Vec<String>, String> {
    let saved_auth = (
        state.auth_type.clone(),
        state.bearer_token.clone(),
        state.basic_user.clone(),
        state.basic_pass.clone(),
        state.api_key_name.clone(),
        state.api_key_value.clone(),
        state.api_key_in_header,
    );
    let mut warnings = crate::curl_import::apply_to_state(state, curl)?;

    if matches!(state.auth_type, HttpAuthType::NoAuth) {
        (
            state.auth_type,
            state.bearer_token,
            state.basic_user,
            state.basic_pass,
            state.api_key_name,
            state.api_key_value,
            state.api_key_in_header,
        ) = saved_auth;
    }

    let before = state.headers.len();
    state.headers.retain(|(_, v, _)| !v.contains(REDACTED));
    if state.headers.len() != before {
        warnings.push("Dropped headers that contained redacted placeholders".to_string());
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::HttpMethod;

    fn state() -> HttpClientState {
        HttpClientState {
            method: HttpMethod::POST,
            url: "https://bob:hunter2@api.example.com/v1/login?page=2&access_token=abc".into(),
            headers: vec![
                ("Authorization".into(), "Bearer sk-live-123".into(), true),
                ("Accept".into(), "application/json".into(), true),
                ("X-Off".into(), "x".into(), false),
            ],
            params: vec![("api_key".into(), "k-999".into(), true)],
            auth_type: HttpAuthType::BearerToken,
            bearer_token: "tok-secret".into(),
            body_type: HttpBodyType::Json,
            body_text: r#"{"user":"bob","password":"pw-1","nested":{"client_secret":"cs"}}"#.into(),
            ..Default::default()
        }
    }

    #[test]
    fn request_description_never_contains_secrets() {
        let text = describe_request(&state());
        for secret in [
            "hunter2",
            "abc",
            "sk-live-123",
            "k-999",
            "tok-secret",
            "pw-1",
            "\"cs\"",
        ] {
            assert!(!text.contains(secret), "leaked {secret}:\n{text}");
        }
        assert!(text.contains("page=2"));
        assert!(text.contains("\"user\": \"bob\""));
        assert!(text.contains("Accept: application/json"));
        assert!(!text.contains("X-Off"), "disabled rows are not sent");
    }

    #[test]
    fn response_description_redacts_cookies_and_truncates() {
        let s = HttpClientState {
            response_status: Some(200),
            response_status_text: "OK".into(),
            response_headers: vec![("Set-Cookie".into(), "sid=zzz".into())],
            response_body: "x".repeat(MAX_BODY_BYTES + 100),
            ..Default::default()
        };
        let text = describe_response(&s);
        assert!(!text.contains("sid=zzz"));
        assert!(text.contains("[truncated 100 bytes]"));
    }

    #[test]
    fn redact_url_keeps_plain_urls() {
        assert_eq!(redact_url("http://h/p?q=1"), "http://h/p?q=1");
        assert_eq!(redact_url("h/p"), "h/p");
        assert_eq!(
            redact_url("https://u:p@h:8080/a?token=1&x"),
            format!("https://{REDACTED}@h:8080/a?token={REDACTED}&x")
        );
    }

    #[test]
    fn extract_json_handles_fences_and_prose() {
        assert_eq!(
            extract_json("```json\n{\"a\":1}\n```").unwrap(),
            "{\n  \"a\": 1\n}"
        );
        assert_eq!(
            extract_json("Here you go: [1, 2] hope it helps").unwrap(),
            "[\n  1,\n  2\n]"
        );
        assert!(extract_json("no json here").is_err());
        assert!(extract_json("{\"a\": }").is_err());
    }

    #[test]
    fn extract_curl_takes_first_command_with_continuations() {
        let reply = "Sure!\n```bash\n$ curl -X POST https://h/a \\\n  -H 'Accept: */*'\n```\nDone.";
        assert_eq!(
            extract_curl(reply).unwrap(),
            "curl -X POST https://h/a \\\n-H 'Accept: */*'"
        );
        assert_eq!(
            extract_curl("curl https://h/x").unwrap(),
            "curl https://h/x"
        );
        assert!(extract_curl("I cannot do that").is_none());
    }

    #[test]
    fn generated_curl_keeps_existing_auth_and_drops_redacted_headers() {
        let mut s = state();
        let warnings = apply_generated_curl(
            &mut s,
            &format!("curl -X GET https://api.example.com/v1/users -H 'X-Token: {REDACTED}'"),
        )
        .unwrap();
        assert_eq!(s.method, HttpMethod::GET);
        assert!(s.url.contains("/v1/users"));
        assert_eq!(s.auth_type, HttpAuthType::BearerToken);
        assert_eq!(s.bearer_token, "tok-secret");
        assert!(s.headers.iter().all(|(_, v, _)| !v.contains(REDACTED)));
        assert!(!warnings.is_empty());
    }

    #[test]
    fn prompts_for_each_task_include_request_and_instruction() {
        let s = state();
        let (sys, user) = build_prompts(HttpAiTask::BuildRequest, &s, "list users");
        assert!(sys.contains("curl"));
        assert!(user.contains("list users") && user.contains("api.example.com"));
        let (sys, _) = build_prompts(HttpAiTask::GenerateBody, &s, "x");
        assert!(sys.contains("ONLY the request body"));
        let (_, user) = build_prompts(HttpAiTask::ExplainResponse, &s, "");
        assert!(user.contains("Response:"));
    }

    /// End-to-end dengan `agy` sungguhan (butuh binary + login), memakai data
    /// sintetis. Jalankan dengan `cargo test --lib -- --ignored real_agy`.
    #[test]
    #[ignore]
    fn real_agy_builds_request_and_generates_body() {
        use crate::agent::harness::CliAgentConfig;
        use crate::config::{AiBackend, AiProvider, CliAgentKind};

        let backend = crate::ai_assistant::ChatBackend {
            target: crate::config::ChatTarget::Cli(CliAgentKind::Antigravity),
            backend: AiBackend::Cli,
            provider: AiProvider::OpenAI,
            api_key: String::new(),
            model: String::new(),
            base_url: String::new(),
            cli: CliAgentConfig {
                kind: CliAgentKind::Antigravity,
                model: "gemini-3.8-flash-low".into(),
                effort: "low".into(),
                ..Default::default()
            },
            mcp_available: false,
            notes_enabled: false,
            notes_writable: false,
        };
        let ask = |task, s: &HttpClientState, instruction: &str| {
            let (system, user) = build_prompts(task, s, instruction);
            crate::ai_assistant::request_text(&backend, system, user)
                .recv_timeout(std::time::Duration::from_secs(180))
                .expect("agy reply in time")
                .expect("agy reply ok")
        };

        let mut s = state();
        let reply = ask(
            HttpAiTask::BuildRequest,
            &s,
            "GET page 2 of /v1/permissions sorted by name",
        );
        eprintln!("build reply:\n{reply}");
        let curl = extract_curl(&reply).expect("curl in reply");
        apply_generated_curl(&mut s, &curl).expect("apply curl");
        assert_eq!(s.method, HttpMethod::GET);
        assert!(s.url.contains("/v1/permissions") || s.url.contains("permissions"));
        assert_eq!(s.bearer_token, "tok-secret", "auth must survive");

        let reply = ask(
            HttpAiTask::GenerateBody,
            &state(),
            "a user with name, email and a list of 2 roles",
        );
        eprintln!("body reply:\n{reply}");
        let json = extract_json(&reply).expect("json in reply");
        assert!(json.contains("email"), "{json}");
    }

    #[test]
    fn poll_reports_disconnected_backend() {
        let mut ai = HttpAiState::default();
        let (tx, rx) = mpsc::channel();
        ai.start(HttpAiTask::GenerateBody, rx);
        assert!(ai.poll().is_none());
        drop(tx);
        let (task, reply) = ai.poll().unwrap();
        assert_eq!(task, HttpAiTask::GenerateBody);
        assert!(reply.is_err());
        assert!(!ai.is_busy());
    }
}
