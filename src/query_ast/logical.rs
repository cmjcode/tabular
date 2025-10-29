//! Simplified logical plan used by rewrite + emission.

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column(String),
    StringLiteral(String),
    Number(String),
    BinaryOp {
        left: Box<Expr>,
        op: String,
        right: Box<Expr>,
    },
    FuncCall {
        name: String,
        args: Vec<Expr>,
    },
    Star,
    Alias {
        expr: Box<Expr>,
        alias: String,
    },
    Raw(String),
    Null,
    Boolean(bool),
    Not(Box<Expr>),
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        when_then: Vec<(Expr, Expr)>,
        else_expr: Option<Box<Expr>>,
    },
    Subquery {
        sql: String,
        correlated: bool,
    },
    WindowFunc {
        name: String,
        args: Vec<Expr>,
        partition_by: Vec<Expr>,
        order_by: Vec<(Expr, bool)>,
        frame: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortItem {
    pub expr: Expr,
    pub asc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalQueryPlan {
    Projection {
        exprs: Vec<Expr>,
        input: Box<LogicalQueryPlan>,
    },
    Distinct {
        input: Box<LogicalQueryPlan>,
    },
    Filter {
        predicate: Expr,
        input: Box<LogicalQueryPlan>,
    },
    Sort {
        items: Vec<SortItem>,
        input: Box<LogicalQueryPlan>,
    },
    Limit {
        limit: u64,
        offset: u64,
        input: Box<LogicalQueryPlan>,
    },
    Group {
        group_exprs: Vec<Expr>,
        input: Box<LogicalQueryPlan>,
    },
    Join {
        left: Box<LogicalQueryPlan>,
        right: Box<LogicalQueryPlan>,
        on: Option<Expr>,
        kind: JoinKind,
    },
    Having {
        predicate: Expr,
        input: Box<LogicalQueryPlan>,
    },
    // WITH ctes as (sql) ... <input>
    With {
        ctes: Vec<(String, String)>,
        input: Box<LogicalQueryPlan>,
    }, // (name, sql)
    // Set operations (currently only UNION / UNION ALL implemented)
    SetOp {
        left: Box<LogicalQueryPlan>,
        right: Box<LogicalQueryPlan>,
        op: SetOpKind,
    },
    TableScan {
        table: String,
        alias: Option<String>,
    },
    SubqueryScan {
        sql: String,
        alias: String,
        correlated: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SetOpKind {
    Union,
    UnionAll,
}

impl LogicalQueryPlan {
    pub fn table_scan(name: impl Into<String>) -> Self {
        Self::TableScan {
            table: name.into(),
            alias: None,
        }
    }
    pub fn subquery_scan(sql: impl Into<String>, alias: impl Into<String>) -> Self {
        Self::SubqueryScan {
            sql: sql.into(),
            alias: alias.into(),
            correlated: false,
        }
    }
}

impl Expr {
    pub fn is_simple_aggregate(&self) -> bool {
        match self {
            Expr::FuncCall { name, .. } => {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "count" | "sum" | "avg" | "min" | "max"
                )
            }
            _ => false,
        }
    }
}
