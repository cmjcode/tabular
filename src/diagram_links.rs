//! Diagram gabungan: database lain di-*link* ke diagram host dan tampil
//! sebagai kontainer. Diagram host hanya menyimpan `LinkedDatabase`
//! (referensi + posisi kontainer); isi kontainer dimaterialisasi ulang dari
//! diagram sumber, sehingga perubahan di diagram sumber ikut terlihat.
//!
//! Item milik link dikenali dari prefix id `{link_id}::` (node, group, edge,
//! relasi). Relasi yang dibuat user di diagram gabungan — termasuk relasi
//! lintas database — tetap milik host dan disimpan di `virtual_relations`.
//!
//! Modul ini murni (tanpa I/O) supaya mudah dites.

use crate::models::structs::{
    DiagramEdge, DiagramGroup, DiagramNode, DiagramState, LinkStatus, LinkedDatabase,
    VirtualRelation,
};
use eframe::egui;
use std::collections::{HashMap, HashSet};

const LINK_PREFIX: &str = "lnk_";
const SEP: &str = "::";
/// Jarak tepi kontainer ke tabel di dalamnya.
pub const CONTAINER_PAD: f32 = 30.0;
/// Tinggi header kontainer.
pub const CONTAINER_HEADER: f32 = 36.0;
/// Jarak header kontainer ke tabel teratas; menyisakan ruang untuk header
/// group (padding + judul group ~66px) supaya tidak menimpa header kontainer.
pub const INNER_TOP: f32 = CONTAINER_HEADER + 72.0;
/// Ukuran kontainer placeholder untuk link yang belum/gagal dimuat.
pub const PLACEHOLDER_SIZE: egui::Vec2 = egui::vec2(340.0, 150.0);
/// Jarak horizontal antar kontainer saat menempatkan link baru.
const CONTAINER_GAP: f32 = 120.0;

/// Buat `link_id` acak yang belum dipakai.
pub fn new_link_id(existing: &[LinkedDatabase]) -> String {
    use rand::RngExt;
    loop {
        let mut bytes = [0u8; 4];
        rand::rng().fill(&mut bytes);
        let id = format!(
            "{LINK_PREFIX}{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        if !existing.iter().any(|l| l.link_id == id) {
            return id;
        }
    }
}

pub fn namespaced(link_id: &str, id: &str) -> String {
    format!("{link_id}{SEP}{id}")
}

/// `link_id` pemilik sebuah id node/group, atau `None` untuk item host.
pub fn link_id_of(id: &str) -> Option<&str> {
    let (prefix, _) = id.split_once(SEP)?;
    prefix.starts_with(LINK_PREFIX).then_some(prefix)
}

pub fn is_linked_id(id: &str) -> bool {
    link_id_of(id).is_some()
}

/// Nama tabel asli tanpa namespace link.
pub fn local_name(id: &str) -> &str {
    match link_id_of(id) {
        Some(link) => &id[link.len() + SEP.len()..],
        None => id,
    }
}

fn owned_by(id: &str, link_id: &str) -> bool {
    link_id_of(id) == Some(link_id)
}

/// Buang semua item hasil materialisasi link (node, group, edge, relasi
/// bawaan sumber). Relasi milik host tidak disentuh.
pub fn strip_linked(state: &mut DiagramState) {
    state.nodes.retain(|n| !is_linked_id(&n.id));
    state.groups.retain(|g| !is_linked_id(&g.id));
    state
        .edges
        .retain(|e| !is_linked_id(&e.source) && !is_linked_id(&e.target));
    state.linked_relations.clear();
}

/// Salinan state yang aman disimpan: tanpa item hasil materialisasi link.
pub fn persistable(state: &DiagramState) -> DiagramState {
    let mut out = state.clone();
    strip_linked(&mut out);
    out
}

