//! Engine autocomplete SQL yang sadar konteks.
//!
//! Pipeline tiga tahap, semuanya murni (tanpa akses `Tabular`):
//! 1. [`lexer`] — tokenisasi toleran terhadap SQL setengah jadi.
//! 2. [`analyzer`] — klausa aktif, apa yang diharapkan di kursor, scope tabel/alias/CTE.
//! 3. [`engine`] — provider kandidat per konteks + ranking.
//!
//! Glue ke UI/cache ada di `editor_autocomplete_new.rs`.

pub mod analyzer;
pub mod engine;
pub mod lexer;

pub use analyzer::{Analysis, Clause, Expect, analyze};
pub use engine::{
    CURSOR_MARK, Catalog, ColumnMeta, CompletionItem, ItemKind, Options, complete, fuzzy_match,
};
pub use lexer::Dialect;
