//! Chart hasil query (checklist C1).
//!
//! Bagian headless (`infer_column_kind`, `default_config`, `build_chart_data`) hanya menerima
//! header dan baris string sehingga bisa diuji tanpa GUI. Bagian UI (`render_result_chart`)
//! menggambar dengan `egui_plot` dan menyimpan pilihan pengguna di memori sementara egui.

use eframe::egui::{self, Color32};
use egui_plot::{Bar, BarChart, Legend, Line, Plot, PlotPoints, Points};
use std::collections::HashMap;

/// Batas baris yang diplot agar frame tetap ringan untuk hasil besar.
pub const MAX_CHART_ROWS: usize = 50_000;

/// Batas kategori pada sumbu X; sisanya diabaikan dan dilaporkan ke pengguna.
pub const MAX_CATEGORIES: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartKind {
    #[default]
    Bar,
    Line,
    Area,
    Scatter,
}

impl ChartKind {
    pub const ALL: [ChartKind; 4] = [Self::Bar, Self::Line, Self::Area, Self::Scatter];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bar => "Bar",
            Self::Line => "Line",
            Self::Area => "Area",
            Self::Scatter => "Scatter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aggregation {
    /// Satu titik per baris, tanpa pengelompokan.
    #[default]
    None,
    Sum,
    Avg,
    Count,
    Min,
    Max,
}

impl Aggregation {
    pub const ALL: [Aggregation; 6] = [
        Self::None,
        Self::Sum,
        Self::Avg,
        Self::Count,
        Self::Min,
        Self::Max,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Sum => "Sum",
            Self::Avg => "Average",
            Self::Count => "Count",
            Self::Min => "Min",
            Self::Max => "Max",
        }
    }
}

/// Jenis nilai sebuah kolom, ditebak dari sampel baris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Numeric,
    Temporal,
    Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartConfig {
    pub kind: ChartKind,
    pub x_col: usize,
    pub y_cols: Vec<usize>,
    pub aggregation: Aggregation,
}

/// Satu seri data yang siap diplot.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartSeries {
    pub name: String,
    pub points: Vec<[f64; 2]>,
}

/// Hasil `build_chart_data`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChartData {
    pub series: Vec<ChartSeries>,
    /// Label kategori bila sumbu X berupa teks; indeks label = nilai x.
    pub x_labels: Option<Vec<String>>,
    /// True bila x berupa detik epoch UTC.
    pub x_is_time: bool,
    /// Baris yang dilewati karena nilai X atau Y tidak terbaca.
    pub skipped_rows: usize,
    /// Kategori yang dibuang karena melewati `MAX_CATEGORIES`.
    pub dropped_categories: usize,
}

fn is_null(v: &str) -> bool {
    let t = v.trim();
    t.is_empty() || t.eq_ignore_ascii_case("null")
}

/// Membaca angka dari sel; menerima pemisah ribuan koma bila pola angkanya jelas.
pub fn parse_number(v: &str) -> Option<f64> {
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(n) = t.parse::<f64>() {
        return n.is_finite().then_some(n);
    }
    if t.contains(',') && !t.contains(' ') {
        let cleaned: String = t.chars().filter(|c| *c != ',').collect();
        if let Ok(n) = cleaned.parse::<f64>() {
            return n.is_finite().then_some(n);
        }
    }
    None
}

/// Membaca tanggal/waktu menjadi detik epoch (UTC untuk nilai tanpa zona).
pub fn parse_temporal(v: &str) -> Option<f64> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime};
    let t = v.trim();
    if t.len() < 8 || !t.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(t) {
        return Some(dt.timestamp_millis() as f64 / 1000.0);
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f %z", "%Y-%m-%d %H:%M:%S%.f%#z"] {
        if let Ok(dt) = DateTime::parse_from_str(t, fmt) {
            return Some(dt.timestamp_millis() as f64 / 1000.0);
        }
    }
    // Postgres TIMESTAMPTZ yang ditampilkan driver berakhiran " UTC".
    let naive_src = t.strip_suffix(" UTC").unwrap_or(t);
    for fmt in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(naive_src, fmt) {
            return Some(dt.and_utc().timestamp_millis() as f64 / 1000.0);
        }
    }
    for fmt in ["%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(d) = NaiveDate::parse_from_str(naive_src, fmt) {
            return d
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc().timestamp() as f64);
        }
    }
    None
}

