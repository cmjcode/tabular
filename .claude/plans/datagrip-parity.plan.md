# Plan: DataGrip Feature Parity

**Source PRD**: memory/tabular-datagrip-parity-plan.md  
**Complexity**: Large (multi-wave, 8 gelombang total)  
**Status Gelombang**: 0✅ 1✅ 2✅ 3⬜ 4⬜ 5⬜ 6⬜ 7⬜ 8⬜

---

## Summary

Tabular is a native Rust/egui desktop database client targeting DataGrip-level feature parity.
Gelombang 0–2 are complete. This plan covers Gelombang 3–8: schema/DDL UI, data grid power
features, SQL editor intelligence, schema browser upgrades, stored procedure editing, and
visualization. Work each Gelombang sequentially; mark tasks `[x]` as they land in a commit.

---

## Codebase Conventions (mirror these patterns)

| Category | Source | Pattern |
|---|---|---|
| Dialog state | `src/models/structs.rs : IndexDialogState` | Each feature owns its own state struct; no loose fields on god-object `Tabular` |
| Async background | `src/connection/execute.rs : spawn_query_job_batch` | Spawn via `tokio::spawn`; send results through `QueryResultMessage` channel |
| Sidebar nodes | `src/models/enums.rs : NodeType` | Add variant + icon string; handle in `sidebar_tree.rs` click & context-menu |
| UI render panels | `src/window_egui/render_dialogs.rs` | `pub fn render_xxx(&mut self, ctx)` method on `Tabular`; called from `app_impl.rs` |
| Engine dispatch | `src/driver_mysql.rs`, `src/driver_postgres.rs`, … | Match `DatabaseType` arm per driver; share logic in `connection/metadata/` |
| Error propagation | `anyhow::Result` + toast via `self.add_notification(msg)` | No `.unwrap()` in UI paths |
| DDL queries | `src/connection/metadata/ddl.rs` | All schema-reading SQL lives here; UI calls `fetch_*` / `compute_*` functions |

---

## Gelombang 3 — Visual / Schema
> FK introspection → ERD edges · Copy DDL · ALTER TABLE UI · Schema Diff UI

### Tasks

#### 3.1 FK → ERD edges
- **Action**: Call `get_foreign_keys()` (already in `ddl.rs:592`) when a diagram is opened.
  Produce `DiagramEdge { from_table, from_col, to_table, to_col }` and draw arrows in
  `window_egui/diagram.rs`. Add force-directed auto-layout (simple spring algo is fine).
- **Files**: `src/window_egui/diagram.rs`, `src/models/structs.rs : DiagramState`
- **Validate**: Open a MySQL/PG DB with FKs → open diagram → FK lines appear

#### 3.2 Copy DDL (right-click → Copy DDL)
- **Action**: Add context menu item "Copy DDL" on `NodeType::Table | View | StoredProcedure |
  UserFunction | Trigger`. Call `fetch_table_definition()` / `fetch_view_definition()` /
  `fetch_procedure_definition()` then `ctx.copy_text(ddl)`.
- **Files**: `src/window_egui/sidebar_tree.rs` (context menu block ~line 2200+)
- **Validate**: Right-click table → Copy DDL → paste in editor shows correct CREATE TABLE

#### 3.3 ALTER TABLE UI
- **Action**: Reuse `CreateTableWizardState` pattern. Add `AlterTableWizardState` with a list
  of `ColumnDefinition` pre-populated from `fetch_schema_columns()` (ddl.rs:1049).
  Diff original vs modified → emit `ALTER TABLE … ADD/DROP/MODIFY COLUMN` statements.
  Present confirmation dialog before executing.
- **Files**: `src/window_egui/table_wizard.rs`, `src/models/structs.rs`, `src/connection/metadata/ddl.rs`
- **Validate**: Open table → Edit Columns → add a column → Submit → column appears in structure

