//! Settings > AI Assistant > "Memory (Engram)": hubungkan AI assistant ke
//! Engram dan pilih kejadian apa yang dicatat. State UI disimpan di memori
//! egui (bukan field `Tabular`); pemeriksaan koneksi berjalan di thread latar.

use std::sync::{Arc, Mutex};

use eframe::egui;

use super::Tabular;
use super::preferences::{Tone, divider, hint, row, section, status, toggle_row};
use super::style;
use crate::engram::{self, EngramSettings, EngramStatus};

type Slot = Arc<Mutex<Option<Result<EngramStatus, String>>>>;

#[derive(Clone, Default)]
struct State {
    settings: Option<EngramSettings>,
    status: Option<Result<EngramStatus, String>>,
    job: Option<Slot>,
    token_input: String,
    /// Draf URL; disimpan saat fokus lepas.
    url_input: Option<String>,
}

fn state_id() -> egui::Id {
    egui::Id::new("engram_settings_state")
}

fn take(slot: &Slot) -> Option<Result<EngramStatus, String>> {
    slot.lock().ok().and_then(|mut g| g.take())
}

fn start_check(st: &mut State, s: &EngramSettings) {
    let slot: Slot = Arc::new(Mutex::new(None));
    let out = slot.clone();
    let s = s.clone();
    let spawned = std::thread::Builder::new()
        .name("engram-status".into())
        .spawn(move || {
            let r = engram::check_status(&s);
            if let Ok(mut g) = out.lock() {
                *g = Some(r);
            }
        });
    match spawned {
        Ok(_) => st.job = Some(slot),
        Err(e) => st.status = Some(Err(format!("Cannot start the check: {e}"))),
    }
}

