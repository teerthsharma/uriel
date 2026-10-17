use uriel_core::{Node, ReadTx, Value};

use crate::bm25::{average_len, idf, term_weight};
use crate::embed::{Embedder, QVec};
use crate::index::{candidates, fuse};
use crate::tokenize::terms;
use crate::RagError;

pub(crate) fn utility_of(node: &Node) -> f32 {
    match node.props.get("utility") {
        Some(Value::Float(u)) => (*u as f32).clamp(-1.0, 1.0),
        _ => 0.0,
    }
}

pub(crate) fn vector_of(node: &Node, mem: &str, embedder: &dyn Embedder) -> Vec<f32> {
    match node.props.get("vec") {
        Some(Value::Vector(v)) if v.len() == embedder.dim() => v.clone(),
        _ => embedder.embed(mem),
    }
}

fn mem_of(node: &Node) -> Option<&str> {
    match node.props.get("mem") {
        Some(Value::String(s)) => Some(s),
        _ => None,
    }
}

fn query_terms(query: &str) -> Vec<String> {
    let mut t = terms(query);
    t.sort();
    t.dedup();
    t
}

fn term_counts(doc_terms: &[String], query: &[String]) -> Vec<u32> {
    let mut tfs = vec![0u32; query.len()];
    for t in doc_terms {
        if let Ok(pos) = query.binary_search(t) {
            tfs[pos] += 1;
        }
    }
    tfs
}

pub(crate) fn direct(
    read: &ReadTx,
    embedder: &dyn Embedder,
    query: &str,
    qvec: &[f32],
    k: usize,
    use_utility: bool,
) -> Result<Vec<(u64, f32)>, RagError> {
    if k == 0 {
        return Ok(Vec::new());
    }
    let cand = candidates(k);
    let qterms = query_terms(query);
    let mut df = vec![0usize; qterms.len()];
    let mut ids: Vec<u64> = Vec::new();
    let mut total_len = 0u64;
    let mut live = 0usize;
    let mut vecs: Vec<(u32, f32)> = Vec::new();
    for node in read.nodes_by_label("Chunk")? {
        let node = node?;
        let idx = u32::try_from(ids.len()).unwrap_or(u32::MAX);
        ids.push(node.id.0);
        let Some(mem) = mem_of(&node) else {
            continue;
        };
        let doc_terms = terms(mem);
        live += 1;
        total_len += doc_terms.len() as u64;
        for (pos, tf) in term_counts(&doc_terms, &qterms).into_iter().enumerate() {
            df[pos] += usize::from(tf > 0);
        }
        let v = vector_of(&node, mem, embedder);
        let s = QVec::from_f32(&v).dot(qvec);
        if s > 0.0 {
            vecs.push((idx, s));
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    vecs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    vecs.truncate(cand);

    let mut bm: Vec<(u32, f32)> = Vec::new();
    if live > 0 && df.iter().any(|d| *d > 0) {
        let n = live as f64;
        let avg = average_len(total_len, live);
        let idfs: Vec<f64> = df.iter().map(|d| idf(n, *d as f64)).collect();
        for (idx, node) in read.nodes_by_label("Chunk")?.enumerate() {
            let node = node?;
            let Some(mem) = mem_of(&node) else {
                continue;
            };
            let doc_terms = terms(mem);
            let len = u32::try_from(doc_terms.len()).unwrap_or(u32::MAX);
            let mut acc = 0.0f64;
            let mut matched = false;
            for (pos, tf) in term_counts(&doc_terms, &qterms).into_iter().enumerate() {
                if tf > 0 {
                    matched = true;
                    acc += term_weight(idfs[pos], tf, len, avg);
                }
            }
            if matched && acc > 0.0 {
                bm.push((u32::try_from(idx).unwrap_or(u32::MAX), acc as f32));
            }
        }
        bm.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        bm.truncate(cand);
    }
    let to_id = |(idx, _): (u32, f32)| ids.get(idx as usize).copied();
    let bm_ids: Vec<u64> = bm.into_iter().filter_map(to_id).collect();
    let vec_ids: Vec<u64> = vecs.into_iter().filter_map(to_id).collect();
    Ok(fuse(&bm_ids, &vec_ids, k, use_utility, |id| {
        read.get_node(uriel_core::NodeId(id))
            .ok()
            .flatten()
            .map(|n| utility_of(&n))
    }))
}

pub(crate) fn totals(read: &ReadTx) -> Result<(usize, u64, u64), RagError> {
    let (mut chunks, mut text, mut mem) = (0usize, 0u64, 0u64);
    for node in read.nodes_by_label("Chunk")? {
        let node = node?;
        chunks += 1;
        if let Some(Value::String(t)) = node.props.get("text") {
            text += t.len() as u64;
        }
        if let Some(m) = mem_of(&node) {
            mem += m.len() as u64;
        }
    }
    Ok((chunks, text, mem))
}
