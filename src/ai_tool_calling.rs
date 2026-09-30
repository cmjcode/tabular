//! Bagian murni (tanpa I/O) dari tool calling untuk chat HTTP API (K5).
//!
//! Model menerima definisi tool dari MCP server luar yang diizinkan user, lalu
//! bisa meminta tool dipanggil. Modul ini hanya membentuk body request dan
//! membaca response untuk tiga format native:
//! - OpenAI-compatible: `tools` / `tool_calls` / pesan role `tool`
//! - Anthropic: `tools` / blok `tool_use` / blok `tool_result`
//! - Gemini: `functionDeclarations` / `functionCall` / `functionResponse`
//!
//! Percakapan disimpan dalam bentuk netral ([`ConvMessage`]) sehingga loop
//! (lihat `ai_tool_chat`) tidak peduli provider mana yang dipakai. Termasuk di
//! sini: prefiks nama tool `<server>__<tool>`, pemotongan output, dan konversi
//! hasil MCP `tools/call` menjadi teks.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config::AiApiStyle;

/// Pemisah server dan tool di nama tool yang dikirim ke model.
pub const NAME_SEPARATOR: &str = "__";
/// Batas panjang nama fungsi yang aman untuk ketiga provider.
pub const MAX_TOOL_NAME_LEN: usize = 64;
/// Batas output satu hasil tool yang dikirim balik ke model (byte).
pub const MAX_TOOL_RESULT_BYTES: usize = 20 * 1024;
/// Maksimum ronde tool per giliran user.
pub const MAX_TOOL_ROUNDS: usize = 8;
/// Batas token jawaban untuk request dengan tool.
const MAX_OUTPUT_TOKENS: u32 = 4096;

/// Definisi satu tool yang ditawarkan ke model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    /// Nama berprefiks (`<server>__<tool>`), sudah disanitasi.
    pub name: String,
    pub description: String,
    /// JSON Schema input dari MCP tool.
    pub parameters: Value,
}

/// Permintaan model untuk memanggil tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Data provider yang harus dikirim ulang apa adanya (Gemini
    /// `thoughtSignature`).
    pub provider_meta: Option<Value>,
}

/// Hasil tool yang dikirim balik ke model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// Satu pesan percakapan dalam bentuk netral.
#[derive(Debug, Clone, PartialEq)]
pub enum ConvMessage {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    ToolResults(Vec<ToolResult>),
}

/// Jawaban model untuk satu request.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelTurn {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// Status pemanggilan tool yang tampil di transkrip chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolCallStatus {
    /// Menunggu Approve / Deny dari user.
    AwaitingApproval,
    #[default]
    Running,
    Done,
    Failed,
    Denied,
}

/// Catatan satu pemanggilan tool untuk transkrip (dan riwayat sesi).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolCallRecord {
    pub call_id: String,
    /// Nama untuk tampilan: `server / tool`.
    pub display_name: String,
    /// Argumen dalam JSON yang sudah dirapikan.
    pub arguments: String,
    pub status: ToolCallStatus,
    /// Ringkasan hasil (beberapa ratus karakter pertama) atau alasan gagal.
    pub result_summary: String,
}

// ─── Nama tool ──────────────────────────────────────────────────────────────

