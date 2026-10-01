//! Flow card: proses bisnis yang tampil sebagai card di kanvas diagram.
//!
//! `DiagramState::endpoint_links` tetap sumber kebenaran "endpoint menyentuh
//! tabel". Card menambah ringkasan, posisi, dan langkah berurutan. Identitas
//! logis card HTTP adalah `(repo_key, route_key(method, path))`, jadi
//! `/users/{id}` dan `/Users/{userId}` dianggap endpoint yang sama.
//!
//! Modul ini murni (tanpa egui dan tanpa `Tabular`) supaya mudah dites.

use std::collections::{HashMap, HashSet};

use crate::models::structs::{
    DiagramState, EndpointLink, FlowCard, FlowOp, FlowTrigger, FlowTriggerKind,
};
use crate::repo_endpoints::route_key;
use crate::repo_links::{ApplyStats, EndpointTables, method_rank};

/// Langkah maksimum per flow.
pub const MAX_STEPS: usize = 25;
/// Panjang maksimum `FlowStep::title` (karakter).
pub const MAX_TITLE_CHARS: usize = 80;
/// Panjang maksimum `FlowStep::detail` (karakter).
pub const MAX_DETAIL_CHARS: usize = 300;
/// Kolom maksimum per langkah.
pub const MAX_STEP_COLUMNS: usize = 12;
/// File sumber maksimum per flow (`FlowMeta::source_files`).
pub const MAX_SOURCE_FILES: usize = 20;

const ID_PREFIX: &str = "flw_";

/// Kunci identitas card HTTP: (repo_key, route_key).
type CardKey = (Option<String>, String);

/// Arah aliran data antara card dan target sebuah langkah.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowDirection {
    /// Card menulis / mengirim ke target.
    ToTarget,
    /// Card membaca / menerima dari target.
    FromTarget,
    /// Beberapa operasi dengan arah berlawanan ke target yang sama.
    Both,
    /// Operasi tidak diketahui; digambar tanpa panah.
    Unknown,
}

impl FlowDirection {
    /// Gabungan arah beberapa langkah ke target yang sama.
    pub fn combine(self, other: FlowDirection) -> FlowDirection {
        use FlowDirection::*;
        match (self, other) {
            (Unknown, d) | (d, Unknown) => d,
            (a, b) if a == b => a,
            _ => Both,
        }
    }
}

/// Arah aliran data untuk operasi `op`.
pub fn op_direction(op: Option<FlowOp>) -> FlowDirection {
    match op {
        Some(FlowOp::Read | FlowOp::Consume) => FlowDirection::FromTarget,
        Some(
            FlowOp::Insert
            | FlowOp::Update
            | FlowOp::Delete
            | FlowOp::Upsert
            | FlowOp::Call
            | FlowOp::Publish,
        ) => FlowDirection::ToTarget,
        Some(FlowOp::Unknown) | None => FlowDirection::Unknown,
    }
}

/// Nomor terbesar id `flw_<n>` yang sudah dipakai, plus satu.
fn next_flow_number(existing: &[FlowCard]) -> u64 {
    existing
        .iter()
        .filter_map(|c| c.id.strip_prefix(ID_PREFIX)?.parse::<u64>().ok())
        .max()
        .map_or(1, |n| n + 1)
}

/// Id baru `flw_<n>` yang belum dipakai card mana pun. Pendek dan urut,
/// karena id ini juga dipakai di prompt AI.
pub fn new_flow_id(existing: &[FlowCard]) -> String {
    let mut n = next_flow_number(existing);
    loop {
        let id = format!("{ID_PREFIX}{n}");
        if !existing.iter().any(|c| c.id == id) {
            return id;
        }
        n += 1;
    }
}

fn http_key(card: &FlowCard) -> Option<CardKey> {
    let t = &card.trigger;
    (t.kind == FlowTriggerKind::Http && !t.method.trim().is_empty() && !t.target.trim().is_empty())
        .then(|| (card.repo_key.clone(), route_key(&t.method, &t.target)))
}

