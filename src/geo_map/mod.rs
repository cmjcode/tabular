//! Map view untuk kolom geometry/PostGIS di hasil query (checklist C4).
//!
//! Data geografis (lon/lat, atau Web Mercator yang dikonversi) digambar di atas peta slippy
//! `walkers` dengan basemap OpenStreetMap opsional. Koordinat proyeksi lain digambar apa
//! adanya di bidang datar memakai `egui_plot`.

pub mod parse;

pub use parse::{Coord, Geometry, Shape, looks_like_geometry, parse_geometry};

use eframe::egui::{self, Color32, Pos2, Stroke};
use std::hash::{Hash, Hasher};
use walkers::{HttpOptions, HttpTiles, Map, MapMemory, Plugin, Projector, lon_lat};

/// Batas feature yang digambar per hasil query.
pub const MAX_FEATURES: usize = 20_000;

/// Ring dengan vertex lebih banyak dari ini hanya digambar garis tepinya.
const MAX_FILL_VERTICES: usize = 2_000;

/// Deteksi kolom geometry: minimal 80% dari sampel non-NULL harus terbaca sebagai geometry.
pub fn detect_geometry_columns(headers: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    (0..headers.len())
        .filter(|&c| {
            let mut seen = 0usize;
            let mut ok = 0usize;
            for row in rows.iter().take(200) {
                let Some(v) = row.get(c) else { continue };
                let t = v.trim();
                if t.is_empty() || crate::models::structs::is_null_cell(v) {
                    continue;
                }
                seen += 1;
                // Nilai pertama yang jelas bukan geometry menggugurkan kolom dengan cepat.
                if seen == 1 && !looks_like_geometry(t) {
                    return false;
                }
                if parse_geometry(t).is_some() {
                    ok += 1;
                }
                if seen >= 25 {
                    break;
                }
            }
            seen > 0 && ok * 5 >= seen * 4
        })
        .collect()
}

/// Ruang koordinat hasil akhir layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordSpace {
    /// x = longitude, y = latitude (WGS84).
    Geographic,
    /// Koordinat proyeksi yang tidak dikenali; digambar tanpa basemap.
    Planar,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub row: usize,
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeoLayer {
    pub features: Vec<Feature>,
    pub space: CoordSpace,
    /// Baris dengan nilai kosong atau tidak terbaca.
    pub skipped: usize,
    /// SRID yang paling sering muncul.
    pub srid: Option<i32>,
    /// [min_x, min_y, max_x, max_y]
    pub bbox: Option<[f64; 4]>,
}

const WEB_MERCATOR_SRIDS: [i32; 4] = [3857, 900913, 102100, 102113];
const GEOGRAPHIC_SRIDS: [i32; 5] = [4326, 4269, 4258, 4979, 4230];

/// Web Mercator (meter) ke lon/lat.
pub fn mercator_to_lon_lat(c: Coord) -> Coord {
    const R: f64 = 6_378_137.0;
    let lon = c[0] / R * 180.0 / std::f64::consts::PI;
    let lat = (2.0 * (c[1] / R).exp().atan() - std::f64::consts::FRAC_PI_2) * 180.0
        / std::f64::consts::PI;
    [lon, lat]
}

