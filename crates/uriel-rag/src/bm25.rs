use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::tokenize::terms;

const K1: f64 = 1.2;
const B: f64 = 0.75;
const VOCAB_ENTRY_BYTES: u64 = 28;
const LIST_HEADER_BYTES: u64 = 24;
const POSTING_BYTES: u64 = 8;
const TERM_ID_BYTES: u64 = 4;
const DOC_BYTES: u64 = 32;

#[derive(Debug, Clone)]
struct DocInfo {
    len: u32,
    terms: Box<[u32]>,
}

#[derive(Debug, Clone, Default)]
pub struct Bm25Index {
    vocab: HashMap<String, u32>,
    postings: Vec<Vec<(u32, u32)>>,
    docs: Vec<Option<DocInfo>>,
    live: usize,
    live_terms: usize,
    total_len: u64,
    bytes: u64,
}

pub(crate) fn idf(n: f64, df: f64) -> f64 {
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
}

pub(crate) fn term_weight(idf: f64, tf: u32, doc_len: u32, avg_len: f64) -> f64 {
    let tf = f64::from(tf);
    let norm = tf + K1 * (1.0 - B + B * f64::from(doc_len) / avg_len);
    idf * tf * (K1 + 1.0) / norm
}

pub(crate) fn average_len(total_len: u64, live: usize) -> f64 {
    (total_len as f64 / live as f64).max(1.0)
}

impl Bm25Index {
    pub fn new() -> Bm25Index {
        Bm25Index::default()
    }

    pub fn add(&mut self, text: &str) -> u32 {
        let slot = u32::try_from(self.docs.len()).unwrap_or(u32::MAX);
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        let mut len = 0u32;
        for t in terms(text) {
            *counts.entry(t).or_insert(0) += 1;
            len += 1;
        }
        let mut ids: Vec<u32> = Vec::with_capacity(counts.len());
        for (t, tf) in counts {
            let next = u32::try_from(self.postings.len()).unwrap_or(u32::MAX);
            let term_bytes = t.len() as u64 + VOCAB_ENTRY_BYTES + LIST_HEADER_BYTES;
            let tid = *self.vocab.entry(t).or_insert(next);
            if tid == next {
                self.postings.push(Vec::new());
                self.bytes += term_bytes;
            }
            let list = &mut self.postings[tid as usize];
            if list.is_empty() {
                self.live_terms += 1;
            }
            list.push((slot, tf));
            self.bytes += POSTING_BYTES;
            ids.push(tid);
        }
        self.bytes += ids.len() as u64 * TERM_ID_BYTES + DOC_BYTES;
        self.total_len += u64::from(len);
        self.live += 1;
        self.docs.push(Some(DocInfo {
            len,
            terms: ids.into_boxed_slice(),
        }));
        slot
    }

    pub fn remove(&mut self, slot: u32) -> bool {
        let Some(info) = self.docs.get_mut(slot as usize).and_then(Option::take) else {
            return false;
        };
        for tid in info.terms.iter() {
            let list = &mut self.postings[*tid as usize];
            list.retain(|(s, _)| *s != slot);
            self.bytes -= POSTING_BYTES + TERM_ID_BYTES;
            if list.is_empty() {
                self.live_terms -= 1;
            }
        }
        self.bytes -= DOC_BYTES;
        self.total_len -= u64::from(info.len);
        self.live -= 1;
        true
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn term_count(&self) -> usize {
        self.live_terms
    }

    pub fn approx_bytes(&self) -> u64 {
        self.bytes
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(u32, f32)> {
        if self.live == 0 || limit == 0 {
            return Vec::new();
        }
        let unique: BTreeSet<String> = terms(query).into_iter().collect();
        let n = self.live as f64;
        let avg = average_len(self.total_len, self.live);
        let mut acc: HashMap<u32, f64> = HashMap::new();
        for t in &unique {
            let Some(&tid) = self.vocab.get(t) else {
                continue;
            };
            let list = &self.postings[tid as usize];
            if list.is_empty() {
                continue;
            }
            let idf = idf(n, list.len() as f64);
            for &(slot, tf) in list {
                let Some(Some(doc)) = self.docs.get(slot as usize) else {
                    continue;
                };
                *acc.entry(slot).or_insert(0.0) += term_weight(idf, tf, doc.len, avg);
            }
        }
        let mut out: Vec<(u32, f32)> = acc
            .into_iter()
            .filter(|(_, s)| *s > 0.0)
            .map(|(slot, s)| (slot, s as f32))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out.truncate(limit);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_matching_documents_and_supports_removal() {
        let mut ix = Bm25Index::new();
        let a = ix.add("rolling checksum over a byte buffer");
        let b = ix.add("prune the roses in winter");
        let c = ix.add("checksum checksum checksum of everything");
        let hits = ix.search("checksum buffer", 10);
        assert_eq!(hits[0].0, a);
        assert!(hits.iter().any(|h| h.0 == c));
        assert!(hits.iter().all(|h| h.0 != b));
        assert!(ix.remove(a));
        assert!(!ix.remove(a));
        assert_eq!(ix.len(), 2);
        let hits = ix.search("checksum buffer", 10);
        assert_eq!(hits[0].0, c);
        assert!(ix.remove(b));
        assert!(ix.remove(c));
        assert!(ix.is_empty());
        assert_eq!(ix.term_count(), 0, "no postings left behind");
    }

    #[test]
    fn ties_break_by_slot() {
        let mut ix = Bm25Index::new();
        let a = ix.add("same words here");
        let b = ix.add("same words here");
        let hits = ix.search("same words", 5);
        assert_eq!((hits[0].0, hits[1].0), (a, b));
        assert_eq!(hits[0].1, hits[1].1);
    }
}
