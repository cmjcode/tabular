//! Prompt saran optimasi query untuk AI, plus petunjuk cepat lokal yang
//! langsung tampil tanpa menunggu model.

use super::{QueryDiagramModel, SourceKind, StatementKind, clip};

/// Petunjuk heuristik (English, tampil di UI) yang bisa dihitung tanpa AI.
pub fn quick_hints(model: &QueryDiagramModel) -> Vec<String> {
    let mut hints: Vec<String> = Vec::new();
    let upper_filter = model.filter.as_deref().unwrap_or("").to_uppercase();

    if matches!(model.kind, StatementKind::Update | StatementKind::Delete) && model.filter.is_none()
    {
        hints.push(format!(
            "No WHERE clause: this {} touches every row. Add a WHERE condition or wrap it in a transaction first.",
            model.kind.label()
        ));
    }
    let star = model
        .tables()
        .any(|t| t.all_columns && t.kind != SourceKind::Values);
    if star && model.kind != StatementKind::Delete {
        hints.push(
            "Select only the columns you need instead of `*`; it reduces I/O and lets covering indexes work."
                .to_string(),
        );
    }
    let has_aggregate = model.output.iter().any(|o| o.aggregate) || !model.group_by.is_empty();
    if model.kind == StatementKind::Select
        && model.limit.is_none()
        && !has_aggregate
        && !model.sources.is_empty()
    {
        hints.push(
            "No LIMIT: the whole result is returned. Add LIMIT while exploring data.".to_string(),
        );
    }
    if !model.joins.is_empty() {
        let cols: Vec<String> = model
            .joins
            .iter()
            .map(|j| format!("{}.{}", j.right.table, j.right.column))
            .collect();
        hints.push(format!(
            "Make sure the join columns are indexed: {}.",
            clip(&cols.join(", "), 120)
        ));
    }
    if upper_filter.contains("LIKE '%") {
        hints.push(
            "A LIKE pattern that starts with `%` cannot use a B-tree index; consider full-text search."
                .to_string(),
        );
    }
    let wrapped = [
        "LOWER(",
        "UPPER(",
        "DATE(",
        "YEAR(",
        "MONTH(",
        "CAST(",
        "COALESCE(",
        "TRIM(",
        "SUBSTR",
    ]
    .iter()
    .any(|f| upper_filter.contains(f));
    if wrapped {
        hints.push(
            "A function wraps a filtered column, so the index on that column is skipped. Compare the raw column or add an expression index."
                .to_string(),
        );
    }
    if upper_filter.contains(" OR ") {
        hints.push(
            "OR across different columns often prevents index use; UNION ALL of two indexed queries can be faster."
                .to_string(),
        );
    }
    if upper_filter.contains("NOT IN (SELECT") || upper_filter.contains("NOT IN(SELECT") {
        hints.push(
            "NOT IN with a subquery behaves badly with NULLs and is often slow; prefer NOT EXISTS."
                .to_string(),
        );
    }
    if model.distinct && !model.joins.is_empty() {
        hints.push(
            "DISTINCT over joins can hide a join that multiplies rows; check the join conditions first."
                .to_string(),
        );
    }
    if !model.order_by.is_empty() && model.limit.is_some() {
        hints.push(format!(
            "ORDER BY with LIMIT is fastest with an index on {}.",
            clip(&model.order_by.join(", "), 80)
        ));
    }
    hints
}

/// Prompt sistem untuk saran optimasi.
pub fn optimize_system_prompt(db_label: &str) -> String {
    format!(
        "You are a senior {db_label} performance engineer inside Tabular, a desktop SQL client. \
         Review one SQL statement and suggest how to make it faster and safer. \
         Never claim you executed anything. Be concise and concrete. \
         Reply in Markdown with exactly these sections:\n\
         ## Summary\nOne or two sentences about the main cost of this statement.\n\
         ## Suggestions\nA numbered list. Each item says what to add or change, why, and the expected impact. \
         Include CREATE INDEX statements when an index would help, using {db_label} syntax.\n\
         ## Optimized query\nOne ```sql block with the complete rewritten statement. \
         If the statement is already optimal, repeat it unchanged and say so above the block."
    )
}

