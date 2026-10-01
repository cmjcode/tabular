//! Geometri flow card di kanvas diagram: ukuran card, baris langkah, dan
//! penataan otomatis card ke pita API di atas tabel yang disentuhnya.
//!
//! Tata letaknya mengikuti diagram arsitektur berlapis: tiap group adalah
//! satu service dengan lapisan API (card endpoint) di atas lapisan DATA
//! (tabel), di dalam bingkai group yang sama. Card dikelompokkan per resource
//! (segmen path pertama) dan tumpukan resource diurutkan menurut posisi
//! tabelnya, supaya garis ke tabel pendek dan jarang bersilangan.
//!
//! Semua koordinat di sini adalah koordinat diagram (zoom 1.0). Modul ini
//! hanya memakai tipe geometri egui (`Rect`, `Pos2`), tanpa `Ui`, supaya
//! mudah dites.

use std::collections::{BTreeMap, HashMap};

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
/// Langkah yang tampil di card tidak terpilih; sisanya "+n more".
pub const CARD_MAX_STEPS_SHOWN: usize = 8;
/// Padding bawah card.
pub const CARD_PAD_BOTTOM: f32 = 6.0;
/// Sisi titik card pada LOD `Overview`.
pub const OVERVIEW_DOT: f32 = 34.0;
/// Jarak tepi bawah pita API ke tabel teratas; ruang garis pemisah API/DATA.
pub const BAND_GAP: f32 = 48.0;
/// Tinggi judul lapisan ("API") di atas tumpukan card.
pub const BAND_LABEL_H: f32 = 26.0;
/// Tinggi judul tumpukan resource.
pub const STACK_HEADER_H: f32 = 22.0;
/// Jarak vertikal antar card dalam satu tumpukan.
pub const STACK_CARD_GAP: f32 = 8.0;
/// Jarak vertikal antar tumpukan dalam satu kolom.
pub const STACK_GAP_Y: f32 = 20.0;
/// Jarak horizontal antar kolom pita.
pub const BAND_COLUMN_GAP: f32 = 28.0;
/// Pita dibuat minimal selebar `BAND_ASPECT` kali tingginya.
const BAND_ASPECT: f32 = 1.6;
/// Jarak pita "Unmapped endpoints" ke isi diagram di kanannya.
const FREE_BAND_GAP: f32 = 140.0;
/// Jarak card sorotan (mode rail) ke tabel-tabelnya; ruang untuk garis dan
/// chip langkahnya.
pub const SPOT_GAP: f32 = 90.0;
/// Jarak aman card sorotan ke tabel lain saat menilai tumpang tindih.
const SPOT_CLEARANCE: f32 = 12.0;
/// Padding kotak group, sama dengan `diagram_view` (sisi, lalu judul di atas).
const GROUP_SIDE_PAD: f32 = 20.0;
const GROUP_TOP_PAD: f32 = GROUP_SIDE_PAD + 30.0 + 16.0;
/// Perkiraan tinggi bagian bawah card terbuka tanpa baris tabel
/// (pemisah, judul "Tables involved", margin).
const FOOTER_BASE_H: f32 = 36.0;
/// Perkiraan tinggi satu baris tabel di bagian bawah card terbuka (nama
/// tabel panjang bisa terbungkus dua baris).
const FOOTER_ROW_H: f32 = 36.0;

/// Ukuran pita API per id group, untuk perhitungan kotak group di luar
/// modul ini (tumpang tindih, pemadatan, bingkai).
pub type BandSizes = HashMap<String, egui::Vec2>;

/// Jumlah langkah yang tampil dan yang disembunyikan ("+n more").
/// `expanded` = card terpilih, semua langkah tampil.
pub fn step_rows(card: &FlowCard, expanded: bool) -> (usize, usize) {
    let n = card.steps.len();
    if expanded || n <= CARD_MAX_STEPS_SHOWN {
        (n, 0)
    } else {
        (CARD_MAX_STEPS_SHOWN, n - CARD_MAX_STEPS_SHOWN)
    }
}

/// Tinggi baris ringkasan (0 bila ringkasan kosong).
pub fn summary_height(card: &FlowCard) -> f32 {
    if card.summary.trim().is_empty() {
        0.0
    } else {
        CARD_SUMMARY_H
    }
}

/// Tinggi card lengkap (header, ringkasan, langkah) pada LOD `Detail`.
pub fn body_height(card: &FlowCard, expanded: bool) -> f32 {
    let (shown, hidden) = step_rows(card, expanded);
    // Card tanpa langkah punya satu baris "No business process yet".
    let rows = if card.steps.is_empty() {
        1
    } else {
        shown + usize::from(hidden > 0)
    };
    CARD_HEADER_H + summary_height(card) + rows as f32 * STEP_ROW_H + CARD_PAD_BOTTOM
}

