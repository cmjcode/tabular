//! Panel "Index Check": analisis index untuk statement di posisi kursor.
//!
//! Terpisah dari popup autocomplete. Saat kursor diam sebentar di sebuah
//! statement, statement itu dianalisis (kolom JOIN/WHERE/GROUP BY/ORDER BY
//! terhadap primary key, foreign key, dan index yang di-cache). Bila ada yang
//! bisa dioptimalkan, panel detail muncul di bawah baris kursor lengkap dengan
//! penjelasan dan SQL perbaikan; bila sudah optimal cukup badge kecil.
//! Analisis lebih dalam (diagram alur + AI) ada di panel Query Insight.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::autocomplete::{AdviceLevel, ColumnStatus, QueryReport};
use crate::editor_autocomplete::{self, ReportStatus};
use crate::window_egui::Tabular;

/// Jeda setelah mengetik / memindah kursor sebelum statement dianalisis.
const IDLE_DELAY: Duration = Duration::from_millis(600);
/// Interval coba lagi saat metadata masih dimuat.
const RETRY_DELAY: Duration = Duration::from_millis(300);
const PANEL_WIDTH: f32 = 500.0;

pub struct IndexCheckState {
    /// Tampilkan panel otomatis bila statement bisa dioptimalkan.
    pub auto_show: bool,
    /// (tab, revision, panjang teks) terakhir; beda berarti teks berubah.
    text_key: (usize, u64, usize),
    cursor: usize,
    changed_at: Instant,
    /// Statement yang sudah dianalisis: rentang byte + hash isinya.
    stmt: Option<(Range<usize>, u64)>,
    report: Option<QueryReport>,
    /// Metadata sedang dimuat; analisis diulang setelah `RETRY_DELAY`.
    retry_at: Option<Instant>,
    open: bool,
    /// Statement yang panelnya ditutup user (tidak dibuka otomatis lagi).
    dismissed: Option<u64>,
}

impl Default for IndexCheckState {
    fn default() -> Self {
        IndexCheckState {
            auto_show: true,
            text_key: (usize::MAX, 0, 0),
            cursor: usize::MAX,
            changed_at: Instant::now(),
            stmt: None,
            report: None,
            retry_at: None,
            open: false,
            dismissed: None,
        }
    }
}

