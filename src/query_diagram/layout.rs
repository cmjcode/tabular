//! Tata letak kartu dan alur untuk diagram query (koordinat diagram, tanpa
//! egui painter). Kiri ke kanan sesuai urutan eksekusi SQL: sumber → WHERE →
//! GROUP BY → HAVING → ORDER BY/LIMIT → hasil/SET → target.

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};

use super::{QueryDiagramModel, SourceKind, StatementKind, clip};

pub const HEADER_H: f32 = 34.0;
pub const ROW_H: f32 = 22.0;
pub const CARD_PAD: f32 = 6.0;
const LANE_GAP: f32 = 100.0;
const CARD_GAP: f32 = 30.0;
const MIN_CARD_W: f32 = 150.0;
const MAX_CARD_W: f32 = 320.0;

/// Peran kartu; menentukan warna dan animasi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardRole {
    Source,
    /// Tahap WHERE (menyaring baris).
    Clauses,
    /// Tahap GROUP BY (kunci grup + agregat).
    Group,
    /// Tahap HAVING (menyaring grup).
    Having,
    /// Tahap window function (`OVER (...)`).
    Window,
    /// Penggabungan UNION / INTERSECT / EXCEPT.
    Union,
    /// Tahap DISTINCT / ORDER BY / LIMIT.
    Sort,
    Values,
    Result,
    Set,
    Target,
}

/// Keadaan baris kolom untuk pewarnaan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState {
    Normal,
    Used,
    Join,
    Filter,
    Changed,
    Added,
    Aggregate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CardRow {
    /// Kunci pencocokan (nama kolom / id klausa).
    pub key: String,
    pub label: String,
    /// Teks lengkap untuk tooltip.
    pub detail: String,
    pub state: RowState,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: String,
    pub title: String,
    pub badge: String,
    pub role: CardRole,
    pub lane: usize,
    pub rect: Rect,
    pub rows: Vec<CardRow>,
}

impl Card {
    pub fn row_center_y(&self, row: usize) -> f32 {
        self.rect.min.y + HEADER_H + CARD_PAD + ROW_H * (row as f32 + 0.5)
    }

    /// Titik jangkar kiri/kanan baris; `None` = tengah header.
    pub fn anchor(&self, row: Option<usize>, right: bool) -> Pos2 {
        let y = match row {
            Some(r) => self.row_center_y(r),
            None => self.rect.min.y + HEADER_H * 0.5,
        };
        pos2(
            if right {
                self.rect.max.x
            } else {
                self.rect.min.x
            },
            y,
        )
    }

