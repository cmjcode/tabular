// connection/mod.rs
// Splits the original monolithic connection.rs into focused submodules.
//
// Module structure:
//   types    – shared data structures (QueryJob, QueryResultMessage, etc.)
//   sql      – SQL analysis helpers (pagination detection, simple-select check, etc.)
//   pool     – connection pool creation, caching, and lifecycle management
//   execute  – per-driver async query execution
//   metadata – schema/database/column discovery and row-cache prefetch
//   crud     – connection CRUD (update, remove, test) + background refresh
//   ui       – egui connection-selector popup

pub mod types;
pub mod sql;
pub mod pool;
pub mod execute;
pub mod metadata;
pub mod crud;
pub mod session;
pub mod ui;

// ── Re-exports ────────────────────────────────────────────────────────────────
// Keep the same API surface that the rest of the crate expects.

// Types
pub(crate) use types::{QueryJobStatus, QueryResultMessage};

// SQL utilities
pub(crate) use sql::{
    add_auto_limit_if_needed, should_enable_auto_pagination, split_sql_statements,
};

// Pool management
pub(crate) use pool::{
    cleanup_connection_pool, ensure_background_pool_creation, get_or_create_connection_pool,
    start_background_pool_creation,
};

// Query execution
pub(crate) use execute::{
    execute_query_with_connection, prepare_query_job, spawn_query_job, spawn_query_job_batch,
};

// Metadata / schema discovery
pub use metadata::fetch_databases_background_task; // fully pub in original
#[allow(deprecated)]
pub(crate) use metadata::{
    compute_schema_diff,
    fetch_columns_from_database,
    fetch_databases_from_connection_async, fetch_databases_from_connection_blocking,
    fetch_procedure_definition, fetch_table_definition, fetch_view_definition, get_foreign_keys,
};

// Connection CRUD + testing
pub(crate) use crud::{
    refresh_connection_background_async, remove_connection, test_database_connection,
    update_connection_in_database,
};

// UI
pub(crate) use ui::render_connection_selector;
