use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Syntax { at: usize, what: &'static str },
    TooDeep,
    InvalidNumber,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Syntax { at, what } => write!(f, "{what} at byte {at}"),
            Error::TooDeep => f.write_str("nesting too deep"),
            Error::InvalidNumber => f.write_str("invalid number literal"),
        }
    }
}

impl std::error::Error for Error {}
