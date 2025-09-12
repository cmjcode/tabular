use lapce_core::buffer::Buffer;

/// Lightweight wrapper integrating lapce_core::Buffer with legacy String access.
/// Goal: progressively replace direct String mutations with rope-based edits.
pub struct EditorBuffer {
    buffer: Buffer,
    /// Cached full text for egui TextEdit binding (temporary until full custom editor)
    pub text: String,
    /// Dirty flag: when true, rope has diverged from cached text (pending sync to String)
    dirty_to_string: bool,
    /// Dirty flag: when true, cached text has diverged from rope (pending apply to rope)
    dirty_to_rope: bool,
    /// Last known revision of the underlying buffer (for incremental features later)
    pub last_revision: u64,
}

impl Default for EditorBuffer {
    fn default() -> Self { Self::new("") }
}

impl EditorBuffer {
    pub fn new(initial: &str) -> Self {
        let buffer = Buffer::new(initial);
        Self {
            buffer,
            text: initial.to_string(),
            dirty_to_string: false,
            dirty_to_rope: false,
            last_revision: 0,
        }
    }

    /// Mark that the UI-updated text (egui TextEdit) should overwrite rope on next sync_to_rope.
    pub fn mark_text_modified(&mut self) {
        self.dirty_to_rope = true;
    }

    /// Get immutable access to current rope content as &str (alloc-free).
    pub fn as_str(&self) -> &str { &self.text }

    /// Replace whole content (fast path). Avoid for large texts; use edit ranges later.
    pub fn set_text(&mut self, new_text: String) {
        self.text = new_text.clone();
    self.buffer = Buffer::new(&new_text); // temporary full rebuild (no granular diff yet)
    self.last_revision = 0;
        self.dirty_to_string = false;
        self.dirty_to_rope = false;
    }

    /// Sync UI -> rope when user edited via egui bound String.
    pub fn sync_to_rope(&mut self) {
        if self.dirty_to_rope {
            // Full rebuild for now; will be replaced with diff-based incremental edits.
            self.buffer = Buffer::new(&self.text);
            self.last_revision = 0;
            self.dirty_to_rope = false;
        }
    }

    /// Sync rope -> UI text (when edits performed programmatically on rope).
    pub fn sync_to_string(&mut self) {
        if self.dirty_to_string {
            // Currently buffer is always rebuilt from text; no reverse sync needed
            self.text = self.text.clone();
            self.last_revision = 0;
            self.dirty_to_string = false;
        }
    }

    /// Insert text at byte offset (rope edit).
    /// Placeholder APIs for future granular editing (currently full text binding via egui String).
    pub fn len(&self) -> usize { self.text.len() }
    pub fn is_empty(&self) -> bool { self.text.is_empty() }
}
