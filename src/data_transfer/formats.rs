//! Format file ekspor: CSV/TSV, JSON, NDJSON, Markdown, HTML, XML, XLSX,
//! Parquet, dan SQL INSERT. Format teks bisa memilih encoding + BOM; semua
//! format bisa dienkripsi (AES-256-GCM) saat ditulis ke disk.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::encoding::{self, TextEncoding};
use super::values::{InferredType, InsertLimits, build_insert_batches, infer_types};
use super::{TableData, encrypt, is_null_cell};
use crate::models::enums::DatabaseType;
use crate::schema_objects::sql::{quote_ident, quote_qualified};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    Ndjson,
    Markdown,
    Html,
    Xml,
    Xlsx,
    Parquet,
    SqlInsert,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 10] = [
        ExportFormat::Csv,
        ExportFormat::Tsv,
        ExportFormat::Json,
        ExportFormat::Ndjson,
        ExportFormat::Markdown,
        ExportFormat::Html,
        ExportFormat::Xml,
        ExportFormat::Xlsx,
        ExportFormat::Parquet,
        ExportFormat::SqlInsert,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Csv => "CSV",
            ExportFormat::Tsv => "TSV",
            ExportFormat::Json => "JSON",
            ExportFormat::Ndjson => "NDJSON (JSON Lines)",
            ExportFormat::Markdown => "Markdown",
            ExportFormat::Html => "HTML",
            ExportFormat::Xml => "XML",
            ExportFormat::Xlsx => "Excel (XLSX)",
            ExportFormat::Parquet => "Parquet",
            ExportFormat::SqlInsert => "SQL INSERT",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::Ndjson => "ndjson",
            ExportFormat::Markdown => "md",
            ExportFormat::Html => "html",
            ExportFormat::Xml => "xml",
            ExportFormat::Xlsx => "xlsx",
            ExportFormat::Parquet => "parquet",
            ExportFormat::SqlInsert => "sql",
        }
    }

    /// Format teks: encoding dan BOM berlaku. XLSX dan Parquet biner.
    pub fn is_text(self) -> bool {
        !matches!(self, ExportFormat::Xlsx | ExportFormat::Parquet)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExportOptions {
    pub encoding: TextEncoding,
    pub bom: bool,
    /// Nama tabel untuk SQL INSERT dan elemen akar XML/HTML.
    pub table_name: String,
    pub db_type: Option<DatabaseType>,
    pub insert_limits: InsertLimits,
    /// `Some` = enkripsi hasil dengan passphrase ini.
    pub passphrase: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportOutcome {
    pub path: PathBuf,
    pub bytes_written: u64,
    pub rows: usize,
    /// Karakter yang tidak ada di encoding tujuan (ditulis sebagai `?`).
    pub unmappable: usize,
    pub encrypted: bool,
}

/// Judul tab seperti "Table: users" menjadi nama tabel.
pub fn table_name_from_caption(caption: &str) -> String {
    let name = caption
        .trim()
        .strip_prefix("Table:")
        .map(str::trim)
        .unwrap_or(caption.trim())
        .replace(' ', "_");
    if name.is_empty() {
        "exported_table".to_string()
    } else {
        name
    }
}

fn json_value(cell: &str) -> serde_json::Value {
    if is_null_cell(cell) {
        serde_json::Value::Null
    } else if let Ok(n) = cell.parse::<i64>() {
        serde_json::Value::from(n)
    } else if let Some(f) = cell.parse::<f64>().ok().filter(|f| f.is_finite()) {
        serde_json::Value::from(f)
    } else {
        serde_json::Value::from(cell)
    }
}

fn json_row(data: &TableData, row: usize) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (i, header) in data.headers.iter().enumerate() {
        obj.insert(header.clone(), json_value(data.cell(row, i)));
    }
    serde_json::Value::Object(obj)
}

pub fn build_json(data: &TableData) -> String {
    let rows: Vec<serde_json::Value> = (0..data.rows.len()).map(|r| json_row(data, r)).collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(rows))
        .unwrap_or_else(|_| "[]".to_string())
}

