use uriel_core::{NodeId, ReadTx, Value};

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
        let doc_terms = match node.props.get("text") {
            Some(Value::String(t)) => terms(t),
            _ => Vec::new(),
        };
        let len = u32::try_from(doc_terms.len()).unwrap_or(u32::MAX);
        pool.push((*id, counts(&doc_terms, &qterms), len, utility_of(&node)));
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
