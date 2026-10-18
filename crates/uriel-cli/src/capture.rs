use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;
use uriel_core::{Graph, MemoryBudget};
use uriel_rag::{Rag, RagConfig};

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
pub const TOPIC_CHARS: usize = 120;
pub const MIN_TOPIC_CHARS: usize = 16;
const TOPIC_SCAN_CHARS: usize = 4 * TOPIC_CHARS;
pub const SUMMARY_PREFIX: &str = "This session is being continued from a previous conversation";
const NL: char = '\n';
const READ_BUFFER_BYTES: usize = 64 << 10;
const MAX_SESSION_KEY_CHARS: usize = 64;
const MAX_TS_CHARS: usize = 40;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Payload {
    pub session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub event: Option<String>,
    pub trigger: Option<String>,
    pub source: Option<String>,
    pub cwd: Option<PathBuf>,
}

pub fn parse_payload(bytes: &[u8]) -> Option<Payload> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let o = v.as_object()?;
    let field = |k: &str| {
        o.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    Some(Payload {
        session_id: field("session_id"),
        transcript_path: field("transcript_path").map(PathBuf::from),
        event: field("hook_event_name"),
        trigger: field("trigger"),
        source: field("source"),
        cwd: field("cwd").map(PathBuf::from),
    })
}

pub fn read_payload<R: Read>(input: R) -> Option<Payload> {
    let mut bytes = Vec::new();
    input.take(MAX_PAYLOAD_BYTES).read_to_end(&mut bytes).ok()?;
    parse_payload(&bytes)
}

pub fn head_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

pub fn summary_head(text: &str) -> &str {
    let body = text.strip_prefix(SUMMARY_PREFIX).unwrap_or(text);
    body.split_once("Summary:").map_or(body, |x| x.1).trim()
}

fn cut_word(s: &str, n: usize) -> String {
    let head = head_chars(s, n);
    if head.len() == s.len() {
        return s.to_owned();
    }
    match head.rfind(' ') {
        Some(i) if head[..i].chars().count() >= n * 3 / 4 => head[..i].to_owned(),
        _ => head.to_owned(),
    }
}

pub fn topic_of(s: &str) -> Option<String> {
    let flat = one_line(head_chars(s, TOPIC_SCAN_CHARS));
    if flat.starts_with('<') || flat.chars().count() < MIN_TOPIC_CHARS {
        return None;
    }
    Some(cut_word(&flat, TOPIC_CHARS))
}