/// Tinggi card pada LOD `Detail`; card yang diciutkan hanya header.
pub fn card_height(card: &FlowCard, expanded: bool) -> f32 {
    if card.collapsed {
        CARD_HEADER_H
    } else {
        body_height(card, expanded)
    }
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
    if !is_selected(state, card) || !shows_body(state, card) {
        return None;
    }
    let i = state.flow_open_step?;
    let h = step_detail_height(card.steps.get(i)?);
    (h > 0.0).then_some((i, h))
}

fn is_selected(state: &DiagramState, card: &FlowCard) -> bool {
    state.selected_flow.as_deref() == Some(card.id.as_str())
}

/// Badan card (ringkasan dan langkah) tampil? Mode ringkas (default): hanya
/// card terpilih. Mode "show steps": semua card kecuali yang diciutkan.
pub fn shows_body(state: &DiagramState, card: &FlowCard) -> bool {
    // Mode rail: hanya card sorotan yang tampil, dan selalu lengkap.
    if state.flow_show_steps && !state.endpoint_display.is_rail() {
        !card.collapsed
    } else {
        is_selected(state, card)
    }
}

/// Tinggi yang digambar pada LOD `Detail`. Card terpilih diperluas: semua
/// langkah, detail langkah yang dibuka, dan bagian bawahnya (tombol + tabel,
/// `state.flow_footer_h`) di dalam bingkai yang sama.
pub fn drawn_height(state: &DiagramState, card: &FlowCard) -> f32 {
    let selected = is_selected(state, card);
    let base = if shows_body(state, card) {
        let open = open_step(state, card).map_or(0.0, |(_, h)| h);
        body_height(card, selected) + open
    } else {
        CARD_HEADER_H
    };
    if selected {
        base + state.flow_footer_h
    } else {
        base
    }
}

/// Perkiraan tinggi card saat dibuka (semua langkah plus daftar tabel di
/// bagian bawahnya), tidak bergantung pada card mana yang terpilih. Pita
/// mencadangkan ruang setinggi ini supaya card yang dibuka tidak menutupi
/// tabel.
pub fn expanded_height(state: &DiagramState, card: &FlowCard) -> f32 {
    let rows = crate::diagram_flow::tables_of(state, card).len().max(1);
    body_height(card, true) + FOOTER_BASE_H + rows as f32 * FOOTER_ROW_H
}

/// Tinggi card di tata letak. Pilihan card tidak mengubah tata letak; card
/// terpilih digambar di atas tetangganya.
pub fn layout_height(state: &DiagramState, card: &FlowCard) -> f32 {
    if state.flow_show_steps && !card.collapsed {
        body_height(card, false)
    } else {
        CARD_HEADER_H
    }
}

/// Rect baris langkah ke-`index` di card `rect`; `None` bila langkah itu di
/// balik "+n more". `open` = langkah yang detailnya dibuka dan tinggi
/// bloknya (lihat [`open_step`]); baris sesudahnya turun sebesar itu.
/// Pemanggil memastikan badan card memang tampil.
pub fn step_row_rect(
    rect: egui::Rect,
    card: &FlowCard,
    index: usize,
    expanded: bool,
    open: Option<(usize, f32)>,
) -> Option<egui::Rect> {
    if index >= step_rows(card, expanded).0 {
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

/// Pemilik pita: group diagram, repository tanpa group, atau card yang tidak
/// menyentuh tabel diagram.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BandOwner {
    Group(String),
    Repo(Option<String>),
    Unmapped,
}

/// Judul satu tumpukan resource di pita.
#[derive(Clone, Debug, PartialEq)]
pub struct BandStack {
    pub resource: String,
    pub header: egui::Rect,
    pub count: usize,
}

/// Satu pita API hasil penataan (koordinat diagram).
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub owner: BandOwner,
    /// Rect pita: judul lapisan plus semua tumpukan.
    pub rect: egui::Rect,
    /// `rect` diperpanjang ke bawah sampai dasar card terpanjang saat dibuka.
    /// Ruang ini dijaga bebas tabel.
    pub reach: egui::Rect,
    /// Kotak tabel yang dilayani pita; `None` untuk `Unmapped`.
    pub tables: Option<egui::Rect>,
    pub stacks: Vec<BandStack>,
}

/// Posisi seluruh card pada satu frame, sejajar `state.flow_cards`.
#[derive(Clone, Debug, Default)]
pub struct FlowFrame {
    /// Rect tata letak tiap card (tinggi tanpa perluasan pilihan).
    pub rects: Vec<egui::Rect>,
    /// Tabel yang disentuh tiap card (link + langkah), hanya yang ada di diagram.
    pub tables: Vec<Vec<String>>,
    /// Pita API; card yang digeser manual tidak masuk pita mana pun.
    pub bands: Vec<Band>,
    /// Card yang digambar di kanvas, sejajar `rects`. Mode pita: semuanya.
    /// Mode rail: hanya card sorotan.
    pub shown: Vec<bool>,
    /// Mode rail: card ditata otomatis di samping tabelnya dan tidak bisa
    /// digeser.
    pub spotlight: bool,
}

