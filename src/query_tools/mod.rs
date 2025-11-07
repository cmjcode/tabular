use std::ops::Range;
use sqlformat::{FormatOptions, Indent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct LintMessage {
    pub severity: LintSeverity,
    pub message: String,
    pub span: Option<Range<usize>>,
    pub hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnippetContext {
    Any,
    SelectList,
    FromClause,
    WhereClause,
}

impl SnippetContext {
    #[inline]
    fn matches(self, other: SnippetContext) -> bool {
        matches!(self, SnippetContext::Any) || self == other || other == SnippetContext::Any
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SnippetDefinition {
    pub label: &'static str,
    pub template: &'static str,
    pub note: &'static str,
    pub context: SnippetContext,
}

const SNIPPETS: &[SnippetDefinition] = &[
    SnippetDefinition {
        label: "SELECT skeleton",
        template: "SELECT column1\nFROM table_name\nWHERE condition;",
        note: "Basic SELECT template",
        context: SnippetContext::Any,
    },
    SnippetDefinition {
        label: "SELECT COUNT(*)",
        template: "SELECT COUNT(*) AS total\nFROM table_name;",
        note: "Count rows in a table",
        context: SnippetContext::SelectList,
    },
    SnippetDefinition {
        label: "JOIN template",
        template: "LEFT JOIN other_table ON condition",
        note: "Skeleton for a JOIN clause",
        context: SnippetContext::FromClause,
    },
    SnippetDefinition {
        label: "INSERT row",
        template: "INSERT INTO table_name (column1, column2)\nVALUES (value1, value2);",
        note: "Insert a single row",
        context: SnippetContext::Any,
    },
    SnippetDefinition {
        label: "UPDATE with WHERE",
        template: "UPDATE table_name\nSET column1 = value1\nWHERE condition;",
        note: "Update rows guarded by a WHERE clause",
        context: SnippetContext::WhereClause,
    },
];

pub fn snippet_candidates(prefix: &str, ctx: SnippetContext) -> Vec<SnippetDefinition> {
    let lowered = prefix.trim().to_ascii_lowercase();
    SNIPPETS
        .iter()
        .copied()
        .filter(|snippet| snippet.context.matches(ctx))
        .filter(|snippet| {
            if lowered.is_empty() {
                return true;
            }
            snippet.label.to_ascii_lowercase().starts_with(&lowered)
                || snippet.template.to_ascii_lowercase().starts_with(&lowered)
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
pub struct ParameterSuggestion {
    pub label: &'static str,
    pub template: &'static str,
    pub note: &'static str,
}

const PARAMETERS: &[ParameterSuggestion] = &[
    ParameterSuggestion {
        label: ":id",
        template: ":id",
        note: "Named identifier parameter",
    },
    ParameterSuggestion {
        label: ":start_date",
        template: ":start_date",
        note: "Start date parameter",
    },
    ParameterSuggestion {
        label: ":end_date",
        template: ":end_date",
        note: "End date parameter",
    },
    ParameterSuggestion {
        label: "@user_id",
        template: "@user_id",
        note: "SQL Server style parameter",
    },
    ParameterSuggestion {
        label: "$1",
        template: "$1",
        note: "Positional parameter",
    },
    ParameterSuggestion {
        label: "$2",
        template: "$2",
        note: "Positional parameter",
    },
];

pub fn parameter_candidates(prefix: &str) -> Vec<ParameterSuggestion> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let first = prefix.chars().next().unwrap_or_default();
    if !matches!(first, ':' | '@' | '$') {
        return Vec::new();
    }
    let lowered = prefix.to_ascii_lowercase();
    PARAMETERS
        .iter()
        .copied()
        .filter(|param| param.label.to_ascii_lowercase().starts_with(&lowered))
        .collect()
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    hay_lower.find(&needle_lower)
}

pub fn lint_sql(sql: &str) -> Vec<LintMessage> {
    let mut messages = Vec::new();
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return messages;
    }

    if let Some(idx) = find_case_insensitive(trimmed, "SELECT *") {
        messages.push(LintMessage {
            severity: LintSeverity::Warning,
            message: "Avoid SELECT * to minimize payload and leverage indexes.".to_string(),
            span: Some(idx..idx + "SELECT *".len()),
            hint: Some("Enumerate the columns you actually need.".to_string()),
        });
    }

    let upper = trimmed.to_ascii_uppercase();
    if upper.starts_with("DELETE") && !upper.contains("WHERE") {
        messages.push(LintMessage {
            severity: LintSeverity::Warning,
            message: "DELETE without a WHERE clause will remove every row.".to_string(),
            span: None,
            hint: Some("Add a WHERE clause or run inside a transaction.".to_string()),
        });
    }
    if upper.starts_with("UPDATE") && !upper.contains("WHERE") {
        messages.push(LintMessage {
            severity: LintSeverity::Warning,
            message: "UPDATE without a WHERE clause will touch every row.".to_string(),
            span: None,
            hint: Some("Add a WHERE clause to scope the update.".to_string()),
        });
    }
    if upper.contains("DROP TABLE")
        && !upper.contains("IF EXISTS")
        && let Some(idx) = find_case_insensitive(trimmed, "DROP TABLE")
    {
        messages.push(LintMessage {
            severity: LintSeverity::Info,
            message: "DROP TABLE without IF EXISTS may fail if the table is missing.".to_string(),
            span: Some(idx..idx + "DROP TABLE".len()),
            hint: Some("Consider DROP TABLE IF EXISTS ...".to_string()),
        });
    }

    messages
}

pub fn format_sql(sql: &str) -> Option<String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut tokens = trimmed.split_whitespace().peekable();
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    while let Some(tok) = tokens.next() {
        let mut token = tok.to_string();
        let mut upper = token.to_ascii_uppercase();
        if matches!(
            upper.as_str(),
            "LEFT" | "RIGHT" | "INNER" | "OUTER" | "FULL" | "CROSS"
        ) && let Some(next) = tokens.peek()
            && next.eq_ignore_ascii_case("JOIN")
        {
            let join = tokens.next().unwrap();
            token = format!("{} {}", tok, join);
            upper = "JOIN".to_string();
        }

        let break_before = matches!(
            upper.as_str(),
            "SELECT" | "FROM" | "WHERE" | "GROUP" | "ORDER" | "HAVING" | "LIMIT" | "JOIN"
        );

        if break_before && !current.is_empty() {
            lines.push(current.trim_end().to_string());
            current.clear();
        }

        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&token);
    }

    if !current.trim().is_empty() {
        lines.push(current.trim_end().to_string());
    }

    let formatted = lines.join("\n").trim().to_string();
    if formatted == trimmed {
        None
    } else {
        Some(formatted)
    }
}

// Centralized sqlformat options used across the app
pub fn default_sqlformat_options() -> FormatOptions<'static> {
    FormatOptions {
        joins_as_top_level: true,
        indent: Indent::Spaces(6),
        uppercase: Some(true),
        lines_between_queries: 2,
        inline: false,
        max_inline_block: 50,       // characters allowed to keep a parenthesized block inline
        max_inline_arguments: Some(40),
        max_inline_top_level: Some(40),
        ..Default::default()
    }
}
