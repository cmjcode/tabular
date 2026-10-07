//! Antarmuka Tabular untuk agent AI (harness seperti Claude Code, Cursor,
//! Codex, dan MCP client lain).
//!
//! Struktur:
//! - [`classify`]: gerbang read-only (klasifikasi statement SQL/Redis).
//! - [`core`]: sesi headless di atas `connections.db` dan pool driver; tidak
//!   bergantung pada egui sama sekali.
//! - [`knowledge`]: pengetahuan di luar skema mentah (diagram, riwayat
//!   query, analisis statement, pemakaian tabel di repository).
//! - [`mcp`]: server Model Context Protocol lewat stdio (`tabular mcp`).
//! - [`cli`]: parsing argumen baris perintah sebelum GUI dijalankan.
//! - [`harness`]: arah sebaliknya — Tabular menjalankan CLI agent (`agy`,
//!   `claude`, `gemini`) sebagai backend panel AI Assistant.
//! - [`projects`]: konteks project (environment, koneksi, memory agent).
//! - [`live_edit`]: protokol blok `sql tabular:tab=…` yang ditulis agent
//!   langsung ke tab editor.
//!
//! `mcp` dan `cli` tidak dikompilasi di iOS karena App Store tidak mengizinkan
//! proses tanpa UI dan tidak ada stdio untuk dipakai. `harness` ikut
//! dikompilasi di semua target supaya tipe state UI seragam; di mobile backend
//! CLI disembunyikan dari pengaturan.

pub mod access;
pub mod classify;
pub mod core;
pub mod harness;
pub mod knowledge;
pub mod live_edit;
pub mod ops;
pub mod projects;

#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub mod cli;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub mod mcp;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub mod mcp_resources;
