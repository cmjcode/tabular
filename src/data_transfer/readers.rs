//! Pembaca file data: CSV/TSV, JSON, NDJSON, spreadsheet (XLSX/XLS/ODS), dan
//! Parquet; juga yang terkompresi (`.gz`, `.zip`, `.zst`) atau terenkripsi
//! (`.enc` hasil ekspor Tabular). Hasilnya selalu [`TableData`].

use std::io::{Cursor, Read};
use std::path::Path;

use super::TableData;
use super::encoding::{self, TextEncoding};
use super::encrypt::{self, EncryptError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Delimited,
    Json,
    Ndjson,
    Spreadsheet,
    Parquet,
}

impl FileKind {
    pub fn label(self) -> &'static str {
        match self {
            FileKind::Delimited => "Delimited text",
            FileKind::Json => "JSON",
            FileKind::Ndjson => "NDJSON",
            FileKind::Spreadsheet => "Spreadsheet",
            FileKind::Parquet => "Parquet",
        }
    }

    /// Format teks: encoding berlaku.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            FileKind::Delimited | FileKind::Json | FileKind::Ndjson
        )
    }
}

/// Ekstensi yang ditawarkan dialog pilih file.
pub const FILE_EXTENSIONS: &[&str] = &[
    "csv", "tsv", "tab", "txt", "psv", "json", "ndjson", "jsonl", "xlsx", "xlsm", "xlsb", "xls",
    "ods", "parquet", "pq", "gz", "zip", "zst", "enc",
];

#[derive(Clone, Debug)]
pub struct ReadOptions {
    /// `None` = dari ekstensi, lalu dari isi.
    pub kind: Option<FileKind>,
    /// `None` = tebak dari baris pertama.
    pub delimiter: Option<u8>,
    pub has_header: bool,
    /// `None` = deteksi otomatis.
    pub encoding: Option<TextEncoding>,
    /// Nama sheet spreadsheet; `None` = sheet pertama.
    pub sheet: Option<String>,
    pub passphrase: Option<String>,
    /// Batas baris (pratinjau); `None` = semua.
    pub max_rows: Option<usize>,
    /// Teks sel file berpemisah yang dibaca sebagai SQL NULL (mis. `NULL`
    /// atau `\N`). `None` = tidak ada: setiap sel adalah string, termasuk
    /// yang berisi `NULL`. Tidak berlaku untuk JSON, spreadsheet, dan Parquet
    /// yang punya nilai null sendiri.
    pub null_text: Option<String>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            kind: None,
            delimiter: None,
            has_header: true,
            encoding: None,
            sheet: None,
            passphrase: None,
            max_rows: None,
            null_text: None,
        }
    }
}

/// Batas ukuran isi file setelah dekompresi (juga untuk file polos). Arsip
/// kecil bisa mengembang jadi puluhan GiB ("zip bomb"); di atas batas ini
/// pembacaan dihentikan alih-alih menghabiskan memori.
pub const MAX_DECOMPRESSED_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Potongan awal pratinjau dan batas atasnya; lihat [`read_preview_prefix`].
const PREVIEW_FIRST_CHUNK: u64 = 256 * 1024;
const PREVIEW_MAX_PREFIX: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LoadedFile {
    pub kind: FileKind,
    pub data: TableData,
    /// Encoding yang dipakai (format teks saja).
    pub encoding: Option<TextEncoding>,
    pub delimiter: Option<u8>,
    pub sheets: Vec<String>,
    pub sheet: Option<String>,
    pub compression: Option<&'static str>,
    pub encrypted: bool,
    /// `max_rows` tercapai; masih ada baris yang tidak dibaca.
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReadError {
    #[error("Cannot read file: {0}")]
    Io(String),
    #[error("This file is encrypted; enter its passphrase")]
    NeedsPassphrase,
    #[error("{0}")]
    Decrypt(#[from] EncryptError),
    #[error("Unsupported file: {0}")]
    Unsupported(String),
    #[error("Cannot parse file: {0}")]
    Parse(String),
    #[error(
        "File content is larger than the {} limit; decompress or split it first",
        format_bytes(*.0)
    )]
    TooLarge(u64),
}

fn format_bytes(bytes: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    if bytes >= GIB && bytes.is_multiple_of(GIB) {
        format!("{} GiB", bytes / GIB)
    } else if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / (1024 * 1024))
    } else {
        format!("{bytes} bytes")
    }
}

fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, e)| e).unwrap_or("")
}

fn strip_extension(name: &str) -> &str {
    name.rsplit_once('.').map(|(n, _)| n).unwrap_or(name)
}

fn kind_from_extension(name: &str) -> Option<FileKind> {
    match extension_of(name) {
        "csv" | "tsv" | "tab" | "txt" | "psv" => Some(FileKind::Delimited),
        "json" => Some(FileKind::Json),
        "ndjson" | "jsonl" => Some(FileKind::Ndjson),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => Some(FileKind::Spreadsheet),
        "parquet" | "pq" => Some(FileKind::Parquet),
        _ => None,
    }
}

fn kind_from_content(bytes: &[u8]) -> FileKind {
    if bytes.starts_with(b"PAR1") {
        return FileKind::Parquet;
    }
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        return FileKind::Spreadsheet;
    }
    let (text, _) = encoding::decode(&bytes[..bytes.len().min(4096)], None);
    match text.trim_start().chars().next() {
        Some('[') => FileKind::Json,
        Some('{') => {
            // Beberapa objek berurutan per baris = NDJSON.
            let mut lines = text.lines().filter(|l| !l.trim().is_empty());
            let first_closed = lines.next().is_some_and(|l| l.trim_end().ends_with('}'));
            let second_object = lines
                .next()
                .is_some_and(|l| l.trim_start().starts_with('{'));
            if first_closed && second_object {
                FileKind::Ndjson
            } else {
                FileKind::Json
            }
        }
        _ => FileKind::Delimited,
    }
}