#### 3.4 Schema Diff UI
- **Action**: `compute_schema_diff()` already exists in `ddl.rs:1112`. Build a dialog with
  two connection selectors + schema selectors → diff button → show side-by-side diff list
  (added tables green, removed red, modified yellow). Export diff as SQL patch.
- **Files**: `src/window_egui/render_dialogs.rs`, `src/connection/metadata/ddl.rs`
- **State**: `SchemaDiffState { conn_a, conn_b, schema_a, schema_b, diff_result, visible }`
- **Validate**: Two PG connections with different schemas → diff shows correct delta

---

## Gelombang 4 — Data Grid Power
> Cell viewer · Add/Delete row · Staged changes · Column stats · Multi result panes

### Tasks

#### 4.1 Cell Value Viewer
- **Action**: Add `CellViewerState { visible: bool, content: String, mode: CellViewMode }`.
  `CellViewMode` = `Raw | Json | Image`. Triggered on cell double-click in `render_data.rs`.
  Render as right-side panel or floating window with `egui::ScrollArea`.
  JSON mode: pretty-print with syntax highlight. Image mode: `egui::Image` from BLOB bytes.
- **Files**: `src/data_table/render_data.rs`, `src/models/structs.rs`
- **Validate**: Click a long-text cell → viewer opens with full content

#### 4.2 Add Row / Delete Row
- **Action**: Toolbar above data grid: `[+ Add Row]` `[- Delete Row]`. Requires PK detection
  from `ColumnMetadata.is_primary_key`. Add Row: insert blank row at bottom with inline edit
  mode. Delete Row: `DELETE FROM {table} WHERE {pk} = {val}` with confirm dialog.
  Only available in table-browse mode (not arbitrary query result).
- **Files**: `src/data_table/render_data.rs`, `src/connection/execute.rs`
- **Validate**: Browse table → Add Row → fill → Submit; Delete Row → confirm → row gone

#### 4.3 Staged Changes Submit
- **Action**: All cell edits accumulate in `Vec<SpreadsheetOperation>`. Show badge "X pending
  changes" in toolbar. "Submit" button runs all ops inside one transaction (BEGIN → ops →
  COMMIT, ROLLBACK on any error). "Revert" discards pending ops and re-fetches.
- **Files**: `src/spreadsheet.rs`, `src/data_table/render_data.rs`, `src/connection/execute.rs`
- **Validate**: Edit 3 cells → Submit → all 3 persisted; test rollback on PK conflict

#### 4.4 Column Statistics
- **Action**: Clicking a column header opens mini popup: `SELECT COUNT(*), COUNT(DISTINCT col),
  MIN(col), MAX(col), AVG(col), SUM(col IS NULL)`. Run in background, show spinner.
- **Files**: `src/data_table/render_data.rs`, new `src/data_table/column_stats.rs`
- **State**: `ColumnStatsState { visible, col_name, loading, result: Option<ColStats> }`
- **Validate**: Click column header → stats popup shows correct values within 2 sec

#### 4.5 Multiple Result Panes (Pinned Results)
- **Action**: Add "Pin result" button on each result toolbar. Pinned results stay as tabs
  inside the result area. Max 5 pinned. `QueryTab` += `pinned_results: Vec<PinnedResult>`.
- **Files**: `src/models/structs.rs`, `src/window_egui/app_impl.rs`
- **Validate**: Run query A → pin → run query B → both results visible in tabs

---

## Gelombang 5 — SQL Editor Intelligence
> Schema-aware completion · Query params · Live lint · Live templates · EXPLAIN ANALYZE

### Tasks

#### 5.1 Schema-Aware Completion
- **Action**: Upgrade `editor_autocomplete_new.rs`. After user types `FROM tablename `, detect
  context (after FROM/JOIN/WHERE/SELECT) via simple token scan. Pull column names from
  `connection/metadata/cache.rs` for the detected table. For JOIN, suggest `ON t.fk_col =
  ref.pk` using FK metadata already loaded in sidebar tree.
