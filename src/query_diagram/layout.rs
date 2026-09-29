//! Tata letak kartu dan alur untuk diagram query (koordinat diagram, tanpa
//! egui painter). Kolom kiri ke kanan: sumber → klausa → hasil/SET → target.

use eframe::egui::{Pos2, Rect, pos2, vec2};

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
    Clauses,
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
pub const CLAUSES_ID: &str = "__clauses";
pub const TRANSFORM_ID: &str = "__transform";

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

    // Lane 0: tabel sumber. VALUES milik INSERT digambar sebagai kartu hasil.
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
            SourceKind::Table => t.join.clone().unwrap_or_else(|| "FROM".to_string()),
        };
        cards.push(new_card(&t.id, t.title(), badge, CardRole::Source, 0, rows));
    }

    // Lane 1: klausa.
    let mut clause_rows: Vec<CardRow> = Vec::new();
    let mut clause = |key: &str, label: &str, text: String, state: RowState| {
        clause_rows.push(CardRow {
            key: key.to_string(),
            label: format!("{label}  {}", clip(&text, 34)),
            detail: format!("{label} {text}"),
            state,
        });
    };
    if model.distinct {
        clause(
            "distinct",
            "DISTINCT",
            "remove duplicate rows".to_string(),
            RowState::Normal,
        );
    }
    if let Some(f) = &model.filter {
        clause("where", "WHERE", f.clone(), RowState::Filter);
    }
    if !model.group_by.is_empty() {
        clause(
            "group",
            "GROUP BY",
            model.group_by.join(", "),
            RowState::Aggregate,
        );
    }
    if let Some(h) = &model.having {
        clause("having", "HAVING", h.clone(), RowState::Filter);
    }
    if !model.order_by.is_empty() {
        clause(
            "order",
            "ORDER BY",
            model.order_by.join(", "),
            RowState::Normal,
        );
    }
    if let Some(l) = &model.limit {
        clause("limit", "LIMIT", l.clone(), RowState::Normal);
    }
    if !clause_rows.is_empty() {
        cards.push(new_card(
            CLAUSES_ID,
            "Conditions".to_string(),
            "FILTER".to_string(),
            CardRole::Clauses,
            1,
            clause_rows,
        ));
    }

    // Lane 2: hasil / VALUES / SET.
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
                2,
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
                    2,
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
                    2,
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
                2,
                rows,
            ));
        }
        StatementKind::Delete => {}
    }

    // Lane 3: target.
    if let Some(t) = &model.target {
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
                    _ if is_filter_col(&t.id, c) => RowState::Filter,
                    _ if is_join_col(&t.id, c) => RowState::Join,
                    _ => RowState::Normal,
                },
            })
            .collect();
        let badge = match kind {
            StatementKind::Insert => "INSERT INTO",
            StatementKind::Update => "UPDATE",
            StatementKind::Delete => "DELETE FROM",
            StatementKind::Select => "TARGET",
        };
        cards.push(new_card(
            &t.id,
            t.title(),
            badge.to_string(),
            CardRole::Target,
            3,
            rows,
        ));
    }

    position_cards(&mut cards);
    let flows = build_flows(model, &cards);
    move_clauses_out_of_the_way(&mut cards, &flows);
    let mut bounds = cards
        .iter()
        .map(|c| c.rect)
        .reduce(|a, b| a.union(b))
        .unwrap_or(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0)));
    // Lengkung join antar tabel dalam satu lane keluar ke kiri kartu.
    if flows
        .iter()
        .any(|f| cards[f.from.0].lane == cards[f.to.0].lane)
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

/// Tempatkan kartu per lane: lane kosong dilewati, tiap lane ditumpuk
/// vertikal dan dipusatkan terhadap lane tertinggi.
fn position_cards(cards: &mut [Card]) {
    let mut x = 0.0;
    let heights: Vec<(usize, f32)> = (0..4)
        .map(|lane| {
            let h: f32 = cards
                .iter()
                .filter(|c| c.lane == lane)
                .map(|c| c.rect.height() + CARD_GAP)
                .sum();
            (lane, (h - CARD_GAP).max(0.0))
        })
        .collect();
    let tallest = heights.iter().map(|(_, h)| *h).fold(0.0, f32::max);
    for (lane, lane_h) in heights {
        let width = cards
            .iter()
            .filter(|c| c.lane == lane)
            .map(|c| c.rect.width())
            .fold(0.0, f32::max);
        if width == 0.0 {
            continue;
        }
        let mut y = (tallest - lane_h) * 0.5;
        for c in cards.iter_mut().filter(|c| c.lane == lane) {
            let size = c.rect.size();
            c.rect = Rect::from_min_size(pos2(x + (width - size.x) * 0.5, y), size);
            y += size.y + CARD_GAP;
        }
        x += width + LANE_GAP;
    }
}

/// Lengkung maksimum alur antar kartu dalam satu lane (lihat renderer).
pub const SAME_LANE_BEND: f32 = 90.0;

