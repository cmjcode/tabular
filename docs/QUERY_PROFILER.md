# Visual Query Profiler & Execution Graph

The **Visual Query Profiler** in Tabular transforms complex, text-heavy execution plans (`EXPLAIN ANALYZE`) into an intuitive, interactive hierarchical node graph.

---

## 🔍 Supported Database Engines & Dialects

| Engine | Query Syntax | Parser Format |
|--------|--------------|---------------|
| **PostgreSQL** | `EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT JSON) <query>` | JSON Tree (`Plan`, `Plans`, `Total Cost`, `Actual Total Time`) |
| **MySQL / MariaDB** | `EXPLAIN FORMAT=JSON <query>` | JSON (`query_block`, `table`, `nested_loop`, `cost_info`) |
| **SQL Server** | `SET SHOWPLAN_XML ON; <query>` | XML ShowPlan (Statement, RelOp, Cost metrics) |

---

## 🏗️ Architecture & Layout Algorithm

The profiler is implemented across three core modules in `src/query_profiler/`:

```
src/query_profiler/
├── mod.rs        # Main state, ProfilerWindow, and public integration
├── parser.rs     # Multi-engine JSON & XML plan parsers -> ExplainNode tree
├── graph.rs      # Hierarchical Sugiyama layout & egui interactive canvas
└── warnings.rs   # Automated heuristic bottleneck & scan detectors
```

### 1. Hierarchical Sugiyama Layout
The graph generator automatically calculates layer ranks, assigns coordinates, and renders smooth bezier connection curves:
- **Layer Assignment**: Root query node at the top, source tables / scans at the bottom (or vice-versa).
- **Node Cost Metric**: Each node calculates its percentage of the total query cost:
  $$\text{Node Cost \%} = \frac{\text{Node Cost}}{\text{Root Total Cost}} \times 100\%$$
- **Dynamic Heatmap Coloring**:
  - 🟢 **< 20% Cost**: Low impact (Green/Neutral)
  - 🟡 **20% - 60% Cost**: Moderate impact (Yellow/Orange)
  - 🔴 **> 60% Cost**: Critical bottleneck (Vibrant Red)

### 2. Automated Heuristics & Warnings
The `warnings.rs` engine scans the plan for common query anti-patterns:
- **Sequential Scan on Large Table**: Triggered when a `Seq Scan` is performed on a table with more than 10,000 rows.
- **Cartesian Product**: Triggered on `Nested Loop` joins without adequate join conditions.
- **Disk Spill**: Triggered when sort operations spill from work memory (`work_mem`) to disk.
- **High Estimated vs. Actual Ratio**: Detects stale optimizer statistics when estimated rows differ significantly from actual rows.

---

## 🖥️ User Interface & Controls

1. **Open the Profiler**:
   - In any SQL query tab, execute your query with `EXPLAIN (ANALYZE)` or click the **"📊 Explain Plan"** button in the query action bar.
2. **Interactive Canvas**:
   - **Pan**: Click and drag the canvas background with the left mouse button.
   - **Zoom**: Scroll mouse wheel or use pinch-to-zoom on trackpads.
   - **Node Details**: Click any node to open the detailed metrics inspector (Actual Time, Startup Cost, Rows Filtered, Buffer Hits/Reads).
   - **Raw JSON / Text View**: Switch between the Visual Graph and the raw formatted output using the top mode toggle.
