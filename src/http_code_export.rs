//! Generate ready-to-run source code (curl / Python / JavaScript / Node.js /
//! Go / PHP / Rust) that reproduces the request currently built in the HTTP
//! client panel, for the "Copy as Code" dialog. This is the mirror of
//! [`crate::curl_import`]: that module turns pasted text into an
//! `HttpClientState`, this one turns an `HttpClientState` into text.

use crate::models::structs::{CodeLang, HttpAuthType, HttpBodyType, HttpClientState};

/// Generate source code for `state` in the given target `lang`.
pub fn generate(lang: &CodeLang, state: &HttpClientState) -> String {
    let export = build_export(state);
    match lang {
        CodeLang::Curl => to_curl(&export),
        CodeLang::Python => to_python(&export),
        CodeLang::JavaScript => to_javascript(&export),
        CodeLang::NodeJs => to_nodejs(&export),
        CodeLang::Go => to_go(&export),
        CodeLang::Php => to_php(&export),
        CodeLang::Rust => to_rust(&export),
    }
}

// ─── Intermediate representation ─────────────────────────────────────────────
//
// Every language renderer consumes this instead of `HttpClientState`
// directly, so the "what does this request actually send" logic (enabled
// rows only, auth → headers/query/basic-auth split, body → content-type)
// lives in one place and matches what `execute_request` in `http_client.rs`
// really does over the wire.

struct ReqExport {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    basic_auth: Option<(String, String)>,
    body: BodyExport,
    unsupported_auth_note: Option<&'static str>,
}

enum BodyExport {
    None,
    Raw { content_type: String, text: String },
    Form(Vec<(String, String)>),
    Multipart(Vec<(String, String)>),
    Unsupported(&'static str),
}

impl BodyExport {
    /// The `Content-Type` this body implies, if any isn't already user-set.
    /// `Multipart` deliberately returns `None` — every HTTP library in these
    /// generators sets the multipart boundary header itself.
    fn content_type(&self) -> Option<&str> {
        match self {
            BodyExport::Raw { content_type, .. } if !content_type.is_empty() => {
                Some(content_type)
            }
            BodyExport::Form(_) => Some("application/x-www-form-urlencoded"),
            _ => None,
        }
    }
}

fn enabled_pairs(rows: &[(String, String, bool)]) -> Vec<(String, String)> {
    rows.iter()
        .filter(|(k, _, en)| *en && !k.is_empty())
        .map(|(k, v, _)| (k.clone(), v.clone()))
        .collect()
}

fn build_export(state: &HttpClientState) -> ReqExport {
    let mut headers = enabled_pairs(&state.headers);
    let mut query = enabled_pairs(&state.params);
    let mut basic_auth = None;
    let mut unsupported_auth_note = None;

    match &state.auth_type {
        HttpAuthType::BearerToken | HttpAuthType::JwtBearer => {
            headers.push(("Authorization".to_string(), format!("Bearer {}", state.bearer_token)));
        }
        HttpAuthType::BasicAuth => {
            basic_auth = Some((state.basic_user.clone(), state.basic_pass.clone()));
        }
        HttpAuthType::ApiKey => {
            if !state.api_key_name.is_empty() {
                if state.api_key_in_header {
                    headers.push((state.api_key_name.clone(), state.api_key_value.clone()));
                } else {
                    query.push((state.api_key_name.clone(), state.api_key_value.clone()));
                }
            }
        }
        HttpAuthType::OAuth1 => unsupported_auth_note = Some("OAuth 1.0"),
        HttpAuthType::OAuth2 => unsupported_auth_note = Some("OAuth 2.0"),
        HttpAuthType::AwsSignature => unsupported_auth_note = Some("AWS Signature"),
        HttpAuthType::NtlmAuth => unsupported_auth_note = Some("NTLM"),
        HttpAuthType::InheritParent | HttpAuthType::NoAuth => {}
    }

    let mut url = state.url.clone();
    if !query.is_empty() {
        let qs: String = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&qs);
    }

