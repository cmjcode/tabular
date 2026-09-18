//! Penyimpanan dan pemulihan sesi editor ("hot exit").
//!
//! Tab query (termasuk draft yang belum disimpan) dan ukuran window disimpan
//! berkala ke `<data_dir>/session.json`, lalu dipulihkan saat aplikasi dibuka
//! lagi. Modul ini juga menangani konfirmasi saat menutup tab yang punya
//! perubahan belum disimpan dan saat keluar aplikasi.

use eframe::egui;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::{editor, window_egui::Tabular};

/// Versi format file sesi; naikkan jika struktur berubah tidak kompatibel.
const SESSION_VERSION: u32 = 1;
/// Jeda minimum antar pengecekan perubahan sesi.
const SAVE_INTERVAL: Duration = Duration::from_secs(2);
/// Batas ukuran konten per tab yang disimpan (draft raksasa dilewati).
const MAX_TAB_CONTENT_BYTES: usize = 5 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionTab {
    pub title: String,
    pub content: String,
    pub file_path: Option<String>,
    pub connection_id: Option<i64>,
    pub database_name: Option<String>,
    pub is_pinned: bool,
    pub is_modified: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub version: u32,
    pub active_index: usize,
    pub tabs: Vec<SessionTab>,
    pub window: Option<WindowGeometry>,
}

/// Aksi tutup tab yang menunggu konfirmasi user. Tab dirujuk lewat
/// `QueryTab::id` agar tetap valid walaupun urutan tab berubah.
#[derive(Clone, Debug)]
pub enum PendingTabClose {
    Single { tab_id: usize },
    Others { keep_tab_id: usize },
    ToTheRight { from_tab_id: usize },
}

fn session_path() -> PathBuf {
    crate::config::get_data_dir().join("session.json")
}

/// Tab editor SQL biasa. Tab khusus (HTTP, monitor DBA, user manager, Redis,
/// diagram) dan tab hasil browse tabel tidak ikut disimpan.
fn is_plain_query_tab(tab: &crate::models::structs::QueryTab) -> bool {
    tab.http_client_state.is_none()
        && tab.dba_monitor_state.is_none()
        && tab.user_manager_state.is_none()
        && tab.redis_browser_state.is_none()
        && tab.diagram_state.is_none()
        && !tab.is_table_browse_mode
}

/// Konten terbaru sebuah tab. Untuk tab aktif, teks editor adalah sumber
/// kebenaran karena `tab.content` baru disinkronkan saat pindah tab.
fn current_content(tabular: &Tabular, index: usize) -> &str {
    if index == tabular.active_tab_index {
        &tabular.editor.text
    } else {
        &tabular.query_tabs[index].content
    }
}

/// True jika tab punya perubahan yang akan hilang bila ditutup.
pub fn tab_has_unsaved_changes(tabular: &Tabular, index: usize) -> bool {
    let Some(tab) = tabular.query_tabs.get(index) else {
        return false;
    };
    if !is_plain_query_tab(tab) || current_content(tabular, index).trim().is_empty() {
        return false;
    }
    let editor_diverged = index == tabular.active_tab_index && tab.content != tabular.editor.text;
    tab.is_modified || editor_diverged
}

fn snapshot(tabular: &Tabular, window: Option<WindowGeometry>) -> SessionSnapshot {
    let mut tabs = Vec::new();
    let mut active_index = 0;
    for (index, tab) in tabular.query_tabs.iter().enumerate() {
        if !is_plain_query_tab(tab) {
            continue;
        }
        let content = current_content(tabular, index);
        if content.len() > MAX_TAB_CONTENT_BYTES {
            continue;
        }
        if index == tabular.active_tab_index {
            active_index = tabs.len();
        }
        tabs.push(SessionTab {
            title: tab.title.clone(),
            content: content.to_string(),
            file_path: tab.file_path.clone(),
            connection_id: tab.connection_id,
            database_name: tab.database_name.clone(),
            is_pinned: tab.is_pinned,
            is_modified: tab_has_unsaved_changes(tabular, index),
        });
    }
    SessionSnapshot {
        version: SESSION_VERSION,
        active_index,
        tabs,
        window,
    }
}

