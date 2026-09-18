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
/// Menggabungkan pencarian berbasis FK pattern (seperti `customer_id` -> `customers.id`)
/// dan kemiripan nama kolom non-generik (seperti `imei`, `sku`, `uuid`) di seluruh diagram.
pub fn suggest_relations(state: &DiagramState) -> Vec<RelationSuggestion> {
    let tables: Vec<TableInfo> = state.nodes.iter().map(TableInfo::new).collect();
    let mut out: Vec<RelationSuggestion> = Vec::new();

    // 1. Relasi berbasis Foreign Key pattern (misal `customer_id` -> `customers.id`)
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

    // 2. Relasi berbasis kemiripan nama kolom non-generik antar pasangan tabel (misal `imei`, `sku`, dsb.)
    for (i, t1) in tables.iter().enumerate() {
        for t2 in &tables[(i + 1)..] {
            if t1.node.id == t2.node.id {
                continue;
            }
            for c1 in &t1.node.columns {
                if is_generic_column_name(c1) {
                    continue;
                }
                // Kolom foreign key berakhiran `_id` atau berawalan `id_` ditangani oleh Section 1
                if c1.ends_with("_id") || c1.starts_with("id_") {
                    continue;
                }
                for c2 in &t2.node.columns {
                    if is_generic_column_name(c2) {
                        continue;
                    }
                    if c2.ends_with("_id") || c2.starts_with("id_") {
                        continue;
                    }
                    let Some((sim_score, sim_reason)) = column_similarity(c1, c2) else {
                        continue;
                    };

                    // Di suggest_relations global, tipe data harus kompatibel
                    if !types_compatible(t1.type_of(c1), t2.type_of(c2)) {
                        continue;
                    }

                    let t1_is_pk = t1.pks.iter().any(|pk| pk.eq_ignore_ascii_case(c1));
                    let t2_is_pk = t2.pks.iter().any(|pk| pk.eq_ignore_ascii_case(c2));

                    // Jika keduanya adalah sole primary key, arahnya ambigu (1:1)
                    if t1.pks.len() == 1 && t2.pks.len() == 1 && t1_is_pk && t2_is_pk {
                        continue;
                    }

                    let (child, child_col, parent, parent_col, reason, final_score) =
                        if t2_is_pk && !t1_is_pk {
                            (
                                t1.node.id.clone(),
                                c1.clone(),
                                t2.node.id.clone(),
                                c2.clone(),
                                format!("`{c2}` is primary key of `{}`", t2.node.id),
                                (sim_score + 0.05).min(1.0),
                            )
                        } else if t1_is_pk && !t2_is_pk {
                            (
                                t2.node.id.clone(),
                                c2.clone(),
                                t1.node.id.clone(),
                                c1.clone(),
                                format!("`{c1}` is primary key of `{}`", t1.node.id),
                                (sim_score + 0.05).min(1.0),
                            )
                        } else {
                            (
                                t1.node.id.clone(),
                                c1.clone(),
                                t2.node.id.clone(),
                                c2.clone(),
                                format!("{sim_reason} in `{}`", t2.node.id),
                                sim_score,
                            )
                        };

                    if !is_already_related(
                        &state.nodes,
                        &state.virtual_relations,
                        &child,
                        &child_col,
                        &parent,
                        &parent_col,
                    ) {
                        out.push(RelationSuggestion {
                            relation: VirtualRelation {
                                child,
                                child_column: child_col,
                                parent,
                                parent_column: parent_col,
                                origin: RelationOrigin::Inferred,
                            },
                            score: final_score,
                            reason,
                        });
                    }
                }
            }
        }
    }

    // Deduplikasi di kedua arah relasi
    let mut seen = HashSet::new();
    out.retain(|s| {
        let key1 = (
            s.relation.child.clone(),
            s.relation.child_column.clone(),
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
        );
        let key2 = (
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
            s.relation.child.clone(),
            s.relation.child_column.clone(),
        );
        if seen.contains(&key1) || seen.contains(&key2) {
            false
        } else {
            seen.insert(key1);
            true
        }
    });

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

/// Kolom-kolom umum yang tidak boleh dihubungkan otomatis hanya karena namanya sama,
/// kecuali bila salah satunya memenuhi aturan foreign key / primary key.
pub fn is_generic_column_name(col: &str) -> bool {
    let lower = col.to_lowercase();
    let clean = strip_column_affixes(&lower);
    matches!(
        lower.as_str(),
        "id" | "name"
            | "nama"
            | "title"
            | "judul"
            | "type"
            | "tipe"
            | "jenis"
            | "status"
            | "state"
            | "description"
            | "deskripsi"
            | "keterangan"
            | "desc"
            | "notes"
            | "note"
            | "catatan"
            | "remark"
            | "remarks"
            | "comment"
            | "comments"
            | "created_at"
            | "updated_at"
            | "deleted_at"
            | "created_time"
            | "updated_time"
            | "deleted_time"
            | "create_time"
            | "update_time"
            | "delete_time"
            | "timestamp"
            | "is_active"
            | "active"
            | "enabled"
            | "is_deleted"
            | "created_by"
            | "updated_by"
            | "deleted_by"
            | "value"
            | "nilai"
            | "data"
            | "code"
            | "kode"
            | "no"
            | "nomor"
            | "num"
            | "number"
            | "date"
            | "tanggal"
            | "tgl"
            | "time"
            | "waktu"
            | "jam"
            | "flag"
            | "order"
            | "seq"
            | "sequence"
            | "sort"
            | "version"
            | "extra"
    ) || matches!(
        clean,
        "id" | "name"
            | "nama"
            | "title"
            | "type"
            | "status"
            | "state"
            | "desc"
            | "note"
            | "remark"
            | "date"
            | "time"
            | "code"
            | "val"
            | "flag"
    )
}

fn is_already_related(
    nodes: &[DiagramNode],
    virtual_relations: &[VirtualRelation],
    child_table: &str,
    child_col: &str,
    parent_table: &str,
    parent_col: &str,
) -> bool {
    if child_table == parent_table && child_col == parent_col {
        return true;
    }
    // Cek relasi virtual di kedua arah
    if virtual_relations.iter().any(|r| {
        (r.child == child_table
            && r.child_column == child_col
            && r.parent == parent_table
            && r.parent_column == parent_col)
            || (r.child == parent_table
                && r.child_column == parent_col
                && r.parent == child_table
                && r.parent_column == child_col)
    }) {
        return true;
    }
    // Cek relasi foreign key fisik dari database di kedua arah
    if let Some(child_node) = nodes.iter().find(|n| n.id == child_table) {
        if child_node.foreign_keys.iter().any(|fk| {
            fk.column_name.eq_ignore_ascii_case(child_col)
                && fk.referenced_table_name.eq_ignore_ascii_case(parent_table)
                && fk.referenced_column_name.eq_ignore_ascii_case(parent_col)
        }) {
            return true;
        }
    }
    if let Some(parent_node) = nodes.iter().find(|n| n.id == parent_table) {
        if parent_node.foreign_keys.iter().any(|fk| {
            fk.column_name.eq_ignore_ascii_case(parent_col)
                && fk.referenced_table_name.eq_ignore_ascii_case(child_table)
                && fk.referenced_column_name.eq_ignore_ascii_case(child_col)
        }) {
            return true;
        }
    }
    false
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();
    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row: Vec<usize> = vec![0; b_len + 1];

    for (i, ca) in a_chars.iter().enumerate() {
        curr_row[0] = i + 1;
        for (j, cb) in b_chars.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr_row[j + 1] = (curr_row[j] + 1)
                .min(prev_row[j + 1] + 1)
                .min(prev_row[j] + cost);
        }
        prev_row.copy_from_slice(&curr_row);
    }
    prev_row[b_len]
}