    let body = match &state.body_type {
        HttpBodyType::NoBody => BodyExport::None,
        HttpBodyType::BinaryFile => {
            BodyExport::Unsupported("Binary file upload is not yet supported by this app")
        }
        HttpBodyType::UrlEncoded => BodyExport::Form(enabled_pairs(&state.form_data)),
        HttpBodyType::MultiPart => BodyExport::Multipart(enabled_pairs(&state.form_data)),
        HttpBodyType::Json => BodyExport::Raw {
            content_type: "application/json".to_string(),
            text: state.body_text.clone(),
        },
        HttpBodyType::GraphQL => BodyExport::Raw {
            content_type: "application/json".to_string(),
            text: state.body_text.clone(),
        },
        HttpBodyType::Xml => BodyExport::Raw {
            content_type: "application/xml".to_string(),
            text: state.body_text.clone(),
        },
        HttpBodyType::OtherText => BodyExport::Raw {
            content_type: String::new(),
            text: state.body_text.clone(),
        },
    };

    ReqExport {
        method: state.method.label().to_string(),
        url,
        headers,
        basic_auth,
        body,
        unsupported_auth_note,
    }
}

/// Custom headers plus the body's implied `Content-Type`, unless the user
/// already set one explicitly.
fn effective_headers(export: &ReqExport) -> Vec<(String, String)> {
    let mut headers = export.headers.clone();
    if let Some(ct) = export.body.content_type()
        && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("Content-Type".to_string(), ct.to_string()));
    }
    headers
}

fn header_entries(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .map(|(k, v)| format!("{}: {},", quoted(k), quoted(v)))
        .collect()
}

fn auth_comment(export: &ReqExport, comment_prefix: &str) -> Option<String> {
    export.unsupported_auth_note.map(|kind| {
        format!("{comment_prefix} NOTE: {kind} authentication is not yet supported by this generator")
    })
}

// ─── String-literal escaping helpers ─────────────────────────────────────────

/// A double-quoted string literal using JSON escaping rules — valid source
/// for JavaScript, Go, Rust, and Python (double-quoted strings).
fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
}