/// Mem-parse kolom `col` menjadi layer siap gambar.
pub fn build_layer(rows: &[Vec<String>], col: usize, swap_xy: bool) -> GeoLayer {
    let mut features = Vec::new();
    let mut skipped = 0usize;
    let mut srid_counts: std::collections::HashMap<i32, usize> = Default::default();
    for (i, row) in rows.iter().enumerate() {
        if features.len() >= MAX_FEATURES {
            skipped += rows.len() - i;
            break;
        }
        let Some(mut g) = row.get(col).and_then(|v| parse_geometry(v)) else {
            skipped += 1;
            continue;
        };
        if let Some(s) = g.srid {
            *srid_counts.entry(s).or_default() += 1;
        }
        if g.srid.is_some_and(|s| WEB_MERCATOR_SRIDS.contains(&s)) {
            g.shape.map_coords(&mercator_to_lon_lat);
        }
        if swap_xy {
            g.shape.map_coords(&|c| [c[1], c[0]]);
        }
        features.push(Feature {
            row: i,
            shape: g.shape,
        });
    }
    let srid = srid_counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(s, _)| s);

    let mut bbox: Option<[f64; 4]> = None;
    for f in &features {
        f.shape.for_each_coord(&mut |c| {
            if !(c[0].is_finite() && c[1].is_finite()) {
                return;
            }
            let b = bbox.get_or_insert([c[0], c[1], c[0], c[1]]);
            b[0] = b[0].min(c[0]);
            b[1] = b[1].min(c[1]);
            b[2] = b[2].max(c[0]);
            b[3] = b[3].max(c[1]);
        });
    }
    let in_lon_lat_range =
        bbox.is_none_or(|b| b[0] >= -180.5 && b[2] <= 180.5 && b[1] >= -90.5 && b[3] <= 90.5);
    let known_projected = srid.is_some_and(|s| {
        s != 0 && !GEOGRAPHIC_SRIDS.contains(&s) && !WEB_MERCATOR_SRIDS.contains(&s)
    });
    let space = if in_lon_lat_range && !known_projected {
        CoordSpace::Geographic
    } else {
        CoordSpace::Planar
    };
    GeoLayer {
        features,
        space,
        skipped,
        srid,
        bbox,
    }
}

/// Pusat dan zoom peta agar bbox lon/lat muat di viewport berukuran `size` piksel.
pub fn fit_view(bbox: [f64; 4], size: egui::Vec2) -> (Coord, f64) {
    fn merc_y(lat: f64) -> f64 {
        let lat = lat.clamp(-85.0511, 85.0511).to_radians();
        (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0
    }
    let center = [(bbox[0] + bbox[2]) / 2.0, (bbox[1] + bbox[3]) / 2.0];
    let span_x = ((bbox[2] - bbox[0]) / 360.0).abs();
    let span_y = (merc_y(bbox[1]) - merc_y(bbox[3])).abs();
    if span_x < 1e-9 && span_y < 1e-9 {
        return (center, 15.0);
    }
    let w = size.x.max(64.0) as f64 * 0.85;
    let h = size.y.max(64.0) as f64 * 0.85;
    let zx = if span_x > 1e-12 {
        (w / (256.0 * span_x)).log2()
    } else {
        18.0
    };
    let zy = if span_y > 1e-12 {
        (h / (256.0 * span_y)).log2()
    } else {
        18.0
    };
    (center, zx.min(zy).clamp(1.0, 18.0))
}

// ─────────────────────────────────────────────────────────────────────────────
// UI
// ─────────────────────────────────────────────────────────────────────────────

/// State Map view yang hidup selama aplikasi berjalan (di `Tabular`).
pub struct MapViewState {
    memory: MapMemory,
    tiles: Option<HttpTiles>,
    /// Basemap OpenStreetMap; tile diunduh dari tile.openstreetmap.org.
    pub basemap: bool,
    geom_col: Option<usize>,
    swap_xy: bool,
    layer: Option<(u64, GeoLayer)>,
    fitted_sig: Option<u64>,
    fit_requested: bool,
}

impl Default for MapViewState {
    fn default() -> Self {
        Self {
            memory: MapMemory::default(),
            tiles: None,
            basemap: true,
            geom_col: None,
            swap_xy: false,
            layer: None,
            fitted_sig: None,
            fit_requested: false,
        }
    }
}

fn data_signature(headers: &[String], rows: &[Vec<String>], col: usize, swap: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    headers.hash(&mut h);
    rows.len().hash(&mut h);
    col.hash(&mut h);
    swap.hash(&mut h);
    for row in rows.iter().take(3).chain(rows.last()) {
        row.get(col).hash(&mut h);
    }
    h.finish()
}

fn tiles_for(ctx: &egui::Context) -> HttpTiles {
    let options = HttpOptions {
        cache: Some(crate::config::get_data_dir().join("tile_cache")),
        user_agent: Some(walkers::HeaderValue::from_static(concat!(
            "tabular/",
            env!("CARGO_PKG_VERSION")
        ))),
        ..Default::default()
    };
    HttpTiles::with_options(walkers::sources::OpenStreetMap, options, ctx.clone())
}

/// Merender tab Map untuk hasil query aktif.
pub fn render_map_view(
    ui: &mut egui::Ui,
    state: &mut MapViewState,
    headers: &[String],
    rows: &[Vec<String>],
) {
    let geo_cols = detect_geometry_columns(headers, rows);
    if geo_cols.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                egui::RichText::new(
                    "No geometry column found. Supported: PostGIS geometry/geography, WKT, GeoJSON, WKB hex, MySQL spatial.",
                )
                .weak(),
            );
        });
        return;
    }
    if state.geom_col.is_none_or(|c| !geo_cols.contains(&c)) {
        state.geom_col = Some(geo_cols[0]);
    }
    let col = state.geom_col.unwrap_or(geo_cols[0]);

    let sig = data_signature(headers, rows, col, state.swap_xy);
    if state.layer.as_ref().is_none_or(|(s, _)| *s != sig) {
        state.layer = Some((sig, build_layer(rows, col, state.swap_xy)));
    }
    let Some((_, layer)) = state.layer.take() else {
        return;
    };

    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                render_toolbar(ui, state, headers, &geo_cols, &layer);
                ui.add_space(4.0);
                match layer.space {
                    CoordSpace::Geographic => {
                        render_geographic(ui, state, headers, rows, &layer, sig)
                    }
                    CoordSpace::Planar => render_planar(ui, &layer),
                }
            });
        });

    state.layer = Some((sig, layer));
}

