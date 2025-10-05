//! Thin wrapper structures around sqlparser AST that we normalize into LogicalQueryPlan.
//! Phase 1 keeps this minimal.

#[derive(Debug, Clone, PartialEq)]
pub struct SelectAst {
    pub projection: Vec<SelectItemAst>,
    pub from: Option<TableRefAst>,
    pub selection: Option<ExprAst>,
    pub order_by: Vec<OrderByExprAst>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItemAst {
    Expr { expr: ExprAst, alias: Option<String> },
    Wildcard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableRefAst {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderByExprAst {
    pub expr: ExprAst,
    pub asc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprAst {
    Column(String),
    StringLiteral(String),
    Number(String),
    BinaryOp { left: Box<ExprAst>, op: String, right: Box<ExprAst> },
    FuncCall { name: String, args: Vec<ExprAst> },
    Paren(Box<ExprAst>),
    // Fallback for unsupported/complex expressions; stored as raw string for emission.
    Raw(String),
}
