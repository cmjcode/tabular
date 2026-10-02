//! Ekspor skema diagram ke format SVG (Scalable Vector Graphics).
//!
//! Modul ini menghasilkan dokumen SVG murni (beresolusi tinggi, vector-based)
//! dari [`DiagramState`]. Menampilkan tabel, kolom, badge primary/foreign key,
//! tipe data dengan pewarnaan sintaks, grup tabel, catatan sticky notes,
//! serta relasi foreign key dan virtual relation dengan kurva kubik Bezier yang halus.

use eframe::egui;
use std::collections::HashSet;

use crate::models::structs::{DiagramNode, DiagramState};

/// Pilihan tema warna visual untuk dokumen SVG yang diekspor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SvgTheme {
    /// Tema gelap modern (latar gelap seperti kanvas Tabular).
    #[default]
    Dark,
    /// Tema terang bersih (latar putih untuk cetak atau dokumen).
    Light,
    /// Latar belakang transparan (hanya elemen diagram).
    Transparent,
}

/// Opsi konfigurasi untuk proses render ke format SVG.
#[derive(Clone, Debug)]
pub struct SvgOptions {
    /// Tema warna (Dark, Light, Transparent).
    pub theme: SvgTheme,
    /// Apakah grid latar belakang (titik-titik) ikut digambar.
    pub include_grid: bool,
    /// Apakah bingkai grup tabel ikut digambar.
    pub include_groups: bool,
    /// Apakah relasi antar tabel (kurva Bezier) ikut digambar.
    pub include_relations: bool,
    /// Apakah sticky notes ikut digambar bila ada.
    pub include_notes: bool,
    /// Padding tepi luar kanvas diagram (piksel).
    pub padding: f32,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            theme: SvgTheme::Dark,
            include_grid: true,
            include_groups: true,
            include_relations: true,
            include_notes: true,
            padding: 40.0,
        }
    }
}

impl SvgOptions {
    /// Buat opsi SVG berdasarkan preferensi tampilan yang aktif pada state diagram.
    pub fn from_state(state: &DiagramState) -> Self {
        Self {
            theme: SvgTheme::Dark,
            include_grid: state.show_grid,
            include_groups: true,
            include_relations: state.show_relations,
            include_notes: state.show_notes,
            padding: 40.0,
        }
    }
}

