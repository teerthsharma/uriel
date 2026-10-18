#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("truncated input: needed {needed} bytes, {available} available")]
    Truncated { needed: usize, available: usize },
    #[error("frame of {len} bytes exceeds the limit of {max}")]
    FrameTooLarge { len: usize, max: usize },
    #[error("frame length prefix {declared} does not match {actual} bytes present")]
    LengthMismatch { declared: usize, actual: usize },
    #[error("empty frame")]
    EmptyFrame,
    #[error("unknown message type {0:#04x}")]
    UnknownMessage(u8),
    #[error("unknown tag {0}")]
    UnknownTag(u8),
    #[error("string is not valid utf-8")]
    InvalidUtf8,
    #[error("value nesting deeper than {0}")]
    TooDeep(usize),
    #[error("{0} trailing bytes after message")]
    Trailing(usize),
    #[error("declared count {count} cannot fit in {available} remaining bytes")]
    CountOverflow { count: usize, available: usize },
    #[error("integer {0} does not fit this platform")]
    IntegerOverflow(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Query = 1,
    Search = 2,
    Malformed = 3,
    ShuttingDown = 4,
    TooLarge = 5,
    Internal = 6,
    Index = 7,
}

impl ErrorCode {
    pub fn from_wire(byte: u8) -> Result<ErrorCode, CodecError> {
        match byte {
            1 => Ok(ErrorCode::Query),
            2 => Ok(ErrorCode::Search),
            3 => Ok(ErrorCode::Malformed),
            4 => Ok(ErrorCode::ShuttingDown),
            5 => Ok(ErrorCode::TooLarge),
            6 => Ok(ErrorCode::Internal),
            7 => Ok(ErrorCode::Index),
            other => Err(CodecError::UnknownTag(other)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("protocol error: {0}")]
    Codec(#[from] CodecError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("hot arena error: {0}")]
    Arena(#[from] uriel_sys::SysError),
    #[error("query error: {0}")]
    Query(#[from] uriel_query::QueryError),
    #[error("search error: {0}")]
    Search(#[from] uriel_rag::RagError),
    #[error("invalid index request: {0}")]
    InvalidIndex(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("connection closed")]
    Closed,
    #[error("timed out waiting for a frame")]
    Timeout,
    #[error("server queue is full")]
    Busy,
    #[error("host is shutting down")]
    ShuttingDown,
    #[error("host is already serving")]
    AlreadyServing,
    #[error("server reported {code:?}: {message}")]
    Remote { code: ErrorCode, message: String },
    #[error("unexpected response: {0}")]
    Unexpected(&'static str),
}
