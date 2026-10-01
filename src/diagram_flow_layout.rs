//! Geometri flow card di kanvas diagram: ukuran card, baris langkah, dan
//! penempatan card sorotan di samping tabel yang disentuhnya.
//!
//! Kanvas hanya menampilkan satu card: endpoint yang dipilih di API rail
//! (card sorotan). Card itu ditata otomatis dan tidak bisa digeser.
//!
//! Semua koordinat di sini adalah koordinat diagram (zoom 1.0). Modul ini
//! hanya memakai tipe geometri egui (`Rect`, `Pos2`), tanpa `Ui`, supaya
//! mudah dites.

use eframe::egui;

use crate::diagram_lod::Lod;
use crate::models::structs::{DiagramState, FlowCard, FlowStep, FlowTrigger, FlowTriggerKind};

/// Lebar card.
pub const CARD_WIDTH: f32 = 300.0;
/// Tinggi header (chip method + path).
pub const CARD_HEADER_H: f32 = 34.0;
/// Tinggi satu baris ringkasan.
pub const CARD_SUMMARY_H: f32 = 22.0;
/// Tinggi satu baris langkah.
pub const STEP_ROW_H: f32 = 22.0;
/// Tinggi satu baris detail langkah yang dibuka.
pub const STEP_DETAIL_H: f32 = 17.0;
/// Padding atas + bawah blok detail langkah.
pub const STEP_DETAIL_PAD: f32 = 6.0;
/// Batas karakter per baris detail langkah (teks 10,5 px di card 300).
const STEP_DETAIL_CHARS: usize = 40;
/// Padding bawah card.
pub const CARD_PAD_BOTTOM: f32 = 6.0;
/// Sisi titik card pada LOD `Overview`.
pub const OVERVIEW_DOT: f32 = 34.0;
/// Jarak card sorotan tanpa tabel di diagram ke isi diagram di kanannya.
const FREE_SPOT_GAP: f32 = 140.0;
/// Jarak card sorotan (mode rail) ke tabel-tabelnya; ruang untuk garis dan
/// chip langkahnya.
pub const SPOT_GAP: f32 = 90.0;
/// Jarak aman card sorotan ke tabel lain saat menilai tumpang tindih.
const SPOT_CLEARANCE: f32 = 12.0;
/// Perkiraan tinggi bagian bawah card terbuka tanpa baris tabel
/// (pemisah, judul "Tables involved", margin).
const FOOTER_BASE_H: f32 = 36.0;
/// Perkiraan tinggi satu baris tabel di bagian bawah card terbuka (nama
/// tabel panjang bisa terbungkus dua baris).
const FOOTER_ROW_H: f32 = 36.0;

/// Tinggi baris ringkasan (0 bila ringkasan kosong).
pub fn summary_height(card: &FlowCard) -> f32 {
    if card.summary.trim().is_empty() {
        0.0
    } else {
        CARD_SUMMARY_H
    }
}

/// Tinggi card lengkap (header, ringkasan, semua langkah) pada LOD `Detail`.
pub fn body_height(card: &FlowCard) -> f32 {
    // Card tanpa langkah punya satu baris "No business process yet".
    let rows = card.steps.len().max(1);
    CARD_HEADER_H + summary_height(card) + rows as f32 * STEP_ROW_H + CARD_PAD_BOTTOM
}

/// Pecah `text` ke baris paling banyak `width` karakter, per kata.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Baris detail langkah yang dibuka di card: kolom, kondisi, keterangan
/// (dipecah per kata), dan lokasi kode. Kosong bila langkah tidak punya detail.
pub fn step_detail_lines(step: &FlowStep) -> Vec<String> {
    let mut out = Vec::new();
    if !step.columns.is_empty() {
        out.extend(wrap_words(
            &format!("Columns: {}", step.columns.join(", ")),
            STEP_DETAIL_CHARS,
        ));
    }
    if let Some(c) = step.condition.as_deref().filter(|c| !c.trim().is_empty()) {
        out.extend(wrap_words(&format!("When: {c}"), STEP_DETAIL_CHARS));
    }
    if !step.detail.trim().is_empty() {
        out.extend(wrap_words(&step.detail, STEP_DETAIL_CHARS));
    }
    if let Some(s) = step.source.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push(format!("Source: {s}"));
    }
    out
}