impl FlowFrame {
    /// Hitung tabel dan posisi semua card.
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
        if state.endpoint_display.is_rail() {
            return Self::spotlight(state, tables);
        }
        let (rects, bands) = arrange_bands(state, &tables);
        Self {
            shown: vec![true; rects.len()],
            rects,
            tables,
            bands,
            spotlight: false,
        }
    }

    /// Mode rail: tanpa pita; hanya card sorotan yang mendapat tempat, di
    /// samping tabel-tabelnya.
    fn spotlight(state: &DiagramState, tables: Vec<Vec<String>>) -> Self {
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
            bands: Vec::new(),
            shown,
            spotlight: true,
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

    /// Ukuran pita tiap group.
    pub fn band_sizes(&self) -> BandSizes {
        self.bands
            .iter()
            .filter_map(|b| match &b.owner {
                BandOwner::Group(g) => Some((g.clone(), b.reach.size())),
                _ => None,
            })
            .collect()
    }
}

/// Card sorotan mode rail: card terpilih, lalu yang difokuskan, lalu yang
/// sedang diputar.
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
            Some(a) => egui::pos2(a.left() - FREE_BAND_GAP - size.x, a.top()),
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

/// Card sedang tampil di kanvas (bukan hanya badge atau disembunyikan)?
pub fn cards_visible(state: &DiagramState) -> bool {
    state.show_endpoints && state.endpoint_display.shows_cards() && !state.flow_cards.is_empty()
}

/// Ukuran pita tiap group untuk state saat ini; kosong bila card tidak tampil.
pub fn band_sizes(state: &DiagramState) -> BandSizes {
    if cards_visible(state) {
        FlowFrame::compute(state).band_sizes()
    } else {
        BandSizes::new()
    }
}

/// Rect pita berukuran `size` di atas kotak tabel `tables`, rata kiri.
/// `size` = ukuran jangkauan pita (lihat [`Band::reach`]).
pub fn band_rect_above(tables: egui::Rect, size: egui::Vec2) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(tables.left(), tables.top() - BAND_GAP - size.y),
        size,
    )
}

/// Isi kotak group: tabel anggota plus pita API-nya (tanpa padding).
pub fn group_content_rect(tables: egui::Rect, band: Option<egui::Vec2>) -> egui::Rect {
    match band {
        Some(size) => tables.union(band_rect_above(tables, size)),
        None => tables,
    }
}

/// Rencana satu pita, relatif ke pojok kiri atasnya.
struct BandPlan {
    size: egui::Vec2,
    /// Tinggi dari atas pita sampai dasar card terpanjang saat dibuka.
    reach_h: f32,
    cards: Vec<(usize, egui::Vec2)>,
    stacks: Vec<(String, egui::Vec2, usize)>,
}

/// Bagi tinggi tumpukan (urutan tetap) ke paling banyak `n` kolom berurutan
/// dengan tinggi kira-kira seimbang.
fn split_columns(heights: &[f32], n: usize) -> Vec<std::ops::Range<usize>> {
    let target = column_height(heights) / n.max(1) as f32;
    let mut out = Vec::new();
    let (mut start, mut acc) = (0usize, 0.0f32);
    for (i, &h) in heights.iter().enumerate() {
        if i > start && out.len() + 1 < n && acc + h / 2.0 > target {
            out.push(start..i);
            start = i;
            acc = 0.0;
        }
        acc += h + STACK_GAP_Y;
    }
    out.push(start..heights.len());
    out
}

fn column_height(heights: &[f32]) -> f32 {
    heights.iter().map(|h| h + STACK_GAP_Y).sum::<f32>() - STACK_GAP_Y
}

fn columns_width(n: usize) -> f32 {
    n as f32 * CARD_WIDTH + n.saturating_sub(1) as f32 * BAND_COLUMN_GAP
}

/// Jumlah kolom terkecil yang membuat pita cukup lebar (`BAND_ASPECT`).
fn choose_columns(heights: &[f32]) -> Vec<std::ops::Range<usize>> {
    for n in 1..heights.len() {
        let cols = split_columns(heights, n);
        let h = cols
            .iter()
            .map(|r| column_height(&heights[r.clone()]))
            .fold(0.0, f32::max);
        if columns_width(cols.len()) >= BAND_ASPECT * (h + BAND_LABEL_H) {
            return cols;
        }
    }
    split_columns(heights, heights.len())
}