pub fn strip_column_affixes(s: &str) -> &str {
    let mut curr = s;
    for prefix in [
        "id_", "no_", "nomor_", "kd_", "kode_", "cd_", "num_", "txt_", "val_",
    ] {
        if let Some(rest) = curr.strip_prefix(prefix) {
            if !rest.is_empty() {
                curr = rest;
                break;
            }
        }
    }
    for suffix in [
        "_id", "_no", "_nomor", "_kd", "_kode", "_code", "_num", "_number", "_val",
    ] {
        if let Some(rest) = curr.strip_suffix(suffix) {
            if !rest.is_empty() {
                curr = rest;
                break;
            }
        }
    }
    curr
}

/// Pecah nama kolom menjadi token-token kata (snake_case, kebab-case, camelCase, dsb.)
pub fn column_tokens(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let lower = s.to_lowercase();
    for part in lower.split(['_', '-', '.', ' ']) {
        let clean = strip_column_affixes(part);
        if !clean.is_empty() {
            tokens.push(clean.to_string());
        }
        if clean != part && !part.is_empty() {
            tokens.push(part.to_string());
        }
    }
    // Pisahkan camelCase juga (e.g. deviceImei -> device, imei)
    let mut curr = String::new();
    for ch in s.chars() {
        if ch.is_uppercase() && !curr.is_empty() {
            let lower_curr = curr.to_lowercase();
            tokens.push(lower_curr);
            curr.clear();
        }
        if ch.is_alphanumeric() {
            curr.push(ch);
        } else if !curr.is_empty() {
            let lower_curr = curr.to_lowercase();
            tokens.push(lower_curr);
            curr.clear();
        }
    }
    if !curr.is_empty() {
        tokens.push(curr.to_lowercase());
    }
    tokens.retain(|t| !t.is_empty());
    tokens
}

