use std::path::Path;

use crate::error::{Error, Result};
use crate::rng::SplitMix64;

const NODES_PER_PAGE: usize = 64;
const ENTRIES_PER_LINE: usize = 16;
const ENTRIES_PER_PAGE: usize = 1024;

#[derive(Debug, Clone)]
pub struct Csr {
    pub offsets: Vec<usize>,
    pub targets: Vec<u32>,
}

impl Csr {
    pub fn from_edges(n: usize, edges: &[(u32, u32)]) -> Result<Csr> {
        let mut pairs: Vec<(u32, u32)> = Vec::with_capacity(edges.len() * 2);
        for &(a, b) in edges {
            if a as usize >= n || b as usize >= n {
                return Err(Error::OutOfRange(format!("edge ({a}, {b}) with {n} nodes")));
            }
            if a != b {
                pairs.push((a, b));
                pairs.push((b, a));
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let mut offsets = vec![0usize; n + 1];
        for &(a, _) in &pairs {
            offsets[a as usize + 1] += 1;
        }
        for i in 0..n {
            offsets[i + 1] += offsets[i];
        }
        Ok(Csr {
            offsets,
            targets: pairs.into_iter().map(|p| p.1).collect(),
        })
    }

    pub fn len(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn neighbors(&self, u: u32) -> &[u32] {
        &self.targets[self.offsets[u as usize]..self.offsets[u as usize + 1]]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Traffic {
    pub nodes: f64,
    pub node_pages: f64,
    pub adj_lines: f64,
    pub adj_pages: f64,
}

struct Stamps {
    seen: Vec<u32>,
    epoch: u32,
}

impl Stamps {
    fn new(len: usize) -> Self {
        Self {
            seen: vec![0; len],
            epoch: 0,
        }
    }
    fn next(&mut self) {
        self.epoch += 1;
    }
    fn mark(&mut self, i: usize) -> bool {
        if self.seen[i] == self.epoch {
            false
        } else {
            self.seen[i] = self.epoch;
            true
        }
    }
}

pub fn two_hop_traffic(g: &Csr, position: &[u32], sources: &[u32]) -> Result<Traffic> {
    let n = g.len();
    if position.len() != n {
        return Err(Error::Shape(format!(
            "{} positions for {n} nodes",
            position.len()
        )));
    }
    let mut order = vec![u32::MAX; n];
    for (u, &p) in position.iter().enumerate() {
        if p as usize >= n || order[p as usize] != u32::MAX {
            return Err(Error::InvalidParameter(
                "position is not a permutation".into(),
            ));
        }
        order[p as usize] = u as u32;
    }
    if let Some(&s) = sources.iter().find(|&&s| s as usize >= n) {
        return Err(Error::OutOfRange(format!("source {s} with {n} nodes")));
    }
    let mut start = vec![0usize; n];
    let mut acc = 0usize;
    for &u in &order {
        start[u as usize] = acc;
        acc += g.neighbors(u).len();
    }
    let entries = acc;
    let mut node_seen = Stamps::new(n);
    let mut page_seen = Stamps::new(n / NODES_PER_PAGE + 1);
    let mut line_seen = Stamps::new(entries / ENTRIES_PER_LINE + 1);
    let mut apage_seen = Stamps::new(entries / ENTRIES_PER_PAGE + 1);
    let mut total = [0f64; 4];
    for &s in sources {
        node_seen.next();
        page_seen.next();
        line_seen.next();
        apage_seen.next();
        let mut c = [0usize; 4];
        let touch = |u: u32, node_seen: &mut Stamps, page_seen: &mut Stamps, c: &mut [usize; 4]| {
            if node_seen.mark(u as usize) {
                c[0] += 1;
            }
            if page_seen.mark(position[u as usize] as usize / NODES_PER_PAGE) {
                c[1] += 1;
            }
        };
        let read = |u: u32, line_seen: &mut Stamps, apage_seen: &mut Stamps, c: &mut [usize; 4]| {
            let (lo, hi) = (start[u as usize], start[u as usize] + g.neighbors(u).len());
            for idx in lo..hi {
                if line_seen.mark(idx / ENTRIES_PER_LINE) {
                    c[2] += 1;
                }
                if apage_seen.mark(idx / ENTRIES_PER_PAGE) {
                    c[3] += 1;
                }
            }
        };
        touch(s, &mut node_seen, &mut page_seen, &mut c);
        read(s, &mut line_seen, &mut apage_seen, &mut c);
        for &v in g.neighbors(s) {
            touch(v, &mut node_seen, &mut page_seen, &mut c);
            read(v, &mut line_seen, &mut apage_seen, &mut c);
            for &w in g.neighbors(v) {
                touch(w, &mut node_seen, &mut page_seen, &mut c);
            }
        }
        for i in 0..4 {
            total[i] += c[i] as f64;
        }
    }
    let k = sources.len().max(1) as f64;
    Ok(Traffic {
        nodes: total[0] / k,
        node_pages: total[1] / k,
        adj_lines: total[2] / k,
        adj_pages: total[3] / k,
    })
}

pub fn random_permutation(n: usize, seed: u64) -> Vec<u32> {
    let mut v: Vec<u32> = (0..n as u32).collect();
    SplitMix64::new(seed).shuffle(&mut v);
    v
}

pub fn community_graph(
    n: usize,
    k: usize,
    seed: u64,
    interleaved: bool,
) -> (usize, Vec<(u32, u32)>, Vec<u32>) {
    community_graph_mixing(n, k, seed, interleaved, 0.92)
}

pub fn community_graph_mixing(
    n: usize,
    k: usize,
    seed: u64,
    interleaved: bool,
    p_inside: f64,
) -> (usize, Vec<(u32, u32)>, Vec<u32>) {
    let k = k.max(1);
    let mut rng = SplitMix64::new(seed);
    let comm: Vec<u32> = (0..n)
        .map(|u| {
            if interleaved {
                rng.below(k) as u32
            } else {
                (u * k / n.max(1)) as u32
            }
        })
        .collect();
    let w: Vec<f64> = (0..n)
        .map(|_| (2.0 * (1.0 - rng.next_f64()).powf(-1.0 / 1.5)).min(400.0))
        .collect();
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); k];
    for u in 0..n {
        members[comm[u] as usize].push(u as u32);
    }
    let cum: Vec<Vec<f64>> = members
        .iter()
        .map(|m| {
            let mut acc = 0.0;
            m.iter()
                .map(|&u| {
                    acc += w[u as usize];
                    acc
                })
                .collect()
        })
        .collect();
    let pick = |rng: &mut SplitMix64, c: usize| -> Option<u32> {
        let cu = &cum[c];
        let total = *cu.last()?;
        let x = rng.next_f64() * total;
        let i = cu.partition_point(|&v| v <= x).min(cu.len() - 1);
        Some(members[c][i])
    };
    let mut edges = Vec::new();
    for u in 0..n {
        let stubs = ((w[u] / 2.0).round() as usize).max(1);
        for _ in 0..stubs {
            let c = if k > 1 && rng.next_f64() >= p_inside {
                let other = rng.below(k - 1);
                if other >= comm[u] as usize {
                    other + 1
                } else {
                    other
                }
            } else {
                comm[u] as usize
            };
            if let Some(v) = pick(&mut rng, c) {
                if v as usize != u {
                    edges.push((u as u32, v));
                }
            }
        }
    }
    (n, edges, comm)
}

pub type CallGraph = (Vec<String>, Vec<(u32, u32)>);

pub fn source_call_graph(root: &Path) -> Result<CallGraph> {
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let crates = root.join("crates");
    let mut dirs: Vec<_> = std::fs::read_dir(&crates)
        .map_err(|e| Error::Io(format!("{}: {e}", crates.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    dirs.sort();
    for d in dirs {
        collect_rs(&d.join("src"), &mut files)?;
    }
    files.sort();
    let mut names: Vec<String> = Vec::new();
    let mut index: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    let mut bodies: Vec<(u32, Vec<String>)> = Vec::new();
    for f in &files {
        let text =
            std::fs::read_to_string(f).map_err(|e| Error::Io(format!("{}: {e}", f.display())))?;
        let code = blank_comments_and_strings(&text);
        for (name, calls) in function_bodies(&code) {
            let id = *index.entry(name.clone()).or_insert_with(|| {
                names.push(name.clone());
                (names.len() - 1) as u32
            });
            bodies.push((id, calls));
        }
    }
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for (caller, calls) in bodies {
        for c in calls {
            if let Some(&callee) = index.get(&c) {
                if callee != caller {
                    edges.push((caller, callee));
                }
            }
        }
    }
    Ok((names, edges))
}

pub(crate) fn collect_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_rs(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
    Ok(())
}

pub(crate) fn blank_comments_and_strings(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(if b[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' && i + 1 < b.len() {
                    out.push(' ');
                    i += 1;
                }
                out.push(if b[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' && b.get(i + 2) == Some(&'\'') {
            out.push_str("   ");
            i += 3;
        } else if c == '\'' && next == Some('\\') {
            let mut j = i + 2;
            while j < b.len() && b[j] != '\'' && j < i + 12 {
                j += 1;
            }
            for _ in i..=j.min(b.len() - 1) {
                out.push(' ');
            }
            i = j + 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn function_bodies(code: &str) -> Vec<(String, Vec<String>)> {
    let c: Vec<char> = code.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if is_ident_start(c[i]) && (i == 0 || !is_ident(c[i - 1])) {
            let s = i;
            while i < c.len() && is_ident(c[i]) {
                i += 1;
            }
            let word: String = c[s..i].iter().collect();
            if word != "fn" {
                continue;
            }
            while i < c.len() && c[i].is_whitespace() {
                i += 1;
            }
            let ns = i;
            while i < c.len() && is_ident(c[i]) {
                i += 1;
            }
            if ns == i {
                continue;
            }
            let name: String = c[ns..i].iter().collect();
            let mut depth = 0i32;
            let mut j = i;
            let mut body_start = None;
            while j < c.len() {
                match c[j] {
                    '(' | '[' => depth += 1,
                    ')' | ']' => depth -= 1,
                    '{' if depth <= 0 => {
                        body_start = Some(j);
                        break;
                    }
                    ';' if depth <= 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let Some(bs) = body_start else {
                continue;
            };
            let mut level = 0i32;
            let mut k = bs;
            while k < c.len() {
                match c[k] {
                    '{' => level += 1,
                    '}' => {
                        level -= 1;
                        if level == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            let end = k.min(c.len());
            let mut calls = Vec::new();
            let mut m = bs;
            while m < end {
                if is_ident_start(c[m]) && !is_ident(c[m - 1]) {
                    let s2 = m;
                    while m < end && is_ident(c[m]) {
                        m += 1;
                    }
                    let mut p = m;
                    while p < end && c[p] == ' ' {
                        p += 1;
                    }
                    if p < end && c[p] == '(' {
                        calls.push(c[s2..m].iter().collect());
                    }
                } else {
                    m += 1;
                }
            }
            out.push((name, calls));
        }
        i += 1;
    }
    out
}
