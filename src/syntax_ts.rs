//! Unified syntax highlighting module.
//!
//! This module blends a tree-sitter powered SQL highlighter with the legacy
//! heuristic highlighter. The flow in `highlight_text` is:
//! 1. Detect language (currently only simple SQL detection by file/connection context).
//! 2. Attempt a tree-sitter parse + span classification (fast-fail guarded).
//! 3. If tree-sitter fails or yields empty output, fall back to legacy rules.
//!
//! Design notes:
//! - We pin tree-sitter crates to 0.19 to stay compatible with `tree-sitter-sql 0.0.2`.
//! - A single parser instance is stored in a `OnceCell<Mutex<...>>` to avoid unsafe statics.
//! - Highlight output is an `egui::text::LayoutJob` built in one pass without allocations per span type.
//! - Future improvements could reuse the last incremental `Tree` for very large buffers; current
//!   workloads are small enough that a fresh parse is acceptable.
//! - Legacy fallback stays for (a) non-SQL content, (b) parse errors, or (c) feature expansion later.
//!
//! Safety / Concurrency:
//! - Parser access is serialized via `Mutex` because tree-sitter's Rust parser object is not `Sync`.
//! - Short critical sections keep UI responsive.
//!
//! Performance:
//! - Current parse cost for typical query sizes (< 5 KB) is negligible; micro-optimizations deferred.
//! - Fallback path avoids regex; it performs linear scans for comments / strings and keyword checks.
//!
//! See also: `editor.rs` for integration, where `highlight_text` is invoked when building the editor layout.

//! Unified syntax & parsing module.
//!
//! This file now contains both:
//! - The legacy heuristic highlighter (previously in `syntax.rs`).
//! - An experimental tree-sitter (SQL) incremental parse (parse-only right now).
//!
//! Rationale: Simplify codebase by removing duplicate modules while keeping an easy path
//! to expand semantic features (folding, structure-aware autocomplete, etc.). The current
//! tree-sitter integration is purposefully minimal: it parses and caches an incremental tree
//! (when the `tree_sitter_sequel` feature is enabled) but still falls back to the lightweight
//! heuristic colorizer for rendering.
//!
//! Future directions (planned):
//! - Replace heuristic tokens with capture-based semantic categories.
//! - Provide range queries for folding and block selection.
//! - Offer context-aware autocomplete (e.g. columns after SELECT .. FROM table).
//! - Multi-language grammars (Redis / Mongo custom languages or disable parse).

#![allow(dead_code)]

use eframe::egui::text::LayoutJob; // For public highlight API (ported from legacy syntax.rs)

/// Language classification (formerly in `syntax.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LanguageKind {
    Sql,
    Redis,
    Mongo,
    Plain,
}

/// Basic filename / path heuristic to detect language. (Formerly `syntax.rs`)
pub fn detect_language_from_name(name: &str) -> LanguageKind {
    let lower = name.to_lowercase();
    if lower.ends_with(".sql") { return LanguageKind::Sql; }
    if lower.contains("redis") { return LanguageKind::Redis; }
    if lower.contains("mongo") { return LanguageKind::Mongo; }
    LanguageKind::Plain
}

#[cfg(feature = "tree_sitter_sequel")]
mod ts {
    use tree_sitter::{Parser, Tree};
    use once_cell::sync::OnceCell;
    use std::sync::Mutex;
    pub struct TsSqlParser {
        parser: Parser,
        tree: Option<Tree>,
        last_hash: u64,
    }
    impl TsSqlParser {
        pub fn new() -> anyhow::Result<Self> {
            let mut parser = Parser::new();
            // tree-sitter-sequel exposes a LanguageFn; convert to tree_sitter::Language
            let language = tree_sitter_sequel::LANGUAGE;
            parser.set_language(&language.into())?;
            Ok(Self { parser, tree: None, last_hash: 0 })
        }
        fn hash(text: &str) -> u64 {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h); h.finish()
        }
        pub fn ensure_parsed(&mut self, text: &str) {
            let h = Self::hash(text);
            if h == self.last_hash { return; }
            self.tree = self.parser.parse(text, self.tree.as_ref());
            self.last_hash = h;
        }
    pub fn root_sexpr(&self) -> Option<String> { self.tree.as_ref().map(|t| t.root_node().to_sexp()) }
    }

    static PARSER: OnceCell<Mutex<TsSqlParser>> = OnceCell::new();

    pub fn parse_sql(text: &str) -> Option<String> {
    let cell = PARSER.get_or_init(|| {
            // If creation fails, store a parser with empty tree (we return None below)
            let parser = TsSqlParser::new().ok();
            Mutex::new(parser.unwrap_or_else(|| TsSqlParser { parser: Parser::new(), tree: None, last_hash: 0 }))
        });
        if let Ok(mut guard) = cell.lock() {
            // If parser.language not set (because creation failed earlier), attempt again.
            if guard.tree.is_none() && guard.last_hash == 0 {
                // best-effort re-init
                if let Ok(fresh) = TsSqlParser::new() {
                    *guard = fresh;
                }
            }
            guard.ensure_parsed(text);
            return guard.root_sexpr();
        }
        None
    }
}

