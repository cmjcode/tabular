//! Objek skema database: builder SQL (rename, comment, maintenance, materialized
//! view, schema, tipe buatan user, source routine) dan eksekutor headless.
//!
//! Modul ini tidak bergantung pada `window_egui`; GUI (`window_egui::schema_actions`)
//! dan test memanggilnya dengan data biasa.

pub mod catalog;
pub mod exec;
pub mod sql;

pub use exec::{ResultSet, run_in_database};
pub use sql::MaintenanceOp;

use std::sync::atomic::{AtomicBool, Ordering};

/// Preferensi "Show system databases/schemas". Disimpan global karena daftar
/// database diambil di task latar belakang yang tidak memegang state GUI.
static SHOW_SYSTEM_OBJECTS: AtomicBool = AtomicBool::new(false);

pub fn show_system_objects() -> bool {
    SHOW_SYSTEM_OBJECTS.load(Ordering::Relaxed)
}

pub fn set_show_system_objects(show: bool) {
    SHOW_SYSTEM_OBJECTS.store(show, Ordering::Relaxed);
}

/// True jika database ini harus disembunyikan dari daftar database.
pub fn hide_database(db: &crate::models::enums::DatabaseType, name: &str) -> bool {
    !show_system_objects() && catalog::is_system_database(db, name)
}

/// Filter SQL daftar database PostgreSQL sesuai preferensi.
pub fn pg_database_list_sql() -> &'static str {
    if show_system_objects() {
        "SELECT datname FROM pg_database WHERE datallowconn ORDER BY datname"
    } else {
        "SELECT datname FROM pg_database WHERE datistemplate = false AND datname NOT IN ('postgres', 'template0', 'template1')"
    }
}
