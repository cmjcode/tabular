//! Menggambar satu baris graf commit (lane, garis, node) dengan `Painter`.
//! Dipakai tab Git Graph dan daftar History di sidebar.

use eframe::egui;

use crate::git::graph::GraphRow;
use crate::git::repos::GraphStyle;

/// Palet warna lane (sama dengan bawaan Git Graph).
const PALETTE: [egui::Color32; 12] = [
    egui::Color32::from_rgb(0x00, 0x85, 0xd9),
    egui::Color32::from_rgb(0xd9, 0x00, 0x8f),
    egui::Color32::from_rgb(0x00, 0xd9, 0x0a),
    egui::Color32::from_rgb(0xd9, 0x85, 0x00),
    egui::Color32::from_rgb(0xa3, 0x00, 0xd9),
    egui::Color32::from_rgb(0xff, 0x00, 0x00),
    egui::Color32::from_rgb(0x00, 0xd9, 0xcc),
    egui::Color32::from_rgb(0xe1, 0x38, 0xe8),
    egui::Color32::from_rgb(0x85, 0xd9, 0x00),
    egui::Color32::from_rgb(0xdc, 0x5b, 0x23),
    egui::Color32::from_rgb(0x6f, 0x24, 0xd6),
    egui::Color32::from_rgb(0xff, 0xcc, 0x00),
];

pub fn lane_color(idx: usize) -> egui::Color32 {
    PALETTE[idx % PALETTE.len()]
}

/// Jenis node yang digambar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Commit,
    /// Commit HEAD: lingkaran berongga.
    Head,
    /// Baris "Uncommitted Changes": abu-abu, garis putus-putus.
    Uncommitted,
    Stash,
}

pub struct Geometry {
    /// Tepi kiri kolom graf.
    pub left: f32,
    pub lane_w: f32,
    pub style: GraphStyle,
}

impl Geometry {
    pub fn x(&self, lane: usize) -> f32 {
        self.left + self.lane_w * (lane as f32 + 0.5)
    }

    /// Lebar kolom graf untuk `lanes` lane.
    pub fn width(lanes: usize, lane_w: f32) -> f32 {
        lane_w * lanes.max(1) as f32 + 6.0
    }
}

fn line(
    painter: &egui::Painter,
    geo: &Geometry,
    a: egui::Pos2,
    b: egui::Pos2,
    stroke: egui::Stroke,
    dashed: bool,
) {
    if dashed {
        painter.extend(egui::Shape::dashed_line(&[a, b], stroke, 3.0, 3.0));
        return;
    }
    if (a.x - b.x).abs() < 0.5 {
        painter.line_segment([a, b], stroke);
        return;
    }
    match geo.style {
        GraphStyle::Angular => {
            painter.line_segment([a, b], stroke);
        }
        GraphStyle::Rounded => {
            let mid = (a.y + b.y) / 2.0;
            let shape = egui::epaint::CubicBezierShape::from_points_stroke(
                [a, egui::pos2(a.x, mid), egui::pos2(b.x, mid), b],
                false,
                egui::Color32::TRANSPARENT,
                stroke,
            );
            painter.add(shape);
        }
    }
}

/// Gambar baris `row` di `rect` (tinggi baris penuh).
pub fn paint_row(
    painter: &egui::Painter,
    rect: egui::Rect,
    geo: &Geometry,
    row: &GraphRow,
    node: NodeKind,
    selected_bg: egui::Color32,
) {
    let (top, bottom) = (rect.top(), rect.bottom());
    let mid = rect.center().y;
    let grey = egui::Color32::from_gray(128);
    let width = 2.0;
    for s in &row.segments {
        let (y0, y1) = if s.top { (top, mid) } else { (mid, bottom) };
        let from_node = !s.top && s.from == row.lane;
        let uncommitted = node == NodeKind::Uncommitted && from_node;
        let color = if uncommitted {
            grey
        } else {
            lane_color(s.color)
        };
        line(
            painter,
            geo,
            egui::pos2(geo.x(s.from), y0),
            egui::pos2(geo.x(s.to), y1),
            egui::Stroke::new(width, color),
            uncommitted,
        );
    }
    let c = egui::pos2(geo.x(row.lane), mid);
    let color = lane_color(row.color);
    let r = 4.0;
    match node {
        NodeKind::Commit => {
            painter.circle_filled(c, r, color);
        }
        NodeKind::Head => {
            painter.circle_filled(c, r + 0.5, selected_bg);
            painter.circle_stroke(c, r, egui::Stroke::new(2.0, color));
        }
        NodeKind::Uncommitted => {
            painter.circle_filled(c, r, selected_bg);
            painter.circle_stroke(c, r, egui::Stroke::new(1.5, grey));
        }
        NodeKind::Stash => {
            painter.circle_filled(c, r + 1.0, selected_bg);
            painter.circle_stroke(c, r + 1.0, egui::Stroke::new(1.5, color));
            painter.circle_filled(c, r - 1.5, color);
        }
    }
}
