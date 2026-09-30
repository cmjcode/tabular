//! Daftar hasil pencarian diagram (Cmd+F).
//!
//! Tabel, kolom, dan group yang cocok dengan query ditampilkan di bawah
//! search box, masing-masing diberi badge jenisnya. Klik satu hasil
//! menganimasikan kanvas ke objek tersebut lalu menyorotnya sebentar.

use crate::diagram_view::{FOCUS_ZOOM, MAX_ZOOM, MIN_ZOOM};
use crate::models::structs::DiagramState;
use crate::search_match::SearchQuery;
use eframe::egui;
use std::sync::Arc;

/// Jumlah maksimum hasil yang ditampilkan di list.
const MAX_RESULTS: usize = 100;
/// Lama sorotan objek setelah hasil diklik.
const HIGHLIGHT_SECS: f64 = 1.6;
/// Lebar panel, sama dengan kartu search box di atasnya.
pub const PANEL_WIDTH: f32 = 295.0;
const ROW_HEIGHT: f32 = 34.0;
const BADGE_WIDTH: f32 = 58.0;

/// Objek diagram yang ditunjuk sebuah hasil pencarian.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchTarget {
    Table(String),
    Column {
        table: String,
        column: String,
    },
    Group(String),
    /// Endpoint HTTP API yang tertaut ke `table`. `card` = flow card-nya
    /// bila endpoint tampil sebagai card; hasilnya melompat ke card itu.
    Endpoint {
        table: String,
        label: String,
        card: Option<String>,
    },
}

impl SearchTarget {
    /// Urutan jenis saat skor sama: tabel, group, lalu kolom.
    fn kind_rank(&self) -> u8 {
        match self {
            SearchTarget::Table(_) => 0,
            SearchTarget::Group(_) => 1,
            SearchTarget::Column { .. } => 2,
            SearchTarget::Endpoint { .. } => 3,
        }
    }