/// Hitung tingkat kemiripan (similarity) antara dua nama kolom.
/// Mengembalikan `Some((score, reason))` bila ada kecocokan atau kemiripan yang cukup kuat.
pub fn column_similarity(a: &str, b: &str) -> Option<(f32, String)> {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();

    // 1. Nama sama persis (case-insensitive)
    if a_lower == b_lower {
        return Some((0.95, format!("Matching column `{b}`")));
    }

    let a_clean = strip_column_affixes(&a_lower);
    let b_clean = strip_column_affixes(&b_lower);

    // 2. Identik setelah pembersihan prefix/suffix (misal `no_imei` vs `imei`, `id_pelanggan` vs `pelanggan_id`)
    if a_clean == b_clean && !a_clean.is_empty() {
        return Some((0.90, format!("Matching column identifier `{b}`")));
    }

    // 3. Substring / contains (misal `device_imei` mengandung `imei`, atau `nomor_imei`)
    let a_stem = if a_clean.len() >= 3 {
        a_clean
    } else {
        &a_lower
    };
    let b_stem = if b_clean.len() >= 3 {
        b_clean
    } else {
        &b_lower
    };

    if a_stem.len() >= 3 && (b_lower.contains(a_stem) || b_clean.contains(a_stem)) {
        return Some((0.85, format!("Similar column `{b}`")));
    }
    if b_stem.len() >= 3 && (a_lower.contains(b_stem) || a_clean.contains(b_stem)) {
        return Some((0.85, format!("Similar column `{b}`")));
    }

    // 4. Token / keyword intersection (misal `device_imei` dan `tracker_imei` berbagi kata kunci `imei`)
    let a_tokens = column_tokens(a);
    let b_tokens = column_tokens(b);
    for tok_a in &a_tokens {
        if tok_a.len() >= 3 && !is_generic_column_name(tok_a) {
            if b_tokens.iter().any(|tok_b| tok_b == tok_a) {
                return Some((0.82, format!("Shared column keyword `{tok_a}` in `{b}`")));
            }
        }
    }

    // 5. Levenshtein edit distance (typo atau selisih 1 karakter bila panjang >= 4)
    if a_lower.len() >= 4 && b_lower.len() >= 4 {
        let dist = levenshtein(&a_lower, &b_lower);
        if dist == 1 {
            return Some((0.78, format!("Close spelling match `{b}`")));
        }
    }

    None
}

