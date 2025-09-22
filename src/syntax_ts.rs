//! Unified syntax & parsing module.
//!
//! This file now contains both:
//! - The legacy heuristic highlighter (previously in `syntax.rs`).
//! - An experimental tree-sitter (SQL) incremental parse (parse-only right now).
//!
//! Rationale: Simplify codebase by removing duplicate modules while keeping an easy path
//! to expand semantic features (folding, structure-aware autocomplete, etc.). The current
//! tree-sitter integration is purposefully minimal: it parses and caches an incremental tree
//! (when the `tree_sitter_sql` feature is enabled) but still falls back to the lightweight
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

#[cfg(feature = "tree_sitter_sql")]
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
            parser.set_language(tree_sitter_sql::language())?;
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
pub fn try_tree_sitter_sql_highlight(text: &str, dark: bool) -> Option<LayoutJob> {
    #[cfg(feature = "tree_sitter_sql")]
    {
        use tree_sitter::{Node, TreeCursor};
        use eframe::egui::{TextFormat};
        use crate::syntax_ts::{keyword_color, string_color, comment_color, number_color, punctuation_color, normal_color};

        // Parse (side effect warms incremental tree); we also need the actual tree.
        let sexpr = ts::parse_sql(text)?; // returns root sexpr currently; we re-fetch parser tree via parse again
        // Re-parse to access the tree via OnceCell. (Refactor later to return tree reference.)
        // Acquire parser again to read its tree.
        // Re-run a fresh parser (non-incremental) for highlight only (keeps logic simple for now)
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(tree_sitter_sql::language()).is_err() { return None; }
        let tree = parser.parse(text, None)?;
        let root = tree.root_node();

        // Collect spans (start_byte, end_byte, type_index)
        #[derive(Clone, Copy)]
        struct Span { s: usize, e: usize, kind: SpanKind }
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum SpanKind { Keyword, String, Comment, Number, Punctuation, Ident, Other }

        let mut spans: Vec<Span> = Vec::with_capacity(256);

        fn classify(node: Node, text: &str) -> Option<SpanKind> {
            let kind = node.kind();
            match kind {
                // Strings & comments
                "string" | "quoted_text" => Some(SpanKind::String),
                "comment" => Some(SpanKind::Comment),
                // Numbers (integer / numeric literal kinds depend on grammar version)
                "number" | "numeric_literal" => Some(SpanKind::Number),
                // punctuation tokens are usually individual symbols; we skip letting them fallback to char loop
                // Keywords: tree-sitter-sql marks many tokens simply as their text (e.g. select, from, where)
                _ => {
                    let txt = &text[node.start_byte()..node.end_byte()];
                    let up = txt.to_ascii_uppercase();
                    if SQL_KEYWORDS.binary_search(&up.as_str()).is_ok() { return Some(SpanKind::Keyword); }
                    // simple identifier detection
                    if kind == "identifier" { return Some(SpanKind::Ident); }
                    None
                }
            }
        }

        // Depth-first traversal; skip large subtrees once classified (e.g., string, comment)
        let cursor: TreeCursor = root.walk();
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
        #[cfg(feature = "tree_sitter_sql")]
        {
            if let Some(ts_job) = try_tree_sitter_sql_highlight(text, dark) && !ts_job.text.is_empty() { return ts_job; }
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

fn word_color(word: &str, lang: LanguageKind, dark: bool) -> Color32 {
    let up = word.to_ascii_uppercase();
    let keyword = match lang {
        LanguageKind::Sql => SQL_KEYWORDS.binary_search(&up.as_str()).is_ok(),
        LanguageKind::Redis => REDIS_CMDS.binary_search(&up.as_str()).is_ok(),
        LanguageKind::Mongo => MONGO_CMDS.binary_search(&up.as_str()).is_ok(),
        LanguageKind::Plain => false,
    };
    if keyword { return keyword_color(dark); }
    if word.chars().all(|c| c.is_ascii_digit()) { return number_color(dark); }
    normal_color(dark)
}

fn keyword_color(dark: bool) -> Color32 { if dark { Color32::from_rgb(220, 180, 90) } else { Color32::from_rgb(160, 60, 0) } }
fn number_color(dark: bool) -> Color32 { if dark { Color32::from_rgb(120, 160, 255) } else { Color32::from_rgb(0, 90, 200) } }
fn string_color(dark: bool) -> Color32 { if dark { Color32::from_rgb(200, 120, 160) } else { Color32::from_rgb(160, 0, 120) } }
fn comment_color(dark: bool) -> Color32 { if dark { Color32::from_rgb(120, 120, 120) } else { Color32::from_rgb(100, 110, 120) } }
fn punctuation_color(dark: bool) -> Color32 { if dark { Color32::from_rgb(180, 180, 180) } else { Color32::from_rgb(80, 80, 80) } }
fn normal_color(dark: bool) -> Color32 { if dark { Color32::from_rgb(210, 210, 210) } else { Color32::from_rgb(30, 30, 30) } }

// Static keyword tables
static SQL_KEYWORDS: &[&str] = &["ALL","ALTER","AND","AS","ASC","BY","CASE","CREATE","DELETE","DESC","DISTINCT","DROP","ELSE","END","EXISTS","FROM","GROUP","HAVING","IF","IN","INDEX","INNER","INSERT","INTO","IS","JOIN","LEFT","LIMIT","NOT","NULL","ON","OR","ORDER","OUTER","RIGHT","SELECT","SET","TABLE","THEN","UNION","UPDATE","VALUES","WHEN","WHERE"];
static REDIS_CMDS: &[&str] = &["DEL","EXISTS","GET","HGETALL","INCR","LRANGE","RPUSH","SADD","SET","SMEMBERS","ZADD","ZRANGE"];
static MONGO_CMDS: &[&str] = &["AGGREGATE","COUNT","DELETE","DISTINCT","FIND","INSERT","UPDATE"];
