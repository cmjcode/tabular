//! Registry shortcut keyboard terpusat.
//!
//! Semua shortcut tingkat aplikasi didefinisikan di satu tempat (`ACTIONS`),
//! sehingga cheatsheet, Quick Open, dan handler keyboard selalu memakai binding
//! yang sama. User dapat mengubah binding; override disimpan di
//! `<data_dir>/keybindings.json` (hanya aksi yang berbeda dari default).

use eframe::egui;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    RunQuery,
    ExplainQuery,
    FormatSql,
    ToggleComment,
    FindReplace,
    NewTab,
    CloseTab,
    SaveTab,
    QuickOpen,
    Refresh,
    GoToDefinition,
    RenameSymbol,
    ToggleAiPanel,
    ToggleTransactionMode,
    OpenSettings,
    ShowShortcuts,
    Quit,
}

/// Deskripsi statis sebuah aksi.
pub struct ActionSpec {
    pub action: Action,
    /// Key stabil untuk file konfigurasi; jangan diubah setelah rilis.
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    /// Binding default dalam format konfigurasi, mis. "Cmd+Shift+F".
    pub defaults: &'static [&'static str],
}

pub const ACTIONS: &[ActionSpec] = &[
    ActionSpec { action: Action::RunQuery, id: "run_query", label: "Run query / selection", category: "Query", defaults: &["Cmd+Enter"] },
    ActionSpec { action: Action::ExplainQuery, id: "explain_query", label: "Explain query", category: "Query", defaults: &["Cmd+Shift+E"] },
    ActionSpec { action: Action::ToggleTransactionMode, id: "toggle_transaction_mode", label: "Toggle manual-commit mode", category: "Query", defaults: &["Cmd+Shift+T"] },
    ActionSpec { action: Action::FormatSql, id: "format_sql", label: "Format SQL", category: "Editor", defaults: &["Cmd+Shift+F"] },
    ActionSpec { action: Action::ToggleComment, id: "toggle_comment", label: "Toggle line comment", category: "Editor", defaults: &["Cmd+Slash"] },
    ActionSpec { action: Action::FindReplace, id: "find_replace", label: "Find & replace", category: "Editor", defaults: &["Cmd+F"] },
    ActionSpec { action: Action::GoToDefinition, id: "go_to_definition", label: "Go to definition", category: "Editor", defaults: &["F12"] },
    ActionSpec { action: Action::RenameSymbol, id: "rename_symbol", label: "Rename symbol", category: "Editor", defaults: &["F2"] },
    ActionSpec { action: Action::ToggleAiPanel, id: "toggle_ai_panel", label: "Toggle AI assistant", category: "Editor", defaults: &["Cmd+Shift+A"] },
    ActionSpec { action: Action::NewTab, id: "new_tab", label: "New query tab", category: "Tabs", defaults: &["Cmd+T"] },
    ActionSpec { action: Action::CloseTab, id: "close_tab", label: "Close tab", category: "Tabs", defaults: &["Cmd+W"] },
    ActionSpec { action: Action::SaveTab, id: "save_tab", label: "Save tab / table changes", category: "Tabs", defaults: &["Cmd+S"] },
    ActionSpec { action: Action::QuickOpen, id: "quick_open", label: "Quick open / command palette", category: "Navigation", defaults: &["Cmd+P", "Cmd+K"] },
    ActionSpec { action: Action::Refresh, id: "refresh", label: "Refresh data / structure", category: "Navigation", defaults: &["Cmd+R"] },
    ActionSpec { action: Action::OpenSettings, id: "open_settings", label: "Open settings", category: "Application", defaults: &["Cmd+Comma"] },
    ActionSpec { action: Action::ShowShortcuts, id: "show_shortcuts", label: "Keyboard shortcuts", category: "Application", defaults: &["Cmd+Shift+Slash", "F1"] },
    ActionSpec { action: Action::Quit, id: "quit", label: "Quit Tabular", category: "Application", defaults: &["Cmd+Q"] },
];

pub fn spec(action: Action) -> &'static ActionSpec {
    ACTIONS
        .iter()
        .find(|s| s.action == action)
        .expect("setiap Action wajib terdaftar di ACTIONS")
}

/// Kombinasi tombol. `command` berarti ⌘ di macOS dan Ctrl di platform lain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: egui::Key,
}