    fn row_of(&self, key: &str) -> Option<usize> {
        if let Some(i) = self
            .rows
            .iter()
            .position(|r| r.key.eq_ignore_ascii_case(key))
        {
            return Some(i);
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    Join,
    Data,
    Filter,
    /// Garis pipeline antar tahap (header ke header).
    Stage,
}

/// Garis alur antar baris kartu: (indeks kartu, baris; `None` = header).
#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub from: (usize, Option<usize>),
    pub to: (usize, Option<usize>),
    pub kind: FlowKind,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryLayout {
    pub kind: StatementKind,
    pub cards: Vec<Card>,
    pub flows: Vec<Flow>,
    pub bounds: Rect,
    /// Langkah-langkah penjelasan (English) sesuai urutan animasi.
    pub steps: Vec<String>,
    /// Peringatan penting (mis. UPDATE/DELETE tanpa WHERE).
    pub warning: Option<String>,
}

impl QueryLayout {
    pub fn card_index(&self, id: &str) -> Option<usize> {
        self.cards.iter().position(|c| c.id == id)
    }
}

fn card_width(title: &str, badge: &str, rows: &[CardRow]) -> f32 {
    let longest = rows
        .iter()
        .map(|r| r.label.chars().count())
        .chain(std::iter::once(
            title.chars().count() + badge.chars().count() + 4,
        ))
        .max()
        .unwrap_or(10);
    (longest as f32 * 6.8 + 48.0).clamp(MIN_CARD_W, MAX_CARD_W)
}

/// Semua kolom ditampilkan; kartu tanpa kolom diberi satu baris keterangan.
fn cap_rows(mut rows: Vec<CardRow>) -> Vec<CardRow> {
    if rows.is_empty() {
        rows.push(CardRow {
            key: String::new(),
            label: "(no columns referenced)".to_string(),
            detail: "The statement does not name any column of this table.".to_string(),
            state: RowState::Normal,
        });
    }
    rows
}

fn new_card(
    id: &str,
    title: String,
    badge: String,
    role: CardRole,
    lane: usize,
    rows: Vec<CardRow>,
) -> Card {
    let rows = cap_rows(rows);
    let w = card_width(&title, &badge, &rows);
    let h = HEADER_H + CARD_PAD * 2.0 + ROW_H * rows.len() as f32;
    Card {
        id: id.to_string(),
        title,
        badge,
        role,
        lane,
        rect: Rect::from_min_size(Pos2::ZERO, vec2(w, h)),
        rows,
    }
}

/// Id kartu non-tabel.
pub const CLAUSES_ID: &str = "__where";
pub const GROUP_ID: &str = "__group";
pub const HAVING_ID: &str = "__having";
pub const SORT_ID: &str = "__sort";
pub const TRANSFORM_ID: &str = "__transform";
pub const WINDOW_ID: &str = "__window";
pub const UNION_ID: &str = "__union";
pub const UPSERT_ID: &str = "__upsert";

/// Lane per tahap, kiri ke kanan sesuai urutan eksekusi SQL:
/// FROM → WHERE → GROUP BY → HAVING → ORDER BY/LIMIT → hasil → target.
const LANE_INNER: usize = 0;
const LANE_SOURCE: usize = 1;
const LANE_WHERE: usize = 2;
const LANE_GROUP: usize = 3;
const LANE_HAVING: usize = 4;
const LANE_WINDOW: usize = 5;
const LANE_UNION: usize = 6;
const LANE_SORT: usize = 7;
const LANE_TRANSFORM: usize = 8;
const LANE_TARGET: usize = 9;
const LANES: usize = 10;
/// Tabel sumber disusun berjenjang: tiap JOIN bergeser ke kanan bawah.
const STAIR_GAP_X: f32 = 64.0;
const STAIR_STEP_Y: f32 = 120.0;

/// Pecah kondisi di `AND` tingkat teratas (di luar kurung dan string).
/// `BETWEEN a AND b` tidak dipecah.
pub fn split_and(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let upper = text.to_ascii_uppercase();
    let ub = upper.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut between = false;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if let Some(q) = quote {
            if b == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match b {
            b'\'' | b'"' | b'`' => quote = Some(b),
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ if depth == 0 && ub[i..].starts_with(b" BETWEEN ") => between = true,
            _ if depth == 0 && ub[i..].starts_with(b" AND ") => {
                if between {
                    between = false;
                } else {
                    parts.push(text[start..i].trim().to_string());
                    start = i + 5;
                    i += 5;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let last = text[start..].trim();
    if !last.is_empty() {
        parts.push(last.to_string());
    }
    parts
}

fn condition_rows(prefix: &str, text: &str, state: RowState) -> Vec<CardRow> {
    split_and(text)
        .into_iter()
        .enumerate()
        .map(|(i, part)| CardRow {
            key: format!("{prefix}{i}"),
            label: if i == 0 {
                clip(&part, 40)
            } else {
                format!("AND {}", clip(&part, 36))
            },
            detail: part,
            state,
        })
        .collect()
}

/// Susun kartu dan alur dari model.
pub fn build_layout(model: &QueryDiagramModel) -> QueryLayout {
    let mut cards: Vec<Card> = Vec::new();
    let kind = model.kind;
    let is_join_col = |table: &str, col: &str| {
        model.joins.iter().any(|j| {
            (j.left.table == table && j.left.column.eq_ignore_ascii_case(col))
                || (j.right.table == table && j.right.column.eq_ignore_ascii_case(col))
        })
    };
    let is_filter_col = |table: &str, col: &str| {
        model
            .filter_columns
            .iter()
            .any(|r| r.table == table && r.column.eq_ignore_ascii_case(col))
    };

    // Tahap 0: tabel sumber. VALUES milik INSERT digambar sebagai kartu hasil.
    for t in &model.sources {
        if t.kind == SourceKind::Values && kind == StatementKind::Insert {
            continue;
        }
        let rows = t
            .columns
            .iter()
            .map(|c| CardRow {
                key: c.clone(),
                label: c.clone(),
                detail: format!("{}.{}", t.id, c),
                state: if is_join_col(&t.id, c) {
                    RowState::Join
                } else if is_filter_col(&t.id, c) {
                    RowState::Filter
                } else if t.is_used(c) {
                    RowState::Used
                } else {
                    RowState::Normal
                },
            })
            .collect();
        let badge = match t.kind {
            SourceKind::Cte => "CTE".to_string(),
            SourceKind::Subquery => "SUBQUERY".to_string(),
            SourceKind::Values => "VALUES".to_string(),
            SourceKind::Table => t
                .badge
                .clone()
                .or_else(|| t.join.clone())
                .unwrap_or_else(|| "FROM".to_string()),
        };
        let badge = match (&t.badge, t.kind) {
            (Some(b), SourceKind::Cte | SourceKind::Subquery) => format!("{badge} {b}"),
            _ => badge,
        };
        let lane = if t.feeds.is_some() {
            LANE_INNER
        } else {
            LANE_SOURCE
        };
        cards.push(new_card(
            &t.id,
            t.title(),
            badge,
            CardRole::Source,
            lane,
            rows,
        ));
    }

    // Tahap WHERE: menyaring baris.
    if let Some(f) = &model.filter {
        cards.push(new_card(
            CLAUSES_ID,
            "WHERE".to_string(),
            "FILTER ROWS".to_string(),
            CardRole::Clauses,
            LANE_WHERE,
            condition_rows("where", f, RowState::Filter),
        ));
    }

    // Tahap GROUP BY: kunci grup + agregat yang dihitung per grup. Agregat
    // tanpa GROUP BY berarti seluruh baris menjadi satu grup.
    let has_select_output = matches!(kind, StatementKind::Select | StatementKind::Insert);
    let aggregates: Vec<&super::OutputColumn> = if has_select_output {
        model.output.iter().filter(|o| o.aggregate).collect()
    } else {
        Vec::new()
    };
    if !model.group_by.is_empty() || !aggregates.is_empty() {
        let mut rows: Vec<CardRow> = model
            .group_by
            .iter()
            .enumerate()
            .map(|(i, g)| CardRow {
                key: format!("g{i}"),
                label: format!("key  {}", clip(g, 30)),
                detail: format!("Rows with the same {g} form one group."),
                state: RowState::Used,
            })
            .collect();
        if model.group_by.is_empty() {
            rows.push(CardRow {
                key: "g_all".to_string(),
                label: "(all rows form one group)".to_string(),
                detail: "There is no GROUP BY, so the aggregates summarize every row.".to_string(),
                state: RowState::Used,
            });
        }
        for o in &aggregates {
            rows.push(CardRow {
                key: format!("a:{}", o.name),
                label: format!("{} = {}", o.name, clip(&o.expr, 26)),
                detail: format!("{} = {} (computed once per group)", o.name, o.expr),
                state: RowState::Aggregate,
            });
        }
        cards.push(new_card(
            GROUP_ID,
            "GROUP BY".to_string(),
            "GROUP".to_string(),
            CardRole::Group,
            LANE_GROUP,
            rows,
        ));
    }

    // Tahap HAVING: menyaring grup.
    if let Some(h) = &model.having {
        cards.push(new_card(
            HAVING_ID,
            "HAVING".to_string(),
            "FILTER GROUPS".to_string(),
            CardRole::Having,
            LANE_HAVING,
            condition_rows("having", h, RowState::Filter),
        ));
    }

    // Tahap window function.
    let windows: Vec<&super::OutputColumn> = if has_select_output {
        model.output.iter().filter(|o| o.window.is_some()).collect()
    } else {
        Vec::new()
    };
    if !windows.is_empty() {
        let rows = windows
            .iter()
            .map(|o| CardRow {
                key: format!("w:{}", o.name),
                label: format!("{} = {}", o.name, clip(&o.expr, 26)),
                detail: format!(
                    "{} = {} (computed per row, looking at other rows of its window)",
                    o.name, o.expr
                ),
                state: RowState::Aggregate,
            })
            .collect();
        cards.push(new_card(
            WINDOW_ID,
            "WINDOW".to_string(),
            "OVER".to_string(),
            CardRole::Window,
            LANE_WINDOW,
            rows,
        ));
    }

    // Tahap UNION / INTERSECT / EXCEPT.
    if model.branches.len() > 1 {
        let rows = model
            .branches
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let tables = if b.tables.is_empty() {
                    "(no table)".to_string()
                } else {
                    b.tables.join(", ")
                };
                let prefix = if b.op.is_empty() {
                    String::new()
                } else {
                    format!("{} ", b.op)
                };
                CardRow {
                    key: format!("b{i}"),
                    label: format!("{prefix}query {}: {}", i + 1, clip(&tables, 28)),
                    detail: match &b.filter {
                        Some(f) => format!("{prefix}query {}: {tables} WHERE {f}", i + 1),
                        None => format!("{prefix}query {}: {tables}", i + 1),
                    },
                    state: RowState::Used,
                }
            })
            .collect();
        let title = model
            .branches
            .get(1)
            .map(|b| {
                b.op.split_whitespace()
                    .next()
                    .unwrap_or("UNION")
                    .to_string()
            })
            .unwrap_or_else(|| "UNION".to_string());
        cards.push(new_card(
            UNION_ID,
            title,
            "COMBINE".to_string(),
            CardRole::Union,
            LANE_UNION,
            rows,
        ));
    }

    // Tahap DISTINCT / ORDER BY / LIMIT.
    let mut sort_rows: Vec<CardRow> = Vec::new();
    let mut sort_title: Vec<&str> = Vec::new();
    if model.distinct {
        sort_title.push("DISTINCT");
        sort_rows.push(CardRow {
            key: "distinct".to_string(),
            label: "remove duplicate rows".to_string(),
            detail: "DISTINCT keeps one copy of identical rows.".to_string(),
            state: RowState::Normal,
        });
    }
    if !model.order_by.is_empty() {
        sort_title.push("ORDER BY");
        let text = model.order_by.join(", ");
        sort_rows.push(CardRow {
            key: "order".to_string(),
            label: format!("sort by {}", clip(&text, 30)),
            detail: format!("ORDER BY {text}"),
            state: RowState::Normal,
        });
    }
    if let Some(l) = &model.limit {
        sort_title.push("LIMIT");
        sort_rows.push(CardRow {
            key: "limit".to_string(),
            label: format!("keep first {}", clip(l, 24)),
            detail: format!("LIMIT {l}"),
            state: RowState::Normal,
        });
    }
    if !sort_rows.is_empty() {
        cards.push(new_card(
            SORT_ID,
            sort_title.join(" / "),
            "SORT".to_string(),
            CardRole::Sort,
            LANE_SORT,
            sort_rows,
        ));
    }

    // Tahap hasil / VALUES / SET.
    match kind {
        StatementKind::Select => {
            let rows = model
                .output
                .iter()
                .map(|o| CardRow {
                    key: o.name.clone(),
                    label: o.name.clone(),
                    detail: if o.expr == o.name {
                        o.expr.clone()
                    } else {
                        format!("{} = {}", o.name, o.expr)
                    },
                    state: if o.aggregate {
                        RowState::Aggregate
                    } else {
                        RowState::Added
                    },
                })
                .collect();
            cards.push(new_card(
                TRANSFORM_ID,
                "Result set".to_string(),
                "OUTPUT".to_string(),
                CardRole::Result,
                LANE_TRANSFORM,
                rows,
            ));
        }
        StatementKind::Insert => {
            let from_values = model.sources.iter().any(|s| s.kind == SourceKind::Values)
                || (model.sources.is_empty() && !model.mutations.is_empty());
            if from_values {
                let rows = model
                    .mutations
                    .iter()
                    .map(|m| CardRow {
                        key: m.column.clone(),
                        label: clip(&m.new_value, 30),
                        detail: format!("{} <- {}", m.column, m.new_value),
                        state: RowState::Added,
                    })
                    .collect();
                let badge = match model.values_rows {
                    0 => "SET".to_string(),
                    1 => "1 ROW".to_string(),
                    n => format!("{n} ROWS"),
                };
                cards.push(new_card(
                    TRANSFORM_ID,
                    "New values".to_string(),
                    badge,
                    CardRole::Values,
                    LANE_TRANSFORM,
                    rows,
                ));
            } else {
                let rows = model
                    .output
                    .iter()
                    .enumerate()
                    .map(|(i, o)| CardRow {
                        key: format!("#{i}"),
                        label: o.name.clone(),
                        detail: o.expr.clone(),
                        state: if o.aggregate {
                            RowState::Aggregate
                        } else {
                            RowState::Added
                        },
                    })
                    .collect();
                cards.push(new_card(
                    TRANSFORM_ID,
                    "SELECT result".to_string(),
                    "ROWS".to_string(),
                    CardRole::Result,
                    LANE_TRANSFORM,
                    rows,
                ));
            }
        }
        StatementKind::Update => {
            let rows = model
                .mutations
                .iter()
                .map(|m| CardRow {
                    key: m.column.clone(),
                    label: format!("{} = {}", m.column, clip(&m.new_value, 26)),
                    detail: format!("{} = {}", m.column, m.new_value),
                    state: RowState::Changed,
                })
                .collect();
            cards.push(new_card(
                TRANSFORM_ID,
                "SET".to_string(),
                "NEW VALUES".to_string(),
                CardRole::Set,
                LANE_TRANSFORM,
                rows,
            ));
        }
        StatementKind::Delete => {}
    }

    // Upsert: ON DUPLICATE KEY / ON CONFLICT / MERGE WHEN NOT MATCHED.
    if !model.upserts.is_empty() {
        let rows = model
            .upserts
            .iter()
            .map(|m| CardRow {
                key: m.column.clone(),
                label: format!("{} = {}", m.column, clip(&m.new_value, 26)),
                detail: format!("{} = {}", m.column, m.new_value),
                state: RowState::Changed,
            })
            .collect();
        let badge = if model.upsert_label.contains("NOT MATCHED") {
            "NOT MATCHED"
        } else {
            "IF EXISTS"
        };
        cards.push(new_card(
            UPSERT_ID,
            model.upsert_label.clone(),
            badge.to_string(),
            CardRole::Set,
            LANE_TRANSFORM,
            rows,
        ));
    }

    // Tahap target.
    if let Some(t) = &model.target {
        let upserted = |c: &str| {
            model
                .upserts
                .iter()
                .any(|m| m.column.eq_ignore_ascii_case(c))
        };
        let changed = |c: &str| {
            model
                .mutations
                .iter()
                .any(|m| m.column.eq_ignore_ascii_case(c))
        };
        // Kolom yang diubah/diisi didahulukan sesuai urutan SET/VALUES supaya
        // garis dari kartu tengah tidak saling silang.
        let mut ordered: Vec<&String> = model
            .mutations
            .iter()
            .filter_map(|m| t.columns.iter().find(|c| c.eq_ignore_ascii_case(&m.column)))
            .collect();
        ordered.dedup();
        for c in &t.columns {
            if !ordered.iter().any(|o| o.eq_ignore_ascii_case(c)) {
                ordered.push(c);
            }
        }
        let rows = ordered
            .into_iter()
            .map(|c| CardRow {
                key: c.clone(),
                label: c.clone(),
                detail: format!("{}.{}", t.id, c),
                state: match kind {
                    StatementKind::Update if changed(c) => RowState::Changed,
                    StatementKind::Insert if changed(c) => RowState::Added,
                    _ if upserted(c) => RowState::Changed,
                    _ if is_filter_col(&t.id, c) => RowState::Filter,
                    _ if is_join_col(&t.id, c) => RowState::Join,
                    _ => RowState::Normal,
                },
            })
            .collect();
        let badge = model.verb.clone().unwrap_or_else(|| {
            match kind {
                StatementKind::Insert => "INSERT INTO",
                StatementKind::Update => "UPDATE",
                StatementKind::Delete => "DELETE FROM",
                StatementKind::Select => "TARGET",
            }
            .to_string()
        });
        cards.push(new_card(
            &t.id,
            t.title(),
            badge,
            CardRole::Target,
            LANE_TARGET,
            rows,
        ));
    }

    position_cards(&mut cards);
    let flows = build_flows(model, &cards);
    move_stages_out_of_the_way(&mut cards, &flows);
    let mut bounds = cards
        .iter()
        .map(|c| c.rect)
        .reduce(|a, b| a.union(b))
        .unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0)));
    // Lengkung join antar tabel dalam satu lane keluar ke kiri kartu.
    if flows
        .iter()
        .any(|f| stacked(cards[f.from.0].rect, cards[f.to.0].rect))
    {
        bounds.min.x -= SAME_LANE_BEND + 40.0;
    }
    QueryLayout {
        kind,
        cards,
        flows,
        bounds,
        steps: describe(model),
        warning: warning(model),
    }
}

/// Dua kartu bertumpuk atas-bawah (tidak berjauhan secara horizontal), jadi
/// garis di antaranya melengkung lewat sisi kiri.
pub fn stacked(a: Rect, b: Rect) -> bool {
    !(b.min.x >= a.max.x + 16.0 || a.min.x >= b.max.x + 16.0)
}

/// Posisi relatif kartu dalam satu lane: lane sumber berjenjang (FROM di
/// kiri atas, tiap JOIN bergeser ke kanan bawah), lane lain ditumpuk.
/// Mengembalikan (offset per kartu, lebar lane, tinggi lane).
fn lane_offsets(cards: &[Card], lane: usize) -> (Vec<(usize, Vec2)>, f32, f32) {
    let members: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| c.lane == lane)
        .map(|(i, _)| i)
        .collect();
    let mut out = Vec::with_capacity(members.len());
    let (mut w, mut h) = (0.0f32, 0.0f32);
    if lane == LANE_SOURCE {
        let mut x = 0.0;
        for (k, &i) in members.iter().enumerate() {
            let size = cards[i].rect.size();
            let y = k as f32 * STAIR_STEP_Y;
            out.push((i, vec2(x, y)));
            w = x + size.x;
            h = h.max(y + size.y);
            x += size.x + STAIR_GAP_X;
        }
    } else {
        let width = members
            .iter()
            .map(|&i| cards[i].rect.width())
            .fold(0.0, f32::max);
        let mut y = 0.0;
        for &i in &members {
            let size = cards[i].rect.size();
            out.push((i, vec2((width - size.x) * 0.5, y)));
            y += size.y + CARD_GAP;
        }
        w = width;
        h = (y - CARD_GAP).max(0.0);
    }
    (out, w, h)
}