/// Attempt tree-sitter based highlight. If it fails or yields nothing, return None
/// so the legacy heuristic highlighter can run as fallback.
#[allow(unused_variables)]
pub fn try_tree_sitter_sequel_highlight(text: &str, dark: bool) -> Option<LayoutJob> {
    #[cfg(feature = "tree_sitter_sequel")]
    {
        use tree_sitter::Node;
        use eframe::egui::TextFormat;
        use crate::syntax_ts::{keyword_color, string_color, comment_color, number_color, punctuation_color, normal_color};

        // Fresh parser (keeps logic simple and avoids global mutable state in UI thread)
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_sequel::LANGUAGE;
    if parser.set_language(&language.into()).is_err() { return None; }
        let tree = parser.parse(text, None)?;
        let root = tree.root_node();

        // Collect spans (start_byte, end_byte, kind)
        #[derive(Clone, Copy)]
        struct Span { s: usize, e: usize, kind: SpanKind }
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum SpanKind { Keyword, String, Comment, Number, Punctuation, Ident, Other }

        let mut spans: Vec<Span> = Vec::with_capacity(256);

        fn classify(node: Node, text: &str) -> Option<SpanKind> {
            let kind = node.kind();

            // Adapted for tree-sitter-sequel: keywords are named as `keyword_*`,
            // literals use `literal`, and comments are `comment`.
            if kind == "comment" || kind == "line_comment" || kind == "block_comment" {
                return Some(SpanKind::Comment);
            }

            // keyword_* nodes (e.g., keyword_select, keyword_from, keyword_insert)
            if kind.starts_with("keyword_") {
                return Some(SpanKind::Keyword);
            }

            // literal may be a string (quoted) or number (unquoted digits)
            if kind == "literal" || kind == "string" || kind == "quoted_text" || kind == "string_literal" {
                let s = node.start_byte();
                let e = node.end_byte().min(text.len());
                if s < e {
                    let token_text = &text[s..e];
                    let trimmed = token_text.trim();
                    if trimmed.starts_with('\'') || trimmed.starts_with('"') {
                        return Some(SpanKind::String);
                    }
                    // Simple numeric detection (int/float)
                    let numeric = trimmed.parse::<f64>().is_ok();
                    if numeric { return Some(SpanKind::Number); }
                }
                // If unsure, treat as Other and let children/punctuation handle details
                return Some(SpanKind::Other);
            }

            // Identifiers
            if matches!(kind, "identifier" | "column_name" | "table_name" | "schema_name" | "field") {
                return Some(SpanKind::Ident);
            }

            // Composite constructs: let traversal recurse into children
            if matches!(kind,
                "program" | "statement" | "select" | "insert" | "update" | "delete" |
                "from" | "where" | "group_by" | "order_by" | "order_target" | "direction" |
                "relation" | "object_reference" | "list" | "select_expression"
            ) || kind.ends_with("_statement") || kind.ends_with("_clause") {
                return None;
            }

            if kind == "ERROR" {
                return None; // dive into children to salvage tokens
            }

            // Punctuation and anonymous tokens
            if !node.is_named() {
                let s = node.start_byte();
                let e = node.end_byte().min(text.len());
                if s < e {
                    let token_text = &text[s..e];
                    // keyword recovery for anonymous uppercase tokens (rare here)
                    if is_sql_keyword(token_text) { return Some(SpanKind::Keyword); }
                    if token_text.len() == 1 {
                        let ch = token_text.chars().next().unwrap();
                        if ch.is_ascii_punctuation() || ch == '`' { return Some(SpanKind::Punctuation); }
                    }
                }
                return None;
            }

            // Default: don't classify composite/unknown; recurse
            None
        }

        // Check if token is a SQL keyword (case-insensitive)
        fn is_sql_keyword(token: &str) -> bool {
            let lower = token.to_ascii_lowercase();
            matches!(lower.as_str(),
                // Core SQL
                "select" | "from" | "where" | "insert" | "into" | "update" | "delete" |
                "create" | "alter" | "drop" | "table" | "values" | "join" | "left" | "right" |
                "inner" | "outer" | "on" | "group" | "by" | "having" | "order" | "limit" |
                "offset" | "union" | "distinct" | "asc" | "desc" | "and" | "or" | "not" |
                "null" | "is" | "set" | "as" | "in" | "exists" | "case" | "when" | "then" |
                "else" | "end" | "between" | "like" | "all" | "any" | "some" |
                // MySQL specific
                "show" | "start" | "stop" | "reset" | "change" | "purge" | "replica" | 
                "master" | "slave" | "binary" | "logs" | "status" | "privileges" | "grants" |
                "processlist" | "global" | "before" | "to" |
                // Table definition
                "primary" | "key" | "foreign" | "references" | "constraint" | "unique" |
                "index" | "using" | "btree" | "hash" | "default" | "auto_increment" |
                "current_timestamp" | "engine" | "innodb" | "myisam" | "charset" | "collate" |
                "character" | "row_format" | "dynamic" | "compressed" | "redundant" | "compact" |
                // Types
                "int" | "varchar" | "text" | "bigint" | "datetime" | "timestamp" | 
                "boolean" | "decimal" | "float" | "double" | "char" | "longtext" | "mediumtext" |
                "tinytext" | "blob" | "longblob" | "mediumblob" | "tinyblob" | "enum" | 
                "unsigned" | "signed" | "zerofill"
            )
        }

        // Debug: print parse tree for first 500 chars (helps diagnose grammar issues)
        if text.len() < 500 {
            eprintln!("=== Parse tree for query ===");
            fn print_tree(node: Node, text: &str, indent: usize) {
                let kind = node.kind();
                let token_text = &text[node.start_byte()..node.end_byte().min(text.len())];
                let display = if token_text.len() > 20 { 
                    format!("{}...", &token_text[..20]) 
                } else { 
                    token_text.to_string() 
                };
                eprintln!("{:indent$}{} [{}] named={}", "", kind, display.replace('\n', "\\n"), node.is_named(), indent=indent*2);
                for i in 0..node.child_count() {
                    if let Some(ch) = node.child(i) { print_tree(ch, text, indent + 1); }
                }
            }
            print_tree(root, text, 0);
            eprintln!("=== End parse tree ===\n");
        }

        // Depth-first traversal; skip large subtrees once classified (e.g., string, comment)
        let mut stack: Vec<Node> = vec![root];
        while let Some(node) = stack.pop() {
            if node.child_count() == 0 { // leaf
                if let Some(k) = classify(node, text) {
                    spans.push(Span { s: node.start_byte(), e: node.end_byte(), kind: k });
                }
            } else if let Some(k) = classify(node, text) {
                // treat whole composite node (like comment) as one span
                spans.push(Span { s: node.start_byte(), e: node.end_byte(), kind: k });
                continue; // don't descend
            } else {
                // push children
                for i in (0..node.child_count()).rev() { if let Some(ch) = node.child(i) { stack.push(ch); } }
            }
        }

        if spans.is_empty() { return None; }
        spans.sort_by_key(|s| s.s);

        let mut job = LayoutJob::default();
        let mut idx = 0; // current byte index
        for span in spans {
            if span.s > idx { // intermediate plain text
                let slice = &text[idx..span.s];
                job.append(slice, 0.0, TextFormat { color: normal_color(dark), ..Default::default() });
            }
            let slice = &text[span.s..span.e];
            let color = match span.kind {
                SpanKind::Keyword => keyword_color(dark),
                SpanKind::String => string_color(dark),
                SpanKind::Comment => comment_color(dark),
                SpanKind::Number => number_color(dark),
                SpanKind::Punctuation => punctuation_color(dark),
                SpanKind::Ident | SpanKind::Other => normal_color(dark),
            };
            job.append(slice, 0.0, TextFormat { color, ..Default::default() });
            idx = span.e;
        }
        if idx < text.len() { job.append(&text[idx..], 0.0, TextFormat { color: normal_color(dark), ..Default::default() }); }
        Some(job)
    }
    #[cfg(not(feature = "tree_sitter_sequel"))]
    {
        let _ = (text, dark);
        None
    }
}

