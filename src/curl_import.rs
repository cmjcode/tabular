//! Parse a pasted/imported `curl` command line and apply it onto an
//! `HttpClientState`, so a user can populate a REST request by pasting a
//! curl command (e.g. copied from browser devtools, Postman, or a README)
//! instead of filling in method/URL/headers/body by hand.

use crate::models::structs::{HttpAuthType, HttpBodyType, HttpClientState, HttpMethod};

/// Heuristic: does `text` look like a curl command rather than a plain URL?
/// Used to auto-convert a curl command pasted directly into the URL field.
/// Requires the literal word `curl` followed by whitespace (or end of input)
/// at the start, so real URLs like `https://curl.se/...` are never matched.
pub fn looks_like_curl(text: &str) -> bool {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed
        .get(..4)
        .filter(|head| head.eq_ignore_ascii_case("curl"))
        .map(|_| &trimmed[4..])
    else {
        return false;
    };
    rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace())
}

/// Parses `raw` as a curl command and overwrites the request-related fields
/// of `state` (url, method, params, headers, body, auth). The previous
/// response is cleared since this represents a new request.
///
/// Returns non-fatal warnings on success (e.g. unsupported flags that were
/// skipped), or a fatal error string if no URL could be found at all.
pub fn apply_to_state(state: &mut HttpClientState, raw: &str) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    let normalized = normalize_continuations(raw);
    let tokens = tokenize(&normalized)?;
    let mut it = tokens.into_iter().peekable();

    if let Some(first) = it.peek()
        && first.eq_ignore_ascii_case("curl")
    {
        it.next();
    }

    let mut method: Option<String> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut data_parts: Vec<String> = Vec::new();
    // `--data-urlencode key=value` -> Some(key); plain content chunk -> None
    let mut data_urlencode_parts: Vec<(Option<String>, String)> = Vec::new();
    let mut form_parts: Vec<(String, String)> = Vec::new();
    let mut basic_auth: Option<(String, String)> = None;
    let mut force_get_with_query = false;
    let mut force_head = false;

    const BOOL_FLAGS: &[&str] = &[
        "-k", "--insecure", "-s", "--silent", "-S", "--show-error", "-v", "--verbose",
        "-L", "--location", "--compressed", "-i", "--include", "-f", "--fail",
        "-N", "--no-buffer", "-0", "--http1.0", "--http1.1", "--http2",
        "--http2-prior-knowledge", "-1", "--tlsv1", "-4", "--ipv4", "-6", "--ipv6",
        "-#", "--progress-bar", "-g", "--globoff", "-n", "--netrc", "--ssl", "--ssl-reqd",
    ];
    const IGNORED_VALUE_FLAGS: &[&str] = &[
        "-o", "--output", "-D", "--dump-header", "-c", "--cookie-jar", "-T",
        "--upload-file", "-x", "--proxy", "-w", "--write-out", "--connect-timeout",
        "-m", "--max-time", "--retry", "--limit-rate", "--cacert", "--cert", "--key",
        "-E", "--interface", "--resolve",
    ];

    while let Some(tok) = it.next() {
        match tok.as_str() {
            "-X" | "--request" => method = it.next(),
            "-H" | "--header" => {
                if let Some(h) = it.next()
                    && let Some((k, v)) = h.split_once(':')
                {
                    headers.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                if let Some(v) = it.next() {
                    data_parts.push(v);
                }
            }
            "--data-urlencode" => {
                if let Some(v) = it.next() {
                    if let Some(rest) = v.strip_prefix('=') {
                        data_urlencode_parts.push((None, rest.to_string()));
                    } else if v.strip_prefix('@').is_some() {
                        warnings.push(
                            "A --data-urlencode file upload was skipped (can't be attached automatically)"
                                .to_string(),
                        );
                    } else if let Some((k, rest)) = v.split_once('=') {
                        data_urlencode_parts.push((Some(k.to_string()), rest.to_string()));
                    } else {
                        data_urlencode_parts.push((None, v));
                    }
                }
            }
            "-F" | "--form" => {
                if let Some(v) = it.next()
                    && let Some((k, val)) = v.split_once('=')
                {
                    if val.starts_with('@') {
                        warnings.push(format!(
                            "Form field \"{k}\" references a file ({val}) that couldn't be attached automatically"
                        ));
                    }
                    form_parts.push((k.to_string(), val.to_string()));
                }
            }
            "-u" | "--user" => {
                if let Some(v) = it.next() {
                    match v.split_once(':') {
                        Some((u, p)) => basic_auth = Some((u.to_string(), p.to_string())),
                        None => basic_auth = Some((v, String::new())),
                    }
                }
            }
            "-A" | "--user-agent" => {
                if let Some(v) = it.next() {
                    headers.push(("User-Agent".to_string(), v));
                }
            }
            "-b" | "--cookie" => {
                if let Some(v) = it.next() {
                    headers.push(("Cookie".to_string(), v));
                }
            }
            "-e" | "--referer" => {
                if let Some(v) = it.next() {
                    headers.push(("Referer".to_string(), v));
                }
            }
            "--url" => url = it.next(),
            "-G" | "--get" => force_get_with_query = true,
            "-I" | "--head" => force_head = true,
            _ if BOOL_FLAGS.contains(&tok.as_str()) => {}
            _ if IGNORED_VALUE_FLAGS.contains(&tok.as_str()) => {
                it.next();
            }
            // Unrecognized flag: not meaningful to this app's request model, drop silently.
            _ if tok.starts_with('-') => {}
            _ => {
                // First bare token is the URL; anything after that (e.g. a second
                // positional some curl builders emit) isn't representable here.
                if url.is_none() {
                    url = Some(tok);
                }
            }
        }
    }

    let mut url = url.ok_or_else(|| "No URL found in curl command".to_string())?;

    // `-G`/`--get` moves any data payload onto the query string instead of the body.
    if force_get_with_query {
        let mut joined = data_parts.join("&");
        for (k, v) in &data_urlencode_parts {
            if !joined.is_empty() {
                joined.push('&');
            }
            match k {
                Some(k) => joined.push_str(&format!("{k}={v}")),
                None => joined.push_str(v),
            }
        }
        if !joined.is_empty() {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&joined);
        }
        data_parts.clear();
        data_urlencode_parts.clear();
    }

    let method_enum = if force_head {
        HttpMethod::HEAD
    } else if let Some(m) = method {
        match m.to_uppercase().as_str() {
            "GET" => HttpMethod::GET,
            "POST" => HttpMethod::POST,
            "PUT" => HttpMethod::PUT,
            "DELETE" => HttpMethod::DELETE,
            "PATCH" => HttpMethod::PATCH,
            "HEAD" => HttpMethod::HEAD,
            "OPTIONS" => HttpMethod::OPTIONS,
            // Unrecognized method: fall back to GET rather than blocking the import.
            _ => HttpMethod::GET,
        }
    } else if !data_parts.is_empty() || !data_urlencode_parts.is_empty() || !form_parts.is_empty() {
        HttpMethod::POST
    } else {
        HttpMethod::GET
    };

    let content_type = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.to_lowercase());

    let (base_url, url_params) = split_url_and_params(&url);

    // ── Apply everything onto state ──────────────────────────────────────
    state.url = base_url;
    state.method = method_enum;
    state.params = if url_params.is_empty() {
        vec![(String::new(), String::new(), true)]
    } else {
        url_params
    };
    state.headers = if headers.is_empty() {
        vec![("Accept".to_string(), "*/*".to_string(), true)]
    } else {
        headers.iter().map(|(k, v)| (k.clone(), v.clone(), true)).collect()
    };

    if !form_parts.is_empty() {
        state.body_type = HttpBodyType::MultiPart;
        state.form_data = form_parts.into_iter().map(|(k, v)| (k, v, true)).collect();
        state.body_text.clear();
    } else if data_urlencode_parts.iter().any(|(k, _)| k.is_some()) {
        state.body_type = HttpBodyType::UrlEncoded;
        state.form_data = data_urlencode_parts
            .into_iter()
            .map(|(k, v)| (k.unwrap_or_default(), v, true))
            .collect();
        state.body_text.clear();
    } else if !data_parts.is_empty() || !data_urlencode_parts.is_empty() {
        let mut body = data_parts.join("&");
        for (_, v) in data_urlencode_parts {
            if !body.is_empty() {
                body.push('&');
            }
            body.push_str(&v);
        }
        state.body_type = sniff_body_type(content_type.as_deref(), &body);
        state.body_text = body;
        state.form_data = vec![(String::new(), String::new(), true)];
    } else {
        state.body_type = HttpBodyType::NoBody;
        state.body_text.clear();
        state.form_data = vec![(String::new(), String::new(), true)];
    }

    if let Some((user, pass)) = basic_auth {
        state.auth_type = HttpAuthType::BasicAuth;
        state.basic_user = user;
        state.basic_pass = pass;
    } else if let Some(idx) = state
        .headers
        .iter()
        .position(|(k, v, _)| k.eq_ignore_ascii_case("authorization") && v.to_lowercase().starts_with("bearer "))
    {
        let (_, v, _) = state.headers.remove(idx);
        state.auth_type = HttpAuthType::BearerToken;
        state.bearer_token = v[7..].trim().to_string();
    } else {
        state.auth_type = HttpAuthType::NoAuth;
    }

    // A freshly imported request supersedes whatever response was showing before.
    state.response_status = None;
    state.response_status_text.clear();
    state.response_body.clear();
    state.response_headers.clear();
    state.response_time_ms = None;
    state.response_size_bytes = None;
    state.response_error = None;

    Ok(warnings)
}

