//! Model "API rail": indeks endpoint diagram yang tampil di panel kiri kanvas.
//!
//! Endpoint dikelompokkan per repository lalu per tabel utamanya (bukan per
//! segmen path), supaya `/panens`, `/panens/{id}` dan `/panens_filter_date`
//! berkumpul di bawah tabel `panens`. Dari model yang sama dihitung huruf
//! CRUD tiap tabel (badge di header tabel) dan matriks CRUD.
//!
//! Modul ini murni (tanpa `Ui` dan tanpa `Tabular`) supaya mudah dites.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::diagram_flow::{TableUse, table_uses, tables_by_card};
use crate::diagram_flow_layout::resource_of;
use crate::models::structs::{DiagramState, FlowCard, FlowTriggerKind};
use crate::repo_links::method_rank;

/// Urutan huruf operasi di semua tampilan.
pub const CRUD: [char; 4] = ['C', 'R', 'U', 'D'];
/// Kolom matriks untuk tabel yang tertaut tanpa proses (operasi belum diketahui).
pub const UNKNOWN_OP: char = '?';
/// Panjang minimum kecocokan nama resource dengan nama tabel.
const MIN_NAME_MATCH: usize = 3;

/// Pemakaian satu tabel oleh satu endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailTable {
    /// Id node tabel.
    pub table: String,
    pub title: String,
    /// Huruf CRUD urut `CRUD`; kosong = tertaut tanpa proses.
    pub crud: String,
}

/// Satu endpoint di rail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailRow {
    /// Id `FlowCard`.
    pub card_id: String,
    /// Method HTTP, atau jenis trigger (`JOB`, `CRON`, ...) untuk non-HTTP.
    pub method: String,
    pub path: String,
    pub summary: String,
    /// Gabungan huruf CRUD ke semua tabelnya.
    pub crud: String,
    pub steps: usize,
    pub tables: Vec<RailTable>,
}

/// Endpoint yang tabel utamanya sama.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailEntity {
    /// Id node tabel utama; `None` = endpoint tanpa tabel di diagram ini.
    pub table: Option<String>,
    pub title: String,
    /// Gabungan huruf CRUD semua endpoint-nya ke tabel utama.
    pub crud: String,
    pub rows: Vec<RailRow>,
}

/// Endpoint satu repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RailSection {
    pub repo_key: Option<String>,
    pub title: String,
    pub entities: Vec<RailEntity>,
}

impl RailSection {
    pub fn count(&self) -> usize {
        self.entities.iter().map(|e| e.rows.len()).sum()
    }
}

/// Satu baris matriks CRUD: jumlah endpoint per operasi ke satu tabel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixRow {
    pub table: String,
    pub title: String,
    /// Jumlah endpoint untuk `C`, `R`, `U`, `D`.
    pub counts: [usize; 4],
    /// Endpoint yang tertaut ke tabel ini tanpa proses.
    pub unknown: usize,
}

impl MatrixRow {
    pub fn total(&self) -> usize {
        self.counts.iter().sum::<usize>() + self.unknown
    }
}

/// Saringan dari sel matriks: endpoint yang melakukan `op` ke `table`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CellFilter {
    pub table: String,
    pub title: String,
    /// Salah satu `CRUD`, atau `UNKNOWN_OP`.
    pub op: char,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailModel {
    pub sections: Vec<RailSection>,
}

/// Gabungkan huruf `letters` ke `into`, hasil tetap urut `CRUD`.
fn merge_crud(into: &mut String, letters: impl IntoIterator<Item = char>) {
    let mut all: Vec<char> = into.chars().collect();
    all.extend(letters);
    *into = CRUD.iter().filter(|c| all.contains(c)).collect();
}

fn normalize(name: &str) -> String {
    name.trim_start_matches('/')
        .to_ascii_lowercase()
        .replace('-', "_")
}

/// Panjang kecocokan nama resource (`/panens_filter_date`) dengan nama tabel
/// (`panens`): salah satunya awalan yang lain, bentuk tunggal tabel ikut
/// dicoba. 0 = tidak cocok.
fn name_match(resource: &str, title: &str) -> usize {
    let res = normalize(resource);
    let full = normalize(title);
    let singular = full.trim_end_matches('s');
    [full.as_str(), singular]
        .into_iter()
        .filter(|t| t.len() >= MIN_NAME_MATCH && res.len() >= MIN_NAME_MATCH)
        .filter(|t| res.starts_with(t) || t.starts_with(res.as_str()))
        .map(|t| t.len().min(res.len()))
        .max()
        .unwrap_or(0)
}

