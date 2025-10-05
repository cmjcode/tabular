#[derive(thiserror::Error, Debug)]
pub enum QueryAstError {
    #[error("parse error: {0}")] Parse(String),
    #[error("unsupported feature: {0}")] Unsupported(&'static str),
    #[error("emit error: {0}")] Emit(String),
}

#[derive(thiserror::Error, Debug)]
pub enum RewriteError {
    #[error("rewrite error: {0}")] Generic(String),
}

impl From<RewriteError> for QueryAstError { fn from(e: RewriteError) -> Self { QueryAstError::Emit(e.to_string()) } }
