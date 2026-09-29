//! Perbandingan dua plan EXPLAIN (checklist C3).
//!
//! Node dicocokkan lewat jalur struktural (tipe node + relasi + posisi anak) sehingga plan
//! yang bentuknya sama menghasilkan delta per node, sedangkan perubahan bentuk tampil sebagai
//! node Added/Removed.

use super::ExplainNode;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Status sebuah node setelah dibandingkan dengan baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaStatus {
    Same,
    Changed,
    Added,
    Removed,
}

/// Metrik satu node yang dipakai untuk perbandingan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeMetrics {
    pub total_cost: f64,
    pub self_cost: f64,
    pub time_ms: Option<f64>,
    pub rows: u64,
}

impl NodeMetrics {
    fn of(node: &ExplainNode) -> Self {
        Self {
            total_cost: node.total_cost,
            self_cost: node.self_cost(),
            time_ms: node.inclusive_time_ms(),
            rows: node.output_rows(),
        }
    }
}

/// Satu baris hasil diff, urut sesuai pohon plan saat ini (node Removed di akhir).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeDelta {
    pub depth: usize,
    pub label: String,
    pub status: DeltaStatus,
    pub base: Option<NodeMetrics>,
    pub current: Option<NodeMetrics>,
}

impl NodeDelta {
    pub fn cost_delta(&self) -> Option<f64> {
        Some(self.current?.total_cost - self.base?.total_cost)
    }

    pub fn time_delta(&self) -> Option<f64> {
        Some(self.current?.time_ms? - self.base?.time_ms?)
    }

    pub fn rows_delta(&self) -> Option<i64> {
        Some(self.current?.rows as i64 - self.base?.rows as i64)
    }
}

/// Ringkasan perbandingan dua plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanDiff {
    pub rows: Vec<NodeDelta>,
    pub base_cost: f64,
    pub current_cost: f64,
    pub base_time_ms: Option<f64>,
    pub current_time_ms: Option<f64>,
    /// True bila ada node yang hilang atau muncul (bentuk plan berubah).
    pub shape_changed: bool,
}

fn node_key(node: &ExplainNode) -> String {
    format!(
        "{}|{}|{}",
        node.node_type,
        node.relation_name.as_deref().unwrap_or(""),
        node.index_name.as_deref().unwrap_or("")
    )
}

struct FlatNode<'a> {
    path: String,
    depth: usize,
    node: &'a ExplainNode,
}

fn flatten<'a>(
    node: &'a ExplainNode,
    parent: &str,
    idx: usize,
    depth: usize,
    out: &mut Vec<FlatNode<'a>>,
) {
    let path = format!("{parent}/{idx}:{}", node_key(node));
    out.push(FlatNode {
        path: path.clone(),
        depth,
        node,
    });
    for (i, c) in node.children.iter().enumerate() {
        flatten(c, &path, i, depth + 1, out);
    }
}

/// Membandingkan plan saat ini dengan baseline.
pub fn diff_plans(base: &ExplainNode, current: &ExplainNode) -> PlanDiff {
    let mut base_flat = Vec::new();
    flatten(base, "", 0, 0, &mut base_flat);
    let mut cur_flat = Vec::new();
    flatten(current, "", 0, 0, &mut cur_flat);

    let mut base_by_path: HashMap<&str, usize> = HashMap::new();
    for (i, f) in base_flat.iter().enumerate() {
        base_by_path.insert(f.path.as_str(), i);
    }
    let mut used = vec![false; base_flat.len()];

    let mut rows = Vec::with_capacity(cur_flat.len());
    for f in &cur_flat {
        let cur = NodeMetrics::of(f.node);
        match base_by_path.get(f.path.as_str()) {
            Some(&bi) => {
                used[bi] = true;
                let base = NodeMetrics::of(base_flat[bi].node);
                let changed = (base.total_cost - cur.total_cost).abs() > 1e-9
                    || base.rows != cur.rows
                    || base.time_ms != cur.time_ms;
                rows.push(NodeDelta {
                    depth: f.depth,
                    label: f.node.short_label(),
                    status: if changed {
                        DeltaStatus::Changed
                    } else {
                        DeltaStatus::Same
                    },
                    base: Some(base),
                    current: Some(cur),
                });
            }
            None => rows.push(NodeDelta {
                depth: f.depth,
                label: f.node.short_label(),
                status: DeltaStatus::Added,
                base: None,
                current: Some(cur),
            }),
        }
    }
    for (i, f) in base_flat.iter().enumerate() {
        if !used[i] {
            rows.push(NodeDelta {
                depth: f.depth,
                label: f.node.short_label(),
                status: DeltaStatus::Removed,
                base: Some(NodeMetrics::of(f.node)),
                current: None,
            });
        }
    }

    let shape_changed = rows
        .iter()
        .any(|r| matches!(r.status, DeltaStatus::Added | DeltaStatus::Removed));
    PlanDiff {
        rows,
        base_cost: base.max_cost(),
        current_cost: current.max_cost(),
        base_time_ms: base.actual_total_time.map(|_| base.max_duration()),
        current_time_ms: current.actual_total_time.map(|_| current.max_duration()),
        shape_changed,
    }
}

