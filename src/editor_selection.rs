//! Transitional multi-selection abstraction toward full lapce-core adoption.
//! Eventually this will be replaced by lapce_core's own selection structures.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub anchor: usize, // selection start
    pub head: usize,   // caret position (end)
}

impl Caret {
    pub fn new(pos: usize) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }
    pub fn collapsed(&self) -> bool {
        self.anchor == self.head
    }
    pub fn range(&self) -> (usize, usize) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
    pub fn set(&mut self, anchor: usize, head: usize) {
        self.anchor = anchor;
        self.head = head;
    }
}

#[derive(Debug, Clone)]
pub struct MultiSelection {
    /// Source of truth for multi-range selection
    inner: lapce_core::selection::Selection,
    /// Transitional cache for UI code paths that directly access carets
    pub carets: Vec<Caret>, // primary caret is carets[0] if non-empty
}

impl Default for MultiSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiSelection {
    pub fn new() -> Self {
        Self { inner: lapce_core::selection::Selection::new(), carets: Vec::new() }
    }
    pub fn clear(&mut self) {
        self.carets.clear();
        self.inner = lapce_core::selection::Selection::new();
    }
    pub fn primary(&self) -> Option<&Caret> {
        self.carets.get(0)
    }
    pub fn primary_mut(&mut self) -> Option<&mut Caret> {
        self.carets.get_mut(0)
    }
    pub fn ensure_primary(&mut self, pos: usize) {
        if self.carets.is_empty() {
            self.carets.push(Caret::new(pos));
            self.sync_inner_from_carets();
        }
    }
    pub fn add_caret(&mut self, caret: Caret) {
        self.carets.push(caret);
        self.dedup_and_sort();
        self.sync_inner_from_carets();
    }
    pub fn add_collapsed(&mut self, pos: usize) {
        self.add_caret(Caret::new(pos));
    }
    pub fn dedup_and_sort(&mut self) {
        self.carets.sort_by_key(|c| c.range());
        self.carets.dedup_by(|a, b| a.range() == b.range());
        // Keep inner in order as well
        self.sync_inner_from_carets();
    }
    pub fn collapse_all(&mut self) {
        for c in &mut self.carets {
            c.anchor = c.head;
        }
        self.sync_inner_from_carets();
    }
    pub fn apply_simple_insert(&mut self, at: usize, len: usize) {
        for c in &mut self.carets {
            if c.head >= at {
                c.head += len;
            }
            if c.anchor >= at {
                c.anchor += len;
            }
        }
        self.sync_inner_from_carets();
    }
    pub fn apply_simple_delete(&mut self, at: usize, del_len: usize) {
        let end = at + del_len;
        for c in &mut self.carets {
            // If caret inside deleted span, move to start
            if c.head >= at && c.head < end {
                c.head = at;
            }
            if c.anchor >= at && c.anchor < end {
                c.anchor = at;
            }
            // Shift positions after deletion
            if c.head >= end {
                c.head -= del_len;
            }
            if c.anchor >= end {
                c.anchor -= del_len;
            }
        }
        self.sync_inner_from_carets();
    }
    /// Return a Vec of (anchor, head) sorted & deduped by the min position.
    pub fn ranges(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self.carets.iter().map(|c| c.range()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    /// Return collapsed caret positions (head) deduped & sorted.
    pub fn caret_positions(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.carets.iter().map(|c| c.head).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    /// Apply same inserted text at each collapsed caret (multi-cursor typing).
    /// Assumes all carets are collapsed. Processes from right to left to avoid shifting earlier indices.
    pub fn apply_insert_text(&mut self, text: &mut String, insert: &str) {
        if insert.is_empty() {
            return;
        }
        let len = insert.len();
        let positions = self.caret_positions();
        // process descending
        for &pos in positions.iter().rev() {
            if pos <= text.len() {
                text.insert_str(pos, insert);
            }
        }
        // Update caret positions
        for &pos in &positions {
            self.apply_simple_insert(pos, len);
        }
        self.sync_inner_from_carets();
    }
    /// Apply backspace (delete one char to the left) for each collapsed caret.
    pub fn apply_backspace(&mut self, text: &mut String) {
        let mut positions = self.caret_positions();
        // For correctness, operate from left to right? Actually we remove left of caret, which shifts later indices.
        // So process from left to right but adjust subsequent positions by tracking delta, simpler approach: process descending too.
        positions.sort_unstable();
        let mut performed: Vec<(usize, usize)> = Vec::new(); // (start,len)
        for &pos in positions.iter() {
            if pos == 0 {
                continue;
            }
            let del_start = pos - 1;
            if del_start < text.len() {
                // Remove single char (could be part of multi-byte; assume ASCII for now – future: use char boundary)
                // Ensure char boundary
                let mut real_start = del_start;
                while !text.is_char_boundary(real_start) && real_start > 0 {
                    real_start -= 1;
                }
                let mut real_end = pos;
                while real_end < text.len() && !text.is_char_boundary(real_end) {
                    real_end += 1;
                }
                text.replace_range(real_start..real_end, "");
                performed.push((real_start, real_end - real_start));
            }
        }
        // Apply selection updates from last deletion to first to avoid double shifting logic.
        performed.sort_by_key(|(s, _)| *s);
        for (start, len) in performed.into_iter().rev() {
            self.apply_simple_delete(start, len);
        }
        self.sync_inner_from_carets();
    }

    // --- Migration helpers to/from lapce_core::selection::Selection ---
    pub fn to_lapce_selection(&self) -> lapce_core::selection::Selection {
        // Ensure inner reflects current carets before exporting
        let mut tmp = self.clone();
        tmp.sync_inner_from_carets();
        tmp.inner
    }

    pub fn from_lapce_selection(sel: &lapce_core::selection::Selection) -> Self {
        let mut s = Self { inner: sel.clone(), carets: Vec::new() };
        s.sync_carets_from_inner();
        s
    }

    fn sync_inner_from_carets(&mut self) {
        let mut sel = lapce_core::selection::Selection::new();
        for c in &self.carets {
            let (start, end) = c.range();
            sel.add_region(lapce_core::selection::SelRegion::new(start, end, None));
        }
        self.inner = sel;
    }

    fn sync_carets_from_inner(&mut self) {
        self.carets.clear();
        for r in self.inner.regions() {
            self.carets.push(Caret { anchor: r.min(), head: r.max() });
        }
        self.dedup_and_sort();
    }

    /// Call after mutating `carets` directly to keep `inner` in sync.
    pub fn resync(&mut self) {
        self.sync_inner_from_carets();
    }
}