/// Susun card `members` ke tumpukan resource dan kolom. `center_x(i)` = titik
/// tengah horizontal tabel yang disentuh card `i`; `open_heights[i]` = tinggi
/// card `i` saat dibuka.
fn plan_band(
    cards: &[FlowCard],
    members: &[usize],
    heights: &[f32],
    open_heights: &[f32],
    center_x: &dyn Fn(usize) -> Option<f32>,
) -> BandPlan {
    let mut by_resource: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &i in members {
        by_resource
            .entry(resource_of(&cards[i].trigger))
            .or_default()
            .push(i);
    }
    // (resource, card, x tabel, tinggi tumpukan)
    let mut stacks: Vec<(String, Vec<usize>, f32, f32)> = by_resource
        .into_iter()
        .map(|(res, mut ids)| {
            ids.sort_by(|&a, &b| {
                let (ta, tb) = (&cards[a].trigger, &cards[b].trigger);
                (
                    ta.target.as_str(),
                    crate::repo_links::method_rank(&ta.method),
                )
                    .cmp(&(
                        tb.target.as_str(),
                        crate::repo_links::method_rank(&tb.method),
                    ))
            });
            let xs: Vec<f32> = ids.iter().filter_map(|&i| center_x(i)).collect();
            let x = if xs.is_empty() {
                f32::MAX
            } else {
                xs.iter().sum::<f32>() / xs.len() as f32
            };
            let h = STACK_HEADER_H
                + ids.iter().map(|&i| heights[i]).sum::<f32>()
                + ids.len().saturating_sub(1) as f32 * STACK_CARD_GAP;
            (res, ids, x, h)
        })
        .collect();
    stacks.sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.cmp(&b.0)));

    let heights_of: Vec<f32> = stacks.iter().map(|s| s.3).collect();
    let columns = choose_columns(&heights_of);
    let mut plan = BandPlan {
        size: egui::Vec2::ZERO,
        reach_h: 0.0,
        cards: Vec::with_capacity(members.len()),
        stacks: Vec::with_capacity(stacks.len()),
    };
    let mut bottom = 0.0f32;
    for (col, range) in columns.iter().enumerate() {
        let x = col as f32 * (CARD_WIDTH + BAND_COLUMN_GAP);
        let mut y = BAND_LABEL_H;
        for (res, ids, _, _) in &stacks[range.clone()] {
            plan.stacks.push((res.clone(), egui::vec2(x, y), ids.len()));
            y += STACK_HEADER_H;
            for &i in ids {
                plan.cards.push((i, egui::vec2(x, y)));
                plan.reach_h = plan.reach_h.max(y + open_heights[i]);
                y += heights[i] + STACK_CARD_GAP;
            }
            y += STACK_GAP_Y - STACK_CARD_GAP;
        }
        bottom = bottom.max(y - STACK_GAP_Y);
    }
    plan.size = egui::vec2(columns_width(columns.len()), bottom);
    plan.reach_h = plan.reach_h.max(bottom);
    plan
}

impl BandPlan {
    /// Ukuran jangkauan pita (lihat [`Band::reach`]).
    fn reach_size(&self) -> egui::Vec2 {
        egui::vec2(self.size.x, self.reach_h)
    }
}

/// Tulis rencana pita di `origin` ke `rects` dan kembalikan `Band`-nya.
fn place_band(
    plan: &BandPlan,
    origin: egui::Pos2,
    owner: BandOwner,
    tables: Option<egui::Rect>,
    rects: &mut [egui::Rect],
) -> Band {
    for &(i, off) in &plan.cards {
        rects[i] = egui::Rect::from_min_size(origin + off, rects[i].size());
    }
    Band {
        owner,
        rect: egui::Rect::from_min_size(origin, plan.size),
        reach: egui::Rect::from_min_size(origin, plan.reach_size()),
        tables,
        stacks: plan
            .stacks
            .iter()
            .map(|(res, off, count)| BandStack {
                resource: res.clone(),
                header: egui::Rect::from_min_size(
                    origin + *off,
                    egui::vec2(CARD_WIDTH, STACK_HEADER_H),
                ),
                count: *count,
            })
            .collect(),
    }
}