/// Nama tabel yang wajar dari nama file: tanpa `.enc`, kompresi, dan ekstensi
/// format; karakter selain huruf/angka menjadi `_`.
pub fn table_name_for(path: &Path) -> String {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for _ in 0..3 {
        let ext = extension_of(&name).to_ascii_lowercase();
        if FILE_EXTENSIONS.contains(&ext.as_str()) {
            name = strip_extension(&name).to_string();
        } else {
            break;
        }
    }
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "data".to_string()
    } else if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("t_{out}")
    } else {
        out
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Compression {
    Gzip,
    Zstd,
    Zip,
}

/// Lapisan kompresi dari ekstensi atau byte awal file.
fn compression_of(head: &[u8], name: &str) -> Option<Compression> {
    let ext = extension_of(name);
    if ext == "gz" || (head.starts_with(&[0x1F, 0x8B]) && kind_from_extension(name).is_none()) {
        Some(Compression::Gzip)
    } else if ext == "zst" || head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        Some(Compression::Zstd)
    } else if ext == "zip" {
        Some(Compression::Zip)
    } else {
        None
    }
}

/// Baca `reader` sampai habis, paling banyak `cap` byte. Lebih dari itu
/// berarti [`ReadError::TooLarge`]; yang dibaca tidak pernah melewati
/// `cap + 1` byte.
fn read_capped(reader: impl Read, cap: u64) -> Result<Vec<u8>, ReadError> {
    let mut out = Vec::new();
    reader
        .take(cap.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|e| ReadError::Io(e.to_string()))?;
    if out.len() as u64 > cap {
        return Err(ReadError::TooLarge(cap));
    }
    Ok(out)
}

/// Buka lapisan kompresi. Mengembalikan isi, nama di dalamnya, dan label.
/// Hasil dekompresi dibatasi `cap` byte.
fn decompress(
    bytes: Vec<u8>,
    name: &str,
    cap: u64,
) -> Result<(Vec<u8>, String, Option<&'static str>), ReadError> {
    let ext = extension_of(name);
    match compression_of(&bytes, name) {
        Some(Compression::Gzip) => {
            let out = read_capped(flate2::read::MultiGzDecoder::new(bytes.as_slice()), cap)?;
            let inner = if ext == "gz" {
                strip_extension(name)
            } else {
                name
            };
            Ok((out, inner.to_string(), Some("gzip")))
        }
        Some(Compression::Zstd) => {
            let decoder = zstd::stream::read::Decoder::new(bytes.as_slice())
                .map_err(|e| ReadError::Io(e.to_string()))?;
            let out = read_capped(decoder, cap)?;
            let inner = if ext == "zst" {
                strip_extension(name)
            } else {
                name
            };
            Ok((out, inner.to_string(), Some("zstd")))
        }
        Some(Compression::Zip) => {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes.as_slice()))
                .map_err(|e| ReadError::Parse(e.to_string()))?;
            // Entri pertama yang formatnya dikenal; kalau tidak ada, entri file pertama.
            let mut chosen: Option<usize> = None;
            for i in 0..archive.len() {
                let entry = archive
                    .by_index(i)
                    .map_err(|e| ReadError::Parse(e.to_string()))?;
                if entry.is_dir() {
                    continue;
                }
                let entry_name = entry.name().to_ascii_lowercase();
                if kind_from_extension(&entry_name).is_some() {
                    chosen = Some(i);
                    break;
                }
                chosen.get_or_insert(i);
            }
            let index =
                chosen.ok_or_else(|| ReadError::Unsupported("zip archive is empty".to_string()))?;
            let entry = archive
                .by_index(index)
                .map_err(|e| ReadError::Parse(e.to_string()))?;
            let inner = entry.name().to_ascii_lowercase();
            // Ukuran di header bisa dipalsukan, jadi pembacaan tetap dibatasi.
            if entry.size() > cap {
                return Err(ReadError::TooLarge(cap));
            }
            let out = read_capped(entry, cap)?;
            Ok((out, inner, Some("zip")))
        }
        None => Ok((bytes, name.to_string(), None)),
    }
}

/// Tebak delimiter dari baris pertama: kandidat yang paling sering muncul di
/// luar tanda kutip.
pub fn sniff_delimiter(text: &str) -> u8 {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut counts = [0usize; 4];
    let candidates = [b',', b';', b'\t', b'|'];
    let mut quoted = false;
    for b in line.bytes() {
        if b == b'"' {
            quoted = !quoted;
        } else if !quoted && let Some(i) = candidates.iter().position(|c| *c == b) {
            counts[i] += 1;
        }
    }
    let (best, n) = counts
        .iter()
        .enumerate()
        .max_by_key(|(i, n)| (**n, std::cmp::Reverse(*i)))
        .map(|(i, n)| (i, *n))
        .unwrap_or((0, 0));
    if n == 0 { b',' } else { candidates[best] }
}

fn generated_headers(width: usize) -> Vec<String> {
    (1..=width).map(|i| format!("col_{i}")).collect()
}

fn read_delimited(
    text: &str,
    delimiter: u8,
    has_header: bool,
    max_rows: Option<usize>,
    null_text: Option<&str>,
) -> Result<(TableData, bool), ReadError> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(has_header)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut headers: Vec<String> = if has_header {
        reader
            .headers()
            .map_err(|e| ReadError::Parse(e.to_string()))?
            .iter()
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut truncated = false;
    for record in reader.records() {
        if max_rows.is_some_and(|m| rows.len() >= m) {
            truncated = true;
            break;
        }
        let record = record.map_err(|e| ReadError::Parse(e.to_string()))?;
        // Sel kosong tetap string kosong; hanya teks pilihan pengguna yang
        // menjadi NULL. Kolom yang tidak ada di baris pendek diisi NULL oleh
        // `TableData::from_cells`.
        rows.push(
            record
                .iter()
                .map(|field| (null_text != Some(field)).then(|| field.to_string()))
                .collect(),
        );
    }
    if !has_header {
        headers = generated_headers(rows.iter().map(Vec::len).max().unwrap_or(0));
    }
    Ok((TableData::from_cells(headers, rows), truncated))
}

