//! Error type of the query engine.

/// Everything that can go wrong while parsing or executing a query.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The query text is not valid Cypher.
    #[error("syntax error: {0}")]
    Parse(#[from] uriel_cypher::ParseError),
    /// A read-only execution contained a clause that writes.
    #[error("read-only access mode forbids write operation: {0}")]
    ReadOnlyViolation(String),
    /// Storage layer failure (constraint violation, I/O, ...).
    #[error("{0}")]
    Core(#[from] uriel_core::Error),
    /// Static problem: unknown variable, unknown function, bad argument.
    #[error("semantic error: {0}")]
    Semantic(String),
    /// Dynamic problem: type mismatch, division by zero, overflow.
    #[error("runtime error: {0}")]
    Runtime(String),
    /// Valid Cypher that this engine does not implement.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// Result cache failure.
    #[error("cache error: {0}")]
    Cache(String),
}

pub(crate) fn sem<T>(msg: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError::Semantic(msg.into()))
}

pub(crate) fn rt<T>(msg: impl Into<String>) -> Result<T, QueryError> {
    Err(QueryError::Runtime(msg.into()))
}

pub(crate) type QResult<T> = Result<T, QueryError>;
