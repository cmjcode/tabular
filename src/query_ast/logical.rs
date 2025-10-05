//! Simplified logical plan used by rewrite + emission.

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Column(String),
    StringLiteral(String),
    Number(String),
    BinaryOp { left: Box<Expr>, op: String, right: Box<Expr> },
    FuncCall { name: String, args: Vec<Expr> },
    Star,
    Alias { expr: Box<Expr>, alias: String },
    Raw(String),
    Null,
    Boolean(bool),
    Not(Box<Expr>),
    IsNull { expr: Box<Expr>, negated: bool },
    Like { expr: Box<Expr>, pattern: Box<Expr>, negated: bool },
    InList { expr: Box<Expr>, list: Vec<Expr>, negated: bool },
    Case { operand: Option<Box<Expr>>, when_then: Vec<(Expr, Expr)>, else_expr: Option<Box<Expr>> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SortItem { pub expr: Expr, pub asc: bool }

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalQueryPlan {
    Projection { exprs: Vec<Expr>, input: Box<LogicalQueryPlan> },
    Distinct { input: Box<LogicalQueryPlan> },
    Filter { predicate: Expr, input: Box<LogicalQueryPlan> },
    Sort { items: Vec<SortItem>, input: Box<LogicalQueryPlan> },
    Limit { limit: u64, offset: u64, input: Box<LogicalQueryPlan> },
    Group { group_exprs: Vec<Expr>, input: Box<LogicalQueryPlan> },
    Join { left: Box<LogicalQueryPlan>, right: Box<LogicalQueryPlan>, on: Option<Expr>, kind: JoinKind },
    Having { predicate: Expr, input: Box<LogicalQueryPlan> },
    TableScan { table: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum JoinKind { Inner, Left, Right, Full }

impl LogicalQueryPlan {
    pub fn table_scan(name: impl Into<String>) -> Self { Self::TableScan { table: name.into() } }
}

impl Expr {
    pub fn is_simple_aggregate(&self) -> bool {
        match self {
            Expr::FuncCall { name, .. } => {
                matches!(name.to_ascii_lowercase().as_str(), "count" | "sum" | "avg" | "min" | "max")
            }
            _ => false,
        }
    }
}