fn link_key(link: &EndpointLink) -> CardKey {
    (link.repo_key.clone(), route_key(&link.method, &link.path))
}

/// `link` milik endpoint card ini.
pub fn card_matches_link(card: &FlowCard, link: &EndpointLink) -> bool {
    http_key(card).is_some_and(|k| k == link_key(link))
}

/// Card HTTP untuk endpoint (repo_key, method, path).
pub fn card_for_endpoint<'a>(
    state: &'a DiagramState,
    repo_key: Option<&str>,
    method: &str,
    path: &str,
) -> Option<&'a FlowCard> {
    let key = (repo_key.map(str::to_string), route_key(method, path));
    state
        .flow_cards
        .iter()
        .find(|c| http_key(c).is_some_and(|k| k == key))
}

fn card_from_link(link: &EndpointLink, id: String) -> FlowCard {
    FlowCard {
        id,
        trigger: FlowTrigger {
            kind: FlowTriggerKind::Http,
            method: link.method.trim().to_ascii_uppercase(),
            target: link.path.trim().to_string(),
        },
        summary: link.summary.clone(),
        repo_key: link.repo_key.clone(),
        source: link.source.clone(),
        request_id: link.request_id.clone(),
        ..Default::default()
    }
}

/// Isi field card yang masih kosong dari link. `true` bila ada yang berubah.
fn fill_missing(card: &mut FlowCard, link: &EndpointLink) -> bool {
    let mut changed = false;
    if card.summary.is_empty() && !link.summary.is_empty() {
        card.summary = link.summary.clone();
        changed = true;
    }
    if card.source.is_none() && link.source.is_some() {
        card.source = link.source.clone();
        changed = true;
    }
    if card.request_id.is_none() && link.request_id.is_some() {
        card.request_id = link.request_id.clone();
        changed = true;
    }
    changed
}

/// Buat card tanpa langkah untuk tiap endpoint di `endpoint_links` yang
/// belum punya card, dan lengkapi field kosong card yang sudah ada. Ini yang
/// memigrasikan diagram lama saat dibuka. `true` bila state berubah.
pub fn sync_cards_from_links(state: &mut DiagramState) -> bool {
    if state.endpoint_links.is_empty() {
        return false;
    }
    let mut index: HashMap<CardKey, usize> = state
        .flow_cards
        .iter()
        .enumerate()
        .filter_map(|(i, c)| Some((http_key(c)?, i)))
        .collect();
    let mut links: Vec<&EndpointLink> = state.endpoint_links.iter().collect();
    links.sort_by(|a, b| {
        (a.path.as_str(), method_rank(&a.method)).cmp(&(b.path.as_str(), method_rank(&b.method)))
    });
    let mut next = next_flow_number(&state.flow_cards);
    let mut changed = false;
    for link in links {
        let key = link_key(link);
        if let Some(&i) = index.get(&key) {
            changed |= fill_missing(&mut state.flow_cards[i], link);
            continue;
        }
        let mut id = format!("{ID_PREFIX}{next}");
        while state.flow_cards.iter().any(|c| c.id == id) {
            next += 1;
            id = format!("{ID_PREFIX}{next}");
        }
        next += 1;
        index.insert(key, state.flow_cards.len());
        state.flow_cards.push(card_from_link(link, id));
        changed = true;
    }
    changed
}

/// Tambahkan `EndpointLink` untuk langkah bertarget tabel di card `card_id`
/// yang belum punya link. Link yang sudah ada tidak diubah.
pub fn links_from_steps(state: &mut DiagramState, card_id: &str) -> ApplyStats {
    let Some(card) = state.flow_cards.iter().find(|c| c.id == card_id) else {
        return ApplyStats::default();
    };
    if http_key(card).is_none() {
        return ApplyStats::default();
    }
    let linked: HashSet<&str> = state
        .endpoint_links
        .iter()
        .filter(|l| card_matches_link(card, l))
        .map(|l| l.table.as_str())
        .collect();
    let mut tables: Vec<String> = Vec::new();
    for t in card.steps.iter().filter_map(|s| s.target.as_ref()?.table()) {
        if !linked.contains(t) && !tables.iter().any(|x| x == t) {
            tables.push(t.to_string());
        }
    }
    if tables.is_empty() {
        return ApplyStats::default();
    }
    let endpoint = EndpointTables {
        method: card.trigger.method.clone(),
        path: card.trigger.target.clone(),
        summary: card.summary.clone(),
        request_id: card.request_id.clone(),
        source: card.source.clone(),
        tables,
    };
    let key = card.repo_key.clone();
    crate::repo_links::apply_endpoint_links(state, key.as_deref(), &[endpoint])
}

