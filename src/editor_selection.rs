//! Lightweight multi-selection abstraction independent of lapce-core.
//! Provides a minimal API used by the editor for multi-caret typing and backspace.

use unicode_segmentation::UnicodeSegmentation;

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
    /// Move all carets one grapheme to the left (collapses selections first).
    pub fn move_left(&mut self, text: &str) {
        if self.regions.is_empty() {
            return;
        }
        let mut updated: Vec<SelRegion> = Vec::with_capacity(self.regions.len());
        for r in &self.regions {
            let anchor = r.anchor.min(text.len());
            let head = r.head.min(text.len());
            let collapsed = anchor == head;
            let target = if collapsed {
                prev_grapheme_boundary(text, head)
            } else {
                anchor.min(head)
            };
            updated.push(SelRegion::new(target, target, None));
        }
        sort_and_dedup(&mut updated);
        self.regions = updated;
    }
    /// Move all carets one grapheme to the right (collapses selections first).
    pub fn move_right(&mut self, text: &str) {
        if self.regions.is_empty() {
            return;
        }
        let len = text.len();
        let mut updated: Vec<SelRegion> = Vec::with_capacity(self.regions.len());
        for r in &self.regions {
            let anchor = r.anchor.min(len);
            let head = r.head.min(len);
            let collapsed = anchor == head;
            let target = if collapsed {
                next_grapheme_boundary(text, head)
            } else {
                anchor.max(head)
            };
            updated.push(SelRegion::new(target, target, None));
        }
        sort_and_dedup(&mut updated);
        self.regions = updated;
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

    /// Find next occurrence of the given text starting from the specified position.
    /// Returns Some((start, end)) if found, None otherwise.
    pub fn find_next_occurrence(text: &str, search: &str, from_pos: usize) -> Option<(usize, usize)> {
        if search.is_empty() {
            return None;
        }
        
        // Search from from_pos to end
        if let Some(idx) = text[from_pos..].find(search) {
            let start = from_pos + idx;
            let end = start + search.len();
            return Some((start, end));
        }
        
        // Wrap around: search from beginning to from_pos
        if let Some(idx) = text[..from_pos].find(search) {
            let start = idx;
            let end = start + search.len();
            return Some((start, end));
        }
        
        None
    }

    /// Add a new selection region for the next occurrence of the currently selected text.
    /// This is the core logic for CMD+D / CTRL+D functionality.
    /// Returns true if a new occurrence was found and added.
    pub fn add_next_occurrence(&mut self, text: &str, selected_text: &str) -> bool {
        if selected_text.is_empty() {
            return false;
        }

        // Get the position to search from (after the last selection in sorted order)
        // We need to search from the END of the last region
        let search_from = if let Some(last) = self.regions.iter().max_by_key(|r| r.max()) {
            last.max()
        } else {
            0
        };

        log::debug!("🔍 Searching for '{}' from position {}", selected_text, search_from);

        // Find next occurrence
        if let Some((start, end)) = Self::find_next_occurrence(text, selected_text, search_from) {
            log::debug!("   Found at {}..{}", start, end);
            
            // Check if this range is already in our regions to avoid duplicates
            let already_exists = self.regions.iter().any(|r| r.min() == start && r.max() == end);
            
            if !already_exists {
                log::debug!("   Adding new region {}..{}", start, end);
                self.regions.push(SelRegion::new(start, end, None));
                // Keep regions sorted by position for consistent behavior
                self.regions.sort_by_key(|r| r.min());
                return true;
            } else {
                log::debug!("   Already exists, wrapping around to find next");
                // If we found the same position, try searching from beginning to find next
                // This handles the wrap-around case where we've circled back
                return false;
            }
        }
        
        log::debug!("   No more occurrences found");
        false
    }

    /// Get all selected text ranges as a vector of (start, end, is_primary) tuples.
    /// The first region is marked as primary.
    pub fn get_all_ranges_with_primary(&self) -> Vec<(usize, usize, bool)> {
        self.regions
            .iter()
            .enumerate()
            .map(|(i, r)| (r.min(), r.max(), i == 0))
            .collect()
    }
}

fn sort_and_dedup(regions: &mut Vec<SelRegion>) {
    regions.sort_by_key(|r| (r.min(), r.max()));
    regions.dedup();
}

fn prev_grapheme_boundary(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let clamp = pos.min(text.len());
    let mut prev = 0usize;
    for (idx, _) in text.grapheme_indices(true) {
        if idx >= clamp {
            break;
        }
        prev = idx;
    }
    prev
}

fn next_grapheme_boundary(text: &str, pos: usize) -> usize {
    let len = text.len();
    if pos >= len {
        return len;
    }
    let clamp = pos.min(len);
    for (idx, _) in text.grapheme_indices(true) {
        if idx > clamp {
            return idx;
        }
    }
    len
}