fn sniff_body_type(content_type: Option<&str>, body: &str) -> HttpBodyType {
    if let Some(ct) = content_type {
        if ct.contains("json") {
            return HttpBodyType::Json;
        }
        if ct.contains("graphql") {
            return HttpBodyType::GraphQL;
        }
        if ct.contains("xml") {
            return HttpBodyType::Xml;
        }
    }
    let trimmed = body.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        HttpBodyType::Json
    } else if trimmed.starts_with('<') {
        HttpBodyType::Xml
    } else {
        HttpBodyType::OtherText
    }
}

/// Splits a URL into its base (no query string) and its query parameters.
/// Uses `reqwest::Url` (already a dependency) for correct percent-decoding;
/// falls back to a naive split for URLs that don't parse as absolute
/// (e.g. templated URLs like `{{baseUrl}}/path`).
fn split_url_and_params(raw_url: &str) -> (String, Vec<(String, String, bool)>) {
    if let Ok(mut u) = reqwest::Url::parse(raw_url) {
        let params: Vec<(String, String, bool)> = u
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned(), true))
            .collect();
        u.set_query(None);
        return (u.to_string(), params);
    }

    match raw_url.find('?') {
        Some(idx) => {
            let base = raw_url[..idx].to_string();
            let params = raw_url[idx + 1..]
                .split('&')
                .filter(|s| !s.is_empty())
                .map(|pair| match pair.split_once('=') {
                    Some((k, v)) => (k.to_string(), v.to_string(), true),
                    None => (pair.to_string(), String::new(), true),
                })
                .collect();
            (base, params)
        }
        None => (raw_url.to_string(), Vec::new()),
    }
}

