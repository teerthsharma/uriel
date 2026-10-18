use std::io::BufRead;
use std::path::{Path, PathBuf};

use uriel_rag::Rag;

pub const CHARS_PER_TOKEN: usize = 4;
pub const FRACTION_TOKENS: usize = 600;
pub const OVERLAP_TOKENS: usize = 60;
pub const TOOL_RESULT_CAP_CHARS: usize = 600;
pub const TOOL_INPUT_CAP_CHARS: usize = 160;
pub const MAX_LINE_BYTES: usize = 2 << 20;
pub const MAX_READ_BYTES: u64 = 512 << 20;
pub const MAX_FRACTIONS: usize = 8192;
pub const MAX_WRITES: usize = 256;
pub const MAX_SUMMARIES: usize = 16;
pub const MAX_SUMMARY_CHARS: usize = 64_000;
pub const MAX_PAYLOAD_BYTES: u64 = 1 << 20;
pub const SUMMARY_PREFIX: &str = "This session is being continued from a previous conversation";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Payload {
    pub session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub event: Option<String>,
    pub trigger: Option<String>,
    pub source: Option<String>,
    pub cwd: Option<PathBuf>,
}

pub fn parse_payload(_bytes: &[u8]) -> Option<Payload> {
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub text: String,
    pub ts: String,
    pub summary: bool,
    pub prompt: Option<String>,
}

pub fn entry_of(_line: &[u8]) -> Option<Entry> {
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fraction {
    pub index: usize,
    pub first_turn: u64,
    pub last_turn: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub n: usize,
    pub text: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Scan {
    pub fractions: Vec<Fraction>,
    pub summaries: Vec<Summary>,
    pub bad_lines: u64,
    pub oversize_lines: u64,
    pub truncated: bool,
}

pub fn scan<R: BufRead>(reader: R, session: &str) -> Scan {
    scan_bounded(reader, session, MAX_FRACTIONS, MAX_READ_BYTES)
}

pub fn scan_bounded<R: BufRead>(
    _reader: R,
    _session: &str,
    _max_fractions: usize,
    _max_bytes: u64,
) -> Scan {
    Scan::default()
}

pub fn session_key(_raw: &str) -> String {
    String::new()
}

pub fn fraction_source(_session: &str, _index: usize) -> String {
    String::new()
}

pub fn summary_source(_session: &str, _n: usize) -> String {
    String::new()
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub fractions: usize,
    pub written: usize,
    pub unchanged: usize,
    pub summaries: usize,
}

pub fn capture_into(
    _rag: &Rag,
    _session: &str,
    _scan: &Scan,
    _max_writes: usize,
) -> Result<Report, String> {
    Ok(Report::default())
}

pub fn run<R: BufRead>(
    _db: &Path,
    _session: Option<String>,
    _transcript: Option<PathBuf>,
    _input: R,
) {
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