/// Escape karakter spesial XML agar aman dimasukkan ke dalam elemen dan atribut SVG.
pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// Ubah `egui::Color32` menjadi format heksadesimal `#rrggbb`.
fn color_to_hex(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Estimasi dimensi node jika belum pernah diukur atau bernilai default.
fn get_node_dimensions(node: &DiagramNode) -> (f32, f32) {
    let header_h = if node.database_name.is_some() {
        30.0
    } else {
        24.0
    };
    let item_h = 16.0;
    let min_h = header_h + node.columns.len() as f32 * item_h + 8.0;

    let char_w = 7.5;
    let mut max_col_w: f32 = 0.0;
    for col in &node.columns {
        let meta = node.column_info(col);
        let type_chars = meta.map_or(0, |c| c.type_name.len().min(15));
        let badge_chars = if meta.is_some_and(|c| c.is_pk) && node.is_fk_column(col) {
            5
        } else if meta.is_some_and(|c| c.is_pk) || node.is_fk_column(col) {
            2
        } else {
            0
        };
        let mut right = type_chars as f32 * 6.5;
        if badge_chars > 0 {
            if type_chars > 0 {
                right += 6.0;
            }
            right += badge_chars as f32 * 6.5;
        }
        let gap = if right > 0.0 { 12.0 } else { 0.0 };
        let w = 12.0 + col.len() as f32 * char_w + gap + right + 12.0;
        max_col_w = max_col_w.max(w);
    }
    let title_w = 12.0 + node.title.len() as f32 * 8.5 + 24.0;
    let db_w = if let Some(db) = &node.database_name {
        12.0 + (db.len() + node.connection_name.as_ref().map_or(0, |c| c.len() + 1)) as f32 * 6.0
            + 12.0
    } else {
        0.0
    };
    let min_w = max_col_w.max(title_w.max(db_w)).max(160.0);

    let w = if node.size.x >= min_w {
        node.size.x
    } else {
        min_w
    };
    let h = if node.size.y >= min_h {
        node.size.y
    } else {
        min_h
    };
    (w, h)
}

/// Hitung batas kanvas (min_x, min_y, max_x, max_y) seluruh elemen diagram.
fn calculate_bounds(state: &DiagramState, options: &SvgOptions) -> (f32, f32, f32, f32) {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;

    for node in &state.nodes {
        let (w, h) = get_node_dimensions(node);
        min_x = min_x.min(node.pos.x);
        min_y = min_y.min(node.pos.y);
        max_x = max_x.max(node.pos.x + w);
        max_y = max_y.max(node.pos.y + h);
    }

    if options.include_groups {
        for (idx, group) in state.groups.iter().enumerate() {
            let member_nodes: Vec<&DiagramNode> = state
                .nodes
                .iter()
                .filter(|n| n.is_in_group(&group.id))
                .collect();
            if !member_nodes.is_empty() {
                let mut g_min_x = f32::MAX;
                let mut g_min_y = f32::MAX;
                let mut g_max_x = f32::MIN;
                let mut g_max_y = f32::MIN;
                for n in member_nodes {
                    let (w, h) = get_node_dimensions(n);
                    g_min_x = g_min_x.min(n.pos.x);
                    g_min_y = g_min_y.min(n.pos.y);
                    g_max_x = g_max_x.max(n.pos.x + w);
                    g_max_y = g_max_y.max(n.pos.y + h);
                }
                let top_offset = (idx as f32 % 5.0) * 4.0;
                min_x = min_x.min(g_min_x - 20.0);
                min_y = min_y.min(g_min_y - 20.0 - 30.0 - top_offset);
                max_x = max_x.max(g_max_x + 20.0);
                max_y = max_y.max(g_max_y + 20.0);
            } else if let Some(pos) = group.manual_pos {
                min_x = min_x.min(pos.x);
                min_y = min_y.min(pos.y);
                max_x = max_x.max(pos.x + 400.0);
                max_y = max_y.max(pos.y + 300.0);
            }
        }
    }

    if options.include_notes && state.show_notes {
        for note in &state.notes {
            if let Some(ar) = crate::diagram_notes::anchor_rect(state, &note.anchor) {
                if let Some(nr) = crate::diagram_notes::note_rect(ar, note) {
                    min_x = min_x.min(nr.min.x);
                    min_y = min_y.min(nr.min.y);
                    max_x = max_x.max(nr.max.x);
                    max_y = max_y.max(nr.max.y);
                }
            }
        }
    }

    // Jika kanvas kosong sama sekali, berikan viewport default
    if min_x >= max_x || min_y >= max_y {
        return (0.0, 0.0, 800.0, 600.0);
    }

    let pad = options.padding.max(10.0);
    (min_x - pad, min_y - pad, max_x + pad, max_y + pad)
}

/// Konversi seluruh diagram ke dalam bentuk dokumen SVG.
pub fn diagram_to_svg(state: &DiagramState, options: &SvgOptions) -> String {
    let (min_x, min_y, max_x, max_y) = calculate_bounds(state, options);
    let width = (max_x - min_x).ceil();
    let height = (max_y - min_y).ceil();

    let mut svg = String::with_capacity(16 * 1024);

    // Header SVG
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{min_x:.1} {min_y:.1} {width:.1} {height:.1}\" width=\"{width:.0}\" height=\"{height:.0}\">\n"
    ));

    // Definisi styles dan patterns
    render_defs(&mut svg, options);

    // Layer Latar Belakang & Grid
    render_background(&mut svg, min_x, min_y, width, height, options);

    // Layer Grup Tabel
    if options.include_groups {
        render_groups(&mut svg, state);
    }

    // Layer Relasi (Kurva Bezier)
    if options.include_relations {
        render_relations(&mut svg, state, options);
    }

    // Layer Tabel & Kolom
    render_tables(&mut svg, state, options);

    // Layer Catatan Sticky Notes
    if options.include_notes && state.show_notes {
        render_notes(&mut svg, state);
    }

    svg.push_str("</svg>\n");
    svg
}

/// Tulis blok `<defs>` berisi CSS typography dan pattern latar.
fn render_defs(svg: &mut String, options: &SvgOptions) {
    let font_sans =
        "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif";
    let font_mono = "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace";

    let (text_color, grid_dot) = match options.theme {
        SvgTheme::Light => ("#0f172a", "#cbd5e1"),
        SvgTheme::Dark | SvgTheme::Transparent => ("#f4f4f5", "#2e2e36"),
    };

    svg.push_str("  <defs>\n");
    svg.push_str("    <style>\n");
    svg.push_str(&format!(
        "      .diag-text {{ font-family: {font_sans}; fill: {text_color}; }}\n"
    ));
    svg.push_str(&format!(
        "      .diag-mono {{ font-family: {font_mono}; }}\n"
    ));
    svg.push_str("      .table-card { filter: drop-shadow(0 4px 6px rgba(0, 0, 0, 0.25)); }\n");
    svg.push_str("    </style>\n");

    if options.include_grid {
        svg.push_str(&format!(
            "    <pattern id=\"diag-grid\" width=\"20\" height=\"20\" patternUnits=\"userSpaceOnUse\">\n      <circle cx=\"10\" cy=\"10\" r=\"1\" fill=\"{grid_dot}\" />\n    </pattern>\n"
        ));
    }

    svg.push_str("  </defs>\n");
}

