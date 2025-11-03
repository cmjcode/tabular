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
use eframe::egui::{Color32, TextFormat};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

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
    if lower.ends_with(".sql") {
        return LanguageKind::Sql;
    }
    if lower.contains("redis") {
        return LanguageKind::Redis;
    }
    if lower.contains("mongo") {
        return LanguageKind::Mongo;
    }
    LanguageKind::Plain
}

/// Token classes emitted by the semantic snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SemanticTokenKind {
    Keyword,
    String,
    Comment,
    Number,
    Operator,
    Identifier,
    Literal,
    Punctuation,
    Other,
}

/// Semantic token with byte range.
#[derive(Clone, Debug)]
pub struct SemanticToken {
    pub range: Range<usize>,
    pub kind: SemanticTokenKind,
}

/// Folding range categories (roughly matching LSP kinds).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FoldingRangeKind {
    Block,
    Comment,
    Region,
}

/// Folding range covering [start_line, end_line].
#[derive(Clone, Debug)]
pub struct FoldingRange {
    pub start_line: usize,
    pub end_line: usize,
    pub kind: FoldingRangeKind,
    pub byte_range: Range<usize>,
}

/// Outline kinds surfaced to side panels / navigation widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    // SQL constructs
    SqlSelect,
    SqlInsert,
    SqlUpdate,
    SqlDelete,
    SqlCreateTable,
    SqlCreateView,
    SqlCreateFunction,
    SqlCreateProcedure,
    SqlCreateIndex,
    SqlWith,
    // JSON constructs
    JsonObject,
    JsonArray,
    JsonProperty,
    // JavaScript constructs
    JsFunction,
    JsClass,
    JsMethod,
    JsVariable,
    Unknown,
}

/// Outline node representing a semantic element in the document.
#[derive(Clone, Debug)]
pub struct SymbolNode {
    pub name: String,
    pub kind: SymbolKind,
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
    pub children: Vec<SymbolNode>,
}

/// Lightweight tree-sitter diagnostic.
#[derive(Clone, Debug)]
pub struct ParseDiagnostic {
    pub message: String,
    pub range: Range<usize>,
}

/// Captures the reusable semantic products of a parse pass.
#[derive(Clone, Debug)]
pub struct SemanticSnapshot {
    pub language: LanguageKind,
    pub source_hash: u64,
    pub tokens: Vec<SemanticToken>,
    pub folding_ranges: Vec<FoldingRange>,
    pub outline: Vec<SymbolNode>,
    pub diagnostics: Vec<ParseDiagnostic>,
    pub root_sexpr: Option<String>,
}

/// Backward-compatibility alias. The legacy name is still consumed by the UI state.
pub type SqlSemanticSnapshot = SemanticSnapshot;

#[cfg(feature = "tree_sitter_sequel")]
mod ts {
    use super::*;
    use anyhow::{Context, anyhow};
    use once_cell::sync::OnceCell;
    use std::sync::Mutex;
    use tree_sitter::{InputEdit, Node, Parser, Point, Tree};

    #[derive(Clone, Copy, Debug)]
    enum GrammarKind {
        Sql,
        Json,
        Javascript,
    }

    impl GrammarKind {
        fn for_language(language: LanguageKind) -> Option<Self> {
            match language {
                LanguageKind::Sql => Some(GrammarKind::Sql),
                LanguageKind::Redis => Some(GrammarKind::Json),
                LanguageKind::Mongo => Some(GrammarKind::Javascript),
                LanguageKind::Plain => None,
            }
        }
    }

    struct ParserService {
        parser: Parser,
        tree: Option<Tree>,
        last_text: String,
        last_hash: u64,
        snapshot: Option<Arc<SemanticSnapshot>>,
        language: LanguageKind,
        grammar: GrammarKind,
    }

    impl ParserService {
        fn new(language: LanguageKind) -> anyhow::Result<Self> {
            let grammar = GrammarKind::for_language(language)
                .ok_or_else(|| anyhow!("language {:?} has no associated grammar", language))?;
            let mut parser = Parser::new();
            let ts_language = match grammar {
                GrammarKind::Sql => {
                    let language = tree_sitter_sequel::LANGUAGE;
                    language.into()
                }
                GrammarKind::Json => {
                    let language = tree_sitter_json::LANGUAGE;
                    language.into()
                }
                GrammarKind::Javascript => {
                    let language = tree_sitter_javascript::LANGUAGE;
                    language.into()
                }
            };
            parser
                .set_language(&ts_language)
                .context("failed to configure tree-sitter language")?;
            Ok(Self {
                parser,
                tree: None,
                last_text: String::new(),
                last_hash: 0,
                snapshot: None,
                language,
                grammar,
            })
        }

        fn ensure_snapshot(&mut self, text: &str) -> Option<Arc<SemanticSnapshot>> {
            let hash = hash_text(text);
            if let Some(snapshot) = self.snapshot.as_ref() {
                if snapshot.source_hash == hash {
                    return self.snapshot.clone();
                }
            }

            let tree = self.reparse(text)?;
            let snapshot = Arc::new(build_snapshot(
                self.language,
                self.grammar,
                &tree,
                text,
                hash,
            ));
            self.snapshot = Some(snapshot.clone());
            self.last_text.clear();
            self.last_text.push_str(text);
            self.last_hash = hash;
            Some(snapshot)
        }

