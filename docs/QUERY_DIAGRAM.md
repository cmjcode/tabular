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

The canvas fills the whole panel; there is no title bar. The diagram reads left to right in
SQL execution order: **sources → WHERE → GROUP BY → HAVING → ORDER BY / LIMIT → result or
SET → target**. Each clause is its own stage card, and thick violet **pipeline** arrows
connect the stages, labelled "matching rows", "groups", "kept groups" and "ordered rows".

- **WHERE** lists one condition per row (split at top-level AND) and filters rows.
- **GROUP BY** lists the group keys and every aggregate computed per group. Column data
  flows from the source tables into the keys and aggregates, and from there into the
  result. Aggregates without GROUP BY show a single "(all rows form one group)" key.
- **HAVING** filters groups; dashed lines show which aggregate or key it tests.
- **ORDER BY / LIMIT** (and DISTINCT) shape the final rows.

| Statement | Left | Middle | Right |
|---|---|---|---|
| `SELECT` | Source tables with all their columns; joins drawn as blue curves between join columns | Stage cards: WHERE, GROUP BY, HAVING, ORDER BY / LIMIT | **Result set** card (violet, glowing) with every output column; green particles flow from each source column into the output column it feeds |
| `INSERT ... VALUES` | | **New values** card with the first row's values and a row count badge | Target table (green, glowing); a `+ N new rows` pill slides in under it |
| `INSERT ... SELECT` | Source tables | WHERE and other stages | **SELECT result** card, then the target table |
| `UPDATE` | Tables read by `JOIN` / `FROM` | WHERE and other stages | **SET** card with every assignment, then the target table (amber, glowing). Changed columns flash, and a chip next to each one alternates between `was: current <col>` and `now: <new value>` |
| `DELETE` | Tables read by `USING` / `JOIN` | WHERE and other stages | Target table (red, glowing); each row is struck through repeatedly, and filtered columns are linked to the WHERE clause with dashed orange lines |

Stage cards that column lines would cross (WHERE, HAVING, ORDER BY) sit on a lower track so
the lines stay readable; GROUP BY stays on the main track because data flows through it.

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
  double-click to fit.
- Floating buttons in the top-right corner: **Refresh** re-analyzes the statement at the
  cursor, **Replay** restarts the intro animation, **Reset** moves every table back, **Fit** fits
  the diagram, and **X** closes the panel.
- Hover a column to highlight its full lineage (upstream and downstream) and see the full
  expression in a tooltip.
- The animation loops for as long as the panel is visible; Replay restarts the intro where cards and lines appear stage by stage.
- Drag the divider to resize the panel.

---

## Supported statements

| Statement | How it is drawn |
|---|---|
| `SELECT` with JOIN / WHERE / GROUP BY / HAVING / ORDER BY / LIMIT / DISTINCT / TOP | Source tables, one stage card per clause, result set |
| `INSERT ... VALUES`, `INSERT ... SELECT`, `INSERT ... SET` | New values or SELECT result flowing into the target |
| `REPLACE INTO`, `INSERT IGNORE` | As INSERT, with the verb on the target badge and a note |
| `INSERT ... ON DUPLICATE KEY UPDATE`, `ON CONFLICT DO UPDATE / DO NOTHING` | Extra upsert card whose columns flow into the target |
| `UPDATE`, `UPDATE ... JOIN`, `UPDATE ... FROM` (PostgreSQL and SQL Server alias style) | SET card, then the target with was/now chips |
| `DELETE`, `DELETE ... USING`, multi-table `DELETE t1 FROM t1 JOIN t2` | Target struck through, WHERE stage |
| `TRUNCATE` | Target with a TRUNCATE badge and a "no WHERE clause" warning |
| `MERGE INTO ... USING ... WHEN MATCHED / NOT MATCHED` | Source joined on the ON condition, SET card for matched rows, upsert card for inserts |
| `SELECT ... INTO`, `CREATE TABLE ... AS SELECT` | Drawn like INSERT ... SELECT into the new table |
| `WITH` (CTE), derived tables in FROM | The CTE card is fed by the tables inside it, drawn on its left |
| `IN (subquery)`, `NOT IN`, `EXISTS`, `NOT EXISTS` | The subquery table appears with its badge, linked by the matching columns |
| Scalar subqueries in SELECT or SET | The subquery table feeds the output or SET column |
| Window functions (`OVER (...)`) | WINDOW stage card between the source columns and the result |
| `UNION`, `UNION ALL`, `INTERSECT`, `EXCEPT` | Every branch's tables, a combine card listing the branches, positional column flows |

Recursive CTEs draw their first step only, and CTEs/subqueries nested deeper than three
levels are shown as a single card. DDL other than `CREATE TABLE AS`, and procedural code
(`DECLARE`, `IF`, stored procedure calls), are not drawn.

### Source table layout

Source tables are laid out as a staircase: the FROM table sits top-left and each JOIN is
placed to the right of and lower than the previous one, so join lines run side to side.
Tables inside a CTE or derived table sit in their own column to the left of it. The rows
produced by the FROM + JOIN chain enter the pipeline as one "joined rows" arrow from the
last table.

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
├── layout.rs   # stage cards, rows, flows, pipeline, step descriptions, warnings
├── render.rs   # egui painter: animation, glow, lineage hover, pan/zoom, card drag,
│               # floating window and button bar helpers
└── prompt.rs   # quick checks, AI prompts, SQL block extraction

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