// ---------------- Legacy heuristic highlighter (ported from syntax.rs) ----------------
use eframe::egui::{Color32, TextFormat};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Cached highlighting with hash-based lookup
pub fn highlight_text_cached(
    text: &str,
    lang: LanguageKind,
    dark: bool,
    cache: &mut std::collections::HashMap<u64, LayoutJob>,
) -> LayoutJob {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    lang.hash(&mut hasher);
    dark.hash(&mut hasher);
    let hash = hasher.finish();
    if let Some(cached_job) = cache.get(&hash) { return cached_job.clone(); }
    let job = highlight_text(text, lang, dark);
    if cache.len() > 100 { cache.clear(); }
    cache.insert(hash, job.clone());
    job
}

/// Whole text highlighter (optionally tries tree-sitter for SQL; currently only for side effects)
pub fn highlight_text(text: &str, lang: LanguageKind, dark: bool) -> LayoutJob {
    if matches!(lang, LanguageKind::Sql) {
        #[cfg(feature = "tree_sitter_sequel")]
        {
            if let Some(ts_job) = try_tree_sitter_sequel_highlight(text, dark) && !ts_job.text.is_empty() { return ts_job; }
        }
    }
    let mut job = LayoutJob::default();
    for (i, line) in text.lines().enumerate() {
        if i > 0 { job.append("\n", 0.0, TextFormat::default()); }
        highlight_single_line(line, lang, dark, &mut job);
    }
    job
}