        fn reparse(&mut self, text: &str) -> Option<Tree> {
            let mut previous_tree = self.tree.take();
            if let (Some(old_tree), true) = (&mut previous_tree, !self.last_text.is_empty()) {
                if let Some(edit) = compute_edit(&self.last_text, text) {
                    let input_edit = to_input_edit(&self.last_text, text, &edit);
                    old_tree.edit(&input_edit);
                }
            }

            let parsed = self
                .parser
                .parse(text, previous_tree.as_ref())
                .or_else(|| self.parser.parse(text, None))?;
            self.tree = Some(parsed.clone());
            Some(parsed)
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct EditSpan {
        start: usize,
        old_end: usize,
        new_end: usize,
    }

    fn hash_text(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }

    fn compute_edit(old: &str, new: &str) -> Option<EditSpan> {
        if old == new {
            return None;
        }
        let prefix = common_prefix_len(old, new);
        let suffix = common_suffix_len(old, new, prefix);
        let old_end = old.len() - suffix;
        let new_end = new.len() - suffix;
        Some(EditSpan {
            start: prefix,
            old_end,
            new_end,
        })
    }

    fn common_prefix_len(a: &str, b: &str) -> usize {
        a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
    }

    fn common_suffix_len(a: &str, b: &str, prefix_len: usize) -> usize {
        let mut i = a.len();
        let mut j = b.len();
        let mut count = 0;
        while i > prefix_len && j > prefix_len {
            if a.as_bytes()[i - 1] != b.as_bytes()[j - 1] {
                break;
            }
            i -= 1;
            j -= 1;
            count += 1;
        }
        count
    }

    fn to_input_edit(old: &str, new: &str, span: &EditSpan) -> InputEdit {
        InputEdit {
            start_byte: span.start,
            old_end_byte: span.old_end,
            new_end_byte: span.new_end,
            start_position: byte_to_point(old, span.start),
            old_end_position: byte_to_point(old, span.old_end),
            new_end_position: byte_to_point(new, span.new_end),
        }
    }

    fn byte_to_point(text: &str, byte_index: usize) -> Point {
        let mut row = 0usize;
        let mut col = 0usize;
        for (idx, ch) in text.bytes().enumerate() {
            if idx == byte_index {
                break;
            }
            if ch == b'\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        Point { row, column: col }
    }

    fn build_snapshot(
        language: LanguageKind,
        grammar: GrammarKind,
        tree: &Tree,
        text: &str,
        hash: u64,
    ) -> SemanticSnapshot {
        match grammar {
            GrammarKind::Sql => sql::build_snapshot(language, tree, text, hash),
            GrammarKind::Json => json_lang::build_snapshot(language, tree, text, hash),
            GrammarKind::Javascript => javascript::build_snapshot(language, tree, text, hash),
        }
    }

    mod sql {
        use super::*;

        pub fn build_snapshot(
            language: LanguageKind,
            tree: &Tree,
            text: &str,
            hash: u64,
        ) -> SemanticSnapshot {
            debug_assert!(matches!(language, LanguageKind::Sql));
            let root = tree.root_node();
            let (tokens, diagnostics) = collect_tokens_and_diagnostics(root, text);
            let folding_ranges = collect_folding_ranges(root, text);
            let outline = collect_outline(root, text);
            SemanticSnapshot {
                language,
                source_hash: hash,
                tokens,
                folding_ranges,
                outline,
                diagnostics,
                root_sexpr: Some(root.to_sexp()),
            }
        }

        fn collect_tokens_and_diagnostics(
            root: Node,
            text: &str,
        ) -> (Vec<SemanticToken>, Vec<ParseDiagnostic>) {
            let mut tokens = Vec::with_capacity(256);
            let mut diagnostics = Vec::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if node.kind() == "ERROR" {
                    let range = node.start_byte()..node.end_byte().min(text.len());
                    diagnostics.push(ParseDiagnostic {
                        message: "Parse error".to_string(),
                        range: range.clone(),
                    });
                }

                if node.child_count() == 0 {
                    if let Some(kind) = classify_token(node, text) {
                        push_token(&mut tokens, node, kind, text);
                    }
                    continue;
                }

                if let Some(kind) = classify_token(node, text) {
                    push_token(&mut tokens, node, kind, text);
                    continue;
                }

                for i in (0..node.child_count()).rev() {
                    if let Some(child) = node.child(i) {
                        stack.push(child);
                    }
                }
            }
            tokens.sort_by_key(|t| t.range.start);
            tokens.dedup_by(|a, b| a.range == b.range && a.kind == b.kind);
            (tokens, diagnostics)
        }

        fn push_token(
            storage: &mut Vec<SemanticToken>,
            node: Node,
            kind: SemanticTokenKind,
            text: &str,
        ) {
            let start = node.start_byte();
            let end = node.end_byte().min(text.len());
            if start >= end {
                return;
            }
            storage.push(SemanticToken {
                range: start..end,
                kind,
            });
        }

        fn classify_token(node: Node, text: &str) -> Option<SemanticTokenKind> {
            let kind = node.kind();
            if matches!(kind, "comment" | "line_comment" | "block_comment") {
                return Some(SemanticTokenKind::Comment);
            }
            if kind.starts_with("keyword_") {
                return Some(SemanticTokenKind::Keyword);
            }
            if matches!(
                kind,
                "literal" | "string" | "quoted_text" | "string_literal"
            ) {
                return literal_kind(node, text);
            }
            if matches!(
                kind,
                "identifier" | "column_name" | "table_name" | "schema_name" | "field"
            ) {
                return Some(SemanticTokenKind::Identifier);
            }
            if kind == "ERROR" {
                return None;
            }
            if !node.is_named() {
                let start = node.start_byte();
                let end = node.end_byte().min(text.len());
                if start >= end {
                    return None;
                }
                let token_text = &text[start..end];
                if is_sql_keyword(token_text) {
                    return Some(SemanticTokenKind::Keyword);
                }
                if token_text.len() == 1 {
                    let ch = token_text.as_bytes()[0];
                    if ch.is_ascii_punctuation() || ch == b'`' {
                        return Some(SemanticTokenKind::Punctuation);
                    }
                }
                return None;
            }
            if kind.ends_with("_operator") {
                return Some(SemanticTokenKind::Operator);
            }
            None
        }

        fn literal_kind(node: Node, text: &str) -> Option<SemanticTokenKind> {
            let start = node.start_byte();
            let end = node.end_byte().min(text.len());
            if start >= end {
                return None;
            }
            let token_text = &text[start..end];
            let trimmed = token_text.trim();
            if trimmed.starts_with('\'') || trimmed.starts_with('"') {
                return Some(SemanticTokenKind::String);
            }
            if trimmed.parse::<f64>().is_ok() {
                return Some(SemanticTokenKind::Number);
            }
            Some(SemanticTokenKind::Literal)
        }

        fn collect_folding_ranges(root: Node, text: &str) -> Vec<FoldingRange> {
            let mut ranges = Vec::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                let start = node.start_position();
                let end = node.end_position();
                if end.row > start.row {
                    let kind =
                        if matches!(node.kind(), "comment" | "block_comment" | "line_comment") {
                            FoldingRangeKind::Comment
                        } else {
                            FoldingRangeKind::Block
                        };
                    ranges.push(FoldingRange {
                        start_line: start.row,
                        end_line: end.row,
                        kind,
                        byte_range: node.start_byte()..node.end_byte().min(text.len()),
                    });
                }
                for i in (0..node.child_count()).rev() {
                    if let Some(child) = node.child(i) {
                        stack.push(child);
                    }
                }
            }
            ranges.sort_by_key(|r| (r.start_line, r.end_line));
            ranges.dedup_by(|a, b| a.start_line == b.start_line && a.end_line == b.end_line);
            ranges
        }

        fn collect_outline(root: Node, text: &str) -> Vec<SymbolNode> {
            let mut items = Vec::new();
            for i in 0..root.named_child_count() {
                if let Some(child) = root.named_child(i) {
                    append_symbol(child, text, &mut items);
                }
            }
            items
        }

        fn append_symbol(node: Node, text: &str, out: &mut Vec<SymbolNode>) {
            if let Some(symbol) = build_symbol(node, text) {
                out.push(symbol);
                return;
            }
            for i in 0..node.named_child_count() {
                if let Some(child) = node.named_child(i) {
                    append_symbol(child, text, out);
                }
            }
        }

        fn build_symbol(node: Node, text: &str) -> Option<SymbolNode> {
            let kind = symbol_kind_for(node)?;
            let name = format_symbol_label(kind, node, text);
            let mut children = Vec::new();
            if kind == SymbolKind::SqlSelect {
                collect_cte_symbols(node, text, &mut children);
            }
            Some(SymbolNode {
                name,
                kind,
                range: node.start_byte()..node.end_byte().min(text.len()),
                selection_range: symbol_selection_range(node, text),
                children,
            })
        }

        fn symbol_kind_for(node: Node) -> Option<SymbolKind> {
            let kind = node.kind();
            match kind {
                "select" | "select_statement" => Some(SymbolKind::SqlSelect),
                "insert" | "insert_statement" => Some(SymbolKind::SqlInsert),
                "update" | "update_statement" => Some(SymbolKind::SqlUpdate),
                "delete" | "delete_statement" => Some(SymbolKind::SqlDelete),
                "create_table_statement" => Some(SymbolKind::SqlCreateTable),
                "create_view_statement" => Some(SymbolKind::SqlCreateView),
                "create_function_statement" => Some(SymbolKind::SqlCreateFunction),
                "create_procedure_statement" => Some(SymbolKind::SqlCreateProcedure),
                "create_index_statement" => Some(SymbolKind::SqlCreateIndex),
                "common_table_expression" => Some(SymbolKind::SqlWith),
                _ => None,
            }
        }

        fn format_symbol_label(kind: SymbolKind, node: Node, text: &str) -> String {
            let base = match kind {
                SymbolKind::SqlSelect => "SELECT",
                SymbolKind::SqlInsert => "INSERT",
                SymbolKind::SqlUpdate => "UPDATE",
                SymbolKind::SqlDelete => "DELETE",
                SymbolKind::SqlCreateTable => "CREATE TABLE",
                SymbolKind::SqlCreateView => "CREATE VIEW",
                SymbolKind::SqlCreateFunction => "CREATE FUNCTION",
                SymbolKind::SqlCreateProcedure => "CREATE PROCEDURE",
                SymbolKind::SqlCreateIndex => "CREATE INDEX",
                SymbolKind::SqlWith => "WITH",
                SymbolKind::JsonObject => "OBJECT",
                SymbolKind::JsonArray => "ARRAY",
                SymbolKind::JsonProperty => "PROPERTY",
                SymbolKind::JsFunction => "FN",
                SymbolKind::JsClass => "CLASS",
                SymbolKind::JsMethod => "METHOD",
                SymbolKind::JsVariable => "VAR",
                SymbolKind::Unknown => "SQL",
            };

            if let Some(target) = find_primary_identifier(node, text) {
                format!("{base} {target}")
            } else {
                base.to_string()
            }
        }

        fn find_primary_identifier(node: Node, text: &str) -> Option<String> {
            let mut stack = vec![node];
            while let Some(current) = stack.pop() {
                if matches!(
                    current.kind(),
                    "table_name" | "object_reference" | "identifier" | "column_name"
                ) {
                    let slice = current.utf8_text(text.as_bytes()).ok()?.trim().to_string();
                    if !slice.is_empty() {
                        return Some(slice);
                    }
                }
                for i in (0..current.named_child_count()).rev() {
                    if let Some(child) = current.named_child(i) {
                        stack.push(child);
                    }
                }
            }
            None
        }

        fn symbol_selection_range(node: Node, text: &str) -> Range<usize> {
            node.start_byte()..node.end_byte().min(text.len())
        }

        fn collect_cte_symbols(node: Node, text: &str, out: &mut Vec<SymbolNode>) {
            let mut stack = vec![node];
            while let Some(current) = stack.pop() {
                if current.kind() == "common_table_expression" {
                    let name = find_primary_identifier(current, text)
                        .unwrap_or_else(|| "(cte)".to_string());
                    out.push(SymbolNode {
                        name: format!("WITH {name}"),
                        kind: SymbolKind::SqlWith,
                        range: current.start_byte()..current.end_byte().min(text.len()),
                        selection_range: symbol_selection_range(current, text),
                        children: Vec::new(),
                    });
                    continue;
                }
                for i in (0..current.named_child_count()).rev() {
                    if let Some(child) = current.named_child(i) {
                        stack.push(child);
                    }
                }
            }
        }
    }

