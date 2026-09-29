//! Sticky note Markdown pada diagram: parsing link `[[tabel]]` gaya Obsidian,
//! resolusi ke node, konversi ke Markdown yang bisa dirender, dan penataan
//! kartu di sekitar anchor-nya.
//!
//! Semua fungsi di sini murni (tanpa `egui::Ui`) supaya bisa dites; bagian
//! UI ada di `crate::diagram_notes_view`.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::models::structs::{DiagramNode, DiagramNote, DiagramState, NoteAnchor};
use eframe::egui;

/// Ukuran awal kartu note (koordinat diagram).
pub const DEFAULT_NOTE_SIZE: [f32; 2] = [280.0, 220.0];
/// Ukuran minimum kartu saat di-resize.
pub const MIN_NOTE_SIZE: egui::Vec2 = egui::vec2(160.0, 100.0);
/// Jarak antar kartu dan antara kartu dengan anchor.
pub const NOTE_GAP: f32 = 16.0;
/// Skema URL link hasil konversi `[[...]]`; dipakai sebagai link hook
/// `egui_commonmark`, jadi tidak pernah dibuka sebagai URL sungguhan.
pub const LINK_SCHEME: &str = "tabular-note://";

/// Warna sticky note (pastel, teks gelap tetap terbaca).
pub const NOTE_COLORS: [egui::Color32; 6] = [
    egui::Color32::from_rgb(255, 236, 153),
    egui::Color32::from_rgb(255, 200, 215),
    egui::Color32::from_rgb(195, 240, 195),
    egui::Color32::from_rgb(185, 218, 255),
    egui::Color32::from_rgb(222, 205, 255),
    egui::Color32::from_rgb(255, 214, 170),
];

// Padding bingkai group, sama dengan yang digambar `diagram_view`.
const GROUP_SIDE_PAD: f32 = 20.0;
const GROUP_TOP_PAD: f32 = 50.0;
const EMPTY_GROUP_SIZE: egui::Vec2 = egui::vec2(400.0, 300.0);

/// Satu link `[[target#kolom|alias]]` di dalam isi note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLink {
    pub target: String,
    pub column: Option<String>,
    pub alias: Option<String>,
    /// Rentang byte `[[...]]` (termasuk `!` embed) di teks sumber.
    pub range: Range<usize>,
}

impl WikiLink {
    /// Teks yang ditampilkan untuk link ini.
    pub fn label(&self) -> String {
        match (&self.alias, &self.column) {
            (Some(a), _) => a.clone(),
            (None, Some(c)) => format!("{}.{}", self.target, c),
            (None, None) => self.target.clone(),
        }
    }
}

/// Tabel (dan kolom) tujuan sebuah link yang ditemukan di diagram.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResolvedLink {
    pub table_id: String,
    pub column: Option<String>,
}

/// Semua link `[[...]]` di `body`, kecuali yang berada di code span atau
/// fenced code block.
pub fn parse_wikilinks(body: &str) -> Vec<WikiLink> {
    let mut out = Vec::new();
    let mut fence: Option<(u8, usize)> = None; // (karakter, panjang) pembuka fence
    let mut line_start = 0usize;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some((ch, len)) = fence_marker(trimmed) {
            match fence {
                None => fence = Some((ch, len)),
                Some((open_ch, open_len)) if open_ch == ch && len >= open_len => fence = None,
                Some(_) => {}
            }
            line_start += line.len();
            continue;
        }
        if fence.is_none() {
            scan_line(line, line_start, &mut out);
        }
        line_start += line.len();
    }
    out
}

fn fence_marker(trimmed: &str) -> Option<(u8, usize)> {
    let b = trimmed.as_bytes();
    let ch = *b.first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let len = b.iter().take_while(|&&c| c == ch).count();
    (len >= 3).then_some((ch, len))
}