/// Saran relasi untuk satu kolom tertentu di tabel target berdasarkan slice nodes dan relasi virtual.
pub fn suggest_relations_for_column_data(
    nodes: &[DiagramNode],
    virtual_relations: &[VirtualRelation],
    target_table: &str,
    target_column: &str,
) -> Vec<RelationSuggestion> {
    let tables: Vec<TableInfo> = nodes.iter().map(TableInfo::new).collect();
    let Some(target_info) = tables.iter().find(|t| t.node.id == target_table) else {
        return Vec::new();
    };
    if !target_info
        .node
        .columns
        .iter()
        .any(|c| c.eq_ignore_ascii_case(target_column))
    {
        return Vec::new();
    }

    let mut out: Vec<RelationSuggestion> = Vec::new();
    let target_col_lower = target_column.to_lowercase();
    let is_generic = is_generic_column_name(target_column);

    // 1. Target table sebagai CHILD: target_column mengarah ke tabel PARENT lain.
    for parent in tables.iter().filter(|t| t.node.id != target_info.node.id) {
        if let Some((target_pk, score, reason)) =
            match_parent(&target_col_lower, target_column, target_info, parent)
        {
            if types_compatible(
                target_info.type_of(target_column),
                parent.type_of(&target_pk),
            ) && !is_already_related(
                nodes,
                virtual_relations,
                &target_info.node.id,
                target_column,
                &parent.node.id,
                &target_pk,
            ) {
                out.push(RelationSuggestion {
                    relation: VirtualRelation {
                        child: target_info.node.id.clone(),
                        child_column: target_column.to_string(),
                        parent: parent.node.id.clone(),
                        parent_column: target_pk,
                        origin: RelationOrigin::Inferred,
                    },
                    score,
                    reason,
                });
            }
        }
    }

    // 2. Target table sebagai PARENT: kolom di tabel CHILD lain mengarah ke target_column.
    for other in tables.iter().filter(|t| t.node.id != target_info.node.id) {
        for other_col in &other.node.columns {
            let other_lower = other_col.to_lowercase();
            if let Some((matched_target, score, reason)) =
                match_parent(&other_lower, other_col, other, target_info)
            {
                if matched_target.eq_ignore_ascii_case(target_column)
                    && types_compatible(
                        other.type_of(other_col),
                        target_info.type_of(target_column),
                    )
                    && !is_already_related(
                        nodes,
                        virtual_relations,
                        &other.node.id,
                        other_col,
                        &target_info.node.id,
                        target_column,
                    )
                {
                    out.push(RelationSuggestion {
                        relation: VirtualRelation {
                            child: other.node.id.clone(),
                            child_column: other_col.clone(),
                            parent: target_info.node.id.clone(),
                            parent_column: target_column.to_string(),
                            origin: RelationOrigin::Inferred,
                        },
                        score,
                        reason,
                    });
                }
            }
        }
    }

    // 3. Similarity Search & Shared Columns:
    //    Cari semua tabel lain yang memiliki kolom dengan nama yang sama atau mirip
    //    (misal `imei`, `no_imei`, `device_imei`, `tracker_imei`, dll).
    if !is_generic {
        for other in tables.iter().filter(|t| t.node.id != target_info.node.id) {
            for other_col in &other.node.columns {
                let Some((sim_score, sim_reason)) = column_similarity(target_column, other_col)
                else {
                    continue;
                };

                let compatible =
                    types_compatible(target_info.type_of(target_column), other.type_of(other_col));
                let (score, type_note) = if compatible {
                    (sim_score, String::new())
                } else {
                    let t1_t = target_info.type_of(target_column).unwrap_or("unknown");
                    let t2_t = other.type_of(other_col).unwrap_or("unknown");
                    ((sim_score - 0.08).max(0.60), format!(" ({t1_t} ~ {t2_t})"))
                };

                let target_is_pk = target_info
                    .pks
                    .iter()
                    .any(|pk| pk.eq_ignore_ascii_case(target_column));
                let other_is_pk = other
                    .pks
                    .iter()
                    .any(|pk| pk.eq_ignore_ascii_case(other_col));

                let (child, child_col, parent, parent_col, reason, final_score) =
                    if other_is_pk && !target_is_pk {
                        (
                            target_info.node.id.clone(),
                            target_column.to_string(),
                            other.node.id.clone(),
                            other_col.clone(),
                            format!(
                                "`{other_col}` is primary key of `{}`{type_note}",
                                other.node.id
                            ),
                            (score + 0.05).min(1.0),
                        )
                    } else if target_is_pk && !other_is_pk {
                        (
                            other.node.id.clone(),
                            other_col.clone(),
                            target_info.node.id.clone(),
                            target_column.to_string(),
                            format!(
                                "`{target_column}` is primary key of `{}`{type_note}",
                                target_info.node.id
                            ),
                            (score + 0.05).min(1.0),
                        )
                    } else {
                        // Keduanya bukan PK atau keduanya PK:
                        // Tetap tampilkan relasi antar tabel dengan kolom yang sama/mirip!
                        (
                            target_info.node.id.clone(),
                            target_column.to_string(),
                            other.node.id.clone(),
                            other_col.clone(),
                            format!("{sim_reason} in `{}`{type_note}", other.node.id),
                            score,
                        )
                    };

                if !is_already_related(
                    nodes,
                    virtual_relations,
                    &child,
                    &child_col,
                    &parent,
                    &parent_col,
                ) {
                    out.push(RelationSuggestion {
                        relation: VirtualRelation {
                            child,
                            child_column: child_col,
                            parent,
                            parent_column: parent_col,
                            origin: RelationOrigin::Inferred,
                        },
                        score: final_score,
                        reason,
                    });
                }
            }
        }
    }

    // Deduplikasi dua arah
    let mut seen = HashSet::new();
    out.retain(|s| {
        let key1 = (
            s.relation.child.clone(),
            s.relation.child_column.clone(),
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
        );
        let key2 = (
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
            s.relation.child.clone(),
            s.relation.child_column.clone(),
        );
        if seen.contains(&key1) || seen.contains(&key2) {
            false
        } else {
            seen.insert(key1);
            true
        }
    });

    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.relation.child.cmp(&b.relation.child))
            .then_with(|| a.relation.child_column.cmp(&b.relation.child_column))
    });

    out
}