fn fingerprint(snapshot: &SessionSnapshot) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    snapshot.active_index.hash(&mut hasher);
    for tab in &snapshot.tabs {
        tab.title.hash(&mut hasher);
        tab.content.hash(&mut hasher);
        tab.file_path.hash(&mut hasher);
        tab.connection_id.hash(&mut hasher);
        tab.database_name.hash(&mut hasher);
        tab.is_pinned.hash(&mut hasher);
        tab.is_modified.hash(&mut hasher);
    }
    if let Some(w) = snapshot.window {
        (w.width as i32, w.height as i32, w.maximized).hash(&mut hasher);
    }
    hasher.finish()
}

fn write_atomically(path: &std::path::Path, json: &str) -> std::io::Result<()> {
    crate::directory::write_file_atomically(path, json.as_bytes())
}

/// Thread penulis tunggal: menerima JSON terbaru dan hanya menulis versi
/// paling akhir, sehingga penulisan tidak pernah terjadi di thread UI dan
/// urutan tulis selalu benar.
fn writer() -> &'static std::sync::mpsc::Sender<String> {
    static WRITER: OnceLock<std::sync::mpsc::Sender<String>> = OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("session-writer".into())
            .spawn(move || {
                while let Ok(mut json) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        json = newer;
                    }
                    if let Err(e) = write_atomically(&session_path(), &json) {
                        log::warn!("Failed to save session: {}", e);
                    }
                }
            })
            .ok();
        tx
    })
}

fn window_geometry(ctx: &egui::Context) -> Option<WindowGeometry> {
    ctx.input(|i| {
        let viewport = i.viewport();
        let rect = viewport.inner_rect?;
        Some(WindowGeometry {
            width: rect.width(),
            height: rect.height(),
            maximized: viewport.maximized.unwrap_or(false),
        })
    })
}

/// Dipanggil setiap frame. Menyimpan sesi (di thread latar) jika ada
/// perubahan sejak penyimpanan terakhir.
pub fn tick(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.restore_session || !tabular.session_restore_done {
        return;
    }
    let now = Instant::now();
    if tabular
        .session_last_check
        .is_some_and(|last| now.duration_since(last) < SAVE_INTERVAL)
    {
        return;
    }
    tabular.session_last_check = Some(now);
    let snap = snapshot(tabular, window_geometry(ctx));
    let fp = fingerprint(&snap);
    if tabular.session_last_fingerprint == Some(fp) {
        return;
    }
    if let Ok(json) = serde_json::to_string(&snap) {
        let _ = writer().send(json);
        tabular.session_last_fingerprint = Some(fp);
    }
}

/// Simpan sesi secara sinkron (dipakai saat aplikasi akan ditutup).
pub fn save_now(tabular: &Tabular, ctx: Option<&egui::Context>) {
    if !tabular.restore_session {
        let _ = std::fs::remove_file(session_path());
        return;
    }
    // Tanpa context (mis. dari on_exit) pertahankan ukuran window yang tersimpan.
    let window = ctx
        .and_then(window_geometry)
        .or_else(|| load().and_then(|s| s.window));
    let snap = snapshot(tabular, window);
    match serde_json::to_string(&snap) {
        Ok(json) => {
            if let Err(e) = write_atomically(&session_path(), &json) {
                log::warn!("Failed to save session on exit: {}", e);
            }
        }
        Err(e) => log::warn!("Failed to serialize session: {}", e),
    }
}

fn load() -> Option<SessionSnapshot> {
    let content = std::fs::read_to_string(session_path()).ok()?;
    match serde_json::from_str::<SessionSnapshot>(&content) {
        Ok(snap) if snap.version == SESSION_VERSION => Some(snap),
        Ok(_) => None,
        Err(e) => {
            log::warn!("Ignoring unreadable session file: {}", e);
            None
        }
    }
}

/// Ukuran window dari sesi sebelumnya, untuk `NativeOptions` saat startup.
/// Hanya ukuran yang dipulihkan (bukan posisi) agar window tidak muncul di
/// luar layar ketika konfigurasi monitor berubah.
pub fn saved_window_geometry() -> Option<WindowGeometry> {
    load()?.window.filter(|w| {
        w.width.is_finite() && w.height.is_finite() && w.width >= 400.0 && w.height >= 300.0
    })
}