/// Tempatkan kartu per lane: lane kosong dilewati, tiap lane dipusatkan
/// vertikal terhadap lane tertinggi.
fn position_cards(cards: &mut [Card]) {
    let lanes: Vec<(Vec<(usize, Vec2)>, f32, f32)> =
        (0..LANES).map(|lane| lane_offsets(cards, lane)).collect();
    let tallest = lanes.iter().map(|(_, _, h)| *h).fold(0.0, f32::max);
    let mut x = 0.0;
    for (offsets, width, height) in lanes {
        if width == 0.0 {
            continue;
        }
        let top = (tallest - height) * 0.5;
        for (i, off) in offsets {
            let size = cards[i].rect.size();
            cards[i].rect = Rect::from_min_size(pos2(x + off.x, top + off.y), size);
        }
        x += width + LANE_GAP;
    }
}

/// Lengkung maksimum alur antar kartu dalam satu lane (lihat renderer).
pub const SAME_LANE_BEND: f32 = 90.0;

/// Kartu tahap (WHERE, HAVING, WINDOW, UNION, ORDER BY) yang dilompati garis data/join
/// dipindah ke jalur bawah supaya garis tidak menembusnya. Urutan tahap tetap
/// terbaca lewat garis pipeline antar kartu.
fn move_stages_out_of_the_way(cards: &mut [Card], flows: &[Flow]) {
    let crossed: Vec<usize> = cards
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            matches!(
                c.role,
                CardRole::Clauses
                    | CardRole::Having
                    | CardRole::Window
                    | CardRole::Union
                    | CardRole::Sort
            )
        })
        .filter(|(_, c)| {
            flows.iter().any(|f| {
                matches!(f.kind, FlowKind::Data | FlowKind::Join) && {
                    let (a, b) = (cards[f.from.0].lane, cards[f.to.0].lane);
                    a.min(b) < c.lane && a.max(b) > c.lane
                }
            })
        })
        .map(|(i, _)| i)
        .collect();
    if crossed.is_empty() {
        return;
    }
    let bottom = cards
        .iter()
        .enumerate()
        .filter(|(i, _)| !crossed.contains(i))
        .map(|(_, c)| c.rect.max.y)
        .fold(0.0, f32::max);
    for i in crossed {
        let size = cards[i].rect.size();
        cards[i].rect =
            Rect::from_min_size(pos2(cards[i].rect.min.x, bottom + CARD_GAP * 1.5), size);
    }
}

