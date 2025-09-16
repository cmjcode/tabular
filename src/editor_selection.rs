//! Transitional multi-selection abstraction toward full lapce-core adoption.
//! Now directly wraps `lapce_core::selection::Selection`.

#[derive(Debug, Clone)]
pub struct MultiSelection {
    /// Source of truth for multi-range selection
    inner: lapce_core::selection::Selection,
}

impl Default for MultiSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiSelection {
    pub fn new() -> Self {
        Self { inner: lapce_core::selection::Selection::new() }
    }
    pub fn clear(&mut self) {
        self.inner = lapce_core::selection::Selection::new();
    }
    pub fn ensure_primary(&mut self, pos: usize) {
        if self.inner.is_empty() {
            self.inner
                .add_region(lapce_core::selection::SelRegion::new(pos, pos, None));
        }
    }
    pub fn add_collapsed(&mut self, pos: usize) {
        self.inner
            .add_region(lapce_core::selection::SelRegion::new(pos, pos, None));
    }
    pub fn collapse_all(&mut self) {
        let regs = self.inner.regions().to_vec();
        let mut out = lapce_core::selection::Selection::new();
        for r in regs {
            let p = r.max();
            out.add_region(lapce_core::selection::SelRegion::new(p, p, None));
        }
        self.inner = out;
    }
    pub fn apply_simple_insert(&mut self, at: usize, len: usize) {
        let regs = self.inner.regions().to_vec();
        let mut out = lapce_core::selection::Selection::new();
        for r in regs {
            let mut a = r.min();
            let mut h = r.max();
            if h >= at { h += len; }
            if a >= at { a += len; }
            out.add_region(lapce_core::selection::SelRegion::new(a, h, None));
        }
        self.inner = out;
    }
    pub fn apply_simple_delete(&mut self, at: usize, del_len: usize) {
        let end = at + del_len;
        let regs = self.inner.regions().to_vec();
        let mut out = lapce_core::selection::Selection::new();
        for r in regs {
            let mut a = r.min();
            let mut h = r.max();
            if h >= at && h < end { h = at; }
            if a >= at && a < end { a = at; }
            if h >= end { h -= del_len; }
            if a >= end { a -= del_len; }
            out.add_region(lapce_core::selection::SelRegion::new(a, h, None));
        }
        self.inner = out;
    }
    /// Return a Vec of (anchor, head) sorted & deduped by the min position.
    pub fn ranges(&self) -> Vec<(usize, usize)> {
        self.inner
            .regions()
            .iter()
            .map(|r| (r.min(), r.max()))
            .collect()
    }
    /// Return collapsed caret positions (head) deduped & sorted.
    pub fn caret_positions(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.inner.regions().iter().map(|r| r.max()).collect();
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
        for &pos in &positions { self.apply_simple_insert(pos, len); }
    }
    /// Apply backspace (delete one char to the left) for each collapsed caret.
    pub fn apply_backspace(&mut self, text: &mut String) {
        let mut positions = self.caret_positions();
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
    }

    // --- Migration helpers to/from lapce_core::selection::Selection ---
    pub fn to_lapce_selection(&self) -> lapce_core::selection::Selection {
        self.inner.clone()
    }

    pub fn from_lapce_selection(sel: &lapce_core::selection::Selection) -> Self {
        Self { inner: sel.clone() }
    }

    /// Replace current selection from a lapce-core Selection.
    pub fn set_from_lapce_selection(&mut self, sel: lapce_core::selection::Selection) {
        self.inner = sel;
    }

    /// Call after mutating `carets` directly to keep `inner` in sync.
    pub fn resync(&mut self) {
        // No-op: `inner` is the sole source of truth now.
    }

    /// Get the primary region's (anchor, head) as (min, max). Returns None if empty.
    pub fn primary_range(&self) -> Option<(usize, usize)> {
        self.inner
            .regions()
            .get(0)
            .map(|r| (r.min(), r.max()))
    }

    /// Set the primary region's (anchor, head). If empty, creates it.
    pub fn set_primary_range(&mut self, anchor: usize, head: usize) {
        let mut regs = self.inner.regions().to_vec();
        let newr = lapce_core::selection::SelRegion::new(anchor, head, None);
        if regs.is_empty() {
            regs.push(newr);
        } else {
            regs[0] = newr;
        }
        let mut out = lapce_core::selection::Selection::new();
        for r in regs { out.add_region(r); }
        self.inner = out;
    }
}