/// Pulihkan tab dari sesi sebelumnya. Hanya dijalankan sekali, dan hanya jika
/// user belum mulai bekerja di tab awal yang kosong.
pub fn restore_on_startup(tabular: &mut Tabular) {
    if tabular.session_restore_done {
        return;
    }
    tabular.session_restore_done = true;
    if !tabular.restore_session {
        return;
    }
    let Some(snap) = load() else {
        return;
    };
    if snap.tabs.is_empty() {
        return;
    }
    let untouched_start = tabular.query_tabs.len() == 1
        && tabular.editor.text.trim().is_empty()
        && is_plain_query_tab(&tabular.query_tabs[0])
        && tabular.query_tabs[0].file_path.is_none();
    if !untouched_start {
        return;
    }

    tabular.query_tabs.clear();
    tabular.active_tab_index = 0;
    let mut restored_drafts = 0;
    for saved in &snap.tabs {
        // File yang tidak diubah dibaca ulang dari disk supaya perubahan dari
        // luar aplikasi ikut terlihat; jika file hilang, konten sesi dipakai.
        let (content, is_modified) = match (&saved.file_path, saved.is_modified) {
            (Some(path), false) => match std::fs::read_to_string(path) {
                Ok(disk) => (disk, false),
                Err(_) => (saved.content.clone(), true),
            },
            _ => (saved.content.clone(), saved.is_modified),
        };
        if is_modified {
            restored_drafts += 1;
        }
        editor::create_new_tab_with_connection_and_database(
            tabular,
            saved.title.clone(),
            content,
            saved.connection_id,
            saved.database_name.clone(),
        );
        if let Some(tab) = tabular.query_tabs.last_mut() {
            tab.file_path = saved.file_path.clone();
            tab.is_saved = saved.file_path.is_some() && !is_modified;
            tab.is_modified = is_modified;
            tab.is_pinned = saved.is_pinned;
        }
    }
    if tabular.query_tabs.is_empty() {
        editor::create_new_tab(tabular, "Untitled Query".to_string(), String::new());
        return;
    }
    let target = snap.active_index.min(tabular.query_tabs.len() - 1);
    if target != tabular.active_tab_index {
        // Teks editor identik dengan konten tab aktif, jadi switch_to_tab
        // tidak mengubah status modified yang dipulihkan dari sesi.
        editor::switch_to_tab(tabular, target);
    }
    tabular.current_connection_id = tabular
        .query_tabs
        .get(tabular.active_tab_index)
        .and_then(|t| t.connection_id);
    if restored_drafts > 0 {
        tabular.toasts.info(format!(
            "Restored {} tab(s) from your last session, including {} unsaved draft(s).",
            tabular.query_tabs.len(),
            restored_drafts
        ));
    }
}

/// Minta penutupan satu tab; tampilkan konfirmasi jika ada perubahan.
pub fn request_close_tab(tabular: &mut Tabular, index: usize) {
    if tab_has_unsaved_changes(tabular, index) {
        tabular.pending_tab_close = Some(PendingTabClose::Single {
            tab_id: tabular.query_tabs[index].id,
        });
    } else {
        editor::close_tab(tabular, index);
    }
}

/// Minta penutupan semua tab lain (kecuali yang di-pin).
pub fn request_close_other_tabs(tabular: &mut Tabular, keep_index: usize) {
    let Some(keep) = tabular.query_tabs.get(keep_index) else {
        return;
    };
    let keep_tab_id = keep.id;
    let affected = (0..tabular.query_tabs.len())
        .filter(|&i| i != keep_index && !tabular.query_tabs[i].is_pinned)
        .any(|i| tab_has_unsaved_changes(tabular, i));
    if affected {
        tabular.pending_tab_close = Some(PendingTabClose::Others { keep_tab_id });
    } else {
        editor::close_other_tabs(tabular, keep_index);
    }
}

/// Minta penutupan tab di sebelah kanan `index` (kecuali yang di-pin).
pub fn request_close_tabs_to_the_right(tabular: &mut Tabular, index: usize) {
    let Some(from) = tabular.query_tabs.get(index) else {
        return;
    };
    let from_tab_id = from.id;
    let affected = (index + 1..tabular.query_tabs.len())
        .filter(|&i| !tabular.query_tabs[i].is_pinned)
        .any(|i| tab_has_unsaved_changes(tabular, i));
    if affected {
        tabular.pending_tab_close = Some(PendingTabClose::ToTheRight { from_tab_id });
    } else {
        editor::close_tabs_to_the_right(tabular, index);
    }
}

fn index_of(tabular: &Tabular, tab_id: usize) -> Option<usize> {
    tabular.query_tabs.iter().position(|t| t.id == tab_id)
}

