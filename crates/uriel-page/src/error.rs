use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageError {
    Parse(String),
    InputTooLarge { max: usize },
    Limit { what: &'static str, max: usize },
    UnsafeHref { field: &'static str },
    Shape(&'static str),
}

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageError::Parse(m) => write!(f, "invalid page spec: {m}"),
            PageError::InputTooLarge { max } => write!(f, "spec larger than {max} bytes"),
            PageError::Limit { what, max } => write!(f, "{what} exceeds the limit of {max}"),
            PageError::UnsafeHref { field } => write!(f, "{field} has a disallowed href"),
            PageError::Shape(m) => write!(f, "malformed spec: {m}"),
        }
    }
}

impl std::error::Error for PageError {}