/// Tinggi blok detail langkah yang dibuka (0 bila tanpa detail).
pub fn step_detail_height(step: &FlowStep) -> f32 {
    let n = step_detail_lines(step).len();
    if n == 0 {
        0.0
    } else {
        n as f32 * STEP_DETAIL_H + STEP_DETAIL_PAD
    }
}

/// Langkah yang detailnya dibuka di card ini beserta tinggi bloknya. Hanya
/// card terpilih yang badannya tampil.
pub fn open_step(state: &DiagramState, card: &FlowCard) -> Option<(usize, f32)> {
    if !is_selected(state, card) {
        return None;
    }
    let i = state.flow_open_step?;
    let h = step_detail_height(card.steps.get(i)?);
    (h > 0.0).then_some((i, h))
}

/// Card terpilih? Hanya card terpilih yang badannya (ringkasan dan langkah)
/// tampil; card sorotan lain (fokus, pemutaran) hanya header.
pub fn is_selected(state: &DiagramState, card: &FlowCard) -> bool {
    state.selected_flow.as_deref() == Some(card.id.as_str())
}

/// Tinggi yang digambar pada LOD `Detail`. Card terpilih diperluas: semua
/// langkah, detail langkah yang dibuka, dan bagian bawahnya (tombol + tabel,
/// `state.flow_footer_h`) di dalam bingkai yang sama.
pub fn drawn_height(state: &DiagramState, card: &FlowCard) -> f32 {
    if is_selected(state, card) {
        let open = open_step(state, card).map_or(0.0, |(_, h)| h);
        body_height(card) + open + state.flow_footer_h
    } else {
        CARD_HEADER_H
    }
}

/// Perkiraan tinggi card saat dibuka (semua langkah plus daftar tabel di
/// bagian bawahnya), tidak bergantung pada card mana yang terpilih. Dipakai
/// untuk mencari tempat card sorotan yang tidak menutupi tabel.
pub fn expanded_height(state: &DiagramState, card: &FlowCard) -> f32 {
    let rows = crate::diagram_flow::tables_of(state, card).len().max(1);
    body_height(card) + FOOTER_BASE_H + rows as f32 * FOOTER_ROW_H
}

/// Rect baris langkah ke-`index` di card `rect`; `None` bila langkah itu
/// tidak ada. `open` = langkah yang detailnya dibuka dan tinggi bloknya
/// (lihat [`open_step`]); baris sesudahnya turun sebesar itu. Pemanggil
/// memastikan badan card memang tampil.
pub fn step_row_rect(
    rect: egui::Rect,
    card: &FlowCard,
    index: usize,
    open: Option<(usize, f32)>,
) -> Option<egui::Rect> {
    if index >= card.steps.len() {
        return None;
    }
    let shift = open.filter(|&(o, _)| o < index).map_or(0.0, |(_, h)| h);
    let top = rect.top() + CARD_HEADER_H + summary_height(card) + index as f32 * STEP_ROW_H + shift;
    Some(egui::Rect::from_min_size(
        egui::pos2(rect.left(), top),
        egui::vec2(rect.width(), STEP_ROW_H),
    ))
}

/// Rect yang benar-benar digambar untuk LOD `lod`: card penuh, pil
/// setinggi header, atau titik.
pub fn lod_rect(rect: egui::Rect, lod: Lod) -> egui::Rect {
    match lod {
        Lod::Detail => rect,
        Lod::Compact => {
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), CARD_HEADER_H))
        }
        Lod::Overview => {
            egui::Rect::from_min_size(rect.min, egui::vec2(OVERVIEW_DOT, OVERVIEW_DOT))
        }
    }
}