/// Render latar kanvas dan grid.
fn render_background(
    svg: &mut String,
    min_x: f32,
    min_y: f32,
    width: f32,
    height: f32,
    options: &SvgOptions,
) {
    if options.theme == SvgTheme::Transparent {
        return;
    }

    let bg_color = match options.theme {
        SvgTheme::Dark => "#141417",
        SvgTheme::Light => "#f8fafc",
        SvgTheme::Transparent => return,
    };

    svg.push_str("  <g id=\"background\">\n");
    svg.push_str(&format!(
        "    <rect x=\"{min_x:.1}\" y=\"{min_y:.1}\" width=\"{width:.1}\" height=\"{height:.1}\" fill=\"{bg_color}\" />\n"
    ));

    if options.include_grid {
        svg.push_str(&format!(
            "    <rect x=\"{min_x:.1}\" y=\"{min_y:.1}\" width=\"{width:.1}\" height=\"{height:.1}\" fill=\"url(#diag-grid)\" />\n"
        ));
    }

    svg.push_str("  </g>\n");
}

/// Render grup tabel sebagai kontainer persegi berujung tumpul dengan header tab.
fn render_groups(svg: &mut String, state: &DiagramState) {
    if state.groups.is_empty() {
        return;
    }

    svg.push_str("  <g id=\"groups\">\n");

    for (idx, group) in state.groups.iter().enumerate() {
        let members: Vec<&DiagramNode> = state
            .nodes
            .iter()
            .filter(|n| n.is_in_group(&group.id))
            .collect();

        let (gx, gy, gw, gh) = if !members.is_empty() {
            let mut min_x = f32::MAX;
            let mut min_y = f32::MAX;
            let mut max_x = f32::MIN;
            let mut max_y = f32::MIN;
            for n in members {
                let (w, h) = get_node_dimensions(n);
                min_x = min_x.min(n.pos.x);
                min_y = min_y.min(n.pos.y);
                max_x = max_x.max(n.pos.x + w);
                max_y = max_y.max(n.pos.y + h);
            }
            let top_offset = (idx as f32 % 5.0) * 4.0;
            let pad = 20.0;
            (
                min_x - pad,
                min_y - pad - 30.0 - top_offset,
                (max_x - min_x) + pad * 2.0,
                (max_y - min_y) + pad * 2.0 + 30.0 + top_offset,
            )
        } else if let Some(pos) = group.manual_pos {
            (pos.x, pos.y, 400.0, 300.0)
        } else {
            continue;
        };

        let color_hex = color_to_hex(group.color);
        let title_escaped = xml_escape(&group.title);
        let tab_w = (group.title.len() as f32 * 7.5 + 24.0).max(80.0);
        let group_id_escaped = xml_escape(&group.id);

        svg.push_str(&format!(
            "    <g id=\"group-{group_id_escaped}\">\n      <rect x=\"{gx:.1}\" y=\"{gy:.1}\" width=\"{gw:.1}\" height=\"{gh:.1}\" rx=\"8\" fill=\"{color_hex}\" fill-opacity=\"0.08\" stroke=\"{color_hex}\" stroke-width=\"1.5\" stroke-dasharray=\"6,4\" />\n      <rect x=\"{tab_x:.1}\" y=\"{tab_y:.1}\" width=\"{tab_w:.1}\" height=\"22\" rx=\"4\" fill=\"{color_hex}\" fill-opacity=\"0.85\" />\n      <text x=\"{text_x:.1}\" y=\"{text_y:.1}\" class=\"diag-text\" font-size=\"11\" font-weight=\"bold\" fill=\"#ffffff\" text-anchor=\"middle\">{title_escaped}</text>\n    </g>\n",
            tab_x = gx + 12.0,
            tab_y = gy + 6.0,
            text_x = gx + 12.0 + tab_w / 2.0,
            text_y = gy + 21.0,
        ));
    }

    svg.push_str("  </g>\n");
}