/// Satu objek JSON per baris.
pub fn build_ndjson(data: &TableData) -> String {
    let mut out = String::new();
    for r in 0..data.rows.len() {
        out.push_str(&json_row(data, r).to_string());
        out.push('\n');
    }
    out
}

pub fn build_delimited(data: &TableData, delimiter: u8) -> Result<String, String> {
    let mut writer = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());
    writer
        .write_record(&data.headers)
        .map_err(|e| e.to_string())?;
    for r in 0..data.rows.len() {
        writer
            .write_record((0..data.headers.len()).map(|c| data.cell(r, c)))
            .map_err(|e| e.to_string())?;
    }
    let bytes = writer.into_inner().map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

fn xml_escape(s: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' | '\n' | '\r' => out.push(ch),
            // Karakter kontrol lain tidak sah di XML 1.0.
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// `<table name="..."><row><field name="col">nilai</field>…</row></table>`.
/// NULL ditulis sebagai `<field name="col" null="true"/>`.
pub fn build_xml(data: &TableData, table_name: &str, encoding: TextEncoding) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"{}\"?>\n<table name=\"{}\">\n",
        encoding.iana_name(),
        xml_escape(table_name, true)
    );
    for r in 0..data.rows.len() {
        out.push_str("  <row>\n");
        for (c, header) in data.headers.iter().enumerate() {
            let cell = data.cell(r, c);
            let name = xml_escape(header, true);
            if is_null_cell(cell) {
                out.push_str(&format!("    <field name=\"{name}\" null=\"true\"/>\n"));
            } else {
                out.push_str(&format!(
                    "    <field name=\"{name}\">{}</field>\n",
                    xml_escape(cell, false)
                ));
            }
        }
        out.push_str("  </row>\n");
    }
    out.push_str("</table>\n");
    out
}

