//! Riwayat sesi AI Assistant di header panel (K6): tombol History berisi
//! daftar sesi (buka, ganti nama, hapus, hapus semua) dan simpan otomatis
//! setelah tiap giliran selesai. Penyimpanan ada di `crate::ai_chat_history`.

use eframe::egui;

use super::Tabular;
use crate::ai_chat_history::{self, ChatSessionRecord, ChatSessionSummary, StoredMessage};

/// Jumlah sesi yang ditampilkan di menu.
const LIST_LIMIT: i64 = 200;

#[derive(Default)]
pub struct AiHistoryState {
    /// Sesi yang sedang terbuka di panel (sudah pernah disimpan).
    pub current_id: Option<i64>,
    created_at: i64,
    list: Vec<ChatSessionSummary>,
    list_loaded: bool,
    renaming: Option<(i64, String)>,
    confirm_clear: bool,
    error: Option<String>,
}

enum HistoryAction {
    Open(i64),
    Rename(i64, String),
    Delete(i64),
    ClearAll,
}

impl Tabular {
    fn ai_history_db(
        &self,
    ) -> Option<(
        std::sync::Arc<sqlx::SqlitePool>,
        std::sync::Arc<tokio::runtime::Runtime>,
    )> {
        Some((self.db_pool.clone()?, self.runtime.clone()?))
    }

    fn ai_history_refresh(&mut self) {
        let Some((pool, rt)) = self.ai_history_db() else {
            return;
        };
        match rt.block_on(ai_chat_history::list(&pool, LIST_LIMIT)) {
            Ok(list) => {
                self.ai_history.list = list;
                self.ai_history.error = None;
            }
            Err(e) => {
                log::warn!("[AI] failed to list chat sessions: {e}");
                self.ai_history.error = Some(format!("Could not load history: {e}"));
            }
        }
        self.ai_history.list_loaded = true;
    }

    /// Simpan percakapan yang sedang terbuka. Dipanggil setelah giliran selesai.
    pub(crate) fn ai_history_autosave(&mut self) {
        if self.ai_chat.is_empty() {
            return;
        }
        let Some((pool, rt)) = self.ai_history_db() else {
            return;
        };
        let now = ai_chat_history::now_secs();
        if self.ai_history.current_id.is_none() {
            self.ai_history.created_at = now;
        }
        let record = ChatSessionRecord {
            id: self.ai_history.current_id.unwrap_or(0),
            title: String::new(),
            target: self.effective_chat_target(),
            native_session: self.ai_session.clone(),
            messages: self.ai_chat.iter().map(StoredMessage::from_chat).collect(),
            created_at: self.ai_history.created_at,
            updated_at: now,
        };
        match rt.block_on(ai_chat_history::save(&pool, &record)) {
            Ok(id) => self.ai_history.current_id = Some(id),
            Err(e) => log::warn!("[AI] failed to save chat session: {e}"),
        }
        self.ai_history.list_loaded = false;
    }

    /// Percakapan baru: sesi berikutnya disimpan sebagai baris baru.
    pub(crate) fn ai_history_detach(&mut self) {
        self.ai_history.current_id = None;
    }

    fn ai_history_open(&mut self, id: i64) {
        let Some((pool, rt)) = self.ai_history_db() else {
            return;
        };
        let record = match rt.block_on(ai_chat_history::load(&pool, id)) {
            Ok(Some(r)) => r,
            Ok(None) => {
                self.ai_history.list_loaded = false;
                return;
            }
            Err(e) => {
                self.ai_history.error = Some(format!("Could not open the session: {e}"));
                return;
            }
        };
        // `ai_new_chat` menghentikan giliran berjalan dan mengosongkan state.
        crate::editor::ai_new_chat(self);
        self.ai_chat = record
            .messages
            .into_iter()
            .map(StoredMessage::into_chat)
            .collect();
        self.ai_session = record.native_session;
        if self.target_enabled(record.target) && self.ai_chat_target != record.target {
            self.ai_chat_target = record.target;
            self.save_ai_prefs();
        }
        self.ai_history.current_id = Some(record.id);
        self.ai_history.created_at = record.created_at;
        log::info!("[AI] reopened chat session {}", record.id);
    }

    fn ai_history_apply(&mut self, action: HistoryAction) {
        let Some((pool, rt)) = self.ai_history_db() else {
            return;
        };
        let result = match action {
            HistoryAction::Open(id) => {
                self.ai_history_open(id);
                Ok(())
            }
            HistoryAction::Rename(id, title) => {
                let title = title.trim().to_string();
                if title.is_empty() {
                    Ok(())
                } else {
                    rt.block_on(ai_chat_history::rename(&pool, id, &title))
                }
            }
            HistoryAction::Delete(id) => {
                if self.ai_history.current_id == Some(id) {
                    self.ai_history.current_id = None;
                }
                rt.block_on(ai_chat_history::delete(&pool, id))
            }
            HistoryAction::ClearAll => {
                self.ai_history.current_id = None;
                rt.block_on(ai_chat_history::clear_all(&pool))
            }
        };
        if let Err(e) = result {
            log::warn!("[AI] chat history action failed: {e}");
            self.ai_history.error = Some(e.to_string());
        }
        self.ai_history.list_loaded = false;
    }