    mod json_lang {
        use super::*;

        pub fn build_snapshot(
            language: LanguageKind,
            tree: &Tree,
            text: &str,
            hash: u64,
        ) -> SemanticSnapshot {
            let root = tree.root_node();
            let (tokens, diagnostics) = collect_tokens_and_diagnostics(root, text);
            let folding_ranges = collect_folding_ranges(root, text);
            let outline = collect_outline(root, text);
            SemanticSnapshot {
                language,
                source_hash: hash,
                tokens,
                folding_ranges,
                outline,
                diagnostics,
                root_sexpr: Some(root.to_sexp()),
            }
        }

        fn collect_tokens_and_diagnostics(
            root: Node,
            text: &str,
        ) -> (Vec<SemanticToken>, Vec<ParseDiagnostic>) {
            let mut tokens = Vec::new();
            let mut diagnostics = Vec::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if node.kind() == "ERROR" {
                    diagnostics.push(ParseDiagnostic {
                        message: "Parse error".to_string(),
                        range: node.start_byte()..node.end_byte().min(text.len()),
                    });
                }

                if node.child_count() == 0 {
                    if let Some(kind) = classify_leaf(node, text) {
                        push_token(&mut tokens, node, kind, text);
                    }
                    continue;
                }

                for i in (0..node.child_count()).rev() {
                    if let Some(child) = node.child(i) {
                        stack.push(child);
                    }
                }
            }
            tokens.sort_by_key(|t| t.range.start);
            tokens.dedup_by(|a, b| a.range == b.range && a.kind == b.kind);
            (tokens, diagnostics)
        }

