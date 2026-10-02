//! HTTP yang dijalankan host atas nama plugin Wasm.
//!
//! Plugin tidak punya socket. Ia meminta host menjalankan request, dan host
//! menolak URL di luar allowlist manifest (mencegah SSRF/exfiltration).
//! Untuk hasil besar, plugin bisa mengembalikan [`HttpTableRequest`]: host
//! menjalankan request dan mem-parse format tabel standar secara native,
//! sehingga baris tidak perlu melewati interpreter Wasm.

use super::{DriverError, DriverResult, ExecuteOutput};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_ERROR_BODY: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn default_method() -> String {
    "GET".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Format respons tabel yang di-parse host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableFormat {
    /// `{"meta":[{"name":..}], "data":[[..]]}` (ClickHouse `JSONCompact`).
    JsonCompact,
    /// Baris pertama nama kolom, dipisah tab, escape `\t \n \\`, NULL = `\N`.
    TsvWithNames,
    /// CSV RFC 4180 dengan baris header.
    CsvWithNames,
    /// Satu objek JSON per baris; kolom mengikuti urutan kunci kemunculan.
    Ndjson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpTableRequest {
    pub request: HttpRequest,
    pub format: TableFormat,
}

/// Allowlist host HTTP dari manifest, dengan `{connection.host}` sudah
/// diganti host efektif koneksi.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpPolicy {
    patterns: Vec<String>,
}

impl HttpPolicy {
    pub fn new(patterns: &[String], connection_host: &str) -> Self {
        let patterns = patterns
            .iter()
            .map(|p| {
                p.replace("{connection.host}", connection_host)
                    .trim()
                    .to_ascii_lowercase()
            })
            .filter(|p| !p.is_empty())
            .collect();
        Self { patterns }
    }

    /// Cek URL: skema harus http/https dan host cocok dengan salah satu pola
    /// (`host` persis atau `*.domain`).
    pub fn check(&self, raw_url: &str) -> DriverResult<url::Url> {
        let url = url::Url::parse(raw_url)
            .map_err(|e| DriverError::Permission(format!("invalid URL '{raw_url}': {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(DriverError::Permission(format!(
                "scheme '{}' is not allowed",
                url.scheme()
            )));
        }
        let host = url
            .host_str()
            .ok_or_else(|| DriverError::Permission("URL has no host".into()))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        let allowed = self.patterns.iter().any(|p| match p.strip_prefix("*.") {
            Some(suffix) => host.ends_with(&format!(".{suffix}")),
            None => host == *p,
        });
        if allowed {
            Ok(url)
        } else {
            Err(DriverError::Permission(format!(
                "host '{host}' is not in the plugin's allowed hosts"
            )))
        }
    }
}

fn client(timeout: Duration) -> DriverResult<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| DriverError::Plugin(format!("HTTP client: {e}")))
}

fn send(policy: &HttpPolicy, req: &HttpRequest) -> DriverResult<reqwest::blocking::Response> {
    let url = policy.check(&req.url)?;
    let method = reqwest::Method::from_bytes(req.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| DriverError::Protocol(format!("invalid HTTP method '{}'", req.method)))?;
    let timeout = req
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TIMEOUT);
    let mut builder = client(timeout)?.request(method, url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    if let Some(body) = &req.body {
        builder = builder.body(body.clone());
    }
    log::debug!("[DRIVER-PLUGIN] HTTP {} {}", req.method, redact_url(&req.url));
    builder
        .send()
        .map_err(|e| DriverError::Connect(format!("HTTP request failed: {e}")))
}

/// Hilangkan kredensial dan query string dari URL untuk log.
fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut u) => {
            let _ = u.set_password(None);
            let _ = u.set_username("");
            u.set_query(None);
            u.to_string()
        }
        Err(_) => "<invalid url>".to_string(),
    }
}

fn error_from_status(status: u16, body: &str) -> DriverError {
    let mut text = body.trim().to_string();
    if text.len() > MAX_ERROR_BODY {
        let cut = (0..=MAX_ERROR_BODY)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        text.truncate(cut);
        text.push('…');
    }
    DriverError::Query(format!("HTTP {status}: {text}"))
}

