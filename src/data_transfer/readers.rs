//! Pembaca file data: CSV/TSV, JSON, NDJSON, spreadsheet (XLSX/XLS/ODS), dan
//! Parquet; juga yang terkompresi (`.gz`, `.zip`, `.zst`) atau terenkripsi
//! (`.enc` hasil ekspor Tabular). Hasilnya selalu [`TableData`].

use std::io::{Cursor, Read};
use std::path::Path;

use super::encoding::{self, TextEncoding};
use super::encrypt::{self, EncryptError};
use super::{NULL_MARKER, TableData};

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
        }
    }
}

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

/// Buka lapisan kompresi. Mengembalikan isi, nama di dalamnya, dan label.
fn decompress(
    bytes: Vec<u8>,
    name: &str,
) -> Result<(Vec<u8>, String, Option<&'static str>), ReadError> {
    let io = |e: std::io::Error| ReadError::Io(e.to_string());
    let ext = extension_of(name);
    if ext == "gz" || (bytes.starts_with(&[0x1F, 0x8B]) && kind_from_extension(name).is_none()) {
        let mut out = Vec::new();
        flate2::read::MultiGzDecoder::new(bytes.as_slice())
            .read_to_end(&mut out)
            .map_err(io)?;
        let inner = if ext == "gz" {
            strip_extension(name)
        } else {
            name
        };
        return Ok((out, inner.to_string(), Some("gzip")));
    }
    if ext == "zst" || bytes.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        let out = zstd::stream::decode_all(bytes.as_slice()).map_err(io)?;
        let inner = if ext == "zst" {
            strip_extension(name)
        } else {
            name
        };
        return Ok((out, inner.to_string(), Some("zstd")));
    }
    if ext == "zip" {
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
        let mut entry = archive
            .by_index(index)
            .map_err(|e| ReadError::Parse(e.to_string()))?;
        let inner = entry.name().to_ascii_lowercase();
        let mut out = Vec::new();
        entry.read_to_end(&mut out).map_err(io)?;
        return Ok((out, inner, Some("zip")));
    }
    Ok((bytes, name.to_string(), None))
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
    let mut rows = Vec::new();
    let mut truncated = false;
    for record in reader.records() {
        if max_rows.is_some_and(|m| rows.len() >= m) {
            truncated = true;
            break;
        }
        let record = record.map_err(|e| ReadError::Parse(e.to_string()))?;
        rows.push(record.iter().map(str::to_string).collect::<Vec<_>>());
    }
    if !has_header {
        headers = generated_headers(rows.iter().map(Vec::len).max().unwrap_or(0));
    }
    Ok((TableData::new(headers, rows), truncated))
}

fn json_cell(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => NULL_MARKER.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Susun tabel dari daftar nilai JSON. Objek: kolom = gabungan semua key
/// dalam urutan kemunculan. Array: `col_1..n`. Skalar: satu kolom `value`.
fn table_from_json_values<I>(values: I, max_rows: Option<usize>) -> (TableData, bool)
where
    I: IntoIterator<Item = serde_json::Value>,
{
    let mut headers: Vec<String> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut truncated = false;
    for value in values {
        if max_rows.is_some_and(|m| rows.len() >= m) {
            truncated = true;
            break;
        }
        let mut row = vec![NULL_MARKER.to_string(); headers.len()];
        let mut set = |key: String, cell: String, row: &mut Vec<String>| {
            let i = *index.entry(key.clone()).or_insert_with(|| {
                headers.push(key);
                headers.len() - 1
            });
            if row.len() <= i {
                row.resize(i + 1, NULL_MARKER.to_string());
            }
            row[i] = cell;
        };
        match value {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    set(k, json_cell(&v), &mut row);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    set(format!("col_{}", i + 1), json_cell(v), &mut row);
                }
            }
            other => set("value".to_string(), json_cell(&other), &mut row),
        }
        rows.push(row);
    }
    let width = headers.len();
    for row in &mut rows {
        row.resize(width, NULL_MARKER.to_string());
    }
    (TableData::new(headers, rows), truncated)
}

fn read_json(text: &str, max_rows: Option<usize>) -> Result<(TableData, bool), ReadError> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ReadError::Parse(e.to_string()))?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        // Bungkus umum `{"data": [...]}`: pakai satu-satunya properti array.
        serde_json::Value::Object(map) => {
            let arrays = map.values().filter(|v| v.is_array()).count();
            if arrays == 1 {
                map.into_iter()
                    .find_map(|(_, v)| match v {
                        serde_json::Value::Array(items) => Some(items),
                        _ => None,
                    })
                    .unwrap_or_default()
            } else {
                vec![serde_json::Value::Object(map)]
            }
        }
        other => vec![other],
    };
    Ok(table_from_json_values(items, max_rows))
}

fn read_ndjson(text: &str, max_rows: Option<usize>) -> Result<(TableData, bool), ReadError> {
    let mut values = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        // Satu baris lebih dari batas cukup untuk menandai `truncated`.
        if max_rows.is_some_and(|m| values.len() > m) {
            break;
        }
        values.push(
            serde_json::from_str::<serde_json::Value>(line)
                .map_err(|e| ReadError::Parse(format!("line {}: {e}", n + 1)))?,
        );
    }
    Ok(table_from_json_values(values, max_rows))
}

fn spreadsheet_cell(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Empty => NULL_MARKER.to_string(),
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
        headers = first
            .iter()
            .map(|c| match c {
                calamine::Data::Empty => String::new(),
                other => spreadsheet_cell(other),
            })
            .collect();
    }
    let mut rows = Vec::new();
    let mut truncated = false;
    for row in rows_iter {
        if max_rows.is_some_and(|m| rows.len() >= m) {
            truncated = true;
            break;
        }
        rows.push(row.iter().map(spreadsheet_cell).collect::<Vec<_>>());
    }
    if !has_header {
        headers = generated_headers(rows.iter().map(Vec::len).max().unwrap_or(0));
    }
    Ok((TableData::new(headers, rows), truncated, sheets, Some(name)))
}

fn parquet_cell(field: &parquet::record::Field) -> String {
    use parquet::record::Field;
    match field {
        Field::Null => NULL_MARKER.to_string(),
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
    }
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
    Ok((TableData::new(headers, rows), truncated))
}

/// Baca file dari byte yang sudah di memori. `file_name` dipakai untuk
/// menentukan format dan kompresi.
pub fn read_bytes(
    bytes: Vec<u8>,
    file_name: &str,
    opts: &ReadOptions,
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
    let (bytes, name, compression) = decompress(bytes, &name)?;
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
            let (text, used) = encoding::decode(&bytes, opts.encoding);
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
                    read_delimited(&text, delimiter, opts.has_header, opts.max_rows)?
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

/// Baca file dari disk.
pub fn read_file(path: &Path, opts: &ReadOptions) -> Result<LoadedFile, ReadError> {
    let bytes = std::fs::read(path).map_err(|e| ReadError::Io(e.to_string()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
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
            vec!["1", "[\"a\",\"b\"]", "NULL", "NULL"]
        );
        assert_eq!(loaded.data.rows[1], vec!["2", "NULL", "NULL", "x"]);
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
