use eframe::egui;

pub mod ai_assistant;
pub mod auto_updater;
pub mod cache_data;
pub mod config;
pub mod connection;
pub mod curl_import;
pub mod data_table;
pub mod diagram_view;
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
pub mod http_client;
pub mod http_code_export;
pub mod http_collection;
pub mod models;
pub mod modules;
pub mod query_tools;
pub mod redis_browser;
pub mod safety_guard;
pub mod secrets;
pub mod self_update;
pub mod sidebar_collection;
pub mod sidebar_database;
pub mod sidebar_history;
pub mod sidebar_query;
pub mod spreadsheet;
pub mod ssh_tunnel;
pub mod sync;
// Unified syntax / parsing module (legacy highlighter + optional tree-sitter parsing)
#[cfg(feature = "query_ast")]
pub mod query_ast;
pub mod syntax_ts;
pub mod window_egui; // re-enabled syntax highlighting helpers

#[cfg(not(target_os = "ios"))]
pub use ::rfd;

#[cfg(target_os = "ios")]
pub mod rfd {
    use std::path::PathBuf;

    #[derive(Default, Clone)]
    pub struct FileDialog;

    impl FileDialog {
        pub fn new() -> Self {
            Self
        }

        pub fn add_filter<S: AsRef<str>, T: AsRef<str>>(self, _name: S, _ext: &[T]) -> Self {
            self
        }

        pub fn set_file_name<S: AsRef<str>>(self, _name: S) -> Self {
            self
        }

        pub fn set_title<S: AsRef<str>>(self, _title: S) -> Self {
            self
        }

        pub fn set_directory<P: AsRef<std::path::Path>>(self, _dir: P) -> Self {
            self
        }

        pub fn pick_file(self) -> Option<PathBuf> {
            None
        }

        pub fn pick_files(self) -> Option<Vec<PathBuf>> {
            None
        }

        pub fn save_file(self) -> Option<PathBuf> {
            None
        }

        pub fn pick_folder(self) -> Option<PathBuf> {
            None
        }
    }

    #[derive(Default, Clone)]
    pub struct MessageDialog;

    impl MessageDialog {
        pub fn new() -> Self {
            Self
        }

        pub fn set_title<S: AsRef<str>>(self, _title: S) -> Self {
            self
        }

        pub fn set_description<S: AsRef<str>>(self, _desc: S) -> Self {
            self
        }

        pub fn show(self) {}
    }
}

pub static STARTUP_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn log_startup_step(step: &str) {
    let start = *STARTUP_TIME.get_or_init(std::time::Instant::now);
    let elapsed = start.elapsed();
    eprintln!("[STARTUP-TIMER {:>7.2?}] {}", elapsed, step);
}

/// Reusable entrypoint so other launchers (e.g., iOS) can run the UI.
pub fn run() -> Result<(), eframe::Error> {
    log_startup_step("run() entrypoint started");
    dotenv::dotenv().ok();
    log_startup_step("dotenv loaded");
    config::init_data_dir();
    log_startup_step("init_data_dir completed");

    let _ = env_logger::Builder::from_default_env()
        // Enable info-level logs for our crate so users can see data source messages
        .filter_module("tabular", log::LevelFilter::Info)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing", log::LevelFilter::Warn)
        .is_test(false)
        .try_init();

    log::debug!(
        "Application starting with data directory: {}",
        config::get_data_dir().display()
    );

    let mut options = eframe::NativeOptions::default();
    options.viewport.inner_size = Some(egui::vec2(1600.0, 1000.0));
    options.viewport.min_inner_size = Some(egui::vec2(800.0, 600.0));
    if let Some(icon) = modules::load_icon() {
        options.viewport.icon = Some(std::sync::Arc::new(icon));
    }
    log_startup_step("starting eframe::run_native");

    eframe::run_native(
        "Tabular",
        options,
        Box::new(move |_cc| {
            log_startup_step("eframe creation closure entered");
            let app = window_egui::Tabular::new();
            log_startup_step("Tabular::new() returned");
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
    let result = std::panic::catch_unwind(|| {
        match run() {
            Ok(_) => 0,
            Err(e) => {
                log::error!("eframe run error: {:?}", e);
                1
            }
        }
    });
    match result {
        Ok(code) => code,
        Err(_) => {
            log::error!("tabular_run encountered an unhandled panic");
            -1
        }
    }
}