/// Bila ada alur yang melompati lane klausa (sumber ke hasil/SET/target),
/// kartu klausa dipindah ke bawah supaya garis tidak menembusnya.
fn move_clauses_out_of_the_way(cards: &mut [Card], flows: &[Flow]) {
    let Some(ci) = cards.iter().position(|c| c.role == CardRole::Clauses) else {
        return;
    };
    let lane = cards[ci].lane;
    let crosses = flows.iter().any(|f| {
        f.kind != FlowKind::Filter && {
            let (a, b) = (cards[f.from.0].lane, cards[f.to.0].lane);
            a.min(b) < lane && a.max(b) > lane
        }
    });
    if !crosses {
        return;
    }
    let bottom = cards
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != ci)
        .map(|(_, c)| c.rect.max.y)
        .fold(0.0, f32::max);
    let size = cards[ci].rect.size();
    cards[ci].rect = Rect::from_min_size(pos2(cards[ci].rect.min.x, bottom + CARD_GAP * 1.5), size);
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
    let target = model.target.as_ref().and_then(|t| idx(&t.id));
    let target_id = model.target.as_ref().map(|t| t.id.as_str());

    match model.kind {
        StatementKind::Select => {
            if let Some(ti) = transform {
                let last = cards[ti].rows.len().saturating_sub(1);
                for (i, o) in model.output.iter().enumerate() {
                    let row = cards[ti].row_of(&o.name).or(Some(i.min(last)));
                    for s in &o.sources {
                        if let Some(from) = endpoint(&s.table, &s.column) {
                            push(&mut flows, data(from, (ti, row)));
                        }
                    }
                }
            }
        }
        StatementKind::Insert => {
            if let (Some(ti), Some(tg)) = (transform, target) {
                let from_select = cards[ti].role == CardRole::Result;
                if from_select {
                    for (i, o) in model.output.iter().enumerate() {
                        let row = cards[ti].row_of(&format!("#{i}"));
                        for s in &o.sources {
                            if let Some(from) = endpoint(&s.table, &s.column) {
                                push(&mut flows, data(from, (ti, row)));
                            }
                        }
                    }
                }
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
        StatementKind::Delete => {}
    }

    // Kolom yang disaring terhubung ke baris WHERE.
    if let Some(ci) = idx(CLAUSES_ID)
        && let Some(where_row) = cards[ci].rows.iter().position(|r| r.key == "where")
    {
        for r in &model.filter_columns {
            if let Some(ep) = endpoint(&r.table, &r.column) {
                let clause = (ci, Some(where_row));
                let (from, to) = if cards[ep.0].lane < cards[ci].lane {
                    (ep, clause)
                } else {
                    (clause, ep)
                };
                push(
                    &mut flows,
                    Flow {
                        from,
                        to,
                        kind: FlowKind::Filter,
                        label: None,
                    },
                );
            }
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

    #[test]
    fn test_select_lanes_left_to_right() {
        let l = layout(
            "SELECT u.name, o.total FROM users u JOIN orders o ON o.user_id = u.id WHERE o.total > 5",
        );
        let src = &l.cards[l.card_index("u").unwrap()];
        let clauses = &l.cards[l.card_index(CLAUSES_ID).unwrap()];
        let out = &l.cards[l.card_index(TRANSFORM_ID).unwrap()];
        assert!(src.rect.max.x < clauses.rect.min.x);
        assert!(clauses.rect.max.x < out.rect.min.x);
        // Alur data melompati lane klausa, jadi kartu klausa turun ke bawah.
        assert!(clauses.rect.min.y > src.rect.max.y.max(out.rect.max.y));
        // Ruang kiri untuk lengkung join antar tabel sumber.
        assert!(l.bounds.min.x < src.rect.min.x - SAME_LANE_BEND);
        assert_eq!(out.role, CardRole::Result);
        assert_eq!(
            l.flows.iter().filter(|f| f.kind == FlowKind::Join).count(),
            1
        );
        assert_eq!(
            l.flows.iter().filter(|f| f.kind == FlowKind::Data).count(),
            2
        );
        assert!(l.flows.iter().any(|f| f.kind == FlowKind::Filter));
        assert!(l.bounds.contains_rect(out.rect));
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
        let email_row = l.cards[target]
            .rows
            .iter()
            .position(|r| r.key == "email")
            .unwrap();
        assert_eq!(l.cards[target].rows[email_row].state, RowState::Changed);
        // u.email → SET, SET → target (email, flag).
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
        let t = &l.cards[l.card_index("t").unwrap()];
        assert_eq!(t.rows.len(), 40);
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
        let u = &l.cards[l.card_index("u").unwrap()];
        let keys: Vec<&str> = u.rows.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["id", "email", "name", "created_at"]);
        assert_eq!(u.rows[1].state, RowState::Used);
        assert_eq!(u.rows[0].state, RowState::Normal);
    }
}