/// Sisi koneksi relasi: x child, x parent, arah keluar (-1 kiri, +1 kanan).
fn calc_relation_sides(child: &DiagramNode, parent: &DiagramNode) -> (f32, f32, f32, f32) {
    let (cw, _) = get_node_dimensions(child);
    let (pw, _) = get_node_dimensions(parent);
    let (cl, cr) = (child.pos.x, child.pos.x + cw);
    let (pl, pr) = (parent.pos.x, parent.pos.x + pw);
    const FACING_SIDES_MIN_GAP: f32 = 40.0;
    if pl - cr >= FACING_SIDES_MIN_GAP {
        (cr, pl, 1.0, -1.0)
    } else if cl - pr >= FACING_SIDES_MIN_GAP {
        (cl, pr, -1.0, 1.0)
    } else if (cl - pl).abs() <= (cr - pr).abs() {
        (cl, pl, -1.0, -1.0)
    } else {
        (cr, pr, 1.0, 1.0)
    }
}

/// Hitung koordinat Y anchor sebuah kolom pada node tabel.
fn calc_column_anchor_y(node: &DiagramNode, column: &str) -> f32 {
    let hh = if node.database_name.is_some() {
        30.0
    } else {
        24.0
    };
    match node.columns.iter().position(|c| c == column) {
        Some(i) => node.pos.y + hh + 4.0 + i as f32 * 16.0 + 8.0,
        None => {
            let (_, h) = get_node_dimensions(node);
            node.pos.y + h / 2.0
        }
    }
}