fn remove_link_items(state: &mut DiagramState, link_id: &str) {
    state.nodes.retain(|n| !owned_by(&n.id, link_id));
    state.groups.retain(|g| !owned_by(&g.id, link_id));
    state
        .edges
        .retain(|e| !owned_by(&e.source, link_id) && !owned_by(&e.target, link_id));
    state
        .linked_relations
        .retain(|r| !owned_by(&r.child, link_id) && !owned_by(&r.parent, link_id));
}

fn bbox<'a>(nodes: impl Iterator<Item = &'a DiagramNode>) -> Option<egui::Rect> {
    nodes.fold(None, |acc: Option<egui::Rect>, n| {
        let r = egui::Rect::from_min_size(n.pos, n.size);
        Some(acc.map_or(r, |a| a.union(r)))
    })
}

/// Isi kontainer `link_id` dari `source` (diagram database sumber).
/// Item link yang lama diganti; item link bersarang di sumber diabaikan
/// (kedalaman link hanya satu level, mencegah siklus A <-> B).
pub fn apply_link(host: &mut DiagramState, link_id: &str, source: &DiagramState) {
    let Some(li) = host
        .linked_databases
        .iter()
        .position(|l| l.link_id == link_id)
    else {
        return;
    };
    remove_link_items(host, link_id);
    let link = host.linked_databases[li].clone();
    let ns = |id: &str| namespaced(link_id, id);

    let src_nodes: Vec<&DiagramNode> = source
        .nodes
        .iter()
        .filter(|n| !is_linked_id(&n.id))
        .collect();
    let src_ids: HashSet<&str> = src_nodes.iter().map(|n| n.id.as_str()).collect();
    let src_min = bbox(src_nodes.iter().copied())
        .map(|r| r.min)
        .unwrap_or(egui::Pos2::ZERO);
    let origin = link.offset + egui::vec2(CONTAINER_PAD, INNER_TOP);
    let place = |p: egui::Pos2| origin + (p - src_min);

    for g in source.groups.iter().filter(|g| !is_linked_id(&g.id)) {
        host.groups.push(DiagramGroup {
            id: ns(&g.id),
            title: g.title.clone(),
            color: g.color,
            manual_pos: g.manual_pos.map(place),
        });
    }

    for n in src_nodes {
        let mut node = n.clone();
        node.id = ns(&n.id);
        node.pos = place(n.pos);
        node.group_ids = n.group_ids.iter().map(|g| ns(g)).collect();
        node.group_id = n.group_id.as_deref().map(ns);
        for fk in &mut node.foreign_keys {
            fk.table_name = ns(&fk.table_name);
            fk.referenced_table_name = ns(&fk.referenced_table_name);
        }
        node.database_name = Some(link.database_name.clone());
        node.connection_id = link.connection_id;
        node.connection_name = Some(link.connection_name.clone());
        host.nodes.push(node);
    }

    host.edges.extend(
        source
            .edges
            .iter()
            .filter(|e| src_ids.contains(e.source.as_str()) && src_ids.contains(e.target.as_str()))
            .map(|e| DiagramEdge {
                source: ns(&e.source),
                target: ns(&e.target),
                label: e.label.clone(),
            }),
    );

    host.linked_relations.extend(
        source
            .virtual_relations
            .iter()
            .filter(|r| src_ids.contains(r.child.as_str()) && src_ids.contains(r.parent.as_str()))
            .map(|r| VirtualRelation {
                child: ns(&r.child),
                parent: ns(&r.parent),
                ..r.clone()
            }),
    );

    host.linked_databases[li].status = LinkStatus::Loaded;
    prune_virtual_relations(host);
}

/// Tandai link gagal dimuat. Item lamanya dibuang, tapi relasi lintas
/// database milik host dibiarkan dorman supaya tidak hilang saat disimpan.
pub fn mark_link_failed(host: &mut DiagramState, link_id: &str, error: String) {
    remove_link_items(host, link_id);
    if let Some(l) = host
        .linked_databases
        .iter_mut()
        .find(|l| l.link_id == link_id)
    {
        l.status = LinkStatus::Failed(error);
    }
}