/// Menebak jenis kolom dari maksimal 200 nilai non-NULL pertama.
pub fn infer_column_kind(rows: &[Vec<String>], col: usize) -> ColumnKind {
    let mut seen = 0usize;
    let mut numeric = 0usize;
    let mut temporal = 0usize;
    for row in rows {
        let Some(v) = row.get(col) else { continue };
        if is_null(v) {
            continue;
        }
        seen += 1;
        if parse_number(v).is_some() {
            numeric += 1;
        } else if parse_temporal(v).is_some() {
            temporal += 1;
        }
        if seen >= 200 {
            break;
        }
    }
    if seen == 0 {
        return ColumnKind::Text;
    }
    // Toleransi kecil untuk nilai kotor; mayoritas 90% menentukan jenis.
    if numeric * 10 >= seen * 9 {
        ColumnKind::Numeric
    } else if temporal * 10 >= seen * 9 {
        ColumnKind::Temporal
    } else {
        ColumnKind::Text
    }
}

/// Konfigurasi awal: X = kolom temporal/teks pertama, Y = kolom numerik lain (maks 3).
pub fn default_config(headers: &[String], rows: &[Vec<String>]) -> Option<ChartConfig> {
    if headers.is_empty() {
        return None;
    }
    let kinds: Vec<ColumnKind> = (0..headers.len())
        .map(|c| infer_column_kind(rows, c))
        .collect();
    let x_col = kinds
        .iter()
        .position(|k| *k == ColumnKind::Temporal)
        .or_else(|| kinds.iter().position(|k| *k == ColumnKind::Text))
        .unwrap_or(0);
    let y_cols: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(i, k)| *i != x_col && **k == ColumnKind::Numeric)
        .map(|(i, _)| i)
        .take(3)
        .collect();
    let x_kind = kinds[x_col];
    let kind = if x_kind == ColumnKind::Temporal {
        ChartKind::Line
    } else if x_kind == ColumnKind::Numeric && !y_cols.is_empty() {
        ChartKind::Scatter
    } else {
        ChartKind::Bar
    };
    // Tanpa kolom numerik, grafik yang masuk akal hanya hitungan per kategori.
    // Nilai X yang berulang (mis. beberapa region per bulan) dijumlahkan agar garis tidak zigzag.
    let aggregation = if y_cols.is_empty() {
        Aggregation::Count
    } else if x_kind != ColumnKind::Numeric && has_duplicate_values(rows, x_col) {
        Aggregation::Sum
    } else {
        Aggregation::None
    };
    Some(ChartConfig {
        kind,
        x_col,
        y_cols,
        aggregation,
    })
}

fn has_duplicate_values(rows: &[Vec<String>], col: usize) -> bool {
    let mut seen = std::collections::HashSet::new();
    rows.iter()
        .take(MAX_CHART_ROWS)
        .filter_map(|r| r.get(col))
        .any(|v| !seen.insert(v.trim()))
}

#[derive(Default, Clone, Copy)]
struct Acc {
    sum: f64,
    count: usize,
    min: f64,
    max: f64,
}

impl Acc {
    fn push(&mut self, v: f64) {
        if self.count == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.sum += v;
        self.count += 1;
    }

    fn value(&self, agg: Aggregation) -> f64 {
        match agg {
            Aggregation::Sum | Aggregation::None => self.sum,
            Aggregation::Avg => {
                if self.count == 0 {
                    0.0
                } else {
                    self.sum / self.count as f64
                }
            }
            Aggregation::Count => self.count as f64,
            Aggregation::Min => self.min,
            Aggregation::Max => self.max,
        }
    }
}