        fn classify_leaf(node: Node, text: &str) -> Option<SemanticTokenKind> {
            match node.kind() {
                "string" => Some(SemanticTokenKind::String),
                "number" => Some(SemanticTokenKind::Number),
                "true" | "false" | "null" => Some(SemanticTokenKind::Literal),
                _ => {
                    if !node.is_named() {
                        let start = node.start_byte();
                        let end = node.end_byte().min(text.len());
                        if start >= end {
                            return None;
                        }
                        let token = &text[start..end];
                        if token.chars().all(|c| "{}[]:,".contains(c)) {
                            return Some(SemanticTokenKind::Punctuation);
                        }
                    }
                    None
                }
            }
        }

        fn push_token(
            tokens: &mut Vec<SemanticToken>,
            node: Node,
            kind: SemanticTokenKind,
            text: &str,
        ) {
            let start = node.start_byte();
            let end = node.end_byte().min(text.len());
            if start >= end {
                return;
            }
            tokens.push(SemanticToken {
                range: start..end,
                kind,
            });
        }

        fn collect_folding_ranges(root: Node, text: &str) -> Vec<FoldingRange> {
            let mut ranges = Vec::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                let start = node.start_position();
                let end = node.end_position();
                if end.row > start.row && matches!(node.kind(), "object" | "array") {
                    ranges.push(FoldingRange {
                        start_line: start.row,
                        end_line: end.row,
                        kind: FoldingRangeKind::Block,
                        byte_range: node.start_byte()..node.end_byte().min(text.len()),
                    });
                }
                for i in (0..node.child_count()).rev() {
                    if let Some(child) = node.child(i) {
                        stack.push(child);
                    }
                }
            }
            ranges.sort_by_key(|r| (r.start_line, r.end_line));
            ranges.dedup_by(|a, b| a.start_line == b.start_line && a.end_line == b.end_line);
            ranges
        }