fn render_toolbar(
    ui: &mut egui::Ui,
    state: &mut MapViewState,
    headers: &[String],
    geo_cols: &[usize],
    layer: &GeoLayer,
) {
    ui.horizontal_wrapped(|ui| {
        if geo_cols.len() > 1 {
            ui.label("Column");
            let current = state
                .geom_col
                .and_then(|c| headers.get(c))
                .cloned()
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("geo_map_column")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for &c in geo_cols {
                        if let Some(h) = headers.get(c) {
                            ui.selectable_value(&mut state.geom_col, Some(c), h);
                        }
                    }
                });
            ui.separator();
        }
        if layer.space == CoordSpace::Geographic {
            ui.checkbox(&mut state.basemap, "Basemap").on_hover_text(
                "OpenStreetMap tiles, downloaded from tile.openstreetmap.org and cached locally.",
            );
            if ui
                .button("Fit")
                .on_hover_text("Zoom to all features")
                .clicked()
            {
                state.fit_requested = true;
            }
        }
        ui.checkbox(&mut state.swap_xy, "Swap X/Y")
            .on_hover_text("Use when latitude and longitude appear reversed.");
        ui.separator();

        let mut info = format!("{} feature(s)", layer.features.len());
        if layer.skipped > 0 {
            info.push_str(&format!(" · {} row(s) without geometry", layer.skipped));
        }
        if let Some(s) = layer.srid {
            info.push_str(&format!(" · SRID {s}"));
        }
        if layer.space == CoordSpace::Planar {
            info.push_str(" · projected coordinates, shown without basemap");
        }
        if layer.features.len() >= MAX_FEATURES {
            info.push_str(&format!(" · limited to {MAX_FEATURES}"));
        }
        ui.label(egui::RichText::new(info).small().weak());
    });
}

fn feature_color(ui: &egui::Ui) -> Color32 {
    crate::window_egui::style::theme_accent(ui.ctx())
}

