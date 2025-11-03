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
        sort_and_dedup(&mut self.regions);
    }
    pub fn collapse_all(&mut self) {
        let regs = self.regions.clone();
        let mut out: Vec<SelRegion> = Vec::with_capacity(regs.len());
        for r in regs {
            let p = r.max();
            out.push(SelRegion::new(p, p, None));
        }
        sort_and_dedup(&mut out);
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
        sort_and_dedup(&mut out);
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
        sort_and_dedup(&mut out);
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
    /// Returns true when at least one selection spans more than a caret.
    pub fn has_expanded_ranges(&self) -> bool {
        self.regions.iter().any(|r| r.anchor != r.head)
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
    /// Move all carets one line up, clamping to the available column on the target line.
    pub fn move_up(&mut self, text: &str) {
        if self.regions.is_empty() {
            return;
        }
        let len = text.len();
        let mut updated: Vec<SelRegion> = Vec::with_capacity(self.regions.len());
        for r in &self.regions {
            let head = r.head.min(len);
            let current_start = line_start(text, head);
            let current_column = column_at(text, current_start, head);
            if let Some(prev_start) = previous_line_start(text, current_start) {
                let prev_end = line_end(text, prev_start);
                let prev_len = text[prev_start..prev_end].chars().count();
                let target_column = current_column.min(prev_len);
                let target = column_to_byte(text, prev_start, prev_end, target_column);
                updated.push(SelRegion::new(target, target, None));
            } else {
                // Already on first line; keep caret where it is
                updated.push(SelRegion::new(head, head, None));
            }
        }
        sort_and_dedup(&mut updated);
        self.regions = updated;
    }
    /// Move all carets one line down, clamping to the available column on the target line.
    pub fn move_down(&mut self, text: &str) {
        if self.regions.is_empty() {
            return;
        }
        let len = text.len();
        let mut updated: Vec<SelRegion> = Vec::with_capacity(self.regions.len());
        for r in &self.regions {
            let head = r.head.min(len);
            let current_start = line_start(text, head);
            let current_end = line_end(text, head);
            let current_column = column_at(text, current_start, head);
            if current_end >= len {
                // Last line; keep caret where it is
                updated.push(SelRegion::new(head, head, None));
            } else {
                // Skip the newline (if present) to reach the next line start
                let mut next_start = current_end;
                if next_start < len && text.as_bytes()[next_start] == b'\n' {
                    next_start += 1;
                }
                if next_start > len {
                    next_start = len;
                }
                if next_start >= len {
                    updated.push(SelRegion::new(len, len, None));
                } else {
                    let next_end = line_end(text, next_start);
                    let next_len = text[next_start..next_end].chars().count();
                    let target_column = current_column.min(next_len);
                    let target = column_to_byte(text, next_start, next_end, target_column);
                    updated.push(SelRegion::new(target, target, None));
                }
            }
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
        let before_len = text.len();
        let insert_dbg = insert.escape_debug().to_string();
        let len = insert.len();
        let positions = self.caret_positions();
        log::debug!(
            "[multi] apply_insert_text start positions={:?} insert='{}' len={} text_len_before={}",
            positions,
            insert_dbg,
            len,
            before_len
        );
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
        let after_len = text.len();
        let caret_positions_after = self.caret_positions();
        log::debug!(
            "[multi] apply_insert_text done text_len_after={} delta={} positions_after={:?}",
            after_len,
            after_len.saturating_sub(before_len),
            caret_positions_after
        );
    }

    /// Apply inserted text to all carets EXCEPT the given primary_pos.
    /// Use this when the primary insertion was already performed by the widget,
    /// so we only need to mirror it to the other carets.
    pub fn apply_insert_text_others(
        &mut self,
        text: &mut String,
        insert: &str,
        primary_pos: usize,
    ) {
        if insert.is_empty() || self.regions.is_empty() {
            return;
        }

        let before_len = text.len();
        let insert_dbg = insert.escape_debug().to_string();
        let len = insert.len();
        let mut positions = self.caret_positions();
        // Remove the primary position so we don't double-insert at the main caret
        positions.retain(|&p| p != primary_pos);

        if positions.is_empty() {
            return;
        }

        log::debug!(
            "[multi] apply_insert_text_others positions={:?} insert='{}' len={} text_len_before={}",
            positions,
            insert_dbg,
            len,
            before_len
        );

        for &pos in positions.iter().rev() {
            if pos <= text.len() {
                text.insert_str(pos, insert);
            }
        }
        // Update caret/selection positions for each mirrored insertion
        for &pos in &positions {
            self.apply_simple_insert(pos, len);
        }

        let after_len = text.len();
        let caret_positions_after = self.caret_positions();
        log::debug!(
            "[multi] apply_insert_text_others done text_len_after={} delta={} positions_after={:?}",
            after_len,
            after_len.saturating_sub(before_len),
            caret_positions_after
        );
    }
    /// Apply backspace (delete one char to the left) for each collapsed caret.
    pub fn apply_backspace(&mut self, text: &mut String) {
        let before_len = text.len();
        let mut positions = self.caret_positions();
        positions.sort_unstable();
        log::debug!(
            "[multi] apply_backspace start positions={:?} text_len_before={}",
            positions,
            before_len
        );
        let mut performed: Vec<(usize, usize)> = Vec::new(); // (start,len)
        for &pos in positions.iter().rev() {
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
                let removed_dbg = text[real_start..real_end].escape_debug().to_string();
                log::debug!(
                    "[multi] apply_backspace removing '{}' at {}..{}",
                    removed_dbg,
                    real_start,
                    real_end
                );
                text.replace_range(real_start..real_end, "");
                performed.push((real_start, real_end - real_start));
            }
        }
        // Apply selection updates from last deletion to first to avoid double shifting logic.
        performed.sort_by_key(|(s, _)| *s);
        log::debug!("[multi] apply_backspace deletions={:?}", performed);
        for &(start, len) in performed.iter().rev() {
            self.apply_simple_delete(start, len);
        }
        let after_len = text.len();
        let caret_positions_after = self.caret_positions();
        log::debug!(
            "[multi] apply_backspace done text_len_after={} delta={} positions_after={:?}",
            after_len,
            before_len.saturating_sub(after_len),
            caret_positions_after
        );
    }

    /// Apply forward delete (Delete key) at each caret.
    pub fn apply_delete_forward(&mut self, text: &mut String) {
        if self.regions.is_empty() {
            return;
        }
        let before_len = text.len();
        let mut positions = self.caret_positions();
        positions.sort_unstable();
        log::debug!(
            "[multi] apply_delete_forward start positions={:?} text_len_before={}",
            positions,
            before_len
        );
        let mut performed: Vec<(usize, usize)> = Vec::new();
        for &pos in positions.iter().rev() {
            if pos >= text.len() {
                continue;
            }
            let end = next_grapheme_boundary(text, pos);
            if end > pos {
                let removed_dbg = text[pos..end].escape_debug().to_string();
                log::debug!(
                    "[multi] apply_delete_forward removing '{}' at {}..{}",
                    removed_dbg,
                    pos,
                    end
                );
                text.replace_range(pos..end, "");
                performed.push((pos, end - pos));
            }
        }
        performed.sort_by_key(|(s, _)| *s);
        for &(start, len) in performed.iter().rev() {
            self.apply_simple_delete(start, len);
        }
        let after_len = text.len();
        let caret_positions_after = self.caret_positions();
        log::debug!(
            "[multi] apply_delete_forward done text_len_after={} delta={} positions_after={:?}",
            after_len,
            before_len.saturating_sub(after_len),
            caret_positions_after
        );
    }

    /// Replace every expanded selection with the same replacement string.
    /// Passing an empty replacement behaves like multi-range deletion.
    pub fn apply_replace_selected(&mut self, text: &mut String, replacement: &str) {
        let mut ranges: Vec<(usize, usize)> = self
            .regions
            .iter()
            .map(|r| (r.min(), r.max()))
            .filter(|(s, e)| s < e)
            .collect();
        if ranges.is_empty() {
            return;
        }
        ranges.sort_unstable();
        let mut replaced: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for &(start, end) in ranges.iter().rev() {
            if end > text.len() || start >= end {
                continue;
            }
            let removed_dbg = text[start..end].escape_debug().to_string();
            log::debug!(
                "[multi] apply_replace_selected {}..{} removing '{}' -> inserting '{}'",
                start,
                end,
                removed_dbg,
                replacement.escape_debug()
            );
            text.replace_range(start..end, replacement);
            replaced.push((start, end));
        }
        let repl_len = replacement.len();
        for (start, end) in replaced.into_iter().rev() {
            let del_len = end - start;
            self.apply_simple_delete(start, del_len);
            if repl_len > 0 {
                self.apply_simple_insert(start, repl_len);
            }
        }
        if repl_len == 0 {
            self.collapse_all();
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
        sort_and_dedup(&mut self.regions);
    }

    /// Find next occurrence of the given text starting from the specified position.
    /// Returns Some((start, end)) if found, None otherwise.
    pub fn find_next_occurrence(
        text: &str,
        search: &str,
        from_pos: usize,
    ) -> Option<(usize, usize)> {
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

        log::debug!(
            "🔍 Searching for '{}' from position {}",
            selected_text,
            search_from
        );

        // Find next occurrence
        if let Some((start, end)) = Self::find_next_occurrence(text, selected_text, search_from) {
            log::debug!("   Found at {}..{}", start, end);

            // Check if this range is already in our regions to avoid duplicates
            let already_exists = self
                .regions
                .iter()
                .any(|r| r.min() == start && r.max() == end);

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

fn line_start(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let mut idx = pos.min(bytes.len());
    while idx > 0 && bytes[idx - 1] != b'\n' {
        idx -= 1;
    }
    idx
}

fn line_end(text: &str, pos: usize) -> usize {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut idx = pos.min(len);
    while idx < len && bytes[idx] != b'\n' {
        idx += 1;
    }
    idx
}

fn previous_line_start(text: &str, current_start: usize) -> Option<usize> {
    if current_start == 0 {
        return None;
    }
    let prev_end = current_start.saturating_sub(1);
    Some(line_start(text, prev_end))
}

fn column_at(text: &str, line_start: usize, pos: usize) -> usize {
    let clamp = pos.min(text.len());
    let slice = &text[line_start..clamp];
    slice.chars().take_while(|&ch| ch != '\n').count()
}

fn column_to_byte(text: &str, line_start: usize, line_end: usize, column: usize) -> usize {
    let slice = &text[line_start..line_end];
    for (idx, (offset, _)) in slice.char_indices().enumerate() {
        if idx == column {
            return line_start + offset;
        }
    }
    line_end
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
