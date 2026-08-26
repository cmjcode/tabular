use super::ExplainNode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WarningSeverity {
    Info,
    Medium,
    High,
    Critical,
}

impl WarningSeverity {
    pub fn badge_color(&self) -> (u8, u8, u8) {
        match self {
            Self::Critical => (244, 67, 54),   // Bright Red
            Self::High => (255, 152, 0),       // Orange
            Self::Medium => (255, 193, 7),     // Amber / Yellow
            Self::Info => (33, 150, 243),      // Blue
        }
    }

    pub fn title_prefix(&self) -> &'static str {
        match self {
            Self::Critical => "🛑 CRITICAL",
            Self::High => "⚠️ HIGH",
            Self::Medium => "⚡ WARNING",
            Self::Info => "💡 INFO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningCategory {
    SequentialScan,
    CartesianProduct,
    DiskSpill,
    EstimationMismatch,
    BufferMiss,
    FilterDiscard,
    MissingIndex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilerWarning {
    pub severity: WarningSeverity,
    pub category: WarningCategory,
    pub title: String,
    pub description: String,
    pub recommendation: Option<String>,
}

/// Recursively inspect the node tree and attach warnings + mark bottleneck nodes
pub fn analyze_tree(root: &mut ExplainNode) {
    let max_cost = root.max_cost();
    let max_duration = root.max_duration();

    // 1. Mark bottlenecks: any node with >= 40% of total cost or duration, or top single node
    mark_bottlenecks(root, max_cost, max_duration);

    // 2. Generate warnings per node
    inspect_node_warnings(root);
}

fn mark_bottlenecks(node: &mut ExplainNode, max_cost: f64, max_duration: f64) {
    let cost_ratio = if max_cost > 0.0 {
        node.total_cost / max_cost
    } else {
        0.0
    };
    let time_ratio = if max_duration > 0.0 {
        node.actual_total_time.unwrap_or(0.0) / max_duration
    } else {
        0.0
    };

    // If node takes > 40% of cost or runtime, flag as bottleneck
    if cost_ratio >= 0.40 || time_ratio >= 0.40 {
        node.is_bottleneck = true;
    }

    for child in &mut node.children {
        mark_bottlenecks(child, max_cost, max_duration);
    }
}

fn inspect_node_warnings(node: &mut ExplainNode) {
    let node_type_lower = node.node_type.to_lowercase();
    let rows = node.actual_rows.unwrap_or(node.plan_rows);

    // ─── 1. Sequential Scan on Large Table Warning ──────────────────────────────
    let is_seq_scan = node_type_lower.contains("seq scan")
        || node_type_lower.contains("full table scan")
        || node_type_lower.contains("table scan")
        || (node_type_lower.contains("scan") && !node_type_lower.contains("index"));

    if is_seq_scan && rows >= 500 {
        let table_name = node.relation_name.as_deref().unwrap_or("target table");
        let severity = if rows >= 10000 || node.cost_percentage >= 50.0 {
            WarningSeverity::Critical
        } else if rows >= 2000 {
            WarningSeverity::High
        } else {
            WarningSeverity::Medium
        };

        let filter_hint = node
            .filter
            .as_deref()
            .map(|f| format!(" Filter predicate: `{}`.", f))
            .unwrap_or_default();

        let rec = if let Some(filter) = &node.filter {
            format!(
                "Consider adding an index on `{}` for columns referenced in `{}`.",
                table_name, filter
            )
        } else {
            format!(
                "Add a targeted INDEX or primary key on `{}` to avoid scanning all {} rows.",
                table_name, rows
            )
        };

        node.warnings.push(ProfilerWarning {
            severity,
            category: WarningCategory::SequentialScan,
            title: format!("Sequential Scan on '{}' ({} rows)", table_name, rows),
            description: format!(
                "A full table sequential scan was performed on table '{}', reading {} rows.{}\
                 Full table scans on growing datasets degrade concurrency and saturate disk I/O.",
                table_name, rows, filter_hint
            ),
            recommendation: Some(rec),
        });
    }

    // ─── 2. Cartesian Product / Unbounded Join Warning ──────────────────────────
    let is_join = node_type_lower.contains("join") || node_type_lower.contains("nested loop");
    if is_join {
        let has_no_condition = node.hash_cond.is_none() && node.index_cond.is_none() && node.filter.is_none();
        if has_no_condition && rows > 1000 {
            node.warnings.push(ProfilerWarning {
                severity: WarningSeverity::Critical,
                category: WarningCategory::CartesianProduct,
                title: "Potential Cartesian Product (Cross Join)".to_string(),
                description: format!(
                    "Join operator '{}' has no join filter/hash condition, producing {} output rows. \
                     This may result in an unintended M x N Cartesian product.",
                    node.node_type, rows
                ),
                recommendation: Some(
                    "Ensure explicit ON / WHERE join conditions connect both relations on foreign keys."
                        .to_string(),
                ),
            });
        }
    }

    // ─── 3. Disk Spill & Temporary File Warning ────────────────────────────────
    let temp_written = node.temp_written_blocks.unwrap_or(0);
    let is_disk_sort = node
        .sort_space_type
        .as_ref()
        .is_some_and(|s| s.to_lowercase().contains("disk"));
    let is_mysql_temp = node
        .extra_properties
        .get("using_temporary_table")
        .is_some_and(|v| v == "true");

    if temp_written > 0 || is_disk_sort || is_mysql_temp {
        let severity = if temp_written > 1000 || is_disk_sort {
            WarningSeverity::High
        } else {
            WarningSeverity::Medium
        };

        let detail = if is_disk_sort {
            format!(
                "Sort operator spilled to disk using {} kB.",
                node.sort_space_used.unwrap_or(0)
            )
        } else if temp_written > 0 {
            format!("Spilled {} temporary disk blocks to storage.", temp_written)
        } else {
            "Operation created an on-disk temporary table.".to_string()
        };

        node.warnings.push(ProfilerWarning {
            severity,
            category: WarningCategory::DiskSpill,
            title: "Memory Limit Exceeded (Disk Spill)".to_string(),
            description: format!(
                "{}. Disk I/O is orders of magnitude slower than in-memory operations.",
                detail
            ),
            recommendation: Some(
                "Increase `work_mem` (PostgreSQL), `sort_buffer_size` (MySQL), or reduce sort/group-by column width."
                    .to_string(),
            ),
        });
    }

    // ─── 4. Stale Statistics / Estimation Mismatch Warning ─────────────────────
    if let (Some(actual), plan) = (node.actual_rows, node.plan_rows) {
        if plan > 0 && actual > 0 {
            let ratio = (actual as f64) / (plan as f64);
            if ratio >= 10.0 || ratio <= 0.1 {
                let severity = if ratio >= 100.0 || ratio <= 0.01 {
                    WarningSeverity::High
                } else {
                    WarningSeverity::Medium
                };

                node.warnings.push(ProfilerWarning {
                    severity,
                    category: WarningCategory::EstimationMismatch,
                    title: format!(
                        "Severe Row Estimation Mismatch ({:.1}x difference)",
                        if ratio > 1.0 { ratio } else { 1.0 / ratio }
                    ),
                    description: format!(
                        "Query planner estimated {} rows but execution produced {} rows ({:.1}x error). \
                         Inaccurate cardinality statistics cause the optimizer to choose suboptimal join algorithms and scan types.",
                        plan, actual, ratio
                    ),
                    recommendation: Some(
                        format!(
                            "Update table statistics by running `ANALYZE {};` or rebuild indexes.",
                            node.relation_name.as_deref().unwrap_or("affected_table")
                        ),
                    ),
                });
            }
        }
    }

    // ─── 5. High Filter Discard Ratio Warning ──────────────────────────────────
    if let (Some(removed), Some(actual)) = (node.rows_removed_by_filter, node.actual_rows) {
        let total_scanned = removed + actual;
        if total_scanned > 500 {
            let discard_pct = (removed as f64 / total_scanned as f64) * 100.0;
            if discard_pct >= 75.0 {
                node.warnings.push(ProfilerWarning {
                    severity: WarningSeverity::Medium,
                    category: WarningCategory::FilterDiscard,
                    title: format!("High Filter Discard Rate ({:.1}%)", discard_pct),
                    description: format!(
                        "Out of {} scanned rows, {} rows ({:.1}%) were discarded by post-scan filter `{}`.",
                        total_scanned, removed, discard_pct, node.filter.as_deref().unwrap_or("")
                    ),
                    recommendation: Some(
                        "Create a composite index or partial index covering the filter predicate so non-matching rows are pruned at index level."
                            .to_string(),
                    ),
                });
            }
        }
    }

    // ─── 6. Buffer Cache Miss Warning ──────────────────────────────────────────
    if let (Some(hit), Some(read)) = (node.buffer_hit, node.buffer_read) {
        let total_buf = hit + read;
        if total_buf > 200 {
            let hit_rate = (hit as f64 / total_buf as f64) * 100.0;
            if hit_rate < 80.0 {
                node.warnings.push(ProfilerWarning {
                    severity: WarningSeverity::Medium,
                    category: WarningCategory::BufferMiss,
                    title: format!("Low Buffer Cache Hit Ratio ({:.1}%)", hit_rate),
                    description: format!(
                        "Read {} blocks directly from disk vs {} blocks from memory cache (hit rate: {:.1}%).",
                        read, hit, hit_rate
                    ),
                    recommendation: Some(
                        "Consider increasing `shared_buffers` (PostgreSQL) or `innodb_buffer_pool_size` (MySQL) to keep working sets in RAM."
                            .to_string(),
                    ),
                });
            }
        }
    }

    // Recurse into children
    for child in &mut node.children {
        inspect_node_warnings(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sequential_scan_warning() {
        let mut node = ExplainNode {
            node_type: "Seq Scan".to_string(),
            relation_name: Some("large_orders".to_string()),
            plan_rows: 25000,
            actual_rows: Some(25000),
            total_cost: 4500.0,
            filter: Some("status = 'PENDING'".to_string()),
            ..Default::default()
        };

        analyze_tree(&mut node);
        assert!(node.is_bottleneck);
        assert!(!node.warnings.is_empty());
        let seq_warn = node
            .warnings
            .iter()
            .find(|w| w.category == WarningCategory::SequentialScan);
        assert!(seq_warn.is_some());
        assert_eq!(seq_warn.unwrap().severity, WarningSeverity::Critical);
    }

    #[test]
    fn test_disk_spill_warning() {
        let mut node = ExplainNode {
            node_type: "Sort".to_string(),
            sort_space_type: Some("Disk".to_string()),
            sort_space_used: Some(8192),
            temp_written_blocks: Some(1024),
            total_cost: 120.0,
            ..Default::default()
        };

        analyze_tree(&mut node);
        let disk_warn = node
            .warnings
            .iter()
            .find(|w| w.category == WarningCategory::DiskSpill);
        assert!(disk_warn.is_some());
        assert_eq!(disk_warn.unwrap().severity, WarningSeverity::High);
    }

    #[test]
    fn test_estimation_mismatch_warning() {
        let mut node = ExplainNode {
            node_type: "Index Scan".to_string(),
            plan_rows: 5,
            actual_rows: Some(1500),
            total_cost: 10.0,
            ..Default::default()
        };

        analyze_tree(&mut node);
        let mismatch_warn = node
            .warnings
            .iter()
            .find(|w| w.category == WarningCategory::EstimationMismatch);
        assert!(mismatch_warn.is_some());
    }
}

