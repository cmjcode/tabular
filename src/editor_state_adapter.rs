//! Adapter layer to isolate direct interactions with `egui::text_edit::TextEditState`.
//! This allows phasing out egui's internal state when migrating to a custom
//! lapce-core powered editor widget.

use eframe::egui::{self, text_edit::TextEditState};
use egui::text::{CCursor, CCursorRange};

#[derive(Debug, Clone)]
pub struct AdapterCursorRange {
    pub start: usize,
    pub end: usize,
    pub primary: usize,
}

pub struct EditorStateAdapter;

impl EditorStateAdapter {
    pub fn get_range(ctx: &egui::Context, id: egui::Id) -> Option<AdapterCursorRange> {
        let state = TextEditState::load(ctx, id)?;
        let range = state.cursor.char_range()?;
        let primary = range.primary.index;
        let secondary = range.secondary.index;
        let start = primary.min(secondary);
        let end = primary.max(secondary);
        Some(AdapterCursorRange {
            start,
            end,
            primary,
        })
    }

    pub fn set_single(ctx: &egui::Context, id: egui::Id, pos: usize) {
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(pos))));
        state.store(ctx, id);
    }

    pub fn set_selection(
        ctx: &egui::Context,
        id: egui::Id,
        start: usize,
        end: usize,
        primary: usize,
    ) {
        let mut state = TextEditState::load(ctx, id).unwrap_or_default();
        let (p, s) = if primary == end {
            (end, start)
        } else if primary == start {
            (start, end)
        } else {
            (end, start)
        };
        let primary_cursor = CCursor::new(p);
        let secondary_cursor = CCursor::new(s);
        let mut c_range = CCursorRange::two(primary_cursor, secondary_cursor);
        c_range.primary = primary_cursor;
        c_range.secondary = secondary_cursor;
        state.cursor.set_char_range(Some(c_range));
        state.store(ctx, id);
    }
}
