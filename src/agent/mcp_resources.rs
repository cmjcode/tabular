//! Resources, prompts, dan subscription MCP (checklist K3).
//!
//! Resource read-only dengan skema URI `tabular://`:
//!
//! | URI | Isi |
//! |---|---|
//! | `tabular://connections` | koneksi yang boleh dilihat klien ini |
//! | `tabular://connections/{id}/databases` | database / schema |
//! | `tabular://connections/{id}/schema{?database}` | skema lengkap (bisa di-subscribe) |
//! | `tabular://connections/{id}/tables{?database}` | daftar tabel dan view |
//! | `tabular://connections/{id}/tables/{table}{?database}` | kolom, FK, index |
//! | `tabular://connections/{id}/tables/{table}/ddl{?database}` | DDL tabel |
//! | `tabular://connections/{id}/history{?limit}` | query terbaru (password disamarkan) |
//! | `tabular://connections/{id}/diagram{?database}` | diagram Tabular |
//!
//! Prompt dirender dari skema live (cache Tabular) supaya model langsung
//! punya konteks yang benar. Semua pembacaan melewati gerbang akses yang sama
//! dengan tool (`HeadlessSession::ensure_access`).

use rmcp::model::{Prompt, PromptArgument, Resource, ResourceTemplate};
use serde::Serialize;
use sqlx::SqlitePool;

use super::core::{AgentError, HeadlessSession};

pub const SCHEME: &str = "tabular://";
const JSON_MIME: &str = "application/json";

/// Resource yang dikenali dari URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceUri {
    Connections,
    Databases {
        id: i64,
    },
    Schema {
        id: i64,
        database: Option<String>,
    },
    Tables {
        id: i64,
        database: Option<String>,
    },
    Table {
        id: i64,
        table: String,
        database: Option<String>,
    },
    TableDdl {
        id: i64,
        table: String,
        database: Option<String>,
    },
    History {
        id: i64,
        limit: Option<usize>,
    },
    Diagram {
        id: i64,
        database: Option<String>,
    },
}

impl ResourceUri {
    pub fn connection_id(&self) -> Option<i64> {
        match self {
            ResourceUri::Connections => None,
            ResourceUri::Databases { id }
            | ResourceUri::Schema { id, .. }
            | ResourceUri::Tables { id, .. }
            | ResourceUri::Table { id, .. }
            | ResourceUri::TableDdl { id, .. }
            | ResourceUri::History { id, .. }
            | ResourceUri::Diagram { id, .. } => Some(*id),
        }
    }