fn scan_line(line: &str, offset: usize, out: &mut Vec<WikiLink>) {
    let b = line.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == b'`' {
            // Lewati code span: cari penutup dengan jumlah backtick yang sama.
            let run = b[i..].iter().take_while(|&&c| c == b'`').count();
            let mut j = i + run;
            let mut closed = None;
            while j < b.len() {
                if b[j] == b'`' {
                    let r = b[j..].iter().take_while(|&&c| c == b'`').count();
                    if r == run {
                        closed = Some(j + r);
                        break;
                    }
                    j += r;
                } else {
                    j += 1;
                }
            }
            i = closed.unwrap_or(i + run);
            continue;
        }
        if b[i] == b'[' && b.get(i + 1) == Some(&b'[') {
            let inner_start = i + 2;
            if let Some(len) = line[inner_start..].find("]]") {
                let inner = &line[inner_start..inner_start + len];
                let end = inner_start + len + 2;
                if !inner.contains(['[', ']', '\n'])
                    && let Some(link) = parse_inner(inner)
                {
                    let start = if i > 0 && b[i - 1] == b'!' { i - 1 } else { i };
                    out.push(WikiLink {
                        range: offset + start..offset + end,
                        ..link
                    });
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
}

fn parse_inner(inner: &str) -> Option<WikiLink> {
    let (target_part, alias) = match inner.split_once('|') {
        Some((t, a)) => (t, Some(a.trim()).filter(|a| !a.is_empty())),
        None => (inner, None),
    };
    let (target, column) = match target_part.split_once('#') {
        Some((t, c)) => (t.trim(), Some(c.trim()).filter(|c| !c.is_empty())),
        None => (target_part.trim(), None),
    };
    if target.is_empty() {
        return None;
    }
    Some(WikiLink {
        target: target.to_string(),
        column: column.map(str::to_string),
        alias: alias.map(str::to_string),
        range: 0..0,
    })
}

/// Cari tabel bernama `name`. Urutan: id atau judul persis, lalu tanpa
/// membedakan huruf besar/kecil, lalu `database.tabel`, lalu nama tanpa
/// skema (`users` untuk `public.users`) bila hanya satu yang cocok.
/// Tabel milik diagram ini didahulukan dari tabel database yang di-link.
pub fn resolve_table<'a>(nodes: &'a [DiagramNode], name: &str) -> Option<&'a DiagramNode> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let ordered = || {
        nodes
            .iter()
            .filter(|n| !crate::diagram_links::is_linked_id(&n.id))
            .chain(
                nodes
                    .iter()
                    .filter(|n| crate::diagram_links::is_linked_id(&n.id)),
            )
    };
    if let Some(n) = ordered().find(|n| n.id == name || n.title == name) {
        return Some(n);
    }
    if let Some(n) =
        ordered().find(|n| n.id.eq_ignore_ascii_case(name) || n.title.eq_ignore_ascii_case(name))
    {
        return Some(n);
    }
    if let Some((db, table)) = name.rsplit_once('.')
        && let Some(n) = ordered().find(|n| {
            n.title.eq_ignore_ascii_case(table)
                && n.database_name
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(db))
        })
    {
        return Some(n);
    }
    let mut suffix = ordered().filter(|n| {
        n.title
            .rsplit_once('.')
            .is_some_and(|(_, t)| t.eq_ignore_ascii_case(name))
    });
    match (suffix.next(), suffix.next()) {
        (Some(n), None) => Some(n),
        _ => None,
    }
}

/// Resolusi satu link ke tabel (dan kolom bila ada di tabel itu).
pub fn resolve_link(nodes: &[DiagramNode], link: &WikiLink) -> Option<ResolvedLink> {
    let node = resolve_table(nodes, &link.target)?;
    let column = link.column.as_ref().and_then(|c| {
        node.columns
            .iter()
            .find(|x| x.eq_ignore_ascii_case(c))
            .cloned()
    });
    Some(ResolvedLink {
        table_id: node.id.clone(),
        column,
    })
}