/// Buang relasi milik host yang ujungnya sudah tidak ada. Ujung milik link
/// yang belum/gagal dimuat dibiarkan; ujung milik link yang sudah di-unlink
/// ikut dibuang.
pub fn prune_virtual_relations(state: &mut DiagramState) {
    let ids: HashSet<&str> = state.nodes.iter().map(|n| n.id.as_str()).collect();
    let status: HashMap<&str, &LinkStatus> = state
        .linked_databases
        .iter()
        .map(|l| (l.link_id.as_str(), &l.status))
        .collect();
    let alive = |id: &str| match link_id_of(id) {
        None => ids.contains(id),
        Some(link) => match status.get(link) {
            None => false,
            Some(LinkStatus::Loaded) => ids.contains(id),
            Some(_) => true,
        },
    };
    let keep: Vec<bool> = state
        .virtual_relations
        .iter()
        .map(|r| alive(&r.child) && alive(&r.parent))
        .collect();
    let mut it = keep.into_iter();
    state
        .virtual_relations
        .retain(|_| it.next().unwrap_or(true));
}

/// Lepas link beserta isi kontainernya dan relasi host yang merujuknya.
pub fn unlink(state: &mut DiagramState, link_id: &str) {
    remove_link_items(state, link_id);
    state.linked_databases.retain(|l| l.link_id != link_id);
    prune_virtual_relations(state);
    state.selected_virtual = None;
    state.selected_edge = None;
    if state
        .selected_column
        .as_ref()
        .is_some_and(|(t, _)| owned_by(t, link_id))
    {
        state.selected_column = None;
    }
}

/// Batas kontainer tabel host (koordinat diagram), `None` bila kosong.
pub fn host_rect(state: &DiagramState) -> Option<egui::Rect> {
    bbox(state.nodes.iter().filter(|n| !is_linked_id(&n.id))).map(|r| {
        egui::Rect::from_min_max(
            r.min - egui::vec2(CONTAINER_PAD, INNER_TOP),
            r.max + egui::vec2(CONTAINER_PAD, CONTAINER_PAD),
        )
    })
}

/// Batas kontainer sebuah link (koordinat diagram). Link tanpa node
/// (belum/gagal dimuat, atau database kosong) tampil sebagai placeholder.
pub fn container_rect(state: &DiagramState, link: &LinkedDatabase) -> egui::Rect {
    match bbox(
        state
            .nodes
            .iter()
            .filter(|n| owned_by(&n.id, &link.link_id)),
    ) {
        Some(r) => egui::Rect::from_min_max(
            link.offset,
            r.max + egui::vec2(CONTAINER_PAD, CONTAINER_PAD),
        ),
        None => egui::Rect::from_min_size(link.offset, PLACEHOLDER_SIZE),
    }
}

/// Posisi kontainer untuk link baru: di kanan semua konten yang ada.
pub fn next_link_offset(state: &DiagramState) -> egui::Pos2 {
    let mut right = f32::MIN;
    let mut top = f32::MAX;
    if let Some(r) = host_rect(state) {
        right = right.max(r.max.x);
        top = top.min(r.min.y);
    }
    for link in &state.linked_databases {
        let r = container_rect(state, link);
        right = right.max(r.max.x);
        top = top.min(r.min.y);
    }
    if right == f32::MIN {
        egui::pos2(50.0, 50.0)
    } else {
        egui::pos2(right + CONTAINER_GAP, top)
    }
}

/// Geser seluruh kontainer link (offset, tabel, dan group kosongnya).
pub fn move_link(state: &mut DiagramState, link_id: &str, delta: egui::Vec2) {
    if let Some(l) = state
        .linked_databases
        .iter_mut()
        .find(|l| l.link_id == link_id)
    {
        l.offset += delta;
    }
    for n in state.nodes.iter_mut().filter(|n| owned_by(&n.id, link_id)) {
        n.pos += delta;
    }
    for g in state.groups.iter_mut().filter(|g| owned_by(&g.id, link_id)) {
        if let Some(p) = &mut g.manual_pos {
            *p += delta;
        }
    }
}

