use std::collections::BTreeMap;

use uriel_core::{Node, NodeId, ReadTx, Value};

use crate::bm25::{average_len, idf, term_weight};
use crate::index::UTILITY_WEIGHT;
use crate::scan::utility_of;
use crate::tokenize::terms;
use crate::RagError;

pub(crate) const RERANK_DEPTH: usize = 100;
const RRF_K: f32 = 60.0;
const FIRST_STAGE_WEIGHT: f32 = 0.5;

fn query_terms(query: &str) -> Vec<String> {
    let mut t = terms(query);
    t.sort();
    t.dedup();
    t
}

const RECORD: usize = 12;
const HEADER: usize = 4;

fn term_hash(term: &str) -> u64 {
    term.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub(crate) fn encode_counts(text: &str) -> Vec<u8> {
    let doc_terms = terms(text);
    let len = u32::try_from(doc_terms.len()).unwrap_or(u32::MAX);
    let mut by_hash: BTreeMap<u64, u32> = BTreeMap::new();
    for t in &doc_terms {
        *by_hash.entry(term_hash(t)).or_insert(0) += 1;
    }
    let mut out = Vec::with_capacity(HEADER + by_hash.len() * RECORD);
    out.extend_from_slice(&len.to_le_bytes());
    for (h, tf) in by_hash {
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&tf.to_le_bytes());
    }
    out
}

fn stored_counts(bytes: &[u8], query: &[String]) -> Option<(Vec<u32>, u32)> {
    let body = bytes.get(HEADER..)?;
    if body.len() % RECORD != 0 {
        return None;
    }
    let len = u32::from_le_bytes(bytes[..HEADER].try_into().ok()?);
    let n = body.len() / RECORD;
    let hash_at = |i: usize| u64::from_le_bytes(body[i * RECORD..i * RECORD + 8].try_into().unwrap_or([0; 8]));
    let tfs = query
        .iter()
        .map(|q| {
            let want = term_hash(q);
            let (mut lo, mut hi) = (0usize, n);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if hash_at(mid) < want {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            if lo < n && hash_at(lo) == want {
                let at = lo * RECORD + 8;
                u32::from_le_bytes(body[at..at + 4].try_into().unwrap_or([0; 4]))
            } else {
                0
            }
        })
        .collect();
    Some((tfs, len))
}

fn doc_counts(node: &Node, query: &[String]) -> (Vec<u32>, u32) {
    if let Some(Value::Bytes(b)) = node.props.get("tc") {
        if let Some(found) = stored_counts(b, query) {
            return found;
        }
    }
    let doc_terms = match node.props.get("text") {
        Some(Value::String(t)) => terms(t),
        _ => Vec::new(),
    };
    let len = u32::try_from(doc_terms.len()).unwrap_or(u32::MAX);
    (counts(&doc_terms, query), len)
}

fn counts(doc_terms: &[String], query: &[String]) -> Vec<u32> {
    let mut tfs = vec![0u32; query.len()];
    for t in doc_terms {
        if let Ok(pos) = query.binary_search(t) {
            tfs[pos] += 1;
        }
    }
    tfs
}

pub(crate) fn rerank(
    read: &ReadTx,
    query: &str,
    first: Vec<(u64, f32)>,
    k: usize,
    use_utility: bool,
) -> Result<Vec<(u64, f32)>, RagError> {
    let qterms = query_terms(query);
    if qterms.is_empty() || first.len() < 2 {
        return Ok(first.into_iter().take(k).collect());
    }
    let mut pool: Vec<(u64, Vec<u32>, u32, f32)> = Vec::with_capacity(first.len());
    for (id, _) in &first {
        let Some(node) = read.get_node(NodeId(*id))? else {
            pool.push((*id, vec![0; qterms.len()], 0, 0.0));
            continue;
        };
        let (tfs, len) = doc_counts(&node, &qterms);
        pool.push((*id, tfs, len, utility_of(&node)));
    }
    let n = pool.len() as f64;
    let total: u64 = pool.iter().map(|p| u64::from(p.2)).sum();
    let avg = average_len(total, pool.len());
    let idfs: Vec<f64> = (0..qterms.len())
        .map(|pos| {
            let df = pool.iter().filter(|p| p.1[pos] > 0).count();
            idf(n, df as f64)
        })
        .collect();
    let scored: Vec<(usize, f64)> = pool
        .iter()
        .enumerate()
        .map(|(rank, p)| {
            let acc: f64 = p
                .1
                .iter()
                .enumerate()
                .filter(|(_, tf)| **tf > 0)
                .map(|(pos, tf)| term_weight(idfs[pos], *tf, p.2, avg))
                .sum();
            (rank, acc)
        })
        .collect();
    let best = scored.iter().map(|s| s.1).fold(0.0f64, f64::max);
    let mut out: Vec<(u64, f32)> = pool
        .iter()
        .zip(&scored)
        .map(|(p, (first_rank, acc))| {
            let lexical = if best > 0.0 { (acc / best) as f32 } else { 0.0 };
            let prior = FIRST_STAGE_WEIGHT * (RRF_K + 1.0) / (RRF_K + *first_rank as f32 + 1.0);
            let fused = lexical + prior;
            let score = if use_utility {
                fused * (1.0 + UTILITY_WEIGHT * p.3)
            } else {
                fused
            };
            (p.0, score)
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out.truncate(k);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "pub fn checksum_stream(buffer: &[u8]) -> u32 {\n    let mut acc = 1;\n    acc = acc.wrapping_mul(31) + buffer[0] as u32;\n    acc\n}\nChecksum of the running buffer";

    #[test]
    fn stored_counts_equal_the_tokenised_counts() {
        let query = query_terms("running checksum of the buffer stream missing_term acc");
        let blob = encode_counts(TEXT);
        let (tfs, len) = stored_counts(&blob, &query).expect("well formed");
        let doc_terms = terms(TEXT);
        assert_eq!(tfs, counts(&doc_terms, &query));
        assert_eq!(len as usize, doc_terms.len());
        assert!(tfs.iter().any(|t| *t > 1));
        assert!(tfs.contains(&0));
    }

    #[test]
    fn a_malformed_blob_is_refused_not_trusted() {
        let query = query_terms("checksum");
        assert!(stored_counts(&[], &query).is_none());
        assert!(stored_counts(&[1, 2, 3], &query).is_none());
        let mut blob = encode_counts(TEXT);
        blob.push(0);
        assert!(stored_counts(&blob, &query).is_none());
        assert_eq!(stored_counts(&encode_counts(""), &query), Some((vec![0], 0)));
    }
}
