
#[derive(Debug, thiserror::Error)]
pub enum TierError {
    #[error("corrupt tiered value: {0}")]
    Corrupt(String),
    #[error("unknown codec tag {0:#04x}")]
    UnknownTag(u8),
}

pub type Result<T> = std::result::Result<T, TierError>;
