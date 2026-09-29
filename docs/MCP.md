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
| `describe_schema` | Tables, columns, primary keys, foreign keys, cached indexes and partitions as compact DDL plus JSON. Also adds what you drew in the diagram: virtual relations, the groups a table belongs to, and how many notes mention it. Pass `question` and the most relevant tables are returned first, ranked by Tabular's local vector index (nothing leaves your machine). |
| `schema_diagram` | The same schema as a Mermaid `erDiagram`. Virtual relations are drawn as dotted lines. |
| `describe_diagram` | Your Tabular diagram for a database: groups with their tables and linked repositories, virtual relations, sticky notes and linked databases. Pass `table` or `group` to narrow it. |
| `refresh_schema_cache` | Re-fetch schema metadata from the server into Tabular's cache. |
| `run_query` | Execute a **read-only** statement (or read-only Redis commands) and return rows. |
| `explain_query` | Execution plan for PostgreSQL, MySQL and SQLite, parsed into a tree with cost percentages and bottleneck warnings from Tabular's query profiler. |
| `analyze_query` | Explain one statement without running it: tables, joins, filter, output columns and their sources, optimization hints, and join or filter columns that no cached index starts with. |
| `search_query_history` | Queries you already ran that are similar to a question, found with the local vector index. Passwords are masked. Queries run by agents are left out unless asked for. |
| `find_table_usages` | Search the code repository linked to a diagram group for the files and lines that use a table. |
| `check_sql_safety` | Classify each statement (read / write / ddl / admin), flag `UPDATE`/`DELETE` without `WHERE`, and lint, without executing. |
| `format_sql` | Format SQL with Tabular's formatter. |
| `search_notes` | Search your Obsidian vault for notes about tables, business rules and conventions. |
| `read_note` | Read one whole vault note, with its tags and `[[wikilinks]]`. |
| `save_note` | Create a new note in the vault's `Tabular Memory` folder. Only works when you allow it in Settings. |

### Where the knowledge comes from

The agent reads what Tabular already stores. Nothing is copied into Markdown files first.

| Source | Stored in | Tools |
|---|---|---|
| Schema, indexes, partitions | `connections.db` cache | `describe_schema`, `schema_diagram`, `analyze_query` |
| Groups, virtual relations, sticky notes | The diagram file in `<data dir>/diagrams/`, or the shared `diagram_by_tabular` table when there is no local file | `describe_diagram`, and as a summary in `describe_schema` |
| Repository per group | Git URL in the diagram, local folder in `diagram_repo_paths.json` | `describe_diagram`, `find_table_usages` |
| Query history | `connections.db` | `search_query_history` |
| Your own notes | Obsidian vault | `search_notes`, `read_note`, `save_note` |

`find_table_usages` reads only the local folder you picked for a group, or a clone Tabular
already made when you ran "Suggest tables". It never runs git and never reads other folders.
Git URLs are shown with any embedded credentials masked.

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
- **Diagram knowledge is read-only.** The agent cannot add or edit sticky notes, groups or
  relations. Durable facts go to the vault through `save_note`.
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

## Using a CLI agent inside Tabular

The AI Assistant panel (Cmd+Shift+A) can run installed coding agents instead of
or alongside an HTTP API. In **Settings → AI Assistant → CLI Agents**, configure and
enable any supported agent. You can enable multiple agents simultaneously and switch
between them directly in the AI Assistant chat panel:

| Agent | Binary | How the Tabular MCP server reaches it | Conversation continuity |
|---|---|---|---|
| Antigravity | `agy` | Global config — press **Register** (runs `agy mcp add tabular -- <tabular> mcp`) | `--conversation <id>` |
| Claude Code | `claude` | Per request via `--mcp-config` + `--strict-mcp-config`, only `mcp__tabular` tools allowed | `--resume <session>` |
| Gemini CLI | `gemini` | Global config — press **Register** (`gemini mcp add tabular <tabular> mcp`) | none (each turn is a new session) |
| Custom | any | Register manually with `tabular mcp --print-config` | `{session}` placeholder |

The agent runs in print mode with `--output-format stream-json`, permission
prompts disabled, and an empty working directory under Tabular's data folder
(`agent-workspace/`). Database access still goes through the read‑only tools
described above, so the agent can inspect data and schemas but cannot write.

### Live edit protocol

The system prompt asks the agent to put SQL meant for the editor in a fenced
block whose info string names the target tab:

~~~text
```sql tabular:tab=12 mode=replace
SELECT id, email FROM users ORDER BY created_at DESC LIMIT 10
```
~~~

`tab_id` values come from the "Open editor tabs" section of the prompt (the
active tab plus any attached tabs). `mode` is `replace` (default), `append`, or
`selection`. Tabular applies the block to that tab while the answer streams;
each edit gets a **Revert** button, and with live edit turned off an **Apply**
button instead. Plain ```` ```sql ```` blocks are only shown in the chat.