/// Resource sebuah trigger: segmen path pertama setelah awalan umum (`api`,
/// `v1`, parameter), mis. `/api/v1/panens/{id}` → `/panens`. Trigger non-HTTP
/// dikelompokkan per jenisnya.
pub fn resource_of(trigger: &FlowTrigger) -> String {
    if trigger.kind != FlowTriggerKind::Http {
        return format!("{:?}", trigger.kind).to_lowercase();
    }
    let path = trigger.target.split(['?', '#']).next().unwrap_or_default();
    let is_prefix = |s: &str| {
        let l = s.to_ascii_lowercase();
        l == "api"
            || l == "rest"
            || (l.len() > 1 && l.starts_with('v') && l[1..].chars().all(|c| c.is_ascii_digit()))
            || s.starts_with(['{', ':', '<', '['])
    };
    match path
        .split('/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .find(|s| !is_prefix(s))
    {
        Some(s) => format!("/{s}"),
        None => "/".into(),
    }
}

/// Posisi seluruh card pada satu frame, sejajar `state.flow_cards`.
#[derive(Clone, Debug, Default)]
pub struct FlowFrame {
    /// Rect tata letak tiap card (tinggi tanpa perluasan pilihan).
    pub rects: Vec<egui::Rect>,
    /// Tabel yang disentuh tiap card (link + langkah), hanya yang ada di diagram.
    pub tables: Vec<Vec<String>>,
    /// Card yang digambar di kanvas, sejajar `rects`: hanya card sorotan.
    pub shown: Vec<bool>,
}

impl FlowFrame {
    /// Hitung tabel semua card dan posisi card sorotan (di samping
    /// tabel-tabelnya).
    pub fn compute(state: &DiagramState) -> Self {
        if state.flow_cards.is_empty() {
            return Self::default();
        }
        let by_card = crate::diagram_flow::tables_by_card(state);
        let ids: std::collections::HashSet<&str> =
            state.nodes.iter().map(|n| n.id.as_str()).collect();
        let tables: Vec<Vec<String>> = state
            .flow_cards
            .iter()
            .map(|c| {
                by_card
                    .get(c.id.as_str())
                    .into_iter()
                    .flatten()
                    .filter(|t| ids.contains(*t))
                    .map(|t| t.to_string())
                    .collect()
            })
            .collect();
        let n = state.flow_cards.len();
        let header = egui::vec2(CARD_WIDTH, CARD_HEADER_H);
        let mut rects = vec![egui::Rect::from_min_size(egui::Pos2::ZERO, header); n];
        let mut shown = vec![false; n];
        if let Some(i) = spotlight_index(state) {
            let size = egui::vec2(CARD_WIDTH, expanded_height(state, &state.flow_cards[i]));
            rects[i] = egui::Rect::from_min_size(spotlight_pos(state, &tables[i], size), header);
            shown[i] = true;
        }
        Self {
            rects,
            tables,
            shown,
        }
    }

    /// Card `i` digambar di kanvas?
    pub fn is_shown(&self, i: usize) -> bool {
        self.shown.get(i).copied().unwrap_or(false)
    }

    /// Rect card `i` di LOD `Detail`, diperluas bila card itu terpilih.
    pub fn drawn_rect(&self, state: &DiagramState, i: usize) -> egui::Rect {
        let card = &state.flow_cards[i];
        let r = self.rects[i];
        egui::Rect::from_min_size(r.min, egui::vec2(r.width(), drawn_height(state, card)))
    }
}

/// Card sorotan: card terpilih, lalu yang difokuskan, lalu yang sedang
/// diputar.
pub fn spotlight_index(state: &DiagramState) -> Option<usize> {
    let id = state
        .selected_flow
        .as_deref()
        .or(state.focus_flow.as_deref())
        .or(state.flow_play.as_ref().map(|p| p.card_id.as_str()))?;
    state.flow_cards.iter().position(|c| c.id == id)
}

/// Pojok kiri atas card sorotan berukuran `size` untuk tabel `tables` (id
/// node). Dicoba berurutan: kiri dan kanan kotak tabel-tabelnya, kiri dan
/// kanan tabel pertamanya, lalu atas dan bawah kotak itu; dipakai tempat
/// pertama yang tidak menutupi tabel mana pun. Bila semuanya menutupi tabel,
/// dipakai yang paling sedikit menutupinya. Card tanpa tabel di diagram
/// ditaruh di kiri seluruh isi diagram.
pub fn spotlight_pos(state: &DiagramState, tables: &[String], size: egui::Vec2) -> egui::Pos2 {
    let rect_of =
        |n: &crate::models::structs::DiagramNode| egui::Rect::from_min_size(n.pos, n.size);
    let all: Vec<egui::Rect> = state.nodes.iter().map(rect_of).collect();
    let touched: Vec<egui::Rect> = tables
        .iter()
        .filter_map(|t| state.nodes.iter().find(|n| &n.id == t))
        .map(rect_of)
        .collect();
    let Some(bbox) = touched.iter().copied().reduce(|a, b| a.union(b)) else {
        return match all.iter().copied().reduce(|a, b| a.union(b)) {
            Some(a) => egui::pos2(a.left() - FREE_SPOT_GAP - size.x, a.top()),
            None => egui::Pos2::ZERO,
        };
    };
    let mean = touched
        .iter()
        .fold(egui::Vec2::ZERO, |acc, r| acc + r.center().to_vec2())
        / touched.len() as f32;
    let first = touched[0];
    let candidates = [
        egui::pos2(bbox.left() - SPOT_GAP - size.x, mean.y - size.y / 2.0),
        egui::pos2(bbox.right() + SPOT_GAP, mean.y - size.y / 2.0),
        egui::pos2(first.left() - SPOT_GAP - size.x, first.top()),
        egui::pos2(first.right() + SPOT_GAP, first.top()),
        egui::pos2(mean.x - size.x / 2.0, bbox.top() - SPOT_GAP - size.y),
        egui::pos2(mean.x - size.x / 2.0, bbox.bottom() + SPOT_GAP),
    ];
    // Luas tabel yang tertutup card (dengan jarak aman) di posisi `pos`.
    let overlap = |pos: egui::Pos2| -> f32 {
        let padded = egui::Rect::from_min_size(pos, size).expand(SPOT_CLEARANCE);
        all.iter()
            .map(|r| {
                let i = r.intersect(padded);
                if i.is_positive() { i.area() } else { 0.0 }
            })
            .sum()
    };
    let mut best = (candidates[0], overlap(candidates[0]));
    for &pos in &candidates[1..] {
        if best.1 <= 0.0 {
            break;
        }
        let o = overlap(pos);
        if o < best.1 {
            best = (pos, o);
        }
    }
    best.0
}

/// Card bisa tampil di kanvas (mode rail, endpoint tidak disembunyikan)?
pub fn cards_visible(state: &DiagramState) -> bool {
    state.show_endpoints && state.endpoint_display.shows_rail() && !state.flow_cards.is_empty()
}

/// Rect card `card_id` (koordinat diagram), seperti yang digambar di LOD
/// `Detail`. Dipakai pencarian dan panel endpoint untuk melompat ke card.
pub fn card_world_rect(state: &DiagramState, card_id: &str) -> Option<egui::Rect> {
    let i = state.flow_cards.iter().position(|c| c.id == card_id)?;
    let frame = FlowFrame::compute(state);
    if !frame.is_shown(i) {
        // Tempat card ini bila menjadi card sorotan.
        let size = egui::vec2(CARD_WIDTH, expanded_height(state, &state.flow_cards[i]));
        let pos = spotlight_pos(state, &frame.tables[i], size);
        return Some(egui::Rect::from_min_size(pos, size));
    }
    Some(frame.drawn_rect(state, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramNode, EndpointLink, FlowStep};

    fn node(id: &str, x: f32, y: f32) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            pos: egui::pos2(x, y),
            size: egui::vec2(200.0, 150.0),
            ..Default::default()
        }
    }

    fn card(id: &str, method: &str, path: &str, repo: Option<&str>) -> FlowCard {
        FlowCard {
            id: id.into(),
            trigger: FlowTrigger {
                kind: FlowTriggerKind::Http,
                method: method.into(),
                target: path.into(),
            },
            repo_key: repo.map(str::to_string),
            ..Default::default()
        }
    }

    fn link(table: &str, method: &str, path: &str, repo: Option<&str>) -> EndpointLink {
        EndpointLink {
            table: table.into(),
            method: method.into(),
            path: path.into(),
            summary: String::new(),
            request_id: None,
            repo_key: repo.map(str::to_string),
            source: None,
        }
    }

    fn steps(n: usize) -> Vec<FlowStep> {
        (0..n)
            .map(|i| FlowStep {
                title: format!("step {i}"),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn body_height_counts_summary_and_all_steps() {
        let mut c = card("flw_1", "GET", "/a", None);
        assert_eq!(
            body_height(&c),
            CARD_HEADER_H + STEP_ROW_H + CARD_PAD_BOTTOM
        );
        c.summary = "Lists things".into();
        c.steps = steps(12);
        assert_eq!(
            body_height(&c),
            CARD_HEADER_H + CARD_SUMMARY_H + 12.0 * STEP_ROW_H + CARD_PAD_BOTTOM
        );
    }

    #[test]
    fn step_rows_follow_header_and_summary() {
        let mut c = card("flw_1", "GET", "/a", None);
        c.summary = "s".into();
        c.steps = steps(10);
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 100.0), egui::vec2(CARD_WIDTH, 400.0));
        let row = step_row_rect(r, &c, 2, None).unwrap();
        assert_eq!(
            row.top(),
            100.0 + CARD_HEADER_H + CARD_SUMMARY_H + 2.0 * STEP_ROW_H
        );
        assert!(step_row_rect(r, &c, 9, None).is_some());
        assert!(step_row_rect(r, &c, 10, None).is_none());
        // Detail langkah 1 yang dibuka mendorong baris sesudahnya saja.
        let open = Some((1, 40.0));
        assert_eq!(
            step_row_rect(r, &c, 1, open).unwrap().top(),
            row.top() - STEP_ROW_H
        );
        assert_eq!(
            step_row_rect(r, &c, 2, open).unwrap().top(),
            row.top() + 40.0
        );
    }

    #[test]
    fn open_step_adds_its_detail_to_the_selected_card() {
        let mut st = DiagramState::default();
        let mut c = card("flw_1", "GET", "/a", None);
        c.steps = steps(3);
        c.steps[1].detail = "word ".repeat(30);
        c.steps[1].columns = vec!["id".into()];
        c.steps[1].source = Some("route.go:25".into());
        st.flow_cards.push(c);
        st.selected_flow = Some("flw_1".into());
        let base = drawn_height(&st, &st.flow_cards[0]);

        st.flow_open_step = Some(1);
        let lines = step_detail_lines(&st.flow_cards[0].steps[1]);
        // Kolom, keterangan 150 karakter (3-4 baris), sumber.
        assert!(lines.len() >= 5, "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|l| l.chars().count() <= STEP_DETAIL_CHARS + 20)
        );
        assert_eq!(
            drawn_height(&st, &st.flow_cards[0]),
            base + lines.len() as f32 * STEP_DETAIL_H + STEP_DETAIL_PAD
        );
        // Langkah tanpa detail tidak menambah tinggi; card lain tidak terpengaruh.
        st.flow_open_step = Some(0);
        assert_eq!(drawn_height(&st, &st.flow_cards[0]), base);
        st.flow_open_step = Some(1);
        st.selected_flow = None;
        assert!(open_step(&st, &st.flow_cards[0]).is_none());
    }

    #[test]
    fn body_is_drawn_only_for_the_selected_card() {
        let mut st = DiagramState::default();
        let mut c = card("flw_1", "GET", "/a", None);
        c.steps = steps(3);
        st.flow_cards.push(c);
        let c = &st.flow_cards[0];
        assert!(!is_selected(&st, c));
        assert_eq!(drawn_height(&st, c), CARD_HEADER_H);

        st.selected_flow = Some("flw_1".into());
        let c = &st.flow_cards[0];
        assert!(is_selected(&st, c));
        assert_eq!(drawn_height(&st, c), body_height(c));
    }

    #[test]
    fn resource_skips_api_prefix_version_and_params() {
        let r = |p: &str| resource_of(&card("x", "GET", p, None).trigger);
        assert_eq!(r("/api/v1/panens/{id}"), "/panens");
        assert_eq!(r("/panen-details"), "/panen-details");
        assert_eq!(r("/v2/users?x=1"), "/users");
        assert_eq!(r("/{tenant}/orders/:id"), "/orders");
        assert_eq!(r("/"), "/");
        let mut job = card("j", "", "SendMail", None);
        job.trigger.kind = FlowTriggerKind::Job;
        assert_eq!(resource_of(&job.trigger), "job");
    }

    #[test]
    fn cards_hidden_in_badges_mode_or_when_endpoints_are_off() {
        let mut st = rail_state(vec![node("users", 600.0, 300.0)]);
        assert!(cards_visible(&st));
        st.endpoint_display = crate::models::structs::EndpointDisplay::Badges;
        assert!(!cards_visible(&st));
        st.endpoint_display = crate::models::structs::EndpointDisplay::Rail;
        st.show_endpoints = false;
        assert!(!cards_visible(&st));
    }

    /// State mode rail: satu card `flw_1` yang tertaut ke tabel `users`.
    fn rail_state(nodes: Vec<DiagramNode>) -> DiagramState {
        DiagramState {
            nodes,
            flow_cards: vec![card("flw_1", "GET", "/users", None)],
            endpoint_links: vec![link("users", "GET", "/users", None)],
            ..Default::default()
        }
    }

    #[test]
    fn rail_mode_places_only_the_selected_card_beside_its_table() {
        let mut st = rail_state(vec![node("users", 600.0, 300.0)]);
        assert!(st.endpoint_display.shows_rail());
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.shown, vec![false]);

        st.selected_flow = Some("flw_1".into());
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.shown, vec![true]);
        let table = egui::Rect::from_min_size(egui::pos2(600.0, 300.0), egui::vec2(200.0, 150.0));
        let drawn = frame.drawn_rect(&st, 0);
        // Di kiri tabel dengan jarak `SPOT_GAP`, tidak menutupinya.
        assert_eq!(drawn.right(), table.left() - SPOT_GAP);
        assert!(!drawn.intersects(table));
        assert_eq!(
            card_world_rect(&st, "flw_1").map(|r| r.min),
            Some(drawn.min)
        );
    }

    #[test]
    fn spotlight_moves_right_when_the_left_side_is_taken() {
        let mut st = rail_state(vec![
            node("users", 600.0, 300.0),
            node("blocker", 250.0, 300.0),
        ]);
        st.selected_flow = Some("flw_1".into());
        let frame = FlowFrame::compute(&st);
        let drawn = frame.drawn_rect(&st, 0);
        assert_eq!(drawn.left(), 800.0 + SPOT_GAP);
        for n in &st.nodes {
            assert!(!drawn.intersects(egui::Rect::from_min_size(n.pos, n.size)));
        }
    }

    #[test]
    fn spotlight_without_diagram_tables_sits_left_of_the_diagram() {
        let mut st = rail_state(vec![node("orders", 600.0, 300.0)]);
        st.endpoint_links.clear();
        let size = egui::vec2(CARD_WIDTH, 100.0);
        let pos = spotlight_pos(&st, &[], size);
        assert_eq!(pos, egui::pos2(600.0 - FREE_SPOT_GAP - CARD_WIDTH, 300.0));
        st.nodes.clear();
        assert_eq!(spotlight_pos(&st, &[], size), egui::Pos2::ZERO);
    }

    #[test]
    fn rail_card_rect_is_known_before_the_card_is_selected() {
        let st = rail_state(vec![node("users", 600.0, 300.0)]);
        let rect = card_world_rect(&st, "flw_1").expect("card");
        assert_eq!(rect.right(), 600.0 - SPOT_GAP);
        assert_eq!(rect.height(), expanded_height(&st, &st.flow_cards[0]));
    }

    #[test]
    fn lod_rect_shrinks_for_small_zoom() {
        let r = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(CARD_WIDTH, 200.0));
        assert_eq!(lod_rect(r, Lod::Detail), r);
        assert_eq!(lod_rect(r, Lod::Compact).height(), CARD_HEADER_H);
        assert_eq!(lod_rect(r, Lod::Overview).width(), OVERVIEW_DOT);
    }
}
