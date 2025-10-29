#[derive(thiserror::Error, Debug)]
pub enum QueryAstError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("unsupported feature: {0}")]
    Unsupported(&'static str),

    #[error("emit error: {0}")]
    Emit(String),

    #[error("semantic error: {0}")]
    Semantic(String),

    #[error("execution error: {query} - {reason}")]
    Execution { query: String, reason: String },

    #[error("type mismatch: expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },

    #[error("database type {db_type} not supported for feature: {feature}")]
    DatabaseFeatureUnsupported { db_type: String, feature: String },
}

#[derive(thiserror::Error, Debug)]
pub enum RewriteError {
    #[error("rewrite error: {0}")]
    Generic(String),

    #[error("infinite rewrite loop detected: {0}")]
    InfiniteLoop(String),

    #[error("invalid plan after rewrite: {0}")]
    InvalidPlan(String),
}

impl From<RewriteError> for QueryAstError {
    fn from(e: RewriteError) -> Self {
        QueryAstError::Emit(e.to_string())
    }
}