/// Saran relasi untuk satu kolom tertentu di tabel target.
pub fn suggest_relations_for_column(
    state: &DiagramState,
    target_table: &str,
    target_column: &str,
) -> Vec<RelationSuggestion> {
    suggest_relations_for_column_data(
        &state.nodes,
        &state.virtual_relations,
        target_table,
        target_column,
    )
}

/// Saran relasi antar tabel berdasarkan nama kolom (atau similaritas kolom) di seluruh diagram.
pub fn suggest_relations_by_column_name(
    state: &DiagramState,
    column_name: &str,
) -> Vec<RelationSuggestion> {
    suggest_relations_by_column_name_data(&state.nodes, &state.virtual_relations, column_name)
}

/// Saran relasi antar tabel berdasarkan nama kolom di seluruh diagram menggunakan data murni.
pub fn suggest_relations_by_column_name_data(
    nodes: &[DiagramNode],
    virtual_relations: &[VirtualRelation],
    column_name: &str,
) -> Vec<RelationSuggestion> {
    let mut out = Vec::new();
    let tables: Vec<TableInfo> = nodes.iter().map(TableInfo::new).collect();
    let col_clean = column_name.trim();
    if col_clean.is_empty() {
        return out;
    }

    // Kumpulkan semua pasangan tabel yang memiliki kolom cocok atau mirip
    let mut col_matches: Vec<(&TableInfo, &str)> = Vec::new();
    for t in &tables {
        for col in &t.node.columns {
            if col.eq_ignore_ascii_case(col_clean) || column_similarity(col_clean, col).is_some() {
                col_matches.push((t, col));
            }
        }
    }

    for (i, (t1, c1)) in col_matches.iter().enumerate() {
        for (t2, c2) in &col_matches[(i + 1)..] {
            if t1.node.id == t2.node.id {
                continue;
            }

            let compatible = types_compatible(t1.type_of(c1), t2.type_of(c2));
            let (sim_score, sim_reason) =
                column_similarity(c1, c2).unwrap_or((0.80, format!("Matching `{c1}` / `{c2}`")));

            let (score, type_note) = if compatible {
                (sim_score, String::new())
            } else {
                let t1_t = t1.type_of(c1).unwrap_or("unknown");
                let t2_t = t2.type_of(c2).unwrap_or("unknown");
                ((sim_score - 0.08).max(0.60), format!(" ({t1_t} ~ {t2_t})"))
            };

            let t1_is_pk = t1.pks.iter().any(|pk| pk.eq_ignore_ascii_case(c1));
            let t2_is_pk = t2.pks.iter().any(|pk| pk.eq_ignore_ascii_case(c2));

            let (child, child_col, parent, parent_col, reason, final_score) =
                if t2_is_pk && !t1_is_pk {
                    (
                        t1.node.id.clone(),
                        c1.to_string(),
                        t2.node.id.clone(),
                        c2.to_string(),
                        format!("`{c2}` is primary key of `{}`{type_note}", t2.node.id),
                        (score + 0.05).min(1.0),
                    )
                } else if t1_is_pk && !t2_is_pk {
                    (
                        t2.node.id.clone(),
                        c2.to_string(),
                        t1.node.id.clone(),
                        c1.to_string(),
                        format!("`{c1}` is primary key of `{}`{type_note}", t1.node.id),
                        (score + 0.05).min(1.0),
                    )
                } else {
                    (
                        t1.node.id.clone(),
                        c1.to_string(),
                        t2.node.id.clone(),
                        c2.to_string(),
                        format!("{sim_reason} in `{}`{type_note}", t2.node.id),
                        score,
                    )
                };

            if !is_already_related(
                nodes,
                virtual_relations,
                &child,
                &child_col,
                &parent,
                &parent_col,
            ) {
                out.push(RelationSuggestion {
                    relation: VirtualRelation {
                        child,
                        child_column: child_col,
                        parent,
                        parent_column: parent_col,
                        origin: RelationOrigin::Inferred,
                    },
                    score: final_score,
                    reason,
                });
            }
        }
    }

    // Deduplikasi dua arah
    let mut seen = HashSet::new();
    out.retain(|s| {
        let key1 = (
            s.relation.child.clone(),
            s.relation.child_column.clone(),
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
        );
        let key2 = (
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
            s.relation.child.clone(),
            s.relation.child_column.clone(),
        );
        if seen.contains(&key1) || seen.contains(&key2) {
            false
        } else {
            seen.insert(key1);
            true
        }
    });

    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.relation.child.cmp(&b.relation.child))
            .then_with(|| a.relation.child_column.cmp(&b.relation.child_column))
    });

    out
}

