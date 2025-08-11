use eframe::egui;

mod helpers;
mod driver_sqlite;
mod export;
mod models;
mod modules;
mod window_egui;
mod driver_mysql;
mod cache_data;
mod driver_postgres;
mod driver_redis;
mod directory;
mod connection;
mod sidebar_database;
mod sidebar_history;
mod sidebar_query;
mod editor;
mod editor_autocomplete;
mod dialog;

fn main() -> Result<(), eframe::Error> {
    dotenv::dotenv().ok();
    // Initialize logger with filter to suppress winit debug logs
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .filter_module("tabular", log::LevelFilter::Debug)
        .init();
    
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





