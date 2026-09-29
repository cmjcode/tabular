use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod compare;
mod compare_view;
pub mod graph;
pub mod history;
pub mod parser;
pub mod warnings;

pub use graph::{
    CompareAction, CompareContext, render_query_profiler, render_query_profiler_with_history,
};
pub use warnings::{ProfilerWarning, WarningCategory, WarningSeverity};

/// Database engine detected for the EXPLAIN output
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProfilerEngine {
    #[default]
    PostgreSQL,
    MySQL,
    MSSQL,
    SQLite,
    Generic,
}

impl ProfilerEngine {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::PostgreSQL => "PostgreSQL (JSON + Buffers)",
            Self::MySQL => "MySQL (JSON / Analyze)",
            Self::MSSQL => "Microsoft SQL Server (ShowPlan XML)",
            Self::SQLite => "SQLite (Query Plan)",
            Self::Generic => "Generic EXPLAIN",
        }
    }
}

/// Unified representation of a node in the execution plan tree
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainNode {
    pub id: usize,
    pub node_type: String,
    pub relation_name: Option<String>,
    pub schema_name: Option<String>,
    pub alias: Option<String>,
    pub index_name: Option<String>,

    // Cost & Timing
    pub startup_cost: f64,
    pub total_cost: f64,
    pub cost_percentage: f32, // 0.0 - 100.0% relative to plan total cost
    pub actual_startup_time: Option<f64>, // ms
    pub actual_total_time: Option<f64>, // ms
    pub time_percentage: f32, // 0.0 - 100.0% relative to total execution time

    // Rows & cardinality
    pub plan_rows: u64,
    pub plan_width: Option<u64>,
    pub actual_rows: Option<u64>,
    pub actual_loops: Option<u64>,

    // Buffer & I/O statistics
    pub buffer_hit: Option<u64>,     // Shared hit blocks
    pub buffer_read: Option<u64>,    // Shared read blocks
    pub buffer_dirtied: Option<u64>, // Shared dirtied blocks
    pub buffer_written: Option<u64>, // Shared written blocks
    pub temp_read_blocks: Option<u64>,
    pub temp_written_blocks: Option<u64>,

    // Filters & Sorting & Joins
    pub filter: Option<String>,
    pub rows_removed_by_filter: Option<u64>,
    pub index_cond: Option<String>,
    pub hash_cond: Option<String>,
    pub join_type: Option<String>,
    pub sort_keys: Vec<String>,
    pub sort_method: Option<String>,
    pub sort_space_used: Option<u64>,
    pub sort_space_type: Option<String>, // e.g. "Disk", "Memory"

    // Intelligence & Warnings
    pub is_bottleneck: bool,
    pub warnings: Vec<ProfilerWarning>,

    // Hierarchy & extra metadata
    pub children: Vec<ExplainNode>,
    pub extra_properties: HashMap<String, String>,
}

impl Default for ExplainNode {
    fn default() -> Self {
        Self {
            id: 0,
            node_type: "Unknown Operation".to_string(),
            relation_name: None,
            schema_name: None,
            alias: None,
            index_name: None,
            startup_cost: 0.0,
            total_cost: 0.0,
            cost_percentage: 0.0,
            actual_startup_time: None,
            actual_total_time: None,
            time_percentage: 0.0,
            plan_rows: 0,
            plan_width: None,
            actual_rows: None,
            actual_loops: None,
            buffer_hit: None,
            buffer_read: None,
            buffer_dirtied: None,
            buffer_written: None,
            temp_read_blocks: None,
            temp_written_blocks: None,
            filter: None,
            rows_removed_by_filter: None,
            index_cond: None,
            hash_cond: None,
            join_type: None,
            sort_keys: Vec::new(),
            sort_method: None,
            sort_space_used: None,
            sort_space_type: None,
            is_bottleneck: false,
            warnings: Vec::new(),
            children: Vec::new(),
            extra_properties: HashMap::new(),
        }
    }
}

impl ExplainNode {
    pub fn max_cost(&self) -> f64 {
        let mut max_c = self.total_cost;
        for child in &self.children {
            max_c = max_c.max(child.max_cost());
        }
        max_c
    }

    pub fn max_duration(&self) -> f64 {
        let mut max_d = self.actual_total_time.unwrap_or(0.0);
        for child in &self.children {
            max_d = max_d.max(child.max_duration());
        }
        max_d
    }

    pub fn total_nodes_count(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(|c| c.total_nodes_count())
            .sum::<usize>()
    }

    pub fn count_warnings(&self) -> usize {
        self.warnings.len()
            + self
                .children
                .iter()
                .map(|c| c.count_warnings())
                .sum::<usize>()
    }

    pub fn total_buffer_hit(&self) -> u64 {
        self.buffer_hit.unwrap_or(0)
            + self
                .children
                .iter()
                .map(|c| c.total_buffer_hit())
                .sum::<u64>()
    }