fn escape_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '[' | ']' | '*' | '_' | '~' | '`' | '<' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Markdown siap render: tiap `[[...]]` yang ditemukan jadi link
/// `tabular-note://{i}` (i = indeks di vektor hasil), yang tidak ditemukan
/// jadi teks dicoret.
pub fn render_markdown(nodes: &[DiagramNode], body: &str) -> (String, Vec<ResolvedLink>) {
    let links = parse_wikilinks(body);
    let mut out = String::with_capacity(body.len() + links.len() * 16);
    let mut targets = Vec::new();
    let mut cursor = 0usize;
    for link in &links {
        out.push_str(&body[cursor..link.range.start]);
        let label = escape_label(&link.label());
        match resolve_link(nodes, link) {
            Some(r) => {
                out.push_str(&format!("[{label}]({LINK_SCHEME}{})", targets.len()));
                targets.push(r);
            }
            None => out.push_str(&format!("~~{label}~~")),
        }
        cursor = link.range.end;
    }
    out.push_str(&body[cursor..]);
    (out, targets)
}

/// Indeks target dari URL link hasil `render_markdown`.
pub fn link_index(url: &str) -> Option<usize> {
    url.strip_prefix(LINK_SCHEME)?.parse().ok()
}

/// Link dari sebuah note ke tabel lain (untuk garis putus-putus).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteLink {
    pub note_id: String,
    pub table_id: String,
    pub column: Option<String>,
}

/// Link unik dari note-note `note_ids` ke tabel yang ditemukan di diagram.
/// Link ke tabel anchor note itu sendiri diabaikan.
pub fn note_links(state: &DiagramState, note_ids: &HashSet<String>) -> Vec<NoteLink> {
    let mut out = Vec::new();
    let mut seen: HashSet<(String, String, Option<String>)> = HashSet::new();
    for note in state.notes.iter().filter(|n| note_ids.contains(&n.id)) {
        for link in parse_wikilinks(&note.body) {
            let Some(r) = resolve_link(&state.nodes, &link) else {
                continue;
            };
            if note.anchor == NoteAnchor::Table(r.table_id.clone()) && r.column.is_none() {
                continue;
            }
            if seen.insert((note.id.clone(), r.table_id.clone(), r.column.clone())) {
                out.push(NoteLink {
                    note_id: note.id.clone(),
                    table_id: r.table_id,
                    column: r.column,
                });
            }
        }
    }
    out
}

/// Anchor masih ada di diagram.
pub fn anchor_exists(state: &DiagramState, anchor: &NoteAnchor) -> bool {
    match anchor {
        NoteAnchor::Table(id) => state.nodes.iter().any(|n| &n.id == id),
        NoteAnchor::Group(id) => state.groups.iter().any(|g| &g.id == id),
    }
}

/// Nama anchor untuk ditampilkan.
pub fn anchor_label(state: &DiagramState, anchor: &NoteAnchor) -> String {
    match anchor {
        NoteAnchor::Table(id) => state
            .nodes
            .iter()
            .find(|n| &n.id == id)
            .map(|n| n.title.clone())
            .unwrap_or_else(|| id.clone()),
        NoteAnchor::Group(id) => state
            .groups
            .iter()
            .find(|g| &g.id == id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| id.clone()),
    }
}

/// Bingkai anchor dalam koordinat diagram.
pub fn anchor_rect(state: &DiagramState, anchor: &NoteAnchor) -> Option<egui::Rect> {
    match anchor {
        NoteAnchor::Table(id) => state
            .nodes
            .iter()
            .find(|n| &n.id == id)
            .map(|n| egui::Rect::from_min_size(n.pos, n.size)),
        NoteAnchor::Group(id) => {
            let members = state
                .nodes
                .iter()
                .filter(|n| n.is_in_group(id))
                .map(|n| egui::Rect::from_min_size(n.pos, n.size))
                .reduce(|a, b| a.union(b));
            match members {
                Some(r) => Some(egui::Rect::from_min_max(
                    r.min - egui::vec2(GROUP_SIDE_PAD, GROUP_TOP_PAD),
                    r.max + egui::vec2(GROUP_SIDE_PAD, GROUP_SIDE_PAD),
                )),
                None => state
                    .groups
                    .iter()
                    .find(|g| &g.id == id)
                    .and_then(|g| g.manual_pos)
                    .map(|p| egui::Rect::from_min_size(p, EMPTY_GROUP_SIZE)),
            }
        }
    }
}