fn json_cell(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

/// Penyusun tabel dari nilai JSON satu per satu, supaya dokumen besar tidak
/// perlu berada di memori sebagai pohon `serde_json::Value` utuh. Objek:
/// kolom = gabungan semua key dalam urutan kemunculan. Array: `col_1..n`.
/// Skalar: satu kolom `value`.
#[derive(Default)]
struct JsonTable {
    headers: Vec<String>,
    index: std::collections::HashMap<String, usize>,
    rows: Vec<Vec<Option<String>>>,
    max_rows: Option<usize>,
    truncated: bool,
}

impl JsonTable {
    fn new(max_rows: Option<usize>) -> Self {
        Self {
            max_rows,
            ..Default::default()
        }
    }

    /// Tambah satu baris. `false` = batas baris tercapai, nilai tidak dipakai.
    fn push(&mut self, value: serde_json::Value) -> bool {
        if self.max_rows.is_some_and(|m| self.rows.len() >= m) {
            self.truncated = true;
            return false;
        }
        let mut row: Vec<Option<String>> = vec![None; self.headers.len()];
        let mut set = |key: String, cell: Option<String>| {
            let i = match self.index.get(&key) {
                Some(i) => *i,
                None => {
                    self.index.insert(key.clone(), self.headers.len());
                    self.headers.push(key);
                    self.headers.len() - 1
                }
            };
            if row.len() <= i {
                row.resize(i + 1, None);
            }
            row[i] = cell;
        };
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    set(k, json_cell(&v));
                }
            }
            serde_json::Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    set(format!("col_{}", i + 1), json_cell(v));
                }
            }
            other => set("value".to_string(), json_cell(&other)),
        }
        self.rows.push(row);
        true
    }

    fn finish(self) -> (TableData, bool) {
        (
            TableData::from_cells(self.headers, self.rows),
            self.truncated,
        )
    }
}

/// Visitor tingkat atas dokumen JSON: array dialirkan elemen demi elemen ke
/// [`JsonTable`]; bentuk lain dikembalikan utuh.
struct JsonTopLevel<'a> {
    table: &'a mut JsonTable,
}

