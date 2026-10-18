use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use serde_json::{json, Value};
use uriel_mcp::compact::estimate_tokens;
use uriel_rag::MEMORY_PREFIX;

mod common;

const BUDGETS: [u64; 3] = [300, 800, 4000];
const K: u64 = 64;
const SESSION_BUDGET: u64 = 800;
const TOOLS: [&str; 4] = ["rag_search", "seal_recall", "view", "soul"];

const OLD_FORMAT_ESTIMATES: [(&str, u64, usize); 12] = [
    ("rag_search", 300, 35927),
    ("rag_search", 800, 96174),
    ("rag_search", 4000, 461604),
    ("seal_recall", 300, 35739),
    ("seal_recall", 800, 95833),
    ("seal_recall", 4000, 191799),
    ("view", 300, 36896),
    ("view", 800, 60774),
    ("view", 4000, 60774),
    ("soul", 300, 4681),
    ("soul", 800, 12319),
    ("soul", 4000, 55808),
];
const OLD_SESSION_ESTIMATE: usize = 56014;
const REQUIRED_KEPT_PERCENT: usize = 95;

#[derive(Debug, Clone, PartialEq)]
struct Cite {
    id: u64,
    file: String,
    start: u32,
    end: u32,
    level: u8,
    graph: bool,
    remembered: bool,
    changed: bool,
    reference: bool,
    body: Option<String>,
}

#[derive(Debug)]
struct Decoded {
    announced: usize,
    omitted: usize,
    cites: Vec<Cite>,
}

fn unescape_path(s: &str) -> Result<String, String> {
    let mut out = String::new();
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
            other => return Err(format!("bad escape {other:?} in {s:?}")),
        }
    }
    Ok(out)
}

fn slot(c: char) -> Option<usize> {
    match c {
        'a'..='z' => Some(c as usize - 'a' as usize),
        'A'..='Z' => Some(26 + c as usize - 'A' as usize),
        _ => None,
    }
}

fn digits(s: &str) -> (&str, &str) {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    s.split_at(n)
}

fn number<T: std::str::FromStr>(s: &str, what: &str) -> Result<T, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{what} is not a number: {s:?}"));
    }
    s.parse().map_err(|_| format!("{what} out of range: {s:?}"))
}

fn lines_of(range: &str, prev_end: Option<u32>) -> Result<(u32, u32), String> {
    let (start, rest) = match range.chars().next() {
        Some(sign @ ('+' | '-')) => {
            let (d, rest) = digits(&range[1..]);
            let delta: u32 = number(d, "relative start")?;
            let base = prev_end.ok_or("relative start without a previous end")?;
            let start = if sign == '+' { base.checked_add(delta) } else { base.checked_sub(delta) };
            (start.ok_or("relative start out of range")?, rest)
        }
        _ => {
            let (d, rest) = digits(range);
            (number(d, "start")?, rest)
        }
    };
    let end = match rest.chars().next() {
        None => start,
        Some('-') => number(&rest[1..], "end")?,
        Some('+') => start.checked_add(number(&rest[1..], "span")?).ok_or("end out of range")?,
        Some(c) => return Err(format!("bad range tail {c:?}")),
    };
    Ok((start, end))
}

fn head_and_text(line: &str) -> (&str, Option<&str>) {
    let skip = if line.starts_with('@') && line.chars().nth(2) == Some(' ') { 3 } else { 0 };
    match line[skip..].find(' ') {
        Some(i) => (&line[..skip + i], Some(&line[skip + i + 1..])),
        None => (line, None),
    }
}

fn resolve_file(field: &str, aliases: &[String], table: &[String]) -> Result<String, String> {
    if let Some(after) = field.strip_prefix('@') {
        let mut cs = after.chars();
        let name = cs.next().ok_or("empty alias")?;
        let prefix = slot(name).and_then(|i| aliases.get(i)).ok_or_else(|| format!("undefined alias {name}"))?;
        let rest = cs.as_str();
        return match rest.strip_prefix(' ') {
            Some(r) => Ok(format!("{prefix}{}", unescape_path(r)?)),
            None if rest.is_empty() => Ok(prefix.clone()),
            None => Err(format!("alias without a space: {field:?}")),
        };
    }
    if let Some(literal) = field.strip_prefix('\'') {
        return unescape_path(literal);
    }
    if !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()) {
        let n: usize = number(field, "file index")?;
        return table.get(n).cloned().ok_or_else(|| format!("file index {n} is not defined yet"));
    }
    unescape_path(field)
}