/// Single-line highlighter (excludes trailing newline)
pub fn highlight_line(line: &str, lang: LanguageKind, dark: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    highlight_single_line(line, lang, dark, &mut job);
    job
}

fn highlight_single_line(line: &str, lang: LanguageKind, dark: bool, job: &mut LayoutJob) {
    if matches!(lang, LanguageKind::Sql) && line.trim_start().starts_with("--") {
        job.append(line, 0.0, TextFormat { color: comment_color(dark), ..Default::default() });
        return;
    }
    let mut chars = line.char_indices().peekable();
    while let Some((start_idx, ch)) = chars.next() {
        if ch == '\'' {
            let mut end_idx = start_idx + 1; let mut found_end = false;
            for (idx, c) in chars.by_ref() { end_idx = idx + c.len_utf8(); if c == '\'' { found_end = true; break; } }
            if !found_end { end_idx = line.len(); }
            job.append(&line[start_idx..end_idx], 0.0, TextFormat { color: string_color(dark), ..Default::default() });
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let mut end_idx = start_idx; let mut word_chars = vec![ch];
            while let Some(&(_, next_ch)) = chars.peek() { if next_ch.is_ascii_alphanumeric() || next_ch == '_' { word_chars.push(next_ch); let (idx, c) = chars.next().unwrap(); end_idx = idx + c.len_utf8(); } else { break; } }
            if end_idx == start_idx { end_idx = start_idx + ch.len_utf8(); }
            let word = &line[start_idx..end_idx];
            let color = word_color(word, lang, dark);
            job.append(word, 0.0, TextFormat { color, ..Default::default() });
        } else if ch.is_whitespace() {
            job.append(&ch.to_string(), 0.0, TextFormat { color: normal_color(dark), ..Default::default() });
        } else {
            job.append(&ch.to_string(), 0.0, TextFormat { color: punctuation_color(dark), ..Default::default() });
        }
    }
}

fn word_color(word: &str, _lang: LanguageKind, dark: bool) -> Color32 {
    // Legacy fallback without static dictionaries: keep it conservative to avoid "ngaco".
    // Only numbers get special color; everything else uses normal text color.
    if word.chars().all(|c| c.is_ascii_digit()) {
        return number_color(dark);
    }
    normal_color(dark)
}

fn keyword_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(220, 180, 90) // #DCA85A
    } else {
        Color32::from_rgb(160, 60, 0) // #A03C00
    }
}

fn number_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(120, 160, 255) // #78A0FF
    } else {
        Color32::from_rgb(0, 90, 200) // #005AC8
    }
}

fn string_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(200, 120, 160) // #ff0000ff
    } else {
        Color32::from_rgb(160, 0, 120) // #A00078
    }
}

fn comment_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(120, 120, 120) // #787878
    } else {
        Color32::from_rgb(100, 110, 120) // #646E78
    }
}

fn punctuation_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(180, 180, 180) // #B4B4B4
    } else {
        Color32::from_rgb(80, 80, 80) // #505050
    }
}

fn normal_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(210, 210, 210) // #D2D2D2
    } else {
        Color32::from_rgb(30, 30, 30) // #1E1E1E
    }
}

// Static keyword tables removed: now using tree-sitter classification and
// lightweight heuristics (uppercase words) for the legacy fallback.