- **Files**: `src/editor_autocomplete_new.rs`, `src/connection/metadata/cache.rs`
- **Validate**: `SELECT | FROM users` → completion suggests users' columns; JOIN FK suggestion appears

#### 5.2 Query Parameters
- **Action**: Before executing, regex-scan SQL for `:param_name`, `$1`, `@param_name`
  placeholders. If found, show a modal dialog with input fields. Replace placeholders with
  quoted values before sending to DB.
- **Files**: `src/editor.rs`, `src/connection/sql.rs`
- **State**: `QueryParamsState { params: Vec<(String, String)>, visible }`
- **Validate**: `SELECT * FROM t WHERE id = :id` → dialog asks for `id` value → correct query runs

#### 5.3 Live Lint (Syntax Squiggle)
- **Action**: Enable `sqlparser` feature (already in Cargo.toml as optional). On each editor
  change (debounced 500ms), parse current statement with `sqlparser::Parser`. On error,
  store error span. Render red underline in editor via egui painter at those char positions.
  Tooltip on hover shows error message.
- **Files**: `src/editor.rs`, `Cargo.toml` (enable `sqlparser` feature)
- **Validate**: Type `SELEKT * FORM t` → red underline appears; correct SQL → underline clears

#### 5.4 Live Templates / Snippets
- **Action**: Store snippets in `~/.tabular/snippets.json` as `[{ trigger, body, description }]`.
  In editor, when user types a trigger word + Tab, expand it. Manage via Settings > Snippets tab.
- **Files**: `src/editor.rs`, `src/window_egui/settings.rs`, `src/config.rs`
- **Built-in defaults**: `sel` → `SELECT * FROM `, `ins` → `INSERT INTO  VALUES ()`, `upd` → `UPDATE  SET  WHERE `
- **Validate**: Type `sel` then Tab → expands to `SELECT * FROM `

#### 5.5 EXPLAIN ANALYZE
- **Action**: Add "Explain Analyze" button next to existing Explain. PG: `EXPLAIN (ANALYZE,
  BUFFERS, FORMAT TEXT)`. MySQL 8+: `EXPLAIN ANALYZE`. Show raw output in scrollable pane.
  Highlight lines containing "actual time=" with yellow background.
- **Files**: `src/editor.rs`, `src/window_egui/render_dialogs.rs`
- **Validate**: Run EXPLAIN ANALYZE on a slow query → output shows timing per node

---

## Gelombang 6 — Schema Browser & Object Search
> Go to Object · Materialized views · Sequences · Connection color tag · Schema filter

### Tasks

#### 6.1 Go to Object (Cmd+Shift+O)
- **Action**: Global fuzzy-search dialog across all loaded `TreeNode` items. User types → filter
  list shows matching tables/views/SPs/functions with DB prefix. Enter → expand sidebar to
  node + optionally open DDL in query tab.
- **Files**: `src/window_egui/search.rs` (extend existing search), `src/window_egui/sidebar_tree.rs`
- **State**: `GoToObjectState { visible, query, results: Vec<TreeNodeRef> }`
- **Validate**: Cmd+Shift+O → type "user" → shows tables/views/SPs containing "user" from all connections

#### 6.2 Materialized Views (PostgreSQL)
- **Action**: Add `NodeType::MaterializedView` + `MaterializedViewsFolder`. Query:
  `SELECT matviewname, definition FROM pg_matviews WHERE schemaname = $1`.
  Right-click → Copy DDL, Refresh, Drop.
- **Files**: `src/models/enums.rs`, `src/driver_postgres.rs`, `src/window_egui/sidebar_tree.rs`
- **Validate**: PG DB with mat views → appears under new "Materialized Views" folder

#### 6.3 Sequences (PostgreSQL)
- **Action**: Add `NodeType::Sequence` + `SequencesFolder`. Query `information_schema.sequences`.
  Show: current_value, min, max, increment, cycle in a detail panel.