/// Bingkai kartu note (koordinat diagram), bila anchor ada dan kartu
/// sudah ditata.
pub fn note_rect(anchor: egui::Rect, note: &DiagramNote) -> Option<egui::Rect> {
    let [ox, oy] = note.offset?;
    Some(egui::Rect::from_min_size(
        anchor.min + egui::vec2(ox, oy),
        note_size(note),
    ))
}

pub fn note_size(note: &DiagramNote) -> egui::Vec2 {
    egui::vec2(note.size[0], note.size[1]).max(MIN_NOTE_SIZE)
}

/// Jumlah note per anchor.
pub fn note_counts(notes: &[DiagramNote]) -> HashMap<NoteAnchor, usize> {
    let mut out = HashMap::new();
    for n in notes {
        *out.entry(n.anchor.clone()).or_insert(0) += 1;
    }
    out
}

/// Note yang tampil di kanvas: di-pin atau dibuka user, dan anchor-nya ada.
pub fn visible_note_ids(state: &DiagramState) -> HashSet<String> {
    if !state.show_notes {
        return HashSet::new();
    }
    state
        .notes
        .iter()
        .filter(|n| n.pinned || state.open_notes.contains(&n.id))
        .filter(|n| anchor_exists(state, &n.anchor))
        .map(|n| n.id.clone())
        .collect()
}

/// Note yang anchor-nya sudah tidak ada (tabel/group dihapus atau skema
/// berubah). Tidak dihapus otomatis supaya tulisan user tidak hilang.
pub fn orphan_notes(state: &DiagramState) -> Vec<&DiagramNote> {
    state
        .notes
        .iter()
        .filter(|n| !anchor_exists(state, &n.anchor))
        .collect()
}

#[derive(Clone, Copy)]
enum Side {
    Right,
    Left,
    Below,
}

fn layout_side(anchor: egui::Rect, sizes: &[egui::Vec2], side: Side) -> Vec<egui::Rect> {
    let mut out = Vec::with_capacity(sizes.len());
    match side {
        Side::Right | Side::Left => {
            // Tumpuk ke bawah; kolom baru bila melewati tinggi anchor (min 600).
            let max_h = anchor.height().max(600.0);
            let mut col_x = 0.0f32; // jarak kolom dari anchor
            let mut col_w = 0.0f32;
            let mut y = anchor.min.y;
            for &s in sizes {
                if y > anchor.min.y && y + s.y > anchor.min.y + max_h {
                    col_x += col_w + NOTE_GAP;
                    col_w = 0.0;
                    y = anchor.min.y;
                }
                let x = match side {
                    Side::Right => anchor.max.x + NOTE_GAP + col_x,
                    _ => anchor.min.x - NOTE_GAP - col_x - s.x,
                };
                out.push(egui::Rect::from_min_size(egui::pos2(x, y), s));
                y += s.y + NOTE_GAP;
                col_w = col_w.max(s.x);
            }
        }
        Side::Below => {
            // Berjajar ke kanan; baris baru bila melewati lebar anchor (min 900).
            let max_w = anchor.width().max(900.0);
            let mut row_y = anchor.max.y + NOTE_GAP;
            let mut row_h = 0.0f32;
            let mut x = anchor.min.x;
            for &s in sizes {
                if x > anchor.min.x && x + s.x > anchor.min.x + max_w {
                    row_y += row_h + NOTE_GAP;
                    row_h = 0.0;
                    x = anchor.min.x;
                }
                out.push(egui::Rect::from_min_size(egui::pos2(x, row_y), s));
                x += s.x + NOTE_GAP;
                row_h = row_h.max(s.y);
            }
        }
    }
    out
}

fn overlap_area(rects: &[egui::Rect], obstacles: &[egui::Rect]) -> f32 {
    let pad = NOTE_GAP / 2.0;
    rects
        .iter()
        .flat_map(|r| {
            obstacles.iter().map(move |o| {
                let i = r.expand(pad).intersect(*o);
                if i.is_positive() { i.area() } else { 0.0 }
            })
        })
        .sum()
}

