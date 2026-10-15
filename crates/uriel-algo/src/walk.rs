//! Seeded random walks over out-arcs.

use crate::error::{Error, Result};
use crate::projection::Projection;
use crate::rng::XorShift64;

/// Parameters for [`random_walks`] (defaults follow common node2vec settings).
#[derive(Debug, Clone)]
pub struct RandomWalkConfig {
    /// Steps per walk; a walk has up to `walk_length + 1` nodes.
    pub walk_length: usize,
    /// Walks started from every node.
    pub walks_per_node: usize,
    /// Seed of the generator shared by all walks.
    pub seed: u64,
    /// Choose arcs proportionally to weight (ignored on unweighted projections).
    pub weighted: bool,
}

impl Default for RandomWalkConfig {
    fn default() -> Self {
        Self {
            walk_length: 80,
            walks_per_node: 10,
            seed: 0,
            weighted: false,
        }
    }
}

/// One random walk of up to `length` steps from `start`, following out-arcs.
///
/// The walk includes `start`. It ends early at a node without usable out-arcs
/// (no arcs, or total weight 0 when `weighted`). Errors for a bad `start` or a
/// negative weight met while `weighted`.
///
/// Complexity: `O(length * max out-degree)` weighted, `O(length)` unweighted.
pub fn random_walk(
    g: &Projection,
    start: u32,
    length: usize,
    rng: &mut XorShift64,
    weighted: bool,
) -> Result<Vec<u32>> {
    g.check_node(start)?;
    let use_weights = weighted && g.is_weighted();
    let mut walk = Vec::with_capacity(length + 1);
    walk.push(start);
    let mut cur = start;
    for _ in 0..length {
        let next = if use_weights {
            let mut total = 0.0;
            for (_, w) in g.out_edges(cur) {
                if w < 0.0 {
                    return Err(Error::InvalidParameter(format!(
                        "negative weight {w} on an arc leaving node {cur}"
                    )));
                }
                total += w;
            }
            if total <= 0.0 {
                break;
            }
            let mut r = rng.next_f64() * total;
            let mut pick = None;
            for (v, w) in g.out_edges(cur) {
                if w > 0.0 {
                    pick = Some(v);
                    if r < w {
                        break;
                    }
                    r -= w;
                }
            }
            match pick {
                Some(v) => v,
                None => break,
            }
        } else {
            let nbrs = g.out_neighbors(cur);
            if nbrs.is_empty() {
                break;
            }
            nbrs[rng.gen_range(nbrs.len() as u64) as usize]
        };
        walk.push(next);
        cur = next;
    }
    Ok(walk)
}

/// `walks_per_node` walks from every node, in node order, from one seeded
/// generator, so the whole corpus is reproducible. Walk `i` starts at node
/// `i / walks_per_node`.
///
/// Complexity: `O(n * walks_per_node * walk_length)`.
pub fn random_walks(g: &Projection, cfg: &RandomWalkConfig) -> Result<Vec<Vec<u32>>> {
    let mut rng = XorShift64::new(cfg.seed);
    let mut out = Vec::with_capacity(g.node_count() * cfg.walks_per_node);
    for u in 0..g.node_count() as u32 {
        for _ in 0..cfg.walks_per_node {
            out.push(random_walk(g, u, cfg.walk_length, &mut rng, cfg.weighted)?);
        }
    }
    Ok(out)
}