/// Joins bash (`\` at end-of-line) and best-effort cmd.exe (`^` at end-of-line)
/// line continuations into a single line, and normalizes CRLF to LF, so a
/// curl command copied across multiple lines parses as one command.
fn normalize_continuations(input: &str) -> String {
    let mut out = String::new();
    for line in input.replace("\r\n", "\n").split('\n') {
        let trimmed_end = line.trim_end();
        if let Some(stripped) = trimmed_end.strip_suffix('\\') {
            out.push_str(stripped.trim_end());
            out.push(' ');
        } else if let Some(stripped) = trimmed_end.strip_suffix('^') {
            out.push_str(stripped.trim_end());
            out.push(' ');
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Minimal POSIX-shell-like word tokenizer: honors `'...'` (fully literal)
/// and `"..."` (backslash-escaped) quoting, and backslash-escapes outside
/// quotes. Sufficient for curl commands copied from browser devtools,
/// Postman, or documentation — not a full shell grammar.
fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err("Unterminated single quote in curl command".to_string()),
                    }
                }
            }
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(next) if matches!(next, '"' | '\\' | '$' | '`') => current.push(next),
                            Some(next) => {
                                current.push('\\');
                                current.push(next);
                            }
                            None => return Err("Unterminated double quote in curl command".to_string()),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err("Unterminated double quote in curl command".to_string()),
                    }
                }
            }
            '\\' => {
                match chars.next() {
                    // A backslash immediately followed by whitespace is (almost
                    // always) a line-continuation marker whose paired newline was
                    // already stripped upstream — e.g. pasting a multi-line curl
                    // command into a singleline widget collapses the newline but
                    // leaves the `\`. Treat it as a token boundary, not an escaped
                    // space, otherwise it corrupts the next flag's token.
                    Some(next) if next.is_whitespace() => {
                        if in_token {
                            tokens.push(std::mem::take(&mut current));
                            in_token = false;
                        }
                    }
                    Some(next) => {
                        in_token = true;
                        current.push(next);
                    }
                    None => {
                        in_token = true;
                        current.push('\\');
                    }
                }
            }
            _ => {
                in_token = true;
                current.push(c);
            }
        }
    }

    if in_token {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> HttpClientState {
        HttpClientState::default()
    }

    #[test]
    fn parses_simple_get() {
        let mut state = fresh();
        let warnings = apply_to_state(&mut state, "curl https://api.example.com/users").unwrap();
        assert!(warnings.is_empty());
        assert_eq!(state.url, "https://api.example.com/users");
        assert_eq!(state.method, HttpMethod::GET);
        assert_eq!(state.body_type, HttpBodyType::NoBody);
    }

    #[test]
    fn parses_post_with_json_body_and_headers() {
        let mut state = fresh();
        let cmd = r#"curl -X POST https://api.example.com/users -H "Content-Type: application/json" -H "Accept: application/json" -d '{"name":"Jayuda"}'"#;
        apply_to_state(&mut state, cmd).unwrap();
        assert_eq!(state.method, HttpMethod::POST);
        assert_eq!(state.url, "https://api.example.com/users");
        assert_eq!(state.body_type, HttpBodyType::Json);
        assert_eq!(state.body_text, r#"{"name":"Jayuda"}"#);
        assert!(state.headers.iter().any(|(k, v, _)| k == "Content-Type" && v == "application/json"));
        assert!(state.headers.iter().any(|(k, v, _)| k == "Accept" && v == "application/json"));
    }

    #[test]
    fn infers_post_from_data_without_explicit_method() {
        let mut state = fresh();
        apply_to_state(&mut state, "curl https://api.example.com/x -d 'a=1'").unwrap();
        assert_eq!(state.method, HttpMethod::POST);
    }

    #[test]
    fn parses_query_params_from_url() {
        let mut state = fresh();
        apply_to_state(&mut state, "curl 'https://api.example.com/search?q=rust&page=2'").unwrap();
        assert_eq!(state.url, "https://api.example.com/search");
        assert!(state.params.iter().any(|(k, v, _)| k == "q" && v == "rust"));
        assert!(state.params.iter().any(|(k, v, _)| k == "page" && v == "2"));
    }

    #[test]
    fn parses_form_urlencoded_body() {
        let mut state = fresh();
        apply_to_state(
            &mut state,
            "curl https://api.example.com/x -d 'a=1' -d 'b=2'",
        )
        .unwrap();
        assert_eq!(state.body_type, HttpBodyType::OtherText);
        assert_eq!(state.body_text, "a=1&b=2");
    }

    #[test]
    fn parses_multipart_form() {
        let mut state = fresh();
        apply_to_state(
            &mut state,
            r#"curl https://api.example.com/upload -F "name=Jayuda" -F "file=@photo.png""#,
        )
        .unwrap();
        assert_eq!(state.body_type, HttpBodyType::MultiPart);
        assert!(state.form_data.iter().any(|(k, v, _)| k == "name" && v == "Jayuda"));
        assert!(state.form_data.iter().any(|(k, v, _)| k == "file" && v == "@photo.png"));
    }

    #[test]
    fn parses_basic_auth() {
        let mut state = fresh();
        apply_to_state(&mut state, "curl -u alice:secret https://api.example.com/x").unwrap();
        assert_eq!(state.auth_type, HttpAuthType::BasicAuth);
        assert_eq!(state.basic_user, "alice");
        assert_eq!(state.basic_pass, "secret");
    }

    #[test]
    fn extracts_bearer_token_header_into_auth() {
        let mut state = fresh();
        apply_to_state(
            &mut state,
            r#"curl https://api.example.com/x -H "Authorization: Bearer abc123""#,
        )
        .unwrap();
        assert_eq!(state.auth_type, HttpAuthType::BearerToken);
        assert_eq!(state.bearer_token, "abc123");
        assert!(!state.headers.iter().any(|(k, _, _)| k.eq_ignore_ascii_case("authorization")));
    }

    #[test]
    fn handles_multiline_backslash_continued_command() {
        let mut state = fresh();
        let cmd = "curl -X POST 'https://api.example.com/x' \\\n  -H 'Content-Type: application/json' \\\n  -d '{\"a\":1}'";
        apply_to_state(&mut state, cmd).unwrap();
        assert_eq!(state.method, HttpMethod::POST);
        assert_eq!(state.body_type, HttpBodyType::Json);
    }

    #[test]
    fn errors_when_no_url_present() {
        let mut state = fresh();
        let err = apply_to_state(&mut state, "curl -X GET").unwrap_err();
        assert!(err.contains("No URL"));
    }

    #[test]
    fn looks_like_curl_matches_curl_commands_only() {
        assert!(looks_like_curl("curl https://api.example.com/x"));
        assert!(looks_like_curl("  curl -X POST https://api.example.com/x"));
        assert!(looks_like_curl("CURL https://api.example.com/x"));
        assert!(!looks_like_curl("https://curl.se/docs"));
        assert!(!looks_like_curl("curlish.example.com"));
        assert!(!looks_like_curl(""));
    }

    #[test]
    fn unsupported_flag_is_silently_dropped_not_fatal() {
        let mut state = fresh();
        let warnings = apply_to_state(&mut state, "curl --some-unknown-flag https://api.example.com/x").unwrap();
        assert!(warnings.is_empty());
        assert_eq!(state.url, "https://api.example.com/x");
    }

    #[test]
    fn multiline_pasted_into_singleline_field_has_no_stray_tokens() {
        // Simulates what a singleline egui::TextEdit hands back after a
        // multi-line bash curl command is pasted into it: the widget strips
        // the '\n' characters itself, leaving the line-continuation
        // backslashes dangling next to a plain space.
        let mut state = fresh();
        let pasted = "curl 'https://api.example.com/x' \\   -H 'accept: */*' \\   -H 'content-type: application/json' \\   -d '{\"a\":1}'";
        let warnings = apply_to_state(&mut state, pasted).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(state.url, "https://api.example.com/x");
        assert!(state.headers.iter().any(|(k, v, _)| k == "accept" && v == "*/*"));
        assert!(state.headers.iter().any(|(k, v, _)| k == "content-type" && v == "application/json"));
        assert_eq!(state.body_text, "{\"a\":1}");
    }
}