/// Sanitasi potongan nama: hanya `[A-Za-z0-9_-]`, karakter lain jadi `_`.
fn sanitize_part(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Hash pendek (FNV-1a) untuk membedakan nama yang dipotong.
fn short_hash(s: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

/// Nama tool untuk model: `<server>__<tool>`, disanitasi, diawali huruf, dan
/// paling panjang [`MAX_TOOL_NAME_LEN`]. Nama yang terlalu panjang dipotong
/// dan diberi hash supaya tetap unik; pemetaan balik memakai tabel binding,
/// bukan parsing string.
pub fn prefixed_tool_name(server: &str, tool: &str) -> String {
    let raw = format!("{server}{NAME_SEPARATOR}{tool}");
    let mut name = sanitize_part(&raw);
    if !name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    {
        name = format!("t_{name}");
    }
    if name.len() > MAX_TOOL_NAME_LEN {
        let hash = short_hash(&raw);
        let keep = MAX_TOOL_NAME_LEN - hash.len() - 1;
        name = format!("{}_{hash}", &name[..keep]);
    }
    name
}

/// Pisahkan nama berprefiks menjadi (server, tool) pada pemisah pertama.
pub fn split_prefixed(name: &str) -> Option<(&str, &str)> {
    let (server, tool) = name.split_once(NAME_SEPARATOR)?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

/// Nama server yang valid: tidak kosong, hanya `[A-Za-z0-9_-]`, tanpa `__`.
pub fn validate_server_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Server name is required.".to_string());
    }
    if name.len() > 32 {
        return Err("Server name must be at most 32 characters.".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("Use only letters, digits, '-' and '_' in the server name.".to_string());
    }
    if name.contains(NAME_SEPARATOR) {
        return Err("The server name cannot contain '__'.".to_string());
    }
    Ok(())
}

// ─── Output ─────────────────────────────────────────────────────────────────

/// Potong teks pada batas karakter UTF-8 dan beri penanda berapa byte hilang.
pub fn truncate_output(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n…[truncated {} bytes]",
        &s[..end],
        s.len().saturating_sub(end)
    )
}

/// Ringkasan satu baris untuk kartu tool di chat.
pub fn summarize(s: &str, max_chars: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        flat
    } else {
        let head: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// Ubah hasil MCP `tools/call` (sudah diserialisasi ke JSON) menjadi teks
/// untuk model. Mengembalikan `(teks, is_error)`.
pub fn mcp_result_to_text(result: &Value) -> (String, bool) {
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    if let Some(items) = result["content"].as_array() {
        for item in items {
            match item["type"].as_str() {
                Some("text") => {
                    if let Some(t) = item["text"].as_str() {
                        parts.push(t.to_string());
                    }
                }
                Some("image") => parts.push(format!(
                    "[image {}]",
                    item["mimeType"].as_str().unwrap_or("")
                )),
                Some("audio") => parts.push(format!(
                    "[audio {}]",
                    item["mimeType"].as_str().unwrap_or("")
                )),
                Some("resource") => {
                    let res = &item["resource"];
                    match res["text"].as_str() {
                        Some(t) => parts.push(t.to_string()),
                        None => {
                            parts.push(format!("[resource {}]", res["uri"].as_str().unwrap_or("")))
                        }
                    }
                }
                Some("resource_link") => parts.push(format!(
                    "[resource link {}]",
                    item["uri"].as_str().unwrap_or("")
                )),
                _ => parts.push(item.to_string()),
            }
        }
    }
    if parts.is_empty()
        && let Some(sc) = result.get("structuredContent")
        && !sc.is_null()
    {
        parts.push(sc.to_string());
    }
    let text = if parts.is_empty() {
        "(no output)".to_string()
    } else {
        parts.join("\n")
    };
    (text, is_error)
}

// ─── Schema ─────────────────────────────────────────────────────────────────

/// Pastikan schema berupa object dengan `type: object` (syarat OpenAI dan
/// Anthropic untuk parameter fungsi).
pub fn normalize_object_schema(schema: &Value) -> Value {
    let mut obj = match schema {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    obj.entry("type").or_insert_with(|| json!("object"));
    if obj.get("type") == Some(&json!("object")) {
        obj.entry("properties").or_insert_with(|| json!({}));
    }
    Value::Object(obj)
}

/// Field schema yang diterima `functionDeclarations.parameters` Gemini
/// (subset OpenAPI). Field lain (`$schema`, `additionalProperties`, `$ref`, …)
/// ditolak API, jadi dibuang.
const GEMINI_SCHEMA_KEYS: &[&str] = &[
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "maxItems",
    "minItems",
    "properties",
    "required",
    "minProperties",
    "maxProperties",
    "minLength",
    "maxLength",
    "pattern",
    "anyOf",
    "items",
    "minimum",
    "maximum",
];

/// Saring JSON Schema menjadi subset yang diterima Gemini secara rekursif.
/// `type: ["string","null"]` menjadi `type: "string", nullable: true`.
pub fn gemini_schema(schema: &Value) -> Value {
    let Value::Object(m) = schema else {
        return schema.clone();
    };
    let mut out = Map::new();
    for (k, v) in m {
        if !GEMINI_SCHEMA_KEYS.contains(&k.as_str()) {
            continue;
        }
        let v = match k.as_str() {
            "type" => match v {
                Value::Array(types) => {
                    let non_null: Vec<&Value> = types
                        .iter()
                        .filter(|t| t.as_str() != Some("null"))
                        .collect();
                    if non_null.len() < types.len() {
                        out.insert("nullable".to_string(), json!(true));
                    }
                    non_null
                        .first()
                        .map(|t| (*t).clone())
                        .unwrap_or(json!("string"))
                }
                other => other.clone(),
            },
            "properties" => match v {
                Value::Object(props) => Value::Object(
                    props
                        .iter()
                        .map(|(pk, pv)| (pk.clone(), gemini_schema(pv)))
                        .collect(),
                ),
                other => other.clone(),
            },
            "items" => gemini_schema(v),
            "anyOf" => match v {
                Value::Array(list) => Value::Array(list.iter().map(gemini_schema).collect()),
                other => other.clone(),
            },
            // Gemini hanya menerima enum string.
            "enum" => match v {
                Value::Array(list) => Value::Array(
                    list.iter()
                        .map(|e| match e {
                            Value::String(_) => e.clone(),
                            other => Value::String(other.to_string()),
                        })
                        .collect(),
                ),
                other => other.clone(),
            },
            _ => v.clone(),
        };
        out.insert(k.clone(), v);
    }
    Value::Object(out)
}

// ─── Body request ───────────────────────────────────────────────────────────

/// Body request untuk gaya API `style`.
pub fn request_body(
    style: AiApiStyle,
    model: &str,
    system: &str,
    messages: &[ConvMessage],
    tools: &[ToolDef],
) -> Value {
    match style {
        AiApiStyle::OpenAiCompatible => openai_body(model, system, messages, tools),
        AiApiStyle::Anthropic => anthropic_body(model, system, messages, tools),
        AiApiStyle::Gemini => gemini_body(system, messages, tools),
    }
}

/// Body `/chat/completions` dengan `tools`.
pub fn openai_body(
    model: &str,
    system: &str,
    messages: &[ConvMessage],
    tools: &[ToolDef],
) -> Value {
    let mut out: Vec<Value> = Vec::new();
    if !system.trim().is_empty() {
        out.push(json!({ "role": "system", "content": system }));
    }
    for m in messages {
        match m {
            ConvMessage::User(text) => out.push(json!({ "role": "user", "content": text })),
            ConvMessage::Assistant { text, calls } => {
                let mut msg = json!({
                    "role": "assistant",
                    "content": if text.is_empty() { Value::Null } else { json!(text) },
                });
                if !calls.is_empty() {
                    msg["tool_calls"] = Value::Array(
                        calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": {
                                        "name": c.name,
                                        "arguments": c.arguments.to_string(),
                                    }
                                })
                            })
                            .collect(),
                    );
                }
                out.push(msg);
            }
            ConvMessage::ToolResults(results) => {
                for r in results {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": r.call_id,
                        "content": r.content,
                    }));
                }
            }
        }
    }
    let mut body = json!({
        "model": model,
        "messages": out,
        "temperature": 0.2,
        "max_tokens": MAX_OUTPUT_TOKENS,
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": normalize_object_schema(&t.parameters),
                        }
                    })
                })
                .collect(),
        );
    }
    body
}

