//! View alternatif panel hasil: Chart (C1), Map (C4), dan Explain dengan riwayat plan (C2/C3).

use crate::models::structs::TableBottomView;
use crate::query_profiler::{self, CompareAction, CompareContext};
use crate::window_egui;
use eframe::egui;
use log::warn;

/// Merender view non-grid bila aktif. Mengembalikan true bila grid tidak perlu dirender.
pub(crate) fn render_alternate_result_view(
    tabular: &mut window_egui::Tabular,
    ui: &mut egui::Ui,
) -> bool {
    match tabular.table_bottom_view {
        TableBottomView::Chart => {
            let (headers, rows) = result_rows(tabular);
            let h = (ui.available_height() - pagination_reserve()).max(120.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), top_down(), |ui| {
                crate::result_chart::render_result_chart(ui, &headers, rows);
            });
            true
        }
        TableBottomView::Map => {
            let (headers, rows) = result_rows(tabular);
            let rows = rows.to_vec();
            let h = (ui.available_height() - pagination_reserve()).max(120.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), top_down(), |ui| {
                crate::geo_map::render_map_view(ui, &mut tabular.geo_map_state, &headers, &rows);
            });
            true
        }
        TableBottomView::Explain => {
            let raw = tabular
                .query_tabs
                .get(tabular.active_tab_index)
                .and_then(|t| t.explain_plan_json.clone());
            let Some(raw) = raw else {
                return false;
            };
            // Hasil JSON biasa juga bisa tertandai EXPLAIN oleh heuristik eksekusi;
            // tampilkan profiler hanya untuk query EXPLAIN atau plan yang terbaca.
            let sql = tabular
                .query_tabs
                .get(tabular.active_tab_index)
                .map(|t| t.last_executed_sql.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(tabular.last_executed_sql.as_str());
            if !looks_like_plan(sql, &raw) {
                return false;
            }
            let h = (ui.available_height() - pagination_reserve()).max(120.0);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), h), top_down(), |ui| {
                render_explain_with_history(tabular, ui, &raw);
            });
            true
        }
        _ => false,
    }
}

/// Query EXPLAIN, atau keluaran yang terbaca sebagai plan terstruktur (bukan fallback teks).
pub(crate) fn looks_like_plan(sql: &str, raw_plan: &str) -> bool {
    query_profiler::compare::is_explain_query(sql)
        || query_profiler::parse_explain(raw_plan)
            .is_some_and(|(_, s)| s.engine != query_profiler::ProfilerEngine::Generic)
}

fn top_down() -> egui::Layout {
    egui::Layout::top_down(egui::Align::LEFT)
}

/// Ruang untuk bar paginasi (tempat tombol Data/Chart/Map/Explain) di bawah view.
fn pagination_reserve() -> f32 {
    36.0
}

/// Header dan seluruh baris hasil (bukan hanya halaman aktif).
fn result_rows(tabular: &window_egui::Tabular) -> (Vec<String>, &[Vec<String>]) {
    let headers = tabular.current_table_headers.clone();
    let rows: &[Vec<String>] = if tabular.all_table_data.is_empty() {
        &tabular.current_table_data
    } else {
        &tabular.all_table_data
    };
    (headers, rows)
}

/// Apakah hasil aktif punya kolom geometry (untuk menampilkan tombol Map).
pub(crate) fn has_geometry_column(tabular: &window_egui::Tabular) -> bool {
    let rows = if tabular.current_table_data.is_empty() {
        &tabular.all_table_data
    } else {
        &tabular.current_table_data
    };
    // Cek murah dulu agar bar paginasi tidak mem-parse setiap frame tanpa perlu.
    let any_candidate = rows
        .iter()
        .take(20)
        .any(|r| r.iter().any(|v| crate::geo_map::looks_like_geometry(v)));
    any_candidate
        && !crate::geo_map::detect_geometry_columns(&tabular.current_table_headers, rows).is_empty()
}