/// Dokumen HTML mandiri dengan satu `<table>`.
pub fn build_html(data: &TableData, table_name: &str, encoding: TextEncoding) -> String {
    let title = xml_escape(table_name, false);
    let mut out = format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"{}\">\n<title>{title}</title>\n\
         <style>\n\
         body {{ font-family: system-ui, sans-serif; margin: 24px; }}\n\
         table {{ border-collapse: collapse; }}\n\
         th, td {{ border: 1px solid #c8ccd4; padding: 4px 8px; text-align: left; vertical-align: top; }}\n\
         th {{ background: #f0f2f5; }}\n\
         td.null {{ color: #9aa0a6; font-style: italic; }}\n\
         </style>\n</head>\n<body>\n<table>\n<caption>{title}</caption>\n<thead>\n<tr>",
        encoding.iana_name()
    );
    for header in &data.headers {
        out.push_str(&format!("<th>{}</th>", xml_escape(header, false)));
    }
    out.push_str("</tr>\n</thead>\n<tbody>\n");
    for r in 0..data.rows.len() {
        out.push_str("<tr>");
        for c in 0..data.headers.len() {
            let cell = data.cell(r, c);
            if is_null_cell(cell) {
                out.push_str("<td class=\"null\">NULL</td>");
            } else {
                out.push_str(&format!(
                    "<td>{}</td>",
                    xml_escape(cell, false).replace('\n', "<br>")
                ));
            }
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n</body>\n</html>\n");
    out
}

/// Statement `INSERT` multi-baris; ukuran tiap statement dibatasi `limits`.
/// Kolom yang seluruh isinya angka ditulis tanpa kutip.
pub fn build_sql_inserts(
    data: &TableData,
    table_name: &str,
    db_type: Option<&DatabaseType>,
    limits: InsertLimits,
) -> String {
    let db = db_type.cloned().unwrap_or(DatabaseType::PostgreSQL);
    let columns: Vec<String> = data.headers.iter().map(|h| quote_ident(&db, h)).collect();
    let kinds: Vec<_> = infer_types(data)
        .into_iter()
        .map(InferredType::kind)
        .collect();
    let source_cols: Vec<usize> = (0..data.headers.len()).collect();
    let mut out = build_insert_batches(
        &db,
        &quote_qualified(&db, table_name),
        &columns,
        &data.rows,
        &source_cols,
        &kinds,
        limits,
    )
    .join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub fn build_xlsx(data: &TableData) -> Result<Vec<u8>, String> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Data").map_err(|e| e.to_string())?;
    let header_format = rust_xlsxwriter::Format::new().set_bold();
    for (col, header) in data.headers.iter().enumerate() {
        worksheet
            .write_string_with_format(0, col as u16, header, &header_format)
            .map_err(|e| e.to_string())?;
        worksheet
            .set_column_width(col as u16, 15.0)
            .map_err(|e| e.to_string())?;
    }
    for r in 0..data.rows.len() {
        for c in 0..data.headers.len() {
            let cell = data.cell(r, c);
            let (row, col) = ((r + 1) as u32, c as u16);
            if is_null_cell(cell) {
                continue;
            }
            match cell.parse::<f64>().ok().filter(|f| f.is_finite()) {
                Some(number) => worksheet.write_number(row, col, number),
                None => worksheet.write_string(row, col, cell),
            }
            .map_err(|e| e.to_string())?;
        }
    }
    workbook.save_to_buffer().map_err(|e| e.to_string())
}

/// File Parquet (kompresi Snappy). Tipe kolom diinferensi dari data:
/// INT64, DOUBLE, BOOLEAN, selain itu string UTF-8. Semua kolom nullable.
pub fn build_parquet(data: &TableData) -> Result<Vec<u8>, String> {
    use parquet::basic::{Compression, LogicalType, Repetition, Type as PhysicalType};
    use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, Int64Type};
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::types::Type;

    let err = |e: parquet::errors::ParquetError| e.to_string();
    let types = infer_types(data);
    let names = super::unique_headers(&data.headers);
    let mut fields = Vec::with_capacity(names.len());
    for (name, ty) in names.iter().zip(&types) {
        let (physical, logical) = match ty {
            InferredType::Integer => (PhysicalType::INT64, None),
            InferredType::Real => (PhysicalType::DOUBLE, None),
            InferredType::Boolean => (PhysicalType::BOOLEAN, None),
            _ => (PhysicalType::BYTE_ARRAY, Some(LogicalType::String)),
        };
        let field = Type::primitive_type_builder(name, physical)
            .with_repetition(Repetition::OPTIONAL)
            .with_logical_type(logical)
            .build()
            .map_err(err)?;
        fields.push(Arc::new(field));
    }
    let schema = Arc::new(
        Type::group_type_builder("schema")
            .with_fields(fields)
            .build()
            .map_err(err)?,
    );
    let props = Arc::new(
        WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build(),
    );

    let mut buf = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut buf, schema, props).map_err(err)?;
    // Row group dibatasi supaya pembaca tidak perlu memuat seluruh kolom.
    const ROW_GROUP: usize = 100_000;
    let total = data.rows.len();
    let mut start = 0;
    loop {
        let end = (start + ROW_GROUP).min(total);
        let mut group = writer.next_row_group().map_err(err)?;
        let mut col = 0usize;
        while let Some(mut column) = group.next_column().map_err(err)? {
            let cells = (start..end).map(|r| data.cell(r, col).trim());
            let present = |v: &str, typed: bool| !(is_null_cell(v) || (typed && v.is_empty()));
            match types[col] {
                InferredType::Integer => {
                    let mut values = Vec::new();
                    let mut defs = Vec::with_capacity(end - start);
                    for v in cells {
                        match v.parse::<i64>() {
                            Ok(n) if present(v, true) => {
                                values.push(n);
                                defs.push(1i16);
                            }
                            _ => defs.push(0),
                        }
                    }
                    column
                        .typed::<Int64Type>()
                        .write_batch(&values, Some(&defs), None)
                        .map_err(err)?;
                }
                InferredType::Real => {
                    let mut values = Vec::new();
                    let mut defs = Vec::with_capacity(end - start);
                    for v in cells {
                        match v.parse::<f64>() {
                            Ok(n) if present(v, true) => {
                                values.push(n);
                                defs.push(1i16);
                            }
                            _ => defs.push(0),
                        }
                    }
                    column
                        .typed::<DoubleType>()
                        .write_batch(&values, Some(&defs), None)
                        .map_err(err)?;
                }
                InferredType::Boolean => {
                    let mut values = Vec::new();
                    let mut defs = Vec::with_capacity(end - start);
                    for v in cells {
                        if present(v, true) {
                            values.push(v.eq_ignore_ascii_case("true"));
                            defs.push(1i16);
                        } else {
                            defs.push(0);
                        }
                    }
                    column
                        .typed::<BoolType>()
                        .write_batch(&values, Some(&defs), None)
                        .map_err(err)?;
                }
                _ => {
                    let mut values = Vec::new();
                    let mut defs = Vec::with_capacity(end - start);
                    // Teks tidak di-trim: spasi di tepi adalah bagian dari data.
                    for r in start..end {
                        let v = data.cell(r, col);
                        if present(v, false) {
                            values.push(ByteArray::from(v));
                            defs.push(1i16);
                        } else {
                            defs.push(0);
                        }
                    }
                    column
                        .typed::<ByteArrayType>()
                        .write_batch(&values, Some(&defs), None)
                        .map_err(err)?;
                }
            }
            column.close().map_err(err)?;
            col += 1;
        }
        group.close().map_err(err)?;
        start = end;
        if start >= total {
            break;
        }
    }
    writer.close().map_err(err)?;
    Ok(buf)
}