fn writes(u: &TableUse) -> bool {
    u.crud_badges().iter().any(|c| *c != 'R')
}

/// Tabel utama sebuah endpoint, dipakai untuk mengelompokkannya di rail:
/// 1. tabel yang namanya cocok dengan resource di path (kecocokan terpanjang),
/// 2. tabel pertama yang ditulis,
/// 3. tabel dengan langkah terbanyak,
/// 4. tabel pertama.
///
/// `uses` = hasil `table_uses`; `title_of` = judul tabel dari id node.
pub fn primary_table<'a>(
    card: &FlowCard,
    uses: &'a [TableUse],
    title_of: &dyn Fn(&str) -> String,
) -> Option<&'a str> {
    if card.trigger.kind == FlowTriggerKind::Http {
        let resource = resource_of(&card.trigger);
        let best = uses
            .iter()
            .map(|u| {
                let title = title_of(&u.table);
                (name_match(&resource, &title), title.len(), u)
            })
            .filter(|(score, _, _)| *score > 0)
            // Kecocokan terpanjang; bila sama, nama tabel terpendek.
            .min_by_key(|(score, len, _)| (std::cmp::Reverse(*score), *len));
        if let Some((_, _, u)) = best {
            return Some(u.table.as_str());
        }
    }
    let written = uses
        .iter()
        .filter(|u| writes(u))
        .min_by_key(|u| u.steps.first().copied().unwrap_or(usize::MAX));
    if let Some(u) = written {
        return Some(u.table.as_str());
    }
    let mut busiest: Option<&TableUse> = None;
    for u in uses {
        if busiest.is_none_or(|b| u.steps.len() > b.steps.len()) {
            busiest = Some(u);
        }
    }
    busiest.map(|u| u.table.as_str())
}

/// Nama pendek repository dari kuncinya (segmen terakhir).
fn repo_label(key: &str) -> String {
    key.trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(key)
        .to_string()
}