/// Body `/messages` Anthropic dengan `tools`.
pub fn anthropic_body(
    model: &str,
    system: &str,
    messages: &[ConvMessage],
    tools: &[ToolDef],
) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        match m {
            ConvMessage::User(text) => out.push(json!({ "role": "user", "content": text })),
            ConvMessage::Assistant { text, calls } => {
                let mut blocks: Vec<Value> = Vec::new();
                // Blok teks kosong ditolak API.
                if !text.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": text }));
                }
                for c in calls {
                    let input = if c.arguments.is_object() {
                        c.arguments.clone()
                    } else {
                        json!({})
                    };
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": c.id,
                        "name": c.name,
                        "input": input,
                    }));
                }
                if blocks.is_empty() {
                    blocks.push(json!({ "type": "text", "text": "(no text)" }));
                }
                out.push(json!({ "role": "assistant", "content": blocks }));
            }
            ConvMessage::ToolResults(results) => {
                let blocks: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": r.call_id,
                            "content": r.content,
                            "is_error": r.is_error,
                        })
                    })
                    .collect();
                out.push(json!({ "role": "user", "content": blocks }));
            }
        }
    }
    let mut body = json!({
        "model": model,
        "messages": out,
        "max_tokens": MAX_OUTPUT_TOKENS,
    });
    if !system.trim().is_empty() {
        body["system"] = json!(system);
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(
            tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": normalize_object_schema(&t.parameters),
                    })
                })
                .collect(),
        );
    }
    body
}

