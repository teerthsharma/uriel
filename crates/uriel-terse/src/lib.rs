#![forbid(unsafe_code)]

pub mod data;
mod error;
mod json;
pub mod prose;
pub mod protected;
mod tokens;

pub use error::Error;
pub use json::Value;
pub use prose::{compact, Level};
pub use protected::{scan, Kind, Span};
pub use tokens::{estimate_tokens, Report, CHARS_PER_TOKEN};