/// Jalankan request dan kembalikan respons mentah ke plugin.
pub fn execute(policy: &HttpPolicy, req: &HttpRequest) -> DriverResult<HttpResponse> {
    let resp = send(policy, req)?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let body = resp
        .text()
        .map_err(|e| DriverError::Connect(format!("HTTP body: {e}")))?;
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// Jalankan request dan parse tabelnya di host.
pub fn execute_table(
    policy: &HttpPolicy,
    table: &HttpTableRequest,
    max_rows: usize,
) -> DriverResult<ExecuteOutput> {
    let resp = send(policy, &table.request)?;
    let status = resp.status().as_u16();
    let body = resp
        .text()
        .map_err(|e| DriverError::Connect(format!("HTTP body: {e}")))?;
    if status >= 400 {
        return Err(error_from_status(status, &body));
    }
    parse_table(table.format, &body, max_rows)
}

pub fn parse_table(
    format: TableFormat,
    body: &str,
    max_rows: usize,
) -> DriverResult<ExecuteOutput> {
    let cap = if max_rows == 0 { usize::MAX } else { max_rows };
    match format {
        TableFormat::JsonCompact => parse_json_compact(body, cap),
        TableFormat::TsvWithNames => Ok(parse_tsv(body, cap)),
        TableFormat::CsvWithNames => parse_csv(body, cap),
        TableFormat::Ndjson => parse_ndjson(body, cap),
    }
}

fn json_cell(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

fn parse_json_compact(body: &str, cap: usize) -> DriverResult<ExecuteOutput> {
    #[derive(Deserialize)]
    struct Meta {
        name: String,
    }
    #[derive(Deserialize)]
    struct Compact {
        #[serde(default)]
        meta: Vec<Meta>,
        #[serde(default)]
        data: Vec<Vec<serde_json::Value>>,
    }
    if body.trim().is_empty() {
        return Ok(ExecuteOutput::default());
    }
    let parsed: Compact = serde_json::from_str(body)
        .map_err(|e| DriverError::Protocol(format!("invalid JSONCompact response: {e}")))?;
    let truncated = parsed.data.len() > cap;
    Ok(ExecuteOutput {
        headers: parsed.meta.into_iter().map(|m| m.name).collect(),
        rows: parsed
            .data
            .iter()
            .take(cap)
            .map(|r| r.iter().map(json_cell).collect())
            .collect(),
        affected_rows: None,
        truncated,
    })
}

fn unescape_tsv(field: &str) -> Option<String> {
    if field == "\\N" {
        return None;
    }
    if !field.contains('\\') {
        return Some(field.to_string());
    }
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    Some(out)
}

fn parse_tsv(body: &str, cap: usize) -> ExecuteOutput {
    let mut lines = body.lines();
    let headers: Vec<String> = match lines.next() {
        Some(h) if !h.is_empty() => h
            .split('\t')
            .map(|f| unescape_tsv(f).unwrap_or_default())
            .collect(),
        _ => return ExecuteOutput::default(),
    };
    let mut rows = Vec::new();
    let mut truncated = false;
    for line in lines {
        if rows.len() == cap {
            truncated = true;
            break;
        }
        rows.push(line.split('\t').map(unescape_tsv).collect());
    }
    ExecuteOutput {
        headers,
        rows,
        affected_rows: None,
        truncated,
    }
}

fn parse_csv(body: &str, cap: usize) -> DriverResult<ExecuteOutput> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(body.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| DriverError::Protocol(format!("invalid CSV header: {e}")))?
        .iter()
        .map(str::to_string)
        .collect();
    let mut rows = Vec::new();
    let mut truncated = false;
    for record in reader.records() {
        if rows.len() == cap {
            truncated = true;
            break;
        }
        let record = record.map_err(|e| DriverError::Protocol(format!("invalid CSV: {e}")))?;
        rows.push(record.iter().map(|f| Some(f.to_string())).collect());
    }
    Ok(ExecuteOutput {
        headers,
        rows,
        affected_rows: None,
        truncated,
    })
}

/// Kunci objek JSON sesuai urutan kemunculan di teks (tanpa bergantung pada
/// fitur `preserve_order` serde_json).
fn ordered_keys(line: &str) -> Vec<String> {
    struct KeyCollector<'a>(&'a mut Vec<String>);
    impl<'de> serde::de::Visitor<'de> for KeyCollector<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
            while let Some(k) = map.next_key::<String>()? {
                map.next_value::<serde::de::IgnoredAny>()?;
                self.0.push(k);
            }
            Ok(())
        }
    }
    let mut keys = Vec::new();
    let mut de = serde_json::Deserializer::from_str(line);
    let _ = serde::Deserializer::deserialize_map(&mut de, KeyCollector(&mut keys));
    keys
}