/// Geser seluruh tabel host (dan group kosongnya).
pub fn move_host(state: &mut DiagramState, delta: egui::Vec2) {
    for n in state.nodes.iter_mut().filter(|n| !is_linked_id(&n.id)) {
        n.pos += delta;
    }
    for g in state.groups.iter_mut().filter(|g| !is_linked_id(&g.id)) {
        if let Some(p) = &mut g.manual_pos {
            *p += delta;
        }
    }
}

/// Susun ulang kontainer link berjajar di kanan tabel host (dipakai setelah
/// auto-arrange supaya kontainer tidak menimpa tabel host).
pub fn restack_links(state: &mut DiagramState) {
    let mut x = host_rect(state).map_or(50.0, |r| r.max.x + CONTAINER_GAP);
    let y = host_rect(state).map_or(50.0, |r| r.min.y);
    let ids: Vec<String> = state
        .linked_databases
        .iter()
        .map(|l| l.link_id.clone())
        .collect();
    for id in ids {
        let Some(link) = state.linked_databases.iter().find(|l| l.link_id == id) else {
            continue;
        };
        let rect = container_rect(state, link);
        let delta = egui::pos2(x, y) - link.offset;
        move_link(state, &id, delta);
        x += rect.width() + CONTAINER_GAP;
    }
}

