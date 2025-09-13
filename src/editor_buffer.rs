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

    /// Replace whole content (used by tab switching / file load)
    pub fn set_text(&mut self, new_text: String) {
        if self.text == new_text { return; }
        let old = std::mem::take(&mut self.text);
        let old_len = old.len();
        self.undo_stack.push(EditRecord { range: 0..old_len, inserted: new_text.clone(), removed: old });
        self.redo_stack.clear();
        self.text = new_text.clone();
        self.buffer = Buffer::new(&new_text);
        self.last_revision = 0;
        self.dirty_to_string = false;
        self.dirty_to_rope = false;
        self.recompute_line_starts();
        for v in &mut self.line_versions { *v = v.wrapping_add(1); }
        self.revision = self.revision.wrapping_add(1);
    }

    /// Mark that egui-bound text mutated externally (not used heavily now but kept for compatibility)
    pub fn mark_text_modified(&mut self) {
        self.dirty_to_rope = true;
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
    /// use `lapce_core::Buffer` incremental edit capabilities instead of rebuilding the
    /// entire rope. We keep the public signature stable; internally we:
    /// 1. Apply edit to `self.text` (source of truth for UI binding for now)
    /// 2. Use Buffer::edit for true rope mutation (no full rebuild) if possible
    /// 3. Run the same incremental line_starts maintenance logic as `apply_single_replace`
    /// 4. Push undo record
    /// Safety: On any mismatch or panic risk we fallback to rebuilding from full text.
    #[cfg(feature = "granular_edit")]
    pub fn apply_granular_edit(&mut self, old_range: std::ops::Range<usize>, replacement: &str) {
        use lapce_core::editor::EditType;
        use lapce_core::selection::Selection as LapceSelection;
        let start = old_range.start.min(self.text.len());
        let end = old_range.end.min(self.text.len()).max(start);
        let removed = self.text.get(start..end).unwrap_or("").to_string();
        if removed.is_empty() && replacement.is_empty() { return; }

        // Capture metrics before mutation
        let removed_has_nl = removed.as_bytes().contains(&b'\n');
        let replacement_has_nl = replacement.as_bytes().contains(&b'\n');
        let (start_line, _sc) = self.offset_to_line_col(start);
        let (end_line, _ec) = self.offset_to_line_col(end);
        let single_line_edit = !removed_has_nl && !replacement_has_nl && start_line == end_line;

        // Apply to UI text first (source of truth currently)
        self.text.replace_range(start..end, replacement);

    // Apply incremental edit to rope using lapce_core Buffer::edit
    let sel = LapceSelection::region(start, end);
    let edits = std::iter::once((sel, replacement));
    let _ = self.buffer.edit(edits, EditType::Other);

        // Undo bookkeeping
        self.undo_stack.push(EditRecord { range: start..start + replacement.len(), inserted: replacement.to_string(), removed: removed.clone() });
        self.redo_stack.clear();
        self.last_revision = 0;
        self.dirty_to_rope = false;
        self.dirty_to_string = false;

        // Incremental line/versions maintenance (mirrors logic in apply_single_replace)
        if single_line_edit {
            let delta: isize = replacement.len() as isize - (end - start) as isize;
            if delta != 0 { for ls in self.line_starts.iter_mut().skip(start_line + 1) { *ls = (*ls as isize + delta) as usize; } }
            if let Some(v) = self.line_versions.get_mut(start_line) { *v = v.wrapping_add(1); }
        } else {
            let removed_nl = removed.as_bytes().iter().filter(|&&b| b == b'\n').count();
            let replacement_nl = replacement.as_bytes().iter().filter(|&&b| b == b'\n').count();
            if removed_nl == replacement_nl && removed_nl > 0 {
                let delta: isize = replacement.len() as isize - (end - start) as isize;
                let block_first_offset = self.line_starts[start_line];
                let mut new_starts: Vec<usize> = Vec::with_capacity(removed_nl);
                for (i, ch) in replacement.char_indices() { if ch == '\n' { if i + 1 < replacement.len() { new_starts.push(block_first_offset + i + 1); } } }
                if new_starts.len() == removed_nl {
                    for (idx, val) in new_starts.iter().enumerate() { let line_idx = start_line + 1 + idx; if line_idx < self.line_starts.len() { self.line_starts[line_idx] = *val; } }
                    if delta != 0 { let tail_start_line = start_line + 1 + removed_nl; for ls in self.line_starts.iter_mut().skip(tail_start_line) { *ls = (*ls as isize + delta) as usize; } }
                    let affected_last = start_line + removed_nl; for line in start_line..=affected_last { if let Some(v) = self.line_versions.get_mut(line) { *v = v.wrapping_add(1); } }
                } else { self.recompute_line_starts(); for v in &mut self.line_versions { *v = v.wrapping_add(1); } }
            } else if removed_nl == 0 && replacement_nl == 0 {
                let delta: isize = replacement.len() as isize - (end - start) as isize;
                if delta != 0 { for ls in self.line_starts.iter_mut().skip(end_line + 1) { *ls = (*ls as isize + delta) as usize; } }
                for line in start_line..=end_line { if let Some(v) = self.line_versions.get_mut(line) { *v = v.wrapping_add(1); } }
            } else {
                // Newline count changed: use incremental algorithm (copied from updated apply_single_replace)
                let old_internal = removed_nl;
                let new_internal = replacement_nl;
                let delta_bytes: isize = replacement.len() as isize - (end - start) as isize;
                if start_line >= self.line_starts.len() { self.recompute_line_starts(); for v in &mut self.line_versions { *v = v.wrapping_add(1); } self.revision = self.revision.wrapping_add(1); return; }
                let mut ok = true;
                for _ in 0..old_internal { let idx = start_line + 1; if idx < self.line_starts.len() { self.line_starts.remove(idx); } else { ok = false; break; } }
                if ok && new_internal > 0 {
                    let block_first_offset = self.line_starts[start_line];
                    let mut new_starts: Vec<usize> = Vec::with_capacity(new_internal);
                    for (i, ch) in replacement.char_indices() { if ch == '\n' && i + 1 < replacement.len() { new_starts.push(block_first_offset + i + 1); } }
                    if new_starts.len() != new_internal { ok = false; } else { let mut insert_pos = start_line + 1; for ns in new_starts { self.line_starts.insert(insert_pos, ns); insert_pos += 1; } }
                }
                if ok {
                    if delta_bytes != 0 { let tail_start = start_line + 1 + new_internal; for ls in self.line_starts.iter_mut().skip(tail_start) { *ls = (*ls as isize + delta_bytes) as usize; } }
                    let line_delta = new_internal as isize - old_internal as isize;
                    if line_delta > 0 { for i in 0..line_delta { self.line_versions.insert(start_line + 1 + i as usize, 0); } }
                    else if line_delta < 0 { for _ in 0..(-line_delta) { if start_line + 1 < self.line_versions.len() { self.line_versions.remove(start_line + 1); } } }
                    let logical_line_count = self.line_count();
                    if self.line_versions.len() < logical_line_count { self.line_versions.resize(logical_line_count, 0); }
                    else if self.line_versions.len() > logical_line_count { self.line_versions.truncate(logical_line_count); }
                    let last_affected = start_line + new_internal;
                    for line in start_line..=last_affected { if let Some(v) = self.line_versions.get_mut(line) { *v = v.wrapping_add(1); } }
                }
                if !ok { self.recompute_line_starts(); for v in &mut self.line_versions { *v = v.wrapping_add(1); } }
            }
        }
        self.revision = self.revision.wrapping_add(1);
        // Debug validation (only in debug builds): ensure rope full text matches our cached text
        debug_assert_eq!(self.buffer.to_string(), self.text, "rope and text diverged after granular edit");
    }

    /// When the feature is disabled we still expose a no-op wrapper for code paths compiled
    /// without the feature; this avoids conditional call sites. (Same behavior for now.)
    // Core primitive replace used by UI & feature-gated granular path fallback.
    pub fn apply_single_replace(&mut self, old_range: std::ops::Range<usize>, replacement: &str) {
        let start = old_range.start.min(self.text.len());
        let end = old_range.end.min(self.text.len()).max(start);
        let removed = self.text.get(start..end).unwrap_or("").to_string();
        let removed_has_nl = removed.as_bytes().contains(&b'\n');
        let replacement_has_nl = replacement.as_bytes().contains(&b'\n');
        let (start_line, _sc) = self.offset_to_line_col(start);
        let (end_line, _ec) = self.offset_to_line_col(end);
        let single_line_edit = !removed_has_nl && !replacement_has_nl && start_line == end_line;

        self.text.replace_range(start..end, replacement);
        #[cfg(feature = "granular_edit")]
        {
            let sel = lapce_core::selection::Selection::region(start, end);
            let _ = self
                .buffer
                .edit(std::iter::once((sel, replacement)), lapce_core::editor::EditType::Other);
        }
        #[cfg(not(feature = "granular_edit"))]
        {
            self.buffer = Buffer::new(&self.text);
        }
        self.last_revision = 0;
        self.dirty_to_rope = false;
        self.dirty_to_string = false;
        self.undo_stack.push(EditRecord { range: start..start + replacement.len(), inserted: replacement.to_string(), removed: removed.clone() });
        self.redo_stack.clear();

        if single_line_edit {
            let delta: isize = replacement.len() as isize - (end - start) as isize;
            if delta != 0 { for ls in self.line_starts.iter_mut().skip(start_line + 1) { *ls = (*ls as isize + delta) as usize; } }
            if let Some(v) = self.line_versions.get_mut(start_line) { *v = v.wrapping_add(1); }
        } else {
            let removed_nl = removed.as_bytes().iter().filter(|&&b| b == b'\n').count();
            let replacement_nl = replacement.as_bytes().iter().filter(|&&b| b == b'\n').count();
            if removed_nl == replacement_nl && removed_nl > 0 {
                let delta: isize = replacement.len() as isize - (end - start) as isize;
                let block_first_offset = self.line_starts[start_line];
                let mut new_starts: Vec<usize> = Vec::with_capacity(removed_nl);
                for (i, ch) in replacement.char_indices() { if ch == '\n' { if i + 1 < replacement.len() { new_starts.push(block_first_offset + i + 1); } } }
                if new_starts.len() == removed_nl {
                    for (idx, val) in new_starts.iter().enumerate() { let line_idx = start_line + 1 + idx; if line_idx < self.line_starts.len() { self.line_starts[line_idx] = *val; } }
                    if delta != 0 { let tail_start_line = start_line + 1 + removed_nl; for ls in self.line_starts.iter_mut().skip(tail_start_line) { *ls = (*ls as isize + delta) as usize; } }
                    let affected_last = start_line + removed_nl; for line in start_line..=affected_last { if let Some(v) = self.line_versions.get_mut(line) { *v = v.wrapping_add(1); } }
                } else { self.recompute_line_starts(); for v in &mut self.line_versions { *v = v.wrapping_add(1); } }
            } else if removed_nl == 0 && replacement_nl == 0 {
                let delta: isize = replacement.len() as isize - (end - start) as isize;
                if delta != 0 { for ls in self.line_starts.iter_mut().skip(end_line + 1) { *ls = (*ls as isize + delta) as usize; } }
                for line in start_line..=end_line { if let Some(v) = self.line_versions.get_mut(line) { *v = v.wrapping_add(1); } }
            } else {
                // Newline count changed: attempt incremental adjustment of line_starts instead of full recompute.
                // Strategy:
                // 1. Remove old internal line starts belonging to the removed text.
                // 2. Insert new internal line starts derived from replacement.
                // 3. Shift subsequent line starts by byte delta.
                // 4. Adjust line_versions length (insert/remove) and bump affected lines.
                // On any inconsistency, fall back to full recompute.
                let old_internal = removed_nl; // number of internal newlines removed
                let new_internal = replacement_nl; // number of internal newlines inserted
                let delta_bytes: isize = replacement.len() as isize - (end - start) as isize;

                // Safety guard: if start_line points beyond existing starts, fallback
                if start_line >= self.line_starts.len() { self.recompute_line_starts(); for v in &mut self.line_versions { *v = v.wrapping_add(1); } self.revision = self.revision.wrapping_add(1); return; }

                // 1. Remove 'old_internal' entries following start_line (these correspond to lines that existed inside removed span).
                let mut ok = true;
                for _ in 0..old_internal {
                    let idx = start_line + 1; // position of next internal line start to remove
                    if idx < self.line_starts.len() { self.line_starts.remove(idx); } else { ok = false; break; }
                }

                // 2. Compute new internal starts based on replacement (relative char_indices)
                if ok {
                    if new_internal > 0 {
                        let block_first_offset = self.line_starts[start_line];
                        let mut new_starts: Vec<usize> = Vec::with_capacity(new_internal);
                        for (i, ch) in replacement.char_indices() {
                            if ch == '\n' && i + 1 < replacement.len() { new_starts.push(block_first_offset + i + 1); }
                        }
                        if new_starts.len() != new_internal { ok = false; }
                        else {
                            // Insert in ascending order at position start_line+1
                            let mut insert_pos = start_line + 1;
                            for ns in new_starts { self.line_starts.insert(insert_pos, ns); insert_pos += 1; }
                        }
                    }
                }

                if ok {
                    // 3. Shift tail line starts by delta_bytes
                    if delta_bytes != 0 {
                        let tail_start = start_line + 1 + new_internal; // first unaffected line after replacement block
                        for ls in self.line_starts.iter_mut().skip(tail_start) { *ls = (*ls as isize + delta_bytes) as usize; }
                    }

                    // 4. Adjust line_versions length and bump affected lines
                    let line_delta = new_internal as isize - old_internal as isize;
                    if line_delta > 0 {
                        // Insert new versions after start_line with initial version 0 (will bump below)
                        for i in 0..line_delta { self.line_versions.insert(start_line + 1 + i as usize, 0); }
                    } else if line_delta < 0 {
                        for _ in 0..(-line_delta) { if start_line + 1 < self.line_versions.len() { self.line_versions.remove(start_line + 1); } }
                    }
                    // Ensure versions vector is not shorter than logical lines (in pathological edge cases)
                    let logical_line_count = self.line_count();
                    if self.line_versions.len() < logical_line_count { self.line_versions.resize(logical_line_count, 0); }
                    else if self.line_versions.len() > logical_line_count { self.line_versions.truncate(logical_line_count); }

                    // Bump versions for lines directly impacted (original start line through last new internal line)
                    let last_affected = start_line + new_internal; // inclusive
                    for line in start_line..=last_affected { if let Some(v) = self.line_versions.get_mut(line) { *v = v.wrapping_add(1); } }
                }

                if !ok { // Fallback path
                    self.recompute_line_starts();
                    for v in &mut self.line_versions { *v = v.wrapping_add(1); }
                }
            }
        }
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn try_single_span_update(&mut self, previous: &str, new_full: &str) -> bool {
        if previous == new_full { return true; }
        let mut prefix = 0usize;
        let prev_bytes = previous.as_bytes();
        let new_bytes = new_full.as_bytes();
        let min_len = prev_bytes.len().min(new_bytes.len());
        while prefix < min_len && prev_bytes[prefix] == new_bytes[prefix] { prefix += 1; }
        let mut suffix = 0usize;
        while suffix < (prev_bytes.len() - prefix)
            && suffix < (new_bytes.len() - prefix)
            && prev_bytes[prev_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix] { suffix += 1; }
        let prev_mid_start = prefix;
        let prev_mid_end = prev_bytes.len() - suffix;
        let new_mid_start = prefix;
        let new_mid_end = new_bytes.len() - suffix;
        if prev_mid_start == prev_mid_end && new_mid_start == new_mid_end { return true; }
        if new_mid_start > new_mid_end || prev_mid_start > prev_mid_end { return false; }
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
