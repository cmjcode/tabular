//! Import, ekspor, dan transfer data (bagian H checklist TablePro).
//!
//! Semua modul di sini headless: menerima data biasa (`&ConnectionConfig`,
//! pool, `TableData`) dan tidak bergantung pada `window_egui`. GUI-nya ada di
//! `window_egui::transfer_ui`.

pub mod catalog;
pub mod compare;
pub mod compare_structure;
pub mod data_files;
pub mod encoding;
pub mod encrypt;
pub mod formats;
pub mod object_export;
pub mod readers;
pub mod saved;
pub mod transfer;
pub mod types;
pub mod values;

/// Penanda sel SQL NULL yang dipakai grid dan konverter driver.
pub const NULL_MARKER: &str = "NULL";

/// True bila sel mewakili SQL NULL.
pub fn is_null_cell(cell: &str) -> bool {
    cell == NULL_MARKER
}

/// Tabel dalam bentuk teks: header + baris. Bentuk yang sama dengan hasil
/// query di grid, sehingga bisa langsung diekspor atau dimuat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl TableData {
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self { headers, rows }
    }

    /// Nilai sel; kolom yang tidak ada di baris pendek dianggap string kosong.
    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Samakan lebar semua baris: baris pendek diisi NULL, dan header ditambah
    /// bila ada baris yang lebih lebar. Header kosong atau kembar diberi nama unik.
    pub fn normalize(&mut self) {
        let width = self
            .rows
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(self.headers.len());
        while self.headers.len() < width {
            self.headers.push(String::new());
        }
        self.headers = unique_headers(&self.headers);
        for row in &mut self.rows {
            row.resize(width, NULL_MARKER.to_string());
        }
    }
}

/// Nama kolom unik dan tidak kosong: `col_3` untuk header kosong, akhiran
/// `_2`, `_3` untuk nama kembar (tanpa membedakan huruf besar/kecil).
pub fn unique_headers(headers: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let base = if h.trim().is_empty() {
                format!("col_{}", i + 1)
            } else {
                h.trim().to_string()
            };
            let mut name = base.clone();
            let mut n = 2;
            while !seen.insert(name.to_lowercase()) {
                name = format!("{base}_{n}");
                n += 1;
            }
            name
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_pads_rows_and_names_columns() {
        let mut data = TableData::new(
            vec!["id".into(), "".into(), "ID".into()],
            vec![
                vec!["1".into()],
                vec!["2".into(), "b".into(), "c".into(), "d".into()],
            ],
        );
        data.normalize();
        assert_eq!(data.headers, vec!["id", "col_2", "ID_2", "col_4"]);
        assert_eq!(data.rows[0], vec!["1", "NULL", "NULL", "NULL"]);
        assert_eq!(data.rows[1].len(), 4);
    }
}