/// Susun card ber-`pos == None` ke pita API. Card yang sudah digeser user
/// (punya `pos`) tidak dipindah. `tables` sejajar `state.flow_cards`.
///
/// 1. Card masuk pita group yang memuat tabel-tabelnya; group dengan
///    repository sama (`repo_key`) didahulukan, lalu group yang memuat tabel
///    terbanyak.
/// 2. Pita group berada `BAND_GAP` di atas tabel group itu, rata kiri, jadi
///    ikut di dalam bingkai group (lihat [`group_content_rect`]). Jaraknya
///    diukur dari dasar jangkauan pita ([`Band::reach`]), jadi card yang
///    dibuka tidak menutupi tabel.
/// 3. Card yang tabelnya tidak di group mana pun masuk pita per repository di
///    atas tabel-tabelnya, dinaikkan bila menabrak tabel atau group lain.
/// 4. Card tanpa tabel di diagram masuk pita "Unmapped" di kiri diagram.
/// 5. Dalam pita: tumpukan per resource, diurutkan menurut posisi x tabelnya;
///    card dalam tumpukan urut path lalu `method_rank`.
pub fn arrange_bands(state: &DiagramState, tables: &[Vec<String>]) -> (Vec<egui::Rect>, Vec<Band>) {
    let cards = &state.flow_cards;
    let heights: Vec<f32> = cards.iter().map(|c| layout_height(state, c)).collect();
    let open_heights: Vec<f32> = cards.iter().map(|c| expanded_height(state, c)).collect();
    let mut rects: Vec<egui::Rect> = cards
        .iter()
        .zip(&heights)
        .map(|(c, &h)| {
            let pos = c.pos.map_or(egui::Pos2::ZERO, |[x, y]| egui::pos2(x, y));
            egui::Rect::from_min_size(pos, egui::vec2(CARD_WIDTH, h))
        })
        .collect();
    if cards.iter().all(|c| c.pos.is_some()) {
        return (rects, Vec::new());
    }

    let index = crate::diagram_lod::node_index(&state.nodes);
    let node_rect = |id: &str| {
        index
            .get(id)
            .map(|&i| egui::Rect::from_min_size(state.nodes[i].pos, state.nodes[i].size))
    };
    let center_x = |i: usize| -> Option<f32> {
        let xs: Vec<f32> = tables
            .get(i)?
            .iter()
            .filter_map(|t| node_rect(t))
            .map(|r| r.center().x)
            .collect();
        (!xs.is_empty()).then(|| xs.iter().sum::<f32>() / xs.len() as f32)
    };

    // Kotak tabel tiap group dan group tiap tabel.
    let mut group_tables: HashMap<&str, egui::Rect> = HashMap::new();
    let mut table_groups: HashMap<&str, Vec<&str>> = HashMap::new();
    for n in &state.nodes {
        let r = egui::Rect::from_min_size(n.pos, n.size);
        let extra = n
            .group_id
            .as_deref()
            .filter(|g| !n.group_ids.iter().any(|x| x == g));
        for gid in n.group_ids.iter().map(String::as_str).chain(extra) {
            group_tables
                .entry(gid)
                .and_modify(|e| *e = e.union(r))
                .or_insert(r);
            table_groups.entry(n.id.as_str()).or_default().push(gid);
        }
    }
    let group_order: HashMap<&str, usize> = state
        .groups
        .iter()
        .enumerate()
        .map(|(i, g)| (g.id.as_str(), i))
        .collect();
    let group_repo: HashMap<&str, String> = state
        .groups
        .iter()
        .filter_map(|g| {
            let key = g.shared_repo_url().and_then(crate::repo_scan::repo_key)?;
            Some((g.id.as_str(), key))
        })
        .collect();

    let owner_of = |card: &FlowCard, touched: &[String]| -> BandOwner {
        if touched.is_empty() {
            return BandOwner::Unmapped;
        }
        let same_repo =
            |g: &str| card.repo_key.is_some() && group_repo.get(g) == card.repo_key.as_ref();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for t in touched {
            for g in table_groups.get(t.as_str()).into_iter().flatten() {
                *counts.entry(g).or_default() += 1;
            }
        }
        let order = |g: &str| group_order.get(g).copied().unwrap_or(usize::MAX);
        let best = counts
            .into_iter()
            .max_by_key(|&(g, n)| (same_repo(g), n, std::cmp::Reverse(order(g))))
            .map(|(g, _)| g)
            // Tabelnya di luar group, tapi repository-nya milik suatu group.
            .or_else(|| {
                state
                    .groups
                    .iter()
                    .map(|g| g.id.as_str())
                    .find(|g| same_repo(g) && group_tables.contains_key(g))
            });
        match best {
            Some(g) => BandOwner::Group(g.to_string()),
            None => BandOwner::Repo(card.repo_key.clone()),
        }
    };

    let mut owners: BTreeMap<BandOwner, Vec<usize>> = BTreeMap::new();
    for (i, c) in cards.iter().enumerate() {
        if c.pos.is_none() {
            let touched = tables.get(i).map(Vec::as_slice).unwrap_or_default();
            owners.entry(owner_of(c, touched)).or_default().push(i);
        }
    }

    let mut bands: Vec<Band> = Vec::new();
    // 1. Pita group.
    for (owner, members) in &owners {
        let BandOwner::Group(gid) = owner else {
            continue;
        };
        let Some(&t) = group_tables.get(gid.as_str()) else {
            continue;
        };
        let plan = plan_band(cards, members, &heights, &open_heights, &center_x);
        let origin = band_rect_above(t, plan.reach_size()).min;
        bands.push(place_band(
            &plan,
            origin,
            owner.clone(),
            Some(t),
            &mut rects,
        ));
    }

    // Penghalang pita di luar group: tabel, lalu kotak group lengkap.
    let mut obstacles: Vec<egui::Rect> = state
        .nodes
        .iter()
        .map(|n| egui::Rect::from_min_size(n.pos, n.size))
        .collect();
    for (gid, &t) in &group_tables {
        let band = bands
            .iter()
            .find(|b| matches!(&b.owner, BandOwner::Group(g) if g == gid))
            .map(|b| b.reach.size());
        let r = group_content_rect(t, band);
        obstacles.push(egui::Rect::from_min_max(
            r.min - egui::vec2(GROUP_SIDE_PAD, GROUP_TOP_PAD),
            r.max + egui::vec2(GROUP_SIDE_PAD, GROUP_SIDE_PAD),
        ));
    }

    // 2. Pita repository tanpa group: di atas tabelnya, naik bila menabrak.
    for (owner, members) in &owners {
        if !matches!(owner, BandOwner::Repo(_)) {
            continue;
        }
        let touched = members
            .iter()
            .flat_map(|&i| tables[i].iter())
            .filter_map(|t| node_rect(t))
            .reduce(|a, b| a.union(b));
        let Some(t) = touched else {
            continue;
        };
        let plan = plan_band(cards, members, &heights, &open_heights, &center_x);
        let mut r = band_rect_above(t, plan.reach_size());
        // Tiap geseran menaruh pita di atas satu penghalang, jadi pasti berhenti.
        for _ in 0..=obstacles.len() {
            let Some(hit) = obstacles.iter().find(|o| o.intersects(r)) else {
                break;
            };
            r = r.translate(egui::vec2(0.0, hit.top() - BAND_GAP - r.bottom()));
        }
        bands.push(place_band(&plan, r.min, owner.clone(), Some(t), &mut rects));
        obstacles.push(r);
    }

    // 3. Card tanpa tabel di diagram: di kiri seluruh isi diagram.
    if let Some(members) = owners.get(&BandOwner::Unmapped) {
        let all = obstacles
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .unwrap_or(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::ZERO,
            ));
        let plan = plan_band(cards, members, &heights, &open_heights, &|_| None);
        let origin = egui::pos2(all.left() - FREE_BAND_GAP - plan.size.x, all.top());
        bands.push(place_band(
            &plan,
            origin,
            BandOwner::Unmapped,
            None,
            &mut rects,
        ));
    }
    (rects, bands)
}