impl Shortcut {
    /// Parse format konfigurasi, mis. "Cmd+Shift+F", "Ctrl+Enter", "F12".
    pub fn parse(text: &str) -> Option<Self> {
        let mut shortcut = Shortcut {
            command: false,
            shift: false,
            alt: false,
            key: egui::Key::Escape,
        };
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key_part, modifiers) = parts.split_last()?;
        for modifier in modifiers {
            match modifier.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "ctrl" | "control" | "⌘" => shortcut.command = true,
                "shift" | "⇧" => shortcut.shift = true,
                "alt" | "option" | "opt" | "⌥" => shortcut.alt = true,
                _ => return None,
            }
        }
        shortcut.key = egui::Key::from_name(key_part)?;
        Some(shortcut)
    }

    /// Format konfigurasi (dipakai saat menyimpan override).
    pub fn to_config(self) -> String {
        let mut parts = Vec::new();
        if self.command {
            parts.push("Cmd");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.alt {
            parts.push("Alt");
        }
        parts.push(self.key.name());
        parts.join("+")
    }

    /// Teks yang ditampilkan ke user sesuai konvensi platform.
    pub fn display(self) -> String {
        let key = self.key.symbol_or_name();
        if cfg!(any(target_os = "macos", target_os = "ios")) {
            format!(
                "{}{}{}{}",
                if self.alt { "⌥" } else { "" },
                if self.shift { "⇧" } else { "" },
                if self.command { "⌘" } else { "" },
                key
            )
        } else {
            let mut parts = Vec::new();
            if self.command {
                parts.push("Ctrl");
            }
            if self.shift {
                parts.push("Shift");
            }
            if self.alt {
                parts.push("Alt");
            }
            parts.push(key);
            parts.join("+")
        }
    }

    fn matches(self, modifiers: &egui::Modifiers, key: egui::Key) -> bool {
        key == self.key
            && modifiers.command == self.command
            && modifiers.shift == self.shift
            && modifiers.alt == self.alt
    }

    /// Shortcut dari event keyboard saat merekam binding baru. Tombol tanpa
    /// ⌘/Ctrl atau Alt hanya diterima untuk tombol fungsi (F1–F20) agar huruf
    /// biasa tidak menjadi shortcut yang mengganggu pengetikan.
    fn from_event(modifiers: &egui::Modifiers, key: egui::Key) -> Option<Self> {
        let name = key.name();
        let is_function_key =
            name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok();
        if !modifiers.command && !modifiers.alt && !is_function_key {
            return None;
        }
        Some(Shortcut {
            command: modifiers.command,
            shift: modifiers.shift,
            alt: modifiers.alt,
            key,
        })
    }
}

pub struct Keymap {
    bindings: HashMap<Action, Vec<Shortcut>>,
    /// Aksi yang sedang direkam binding barunya; selama ini shortcut tidak dieksekusi.
    pub recording: Option<Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        let bindings = ACTIONS
            .iter()
            .map(|s| (s.action, default_shortcuts(s)))
            .collect();
        Self {
            bindings,
            recording: None,
        }
    }
}

fn default_shortcuts(spec: &ActionSpec) -> Vec<Shortcut> {
    spec.defaults.iter().filter_map(|d| Shortcut::parse(d)).collect()
}

fn keybindings_path() -> std::path::PathBuf {
    crate::config::get_data_dir().join("keybindings.json")
}

impl Keymap {
    /// Muat binding default lalu terapkan override dari `keybindings.json`.
    pub fn load() -> Self {
        let mut keymap = Self::default();
        let Ok(content) = std::fs::read_to_string(keybindings_path()) else {
            return keymap;
        };
        match serde_json::from_str::<HashMap<String, Vec<String>>>(&content) {
            Ok(overrides) => keymap.apply_overrides(&overrides),
            Err(e) => log::warn!("Ignoring invalid keybindings.json: {}", e),
        }
        keymap
    }

    fn apply_overrides(&mut self, overrides: &HashMap<String, Vec<String>>) {
        for (id, shortcuts) in overrides {
            let Some(spec) = ACTIONS.iter().find(|s| s.id == id) else {
                log::warn!("Unknown action '{}' in keybindings.json", id);
                continue;
            };
            let parsed: Vec<Shortcut> = shortcuts
                .iter()
                .filter_map(|text| {
                    let parsed = Shortcut::parse(text);
                    if parsed.is_none() {
                        log::warn!("Invalid shortcut '{}' for '{}' in keybindings.json", text, id);
                    }
                    parsed
                })
                .collect();
            self.bindings.insert(spec.action, parsed);
        }
    }

