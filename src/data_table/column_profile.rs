//! Profil satu kolom hasil query (C-profil): jumlah NULL, kosong, distinct,
//! nilai terbanyak, rentang angka/tanggal, dan panjang teks.
//!
//! Dihitung murni dari baris yang sudah dimuat di grid, jadi berlaku untuk
//! semua engine tanpa query tambahan. Hasilnya menyebut berapa baris yang
//! dianalisis supaya tidak disangka mewakili seluruh tabel.

use crate::models::structs::is_null_cell;
use std::collections::HashMap;

/// Jumlah nilai terbanyak yang ditampilkan.
pub const TOP_VALUES: usize = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct NumericStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub sum: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ColumnProfile {
    pub column: String,
    pub rows: usize,
    pub nulls: usize,
    /// String kosong atau hanya spasi (bukan NULL).
    pub blanks: usize,
    pub distinct: usize,
    /// `(nilai, jumlah)` terurut menurun, maksimal [`TOP_VALUES`].
    pub top: Vec<(String, usize)>,
    /// Ada bila semua nilai non-NULL non-kosong adalah angka.
    pub numeric: Option<NumericStats>,
    /// Nilai terkecil/terbesar secara leksikografis (berguna untuk tanggal ISO).
    pub min_text: Option<String>,
    pub max_text: Option<String>,
    pub min_len: usize,
    pub max_len: usize,
}

impl ColumnProfile {
    pub fn non_null(&self) -> usize {
        self.rows - self.nulls
    }

    pub fn percent(&self, n: usize) -> f64 {
        if self.rows == 0 {
            0.0
        } else {
            n as f64 * 100.0 / self.rows as f64
        }
    }

    /// Kolom tampak unik (kandidat kunci): semua nilai terisi dan berbeda.
    pub fn looks_unique(&self) -> bool {
        self.rows > 0 && self.nulls == 0 && self.distinct == self.rows
    }
}

fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Profilkan nilai-nilai satu kolom.
pub fn profile<'a>(column: &str, values: impl IntoIterator<Item = &'a str>) -> ColumnProfile {
    let mut rows = 0;
    let mut nulls = 0;
    let mut blanks = 0;
    let mut counts: HashMap<&'a str, usize> = HashMap::new();
    let mut all_numeric = true;
    let mut num_count = 0usize;
    let (mut min, mut max, mut sum) = (f64::INFINITY, f64::NEG_INFINITY, 0.0);
    let mut min_text: Option<&'a str> = None;
    let mut max_text: Option<&'a str> = None;
    let (mut min_len, mut max_len) = (usize::MAX, 0usize);

    for v in values {
        rows += 1;
        if is_null_cell(v) {
            nulls += 1;
            continue;
        }
        *counts.entry(v).or_insert(0) += 1;
        let len = v.chars().count();
        min_len = min_len.min(len);
        max_len = max_len.max(len);
        if v.trim().is_empty() {
            blanks += 1;
            continue;
        }
        if min_text.is_none_or(|m| v < m) {
            min_text = Some(v);
        }
        if max_text.is_none_or(|m| v > m) {
            max_text = Some(v);
        }
        if all_numeric {
            match parse_number(v) {
                Some(n) => {
                    num_count += 1;
                    min = min.min(n);
                    max = max.max(n);
                    sum += n;
                }
                None => all_numeric = false,
            }
        }
    }

    let distinct = counts.len();
    let mut top: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, c)| (k.to_string(), c))
        .collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(TOP_VALUES);

    let numeric = (all_numeric && num_count > 0).then(|| NumericStats {
        min,
        max,
        mean: sum / num_count as f64,
        sum,
    });
    if min_len == usize::MAX {
        min_len = 0;
    }

    ColumnProfile {
        column: column.to_string(),
        rows,
        nulls,
        blanks,
        distinct,
        top,
        numeric,
        min_text: min_text.map(str::to_string),
        max_text: max_text.map(str::to_string),
        min_len,
        max_len,
    }
}

/// Format angka ringkas untuk ditampilkan (tanpa notasi ilmiah untuk nilai biasa).
pub fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::structs::NULL_CELL as NULL;

    #[test]
    fn counts_nulls_blanks_and_distinct() {
        let p = profile("c", ["a", NULL, "", "a", "b", "  "]);
        assert_eq!(p.rows, 6);
        assert_eq!(p.nulls, 1);
        assert_eq!(p.blanks, 2);
        assert_eq!(p.distinct, 4); // a, "", b, "  "
        assert_eq!(p.top[0], ("a".to_string(), 2));
        assert!(p.numeric.is_none());
        assert_eq!(p.min_text.as_deref(), Some("a"));
        assert_eq!(p.max_text.as_deref(), Some("b"));
    }

    #[test]
    fn numeric_stats_when_all_values_are_numbers() {
        let p = profile("n", ["1", "2.5", NULL, "-3"]);
        let n = p.numeric.unwrap();
        assert_eq!(n.min, -3.0);
        assert_eq!(n.max, 2.5);
        assert_eq!(n.sum, 0.5);
        assert!((n.mean - 0.5 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn mixed_values_are_not_numeric() {
        assert!(profile("n", ["1", "x"]).numeric.is_none());
        assert!(profile("n", [NULL]).numeric.is_none());
    }

    #[test]
    fn unique_detection_and_empty_input() {
        assert!(profile("id", ["1", "2", "3"]).looks_unique());
        assert!(!profile("id", ["1", "1"]).looks_unique());
        let empty = profile("x", std::iter::empty());
        assert_eq!(empty.rows, 0);
        assert!(!empty.looks_unique());
        assert_eq!(empty.percent(1), 0.0);
        assert_eq!(empty.min_len, 0);
    }

    #[test]
    fn top_values_are_capped_and_ordered() {
        let values: Vec<String> = (0..30).map(|i| (i % 12).to_string()).collect();
        let p = profile("v", values.iter().map(String::as_str));
        assert_eq!(p.top.len(), TOP_VALUES);
        assert!(p.top.windows(2).all(|w| w[0].1 >= w[1].1));
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(2.5), "2.5");
        assert_eq!(format_number(1.0 / 3.0), "0.3333");
    }
}
