use std::collections::HashMap;

use serde_json::{Map, Value as Json};

const MAX_ALIASES: usize = 16;
const MIN_ALIAS_SAVING: usize = 3;
const ALIAS_USE_TOKENS: usize = 1;
const ALIAS_DEF_TOKENS: usize = 1;

const TABLE_FLAG: &str = " +";
const RUN_MARK: &str = "^";
const MAX_DICT: usize = 52;
const MIN_DICT_SAVING: usize = 2;
const PREFIX_CHARS: &str = " _/-.$@#&:";
const LETTERS_PER_TOKEN: usize = 6;
const PUNCT_PER_TOKEN: usize = 2;
const EXPANSION_FACTOR: usize = 64;
const EXPANSION_FLOOR: usize = 1 << 24;

static NULL: Json = Json::Null;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("compact decode error at line {line}: {message}")]
    Malformed {
        line: usize,
        message: String,
    },
}

fn bad(line: usize, message: impl Into<String>) -> DecodeError {
    DecodeError::Malformed {
        line,
        message: message.into(),
    }
}

pub fn proxy_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

pub fn estimate_tokens(s: &str) -> usize {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut i = 0;
    let mut total = 0;
    while i < n {
        let ch = c[i];
        if ch.is_numeric() {
            let mut j = i;
            while j < n && c[j].is_numeric() {
                j += 1;
            }
            total += (j - i).div_ceil(3);
            i = j;
            continue;
        }
        let start = if !ch.is_alphabetic()
            && PREFIX_CHARS.contains(ch)
            && c.get(i + 1).is_some_and(|x| x.is_alphabetic())
        {
            i + 1
        } else {
            i
        };
        if c.get(start).is_some_and(|x| x.is_alphabetic()) {
            let mut j = start;
            let (mut ascii, mut other) = (0usize, 0usize);
            while j < n && c[j].is_alphabetic() {
                if c[j].is_ascii() {
                    ascii += 1;
                } else {
                    other += 1;
                }
                j += 1;
            }
            total += ascii.div_ceil(LETTERS_PER_TOKEN).max(1) + other;
            i = j;
            continue;
        }
        let mut j = i;
        if ch == '\r' || ch == '\n' {
            while j < n && (c[j] == '\r' || c[j] == '\n') {
                j += 1;
            }
            total += 1;
        } else if ch.is_whitespace() {
            while j < n && c[j].is_whitespace() && c[j] != '\r' && c[j] != '\n' {
                j += 1;
            }
            total += 1;
        } else {
            while j < n && !c[j].is_alphanumeric() && !c[j].is_whitespace() {
                j += 1;
            }
            total += (j - i).div_ceil(PUNCT_PER_TOKEN);
        }
        i = j.max(i + 1);
    }
    total
}

fn escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
}

fn escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    escape_into(s, &mut out);
    out
}

fn unescape(s: &str, line: usize) -> Result<String, DecodeError> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => return Err(bad(line, format!("unknown escape \\{other}"))),
            None => return Err(bad(line, "dangling backslash")),
        }
    }
    Ok(out)
}

fn needs_quote(s: &str) -> bool {
    match s.chars().next() {
        None => true,
        Some(c) => {
            matches!(c, '\'' | '@' | '~' | '-' | '+' | '.' | '^' | '$' | '0'..='9')
                || s == "true"
                || s == "false"
        }
    }
}

fn encode_string(s: &str) -> String {
    if needs_quote(s) {
        format!("'{}", escaped(s))
    } else {
        escaped(s)
    }
}

fn alias_name(i: usize) -> char {
    let a = u8::try_from(i).unwrap_or(0);
    if a < 26 {
        char::from(b'a' + a)
    } else {
        char::from(b'A' + (a - 26) % 26)
    }
}

fn prefixes(s: &str) -> impl Iterator<Item = &str> {
    s.char_indices()
        .filter(|(_, c)| matches!(c, '/' | '\\'))
        .map(|(i, c)| &s[..i + c.len_utf8()])
        .chain(std::iter::once(s))
        .filter(|p| !p.is_empty())
}

fn plan_aliases<'a>(strings: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for s in strings {
        *counts.entry(s).or_insert(0) += 1;
    }
    let mut remaining: Vec<(&str, usize)> = counts.into_iter().collect();
    remaining.sort_unstable();
    let mut chosen: Vec<String> = Vec::new();
    while chosen.len() < MAX_ALIASES {
        let mut tally: HashMap<&str, usize> = HashMap::new();
        for (s, n) in &remaining {
            for p in prefixes(s) {
                *tally.entry(p).or_insert(0) += n;
            }
        }
        let best = tally
            .into_iter()
            .filter_map(|(p, uses)| {
                let t = proxy_tokens(&escaped(p));
                let gain = uses * t.saturating_sub(ALIAS_USE_TOKENS);
                let cost = t + ALIAS_DEF_TOKENS;
                (gain >= cost + MIN_ALIAS_SAVING).then(|| (gain - cost, p))
            })
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(a.1)));
        let Some((_, prefix)) = best else {
            break;
        };
        let prefix = prefix.to_string();
        remaining.retain(|(s, _)| !s.starts_with(prefix.as_str()));
        chosen.push(prefix);
    }
    chosen
}

fn apply_alias(s: &str, aliases: &[String]) -> Option<String> {
    let (i, prefix) = aliases
        .iter()
        .enumerate()
        .filter(|(_, p)| s.starts_with(p.as_str()))
        .max_by_key(|(_, p)| p.len())?;
    let rest = &s[prefix.len()..];
    let mut out = String::new();
    out.push('@');
    out.push(alias_name(i));
    if !rest.is_empty() {
        out.push(' ');
        escape_into(rest, &mut out);
    }
    Some(out)
}

fn alias_index(c: char) -> Option<usize> {
    match c {
        'a'..='z' => Some(usize::from(c as u8 - b'a')),
        'A'..='Z' => Some(26 + usize::from(c as u8 - b'A')),
        _ => None,
    }
}

fn expand_alias(field: &str, aliases: &[String], line: usize) -> Result<String, DecodeError> {
    let mut chars = field.chars();
    chars.next();
    let name = chars.next().ok_or_else(|| bad(line, "empty alias"))?;
    let prefix = alias_index(name)
        .and_then(|i| aliases.get(i))
        .ok_or_else(|| bad(line, format!("undefined alias @{name}")))?;
    let rest = chars.as_str();
    if rest.is_empty() {
        return Ok(prefix.clone());
    }
    let rest = rest
        .strip_prefix(' ')
        .ok_or_else(|| bad(line, "alias must be followed by a space"))?;
    Ok(format!("{prefix}{}", unescape(rest, line)?))
}

fn read_aliases(lines: &[&str], pos: &mut usize) -> Result<Vec<String>, DecodeError> {
    let mut aliases: Vec<String> = Vec::new();
    while let Some(l) = lines.get(*pos) {
        let mut it = l.chars();
        if it.next() != Some('@') {
            break;
        }
        let (Some(name), Some('=')) = (it.next(), it.next()) else {
            break;
        };
        if alias_index(name) != Some(aliases.len()) {
            return Err(bad(*pos + 1, "alias definitions must be a, b, c, ... in order"));
        }
        let value = &l[2 + name.len_utf8()..];
        aliases.push(unescape(value, *pos + 1)?);
        *pos += 1;
    }
    Ok(aliases)
}

fn write_aliases(aliases: &[String], out: &mut String) {
    for (i, p) in aliases.iter().enumerate() {
        out.push('@');
        out.push(alias_name(i));
        out.push('=');
        escape_into(p, out);
        out.push('\n');
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Json>>,
}

impl Table {
    pub fn from_rows(rows: &[Json]) -> Option<Table> {
        let Some(first) = rows.first() else {
            return Some(Table {
                columns: Vec::new(),
                rows: Vec::new(),
            });
        };
        let columns: Vec<String> = first.as_object()?.keys().cloned().collect();
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let obj = row.as_object()?;
            if obj.len() != columns.len() {
                return None;
            }
            let mut cells = Vec::with_capacity(columns.len());
            for c in &columns {
                cells.push(obj.get(c)?.clone());
            }
            out.push(cells);
        }
        Some(Table { columns, rows: out })
    }

    pub fn to_rows(&self) -> Vec<Json> {
        self.rows
            .iter()
            .map(|r| {
                let mut m = Map::new();
                for (c, v) in self.columns.iter().zip(r) {
                    m.insert(c.clone(), v.clone());
                }
                Json::Object(m)
            })
            .collect()
    }
}

