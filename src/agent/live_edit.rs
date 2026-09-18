//! Protokol live edit antara agent dan SQL Editor.
//!
//! Agent menulis SQL yang harus masuk ke editor sebagai fenced block dengan
//! info string khusus:
//!
//! ~~~text
//! ```sql tabular:tab=12 mode=replace
//! SELECT …
//! ```
//! ~~~
//!
//! `tab` adalah `QueryTab::id` yang diberikan Tabular di konteks prompt, `mode`
//! salah satu dari `replace` (default, ganti seluruh isi tab), `append`
//! (tambahkan di akhir), atau `selection` (ganti teks yang sedang dipilih).
//! Fence biasa (```` ```sql ````) tetap hanya tampil di chat.
//!
//! [`LiveEditParser`] bekerja per-delta streaming sehingga editor terisi
//! sambil model masih mengetik; penerapan ke tab dilakukan UI (lihat
//! `editor::render_ai_panel`), modul ini tidak menyentuh egui.

/// Cara isi blok diterapkan ke tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LiveEditMode {
    #[default]
    Replace,
    Append,
    Selection,
}

impl LiveEditMode {
    pub fn label(self) -> &'static str {
        match self {
            LiveEditMode::Replace => "replace",
            LiveEditMode::Append => "append",
            LiveEditMode::Selection => "selection",
        }
    }
}

/// Kejadian yang dihasilkan parser saat streaming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveEditEvent {
    Begin { tab_id: usize, mode: LiveEditMode },
    /// Isi blok sejauh ini (baris lengkap + baris parsial yang aman).
    Progress { tab_id: usize, body: String },
    End { tab_id: usize, mode: LiveEditMode, body: String },
}

/// Catatan satu edit yang sudah/bisa diterapkan, disimpan di pesan chat
/// untuk tombol Apply / Revert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEditRecord {
    pub tab_id: usize,
    pub tab_title: String,
    pub mode: LiveEditMode,
    /// Isi tab sebelum edit (untuk Revert).
    pub original: String,
    /// Isi tab setelah edit diterapkan penuh.
    pub applied_text: String,
    pub applied: bool,
    pub reverted: bool,
    /// Alasan edit tidak diterapkan otomatis (tab tidak ditemukan, dsb.).
    pub note: Option<String>,
}

#[derive(Debug)]
struct ActiveBlock {
    tab_id: usize,
    mode: LiveEditMode,
    /// Baris-baris lengkap (masing-masing diakhiri '\n').
    lines: String,
    last_body: String,
}

/// Parser streaming. Umpankan setiap `TextDelta` lewat [`feed`](Self::feed)
/// dan panggil [`finish`](Self::finish) saat giliran selesai.
#[derive(Debug, Default)]
pub struct LiveEditParser {
    partial: String,
    active: Option<ActiveBlock>,
}

/// Baca info string fence pembuka. `None` bila bukan fence live edit.
pub fn parse_fence_info(line: &str) -> Option<(usize, LiveEditMode)> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("```")?;
    let mut tab: Option<usize> = None;
    let mut mode = LiveEditMode::Replace;
    for tok in rest.split_whitespace() {
        if let Some(v) = tok.strip_prefix("tabular:tab=") {
            tab = v.trim_matches(|c| c == '"' || c == '\'').parse().ok();
        } else if let Some(v) = tok.strip_prefix("mode=") {
            mode = match v.trim_matches(|c| c == '"' || c == '\'') {
                "append" => LiveEditMode::Append,
                "selection" => LiveEditMode::Selection,
                _ => LiveEditMode::Replace,
            };
        }
    }
    tab.map(|t| (t, mode))
}

impl LiveEditParser {
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    pub fn feed(&mut self, delta: &str) -> Vec<LiveEditEvent> {
        let mut out = Vec::new();
        self.partial.push_str(delta);
        while let Some(pos) = self.partial.find('\n') {
            let line = self.partial[..pos].to_string();
            self.partial.drain(..=pos);
            self.handle_line(&line, &mut out);
        }
        if let Some(block) = self.active.as_mut() {
            // Baris parsial ikut ditampilkan kecuali bisa jadi awal fence penutup.
            let safe_partial = !block_partial_may_be_fence(&self.partial);
            let mut body = block.lines.clone();
            if safe_partial {
                body.push_str(&self.partial);
            }
            if body != block.last_body {
                block.last_body = body.clone();
                out.push(LiveEditEvent::Progress {
                    tab_id: block.tab_id,
                    body: trim_body(&body),
                });
            }
        }
        out
    }

    fn handle_line(&mut self, line: &str, out: &mut Vec<LiveEditEvent>) {
        match self.active.as_mut() {
            None => {
                if let Some((tab_id, mode)) = parse_fence_info(line) {
                    self.active = Some(ActiveBlock {
                        tab_id,
                        mode,
                        lines: String::new(),
                        last_body: String::new(),
                    });
                    out.push(LiveEditEvent::Begin { tab_id, mode });
                }
            }
            Some(block) => {
                if line.trim() == "```" {
                    let body = trim_body(&block.lines);
                    out.push(LiveEditEvent::End {
                        tab_id: block.tab_id,
                        mode: block.mode,
                        body,
                    });
                    self.active = None;
                } else {
                    block.lines.push_str(line);
                    block.lines.push('\n');
                }
            }
        }
    }

