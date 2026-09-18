# Tabular as an MCP server (for AI agent harnesses)

Tabular ships a built-in [Model Context Protocol](https://modelcontextprotocol.io) server so
coding agents such as Claude Code, Cursor, Codex CLI, Windsurf and any other MCP client can
inspect and query the databases you have already configured in the app.

The design goal is simple: **the agent never sees a credential, and it can never write.**
Tabular keeps passwords, SSH keys and TLS material in the OS keychain, opens the SSH tunnel or
mTLS session itself, and hands the agent only connection ids. Every statement passes a
read-only gate before it reaches a driver. Anything else is refused with a message that tells
the agent to ask you to run it in the app.

## Quick start

```bash
# 1. Open Tabular once and add your connections as usual.
# 2. Register the server with your harness. Print a ready-made snippet:
tabular mcp --print-config
```

Claude Code:

```bash
claude mcp add tabular -- /path/to/tabular mcp
```

Cursor (`~/.cursor/mcp.json`), Windsurf and most other clients accept the JSON that
`--print-config` prints:

```json
{
  "mcpServers": {
    "tabular": { "command": "/path/to/tabular", "args": ["mcp"] }
  }
}
```

Codex CLI (`~/.codex/config.toml`):

```toml
[mcp_servers.tabular]
command = "/path/to/tabular"
args = ["mcp"]
```

On macOS the binary lives inside the bundle:
`/Applications/Tabular.app/Contents/MacOS/tabular`. On Linux and Windows point at the
installed `tabular` executable. The server speaks MCP over stdio; it opens no network port.

## Tools

| Tool | Purpose |
|---|---|
| `list_connections` | Saved connections: id, name, kind, host, default database, `supports_query`. No secrets. |
| `list_databases` | Databases / schemas known for a connection (fetches from the server on first use). |
| `describe_schema` | Tables, columns, primary keys and foreign keys as compact DDL plus JSON. Pass `question` and the most relevant tables are returned first, ranked by Tabular's local vector index (nothing leaves your machine). |
| `refresh_schema_cache` | Re-fetch schema metadata from the server into Tabular's cache. |
| `run_query` | Execute a **read-only** statement (or read-only Redis commands) and return rows. |
| `explain_query` | Execution plan for PostgreSQL, MySQL and SQLite, parsed into a tree with cost percentages and bottleneck warnings from Tabular's query profiler. |
| `check_sql_safety` | Classify each statement (read / write / ddl / admin), flag `UPDATE`/`DELETE` without `WHERE`, and lint, without executing. |
| `format_sql` | Format SQL with Tabular's formatter. |

Supported for `run_query`: PostgreSQL, MySQL/MariaDB, SQLite, SQL Server, Redis.
MongoDB and HTTP connections are listed but cannot run queries yet.

## Safety model

- **Read-only gate.** Every statement is classified before execution
  (`src/agent/classify.rs`). `SELECT`, `WITH ... SELECT`, `SHOW`, `DESCRIBE`, `EXPLAIN` and
  read-only Redis commands pass. `INSERT`/`UPDATE`/`DELETE`/`MERGE`/`TRUNCATE`/`CALL`, all DDL,
  and session or server commands (`SET`, `USE`, `BEGIN`, `COMMIT`, `KILL`, `PRAGMA`, ...) are
  refused. `SELECT ... INTO`, CTEs that wrap DML, and `EXPLAIN ANALYZE <write>` are also
  refused. Unknown statements are treated as unsafe. String literals, quoted identifiers and
  comments are skipped so `SELECT 'DELETE'` is still a read.
- **No credentials on the wire.** The agent receives only ids, names, hosts and database
  names. Passwords, SSH keys and certificates stay in the keychain.
- **Bounded output.** Results are capped (default 200 rows, 500 characters per cell, about
  256 KB per response) and flagged `truncated: true` so the agent knows to add `LIMIT`.
- **Statement timeout.** 30 seconds per statement.
- **Audit trail.** Every query the agent runs is written to your Tabular query history with
  the connection name suffixed `(agent)`, so you can review what it did.

There is deliberately no flag to enable writes from the agent. The intended flow is:
the agent drafts the statement, checks it with `check_sql_safety`, and asks you to run it in
Tabular where the Safety Guard, transaction mode and backup wizard apply.

## How it works

`tabular mcp` runs the same binary as the desktop app in headless mode
(`src/agent/cli.rs`). It opens Tabular's local `connections.db`, reuses the driver pool,
SSH tunnel and metadata cache code paths of the GUI (`src/agent/core.rs`), and serves MCP
over stdin/stdout with the official Rust SDK (`src/agent/mcp.rs`). Logs go to stderr and to
`<data dir>/logs/tabular.log`; stdout carries only protocol messages.

Both the GUI and the MCP server may run at the same time. They share the SQLite cache in WAL
mode; the MCP process opens its own connections to your databases.

Set `TABULAR_DATA_DIR` to point the server at a different data directory, for example a
CI machine with its own connections.

## Limitations

- MongoDB: schema browsing works, `run_query` does not (the GUI does not execute Mongo
  queries either).
- SQL Server: `explain_query` is not supported yet.
- The read-only classifier is conservative by design; a stored procedure that only reads is
  still refused because `CALL` may write.
- On iOS the server is not compiled (no stdio, not permitted by the App Store).
