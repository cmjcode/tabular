use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod graph;
pub mod parser;
pub mod warnings;

pub use graph::render_query_profiler;
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
    pub actual_total_time: Option<f64>,   // ms
    pub time_percentage: f32, // 0.0 - 100.0% relative to total execution time

    // Rows & cardinality
    pub plan_rows: u64,
    pub plan_width: Option<u64>,
    pub actual_rows: Option<u64>,
    pub actual_loops: Option<u64>,

    // Buffer & I/O statistics
    pub buffer_hit: Option<u64>,      // Shared hit blocks
    pub buffer_read: Option<u64>,     // Shared read blocks
    pub buffer_dirtied: Option<u64>,  // Shared dirtied blocks
    pub buffer_written: Option<u64>,  // Shared written blocks
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
        1 + self.children.iter().map(|c| c.total_nodes_count()).sum::<usize>()
    }

    pub fn count_warnings(&self) -> usize {
        self.warnings.len() + self.children.iter().map(|c| c.count_warnings()).sum::<usize>()
    }

    pub fn total_buffer_hit(&self) -> u64 {
        self.buffer_hit.unwrap_or(0) + self.children.iter().map(|c| c.total_buffer_hit()).sum::<u64>()
    }

    pub fn total_buffer_read(&self) -> u64 {
        self.buffer_read.unwrap_or(0) + self.children.iter().map(|c| c.total_buffer_read()).sum::<u64>()
    }

    pub fn total_temp_written(&self) -> u64 {
        self.temp_written_blocks.unwrap_or(0)
            + self.children.iter().map(|c| c.total_temp_written()).sum::<u64>()
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