/// Membuang prefix EXPLAIN/SHOWPLAN sehingga `EXPLAIN SELECT ..` dan
/// `EXPLAIN (ANALYZE, FORMAT JSON) SELECT ..` punya fingerprint yang sama.
pub fn strip_explain_prefix(sql: &str) -> &str {
    const OPTION_WORDS: [&str; 12] = [
        "analyze",
        "analyse",
        "verbose",
        "query",
        "plan",
        "extended",
        "partitions",
        "format",
        "json",
        "tree",
        "traditional",
        "buffers",
    ];
    let mut rest = sql.trim_start();

    // SQL Server: "SET SHOWPLAN_XML ON; ..." atau "SET STATISTICS XML ON; ...".
    loop {
        let upper: String = rest
            .chars()
            .take(32)
            .collect::<String>()
            .to_ascii_uppercase();
        if upper.starts_with("SET SHOWPLAN") || upper.starts_with("SET STATISTICS") {
            match rest.find(';').or_else(|| rest.find('\n')) {
                Some(p) => rest = rest[p + 1..].trim_start(),
                None => return rest,
            }
        } else {
            break;
        }
    }

    let starts_with_word = |s: &str, w: &str| {
        s.len() >= w.len()
            && s[..w.len()].eq_ignore_ascii_case(w)
            && s[w.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
    };
    if !starts_with_word(rest, "explain") {
        return rest;
    }
    rest = rest["explain".len()..].trim_start();
    loop {
        if rest.starts_with('(') {
            let mut depth = 0i32;
            let mut end = None;
            for (i, c) in rest.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match end {
                Some(e) => rest = rest[e + 1..].trim_start(),
                None => return rest,
            }
            continue;
        }
        // "FORMAT=JSON" gaya MySQL.
        if rest.len() >= 7 && rest[..7].eq_ignore_ascii_case("format=") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            rest = rest[end..].trim_start();
            continue;
        }
        match OPTION_WORDS.iter().find(|w| starts_with_word(rest, w)) {
            Some(w) => rest = rest[w.len()..].trim_start(),
            None => break,
        }
    }
    rest
}

/// True bila `sql` diawali EXPLAIN atau SET SHOWPLAN/STATISTICS (SQL Server).
pub fn is_explain_query(sql: &str) -> bool {
    strip_explain_prefix(sql).len() != sql.trim_start().len()
}

/// Fingerprint stabil untuk query yang di-EXPLAIN: tanpa prefix EXPLAIN, spasi diringkas,
/// huruf kecil, tanpa titik koma di akhir. Hasil berupa 16 digit hex SHA-256.
pub fn query_fingerprint(sql: &str) -> String {
    let body = strip_explain_prefix(sql);
    let normalized = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(';')
        .trim()
        .to_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(t: &str, rel: &str, cost: f64, rows: u64) -> ExplainNode {
        ExplainNode {
            node_type: t.into(),
            relation_name: Some(rel.into()),
            total_cost: cost,
            plan_rows: rows,
            ..Default::default()
        }
    }

    #[test]
    fn fingerprint_mengabaikan_opsi_explain_dan_spasi() {
        let a = query_fingerprint("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT *  FROM t;");
        let b = query_fingerprint("explain select * from t");
        let c = query_fingerprint("EXPLAIN FORMAT=JSON SELECT * FROM t");
        let d = query_fingerprint("EXPLAIN QUERY PLAN SELECT * FROM t");
        let e = query_fingerprint("SET SHOWPLAN_XML ON;\nSELECT * FROM t");
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
        assert_eq!(a, e);
        assert_ne!(a, query_fingerprint("SELECT * FROM u"));
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn deteksi_query_explain() {
        assert!(is_explain_query("  explain analyze select 1"));
        assert!(is_explain_query("SET STATISTICS XML ON; SELECT 1"));
        assert!(!is_explain_query("SELECT '{\"a\":1}'"));
    }

    #[test]
    fn strip_prefix_tidak_memotong_identifier() {
        assert_eq!(strip_explain_prefix("explained_view"), "explained_view");
        assert_eq!(
            strip_explain_prefix("EXPLAIN ANALYZE SELECT analyze_col FROM x"),
            "SELECT analyze_col FROM x"
        );
    }

    #[test]
    fn diff_mendeteksi_perubahan_dan_bentuk() {
        let base = ExplainNode {
            node_type: "Hash Join".into(),
            total_cost: 200.0,
            children: vec![
                leaf("Seq Scan", "users", 100.0, 1000),
                leaf("Seq Scan", "orders", 50.0, 10),
            ],
            ..Default::default()
        };
        let current = ExplainNode {
            node_type: "Hash Join".into(),
            total_cost: 80.0,
            children: vec![
                leaf("Index Scan", "users", 20.0, 1000),
                leaf("Seq Scan", "orders", 50.0, 10),
            ],
            ..Default::default()
        };
        let d = diff_plans(&base, &current);
        assert!(d.shape_changed);
        assert_eq!(d.rows[0].status, DeltaStatus::Changed);
        assert_eq!(d.rows[0].cost_delta(), Some(-120.0));
        assert_eq!(d.rows[1].status, DeltaStatus::Added);
        assert_eq!(d.rows[2].status, DeltaStatus::Same);
        assert_eq!(d.rows.last().map(|r| r.status), Some(DeltaStatus::Removed));
        assert_eq!(d.base_cost, 200.0);
        assert_eq!(d.current_cost, 80.0);
        assert_eq!(d.current_time_ms, None);
    }
}