/// Render garis lengkung kurva Bezier foreign key dan virtual relation.
fn render_relations(svg: &mut String, state: &DiagramState, options: &SvgOptions) {
    svg.push_str("  <g id=\"relations\">\n");

    let node_map: std::collections::HashMap<&str, &DiagramNode> =
        state.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    // 1. Foreign Key Edges
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for edge in &state.edges {
        if !seen.insert((edge.source.as_str(), edge.target.as_str())) {
            continue;
        }
        let (Some(&src), Some(&dst)) = (
            node_map.get(edge.source.as_str()),
            node_map.get(edge.target.as_str()),
        ) else {
            continue;
        };

        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for fk in &src.foreign_keys {
            if fk.referenced_table_name == edge.target && fk.table_name == src.id {
                let pair = (fk.column_name.as_str(), fk.referenced_column_name.as_str());
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }

        if pairs.is_empty() {
            render_bezier_curve(svg, src, "", dst, "", false, options);
        } else {
            for (c_col, p_col) in pairs {
                render_bezier_curve(svg, src, c_col, dst, p_col, false, options);
            }
        }
    }

    // 2. Virtual Relations
    for rel in &state.virtual_relations {
        let (Some(&src), Some(&dst)) = (
            node_map.get(rel.child.as_str()),
            node_map.get(rel.parent.as_str()),
        ) else {
            continue;
        };
        render_bezier_curve(
            svg,
            src,
            &rel.child_column,
            dst,
            &rel.parent_column,
            true,
            options,
        );
    }

    svg.push_str("  </g>\n");
}

/// Buat elemen `<path>` kurva kubik Bezier dan lingkaran di ujungnya.
fn render_bezier_curve(
    svg: &mut String,
    child: &DiagramNode,
    child_col: &str,
    parent: &DiagramNode,
    parent_col: &str,
    is_virtual: bool,
    options: &SvgOptions,
) {
    let (cx, px, start_dir, end_dir) = calc_relation_sides(child, parent);
    let start_x = cx;
    let start_y = calc_column_anchor_y(child, child_col);
    let end_x = px;
    let end_y = calc_column_anchor_y(parent, parent_col);

    let (c1_x, c1_y, c2_x, c2_y) = if (start_dir - end_dir).abs() < f32::EPSILON {
        let bend = ((end_y - start_y).abs() * 0.15).clamp(30.0, 80.0);
        let outer = if start_dir < 0.0 {
            start_x.min(end_x) - bend
        } else {
            start_x.max(end_x) + bend
        };
        (outer, start_y, outer, end_y)
    } else {
        let bend = (end_x - start_x).abs().max(60.0) * 0.5;
        (
            start_x + bend * start_dir,
            start_y,
            end_x + bend * end_dir,
            end_y,
        )
    };

    let stroke_color = if is_virtual { "#78d7aa" } else { "#5aaaff" };

    let dash_attr = if is_virtual {
        " stroke-dasharray=\"5,4\""
    } else {
        ""
    };

    let bg_color = match options.theme {
        SvgTheme::Light => "#ffffff",
        SvgTheme::Dark | SvgTheme::Transparent => "#18181b",
    };

    svg.push_str(&format!(
        "    <path d=\"M {start_x:.1} {start_y:.1} C {c1_x:.1} {c1_y:.1} {c2_x:.1} {c2_y:.1} {end_x:.1} {end_y:.1}\" fill=\"none\" stroke=\"{stroke_color}\" stroke-width=\"1.8\"{dash_attr} opacity=\"0.85\" />\n    <circle cx=\"{start_x:.1}\" cy=\"{start_y:.1}\" r=\"3\" fill=\"{bg_color}\" stroke=\"{stroke_color}\" stroke-width=\"1.5\" />\n    <circle cx=\"{end_x:.1}\" cy=\"{end_y:.1}\" r=\"3.5\" fill=\"#ffc43c\" />\n"
    ));
}

/// Render kartu tabel, header, dan daftar kolom.
fn render_tables(svg: &mut String, state: &DiagramState, options: &SvgOptions) {
    svg.push_str("  <g id=\"tables\">\n");

    let (card_bg, header_bg, card_border, col_name_color, separator_color) = match options.theme {
        SvgTheme::Light => ("#ffffff", "#f1f5f9", "#cbd5e1", "#1e293b", "#e2e8f0"),
        SvgTheme::Dark | SvgTheme::Transparent => {
            ("#1e1e24", "#272730", "#3f3f46", "#e4e4e7", "#2e2e38")
        }
    };

    for node in &state.nodes {
        let (w, h) = get_node_dimensions(node);
        let x = node.pos.x;
        let y = node.pos.y;
        let hh = if node.database_name.is_some() {
            30.0
        } else {
            24.0
        };
        let title_escaped = xml_escape(&node.title);

        svg.push_str(&format!(
            "    <!-- Table: {title_raw} -->\n    <g id=\"table-{id}\" class=\"table-card\">\n      <rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" rx=\"6\" fill=\"{card_bg}\" stroke=\"{card_border}\" stroke-width=\"1\" />\n      <path d=\"M {x:.1} {y_r:.1} A 6 6 0 0 1 {x_r:.1} {y:.1} L {x_w_r:.1} {y:.1} A 6 6 0 0 1 {x_w:.1} {y_r:.1} L {x_w:.1} {y_hh:.1} L {x:.1} {y_hh:.1} Z\" fill=\"{header_bg}\" />\n      <line x1=\"{x:.1}\" y1=\"{y_hh:.1}\" x2=\"{x_w:.1}\" y2=\"{y_hh:.1}\" stroke=\"{separator_color}\" stroke-width=\"1\" />\n",
            title_raw = node.title,
            id = xml_escape(&node.id),
            y_r = y + 6.0,
            x_r = x + 6.0,
            x_w_r = x + w - 6.0,
            x_w = x + w,
            y_hh = y + hh,
        ));

        // Group dot markers di header
        let mut dot_x = x + 8.0;
        for gid in &node.group_ids {
            if let Some(group) = state.groups.iter().find(|g| &g.id == gid) {
                let color_hex = color_to_hex(group.color);
                svg.push_str(&format!(
                    "      <circle cx=\"{dot_x:.1}\" cy=\"{cy:.1}\" r=\"3.5\" fill=\"{color_hex}\" stroke=\"#000000\" stroke-width=\"0.5\" stroke-opacity=\"0.3\" />\n",
                    cy = y + hh / 2.0,
                ));
                dot_x += 9.0;
            }
        }

        // Judul tabel & nama database
        if let Some(db) = &node.database_name {
            let db_label = if let Some(conn) = &node.connection_name {
                format!("{}/{}", conn, db)
            } else {
                db.clone()
            };
            let db_escaped = xml_escape(&db_label);
            svg.push_str(&format!(
                "      <text x=\"{cx:.1}\" y=\"{ty:.1}\" class=\"diag-text\" font-size=\"12\" font-weight=\"600\" text-anchor=\"middle\">{title_escaped}</text>\n      <text x=\"{cx:.1}\" y=\"{dby:.1}\" class=\"diag-text\" font-size=\"9\" opacity=\"0.65\" text-anchor=\"middle\">[{db_escaped}]</text>\n",
                cx = x + w / 2.0,
                ty = y + 14.0,
                dby = y + 25.5,
            ));
        } else {
            svg.push_str(&format!(
                "      <text x=\"{cx:.1}\" y=\"{ty:.1}\" class=\"diag-text\" font-size=\"13\" font-weight=\"600\" text-anchor=\"middle\">{title_escaped}</text>\n",
                cx = x + w / 2.0,
                ty = y + 16.5,
            ));
        }

        // Daftar baris kolom
        for (idx, col) in node.columns.iter().enumerate() {
            let row_y = y + hh + 4.0 + idx as f32 * 16.0;
            let info = node
                .column_meta
                .get(idx)
                .filter(|m| m.name == *col)
                .or_else(|| node.column_info(col));
            let is_pk = info.is_some_and(|c| c.is_pk);
            let is_fk = node.is_fk_column(col);

            let mut name_x = x + 10.0;

            // Badges PK / FK
            if is_pk && is_fk {
                svg.push_str(&format!(
                    "      <rect x=\"{x_b:.1}\" y=\"{y_b:.1}\" width=\"34\" height=\"12\" rx=\"3\" fill=\"#ff8250\" fill-opacity=\"0.18\" stroke=\"#ff8250\" stroke-width=\"0.8\" />\n      <text x=\"{x_t:.1}\" y=\"{y_t:.1}\" class=\"diag-mono\" font-size=\"8.5\" font-weight=\"bold\" fill=\"#ff8250\" text-anchor=\"middle\">PK FK</text>\n",
                    x_b = x + 8.0,
                    y_b = row_y + 2.0,
                    x_t = x + 25.0,
                    y_t = row_y + 11.0,
                ));
                name_x = x + 46.0;
            } else if is_pk {
                svg.push_str(&format!(
                    "      <rect x=\"{x_b:.1}\" y=\"{y_b:.1}\" width=\"18\" height=\"12\" rx=\"3\" fill=\"#ffc43c\" fill-opacity=\"0.18\" stroke=\"#ffc43c\" stroke-width=\"0.8\" />\n      <text x=\"{x_t:.1}\" y=\"{y_t:.1}\" class=\"diag-mono\" font-size=\"8.5\" font-weight=\"bold\" fill=\"#ffc43c\" text-anchor=\"middle\">PK</text>\n",
                    x_b = x + 8.0,
                    y_b = row_y + 2.0,
                    x_t = x + 17.0,
                    y_t = row_y + 11.0,
                ));
                name_x = x + 30.0;
            } else if is_fk {
                svg.push_str(&format!(
                    "      <rect x=\"{x_b:.1}\" y=\"{y_b:.1}\" width=\"18\" height=\"12\" rx=\"3\" fill=\"#5aaaff\" fill-opacity=\"0.18\" stroke=\"#5aaaff\" stroke-width=\"0.8\" />\n      <text x=\"{x_t:.1}\" y=\"{y_t:.1}\" class=\"diag-mono\" font-size=\"8.5\" font-weight=\"bold\" fill=\"#5aaaff\" text-anchor=\"middle\">FK</text>\n",
                    x_b = x + 8.0,
                    y_b = row_y + 2.0,
                    x_t = x + 17.0,
                    y_t = row_y + 11.0,
                ));
                name_x = x + 30.0;
            }

            let col_name_escaped = xml_escape(col);
            let name_fill = if is_pk {
                "#ffc43c"
            } else if is_fk {
                "#5aaaff"
            } else {
                col_name_color
            };

            svg.push_str(&format!(
                "      <text x=\"{name_x:.1}\" y=\"{ny:.1}\" class=\"diag-mono\" font-size=\"11\" fill=\"{name_fill}\">{col_name_escaped}</text>\n",
                ny = row_y + 12.0,
            ));

            // Tipe data kolom
            if let Some(col_info) = info {
                if !col_info.type_name.is_empty() {
                    let type_color = get_column_type_color(&col_info.type_name, options.theme);
                    let display_type = if col_info.type_name.len() > 15 {
                        format!("{}…", &col_info.type_name[..14])
                    } else {
                        col_info.type_name.clone()
                    };
                    let type_escaped = xml_escape(&display_type);

                    svg.push_str(&format!(
                        "      <text x=\"{tx:.1}\" y=\"{ny:.1}\" class=\"diag-mono\" font-size=\"10\" fill=\"{type_color}\" text-anchor=\"end\">{type_escaped}</text>\n",
                        tx = x + w - 10.0,
                        ny = row_y + 12.0,
                    ));
                }
            }
        }

        svg.push_str("    </g>\n");
    }

    svg.push_str("  </g>\n");
}

/// Dapatkan warna heksadesimal tipe data sesuai kategori sintaks.
fn get_column_type_color(type_name: &str, theme: SvgTheme) -> &'static str {
    let t = type_name.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| t.contains(k));
    let is_light = theme == SvgTheme::Light;

    if has(&["bool", "bit"]) {
        if is_light { "#0891b2" } else { "#5ac3cd" }
    } else if has(&["uuid", "uniqueidentifier", "guid"]) {
        if is_light { "#9333ea" } else { "#dc8cdc" }
    } else if has(&["json", "xml", "array", "[]", "hstore", "object", "document"]) {
        if is_light { "#db2777" } else { "#eb7d96" }
    } else if has(&["date", "time", "year", "interval"]) {
        if is_light { "#7c3aed" } else { "#af96eb" }
    } else if has(&["char", "text", "string", "clob", "enum", "citext"]) {
        if is_light { "#b45309" } else { "#d7af87" }
    } else if has(&[
        "int", "serial", "num", "dec", "float", "double", "real", "money",
    ]) {
        if is_light { "#16a34a" } else { "#82c88c" }
    } else if has(&["blob", "binary", "bytea", "image"]) {
        if is_light { "#64748b" } else { "#9ca3af" }
    } else if is_light {
        "#94a3b8"
    } else {
        "#71717a"
    }
}

