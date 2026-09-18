//! Konversi skema diagram ke/dari Mermaid `erDiagram`.
//!
//! Canvas egui ([`crate::diagram_view`]) tetap renderer utama dan layout
//! (posisi, group, warna) tetap disimpan sebagai JSON. Mermaid dipakai untuk
//! semantik skema yang ringkas: diekspor ke file / clipboard, disimpan sebagai
//! catatan memory di vault Obsidian (yang me-render blok ```` ```mermaid ````
//! secara native), dan dikembalikan ke agent AI karena jauh lebih hemat token
//! dibanding JSON `DiagramState`.
//!
//! Parser hanya mendukung subset `erDiagram` yang dihasilkan modul ini plus
//! sintaks umum (blok atribut, relasi `||--o{`, alias `a["Label"]`); baris
//! lain dilewati dan dilaporkan sebagai warning, bukan error.

use std::collections::{HashMap, HashSet};

use crate::models::structs::{
    DiagramColumn, DiagramGroup, DiagramNode, DiagramState, RelationOrigin, VirtualRelation,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErColumn {
    pub name: String,
    pub type_name: String,
    pub is_pk: bool,
    pub is_fk: bool,
    /// `None` bila engine / cache tidak memberi informasi nullable.
    pub nullable: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErEntity {
    pub name: String,
    pub columns: Vec<ErColumn>,
    pub groups: Vec<String>,
    pub group: Option<String>,
}

/// Foreign key `child.child_column -> parent.parent_column`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErRelation {
    pub child: String,
    pub child_column: String,
    pub parent: String,
    pub parent_column: String,
    /// Relasi hasil tebakan / buatan user (bukan FK di database); ditulis
    /// sebagai garis putus-putus `..` di Mermaid.
    pub inferred: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErModel {
    pub entities: Vec<ErEntity>,
    pub relations: Vec<ErRelation>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MermaidOptions {
    /// Batas kolom per tabel; kolom PK/FK selalu didahulukan.
    pub max_columns: Option<usize>,
    /// Hanya entity dan relasi, tanpa blok atribut (paling hemat token).
    pub relations_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedEr {
    pub model: ErModel,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MergeStats {
    pub added_tables: usize,
    pub updated_tables: usize,
    pub added_relations: usize,
}

impl ErModel {
    /// Bangun model dari state diagram; entity diurutkan per nama supaya
    /// output deterministik (diff catatan di vault tetap kecil).
    pub fn from_diagram(state: &DiagramState) -> Self {
        let group_titles: HashMap<&str, &str> = state
            .groups
            .iter()
            .map(|g| (g.id.as_str(), g.title.as_str()))
            .collect();

        let mut nodes: Vec<&DiagramNode> = state.nodes.iter().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        let mut entities = Vec::with_capacity(nodes.len());
        let mut relations: Vec<ErRelation> = Vec::new();
        for node in nodes {
            let columns = node
                .columns
                .iter()
                .map(|name| {
                    let meta = node.column_info(name);
                    ErColumn {
                        name: name.clone(),
                        type_name: meta.map(|m| m.type_name.clone()).unwrap_or_default(),
                        is_pk: meta.is_some_and(|m| m.is_pk),
                        is_fk: node.is_fk_column(name)
                            || state
                                .virtual_relations
                                .iter()
                                .any(|v| v.child == node.id && v.child_column == *name),
                        nullable: meta.map(|m| m.nullable),
                    }
                })
                .collect();
            let groups: Vec<String> = node
                .group_ids
                .iter()
                .filter_map(|gid| group_titles.get(gid.as_str()).map(|t| t.to_string()))
                .collect();
            let legacy_group = groups.first().cloned().or_else(|| {
                node.group_id
                    .as_deref()
                    .and_then(|gid| group_titles.get(gid))
                    .map(|t| t.to_string())
            });
            let all_groups = if groups.is_empty() && legacy_group.is_some() {
                legacy_group.clone().into_iter().collect()
            } else {
                groups
            };
            entities.push(ErEntity {
                name: node.id.clone(),
                columns,
                group: legacy_group,
                groups: all_groups,
            });
            for fk in node
                .foreign_keys
                .iter()
                .filter(|fk| fk.table_name == node.id)
            {
                let rel = ErRelation {
                    child: fk.table_name.clone(),
                    child_column: fk.column_name.clone(),
                    parent: fk.referenced_table_name.clone(),
                    parent_column: fk.referenced_column_name.clone(),
                    inferred: false,
                };
                if !relations.contains(&rel) {
                    relations.push(rel);
                }
            }
        }
        for v in &state.virtual_relations {
            let duplicate = relations.iter().any(|r| {
                r.child == v.child
                    && r.child_column == v.child_column
                    && r.parent == v.parent
                    && r.parent_column == v.parent_column
            });
            if !duplicate {
                relations.push(ErRelation {
                    child: v.child.clone(),
                    child_column: v.child_column.clone(),
                    parent: v.parent.clone(),
                    parent_column: v.parent_column.clone(),
                    inferred: v.origin != RelationOrigin::Imported,
                });
            }
        }
        Self {
            entities,
            relations,
        }
    }

    fn entity(&self, name: &str) -> Option<&ErEntity> {
        self.entities.iter().find(|e| e.name == name)
    }

    /// Render sebagai teks `erDiagram` (tanpa fence Markdown).
    pub fn to_mermaid(&self, opts: MermaidOptions) -> String {
        let ids = EntityIds::new(self);
        let mut out = String::from("erDiagram\n");

        for entity in &self.entities {
            let id = ids.get(&entity.name);
            if id != entity.name {
                out.push_str(&format!("    %% table {id} = {}\n", entity.name));
            }
        }

        // Group ditulis sebagai komentar supaya bisa dipulihkan saat impor.
        let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
        for entity in &self.entities {
            let entity_groups: Vec<&str> = if !entity.groups.is_empty() {
                entity.groups.iter().map(|s| s.as_str()).collect()
            } else if let Some(g) = entity.group.as_deref() {
                vec![g]
            } else {
                Vec::new()
            };
            for g in entity_groups {
                let id = ids.get(&entity.name);
                match groups.iter_mut().find(|(t, _)| *t == g) {
                    Some((_, members)) => {
                        if !members.contains(&id) {
                            members.push(id);
                        }
                    }
                    None => groups.push((g, vec![id])),
                }
            }
        }
        for (title, members) in &groups {
            out.push_str(&format!(
                "    %% group {}: {}\n",
                title.replace(':', " "),
                members.join(", ")
            ));
        }

        let related: HashSet<&str> = self
            .relations
            .iter()
            .flat_map(|r| [r.child.as_str(), r.parent.as_str()])
            .collect();

        for entity in &self.entities {
            let id = ids.get(&entity.name);
            if opts.relations_only || entity.columns.is_empty() {
                // Entity terisolasi tetap ditulis supaya tidak hilang dari diagram.
                if !related.contains(entity.name.as_str()) {
                    out.push_str(&format!("    {id}\n"));
                }
                continue;
            }
            let columns = pick_columns(&entity.columns, opts.max_columns);
            if columns.len() < entity.columns.len() {
                out.push_str(&format!(
                    "    %% {id}: showing {} of {} columns\n",
                    columns.len(),
                    entity.columns.len()
                ));
            }
            out.push_str(&format!("    {id} {{\n"));
            for col in columns {
                out.push_str(&format!("        {}\n", attribute_line(col)));
            }
            out.push_str("    }\n");
        }

        for rel in &self.relations {
            let optional = self
                .entity(&rel.child)
                .and_then(|e| e.columns.iter().find(|c| c.name == rel.child_column))
                .and_then(|c| c.nullable)
                .unwrap_or(false);
            let line = if rel.inferred { ".." } else { "--" };
            let cardinality = if optional {
                format!("|o{line}o{{")
            } else {
                format!("||{line}o{{")
            };
            out.push_str(&format!(
                "    {} {cardinality} {} : \"{} -> {}\"\n",
                ids.get(&rel.parent),
                ids.get(&rel.child),
                quote_safe(&rel.child_column),
                quote_safe(&rel.parent_column),
            ));
        }
        out
    }
}

/// Isi catatan Markdown untuk vault: blok mermaid plus daftar tabel sebagai
/// `[[wikilink]]` (supaya user bisa membuat catatan per tabel). Frontmatter
/// ditambahkan oleh [`crate::obsidian::save_schema_note`].
pub fn schema_note_markdown(title: &str, model: &ErModel) -> String {
    let mut out = format!(
        "# {title}\n\n> Generated by Tabular from the live schema and overwritten on the next \
         \"Save to Vault\". Keep your own notes in separate notes and link them here.\n\n\
         ```mermaid\n{}```\n\n## Tables\n\n",
        model.to_mermaid(MermaidOptions::default())
    );
    for entity in &model.entities {
        let pks: Vec<&str> = entity
            .columns
            .iter()
            .filter(|c| c.is_pk)
            .map(|c| c.name.as_str())
            .collect();
        let mut line = format!("- [[{}]] — {} columns", entity.name, entity.columns.len());
        if !pks.is_empty() {
            line.push_str(&format!(", PK {}", pks.join(", ")));
        }
        let fks: Vec<String> = model
            .relations
            .iter()
            .filter(|r| r.child == entity.name)
            .map(|r| {
                format!(
                    "{} → {}.{}{}",
                    r.child_column,
                    r.parent,
                    r.parent_column,
                    if r.inferred { " (inferred)" } else { "" }
                )
            })
            .collect();
        if !fks.is_empty() {
            line.push_str(&format!(", FK {}", fks.join("; ")));
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Kolom PK/FK dulu (urutan asli dipertahankan), lalu sisanya sampai batas.
fn pick_columns(columns: &[ErColumn], max: Option<usize>) -> Vec<&ErColumn> {
    let Some(max) = max.filter(|m| *m < columns.len()) else {
        return columns.iter().collect();
    };
    let mut keep: HashSet<usize> = columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_pk || c.is_fk)
        .map(|(i, _)| i)
        .take(max)
        .collect();
    for i in 0..columns.len() {
        if keep.len() >= max {
            break;
        }
        keep.insert(i);
    }
    columns
        .iter()
        .enumerate()
        .filter(|(i, _)| keep.contains(i))
        .map(|(_, c)| c)
        .collect()
}

fn attribute_line(col: &ErColumn) -> String {
    let ty = sanitize_word(&col.type_name, "unknown");
    let name = sanitize_word(&col.name, "column");
    let mut line = format!("{ty} {name}");
    let keys: Vec<&str> = [(col.is_pk, "PK"), (col.is_fk, "FK")]
        .into_iter()
        .filter_map(|(on, k)| on.then_some(k))
        .collect();
    if !keys.is_empty() {
        line.push(' ');
        line.push_str(&keys.join(", "));
    }
    if name != col.name {
        line.push_str(&format!(" \"name: {}\"", quote_safe(&col.name)));
    }
    line
}

/// Kata atribut Mermaid: `[A-Za-z_][A-Za-z0-9_()\[\]-]*`. Karakter lain
/// (spasi, koma di `decimal(10,2)`, titik) menjadi `_`.
fn sanitize_word(raw: &str, fallback: &str) -> String {
    let mut out: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '(' | ')' | '[' | ']' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        return fallback.to_string();
    }
    if !out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        out.insert(0, '_');
    }
    out
}

/// Id entity Mermaid: `[A-Za-z_][A-Za-z0-9_]*`.
fn sanitize_entity(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || !out.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        out.insert(0, '_');
    }
    out
}

fn quote_safe(s: &str) -> String {
    s.replace('"', "'")
}

/// Pemetaan nama tabel asli -> id Mermaid yang unik.
struct EntityIds {
    ids: HashMap<String, String>,
}

impl EntityIds {
    fn new(model: &ErModel) -> Self {
        let mut names: Vec<&str> = model.entities.iter().map(|e| e.name.as_str()).collect();
        for rel in &model.relations {
            names.push(&rel.child);
            names.push(&rel.parent);
        }
        let mut ids = HashMap::new();
        let mut used = HashSet::new();
        for name in names {
            if ids.contains_key(name) {
                continue;
            }
            let base = sanitize_entity(name);
            let mut id = base.clone();
            let mut n = 2;
            while !used.insert(id.clone()) {
                id = format!("{base}_{n}");
                n += 1;
            }
            ids.insert(name.to_string(), id);
        }
        Self { ids }
    }

    fn get<'a>(&'a self, name: &'a str) -> &'a str {
        self.ids.get(name).map(String::as_str).unwrap_or(name)
    }
}

/// Parse teks `erDiagram`. Menerima juga Markdown: blok ```` ```mermaid ````
/// pertama yang berisi `erDiagram` diambil (mis. catatan skema dari vault).
pub fn parse_mermaid_er(text: &str) -> Result<ParsedEr, String> {
    let source = extract_er_block(text).ok_or_else(|| {
        "no `erDiagram` found; expected Mermaid ER text or a ```mermaid block".to_string()
    })?;

    let mut warnings = Vec::new();
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut entities: Vec<ErEntity> = Vec::new();
    let mut relations: Vec<(String, String, String, String)> = Vec::new(); // parent, child, label, card
    let mut current: Option<ErEntity> = None;

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();
        if line.is_empty() || line == "erDiagram" {
            continue;
        }
        if let Some(comment) = line.strip_prefix("%%") {
            let comment = comment.trim();
            if let Some(rest) = comment.strip_prefix("table ")
                && let Some((id, original)) = rest.split_once(" = ")
            {
                aliases.insert(id.trim().to_string(), original.trim().to_string());
            } else if let Some(rest) = comment.strip_prefix("group ")
                && let Some((title, members)) = rest.split_once(':')
            {
                groups.push((
                    title.trim().to_string(),
                    members
                        .split(',')
                        .map(|m| m.trim().to_string())
                        .filter(|m| !m.is_empty())
                        .collect(),
                ));
            }
            continue;
        }

        if let Some(entity) = current.as_mut() {
            if line == "}" {
                entities.push(current.take().expect("current entity"));
                continue;
            }
            match parse_attribute(line) {
                Some(col) => entity.columns.push(col),
                None => warnings.push(format!("line {line_no}: skipped attribute `{line}`")),
            }
            continue;
        }

        if let Some(head) = line.strip_suffix('{') {
            let (id, label) = parse_entity_token(head.trim());
            let entity = ErEntity {
                name: label.unwrap_or(id),
                ..Default::default()
            };
            current = Some(entity);
            continue;
        }
        if let Some((head, rest)) = line.split_once('{')
            && rest.trim() == "}"
        {
            let (id, label) = parse_entity_token(head.trim());
            entities.push(ErEntity {
                name: label.unwrap_or(id),
                ..Default::default()
            });
            continue;
        }

        if let Some(rel) = parse_relation(line) {
            relations.push(rel);
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() == 1 {
            let (id, label) = parse_entity_token(tokens[0]);
            entities.push(ErEntity {
                name: label.unwrap_or(id),
                ..Default::default()
            });
            continue;
        }
        warnings.push(format!("line {line_no}: skipped `{line}`"));
    }
    if let Some(entity) = current {
        warnings.push(format!(
            "entity `{}` is missing its closing `}}`",
            entity.name
        ));
        entities.push(entity);
    }

    let resolve = |id: &str| aliases.get(id).cloned().unwrap_or_else(|| id.to_string());
    for entity in &mut entities {
        entity.name = resolve(&entity.name);
    }
    for (title, members) in &groups {
        for member in members {
            let name = resolve(member);
            if let Some(e) = entities.iter_mut().find(|e| e.name == name) {
                if !e.groups.contains(title) {
                    e.groups.push(title.clone());
                }
                e.group = e.groups.first().cloned();
            }
        }
    }

    let mut model = ErModel {
        entities,
        relations: Vec::new(),
    };
    for (parent, child, label, card) in relations {
        let parent = resolve(&parent);
        let child = resolve(&child);
        let (child_column, parent_column) = match label.split_once("->") {
            Some((c, p)) => (c.trim().to_string(), p.trim().to_string()),
            None => (String::new(), String::new()),
        };
        for name in [&parent, &child] {
            if model.entity(name).is_none() {
                model.entities.push(ErEntity {
                    name: name.clone(),
                    ..Default::default()
                });
            }
        }
        if !child_column.is_empty()
            && let Some(e) = model.entities.iter_mut().find(|e| e.name == child)
            && let Some(c) = e.columns.iter_mut().find(|c| c.name == child_column)
        {
            c.is_fk = true;
        }
        let rel = ErRelation {
            child,
            child_column,
            parent,
            parent_column,
            inferred: card.contains(".."),
        };
        if !model.relations.contains(&rel) {
            model.relations.push(rel);
        }
    }

    // Dedupe entity dengan nama sama (mis. dideklarasikan dua kali): gabungkan kolom.
    let mut merged: Vec<ErEntity> = Vec::new();
    for entity in model.entities {
        match merged.iter_mut().find(|e| e.name == entity.name) {
            Some(existing) => {
                for col in entity.columns {
                    if !existing.columns.iter().any(|c| c.name == col.name) {
                        existing.columns.push(col);
                    }
                }
                for g in entity.groups {
                    if !existing.groups.contains(&g) {
                        existing.groups.push(g);
                    }
                }
                if existing.group.is_none() {
                    existing.group = entity.group.or_else(|| existing.groups.first().cloned());
                }
            }
            None => merged.push(entity),
        }
    }
    model.entities = merged;

    Ok(ParsedEr { model, warnings })
}

/// Ambil sumber `erDiagram`: teks mentah, atau isi fence mermaid di Markdown.
fn extract_er_block(text: &str) -> Option<String> {
    let mut in_fence = false;
    let mut block = String::new();
    let mut saw_fence = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if !in_fence && trimmed.starts_with("```") && trimmed.contains("mermaid") {
            in_fence = true;
            saw_fence = true;
            block.clear();
            continue;
        }
        if in_fence {
            if trimmed.starts_with("```") {
                in_fence = false;
                if block.lines().any(|l| l.trim() == "erDiagram") {
                    return Some(block);
                }
                continue;
            }
            block.push_str(line);
            block.push('\n');
        }
    }
    if saw_fence {
        return None;
    }

    // Teks mentah: lewati frontmatter `---` Mermaid (title/config) bila ada.
    let mut lines = text.lines().peekable();
    let mut out = String::new();
    if lines.peek().map(|l| l.trim()) == Some("---") {
        lines.next();
        for l in lines.by_ref() {
            if l.trim() == "---" {
                break;
            }
        }
    }
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out.lines().any(|l| l.trim() == "erDiagram").then_some(out)
}

/// `id` atau `id["Label"]` -> (id, label).
fn parse_entity_token(token: &str) -> (String, Option<String>) {
    let token = token.trim().trim_matches('"');
    if let Some((id, rest)) = token.split_once('[') {
        let label = rest
            .trim_end_matches(']')
            .trim()
            .trim_matches('"')
            .to_string();
        return (id.trim().to_string(), (!label.is_empty()).then_some(label));
    }
    (token.to_string(), None)
}

/// `type name [PK, FK] ["comment"]`.
fn parse_attribute(line: &str) -> Option<ErColumn> {
    let (head, comment) = match line.find('"') {
        Some(pos) => (
            &line[..pos],
            Some(line[pos..].trim().trim_matches('"').to_string()),
        ),
        None => (line, None),
    };
    let mut tokens = head.split_whitespace();
    let type_name = tokens.next()?.to_string();
    let mut name = tokens.next()?.trim_start_matches('*').to_string();
    let keys: Vec<String> = tokens
        .flat_map(|t| t.split(','))
        .map(|k| k.trim().to_ascii_uppercase())
        .filter(|k| !k.is_empty())
        .collect();
    if let Some(original) = comment.as_deref().and_then(|c| c.strip_prefix("name: ")) {
        name = original.to_string();
    }
    Some(ErColumn {
        name,
        type_name,
        is_pk: keys.iter().any(|k| k == "PK"),
        is_fk: keys.iter().any(|k| k == "FK"),
        nullable: None,
    })
}

/// `A <kiri>--<kanan> B : label` -> (parent, child, label, kardinalitas).
/// Sisi "one" (`||`, `|o`, `o|`) dianggap parent; bila keduanya sama, A.
fn parse_relation(line: &str) -> Option<(String, String, String, String)> {
    let (lhs, label) = match line.split_once(':') {
        Some((l, r)) => (l.trim(), r.trim().trim_matches('"').to_string()),
        None => (line, String::new()),
    };
    let tokens: Vec<&str> = lhs.split_whitespace().collect();
    if tokens.len() != 3 {
        return None;
    }
    let card = tokens[1];
    let (left, right) = card.split_once("--").or_else(|| card.split_once(".."))?;
    let is_valid = |s: &str| !s.is_empty() && s.chars().all(|c| matches!(c, '|' | 'o' | '{' | '}'));
    if !is_valid(left) || !is_valid(right) {
        return None;
    }
    let (a, _) = parse_entity_token(tokens[0]);
    let (b, _) = parse_entity_token(tokens[2]);
    let left_many = left.contains('}');
    let right_many = right.contains('{');
    let (parent, child) = if left_many && !right_many {
        (b, a)
    } else {
        (a, b)
    };
    Some((parent, child, label, card.to_string()))
}

/// Gabungkan model hasil impor ke state diagram. Tabel yang sudah ada tetap
/// di posisinya; kolom diganti bila entity impor punya kolom. Tabel baru
/// diletakkan dalam grid di kanan diagram (atau auto-layout bila kosong).
pub fn merge_into_state(state: &mut DiagramState, model: &ErModel) -> MergeStats {
    let mut stats = MergeStats::default();
    let was_empty = state.nodes.is_empty();

    let right_edge = state
        .nodes
        .iter()
        .map(|n| n.pos.x + n.size.x)
        .fold(f32::MIN, f32::max);
    let top = state.nodes.iter().map(|n| n.pos.y).fold(f32::MAX, f32::min);
    let origin = if was_empty {
        eframe::egui::pos2(100.0, 100.0)
    } else {
        eframe::egui::pos2(right_edge + 150.0, top)
    };
    let mut new_index = 0usize;

    for entity in &model.entities {
        let group_titles: Vec<String> = if !entity.groups.is_empty() {
            entity.groups.clone()
        } else {
            entity.group.clone().into_iter().collect()
        };
        let group_ids: Vec<String> = group_titles
            .iter()
            .map(|title| ensure_group(state, title))
            .collect();
        let primary_group_id = group_ids.first().cloned();
        let columns: Vec<String> = entity.columns.iter().map(|c| c.name.clone()).collect();
        let meta: Vec<DiagramColumn> = entity
            .columns
            .iter()
            .map(|c| DiagramColumn {
                name: c.name.clone(),
                type_name: c.type_name.clone(),
                is_pk: c.is_pk,
                nullable: c.nullable.unwrap_or(true),
            })
            .collect();

        match state.nodes.iter_mut().find(|n| n.id == entity.name) {
            Some(node) => {
                if !columns.is_empty() {
                    node.columns = columns;
                    node.column_meta = meta;
                }
                if !group_ids.is_empty() {
                    for gid in &group_ids {
                        if !node.group_ids.contains(gid) {
                            node.group_ids.push(gid.clone());
                        }
                    }
                    node.group_id = primary_group_id.or(node.group_id.clone());
                }
                stats.updated_tables += 1;
            }
            None => {
                let col = (new_index % 4) as f32;
                let row = (new_index / 4) as f32;
                new_index += 1;
                state.nodes.push(DiagramNode {
                    id: entity.name.clone(),
                    title: entity.name.clone(),
                    pos: origin + eframe::egui::vec2(col * 260.0, row * 320.0),
                    size: eframe::egui::vec2(180.0, 100.0),
                    columns,
                    foreign_keys: Vec::new(),
                    group_ids: group_ids.clone(),
                    group_id: primary_group_id,
                    column_meta: meta,
                    detached: true,
                    database_name: None,
                    connection_id: None,
                    connection_name: None,
                });
                stats.added_tables += 1;
            }
        }
    }

    // Relasi impor disimpan sebagai relasi virtual (bukan FK node) supaya
    // tidak tertimpa saat skema database di-refresh.
    for rel in &model.relations {
        let is_db_fk = state.nodes.iter().any(|n| {
            n.id == rel.child
                && n.foreign_keys.iter().any(|fk| {
                    fk.referenced_table_name == rel.parent
                        && fk.column_name == rel.child_column
                        && fk.referenced_column_name == rel.parent_column
                })
        });
        if is_db_fk {
            continue;
        }
        let added = crate::diagram_relations::add_virtual_relation(
            state,
            VirtualRelation {
                child: rel.child.clone(),
                child_column: rel.child_column.clone(),
                parent: rel.parent.clone(),
                parent_column: rel.parent_column.clone(),
                origin: if rel.inferred {
                    RelationOrigin::Inferred
                } else {
                    RelationOrigin::Imported
                },
            },
        );
        if added {
            stats.added_relations += 1;
        }
    }

    if was_empty && !state.nodes.is_empty() {
        crate::diagram_view::perform_auto_layout(state);
        state.is_centered = false;
    }
    stats
}

/// Id group berdasarkan judul; dibuat baru bila belum ada.
fn ensure_group(state: &mut DiagramState, title: &str) -> String {
    if let Some(g) = state.groups.iter().find(|g| g.title == title) {
        return g.id.clone();
    }
    let id = format!("group_{}", sanitize_entity(&title.to_lowercase()));
    let color = crate::diagram_view::GROUP_COLORS
        [state.groups.len() % crate::diagram_view::GROUP_COLORS.len()];
    state.groups.push(DiagramGroup {
        id: id.clone(),
        title: title.to_string(),
        color,
        manual_pos: None,
    });
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, pk: bool, fk: bool, nullable: Option<bool>) -> ErColumn {
        ErColumn {
            name: name.into(),
            type_name: ty.into(),
            is_pk: pk,
            is_fk: fk,
            nullable,
        }
    }

    fn sample() -> ErModel {
        ErModel {
            entities: vec![
                ErEntity {
                    name: "customers".into(),
                    columns: vec![
                        col("id", "int", true, false, Some(false)),
                        col("name", "varchar(255)", false, false, Some(true)),
                    ],
                    group: Some("Sales".into()),
                    groups: vec!["Sales".into()],
                },
                ErEntity {
                    name: "orders".into(),
                    columns: vec![
                        col("id", "int", true, false, Some(false)),
                        col("customer_id", "int", false, true, Some(true)),
                        col("total", "decimal(10,2)", false, false, Some(false)),
                    ],
                    group: Some("Sales".into()),
                    groups: vec!["Sales".into()],
                },
            ],
            relations: vec![ErRelation {
                child: "orders".into(),
                child_column: "customer_id".into(),
                parent: "customers".into(),
                parent_column: "id".into(),
                inferred: false,
            }],
        }
    }

    #[test]
    fn renders_entities_keys_and_relations() {
        let text = sample().to_mermaid(MermaidOptions::default());
        assert!(text.starts_with("erDiagram\n"));
        assert!(text.contains("    %% group Sales: customers, orders\n"));
        assert!(text.contains("        int id PK\n"));
        assert!(text.contains("        int customer_id FK\n"));
        // Koma di tipe tidak valid untuk Mermaid.
        assert!(text.contains("        decimal(10_2) total\n"));
        // FK nullable -> relasi opsional.
        assert!(text.contains("    customers |o--o{ orders : \"customer_id -> id\"\n"));
    }

    #[test]
    fn round_trips_through_parser() {
        let model = sample();
        let parsed = parse_mermaid_er(&model.to_mermaid(MermaidOptions::default())).unwrap();
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        let names: Vec<&str> = parsed
            .model
            .entities
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["customers", "orders"]);
        assert_eq!(parsed.model.relations, model.relations);
        let orders = &parsed.model.entities[1];
        assert_eq!(orders.group.as_deref(), Some("Sales"));
        assert!(orders.columns[0].is_pk);
        assert!(orders.columns[1].is_fk);
    }

    #[test]
    fn escapes_awkward_names_and_restores_them() {
        let model = ErModel {
            entities: vec![ErEntity {
                name: "order items".into(),
                columns: vec![col("unit price", "numeric", false, false, None)],
                group: None,
                groups: vec![],
            }],
            relations: vec![],
        };
        let text = model.to_mermaid(MermaidOptions::default());
        assert!(text.contains("%% table order_items = order items"));
        assert!(text.contains("numeric unit_price \"name: unit price\""));
        let parsed = parse_mermaid_er(&text).unwrap();
        assert_eq!(parsed.model.entities[0].name, "order items");
        assert_eq!(parsed.model.entities[0].columns[0].name, "unit price");
    }

    #[test]
    fn colliding_ids_get_suffix() {
        let model = ErModel {
            entities: vec![
                ErEntity {
                    name: "a-b".into(),
                    ..Default::default()
                },
                ErEntity {
                    name: "a b".into(),
                    ..Default::default()
                },
            ],
            relations: vec![],
        };
        let text = model.to_mermaid(MermaidOptions::default());
        assert!(text.contains("%% table a_b = a-b"));
        assert!(text.contains("%% table a_b_2 = a b"));
    }

    #[test]
    fn max_columns_keeps_keys_first() {
        let model = sample();
        let text = model.to_mermaid(MermaidOptions {
            max_columns: Some(2),
            relations_only: false,
        });
        assert!(text.contains("%% orders: showing 2 of 3 columns"));
        assert!(text.contains("int customer_id FK"));
        assert!(!text.contains("total"));
    }

    #[test]
    fn relations_only_skips_attributes_but_keeps_isolated_tables() {
        let mut model = sample();
        model.entities.push(ErEntity {
            name: "audit_log".into(),
            ..Default::default()
        });
        let text = model.to_mermaid(MermaidOptions {
            max_columns: None,
            relations_only: true,
        });
        assert!(!text.contains(" {\n"));
        assert!(text.contains("    audit_log\n"));
        assert!(!text.contains("    customers\n"));
    }

    #[test]
    fn parses_handwritten_markdown_with_reverse_cardinality() {
        let md = "# Notes\n\n```mermaid\nerDiagram\n  ORDER }o--|| CUSTOMER : places\n  \
                  CUSTOMER {\n    string name\n    int id PK \"primary\"\n  }\n  \
                  LINE-ITEM\n```\n";
        let parsed = parse_mermaid_er(md).unwrap();
        let rel = &parsed.model.relations[0];
        assert_eq!(rel.parent, "CUSTOMER");
        assert_eq!(rel.child, "ORDER");
        assert!(rel.child_column.is_empty());
        let customer = parsed
            .model
            .entities
            .iter()
            .find(|e| e.name == "CUSTOMER")
            .unwrap();
        assert_eq!(customer.columns.len(), 2);
        assert!(customer.columns[1].is_pk);
        assert!(parsed.model.entities.iter().any(|e| e.name == "LINE-ITEM"));
    }

    #[test]
    fn rejects_text_without_er_diagram() {
        assert!(parse_mermaid_er("flowchart LR\n A --> B").is_err());
        assert!(parse_mermaid_er("```mermaid\nflowchart LR\n```").is_err());
    }

    #[test]
    fn reports_unknown_lines_as_warnings() {
        let parsed =
            parse_mermaid_er("erDiagram\n  A ||--o{ B : x\n  style A fill:#f9f\n").unwrap();
        assert_eq!(parsed.model.relations.len(), 1);
        assert_eq!(parsed.warnings.len(), 1);
    }

    #[test]
    fn diagram_state_round_trip_via_merge() {
        let mut state = DiagramState::default();
        let stats = merge_into_state(&mut state, &sample());
        assert_eq!(stats.added_tables, 2);
        assert_eq!(stats.added_relations, 1);
        // Relasi impor disimpan sebagai relasi virtual, bukan edge/FK database.
        assert!(state.edges.is_empty());
        assert_eq!(state.virtual_relations.len(), 1);
        assert_eq!(state.virtual_relations[0].origin, RelationOrigin::Imported);
        assert!(state.nodes.iter().all(|n| n.detached));
        assert_eq!(state.groups.len(), 1);

        let back = ErModel::from_diagram(&state);
        assert_eq!(back.relations, sample().relations);
        let orders = back.entities.iter().find(|e| e.name == "orders").unwrap();
        assert_eq!(orders.group.as_deref(), Some("Sales"));
        assert!(
            orders
                .columns
                .iter()
                .any(|c| c.name == "customer_id" && c.is_fk)
        );

        // Merge kedua tidak menduplikasi relasi / node.
        let again = merge_into_state(&mut state, &sample());
        assert_eq!(again.added_tables, 0);
        assert_eq!(again.updated_tables, 2);
        assert_eq!(again.added_relations, 0);
        assert_eq!(state.virtual_relations.len(), 1);
    }

    #[test]
    fn inferred_relations_use_dotted_line_and_round_trip() {
        let mut model = sample();
        model.relations[0].inferred = true;
        let text = model.to_mermaid(MermaidOptions::default());
        assert!(text.contains("customers |o..o{ orders"));
        let parsed = parse_mermaid_er(&text).unwrap();
        assert!(parsed.model.relations[0].inferred);

        let mut state = DiagramState::default();
        merge_into_state(&mut state, &parsed.model);
        assert_eq!(state.virtual_relations[0].origin, RelationOrigin::Inferred);
    }

    #[test]
    fn schema_note_contains_mermaid_block_and_wikilinks() {
        let md = schema_note_markdown("Schema: shop", &sample());
        assert!(md.contains("```mermaid\nerDiagram\n"));
        assert!(md.contains("- [[orders]] — 3 columns, PK id, FK customer_id → customers.id"));
        // Catatan hasil generate bisa diimpor balik.
        let parsed = parse_mermaid_er(&md).unwrap();
        assert_eq!(parsed.model.entities.len(), 2);
    }

    #[test]
    fn multi_group_per_table_round_trip() {
        let mut model = sample();
        // Add "orders" to a second group "Finance"
        let orders = model
            .entities
            .iter_mut()
            .find(|e| e.name == "orders")
            .unwrap();
        orders.groups = vec!["Sales".into(), "Finance".into()];

        let text = model.to_mermaid(MermaidOptions::default());
        assert!(text.contains("%% group Sales:"));
        assert!(text.contains("%% group Finance:"));

        let parsed = parse_mermaid_er(&text).unwrap();
        let parsed_orders = parsed
            .model
            .entities
            .iter()
            .find(|e| e.name == "orders")
            .unwrap();
        assert!(parsed_orders.groups.contains(&"Sales".to_string()));
        assert!(parsed_orders.groups.contains(&"Finance".to_string()));

        let mut state = DiagramState::default();
        merge_into_state(&mut state, &parsed.model);
        let node = state.nodes.iter().find(|n| n.id == "orders").unwrap();
        assert_eq!(node.group_ids.len(), 2);
        assert!(node.is_in_group(&node.group_ids[0]));
        assert!(node.is_in_group(&node.group_ids[1]));
    }

    #[test]
    fn diagram_node_multi_group_helpers() {
        let mut node = DiagramNode {
            id: "users".into(),
            title: "users".into(),
            pos: eframe::egui::Pos2::ZERO,
            size: eframe::egui::Vec2::ZERO,
            columns: vec![],
            foreign_keys: vec![],
            group_ids: vec![],
            group_id: Some("group_auth".into()), // legacy field
            column_meta: vec![],
            detached: false,
            database_name: None,
            connection_id: None,
            connection_name: None,
        };

        // ensure_groups_migrated migrates legacy group_id
        node.ensure_groups_migrated();
        assert_eq!(node.group_ids, vec!["group_auth"]);
        assert!(node.is_in_group("group_auth"));

        // add a second group
        node.add_to_group("group_admin".into());
        assert_eq!(node.group_ids.len(), 2);
        assert!(node.is_in_group("group_auth"));
        assert!(node.is_in_group("group_admin"));

        // remove the first group
        node.remove_from_group("group_auth");
        assert!(!node.is_in_group("group_auth"));
        assert!(node.is_in_group("group_admin"));
        assert_eq!(node.group_id.as_deref(), Some("group_admin"));
    }
}