fn encode_cell(v: &Json, aliases: &[String], out: &mut String) {
    match v {
        Json::Null => {}
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else if let Some(f) = n.as_f64() {
                out.push_str(&format!("{f:?}"));
            }
        }
        Json::String(s) => match apply_alias(s, aliases) {
            Some(a) => out.push_str(&a),
            None => out.push_str(&encode_string(s)),
        },
        Json::Array(_) | Json::Object(_) => {
            out.push('~');
            out.push_str(&v.to_string());
        }
    }
}

fn decode_ref(rest: &str, dict: &[Json], line: usize) -> Result<Json, DecodeError> {
    let mut it = rest.chars();
    let name = it
        .next()
        .ok_or_else(|| bad(line, "empty dictionary reference"))?;
    if it.next().is_some() {
        return Err(bad(line, "dictionary reference must be `$` and one letter"));
    }
    alias_index(name)
        .and_then(|i| dict.get(i))
        .cloned()
        .ok_or_else(|| bad(line, format!("undefined dictionary value ${name}")))
}

fn decode_cell(
    field: &str,
    aliases: &[String],
    dict: Option<&[Json]>,
    line: usize,
) -> Result<Json, DecodeError> {
    if field.is_empty() {
        return Ok(Json::Null);
    }
    match field {
        "true" => return Ok(Json::Bool(true)),
        "false" => return Ok(Json::Bool(false)),
        _ => {}
    }
    if let Some(d) = dict {
        if field == RUN_MARK {
            return Err(bad(line, "run marker is not allowed here"));
        }
        if let Some(rest) = field.strip_prefix('$') {
            return decode_ref(rest, d, line);
        }
    }
    let first = field.chars().next().unwrap_or(' ');
    match first {
        '\'' => Ok(Json::String(unescape(&field[1..], line)?)),
        '@' => Ok(Json::String(expand_alias(field, aliases, line)?)),
        '~' => serde_json::from_str::<Json>(&field[1..])
            .map_err(|e| bad(line, format!("bad nested JSON: {e}"))),
        '0'..='9' | '-' | '+' | '.' => parse_number(field)
            .ok_or_else(|| bad(line, format!("bad number `{field}`"))),
        _ => Ok(Json::String(unescape(field, line)?)),
    }
}

fn json_cost(v: &Json) -> usize {
    match v {
        Json::String(s) => s.len() + 1,
        Json::Array(a) => 1 + a.iter().map(json_cost).sum::<usize>(),
        Json::Object(m) => 1 + m.iter().map(|(k, v)| k.len() + json_cost(v)).sum::<usize>(),
        _ => 8,
    }
}
fn parse_number(field: &str) -> Option<Json> {
    if let Ok(i) = field.parse::<i64>() {
        return Some(Json::from(i));
    }
    if let Ok(u) = field.parse::<u64>() {
        return Some(Json::from(u));
    }
    let f = field.parse::<f64>().ok()?;
    serde_json::Number::from_f64(f).map(Json::Number)
}

fn cell_tokens(column: usize, body: &str) -> usize {
    let mut s = String::with_capacity(body.len() + 1);
    s.push(if column == 0 { '\n' } else { '\t' });
    s.push_str(body);
    estimate_tokens(&s)
}

fn raw_cell(v: &Json) -> String {
    let mut s = String::new();
    encode_cell(v, &[], &mut s);
    s
}

fn is_dict_candidate(v: &Json) -> bool {
    matches!(v, Json::String(_) | Json::Number(_))
}

fn plan_copies(table: &Table, runs: bool) -> Vec<Vec<bool>> {
    let mark = |c: usize| cell_tokens(c, RUN_MARK);
    table
        .rows
        .iter()
        .enumerate()
        .map(|(r, row)| {
            row.iter()
                .enumerate()
                .map(|(c, v)| {
                    runs && r > 0
                        && !v.is_null()
                        && table.rows[r - 1].get(c).unwrap_or(&NULL) == v
                        && cell_tokens(c, &raw_cell(v)) > mark(c)
                })
                .collect()
        })
        .collect()
}

fn plan_dictionary(table: &Table, copies: &[Vec<bool>]) -> Vec<(String, Json)> {
    let mut tally: HashMap<String, (Json, usize, usize)> = HashMap::new();
    for (r, row) in table.rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            if copies[r][c] || !is_dict_candidate(v) {
                continue;
            }
            let raw = raw_cell(v);
            let saved = cell_tokens(c, &raw).saturating_sub(cell_tokens(c, "$a"));
            let entry = tally
                .entry(raw)
                .or_insert_with(|| (v.clone(), 0, 0));
            entry.1 += 1;
            entry.2 += saved;
        }
    }
    let mut chosen: Vec<(usize, String, Json)> = tally
        .into_iter()
        .filter_map(|(raw, (v, _, saved))| {
            let cost = estimate_tokens(&format!("\n$a={raw}"));
            (saved >= cost + MIN_DICT_SAVING).then(|| (saved - cost, raw, v))
        })
        .collect();
    chosen.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    chosen.truncate(MAX_DICT);
    chosen.into_iter().map(|(_, raw, v)| (raw, v)).collect()
}

fn encode_features(table: &Table, runs: bool, dict: bool) -> String {
    let mut out = String::new();
    out.push('#');
    out.push_str(&table.rows.len().to_string());
    if table.columns.is_empty() && table.rows.is_empty() {
        return out;
    }
    let copies = plan_copies(table, runs);
    let entries = if dict {
        plan_dictionary(table, &copies)
    } else {
        Vec::new()
    };
    let index: HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, (raw, _))| (raw.as_str(), i))
        .collect();
    let in_dict = |v: &Json| {
        if is_dict_candidate(v) {
            index.get(raw_cell(v).as_str()).copied()
        } else {
            None
        }
    };
    let mut strings: Vec<&str> = Vec::new();
    for (r, row) in table.rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            if let Some(s) = v.as_str() {
                if !copies[r][c] && in_dict(v).is_none() {
                    strings.push(s);
                }
            }
        }
    }
    strings.extend(entries.iter().filter_map(|(_, v)| v.as_str()));
    let aliases = plan_aliases(strings.into_iter());
    if copies.iter().flatten().any(|b| *b) || !entries.is_empty() {
        out.push_str(TABLE_FLAG);
    }
    out.push('\n');
    write_aliases(&aliases, &mut out);
    for (i, (_, v)) in entries.iter().enumerate() {
        out.push('$');
        out.push(alias_name(i));
        out.push('=');
        encode_cell(v, &aliases, &mut out);
        out.push('\n');
    }
    for (i, c) in table.columns.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        out.push_str(&encode_string(c));
    }
    for (r, row) in table.rows.iter().enumerate() {
        out.push('\n');
        let mut line = String::new();
        for (c, v) in row.iter().enumerate() {
            if c > 0 {
                line.push('\t');
            }
            if copies[r][c] {
                line.push_str(RUN_MARK);
            } else if let Some(i) = in_dict(v) {
                line.push('$');
                line.push(alias_name(i));
            } else {
                encode_cell(v, &aliases, &mut line);
            }
        }
        out.push_str(line.trim_end_matches('\t'));
    }
    out
}

pub fn encode_table_plain(table: &Table) -> String {
    encode_features(table, false, false)
}

pub fn encode_table(table: &Table) -> String {
    let mut best = encode_table_plain(table);
    let mut best_cost = estimate_tokens(&best);
    for (runs, dict) in [(true, false), (false, true), (true, true)] {
        let candidate = encode_features(table, runs, dict);
        let cost = estimate_tokens(&candidate);
        if cost < best_cost {
            best = candidate;
            best_cost = cost;
        }
    }
    best
}

fn parse_count(line: &str) -> Result<usize, DecodeError> {
    line.strip_prefix('#')
        .and_then(|n| n.parse::<usize>().ok())
        .ok_or_else(|| bad(1, "expected `#<count>` on the first line"))
}