    /// Nama singkat untuk log aktivitas.
    pub fn label(&self) -> &'static str {
        match self {
            ResourceUri::Connections => "connections",
            ResourceUri::Databases { .. } => "databases",
            ResourceUri::Schema { .. } => "schema",
            ResourceUri::Tables { .. } => "tables",
            ResourceUri::Table { .. } => "table",
            ResourceUri::TableDdl { .. } => "ddl",
            ResourceUri::History { .. } => "history",
            ResourceUri::Diagram { .. } => "diagram",
        }
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Encode satu segmen path/nilai query (hanya karakter tak-aman).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Parse URI `tabular://...`. `None` bila tidak dikenali.
pub fn parse_uri(uri: &str) -> Option<ResourceUri> {
    let rest = uri.strip_prefix(SCHEME)?;
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let mut database = None;
    let mut limit = None;
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = percent_decode(v)?;
        match k {
            "database" if !v.trim().is_empty() => database = Some(v),
            "limit" => limit = v.trim().parse::<usize>().ok(),
            _ => {}
        }
    }
    let segs: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    match segs.as_slice() {
        ["connections"] => Some(ResourceUri::Connections),
        ["connections", id, rest @ ..] => {
            let id: i64 = id.parse().ok()?;
            match rest {
                ["databases"] => Some(ResourceUri::Databases { id }),
                ["schema"] => Some(ResourceUri::Schema { id, database }),
                ["tables"] => Some(ResourceUri::Tables { id, database }),
                ["tables", table] => Some(ResourceUri::Table {
                    id,
                    table: percent_decode(table)?,
                    database,
                }),
                ["tables", table, "ddl"] => Some(ResourceUri::TableDdl {
                    id,
                    table: percent_decode(table)?,
                    database,
                }),
                ["history"] => Some(ResourceUri::History { id, limit }),
                ["diagram"] => Some(ResourceUri::Diagram { id, database }),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn schema_uri(id: i64, database: Option<&str>) -> String {
    match database {
        Some(db) => format!(
            "{SCHEME}connections/{id}/schema?database={}",
            percent_encode(db)
        ),
        None => format!("{SCHEME}connections/{id}/schema"),
    }
}

/// Template resource untuk `resources/templates/list`.
pub fn templates() -> Vec<ResourceTemplate> {
    let t = |uri: &str, name: &str, desc: &str| {
        ResourceTemplate::new(uri, name)
            .with_description(desc)
            .with_mime_type(JSON_MIME)
    };
    vec![
        t(
            "tabular://connections/{id}/databases",
            "databases",
            "Databases / schemas known for a connection.",
        ),
        t(
            "tabular://connections/{id}/schema{?database}",
            "schema",
            "Tables, columns, keys and indexes of a database as compact DDL plus JSON. Subscribable: \
             an update notification is sent when the cached schema changes.",
        ),
        t(
            "tabular://connections/{id}/tables{?database}",
            "tables",
            "Table and view names of a database.",
        ),
        t(
            "tabular://connections/{id}/tables/{table}{?database}",
            "table",
            "Columns, primary key, foreign keys (both directions) and indexes of one table.",
        ),
        t(
            "tabular://connections/{id}/tables/{table}/ddl{?database}",
            "table_ddl",
            "CREATE statement of one table (from the server for MySQL/SQLite, reconstructed from \
             the cache otherwise).",
        ),
        t(
            "tabular://connections/{id}/history{?limit}",
            "history",
            "Most recent queries run in Tabular on this connection, passwords masked.",
        ),
        t(
            "tabular://connections/{id}/diagram{?database}",
            "diagram",
            "The user's Tabular diagram: groups, virtual relations and sticky notes.",
        ),
    ]
}

fn resource(uri: String, name: String, desc: String) -> Resource {
    Resource::new(uri, name)
        .with_description(desc)
        .with_mime_type(JSON_MIME)
}

/// Resource konkret untuk `resources/list`.
pub async fn list(session: &HeadlessSession, client: &str) -> Result<Vec<Resource>, AgentError> {
    let mut out = vec![resource(
        format!("{SCHEME}connections"),
        "connections".to_string(),
        "Connections this client may use, with their agent access level.".to_string(),
    )];
    for c in session.list_connections_for(client).await? {
        let id = c.summary.id;
        let name = &c.summary.name;
        out.push(resource(
            schema_uri(id, None),
            format!("{name}: schema"),
            format!("Schema of {name} ({}), default database.", c.summary.kind),
        ));
        out.push(resource(
            format!("{SCHEME}connections/{id}/tables"),
            format!("{name}: tables"),
            format!("Tables and views of {name}."),
        ));
        out.push(resource(
            format!("{SCHEME}connections/{id}/history"),
            format!("{name}: history"),
            format!("Recent queries on {name}."),
        ));
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryEntry {
    pub query: String,
    pub executed_at: String,
    /// `true` bila dijalankan oleh agent.
    pub agent: bool,
}

/// Query terbaru untuk satu koneksi, password disamarkan.
pub async fn recent_history(
    pool: &SqlitePool,
    id: i64,
    limit: usize,
) -> Result<Vec<HistoryEntry>, AgentError> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT query_text, COALESCE(executed_at, ''), connection_name FROM query_history \
         WHERE connection_id = ? ORDER BY id DESC LIMIT ?",
    )
    .bind(id)
    .bind(limit.clamp(1, 500) as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(q, at, name)| HistoryEntry {
            query: super::knowledge::redact_sql_secrets(&q),
            executed_at: at,
            agent: name.ends_with("(agent)"),
        })
        .collect())
}

fn to_json<T: Serialize>(v: &T) -> Result<String, AgentError> {
    serde_json::to_string_pretty(v)
        .map_err(|e| AgentError::Io(std::io::Error::other(format!("serialize: {e}"))))
}

/// Baca resource sebagai teks JSON. Pemanggil sudah memeriksa akses koneksi.
pub async fn read(
    session: &HeadlessSession,
    client: &str,
    uri: &ResourceUri,
) -> Result<String, AgentError> {
    match uri {
        ResourceUri::Connections => to_json(&session.list_connections_for(client).await?),
        ResourceUri::Databases { id } => to_json(&session.list_databases(*id).await?),
        ResourceUri::Schema { id, database } => to_json(
            &session
                .describe_schema(*id, database.as_deref(), None, Some(200))
                .await?,
        ),
        ResourceUri::Tables { id, database } => to_json(
            &session
                .list_tables(*id, database.as_deref(), None, Some(2000))
                .await?,
        ),
        ResourceUri::Table {
            id,
            table,
            database,
        } => to_json(
            &session
                .describe_table(*id, database.as_deref(), table)
                .await?,
        ),
        ResourceUri::TableDdl {
            id,
            table,
            database,
        } => to_json(&session.table_ddl(*id, database.as_deref(), table).await?),
        ResourceUri::History { id, limit } => {
            to_json(&recent_history(session.cache_pool(), *id, limit.unwrap_or(50)).await?)
        }
        ResourceUri::Diagram { id, database } => to_json(
            &session
                .describe_diagram(*id, database.as_deref(), None, None)
                .await?,
        ),
    }
}

// ── Subscription ────────────────────────────────────────────────────────────

/// Sidik skema dari cache: berubah bila tabel/kolom/tipe berubah. Bukan hash
/// kriptografis; cukup untuk mendeteksi refresh cache yang mengubah isi.
pub async fn schema_fingerprint(pool: &SqlitePool, id: i64, database: Option<&str>) -> String {
    let row: Result<(i64, i64), sqlx::Error> = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM((LENGTH(table_name) * 7 + LENGTH(column_name) * 13 \
                + LENGTH(data_type) * 31 + ordinal_position * 17 + is_primary_key * 101) \
                % 1000003), 0) \
         FROM column_cache WHERE connection_id = ? \
           AND (? IS NULL OR database_name = ? COLLATE NOCASE)",
    )
    .bind(id)
    .bind(database)
    .bind(database)
    .fetch_one(pool)
    .await;
    match row {
        Ok((n, sum)) => format!("{n}:{sum}"),
        Err(e) => {
            log::debug!("[AGENT] schema fingerprint failed: {e}");
            String::new()
        }
    }
}

/// Sidik daftar koneksi + akses, untuk `resources/list_changed`.
pub async fn connections_fingerprint(pool: &SqlitePool) -> String {
    let conns: Result<(i64, String), sqlx::Error> = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(GROUP_CONCAT(id || ':' || name, ','), '') \
         FROM (SELECT id, name FROM connections ORDER BY id)",
    )
    .fetch_one(pool)
    .await;
    let access = super::access::load_all(pool).await.map(|m| {
        let mut v: Vec<String> = m
            .into_iter()
            .map(|(id, a)| format!("{id}={}:{:?}", a.level.key(), a.allowed_clients))
            .collect();
        v.sort();
        v.join(",")
    });
    format!(
        "{:?}|{:?}",
        conns.map_err(|e| e.to_string()),
        access.map_err(|e| e.to_string())
    )
}

