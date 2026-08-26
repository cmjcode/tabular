use super::{ExplainNode, ProfilerEngine};
use std::collections::HashMap;

/// Parses raw plan text into an ExplainNode tree and detected ProfilerEngine
pub fn parse_explain_raw(raw_plan: &str) -> Option<(ExplainNode, ProfilerEngine)> {
    let trimmed = raw_plan.trim();
    if trimmed.is_empty() {
        return None;
    }

    // 1. Try JSON formats (PostgreSQL / MySQL)
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(node) = parse_pg_json_root(&v) {
                return Some((node, ProfilerEngine::PostgreSQL));
            }
            if let Some(node) = parse_mysql_json_root(&v) {
                return Some((node, ProfilerEngine::MySQL));
            }
        }
    }

    // 2. Try MSSQL XML ShowPlan
    if trimmed.contains("<ShowPlanXML")
        || trimmed.contains("<StmtSimple")
        || trimmed.contains("<QueryPlan")
        || trimmed.contains("<RelOp")
    {
        if let Some(node) = parse_mssql_xml(trimmed) {
            return Some((node, ProfilerEngine::MSSQL));
        }
    }

    // 3. Try SQLite Query Plan text lines
    if trimmed.lines().any(|l| l.contains("SCAN ") || l.contains("SEARCH ")) {
        if let Some(node) = parse_sqlite_text(trimmed) {
            return Some((node, ProfilerEngine::SQLite));
        }
    }

    // 4. Fallback text parser (Postgres plain text or generic text)
    parse_generic_text(trimmed).map(|node| (node, ProfilerEngine::Generic))
}

// ─────────────────────────────────────────────────────────────────────────────
// PostgreSQL JSON Parser
// ─────────────────────────────────────────────────────────────────────────────

fn parse_pg_json_root(v: &serde_json::Value) -> Option<ExplainNode> {
    if let Some(arr) = v.as_array() {
        if let Some(first) = arr.first() {
            return parse_pg_json_root(first);
        }
    }

    if let Some(obj) = v.as_object() {
        if let Some(plan) = obj.get("Plan") {
            return parse_pg_plan_object(plan);
        }
        if obj.contains_key("Node Type") {
            return parse_pg_plan_object(v);
        }
    }

    None
}