        fn collect_outline(root: Node, text: &str) -> Vec<SymbolNode> {
            let mut items = Vec::new();
            for i in 0..root.named_child_count() {
                if let Some(child) = root.named_child(i) {
                    append_symbol(child, text, &mut items);
                }
            }
            items
        }

        fn append_symbol(node: Node, text: &str, out: &mut Vec<SymbolNode>) {
            match node.kind() {
                "object" => {
                    let children = collect_properties(node, text);
                    out.push(SymbolNode {
                        name: format_object_label(&children),
                        kind: SymbolKind::JsonObject,
                        range: node.start_byte()..node.end_byte().min(text.len()),
                        selection_range: node.start_byte()..node.end_byte().min(text.len()),
                        children,
                    });
                }
                "array" => {
                    let child_symbols = collect_array_children(node, text);
                    out.push(SymbolNode {
                        name: format!("Array [{}]", child_symbols.len()),
                        kind: SymbolKind::JsonArray,
                        range: node.start_byte()..node.end_byte().min(text.len()),
                        selection_range: node.start_byte()..node.end_byte().min(text.len()),
                        children: child_symbols,
                    });
                }
                "pair" => {
                    out.push(build_property_symbol(node, text));
                }
                _ => {
                    for i in 0..node.named_child_count() {
                        if let Some(child) = node.named_child(i) {
                            append_symbol(child, text, out);
                        }
                    }
                }
            }
        }

        fn collect_properties(object: Node, text: &str) -> Vec<SymbolNode> {
            let mut props = Vec::new();
            for i in 0..object.named_child_count() {
                if let Some(child) = object.named_child(i) {
                    if child.kind() == "pair" {
                        props.push(build_property_symbol(child, text));
                    }
                }
            }
            props
        }

        fn collect_array_children(array: Node, text: &str) -> Vec<SymbolNode> {
            let mut result = Vec::new();
            for i in 0..array.named_child_count() {
                if let Some(child) = array.named_child(i) {
                    append_symbol(child, text, &mut result);
                }
            }
            result
        }

        fn build_property_symbol(node: Node, text: &str) -> SymbolNode {
            let name = node
                .child_by_field_name("key")
                .and_then(|key| key.utf8_text(text.as_bytes()).ok())
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_else(|| "(property)".to_string());
            let mut children = Vec::new();
            if let Some(value) = node.child_by_field_name("value") {
                append_symbol(value, text, &mut children);
            }
            SymbolNode {
                name,
                kind: SymbolKind::JsonProperty,
                range: node.start_byte()..node.end_byte().min(text.len()),
                selection_range: node.start_byte()..node.end_byte().min(text.len()),
                children,
            }
        }

