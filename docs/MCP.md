# Tabular as an MCP server (for AI agent harnesses)

Tabular ships a built-in [Model Context Protocol](https://modelcontextprotocol.io) server so
coding agents such as Claude Code, Cursor, Codex CLI, Windsurf and any other MCP client can
inspect and query the databases you have already configured in the app.

The design goal: **the agent never sees a credential, and it writes only where you allowed
it.** Tabular keeps passwords, SSH keys and TLS material in the OS keychain, opens the SSH
tunnel or mTLS session itself, and hands the agent only connection ids. Every connection is
read-only for agents until you raise its access level (see
[Agent access per connection](#agent-access-per-connection)); even then, risky statements
wait for your approval in the Tabular window.

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
| `list_connections` | Connections this client may use: id, name, kind, host, default database, `supports_query`, agent `access` level and environment. Blocked connections are left out. No secrets. |
| `get_agent_permissions` | What the agent may do on each connection and which statements always need approval. |
| `list_databases` | Databases / schemas known for a connection (fetches from the server on first use). |
| `list_tables` | Table and view names, optionally filtered by a substring. |
| `describe_table` | One table: columns, primary key, foreign keys in both directions, cached indexes. |
| `get_table_ddl` | `CREATE` statement of a table: from the server for MySQL and SQLite, reconstructed from the cache (`source: cache`) for other engines. |
| `sample_rows` | First rows of a table (`SELECT *` with a limit). |
| `count_rows` | Exact `COUNT(*)` of a table. |
| `list_running_queries` | Statements running on the server (PostgreSQL, MySQL, SQL Server) with duration, wait event and blocking info. |
| `cancel_query` | Cancel a statement or end a session by pid. Admin command: always needs your approval. |
| `execute_statement` | Run writes, DDL or admin statements on a connection whose access is Ask, Edit or Agent, after approval where required. |
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
  read-only Redis commands pass `run_query`. `INSERT`/`UPDATE`/`DELETE`/`MERGE`/`TRUNCATE`/`CALL`,
  all DDL, and session or server commands (`SET`, `USE`, `BEGIN`, `COMMIT`, `KILL`, `PRAGMA`,
  ...) are refused there. `SELECT ... INTO`, CTEs that wrap DML, and `EXPLAIN ANALYZE <write>`
  are also refused, and so is a `SELECT` that calls a function with side effects
  (`pg_terminate_backend`, `nextval`, `setval`, `set_config`, `dblink`, `lo_*`, `get_lock`,
  `load_extension`, `xp_cmdshell`, `OPENROWSET`, ...): those are classified as admin commands.
  Unknown statements are treated as unsafe. String literals, quoted identifiers and comments
  are skipped so `SELECT 'DELETE'` is still a read. The classifier is a keyword scanner, not a
  full parser; a user-defined function that writes is still seen as a read.
- **Writes only where you allowed them.** `execute_statement` and `cancel_query` follow the
  connection's access level; see below.
- **No credentials on the wire.** The agent receives only ids, names, hosts and database
  names. Passwords, SSH keys and certificates stay in the keychain.
- **Bounded output.** Results are capped (default 200 rows, 500 characters per cell, about
  256 KB per response) and flagged `truncated: true` so the agent knows to add `LIMIT`.
- **Statement timeout.** 30 seconds per statement.
- **Diagram knowledge is read-only.** The agent cannot add or edit sticky notes, groups or
  relations. Durable facts go to the vault through `save_note`.
- **Audit trail.** Every query the agent runs is written to your Tabular query history with
  the connection name suffixed `(agent)`, so you can review what it did. Every tool call,
  resource read and prompt is also written to the agent activity log (see below).

## Agent access per connection

Open **Settings menu → Agent Access (MCP)**. For each connection pick a level:

| Level | Reads | INSERT / UPDATE / DELETE | DDL | Admin (KILL, SET, side-effect functions) |
|---|---|---|---|---|
| Blocked | refused; the connection is hidden | refused | refused | refused |
| Read only (default) | yes | refused | refused | refused |
| Ask | yes | after approval | after approval | after approval |
| Edit | yes | runs directly | after approval | after approval |
| Agent | yes | runs directly | runs directly | after approval |

Whatever the level, these always wait for approval: `UPDATE`/`DELETE` without `WHERE`,
`DROP DATABASE/SCHEMA/TABLE/USER/ROLE`, `TRUNCATE`, `FLUSHALL`/`FLUSHDB`, and any write on a
connection marked (or named like) **Production**. A batch is approved as a whole; one refused
statement refuses the batch.

**Approvals.** The MCP process queues the request in `connections.db` and waits. A running
Tabular window notices it within about a second, shows an "Agent wants to run SQL" dialog with
the client, connection, reason and full SQL (and an OS notification when the window is in the
background), and you choose **Approve & Run** or **Deny**. If no Tabular window picks the
request up within 8 seconds, the server asks through the MCP client instead, but only when the
client supports elicitation; otherwise the statement is refused with a message telling the
agent to ask you to open Tabular. Undecided requests expire after 3 minutes. Approved
statements run on the agent process's own connection, not inside your editor's transaction.

**Clients.** Each client is recorded under the name it reports when it connects
(`clientInfo.name`). Per connection you can allow all clients or only some. The **Clients**
tab lists every client seen, with **Forget**, which removes it from the list and from every
allowlist. The name is not verified: a local program can claim any name, so the allowlist
keeps well-behaved clients apart but is not a security boundary. The stdio transport has no
token; anything that can start `tabular mcp` as your user can use it.

**Activity log.** The **Activity Log** tab shows the last 500 entries: time, client, tool /
resource / prompt, connection, statement kind, outcome (`ok`, `refused`, `denied`,
`approved`, `error`) and duration. Statements are stored as a SHA-256 digest, not as text; the
text of what actually ran is in Query History. Entries are kept 90 days (at most 20,000).

These settings are stored in `connections.db` (`agent_connection_access`, `agent_clients`,
`agent_approvals`, `agent_activity`) on this machine only; they are not synced.

## Resources and prompts

Resources (read-only JSON; the same access rules as tools):

| URI | Content |
|---|---|
| `tabular://connections` | Connections this client may use |
| `tabular://connections/{id}/databases` | Databases / schemas |
| `tabular://connections/{id}/schema{?database}` | Tables, columns, keys, indexes (subscribable) |
| `tabular://connections/{id}/tables{?database}` | Table and view names |
| `tabular://connections/{id}/tables/{table}{?database}` | One table's structure |
| `tabular://connections/{id}/tables/{table}/ddl{?database}` | `CREATE` statement |
| `tabular://connections/{id}/history{?limit}` | Recent queries, passwords masked |
| `tabular://connections/{id}/diagram{?database}` | Your diagram: groups, virtual relations, notes |

Table names in URIs are percent-encoded. Subscriptions: a client can subscribe to a `schema`
resource (legacy `resources/subscribe` or the newer `subscriptions/listen`). The server checks
Tabular's schema cache every 20 seconds and sends `notifications/resources/updated` when the
cached tables, columns or types change, and `resources/list_changed` when connections or
access settings change. It detects changes to the cache, so a schema change on the server is
noticed after Tabular (or `refresh_schema_cache`) refreshes the cache.

Prompts, rendered from the live schema: `explain_schema`, `explain_table`,
`data_quality_audit`, `question_to_sql`, `review_query`, `propose_indexes`,
`write_migration`, `summarize_query_history`. Each takes `connection_id` plus the arguments
shown in `prompts/list`.

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
  still refused by `run_query` because `CALL` may write (use `execute_statement` on a
  connection with a write-capable level).
- Transport is stdio only. There is no HTTP/SSE endpoint, so there are no tokens, pairing or
  per-token scopes; access is controlled per connection and per client name as above.
- `get_table_ddl` for PostgreSQL and SQL Server is reconstructed from the cache and omits
  defaults, check constraints and storage options.
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
(`agent-workspace/`). Database access still goes through the Tabular MCP tools
described above, so the connection's agent access level applies: read-only by default, and
writes on Ask/Edit/Agent connections show the approval dialog in this window.

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

## Using outside MCP servers from the AI Assistant

When the AI Assistant uses an **HTTP API provider** (OpenAI-compatible, Anthropic or
Gemini), it can call tools from other MCP servers you configure, for example a GitHub,
filesystem or ticketing server. CLI agents (Claude Code, agy, Gemini CLI) keep using their
own MCP configuration and are not affected.

### Setup

1. Open **Settings → AI Assistant → MCP Servers** and press **+ Add MCP server**.
2. Enter a name (letters, digits, `-`, `_`; it becomes the tool prefix), the command and its
   arguments, and any environment variables. Example:
   name `github`, command `npx`, arguments `-y @modelcontextprotocol/server-github`,
   env `GITHUB_PERSONAL_ACCESS_TOKEN=…`.
3. Press **Test / List tools**. Tabular starts the server, reads `tools/list`, and stops it.
4. Tick **Allow** for each tool the model may use. New tools start disallowed. **Ask first**
   (on by default) shows an Approve / Deny card in the chat before the tool runs.

Tabular's own server can be added the same way (command: the path to the `tabular` binary,
arguments: `mcp`), which gives an HTTP provider the read-only database tools described above.

### How a turn works

- When at least one allowed tool exists, the chat request carries tool definitions in the
  provider's native format (`tools`/`tool_calls` for OpenAI-compatible, `tools`/`tool_use`/
  `tool_result` for Anthropic, `functionDeclarations`/`functionCall`/`functionResponse` for
  Gemini). Tool names are `<server>__<tool>`.
- Each call is shown in the transcript with its arguments (collapsed) and a short result
  summary. A denied call is reported to the model as declined.
- Servers are started on the first call of a turn and stopped when the turn ends. Each call
  has a 60 second timeout, results are cut to 20 KB, and a turn stops after 8 tool rounds.
- With no allowed tools, the chat works exactly as before.

### Storage and limits

- The server list lives in the local `connections.db` table `ai_mcp_servers`. It is not
  synced or exported. Environment values whose name looks secret (`TOKEN`, `KEY`, `SECRET`,
  `PASSWORD`, ...) are stored in Tabular's encrypted secret store; the table holds only a
  marker.
- Only stdio servers are supported. Streamable-HTTP / SSE servers are not.
- Tool definitions come from the last **Test / List tools**. Run it again after upgrading a
  server.
- Not available on iOS (no child processes).
