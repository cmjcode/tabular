//! Subcommand baris perintah. Tanpa argumen yang dikenali, Tabular tetap
//! menjalankan GUI seperti biasa, sehingga bundle macOS/Linux/Windows tidak
//! berubah perilaku (argumen `-psn_*` dari Finder pun jatuh ke GUI).
//!
//! Dipakai:
//! - `tabular mcp`                 → server MCP lewat stdio.
//! - `tabular mcp --print-config`  → cetak snippet konfigurasi untuk harness.
//! - `tabular open <url>`          → buka deep link / DSN di instance GUI (M2).
//! - `tabular connections [--json]`→ daftar koneksi tersimpan tanpa rahasia.
//! - `tabular tabular://...`       → sama dengan `open` (dipakai handler skema
//!   URL di Linux/Windows).
//! - `tabular --help` / `--version`.

use std::sync::Arc;

use super::core::{HeadlessSession, open_cache_pool};

const USAGE: &str = "\
Tabular — SQL & NoSQL client

USAGE:
  tabular                      launch the desktop app
  tabular mcp                  run the MCP server on stdio (for AI agent harnesses)
  tabular mcp --print-config   print a JSON snippet for Claude Code / Cursor / Codex
  tabular open <url>           open a tabular:// link or a database URL in the running app
                               (e.g. tabular open postgres://user@localhost:5432/app)
  tabular connections [--json] list saved connections (never includes passwords)
  tabular --version
  tabular --help

Environment:
  TABULAR_DATA_DIR             override the data directory (connections.db, logs)
  TABULAR_POLICY_FILE          managed policy JSON (updates, network toggles, language)
  RUST_LOG                     log level for the MCP server (logs go to stderr + file)
";

/// Jalankan mode CLI bila argumen pertama dikenali. `None` berarti lanjut ke GUI.
pub fn try_run_from_args() -> Option<Result<(), String>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mcp") => Some(run_mcp(&args[1..])),
        Some("open") => run_open(args.get(1).map(String::as_str)),
        Some("connections") => Some(run_connections(&args[1..])),
        // Handler skema URL OS memanggil `tabular "tabular://..."`.
        Some(url) if url.to_ascii_lowercase().starts_with("tabular:") => run_open(Some(url)),
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

/// Siapkan data dir seperti mode MCP: env `TABULAR_DATA_DIR` dari pemanggil
/// menang atas lokasi yang tersimpan dari GUI.
fn init_headless_data_dir() {
    crate::vector_index::register_sqlite_vec();
    dotenvy::dotenv().ok();
    match std::env::var("TABULAR_DATA_DIR") {
        Ok(dir) if !dir.trim().is_empty() => {}
        _ => crate::config::init_data_dir(),
    }
}

/// `tabular open <url>`: teruskan ke instance yang berjalan. Bila tidak ada,
/// kembalikan `None` agar proses ini lanjut menjadi GUI dengan URL di antrean.
fn run_open(url: Option<&str>) -> Option<Result<(), String>> {
    let Some(url) = url.map(str::trim).filter(|u| !u.is_empty()) else {
        return Some(Err(format!("`tabular open` needs a URL\n\n{USAGE}")));
    };
    if let Err(e) = crate::deeplink::parse(url) {
        return Some(Err(format!("cannot open {url:?}: {e}")));
    }
    init_headless_data_dir();
    match crate::single_instance::forward(url) {
        Ok(true) => {
            eprintln!("Sent to the running Tabular window.");
            return Some(Ok(()));
        }
        Ok(false) => {}
        Err(e) => return Some(Err(e)),
    }
    #[cfg(target_os = "macos")]
    if let Some(result) = open_via_launch_services(url) {
        return Some(result);
    }
    crate::deeplink::push_incoming(url);
    None
}

/// Binary CLI di dalam `.app`: minta LaunchServices meluncurkan bundle dan
/// mengirim URL lewat Apple Event, supaya aplikasi tidak terikat ke terminal.
#[cfg(target_os = "macos")]
fn open_via_launch_services(url: &str) -> Option<Result<(), String>> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_string_lossy().to_string();
    let bundle = &exe[..exe.find(".app/Contents/MacOS/")? + 4];
    // DSN mentah dibungkus agar sampai sebagai `tabular://import` (lengkap
    // dengan password; hanya lewat Apple Event lokal).
    let link = if url.to_ascii_lowercase().starts_with("tabular:") {
        url.to_string()
    } else {
        let enc: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
        format!("tabular://import?url={enc}")
    };
    let status = std::process::Command::new("/usr/bin/open")
        .args(["-a", bundle, &link])
        .status();
    Some(match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("`open -a` exited with {s}")),
        Err(e) => Err(format!("cannot launch Tabular: {e}")),
    })
}

#[derive(serde::Serialize)]
struct ConnectionListItem {
    id: i64,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    host: String,
    port: String,
    database: String,
    folder: Option<String>,
    environment: Option<&'static str>,
}

/// `tabular connections [--json]`: daftar koneksi untuk Raycast/skrip. Tidak
/// pernah memuat password atau `ConnectionConfig` utuh.
fn run_connections(rest: &[String]) -> Result<(), String> {
    let json = rest.iter().any(|a| a == "--json");
    if let Some(unknown) = rest.iter().find(|a| a.as_str() != "--json") {
        return Err(format!(
            "unknown option for `tabular connections`: {unknown}\n\n{USAGE}"
        ));
    }
    init_headless_data_dir();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to start async runtime: {e}"))?;
    let items = runtime.block_on(async {
        let pool = open_cache_pool().await.map_err(|e| e.to_string())?;
        let session = HeadlessSession::new(pool.clone());
        let list = session
            .list_connections()
            .await
            .map_err(|e| e.to_string())?;
        let envs = crate::connection_env::load_all(&pool)
            .await
            .unwrap_or_default();
        Ok::<_, String>(
            list.into_iter()
                .map(|c| ConnectionListItem {
                    environment: crate::connection_env::effective(
                        envs.get(&c.id).copied(),
                        &c.name,
                    )
                    .map(|e| e.key()),
                    id: c.id,
                    name: c.name,
                    kind: c.kind,
                    host: c.host,
                    port: c.port,
                    database: c.database,
                    folder: c.folder,
                })
                .collect::<Vec<_>>(),
        )
    })?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?
        );
    } else {
        for c in items {
            let folder = c.folder.map(|f| format!("{f}/")).unwrap_or_default();
            let env = c.environment.map(|e| format!(" [{e}]")).unwrap_or_default();
            println!(
                "{:>4}  {folder}{}  ({} {}:{}/{}){env}",
                c.id, c.name, c.kind, c.host, c.port, c.database
            );
        }
    }
    Ok(())
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

    // Stack worker 8 MB (bawaan 2 MB): rantai future eksekusi query (driver,
    // SSH tunnel, profiler) dalam dan besar di build debug.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 * 1024 * 1024)
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