fn truncate_chars(s: &str, n: usize) -> String {
    let head = head_chars(s, n);
    if head.len() < s.len() {
        format!("{head}...")
    } else {
        s.to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub text: String,
    pub ts: String,
    pub summary: bool,
    pub prompt: Option<String>,
    pub topic: Option<String>,
}

fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.trim().to_owned(),
        Value::Array(items) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn clean_ts(v: &Value) -> String {
    v.get("timestamp")
        .and_then(Value::as_str)
        .filter(|t| {
            !t.is_empty()
                && t.chars().count() <= MAX_TS_CHARS
                && !t.chars().any(|c| c.is_whitespace() || c.is_control())
        })
        .map_or_else(|| "-".to_owned(), str::to_owned)
}

pub fn entry_from_value(v: &Value) -> Option<Entry> {
    let kind = v.get("type")?.as_str()?;
    if kind != "user" && kind != "assistant" {
        return None;
    }
    if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let mut lines: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut note_text = |s: &str, lines: &mut Vec<String>| {
        let t = s.trim();
        if !t.is_empty() {
            lines.push(format!("[{kind}] {t}"));
            texts.push(t.to_owned());
        }
    };
    match content {
        Value::String(s) => note_text(s, &mut lines),
        Value::Array(blocks) => {
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(s) = b.get("text").and_then(Value::as_str) {
                            note_text(s, &mut lines);
                        }
                    }
                    Some("tool_use") => {
                        if let Some(name) = b.get("name").and_then(Value::as_str) {
                            let input = b.get("input").map(Value::to_string).unwrap_or_default();
                            let summary = truncate_chars(&input, TOOL_INPUT_CAP_CHARS);
                            lines.push(format!("[tool_use] {name} {summary}").trim_end().to_owned());
                        }
                    }
                    Some("tool_result") => {
                        let text = b.get("content").map(tool_result_text).unwrap_or_default();
                        if !text.is_empty() {
                            lines.push(format!(
                                "[tool_result] {}",
                                truncate_chars(&text, TOOL_RESULT_CAP_CHARS)
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if lines.is_empty() {
        return None;
    }
    let is_user = kind == "user";
    let flagged = is_user && v.get("isCompactSummary").and_then(Value::as_bool) == Some(true);
    let summary =
        flagged || (is_user && texts.first().is_some_and(|t| t.starts_with(SUMMARY_PREFIX)));
    let joined = texts.join("\n");
    let topic = if summary { None } else { texts.iter().find_map(|t| topic_of(t)) };
    Some(Entry {
        text: if summary { joined.clone() } else { lines.join("\n") },
        ts: clean_ts(v),
        summary,
        prompt: (is_user && !summary && !texts.is_empty()).then_some(joined),
        topic,
    })
}

#[cfg(test)]
pub fn entry_of(line: &[u8]) -> Option<Entry> {
    let text = String::from_utf8_lossy(line);
    let v: Value = serde_json::from_str(&text).ok()?;
    entry_from_value(&v)
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

pub enum Event {
    Entry(Entry),
    Bad,
    Oversize,
}

enum LineKind {
    Eof,
    Line,
    Oversize,
}

fn read_line<R: BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<(LineKind, u64)> {
    buf.clear();
    let mut consumed = 0u64;
    let mut over = false;
    loop {
        let avail = match r.fill_buf() {
            Ok(a) => a,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if avail.is_empty() {
            break;
        }
        let (take, done) = match avail.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (avail.len(), false),
        };
        if !over {
            let payload = take - usize::from(done);
            if buf.len().saturating_add(payload) > cap {
                over = true;
                buf.clear();
                buf.shrink_to(0);
            } else {
                buf.extend_from_slice(&avail[..payload]);
            }
        }
        r.consume(take);
        consumed = consumed.saturating_add(take as u64);
        if done {
            break;
        }
    }
    let kind = if consumed == 0 {
        LineKind::Eof
    } else if over {
        LineKind::Oversize
    } else {
        LineKind::Line
    };
    Ok((kind, consumed))
}

pub fn walk<R: BufRead, F: FnMut(Event)>(mut reader: R, max_bytes: u64, mut on: F) -> bool {
    let mut buf: Vec<u8> = Vec::new();
    let mut read = 0u64;
    loop {
        if read >= max_bytes {
            return reader.fill_buf().is_ok_and(|b| !b.is_empty());
        }
        let (kind, n) = match read_line(&mut reader, &mut buf, MAX_LINE_BYTES) {
            Ok(x) => x,
            Err(_) => return true,
        };
        read = read.saturating_add(n);
        match kind {
            LineKind::Eof => return false,
            LineKind::Oversize => on(Event::Oversize),
            LineKind::Line => {
                while buf.last().is_some_and(u8::is_ascii_whitespace) {
                    buf.pop();
                }
                if buf.is_empty() {
                    continue;
                }
                let text = String::from_utf8_lossy(&buf);
                match serde_json::from_str::<Value>(&text) {
                    Ok(v) => {
                        if let Some(e) = entry_from_value(&v) {
                            on(Event::Entry(e));
                        }
                    }
                    Err(_) => on(Event::Bad),
                }
            }
        }
    }
}

pub fn session_key(raw: &str) -> String {
    let key: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_SESSION_KEY_CHARS)
        .collect();
    if key.chars().all(|c| c == '.') {
        "unknown".to_owned()
    } else {
        key
    }
}

pub fn fraction_source(session: &str, index: usize) -> String {
    format!("ctx/{session}/{index}.md")
}

pub fn summary_source(session: &str, n: usize) -> String {
    format!("ctx/{session}/summary-{n}.md")
}

struct Builder {
    session: String,
    cap: usize,
    overlap: usize,
    max_fractions: usize,
    index: usize,
    turn: u64,
    body: String,
    body_chars: usize,
    fresh: bool,
    first_turn: u64,
    last_turn: u64,
    first_ts: String,
    last_ts: String,
    topic: Option<String>,
    out: VecDeque<Fraction>,
    evicted: bool,
}

fn split_fit(s: &str, room: usize) -> (&str, &str) {
    let cut = s.char_indices().nth(room).map_or(s.len(), |x| x.0);
    let window = &s[..cut];
    let at = match window.rfind(char::is_whitespace) {
        Some(j) if window[..j].chars().count() >= room / 2 => j,
        _ => cut,
    };
    (&s[..at], s[at..].trim_start())
}

fn carry_of(body: &str, max_chars: usize) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut total = 0usize;
    for line in body.rsplit(NL) {
        let cost = line.chars().count() + usize::from(!kept.is_empty());
        if total.saturating_add(cost) > max_chars {
            break;
        }
        total += cost;
        kept.push(line);
    }
    if kept.is_empty() {
        let last = body.rsplit(NL).next().unwrap_or("");
        let skip = last.chars().count().saturating_sub(max_chars);
        let start = last.char_indices().nth(skip).map_or(last.len(), |x| x.0);
        return last[start..].trim_start().to_owned();
    }
    kept.reverse();
    kept.join("\n")
}

impl Builder {
    fn new(session: &str, max_fractions: usize) -> Builder {
        Builder {
            session: session.to_owned(),
            cap: FRACTION_TOKENS * CHARS_PER_TOKEN,
            overlap: OVERLAP_TOKENS * CHARS_PER_TOKEN,
            max_fractions: max_fractions.max(1),
            index: 0,
            turn: 0,
            body: String::new(),
            body_chars: 0,
            fresh: false,
            first_turn: 0,
            last_turn: 0,
            first_ts: "-".to_owned(),
            last_ts: "-".to_owned(),
            topic: None,
            out: VecDeque::new(),
            evicted: false,
        }
    }

    fn push_entry(&mut self, text: &str, ts: &str, topic: Option<&str>) {
        let turn = self.turn;
        self.turn = self.turn.saturating_add(1);
        let mut rest = text;
        let mut lead = topic.map(str::to_owned);
        let mut first = true;
        loop {
            let len = rest.chars().count();
            let sep = usize::from(self.body_chars > 0);
            if self.body_chars.saturating_add(sep).saturating_add(len) <= self.cap {
                let t = if first { lead.take() } else { topic_of(rest) };
                self.append(turn, ts, rest, sep, t);
                return;
            }
            if self.fresh {
                self.close();
                continue;
            }
            let room = self.cap.saturating_sub(self.body_chars + sep).max(1);
            let (head, tail) = split_fit(rest, room);
            let t = if first { lead.take() } else { topic_of(head) };
            self.append(turn, ts, head, sep, t);
            first = false;
            self.close();
            if tail.is_empty() {
                return;
            }
            rest = tail;
        }
    }

    fn append(&mut self, turn: u64, ts: &str, s: &str, sep: usize, topic: Option<String>) {
        if self.topic.is_none() {
            self.topic = topic;
        }
        if self.body_chars == 0 {
            self.first_turn = turn;
            self.first_ts = ts.to_owned();
        }
        if sep == 1 {
            self.body.push('\n');
        }
        self.body.push_str(s);
        self.body_chars = self
            .body_chars
            .saturating_add(sep)
            .saturating_add(s.chars().count());
        self.last_turn = turn;
        self.last_ts = ts.to_owned();
        self.fresh = true;
    }

    fn close(&mut self) {
        let topic = self
            .topic
            .take()
            .unwrap_or_else(|| cut_word(&one_line(head_chars(&self.body, TOPIC_SCAN_CHARS)), TOPIC_CHARS));
        let text = format!(
            "# {topic}\nctx turns={}-{} ts={} session={}\n{}",
            self.first_turn, self.last_turn, self.first_ts, self.session, self.body
        );
        self.out.push_back(Fraction {
            index: self.index,
            first_turn: self.first_turn,
            last_turn: self.last_turn,
            text,
        });
        self.index += 1;
        if self.out.len() > self.max_fractions {
            self.out.pop_front();
            self.evicted = true;
        }
        self.body = carry_of(&self.body, self.overlap);
        self.body_chars = self.body.chars().count();
        self.first_turn = self.last_turn;
        self.first_ts = self.last_ts.clone();
        self.fresh = false;
    }

    fn finish(mut self) -> (VecDeque<Fraction>, bool) {
        if self.fresh {
            self.close();
        }
        (self.out, self.evicted)
    }
}

pub fn scan<R: BufRead>(reader: R, session: &str) -> Scan {
    scan_bounded(reader, session, MAX_FRACTIONS, MAX_READ_BYTES)
}

pub fn scan_bounded<R: BufRead>(
    reader: R,
    session: &str,
    max_fractions: usize,
    max_bytes: u64,
) -> Scan {
    let key = session_key(session);
    let mut builder = Builder::new(&key, max_fractions);
    let mut summaries: VecDeque<Summary> = VecDeque::new();
    let mut seen = 0usize;
    let mut bad = 0u64;
    let mut oversize = 0u64;
    let cut = walk(reader, max_bytes, |event| match event {
        Event::Bad => bad = bad.saturating_add(1),
        Event::Oversize => oversize = oversize.saturating_add(1),
        Event::Entry(e) if e.summary => {
            seen += 1;
            let body = truncate_chars(&e.text, MAX_SUMMARY_CHARS);
            let topic = topic_of(summary_head(&body)).unwrap_or_else(|| "ctx-summary".to_owned());
            summaries.push_back(Summary {
                n: seen,
                text: format!("# {topic}\nctx-summary n={seen} ts={} session={key}\n{body}", e.ts),
            });
            if summaries.len() > MAX_SUMMARIES {
                summaries.pop_front();
            }
        }
        Event::Entry(e) => builder.push_entry(&e.text, &e.ts, e.topic.as_deref()),
    });
    let (fractions, evicted) = builder.finish();
    Scan {
        fractions: fractions.into(),
        summaries: summaries.into(),
        bad_lines: bad,
        oversize_lines: oversize,
        truncated: cut || evicted,
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub fractions: usize,
    pub written: usize,
    pub unchanged: usize,
    pub summaries: usize,
}

pub fn capture_into(
    rag: &Rag,
    session: &str,
    scan: &Scan,
    max_writes: usize,
) -> Result<Report, String> {
    let key = session_key(session);
    let mut report = Report {
        fractions: scan.fractions.len(),
        summaries: scan.summaries.len(),
        ..Report::default()
    };
    let put = |source: &str, text: &str, report: &mut Report| -> Result<(), String> {
        let r = rag.index_text(source, text).map_err(|e| e.to_string())?;
        report.written += r.files_indexed;
        report.unchanged += r.files_unchanged;
        Ok(())
    };
    for s in &scan.summaries {
        put(&summary_source(&key, s.n), &s.text, &mut report)?;
    }
    for f in scan.fractions.iter().rev() {
        if report.written >= max_writes {
            break;
        }
        put(&fraction_source(&key, f.index), &f.text, &mut report)?;
    }
    Ok(report)
}

pub fn prune_stale(rag: &Rag, graph: &Graph, session: &str, scan: &Scan) -> Result<usize, String> {
    if scan.truncated {
        return Ok(0);
    }
    let prefix = format!("ctx/{}/", session_key(session));
    let last_fraction = scan.fractions.last().map(|f| f.index);
    let last_summary = scan.summaries.last().map(|s| s.n);
    let mut stale: Vec<String> = Vec::new();
    {
        let read = graph.begin_read().map_err(|e| e.to_string())?;
        for node in read.nodes_by_label("File").map_err(|e| e.to_string())? {
            let node = node.map_err(|e| e.to_string())?;
            let Some(uriel_core::Value::String(path)) = node.props.get("path") else {
                continue;
            };
            let Some(name) = path.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".md")) else {
                continue;
            };
            let dead = match name.strip_prefix("summary-") {
                Some(n) => n.parse::<usize>().ok().zip(last_summary).is_some_and(|(n, l)| n > l),
                None => name.parse::<usize>().ok().zip(last_fraction).is_some_and(|(n, l)| n > l),
            };
            if dead {
                stale.push(path.clone());
            }
        }
    }
    let mut removed = 0;
    for path in stale {
        removed += rag.index_text(&path, "").map_err(|e| e.to_string())?.files_indexed;
    }
    Ok(removed)
}

pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn open_read_only(db: &Path) -> Result<Rag, String> {
    let budget = MemoryBudget::mib(uriel_mcp::config::DEFAULT_MEMORY_MIB);
    let graph = Graph::open_read_only(db, budget)
        .map_err(|e| format!("cannot open database {}: {e}", db.display()))?;
    Rag::open(graph, RagConfig::default()).map_err(|e| e.to_string())
}

pub fn open_db(db: &Path) -> Result<(Rag, Graph), String> {
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    let budget = MemoryBudget::mib(uriel_mcp::config::DEFAULT_MEMORY_MIB);
    let graph = Graph::open(db, budget)
        .map_err(|e| format!("cannot open database {}: {e}", db.display()))?;
    let rag = Rag::open(graph.clone(), RagConfig::default()).map_err(|e| e.to_string())?;
    Ok((rag, graph))
}

pub fn scan_file(path: &Path, session: &str) -> Result<Scan, String> {
    let file = File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    Ok(scan(BufReader::with_capacity(READ_BUFFER_BYTES, file), session))
}

pub fn resolve_session(
    flag: Option<String>,
    payload: &Payload,
    transcript: &Path,
) -> String {
    flag.or_else(|| payload.session_id.clone())
        .or_else(|| {
            transcript
                .file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

fn run_inner<R: BufRead>(
    db: &Path,
    session: Option<String>,
    transcript: Option<PathBuf>,
    input: R,
) -> Result<(), String> {
    let payload = read_payload(input).unwrap_or_default();
    let transcript = transcript
        .or_else(|| payload.transcript_path.clone())
        .ok_or("no transcript: pass the hook payload on stdin or --transcript")?;
    let session = resolve_session(session, &payload, &transcript);
    let scanned = scan_file(&transcript, &session)?;
    if scanned.fractions.is_empty() && scanned.summaries.is_empty() {
        return Ok(());
    }
    let (rag, graph) = open_db(db)?;
    capture_into(&rag, &session, &scanned, MAX_WRITES)?;
    prune_stale(&rag, &graph, &session, &scanned)?;
    Ok(())
}

pub fn run<R: BufRead>(
    db: &Path,
    session: Option<String>,
    transcript: Option<PathBuf>,
    input: R,
) {
    if let Err(e) = run_inner(db, session, transcript, input) {
        eprintln!("uriel capture: {}", one_line(&e));
    }
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod tests;
