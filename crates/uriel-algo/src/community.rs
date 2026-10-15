//! Community detection: modularity, Louvain and label propagation.
//!
//! Both detectors see the projection as an undirected multigraph: every arc of a
//! directed projection is one undirected edge, and parallel edges add their
//! weights. Community ids returned are dense (`0..k`) and numbered by the lowest
//! node id they contain, so results are easy to compare.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::projection::Projection;
use crate::rng::XorShift64;

/// Logical edges `(u, v, w)` with unit weights unless `weighted`.
fn edge_list(g: &Projection, weighted: bool) -> Result<Vec<(u32, u32, f64)>> {
    let use_weights = weighted && g.is_weighted();
    let mut edges = g.edges();
    if use_weights {
        if let Some(&(u, v, w)) = edges.iter().find(|e| e.2 < 0.0) {
            return Err(Error::InvalidParameter(format!(
                "negative weight {w} on edge {u} - {v}: modularity needs non-negative weights"
            )));
        }
    } else {
        for e in &mut edges {
            e.2 = 1.0;
        }
    }
    Ok(edges)
}

/// Renumbers labels densely in order of first appearance.
fn densify(labels: &[u32]) -> Vec<u32> {
    let mut map: HashMap<u32, u32> = HashMap::new();
    labels
        .iter()
        .map(|&l| {
            let next = map.len() as u32;
            *map.entry(l).or_insert(next)
        })
        .collect()
}

/// Newman modularity `Q = sum_c [ in_c / 2m - (tot_c / 2m)^2 ]` of a partition.
///
/// `communities[u]` is the community label of node `u` (any `u32` values). Arcs
/// are treated as undirected edges; a self loop of weight `w` adds `2w` to both
/// its community's internal and total degree. Returns 0 for a graph without
/// edge weight. With `weighted == false` (or on unweighted projections) every
/// edge has weight 1.
///
/// Errors if `communities.len() != node_count` or a used weight is negative.
///
/// Complexity: `O(n + m)`.
pub fn modularity(g: &Projection, communities: &[u32], weighted: bool) -> Result<f64> {
    if communities.len() != g.node_count() {
        return Err(Error::InvalidParameter(format!(
            "expected {} community labels, got {}",
            g.node_count(),
            communities.len()
        )));
    }
    let edges = edge_list(g, weighted)?;
    let m: f64 = edges.iter().map(|e| e.2).sum();
    if m <= 0.0 {
        return Ok(0.0);
    }
    let dense = densify(communities);
    let k = dense.iter().copied().max().map_or(0, |x| x as usize + 1);
    let mut inner = vec![0.0; k];
    let mut total = vec![0.0; k];
    for &(u, v, w) in &edges {
        let (cu, cv) = (dense[u as usize] as usize, dense[v as usize] as usize);
        total[cu] += w;
        total[cv] += w;
        if cu == cv {
            inner[cu] += 2.0 * w;
        }
    }
    let m2 = 2.0 * m;
    Ok((0..k)
        .map(|c| inner[c] / m2 - (total[c] / m2).powi(2))
        .sum())
}

/// Louvain parameters (defaults follow GDS: 10 levels, 10 iterations per level,
/// tolerance 1e-7, resolution 1).
#[derive(Debug, Clone)]
pub struct LouvainConfig {
    /// Resolution `gamma > 0`; larger values favour smaller communities.
    pub resolution: f64,
    /// Maximum number of aggregation levels (at least 1).
    pub max_levels: usize,
    /// Maximum local-moving sweeps per level (at least 1).
    pub max_iterations: usize,
    /// A node only moves if the modularity gain exceeds this.
    pub tolerance: f64,
    /// Use arc weights (ignored on unweighted projections).
    pub weighted: bool,
}

impl Default for LouvainConfig {
    fn default() -> Self {
        Self {
            resolution: 1.0,
            max_levels: 10,
            max_iterations: 10,
            tolerance: 1e-7,
            weighted: false,
        }
    }
}

/// Louvain output.
#[derive(Debug, Clone, PartialEq)]
pub struct LouvainResult {
    /// Dense community id per node.
    pub communities: Vec<u32>,
    /// Modularity of `communities` (at resolution 1), as [`modularity`] reports.
    pub modularity: f64,
    /// Number of levels in which at least one node moved.
    pub levels: usize,
}

/// One level of the Louvain hierarchy: a weighted undirected multigraph without
/// loops in `adj`; loop weights live in `loops` (a loop adds `2w` to the degree).
struct Level {
    adj: Vec<Vec<(u32, f64)>>,
    loops: Vec<f64>,
    deg: Vec<f64>,
}