impl<'de> serde::de::Visitor<'de> for JsonTopLevel<'_> {
    /// `None` = array sudah dialirkan ke tabel.
    type Value = Option<serde_json::Value>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        while let Some(value) = seq.next_element::<serde_json::Value>()? {
            if !self.table.push(value) {
                // Batas pratinjau tercapai: sisa dokumen tidak perlu diurai.
                return Err(serde::de::Error::custom("row limit reached"));
            }
        }
        Ok(None)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        serde::Deserialize::deserialize(serde::de::value::MapAccessDeserializer::new(map)).map(Some)
    }

    fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E> {
        Ok(Some(serde_json::Value::Bool(v)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E> {
        Ok(Some(serde_json::Value::from(v)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E> {
        Ok(Some(serde_json::Value::from(v)))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E> {
        Ok(Some(serde_json::Value::from(v)))
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Some(serde_json::Value::from(v)))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(Some(serde_json::Value::Null))
    }
}

fn read_json(text: &str, max_rows: Option<usize>) -> Result<(TableData, bool), ReadError> {
    use serde::Deserializer;
    let mut table = JsonTable::new(max_rows);
    let mut de = serde_json::Deserializer::from_str(text);
    let top = match de.deserialize_any(JsonTopLevel { table: &mut table }) {
        Ok(top) => {
            de.end().map_err(|e| ReadError::Parse(e.to_string()))?;
            top
        }
        // Berhenti karena batas baris, bukan karena dokumen rusak.
        Err(_) if table.truncated => None,
        Err(e) => return Err(ReadError::Parse(e.to_string())),
    };
    match top {
        None => {}
        // Bungkus umum `{"data": [...]}`: pakai satu-satunya properti array.
        Some(serde_json::Value::Object(map)) => {
            let arrays = map.values().filter(|v| v.is_array()).count();
            if arrays == 1 {
                let items = map
                    .into_iter()
                    .find_map(|(_, v)| match v {
                        serde_json::Value::Array(items) => Some(items),
                        _ => None,
                    })
                    .unwrap_or_default();
                for item in items {
                    if !table.push(item) {
                        break;
                    }
                }
            } else {
                table.push(serde_json::Value::Object(map));
            }
        }
        Some(other) => {
            table.push(other);
        }
    }
    Ok(table.finish())
}

fn read_ndjson(text: &str, max_rows: Option<usize>) -> Result<(TableData, bool), ReadError> {
    let mut table = JsonTable::new(max_rows);
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<serde_json::Value>(line)
            .map_err(|e| ReadError::Parse(format!("line {}: {e}", n + 1)))?;
        // Satu baris lebih dari batas cukup untuk menandai `truncated`.
        if !table.push(value) {
            break;
        }
    }
    Ok(table.finish())
}

/// Teks sel spreadsheet; sel kosong dibedakan pemanggil lewat `Data::Empty`.
fn spreadsheet_text(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => {
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => {
            if dt.is_duration() {
                return dt.as_f64().to_string();
            }
            let (y, mo, d, h, mi, s, _ms) = dt.to_ymd_hms_milli();
            if h == 0 && mi == 0 && s == 0 && dt.as_f64().fract() == 0.0 {
                format!("{y:04}-{mo:02}-{d:02}")
            } else {
                format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
            }
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("#{e:?}"),
    }
}

fn read_spreadsheet(
    bytes: &[u8],
    sheet: Option<&str>,
    has_header: bool,
    max_rows: Option<usize>,
) -> Result<(TableData, bool, Vec<String>, Option<String>), ReadError> {
    use calamine::Reader;
    let mut workbook = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
        .map_err(|e| ReadError::Parse(e.to_string()))?;
    let sheets = workbook.sheet_names();
    let name = match sheet {
        Some(s) if sheets.iter().any(|n| n == s) => s.to_string(),
        Some(s) => return Err(ReadError::Parse(format!("sheet \"{s}\" not found"))),
        None => sheets
            .first()
            .cloned()
            .ok_or_else(|| ReadError::Parse("workbook has no sheets".to_string()))?,
    };
    let range = workbook
        .worksheet_range(&name)
        .map_err(|e| ReadError::Parse(e.to_string()))?;
    let mut rows_iter = range.rows();
    let mut headers: Vec<String> = Vec::new();
    if has_header && let Some(first) = rows_iter.next() {
        headers = first.iter().map(spreadsheet_text).collect();
    }
    let mut rows = Vec::new();
    let mut truncated = false;
    for row in rows_iter {
        if max_rows.is_some_and(|m| rows.len() >= m) {
            truncated = true;
            break;
        }
        rows.push(
            row.iter()
                .map(|c| (!matches!(c, calamine::Data::Empty)).then(|| spreadsheet_text(c)))
                .collect::<Vec<_>>(),
        );
    }
    if !has_header {
        headers = generated_headers(rows.iter().map(Vec::len).max().unwrap_or(0));
    }
    Ok((
        TableData::from_cells(headers, rows),
        truncated,
        sheets,
        Some(name),
    ))
}

fn parquet_cell(field: &parquet::record::Field) -> Option<String> {
    use parquet::record::Field;
    Some(match field {
        Field::Null => return None,
        Field::Str(s) => s.clone(),
        Field::Bytes(b) => format!("0x{}", hex::encode(b.data())),
        Field::Date(days) => chrono::DateTime::from_timestamp(i64::from(*days) * 86_400, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| days.to_string()),
        Field::TimestampMillis(ms) => chrono::DateTime::from_timestamp_millis(*ms)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| ms.to_string()),
        Field::TimestampMicros(us) => chrono::DateTime::from_timestamp_micros(*us)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
            .unwrap_or_else(|| us.to_string()),
        Field::Group(_) | Field::ListInternal(_) | Field::MapInternal(_) => {
            field.to_json_value().to_string()
        }
        other => other.to_string(),
    })
}

fn read_parquet(bytes: Vec<u8>, max_rows: Option<usize>) -> Result<(TableData, bool), ReadError> {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let err = |e: parquet::errors::ParquetError| ReadError::Parse(e.to_string());
    let reader = SerializedFileReader::new(bytes::Bytes::from(bytes)).map_err(err)?;
    let headers: Vec<String> = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .root_schema()
        .get_fields()
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    let mut rows = Vec::new();
    let mut truncated = false;
    for row in reader.get_row_iter(None).map_err(err)? {
        if max_rows.is_some_and(|m| rows.len() >= m) {
            truncated = true;
            break;
        }
        let row = row.map_err(err)?;
        rows.push(
            row.get_column_iter()
                .map(|(_, field)| parquet_cell(field))
                .collect::<Vec<_>>(),
        );
    }
    Ok((TableData::from_cells(headers, rows), truncated))
}

/// Baca file dari byte yang sudah di memori. `file_name` dipakai untuk
/// menentukan format dan kompresi.
pub fn read_bytes(
    bytes: Vec<u8>,
    file_name: &str,
    opts: &ReadOptions,
) -> Result<LoadedFile, ReadError> {
    read_bytes_capped(bytes, file_name, opts, MAX_DECOMPRESSED_BYTES)
}

fn read_bytes_capped(
    bytes: Vec<u8>,
    file_name: &str,
    opts: &ReadOptions,
    cap: u64,
) -> Result<LoadedFile, ReadError> {
    let mut name = file_name.to_ascii_lowercase();
    let encrypted = encrypt::is_encrypted(&bytes);
    let bytes = if encrypted {
        let passphrase = opts
            .passphrase
            .as_deref()
            .filter(|p| !p.is_empty())
            .ok_or(ReadError::NeedsPassphrase)?;
        if extension_of(&name) == encrypt::ENCRYPTED_EXTENSION {
            name = strip_extension(&name).to_string();
        }
        encrypt::decrypt_bytes(passphrase, &bytes)?
    } else {
        bytes
    };
    let (bytes, name, compression) = decompress(bytes, &name, cap)?;
    let kind = opts
        .kind
        .or_else(|| kind_from_extension(&name))
        .unwrap_or_else(|| kind_from_content(&bytes));

    let mut loaded = LoadedFile {
        kind,
        data: TableData::default(),
        encoding: None,
        delimiter: None,
        sheets: Vec::new(),
        sheet: None,
        compression,
        encrypted,
        truncated: false,
    };
    match kind {
        FileKind::Delimited | FileKind::Json | FileKind::Ndjson => {
            // Buffer diambil alih: UTF-8 valid tidak disalin lagi.
            let (text, used) = encoding::decode_owned(bytes, opts.encoding);
            loaded.encoding = Some(used);
            let (data, truncated) = match kind {
                FileKind::Delimited => {
                    let delimiter = opts.delimiter.unwrap_or_else(|| {
                        if extension_of(&name) == "tsv" || extension_of(&name) == "tab" {
                            b'\t'
                        } else {
                            sniff_delimiter(&text)
                        }
                    });
                    loaded.delimiter = Some(delimiter);
                    read_delimited(
                        &text,
                        delimiter,
                        opts.has_header,
                        opts.max_rows,
                        opts.null_text.as_deref(),
                    )?
                }
                FileKind::Json => read_json(&text, opts.max_rows)?,
                _ => read_ndjson(&text, opts.max_rows)?,
            };
            loaded.data = data;
            loaded.truncated = truncated;
        }
        FileKind::Spreadsheet => {
            let (data, truncated, sheets, sheet) = read_spreadsheet(
                &bytes,
                opts.sheet.as_deref(),
                opts.has_header,
                opts.max_rows,
            )?;
            loaded.data = data;
            loaded.truncated = truncated;
            loaded.sheets = sheets;
            loaded.sheet = sheet;
        }
        FileKind::Parquet => {
            let (data, truncated) = read_parquet(bytes, opts.max_rows)?;
            loaded.data = data;
            loaded.truncated = truncated;
        }
    }
    loaded.data.normalize();
    Ok(loaded)
}

/// Panjang awalan `buf` yang berakhir di batas baris (encoding 8-bit) atau
/// di batas unit kode (UTF-16), supaya potongan tidak membelah karakter.
fn complete_prefix_len(buf: &[u8], encoding: TextEncoding) -> usize {
    match encoding {
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => buf.len() & !1,
        _ => buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1),
    }
}

/// Pratinjau murah: untuk file teks baris-per-baris yang polos (CSV/TSV,
/// NDJSON; tidak terkompresi, tidak terenkripsi) hanya awalan file yang
/// dibaca, sebanyak yang dibutuhkan untuk `max_rows` baris. Awalan mulai dari
/// [`PREVIEW_FIRST_CHUNK`] dan dilipatgandakan sampai jumlah baris tercapai
/// atau [`PREVIEW_MAX_PREFIX`]. Mengembalikan hasil dan jumlah byte yang
/// dibaca; `None` berarti jalur ini tidak berlaku dan seluruh file dibaca.
///
/// Potongan bisa memutus record terakhir (mis. newline di dalam tanda kutip),
/// jadi hasil hanya dipakai bila pembaca melihat lebih dari `max_rows` record:
/// record ke-`max_rows` pasti utuh karena ada record sesudahnya.
fn read_preview_prefix(
    path: &Path,
    name: &str,
    opts: &ReadOptions,
) -> Result<Option<(LoadedFile, u64)>, ReadError> {
    let Some(max_rows) = opts.max_rows else {
        return Ok(None);
    };
    let io = |e: std::io::Error| ReadError::Io(e.to_string());
    let lower = name.to_ascii_lowercase();
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut buf: Vec<u8> = Vec::new();
    let mut want = PREVIEW_FIRST_CHUNK;
    let mut plan: Option<ReadOptions> = None;
    loop {
        let missing = want.saturating_sub(buf.len() as u64);
        let read = file
            .by_ref()
            .take(missing)
            .read_to_end(&mut buf)
            .map_err(io)? as u64;
        let eof = read < missing;
        if plan.is_none() {
            if encrypt::is_encrypted(&buf) || compression_of(&buf, &lower).is_some() {
                return Ok(None);
            }
            let kind = opts
                .kind
                .or_else(|| kind_from_extension(&lower))
                .unwrap_or_else(|| kind_from_content(&buf));
            if !matches!(kind, FileKind::Delimited | FileKind::Ndjson) {
                return Ok(None);
            }
            // Format dan encoding dikunci dari potongan pertama supaya
            // potongan berikutnya dibaca dengan cara yang sama.
            plan = Some(ReadOptions {
                kind: Some(kind),
                encoding: Some(opts.encoding.unwrap_or_else(|| encoding::detect(&buf))),
                ..opts.clone()
            });
        }
        let Some(plan) = plan.as_ref() else {
            return Ok(None);
        };
        if eof {
            // File lebih kecil dari potongan: ini sudah seluruh isinya.
            let total = buf.len() as u64;
            return read_bytes(buf, name, plan).map(|loaded| Some((loaded, total)));
        }
        let cut = complete_prefix_len(&buf, plan.encoding.unwrap_or_default());
        if cut > 0 {
            let loaded = read_bytes(buf[..cut].to_vec(), name, plan)?;
            if loaded.truncated && loaded.data.rows.len() >= max_rows {
                return Ok(Some((loaded, buf.len() as u64)));
            }
        }
        if want >= PREVIEW_MAX_PREFIX {
            return Ok(None);
        }
        want = (want * 4).min(PREVIEW_MAX_PREFIX);
    }
}

/// Baca file dari disk. Dengan `max_rows` (pratinjau) file teks polos hanya
/// dibaca awalannya; lihat [`read_preview_prefix`]. Selain itu seluruh file
/// dimuat, dengan batas [`MAX_DECOMPRESSED_BYTES`].
pub fn read_file(path: &Path, opts: &ReadOptions) -> Result<LoadedFile, ReadError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some((loaded, _)) = read_preview_prefix(path, &name, opts)? {
        return Ok(loaded);
    }
    let io = |e: std::io::Error| ReadError::Io(e.to_string());
    let mut file = std::fs::File::open(path).map_err(io)?;
    // Ukuran diketahui dari awal: tolak sebelum membaca, dan alokasikan sekali.
    let size = file.metadata().map_err(io)?.len();
    if size > MAX_DECOMPRESSED_BYTES {
        return Err(ReadError::TooLarge(MAX_DECOMPRESSED_BYTES));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.by_ref()
        .take(MAX_DECOMPRESSED_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > MAX_DECOMPRESSED_BYTES {
        return Err(ReadError::TooLarge(MAX_DECOMPRESSED_BYTES));
    }
    read_bytes(bytes, &name, opts)
}

/// True bila file di `path` adalah ekspor terenkripsi Tabular (hanya membaca
/// header).
pub fn file_is_encrypted(path: &Path) -> bool {
    let mut head = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && encrypt::is_encrypted(&head)
}

#[cfg(test)]
mod tests {
    use super::super::formats::{self, ExportFormat, ExportOptions};
    use super::*;

    fn sample() -> TableData {
        TableData::new(
            vec!["id".into(), "name".into(), "score".into()],
            vec![
                vec!["1".into(), "Ann".into(), "1.5".into()],
                vec!["2".into(), "NULL".into(), "2".into()],
            ],
        )
    }

    fn read(bytes: Vec<u8>, name: &str) -> LoadedFile {
        read_bytes(bytes, name, &ReadOptions::default()).unwrap()
    }

    #[test]
    fn reads_csv_with_sniffed_semicolon() {
        let loaded = read(b"id;name\n1;\"a;b\"\n2;c\n".to_vec(), "x.csv");
        assert_eq!(loaded.kind, FileKind::Delimited);
        assert_eq!(loaded.delimiter, Some(b';'));
        assert_eq!(loaded.data.headers, vec!["id", "name"]);
        assert_eq!(loaded.data.rows[0], vec!["1", "a;b"]);
    }

    #[test]
    fn reads_utf16_and_windows_1252_csv() {
        let utf16 = encoding::encode("id,name\n1,Żółć\n", TextEncoding::Utf16Le, true).bytes;
        let loaded = read(utf16, "x.csv");
        assert_eq!(loaded.encoding, Some(TextEncoding::Utf16Le));
        assert_eq!(loaded.data.rows[0][1], "Żółć");

        let cp1252 = encoding::encode("id,name\n1,café\n", TextEncoding::Windows1252, false).bytes;
        let loaded = read(cp1252, "x.csv");
        assert_eq!(loaded.encoding, Some(TextEncoding::Windows1252));
        assert_eq!(loaded.data.rows[0][1], "café");
    }

    #[test]
    fn csv_without_header_gets_generated_names_and_row_limit() {
        let opts = ReadOptions {
            has_header: false,
            max_rows: Some(1),
            ..Default::default()
        };
        let loaded = read_bytes(b"1,a\n2,b\n".to_vec(), "x.csv", &opts).unwrap();
        assert_eq!(loaded.data.headers, vec!["col_1", "col_2"]);
        assert_eq!(loaded.data.rows.len(), 1);
        assert!(loaded.truncated);
    }

    #[test]
    fn reads_json_array_wrapped_object_and_ragged_keys() {
        let loaded = read(
            br#"{"data":[{"id":1,"tags":["a","b"]},{"id":2,"extra":null,"name":"x"}]}"#.to_vec(),
            "x.json",
        );
        assert_eq!(loaded.data.headers, vec!["id", "tags", "extra", "name"]);
        assert_eq!(
            loaded.data.rows[0],
            vec![
                "1",
                "[\"a\",\"b\"]",
                crate::data_transfer::NULL_MARKER,
                crate::data_transfer::NULL_MARKER
            ]
        );
        assert_eq!(
            loaded.data.rows[1],
            vec![
                "2",
                crate::data_transfer::NULL_MARKER,
                crate::data_transfer::NULL_MARKER,
                "x"
            ]
        );
    }

    #[test]
    fn detects_ndjson_from_content() {
        let loaded = read(b"{\"a\":1}\n{\"a\":2}\n".to_vec(), "dump");
        assert_eq!(loaded.kind, FileKind::Ndjson);
        assert_eq!(loaded.data.rows.len(), 2);
    }

    #[test]
    fn xlsx_roundtrip_through_calamine() {
        let bytes = formats::build_xlsx(&sample()).unwrap();
        let loaded = read(bytes, "x.xlsx");
        assert_eq!(loaded.kind, FileKind::Spreadsheet);
        assert_eq!(loaded.sheets, vec!["Data"]);
        assert_eq!(loaded.data.headers, vec!["id", "name", "score"]);
        assert_eq!(loaded.data.rows[0], vec!["1", "Ann", "1.5"]);
        assert_eq!(loaded.data.rows[1], vec!["2", "NULL", "2"]);
    }

    #[test]
    fn parquet_roundtrip_keeps_nulls_and_types() {
        let bytes = formats::build_parquet(&sample()).unwrap();
        let loaded = read(bytes, "x.parquet");
        assert_eq!(loaded.kind, FileKind::Parquet);
        assert_eq!(loaded.data.headers, vec!["id", "name", "score"]);
        assert_eq!(loaded.data.rows[0], vec!["1", "Ann", "1.5"]);
        assert_eq!(loaded.data.rows[1][1], "NULL");
    }

    #[test]
    fn reads_gzip_zstd_and_zip_wrapped_files() {
        use std::io::Write;
        let csv = b"id,name\n1,a\n";

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(csv).unwrap();
        let loaded = read(gz.finish().unwrap(), "x.csv.gz");
        assert_eq!(loaded.compression, Some("gzip"));
        assert_eq!(loaded.data.rows[0], vec!["1", "a"]);

        let zst = zstd::stream::encode_all(&csv[..], 0).unwrap();
        let loaded = read(zst, "x.csv.zst");
        assert_eq!(loaded.compression, Some("zstd"));
        assert_eq!(loaded.data.headers, vec!["id", "name"]);

        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("readme.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"# hi").unwrap();
        zip.start_file("inner.tsv", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"id\tname\n7\tz\n").unwrap();
        let loaded = read(zip.finish().unwrap().into_inner(), "bundle.zip");
        assert_eq!(loaded.compression, Some("zip"));
        assert_eq!(loaded.delimiter, Some(b'\t'));
        assert_eq!(loaded.data.rows[0], vec!["7", "z"]);
    }

    #[test]
    fn encrypted_export_needs_and_accepts_passphrase() {
        let opts = ExportOptions::default();
        let (plain, _) = formats::render(ExportFormat::Json, &sample(), &opts).unwrap();
        let sealed = encrypt::encrypt_bytes("pw", &plain).unwrap();
        assert_eq!(
            read_bytes(sealed.clone(), "x.json.enc", &ReadOptions::default()).unwrap_err(),
            ReadError::NeedsPassphrase
        );
        let wrong = ReadOptions {
            passphrase: Some("nope".into()),
            ..Default::default()
        };
        assert_eq!(
            read_bytes(sealed.clone(), "x.json.enc", &wrong).unwrap_err(),
            ReadError::Decrypt(EncryptError::WrongPassphrase)
        );
        let right = ReadOptions {
            passphrase: Some("pw".into()),
            ..Default::default()
        };
        let loaded = read_bytes(sealed, "x.json.enc", &right).unwrap();
        assert!(loaded.encrypted);
        assert_eq!(loaded.kind, FileKind::Json);
        assert_eq!(loaded.data.rows.len(), 2);
    }

    fn temp_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tabular_readers_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(tag);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn decompression_is_capped_for_gzip_zstd_and_zip() {
        use std::io::Write;
        // 1 MiB nol memampat jadi beberapa ratus byte: bom dekompresi mini.
        let bomb = vec![b'0'; 1024 * 1024];
        let cap = 64 * 1024;
        let opts = ReadOptions::default();

        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&bomb).unwrap();
        let gz = gz.finish().unwrap();
        assert!(gz.len() < cap as usize);
        assert_eq!(
            read_bytes_capped(gz.clone(), "x.csv.gz", &opts, cap).unwrap_err(),
            ReadError::TooLarge(cap)
        );

        let zst = zstd::stream::encode_all(bomb.as_slice(), 0).unwrap();
        assert_eq!(
            read_bytes_capped(zst, "x.csv.zst", &opts, cap).unwrap_err(),
            ReadError::TooLarge(cap)
        );

        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("inner.csv", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&bomb).unwrap();
        let zip = zip.finish().unwrap().into_inner();
        assert_eq!(
            read_bytes_capped(zip, "bundle.zip", &opts, cap).unwrap_err(),
            ReadError::TooLarge(cap)
        );

        // Di bawah batas tetap terbaca, dan pesan menyebut batasnya.
        assert!(read_bytes_capped(gz, "x.csv.gz", &opts, 2 * 1024 * 1024).is_ok());
        assert_eq!(
            ReadError::TooLarge(MAX_DECOMPRESSED_BYTES).to_string(),
            "File content is larger than the 4 GiB limit; decompress or split it first"
        );
        // Pembaca tidak pernah mengambil lebih dari batas + 1 byte.
        assert_eq!(
            read_capped(std::io::repeat(0), 1000).unwrap_err(),
            ReadError::TooLarge(1000)
        );
    }

    #[test]
    fn preview_of_large_csv_reads_only_a_prefix() {
        let mut csv = String::from("id,name\n");
        let mut n = 0;
        while csv.len() < 3 * 1024 * 1024 {
            n += 1;
            csv.push_str(&format!("{n},row number {n}\n"));
        }
        let mut bytes = csv.into_bytes();
        // Ekor rusak: kutip tak tertutup dan byte bukan UTF-8. Pratinjau tidak
        // boleh sampai ke sini.
        bytes.extend_from_slice(b"9,\"never closed \xFF\xFE\xFF");
        let path = temp_file("big.csv", &bytes);
        let opts = ReadOptions {
            max_rows: Some(5),
            ..Default::default()
        };

        let (loaded, read) = read_preview_prefix(&path, "big.csv", &opts)
            .unwrap()
            .expect("plain CSV uses the prefix path");
        assert_eq!(read, PREVIEW_FIRST_CHUNK);
        assert!(read < bytes.len() as u64 / 10);
        assert!(loaded.truncated);
        assert_eq!(loaded.encoding, Some(TextEncoding::Utf8));
        assert_eq!(loaded.data.headers, vec!["id", "name"]);
        assert_eq!(loaded.data.rows.len(), 5);
        assert_eq!(loaded.data.rows[4], vec!["5", "row number 5"]);
        // `read_file` memakai jalur yang sama dan hasilnya sama.
        assert_eq!(read_file(&path, &opts).unwrap().data, loaded.data);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preview_of_ndjson_ignores_an_invalid_tail() {
        let mut text = String::new();
        let mut n = 0;
        while text.len() < 1024 * 1024 {
            n += 1;
            text.push_str(&format!("{{\"id\":{n},\"v\":null}}\n"));
        }
        text.push_str("{this is not json\n");
        let path = temp_file("big.ndjson", text.as_bytes());
        let preview = ReadOptions {
            max_rows: Some(3),
            ..Default::default()
        };
        let loaded = read_file(&path, &preview).unwrap();
        assert_eq!(loaded.kind, FileKind::Ndjson);
        assert_eq!(loaded.data.rows.len(), 3);
        assert!(loaded.truncated && loaded.data.is_null(0, 1));
        // Baca penuh sampai ke ekor dan melaporkan baris yang rusak.
        let err = read_file(&path, &ReadOptions::default()).unwrap_err();
        assert!(
            matches!(err, ReadError::Parse(ref m) if m.starts_with(&format!("line {}", n + 1)))
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preview_grows_the_prefix_and_skips_wrapped_files() {
        // Record pertama lebih panjang dari potongan awal (newline di dalam
        // kutip): awalan harus diperbesar sampai record itu utuh.
        let long = "line\n".repeat(PREVIEW_FIRST_CHUNK as usize / 4);
        let mut csv = format!("id,note\n1,\"{long}\"\n");
        for n in 2..200_000 {
            csv.push_str(&format!("{n},x\n"));
        }
        let path = temp_file("wide.csv", csv.as_bytes());
        let opts = ReadOptions {
            max_rows: Some(2),
            ..Default::default()
        };
        let (loaded, read) = read_preview_prefix(&path, "wide.csv", &opts)
            .unwrap()
            .unwrap();
        assert!(read > PREVIEW_FIRST_CHUNK && read < csv.len() as u64);
        assert_eq!(loaded.data.rows.len(), 2);
        assert_eq!(loaded.data.rows[0][1], long);
        assert_eq!(loaded.data.rows[1], vec!["2", "x"]);
        let _ = std::fs::remove_file(&path);

        // File kecil: seluruh isi, tanpa tanda terpotong.
        let small = temp_file("small.csv", b"id\n1\n2\n");
        let (loaded, read) = read_preview_prefix(&small, "small.csv", &opts)
            .unwrap()
            .unwrap();
        assert_eq!(
            (read, loaded.data.rows.len(), loaded.truncated),
            (7, 2, false)
        );

        // Tanpa batas baris, JSON, dan file terkompresi lewat jalur penuh.
        assert!(
            read_preview_prefix(&small, "small.csv", &ReadOptions::default())
                .unwrap()
                .is_none()
        );
        let json = temp_file("a.json", b"[{\"a\":1}]");
        assert!(
            read_preview_prefix(&json, "a.json", &opts)
                .unwrap()
                .is_none()
        );
        let gz = temp_file("a.csv.gz", &[0x1F, 0x8B, 0, 0]);
        assert!(
            read_preview_prefix(&gz, "a.csv.gz", &opts)
                .unwrap()
                .is_none()
        );
        for p in [small, json, gz] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn csv_text_null_is_a_string_unless_opted_in() {
        use super::super::values::{InsertLimits, ValueKind, build_insert_batches};
        use crate::models::enums::DatabaseType;
        let csv = b"id,name,note\n1,NULL,a\n2,,b\n3\n".to_vec();

        let loaded = read(csv.clone(), "x.csv");
        let data = &loaded.data;
        assert!(data.has_explicit_nulls());
        assert_eq!(data.value(0, 1), Some("NULL"));
        // Sel kosong tetap string kosong; hanya kolom yang tidak ada yang NULL.
        assert_eq!(data.value(1, 1), Some(""));
        assert_eq!(data.value(2, 1), None);
        assert_eq!(data.value(2, 2), None);

        // File -> nilai -> literal SQL: string `NULL` tetap string.
        let columns: Vec<String> = data.headers.iter().map(|h| format!("\"{h}\"")).collect();
        let kinds = [ValueKind::Number, ValueKind::Text, ValueKind::Text];
        let sql = build_insert_batches(
            &DatabaseType::PostgreSQL,
            "\"t\"",
            &columns,
            data,
            &[0, 1, 2],
            &kinds,
            InsertLimits::default(),
        );
        assert_eq!(
            sql,
            vec![
                "INSERT INTO \"t\" (\"id\", \"name\", \"note\") VALUES\n\
                 (1, 'NULL', 'a'),\n(2, '', 'b'),\n(3, NULL, NULL);"
            ]
        );

        // Pilihan eksplisit: teks `NULL` dibaca sebagai SQL NULL.
        let opts = ReadOptions {
            null_text: Some("NULL".to_string()),
            ..Default::default()
        };
        let loaded = read_bytes(csv, "x.csv", &opts).unwrap();
        assert_eq!(loaded.data.value(0, 1), None);
        assert_eq!(loaded.data.value(0, 2), Some("a"));
        assert_eq!(loaded.data.value(1, 1), Some(""));
    }

    #[test]
    fn json_and_spreadsheet_nulls_stay_distinct_from_the_text_null() {
        let loaded = read(br#"[{"a":"NULL","b":null},{"a":null}]"#.to_vec(), "x.json");
        assert_eq!(loaded.data.value(0, 0), Some("NULL"));
        assert_eq!(loaded.data.value(0, 1), None);
        assert_eq!(loaded.data.value(1, 0), None);
        assert_eq!(loaded.data.value(1, 1), None);

        let xlsx = formats::build_xlsx(&TableData::from_cells(
            vec!["a".into(), "b".into()],
            vec![vec![Some("NULL".into()), None]],
        ))
        .unwrap();
        let loaded = read(xlsx, "x.xlsx");
        assert_eq!(loaded.data.value(0, 0), Some("NULL"));
        assert_eq!(loaded.data.value(0, 1), None);
    }

    #[test]
    fn json_array_is_streamed_and_stops_at_the_row_limit() {
        // Ekor rusak tidak dicapai bila batas baris sudah terpenuhi.
        let opts = ReadOptions {
            max_rows: Some(2),
            ..Default::default()
        };
        let broken = br#"[{"a":1},{"a":2},{"a":3},{"a": oops"#.to_vec();
        let loaded = read_bytes(broken.clone(), "x.json", &opts).unwrap();
        assert_eq!(loaded.data.rows, vec![vec!["1"], vec!["2"]]);
        assert!(loaded.truncated);
        assert!(matches!(
            read_bytes(broken, "x.json", &ReadOptions::default()),
            Err(ReadError::Parse(_))
        ));
        // Sampah setelah dokumen tetap ditolak.
        assert!(matches!(
            read_bytes(
                b"[1,2] trailing".to_vec(),
                "x.json",
                &ReadOptions::default()
            ),
            Err(ReadError::Parse(_))
        ));
        // Skalar dan objek tunggal tetap menjadi satu baris.
        assert_eq!(read(b"42".to_vec(), "x.json").data.rows, vec![vec!["42"]]);
        let single = read(br#"{"a":1,"b":[1],"c":[2]}"#.to_vec(), "x.json");
        assert_eq!(single.data.rows, vec![vec!["1", "[1]", "[2]"]]);
        let limited = read_bytes(br#"{"rows":[1,2,3]}"#.to_vec(), "x.json", &opts).unwrap();
        assert_eq!(limited.data.rows.len(), 2);
        assert!(limited.truncated);
    }

    #[test]
    fn table_names_from_file_names() {
        assert_eq!(
            table_name_for(Path::new("/tmp/Sales 2026.csv.gz")),
            "Sales_2026"
        );
        assert_eq!(table_name_for(Path::new("orders.parquet")), "orders");
        assert_eq!(
            table_name_for(Path::new("2026-report.xlsx.enc")),
            "t_2026_report"
        );
        assert_eq!(table_name_for(Path::new(".csv")), "data");
    }
}