/// Rect card `card_id` (koordinat diagram), seperti yang digambar di LOD
/// `Detail`. Dipakai pencarian dan panel endpoint untuk melompat ke card.
pub fn card_world_rect(state: &DiagramState, card_id: &str) -> Option<egui::Rect> {
    let i = state.flow_cards.iter().position(|c| c.id == card_id)?;
    let frame = FlowFrame::compute(state);
    if frame.spotlight && !frame.is_shown(i) {
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
    use crate::models::structs::{DiagramGroup, DiagramNode, EndpointLink, FlowStep};

    fn node(id: &str, x: f32, y: f32) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            pos: egui::pos2(x, y),
            size: egui::vec2(200.0, 150.0),
            ..Default::default()
        }
    }

    fn grouped(id: &str, x: f32, y: f32, group: &str) -> DiagramNode {
        let mut n = node(id, x, y);
        n.group_ids = vec![group.into()];
        n
    }

    fn group(id: &str, repo: Option<&str>) -> DiagramGroup {
        DiagramGroup {
            id: id.into(),
            title: id.into(),
            color: egui::Color32::RED,
            manual_pos: None,
            repo_url: repo.map(str::to_string),
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

    /// Dasar state untuk tes pita API (mode `Cards`); default-nya mode rail.
    fn band_state() -> DiagramState {
        DiagramState {
            endpoint_display: crate::models::structs::EndpointDisplay::Cards,
            ..Default::default()
        }
    }

    fn assert_no_overlap(rects: &[egui::Rect]) {
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn card_height_counts_summary_steps_and_more_row() {
        let mut c = card("flw_1", "GET", "/a", None);
        assert_eq!(
            card_height(&c, false),
            CARD_HEADER_H + STEP_ROW_H + CARD_PAD_BOTTOM
        );
        c.summary = "Lists things".into();
        c.steps = steps(3);
        assert_eq!(
            card_height(&c, false),
            CARD_HEADER_H + CARD_SUMMARY_H + 3.0 * STEP_ROW_H + CARD_PAD_BOTTOM
        );
        c.steps = steps(12);
        assert_eq!(step_rows(&c, false), (8, 4));
        assert_eq!(step_rows(&c, true), (12, 0));
        assert!(card_height(&c, true) > card_height(&c, false));
        c.collapsed = true;
        assert_eq!(card_height(&c, true), CARD_HEADER_H);
    }

    #[test]
    fn step_rows_follow_header_and_summary() {
        let mut c = card("flw_1", "GET", "/a", None);
        c.summary = "s".into();
        c.steps = steps(10);
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 100.0), egui::vec2(CARD_WIDTH, 400.0));
        let row = step_row_rect(r, &c, 2, false, None).unwrap();
        assert_eq!(
            row.top(),
            100.0 + CARD_HEADER_H + CARD_SUMMARY_H + 2.0 * STEP_ROW_H
        );
        assert!(step_row_rect(r, &c, 9, false, None).is_none());
        assert!(step_row_rect(r, &c, 9, true, None).is_some());
        // Detail langkah 1 yang dibuka mendorong baris sesudahnya saja.
        let open = Some((1, 40.0));
        assert_eq!(
            step_row_rect(r, &c, 1, true, open).unwrap().top(),
            row.top() - STEP_ROW_H
        );
        assert_eq!(
            step_row_rect(r, &c, 2, true, open).unwrap().top(),
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
    fn compact_mode_shows_body_only_for_selected_card() {
        let mut st = DiagramState::default();
        let mut c = card("flw_1", "GET", "/a", None);
        c.steps = steps(3);
        st.flow_cards.push(c);
        let c = &st.flow_cards[0];
        assert!(!shows_body(&st, c));
        assert_eq!(layout_height(&st, c), CARD_HEADER_H);
        assert_eq!(drawn_height(&st, c), CARD_HEADER_H);

        st.selected_flow = Some("flw_1".into());
        let c = &st.flow_cards[0];
        assert!(shows_body(&st, c));
        assert_eq!(layout_height(&st, c), CARD_HEADER_H);
        assert_eq!(drawn_height(&st, c), body_height(c, true));

        st.selected_flow = None;
        st.flow_show_steps = true;
        let c = &st.flow_cards[0];
        assert_eq!(layout_height(&st, c), body_height(c, false));
        st.flow_cards[0].collapsed = true;
        assert_eq!(layout_height(&st, &st.flow_cards[0]), CARD_HEADER_H);
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
    fn group_band_sits_above_its_tables_inside_the_group() {
        let mut st = DiagramState {
            nodes: vec![
                grouped("users", 1000.0, 200.0, "g"),
                grouped("orders", 1400.0, 500.0, "g"),
            ],
            groups: vec![group("g", None)],
            endpoint_links: vec![
                link("users", "POST", "/users", Some("r")),
                link("users", "GET", "/users/{id}", Some("r")),
                link("users", "GET", "/users", Some("r")),
                link("orders", "GET", "/orders", Some("r")),
            ],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.bands.len(), 1);
        let band = &frame.bands[0];
        assert_eq!(band.owner, BandOwner::Group("g".into()));
        assert_eq!(band.rect.left(), 1000.0);
        assert_eq!(band.reach.bottom(), 200.0 - BAND_GAP);
        assert!(band.reach.contains_rect(band.rect));
        for (c, r) in st.flow_cards.iter().zip(&frame.rects) {
            assert!(band.rect.contains_rect(*r), "{r:?} outside {:?}", band.rect);
            // Card yang dibuka tetap di atas tabel.
            assert!(r.top() + expanded_height(&st, c) <= band.reach.bottom() + 0.5);
        }
        assert_no_overlap(&frame.rects);
        let tables = egui::Rect::from_min_max(egui::pos2(1000.0, 200.0), egui::pos2(1600.0, 650.0));
        let content = group_content_rect(tables, frame.band_sizes().get("g").copied());
        assert!(content.contains_rect(band.reach));

        // Tumpukan urut posisi tabel: /users (x 1100) sebelum /orders (x 1500).
        let res: Vec<&str> = band.stacks.iter().map(|s| s.resource.as_str()).collect();
        assert_eq!(res, vec!["/users", "/orders"]);
        // Dalam tumpukan: path lalu method.
        let mut users: Vec<(f32, String)> = st
            .flow_cards
            .iter()
            .zip(&frame.rects)
            .filter(|(c, _)| c.trigger.target.starts_with("/users"))
            .map(|(c, r)| {
                (
                    r.top(),
                    format!("{} {}", c.trigger.method, c.trigger.target),
                )
            })
            .collect();
        users.sort_by(|a, b| a.0.total_cmp(&b.0));
        let order: Vec<String> = users.into_iter().map(|x| x.1).collect();
        assert_eq!(order, vec!["GET /users", "POST /users", "GET /users/{id}"]);
    }

    #[test]
    fn opened_card_never_covers_tables_below_repo_band() {
        let mut st = DiagramState {
            nodes: vec![node("t", 0.0, 0.0), node("u", 0.0, 400.0)],
            endpoint_links: vec![link("t", "POST", "/t", None), link("u", "POST", "/t", None)],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        st.flow_cards[0].steps = steps(25);
        let frame = FlowFrame::compute(&st);
        let band = &frame.bands[0];
        let open = egui::Rect::from_min_size(
            frame.rects[0].min,
            egui::vec2(CARD_WIDTH, expanded_height(&st, &st.flow_cards[0])),
        );
        assert!(band.reach.contains_rect(open));
        for n in &st.nodes {
            let r = egui::Rect::from_min_size(n.pos, n.size);
            assert!(!open.intersects(r), "open card covers {}", n.id);
        }
    }

    #[test]
    fn many_resources_wrap_into_wide_columns() {
        let mut st = DiagramState {
            nodes: vec![grouped("t", 0.0, 0.0, "g")],
            groups: vec![group("g", None)],
            ..band_state()
        };
        for i in 0..40 {
            st.endpoint_links
                .push(link("t", "GET", &format!("/p{i:02}"), None));
        }
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let frame = FlowFrame::compute(&st);
        let lefts: std::collections::BTreeSet<i32> =
            frame.rects.iter().map(|r| r.left() as i32).collect();
        assert!(lefts.len() >= 3, "expected several columns, got {lefts:?}");
        let band = frame.bands[0].rect;
        assert!(band.width() >= band.height());
        assert!(frame.bands[0].reach.bottom() <= -BAND_GAP + 0.5);
        assert_no_overlap(&frame.rects);
    }

    #[test]
    fn moved_cards_keep_their_position() {
        let mut st = DiagramState {
            nodes: vec![node("t", 500.0, 0.0)],
            endpoint_links: vec![link("t", "GET", "/a", None), link("t", "GET", "/b", None)],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        st.flow_cards[0].pos = Some([-2000.0, 700.0]);
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.rects[0].min, egui::pos2(-2000.0, 700.0));
        // Card yang ditata otomatis tetap di pita di atas tabelnya.
        assert!(frame.rects[1].bottom() <= -BAND_GAP + 0.5);
        assert_eq!(frame.bands[0].stacks.len(), 1);
    }

    #[test]
    fn repo_band_without_group_climbs_over_blocking_tables() {
        let mut st = DiagramState {
            // `u` tepat di atas `t`, menutup ruang pita `t`.
            nodes: vec![node("t", 0.0, 0.0), node("u", 0.0, -200.0)],
            endpoint_links: vec![link("t", "GET", "/a", Some("x"))],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.bands[0].owner, BandOwner::Repo(Some("x".into())));
        for n in &st.nodes {
            let nr = egui::Rect::from_min_size(n.pos, n.size);
            assert!(!frame.rects[0].intersects(nr));
        }
        assert!(frame.rects[0].bottom() <= -200.0);
    }

    #[test]
    fn repositories_get_separate_non_overlapping_bands() {
        let mut st = DiagramState {
            nodes: vec![node("a", 0.0, 0.0), node("b", 100.0, 0.0)],
            endpoint_links: vec![
                link("a", "GET", "/a", Some("x")),
                link("b", "GET", "/b", Some("y")),
            ],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.bands.len(), 2);
        assert!(!frame.rects[0].intersects(frame.rects[1]));
    }

    #[test]
    fn group_with_same_repository_wins() {
        let mut t = node("t", 0.0, 0.0);
        t.group_ids = vec!["plain".into(), "svc".into()];
        let mut st = DiagramState {
            nodes: vec![t],
            groups: vec![
                group("plain", None),
                group("svc", Some("https://github.com/org/app.git")),
            ],
            endpoint_links: vec![link("t", "GET", "/a", Some("github.com/org/app"))],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.bands[0].owner, BandOwner::Group("svc".into()));
        assert!(frame.band_sizes().contains_key("svc"));
    }

    #[test]
    fn repo_group_takes_cards_whose_tables_are_outside_it() {
        let mut st = DiagramState {
            nodes: vec![node("t", 0.0, 0.0), grouped("far", 3000.0, 0.0, "g")],
            groups: vec![group("g", Some("https://github.com/org/app.git"))],
            endpoint_links: vec![link("t", "GET", "/a", Some("github.com/org/app"))],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let frame = FlowFrame::compute(&st);
        assert_eq!(frame.bands[0].owner, BandOwner::Group("g".into()));
        assert_eq!(frame.rects[0].left(), 3000.0);
    }

    #[test]
    fn cards_without_diagram_tables_go_left_of_whole_diagram() {
        let mut st = DiagramState {
            nodes: vec![node("t", 0.0, 0.0), node("u", -800.0, 50.0)],
            endpoint_links: vec![link("t", "GET", "/a", None)],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        let mut lonely = card("flw_9", "GET", "/ghost", None);
        lonely.steps = steps(1);
        st.flow_cards.push(lonely);
        let frame = FlowFrame::compute(&st);
        assert!(frame.tables[1].is_empty());
        assert!(frame.rects[1].right() <= -800.0 - FREE_BAND_GAP + 0.5);
        assert!(!frame.rects[0].intersects(frame.rects[1]));
        assert!(frame.bands.iter().any(|b| b.owner == BandOwner::Unmapped));
    }

    #[test]
    fn band_sizes_empty_when_cards_hidden() {
        let mut st = DiagramState {
            nodes: vec![grouped("t", 0.0, 0.0, "g")],
            groups: vec![group("g", None)],
            endpoint_links: vec![link("t", "GET", "/a", None)],
            ..band_state()
        };
        crate::diagram_flow::sync_cards_from_links(&mut st);
        st.endpoint_display = crate::models::structs::EndpointDisplay::Cards;
        assert!(band_sizes(&st).contains_key("g"));
        st.show_endpoints = false;
        assert!(band_sizes(&st).is_empty());
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
        assert!(st.endpoint_display.is_rail());
        let frame = FlowFrame::compute(&st);
        assert!(frame.spotlight && frame.bands.is_empty());
        assert_eq!(frame.shown, vec![false]);
        assert!(band_sizes(&st).is_empty());

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
        assert_eq!(pos, egui::pos2(600.0 - FREE_BAND_GAP - CARD_WIDTH, 300.0));
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