impl Level {
    fn from_edges(n: usize, edges: &[(u32, u32, f64)]) -> Self {
        let mut adj = vec![Vec::new(); n];
        let mut loops = vec![0.0; n];
        for &(u, v, w) in edges {
            if u == v {
                loops[u as usize] += w;
            } else {
                adj[u as usize].push((v, w));
                adj[v as usize].push((u, w));
            }
        }
        let deg = (0..n)
            .map(|i| adj[i].iter().map(|x| x.1).sum::<f64>() + 2.0 * loops[i])
            .collect();
        Self { adj, loops, deg }
    }

    fn n(&self) -> usize {
        self.deg.len()
    }

    /// Local moving phase; returns dense community per node and whether any node moved.
    fn local_moving(&self, cfg: &LouvainConfig, m2: f64) -> (Vec<u32>, bool) {
        let n = self.n();
        let mut comm: Vec<u32> = (0..n as u32).collect();
        let mut tot = self.deg.clone();
        let mut nw = vec![0.0f64; n];
        let mut seen = vec![false; n];
        let mut touched: Vec<u32> = Vec::new();
        let threshold = cfg.tolerance * (m2 / 2.0);
        let mut moved_any = false;
        for _ in 0..cfg.max_iterations {
            let mut moved = false;
            for i in 0..n {
                let ci = comm[i];
                touched.clear();
                for &(j, w) in &self.adj[i] {
                    let cj = comm[j as usize];
                    if !seen[cj as usize] {
                        seen[cj as usize] = true;
                        touched.push(cj);
                    }
                    nw[cj as usize] += w;
                }
                let ki = self.deg[i];
                tot[ci as usize] -= ki;
                let gain = |c: u32, nw: &[f64], tot: &[f64]| {
                    nw[c as usize] - cfg.resolution * tot[c as usize] * ki / m2
                };
                let mut best = ci;
                let mut best_gain = gain(ci, &nw, &tot);
                for &c in &touched {
                    let gc = gain(c, &nw, &tot);
                    if gc > best_gain + threshold {
                        best = c;
                        best_gain = gc;
                    }
                }
                tot[best as usize] += ki;
                comm[i] = best;
                if best != ci {
                    moved = true;
                }
                for &c in &touched {
                    nw[c as usize] = 0.0;
                    seen[c as usize] = false;
                }
            }
            if !moved {
                break;
            }
            moved_any = true;
        }
        (densify(&comm), moved_any)
    }

    /// Collapses each community into one node.
    fn aggregate(&self, comm: &[u32]) -> Level {
        let k = comm.iter().copied().max().map_or(0, |x| x as usize + 1);
        let mut loops = vec![0.0; k];
        let mut cross: Vec<(u32, u32, f64)> = Vec::new();
        for i in 0..self.n() {
            let ci = comm[i];
            loops[ci as usize] += self.loops[i];
            for &(j, w) in &self.adj[i] {
                let cj = comm[j as usize];
                if ci == cj {
                    if (j as usize) > i {
                        loops[ci as usize] += w;
                    }
                } else {
                    cross.push((ci, cj, w));
                }
            }
        }
        cross.sort_by_key(|e| (e.0, e.1));
        let mut adj: Vec<Vec<(u32, f64)>> = vec![Vec::new(); k];
        for (a, b, w) in cross {
            match adj[a as usize].last_mut() {
                Some(last) if last.0 == b => last.1 += w,
                _ => adj[a as usize].push((b, w)),
            }
        }
        let deg = (0..k)
            .map(|c| adj[c].iter().map(|x| x.1).sum::<f64>() + 2.0 * loops[c])
            .collect();
        Level { adj, loops, deg }
    }
}

