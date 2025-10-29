//! Lightweight multi-selection abstraction independent of lapce-core.
//! Provides a minimal API used by the editor for multi-caret typing and backspace.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelRegion {
    pub anchor: usize,
    pub head: usize,
}

impl SelRegion {
    pub fn new(anchor: usize, head: usize, _placeholder: Option<()>) -> Self {
        Self { anchor, head }
    }
    #[inline]
    pub fn min(&self) -> usize {
        self.anchor.min(self.head)
    }
    #[inline]
    pub fn max(&self) -> usize {
        self.anchor.max(self.head)
    }
}

#[derive(Debug, Clone)]
pub struct MultiSelection {
    /// Source of truth for multi-range selection (ordered, non-overlapping preferred but not enforced)
    regions: Vec<SelRegion>,
}

impl Default for MultiSelection {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiSelection {
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
        }
    }
    pub fn clear(&mut self) {
        self.regions.clear();
    }
    pub fn ensure_primary(&mut self, pos: usize) {
        if self.regions.is_empty() {
            self.regions.push(SelRegion::new(pos, pos, None));
        }
    }
    pub fn add_collapsed(&mut self, pos: usize) {
        self.regions.push(SelRegion::new(pos, pos, None));
        // keep a stable order and dedup exact duplicates
        self.regions.sort_by_key(|r| (r.min(), r.max()));
        self.regions.dedup();
    }
    pub fn collapse_all(&mut self) {
        let regs = self.regions.clone();
        let mut out: Vec<SelRegion> = Vec::with_capacity(regs.len());
        for r in regs {
            let p = r.max();
            out.push(SelRegion::new(p, p, None));
        }
        self.regions = out;
    }
    pub fn apply_simple_insert(&mut self, at: usize, len: usize) {
        let regs = self.regions.clone();
        let mut out: Vec<SelRegion> = Vec::with_capacity(regs.len());
        for r in regs {
            let mut a = r.min();
            let mut h = r.max();
            if h >= at {
                h += len;
            }
            if a >= at {
                a += len;
            }
            out.push(SelRegion::new(a, h, None));
        }
        self.regions = out;
    }
    pub fn apply_simple_delete(&mut self, at: usize, del_len: usize) {
        let end = at + del_len;
        let regs = self.regions.clone();
        let mut out: Vec<SelRegion> = Vec::with_capacity(regs.len());
        for r in regs {
            let mut a = r.min();
            let mut h = r.max();
            if h >= at && h < end {
                h = at;
            }
            if a >= at && a < end {
                a = at;
            }
            if h >= end {
                h -= del_len;
            }
            if a >= end {
                a -= del_len;
            }
            out.push(SelRegion::new(a, h, None));
        }
        self.regions = out;
    }
    /// Return a Vec of (anchor, head) sorted & deduped by the min position.
    pub fn ranges(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self.regions.iter().map(|r| (r.min(), r.max())).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    /// Return collapsed caret positions (head) deduped & sorted.
    pub fn caret_positions(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.regions.iter().map(|r| r.max()).collect();
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

    /// Simple helpers keeping compatibility with previous API surface
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
    pub fn len(&self) -> usize {
        self.regions.len()
    }
    pub fn regions(&self) -> &[SelRegion] {
        &self.regions
    }

    /// No-op in egui-only mode (kept for compatibility)
    pub fn resync(&mut self) {}

    /// Get the primary region's (anchor, head) as (min, max). Returns None if empty.
    pub fn primary_range(&self) -> Option<(usize, usize)> {
        self.regions.first().map(|r| (r.min(), r.max()))
    }

    /// Set the primary region's (anchor, head). If empty, creates it.
    pub fn set_primary_range(&mut self, anchor: usize, head: usize) {
        if self.regions.is_empty() {
            self.regions.push(SelRegion::new(anchor, head, None));
        } else {
            self.regions[0] = SelRegion::new(anchor, head, None);
        }
    }
}