/// Saran relasi berdasarkan pencarian nama kolom dinamis di seluruh diagram.
/// Mencari kolom yang sama, mirip, atau berbagi kata kunci dengan `query`,
/// serta menyarankan relasi ke/dari tabel lain (termasuk foreign key / primary key pattern).
pub fn suggest_relations_by_column_search(
    state: &DiagramState,
    query: &str,
) -> Vec<RelationSuggestion> {
    suggest_relations_by_column_search_data(&state.nodes, &state.virtual_relations, query)
}

/// Implementasi murni pencarian saran relasi berbasis nama kolom.
pub fn suggest_relations_by_column_search_data(
    nodes: &[DiagramNode],
    virtual_relations: &[VirtualRelation],
    query: &str,
) -> Vec<RelationSuggestion> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let q_lower = q.to_lowercase();
    let q_clean = strip_column_affixes(&q_lower);

    let tables: Vec<TableInfo> = nodes.iter().map(TableInfo::new).collect();
    let mut out: Vec<RelationSuggestion> = Vec::new();

    // 1. Kumpulkan semua tabel dan kolom di diagram yang relevan dengan query pencarian:
    let mut matched_cols: Vec<(&TableInfo, &str)> = Vec::new();
    for t in &tables {
        for col in &t.node.columns {
            let col_lower = col.to_lowercase();
            let col_clean = strip_column_affixes(&col_lower);
            let is_generic = is_generic_column_name(col);

            let is_match = col.eq_ignore_ascii_case(q)
                || column_similarity(q, col).is_some()
                || (q_clean.len() >= 2 && col_clean.eq_ignore_ascii_case(q_clean))
                || (!is_generic && q_lower.len() >= 2 && col_lower.contains(&q_lower))
                || (!is_generic && col_lower.len() >= 3 && q_lower.contains(&col_lower));

            if is_match {
                matched_cols.push((t, col));
            }
        }
    }

    // 2. Untuk setiap kolom yang cocok, gunakan `suggest_relations_for_column_data`
    //    untuk menemukan tabel tujuan (parent PK, child FK, atau shared column).
    for (t, col) in &matched_cols {
        let col_suggestions =
            suggest_relations_for_column_data(nodes, virtual_relations, &t.node.id, col);
        out.extend(col_suggestions);
    }

    // 3. Pasangkan langsung antar tabel yang sama-sama memiliki kolom yang cocok
    for (i, (t1, c1)) in matched_cols.iter().enumerate() {
        for (t2, c2) in &matched_cols[(i + 1)..] {
            if t1.node.id == t2.node.id {
                continue;
            }
            let compatible = types_compatible(t1.type_of(c1), t2.type_of(c2));
            let (sim_score, sim_reason) =
                column_similarity(c1, c2).unwrap_or((0.85, format!("Matching `{c1}` / `{c2}`")));

            let (score, type_note) = if compatible {
                (sim_score, String::new())
            } else {
                let t1_t = t1.type_of(c1).unwrap_or("unknown");
                let t2_t = t2.type_of(c2).unwrap_or("unknown");
                ((sim_score - 0.08).max(0.60), format!(" ({t1_t} ~ {t2_t})"))
            };

            let t1_is_pk = t1.pks.iter().any(|pk| pk.eq_ignore_ascii_case(c1));
            let t2_is_pk = t2.pks.iter().any(|pk| pk.eq_ignore_ascii_case(c2));

            let (child, child_col, parent, parent_col, reason, final_score) =
                if t2_is_pk && !t1_is_pk {
                    (
                        t1.node.id.clone(),
                        c1.to_string(),
                        t2.node.id.clone(),
                        c2.to_string(),
                        format!("`{c2}` is primary key of `{}`{type_note}", t2.node.id),
                        (score + 0.05).min(1.0),
                    )
                } else if t1_is_pk && !t2_is_pk {
                    (
                        t2.node.id.clone(),
                        c2.to_string(),
                        t1.node.id.clone(),
                        c1.to_string(),
                        format!("`{c1}` is primary key of `{}`{type_note}", t1.node.id),
                        (score + 0.05).min(1.0),
                    )
                } else {
                    (
                        t1.node.id.clone(),
                        c1.to_string(),
                        t2.node.id.clone(),
                        c2.to_string(),
                        format!("{sim_reason} in `{}`{type_note}", t2.node.id),
                        score,
                    )
                };

            if !is_already_related(
                nodes,
                virtual_relations,
                &child,
                &child_col,
                &parent,
                &parent_col,
            ) {
                out.push(RelationSuggestion {
                    relation: VirtualRelation {
                        child,
                        child_column: child_col,
                        parent,
                        parent_column: parent_col,
                        origin: RelationOrigin::Inferred,
                    },
                    score: final_score,
                    reason,
                });
            }
        }
    }

    // 4. Deduplikasi dua arah
    let mut seen = HashSet::new();
    out.retain(|s| {
        let key1 = (
            s.relation.child.clone(),
            s.relation.child_column.clone(),
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
        );
        let key2 = (
            s.relation.parent.clone(),
            s.relation.parent_column.clone(),
            s.relation.child.clone(),
            s.relation.child_column.clone(),
        );
        if seen.contains(&key1) || seen.contains(&key2) {
            false
        } else {
            seen.insert(key1);
            true
        }
    });

    out.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.relation.child.cmp(&b.relation.child))
            .then_with(|| a.relation.child_column.cmp(&b.relation.child_column))
    });

    out
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
            database_name: None,
            connection_id: None,
            connection_name: None,
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

    #[test]
    fn suggests_relations_for_column_both_directions() {
        let st = state(vec![
            node(
                "customers",
                &[("id", "int", true), ("name", "varchar(50)", false)],
            ),
            node(
                "orders",
                &[("id", "int", true), ("customer_id", "int", false)],
            ),
            node(
                "invoices",
                &[("id", "int", true), ("customer_id", "int", false)],
            ),
        ]);

        // 1. Kolom customer_id di tabel orders menemukan customers.id dan invoices.customer_id
        let child_suggs = pairs(&suggest_relations_for_column(&st, "orders", "customer_id"));
        assert!(child_suggs.contains(&"orders.customer_id->customers.id".to_string()));
        assert!(child_suggs.contains(&"orders.customer_id->invoices.customer_id".to_string()));

        // 2. Kolom id di tabel customers (sebagai parent) menemukan orders.customer_id dan invoices.customer_id
        let parent_suggs = pairs(&suggest_relations_for_column(&st, "customers", "id"));
        assert!(parent_suggs.contains(&"orders.customer_id->customers.id".to_string()));
        assert!(parent_suggs.contains(&"invoices.customer_id->customers.id".to_string()));
    }

    #[test]
    fn suggests_relations_for_shared_non_generic_column() {
        let st = state(vec![
            node(
                "products",
                &[("sku", "varchar(20)", true), ("name", "varchar(50)", false)],
            ),
            node(
                "stock",
                &[
                    ("id", "int", true),
                    ("sku", "varchar(20)", false),
                    ("qty", "int", false),
                ],
            ),
        ]);

        let suggs = pairs(&suggest_relations_for_column(&st, "stock", "sku"));
        assert!(suggs.contains(&"stock.sku->products.sku".to_string()));

        let parent_suggs = pairs(&suggest_relations_for_column(&st, "products", "sku"));
        assert!(parent_suggs.contains(&"stock.sku->products.sku".to_string()));
    }

    #[test]
    fn test_similarity_search_finds_all_imei_tables() {
        let st = state(vec![
            node(
                "devices",
                &[("id", "int", true), ("imei", "varchar(20)", false)],
            ),
            node(
                "trackers",
                &[("id", "int", true), ("imei", "varchar(20)", false)],
            ),
            node(
                "gps_logs",
                &[
                    ("id", "bigint", true),
                    ("device_imei", "varchar(20)", false),
                ],
            ),
            node(
                "sim_cards",
                &[("id", "int", true), ("no_imei", "varchar(20)", false)],
            ),
            node(
                "telemetry",
                &[("id", "bigint", true), ("imei", "bigint", false)],
            ),
            node(
                "vehicle_units",
                &[("id", "int", true), ("vehicle_imei", "varchar(20)", false)],
            ),
            node(
                "users",
                &[("id", "int", true), ("name", "varchar(50)", false)],
            ),
        ]);

        let suggs = suggest_relations_for_column(&st, "devices", "imei");
        let sugg_pairs = pairs(&suggs);

        // Harus menemukan trackers (imei exact), gps_logs (device_imei), sim_cards (no_imei),
        // telemetry (imei bigint), dan vehicle_units (vehicle_imei keyword)
        assert!(
            sugg_pairs.iter().any(|p| p.contains("trackers")),
            "Must match trackers. Pairs: {sugg_pairs:?}"
        );
        assert!(
            sugg_pairs.iter().any(|p| p.contains("gps_logs")),
            "Must match gps_logs (device_imei). Pairs: {sugg_pairs:?}"
        );
        assert!(
            sugg_pairs.iter().any(|p| p.contains("sim_cards")),
            "Must match sim_cards (no_imei). Pairs: {sugg_pairs:?}"
        );
        assert!(
            sugg_pairs.iter().any(|p| p.contains("telemetry")),
            "Must match telemetry (bigint type tolerance). Pairs: {sugg_pairs:?}"
        );
        assert!(
            sugg_pairs.iter().any(|p| p.contains("vehicle_units")),
            "Must match vehicle_units (vehicle_imei token). Pairs: {sugg_pairs:?}"
        );
        // Tidak boleh cocok dengan users (yang tidak punya imei)
        assert!(
            !sugg_pairs.iter().any(|p| p.contains("users")),
            "Must not match users"
        );

        // Uji juga pencarian eksplisit berdasarkan nama kolom `imei` di seluruh diagram
        let col_suggs = suggest_relations_by_column_name(&st, "imei");
        let col_pairs = pairs(&col_suggs);
        assert!(
            !col_pairs.is_empty(),
            "Must find relations by column name 'imei'"
        );
        assert!(
            col_pairs
                .iter()
                .any(|p| p.contains("devices") && p.contains("trackers"))
        );
        assert!(
            col_pairs
                .iter()
                .any(|p| p.contains("devices") && p.contains("telemetry"))
        );
    }

    #[test]
    fn test_suggest_relations_by_column_search_user_id() {
        let st = state(vec![
            node(
                "temp_report_td",
                &[
                    ("id", "bigint", true),
                    ("temp_report_id", "bigint", true),
                    ("user_id", "bigint", true),
                ],
            ),
            node(
                "users",
                &[("id", "bigint", true), ("name", "varchar(50)", false)],
            ),
            node(
                "devices",
                &[
                    ("id", "int", true),
                    ("user_id", "bigint", false),
                    ("imei", "varchar(20)", false),
                ],
            ),
            node(
                "orders",
                &[("id", "int", true), ("id_user", "bigint", false)],
            ),
        ]);

        // Saat mencari "user_id":
        let results = suggest_relations_by_column_search(&st, "user_id");
        let p = pairs(&results);

        // 1. temp_report_td.user_id -> users.id (FK pattern ke parent users)
        assert!(
            p.iter()
                .any(|s| s.contains("temp_report_td") && s.contains("users")),
            "Must link temp_report_td to users. Found: {p:?}"
        );
        // 2. devices.user_id -> users.id (FK pattern ke parent users)
        assert!(
            p.iter()
                .any(|s| s.contains("devices") && s.contains("users")),
            "Must link devices to users. Found: {p:?}"
        );
        // 3. orders.id_user -> users.id (similar stem match ke parent users)
        assert!(
            p.iter()
                .any(|s| s.contains("orders") && s.contains("users")),
            "Must link orders to users. Found: {p:?}"
        );
        // 4. temp_report_td.user_id <-> devices.user_id (shared column)
        assert!(
            p.iter()
                .any(|s| s.contains("temp_report_td") && s.contains("devices")),
            "Must link temp_report_td and devices. Found: {p:?}"
        );
    }
}