/// Render `data` ke byte format `format` (belum dienkripsi). Mengembalikan
/// byte dan jumlah karakter yang tidak terpetakan ke encoding tujuan.
pub fn render(
    format: ExportFormat,
    data: &TableData,
    opts: &ExportOptions,
) -> Result<(Vec<u8>, usize), String> {
    let table_name = table_name_from_caption(&opts.table_name);
    let text = match format {
        ExportFormat::Xlsx => return Ok((build_xlsx(data)?, 0)),
        ExportFormat::Parquet => return Ok((build_parquet(data)?, 0)),
        ExportFormat::Csv => build_delimited(data, b',')?,
        ExportFormat::Tsv => build_delimited(data, b'\t')?,
        ExportFormat::Json => build_json(data),
        ExportFormat::Ndjson => build_ndjson(data),
        ExportFormat::Markdown => crate::export::build_markdown(&data.rows, &data.headers),
        ExportFormat::Html => build_html(data, &table_name, opts.encoding),
        ExportFormat::Xml => build_xml(data, &table_name, opts.encoding),
        ExportFormat::SqlInsert => {
            build_sql_inserts(data, &table_name, opts.db_type.as_ref(), opts.insert_limits)
        }
    };
    let encoded = encoding::encode(&text, opts.encoding, opts.bom);
    Ok((encoded.bytes, encoded.unmappable))
}

/// Nama file akhir: tambahkan `.enc` bila dienkripsi dan belum berakhiran itu.
pub fn encrypted_path(path: &Path) -> PathBuf {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(encrypt::ENCRYPTED_EXTENSION))
    {
        return path.to_path_buf();
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(".");
    name.push(encrypt::ENCRYPTED_EXTENSION);
    PathBuf::from(name)
}

/// Tulis byte ke `path`, dienkripsi bila `passphrase` diisi. Mengembalikan
/// path yang benar-benar ditulis dan ukurannya.
pub fn write_bytes(
    path: &Path,
    bytes: Vec<u8>,
    passphrase: Option<&str>,
) -> Result<(PathBuf, u64), String> {
    let (target, payload) = match passphrase.filter(|p| !p.is_empty()) {
        Some(p) => (
            encrypted_path(path),
            encrypt::encrypt_bytes(p, &bytes).map_err(|e| e.to_string())?,
        ),
        None => (path.to_path_buf(), bytes),
    };
    std::fs::write(&target, &payload)
        .map_err(|e| format!("Failed to write {}: {e}", target.display()))?;
    Ok((target, payload.len() as u64))
}

