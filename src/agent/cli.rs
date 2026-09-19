//! Subcommand baris perintah. Tanpa argumen yang dikenali, Tabular tetap
//! menjalankan GUI seperti biasa, sehingga bundle macOS/Linux/Windows tidak
//! berubah perilaku (argumen `-psn_*` dari Finder pun jatuh ke GUI).
//!
//! Dipakai:
//! - `tabular mcp`                 → server MCP lewat stdio.
//! - `tabular mcp --print-config`  → cetak snippet konfigurasi untuk harness.
//! - `tabular --help` / `--version`.

use std::sync::Arc;

use super::core::{HeadlessSession, open_cache_pool};

const USAGE: &str = "\
Tabular — SQL & NoSQL client

USAGE:
  tabular                      launch the desktop app
  tabular mcp                  run the MCP server on stdio (for AI agent harnesses)
  tabular mcp --print-config   print a JSON snippet for Claude Code / Cursor / Codex
  tabular --version
  tabular --help

Environment:
  TABULAR_DATA_DIR             override the data directory (connections.db, logs)
  RUST_LOG                     log level for the MCP server (logs go to stderr + file)
";

/// Jalankan mode CLI bila argumen pertama dikenali. `None` berarti lanjut ke GUI.
pub fn try_run_from_args() -> Option<Result<(), String>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => Some(run_mcp(&args[1..])),
        Some("--help") | Some("-h") | Some("help") => {
            print!("{USAGE}");
            Some(Ok(()))
        }
        Some("--version") | Some("-V") => {
            println!("tabular {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        _ => None,
    }
}

fn run_mcp(rest: &[String]) -> Result<(), String> {
    if rest.iter().any(|a| a == "--print-config") {
        print_config();
        return Ok(());
    }
    if let Some(unknown) = rest.iter().find(|a| a.starts_with('-')) {
        return Err(format!(
            "unknown option for `tabular mcp`: {unknown}\n\n{USAGE}"
        ));
    }

    // Urutan sama dengan `run()` GUI: sqlite-vec harus terdaftar sebelum pool
    // SQLite pertama dibuka, dan data dir harus final sebelum logging ke file.
    crate::vector_index::register_sqlite_vec();
    dotenvy::dotenv().ok();
    // `init_data_dir()` menimpa TABULAR_DATA_DIR dengan lokasi yang tersimpan
    // dari GUI. Untuk mode headless, env var yang diberikan harness harus
    // menang supaya server bisa diarahkan ke data dir lain (CI, sandbox, tes).
    match std::env::var("TABULAR_DATA_DIR") {
        Ok(dir) if !dir.trim().is_empty() => {
            log::debug!("[AGENT] using TABULAR_DATA_DIR from environment: {dir}");
        }
        _ => crate::config::init_data_dir(),
    }
    crate::app_logging::init();
    crate::app_logging::install_panic_hook();
    log::info!(
        "[AGENT] starting MCP server (data dir: {})",
        crate::config::get_data_dir().display()
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to start async runtime: {e}"))?;

    runtime.block_on(async {
        let cache_pool = open_cache_pool().await.map_err(|e| e.to_string())?;
        let session = Arc::new(HeadlessSession::new(cache_pool));
        super::mcp::serve_stdio(session).await
    })
}

/// Cetak konfigurasi siap tempel. Path binary diambil dari proses ini sendiri
/// supaya cocok untuk instalasi bundle (.app) maupun binary lepas.
fn print_config() {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "tabular".to_string());
    let config = serde_json::json!({
        "mcpServers": {
            "tabular": {
                "command": exe,
                "args": ["mcp"]
            }
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&config).unwrap_or_default()
    );
    eprintln!();
    eprintln!("Claude Code:  claude mcp add tabular -- \"{exe}\" mcp");
    eprintln!("Antigravity:  agy mcp add tabular -- \"{exe}\" mcp");
    eprintln!("Gemini CLI:   gemini mcp add tabular \"{exe}\" mcp");
    eprintln!("Cursor:       paste the JSON above into ~/.cursor/mcp.json");
    eprintln!(
        "Codex CLI:    add [mcp_servers.tabular] command = \"{exe}\" args = [\"mcp\"] to ~/.codex/config.toml"
    );
}
