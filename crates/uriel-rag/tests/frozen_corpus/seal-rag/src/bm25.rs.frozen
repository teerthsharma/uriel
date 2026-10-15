use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::tokenize::terms;

const K1: f64 = 1.2;
const B: f64 = 0.75;

#[derive(Debug, Clone)]
struct DocInfo {
    len: u32,
    terms: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct Bm25Index {
    postings: HashMap<String, Vec<(u32, u32)>>,
    docs: Vec<Option<DocInfo>>,
    live: usize,
    total_len: u64,
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
        for (t, tf) in &counts {
            self.postings
                .entry(t.clone())
                .or_default()
                .push((slot, *tf));
        }
        self.total_len += u64::from(len);
        self.live += 1;
        self.docs.push(Some(DocInfo {
            len,
            terms: counts.into_iter().collect(),
        }));
        slot
    }

    pub fn remove(&mut self, slot: u32) -> bool {
        let Some(info) = self.docs.get_mut(slot as usize).and_then(Option::take) else {
            return false;
        };
        for (t, _) in &info.terms {
            if let Some(list) = self.postings.get_mut(t) {
                list.retain(|(s, _)| *s != slot);
                if list.is_empty() {
                    self.postings.remove(t);
                }
            }
        }
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
        self.postings.len()
    }

    pub fn approx_bytes(&self) -> u64 {
        let postings: usize = self
            .postings
            .iter()
            .map(|(t, l)| t.len() + l.len() * 8 + 24)
            .sum();
        let docs: usize = self
            .docs
            .iter()
            .flatten()
            .map(|d| d.terms.iter().map(|(t, _)| t.len() + 4 + 24).sum::<usize>() + 32)
            .sum();
        (postings + docs) as u64
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(u32, f32)> {
        if self.live == 0 || limit == 0 {
            return Vec::new();
        }
        let unique: BTreeSet<String> = terms(query).into_iter().collect();
        let n = self.live as f64;
        let avg = (self.total_len as f64 / n).max(1.0);
        let mut acc: HashMap<u32, f64> = HashMap::new();
        for t in &unique {
            let Some(list) = self.postings.get(t) else {
                continue;
            };
            let df = list.len() as f64;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            for &(slot, tf) in list {
                let Some(Some(doc)) = self.docs.get(slot as usize) else {
                    continue;
                };
                let tf = f64::from(tf);
                let norm = tf + K1 * (1.0 - B + B * f64::from(doc.len) / avg);
                *acc.entry(slot).or_insert(0.0) += idf * tf * (K1 + 1.0) / norm;
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