fn decode(text: &str) -> Result<Decoded, String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let first = lines[0];
    let (count_text, omitted) = match first.split_once(" omit=") {
        Some((c, o)) => (c, number::<usize>(o, "omit")?),
        None => (first, 0),
    };
    let announced: usize = number(count_text.strip_prefix('#').ok_or("no # header")?, "count")?;
    let mut pos = 1;
    let mut aliases: Vec<String> = Vec::new();
    while let Some(l) = lines.get(pos) {
        let mut cs = l.chars();
        let (Some('@'), Some(name), Some('=')) = (cs.next(), cs.next(), cs.next()) else {
            break;
        };
        if slot(name) != Some(aliases.len()) {
            return Err(format!("alias {name} out of order"));
        }
        aliases.push(unescape_path(&l[3..])?);
        pos += 1;
    }
    let mut cites: Vec<Cite> = Vec::new();
    let mut table: Vec<String> = Vec::new();
    let mut previous: Option<String> = None;
    let mut last_end: HashMap<String, u32> = HashMap::new();
    let mut graph_mode = false;
    while pos < lines.len() {
        let line = lines[pos];
        pos += 1;
        match line {
            "=g" => {
                graph_mode = true;
                continue;
            }
            "=" => {
                graph_mode = false;
                continue;
            }
            _ => {}
        }
        let (head, inline) = head_and_text(line);
        let (loc, tail) = head.rsplit_once('#').ok_or_else(|| format!("no #id in {line:?}"))?;
        let (id_text, flag_text) = digits(tail);
        let id: u64 = number(id_text, "id")?;
        let (mut level, mut graph, mut remembered, mut changed, mut reference, mut extra) = (3u8, false, false, false, false, None);
        let mut fs = flag_text.chars().peekable();
        while let Some(c) = fs.next() {
            match c {
                'L' => level = fs.next().and_then(|d| d.to_digit(10)).ok_or("L without digit")? as u8,
                'g' => graph = true,
                'm' => remembered = true,
                '~' => changed = true,
                'r' => reference = true,
                '+' => {
                    let mut n = String::new();
                    while let Some(d) = fs.next_if(char::is_ascii_digit) {
                        n.push(d);
                    }
                    extra = Some(number::<usize>(&n, "line count")?);
                }
                other => return Err(format!("unknown flag {other:?}")),
            }
        }
        if reference {
            if !loc.is_empty() || inline.is_some() || extra.is_some() {
                return Err(format!("malformed reference {line:?}"));
            }
            cites.push(Cite { id, file: String::new(), start: 0, end: 0, level, graph: graph || graph_mode, remembered, changed, reference, body: None });
            continue;
        }
        if loc.is_empty() {
            return Err(format!("citation without a location {line:?}"));
        }
        let (file, range) = match loc.rsplit_once(':') {
            Some((field, range)) => {
                let f = resolve_file(field, &aliases, &table)?;
                if !table.contains(&f) {
                    table.push(f.clone());
                }
                (f, range)
            }
            None => (previous.clone().ok_or("no previous file")?, loc),
        };
        let (start, end) = lines_of(range, last_end.get(&file).copied())?;
        let body = match (inline, extra) {
            (Some(b), None) => b.to_string(),
            (None, Some(n)) => {
                let slice = lines.get(pos..pos + n).ok_or("body truncated")?;
                pos += n;
                slice.join("\n")
            }
            _ => return Err(format!("bad body shape {line:?}")),
        };
        last_end.insert(file.clone(), end);
        previous = Some(file.clone());
        cites.push(Cite { id, file, start, end, level, graph: graph || graph_mode, remembered, changed, reference, body: Some(body) });
    }
    if cites.len() != announced {
        return Err(format!("announced {announced} hits, decoded {}", cites.len()));
    }
    Ok(Decoded { announced, omitted, cites })
}

