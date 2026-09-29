//! Level-of-detail dan agregasi relasi untuk diagram besar.
//!
//! Semua fungsi di sini murni (tanpa `egui::Ui`) supaya bisa dites dan
//! dipanggil tiap frame oleh `diagram_view::render_diagram`. Tujuannya:
//! kerja per frame sebanding dengan yang terlihat, bukan dengan ukuran
//! diagram, dan relasi yang terlalu padat diringkas alih-alih digambar semua.

use std::collections::{HashMap, HashSet};

use crate::models::structs::{DiagramNode, DiagramState};
use eframe::egui;

/// Zoom minimal untuk relasi per kolom. Di bawahnya relasi digabung per
/// pasangan tabel. Tabel selalu digambar sebagai ERD (header + kolom).
/// Rentang zoom lama (50%–150%) tetap memakai relasi per kolom.
pub const DETAIL_MIN_ZOOM: f32 = 0.4;
/// Di bawah zoom ini tampilan jadi overview: relasi dibundel per group.
pub const OVERVIEW_MAX_ZOOM: f32 = 0.25;
/// Relasi virtual/linked digambar putus-putus hanya bila yang terlihat
/// sebanyak ini atau kurang; di atasnya garis solid (jauh lebih murah).
pub const DASH_BUDGET: usize = 300;
/// Di atas jumlah relasi terlihat ini, relasi tanpa sorotan dibuat samar
/// supaya struktur tetap terbaca dan tidak jadi gumpalan garis.
pub const DENSE_RELATIONS: usize = 500;
/// Hover tabel meredupkan relasi lain hanya bila relasi terlihat lebih dari ini.
pub const HOVER_DIM_MIN: usize = 60;
/// Opacity relasi saat diagram padat.
pub const DENSE_OPACITY: f32 = 0.35;
/// Opacity relasi lain saat sebuah tabel di-hover.
pub const HOVER_OTHERS_OPACITY: f32 = 0.2;
/// Lama animasi aliran data sebelum berhenti sendiri (detik).
pub const FLOW_ANIM_SECS: f64 = 30.0;

/// Tingkat detail gambar diagram, ditentukan oleh zoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lod {
    /// Relasi per kolom.
    Detail,
    /// Relasi digabung per pasangan tabel.
    Compact,
    /// Relasi antar group dibundel.
    Overview,
}

pub fn lod_for_zoom(zoom: f32) -> Lod {
    if zoom >= DETAIL_MIN_ZOOM {
        Lod::Detail
    } else if zoom >= OVERVIEW_MAX_ZOOM {
        Lod::Compact
    } else {
        Lod::Overview
    }
}

/// Indeks id node -> posisi di `nodes`. Dibangun sekali per frame supaya
/// resolusi ujung relasi O(1), bukan `nodes.iter().find` per relasi.
pub fn node_index(nodes: &[DiagramNode]) -> HashMap<&str, usize> {
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect()
}

/// Kurva bezier selalu berada di dalam convex hull titik kontrolnya, jadi
/// bounding box keempat titik cukup untuk menentukan kurva di luar layar.
pub fn curve_visible(ctrl: &[egui::Pos2; 4], clip: egui::Rect, margin: f32) -> bool {
    egui::Rect::from_points(ctrl)
        .expand(margin)
        .intersects(clip)
}

/// Jumlah sample kurva menurut panjangnya di layar: kurva pendek tidak
/// perlu 40 titik, kurva panjang tetap halus.
pub fn curve_samples(start: egui::Pos2, end: egui::Pos2) -> usize {
    ((start.distance(end) / 16.0).ceil() as usize).clamp(6, 32)
}