/// Migrasi diagram lama hasil "Add Tables": tabel dari database lain yang
/// dulu disalin ke diagram host diubah menjadi link database. Relasi virtual
/// ke tabel tersebut dipetakan ke id namespace baru. Mengembalikan jumlah
/// link yang dibuat.
pub fn migrate_legacy_foreign_nodes(
    state: &mut DiagramState,
    host_conn: i64,
    host_db: &str,
    color_for: impl Fn(usize) -> egui::Color32,
) -> usize {
    let is_foreign = |n: &DiagramNode| {
        !n.detached
            && !is_linked_id(&n.id)
            && (n.database_name.as_deref().is_some_and(|d| d != host_db)
                || n.connection_id.is_some_and(|c| c != host_conn))
    };
    if !state.nodes.iter().any(is_foreign) {
        return 0;
    }

    // Kelompokkan per (koneksi, database), urutan kemunculan dipertahankan.
    let mut buckets: Vec<((Option<i64>, String), Vec<DiagramNode>)> = Vec::new();
    let mut kept = Vec::with_capacity(state.nodes.len());
    for n in std::mem::take(&mut state.nodes) {
        if !is_foreign(&n) {
            kept.push(n);
            continue;
        }
        let key = (n.connection_id, n.database_name.clone().unwrap_or_default());
        match buckets.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => v.push(n),
            None => buckets.push((key, vec![n])),
        }
    }
    state.nodes = kept;

    let mut id_map: HashMap<String, String> = HashMap::new();
    let mut created = 0;
    for ((conn_id, db), nodes) in buckets {
        let existing = state
            .linked_databases
            .iter()
            .find(|l| l.database_name == db && l.connection_id == conn_id);
        let link_id = match existing {
            Some(l) => l.link_id.clone(),
            None => {
                let link_id = new_link_id(&state.linked_databases);
                let offset = bbox(nodes.iter())
                    .map(|r| r.min - egui::vec2(CONTAINER_PAD, INNER_TOP))
                    .unwrap_or_else(|| next_link_offset(state));
                let color = color_for(state.linked_databases.len());
                state.linked_databases.push(LinkedDatabase {
                    link_id: link_id.clone(),
                    connection_id: conn_id,
                    connection_name: nodes
                        .iter()
                        .find_map(|n| n.connection_name.clone())
                        .unwrap_or_default(),
                    database_name: db.clone(),
                    offset,
                    color,
                    status: LinkStatus::Pending,
                });
                created += 1;
                link_id
            }
        };
        for n in &nodes {
            id_map.insert(n.id.clone(), namespaced(&link_id, &n.title));
        }
        // Group otomatis "DB: x" buatan Add Tables sudah digantikan kontainer.
        let legacy_group = format!("group_{}", db.replace(' ', "_"));
        state.groups.retain(|g| g.id != legacy_group);
    }

    state
        .edges
        .retain(|e| !id_map.contains_key(&e.source) && !id_map.contains_key(&e.target));
    for r in &mut state.virtual_relations {
        if let Some(new) = id_map.get(&r.child) {
            r.child = new.clone();
        }
        if let Some(new) = id_map.get(&r.parent) {
            r.parent = new.clone();
        }
    }
    created
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{ForeignKey, RelationOrigin};

    fn node(id: &str, x: f32, y: f32) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: egui::pos2(x, y),
            columns: vec!["id".into(), "user_id".into()],
            ..Default::default()
        }
    }

    fn rel(child: &str, parent: &str) -> VirtualRelation {
        VirtualRelation {
            child: child.into(),
            child_column: "user_id".into(),
            parent: parent.into(),
            parent_column: "id".into(),
            origin: RelationOrigin::Manual,
        }
    }

    fn link(id: &str) -> LinkedDatabase {
        LinkedDatabase {
            link_id: id.into(),
            connection_id: Some(2),
            connection_name: "Other".into(),
            database_name: "auth".into(),
            offset: egui::pos2(1000.0, 0.0),
            color: egui::Color32::RED,
            status: LinkStatus::Pending,
        }
    }

    fn source() -> DiagramState {
        let mut users = node("users", 500.0, 500.0);
        users.group_ids = vec!["group_auth".into()];
        let mut sessions = node("sessions", 800.0, 500.0);
        sessions.foreign_keys.push(ForeignKey {
            constraint_name: "fk".into(),
            table_name: "sessions".into(),
            column_name: "user_id".into(),
            referenced_table_name: "users".into(),
            referenced_column_name: "id".into(),
        });
        DiagramState {
            nodes: vec![users, sessions],
            edges: vec![DiagramEdge {
                source: "sessions".into(),
                target: "users".into(),
                label: String::new(),
            }],
            groups: vec![DiagramGroup {
                id: "group_auth".into(),
                title: "Auth".into(),
                color: egui::Color32::BLUE,
                manual_pos: None,
            }],
            virtual_relations: vec![rel("sessions", "users")],
            ..Default::default()
        }
    }

    fn host_with_link() -> DiagramState {
        let mut host = DiagramState {
            nodes: vec![node("orders", 0.0, 0.0)],
            linked_databases: vec![link("lnk_aaaa")],
            ..Default::default()
        };
        apply_link(&mut host, "lnk_aaaa", &source());
        host
    }

    #[test]
    fn link_id_parsing() {
        assert_eq!(link_id_of("lnk_ab12::users"), Some("lnk_ab12"));
        assert_eq!(link_id_of("users"), None);
        // Id lama "db::table" dari Add Tables bukan milik link.
        assert_eq!(link_id_of("auth::users"), None);
        assert_eq!(local_name("lnk_ab12::users"), "users");
        assert_eq!(local_name("users"), "users");
    }

    #[test]
    fn apply_link_namespaces_everything_and_places_at_offset() {
        let host = host_with_link();
        let users = host
            .nodes
            .iter()
            .find(|n| n.id == "lnk_aaaa::users")
            .unwrap();
        assert_eq!(users.pos, egui::pos2(1000.0 + CONTAINER_PAD, INNER_TOP));
        assert_eq!(users.group_ids, vec!["lnk_aaaa::group_auth".to_string()]);
        assert_eq!(users.database_name.as_deref(), Some("auth"));
        let sessions = host
            .nodes
            .iter()
            .find(|n| n.id == "lnk_aaaa::sessions")
            .unwrap();
        assert!(sessions.is_fk_column("user_id"));
        assert_eq!(
            sessions.foreign_keys[0].referenced_table_name,
            "lnk_aaaa::users"
        );
        assert!(host.groups.iter().any(|g| g.id == "lnk_aaaa::group_auth"));
        assert_eq!(host.edges[0].source, "lnk_aaaa::sessions");
        assert_eq!(host.linked_relations[0].parent, "lnk_aaaa::users");
        assert!(host.virtual_relations.is_empty());
        assert_eq!(host.linked_databases[0].status, LinkStatus::Loaded);
    }

    #[test]
    fn reapply_replaces_instead_of_duplicating() {
        let mut host = host_with_link();
        let mut src = source();
        src.nodes.retain(|n| n.id != "sessions");
        apply_link(&mut host, "lnk_aaaa", &src);
        assert_eq!(host.nodes.len(), 2); // orders + users
        assert!(host.edges.is_empty());
        assert!(host.linked_relations.is_empty());
    }

    #[test]
    fn nested_links_in_source_are_ignored() {
        let mut src = source();
        src.nodes.push(node("lnk_bbbb::nested", 0.0, 0.0));
        let mut host = DiagramState {
            linked_databases: vec![link("lnk_aaaa")],
            ..Default::default()
        };
        apply_link(&mut host, "lnk_aaaa", &src);
        assert!(!host.nodes.iter().any(|n| n.id.contains("nested")));
    }

    #[test]
    fn cross_database_relation_survives_persist_and_rematerialize() {
        let mut host = host_with_link();
        host.virtual_relations
            .push(rel("orders", "lnk_aaaa::users"));

        let saved = persistable(&host);
        assert_eq!(saved.nodes.len(), 1, "linked tables must not be persisted");
        assert!(saved.groups.is_empty());
        assert!(saved.edges.is_empty());
        assert!(saved.linked_relations.is_empty());
        assert_eq!(saved.virtual_relations.len(), 1);

        let json = serde_json::to_string(&saved).unwrap();
        let mut reopened: DiagramState = serde_json::from_str(&json).unwrap();
        assert_eq!(reopened.linked_databases[0].status, LinkStatus::Pending);
        apply_link(&mut reopened, "lnk_aaaa", &source());
        assert_eq!(
            reopened.virtual_relations,
            vec![rel("orders", "lnk_aaaa::users")]
        );
    }

    #[test]
    fn relation_to_failed_link_stays_dormant() {
        let mut state = DiagramState {
            nodes: vec![node("orders", 0.0, 0.0)],
            linked_databases: vec![link("lnk_aaaa")],
            virtual_relations: vec![rel("orders", "lnk_aaaa::users")],
            ..Default::default()
        };
        mark_link_failed(&mut state, "lnk_aaaa", "offline".into());
        prune_virtual_relations(&mut state);
        assert_eq!(state.virtual_relations.len(), 1);
    }

    #[test]
    fn relation_to_dropped_table_is_pruned_once_link_loads() {
        let mut host = host_with_link();
        host.virtual_relations
            .push(rel("orders", "lnk_aaaa::users"));
        let mut src = source();
        src.nodes.retain(|n| n.id != "users");
        apply_link(&mut host, "lnk_aaaa", &src);
        assert!(host.virtual_relations.is_empty());
    }

    #[test]
    fn host_relation_inside_one_linked_database_is_kept_by_host() {
        let mut host = host_with_link();
        host.virtual_relations
            .push(rel("lnk_aaaa::users", "lnk_aaaa::sessions"));
        apply_link(&mut host, "lnk_aaaa", &source());
        assert_eq!(persistable(&host).virtual_relations.len(), 1);
    }

    #[test]
    fn relink_to_other_connection_keeps_relations() {
        let mut host = host_with_link();
        host.virtual_relations
            .push(rel("orders", "lnk_aaaa::users"));
        host.linked_databases[0].connection_id = Some(99);
        host.linked_databases[0].connection_name = "Renamed".into();
        apply_link(&mut host, "lnk_aaaa", &source());
        assert_eq!(host.virtual_relations.len(), 1);
        let users = host
            .nodes
            .iter()
            .find(|n| n.id == "lnk_aaaa::users")
            .unwrap();
        assert_eq!(users.connection_id, Some(99));
    }

    #[test]
    fn unlink_removes_items_and_relations() {
        let mut host = host_with_link();
        host.virtual_relations
            .push(rel("orders", "lnk_aaaa::users"));
        unlink(&mut host, "lnk_aaaa");
        assert_eq!(host.nodes.len(), 1);
        assert!(host.groups.is_empty() && host.edges.is_empty());
        assert!(host.linked_databases.is_empty());
        assert!(host.virtual_relations.is_empty());
    }

    #[test]
    fn move_link_shifts_offset_and_nodes() {
        let mut host = host_with_link();
        let before = host
            .nodes
            .iter()
            .find(|n| n.id == "lnk_aaaa::users")
            .unwrap()
            .pos;
        move_link(&mut host, "lnk_aaaa", egui::vec2(10.0, 20.0));
        let after = host
            .nodes
            .iter()
            .find(|n| n.id == "lnk_aaaa::users")
            .unwrap()
            .pos;
        assert_eq!(after - before, egui::vec2(10.0, 20.0));
        assert_eq!(host.linked_databases[0].offset, egui::pos2(1010.0, 20.0));
        assert_eq!(
            host.nodes[0].pos,
            egui::pos2(0.0, 0.0),
            "host table untouched"
        );
    }

    #[test]
    fn next_offset_is_right_of_existing_content() {
        let host = host_with_link();
        let off = next_link_offset(&host);
        let rect = container_rect(&host, &host.linked_databases[0]);
        assert!(off.x > rect.max.x);
    }

    #[test]
    fn migrates_legacy_add_tables_nodes_into_link() {
        let mut foreign = node("users", 900.0, 100.0);
        foreign.database_name = Some("auth".into());
        foreign.connection_id = Some(2);
        foreign.connection_name = Some("Other".into());
        foreign.group_ids = vec!["group_auth".into()];
        let mut clash = node("auth::orders", 900.0, 400.0);
        clash.title = "orders".into();
        clash.database_name = Some("auth".into());
        clash.connection_id = Some(2);
        let mut own = node("orders", 0.0, 0.0);
        own.database_name = Some("shop".into());
        own.connection_id = Some(1);

        let mut state = DiagramState {
            nodes: vec![own, foreign, clash],
            groups: vec![DiagramGroup {
                id: "group_auth".into(),
                title: "DB: auth".into(),
                color: egui::Color32::BLUE,
                manual_pos: None,
            }],
            edges: vec![DiagramEdge {
                source: "users".into(),
                target: "auth::orders".into(),
                label: String::new(),
            }],
            virtual_relations: vec![rel("orders", "users")],
            ..Default::default()
        };
        let created = migrate_legacy_foreign_nodes(&mut state, 1, "shop", |_| egui::Color32::GREEN);
        assert_eq!(created, 1);
        assert_eq!(state.nodes.len(), 1);
        assert!(state.groups.is_empty());
        assert!(state.edges.is_empty());
        let link = &state.linked_databases[0];
        assert_eq!(link.database_name, "auth");
        assert_eq!(link.connection_name, "Other");
        assert_eq!(
            state.virtual_relations[0].parent,
            namespaced(&link.link_id, "users")
        );
        // Tidak ada yang dimigrasi dua kali.
        assert_eq!(
            migrate_legacy_foreign_nodes(&mut state, 1, "shop", |_| egui::Color32::GREEN),
            0
        );
    }
}