impl RailModel {
    /// Susun rail dari flow card diagram. Hanya tabel yang ada di diagram
    /// yang dihitung.
    pub fn build(state: &DiagramState) -> Self {
        if state.flow_cards.is_empty() {
            return Self::default();
        }
        let titles: HashMap<&str, &str> = state
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.title.as_str()))
            .collect();
        let title_of = |id: &str| {
            titles
                .get(id)
                .map_or_else(|| id.to_string(), |t| t.to_string())
        };
        let by_card = tables_by_card(state);
        // Judul section: group yang repository-nya sama, kalau ada.
        let group_titles: HashMap<String, &str> = state
            .groups
            .iter()
            .rev()
            .filter_map(|g| {
                let key = g.shared_repo_url().and_then(crate::repo_scan::repo_key)?;
                Some((key, g.title.as_str()))
            })
            .collect();

        // repo -> (tanpa tabel, judul tabel, id tabel) -> endpoint
        type EntityKey = (bool, String, Option<String>);
        let mut sections: BTreeMap<Option<String>, BTreeMap<EntityKey, Vec<RailRow>>> =
            BTreeMap::new();
        for card in &state.flow_cards {
            let tables: Vec<&str> = by_card
                .get(card.id.as_str())
                .into_iter()
                .flatten()
                .copied()
                .filter(|t| titles.contains_key(t))
                .collect();
            let uses = table_uses(card, &tables);
            let primary = primary_table(card, &uses, &title_of).map(str::to_string);
            let mut crud = String::new();
            let rail_tables: Vec<RailTable> = uses
                .iter()
                .map(|u| {
                    let letters: String = u.crud_badges().into_iter().collect();
                    merge_crud(&mut crud, letters.chars());
                    RailTable {
                        table: u.table.clone(),
                        title: title_of(&u.table),
                        crud: letters,
                    }
                })
                .collect();
            let method = if card.trigger.kind == FlowTriggerKind::Http {
                card.trigger.method.trim().to_ascii_uppercase()
            } else {
                format!("{:?}", card.trigger.kind).to_ascii_uppercase()
            };
            let key: EntityKey = match &primary {
                Some(t) => (false, title_of(t).to_lowercase(), primary.clone()),
                None => (true, String::new(), None),
            };
            sections
                .entry(card.repo_key.clone())
                .or_default()
                .entry(key)
                .or_default()
                .push(RailRow {
                    card_id: card.id.clone(),
                    method,
                    path: card.trigger.target.clone(),
                    summary: card.summary.clone(),
                    crud,
                    steps: card.steps.len(),
                    tables: rail_tables,
                });
        }

        let mut out: Vec<RailSection> = sections
            .into_iter()
            .map(|(repo_key, entities)| {
                let title = match &repo_key {
                    Some(k) => group_titles
                        .get(k)
                        .map_or_else(|| repo_label(k), |t| t.to_string()),
                    None => "API".to_string(),
                };
                let entities = entities
                    .into_iter()
                    .map(|((_, _, table), mut rows)| {
                        rows.sort_by(|a, b| {
                            (a.path.as_str(), method_rank(&a.method))
                                .cmp(&(b.path.as_str(), method_rank(&b.method)))
                        });
                        let mut crud = String::new();
                        for t in rows.iter().flat_map(|r| &r.tables) {
                            if Some(&t.table) == table.as_ref() {
                                merge_crud(&mut crud, t.crud.chars());
                            }
                        }
                        RailEntity {
                            title: table
                                .as_deref()
                                .map_or_else(|| "No tables in this diagram".to_string(), &title_of),
                            table,
                            crud,
                            rows,
                        }
                    })
                    .collect();
                RailSection {
                    repo_key,
                    title,
                    entities,
                }
            })
            .collect();
        out.sort_by_key(|s| s.title.to_lowercase());
        Self { sections: out }
    }

    pub fn rows(&self) -> impl Iterator<Item = &RailRow> {
        self.sections
            .iter()
            .flat_map(|s| &s.entities)
            .flat_map(|e| &e.rows)
    }

    pub fn total(&self) -> usize {
        self.rows().count()
    }

    /// Endpoint yang sudah punya langkah proses.
    pub fn with_process(&self) -> usize {
        self.rows().filter(|r| r.steps > 0).count()
    }

    /// Rail yang hanya berisi endpoint yang cocok dengan `query` (method,
    /// path, ringkasan, nama tabel) dan dengan sel matriks `cell`.
    pub fn filtered(&self, query: &str, cell: Option<&CellFilter>) -> Self {
        let q = query.trim().to_lowercase();
        if q.is_empty() && cell.is_none() {
            return self.clone();
        }
        let matches = |entity: &RailEntity, row: &RailRow| {
            let text = q.is_empty()
                || entity.title.to_lowercase().contains(&q)
                || row.path.to_lowercase().contains(&q)
                || row.method.to_lowercase().contains(&q)
                || row.summary.to_lowercase().contains(&q)
                || row
                    .tables
                    .iter()
                    .any(|t| t.title.to_lowercase().contains(&q));
            let op = cell.is_none_or(|c| {
                row.tables.iter().any(|t| {
                    t.table == c.table
                        && if c.op == UNKNOWN_OP {
                            t.crud.is_empty()
                        } else {
                            t.crud.contains(c.op)
                        }
                })
            });
            text && op
        };
        let sections = self
            .sections
            .iter()
            .filter_map(|s| {
                let entities: Vec<RailEntity> = s
                    .entities
                    .iter()
                    .filter_map(|e| {
                        let rows: Vec<RailRow> =
                            e.rows.iter().filter(|r| matches(e, r)).cloned().collect();
                        (!rows.is_empty()).then(|| RailEntity { rows, ..e.clone() })
                    })
                    .collect();
                (!entities.is_empty()).then(|| RailSection {
                    entities,
                    ..s.clone()
                })
            })
            .collect();
        Self { sections }
    }

    /// Huruf CRUD gabungan semua endpoint per tabel, untuk badge di header
    /// tabel. Tabel yang hanya tertaut tanpa proses tidak masuk.
    pub fn table_crud(&self) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::new();
        for t in self.rows().flat_map(|r| &r.tables) {
            if !t.crud.is_empty() {
                merge_crud(out.entry(t.table.clone()).or_default(), t.crud.chars());
            }
        }
        out
    }

    /// Matriks CRUD: jumlah endpoint per operasi untuk tiap tabel, urut dari
    /// tabel yang paling banyak dipakai.
    pub fn matrix(&self) -> Vec<MatrixRow> {
        let mut by_table: HashMap<&str, MatrixRow> = HashMap::new();
        for row in self.rows() {
            let mut seen: HashSet<&str> = HashSet::new();
            for t in &row.tables {
                if !seen.insert(t.table.as_str()) {
                    continue;
                }
                let m = by_table
                    .entry(t.table.as_str())
                    .or_insert_with(|| MatrixRow {
                        table: t.table.clone(),
                        title: t.title.clone(),
                        counts: [0; 4],
                        unknown: 0,
                    });
                if t.crud.is_empty() {
                    m.unknown += 1;
                }
                for (i, c) in CRUD.iter().enumerate() {
                    if t.crud.contains(*c) {
                        m.counts[i] += 1;
                    }
                }
            }
        }
        let mut out: Vec<MatrixRow> = by_table.into_values().collect();
        out.sort_by(|a, b| {
            b.total()
                .cmp(&a.total())
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{
        DiagramNode, EndpointLink, FlowOp, FlowStep, FlowTarget, FlowTrigger,
    };

    fn node(id: &str) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            ..Default::default()
        }
    }

    fn card(id: &str, method: &str, path: &str, steps: &[(&str, FlowOp)]) -> FlowCard {
        FlowCard {
            id: id.into(),
            trigger: FlowTrigger {
                kind: FlowTriggerKind::Http,
                method: method.into(),
                target: path.into(),
            },
            steps: steps
                .iter()
                .map(|(t, op)| FlowStep {
                    title: format!("{op:?} {t}"),
                    target: Some(FlowTarget::Table(t.to_string())),
                    op: Some(*op),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    fn link(table: &str, method: &str, path: &str) -> EndpointLink {
        EndpointLink {
            table: table.into(),
            method: method.into(),
            path: path.into(),
            summary: String::new(),
            request_id: None,
            repo_key: None,
            source: None,
        }
    }

    fn state(nodes: &[&str], cards: Vec<FlowCard>) -> DiagramState {
        DiagramState {
            nodes: nodes.iter().map(|n| node(n)).collect(),
            flow_cards: cards,
            ..Default::default()
        }
    }

    fn entity_titles(model: &RailModel) -> Vec<(String, usize)> {
        model.sections[0]
            .entities
            .iter()
            .map(|e| (e.title.clone(), e.rows.len()))
            .collect()
    }

    #[test]
    fn name_match_handles_prefix_plural_and_dashes() {
        assert_eq!(name_match("/panens_filter_date", "panens"), 6);
        assert_eq!(name_match("/panen-details", "panen_details"), 13);
        assert_eq!(name_match("/panen", "panens"), 5);
        assert_eq!(name_match("/price-today-chickens", "price_todays"), 11);
        assert_eq!(name_match("/recordings", "users"), 0);
        assert_eq!(name_match("/a", "ab"), 0);
    }

    #[test]
    fn endpoints_group_under_the_table_named_in_their_path() {
        let st = state(
            &["panens", "panen_details", "projects"],
            vec![
                card("flw_1", "GET", "/panens", &[("panens", FlowOp::Read)]),
                card(
                    "flw_2",
                    "GET",
                    "/panens_filter_project/{id}",
                    &[("projects", FlowOp::Read), ("panens", FlowOp::Read)],
                ),
                card(
                    "flw_3",
                    "POST",
                    "/panen-details",
                    &[("panens", FlowOp::Read), ("panen_details", FlowOp::Insert)],
                ),
            ],
        );
        let model = RailModel::build(&st);
        assert_eq!(model.sections.len(), 1);
        assert_eq!(
            entity_titles(&model),
            vec![("panen_details".to_string(), 1), ("panens".to_string(), 2)]
        );
        let panens = &model.sections[0].entities[1];
        assert_eq!(panens.crud, "R");
        // Urut path lalu method.
        assert_eq!(panens.rows[0].path, "/panens");
        assert_eq!(panens.rows[1].path, "/panens_filter_project/{id}");
    }

    #[test]
    fn primary_falls_back_to_first_written_then_busiest_table() {
        let st = state(
            &["users", "orders", "audit"],
            vec![
                card(
                    "flw_1",
                    "POST",
                    "/checkout",
                    &[("users", FlowOp::Read), ("orders", FlowOp::Insert)],
                ),
                card(
                    "flw_2",
                    "GET",
                    "/report",
                    &[
                        ("users", FlowOp::Read),
                        ("audit", FlowOp::Read),
                        ("audit", FlowOp::Read),
                    ],
                ),
            ],
        );
        let model = RailModel::build(&st);
        assert_eq!(
            entity_titles(&model),
            vec![("audit".to_string(), 1), ("orders".to_string(), 1)]
        );
    }

    #[test]
    fn cards_without_diagram_tables_go_last() {
        let mut st = state(
            &["users"],
            vec![
                card("flw_1", "GET", "/health", &[]),
                card("flw_2", "GET", "/users", &[]),
            ],
        );
        st.endpoint_links.push(link("users", "GET", "/users"));
        let model = RailModel::build(&st);
        let entities = &model.sections[0].entities;
        assert_eq!(entities[0].table.as_deref(), Some("users"));
        assert_eq!(entities[1].table, None);
        assert_eq!(entities[1].rows[0].path, "/health");
        // Tertaut tanpa proses: belum ada huruf CRUD.
        assert_eq!(entities[0].rows[0].crud, "");
        assert_eq!(model.total(), 2);
        assert_eq!(model.with_process(), 0);
    }

    #[test]
    fn row_crud_merges_all_tables_in_crud_order() {
        let st = state(
            &["orders", "stock"],
            vec![card(
                "flw_1",
                "PUT",
                "/orders/{id}",
                &[
                    ("stock", FlowOp::Delete),
                    ("orders", FlowOp::Update),
                    ("orders", FlowOp::Read),
                ],
            )],
        );
        let model = RailModel::build(&st);
        let row = &model.sections[0].entities[0].rows[0];
        assert_eq!(row.crud, "RUD");
        assert_eq!(
            model.table_crud().get("orders").map(String::as_str),
            Some("RU")
        );
        assert_eq!(
            model.table_crud().get("stock").map(String::as_str),
            Some("D")
        );
    }

    #[test]
    fn filter_by_text_and_matrix_cell() {
        let mut st = state(
            &["orders", "users"],
            vec![
                card("flw_1", "GET", "/orders", &[("orders", FlowOp::Read)]),
                card("flw_2", "POST", "/orders", &[("orders", FlowOp::Insert)]),
                card("flw_3", "GET", "/users", &[]),
            ],
        );
        st.endpoint_links.push(link("users", "GET", "/users"));
        let model = RailModel::build(&st);
        assert_eq!(model.filtered("post", None).total(), 1);
        assert_eq!(model.filtered("ORDERS", None).total(), 2);
        assert_eq!(model.filtered("nothing", None).total(), 0);
        let cell = |table: &str, op: char| CellFilter {
            table: table.into(),
            title: table.into(),
            op,
        };
        let created = model.filtered("", Some(&cell("orders", 'C')));
        assert_eq!(created.total(), 1);
        assert_eq!(
            created.rows().next().map(|r| r.method.as_str()),
            Some("POST")
        );
        assert_eq!(
            model.filtered("", Some(&cell("users", UNKNOWN_OP))).total(),
            1
        );
        assert_eq!(model.filtered("get", Some(&cell("orders", 'C'))).total(), 0);
    }

    #[test]
    fn matrix_counts_endpoints_per_operation() {
        let mut st = state(
            &["orders", "users"],
            vec![
                card("flw_1", "GET", "/orders", &[("orders", FlowOp::Read)]),
                card(
                    "flw_2",
                    "POST",
                    "/orders",
                    &[("orders", FlowOp::Upsert), ("orders", FlowOp::Read)],
                ),
                card("flw_3", "GET", "/users", &[]),
            ],
        );
        st.endpoint_links.push(link("users", "GET", "/users"));
        let matrix = RailModel::build(&st).matrix();
        assert_eq!(matrix[0].title, "orders");
        assert_eq!(matrix[0].counts, [1, 2, 1, 0]);
        assert_eq!(matrix[0].unknown, 0);
        assert_eq!(matrix[1].title, "users");
        assert_eq!(matrix[1].counts, [0; 4]);
        assert_eq!(matrix[1].unknown, 1);
    }

    #[test]
    fn sections_split_by_repository() {
        let mut a = card("flw_1", "GET", "/a", &[]);
        a.repo_key = Some("github.com/acme/billing".into());
        let b = card("flw_2", "GET", "/b", &[]);
        let model = RailModel::build(&state(&[], vec![a, b]));
        let titles: Vec<&str> = model.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["API", "billing"]);
        assert_eq!(model.sections[1].count(), 1);
    }
}