fn render_geographic(
    ui: &mut egui::Ui,
    state: &mut MapViewState,
    headers: &[String],
    rows: &[Vec<String>],
    layer: &GeoLayer,
    sig: u64,
) {
    let size = ui.available_size();
    if (state.fitted_sig != Some(sig) || state.fit_requested)
        && let Some(b) = layer.bbox
    {
        let (center, zoom) = fit_view(b, size);
        state.memory.center_at(lon_lat(center[0], center[1]));
        let _ = state.memory.set_zoom(zoom);
        state.fitted_sig = Some(sig);
        state.fit_requested = false;
    }
    // M12: basemap mengunduh tile dari OpenStreetMap; hormati toggle privasi.
    if state.basemap && !crate::privacy::allowed(crate::privacy::NetCategory::MapTiles) {
        crate::privacy::record(
            crate::privacy::NetCategory::MapTiles,
            "https://tile.openstreetmap.org/",
            false,
        );
        state.basemap = false;
        state.tiles = None;
    }
    if state.basemap && state.tiles.is_none() {
        crate::privacy::record(
            crate::privacy::NetCategory::MapTiles,
            "https://tile.openstreetmap.org/",
            true,
        );
        state.tiles = Some(tiles_for(ui.ctx()));
    }

    let accent = feature_color(ui);
    let dark = ui.visuals().dark_mode;
    let tiles: Option<&mut dyn walkers::Tiles> = if state.basemap {
        state.tiles.as_mut().map(|t| t as &mut dyn walkers::Tiles)
    } else {
        None
    };
    let fallback_center = layer
        .bbox
        .map(|b| lon_lat((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0))
        .unwrap_or_else(|| lon_lat(0.0, 0.0));

    let bg = ui.visuals().extreme_bg_color;
    let rect = egui::Rect::from_min_size(ui.cursor().min, size);
    ui.painter().rect_filled(rect, 0.0, bg);

    let hovered = std::cell::Cell::new(None::<usize>);
    let plugin = FeaturePlugin {
        layer,
        accent,
        dark,
        hovered: &hovered,
    };
    let response = Map::new(tiles, &mut state.memory, fallback_center)
        .zoom_with_ctrl(false)
        .with_plugin(plugin)
        .show(ui, |_, _, _, _| {})
        .response;

    if let Some(fi) = hovered.get()
        && let Some(f) = layer.features.get(fi)
        && let Some(row) = rows.get(f.row)
    {
        let geom_col = state.geom_col;
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(egui::RichText::new(format!("Row {}", f.row + 1)).strong());
            egui::Grid::new("geo_map_tooltip")
                .num_columns(2)
                .show(ui, |ui| {
                    for (ci, h) in headers.iter().enumerate().take(14) {
                        if Some(ci) == geom_col {
                            continue;
                        }
                        let v = row.get(ci).map(String::as_str).unwrap_or("");
                        let v: String = if v.chars().count() > 60 {
                            v.chars().take(59).chain(std::iter::once('…')).collect()
                        } else {
                            v.to_string()
                        };
                        ui.label(egui::RichText::new(h).weak());
                        ui.label(v);
                        ui.end_row();
                    }
                });
        });
    }

    if state.basemap {
        // Atribusi wajib menurut kebijakan tile OpenStreetMap.
        let galley = ui.painter().layout_no_wrap(
            "© OpenStreetMap contributors".to_string(),
            egui::FontId::proportional(10.5),
            Color32::from_gray(40),
        );
        let min = rect.right_bottom() - galley.size() - egui::vec2(6.0, 4.0);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(min, galley.size()).expand(2.0),
            2.0,
            Color32::from_white_alpha(190),
        );
        ui.painter().galley(min, galley, Color32::from_gray(40));
    }
}

struct FeaturePlugin<'a> {
    layer: &'a GeoLayer,
    accent: Color32,
    dark: bool,
    hovered: &'a std::cell::Cell<Option<usize>>,
}

