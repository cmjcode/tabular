use log::debug;
use std::path::Path;

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
    let workbook = xlsxwriter::Workbook::new(path.to_str().unwrap())?;
    let mut worksheet = workbook.add_worksheet(Some("Data"))?;

    // Create header format (bold)
    let mut header_format = xlsxwriter::Format::new();
    header_format.set_bold();

    // Write headers
    for (col, header) in current_table_headers.iter().enumerate() {
        worksheet.write_string(0, col as u16, header, Some(&header_format))?;
    }

    // Write data rows
    for (row_idx, row) in all_table_data.iter().enumerate() {
        for (col_idx, cell) in row.iter().enumerate() {
            // Try to parse as number first, otherwise write as string
            if let Ok(number) = cell.parse::<f64>() {
                worksheet.write_number((row_idx + 1) as u32, col_idx as u16, number, None)?;
            } else {
                worksheet.write_string((row_idx + 1) as u32, col_idx as u16, cell, None)?;
            }
        }
    }

    // Auto-fit columns
    for col in 0..current_table_headers.len() {
        worksheet.set_column(col as u16, col as u16, 15.0, None)?;
    }

    workbook.close()?;
    Ok(())
}