fn build_flows(model: &QueryDiagramModel, cards: &[Card]) -> Vec<Flow> {
    let idx = |id: &str| cards.iter().position(|c| c.id == id);
    let mut flows: Vec<Flow> = Vec::new();
    let push = |flows: &mut Vec<Flow>, f: Flow| {
        if f.from != f.to && !flows.iter().any(|x| x.from == f.from && x.to == f.to) {
            flows.push(f);
        }
    };
    let data = |from: (usize, Option<usize>), to: (usize, Option<usize>)| Flow {
        from,
        to,
        kind: FlowKind::Data,
        label: None,
    };
    let filter = |from: (usize, Option<usize>), to: (usize, Option<usize>)| Flow {
        from,
        to,
        kind: FlowKind::Filter,
        label: None,
    };
    let endpoint = |table: &str, column: &str| -> Option<(usize, Option<usize>)> {
        let ci = idx(table)?;
        if column == "*" {
            return Some((ci, None));
        }
        Some((ci, cards[ci].row_of(column)))
    };

    for j in &model.joins {
        if let (Some(a), Some(b)) = (
            endpoint(&j.left.table, &j.left.column),
            endpoint(&j.right.table, &j.right.column),
        ) {
            push(
                &mut flows,
                Flow {
                    from: a,
                    to: b,
                    kind: FlowKind::Join,
                    label: Some(j.join_type.clone()),
                },
            );
        }
    }

    let transform = idx(TRANSFORM_ID);
    let group = idx(GROUP_ID);
    let window = idx(WINDOW_ID);
    let target = model.target.as_ref().and_then(|t| idx(&t.id));
    let target_id = model.target.as_ref().map(|t| t.id.as_str());

    // Kolom hasil SELECT (juga sumber INSERT ... SELECT). Bila ada tahap
    // GROUP BY, data mengalir sumber → kunci/agregat grup → hasil.
    let select_output = transform.filter(|ti| cards[*ti].role == CardRole::Result);
    if let Some(ti) = select_output {
        let last = cards[ti].rows.len().saturating_sub(1);
        for (i, o) in model.output.iter().enumerate() {
            let row = if model.kind == StatementKind::Insert {
                cards[ti].row_of(&format!("#{i}"))
            } else {
                cards[ti].row_of(&o.name).or(Some(i.min(last)))
            };
            let to = (ti, row);
            let via_window = window
                .filter(|_| o.window.is_some())
                .map(|wi| (wi, cards[wi].row_of(&format!("w:{}", o.name))));
            let via = via_window.or_else(|| {
                group.and_then(|gi| {
                    if o.aggregate {
                        return Some((gi, cards[gi].row_of(&format!("a:{}", o.name))));
                    }
                    let k = model
                        .group_columns
                        .iter()
                        .enumerate()
                        .position(|(k, refs)| {
                            refs.iter().any(|r| o.sources.contains(r))
                                || model.group_by[k].eq_ignore_ascii_case(&o.expr)
                        })?;
                    Some((gi, cards[gi].row_of(&format!("g{k}"))))
                })
            });
            for s in &o.sources {
                if let Some(from) = endpoint(&s.table, &s.column) {
                    push(&mut flows, data(from, via.unwrap_or(to)));
                }
            }
            if let Some(v) = via {
                push(&mut flows, data(v, to));
            }
        }
    }
    // Kolom kunci GROUP BY yang tidak ikut di-SELECT tetap tersambung.
    if let Some(gi) = group {
        for (k, refs) in model.group_columns.iter().enumerate() {
            let key_row = cards[gi].row_of(&format!("g{k}"));
            for r in refs {
                if let Some(from) = endpoint(&r.table, &r.column) {
                    push(&mut flows, data(from, (gi, key_row)));
                }
            }
        }
    }

    match model.kind {
        StatementKind::Insert => {
            if let (Some(ti), Some(tg)) = (transform, target) {
                let from_select = cards[ti].role == CardRole::Result;
                for (i, m) in model.mutations.iter().enumerate() {
                    let from_row = if from_select {
                        cards[ti].row_of(&format!("#{i}"))
                    } else {
                        cards[ti].row_of(&m.column)
                    };
                    let to_row = cards[tg].row_of(&m.column);
                    push(&mut flows, data((ti, from_row), (tg, to_row)));
                }
            }
        }
        StatementKind::Update => {
            if let (Some(ti), Some(tg)) = (transform, target) {
                for m in &model.mutations {
                    let set_row = cards[ti].row_of(&m.column);
                    for s in &m.sources {
                        if Some(s.table.as_str()) == target_id {
                            continue;
                        }
                        if let Some(from) = endpoint(&s.table, &s.column) {
                            push(&mut flows, data(from, (ti, set_row)));
                        }
                    }
                    push(
                        &mut flows,
                        data((ti, set_row), (tg, cards[tg].row_of(&m.column))),
                    );
                }
            }
        }
        StatementKind::Select | StatementKind::Delete => {}
    }

    // Kolom tabel di dalam CTE/subquery mengisi kolom kartu CTE/subquery.
    for (from, to) in &model.derived_links {
        if let (Some(a), Some(b)) = (
            endpoint(&from.table, &from.column),
            endpoint(&to.table, &to.column),
        ) {
            push(&mut flows, data(a, b));
        }
    }

    // Upsert: sumber → kartu upsert → kolom target.
    if let (Some(ui), Some(tg)) = (idx(UPSERT_ID), target) {
        for m in &model.upserts {
            let row = cards[ui].row_of(&m.column);
            for s in &m.sources {
                if Some(s.table.as_str()) == target_id {
                    continue;
                }
                if let Some(from) = endpoint(&s.table, &s.column) {
                    push(&mut flows, data(from, (ui, row)));
                }
            }
            push(
                &mut flows,
                data((ui, row), (tg, cards[tg].row_of(&m.column))),
            );
        }
    }

    // Kolom yang disaring terhubung ke kondisi WHERE yang memakainya.
    if let Some(ci) = idx(CLAUSES_ID) {
        for r in &model.filter_columns {
            let Some(ep) = endpoint(&r.table, &r.column) else {
                continue;
            };
            let col = r.column.to_lowercase();
            let row = cards[ci]
                .rows
                .iter()
                .position(|row| row.detail.to_lowercase().contains(&col))
                .unwrap_or(0);
            let clause = (ci, Some(row));
            let (from, to) = if cards[ep.0].lane < cards[ci].lane {
                (ep, clause)
            } else {
                (clause, ep)
            };
            push(&mut flows, filter(from, to));
        }
    }

    // Kunci/agregat grup yang dipakai HAVING terhubung ke kondisinya.
    if let (Some(hi), Some(gi)) = (idx(HAVING_ID), group) {
        for (hr, cond) in cards[hi].rows.iter().enumerate() {
            let cond_up = cond.detail.to_uppercase();
            for (gr, grow) in cards[gi].rows.iter().enumerate() {
                let used = if let Some(name) = grow.key.strip_prefix("a:") {
                    let expr = model
                        .output
                        .iter()
                        .find(|o| o.name == name)
                        .map(|o| o.expr.to_uppercase())
                        .unwrap_or_default();
                    (!expr.is_empty() && cond_up.contains(&expr))
                        || cond_up
                            .split(|c: char| !c.is_alphanumeric() && c != '_')
                            .any(|w| w.eq_ignore_ascii_case(name))
                } else if let Some(k) = grow
                    .key
                    .strip_prefix('g')
                    .and_then(|k| k.parse::<usize>().ok())
                {
                    model
                        .group_columns
                        .get(k)
                        .is_some_and(|refs| refs.iter().any(|r| model.having_columns.contains(r)))
                } else {
                    false
                };
                if used {
                    push(&mut flows, filter((gi, Some(gr)), (hi, Some(hr))));
                }
            }
        }
    }

    // Garis pipeline antar tahap: sumber → WHERE → GROUP BY → HAVING →
    // ORDER BY/LIMIT → hasil (atau target bila tidak ada kartu hasil).
    let stages: Vec<usize> = [
        CLAUSES_ID, GROUP_ID, HAVING_ID, WINDOW_ID, UNION_ID, SORT_ID,
    ]
    .iter()
    .filter_map(|id| idx(id))
    .collect();
    if !stages.is_empty() {
        let mut chain = stages.clone();
        if let Some(end) = transform.or(target) {
            chain.push(end);
        }
        let stage_label = |ci: usize| match cards[ci].role {
            CardRole::Clauses => "matching rows",
            CardRole::Group => "groups",
            CardRole::Having => "kept groups",
            CardRole::Window => "rows + window values",
            CardRole::Union => "combined rows",
            _ => "ordered rows",
        };
        // Hasil gabungan FROM + JOIN satu cabang adalah satu kumpulan baris:
        // satu garis pipeline dari tabel terakhir di tangga. Tabel subquery
        // IN/EXISTS/skalar sudah terwakili lewat garis relasinya.
        let union = idx(UNION_ID);
        let is_subquery_card = |id: &str| {
            model
                .table(id)
                .and_then(|t| t.badge.as_deref())
                .is_some_and(|b| {
                    b.contains("subquery") || b.contains("EXISTS") || b == "SCALAR SUBQUERY"
                })
        };
        let branches = model.branches.len().max(1);
        for branch in 0..branches {
            let members: Vec<usize> = cards
                .iter()
                .enumerate()
                .filter(|(_, c)| c.role == CardRole::Source && c.lane == LANE_SOURCE)
                .filter(|(_, c)| {
                    model.table(&c.id).is_some_and(|t| t.branch == branch)
                        && !is_subquery_card(&c.id)
                })
                .map(|(i, _)| i)
                .collect();
            let Some(&last) = members.last() else {
                continue;
            };
            let to = match union {
                Some(u) if branch > 0 => u,
                _ => chain[0],
            };
            let label = if members.len() > 1 {
                "joined rows"
            } else {
                "rows"
            };
            push(
                &mut flows,
                Flow {
                    from: (last, None),
                    to: (to, None),
                    kind: FlowKind::Stage,
                    label: Some(label.to_string()),
                },
            );
        }
        for pair in chain.windows(2) {
            push(
                &mut flows,
                Flow {
                    from: (pair[0], None),
                    to: (pair[1], None),
                    kind: FlowKind::Stage,
                    label: Some(stage_label(pair[0]).to_string()),
                },
            );
        }
    }
    flows
}