fn read_dictionary(
    lines: &[&str],
    pos: &mut usize,
    aliases: &[String],
) -> Result<Vec<Json>, DecodeError> {
    let mut dict: Vec<Json> = Vec::new();
    while let Some(l) = lines.get(*pos) {
        let Some(rest) = l.strip_prefix('$') else {
            break;
        };
        let mut it = rest.chars();
        let (Some(name), Some('=')) = (it.next(), it.next()) else {
            break;
        };
        if alias_index(name) != Some(dict.len()) {
            return Err(bad(*pos + 1, "dictionary definitions must be a, b, c, ... in order"));
        }
        let value = &rest[name.len_utf8() + 1..];
        dict.push(decode_cell(value, aliases, Some(&[]), *pos + 1)?);
        *pos += 1;
    }
    Ok(dict)
}

pub fn decode_table(text: &str) -> Result<Table, DecodeError> {
    let lines: Vec<&str> = text.split('\n').collect();
    let (n, flagged) = match lines[0].strip_suffix(TABLE_FLAG) {
        Some(head) => (parse_count(head)?, true),
        None => (parse_count(lines[0])?, false),
    };
    let mut pos = 1;
    let aliases = read_aliases(&lines, &mut pos)?;
    let dict = if flagged {
        read_dictionary(&lines, &mut pos, &aliases)?
    } else {
        Vec::new()
    };
    let dict_ref: Option<&[Json]> = flagged.then_some(dict.as_slice());
    let columns: Vec<String> = match lines.get(pos) {
        None | Some(&"") => Vec::new(),
        Some(h) => h
            .split('\t')
            .map(|f| decode_name(f, pos + 1))
            .collect::<Result<_, _>>()?,
    };
    pos += 1;
    let mut budget = text
        .len()
        .saturating_mul(EXPANSION_FACTOR)
        .max(EXPANSION_FLOOR);
    let mut rows: Vec<Vec<Json>> = Vec::with_capacity(n.min(lines.len()));
    for r in 0..n {
        let ln = pos + r + 1;
        let line = lines
            .get(pos + r)
            .ok_or_else(|| bad(ln, "truncated: fewer rows than announced"))?;
        let mut cells: Vec<Json> = Vec::with_capacity(columns.len());
        if !line.is_empty() {
            for f in line.split('\t') {
                let copied = flagged && (f == RUN_MARK || f.starts_with('$'));
                let cell = if flagged && f == RUN_MARK {
                    rows.last()
                        .and_then(|prev| prev.get(cells.len()))
                        .cloned()
                        .ok_or_else(|| bad(ln, "run marker without a cell above"))?
                } else {
                    decode_cell(f, &aliases, dict_ref, ln)?
                };
                if copied {
                    budget = budget
                        .checked_sub(json_cost(&cell))
                        .ok_or_else(|| bad(ln, "expansion limit exceeded"))?;
                }
                cells.push(cell);
            }
        }
        if cells.len() > columns.len() {
            return Err(bad(ln, "more cells than columns"));
        }
        cells.resize(columns.len(), Json::Null);
        rows.push(cells);
    }
    if lines.len() > pos + n && !(n == 0 && columns.is_empty()) {
        return Err(bad(pos + n + 1, "trailing lines after the last row"));
    }
    Ok(Table { columns, rows })
}
fn decode_name(field: &str, line: usize) -> Result<String, DecodeError> {
    match field.strip_prefix('\'') {
        Some(rest) => unescape(rest, line),
        None => unescape(field, line),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackHit {
    pub chunk_id: u64,
    pub file: String,
    pub start: u32,
    pub end: u32,
    pub level: u8,
    pub graph: bool,
    pub remembered: bool,
    pub changed: bool,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pack {
    pub hits: Vec<PackHit>,
    pub omitted: usize,
}

const PACK_LEVEL: u8 = 3;
const GRAPH_ON: &str = "=g";
const GRAPH_OFF: &str = "=";
const RUN_TOGGLE_TOKENS: usize = 2;

fn escape_path_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            ' ' => out.push_str("\\s"),
            c => out.push(c),
        }
    }
}

fn unescape_path(s: &str, line: usize) -> Result<String, DecodeError> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('s') => out.push(' '),
            Some(other) => return Err(bad(line, format!("unknown escape \\{other}"))),
            None => return Err(bad(line, "dangling backslash")),
        }
    }
    Ok(out)
}

fn encode_file(file: &str, aliases: &[String], out: &mut String) {
    let best = aliases
        .iter()
        .enumerate()
        .filter(|(_, p)| file.starts_with(p.as_str()))
        .max_by_key(|(_, p)| p.len());
    if let Some((i, prefix)) = best {
        out.push('@');
        out.push(alias_name(i));
        let rest = &file[prefix.len()..];
        if !rest.is_empty() {
            out.push(' ');
            escape_path_into(rest, out);
        }
        return;
    }
    if file.starts_with(['@', '\'']) || (!file.is_empty() && file.bytes().all(|b| b.is_ascii_digit())) {
        out.push('\'');
    }
    escape_path_into(file, out);
}

fn decode_file(field: &str, aliases: &[String], line: usize) -> Result<String, DecodeError> {
    if let Some(after) = field.strip_prefix('@') {
        let mut chars = after.chars();
        let name = chars.next().ok_or_else(|| bad(line, "empty alias"))?;
        let prefix = alias_index(name)
            .and_then(|i| aliases.get(i))
            .ok_or_else(|| bad(line, format!("undefined alias @{name}")))?;
        let rest = chars.as_str();
        if rest.is_empty() {
            return Ok(prefix.clone());
        }
        let rest = rest
            .strip_prefix(' ')
            .ok_or_else(|| bad(line, "alias must be followed by a space"))?;
        return Ok(format!("{prefix}{}", unescape_path(rest, line)?));
    }
    match field.strip_prefix('\'') {
        Some(rest) => unescape_path(rest, line),
        None => unescape_path(field, line),
    }
}

fn encode_range(start: u32, end: u32, prev_end: Option<u32>) -> String {
    let mut starts = vec![start.to_string()];
    if let Some(p) = prev_end {
        starts.push(if start >= p {
            format!("+{}", start - p)
        } else {
            format!("-{}", p - start)
        });
    }
    let mut ends = Vec::new();
    if end == start {
        ends.push(String::new());
    }
    ends.push(format!("-{end}"));
    if end >= start {
        ends.push(format!("+{}", end - start));
    }
    let mut best = String::new();
    let mut best_cost = usize::MAX;
    for s in &starts {
        for e in &ends {
            let candidate = format!("{s}{e}");
            let cost = estimate_tokens(&candidate);
            if cost < best_cost {
                best = candidate;
                best_cost = cost;
            }
        }
    }
    best
}

fn split_digits(s: &str) -> (&str, &str) {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    s.split_at(n)
}

fn parse_digits<T: std::str::FromStr>(s: &str, line: usize, what: &str) -> Result<T, DecodeError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad(line, format!("bad {what}")));
    }
    s.parse::<T>().map_err(|_| bad(line, format!("bad {what}")))
}

fn decode_range(text: &str, prev_end: Option<u32>, line: usize) -> Result<(u32, u32), DecodeError> {
    let (start, rest) = match text.as_bytes().first() {
        Some(sign @ (b'+' | b'-')) => {
            let p = prev_end
                .ok_or_else(|| bad(line, "relative start without an earlier hit in the same file"))?;
            let (digits, rest) = split_digits(&text[1..]);
            let d: u32 = parse_digits(digits, line, "relative start")?;
            let s = if *sign == b'+' { p.checked_add(d) } else { p.checked_sub(d) }
                .ok_or_else(|| bad(line, "relative start out of range"))?;
            (s, rest)
        }
        _ => {
            let (digits, rest) = split_digits(text);
            (parse_digits::<u32>(digits, line, "start line")?, rest)
        }
    };
    let end = match rest.as_bytes().first() {
        None => start,
        Some(b'-') => parse_digits(&rest[1..], line, "end line")?,
        Some(b'+') => start
            .checked_add(parse_digits(&rest[1..], line, "line count")?)
            .ok_or_else(|| bad(line, "end line out of range"))?,
        Some(_) => return Err(bad(line, "bad line range")),
    };
    Ok((start, end))
}