/// Prompt user: statement, ringkasan struktur, skema, dan petunjuk lokal.
pub fn optimize_user_prompt(model: &QueryDiagramModel, schema: &str, hints: &[String]) -> String {
    let mut out = String::new();
    out.push_str("Statement:\n```sql\n");
    out.push_str(model.sql.trim());
    out.push_str("\n```\n\nStructure detected by Tabular:\n");
    out.push_str(&format!("- Kind: {}\n", model.kind.label()));
    if let Some(t) = &model.target {
        out.push_str(&format!("- Target table: {}\n", t.table));
    }
    for s in &model.sources {
        let join = s.join.as_deref().unwrap_or("FROM");
        out.push_str(&format!("- Source: {} ({join})\n", s.title()));
    }
    for j in &model.joins {
        out.push_str(&format!(
            "- Join: {}.{} = {}.{} ({})\n",
            j.left.table, j.left.column, j.right.table, j.right.column, j.join_type
        ));
    }
    if let Some(f) = &model.filter {
        out.push_str(&format!("- WHERE: {}\n", clip(f, 400)));
    }
    if !model.group_by.is_empty() {
        out.push_str(&format!("- GROUP BY: {}\n", model.group_by.join(", ")));
    }
    if !model.order_by.is_empty() {
        out.push_str(&format!("- ORDER BY: {}\n", model.order_by.join(", ")));
    }
    if let Some(l) = &model.limit {
        out.push_str(&format!("- LIMIT: {l}\n"));
    }
    if !schema.trim().is_empty() {
        out.push_str(
            "\nSchema of the referenced tables (from Tabular's cache; index list may be incomplete):\n",
        );
        out.push_str(schema.trim());
        out.push('\n');
    }
    if !hints.is_empty() {
        out.push_str(
            "\nHeuristic findings already shown to the user (confirm, refine, or dismiss them):\n",
        );
        for h in hints {
            out.push_str(&format!("- {h}\n"));
        }
    }
    out
}

/// Ambil blok ```sql terakhir dari jawaban Markdown.
pub fn extract_sql_block(markdown: &str) -> Option<String> {
    let mut found: Option<String> = None;
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        match current.as_mut() {
            None => {
                if let Some(lang) = trimmed.strip_prefix("```") {
                    let lang = lang.trim().to_lowercase();
                    if matches!(
                        lang.as_str(),
                        "sql" | "" | "postgresql" | "mysql" | "tsql" | "sqlite"
                    ) {
                        current = Some(String::new());
                    }
                }
            }
            Some(buf) => {
                if trimmed.starts_with("```") {
                    let text = buf.trim().to_string();
                    if !text.is_empty() {
                        found = Some(text);
                    }
                    current = None;
                } else {
                    buf.push_str(line);
                    buf.push('\n');
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_last_sql_block() {
        let md = "## Suggestions\n```sql\nCREATE INDEX a ON t(x);\n```\n## Optimized query\n```sql\nSELECT x FROM t\nWHERE y = 1;\n```\n";
        assert_eq!(
            extract_sql_block(md).as_deref(),
            Some("SELECT x FROM t\nWHERE y = 1;")
        );
        assert_eq!(extract_sql_block("no code"), None);
        assert_eq!(extract_sql_block("```python\nprint(1)\n```"), None);
    }

    #[test]
    fn test_system_prompt_names_database() {
        assert!(optimize_system_prompt("PostgreSQL").contains("PostgreSQL performance"));
    }

    #[cfg(feature = "query_ast")]
    #[test]
    fn test_quick_hints() {
        use crate::models::enums::DatabaseType;
        use crate::query_diagram::analyze_statement;
        let m = analyze_statement(
            "SELECT * FROM users WHERE LOWER(email) LIKE '%x'",
            &DatabaseType::MySQL,
        )
        .unwrap();
        let hints = quick_hints(&m);
        assert!(hints.iter().any(|h| h.contains("`*`")));
        assert!(hints.iter().any(|h| h.contains("LIMIT")));
        assert!(hints.iter().any(|h| h.contains("starts with `%`")));
        assert!(hints.iter().any(|h| h.contains("function wraps")));

        let d = analyze_statement("DELETE FROM logs", &DatabaseType::MySQL).unwrap();
        assert!(quick_hints(&d)[0].starts_with("No WHERE clause"));

        let p = optimize_user_prompt(&m, "users(id int, email text)", &hints);
        assert!(p.contains("Schema of the referenced tables"));
        assert!(p.contains("- Kind: SELECT"));
    }
}