/// Pecah polyline menjadi potongan yang bersinggungan dengan `clip`.
/// Segmen di luar layar dibuang supaya relasi panjang yang hanya melintas
/// tidak di-tessellate sepanjang kurva penuhnya.
pub fn clip_polyline(points: &[egui::Pos2], clip: egui::Rect) -> Vec<Vec<egui::Pos2>> {
    let mut runs = Vec::new();
    let mut cur: Vec<egui::Pos2> = Vec::new();
    for w in points.windows(2) {
        if egui::Rect::from_two_pos(w[0], w[1]).intersects(clip) {
            if cur.is_empty() {
                cur.push(w[0]);
            }
            cur.push(w[1]);
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if cur.len() >= 2 {
        runs.push(cur);
    }
    runs
}

/// Ukuran font yang dibulatkan ke 0.5 px. Zoom halus tidak lagi membuat
/// galley teks baru di setiap langkah (cache galley egui dikunci per ukuran).
pub fn quantize_font(px: f32) -> f32 {
    (px * 2.0).round() / 2.0
}

/// Potong teks supaya kira-kira muat di `width` px pada ukuran font `px`
/// (lebar rata-rata glyph ≈ 0.55 × ukuran font), dengan elipsis.
pub fn truncate_for_width(text: &str, width: f32, px: f32) -> String {
    let max = (width / (px * 0.55)).floor().max(0.0) as usize;
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Jenis asal relasi.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelationKind {
    ForeignKey,
    Virtual,
    Linked,
}

impl RelationKind {
    pub fn label(self) -> &'static str {
        match self {
            RelationKind::ForeignKey => "FK",
            RelationKind::Virtual => "virtual",
            RelationKind::Linked => "linked",
        }
    }
}

/// Jenis relasi yang ditampilkan (toggle di toolbar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindFilter {
    pub fk: bool,
    pub virtual_: bool,
    pub linked: bool,
}

impl KindFilter {
    pub fn from_state(state: &DiagramState) -> Self {
        Self {
            fk: state.show_fk_relations,
            virtual_: state.show_virtual_relations,
            linked: state.show_linked_relations,
        }
    }

    pub fn allows(self, kind: RelationKind) -> bool {
        match kind {
            RelationKind::ForeignKey => self.fk,
            RelationKind::Virtual => self.virtual_,
            RelationKind::Linked => self.linked,
        }
    }
}

impl Default for KindFilter {
    fn default() -> Self {
        Self {
            fk: true,
            virtual_: true,
            linked: true,
        }
    }
}

/// Relasi antar dua tabel (tanpa arah) beserta jumlah per jenis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableLink {
    /// Index node, selalu `a < b`.
    pub a: usize,
    pub b: usize,
    pub fk: u32,
    pub virtual_: u32,
    pub linked: u32,
}

impl TableLink {
    pub fn total(&self) -> u32 {
        self.fk + self.virtual_ + self.linked
    }

    pub fn touches(&self, node: usize) -> bool {
        self.a == node || self.b == node
    }

    /// Ringkasan jumlah per jenis, mis. "3 FK · 1 virtual".
    pub fn breakdown(&self) -> String {
        let parts: Vec<String> = [
            (self.fk, "FK"),
            (self.virtual_, "virtual"),
            (self.linked, "linked"),
        ]
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, l)| format!("{n} {l}"))
        .collect();
        parts.join(" · ")
    }
}

/// Gabungkan FK (`state.edges`), relasi virtual, dan relasi link menjadi satu
/// entri per pasangan tabel. Relasi ke tabel yang tidak ada dan relasi ke
/// diri sendiri dilewati. Urutan hasil deterministik (urut index node).
pub fn aggregate_table_links(
    state: &DiagramState,
    index: &HashMap<&str, usize>,
    filter: KindFilter,
) -> Vec<TableLink> {
    let mut map: HashMap<(usize, usize), TableLink> = HashMap::new();
    let mut add = |x: &str, y: &str, kind: RelationKind| {
        let (Some(&i), Some(&j)) = (index.get(x), index.get(y)) else {
            return;
        };
        if i == j {
            return;
        }
        let (a, b) = if i < j { (i, j) } else { (j, i) };
        let link = map.entry((a, b)).or_insert_with(|| TableLink {
            a,
            b,
            ..Default::default()
        });
        match kind {
            RelationKind::ForeignKey => link.fk += 1,
            RelationKind::Virtual => link.virtual_ += 1,
            RelationKind::Linked => link.linked += 1,
        }
    };
    if filter.fk {
        for e in &state.edges {
            add(&e.source, &e.target, RelationKind::ForeignKey);
        }
    }
    if filter.virtual_ {
        for r in &state.virtual_relations {
            add(&r.child, &r.parent, RelationKind::Virtual);
        }
    }
    if filter.linked {
        for r in &state.linked_relations {
            add(&r.child, &r.parent, RelationKind::Linked);
        }
    }
    let mut out: Vec<TableLink> = map.into_values().collect();
    out.sort_by_key(|l| (l.a, l.b));
    out
}

