use std::sync::mpsc;
use serde_json::json;

use crate::config::AiProvider;

/// Build a schema context string from the active connection's cached tables + columns.
/// Returns an empty string if cache is empty or no connection is active.
/// Caps at `max_tables` tables to avoid bloating the prompt.
pub fn build_schema_context(tabular: &crate::window_egui::Tabular, max_tables: usize) -> String {
    let conn_id = match tabular.current_connection_id {
        Some(id) => id,
        None => return String::new(),
    };

    // Determine database name from active tab
    let db_name: String = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.database_name.clone())
        .unwrap_or_default();

    if db_name.is_empty() {
        // Try to pick first available database from in-memory cache
        if let Some(dbs) = tabular.database_cache.get(&conn_id) {
            if let Some(first_db) = dbs.first() {
                return build_schema_for_db(tabular, conn_id, first_db, max_tables);
            }
        }
        return String::new();
    }

    build_schema_for_db(tabular, conn_id, &db_name, max_tables)
}

fn build_schema_for_db(
    tabular: &crate::window_egui::Tabular,
    conn_id: i64,
    db_name: &str,
    max_tables: usize,
) -> String {
    // Fetch tables from cache
    let tables = match crate::cache_data::get_tables_from_cache(tabular, conn_id, db_name, "table") {
        Some(t) if !t.is_empty() => t,
        _ => return String::new(),
    };

    let mut out = format!("-- Database: {db_name}\n");

    for table in tables.iter().take(max_tables) {
        out.push_str(&format!("-- Table: {table}\n"));

        if let Some(cols) = crate::cache_data::get_columns_from_cache(tabular, conn_id, db_name, table) {
            if cols.is_empty() {
                out.push_str("--   (no columns cached)\n");
            } else {
                let col_list: Vec<String> = cols
                    .iter()
                    .map(|(name, typ)| format!("  {name} {typ}"))
                    .collect();
                out.push_str(&format!("CREATE TABLE {table} (\n{}\n);\n", col_list.join(",\n")));
            }
        } else {
            out.push_str(&format!("-- Table {table}: (columns not cached yet — browse the table first)\n"));
        }
        out.push('\n');
    }

    if tables.len() > max_tables {
        out.push_str(&format!(
            "-- ... and {} more tables (showing first {max_tables})\n",
            tables.len() - max_tables
        ));
    }

    out
}

/// Request an AI suggestion asynchronously.
/// Returns a receiver that will yield `Ok(suggestion)` or `Err(message)`.
pub fn request_ai_suggestion(
    provider: AiProvider,
    api_key: String,
    model: String,
    base_url: String,
    system_prompt: String,
    user_prompt: String,
) -> mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::channel();

    let effective_model = if model.is_empty() {
        provider.default_model().to_string()
    } else {
        model
    };

    let effective_base_url = if base_url.is_empty() {
        provider.default_base_url().to_string()
    } else {
        base_url
    };

    std::thread::spawn(move || {
        let result = match provider {
            AiProvider::Anthropic => call_anthropic(
                &api_key,
                &effective_model,
                &effective_base_url,
                &system_prompt,
                &user_prompt,
            ),
            // OpenAI, GitHub, Groq, and Custom all use the OpenAI-compatible /chat/completions endpoint
            _ => call_openai_compatible(
                &api_key,
                &effective_model,
                &effective_base_url,
                &system_prompt,
                &user_prompt,
            ),
        };
        let _ = tx.send(result);
    });

    rx
}

fn call_openai_compatible(
    api_key: &str,
    model: &str,
    base_url: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, String> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": user_prompt }
        ],
        "temperature": 0.2,
        "max_tokens": 1024
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().map_err(|e| format!("Failed to read response: {e}"))?;

    if !status.is_success() {
        return Err(format!("API error {status}: {text}"));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse response: {e}"))?;

    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("Unexpected response format: {text}"))
}

fn call_anthropic(
    api_key: &str,
    model: &str,
    base_url: &str,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, String> {
    let url = format!("{}/messages", base_url.trim_end_matches('/'));

    let body = json!({
        "model": model,
        "system": system_prompt,
        "messages": [
            { "role": "user", "content": user_prompt }
        ],
        "max_tokens": 1024
    });

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("Request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().map_err(|e| format!("Failed to read response: {e}"))?;

    if !status.is_success() {
        return Err(format!("Anthropic API error {status}: {text}"));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse response: {e}"))?;

    json["content"][0]["text"]
        .as_str()
        .map(|s| s.trim().to_string())
        .ok_or_else(|| format!("Unexpected Anthropic response format: {text}"))
}

/// Build a SQL-focused system prompt, optionally including database schema.
pub fn sql_system_prompt_with_schema(schema: &str) -> String {
    let base = "You are an expert SQL assistant embedded in Tabular, a database GUI tool. \
                Help the user write, explain, optimize, or debug SQL queries. \
                Be concise. When you write SQL, wrap it in a ```sql code block. \
                Do not repeat the user's query unless asked.";

    if schema.is_empty() {
        base.to_string()
    } else {
        format!(
            "{base}\n\nThe user's active database schema is provided below for reference. \
             Use it to write accurate table/column names in queries:\n\n{schema}"
        )
    }
}

/// Legacy alias kept for any remaining call sites.
pub fn sql_system_prompt() -> String {
    sql_system_prompt_with_schema("")
}
