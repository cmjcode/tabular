# Working on Tabular with an AI coding agent

This file is read by Claude Code, Codex, Cursor and similar tools. Keep it short and factual.

## What this is

Tabular is a native desktop SQL/NoSQL client in Rust (`eframe`/`egui`), single crate, about
320k lines. Drivers: PostgreSQL, MySQL, SQLite, SQL Server, Redis, MongoDB. It also ships an
MCP server for agents (`tabular mcp`, see `docs/MCP.md`).

## Commands

```bash
cargo check                                     # fast compile check
cargo clippy --all-targets -- -D warnings       # CI gate, must be clean
cargo clippy --all-targets --features collab -- -D warnings
cargo fmt --all --check                         # CI gate (whole tree is formatted)
cargo test                                      # unit + integration tests
cargo test --lib agent::                        # only the agent / MCP layer
cargo run -- mcp --print-config                 # MCP config snippet
```

CI runs on ubuntu, macos and windows (`.github/workflows/rust.yml`), plus `cargo audit`,
`cargo deny` and a `drivers` job that runs `src/connection/live_db_tests.rs` against real
PostgreSQL and MySQL services (set `TABULAR_TEST_PG_URL` / `TABULAR_TEST_MYSQL_URL` to run them
locally). Windows failures do not block yet.

## Layout

| Area | Path | Notes |
|---|---|---|
| Entrypoint | `src/lib.rs` (`run()`), `src/main.rs` | CLI dispatch happens before eframe starts |
| App state / UI | `src/window_egui/` (`Tabular` struct in `app_impl.rs`) | Most GUI functions take `&mut Tabular` |
| Editor | `src/editor*.rs`, `src/query_tools/` | Statement parser, formatter, lints |
| Connections & execution | `src/connection/` | `pool.rs` builds pools (SSH/TLS), `execute.rs` runs `QueryJob`s |
| Drivers | `src/driver_*.rs` | Per-database metadata fetching |
| Local cache | `connections.db` (SQLite) via `src/sidebar_database.rs`, `src/cache_data.rs` | Tables: `connections`, `table_cache`, `column_cache`, `foreign_key_cache`, `query_history` |
| Secrets | `src/secrets.rs` | OS keychain, encrypted-file fallback; failures queue a `SecretNotice` the UI shows |
| Runtime | `src/runtime.rs` | The one tokio runtime: `shared()`, `block_on()` (safe from any context), `spawn()` |
| Execution safety | `src/connection_safety.rs` | Per-connection Safety mode (Standard / Confirm all writes / Read-only); `assess()` uses the agent classifier |
| Menu bar | `src/window_egui/app_menu.rs` | Menu bar entries map to command-palette keys run by `editor::execute_command` |
| AI assistant (in-app) | `src/ai_assistant.rs`, `src/vector_index.rs`, `src/window_egui/ai_context_jobs.rs` | Local feature-hash embeddings via sqlite-vec. Each chat turn adds a context bundle (FKs/indexes, API endpoints, business processes, notes, code + git, past queries) built in a background thread by `agent::context`; see `docs/MCP.md` "Context sent with each question", ADR 0008 |
| Agent / MCP layer | `src/agent/` | Headless; must never depend on `window_egui`. `context.rs` = `table_context` tool and the chat context bundle |
| Sync / collab | `src/sync/` | E2E encrypted vault; `collab` feature is optional |
| Plugins | `src/plugin_runtime/` | Wasm plugins via `wasmi` |
| Import / export / transfer | `src/data_transfer/` (headless), `src/window_egui/transfer_*.rs` (UI) | File formats, encrypted exports, Data Files workspace (SQLite, `data_files.sqlite`), table transfer, data compare; see `docs/DATA_TRANSFER.md` |
| Multi-source comparison | `src/data_transfer/multi_compare*.rs`, `mongo_source.rs` (headless), `src/window_egui/multi_compare_{ui,jobs}.rs` | Sidebar DBs > Comp; N queries on different connections (incl. MongoDB JSON queries), `${var}` substitution, only differing keys shown; runs as a background job (`TaskOwner::Compare`); stored in `multi_compare_saved`; see `docs/DATA_TRANSFER.md`, ADR 0005 |
| Repository setup | `src/repo_setup/` (headless discovery), `src/window_egui/repo_setup_jobs.rs` (apply + background watcher) | Git folder → project, connections from `.env`/compose/framework config, API folder per service, diagram group per service; personal store `repo_setups.json`; see `docs/REPO_SETUP.md`, ADR 0004 |
| Integration testing | `src/http_tests.rs` (model, runner, cleanup, lint, concurrent bursts), `src/http_test_db.rs` (database steps: `{{x}}` → bound parameters, query targets, executor), `src/agent/bound_query.rs` (read-only parameterized SELECT per driver), `src/http_test_report.rs` (ISTQB/ISO 29119 stats, HTML/MD/JUnit/JSON), `src/http_test_cli.rs` (`tabular test`), `src/window_egui/integration_test_*.rs` (tab + jobs) | Project-scoped suites of ordered HTTP steps with phases (setup/test/cleanup) and groups; cleanup always runs; database steps never splice values into SQL and refuse non-SELECT on every access level; see `docs/HTTP_API_REPOSITORY.md`, ADR 0006 |
| Data checks | `src/data_checks.rs` (headless model, runner, reports, schedule), `src/data_check_cli.rs` (`tabular check`), `src/agent/test_tools.rs` (MCP test/check tools), `src/window_egui/data_health_ui.rs` (Project > Data Health) | Read-only invariant queries per project that must return no rows; stored in `data_checks/<project id>.json` |
| Live query diagnostics | `src/query_diagnostics/` (headless: syntax, schema, server-message locate, Mongo, Redis), `src/connection/validate.rs` (prepare without execution), `src/window_egui/diagnostics_{jobs,ui}.rs` | Squiggles, gutter marks, Problems list, quick fixes while typing; local checks use the autocomplete analyzer and cached schema, server checks prepare statements on idle pool connections (not on Production by default; MsSQL via `SET NOEXEC ON`); see `docs/QUERY_DIAGNOSTICS.md`, ADR 0009 |
| File Viewer | `src/file_viewer/` (headless: type detection, language map, image/SVG/PDF decode), `src/syntax_code.rs`, `src/window_egui/file_viewer_{ui,jobs}.rs` | Opens images, SVG, PDF (`pdf` feature, hayro), Markdown, CSV/XLSX (result grid), source code and config in a tab; Cmd+S saves keeping CRLF/BOM; entry points: File > Open File…, drag and drop, Git sidebar > Files; see `docs/FILE_VIEWER.md`, ADR 0007 |
| Git client | `src/git/` (headless), `src/window_egui/git_*.rs` (UI) | Runs the `git` CLI; Merge Review calls GitHub/GitLab REST; repo list merges diagram groups, HTTP API folders, projects and `git_repos.json` by normalized git URL. Sidebar state is per repository (`RepoUi`, keyed by repo key); Git Graph lane layout is pure in `git/graph.rs`, its tab data lives in `GitUiState::graphs` |

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
- Never report a failed write as success: no in-memory fallback for a save that failed. Use a
  toast (`toasts.error`, `error_once`) or, from headless/background code,
  `persist_notice::report`.