        fn format_object_label(children: &[SymbolNode]) -> String {
            if let Some(first) = children.first() {
                format!("Object {{{}}}", first.name)
            } else {
                "Object {}".to_string()
            }
        }
    }

    mod javascript {
        use super::*;

        pub fn build_snapshot(
            language: LanguageKind,
            tree: &Tree,
            text: &str,
            hash: u64,
        ) -> SemanticSnapshot {
            let root = tree.root_node();
            let (tokens, diagnostics) = collect_tokens_and_diagnostics(root, text);
            let folding_ranges = collect_folding_ranges(root, text);
            let outline = collect_outline(root, text);
            SemanticSnapshot {
                language,
                source_hash: hash,
                tokens,
                folding_ranges,
                outline,
                diagnostics,
                root_sexpr: Some(root.to_sexp()),
            }
        }

        fn collect_tokens_and_diagnostics(
            root: Node,
            text: &str,
        ) -> (Vec<SemanticToken>, Vec<ParseDiagnostic>) {
            let mut tokens = Vec::new();
            let mut diagnostics = Vec::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                if node.kind() == "ERROR" {
                    diagnostics.push(ParseDiagnostic {
                        message: "Parse error".to_string(),
                        range: node.start_byte()..node.end_byte().min(text.len()),
                    });
                }

                if node.child_count() == 0 {
                    if let Some(kind) = classify_leaf(node, text) {
                        push_token(&mut tokens, node, kind, text);
                    }
                    continue;
                }

                for i in (0..node.child_count()).rev() {
                    if let Some(child) = node.child(i) {
                        stack.push(child);
                    }
                }
            }
            tokens.sort_by_key(|t| t.range.start);
            tokens.dedup_by(|a, b| a.range == b.range && a.kind == b.kind);
            (tokens, diagnostics)
        }

        fn classify_leaf(node: Node, text: &str) -> Option<SemanticTokenKind> {
            match node.kind() {
                "comment" | "hash_bang_line" => Some(SemanticTokenKind::Comment),
                "string" | "template_string" => Some(SemanticTokenKind::String),
                "number" => Some(SemanticTokenKind::Number),
                "regex" => Some(SemanticTokenKind::Literal),
                "identifier" => Some(SemanticTokenKind::Identifier),
                _ => {
                    if !node.is_named() {
                        let start = node.start_byte();
                        let end = node.end_byte().min(text.len());
                        if start >= end {
                            return None;
                        }
                        let token = &text[start..end];
                        if is_js_keyword(token) {
                            return Some(SemanticTokenKind::Keyword);
                        }
                        if token.len() == 1
                            && token.chars().all(|c| "{}[]().,;:+-*/%<>=!&|^?".contains(c))
                        {
                            return Some(SemanticTokenKind::Punctuation);
                        }
                    }
                    None
                }
            }
        }

        fn push_token(
            tokens: &mut Vec<SemanticToken>,
            node: Node,
            kind: SemanticTokenKind,
            text: &str,
        ) {
            let start = node.start_byte();
            let end = node.end_byte().min(text.len());
            if start >= end {
                return;
            }
            tokens.push(SemanticToken {
                range: start..end,
                kind,
            });
        }

        fn collect_folding_ranges(root: Node, text: &str) -> Vec<FoldingRange> {
            let mut ranges = Vec::new();
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                let start = node.start_position();
                let end = node.end_position();
                if end.row > start.row
                    && matches!(
                        node.kind(),
                        "statement_block"
                            | "class_body"
                            | "object"
                            | "array"
                            | "function_declaration"
                            | "method_definition"
                            | "arrow_function"
                    )
                {
                    let kind = if node.kind() == "comment" {
                        FoldingRangeKind::Comment
                    } else {
                        FoldingRangeKind::Block
                    };
                    ranges.push(FoldingRange {
                        start_line: start.row,
                        end_line: end.row,
                        kind,
                        byte_range: node.start_byte()..node.end_byte().min(text.len()),
                    });
                }
                for i in (0..node.child_count()).rev() {
                    if let Some(child) = node.child(i) {
                        stack.push(child);
                    }
                }
            }
            ranges.sort_by_key(|r| (r.start_line, r.end_line));
            ranges.dedup_by(|a, b| a.start_line == b.start_line && a.end_line == b.end_line);
            ranges
        }

        fn collect_outline(root: Node, text: &str) -> Vec<SymbolNode> {
            let mut items = Vec::new();
            for i in 0..root.named_child_count() {
                if let Some(child) = root.named_child(i) {
                    append_symbol(child, text, &mut items);
                }
            }
            items
        }

        fn append_symbol(node: Node, text: &str, out: &mut Vec<SymbolNode>) {
            if let Some(symbol) = build_symbol(node, text) {
                out.push(symbol);
                return;
            }
            for i in 0..node.named_child_count() {
                if let Some(child) = node.named_child(i) {
                    append_symbol(child, text, out);
                }
            }
        }

        fn build_symbol(node: Node, text: &str) -> Option<SymbolNode> {
            match node.kind() {
                "function_declaration" => {
                    let name = node
                        .child_by_field_name("name")
                        .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "(anonymous)".to_string());
                    let body_children = node
                        .child_by_field_name("body")
                        .map(|body| collect_nested_symbols(body, text))
                        .unwrap_or_default();
                    Some(SymbolNode {
                        name: format!("fn {name}"),
                        kind: SymbolKind::JsFunction,
                        range: node.start_byte()..node.end_byte().min(text.len()),
                        selection_range: node.start_byte()..node.end_byte().min(text.len()),
                        children: body_children,
                    })
                }
                "class_declaration" => {
                    let name = node
                        .child_by_field_name("name")
                        .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "(anonymous class)".to_string());
                    let body_children = node
                        .child_by_field_name("body")
                        .map(|body| collect_nested_symbols(body, text))
                        .unwrap_or_default();
                    Some(SymbolNode {
                        name: format!("class {name}"),
                        kind: SymbolKind::JsClass,
                        range: node.start_byte()..node.end_byte().min(text.len()),
                        selection_range: node.start_byte()..node.end_byte().min(text.len()),
                        children: body_children,
                    })
                }
                "method_definition" => {
                    let name = node
                        .child_by_field_name("name")
                        .and_then(|n| n.utf8_text(text.as_bytes()).ok())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "(method)".to_string());
                    let body_children = node
                        .child_by_field_name("body")
                        .map(|body| collect_nested_symbols(body, text))
                        .unwrap_or_default();
                    Some(SymbolNode {
                        name,
                        kind: SymbolKind::JsMethod,
                        range: node.start_byte()..node.end_byte().min(text.len()),
                        selection_range: node.start_byte()..node.end_byte().min(text.len()),
                        children: body_children,
                    })
                }
                "lexical_declaration" | "variable_declaration" => {
                    let mut names = Vec::new();
                    for i in 0..node.named_child_count() {
                        if let Some(child) = node.named_child(i) {
                            if child.kind() == "variable_declarator" {
                                if let Some(name_node) = child.child_by_field_name("name") {
                                    if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
                                        names.push(name.to_string());
                                    }
                                }
                            }
                        }
                    }
                    if names.is_empty() {
                        None
                    } else {
                        Some(SymbolNode {
                            name: names.join(", "),
                            kind: SymbolKind::JsVariable,
                            range: node.start_byte()..node.end_byte().min(text.len()),
                            selection_range: node.start_byte()..node.end_byte().min(text.len()),
                            children: Vec::new(),
                        })
                    }
                }
                _ => None,
            }
        }

        fn collect_nested_symbols(node: Node, text: &str) -> Vec<SymbolNode> {
            let mut items = Vec::new();
            for i in 0..node.named_child_count() {
                if let Some(child) = node.named_child(i) {
                    if let Some(symbol) = build_symbol(child, text) {
                        items.push(symbol);
                    } else {
                        items.extend(collect_nested_symbols(child, text));
                    }
                }
            }
            items
        }

        pub(super) fn is_js_keyword(token: &str) -> bool {
            matches!(
                token,
                "break"
                    | "case"
                    | "catch"
                    | "class"
                    | "const"
                    | "continue"
                    | "debugger"
                    | "default"
                    | "delete"
                    | "do"
                    | "else"
                    | "export"
                    | "extends"
                    | "finally"
                    | "for"
                    | "function"
                    | "if"
                    | "import"
                    | "in"
                    | "instanceof"
                    | "let"
                    | "new"
                    | "return"
                    | "super"
                    | "switch"
                    | "this"
                    | "throw"
                    | "try"
                    | "typeof"
                    | "var"
                    | "void"
                    | "while"
                    | "with"
                    | "yield"
            )
        }
    }

    static SQL_SERVICE: OnceCell<Mutex<ParserService>> = OnceCell::new();
    static JSON_SERVICE: OnceCell<Mutex<ParserService>> = OnceCell::new();
    static JS_SERVICE: OnceCell<Mutex<ParserService>> = OnceCell::new();

    fn service_cell(language: LanguageKind) -> Option<&'static OnceCell<Mutex<ParserService>>> {
        match language {
            LanguageKind::Sql => Some(&SQL_SERVICE),
            LanguageKind::Redis => Some(&JSON_SERVICE),
            LanguageKind::Mongo => Some(&JS_SERVICE),
            LanguageKind::Plain => None,
        }
    }

    pub fn ensure_snapshot(language: LanguageKind, text: &str) -> Option<Arc<SemanticSnapshot>> {
        let cell = service_cell(language)?;
        let mutex = cell.get_or_init(|| {
            let parser = ParserService::new(language).ok();
            Mutex::new(parser.unwrap_or_else(|| ParserService {
                parser: Parser::new(),
                tree: None,
                last_text: String::new(),
                last_hash: 0,
                snapshot: None,
                language,
                grammar: GrammarKind::for_language(language).unwrap(),
            }))
        });
        mutex.lock().ok()?.ensure_snapshot(text)
    }

    pub fn last_snapshot(language: LanguageKind) -> Option<Arc<SemanticSnapshot>> {
        service_cell(language)
            .and_then(|cell| cell.get())
            .and_then(|mutex| mutex.lock().ok())
            .and_then(|guard| guard.snapshot.clone())
    }

    fn is_js_keyword(token: &str) -> bool {
        javascript::is_js_keyword(token)
    }
}

