//! Server Model Context Protocol (stdio) di atas [`HeadlessSession`].
//!
//! Semua tool bersifat read-only. Hasil dikembalikan sebagai
//! `structured_content` JSON sekaligus teks, supaya harness yang belum
//! mendukung structured output tetap bisa membacanya.
//!
//! Kesalahan yang "milik agent" (query ditolak, koneksi tidak ada, SQL salah)
//! dikembalikan sebagai tool error (`is_error = true`) dengan pesan yang bisa
//! ditindaklanjuti, bukan sebagai error protokol; ini sesuai anjuran spec MCP.

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use super::core::{AgentError, HeadlessSession};

const INSTRUCTIONS: &str = "\
Tabular gives you read-only access to the databases the user has already \
configured in the Tabular desktop app (PostgreSQL, MySQL/MariaDB, SQLite, \
SQL Server, Redis). Credentials, SSH tunnels and TLS are handled by Tabular; \
you only ever see connection ids.

Workflow: list_connections -> describe_schema(connection_id, question) -> \
run_query. Use check_sql_safety before proposing any INSERT/UPDATE/DELETE/DDL \
to the user: those statements are refused here and must be run by the user in \
the Tabular app. Results are truncated (default 200 rows, 500 chars per cell); \
add LIMIT and select only the columns you need. Every query you run is recorded \
in the user's Tabular history, tagged \"(agent)\".";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionArg {
    /// Connection id from list_connections.
    pub connection_id: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DescribeSchemaArgs {
    /// Connection id from list_connections.
    pub connection_id: i64,
    /// Database / schema name. Defaults to the connection's default database.
    #[serde(default)]
    pub database: Option<String>,
    /// What you are trying to answer. When the database has more tables than
    /// fit, the most relevant tables for this question are returned first.
    #[serde(default)]
    pub question: Option<String>,
    /// Maximum number of tables to include (default 40, max 500).
    #[serde(default)]
    pub max_tables: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RunQueryArgs {
    /// Connection id from list_connections.
    pub connection_id: i64,
    /// SQL (or a Redis command line, one command per line). Read-only only.
    pub sql: String,
    /// Database / schema to run against. Defaults to the connection's default.
    #[serde(default)]
    pub database: Option<String>,
    /// Maximum rows to return (default 200, hard cap 200).
    #[serde(default)]
    pub max_rows: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExplainArgs {
    /// Connection id from list_connections.
    pub connection_id: i64,
    /// A single read-only statement. Do not include the EXPLAIN keyword.
    pub sql: String,
    /// Database / schema to run against.
    #[serde(default)]
    pub database: Option<String>,
    /// Actually execute the statement to get real timings (EXPLAIN ANALYZE).
    /// Only allowed for read-only statements. Default false.
    #[serde(default)]
    pub analyze: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SqlArg {
    /// SQL text, may contain several statements separated by `;`.
    pub sql: String,
    /// Optional connection id, used to pick the dialect. Defaults to PostgreSQL.
    #[serde(default)]
    pub connection_id: Option<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FormatSqlArgs {
    /// SQL text to format.
    pub sql: String,
    /// Keyword casing: "upper" (default), "lower", or "preserve".
    #[serde(default)]
    pub keyword_case: Option<String>,
}

#[derive(Clone)]
pub struct TabularMcp {
    session: Arc<HeadlessSession>,
    tool_router: ToolRouter<Self>,
}

fn ok_json<T: serde::Serialize>(value: T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_value(value)
        .map_err(|e| McpError::internal_error(format!("serialize result: {e}"), None))?;
    Ok(CallToolResult::structured(json))
}

/// Error yang bisa ditindaklanjuti agent menjadi tool error; error internal
/// (cache lokal rusak, I/O) menjadi error protokol.
fn map_err(err: AgentError) -> Result<CallToolResult, McpError> {
    match err {
        AgentError::Cache(e) => Err(McpError::internal_error(e.to_string(), None)),
        AgentError::Io(e) => Err(McpError::internal_error(e.to_string(), None)),
        other => Ok(CallToolResult::error(vec![ContentBlock::text(
            other.to_string(),
        )])),
    }
}

fn finish<T: serde::Serialize>(res: Result<T, AgentError>) -> Result<CallToolResult, McpError> {
    match res {
        Ok(v) => ok_json(v),
        Err(e) => map_err(e),
    }
}

#[tool_router]
impl TabularMcp {
    pub fn new(session: Arc<HeadlessSession>) -> Self {
        Self {
            session,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List the database connections saved in Tabular. Returns id, name, kind (PostgreSQL/MySQL/SQLite/MsSQL/Redis/MongoDB), host, default database and whether run_query is supported. Never returns credentials."
    )]
    async fn list_connections(&self) -> Result<CallToolResult, McpError> {
        finish(self.session.list_connections().await)
    }

    #[tool(
        description = "List the databases / schemas known for a connection. Fetches from the server if Tabular has not cached them yet."
    )]
    async fn list_databases(
        &self,
        Parameters(p): Parameters<ConnectionArg>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.session.list_databases(p.connection_id).await)
    }

    #[tool(
        description = "Describe tables, columns, primary keys and foreign keys of a database as compact DDL plus structured JSON. Pass `question` so the most relevant tables come first when the schema is large. Uses Tabular's local schema cache; call refresh_schema_cache if it looks stale."
    )]
    async fn describe_schema(
        &self,
        Parameters(p): Parameters<DescribeSchemaArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(
            self.session
                .describe_schema(
                    p.connection_id,
                    p.database.as_deref(),
                    p.question.as_deref(),
                    p.max_tables,
                )
                .await,
        )
    }

    #[tool(
        description = "Re-fetch the schema (databases, tables, columns, indexes, foreign keys) from the server into Tabular's cache. Returns the number of cached tables."
    )]
    async fn refresh_schema_cache(
        &self,
        Parameters(p): Parameters<ConnectionArg>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.session.refresh_schema_cache(p.connection_id).await)
    }

    #[tool(
        description = "Run a READ-ONLY query (SELECT/SHOW/EXPLAIN, or read-only Redis commands) and return columns and rows. Writes, DDL and session commands are refused. Results are truncated to max_rows (<=200) and 500 chars per cell; add LIMIT."
    )]
    async fn run_query(
        &self,
        Parameters(p): Parameters<RunQueryArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(
            self.session
                .run_query(p.connection_id, &p.sql, p.database.as_deref(), p.max_rows)
                .await,
        )
    }

    #[tool(
        description = "Get the execution plan of a read-only statement (PostgreSQL, MySQL, SQLite) parsed into a tree with cost percentages, detected bottlenecks and warnings such as sequential scans on large tables."
    )]
    async fn explain_query(
        &self,
        Parameters(p): Parameters<ExplainArgs>,
    ) -> Result<CallToolResult, McpError> {
        finish(
            self.session
                .explain_query(p.connection_id, &p.sql, p.database.as_deref(), p.analyze)
                .await,
        )
    }

    #[tool(
        description = "Classify each statement as read/write/ddl/admin, flag UPDATE/DELETE without WHERE, and lint the SQL, without executing anything. Use it before proposing a write to the user."
    )]
    async fn check_sql_safety(
        &self,
        Parameters(p): Parameters<SqlArg>,
    ) -> Result<CallToolResult, McpError> {
        finish(self.session.check_sql_safety(p.connection_id, &p.sql).await)
    }

    #[tool(description = "Format SQL with Tabular's formatter (indentation and keyword casing).")]
    async fn format_sql(
        &self,
        Parameters(p): Parameters<FormatSqlArgs>,
    ) -> Result<CallToolResult, McpError> {
        use crate::models::enums::KeywordCasing;
        let casing = match p
            .keyword_case
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("lower") => KeywordCasing::Lower,
            Some("preserve") => KeywordCasing::Preserve,
            _ => KeywordCasing::Upper,
        };
        match crate::query_tools::format_sql_with_casing(&p.sql, casing) {
            Some(formatted) => ok_json(serde_json::json!({ "sql": formatted })),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(
                "nothing to format (empty input)",
            )])),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for TabularMcp {
    fn get_info(&self) -> ServerConfig {
        let mut info = Implementation::default();
        info.name = "tabular".to_string();
        info.version = env!("CARGO_PKG_VERSION").to_string();
        info.title = Some("Tabular".to_string());
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(info)
            .with_instructions(INSTRUCTIONS)
    }
}

/// Layani MCP lewat stdin/stdout sampai client menutup koneksi.
pub async fn serve_stdio(session: Arc<HeadlessSession>) -> Result<(), String> {
    let server = TabularMcp::new(session)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| format!("MCP handshake failed: {e}"))?;
    server
        .waiting()
        .await
        .map_err(|e| format!("MCP server stopped with error: {e}"))?;
    log::info!("[AGENT] MCP client disconnected");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_expected_tools_with_schemas() {
        let router = TabularMcp::tool_router();
        let mut names: Vec<String> = router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "check_sql_safety",
                "describe_schema",
                "explain_query",
                "format_sql",
                "list_connections",
                "list_databases",
                "refresh_schema_cache",
                "run_query",
            ]
        );
        for tool in router.list_all() {
            assert!(
                tool.description.as_deref().is_some_and(|d| !d.is_empty()),
                "{} needs a description",
                tool.name
            );
        }
        let run = router
            .list_all()
            .into_iter()
            .find(|t| t.name == "run_query")
            .expect("run_query");
        let props = run.input_schema.get("properties").expect("properties");
        assert!(props.get("sql").is_some());
        assert!(props.get("connection_id").is_some());
    }
}
