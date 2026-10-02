# WebAssembly (Wasm) Plugin Development Guide

Tabular features a sandboxed, pure-Rust WebAssembly plugin runtime powered by the `wasmi` interpreter engine (located in `src/plugin_runtime/`).

Plugins can read selected table data, inspect schemas, generate custom export formats, or produce ORM source models without needing to recompile Tabular.

---

## 🏗️ Architecture & Host API

Wasm guest modules communicate with the Tabular Host via standard C-ABI functions:

```
+-----------------------------------------------------------------------------------------+
|                                    TABULAR HOST                                         |
+-----------------------------------------------------------------------------------------+
|  Host APIs:                                                                             |
|  • tabular_get_selected_rows_len() -> i32                                               |
|  • tabular_get_selected_rows_data(ptr: i32, max_len: i32) -> i32                        |
|  • tabular_get_table_schema_len() -> i32                                                |
|  • tabular_get_table_schema_data(ptr: i32, max_len: i32) -> i32                         |
|  • tabular_set_result(ptr: i32, len: i32)                                               |
|  • tabular_log(level: i32, ptr: i32, len: i32)                                          |
+-----------------------------------------------------------------------------------------+
                                      ▲              │
                               Memory │ Buffer       │ Function Calls
                                      │              ▼
+-----------------------------------------------------------------------------------------+
|                               WASM SANDBOX GUEST MODULE                                 |
|                         (Compiled from Rust, C, WAT, or Zig)                            |
+-----------------------------------------------------------------------------------------+
```

---

## 🚀 Building a Plugin in Rust

### 1. Configure `Cargo.toml`
```toml
[package]
name = "my_custom_exporter"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 2. Implement the Entrypoint
```rust
extern "C" {
    fn tabular_get_selected_rows_len() -> i32;
    fn tabular_get_selected_rows_data(ptr: *mut u8, max_len: i32) -> i32;
    fn tabular_set_result(ptr: *const u8, len: i32);
    fn tabular_log(level: i32, ptr: *const u8, len: i32);
}

#[no_mangle]
pub extern "C" fn tabular_plugin_run() -> i32 {
    let len = unsafe { tabular_get_selected_rows_len() };
    if len <= 0 {
        return 0;
    }

    let mut buf = vec![0u8; len as usize];
    unsafe {
        tabular_get_selected_rows_data(buf.as_mut_ptr(), len);
    }

    // Process JSON data
    let json_str = String::from_utf8_lossy(&buf);
    let output = format!("// Generated from Tabular\n// Records count: {}\n{}", len, json_str);

    unsafe {
        tabular_set_result(output.as_ptr(), output.len() as i32);
    }

    1
}
```

### 3. Compile to WebAssembly
```bash
cargo build --target wasm32-unknown-unknown --release
```

The resulting `.wasm` binary in `target/wasm32-unknown-unknown/release/` can be loaded directly into Tabular via the **Plugin Manager Modal**.

---

## 📦 Built-In Starter Plugins & Templates

Tabular includes pre-configured plugins ready to run from `src/plugin_runtime/templates/`:
1. **Apache Parquet / DuckDB Exporter**: Converts result sets to column-oriented formats.
2. **Rust ORM Generator**: Emits `Diesel` and `SeaORM` entity definitions with typed fields.
3. **TypeScript ORM Generator**: Emits `Prisma` schema models and `TypeORM` entities.
4. **Python ORM Generator**: Emits `SQLAlchemy 2.0` Declarative Base models.

---

## 🔌 Database Driver Plugins (`tabular-driver-v1`)

PostgreSQL, MySQL, SQLite, SQL Server, MongoDB and Redis are built in. Any other engine is
added as a **driver plugin** (see `docs/adr/0002-engine-driver-plugins.md`). A driver plugin
gets the generic features: connection form, object tree, query editor, result grid, charts,
export and read-only MCP access. Engine-specific tools (DBA monitor, backup, structure
editor) stay with the built-in engines.

There are two kinds of driver plugins:

| Kind | Runs as | Use it for | Network |
|---|---|---|---|
| `wasm` | Sandboxed `wasmi` module inside Tabular | Engines with an HTTP API (ClickHouse, Elasticsearch, Trino, BigQuery…) | Only through the host, limited to `permissions.http_hosts` |
| `sidecar` | Separate native process, JSON-RPC over stdio | Binary protocols or native client libraries (Oracle, Cassandra, Kafka, DuckDB…) | Unrestricted; the user must approve the executable's SHA-256 first |

Sidecar drivers are desktop-only. On iOS only bundled Wasm drivers can run.

### Package layout

A plugin is a folder with a `manifest.json` and its entry file. Install it from
**Plugins → Database Drivers → Install from Folder…**, which copies it to
`<data dir>/plugins/drivers/<engine-id>/`.

```json
{
  "abi": "tabular-driver-v1",
  "kind": "wasm",
  "version": "0.1.0",
  "author": "You",
  "description": "What the driver connects to.",
  "entry": "driver.wasm",
  "permissions": { "http_hosts": ["{connection.host}", "*.example.cloud"] },
  "engine": {
    "id": "myengine",
    "name": "My Engine",
    "icon": "🧩",
    "default_port": 8080,
    "standard_fields": { "host": true, "port": true, "username": true, "password": true, "database": true },
    "options": [
      { "key": "region", "label": "Region", "kind": "select", "choices": ["eu", "us"], "default": "eu" },
      { "key": "api_key", "label": "API key", "kind": "secret", "required": true }
    ],
    "capabilities": {
      "databases": true,
      "schemas": false,
      "query_language": "sql",
      "sql_dialect": "ansi",
      "cancel": false,
      "ssh_tunnel": true,
      "tls": true,
      "preview_template": null
    }
  }
}
```

- `engine.id` must match `[a-z][a-z0-9_-]{0,47}` and may not reuse a built-in engine name.
  Connections store it as `plugin:<id>`.
- `entry` can also be a map keyed by `"<os>-<arch>"` (for example `"macos-aarch64"`,
  `"windows-x86_64"`) for per-platform sidecar binaries. Entry paths must stay inside the
  plugin folder.
- Option kinds: `text`, `secret` (stored in the OS keychain, not in `connections.db`),
  `number`, `bool` (`"true"`/`"false"`) and `select`.
- `{connection.host}` in `http_hosts` expands to the connection's host, or to `127.0.0.1`
  when Tabular opens an SSH tunnel for the connection.
- `preview_template` sets the query opened when a table is clicked. Placeholders:
  `{table}` (quoted), `{raw_table}`, `{database}`, `{limit}`.

### Methods

Both kinds implement the same methods. Payloads and results are JSON.

| Method | Params | Result |
|---|---|---|
| `connect` | `ConnectParams` (host, port, username, password, database, tls, options) | `null` (Wasm) / `{ "session": "<id>" }` (sidecar) |
| `list_databases` | — | `["db1", …]` |
| `list_schemas` | `{ database }` | `["public", …]` |
| `list_tables` | `{ database, schema }` | `[{ "name": "t", "kind": "table" \| "view" \| "other" }]` |
| `list_columns` | `{ database, schema, table }` | `[{ "name", "data_type", "nullable", "primary_key" }]` |
| `execute` | `{ query, database, schema, max_rows, job_id }` | `ExecuteOutput` `{ headers, rows, affected_rows, truncated }`; rows use `null` for SQL NULL |
| `cancel` | `{ job_id }` | `null` (only called when `capabilities.cancel` is true) |
| `close` | — | `null` |

Stop reading after `max_rows` and set `truncated`. Sidecar methods other than `connect`
also receive `"session"`.

### Wasm drivers

Use the guest SDK in `plugins/sdk` and export your type with `export_driver!`:

```rust
use tabular_driver_sdk::*;

