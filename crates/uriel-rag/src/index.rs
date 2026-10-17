use std::collections::{HashMap, HashSet};

use crate::bm25::Bm25Index;
use crate::chunk::{rust_item_start, Lang};
use crate::embed::QVec;
use crate::tokenize::raw_words;

const RRF_K: f32 = 60.0;
pub(crate) const HOP_DECAY: f32 = 0.5;
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
    pub vector: QVec,
    pub utility: f32,
    pub lang: Lang,
    pub text_bytes: u32,
    pub mem_len: u32,
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
    mention_bytes: u64,
}

const BY_ID_ENTRY_BYTES: u64 = 40;
const MENTION_KEY_BYTES: u64 = 56;
const MENTION_SLOT_BYTES: u64 = 4;
const SLACK_NUM: u64 = 5;
const SLACK_DEN: u64 = 4;

pub(crate) fn fuse(
    bm: &[u64],
    vecs: &[u64],
    k: usize,
    use_utility: bool,
    utility: impl Fn(u64) -> Option<f32>,
) -> Vec<(u64, f32)> {
    let mut fused: HashMap<u64, f32> = HashMap::new();
    for (rank, id) in bm.iter().enumerate() {
        *fused.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
    for (rank, id) in vecs.iter().enumerate() {
        *fused.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
    }
    let mut out: Vec<(u64, f32)> = fused
        .into_iter()
        .filter_map(|(id, score)| {
            let u = utility(id)?;
            let score = if use_utility {
                score * (1.0 + UTILITY_WEIGHT * u)
            } else {
                score
            };
            Some((id, score))
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(k);
    out
}

pub(crate) fn candidates(k: usize) -> usize {
    (k * 4).max(50)
}

impl Index {
    pub(crate) fn len(&self) -> usize {
        self.live
    }

    pub(crate) fn contains(&self, id: u64) -> bool {
        self.by_id.contains_key(&id)
    }

    pub(crate) fn get(&self, id: u64) -> Option<&Entry> {
        let slot = *self.by_id.get(&id)?;
        self.slots.get(slot as usize)?.as_ref()
    }

    pub(crate) fn get_mut(&mut self, id: u64) -> Option<&mut Entry> {
        let slot = *self.by_id.get(&id)?;
        self.slots.get_mut(slot as usize)?.as_mut()
    }

    pub(crate) fn insert(&mut self, entry: Entry, memory: &str, mentions: Vec<String>) {
        let slot = self.bm25.add(memory);
        let idx = slot as usize;
        if self.slots.len() <= idx {
            self.slots.resize_with(idx + 1, || None);
        }
        if entry.lang == Lang::Rust {
            for w in mentions {
                match self.mentions.get_mut(&w) {
                    Some(list) => list.push(slot),
                    None => {
                        self.mention_bytes += w.len() as u64 + MENTION_KEY_BYTES;
                        self.mentions.insert(w, vec![slot]);
                    }
                }
                self.mention_bytes += MENTION_SLOT_BYTES;
            }
        }
        self.by_id.insert(entry.id, slot);
        self.text_bytes += u64::from(entry.text_bytes);
        self.mem_bytes += u64::from(entry.mem_len);
        self.vec_bytes += entry.vector.resident_bytes();
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
        self.text_bytes -= u64::from(entry.text_bytes);
        self.mem_bytes -= u64::from(entry.mem_len);
        self.vec_bytes -= entry.vector.resident_bytes();
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

    pub(crate) fn vector_bytes(&self) -> u64 {
        self.vec_bytes
    }

    pub(crate) fn resident_bytes(&self) -> u64 {
        let slots = (self.slots.len() * std::mem::size_of::<Option<Entry>>()) as u64;
        let raw = slots
            + self.vec_bytes
            + self.by_id.len() as u64 * BY_ID_ENTRY_BYTES
            + self.bm25.approx_bytes()
            + self.mention_bytes;
        raw / SLACK_DEN * SLACK_NUM
    }

    fn live_slot(&self, slot: u32) -> Option<&Entry> {
        self.slots.get(slot as usize)?.as_ref()
    }

    pub(crate) fn direct(
        &self,
        query: &str,
        qvec: &[f32],
        k: usize,
        use_utility: bool,
    ) -> Vec<(u64, f32)> {
        if k == 0 || self.live == 0 {
            return Vec::new();
        }
        let cand = candidates(k);
        let bm: Vec<u64> = self
            .bm25
            .search(query, cand)
            .into_iter()
            .filter_map(|(slot, _)| self.live_slot(slot).map(|e| e.id))
            .collect();
        let vecs = self.vector_ids(qvec, cand);
        fuse(&bm, &vecs, k, use_utility, |id| self.get(id).map(|e| e.utility))
    }

    pub(crate) fn vector_ids(&self, qvec: &[f32], n: usize) -> Vec<u64> {
        let mut vecs: Vec<(u32, f32)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let s = e.as_ref()?.vector.dot(qvec);
                (s > 0.0).then_some((u32::try_from(i).unwrap_or(u32::MAX), s))
            })
            .collect();
        vecs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        vecs.truncate(n);
        vecs.into_iter()
            .filter_map(|(slot, _)| self.live_slot(slot).map(|e| e.id))
            .collect()
    }

    pub(crate) fn symbol_neighbours(&self, id: u64, defs: &[String]) -> Vec<u64> {
        let Some(&slot) = self.by_id.get(&id) else {
            return Vec::new();
        };
        let mut votes: HashMap<u32, usize> = HashMap::new();
        for d in defs {
            for &m in self.mentions.get(d).into_iter().flatten() {
                if m != slot && self.live_slot(m).is_some() {
                    *votes.entry(m).or_insert(0) += 1;
                }
            }
        }
        let mut syms: Vec<(u32, usize)> = votes.into_iter().collect();
        syms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        syms.truncate(MAX_SYMBOL_NEIGHBOURS);
        syms.into_iter()
            .filter_map(|(s, _)| self.live_slot(s).map(|e| e.id))
            .collect()
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