- **Files**: `src/models/enums.rs`, `src/driver_postgres.rs`, `src/window_egui/sidebar_tree.rs`
- **Validate**: PG DB with sequences → "Sequences" folder visible with correct data

#### 6.4 Connection Color Tag
- **Action**: Add `color: Option<[u8; 3]>` to `ConnectionConfig` (persist in connections.db).
  In sidebar, draw a 4px vertical color strip on the left of each connection node. Color
  picker in connection settings dialog. Preset colors: Red=Prod, Yellow=Staging, Green=Dev, Blue=Local.
- **Files**: `src/models/structs.rs`, `src/sidebar_database.rs`, `src/window_egui/sidebar_tree.rs`
- **Validate**: Set connection color red → sidebar shows red strip; persists after restart

#### 6.5 Schema Filter
- **Action**: Gear icon in sidebar header → checkbox list of schemas. Hidden schemas not
  loaded/shown. Preference stored per connection in config.
- **Files**: `src/window_egui/sidebar_tree.rs`, `src/config.rs`
- **Validate**: Uncheck schema → its tables disappear; re-check → reappear

---

## Gelombang 7 — Stored Procedure Editor & Import Extensions
> SP editor · Import XLSX · Import JSON · Full-text search in query files

### Tasks

#### 7.1 Stored Procedure / Function Editor
- **Action**: Right-click `StoredProcedure | UserFunction | Trigger` → "Edit..." opens a new
  query tab with DDL pre-filled (from `fetch_procedure_definition()`). Run button executes
  `CREATE OR REPLACE`. Detect engine: PG uses `CREATE OR REPLACE`, MySQL drops + creates.
- **Files**: `src/editor.rs`, `src/window_egui/sidebar_tree.rs`, `src/connection/metadata/ddl.rs`
- **Validate**: Edit and save a stored procedure → changes persist in DB

#### 7.2 Import XLSX
- **Action**: Add crate `calamine = "0.24"` to Cargo.toml. Extend import wizard with file
  type selector (CSV / XLSX / JSON). XLSX path: open workbook → list sheets → user picks
  sheet → read headers + rows → same column-mapping UI as CSV.
- **Files**: `src/window_egui/app_impl.rs` (import wizard), `Cargo.toml`
- **Validate**: Import .xlsx file → data appears in table

#### 7.3 Import JSON Array
- **Action**: Accept `[{...}, {...}]` JSON files. Parse with `serde_json`. Infer columns from
  union of all keys. Same mapping UI. Handle nested objects as JSON string fallback.
- **Files**: Same import wizard files as 7.2
- **Validate**: Import JSON array → data appears in table

#### 7.4 Full-Text Search in Query Files
- **Action**: Search bar in "Queries" sidebar panel greps inside `.sql` file contents
  (not just filename). Show file name + matching line excerpt. Click → open file + scroll to match.
- **Files**: `src/window_egui/search.rs`, `src/sidebar_query.rs`
- **Validate**: Type a SQL keyword → query files containing that keyword appear in results

---

## Gelombang 8 — Visualization & Polish
> Visual EXPLAIN tree · Chart from result · Data comparison · Transpose result

### Tasks

#### 8.1 Visual EXPLAIN Tree (PostgreSQL)
- **Action**: PG supports `EXPLAIN (FORMAT JSON)`. Parse JSON tree. Render as collapsible
  egui tree where each node shows: Node Type, Cost, Actual Time, Rows. Color-code by
  relative cost (green=fast, yellow=medium, red=expensive). Toggle between text and tree view.
- **Files**: `src/window_egui/render_dialogs.rs`, new `src/window_egui/explain_tree.rs`
- **Validate**: EXPLAIN on a join query → tree renders with correct node hierarchy

