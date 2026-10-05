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

/// Satu-satunya tempat penanda teks [`NULL_MARKER`] ditafsirkan sebagai SQL
/// NULL.
///
/// Baris hasil eksekutor query (`Vec<Vec<String>>` dari `src/connection/*`,
/// juga isi grid) tidak membawa nullness: SQL NULL dan string berisi empat
/// huruf `NULL` sama-sama tiba sebagai `"NULL"`. Di titik ini keduanya tidak
/// bisa dibedakan, jadi string `NULL` asli ikut dianggap NULL. Pemanggil yang
/// bisa menanyakan nullness ke database (lihat `catalog::PageSelect`) atau ke
/// file (pembaca di `readers`) tidak melewati fungsi ini untuk kolom
/// tersebut. Batasan ini baru hilang bila eksekutor mengembalikan nullness
/// per sel (mis. `Vec<Vec<Option<String>>>` atau bitmap sejajar baris).
pub fn cell_from_executor(cell: &str) -> Option<&str> {
    (cell != NULL_MARKER).then_some(cell)
}

/// Bitmap nullness per sel, baris demi baris dengan lebar tetap.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct NullMask {
    width: usize,
    bits: Vec<u64>,
}

impl NullMask {
    fn new(width: usize, rows: usize) -> Self {
        Self {
            width,
            bits: vec![0; (width * rows).div_ceil(64)],
        }
    }

    fn get(&self, row: usize, col: usize) -> bool {
        if col >= self.width {
            return true;
        }
        let i = row * self.width + col;
        self.bits
            .get(i / 64)
            .is_none_or(|word| word & (1 << (i % 64)) != 0)
    }

    fn set(&mut self, row: usize, col: usize) {
        let i = row * self.width + col;
        if col < self.width
            && let Some(word) = self.bits.get_mut(i / 64)
        {
            *word |= 1 << (i % 64);
        }
    }

    /// Bitmap yang sama dengan lebar baru; kolom tambahan bernilai NULL.
    fn widened(&self, width: usize, rows: usize) -> Self {
        let mut out = Self::new(width, rows);
        for row in 0..rows {
            for col in 0..width {
                if self.get(row, col) {
                    out.set(row, col);
                }
            }
        }
        out
    }
}

/// Tabel dalam bentuk teks: header + baris. Bentuk yang sama dengan hasil
/// query di grid, sehingga bisa langsung diekspor atau dimuat.
///
/// Nullness punya dua mode. Tabel dari grid ([`TableData::new`]) hanya punya
/// teks, jadi sel `"NULL"` ditafsirkan lewat [`cell_from_executor`]. Tabel
/// dari file atau dari query yang membawa indikator NULL
/// ([`TableData::from_cells`]) menyimpan bitmap eksplisit; di sana string
/// `NULL` adalah string biasa. Sel NULL tetap ditulis sebagai [`NULL_MARKER`]
/// di `rows` supaya tampilan lama tidak berubah; jangan menafsirkan `rows`
/// langsung, pakai [`TableData::value`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    nulls: Option<NullMask>,
}

impl TableData {
    /// Tabel dari baris teks grid/eksekutor (nullness lewat penanda).
    pub fn new(headers: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self {
            headers,
            rows,
            nulls: None,
        }
    }

    /// Tabel dengan nullness eksplisit: `None` = SQL NULL. Baris pendek diisi
    /// NULL sampai selebar baris terpanjang (atau header).
    pub fn from_cells(headers: Vec<String>, cells: Vec<Vec<Option<String>>>) -> Self {
        let width = cells
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
            .max(headers.len());
        let mut mask = NullMask::new(width, cells.len());
        let rows = cells
            .into_iter()
            .enumerate()
            .map(|(r, row)| {
                let mut out = Vec::with_capacity(width);
                let mut cells = row.into_iter();
                for c in 0..width {
                    match cells.next().flatten() {
                        Some(text) => out.push(text),
                        None => {
                            mask.set(r, c);
                            out.push(NULL_MARKER.to_string());
                        }
                    }
                }
                out
            })
            .collect();
        Self {
            headers,
            rows,
            nulls: Some(mask),
        }
    }

    /// True bila nullness disimpan eksplisit (bukan lewat penanda teks).
    pub fn has_explicit_nulls(&self) -> bool {
        self.nulls.is_some()
    }

    /// Nilai sel; kolom yang tidak ada di baris pendek dianggap string kosong.
    pub fn cell(&self, row: usize, col: usize) -> &str {
        self.rows
            .get(row)
            .and_then(|r| r.get(col))
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Nilai sel dengan nullness: `None` = SQL NULL (juga untuk sel yang
    /// tidak ada).
    pub fn value(&self, row: usize, col: usize) -> Option<&str> {
        let text = self.rows.get(row)?.get(col)?.as_str();
        match &self.nulls {
            Some(mask) => (!mask.get(row, col)).then_some(text),
            None => cell_from_executor(text),
        }
    }

    pub fn is_null(&self, row: usize, col: usize) -> bool {
        self.value(row, col).is_none()
    }

    /// Baris sebagai sel ber-nullness eksplisit.
    pub fn into_cells(self) -> Vec<Vec<Option<String>>> {
        let nulls = self.nulls;
        self.rows
            .into_iter()
            .enumerate()
            .map(|(r, row)| {
                row.into_iter()
                    .enumerate()
                    .map(|(c, text)| match &nulls {
                        Some(mask) => (!mask.get(r, c)).then_some(text),
                        None => cell_from_executor(&text).is_some().then_some(text),
                    })
                    .collect()
            })
            .collect()
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
        if let Some(mask) = &self.nulls
            && mask.width != width
        {
            self.nulls = Some(mask.widened(width, self.rows.len()));
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
        assert!(data.is_null(0, 1));
        assert_eq!(data.value(1, 1), Some("b"));
    }

    #[test]
    fn explicit_nulls_keep_the_text_null_as_a_string() {
        let mut data = TableData::from_cells(
            vec!["a".into(), "b".into(), "c".into()],
            vec![
                vec![Some("NULL".into()), None],
                vec![Some("x".into()), Some("".into()), Some("z".into())],
            ],
        );
        data.normalize();
        assert!(data.has_explicit_nulls());
        // Teks `NULL` dari file adalah string; sel kosong/absen yang NULL.
        assert_eq!(data.value(0, 0), Some("NULL"));
        assert_eq!(data.value(0, 1), None);
        assert_eq!(data.value(0, 2), None);
        assert_eq!(data.value(1, 1), Some(""));
        assert_eq!(data.rows[0], vec!["NULL", "NULL", "NULL"]);
        assert_eq!(
            data.clone().into_cells()[0],
            vec![Some("NULL".to_string()), None, None]
        );

        // Mode penanda (grid): teks `NULL` tidak bisa dibedakan dari NULL.
        let grid = TableData::new(vec!["a".into()], vec![vec!["NULL".into()]]);
        assert!(!grid.has_explicit_nulls());
        assert_eq!(grid.value(0, 0), None);
        assert_eq!(grid.value(9, 9), None);
    }
}