/// Render sticky notes bila ada.
fn render_notes(svg: &mut String, state: &DiagramState) {
    let visible = crate::diagram_notes::visible_note_ids(state);
    if visible.is_empty() {
        return;
    }

    svg.push_str("  <g id=\"notes\">\n");

    for note in state.notes.iter().filter(|n| visible.contains(&n.id)) {
        let Some(ar) = crate::diagram_notes::anchor_rect(state, &note.anchor) else {
            continue;
        };
        let Some(nr) = crate::diagram_notes::note_rect(ar, note) else {
            continue;
        };

        let note_bg = color_to_hex(note.color);
        let note_title = xml_escape(&note.title);
        let note_id = xml_escape(&note.id);

        svg.push_str(&format!(
            "    <g id=\"note-{note_id}\">\n      <rect x=\"{nx:.1}\" y=\"{ny:.1}\" width=\"{nw:.1}\" height=\"{nh:.1}\" rx=\"5\" fill=\"{note_bg}\" fill-opacity=\"0.22\" stroke=\"{note_bg}\" stroke-width=\"1.2\" />\n      <rect x=\"{nx:.1}\" y=\"{ny:.1}\" width=\"{nw:.1}\" height=\"22\" rx=\"4\" fill=\"{note_bg}\" fill-opacity=\"0.85\" />\n      <text x=\"{tx:.1}\" y=\"{ty:.1}\" class=\"diag-text\" font-size=\"11\" font-weight=\"bold\" fill=\"#ffffff\">{note_title}</text>\n",
            nx = nr.min.x,
            ny = nr.min.y,
            nw = nr.width(),
            nh = nr.height(),
            tx = nr.min.x + 8.0,
            ty = nr.min.y + 15.0,
        ));

        // Tampilkan beberapa baris pertama isi teks catatan
        let mut line_y = nr.min.y + 36.0;
        for line in note.body.lines().take(6) {
            if line_y > nr.max.y - 10.0 {
                break;
            }
            let escaped_line = xml_escape(line);
            svg.push_str(&format!(
                "      <text x=\"{tx:.1}\" y=\"{line_y:.1}\" class=\"diag-text\" font-size=\"10\" opacity=\"0.85\">{escaped_line}</text>\n",
                tx = nr.min.x + 8.0,
            ));
            line_y += 14.0;
        }

        svg.push_str("    </g>\n");
    }

    svg.push_str("  </g>\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{
        DiagramColumn, DiagramEdge, DiagramGroup, DiagramNode, ForeignKey, RelationOrigin,
        VirtualRelation,
    };

    fn make_test_node(
        id: &str,
        title: &str,
        pos: egui::Pos2,
        cols: &[(&str, &str, bool, bool)],
    ) -> DiagramNode {
        let columns: Vec<String> = cols.iter().map(|(c, _, _, _)| c.to_string()).collect();
        let column_meta: Vec<DiagramColumn> = cols
            .iter()
            .map(|(c, t, pk, _)| DiagramColumn {
                name: c.to_string(),
                type_name: t.to_string(),
                is_pk: *pk,
                nullable: !*pk,
            })
            .collect();
        let foreign_keys: Vec<ForeignKey> = cols
            .iter()
            .filter(|(_, _, _, fk)| *fk)
            .map(|(c, _, _, _)| ForeignKey {
                table_name: id.to_string(),
                column_name: c.to_string(),
                referenced_table_name: "parent_table".to_string(),
                referenced_column_name: "id".to_string(),
                constraint_name: "fk_test".to_string(),
            })
            .collect();

        DiagramNode {
            id: id.to_string(),
            title: title.to_string(),
            pos,
            size: egui::vec2(200.0, 150.0),
            columns,
            foreign_keys,
            column_meta,
            ..Default::default()
        }
    }

    #[test]
    fn test_empty_diagram_produces_valid_svg() {
        let state = DiagramState {
            nodes: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            is_centered: true,
            show_grid: true,
            show_relations: true,
            show_notes: true,
            virtual_relations: Vec::new(),
            linked_databases: Vec::new(),
            diagram_title: None,
            notes: Vec::new(),
            flow_cards: Vec::new(),
            ..Default::default()
        };

        let svg = diagram_to_svg(&state, &SvgOptions::default());
        assert!(svg.starts_with("<svg xmlns="));
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains("viewBox=\"0.0 0.0 800.0 600.0\""));
    }

    #[test]
    fn test_xml_escaping() {
        assert_eq!(xml_escape("users & orders"), "users &amp; orders");
        assert_eq!(
            xml_escape("<script>alert('x')</script>"),
            "&lt;script&gt;alert(&apos;x&apos;)&lt;/script&gt;"
        );
        assert_eq!(xml_escape("col = \"id\""), "col = &quot;id&quot;");
    }

    #[test]
    fn test_single_table_svg_generation() {
        let node = make_test_node(
            "users",
            "users",
            egui::pos2(100.0, 100.0),
            &[
                ("id", "bigint", true, false),
                ("username", "varchar(50)", false, false),
                ("role_id", "integer", false, true),
            ],
        );

        let state = DiagramState {
            nodes: vec![node],
            edges: Vec::new(),
            groups: Vec::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            is_centered: true,
            show_grid: false,
            show_relations: true,
            show_notes: false,
            virtual_relations: Vec::new(),
            linked_databases: Vec::new(),
            diagram_title: Some("My ERD".to_string()),
            notes: Vec::new(),
            flow_cards: Vec::new(),
            ..Default::default()
        };

        let svg = diagram_to_svg(&state, &SvgOptions::default());
        assert!(svg.contains("users"));
        assert!(svg.contains("bigint"));
        assert!(svg.contains("varchar(50)"));
        assert!(svg.contains(">PK<"));
        assert!(svg.contains(">FK<"));
    }

    #[test]
    fn test_groups_and_relations_svg() {
        let node_a = make_test_node(
            "orders",
            "orders",
            egui::pos2(50.0, 50.0),
            &[("id", "int", true, false), ("user_id", "int", false, true)],
        );
        let mut node_b = make_test_node(
            "users",
            "users",
            egui::pos2(350.0, 50.0),
            &[("id", "int", true, false), ("name", "text", false, false)],
        );
        node_b.group_ids.push("group1".to_string());

        let group = DiagramGroup {
            id: "group1".to_string(),
            title: "Auth & Users".to_string(),
            color: egui::Color32::from_rgb(100, 149, 237),
            manual_pos: None,
            repo_url: None,
        };

        let edge = DiagramEdge {
            source: "orders".to_string(),
            target: "users".to_string(),
            label: "orders -> users".to_string(),
        };

        let virt = VirtualRelation {
            child: "orders".to_string(),
            child_column: "user_id".to_string(),
            parent: "users".to_string(),
            parent_column: "id".to_string(),
            origin: RelationOrigin::Manual,
        };

        let state = DiagramState {
            nodes: vec![node_a, node_b],
            edges: vec![edge],
            groups: vec![group],
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            is_centered: true,
            show_grid: true,
            show_relations: true,
            show_notes: false,
            virtual_relations: vec![virt],
            linked_databases: Vec::new(),
            diagram_title: None,
            notes: Vec::new(),
            flow_cards: Vec::new(),
            ..Default::default()
        };

        let svg = diagram_to_svg(&state, &SvgOptions::default());
        // Memastikan grup dirender
        assert!(svg.contains("Auth &amp; Users") || svg.contains("Auth & Users"));
        assert!(svg.contains("id=\"group-group1\""));
        // Memastikan kurva Bezier foreign key dan virtual relation dirender
        assert!(svg.contains("<path d=\"M "));
        assert!(svg.contains("stroke-dasharray=\"5,4\""));
    }

    #[test]
    fn test_theme_options() {
        let state = DiagramState {
            nodes: Vec::new(),
            edges: Vec::new(),
            groups: Vec::new(),
            pan: egui::Vec2::ZERO,
            zoom: 1.0,
            is_centered: true,
            show_grid: false,
            show_relations: false,
            show_notes: false,
            virtual_relations: Vec::new(),
            linked_databases: Vec::new(),
            diagram_title: None,
            notes: Vec::new(),
            flow_cards: Vec::new(),
            ..Default::default()
        };

        let dark_svg = diagram_to_svg(
            &state,
            &SvgOptions {
                theme: SvgTheme::Dark,
                ..Default::default()
            },
        );
        assert!(dark_svg.contains("fill=\"#141417\""));

        let light_svg = diagram_to_svg(
            &state,
            &SvgOptions {
                theme: SvgTheme::Light,
                ..Default::default()
            },
        );
        assert!(light_svg.contains("fill=\"#f8fafc\""));

        let trans_svg = diagram_to_svg(
            &state,
            &SvgOptions {
                theme: SvgTheme::Transparent,
                ..Default::default()
            },
        );
        assert!(!trans_svg.contains("id=\"background\""));
    }
}
