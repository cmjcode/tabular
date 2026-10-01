//! Sinkronisasi diagram antara salinan lokal dan tabel `diagram_by_tabular`,
//! bergaya git: tiga versi dibandingkan (lokal, base, database). Base adalah
//! versi database pada revision terakhir yang diketahui klien ini.
//!
//! Perubahan yang hanya terjadi di satu sisi digabung otomatis per entitas
//! (tabel, grup, relasi, note, flow, link). Entitas yang diubah di kedua sisi
//! dengan hasil berbeda menjadi [`Conflict`] yang dipilih user.
//!
//! Modul ini headless: hanya bekerja pada `DiagramState` / JSON.

use serde_json::{Map, Value};

use crate::models::structs::DiagramState;

/// Field preferensi tampilan lokal: tidak dihitung sebagai perubahan dan
/// selalu memakai nilai lokal saat merge.
const LOCAL_ONLY_FIELDS: &[&str] = &[
    "pan",
    "zoom",
    "is_centered",
    "edges",
    "show_grid",
    "prevent_overlap",
    "show_relations",
    "show_fk_relations",
    "show_virtual_relations",
    "show_linked_relations",
    "show_notes",
    "show_endpoints",
    "endpoint_display",
    "flow_lines",
    "flow_show_steps",
    "auto_save",
    "remote_id",
];

/// Koleksi yang di-merge per item.
struct Collection {
    field: &'static str,
    /// Nama jenis item untuk popup merge.
    kind: &'static str,
    key: fn(&Value) -> Option<String>,
    /// Field item yang dibandingkan; kosong = seluruh item.
    compare: &'static [&'static str],
}

fn str_field(v: &Value, f: &str) -> String {
    v.get(f).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn key_id(v: &Value) -> Option<String> {
    v.get("id").and_then(Value::as_str).map(str::to_string)
}

fn key_link(v: &Value) -> Option<String> {
    v.get("link_id").and_then(Value::as_str).map(str::to_string)
}

fn key_relation(v: &Value) -> Option<String> {
    Some(format!(
        "{}.{} → {}.{}",
        str_field(v, "child"),
        str_field(v, "child_column"),
        str_field(v, "parent"),
        str_field(v, "parent_column")
    ))
}

fn key_endpoint(v: &Value) -> Option<String> {
    Some(format!(
        "{} {} ({})",
        str_field(v, "method"),
        str_field(v, "path"),
        str_field(v, "table")
    ))
}

const COLLECTIONS: &[Collection] = &[
    Collection {
        field: "nodes",
        kind: "Table",
        key: key_id,
        // Kolom dan FK berasal dari skema live, bukan hasil edit user.
        compare: &["pos", "size", "group_ids", "group_id", "detached"],
    },
    Collection {
        field: "groups",
        kind: "Group",
        key: key_id,
        compare: &[],
    },
    Collection {
        field: "virtual_relations",
        kind: "Relation",
        key: key_relation,
        compare: &[],
    },
    Collection {
        field: "notes",
        kind: "Note",
        key: key_id,
        compare: &[],
    },
    Collection {
        field: "flow_cards",
        kind: "Flow",
        key: key_id,
        compare: &[],
    },
    Collection {
        field: "linked_databases",
        kind: "Linked database",
        key: key_link,
        compare: &[],
    },
    Collection {
        field: "endpoint_links",
        kind: "Endpoint",
        key: key_endpoint,
        compare: &[],
    },
];

fn collection(field: &str) -> Option<&'static Collection> {
    COLLECTIONS.iter().find(|c| c.field == field)
}

/// Serialisasi state yang disimpan (tanpa isi link database).
pub fn to_value(state: &DiagramState) -> Value {
    serde_json::to_value(crate::diagram_links::persistable(state)).unwrap_or(Value::Null)
}

pub fn from_value(value: Value) -> Result<DiagramState, String> {
    serde_json::from_value(value).map_err(|e| format!("invalid merged diagram: {e}"))
}

