//! Logika headless untuk parsing dan pengelompokan tabel berdasarkan komentar database.
//!
//! User dapat menentukan pola/format seperti `[GROUP]-[SUB GROUP]-[Comment Table]`
//! atau variasi delimiter lainnya. Modul ini mengekstrak group, sub-group, dan deskripsi tabel.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Konfigurasi pola pengelompokan tabel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableGroupConfig {
    /// Format pola, misalnya `"[GROUP]-[SUB GROUP]-[Comment Table]"`.
    pub pattern: String,
    /// Apakah pengelompokan berdasarkan komentar diaktifkan.
    pub enabled: bool,
}

impl Default for TableGroupConfig {
    fn default() -> Self {
        Self {
            pattern: "[GROUP]-[SUB GROUP]-[Comment Table]".to_string(),
            enabled: true,
        }
    }
}

/// Hasil ekstraksi komentar tabel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTableComment {
    /// Nama group utama (misal: "HR", "FINANCE", atau "Ungrouped").
    pub group: String,
    /// Nama sub-group opsional (misal: Some("PAYROLL")).
    pub sub_group: Option<String>,
    /// Deskripsi atau komentar tabel yang sudah bersih dari tag group.
    pub description: Option<String>,
}

/// Ekstrak bagian yang ada di dalam kurung siku `[...]`.
fn extract_bracketed_parts(text: &str) -> (Vec<String>, String) {
    let mut parts = Vec::new();
    let mut in_bracket = false;
    let mut current = String::new();
    let mut last_bracket_end = None;

    for (idx, ch) in text.char_indices() {
        if ch == '[' {
            in_bracket = true;
            current.clear();
        } else if ch == ']' {
            if in_bracket {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    parts.push(trimmed.to_string());
                }
                current.clear();
                in_bracket = false;
                last_bracket_end = Some(idx + ch.len_utf8());
            }
        } else if in_bracket {
            current.push(ch);
        }
    }

    let clean_remainder = if let Some(end_idx) = last_bracket_end {
        let trailing = &text[end_idx..];
        trailing
            .trim()
            .trim_start_matches(['-', '/', '_', ':', '|', '.'])
            .trim()
            .to_string()
    } else {
        String::new()
    };

    (parts, clean_remainder)
}

/// Pisahkan teks berdasarkan delimiter umum jika tidak menggunakan kurung siku.
fn extract_delimited_parts(text: &str, delimiter: char) -> Vec<String> {
    text.split(delimiter)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Deteksi delimiter utama pada pola yang diinputkan user.
fn detect_delimiter(pattern: &str) -> char {
    for ch in pattern.chars() {
        if matches!(ch, '-' | '/' | '|' | '_' | ':') {
            return ch;
        }
    }
    '-'
}

/// Parse komentar tabel mentah berdasarkan pola yang ditentukan.
pub fn parse_table_comment(pattern: &str, raw_comment: Option<&str>) -> ParsedTableComment {
    let raw = match raw_comment {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => {
            return ParsedTableComment {
                group: "Ungrouped".to_string(),
                sub_group: None,
                description: None,
            };
        }
    };

    let (bracketed, remainder) = extract_bracketed_parts(raw);

    if !bracketed.is_empty() {
        match bracketed.len() {
            // Contoh: [GROUP]-[SUB GROUP]-[Comment Table]
            3.. => {
                let group = bracketed[0].clone();
                let sub_group = Some(bracketed[1].clone());
                // Bagian ke-3 adalah komentar
                let mut desc = bracketed[2].clone();
                if !remainder.is_empty() {
                    desc = format!("{desc} {remainder}");
                }
                ParsedTableComment {
                    group,
                    sub_group,
                    description: Some(desc.trim().to_string()),
                }
            }
            // Contoh: [GROUP]-[SUB GROUP] dengan sisa deskripsi di luar bracket
            2 => {
                let group = bracketed[0].clone();
                let sub_group = Some(bracketed[1].clone());
                let description = if !remainder.is_empty() {
                    Some(remainder)
                } else {
                    None
                };
                ParsedTableComment {
                    group,
                    sub_group,
                    description,
                }
            }
            // Contoh: [GROUP] Sisa deskripsi atau hanya [GROUP]
            1 => {
                let group = bracketed[0].clone();
                let description = if !remainder.is_empty() {
                    Some(remainder)
                } else {
                    None
                };
                ParsedTableComment {
                    group,
                    sub_group: None,
                    description,
                }
            }
            _ => unreachable!(),
        }
    } else {
        // Jika tidak ada kurung siku, coba pisahkan berdasarkan delimiter yang dipakai pola
        let delim = detect_delimiter(pattern);
        let parts = extract_delimited_parts(raw, delim);

        if parts.len() >= 3 {
            ParsedTableComment {
                group: parts[0].clone(),
                sub_group: Some(parts[1].clone()),
                description: Some(parts[2..].join(&format!(" {delim} "))),
            }
        } else if parts.len() == 2 {
            // Periksa apakah pola menentukan 3 bagian atau 2 bagian
            let pattern_has_subgroup = pattern.to_uppercase().contains("SUB");
            let pattern_has_comment = pattern.to_uppercase().contains("COMMENT")
                || pattern.to_uppercase().contains("DESC");

            if pattern_has_subgroup && !pattern_has_comment {
                ParsedTableComment {
                    group: parts[0].clone(),
                    sub_group: Some(parts[1].clone()),
                    description: None,
                }
            } else if pattern_has_subgroup && pattern_has_comment {
                // Dianggap Group dan Sub Group
                ParsedTableComment {
                    group: parts[0].clone(),
                    sub_group: Some(parts[1].clone()),
                    description: None,
                }
            } else {
                ParsedTableComment {
                    group: parts[0].clone(),
                    sub_group: None,
                    description: Some(parts[1].clone()),
                }
            }
        } else if parts.len() == 1 {
            // Hanya 1 kata/frasa tanpa delimiter
            ParsedTableComment {
                group: "Ungrouped".to_string(),
                sub_group: None,
                description: Some(raw.to_string()),
            }
        } else {
            ParsedTableComment {
                group: "Ungrouped".to_string(),
                sub_group: None,
                description: None,
            }
        }
    }
}

/// Item tabel yang sudah dianalisis grupnya.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupedTableItem {
    pub name: String,
    pub raw_comment: Option<String>,
    pub parsed: ParsedTableComment,
}