fn ladder_truth(json_text: &str) -> Decoded {
    let v: Value = serde_json::from_str(json_text).unwrap_or_else(|e| panic!("json ({e}): {json_text}"));
    let text = v["text"].as_str().expect("text");
    let hits = v["hits"].as_array().expect("hits");
    let heads: Vec<String> = hits
        .iter()
        .map(|h| format!("// {}:{}-{} #{}", h["file"].as_str().expect("file"), h["start_line"], h["end_line"], h["chunk_id"]))
        .collect();
    let mut cites = Vec::new();
    let mut pos = 0;
    for (i, h) in hits.iter().enumerate() {
        assert!(text[pos..].starts_with(&heads[i]), "head {} not at {pos}", heads[i]);
        let after = pos + heads[i].len() + 1;
        let stop = match heads.get(i + 1) {
            Some(next) => text[after..].find(&format!("\n{next}")).map(|j| after + j).expect("next head"),
            None => text.len() - 1,
        };
        let file = h["file"].as_str().expect("file").to_string();
        cites.push(Cite {
            id: h["chunk_id"].as_u64().expect("id"),
            start: h["start_line"].as_u64().expect("start") as u32,
            end: h["end_line"].as_u64().expect("end") as u32,
            level: h["level"].as_u64().expect("level") as u8,
            graph: h["via"].as_str() == Some("graph"),
            remembered: file.starts_with(MEMORY_PREFIX),
            changed: false,
            reference: false,
            body: Some(text[after.min(stop)..stop].to_string()),
            file,
        });
        pos = stop + 1;
    }
    Decoded { announced: cites.len(), omitted: v["omitted"].as_u64().expect("omitted") as usize, cites }
}

fn soul_truth(json_text: &str) -> Decoded {
    let v: Value = serde_json::from_str(json_text).unwrap_or_else(|e| panic!("json ({e})"));
    let cites: Vec<Cite> = v["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .map(|s| {
            let file = s["file"].as_str().expect("file").to_string();
            Cite {
                id: s["chunk_id"].as_u64().expect("id"),
                start: s["start_line"].as_u64().expect("start") as u32,
                end: s["end_line"].as_u64().expect("end") as u32,
                level: s["level"].as_u64().expect("level") as u8,
                graph: false,
                remembered: file.starts_with(MEMORY_PREFIX),
                changed: false,
                reference: false,
                body: Some(s["text"].as_str().expect("text").to_string()),
                file,
            }
        })
        .collect();
    Decoded { announced: cites.len(), omitted: v["omitted"].as_u64().expect("omitted") as usize, cites }
}

struct Outputs {
    matrix: BTreeMap<(String, u64, usize), (String, String)>,
    session: Vec<String>,
}

fn collect() -> Outputs {
    let mut f = common::frozen();
    let mut matrix = BTreeMap::new();
    for b in BUDGETS {
        for (i, q) in f.queries.iter().enumerate() {
            let search = |extra: Value| {
                let mut v = json!({"query": q, "budget_tokens": b, "k": K, "fresh": true});
                for (key, x) in extra.as_object().expect("object") {
                    v[key] = x.clone();
                }
                v
            };
            for tool in ["rag_search", "seal_recall"] {
                let c = f.session.text(tool, search(json!({})));
                let j = f.session.text(tool, search(json!({"format": "json", "ladder": true})));
                matrix.insert((tool.to_string(), b, i), (c, j));
            }
            let v = json!({"question": q, "budget_tokens": b, "k": K});
            let c = f.session.text("view", v.clone());
            let j = f.session.text("view", json!({"question": q, "budget_tokens": b, "k": K, "format": "json"}));
            matrix.insert(("view".to_string(), b, i), (c, j));
        }
        for (j, focus) in std::iter::once(String::new()).chain(f.crates.clone()).enumerate() {
            let mut a = json!({"budget_tokens": b});
            if !focus.is_empty() {
                a["focus"] = json!(focus);
            }
            let c = f.session.text("soul", a.clone());
            a["format"] = json!("json");
            let jt = f.session.text("soul", a);
            matrix.insert(("soul".to_string(), b, j), (c, jt));
        }
    }
    f.session.text("session_reset", json!({}));
    let session = f
        .queries
        .iter()
        .map(|q| f.session.text("rag_search", json!({"query": q, "budget_tokens": SESSION_BUDGET, "k": K})))
        .collect();
    Outputs { matrix, session }
}

fn outputs() -> &'static Outputs {
    static CELL: OnceLock<Outputs> = OnceLock::new();
    CELL.get_or_init(collect)
}

fn truth_of(tool: &str, json_text: &str) -> Decoded {
    if tool == "soul" {
        soul_truth(json_text)
    } else {
        ladder_truth(json_text)
    }
}

