use log::debug;
use std::path::Path;

use crate::models::enums::DatabaseType;

pub fn export_to_csv(
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
    current_table_name: &str,
) {
    // Use rfd to open save dialog
    let file_dialog = rfd::FileDialog::new()
        .add_filter("CSV files", &["csv"])
        .set_file_name(format!("{}.csv", current_table_name.replace(' ', "_")));

    if let Some(path) = file_dialog.save_file() {
        match write_csv_file(&path, all_table_data, current_table_headers) {
            Ok(_) => debug!(
                "✓ Successfully exported {} rows to CSV: {:?}",
                all_table_data.len(),
                path
            ),
            Err(e) => debug!("❌ Failed to export CSV: {}", e),
        }
    } else {
        debug!("CSV file dialog was cancelled");
    }
}

fn write_csv_file(
    path: &Path,
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = csv::Writer::from_path(path)?;

    // Write headers
    writer.write_record(current_table_headers)?;

    // Write data rows
    for row in all_table_data.iter() {
        writer.write_record(row)?;
    }

    writer.flush()?;

    Ok(())
}

pub fn export_to_xlsx(
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
    current_table_name: &str,
) {
    // Use rfd to open save dialog
    let file_dialog = rfd::FileDialog::new()
        .add_filter("Excel files", &["xlsx"])
        .set_file_name(format!("{}.xlsx", current_table_name.replace(' ', "_")));

    if let Some(path) = file_dialog.save_file() {
        match write_xlsx_file(&path, all_table_data, current_table_headers) {
            Ok(_) => debug!(
                "✓ Successfully exported {} rows to XLSX: {:?}",
                all_table_data.len(),
                path
            ),
            Err(e) => debug!("❌ Failed to export XLSX: {}", e),
        }
    }
}

fn write_xlsx_file(
    path: &Path,
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let worksheet = workbook.add_worksheet();
    worksheet.set_name("Data")?;

    // Create header format (bold)
    let header_format = rust_xlsxwriter::Format::new().set_bold();

    // Write headers
    for (col, header) in current_table_headers.iter().enumerate() {
        worksheet.write_string_with_format(0, col as u16, header, &header_format)?;
    }

    // Write data rows
    for (row_idx, row) in all_table_data.iter().enumerate() {
        for (col_idx, cell) in row.iter().enumerate() {
            // Try to parse as number first, otherwise write as string
            if let Ok(number) = cell.parse::<f64>() {
                worksheet.write_number((row_idx + 1) as u32, col_idx as u16, number)?;
            } else {
                worksheet.write_string((row_idx + 1) as u32, col_idx as u16, cell)?;
            }
        }
    }

    // Set a consistent column width
    for col in 0..current_table_headers.len() {
        worksheet.set_column_width(col as u16, 15.0)?;
    }

    workbook.save(path)?;
    Ok(())
}

pub fn export_to_json(
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
    current_table_name: &str,
) {
    let file_dialog = rfd::FileDialog::new()
        .add_filter("JSON files", &["json"])
        .set_file_name(format!("{}.json", current_table_name.replace(' ', "_")));

    if let Some(path) = file_dialog.save_file() {
        match std::fs::write(
            &path,
            build_json(all_table_data, current_table_headers),
        ) {
            Ok(_) => debug!(
                "✓ Successfully exported {} rows to JSON: {:?}",
                all_table_data.len(),
                path
            ),
            Err(e) => debug!("❌ Failed to export JSON: {}", e),
        }
    }
}

