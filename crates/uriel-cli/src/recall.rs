use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde_json::json;
use uriel_rag::{Hit, Rag, SearchOptions};

use crate::capture::{
    capture_into, head_chars, one_line, open_db, open_read_only, prune_stale, read_payload,
    resolve_session, scan_file, session_key, summary_head, walk, Event, Payload, MAX_WRITES,
};

pub const DEFAULT_BUDGET: usize = 1200;
pub const MAX_BUDGET: usize = 8000;
pub const MAX_OUTPUT_BYTES: usize = 24 << 10;
pub const TAIL_BYTES: u64 = 256 << 10;
pub const QUERY_PART_CHARS: usize = 300;
pub const QUERY_PROMPTS: usize = 3;
pub const QUERY_SUMMARY_CHARS: usize = 1200;
pub const HITS_PER_QUERY: usize = 16;
pub const DEFAULT_K: usize = 8;
pub const MAX_K: usize = 64;
const SUMMARY_HITS: usize = 2;
const SUMMARY_PIN_DEPTH: usize = 16;
const CWD_CHARS: usize = 100;

fn header(db: Option<&Path>) -> String {
    let at = db.map_or(String::new(), |d| format!(" --db {}", d.display()));
    format!("uriel recall (earlier context; zoom a hit: uriel recall{at} --expand <chunk_id>):\n")
}

pub fn tail_query<R: BufRead>(reader: R, cwd: Option<&Path>) -> String {
    let mut prompts: VecDeque<String> = VecDeque::new();
    let mut summary: Option<String> = None;
    walk(reader, TAIL_BYTES, |event| {
        if let Event::Entry(e) = event {
            if e.summary {
                summary = Some(summary_head(&e.text).to_owned());
            } else if let Some(p) = e.prompt.filter(|p| !p.starts_with('<')) {
                if prompts.len() == QUERY_PROMPTS {
                    prompts.pop_front();
                }
                prompts.push_back(p);
            }
        }
    });
    let name = cwd
        .and_then(Path::file_name)
        .and_then(std::ffi::OsStr::to_str)
        .map(|n| head_chars(n, CWD_CHARS).to_owned());
    let parts: Vec<String> = match summary.filter(|s| !s.is_empty()) {
        Some(s) => vec![head_chars(&s, QUERY_SUMMARY_CHARS).to_owned()],
        None => prompts
            .iter()
            .map(|p| head_chars(p, QUERY_PART_CHARS).to_owned())
            .collect(),
    };
    parts.into_iter().chain(name).collect::<Vec<_>>().join(" ")
}

fn summary_number(file: &str, prefix: &str) -> Option<usize> {
    file.strip_prefix(prefix)?.strip_suffix(".md")?.parse().ok()
}

fn is_ctx(hit: &Hit) -> bool {
    hit.file.starts_with("ctx/")
}

#[cfg(test)]
pub fn recall_json(rag: &Rag, session: &str, query: &str, budget: usize) -> Result<String, String> {
    recall_json_at(rag, session, query, budget, None)
}

pub fn recall_json_at(
    rag: &Rag,
    session: &str,
    query: &str,
    budget: usize,
    db: Option<&Path>,
) -> Result<String, String> {
    let query = query.trim();
    let header = header(db);
    let header_tokens = header.chars().count().div_ceil(4);
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
    let prefix = format!("ctx/{key}/summary-");
    let finder = SearchOptions {
        k: SUMMARY_PIN_DEPTH,
        ..topical.clone()
    };
    let mut hits: Vec<Hit> = rag
        .search(&format!("ctx-summary session {key}"), &finder)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|h| summary_number(&h.file, &prefix).is_some())
        .collect();
    hits.sort_by(|a, b| {
        summary_number(&b.file, &prefix)
            .cmp(&summary_number(&a.file, &prefix))
            .then(a.start_line.cmp(&b.start_line))
    });
    hits.truncate(SUMMARY_HITS);
    let mut seen: HashSet<u64> = hits.iter().map(|h| h.chunk_id).collect();
    for h in rag.search(query, &topical).map_err(|e| e.to_string())? {
        if is_ctx(&h) && seen.insert(h.chunk_id) {
            hits.push(h);
        }
    }
    if hits.is_empty() {
        return Ok("{}".to_owned());
    }
    let pack = rag
        .ladder_pack(hits, budget - header_tokens)
        .map_err(|e| e.to_string())?;
    if pack.hits.is_empty() {
        return Ok("{}".to_owned());
    }
    let mut context = format!("{header}{}", pack.text);
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

pub fn recall_text(rag: &Rag, query: &str, budget: usize, k: usize) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(String::new());
    }
    let budget = budget.clamp(1, MAX_BUDGET);
    let opts = SearchOptions {
        k: k.clamp(1, MAX_K),
        budget_tokens: budget,
        expand_hops: 0,
        use_utility: true,
    };
    let hits = rag.search(query, &opts).map_err(|e| e.to_string())?;
    if hits.is_empty() {
        return Ok(String::new());
    }
    Ok(capped(rag.ladder_pack(hits, budget).map_err(|e| e.to_string())?.text))
}

pub fn expand_text(rag: &Rag, chunk_id: u64) -> Result<String, String> {
    rag.expand(chunk_id).map(capped).map_err(|e| e.to_string())
}

fn capped(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_BYTES {
        let mut cut = MAX_OUTPUT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    text
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
    let (rag, graph) = open_db(db)?;
    if let Some(s) = &scanned {
        capture_into(&rag, &session, s, MAX_WRITES)?;
        prune_stale(&rag, &graph, &session, s)?;
    }
    recall_json_at(&rag, &session, &query, budget, Some(db))
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

fn search_inner(
    db: &Path,
    budget: usize,
    k: usize,
    query: &str,
    expand: Option<u64>,
) -> Result<String, String> {
    if !db.exists() {
        return Err(format!("no database at {}", db.display()));
    }
    let rag = open_read_only(db)?;
    match expand {
        Some(id) => expand_text(&rag, id),
        None => recall_text(&rag, query, budget, k),
    }
}

pub fn run_search<W: Write>(
    db: &Path,
    budget: usize,
    k: usize,
    query: &str,
    expand: Option<u64>,
    out: &mut W,
) {
    match search_inner(db, budget, k, query, expand) {
        Ok(text) if text.is_empty() => eprintln!("uriel recall: no match"),
        Ok(text) => {
            if let Err(e) = out.write_all(text.as_bytes()) {
                eprintln!("uriel recall: {}", one_line(&e.to_string()));
            }
        }
        Err(e) => eprintln!("uriel recall: {}", one_line(&e)),
    }
}

#[cfg(test)]
#[path = "recall_tests.rs"]
mod tests;
