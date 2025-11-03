#![cfg(feature = "tree_sitter_sequel")]

use tabular::syntax_ts::{self, SemanticTokenKind, SymbolKind};

#[test]
fn semantic_snapshot_contains_outline_and_folding() {
    let sql = "WITH monthly AS (\n    SELECT id FROM accounts\n)\nSELECT * FROM users;\n";
    let snapshot = syntax_ts::ensure_sql_semantics(sql).expect("snapshot available");
    assert!(!snapshot.tokens.is_empty(), "expected token stream");
    assert!(
        snapshot
            .folding_ranges
            .iter()
            .any(|range| range.end_line > range.start_line),
        "expected at least one multi-line folding range"
    );
    assert!(
        snapshot
            .outline
            .iter()
            .any(|symbol| matches!(symbol.kind, SymbolKind::Select)),
        "expected SELECT symbol in outline"
    );
}

#[test]
fn semantic_snapshot_updates_after_edit() {
    let before = "SELECT * FROM users;\n";
    let after = "SELECT id, email FROM users WHERE active = 1;\n";
    let snap_before = syntax_ts::ensure_sql_semantics(before).expect("snapshot before");
    let snap_after = syntax_ts::ensure_sql_semantics(after).expect("snapshot after");

    assert_ne!(
        snap_before.source_hash, snap_after.source_hash,
        "snapshot hash should reflect text changes"
    );
    assert!(
        snap_after
            .tokens
            .iter()
            .any(|token| matches!(token.kind, SemanticTokenKind::Keyword)),
        "expected keywords in updated token stream"
    );
    assert!(
        snap_after
            .outline
            .iter()
            .any(|symbol| matches!(symbol.kind, SymbolKind::Select)),
        "expected outline to retain SELECT entry"
    );
}
