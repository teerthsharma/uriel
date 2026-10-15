use std::collections::{HashMap, HashSet};

use crate::bm25::Bm25Index;
use crate::chunk::{rust_item_start, Lang};
use crate::embed::dot;
use crate::tokenize::raw_words;
use crate::{Hit, HitVia};

const RRF_K: f32 = 60.0;
const HOP_DECAY: f32 = 0.5;
const MAX_SYMBOL_NEIGHBOURS: usize = 3;
pub(crate) const UTILITY_WEIGHT: f32 = 0.25;

const COMMON_NAMES: &[&str] = &[
    "default", "main", "test", "tests", "name", "type", "data", "value", "text", "path", "file",
    "list", "item", "iter", "next", "drop", "read", "write", "open", "close", "size", "kind",
    "mode", "init", "with", "build", "from", "into", "self", "code", "info", "node", "edge",
    "keys", "new", "empty", "clone", "show", "fmt", "none", "some", "true", "false",
];

fn linkable(word: &str) -> bool {
    word.chars().count() >= 4 && !COMMON_NAMES.contains(&word.to_ascii_lowercase().as_str())
}

pub(crate) fn rust_defs(text: &str) -> Vec<String> {
    const KW: &[&str] = &[
        "fn", "struct", "enum", "trait", "type", "const", "static", "mod", "union",
    ];
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if !rust_item_start(line) {
            continue;
        }
        let mut toks = line.split_whitespace();
        while let Some(t) = toks.next() {
            let t = t.trim_start_matches("pub(crate)");
            if KW.contains(&t) {
                let mut name = toks.next();
                if name == Some("fn") {
                    name = toks.next();
                }
                if let Some(name) = name {
                    let name: String = name
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if linkable(&name) && !out.contains(&name) {
                        out.push(name);
                    }
                }
                break;
            }
            if t == "macro_rules!" {
                break;
            }
        }
    }
    out
}

pub(crate) fn rust_mentions(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for w in raw_words(text) {
        if linkable(w) && seen.insert(w) {
            out.push(w.to_string());
        }
    }
    out
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub id: u64,
    pub file: String,
    pub start: u32,
    pub end: u32,
    pub memory: String,
    pub gist: String,
    pub vector: Vec<f32>,
    pub utility: f32,
    pub lang: Lang,
    pub defs: Vec<String>,
    pub mentions: Vec<String>,
    pub prev: Option<u64>,
    pub next: Option<u64>,
    pub text_bytes: usize,
}

#[derive(Debug, Default)]
pub(crate) struct Index {
    bm25: Bm25Index,
    slots: Vec<Option<Entry>>,
    by_id: HashMap<u64, u32>,
    mentions: HashMap<String, Vec<u32>>,
    live: usize,
    text_bytes: u64,
    mem_bytes: u64,
    vec_bytes: u64,
}

impl Index {
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    pub(crate) fn get(&self, id: u64) -> Option<&Entry> {
        let slot = *self.by_id.get(&id)?;
        self.slots.get(slot as usize)?.as_ref()
    }

    pub(crate) fn get_mut(&mut self, id: u64) -> Option<&mut Entry> {
        let slot = *self.by_id.get(&id)?;
        self.slots.get_mut(slot as usize)?.as_mut()
    }

    pub(crate) fn insert(&mut self, entry: Entry) {
        let slot = self.bm25.add(&entry.memory);
        let idx = slot as usize;
        if self.slots.len() <= idx {
            self.slots.resize_with(idx + 1, || None);
        }
        if entry.lang == Lang::Rust {
            for w in &entry.mentions {
                self.mentions.entry(w.clone()).or_default().push(slot);
            }
        }
        self.by_id.insert(entry.id, slot);
        self.text_bytes += entry.text_bytes as u64;
        self.mem_bytes += entry.memory.len() as u64;
        self.vec_bytes += (entry.vector.len() * 4) as u64;
        self.live += 1;
        self.slots[idx] = Some(entry);
    }

    pub(crate) fn remove(&mut self, id: u64) -> bool {
        let Some(slot) = self.by_id.remove(&id) else {
            return false;
        };
        let Some(entry) = self.slots.get_mut(slot as usize).and_then(Option::take) else {
            return false;
        };
        self.bm25.remove(slot);
        if entry.lang == Lang::Rust {
            for w in &entry.mentions {
                if let Some(list) = self.mentions.get_mut(w) {
                    list.retain(|s| *s != slot);
                    if list.is_empty() {
                        self.mentions.remove(w);
                    }
                }
            }
        }
        self.text_bytes -= entry.text_bytes as u64;
        self.mem_bytes -= entry.memory.len() as u64;
        self.vec_bytes -= (entry.vector.len() * 4) as u64;
        self.live -= 1;
        true
    }