    /// Simpan hanya binding yang berbeda dari default.
    pub fn save(&self) -> Result<(), String> {
        let overrides: HashMap<&str, Vec<String>> = ACTIONS
            .iter()
            .filter(|spec| self.shortcuts(spec.action) != default_shortcuts(spec).as_slice())
            .map(|spec| {
                (
                    spec.id,
                    self.shortcuts(spec.action).iter().map(|s| s.to_config()).collect(),
                )
            })
            .collect();
        let json = serde_json::to_string_pretty(&overrides).map_err(|e| e.to_string())?;
        crate::directory::write_file_atomically(&keybindings_path(), json.as_bytes())
            .map_err(|e| e.to_string())
    }

    pub fn shortcuts(&self, action: Action) -> &[Shortcut] {
        self.bindings.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Label shortcut utama untuk ditampilkan (kosong jika tidak ada binding).
    pub fn label(&self, action: Action) -> String {
        self.shortcuts(action)
            .first()
            .map(|s| s.display())
            .unwrap_or_default()
    }

    pub fn set(&mut self, action: Action, shortcuts: Vec<Shortcut>) {
        self.bindings.insert(action, shortcuts);
    }

    pub fn reset(&mut self, action: Action) {
        self.bindings.insert(action, default_shortcuts(spec(action)));
    }

    /// Aksi lain yang memakai shortcut yang sama.
    pub fn conflicts_with(&self, action: Action, shortcut: Shortcut) -> Vec<Action> {
        ACTIONS
            .iter()
            .map(|s| s.action)
            .filter(|a| *a != action && self.shortcuts(*a).contains(&shortcut))
            .collect()
    }
}

/// True (dan event dikonsumsi) jika salah satu binding `action` ditekan di
/// frame ini. Event dikonsumsi agar widget lain (mis. TextEdit) atau handler
/// kedua tidak ikut memprosesnya.
pub fn consume(ctx: &egui::Context, keymap: &Keymap, action: Action) -> bool {
    if keymap.recording.is_some() {
        return false;
    }
    let shortcuts = keymap.shortcuts(action);
    if shortcuts.is_empty() {
        return false;
    }
    ctx.input_mut(|input| {
        let mut hit = false;
        input.events.retain(|event| match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if !hit && shortcuts.iter().any(|s| s.matches(modifiers, *key)) => {
                hit = true;
                false
            }
            _ => true,
        });
        hit
    })
}