fn hash_text(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Statement cukup berarti untuk dianalisis (punya klausa yang memakai kolom).
fn worth_analyzing(stmt: &str) -> bool {
    let upper = stmt.to_ascii_uppercase();
    ["WHERE", "JOIN", "GROUP BY", "ORDER BY"]
        .iter()
        .any(|k| upper.contains(k))
}

/// Perbarui state: deteksi perubahan, tunggu kursor diam, lalu analisis.
fn update(app: &mut Tabular, ctx: &egui::Context) {
    let Some(dialect) = editor_autocomplete::active_dialect(app) else {
        app.index_check.report = None;
        app.index_check.open = false;
        return;
    };
    let now = Instant::now();
    let text_key = (
        app.active_tab_index,
        app.editor.revision,
        app.editor.text.len(),
    );
    let cursor = app.cursor_position.min(app.editor.text.len());
    let st = &mut app.index_check;
    if st.text_key != text_key {
        // Teks berubah: sembunyikan panel selama user mengetik
        st.text_key = text_key;
        st.cursor = cursor;
        st.changed_at = now;
        st.open = false;
        st.stmt = None;
        st.report = None;
        st.retry_at = None;
    } else if st.cursor != cursor {
        st.cursor = cursor;
        st.changed_at = now;
        // Kursor keluar dari statement yang dianalisis → analisis ulang nanti
        if st
            .stmt
            .as_ref()
            .is_some_and(|(r, _)| cursor < r.start || cursor > r.end)
        {
            st.stmt = None;
            st.report = None;
            st.open = false;
        }
    }

    let waited = now.saturating_duration_since(st.changed_at);
    if waited < IDLE_DELAY {
        ctx.request_repaint_after(IDLE_DELAY - waited);
        return;
    }
    let retry_due = st.retry_at.is_some_and(|t| now >= t);
    if st.stmt.is_some() && !retry_due {
        if let Some(t) = st.retry_at {
            ctx.request_repaint_after(t.saturating_duration_since(now));
        }
        return;
    }

    let text = &app.editor.text;
    let (s0, s1) = crate::autocomplete::lexer::statement_bounds(text, cursor, dialect);
    let raw = &text[s0..s1];
    // Hitung dari hasil trim agar statement berisi whitespace saja tidak
    // menghasilkan range terbalik (start > end)
    let lead = raw.len() - raw.trim_start().len();
    let range = s0 + lead..s0 + lead + raw.trim().len();
    let stmt_text = text[range.clone()].to_string();
    let hash = hash_text(&stmt_text);
    if stmt_text.is_empty() || !worth_analyzing(&stmt_text) {
        let st = &mut app.index_check;
        st.stmt = Some((range, hash));
        st.report = None;
        st.retry_at = None;
        return;
    }
    let status = editor_autocomplete::statement_report(app, &stmt_text);
    let base = range.start;
    let st = &mut app.index_check;
    st.stmt = Some((range, hash));
    match status {
        ReportStatus::Unavailable => {
            st.report = None;
            st.retry_at = None;
        }
        ReportStatus::Loading => {
            st.retry_at = Some(now + RETRY_DELAY);
            ctx.request_repaint_after(RETRY_DELAY);
        }
        ReportStatus::Ready(mut report) => {
            // Span relatif ke statement → offset absolut di editor
            for u in &mut report.usages {
                u.span = u.span.start + base..u.span.end + base;
            }
            for a in &mut report.advice {
                if let Some(sp) = &mut a.span {
                    *sp = sp.start + base..sp.end + base;
                }
            }
            st.retry_at = None;
            st.open = st.auto_show && !report.is_optimal() && st.dismissed != Some(hash);
            st.report = (!report.is_empty()).then_some(report);
        }
    }
}

/// Dipanggil tiap frame dari editor. `anchor` = kiri-bawah baris kursor,
/// `editor_rect` = area editor (untuk badge di pojok kanan atas).
pub fn show(app: &mut Tabular, ui: &mut egui::Ui, anchor: egui::Pos2, editor_rect: egui::Rect) {
    update(app, ui.ctx());
    if app.index_check.report.is_none() {
        return;
    }
    // Autocomplete punya prioritas; panel menunggu sampai popup itu tutup
    if app.show_autocomplete {
        return;
    }
    if app.index_check.open && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        close(app);
    }
    render_badge(app, ui, editor_rect);
    if app.index_check.open {
        render_panel(app, ui, anchor);
    }
}

fn close(app: &mut Tabular) {
    let st = &mut app.index_check;
    st.open = false;
    st.dismissed = st.stmt.as_ref().map(|(_, h)| *h);
}

fn render_badge(app: &mut Tabular, ui: &mut egui::Ui, editor_rect: egui::Rect) {
    let Some(report) = app.index_check.report.as_ref() else {
        return;
    };
    let warnings = report
        .advice
        .iter()
        .filter(|a| a.level == AdviceLevel::Warning)
        .count();
    let (text, color) = if warnings > 0 {
        let plural = if warnings == 1 { "" } else { "s" };
        (
            format!("⚠ {warnings} index issue{plural}"),
            crate::window_egui::style::theme_warning(ui.ctx()),
        )
    } else if !report.missing_metadata.is_empty() {
        // Tanpa metadata index tidak bisa dinilai; jangan klaim "OK"
        (
            "ℹ Index info unavailable".to_string(),
            crate::window_egui::style::theme_info(ui.ctx()),
        )
    } else {
        (
            "✅ Indexes OK".to_string(),
            crate::window_egui::style::theme_success(ui.ctx()),
        )
    };
    let pos = egui::pos2(editor_rect.right() - 12.0, editor_rect.top() + 6.0);
    let mut clicked = false;
    egui::Area::new(egui::Id::new("index_check_badge"))
        .fixed_pos(pos)
        .pivot(egui::Align2::RIGHT_TOP)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let resp = ui
                .add(
                    egui::Button::new(egui::RichText::new(&text).small().color(color))
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .corner_radius(egui::CornerRadius::same(10u8)),
                )
                .on_hover_text("Index Check: how the statement at the cursor uses indexes");
            clicked = resp.clicked();
        });
    if clicked {
        let st = &mut app.index_check;
        st.open = !st.open;
        st.dismissed = if st.open {
            None
        } else {
            st.stmt.as_ref().map(|(_, h)| *h)
        };
    }
}

