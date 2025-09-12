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
    /// Undo stack (Vec of Edit record). Most recent at end.
    undo_stack: Vec<EditRecord>,
    /// Redo stack.
    redo_stack: Vec<EditRecord>,
    /// Cached line start offsets (byte indices) for fast line/col translation.
    line_starts: Vec<usize>,
    /// Monotonic revision counter we control (separate from any internal lapce buffer revs)
    pub revision: u64,
    /// Per-line version numbers for fine-grained cache invalidation (same length as logical lines).
    line_versions: Vec<u64>,
}

/// A simple reversible edit representation (single replace operation)
#[derive(Clone, Debug)]
struct EditRecord {
    range: std::ops::Range<usize>, // replaced old text range in the PREVIOUS document
    inserted: String,              // new inserted text
    removed: String,               // old removed text (for undo)
}

impl Default for EditorBuffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl EditorBuffer {
    pub fn new(initial: &str) -> Self {
        let buffer = Buffer::new(initial);
        let line_starts = Self::compute_line_starts(initial);
        Self {
            buffer,
            text: initial.to_string(),
            dirty_to_string: false,
            dirty_to_rope: false,
            last_revision: 0,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            line_starts,
            revision: 0,
            line_versions: vec![0],
        }
    }

    /// Full recompute of line starts (fallback path; later we can do incremental adjustments).
    fn recompute_line_starts(&mut self) {
        self.line_starts = Self::compute_line_starts(&self.text);
        let logical_line_count = self.line_count();
        if self.line_versions.len() < logical_line_count {
            self.line_versions.resize(logical_line_count, 0);
        } else if self.line_versions.len() > logical_line_count {
            self.line_versions.truncate(logical_line_count);
        }
    }

    fn compute_line_starts(s: &str) -> Vec<usize> {
        let mut v = Vec::with_capacity(128);
        v.push(0);
        for (i, ch) in s.char_indices() {
            if ch == '\n' && i + 1 < s.len() {
                v.push(i + 1);
            }
        }
        v
    }

    /// Number of lines (at least 1 even if empty text) – consistent with typical editor semantics.
    pub fn line_count(&self) -> usize {
        // If text ends with a newline, last start still counts as an empty trailing line.
        if self.text.ends_with('\n') { self.line_starts.len() + 1 } else { self.line_starts.len() }
    }

    /// Get per-line version (returns 0 if out of range)
    pub fn line_version(&self, line: usize) -> u64 {
        self.line_versions.get(line).cloned().unwrap_or(0)
    }