#### 8.2 Chart from Result
- **Action**: Add `egui_plot` crate. "Chart" button on result toolbar. Dialog: pick X column,
  Y column(s), chart type (Bar / Line / Scatter). Render inline below result or in floating window.
- **Files**: `src/data_table/render_data.rs`, `Cargo.toml`
- **Validate**: SELECT date, count FROM … → chart renders as line chart

#### 8.3 Data Comparison
- **Action**: New menu item "Compare Data..." (right-click table). Select two tables/queries
  (can be different connections). Fetch both result sets. Show side-by-side with rows
  highlighted: green=only in A, red=only in B, yellow=same key different values.
  Key columns user-selectable.
- **Files**: New `src/window_egui/data_compare.rs`, `src/models/structs.rs`
- **State**: `DataCompareState { visible, left, right, key_cols, diff_rows }`
- **Validate**: Two tables with 1 row difference → diff shows correctly

#### 8.4 Transpose Result
- **Action**: Cmd+T (or button) on result pane rotates rows <-> columns. Column names become
  row labels in column 0. Useful for wide result sets with few rows (e.g., SHOW STATUS).
  Togglable; state stored per result tab.
- **Files**: `src/data_table/render_data.rs`, `src/models/structs.rs`
- **Validate**: Wide 1-row result → transpose → each column becomes a row

---

## Execution Order (recommended)

```
3 → 4.1 → 4.2 → 4.3 → 5.1 → 5.2 → 6.1 → 6.4 → 7.1 → 4.4 → 4.5 → 5.3 → 5.4 → 5.5 → 6.2 → 6.3 → 6.5 → 7.2 → 7.3 → 7.4 → 8.1 → 8.2 → 8.3 → 8.4
```

Prioritas tinggi (paling sering dipakai di DataGrip daily use):

```
3 → 4.1+4.2+4.3 → 5.1+5.2 → 6.1+6.4 → 7.1
```

---

## Validation Commands

```bash
# Build check sebelum commit
cargo build --release 2>&1 | tail -20

# Run tests (3 known failing tests di query_ast — biarkan, jangan #[ignore])
cargo test 2>&1 | grep -E "FAILED|ok|error"

# Lint
cargo clippy -- -D warnings 2>&1 | tail -30
```

---

## Risks

| Risk | Likelihood | Mitigasi |
|---|---|---|
| `editor_autocomplete_new.rs` belum mature untuk context-aware | HIGH | Mulai dari table name detection dulu; FK suggestion boleh tertunda |
| Visual EXPLAIN JSON parse kompleks | MEDIUM | Fallback ke text view jika JSON parse gagal |
| `egui_plot` kompatibilitas egui 0.32 | LOW | Verify version sebelum mulai 8.2; pin versi |
| Staged changes butuh reliable PK detection | MEDIUM | Wajibkan table-browse mode; show warning jika no PK |
| `calamine` XLSX crate cukup besar | LOW | Gate di cargo feature `import-xlsx` jika perlu |

---

## Acceptance Criteria per Gelombang

- [ ] **G3**: FK arrows muncul di diagram; Copy DDL berfungsi; ALTER TABLE UI bisa add/drop column; Schema Diff menampilkan delta
- [ ] **G4**: Cell viewer buka large text; Add/Delete row works; Staged submit in 1 tx; Column stats popup correct; Multiple pinned result tabs
- [ ] **G5**: Completion suggests columns from correct table; Query params dialog appears; Lint underlines bad syntax; Snippets expand on Tab; EXPLAIN ANALYZE shows timing
- [ ] **G6**: Cmd+Shift+O searches all loaded objects; Mat views folder in PG; Sequences folder in PG; Connection color persists; Schema filter hides/shows schemas
- [ ] **G7**: SP editor opens DDL + saves back; XLSX import works; JSON array import works; Query file content search works
- [ ] **G8**: Visual EXPLAIN tree renders for PG; Chart from result; Data compare shows diff rows; Transpose toggles correctly
