//! Engine autocomplete SQL yang sadar konteks.
//!
//! Pipeline tiga tahap, semuanya murni (tanpa akses `Tabular`):
//! 1. [`lexer`] — tokenisasi toleran terhadap SQL setengah jadi.
//! 2. [`analyzer`] — klausa aktif, apa yang diharapkan di kursor, scope tabel/alias/CTE.
//! 3. [`engine`] — provider kandidat per konteks + ranking.
//! 4. [`index_advisor`] — rekomendasi index/performa per statement.
//! 5. [`usage`] — statistik pemakaian dari riwayat query (sinyal ranking).
//!
//! Glue ke UI/cache ada di `editor_autocomplete_new.rs`.

pub mod analyzer;
pub mod engine;
pub mod index_advisor;
pub mod lexer;
pub mod usage;

pub use analyzer::{Analysis, Clause, Expect, analyze};
pub use engine::{
    CURSOR_MARK, Catalog, ColumnMeta, CompletionItem, ItemKind, Options, complete, fuzzy_match,
};
pub use index_advisor::{
    AdviceLevel, ColumnStatus, ColumnUsage, IndexAdvice, IndexCatalog, IndexDef, QueryReport,
    advise, report,
};
pub use lexer::Dialect;
pub use usage::UsageStats;