/// Mengubah hasil query menjadi seri chart sesuai konfigurasi.
pub fn build_chart_data(headers: &[String], rows: &[Vec<String>], cfg: &ChartConfig) -> ChartData {
    let mut out = ChartData::default();
    if cfg.x_col >= headers.len() {
        return out;
    }
    let rows = &rows[..rows.len().min(MAX_CHART_ROWS)];
    let x_kind = infer_column_kind(rows, cfg.x_col);
    out.x_is_time = x_kind == ColumnKind::Temporal;
    let categorical = x_kind == ColumnKind::Text;

    // Count tidak butuh kolom Y; seri tunggal bernama "count".
    let y_cols: Vec<Option<usize>> = if cfg.aggregation == Aggregation::Count {
        vec![None]
    } else {
        cfg.y_cols
            .iter()
            .copied()
            .filter(|c| *c < headers.len())
            .map(Some)
            .collect()
    };
    if y_cols.is_empty() {
        return out;
    }

    let mut labels: Vec<String> = Vec::new();
    let mut label_index: HashMap<String, usize> = HashMap::new();
    let mut dropped: std::collections::HashSet<String> = std::collections::HashSet::new();

    let group = cfg.aggregation != Aggregation::None;
    // Mode grup: kunci x (bit f64) -> akumulator, urutan kunci dijaga.
    let mut grouped: Vec<(Vec<u64>, HashMap<u64, Acc>)> = y_cols
        .iter()
        .map(|_| (Vec::new(), HashMap::new()))
        .collect();
    let mut plain: Vec<Vec<[f64; 2]>> = y_cols.iter().map(|_| Vec::new()).collect();

    for row in rows {
        let Some(raw_x) = row.get(cfg.x_col) else {
            out.skipped_rows += 1;
            continue;
        };
        let x = if categorical {
            let key = if is_null(raw_x) {
                "NULL".to_string()
            } else {
                raw_x.trim().to_string()
            };
            match label_index.get(&key) {
                Some(i) => Some(*i as f64),
                None if labels.len() < MAX_CATEGORIES => {
                    let i = labels.len();
                    label_index.insert(key.clone(), i);
                    labels.push(key);
                    Some(i as f64)
                }
                None => {
                    dropped.insert(key);
                    None
                }
            }
        } else if out.x_is_time {
            parse_temporal(raw_x)
        } else {
            parse_number(raw_x)
        };
        let Some(x) = x else {
            out.skipped_rows += 1;
            continue;
        };

        let mut row_ok = true;
        for (si, yc) in y_cols.iter().enumerate() {
            let y = match yc {
                None => Some(1.0),
                Some(c) => row.get(*c).and_then(|v| parse_number(v)),
            };
            let Some(y) = y else {
                row_ok = false;
                continue;
            };
            if group {
                let key = x.to_bits();
                let (order, map) = &mut grouped[si];
                map.entry(key)
                    .or_insert_with(|| {
                        order.push(key);
                        Acc::default()
                    })
                    .push(y);
            } else {
                plain[si].push([x, y]);
            }
        }
        if !row_ok {
            out.skipped_rows += 1;
        }
    }

    for (si, yc) in y_cols.iter().enumerate() {
        let mut points: Vec<[f64; 2]> = if group {
            let (order, map) = &grouped[si];
            order
                .iter()
                .map(|k| [f64::from_bits(*k), map[k].value(cfg.aggregation)])
                .collect()
        } else {
            std::mem::take(&mut plain[si])
        };
        // Garis dan area harus urut X; kategori dibiarkan sesuai urutan kemunculan.
        if !categorical && cfg.kind != ChartKind::Scatter {
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
        }
        let name = match yc {
            Some(i) => headers[*i].clone(),
            None => "count".to_string(),
        };
        out.series.push(ChartSeries { name, points });
    }

    if categorical {
        out.x_labels = Some(labels);
    }
    out.dropped_categories = dropped.len();
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// UI
// ─────────────────────────────────────────────────────────────────────────────

/// State UI yang disimpan di memori sementara egui, diikat ke tanda tangan header.
#[derive(Clone)]
struct ChartUiState {
    header_sig: u64,
    config: ChartConfig,
}

fn header_signature(headers: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    headers.hash(&mut h);
    h.finish()
}

/// Palet seri; kontras cukup di tema gelap maupun terang.
const SERIES_COLORS: [Color32; 8] = [
    Color32::from_rgb(66, 133, 244),
    Color32::from_rgb(234, 134, 51),
    Color32::from_rgb(52, 168, 83),
    Color32::from_rgb(217, 72, 95),
    Color32::from_rgb(142, 99, 206),
    Color32::from_rgb(0, 158, 170),
    Color32::from_rgb(196, 160, 0),
    Color32::from_rgb(120, 130, 145),
];

fn format_epoch(secs: f64, span_secs: f64) -> String {
    let Some(dt) = chrono::DateTime::from_timestamp(secs as i64, 0) else {
        return String::new();
    };
    if span_secs < 2.0 * 86_400.0 {
        dt.format("%m-%d %H:%M").to_string()
    } else if span_secs < 400.0 * 86_400.0 {
        dt.format("%Y-%m-%d").to_string()
    } else {
        dt.format("%Y-%m").to_string()
    }
}

/// Merender tab Chart untuk hasil query aktif.
pub fn render_result_chart(ui: &mut egui::Ui, headers: &[String], rows: &[Vec<String>]) {
    if headers.is_empty() || rows.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(egui::RichText::new("No rows to chart.").weak());
        });
        return;
    }

    let state_id = egui::Id::new("result_chart_state");
    let sig = header_signature(headers);
    let stored: Option<ChartUiState> = ui.data(|d| d.get_temp(state_id));
    let mut config = match stored {
        Some(s) if s.header_sig == sig => s.config,
        _ => match default_config(headers, rows) {
            Some(c) => c,
            None => return,
        },
    };

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                render_chart_controls(ui, headers, &mut config);
                ui.add_space(4.0);
                let data = build_chart_data(headers, rows, &config);
                render_chart_notes(ui, rows.len(), &data);
                render_chart_plot(ui, &config, &data);
            });
        });

    ui.data_mut(|d| {
        d.insert_temp(
            state_id,
            ChartUiState {
                header_sig: sig,
                config,
            },
        )
    });
}