fn screen(projector: &Projector, c: Coord) -> Pos2 {
    projector.project(lon_lat(c[0], c[1])).to_pos2()
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn point_in_ring(p: Pos2, ring: &[Pos2]) -> bool {
    let mut inside = false;
    let mut j = ring.len().wrapping_sub(1);
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Triangulasi ear clipping untuk ring sederhana (tanpa lubang). Mengembalikan indeks segitiga.
fn triangulate(ring: &[Pos2]) -> Vec<u32> {
    let mut idx: Vec<usize> = (0..ring.len()).collect();
    if idx.len() > 3 && ring.first() == ring.last() {
        idx.pop();
    }
    if idx.len() < 3 {
        return Vec::new();
    }
    let area: f32 = idx
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            let j = idx[(k + 1) % idx.len()];
            ring[i].x * ring[j].y - ring[j].x * ring[i].y
        })
        .sum();
    if area < 0.0 {
        idx.reverse();
    }
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let mut out = Vec::with_capacity((idx.len() - 2) * 3);
    let mut guard = idx.len() * idx.len();
    while idx.len() > 3 && guard > 0 {
        guard -= 1;
        let n = idx.len();
        let mut clipped = false;
        for k in 0..n {
            let (ia, ib, ic) = (idx[(k + n - 1) % n], idx[k], idx[(k + 1) % n]);
            let (a, b, c) = (ring[ia], ring[ib], ring[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            let contains_other = idx.iter().any(|&m| {
                m != ia
                    && m != ib
                    && m != ic
                    && cross(a, b, ring[m]) >= 0.0
                    && cross(b, c, ring[m]) >= 0.0
                    && cross(c, a, ring[m]) >= 0.0
            });
            if contains_other {
                continue;
            }
            out.extend([ia as u32, ib as u32, ic as u32]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Ring tidak sederhana (self-intersecting); hentikan tanpa isi sisa.
            break;
        }
    }
    if idx.len() == 3 {
        out.extend(idx.iter().map(|&i| i as u32));
    }
    out
}

impl FeaturePlugin<'_> {
    fn draw_shape(
        &self,
        painter: &egui::Painter,
        projector: &Projector,
        shape: &Shape,
        color: Color32,
        pointer: Option<Pos2>,
        hit: &mut bool,
    ) {
        let outline = if self.dark {
            Color32::from_gray(20)
        } else {
            Color32::WHITE
        };
        match shape {
            Shape::Point(c) => {
                let p = screen(projector, *c);
                painter.circle(p, 5.0, color, Stroke::new(1.5, outline));
                if pointer.is_some_and(|q| q.distance(p) <= 8.0) {
                    *hit = true;
                }
            }
            Shape::Line(cs) => {
                let pts: Vec<Pos2> = cs.iter().map(|c| screen(projector, *c)).collect();
                painter.line(pts.clone(), Stroke::new(2.5, color));
                if let Some(q) = pointer
                    && pts
                        .windows(2)
                        .any(|w| dist_to_segment(q, w[0], w[1]) <= 6.0)
                {
                    *hit = true;
                }
            }
            Shape::Polygon(rings) => {
                for (ri, ring) in rings.iter().enumerate() {
                    let pts: Vec<Pos2> = ring.iter().map(|c| screen(projector, *c)).collect();
                    if ri == 0 && pts.len() <= MAX_FILL_VERTICES {
                        let tris = triangulate(&pts);
                        if !tris.is_empty() {
                            let mut mesh = egui::Mesh::default();
                            let fill = color.gamma_multiply(0.28);
                            for p in &pts {
                                mesh.colored_vertex(*p, fill);
                            }
                            mesh.indices = tris;
                            painter.add(egui::Shape::mesh(mesh));
                        }
                    }
                    painter.add(egui::Shape::closed_line(
                        pts.clone(),
                        Stroke::new(2.0, color),
                    ));
                    if ri == 0 && pointer.is_some_and(|q| point_in_ring(q, &pts)) {
                        *hit = true;
                    }
                }
            }
            Shape::Collection(items) => {
                for s in items {
                    self.draw_shape(painter, projector, s, color, pointer, hit);
                }
            }
        }
    }
}

impl Plugin for FeaturePlugin<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut egui::Ui,
        response: &egui::Response,
        projector: &Projector,
        _map_memory: &walkers::MapMemory,
    ) {
        let painter = ui.painter().with_clip_rect(response.rect);
        let pointer = response.hover_pos();
        let highlight = Color32::from_rgb(255, 152, 0);
        let mut hovered = None;
        for (fi, f) in self.layer.features.iter().enumerate() {
            let mut hit = false;
            self.draw_shape(
                &painter,
                projector,
                &f.shape,
                self.accent,
                pointer,
                &mut hit,
            );
            if hit {
                hovered = Some(fi);
            }
        }
        // Feature yang di-hover digambar ulang paling atas dengan warna sorot.
        if let Some(fi) = hovered {
            let mut ignore = false;
            self.draw_shape(
                &painter,
                projector,
                &self.layer.features[fi].shape,
                highlight,
                None,
                &mut ignore,
            );
        }
        self.hovered.set(hovered);
    }
}

