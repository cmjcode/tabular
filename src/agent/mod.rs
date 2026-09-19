//! Antarmuka Tabular untuk agent AI (harness seperti Claude Code, Cursor,
//! Codex, dan MCP client lain).
//!
//! Struktur:
//! - [`classify`]: gerbang read-only (klasifikasi statement SQL/Redis).
//! - [`core`]: sesi headless di atas `connections.db` dan pool driver; tidak
//!   bergantung pada egui sama sekali.
//! - [`mcp`]: server Model Context Protocol lewat stdio (`tabular mcp`).
//! - [`cli`]: parsing argumen baris perintah sebelum GUI dijalankan.
//! - [`harness`]: arah sebaliknya — Tabular menjalankan CLI agent (`agy`,
//!   `claude`, `gemini`) sebagai backend panel AI Assistant.
//! - [`live_edit`]: protokol blok `sql tabular:tab=…` yang ditulis agent
//!   langsung ke tab editor.
//!
//! `mcp` dan `cli` tidak dikompilasi di iOS karena App Store tidak mengizinkan
//! proses tanpa UI dan tidak ada stdio untuk dipakai. `harness` ikut
//! dikompilasi di semua target supaya tipe state UI seragam; di mobile backend
//! CLI disembunyikan dari pengaturan.

pub mod classify;
pub mod core;
pub mod harness;
pub mod live_edit;

#[cfg(not(target_os = "ios"))]
pub mod cli;
#[cfg(not(target_os = "ios"))]
pub mod mcp;