/// Tata kartu berukuran `sizes` di sekitar `anchor`: satu blok rapi di
/// kanan, kiri, atau bawah anchor, pilih sisi yang paling sedikit menutupi
/// `obstacles` (tabel lain dan kartu yang sudah tampil). Hasilnya offset
/// tiap kartu relatif ke `anchor.min`, urutan sama dengan `sizes`.
pub fn arrange_block(
    anchor: egui::Rect,
    sizes: &[egui::Vec2],
    obstacles: &[egui::Rect],
) -> Vec<egui::Vec2> {
    let best = [Side::Right, Side::Left, Side::Below]
        .into_iter()
        .map(|side| {
            let rects = layout_side(anchor, sizes, side);
            (overlap_area(&rects, obstacles), rects)
        })
        // Urutan sisi stabil: bila sama baiknya, kanan menang.
        .reduce(|a, b| if b.0 < a.0 { b } else { a })
        .map(|(_, rects)| rects)
        .unwrap_or_default();
    best.into_iter().map(|r| r.min - anchor.min).collect()
}

/// Tata note `ids` milik `anchor` yang belum punya posisi (atau semua bila
/// `reset`). Tabel lain, kartu lain yang tampil, dan kartu yang sudah
/// berposisi dianggap penghalang. Mengembalikan `true` bila ada perubahan.
pub fn arrange_anchor_notes(
    state: &mut DiagramState,
    anchor: &NoteAnchor,
    visible: &HashSet<String>,
    reset: bool,
) -> bool {
    let Some(a_rect) = anchor_rect(state, anchor) else {
        return false;
    };
    let mut targets: Vec<usize> = state
        .notes
        .iter()
        .enumerate()
        .filter(|(_, n)| &n.anchor == anchor && visible.contains(&n.id))
        .filter(|(_, n)| reset || n.offset.is_none())
        .map(|(i, _)| i)
        .collect();
    if targets.is_empty() {
        return false;
    }
    targets.sort_by(|&a, &b| {
        let (na, nb) = (&state.notes[a], &state.notes[b]);
        na.created_at.cmp(&nb.created_at).then(na.id.cmp(&nb.id))
    });
    let target_ids: HashSet<&str> = targets
        .iter()
        .map(|&i| state.notes[i].id.as_str())
        .collect();

    let mut obstacles: Vec<egui::Rect> = state
        .nodes
        .iter()
        .filter(|n| match anchor {
            NoteAnchor::Table(id) => &n.id != id,
            NoteAnchor::Group(id) => !n.is_in_group(id),
        })
        .map(|n| egui::Rect::from_min_size(n.pos, n.size))
        .collect();
    for n in &state.notes {
        if !visible.contains(&n.id) || target_ids.contains(n.id.as_str()) {
            continue;
        }
        if let Some(r) = anchor_rect(state, &n.anchor).and_then(|ar| note_rect(ar, n)) {
            obstacles.push(r);
        }
    }

    let sizes: Vec<egui::Vec2> = targets
        .iter()
        .map(|&i| note_size(&state.notes[i]))
        .collect();
    let offsets = arrange_block(a_rect, &sizes, &obstacles);
    for (&i, off) in targets.iter().zip(offsets) {
        state.notes[i].offset = Some([off.x, off.y]);
    }
    true
}

/// Id note baru yang unik.
pub fn new_note_id(existing: &[DiagramNote]) -> String {
    let seed = format!(
        "{}-{}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        existing.len()
    );
    let mut id = format!("note_{:x}", md5::compute(seed.as_bytes()));
    while existing.iter().any(|n| n.id == id) {
        id = format!("note_{:x}", md5::compute(id.as_bytes()));
    }
    id
}

