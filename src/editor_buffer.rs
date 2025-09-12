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

    /// Apply a single replace (delete old_range and insert replacement) on both rope and cached text.
    /// old_range is byte indices in current text before change.
    pub fn apply_single_replace(&mut self, old_range: std::ops::Range<usize>, replacement: &str) {
        // Safety clamp
        let start = old_range.start.min(self.text.len());
        let end = old_range.end.min(self.text.len()).max(start);
        // Update cached text
        self.text.replace_range(start..end, replacement);
        // Apply to rope (naive rebuild of slice via edit API). Buffer currently lacks public granular API in this wrapper, so rebuild for now.
        // In future we can call self.buffer.edit(&[(Interval::new(start as i64, end as i64), replacement.into())]);
        self.buffer = Buffer::new(&self.text);
        self.last_revision = 0; // reset revision tracking for now
        self.dirty_to_rope = false;
        self.dirty_to_string = false;
    }

    /// Heuristic diff between previous and new full text; if it matches a single contiguous replace,
    /// apply via apply_single_replace and return true. Else return false (caller can fallback to set_text).
    pub fn try_single_span_update(&mut self, previous: &str, new_full: &str) -> bool {
        if previous == new_full { return true; }
        // Quick bounds: find common prefix
        let mut prefix = 0usize;
        let prev_bytes = previous.as_bytes();
        let new_bytes = new_full.as_bytes();
        let min_len = prev_bytes.len().min(new_bytes.len());
        while prefix < min_len && prev_bytes[prefix] == new_bytes[prefix] { prefix += 1; }
        // Find common suffix (excluding prefix region)
        let mut suffix = 0usize;
        while suffix < (prev_bytes.len() - prefix)
            && suffix < (new_bytes.len() - prefix)
            && prev_bytes[prev_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
        { suffix += 1; }
        // Compute differing spans
        let prev_mid_start = prefix;
        let prev_mid_end = prev_bytes.len() - suffix;
        let new_mid_start = prefix;
        let new_mid_end = new_bytes.len() - suffix;
        // If no change region -> done
        if prev_mid_start == prev_mid_end && new_mid_start == new_mid_end { return true; }
        // Extract replacement slice
        if new_mid_start > new_mid_end || prev_mid_start > prev_mid_end { return false; }
        if let Some(replacement) = new_full.get(new_mid_start..new_mid_end) {
            self.apply_single_replace(prev_mid_start..prev_mid_end, replacement);
            return true;
        }
        false
    }
}