/// Judul tab yang akan kehilangan perubahan jika aksi dilanjutkan.
fn unsaved_titles(tabular: &Tabular, pending: &PendingTabClose) -> Vec<String> {
    let indices: Vec<usize> = match pending {
        PendingTabClose::Single { tab_id } => index_of(tabular, *tab_id).into_iter().collect(),
        PendingTabClose::Others { keep_tab_id } => {
            let keep = index_of(tabular, *keep_tab_id);
            (0..tabular.query_tabs.len())
                .filter(|&i| Some(i) != keep && !tabular.query_tabs[i].is_pinned)
                .collect()
        }
        PendingTabClose::ToTheRight { from_tab_id } => match index_of(tabular, *from_tab_id) {
            Some(from) => (from + 1..tabular.query_tabs.len())
                .filter(|&i| !tabular.query_tabs[i].is_pinned)
                .collect(),
            None => Vec::new(),
        },
    };
    indices
        .into_iter()
        .filter(|&i| tab_has_unsaved_changes(tabular, i))
        .map(|i| tabular.query_tabs[i].title.clone())
        .collect()
}

/// Dialog konfirmasi untuk `pending_tab_close`.
pub fn render_close_tab_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    let Some(pending) = tabular.pending_tab_close.clone() else {
        return;
    };
    let titles = unsaved_titles(tabular, &pending);
    if titles.is_empty() {
        // Perubahan sudah tersimpan atau tab sudah hilang: lanjutkan tanpa bertanya.
        tabular.pending_tab_close = None;
        perform_close(tabular, &pending);
        return;
    }

    enum Choice {
        Discard,
        Save,
        Cancel,
    }
    let mut choice = None;
    let mut close = false;
    crate::window_egui::style::render_modal_backdrop(ctx, "close_tab_confirm", true);
    egui::Window::new("Unsaved Changes")
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(380.0)
        .show(ctx, |ui| {
            crate::window_egui::style::render_modal_header(ui, "Unsaved Changes", &mut close);
            ui.add_space(8.0);

            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                if titles.len() == 1 {
                    ui.label(format!(
                        "“{}” has changes that have not been saved.",
                        titles[0]
                    ));
                } else {
                    ui.label(format!(
                        "{} tabs have changes that have not been saved:",
                        titles.len()
                    ));
                    for title in titles.iter().take(8) {
                        ui.label(egui::RichText::new(format!("• {}", title)).monospace());
                    }
                    if titles.len() > 8 {
                        ui.label(format!("…and {} more", titles.len() - 8));
                    }
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let discard = egui::Button::new(
                        egui::RichText::new("Close Without Saving")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(crate::window_egui::style::theme_danger(ctx));
                    if ui.add(discard).clicked() {
                        choice = Some(Choice::Discard);
                    }
                    if matches!(pending, PendingTabClose::Single { .. })
                        && ui.button("Save…").clicked()
                    {
                        choice = Some(Choice::Save);
                    }
                });
            });
        });

    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        choice = Some(Choice::Cancel);
    }

    match choice {
        Some(Choice::Discard) => {
            tabular.pending_tab_close = None;
            perform_close(tabular, &pending);
        }
        Some(Choice::Save) => {
            tabular.pending_tab_close = None;
            if let PendingTabClose::Single { tab_id } = pending
                && let Some(index) = index_of(tabular, tab_id)
            {
                if index != tabular.active_tab_index {
                    editor::switch_to_tab(tabular, index);
                }
                if let Err(e) = editor::save_current_tab(tabular) {
                    tabular.toasts.error(format!("Save failed: {}", e));
                }
            }
        }
        Some(Choice::Cancel) => tabular.pending_tab_close = None,
        None => {}
    }
}

fn perform_close(tabular: &mut Tabular, pending: &PendingTabClose) {
    match *pending {
        PendingTabClose::Single { tab_id } => {
            if let Some(index) = index_of(tabular, tab_id) {
                editor::close_tab(tabular, index);
            }
        }
        PendingTabClose::Others { keep_tab_id } => {
            if let Some(index) = index_of(tabular, keep_tab_id) {
                editor::close_other_tabs(tabular, index);
            }
        }
        PendingTabClose::ToTheRight { from_tab_id } => {
            if let Some(index) = index_of(tabular, from_tab_id) {
                editor::close_tabs_to_the_right(tabular, index);
            }
        }
    }
}

