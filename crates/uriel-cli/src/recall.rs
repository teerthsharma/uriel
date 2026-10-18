use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::json;
use uriel_rag::{Hit, Rag, SearchOptions};

use crate::capture::{
    capture_into, head_chars, one_line, open_rag, read_payload, resolve_session, scan_file,
    session_key, walk, Event, Payload, MAX_WRITES, SUMMARY_PREFIX,
};

pub const DEFAULT_BUDGET: usize = 1200;
pub const MAX_BUDGET: usize = 8000;
pub const MAX_OUTPUT_BYTES: usize = 24 << 10;
pub const TAIL_BYTES: u64 = 256 << 10;
pub const QUERY_PART_CHARS: usize = 300;
pub const HITS_PER_QUERY: usize = 16;
const SUMMARY_HITS: usize = 4;
const CWD_CHARS: usize = 100;
const HEADER: &str = "uriel recall (earlier context; zoom a hit with rag_expand chunk_id):\n";

fn summary_head(text: &str) -> &str {
    let body = text.strip_prefix(SUMMARY_PREFIX).unwrap_or(text);
    body.split_once("Summary:").map_or(body, |x| x.1).trim()
}

pub fn tail_query<R: BufRead>(reader: R, cwd: Option<&Path>) -> String {
    let mut prompt: Option<String> = None;
    let mut summary: Option<String> = None;
    walk(reader, TAIL_BYTES, |event| {
        if let Event::Entry(e) = event {
            if e.summary {
                summary = Some(summary_head(&e.text).to_owned());
            } else if let Some(p) = e.prompt {
                prompt = Some(p);
            }
        }
    });
    let name = cwd
        .and_then(Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .map(|n| head_chars(n, CWD_CHARS).to_owned());
    [
        prompt.map(|p| head_chars(&p, QUERY_PART_CHARS).to_owned()),
        summary.map(|s| head_chars(&s, QUERY_PART_CHARS).to_owned()),
        name,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

fn is_ctx(hit: &Hit) -> bool {
    hit.file.starts_with("ctx/")
}

pub fn recall_json(rag: &Rag, session: &str, query: &str, budget: usize) -> Result<String, String> {
    let query = query.trim();
    let header_tokens = HEADER.chars().count().div_ceil(4);
    let budget = budget.clamp(1, MAX_BUDGET);
    if query.is_empty() || budget <= header_tokens {
        return Ok("{}".to_owned());
    }
    let key = session_key(session);
    let topical = SearchOptions {
        k: HITS_PER_QUERY,
        budget_tokens: budget,
        expand_hops: 0,
        use_utility: true,
    };
    let pinned = SearchOptions {
        k: SUMMARY_HITS,
        ..topical.clone()
    };
    let mut found = rag
        .search(&format!("ctx-summary session {key}"), &pinned)
        .map_err(|e| e.to_string())?;
    found.extend(rag.search(query, &topical).map_err(|e| e.to_string())?);
    let prefix = format!("ctx/{key}/summary-");
    let mut seen = HashSet::new();
    found.retain(|h| is_ctx(h) && seen.insert(h.chunk_id));
    let (mut hits, rest): (Vec<Hit>, Vec<Hit>) =
        found.into_iter().partition(|h| h.file.starts_with(&prefix));
    hits.extend(rest);
    if hits.is_empty() {
        return Ok("{}".to_owned());
    }
    let pack = rag
        .ladder_pack(hits, budget - header_tokens)
        .map_err(|e| e.to_string())?;
    if pack.hits.is_empty() {
        return Ok("{}".to_owned());
    }
    let mut context = format!("{HEADER}{}", pack.text);
    if context.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while !context.is_char_boundary(cut) {
            cut -= 1;
        }
        context.truncate(cut);
    }
    Ok(json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}})
        .to_string())
}

fn read_tail(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut reader = BufReader::new(file);
    if start > 0 {
        let mut skipped = Vec::new();
        let mut bounded = (&mut reader).take(TAIL_BYTES);
        bounded.read_until(b'\n', &mut skipped).ok()?;
    }
    let mut bytes = Vec::new();
    reader
        .take(TAIL_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn run_inner<R: BufRead>(
    db: &Path,
    budget: usize,
    with_capture: bool,
    input: R,
) -> Result<String, String> {
    let payload: Payload = read_payload(input).unwrap_or_default();
    let transcript = payload.transcript_path.clone();
    let session = transcript
        .as_deref()
        .map(|t| resolve_session(None, &payload, t))
        .or_else(|| payload.session_id.clone())
        .unwrap_or_default();
    let scanned = match (&transcript, with_capture) {
        (Some(t), true) => Some(scan_file(t, &session)?),
        _ => None,
    };
    if scanned.is_none() && !db.exists() {
        return Ok("{}".to_owned());
    }
    let query = transcript
        .as_deref()
        .and_then(read_tail)
        .map(|bytes| tail_query(bytes.as_slice(), payload.cwd.as_deref()))
        .unwrap_or_else(|| tail_query(std::io::empty(), payload.cwd.as_deref()));
    let rag = open_rag(db)?;
    if let Some(s) = &scanned {
        capture_into(&rag, &session, s, MAX_WRITES)?;
    }
    recall_json(&rag, &session, &query, budget)
}

pub fn run<R: BufRead, W: Write>(db: &Path, budget: usize, with_capture: bool, input: R, out: &mut W) {
    let json = run_inner(db, budget, with_capture, input).unwrap_or_else(|e| {
        eprintln!("uriel recall-context: {}", one_line(&e));
        "{}".to_owned()
    });
    if let Err(e) = writeln!(out, "{json}") {
        eprintln!("uriel recall-context: {}", one_line(&e.to_string()));
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
