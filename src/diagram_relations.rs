//! Relasi "virtual" antar tabel yang tidak punya foreign key di database.
//!
//! Banyak database (terutama MySQL lama / MyISAM) tidak mendeklarasikan FK,
//! padahal relasinya jelas dari nama kolom: `orders.customer_id` ->
//! `customers.id`, `kandang.id_user` -> `user.id_user`. Modul ini menyarankan
//! relasi tersebut dari kemiripan nama (plus kecocokan tipe bila diketahui).
//! Relasi yang diterima disimpan di `DiagramState::virtual_relations`, terpisah
//! dari FK database, sehingga tidak tertimpa saat skema di-refresh.

use std::collections::HashSet;

use crate::models::structs::{DiagramNode, DiagramState, RelationOrigin, VirtualRelation};

#[derive(Clone, Debug, PartialEq)]
pub struct RelationSuggestion {
    pub relation: VirtualRelation,
    /// 0.0..=1.0; makin tinggi makin yakin.
    pub score: f32,
    /// Alasan singkat untuk ditampilkan ke user.
    pub reason: String,
}

/// Saran relasi untuk kolom yang belum punya FK maupun relasi virtual.
/// Tiap kolom hanya mendapat satu saran (skor tertinggi); hasil diurutkan
/// dari skor tertinggi.
pub fn suggest_relations(state: &DiagramState) -> Vec<RelationSuggestion> {
    let tables: Vec<TableInfo> = state.nodes.iter().map(TableInfo::new).collect();
    let mut out: Vec<RelationSuggestion> = Vec::new();

    for child in &tables {
        for column in &child.node.columns {
            if child.node.is_fk_column(column)
                || state
                    .virtual_relations
                    .iter()
                    .any(|r| r.child == child.node.id && r.child_column == *column)
            {
                continue;
            }
            let lower = column.to_lowercase();
            let mut best: Option<RelationSuggestion> = None;
            for parent in tables.iter().filter(|t| t.node.id != child.node.id) {
                let Some((target, score, reason)) = match_parent(&lower, column, child, parent)
                else {
                    continue;
                };
                if !types_compatible(child.type_of(column), parent.type_of(&target)) {
                    continue;
                }
                if best.as_ref().is_none_or(|b| score > b.score) {
                    best = Some(RelationSuggestion {
                        relation: VirtualRelation {
                            child: child.node.id.clone(),
                            child_column: column.clone(),
                            parent: parent.node.id.clone(),
                            parent_column: target,
                            origin: RelationOrigin::Inferred,
                        },
                        score,
                        reason,
                    });
                }
            }
            out.extend(best);
        }
    }

    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.relation.child.cmp(&b.relation.child))
            .then_with(|| a.relation.child_column.cmp(&b.relation.child_column))
    });
    out
}

/// Tambahkan relasi virtual bila belum ada (child + kolom + parent sama).
/// Mengembalikan `false` bila duplikat atau relasi ke dirinya sendiri.
pub fn add_virtual_relation(state: &mut DiagramState, relation: VirtualRelation) -> bool {
    if relation.child == relation.parent && relation.child_column == relation.parent_column {
        return false;
    }
    let exists = state.virtual_relations.iter().any(|r| {
        r.child == relation.child
            && r.child_column == relation.child_column
            && r.parent == relation.parent
            && r.parent_column == relation.parent_column
    });
    if exists {
        return false;
    }
    state.virtual_relations.push(relation);
    true
}

struct TableInfo<'a> {
    node: &'a DiagramNode,
    /// Nama tabel (lowercase) beserta bentuk tunggal / tanpa prefix.
    names: HashSet<String>,
    /// Kolom primary key; `id` bila metadata tidak tersedia tapi kolomnya ada.
    pks: Vec<String>,
}

impl<'a> TableInfo<'a> {
    fn new(node: &'a DiagramNode) -> Self {
        let lower = node.id.to_lowercase();
        let base = strip_table_prefix(&lower).to_string();
        let names = [singular(&lower), singular(&base), lower, base]
            .into_iter()
            .filter(|n| !n.is_empty())
            .collect();
        let mut pks: Vec<String> = node
            .column_meta
            .iter()
            .filter(|c| c.is_pk)
            .map(|c| c.name.clone())
            .collect();
        if pks.is_empty()
            && let Some(id) = node.columns.iter().find(|c| c.eq_ignore_ascii_case("id"))
        {
            pks.push(id.clone());
        }
        Self { node, names, pks }
    }

    fn type_of(&self, column: &str) -> Option<&str> {
        self.node
            .column_info(column)
            .map(|c| c.type_name.as_str())
            .filter(|t| !t.is_empty())
    }

