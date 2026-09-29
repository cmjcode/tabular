# Query Diagram & AI Optimization

Right-click inside the SQL editor and choose **Show Query Diagram**. Tabular explains the
selected statement, or the statement at the cursor, as an animated data-flow diagram in a
split view on the right of the editor. Two floating buttons in the bottom-right corner of
the canvas open the step-by-step explanation (**?**) and the AI optimization window
(**Analyze AI**). Both windows float in the bottom-right corner, just above the buttons;
when both are open and there is room, How it works sits to the left of the AI window.

Works for MySQL, PostgreSQL, SQLite and SQL Server connections. On Redis, MongoDB and HTTP
connections the panel explains that diagrams are not available.

---

## What each statement looks like

The diagram reads left to right: **sources → conditions → transformation → target**.

| Statement | Left | Middle | Right |
|---|---|---|---|
| `SELECT` | Source tables with the columns the query uses; joins drawn as blue curves between join columns | `Conditions` card: WHERE, GROUP BY, HAVING, ORDER BY, LIMIT | **Result set** card (violet, glowing) with every output column; green particles flow from each source column into the output column it feeds |
| `INSERT ... VALUES` | | **New values** card with the first row's values and a row count badge | Target table (green, glowing); a `+ N new rows` pill slides in under it |
| `INSERT ... SELECT` | Source tables | Conditions | **SELECT result** card, then the target table |
| `UPDATE` | Tables read by `JOIN` / `FROM` | Conditions | **SET** card with every assignment, then the target table (amber, glowing). Changed columns flash, and a chip next to each one alternates between `was: current <col>` and `now: <new value>` |
| `DELETE` | Tables read by `USING` / `JOIN` | Conditions | Target table (red, glowing); each row is struck through repeatedly, and filtered columns are linked to the WHERE clause with dashed orange lines |

When data lines would cross the Conditions card, the card moves below the flow so the lines
stay readable.

Colour legend, shown at the bottom of the canvas:

- **Blue** lines are join conditions (`ON a.x = b.y`, `USING`, or equalities in WHERE for
  old-style joins, `UPDATE ... FROM` and `DELETE ... USING`).
- **Green** lines are data flow, from the columns that are read to the column they produce.
- **Dashed orange** lines connect filtered columns to the WHERE clause.

Every table card lists all of its columns (from Tabular's schema cache). Columns the
statement uses are highlighted; the rest are dimmed.

The **?** button opens a floating **How it works** window (60% background opacity, so the
diagram stays visible) with the same steps in words. A red banner above the canvas warns
when an `UPDATE` or `DELETE` has no WHERE clause.

### Interaction

- Drag a table card to move it; drag the empty canvas to pan. Scroll or pinch to zoom,
  double-click to fit. The reset button in the header moves every table back.
- Hover a column to highlight its full lineage (upstream and downstream) and see the full
  expression in a tooltip.
- **Replay** restarts the animation. The animation stops by itself after 30 seconds so an
  idle panel does not keep redrawing.
- **Refresh** re-analyzes the statement at the cursor.
- **Open as a diagram tab** converts the picture into a regular diagram tab. That tab is
  never saved over the database diagram.
- Drag the divider to resize the panel; close it with the X button.

---

## Optimization suggestions

Nothing is sent to the AI until you click **Analyze AI**. The first click opens the floating
window and sends the request; clicking the button again or the X hides the window, and the
refresh button inside asks again. The window shows two layers of advice:

1. **Quick checks** are computed locally and appear instantly: missing WHERE on writes,
   `SELECT *`, missing LIMIT, join columns that should be indexed, leading-wildcard LIKE,
   functions wrapped around filtered columns, OR across columns, `NOT IN (SELECT ...)`,
   DISTINCT over joins, and ORDER BY + LIMIT index hints.
2. **AI suggestions** use the backend selected in the AI panel (API key or CLI agent). The
   prompt contains the statement,
   the structure Tabular detected, the cached columns, primary keys and foreign keys of the
   referenced tables, and the quick checks. The reply has a summary, a numbered list of
   suggestions (with `CREATE INDEX` statements when useful) and one optimized query.

**Apply optimized query** replaces the original statement in the editor (one undo step) and
redraws the diagram for the new query. If the statement was edited in the meantime, the
optimized query is copied to the clipboard instead. **Continue in chat** opens the AI panel
with the query prefilled.

If no AI backend is configured, the panel says so and still shows the quick checks.

---

## Architecture

```
src/query_diagram/          # headless, no window_egui dependency
├── mod.rs      # QueryDiagramModel, analyze_with_schema(), finalize(), errors
├── parse.rs    # sqlparser AST -> model (feature `query_ast`, on by default)
├── layout.rs   # cards, rows, flows, step descriptions, warnings
├── render.rs   # egui painter: animation, glow, lineage hover, pan/zoom, card drag,
│               # floating window and button bar helpers
├── prompt.rs   # quick checks, AI prompts, SQL block extraction
└── build.rs    # model -> DiagramState for "Open as a diagram tab"

src/window_egui/query_insight.rs   # split view, schema lookup, AI request/poll, actions
```

- The parser tries the connection's dialect first and falls back to the generic dialect.
  Column references inside expressions are collected with the sqlparser tokenizer, so
  dialect-specific expressions do not break the diagram; subqueries inside expressions are
  skipped because their columns belong to another scope.
- Unqualified columns are resolved against the cached schema; `*` is expanded when the
  table's columns are cached. Without a cache the diagram shows only the columns the query
  names.
- CTEs and derived tables appear as single source cards with a note listing the tables they
  read. For `UNION` only the first query is drawn, with a note.
- Panel state lives in `Tabular::query_insights`, keyed by the stable tab id, and is dropped
  when the tab closes.

Tests: `cargo test --lib query_diagram::`.
