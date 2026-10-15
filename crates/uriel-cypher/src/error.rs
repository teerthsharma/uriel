//! Parse errors with byte spans and line/column positions.

use thiserror::Error;

/// A half-open byte range `[start, end)` into the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    /// Byte offset of the first byte.
    pub start: usize,
    /// Byte offset one past the last byte.
    pub end: usize,
}

/// Error returned by [`crate::parse`].
///
/// `line` and `col` are 1-based; `col` counts Unicode scalar values from the
/// start of the line.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message} (line {line}, column {col})")]
pub struct ParseError {
    /// Human-readable description of what went wrong.
    pub message: String,
    /// Byte span of the offending text (empty at end of input).
    pub span: Span,
    /// 1-based line number of `span.start`.
    pub line: usize,
    /// 1-based column (in characters) of `span.start`.
    pub col: usize,
}

impl ParseError {
    /// Builds an error for `src[start..end]`, computing line/column.
    pub fn new(src: &str, start: usize, end: usize, message: impl Into<String>) -> Self {
        let start = start.min(src.len());
        let end = end.clamp(start, src.len());
        let (line, col) = line_col(src, start);
        ParseError {
            message: message.into(),
            span: Span { start, end },
            line,
            col,
        }
    }
}

/// Computes the 1-based (line, column) of byte offset `pos` in `src`.
pub fn line_col(src: &str, pos: usize) -> (usize, usize) {
    let mut pos = pos.min(src.len());
    while !src.is_char_boundary(pos) {
        pos -= 1;
    }
    let before = &src[..pos];
    let line = before.matches('\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col = before[line_start..].chars().count() + 1;
    (line, col)
}
