use std::collections::BTreeMap;
use std::iter::Peekable;
use std::str::Chars;

use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};

pub const HARD_INPUT_BYTES: usize = 1 << 20;
pub const MIN_CAP: usize = 64;
pub const DEFAULT_CAP: usize = 4096;
const TRUNC_MARK: &str = "\n[truncated]";
const FENCE_OPEN: &str = "<<uriel-data";
const FENCE_CLOSE: &str = "<<end-uriel-data";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    pub max_bytes: usize,
}

impl Policy {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes: max_bytes.clamp(MIN_CAP, HARD_INPUT_BYTES),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::new(DEFAULT_CAP)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub truncated: bool,
    pub ansi: usize,
    pub controls: usize,
    pub hidden: usize,
    pub tags: usize,
    pub secrets: BTreeMap<&'static str, usize>,
    pub fences: usize,
    pub injection: usize,
}

impl Report {
    pub fn secret_total(&self) -> usize {
        self.secrets.values().sum()
    }

    pub fn suspect(&self) -> bool {
        self.injection > 0 || self.tags > 0 || self.hidden > 0 || self.fences > 0
    }

    pub fn clean(&self) -> bool {
        !self.truncated
            && self.ansi == 0
            && self.controls == 0
            && self.hidden == 0
            && self.tags == 0
            && self.secrets.is_empty()
            && self.fences == 0
            && self.injection == 0
    }

    pub fn to_json(&self) -> Json {
        json!({
            "truncated": self.truncated,
            "ansi": self.ansi,
            "controls": self.controls,
            "hidden": self.hidden,
            "tags": self.tags,
            "secrets": self.secrets,
            "fences": self.fences,
            "injection_markers": self.injection,
            "suspect": self.suspect()
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    pub text: String,
    pub report: Report,
}

pub fn sanitize(input: &str, policy: &Policy) -> Sanitized {
    let mut report = Report::default();
    let bounded = cut(input, HARD_INPUT_BYTES, &mut report);
    let stripped = strip(bounded, &mut report);
    let redacted = redact(&stripped, &mut report);
    let neutral = neutralise_fences(&redacted, &mut report);
    report.injection = injection_markers(&neutral);
    let text = fit(neutral, policy.max_bytes, &mut report);
    Sanitized { text, report }
}

pub fn fence(sanitized: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"uriel-fence");
    hasher.update(sanitized.as_bytes());
    let nonce: String = hasher
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("{FENCE_OPEN} {nonce} untrusted>>\n{sanitized}\n{FENCE_CLOSE} {nonce}>>")
}

fn cut<'a>(text: &'a str, limit: usize, report: &mut Report) -> &'a str {
    if text.len() <= limit {
        return text;
    }
    report.truncated = true;
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn fit(text: String, cap: usize, report: &mut Report) -> String {
    if text.len() <= cap {
        return text;
    }
    report.truncated = true;
    let room = cap.saturating_sub(TRUNC_MARK.len());
    let mut out = cut(&text, room, &mut Report::default()).to_string();
    out.push_str(TRUNC_MARK);
    out
}

fn strip(input: &str, report: &mut Report) -> String {
    let mut out = String::with_capacity(input.len());
    let mut it = input.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => {
                report.ansi += 1;
                skip_escape(&mut it);
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => report.controls += 1,
            '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}' => report.hidden += 1,
            '\u{E0000}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}' => report.tags += 1,
            c => out.push(c),
        }
    }
    out
}

fn skip_escape(it: &mut Peekable<Chars<'_>>) {
    match it.peek().copied() {
        Some('[') => {
            it.next();
            for c in it.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            it.next();
            while let Some(c) = it.next() {
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' {
                    if it.peek() == Some(&'\\') {
                        it.next();
                    }
                    break;
                }
            }
        }
        Some(_) => {
            it.next();
        }
        None => {}
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

fn upper_digit(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}

fn bearer_set(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'+' | b'/' | b'=' | b'-')
}

fn ident_set(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')
}

fn value_set(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'_' | b'-' | b'.' | b'/' | b'+' | b'=' | b'!' | b'@' | b'#' | b'$' | b'%' | b'^' | b'&' | b'*' | b'~' | b':'
        )
}

struct Rule {
    prefix: &'static str,
    class: &'static str,
    min_tail: usize,
    set: fn(u8) -> bool,
}

const RULES: &[Rule] = &[
    Rule { prefix: "sk-ant-", class: "anthropic_key", min_tail: 20, set: b64url },
    Rule { prefix: "sk-", class: "openai_key", min_tail: 20, set: b64url },
    Rule { prefix: "nvapi-", class: "nvidia_key", min_tail: 20, set: b64url },
    Rule { prefix: "ghp_", class: "github_token", min_tail: 30, set: alnum },
    Rule { prefix: "gho_", class: "github_token", min_tail: 30, set: alnum },
    Rule { prefix: "ghu_", class: "github_token", min_tail: 30, set: alnum },
    Rule { prefix: "ghs_", class: "github_token", min_tail: 30, set: alnum },
    Rule { prefix: "ghr_", class: "github_token", min_tail: 30, set: alnum },
    Rule { prefix: "github_pat_", class: "github_token", min_tail: 20, set: is_word },
    Rule { prefix: "AKIA", class: "aws_key_id", min_tail: 16, set: upper_digit },
    Rule { prefix: "ASIA", class: "aws_key_id", min_tail: 16, set: upper_digit },
    Rule { prefix: "AIza", class: "google_key", min_tail: 35, set: b64url },
    Rule { prefix: "xoxb-", class: "slack_token", min_tail: 10, set: b64url },
    Rule { prefix: "xoxa-", class: "slack_token", min_tail: 10, set: b64url },
    Rule { prefix: "xoxp-", class: "slack_token", min_tail: 10, set: b64url },
    Rule { prefix: "xoxr-", class: "slack_token", min_tail: 10, set: b64url },
    Rule { prefix: "xoxs-", class: "slack_token", min_tail: 10, set: b64url },
    Rule { prefix: "hf_", class: "hf_token", min_tail: 30, set: alnum },
];