// ── Prompts ─────────────────────────────────────────────────────────────────

fn arg(name: &str, desc: &str, required: bool) -> PromptArgument {
    PromptArgument::new(name)
        .with_description(desc)
        .with_required(required)
}

fn conn_arg() -> PromptArgument {
    arg(
        "connection_id",
        "Connection id from list_connections.",
        true,
    )
}

fn db_arg() -> PromptArgument {
    arg(
        "database",
        "Database / schema. Defaults to the connection's default database.",
        false,
    )
}

/// Delapan prompt yang dirender dari skema live.
pub fn prompt_defs() -> Vec<Prompt> {
    let p = |name: &str, desc: &str, args: Vec<PromptArgument>| {
        Prompt::new(name, Some(desc), Some(args))
    };
    vec![
        p(
            "explain_schema",
            "Guided tour of a database: domains, central tables and how they join.",
            vec![
                conn_arg(),
                db_arg(),
                arg(
                    "audience",
                    "Who the explanation is for, e.g. \"new backend engineer\" or \"analyst\".",
                    false,
                ),
            ],
        ),
        p(
            "explain_table",
            "Explain one table's purpose, columns, keys and relationships, with three useful queries.",
            vec![
                conn_arg(),
                arg("table", "Table name.", true),
                db_arg(),
                arg("audience", "Who the explanation is for.", false),
            ],
        ),
        p(
            "data_quality_audit",
            "Data-quality checklist for one table with runnable read-only queries, cheapest first.",
            vec![conn_arg(), arg("table", "Table name.", true), db_arg()],
        ),
        p(
            "question_to_sql",
            "Turn a question into one read-only SQL query using only columns that exist.",
            vec![
                conn_arg(),
                arg("question", "The question in plain language.", true),
                db_arg(),
            ],
        ),
        p(
            "review_query",
            "Review a query for correctness, cost and risk, using its tables' structure.",
            vec![
                conn_arg(),
                arg("query", "The SQL to review.", true),
                db_arg(),
            ],
        ),
        p(
            "propose_indexes",
            "Ranked CREATE INDEX proposals for a query, noting existing and redundant indexes.",
            vec![conn_arg(), arg("query", "The slow SQL.", true), db_arg()],
        ),
        p(
            "write_migration",
            "Migration, rollback and verification query for a schema change.",
            vec![
                conn_arg(),
                arg(
                    "change",
                    "The change you want, e.g. \"add a nullable archived_at to orders\".",
                    true,
                ),
                arg("table", "Main table affected, if known.", false),
                db_arg(),
            ],
        ),
        p(
            "summarize_query_history",
            "Summarize recent query history on a connection by intent; highlight writes and failures.",
            vec![
                conn_arg(),
                arg(
                    "limit",
                    "How many recent queries to include (default 100, max 500).",
                    false,
                ),
            ],
        ),
    ]
}