/// Jumlah relasi per node (index sama dengan `nodes`).
pub fn relation_counts(links: &[TableLink], node_count: usize) -> Vec<u32> {
    let mut counts = vec![0u32; node_count];
    for l in links {
        counts[l.a] += l.total();
        counts[l.b] += l.total();
    }
    counts
}

/// Bundel relasi antar dua group (index di `state.groups`, `a < b`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupLink {
    pub a: usize,
    pub b: usize,
    pub count: u32,
}

/// Hasil bundling untuk tampilan overview.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundled {
    /// Relasi antar group berbeda, satu garis per pasangan group.
    pub groups: Vec<GroupLink>,
    /// Jumlah relasi di dalam satu group (index = index group).
    pub intra: Vec<u32>,
    /// Relasi yang salah satu ujungnya tanpa group; tetap per tabel.
    pub rest: Vec<TableLink>,
}

/// Group utama sebuah node (group pertama) sebagai index `state.groups`.
fn primary_group(node: &DiagramNode, groups: &HashMap<&str, usize>) -> Option<usize> {
    node.group_ids
        .first()
        .or(node.group_id.as_ref())
        .and_then(|g| groups.get(g.as_str()).copied())
}

/// Bundel relasi per pasangan group untuk tampilan overview.
pub fn bundle_by_group(state: &DiagramState, links: &[TableLink]) -> Bundled {
    let groups: HashMap<&str, usize> = state
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.as_str(), i))
        .collect();
    let node_group: Vec<Option<usize>> = state
        .nodes
        .iter()
        .map(|n| primary_group(n, &groups))
        .collect();

    let mut out = Bundled {
        intra: vec![0; state.groups.len()],
        ..Default::default()
    };
    let mut map: HashMap<(usize, usize), u32> = HashMap::new();
    for l in links {
        match (node_group[l.a], node_group[l.b]) {
            (Some(x), Some(y)) if x == y => out.intra[x] += l.total(),
            (Some(x), Some(y)) => {
                let key = if x < y { (x, y) } else { (y, x) };
                *map.entry(key).or_default() += l.total();
            }
            _ => out.rest.push(l.clone()),
        }
    }
    out.groups = map
        .into_iter()
        .map(|((a, b), count)| GroupLink { a, b, count })
        .collect();
    out.groups.sort_by_key(|g| (g.a, g.b));
    out
}

/// Satu baris di panel relasi.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationRow {
    pub kind: RelationKind,
    pub child: String,
    pub child_column: String,
    pub parent: String,
    pub parent_column: String,
}

impl RelationRow {
    /// Tabel di ujung lain dari `table`.
    pub fn other<'a>(&'a self, table: &str) -> &'a str {
        if self.child == table {
            &self.parent
        } else {
            &self.child
        }
    }

    /// Cocok dengan teks pencarian (nama tabel atau kolom, tanpa beda huruf).
    pub fn matches(&self, query_lower: &str) -> bool {
        query_lower.is_empty()
            || [
                &self.child,
                &self.child_column,
                &self.parent,
                &self.parent_column,
            ]
            .iter()
            .any(|s| s.to_lowercase().contains(query_lower))
    }
}

