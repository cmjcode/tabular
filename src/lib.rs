use eframe::egui;

pub mod auto_updater;
pub mod cache_data;
pub mod config;
pub mod connection;
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
pub mod export;
pub mod models;
pub mod modules;
pub mod self_update;
pub mod sidebar_database;
pub mod sidebar_history;
pub mod sidebar_query;
pub mod window_egui;

/// Reusable entrypoint so other launchers (e.g., iOS) can run the UI.
pub fn run() -> Result<(), eframe::Error> {
    dotenv::dotenv().ok();
    let _ = env_logger::Builder::from_default_env()
        .filter_module("tabular", log::LevelFilter::Debug)
        .is_test(false)
        .try_init();
    config::init_data_dir();
    log::info!(
        "Application starting with data directory: {}",
        config::get_data_dir().display()
    );
    let mut options = eframe::NativeOptions::default();
    options.viewport.inner_size = Some(egui::vec2(1400.0, 900.0));
    options.viewport.min_inner_size = Some(egui::vec2(800.0, 600.0));
    if let Some(icon) = modules::load_icon() {
        options.viewport.icon = Some(std::sync::Arc::new(icon));
    }
    eframe::run_native(
        "Tabular",
        options,
        Box::new(|_cc| Ok(Box::new(window_egui::Tabular::new()))),
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
