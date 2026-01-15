use eframe::egui;

pub mod auto_updater;
pub mod cache_data;
pub mod config;
pub mod connection;
pub mod data_table;
pub mod dialog;
pub mod directory;
pub mod driver_mongodb;
pub mod driver_mssql;
pub mod driver_mysql;
pub mod driver_postgres;
pub mod driver_redis;
pub mod driver_sqlite;
pub mod editor;
pub mod editor_autocomplete;
pub mod editor_autocomplete_new; // temporary clean implementation backing the shim
pub mod editor_buffer;
pub mod editor_selection;
pub mod editor_state_adapter;
pub mod export;
pub mod models;
pub mod modules;
pub mod query_tools;
pub mod self_update;
pub mod sidebar_database;
pub mod sidebar_history;
pub mod sidebar_query;
pub mod spreadsheet;
pub mod ssh_tunnel;
// Unified syntax / parsing module (legacy highlighter + optional tree-sitter parsing)
#[cfg(feature = "query_ast")]
pub mod query_ast;
pub mod syntax_ts;
pub mod window_egui; // re-enabled syntax highlighting helpers

/// Reusable entrypoint so other launchers (e.g., iOS) can run the UI.
pub fn run() -> Result<(), eframe::Error> {
    dotenv::dotenv().ok();
    config::init_data_dir();

    // 1. Load preferences early to determine log level
    // We use a temporary runtime because ConfigStore requires async and we haven't started our main runtime yet.
    let prefs = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create temp runtime for config load");
        rt.block_on(async {
            match config::ConfigStore::new().await {
                Ok(store) => store.load().await,
                Err(_) => config::AppPreferences::default(),
            }
        })
    };

    let log_level = if prefs.enable_debug_logging {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Warn
    };

    let _ = env_logger::Builder::from_default_env()
        // Enable info-level logs for our crate so users can see data source messages
        .filter_module("tabular", log_level)
        .is_test(false)
        .try_init();
    
    log::info!(
        "Application starting with data directory: {}",
        config::get_data_dir().display()
    );
    if prefs.enable_debug_logging {
        log::info!("Debug logging enabled");
    }

    let mut options = eframe::NativeOptions::default();
    options.viewport.inner_size = Some(egui::vec2(1600.0, 1000.0));
    options.viewport.min_inner_size = Some(egui::vec2(800.0, 600.0));
    if let Some(icon) = modules::load_icon() {
        options.viewport.icon = Some(std::sync::Arc::new(icon));
    }
    
    let initial_prefs = prefs.clone();
    eframe::run_native(
        "Tabular",
        options,
        Box::new(move |_cc| {
            let mut app = window_egui::Tabular::new();
            app.set_initial_prefs(initial_prefs);
            Ok(Box::new(app))
        }),
    )
}

// ----------------- FFI (iOS) -----------------
// Basic exported C ABI helpers so Swift can query version and (optionally) launch.
use std::os::raw::c_char;

#[unsafe(no_mangle)]
pub extern "C" fn tabular_version() -> *const c_char {
    // Compile-time string; ends with NUL for C.
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

#[unsafe(no_mangle)]
pub extern "C" fn tabular_run() -> i32 {
    match run() {
        Ok(_) => 0,
        Err(_) => 1,
    }
}
