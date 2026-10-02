//! Kirim request HTTP tanpa UI. Dipakai HTTP client (tab request) dan runner
//! integration test, supaya keduanya mengirim request dengan cara yang sama.

use crate::http_collection::SavedRequest;
use crate::models::structs::{
    HttpAuthType, HttpBodyType, HttpClientResponse, HttpClientState, HttpMethod,
};

/// Data satu request, sudah terlepas dari state UI.
#[derive(Debug, Clone, Default)]
pub struct RequestSpec {
    pub url: String,
    pub method: HttpMethod,
    pub body_type: HttpBodyType,
    pub body_text: String,
    /// Field form yang aktif dan bernama.
    pub form_data: Vec<(String, String)>,
    /// Query param yang aktif dan bernama.
    pub params: Vec<(String, String)>,
    /// Header yang aktif dan bernama.
    pub headers: Vec<(String, String)>,
    pub auth_type: HttpAuthType,
    pub bearer_token: String,
    pub basic_user: String,
    pub basic_pass: String,
    pub api_key_name: String,
    pub api_key_value: String,
    pub api_key_in_header: bool,
}

fn enabled(rows: &[(String, String, bool)]) -> Vec<(String, String)> {
    rows.iter()
        .filter(|(k, _, en)| *en && !k.is_empty())
        .map(|(k, v, _)| (k.clone(), v.clone()))
        .collect()
}

impl RequestSpec {
    pub fn from_state(state: &HttpClientState) -> Self {
        Self {
            url: state.url.clone(),
            method: state.method.clone(),
            body_type: state.body_type.clone(),
            body_text: state.body_text.clone(),
            form_data: enabled(&state.form_data),
            params: enabled(&state.params),
            headers: enabled(&state.headers),
            auth_type: state.auth_type.clone(),
            bearer_token: state.bearer_token.clone(),
            basic_user: state.basic_user.clone(),
            basic_pass: state.basic_pass.clone(),
            api_key_name: state.api_key_name.clone(),
            api_key_value: state.api_key_value.clone(),
            api_key_in_header: state.api_key_in_header,
        }
    }

    pub fn from_saved(req: &SavedRequest) -> Self {
        Self {
            url: req.url.clone(),
            method: req.method.clone(),
            body_type: req.body_type.clone(),
            body_text: req.body_text.clone(),
            form_data: enabled(&req.form_data),
            params: enabled(&req.params),
            headers: enabled(&req.headers),
            auth_type: req.auth_type.clone(),
            bearer_token: req.bearer_token.clone(),
            basic_user: req.basic_user.clone(),
            basic_pass: req.basic_pass.clone(),
            api_key_name: req.api_key_name.clone(),
            api_key_value: req.api_key_value.clone(),
            api_key_in_header: req.api_key_in_header,
        }
    }

    /// Ganti `{{KEY}}` yang dikenal di semua bagian request dengan nilai dari
    /// environment project. Placeholder yang tidak dikenal dibiarkan.
    pub fn with_vars(mut self, vars: &std::collections::HashMap<String, String>) -> Self {
        if vars.is_empty() {
            return self;
        }
        let sub = |s: &str| crate::http_tests::substitute(s, vars);
        let rows = |r: &[(String, String)]| -> Vec<(String, String)> {
            r.iter().map(|(k, v)| (sub(k), sub(v))).collect()
        };
        self.url = sub(&self.url);
        self.body_text = sub(&self.body_text);
        self.form_data = rows(&self.form_data);
        self.params = rows(&self.params);
        self.headers = rows(&self.headers);
        self.bearer_token = sub(&self.bearer_token);
        self.basic_user = sub(&self.basic_user);
        self.basic_pass = sub(&self.basic_pass);
        self.api_key_name = sub(&self.api_key_name);
        self.api_key_value = sub(&self.api_key_value);
        self
    }