    #[cfg(test)]
    pub(crate) fn ids(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.by_id.keys().copied().collect();
        v.sort_unstable();
        v
    }

    pub(crate) fn byte_totals(&self) -> (u64, u64) {
        (self.text_bytes, self.mem_bytes)
    }

    pub(crate) fn memory_bytes(&self) -> u64 {
        self.mem_bytes
    }

    pub(crate) fn index_bytes(&self) -> u64 {
        self.bm25.approx_bytes() + self.mem_bytes + self.vec_bytes
    }

    fn entry_at(&self, slot: u32) -> Option<&Entry> {
        self.slots.get(slot as usize)?.as_ref()
    }

    pub(crate) fn direct(
        &self,
        query: &str,
        qvec: &[f32],
        k: usize,
        use_utility: bool,
    ) -> Vec<(u32, f32)> {
        if k == 0 || self.live == 0 {
            return Vec::new();
        }
        let cand = (k * 4).max(50);
        let bm = self.bm25.search(query, cand);
        let mut vecs: Vec<(u32, f32)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let e = e.as_ref()?;
                let s = if e.vector.len() == qvec.len() {
                    dot(qvec, &e.vector)
                } else {
                    0.0
                };
                (s > 0.0).then_some((u32::try_from(i).unwrap_or(u32::MAX), s))
            })
            .collect();
        vecs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        vecs.truncate(cand);

        let mut fused: HashMap<u32, f32> = HashMap::new();
        for (rank, (slot, _)) in bm.iter().enumerate() {
            *fused.entry(*slot).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        for (rank, (slot, _)) in vecs.iter().enumerate() {
            *fused.entry(*slot).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
        let mut out: Vec<(u32, f32, u64)> = fused
            .into_iter()
            .filter_map(|(slot, score)| {
                let e = self.entry_at(slot)?;
                let score = if use_utility {
                    score * (1.0 + UTILITY_WEIGHT * e.utility)
                } else {
                    score
                };
                Some((slot, score, e.id))
            })
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)));
        out.truncate(k);
        out.into_iter().map(|(s, sc, _)| (s, sc)).collect()
    }

    pub(crate) fn expand(&self, direct: &[(u32, f32)], hops: usize) -> Vec<(u32, f32)> {
        let mut seen: HashSet<u32> = direct.iter().map(|(s, _)| *s).collect();
        let mut frontier: Vec<(u32, f32)> = direct.to_vec();
        let mut out = Vec::new();
        for _ in 0..hops {
            let mut next = Vec::new();
            for &(slot, score) in &frontier {
                let Some(e) = self.entry_at(slot) else {
                    continue;
                };
                let decayed = score * HOP_DECAY;
                let mut cands: Vec<u32> = [e.prev, e.next]
                    .into_iter()
                    .flatten()
                    .filter_map(|id| self.by_id.get(&id).copied())
                    .collect();
                if e.lang == Lang::Rust {
                    let mut votes: HashMap<u32, usize> = HashMap::new();
                    for d in &e.defs {
                        for &m in self.mentions.get(d).into_iter().flatten() {
                            if m != slot {
                                *votes.entry(m).or_insert(0) += 1;
                            }
                        }
                    }
                    let mut syms: Vec<(u32, usize)> = votes.into_iter().collect();
                    syms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                    syms.truncate(MAX_SYMBOL_NEIGHBOURS);
                    cands.extend(syms.into_iter().map(|(s, _)| s));
                }
                for c in cands {
                    if seen.insert(c) {
                        next.push((c, decayed));
                    }
                }
            }
            out.extend(next.iter().copied());
            frontier = next;
        }
        out
    }

    pub(crate) fn hit(&self, slot: u32, score: f32, via: HitVia) -> Option<Hit> {
        let e = self.entry_at(slot)?;
        Some(Hit {
            chunk_id: e.id,
            file: e.file.clone(),
            start_line: e.start,
            end_line: e.end,
            memory: e.memory.clone(),
            score,
            via,
        })
    }

    pub(crate) fn gist_of(&self, id: u64) -> Option<&str> {
        self.get(id).map(|e| e.gist.as_str())
    }

    pub(crate) fn entry_id(&self, slot: u32) -> Option<u64> {
        self.entry_at(slot).map(|e| e.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_defs_extracts_item_names() {
        let defs = rust_defs(
            "/// doc\npub fn compute_checksum(x: u8) {\n}\npub struct PacketHeader;\nimpl Foo {\n    fn new() {}\n}\nconst MAX_SIZE: usize = 3;",
        );
        assert_eq!(defs, vec!["compute_checksum", "PacketHeader", "MAX_SIZE"]);
    }
}