/// Body `generateContent` Gemini dengan `functionDeclarations`.
pub fn gemini_body(system: &str, messages: &[ConvMessage], tools: &[ToolDef]) -> Value {
    let mut contents: Vec<Value> = Vec::new();
    for m in messages {
        match m {
            ConvMessage::User(text) => {
                contents.push(json!({ "role": "user", "parts": [ { "text": text } ] }))
            }
            ConvMessage::Assistant { text, calls } => {
                let mut parts: Vec<Value> = Vec::new();
                if !text.is_empty() {
                    parts.push(json!({ "text": text }));
                }
                for c in calls {
                    let mut fc = json!({ "name": c.name, "args": c.arguments });
                    if !c.id.is_empty() && !c.id.starts_with(SYNTHETIC_ID_PREFIX) {
                        fc["id"] = json!(c.id);
                    }
                    let mut part = json!({ "functionCall": fc });
                    // Model Gemini 2.5/3 mewajibkan thoughtSignature dikirim ulang.
                    if let Some(sig) = c.provider_meta.as_ref() {
                        part["thoughtSignature"] = sig.clone();
                    }
                    parts.push(part);
                }
                if parts.is_empty() {
                    parts.push(json!({ "text": "" }));
                }
                contents.push(json!({ "role": "model", "parts": parts }));
            }
            ConvMessage::ToolResults(results) => {
                let parts: Vec<Value> = results
                    .iter()
                    .map(|r| {
                        let key = if r.is_error { "error" } else { "content" };
                        let mut fr = json!({
                            "name": r.name,
                            "response": { key: r.content },
                        });
                        if !r.call_id.starts_with(SYNTHETIC_ID_PREFIX) {
                            fr["id"] = json!(r.call_id);
                        }
                        json!({ "functionResponse": fr })
                    })
                    .collect();
                contents.push(json!({ "role": "user", "parts": parts }));
            }
        }
    }
    let mut body = json!({
        "contents": contents,
        "generationConfig": { "temperature": 0.2, "maxOutputTokens": 8192 },
    });
    if !system.trim().is_empty() {
        body["systemInstruction"] = json!({ "parts": [ { "text": system } ] });
    }
    if !tools.is_empty() {
        body["tools"] = json!([{
            "functionDeclarations": tools
                .iter()
                .map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": gemini_schema(&normalize_object_schema(&t.parameters)),
                }))
                .collect::<Vec<_>>()
        }]);
    }
    body
}