fn parse_ndjson(body: &str, cap: usize) -> DriverResult<ExecuteOutput> {
    let mut headers: Vec<String> = Vec::new();
    let mut objects = Vec::new();
    let mut truncated = false;
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        if objects.len() == cap {
            truncated = true;
            break;
        }
        let obj: serde_json::Map<String, serde_json::Value> = serde_json::from_str(line)
            .map_err(|e| DriverError::Protocol(format!("invalid NDJSON line: {e}")))?;
        for k in ordered_keys(line) {
            if !headers.contains(&k) {
                headers.push(k);
            }
        }
        objects.push(obj);
    }
    let rows = objects
        .iter()
        .map(|obj| {
            headers
                .iter()
                .map(|h| obj.get(h).and_then(json_cell))
                .collect()
        })
        .collect();
    Ok(ExecuteOutput {
        headers,
        rows,
        affected_rows: None,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_allows_only_listed_hosts() {
        let p = HttpPolicy::new(
            &["{connection.host}".into(), "*.clickhouse.cloud".into()],
            "db.internal",
        );
        assert!(p.check("http://db.internal:8123/?query=1").is_ok());
        assert!(p.check("https://abc.eu.clickhouse.cloud/").is_ok());
        assert!(p.check("https://clickhouse.cloud.evil.example/").is_err());
        assert!(p.check("https://evil.example/").is_err());
        assert!(p.check("ftp://db.internal/").is_err());
        assert!(p.check("not a url").is_err());
    }

    #[test]
    fn empty_policy_denies_everything() {
        let p = HttpPolicy::new(&[], "db.internal");
        assert!(p.check("http://db.internal/").is_err());
    }

    #[test]
    fn parses_json_compact_with_nulls_and_cap() {
        let body = r#"{"meta":[{"name":"id","type":"UInt64"},{"name":"v","type":"Nullable(String)"}],
                       "data":[["1","a"],["2",null],["3","c"]],"rows":3}"#;
        let out = parse_table(TableFormat::JsonCompact, body, 2).unwrap();
        assert_eq!(out.headers, vec!["id", "v"]);
        assert_eq!(out.rows[1], vec![Some("2".into()), None]);
        assert!(out.truncated);
    }

    #[test]
    fn parses_tsv_escapes() {
        let body = "a\tb\nx\\ty\t\\N\nline\\nbreak\tz\n";
        let out = parse_table(TableFormat::TsvWithNames, body, 0).unwrap();
        assert_eq!(out.headers, vec!["a", "b"]);
        assert_eq!(out.rows[0], vec![Some("x\ty".into()), None]);
        assert_eq!(
            out.rows[1],
            vec![Some("line\nbreak".into()), Some("z".into())]
        );
        assert!(!out.truncated);
    }

    #[test]
    fn parses_csv_and_ndjson() {
        let out = parse_table(TableFormat::CsvWithNames, "a,b\n1,\"x,y\"\n", 0).unwrap();
        assert_eq!(out.rows[0], vec![Some("1".into()), Some("x,y".into())]);

        let body = "{\"z\":1,\"a\":null}\n{\"z\":2,\"a\":\"k\",\"extra\":true}\n";
        let out = parse_table(TableFormat::Ndjson, body, 0).unwrap();
        assert_eq!(out.headers, vec!["z", "a", "extra"]);
        assert_eq!(out.rows[0], vec![Some("1".into()), None, None]);
        assert_eq!(out.rows[1][2], Some("true".into()));
    }

    #[test]
    fn redacts_credentials_in_urls() {
        let r = redact_url("https://user:pw@host:8443/path?token=x");
        assert!(!r.contains("pw@"));
        assert!(!r.contains("token"));
        assert!(r.contains("host:8443/path"));
    }

    #[test]
    fn error_body_is_cut_on_char_boundary() {
        let body = "é".repeat(5000);
        let DriverError::Query(msg) = error_from_status(500, &body) else {
            panic!("expected query error");
        };
        assert!(msg.len() < 4200);
        assert!(msg.ends_with('…'));
    }
}
