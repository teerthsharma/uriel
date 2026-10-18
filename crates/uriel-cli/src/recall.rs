use std::io::{BufRead, Write};
use std::path::Path;

use uriel_rag::Rag;

pub const DEFAULT_BUDGET: usize = 1200;
pub const MAX_BUDGET: usize = 8000;
pub const MAX_OUTPUT_BYTES: usize = 24 << 10;
pub const TAIL_BYTES: u64 = 256 << 10;
pub const QUERY_PART_CHARS: usize = 300;
pub const HITS_PER_QUERY: usize = 16;

pub fn tail_query<R: BufRead>(_reader: R, _cwd: Option<&Path>) -> String {
    String::new()
}

pub fn recall_json(
    _rag: &Rag,
    _session: &str,
    _query: &str,
    _budget: usize,
) -> Result<String, String> {
    Ok(String::new())
}

pub fn run<R: BufRead, W: Write>(
    _db: &Path,
    _budget: usize,
    _with_capture: bool,
    _input: R,
    _out: &mut W,
) {
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