// ─── Parsing response ───────────────────────────────────────────────────────

/// Prefiks id buatan saat provider tidak memberi id pemanggilan.
const SYNTHETIC_ID_PREFIX: &str = "tabular_call_";

fn synthetic_id(round: usize, idx: usize) -> String {
    format!("{SYNTHETIC_ID_PREFIX}{round}_{idx}")
}

/// Baca response untuk gaya API `style`. `round` dipakai untuk id buatan.
pub fn parse_response(style: AiApiStyle, text: &str, round: usize) -> Result<ModelTurn, String> {
    let json: Value =
        serde_json::from_str(text).map_err(|e| format!("Failed to parse response: {e}"))?;
    match style {
        AiApiStyle::OpenAiCompatible => parse_openai(&json, text, round),
        AiApiStyle::Anthropic => parse_anthropic(&json, text, round),
        AiApiStyle::Gemini => parse_gemini(&json, text, round),
    }
}

fn parse_openai(json: &Value, raw: &str, round: usize) -> Result<ModelTurn, String> {
    let msg = json["choices"]
        .get(0)
        .map(|c| &c["message"])
        .ok_or_else(|| format!("Unexpected response format: {raw}"))?;
    let text = msg["content"].as_str().unwrap_or("").trim().to_string();
    let mut calls = Vec::new();
    if let Some(list) = msg["tool_calls"].as_array() {
        for (i, c) in list.iter().enumerate() {
            let Some(name) = c["function"]["name"].as_str() else {
                continue;
            };
            let arguments = match &c["function"]["arguments"] {
                Value::String(s) if s.trim().is_empty() => json!({}),
                Value::String(s) => {
                    serde_json::from_str(s).unwrap_or_else(|_| json!({ "_raw": s }))
                }
                Value::Null => json!({}),
                other => other.clone(),
            };
            let id = c["id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| synthetic_id(round, i));
            calls.push(ToolCall {
                id,
                name: name.to_string(),
                arguments,
                provider_meta: None,
            });
        }
    }
    Ok(ModelTurn { text, calls })
}

fn parse_anthropic(json: &Value, raw: &str, round: usize) -> Result<ModelTurn, String> {
    let blocks = json["content"]
        .as_array()
        .ok_or_else(|| format!("Unexpected Anthropic response format: {raw}"))?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, b) in blocks.iter().enumerate() {
        match b["type"].as_str() {
            Some("text") => text.push_str(b["text"].as_str().unwrap_or("")),
            Some("tool_use") => {
                let Some(name) = b["name"].as_str() else {
                    continue;
                };
                calls.push(ToolCall {
                    id: b["id"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| synthetic_id(round, i)),
                    name: name.to_string(),
                    arguments: b.get("input").cloned().unwrap_or_else(|| json!({})),
                    provider_meta: None,
                });
            }
            _ => {}
        }
    }
    Ok(ModelTurn {
        text: text.trim().to_string(),
        calls,
    })
}