const KEYWORDS: &[&str] = &[
    "password", "passwd", "secret", "token", "api_key", "apikey", "api-key", "private_key", "credential",
];

fn take(b: &[u8], from: usize, set: fn(u8) -> bool) -> usize {
    b[from..].iter().take_while(|x| set(**x)).count()
}

struct Hit {
    end: usize,
    class: &'static str,
    keep: usize,
}

fn match_at(s: &str, b: &[u8], i: usize) -> Option<Hit> {
    let rest = &s[i..];
    if rest.starts_with("-----BEGIN ") {
        return pem(s, i);
    }
    for r in RULES {
        if rest.starts_with(r.prefix) {
            let tail = take(b, i + r.prefix.len(), r.set);
            if tail >= r.min_tail {
                return Some(Hit { end: i + r.prefix.len() + tail, class: r.class, keep: 0 });
            }
        }
    }
    if rest.starts_with("eyJ") {
        if let Some(end) = jwt(b, i) {
            return Some(Hit { end, class: "jwt", keep: 0 });
        }
    }
    if rest.len() > 7 && rest.as_bytes()[..7].eq_ignore_ascii_case(b"bearer ") {
        let tail = take(b, i + 7, bearer_set);
        if tail >= 16 {
            return Some(Hit { end: i + 7 + tail, class: "bearer_token", keep: 7 });
        }
    }
    if b[i].is_ascii_alphabetic() {
        return assignment(b, i);
    }
    None
}

fn pem(s: &str, i: usize) -> Option<Hit> {
    let rest = &s[i..];
    let header_end = rest.find('\n').unwrap_or(rest.len());
    if !rest[..header_end].contains("PRIVATE KEY") {
        return None;
    }
    let end = match rest.find("-----END ") {
        Some(e) => {
            let after = e + 9;
            i + rest[after..].find('\n').map_or(rest.len(), |n| after + n)
        }
        None => s.len(),
    };
    Some(Hit { end, class: "private_key", keep: 0 })
}

fn jwt(b: &[u8], i: usize) -> Option<usize> {
    let a = take(b, i, b64url);
    let mut p = i + a;
    if a < 10 || b.get(p) != Some(&b'.') {
        return None;
    }
    p += 1;
    let body = take(b, p, b64url);
    p += body;
    if body < 10 || b.get(p) != Some(&b'.') {
        return None;
    }
    p += 1;
    Some(p + take(b, p, b64url))
}

fn assignment(b: &[u8], i: usize) -> Option<Hit> {
    let id_len = take(b, i, ident_set);
    let ident = String::from_utf8_lossy(&b[i..i + id_len]).to_ascii_lowercase();
    if !KEYWORDS.iter().any(|k| ident.contains(k)) {
        return None;
    }
    let mut j = i + id_len;
    if matches!(b.get(j), Some(b'"' | b'\'')) {
        j += 1;
    }
    while b.get(j) == Some(&b' ') {
        j += 1;
    }
    if !matches!(b.get(j), Some(b'=' | b':')) {
        return None;
    }
    j += 1;
    while b.get(j) == Some(&b' ') {
        j += 1;
    }
    if matches!(b.get(j), Some(b'"' | b'\'')) {
        j += 1;
    }
    let len = take(b, j, value_set);
    let value = &b[j..j + len];
    if len < 8 || value.iter().all(u8::is_ascii_lowercase) {
        return None;
    }
    Some(Hit { end: j + len, class: "assignment", keep: j - i })
}

const REDACT_PASSES: usize = 4;

fn redact(s: &str, report: &mut Report) -> String {
    let mut text = s.to_string();
    for _ in 0..REDACT_PASSES {
        let before = report.secret_total();
        text = redact_once(&text, report);
        if report.secret_total() == before {
            break;
        }
    }
    text
}

fn redact_once(s: &str, report: &mut Report) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !is_word(b[i - 1]);
        if boundary {
            if let Some(hit) = match_at(s, b, i) {
                out.push_str(&s[i..i + hit.keep]);
                out.push_str("[REDACTED:");
                out.push_str(hit.class);
                out.push(']');
                *report.secrets.entry(hit.class).or_insert(0) += 1;
                i = hit.end;
                continue;
            }
        }
        let Some(ch) = s[i..].chars().next() else { break };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn neutralise_fences(s: &str, report: &mut Report) -> String {
    let opens = s.matches(FENCE_OPEN).count();
    let closes = s.matches(FENCE_CLOSE).count();
    if opens + closes == 0 {
        return s.to_string();
    }
    report.fences = opens + closes;
    s.replace(FENCE_CLOSE, "<<end_uriel_data")
        .replace(FENCE_OPEN, "<<uriel_data")
}

const INJECTION: &[&str] = &[
    "ignore previous instructions",
    "ignore all previous",
    "ignore the above",
    "disregard the above",
    "disregard previous",
    "you are now",
    "new instructions:",
    "system prompt",
    "<|im_start|>",
    "<|im_end|>",
    "[inst]",
    "\nsystem:",
];

fn injection_markers(s: &str) -> usize {
    let lower = s.to_lowercase();
    INJECTION.iter().map(|p| lower.matches(p).count()).sum()
}
