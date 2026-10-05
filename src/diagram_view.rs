use crate::diagram_lod::{
    DASH_BUDGET, Emphasis, KindFilter, Lod, TableLink, curve_samples, curve_visible, lod_for_zoom,
    node_index, quantize_font,
};
use crate::models::structs::{
    DiagramFlowAnimation, DiagramNode, DiagramState, DiagramViewAnimation, EndpointDisplay,
    RelationOrigin, VirtualRelation,
};
use crate::rfd;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Palet warna group (tanpa duplikat), dipakai menu warna, grouping otomatis,
/// dan group hasil impor Mermaid.
pub const GROUP_COLORS: [egui::Color32; 20] = [
    egui::Color32::from_rgb(100, 149, 237), // Cornflower Blue
    egui::Color32::from_rgb(60, 179, 113),  // Medium Sea Green
    egui::Color32::from_rgb(205, 92, 92),   // Indian Red
    egui::Color32::from_rgb(218, 165, 32),  // Goldenrod
    egui::Color32::from_rgb(147, 112, 219), // Medium Purple
    egui::Color32::from_rgb(70, 130, 180),  // Steel Blue
    egui::Color32::from_rgb(255, 127, 80),  // Coral
    egui::Color32::from_rgb(255, 105, 180), // Hot Pink
    egui::Color32::from_rgb(0, 206, 209),   // Dark Turquoise
    egui::Color32::from_rgb(123, 104, 238), // Medium Slate Blue
    egui::Color32::from_rgb(50, 205, 50),   // Lime Green
    egui::Color32::from_rgb(255, 165, 0),   // Orange
    egui::Color32::from_rgb(106, 90, 205),  // Slate Blue
    egui::Color32::from_rgb(255, 99, 71),   // Tomato
    egui::Color32::from_rgb(64, 224, 208),  // Turquoise
    egui::Color32::from_rgb(238, 130, 238), // Violet
    egui::Color32::from_rgb(255, 215, 0),   // Gold
    egui::Color32::from_rgb(0, 250, 154),   // Medium Spring Green
    egui::Color32::from_rgb(138, 43, 226),  // Blue Violet
    egui::Color32::from_rgb(255, 140, 0),   // Dark Orange
];

/// Zoom terkecil. Cukup jauh supaya diagram ratusan tabel muat di layar;
/// di zoom kecil tampilan beralih ke kartu ringkas / overview (lihat
/// `diagram_lod::Lod`).
pub const MIN_ZOOM: f32 = 0.1;
pub const MAX_ZOOM: f32 = 1.5;
pub const DEFAULT_ZOOM: f32 = 1.0;
/// Zoom tujuan saat double-click judul tabel (nyaman dibaca).
pub const FOCUS_ZOOM: f32 = 1.0;
/// Opacity tabel/relasi yang tidak terkait saat mode fokus aktif.
pub const DIM_OPACITY: f32 = 0.15;
/// Durasi animasi pan/zoom viewport (detik).
pub const VIEW_ANIM_SECS: f64 = 0.35;
/// Jumlah partikel aliran data per relasi.
const FLOW_PARTICLES: usize = 3;
/// Kecepatan partikel (putaran kurva per detik).
const FLOW_SPEED: f64 = 0.45;

/// Ukuran seragam tombol square di floating toolbar diagram.
const TOOLBAR_BTN_SIZE: f32 = 46.0;
const TOOLBAR_ICON_SIZE: f32 = 18.0;
const TOOLBAR_LABEL_SIZE: f32 = 9.5;
/// Padding luar floating toolbar diagram (jarak tepi menu ke tombol).
const TOOLBAR_PADDING: f32 = 2.0;
/// Opacity latar belakang floating toolbar diagram (60%).
const TOOLBAR_OPACITY: f32 = 0.60;

/// Aksi dari toolbar diagram yang butuh state aplikasi (toast, vault, database).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagramAction {
    /// Simpan diagram ke tabel `diagram_by_tabular` di database target (dan
    /// cache lokal, plus Obsidian vault bila aktif).
    Save,
    /// Simpan skema sebagai catatan Mermaid di vault Obsidian.
    SaveToVault,
    /// Bandingkan ulang diagram dengan versi di `diagram_by_tabular` (merge
    /// otomatis atau popup merge bila ada konflik).
    SyncWithDatabase,
    /// Buka dialog untuk me-link database lain sebagai kontainer di kanvas.
    OpenLinkDatabaseModal,
    /// Muat ulang isi kontainer link database (`None` = semua link).
    RefreshLinks(Option<String>),
    /// Arahkan link database ke koneksi lain (id node & relasi tetap).
    RelinkDatabase(String),
    /// Buka tab diagram sumber sebuah link database.
    OpenLinkedDiagram(String),
    /// Buka tab baru berisi tabel ini dan tabel yang berelasi saja.
    OpenFocusInNewTab(String),
    /// Pindai repository milik group ini dan sarankan tabel untuk ditambahkan.
    SuggestGroupTables(String),
    /// Buka tab baru berisi seluruh tabel anggota group (id group).
    OpenGroupInNewTab(String),
    /// Buka tab baru berisi flow card endpoint ini (id card) dan semua
    /// tabel yang tertaut dengannya.
    OpenFlowInNewTab(String),
    /// Buka dialog konfigurasi Group & Sub Group tabel berbasis komentar.
    ConfigureTableGroups,
    /// Ambil ulang skema live supaya group dari komentar tabel dibangun lagi.
    ReloadSchema,
    /// Buka request HTTP API sebuah endpoint yang tertaut ke tabel.
    OpenEndpointRequest {
        request_id: Option<String>,
        label: String,
    },
    /// Tampilkan folder HTTP API yang repository-nya sama dengan group ini.
    ShowLinkedHttpFolders(String),
    /// Generate alur bisnis dengan AI. `card_ids` kosong = semua card milik
    /// repository group `group_id` (atau semua card bila `group_id` juga
    /// kosong). `force` = generate ulang meski file sumbernya tidak berubah.
    GenerateFlows {
        group_id: Option<String>,
        card_ids: Vec<String>,
        force: bool,
    },
    Info(String),
    Error(String),
}

/// Tulis file secara atomik (file sementara unik + fsync + rename), supaya
/// crash di tengah penulisan tidak meninggalkan file setengah jadi.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    crate::directory::write_file_atomically(path, contents)
}

fn export_json(state: &DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .save_file()?;
    Some(
        match serde_json::to_vec_pretty(state)
            .map_err(|e| e.to_string())
            .and_then(|bytes| write_atomic(&path, &bytes).map_err(|e| e.to_string()))
        {
            Ok(()) => DiagramAction::Info(format!("Diagram exported to {}", path.display())),
            Err(e) => DiagramAction::Error(format!("Export failed: {e}")),
        },
    )
}

fn export_mermaid(state: &DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("Mermaid", &["mmd", "mermaid"])
        .add_filter("Markdown", &["md"])
        .save_file()?;
    let model = crate::diagram_mermaid::ErModel::from_diagram(state);
    let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");
    let text = if is_md {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Schema");
        crate::diagram_mermaid::schema_note_markdown(stem, &model)
    } else {
        model.to_mermaid(Default::default())
    };
    Some(match write_atomic(&path, text.as_bytes()) {
        Ok(()) => DiagramAction::Info(format!("Mermaid exported to {}", path.display())),
        Err(e) => DiagramAction::Error(format!("Export failed: {e}")),
    })
}

/// Ekspor semua flow card sebagai Markdown berisi satu `flowchart` per card.
fn export_flows_mermaid(state: &DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("Markdown", &["md"])
        .set_file_name("business-processes.md")
        .save_file()?;
    let title = state
        .diagram_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t} business processes"))
        .unwrap_or_else(|| "Business processes".to_string());
    let text = crate::diagram_mermaid::flows_markdown(state, &title, &[]);
    Some(match write_atomic(&path, text.as_bytes()) {
        Ok(()) => DiagramAction::Info(format!("Business processes exported to {}", path.display())),
        Err(e) => DiagramAction::Error(format!("Export failed: {e}")),
    })
}

/// Ekspor seluruh diagram skema ke file SVG beresolusi tinggi.
fn export_svg(state: &DiagramState) -> Option<DiagramAction> {
    let default_name = state
        .diagram_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}.svg", t.replace(' ', "_")))
        .unwrap_or_else(|| "schema-diagram.svg".to_string());

    let path = rfd::FileDialog::new()
        .add_filter("SVG Image", &["svg"])
        .set_file_name(&default_name)
        .save_file()?;
    let svg = crate::diagram_svg::diagram_to_svg(
        state,
        &crate::diagram_svg::SvgOptions::from_state(state),
    );
    Some(match write_atomic(&path, svg.as_bytes()) {
        Ok(()) => DiagramAction::Info(format!("SVG diagram exported to {}", path.display())),
        Err(e) => DiagramAction::Error(format!("Export failed: {e}")),
    })
}

fn import_json(state: &mut DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()?;
    let result = std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| {
            serde_json::from_slice::<DiagramState>(&bytes).map_err(|e| e.to_string())
        });
    Some(match result {
        Ok(new_state) => {
            *state = new_state;
            state.dragging_node = None;
            state.last_mouse_pos = None;
            state.save_requested = true;
            DiagramAction::Info("Diagram imported".to_string())
        }
        Err(e) => DiagramAction::Error(format!("Import failed: {e}")),
    })
}

fn import_mermaid(state: &mut DiagramState) -> Option<DiagramAction> {
    let path = rfd::FileDialog::new()
        .add_filter("Mermaid / Markdown", &["mmd", "mermaid", "md", "txt"])
        .pick_file()?;
    let parsed = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|text| crate::diagram_mermaid::parse_mermaid_er(&text));
    Some(match parsed {
        Ok(parsed) => {
            for w in &parsed.warnings {
                log::warn!("Mermaid import {}: {w}", path.display());
            }
            let stats = crate::diagram_mermaid::merge_into_state(state, &parsed.model);
            state.save_requested = true;
            let mut msg = format!(
                "Mermaid imported: {} new, {} updated tables, {} new relations",
                stats.added_tables, stats.updated_tables, stats.added_relations
            );
            if !parsed.warnings.is_empty() {
                msg.push_str(&format!(
                    " ({} lines skipped, see log)",
                    parsed.warnings.len()
                ));
            }
            DiagramAction::Info(msg)
        }
        Err(e) => DiagramAction::Error(format!("Mermaid import failed: {e}")),
    })
}

/// Tombol square toolbar: ikon besar di atas, label kecil di bawah.
/// `selected` menandai toggle yang sedang aktif (warna seleksi tema).
/// Teks status sinkronisasi diagram dengan database untuk toolbar.
fn db_status_text(status: &crate::models::structs::DiagramDbStatus) -> String {
    use crate::models::structs::DiagramDbStatus;
    match status {
        DiagramDbStatus::Unknown => "Database: not checked yet".to_string(),
        DiagramDbStatus::Checking => "Database: checking…".to_string(),
        DiagramDbStatus::Saving => "Database: saving…".to_string(),
        DiagramDbStatus::Synced {
            revision,
            updated_by,
            updated_at,
        } => {
            let mut text = format!("Saved in database · revision {revision}");
            if let Some(by) = updated_by {
                text.push_str(&format!(" · by {by}"));
            }
            if let Some(at) = updated_at {
                text.push_str(&format!(" · {at}"));
            }
            text
        }
        DiagramDbStatus::NotInDatabase => {
            "Not saved to the database yet. Press Save to store it in `diagram_by_tabular`."
                .to_string()
        }
        DiagramDbStatus::LocalOnly(e) => format!("Saved locally only. Database: {e}"),
        DiagramDbStatus::MergePending => {
            "Changed in the database too. Resolve the merge before saving.".to_string()
        }
    }
}

fn toolbar_square_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    selected: bool,
) -> egui::Response {
    // ID stabil berbasis label: Popup::menu menyimpan status buka per ID tombol,
    // sedangkan ID otomatis bisa bergeser antar frame (widget kanvas yang muncul
    // saat hover) sehingga popup langsung tertutup.
    let (_, rect) = ui.allocate_space(egui::vec2(TOOLBAR_BTN_SIZE, TOOLBAR_BTN_SIZE));
    let response = ui.interact(
        rect,
        ui.id().with(("toolbar_square_button", label)),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        let painter = ui.painter();
        // Latar hanya digambar saat hover/aktif/terpilih agar toolbar tetap bersih.
        if selected
            || response.hovered()
            || response.has_focus()
            || response.is_pointer_button_down_on()
        {
            painter.rect(
                rect,
                4.0,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }
        let color = visuals.text_color();
        painter.text(
            rect.center_top() + egui::vec2(0.0, 17.0),
            egui::Align2::CENTER_CENTER,
            icon,
            egui::FontId::proportional(TOOLBAR_ICON_SIZE),
            color,
        );
        painter.text(
            rect.center_bottom() - egui::vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(TOOLBAR_LABEL_SIZE),
            color,
        );
    }

    response
}

/// Tombol panah sempit di samping tombol toolbar, untuk membuka menu opsi tambahan.
fn toolbar_dropdown_arrow(ui: &mut egui::Ui, id_source: &str) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(16.0, TOOLBAR_BTN_SIZE));
    let response = ui.interact(
        rect,
        ui.id().with(("toolbar_dropdown_arrow", id_source)),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "More options")
    });
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        if response.hovered() || response.has_focus() || response.is_pointer_button_down_on() {
            ui.painter().rect(
                rect,
                4.0,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            egui_icons::icons::ICON_EXPAND_MORE.codepoint,
            egui::FontId::proportional(TOOLBAR_ICON_SIZE - 2.0),
            visuals.text_color(),
        );
    }
    response.on_hover_text("More save & sync options")
}

/// Pusatkan posisi semua node diagram ke tengah area tampilan (viewport).
pub fn center_diagram(state: &mut DiagramState, view_size: egui::Vec2) {
    if state.nodes.is_empty() {
        state.pan = egui::Vec2::ZERO;
        return;
    }

    // Hitung bounding box dari seluruh node tabel
    let mut min_pos = state.nodes[0].pos;
    let mut max_pos = state.nodes[0].pos + state.nodes[0].size;

    for node in &state.nodes {
        min_pos = min_pos.min(node.pos);
        max_pos = max_pos.max(node.pos + node.size);
    }

    let content_center = min_pos + (max_pos - min_pos) / 2.0;
    let view_center = view_size / 2.0;

    // Geser pan agar titik tengah konten tepat di tengah viewport
    state.pan = view_center - content_center.to_vec2() * state.zoom;
}

/// Pan baru supaya titik `anchor` (koordinat lokal kanvas) tetap diam saat
/// zoom berubah dari `old_zoom` ke `new_zoom`.
pub fn zoom_around(
    pan: egui::Vec2,
    old_zoom: f32,
    new_zoom: f32,
    anchor: egui::Vec2,
) -> egui::Vec2 {
    if old_zoom <= 0.0 {
        return pan;
    }
    anchor - (anchor - pan) * (new_zoom / old_zoom)
}

/// Ubah zoom dengan jangkar di tengah viewport (tombol toolbar).
fn set_zoom_centered(state: &mut DiagramState, zoom: f32, view_size: egui::Vec2) {
    let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    state.pan = zoom_around(state.pan, state.zoom, zoom, view_size / 2.0);
    state.zoom = zoom;
    state.view_anim = None;
}

/// Pan yang menaruh titik diagram `center` tepat di tengah viewport.
pub fn pan_to_center(center: egui::Pos2, view_size: egui::Vec2, zoom: f32) -> egui::Vec2 {
    view_size / 2.0 - center.to_vec2() * zoom
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Nilai (pan, zoom) animasi viewport pada waktu `now`, plus penanda selesai.
pub fn sample_view_anim(anim: &DiagramViewAnimation, now: f64) -> (egui::Vec2, f32, bool) {
    let raw = if anim.duration <= 0.0 {
        1.0
    } else {
        ((now - anim.start_time) / anim.duration) as f32
    };
    let done = raw >= 1.0;
    let t = ease_out_cubic(raw);
    let pan = anim.from_pan + (anim.to_pan - anim.from_pan) * t;
    let zoom = anim.from_zoom + (anim.to_zoom - anim.from_zoom) * t;
    (pan, zoom, done)
}

/// Mulai animasi viewport menuju tabel `node_id` (tengah layar, zoom baca).
fn start_view_animation(state: &mut DiagramState, node_id: &str, view_size: egui::Vec2, now: f64) {
    let Some(node) = state.nodes.iter().find(|n| n.id == node_id) else {
        return;
    };
    let center = node.pos + node.size / 2.0;
    animate_view_to(state, center, FOCUS_ZOOM, view_size, now);
}

/// Mulai animasi viewport yang menaruh titik diagram `center` di tengah
/// layar dengan zoom `to_zoom`.
pub(crate) fn animate_view_to(
    state: &mut DiagramState,
    center: egui::Pos2,
    to_zoom: f32,
    view_size: egui::Vec2,
    now: f64,
) {
    let to_zoom = to_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    state.view_anim = Some(DiagramViewAnimation {
        from_pan: state.pan,
        to_pan: pan_to_center(center, view_size, to_zoom),
        from_zoom: state.zoom,
        to_zoom,
        start_time: now,
        duration: VIEW_ANIM_SECS,
    });
    state.is_centered = true;
}

/// Mulai animasi viewport menuju pusat kelompok tabel `group_id` (tengah layar, zoom fokus).
pub(crate) fn animate_to_group_center(
    state: &mut DiagramState,
    group_id: &str,
    view_size: egui::Vec2,
    now: f64,
) {
    let members = group_members(state, group_id);
    let nodes: Vec<&DiagramNode> = state
        .nodes
        .iter()
        .filter(|n| members.contains(&n.id))
        .collect();
    if !nodes.is_empty() {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for n in nodes {
            min_x = min_x.min(n.pos.x);
            min_y = min_y.min(n.pos.y);
            max_x = max_x.max(n.pos.x + n.size.x);
            max_y = max_y.max(n.pos.y + n.size.y);
        }
        let center = egui::pos2((min_x + max_x) / 2.0, (min_y + max_y) / 2.0);
        animate_view_to(state, center, FOCUS_ZOOM, view_size, now);
    }
}

/// Seperti `start_view_animation`, plus partikel aliran data di relasi
/// tabel tersebut (berhenti sendiri setelah `FLOW_ANIM_SECS`).
fn start_focus_animation(state: &mut DiagramState, node_id: &str, view_size: egui::Vec2, now: f64) {
    if !state.nodes.iter().any(|n| n.id == node_id) {
        return;
    }
    start_view_animation(state, node_id, view_size, now);
    state.flow_anim = Some(DiagramFlowAnimation {
        table_id: node_id.to_string(),
        start_time: now,
    });
}

/// Zoom sampai seluruh diagram muat di viewport, lalu pusatkan. Tidak
/// memperbesar melebihi zoom normal.
pub fn fit_diagram(state: &mut DiagramState, view_size: egui::Vec2) {
    let Some(mut bounds) = crate::diagram_lod::content_bounds(&state.nodes) else {
        state.pan = egui::Vec2::ZERO;
        return;
    };
    // Card sorotan di samping tabelnya ikut termuat.
    if crate::diagram_flow_layout::cards_visible(state) {
        let frame = crate::diagram_flow_layout::FlowFrame::compute(state);
        for i in (0..frame.rects.len()).filter(|&i| frame.is_shown(i)) {
            bounds = bounds.union(frame.drawn_rect(state, i));
        }
    }
    let zoom = crate::diagram_lod::fit_zoom(bounds.size(), view_size, MIN_ZOOM, DEFAULT_ZOOM);
    state.zoom = zoom;
    // Diagram yang tidak muat pada zoom minimum: tengah bounding box bisa
    // jatuh di celah kosong antar blok, jadi arahkan ke area terpadat.
    let visible = view_size / zoom;
    let center = if bounds.width() > visible.x || bounds.height() > visible.y {
        densest_view_center(&state.nodes, visible).unwrap_or(bounds.center())
    } else {
        bounds.center()
    };
    state.pan = pan_to_center(center, view_size, zoom);
    state.view_anim = None;
}

/// Titik tengah jendela seukuran `visible` (koordinat diagram) yang memuat
/// tabel terbanyak. Kandidatnya titik tengah tiap tabel; hasil imbang
/// dimenangkan kandidat pertama supaya deterministik.
fn densest_view_center(nodes: &[DiagramNode], visible: egui::Vec2) -> Option<egui::Pos2> {
    let centers: Vec<egui::Pos2> = nodes.iter().map(|n| n.pos + n.size / 2.0).collect();
    // Diagram sangat besar: kandidat dijarangkan agar tetap ringan.
    let step = (centers.len() / 1500).max(1);
    let mut best: Option<(usize, egui::Pos2)> = None;
    for candidate in centers.iter().step_by(step) {
        let window = egui::Rect::from_center_size(*candidate, visible);
        let (count, sum) = centers
            .iter()
            .filter(|c| window.contains(**c))
            .fold((0usize, egui::Vec2::ZERO), |(n, sum), c| (n + 1, sum + c.to_vec2()));
        if best.is_none_or(|(n, _)| count > n) {
            // Pusatkan ke rata-rata tabel di jendela, bukan ke tabel kandidat.
            best = Some((count, (sum / count.max(1) as f32).to_pos2()));
        }
    }
    best.map(|(_, center)| center)
}

/// Warna glow tabel asal pada diagram fokus. Dibedakan dari emas (relasi
/// terpilih), merah (hasil cari), dan amber (tabrakan saat drag).
const FOCUS_ORIGIN_GLOW: egui::Color32 = egui::Color32::from_rgb(56, 189, 248);

/// Tabel yang disentuh flow card fokus (`focus_flow`), `None` bila tidak ada.
fn flow_focus_tables(state: &DiagramState) -> Option<HashSet<String>> {
    let id = state.focus_flow.as_deref()?;
    let card = state.flow_cards.iter().find(|c| c.id == id)?;
    Some(
        crate::diagram_flow::tables_of(state, card)
            .into_iter()
            .collect(),
    )
}

/// Tabel `table_id` beserta semua tabel yang berelasi dengannya (FK, relasi
/// virtual, dan relasi bawaan link database), dua arah.
pub fn related_tables(state: &DiagramState, table_id: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    set.insert(table_id.to_string());
    for e in &state.edges {
        if e.source == table_id {
            set.insert(e.target.clone());
        } else if e.target == table_id {
            set.insert(e.source.clone());
        }
    }
    for r in state
        .virtual_relations
        .iter()
        .chain(&state.linked_relations)
    {
        if r.child == table_id {
            set.insert(r.parent.clone());
        } else if r.parent == table_id {
            set.insert(r.child.clone());
        }
    }
    set
}

/// Id node tabel di diagram untuk nama `name` dari luar diagram (mis. cache
/// tabel Quick Open). Urutan: id persis, lalu id/judul tanpa beda huruf,
/// lalu nama tanpa prefix schema (`dbo.users` ↔ `users`).
pub fn find_table_id(state: &DiagramState, name: &str) -> Option<String> {
    fn bare(s: &str) -> &str {
        s.rsplit('.').next().unwrap_or(s)
    }
    let nodes = &state.nodes;
    nodes
        .iter()
        .find(|n| n.id == name)
        .or_else(|| {
            nodes
                .iter()
                .find(|n| n.id.eq_ignore_ascii_case(name) || n.title.eq_ignore_ascii_case(name))
        })
        .or_else(|| {
            nodes.iter().find(|n| {
                bare(&n.id).eq_ignore_ascii_case(bare(name))
                    || bare(&n.title).eq_ignore_ascii_case(bare(name))
            })
        })
        .map(|n| n.id.clone())
}

/// Id tabel anggota group `group_id`.
/// ID group ini beserta sub group-nya (group berjudul `"<judul> - <sub>"`).
pub fn group_with_sub_ids(state: &DiagramState, group_id: &str) -> Vec<String> {
    let Some(title) = state.groups.iter().find(|g| g.id == group_id).map(|g| &g.title) else {
        return Vec::new();
    };
    let sub_prefix = format!("{title} - ");
    state
        .groups
        .iter()
        .filter(|g| {
            g.id == group_id
                || (g.title.starts_with(&sub_prefix) && !crate::diagram_links::is_linked_id(&g.id))
        })
        .map(|g| g.id.clone())
        .collect()
}

/// Hapus group dari diagram: catatan group ikut terhapus, tabel anggotanya
/// tetap ada. Group turunan skema (`group_*`) dicatat di `deleted_group_ids`
/// supaya tidak dibuat lagi oleh `merge_schema`.
pub fn delete_groups(state: &mut DiagramState, group_ids: &[String]) {
    for gid in group_ids {
        let anchor = crate::models::structs::NoteAnchor::Group(gid.clone());
        state.notes.retain(|n| n.anchor != anchor);
        state.groups.retain(|g| &g.id != gid);
        for node in &mut state.nodes {
            node.remove_from_group(gid);
        }
        if gid.starts_with("group_") && !state.deleted_group_ids.contains(gid) {
            state.deleted_group_ids.push(gid.clone());
        }
        if state.focus_group.as_deref() == Some(gid.as_str()) {
            state.focus_group = None;
        }
    }
    if !group_ids.is_empty() {
        state.save_requested = true;
    }
}

pub fn group_members(state: &DiagramState, group_id: &str) -> std::collections::HashSet<String> {
    state
        .nodes
        .iter()
        .filter(|n| n.is_in_group(group_id))
        .map(|n| n.id.clone())
        .collect()
}

/// Mode fokus group: seluruh anggota group beserta tabel yang berelasi
/// langsung dengan salah satu anggotanya (lihat [`related_tables`]).
pub fn group_focus_tables(
    state: &DiagramState,
    group_id: &str,
) -> std::collections::HashSet<String> {
    group_members(state, group_id)
        .iter()
        .flat_map(|id| related_tables(state, id))
        .collect()
}

/// Salinan diagram yang hanya berisi `table_id` dan tabel yang berelasi
/// langsung dengannya (lihat [`related_tables`]), sudah ditata rapi.
///
/// Dipakai tombol "Open in new tab" pada chip mode fokus. Group dan link
/// database tidak ikut: bingkai group mengikuti posisi anggotanya sehingga
/// akan melebar tak beraturan setelah tata letak baru. Hasilnya ditandai
/// `scoped_to` supaya tab-nya tidak pernah disimpan sebagai diagram database.
pub fn focus_subset_state(source: &DiagramState, table_id: &str) -> DiagramState {
    let keep = related_tables(source, table_id);
    let nodes = source
        .nodes
        .iter()
        .filter(|n| keep.contains(&n.id))
        .map(|n| {
            let mut n = n.clone();
            n.group_id = None;
            n.group_ids.clear();
            n
        })
        .collect();
    let mut out = subset_with_relations(source, nodes, table_id);
    out.focus_origin = Some(table_id.to_string());
    layout_focus_subset(&mut out, table_id);
    out
}

/// Salinan diagram yang hanya berisi tabel anggota group `group_id` beserta
/// relasi di antara mereka. Posisi tabel dan bingkai group dipertahankan
/// (group lain dibuang), jadi tab baru terlihat persis seperti group itu.
/// `None` bila group tidak ada atau belum punya anggota.
///
/// Dipakai tombol "Open in new tab" di pojok kanan atas header group.
pub fn group_subset_state(source: &DiagramState, group_id: &str) -> Option<DiagramState> {
    let group = source.groups.iter().find(|g| g.id == group_id)?;
    let nodes: Vec<DiagramNode> = source
        .nodes
        .iter()
        .filter(|n| n.is_in_group(group_id))
        .map(|n| {
            let mut n = n.clone();
            n.group_id = None;
            n.group_ids = vec![group_id.to_string()];
            n
        })
        .collect();
    if nodes.is_empty() {
        return None;
    }
    let mut out = subset_with_relations(source, nodes, group_id);
    out.groups = vec![crate::models::structs::DiagramGroup {
        manual_pos: None,
        ..group.clone()
    }];
    let group_anchor = crate::models::structs::NoteAnchor::Group(group_id.to_string());
    out.notes.extend(
        source
            .notes
            .iter()
            .filter(|n| n.anchor == group_anchor)
            .cloned(),
    );
    Some(out)
}

/// Salinan diagram yang hanya berisi flow card endpoint `card_id` beserta
/// semua tabel yang tertaut dengannya (lihat [`crate::diagram_flow::tables_of`])
/// dan relasi di antara tabel itu. Tabel dijajarkan dalam baris, card-nya
/// ditata otomatis di sampingnya (card sorotan). `None` bila card tidak ada atau
/// belum tertaut ke tabel mana pun di diagram.
///
/// Dipakai tombol "Open in tab" pada chip mode fokus endpoint.
pub fn flow_subset_state(source: &DiagramState, card_id: &str) -> Option<DiagramState> {
    const GAP_X: f32 = 80.0;
    const GAP_Y: f32 = 80.0;
    const MIN_SIZE: egui::Vec2 = egui::vec2(160.0, 60.0);
    const MAX_ROW_WIDTH: f32 = 2400.0;

    let card = source.flow_cards.iter().find(|c| c.id == card_id)?;
    let order = crate::diagram_flow::tables_of(source, card);
    let nodes: Vec<DiagramNode> = order
        .iter()
        .filter_map(|t| source.nodes.iter().find(|n| &n.id == t))
        .map(|n| {
            let mut n = n.clone();
            n.group_id = None;
            n.group_ids.clear();
            n
        })
        .collect();
    if nodes.is_empty() {
        return None;
    }
    let mut out = subset_with_relations(source, nodes, card_id);
    out.flow_cards = vec![card.clone()];
    // Card hanya tampil bila rail tampil.
    out.show_endpoints = true;
    if !out.endpoint_display.shows_rail() {
        out.endpoint_display = EndpointDisplay::Both;
    }
    out.selected_flow = Some(card_id.to_string());

    // Baris tabel sesuai urutan tautan; baris baru bila terlalu lebar.
    let (mut x, mut y, mut row_h) = (0.0_f32, 0.0_f32, 0.0_f32);
    for n in &mut out.nodes {
        let size = n.size.max(MIN_SIZE);
        if x > 0.0 && x + size.x > MAX_ROW_WIDTH {
            x = 0.0;
            y += row_h + GAP_Y;
            row_h = 0.0;
        }
        n.pos = egui::pos2(x, y);
        x += size.x + GAP_X;
        row_h = row_h.max(size.y);
    }
    Some(out)
}

/// Diagram baru berisi `nodes` dan relasi yang kedua ujungnya ada di
/// dalamnya; opsi tampilan disalin dari `source`. Ditandai `scoped_to`
/// supaya tab-nya tidak pernah disimpan sebagai diagram database.
fn subset_with_relations(
    source: &DiagramState,
    nodes: Vec<DiagramNode>,
    scoped_to: &str,
) -> DiagramState {
    let keep: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let inside = |a: &str, b: &str| keep.contains(a) && keep.contains(b);
    let edges = source
        .edges
        .iter()
        .filter(|e| inside(&e.source, &e.target))
        .cloned()
        .collect();
    let virtual_relations = source
        .virtual_relations
        .iter()
        .filter(|r| inside(&r.child, &r.parent))
        .cloned()
        .collect();
    let linked_relations = source
        .linked_relations
        .iter()
        .filter(|r| inside(&r.child, &r.parent))
        .cloned()
        .collect();
    let notes = source
        .notes
        .iter()
        .filter(|n| match &n.anchor {
            crate::models::structs::NoteAnchor::Table(t) => keep.contains(t.as_str()),
            crate::models::structs::NoteAnchor::Group(_) => false,
        })
        .cloned()
        .collect();
    let endpoint_links = source
        .endpoint_links
        .iter()
        .filter(|l| keep.contains(l.table.as_str()))
        .cloned()
        .collect();
    // Langkah ke tabel di luar subset tetap tersimpan, hanya tanpa garis.
    let flow_cards = source
        .flow_cards
        .iter()
        .filter(|c| {
            crate::diagram_flow::tables_of(source, c)
                .iter()
                .any(|t| keep.contains(t.as_str()))
        })
        .cloned()
        .collect();
    DiagramState {
        nodes,
        edges,
        virtual_relations,
        linked_relations,
        notes,
        endpoint_links,
        flow_cards,
        show_endpoints: source.show_endpoints,
        endpoint_display: source.endpoint_display,
        show_notes: source.show_notes,
        show_grid: source.show_grid,
        prevent_overlap: source.prevent_overlap,
        show_relations: source.show_relations,
        show_fk_relations: source.show_fk_relations,
        show_virtual_relations: source.show_virtual_relations,
        show_linked_relations: source.show_linked_relations,
        zoom: DEFAULT_ZOOM,
        scoped_to: Some(scoped_to.to_string()),
        ..Default::default()
    }
}

/// Tata letak bintang untuk diagram subset: tabel fokus di tengah, tabel
/// yang dirujuknya (parent) di kiri, tabel yang merujuknya (child) di kanan.
/// Tiap sisi disusun per kolom, urut nama, dan dipusatkan vertikal terhadap
/// tabel fokus. Kolom baru dibuka bila tinggi kolom melewati batas, jadi
/// tabel dengan puluhan relasi tetap membentuk blok yang ringkas.
fn layout_focus_subset(state: &mut DiagramState, focus: &str) {
    const GAP_X: f32 = 180.0;
    const GAP_Y: f32 = 40.0;
    const MIN_SIZE: egui::Vec2 = egui::vec2(160.0, 60.0);
    const MAX_COLUMN_HEIGHT: f32 = 1400.0;

    let Some(focus_idx) = state.nodes.iter().position(|n| n.id == focus) else {
        return;
    };
    // Tabel yang dirujuk tabel fokus (FK / relasi virtual dari fokus).
    let mut parents: HashSet<&str> = HashSet::new();
    for e in &state.edges {
        if e.source == focus && e.target != focus {
            parents.insert(e.target.as_str());
        }
    }
    for r in state
        .virtual_relations
        .iter()
        .chain(&state.linked_relations)
    {
        if r.child == focus && r.parent != focus {
            parents.insert(r.parent.as_str());
        }
    }

    let size_of = |n: &DiagramNode| n.size.max(MIN_SIZE);
    let mut left: Vec<usize> = Vec::new();
    let mut right: Vec<usize> = Vec::new();
    for (i, n) in state.nodes.iter().enumerate() {
        if i == focus_idx {
            continue;
        }
        if parents.contains(n.id.as_str()) {
            left.push(i);
        } else {
            right.push(i);
        }
    }
    let by_name = |a: &usize, b: &usize| state.nodes[*a].id.cmp(&state.nodes[*b].id);
    left.sort_by(by_name);
    right.sort_by(by_name);

    let focus_size = size_of(&state.nodes[focus_idx]);
    let limit = MAX_COLUMN_HEIGHT.max(focus_size.y);
    let center_y = focus_size.y / 2.0;

    // Pecah satu sisi menjadi kolom: (indeks node, lebar kolom, tinggi kolom).
    let columns = |side: &[usize]| -> Vec<(Vec<usize>, f32, f32)> {
        let mut cols: Vec<(Vec<usize>, f32, f32)> = Vec::new();
        for &i in side {
            let sz = size_of(&state.nodes[i]);
            match cols.last_mut() {
                Some((items, w, h)) if *h + GAP_Y + sz.y <= limit => {
                    items.push(i);
                    *w = w.max(sz.x);
                    *h += GAP_Y + sz.y;
                }
                _ => cols.push((vec![i], sz.x, sz.y)),
            }
        }
        cols
    };
    let left_cols = columns(&left);
    let right_cols = columns(&right);

    let mut placed: Vec<(usize, egui::Pos2)> = vec![(focus_idx, egui::Pos2::ZERO)];
    let mut place_column = |items: &[usize], x: f32, width: f32, height: f32, align_right: bool| {
        let mut y = center_y - height / 2.0;
        for &i in items {
            let sz = size_of(&state.nodes[i]);
            // Kolom kiri rata kanan agar ujung relasi ke tabel fokus sejajar.
            let nx = if align_right { x + width - sz.x } else { x };
            placed.push((i, egui::pos2(nx, y)));
            y += sz.y + GAP_Y;
        }
    };
    let mut x = 0.0;
    for (items, w, h) in &left_cols {
        x -= GAP_X + w;
        place_column(items, x, *w, *h, true);
    }
    let mut x = focus_size.x + GAP_X;
    for (items, w, h) in &right_cols {
        place_column(items, x, *w, *h, false);
        x += w + GAP_X;
    }

    let min = placed
        .iter()
        .fold(egui::pos2(f32::MAX, f32::MAX), |m, (_, p)| m.min(*p));
    let offset = egui::vec2(50.0, 50.0) - min.to_vec2();
    for (i, pos) in placed {
        state.nodes[i].pos = pos + offset;
    }
}

/// Warna kolom primary key (nama, badge, aksen baris, ujung relasi parent).
const PK_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 196, 60);
/// Warna kolom foreign key (nama, badge, aksen baris, ujung relasi child).
const FK_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 170, 255);
/// Kolom yang sekaligus primary key dan foreign key (mis. tabel pivot).
const PK_FK_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 130, 80);
/// Ujung parent relasi FK yang merujuk kolom non-PK (unique key).
const REF_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 215, 170);

/// Peran kunci sebuah kolom di tabelnya.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    None,
    Pk,
    Fk,
    PkFk,
}

impl KeyKind {
    fn of(is_pk: bool, is_fk: bool) -> Self {
        match (is_pk, is_fk) {
            (true, true) => Self::PkFk,
            (true, false) => Self::Pk,
            (false, true) => Self::Fk,
            (false, false) => Self::None,
        }
    }

    fn color(self) -> Option<egui::Color32> {
        match self {
            Self::None => None,
            Self::Pk => Some(PK_COLOR),
            Self::Fk => Some(FK_COLOR),
            Self::PkFk => Some(PK_FK_COLOR),
        }
    }

    fn badge(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Pk => "PK",
            Self::Fk => "FK",
            Self::PkFk => "PK FK",
        }
    }
}

/// Warna teks tipe kolom menurut kategorinya, supaya kolom biasa pun bisa
/// dibedakan sekilas (angka, teks, waktu, boolean, JSON, UUID, biner).
/// Urutan cek penting: "interval" sebelum "int", "longtext" ke teks.
fn column_type_color(type_name: &str) -> egui::Color32 {
    let t = type_name.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| t.contains(k));
    if has(&["bool", "bit"]) {
        egui::Color32::from_rgb(90, 195, 205)
    } else if has(&["uuid", "uniqueidentifier", "guid"]) {
        egui::Color32::from_rgb(220, 140, 220)
    } else if has(&["json", "xml", "array", "[]", "hstore", "object", "document"]) {
        egui::Color32::from_rgb(235, 125, 150)
    } else if has(&["date", "time", "year", "interval"]) {
        egui::Color32::from_rgb(175, 150, 235)
    } else if has(&["char", "text", "string", "clob", "enum", "citext"]) {
        egui::Color32::from_rgb(215, 175, 135)
    } else if has(&[
        "int", "serial", "num", "dec", "float", "double", "real", "money",
    ]) {
        egui::Color32::from_rgb(130, 200, 140)
    } else if has(&["blob", "binary", "bytea", "image"]) {
        egui::Color32::from_gray(150)
    } else {
        egui::Color32::from_gray(130)
    }
}

/// Legenda warna kunci & relasi di pojok kiri bawah kanvas (mode detail).
fn draw_key_legend(ui: &egui::Ui, rect: egui::Rect) {
    let items: [(egui::Color32, &str); 4] = [
        (PK_COLOR, "PK"),
        (FK_COLOR, "FK"),
        (PK_FK_COLOR, "PK+FK"),
        (REF_COLOR, "Unique ref"),
    ];
    let font = egui::FontId::proportional(11.0);
    let text_color = egui::Color32::from_gray(200);
    let painter = ui.painter();
    let galleys: Vec<_> = items
        .iter()
        .map(|(_, t)| painter.layout_no_wrap((*t).to_owned(), font.clone(), text_color))
        .collect();
    let hint = painter.layout_no_wrap(
        "relation: FK side to referenced side".to_owned(),
        font.clone(),
        egui::Color32::from_gray(150),
    );
    let swatch = 10.0;
    let row_w: f32 = galleys
        .iter()
        .map(|g| swatch + 4.0 + g.size().x + 10.0)
        .sum();
    let w = row_w.max(hint.size().x) + 12.0;
    let h = 14.0 * 2.0 + 12.0;
    let r = egui::Rect::from_min_size(
        rect.left_bottom() + egui::vec2(12.0, -h - 12.0),
        egui::vec2(w, h),
    );
    painter.rect_filled(r, 4.0, egui::Color32::from_black_alpha(170));
    let mut x = r.left() + 6.0;
    let y = r.top() + 6.0;
    for ((color, _), g) in items.iter().zip(galleys) {
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, y + 2.0), egui::vec2(swatch, swatch)),
            2.0,
            *color,
        );
        x += swatch + 4.0;
        let gw = g.size().x;
        painter.galley(egui::pos2(x, y), g, text_color);
        x += gw + 10.0;
    }
    painter.galley(egui::pos2(r.left() + 6.0, y + 16.0), hint, text_color);
}

/// Redupkan warna sesuai faktor opacity mode fokus.
fn fade(color: egui::Color32, dim: f32) -> egui::Color32 {
    if dim >= 1.0 {
        color
    } else {
        color.linear_multiply(dim)
    }
}

pub fn render_diagram(ui: &mut egui::Ui, state: &mut DiagramState) -> Option<DiagramAction> {
    let frame_start = std::time::Instant::now();
    let mut action: Option<DiagramAction> = None;
    // API rail: panel kiri berskala layar. Digambar lebih dulu supaya kanvas
    // memakai sisa areanya. Modelnya juga dipakai badge CRUD di header tabel.
    let rail_model = (state.show_endpoints && !state.flow_cards.is_empty())
        .then(|| crate::diagram_api_rail::RailModel::build(state));
    let mut rail = match &rail_model {
        Some(model) if crate::diagram_api_rail_view::visible(state) => {
            crate::diagram_api_rail_view::render_rail(ui, state, model)
        }
        _ => Default::default(),
    };
    if rail.action.is_some() {
        action = rail.action.take();
    }
    let rect = ui.available_rect_before_wrap();
    // Semua gambar & interaksi dibatasi ke area diagram, supaya node/group
    // yang digeser ke atas tidak menutupi tab bar.
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));

    // Handle Pan and Zoom
    let response = ui.interact(
        rect,
        ui.id().with("diagram_bg"),
        egui::Sense::click_and_drag(),
    );

    // Pan with middle mouse or drag on background
    if response.dragged() {
        state.pan += response.drag_delta();
        state.view_anim = None;
    }
    // Klik latar kosong menghentikan animasi aliran data dan pemutaran, tapi
    // flow card terpilih tetap tampil sampai Esc atau endpoint-nya diklik
    // lagi di rail.
    if response.clicked() {
        state.flow_anim = None;
        state.focus_flow = None;
        state.flow_play = None;
    }

    // Context Menu for Background
    response.context_menu(|ui| {
        if ui.button("Add Group").clicked() {
            ui.close();
            // Store the click position in Diagram coordinates
            if let Some(mouse_pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                let diagram_vec = (mouse_pos - rect.min - state.pan) / state.zoom;
                let diagram_pos = egui::pos2(diagram_vec.x, diagram_vec.y);
                state.add_group_popup = Some(diagram_pos);
                state.new_group_buffer.clear();
            }
        }
        if ui.button("⚙ Configure Comment Groups…").clicked() {
            action = Some(DiagramAction::ConfigureTableGroups);
            ui.close();
        }
        ui.separator();
        if ui
            .checkbox(&mut state.show_relations, "Show relationship links")
            .clicked()
        {
            state.save_requested = true;
        }
        if ui
            .checkbox(&mut state.show_notes, "Show notes")
            .on_hover_text("Show Markdown sticky notes and their dashed links")
            .clicked()
        {
            state.save_requested = true;
        }
        if ui
            .checkbox(&mut state.show_endpoints, "Show API endpoints")
            .on_hover_text(
                "Show on each table how many HTTP API endpoints use it (generated from the \
                 repository of a linked HTTP API folder)",
            )
            .clicked()
        {
            state.save_requested = true;
        }
        if state.show_endpoints {
            ui.indent("diagram_endpoint_display", |ui| {
                let before = state.endpoint_display;
                ui.horizontal(|ui| {
                    ui.label("API endpoints as:");
                    ui.selectable_value(&mut state.endpoint_display, EndpointDisplay::Rail, "Rail")
                        .on_hover_text(
                            "An API panel beside the diagram; the selected endpoint shows its \
                             process card next to its tables",
                        );
                    ui.selectable_value(
                        &mut state.endpoint_display,
                        EndpointDisplay::Badges,
                        "Badges",
                    )
                    .on_hover_text("An \"API n\" badge on each table header");
                    ui.selectable_value(&mut state.endpoint_display, EndpointDisplay::Both, "Both")
                        .on_hover_text("The API panel and the table badges");
                });
                if before != state.endpoint_display {
                    state.save_requested = true;
                }
            });
        }
        if ui
            .checkbox(&mut state.prevent_overlap, "Prevent table & group overlap")
            .clicked()
        {
            if state.prevent_overlap {
                resolve_all_overlaps(&mut state.nodes, 20.0, None);
            }
            state.save_requested = true;
        }
        if ui.button("⚡ Auto Arrange").clicked() {
            ui.close();
            auto_arrange(state);
            state.save_requested = true;
        }
        if state.focus_table.is_some() || state.focus_group.is_some() || state.focus_flow.is_some()
        {
            ui.separator();
            if ui.button("Clear focus (Esc)").clicked() {
                ui.close();
                state.focus_table = None;
                state.focus_group = None;
                state.focus_flow = None;
                state.flow_play = None;
            }
        }
    });

    // Cek apakah pengguna sedang fokus mengetik teks di widget lain
    let typing = ui.ctx().egui_wants_keyboard_input() || ui.memory(|m| m.focused().is_some());
    // Selama flow card diputar, Spasi = Play/Pause (bukan hand tool).
    let flow_space = crate::diagram_flow_play_view::handle_space(ui, state, typing);
    let space_held = !typing && !flow_space && ui.input(|i| i.key_down(egui::Key::Space));

    // Pointer benar-benar di atas kanvas (bukan di jendela/popup yang menutupinya).
    let pointer_over_canvas = ui.rect_contains_pointer(rect);
    // Scroll di atas kartu note menggulung isinya, bukan zoom diagram.
    let mut note_card_rects = crate::diagram_notes_view::last_card_rects(ui);
    // Begitu juga kartu endpoint yang daftarnya bisa di-scroll.
    note_card_rects.extend(crate::diagram_endpoints_view::last_scroll_rect(ui));

    // Zoom & Shortcut Input Handling
    ui.input_mut(|i| {
        // Toggle Hand Tool (H) & Switch to Select (V / Esc)
        if !typing {
            if i.consume_key(egui::Modifiers::NONE, egui::Key::H) {
                state.hand_tool = !state.hand_tool;
            }
            let esc = !state.show_search && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            if i.consume_key(egui::Modifiers::NONE, egui::Key::V) || esc {
                state.hand_tool = false;
            }
            // Esc juga keluar dari mode fokus dan menghentikan aliran data.
            if esc {
                state.focus_table = None;
                state.focus_group = None;
                state.flow_anim = None;
                state.selected_flow = None;
                state.focus_flow = None;
                state.flow_play = None;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::L) {
                state.show_relations = !state.show_relations;
                state.save_requested = true;
            }
        }

        // Titik jangkar zoom: posisi pointer bila di dalam kanvas, selain itu
        // tengah viewport. Titik diagram di bawah jangkar tidak bergeser.
        let pointer_local = i
            .pointer
            .hover_pos()
            .filter(|p| pointer_over_canvas && rect.contains(*p))
            .filter(|p| !note_card_rects.iter().any(|r| r.contains(*p)))
            .map(|p| p - rect.min);
        let view_center = rect.size() / 2.0;
        let old_zoom = state.zoom;
        let mut new_zoom = old_zoom;
        let mut anchor = view_center;

        // Zoom In (Cmd + / Cmd =)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Plus)
            || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Equals)
        {
            new_zoom *= 1.15;
        }
        // Zoom Out (Cmd -)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Minus) {
            new_zoom /= 1.15;
        }
        // Reset Zoom (Cmd 0)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0) {
            new_zoom = DEFAULT_ZOOM;
        }

        if let Some(p) = pointer_local {
            // Mouse Wheel Zoom / Trackpad scroll (damped to prevent runaway zooming)
            let scroll_delta = i.smooth_scroll_delta.y;
            if scroll_delta != 0.0 {
                new_zoom *= (1.0 + scroll_delta * 0.001).clamp(0.85, 1.15);
                anchor = p;
            }

            // Trackpad pinch gesture
            let zoom_delta = i.zoom_delta();
            if zoom_delta != 1.0 {
                new_zoom *= zoom_delta;
                anchor = p;
            }
        }

        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - old_zoom).abs() > f32::EPSILON {
            state.pan = zoom_around(state.pan, old_zoom, new_zoom, anchor);
            state.zoom = new_zoom;
            // Zoom manual membatalkan animasi viewport yang sedang jalan.
            state.view_anim = None;
        }

        // Save Shortcut (Cmd + S)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::S) {
            state.save_requested = true;
            state.unsaved_changes = false;
            action = Some(DiagramAction::Save);
        }

        // Search Shortcut (Cmd + F)
        if i.consume_key(egui::Modifiers::COMMAND, egui::Key::F) {
            state.show_search = !state.show_search;
            if state.show_search {
                state.search_query.clear();
            }
        }
    });

    let is_hand_mode = state.hand_tool || space_held;

    // Terapkan animasi viewport (pan + zoom) yang sedang berjalan.
    let now = ui.input(|i| i.time);
    if let Some(anim) = &state.view_anim {
        let (pan, zoom, done) = sample_view_anim(anim, now);
        state.pan = pan;
        state.zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if done {
            state.view_anim = None;
        } else {
            ui.ctx().request_repaint();
        }
    }
    // Pilihan dari API rail: card sorotan, lompat ke tabel, progress generate.
    if rail.deselect {
        crate::diagram_flow_play_view::clear(state);
    }
    if let Some(id) = &rail.select {
        crate::diagram_flow_view::spotlight_card(state, id, rect.size(), now);
        ui.ctx().request_repaint();
    }
    if let Some(table) = &rail.focus_table {
        start_focus_animation(state, table, rect.size(), now);
        ui.ctx().request_repaint();
    }
    if rail.show_gen_progress {
        crate::diagram_flow_gen_view::show_progress(state);
    }
    // Majukan pemutaran flow card; berhenti sendiri di akhir timeline.
    crate::diagram_flow_play_view::tick(state, now);
    // Tabel fokus yang sudah hilang dari diagram tidak boleh meredupkan semua.
    if let Some(f) = &state.focus_table
        && !state.nodes.iter().any(|n| &n.id == f)
    {
        state.focus_table = None;
    }
    // Begitu juga group fokus yang sudah dihapus atau tidak punya anggota.
    if let Some(g) = &state.focus_group
        && !state.nodes.iter().any(|n| n.is_in_group(g))
    {
        state.focus_group = None;
    }
    if let Some(f) = &state.flow_anim
        && !state.nodes.iter().any(|n| n.id == f.table_id)
    {
        state.flow_anim = None;
    }
    // Flow card terpilih/fokus yang sudah dihapus dilepas.
    let card_alive = |id: &Option<String>| {
        id.as_ref()
            .is_none_or(|id| state.flow_cards.iter().any(|c| &c.id == id))
    };
    if !card_alive(&state.selected_flow) {
        state.selected_flow = None;
    }
    if !card_alive(&state.focus_flow) {
        state.focus_flow = None;
    }

    // Handle Initial Centering
    if !state.is_centered && !state.nodes.is_empty() {
        // Diagram subset baru saja ditata: tampilkan seluruhnya sekaligus.
        if state.scoped_to.is_some() {
            fit_diagram(state, rect.size());
        } else {
            center_diagram(state, rect.size());
        }
        state.is_centered = true;
    }

    // Scale helper
    let scale = state.zoom;
    let pan = state.pan;

    let to_screen = move |pos: egui::Pos2| -> egui::Pos2 { rect.min + pan + pos.to_vec2() * scale };

    if state.show_grid {
        draw_grid(ui, rect, pan, scale);
    }

    if let Some(a) = draw_link_containers(ui, state, &to_screen, is_hand_mode) {
        action = Some(a);
    }

    // Draw Groups (Containers)
    // Mode fokus: tabel yang tetap tampil normal (None = tidak ada fokus).
    let focus_table = state.focus_table.clone();
    let focus_group = state.focus_group.clone();
    let focus_group_members = focus_group.as_deref().map(|g| group_members(state, g));
    let focus_set = match (focus_table.as_deref(), focus_group.as_deref()) {
        (Some(f), _) => Some(related_tables(state, f)),
        (None, Some(g)) => Some(group_focus_tables(state, g)),
        (None, None) => flow_focus_tables(state),
    };

    let mut _group_rename_request: Option<(usize, String)> = None;
    let mut _group_delete_request: Option<String> = None;
    let mut group_repo_request: Option<String> = None;
    let mut group_suggest_request: Option<String> = None;
    let mut group_flow_request: Option<String> = None;
    let mut flow_progress_request = false;
    let flow_gen_running = crate::diagram_flow_gen_view::is_running(state);
    let mut group_picker_request: Option<String> = None;
    let mut group_open_folder_request: Option<std::path::PathBuf> = None;
    let mut group_drag_delta: Option<(String, egui::Vec2)> = None;
    let mut group_drag_stopped: Option<String> = None;
    let mut group_focus_request: Option<Option<String>> = None;
    let mut note_request: Option<crate::diagram_notes_view::NoteRequest> = None;
    let note_counts = crate::diagram_notes::note_counts(&state.notes);
    let endpoint_counts: HashMap<String, usize> =
        if state.show_endpoints && state.endpoint_display.shows_badges() {
            crate::repo_links::endpoint_counts(state)
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect()
        } else {
            HashMap::new()
        };
    let table_crud = rail_model
        .as_ref()
        .map(|m| m.table_crud())
        .unwrap_or_default();
    let mut endpoint_panel_request: Option<String> = None;

    // Posisi card sorotan dihitung sekali per frame.
    let flow_frame = if crate::diagram_flow_layout::cards_visible(state) {
        crate::diagram_flow_layout::FlowFrame::compute(state)
    } else {
        crate::diagram_flow_layout::FlowFrame::default()
    };

    // 1. Calculate Group Bounds (requires immutable access to nodes and groups)
    let mut group_bounds: Vec<(usize, String, egui::Rect, egui::Color32, String)> = Vec::new(); // (index, id, rect, color, title)

    let shift_held = ui.input(|i| i.modifiers.shift);
    // Extent tiap group dihitung dalam satu lintasan node, bukan
    // O(group × node). Node yang di-drag dengan Shift tidak ikut (sedang
    // dipindah ke group lain).
    let excluded = state.dragging_node.as_deref().filter(|_| shift_held);
    let mut group_extent: std::collections::HashMap<&str, (egui::Pos2, egui::Pos2)> =
        std::collections::HashMap::new();
    for n in &state.nodes {
        if excluded == Some(n.id.as_str()) {
            continue;
        }
        let extra = n
            .group_id
            .as_deref()
            .filter(|g| !n.group_ids.iter().any(|x| x == g));
        for gid in n.group_ids.iter().map(String::as_str).chain(extra) {
            let (lo, hi) = (n.pos, n.pos + n.size);
            group_extent
                .entry(gid)
                .and_modify(|(a, b)| {
                    *a = a.min(lo);
                    *b = b.max(hi);
                })
                .or_insert((lo, hi));
        }
    }
    for (idx, group) in state.groups.iter().enumerate() {
        let Some(&(mut min_pos, mut max_pos)) = group_extent.get(group.id.as_str()) else {
            // Handle Empty Groups with manual_pos
            if let Some(pos) = group.manual_pos {
                let size = egui::vec2(400.0, 300.0);
                let rect = egui::Rect::from_min_size(to_screen(pos), size * scale);
                group_bounds.push((
                    idx,
                    group.id.clone(),
                    rect,
                    group.color,
                    group.title.clone(),
                ));
            }
            continue;
        };

        // Padding
        let padding = 20.0;
        let top_offset = (idx as f32 % 5.0) * 4.0;
        min_pos -= egui::vec2(padding, padding + 30.0 + top_offset);
        max_pos += egui::vec2(padding, padding);

        let min_screen = to_screen(min_pos);
        let max_screen = to_screen(max_pos);
        let rect = egui::Rect::from_min_max(min_screen, max_screen);

        group_bounds.push((
            idx,
            group.id.clone(),
            rect,
            group.color,
            group.title.clone(),
        ));
    }

    // Mode fokus: group yang punya anggota terkait (lainnya diredupkan).
    // Pada fokus group, hanya bingkai group itu sendiri yang tetap terang.
    let focus_groups: Option<std::collections::HashSet<String>> = if let Some(g) = &focus_group {
        Some(std::iter::once(g.clone()).collect())
    } else {
        focus_set.as_ref().map(|set| {
            state
                .nodes
                .iter()
                .filter(|n| set.contains(&n.id))
                .flat_map(|n| n.group_ids.iter().chain(n.group_id.as_ref()).cloned())
                .collect()
        })
    };
    let clip = ui.clip_rect();

    // 2. Render Groups (requires mutable access to groups for Rename, but NOT nodes)
    // We used state.nodes in step 1, now we are done with nodes.
    // But we need to update state.groups.

    for (idx, group_id, group_rect, color, _) in &group_bounds {
        // Group di luar layar tidak digambar (kecuali sedang di-rename).
        if !clip.intersects(*group_rect)
            && state.renaming_group.as_deref() != Some(group_id.as_str())
        {
            continue;
        }
        // Retrieve mutable reference to group
        // We know it exists because we just got it from state.groups
        // But we can't iterate state.groups directly while modifying?
        // Actually we can iterate indices.

        let idx = *idx;
        let group_rect = *group_rect;
        // Mode fokus: grup tanpa anggota terkait ikut diredupkan.
        let group_dim = match &focus_groups {
            Some(set) if !set.contains(group_id) => DIM_OPACITY,
            _ => 1.0,
        };
        let color = fade(*color, group_dim);

        // CAUTION: TextEdit needs `&mut String`.
        // We can get `&mut state.groups[idx]`

        let group = &mut state.groups[idx];

        let is_group_search_match = state.show_search
            && state.search_groups
            && !state.search_query.is_empty()
            && group
                .title
                .to_lowercase()
                .contains(&state.search_query.to_lowercase());

        if is_group_search_match {
            ui.painter().rect_filled(
                group_rect.expand(6.0 * scale),
                12.0 * scale,
                egui::Color32::from_rgb(255, 0, 0).linear_multiply(0.35),
            );
        }

        // Draw Background
        ui.painter()
            .rect_filled(group_rect, 8.0 * scale, color.linear_multiply(0.1));
        let border_color = if is_group_search_match {
            egui::Color32::from_rgb(255, 0, 0)
        } else {
            color.linear_multiply(0.5)
        };
        let border_width = if is_group_search_match {
            2.5 * scale
        } else {
            1.0 * scale
        };
        ui.painter().rect_stroke(
            group_rect,
            8.0 * scale,
            egui::Stroke::new(border_width, border_color),
            egui::StrokeKind::Middle,
        );

        // Header Rect
        let title_rect =
            egui::Rect::from_min_size(group_rect.min, egui::vec2(group_rect.width(), 30.0 * scale));

        let title_fill = if is_group_search_match {
            egui::Color32::from_rgb(200, 30, 30)
        } else {
            color.linear_multiply(0.8)
        };
        ui.painter()
            .rect_filled(title_rect, 8.0 * scale, title_fill);

        let is_renaming = state.renaming_group.as_deref() == Some(group_id);

        if is_renaming {
            let edit_rect = title_rect.shrink(2.0);
            let response = ui
                .scope_builder(egui::UiBuilder::new().max_rect(edit_rect), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut group.title)
                            .frame(egui::Frame::NONE)
                            .text_color(egui::Color32::WHITE)
                            .font(egui::FontId::proportional(16.0 * scale)),
                    )
                })
                .inner;

            if response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                state.renaming_group = None;
            } else {
                response.request_focus();
            }
        } else {
            // Zoom kecil: header terlalu pendek untuk teks, jadi judul
            // digambar di atas kontainer dengan ukuran minimum yang terbaca.
            let title_px = 16.0 * scale;
            if title_px < 11.0 {
                ui.painter().text(
                    group_rect.center_top() - egui::vec2(0.0, 3.0),
                    egui::Align2::CENTER_BOTTOM,
                    &group.title,
                    egui::FontId::proportional(12.0),
                    fade(color, group_dim),
                );
            } else {
                ui.painter().text(
                    title_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    &group.title,
                    egui::FontId::proportional(quantize_font(title_px)),
                    fade(egui::Color32::WHITE, group_dim),
                );
            }

            // Interaction. Group milik link database mengikuti diagram
            // sumbernya, jadi tidak bisa digeser/diubah di sini.
            let is_linked_group = crate::diagram_links::is_linked_id(group_id);
            let interact_rect = title_rect;
            let group_sense = if is_hand_mode || is_linked_group {
                egui::Sense::hover()
            } else {
                egui::Sense::click_and_drag()
            };
            let response = ui.interact(
                interact_rect,
                ui.id().with("group_header").with(idx),
                group_sense,
            );

            if !is_hand_mode && !is_linked_group && response.dragged() {
                let delta = response.drag_delta() / scale;
                group_drag_delta = Some((group_id.clone(), delta));
            }
            if !is_hand_mode && !is_linked_group && response.drag_stopped() {
                group_drag_stopped = Some(group_id.clone());
            }

            let repo_tip: Vec<String> = [
                group
                    .shared_repo_url()
                    .map(|u| format!("Repository: {}", crate::repo_scan::redact(u))),
                group.local_repo_path().map(|p| format!("Your folder: {p}")),
            ]
            .into_iter()
            .flatten()
            .collect();
            let response = if repo_tip.is_empty() {
                response
            } else {
                response.on_hover_text(repo_tip.join("\n"))
            };

            if !is_hand_mode && !is_linked_group {
                response.context_menu(|ui| {
                    if focus_group.as_deref() == Some(group_id.as_str()) {
                        if ui.button("Clear focus").clicked() {
                            ui.close();
                            group_focus_request = Some(None);
                        }
                    } else if group_extent.contains_key(group_id.as_str())
                        && ui
                        .button(format!(
                            "{} Focus on this group",
                            egui_icons::icons::ICON_FILTER_CENTER_FOCUS.codepoint
                        ))
                        .on_hover_text(
                            "Show only the tables in this group and their related tables; dim the rest",
                        )
                        .clicked()
                    {
                        ui.close();
                        group_focus_request = Some(Some(group_id.clone()));
                    }
                    ui.separator();
                    if ui.button("Rename Container").clicked() {
                        ui.close();
                        _group_rename_request = Some((idx, group_id.clone()));
                    }
                    if ui
                        .button(format!(
                            "{} Search Table to Add…",
                            egui_icons::icons::ICON_SEARCH.codepoint
                        ))
                        .on_hover_text("Search the tables in this diagram and add them to this group")
                        .clicked()
                    {
                        ui.close();
                        group_picker_request = Some(group_id.clone());
                    }
                    let has_repo = group.has_repository();
                    let folder = group
                        .local_repo_path()
                        .map(|p| crate::repo_scan::expand_home(&p));
                    let repo_label = if has_repo {
                        format!("{} Edit Repository…", egui_icons::icons::MDI_GIT.codepoint)
                    } else {
                        format!("{} Set Repository…", egui_icons::icons::MDI_GIT.codepoint)
                    };
                    if ui.button(repo_label).clicked() {
                        ui.close();
                        group_repo_request = Some(group_id.clone());
                    }
                    if !cfg!(target_os = "ios") && folder.is_some() {
                        let exists = folder.as_ref().is_some_and(|p| p.is_dir());
                        let open = ui
                            .add_enabled(
                                exists,
                                egui::Button::new(format!(
                                    "{} Open Folder Location",
                                    egui_icons::icons::ICON_FOLDER.codepoint
                                )),
                            )
                            .on_disabled_hover_text("The project folder is not on this computer");
                        if open.clicked() {
                            ui.close();
                            group_open_folder_request = folder.clone();
                        }
                    }
                    let suggest = ui
                        .add_enabled(
                            has_repo,
                            egui::Button::new(format!(
                                "{} Suggest Tables from Repo (AI)",
                                egui_icons::icons::ICON_AUTO_AWESOME.codepoint
                            )),
                        )
                        .on_disabled_hover_text("Set a git repository for this group first");
                    if suggest.clicked() {
                        ui.close();
                        group_suggest_request = Some(group_id.clone());
                    }
                    let has_cards = has_repo
                        && group.repo_key().is_some_and(|k| {
                            state
                                .flow_cards
                                .iter()
                                .any(|c| c.repo_key.as_deref() == Some(k.as_str()))
                        });
                    match crate::diagram_flow_gen_view::generate_menu_item(
                        ui,
                        flow_gen_running,
                        has_cards,
                        false,
                        if has_repo {
                            "No API cards use this repository yet. Generate endpoints from a \
                             linked HTTP API folder first."
                        } else {
                            "Set a git repository for this group first"
                        },
                    ) {
                        Some(crate::diagram_flow_gen_view::GenMenuPick::Generate) => {
                            ui.close();
                            group_flow_request = Some(group_id.clone());
                        }
                        Some(crate::diagram_flow_gen_view::GenMenuPick::ShowProgress) => {
                            ui.close();
                            flow_progress_request = true;
                        }
                        None => {}
                    }
                    if has_repo
                        && ui
                            .button(format!(
                                "{} HTTP API Folders…",
                                egui_icons::icons::MDI_API.codepoint
                            ))
                            .on_hover_text(
                                "HTTP API folders that use the same git repository as this group",
                            )
                            .clicked()
                    {
                        ui.close();
                        action = Some(DiagramAction::ShowLinkedHttpFolders(group_id.clone()));
                    }
                    ui.separator();
                    let anchor = crate::models::structs::NoteAnchor::Group(group_id.clone());
                    if ui
                        .button(format!(
                            "{} Add Note…",
                            egui_icons::icons::ICON_NOTE_ADD.codepoint
                        ))
                        .clicked()
                    {
                        ui.close();
                        note_request =
                            Some(crate::diagram_notes_view::NoteRequest::New(anchor.clone()));
                    }
                    let count = note_counts.get(&anchor).copied().unwrap_or(0);
                    if count > 0
                        && ui
                            .button(format!(
                                "{} Notes ({count})…",
                                egui_icons::icons::ICON_STICKY_NOTE_2.codepoint
                            ))
                            .clicked()
                    {
                        ui.close();
                        note_request = Some(crate::diagram_notes_view::NoteRequest::List(anchor));
                    }
                    ui.separator();
                    if ui.button("Delete Group").clicked() {
                        ui.close();
                        _group_delete_request = Some(group_id.clone());
                    }

                    ui.horizontal(|ui| {
                        ui.label("Color:");
                        egui::ScrollArea::horizontal()
                            .max_width(200.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let colors = GROUP_COLORS;

                                    for &c in &colors {
                                        let (response, painter) = ui.allocate_painter(
                                            egui::vec2(20.0, 20.0),
                                            egui::Sense::click(),
                                        );
                                        let rect = response.rect;
                                        painter.rect_filled(rect, 4.0, c);
                                        if response.hovered() {
                                            painter.rect_stroke(
                                                rect,
                                                4.0,
                                                egui::Stroke::new(2.0, egui::Color32::WHITE),
                                                egui::StrokeKind::Middle,
                                            );
                                        }
                                        if response.clicked() {
                                            group.color = c;
                                            ui.close();
                                        }
                                    }
                                });
                            });
                    });
                });
            }

            // Tombol "Open in new tab" di pojok kanan atas header. Didaftarkan
            // setelah area header supaya klik tidak tertangkap drag header.
            // Disembunyikan bila header terlalu kecil atau group masih kosong.
            let btn_size = 22.0 * scale;
            if btn_size >= 14.0 && group_extent.contains_key(group_id.as_str()) {
                let btn_rect = egui::Rect::from_center_size(
                    egui::pos2(
                        title_rect.right() - 6.0 * scale - btn_size / 2.0,
                        title_rect.center().y,
                    ),
                    egui::vec2(btn_size, btn_size),
                );
                let resp = ui
                    .interact(
                        btn_rect,
                        ui.id().with("group_open_tab").with(idx),
                        egui::Sense::click(),
                    )
                    .on_hover_text("Open all tables in this group in a new diagram tab")
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.hovered() || resp.is_pointer_button_down_on() {
                    ui.painter().rect_filled(
                        btn_rect,
                        4.0 * scale,
                        egui::Color32::from_white_alpha(40),
                    );
                }
                ui.painter().text(
                    btn_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    egui_icons::icons::ICON_OPEN_IN_NEW.codepoint,
                    egui::FontId::proportional(quantize_font(14.0 * scale)),
                    fade(egui::Color32::WHITE, group_dim),
                );
                if resp.clicked() {
                    action = Some(DiagramAction::OpenGroupInNewTab(group_id.clone()));
                }
            }

            // Badge jumlah note group, di kiri tombol "Open in new tab".
            let anchor = crate::models::structs::NoteAnchor::Group(group_id.clone());
            if let Some(&count) = note_counts.get(&anchor) {
                let right = if btn_size >= 14.0 && group_extent.contains_key(group_id.as_str()) {
                    title_rect.right() - 12.0 * scale - btn_size
                } else {
                    title_rect.right() - 6.0 * scale
                };
                let active = state
                    .notes
                    .iter()
                    .any(|n| n.anchor == anchor && state.open_notes.contains(&n.id));
                if let Some(resp) = crate::diagram_notes_view::draw_badge(
                    ui,
                    egui::pos2(right, title_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    count,
                    scale,
                    ui.id().with("group_note_badge").with(idx),
                    active,
                ) {
                    let resp = resp.on_hover_text(crate::diagram_notes_view::badge_tooltip(count));
                    if resp.clicked() {
                        note_request = Some(crate::diagram_notes_view::NoteRequest::Toggle(
                            anchor.clone(),
                        ));
                    }
                    if resp.secondary_clicked() {
                        note_request = Some(crate::diagram_notes_view::NoteRequest::List(anchor));
                    }
                }
            }
        }
    }

    // Apply rename request (workaround for borrow checker)
    if let Some(req) = group_focus_request {
        state.focus_group = req;
        state.focus_table = None;
    }
    if let Some((_, gid)) = _group_rename_request {
        state.renaming_group = Some(gid);
    }
    if let Some(gid) = group_repo_request
        && let Some(g) = state.groups.iter().find(|g| g.id == gid)
    {
        state.group_repo_editor = Some(crate::models::structs::GroupRepoDraft {
            group_id: gid,
            path: g.local_repo_path().unwrap_or_default(),
            url: g.repo_url.clone().unwrap_or_default(),
            url_auto: false,
        });
    }
    if let Some(dir) = group_open_folder_request
        && let Err(e) = crate::url_opener::open_folder(&dir)
    {
        log::warn!("[DIAGRAM] cannot open folder {}: {e}", dir.display());
        action = Some(DiagramAction::Error(format!("Cannot open folder: {e}")));
    }
    if let Some(gid) = group_suggest_request {
        action = Some(DiagramAction::SuggestGroupTables(gid));
    }
    if let Some(gid) = group_flow_request {
        action = Some(DiagramAction::GenerateFlows {
            group_id: Some(gid),
            card_ids: Vec::new(),
            force: false,
        });
    }
    if flow_progress_request {
        crate::diagram_flow_gen_view::show_progress(state);
    }
    if let Some(gid) = group_picker_request
        && let Some(g) = state.groups.iter().find(|g| g.id == gid)
    {
        state.group_table_picker = Some(crate::models::structs::GroupTablePicker {
            group_id: gid,
            group_title: g.title.clone(),
            ..Default::default()
        });
    }
    // Apply group deletion request
    if let Some(del_gid) = _group_delete_request {
        // Sub group ikut terhapus bersama group induknya.
        let ids = group_with_sub_ids(state, &del_gid);
        delete_groups(state, &ids);
    }
    // Apply deferred group move
    if let Some((group_id, delta)) = group_drag_delta {
        // Move nodes belonging to group
        for node in &mut state.nodes {
            if node.is_in_group(&group_id) {
                node.pos += delta;
            }
        }

        // Move group manual_pos if it exists (for empty groups)
        if let Some(group) = state.groups.iter_mut().find(|g| g.id == group_id)
            && let Some(pos) = &mut group.manual_pos
        {
            *pos += delta;
        }
    }
    // Group selesai digeser: group itu yang mengalah bila menabrak group lain.
    if let Some(group_id) = group_drag_stopped
        && state.prevent_overlap
    {
        resolve_all_overlaps(&mut state.nodes, 20.0, Some(&group_id));
        state.save_requested = true;
    }

    // Draw edges (relationships)
    let mut clicked_edge: Option<(String, String)> = None;
    let pointer_down = !is_hand_mode && ui.input(|i| i.pointer.primary_clicked());
    let pointer_double = !is_hand_mode
        && ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
        });

    let lod = lod_for_zoom(scale);
    let kind_filter = KindFilter::from_state(state);
    let hover_pos = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| pointer_over_canvas && rect.contains(*p));
    // Tabel di bawah pointer: relasinya disorot, relasi lain disamarkan.
    let hovered_table: Option<String> = hover_pos
        .filter(|_| state.dragging_node.is_none())
        .and_then(|p| crate::diagram_lod::node_at(&state.nodes, &to_screen, scale, p))
        .map(|i| state.nodes[i].id.clone());
    // Jumlah relasi terlihat pada frame sebelumnya; menentukan mode padat
    // dan garis putus-putus tanpa perlu dua lintasan per frame.
    let visible_key = ui.id().with("diagram_visible_relations");
    let visible_prev: usize = ui.data(|d| d.get_temp(visible_key)).unwrap_or(0);
    let mut rel_stats = RelStats::default();
    let mut virtual_outcome = VirtualOutcome::None;

    {
        let index = node_index(&state.nodes);
        let ctx = RelCtx {
            index: &index,
            clip,
            hover: hover_pos,
            dashed: visible_prev <= DASH_BUDGET,
            emphasis: Emphasis {
                focus: focus_table.as_deref(),
                focus_group: focus_group_members.as_ref(),
                hovered: hovered_table.as_deref(),
                visible: visible_prev,
                dim_opacity: DIM_OPACITY,
            },
        };
        let links = (lod != Lod::Detail)
            .then(|| crate::diagram_lod::aggregate_table_links(state, &index, kind_filter));
        if state.show_relations {
            match &links {
                None => {
                    if kind_filter.fk {
                        clicked_edge = draw_fk_edges(
                            ui,
                            state,
                            &ctx,
                            &to_screen,
                            pointer_down,
                            &mut rel_stats,
                        );
                    }
                    if kind_filter.linked {
                        draw_linked_relations(ui, state, &ctx, &to_screen, &mut rel_stats);
                    }
                    if kind_filter.virtual_ {
                        virtual_outcome = draw_virtual_relations(
                            ui,
                            state,
                            &ctx,
                            &to_screen,
                            pointer_down,
                            pointer_double,
                            &mut rel_stats,
                        );
                    }
                }
                Some(links) => {
                    if let Some(pair) = draw_aggregated_links(
                        ui,
                        state,
                        lod,
                        links,
                        &group_bounds,
                        &ctx,
                        &to_screen,
                        pointer_down,
                        &mut rel_stats,
                    ) {
                        // Klik garis ringkas: sorot kedua tabel dan buka daftar relasinya.
                        state.relations_panel = Some(pair.0.clone());
                        clicked_edge = Some(pair);
                    }
                }
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(visible_key, rel_stats.drawn));

    let visible_notes = crate::diagram_notes_view::prepare_visible(state);
    crate::diagram_notes_view::draw_note_links(
        ui,
        state,
        &visible_notes,
        &to_screen,
        clip,
        hover_pos,
    );

    // Flow card proses bisnis: garisnya di bawah tabel, card-nya di atas
    // tabel.
    if !flow_frame.rects.is_empty() {
        crate::diagram_flow_view::draw_flow_lines(ui, state, &flow_frame, &to_screen, clip);
    }

    let virtual_clicked = match virtual_outcome {
        VirtualOutcome::None => false,
        VirtualOutcome::Remove(idx) => {
            remove_virtual(state, idx);
            true
        }
        VirtualOutcome::Select(idx) => {
            state.selected_virtual = Some(idx);
            state.selected_edge = None;
            true
        }
        VirtualOutcome::Edit(idx) => {
            crate::diagram_relation_editor::open_relation_editor(state, idx);
            true
        }
    };
    let edge_was_clicked = clicked_edge.is_some() || virtual_clicked;
    if let Some(edge) = clicked_edge
        && !is_hand_mode
    {
        state.selected_edge = Some(edge);
    }

    // Draw nodes
    let mut dragging_node_id = None;
    let mut drag_delta = egui::Vec2::ZERO;
    let mut drag_stopped_node_id: Option<String> = None;
    let mut node_clicked = false;
    let mut column_clicked_request: Option<(String, String)> = None;

    // Snapshot nodes untuk deteksi tabrakan, hanya saat ada node di-drag
    // (clone penuh tiap frame mahal untuk diagram besar).
    let nodes_snapshot = if state.prevent_overlap && state.dragging_node.is_some() {
        state.nodes.clone()
    } else {
        Vec::new()
    };
    let search_lower = state.search_query.to_lowercase();
    let mut nodes_drawn = 0usize;
    let mut relations_request: Option<String> = None;

    // For manual interaction & relation search:
    let selected_column = state.selected_column.clone();
    let sel_col_is_pk = selected_column
        .as_ref()
        .is_some_and(|(sel_table, sel_col)| {
            state
                .nodes
                .iter()
                .find(|n| &n.id == sel_table)
                .and_then(|n| n.column_info(sel_col))
                .is_some_and(|c| c.is_pk)
        });
    let relation_cols = selected_relation_columns(state);
    let shift_down = ui.input(|i| i.modifiers.shift);
    let ctrl_down = ui.input(|i| i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd);
    let mut link_request: Option<VirtualRelation> = None;
    let mut remove_node_request: Option<String> = None;
    let mut search_relations_for_column: Option<(String, String)> = None;

    let available_groups: Vec<(String, String, egui::Color32)> = state
        .groups
        .iter()
        .map(|g| (g.id.clone(), g.title.clone(), g.color))
        .collect();
    let mut empty_group_retention: Option<(String, egui::Pos2)> = None;
    let mut add_group_at_pos: Option<egui::Pos2> = None;
    let mut open_source_request: Option<String> = None;
    let canvas_bg = ui.visuals().panel_fill;
    let mut focus_request: Option<Option<String>> = None;
    let mut double_clicked_node: Option<String> = None;

    // Lebar glyph monospace pada ukuran unscaled; dipakai untuk menyesuaikan
    // lebar node dengan nama kolom terpanjang.
    let (name_advance, small_advance) = ui.fonts_mut(|f| {
        (
            f.glyph_width(&egui::FontId::monospace(12.0), 'M'),
            f.glyph_width(&egui::FontId::monospace(10.0), 'M'),
        )
    });

    for node in &mut state.nodes {
        node.ensure_groups_migrated();

        // Estimate height based on columns
        let header_height_unscaled = node_header_height(node);
        let item_height_unscaled = 16.0;
        let content_height_unscaled = node.columns.len() as f32 * item_height_unscaled;
        let node_height_unscaled = header_height_unscaled + content_height_unscaled + 8.0; // padding
        let node_width = node_content_width(node, name_advance, small_advance);
        node.size = egui::vec2(node_width, node_height_unscaled);

        let node_size_scaled = node.size * scale;
        let node_pos_screen = to_screen(node.pos);
        let node_rect = egui::Rect::from_min_size(node_pos_screen, node_size_scaled);

        // Node di luar layar tidak digambar dan tidak mendaftarkan widget,
        // kecuali yang sedang di-drag (drag harus tetap tersambung).
        if !clip.intersects(node_rect.expand(8.0 * scale))
            && state.dragging_node.as_deref() != Some(node.id.as_str())
        {
            continue;
        }
        nodes_drawn += 1;

        // Interact
        let node_id = ui.id().with("node").with(&node.id);
        // Tabel milik link database: posisinya mengikuti diagram sumber, jadi
        // tidak bisa digeser. Kolomnya tetap bisa dipakai membuat relasi.
        let is_linked = crate::diagram_links::is_linked_id(&node.id);
        let node_sense = if is_hand_mode {
            egui::Sense::hover()
        } else if is_linked {
            egui::Sense::click()
        } else {
            egui::Sense::click_and_drag()
        };
        let node_response = ui.interact(node_rect, node_id, node_sense);

        if !is_hand_mode && node_response.clicked() {
            node_clicked = true;
        }
        // Double-click judul tabel: pusatkan & zoom, lalu animasikan relasinya.
        if !is_hand_mode
            && node_response.double_clicked()
            && ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|p| p.y <= node_rect.top() + header_height_unscaled * scale)
        {
            double_clicked_node = Some(node.id.clone());
        }

        let mut toggle_group: Option<(String, bool)> = None;
        let mut new_group_for_node = false;
        if !is_hand_mode {
            node_response.context_menu(|ui| {
            ui.label(egui::RichText::new(&node.title).strong());
            ui.separator();

            if focus_table.as_deref() == Some(node.id.as_str()) {
                if ui.button("Clear focus").clicked() {
                    ui.close();
                    focus_request = Some(None);
                }
            } else if ui
                .button("Focus on this table")
                .on_hover_text("Show only this table and its related tables; dim the rest")
                .clicked()
            {
                ui.close();
                focus_request = Some(Some(node.id.clone()));
            }
            if ui
                .button("Show relations")
                .on_hover_text("List every relation of this table in a searchable panel")
                .clicked()
            {
                ui.close();
                relations_request = Some(node.id.clone());
            }
            ui.separator();

            if is_linked {
                ui.label(
                    egui::RichText::new(format!(
                        "From linked database {}",
                        node.database_name.as_deref().unwrap_or("?")
                    ))
                    .weak(),
                );
                ui.label(
                    egui::RichText::new(
                        "Layout and groups follow the source diagram.\nRelations can still be drawn from its columns.",
                    )
                    .weak()
                    .small(),
                );
                if ui.button("Open source diagram").clicked() {
                    ui.close();
                    open_source_request =
                        crate::diagram_links::link_id_of(&node.id).map(str::to_string);
                }
                return;
            }

            let anchor = crate::models::structs::NoteAnchor::Table(node.id.clone());
            if ui
                .button(format!(
                    "{} Add note…",
                    egui_icons::icons::ICON_NOTE_ADD.codepoint
                ))
                .on_hover_text("Add a Markdown sticky note to this table")
                .clicked()
            {
                ui.close();
                note_request = Some(crate::diagram_notes_view::NoteRequest::New(anchor.clone()));
            }
            let count = note_counts.get(&anchor).copied().unwrap_or(0);
            if count > 0
                && ui
                    .button(format!(
                        "{} Notes ({count})…",
                        egui_icons::icons::ICON_STICKY_NOTE_2.codepoint
                    ))
                    .clicked()
            {
                ui.close();
                note_request = Some(crate::diagram_notes_view::NoteRequest::List(anchor));
            }
            ui.separator();

            if node.detached {
                ui.label(
                    egui::RichText::new("This table is not in the database (imported)").weak(),
                );
                if ui.button("Remove from diagram").clicked() {
                    ui.close();
                    remove_node_request = Some(node.id.clone());
                }
                ui.separator();
            }

            ui.label(egui::RichText::new("Groups:").small().weak());
            if available_groups.is_empty() {
                ui.label(egui::RichText::new("No groups created yet").weak());
            } else {
                for (gid, gtitle, gcolor) in &available_groups {
                    if crate::diagram_links::is_linked_id(gid) {
                        continue;
                    }
                    let in_group = node.is_in_group(gid);
                    let (prefix, action_label) = if in_group {
                        (
                            egui_icons::icons::ICON_CHECK.codepoint,
                            format!("Remove from {}", gtitle),
                        )
                    } else {
                        (
                            egui_icons::icons::ICON_ADD.codepoint,
                            format!("Add to {}", gtitle),
                        )
                    };
                    let button = egui::Button::new(
                        egui::RichText::new(format!("{} {}", prefix, action_label)).color(
                            if in_group {
                                *gcolor
                            } else {
                                ui.visuals().text_color()
                            },
                        ),
                    );
                    if ui.add(button).clicked() {
                        toggle_group = Some((gid.clone(), !in_group));
                        ui.close();
                    }
                }
            }
            ui.separator();
            if ui.button("➕ New Group…").clicked() {
                new_group_for_node = true;
                ui.close();
            }
            });
        }

        if let Some((gid, add)) = toggle_group {
            if add {
                node.add_to_group(gid);
            } else {
                node.remove_from_group(&gid);
                empty_group_retention = Some((gid, node.pos));
            }
            state.save_requested = true;
        }
        if new_group_for_node {
            add_group_at_pos = Some(node.pos + egui::vec2(node.size.x + 20.0, 0.0));
        }

        if !is_hand_mode && !is_linked && node_response.dragged() {
            dragging_node_id = Some(node.id.clone());
            drag_delta = node_response.drag_delta();

            // Track globally for drop detection
            state.dragging_node = Some(node.id.clone());
        } else if !is_hand_mode && !is_linked && node_response.drag_stopped() {
            let shift_held = ui.input(|i| i.modifiers.shift);
            if shift_held {
                // Check drop target
                if let Some(pointer_pos) = ui.input(|i| i.pointer.hover_pos()) {
                    for (_, gid, rect, _, _) in &group_bounds {
                        if crate::diagram_links::is_linked_id(gid) {
                            continue;
                        }
                        if rect.contains(pointer_pos) {
                            node.add_to_group(gid.clone());
                            state.save_requested = true;
                            break;
                        }
                    }
                }
            }
            drag_stopped_node_id = Some(node.id.clone());
            state.dragging_node = None;
        }

        // Check if this node is part of the selected relationship
        let is_selected_edge_node = if let Some((s, t)) = &state.selected_edge {
            node.id == *s || node.id == *t
        } else {
            false
        };

        // Check if node matches search
        let is_search_match = if state.show_search && !search_lower.is_empty() {
            let query = &search_lower;
            (state.search_tables && node.title.to_lowercase().contains(query))
                || (state.search_columns
                    && node
                        .columns
                        .iter()
                        .any(|c| c.to_lowercase().contains(query)))
        } else {
            false
        };

        // Deteksi apakah node yang sedang di-drag sedang bertabrakan dengan tabel lain
        let is_colliding_drag = state.prevent_overlap
            && state.dragging_node.as_deref() == Some(&node.id)
            && check_single_node_collision(&nodes_snapshot, &node.id, 20.0);

        let is_glow = is_selected_edge_node || is_search_match || is_colliding_drag;
        let is_focus_origin = state.focus_origin.as_deref() == Some(node.id.as_str());

        // Draw Shadow/Border
        if is_focus_origin && !is_glow {
            // Tabel asal diagram fokus: glow berlapis yang memudar ke luar.
            for (spread, alpha) in [(14.0, 0.10), (10.0, 0.18), (6.0, 0.30), (3.0, 0.45)] {
                ui.painter().rect_filled(
                    node_rect.expand(spread * scale),
                    (4.0 + spread) * scale,
                    FOCUS_ORIGIN_GLOW.linear_multiply(alpha),
                );
            }
        } else if is_glow {
            // Glow effect
            let glow_color = if is_search_match {
                egui::Color32::from_rgb(255, 0, 0) // Bright Red
            } else if is_colliding_drag {
                egui::Color32::from_rgb(255, 140, 0) // Warning Amber
            } else {
                egui::Color32::from_rgb(255, 215, 0) // Gold
            };

            ui.painter().rect_filled(
                node_rect.expand(6.0 * scale),
                12.0 * scale,
                glow_color.linear_multiply(0.5),
            );
        } else {
            ui.painter().rect_filled(
                node_rect.expand(2.0 * scale),
                5.0 * scale,
                egui::Color32::from_black_alpha(50),
            );
        }

        let fill_color = egui::Color32::from_rgb(30, 30, 35);
        ui.painter().rect_filled(node_rect, 4.0 * scale, fill_color);
        // Corrected rect_stroke args
        let border_color = if is_search_match {
            egui::Color32::from_rgb(255, 0, 0)
        } else if is_colliding_drag {
            egui::Color32::from_rgb(255, 140, 0)
        } else if is_selected_edge_node {
            egui::Color32::from_rgb(255, 215, 0)
        } else if is_focus_origin {
            FOCUS_ORIGIN_GLOW
        } else {
            egui::Color32::from_gray(60)
        };

        let border_width = if is_glow || is_focus_origin {
            2.0 * scale
        } else {
            1.0 * scale
        };

        ui.painter().rect_stroke(
            node_rect,
            4.0 * scale,
            egui::Stroke::new(border_width, border_color),
            egui::StrokeKind::Middle,
        );

        let is_dimmed = focus_set
            .as_ref()
            .is_some_and(|set| !set.contains(&node.id));

        // Header
        let header_height = header_height_unscaled * scale;
        let header_rect = egui::Rect::from_min_size(
            node_pos_screen,
            egui::vec2(node_rect.width(), header_height),
        );

        // Simplified rounding to avoid compilation error
        // Header ungu untuk tabel yang tidak ada di database.
        let header_fill = if node.detached {
            egui::Color32::from_rgb(72, 52, 100)
        } else {
            egui::Color32::from_rgb(50, 50, 60)
        };
        ui.painter()
            .rect_filled(header_rect, 4.0 * scale, header_fill);
        if node.detached {
            ui.painter().text(
                egui::pos2(header_rect.right() - 6.0 * scale, header_rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "not in DB",
                egui::FontId::proportional(quantize_font(9.0 * scale)),
                egui::Color32::from_gray(190),
            );
        }

        // Group dots on header
        let mut dot_x = header_rect.left() + 8.0 * scale;
        let mut member_group_names = Vec::new();
        for gid in &node.group_ids {
            if let Some((_, title, color)) = available_groups.iter().find(|(id, _, _)| id == gid) {
                member_group_names.push(title.as_str());
                ui.painter().circle_filled(
                    egui::pos2(dot_x, header_rect.center().y),
                    3.5 * scale,
                    *color,
                );
                ui.painter().circle_stroke(
                    egui::pos2(dot_x, header_rect.center().y),
                    3.5 * scale,
                    egui::Stroke::new(1.0 * scale, egui::Color32::from_black_alpha(120)),
                );
                dot_x += 9.0 * scale;
            }
        }

        let comment_str = state.table_comments.get(&node.id).map(|s| s.as_str()).unwrap_or("");
        let comment_line = if !comment_str.is_empty() {
            format!("\nComment: {comment_str}")
        } else {
            String::new()
        };

        if !member_group_names.is_empty() {
            node_response.on_hover_text(format!(
                "Table: {}{comment_line}\nGroups: {}\nDouble-click the title to zoom in and show data flow\nRight-click to focus or manage groups",
                node.title,
                member_group_names.join(", ")
            ));
        } else {
            node_response.on_hover_text(format!(
                "Table: {}{comment_line}\nDouble-click the title to zoom in and show data flow\nRight-click to focus or add to a group",
                node.title
            ));
        }

        // Title & Database badge
        if let Some(db) = &node.database_name {
            let title_pos = header_rect.center() + egui::vec2(0.0, -5.0 * scale);
            let db_pos = header_rect.center() + egui::vec2(0.0, 7.5 * scale);
            let db_label = if let Some(conn) = &node.connection_name {
                format!("{}/{}", conn, db)
            } else {
                db.clone()
            };
            ui.painter().text(
                title_pos,
                egui::Align2::CENTER_CENTER,
                &node.title,
                egui::FontId::proportional(quantize_font(12.5 * scale)),
                egui::Color32::WHITE,
            );
            ui.painter().text(
                db_pos,
                egui::Align2::CENTER_CENTER,
                format!("[{}]", db_label),
                egui::FontId::proportional(quantize_font(9.0 * scale)),
                egui::Color32::from_white_alpha(190),
            );
        } else {
            ui.painter().text(
                header_rect.center(),
                egui::Align2::CENTER_CENTER,
                &node.title,
                egui::FontId::proportional(quantize_font(14.0 * scale)),
                egui::Color32::WHITE,
            );
        }

        // Columns
        let item_height = item_height_unscaled * scale;
        let mut y_offset = header_height + 4.0 * scale;
        // Zoom sangat kecil: teks kolom tak terbaca (< 5 px). Baris tetap
        // digambar sebagai bar tipis supaya bentuk tabel ERD utuh, tanpa biaya
        // layout teks dan tanpa widget per kolom.
        let tiny_rows = 12.0 * scale < 5.0;

        for (col_idx, col) in node.columns.iter().enumerate() {
            // `column_meta` biasanya sejajar dengan `columns`; cari linear hanya bila tidak.
            let info = node
                .column_meta
                .get(col_idx)
                .filter(|m| m.name == *col)
                .or_else(|| node.column_info(col));
            let is_pk = info.is_some_and(|c| c.is_pk);
            let is_fk = node.is_fk_column(col);
            let key = KeyKind::of(is_pk, is_fk);
            let type_name = info.map(|c| c.type_name.as_str()).unwrap_or("");

            let col_pos_screen = node_pos_screen + egui::vec2(0.0, y_offset);
            let col_rect = egui::Rect::from_min_size(
                col_pos_screen,
                egui::vec2(node_rect.width(), item_height),
            );
            // Tabel panjang yang terpotong layar: baris di luar layar dilewati.
            if !clip.intersects(col_rect) {
                y_offset += item_height;
                continue;
            }
            if tiny_rows {
                let color = key.color().unwrap_or_else(|| {
                    if type_name.is_empty() {
                        egui::Color32::from_gray(120)
                    } else {
                        column_type_color(type_name)
                    }
                });
                let text_w = (col.chars().count() as f32 * 7.2 * scale)
                    .min(node_rect.width() - 16.0 * scale);
                let bar = egui::Rect::from_min_size(
                    col_pos_screen + egui::vec2(8.0 * scale, item_height * 0.3),
                    egui::vec2(text_w, item_height * 0.4),
                );
                ui.painter()
                    .rect_filled(bar, 0.0, color.linear_multiply(0.6));
                y_offset += item_height;
                continue;
            }

            let col_id = ui.id().with("col").with(&node.id).with(col);
            let col_sense = if is_hand_mode {
                egui::Sense::hover()
            } else {
                egui::Sense::click()
            };
            let mut response = ui.interact(col_rect, col_id, col_sense);

            let is_selected_col = selected_column
                .as_ref()
                .is_some_and(|(t, c)| *t == node.id && c == col);

            if !is_hand_mode {
                // Context menu saat klik kanan pada kolom
                response.context_menu(|ui| {
                    ui.label(egui::RichText::new(format!("{}.{}", node.id, col)).strong());
                    if let Some(c_type) =
                        info.map(|c| c.type_name.as_str()).filter(|t| !t.is_empty())
                    {
                        ui.label(
                            egui::RichText::new(format!("Type: {c_type}"))
                                .weak()
                                .small(),
                        );
                    }
                    ui.separator();

                    if ui.button("🔍 Search relation").clicked() {
                        ui.close();
                        search_relations_for_column = Some((node.id.clone(), col.clone()));
                    }

                    ui.separator();
                    if is_selected_col {
                        if ui.button("Deselect column").clicked() {
                            ui.close();
                            column_clicked_request = Some((String::new(), String::new()));
                        }
                    } else if ui
                        .button("🔗 Select for manual relation (Ctrl+Click)")
                        .clicked()
                    {
                        ui.close();
                        column_clicked_request = Some((node.id.clone(), col.clone()));
                    }
                });
            }

            // Tooltip interaktif saat ada kolom yang sedang dipilih dari tabel lain
            if let Some((sel_table, sel_col)) = selected_column.as_ref() {
                if *sel_table != node.id {
                    response = response
                        .on_hover_text(format!("Ctrl+Click to link with {sel_table}.{sel_col}"));
                }
            }

            let is_link_target_hover = (ctrl_down || shift_down)
                && response.hovered()
                && selected_column.as_ref().is_some_and(|(t, _)| *t != node.id);

            if !is_hand_mode && response.clicked() {
                let modifier_active = ctrl_down || shift_down;
                match selected_column.as_ref() {
                    // Ada kolom terpilih di tabel lain:
                    Some((sel_table, sel_col)) if *sel_table != node.id => {
                        if modifier_active {
                            // Ctrl+klik kolom tabel kedua -> buat relasi manual!
                            let this_is_pk = is_pk;
                            let sel_is_pk = sel_col_is_pk;

                            let (child_table, child_col, parent_table, parent_col) =
                                if this_is_pk && !sel_is_pk {
                                    (
                                        sel_table.clone(),
                                        sel_col.clone(),
                                        node.id.clone(),
                                        col.clone(),
                                    )
                                } else if sel_is_pk && !this_is_pk {
                                    (
                                        node.id.clone(),
                                        col.clone(),
                                        sel_table.clone(),
                                        sel_col.clone(),
                                    )
                                } else {
                                    (
                                        sel_table.clone(),
                                        sel_col.clone(),
                                        node.id.clone(),
                                        col.clone(),
                                    )
                                };

                            link_request = Some(VirtualRelation {
                                child: child_table,
                                child_column: child_col,
                                parent: parent_table,
                                parent_column: parent_col,
                                origin: RelationOrigin::Manual,
                            });
                        } else {
                            column_clicked_request = Some((node.id.clone(), col.clone()));
                        }
                    }
                    // Kolom pada tabel yang sama:
                    Some((sel_table, sel_col)) if *sel_table == node.id => {
                        if sel_col == col && modifier_active {
                            // Deselect saat Ctrl+klik kolom yang sama
                            column_clicked_request = Some((String::new(), String::new()));
                        } else {
                            column_clicked_request = Some((node.id.clone(), col.clone()));
                        }
                    }
                    _ => column_clicked_request = Some((node.id.clone(), col.clone())),
                }
            }

            let is_col_search_match = state.show_search
                && state.search_columns
                && !state.search_query.is_empty()
                && col.to_lowercase().contains(&search_lower);
            let is_relation_col = relation_cols.contains(&(node.id.clone(), col.clone()));

            if is_selected_col {
                // Highlight jelas kolom sumber terpilih (emas dengan border)
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(255, 215, 0).linear_multiply(0.35),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.5 * scale, egui::Color32::from_rgb(255, 215, 0)),
                    egui::StrokeKind::Inside,
                );
            } else if is_link_target_hover {
                // Highlight kolom target saat di-hover dengan Ctrl (cyan terang)
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(0, 200, 220).linear_multiply(0.25),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.5 * scale, egui::Color32::from_rgb(0, 200, 220)),
                    egui::StrokeKind::Inside,
                );
            } else if is_relation_col {
                // Kolom ujung relasi terpilih: glow emas senada dengan glow tabel.
                ui.painter().rect_filled(
                    col_rect.expand2(egui::vec2(0.0, 1.5 * scale)),
                    2.0 * scale,
                    egui::Color32::from_rgb(255, 215, 0).linear_multiply(0.18),
                );
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(255, 215, 0).linear_multiply(0.30),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.5 * scale, egui::Color32::from_rgb(255, 215, 0)),
                    egui::StrokeKind::Inside,
                );
            } else if is_col_search_match {
                // Highlight kolom pencarian (merah lembut dengan aksen)
                ui.painter().rect_filled(
                    col_rect,
                    0.0,
                    egui::Color32::from_rgb(255, 60, 60).linear_multiply(0.25),
                );
                ui.painter().rect_stroke(
                    col_rect,
                    0.0,
                    egui::Stroke::new(1.0 * scale, egui::Color32::from_rgb(255, 80, 80)),
                    egui::StrokeKind::Inside,
                );
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(col_rect, 0.0, egui::Color32::from_white_alpha(10));
            }

            // Aksen tipis di tepi kiri baris kunci: PK / FK / PK+FK.
            if let Some(c) = key.color() {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        col_pos_screen + egui::vec2(1.0 * scale, 2.0 * scale),
                        egui::vec2(3.0 * scale, item_height - 4.0 * scale),
                    ),
                    1.0 * scale,
                    c,
                );
            }

            let name_color = if is_col_search_match {
                egui::Color32::WHITE
            } else {
                key.color().unwrap_or(egui::Color32::LIGHT_GRAY)
            };
            ui.painter().text(
                node_pos_screen + egui::vec2(8.0 * scale, y_offset),
                egui::Align2::LEFT_TOP,
                col,
                egui::FontId::monospace(quantize_font(12.0 * scale)),
                name_color,
            );

            // Tipe (warna per kategori) paling kanan, badge kunci di kirinya.
            let small = egui::FontId::monospace(quantize_font(10.0 * scale));
            let mut right_x = node_rect.right() - 8.0 * scale;
            if !type_name.is_empty() {
                let mut ty: String = type_name.chars().take(14).collect();
                if type_name.chars().count() > 14 {
                    ty.push('…');
                }
                let r = ui.painter().text(
                    egui::pos2(right_x, col_pos_screen.y),
                    egui::Align2::RIGHT_TOP,
                    ty,
                    small.clone(),
                    column_type_color(type_name).linear_multiply(0.85),
                );
                right_x = r.left() - 5.0 * scale;
            }
            if let Some(c) = key.color() {
                ui.painter().text(
                    egui::pos2(right_x, col_pos_screen.y),
                    egui::Align2::RIGHT_TOP,
                    key.badge(),
                    small,
                    c,
                );
            }
            y_offset += item_height;
        }

        if is_dimmed {
            dim_node(ui, node_rect, scale, canvas_bg);
        }

        let anchor = crate::models::structs::NoteAnchor::Table(node.id.clone());
        if let Some(&count) = note_counts.get(&anchor) {
            let active = state
                .notes
                .iter()
                .any(|n| n.anchor == anchor && state.open_notes.contains(&n.id));
            if let Some(resp) = crate::diagram_notes_view::draw_badge(
                ui,
                egui::pos2(node_rect.right() - 10.0 * scale, node_rect.top()),
                egui::Align2::RIGHT_CENTER,
                count,
                scale,
                ui.id().with("node_note_badge").with(&node.id),
                active,
            ) {
                let resp = resp.on_hover_text(crate::diagram_notes_view::badge_tooltip(count));
                if resp.clicked() {
                    note_request = Some(crate::diagram_notes_view::NoteRequest::Toggle(
                        anchor.clone(),
                    ));
                }
                if resp.secondary_clicked() {
                    note_request = Some(crate::diagram_notes_view::NoteRequest::List(anchor));
                }
            }
        }

        if let Some(&count) = endpoint_counts.get(&node.id) {
            let active = state.endpoints_panel.as_deref() == Some(node.id.as_str());
            if let Some(resp) = crate::diagram_endpoints_view::draw_badge(
                ui,
                egui::pos2(node_rect.left() + 10.0 * scale, node_rect.top()),
                count,
                table_crud.get(&node.id).map_or("", String::as_str),
                scale,
                ui.id().with("node_endpoint_badge").with(&node.id),
                active,
            ) {
                let resp = resp.on_hover_text(crate::diagram_endpoints_view::badge_tooltip(
                    &state.endpoint_links,
                    &node.id,
                ));
                if resp.clicked() {
                    endpoint_panel_request = Some(node.id.clone());
                }
            }
        }
    }
    if let Some(table) = endpoint_panel_request {
        state.endpoints_panel = if state.endpoints_panel.as_deref() == Some(table.as_str()) {
            None
        } else {
            Some(table)
        };
        state.endpoints_panel_query.clear();
    }
    if let Some(table) = relations_request {
        state.relations_panel = Some(table);
        state.relations_panel_query.clear();
    }

    if !draw_flow_animation(ui, state, &to_screen, now) {
        state.flow_anim = None;
    }

    if let Some(req) = focus_request {
        state.focus_table = req;
        state.focus_group = None;
    }
    if let Some(id) = double_clicked_node {
        start_focus_animation(state, &id, rect.size(), now);
        ui.ctx().request_repaint();
    }

    if let Some((gid, pos)) = empty_group_retention {
        let has_other_members = state.nodes.iter().any(|n| n.is_in_group(&gid));
        if !has_other_members {
            if let Some(g) = state.groups.iter_mut().find(|g| g.id == gid) {
                if g.manual_pos.is_none() {
                    g.manual_pos = Some(pos);
                }
            }
        }
    }
    if let Some(pos) = add_group_at_pos {
        state.add_group_popup = Some(pos);
        state.new_group_buffer.clear();
    }
    if let Some(link_id) = open_source_request {
        action = Some(DiagramAction::OpenLinkedDiagram(link_id));
    }

    // Clear selection if clicked on background (and not on an edge or node)
    // We check `response` from the beginning of the function (passed down? no it was `ui.interact(rect...)`)
    // We need to check if the main rect was clicked, and ensure no edge/node was clicked.
    if !is_hand_mode
        && ui.input(|i| i.pointer.primary_clicked())
        && !node_clicked
        && !edge_was_clicked
        && column_clicked_request.is_none()
    {
        // But wait, `ui.interact` for background handles drag. Does it also report click?
        // We can check if the pointer is within the clip rect and nothing else claimed it?
        // Simpler: If the background response was clicked?
        // Accessing `response` from top of function might be hard unless we passed it.
        // Let's rely on global input.
        if ui.rect_contains_pointer(rect) {
            state.selected_edge = None;
            state.selected_column = None;
            state.selected_virtual = None;
        }
    }

    if let Some(req) = column_clicked_request {
        if req.0.is_empty() {
            state.selected_column = None;
        } else {
            state.selected_column = Some(req);
        }
    }
    if let Some(rel) = link_request {
        let label = format!(
            "Linked {}.{} → {}.{}",
            rel.child, rel.child_column, rel.parent, rel.parent_column
        );
        if crate::diagram_relations::add_virtual_relation(state, rel) {
            state.save_requested = true;
            action = Some(DiagramAction::Info(label));
        }
        state.selected_column = None;
    }
    if let Some((table, column)) = search_relations_for_column {
        let suggestions =
            crate::diagram_relations::suggest_relations_for_column(state, &table, &column);
        state.relation_suggestions_title = Some(format!("{table}.{column}"));
        state.relation_column_search_query = column.clone();
        state.relation_database_filter = None;
        state.relation_suggestions = Some(suggestions.into_iter().map(|s| (s, true)).collect());
    }
    if let Some(id) = remove_node_request {
        state.nodes.retain(|n| n.id != id);
        state
            .virtual_relations
            .retain(|r| r.child != id && r.parent != id);
        state.selected_virtual = None;
        state.save_requested = true;
    }
    // Delete / Backspace menghapus relasi virtual terpilih (bila tidak sedang mengetik).
    if let Some(idx) = state.selected_virtual
        && state.relation_editor.is_none()
        && ui.memory(|m| m.focused().is_none())
        && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
    {
        remove_virtual(state, idx);
    }

    if let Some(id) = dragging_node_id
        && let Some(node) = state.nodes.iter_mut().find(|n| n.id == id)
    {
        node.pos += drag_delta / scale;
    }

    if let Some(id) = drag_stopped_node_id {
        if state.prevent_overlap {
            resolve_all_overlaps(&mut state.nodes, 20.0, Some(&id));
            state.save_requested = true;
        }
    }

    if !flow_frame.rects.is_empty() {
        let flow = crate::diagram_flow_view::render_flow_cards(
            ui,
            state,
            &flow_frame,
            &to_screen,
            rect,
            !is_hand_mode,
            focus_set.as_ref(),
            now,
        );
        if flow.action.is_some() {
            action = flow.action;
        }
    }
    // Animasi pemutaran flow card (di atas tabel dan card) dan bilah
    // kontrolnya. Card yang baru dihapus frame ini menggeser indeks, jadi
    // dilewati sampai frame berikutnya.
    if state.flow_play.is_some() && flow_frame.rects.len() == state.flow_cards.len() {
        let index_of = |id: &str| state.flow_cards.iter().position(|c| c.id == id);
        let card_rect = |id: &str| {
            let r = flow_frame.drawn_rect(state, index_of(id)?);
            Some(egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max)))
        };
        let curve = |id: &str, table: &str| {
            crate::diagram_flow_view::card_curve(
                state,
                &flow_frame,
                index_of(id)?,
                table,
                &to_screen,
            )
        };
        crate::diagram_flow_play_view::draw_playback(
            ui, state, &to_screen, &card_rect, &curve, now,
        );
        let play_rect = state.flow_play.as_ref().and_then(|p| card_rect(&p.card_id));
        crate::diagram_flow_play_view::render_play_controls(ui, state, play_rect);
    }

    // Kartu note digambar di atas tabel dan di bawah toolbar.
    let cards = crate::diagram_notes_view::render_note_cards(
        ui,
        state,
        &visible_notes,
        &to_screen,
        !is_hand_mode,
    );
    if let Some(id) = cards.edit {
        crate::diagram_notes_view::open_note_editor(state, &id);
    }
    if let Some(table) = cards.jump_table {
        start_view_animation(state, &table, rect.size(), now);
        ui.ctx().request_repaint();
    }
    if let Some(req) = note_request {
        crate::diagram_notes_view::apply_request(state, req);
    }

    // Indikator sinkronisasi skema live (tampilan masih dari cache).
    if state.schema_syncing {
        let text = if state.nodes.is_empty() {
            "Loading schema…"
        } else {
            "Syncing schema…"
        };
        let spinner_rect = egui::Rect::from_center_size(
            rect.center_top() + egui::vec2(-60.0, 24.0),
            egui::vec2(14.0, 14.0),
        );
        ui.put(spinner_rect, egui::Spinner::new().size(14.0));
        ui.painter().text(
            spinner_rect.right_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
    }

    // Floating Toolbar: Zoom & Navigasi, Grid, Layout, Relasi, Save (+ cloud sync), Import & Export.
    let toolbar_id = ui.id().with("diagram_floating_toolbar_width");
    let measured_width: f32 = ui.data(|d| d.get_temp(toolbar_id)).unwrap_or(720.0);
    let toolbar_width = measured_width.max(TOOLBAR_BTN_SIZE * 4.0);
    let toolbar_height = TOOLBAR_BTN_SIZE + TOOLBAR_PADDING * 2.0;
    let focus_chip_rect = render_focus_chip(ui, state, rect, &mut action);

    let toolbar_rect = egui::Rect::from_min_size(
        rect.right_bottom() + egui::vec2(-toolbar_width - 16.0, -toolbar_height - 16.0),
        egui::vec2(toolbar_width, toolbar_height),
    );

    let card_fill = ui.visuals().window_fill.gamma_multiply(TOOLBAR_OPACITY);
    let mut card_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    card_stroke.color = card_stroke.color.gamma_multiply(TOOLBAR_OPACITY);
    ui.painter().rect_filled(toolbar_rect, 6.0, card_fill);
    ui.painter()
        .rect_stroke(toolbar_rect, 6.0, card_stroke, egui::StrokeKind::Middle);

    let toolbar_res = ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("diagram_floating_toolbar")
            .max_rect(toolbar_rect.shrink(TOOLBAR_PADDING)),
        |ui| {
            ui.visuals_mut().widgets.noninteractive.bg_stroke.color = ui
                .visuals()
                .widgets
                .noninteractive
                .bg_stroke
                .color
                .gamma_multiply(TOOLBAR_OPACITY);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

                // --- 1. Zoom & Navigasi ---
                if toolbar_square_button(ui, egui_icons::icons::ICON_REMOVE.codepoint, "Out", false)
                    .on_hover_text("Zoom Out (Cmd -)")
                    .clicked()
                {
                    set_zoom_centered(state, state.zoom / 1.15, rect.size());
                }

                // Persentase zoom ditampilkan di posisi ikon.
                let zoom_text = format!("{:.0}%", state.zoom * 100.0);
                if toolbar_square_button(ui, &zoom_text, "Zoom", false)
                    .on_hover_text("Reset Zoom to 100% (Cmd 0)")
                    .clicked()
                {
                    set_zoom_centered(state, DEFAULT_ZOOM, rect.size());
                }

                if toolbar_square_button(ui, egui_icons::icons::ICON_ADD.codepoint, "In", false)
                    .on_hover_text("Zoom In (Cmd +)")
                    .clicked()
                {
                    set_zoom_centered(state, state.zoom * 1.15, rect.size());
                }

                if toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_FILTER_CENTER_FOCUS.codepoint,
                    "Fit",
                    false,
                )
                .on_hover_text("Zoom out until the whole diagram fits, then center it")
                .clicked()
                {
                    fit_diagram(state, rect.size());
                }

                if toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_PAN_TOOL.codepoint,
                    "Hand",
                    is_hand_mode,
                )
                    .on_hover_text("Hand Tool (H or hold Space)\nClick and drag anywhere to pan diagram navigation")
                    .clicked()
                {
                    state.hand_tool = !state.hand_tool;
                }

                ui.separator();

                // --- 2. Layout Menu (Grid & Anti-Overlap sebagai checkbox) ---
                let layout_btn = toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_VIEW_MODULE.codepoint,
                    "Layout",
                    false,
                )
                .on_hover_text("Layout options");
                egui::Popup::menu(&layout_btn).show(
                    |ui| {
                        if ui
                            .checkbox(&mut state.show_grid, "Show grid")
                            .on_hover_text("Show or hide the background grid")
                            .clicked()
                        {
                            state.save_requested = true;
                        }
                        if ui
                            .checkbox(&mut state.prevent_overlap, "Prevent table & group overlap")
                            .on_hover_text("When enabled, tables and groups that share no table will not overlap when moved or organized")
                            .clicked()
                        {
                            if state.prevent_overlap {
                                resolve_all_overlaps(&mut state.nodes, 20.0, None);
                            }
                            state.save_requested = true;
                        }
                        ui.separator();
                        if ui
                            .button("⚡ Auto Arrange")
                            .on_hover_text("Arrange all tables, then compact groups and resolve overlaps")
                            .clicked()
                        {
                            ui.close();
                            auto_arrange(state);
                            state.save_requested = true;
                        }
                    },
                );

                ui.separator();

                // --- 3. Relations & Database Sync ---
                let relations_btn = toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_LINK.codepoint,
                    "Links",
                    false,
                )
                .on_hover_text("Relations");
                egui::Popup::menu(&relations_btn).show(
                    |ui| {
                        if ui
                            .checkbox(&mut state.show_relations, "Show relationship links")
                            .on_hover_text("Show or hide relationship links between table columns (L)")
                            .clicked()
                        {
                            state.save_requested = true;
                        }
                        ui.add_enabled_ui(state.show_relations, |ui| {
                            ui.indent("relation_kinds", |ui| {
                                for (value, label, hint) in [
                                    (
                                        &mut state.show_fk_relations,
                                        "Foreign keys",
                                        "Relations defined by database foreign keys",
                                    ),
                                    (
                                        &mut state.show_virtual_relations,
                                        "Virtual relations",
                                        "Suggested, manual and imported relations",
                                    ),
                                    (
                                        &mut state.show_linked_relations,
                                        "Linked database relations",
                                        "Relations that come from linked databases",
                                    ),
                                ] {
                                    if ui.checkbox(value, label).on_hover_text(hint).changed() {
                                        state.save_requested = true;
                                    }
                                }
                            });
                        });
                        ui.label(
                            egui::RichText::new(
                                "Zoom out to see relations merged per table pair\n(and per group below 25%).",
                            )
                            .weak()
                            .small(),
                        );
                        ui.separator();
                        if ui.button("🔍 Suggest from all similar columns…").clicked() {
                            ui.close();
                            // Diagram besar bisa butuh beberapa detik; jalankan di background.
                            // Jendela saran langsung terbuka dan menampilkan loading sampai
                            // hasilnya diambil di render_relation_suggestions.
                            let repaint_ctx = ui.ctx().clone();
                            state.relation_suggest_job =
                                Some(crate::diagram_relations::RelationSuggestJob::spawn(
                                    state.nodes.clone(),
                                    state.virtual_relations.clone(),
                                    move || repaint_ctx.request_repaint(),
                                ));
                            state.relation_suggestions = Some(Vec::new());
                            state.relation_suggestions_title = Some("all tables".to_string());
                            state.relation_column_search_query.clear();
                            state.relation_database_filter = None;
                        }
                        if ui.button("🔎 Search relations by column name…").clicked() {
                            ui.close();
                            state.relation_suggestions_title = Some("Search by column name".to_string());
                            state.relation_column_search_query.clear();
                            state.relation_database_filter = None;
                            state.relation_suggestions = Some(Vec::new());
                        }
                        if let Some((sel_table, sel_col)) = &state.selected_column {
                            if ui
                                .button(format!("Search relations for {sel_table}.{sel_col}…"))
                                .clicked()
                            {
                                ui.close();
                                let suggestions =
                                    crate::diagram_relations::suggest_relations_for_column(
                                        state, sel_table, sel_col,
                                    );
                                state.relation_suggestions_title =
                                    Some(format!("{sel_table}.{sel_col}"));
                                state.relation_column_search_query = sel_col.clone();
                                state.relation_database_filter = None;
                                state.relation_suggestions =
                                    Some(suggestions.into_iter().map(|s| (s, true)).collect());
                            }
                        }
                        let removable = state
                            .virtual_relations
                            .iter()
                            .filter(|r| r.origin != RelationOrigin::Imported)
                            .count();
                        if ui
                            .add_enabled(
                                removable > 0,
                                egui::Button::new(format!(
                                    "Remove suggested & manual relations ({removable})"
                                )),
                            )
                            .clicked()
                        {
                            ui.close();
                            state
                                .virtual_relations
                                .retain(|r| r.origin == RelationOrigin::Imported);
                            state.selected_virtual = None;
                            state.save_requested = true;
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(
                                "Manual link: Ctrl+click a column, then Ctrl+click\nthe target column in another table.\nRight-click a column for automatic search.\nSelect a dashed line and press Delete to remove it.",
                            )
                            .weak()
                            .small(),
                        );
                    },
                );

                // --- 4. Multi-Database: Link Database ---
                if toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_ADD_LINK.codepoint,
                    "Link DB",
                    false,
                )
                    .on_hover_text(
                        "Link Database…\nShow every table of another database in its own container.\nThe container follows that database's diagram when it changes.",
                    )
                    .clicked()
                {
                    action = Some(DiagramAction::OpenLinkDatabaseModal);
                }
                if !state.linked_databases.is_empty()
                    && toolbar_square_button(
                        ui,
                        egui_icons::icons::ICON_REFRESH.codepoint,
                        "Reload",
                        false,
                    )
                        .on_hover_text("Reload linked databases from their source diagrams")
                        .clicked()
                {
                    action = Some(DiagramAction::RefreshLinks(None));
                }

                // --- 4b. Notes ---
                if toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_STICKY_NOTE_2.codepoint,
                    "Notes",
                    state.notes_panel.is_some(),
                )
                .on_hover_text(
                    "Notes\nMarkdown sticky notes on tables and groups, saved with the diagram.\nRight-click a table or group to add one.",
                )
                .clicked()
                {
                    state.notes_panel = if state.notes_panel.is_some() {
                        None
                    } else {
                        Some(None)
                    };
                }

                // --- 4c. Table Groups (Comment based) ---
                let groups_btn = toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_FOLDER.codepoint,
                    "Groups",
                    state.focus_group.is_some(),
                )
                .on_hover_text("Table Groups\nView, focus, or open groups and sub groups created from table comments");
                // Tetap terbuka saat diklik di dalam, supaya kolom filter bisa diketik.
                egui::Popup::menu(&groups_btn)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                    ui.set_min_width(280.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("🏷️ Groups & Sub Groups").strong());
                        if let Some(ref pat) = state.group_pattern {
                            ui.label(egui::RichText::new(format!("({pat})")).weak().small());
                        }
                    });

                    ui.add_space(4.0);

                    // Tombol konfigurasi comment group ditaruh di ATAS agar selalu terlihat dan tidak terpotong layar
                    if ui
                        .button("⚙ Configure Comment Groups…")
                        .on_hover_text("Configure pattern [GROUP]-[SUB GROUP] and enable/disable table grouping")
                        .clicked()
                    {
                        action = Some(DiagramAction::ConfigureTableGroups);
                        if let Some(cid) = state.nodes.iter().find_map(|n| n.connection_id) {
                            let db = state
                                .nodes
                                .iter()
                                .find_map(|n| n.database_name.clone())
                                .unwrap_or_default();
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("open_table_group_dialog"), (cid, db));
                            });
                        }
                        ui.close();
                    }

                    // Tombol reset fokus bila mode fokus sedang aktif
                    if state.focus_group.is_some() || state.focus_table.is_some() {
                        if ui.button("✖ Clear Focus / Show All Tables").clicked() {
                            state.focus_group = None;
                            state.focus_table = None;
                            state.focus_flow = None;
                            ui.close();
                        }
                    }

                    // Group yang dihapus bisa dikembalikan: penandanya dibuang lalu
                    // skema diambil ulang.
                    if !state.deleted_group_ids.is_empty()
                        && ui
                            .button(format!(
                                "Restore deleted groups ({})",
                                state.deleted_group_ids.len()
                            ))
                            .on_hover_text("Bring back groups deleted from this diagram")
                            .clicked()
                    {
                        state.deleted_group_ids.clear();
                        state.save_requested = true;
                        action = Some(DiagramAction::ReloadSchema);
                    }

                    // Search box filter untuk grup bila grup banyak
                    let filter_id = ui.id().with("table_group_filter_text");
                    let mut filter_text: String = ui.ctx().data(|d| d.get_temp(filter_id)).unwrap_or_default();
                    let filter_edit = ui.add(
                        egui::TextEdit::singleline(&mut filter_text)
                            .hint_text("🔍 Filter groups…")
                            .desired_width(f32::INFINITY),
                    );
                    if filter_edit.changed() {
                        ui.ctx().data_mut(|d| d.insert_temp(filter_id, filter_text.clone()));
                    }

                    // Konfirmasi hapus: (pesan, id group yang dihapus).
                    let pending_delete_id = ui.id().with("table_group_pending_delete");
                    let pending_delete: Option<(String, Vec<String>)> =
                        ui.ctx().data(|d| d.get_temp(pending_delete_id));
                    if let Some((message, ids)) = pending_delete {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(message).color(ui.visuals().warn_fg_color));
                        ui.horizontal(|ui| {
                            let delete_clicked = ui
                                .button(
                                    egui::RichText::new("Delete")
                                        .color(egui::Color32::from_rgb(240, 80, 80)),
                                )
                                .clicked();
                            if delete_clicked {
                                delete_groups(state, &ids);
                            }
                            if delete_clicked || ui.button("Cancel").clicked() {
                                ui.ctx().data_mut(|d| {
                                    d.remove_temp::<(String, Vec<String>)>(pending_delete_id)
                                });
                            }
                        });
                    }

                    ui.separator();

                    if state.groups.is_empty() {
                        ui.label(egui::RichText::new("No groups found").weak());
                    } else {
                        // Kumpulkan dan kelompokkan berdasarkan nama Main Group
                        struct SubItem {
                            sub_name: String,
                            group_id: String,
                            count: usize,
                            color: egui::Color32,
                        }
                        struct MainGroupHierarchy {
                            main_name: String,
                            main_group_id: Option<String>,
                            sub_groups: Vec<SubItem>,
                            total_count: usize,
                            color: egui::Color32,
                        }

                        let mut main_map: std::collections::BTreeMap<String, MainGroupHierarchy> = std::collections::BTreeMap::new();

                        for grp in &state.groups {
                            let member_count = state
                                .nodes
                                .iter()
                                .filter(|n| n.group_ids.contains(&grp.id) || n.group_id.as_deref() == Some(&grp.id))
                                .count();

                            if let Some((main_part, sub_part)) = grp.title.split_once(" - ") {
                                let entry = main_map.entry(main_part.to_string()).or_insert_with(|| MainGroupHierarchy {
                                    main_name: main_part.to_string(),
                                    main_group_id: None,
                                    sub_groups: Vec::new(),
                                    total_count: 0,
                                    color: grp.color,
                                });
                                entry.sub_groups.push(SubItem {
                                    sub_name: sub_part.to_string(),
                                    group_id: grp.id.clone(),
                                    count: member_count,
                                    color: grp.color,
                                });
                            } else {
                                let entry = main_map.entry(grp.title.clone()).or_insert_with(|| MainGroupHierarchy {
                                    main_name: grp.title.clone(),
                                    main_group_id: Some(grp.id.clone()),
                                    sub_groups: Vec::new(),
                                    total_count: 0,
                                    color: grp.color,
                                });
                                entry.main_group_id = Some(grp.id.clone());
                                entry.color = grp.color;
                            }
                        }

                        // Hitung total tabel per main group
                        for entry in main_map.values_mut() {
                            let mut member_set = std::collections::HashSet::new();
                            if let Some(ref mg_id) = entry.main_group_id {
                                for n in &state.nodes {
                                    if n.is_in_group(mg_id) {
                                        member_set.insert(&n.id);
                                    }
                                }
                            }
                            for sub in &entry.sub_groups {
                                for n in &state.nodes {
                                    if n.is_in_group(&sub.group_id) {
                                        member_set.insert(&n.id);
                                    }
                                }
                            }
                            entry.total_count = member_set.len();
                        }

                        let active_focus = state.focus_group.clone();
                        let mut focus_req: Option<String> = None;
                        let mut delete_req: Option<(String, Vec<String>)> = None;
                        let filter_needle = filter_text.trim().to_lowercase();
                        let delete_button = |ui: &mut egui::Ui, hover: &str| {
                            ui.small_button(
                                egui::RichText::new(egui_icons::icons::ICON_DELETE.codepoint)
                                    .color(egui::Color32::from_rgb(240, 80, 80)),
                            )
                            .on_hover_text(hover)
                            .clicked()
                        };

                        egui::ScrollArea::vertical()
                            .max_height(350.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let mut any_matched = false;
                                for (_name, entry) in main_map {
                                    if !filter_needle.is_empty() {
                                        let main_match = entry.main_name.to_lowercase().contains(&filter_needle);
                                        let sub_match = entry.sub_groups.iter().any(|s| s.sub_name.to_lowercase().contains(&filter_needle));
                                        if !main_match && !sub_match {
                                            continue;
                                        }
                                    }
                                    any_matched = true;

                                    if entry.sub_groups.is_empty() {
                                        // Group tunggal tanpa sub-group
                                        ui.horizontal(|ui| {
                                            let (rect_badge, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                            ui.painter().rect_filled(rect_badge, 2.0, entry.color);
                                            let is_focused = entry.main_group_id.as_ref() == active_focus.as_ref();
                                            let label_text = if is_focused {
                                                egui::RichText::new(format!("{} ({})", entry.main_name, entry.total_count)).strong().underline()
                                            } else {
                                                egui::RichText::new(format!("{} ({})", entry.main_name, entry.total_count))
                                            };
                                            if ui.button(label_text).clicked() {
                                                if let Some(ref mg_id) = entry.main_group_id {
                                                    focus_req = Some(mg_id.clone());
                                                    ui.close();
                                                }
                                            }
                                            if let Some(ref mg_id) = entry.main_group_id {
                                                if ui.small_button("↗").on_hover_text("Open in new diagram tab").clicked() {
                                                    action = Some(DiagramAction::OpenGroupInNewTab(mg_id.clone()));
                                                    ui.close();
                                                }
                                                if delete_button(ui, "Delete this group") {
                                                    delete_req = Some((
                                                        format!("Delete group \"{}\"? Its tables are kept.", entry.main_name),
                                                        vec![mg_id.clone()],
                                                    ));
                                                }
                                            }
                                        });
                                    } else {
                                        // Main group dengan sub-groups
                                        let id = ui.make_persistent_id(&entry.main_name);
                                        // Hapus group utama ikut menghapus semua sub group-nya.
                                        let cascade_ids: Vec<String> = entry
                                            .main_group_id
                                            .iter()
                                            .cloned()
                                            .chain(entry.sub_groups.iter().map(|s| s.group_id.clone()))
                                            .collect();
                                        let sub_total = entry.sub_groups.len();
                                        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true)
                                            .show_header(ui, |ui| {
                                                let (rect_badge, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                                                ui.painter().rect_filled(rect_badge, 2.0, entry.color);
                                                let is_focused = entry.main_group_id.as_ref() == active_focus.as_ref();
                                                let text = if is_focused {
                                                    egui::RichText::new(format!("{} ({})", entry.main_name, entry.total_count)).strong().underline()
                                                } else {
                                                    egui::RichText::new(format!("{} ({})", entry.main_name, entry.total_count)).strong()
                                                };
                                                if ui.selectable_label(is_focused, text).clicked() {
                                                    if let Some(ref mg_id) = entry.main_group_id {
                                                        focus_req = Some(mg_id.clone());
                                                        ui.close();
                                                    }
                                                }
                                                if let Some(ref mg_id) = entry.main_group_id {
                                                    if ui.small_button("↗").on_hover_text("Open all tables in this group in new tab").clicked() {
                                                        action = Some(DiagramAction::OpenGroupInNewTab(mg_id.clone()));
                                                        ui.close();
                                                    }
                                                }
                                                if delete_button(ui, "Delete this group and all its sub groups") {
                                                    delete_req = Some((
                                                        format!(
                                                            "Delete group \"{}\" and its {} sub group(s)? Its tables are kept.",
                                                            entry.main_name, sub_total
                                                        ),
                                                        cascade_ids.clone(),
                                                    ));
                                                }
                                            })
                                            .body(|ui| {
                                                for sub in entry.sub_groups {
                                                    if !filter_needle.is_empty()
                                                        && !entry.main_name.to_lowercase().contains(&filter_needle)
                                                        && !sub.sub_name.to_lowercase().contains(&filter_needle)
                                                    {
                                                        continue;
                                                    }
                                                    ui.horizontal(|ui| {
                                                        ui.add_space(8.0);
                                                        let (rect_sub, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                                                        ui.painter().rect_filled(rect_sub, 1.5, sub.color);
                                                        let is_sub_focused = active_focus.as_deref() == Some(&sub.group_id);
                                                        let sub_text = if is_sub_focused {
                                                            egui::RichText::new(format!("{} ({})", sub.sub_name, sub.count)).underline()
                                                        } else {
                                                            egui::RichText::new(format!("{} ({})", sub.sub_name, sub.count))
                                                        };
                                                        if ui.selectable_label(is_sub_focused, sub_text).clicked() {
                                                            focus_req = Some(sub.group_id.clone());
                                                            ui.close();
                                                        }
                                                        if ui.small_button("↗").on_hover_text("Open this sub group in new tab").clicked() {
                                                            action = Some(DiagramAction::OpenGroupInNewTab(sub.group_id.clone()));
                                                            ui.close();
                                                        }
                                                        if delete_button(ui, "Delete this sub group") {
                                                            delete_req = Some((
                                                                format!(
                                                                    "Delete sub group \"{} - {}\"? Its tables are kept.",
                                                                    entry.main_name, sub.sub_name
                                                                ),
                                                                vec![sub.group_id.clone()],
                                                            ));
                                                        }
                                                    });
                                                }
                                            });
                                    }
                                }
                                if !any_matched {
                                    ui.label(egui::RichText::new("No groups match filter").weak());
                                }
                            });

                        if let Some(req) = delete_req {
                            ui.ctx().data_mut(|d| d.insert_temp(pending_delete_id, req));
                        }
                        if let Some(gid) = focus_req {
                            state.focus_group = Some(gid.clone());
                            state.focus_table = None;
                            state.focus_flow = None;
                            animate_to_group_center(state, &gid, rect.size(), now);
                        }
                    }
                });

                ui.separator();

                // --- 4. Save (database target + cache lokal + Obsidian) ---
                use crate::models::structs::DiagramDbStatus;
                let status_text = db_status_text(&state.db_status);
                let not_in_db = matches!(
                    state.db_status,
                    DiagramDbStatus::NotInDatabase | DiagramDbStatus::LocalOnly(_)
                );
                let pending = state.unsaved_changes || state.db_dirty_since.is_some() || not_in_db;
                let mut save_hover =
                    "Save diagram to table `diagram_by_tabular` in this database (Cmd S)".to_string();
                if state.unsaved_changes {
                    save_hover = format!("Unsaved changes (auto save is off)\n{save_hover}");
                }
                save_hover = format!("{save_hover}\n{status_text}");
                // Tombol Save tampil aktif selama ada perubahan belum tersimpan.
                if toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_SAVE.codepoint,
                    "Save",
                    pending,
                )
                .on_hover_text(save_hover)
                .clicked()
                {
                    state.save_requested = true;
                    state.unsaved_changes = false;
                    action = Some(DiagramAction::Save);
                }

                let more_btn = toolbar_dropdown_arrow(ui, "save_more");
                egui::Popup::menu(&more_btn).show(|ui| {
                    if ui
                        .checkbox(&mut state.auto_save, "Auto save")
                        .on_hover_text(
                            "Save changes automatically to the database a few seconds after you stop editing.\nWhen off, changes are kept until you press Save (Cmd S).",
                        )
                        .changed()
                    {
                        // Simpan preferensi ini sekaligus perubahan yang tertunda.
                        state.save_requested = true;
                        state.force_save = true;
                    }
                    ui.separator();
                    if ui
                        .button("Sync with database now")
                        .on_hover_text(
                            "Compare this diagram with the copy in `diagram_by_tabular` and merge changes made by others",
                        )
                        .clicked()
                    {
                        ui.close();
                        action = Some(DiagramAction::SyncWithDatabase);
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(status_text.as_str()).weak().small());
                });

                if let Some(merge) = state.pending_merge.as_mut() {
                    if toolbar_square_button(
                        ui,
                        egui_icons::icons::ICON_CALL_MERGE.codepoint,
                        "Resolve merge",
                        true,
                    )
                    .on_hover_text(
                        "This diagram was changed in the database too. Click to resolve the differences.",
                    )
                    .clicked()
                    {
                        merge.visible = true;
                    }
                } else if matches!(state.db_status, DiagramDbStatus::LocalOnly(_))
                    && toolbar_square_button(
                        ui,
                        egui_icons::icons::ICON_SYNC_PROBLEM.codepoint,
                        "Database unavailable",
                        false,
                    )
                    .on_hover_text(format!("{status_text}\nClick to try again."))
                    .clicked()
                {
                    action = Some(DiagramAction::SyncWithDatabase);
                }

                let import_btn = toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_UPLOAD.codepoint,
                    "Import",
                    false,
                )
                .on_hover_text("Import diagram");
                egui::Popup::menu(&import_btn).show(
                    |ui| {
                        if ui.button("Diagram layout (JSON)…").clicked() {
                            ui.close();
                            action = import_json(state);
                        }
                        if ui.button("Mermaid erDiagram (.mmd / .md)…").clicked() {
                            ui.close();
                            action = import_mermaid(state);
                        }
                    },
                );

                let export_btn = toolbar_square_button(
                    ui,
                    egui_icons::icons::ICON_DOWNLOAD.codepoint,
                    "Export",
                    false,
                )
                .on_hover_text("Export diagram");
                egui::Popup::menu(&export_btn).show(
                    |ui| {
                        if ui.button("Diagram layout (JSON)…").clicked() {
                            ui.close();
                            action = export_json(state);
                        }
                        if ui.button("SVG diagram (.svg)…").clicked() {
                            ui.close();
                            action = export_svg(state);
                        }
                        if ui.button("Copy SVG to clipboard").clicked() {
                            ui.close();
                            let svg = crate::diagram_svg::diagram_to_svg(
                                state,
                                &crate::diagram_svg::SvgOptions::from_state(state),
                            );
                            ui.ctx().copy_text(svg);
                            action = Some(DiagramAction::Info(
                                "SVG copied to clipboard".to_string(),
                            ));
                        }
                        if ui.button("Mermaid erDiagram (.mmd / .md)…").clicked() {
                            ui.close();
                            action = export_mermaid(state);
                        }
                        if ui.button("Copy Mermaid to clipboard").clicked() {
                            ui.close();
                            let text = crate::diagram_mermaid::ErModel::from_diagram(state)
                                .to_mermaid(Default::default());
                            ui.ctx().copy_text(text);
                            action = Some(DiagramAction::Info(
                                "Mermaid copied to clipboard".to_string(),
                            ));
                        }
                        if ui
                            .add_enabled(
                                !state.flow_cards.is_empty(),
                                egui::Button::new("Business processes (Mermaid .md)…"),
                            )
                            .on_hover_text("One Mermaid flowchart per API card")
                            .on_disabled_hover_text("This diagram has no API cards")
                            .clicked()
                        {
                            ui.close();
                            action = export_flows_mermaid(state);
                        }
                    },
                );
            });
        },
    );

    let actual_content_width = toolbar_res.response.rect.width() + TOOLBAR_PADDING * 2.0;
    if (actual_content_width - measured_width).abs() > 1.0 {
        ui.data_mut(|d| d.insert_temp(toolbar_id, actual_content_width));
    }

    // Render Search Box
    if state.show_search {
        // Sorotan objek yang baru dipilih dari list hasil, di bawah kartu search.
        crate::diagram_search::draw_highlight(ui, state, rect, now);

        // Two-row card layout: search field + close button on row 1, filter checkboxes on row 2
        let search_rect = egui::Rect::from_min_size(
            rect.min + egui::vec2(20.0, 20.0),
            egui::vec2(crate::diagram_search::PANEL_WIDTH, 70.0),
        );

        let card_fill = ui.visuals().window_fill;
        let card_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        ui.painter().rect_filled(search_rect, 6.0, card_fill);
        ui.painter()
            .rect_stroke(search_rect, 6.0, card_stroke, egui::StrokeKind::Middle);

        ui.scope_builder(
            egui::UiBuilder::new().max_rect(search_rect.shrink(6.0)),
            |ui| {
                ui.vertical(|ui| {
                    let mut search_changed = false;

                    // Baris 1: Field pencarian + tombol tutup
                    ui.horizontal(|ui| {
                        let search_id = ui.id().with("diagram_search_field");
                        let response = crate::window_egui::style::render_search_field_with_id(
                            ui,
                            search_id,
                            &mut state.search_query,
                            "Search diagram…",
                            240.0,
                        );

                        // Auto-focus bila kosong (baru dibuka / dibersihkan), tapi jangan
                        // merebut fokus kembali saat masih ada ketikan yang belum di-Enter.
                        if state.search_query.is_empty()
                            && !response.has_focus()
                            && !crate::window_egui::style::search_draft_pending(
                                ui.ctx(),
                                search_id,
                                &state.search_query,
                            )
                        {
                            response.request_focus();
                        }

                        if response.changed() {
                            search_changed = true;
                        }

                        if ui.button("X").clicked() {
                            state.show_search = false;
                            state.search_query.clear();
                        }
                    });

                    ui.add_space(2.0);

                    // Baris 2: Checkbox filter (Table, Column, Group)
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let cb_tbl = ui
                            .checkbox(&mut state.search_tables, "Table")
                            .on_hover_text("Search table names");
                        let cb_col = ui
                            .checkbox(&mut state.search_columns, "Column")
                            .on_hover_text("Search column names");
                        let cb_grp = ui
                            .checkbox(&mut state.search_groups, "Group")
                            .on_hover_text("Search group container names");

                        if cb_tbl.changed() || cb_col.changed() || cb_grp.changed() {
                            search_changed = true;
                        }
                    });

                    if search_changed {
                        let query = crate::search_match::SearchQuery::new(&state.search_query);
                        if !query.is_empty() {
                            // Hitung skor terbaik untuk node tabel / kolom
                            let mut best_node: Option<(f32, egui::Pos2)> = None;
                            if state.search_tables || state.search_columns {
                                for node in &state.nodes {
                                    let score = match (state.search_tables, state.search_columns) {
                                        (true, true) => query.best_score(
                                            std::iter::once(node.title.as_str())
                                                .chain(node.columns.iter().map(String::as_str)),
                                        ),
                                        (true, false) => query.score(&node.title),
                                        (false, true) => query
                                            .best_score(node.columns.iter().map(String::as_str)),
                                        (false, false) => None,
                                    };
                                    if let Some(score) = score
                                        && best_node
                                            .is_none_or(|(best_score, _)| score > best_score)
                                    {
                                        let node_center = node.pos + node.size / 2.0;
                                        best_node = Some((score, node_center));
                                    }
                                }
                            }

                            // Hitung skor terbaik untuk group container
                            let mut best_group: Option<(f32, egui::Pos2)> = None;
                            if state.search_groups {
                                for group in &state.groups {
                                    if let Some(score) = query.score(&group.title) {
                                        if best_group
                                            .is_none_or(|(best_score, _)| score > best_score)
                                        {
                                            let group_nodes: Vec<&DiagramNode> = state
                                                .nodes
                                                .iter()
                                                .filter(|n| n.is_in_group(&group.id))
                                                .collect();

                                            let group_center = if !group_nodes.is_empty() {
                                                let mut min_pos = group_nodes[0].pos;
                                                let mut max_pos =
                                                    group_nodes[0].pos + group_nodes[0].size;
                                                for n in &group_nodes {
                                                    min_pos = min_pos.min(n.pos);
                                                    max_pos = max_pos.max(n.pos + n.size);
                                                }
                                                min_pos + (max_pos - min_pos) / 2.0
                                            } else if let Some(pos) = group.manual_pos {
                                                pos + egui::vec2(200.0, 150.0)
                                            } else {
                                                egui::Pos2::ZERO
                                            };

                                            best_group = Some((score, group_center));
                                        }
                                    }
                                }
                            }

                            // Pilih kecocokan dengan skor tertinggi antara node atau group
                            let best_match = match (best_node, best_group) {
                                (Some(n), Some(g)) => {
                                    if g.0 > n.0 {
                                        Some(g.1)
                                    } else {
                                        Some(n.1)
                                    }
                                }
                                (Some(n), None) => Some(n.1),
                                (None, Some(g)) => Some(g.1),
                                (None, None) => None,
                            };

                            let target_pan = best_match.map(|center| {
                                let view_center = rect.size() / 2.0;
                                view_center - center.to_vec2() * state.zoom
                            });

                            if let Some(pan) = target_pan {
                                state.pan = pan;
                                state.is_centered = true; // Ensure we don't auto-center back
                            }
                        }
                    }
                });
            },
        );

        // List hasil (Table / Column / Group) di bawah kartu search.
        if state.show_search {
            crate::diagram_search::render_results_panel(ui, state, rect, search_rect, now);
        }

        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            state.show_search = false;
        }
    }

    // Render "Add Group" Popup
    if let Some(pos) = state.add_group_popup {
        let mut close = false;
        let window_pos = to_screen(pos);

        crate::window_egui::style::render_modal_backdrop(
            ui.ctx(),
            "add_group_popup_backdrop",
            state.add_group_popup.is_some(),
        );

        egui::Window::new("New Group")
            .title_bar(false)
            .frame(crate::window_egui::style::modal_window_frame(ui.ctx()))
            .collapsible(false)
            .resizable(false)
            .fixed_pos(window_pos)
            .default_width(280.0)
            .show(ui.ctx(), |ui| {
                crate::window_egui::style::render_modal_header(ui, "New Group", &mut close);
                ui.add_space(8.0);

                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.label("Enter group name:");
                    ui.add_space(4.0);
                    let text_res = crate::window_egui::style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut state.new_group_buffer),
                        f32::INFINITY,
                        None,
                    );
                    if text_res.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        // Trigger save
                    } else {
                        text_res.request_focus();
                    }
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let save_btn = egui::Button::new(
                            egui::RichText::new("Save")
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(crate::window_egui::style::theme_accent(ui.ctx()));

                        if ui.add(save_btn).clicked()
                            || (ui.input(|i| i.key_pressed(egui::Key::Enter))
                                && !state.new_group_buffer.is_empty())
                        {
                            let timestamp = chrono::Utc::now().to_rfc3339();
                            let digest = md5::compute(timestamp);
                            let group_id = format!("{:x}", digest);
                            let color = egui::Color32::from_rgb(100, 149, 237); // Default Blue

                            let new_group = crate::models::structs::DiagramGroup {
                                id: group_id,
                                title: state.new_group_buffer.clone(),
                                color,
                                manual_pos: Some(pos),
                                repo_url: None,
                            };

                            state.groups.push(new_group);
                            state.add_group_popup = None;
                            state.new_group_buffer.clear();
                        }
                    });
                });
            });

        if close {
            state.add_group_popup = None;
        }
    }

    if let Some(a) = render_relation_suggestions(ui.ctx(), state) {
        action = Some(a);
    }
    if let Some(a) = crate::diagram_repo::render_group_repo_editor(ui.ctx(), state) {
        action = Some(a);
    }
    if let Some(a) = crate::diagram_repo::render_group_table_suggestions(ui.ctx(), state) {
        action = Some(a);
    }
    crate::diagram_flow_gen_view::render_flow_gen_window(ui.ctx(), state);
    if let Some(a) = crate::diagram_repo::render_group_table_picker(ui.ctx(), state) {
        action = Some(a);
    }

    render_relations_panel(ui, state, rect, now);
    // Detail proses menempel di bawah card terpilih, di semua zoom.
    let selected_card_rect = state
        .selected_flow
        .as_deref()
        .filter(|_| flow_frame.rects.len() == state.flow_cards.len())
        .and_then(|id| state.flow_cards.iter().position(|c| c.id == id))
        .map(|i| {
            let r = flow_frame.drawn_rect(state, i);
            egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max))
        });
    crate::diagram_flow_play_view::render_process_panel(ui, state, selected_card_rect, rect, now);
    let endpoint_table_screen = state
        .endpoints_panel
        .as_ref()
        .and_then(|t| state.nodes.iter().find(|n| &n.id == t))
        .map(|n| egui::Rect::from_min_max(to_screen(n.pos), to_screen(n.pos + n.size)));
    if let Some(a) = crate::diagram_endpoints_view::render_endpoints_panel(
        ui,
        state,
        rect,
        endpoint_table_screen,
    ) {
        action = Some(a);
    }
    crate::diagram_notes_view::render_notes_panel(ui, state, rect, now);
    crate::diagram_notes_view::render_note_editor(ui.ctx(), state);
    crate::diagram_relation_editor::render_relation_editor(ui.ctx(), state);

    let key_legend = lod == Lod::Detail && !state.nodes.is_empty();
    if key_legend {
        draw_key_legend(ui, rect);
    }
    // Legenda flow hanya saat ada card di layar.
    let card_on_screen = flow_frame
        .rects
        .iter()
        .enumerate()
        .filter(|&(i, _)| flow_frame.is_shown(i))
        .any(|(_, r)| {
            rect.intersects(egui::Rect::from_min_max(to_screen(r.min), to_screen(r.max)))
        });
    if card_on_screen {
        // Di atas legenda kunci (tinggi 40 + jarak) bila legenda itu tampil.
        let bottom = rect.bottom() - if key_legend { 12.0 + 40.0 + 8.0 } else { 12.0 };
        crate::diagram_flow_view::draw_flow_legend(ui, state, &flow_frame, rect.left(), bottom);
    }

    // Overlay performa (build debug, toggle F3): waktu CPU membangun frame
    // diagram dan jumlah relasi/tabel yang benar-benar digambar.
    if cfg!(debug_assertions) {
        let overlay_key = ui.id().with("diagram_perf_overlay");
        if !typing && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F3)) {
            let on: bool = ui.data(|d| d.get_temp(overlay_key)).unwrap_or(false);
            ui.data_mut(|d| d.insert_temp(overlay_key, !on));
        }
        if ui.data(|d| d.get_temp(overlay_key)).unwrap_or(false) {
            let text = format!(
                "build {:.1} ms · zoom {:.2} ({:?})\nrelations drawn {}/{}\ntables drawn {}/{}",
                frame_start.elapsed().as_secs_f64() * 1000.0,
                state.zoom,
                lod,
                rel_stats.drawn,
                rel_stats.total,
                nodes_drawn,
                state.nodes.len(),
            );
            let galley = ui.painter().layout_no_wrap(
                text,
                egui::FontId::monospace(11.0),
                egui::Color32::from_rgb(120, 255, 160),
            );
            let r = egui::Rect::from_min_size(
                rect.right_top() + egui::vec2(-galley.size().x - 28.0, 12.0),
                galley.size() + egui::vec2(16.0, 10.0),
            );
            ui.painter()
                .rect_filled(r, 4.0, egui::Color32::from_black_alpha(190));
            ui.painter()
                .galley(r.min + egui::vec2(8.0, 5.0), galley, egui::Color32::WHITE);
            ui.ctx().request_repaint();
        }
    }

    // Atur kursor mouse untuk mode Hand Tool
    if is_hand_mode {
        let pointer_down = ui.input(|i| i.pointer.primary_down() || i.pointer.middle_down());
        if response.dragged() || (pointer_down && ui.rect_contains_pointer(rect)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if let Some(hover_pos) = ui.input(|i| i.pointer.hover_pos()) {
            let in_canvas = rect.contains(hover_pos);
            let in_toolbar = toolbar_rect.contains(hover_pos);
            let in_search = state.show_search
                && egui::Rect::from_min_size(
                    rect.min + egui::vec2(20.0, 20.0),
                    egui::vec2(295.0, 70.0),
                )
                .contains(hover_pos);
            let in_focus_chip = focus_chip_rect.is_some_and(|r| r.contains(hover_pos));
            if in_canvas && !in_toolbar && !in_search && !in_focus_chip {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
    }

    action
}

/// Toolbar kecil melayang di tengah atas kanvas selama mode fokus aktif:
/// `[ikon] Focusing  nama_tabel │ [⧉ Open in tab] [×]`. Satu baris, tombol
/// setinggi 26px agar mudah diklik, aksi utama diberi label teks. Pada mode
/// fokus group, yang tampil nama group dan "Open in tab" membuka group itu;
/// pada fokus endpoint (`focus_flow`), card beserta tabel tertautnya.
fn render_focus_chip(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    rect: egui::Rect,
    action: &mut Option<DiagramAction>,
) -> Option<egui::Rect> {
    #[derive(PartialEq)]
    enum Kind {
        Table,
        Group,
        Flow,
    }
    let (fid, kind) = match (&state.focus_table, &state.focus_group, &state.focus_flow) {
        (Some(t), _, _) => (t.clone(), Kind::Table),
        (None, Some(g), _) => (g.clone(), Kind::Group),
        (None, None, Some(f)) => (f.clone(), Kind::Flow),
        (None, None, None) => return None,
    };
    let fid = fid.as_str();
    let flow_title;
    let title = match kind {
        Kind::Group => state
            .groups
            .iter()
            .find(|g| g.id == fid)
            .map_or(fid, |g| g.title.as_str()),
        Kind::Table => state
            .nodes
            .iter()
            .find(|n| n.id == fid)
            .map_or(fid, |n| n.title.as_str()),
        Kind::Flow => {
            flow_title = state.flow_cards.iter().find(|c| c.id == fid).map_or_else(
                || fid.to_string(),
                |c| format!("{} {}", c.trigger.method, c.trigger.target),
            );
            flow_title.as_str()
        }
    };

    let accent = crate::window_egui::style::theme_accent(ui.ctx());
    let strong = ui.visuals().strong_text_color();
    let weak = ui.visuals().weak_text_color();
    let font = egui::FontId::proportional(12.5);
    let icon_font = egui::FontId::proportional(14.0);

    let painter = ui.painter().clone();
    let icon_g = painter.layout_no_wrap(
        egui_icons::icons::ICON_FILTER_CENTER_FOCUS
            .codepoint
            .to_string(),
        icon_font.clone(),
        accent,
    );
    let label_g = painter.layout_no_wrap("Focusing".to_owned(), font.clone(), weak);
    let title_g = painter.layout_no_wrap(title.to_owned(), font.clone(), strong);
    let open_label = "Open in tab";
    let open_text_w = painter
        .layout_no_wrap(open_label.to_owned(), font.clone(), strong)
        .size()
        .x;

    // Metrik tata letak
    const H: f32 = 36.0;
    const BTN_H: f32 = 26.0;
    const PAD_L: f32 = 12.0;
    const PAD_R: f32 = 5.0;
    const OPEN_ICON_W: f32 = 14.0;
    let open_btn_w = 10.0 + OPEN_ICON_W + 6.0 + open_text_w + 12.0;
    let info_w = icon_g.size().x + 8.0 + label_g.size().x + 6.0 + title_g.size().x;
    let width = PAD_L + info_w + 12.0 + 1.0 + 6.0 + open_btn_w + 2.0 + BTN_H + PAD_R;

    let chip = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 14.0 + H / 2.0),
        egui::vec2(width, H),
    );
    let cy = chip.center().y;
    let radius = H / 2.0;

    // Tangkap klik pada badan chip agar tidak tembus ke background canvas
    let _ = ui.interact(chip, ui.id().with("focus_chip_pill"), egui::Sense::click());

    // Bayangan halus + latar kapsul + garis tepi netral
    painter.add(
        egui::Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: egui::Color32::from_black_alpha(if ui.visuals().dark_mode { 110 } else { 40 }),
        }
        .as_shape(chip, radius),
    );
    painter.rect_filled(chip, radius, ui.visuals().window_fill);
    painter.rect_stroke(
        chip,
        radius,
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );

    // Bagian info: ikon aksen, label redup, nama tabel tegas
    let mut x = chip.left() + PAD_L;
    let icon_w = icon_g.size().x;
    painter.galley(egui::pos2(x, cy - icon_g.size().y / 2.0), icon_g, accent);
    x += icon_w + 8.0;
    let label_w = label_g.size().x;
    painter.galley(egui::pos2(x, cy - label_g.size().y / 2.0), label_g, weak);
    x += label_w + 6.0;
    let title_w = title_g.size().x;
    painter.galley(egui::pos2(x, cy - title_g.size().y / 2.0), title_g, strong);
    x += title_w + 12.0;

    // Pemisah vertikal antara info dan aksi
    painter.line_segment(
        [egui::pos2(x, cy - 9.0), egui::pos2(x, cy + 9.0)],
        egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    x += 1.0 + 6.0;

    // Tombol berlabel "Open in tab"
    let open_rect = egui::Rect::from_min_size(
        egui::pos2(x, cy - BTN_H / 2.0),
        egui::vec2(open_btn_w, BTN_H),
    );
    let open_resp = chip_button(
        ui,
        open_rect,
        "focus_chip_open_tab",
        match kind {
            Kind::Group => "Open all tables in this group in a new diagram tab",
            Kind::Table => "Open this table and its related tables in a new diagram tab",
            Kind::Flow => "Open this endpoint and all its linked tables in a new diagram tab",
        },
    );
    let open_color = chip_icon_color(ui, &open_resp);
    painter.text(
        egui::pos2(open_rect.left() + 10.0 + OPEN_ICON_W / 2.0, cy),
        egui::Align2::CENTER_CENTER,
        egui_icons::icons::ICON_OPEN_IN_NEW.codepoint,
        egui::FontId::proportional(13.0),
        open_color,
    );
    painter.text(
        egui::pos2(open_rect.left() + 10.0 + OPEN_ICON_W + 6.0, cy),
        egui::Align2::LEFT_CENTER,
        open_label,
        font,
        open_color,
    );
    if open_resp.clicked() {
        *action = Some(match kind {
            Kind::Group => DiagramAction::OpenGroupInNewTab(fid.to_string()),
            Kind::Table => DiagramAction::OpenFocusInNewTab(fid.to_string()),
            Kind::Flow => DiagramAction::OpenFlowInNewTab(fid.to_string()),
        });
    }

    // Tombol close bulat, konsentris dengan lengkungan kanan kapsul
    let close_rect = egui::Rect::from_center_size(
        egui::pos2(chip.right() - PAD_R - BTN_H / 2.0, cy),
        egui::vec2(BTN_H, BTN_H),
    );
    let resp = chip_button(ui, close_rect, "focus_chip_close", "Exit focus (Esc)");
    let c = close_rect.center();
    let half = 4.0;
    let stroke = egui::Stroke::new(1.5, chip_icon_color(ui, &resp));
    painter.line_segment(
        [c + egui::vec2(-half, -half), c + egui::vec2(half, half)],
        stroke,
    );
    painter.line_segment(
        [c + egui::vec2(-half, half), c + egui::vec2(half, -half)],
        stroke,
    );
    if resp.clicked() {
        state.focus_table = None;
        state.focus_group = None;
        if kind == Kind::Flow {
            state.focus_flow = None;
            state.flow_play = None;
        }
    }

    Some(chip)
}

/// Tombol di dalam chip fokus: area klik, kursor, tooltip, dan latar kapsul
/// yang hanya muncul saat hover/tekan (gaya tombol toolbar "ghost").
fn chip_button(ui: &mut egui::Ui, rect: egui::Rect, id: &str, tooltip: &str) -> egui::Response {
    let resp = ui
        .interact(rect, ui.id().with(id), egui::Sense::click())
        .on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let alpha = if resp.is_pointer_button_down_on() {
        Some((45, 35))
    } else if resp.hovered() {
        Some((25, 18))
    } else {
        None
    };
    if let Some((dark, light)) = alpha {
        let bg = if ui.visuals().dark_mode {
            egui::Color32::from_white_alpha(dark)
        } else {
            egui::Color32::from_black_alpha(light)
        };
        ui.painter().rect_filled(rect, rect.height() / 2.0, bg);
    }
    resp
}

/// Warna ikon/label tombol chip: tegas saat di-hover/ditekan, redup selainnya.
fn chip_icon_color(ui: &egui::Ui, resp: &egui::Response) -> egui::Color32 {
    if resp.hovered() || resp.is_pointer_button_down_on() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    }
}

/// Panel daftar relasi sebuah tabel: semua FK, relasi virtual, dan relasi
/// link, bisa dicari. Inilah jalan informasi relasi sampai ke user saat
/// garisnya terlalu padat untuk dibaca. Baris divirtualisasi (`show_rows`),
/// jadi ribuan relasi tetap ringan. Klik baris = animasi ke tabel ujungnya.
fn render_relations_panel(ui: &mut egui::Ui, state: &mut DiagramState, rect: egui::Rect, now: f64) {
    let Some(table) = state.relations_panel.clone() else {
        return;
    };
    if !state.nodes.iter().any(|n| n.id == table) {
        state.relations_panel = None;
        return;
    }
    let rows = crate::diagram_lod::relations_of(state, &table, KindFilter::from_state(state));
    let query = state.relations_panel_query.to_lowercase();
    let shown: Vec<&crate::diagram_lod::RelationRow> =
        rows.iter().filter(|r| r.matches(&query)).collect();
    let count_of =
        |k: crate::diagram_lod::RelationKind| rows.iter().filter(|r| r.kind == k).count();
    let summary = {
        use crate::diagram_lod::RelationKind::*;
        let parts: Vec<String> = [ForeignKey, Virtual, Linked]
            .into_iter()
            .map(|k| (count_of(k), k.label()))
            .filter(|(n, _)| *n > 0)
            .map(|(n, l)| format!("{n} {l}"))
            .collect();
        if parts.is_empty() {
            "No relations".to_string()
        } else {
            parts.join(" · ")
        }
    };
    let title = state
        .nodes
        .iter()
        .find(|n| n.id == table)
        .map(|n| n.title.clone())
        .unwrap_or_else(|| table.clone());

    let mut open = true;
    let mut jump: Option<(String, String, String)> = None; // (tabel tujuan, child, parent)
    egui::Window::new(format!("Relations · {title}"))
        .id(ui.id().with("diagram_relations_panel"))
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_pos(rect.right_top() + egui::vec2(-360.0, 16.0))
        .default_size(egui::vec2(340.0, 420.0))
        .constrain_to(rect)
        .show(ui.ctx(), |ui| {
            ui.label(egui::RichText::new(summary).weak());
            ui.add_space(4.0);
            crate::window_egui::style::render_search_field(
                ui,
                &mut state.relations_panel_query,
                "Filter by table or column…",
                ui.available_width(),
            );
            ui.add_space(4.0);
            if shown.is_empty() {
                ui.label(egui::RichText::new("No relations match.").weak());
                return;
            }
            let row_h = 36.0;
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, row_h, shown.len(), |ui, range| {
                    for row in &shown[range] {
                        let (resp, painter) = ui.allocate_painter(
                            egui::vec2(ui.available_width(), row_h),
                            egui::Sense::click(),
                        );
                        let r = resp.rect;
                        if resp.hovered() {
                            painter.rect_filled(r, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
                        }
                        let kind_color = match row.kind {
                            crate::diagram_lod::RelationKind::ForeignKey => {
                                egui::Color32::from_rgb(150, 160, 185)
                            }
                            crate::diagram_lod::RelationKind::Virtual => {
                                egui::Color32::from_rgb(0, 190, 200)
                            }
                            crate::diagram_lod::RelationKind::Linked => {
                                egui::Color32::from_rgb(0, 150, 160)
                            }
                        };
                        // Badge jenis relasi di kiri.
                        let badge = egui::Rect::from_min_size(
                            r.left_top() + egui::vec2(4.0, 9.0),
                            egui::vec2(48.0, 18.0),
                        );
                        painter.rect_filled(badge, 3.0, kind_color.linear_multiply(0.25));
                        painter.text(
                            badge.center(),
                            egui::Align2::CENTER_CENTER,
                            row.kind.label(),
                            egui::FontId::proportional(10.0),
                            kind_color,
                        );
                        let other = row.other(&table);
                        let outgoing = row.child == table && row.parent != table;
                        let headline = if outgoing {
                            format!("references {other}")
                        } else {
                            format!("referenced by {other}")
                        };
                        let text_x = badge.right() + 8.0;
                        painter.text(
                            egui::pos2(text_x, r.top() + 4.0),
                            egui::Align2::LEFT_TOP,
                            headline,
                            egui::FontId::proportional(12.5),
                            ui.visuals().strong_text_color(),
                        );
                        let detail = if row.child_column.is_empty() {
                            format!("{} → {}", row.child, row.parent)
                        } else {
                            format!(
                                "{}.{} → {}.{}",
                                row.child, row.child_column, row.parent, row.parent_column
                            )
                        };
                        painter.text(
                            egui::pos2(text_x, r.top() + 20.0),
                            egui::Align2::LEFT_TOP,
                            detail,
                            egui::FontId::monospace(10.5),
                            ui.visuals().weak_text_color(),
                        );
                        if resp
                            .on_hover_text(format!("Click to jump to {other}"))
                            .clicked()
                        {
                            jump = Some((other.to_string(), row.child.clone(), row.parent.clone()));
                        }
                    }
                });
        });

    if !open {
        state.relations_panel = None;
    }
    if let Some((target, child, parent)) = jump {
        start_view_animation(state, &target, rect.size(), now);
        // Kedua tabel ujung relasi diberi glow emas.
        state.selected_edge = Some((child, parent));
        ui.ctx().request_repaint();
    }
}

/// Kolom (tabel, kolom) di kedua ujung relasi yang sedang dipilih, baik dari
/// garis FK / garis ringkas (`selected_edge`) maupun relasi virtual
/// (`selected_virtual`). Dipakai untuk memberi glow pada kolom terkait.
fn selected_relation_columns(state: &DiagramState) -> HashSet<(String, String)> {
    let mut cols = HashSet::new();
    let add_virtual = |cols: &mut HashSet<(String, String)>, r: &VirtualRelation| {
        cols.insert((r.child.clone(), r.child_column.clone()));
        cols.insert((r.parent.clone(), r.parent_column.clone()));
    };
    if let Some((a, b)) = &state.selected_edge {
        // Garis ringkas tidak berarah, jadi periksa FK kedua arah.
        for node in state.nodes.iter().filter(|n| n.id == *a || n.id == *b) {
            let other = if node.id == *a { b } else { a };
            for fk in node
                .foreign_keys
                .iter()
                .filter(|fk| fk.referenced_table_name == *other)
            {
                cols.insert((node.id.clone(), fk.column_name.clone()));
                cols.insert((other.clone(), fk.referenced_column_name.clone()));
            }
        }
        for r in state
            .virtual_relations
            .iter()
            .filter(|r| (r.child == *a && r.parent == *b) || (r.child == *b && r.parent == *a))
        {
            add_virtual(&mut cols, r);
        }
    }
    if let Some(r) = state
        .selected_virtual
        .and_then(|idx| state.virtual_relations.get(idx))
    {
        add_virtual(&mut cols, r);
    }
    cols
}

/// Mode fokus: tabel yang tidak terkait ditutup lapisan warna latar
/// sehingga tampak transparan (setara opacity DIM_OPACITY).
fn dim_node(ui: &egui::Ui, node_rect: egui::Rect, scale: f32, canvas_bg: egui::Color32) {
    ui.painter().rect_filled(
        node_rect.expand(7.0 * scale),
        12.0 * scale,
        canvas_bg.gamma_multiply(1.0 - DIM_OPACITY),
    );
}

/// Grid latar mengikuti pan & zoom; tiap garis ke-5 lebih tegas.
fn draw_grid(ui: &egui::Ui, rect: egui::Rect, pan: egui::Vec2, scale: f32) {
    let spacing = 40.0 * scale;
    if spacing < 6.0 {
        return;
    }
    let base = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let minor = egui::Stroke::new(1.0, base.linear_multiply(0.25));
    let major = egui::Stroke::new(1.0, base.linear_multiply(0.55));
    let origin = rect.min + pan;
    let painter = ui.painter();

    let first = ((rect.left() - origin.x) / spacing).floor() as i64;
    let last = ((rect.right() - origin.x) / spacing).ceil() as i64;
    for k in first..=last {
        let x = origin.x + k as f32 * spacing;
        let stroke = if k % 5 == 0 { major } else { minor };
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            stroke,
        );
    }
    let first = ((rect.top() - origin.y) / spacing).floor() as i64;
    let last = ((rect.bottom() - origin.y) / spacing).ceil() as i64;
    for k in first..=last {
        let y = origin.y + k as f32 * spacing;
        let stroke = if k % 5 == 0 { major } else { minor };
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            stroke,
        );
    }
}

/// Lebar node (koordinat diagram) mengikuti isinya: nama kolom terpanjang
/// beserta badge kunci dan tipe di kanannya, serta judul header. Tidak pernah
/// lebih sempit dari lebar default lama (180 / 240). `name_adv` dan
/// `small_adv` adalah lebar satu glyph monospace 12 px dan 10 px.
fn node_content_width(node: &DiagramNode, name_adv: f32, small_adv: f32) -> f32 {
    // Cadangan kecil: ukuran font di-quantize ke 0.5 px saat zoom, sehingga
    // teks bisa sedikit lebih lebar dari skala linear.
    const SLACK: f32 = 1.05;
    const PAD: f32 = 8.0;
    let min = if node.column_meta.is_empty() {
        180.0
    } else {
        240.0
    };

    let mut widest: f32 = 0.0;
    for (i, col) in node.columns.iter().enumerate() {
        let info = node
            .column_meta
            .get(i)
            .filter(|m| m.name == *col)
            .or_else(|| node.column_info(col));
        let key = KeyKind::of(info.is_some_and(|c| c.is_pk), node.is_fk_column(col));
        // Tipe dipotong 14 karakter + elipsis saat digambar.
        let type_chars = info.map_or(0, |c| c.type_name.chars().count().min(15));
        let badge_chars = key.badge().chars().count();
        let mut right = 0.0;
        if type_chars > 0 {
            right += type_chars as f32 * small_adv;
        }
        if badge_chars > 0 {
            if type_chars > 0 {
                right += 5.0;
            }
            right += badge_chars as f32 * small_adv;
        }
        let gap = if right > 0.0 { 10.0 } else { 0.0 };
        let w = PAD + col.chars().count() as f32 * name_adv + gap + right + PAD;
        widest = widest.max(w);
    }

    // Judul proporsional: perkiraan lebar rata-rata glyph ≈ 0.6 × ukuran font.
    let title_px = if node.database_name.is_some() {
        12.5
    } else {
        14.0
    };
    let mut header = node.title.chars().count() as f32 * title_px * 0.6;
    if let Some(db) = &node.database_name {
        let label_chars = db.chars().count()
            + node
                .connection_name
                .as_ref()
                .map_or(0, |c| c.chars().count() + 1)
            + 2;
        header = header.max(label_chars as f32 * 9.0 * 0.6);
    }
    let header = header + 2.0 * PAD + if node.detached { 90.0 } else { 0.0 };

    (widest.max(header) * SLACK).max(min).ceil()
}

/// Tinggi header node (koordinat diagram); header lebih tinggi bila ada
/// badge nama database. Harus sama dengan layout di `render_diagram`.
fn node_header_height(node: &DiagramNode) -> f32 {
    if node.database_name.is_some() {
        30.0
    } else {
        24.0
    }
}

/// Titik tengah vertikal baris kolom (koordinat diagram), mengikuti layout
/// node di `render_diagram`: header 24/30, padding 4, tinggi baris 16.
pub(crate) fn column_anchor_y(node: &DiagramNode, column: &str) -> f32 {
    match node.columns.iter().position(|c| c == column) {
        Some(i) => node.pos.y + node_header_height(node) + 4.0 + i as f32 * 16.0 + 8.0,
        None => node.pos.y + node.size.y / 2.0,
    }
}

fn remove_virtual(state: &mut DiagramState, idx: usize) {
    if idx < state.virtual_relations.len() {
        state.virtual_relations.remove(idx);
        state.save_requested = true;
    }
    state.selected_virtual = None;
}

/// Jarak terdekat dari titik `p` ke ruas garis lurus antara `a` dan `b`.
fn dist_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= 1e-4 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let proj = egui::pos2(a.x + t * dx, a.y + t * dy);
    p.distance(proj)
}

/// Kurva relasi dari baris kolom child ke baris kolom parent, keluar dari sisi
/// yang menghadap tabel tujuan. Mengembalikan (start, end, arah x, kurva).
fn relation_curve(
    child: &DiagramNode,
    parent: &DiagramNode,
    rel: &VirtualRelation,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    scale: f32,
) -> (
    egui::Pos2,
    egui::Pos2,
    (f32, f32),
    egui::epaint::CubicBezierShape,
) {
    column_curve(
        child,
        &rel.child_column,
        parent,
        &rel.parent_column,
        to_screen,
        scale,
    )
}

/// Jarak horizontal minimum (koordinat diagram) antara dua tabel supaya relasi
/// ditarik sisi-ke-sisi yang berhadapan. Di bawah ini tabel dianggap bertumpuk
/// atas-bawah dan relasi ditarik lewat sisi yang sama agar tidak membentuk S.
const FACING_SIDES_MIN_GAP: f32 = 40.0;

/// Sisi anchor relasi: x child, x parent, dan arah keluar (-1 kiri, +1 kanan)
/// di masing-masing ujung.
fn relation_sides(child: &DiagramNode, parent: &DiagramNode) -> (f32, f32, f32, f32) {
    let (cl, cr) = (child.pos.x, child.pos.x + child.size.x);
    let (pl, pr) = (parent.pos.x, parent.pos.x + parent.size.x);
    if pl - cr >= FACING_SIDES_MIN_GAP {
        (cr, pl, 1.0, -1.0)
    } else if cl - pr >= FACING_SIDES_MIN_GAP {
        (cl, pr, -1.0, 1.0)
    } else if (cl - pl).abs() <= (cr - pr).abs() {
        // Bertumpuk: sisi kiri ke kiri (default), kecuali sisi kanan lebih sejajar.
        (cl, pl, -1.0, -1.0)
    } else {
        (cr, pr, 1.0, 1.0)
    }
}

/// Kurva dari baris `child_column` di tabel child ke baris `parent_column` di
/// tabel parent. Dipakai relasi virtual, FK per kolom, dan animasi aliran.
/// Mengembalikan (start, end, (arah keluar start, arah keluar end), kurva).
fn column_curve(
    child: &DiagramNode,
    child_column: &str,
    parent: &DiagramNode,
    parent_column: &str,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    scale: f32,
) -> (
    egui::Pos2,
    egui::Pos2,
    (f32, f32),
    egui::epaint::CubicBezierShape,
) {
    let (cx, px, start_dir, end_dir) = relation_sides(child, parent);
    let start = to_screen(egui::pos2(cx, column_anchor_y(child, child_column)));
    let end = to_screen(egui::pos2(px, column_anchor_y(parent, parent_column)));
    let (c1, c2) = if start_dir == end_dir {
        // Sisi sama: kedua titik kontrol di luar tepi terluar, lengkungnya
        // melebar sesuai jarak vertikal supaya tidak menempel ke tabel.
        let bend = ((end.y - start.y).abs() * 0.15).clamp(30.0 * scale, 80.0 * scale);
        let outer = if start_dir < 0.0 {
            start.x.min(end.x) - bend
        } else {
            start.x.max(end.x) + bend
        };
        (egui::pos2(outer, start.y), egui::pos2(outer, end.y))
    } else {
        let bend = (end.x - start.x).abs().max(60.0 * scale) * 0.5;
        (
            start + egui::vec2(bend * start_dir, 0.0),
            end + egui::vec2(bend * end_dir, 0.0),
        )
    };
    let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
        [start, c1, c2, end],
        false,
        egui::Color32::TRANSPARENT,
        egui::Stroke::NONE,
    );
    (start, end, (start_dir, end_dir), bezier)
}

/// Satu relasi kolom untuk animasi aliran: (child, kolom child, parent,
/// kolom parent). Data mengalir dari parent (sumber) ke child.
type FlowLink = (String, String, String, String);

/// Semua relasi kolom yang menyentuh `table_id`: FK database, relasi virtual,
/// dan relasi bawaan link database. Duplikat dibuang.
pub fn flow_relations(state: &DiagramState, table_id: &str) -> Vec<FlowLink> {
    let mut out: Vec<FlowLink> = Vec::new();
    let mut push = |link: FlowLink| {
        if !out.contains(&link) {
            out.push(link);
        }
    };
    for node in &state.nodes {
        for fk in &node.foreign_keys {
            let parent = &fk.referenced_table_name;
            if node.id != table_id && parent != table_id {
                continue;
            }
            if !state.nodes.iter().any(|n| &n.id == parent) {
                continue;
            }
            push((
                node.id.clone(),
                fk.column_name.clone(),
                parent.clone(),
                fk.referenced_column_name.clone(),
            ));
        }
    }
    for rel in state
        .virtual_relations
        .iter()
        .chain(&state.linked_relations)
    {
        if rel.child == table_id || rel.parent == table_id {
            push((
                rel.child.clone(),
                rel.child_column.clone(),
                rel.parent.clone(),
                rel.parent_column.clone(),
            ));
        }
    }
    out
}

/// Animasi "data mengalir": kurva tiap relasi kolom tabel terpilih disorot,
/// baris kolom sumber/tujuan diberi warna, dan partikel bergerak dari kolom
/// parent (sumber data) ke kolom child. Mengembalikan `false` bila animasi
/// sudah selesai sehingga `flow_anim` boleh dibersihkan.
fn draw_flow_animation(
    ui: &egui::Ui,
    state: &DiagramState,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    now: f64,
) -> bool {
    let Some(flow) = &state.flow_anim else {
        return false;
    };
    let elapsed = (now - flow.start_time).max(0.0);
    // Animasi berhenti sendiri: tanpa batas, seluruh diagram digambar ulang
    // 30x/detik terus-menerus walau user diam.
    if elapsed > crate::diagram_lod::FLOW_ANIM_SECS {
        return false;
    }
    // Baris kolom tidak digambar di zoom kecil; animasi ditahan tapi waktu
    // tetap berjalan.
    if lod_for_zoom(state.zoom) != Lod::Detail {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
        return true;
    }
    let scale = state.zoom;
    let links = flow_relations(state, &flow.table_id);
    if links.is_empty() {
        return false;
    }
    let index = node_index(&state.nodes);
    let clip = ui.clip_rect();
    let painter = ui.painter();
    let source_color = egui::Color32::from_rgb(80, 220, 140);
    let target_color = egui::Color32::from_rgb(80, 200, 255);
    // Denyut halus untuk sorotan baris kolom.
    let pulse = 0.55 + 0.25 * ((elapsed * 4.0).sin() as f32);
    let mut any_visible = false;

    for (child_id, child_col, parent_id, parent_col) in &links {
        let (Some(&ci), Some(&pi)) = (index.get(child_id.as_str()), index.get(parent_id.as_str()))
        else {
            continue;
        };
        let (child, parent) = (&state.nodes[ci], &state.nodes[pi]);
        let (start, end, _, bezier) =
            column_curve(child, child_col, parent, parent_col, to_screen, scale);
        if !curve_visible(&bezier.points, clip, 16.0) {
            continue;
        }
        any_visible = true;

        // Sorot baris kolom: hijau = sumber data, biru = penerima.
        for (node, col, color) in [
            (parent, parent_col, source_color),
            (child, child_col, target_color),
        ] {
            if !node.columns.iter().any(|c| c == col) {
                continue;
            }
            let y = column_anchor_y(node, col);
            let row = egui::Rect::from_min_max(
                to_screen(egui::pos2(node.pos.x, y - 8.0)),
                to_screen(egui::pos2(node.pos.x + node.size.x, y + 8.0)),
            );
            painter.rect_filled(row, 0.0, color.linear_multiply(0.18 * pulse));
            painter.rect_stroke(
                row,
                0.0,
                egui::Stroke::new(1.0 * scale.max(0.6), color.linear_multiply(pulse)),
                egui::StrokeKind::Inside,
            );
        }

        painter.add(egui::Shape::line(
            sample_curve(&bezier),
            egui::Stroke::new(2.0 * scale.max(0.6), target_color.linear_multiply(0.45)),
        ));
        painter.circle_filled(end, 3.5 * scale, source_color);
        painter.circle_filled(start, 3.0 * scale, target_color);

        // Partikel bergerak parent -> child (t kurva 1 -> 0) dengan ekor pendek.
        for k in 0..FLOW_PARTICLES {
            let phase = (elapsed * FLOW_SPEED + k as f64 / FLOW_PARTICLES as f64).fract() as f32;
            for tail in 0..4 {
                let t = phase - tail as f32 * 0.025;
                if t < 0.0 {
                    break;
                }
                let p = bezier.sample(1.0 - t);
                let fade_k = 1.0 - tail as f32 * 0.25;
                let radius = (3.2 - tail as f32 * 0.6) * scale.max(0.6);
                painter.circle_filled(p, radius, egui::Color32::WHITE.linear_multiply(fade_k));
                if tail == 0 {
                    painter.circle_filled(p, radius * 2.2, target_color.linear_multiply(0.25));
                }
            }
        }
    }
    // Repaint cepat hanya bila ada partikel di layar; selain itu cukup
    // bangun sesekali untuk menghentikan animasi tepat waktu.
    let next = if any_visible { 33 } else { 250 };
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(next));
    true
}

/// Relasi bawaan diagram sumber link database: garis putus-putus tipis,
/// read-only (tanpa seleksi/hapus; diubah dari diagram sumbernya).
/// Gambar kontainer database: satu untuk tabel host (bila ada link) dan satu
/// per link. Header kontainer bisa digeser; kontainer link punya tombol buka
/// sumber / refresh / unlink, dan tampil sebagai placeholder bila gagal dimuat.
fn draw_link_containers(
    ui: &mut egui::Ui,
    state: &mut DiagramState,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    is_hand_mode: bool,
) -> Option<DiagramAction> {
    use crate::diagram_links as links;
    use crate::models::structs::LinkStatus;
    if state.linked_databases.is_empty() {
        return None;
    }
    let scale = state.zoom;
    let mut action = None;
    let mut drag: Option<(Option<String>, egui::Vec2)> = None;
    let mut drag_stopped = false;
    let mut unlink_request: Option<String> = None;

    // (link_id, rect koordinat diagram, judul, warna, status); `None` = host.
    let mut boxes: Vec<(
        Option<String>,
        egui::Rect,
        String,
        egui::Color32,
        LinkStatus,
    )> = Vec::new();
    if let Some(r) = links::host_rect(state) {
        let title = state
            .nodes
            .iter()
            .find(|n| !links::is_linked_id(&n.id))
            .and_then(|n| n.database_name.clone())
            .map(|db| format!("{db} (this diagram)"))
            .unwrap_or_else(|| "This diagram".to_string());
        boxes.push((
            None,
            r,
            title,
            egui::Color32::from_gray(150),
            LinkStatus::Loaded,
        ));
    }
    for l in &state.linked_databases {
        let title = if l.connection_name.is_empty() {
            l.database_name.clone()
        } else {
            format!("{} / {}", l.connection_name, l.database_name)
        };
        boxes.push((
            Some(l.link_id.clone()),
            links::container_rect(state, l),
            title,
            l.color,
            l.status.clone(),
        ));
    }

    let btn_size = egui::vec2(24.0, 22.0);
    for (link_id, world, title, color, status) in boxes {
        let rect = egui::Rect::from_min_max(to_screen(world.min), to_screen(world.max));
        if !ui.clip_rect().intersects(rect) {
            continue;
        }
        ui.painter()
            .rect_filled(rect, 10.0 * scale, color.linear_multiply(0.05));
        ui.painter().rect_stroke(
            rect,
            10.0 * scale,
            egui::Stroke::new(1.5 * scale.max(0.6), color.linear_multiply(0.7)),
            egui::StrokeKind::Middle,
        );
        let header = egui::Rect::from_min_size(
            rect.min,
            egui::vec2(rect.width(), links::CONTAINER_HEADER * scale),
        );
        ui.painter()
            .rect_filled(header, 10.0 * scale, color.linear_multiply(0.35));
        ui.painter().text(
            egui::pos2(header.left() + 12.0 * scale, header.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{}  {}", egui_icons::icons::ICON_STORAGE.codepoint, title),
            egui::FontId::proportional(14.0 * scale),
            egui::Color32::WHITE,
        );

        let key = link_id.clone().unwrap_or_else(|| "host".to_string());
        let mut buttons_left = header.right();
        if let Some(id) = &link_id
            && !is_hand_mode
        {
            let items = [
                (
                    egui_icons::icons::ICON_LINK_OFF.codepoint,
                    "Unlink database",
                ),
                (
                    egui_icons::icons::ICON_REFRESH.codepoint,
                    "Reload from source diagram",
                ),
                (
                    egui_icons::icons::ICON_OPEN_IN_NEW.codepoint,
                    "Open source diagram",
                ),
            ];
            for (i, (icon, tip)) in items.iter().enumerate() {
                let x = header.right() - 8.0 - (i as f32 + 1.0) * (btn_size.x + 4.0);
                if x < header.left() + 60.0 {
                    break;
                }
                let r = egui::Rect::from_min_size(
                    egui::pos2(x, header.center().y - btn_size.y / 2.0),
                    btn_size,
                );
                buttons_left = r.left();
                if ui
                    .put(r, egui::Button::new(*icon))
                    .on_hover_text(*tip)
                    .clicked()
                {
                    match i {
                        0 => unlink_request = Some(id.clone()),
                        1 => action = Some(DiagramAction::RefreshLinks(Some(id.clone()))),
                        _ => action = Some(DiagramAction::OpenLinkedDiagram(id.clone())),
                    }
                }
            }
        }

        if !is_hand_mode {
            let drag_rect = egui::Rect::from_min_max(
                header.min,
                egui::pos2(buttons_left.max(header.left()), header.max.y),
            );
            let response = ui
                .interact(
                    drag_rect,
                    ui.id().with("db_container").with(&key),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_text(if link_id.is_some() {
                    "Linked database. Drag to move the container.\nTables inside follow the source diagram."
                } else {
                    "Tables of this diagram's database. Drag to move them together."
                });
            if response.dragged() {
                drag = Some((link_id.clone(), response.drag_delta() / scale));
            }
            if response.drag_stopped() {
                drag_stopped = true;
            }
            if let Some(id) = &link_id {
                response.context_menu(|ui| {
                    if ui.button("Open source diagram").clicked() {
                        ui.close();
                        action = Some(DiagramAction::OpenLinkedDiagram(id.clone()));
                    }
                    if ui.button("Reload from source").clicked() {
                        ui.close();
                        action = Some(DiagramAction::RefreshLinks(Some(id.clone())));
                    }
                    if ui.button("Relink to another connection…").clicked() {
                        ui.close();
                        action = Some(DiagramAction::RelinkDatabase(id.clone()));
                    }
                    ui.separator();
                    if ui.button("Unlink database").clicked() {
                        ui.close();
                        unlink_request = Some(id.clone());
                    }
                });
            }
        }

        // Placeholder: link belum/gagal dimuat, atau database tanpa tabel.
        let empty = link_id.as_deref().is_some_and(|id| {
            !state
                .nodes
                .iter()
                .any(|n| links::link_id_of(&n.id) == Some(id))
        });
        if let Some(id) = &link_id
            && empty
        {
            let (msg, warn) = match &status {
                LinkStatus::Pending => ("Loading…".to_string(), false),
                LinkStatus::Failed(e) => (format!("Could not load: {e}"), true),
                LinkStatus::Loaded => ("No tables in this database.".to_string(), false),
            };
            let body_center = egui::pos2(rect.center().x, (header.bottom() + rect.bottom()) / 2.0);
            ui.painter().text(
                body_center - egui::vec2(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                msg,
                egui::FontId::proportional(12.0),
                if warn {
                    ui.visuals().warn_fg_color
                } else {
                    ui.visuals().weak_text_color()
                },
            );
            if warn && !is_hand_mode {
                let r = egui::Rect::from_center_size(
                    body_center + egui::vec2(0.0, 16.0),
                    egui::vec2(130.0, 22.0),
                );
                if ui.put(r, egui::Button::new("Relink…")).clicked() {
                    action = Some(DiagramAction::RelinkDatabase(id.clone()));
                }
            }
        }
    }

    if let Some((link_id, delta)) = drag {
        match link_id {
            Some(id) => links::move_link(state, &id, delta),
            None => links::move_host(state, delta),
        }
    }
    if drag_stopped {
        state.save_requested = true;
    }
    if let Some(id) = unlink_request {
        let name = state
            .linked_databases
            .iter()
            .find(|l| l.link_id == id)
            .map(|l| l.database_name.clone())
            .unwrap_or_default();
        links::unlink(state, &id);
        state.save_requested = true;
        action = Some(DiagramAction::Info(format!("Database '{name}' unlinked")));
    }
    action
}

/// Gambar relasi virtual sebagai garis putus-putus dari baris kolom child ke
/// baris kolom parent. Mengembalikan `true` bila salah satunya diklik.
/// Hitungan relasi per frame (untuk mode padat dan overlay performa).
#[derive(Default)]
struct RelStats {
    /// Semua relasi yang dipertimbangkan.
    total: usize,
    /// Relasi yang benar-benar digambar (lolos culling).
    drawn: usize,
}

/// Konteks bersama penggambaran relasi dalam satu frame.
struct RelCtx<'a> {
    index: &'a HashMap<&'a str, usize>,
    clip: egui::Rect,
    hover: Option<egui::Pos2>,
    /// Garis putus-putus hanya bila relasi terlihat sedikit.
    dashed: bool,
    emphasis: Emphasis<'a>,
}

impl RelCtx<'_> {
    fn nodes<'s>(
        &self,
        state: &'s DiagramState,
        a: &str,
        b: &str,
    ) -> Option<(&'s DiagramNode, &'s DiagramNode)> {
        let (Some(&i), Some(&j)) = (self.index.get(a), self.index.get(b)) else {
            return None;
        };
        Some((&state.nodes[i], &state.nodes[j]))
    }

    /// Pointer dekat polyline `points` (dalam `tol` px).
    fn hovers(&self, points: &[egui::Pos2], tol: f32) -> bool {
        self.hover.is_some_and(|p| {
            egui::Rect::from_points(points)
                .expand(tol + 2.0)
                .contains(p)
                && points
                    .windows(2)
                    .any(|w| dist_to_segment(p, w[0], w[1]) < tol)
        })
    }
}

/// Titik sample kurva, jumlahnya mengikuti panjang kurva di layar.
fn sample_curve(bezier: &egui::epaint::CubicBezierShape) -> Vec<egui::Pos2> {
    let [start, .., end] = bezier.points;
    let n = curve_samples(start, end);
    (0..=n)
        .map(|i| bezier.sample(i as f32 / n as f32))
        .collect()
}

/// Garis relasi virtual/linked: putus-putus bila relasi sedikit, solid bila
/// padat (satu shape, bukan satu shape per dash).
fn relation_line(
    shapes: &mut Vec<egui::Shape>,
    points: &[egui::Pos2],
    clip: egui::Rect,
    stroke: egui::Stroke,
    dashed: bool,
    scale: f32,
) {
    // Hanya potongan yang terlihat yang di-tessellate.
    for run in crate::diagram_lod::clip_polyline(points, clip.expand(stroke.width + 2.0)) {
        if dashed {
            shapes.extend(egui::Shape::dashed_line(
                &run,
                stroke,
                6.0 * scale,
                4.0 * scale,
            ));
        } else {
            shapes.push(egui::Shape::line(run, stroke));
        }
    }
}

/// Relasi FK database (tampilan detail): satu kurva per kolom FK, dari baris
/// kolom child ke baris kolom yang dirujuk. Separuh sisi child berwarna FK,
/// separuh sisi parent berwarna PK (atau hijau bila merujuk kolom non-PK),
/// sehingga arah dan jenis relasi terbaca tanpa klik. Mengembalikan edge
/// (pasangan tabel) yang diklik.
fn draw_fk_edges(
    ui: &egui::Ui,
    state: &DiagramState,
    ctx: &RelCtx,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    pointer_down: bool,
    stats: &mut RelStats,
) -> Option<(String, String)> {
    let scale = state.zoom;
    // Beberapa edge untuk pasangan tabel yang sama berbagi daftar FK yang sama.
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut shapes: Vec<egui::Shape> = Vec::new();
    let mut clicked = None;

    for edge in &state.edges {
        stats.total += 1;
        if !seen.insert((edge.source.as_str(), edge.target.as_str())) {
            continue;
        }
        let Some((src, dst)) = ctx.nodes(state, &edge.source, &edge.target) else {
            continue;
        };
        let is_selected = state
            .selected_edge
            .as_ref()
            .is_some_and(|(s, t)| *s == edge.source && *t == edge.target);
        let emph = ctx.emphasis.of(&edge.source, &edge.target);

        // Pasangan kolom (child, parent); kosong bila metadata FK tidak ada,
        // lalu jatuh ke satu kurva antar-tabel.
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for fk in &src.foreign_keys {
            if fk.referenced_table_name == edge.target && fk.table_name == src.id {
                let pair = (fk.column_name.as_str(), fk.referenced_column_name.as_str());
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        let curves: Vec<(Option<(&str, &str)>, [egui::Pos2; 4])> = if pairs.is_empty() {
            // Kolom kosong tidak ditemukan, jadi anchor jatuh ke tengah tabel.
            let (_, _, _, bezier) = column_curve(src, "", dst, "", to_screen, scale);
            vec![(None, bezier.points)]
        } else {
            pairs
                .iter()
                .map(|&(c, p)| {
                    let (_, _, _, bezier) = column_curve(src, c, dst, p, to_screen, scale);
                    (Some((c, p)), bezier.points)
                })
                .collect()
        };

        for (cols, points) in curves {
            if !curve_visible(&points, ctx.clip, 4.0) {
                continue;
            }
            stats.drawn += 1;

            // Sorot bila kolom terpilih adalah salah satu ujung kurva ini.
            let is_highlighted_by_col =
                state
                    .selected_column
                    .as_ref()
                    .is_some_and(|(t, c)| match cols {
                        Some((cc, pc)) => (*t == src.id && c == cc) || (*t == dst.id && c == pc),
                        None => *t == src.id || *t == dst.id,
                    });
            let is_active = is_selected || is_highlighted_by_col;

            // Warna ujung child: FK, atau PK+FK bila kolomnya juga PK.
            // Ujung parent: PK, atau hijau bila merujuk kolom non-PK.
            let (child_color, parent_color) = match cols {
                Some((cc, pc)) => {
                    let child_pk = src.column_info(cc).is_some_and(|m| m.is_pk);
                    // Tanpa metadata kolom, anggap merujuk PK (kasus umum).
                    let parent_pk = dst.column_info(pc).is_none_or(|m| m.is_pk);
                    (
                        if child_pk { PK_FK_COLOR } else { FK_COLOR },
                        if parent_pk { PK_COLOR } else { REF_COLOR },
                    )
                }
                None => (FK_COLOR, PK_COLOR),
            };
            let (child_color, parent_color, width) = if is_active {
                let gold = egui::Color32::from_rgb(255, 215, 0);
                (gold, gold, 3.0)
            } else if emph.highlight {
                (child_color, parent_color, 2.2)
            } else {
                (
                    child_color.linear_multiply(0.75),
                    parent_color.linear_multiply(0.75),
                    1.3,
                )
            };
            let child_color = fade(child_color, emph.alpha);
            let parent_color = fade(parent_color, emph.alpha);
            let w = width * scale.max(0.5);

            let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
                points,
                false,
                egui::Color32::TRANSPARENT,
                egui::Stroke::NONE,
            );
            let sampled = sample_curve(&bezier);

            // Hit test hanya bila pointer dekat bounding box kurva. Relasi redup
            // (mode fokus) tidak bisa di-hover/klik.
            let is_hovered = emph.interactive
                && ctx.hover.is_some_and(|p| {
                    egui::Rect::from_points(&points).expand(20.0).contains(p)
                        && (0..=30).any(|i| bezier.sample(i as f32 / 30.0).distance(p) < 20.0)
                });
            if is_hovered {
                if pointer_down {
                    clicked = Some((edge.source.clone(), edge.target.clone()));
                }
                if !is_selected {
                    relation_line(
                        &mut shapes,
                        &sampled,
                        ctx.clip,
                        egui::Stroke::new(w + 2.0 * scale, egui::Color32::from_gray(180)),
                        false,
                        scale,
                    );
                }
            }

            // Dua warna: titik tengah dipakai kedua potongan supaya garis menyambung.
            let mid = sampled.len() / 2;
            relation_line(
                &mut shapes,
                &sampled[..=mid],
                ctx.clip,
                egui::Stroke::new(w, child_color),
                false,
                scale,
            );
            relation_line(
                &mut shapes,
                &sampled[mid..],
                ctx.clip,
                egui::Stroke::new(w, parent_color),
                false,
                scale,
            );
            // Ujung child: lingkaran kosong (sisi "banyak"); ujung parent: titik penuh.
            shapes.push(egui::Shape::circle_stroke(
                points[0],
                2.8 * scale,
                egui::Stroke::new(1.2 * scale.max(0.5), child_color),
            ));
            shapes.push(egui::Shape::circle_filled(
                points[3],
                3.2 * scale,
                parent_color,
            ));
        }
    }
    ui.painter().extend(shapes);
    clicked
}

/// Relasi bawaan diagram sumber link database (read-only, putus-putus tipis).
fn draw_linked_relations(
    ui: &egui::Ui,
    state: &DiagramState,
    ctx: &RelCtx,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    stats: &mut RelStats,
) {
    let scale = state.zoom;
    let base = egui::Color32::from_rgb(0, 190, 200).linear_multiply(0.5);
    let mut shapes: Vec<egui::Shape> = Vec::new();
    for rel in &state.linked_relations {
        stats.total += 1;
        let Some((child, parent)) = ctx.nodes(state, &rel.child, &rel.parent) else {
            continue;
        };
        let (_, end, _, bezier) = relation_curve(child, parent, rel, to_screen, scale);
        if !curve_visible(&bezier.points, ctx.clip, 4.0) {
            continue;
        }
        stats.drawn += 1;
        let emph = ctx.emphasis.of(&rel.child, &rel.parent);
        let width = if emph.highlight { 2.2 } else { 1.2 };
        let color = fade(base, emph.alpha);
        relation_line(
            &mut shapes,
            &sample_curve(&bezier),
            ctx.clip,
            egui::Stroke::new(width * scale.max(0.5), color),
            ctx.dashed,
            scale,
        );
        shapes.push(egui::Shape::circle_filled(end, 2.5 * scale, color));
    }
    ui.painter().extend(shapes);
}

/// Hasil interaksi relasi virtual; diterapkan setelah penggambaran karena
/// penggambaran hanya meminjam state secara immutable.
enum VirtualOutcome {
    None,
    Select(usize),
    Remove(usize),
    /// Double-click garis: buka modal edit relasi.
    Edit(usize),
}

/// Relasi virtual (putus-putus bila sedikit). Relasi yang di-hover atau
/// terpilih menampilkan label dan dua tombol hapus; double-click garis
/// membuka modal edit.
fn draw_virtual_relations(
    ui: &mut egui::Ui,
    state: &DiagramState,
    ctx: &RelCtx,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    pointer_down: bool,
    pointer_double: bool,
    stats: &mut RelStats,
) -> VirtualOutcome {
    let scale = state.zoom;
    // Klik di atas node milik node, bukan garis di bawahnya.
    let over_node = ctx
        .hover
        .is_some_and(|p| crate::diagram_lod::node_at(&state.nodes, to_screen, scale, p).is_some());

    let mut shapes: Vec<egui::Shape> = Vec::new();
    // Label + tombol hapus relasi yang di-hover/terpilih, digambar setelah
    // semua garis supaya tidak tertutup garis lain.
    let mut overlays: Vec<(
        usize,
        egui::Pos2,
        String,
        egui::Color32,
        egui::Rect,
        egui::Rect,
    )> = Vec::new();
    let mut clicked: Option<usize> = None;
    let mut remove: Option<usize> = None;
    let mut edit: Option<usize> = None;
    for (idx, rel) in state.virtual_relations.iter().enumerate() {
        stats.total += 1;
        let Some((child, parent)) = ctx.nodes(state, &rel.child, &rel.parent) else {
            continue;
        };
        let (start, end, dir, bezier) = relation_curve(child, parent, rel, to_screen, scale);
        let selected = state.selected_virtual == Some(idx);
        if !selected && !curve_visible(&bezier.points, ctx.clip, 16.0) {
            continue;
        }
        stats.drawn += 1;
        let points = sample_curve(&bezier);
        let emph = ctx.emphasis.of(&rel.child, &rel.parent);

        let btn_size = egui::vec2(20.0, 20.0);
        let dist = (end - start).length();
        let offset = 28.0f32.min(dist * 0.35).max(14.0);
        let child_btn_rect =
            egui::Rect::from_center_size(start + egui::vec2(dir.0 * offset, 0.0), btn_size);
        let parent_btn_rect =
            egui::Rect::from_center_size(end + egui::vec2(dir.1 * offset, 0.0), btn_size);

        let is_btn_hover = emph.interactive
            && ctx.hover.is_some_and(|p| {
                child_btn_rect.expand(2.0).contains(p) || parent_btn_rect.expand(2.0).contains(p)
            });
        let is_line_hover = !over_node && emph.interactive && ctx.hovers(&points, 12.0);
        let hovered = is_btn_hover || is_line_hover;

        if hovered && pointer_down {
            clicked = Some(idx);
        }
        // Double-click hanya pada garis, bukan tombol hapus.
        if is_line_hover && !is_btn_hover && pointer_double {
            edit = Some(idx);
        }
        let base = match rel.origin {
            RelationOrigin::Imported => egui::Color32::from_rgb(147, 112, 219),
            RelationOrigin::Inferred | RelationOrigin::Manual => {
                egui::Color32::from_rgb(0, 190, 200)
            }
        };
        let (color, width) = if selected {
            (egui::Color32::from_rgb(255, 215, 0), 2.5)
        } else if hovered || emph.highlight {
            (base, 2.5)
        } else {
            (base.linear_multiply(0.85), 1.5)
        };
        let color = if selected || hovered {
            color
        } else {
            fade(color, emph.alpha)
        };
        relation_line(
            &mut shapes,
            &points,
            ctx.clip,
            egui::Stroke::new(width * scale.max(0.5), color),
            ctx.dashed || selected || hovered,
            scale,
        );
        shapes.push(egui::Shape::circle_filled(end, 3.0 * scale, color));

        if selected || hovered {
            let origin = match rel.origin {
                RelationOrigin::Inferred => "suggested",
                RelationOrigin::Manual => "manual",
                RelationOrigin::Imported => "imported",
            };
            overlays.push((
                idx,
                bezier.sample(0.5),
                if hovered {
                    format!(
                        "{} x {} ({origin}) · double-click to edit",
                        rel.child_column, rel.parent_column
                    )
                } else {
                    format!("{} x {} ({origin})", rel.child_column, rel.parent_column)
                },
                color,
                child_btn_rect,
                parent_btn_rect,
            ));
        }
    }
    ui.painter().extend(shapes);

    for (idx, mid, text, color, child_btn_rect, parent_btn_rect) in overlays {
        ui.painter().text(
            mid - egui::vec2(0.0, 10.0),
            egui::Align2::CENTER_BOTTOM,
            text,
            egui::FontId::proportional(11.0),
            color,
        );

        // Dua tombol tong sampah untuk menghapus relasi: dekat kolom child dan parent
        let make_del_btn = |ui: &egui::Ui| {
            egui::Button::new(
                egui::RichText::new(egui_icons::icons::ICON_DELETE.codepoint)
                    .size(11.0)
                    .color(egui::Color32::from_rgb(240, 80, 80)),
            )
            .fill(ui.visuals().window_fill)
            .stroke(egui::Stroke::new(
                1.0,
                egui::Color32::from_rgb(240, 80, 80).linear_multiply(0.7),
            ))
            .corner_radius(4.0)
        };
        let child_res = ui
            .put(child_btn_rect, make_del_btn(ui))
            .on_hover_text("Remove relation");
        let parent_res = ui
            .put(parent_btn_rect, make_del_btn(ui))
            .on_hover_text("Remove relation");
        if child_res.clicked() || parent_res.clicked() {
            remove = Some(idx);
        }
    }

    match (remove, edit, clicked) {
        (Some(idx), _, _) => VirtualOutcome::Remove(idx),
        (None, Some(idx), _) => VirtualOutcome::Edit(idx),
        (None, None, Some(idx)) => VirtualOutcome::Select(idx),
        _ => VirtualOutcome::None,
    }
}

/// Titik sambung garis ringkas antara dua kotak layar: sisi yang saling
/// menghadap, di tengah tinggi kotak.
fn facing_points(a: egui::Rect, b: egui::Rect) -> [egui::Pos2; 4] {
    let a_left = a.center().x > b.center().x;
    let (start, end) = if a_left {
        (a.left_center(), b.right_center())
    } else {
        (a.right_center(), b.left_center())
    };
    let bend = ((end.x - start.x).abs() * 0.5).max(12.0) * if a_left { -1.0 } else { 1.0 };
    [
        start,
        start + egui::vec2(bend, 0.0),
        end - egui::vec2(bend, 0.0),
        end,
    ]
}

/// Tebal garis ringkas menurut jumlah relasi yang diwakilinya.
fn bundle_width(count: u32) -> f32 {
    (1.0 + (count.max(1) as f32).log2() * 0.9).min(7.0)
}

/// Label kecil berlatar di layar (dipakai garis ringkas yang di-hover).
pub(crate) fn draw_chip_label(ui: &egui::Ui, at: egui::Pos2, text: String, color: egui::Color32) {
    let galley =
        ui.painter()
            .layout_no_wrap(text, egui::FontId::proportional(11.0), egui::Color32::WHITE);
    let r = egui::Rect::from_center_size(at, galley.size() + egui::vec2(10.0, 4.0));
    ui.painter()
        .rect_filled(r, 4.0, egui::Color32::from_black_alpha(200));
    ui.painter().rect_stroke(
        r,
        4.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(r.min + egui::vec2(5.0, 2.0), galley, egui::Color32::WHITE);
}

/// Tampilan ringkas/overview: satu garis per pasangan tabel (tebal menurut
/// jumlah relasi) dan, di overview, satu garis per pasangan group.
/// Mengembalikan pasangan tabel yang diklik.
#[allow(clippy::too_many_arguments)]
fn draw_aggregated_links(
    ui: &egui::Ui,
    state: &DiagramState,
    lod: Lod,
    links: &[TableLink],
    group_bounds: &[(usize, String, egui::Rect, egui::Color32, String)],
    ctx: &RelCtx,
    to_screen: &dyn Fn(egui::Pos2) -> egui::Pos2,
    pointer_down: bool,
    stats: &mut RelStats,
) -> Option<(String, String)> {
    let scale = state.zoom;
    let node_rect = |i: usize| {
        let n = &state.nodes[i];
        egui::Rect::from_min_size(to_screen(n.pos), n.size * scale)
    };
    let mut shapes: Vec<egui::Shape> = Vec::new();
    // Teks digambar setelah semua garis supaya tidak tertutup.
    let mut texts: Vec<(egui::Pos2, egui::Align2, String, egui::Color32)> = Vec::new();
    let mut label: Option<(egui::Pos2, String, egui::Color32)> = None;
    let mut clicked = None;

    let (group_links, table_links, intra) = if lod == Lod::Overview && !state.groups.is_empty() {
        let b = crate::diagram_lod::bundle_by_group(state, links);
        (b.groups, std::borrow::Cow::Owned(b.rest), b.intra)
    } else {
        (Vec::new(), std::borrow::Cow::Borrowed(links), Vec::new())
    };

    // Bundel antar group (overview).
    let group_rect: HashMap<usize, egui::Rect> = group_bounds
        .iter()
        .map(|(i, _, r, _, _)| (*i, *r))
        .collect();
    for gl in &group_links {
        stats.total += 1;
        let (Some(ra), Some(rb)) = (group_rect.get(&gl.a), group_rect.get(&gl.b)) else {
            continue;
        };
        let points = facing_points(*ra, *rb);
        if !curve_visible(&points, ctx.clip, 8.0) {
            continue;
        }
        stats.drawn += 1;
        let color = egui::Color32::from_rgb(170, 180, 200);
        let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
            points,
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::NONE,
        );
        let pts = sample_curve(&bezier);
        let hovered = ctx.hovers(&pts, 8.0);
        let width = bundle_width(gl.count) * if hovered { 1.6 } else { 1.0 };
        let alpha = if hovered { 0.95 } else { 0.55 };
        relation_line(
            &mut shapes,
            &pts,
            ctx.clip,
            egui::Stroke::new(width, color.linear_multiply(alpha)),
            false,
            scale,
        );
        let mid = bezier.sample(0.5);
        if hovered {
            let ga = &state.groups[gl.a].title;
            let gb = &state.groups[gl.b].title;
            label = Some((mid, format!("{ga} ↔ {gb}: {} relations", gl.count), color));
        } else if (points[3] - points[0]).length() > 80.0 {
            texts.push((
                mid,
                egui::Align2::CENTER_CENTER,
                gl.count.to_string(),
                color,
            ));
        }
    }
    // Jumlah relasi internal tiap group, di pojok kanan header group.
    for (gi, count) in intra.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        if let Some(r) = group_rect.get(&gi).filter(|r| ctx.clip.intersects(**r)) {
            texts.push((
                r.right_bottom() - egui::vec2(8.0, 6.0),
                egui::Align2::RIGHT_BOTTOM,
                format!("{count} internal relations"),
                egui::Color32::from_gray(170),
            ));
        }
    }

    // Garis per pasangan tabel.
    for link in table_links.iter() {
        stats.total += 1;
        let (ra, rb) = (node_rect(link.a), node_rect(link.b));
        let points = facing_points(ra, rb);
        if !curve_visible(&points, ctx.clip, 8.0) {
            continue;
        }
        stats.drawn += 1;
        let (ida, idb) = (&state.nodes[link.a].id, &state.nodes[link.b].id);
        let emph = ctx.emphasis.of(ida, idb);
        let selected = state
            .selected_edge
            .as_ref()
            .is_some_and(|(s, t)| (s == ida && t == idb) || (s == idb && t == ida));
        let only_virtual = link.fk == 0;
        let base = if only_virtual {
            egui::Color32::from_rgb(0, 190, 200)
        } else {
            egui::Color32::from_rgb(150, 160, 185)
        };
        let bezier = egui::epaint::CubicBezierShape::from_points_stroke(
            points,
            false,
            egui::Color32::TRANSPARENT,
            egui::Stroke::NONE,
        );
        let pts = sample_curve(&bezier);
        let hovered = emph.interactive && ctx.hovers(&pts, 8.0);
        if hovered && pointer_down {
            clicked = Some((ida.clone(), idb.clone()));
        }
        let (color, alpha, boost) = if selected {
            (egui::Color32::from_rgb(255, 215, 0), 1.0, 1.5)
        } else if hovered || emph.highlight {
            (base, 1.0, 1.5)
        } else {
            (base, emph.alpha * 0.8, 1.0)
        };
        relation_line(
            &mut shapes,
            &pts,
            ctx.clip,
            egui::Stroke::new(
                bundle_width(link.total()) * boost,
                color.linear_multiply(alpha),
            ),
            false,
            scale,
        );
        if hovered || selected {
            let (ta, tb) = (&state.nodes[link.a].title, &state.nodes[link.b].title);
            label = Some((
                bezier.sample(0.5),
                format!("{ta} ↔ {tb}: {}", link.breakdown()),
                color,
            ));
        }
    }
    ui.painter().extend(shapes);
    for (at, align, text, color) in texts {
        ui.painter()
            .text(at, align, text, egui::FontId::proportional(10.0), color);
    }
    if let Some((at, text, color)) = label {
        draw_chip_label(ui, at, text, color);
    }
    clicked
}

/// Kolom sumber default saat tabel sumber dipilih: primary key, lalu `id`, lalu kolom pertama.
fn default_source_column(nodes: &[DiagramNode], table: &str) -> Option<String> {
    let node = nodes.iter().find(|n| n.id == table)?;
    node.column_meta
        .iter()
        .find(|c| c.is_pk)
        .map(|c| c.name.clone())
        .or_else(|| {
            node.columns
                .iter()
                .find(|c| c.eq_ignore_ascii_case("id"))
                .cloned()
        })
        .or_else(|| node.columns.first().cloned())
}

/// Input mode Source → Destination dalam dua kartu berdampingan: kiri SOURCE
/// (tabel + kolom, sisi parent), kanan DESTINATION (tabel opsional + pencarian
/// nama kolom mirip, sisi child). Mengembalikan `true` bila input berubah
/// sehingga saran perlu dihitung ulang.
fn render_relation_pair_picker(ui: &mut egui::Ui, state: &mut DiagramState, show_db: bool) -> bool {
    use crate::window_egui::searchable_picker::{PickerConfig, searchable_picker};

    let mut changed = false;
    let is_touch = ui.spacing().interact_size.y >= 30.0;
    let gap = 32.0;
    let card_margin = 14.0 * 2.0 + 2.0;
    let card_w = ((ui.available_width() - gap) / 2.0).floor();
    let field_w = (card_w - card_margin).max(120.0);

    // (node id, label); label "db.tabel" bila diagram berisi lebih dari satu database.
    let mut tables: Vec<(String, String)> = state
        .nodes
        .iter()
        .map(|n| {
            let name = crate::diagram_links::local_name(&n.id);
            let db = crate::diagram_relations::table_database_name(
                &state.nodes,
                &state.linked_databases,
                &n.id,
            );
            let label = match db {
                Some(db) if show_db => format!("{db}.{name}"),
                _ => name.to_string(),
            };
            (n.id.clone(), label)
        })
        .collect();
    tables.sort_by_key(|(_, l)| l.to_lowercase());
    let labels: Vec<String> = tables.iter().map(|(_, l)| l.clone()).collect();
    let position_of = |id: &str| tables.iter().position(|(t, _)| t == id);
    let label_of = |id: &str| position_of(id).map(|i| tables[i].1.clone());

    let picker = |id_salt: &'static str,
                  icon: &'static str,
                  title: &'static str,
                  search_hint: &'static str,
                  tooltip: &'static str| PickerConfig {
        id_salt,
        icon,
        title,
        search_hint,
        tooltip,
        min_width: field_w,
        max_width: field_w,
        is_touch,
    };
    // Judul kartu + keterangan singkat di bawahnya.
    let card_header = |ui: &mut egui::Ui, title: &str, hint: &str| {
        ui.label(egui::RichText::new(title).small().strong().weak());
        ui.label(egui::RichText::new(hint).small().weak());
        ui.add_space(6.0);
    };
    let field_label = |ui: &mut egui::Ui, text: &str| {
        ui.label(egui::RichText::new(text).small());
    };

    // Tinggi kartu disamakan memakai tinggi tertinggi dari frame sebelumnya.
    let height_id = ui.id().with("rel_pair_card_h");
    let min_h: f32 = ui.ctx().data(|d| d.get_temp(height_id)).unwrap_or(0.0);
    let mut max_h: f32 = 0.0;

    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        // --- SOURCE ---
        let resp = ui.allocate_ui_with_layout(
            egui::vec2(card_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.set_width(field_w);
                    ui.set_min_height(min_h);
                    card_header(ui, "SOURCE", "The referenced column, e.g. users.id");

                    field_label(ui, "Table");
                    let table_text = state
                        .relation_source_table
                        .as_deref()
                        .and_then(label_of)
                        .unwrap_or_else(|| "Select table…".to_string());
                    let table_sel = state.relation_source_table.as_deref().and_then(position_of);
                    let cfg = picker(
                        "rel_pair_src_table",
                        egui_icons::icons::MDI_TABLE.codepoint,
                        "Tables",
                        "Search tables…",
                        "Source table (the referenced side, e.g. users)",
                    );
                    if let Some(i) = searchable_picker(ui, &cfg, &table_text, &labels, table_sel) {
                        let id = tables[i].0.clone();
                        if state.relation_source_table.as_deref() != Some(id.as_str()) {
                            state.relation_source_column = default_source_column(&state.nodes, &id);
                            state.relation_source_table = Some(id);
                            changed = true;
                        }
                    }
                    ui.add_space(6.0);

                    field_label(ui, "Column");
                    let columns: Vec<String> = state
                        .relation_source_table
                        .as_deref()
                        .and_then(|id| state.nodes.iter().find(|n| n.id == id))
                        .map(|n| n.columns.clone())
                        .unwrap_or_default();
                    let col_text = state
                        .relation_source_column
                        .clone()
                        .unwrap_or_else(|| "Select column…".to_string());
                    let col_sel = state
                        .relation_source_column
                        .as_deref()
                        .and_then(|c| columns.iter().position(|x| x == c));
                    let cfg = picker(
                        "rel_pair_src_col",
                        egui_icons::icons::MDI_TABLE_COLUMN.codepoint,
                        "Columns",
                        "Search columns…",
                        "Source column (usually the primary key, e.g. id)",
                    );
                    ui.add_enabled_ui(!columns.is_empty(), |ui| {
                        if let Some(i) = searchable_picker(ui, &cfg, &col_text, &columns, col_sel) {
                            if state.relation_source_column.as_deref() != Some(columns[i].as_str())
                            {
                                state.relation_source_column = Some(columns[i].clone());
                                changed = true;
                            }
                        }
                    });
                })
            },
        );
        max_h = max_h.max(resp.inner.response.rect.height() - card_margin);

        // Panah penghubung di antara kedua kartu.
        ui.allocate_ui_with_layout(
            egui::vec2(gap, resp.response.rect.height()),
            egui::Layout::centered_and_justified(egui::Direction::TopDown),
            |ui| {
                ui.label(
                    egui_icons::icons::ICON_ARROW_FORWARD
                        .rich_text()
                        .color(ui.visuals().weak_text_color()),
                )
                .on_hover_text("Destination columns will reference the source column");
            },
        );

        // --- DESTINATION ---
        let resp = ui.allocate_ui_with_layout(
            egui::vec2(card_w, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                    ui.set_width(field_w);
                    ui.set_min_height(min_h);
                    card_header(
                        ui,
                        "DESTINATION",
                        "Columns that point to the source, e.g. created_by_id",
                    );

                    field_label(ui, "Table");
                    let mut items = Vec::with_capacity(labels.len() + 1);
                    items.push("All tables".to_string());
                    items.extend(labels.iter().cloned());
                    let table_text = state
                        .relation_dest_table
                        .as_deref()
                        .and_then(label_of)
                        .unwrap_or_else(|| "All tables".to_string());
                    let table_sel = match state.relation_dest_table.as_deref() {
                        None => Some(0),
                        Some(id) => position_of(id).map(|i| i + 1),
                    };
                    let cfg = picker(
                        "rel_pair_dest_table",
                        egui_icons::icons::MDI_TABLE.codepoint,
                        "Tables",
                        "Search tables…",
                        "Destination table (the referencing side), or all tables",
                    );
                    if let Some(i) = searchable_picker(ui, &cfg, &table_text, &items, table_sel) {
                        let picked = (i > 0).then(|| tables[i - 1].0.clone());
                        if picked != state.relation_dest_table {
                            state.relation_dest_table = picked;
                            changed = true;
                        }
                    }
                    ui.add_space(6.0);

                    field_label(ui, "Similar column name");
                    let muted = crate::window_egui::style::nav_text_muted(ui.ctx());
                    let resp = crate::window_egui::style::render_text_field(
                        ui,
                        egui::TextEdit::singleline(&mut state.relation_dest_columns).hint_text(
                            egui::RichText::new("created_by, updated_by_id or *_by_id")
                                .color(muted),
                        ),
                        field_w,
                        Some(egui_icons::icons::ICON_SEARCH.codepoint),
                    );
                    if resp.changed() {
                        changed = true;
                    }
                    resp.on_hover_text(
                        "Column names to look for, separated by commas. Partial and similar \
                         names match too; use * as a wildcard (e.g. *_by_id). Leave empty for \
                         conventional guesses such as user_id.",
                    );
                })
            },
        );
        max_h = max_h.max(resp.inner.response.rect.height() - card_margin);
    });

    if (max_h - min_h).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(height_id, max_h));
        ui.ctx().request_repaint();
    }
    changed
}

/// Jendela daftar saran relasi; user mencentang lalu menambahkan.
fn render_relation_suggestions(
    ctx: &egui::Context,
    state: &mut DiagramState,
) -> Option<DiagramAction> {
    let mut suggestions = state.relation_suggestions.take()?;
    let mut close = false;
    let mut result = None;

    // Cache saran awal (sebelum user mengetik kolom pencarian baru). Disimpan dalam
    // Arc: `get_temp` meng-clone nilainya setiap frame, dan daftar ini bisa sangat besar.
    type BaseSuggestions = std::sync::Arc<Vec<(crate::diagram_relations::RelationSuggestion, bool)>>;
    let base_id = egui::Id::new("rel_suggest_base");
    let showing_base = state.relation_column_search_query.is_empty() && !state.relation_pair_mode;

    // Hasil pencarian background: selalu jadi cache awal, dan langsung ditampilkan
    // kecuali user sudah beralih ke pencarian lain selagi menunggu.
    if let Some(found) = state.relation_suggest_job.as_ref().and_then(|job| job.poll()) {
        state.relation_suggest_job = None;
        let found: Vec<_> = found.into_iter().map(|s| (s, true)).collect();
        if showing_base {
            suggestions = found.clone();
        }
        ctx.data_mut(|d| d.insert_temp(base_id, BaseSuggestions::new(found)));
    }
    let loading = state.relation_suggest_job.is_some();

    if showing_base && !loading {
        ctx.data_mut(|d| {
            if d.get_temp::<BaseSuggestions>(base_id).is_none() {
                d.insert_temp(base_id, BaseSuggestions::new(suggestions.clone()));
            }
        });
    }

    let window_title = if let Some(t) = &state.relation_suggestions_title {
        format!("Suggested relations for {t}")
    } else {
        "Suggested relations".to_string()
    };

    crate::window_egui::style::render_modal_backdrop(
        ctx,
        "relation_suggestions_backdrop",
        state.relation_suggestions.is_some(),
    );

    // Ukuran jendela tetap (tidak ikut melebar mengikuti isi) dan dijepit ke layar.
    let screen = ctx.content_rect();
    let win_w = (screen.width() - 48.0).clamp(360.0, 760.0);
    let list_h = (screen.height() * 0.45).clamp(160.0, 380.0);

    egui::Window::new(&window_title)
        .title_bar(false)
        .frame(crate::window_egui::style::modal_window_frame(ctx))
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::vec2(win_w, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(win_w);
            crate::window_egui::style::render_modal_header(ui, "Suggested relations", &mut close);
            ui.label(
                egui::RichText::new(match &state.relation_suggestions_title {
                    _ if state.relation_pair_mode => "Link one source column to destination columns with different names (e.g. users.id to created_by_id, updated_by_id). Accepted relations are saved with the diagram as dashed lines.".to_string(),
                    Some(t) => format!(
                        "Target: {t}. Based on column names, similarity and types; accepted relations are saved with the diagram as dashed lines."
                    ),
                    None => "Based on column names, similarity and types; accepted relations are saved with the diagram as dashed lines.".to_string(),
                })
                .weak()
                .small(),
            );
            ui.add_space(10.0);

            // Input pencarian kolom dinamis & filter database
            let mut search_triggered = false;
            let available_dbs = crate::diagram_relations::extract_diagram_databases(
                &state.nodes,
                &state.linked_databases,
            );
            if let Some(ref current) = state.relation_database_filter {
                if !available_dbs.iter().any(|d| d.eq_ignore_ascii_case(current)) {
                    state.relation_database_filter = None;
                }
            }
            let has_dbs = !available_dbs.is_empty();
            let has_multiple_dbs = available_dbs.len() > 1;

            let mut pair_triggered = false;
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(!state.relation_pair_mode, "By column name")
                    .on_hover_text("Find columns with the same or a similar name")
                    .clicked()
                    && state.relation_pair_mode
                {
                    state.relation_pair_mode = false;
                    search_triggered = true;
                }
                if ui
                    .selectable_label(state.relation_pair_mode, "Source to destination")
                    .on_hover_text(
                        "Link one source column (e.g. users.id) to destination columns \
                         with other names (e.g. created_by_id, updated_by_id)",
                    )
                    .clicked()
                    && !state.relation_pair_mode
                {
                    state.relation_pair_mode = true;
                    state.relation_database_filter = None;
                    if state.relation_dest_columns.trim().is_empty() {
                        state.relation_dest_columns =
                            state.relation_column_search_query.trim().to_string();
                    }
                    pair_triggered = true;
                }
            });
            ui.add_space(8.0);

            if state.relation_pair_mode {
                pair_triggered |= render_relation_pair_picker(ui, state, has_multiple_dbs);
            } else {
            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let clear_search_w = if state.relation_column_search_query.is_empty() {
                        0.0
                    } else {
                        24.0
                    };
                    let db_filter_w = if has_dbs {
                        let clear_db_w = if state.relation_database_filter.is_some() {
                            24.0
                        } else {
                            0.0
                        };
                        18.0 + 170.0 + clear_db_w + 24.0
                    } else {
                        0.0
                    };

                    let search_w =
                        (ui.available_width() - db_filter_w - clear_search_w - 8.0).max(100.0);
                    let edit = crate::window_egui::style::render_search_field(
                        ui,
                        &mut state.relation_column_search_query,
                        "Search by column name (e.g. user_id, imei)",
                        search_w,
                    );
                    if edit.changed() {
                        search_triggered = true;
                    }
                    if !state.relation_column_search_query.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    egui_icons::icons::ICON_CLOSE
                                        .rich_text()
                                        .color(ui.visuals().weak_text_color()),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Clear search")
                            .clicked()
                    {
                        state.relation_column_search_query.clear();
                        search_triggered = true;
                    }

                    if has_dbs {
                        ui.separator();
                        let db_icon_color = if state.relation_database_filter.is_some() {
                            crate::window_egui::style::theme_accent(ui.ctx())
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.label(
                            egui_icons::icons::MDI_DATABASE
                                .rich_text()
                                .color(db_icon_color),
                        );
                        let selected_label = match &state.relation_database_filter {
                            Some(db) => db.clone(),
                            None => "All databases".to_string(),
                        };
                        let combo = egui::ComboBox::from_id_salt("rel_suggest_db_filter")
                            .selected_text(
                                egui::RichText::new(&selected_label).color(
                                    if state.relation_database_filter.is_some() {
                                        crate::window_egui::style::theme_accent(ui.ctx())
                                    } else {
                                        ui.visuals().text_color()
                                    },
                                ),
                            )
                            .width(170.0);

                        combo.show_ui(ui, |ui| {
                            let is_all = state.relation_database_filter.is_none();
                            if ui.selectable_label(is_all, "All databases").clicked() {
                                state.relation_database_filter = None;
                            }
                            for db in &available_dbs {
                                let is_sel = state
                                    .relation_database_filter
                                    .as_deref()
                                    .is_some_and(|d| d.eq_ignore_ascii_case(db));
                                if ui.selectable_label(is_sel, db).clicked() {
                                    state.relation_database_filter = Some(db.clone());
                                }
                            }
                        });

                        if state.relation_database_filter.is_some()
                            && ui
                                .add(
                                    egui::Button::new(
                                        egui_icons::icons::ICON_CLOSE
                                            .rich_text()
                                            .color(ui.visuals().weak_text_color()),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Reset to all databases")
                                .clicked()
                        {
                            state.relation_database_filter = None;
                        }
                    }
                });
            });
            }

            if search_triggered {
                let trimmed = state.relation_column_search_query.trim();
                if trimmed.is_empty() {
                    if let Some(base) = ctx.data(|d| d.get_temp::<BaseSuggestions>(base_id)) {
                        suggestions = base.as_ref().clone();
                    } else if !state.relation_pair_mode {
                        suggestions = Vec::new();
                    }
                } else {
                    let found = crate::diagram_relations::suggest_relations_by_column_search(
                        state, trimmed,
                    );
                    suggestions = found.into_iter().map(|s| (s, true)).collect();
                }
            }
            if pair_triggered {
                suggestions = match (&state.relation_source_table, &state.relation_source_column) {
                    (Some(table), Some(column)) => {
                        crate::diagram_relations::suggest_relations_source_to_destination_data(
                            &state.nodes,
                            &state.virtual_relations,
                            table,
                            column,
                            state.relation_dest_table.as_deref(),
                            &state.relation_dest_columns,
                        )
                        .into_iter()
                        .map(|s| (s, true))
                        .collect()
                    }
                    _ => Vec::new(),
                };
            }

            let filter_db = state.relation_database_filter.as_deref();
            let min_score = f32::from(state.relation_min_match) / 100.0;
            let visible_indices: Vec<usize> = suggestions
                .iter()
                .enumerate()
                .filter(|(_, (s, _))| {
                    // Toleransi kecil agar skor 0.8999 tetap lolos ambang 90% (tampil "90%")
                    if s.score + 0.005 < min_score {
                        return false;
                    }
                    if let Some(target) = filter_db {
                        crate::diagram_relations::relation_matches_database(
                            &state.nodes,
                            &state.linked_databases,
                            &s.relation,
                            target,
                        )
                    } else {
                        true
                    }
                })
                .map(|(idx, _)| idx)
                .collect();

            let is_searching = !state.relation_column_search_query.trim().is_empty();
            let total = visible_indices.len();
            let chosen = visible_indices
                .iter()
                .filter(|&&idx| suggestions[idx].1)
                .count();

            ui.add_space(8.0);
            crate::window_egui::style::modal_card_frame(ui.ctx()).show(ui, |ui| {
                ui.set_width(ui.available_width());

                // Toolbar: jumlah terpilih + aksi massal
                ui.horizontal(|ui| {
                    let count_text = if let Some(target) = filter_db {
                        format!("{chosen} of {total} selected (database: {target})")
                    } else {
                        format!("{chosen} of {total} selected")
                    };
                    ui.label(
                        egui::RichText::new(count_text)
                            .weak()
                            .small(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(chosen > 0, egui::Button::new("Select none").small())
                            .clicked()
                        {
                            for &idx in &visible_indices {
                                suggestions[idx].1 = false;
                            }
                        }
                        if ui
                            .add_enabled(chosen < total, egui::Button::new("Select all").small())
                            .clicked()
                        {
                            for &idx in &visible_indices {
                                suggestions[idx].1 = true;
                            }
                        }
                        ui.separator();
                        ui.add(
                            egui::Slider::new(&mut state.relation_min_match, 0..=100)
                                .suffix("%")
                                .step_by(5.0),
                        )
                        .on_hover_text(
                            "Only show suggestions with a match score at or above this value. \
                             Select all / Add apply to the visible rows only.",
                        );
                        ui.label(egui::RichText::new("Min match").weak().small());
                    });
                });
                ui.add_space(4.0);

                let row_h = 26.0;
                let check_w = 24.0;
                let arrow_w = 24.0;
                let score_w = 52.0;
                let spacing = ui.spacing().item_spacing.x;
                let col_w = ((ui.available_width() - check_w - arrow_w - score_w - spacing * 4.0)
                    / 2.0)
                    .max(80.0);

                // Header kolom
                let header = |ui: &mut egui::Ui, text: &str, w: f32, align: egui::Align| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(w, 18.0),
                        egui::Layout::left_to_right(egui::Align::Center).with_main_align(align),
                        |ui| {
                            ui.label(egui::RichText::new(text).small().strong().weak());
                        },
                    );
                };
                ui.horizontal(|ui| {
                    ui.add_space(check_w + spacing);
                    header(ui, "COLUMN", col_w, egui::Align::Min);
                    ui.add_space(arrow_w + spacing);
                    header(ui, "REFERENCES", col_w, egui::Align::Min);
                    header(ui, "MATCH", score_w, egui::Align::Max);
                });
                ui.separator();

                let weak = ui.visuals().weak_text_color();
                let text_color = ui.visuals().text_color();
                let hover_bg = ui.visuals().widgets.hovered.weak_bg_fill;
                // Label "table.column": nama tabel redup, kolom tegas
                let body_font = egui::TextStyle::Body.resolve(ui.style());
                let qualified = |table_id: &str, column: &str| {
                    let mut job = egui::text::LayoutJob::default();
                    let font = body_font.clone();
                    let table_name = crate::diagram_links::local_name(table_id);
                    let table_db = crate::diagram_relations::table_database_name(
                        &state.nodes,
                        &state.linked_databases,
                        table_id,
                    );

                    if has_multiple_dbs {
                        if let Some(db) = table_db {
                            job.append(
                                &format!("{db}."),
                                0.0,
                                egui::TextFormat::simple(
                                    font.clone(),
                                    weak.linear_multiply(0.7),
                                ),
                            );
                        }
                    }
                    job.append(
                        &format!("{table_name}."),
                        0.0,
                        egui::TextFormat::simple(font.clone(), weak),
                    );
                    job.append(column, 0.0, egui::TextFormat::simple(font, text_color));
                    job.wrap = egui::text::TextWrapping::truncate_at_width(col_w);
                    job
                };

                if loading && showing_base {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), list_h),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add_space((list_h - 60.0).max(0.0) / 2.0);
                                ui.spinner();
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Comparing columns across {} tables…",
                                        state.nodes.len()
                                    ))
                                    .weak(),
                                );
                            });
                        },
                    );
                    return;
                }
                if total == 0 {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), list_h),
                        egui::Layout::centered_and_justified(egui::Direction::TopDown),
                        |ui| {
                            let msg = if min_score > 0.0 && !suggestions.is_empty() {
                                format!(
                                    "No suggestions with a match of {}% or higher",
                                    state.relation_min_match
                                )
                            } else if state.relation_pair_mode {
                                if state.relation_source_column.is_none() {
                                    "Pick a source table and column, then the destination column names to link.".to_string()
                                } else if state.relation_dest_columns.trim().is_empty() {
                                    "No conventional matches (e.g. user_id). Type a destination column name, e.g. created_by_id or *_by_id".to_string()
                                } else {
                                    format!(
                                        "No unlinked columns matching \"{}\"",
                                        state.relation_dest_columns.trim()
                                    )
                                }
                            } else if let Some(db) = filter_db {
                                if is_searching {
                                    format!(
                                        "No relations found matching column \"{}\" in database \"{}\"",
                                        state.relation_column_search_query.trim(),
                                        db
                                    )
                                } else {
                                    format!("No relations found involving database \"{}\"", db)
                                }
                            } else if is_searching {
                                format!(
                                    "No relations found matching column \"{}\"",
                                    state.relation_column_search_query.trim()
                                )
                            } else {
                                "No automatic relations found. Search a column name above to scan the diagram.".to_string()
                            };
                            ui.label(egui::RichText::new(msg).italics().weak());
                        },
                    );
                    return;
                }

                // Hanya baris yang terlihat yang dirender; daftar bisa puluhan ribu.
                egui::ScrollArea::vertical()
                    .max_height(list_h)
                    .min_scrolled_height(list_h)
                    .auto_shrink([false, false])
                    .show_rows(ui, row_h, total, |ui, range| {
                        for &idx in &visible_indices[range] {
                            let (s, on) = &mut suggestions[idx];
                            let r = &s.relation;
                            let (row_rect, row_resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), row_h),
                                egui::Sense::click(),
                            );
                            if row_resp.hovered() {
                                ui.painter().rect_filled(row_rect, 4.0, hover_bg);
                            }
                            let mut row_ui = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(row_rect)
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            );
                            row_ui.allocate_ui_with_layout(
                                egui::vec2(check_w, row_h),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.checkbox(on, "");
                                },
                            );
                            row_ui.allocate_ui_with_layout(
                                egui::vec2(col_w, row_h),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(egui::Label::new(qualified(&r.child, &r.child_column)).selectable(false));
                                },
                            );
                            row_ui.allocate_ui_with_layout(
                                egui::vec2(arrow_w, row_h),
                                egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                                |ui| {
                                    ui.label(egui_icons::icons::ICON_ARROW_FORWARD.rich_text().color(weak));
                                },
                            );
                            row_ui.allocate_ui_with_layout(
                                egui::vec2(col_w, row_h),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.add(egui::Label::new(qualified(&r.parent, &r.parent_column)).selectable(false));
                                },
                            );
                            row_ui.allocate_ui_with_layout(
                                egui::vec2(score_w, row_h),
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let pct = s.score * 100.0;
                                    let color = if pct >= 80.0 {
                                        egui::Color32::from_rgb(80, 180, 110)
                                    } else if pct >= 60.0 {
                                        egui::Color32::from_rgb(210, 160, 60)
                                    } else {
                                        weak
                                    };
                                    ui.label(egui::RichText::new(format!("{pct:.0}%")).small().strong().color(color));
                                },
                            );

                            // Klik di mana pun pada baris mengubah centang
                            if row_resp.clicked() {
                                *on = !*on;
                            }
                            let child_name = crate::diagram_links::local_name(&r.child);
                            let parent_name = crate::diagram_links::local_name(&r.parent);
                            let child_db_str = crate::diagram_relations::table_database_name(
                                &state.nodes,
                                &state.linked_databases,
                                &r.child,
                            )
                            .map(|d| format!("[{d}] "))
                            .unwrap_or_default();
                            let parent_db_str = crate::diagram_relations::table_database_name(
                                &state.nodes,
                                &state.linked_databases,
                                &r.parent,
                            )
                            .map(|d| format!("[{d}] "))
                            .unwrap_or_default();
                            row_resp.on_hover_text(format!(
                                "{}{}.{} references {}{}.{}\n{}",
                                child_db_str, child_name, r.child_column,
                                parent_db_str, parent_name, r.parent_column,
                                s.reason
                            ));
                        }
                    });
            });

            // Footer
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let btn = egui::Button::new(
                        egui::RichText::new(format!("Add {chosen} relation(s)"))
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(crate::window_egui::style::theme_accent(ui.ctx()))
                    .min_size(egui::vec2(0.0, 28.0));

                    if ui.add_enabled(chosen > 0, btn).clicked() {
                        let accepted: Vec<_> = visible_indices
                            .iter()
                            .map(|&idx| &suggestions[idx])
                            .filter(|(_, on)| *on)
                            .map(|(s, _)| s.relation.clone())
                            .collect();
                        let added =
                            crate::diagram_relations::add_virtual_relations(state, accepted);
                        state.save_requested = true;
                        result = Some(DiagramAction::Info(format!("Added {added} relation(s)")));
                        close = true;
                    }
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(0.0, 28.0)))
                        .clicked()
                    {
                        close = true;
                    }
                });
            });
        });

    if !close {
        state.relation_suggestions = Some(suggestions);
    } else {
        ctx.data_mut(|d| {
            d.remove::<BaseSuggestions>(base_id);
        });
        if let Some(job) = state.relation_suggest_job.take() {
            job.cancel();
        }
        state.relation_column_search_query.clear();
        state.relation_database_filter = None;
        state.relation_min_match = 0;
        state.relation_suggestions_title = None;
        // Pilihan source/destination dipertahankan agar mudah dipakai ulang.
        state.relation_pair_mode = false;
    }
    result
}

/// Cek apakah ada pasangan tabel yang saling tumpang tindih dalam batas padding.
pub fn check_nodes_overlap(nodes: &[DiagramNode], padding: f32) -> bool {
    let half_pad = padding.max(0.0) / 2.0;
    for (i, node) in nodes.iter().enumerate() {
        let rect_i = egui::Rect::from_min_size(node.pos, node.size).expand(half_pad);
        for other_node in &nodes[(i + 1)..] {
            let rect_j =
                egui::Rect::from_min_size(other_node.pos, other_node.size).expand(half_pad);
            let inter = rect_i.intersect(rect_j);
            if inter.width() > 0.0 && inter.height() > 0.0 {
                return true;
            }
        }
    }
    false
}

/// Cek apakah tabel spesifik saat ini bertabrakan dengan tabel lain dalam diagram.
pub fn check_single_node_collision(nodes: &[DiagramNode], node_id: &str, padding: f32) -> bool {
    let Some(target) = nodes.iter().find(|n| n.id == node_id) else {
        return false;
    };
    let half_pad = padding.max(0.0) / 2.0;
    let target_rect = egui::Rect::from_min_size(target.pos, target.size).expand(half_pad);
    for other in nodes.iter().filter(|n| n.id != node_id) {
        let other_rect = egui::Rect::from_min_size(other.pos, other.size).expand(half_pad);
        let inter = target_rect.intersect(other_rect);
        if inter.width() > 0.0 && inter.height() > 0.0 {
            return true;
        }
    }
    false
}

/// Pisahkan semua tabel yang bertumpukan secara iteratif menggunakan AABB collision resolution.
/// Menjamin tidak ada dua tabel yang tumpang tindih dengan jarak minimal `padding`.
pub fn resolve_node_overlaps(nodes: &mut [DiagramNode], padding: f32) {
    let node_count = nodes.len();
    if node_count < 2 {
        return;
    }

    let half_pad = padding.max(0.0) / 2.0;
    let max_iterations = 40;

    for _ in 0..max_iterations {
        let mut any_collision = false;

        for i in 0..node_count {
            for j in (i + 1)..node_count {
                let rect_i =
                    egui::Rect::from_min_size(nodes[i].pos, nodes[i].size).expand(half_pad);
                let rect_j =
                    egui::Rect::from_min_size(nodes[j].pos, nodes[j].size).expand(half_pad);

                // Tabel milik link database tidak digeser (posisinya milik
                // diagram sumber); tabel host yang menabraknya didorong penuh.
                let pin_i = crate::diagram_links::is_linked_id(&nodes[i].id);
                let pin_j = crate::diagram_links::is_linked_id(&nodes[j].id);
                if pin_i && pin_j {
                    continue;
                }

                let inter = rect_i.intersect(rect_j);
                if inter.width() > 0.0 && inter.height() > 0.0 {
                    any_collision = true;
                    let overlap_w = inter.width();
                    let overlap_h = inter.height();

                    // Dorong pada sumbu irisan terkecil agar pergeseran seminimal mungkin
                    let push = if overlap_w < overlap_h {
                        let dir = if rect_i.center().x <= rect_j.center().x {
                            -1.0
                        } else {
                            1.0
                        };
                        egui::vec2(dir * (overlap_w / 2.0 + 1.0), 0.0)
                    } else {
                        let dir = if rect_i.center().y <= rect_j.center().y {
                            -1.0
                        } else {
                            1.0
                        };
                        egui::vec2(0.0, dir * (overlap_h / 2.0 + 1.0))
                    };

                    match (pin_i, pin_j) {
                        (true, _) => nodes[j].pos -= push * 2.0,
                        (_, true) => nodes[i].pos += push * 2.0,
                        _ => {
                            nodes[i].pos += push;
                            nodes[j].pos -= push;
                        }
                    }
                }
            }
        }

        if !any_collision {
            break;
        }
    }
}

/// Pisahkan tabel yang baru selesai digeser agar tidak tumpang tindih dengan tabel lain.
/// Memprioritaskan posisi tabel lain tetap stabil di tempatnya.
pub fn resolve_dragged_node_overlap(nodes: &mut [DiagramNode], dragged_id: &str, padding: f32) {
    let half_pad = padding.max(0.0) / 2.0;
    let max_single_passes = 25;

    let Some(dragged_idx) = nodes.iter().position(|n| n.id == dragged_id) else {
        return;
    };

    let mut still_colliding = false;
    for _ in 0..max_single_passes {
        let dragged_rect =
            egui::Rect::from_min_size(nodes[dragged_idx].pos, nodes[dragged_idx].size)
                .expand(half_pad);

        // Cari rintangan terdekat yang bertabrakan
        let mut min_push: Option<egui::Vec2> = None;
        let mut min_dist_sq = f32::MAX;

        for (j, other) in nodes.iter().enumerate() {
            if j == dragged_idx {
                continue;
            }
            let other_rect = egui::Rect::from_min_size(other.pos, other.size).expand(half_pad);
            let inter = dragged_rect.intersect(other_rect);
            if inter.width() > 0.0 && inter.height() > 0.0 {
                let overlap_w = inter.width();
                let overlap_h = inter.height();

                // Hitung vektor dorong untuk mengeluarkan dragged_node dari obstacle
                let (dir_x, dist_x) = if dragged_rect.center().x <= other_rect.center().x {
                    (-1.0, overlap_w + 1.0)
                } else {
                    (1.0, overlap_w + 1.0)
                };
                let (dir_y, dist_y) = if dragged_rect.center().y <= other_rect.center().y {
                    (-1.0, overlap_h + 1.0)
                } else {
                    (1.0, overlap_h + 1.0)
                };

                let push = if dist_x < dist_y {
                    egui::vec2(dir_x * dist_x, 0.0)
                } else {
                    egui::vec2(0.0, dir_y * dist_y)
                };

                let dist_sq = push.length_sq();
                if dist_sq < min_dist_sq {
                    min_dist_sq = dist_sq;
                    min_push = Some(push);
                }
            }
        }

        if let Some(push) = min_push {
            nodes[dragged_idx].pos += push;
            still_colliding = true;
        } else {
            still_colliding = false;
            break;
        }
    }

    // Jika ruang sangat sempit dan dragged_node masih terjepit di antara beberapa tabel,
    // jalankan relaksasi global untuk memberi ruang.
    if still_colliding {
        resolve_node_overlaps(nodes, padding);
    }
}

/// Padding kotak group di sekitar anggotanya (world unit), sama dengan saat
/// render: 20 di sisi, plus judul 30 dan offset judul maksimal 16 di atas.
const GROUP_SIDE_PAD: f32 = 20.0;
const GROUP_TOP_PAD: f32 = GROUP_SIDE_PAD + 30.0 + 16.0;

/// Kelompokkan group yang beririsan (berbagi tabel) ke dalam satu cluster.
/// Group dalam cluster yang sama boleh tumpang tindih dan selalu digeser
/// bersama; hasilnya: cluster per group dan cluster per node (None = tanpa group).
fn group_clusters(nodes: &[DiagramNode]) -> (HashMap<String, usize>, Vec<Option<usize>>) {
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }

    let mut index: HashMap<String, usize> = HashMap::new();
    let mut parent: Vec<usize> = Vec::new();
    for n in nodes {
        let mut first: Option<usize> = None;
        for gid in n.group_ids.iter().chain(n.group_id.as_ref()) {
            let next = index.len();
            let g = *index.entry(gid.clone()).or_insert_with(|| {
                parent.push(next);
                next
            });
            match first {
                None => first = Some(g),
                Some(f) => {
                    let (rf, rg) = (find(&mut parent, f), find(&mut parent, g));
                    parent[rg] = rf;
                }
            }
        }
    }

    // Nomori ulang root menjadi 0..jumlah_cluster.
    let mut dense: HashMap<usize, usize> = HashMap::new();
    let mut group_cluster: HashMap<String, usize> = HashMap::new();
    let mut ids: Vec<(&String, usize)> = index.iter().map(|(k, &v)| (k, v)).collect();
    ids.sort();
    for (gid, g) in ids {
        let root = find(&mut parent, g);
        let next = dense.len();
        let c = *dense.entry(root).or_insert(next);
        group_cluster.insert(gid.clone(), c);
    }
    let node_cluster = nodes
        .iter()
        .map(|n| {
            n.group_ids
                .iter()
                .chain(n.group_id.as_ref())
                .next()
                .and_then(|g| group_cluster.get(g).copied())
        })
        .collect();
    (group_cluster, node_cluster)
}

/// Kotak tiap group dalam koordinat diagram: (id group, cluster, rect).
fn group_world_rects(
    nodes: &[DiagramNode],
    group_cluster: &HashMap<String, usize>,
) -> Vec<(String, usize, egui::Rect)> {
    let mut extent: HashMap<&str, egui::Rect> = HashMap::new();
    for n in nodes {
        let r = egui::Rect::from_min_size(n.pos, n.size);
        for gid in n.group_ids.iter().chain(n.group_id.as_ref()) {
            extent
                .entry(gid.as_str())
                .and_modify(|e| *e = e.union(r))
                .or_insert(r);
        }
    }
    let mut rects: Vec<(String, usize, egui::Rect)> = extent
        .into_iter()
        .filter_map(|(gid, r)| {
            let cluster = *group_cluster.get(gid)?;
            let rect = egui::Rect::from_min_max(
                r.min - egui::vec2(GROUP_SIDE_PAD, GROUP_TOP_PAD),
                r.max + egui::vec2(GROUP_SIDE_PAD, GROUP_SIDE_PAD),
            );
            Some((gid.to_string(), cluster, rect))
        })
        .collect();
    // Urutan stabil agar hasil resolusi deterministik.
    rects.sort_by(|a, b| a.0.cmp(&b.0));
    rects
}

/// Cek apakah ada dua group yang tidak beririsan (tidak berbagi tabel) saling
/// tumpang tindih dalam batas padding.
pub fn check_groups_overlap(nodes: &[DiagramNode], padding: f32) -> bool {
    let (group_cluster, _) = group_clusters(nodes);
    let half_pad = padding.max(0.0) / 2.0;
    let rects = group_world_rects(nodes, &group_cluster);
    rects.iter().enumerate().any(|(i, (_, ci, ri))| {
        rects[(i + 1)..].iter().any(|(_, cj, rj)| {
            let inter = ri.expand(half_pad).intersect(rj.expand(half_pad));
            ci != cj && inter.width() > 0.0 && inter.height() > 0.0
        })
    })
}

/// Pisahkan group yang tidak beririsan agar kotaknya tidak tumpang tindih.
/// Seluruh anggota satu cluster (group-group yang berbagi tabel) digeser
/// bersama. Bila `mover` (id tabel atau id group) diisi, hanya cluster milik
/// `mover` yang digeser saat bertabrakan; cluster lain tetap di tempatnya.
pub fn resolve_group_overlaps(nodes: &mut [DiagramNode], padding: f32, mover: Option<&str>) {
    let (group_cluster, node_cluster) = group_clusters(nodes);
    let cluster_count = group_cluster.values().max().map_or(0, |m| m + 1);
    if cluster_count < 2 {
        return;
    }

    // Cluster yang berisi tabel link database tidak digeser.
    let mut pinned = vec![false; cluster_count];
    for (n, c) in nodes.iter().zip(&node_cluster) {
        if let Some(c) = c
            && crate::diagram_links::is_linked_id(&n.id)
        {
            pinned[*c] = true;
        }
    }
    let mover_cluster = mover.and_then(|m| {
        group_cluster.get(m).copied().or_else(|| {
            nodes
                .iter()
                .position(|n| n.id == m)
                .and_then(|i| node_cluster[i])
        })
    });

    let half_pad = padding.max(0.0) / 2.0;
    for _ in 0..40 {
        let rects = group_world_rects(nodes, &group_cluster);
        let mut shift = vec![egui::Vec2::ZERO; cluster_count];
        let mut any_collision = false;

        for (i, (_, ci, ri)) in rects.iter().enumerate() {
            for (_, cj, rj) in &rects[(i + 1)..] {
                let (ci, cj) = (*ci, *cj);
                if ci == cj || (pinned[ci] && pinned[cj]) {
                    continue;
                }
                let (ri, rj) = (ri.expand(half_pad), rj.expand(half_pad));
                let inter = ri.intersect(rj);
                if inter.width() <= 0.0 || inter.height() <= 0.0 {
                    continue;
                }
                any_collision = true;

                // Dorong pada sumbu irisan terkecil; `push` menggeser ci
                // menjauh dari cj sejauh seluruh irisan.
                let push = if inter.width() < inter.height() {
                    let dir = if ri.center().x <= rj.center().x {
                        -1.0
                    } else {
                        1.0
                    };
                    egui::vec2(dir * (inter.width() + 1.0), 0.0)
                } else {
                    let dir = if ri.center().y <= rj.center().y {
                        -1.0
                    } else {
                        1.0
                    };
                    egui::vec2(0.0, dir * (inter.height() + 1.0))
                };

                let i_fixed = pinned[ci] || mover_cluster.is_some_and(|m| m == cj);
                let j_fixed = pinned[cj] || mover_cluster.is_some_and(|m| m == ci);
                match (i_fixed, j_fixed) {
                    (true, false) => shift[cj] -= push,
                    (false, true) => shift[ci] += push,
                    _ => {
                        shift[ci] += push / 2.0;
                        shift[cj] -= push / 2.0;
                    }
                }
            }
        }

        if !any_collision {
            break;
        }
        for (n, c) in nodes.iter_mut().zip(&node_cluster) {
            if let Some(c) = c {
                n.pos += shift[*c];
            }
        }
    }
}

/// Pisahkan tabel dan group yang tumpang tindih sekaligus. Menggeser tabel
/// dapat memperbesar group sehingga keduanya diulang sampai stabil. `mover`
/// (id tabel atau group yang baru digeser) diprioritaskan untuk mengalah.
pub fn resolve_all_overlaps(nodes: &mut [DiagramNode], padding: f32, mover: Option<&str>) {
    for _ in 0..5 {
        resolve_group_overlaps(nodes, padding, mover);
        match mover {
            Some(id) if nodes.iter().any(|n| n.id == id) => {
                resolve_dragged_node_overlap(nodes, id, padding)
            }
            _ => resolve_node_overlaps(nodes, padding),
        }
        if !check_groups_overlap(nodes, padding) {
            break;
        }
    }
}

/// Jarak kosong maksimal antar tabel di dalam satu group (world unit).
pub const GROUP_MAX_GAP: f32 = 40.0;
/// Jarak kosong maksimal antar blok (group atau tabel tanpa group).
pub const BLOCK_MAX_GAP: f32 = 80.0;

/// Hitung pergeseran tiap kotak agar celah kosong di antaranya tidak melebihi
/// `max_gap`. Dipadatkan per sumbu (x lalu y) dengan sapuan: urutan relatif
/// tetap, kotak yang semula tidak bertumpuk tetap tidak bertumpuk, dan kotak
/// paling kiri/atas tetap di tempat.
fn compact_rects(rects: &[egui::Rect], max_gap: f32) -> Vec<egui::Vec2> {
    let max_gap = max_gap.max(0.0);
    let mut offsets = vec![egui::Vec2::ZERO; rects.len()];
    for axis in [0usize, 1] {
        let mut order: Vec<usize> = (0..rects.len()).collect();
        order.sort_by(|&a, &b| rects[a].min[axis].total_cmp(&rects[b].min[axis]));

        let mut frontier: Option<f32> = None;
        let mut shift = 0.0;
        for &i in &order {
            let s = rects[i].min[axis] - shift;
            if let Some(f) = frontier
                && s > f + max_gap
            {
                shift += s - (f + max_gap);
            }
            offsets[i][axis] = -shift;
            let end = rects[i].max[axis] - shift;
            frontier = Some(frontier.map_or(end, |f| f.max(end)));
        }
    }
    offsets
}

/// Cluster yang berisi tabel link database tidak boleh digeser.
fn pinned_clusters(
    nodes: &[DiagramNode],
    node_cluster: &[Option<usize>],
    count: usize,
) -> Vec<bool> {
    let mut pinned = vec![false; count];
    for (n, c) in nodes.iter().zip(node_cluster) {
        if let Some(c) = c
            && crate::diagram_links::is_linked_id(&n.id)
        {
            pinned[*c] = true;
        }
    }
    pinned
}

/// Rapatkan tabel di dalam tiap group agar celah kosong di antaranya tidak
/// melebihi `max_gap`, sehingga group tidak makan tempat. Sudut kiri-atas
/// group tetap di tempat. Cluster yang berisi tabel link database tidak disentuh.
pub fn compact_groups(nodes: &mut [DiagramNode], max_gap: f32) {
    let (group_cluster, node_cluster) = group_clusters(nodes);
    let cluster_count = group_cluster.values().max().map_or(0, |m| m + 1);
    let pinned = pinned_clusters(nodes, &node_cluster, cluster_count);

    for c in (0..cluster_count).filter(|&c| !pinned[c]) {
        let members: Vec<usize> = (0..nodes.len())
            .filter(|&i| node_cluster[i] == Some(c))
            .collect();
        let rects: Vec<egui::Rect> = members
            .iter()
            .map(|&i| egui::Rect::from_min_size(nodes[i].pos, nodes[i].size))
            .collect();
        for (&i, off) in members.iter().zip(compact_rects(&rects, max_gap)) {
            nodes[i].pos += off;
        }
    }
}

/// Rapatkan antar blok: tiap cluster group digeser utuh sebagai satu kotak,
/// tabel tanpa group sebagai kotak sendiri. Tabel link database dan cluster
/// yang memuatnya tidak digeser.
pub fn compact_blocks(nodes: &mut [DiagramNode], max_gap: f32) {
    move_blocks(nodes, |rects| compact_rects(rects, max_gap));
}

/// Jarak antar blok (group atau tabel tanpa group) hasil "Auto Arrange".
pub const ARRANGE_GAP: f32 = 40.0;

/// Hitung pergeseran tiap kotak agar merapat ke pusat gabungannya: tiap
/// kotak digeser per sumbu sampai tertahan kotak lain pada jarak `gap`.
/// Kotak yang semula tidak bertumpuk tetap tidak bertumpuk.
fn pack_rects(rects: &[egui::Rect], gap: f32) -> Vec<egui::Vec2> {
    let mut r = rects.to_vec();
    for _ in 0..40 {
        let Some(center) = r
            .iter()
            .copied()
            .reduce(|a, b| a.union(b))
            .map(|b| b.center())
        else {
            break;
        };
        let mut order: Vec<usize> = (0..r.len()).collect();
        order.sort_by(|&a, &b| {
            r[a].center()
                .distance_sq(center)
                .total_cmp(&r[b].center().distance_sq(center))
        });
        let mut moved = false;
        for &i in &order {
            for axis in [0usize, 1] {
                let other = 1 - axis;
                let d = center[axis] - r[i].center()[axis];
                let mut travel = d.abs();
                for (j, o) in r.iter().enumerate() {
                    // Hanya kotak yang berada di jalur geser (dalam jarak `gap`).
                    if j == i
                        || o.max[other] <= r[i].min[other] - gap
                        || o.min[other] >= r[i].max[other] + gap
                    {
                        continue;
                    }
                    if d > 0.0 && o.center()[axis] > r[i].center()[axis] {
                        travel = travel.min(o.min[axis] - gap - r[i].max[axis]);
                    } else if d < 0.0 && o.center()[axis] < r[i].center()[axis] {
                        travel = travel.min(r[i].min[axis] - gap - o.max[axis]);
                    }
                }
                if travel > 0.5 {
                    let mut off = egui::Vec2::ZERO;
                    off[axis] = travel * d.signum();
                    r[i] = r[i].translate(off);
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    r.iter().zip(rects).map(|(a, b)| a.min - b.min).collect()
}

/// Rapatkan semua blok ke pusat diagram dengan jarak `gap` (lihat
/// [`pack_rects`]). Dipakai "Auto Arrange" agar hasilnya tidak renggang.
pub fn pack_blocks(nodes: &mut [DiagramNode], gap: f32) {
    move_blocks(nodes, |rects| pack_rects(rects, gap));
}

/// Geser blok (cluster group utuh, atau tabel tanpa group) sebesar offset
/// yang dihitung `offsets_of` dari kotak tiap blok.
fn move_blocks(nodes: &mut [DiagramNode], offsets_of: impl Fn(&[egui::Rect]) -> Vec<egui::Vec2>) {
    let (group_cluster, node_cluster) = group_clusters(nodes);
    let cluster_count = group_cluster.values().max().map_or(0, |m| m + 1);
    let pinned = pinned_clusters(nodes, &node_cluster, cluster_count);

    // Kotak cluster = gabungan kotak group-nya (termasuk judul & padding).
    let mut cluster_rect: Vec<Option<egui::Rect>> = vec![None; cluster_count];
    for (_, c, r) in group_world_rects(nodes, &group_cluster) {
        cluster_rect[c] = Some(cluster_rect[c].map_or(r, |e| e.union(r)));
    }

    // Blok: indeks cluster (Some) atau indeks tabel tanpa group (None, i).
    let mut blocks: Vec<(Option<usize>, usize)> = Vec::new();
    let mut rects: Vec<egui::Rect> = Vec::new();
    for (c, r) in cluster_rect.iter().enumerate() {
        if let Some(r) = r
            && !pinned[c]
        {
            blocks.push((Some(c), 0));
            rects.push(*r);
        }
    }
    for (i, n) in nodes.iter().enumerate() {
        if node_cluster[i].is_none() && !crate::diagram_links::is_linked_id(&n.id) {
            blocks.push((None, i));
            rects.push(egui::Rect::from_min_size(n.pos, n.size));
        }
    }
    if blocks.len() < 2 {
        return;
    }

    let offsets = offsets_of(&rects);
    let mut cluster_off = vec![egui::Vec2::ZERO; cluster_count];
    for (&(c, i), off) in blocks.iter().zip(offsets) {
        match c {
            Some(c) => cluster_off[c] = off,
            None => nodes[i].pos += off,
        }
    }
    for (n, c) in nodes.iter_mut().zip(&node_cluster) {
        if let Some(c) = c {
            n.pos += cluster_off[*c];
        }
    }
}

/// Aksi menu "Auto Arrange": susun ulang seluruh diagram (smart layout),
/// lalu rapatkan group dan hilangkan tumpang tindih.
pub fn auto_arrange(state: &mut DiagramState) {
    auto_layout_host(state);
    compact_and_resolve_overlaps(&mut state.nodes, 20.0);
    square_up(&mut state.nodes, 20.0);
}

/// Rapatkan isi group, pisahkan yang tumpang tindih, lalu rapatkan jarak
/// antar group dan tabel tanpa group.
pub fn compact_and_resolve_overlaps(nodes: &mut [DiagramNode], padding: f32) {
    compact_groups(nodes, GROUP_MAX_GAP.max(padding));
    resolve_all_overlaps(nodes, padding, None);
    compact_blocks(nodes, BLOCK_MAX_GAP.max(padding));
    resolve_all_overlaps(nodes, padding, None);
}

/// Auto-arrange tabel host saja; kontainer link database lalu dijajarkan di
/// kanannya (posisi tabel di dalam kontainer milik diagram sumber).
pub fn auto_layout_host(state: &mut DiagramState) {
    if state.linked_databases.is_empty() {
        perform_auto_layout(state);
        return;
    }
    let (linked, host): (Vec<DiagramNode>, Vec<DiagramNode>) = std::mem::take(&mut state.nodes)
        .into_iter()
        .partition(|n| crate::diagram_links::is_linked_id(&n.id));
    state.nodes = host;
    perform_auto_layout(state);
    state.nodes.extend(linked);
    crate::diagram_links::restack_links(state);
}

/// Rasio lebar : tinggi hasil auto-arrange: persegi.
pub const ARRANGE_ASPECT: f32 = 1.0;
/// Rasio dalam rentang `ARRANGE_ASPECT / tol ..= ARRANGE_ASPECT * tol`
/// dianggap sudah persegi.
pub const ARRANGE_ASPECT_TOLERANCE: f32 = 1.15;

/// Kotak pembatas tabel host (tabel link database diabaikan).
fn host_bounds(nodes: &[DiagramNode]) -> Option<egui::Rect> {
    nodes
        .iter()
        .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
        .map(|n| egui::Rect::from_min_size(n.pos, n.size))
        .reduce(|a, b| a.union(b))
}

/// Skalakan posisi pusat tabel host (satu sumbu membesar, sumbu lain
/// mengecil) bila kotak pembatasnya terlalu lebar atau terlalu tinggi.
/// Mengembalikan `false` bila sudah persegi. Tumpang tindih yang muncul
/// diselesaikan pemanggil.
fn stretch_to_square(nodes: &mut [DiagramNode]) -> bool {
    let Some(bounds) = host_bounds(nodes) else {
        return false;
    };
    if bounds.height() <= 0.0 || bounds.width() <= 0.0 {
        return false;
    }
    let ratio = bounds.width() / bounds.height() / ARRANGE_ASPECT;
    if (1.0 / ARRANGE_ASPECT_TOLERANCE..=ARRANGE_ASPECT_TOLERANCE).contains(&ratio) {
        return false;
    }
    let k = (1.0 / ratio).sqrt();
    let c = bounds.center();
    for n in nodes
        .iter_mut()
        .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
    {
        let center = n.pos + n.size / 2.0;
        let moved = egui::pos2(c.x + (center.x - c.x) * k, c.y + (center.y - c.y) / k);
        n.pos = moved - n.size / 2.0;
    }
    true
}

/// Ulangi penskalaan + pemisahan tumpang tindih sampai hasil persegi.
/// Pemisahan cenderung mendorong kembali ke bentuk semula, jadi satu kali
/// tidak cukup.
fn square_up(nodes: &mut [DiagramNode], padding: f32) {
    for _ in 0..8 {
        pack_blocks(nodes, ARRANGE_GAP.max(padding));
        if !stretch_to_square(nodes) {
            break;
        }
        compact_groups(nodes, GROUP_MAX_GAP.max(padding));
        resolve_all_overlaps(nodes, padding, None);
    }
    pack_blocks(nodes, ARRANGE_GAP.max(padding));
}

pub fn perform_auto_layout(state: &mut DiagramState) {
    let iterations = 1000; // Increased iterations for better convergence
    let repulsion_force = 800_000.0; // Stronger base repulsion
    let spring_length = 400.0; // Longer edges
    let attraction_constant = 0.04;
    let center_gravity = 0.01; // Weaker gravity to allow expansion
    let prefix_attraction = 0.05; // Reduced prefix attraction to prevent clumping
    let delta_time = 0.1;

    let node_count = state.nodes.len();
    if node_count == 0 {
        return;
    }

    // Helper to get prefix (e.g., "user" from "user_data")
    let get_prefix = |name: &str| -> String { name.split('_').next().unwrap_or(name).to_string() };

    // Pre-calculate prefixes
    let prefixes: Vec<String> = state.nodes.iter().map(|n| get_prefix(&n.id)).collect();

    for _ in 0..iterations {
        let mut forces = vec![egui::Vec2::ZERO; node_count];

        // 1. Repulsion (between every pair)
        for i in 0..node_count {
            for j in (i + 1)..node_count {
                // Calculate center-to-center distance
                let center_i = state.nodes[i].pos + state.nodes[i].size / 2.0;
                let center_j = state.nodes[j].pos + state.nodes[j].size / 2.0;
                let diff = center_i - center_j;
                let mut dist = diff.length();
                if dist < 1.0 {
                    dist = 1.0;
                } // Avoid zero division

                let mut force_scalar = repulsion_force / (dist * dist);

                // Boost repulsion if prefixes are different
                if prefixes[i] != prefixes[j] {
                    force_scalar *= 5.0; // Stronger group separation
                }

                // COLLISION AVOIDANCE
                // Use actual bounding boxes + margin
                let size_i = state.nodes[i].size;
                let size_j = state.nodes[j].size;

                // Effective radius for fast check
                let r_i = size_i.length() / 2.0;
                let r_j = size_j.length() / 2.0;
                let min_dist_circle = r_i + r_j + 100.0; // generous margin

                if dist < min_dist_circle {
                    // Check for actual Box Overlap for stronger push
                    let delta = diff.abs();
                    let combined_half_size = (size_i + size_j) / 2.0 + egui::vec2(50.0, 50.0); // 50px padding

                    if delta.x < combined_half_size.x && delta.y < combined_half_size.y {
                        // Overlap detected! Explosive force.
                        force_scalar += 2_000_000.0;
                    } else {
                        // Near miss, gentle push
                        force_scalar += 100_000.0 * (min_dist_circle - dist) / min_dist_circle;
                    }
                }

                let force_dir = diff / dist;
                let force = force_dir * force_scalar;

                forces[i] += force;
                forces[j] -= force;
            }
        }

        // 2. Attraction (Edges / Foreign Keys)
        for edge in &state.edges {
            if let Some(src_idx) = state.nodes.iter().position(|n| n.id == edge.source)
                && let Some(dst_idx) = state.nodes.iter().position(|n| n.id == edge.target)
            {
                let diff = state.nodes[src_idx].pos - state.nodes[dst_idx].pos;
                let dist = diff.length();

                if dist > 0.0 {
                    let force_scalar = (dist - spring_length) * attraction_constant;
                    let force_dir = diff / dist;
                    let force = force_dir * force_scalar;

                    forces[src_idx] -= force;
                    forces[dst_idx] += force;
                }
            }
        }

        // 3. Prefix Attraction (Group by name similarity)
        for i in 0..node_count {
            for j in (i + 1)..node_count {
                if prefixes[i] == prefixes[j] {
                    let diff = state.nodes[i].pos - state.nodes[j].pos;
                    let dist = diff.length();
                    if dist > 0.0 {
                        let force_scalar = (dist - (spring_length * 0.8)) * prefix_attraction; // Check if effective
                        let force_dir = diff / dist;
                        let force = force_dir * force_scalar;

                        forces[i] -= force;
                        forces[j] += force;
                    }
                }
            }
        }

        // Catatan: tidak ada tolakan antar group bounds di sini; group yang
        // tidak beririsan dipisahkan pada post-process di bawah.

        // 4. Center Gravity (Pull to 0,0) + Apply Forces
        for (node, force) in state.nodes.iter_mut().zip(forces.iter_mut()) {
            if state.dragging_node.as_deref() == Some(&node.id) {
                continue;
            } // Don't move dragged node

            // Weaker center pull
            let center_pull = egui::Vec2::ZERO - node.pos.to_vec2();
            *force += center_pull * center_gravity;

            // Limit max force to prevent explosion
            let max_force = 1000.0;
            if force.length() > max_force {
                *force = force.normalized() * max_force;
            }

            node.pos += *force * delta_time;
        }
    }

    stretch_to_square(&mut state.nodes);

    // STRICT COLLISION RESOLUTION (Post-Process)
    // Pastikan semua tabel terpisah sempurna dengan padding aman; group yang
    // tidak beririsan ikut dipisahkan bila anti-overlap aktif.
    if state.prevent_overlap {
        resolve_all_overlaps(&mut state.nodes, 20.0, None);
    } else {
        resolve_node_overlaps(&mut state.nodes, 20.0);
    }

    // Normalize coordinates to be positive and start at somewhat reasonable position
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    for node in &state.nodes {
        if node.pos.x < min_x {
            min_x = node.pos.x;
        }
        if node.pos.y < min_y {
            min_y = node.pos.y;
        }
    }

    for node in &mut state.nodes {
        node.pos.x -= min_x - 50.0;
        node.pos.y -= min_y - 50.0;
    }
}

// ============================================================================
// VISUAL EXPLAIN PLAN VIEWER (PostgreSQL / MySQL / Text fallback)
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExplainPlanNode {
    pub node_type: String,
    pub relation_name: Option<String>,
    pub index_name: Option<String>,
    pub alias: Option<String>,
    pub startup_cost: f64,
    pub total_cost: f64,
    pub plan_rows: u64,
    pub plan_width: u64,
    pub actual_startup_time: Option<f64>,
    pub actual_total_time: Option<f64>,
    pub actual_rows: Option<u64>,
    pub actual_loops: Option<u64>,
    pub children: Vec<ExplainPlanNode>,
}

impl ExplainPlanNode {
    pub fn parse(raw_plan: &str) -> Option<Self> {
        let trimmed = raw_plan.trim();
        if (trimmed.starts_with('[') || trimmed.starts_with('{'))
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(node) = Self::parse_json_value(&v)
        {
            return Some(node);
        }

        // Fallback: parse plain text EXPLAIN output lines
        Self::parse_text_lines(trimmed)
    }

    fn parse_json_value(v: &serde_json::Value) -> Option<Self> {
        if let Some(arr) = v.as_array()
            && let Some(first) = arr.first()
        {
            return Self::parse_json_value(first);
        }
        if let Some(obj) = v.as_object() {
            if let Some(plan) = obj.get("Plan") {
                return Self::parse_pg_node(plan);
            }
            if let Some(qb) = obj.get("query_block") {
                return Self::parse_mysql_qb(qb);
            }
            if obj.contains_key("Node Type") {
                return Self::parse_pg_node(v);
            }
        }
        None
    }

    fn parse_pg_node(v: &serde_json::Value) -> Option<Self> {
        let node_type = v.get("Node Type")?.as_str()?.to_string();
        let relation_name = v
            .get("Relation Name")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let index_name = v
            .get("Index Name")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let alias = v
            .get("Alias")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        let startup_cost = v
            .get("Startup Cost")
            .and_then(|n| n.as_f64())
            .unwrap_or(0.0);
        let total_cost = v.get("Total Cost").and_then(|n| n.as_f64()).unwrap_or(0.0);
        let plan_rows = v.get("Plan Rows").and_then(|n| n.as_u64()).unwrap_or(0);
        let plan_width = v.get("Plan Width").and_then(|n| n.as_u64()).unwrap_or(0);

        let actual_startup_time = v.get("Actual Startup Time").and_then(|n| n.as_f64());
        let actual_total_time = v.get("Actual Total Time").and_then(|n| n.as_f64());
        let actual_rows = v.get("Actual Rows").and_then(|n| n.as_u64());
        let actual_loops = v.get("Actual Loops").and_then(|n| n.as_u64());

        let mut children = Vec::new();
        if let Some(plans) = v.get("Plans").and_then(|p| p.as_array()) {
            for child_val in plans {
                if let Some(child_node) = Self::parse_pg_node(child_val) {
                    children.push(child_node);
                }
            }
        }

        Some(ExplainPlanNode {
            node_type,
            relation_name,
            index_name,
            alias,
            startup_cost,
            total_cost,
            plan_rows,
            plan_width,
            actual_startup_time,
            actual_total_time,
            actual_rows,
            actual_loops,
            children,
        })
    }

    fn parse_mysql_qb(v: &serde_json::Value) -> Option<Self> {
        let mut children = Vec::new();
        let mut node_type = "Query Block".to_string();
        let mut total_cost = 0.0;

        if let Some(cost) = v
            .get("cost_info")
            .and_then(|c| c.get("query_cost"))
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<f64>().ok())
        {
            total_cost = cost;
        }

        if let Some(nl) = v.get("nested_loop").and_then(|n| n.as_array()) {
            node_type = "Nested Loop Join".to_string();
            for item in nl {
                if let Some(t) = item.get("table")
                    && let Some(cn) = Self::parse_mysql_table(t)
                {
                    children.push(cn);
                }
            }
        } else if let Some(t) = v.get("table") {
            return Self::parse_mysql_table(t);
        }

        Some(ExplainPlanNode {
            node_type,
            relation_name: None,
            index_name: None,
            alias: None,
            startup_cost: 0.0,
            total_cost,
            plan_rows: 0,
            plan_width: 0,
            actual_startup_time: None,
            actual_total_time: None,
            actual_rows: None,
            actual_loops: None,
            children,
        })
    }

    fn parse_mysql_table(v: &serde_json::Value) -> Option<Self> {
        let table_name = v
            .get("table_name")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
        let access_type = v
            .get("access_type")
            .and_then(|s| s.as_str())
            .unwrap_or("ALL");
        let node_type = match access_type {
            "ALL" => "Seq Scan (Full Table Scan)".to_string(),
            "ref" | "eq_ref" | "const" => "Index Scan".to_string(),
            "range" => "Index Range Scan".to_string(),
            other => format!("{} Scan", other),
        };
        let rows = v
            .get("rows_examined_per_scan")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let cost = v
            .get("cost_info")
            .and_then(|c| c.get("prefix_cost"))
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let key = v.get("key").and_then(|s| s.as_str()).map(|s| s.to_string());

        Some(ExplainPlanNode {
            node_type,
            relation_name: table_name,
            index_name: key,
            alias: None,
            startup_cost: 0.0,
            total_cost: cost,
            plan_rows: rows,
            plan_width: 0,
            actual_startup_time: None,
            actual_total_time: None,
            actual_rows: None,
            actual_loops: None,
            children: Vec::new(),
        })
    }

    fn parse_text_lines(text: &str) -> Option<Self> {
        let lines: Vec<&str> = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return None;
        }
        let first_line = lines[0];
        Some(ExplainPlanNode {
            node_type: first_line.to_string(),
            relation_name: None,
            index_name: None,
            alias: None,
            startup_cost: 0.0,
            total_cost: 1.0,
            plan_rows: 1,
            plan_width: 0,
            actual_startup_time: None,
            actual_total_time: None,
            actual_rows: None,
            actual_loops: None,
            children: Vec::new(),
        })
    }

    pub fn max_cost(&self) -> f64 {
        let mut max_c = self.total_cost;
        for child in &self.children {
            max_c = max_c.max(child.max_cost());
        }
        max_c
    }

    pub fn max_duration(&self) -> f64 {
        let mut max_d = self.actual_total_time.unwrap_or(0.0);
        for child in &self.children {
            max_d = max_d.max(child.max_duration());
        }
        max_d
    }
}

pub fn render_explain_plan_viewer(ui: &mut egui::Ui, raw_plan: &str) {
    crate::query_profiler::render_query_profiler(ui, raw_plan);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta_node(columns: &[(&str, &str)]) -> DiagramNode {
        DiagramNode {
            id: "t".into(),
            title: "t".into(),
            columns: columns.iter().map(|(c, _)| c.to_string()).collect(),
            column_meta: columns
                .iter()
                .map(|(c, t)| crate::models::structs::DiagramColumn {
                    name: c.to_string(),
                    type_name: t.to_string(),
                    is_pk: false,
                    nullable: true,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn node_width_keeps_default_for_short_columns() {
        let node = meta_node(&[("id", "bigint"), ("name", "varchar(50)")]);
        assert_eq!(node_content_width(&node, 7.2, 6.0), 240.0);
        let mut bare = node.clone();
        bare.column_meta.clear();
        assert_eq!(node_content_width(&bare, 7.2, 6.0), 180.0);
    }

    #[test]
    fn node_width_grows_to_fit_long_column_and_type() {
        let long = "material_request_detail_kandang_partnership_type";
        let node = meta_node(&[("id", "bigint"), (long, "varchar(191)")]);
        let w = node_content_width(&node, 7.2, 6.0);
        let needed = 8.0 + long.len() as f32 * 7.2 + 10.0 + 12.0 * 6.0 + 8.0;
        assert!(w >= needed, "width {w} < needed {needed}");
    }

    #[test]
    fn test_parse_postgres_explain_json() {
        let pg_json = r#"[
          {
            "Plan": {
              "Node Type": "Nested Loop",
              "Startup Cost": 0.29,
              "Total Cost": 16.34,
              "Plan Rows": 1,
              "Plan Width": 244,
              "Actual Startup Time": 0.035,
              "Actual Total Time": 0.042,
              "Plans": [
                {
                  "Node Type": "Index Scan",
                  "Relation Name": "users",
                  "Index Name": "users_pkey",
                  "Startup Cost": 0.15,
                  "Total Cost": 8.17,
                  "Plan Rows": 1,
                  "Plan Width": 120
                }
              ]
            }
          }
        ]"#;

        let node = ExplainPlanNode::parse(pg_json).expect("should parse PostgreSQL EXPLAIN JSON");
        assert_eq!(node.node_type, "Nested Loop");
        assert_eq!(node.total_cost, 16.34);
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].node_type, "Index Scan");
        assert_eq!(node.children[0].relation_name.as_deref(), Some("users"));
        assert_eq!(node.children[0].index_name.as_deref(), Some("users_pkey"));
    }

    #[test]
    fn test_parse_mysql_explain_json() {
        let mysql_json = r#"{
          "query_block": {
            "select_id": 1,
            "cost_info": {
              "query_cost": "2.50"
            },
            "table": {
              "table_name": "orders",
              "access_type": "ALL",
              "rows_examined_per_scan": 100,
              "cost_info": {
                "prefix_cost": "2.50"
              }
            }
          }
        }"#;

        let node = ExplainPlanNode::parse(mysql_json).expect("should parse MySQL EXPLAIN JSON");
        assert_eq!(node.relation_name.as_deref(), Some("orders"));
        assert!(node.node_type.contains("Seq Scan"));
        assert_eq!(node.total_cost, 2.50);
        assert_eq!(node.plan_rows, 100);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_zoom_constants() {
        assert!(MIN_ZOOM > 0.0);
        assert!(MAX_ZOOM > MIN_ZOOM);
        assert!(DEFAULT_ZOOM >= MIN_ZOOM && DEFAULT_ZOOM <= MAX_ZOOM);
    }

    #[test]
    fn test_center_diagram_empty() {
        let mut state = DiagramState {
            pan: egui::vec2(100.0, 50.0),
            ..Default::default()
        };
        center_diagram(&mut state, egui::vec2(800.0, 600.0));
        assert_eq!(state.pan, egui::Vec2::ZERO);
    }

    #[test]
    fn test_center_diagram_with_nodes() {
        let mut state = DiagramState::default();
        state.nodes.push(crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "users".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        });

        // Bounding box: min (100, 100), max (300, 200), center (200, 150)
        // Viewport size: (800, 600), view center: (400, 300)
        // Expected pan = (400, 300) - (200, 150) * 1.0 = (200, 150)
        center_diagram(&mut state, egui::vec2(800.0, 600.0));
        assert_eq!(state.pan, egui::vec2(200.0, 150.0));
    }

    #[test]
    fn test_check_nodes_overlap_detection() {
        let node_a = crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "table_a".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        };

        // Node B bertumpukan langsung dengan Node A
        let mut node_b = node_a.clone();
        node_b.id = "table_b".to_string();
        node_b.pos = egui::pos2(150.0, 120.0);

        let nodes = vec![node_a.clone(), node_b];
        assert!(check_nodes_overlap(&nodes, 20.0));
        assert!(check_single_node_collision(&nodes, "table_a", 20.0));

        // Node C berada jauh di posisi aman (tidak bertumpukan)
        let mut node_c = node_a.clone();
        node_c.id = "table_c".to_string();
        node_c.pos = egui::pos2(500.0, 500.0);

        let non_overlapping = vec![node_a, node_c];
        assert!(!check_nodes_overlap(&non_overlapping, 20.0));
        assert!(!check_single_node_collision(
            &non_overlapping,
            "table_a",
            20.0
        ));
    }

    #[test]
    fn test_resolve_node_overlaps_separates_nodes() {
        let node_a = crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "table_a".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        };

        let mut node_b = node_a.clone();
        node_b.id = "table_b".to_string();
        node_b.pos = egui::pos2(120.0, 110.0); // Sengaja tumpang tindih

        let mut nodes = vec![node_a.clone(), node_b.clone()];
        assert!(check_nodes_overlap(&nodes, 20.0));

        // Jalankan resolusi tumpang tindih
        resolve_node_overlaps(&mut nodes, 20.0);

        // Setelah dipisahkan, tidak boleh lagi ada yang tumpang tindih
        assert!(!check_nodes_overlap(&nodes, 20.0));
    }

    #[test]
    fn test_resolve_dragged_node_overlap_leaves_stationary_node_in_place() {
        let node_a = crate::models::structs::DiagramNode {
            id: "table_a".to_string(),
            title: "table_a".to_string(),
            pos: egui::pos2(100.0, 100.0),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        };

        // Node B di-drop tepat menimpa Node A
        let mut node_b = node_a.clone();
        node_b.id = "table_b".to_string();
        node_b.pos = egui::pos2(150.0, 100.0);

        let mut nodes = vec![node_a.clone(), node_b];
        assert!(check_nodes_overlap(&nodes, 20.0));

        // Selesaikan overlap khusus untuk node_b yang di-drag
        resolve_dragged_node_overlap(&mut nodes, "table_b", 20.0);

        // table_a harus tetap stabil di posisi aslinya (100.0, 100.0)
        assert_eq!(nodes[0].pos, egui::pos2(100.0, 100.0));

        // Dan kedua tabel sudah tidak lagi tumpang tindih
        assert!(!check_nodes_overlap(&nodes, 20.0));
    }

    fn grouped_node(id: &str, x: f32, y: f32, groups: &[&str]) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: egui::pos2(x, y),
            size: egui::vec2(200.0, 100.0),
            columns: vec!["id".to_string()],
            foreign_keys: vec![],
            group_ids: groups.iter().map(|g| g.to_string()).collect(),
            group_id: groups.first().map(|g| g.to_string()),
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        }
    }

    #[test]
    fn test_disjoint_groups_are_separated() {
        // Group a dan b tidak berbagi tabel, kotaknya bertumpuk.
        let mut nodes = vec![
            grouped_node("a1", 0.0, 0.0, &["a"]),
            grouped_node("a2", 0.0, 150.0, &["a"]),
            grouped_node("b1", 150.0, 60.0, &["b"]),
        ];
        assert!(check_groups_overlap(&nodes, 20.0));
        resolve_all_overlaps(&mut nodes, 20.0, None);
        assert!(!check_groups_overlap(&nodes, 20.0));
        assert!(!check_nodes_overlap(&nodes, 20.0));
        // Anggota satu group bergeser bersama: jarak relatif tetap.
        assert_eq!(nodes[1].pos - nodes[0].pos, egui::vec2(0.0, 150.0));
    }

    #[test]
    fn test_intersecting_groups_may_overlap() {
        // Tabel "shared" dimiliki a dan b, jadi kedua group boleh bertumpuk.
        let mut nodes = vec![
            grouped_node("a1", 0.0, 0.0, &["a"]),
            grouped_node("shared", 250.0, 0.0, &["a", "b"]),
            grouped_node("b1", 500.0, 0.0, &["b"]),
        ];
        let before: Vec<egui::Pos2> = nodes.iter().map(|n| n.pos).collect();
        assert!(!check_groups_overlap(&nodes, 20.0));
        resolve_group_overlaps(&mut nodes, 20.0, None);
        let after: Vec<egui::Pos2> = nodes.iter().map(|n| n.pos).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn test_compact_groups_limits_gap_inside_group() {
        // Anggota group "a" berjauhan; tabel tanpa group tidak disentuh.
        let mut nodes = vec![
            grouped_node("a1", 0.0, 0.0, &["a"]),
            grouped_node("a2", 1000.0, 0.0, &["a"]),
            grouped_node("a3", 0.0, 900.0, &["a"]),
            grouped_node("free", 5000.0, 5000.0, &[]),
        ];
        compact_groups(&mut nodes, 40.0);
        assert_eq!(nodes[0].pos, egui::pos2(0.0, 0.0));
        assert_eq!(nodes[1].pos, egui::pos2(240.0, 0.0));
        assert_eq!(nodes[2].pos, egui::pos2(0.0, 140.0));
        assert_eq!(nodes[3].pos, egui::pos2(5000.0, 5000.0));
        assert!(!check_nodes_overlap(&nodes, 20.0));
    }

    #[test]
    fn test_compact_blocks_moves_groups_and_free_tables_closer() {
        // Group "a" (dua tabel) dan tabel bebas berjauhan.
        let mut nodes = vec![
            grouped_node("a1", 0.0, 0.0, &["a"]),
            grouped_node("a2", 250.0, 0.0, &["a"]),
            grouped_node("free", 3000.0, 2000.0, &[]),
        ];
        compact_blocks(&mut nodes, 80.0);
        // Anggota group bergeser bersama (di sini tetap karena paling kiri-atas).
        assert_eq!(nodes[0].pos, egui::pos2(0.0, 0.0));
        assert_eq!(nodes[1].pos, egui::pos2(250.0, 0.0));
        // Kotak group: x -20..470, y -66..120 -> tabel bebas di 550, 200.
        assert_eq!(nodes[2].pos, egui::pos2(550.0, 200.0));
        assert!(!check_groups_overlap(&nodes, 20.0));
        assert!(!check_nodes_overlap(&nodes, 20.0));
    }

    #[test]
    fn test_group_mover_yields_to_stationary_group() {
        let mut nodes = vec![
            grouped_node("a1", 0.0, 0.0, &["a"]),
            grouped_node("b1", 150.0, 60.0, &["b"]),
        ];
        resolve_group_overlaps(&mut nodes, 20.0, Some("b"));
        assert_eq!(nodes[0].pos, egui::pos2(0.0, 0.0));
        assert!(!check_groups_overlap(&nodes, 20.0));
    }

    #[test]
    fn test_diagram_state_prevent_overlap_default() {
        let state = DiagramState::default();
        assert!(state.prevent_overlap);

        // JSON tanpa properti prevent_overlap harus mendefaultkan ke true
        let json_data =
            r#"{"nodes":[],"edges":[],"groups":[],"pan":[0.0,0.0],"zoom":1.0,"is_centered":false}"#;
        let deserialized: DiagramState =
            serde_json::from_str(json_data).expect("should deserialize");
        assert!(deserialized.prevent_overlap);
    }

    #[test]
    fn test_diagram_search_filter_flags() {
        let mut state = DiagramState::default();
        assert!(state.search_tables);
        assert!(state.search_columns);
        assert!(state.search_groups);

        // JSON deserialization harus mendefaultkan search flags ke true
        let json_data =
            r#"{"nodes":[],"edges":[],"groups":[],"pan":[0.0,0.0],"zoom":1.0,"is_centered":false}"#;
        let deserialized: DiagramState =
            serde_json::from_str(json_data).expect("should deserialize");
        assert!(deserialized.search_tables);
        assert!(deserialized.search_columns);
        assert!(deserialized.search_groups);

        // Uji fleksibilitas filter pencarian (bisa salah satu, kombinasi, atau semua)
        let table_title = "users";
        let col_name = "email";
        let group_title = "Auth Group";
        let q = "user";

        state.search_tables = true;
        state.search_columns = false;
        state.search_groups = false;
        assert!(state.search_tables && table_title.contains(q));
        assert!(!(state.search_columns && col_name.contains("mail")));
        assert!(!(state.search_groups && group_title.to_lowercase().contains("auth")));

        state.search_tables = false;
        state.search_columns = true;
        assert!(!(state.search_tables && table_title.contains(q)));
        assert!(state.search_columns && col_name.contains("mail"));

        state.search_columns = false;
        state.search_groups = true;
        assert!(state.search_groups && group_title.to_lowercase().contains("auth"));
    }

    #[test]
    fn test_diagram_hand_tool_toggle() {
        let mut state = DiagramState::default();
        assert!(!state.hand_tool);

        // Toggle hand tool aktif
        state.hand_tool = true;
        assert!(state.hand_tool);

        // Deserialisasi JSON tidak terpengaruh oleh hand_tool (karena skip)
        let json_data =
            r#"{"nodes":[],"edges":[],"groups":[],"pan":[0.0,0.0],"zoom":1.0,"is_centered":false}"#;
        let deserialized: DiagramState =
            serde_json::from_str(json_data).expect("should deserialize");
        assert!(!deserialized.hand_tool);
    }

    #[test]
    fn test_column_type_color_categories() {
        let num = column_type_color("bigint");
        let text = column_type_color("varchar(255)");
        let time = column_type_color("timestamp with time zone");
        assert_eq!(column_type_color("INTERVAL"), time);
        assert_eq!(column_type_color("longtext"), text);
        assert_eq!(column_type_color("numeric(10,2)"), num);
        assert_ne!(num, text);
        assert_ne!(column_type_color("jsonb"), column_type_color("uuid"));
        assert_eq!(column_type_color("geometry"), egui::Color32::from_gray(130));
    }

    #[test]
    fn test_key_kind_colors_are_distinct() {
        assert_eq!(KeyKind::of(false, false).color(), None);
        let pk = KeyKind::of(true, false).color();
        let fk = KeyKind::of(false, true).color();
        let both = KeyKind::of(true, true).color();
        assert!(pk != fk && fk != both && pk != both);
        assert_eq!(KeyKind::of(true, true).badge(), "PK FK");
    }

    #[test]
    fn test_diagram_state_show_relations_default() {
        let state = DiagramState::default();
        assert!(state.show_relations);

        // JSON tanpa properti show_relations harus mendefaultkan ke true
        let json_data =
            r#"{"nodes":[],"edges":[],"groups":[],"pan":[0.0,0.0],"zoom":1.0,"is_centered":false}"#;
        let deserialized: DiagramState =
            serde_json::from_str(json_data).expect("should deserialize");
        assert!(deserialized.show_relations);
    }

    #[test]
    fn test_diagram_state_show_relations_toggle_and_persistence() {
        let mut state = DiagramState::default();
        assert!(state.show_relations);

        // Toggle sembunyikan relasi
        state.show_relations = false;
        assert!(!state.show_relations);

        // Serialize ke JSON dan pastikan tersimpan sebagai false
        let serialized = serde_json::to_string(&state).expect("should serialize");
        assert!(serialized.contains(r#""show_relations":false"#));

        // Deserialize kembali dan pastikan nilai false tetap dipertahankan
        let deserialized: DiagramState =
            serde_json::from_str(&serialized).expect("should deserialize");
        assert!(!deserialized.show_relations);
    }

    fn node(id: &str, columns: &[&str]) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            ..Default::default()
        }
    }

    fn fk(
        table: &str,
        col: &str,
        ref_table: &str,
        ref_col: &str,
    ) -> crate::models::structs::ForeignKey {
        crate::models::structs::ForeignKey {
            constraint_name: format!("fk_{table}_{col}"),
            table_name: table.to_string(),
            column_name: col.to_string(),
            referenced_table_name: ref_table.to_string(),
            referenced_column_name: ref_col.to_string(),
        }
    }

    /// users <- orders (FK), orders <- items (virtual), audit terisolasi.
    fn focus_fixture() -> DiagramState {
        let mut orders = node("orders", &["id", "user_id"]);
        orders
            .foreign_keys
            .push(fk("orders", "user_id", "users", "id"));
        DiagramState {
            nodes: vec![
                node("users", &["id", "name"]),
                orders,
                node("items", &["id", "order_id"]),
                node("audit", &["id"]),
            ],
            edges: vec![crate::models::structs::DiagramEdge {
                source: "orders".to_string(),
                target: "users".to_string(),
                label: String::new(),
            }],
            virtual_relations: vec![VirtualRelation {
                child: "items".to_string(),
                child_column: "order_id".to_string(),
                parent: "orders".to_string(),
                parent_column: "id".to_string(),
                origin: RelationOrigin::Manual,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn find_table_id_matches_exact_case_and_schema_prefix() {
        let mut state = focus_fixture();
        state.nodes.push(node("dbo.Invoices", &["id"]));
        assert_eq!(find_table_id(&state, "orders").as_deref(), Some("orders"));
        assert_eq!(find_table_id(&state, "USERS").as_deref(), Some("users"));
        assert_eq!(
            find_table_id(&state, "invoices").as_deref(),
            Some("dbo.Invoices")
        );
        assert_eq!(
            find_table_id(&state, "public.audit").as_deref(),
            Some("audit")
        );
        assert_eq!(find_table_id(&state, "missing"), None);
    }

    #[test]
    fn test_zoom_around_keeps_anchor_fixed() {
        let pan = egui::vec2(40.0, -20.0);
        let anchor = egui::vec2(300.0, 200.0);
        let (old_zoom, new_zoom) = (1.0, 1.4);
        // Titik diagram di bawah jangkar sebelum zoom.
        let world = (anchor - pan) / old_zoom;
        let new_pan = zoom_around(pan, old_zoom, new_zoom, anchor);
        let screen_after = new_pan + world * new_zoom;
        assert!((screen_after - anchor).length() < 1e-3);
    }

    #[test]
    fn test_zoom_around_zero_old_zoom_is_noop() {
        let pan = egui::vec2(5.0, 6.0);
        assert_eq!(zoom_around(pan, 0.0, 1.0, egui::vec2(1.0, 1.0)), pan);
    }

    #[test]
    fn test_pan_to_center_puts_point_in_middle() {
        let pan = pan_to_center(egui::pos2(100.0, 50.0), egui::vec2(800.0, 600.0), 1.0);
        assert_eq!(pan, egui::vec2(300.0, 250.0));
    }

    #[test]
    fn test_sample_view_anim_endpoints() {
        let anim = DiagramViewAnimation {
            from_pan: egui::vec2(0.0, 0.0),
            to_pan: egui::vec2(100.0, 200.0),
            from_zoom: 0.5,
            to_zoom: 1.0,
            start_time: 10.0,
            duration: 0.5,
        };
        let (pan, zoom, done) = sample_view_anim(&anim, 10.0);
        assert_eq!((pan, zoom, done), (egui::vec2(0.0, 0.0), 0.5, false));
        let (pan, zoom, _) = sample_view_anim(&anim, 10.25);
        assert!(
            pan.x > 50.0 && pan.x < 100.0,
            "ease-out melewati titik linear"
        );
        assert!(zoom > 0.75 && zoom < 1.0);
        let (pan, zoom, done) = sample_view_anim(&anim, 11.0);
        assert_eq!((pan, zoom, done), (egui::vec2(100.0, 200.0), 1.0, true));
    }

    #[test]
    fn test_related_tables_includes_fk_and_virtual_both_ways() {
        let state = focus_fixture();
        let set = related_tables(&state, "orders");
        assert!(set.contains("orders"));
        assert!(set.contains("users"));
        assert!(set.contains("items"));
        assert!(!set.contains("audit"));

        let users = related_tables(&state, "users");
        assert!(users.contains("orders"));
        assert!(!users.contains("items"), "hanya relasi langsung");

        let audit = related_tables(&state, "audit");
        assert_eq!(audit.len(), 1);
    }

    #[test]
    fn test_flow_relations_lists_column_links_parent_to_child() {
        let state = focus_fixture();
        let links = flow_relations(&state, "orders");
        assert_eq!(links.len(), 2);
        assert!(links.contains(&(
            "orders".to_string(),
            "user_id".to_string(),
            "users".to_string(),
            "id".to_string()
        )));
        assert!(links.contains(&(
            "items".to_string(),
            "order_id".to_string(),
            "orders".to_string(),
            "id".to_string()
        )));
        assert!(flow_relations(&state, "audit").is_empty());
    }

    #[test]
    fn test_flow_relations_skips_missing_parent_and_dedupes() {
        let mut state = focus_fixture();
        state.nodes[1]
            .foreign_keys
            .push(fk("orders", "ghost_id", "ghost", "id"));
        // Relasi virtual yang sama persis dengan FK tidak digandakan.
        state.virtual_relations.push(VirtualRelation {
            child: "orders".to_string(),
            child_column: "user_id".to_string(),
            parent: "users".to_string(),
            parent_column: "id".to_string(),
            origin: RelationOrigin::Inferred,
        });
        assert_eq!(flow_relations(&state, "orders").len(), 2);
    }

    #[test]
    fn test_relation_sides_stacked_tables_use_same_side() {
        let mut child = node("child", &["a"]);
        let mut parent = node("parent", &["id"]);
        child.size = egui::vec2(300.0, 200.0);
        parent.size = egui::vec2(300.0, 200.0);

        // Berdampingan: sisi yang berhadapan.
        parent.pos = egui::pos2(400.0, 0.0);
        assert_eq!(relation_sides(&child, &parent), (300.0, 400.0, 1.0, -1.0));
        parent.pos = egui::pos2(-400.0, 0.0);
        assert_eq!(relation_sides(&child, &parent), (0.0, -100.0, -1.0, 1.0));

        // Atas-bawah (sedikit bergeser): kiri ke kiri, bukan bentuk S.
        parent.pos = egui::pos2(12.0, 400.0);
        assert_eq!(relation_sides(&child, &parent), (0.0, 12.0, -1.0, -1.0));

        // Tepi kanan lebih sejajar: kanan ke kanan.
        parent.size = egui::vec2(250.0, 200.0);
        parent.pos = egui::pos2(50.0, 400.0);
        assert_eq!(relation_sides(&child, &parent), (300.0, 300.0, 1.0, 1.0));
    }

    #[test]
    fn test_column_anchor_y_follows_header_height() {
        let mut n = node("t", &["a", "b"]);
        assert_eq!(column_anchor_y(&n, "b"), 24.0 + 4.0 + 16.0 + 8.0);
        n.database_name = Some("db".to_string());
        assert_eq!(column_anchor_y(&n, "b"), 30.0 + 4.0 + 16.0 + 8.0);
    }

    #[test]
    fn test_start_focus_animation_targets_node_center() {
        let mut state = focus_fixture();
        state.nodes[0].pos = egui::pos2(100.0, 100.0);
        state.nodes[0].size = egui::vec2(200.0, 100.0);
        state.zoom = 0.6;
        start_focus_animation(&mut state, "users", egui::vec2(800.0, 600.0), 5.0);
        let anim = state.view_anim.as_ref().expect("animasi viewport dimulai");
        assert_eq!(anim.to_zoom, FOCUS_ZOOM);
        assert_eq!(anim.from_zoom, 0.6);
        assert_eq!(anim.to_pan, egui::vec2(200.0, 150.0));
        assert_eq!(
            state.flow_anim.as_ref().map(|f| f.table_id.as_str()),
            Some("users")
        );

        // Tabel yang tidak ada tidak memulai apa pun.
        let mut empty = DiagramState::default();
        start_focus_animation(&mut empty, "nope", egui::vec2(800.0, 600.0), 0.0);
        assert!(empty.view_anim.is_none() && empty.flow_anim.is_none());
    }

    #[test]
    fn test_relation_dim_only_when_focus_elsewhere() {
        let state = focus_fixture();
        let rel = &state.virtual_relations[0];
        let alpha = |focus: Option<&str>| {
            Emphasis {
                focus,
                focus_group: None,
                hovered: None,
                visible: 0,
                dim_opacity: DIM_OPACITY,
            }
            .of(&rel.child, &rel.parent)
            .alpha
        };
        assert_eq!(alpha(None), 1.0);
        assert_eq!(alpha(Some("items")), 1.0);
        assert_eq!(alpha(Some("users")), DIM_OPACITY);
    }

    #[test]
    fn test_focus_state_is_not_serialized() {
        let state = DiagramState {
            focus_table: Some("users".to_string()),
            focus_origin: Some("users".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&state).expect("serialize");
        assert!(!json.contains("focus_table"));
        assert!(!json.contains("flow_anim"));
        assert!(!json.contains("scoped_to"));
        assert!(!json.contains("focus_origin"));
    }

    fn ids(state: &DiagramState) -> Vec<&str> {
        let mut v: Vec<&str> = state.nodes.iter().map(|n| n.id.as_str()).collect();
        v.sort_unstable();
        v
    }

    fn node_rect(state: &DiagramState, id: &str) -> egui::Rect {
        let n = state.nodes.iter().find(|n| n.id == id).expect("node");
        egui::Rect::from_min_size(n.pos, n.size.max(egui::vec2(160.0, 60.0)))
    }

    #[test]
    fn test_focus_subset_keeps_only_related_tables() {
        let sub = focus_subset_state(&focus_fixture(), "orders");
        assert_eq!(ids(&sub), ["items", "orders", "users"]);
        assert_eq!(sub.scoped_to.as_deref(), Some("orders"));
        assert_eq!(sub.focus_origin.as_deref(), Some("orders"));
        assert_eq!(sub.edges.len(), 1);
        assert_eq!(sub.virtual_relations.len(), 1);
        assert!(sub.focus_table.is_none());
        assert!(!sub.is_centered);
    }

    #[test]
    fn test_focus_subset_drops_relations_leaving_the_subset() {
        // items -> orders berada di luar subset "users" (items tidak berelasi langsung).
        let sub = focus_subset_state(&focus_fixture(), "users");
        assert_eq!(ids(&sub), ["orders", "users"]);
        assert_eq!(sub.edges.len(), 1);
        assert!(sub.virtual_relations.is_empty());
    }

    #[test]
    fn test_focus_subset_of_isolated_table() {
        let sub = focus_subset_state(&focus_fixture(), "audit");
        assert_eq!(ids(&sub), ["audit"]);
        assert!(sub.edges.is_empty());
    }

    #[test]
    fn test_focus_subset_drops_groups() {
        let mut source = focus_fixture();
        source.groups.push(crate::models::structs::DiagramGroup {
            id: "g".into(),
            title: "G".into(),
            color: egui::Color32::RED,
            manual_pos: None,
            repo_url: None,
        });
        for n in &mut source.nodes {
            n.group_id = Some("g".into());
            n.group_ids = vec!["g".into()];
        }
        let sub = focus_subset_state(&source, "orders");
        assert!(sub.groups.is_empty());
        assert!(
            sub.nodes
                .iter()
                .all(|n| n.group_id.is_none() && n.group_ids.is_empty())
        );
    }

    fn group_fixture() -> DiagramState {
        let mut source = focus_fixture();
        for (id, title) in [("g", "Sales"), ("h", "Other")] {
            source.groups.push(crate::models::structs::DiagramGroup {
                id: id.into(),
                title: title.into(),
                color: egui::Color32::RED,
                manual_pos: None,
                repo_url: None,
            });
        }
        for (i, n) in source.nodes.iter_mut().enumerate() {
            n.pos = egui::pos2(i as f32 * 300.0, 50.0);
            n.group_ids = match n.id.as_str() {
                // orders anggota dua group sekaligus.
                "orders" => vec!["g".into(), "h".into()],
                "items" => vec!["g".into()],
                "users" => vec!["h".into()],
                _ => Vec::new(),
            };
        }
        source
    }

    #[test]
    fn test_group_subset_keeps_members_positions_and_inner_relations() {
        let source = group_fixture();
        let sub = group_subset_state(&source, "g").expect("group subset");
        assert_eq!(ids(&sub), ["items", "orders"]);
        assert_eq!(sub.scoped_to.as_deref(), Some("g"));
        assert_eq!(sub.focus_origin, None);
        // Hanya relasi items -> orders yang kedua ujungnya di dalam group.
        assert!(sub.edges.is_empty());
        assert_eq!(sub.virtual_relations.len(), 1);
        for n in &sub.nodes {
            let orig = source.nodes.iter().find(|o| o.id == n.id).expect("node");
            assert_eq!(n.pos, orig.pos);
            assert_eq!(n.group_ids, ["g"]);
            assert!(n.group_id.is_none());
        }
        assert_eq!(sub.groups.len(), 1);
        assert_eq!(sub.groups[0].title, "Sales");
    }

    #[test]
    fn test_group_subset_missing_or_empty_group() {
        let mut source = group_fixture();
        assert!(group_subset_state(&source, "nope").is_none());
        source.groups.push(crate::models::structs::DiagramGroup {
            id: "empty".into(),
            title: "Empty".into(),
            color: egui::Color32::RED,
            manual_pos: Some(egui::pos2(0.0, 0.0)),
            repo_url: None,
        });
        assert!(group_subset_state(&source, "empty").is_none());
    }

    /// Parent di kiri tabel fokus, child di kanan, tanpa tumpang tindih.
    #[test]
    fn test_focus_subset_layout_parents_left_children_right() {
        let sub = focus_subset_state(&focus_fixture(), "orders");
        let (focus, parent, child) = (
            node_rect(&sub, "orders"),
            node_rect(&sub, "users"),
            node_rect(&sub, "items"),
        );
        assert!(parent.right() < focus.left());
        assert!(child.left() > focus.right());
        assert!(!parent.intersects(focus) && !child.intersects(focus));
        assert!(sub.nodes.iter().all(|n| n.pos.x >= 0.0 && n.pos.y >= 0.0));
    }

    /// Banyak tabel di satu sisi dipecah menjadi beberapa kolom yang tidak
    /// saling menimpa.
    #[test]
    fn test_focus_subset_layout_wraps_many_children_into_columns() {
        let mut source = DiagramState {
            nodes: vec![node("hub", &["id"])],
            ..Default::default()
        };
        for i in 0..40 {
            let id = format!("child_{i:02}");
            let mut n = node(&id, &["id", "hub_id"]);
            n.size = egui::vec2(200.0, 120.0);
            source.nodes.push(n);
            source.edges.push(crate::models::structs::DiagramEdge {
                source: id,
                target: "hub".into(),
                label: String::new(),
            });
        }
        let sub = focus_subset_state(&source, "hub");
        assert_eq!(sub.nodes.len(), 41);
        let rects: Vec<egui::Rect> = sub.nodes.iter().map(|n| node_rect(&sub, &n.id)).collect();
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
            }
        }
        let columns: HashSet<i64> = sub
            .nodes
            .iter()
            .filter(|n| n.id != "hub")
            .map(|n| n.pos.x.round() as i64)
            .collect();
        assert!(
            columns.len() > 1,
            "40 children must wrap into several columns"
        );
        let hub = node_rect(&sub, "hub");
        assert!(rects.iter().all(|r| *r == hub || r.left() > hub.right()));
    }

    /// Frame pertama tab subset memuat seluruh tabel di viewport.
    #[test]
    fn test_focus_subset_first_frame_fits_viewport() {
        let ctx = egui::Context::default();
        let mut sub = focus_subset_state(&focus_fixture(), "orders");
        render_frame(&ctx, &mut sub, None);
        assert!(sub.is_centered);
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        for n in &sub.nodes {
            let min = egui::Pos2::ZERO + sub.pan + n.pos.to_vec2() * sub.zoom;
            let max = min + n.size * sub.zoom;
            assert!(
                view.contains(min) && view.contains(max),
                "{} outside view",
                n.id
            );
        }
    }

    #[test]
    fn test_group_focus_tables_include_members_and_their_relations() {
        let mut state = focus_fixture();
        for n in &mut state.nodes {
            if n.id == "users" || n.id == "audit" {
                n.group_ids = vec!["g1".to_string()];
            }
        }
        let mut members: Vec<String> = group_members(&state, "g1").into_iter().collect();
        members.sort_unstable();
        assert_eq!(members, ["audit", "users"]);
        let mut set: Vec<String> = group_focus_tables(&state, "g1").into_iter().collect();
        set.sort_unstable();
        // orders berelasi dengan users; items tidak berelasi dengan anggota mana pun.
        assert_eq!(set, ["audit", "orders", "users"]);
        assert!(group_focus_tables(&state, "missing").is_empty());
    }

    /// Fokus group yang group-nya tidak punya anggota dibersihkan saat render.
    #[test]
    fn test_group_focus_cleared_when_group_has_no_members() {
        let ctx = egui::Context::default();
        let mut state = focus_fixture();
        state.is_centered = true;
        state.focus_group = Some("missing".into());
        render_frame(&ctx, &mut state, None);
        assert!(state.focus_group.is_none());
    }

    fn note_fixture(id: &str, table: &str, body: &str) -> crate::models::structs::DiagramNote {
        crate::models::structs::DiagramNote {
            id: id.into(),
            anchor: crate::models::structs::NoteAnchor::Table(table.into()),
            title: format!("Note {id}"),
            body: body.into(),
            color: crate::diagram_notes::NOTE_COLORS[0],
            offset: None,
            size: crate::diagram_notes::DEFAULT_NOTE_SIZE,
            pinned: true,
            author: None,
            created_at: id.into(),
            updated_at: String::new(),
        }
    }

    /// Kartu note dengan link `[[...]]` dirender tanpa panic, ditata di
    /// sekitar tabelnya tanpa saling tumpuk, dan jendela daftarnya terbuka.
    #[test]
    fn test_notes_render_and_arrange() {
        let ctx = egui::Context::default();
        let mut state = focus_fixture();
        state.is_centered = true;
        state.notes = vec![
            note_fixture(
                "a",
                "orders",
                "# Orders\nSee [[users]] and [[items#order_id]].",
            ),
            note_fixture("b", "orders", "- [[ghost]]\n`[[not a link]]`"),
        ];
        state.notes_panel = Some(None);
        render_frame(&ctx, &mut state, None);
        render_frame(&ctx, &mut state, Some(egui::pos2(800.0, 500.0)));
        let a = state.notes[0].offset.expect("note a placed");
        let b = state.notes[1].offset.expect("note b placed");
        let size = egui::vec2(280.0, 220.0);
        let ra = egui::Rect::from_min_size(egui::pos2(a[0], a[1]), size);
        let rb = egui::Rect::from_min_size(egui::pos2(b[0], b[1]), size);
        assert!(!ra.intersects(rb));
    }

    /// Tab subset membawa note milik tabel/group di dalamnya saja.
    #[test]
    fn test_subset_keeps_notes_inside() {
        let mut state = focus_fixture();
        state.notes = vec![
            note_fixture("a", "orders", ""),
            note_fixture("b", "audit", ""),
        ];
        let sub = focus_subset_state(&state, "orders");
        let ids: Vec<&str> = sub.notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }

    /// Chip fokus dengan tombol barunya tetap bisa dirender.
    #[test]
    fn test_focus_chip_renders_with_open_tab_button() {
        let ctx = egui::Context::default();
        let mut state = focus_fixture();
        state.is_centered = true;
        state.focus_table = Some("orders".into());
        render_frame(&ctx, &mut state, None);
        render_frame(&ctx, &mut state, None);
        assert_eq!(state.focus_table.as_deref(), Some("orders"));
    }

    /// Jalankan satu frame `render_diagram` tanpa jendela (1600x1000) dan
    /// tessellate hasilnya. Mengembalikan (jumlah vertex, waktu build+tessellate).
    fn render_frame(
        ctx: &egui::Context,
        state: &mut DiagramState,
        pointer: Option<egui::Pos2>,
    ) -> (usize, std::time::Duration) {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        if let Some(p) = pointer {
            input.events.push(egui::Event::PointerMoved(p));
        }
        let t = std::time::Instant::now();
        let mut out = ctx.run_ui(input, |ui| {
            render_diagram(ui, state);
        });
        // Tanpa renderer, perubahan tekstur font dibuang saja.
        out.textures_delta.clear();
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        let vertices = prims
            .iter()
            .map(|p| match &p.primitive {
                egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                _ => 0,
            })
            .sum();
        (vertices, t.elapsed())
    }

    /// Diagram besar tapi viewport di area kosong: relasi dan tabel di luar
    /// layar tidak boleh menghasilkan geometri.
    #[test]
    fn test_large_diagram_culls_offscreen_content() {
        let ctx = egui::Context::default();
        let mut empty = DiagramState {
            is_centered: true,
            ..Default::default()
        };
        render_frame(&ctx, &mut empty, None);
        let (baseline, _) = render_frame(&ctx, &mut empty, None);

        let mut state = crate::diagram_lod::synthetic_state(800, 16, 2000, 1000);
        state.is_centered = true;
        state.zoom = 1.0;
        state.pan = egui::vec2(50_000.0, 50_000.0);
        render_frame(&ctx, &mut state, None);
        let (far, _) = render_frame(&ctx, &mut state, None);
        assert!(
            far <= baseline + 2_000,
            "off-screen diagram produced {far} vertices (empty canvas: {baseline})"
        );

        state.pan = egui::Vec2::ZERO;
        render_frame(&ctx, &mut state, None);
        let (near, _) = render_frame(&ctx, &mut state, None);
        assert!(near > far, "visible part must still be drawn");
    }

    /// Zoom kecil memakai kartu ringkas + garis gabungan: jauh lebih sedikit
    /// geometri daripada tampilan detail dengan jumlah relasi yang sama.
    #[test]
    fn test_overview_is_lighter_than_detail() {
        let ctx = egui::Context::default();
        let mut state = crate::diagram_lod::synthetic_state(400, 8, 1500, 800);
        state.is_centered = true;
        fit_diagram(&mut state, egui::vec2(1600.0, 1000.0));
        assert!(state.zoom < crate::diagram_lod::DETAIL_MIN_ZOOM);
        render_frame(&ctx, &mut state, None);
        let (overview, _) = render_frame(&ctx, &mut state, None);

        // Kontrol: diagram kecil yang seluruhnya tampil pada zoom detail.
        let mut detail = crate::diagram_lod::synthetic_state(24, 2, 90, 50);
        detail.is_centered = true;
        fit_diagram(&mut detail, egui::vec2(1600.0, 1000.0));
        detail.zoom = crate::diagram_lod::DETAIL_MIN_ZOOM;
        render_frame(&ctx, &mut detail, None);
        let (detail_vertices, _) = render_frame(&ctx, &mut detail, None);
        assert!(
            overview < detail_vertices * 8,
            "overview of 400 tables: {overview} vertices, detail of 24 tables: {detail_vertices}"
        );
    }

    #[test]
    fn test_flow_animation_stops_by_itself() {
        let ctx = egui::Context::default();
        let mut state = focus_fixture();
        state.is_centered = true;
        state.flow_anim = Some(DiagramFlowAnimation {
            table_id: "users".into(),
            start_time: -(crate::diagram_lod::FLOW_ANIM_SECS + 1.0),
        });
        render_frame(&ctx, &mut state, None);
        assert!(state.flow_anim.is_none());
    }

    #[test]
    fn test_fit_diagram_shows_everything() {
        let mut state = crate::diagram_lod::synthetic_state(900, 0, 0, 0);
        let view = egui::vec2(1600.0, 1000.0);
        fit_diagram(&mut state, view);
        let bounds = crate::diagram_lod::content_bounds(&state.nodes).expect("bounds");
        let min = state.pan + bounds.min.to_vec2() * state.zoom;
        let max = state.pan + bounds.max.to_vec2() * state.zoom;
        assert!(min.x >= -1.0 && min.y >= -1.0, "{min:?}");
        assert!(max.x <= view.x + 1.0 && max.y <= view.y + 1.0, "{max:?}");
        assert!(state.zoom >= MIN_ZOOM && state.zoom <= DEFAULT_ZOOM);
    }

    /// Diagram yang terlalu besar untuk zoom minimum: Fit mengarah ke blok
    /// tabel terpadat, bukan ke celah kosong di tengah bounding box.
    #[test]
    fn test_fit_diagram_lands_on_tables_when_too_large() {
        let mut state = crate::diagram_lod::synthetic_state(40, 0, 0, 0);
        // Blok kecil yang sangat jauh di bawah membuat celah kosong besar.
        for (i, n) in state.nodes.iter_mut().enumerate().take(5) {
            n.pos = egui::pos2(i as f32 * 300.0, 140_000.0);
        }
        let view = egui::vec2(1600.0, 1000.0);
        fit_diagram(&mut state, view);
        assert_eq!(state.zoom, MIN_ZOOM);
        let view_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, view);
        let visible = state
            .nodes
            .iter()
            .filter(|n| {
                let c = n.pos + n.size / 2.0;
                view_rect.contains(egui::Pos2::ZERO + state.pan + c.to_vec2() * state.zoom)
            })
            .count();
        assert_eq!(visible, 35, "Fit harus menampilkan blok terbesar");
    }

    /// Benchmark manual (tidak jalan di CI):
    /// `cargo test --release --lib bench_large_diagram -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_large_diagram() {
        let ctx = egui::Context::default();
        let view = egui::vec2(1600.0, 1000.0);
        for (label, zoom) in [
            ("fit", None),
            ("compact 0.4", Some(0.4)),
            ("detail 1.0", Some(1.0)),
        ] {
            let mut state = crate::diagram_lod::synthetic_state(800, 16, 2000, 1000);
            state.is_centered = true;
            fit_diagram(&mut state, view);
            if let Some(z) = zoom {
                set_zoom_centered(&mut state, z, view);
            }
            for _ in 0..3 {
                render_frame(&ctx, &mut state, Some(egui::pos2(800.0, 500.0)));
            }
            let frames = 20;
            let mut total = std::time::Duration::ZERO;
            let mut vertices = 0;
            for i in 0..frames {
                // Pointer bergerak supaya hit-test hover ikut terukur.
                let p = egui::pos2(700.0 + i as f32 * 10.0, 500.0);
                let (v, t) = render_frame(&ctx, &mut state, Some(p));
                total += t;
                vertices = v;
            }
            println!(
                "{label:>12}: zoom {:.2}  {:>7.2} ms/frame  {vertices:>8} vertices",
                state.zoom,
                total.as_secs_f64() * 1000.0 / frames as f64
            );
        }
    }

    /// Benchmark pembanding sebelum/sesudah (kode sama di kedua versi):
    /// `cargo test --release --lib bench_compare -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_compare() {
        let ctx = egui::Context::default();
        let view = egui::vec2(1600.0, 1000.0);
        for zoom in [0.5f32, 1.0] {
            let mut state = crate::diagram_lod::synthetic_state(800, 16, 2000, 1000);
            state.is_centered = true;
            let (mut lo, mut hi) = (state.nodes[0].pos, state.nodes[0].pos);
            for n in &state.nodes {
                lo = lo.min(n.pos);
                hi = hi.max(n.pos + n.size);
            }
            state.zoom = zoom;
            state.pan = pan_to_center(lo + (hi - lo) / 2.0, view, zoom);
            for _ in 0..3 {
                render_frame(&ctx, &mut state, Some(egui::pos2(800.0, 500.0)));
            }
            let frames = 20;
            let mut total = std::time::Duration::ZERO;
            let mut vertices = 0;
            for i in 0..frames {
                let p = egui::pos2(700.0 + i as f32 * 10.0, 500.0);
                let (v, t) = render_frame(&ctx, &mut state, Some(p));
                total += t;
                vertices = v;
            }
            println!(
                "compare zoom {zoom:.1}: {:>8.2} ms/frame  {vertices:>9} vertices",
                total.as_secs_f64() * 1000.0 / frames as f64
            );
        }
    }

    /// Diagram kecil dengan `cards` flow card (satu endpoint per card) yang
    /// masing-masing menyentuh dua tabel dan punya beberapa langkah.
    fn flow_fixture(cards: usize) -> DiagramState {
        use crate::models::structs::{EndpointLink, FlowOp, FlowStep, FlowStepKind, FlowTarget};
        let mut state = DiagramState {
            is_centered: true,
            ..Default::default()
        };
        for (i, t) in ["users", "orders"].iter().enumerate() {
            state.nodes.push(DiagramNode {
                id: t.to_string(),
                title: t.to_string(),
                pos: egui::pos2(600.0 + i as f32 * 320.0, 100.0),
                size: egui::vec2(220.0, 160.0),
                columns: vec!["id".into(), "user_id".into(), "total".into()],
                ..Default::default()
            });
        }
        for c in 0..cards {
            for t in ["users", "orders"] {
                state.endpoint_links.push(EndpointLink {
                    table: t.into(),
                    method: if c % 2 == 0 { "GET" } else { "POST" }.into(),
                    path: format!("/api/orders/{c}"),
                    summary: format!("Endpoint {c}"),
                    request_id: None,
                    repo_key: None,
                    source: None,
                });
            }
        }
        crate::diagram_flow::sync_cards_from_links(&mut state);
        for card in &mut state.flow_cards {
            card.steps = vec![
                FlowStep {
                    kind: FlowStepKind::Auth,
                    title: "Verify token".into(),
                    ..Default::default()
                },
                FlowStep {
                    kind: FlowStepKind::Db,
                    title: "Load the customer".into(),
                    target: Some(FlowTarget::Table("users".into())),
                    op: Some(FlowOp::Read),
                    columns: vec!["id".into()],
                    ..Default::default()
                },
                FlowStep {
                    kind: FlowStepKind::Db,
                    title: "Create the order".into(),
                    target: Some(FlowTarget::Table("orders".into())),
                    op: Some(FlowOp::Insert),
                    columns: vec!["user_id".into(), "total".into()],
                    ..Default::default()
                },
                FlowStep {
                    kind: FlowStepKind::Respond,
                    title: "Return 201".into(),
                    ..Default::default()
                },
            ];
        }
        state
    }

    /// Tab endpoint hanya membawa card itu dan tabel tertautnya, dan frame
    /// pertamanya memuat card beserta tabel di viewport.
    #[test]
    fn test_flow_subset_keeps_card_and_linked_tables() {
        let mut source = flow_fixture(3);
        source.nodes.push(DiagramNode {
            id: "audit".into(),
            title: "audit".into(),
            size: egui::vec2(200.0, 100.0),
            ..Default::default()
        });
        source.endpoint_display = crate::models::structs::EndpointDisplay::Badges;
        let id = source.flow_cards[1].id.clone();
        let mut sub = flow_subset_state(&source, &id).expect("subset");
        let mut tables: Vec<&str> = sub.nodes.iter().map(|n| n.id.as_str()).collect();
        tables.sort();
        assert_eq!(tables, ["orders", "users"]);
        assert_eq!(sub.flow_cards.len(), 1);
        assert_eq!(sub.flow_cards[0].id, id);
        assert_eq!(sub.scoped_to.as_deref(), Some(id.as_str()));
        assert!(sub.endpoint_display.shows_rail());
        assert!(flow_subset_state(&source, "missing").is_none());

        let ctx = egui::Context::default();
        render_frame(&ctx, &mut sub, None);
        let view = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0));
        let frame = crate::diagram_flow_layout::FlowFrame::compute(&sub);
        let card = frame.drawn_rect(&sub, 0);
        let min = egui::Pos2::ZERO + sub.pan + card.min.to_vec2() * sub.zoom;
        let max = egui::Pos2::ZERO + sub.pan + card.max.to_vec2() * sub.zoom;
        assert!(
            view.contains(min) && view.contains(max),
            "card outside view"
        );
    }

    /// Jadikan card `i` card sorotan (lewat `focus_flow`, tanpa memilihnya)
    /// dan pusatkan viewport (1600x1000) ke card itu.
    fn spotlight_and_center(state: &mut DiagramState, i: usize, zoom: f32) {
        state.focus_flow = Some(state.flow_cards[i].id.clone());
        let frame = crate::diagram_flow_layout::FlowFrame::compute(state);
        assert!(frame.is_shown(i));
        state.zoom = zoom;
        state.pan = pan_to_center(
            frame.drawn_rect(state, i).center(),
            egui::vec2(1600.0, 1000.0),
            zoom,
        );
    }

    /// Pojok kiri atas kanvas di layar: panel API rail di kiri menggesernya.
    fn canvas_origin(ctx: &egui::Context, state: &mut DiagramState) -> egui::Pos2 {
        let mut origin = egui::Pos2::ZERO;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 1000.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            render_diagram(ui, state);
            // Panel rail menggeser tepi kiri area tersisa; kanvas mulai
            // dari tepi atas `ui`.
            origin = egui::pos2(ui.available_rect_before_wrap().left(), ui.max_rect().top());
        });
        out.textures_delta.clear();
        origin
    }

    /// Vertex yang ditambahkan card sorotan beserta garisnya, di luar
    /// perubahan panel rail: selisih dengan state tanpa card sorotan,
    /// dikurangi selisih yang sama saat viewport jauh dari isi diagram.
    fn card_vertices(state: &DiagramState) -> usize {
        let diff = |state: &DiagramState| {
            let mut idle = state.clone();
            idle.selected_flow = None;
            idle.focus_flow = None;
            idle.flow_play = None;
            let ctx = egui::Context::default();
            render_frame(&ctx, &mut idle, None);
            let (without, _) = render_frame(&ctx, &mut idle, None);
            let mut shown = state.clone();
            let ctx = egui::Context::default();
            render_frame(&ctx, &mut shown, None);
            let (with, _) = render_frame(&ctx, &mut shown, None);
            with.saturating_sub(without)
        };
        let mut far = state.clone();
        far.pan += egui::vec2(60_000.0, 60_000.0);
        diff(state).saturating_sub(diff(&far))
    }

    /// Mode rail: kanvas tanpa card sampai sebuah endpoint dipilih, lalu
    /// hanya card itu yang digambar.
    #[test]
    fn test_rail_mode_draws_only_the_selected_card() {
        let view = egui::vec2(1600.0, 1000.0);
        let mut state = flow_fixture(40);
        state.endpoint_display = crate::models::structs::EndpointDisplay::Rail;
        let frame = crate::diagram_flow_layout::FlowFrame::compute(&state);
        assert!(frame.shown.iter().all(|s| !s));

        let ctx = egui::Context::default();
        render_frame(&ctx, &mut state, None);
        let (idle, _) = render_frame(&ctx, &mut state, None);

        let id = state.flow_cards[7].id.clone();
        assert!(crate::diagram_flow_view::spotlight_card(
            &mut state, &id, view, 0.0
        ));
        assert_eq!(state.selected_flow.as_deref(), Some(id.as_str()));
        assert_eq!(state.focus_flow.as_deref(), Some(id.as_str()));
        let frame = crate::diagram_flow_layout::FlowFrame::compute(&state);
        assert_eq!(frame.shown.iter().filter(|s| **s).count(), 1);
        assert!(frame.is_shown(7));

        // Lewati animasi viewport: langsung pusatkan card sorotan.
        state.view_anim = None;
        state.zoom = 1.0;
        state.pan = pan_to_center(frame.drawn_rect(&state, 7).center(), view, 1.0);
        let ctx = egui::Context::default();
        render_frame(&ctx, &mut state, None);
        let (selected, _) = render_frame(&ctx, &mut state, None);
        assert!(
            selected > idle,
            "selected card must add geometry ({selected} vs {idle})"
        );
    }

    /// Card digambar penuh di semua zoom: zoom kecil tidak mengecilkannya
    /// menjadi pil atau titik.
    #[test]
    fn test_flow_card_stays_full_when_zoomed_out() {
        let mut state = flow_fixture(12);
        spotlight_and_center(&mut state, 3, 1.0);
        state.selected_flow = state.focus_flow.clone();
        let detail = card_vertices(&state);
        spotlight_and_center(&mut state, 3, 0.2);
        let zoomed_out = card_vertices(&state);
        assert!(
            zoomed_out * 2 > detail,
            "zoomed out {zoomed_out} vertices vs detail {detail}"
        );
    }

    /// Klik endpoint memusatkan viewport ke card-nya pada zoom 50%.
    #[test]
    fn test_spotlight_centers_the_card_at_half_zoom() {
        let view = egui::vec2(1600.0, 1000.0);
        let mut state = flow_fixture(6);
        let id = state.flow_cards[2].id.clone();
        assert!(crate::diagram_flow_view::spotlight_card(
            &mut state, &id, view, 0.0
        ));
        let anim = state.view_anim.as_ref().expect("viewport tween");
        assert_eq!(anim.to_zoom, 0.5);
        let frame = crate::diagram_flow_layout::FlowFrame::compute(&state);
        let center = frame.drawn_rect(&state, 2).center();
        assert_eq!(anim.to_pan, pan_to_center(center, view, 0.5));
    }

    /// Garis proses hanya digambar untuk card sorotan.
    #[test]
    fn test_flow_lines_only_for_the_spotlight_card() {
        let mut state = flow_fixture(6);
        let lines = |state: &DiagramState| {
            let ctx = egui::Context::default();
            let mut drawn = 0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let frame = crate::diagram_flow_layout::FlowFrame::compute(state);
                let (pan, zoom) = (state.pan, state.zoom);
                let to_screen = |p: egui::Pos2| egui::Pos2::ZERO + pan + p.to_vec2() * zoom;
                drawn = crate::diagram_flow_view::draw_flow_lines(
                    ui,
                    state,
                    &frame,
                    &to_screen,
                    ui.clip_rect(),
                );
            });
            out.textures_delta.clear();
            drawn
        };
        assert_eq!(lines(&state), 0);
        spotlight_and_center(&mut state, 0, 1.0);
        assert_eq!(lines(&state), 2);
    }

    /// Double-click card memfokuskan card dan memilihnya; Esc melepasnya.
    #[test]
    fn test_double_click_card_sets_focus_flow() {
        let ctx = egui::Context::default();
        let mut state = flow_fixture(3);
        // Frame awal mengukur ukuran tabel; posisi card sorotan mengikutinya.
        canvas_origin(&ctx, &mut state);
        let origin = canvas_origin(&ctx, &mut state);
        // Card hanya ada di kanvas sebagai card sorotan.
        spotlight_and_center(&mut state, 1, 1.0);
        let frame = crate::diagram_flow_layout::FlowFrame::compute(&state);
        let world = frame.rects[1];
        let to_screen = |p: egui::Pos2| origin + state.pan + p.to_vec2() * state.zoom;
        // Titik di header card, bukan di baris langkah.
        let p = to_screen(egui::pos2(world.center().x, world.top() + 10.0));
        let id = state.flow_cards[1].id.clone();

        let mut t = 0.0;
        let mut step = |state: &mut DiagramState, events: Vec<egui::Event>| {
            t += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                render_diagram(ui, state);
            });
            out.textures_delta.clear();
        };
        let button = |pressed| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        step(&mut state, vec![egui::Event::PointerMoved(p)]);
        step(&mut state, vec![egui::Event::PointerMoved(p)]);
        step(&mut state, vec![button(true)]);
        step(&mut state, vec![button(false)]);
        assert_eq!(state.selected_flow.as_deref(), Some(id.as_str()));
        step(&mut state, vec![button(true)]);
        step(&mut state, vec![button(false)]);
        step(&mut state, vec![]);
        assert_eq!(state.focus_flow.as_deref(), Some(id.as_str()));
        assert!(state.view_anim.is_some(), "viewport tweens to the card");

        step(
            &mut state,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(state.focus_flow.is_none() && state.selected_flow.is_none());
    }

    /// Card terpilih tidak hilang saat klik area lain maupun saat card itu
    /// diklik lagi, dan bisa digeser.
    #[test]
    fn test_card_stays_on_click_and_can_be_dragged() {
        let ctx = egui::Context::default();
        let mut state = flow_fixture(3);
        // Frame awal mengukur ukuran tabel; posisi card sorotan mengikutinya.
        canvas_origin(&ctx, &mut state);
        let origin = canvas_origin(&ctx, &mut state);
        // Card hanya ada di kanvas sebagai card sorotan.
        spotlight_and_center(&mut state, 1, 1.0);
        let frame = crate::diagram_flow_layout::FlowFrame::compute(&state);
        let world = frame.rects[1];
        let (pan, zoom) = (state.pan, state.zoom);
        let to_screen = |p: egui::Pos2| origin + pan + p.to_vec2() * zoom;
        let card = to_screen(egui::pos2(world.center().x, world.top() + 10.0));
        let id = state.flow_cards[1].id.clone();

        let mut t = 0.0;
        let mut step = |state: &mut DiagramState, events: Vec<egui::Event>| {
            // Klik pertama tiap rangkaian didahului jeda > batas double-click.
            t += if events.is_empty() { 1.0 } else { 0.05 };
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                render_diagram(ui, state);
            });
            out.textures_delta.clear();
        };
        let click = |state: &mut DiagramState,
                     step: &mut dyn FnMut(&mut DiagramState, Vec<egui::Event>),
                     p| {
            let button = |pressed| egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            step(state, vec![]);
            step(state, vec![egui::Event::PointerMoved(p)]);
            step(state, vec![button(true)]);
            step(state, vec![button(false)]);
        };

        click(&mut state, &mut step, card);
        assert_eq!(state.selected_flow.as_deref(), Some(id.as_str()));

        // Klik area kanvas lain: detail tetap tampil.
        let empty = to_screen(egui::pos2(world.center().x, world.top() - 400.0));
        click(&mut state, &mut step, empty);
        assert_eq!(state.selected_flow.as_deref(), Some(id.as_str()));

        // Klik card yang sama: card tetap tampil.
        click(&mut state, &mut step, card);
        assert_eq!(state.selected_flow.as_deref(), Some(id.as_str()));

        // Geser card 120 px ke kanan dan 60 px ke bawah.
        let before = crate::diagram_flow_layout::FlowFrame::compute(&state).rects[1].min;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let to = card + egui::vec2(120.0, 60.0);
        step(&mut state, vec![]);
        step(&mut state, vec![egui::Event::PointerMoved(card)]);
        step(&mut state, vec![button(card, true)]);
        step(
            &mut state,
            vec![egui::Event::PointerMoved(card + egui::vec2(60.0, 30.0))],
        );
        step(&mut state, vec![egui::Event::PointerMoved(to)]);
        step(&mut state, vec![button(to, false)]);
        let after = crate::diagram_flow_layout::FlowFrame::compute(&state).rects[1].min;
        let moved = after - before;
        assert!(
            (moved.x - 120.0 / zoom).abs() < 8.0 && (moved.y - 60.0 / zoom).abs() < 8.0,
            "card moved by {moved:?}"
        );
        assert_eq!(state.selected_flow.as_deref(), Some(id.as_str()));
    }

    /// Tabel mirip kasus nyata: beberapa tabel tinggi, relasi berpusat di
    /// `users` dan `user_data`.
    fn arrange_fixture() -> DiagramState {
        let sizes = [
            ("alert", 250.0, 280.0),
            ("users", 220.0, 520.0),
            ("datalogs", 220.0, 520.0),
            ("user_data_expireds", 220.0, 140.0),
            ("user_data", 250.0, 900.0),
            ("data_alerts", 230.0, 560.0),
            ("vehicle", 230.0, 300.0),
        ];
        let mut state = DiagramState::default();
        for (i, (id, w, h)) in sizes.iter().enumerate() {
            let mut n = node(id, &["id"]);
            n.size = egui::vec2(*w, *h);
            n.pos = egui::pos2(i as f32 * 3.0, i as f32 * 2.0);
            state.nodes.push(n);
        }
        for (s, t) in [
            ("alert", "users"),
            ("datalogs", "user_data"),
            ("user_data_expireds", "user_data"),
            ("user_data", "users"),
            ("data_alerts", "user_data"),
            ("vehicle", "users"),
        ] {
            state.edges.push(crate::models::structs::DiagramEdge {
                source: s.into(),
                target: t.into(),
                label: String::new(),
            });
        }
        state
    }

    #[test]
    fn auto_arrange_defaults_to_square() {
        let mut state = arrange_fixture();
        auto_arrange(&mut state);
        let bounds = state
            .nodes
            .iter()
            .map(|n| egui::Rect::from_min_size(n.pos, n.size))
            .reduce(|a, b| a.union(b))
            .unwrap();
        let ratio = bounds.width() / bounds.height() / ARRANGE_ASPECT;
        let tol = ARRANGE_ASPECT_TOLERANCE * 1.1;
        assert!(
            (1.0 / tol..=tol).contains(&ratio),
            "layout {:?} tidak persegi",
            bounds.size()
        );
        for (i, a) in state.nodes.iter().enumerate() {
            for b in &state.nodes[i + 1..] {
                let ra = egui::Rect::from_min_size(a.pos, a.size);
                let rb = egui::Rect::from_min_size(b.pos, b.size);
                let inter = ra.intersect(rb);
                assert!(inter.width() <= 0.0 || inter.height() <= 0.0);
            }
        }
        // Tiap tabel punya tetangga pada jarak rapat.
        for a in &state.nodes {
            let ra = egui::Rect::from_min_size(a.pos, a.size);
            let nearest = state
                .nodes
                .iter()
                .filter(|b| b.id != a.id)
                .map(|b| {
                    let rb = egui::Rect::from_min_size(b.pos, b.size);
                    let dx = (rb.left() - ra.right()).max(ra.left() - rb.right());
                    let dy = (rb.top() - ra.bottom()).max(ra.top() - rb.bottom());
                    dx.max(dy)
                })
                .fold(f32::MAX, f32::min);
            assert!(
                nearest <= ARRANGE_GAP + 1.0,
                "{} terlalu jauh: {nearest}",
                a.id
            );
        }
    }
}