/// Tangani permintaan tutup window: batalkan penutupan dan tampilkan
/// konfirmasi jika ada transaksi terbuka atau draft yang akan hilang; jika
/// tidak, simpan sesi dan biarkan aplikasi tertutup.
pub fn handle_close_request(tabular: &mut Tabular, ctx: &egui::Context) {
    if !ctx.input(|i| i.viewport().close_requested()) {
        return;
    }
    if tabular.quit_confirmed {
        save_now(tabular, Some(ctx));
        return;
    }
    let open_transactions = tabular.query_tabs.iter().any(|t| t.tx_active);
    let unsaved_without_restore = !tabular.restore_session
        && (0..tabular.query_tabs.len()).any(|i| tab_has_unsaved_changes(tabular, i));
    if open_transactions || unsaved_without_restore {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        tabular.show_quit_confirm = true;
    } else {
        save_now(tabular, Some(ctx));
    }
}

/// Dialog konfirmasi keluar aplikasi.
pub fn render_quit_dialog(tabular: &mut Tabular, ctx: &egui::Context) {
    if !tabular.show_quit_confirm {
        return;
    }
    let open_transactions: Vec<String> = tabular
        .query_tabs
        .iter()
        .filter(|t| t.tx_active)
        .map(|t| t.title.clone())
        .collect();
    let unsaved = !tabular.restore_session
        && (0..tabular.query_tabs.len()).any(|i| tab_has_unsaved_changes(tabular, i));

    let mut quit = false;
    let mut cancel = false;
    crate::window_egui::style::render_modal_backdrop(ctx, "quit_confirm", true);
    egui::Window::new("Quit Tabular?")
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(400.0)
        .show(ctx, |ui| {
            crate::window_egui::style::render_modal_header(ui, "Quit Tabular?", &mut cancel);
            ui.add_space(8.0);

            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                if !open_transactions.is_empty() {
                    ui.label(
                        egui::RichText::new("Uncommitted transactions will be rolled back:")
                            .strong()
                            .color(crate::window_egui::style::theme_danger(ctx)),
                    );
                    for title in &open_transactions {
                        ui.label(egui::RichText::new(format!("• {}", title)).monospace());
                    }
                    ui.add_space(6.0);
                }
                if unsaved {
                    ui.label("Some tabs have unsaved changes and session restore is turned off, so they will be lost.");
                }
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let quit_btn = egui::Button::new(
                        egui::RichText::new("Quit Anyway").color(egui::Color32::WHITE).strong(),
                    )
                    .fill(crate::window_egui::style::theme_danger(ctx));
                    if ui.add(quit_btn).clicked() {
                        quit = true;
                    }
                });
            });
        });

    if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }

    if quit {
        tabular.show_quit_confirm = false;
        tabular.quit_confirmed = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    } else if cancel {
        tabular.show_quit_confirm = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(content: &str) -> SessionTab {
        SessionTab {
            title: "Untitled Query".into(),
            content: content.into(),
            file_path: None,
            connection_id: Some(4),
            database_name: Some("app".into()),
            is_pinned: false,
            is_modified: true,
        }
    }

    #[test]
    fn snapshot_json_roundtrip_keeps_drafts() {
        let snap = SessionSnapshot {
            version: SESSION_VERSION,
            active_index: 1,
            tabs: vec![
                draft("SELECT 1; -- draft"),
                SessionTab {
                    title: "report.sql".into(),
                    content: "SELECT * FROM orders".into(),
                    file_path: Some("/tmp/report.sql".into()),
                    connection_id: None,
                    database_name: None,
                    is_pinned: true,
                    is_modified: false,
                },
            ],
            window: Some(WindowGeometry {
                width: 1280.0,
                height: 800.0,
                maximized: false,
            }),
        };
        let json = serde_json::to_string(&snap).unwrap();
        let back: SessionSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tabs, snap.tabs);
        assert_eq!(back.window, snap.window);
        assert_eq!(fingerprint(&back), fingerprint(&snap));
    }

    #[test]
    fn fingerprint_changes_when_draft_changes() {
        let mut snap = SessionSnapshot {
            version: SESSION_VERSION,
            tabs: vec![draft("SELECT 1")],
            ..Default::default()
        };
        let before = fingerprint(&snap);
        snap.tabs[0].content.push('0');
        assert_ne!(before, fingerprint(&snap));
    }
}