    /// Get start offset of given line index (returns text.len() if out of range).
    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts.get(line).cloned().unwrap_or(self.text.len())
    }

    /// Translate byte offset to (line, column) in O(log N) using binary search over line_starts.
    pub fn offset_to_line_col(&self, offset: usize) -> (usize, usize) {
        let off = offset.min(self.text.len());
        match self.line_starts.binary_search(&off) {
            Ok(line) => (line, 0),
            Err(idx) => {
                let line = idx - 1; // safe because first element always 0 and Err>0 for off>0
                let col = off - self.line_starts[line];
                (line, col)
            }
        }
    }

    /// Granular edit API (feature-gated). When `granular_edit` is enabled this will
    /// eventually use `lapce_core::Buffer` incremental edit capabilities; currently it
    /// still delegates to `apply_single_replace` (full rebuild of rope) until that logic
    /// is implemented. Keeping the function behind a feature flag allows us to evolve
    /// its contract without breaking downstream crates.
    #[cfg(feature = "granular_edit")]
    pub fn apply_granular_edit(&mut self, old_range: std::ops::Range<usize>, replacement: &str) {
        self.apply_single_replace(old_range, replacement);
    }

    /// When the feature is disabled we still expose a no-op wrapper for code paths compiled
    /// without the feature; this avoids conditional call sites. (Same behavior for now.)
    #[cfg(not(feature = "granular_edit"))]
    pub fn apply_granular_edit(&mut self, old_range: std::ops::Range<usize>, replacement: &str) {
        self.apply_single_replace(old_range, replacement);
    }

    /// Mark that the UI-updated text (egui TextEdit) should overwrite rope on next sync_to_rope.
    pub fn mark_text_modified(&mut self) {
        self.dirty_to_rope = true;
    }

    /// Get immutable access to current rope content as &str (alloc-free).
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Replace whole content (fast path). Avoid for large texts; use edit ranges later.
    pub fn set_text(&mut self, new_text: String) {
        // Treat as full replacement edit for undo (capture old full content)
        let old = std::mem::take(&mut self.text);
        let old_len = old.len();
        // Push edit record (full doc replace) unless this is initial empty -> initial text
        if !(old.is_empty() && new_text.is_empty()) {
            self.undo_stack.push(EditRecord {
                range: 0..old_len,
                inserted: new_text.clone(),
                removed: old,
            });
            self.redo_stack.clear();
        }
        self.text = new_text.clone();
        self.buffer = Buffer::new(&new_text); // temporary full rebuild (no granular diff yet)
        self.last_revision = 0;
        self.dirty_to_string = false;
        self.dirty_to_rope = false;
        self.recompute_line_starts();
        // Bulk bump all line versions
        for v in &mut self.line_versions { *v = v.wrapping_add(1); }
        self.revision = self.revision.wrapping_add(1);
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
    pub fn len(&self) -> usize {
        self.text.len()
    }
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Apply a single replace (delete old_range and insert replacement) on both rope and cached text.
    /// old_range is byte indices in current text before change.
    pub fn apply_single_replace(&mut self, old_range: std::ops::Range<usize>, replacement: &str) {
        // Safety clamp
        let start = old_range.start.min(self.text.len());
        let end = old_range.end.min(self.text.len()).max(start);
        // Capture removed text for undo & newline presence before mutation
        let removed = self.text.get(start..end).unwrap_or("").to_string();
        let removed_has_nl = removed.as_bytes().contains(&b'\n');
        let replacement_has_nl = replacement.as_bytes().contains(&b'\n');

        // Determine if this is a pure single-line edit (no newline creation/removal and both ends within same original line)
        let (start_line, _start_col) = self.offset_to_line_col(start);
        let (end_line, _end_col) = self.offset_to_line_col(end);
        let single_line_edit = !removed_has_nl && !replacement_has_nl && start_line == end_line;

        // Perform text mutation
        self.text.replace_range(start..end, replacement);

        // For now still rebuild underlying rope fully (will switch to granular Buffer::edit later)
        self.buffer = Buffer::new(&self.text);
        self.last_revision = 0;
        self.dirty_to_rope = false;
        self.dirty_to_string = false;

        // Record undo information (range of newly inserted text after mutation)
        self.undo_stack.push(EditRecord {
            range: start..start + replacement.len(),
            inserted: replacement.to_string(),
            removed: removed.clone(),
        });
        self.redo_stack.clear();

        if single_line_edit {
            // Incremental path: adjust subsequent line starts by delta instead of full recompute
            let delta: isize = replacement.len() as isize - (end - start) as isize;
            if delta != 0 {
                // Shift all following line start offsets
                for ls in self.line_starts.iter_mut().skip(start_line + 1) {
                    *ls = (*ls as isize + delta) as usize;
                }
            }
            // Bump only this line's version; others unchanged
            if let Some(v) = self.line_versions.get_mut(start_line) {
                *v = v.wrapping_add(1);
            }
            // (Line count unchanged in single-line edit without newlines)
        } else {
            // Fallback: full recompute (multi-line or newline-affecting edit)
            self.recompute_line_starts();
            // Bump all line versions (simpler for now; could refine to only touched span lines)
            for v in &mut self.line_versions { *v = v.wrapping_add(1); }
        }

        // Global revision always bumps
        self.revision = self.revision.wrapping_add(1);
    }

    /// Heuristic diff between previous and new full text; if it matches a single contiguous replace,
    /// apply via apply_single_replace and return true. Else return false (caller can fallback to set_text).
    pub fn try_single_span_update(&mut self, previous: &str, new_full: &str) -> bool {
        if previous == new_full {
            return true;
        }
        // Quick bounds: find common prefix
        let mut prefix = 0usize;
        let prev_bytes = previous.as_bytes();
        let new_bytes = new_full.as_bytes();
        let min_len = prev_bytes.len().min(new_bytes.len());
        while prefix < min_len && prev_bytes[prefix] == new_bytes[prefix] {
            prefix += 1;
        }
        // Find common suffix (excluding prefix region)
        let mut suffix = 0usize;
        while suffix < (prev_bytes.len() - prefix)
            && suffix < (new_bytes.len() - prefix)
            && prev_bytes[prev_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
        {
            suffix += 1;
        }
        // Compute differing spans
        let prev_mid_start = prefix;
        let prev_mid_end = prev_bytes.len() - suffix;
        let new_mid_start = prefix;
        let new_mid_end = new_bytes.len() - suffix;
        // If no change region -> done
        if prev_mid_start == prev_mid_end && new_mid_start == new_mid_end {
            return true;
        }
        // Extract replacement slice
        if new_mid_start > new_mid_end || prev_mid_start > prev_mid_end {
            return false;
        }
        if let Some(replacement) = new_full.get(new_mid_start..new_mid_end) {
            self.apply_single_replace(prev_mid_start..prev_mid_end, replacement);
            return true;
        }
        false
    }
}

impl EditorBuffer {
    /// Can we undo?
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }
    /// Can we redo?
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Undo last edit (if any). Returns true if something changed.
    pub fn undo(&mut self) -> bool {
        if let Some(edit) = self.undo_stack.pop() {
            // The recorded range in edit.range reflects the inserted text region after the edit.
            let start = edit.range.start;
            let end = start + edit.inserted.len();
            // Replace inserted with original removed text
            if end <= self.text.len() {
                self.text.replace_range(start..end, &edit.removed);
                // Rebuild rope
                self.buffer = Buffer::new(&self.text);
                self.last_revision = 0;
                // Push inverse onto redo stack
                let inverse = EditRecord {
                    range: start..start + edit.removed.len(),
                    inserted: edit.removed.clone(),
                    removed: edit.inserted,
                }; // note swapped roles
                self.redo_stack.push(inverse);
                self.recompute_line_starts();
                for v in &mut self.line_versions { *v = v.wrapping_add(1); }
                self.revision = self.revision.wrapping_add(1);
                return true;
            }
        }
        false
    }

    /// Redo last undone edit (if any). Returns true if something changed.
    pub fn redo(&mut self) -> bool {
        if let Some(edit) = self.redo_stack.pop() {
            let start = edit.range.start;
            let end = start + edit.inserted.len();
            if end <= self.text.len() {
                self.text.replace_range(start..end, &edit.removed);
                self.buffer = Buffer::new(&self.text);
                self.last_revision = 0;
                // Push inverse back to undo
                let inverse = EditRecord {
                    range: start..start + edit.removed.len(),
                    inserted: edit.removed.clone(),
                    removed: edit.inserted,
                };
                self.undo_stack.push(inverse);
                self.recompute_line_starts();
                for v in &mut self.line_versions { *v = v.wrapping_add(1); }
                self.revision = self.revision.wrapping_add(1);
                return true;
            }
        }
        false
    }

    /// Notify that external bulk text changes were applied directly on self.text (e.g., multi-cursor direct mutations)
    /// This recomputes line indices and bumps all line versions.
    pub fn notify_bulk_text_changed(&mut self) {
        self.recompute_line_starts();
        for v in &mut self.line_versions { *v = v.wrapping_add(1); }
        self.revision = self.revision.wrapping_add(1);
    }
}