#[cfg(not(feature = "tree_sitter_sequel"))]
mod ts {
    use super::*;

    pub fn ensure_snapshot(_language: LanguageKind, _text: &str) -> Option<Arc<SemanticSnapshot>> {
        None
    }

    pub fn last_snapshot(_language: LanguageKind) -> Option<Arc<SemanticSnapshot>> {
        None
    }
}

/// Update the global SQL snapshot and build a themed layout job.
#[allow(unused_variables)]
pub fn try_tree_sitter_sequel_highlight(text: &str, dark: bool) -> Option<LayoutJob> {
    #[cfg(feature = "tree_sitter_sequel")]
    {
        let snapshot = ts::ensure_snapshot(LanguageKind::Sql, text)?;
        Some(layout_from_tokens(text, &snapshot.tokens, dark))
    }
    #[cfg(not(feature = "tree_sitter_sequel"))]
    {
        let _ = (text, dark);
        None
    }
}

/// Returns the most recently computed SQL semantic snapshot (if any).
pub fn get_last_sql_snapshot() -> Option<Arc<SemanticSnapshot>> {
    ts::last_snapshot(LanguageKind::Sql)
}

/// Ensure the parser cache is synchronized with `text`, returning the snapshot.
pub fn ensure_sql_semantics(text: &str) -> Option<Arc<SemanticSnapshot>> {
    ts::ensure_snapshot(LanguageKind::Sql, text)
}

/// Ensure the parser cache for `language` matches `text` and return the snapshot.
pub fn ensure_semantics(language: LanguageKind, text: &str) -> Option<Arc<SemanticSnapshot>> {
    ts::ensure_snapshot(language, text)
}

/// Fetch the last semantic snapshot recorded for `language`.
pub fn get_last_semantic_snapshot(language: LanguageKind) -> Option<Arc<SemanticSnapshot>> {
    ts::last_snapshot(language)
}

fn layout_from_tokens(text: &str, tokens: &[SemanticToken], dark: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut cursor = 0usize;
    for token in tokens {
        if token.range.start > cursor {
            job.append(
                &text[cursor..token.range.start],
                0.0,
                TextFormat {
                    color: normal_color(dark),
                    ..Default::default()
                },
            );
        }
        let color = color_for_token(token.kind, dark);
        let slice = &text[token.range.start..token.range.end];
        job.append(
            slice,
            0.0,
            TextFormat {
                color,
                ..Default::default()
            },
        );
        cursor = token.range.end;
    }
    if cursor < text.len() {
        job.append(
            &text[cursor..],
            0.0,
            TextFormat {
                color: normal_color(dark),
                ..Default::default()
            },
        );
    }
    job
}

fn color_for_token(kind: SemanticTokenKind, dark: bool) -> Color32 {
    match kind {
        SemanticTokenKind::Keyword => keyword_color(dark),
        SemanticTokenKind::String => string_color(dark),
        SemanticTokenKind::Comment => comment_color(dark),
        SemanticTokenKind::Number => number_color(dark),
        SemanticTokenKind::Operator | SemanticTokenKind::Punctuation => punctuation_color(dark),
        _ => normal_color(dark),
    }
}