    pub fn total_buffer_read(&self) -> u64 {
        self.buffer_read.unwrap_or(0)
            + self
                .children
                .iter()
                .map(|c| c.total_buffer_read())
                .sum::<u64>()
    }

    pub fn total_temp_written(&self) -> u64 {
        self.temp_written_blocks.unwrap_or(0)
            + self
                .children
                .iter()
                .map(|c| c.total_temp_written())
                .sum::<u64>()
    }

    pub fn has_disk_spill(&self) -> bool {
        if self.temp_written_blocks.unwrap_or(0) > 0
            || self.temp_read_blocks.unwrap_or(0) > 0
            || self
                .sort_space_type
                .as_ref()
                .is_some_and(|s| s.to_lowercase().contains("disk"))
        {
            return true;
        }
        self.children.iter().any(|c| c.has_disk_spill())
    }

    /// Total waktu node untuk semua loop (ms). PostgreSQL melaporkan waktu per loop.
    pub fn inclusive_time_ms(&self) -> Option<f64> {
        self.actual_total_time
            .map(|t| t * self.actual_loops.unwrap_or(1).max(1) as f64)
    }

    /// Cost milik node ini saja: total cost dikurangi total cost anak langsung.
    pub fn self_cost(&self) -> f64 {
        let children: f64 = self.children.iter().map(|c| c.total_cost).sum();
        (self.total_cost - children).max(0.0)
    }

    /// Waktu milik node ini saja (ms), tanpa waktu anak. None bila plan tanpa ANALYZE.
    pub fn self_time_ms(&self) -> Option<f64> {
        let own = self.inclusive_time_ms()?;
        let children: f64 = self
            .children
            .iter()
            .filter_map(|c| c.inclusive_time_ms())
            .sum();
        Some((own - children).max(0.0))
    }

    /// Baris keluaran node: aktual (dikali loop) bila ada, selain itu estimasi planner.
    pub fn output_rows(&self) -> u64 {
        match self.actual_rows {
            Some(r) => r.saturating_mul(self.actual_loops.unwrap_or(1).max(1)),
            None => self.plan_rows,
        }
    }

    /// Label ringkas untuk daftar dan chart: tipe node plus relasi/index bila ada.
    pub fn short_label(&self) -> String {
        match (&self.relation_name, &self.index_name) {
            (Some(rel), Some(idx)) => format!("{} on {} ({})", self.node_type, rel, idx),
            (Some(rel), None) => format!("{} on {}", self.node_type, rel),
            (None, Some(idx)) => format!("{} ({})", self.node_type, idx),
            (None, None) => self.node_type.clone(),
        }
    }

    /// Recursively collect all nodes in depth-first order
    pub fn collect_all_nodes<'a>(&'a self, list: &mut Vec<&'a ExplainNode>) {
        list.push(self);
        for child in &self.children {
            child.collect_all_nodes(list);
        }
    }

    /// Find a node by its ID
    pub fn find_node_by_id(&self, target_id: usize) -> Option<&ExplainNode> {
        if self.id == target_id {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_node_by_id(target_id) {
                return Some(found);
            }
        }
        None
    }
}

/// Aggregated query profile summary
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExplainSummary {
    pub engine: ProfilerEngine,
    pub total_cost: f64,
    pub total_duration_ms: f64,
    pub total_rows: u64,
    pub buffer_hit_total: u64,
    pub buffer_read_total: u64,
    pub buffer_hit_rate: f32, // percentage 0 - 100%
    pub temp_disk_spill_blocks: u64,
    pub bottlenecks_count: usize,
    pub warnings_count: usize,
}

impl ExplainSummary {
    pub fn from_root(root: &ExplainNode, engine: ProfilerEngine) -> Self {
        let total_cost = root.max_cost();
        let total_duration_ms = root.max_duration();
        let total_rows = root.actual_rows.unwrap_or(root.plan_rows);
        let buffer_hit_total = root.total_buffer_hit();
        let buffer_read_total = root.total_buffer_read();
        let total_buf = buffer_hit_total + buffer_read_total;
        let buffer_hit_rate = if total_buf > 0 {
            (buffer_hit_total as f32 / total_buf as f32) * 100.0
        } else {
            100.0
        };
        let temp_disk_spill_blocks = root.total_temp_written();
        let warnings_count = root.count_warnings();

        let mut all_nodes = Vec::new();
        root.collect_all_nodes(&mut all_nodes);
        let bottlenecks_count = all_nodes.iter().filter(|n| n.is_bottleneck).count();

        Self {
            engine,
            total_cost,
            total_duration_ms,
            total_rows,
            buffer_hit_total,
            buffer_read_total,
            buffer_hit_rate,
            temp_disk_spill_blocks,
            bottlenecks_count,
            warnings_count,
        }
    }
}