fn table_list(model: &QueryDiagramModel) -> String {
    let names: Vec<String> = model
        .sources
        .iter()
        .filter(|s| s.kind != SourceKind::Values)
        .map(|s| s.title())
        .collect();
    names.join(", ")
}

/// Penjelasan langkah demi langkah (English, tampil di UI).
pub fn describe(model: &QueryDiagramModel) -> Vec<String> {
    let mut steps: Vec<String> = Vec::new();
    let sources = table_list(model);
    let target = model.target.as_ref().map(|t| t.title()).unwrap_or_default();
    match model.kind {
        StatementKind::Select => {
            if sources.is_empty() {
                steps.push("Compute values without reading a table.".to_string());
            } else {
                steps.push(format!("Read rows from {sources}."));
            }
        }
        StatementKind::Insert => {}
        StatementKind::Update | StatementKind::Delete => {
            steps.push(format!("Look up rows in {target}."));
        }
    }
    for j in &model.joins {
        steps.push(format!(
            "Match {}.{} with {}.{} ({}).",
            j.right.table, j.right.column, j.left.table, j.left.column, j.join_type
        ));
    }
    if let Some(f) = &model.filter {
        steps.push(format!("Keep only rows where {}.", clip(f, 90)));
    }
    if !model.group_by.is_empty() {
        let aggs: Vec<&str> = model
            .output
            .iter()
            .filter(|o| o.aggregate)
            .map(|o| o.name.as_str())
            .collect();
        if aggs.is_empty() {
            steps.push(format!("Group rows by {}.", model.group_by.join(", ")));
        } else {
            steps.push(format!(
                "Group rows by {} and compute {}.",
                model.group_by.join(", "),
                aggs.join(", ")
            ));
        }
    }
    if let Some(h) = &model.having {
        steps.push(format!("Keep only groups where {}.", clip(h, 80)));
    }
    if model.distinct {
        steps.push("Remove duplicate rows.".to_string());
    }
    if !model.order_by.is_empty() {
        steps.push(format!("Sort by {}.", model.order_by.join(", ")));
    }
    if let Some(l) = &model.limit {
        steps.push(format!("Return at most {l} rows."));
    }
    match model.kind {
        StatementKind::Select => {
            let names: Vec<&str> = model.output.iter().map(|o| o.name.as_str()).collect();
            steps.push(format!(
                "Output {} column(s): {}.",
                names.len(),
                clip(&names.join(", "), 120)
            ));
        }
        StatementKind::Insert => {
            let cols: Vec<&str> = model.mutations.iter().map(|m| m.column.as_str()).collect();
            let from_select = model.sources.iter().any(|s| s.kind != SourceKind::Values);
            let what = match model.values_rows {
                _ if from_select => format!("the rows selected from {sources}"),
                0 | 1 => "1 new row".to_string(),
                n => format!("{n} new rows"),
            };
            steps.insert(
                0,
                format!(
                    "Insert {what} into {target} ({}).",
                    clip(&cols.join(", "), 100)
                ),
            );
        }
        StatementKind::Update => {
            for m in &model.mutations {
                let how = if m.is_static {
                    "a fixed value".to_string()
                } else {
                    let from: Vec<String> = m
                        .sources
                        .iter()
                        .map(|s| format!("{}.{}", s.table, s.column))
                        .collect();
                    format!("a value computed from {}", from.join(", "))
                };
                steps.push(format!(
                    "Set {} to {} ({how}).",
                    m.column,
                    clip(&m.new_value, 60)
                ));
            }
        }
        StatementKind::Delete => steps.push(format!("Remove the matching rows from {target}.")),
    }
    for t in &model.sources {
        match t.badge.as_deref() {
            Some("IN (subquery)") => steps.push(format!(
                "Keep rows whose value appears in {} (IN subquery).",
                t.title()
            )),
            Some("NOT IN (subquery)") => steps.push(format!(
                "Drop rows whose value appears in {} (NOT IN subquery).",
                t.title()
            )),
            Some("EXISTS") => steps.push(format!(
                "Keep rows that have a matching row in {} (EXISTS).",
                t.title()
            )),
            Some("NOT EXISTS") => steps.push(format!(
                "Keep rows that have no matching row in {} (NOT EXISTS).",
                t.title()
            )),
            Some("SCALAR SUBQUERY") => steps.push(format!(
                "Look up one value per row from {} (scalar subquery).",
                t.title()
            )),
            _ => {}
        }
    }
    let windows: Vec<&str> = model
        .output
        .iter()
        .filter(|o| o.window.is_some())
        .map(|o| o.name.as_str())
        .collect();
    if !windows.is_empty() {
        steps.push(format!(
            "Compute window values {} per row without collapsing rows.",
            windows.join(", ")
        ));
    }
    for b in model.branches.iter().skip(1) {
        steps.push(format!(
            "Combine with rows from {} ({}).",
            b.tables.join(", "),
            b.op
        ));
    }
    if !model.upserts.is_empty() {
        let cols: Vec<&str> = model.upserts.iter().map(|m| m.column.as_str()).collect();
        steps.push(format!("{}: set {}.", model.upsert_label, cols.join(", ")));
    }
    for n in &model.notes {
        steps.push(n.clone());
    }
    steps
}