/// Hirarki Sub Group berisi daftar tabel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableSubGroupTree {
    pub sub_group_name: String,
    pub tables: Vec<GroupedTableItem>,
}

/// Hirarki Group utama berisi tabel langsung dan sub group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableGroupTree {
    pub group_name: String,
    pub direct_tables: Vec<GroupedTableItem>,
    pub sub_groups: Vec<TableSubGroupTree>,
}

/// Bangun pohon hirarki Group -> Sub Group -> Tabel.
pub fn build_table_group_tree(
    pattern: &str,
    tables: &[(String, Option<String>)],
) -> Vec<TableGroupTree> {
    // Map: Group -> (Direct Tables, Map<SubGroup, Tables>)
    let mut groups: BTreeMap<String, (Vec<GroupedTableItem>, BTreeMap<String, Vec<GroupedTableItem>>)> =
        BTreeMap::new();

    for (tbl_name, raw_comment) in tables {
        let parsed = parse_table_comment(pattern, raw_comment.as_deref());
        let item = GroupedTableItem {
            name: tbl_name.clone(),
            raw_comment: raw_comment.clone(),
            parsed: parsed.clone(),
        };

        let entry = groups.entry(parsed.group).or_default();
        if let Some(sub) = parsed.sub_group {
            entry.1.entry(sub).or_default().push(item);
        } else {
            entry.0.push(item);
        }
    }

    let mut result = Vec::new();
    for (group_name, (mut direct, sub_map)) in groups {
        direct.sort_by_key(|a| a.name.to_lowercase());
        let mut sub_groups = Vec::new();
        for (sub_name, mut sub_tables) in sub_map {
            sub_tables.sort_by_key(|a| a.name.to_lowercase());
            sub_groups.push(TableSubGroupTree {
                sub_group_name: sub_name,
                tables: sub_tables,
            });
        }
        sub_groups.sort_by_key(|a| a.sub_group_name.to_lowercase());

        result.push(TableGroupTree {
            group_name,
            direct_tables: direct,
            sub_groups,
        });
    }

    // Urutkan supaya "Ungrouped" berada di paling akhir
    result.sort_by(|a, b| {
        if a.group_name.eq_ignore_ascii_case("Ungrouped") {
            std::cmp::Ordering::Greater
        } else if b.group_name.eq_ignore_ascii_case("Ungrouped") {
            std::cmp::Ordering::Less
        } else {
            a.group_name.to_lowercase().cmp(&b.group_name.to_lowercase())
        }
    });

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_pattern() {
        let pattern = "[GROUP]-[SUB GROUP]-[Comment Table]";
        let comment = "[HR]-[PAYROLL]-[Daftar Penggajian]";
        let res = parse_table_comment(pattern, Some(comment));
        assert_eq!(res.group, "HR");
        assert_eq!(res.sub_group.as_deref(), Some("PAYROLL"));
        assert_eq!(res.description.as_deref(), Some("Daftar Penggajian"));
    }

    #[test]
    fn test_parse_two_brackets_with_remainder() {
        let pattern = "[GROUP]-[SUB GROUP]-[Comment Table]";
        let comment = "[FINANCE]-[INVOICING] - Data faktur tagihan";
        let res = parse_table_comment(pattern, Some(comment));
        assert_eq!(res.group, "FINANCE");
        assert_eq!(res.sub_group.as_deref(), Some("INVOICING"));
        assert_eq!(res.description.as_deref(), Some("Data faktur tagihan"));
    }

    #[test]
    fn test_parse_only_group_in_brackets() {
        let pattern = "[GROUP]-[SUB GROUP]-[Comment Table]";
        let comment = "[AUTH] Tabel pengguna";
        let res = parse_table_comment(pattern, Some(comment));
        assert_eq!(res.group, "AUTH");
        assert_eq!(res.sub_group, None);
        assert_eq!(res.description.as_deref(), Some("Tabel pengguna"));
    }

    #[test]
    fn test_parse_without_brackets_delimiter() {
        let pattern = "[GROUP]/[SUB GROUP]/[Comment Table]";
        let comment = "SALES / ORDERS / Data Pesanan";
        let res = parse_table_comment(pattern, Some(comment));
        assert_eq!(res.group, "SALES");
        assert_eq!(res.sub_group.as_deref(), Some("ORDERS"));
        assert_eq!(res.description.as_deref(), Some("Data Pesanan"));
    }

    #[test]
    fn test_parse_empty_and_fallback() {
        let pattern = "[GROUP]-[SUB GROUP]-[Comment Table]";
        let res1 = parse_table_comment(pattern, None);
        assert_eq!(res1.group, "Ungrouped");
        assert_eq!(res1.sub_group, None);
        assert_eq!(res1.description, None);

        let res2 = parse_table_comment(pattern, Some("Hanya komentar biasa tanpa pola"));
        assert_eq!(res2.group, "Ungrouped");
        assert_eq!(res2.description.as_deref(), Some("Hanya komentar biasa tanpa pola"));
    }

    #[test]
    fn test_build_tree_hierarchy() {
        let pattern = "[GROUP]-[SUB GROUP]-[Comment Table]";
        let tables = vec![
            ("users".to_string(), Some("[AUTH]-[USER]-[Tabel akun]".to_string())),
            ("roles".to_string(), Some("[AUTH]-[ROLE]-[Tabel peran]".to_string())),
            ("payroll".to_string(), Some("[HR]-[PAYROLL]-[Gaji]".to_string())),
            ("employees".to_string(), Some("[HR] Data Karyawan".to_string())),
            ("logs".to_string(), Some("Catatan log".to_string())),
        ];

        let tree = build_table_group_tree(pattern, &tables);
        // Expect groups: AUTH, HR, Ungrouped
        let group_names: Vec<&str> = tree.iter().map(|g| g.group_name.as_str()).collect();
        assert_eq!(group_names, vec!["AUTH", "HR", "Ungrouped"]);

        let auth = &tree[0];
        assert_eq!(auth.sub_groups.len(), 2);
        assert_eq!(auth.sub_groups[0].sub_group_name, "ROLE");
        assert_eq!(auth.sub_groups[1].sub_group_name, "USER");

        let hr = &tree[1];
        assert_eq!(hr.direct_tables.len(), 1);
        assert_eq!(hr.direct_tables[0].name, "employees");
        assert_eq!(hr.sub_groups.len(), 1);
        assert_eq!(hr.sub_groups[0].sub_group_name, "PAYROLL");

        let ungr = &tree[2];
        assert_eq!(ungr.direct_tables.len(), 1);
        assert_eq!(ungr.direct_tables[0].name, "logs");
    }
}