/// Render lalu tulis ke disk.
pub fn write_file(
    path: &Path,
    format: ExportFormat,
    data: &TableData,
    opts: &ExportOptions,
) -> Result<ExportOutcome, String> {
    let (bytes, unmappable) = render(format, data, opts)?;
    let passphrase = opts.passphrase.as_deref().filter(|p| !p.is_empty());
    let (path, bytes_written) = write_bytes(path, bytes, passphrase)?;
    Ok(ExportOutcome {
        path,
        bytes_written,
        rows: data.rows.len(),
        unmappable,
        encrypted: passphrase.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TableData {
        TableData::new(
            vec!["id".into(), "name".into(), "note".into()],
            vec![
                vec!["1".into(), "Ann <A&B>".into(), "NULL".into()],
                vec!["2".into(), "Bob \"B\"".into(), "line1\nline2".into()],
            ],
        )
    }

    #[test]
    fn ndjson_is_one_object_per_line() {
        let out = build_ndjson(&sample());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["id"], 1);
        assert!(first["note"].is_null());
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["note"], "line1\nline2");
    }

    #[test]
    fn xml_escapes_and_marks_nulls() {
        let out = build_xml(&sample(), "my \"t\"", TextEncoding::Utf8);
        assert!(out.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(out.contains("<table name=\"my &quot;t&quot;\">"));
        assert!(out.contains("<field name=\"name\">Ann &lt;A&amp;B&gt;</field>"));
        assert!(out.contains("<field name=\"note\" null=\"true\"/>"));
        assert!(out.trim_end().ends_with("</table>"));
    }

    #[test]
    fn html_escapes_cells_and_declares_charset() {
        let out = build_html(&sample(), "users", TextEncoding::Windows1252);
        assert!(out.contains("<meta charset=\"windows-1252\">"));
        assert!(out.contains("<th>name</th>"));
        assert!(out.contains("<td>Ann &lt;A&amp;B&gt;</td>"));
        assert!(out.contains("<td class=\"null\">NULL</td>"));
        assert!(out.contains("line1<br>line2"));
    }

    #[test]
    fn tsv_uses_tab_delimiter() {
        let out = build_delimited(&sample(), b'\t').unwrap();
        assert!(out.starts_with("id\tname\tnote\n"));
    }

    #[test]
    fn sql_inserts_split_by_max_size() {
        let data = TableData::new(
            vec!["id".into(), "v".into()],
            (0..50)
                .map(|i| vec![i.to_string(), "x".repeat(40)])
                .collect(),
        );
        let limits = InsertLimits {
            max_rows: 0,
            max_bytes: 400,
        };
        let sql = build_sql_inserts(&data, "t", Some(&DatabaseType::MySQL), limits);
        let statements: Vec<&str> = sql.split("\n\n").collect();
        assert!(statements.len() > 5);
        assert!(statements.iter().all(|s| s.trim_end().len() <= 400));
        assert!(sql.contains("INSERT INTO `t` (`id`, `v`) VALUES"));
        // Kolom angka tanpa kutip.
        assert!(sql.contains("(0, '"));
    }

    #[test]
    fn render_applies_encoding_and_bom() {
        let opts = ExportOptions {
            encoding: TextEncoding::Utf16Le,
            bom: true,
            ..Default::default()
        };
        let (bytes, unmappable) = render(ExportFormat::Csv, &sample(), &opts).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        assert_eq!(unmappable, 0);
        let (text, _) = encoding::decode(&bytes, None);
        assert!(text.starts_with("id,name,note"));
    }

    #[test]
    fn xlsx_and_parquet_produce_valid_containers() {
        let xlsx = build_xlsx(&sample()).unwrap();
        assert_eq!(&xlsx[..2], b"PK");
        let parquet = build_parquet(&sample()).unwrap();
        assert_eq!(&parquet[..4], b"PAR1");
        assert_eq!(&parquet[parquet.len() - 4..], b"PAR1");
    }

    #[test]
    fn write_file_encrypts_and_appends_extension() {
        let dir = std::env::temp_dir().join(format!("tabular_fmt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = ExportOptions {
            passphrase: Some("pw".into()),
            ..Default::default()
        };
        let outcome =
            write_file(&dir.join("out.csv"), ExportFormat::Csv, &sample(), &opts).unwrap();
        assert!(outcome.encrypted);
        assert_eq!(outcome.path, dir.join("out.csv.enc"));
        let raw = std::fs::read(&outcome.path).unwrap();
        assert!(encrypt::is_encrypted(&raw));
        let plain = encrypt::decrypt_bytes("pw", &raw).unwrap();
        assert!(plain.starts_with(b"id,name,note"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