fn split_header(line: &str) -> (&str, Option<&str>) {
    let mut it = line.char_indices();
    let skip = match (it.next(), it.next(), it.next()) {
        (Some((_, '@')), Some(_), Some((i, ' '))) => i + 1,
        _ => 0,
    };
    match line[skip..].find(' ') {
        Some(j) => (&line[..skip + j], Some(&line[skip + j + 1..])),
        None => (line, None),
    }
}

fn graph_runs(hits: &[PackHit]) -> Vec<bool> {
    let mut in_run = vec![false; hits.len()];
    let mut i = 0;
    while i < hits.len() {
        if !hits[i].graph {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < hits.len() && hits[j].graph {
            j += 1;
        }
        let toggles = if j < hits.len() { 2 * RUN_TOGGLE_TOKENS } else { RUN_TOGGLE_TOKENS };
        if toggles < j - i {
            in_run[i..j].fill(true);
        }
        i = j;
    }
    in_run
}

pub fn encode_pack(pack: &Pack) -> String {
    let mut out = format!("#{}", pack.hits.len());
    if pack.omitted > 0 {
        out.push_str(&format!(" omit={}", pack.omitted));
    }
    out.push('\n');
    let mut first_seen: Vec<&str> = Vec::new();
    for h in pack.hits.iter().filter(|h| h.body.is_some()) {
        if !first_seen.contains(&h.file.as_str()) {
            first_seen.push(h.file.as_str());
        }
    }
    let aliases = plan_aliases(first_seen.iter().copied());
    write_aliases(&aliases, &mut out);
    let in_run = graph_runs(&pack.hits);
    let mut introduced: HashMap<&str, usize> = HashMap::new();
    let mut prev_file: Option<&str> = None;
    let mut last_end: HashMap<&str, u32> = HashMap::new();
    let mut run_open = false;
    for (h, runs) in pack.hits.iter().zip(&in_run) {
        if *runs != run_open {
            out.push_str(if *runs { GRAPH_ON } else { GRAPH_OFF });
            out.push('\n');
            run_open = *runs;
        }
        if h.body.is_some() {
            if prev_file != Some(h.file.as_str()) {
                match introduced.get(h.file.as_str()) {
                    Some(n) => out.push_str(&n.to_string()),
                    None => {
                        encode_file(&h.file, &aliases, &mut out);
                        introduced.insert(h.file.as_str(), introduced.len());
                    }
                }
                out.push(':');
            }
            out.push_str(&encode_range(h.start, h.end, last_end.get(h.file.as_str()).copied()));
            prev_file = Some(h.file.as_str());
            last_end.insert(h.file.as_str(), h.end);
        }
        out.push('#');
        out.push_str(&h.chunk_id.to_string());
        if h.level != PACK_LEVEL {
            out.push_str(&format!("L{}", h.level));
        }
        if h.graph && !runs {
            out.push('g');
        }
        if h.remembered {
            out.push('m');
        }
        if h.changed {
            out.push('~');
        }
        match &h.body {
            None => {
                out.push('r');
                out.push('\n');
            }
            Some(b) if !b.contains('\n') => {
                out.push(' ');
                out.push_str(b);
                out.push('\n');
            }
            Some(b) => {
                out.push_str(&format!("+{}\n", b.split('\n').count()));
                out.push_str(b);
                out.push('\n');
            }
        }
    }
    out
}

struct Flags {
    level: u8,
    graph: bool,
    remembered: bool,
    changed: bool,
    reference: bool,
    lines: Option<usize>,
}

fn parse_flags(s: &str, line: usize) -> Result<Flags, DecodeError> {
    let mut f = Flags {
        level: PACK_LEVEL,
        graph: false,
        remembered: false,
        changed: false,
        reference: false,
        lines: None,
    };
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            'g' => f.graph = true,
            'm' => f.remembered = true,
            '~' => f.changed = true,
            'r' => f.reference = true,
            'L' => {
                f.level = it
                    .next()
                    .and_then(|d| d.to_digit(10))
                    .and_then(|d| u8::try_from(d).ok())
                    .ok_or_else(|| bad(line, "`L` must be followed by a digit"))?;
            }
            '+' => {
                let mut digits = String::new();
                while let Some(d) = it.next_if(char::is_ascii_digit) {
                    digits.push(d);
                }
                f.lines = Some(
                    digits
                        .parse()
                        .map_err(|_| bad(line, "`+` must be followed by a line count"))?,
                );
            }
            other => return Err(bad(line, format!("unknown flag `{other}`"))),
        }
    }
    Ok(f)
}

fn resolve_file(
    field: &str,
    aliases: &[String],
    introduced: &[String],
    line: usize,
) -> Result<String, DecodeError> {
    if !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()) {
        let n: usize = parse_digits(field, line, "file index")?;
        return introduced
            .get(n)
            .cloned()
            .ok_or_else(|| bad(line, format!("file index {n} is not introduced yet")));
    }
    decode_file(field, aliases, line)
}

