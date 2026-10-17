#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("shape mismatch: {0}")]
    Shape(String),
    #[error("non-finite value: {0}")]
    NonFinite(String),
    #[error("duplicate stable id {0}")]
    DuplicateId(u64),
    #[error("distance overflow: coordinates too large for f64 distances")]
    Overflow,
    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("index out of range: {0}")]
    OutOfRange(String),
    #[error("uriel-algo: {0}")]
    Algo(String),
    #[error("malformed csr export: {0}")]
    Malformed(String),
    #[error("io: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<uriel_algo::Error> for Error {
    fn from(e: uriel_algo::Error) -> Self {
        Error::Algo(e.to_string())
    }
}