/// Teks status kolom + apakah bagus (`None` = belum diketahui).
pub(crate) fn status_text(status: &ColumnStatus) -> (String, Option<bool>) {
    match status {
        ColumnStatus::PrimaryKey => ("Primary key".into(), Some(true)),
        ColumnStatus::Indexed { index, unique } => {
            let u = if *unique { " (unique)" } else { "" };
            (format!("Index {index}{u}"), Some(true))
        }
        ColumnStatus::NotLeading { index, position } => (
            format!("Column #{position} of {index}: not usable alone"),
            Some(false),
        ),
        ColumnStatus::ForeignKeyNoIndex { references } => {
            (format!("FK to {references}, no index"), Some(false))
        }
        ColumnStatus::NotIndexed => ("No index".into(), Some(false)),
        ColumnStatus::Unknown => ("Index metadata not cached".into(), None),
    }
}

/// Penjelasan umum di bagian "Why this matters".
const WHY_LINES: &[&str] = &[
    "An index is only used when the column appears unmodified in the condition and is the first column of the index (or follows earlier index columns that are also filtered).",
    "JOIN columns on the looked-up side need an index; otherwise every row of the other table triggers a full scan.",
    "Foreign keys are not indexed automatically in PostgreSQL, SQLite or SQL Server.",
    "Composite indexes work best ordered by equality filters, then ORDER BY columns, then range filters.",
    "Recommendations use cached metadata only; table size is unknown, so small tables may not need an index.",
];

fn render_panel(app: &mut Tabular, ui: &mut egui::Ui, anchor: egui::Pos2) {
    let Some(report) = app.index_check.report.clone() else {
        return;
    };
    let ctx = ui.ctx().clone();
    let warn = crate::window_egui::style::theme_warning(&ctx);
    let ok = crate::window_egui::style::theme_success(&ctx);
    let info = crate::window_egui::style::theme_info(&ctx);

    let screen = ctx.content_rect();
    let mut pos = anchor;
    if pos.x + PANEL_WIDTH > screen.right() - 8.0 {
        pos.x = (screen.right() - PANEL_WIDTH - 8.0).max(screen.left());
    }
    let max_h = (screen.bottom() - pos.y - 90.0).clamp(140.0, 420.0);

    let mut close_clicked = false;
    let mut open_diagram = false;
    let mut auto_show = app.index_check.auto_show;
    egui::Area::new(egui::Id::new("index_check_panel"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(&ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(PANEL_WIDTH);
                    ui.horizontal(|ui| {
                        let (title, color) = if !report.is_optimal() {
                            ("⚠ Query can be optimized", warn)
                        } else if !report.missing_metadata.is_empty() {
                            ("ℹ Index usage can't be fully checked", info)
                        } else {
                            ("✅ Query uses indexes well", ok)
                        };
                        ui.label(egui::RichText::new(title).strong().color(color));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::window_egui::style::render_close_icon_button(ui).clicked() {
                                close_clicked = true;
                            }
                        });
                    });
                    let n = report.usages.len();
                    ui.label(
                        egui::RichText::new(format!(
                            "Statement at cursor · {n} column{} checked against primary keys, foreign keys and indexes",
                            if n == 1 { "" } else { "s" }
                        ))
                        .small()
                        .weak(),
                    );
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .max_height(max_h)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            render_columns(ui, &report, warn, ok, info);
                            render_advice(ui, &report, warn, info);
                            for t in &report.missing_metadata {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "ℹ Index metadata for `{t}` isn't cached yet. Refresh the connection to include it."
                                    ))
                                    .small()
                                    .color(info),
                                );
                            }
                            egui::CollapsingHeader::new(
                                egui::RichText::new("Why this matters").small(),
                            )
                            .id_salt("index_check_why")
                            .show(ui, |ui| {
                                for line in WHY_LINES {
                                    ui.label(egui::RichText::new(format!("• {line}")).small());
                                }
                            });
                        });

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut auto_show, egui::RichText::new("Show automatically").small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new("Esc to close").small().weak());
                            if ui
                                .small_button("Query diagram")
                                .on_hover_text("Open the animated query diagram with AI suggestions")
                                .clicked()
                            {
                                open_diagram = true;
                            }
                        });
                    });
                });
        });
    app.index_check.auto_show = auto_show;
    if close_clicked {
        close(app);
    }
    if open_diagram {
        close(app);
        crate::window_egui::query_insight::open_query_insight(app);
    }
}