fn render_chart_controls(ui: &mut egui::Ui, headers: &[String], config: &mut ChartConfig) {
    ui.horizontal_wrapped(|ui| {
        for kind in ChartKind::ALL {
            ui.selectable_value(&mut config.kind, kind, kind.label());
        }
        ui.separator();

        ui.label("X");
        egui::ComboBox::from_id_salt("result_chart_x")
            .selected_text(headers.get(config.x_col).cloned().unwrap_or_default())
            .width(160.0)
            .show_ui(ui, |ui| {
                for (i, h) in headers.iter().enumerate() {
                    ui.selectable_value(&mut config.x_col, i, h);
                }
            });

        ui.label("Y");
        let y_text = if config.aggregation == Aggregation::Count {
            "(row count)".to_string()
        } else if config.y_cols.is_empty() {
            "Select columns".to_string()
        } else {
            config
                .y_cols
                .iter()
                .filter_map(|c| headers.get(*c).map(String::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        };
        egui::ComboBox::from_id_salt("result_chart_y")
            .selected_text(y_text)
            .width(200.0)
            .show_ui(ui, |ui| {
                for (i, h) in headers.iter().enumerate() {
                    if i == config.x_col {
                        continue;
                    }
                    let mut on = config.y_cols.contains(&i);
                    if ui.checkbox(&mut on, h).changed() {
                        if on {
                            config.y_cols.push(i);
                            config.y_cols.sort_unstable();
                        } else {
                            config.y_cols.retain(|c| *c != i);
                        }
                    }
                }
            });

        ui.label("Aggregate");
        egui::ComboBox::from_id_salt("result_chart_agg")
            .selected_text(config.aggregation.label())
            .width(90.0)
            .show_ui(ui, |ui| {
                for agg in Aggregation::ALL {
                    ui.selectable_value(&mut config.aggregation, agg, agg.label());
                }
            });
    });
}

fn render_chart_notes(ui: &mut egui::Ui, total_rows: usize, data: &ChartData) {
    let mut notes: Vec<String> = Vec::new();
    if total_rows > MAX_CHART_ROWS {
        notes.push(format!("Plotting the first {} rows", MAX_CHART_ROWS));
    }
    if data.skipped_rows > 0 {
        notes.push(format!(
            "{} row(s) skipped (empty or non-numeric)",
            data.skipped_rows
        ));
    }
    if data.dropped_categories > 0 {
        notes.push(format!(
            "{} extra categories hidden (limit {})",
            data.dropped_categories, MAX_CATEGORIES
        ));
    }
    if !notes.is_empty() {
        ui.label(egui::RichText::new(notes.join(" · ")).small().weak());
    }
}

fn render_chart_plot(ui: &mut egui::Ui, config: &ChartConfig, data: &ChartData) {
    if data.series.iter().all(|s| s.points.is_empty()) {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(
                    "Nothing to plot. Pick a numeric Y column or use Count aggregation.",
                )
                .weak(),
            );
        });
        return;
    }

    let labels = data.x_labels.clone();
    let x_is_time = data.x_is_time;
    let (min_x, max_x) = data
        .series
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p[0]))
        .fold((f64::MAX, f64::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let span = (max_x - min_x).abs();

    let mut plot = Plot::new("result_chart_plot")
        .legend(Legend::default())
        .height(ui.available_height().max(160.0))
        .allow_scroll(false);
    if let Some(labels) = labels.clone() {
        plot = plot.x_axis_formatter(move |mark, _range| {
            let v = mark.value;
            if (v - v.round()).abs() > 1e-6 || v < 0.0 {
                return String::new();
            }
            labels.get(v as usize).cloned().unwrap_or_default()
        });
    } else if x_is_time {
        plot = plot.x_axis_formatter(move |mark, _range| format_epoch(mark.value, span));
    }

    let n_series = data.series.len().max(1) as f64;
    // Lebar bar mengikuti jarak antar-x agar seri berdampingan tidak saling tumpuk.
    let x_step = if labels.is_some() {
        1.0
    } else {
        min_spacing(&data.series).unwrap_or(1.0)
    };
    let bar_width = x_step * 0.8 / n_series;

    plot.show(ui, |plot_ui| {
        for (si, s) in data.series.iter().enumerate() {
            let color = SERIES_COLORS[si % SERIES_COLORS.len()];
            match config.kind {
                ChartKind::Bar => {
                    let offset = (si as f64 - (n_series - 1.0) / 2.0) * bar_width;
                    let bars: Vec<Bar> = s
                        .points
                        .iter()
                        .map(|p| Bar::new(p[0] + offset, p[1]).width(bar_width))
                        .collect();
                    plot_ui.add(BarChart::new(s.name.clone(), bars).color(color));
                }
                ChartKind::Line => {
                    plot_ui.add(
                        Line::new(s.name.clone(), PlotPoints::from(s.points.clone()))
                            .color(color)
                            .width(2.0),
                    );
                }
                ChartKind::Area => {
                    plot_ui.add(
                        Line::new(s.name.clone(), PlotPoints::from(s.points.clone()))
                            .color(color)
                            .width(1.5)
                            .fill(0.0)
                            .fill_alpha(0.25),
                    );
                }
                ChartKind::Scatter => {
                    plot_ui.add(
                        Points::new(s.name.clone(), PlotPoints::from(s.points.clone()))
                            .color(color)
                            .radius(3.0),
                    );
                }
            }
        }
    });
}

/// Jarak terkecil antar nilai x berbeda, untuk menentukan lebar bar.
fn min_spacing(series: &[ChartSeries]) -> Option<f64> {
    let mut xs: Vec<f64> = series
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p[0]))
        .collect();
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    xs.windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .min_by(f64::total_cmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn menebak_jenis_kolom() {
        let rows = vec![
            s(&["a", "1", "2024-01-01"]),
            s(&["b", "2.5", "2024-01-02 10:00:00"]),
            s(&["c", "NULL", "2024-01-03T11:00:00Z"]),
        ];
        assert_eq!(infer_column_kind(&rows, 0), ColumnKind::Text);
        assert_eq!(infer_column_kind(&rows, 1), ColumnKind::Numeric);
        assert_eq!(infer_column_kind(&rows, 2), ColumnKind::Temporal);
    }

    #[test]
    fn default_config_memilih_x_teks_dan_y_numerik() {
        let headers = s(&["id", "region", "total"]);
        let rows = vec![s(&["1", "EU", "10"]), s(&["2", "US", "20"])];
        let cfg = default_config(&headers, &rows).unwrap();
        assert_eq!(cfg.x_col, 1);
        assert_eq!(cfg.y_cols, vec![0, 2]);
        assert_eq!(cfg.kind, ChartKind::Bar);
        assert_eq!(cfg.aggregation, Aggregation::None);

        // X berulang -> Sum.
        let rows = vec![s(&["1", "EU", "10"]), s(&["2", "EU", "20"])];
        let cfg = default_config(&headers, &rows).unwrap();
        assert_eq!(cfg.aggregation, Aggregation::Sum);
    }

    #[test]
    fn agregasi_sum_per_kategori_menjaga_urutan() {
        let headers = s(&["region", "total"]);
        let rows = vec![
            s(&["EU", "10"]),
            s(&["US", "5"]),
            s(&["EU", "7"]),
            s(&["US", "x"]),
        ];
        let cfg = ChartConfig {
            kind: ChartKind::Bar,
            x_col: 0,
            y_cols: vec![1],
            aggregation: Aggregation::Sum,
        };
        let data = build_chart_data(&headers, &rows, &cfg);
        assert_eq!(data.x_labels, Some(s(&["EU", "US"])));
        assert_eq!(data.series[0].points, vec![[0.0, 17.0], [1.0, 5.0]]);
        assert_eq!(data.skipped_rows, 1);
    }

    #[test]
    fn count_tidak_butuh_kolom_y() {
        let headers = s(&["status"]);
        let rows = vec![s(&["ok"]), s(&["fail"]), s(&["ok"])];
        let cfg = ChartConfig {
            kind: ChartKind::Bar,
            x_col: 0,
            y_cols: vec![],
            aggregation: Aggregation::Count,
        };
        let data = build_chart_data(&headers, &rows, &cfg);
        assert_eq!(data.series.len(), 1);
        assert_eq!(data.series[0].name, "count");
        assert_eq!(data.series[0].points, vec![[0.0, 2.0], [1.0, 1.0]]);
    }

    #[test]
    fn line_temporal_diurutkan_dan_avg() {
        let headers = s(&["day", "v"]);
        let rows = vec![
            s(&["2024-01-02", "4"]),
            s(&["2024-01-01", "1"]),
            s(&["2024-01-02", "6"]),
        ];
        let cfg = ChartConfig {
            kind: ChartKind::Line,
            x_col: 0,
            y_cols: vec![1],
            aggregation: Aggregation::Avg,
        };
        let data = build_chart_data(&headers, &rows, &cfg);
        assert!(data.x_is_time);
        let pts = &data.series[0].points;
        assert_eq!(pts.len(), 2);
        assert!(pts[0][0] < pts[1][0]);
        assert_eq!(pts[0][1], 1.0);
        assert_eq!(pts[1][1], 5.0);
    }

    #[test]
    fn parse_number_menerima_pemisah_ribuan() {
        assert_eq!(parse_number("1,234.5"), Some(1234.5));
        assert_eq!(parse_number(" 42 "), Some(42.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("NaN"), None);
    }
}