    fn has_column(&self, column: &str) -> Option<String> {
        self.node
            .columns
            .iter()
            .find(|c| c.eq_ignore_ascii_case(column))
            .cloned()
    }

    fn matches_name(&self, stem: &str) -> bool {
        !stem.is_empty() && (self.names.contains(stem) || self.names.contains(&singular(stem)))
    }
}

/// Kolom `column` di `child` mengarah ke `parent`? Kembalikan kolom target,
/// skor, dan alasan.
fn match_parent(
    lower: &str,
    column: &str,
    child: &TableInfo,
    parent: &TableInfo,
) -> Option<(String, f32, String)> {
    // 1. Nama kolom = nama tabel + id: `customer_id`, `customerid`, `id_customer`.
    let stem = lower
        .strip_suffix("_id")
        .or_else(|| lower.strip_prefix("id_"))
        .or_else(|| lower.strip_suffix("id").filter(|s| s.len() >= 2));
    if let Some(stem) = stem
        && parent.matches_name(stem)
    {
        let target = if parent.pks.len() == 1 {
            Some(parent.pks[0].clone())
        } else {
            parent
                .has_column(column)
                .or_else(|| parent.has_column("id"))
        };
        if let Some(target) = target {
            return Some((
                target,
                0.95,
                format!("`{column}` matches table name `{}`", parent.node.id),
            ));
        }
    }

    // 2. Nama kolom sama dengan primary key tunggal tabel lain (`id_user`,
    //    `imei`). `id` polos terlalu umum dan dilewati. Bila kolom itu juga PK
    //    tunggal di child, arahnya ambigu (1:1) sehingga tidak disarankan.
    if parent.pks.len() == 1 {
        let pk = &parent.pks[0];
        let child_sole_pk = child.pks.len() == 1 && child.pks[0].eq_ignore_ascii_case(column);
        if !pk.eq_ignore_ascii_case("id") && pk.eq_ignore_ascii_case(column) && !child_sole_pk {
            return Some((
                pk.clone(),
                0.85,
                format!("`{column}` is the primary key of `{}`", parent.node.id),
            ));
        }
    }
    None
}

/// Tipe dianggap cocok bila salah satunya tidak diketahui atau keluarganya sama.
fn types_compatible(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => type_family(a) == type_family(b),
        _ => true,
    }
}

fn type_family(raw: &str) -> String {
    let t = raw.to_lowercase();
    let base = t
        .split(|c: char| c == '(' || c.is_whitespace())
        .next()
        .unwrap_or("")
        .to_string();
    const INTS: &[&str] = &[
        "int",
        "integer",
        "bigint",
        "smallint",
        "tinyint",
        "mediumint",
        "serial",
        "bigserial",
        "smallserial",
        "int2",
        "int4",
        "int8",
        "number",
        "numeric",
        "decimal",
    ];
    const TEXTS: &[&str] = &[
        "char",
        "varchar",
        "character",
        "text",
        "string",
        "nvarchar",
        "nchar",
        "bpchar",
        "tinytext",
        "mediumtext",
        "longtext",
        "citext",
    ];
    if INTS.contains(&base.as_str()) {
        "int".to_string()
    } else if TEXTS.contains(&base.as_str()) {
        "text".to_string()
    } else {
        base
    }
}

/// Buang prefix nama tabel yang umum: `tbl_user` -> `user`.
fn strip_table_prefix(name: &str) -> &str {
    for prefix in ["tbl_", "tb_", "mst_", "ms_", "m_", "t_"] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return rest;
        }
    }
    name
}