fn render_planar(ui: &mut egui::Ui, layer: &GeoLayer) {
    use egui_plot::{Line, Plot, PlotPoints, Points, Polygon};
    let color = feature_color(ui);

    fn collect<'a>(
        shape: &'a Shape,
        pts: &mut Vec<[f64; 2]>,
        lines: &mut Vec<&'a [Coord]>,
        polys: &mut Vec<&'a [Coord]>,
    ) {
        match shape {
            Shape::Point(c) => pts.push(*c),
            Shape::Line(cs) => lines.push(cs),
            Shape::Polygon(rings) => {
                if let Some(outer) = rings.first() {
                    polys.push(outer);
                }
                lines.extend(rings.iter().skip(1).map(Vec::as_slice));
            }
            Shape::Collection(items) => {
                for s in items {
                    collect(s, pts, lines, polys);
                }
            }
        }
    }
    let mut pts = Vec::new();
    let mut lines = Vec::new();
    let mut polys = Vec::new();
    for f in &layer.features {
        collect(&f.shape, &mut pts, &mut lines, &mut polys);
    }

    Plot::new("geo_map_planar")
        .data_aspect(1.0)
        .height(ui.available_height().max(160.0))
        .show(ui, |plot_ui| {
            for p in polys {
                plot_ui.add(
                    Polygon::new("", PlotPoints::from(p.to_vec()))
                        .fill_color(color.gamma_multiply(0.25))
                        .stroke(Stroke::new(1.5, color)),
                );
            }
            for l in lines {
                plot_ui.add(
                    Line::new("", PlotPoints::from(l.to_vec()))
                        .color(color)
                        .width(2.0),
                );
            }
            if !pts.is_empty() {
                plot_ui.add(
                    Points::new("", PlotPoints::from(pts))
                        .color(color)
                        .radius(4.0),
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn deteksi_kolom_geometry() {
        let headers = s(&["id", "name", "geom"]);
        let rows = vec![
            s(&["1", "Jakarta", "POINT(106.8 -6.2)"]),
            s(&["2", "Bandung", crate::models::structs::NULL_CELL]),
            s(&["3", "Surabaya", "SRID=4326;POINT(112.75 -7.25)"]),
        ];
        assert_eq!(detect_geometry_columns(&headers, &rows), vec![2]);
    }

    #[test]
    fn layer_geografis_dan_planar() {
        let rows = vec![
            s(&["POINT(106.8 -6.2)"]),
            s(&["bad"]),
            s(&["POINT(112.7 -7.2)"]),
        ];
        let l = build_layer(&rows, 0, false);
        assert_eq!(l.space, CoordSpace::Geographic);
        assert_eq!(l.features.len(), 2);
        assert_eq!(l.skipped, 1);
        assert_eq!(l.features[1].row, 2);

        let rows = vec![s(&["SRID=32748;POINT(700000 9300000)"])];
        assert_eq!(build_layer(&rows, 0, false).space, CoordSpace::Planar);
    }

    #[test]
    fn web_mercator_dikonversi_ke_lon_lat() {
        let rows = vec![s(&["SRID=3857;POINT(11131949.08 -692000.0)"])];
        let l = build_layer(&rows, 0, false);
        assert_eq!(l.space, CoordSpace::Geographic);
        let Shape::Point(c) = l.features[0].shape else {
            panic!("bukan titik")
        };
        assert!((c[0] - 100.0).abs() < 1e-3);
        assert!((c[1] + 6.19).abs() < 0.05);
    }

    #[test]
    fn swap_xy_menukar_koordinat() {
        let rows = vec![s(&["POINT(-6.2 106.8)"])];
        let l = build_layer(&rows, 0, true);
        assert_eq!(l.features[0].shape, Shape::Point([106.8, -6.2]));
    }

    #[test]
    fn fit_view_titik_tunggal_dan_area() {
        let (c, z) = fit_view([10.0, 20.0, 10.0, 20.0], egui::vec2(800.0, 600.0));
        assert_eq!(c, [10.0, 20.0]);
        assert_eq!(z, 15.0);
        let (_, z_world) = fit_view([-170.0, -60.0, 170.0, 70.0], egui::vec2(800.0, 600.0));
        let (_, z_city) = fit_view([106.7, -6.3, 106.9, -6.1], egui::vec2(800.0, 600.0));
        assert!(z_world < 3.0);
        assert!(z_city > 9.0);
    }

    #[test]
    fn triangulasi_poligon_cekung() {
        // Bentuk L: 6 vertex -> 4 segitiga.
        let ring: Vec<Pos2> = [(0., 0.), (4., 0.), (4., 1.), (1., 1.), (1., 4.), (0., 4.)]
            .iter()
            .map(|(x, y)| Pos2::new(*x, *y))
            .collect();
        assert_eq!(triangulate(&ring).len(), 12);
    }
}