    fn badge(&self) -> (&'static str, egui::Color32) {
        match self {
            SearchTarget::Table(_) => ("TABLE", egui::Color32::from_rgb(66, 133, 244)),
            SearchTarget::Column { .. } => ("COLUMN", egui::Color32::from_rgb(46, 160, 90)),
            SearchTarget::Group(_) => ("GROUP", egui::Color32::from_rgb(171, 71, 188)),
            SearchTarget::Endpoint { .. } => ("API", egui::Color32::from_rgb(33, 150, 243)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub target: SearchTarget,
    /// Nama yang dicocokkan (nama tabel, kolom, atau group).
    pub label: String,
    /// Keterangan sekunder, mis. tabel pemilik kolom atau jumlah anggota group.
    pub detail: String,
    pub score: f32,
}

/// Hasil pencarian beserta jumlah total sebelum dipotong `MAX_RESULTS`.
#[derive(Clone, Debug, Default)]
pub struct SearchHits {
    pub hits: Vec<SearchHit>,
    pub total: usize,
}

/// Cari tabel/kolom/group yang cocok dengan `query`, mengikuti checkbox
/// filter di `state`. Diurutkan dari skor tertinggi.
pub fn search_hits(state: &DiagramState, query: &str) -> SearchHits {
    let q = SearchQuery::new(query);
    if q.is_empty() {
        return SearchHits::default();
    }
    let mut hits = Vec::new();

    for node in &state.nodes {
        if state.search_tables
            && let Some(score) = q.score(&node.title)
        {
            let mut detail = format!("{} columns", node.columns.len());
            if let Some(db) = node.database_name.as_deref().filter(|d| !d.is_empty()) {
                detail = format!("{db} · {detail}");
            }
            hits.push(SearchHit {
                target: SearchTarget::Table(node.id.clone()),
                label: node.title.clone(),
                detail,
                score,
            });
        }
        if state.search_columns {
            for col in &node.columns {
                let Some(score) = q.score(col) else {
                    continue;
                };
                let type_name = node
                    .column_meta
                    .iter()
                    .find(|m| &m.name == col)
                    .map(|m| m.type_name.as_str())
                    .filter(|t| !t.is_empty());
                let detail = match type_name {
                    Some(t) => format!("{} · {t}", node.title),
                    None => node.title.clone(),
                };
                hits.push(SearchHit {
                    target: SearchTarget::Column {
                        table: node.id.clone(),
                        column: col.clone(),
                    },
                    label: col.clone(),
                    detail,
                    score,
                });
            }
        }
    }

    if state.search_groups {
        for group in &state.groups {
            let Some(score) = q.score(&group.title) else {
                continue;
            };
            let members = state
                .nodes
                .iter()
                .filter(|n| n.is_in_group(&group.id))
                .count();
            hits.push(SearchHit {
                target: SearchTarget::Group(group.id.clone()),
                label: group.title.clone(),
                detail: format!("{members} tables"),
                score,
            });
        }
    }

    // Endpoint HTTP API ikut filter tabel: hasilnya menunjuk tabel pemakainya,
    // atau pada mode Cards satu hasil per card yang melompat ke card itu.
    if state.search_tables && state.show_endpoints {
        let cards = state.endpoint_display.shows_cards();
        let mut seen_cards: std::collections::HashSet<&str> = Default::default();
        for link in &state.endpoint_links {
            let label = format!("{} {}", link.method, link.path);
            let Some(score) = q.score(&label).or_else(|| q.score(&link.summary)) else {
                continue;
            };
            let card = cards
                .then(|| {
                    crate::diagram_flow::card_for_endpoint(
                        state,
                        link.repo_key.as_deref(),
                        &link.method,
                        &link.path,
                    )
                })
                .flatten();
            if let Some(c) = card
                && !seen_cards.insert(c.id.as_str())
            {
                continue;
            }
            let detail = match card {
                Some(c) => {
                    let n = crate::diagram_flow::tables_of(state, c).len();
                    if c.steps.is_empty() {
                        format!("process card · {n} table(s)")
                    } else {
                        format!("process card · {} steps · {n} table(s)", c.steps.len())
                    }
                }
                None => {
                    let table = state
                        .nodes
                        .iter()
                        .find(|n| n.id == link.table)
                        .map_or(link.table.as_str(), |n| n.title.as_str());
                    format!("uses {table}")
                }
            };
            hits.push(SearchHit {
                target: SearchTarget::Endpoint {
                    table: link.table.clone(),
                    label: label.clone(),
                    card: card.map(|c| c.id.clone()),
                },
                label,
                detail,
                score,
            });
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.target.kind_rank().cmp(&b.target.kind_rank()))
            .then(a.label.len().cmp(&b.label.len()))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    let total = hits.len();
    hits.truncate(MAX_RESULTS);
    SearchHits { hits, total }
}

/// Area objek (koordinat diagram) yang ditunjuk `target`, `None` bila
/// objeknya sudah tidak ada.
pub fn target_world_rect(state: &DiagramState, target: &SearchTarget) -> Option<egui::Rect> {
    match target {
        SearchTarget::Endpoint {
            card: Some(card), ..
        } => crate::diagram_flow_layout::card_world_rect(state, card),
        SearchTarget::Table(id) | SearchTarget::Endpoint { table: id, .. } => state
            .nodes
            .iter()
            .find(|n| &n.id == id)
            .map(|n| egui::Rect::from_min_size(n.pos, n.size)),
        SearchTarget::Column { table, column } => {
            let node = state.nodes.iter().find(|n| &n.id == table)?;
            let y = crate::diagram_view::column_anchor_y(node, column);
            Some(egui::Rect::from_min_max(
                egui::pos2(node.pos.x, y - 8.0),
                egui::pos2(node.pos.x + node.size.x, y + 8.0),
            ))
        }
        SearchTarget::Group(id) => {
            let members = state
                .nodes
                .iter()
                .filter(|n| n.is_in_group(id))
                .map(|n| egui::Rect::from_min_size(n.pos, n.size))
                .reduce(|a, b| a.union(b));
            match members {
                // Padding sama dengan bingkai group di `render_diagram`.
                Some(r) => Some(egui::Rect::from_min_max(
                    r.min - egui::vec2(20.0, 50.0),
                    r.max + egui::vec2(20.0, 20.0),
                )),
                None => {
                    let group = state.groups.iter().find(|g| &g.id == id)?;
                    group
                        .manual_pos
                        .map(|p| egui::Rect::from_min_size(p, egui::vec2(400.0, 300.0)))
                }
            }
        }
    }
}

/// Mulai animasi kanvas menuju `target`. Tabel dan kolom memakai zoom baca;
/// group di-zoom sampai seluruh bingkainya muat di layar.
pub fn focus_target(
    state: &mut DiagramState,
    target: &SearchTarget,
    view_size: egui::Vec2,
    now: f64,
) -> bool {
    let Some(world) = target_world_rect(state, target) else {
        return false;
    };
    let zoom = match target {
        SearchTarget::Group(_) => {
            crate::diagram_lod::fit_zoom(world.size(), view_size, MIN_ZOOM, FOCUS_ZOOM)
        }
        _ => FOCUS_ZOOM,
    }
    .clamp(MIN_ZOOM, MAX_ZOOM);
    crate::diagram_view::animate_view_to(state, world.center(), zoom, view_size, now);
    if let SearchTarget::Endpoint {
        card: Some(card), ..
    } = target
    {
        state.selected_flow = Some(card.clone());
    }
    true
}

fn cache_id(canvas_id: egui::Id) -> egui::Id {
    canvas_id.with("diagram_search_hits")
}

fn highlight_id(canvas_id: egui::Id) -> egui::Id {
    canvas_id.with("diagram_search_highlight")
}

/// Sidik jari murah atas isi diagram + filter, supaya hasil hanya dihitung
/// ulang bila query, filter, atau isi diagram berubah.
fn fingerprint(state: &DiagramState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    state.search_query.hash(&mut h);
    (
        state.search_tables,
        state.search_columns,
        state.search_groups,
    )
        .hash(&mut h);
    state.nodes.len().hash(&mut h);
    state.groups.len().hash(&mut h);
    (
        state.show_endpoints,
        state.endpoint_display.shows_cards(),
        state.endpoint_links.len(),
        state.flow_cards.len(),
    )
        .hash(&mut h);
    for n in &state.nodes {
        n.title.hash(&mut h);
        n.columns.len().hash(&mut h);
    }
    for g in &state.groups {
        g.title.hash(&mut h);
    }
    h.finish()
}

fn cached_hits(ui: &egui::Ui, canvas_id: egui::Id, state: &DiagramState) -> Arc<SearchHits> {
    let key = fingerprint(state);
    let id = cache_id(canvas_id);
    if let Some((k, hits)) = ui.data(|d| d.get_temp::<(u64, Arc<SearchHits>)>(id))
        && k == key
    {
        return hits;
    }
    let hits = Arc::new(search_hits(state, &state.search_query));
    ui.data_mut(|d| d.insert_temp(id, (key, hits.clone())));
    hits
}

/// Label dengan bagian yang cocok dengan query diwarnai & digarisbawahi.
fn highlighted_label(
    label: &str,
    query: &str,
    color: egui::Color32,
    accent: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(13.0);
    let plain = egui::TextFormat::simple(font.clone(), color);
    let lower = label.to_lowercase();
    let q = query.trim().to_lowercase();
    // Offset byte hanya aman bila lowercase tidak mengubah panjang teks.
    let found = (!q.is_empty() && lower.len() == label.len())
        .then(|| lower.find(&q))
        .flatten()
        .filter(|&i| label.is_char_boundary(i) && label.is_char_boundary(i + q.len()));
    match found {
        Some(i) => {
            let end = i + q.len();
            job.append(&label[..i], 0.0, plain.clone());
            let mut strong = egui::TextFormat::simple(font, accent);
            strong.underline = egui::Stroke::new(1.0, accent);
            job.append(&label[i..end], 0.0, strong);
            job.append(&label[end..], 0.0, plain);
        }
        None => job.append(label, 0.0, plain),
    }
    job
}

/// Gambar sorotan berdenyut pada objek yang baru dipilih dari list.
pub fn draw_highlight(ui: &egui::Ui, state: &DiagramState, canvas_rect: egui::Rect, now: f64) {
    let id = highlight_id(ui.id());
    let Some((target, start)) = ui.data(|d| d.get_temp::<(SearchTarget, f64)>(id)) else {
        return;
    };
    let elapsed = now - start;
    let Some(world) = target_world_rect(state, &target).filter(|_| elapsed < HIGHLIGHT_SECS) else {
        ui.data_mut(|d| d.remove::<(SearchTarget, f64)>(id));
        return;
    };
    let to_screen = |p: egui::Pos2| canvas_rect.min + state.pan + p.to_vec2() * state.zoom;
    let screen = egui::Rect::from_min_max(to_screen(world.min), to_screen(world.max));
    let (_, color) = target.badge();
    // Memudar di akhir, berdenyut selama durasi sorotan.
    let fade = (1.0 - elapsed / HIGHLIGHT_SECS).clamp(0.0, 1.0) as f32;
    let pulse = (0.5 + 0.5 * (elapsed * std::f64::consts::TAU * 1.5).cos()) as f32;
    let grow = 2.0 + 4.0 * pulse;
    ui.painter().rect_stroke(
        screen.expand(grow),
        6.0,
        egui::Stroke::new(2.5, color.gamma_multiply(fade)),
        egui::StrokeKind::Outside,
    );
    ui.painter()
        .rect_filled(screen, 4.0, color.gamma_multiply(0.12 * fade));
    ui.ctx().request_repaint();
}

/// Panel list hasil tepat di bawah search box (`anchor` = rect kartu search).
/// Dipanggil dari dalam `render_diagram` dengan `ui` kanvas.
pub fn render_results_panel(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    canvas_rect: egui::Rect,
    anchor: egui::Rect,
    now: f64,
) {
    if state.search_query.trim().is_empty() {
        return;
    }
    let canvas_id = ui.id();
    let results = cached_hits(ui, canvas_id, state);
    let muted = crate::window_egui::style::nav_text_muted(ui.ctx());
    let text_color = ui.visuals().text_color();
    let accent = ui.visuals().selection.stroke.color;
    let query = state.search_query.clone();

    let top = anchor.bottom() + 4.0;
    let max_height = (canvas_rect.bottom() - top - 40.0).clamp(ROW_HEIGHT, 320.0);
    let mut picked: Option<SearchTarget> = None;

    // Area terpisah (layer sendiri) supaya scroll list tidak men-zoom kanvas
    // dan klik tidak tembus ke node di bawahnya.
    egui::Area::new(canvas_id.with("diagram_search_results"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::pos2(anchor.left(), top))
        .constrain_to(canvas_rect)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(ui.visuals().window_fill)
                .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                .corner_radius(6.0)
                .inner_margin(egui::Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(PANEL_WIDTH - 12.0);
                    if results.hits.is_empty() {
                        ui.label(egui::RichText::new("No matches").color(muted).small());
                        return;
                    }
                    let summary = if results.total > results.hits.len() {
                        format!("{} of {} results", results.hits.len(), results.total)
                    } else if results.total == 1 {
                        "1 result".to_string()
                    } else {
                        format!("{} results", results.total)
                    };
                    ui.label(egui::RichText::new(summary).color(muted).small());
                    ui.add_space(2.0);

                    egui::ScrollArea::vertical()
                        .max_height(max_height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            for hit in &results.hits {
                                if result_row(ui, hit, &query, text_color, muted, accent).clicked()
                                {
                                    picked = Some(hit.target.clone());
                                }
                            }
                        });
                });
        });

    if let Some(target) = picked
        && focus_target(state, &target, canvas_rect.size(), now)
    {
        ui.data_mut(|d| d.insert_temp(highlight_id(canvas_id), (target, now)));
    }
}

fn result_row(
    ui: &mut egui::Ui,
    hit: &SearchHit,
    query: &str,
    text_color: egui::Color32,
    muted: egui::Color32,
    accent: egui::Color32,
) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }

    // Badge jenis objek.
    let (kind, color) = hit.target.badge();
    let badge = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 4.0, rect.center().y - 8.0),
        egui::vec2(BADGE_WIDTH, 16.0),
    );
    ui.painter()
        .rect_filled(badge, 3.0, color.gamma_multiply(0.18));
    ui.painter().rect_stroke(
        badge,
        3.0,
        egui::Stroke::new(1.0, color.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        kind,
        egui::FontId::proportional(9.5),
        color,
    );

    // Nama (dengan sorotan query) dan keterangan di bawahnya.
    let text_left = badge.right() + 8.0;
    let text_width = (rect.right() - text_left - 4.0).max(10.0);
    let mut job = highlighted_label(&hit.label, query, text_color, accent);
    job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
    let name = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter()
        .galley(egui::pos2(text_left, rect.top() + 3.0), name, text_color);
    let mut detail = egui::text::LayoutJob::simple_singleline(
        hit.detail.clone(),
        egui::FontId::proportional(10.5),
        muted,
    );
    detail.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
    let detail = ui.fonts_mut(|f| f.layout_job(detail));
    ui.painter()
        .galley(egui::pos2(text_left, rect.top() + 19.0), detail, muted);

    let tooltip = match &hit.target {
        SearchTarget::Table(_) => "Focus this table",
        SearchTarget::Column { .. } => "Focus this column",
        SearchTarget::Group(_) => "Focus this group",
        SearchTarget::Endpoint { card: Some(_), .. } => "Show the process card of this endpoint",
        SearchTarget::Endpoint { card: None, .. } => "Focus the table this endpoint uses",
    };
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramGroup, DiagramNode};

    fn node(id: &str, columns: &[&str], groups: &[&str], x: f32) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: egui::pos2(x, 0.0),
            size: egui::vec2(200.0, 120.0),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            group_ids: groups.iter().map(|g| g.to_string()).collect(),
            ..Default::default()
        }
    }

    fn fixture() -> DiagramState {
        let mut state = DiagramState {
            nodes: vec![
                node("users", &["id", "email", "user_role"], &["g1"], 0.0),
                node("orders", &["id", "user_id", "total"], &[], 400.0),
            ],
            ..Default::default()
        };
        state.groups.push(DiagramGroup {
            id: "g1".into(),
            title: "User Management".into(),
            color: egui::Color32::RED,
            manual_pos: None,
            repo_url: None,
        });
        state
    }

    #[test]
    fn hits_cover_all_kinds_with_details() {
        let state = fixture();
        let res = search_hits(&state, "user");
        let has = |t: &SearchTarget| res.hits.iter().any(|h| &h.target == t);
        assert!(has(&SearchTarget::Table("users".into())));
        assert!(has(&SearchTarget::Group("g1".into())));
        assert!(has(&SearchTarget::Column {
            table: "orders".into(),
            column: "user_id".into()
        }));
        let col = res
            .hits
            .iter()
            .find(|h| h.label == "user_id")
            .expect("kolom user_id");
        assert_eq!(col.detail, "orders");
        let group = res
            .hits
            .iter()
            .find(|h| h.label == "User Management")
            .expect("group");
        assert_eq!(group.detail, "1 tables");
        // Skor sama → tabel lebih dulu daripada group dan kolom.
        assert!(matches!(res.hits[0].target, SearchTarget::Table(_)));
    }

    #[test]
    fn hits_respect_filters_and_empty_query() {
        let mut state = fixture();
        assert_eq!(search_hits(&state, "  ").total, 0);
        state.search_tables = false;
        state.search_groups = false;
        let res = search_hits(&state, "user");
        assert!(
            res.hits
                .iter()
                .all(|h| matches!(h.target, SearchTarget::Column { .. }))
        );
        assert!(res.total >= 2);
    }

    #[test]
    fn target_rects_follow_objects() {
        let state = fixture();
        let table = target_world_rect(&state, &SearchTarget::Table("orders".into())).unwrap();
        assert_eq!(table.min, egui::pos2(400.0, 0.0));
        let col = target_world_rect(
            &state,
            &SearchTarget::Column {
                table: "users".into(),
                column: "email".into(),
            },
        )
        .unwrap();
        assert!(col.height() < table.height() && col.min.y > 0.0);
        let group = target_world_rect(&state, &SearchTarget::Group("g1".into())).unwrap();
        assert!(group.contains_rect(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(200.0, 120.0)
        )));
        assert!(target_world_rect(&state, &SearchTarget::Table("missing".into())).is_none());
    }

    #[test]
    fn focus_target_starts_animation_toward_object() {
        let mut state = fixture();
        let view = egui::vec2(1000.0, 800.0);
        assert!(focus_target(
            &mut state,
            &SearchTarget::Table("orders".into()),
            view,
            1.0
        ));
        let anim = state.view_anim.as_ref().expect("animasi dimulai");
        // Tengah tabel orders (500, 60) jatuh di tengah viewport.
        let center = anim.to_pan + egui::vec2(500.0, 60.0) * anim.to_zoom;
        assert!((center - view / 2.0).length() < 0.01);
        assert!(!focus_target(
            &mut state,
            &SearchTarget::Group("nope".into()),
            view,
            1.0
        ));
    }

    #[test]
    fn finds_endpoints_and_points_to_their_table() {
        let mut state = fixture();
        state
            .endpoint_links
            .push(crate::models::structs::EndpointLink {
                table: "orders".into(),
                method: "POST".into(),
                path: "/api/orders".into(),
                summary: "Create order".into(),
                request_id: None,
                repo_key: None,
                source: None,
            });
        let hits = search_hits(&state, "api/orders");
        let hit = hits
            .hits
            .iter()
            .find(|h| matches!(h.target, SearchTarget::Endpoint { .. }))
            .expect("endpoint hit");
        assert_eq!(hit.label, "POST /api/orders");
        assert_eq!(hit.detail, "uses orders");
        assert!(target_world_rect(&state, &hit.target).is_some());

        // Mode Cards: satu hasil per card, melompat ke card.
        state
            .endpoint_links
            .push(crate::models::structs::EndpointLink {
                table: "users".into(),
                ..state.endpoint_links[0].clone()
            });
        crate::diagram_flow::sync_cards_from_links(&mut state);
        let hits = search_hits(&state, "api/orders");
        let endpoints: Vec<&SearchHit> = hits
            .hits
            .iter()
            .filter(|h| matches!(h.target, SearchTarget::Endpoint { .. }))
            .collect();
        assert_eq!(endpoints.len(), 1);
        let SearchTarget::Endpoint {
            card: Some(card), ..
        } = &endpoints[0].target
        else {
            panic!("endpoint hit must point to its card");
        };
        let rect = target_world_rect(&state, &endpoints[0].target).unwrap();
        assert_eq!(
            Some(rect),
            crate::diagram_flow_layout::card_world_rect(&state, card)
        );
        assert!(focus_target(
            &mut state,
            &endpoints[0].target.clone(),
            egui::vec2(800.0, 600.0),
            0.0
        ));
        assert_eq!(state.selected_flow.as_deref(), Some(card.as_str()));

        state.endpoint_display = crate::models::structs::EndpointDisplay::Badges;
        let hits = search_hits(&state, "api/orders");
        assert!(
            hits.hits
                .iter()
                .any(|h| matches!(h.target, SearchTarget::Endpoint { card: None, .. }))
        );

        state.show_endpoints = false;
        assert!(
            search_hits(&state, "api/orders")
                .hits
                .iter()
                .all(|h| !matches!(h.target, SearchTarget::Endpoint { .. }))
        );
    }
}