fn project(item: &Value, compare: &[&str]) -> Value {
    if compare.is_empty() {
        return item.clone();
    }
    let mut out = Map::new();
    for f in compare {
        if let Some(v) = item.get(*f) {
            out.insert((*f).to_string(), v.clone());
        }
    }
    Value::Object(out)
}

/// Bentuk pembanding: tanpa field lokal, koleksi menjadi map berkunci
/// (urutan tidak berpengaruh) dan item diproyeksikan ke field yang relevan.
fn normalize(v: &Value) -> Value {
    let Some(obj) = v.as_object() else {
        return v.clone();
    };
    let mut out = Map::new();
    for (k, val) in obj {
        if LOCAL_ONLY_FIELDS.contains(&k.as_str()) {
            continue;
        }
        let normalized = match (collection(k), val.as_array()) {
            (Some(c), Some(items)) => {
                let mut m = Map::new();
                for item in items {
                    if let Some(key) = (c.key)(item) {
                        m.insert(key, project(item, c.compare));
                    }
                }
                Value::Object(m)
            }
            _ => val.clone(),
        };
        // Koleksi kosong sama dengan field yang tidak diserialisasi.
        let empty = match &normalized {
            Value::Object(m) => m.is_empty() && collection(k).is_some(),
            Value::Array(a) => a.is_empty(),
            Value::Null => true,
            _ => false,
        };
        if !empty {
            out.insert(k.clone(), normalized);
        }
    }
    Value::Object(out)
}

/// Isi dua diagram sama (mengabaikan pan/zoom dan preferensi tampilan).
pub fn same_content(a: &DiagramState, b: &DiagramState) -> bool {
    normalize(&to_value(a)) == normalize(&to_value(b))
}

/// Tindakan sinkronisasi hasil perbandingan tiga versi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPlan {
    /// Lokal dan database sama.
    UpToDate,
    /// Hanya database yang berubah: pakai versi database.
    TakeRemote,
    /// Hanya lokal yang berubah: kirim ke database.
    PushLocal,
    /// Keduanya berubah (atau belum ada base): perlu merge.
    Merge,
}

