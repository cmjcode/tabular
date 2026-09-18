# Working on Tabular with an AI coding agent

This file is read by Claude Code, Codex, Cursor and similar tools. Keep it short and factual.

## What this is

Tabular is a native desktop SQL/NoSQL client in Rust (`eframe`/`egui`), single crate, about
115k lines. Drivers: PostgreSQL, MySQL, SQLite, SQL Server, Redis, MongoDB. It also ships an
MCP server for agents (`tabular mcp`, see `docs/MCP.md`).

## Commands

```bash
cargo check                                     # fast compile check
cargo clippy --all-targets -- -D warnings       # CI gate, must be clean
cargo clippy --all-targets --features collab -- -D warnings
cargo test                                      # unit + integration tests
cargo test --lib agent::                        # only the agent / MCP layer
cargo run -- mcp --print-config                 # MCP config snippet
```

CI runs on ubuntu, macos and windows (`.github/workflows/rust.yml`). `cargo fmt` is not yet
enforced on the whole tree; format only the files you touch.

## Layout

| Area | Path | Notes |
|---|---|---|
| Entrypoint | `src/lib.rs` (`run()`), `src/main.rs` | CLI dispatch happens before eframe starts |
| App state / UI | `src/window_egui/` (`Tabular` struct in `app_impl.rs`) | Most GUI functions take `&mut Tabular` |
| Editor | `src/editor*.rs`, `src/query_tools/` | Statement parser, formatter, lints |
| Connections & execution | `src/connection/` | `pool.rs` builds pools (SSH/TLS), `execute.rs` runs `QueryJob`s |
| Drivers | `src/driver_*.rs` | Per-database metadata fetching |
| Local cache | `connections.db` (SQLite) via `src/sidebar_database.rs`, `src/cache_data.rs` | Tables: `connections`, `table_cache`, `column_cache`, `foreign_key_cache`, `query_history` |
| Secrets | `src/secrets.rs` | OS keychain, encrypted-file fallback |
| AI assistant (in-app) | `src/ai_assistant.rs`, `src/vector_index.rs` | Local feature-hash embeddings via sqlite-vec |
| Agent / MCP layer | `src/agent/` | Headless; must never depend on `window_egui` |
| Sync / collab | `src/sync/` | E2E encrypted vault; `collab` feature is optional |
| Plugins | `src/plugin_runtime/` | Wasm plugins via `wasmi` |

## Conventions

- Code comments and doc comments are written in **Bahasa Indonesia**; UI strings and
  user-facing docs are in **English**.
- Errors: `thiserror` enums per domain (`QueryExecutionError`, `AgentError`). Do not
  `unwrap()` on I/O or network paths.
- Logging: `log::{debug,info,warn,error}!` with a bracket tag, e.g. `log::warn!("[AGENT] ...")`.
- Headless code (anything an agent or a test calls) takes plain data (`&ConnectionConfig`,
  `&SqlitePool`), never `&mut Tabular`. See `src/export_import_all.rs::export_all_data_payload`
  and `src/agent/core.rs` for the pattern.
- Tests: unit tests inline under `#[cfg(test)]`; integration tests in `tests/`. SQLite
  in-memory pools are the preferred fixture (see `src/connection/execute.rs` tests).
- Lints allowed globally: `collapsible_if`, `too_many_arguments`, `type_complexity`. Everything
  else must pass `clippy -D warnings`.
- Never serialize `ConnectionConfig` towards an agent or a network peer; it carries secrets.

## Things that bite

- `libsqlite3-sys` is pinned to 0.37 with `bundled`; `sqlite-vec` links against it. Do not
  add a second SQLite.
- `mssql-client` is pre-1.0 and pinned to 0.20.x.
- iOS builds the crate as a `staticlib`; gate anything that needs stdio, processes or
  file dialogs with `#[cfg(not(target_os = "ios"))]`.
- The GUI and `tabular mcp` may run concurrently against the same `connections.db` (WAL).