    /// URL lengkap dengan query param (dan API key bila dikirim lewat query).
    pub fn full_url(&self) -> String {
        let mut full_url = self.url.clone();
        let mut push = |q: String| {
            full_url.push(if full_url.contains('?') { '&' } else { '?' });
            full_url.push_str(&q);
        };
        if !self.params.is_empty() {
            push(
                self.params
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        if matches!(self.auth_type, HttpAuthType::ApiKey) && !self.api_key_in_header {
            push(format!("{}={}", self.api_key_name, self.api_key_value));
        }
        full_url
    }
}

fn failed(error: String, time_ms: u128) -> HttpClientResponse {
    HttpClientResponse {
        status: 0,
        status_text: String::new(),
        body: String::new(),
        headers: Vec::new(),
        time_ms,
        size_bytes: 0,
        error: Some(error),
    }
}

/// Kirim satu request. Tidak pernah panic; kegagalan jaringan menjadi
/// `HttpClientResponse::error`.
pub async fn send(client: &reqwest::Client, spec: RequestSpec) -> HttpClientResponse {
    let start = std::time::Instant::now();
    let full_url = spec.full_url();

    let mut req_builder = match spec.method {
        HttpMethod::GET => client.get(&full_url),
        HttpMethod::POST => client.post(&full_url),
        HttpMethod::PUT => client.put(&full_url),
        HttpMethod::DELETE => client.delete(&full_url),
        HttpMethod::PATCH => client.patch(&full_url),
        HttpMethod::HEAD => client.head(&full_url),
        HttpMethod::OPTIONS => client.request(reqwest::Method::OPTIONS, &full_url),
    };

    for (k, v) in &spec.headers {
        req_builder = req_builder.header(k.as_str(), v.as_str());
    }

    match spec.auth_type {
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            req_builder =
                req_builder.header("Authorization", format!("Bearer {}", spec.bearer_token));
        }
        HttpAuthType::BasicAuth => {
            req_builder = req_builder.basic_auth(&spec.basic_user, Some(&spec.basic_pass));
        }
        HttpAuthType::ApiKey if spec.api_key_in_header && !spec.api_key_name.is_empty() => {
            req_builder =
                req_builder.header(spec.api_key_name.as_str(), spec.api_key_value.as_str());
        }
        _ => {}
    }

    req_builder = match &spec.body_type {
        HttpBodyType::Json | HttpBodyType::GraphQL => req_builder
            .header("Content-Type", "application/json")
            .body(spec.body_text.clone()),
        HttpBodyType::Xml => req_builder
            .header("Content-Type", "application/xml")
            .body(spec.body_text.clone()),
        HttpBodyType::OtherText => req_builder.body(spec.body_text.clone()),
        HttpBodyType::UrlEncoded => req_builder.form(&spec.form_data),
        HttpBodyType::MultiPart => {
            let mut form = reqwest::multipart::Form::new();
            for (k, v) in spec.form_data {
                form = form.text(k, v);
            }
            req_builder.multipart(form)
        }
        HttpBodyType::NoBody | HttpBodyType::BinaryFile => req_builder,
    };

    match req_builder.send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            let status_text = response
                .status()
                .canonical_reason()
                .unwrap_or("")
                .to_string();
            let headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_string()))
                .collect();
            let body = response.text().await.unwrap_or_default();
            HttpClientResponse {
                status,
                status_text,
                size_bytes: body.len(),
                body,
                headers,
                time_ms: start.elapsed().as_millis(),
                error: None,
            }
        }
        Err(e) => failed(e.to_string(), start.elapsed().as_millis()),
    }
}

/// Klien HTTP standar Tabular (sertifikat tetap diverifikasi).
pub fn default_client() -> reqwest::Client {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(false)
        .build()
        .unwrap_or_default()
}

/// Kirim satu request dari thread biasa (membuat runtime tokio sendiri).
pub fn send_blocking(spec: RequestSpec) -> HttpClientResponse {
    match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt.block_on(async move { send(&default_client(), spec).await }),
        Err(e) => {
            log::error!("[HTTP] failed to start tokio runtime: {e}");
            failed(format!("Could not start HTTP runtime: {e}"), 0)
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn with_vars_substitutes_all_parts() {
        let state = HttpClientState {
            url: "{{BASE_URL}}/users".into(),
            headers: vec![("Authorization".into(), "Bearer {{TOKEN}}".into(), true)],
            ..Default::default()
        };
        let vars: std::collections::HashMap<String, String> = [
            ("BASE_URL".to_string(), "https://api.dev".to_string()),
            ("TOKEN".to_string(), "t0k".to_string()),
        ]
        .into();
        let spec = RequestSpec::from_state(&state).with_vars(&vars);
        assert_eq!(spec.url, "https://api.dev/users");
        assert_eq!(spec.headers[0].1, "Bearer t0k");
        let untouched = RequestSpec::from_state(&state).with_vars(&Default::default());
        assert_eq!(untouched.url, "{{BASE_URL}}/users");
    }

    use super::*;

    #[test]
    fn full_url_appends_params_and_query_api_key() {
        let spec = RequestSpec {
            url: "http://h/x?a=1".into(),
            params: vec![("b".into(), "2".into())],
            auth_type: HttpAuthType::ApiKey,
            api_key_name: "key".into(),
            api_key_value: "v".into(),
            api_key_in_header: false,
            ..Default::default()
        };
        assert_eq!(spec.full_url(), "http://h/x?a=1&b=2&key=v");
    }

    #[test]
    fn from_saved_keeps_only_enabled_rows() {
        let req = SavedRequest {
            url: "http://h".into(),
            headers: vec![
                ("A".into(), "1".into(), true),
                ("B".into(), "2".into(), false),
                (String::new(), "x".into(), true),
            ],
            ..Default::default()
        };
        assert_eq!(
            RequestSpec::from_saved(&req).headers,
            vec![("A".into(), "1".into())]
        );
    }

    #[test]
    fn send_blocking_reports_connection_errors() {
        // Port 9 (discard) di localhost hampir pasti menolak koneksi.
        let resp = send_blocking(RequestSpec {
            url: "http://127.0.0.1:9/".into(),
            ..Default::default()
        });
        assert_eq!(resp.status, 0);
        assert!(resp.error.is_some());
    }
}
