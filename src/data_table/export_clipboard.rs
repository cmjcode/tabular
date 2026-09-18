//! Utilities for exporting query result sets to clipboard in various formats:
//! Markdown Table, JSON, CSV, and SQL INSERT statements.

pub fn format_as_markdown_table(headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return String::new();
    }

    let mut output = String::new();

    // Headers
    output.push('|');
    for h in headers {
        output.push(' ');
        output.push_str(&h.replace('|', "\\|").replace('\n', " "));
        output.push_str(" |");
    }
    output.push('\n');

    // Separator
    output.push('|');
    for _ in headers {
        output.push_str(" --- |");
    }
    output.push('\n');

    // Rows
    for row in rows {
        output.push('|');
        for i in 0..headers.len() {
            output.push(' ');
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            output.push_str(&cell.replace('|', "\\|").replace('\n', " "));
            output.push_str(" |");
        }
        output.push('\n');
    }

    output
}

pub fn format_as_json(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut objects = Vec::new();

    for row in rows {
        let mut map = serde_json::Map::new();
        for (i, h) in headers.iter().enumerate() {
            let val = row.get(i).map(|s| s.as_str()).unwrap_or("");
            map.insert(h.clone(), serde_json::Value::String(val.to_string()));
        }
        objects.push(serde_json::Value::Object(map));
    }

    serde_json::to_string_pretty(&serde_json::Value::Array(objects))
        .unwrap_or_else(|_| "[]".to_string())
}

pub fn format_as_csv(headers: &[String], rows: &[Vec<String>]) -> String {
    fn escape_csv(val: &str) -> String {
        if val.contains(',') || val.contains('"') || val.contains('\n') || val.contains('\r') {
            format!("\"{}\"", val.replace('"', "\"\""))
        } else {
            val.to_string()
        }
    }

    let mut output = String::new();

    // Headers
    let escaped_headers: Vec<String> = headers.iter().map(|h| escape_csv(h)).collect();
    output.push_str(&escaped_headers.join(","));
    output.push('\n');

    // Rows
    for row in rows {
        let mut row_cells = Vec::with_capacity(headers.len());
        for i in 0..headers.len() {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            row_cells.push(escape_csv(cell));
        }
        output.push_str(&row_cells.join(","));
        output.push('\n');
    }

    output
}

pub fn format_as_sql_inserts(table_name: &str, headers: &[String], rows: &[Vec<String>]) -> String {
    if headers.is_empty() || rows.is_empty() {
        return String::new();
    }

    let cols = headers.join(", ");
    let mut output = String::new();

    for row in rows {
        let mut vals = Vec::with_capacity(headers.len());
        for i in 0..headers.len() {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            if cell.eq_ignore_ascii_case("NULL") {
                vals.push("NULL".to_string());
            } else {
                vals.push(format!("'{}'", cell.replace('\'', "''")));
            }
        }
        output.push_str(&format!(
            "INSERT INTO {} ({}) VALUES ({});\n",
            table_name,
            cols,
            vals.join(", ")
        ));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_as_markdown_table() {
        let headers = vec!["id".into(), "name".into()];
        let rows = vec![
            vec!["1".into(), "Alice".into()],
            vec!["2".into(), "Bob".into()],
        ];
        let md = format_as_markdown_table(&headers, &rows);
        assert!(md.contains("| id | name |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| 1 | Alice |"));
        assert!(md.contains("| 2 | Bob |"));
    }

    #[test]
    fn test_format_as_csv() {
        let headers = vec!["id".into(), "notes".into()];
        let rows = vec![
            vec!["1".into(), "foo, bar".into()],
            vec!["2".into(), "quoted \"val\"".into()],
        ];
        let csv = format_as_csv(&headers, &rows);
        assert!(csv.contains("1,\"foo, bar\""));
        assert!(csv.contains("2,\"quoted \"\"val\"\"\""));
    }

    #[test]
    fn test_format_as_json() {
        let headers = vec!["id".into(), "name".into()];
        let rows = vec![vec!["1".into(), "Alice".into()]];
        let json_str = format_as_json(&headers, &rows);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed[0]["name"], "Alice");
    }

    #[test]
    fn test_format_as_sql_inserts() {
        let headers = vec!["id".into(), "name".into()];
        let rows = vec![
            vec!["1".into(), "O'Connor".into()],
            vec!["2".into(), "NULL".into()],
        ];
        let sql = format_as_sql_inserts("users", &headers, &rows);
        assert!(sql.contains("INSERT INTO users (id, name) VALUES ('1', 'O''Connor');"));
        assert!(sql.contains("INSERT INTO users (id, name) VALUES ('2', NULL);"));
    }
}