fn parse_gemini(json: &Value, raw: &str, round: usize) -> Result<ModelTurn, String> {
    let Some(candidate) = json["candidates"].get(0) else {
        if let Some(reason) = json["promptFeedback"]["blockReason"].as_str() {
            return Err(format!("Gemini blocked the prompt ({reason})."));
        }
        return Err(format!("Unexpected Gemini response format: {raw}"));
    };
    let mut text = String::new();
    let mut calls = Vec::new();
    if let Some(parts) = candidate["content"]["parts"].as_array() {
        for (i, p) in parts.iter().enumerate() {
            if p["thought"].as_bool().unwrap_or(false) {
                continue;
            }
            if let Some(fc) = p.get("functionCall") {
                let Some(name) = fc["name"].as_str() else {
                    continue;
                };
                calls.push(ToolCall {
                    id: fc["id"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| synthetic_id(round, i)),
                    name: name.to_string(),
                    arguments: fc.get("args").cloned().unwrap_or_else(|| json!({})),
                    provider_meta: p.get("thoughtSignature").cloned(),
                });
            } else if let Some(t) = p["text"].as_str() {
                text.push_str(t);
            }
        }
    }
    let text = text.trim().to_string();
    if text.is_empty() && calls.is_empty() {
        let reason = candidate["finishReason"].as_str().unwrap_or("unknown");
        return Err(format!(
            "Gemini returned no text (finish reason: {reason})."
        ));
    }
    Ok(ModelTurn { text, calls })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> ToolDef {
        ToolDef {
            name: "github__search_issues".into(),
            description: "Search issues".into(),
            parameters: json!({
                "$schema": "http://json-schema.org/draft-07/schema#",
                "type": "object",
                "properties": {
                    "q": { "type": "string" },
                    "limit": { "type": ["integer", "null"], "default": 10 }
                },
                "required": ["q"],
                "additionalProperties": false
            }),
        }
    }

    fn convo() -> Vec<ConvMessage> {
        vec![
            ConvMessage::User("find bugs".into()),
            ConvMessage::Assistant {
                text: "Searching.".into(),
                calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "github__search_issues".into(),
                    arguments: json!({ "q": "bug" }),
                    provider_meta: Some(json!("sig123")),
                }],
            },
            ConvMessage::ToolResults(vec![ToolResult {
                call_id: "call_1".into(),
                name: "github__search_issues".into(),
                content: "2 issues".into(),
                is_error: false,
            }]),
        ]
    }

    #[test]
    fn prefixes_and_splits_names() {
        assert_eq!(prefixed_tool_name("github", "search"), "github__search");
        assert_eq!(prefixed_tool_name("my srv", "a.b"), "my_srv__a_b");
        assert_eq!(prefixed_tool_name("1st", "x"), "t_1st__x");
        let long = prefixed_tool_name("server", &"x".repeat(100));
        assert_eq!(long.len(), MAX_TOOL_NAME_LEN);
        assert_ne!(long, prefixed_tool_name("server", &"x".repeat(101)));
        assert_eq!(split_prefixed("gh__list__all"), Some(("gh", "list__all")));
        assert_eq!(split_prefixed("nosep"), None);
        assert!(validate_server_name("git-hub_2").is_ok());
        assert!(validate_server_name("a__b").is_err());
        assert!(validate_server_name("a b").is_err());
        assert!(validate_server_name("").is_err());
    }

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_output("abc", 10), "abc");
        let t = truncate_output("ééé", 3);
        assert!(t.starts_with('é'));
        assert!(t.contains("[truncated 4 bytes]"));
        assert_eq!(summarize("a\n  b   c", 10), "a b c");
        assert_eq!(summarize("abcdefghijkl", 5), "abcd…");
    }

    #[test]
    fn mcp_result_text_extraction() {
        let r = json!({
            "content": [
                { "type": "text", "text": "hello" },
                { "type": "image", "mimeType": "image/png", "data": "..." },
                { "type": "resource", "resource": { "uri": "file:///a", "text": "body" } }
            ],
            "isError": true
        });
        let (text, err) = mcp_result_to_text(&r);
        assert_eq!(text, "hello\n[image image/png]\nbody");
        assert!(err);
        let (text, err) =
            mcp_result_to_text(&json!({ "content": [], "structuredContent": { "n": 1 } }));
        assert_eq!(text, "{\"n\":1}");
        assert!(!err);
        assert_eq!(mcp_result_to_text(&json!({})).0, "(no output)");
    }

    #[test]
    fn openai_shape_and_parse() {
        let body = openai_body("gpt", "sys", &convo(), &[tool()]);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(
            msgs[2]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"bug\"}"
        );
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "call_1");
        assert_eq!(
            body["tools"][0]["function"]["name"],
            "github__search_issues"
        );
        assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
        // Tanpa tool: field `tools` tidak ada.
        assert!(
            openai_body("gpt", "sys", &convo(), &[])
                .get("tools")
                .is_none()
        );

        let resp = r#"{"choices":[{"message":{"content":null,"tool_calls":[
            {"id":"c9","type":"function","function":{"name":"gh__x","arguments":"{\"a\":1}"}},
            {"type":"function","function":{"name":"gh__y","arguments":""}}]}}]}"#;
        let turn = parse_response(AiApiStyle::OpenAiCompatible, resp, 2).unwrap();
        assert_eq!(turn.text, "");
        assert_eq!(turn.calls.len(), 2);
        assert_eq!(turn.calls[0].id, "c9");
        assert_eq!(turn.calls[0].arguments, json!({ "a": 1 }));
        assert_eq!(turn.calls[1].id, "tabular_call_2_1");
        assert_eq!(turn.calls[1].arguments, json!({}));
    }

    #[test]
    fn anthropic_shape_and_parse() {
        let body = anthropic_body("claude", "sys", &convo(), &[tool()]);
        assert_eq!(body["system"], "sys");
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!(msgs[1]["content"][1]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][1]["input"]["q"], "bug");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(body["tools"][0]["input_schema"]["required"][0], "q");

        let resp = r#"{"content":[{"type":"text","text":"Let me look."},
            {"type":"tool_use","id":"toolu_1","name":"gh__x","input":{"q":"a"}}],"stop_reason":"tool_use"}"#;
        let turn = parse_response(AiApiStyle::Anthropic, resp, 0).unwrap();
        assert_eq!(turn.text, "Let me look.");
        assert_eq!(turn.calls[0].id, "toolu_1");
        assert_eq!(turn.calls[0].arguments, json!({ "q": "a" }));
    }

    #[test]
    fn gemini_shape_and_parse() {
        let body = gemini_body("sys", &convo(), &[tool()]);
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(
            contents[1]["parts"][1]["functionCall"]["name"],
            "github__search_issues"
        );
        assert_eq!(contents[1]["parts"][1]["thoughtSignature"], "sig123");
        assert_eq!(
            contents[2]["parts"][0]["functionResponse"]["response"]["content"],
            "2 issues"
        );
        let decl = &body["tools"][0]["functionDeclarations"][0];
        let params = &decl["parameters"];
        assert!(params.get("$schema").is_none());
        assert!(params.get("additionalProperties").is_none());
        assert_eq!(params["properties"]["limit"]["type"], "integer");
        assert_eq!(params["properties"]["limit"]["nullable"], true);
        assert!(params["properties"]["limit"].get("default").is_none());

        let resp = r#"{"candidates":[{"content":{"parts":[
            {"text":"thinking","thought":true},
            {"functionCall":{"name":"gh__x","args":{"q":"a"}},"thoughtSignature":"S"}]}}]}"#;
        let turn = parse_response(AiApiStyle::Gemini, resp, 3).unwrap();
        assert_eq!(turn.text, "");
        assert_eq!(turn.calls[0].id, "tabular_call_3_1");
        assert_eq!(turn.calls[0].provider_meta, Some(json!("S")));
        // Id buatan tidak dikirim balik ke Gemini.
        let conv = vec![ConvMessage::ToolResults(vec![ToolResult {
            call_id: turn.calls[0].id.clone(),
            name: "gh__x".into(),
            content: "x".into(),
            is_error: true,
        }])];
        let body = gemini_body("", &conv, &[]);
        let fr = &body["contents"][0]["parts"][0]["functionResponse"];
        assert!(fr.get("id").is_none());
        assert_eq!(fr["response"]["error"], "x");
        assert!(body.get("tools").is_none());
        assert!(body.get("systemInstruction").is_none());
    }
}