pub fn classify(
    local: &DiagramState,
    base: Option<&DiagramState>,
    remote: &DiagramState,
) -> SyncPlan {
    let l = normalize(&to_value(local));
    let r = normalize(&to_value(remote));
    if l == r {
        return SyncPlan::UpToDate;
    }
    let Some(base) = base else {
        return SyncPlan::Merge;
    };
    let b = normalize(&to_value(base));
    if l == b {
        SyncPlan::TakeRemote
    } else if r == b {
        SyncPlan::PushLocal
    } else {
        SyncPlan::Merge
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

/// Satu entitas yang diubah berbeda di lokal dan database.
#[derive(Clone, Debug)]
pub struct Conflict {
    /// Field `DiagramState` (mis. `nodes`).
    pub field: String,
    /// Kunci item dalam koleksi; `None` untuk field tunggal.
    pub key: Option<String>,
    /// Jenis untuk pengelompokan di popup (Table, Group, Setting, ...).
    pub kind: &'static str,
    pub label: String,
    pub local: Option<Value>,
    pub remote: Option<Value>,
    /// Entitas sudah ada di base (untuk membedakan "added" dan "changed").
    pub in_base: bool,
    pub choice: Side,
}

impl Conflict {
    /// Ringkasan perubahan di satu sisi, mis. "changed" / "deleted".
    pub fn describe(&self, side: Side) -> &'static str {
        let value = match side {
            Side::Local => &self.local,
            Side::Remote => &self.remote,
        };
        match (value, self.in_base) {
            (None, _) => "deleted",
            (Some(_), false) => "added",
            (Some(_), true) => "changed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct MergeResult {
    /// Hasil merge dengan setiap konflik sementara memakai versi lokal.
    pub merged: Value,
    pub conflicts: Vec<Conflict>,
    /// Jumlah perubahan database yang diambil otomatis.
    pub auto_merged: usize,
}

enum Decision {
    Local,
    Remote,
    Conflict,
}

fn decide(
    b: Option<&Value>,
    l: Option<&Value>,
    r: Option<&Value>,
    eq: impl Fn(Option<&Value>, Option<&Value>) -> bool,
) -> Decision {
    if eq(l, r) {
        Decision::Local
    } else if eq(l, b) {
        Decision::Remote
    } else if eq(r, b) {
        Decision::Local
    } else {
        Decision::Conflict
    }
}

fn item_label(c: &Collection, key: &str, item: Option<&Value>) -> String {
    let name = item
        .and_then(|v| {
            ["title", "database_name", "summary"]
                .iter()
                .find_map(|f| v.get(*f).and_then(Value::as_str).filter(|s| !s.is_empty()))
        })
        .unwrap_or(key);
    format!("{} '{}'", c.kind, name)
}

fn merge_collection(
    c: &Collection,
    base: Option<&Value>,
    local: Option<&Value>,
    remote: Option<&Value>,
    conflicts: &mut Vec<Conflict>,
    auto_merged: &mut usize,
) -> Value {
    let items = |v: Option<&Value>| -> Vec<Value> {
        v.and_then(Value::as_array).cloned().unwrap_or_default()
    };
    let (b_items, l_items, r_items) = (items(base), items(local), items(remote));
    let find = |list: &[Value], key: &str| -> Option<Value> {
        list.iter()
            .find(|v| (c.key)(v).as_deref() == Some(key))
            .cloned()
    };
    let eq = |x: Option<&Value>, y: Option<&Value>| {
        x.map(|v| project(v, c.compare)) == y.map(|v| project(v, c.compare))
    };

    // Urutan: urutan lokal, lalu item baru dari database.
    let mut keys: Vec<String> = Vec::new();
    let mut out: Vec<Value> = Vec::new();
    for item in &l_items {
        match (c.key)(item) {
            Some(k) if !keys.contains(&k) => keys.push(k),
            Some(_) => {}
            // Item tanpa kunci tidak bisa dicocokkan: pertahankan versi lokal.
            None => out.push(item.clone()),
        }
    }
    for item in &r_items {
        if let Some(k) = (c.key)(item)
            && !keys.contains(&k)
        {
            keys.push(k);
        }
    }

    for key in keys {
        let (b, l, r) = (
            find(&b_items, &key),
            find(&l_items, &key),
            find(&r_items, &key),
        );
        match decide(b.as_ref(), l.as_ref(), r.as_ref(), eq) {
            Decision::Local => out.extend(l),
            Decision::Remote => {
                *auto_merged += 1;
                out.extend(r);
            }
            Decision::Conflict => {
                conflicts.push(Conflict {
                    field: c.field.to_string(),
                    key: Some(key.clone()),
                    kind: c.kind,
                    label: item_label(c, &key, l.as_ref().or(r.as_ref())),
                    local: l.clone(),
                    remote: r,
                    in_base: b.is_some(),
                    choice: Side::Local,
                });
                out.extend(l);
            }
        }
    }
    Value::Array(out)
}

/// Merge tiga arah. `base = None` berarti belum pernah sinkron: item yang
/// hanya ada di satu sisi digabung, yang berbeda di kedua sisi jadi konflik.
pub fn three_way_merge(
    base: Option<&DiagramState>,
    local: &DiagramState,
    remote: &DiagramState,
) -> MergeResult {
    let base_v = base.map(to_value);
    let local_v = to_value(local);
    let remote_v = to_value(remote);
    let obj = |v: Option<&Value>| v.and_then(Value::as_object).cloned().unwrap_or_default();
    let (b, l, r) = (obj(base_v.as_ref()), obj(Some(&local_v)), obj(Some(&remote_v)));

    let mut merged = l.clone();
    let mut conflicts = Vec::new();
    let mut auto_merged = 0;

    let mut fields: Vec<&String> = l.keys().collect();
    for k in r.keys() {
        if !fields.contains(&k) {
            fields.push(k);
        }
    }
    for field in fields {
        if LOCAL_ONLY_FIELDS.contains(&field.as_str()) {
            continue;
        }
        let (bf, lf, rf) = (b.get(field), l.get(field), r.get(field));
        if let Some(c) = collection(field) {
            let v = merge_collection(c, bf, lf, rf, &mut conflicts, &mut auto_merged);
            merged.insert(field.clone(), v);
            continue;
        }
        // Null dan field yang tidak ada dianggap sama.
        let norm = |v: Option<&Value>| v.filter(|v| !v.is_null()).cloned();
        let eq = |x: Option<&Value>, y: Option<&Value>| norm(x) == norm(y);
        match decide(bf, lf, rf, eq) {
            Decision::Local => {}
            Decision::Remote => {
                auto_merged += 1;
                match rf {
                    Some(v) => merged.insert(field.clone(), v.clone()),
                    None => merged.remove(field),
                };
            }
            Decision::Conflict => conflicts.push(Conflict {
                field: field.clone(),
                key: None,
                kind: "Setting",
                label: field.replace('_', " "),
                local: lf.cloned(),
                remote: rf.cloned(),
                in_base: bf.is_some(),
                choice: Side::Local,
            }),
        }
    }

    MergeResult {
        merged: Value::Object(merged),
        conflicts,
        auto_merged,
    }
}

/// Terapkan pilihan setiap konflik ke hasil merge.
pub fn resolve(result: &MergeResult) -> Result<DiagramState, String> {
    let mut merged = result.merged.clone();
    let Some(obj) = merged.as_object_mut() else {
        return Err("merged diagram is not an object".to_string());
    };
    for c in result.conflicts.iter().filter(|c| c.choice == Side::Remote) {
        match (&c.key, collection(&c.field)) {
            (Some(key), Some(spec)) => {
                let list = obj
                    .entry(c.field.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                let Some(items) = list.as_array_mut() else {
                    continue;
                };
                let pos = items
                    .iter()
                    .position(|v| (spec.key)(v).as_deref() == Some(key.as_str()));
                match (pos, &c.remote) {
                    (Some(i), Some(v)) => items[i] = v.clone(),
                    (Some(i), None) => {
                        items.remove(i);
                    }
                    (None, Some(v)) => items.push(v.clone()),
                    (None, None) => {}
                }
            }
            _ => match &c.remote {
                Some(v) => {
                    obj.insert(c.field.clone(), v.clone());
                }
                None => {
                    obj.remove(&c.field);
                }
            },
        }
    }
    from_value(merged)
}

/// Pindahkan data skema live (kolom, FK, edge) dari state lama ke state
/// hasil merge. Kolom di salinan database bisa basi; tabel live yang belum
/// ada di hasil merge ditambahkan kembali seperti yang dilakukan
/// `merge_schema`.
pub fn adopt_schema_from(state: &mut DiagramState, old: &DiagramState) {
    for n in &mut state.nodes {
        if let Some(o) = old.nodes.iter().find(|o| o.id == n.id) {
            n.columns = o.columns.clone();
            n.foreign_keys = o.foreign_keys.clone();
            n.column_meta = o.column_meta.clone();
        }
    }
    for o in &old.nodes {
        if !o.detached
            && !crate::diagram_links::is_linked_id(&o.id)
            && !state.nodes.iter().any(|n| n.id == o.id)
        {
            state.nodes.push(o.clone());
        }
    }
    if !old.edges.is_empty() {
        state.edges = old.edges.clone();
    }
}

/// Salinan database pada revision terakhir yang diketahui (file base).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SyncBase {
    pub revision: i64,
    pub state: DiagramState,
}

pub fn read_base(path: &std::path::Path) -> Option<SyncBase> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(base) => Some(base),
        Err(e) => {
            log::warn!("[DIAGRAM_SYNC] ignoring unreadable base {path:?}: {e}");
            None
        }
    }
}

pub fn write_base(path: &std::path::Path, base: &SyncBase) {
    let stored = SyncBase {
        revision: base.revision,
        state: crate::diagram_links::persistable(&base.state),
    };
    match serde_json::to_vec(&stored) {
        Ok(bytes) => {
            if let Err(e) = crate::diagram_view::write_atomic(path, &bytes) {
                log::error!("[DIAGRAM_SYNC] failed to write base {path:?}: {e}");
            }
        }
        Err(e) => log::error!("[DIAGRAM_SYNC] failed to serialize base: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::{DiagramGroup, DiagramNode, RelationOrigin, VirtualRelation};
    use eframe::egui::{Color32, pos2, vec2};

    fn node(id: &str, x: f32) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            title: id.to_string(),
            pos: pos2(x, 0.0),
            size: vec2(100.0, 80.0),
            columns: vec![],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: None,
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        }
    }

    fn group(id: &str, title: &str) -> DiagramGroup {
        DiagramGroup {
            id: id.to_string(),
            title: title.to_string(),
            color: Color32::from_rgb(10, 20, 30),
            manual_pos: None,
            repo_url: None,
        }
    }

    fn base() -> DiagramState {
        DiagramState {
            nodes: vec![node("users", 0.0), node("orders", 200.0)],
            groups: vec![group("g1", "Auth")],
            ..Default::default()
        }
    }

    #[test]
    fn classify_covers_every_direction() {
        let b = base();
        assert_eq!(classify(&b, Some(&b), &b), SyncPlan::UpToDate);

        let mut changed = b.clone();
        changed.nodes[0].pos.x = 50.0;
        assert_eq!(classify(&b, Some(&b), &changed), SyncPlan::TakeRemote);
        assert_eq!(classify(&changed, Some(&b), &b), SyncPlan::PushLocal);

        let mut other = b.clone();
        other.groups[0].title = "Security".into();
        assert_eq!(classify(&changed, Some(&b), &other), SyncPlan::Merge);
        assert_eq!(classify(&changed, None, &b), SyncPlan::Merge);
        assert_eq!(classify(&b, None, &b), SyncPlan::UpToDate);
    }

    #[test]
    fn viewport_and_schema_columns_are_not_changes() {
        let b = base();
        let mut local = b.clone();
        local.pan = vec2(300.0, 10.0);
        local.zoom = 2.0;
        local.show_grid = false;
        local.nodes[0].columns = vec!["id".into(), "email".into()];
        assert_eq!(classify(&local, Some(&b), &b), SyncPlan::UpToDate);
    }

    #[test]
    fn independent_edits_merge_without_conflict() {
        let b = base();
        let mut local = b.clone();
        local.nodes[0].pos.x = 11.0; // user lokal memindah users
        local.pan = vec2(5.0, 5.0);
        let mut remote = b.clone();
        remote.nodes[1].pos.x = 999.0; // orang lain memindah orders
        remote.groups.push(group("g2", "Billing"));
        remote.virtual_relations.push(VirtualRelation {
            child: "orders".into(),
            child_column: "user_id".into(),
            parent: "users".into(),
            parent_column: "id".into(),
            origin: RelationOrigin::Manual,
        });

        let result = three_way_merge(Some(&b), &local, &remote);
        assert!(result.conflicts.is_empty(), "{:?}", result.conflicts);
        assert_eq!(result.auto_merged, 3);
        let merged = resolve(&result).unwrap();
        assert_eq!(merged.nodes[0].pos.x, 11.0);
        assert_eq!(merged.nodes[1].pos.x, 999.0);
        assert_eq!(merged.groups.len(), 2);
        assert_eq!(merged.virtual_relations.len(), 1);
        // Viewport tetap milik lokal.
        assert_eq!(merged.pan, vec2(5.0, 5.0));
    }

    #[test]
    fn deletions_on_one_side_are_applied() {
        let b = base();
        let mut local = b.clone();
        local.groups.clear();
        let remote = b.clone();
        let merged = resolve(&three_way_merge(Some(&b), &local, &remote)).unwrap();
        assert!(merged.groups.is_empty());

        let local = b.clone();
        let mut remote = b.clone();
        remote.nodes.retain(|n| n.id != "orders");
        let merged = resolve(&three_way_merge(Some(&b), &local, &remote)).unwrap();
        assert_eq!(merged.nodes.len(), 1);
    }

    #[test]
    fn same_item_changed_both_sides_is_a_conflict() {
        let b = base();
        let mut local = b.clone();
        local.nodes[0].pos.x = 10.0;
        local.groups[0].title = "Mine".into();
        let mut remote = b.clone();
        remote.nodes[0].pos.x = 20.0;
        remote.groups.clear(); // dihapus di database, diubah di lokal

        let mut result = three_way_merge(Some(&b), &local, &remote);
        assert_eq!(result.conflicts.len(), 2);
        let table = result.conflicts.iter().find(|c| c.kind == "Table").unwrap();
        assert_eq!(table.label, "Table 'users'");
        assert_eq!(table.describe(Side::Remote), "changed");
        let grp = result.conflicts.iter().find(|c| c.kind == "Group").unwrap();
        assert_eq!(grp.describe(Side::Remote), "deleted");

        // Default: versi lokal.
        let mine = resolve(&result).unwrap();
        assert_eq!(mine.nodes[0].pos.x, 10.0);
        assert_eq!(mine.groups[0].title, "Mine");

        for c in &mut result.conflicts {
            c.choice = Side::Remote;
        }
        let theirs = resolve(&result).unwrap();
        assert_eq!(theirs.nodes[0].pos.x, 20.0);
        assert!(theirs.groups.is_empty());
        assert_eq!(theirs.nodes.len(), 2);
    }

    #[test]
    fn scalar_fields_merge_three_way() {
        let b = DiagramState {
            diagram_title: Some("A".into()),
            ..Default::default()
        };
        let local = b.clone();
        let remote = DiagramState {
            diagram_title: Some("B".into()),
            ..Default::default()
        };
        let r = three_way_merge(Some(&b), &local, &remote);
        assert!(r.conflicts.is_empty());
        assert_eq!(resolve(&r).unwrap().diagram_title.as_deref(), Some("B"));

        let local = DiagramState {
            diagram_title: Some("C".into()),
            ..Default::default()
        };
        let r = three_way_merge(Some(&b), &local, &remote);
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].kind, "Setting");
    }

    #[test]
    fn without_base_items_from_both_sides_are_kept() {
        let mut local = DiagramState::default();
        local.groups.push(group("g1", "Auth"));
        local.nodes.push(node("users", 0.0));
        let mut remote = DiagramState::default();
        remote.groups.push(group("g2", "Billing"));
        remote.nodes.push(node("users", 40.0));

        let result = three_way_merge(None, &local, &remote);
        // Posisi `users` berbeda tanpa base: konflik; grup digabung.
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].describe(Side::Local), "added");
        let merged = resolve(&result).unwrap();
        assert_eq!(merged.groups.len(), 2);
    }

    #[test]
    fn adopted_state_keeps_live_columns_and_tables() {
        let mut old = base();
        old.nodes[0].columns = vec!["id".into(), "email".into()];
        old.nodes.push(node("payments", 400.0));
        let mut merged = base();
        merged.nodes[0].pos.x = 77.0;
        adopt_schema_from(&mut merged, &old);
        assert_eq!(merged.nodes[0].pos.x, 77.0);
        assert_eq!(merged.nodes[0].columns.len(), 2);
        assert!(merged.nodes.iter().any(|n| n.id == "payments"));
    }

    #[test]
    fn base_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("tabular_sync_base_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("conn_1_main.base.json");
        write_base(
            &path,
            &SyncBase {
                revision: 7,
                state: base(),
            },
        );
        let read = read_base(&path).unwrap();
        assert_eq!(read.revision, 7);
        assert!(same_content(&read.state, &base()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