fn build_json(all_table_data: &[Vec<String>], headers: &[String]) -> String {
    let rows: Vec<serde_json::Value> = all_table_data
        .iter()
        .map(|row| {
            let mut obj = serde_json::Map::new();
            for (i, header) in headers.iter().enumerate() {
                let cell = row.get(i).map(String::as_str).unwrap_or("");
                // Grid semantics: the literal NULL marker means SQL NULL.
                let value = if cell.eq_ignore_ascii_case("null") {
                    serde_json::Value::Null
                } else if let Ok(n) = cell.parse::<i64>() {
                    serde_json::Value::from(n)
                } else if let Ok(f) = cell.parse::<f64>() {
                    serde_json::Value::from(f)
                } else {
                    serde_json::Value::from(cell)
                };
                obj.insert(header.clone(), value);
            }
            serde_json::Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::Value::Array(rows))
        .unwrap_or_else(|_| "[]".to_string())
}

pub fn export_to_markdown(
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
    current_table_name: &str,
) {
    let file_dialog = rfd::FileDialog::new()
        .add_filter("Markdown files", &["md"])
        .set_file_name(format!("{}.md", current_table_name.replace(' ', "_")));

    if let Some(path) = file_dialog.save_file() {
        match std::fs::write(&path, build_markdown(all_table_data, current_table_headers)) {
            Ok(_) => debug!(
                "✓ Successfully exported {} rows to Markdown: {:?}",
                all_table_data.len(),
                path
            ),
            Err(e) => debug!("❌ Failed to export Markdown: {}", e),
        }
    }
}

pub fn build_markdown(all_table_data: &[Vec<String>], headers: &[String]) -> String {
    let escape = |s: &str| s.replace('|', "\\|").replace('\n', "<br>");
    let mut out = String::new();
    out.push_str(&format!(
        "| {} |\n",
        headers.iter().map(|h| escape(h)).collect::<Vec<_>>().join(" | ")
    ));
    out.push_str(&format!("|{}\n", " --- |".repeat(headers.len())));
    for row in all_table_data {
        let cells: Vec<String> = (0..headers.len())
            .map(|i| escape(row.get(i).map(String::as_str).unwrap_or("")))
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

pub fn export_to_sql_inserts(
    all_table_data: &[Vec<String>],
    current_table_headers: &[String],
    current_table_name: &str,
    db_type: Option<&DatabaseType>,
) {
    let file_dialog = rfd::FileDialog::new()
        .add_filter("SQL files", &["sql"])
        .set_file_name(format!("{}.sql", current_table_name.replace(' ', "_")));

    if let Some(path) = file_dialog.save_file() {
        let sql = build_sql_inserts(
            all_table_data,
            current_table_headers,
            current_table_name,
            db_type,
        );
        match std::fs::write(&path, sql) {
            Ok(_) => debug!(
                "✓ Successfully exported {} rows as SQL INSERTs: {:?}",
                all_table_data.len(),
                path
            ),
            Err(e) => debug!("❌ Failed to export SQL: {}", e),
        }
    }
}

pub fn build_sql_inserts(
    all_table_data: &[Vec<String>],
    headers: &[String],
    table_caption: &str,
    db_type: Option<&DatabaseType>,
) -> String {
    // Captions look like "Table: users" or a free-form query title.
    let table_name = table_caption
        .trim()
        .strip_prefix("Table:")
        .map(str::trim)
        .unwrap_or(table_caption.trim())
        .replace(' ', "_");
    let table_name = if table_name.is_empty() {
        "exported_table".to_string()
    } else {
        table_name
    };

    let quote_ident = |s: &str| -> String {
        match db_type {
            Some(DatabaseType::MySQL) => format!("`{}`", s.replace('`', "``")),
            Some(DatabaseType::MsSQL) => format!("[{}]", s.trim_matches(['[', ']'])),
            _ => format!("\"{}\"", s.replace('"', "\"\"")),
        }
    };
    let quote_value = |v: &str| -> String {
        if v.is_empty() || v.eq_ignore_ascii_case("null") {
            return "NULL".to_string();
        }
        match db_type {
            // MySQL treats backslash as an escape character by default.
            Some(DatabaseType::MySQL) => {
                format!("'{}'", v.replace('\\', "\\\\").replace('\'', "''"))
            }
            _ => format!("'{}'", v.replace('\'', "''")),
        }
    };

    let column_list = headers
        .iter()
        .map(|h| quote_ident(h))
        .collect::<Vec<_>>()
        .join(", ");

    let mut out = String::new();
    // Multi-row VALUES in chunks keeps the file loadable and fast to run.
    for chunk in all_table_data.chunks(100) {
        out.push_str(&format!(
            "INSERT INTO {} ({}) VALUES\n",
            quote_ident(&table_name),
            column_list
        ));
        let rows: Vec<String> = chunk
            .iter()
            .map(|row| {
                let values: Vec<String> = (0..headers.len())
                    .map(|i| quote_value(row.get(i).map(String::as_str).unwrap_or("")))
                    .collect();
                format!("  ({})", values.join(", "))
            })
            .collect();
        out.push_str(&rows.join(",\n"));
        out.push_str(";\n\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_inserts_escape_and_chunk() {
        let data = vec![
            vec!["1".to_string(), "it's".to_string()],
            vec!["2".to_string(), "NULL".to_string()],
        ];
        let headers = vec!["id".to_string(), "name".to_string()];
        let sql = build_sql_inserts(&data, &headers, "Table: users", Some(&DatabaseType::MySQL));
        assert!(sql.starts_with("INSERT INTO `users` (`id`, `name`) VALUES"));
        assert!(sql.contains("('1', 'it''s')"));
        assert!(sql.contains("('2', NULL)"));
    }

    #[test]
    fn markdown_escapes_pipes() {
        let data = vec![vec!["a|b".to_string()]];
        let headers = vec!["col".to_string()];
        let md = build_markdown(&data, &headers);
        assert!(md.contains("a\\|b"));
    }

    #[test]
    fn json_nulls_and_numbers() {
        let data = vec![vec!["NULL".to_string(), "42".to_string(), "x".to_string()]];
        let headers = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let json = build_json(&data, &headers);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed[0]["a"].is_null());
        assert_eq!(parsed[0]["b"], 42);
        assert_eq!(parsed[0]["c"], "x");
    }
}
