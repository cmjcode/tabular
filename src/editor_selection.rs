//! Transitional multi-selection abstraction toward full lapce-core adoption.
//! Eventually this will be replaced by lapce_core's own selection structures.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caret {
    pub anchor: usize, // selection start
    pub head: usize,   // caret position (end)
}

impl Caret {
    pub fn new(pos: usize) -> Self { Self { anchor: pos, head: pos } }
    pub fn collapsed(&self) -> bool { self.anchor == self.head }
    pub fn range(&self) -> (usize, usize) { if self.anchor <= self.head { (self.anchor, self.head) } else { (self.head, self.anchor) } }
    pub fn set(&mut self, anchor: usize, head: usize) { self.anchor = anchor; self.head = head; }
}

#[derive(Debug, Default, Clone)]
pub struct MultiSelection {
    pub carets: Vec<Caret>, // primary caret is carets[0] if non-empty
}

impl MultiSelection {
    pub fn new() -> Self { Self { carets: Vec::new() } }
    pub fn clear(&mut self) { self.carets.clear(); }
    pub fn primary(&self) -> Option<&Caret> { self.carets.get(0) }
    pub fn primary_mut(&mut self) -> Option<&mut Caret> { self.carets.get_mut(0) }
    pub fn ensure_primary(&mut self, pos: usize) { if self.carets.is_empty() { self.carets.push(Caret::new(pos)); } }
    pub fn add_caret(&mut self, caret: Caret) { self.carets.push(caret); self.dedup_and_sort(); }
    pub fn add_collapsed(&mut self, pos: usize) { self.add_caret(Caret::new(pos)); }
    pub fn dedup_and_sort(&mut self) {
        self.carets.sort_by_key(|c| c.range());
        self.carets.dedup_by(|a,b| a.range()==b.range());
    }
    pub fn collapse_all(&mut self) { for c in &mut self.carets { c.anchor = c.head; } }
    pub fn apply_simple_insert(&mut self, at: usize, len: usize) {
        for c in &mut self.carets {
            if c.head >= at { c.head += len; }
            if c.anchor >= at { c.anchor += len; }
        }
    }
    pub fn apply_simple_delete(&mut self, at: usize, del_len: usize) {
        let end = at + del_len;
        for c in &mut self.carets {
            // If caret inside deleted span, move to start
            if c.head >= at && c.head < end { c.head = at; }
            if c.anchor >= at && c.anchor < end { c.anchor = at; }
            // Shift positions after deletion
            if c.head >= end { c.head -= del_len; }
            if c.anchor >= end { c.anchor -= del_len; }
        }
    }
}