impl Tabular {
    pub(crate) fn render_engram_settings(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let mut st: State = ctx.data_mut(|d| d.get_temp(state_id()).unwrap_or_default());
        let mut s = st.settings.take().unwrap_or_else(EngramSettings::load);
        if let Some(job) = &st.job {
            match take(job) {
                Some(r) => {
                    st.status = Some(r);
                    st.job = None;
                }
                None => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            }
        }
        if s.enabled && st.status.is_none() && st.job.is_none() {
            start_check(&mut st, &s);
        }

        let mut changed = false;
        section(ui, "Memory (Engram)", |ui| {
            hint(
                ui,
                "Connect Tabular to Engram, your local personal memory. The AI assistant recalls relevant \
                 memories for each request, and important work in Tabular is recorded in Engram.",
            );
            ui.add_space(4.0);
            if toggle_row(ui, &mut s.enabled, "Use Engram", None) {
                changed = true;
                st.status = None;
            }
            if !s.enabled {
                return;
            }

            ui.horizontal_wrapped(|ui| {
                match (&st.job, &st.status) {
                    (Some(_), _) => {
                        ui.spinner();
                        hint(ui, "Checking Engram…");
                    }
                    (None, Some(Ok(info))) => {
                        let subject = info
                            .subjects
                            .iter()
                            .find(|x| x.id == info.subject)
                            .map_or(info.subject.clone(), |x| x.name.clone());
                        status(
                            ui,
                            Tone::Success,
                            format!(
                                "Connected to Engram {} at {} (memory of {subject}; token from {})",
                                info.version, info.base, info.token_source
                            ),
                        );
                    }
                    (None, Some(Err(e))) => status(ui, Tone::Danger, e.clone()),
                    (None, None) => hint(ui, "Not checked yet"),
                }
                if st.job.is_none() && ui.add(style::btn_secondary("Check connection")).clicked() {
                    start_check(&mut st, &s);
                }
            });
            let pending = match &st.status {
                Some(Ok(info)) => info.pending,
                _ => 0,
            };
            if pending > 0 {
                ui.horizontal_wrapped(|ui| {
                    status(
                        ui,
                        Tone::Warning,
                        format!("{pending} recorded event(s) waiting to be sent"),
                    );
                    if ui.add(style::btn_secondary("Send now")).clicked() {
                        engram::flush_now();
                        st.status = None;
                    }
                });
            }
            divider(ui);

            let subjects = match &st.status {
                Some(Ok(info)) => info.subjects.clone(),
                _ => Vec::new(),
            };
            row(
                ui,
                "Memory of",
                Some("The Engram subject (person) whose memory Tabular reads and writes."),
                |ui| {
                    let current = if s.subject.is_empty() {
                        "Engram default".to_string()
                    } else {
                        subjects
                            .iter()
                            .find(|x| x.id == s.subject)
                            .map_or(s.subject.clone(), |x| x.name.clone())
                    };
                    egui::ComboBox::from_id_salt("engram_subject")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            changed |= ui
                                .selectable_value(&mut s.subject, String::new(), "Engram default")
                                .changed();
                            for x in &subjects {
                                changed |= ui
                                    .selectable_value(&mut s.subject, x.id.clone(), &x.name)
                                    .changed();
                            }
                        });
                },
            );
            row(
                ui,
                "Engram URL",
                Some("Leave empty to use the address the Engram app announces."),
                |ui| {
                    let draft = st.url_input.get_or_insert_with(|| s.url.clone());
                    let resp = ui.add(
                        egui::TextEdit::singleline(draft)
                            .hint_text(engram::DEFAULT_URL)
                            .desired_width(240.0),
                    );
                    if resp.lost_focus() && draft.trim() != s.url {
                        s.url = draft.trim().to_string();
                        changed = true;
                        st.status = None;
                    }
                },
            );
            row(
                ui,
                "Token",
                Some(
                    "Found automatically from the Engram app on this computer. Enter one only when \
                     Engram runs elsewhere; it is kept in the system keychain.",
                ),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut st.token_input)
                            .password(true)
                            .hint_text("automatic")
                            .desired_width(160.0),
                    );
                    if !st.token_input.trim().is_empty()
                        && ui.add(style::btn_secondary("Save")).clicked()
                    {
                        if crate::secrets::set_secret(engram::TOKEN_SECRET, st.token_input.trim()) {
                            st.token_input.clear();
                            st.status = None;
                            self.toasts.success("Engram token saved");
                        } else {
                            self.toasts
                                .error("Could not save the Engram token in the keychain");
                        }
                    }
                    if ui.add(style::btn_secondary("Use automatic")).clicked() {
                        crate::secrets::delete_secret(engram::TOKEN_SECRET);
                        st.token_input.clear();
                        st.status = None;
                    }
                },
            );
            divider(ui);

            changed |= toggle_row(
                ui,
                &mut s.recall_in_chat,
                "Use memories in AI chat",
                Some(
                    "Memories from Engram that relate to your question are added to each AI request and \
                     sent to the AI provider together with it.",
                ),
            );
            changed |= toggle_row(
                ui,
                &mut s.record_chat,
                "Record AI conversations",
                Some("Each question you ask the AI assistant and its answer are stored in Engram."),
            );
            changed |= toggle_row(
                ui,
                &mut s.record_queries,
                "Record schema and data changes",
                Some(
                    "Successful CREATE, ALTER and DROP statements are stored with their SQL. INSERT, \
                     UPDATE and DELETE are stored only as statement type, table and row count, never the \
                     values. Passwords and tokens are masked.",
                ),
            );
            changed |= toggle_row(
                ui,
                &mut s.record_activity,
                "Record other activity",
                Some(
                    "Connections, projects, repository setup, integration test and Data Health results, \
                     git commits, pushes and merges, transfers, exports, and memories the AI saves.",
                ),
            );
            hint(
                ui,
                "Events are queued on this computer and sent to Engram in batches; when Engram is closed \
                 they wait and are sent later.",
            );
        });

        if changed {
            if let Err(e) = s.save() {
                self.toasts.error(e);
                // Tampilkan nilai di disk, bukan perubahan yang gagal disimpan.
                s = EngramSettings::load();
            }
        }
        st.settings = Some(s);
        ctx.data_mut(|d| d.insert_temp(state_id(), st));
    }
}