/// Semua relasi per kolom yang menyentuh `table`: FK database (dari
/// `foreign_keys` tiap node), relasi virtual, dan relasi link. Duplikat
/// dibuang; FK tanpa detail kolom tetap tampil lewat `state.edges`.
pub fn relations_of(state: &DiagramState, table: &str, filter: KindFilter) -> Vec<RelationRow> {
    let mut out: Vec<RelationRow> = Vec::new();
    let mut seen: HashSet<(RelationKind, String, String, String, String)> = HashSet::new();
    let mut push = |row: RelationRow| {
        let key = (
            row.kind,
            row.child.clone(),
            row.child_column.clone(),
            row.parent.clone(),
            row.parent_column.clone(),
        );
        if seen.insert(key) {
            out.push(row);
        }
    };

    if filter.fk {
        let mut fk_pairs: HashSet<(&str, &str)> = HashSet::new();
        for node in &state.nodes {
            for fk in &node.foreign_keys {
                if fk.table_name != node.id {
                    continue;
                }
                if fk.table_name == table || fk.referenced_table_name == table {
                    fk_pairs.insert((&fk.table_name, &fk.referenced_table_name));
                    push(RelationRow {
                        kind: RelationKind::ForeignKey,
                        child: fk.table_name.clone(),
                        child_column: fk.column_name.clone(),
                        parent: fk.referenced_table_name.clone(),
                        parent_column: fk.referenced_column_name.clone(),
                    });
                }
            }
        }
        // Edge FK tanpa detail kolom (mis. dari diagram lama).
        for e in &state.edges {
            if (e.source == table || e.target == table)
                && !fk_pairs.contains(&(e.source.as_str(), e.target.as_str()))
            {
                push(RelationRow {
                    kind: RelationKind::ForeignKey,
                    child: e.source.clone(),
                    child_column: String::new(),
                    parent: e.target.clone(),
                    parent_column: String::new(),
                });
            }
        }
    }
    for (kind, rels) in [
        (RelationKind::Virtual, &state.virtual_relations),
        (RelationKind::Linked, &state.linked_relations),
    ] {
        if !filter.allows(kind) {
            continue;
        }
        for r in rels {
            if r.child == table || r.parent == table {
                push(RelationRow {
                    kind,
                    child: r.child.clone(),
                    child_column: r.child_column.clone(),
                    parent: r.parent.clone(),
                    parent_column: r.parent_column.clone(),
                });
            }
        }
    }
    out
}

/// Konteks penekanan relasi untuk satu frame.
#[derive(Clone, Copy, Debug)]
pub struct Emphasis<'a> {
    pub focus: Option<&'a str>,
    pub hovered: Option<&'a str>,
    /// Jumlah relasi terlihat pada frame sebelumnya.
    pub visible: usize,
    pub dim_opacity: f32,
}

/// Cara menggambar satu relasi.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emph {
    /// Pengali opacity warna.
    pub alpha: f32,
    /// Relasi milik tabel yang di-hover: lebih tebal dan terang.
    pub highlight: bool,
    /// Bisa di-hover/klik (tidak redup karena mode fokus).
    pub interactive: bool,
}

impl Emphasis<'_> {
    pub fn of(&self, a: &str, b: &str) -> Emph {
        if let Some(f) = self.focus {
            let on = a == f || b == f;
            return Emph {
                alpha: if on { 1.0 } else { self.dim_opacity },
                highlight: false,
                interactive: on,
            };
        }
        if let Some(h) = self.hovered {
            if a == h || b == h {
                return Emph {
                    alpha: 1.0,
                    highlight: true,
                    interactive: true,
                };
            }
            if self.visible > HOVER_DIM_MIN {
                return Emph {
                    alpha: HOVER_OTHERS_OPACITY,
                    highlight: false,
                    interactive: true,
                };
            }
        }
        Emph {
            alpha: if self.visible > DENSE_RELATIONS {
                DENSE_OPACITY
            } else {
                1.0
            },
            highlight: false,
            interactive: true,
        }
    }
}

