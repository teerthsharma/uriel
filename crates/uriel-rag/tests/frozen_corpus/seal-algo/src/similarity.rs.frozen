//! Node similarity over out-neighbour sets: Jaccard and cosine.
//!
//! The neighbour set of a node is its set of distinct out-neighbours (parallel
//! arcs collapse; for a repeated target the first-inserted weight is used). On
//! undirected projections that is simply the neighbourhood.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::projection::Projection;

/// Similarity measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimilarityMetric {
    /// `|A ∩ B| / |A ∪ B|`.
    Jaccard,
    /// Cosine of the neighbour vectors (binary, or weighted if enabled).
    Cosine,
}

/// Parameters for [`node_similarity`].
#[derive(Debug, Clone)]
pub struct SimilarityConfig {
    /// Measure to compute.
    pub metric: SimilarityMetric,
    /// Keep, per node, only its `k` best pairs (then the union over nodes).
    pub top_k: Option<usize>,
    /// Drop pairs scoring below this.
    pub cutoff: f64,
    /// Cosine only: use arc weights (ignored on unweighted projections).
    pub weighted: bool,
}

impl Default for SimilarityConfig {
    fn default() -> Self {
        Self {
            metric: SimilarityMetric::Jaccard,
            top_k: None,
            cutoff: 0.0,
            weighted: false,
        }
    }
}

/// A scored node pair with `a < b`.
#[derive(Debug, Clone, PartialEq)]
pub struct SimilarityPair {
    /// Smaller node id.
    pub a: u32,
    /// Larger node id.
    pub b: u32,
    /// Similarity in `(0, 1]`.
    pub score: f64,
}

/// Distinct out-neighbours with weights, ascending by id.
fn neighbour_set(g: &Projection, u: u32) -> Vec<(u32, f64)> {
    let mut v: Vec<(u32, f64)> = Vec::with_capacity(g.out_degree(u));
    for (t, w) in g.out_edges(u) {
        if v.last().map(|x| x.0) != Some(t) {
            v.push((t, w));
        }
    }
    v
}

fn merge_stats(a: &[(u32, f64)], b: &[(u32, f64)]) -> (usize, f64) {
    let (mut i, mut j, mut inter, mut dot) = (0, 0, 0usize, 0.0);
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                inter += 1;
                dot += a[i].1 * b[j].1;
                i += 1;
                j += 1;
            }
        }
    }
    (inter, dot)
}

fn finish(
    metric: SimilarityMetric,
    weighted: bool,
    inter: usize,
    dot: f64,
    sa: &[(u32, f64)],
    sb: &[(u32, f64)],
) -> f64 {
    match metric {
        SimilarityMetric::Jaccard => {
            let union = sa.len() + sb.len() - inter;
            if union == 0 {
                0.0
            } else {
                inter as f64 / union as f64
            }
        }
        SimilarityMetric::Cosine => {
            if sa.is_empty() || sb.is_empty() {
                return 0.0;
            }
            if weighted {
                let na: f64 = sa.iter().map(|x| x.1 * x.1).sum::<f64>().sqrt();
                let nb: f64 = sb.iter().map(|x| x.1 * x.1).sum::<f64>().sqrt();
                if na == 0.0 || nb == 0.0 {
                    0.0
                } else {
                    dot / (na * nb)
                }
            } else {
                inter as f64 / ((sa.len() * sb.len()) as f64).sqrt()
            }
        }
    }
}

fn pair_score(
    g: &Projection,
    a: u32,
    b: u32,
    metric: SimilarityMetric,
    weighted: bool,
) -> Result<f64> {
    g.check_node(a)?;
    g.check_node(b)?;
    let (sa, sb) = (neighbour_set(g, a), neighbour_set(g, b));
    let (inter, dot) = merge_stats(&sa, &sb);
    Ok(finish(
        metric,
        weighted && g.is_weighted(),
        inter,
        dot,
        &sa,
        &sb,
    ))
}

/// Jaccard similarity of the out-neighbour sets of `a` and `b` (0 if both empty).
///
/// Complexity: `O(deg(a) + deg(b))`.
pub fn jaccard_similarity(g: &Projection, a: u32, b: u32) -> Result<f64> {
    pair_score(g, a, b, SimilarityMetric::Jaccard, false)
}

/// Cosine similarity of the out-neighbour vectors of `a` and `b`: binary
/// indicator vectors, or arc-weight vectors when `weighted` and the projection
/// is weighted. 0 if either is empty.
///
/// Complexity: `O(deg(a) + deg(b))`.
pub fn cosine_similarity(g: &Projection, a: u32, b: u32, weighted: bool) -> Result<f64> {
    pair_score(g, a, b, SimilarityMetric::Cosine, weighted)
}

/// All node pairs that share at least one out-neighbour, scored and sorted by
/// `(a, b)`.
///
/// Uses an inverted index (neighbour -> nodes pointing to it), so pairs without
/// a common neighbour are never touched. With `top_k`, each node keeps its `k`
/// best pairs (ties broken by lower partner id) and the result is the union.
///
/// Complexity: `O(sum over n of indeg(n)^2)` time, `O(n + m + pairs)` memory.
pub fn node_similarity(g: &Projection, cfg: &SimilarityConfig) -> Result<Vec<SimilarityPair>> {
    if cfg.top_k == Some(0) {
        return Err(Error::InvalidParameter("top_k must be at least 1".into()));
    }
    let n = g.node_count();
    let sets: Vec<Vec<(u32, f64)>> = (0..n as u32).map(|u| neighbour_set(g, u)).collect();
    let mut inverted: HashMap<u32, Vec<(u32, f64)>> = HashMap::new();
    for (u, set) in sets.iter().enumerate() {
        for &(t, w) in set {
            inverted.entry(t).or_default().push((u as u32, w));
        }
    }
    let weighted = cfg.weighted && g.is_weighted();
    let mut all: Vec<SimilarityPair> = Vec::new();
    let mut acc: HashMap<u32, (usize, f64)> = HashMap::new();
    for a in 0..n as u32 {
        acc.clear();
        for &(t, wa) in &sets[a as usize] {
            for &(b, wb) in &inverted[&t] {
                if b > a {
                    let e = acc.entry(b).or_insert((0, 0.0));
                    e.0 += 1;
                    e.1 += wa * wb;
                }
            }
        }
        for (&b, &(inter, dot)) in &acc {
            let score = finish(
                cfg.metric,
                weighted,
                inter,
                dot,
                &sets[a as usize],
                &sets[b as usize],
            );
            if score > 0.0 && score >= cfg.cutoff {
                all.push(SimilarityPair { a, b, score });
            }
        }
    }
    if let Some(k) = cfg.top_k {
        let mut per_node: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, p) in all.iter().enumerate() {
            per_node[p.a as usize].push(i);
            per_node[p.b as usize].push(i);
        }
        let mut keep = vec![false; all.len()];
        for (u, list) in per_node.iter_mut().enumerate() {
            let partner = |i: usize| {
                if all[i].a as usize == u {
                    all[i].b
                } else {
                    all[i].a
                }
            };
            list.sort_by(|&x, &y| {
                all[y]
                    .score
                    .total_cmp(&all[x].score)
                    .then_with(|| partner(x).cmp(&partner(y)))
            });
            for &i in list.iter().take(k) {
                keep[i] = true;
            }
        }
        all = all
            .into_iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(p, _)| p)
            .collect();
    }
    all.sort_by_key(|p| (p.a, p.b));
    Ok(all)
}