/// Bentuk tunggal sederhana (Inggris): `categories` -> `category`,
/// `boxes` -> `box`, `users` -> `user`.
fn singular(word: &str) -> String {
    if let Some(stem) = word.strip_suffix("ies")
        && !stem.is_empty()
    {
        return format!("{stem}y");
    }
    for suffix in ["sses", "xes", "ches", "shes"] {
        if word.ends_with(suffix) {
            return word[..word.len() - 2].to_string();
        }
    }
    if word.len() > 1 && word.ends_with('s') && !word.ends_with("ss") {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::DiagramColumn;

    fn node(id: &str, cols: &[(&str, &str, bool)]) -> DiagramNode {
        DiagramNode {
            id: id.into(),
            title: id.into(),
            pos: eframe::egui::Pos2::ZERO,
            size: eframe::egui::Vec2::ZERO,
            columns: cols.iter().map(|c| c.0.to_string()).collect(),
            foreign_keys: Vec::new(),
            group_ids: Vec::new(),
            group_id: None,
            column_meta: cols
                .iter()
                .map(|(n, t, pk)| DiagramColumn {
                    name: n.to_string(),
                    type_name: t.to_string(),
                    is_pk: *pk,
                    nullable: true,
                })
                .collect(),
            detached: false,
        }
    }

    fn state(nodes: Vec<DiagramNode>) -> DiagramState {
        DiagramState {
            nodes,
            ..Default::default()
        }
    }

    fn pairs(s: &[RelationSuggestion]) -> Vec<String> {
        s.iter()
            .map(|s| {
                let r = &s.relation;
                format!(
                    "{}.{}->{}.{}",
                    r.child, r.child_column, r.parent, r.parent_column
                )
            })
            .collect()
    }

    #[test]
    fn suggests_from_table_name_plus_id() {
        let st = state(vec![
            node(
                "customers",
                &[("id", "int", true), ("name", "varchar(50)", false)],
            ),
            node(
                "orders",
                &[("id", "int", true), ("customer_id", "int", false)],
            ),
            node("categories", &[("id", "int", true)]),
            node(
                "products",
                &[("id", "int", true), ("categoryId", "int", false)],
            ),
        ]);
        let got = pairs(&suggest_relations(&st));
        assert!(got.contains(&"orders.customer_id->customers.id".to_string()));
        assert!(got.contains(&"products.categoryId->categories.id".to_string()));
        // `id` polos tidak pernah disarankan.
        assert!(!got.iter().any(|p| p.contains(".id->")));
    }

    #[test]
    fn suggests_indonesian_style_and_shared_primary_key() {
        let st = state(vec![
            node(
                "tbl_user",
                &[("id_user", "bigint", true), ("nama", "varchar(50)", false)],
            ),
            node(
                "kandang",
                &[("id_kandang", "int", true), ("id_user", "bigint", false)],
            ),
            node("devices", &[("imei", "char(30)", true)]),
            node(
                "user_data",
                &[("imei", "char(30)", true), ("user_id", "bigint", true)],
            ),
        ]);
        let got = pairs(&suggest_relations(&st));
        assert!(
            got.contains(&"kandang.id_user->tbl_user.id_user".to_string()),
            "{got:?}"
        );
        assert!(
            got.contains(&"user_data.imei->devices.imei".to_string()),
            "{got:?}"
        );
        assert!(
            got.contains(&"user_data.user_id->tbl_user.id_user".to_string()),
            "{got:?}"
        );
    }

    #[test]
    fn rejects_incompatible_types_and_existing_relations() {
        let mut st = state(vec![
            node("users", &[("id", "int", true)]),
            node("logs", &[("user_id", "varchar(20)", false)]),
            node("posts", &[("user_id", "int", false)]),
        ]);
        st.virtual_relations.push(VirtualRelation {
            child: "posts".into(),
            child_column: "user_id".into(),
            parent: "users".into(),
            parent_column: "id".into(),
            origin: RelationOrigin::Manual,
        });
        assert!(suggest_relations(&st).is_empty());
    }

    #[test]
    fn skips_columns_that_already_have_a_database_fk() {
        let mut orders = node("orders", &[("customer_id", "int", false)]);
        orders
            .foreign_keys
            .push(crate::models::structs::ForeignKey {
                constraint_name: "fk".into(),
                table_name: "orders".into(),
                column_name: "customer_id".into(),
                referenced_table_name: "customers".into(),
                referenced_column_name: "id".into(),
            });
        let st = state(vec![node("customers", &[("id", "int", true)]), orders]);
        assert!(suggest_relations(&st).is_empty());
    }

    #[test]
    fn sole_primary_key_shared_by_two_tables_is_ambiguous() {
        let st = state(vec![
            node("device", &[("imei", "char(30)", true)]),
            node("tracker", &[("imei", "char(30)", true)]),
        ]);
        assert!(suggest_relations(&st).is_empty());
    }

    #[test]
    fn add_virtual_relation_dedupes() {
        let mut st = DiagramState::default();
        let rel = VirtualRelation {
            child: "a".into(),
            child_column: "b_id".into(),
            parent: "b".into(),
            parent_column: "id".into(),
            origin: RelationOrigin::Manual,
        };
        assert!(add_virtual_relation(&mut st, rel.clone()));
        assert!(!add_virtual_relation(&mut st, rel));
        assert_eq!(st.virtual_relations.len(), 1);
    }

    #[test]
    fn singular_handles_common_plurals() {
        assert_eq!(singular("categories"), "category");
        assert_eq!(singular("boxes"), "box");
        assert_eq!(singular("users"), "user");
        assert_eq!(singular("address"), "address");
        assert_eq!(singular("addresses"), "address");
    }
}