- One tokio runtime per process: `crate::runtime`. `clippy.toml` forbids `Runtime::new` and
  runtime builders outside the CLI entry points and tests.
- No network or database call from a render path: start a background job and render a
  placeholder (pattern: `window_egui/folder_fetch_jobs.rs`, `query_jobs.rs`).
- SQL statement splitting goes through `connection::sql::split_sql_segments`/`split_sql_spans`;
  identifier quoting through `schema_objects::sql::{quote_ident, quote_qualified}`.
- Destructive confirmations use `style::confirm_dialog`; overwrites of a whole database use
  `style::type_to_confirm`. New SQL execution paths must respect `connection_safety::assess`.
- UI strings stay English (`tests/ui_english_strings.rs`); use `egui_icons::icons::ICON_*`, not
  emoji, for icons (`tests/ui_glyph_coverage.rs`). New user-facing features get a menu bar entry
  (and so a palette command).

## Things that bite

- `libsqlite3-sys` is pinned to 0.37 with `bundled`; `sqlite-vec` links against it. Do not
  add a second SQLite.
- `mssql-client` is pre-1.0 and pinned to 0.20.x. Its TLS is rustls (TLS 1.2+ only), so legacy
  SQL Server needs `MssqlEncryption::Disabled` (`no_tls`). MsSQL options live in `plugin_options`
  (`DatabaseType::has_stored_options`); every `connect_mssql`/`mssql_config` call must pass
  `MssqlOptions::from_connection`. Do not emit `OFFSET/FETCH` for MsSQL without checking
  `driver_mssql::is_legacy_server` (or use `TOP`/`ROW_NUMBER()`; the `__tabular_rn` column is
  stripped by the driver).
- iOS builds the crate as a `staticlib`; gate anything that needs stdio, processes or
  file dialogs with `#[cfg(not(target_os = "ios"))]`.
- The GUI and `tabular mcp` may run concurrently against the same `connections.db` (WAL).