#[derive(Default)]
struct MyEngine { base: String }

impl Driver for MyEngine {
    fn connect(&mut self, p: ConnectParams) -> DriverResult<()> {
        self.base = format!("https://{}:{}", p.host, p.port.unwrap_or(443));
        http(&HttpRequest { method: "GET".into(), url: format!("{}/ping", self.base),
            headers: vec![], body: None, timeout_ms: Some(10_000) }).map(|_| ())
    }
    fn list_tables(&mut self, _db: Option<String>, _s: Option<String>) -> DriverResult<Vec<TableInfo>> { Ok(vec![]) }
    fn list_columns(&mut self, _db: Option<String>, _s: Option<String>, _t: String) -> DriverResult<Vec<ColumnInfo>> { Ok(vec![]) }
    fn execute(&mut self, r: ExecuteRequest) -> DriverResult<ExecuteReply> {
        // Let Tabular run the request and parse the table natively (fast for large results).
        Ok(ExecuteReply::HttpTable(HttpTableRequest {
            request: HttpRequest { method: "POST".into(), url: format!("{}/query", self.base),
                headers: vec![], body: Some(r.query), timeout_ms: None },
            format: TableFormat::Ndjson,
        }))
    }
}

export_driver!(MyEngine);
```

Build with `cargo build --release --target wasm32-unknown-unknown` (`crate-type = ["cdylib"]`)
and put the `.wasm` next to `manifest.json`.

Rules enforced by the host:
- No sockets or files. `http()` is the only I/O, checked against `http_hosts`; redirects are
  not followed.
- Each call gets a CPU budget and 256 MiB of memory. Waiting on HTTP does not count.
- Tabular runs up to four instances of the module per connection so a long query does not
  block the object tree or `cancel`. Each instance receives its own `connect`, so keep
  per-connection state in your driver struct.
- Returning `http_table` (formats `json_compact`, `tsv_with_names`, `csv_with_names`,
  `ndjson`) keeps result rows out of the interpreter. Prefer it for query results.

The ABI underneath the SDK: exports `memory`, `tdrv_abi_version() -> i32` (returns 1),
`tdrv_alloc(len) -> ptr`, `tdrv_free(ptr, len)` and
`tdrv_call(method_ptr, method_len, payload_ptr, payload_len) -> i64` returning
`(ptr << 32) | len` of `{"ok": …}` or `{"err": "…"}`. Imports from module `tabular`:
`log(level, ptr, len)`, `http(ptr, len) -> len`, `take(ptr, len) -> len`, `now_ms() -> i64`.

### Sidecar drivers

A sidecar is any executable that speaks JSON-RPC 2.0 with one JSON object per line on
stdin/stdout. Tabular sends `initialize {"abi": "tabular-driver-v1"}` first and expects
`{"abi_version": 1}`. Requests can arrive concurrently (for example `cancel` while `execute`
runs), so answer them independently and match replies by `id`. Write logs to stderr; stdout
lines that are not JSON are ignored. If the process exits, Tabular restarts it on the next
request and calls `connect` again.

Sidecars are not sandboxed. Tabular loads a sidecar only after the user reviews its path and
SHA-256 in the Plugin Manager; replacing the binary requires approval again.

### Reference plugins

- `plugins/examples/clickhouse` — Wasm driver for ClickHouse over HTTP. Build with
  `plugins/examples/clickhouse/build.sh`; the folder `dist/` is ready to install.
- `plugins/examples/duckdb-sidecar` — DuckDB as a sidecar process (DuckDB bundles its own
  engine, so it runs outside Tabular). Build with `plugins/examples/duckdb-sidecar/build.sh`.