    /// Tombol History di header panel AI.
    pub(crate) fn render_ai_history_button(&mut self, ui: &mut egui::Ui, busy: bool) {
        use super::style;
        use egui_icons::icons;

        let btn = ui
            .add_enabled_ui(!busy, |ui| {
                style::ai_icon_button(
                    ui,
                    icons::ICON_HISTORY.codepoint,
                    "Chat history (saved locally)",
                )
            })
            .inner;
        if btn.clicked() && !self.ai_history.list_loaded {
            self.ai_history_refresh();
        }
        let mut action: Option<HistoryAction> = None;
        egui::Popup::menu(&btn)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .width(300.0)
            .show(|ui| {
                let muted = style::theme_muted_text(ui.ctx());
                ui.label(egui::RichText::new("Recent chats").strong().size(12.0));
                if let Some(e) = &self.ai_history.error {
                    ui.label(
                        egui::RichText::new(e)
                            .size(11.0)
                            .color(style::theme_danger(ui.ctx())),
                    );
                }
                if self.ai_history.list.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No saved chats yet. Chats are saved after each answer.",
                        )
                        .size(11.0)
                        .color(muted),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(360.0)
                    .show(ui, |ui| {
                        for s in &self.ai_history.list {
                            let current = self.ai_history.current_id == Some(s.id);
                            if let Some((rid, text)) = self.ai_history.renaming.as_mut()
                                && *rid == s.id
                            {
                                let mut done = false;
                                ui.horizontal(|ui| {
                                    let r = ui
                                        .add(egui::TextEdit::singleline(text).desired_width(200.0));
                                    r.request_focus();
                                    if ui.small_button("Save").clicked()
                                        || (r.lost_focus()
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                    {
                                        action = Some(HistoryAction::Rename(s.id, text.clone()));
                                        done = true;
                                    }
                                    if ui.small_button("Cancel").clicked() {
                                        done = true;
                                    }
                                });
                                if done {
                                    self.ai_history.renaming = None;
                                }
                                continue;
                            }
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                let title = if current {
                                    format!("● {}", s.title)
                                } else {
                                    s.title.clone()
                                };
                                let open = ui
                                    .add(
                                        egui::Button::new(egui::RichText::new(title).size(11.5))
                                            .frame_when_inactive(false)
                                            .truncate(),
                                    )
                                    .on_hover_text(format!(
                                        "{} · {} messages · {}",
                                        backend_label(s.target),
                                        s.message_count,
                                        format_time(s.updated_at)
                                    ));
                                if open.clicked() {
                                    action = Some(HistoryAction::Open(s.id));
                                    ui.close();
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if style::ai_icon_button(
                                            ui,
                                            icons::ICON_DELETE.codepoint,
                                            "Delete this chat",
                                        )
                                        .clicked()
                                        {
                                            action = Some(HistoryAction::Delete(s.id));
                                        }
                                        if style::ai_icon_button(
                                            ui,
                                            icons::ICON_EDIT.codepoint,
                                            "Rename",
                                        )
                                        .clicked()
                                        {
                                            self.ai_history.renaming =
                                                Some((s.id, s.title.clone()));
                                        }
                                    },
                                );
                            });
                        }
                    });
                if !self.ai_history.list.is_empty() {
                    ui.separator();
                    if self.ai_history.confirm_clear {
                        ui.horizontal(|ui| {
                            if ui
                                .button(
                                    egui::RichText::new("Delete all saved chats")
                                        .size(11.0)
                                        .color(style::theme_danger(ui.ctx())),
                                )
                                .clicked()
                            {
                                action = Some(HistoryAction::ClearAll);
                                self.ai_history.confirm_clear = false;
                            }
                            if ui.small_button("Cancel").clicked() {
                                self.ai_history.confirm_clear = false;
                            }
                        });
                    } else if ui
                        .small_button("Clear all")
                        .on_hover_text("Delete every saved AI chat on this machine")
                        .clicked()
                    {
                        self.ai_history.confirm_clear = true;
                    }
                }
                ui.label(
                    egui::RichText::new(format!(
                        "Stored locally in connections.db (latest {} chats).",
                        ai_chat_history::MAX_SESSIONS
                    ))
                    .size(10.0)
                    .color(muted),
                );
            });
        if let Some(a) = action {
            self.ai_history_apply(a);
            if !self.ai_history.list_loaded {
                self.ai_history_refresh();
            }
        }
    }
}

fn backend_label(target: crate::config::ChatTarget) -> String {
    match target {
        crate::config::ChatTarget::Api => "HTTP API".to_string(),
        crate::config::ChatTarget::Cli(k) => k.display_name().to_string(),
    }
}

fn format_time(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}