    /// Tutup blok yang masih terbuka (model berhenti tanpa fence penutup).
    pub fn finish(&mut self) -> Vec<LiveEditEvent> {
        let mut out = Vec::new();
        if !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.handle_line(&line, &mut out);
        }
        if let Some(block) = self.active.take() {
            out.push(LiveEditEvent::End {
                tab_id: block.tab_id,
                mode: block.mode,
                body: trim_body(&block.lines),
            });
        }
        out
    }
}

fn block_partial_may_be_fence(partial: &str) -> bool {
    let t = partial.trim_start();
    t.is_empty() && !partial.is_empty() || "```".starts_with(t) || t.starts_with('`')
}

fn trim_body(body: &str) -> String {
    body.trim_end_matches('\n').to_string()
}

/// Susun isi tab baru dari isi lama, mode, seleksi (byte offset), dan body.
pub fn compose(mode: LiveEditMode, original: &str, selection: (usize, usize), body: &str) -> String {
    match mode {
        LiveEditMode::Replace => body.to_string(),
        LiveEditMode::Append => {
            let base = original.trim_end_matches(['\n', ' ', '\t']);
            if base.is_empty() {
                body.to_string()
            } else {
                format!("{base}\n\n{body}")
            }
        }
        LiveEditMode::Selection => {
            let (s, e) = selection;
            let len = original.len();
            let s = clamp_char_boundary(original, s.min(len));
            let e = clamp_char_boundary(original, e.min(len)).max(s);
            format!("{}{}{}", &original[..s], body, &original[e..])
        }
    }
}

fn clamp_char_boundary(s: &str, mut idx: usize) -> usize {
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Instruksi protokol untuk system prompt.
pub const PROTOCOL_INSTRUCTIONS: &str = "\
## Writing into the SQL editor
The user's open editor tabs are listed in the context with their `tab_id`. \
To put SQL into a tab, emit a fenced code block whose info string names the tab:

```sql tabular:tab=<tab_id> mode=replace
SELECT ...
```

`mode` is one of `replace` (replace the whole tab, default), `append` (add at the end) \
or `selection` (replace the user's current selection in the active tab). Tabular applies \
such blocks to the editor live while you stream; keep only the final SQL inside the block \
(no prose, no `--` explanations unless they are meant to stay in the file). Use plain \
```sql blocks for examples or alternatives that must NOT be written into the editor. \
Only use tab_ids that appear in the context. When the user asks you to fix, rewrite, \
complete or optimize the query in a tab, write the result back into that tab.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_info_parsing() {
        assert_eq!(parse_fence_info("```sql tabular:tab=3"), Some((3, LiveEditMode::Replace)));
        assert_eq!(
            parse_fence_info("```sql tabular:tab=12 mode=append"),
            Some((12, LiveEditMode::Append))
        );
        assert_eq!(
            parse_fence_info("  ```tabular:tab=\"7\" mode=selection  "),
            Some((7, LiveEditMode::Selection))
        );
        assert_eq!(parse_fence_info("```sql"), None);
        assert_eq!(parse_fence_info("SELECT 1"), None);
        assert_eq!(parse_fence_info("```sql tabular:tab=x"), None);
    }

    #[test]
    fn streams_block_split_across_deltas() {
        let mut p = LiveEditParser::default();
        assert!(p.feed("Here is the fix:\n``").is_empty());
        let evs = p.feed("`sql tabular:tab=5 mode=replace\nSEL");
        assert_eq!(evs[0], LiveEditEvent::Begin { tab_id: 5, mode: LiveEditMode::Replace });
        assert_eq!(evs[1], LiveEditEvent::Progress { tab_id: 5, body: "SEL".into() });
        let evs = p.feed("ECT 1\nFROM t;\n`");
        // Partial "`" could be a closing fence: not shown yet.
        assert_eq!(
            evs.last(),
            Some(&LiveEditEvent::Progress { tab_id: 5, body: "SELECT 1\nFROM t;".into() })
        );
        let evs = p.feed("``\nDone.\n");
        assert_eq!(
            evs,
            vec![LiveEditEvent::End {
                tab_id: 5,
                mode: LiveEditMode::Replace,
                body: "SELECT 1\nFROM t;".into()
            }]
        );
        assert!(!p.is_active());
        assert!(p.finish().is_empty());
    }

    #[test]
    fn plain_sql_fence_is_ignored() {
        let mut p = LiveEditParser::default();
        let evs = p.feed("```sql\nSELECT 1;\n```\n");
        assert!(evs.is_empty());
    }

    #[test]
    fn finish_closes_unterminated_block() {
        let mut p = LiveEditParser::default();
        p.feed("```sql tabular:tab=1 mode=append\nSELECT 2");
        let evs = p.finish();
        assert_eq!(
            evs.last(),
            Some(&LiveEditEvent::End {
                tab_id: 1,
                mode: LiveEditMode::Append,
                body: "SELECT 2".into()
            })
        );
    }

    #[test]
    fn compose_modes() {
        assert_eq!(compose(LiveEditMode::Replace, "old", (0, 0), "new"), "new");
        assert_eq!(compose(LiveEditMode::Append, "", (0, 0), "new"), "new");
        assert_eq!(compose(LiveEditMode::Append, "old;\n\n", (0, 0), "new"), "old;\n\nnew");
        assert_eq!(compose(LiveEditMode::Selection, "abcdef", (2, 4), "XY"), "abXYef");
        assert_eq!(compose(LiveEditMode::Selection, "abc", (5, 9), "X"), "abcX");
        // Byte offset di tengah karakter multibyte digeser ke batas karakter.
        assert_eq!(compose(LiveEditMode::Selection, "héllo", (2, 3), "E"), "hEllo");
    }
}