/// A single-quoted shell literal, safe to paste into a curl command.
fn sh_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A single-quoted PHP string literal. PHP double-quoted strings interpolate
/// `$variables`, so single-quoting is the safe default for arbitrary content.
fn php_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A Go string literal. Prefers a raw backtick string (readable for JSON/XML
/// bodies); falls back to a JSON-escaped double-quoted literal when the
/// content itself contains a backtick or carriage return.
fn go_string_literal(s: &str) -> String {
    if !s.contains('`') && !s.contains('\r') {
        format!("`{s}`")
    } else {
        quoted(s)
    }
}

/// A Rust string literal. Prefers a raw string (`r#"..."#`, readable for
/// JSON/XML bodies); falls back to a JSON-escaped double-quoted literal
/// (valid Rust escaping too) when the content contains `"#`.
fn rust_string_literal(s: &str) -> String {
    if !s.contains("\"#") {
        format!("r#\"{s}\"#")
    } else {
        quoted(s)
    }
}

// ─── curl ─────────────────────────────────────────────────────────────────

fn to_curl(export: &ReqExport) -> String {
    let mut out = format!("curl -X {} {}", export.method, sh_quoted(&export.url));

    for (k, v) in effective_headers(export) {
        out.push_str(&format!(" \\\n  -H {}", sh_quoted(&format!("{k}: {v}"))));
    }

    if let Some((user, pass)) = &export.basic_auth {
        out.push_str(&format!(" \\\n  -u {}", sh_quoted(&format!("{user}:{pass}"))));
    }

    match &export.body {
        BodyExport::None => {}
        BodyExport::Raw { text, .. } => {
            if !text.is_empty() {
                out.push_str(&format!(" \\\n  -d {}", sh_quoted(text)));
            }
        }
        BodyExport::Form(pairs) => {
            for (k, v) in pairs {
                out.push_str(&format!(" \\\n  -d {}", sh_quoted(&format!("{k}={v}"))));
            }
        }
        BodyExport::Multipart(pairs) => {
            for (k, v) in pairs {
                out.push_str(&format!(" \\\n  -F {}", sh_quoted(&format!("{k}={v}"))));
            }
        }
        BodyExport::Unsupported(msg) => {
            out.push_str(&format!("\n# NOTE: {msg}"));
        }
    }

    if let Some(note) = auth_comment(export, "\n#") {
        out.push_str(&note);
    }

    out.push('\n');
    out
}

// ─── Python (requests) ───────────────────────────────────────────────────

fn to_python(export: &ReqExport) -> String {
    let mut out = String::from("import requests\n\n");
    out.push_str(&format!("url = {}\n\n", quoted(&export.url)));

    let headers = effective_headers(export);
    if headers.is_empty() {
        out.push_str("headers = {}\n\n");
    } else {
        out.push_str("headers = {\n");
        for (k, v) in &headers {
            out.push_str(&format!("    {}: {},\n", quoted(k), quoted(v)));
        }
        out.push_str("}\n\n");
    }

    let mut kwargs = vec!["headers=headers".to_string()];

    match &export.body {
        BodyExport::None => {}
        BodyExport::Unsupported(msg) => {
            out.push_str(&format!("# NOTE: {msg}\n\n"));
        }
        BodyExport::Raw { text, .. } => {
            out.push_str(&format!("payload = {}\n\n", quoted(text)));
            kwargs.push("data=payload".to_string());
        }
        BodyExport::Form(pairs) => {
            out.push_str("payload = {\n");
            for (k, v) in pairs {
                out.push_str(&format!("    {}: {},\n", quoted(k), quoted(v)));
            }
            out.push_str("}\n\n");
            kwargs.push("data=payload".to_string());
        }
        BodyExport::Multipart(pairs) => {
            out.push_str("files = {\n");
            for (k, v) in pairs {
                out.push_str(&format!("    {}: (None, {}),\n", quoted(k), quoted(v)));
            }
            out.push_str("}\n\n");
            kwargs.push("files=files".to_string());
        }
    }

    if let Some((user, pass)) = &export.basic_auth {
        kwargs.push(format!("auth=({}, {})", quoted(user), quoted(pass)));
    }

    if let Some(note) = auth_comment(export, "#") {
        out.push_str(&note);
        out.push_str("\n\n");
    }

    out.push_str(&format!(
        "response = requests.{}(url, {})\n\n",
        export.method.to_lowercase(),
        kwargs.join(", ")
    ));
    out.push_str("print(response.status_code)\nprint(response.text)\n");
    out
}

// ─── JavaScript (browser fetch) ──────────────────────────────────────────

fn to_javascript(export: &ReqExport) -> String {
    let mut header_lines = header_entries(&effective_headers(export));

    let mut leading_comment = String::new();
    if let Some((user, pass)) = &export.basic_auth {
        leading_comment.push_str(
            "// btoa() is a browser global; in Node.js use Buffer.from(...).toString(\"base64\")\n",
        );
        header_lines.push(format!(
            "Authorization: \"Basic \" + btoa({}),",
            quoted(&format!("{user}:{pass}"))
        ));
    }

    let mut preamble = String::new();
    let mut body_line: Option<String> = None;
    match &export.body {
        BodyExport::None => {}
        BodyExport::Unsupported(msg) => preamble.push_str(&format!("// NOTE: {msg}\n\n")),
        BodyExport::Raw { text, .. } => body_line = Some(format!("  body: {},", quoted(text))),
        BodyExport::Form(pairs) => {
            let obj = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", quoted(k), quoted(v)))
                .collect::<Vec<_>>()
                .join(", ");
            body_line = Some(format!("  body: new URLSearchParams({{ {obj} }}).toString(),"));
        }
        BodyExport::Multipart(pairs) => {
            preamble.push_str("const formData = new FormData();\n");
            for (k, v) in pairs {
                preamble.push_str(&format!("formData.append({}, {});\n", quoted(k), quoted(v)));
            }
            preamble.push('\n');
            body_line = Some("  body: formData,".to_string());
        }
    }

    let mut out = leading_comment;
    out.push_str(&preamble);
    out.push_str(&format!("fetch({}, {{\n", quoted(&export.url)));
    out.push_str(&format!("  method: {},\n", quoted(&export.method)));
    if !header_lines.is_empty() {
        out.push_str("  headers: {\n");
        for l in &header_lines {
            out.push_str(&format!("    {l}\n"));
        }
        out.push_str("  },\n");
    }
    if let Some(b) = &body_line {
        out.push_str(&format!("{b}\n"));
    }
    out.push_str("})\n  .then((res) => res.text())\n  .then(console.log)\n  .catch(console.error);\n");

    if let Some(note) = auth_comment(export, "//") {
        out.push_str(&note);
        out.push('\n');
    }

    out
}

// ─── Node.js (axios) ──────────────────────────────────────────────────────

fn to_nodejs(export: &ReqExport) -> String {
    let mut header_lines = header_entries(&effective_headers(export));

    let mut preamble = String::new();
    let mut data_line: Option<String> = None;
    match &export.body {
        BodyExport::None => {}
        BodyExport::Unsupported(msg) => preamble.push_str(&format!("// NOTE: {msg}\n\n")),
        BodyExport::Raw { text, .. } => {
            data_line = Some(format!("  data: {},", quoted(text)));
        }
        BodyExport::Form(pairs) => {
            let obj = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", quoted(k), quoted(v)))
                .collect::<Vec<_>>()
                .join(", ");
            preamble.push_str(&format!("const payload = new URLSearchParams({{ {obj} }}).toString();\n\n"));
            data_line = Some("  data: payload,".to_string());
        }
        BodyExport::Multipart(pairs) => {
            preamble.push_str(
                "// npm install form-data\nconst FormData = require(\"form-data\");\nconst form = new FormData();\n",
            );
            for (k, v) in pairs {
                preamble.push_str(&format!("form.append({}, {});\n", quoted(k), quoted(v)));
            }
            preamble.push('\n');
            data_line = Some("  data: form,".to_string());
            header_lines.push("...form.getHeaders(),".to_string());
        }
    }

    let mut out = String::from("const axios = require(\"axios\");\n\n");
    out.push_str(&preamble);
    out.push_str("axios({\n");
    out.push_str(&format!("  method: {},\n", quoted(&export.method.to_lowercase())));
    out.push_str(&format!("  url: {},\n", quoted(&export.url)));
    if !header_lines.is_empty() {
        out.push_str("  headers: {\n");
        for l in &header_lines {
            out.push_str(&format!("    {l}\n"));
        }
        out.push_str("  },\n");
    }
    if let Some(d) = &data_line {
        out.push_str(&format!("{d}\n"));
    }
    if let Some((user, pass)) = &export.basic_auth {
        out.push_str(&format!(
            "  auth: {{ username: {}, password: {} }},\n",
            quoted(user),
            quoted(pass)
        ));
    }
    out.push_str("})\n  .then((res) => console.log(res.data))\n  .catch(console.error);\n");

    if let Some(note) = auth_comment(export, "//") {
        out.push_str(&note);
        out.push('\n');
    }

    out
}

// ─── Go (net/http) ────────────────────────────────────────────────────────

fn to_go(export: &ReqExport) -> String {
    let mut imports = vec!["\"fmt\"", "\"io\"", "\"net/http\""];
    let mut preamble = String::new();
    let body_expr: String;
    let is_multipart = matches!(export.body, BodyExport::Multipart(_));

    match &export.body {
        BodyExport::None => {
            body_expr = "nil".to_string();
        }
        BodyExport::Unsupported(msg) => {
            preamble.push_str(&format!("\t// NOTE: {msg}\n"));
            body_expr = "nil".to_string();
        }
        BodyExport::Raw { text, .. } => {
            imports.push("\"strings\"");
            preamble.push_str(&format!("\tpayload := strings.NewReader({})\n", go_string_literal(text)));
            body_expr = "payload".to_string();
        }
        BodyExport::Form(pairs) => {
            imports.push("\"net/url\"");
            imports.push("\"strings\"");
            preamble.push_str("\tform := url.Values{}\n");
            for (k, v) in pairs {
                preamble.push_str(&format!(
                    "\tform.Set({}, {})\n",
                    go_string_literal(k),
                    go_string_literal(v)
                ));
            }
            preamble.push_str("\tpayload := strings.NewReader(form.Encode())\n");
            body_expr = "payload".to_string();
        }
        BodyExport::Multipart(pairs) => {
            imports.push("\"bytes\"");
            imports.push("\"mime/multipart\"");
            preamble.push_str("\tvar buf bytes.Buffer\n\twriter := multipart.NewWriter(&buf)\n");
            for (k, v) in pairs {
                preamble.push_str(&format!(
                    "\twriter.WriteField({}, {})\n",
                    go_string_literal(k),
                    go_string_literal(v)
                ));
            }
            preamble.push_str("\twriter.Close()\n\tpayload := &buf\n");
            body_expr = "payload".to_string();
        }
    }

    imports.sort();
    imports.dedup();

    let mut out = String::from("package main\n\nimport (\n");
    for imp in &imports {
        out.push_str(&format!("\t{imp}\n"));
    }
    out.push_str(")\n\nfunc main() {\n");
    out.push_str(&format!("\turl := {}\n", go_string_literal(&export.url)));
    out.push_str(&preamble);
    out.push('\n');
    out.push_str(&format!(
        "\treq, err := http.NewRequest({}, url, {})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\n",
        go_string_literal(&export.method),
        body_expr
    ));

    let headers = effective_headers(export);
    let mut wrote_header_setup = false;
    if is_multipart {
        out.push_str("\treq.Header.Set(\"Content-Type\", writer.FormDataContentType())\n");
        wrote_header_setup = true;
    }
    for (k, v) in &headers {
        out.push_str(&format!(
            "\treq.Header.Add({}, {})\n",
            go_string_literal(k),
            go_string_literal(v)
        ));
        wrote_header_setup = true;
    }
    if let Some((user, pass)) = &export.basic_auth {
        out.push_str(&format!(
            "\treq.SetBasicAuth({}, {})\n",
            go_string_literal(user),
            go_string_literal(pass)
        ));
        wrote_header_setup = true;
    }
    if wrote_header_setup {
        out.push('\n');
    }

    out.push_str(
        "\tclient := &http.Client{}\n\tresp, err := client.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer resp.Body.Close()\n\n\tbody, _ := io.ReadAll(resp.Body)\n\tfmt.Println(resp.StatusCode)\n\tfmt.Println(string(body))\n}\n",
    );

    if let Some(note) = auth_comment(export, "\t//") {
        out.push_str(&note);
        out.push('\n');
    }

    out
}

// ─── PHP (cURL) ───────────────────────────────────────────────────────────

fn to_php(export: &ReqExport) -> String {
    let mut out = String::from("<?php\n\n$curl = curl_init();\n\ncurl_setopt_array($curl, [\n");
    out.push_str(&format!("    CURLOPT_URL => {},\n", php_quoted(&export.url)));
    out.push_str("    CURLOPT_RETURNTRANSFER => true,\n");
    out.push_str(&format!(
        "    CURLOPT_CUSTOMREQUEST => {},\n",
        php_quoted(&export.method)
    ));

    match &export.body {
        BodyExport::None => {}
        BodyExport::Unsupported(msg) => {
            out.push_str(&format!("    // NOTE: {msg}\n"));
        }
        BodyExport::Raw { text, .. } => {
            out.push_str(&format!("    CURLOPT_POSTFIELDS => {},\n", php_quoted(text)));
        }
        BodyExport::Form(pairs) => {
            let joined = pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            out.push_str(&format!("    CURLOPT_POSTFIELDS => {},\n", php_quoted(&joined)));
        }
        BodyExport::Multipart(pairs) => {
            out.push_str("    CURLOPT_POSTFIELDS => [\n");
            for (k, v) in pairs {
                out.push_str(&format!("        {} => {},\n", php_quoted(k), php_quoted(v)));
            }
            out.push_str("    ],\n");
        }
    }

    if let Some((user, pass)) = &export.basic_auth {
        out.push_str(&format!(
            "    CURLOPT_USERPWD => {},\n",
            php_quoted(&format!("{user}:{pass}"))
        ));
    }

    let headers = effective_headers(export);
    if !headers.is_empty() {
        out.push_str("    CURLOPT_HTTPHEADER => [\n");
        for (k, v) in &headers {
            out.push_str(&format!("        {},\n", php_quoted(&format!("{k}: {v}"))));
        }
        out.push_str("    ],\n");
    }

    out.push_str(
        "]);\n\n$response = curl_exec($curl);\n$err = curl_error($curl);\ncurl_close($curl);\n\nif ($err) {\n    echo \"cURL Error: \" . $err;\n} else {\n    echo $response;\n}\n",
    );

    if let Some(note) = auth_comment(export, "//") {
        out.push_str(&note);
        out.push('\n');
    }

    out
}

// ─── Rust (reqwest) ───────────────────────────────────────────────────────
//
// Uses `reqwest::blocking`, the same crate (async form) this app itself uses
// to execute requests in `http_client.rs::execute_request`.

fn to_rust(export: &ReqExport) -> String {
    let method_call = match export.method.as_str() {
        "GET" => format!("client.get({})", quoted(&export.url)),
        "POST" => format!("client.post({})", quoted(&export.url)),
        "PUT" => format!("client.put({})", quoted(&export.url)),
        "DELETE" => format!("client.delete({})", quoted(&export.url)),
        "PATCH" => format!("client.patch({})", quoted(&export.url)),
        "HEAD" => format!("client.head({})", quoted(&export.url)),
        other => format!("client.request(reqwest::Method::{other}, {})", quoted(&export.url)),
    };

    let mut out = String::from(
        "use reqwest::blocking::Client;\n\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let client = Client::new();\n\n",
    );

    if let BodyExport::Unsupported(msg) = &export.body {
        out.push_str(&format!("    // NOTE: {msg}\n"));
    }
    if matches!(export.body, BodyExport::Multipart(_)) {
        out.push_str("    let form = reqwest::blocking::multipart::Form::new()\n");
        if let BodyExport::Multipart(pairs) = &export.body {
            for (k, v) in pairs {
                out.push_str(&format!("        .text({}, {})\n", quoted(k), quoted(v)));
            }
        }
        out.push_str("        ;\n\n");
    }

    out.push_str(&format!("    let response = {method_call}\n"));

    for (k, v) in effective_headers(export) {
        out.push_str(&format!("        .header({}, {})\n", quoted(&k), quoted(&v)));
    }
    if let Some((user, pass)) = &export.basic_auth {
        out.push_str(&format!(
            "        .basic_auth({}, Some({}))\n",
            quoted(user),
            quoted(pass)
        ));
    }

    match &export.body {
        BodyExport::None | BodyExport::Unsupported(_) => {}
        BodyExport::Raw { text, .. } => {
            out.push_str(&format!("        .body({})\n", rust_string_literal(text)));
        }
        BodyExport::Form(pairs) => {
            let items = pairs
                .iter()
                .map(|(k, v)| format!("({}, {})", quoted(k), quoted(v)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("        .form(&[{items}])\n"));
        }
        BodyExport::Multipart(_) => {
            out.push_str("        .multipart(form)\n");
        }
    }

    out.push_str("        .send()?;\n\n");
    out.push_str("    println!(\"{}\", response.status());\n    println!(\"{}\", response.text()?);\n\n    Ok(())\n}\n");

    if let Some(note) = auth_comment(export, "    //") {
        out.push_str(&note);
        out.push('\n');
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::HttpMethod;

    fn fresh() -> HttpClientState {
        let mut s = HttpClientState::default();
        // Default state ships an empty "Accept: */*" header and empty rows;
        // start each test from a clean slate so assertions are precise.
        s.headers.clear();
        s.params.clear();
        s.form_data.clear();
        s
    }

    #[test]
    fn generates_curl_for_simple_get() {
        let mut s = fresh();
        s.url = "https://api.example.com/users".to_string();
        s.method = HttpMethod::GET;
        let code = generate(&CodeLang::Curl, &s);
        assert!(code.contains("curl -X GET 'https://api.example.com/users'"));
    }

    #[test]
    fn generates_curl_includes_json_body_and_headers() {
        let mut s = fresh();
        s.url = "https://api.example.com/users".to_string();
        s.method = HttpMethod::POST;
        s.body_type = HttpBodyType::Json;
        s.body_text = r#"{"name":"Jayuda"}"#.to_string();
        s.headers.push(("X-Trace".to_string(), "abc".to_string(), true));
        let code = generate(&CodeLang::Curl, &s);
        assert!(code.contains("-X POST"));
        assert!(code.contains("Content-Type: application/json"));
        assert!(code.contains(r#"{"name":"Jayuda"}"#));
        assert!(code.contains("X-Trace: abc"));
    }

    #[test]
    fn generates_python_with_bearer_auth() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.method = HttpMethod::GET;
        s.auth_type = HttpAuthType::BearerToken;
        s.bearer_token = "abc123".to_string();
        let code = generate(&CodeLang::Python, &s);
        assert!(code.contains("import requests"));
        assert!(code.contains("\"Authorization\": \"Bearer abc123\""));
        assert!(code.contains("requests.get(url, headers=headers)"));
    }

    #[test]
    fn generates_python_urlencoded_form() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.method = HttpMethod::POST;
        s.body_type = HttpBodyType::UrlEncoded;
        s.form_data.push(("a".to_string(), "1".to_string(), true));
        s.form_data.push(("b".to_string(), "2".to_string(), true));
        let code = generate(&CodeLang::Python, &s);
        assert!(code.contains("\"a\": \"1\""));
        assert!(code.contains("\"b\": \"2\""));
        assert!(code.contains("data=payload"));
    }

    #[test]
    fn generates_javascript_fetch_basic_shape() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.method = HttpMethod::GET;
        let code = generate(&CodeLang::JavaScript, &s);
        assert!(code.contains(r#"fetch("https://api.example.com/x""#));
        assert!(code.contains(r#"method: "GET""#));
    }

    #[test]
    fn generates_nodejs_axios_with_basic_auth() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.method = HttpMethod::GET;
        s.auth_type = HttpAuthType::BasicAuth;
        s.basic_user = "alice".to_string();
        s.basic_pass = "secret".to_string();
        let code = generate(&CodeLang::NodeJs, &s);
        assert!(code.contains("require(\"axios\")"));
        assert!(code.contains("auth: { username: \"alice\", password: \"secret\" }"));
    }

    #[test]
    fn generates_go_with_query_params() {
        let mut s = fresh();
        s.url = "https://api.example.com/search".to_string();
        s.method = HttpMethod::GET;
        s.params.push(("q".to_string(), "rust".to_string(), true));
        let code = generate(&CodeLang::Go, &s);
        assert!(code.contains("package main"));
        assert!(code.contains("net/http"));
        assert!(code.contains("api.example.com/search?q=rust"));
    }

    #[test]
    fn generates_php_with_headers() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.method = HttpMethod::GET;
        s.headers.push(("Accept".to_string(), "application/json".to_string(), true));
        let code = generate(&CodeLang::Php, &s);
        assert!(code.contains("curl_init()"));
        assert!(code.contains("'Accept: application/json'"));
    }

    #[test]
    fn generates_rust_reqwest_with_json_body() {
        let mut s = fresh();
        s.url = "https://api.example.com/users".to_string();
        s.method = HttpMethod::POST;
        s.body_type = HttpBodyType::Json;
        s.body_text = r#"{"name":"Jayuda"}"#.to_string();
        let code = generate(&CodeLang::Rust, &s);
        assert!(code.contains("use reqwest::blocking::Client"));
        assert!(code.contains("client.post(\"https://api.example.com/users\")"));
        assert!(code.contains(".header(\"Content-Type\", \"application/json\")"));
        assert!(code.contains(r#"{"name":"Jayuda"}"#));
    }

    #[test]
    fn generates_rust_reqwest_with_basic_auth() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.auth_type = HttpAuthType::BasicAuth;
        s.basic_user = "alice".to_string();
        s.basic_pass = "secret".to_string();
        let code = generate(&CodeLang::Rust, &s);
        assert!(code.contains(".basic_auth(\"alice\", Some(\"secret\"))"));
    }

    #[test]
    fn disabled_rows_are_excluded_from_every_language() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.headers.push(("X-Off".to_string(), "nope".to_string(), false));
        s.params.push(("off".to_string(), "nope".to_string(), false));
        for lang in CodeLang::all() {
            let code = generate(&lang, &s);
            assert!(!code.contains("X-Off"), "{lang:?} leaked a disabled header");
            assert!(!code.contains("nope"), "{lang:?} leaked a disabled param/value");
        }
    }

    #[test]
    fn binary_file_body_emits_not_supported_note() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.method = HttpMethod::POST;
        s.body_type = HttpBodyType::BinaryFile;
        for lang in CodeLang::all() {
            let code = generate(&lang, &s);
            assert!(code.contains("not yet supported"), "{lang:?} missing unsupported-body note");
        }
    }

    #[test]
    fn unsupported_auth_kind_emits_note_across_all_langs() {
        let mut s = fresh();
        s.url = "https://api.example.com/x".to_string();
        s.auth_type = HttpAuthType::OAuth2;
        for lang in CodeLang::all() {
            let code = generate(&lang, &s);
            assert!(
                code.contains("OAuth 2.0 authentication is not yet supported"),
                "{lang:?} missing unsupported-auth note"
            );
        }
    }
}