pub fn decode_pack(text: &str) -> Result<Pack, DecodeError> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines[0];
    let (count, omitted) = match first.split_once(" omit=") {
        Some((c, o)) => (
            parse_count(c)?,
            o.parse::<usize>()
                .map_err(|_| bad(1, "bad omit count"))?,
        ),
        None => (parse_count(first)?, 0),
    };
    let mut pos = 1;
    let aliases = read_aliases(&lines, &mut pos)?;
    let mut hits = Vec::with_capacity(count.min(lines.len()));
    let mut introduced: Vec<String> = Vec::new();
    let mut prev_file: Option<String> = None;
    let mut last_end: HashMap<String, u32> = HashMap::new();
    let mut run_open = false;
    while hits.len() < count {
        let line_no = pos + 1;
        let raw = *lines
            .get(pos)
            .ok_or_else(|| bad(line_no, "truncated: fewer citations than announced"))?;
        pos += 1;
        if raw == GRAPH_ON || raw == GRAPH_OFF {
            run_open = raw == GRAPH_ON;
            continue;
        }
        let (header, inline) = split_header(raw);
        let (loc, tail) = header
            .rsplit_once('#')
            .ok_or_else(|| bad(line_no, "citation lacks `#<id>`"))?;
        let (id_text, flags_text) = split_digits(tail);
        let chunk_id: u64 = parse_digits(id_text, line_no, "chunk id")?;
        let flags = parse_flags(flags_text, line_no)?;
        if loc.is_empty() {
            if !flags.reference || inline.is_some() || flags.lines.is_some() {
                return Err(bad(line_no, "a bare `#<id>` citation must carry the `r` flag and no text"));
            }
            hits.push(PackHit {
                chunk_id,
                file: String::new(),
                start: 0,
                end: 0,
                level: flags.level,
                graph: flags.graph || run_open,
                remembered: flags.remembered,
                changed: flags.changed,
                body: None,
            });
            continue;
        }
        if flags.reference {
            return Err(bad(line_no, "a reference carries no location"));
        }
        let (file, range) = match loc.rsplit_once(':') {
            Some((f, r)) => {
                let file = resolve_file(f, &aliases, &introduced, line_no)?;
                if !introduced.contains(&file) {
                    introduced.push(file.clone());
                }
                (file, r)
            }
            None => (
                prev_file
                    .clone()
                    .ok_or_else(|| bad(line_no, "no earlier file to repeat"))?,
                loc,
            ),
        };
        let (start, end) = decode_range(range, last_end.get(&file).copied(), line_no)?;
        let body = match (inline, flags.lines) {
            (Some(b), None) => b.to_string(),
            (None, Some(k)) => {
                let slice = pos
                    .checked_add(k)
                    .and_then(|stop| lines.get(pos..stop))
                    .ok_or_else(|| bad(pos + 1, "truncated: body shorter than announced"))?;
                pos += k;
                slice.join("\n")
            }
            (Some(_), Some(_)) => return Err(bad(line_no, "inline text and a line count")),
            (None, None) => return Err(bad(line_no, "a hit without text must be a reference")),
        };
        last_end.insert(file.clone(), end);
        prev_file = Some(file.clone());
        hits.push(PackHit {
            chunk_id,
            file,
            start,
            end,
            level: flags.level,
            graph: flags.graph || run_open,
            remembered: flags.remembered,
            changed: flags.changed,
            body: Some(body),
        });
    }
    if pos < lines.len() {
        return Err(bad(pos + 1, "trailing lines after the last hit"));
    }
    Ok(Pack { hits, omitted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
        fn chance(&mut self, percent: u64) -> bool {
            self.next() % 100 < percent
        }
    }

    const FRAGMENTS: &[&str] = &[
        "", "\t", "\n", "\r\n", "\\", "\\t", "\\n", "|", ",", "@a", "@a=x", "@", "'", "''", "~",
        "#5", " #7", ":1-2", "x:3-4 #9 L1", "-3", "+1", ".5", "1.5e3", "0", "true", "false",
        "null", "\u{e9}", "\u{65e5}\u{672c}\u{8a9e}", "\u{1F600}", "a b", " lead", "trail ", "/", "\\\\", "\u{2028}", "\u{0}",
        "\u{7f}", ">", ">> x", "L1gm", "+4", "r", "~{\"a\":1}", "[1,2]", "{}", "word",
        "^", "^x", "$", "$a", "$a=x", "'^", "#5 +", " +",
    ];

    const LONG: &[&str] = &[
        "crates/uriel-house/src/validate.rs",
        "crates/uriel-house/src/lib.rs",
        "crates/uriel-rag/src/ladder.rs",
        "C:\\Users\\seal\\seal\\crates\\uriel-core\\src\\graph.rs",
        "a fairly long sentence with several plain words",
        "^caret path/with/depth/file.rs",
        "$dollar path/with/depth/file.rs",
        "^",
        "$a",
        "'quoted start of a long value",
        "tab\tinside a long value that repeats",
    ];

    const SHARED: &[&str] = &[
        "crates/uriel-house/src/",
        "crates/uriel-rag/src/",
        "C:\\Users\\seal\\seal\\crates\\",
    ];

    fn gen_string(r: &mut Rng) -> String {
        let mut s = String::new();
        if r.chance(35) {
            s.push_str(SHARED[r.below(SHARED.len())]);
        }
        for _ in 0..r.below(4) {
            if r.chance(70) {
                s.push_str(FRAGMENTS[r.below(FRAGMENTS.len())]);
            } else {
                for _ in 0..r.below(6) {
                    s.push(char::from(32 + (r.next() % 95) as u8));
                }
            }
        }
        s
    }

    fn gen_json(r: &mut Rng, depth: usize) -> Json {
        match r.below(if depth == 0 { 9 } else { 6 }) {
            0 => Json::Null,
            1 => Json::Bool(r.chance(50)),
            2 => Json::from(r.next() as i64),
            3 => Json::from(r.next()),
            4 => Json::from(r.below(2000) as i64 - 1000),
            5 => {
                let f = (r.next() as i64 as f64) / 1e3;
                Json::from(if r.chance(30) { f.trunc() } else { f })
            }
            6 => Json::String(gen_string(r)),
            7 => Json::Array((0..r.below(4)).map(|_| gen_json(r, 1)).collect()),
            _ => {
                let mut m = Map::new();
                for _ in 0..r.below(4) {
                    m.insert(gen_string(r), gen_json(r, 1));
                }
                Json::Object(m)
            }
        }
    }

    fn gen_cell(r: &mut Rng) -> Json {
        if r.chance(40) {
            Json::String(gen_string(r))
        } else {
            gen_json(r, 0)
        }
    }

    fn gen_table(r: &mut Rng) -> Table {
        let ncols = r.below(6);
        let nrows = r.below(8);
        Table {
            columns: (0..ncols).map(|_| gen_string(r)).collect(),
            rows: (0..nrows)
                .map(|_| {
                    (0..ncols)
                        .map(|_| if r.chance(15) { Json::Null } else { gen_cell(r) })
                        .collect()
                })
                .collect(),
        }
    }

    fn gen_pool_value(r: &mut Rng) -> Json {
        match r.below(6) {
            0..=2 => Json::String(LONG[r.below(LONG.len())].to_string()),
            3 => Json::from(1_000_000_000_000i64 + r.below(5) as i64),
            4 => Json::from(r.below(3) as i64),
            _ => Json::String(gen_string(r)),
        }
    }

    fn gen_repeat_table(r: &mut Rng) -> Table {
        let ncols = 1 + r.below(5);
        let nrows = r.below(40);
        let pools: Vec<Vec<Json>> = (0..ncols)
            .map(|_| (0..1 + r.below(5)).map(|_| gen_pool_value(r)).collect())
            .collect();
        let mut rows: Vec<Vec<Json>> = Vec::new();
        for i in 0..nrows {
            let mut row: Vec<Json> = Vec::with_capacity(ncols);
            for (c, pool) in pools.iter().enumerate() {
                let v = if i > 0 && r.chance(55) {
                    rows[i - 1][c].clone()
                } else if r.chance(8) {
                    Json::Null
                } else {
                    pool[r.below(pool.len())].clone()
                };
                row.push(v);
            }
            if r.chance(30) {
                let k = 1 + r.below(ncols);
                for cell in row.iter_mut().rev().take(k) {
                    *cell = Json::Null;
                }
            }
            rows.push(row);
        }
        Table {
            columns: (0..ncols).map(|i| format!("c{i}")).collect(),
            rows,
        }
    }

    fn features(text: &str) -> (bool, bool, bool) {
        let flagged = text.lines().next().is_some_and(|l| l.ends_with(" +"));
        let runs = text.lines().any(|l| l.split('\t').any(|f| f == "^"));
        let dict = text
            .lines()
            .any(|l| l.starts_with('$') && l.chars().nth(2) == Some('='));
        (flagged, runs, dict)
    }

    fn long_table(values: &[&str]) -> Table {
        Table {
            columns: vec!["f".into()],
            rows: values.iter().map(|v| vec![json!(v)]).collect(),
        }
    }

    #[test]
    fn repeat_tables_roundtrip_over_2000_random_tables() {
        let mut r = Rng(0xC0FF_EE00_1234_5678);
        let (mut runs, mut dicts, mut plain) = (0, 0, 0);
        for i in 0..2000 {
            let t = gen_repeat_table(&mut r);
            let text = encode_table(&t);
            let (flagged, has_run, has_dict) = features(&text);
            runs += usize::from(has_run);
            dicts += usize::from(has_dict);
            plain += usize::from(!flagged);
            assert_eq!(flagged, has_run || has_dict, "#{i}\n{text:?}");
            let back = decode_table(&text).unwrap_or_else(|e| panic!("#{i}: {e}\n{text:?}"));
            assert_eq!(back, t, "#{i}\n{text:?}");
        }
        assert!(runs > 200, "run markers in {runs} of 2000");
        assert!(dicts > 100, "dictionaries in {dicts} of 2000");
        assert!(plain > 20, "plain format in {plain} of 2000");
    }

    #[test]
    fn plain_encoding_still_roundtrips_over_2000_random_tables_of_each_kind() {
        let mut r = Rng(0x00DD_BA11_5EED_0001);
        for i in 0..2000 {
            let t = if i % 2 == 0 { gen_table(&mut r) } else { gen_repeat_table(&mut r) };
            let text = encode_table_plain(&t);
            assert_eq!(features(&text), (false, false, false), "#{i}\n{text:?}");
            let back = decode_table(&text).unwrap_or_else(|e| panic!("#{i}: {e}\n{text:?}"));
            assert_eq!(back, t, "#{i}\n{text:?}");
        }
    }

    #[test]
    fn legacy_text_decodes_unchanged() {
        let legacy = "#4\n@a=crates/uriel-house/src/\nfile\tlines\tn\n@a validate.rs\t'32-36\t5\n@a lib.rs\t'1-9\n@a gate.rs\t'7\t-2\n@a plan.rs\t'7\t1.5";
        let t = decode_table(legacy).expect("legacy");
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[0][0], json!("crates/uriel-house/src/validate.rs"));
        assert_eq!(t.rows[3][2], json!(1.5));
        let bare = decode_table("#2\nf\n^\n$a").expect("bare markers are plain strings without the flag");
        assert_eq!(bare.rows, vec![vec![json!("^")], vec![json!("$a")]]);
    }

    #[test]
    fn encoding_is_never_larger_than_plain_by_the_estimate() {
        let mut r = Rng(0xFEED_FACE_0000_0007);
        for i in 0..2000 {
            let t = if i % 2 == 0 { gen_table(&mut r) } else { gen_repeat_table(&mut r) };
            let new = estimate_tokens(&encode_table(&t));
            let old = estimate_tokens(&encode_table_plain(&t));
            assert!(new <= old, "#{i}: {new} > {old}\n{t:?}");
        }
    }

    #[test]
    fn equal_neighbours_collapse_to_run_markers() {
        let t = long_table(&[LONG[0], LONG[0], LONG[0], LONG[0], LONG[0], LONG[0]]);
        let text = encode_table(&t);
        assert_eq!(text.lines().filter(|l| *l == "^").count(), 5, "{text:?}");
        assert!(text.lines().next().is_some_and(|l| l == "#6 +"), "{text:?}");
        assert!(estimate_tokens(&text) < estimate_tokens(&encode_table_plain(&t)));
        assert_eq!(decode_table(&text).expect("decode"), t);
    }

    #[test]
    fn many_scattered_repeats_enter_the_dictionary() {
        let values: Vec<String> = (0..24)
            .map(|i| format!("a long distinct sentence number {i} with several plain words"))
            .collect();
        let rows: Vec<Vec<Json>> = (0..3)
            .flat_map(|_| values.iter().map(|v| vec![json!(v)]))
            .collect();
        let t = Table {
            columns: vec!["f".into()],
            rows,
        };
        let text = encode_table(&t);
        let defs = text.lines().filter(|l| l.starts_with('$') && l.chars().nth(2) == Some('=')).count();
        assert_eq!(defs, 24, "{text:?}");
        assert!(estimate_tokens(&text) < estimate_tokens(&encode_table_plain(&t)));
        assert_eq!(decode_table(&text).expect("decode"), t);
    }

    #[test]
    fn decoder_refuses_expansion_bombs() {
        let big = "x".repeat(100_000);
        let mut dict_bomb = format!("#200000 +\n$a={big}\nf");
        let mut run_bomb = format!("#200000 +\nf\n{big}");
        for _ in 0..200_000 {
            dict_bomb.push_str("\n$a");
            run_bomb.push_str("\n^");
        }
        for text in [dict_bomb, run_bomb] {
            let err = decode_table(&text).expect_err("bomb");
            assert!(err.to_string().contains("expansion limit"), "{err}");
        }
    }
    #[test]
    fn long_numbers_enter_the_dictionary() {
        let big = [1_234_567_890_123_i64, 7, 1_234_567_890_123, 8, 1_234_567_890_123, 9, 1_234_567_890_123];
        let t = Table {
            columns: vec!["n".into()],
            rows: big.iter().map(|v| vec![json!(v)]).collect(),
        };
        let text = encode_table(&t);
        assert!(text.lines().any(|l| l == "$a=1234567890123"), "{text:?}");
        assert_eq!(decode_table(&text).expect("decode"), t);
    }

    #[test]
    fn shortcuts_are_not_used_when_they_do_not_save() {
        let tiny = Table {
            columns: vec!["a".into(), "b".into()],
            rows: (0..30)
                .map(|i| vec![json!(i % 2), json!("new")])
                .chain((0..10).map(|_| vec![json!(1), json!("new")]))
                .collect(),
        };
        assert_eq!(encode_table(&tiny), encode_table_plain(&tiny));
        let unique = long_table(&[LONG[0], LONG[1], LONG[2], LONG[3]]);
        assert_eq!(encode_table(&unique), encode_table_plain(&unique));
    }

    #[test]
    fn trailing_nulls_are_omitted() {
        let t = Table {
            columns: vec!["a".into(), "b".into(), "c".into()],
            rows: vec![
                vec![json!(1), Json::Null, Json::Null],
                vec![json!(2), json!(3), Json::Null],
                vec![Json::Null, Json::Null, Json::Null],
            ],
        };
        let text = encode_table(&t);
        assert_eq!(text, "#3\na\tb\tc\n1\n2\t3\n");
        assert_eq!(decode_table(&text).expect("decode"), t);
    }

    #[test]
    fn markers_decode_only_in_flagged_tables_and_reject_misuse() {
        let ok = decode_table("#3 +\n$a=x\n$b=7\nf\tg\n$a\t$b\n^\t^\n^").expect("flagged");
        assert_eq!(
            ok.rows,
            vec![
                vec![json!("x"), json!(7)],
                vec![json!("x"), json!(7)],
                vec![json!("x"), Json::Null],
            ]
        );
        for bad in [
            "#1 +\nf\n^",
            "#1 +\nf\n$a",
            "#1 +\n$b=x\nf\nx",
            "#1 +\n$a=$a\nf\nx",
            "#1 +\n$a=^\nf\nx",
            "#1 +\n$a=x\nf\n$ab",
            "#1 +\n$a=x\nf\n$",
            "#1 +\n$a=x\nf\n$\u{e9}",
            "#1 +x\nf\nx",
            "#1  +\nf\nx",
            "#2 +\nf\nx\n^\t^",
        ] {
            assert!(decode_table(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn estimate_tracks_cl100k_on_fixed_samples() {
        let samples: &[(&str, usize)] = &[
            ("crates/uriel-house/src/validate.rs", 9),
            ("\tabc", 2),
            ("\t^", 2),
            ("\t$a", 2),
            ("1234567", 3),
            ("validate_rules", 2),
            ("@a lib.rs", 3),
        ];
        let mut est = 0;
        let mut real = 0;
        for (s, n) in samples {
            est += estimate_tokens(s);
            real += n;
        }
        let ratio = est as f64 / real as f64;
        assert!((0.8..=1.2).contains(&ratio), "estimate {est} vs cl100k {real}");
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("1234567"), 3);
        assert_eq!(estimate_tokens("\t^"), 2);
    }

    fn gen_pack(r: &mut Rng) -> Pack {
        let n = r.below(9);
        let hits = (0..n)
            .map(|_| PackHit {
                chunk_id: if r.chance(10) { u64::MAX } else { r.next() >> r.below(60) },
                file: gen_string(r),
                start: if r.chance(10) { u32::MAX } else { r.next() as u32 >> r.below(30) },
                end: r.next() as u32 >> r.below(30),
                level: r.below(4) as u8,
                graph: r.chance(30),
                remembered: r.chance(20),
                changed: r.chance(20),
                body: if r.chance(25) {
                    None
                } else {
                    let mut b = String::new();
                    for _ in 0..r.below(4) {
                        b.push_str(&gen_string(r));
                        if r.chance(60) {
                            b.push('\n');
                        }
                    }
                    Some(b)
                },
            })
            .map(|mut h| {
                if h.body.is_none() {
                    h.file.clear();
                    h.start = 0;
                    h.end = 0;
                }
                h
            })
            .collect();
        Pack {
            hits,
            omitted: if r.chance(40) { r.below(50) } else { 0 },
        }
    }

    #[test]
    fn table_roundtrip_over_2000_random_tables() {
        let mut r = Rng(0x9E37_79B9_7F4A_7C15);
        let mut aliased = 0;
        for i in 0..2000 {
            let t = gen_table(&mut r);
            let text = encode_table(&t);
            if text.contains("\n@") && text.lines().any(|l| l.starts_with("@a=")) {
                aliased += 1;
            }
            let back = decode_table(&text).unwrap_or_else(|e| panic!("#{i}: {e}\n{text:?}"));
            assert_eq!(back, t, "#{i}\n{text:?}");
        }
        assert!(aliased > 20, "generator should exercise aliases, got {aliased}");
    }

    #[test]
    fn pack_roundtrip_over_2000_random_packs() {
        let mut r = Rng(0xD1B5_4A32_D192_ED03);
        let mut aliased = 0;
        for i in 0..2000 {
            let p = gen_pack(&mut r);
            let text = encode_pack(&p);
            if text.lines().any(|l| l.starts_with("@a=")) {
                aliased += 1;
            }
            let back = decode_pack(&text).unwrap_or_else(|e| panic!("#{i}: {e}\n{text:?}"));
            assert_eq!(back, p, "#{i}\n{text:?}");
        }
        assert!(aliased > 20, "generator should exercise aliases, got {aliased}");
    }

    #[test]
    fn table_example_is_exactly_as_documented() {
        let rows = vec![
            json!({"file": "crates/uriel-house/src/validate.rs", "lines": "32-36", "n": 5}),
            json!({"file": "crates/uriel-house/src/lib.rs", "lines": "1-9", "n": null}),
            json!({"file": "crates/uriel-house/src/gate.rs", "lines": "7", "n": -2}),
            json!({"file": "crates/uriel-house/src/plan.rs", "lines": "7", "n": 1.5}),
        ];
        let t = Table::from_rows(&rows).expect("uniform");
        let text = encode_table(&t);
        assert_eq!(
            text,
            "#4\n@a=crates/uriel-house/src/\nfile\tlines\tn\n@a validate.rs\t'32-36\t5\n@a lib.rs\t'1-9\n@a gate.rs\t'7\t-2\n@a plan.rs\t'7\t1.5"
        );
        assert_eq!(decode_table(&text).expect("decode").to_rows(), rows);
    }

    #[test]
    fn aliases_are_only_used_when_they_save() {
        let few = Table {
            columns: vec!["f".into()],
            rows: vec![vec![json!("crates/a/")], vec![json!("src/b/")]],
        };
        assert!(!encode_table(&few).contains("@a="), "{}", encode_table(&few));
        let many = Table {
            columns: vec!["f".into()],
            rows: (0..30)
                .map(|i| vec![json!(format!("crates/uriel-house/src/file{i}.rs"))])
                .collect(),
        };
        let text = encode_table(&many);
        assert!(text.contains("@a=crates/uriel-house/src/"), "{text}");
        let plain: usize = many
            .rows
            .iter()
            .map(|r| r[0].as_str().map_or(0, str::len))
            .sum();
        assert!(text.len() < plain, "{} vs {plain}", text.len());
        assert_eq!(decode_table(&text).expect("decode"), many);
    }

    #[test]
    fn nulls_numbers_and_ambiguous_strings_survive() {
        let t = Table {
            columns: vec!["a".into(), "b".into(), "c".into()],
            rows: vec![
                vec![json!(""), json!("42"), json!("true")],
                vec![json!(3.0), json!(-0.5), Json::Null],
                vec![Json::Null, Json::Null, Json::Null],
                vec![json!("@x"), json!("'q"), json!("a\tb\nc\\d")],
                vec![json!([1, "x"]), json!({"k": "v"}), json!(u64::MAX)],
            ],
        };
        let text = encode_table(&t);
        assert_eq!(decode_table(&text).expect("decode"), t, "{text:?}");
        assert!(text.contains("\n3.0\t"), "float keeps its point: {text:?}");
    }

    #[test]
    fn empty_results_and_non_uniform_rows() {
        let empty = Table::from_rows(&[]).expect("empty");
        assert_eq!(encode_table(&empty), "#0");
        assert_eq!(decode_table("#0").expect("decode"), empty);
        assert!(Table::from_rows(&[json!({"a": 1}), json!({"b": 2})]).is_none());
        assert!(Table::from_rows(&[json!(1)]).is_none());
        assert!(Table::from_rows(&[json!({"a": 1}), json!({"a": 1, "b": 2})]).is_none());
    }

    #[test]
    fn decoder_rejects_garbage() {
        for bad in ["", "x", "#x", "#2\na\n1", "#1\na\n1\t2", "#1\n@b=x\na\n1", "#1\na\n\\q"] {
            assert!(decode_table(bad).is_err(), "{bad:?}");
        }
        for bad in ["", "#1", "#1\nfile:1-2 #3", "#1\nf:1-2 #3 z\nx", "#1\nf:1-2 #3 +3\nx", "#1\n#3", "#1\n#x r"] {
            assert!(decode_pack(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn pack_example_is_exactly_as_documented() {
        let pack = Pack {
            hits: vec![
                PackHit {
                    chunk_id: 1547,
                    file: "crates/uriel-house/src/validate.rs".into(),
                    start: 32,
                    end: 36,
                    level: 1,
                    graph: false,
                    remembered: false,
                    changed: false,
                    body: Some("fn a() {\n}".into()),
                },
                PackHit {
                    chunk_id: 12,
                    file: String::new(),
                    start: 0,
                    end: 0,
                    level: 2,
                    graph: true,
                    remembered: false,
                    changed: false,
                    body: None,
                },
                PackHit {
                    chunk_id: 13,
                    file: "crates/uriel-house/src/gate.rs".into(),
                    start: 3,
                    end: 4,
                    level: 3,
                    graph: false,
                    remembered: false,
                    changed: false,
                    body: Some("fn gate".into()),
                },
            ],
            omitted: 2,
        };
        let text = encode_pack(&pack);
        assert_eq!(
            text,
            "#3 omit=2\n@a=crates/uriel-house/src/\n@a validate.rs:32-36#1547L1+2\nfn a() {\n}\n#12L2gr\n@a gate.rs:3-4#13 fn gate\n"
        );
        assert_eq!(decode_pack(&text).expect("decode"), pack);
    }

    fn gist(chunk_id: u64, file: &str, start: u32, end: u32, body: &str) -> PackHit {
        PackHit {
            chunk_id,
            file: file.into(),
            start,
            end,
            level: 3,
            graph: false,
            remembered: false,
            changed: false,
            body: Some(body.into()),
        }
    }

    #[test]
    fn a_repeated_file_is_dropped_and_its_range_may_be_relative() {
        let pack = Pack {
            hits: vec![
                gist(2229, "uriel-house/src/sched.rs", 179, 218, "fn defer()"),
                gist(2230, "uriel-house/src/sched.rs", 214, 253, "let over = heavy"),
                gist(2875, "uriel-mcp/src/compact.rs", 1343, 1358, "fn table_example()"),
                gist(2880, "uriel-mcp/src/compact.rs", 1421, 1467, "fn pack_example()"),
                gist(2231, "uriel-house/src/probe.rs", 249, 285, "}"),
            ],
            omitted: 0,
        };
        let text = encode_pack(&pack);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "#5", "{text}");
        assert_eq!(lines[1], "uriel-house/src/sched.rs:179-218#2229 fn defer()", "{text}");
        assert_eq!(lines[2], "214-253#2230 let over = heavy", "{text}");
        assert_eq!(lines[3], "uriel-mcp/src/compact.rs:1343+15#2875 fn table_example()", "{text}");
        assert_eq!(lines[4], "1421+46#2880 fn pack_example()", "{text}");
        assert_eq!(lines[5], "uriel-house/src/probe.rs:249-285#2231 }", "{text}");
        assert_eq!(lines.len(), 6, "every gist shares its citation line: {text}");
        assert_eq!(decode_pack(&text).expect("decode"), pack);
        let relative = decode_pack("#4\nf:10-20#1 x\n+5+3#2 y\ng:7#3 z\nf:-4-30#4 w").expect("relative");
        let spans: Vec<(&str, u32, u32)> = relative.hits.iter().map(|h| (h.file.as_str(), h.start, h.end)).collect();
        assert_eq!(spans, [("f", 10, 20), ("f", 25, 28), ("g", 7, 7), ("f", 24, 30)]);
    }

    #[test]
    fn a_relative_start_is_chosen_only_when_it_is_cheaper() {
        assert_eq!(encode_range(214, 253, Some(218)), "214-253");
        assert_eq!(encode_range(1_000_000_003, 1_000_000_009, Some(1_000_000_001)), "+2+6");
        assert_eq!(encode_range(5, 5, None), "5");
        assert_eq!(encode_range(9, 4, Some(2)), "9-4");
    }

    #[test]
    fn single_line_bodies_share_the_citation_line_and_multi_line_bodies_follow_it() {
        let mut two = gist(7, "a b/c.rs", 1, 9, "x\ny");
        two.level = 1;
        let pack = Pack {
            hits: vec![gist(5, "a b/c.rs", 3, 3, "  spaced # body : 1-2#3"), two, gist(6, "@odd", 4, 2, "")],
            omitted: 0,
        };
        let text = encode_pack(&pack);
        assert_eq!(
            text,
            "#3\na\\sb/c.rs:3#5   spaced # body : 1-2#3\n1-9#7L1+2\nx\ny\n'@odd:4-2#6 \n"
        );
        assert_eq!(decode_pack(&text).expect("decode"), pack);
    }

    #[test]
    fn the_compact_citation_decoder_rejects_dangling_shortcuts() {
        for bad in [
            "#1\n1-2#3 x",
            "#1\nf:+1-2#3 x",
            "#2\nf:1-2#3 x\ng:-5#4 x",
            "#1\nf:1-2#3+1 x\ny",
            "#1\nf:1-2#3r",
            "#1\nf:1-2#3",
            "#1\n#3r x",
            "#1\nf:1-+2#3 x",
            "#1\nf:1x#3 x",
            "#1\nf:4294967295+1#3 x",
            "#2\nf:1-2#3 x\n-3#4 x",
            "#1\n@a x:1-2#3 y",
            "#1\n5:1-2#3 x",
            "#2\nf:1-2#3 x\n1:1-2#4 y",
            "#0\n=g",
            "#1\n=g\n=\n",
        ] {
            assert!(decode_pack(bad).is_err(), "{bad:?}");
        }
    }

    fn located(chunk_id: u64, file: &str, graph: bool) -> PackHit {
        PackHit {
            graph,
            ..gist(chunk_id, file, 1, 2, "x")
        }
    }

    #[test]
    fn a_file_seen_before_is_cited_by_its_number() {
        let pack = Pack {
            hits: vec![
                gist(1, "dir/a.rs", 1, 2, "x"),
                gist(2, "dir/b.rs", 3, 4, "y"),
                gist(3, "dir/a.rs", 5, 6, "z"),
                gist(4, "dir/c.rs", 7, 8, "w"),
                gist(5, "dir/b.rs", 9, 9, "v"),
                gist(6, "dir/b.rs", 10, 11, "u"),
                gist(7, "dir/a.rs", 12, 13, "t"),
            ],
            omitted: 0,
        };
        let text = encode_pack(&pack);
        assert_eq!(
            text,
            "#7\ndir/a.rs:1-2#1 x\ndir/b.rs:3-4#2 y\n0:5-6#3 z\ndir/c.rs:7-8#4 w\n1:9#5 v\n10-11#6 u\n0:12-13#7 t\n"
        );
        assert_eq!(decode_pack(&text).expect("decode"), pack);
    }

    #[test]
    fn a_path_made_of_digits_is_quoted_so_it_is_not_a_file_number() {
        let pack = Pack {
            hits: vec![gist(1, "7", 1, 2, "x"), gist(2, "dir/a.rs", 1, 2, "y"), gist(3, "7", 3, 4, "z"), gist(4, "", 1, 1, "e")],
            omitted: 0,
        };
        let text = encode_pack(&pack);
        assert_eq!(text, "#4\n'7:1-2#1 x\ndir/a.rs:1-2#2 y\n0:3-4#3 z\n:1#4 e\n");
        assert_eq!(decode_pack(&text).expect("decode"), pack);
    }

    #[test]
    fn a_long_run_of_graph_hits_is_one_toggle_pair_and_a_short_one_keeps_its_flags() {
        let run = |n: usize, tail: bool| {
            let mut hits = vec![located(1, "a.rs", false)];
            hits.extend((0..n).map(|i| located(10 + i as u64, "a.rs", true)));
            if tail {
                hits.push(located(99, "a.rs", false));
            }
            Pack { hits, omitted: 0 }
        };
        let flags = |text: &str| text.lines().filter(|l| l.contains("#") && l.contains('g')).count();
        for (n, tail, toggled) in [(4, true, false), (5, true, true), (2, false, false), (3, false, true), (6, false, true)] {
            let p = run(n, tail);
            let text = encode_pack(&p);
            assert_eq!(text.lines().any(|l| l == "=g"), toggled, "{n} {tail}: {text}");
            assert_eq!(text.lines().any(|l| l == "="), toggled && tail, "{n} {tail}: {text}");
            assert_eq!(flags(&text), if toggled { 0 } else { n }, "{n} {tail}: {text}");
            assert_eq!(decode_pack(&text).expect("decode"), p, "{n} {tail}: {text}");
        }
        let reopened = Pack {
            hits: (0..14).map(|i| located(i, "a.rs", (3..8).contains(&i) || i >= 10)).collect(),
            omitted: 0,
        };
        let text = encode_pack(&reopened);
        assert_eq!(text.lines().filter(|l| *l == "=g").count(), 2, "{text}");
        assert_eq!(decode_pack(&text).expect("decode"), reopened);
    }

    fn mutate(r: &mut Rng, s: &str, edits: usize) -> String {
        const ALPHABET: &[&str] = &[
            "\t", "\n", "\\", "#", "@", "'", "~", ">", " ", "-", "+", "=", ":", "0", "9", "99999999999999999999",
            "18446744073709551615", "L", "g", "r", "m", "x", "a", "\u{e9}", "\u{1F600}", "\u{0}", "{", "[", "\"", ".", "e",
            "^", "$", "$a", "$a=", " +", "\t^",
        ];
        let mut chars: Vec<char> = s.chars().collect();
        for _ in 0..edits {
            let at = r.below(chars.len() + 1);
            let piece: Vec<char> = ALPHABET[r.below(ALPHABET.len())].chars().collect();
            match r.below(3) {
                0 => {
                    chars.splice(at..at, piece);
                }
                1 if at < chars.len() => {
                    chars.remove(at);
                }
                _ if at < chars.len() => {
                    chars.splice(at..=at, piece);
                }
                _ => {}
            }
        }
        chars.into_iter().collect()
    }

    fn noise(r: &mut Rng) -> String {
        const BITS: &[&str] = &[
            "#", "#3", "#99999999999999999999", "#18446744073709551615", "#4294967295", " omit=", "\n", "\t", "@a=", "@b ", "@", ">", ">#", " #", ":", "-", "L", "g", "r",
            "+", "+99999999999999999999", "~", "'", "\\", "x", "\u{e9}", " ", "0", "7", "\u{0}",
            "#3 +", " +", "$a=", "$b=x", "$a", "$", "^", "\t^", "\n^", "$a=x\n", "#2 +\n$a=1\nf\n$a\n^",
        ];
        (0..r.below(24)).map(|_| BITS[r.below(BITS.len())]).collect()
    }

    #[test]
    fn decoders_survive_20000_adversarial_inputs_each() {
        let mut r = Rng(0xA5A5_5A5A_1234_5678);
        let (mut table_ok, mut table_err, mut pack_ok, mut pack_err) = (0, 0, 0, 0);
        let mut flagged_ok = 0;
        for i in 0..20_000 {
            let t_text = match i % 3 {
                0 => noise(&mut r),
                _ => {
                    let base = if i % 2 == 0 { gen_table(&mut r) } else { gen_repeat_table(&mut r) };
                    let valid = encode_table(&base);
                    let edits = 1 + r.below(4);
                    mutate(&mut r, &valid, edits)
                }
            };
            match decode_table(&t_text) {
                Ok(_) => {
                    table_ok += 1;
                    flagged_ok += usize::from(t_text.lines().next().is_some_and(|l| l.ends_with(" +")));
                }
                Err(_) => table_err += 1,
            }
            let p_text = match i % 3 {
                0 => noise(&mut r),
                _ => {
                    let valid = encode_pack(&gen_pack(&mut r));
                    let edits = 1 + r.below(4);
                    mutate(&mut r, &valid, edits)
                }
            };
            match decode_pack(&p_text) {
                Ok(_) => pack_ok += 1,
                Err(_) => pack_err += 1,
            }
        }
        assert!(table_err > 2_000 && table_ok > 200, "tables ok {table_ok} err {table_err}");
        assert!(flagged_ok > 200, "flagged tables decoded {flagged_ok}");
        assert!(pack_err > 2_000 && pack_ok > 200, "packs ok {pack_ok} err {pack_err}");
    }}