fn explain_key(tabular: &window_egui::Tabular) -> Option<(i64, String)> {
    let tab = tabular.query_tabs.get(tabular.active_tab_index)?;
    let conn = tab.connection_id.or(tabular.current_connection_id)?;
    let sql = if tab.last_executed_sql.trim().is_empty() {
        tabular.last_executed_sql.as_str()
    } else {
        tab.last_executed_sql.as_str()
    };
    if sql.trim().is_empty() {
        return None;
    }
    Some((conn, query_profiler::compare::query_fingerprint(sql)))
}

fn reload_history(tabular: &mut window_egui::Tabular, key: (i64, String)) {
    let Some(pool) = tabular.db_pool.clone() else {
        return;
    };
    let rt = tabular.get_runtime();
    let (conn, hash) = key.clone();
    match rt.block_on(async move {
        query_profiler::history::list_plans(pool.as_ref(), conn, &hash).await
    }) {
        Ok(list) => tabular.explain_history.snapshots = list,
        Err(e) => {
            warn!("[EXPLAIN] gagal memuat riwayat plan: {e}");
            tabular.explain_history.snapshots.clear();
        }
    }
    tabular.explain_history.key = Some(key);
}

fn render_explain_with_history(tabular: &mut window_egui::Tabular, ui: &mut egui::Ui, raw: &str) {
    let key = explain_key(tabular);
    if let Some(k) = key.clone()
        && tabular.explain_history.key.as_ref() != Some(&k)
    {
        reload_history(tabular, k);
    }

    let action = if key.is_some() && tabular.db_pool.is_some() {
        let ctx = CompareContext {
            snapshots: &tabular.explain_history.snapshots,
            current_id: tabular.explain_history.current_id(raw),
        };
        query_profiler::render_query_profiler_with_history(ui, raw, Some(ctx))
    } else {
        query_profiler::render_query_profiler_with_history(ui, raw, None)
    };

    if let Some(CompareAction::SetPinned { id, pinned }) = action
        && let Some(pool) = tabular.db_pool.clone()
    {
        let rt = tabular.get_runtime();
        match rt.block_on(async move {
            query_profiler::history::set_pinned(pool.as_ref(), id, pinned).await
        }) {
            Ok(()) => tabular.explain_history.invalidate(),
            Err(e) => {
                warn!("[EXPLAIN] gagal mengubah pin plan: {e}");
                tabular
                    .toasts
                    .error(format!("Could not update pinned plan: {e}"));
            }
        }
    }
}

/// Menyimpan plan EXPLAIN yang baru dijalankan ke riwayat (dipanggil setelah eksekusi).
pub(crate) fn record_explain_plan(
    tabular: &mut window_egui::Tabular,
    connection_id: i64,
    query: &str,
    raw_plan: &str,
) {
    let Some(pool) = tabular.db_pool.clone() else {
        return;
    };
    let rt = tabular.get_runtime();
    let query = query.to_string();
    let raw = raw_plan.to_string();
    if let Err(e) = rt.block_on(async move {
        query_profiler::history::record_plan(pool.as_ref(), connection_id, &query, &raw).await
    }) {
        warn!("[EXPLAIN] gagal menyimpan riwayat plan: {e}");
    }
    tabular.explain_history.invalidate();
}

/// Tombol view di bar paginasi, bergaya sama dengan tombol Data/Explain.
pub(crate) fn render_view_toggle(
    tabular: &mut window_egui::Tabular,
    ui: &mut egui::Ui,
    view: TableBottomView,
    label: String,
    button_height: f32,
) {
    let active = tabular.table_bottom_view == view
        && !tabular.show_message_panel
        && !tabular.show_lint_panel;
    let bg = if active {
        window_egui::style::theme_accent(ui.ctx())
    } else if ui.visuals().dark_mode {
        egui::Color32::from_rgb(45, 45, 50)
    } else {
        egui::Color32::from_rgb(225, 225, 230)
    };
    let fg = if active {
        egui::Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    let btn = egui::Button::new(egui::RichText::new(label).small().strong().color(fg))
        .fill(bg)
        .corner_radius(egui::CornerRadius::same(4u8))
        .min_size(egui::vec2(0.0, button_height));
    if ui.add(btn).clicked() {
        tabular.table_bottom_view = view;
        tabular.show_message_panel = false;
        tabular.show_lint_panel = false;
    }
}