fn parse_pg_plan_object(v: &serde_json::Value) -> Option<ExplainNode> {
    let node_type = v.get("Node Type")?.as_str()?.to_string();
    let relation_name = v.get("Relation Name").and_then(|s| s.as_str()).map(|s| s.to_string());
    let schema_name = v.get("Schema").and_then(|s| s.as_str()).map(|s| s.to_string());
    let alias = v.get("Alias").and_then(|s| s.as_str()).map(|s| s.to_string());
    let index_name = v.get("Index Name").and_then(|s| s.as_str()).map(|s| s.to_string());

    let startup_cost = v.get("Startup Cost").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let total_cost = v.get("Total Cost").and_then(|n| n.as_f64()).unwrap_or(0.0);
    let plan_rows = v.get("Plan Rows").and_then(|n| n.as_u64()).unwrap_or(0);
    let plan_width = v.get("Plan Width").and_then(|n| n.as_u64());

    let actual_startup_time = v.get("Actual Startup Time").and_then(|n| n.as_f64());
    let actual_total_time = v.get("Actual Total Time").and_then(|n| n.as_f64());
    let actual_rows = v.get("Actual Rows").and_then(|n| n.as_u64());
    let actual_loops = v.get("Actual Loops").and_then(|n| n.as_u64());

    // Buffer Stats (from EXPLAIN (ANALYZE, BUFFERS))
    let buffer_hit = v.get("Shared Hit Blocks").and_then(|n| n.as_u64());
    let buffer_read = v.get("Shared Read Blocks").and_then(|n| n.as_u64());
    let buffer_dirtied = v.get("Shared Dirtied Blocks").and_then(|n| n.as_u64());
    let buffer_written = v.get("Shared Written Blocks").and_then(|n| n.as_u64());
    let temp_read_blocks = v.get("Temp Read Blocks").and_then(|n| n.as_u64());
    let temp_written_blocks = v.get("Temp Written Blocks").and_then(|n| n.as_u64());

    // Filtering, sorting, joins
    let filter = v.get("Filter").and_then(|s| s.as_str()).map(|s| s.to_string());
    let rows_removed_by_filter = v.get("Rows Removed by Filter").and_then(|n| n.as_u64());
    let index_cond = v.get("Index Cond").and_then(|s| s.as_str()).map(|s| s.to_string());
    let hash_cond = v.get("Hash Cond").and_then(|s| s.as_str()).map(|s| s.to_string());
    let join_type = v.get("Join Type").and_then(|s| s.as_str()).map(|s| s.to_string());

    let mut sort_keys = Vec::new();
    if let Some(keys) = v.get("Sort Key").and_then(|k| k.as_array()) {
        for k in keys {
            if let Some(s) = k.as_str() {
                sort_keys.push(s.to_string());
            }
        }
    } else if let Some(key_str) = v.get("Sort Key").and_then(|k| k.as_str()) {
        sort_keys.push(key_str.to_string());
    }

    let sort_method = v.get("Sort Method").and_then(|s| s.as_str()).map(|s| s.to_string());
    let sort_space_used = v.get("Sort Space Used").and_then(|n| n.as_u64());
    let sort_space_type = v.get("Sort Space Type").and_then(|s| s.as_str()).map(|s| s.to_string());

    let mut extra_properties = HashMap::new();
    if let Some(strategy) = v.get("Strategy").and_then(|s| s.as_str()) {
        extra_properties.insert("Strategy".to_string(), strategy.to_string());
    }
    if let Some(parent_rel) = v.get("Parent Relationship").and_then(|s| s.as_str()) {
        extra_properties.insert("Parent Relationship".to_string(), parent_rel.to_string());
    }
    if let Some(hash_batches) = v.get("Hash Batches").and_then(|n| n.as_u64()) {
        extra_properties.insert("Hash Batches".to_string(), hash_batches.to_string());
    }

    // Children
    let mut children = Vec::new();
    if let Some(plans) = v.get("Plans").and_then(|p| p.as_array()) {
        for child_val in plans {
            if let Some(child_node) = parse_pg_plan_object(child_val) {
                children.push(child_node);
            }
        }
    }

    Some(ExplainNode {
        id: 0,
        node_type,
        relation_name,
        schema_name,
        alias,
        index_name,
        startup_cost,
        total_cost,
        cost_percentage: 0.0,
        actual_startup_time,
        actual_total_time,
        time_percentage: 0.0,
        plan_rows,
        plan_width,
        actual_rows,
        actual_loops,
        buffer_hit,
        buffer_read,
        buffer_dirtied,
        buffer_written,
        temp_read_blocks,
        temp_written_blocks,
        filter,
        rows_removed_by_filter,
        index_cond,
        hash_cond,
        join_type,
        sort_keys,
        sort_method,
        sort_space_used,
        sort_space_type,
        is_bottleneck: false,
        warnings: Vec::new(),
        children,
        extra_properties,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// MySQL JSON Parser (EXPLAIN FORMAT=JSON / EXPLAIN ANALYZE)
// ─────────────────────────────────────────────────────────────────────────────

fn parse_mysql_json_root(v: &serde_json::Value) -> Option<ExplainNode> {
    if let Some(arr) = v.as_array() {
        if let Some(first) = arr.first() {
            return parse_mysql_json_root(first);
        }
    }

    if let Some(obj) = v.as_object() {
        if let Some(qb) = obj.get("query_block") {
            return parse_mysql_query_block(qb);
        }
    }

    None
}

fn parse_mysql_query_block(v: &serde_json::Value) -> Option<ExplainNode> {
    let mut total_cost = 0.0;
    if let Some(cost) = v
        .get("cost_info")
        .and_then(|c| c.get("query_cost"))
        .and_then(|s| s.as_str())
        .and_then(|s| s.parse::<f64>().ok())
    {
        total_cost = cost;
    } else if let Some(cost) = v
        .get("cost_info")
        .and_then(|c| c.get("query_cost"))
        .and_then(|n| n.as_f64())
    {
        total_cost = cost;
    }

    let mut children = Vec::new();
    let mut node_type = "Query Block".to_string();

    // Check for nested loop join
    if let Some(nl) = v.get("nested_loop").and_then(|n| n.as_array()) {
        node_type = "Nested Loop Join".to_string();
        for item in nl {
            if let Some(t) = item.get("table") {
                if let Some(cn) = parse_mysql_table(t) {
                    children.push(cn);
                }
            } else if let Some(sub_qb) = item.get("query_block") {
                if let Some(cn) = parse_mysql_query_block(sub_qb) {
                    children.push(cn);
                }
            }
        }
    } else if let Some(t) = v.get("table") {
        return parse_mysql_table(t);
    } else if let Some(union_result) = v.get("union_result") {
        node_type = "UNION Result".to_string();
        if let Some(tbl_arr) = union_result.get("using_temporary_table").and_then(|_| v.get("table")) {
            if let Some(cn) = parse_mysql_table(tbl_arr) {
                children.push(cn);
            }
        }
    }

    Some(ExplainNode {
        id: 0,
        node_type,
        relation_name: None,
        schema_name: None,
        alias: None,
        index_name: None,
        startup_cost: 0.0,
        total_cost,
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
        children,
        extra_properties: HashMap::new(),
    })
}

fn parse_mysql_table(v: &serde_json::Value) -> Option<ExplainNode> {
    let table_name = v.get("table_name").and_then(|s| s.as_str()).map(|s| s.to_string());
    let access_type = v.get("access_type").and_then(|s| s.as_str()).unwrap_or("ALL");
    let key = v.get("key").and_then(|s| s.as_str()).map(|s| s.to_string());

    let node_type = match access_type {
        "ALL" => "Seq Scan (Full Table Scan)".to_string(),
        "index" => "Full Index Scan".to_string(),
        "range" => "Index Range Scan".to_string(),
        "ref" | "eq_ref" => "Index Lookup (Ref)".to_string(),
        "const" => "Const Index Lookup".to_string(),
        other => format!("{} Scan", other),
    };

    let rows_examined = v
        .get("rows_examined_per_scan")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);

    let mut cost = 0.0;
    if let Some(c) = v.get("cost_info") {
        if let Some(val) = c.get("prefix_cost").and_then(|s| s.as_str()).and_then(|s| s.parse::<f64>().ok()) {
            cost = val;
        } else if let Some(val) = c.get("read_cost").and_then(|s| s.as_str()).and_then(|s| s.parse::<f64>().ok()) {
            cost = val;
        } else if let Some(val) = c.get("prefix_cost").and_then(|n| n.as_f64()) {
            cost = val;
        }
    }

    let filter = v.get("attached_condition").and_then(|s| s.as_str()).map(|s| s.to_string());
    let mut extra_props = HashMap::new();
    if let Some(using_filesort) = v.get("using_filesort").and_then(|b| b.as_bool()) {
        if using_filesort {
            extra_props.insert("using_filesort".to_string(), "true".to_string());
        }
    }
    if let Some(using_temp) = v.get("using_temporary_table").and_then(|b| b.as_bool()) {
        if using_temp {
            extra_props.insert("using_temporary_table".to_string(), "true".to_string());
        }
    }

    // Materialized subqueries or nested scans
    let mut children = Vec::new();
    if let Some(materialized) = v.get("materialized_from_subquery") {
        if let Some(sub_qb) = materialized.get("query_block") {
            if let Some(sub_node) = parse_mysql_query_block(sub_qb) {
                children.push(sub_node);
            }
        }
    }

    Some(ExplainNode {
        id: 0,
        node_type,
        relation_name: table_name,
        schema_name: None,
        alias: None,
        index_name: key,
        startup_cost: 0.0,
        total_cost: cost,
        cost_percentage: 0.0,
        actual_startup_time: None,
        actual_total_time: None,
        time_percentage: 0.0,
        plan_rows: rows_examined,
        plan_width: None,
        actual_rows: None,
        actual_loops: None,
        buffer_hit: None,
        buffer_read: None,
        buffer_dirtied: None,
        buffer_written: None,
        temp_read_blocks: None,
        temp_written_blocks: None,
        filter,
        rows_removed_by_filter: None,
        index_cond: None,
        hash_cond: None,
        join_type: None,
        sort_keys: Vec::new(),
        sort_method: None,
        sort_space_used: None,
        sort_space_type: if extra_props.contains_key("using_temporary_table") {
            Some("Disk / Temporary Table".to_string())
        } else {
            None
        },
        is_bottleneck: false,
        warnings: Vec::new(),
        children,
        extra_properties: extra_props,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// MSSQL ShowPlan XML Parser
// ─────────────────────────────────────────────────────────────────────────────

fn parse_mssql_xml(xml: &str) -> Option<ExplainNode> {
    // Find the root RelOp or first statement query plan
    // We build an XML tag-matching parser resilient to nested attributes
    let rel_ops = extract_mssql_rel_ops(xml);
    if rel_ops.is_empty() {
        return None;
    }

    // Usually the first RelOp is the root of the query tree
    Some(rel_ops.into_iter().next().unwrap())
}

fn extract_mssql_rel_ops(xml: &str) -> Vec<ExplainNode> {
    let mut nodes = Vec::new();
    let mut cursor = 0;

    while let Some(start_idx) = xml[cursor..].find("<RelOp") {
        let actual_start = cursor + start_idx;
        // Find matching tag end or closing </RelOp>
        if let Some((node, next_cursor)) = parse_single_mssql_relop(&xml[actual_start..]) {
            nodes.push(node);
            cursor = actual_start + next_cursor;
        } else {
            cursor = actual_start + 6;
        }
    }

    nodes
}

fn parse_single_mssql_relop(slice: &str) -> Option<(ExplainNode, usize)> {
    if !slice.starts_with("<RelOp") {
        return None;
    }

    // Find end of <RelOp ...> header
    let header_end = slice.find('>')?;
    let header = &slice[..header_end];

    let physical_op = extract_xml_attr(header, "PhysicalOp").unwrap_or_else(|| "RelOp".to_string());
    let logical_op = extract_xml_attr(header, "LogicalOp");
    let estimate_rows = extract_xml_attr(header, "EstimateRows")
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0);
    let subtree_cost = extract_xml_attr(header, "EstimatedTotalSubtreeCost")
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let estimate_cpu = extract_xml_attr(header, "EstimateCPU").and_then(|s| s.parse::<f64>().ok());
    let estimate_io = extract_xml_attr(header, "EstimateIO").and_then(|s| s.parse::<f64>().ok());

    // Check if self-closing
    let (body, total_consumed) = if header.ends_with('/') {
        ("", header_end + 1)
    } else {
        // Find closing </RelOp> taking nesting into account
        let mut depth = 1;
        let mut idx = header_end + 1;
        let mut found_end = None;

        while idx < slice.len() {
            if slice[idx..].starts_with("<RelOp") {
                depth += 1;
                idx += 6;
            } else if slice[idx..].starts_with("</RelOp>") {
                depth -= 1;
                if depth == 0 {
                    found_end = Some(idx);
                    break;
                }
                idx += 8;
            } else {
                idx += 1;
            }
        }

        if let Some(end_pos) = found_end {
            (&slice[header_end + 1..end_pos], end_pos + 8)
        } else {
            (&slice[header_end + 1..], slice.len())
        }
    };

    // Extract object / table name
    let relation_name = extract_xml_attr(body, "Table")
        .or_else(|| extract_xml_attr(body, "Schema"))
        .or_else(|| {
            if let Some(pos) = body.find("<Object") {
                let obj_slice = &body[pos..];
                extract_xml_attr(obj_slice, "Table")
            } else {
                None
            }
        });

    let index_name = extract_xml_attr(body, "Index");

    // Extract Actual Execution Runtime info if present
    let actual_rows = extract_xml_attr(body, "ActualRows")
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64);
    let actual_elapsed_ms = extract_xml_attr(body, "ActualElapsedms")
        .and_then(|s| s.parse::<f64>().ok());

    let mut extra_props = HashMap::new();
    if let Some(log_op) = logical_op {
        extra_props.insert("LogicalOp".to_string(), log_op);
    }
    if let Some(cpu) = estimate_cpu {
        extra_props.insert("EstimateCPU".to_string(), format!("{:.5}", cpu));
    }
    if let Some(io) = estimate_io {
        extra_props.insert("EstimateIO".to_string(), format!("{:.5}", io));
    }

    // Recursively parse children inside the body
    let children = extract_mssql_rel_ops(body);

    let node = ExplainNode {
        id: 0,
        node_type: physical_op,
        relation_name,
        schema_name: None,
        alias: None,
        index_name,
        startup_cost: 0.0,
        total_cost: subtree_cost,
        cost_percentage: 0.0,
        actual_startup_time: None,
        actual_total_time: actual_elapsed_ms,
        time_percentage: 0.0,
        plan_rows: estimate_rows,
        plan_width: None,
        actual_rows,
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
        children,
        extra_properties: extra_props,
    };

    Some((node, total_consumed))
}

fn extract_xml_attr(text: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr_name);
    let start_idx = text.find(&pattern)? + pattern.len();
    let end_idx = text[start_idx..].find('"')?;
    let val = text[start_idx..start_idx + end_idx].trim();
    if val.is_empty() {
        None
    } else {
        // Clean out brackets e.g. [dbo].[Users] -> dbo.Users
        Some(val.replace('[', "").replace(']', ""))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SQLite & Generic Text Parsers
// ─────────────────────────────────────────────────────────────────────────────

fn parse_sqlite_text(text: &str) -> Option<ExplainNode> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return None;
    }

    let mut root = ExplainNode {
        id: 0,
        node_type: "Query Plan".to_string(),
        total_cost: 1.0,
        plan_rows: 1,
        ..Default::default()
    };

    for line in lines {
        let is_scan = line.contains("SCAN ");
        let is_search = line.contains("SEARCH ");
        let node_type = if is_search {
            "SEARCH (Index Scan)".to_string()
        } else if is_scan {
            "SCAN (Table Scan)".to_string()
        } else {
            line.to_string()
        };

        let mut rel = None;
        let mut idx = None;
        if let Some(pos) = line.find("TABLE ") {
            let rest = &line[pos + 6..];
            let name = rest.split_whitespace().next().unwrap_or_default();
            rel = Some(name.to_string());
        }
        if let Some(pos) = line.find("USING INDEX ") {
            let rest = &line[pos + 12..];
            let name = rest.split_whitespace().next().unwrap_or_default();
            idx = Some(name.to_string());
        }

        root.children.push(ExplainNode {
            id: 0,
            node_type,
            relation_name: rel,
            index_name: idx,
            total_cost: if is_scan { 10.0 } else { 1.0 },
            plan_rows: if is_scan { 1000 } else { 1 },
            ..Default::default()
        });
    }

    Some(root)
}

fn parse_generic_text(text: &str) -> Option<ExplainNode> {
    let lines: Vec<&str> = text.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return None;
    }

    let first_line = lines[0];
    let mut children = Vec::new();

    for line in &lines[1..] {
        children.push(ExplainNode {
            id: 0,
            node_type: line.to_string(),
            total_cost: 1.0,
            plan_rows: 1,
            ..Default::default()
        });
    }

    Some(ExplainNode {
        id: 0,
        node_type: first_line.to_string(),
        total_cost: 1.0,
        plan_rows: 1,
        children,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pg_json_with_buffers() {
        let json_sample = r#"
        [
          {
            "Plan": {
              "Node Type": "Seq Scan",
              "Relation Name": "orders",
              "Schema": "public",
              "Alias": "o",
              "Startup Cost": 0.00,
              "Total Cost": 1540.25,
              "Plan Rows": 50000,
              "Plan Width": 64,
              "Actual Startup Time": 0.052,
              "Actual Total Time": 24.120,
              "Actual Rows": 48200,
              "Actual Loops": 1,
              "Shared Hit Blocks": 340,
              "Shared Read Blocks": 1200,
              "Filter": "(status = 'PENDING')",
              "Rows Removed by Filter": 1800
            }
          }
        ]
        "#;

        let res = parse_explain_raw(json_sample);
        assert!(res.is_some());
        let (node, engine) = res.unwrap();
        assert_eq!(engine, ProfilerEngine::PostgreSQL);
        assert_eq!(node.node_type, "Seq Scan");
        assert_eq!(node.relation_name.as_deref(), Some("orders"));
        assert_eq!(node.total_cost, 1540.25);
        assert_eq!(node.buffer_hit, Some(340));
        assert_eq!(node.buffer_read, Some(1200));
        assert_eq!(node.actual_rows, Some(48200));
    }

    #[test]
    fn test_parse_mysql_json() {
        let json_sample = r#"
        {
          "query_block": {
            "select_id": 1,
            "cost_info": {
              "query_cost": "240.50"
            },
            "table": {
              "table_name": "users",
              "access_type": "ALL",
              "rows_examined_per_scan": 12500,
              "cost_info": {
                "prefix_cost": "240.50"
              },
              "attached_condition": "(`users`.`age` > 30)"
            }
          }
        }
        "#;

        let res = parse_explain_raw(json_sample);
        assert!(res.is_some());
        let (node, engine) = res.unwrap();
        assert_eq!(engine, ProfilerEngine::MySQL);
        assert_eq!(node.relation_name.as_deref(), Some("users"));
        assert!(node.node_type.contains("Seq Scan"));
        assert_eq!(node.plan_rows, 12500);
    }

    #[test]
    fn test_parse_mssql_xml() {
        let xml_sample = r#"
        <ShowPlanXML xmlns="http://schemas.microsoft.com/sqlserver/2004/07/showplan">
          <BatchSequence>
            <Batch>
              <Statements>
                <StmtSimple StatementText="SELECT * FROM Customers WHERE City = 'London'">
                  <QueryPlan>
                    <RelOp NodeId="0" PhysicalOp="Clustered Index Scan" LogicalOp="Clustered Index Scan" EstimateRows="120" EstimatedTotalSubtreeCost="0.045">
                      <IndexScan>
                        <Object Table="[dbo].[Customers]" Index="[PK_Customers]" />
                      </IndexScan>
                    </RelOp>
                  </QueryPlan>
                </StmtSimple>
              </Statements>
            </Batch>
          </BatchSequence>
        </ShowPlanXML>
        "#;

        let res = parse_explain_raw(xml_sample);
        assert!(res.is_some());
        let (node, engine) = res.unwrap();
        assert_eq!(engine, ProfilerEngine::MSSQL);
        assert_eq!(node.node_type, "Clustered Index Scan");
        assert_eq!(node.relation_name.as_deref(), Some("dbo.Customers"));
        assert_eq!(node.index_name.as_deref(), Some("PK_Customers"));
    }
}