/// Louvain community detection: repeated local moving of nodes to the neighbour
/// community with the best modularity gain, then aggregation of communities into
/// super-nodes, until no node moves or `max_levels` is reached.
///
/// Deterministic: nodes are scanned in id order. Errors on a non-positive
/// resolution, zero limits, or negative weights when `weighted`.
///
/// Complexity: `O(m)` per sweep, typically `O(m log n)` overall; `O(n + m)` memory.
pub fn louvain(g: &Projection, cfg: &LouvainConfig) -> Result<LouvainResult> {
    if !(cfg.resolution.is_finite() && cfg.resolution > 0.0) {
        return Err(Error::InvalidParameter(format!(
            "resolution {} must be > 0",
            cfg.resolution
        )));
    }
    if cfg.max_levels == 0 || cfg.max_iterations == 0 {
        return Err(Error::InvalidParameter(
            "max_levels and max_iterations must be at least 1".into(),
        ));
    }
    if !(cfg.tolerance.is_finite() && cfg.tolerance >= 0.0) {
        return Err(Error::InvalidParameter("tolerance must be >= 0".into()));
    }
    let n = g.node_count();
    let edges = edge_list(g, cfg.weighted)?;
    let mut level = Level::from_edges(n, &edges);
    let m2: f64 = level.deg.iter().sum();
    let mut node_comm: Vec<u32> = (0..n as u32).collect();
    let mut levels = 0;
    if m2 > 0.0 {
        for _ in 0..cfg.max_levels {
            let (comm, moved) = level.local_moving(cfg, m2);
            if !moved {
                break;
            }
            for c in &mut node_comm {
                *c = comm[*c as usize];
            }
            levels += 1;
            level = level.aggregate(&comm);
            if level.n() <= 1 {
                break;
            }
        }
    }
    let communities = densify(&node_comm);
    let modularity = modularity(g, &communities, cfg.weighted)?;
    Ok(LouvainResult {
        communities,
        modularity,
        levels,
    })
}

/// Label propagation parameters.
#[derive(Debug, Clone)]
pub struct LabelPropagationConfig {
    /// Maximum sweeps over all nodes (at least 1).
    pub max_iterations: usize,
    /// Seed for the node order and tie-breaking.
    pub seed: u64,
    /// Weight votes by arc weight (ignored on unweighted projections).
    pub weighted: bool,
}

impl Default for LabelPropagationConfig {
    fn default() -> Self {
        Self {
            max_iterations: 10,
            seed: 0,
            weighted: false,
        }
    }
}

/// Label propagation output.
#[derive(Debug, Clone, PartialEq)]
pub struct LabelPropagationResult {
    /// Dense community id per node.
    pub labels: Vec<u32>,
    /// Sweeps executed.
    pub iterations: usize,
    /// `true` if a full sweep changed no label.
    pub converged: bool,
}

/// Asynchronous label propagation (Raghavan et al.): every node repeatedly adopts
/// the label with the largest (weighted) vote among its neighbours, visiting nodes
/// in a seeded random order. A node keeps its label if it is among the winners;
/// other ties are broken with the seeded generator. Direction and self loops are
/// ignored; isolated nodes keep their own label.
///
/// Labels never cross weakly connected components. Errors if
/// `max_iterations == 0`.
///
/// Complexity: `O(iterations * (n + m))` time, `O(n)` extra memory.
pub fn label_propagation(
    g: &Projection,
    cfg: &LabelPropagationConfig,
) -> Result<LabelPropagationResult> {
    if cfg.max_iterations == 0 {
        return Err(Error::InvalidParameter(
            "max_iterations must be at least 1".into(),
        ));
    }
    let n = g.node_count();
    let use_weights = cfg.weighted && g.is_weighted();
    let mut rng = XorShift64::new(cfg.seed);
    let mut labels: Vec<u32> = (0..n as u32).collect();
    let mut order: Vec<u32> = (0..n as u32).collect();
    let mut votes = vec![0.0f64; n];
    let mut touched: Vec<u32> = Vec::new();
    let mut iterations = 0;
    let mut converged = false;
    while iterations < cfg.max_iterations {
        iterations += 1;
        rng.shuffle(&mut order);
        let mut changed = false;
        for &u in &order {
            touched.clear();
            for (v, w) in g.neighbors_any(u) {
                if v == u {
                    continue;
                }
                let l = labels[v as usize];
                if votes[l as usize] == 0.0 {
                    touched.push(l);
                }
                votes[l as usize] += if use_weights { w } else { 1.0 };
            }
            if !touched.is_empty() {
                let top = touched
                    .iter()
                    .map(|&l| votes[l as usize])
                    .fold(f64::MIN, f64::max);
                let winners: Vec<u32> = touched
                    .iter()
                    .copied()
                    .filter(|&l| votes[l as usize] >= top - 1e-12)
                    .collect();
                let current = labels[u as usize];
                if !winners.contains(&current) {
                    labels[u as usize] = winners[rng.gen_range(winners.len() as u64) as usize];
                    changed = true;
                }
            }
            for &l in &touched {
                votes[l as usize] = 0.0;
            }
        }
        if !changed {
            converged = true;
            break;
        }
    }
    Ok(LabelPropagationResult {
        labels: densify(&labels),
        iterations,
        converged,
    })
}