fn is_sql_keyword(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "select"
            | "from"
            | "where"
            | "insert"
            | "into"
            | "update"
            | "delete"
            | "create"
            | "alter"
            | "drop"
            | "table"
            | "values"
            | "join"
            | "left"
            | "right"
            | "inner"
            | "outer"
            | "on"
            | "group"
            | "by"
            | "having"
            | "order"
            | "limit"
            | "offset"
            | "union"
            | "distinct"
            | "asc"
            | "desc"
            | "and"
            | "or"
            | "not"
            | "null"
            | "is"
            | "set"
            | "as"
            | "in"
            | "exists"
            | "case"
            | "when"
            | "then"
            | "else"
            | "end"
            | "between"
            | "like"
            | "all"
            | "any"
            | "some"
            | "transaction"
            | "commit"
            | "rollback"
            | "show"
            | "start"
            | "stop"
            | "reset"
            | "change"
            | "purge"
            | "binary"
            | "logs"
            | "privileges"
            | "grants"
            | "processlist"
            | "before"
            | "to"
            | "primary"
            | "key"
            | "foreign"
            | "references"
            | "constraint"
            | "unique"
            | "index"
            | "using"
            | "btree"
            | "hash"
            | "default"
            | "auto_increment"
            | "current_timestamp"
            | "engine"
            | "innodb"
            | "myisam"
            | "charset"
            | "collate"
            | "character"
            | "row_format"
            | "dynamic"
            | "compressed"
            | "redundant"
            | "compact"
            | "int"
            | "varchar"
            | "text"
            | "bigint"
            | "datetime"
            | "timestamp"
            | "boolean"
            | "decimal"
            | "float"
            | "double"
            | "char"
            | "longtext"
            | "mediumtext"
            | "tinytext"
            | "blob"
            | "longblob"
            | "mediumblob"
            | "tinyblob"
            | "enum"
            | "unsigned"
            | "signed"
            | "zerofill"
    )
}

// ---------------- Legacy heuristic highlighter (ported from syntax.rs) ----------------

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
    if let Some(cached_job) = cache.get(&hash) {
        return cached_job.clone();
    }
    let job = highlight_text(text, lang, dark);
    if cache.len() > 100 {
        cache.clear();
    }
    cache.insert(hash, job.clone());
    job
}

/// Whole text highlighter (tree-sitter path is kept for side effects only).
pub fn highlight_text(text: &str, lang: LanguageKind, dark: bool) -> LayoutJob {
    if matches!(
        lang,
        LanguageKind::Sql | LanguageKind::Redis | LanguageKind::Mongo
    ) {
        #[cfg(feature = "tree_sitter_sequel")]
        {
            // Keep semantic snapshot in sync; fall back to legacy rendering for stability.
            let _ = ensure_semantics(lang, text);
        }
    }
    let mut job = LayoutJob::default();
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            job.append("\n", 0.0, TextFormat::default());
        }
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
        job.append(
            line,
            0.0,
            TextFormat {
                color: comment_color(dark),
                ..Default::default()
            },
        );
        return;
    }
    let mut chars = line.char_indices().peekable();
    while let Some((start_idx, ch)) = chars.next() {
        if ch == '\'' {
            let mut end_idx = start_idx + 1;
            let mut found_end = false;
            for (idx, c) in chars.by_ref() {
                end_idx = idx + c.len_utf8();
                if c == '\'' {
                    found_end = true;
                    break;
                }
            }
            if !found_end {
                end_idx = line.len();
            }
            job.append(
                &line[start_idx..end_idx],
                0.0,
                TextFormat {
                    color: string_color(dark),
                    ..Default::default()
                },
            );
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let mut end_idx = start_idx;
            let mut word_chars = vec![ch];
            while let Some(&(_, next_ch)) = chars.peek() {
                if next_ch.is_ascii_alphanumeric() || next_ch == '_' {
                    word_chars.push(next_ch);
                    let (idx, c) = chars.next().unwrap();
                    end_idx = idx + c.len_utf8();
                } else {
                    break;
                }
            }
            if end_idx == start_idx {
                end_idx = start_idx + ch.len_utf8();
            }
            let word = &line[start_idx..end_idx];
            let color = word_color(word, lang, dark);
            job.append(
                word,
                0.0,
                TextFormat {
                    color,
                    ..Default::default()
                },
            );
        } else if ch.is_whitespace() {
            job.append(
                &ch.to_string(),
                0.0,
                TextFormat {
                    color: normal_color(dark),
                    ..Default::default()
                },
            );
        } else {
            job.append(
                &ch.to_string(),
                0.0,
                TextFormat {
                    color: punctuation_color(dark),
                    ..Default::default()
                },
            );
        }
    }
}

fn word_color(word: &str, lang: LanguageKind, dark: bool) -> Color32 {
    if word.chars().all(|c| c.is_ascii_digit()) {
        return number_color(dark);
    }
    if matches!(lang, LanguageKind::Sql) && is_sql_keyword(word) {
        return keyword_color(dark);
    }
    normal_color(dark)
}

fn keyword_color(dark: bool) -> Color32 {
    if dark {
        Color32::from_rgb(255, 179, 0) // rgba(255, 179, 0, 1)
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

fn string_color(_: bool) -> Color32 {
    Color32::from_rgb(21, 255, 0) // rgba(21, 255, 0, 1)
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