/// Node teratas (terakhir digambar) yang memuat titik layar `p`.
pub fn node_at(
    nodes: &[DiagramNode],
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    scale: f32,
    p: egui::Pos2,
) -> Option<usize> {
    nodes
        .iter()
        .rposition(|n| egui::Rect::from_min_size(to_screen(n.pos), n.size * scale).contains(p))
}

/// Zoom supaya seluruh konten (`content`, koordinat diagram) muat di
/// viewport dengan margin, dibatasi ke rentang zoom yang diizinkan.
pub fn fit_zoom(content: egui::Vec2, view: egui::Vec2, min: f32, max: f32) -> f32 {
    let margin = 48.0;
    let avail = (view - egui::vec2(margin, margin) * 2.0).max(egui::vec2(1.0, 1.0));
    let zx = avail.x / content.x.max(1.0);
    let zy = avail.y / content.y.max(1.0);
    zx.min(zy).clamp(min, max)
}

/// Bounding box seluruh node (koordinat diagram).
pub fn content_bounds(nodes: &[DiagramNode]) -> Option<egui::Rect> {
    nodes.iter().fold(None, |acc: Option<egui::Rect>, n| {
        let r = egui::Rect::from_min_size(n.pos, n.size);
        Some(acc.map_or(r, |a| a.union(r)))
    })
}

