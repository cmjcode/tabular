use eframe::egui;

mod driver_sqlite;
mod export;
mod models;
mod modules;
mod window_egui;
mod driver_mysql;
mod cache_data;
mod driver_postgres;
mod driver_redis;
mod driver_mssql;
mod driver_mongodb;
mod directory;
mod connection;
mod sidebar_database;
mod sidebar_history;
mod sidebar_query;
mod editor;
mod editor_autocomplete;
mod dialog;
mod config;

fn main() -> Result<(), eframe::Error> {
    dotenv::dotenv().ok();
    
    // Initialize logger first so we can see logs from init_data_dir
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .filter_module("tabular", log::LevelFilter::Debug)
        .init();
    
    // Initialize data directory configuration
    config::init_data_dir();
    
    // Log the final data directory being used
    log::info!("Application starting with data directory: {}", config::get_data_dir().display());
    
    let mut options = eframe::NativeOptions::default();
    
    // Set window size
    options.viewport.inner_size = Some(egui::vec2(1400.0, 900.0));
    options.viewport.min_inner_size = Some(egui::vec2(800.0, 600.0));
    
    // Set window icon
    if let Some(icon) = modules::load_icon() {
        options.viewport.icon = Some(std::sync::Arc::new(icon));
    }
    
    eframe::run_native(
        "Tabular",
        options,
        Box::new(|_cc| Ok(Box::new(window_egui::Tabular::new()))),
    )
}