/// Tabel yang disentuh card: gabungan tabel dari `endpoint_links` dan dari
/// langkah bertarget tabel, tanpa duplikat, urut kemunculan.
pub fn tables_of(state: &DiagramState, card: &FlowCard) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let links = state
        .endpoint_links
        .iter()
        .filter(|l| card_matches_link(card, l))
        .map(|l| l.table.as_str());
    let steps = card.steps.iter().filter_map(|s| s.target.as_ref()?.table());
    for t in links.chain(steps) {
        if !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// `tables_of` untuk semua card sekaligus: satu lintasan `endpoint_links`,
/// bukan satu lintasan per card. Dipakai tiap frame oleh kanvas.
pub fn tables_by_card(state: &DiagramState) -> HashMap<&str, Vec<&str>> {
    let mut by_key: HashMap<CardKey, Vec<&str>> = HashMap::new();
    for l in &state.endpoint_links {
        let list = by_key.entry(link_key(l)).or_default();
        if !list.contains(&l.table.as_str()) {
            list.push(l.table.as_str());
        }
    }
    state
        .flow_cards
        .iter()
        .map(|c| {
            let mut out: Vec<&str> = http_key(c)
                .and_then(|k| by_key.get(&k))
                .cloned()
                .unwrap_or_default();
            for t in c.steps.iter().filter_map(|s| s.target.as_ref()?.table()) {
                if !out.contains(&t) {
                    out.push(t);
                }
            }
            (c.id.as_str(), out)
        })
        .collect()
}

/// Pemakaian satu tabel oleh sebuah card: operasi dan nomor langkahnya.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableUse {
    pub table: String,
    /// Operasi unik, urut kemunculan. Kosong = hanya dari `endpoint_links`.
    pub ops: Vec<FlowOp>,
    /// Indeks langkah (0-based) yang menyentuh tabel ini.
    pub steps: Vec<usize>,
    /// Kolom pertama yang disebut langkah mana pun (untuk ujung garis).
    pub column: Option<String>,
}

impl TableUse {
    /// Arah gabungan semua operasi ke tabel ini.
    pub fn direction(&self) -> FlowDirection {
        self.ops
            .iter()
            .map(|op| op_direction(Some(*op)))
            .reduce(FlowDirection::combine)
            .unwrap_or(FlowDirection::Unknown)
    }

    /// Lencana CRUD (`C`, `R`, `U`, `D`) urut CRUD, tanpa duplikat.
    pub fn crud_badges(&self) -> Vec<char> {
        let mut out = Vec::new();
        for (ch, hit) in [
            ('C', self.has(&[FlowOp::Insert, FlowOp::Upsert])),
            ('R', self.has(&[FlowOp::Read])),
            ('U', self.has(&[FlowOp::Update, FlowOp::Upsert])),
            ('D', self.has(&[FlowOp::Delete])),
        ] {
            if hit {
                out.push(ch);
            }
        }
        out
    }

    fn has(&self, ops: &[FlowOp]) -> bool {
        self.ops.iter().any(|o| ops.contains(o))
    }
}

/// Tabel yang disentuh card beserta operasinya, urut seperti `tables_of`.
/// `tables` adalah hasil `tables_of` / `tables_by_card` untuk card ini.
pub fn table_uses(card: &FlowCard, tables: &[&str]) -> Vec<TableUse> {
    tables
        .iter()
        .map(|&t| {
            let mut u = TableUse {
                table: t.to_string(),
                ..Default::default()
            };
            for (i, s) in card.steps.iter().enumerate() {
                if s.target.as_ref().and_then(|x| x.table()) != Some(t) {
                    continue;
                }
                u.steps.push(i);
                if let Some(op) = s.op
                    && !u.ops.contains(&op)
                {
                    u.ops.push(op);
                }
                if u.column.is_none() {
                    u.column = s.columns.first().cloned();
                }
            }
            u
        })
        .collect()
}