/// Peringatan untuk statement yang berisiko.
pub fn warning(model: &QueryDiagramModel) -> Option<String> {
    match model.kind {
        StatementKind::Update | StatementKind::Delete if model.filter.is_none() => Some(format!(
            "No WHERE clause: this {} affects every row in {}.",
            model.kind.label(),
            model
                .target
                .as_ref()
                .map(|t| t.table.as_str())
                .unwrap_or("the table")
        )),
        _ => None,
    }
}

#[cfg(test)]
#[cfg(feature = "query_ast")]
mod tests {
    use super::*;
    use crate::models::enums::DatabaseType;
    use crate::query_diagram::analyze_statement;

    fn layout(sql: &str) -> QueryLayout {
        build_layout(&analyze_statement(sql, &DatabaseType::MySQL).unwrap())
    }

    fn card<'a>(l: &'a QueryLayout, id: &str) -> &'a Card {
        &l.cards[l.card_index(id).unwrap()]
    }

    fn stage_labels(l: &QueryLayout) -> Vec<String> {
        l.flows
            .iter()
            .filter(|f| f.kind == FlowKind::Stage)
            .filter_map(|f| f.label.clone())
            .collect()
    }

    #[test]
    fn test_split_and() {
        assert_eq!(
            split_and("a = 1 AND (b = 2 AND c = 3) AND d BETWEEN 1 AND 5 AND e = 'x AND y'"),
            vec![
                "a = 1",
                "(b = 2 AND c = 3)",
                "d BETWEEN 1 AND 5",
                "e = 'x AND y'"
            ]
        );
        assert_eq!(split_and("x > 1"), vec!["x > 1"]);
    }

    #[test]
    fn test_select_without_group_goes_through_where() {
        let l = layout(
            "SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id WHERE o.total > 5 AND u.active = 1",
        );
        let src = card(&l, "u");
        let wh = card(&l, CLAUSES_ID);
        let out = card(&l, TRANSFORM_ID);
        assert!(src.rect.max.x < wh.rect.min.x && wh.rect.max.x < out.rect.min.x);
        // Garis data melompati WHERE, jadi WHERE turun ke jalur bawah.
        assert!(wh.rect.min.y > src.rect.max.y.max(out.rect.max.y));
        // Satu baris per kondisi AND.
        assert_eq!(wh.rows.len(), 2);
        assert_eq!(
            l.flows.iter().filter(|f| f.kind == FlowKind::Join).count(),
            1
        );
        assert_eq!(
            l.flows.iter().filter(|f| f.kind == FlowKind::Data).count(),
            2
        );
        assert!(l.flows.iter().any(|f| f.kind == FlowKind::Filter));
        // Pipeline: hasil join → WHERE → hasil.
        assert_eq!(
            l.flows.iter().filter(|f| f.kind == FlowKind::Stage).count(),
            2
        );
        assert_eq!(stage_labels(&l), vec!["joined rows", "matching rows"]);
        // Tabel sumber berjenjang: JOIN di kanan dan lebih bawah dari FROM,
        // jadi relasinya ditarik sisi ke sisi tanpa lengkung kiri.
        let joined = card(&l, "o");
        assert!(joined.rect.min.x > src.rect.max.x);
        assert!(joined.rect.min.y > src.rect.min.y);
        assert!(!stacked(src.rect, joined.rect));
    }

    #[test]
    fn test_group_by_and_having_are_separate_stages() {
        let l = layout(
            "SELECT u.name, COUNT(o.id) AS orders FROM users u JOIN orders o ON o.user_id = u.id \
             WHERE u.active = 1 GROUP BY u.name HAVING COUNT(o.id) > 5 ORDER BY orders DESC LIMIT 10",
        );
        let wh = card(&l, CLAUSES_ID);
        let gr = card(&l, GROUP_ID);
        let hv = card(&l, HAVING_ID);
        let so = card(&l, SORT_ID);
        let out = card(&l, TRANSFORM_ID);
        assert_eq!(gr.role, CardRole::Group);
        assert_eq!(hv.role, CardRole::Having);
        // Urutan tahap kiri ke kanan.
        assert!(wh.rect.min.x < gr.rect.min.x);
        assert!(gr.rect.min.x < hv.rect.min.x);
        assert!(hv.rect.min.x < so.rect.min.x);
        assert!(so.rect.min.x < out.rect.min.x);
        // GROUP BY tetap di jalur utama (dilewati data), tahap filter turun.
        assert!(gr.rect.min.y < wh.rect.min.y);
        assert!(gr.rect.min.y < hv.rect.min.y);
        // Kunci + agregat.
        let keys: Vec<&str> = gr.rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["g0", "a:orders"]);
        // Semua data yang masuk ke hasil datang dari kartu GROUP BY.
        let gi = l.card_index(GROUP_ID).unwrap();
        let ti = l.card_index(TRANSFORM_ID).unwrap();
        assert!(
            l.flows
                .iter()
                .filter(|f| f.kind == FlowKind::Data && f.to.0 == ti)
                .all(|f| f.from.0 == gi)
        );
        assert_eq!(
            l.flows
                .iter()
                .filter(|f| f.kind == FlowKind::Data && f.to.0 == ti)
                .count(),
            2
        );
        // HAVING memakai agregat COUNT(o.id).
        let hi = l.card_index(HAVING_ID).unwrap();
        assert!(
            l.flows
                .iter()
                .any(|f| f.kind == FlowKind::Filter && f.from == (gi, Some(1)) && f.to.0 == hi)
        );
        assert_eq!(
            stage_labels(&l),
            vec![
                "joined rows",
                "matching rows",
                "groups",
                "kept groups",
                "ordered rows"
            ]
        );
    }

    #[test]
    fn test_aggregate_without_group_by_is_one_group() {
        let l = layout("SELECT COUNT(*) AS n FROM t");
        let gr = card(&l, GROUP_ID);
        assert_eq!(gr.rows[0].key, "g_all");
        assert_eq!(gr.rows[1].key, "a:n");
    }

    #[test]
    fn test_update_flows_through_set_card() {
        let l = layout(
            "UPDATE orders o JOIN users u ON u.id = o.user_id SET o.email = u.email, o.flag = 1 WHERE u.active = 1",
        );
        let set = l.card_index(TRANSFORM_ID).unwrap();
        let target = l.card_index("o").unwrap();
        assert_eq!(l.cards[set].role, CardRole::Set);
        assert_eq!(l.cards[target].role, CardRole::Target);
        // Kolom yang diubah tampil paling atas sesuai urutan SET.
        assert_eq!(l.cards[target].rows[0].key, "email");
        assert_eq!(l.cards[target].rows[1].key, "flag");
        assert_eq!(l.cards[target].rows[0].state, RowState::Changed);
        let into_target = l
            .flows
            .iter()
            .filter(|f| f.to.0 == target && f.kind == FlowKind::Data)
            .count();
        assert_eq!(into_target, 2);
        let u = l.card_index("u").unwrap();
        assert!(l.flows.iter().any(|f| f.to.0 == set && f.from.0 == u));
        assert!(l.warning.is_none());
    }

    #[test]
    fn test_delete_without_where_warns() {
        let l = layout("DELETE FROM logs");
        assert!(l.warning.as_deref().unwrap_or("").contains("every row"));
        assert_eq!(l.cards.len(), 1);
        assert_eq!(l.cards[0].role, CardRole::Target);
    }

    #[test]
    fn test_insert_values_card() {
        let l = layout("INSERT INTO users (name, email) VALUES ('a', 'b')");
        let v = l.card_index(TRANSFORM_ID).unwrap();
        assert_eq!(l.cards[v].role, CardRole::Values);
        assert_eq!(l.cards[v].badge, "1 ROW");
        assert_eq!(l.flows.len(), 2);
        assert!(l.steps[0].starts_with("Insert 1 new row into users"));
    }

    #[test]
    fn test_all_columns_are_shown() {
        let cols: Vec<String> = (0..40).map(|i| format!("c{i}")).collect();
        let sql = format!("SELECT {} FROM t", cols.join(", "));
        let l = layout(&sql);
        assert_eq!(card(&l, "t").rows.len(), 40);
        assert_eq!(l.flows.len(), 40);
    }

    #[test]
    fn test_schema_columns_shown_and_unused_are_dimmed() {
        use crate::query_diagram::analyze_with_schema;
        let lookup = |t: &str| {
            (t == "users").then(|| {
                ["id", "email", "name", "created_at"]
                    .map(String::from)
                    .to_vec()
            })
        };
        let m = analyze_with_schema("SELECT u.email FROM users u", &DatabaseType::MySQL, &lookup)
            .unwrap();
        let l = build_layout(&m);
        let u = card(&l, "u");
        let keys: Vec<&str> = u.rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["id", "email", "name", "created_at"]);
        assert_eq!(u.rows[1].state, RowState::Used);
        assert_eq!(u.rows[0].state, RowState::Normal);
    }

    #[test]
    fn test_window_union_upsert_and_verbs() {
        let w = layout("SELECT name, SUM(amount) OVER (PARTITION BY dept) AS s FROM emp");
        assert!(w.card_index(GROUP_ID).is_none());
        let wi = w.card_index(WINDOW_ID).expect("kartu WINDOW");
        let ti = w.card_index(TRANSFORM_ID).unwrap();
        assert!(w.flows.iter().any(|f| f.from.0 == wi && f.to.0 == ti));

        let u = layout("SELECT id, name FROM users UNION ALL SELECT id, name FROM admins");
        let ui = u.card_index(UNION_ID).expect("kartu UNION");
        assert_eq!(u.cards[ui].rows.len(), 2);
        let admins = u.card_index("admins").unwrap();
        // Cabang ke-2 masuk ke kartu UNION lewat pipeline.
        assert!(
            u.flows
                .iter()
                .any(|f| f.kind == FlowKind::Stage && f.from.0 == admins && f.to.0 == ui)
        );

        let up =
            layout("INSERT INTO t (id, n) VALUES (1, 'a') ON DUPLICATE KEY UPDATE n = VALUES(n)");
        let upi = up.card_index(UPSERT_ID).expect("kartu upsert");
        assert_eq!(up.cards[upi].title, "ON DUPLICATE KEY UPDATE");
        let tg = up.card_index("t").unwrap();
        assert!(up.flows.iter().any(|f| f.from.0 == upi && f.to.0 == tg));

        let tr = layout("TRUNCATE TABLE logs");
        assert_eq!(card(&tr, "logs").badge, "TRUNCATE");
    }

    #[test]
    fn test_cte_tables_sit_left_of_the_cte() {
        let l = layout("WITH recent AS (SELECT o.id FROM orders o) SELECT r.id FROM recent r");
        let inner = card(&l, "o");
        let cte = card(&l, "r");
        assert!(inner.rect.max.x < cte.rect.min.x);
        let (oi, ri) = (l.card_index("o").unwrap(), l.card_index("r").unwrap());
        assert!(
            l.flows
                .iter()
                .any(|f| f.kind == FlowKind::Data && f.from.0 == oi && f.to.0 == ri)
        );
    }
}