#[test]
fn every_frozen_query_round_trips_through_the_compact_decoder_at_three_budgets() {
    let o = outputs();
    let mut checked = BTreeMap::new();
    for ((tool, b, i), (compact, json_text)) in &o.matrix {
        let got = decode(compact).unwrap_or_else(|e| panic!("{tool} b={b} #{i}: {e}\n{compact}"));
        let want = truth_of(tool, json_text);
        assert_eq!(got.omitted, want.omitted, "{tool} b={b} #{i} omitted");
        assert_eq!(got.announced, got.cites.len());
        assert_eq!(got.cites, want.cites, "{tool} b={b} #{i}\n{compact}");
        *checked.entry((tool.clone(), *b)).or_insert(0usize) += got.cites.len();
    }
    for tool in TOOLS {
        for b in BUDGETS {
            let n = checked.get(&(tool.to_string(), b)).copied().unwrap_or(0);
            assert!(n > 0, "{tool} {b} decoded no hits");
        }
    }
    let queries = o.matrix.keys().filter(|(t, ..)| t == "rag_search").count();
    assert_eq!(queries, 300);
}

#[test]
fn a_session_pass_decodes_and_every_reference_points_at_an_earlier_citation() {
    let o = outputs();
    let mut delivered: HashMap<u64, (String, u32, u32)> = HashMap::new();
    let mut refs = 0;
    for (i, text) in o.session.iter().enumerate() {
        let got = decode(text).unwrap_or_else(|e| panic!("session #{i}: {e}\n{text}"));
        let (_, json_text) = &o.matrix[&("rag_search".to_string(), SESSION_BUDGET, i)];
        let want = ladder_truth(json_text);
        assert_eq!(got.cites.len(), want.cites.len(), "session #{i}");
        let mut fresh = Vec::new();
        for (g, w) in got.cites.iter().zip(&want.cites) {
            assert_eq!((g.id, g.level, g.graph), (w.id, w.level, w.graph), "session #{i}");
            if g.reference {
                refs += 1;
                let earlier = delivered.get(&g.id).unwrap_or_else(|| panic!("reference #{} before delivery", g.id));
                assert_eq!(earlier, &(w.file.clone(), w.start, w.end), "session #{i} id {}", g.id);
            } else {
                assert_eq!((&g.file, g.start, g.end, &g.body), (&w.file, w.start, w.end, &w.body), "session #{i}");
                fresh.push((g.id, (g.file.clone(), g.start, g.end)));
            }
        }
        delivered.extend(fresh);
    }
    assert!(refs > 50, "the session pass produced only {refs} references");
}

#[test]
fn rag_expand_resolves_every_cited_chunk_id() {
    let o = outputs();
    let mut ids: BTreeSet<u64> = BTreeSet::new();
    for (compact, _) in o.matrix.values() {
        ids.extend(decode(compact).expect("decode").cites.iter().map(|c| c.id));
    }
    for text in &o.session {
        ids.extend(decode(text).expect("decode").cites.iter().map(|c| c.id));
    }
    assert!(ids.len() > 1000, "only {} distinct ids", ids.len());
    let mut f = common::frozen();
    for id in &ids {
        let text = f.session.text("rag_expand", json!({"chunk_id": id}));
        assert!(!text.trim().is_empty(), "rag_expand({id}) returned nothing");
    }
}

#[test]
fn the_compact_outputs_stay_below_the_recorded_old_format_by_the_estimate() {
    let o = outputs();
    for (tool, b, old) in OLD_FORMAT_ESTIMATES {
        let total: usize = o
            .matrix
            .iter()
            .filter(|((t, bb, _), _)| t == tool && *bb == b)
            .map(|(_, (compact, _))| estimate_tokens(compact))
            .sum();
        assert!(
            total * 100 <= old * REQUIRED_KEPT_PERCENT,
            "{tool} {b}: {total} estimated tokens against the old format's {old} (must be at most {REQUIRED_KEPT_PERCENT} percent)"
        );
    }
    let session: usize = o.session.iter().map(|t| estimate_tokens(t)).sum();
    assert!(
        session * 100 <= OLD_SESSION_ESTIMATE * REQUIRED_KEPT_PERCENT,
        "session pass: {session} against {OLD_SESSION_ESTIMATE}"
    );
}