/// Hapus card beserta link endpoint-nya (supaya tidak dibuat ulang oleh
/// `sync_cards_from_links`).
pub fn remove_card(state: &mut DiagramState, card_id: &str) {
    let Some(i) = state.flow_cards.iter().position(|c| c.id == card_id) else {
        return;
    };
    let card = state.flow_cards.remove(i);
    state
        .endpoint_links
        .retain(|l| !card_matches_link(&card, l));
    if state.selected_flow.as_deref() == Some(card_id) {
        state.selected_flow = None;
    }
    if state.focus_flow.as_deref() == Some(card_id) {
        state.focus_flow = None;
    }
    if state
        .flow_play
        .as_ref()
        .is_some_and(|p| p.card_id == card_id)
    {
        state.flow_play = None;
    }
}

/// Hapus card HTTP tanpa langkah yang sudah tidak punya link. Card yang
/// punya langkah tetap ada. Pilihan, fokus, dan pemutaran yang menunjuk card
/// terhapus ikut dilepas. `true` bila ada card yang dihapus.
pub fn drop_orphan_cards(state: &mut DiagramState) -> bool {
    if state.flow_cards.is_empty() {
        return false;
    }
    let linked: HashSet<CardKey> = state.endpoint_links.iter().map(link_key).collect();
    let before = state.flow_cards.len();
    state
        .flow_cards
        .retain(|c| !c.steps.is_empty() || http_key(c).is_none_or(|k| linked.contains(&k)));
    if state.flow_cards.len() == before {
        return false;
    }
    let alive = |id: &Option<String>, cards: &[FlowCard]| {
        id.as_ref()
            .is_none_or(|id| cards.iter().any(|c| &c.id == id))
    };
    if !alive(&state.selected_flow, &state.flow_cards) {
        state.selected_flow = None;
    }
    if !alive(&state.focus_flow, &state.flow_cards) {
        state.focus_flow = None;
    }
    if state
        .flow_play
        .as_ref()
        .is_some_and(|p| !state.flow_cards.iter().any(|c| c.id == p.card_id))
    {
        state.flow_play = None;
    }
    true
}

/// Lepas satu link endpoint. Bila itu link terakhir sebuah card tanpa
/// langkah, card-nya ikut dihapus.
pub fn unlink_endpoint(state: &mut DiagramState, link: &EndpointLink) {
    state.endpoint_links.retain(|x| !x.same_endpoint(link));
    drop_orphan_cards(state);
}