/// Nama user OS untuk kolom penulis note.
pub fn current_author() -> Option<String> {
    ["USER", "USERNAME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

/// Judul note untuk daftar: judul, atau baris pertama isi tanpa tanda
/// Markdown, atau "Untitled note".
pub fn display_title(note: &DiagramNote) -> String {
    let t = note.title.trim();
    if !t.is_empty() {
        return t.to_string();
    }
    note.body
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['#', '>', '-', '*', ' '])
                .trim()
        })
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(60).collect())
        .unwrap_or_else(|| "Untitled note".to_string())
}

/// Kata di depan kursor yang sedang diketik sebagai `[[...`: mengembalikan
/// (indeks karakter setelah `[[`, teks yang sudah diketik).
pub fn pending_wikilink(text: &str, cursor_chars: usize) -> Option<(usize, String)> {
    let byte_cursor = text
        .char_indices()
        .nth(cursor_chars)
        .map_or(text.len(), |(b, _)| b);
    let before = &text[..byte_cursor];
    let open = before.rfind("[[")?;
    let typed = &before[open + 2..];
    if typed.contains([']', '[', '\n', '|', '#']) {
        return None;
    }
    let start_chars = before[..open + 2].chars().count();
    Some((start_chars, typed.to_string()))
}

/// Nama tabel yang cocok dengan `query` untuk saran link (maks `limit`).
/// Awalan nama didahulukan dari yang hanya memuat `query`.
pub fn table_suggestions(nodes: &[DiagramNode], query: &str, limit: usize) -> Vec<String> {
    let q = query.trim().to_lowercase();
    let mut prefix = Vec::new();
    let mut contains = Vec::new();
    let mut seen = HashSet::new();
    for n in nodes {
        if crate::diagram_links::is_linked_id(&n.id) || !seen.insert(n.title.as_str()) {
            continue;
        }
        let t = n.title.to_lowercase();
        if t.starts_with(&q) {
            prefix.push(n.title.clone());
        } else if t.contains(&q) {
            contains.push(n.title.clone());
        }
    }
    prefix.sort();
    contains.sort();
    prefix.into_iter().chain(contains).take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, cols: &[&str], pos: (f32, f32)) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            pos: egui::pos2(pos.0, pos.1),
            size: egui::vec2(200.0, 120.0),
            columns: cols.iter().map(|c| c.to_string()).collect(),
            foreign_keys: Vec::new(),
            group_ids: Vec::new(),
            group_id: None,
            column_meta: Vec::new(),
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        }
    }

    fn note(id: &str, anchor: NoteAnchor, body: &str) -> DiagramNote {
        DiagramNote {
            id: id.into(),
            anchor,
            title: String::new(),
            body: body.into(),
            color: NOTE_COLORS[0],
            offset: None,
            size: DEFAULT_NOTE_SIZE,
            pinned: false,
            author: None,
            created_at: id.into(),
            updated_at: String::new(),
        }
    }

    fn targets(body: &str) -> Vec<(String, Option<String>, Option<String>)> {
        parse_wikilinks(body)
            .into_iter()
            .map(|l| (l.target, l.column, l.alias))
            .collect()
    }

    #[test]
    fn test_parse_basic_alias_and_column() {
        let body = "See [[users]] and [[orders#user_id|the FK]] plus ![[items]].";
        assert_eq!(
            targets(body),
            vec![
                ("users".into(), None, None),
                (
                    "orders".into(),
                    Some("user_id".into()),
                    Some("the FK".into())
                ),
                ("items".into(), None, None),
            ]
        );
        let links = parse_wikilinks(body);
        assert_eq!(&body[links[0].range.clone()], "[[users]]");
        assert_eq!(&body[links[2].range.clone()], "![[items]]");
    }

    #[test]
    fn test_parse_skips_code_and_invalid() {
        let body =
            "`[[inline]]` [[ ]] [[a[b]] [[ok]]\n```sql\n[[fenced]]\n```\n``x [[two]] `` [[after]]";
        assert_eq!(
            targets(body).into_iter().map(|t| t.0).collect::<Vec<_>>(),
            vec!["ok".to_string(), "after".to_string()]
        );
    }

    #[test]
    fn test_parse_multibyte_offsets() {
        let body = "Catatan é → [[pelanggan]] ✓";
        let links = parse_wikilinks(body);
        assert_eq!(links.len(), 1);
        assert_eq!(&body[links[0].range.clone()], "[[pelanggan]]");
    }

    #[test]
    fn test_resolve_table_variants() {
        let mut a = node("public.users", &["id", "Email"], (0.0, 0.0));
        a.title = "public.users".into();
        let mut b = node("orders", &["id"], (0.0, 0.0));
        b.database_name = Some("shop".into());
        let linked = node("L1::orders", &["id"], (0.0, 0.0));
        let nodes = vec![linked, a, b];
        assert_eq!(resolve_table(&nodes, "ORDERS").unwrap().id, "orders");
        assert_eq!(resolve_table(&nodes, "shop.orders").unwrap().id, "orders");
        assert_eq!(resolve_table(&nodes, "users").unwrap().id, "public.users");
        assert!(resolve_table(&nodes, "nope").is_none());
        let link = &parse_wikilinks("[[users#email]]")[0];
        assert_eq!(
            resolve_link(&nodes, link),
            Some(ResolvedLink {
                table_id: "public.users".into(),
                column: Some("Email".into())
            })
        );
    }

    #[test]
    fn test_render_markdown_links_and_missing() {
        let nodes = vec![node("users", &["id"], (0.0, 0.0))];
        let (md, targets) = render_markdown(&nodes, "Go [[users|the_users]] not [[ghost]]");
        assert_eq!(md, "Go [the\\_users](tabular-note://0) not ~~ghost~~");
        assert_eq!(targets[0].table_id, "users");
        assert_eq!(link_index("tabular-note://0"), Some(0));
        assert_eq!(link_index("https://x"), None);
    }

    #[test]
    fn test_note_links_dedup_and_skip_self() {
        let state = DiagramState {
            nodes: vec![
                node("users", &["id"], (0.0, 0.0)),
                node("orders", &["user_id"], (400.0, 0.0)),
            ],
            notes: vec![
                note(
                    "n1",
                    NoteAnchor::Table("users".into()),
                    "[[orders]] [[orders]] [[users]] [[orders#user_id]] [[x]]",
                ),
                note("n2", NoteAnchor::Table("users".into()), "[[orders]]"),
            ],
            ..Default::default()
        };
        let visible: HashSet<String> = ["n1".to_string()].into();
        let links = note_links(&state, &visible);
        assert_eq!(links.len(), 2);
        assert!(
            links
                .iter()
                .all(|l| l.note_id == "n1" && l.table_id == "orders")
        );
        assert_eq!(links[1].column.as_deref(), Some("user_id"));
    }

    #[test]
    fn test_arrange_block_no_overlap_and_avoids_obstacles() {
        let anchor = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 120.0));
        let sizes = vec![egui::vec2(280.0, 220.0); 4];
        let offsets = arrange_block(anchor, &sizes, &[]);
        let rects: Vec<egui::Rect> = offsets
            .iter()
            .zip(&sizes)
            .map(|(o, s)| egui::Rect::from_min_size(anchor.min + *o, *s))
            .collect();
        for (i, a) in rects.iter().enumerate() {
            assert!(!a.intersects(anchor), "card {i} overlaps anchor");
            for b in &rects[i + 1..] {
                assert!(!a.intersects(*b));
            }
        }
        // Tanpa penghalang: kanan.
        assert!(rects.iter().all(|r| r.min.x >= anchor.max.x));
        // Tabel tepat di kanan: pindah ke sisi lain.
        let blocker = egui::Rect::from_min_size(egui::pos2(230.0, -50.0), egui::vec2(800.0, 900.0));
        let offsets = arrange_block(anchor, &sizes, &[blocker]);
        assert!(offsets.iter().all(|o| o.x < 0.0 || o.y > 0.0));
    }

    #[test]
    fn test_arrange_anchor_notes_only_unplaced() {
        let mut state = DiagramState {
            nodes: vec![node("users", &["id"], (0.0, 0.0))],
            notes: vec![
                note("a", NoteAnchor::Table("users".into()), ""),
                note("b", NoteAnchor::Table("users".into()), ""),
            ],
            ..Default::default()
        };
        state.notes[0].offset = Some([500.0, 0.0]);
        let visible: HashSet<String> = ["a".to_string(), "b".to_string()].into();
        assert!(arrange_anchor_notes(
            &mut state,
            &NoteAnchor::Table("users".into()),
            &visible,
            false
        ));
        assert_eq!(state.notes[0].offset, Some([500.0, 0.0]));
        let b = state.notes[1].offset.expect("b placed");
        let ra = egui::Rect::from_min_size(egui::pos2(500.0, 0.0), egui::vec2(280.0, 220.0));
        let rb = egui::Rect::from_min_size(egui::pos2(b[0], b[1]), egui::vec2(280.0, 220.0));
        assert!(!ra.intersects(rb));
    }

    #[test]
    fn test_orphans_and_visibility() {
        let mut state = DiagramState {
            nodes: vec![node("users", &["id"], (0.0, 0.0))],
            notes: vec![
                note("a", NoteAnchor::Table("users".into()), ""),
                note("b", NoteAnchor::Table("gone".into()), ""),
                note("c", NoteAnchor::Group("g".into()), ""),
            ],
            ..Default::default()
        };
        state.notes[0].pinned = true;
        state.open_notes.insert("b".into());
        assert_eq!(
            orphan_notes(&state)
                .iter()
                .map(|n| n.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "c"]
        );
        let v = visible_note_ids(&state);
        assert!(v.contains("a") && !v.contains("b"));
        state.show_notes = false;
        assert!(visible_note_ids(&state).is_empty());
    }

    #[test]
    fn test_group_anchor_rect_wraps_members() {
        let mut n = node("users", &["id"], (100.0, 100.0));
        n.group_ids = vec!["g".into()];
        let state = DiagramState {
            nodes: vec![n],
            groups: vec![crate::models::structs::DiagramGroup {
                id: "g".into(),
                title: "Core".into(),
                color: egui::Color32::RED,
                manual_pos: None,
                repo_url: None,
            }],
            ..Default::default()
        };
        let r = anchor_rect(&state, &NoteAnchor::Group("g".into())).unwrap();
        assert_eq!(r.min, egui::pos2(80.0, 50.0));
        assert_eq!(anchor_label(&state, &NoteAnchor::Group("g".into())), "Core");
    }

    #[test]
    fn test_pending_wikilink_and_suggestions() {
        assert_eq!(pending_wikilink("see [[us", 9), Some((6, "us".into())));
        assert_eq!(pending_wikilink("see [[us]] x", 12), None);
        assert_eq!(pending_wikilink("é [[", 4), Some((4, String::new())));
        let nodes = vec![
            node("users", &[], (0.0, 0.0)),
            node("app_users", &[], (0.0, 0.0)),
            node("orders", &[], (0.0, 0.0)),
        ];
        assert_eq!(
            table_suggestions(&nodes, "us", 5),
            vec!["users", "app_users"]
        );
    }

    #[test]
    fn test_display_title_and_serde_roundtrip() {
        let mut n = note(
            "a",
            NoteAnchor::Group("g".into()),
            "\n## Invoice rules\nbody",
        );
        assert_eq!(display_title(&n), "Invoice rules");
        n.title = "Custom".into();
        assert_eq!(display_title(&n), "Custom");
        let json = serde_json::to_string(&n).unwrap();
        assert!(json.contains(r#""anchor":{"kind":"group","id":"g"}"#));
        let back: DiagramNote = serde_json::from_str(&json).unwrap();
        assert_eq!(back, n);
        // Diagram lama tanpa `notes` tetap termuat.
        let old: DiagramState = serde_json::from_str(
            r#"{"nodes":[],"edges":[],"groups":[],"pan":[0,0],"zoom":1,"is_centered":false}"#,
        )
        .unwrap();
        assert!(old.notes.is_empty() && old.show_notes);
    }
}
