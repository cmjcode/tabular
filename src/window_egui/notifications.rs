//! Centralized toast / notification system.
//!
//! Replaces the scattered `error_message` + `show_error_message` dialog pattern
//! (which was also misused for *success* messages) with a single, non-blocking,
//! auto-dismissing, stackable toast surface anchored to the top-right corner.
//!
//! Usage from anywhere on `Tabular`:
//! ```ignore
//! self.toasts.success("Connection saved");
//! self.toasts.error(format!("Query failed: {e}"));
//! ```
//! and once per frame in `update()`:
//! ```ignore
//! self.toasts.show(ctx);
//! ```

use eframe::egui;
use std::time::{Duration, Instant};

/// Maximum number of toasts kept on screen at once. Oldest non-error toasts are
/// evicted first so the stack never grows without bound.
const MAX_TOASTS: usize = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Success,
    Error,
    Warning,
    Info,
}

impl ToastKind {
    fn icon(self) -> &'static str {
        match self {
            ToastKind::Success => "✅",
            ToastKind::Error => "⛔",
            ToastKind::Warning => "⚠",
            ToastKind::Info => "ℹ",
        }
    }

    fn accent(self) -> egui::Color32 {
        match self {
            ToastKind::Success => egui::Color32::from_rgb(34, 160, 90),
            ToastKind::Error => egui::Color32::from_rgb(200, 64, 64),
            ToastKind::Warning => egui::Color32::from_rgb(200, 150, 40),
            ToastKind::Info => egui::Color32::from_rgb(70, 120, 200),
        }
    }

    /// Default time a toast stays before auto-dismissing. Errors are sticky so
    /// the user never misses a failure; everything else fades out.
    fn default_ttl(self) -> Option<Duration> {
        match self {
            ToastKind::Error => None,
            ToastKind::Warning => Some(Duration::from_secs(7)),
            _ => Some(Duration::from_secs(4)),
        }
    }
}

struct Toast {
    kind: ToastKind,
    message: String,
    created: Instant,
    ttl: Option<Duration>,
}

impl Toast {
    fn remaining(&self, now: Instant) -> Option<Duration> {
        self.ttl
            .map(|ttl| ttl.saturating_sub(now.saturating_duration_since(self.created)))
    }

    fn expired(&self, now: Instant) -> bool {
        self.remaining(now).is_some_and(|r| r.is_zero())
    }
}

#[derive(Default)]
pub struct ToastManager {
    toasts: Vec<Toast>,
}

impl ToastManager {
    pub fn push(&mut self, kind: ToastKind, message: impl Into<String>) {
        let message = message.into();
        if message.trim().is_empty() {
            return;
        }
        // De-duplicate identical messages that are already visible (avoids
        // spamming the same error every frame from a polling loop).
        if self
            .toasts
            .iter()
            .any(|t| t.kind == kind && t.message == message)
        {
            return;
        }
        self.toasts.push(Toast {
            kind,
            message,
            created: Instant::now(),
            ttl: kind.default_ttl(),
        });
        // Evict oldest dismissible toast(s) when over capacity.
        while self.toasts.len() > MAX_TOASTS {
            let idx = self
                .toasts
                .iter()
                .position(|t| t.ttl.is_some())
                .unwrap_or(0);
            self.toasts.remove(idx);
        }
    }

    pub fn success(&mut self, message: impl Into<String>) {
        self.push(ToastKind::Success, message);
    }

    pub fn error(&mut self, message: impl Into<String>) {
        self.push(ToastKind::Error, message);
    }

    pub fn warning(&mut self, message: impl Into<String>) {
        self.push(ToastKind::Warning, message);
    }

    pub fn info(&mut self, message: impl Into<String>) {
        self.push(ToastKind::Info, message);
    }

    /// Render the toast stack and drop expired entries. Call once per frame.
    pub fn show(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        self.toasts.retain(|t| !t.expired(now));
        if self.toasts.is_empty() {
            return;
        }

        // Keep animating auto-dismiss timers even when nothing else repaints.
        if let Some(soonest) = self.toasts.iter().filter_map(|t| t.remaining(now)).min() {
            ctx.request_repaint_after(soonest);
        }

        let mut dismiss: Option<usize> = None;
        egui::Area::new(egui::Id::new("tabular_toasts"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 56.0))
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                for (idx, toast) in self.toasts.iter().enumerate() {
                    let accent = toast.kind.accent();
                    let frame = egui::Frame::new()
                        .fill(ui.visuals().panel_fill)
                        .stroke(egui::Stroke::new(1.0, accent))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .outer_margin(egui::Margin {
                            bottom: 8,
                            ..Default::default()
                        });
                    frame.show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.label(
                                egui::RichText::new(toast.kind.icon())
                                    .color(accent)
                                    .strong(),
                            );
                            ui.add(
                                egui::Label::new(egui::RichText::new(&toast.message).size(13.0))
                                    .wrap(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::TOP),
                                |ui| {
                                    if super::style::render_close_icon_button(ui).clicked() {
                                        dismiss = Some(idx);
                                    }
                                },
                            );
                        });
                    });
                }
            });

        if let Some(idx) = dismiss
            && idx < self.toasts.len()
        {
            self.toasts.remove(idx);
        }
    }
}