fn render_columns(
    ui: &mut egui::Ui,
    report: &QueryReport,
    warn: egui::Color32,
    ok: egui::Color32,
    info: egui::Color32,
) {
    if report.usages.is_empty() {
        return;
    }
    ui.label(egui::RichText::new("Columns").strong());
    egui::Grid::new("index_check_columns")
        .num_columns(3)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for u in &report.usages {
                ui.label(
                    egui::RichText::new(format!("{}.{}", u.table, u.column))
                        .family(egui::FontFamily::Monospace),
                );
                ui.label(egui::RichText::new(u.used_in).small().weak());
                let (text, good) = status_text(&u.status);
                let (icon, color) = match (good, u.defeated_by.is_some()) {
                    (_, true) | (Some(false), _) => ("⚠", warn),
                    (Some(true), false) => ("✅", ok),
                    (None, false) => ("ℹ", info),
                };
                let text = match &u.defeated_by {
                    Some(why) => format!("{text}, but {why}: index not used"),
                    None => text,
                };
                ui.label(
                    egui::RichText::new(format!("{icon} {text}"))
                        .small()
                        .color(color),
                );
                ui.end_row();
            }
        });
    ui.add_space(6.0);
}

fn render_advice(
    ui: &mut egui::Ui,
    report: &QueryReport,
    warn: egui::Color32,
    info: egui::Color32,
) {
    if report.advice.is_empty() {
        return;
    }
    ui.label(egui::RichText::new("Recommendations").strong());
    for a in &report.advice {
        let (icon, color) = match a.level {
            AdviceLevel::Warning => ("⚠", warn),
            AdviceLevel::Info => ("ℹ", info),
        };
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(icon).color(color));
            ui.label(egui::RichText::new(&a.message).strong());
        });
        if let Some(h) = &a.hint {
            ui.label(egui::RichText::new(h).small().weak());
        }
        if let Some(ddl) = &a.ddl {
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(ddl)
                            .small()
                            .family(egui::FontFamily::Monospace),
                    )
                    .truncate(),
                )
                .on_hover_text(ddl);
                if ui.small_button("Copy SQL").clicked() {
                    ui.ctx().copy_text(ddl.clone());
                }
            });
        }
    }
    ui.add_space(6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_statements_with_column_clauses_are_analyzed() {
        assert!(worth_analyzing("select * from t where a = 1"));
        assert!(worth_analyzing("SELECT * FROM a JOIN b ON a.id = b.a_id"));
        assert!(worth_analyzing("SELECT x, count(*) FROM t GROUP BY x"));
        assert!(!worth_analyzing("SELECT * FROM t"));
        assert!(!worth_analyzing("INSERT INTO t VALUES (1)"));
    }

    #[test]
    fn status_text_marks_good_and_bad() {
        assert_eq!(status_text(&ColumnStatus::PrimaryKey).1, Some(true));
        assert_eq!(status_text(&ColumnStatus::NotIndexed).1, Some(false));
        assert_eq!(status_text(&ColumnStatus::Unknown).1, None);
        let (t, g) = status_text(&ColumnStatus::NotLeading {
            index: "ix".into(),
            position: 2,
        });
        assert!(t.contains("#2 of ix") && g == Some(false));
    }
}