/// Diagram sintetis untuk tes dan benchmark: `tables` tabel dalam grid,
/// dibagi ke `groups` group, dengan `fks` FK dan `virtuals` relasi virtual
/// yang tersebar deterministik.
#[cfg(test)]
pub fn synthetic_state(tables: usize, groups: usize, fks: usize, virtuals: usize) -> DiagramState {
    use crate::models::structs::{
        DiagramEdge, DiagramGroup, ForeignKey, RelationOrigin, VirtualRelation,
    };

    let mut state = DiagramState::default();
    let cols_per_table = 8;
    let per_row = (tables as f32).sqrt().ceil().max(1.0) as usize;
    for g in 0..groups {
        state.groups.push(DiagramGroup {
            id: format!("g{g}"),
            title: format!("Group {g}"),
            color: crate::diagram_view::GROUP_COLORS[g % crate::diagram_view::GROUP_COLORS.len()],
            manual_pos: None,
        });
    }
    for t in 0..tables {
        let columns: Vec<String> = (0..cols_per_table).map(|c| format!("col_{c}")).collect();
        let mut node = DiagramNode {
            id: format!("t{t}"),
            title: format!("t{t}"),
            pos: egui::pos2((t % per_row) as f32 * 320.0, (t / per_row) as f32 * 260.0),
            size: egui::vec2(180.0, 24.0 + cols_per_table as f32 * 16.0 + 8.0),
            columns,
            ..Default::default()
        };
        if groups > 0 {
            node.add_to_group(format!("g{}", t * groups / tables.max(1)));
        }
        state.nodes.push(node);
    }
    // Pseudo-random deterministik (LCG) supaya relasi tersebar merata.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |n: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) as usize) % n.max(1)
    };
    for k in 0..fks {
        let child = next(tables);
        let mut parent = next(tables);
        if parent == child {
            parent = (parent + 1) % tables;
        }
        let col = format!("col_{}", 1 + k % (cols_per_table - 1));
        let fk = ForeignKey {
            constraint_name: format!("fk_{k}"),
            table_name: format!("t{child}"),
            column_name: col,
            referenced_table_name: format!("t{parent}"),
            referenced_column_name: "col_0".to_string(),
        };
        state.edges.push(DiagramEdge {
            source: fk.table_name.clone(),
            target: fk.referenced_table_name.clone(),
            label: String::new(),
        });
        state.nodes[child].foreign_keys.push(fk);
    }
    for k in 0..virtuals {
        let child = next(tables);
        let mut parent = next(tables);
        if parent == child {
            parent = (parent + 1) % tables;
        }
        state.virtual_relations.push(VirtualRelation {
            child: format!("t{child}"),
            child_column: format!("col_{}", 1 + k % (cols_per_table - 1)),
            parent: format!("t{parent}"),
            parent_column: "col_0".to_string(),
            origin: RelationOrigin::Manual,
        });
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramEdge, RelationOrigin, VirtualRelation};

    fn node(id: &str, x: f32, group: Option<&str>) -> DiagramNode {
        let mut n = DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: egui::pos2(x, 0.0),
            size: egui::vec2(100.0, 100.0),
            ..Default::default()
        };
        if let Some(g) = group {
            n.add_to_group(g.to_string());
        }
        n
    }

    fn vrel(child: &str, parent: &str) -> VirtualRelation {
        VirtualRelation {
            child: child.to_string(),
            child_column: "x_id".to_string(),
            parent: parent.to_string(),
            parent_column: "id".to_string(),
            origin: RelationOrigin::Manual,
        }
    }

    fn edge(s: &str, t: &str) -> DiagramEdge {
        DiagramEdge {
            source: s.to_string(),
            target: t.to_string(),
            label: String::new(),
        }
    }

    #[test]
    fn lod_follows_zoom_thresholds() {
        assert_eq!(lod_for_zoom(1.0), Lod::Detail);
        assert_eq!(lod_for_zoom(DETAIL_MIN_ZOOM), Lod::Detail);
        assert_eq!(lod_for_zoom(0.3), Lod::Compact);
        assert_eq!(lod_for_zoom(OVERVIEW_MAX_ZOOM), Lod::Compact);
        assert_eq!(lod_for_zoom(0.1), Lod::Overview);
    }

    #[test]
    fn curve_visibility_uses_control_point_hull() {
        let clip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        let inside = [
            egui::pos2(10.0, 10.0),
            egui::pos2(20.0, 10.0),
            egui::pos2(30.0, 50.0),
            egui::pos2(40.0, 50.0),
        ];
        assert!(curve_visible(&inside, clip, 0.0));
        let far = inside.map(|p| p + egui::vec2(500.0, 0.0));
        assert!(!curve_visible(&far, clip, 10.0));
        // Ujung di luar, tapi kurva melintasi layar.
        let crossing = [
            egui::pos2(-200.0, 50.0),
            egui::pos2(-100.0, 50.0),
            egui::pos2(200.0, 50.0),
            egui::pos2(300.0, 50.0),
        ];
        assert!(curve_visible(&crossing, clip, 0.0));
    }

    #[test]
    fn curve_samples_scale_with_length() {
        let a = egui::pos2(0.0, 0.0);
        assert_eq!(curve_samples(a, egui::pos2(10.0, 0.0)), 6);
        assert_eq!(curve_samples(a, egui::pos2(5000.0, 0.0)), 32);
        let mid = curve_samples(a, egui::pos2(200.0, 0.0));
        assert!((6..32).contains(&mid));
    }

    #[test]
    fn clip_polyline_keeps_only_visible_runs() {
        let clip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
        let pts: Vec<egui::Pos2> = [-300.0, -200.0, -50.0, 50.0, 150.0, 300.0, 400.0]
            .iter()
            .map(|&x| egui::pos2(x, 50.0))
            .collect();
        let runs = clip_polyline(&pts, clip);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].first().map(|p| p.x), Some(-50.0));
        assert_eq!(runs[0].last().map(|p| p.x), Some(150.0));
        // Keluar lalu masuk lagi -> dua potongan.
        let zig: Vec<egui::Pos2> = [10.0, 50.0, 500.0, 600.0, 50.0, 20.0]
            .iter()
            .map(|&x| egui::pos2(x, if x > 400.0 { 400.0 } else { 50.0 }))
            .collect();
        assert!(!clip_polyline(&zig, clip).is_empty());
        assert!(
            clip_polyline(&[egui::pos2(500.0, 500.0), egui::pos2(600.0, 600.0)], clip).is_empty()
        );
    }

    #[test]
    fn quantize_font_rounds_to_half_pixel() {
        assert_eq!(quantize_font(12.26), 12.5);
        assert_eq!(quantize_font(12.24), 12.0);
    }

    #[test]
    fn truncate_for_width_adds_ellipsis() {
        assert_eq!(truncate_for_width("orders", 100.0, 10.0), "orders");
        assert_eq!(
            truncate_for_width("customer_addresses", 33.0, 10.0),
            "custo…"
        );
        assert_eq!(truncate_for_width("abc", 1.0, 10.0), "…");
    }

    #[test]
    fn aggregate_merges_pairs_and_counts_kinds() {
        let mut state = DiagramState {
            nodes: vec![
                node("a", 0.0, None),
                node("b", 200.0, None),
                node("c", 400.0, None),
            ],
            ..Default::default()
        };
        // Dua FK a->b (FK komposit / dua kolom) dan satu b->a arah sebaliknya.
        state.edges = vec![
            edge("a", "b"),
            edge("a", "b"),
            edge("b", "a"),
            edge("a", "a"),
        ];
        state.virtual_relations = vec![vrel("a", "b"), vrel("c", "a"), vrel("c", "missing")];
        state.linked_relations = vec![vrel("b", "c")];

        let index = node_index(&state.nodes);
        let links = aggregate_table_links(&state, &index, KindFilter::default());
        assert_eq!(links.len(), 3);
        let ab = &links[0];
        assert_eq!((ab.a, ab.b, ab.fk, ab.virtual_, ab.linked), (0, 1, 3, 1, 0));
        assert_eq!(ab.total(), 4);
        assert_eq!(ab.breakdown(), "3 FK · 1 virtual");
        assert_eq!(
            links[1],
            TableLink {
                a: 0,
                b: 2,
                virtual_: 1,
                ..Default::default()
            }
        );
        assert_eq!(
            links[2],
            TableLink {
                a: 1,
                b: 2,
                linked: 1,
                ..Default::default()
            }
        );

        assert_eq!(relation_counts(&links, 3), vec![5, 5, 2]);

        let only_virtual = KindFilter {
            fk: false,
            virtual_: true,
            linked: false,
        };
        let links = aggregate_table_links(&state, &index, only_virtual);
        assert_eq!(links.iter().map(TableLink::total).sum::<u32>(), 2);
    }

    #[test]
    fn bundle_groups_intra_and_rest() {
        let mut state = DiagramState {
            groups: synthetic_state(1, 2, 0, 0).groups,
            nodes: vec![
                node("a", 0.0, Some("g0")),
                node("b", 0.0, Some("g0")),
                node("c", 0.0, Some("g1")),
                node("d", 0.0, None),
            ],
            ..Default::default()
        };
        state.edges = vec![
            edge("a", "b"),
            edge("a", "c"),
            edge("b", "c"),
            edge("c", "d"),
        ];
        let index = node_index(&state.nodes);
        let links = aggregate_table_links(&state, &index, KindFilter::default());
        let bundled = bundle_by_group(&state, &links);
        assert_eq!(bundled.intra, vec![1, 0]);
        assert_eq!(
            bundled.groups,
            vec![GroupLink {
                a: 0,
                b: 1,
                count: 2
            }]
        );
        assert_eq!(bundled.rest.len(), 1);
        assert_eq!((bundled.rest[0].a, bundled.rest[0].b), (2, 3));
    }

    #[test]
    fn relations_of_lists_all_kinds_without_duplicates() {
        let mut state = synthetic_state(3, 0, 0, 0);
        let fk = crate::models::structs::ForeignKey {
            constraint_name: "fk".into(),
            table_name: "t0".into(),
            column_name: "col_1".into(),
            referenced_table_name: "t1".into(),
            referenced_column_name: "col_0".into(),
        };
        state.nodes[0].foreign_keys.push(fk.clone());
        // FK yang sama tercatat juga di node tujuan: tidak boleh dobel.
        state.nodes[1].foreign_keys.push(fk);
        state.edges = vec![edge("t0", "t1"), edge("t2", "t1")];
        state.virtual_relations = vec![vrel("t2", "t1"), vrel("t2", "t0")];
        state.linked_relations = vec![vrel("t1", "t2")];

        let rows = relations_of(&state, "t1", KindFilter::default());
        let kinds: Vec<RelationKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(
            kinds,
            vec![
                RelationKind::ForeignKey,
                RelationKind::ForeignKey,
                RelationKind::Virtual,
                RelationKind::Linked
            ]
        );
        assert_eq!(rows[0].child_column, "col_1");
        // Edge t2->t1 tanpa detail kolom tetap muncul.
        assert_eq!(rows[1].child, "t2");
        assert!(rows[1].child_column.is_empty());
        assert_eq!(rows[2].other("t1"), "t2");
        assert!(rows[0].matches("col_1"));
        assert!(!rows[0].matches("nothing"));

        let fk_only = KindFilter {
            fk: true,
            virtual_: false,
            linked: false,
        };
        assert_eq!(relations_of(&state, "t1", fk_only).len(), 2);
    }

    #[test]
    fn emphasis_rules() {
        let base = Emphasis {
            focus: None,
            hovered: None,
            visible: 10,
            dim_opacity: 0.15,
        };
        assert_eq!(base.of("a", "b").alpha, 1.0);

        let dense = Emphasis {
            visible: DENSE_RELATIONS + 1,
            ..base
        };
        assert_eq!(dense.of("a", "b").alpha, DENSE_OPACITY);
        assert!(dense.of("a", "b").interactive);

        let focus = Emphasis {
            focus: Some("a"),
            ..dense
        };
        assert_eq!(focus.of("a", "b").alpha, 1.0);
        let off = focus.of("c", "b");
        assert_eq!(off.alpha, 0.15);
        assert!(!off.interactive);

        let hover_small = Emphasis {
            hovered: Some("a"),
            ..base
        };
        assert!(hover_small.of("b", "a").highlight);
        assert_eq!(hover_small.of("b", "c").alpha, 1.0);

        let hover_big = Emphasis {
            hovered: Some("a"),
            visible: HOVER_DIM_MIN + 1,
            ..base
        };
        assert_eq!(hover_big.of("b", "c").alpha, HOVER_OTHERS_OPACITY);
    }

    #[test]
    fn fit_zoom_clamps() {
        let view = egui::vec2(1000.0, 800.0);
        let z = fit_zoom(egui::vec2(9040.0, 100.0), view, 0.1, 1.0);
        assert!((z - 0.1).abs() < 1e-3);
        assert_eq!(fit_zoom(egui::vec2(10.0, 10.0), view, 0.1, 1.0), 1.0);
        assert_eq!(fit_zoom(egui::vec2(1e6, 1e6), view, 0.1, 1.0), 0.1);
    }

    #[test]
    fn node_at_prefers_topmost() {
        let nodes = vec![node("a", 0.0, None), node("b", 50.0, None)];
        let to_screen = |p: egui::Pos2| p;
        assert_eq!(
            node_at(&nodes, &to_screen, 1.0, egui::pos2(60.0, 10.0)),
            Some(1)
        );
        assert_eq!(
            node_at(&nodes, &to_screen, 1.0, egui::pos2(10.0, 10.0)),
            Some(0)
        );
        assert_eq!(
            node_at(&nodes, &to_screen, 1.0, egui::pos2(500.0, 10.0)),
            None
        );
    }

    #[test]
    fn synthetic_state_shape() {
        let s = synthetic_state(100, 5, 300, 200);
        assert_eq!(s.nodes.len(), 100);
        assert_eq!(s.groups.len(), 5);
        assert_eq!(s.edges.len(), 300);
        assert_eq!(s.virtual_relations.len(), 200);
        assert!(s.nodes.iter().all(|n| !n.group_ids.is_empty()));
    }
}
