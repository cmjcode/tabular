//! Konversi diagram query ke `DiagramState` biasa supaya bisa dibuka di tab
//! diagram (fitur "Open in new tab"). Tab hasil konversi ditandai
//! `scoped_to` sehingga tidak pernah disimpan sebagai diagram database.

use crate::models::structs::{DiagramNode, DiagramState, RelationOrigin, VirtualRelation};

use super::layout::{CardRole, FlowKind, QueryLayout};

/// Penanda `scoped_to` untuk tab diagram hasil query.
pub const QUERY_SCOPE: &str = "query";

pub fn build_diagram_state(layout: &QueryLayout) -> DiagramState {
    let mut state = DiagramState::default();
    for card in &layout.cards {
        if card.role == CardRole::Clauses {
            continue;
        }
        state.nodes.push(DiagramNode {
            id: card.id.clone(),
            title: format!("{} [{}]", card.title, card.badge),
            pos: card.rect.min,
            size: card.rect.size(),
            columns: card
                .rows
                .iter()
                .filter(|r| !r.key.is_empty())
                .map(|r| r.key.clone())
                .collect(),
            detached: true,
            ..Default::default()
        });
    }
    for f in &layout.flows {
        if f.kind == FlowKind::Filter {
            continue;
        }
        let (a, b) = (&layout.cards[f.from.0], &layout.cards[f.to.0]);
        let (Some(ra), Some(rb)) = (f.from.1, f.to.1) else {
            continue;
        };
        let (Some(ca), Some(cb)) = (a.rows.get(ra), b.rows.get(rb)) else {
            continue;
        };
        if ca.key.is_empty()
            || cb.key.is_empty()
            || a.role == CardRole::Clauses
            || b.role == CardRole::Clauses
        {
            continue;
        }
        state.virtual_relations.push(VirtualRelation {
            child: b.id.clone(),
            child_column: cb.key.clone(),
            parent: a.id.clone(),
            parent_column: ca.key.clone(),
            origin: RelationOrigin::Manual,
        });
    }
    state.scoped_to = Some(QUERY_SCOPE.to_string());
    state.is_centered = false;
    state
}

#[cfg(test)]
#[cfg(feature = "query_ast")]
mod tests {
    use super::*;
    use crate::models::enums::DatabaseType;
    use crate::query_diagram::{analyze_statement, layout::build_layout};

    #[test]
    fn test_state_is_scoped_and_has_relations() {
        let m = analyze_statement(
            "SELECT u.name FROM users u JOIN orders o ON o.user_id = u.id WHERE o.total > 1",
            &DatabaseType::MySQL,
        )
        .unwrap();
        let s = build_diagram_state(&build_layout(&m));
        assert_eq!(s.scoped_to.as_deref(), Some(QUERY_SCOPE));
        // u, o, result (kartu Conditions dilewati).
        assert_eq!(s.nodes.len(), 3);
        // join + u.name -> result.name
        assert_eq!(s.virtual_relations.len(), 2);
    }
}