/// Jendela daftar shortcut yang bisa dicari dan diubah.
pub fn render_shortcuts_window(tabular: &mut crate::window_egui::Tabular, ctx: &egui::Context) {
    if !tabular.show_shortcuts_window {
        tabular.keymap.recording = None;
        return;
    }

    // Rekam binding baru dari event keyboard frame ini.
    if let Some(action) = tabular.keymap.recording {
        let captured = ctx.input_mut(|input| {
            let mut captured = None;
            input.events.retain(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if captured.is_none() => {
                    captured = Some((*key, *modifiers));
                    false
                }
                _ => true,
            });
            captured
        });
        if let Some((key, modifiers)) = captured {
            if key == egui::Key::Escape && !modifiers.any() {
                tabular.keymap.recording = None;
            } else if let Some(shortcut) = Shortcut::from_event(&modifiers, key) {
                let conflicts = tabular.keymap.conflicts_with(action, shortcut);
                tabular.keymap.set(action, vec![shortcut]);
                tabular.keymap.recording = None;
                if let Err(e) = tabular.keymap.save() {
                    tabular.toasts.error(format!("Could not save keybindings: {}", e));
                } else if !conflicts.is_empty() {
                    let names: Vec<&str> = conflicts.iter().map(|a| spec(*a).label).collect();
                    tabular.toasts.warning(format!(
                        "{} is also bound to: {}",
                        shortcut.display(),
                        names.join(", ")
                    ));
                }
            }
        }
    }

    let mut open = true;
    egui::Window::new("Keyboard Shortcuts")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .default_height(480.0)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Search:");
                ui.add(
                    egui::TextEdit::singleline(&mut tabular.shortcuts_filter)
                        .hint_text("action or key")
                        .desired_width(220.0),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Click a shortcut to record a new one (Esc cancels). Saved to keybindings.json in the data directory.",
                )
                .small()
                .weak(),
            );
            ui.separator();

            let filter = tabular.shortcuts_filter.to_lowercase();
            let mut record = None;
            let mut reset = None;
            let mut clear = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let mut current_category = "";
                for spec in ACTIONS {
                    let shortcuts = tabular.keymap.shortcuts(spec.action);
                    let shortcut_text = shortcuts
                        .iter()
                        .map(|s| s.display())
                        .collect::<Vec<_>>()
                        .join("  /  ");
                    if !filter.is_empty()
                        && !spec.label.to_lowercase().contains(&filter)
                        && !shortcut_text.to_lowercase().contains(&filter)
                    {
                        continue;
                    }
                    if spec.category != current_category {
                        current_category = spec.category;
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(spec.category).strong());
                    }
                    ui.horizontal(|ui| {
                        ui.add_sized([230.0, 20.0], egui::Label::new(spec.label));
                        let is_recording = tabular.keymap.recording == Some(spec.action);
                        let button_text = if is_recording {
                            "Press keys…".to_string()
                        } else if shortcut_text.is_empty() {
                            "Unassigned".to_string()
                        } else {
                            shortcut_text.clone()
                        };
                        let has_conflict = shortcuts
                            .iter()
                            .any(|s| !tabular.keymap.conflicts_with(spec.action, *s).is_empty());
                        let mut text = egui::RichText::new(button_text).monospace();
                        if has_conflict {
                            text = text.color(crate::window_egui::style::theme_danger(ui.ctx()));
                        }
                        let response = ui.add_sized([160.0, 20.0], egui::Button::new(text));
                        let response = if has_conflict {
                            response.on_hover_text("This shortcut is also bound to another action")
                        } else {
                            response
                        };
                        if response.clicked() {
                            record = Some(spec.action);
                        }
                        if shortcuts != default_shortcuts(spec).as_slice()
                            && ui.small_button("Reset").clicked()
                        {
                            reset = Some(spec.action);
                        }
                        if !shortcuts.is_empty()
                            && ui.small_button("✕").on_hover_text("Remove shortcut").clicked()
                        {
                            clear = Some(spec.action);
                        }
                    });
                }
            });

            if let Some(action) = record {
                tabular.keymap.recording = Some(action);
            }
            let changed = if let Some(action) = reset {
                tabular.keymap.reset(action);
                true
            } else if let Some(action) = clear {
                tabular.keymap.set(action, Vec::new());
                true
            } else {
                false
            };
            if changed && let Err(e) = tabular.keymap.save() {
                tabular.toasts.error(format!("Could not save keybindings: {}", e));
            }
        });
    if !open {
        tabular.show_shortcuts_window = false;
        tabular.keymap.recording = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_parses() {
        for spec in ACTIONS {
            assert_eq!(
                default_shortcuts(spec).len(),
                spec.defaults.len(),
                "default shortcut for {} does not parse",
                spec.id
            );
        }
    }

    #[test]
    fn default_bindings_have_no_conflicts() {
        let keymap = Keymap::default();
        for spec in ACTIONS {
            for shortcut in keymap.shortcuts(spec.action) {
                assert!(
                    keymap.conflicts_with(spec.action, *shortcut).is_empty(),
                    "{} conflicts for {}",
                    shortcut.to_config(),
                    spec.id
                );
            }
        }
    }

    #[test]
    fn config_roundtrip_and_overrides() {
        let shortcut = Shortcut::parse("Ctrl+Shift+Enter").unwrap();
        assert!(shortcut.command && shortcut.shift && !shortcut.alt);
        assert_eq!(shortcut.key, egui::Key::Enter);
        assert_eq!(Shortcut::parse(&shortcut.to_config()), Some(shortcut));
        assert_eq!(Shortcut::parse("Hyper+X"), None);

        let mut keymap = Keymap::default();
        let overrides = HashMap::from([
            ("run_query".to_string(), vec!["Cmd+Shift+Enter".to_string()]),
            ("unknown_action".to_string(), vec!["Cmd+J".to_string()]),
        ]);
        keymap.apply_overrides(&overrides);
        assert_eq!(
            keymap.shortcuts(Action::RunQuery),
            &[Shortcut::parse("Cmd+Shift+Enter").unwrap()]
        );
        assert_eq!(
            keymap.shortcuts(Action::CloseTab),
            &[Shortcut::parse("Cmd+W").unwrap()]
        );
    }

    #[test]
    fn plain_letters_cannot_be_recorded() {
        let none = egui::Modifiers::NONE;
        assert_eq!(Shortcut::from_event(&none, egui::Key::A), None);
        assert!(Shortcut::from_event(&none, egui::Key::F5).is_some());
        assert!(Shortcut::from_event(&egui::Modifiers::COMMAND, egui::Key::J).is_some());
    }
}