/// Target tabel yang tabelnya sudah hilang dari diagram diubah menjadi
/// `None`; langkahnya tetap karena teksnya masih berguna. Card tanpa langkah
/// yang link-nya sudah terpangkas ikut dihapus. Diagram yang belum memuat
/// node tidak dipangkas. `true` bila state berubah.
pub fn prune_flow_cards(state: &mut DiagramState) -> bool {
    if state.flow_cards.is_empty() {
        return false;
    }
    let ids: HashSet<&str> = state.nodes.iter().map(|n| n.id.as_str()).collect();
    if ids.is_empty() {
        return false;
    }
    let mut changed = false;
    for step in state.flow_cards.iter_mut().flat_map(|c| c.steps.iter_mut()) {
        let gone = step
            .target
            .as_ref()
            .and_then(|t| t.table())
            .is_some_and(|t| {
                // Tabel link database dimuat belakangan; jangan dianggap hilang.
                !ids.contains(t) && !crate::diagram_links::is_linked_id(t)
            });
        if gone {
            step.target = None;
            changed = true;
        }
    }
    drop_orphan_cards(state) || changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramNode, FlowStep, FlowStepKind, FlowTarget};

    fn node(id: &str) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            ..Default::default()
        }
    }

    fn link(table: &str, method: &str, path: &str, key: Option<&str>) -> EndpointLink {
        EndpointLink {
            table: table.into(),
            method: method.into(),
            path: path.into(),
            summary: String::new(),
            request_id: None,
            repo_key: key.map(str::to_string),
            source: None,
        }
    }

    fn state(nodes: &[&str], links: Vec<EndpointLink>) -> DiagramState {
        DiagramState {
            nodes: nodes.iter().map(|n| node(n)).collect(),
            endpoint_links: links,
            ..Default::default()
        }
    }

    fn db_step(table: &str, op: FlowOp) -> FlowStep {
        FlowStep {
            kind: FlowStepKind::Db,
            title: format!("{op:?} {table}"),
            target: Some(FlowTarget::Table(table.into())),
            op: Some(op),
            ..Default::default()
        }
    }

    #[test]
    fn new_flow_id_is_sequential_and_ignores_foreign_ids() {
        assert_eq!(new_flow_id(&[]), "flw_1");
        let cards = vec![
            FlowCard {
                id: "flw_3".into(),
                ..Default::default()
            },
            FlowCard {
                id: "custom".into(),
                ..Default::default()
            },
        ];
        assert_eq!(new_flow_id(&cards), "flw_4");
    }

    #[test]
    fn sync_creates_one_card_per_endpoint_and_is_idempotent() {
        let mut st = state(
            &["users", "orders"],
            vec![
                link("orders", "POST", "/orders", Some("repo")),
                link("users", "post", "/orders", Some("repo")),
                link("users", "GET", "/users/{id}", Some("repo")),
            ],
        );
        assert!(sync_cards_from_links(&mut st));
        assert_eq!(st.flow_cards.len(), 2);
        let post = card_for_endpoint(&st, Some("repo"), "POST", "/orders").unwrap();
        assert_eq!(post.trigger.method, "POST");
        assert_eq!(post.trigger.kind, FlowTriggerKind::Http);
        assert!(!sync_cards_from_links(&mut st));
        assert_eq!(st.flow_cards.len(), 2);
    }

    #[test]
    fn sync_matches_routes_by_route_key_and_separates_repositories() {
        let mut st = state(
            &["users"],
            vec![
                link("users", "GET", "/users/{id}", Some("a")),
                link("users", "GET", "/Users/{userId}", Some("a")),
                link("users", "GET", "/users/{id}", Some("b")),
            ],
        );
        sync_cards_from_links(&mut st);
        assert_eq!(st.flow_cards.len(), 2);
        assert!(card_for_endpoint(&st, Some("a"), "get", "/users/{uid}").is_some());
        assert!(card_for_endpoint(&st, None, "GET", "/users/{id}").is_none());
    }

    #[test]
    fn sync_fills_missing_fields_without_overwriting() {
        let mut st = state(&["users"], vec![link("users", "GET", "/users", None)]);
        sync_cards_from_links(&mut st);
        st.flow_cards[0].source = Some("keep.ts:1".into());
        st.endpoint_links[0].summary = "List users".into();
        st.endpoint_links[0].source = Some("other.ts:9".into());
        assert!(sync_cards_from_links(&mut st));
        assert_eq!(st.flow_cards[0].summary, "List users");
        assert_eq!(st.flow_cards[0].source.as_deref(), Some("keep.ts:1"));
    }

    #[test]
    fn tables_of_merges_links_and_steps() {
        let mut st = state(
            &["users", "orders", "stock"],
            vec![link("users", "POST", "/orders", None)],
        );
        sync_cards_from_links(&mut st);
        st.flow_cards[0].steps = vec![
            db_step("users", FlowOp::Read),
            db_step("orders", FlowOp::Insert),
            FlowStep {
                kind: FlowStepKind::Queue,
                target: Some(FlowTarget::Queue("order.created".into())),
                ..Default::default()
            },
            db_step("stock", FlowOp::Update),
        ];
        let card = st.flow_cards[0].clone();
        assert_eq!(tables_of(&st, &card), vec!["users", "orders", "stock"]);
    }

    #[test]
    fn tables_by_card_matches_tables_of() {
        let mut st = state(
            &["users", "orders"],
            vec![
                link("users", "POST", "/orders", None),
                link("orders", "GET", "/orders", None),
            ],
        );
        sync_cards_from_links(&mut st);
        st.flow_cards[0].steps = vec![db_step("orders", FlowOp::Insert)];
        let by_card = tables_by_card(&st);
        for c in &st.flow_cards {
            let expected = tables_of(&st, c);
            assert_eq!(by_card[c.id.as_str()], expected);
        }
    }

    #[test]
    fn table_uses_collects_ops_steps_and_crud_badges() {
        let mut card = FlowCard::default();
        let mut write = db_step("orders", FlowOp::Insert);
        write.columns = vec!["user_id".into()];
        card.steps = vec![
            db_step("users", FlowOp::Read),
            write,
            db_step("orders", FlowOp::Update),
        ];
        let uses = table_uses(&card, &["users", "orders", "audit"]);
        assert_eq!(uses[0].steps, vec![0]);
        assert_eq!(uses[0].crud_badges(), vec!['R']);
        assert_eq!(uses[0].direction(), FlowDirection::FromTarget);
        assert_eq!(uses[1].steps, vec![1, 2]);
        assert_eq!(uses[1].crud_badges(), vec!['C', 'U']);
        assert_eq!(uses[1].column.as_deref(), Some("user_id"));
        assert_eq!(uses[1].direction(), FlowDirection::ToTarget);
        // Tabel hanya dari link: tanpa operasi, arah tidak diketahui.
        assert!(uses[2].ops.is_empty());
        assert_eq!(uses[2].direction(), FlowDirection::Unknown);
    }

    #[test]
    fn remove_card_drops_its_links_and_runtime_state() {
        let mut st = state(
            &["users"],
            vec![
                link("users", "GET", "/users", None),
                link("users", "POST", "/users", None),
            ],
        );
        sync_cards_from_links(&mut st);
        let id = card_for_endpoint(&st, None, "GET", "/users")
            .unwrap()
            .id
            .clone();
        st.selected_flow = Some(id.clone());
        st.focus_flow = Some(id.clone());
        st.flow_play = Some(crate::diagram_flow_play::new_playback(&id));
        remove_card(&mut st, &id);
        assert_eq!(st.flow_cards.len(), 1);
        assert_eq!(st.endpoint_links.len(), 1);
        assert!(st.selected_flow.is_none() && st.focus_flow.is_none() && st.flow_play.is_none());
        // Tidak dibuat ulang saat sinkron berikutnya.
        assert!(!sync_cards_from_links(&mut st));
    }

    #[test]
    fn links_from_steps_adds_only_missing_tables() {
        let mut st = state(
            &["users", "orders"],
            vec![link("users", "POST", "/orders", Some("repo"))],
        );
        st.endpoint_links[0].summary = "Existing".into();
        sync_cards_from_links(&mut st);
        let id = st.flow_cards[0].id.clone();
        st.flow_cards[0].summary = "Creates an order".into();
        st.flow_cards[0].steps = vec![
            db_step("users", FlowOp::Read),
            db_step("orders", FlowOp::Insert),
            db_step("ghost", FlowOp::Delete),
        ];
        let stats = links_from_steps(&mut st, &id);
        assert_eq!(stats.added, 1);
        assert_eq!(stats.updated, 0);
        assert_eq!(stats.unresolved, 1);
        let orders = st
            .endpoint_links
            .iter()
            .find(|l| l.table == "orders")
            .unwrap();
        assert_eq!(orders.repo_key.as_deref(), Some("repo"));
        assert_eq!(orders.method, "POST");
        let users = st
            .endpoint_links
            .iter()
            .find(|l| l.table == "users")
            .unwrap();
        assert_eq!(users.summary, "Existing");
        assert_eq!(
            links_from_steps(&mut st, &id),
            ApplyStats {
                unresolved: 1,
                ..Default::default()
            }
        );
        assert_eq!(links_from_steps(&mut st, "missing"), ApplyStats::default());
    }

    #[test]
    fn unlink_last_link_removes_empty_card_but_keeps_card_with_steps() {
        let mut st = state(
            &["users", "orders"],
            vec![
                link("users", "GET", "/users", None),
                link("orders", "POST", "/orders", None),
            ],
        );
        sync_cards_from_links(&mut st);
        let users_card = card_for_endpoint(&st, None, "GET", "/users")
            .unwrap()
            .id
            .clone();
        st.selected_flow = Some(users_card.clone());
        st.focus_flow = Some(users_card);
        st.flow_cards
            .iter_mut()
            .find(|c| c.trigger.target == "/orders")
            .unwrap()
            .steps = vec![db_step("orders", FlowOp::Insert)];

        let l = st.endpoint_links[0].clone();
        unlink_endpoint(&mut st, &l);
        assert!(card_for_endpoint(&st, None, "GET", "/users").is_none());
        assert_eq!(st.selected_flow, None);
        assert_eq!(st.focus_flow, None);

        let l = st.endpoint_links[0].clone();
        unlink_endpoint(&mut st, &l);
        assert!(st.endpoint_links.is_empty());
        assert!(card_for_endpoint(&st, None, "POST", "/orders").is_some());
    }

    #[test]
    fn unlink_keeps_card_while_other_tables_remain() {
        let mut st = state(
            &["users", "orders"],
            vec![
                link("users", "POST", "/orders", None),
                link("orders", "POST", "/orders", None),
            ],
        );
        sync_cards_from_links(&mut st);
        let l = st.endpoint_links[0].clone();
        unlink_endpoint(&mut st, &l);
        assert_eq!(st.flow_cards.len(), 1);
    }

    #[test]
    fn prune_clears_missing_table_targets_but_keeps_steps() {
        let mut st = state(&["users"], vec![link("users", "POST", "/orders", None)]);
        sync_cards_from_links(&mut st);
        st.flow_cards[0].steps = vec![
            db_step("users", FlowOp::Read),
            db_step("orders", FlowOp::Insert),
            db_step(
                &crate::diagram_links::namespaced("lnk_1", "items"),
                FlowOp::Read,
            ),
        ];
        assert!(prune_flow_cards(&mut st));
        let steps = &st.flow_cards[0].steps;
        assert_eq!(steps.len(), 3);
        assert!(steps[0].target.is_some());
        assert_eq!(steps[1].target, None);
        assert_eq!(steps[1].title, "Insert orders");
        assert!(steps[2].target.is_some());
        assert!(!prune_flow_cards(&mut st));
    }

    #[test]
    fn prune_skips_diagram_without_nodes_and_drops_orphans() {
        let mut st = state(&[], vec![]);
        st.flow_cards.push(FlowCard {
            id: "flw_1".into(),
            steps: vec![db_step("orders", FlowOp::Insert)],
            ..Default::default()
        });
        assert!(!prune_flow_cards(&mut st));
        assert!(st.flow_cards[0].steps[0].target.is_some());

        // Link terpangkas karena tabelnya hilang: card kosong ikut dihapus.
        let mut st = state(&["users"], vec![link("users", "GET", "/users", None)]);
        sync_cards_from_links(&mut st);
        st.nodes = vec![node("orders")];
        crate::repo_links::prune_endpoint_links(&mut st);
        assert!(prune_flow_cards(&mut st));
        assert!(st.flow_cards.is_empty());
    }

    #[test]
    fn layout_fingerprint_tracks_card_position() {
        let mut st = state(&["users"], vec![link("users", "GET", "/users", None)]);
        sync_cards_from_links(&mut st);
        let before = crate::diagram_schema::layout_fingerprint(&st);
        st.flow_cards[0].pos = Some([40.0, 80.0]);
        assert_ne!(crate::diagram_schema::layout_fingerprint(&st), before);
    }

    #[test]
    fn op_direction_follows_data_flow() {
        assert_eq!(op_direction(Some(FlowOp::Read)), FlowDirection::FromTarget);
        assert_eq!(
            op_direction(Some(FlowOp::Consume)),
            FlowDirection::FromTarget
        );
        for op in [
            FlowOp::Insert,
            FlowOp::Update,
            FlowOp::Upsert,
            FlowOp::Delete,
            FlowOp::Publish,
        ] {
            assert_eq!(op_direction(Some(op)), FlowDirection::ToTarget);
        }
        assert_eq!(op_direction(None), FlowDirection::Unknown);
        assert_eq!(op_direction(Some(FlowOp::Unknown)), FlowDirection::Unknown);
        let read = FlowDirection::FromTarget;
        assert_eq!(read.combine(FlowDirection::ToTarget), FlowDirection::Both);
        assert_eq!(read.combine(FlowDirection::Unknown), read);
        assert_eq!(read.combine(read), read);
    }

    #[test]
    fn old_diagram_without_flow_fields_deserializes() {
        let mut json = serde_json::to_value(DiagramState::default()).unwrap();
        let obj = json.as_object_mut().unwrap();
        assert!(!obj.contains_key("flow_cards"));
        assert!(!obj.contains_key("endpoint_display"));
        assert!(!obj.contains_key("flow_lines"));
        obj.insert(
            "endpoint_links".into(),
            serde_json::json!([{"table": "users", "method": "GET", "path": "/users"}]),
        );
        let mut st: DiagramState = serde_json::from_value(json).unwrap();
        assert!(st.flow_cards.is_empty());
        assert_eq!(
            st.endpoint_display,
            crate::models::structs::EndpointDisplay::Rail
        );
        assert!(sync_cards_from_links(&mut st));
        assert_eq!(st.flow_cards.len(), 1);
    }

    #[test]
    fn unknown_kinds_deserialize_as_unknown() {
        let step: FlowStep = serde_json::from_value(serde_json::json!({
            "kind": "saga",
            "title": "Compensate",
            "op": "stream",
            "target": {"kind": "grpc", "id": "billing"}
        }))
        .unwrap();
        assert_eq!(step.kind, FlowStepKind::Unknown);
        assert_eq!(step.op, Some(FlowOp::Unknown));
        assert_eq!(step.target, Some(FlowTarget::Unknown));
        let known: FlowTarget =
            serde_json::from_value(serde_json::json!({"kind": "cache", "id": "user:*"})).unwrap();
        assert_eq!(known, FlowTarget::Cache("user:*".into()));
        let card: FlowCard = serde_json::from_value(serde_json::json!({
            "id": "flw_1",
            "trigger": {"kind": "webhook", "target": "stripe"}
        }))
        .unwrap();
        assert_eq!(card.trigger.kind, FlowTriggerKind::Unknown);
    }

    #[test]
    fn flow_card_roundtrips() {
        let card = FlowCard {
            id: "flw_7".into(),
            trigger: FlowTrigger {
                kind: FlowTriggerKind::Http,
                method: "POST".into(),
                target: "/orders".into(),
            },
            summary: "Creates an order".into(),
            pos: Some([10.0, -20.5]),
            steps: vec![
                FlowStep {
                    columns: vec!["user_id".into()],
                    source: Some("src/order.ts:58".into()),
                    condition: Some("if stock".into()),
                    ..db_step("orders", FlowOp::Insert)
                },
                FlowStep {
                    kind: FlowStepKind::External,
                    target: Some(FlowTarget::External("Stripe API".into())),
                    op: Some(FlowOp::Call),
                    ..Default::default()
                },
            ],
            meta: Some(crate::models::structs::FlowMeta {
                generated_at: "2026-09-30T10:00:00Z".into(),
                backend: "Claude Code".into(),
                source_files: vec!["src/order.ts".into()],
                source_hash: "abc".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let json = serde_json::to_value(&card).unwrap();
        assert_eq!(
            json["steps"][0]["target"],
            serde_json::json!({"kind": "table", "id": "orders"})
        );
        assert_eq!(json["steps"][0]["op"], "insert");
        assert!(json["steps"][1].get("detail").is_none());
        let back: FlowCard = serde_json::from_value(json).unwrap();
        assert_eq!(back, card);
    }
}