/// Argumen prompt (MCP mengirim argumen prompt sebagai string; angka juga diterima).
pub struct PromptArgs<'a>(pub Option<&'a serde_json::Map<String, serde_json::Value>>);

impl PromptArgs<'_> {
    pub fn get(&self, key: &str) -> Option<String> {
        let v = self.0?.get(key)?;
        let s = match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => return None,
        };
        let s = s.trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    pub fn required(&self, key: &str) -> Result<String, AgentError> {
        self.get(key)
            .ok_or_else(|| AgentError::Refused(format!("prompt argument `{key}` is required")))
    }

    pub fn connection_id(&self) -> Result<i64, AgentError> {
        self.required("connection_id")?
            .parse::<i64>()
            .map_err(|_| AgentError::Refused("`connection_id` must be a number".to_string()))
    }
}

/// Render prompt menjadi (deskripsi, teks pesan user). Pemanggil sudah
/// memeriksa akses ke `connection_id`.
pub async fn render_prompt(
    session: &HeadlessSession,
    name: &str,
    args: &PromptArgs<'_>,
) -> Result<(String, String), AgentError> {
    let id = args.connection_id()?;
    let database = args.get("database");
    let db = database.as_deref();
    let engine = session
        .list_connections()
        .await?
        .into_iter()
        .find(|c| c.id == id)
        .map(|c| c.kind)
        .unwrap_or_default();
    let audience = args
        .get("audience")
        .map(|a| format!(" Write for this audience: {a}."))
        .unwrap_or_default();
    let fence = |label: &str, body: &str| format!("{label}:\n```\n{}\n```\n", body.trim_end());

    let text = match name {
        "explain_schema" => {
            let s = session.describe_schema(id, db, None, Some(80)).await?;
            format!(
                "Give me a guided tour of the {engine} database `{}` ({} tables, {} shown).{audience}\n\
                 Group the tables into business domains, name the central tables, explain how they join \
                 (including VIRTUAL relations from the diagram), and point out anything surprising. \
                 Use only what the schema below shows; say so when you are guessing.\n\n{}",
                s.database,
                s.total_tables,
                s.shown_tables,
                fence("Schema", &s.ddl)
            )
        }
        "explain_table" => {
            let table = args.required("table")?;
            let ddl = session.table_ddl(id, db, &table).await?;
            let detail = session.describe_table(id, db, &table).await?;
            let refs: Vec<String> = detail
                .referenced_by
                .iter()
                .map(|r| format!("{}.{} -> {}", r.table, r.column, r.references_column))
                .collect();
            format!(
                "Explain the {engine} table `{}`: what one row represents, what each column means, \
                 its keys and how it relates to other tables.{audience} Then propose three read-only \
                 queries worth running on it, each with one sentence on what it shows. Add LIMIT.\n\n{}\
                 Referenced by: {}\n",
                ddl.table,
                fence("DDL", &ddl.ddl),
                if refs.is_empty() {
                    "nothing cached".to_string()
                } else {
                    refs.join(", ")
                }
            )
        }
        "data_quality_audit" => {
            let table = args.required("table")?;
            let ddl = session.table_ddl(id, db, &table).await?;
            format!(
                "Build a data-quality audit for the {engine} table `{}`. For each check (NULLs in \
                 columns that look mandatory, duplicates on natural keys, orphaned foreign keys, \
                 out-of-range dates or amounts, inconsistent enums/status values, whitespace or case \
                 variants) give one runnable read-only query. Order the checks from cheapest to most \
                 expensive and warn about full scans on large tables. Do not modify data.\n\n{}",
                ddl.table,
                fence("DDL", &ddl.ddl)
            )
        }
        "question_to_sql" => {
            let question = args.required("question")?;
            let s = session
                .describe_schema(id, db, Some(&question), Some(30))
                .await?;
            format!(
                "Write one read-only {engine} query that answers: {question}\n\
                 Rules: use only tables and columns that appear in the schema below, never invent \
                 one; if the schema cannot answer the question, say what is missing instead. Always \
                 add a LIMIT (or TOP for SQL Server). Explain the joins you chose in one or two \
                 sentences.\n\n{}",
                fence("Schema (most relevant tables first)", &s.ddl)
            )
        }
        "review_query" | "propose_indexes" => {
            let query = args.required("query")?;
            let analysis = session.analyze_query(Some(id), &query, db).await?;
            let s = session
                .describe_schema(id, db, Some(&query), Some(12))
                .await?;
            let analysis_json = serde_json::to_string_pretty(&analysis).unwrap_or_default();
            let ask = if name == "review_query" {
                "Review this query for correctness (joins, filters, NULL handling, duplicates), cost \
                 (scans, missing indexes, sorts) and risk (writes without WHERE, locking). Suggest a \
                 corrected or faster version when useful."
            } else {
                "Propose indexes that would speed up this query, ranked by expected benefit. Give \
                 each as a CREATE INDEX statement with one sentence of reasoning, note existing \
                 indexes that already cover it, and flag redundant indexes."
            };
            format!(
                "{ask} The database is {engine}.\n\n{}{}{}",
                fence("Query", &query),
                fence("Static analysis by Tabular", &analysis_json),
                fence("Tables involved (with cached indexes)", &s.ddl)
            )
        }
        "write_migration" => {
            let change = args.required("change")?;
            let context = match args.get("table") {
                Some(t) => session.table_ddl(id, db, &t).await?.ddl,
                None => {
                    session
                        .describe_schema(id, db, Some(&change), Some(12))
                        .await?
                        .ddl
                }
            };
            format!(
                "Write a {engine} migration for this change: {change}\n\
                 Give (1) the forward migration, (2) a rollback, and (3) a read-only query that \
                 verifies the result. Keep it safe for a live database: call out locks, table \
                 rewrites and backfills, and split risky steps.\n\n{}",
                fence("Current structure", &context)
            )
        }
        "summarize_query_history" => {
            let limit = args
                .get("limit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(100);
            let rows = recent_history(session.cache_pool(), id, limit).await?;
            let mut body = String::new();
            for r in &rows {
                let q: String = r.query.chars().take(400).collect();
                body.push_str(&format!(
                    "[{}]{} {}\n",
                    r.executed_at,
                    if r.agent { " (agent)" } else { "" },
                    q.replace('\n', " ")
                ));
            }
            format!(
                "Summarize this {engine} query history ({} most recent statements) by intent: group \
                 similar queries, name what the user was working on, and highlight every statement \
                 that writes or changes schema and anything run by an agent.\n\n{}",
                rows.len(),
                fence("History (newest first)", &body)
            )
        }
        other => {
            return Err(AgentError::Refused(format!(
                "unknown prompt `{other}`; see prompts/list"
            )));
        }
    };
    let description = prompt_defs()
        .into_iter()
        .find(|p| p.name == name)
        .and_then(|p| p.description)
        .unwrap_or_default();
    Ok((description, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_uri_shapes() {
        assert_eq!(
            parse_uri("tabular://connections"),
            Some(ResourceUri::Connections)
        );
        assert_eq!(
            parse_uri("tabular://connections/3/databases"),
            Some(ResourceUri::Databases { id: 3 })
        );
        assert_eq!(
            parse_uri("tabular://connections/3/schema?database=shop%20db"),
            Some(ResourceUri::Schema {
                id: 3,
                database: Some("shop db".into())
            })
        );
        assert_eq!(
            parse_uri("tabular://connections/3/tables/public.Orders%2FX/ddl"),
            Some(ResourceUri::TableDdl {
                id: 3,
                table: "public.Orders/X".into(),
                database: None
            })
        );
        assert_eq!(
            parse_uri("tabular://connections/3/history?limit=5"),
            Some(ResourceUri::History {
                id: 3,
                limit: Some(5)
            })
        );
        assert_eq!(parse_uri("tabular://connections/x/schema"), None);
        assert_eq!(parse_uri("tabular://other"), None);
        assert_eq!(parse_uri("file:///etc/passwd"), None);
    }

    #[test]
    fn encode_roundtrip() {
        let name = "we ird/näme";
        let uri = format!("tabular://connections/1/tables/{}", percent_encode(name));
        assert_eq!(
            parse_uri(&uri),
            Some(ResourceUri::Table {
                id: 1,
                table: name.into(),
                database: None
            })
        );
        assert_eq!(
            parse_uri(&schema_uri(2, Some("a&b"))),
            Some(ResourceUri::Schema {
                id: 2,
                database: Some("a&b".into())
            })
        );
    }

    #[test]
    fn eight_prompts_with_required_connection() {
        let defs = prompt_defs();
        assert_eq!(defs.len(), 8);
        for p in &defs {
            let args = p.arguments.as_ref().expect("arguments");
            assert!(
                args.iter()
                    .any(|a| a.name == "connection_id" && a.required == Some(true)),
                "{} needs connection_id",
                p.name
            );
        }
        assert_eq!(templates().len(), 7);
    }

    #[test]
    fn prompt_args_accept_strings_and_numbers() {
        let mut m = serde_json::Map::new();
        m.insert("connection_id".into(), serde_json::json!(4));
        m.insert("table".into(), serde_json::json!("  orders "));
        m.insert("empty".into(), serde_json::json!(" "));
        let a = PromptArgs(Some(&m));
        assert_eq!(a.connection_id().expect("id"), 4);
        assert_eq!(a.get("table").as_deref(), Some("orders"));
        assert!(a.get("empty").is_none());
        assert!(a.required("question").is_err());
        assert!(PromptArgs(None).connection_id().is_err());
    }

    #[tokio::test]
    async fn fingerprints_change_with_cache() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("pool");
        sqlx::query(
            "CREATE TABLE column_cache (connection_id INTEGER, database_name TEXT, table_name TEXT, \
             column_name TEXT, data_type TEXT, ordinal_position INTEGER, is_primary_key INTEGER)",
        )
        .execute(&pool)
        .await
        .expect("create");
        let empty = schema_fingerprint(&pool, 1, None).await;
        sqlx::query("INSERT INTO column_cache VALUES (1, 'shop', 'orders', 'id', 'int', 1, 1)")
            .execute(&pool)
            .await
            .expect("insert");
        let one = schema_fingerprint(&pool, 1, Some("shop")).await;
        assert_ne!(empty, one);
        sqlx::query("UPDATE column_cache SET data_type = 'bigint'")
            .execute(&pool)
            .await
            .expect("update");
        assert_ne!(one, schema_fingerprint(&pool, 1, Some("shop")).await);
        assert_eq!(schema_fingerprint(&pool, 2, None).await, "0:0");
    }
}