/// Metrik yang bisa ditampilkan pada bar chart EXPLAIN (C2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanMetric {
    #[default]
    SelfCost,
    SelfTime,
    Rows,
}

impl PlanMetric {
    pub const ALL: [PlanMetric; 3] = [Self::SelfCost, Self::SelfTime, Self::Rows];

    pub fn label(self) -> &'static str {
        match self {
            Self::SelfCost => "Self cost",
            Self::SelfTime => "Self time",
            Self::Rows => "Rows",
        }
    }

    /// Nilai metrik untuk satu node; None bila plan tidak memuat data tersebut.
    pub fn value(self, node: &ExplainNode) -> Option<f64> {
        match self {
            Self::SelfCost => Some(node.self_cost()),
            Self::SelfTime => node.self_time_ms(),
            Self::Rows => Some(node.output_rows() as f64),
        }
    }
}

/// Satu baris bar chart metrik: node dan nilainya, diurutkan menurun.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeMetricRow {
    pub node_id: usize,
    pub label: String,
    pub value: f64,
    pub share: f32, // 0.0 - 1.0 terhadap jumlah seluruh node
}

/// Menyusun baris bar chart untuk metrik tertentu, terbesar dulu.
pub fn metric_rows(root: &ExplainNode, metric: PlanMetric) -> Vec<NodeMetricRow> {
    let mut nodes = Vec::new();
    root.collect_all_nodes(&mut nodes);
    let mut rows: Vec<NodeMetricRow> = nodes
        .iter()
        .filter_map(|n| {
            metric.value(n).map(|v| NodeMetricRow {
                node_id: n.id,
                label: n.short_label(),
                value: v,
                share: 0.0,
            })
        })
        .collect();
    let total: f64 = rows.iter().map(|r| r.value).sum();
    for r in &mut rows {
        r.share = if total > 0.0 {
            (r.value / total) as f32
        } else {
            0.0
        };
    }
    rows.sort_by(|a, b| b.value.total_cmp(&a.value));
    rows
}

/// Main entry point to parse raw EXPLAIN output into a processed ExplainNode tree
pub fn parse_explain(raw_plan: &str) -> Option<(ExplainNode, ExplainSummary)> {
    let (mut root, engine) = parser::parse_explain_raw(raw_plan)?;

    // 1. Assign sequential IDs & calculate metrics percentages
    let mut next_id = 1;
    assign_node_ids(&mut root, &mut next_id);

    let max_cost = root.max_cost().max(0.0001);
    let max_duration = root.max_duration();
    calculate_percentages(&mut root, max_cost, max_duration);

    // 2. Query Intelligence: Analyze and generate warnings & detect bottlenecks
    warnings::analyze_tree(&mut root);

    // 3. Generate summary
    let summary = ExplainSummary::from_root(&root, engine);

    Some((root, summary))
}

fn assign_node_ids(node: &mut ExplainNode, next_id: &mut usize) {
    node.id = *next_id;
    *next_id += 1;
    for child in &mut node.children {
        assign_node_ids(child, next_id);
    }
}

fn calculate_percentages(node: &mut ExplainNode, max_cost: f64, max_duration: f64) {
    node.cost_percentage = if max_cost > 0.0 {
        ((node.total_cost / max_cost) as f32 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    node.time_percentage = if max_duration > 0.0 {
        ((node.actual_total_time.unwrap_or(0.0) / max_duration) as f32 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    for child in &mut node.children {
        calculate_percentages(child, max_cost, max_duration);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        cost: f64,
        time: Option<f64>,
        loops: Option<u64>,
        children: Vec<ExplainNode>,
    ) -> ExplainNode {
        ExplainNode {
            total_cost: cost,
            actual_total_time: time,
            actual_loops: loops,
            children,
            ..Default::default()
        }
    }

    #[test]
    fn self_cost_dan_self_time_mengurangi_anak() {
        let child = node(40.0, Some(2.0), Some(5), vec![]);
        let root = node(100.0, Some(15.0), Some(1), vec![child]);
        assert_eq!(root.self_cost(), 60.0);
        // Anak: 2 ms x 5 loop = 10 ms, sehingga root sendiri 5 ms.
        assert_eq!(root.self_time_ms(), Some(5.0));
        assert_eq!(root.children[0].self_time_ms(), Some(10.0));
    }

    #[test]
    fn metric_rows_urut_menurun_dan_share() {
        let mut root = node(100.0, None, None, vec![node(75.0, None, None, vec![])]);
        root.id = 1;
        root.children[0].id = 2;
        let rows = metric_rows(&root, PlanMetric::SelfCost);
        assert_eq!(rows[0].node_id, 2);
        assert_eq!(rows[0].value, 75.0);
        assert!((rows[0].share - 0.75).abs() < 1e-6);
        // Tanpa ANALYZE tidak ada baris self time.
        assert!(metric_rows(&root, PlanMetric::SelfTime).is_empty());
    }
}
