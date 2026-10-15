//! Centrality measures: PageRank (plain and personalized), degree, betweenness
//! (Brandes, optional source sampling) and closeness.

use crate::error::{Error, Result};
use crate::projection::Projection;
use crate::rng::XorShift64;

/// Which arcs a degree is counted over.
///
/// On an undirected projection every variant means "all incident arcs".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Outgoing arcs.
    Out,
    /// Incoming arcs.
    In,
    /// Incoming plus outgoing arcs.
    Both,
}

/// PageRank parameters (defaults follow GDS: damping 0.85, tolerance 1e-7,
/// 100 iterations).
#[derive(Debug, Clone)]
pub struct PageRankConfig {
    /// Probability of following a link instead of teleporting; in `[0, 1)`.
    pub damping: f64,
    /// Stop when the L1 change between iterations drops below this.
    pub tolerance: f64,
    /// Hard iteration cap (at least 1).
    pub max_iterations: usize,
    /// Use arc weights as transition weights (ignored on unweighted projections).
    pub weighted: bool,
}

impl Default for PageRankConfig {
    fn default() -> Self {
        Self {
            damping: 0.85,
            tolerance: 1e-7,
            max_iterations: 100,
            weighted: false,
        }
    }
}

/// PageRank output; `scores` sum to 1.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRankResult {
    /// Score per node id.
    pub scores: Vec<f64>,
    /// Iterations executed.
    pub iterations: usize,
    /// `true` if the tolerance was reached before `max_iterations`.
    pub converged: bool,
}

/// PageRank by power iteration.
///
/// Dangling nodes (no outgoing weight) redistribute their mass uniformly, so the
/// scores always sum to 1. Parallel arcs add up; self loops are ordinary links.
///
/// Complexity: `O(iterations * (n + m))` time, `O(n)` memory.
pub fn pagerank(g: &Projection, cfg: &PageRankConfig) -> Result<PageRankResult> {
    pagerank_impl(g, None, cfg)
}

/// Personalized PageRank: teleportation (and dangling mass) returns to the
/// distinct nodes of `sources` uniformly instead of to all nodes.
///
/// Errors if `sources` is empty or contains an out-of-range id. Complexity as
/// [`pagerank`].
pub fn personalized_pagerank(
    g: &Projection,
    sources: &[u32],
    cfg: &PageRankConfig,
) -> Result<PageRankResult> {
    if sources.is_empty() {
        return Err(Error::InvalidParameter(
            "personalized PageRank needs at least one source".into(),
        ));
    }
    for &s in sources {
        g.check_node(s)?;
    }
    pagerank_impl(g, Some(sources), cfg)
}

fn pagerank_impl(
    g: &Projection,
    sources: Option<&[u32]>,
    cfg: &PageRankConfig,
) -> Result<PageRankResult> {
    if !(cfg.damping.is_finite() && (0.0..1.0).contains(&cfg.damping)) {
        return Err(Error::InvalidParameter(format!(
            "damping {} not in [0, 1)",
            cfg.damping
        )));
    }
    if !(cfg.tolerance.is_finite() && cfg.tolerance >= 0.0) {
        return Err(Error::InvalidParameter(format!(
            "tolerance {} must be >= 0",
            cfg.tolerance
        )));
    }
    if cfg.max_iterations == 0 {
        return Err(Error::InvalidParameter(
            "max_iterations must be at least 1".into(),
        ));
    }
    let n = g.node_count();
    if n == 0 {
        return Ok(PageRankResult {
            scores: Vec::new(),
            iterations: 0,
            converged: true,
        });
    }
    let use_weights = cfg.weighted && g.is_weighted();
    let mut out_sum = vec![0.0f64; n];
    for u in 0..n as u32 {
        for (_, w) in g.out_edges(u) {
            if use_weights && w < 0.0 {
                return Err(Error::InvalidParameter(
                    "PageRank needs non-negative weights".into(),
                ));
            }
            out_sum[u as usize] += if use_weights { w } else { 1.0 };
        }
    }
    let mut teleport = vec![0.0f64; n];
    match sources {
        None => teleport.fill(1.0 / n as f64),
        Some(s) => {
            let mut distinct = s.to_vec();
            distinct.sort_unstable();
            distinct.dedup();
            let share = 1.0 / distinct.len() as f64;
            for u in distinct {
                teleport[u as usize] = share;
            }
        }
    }
    let d = cfg.damping;
    let mut p = teleport.clone();
    let mut next = vec![0.0f64; n];
    let mut iterations = 0;
    let mut converged = false;
    while iterations < cfg.max_iterations {
        iterations += 1;
        let dangling: f64 = (0..n).filter(|&u| out_sum[u] <= 0.0).map(|u| p[u]).sum();
        let base = (1.0 - d) + d * dangling;
        for (x, t) in next.iter_mut().zip(&teleport) {
            *x = base * t;
        }
        for u in 0..n as u32 {
            let total = out_sum[u as usize];
            if total <= 0.0 {
                continue;
            }
            let share = d * p[u as usize] / total;
            for (v, w) in g.out_edges(u) {
                next[v as usize] += share * if use_weights { w } else { 1.0 };
            }
        }
        let delta: f64 = p.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
        std::mem::swap(&mut p, &mut next);
        if delta < cfg.tolerance {
            converged = true;
            break;
        }
    }
    Ok(PageRankResult {
        scores: p,
        iterations,
        converged,
    })
}

/// Degree centrality: the number of arcs (or their weight sum when `weighted`
/// and the projection is weighted) per node. Parallel arcs and self loops count.
///
/// Complexity: `O(n + m)`.
pub fn degree_centrality(g: &Projection, dir: Direction, weighted: bool) -> Vec<f64> {
    let use_weights = weighted && g.is_weighted();
    let sum = |it: &mut dyn Iterator<Item = (u32, f64)>| -> f64 {
        it.map(|(_, w)| if use_weights { w } else { 1.0 }).sum()
    };
    (0..g.node_count() as u32)
        .map(|u| match (dir, g.is_directed()) {
            (Direction::Out, _) | (_, false) => sum(&mut g.out_edges(u)),
            (Direction::In, true) => sum(&mut g.in_edges(u)),
            (Direction::Both, true) => sum(&mut g.out_edges(u)) + sum(&mut g.in_edges(u)),
        })
        .collect()
}

/// Betweenness parameters.
#[derive(Debug, Clone, Default)]
pub struct BetweennessConfig {
    /// Divide by the number of node pairs (as networkx does): `(n-1)(n-2)/2`
    /// undirected, `(n-1)(n-2)` directed. Ignored for `n < 3`.
    pub normalize: bool,
    /// Run Brandes from this many random sources only and scale by `n / k`
    /// (an unbiased estimate). `None`, or `k >= n`, is exact.
    pub sample_size: Option<usize>,
    /// Seed for source sampling.
    pub seed: u64,
}

/// Deduplicated, loop-free adjacency used by the BFS-based centralities.
struct SimpleAdj {
    offsets: Vec<usize>,
    targets: Vec<u32>,
}

impl SimpleAdj {
    fn new(g: &Projection) -> Self {
        let mut offsets = Vec::with_capacity(g.node_count() + 1);
        let mut targets = Vec::with_capacity(g.arc_count());
        offsets.push(0);
        for u in 0..g.node_count() as u32 {
            let mut prev = None;
            for &v in g.out_neighbors(u) {
                if v != u && prev != Some(v) {
                    targets.push(v);
                }
                prev = Some(v);
            }
            offsets.push(targets.len());
        }
        Self { offsets, targets }
    }

    fn of(&self, u: u32) -> &[u32] {
        &self.targets[self.offsets[u as usize]..self.offsets[u as usize + 1]]
    }
}

struct Scratch {
    dist: Vec<u32>,
    sigma: Vec<f64>,
    delta: Vec<f64>,
    order: Vec<u32>,
}

impl Scratch {
    fn new(n: usize) -> Self {
        Self {
            dist: vec![u32::MAX; n],
            sigma: vec![0.0; n],
            delta: vec![0.0; n],
            order: Vec::with_capacity(n),
        }
    }
}

/// One Brandes source: BFS counting shortest paths, then dependency accumulation
/// in reverse BFS order using successors (so only out-adjacency is needed).
fn brandes_from(adj: &SimpleAdj, s: u32, sc: &mut Scratch, bc: &mut [f64]) {
    sc.order.clear();
    sc.dist[s as usize] = 0;
    sc.sigma[s as usize] = 1.0;
    sc.order.push(s);
    let mut head = 0;
    while head < sc.order.len() {
        let u = sc.order[head];
        head += 1;
        let du = sc.dist[u as usize];
        for &v in adj.of(u) {
            if sc.dist[v as usize] == u32::MAX {
                sc.dist[v as usize] = du + 1;
                sc.order.push(v);
            }
            if sc.dist[v as usize] == du + 1 {
                sc.sigma[v as usize] += sc.sigma[u as usize];
            }
        }
    }
    for i in (0..sc.order.len()).rev() {
        let v = sc.order[i];
        let dv = sc.dist[v as usize];
        let mut acc = 0.0;
        for &w in adj.of(v) {
            if sc.dist[w as usize] == dv + 1 {
                acc += sc.sigma[v as usize] / sc.sigma[w as usize] * (1.0 + sc.delta[w as usize]);
            }
        }
        sc.delta[v as usize] = acc;
        if v != s {
            bc[v as usize] += acc;
        }
    }
    for &v in &sc.order {
        sc.dist[v as usize] = u32::MAX;
        sc.sigma[v as usize] = 0.0;
        sc.delta[v as usize] = 0.0;
    }
}

#[cfg(not(feature = "parallel"))]
fn accumulate(adj: &SimpleAdj, n: usize, sources: &[u32]) -> Vec<f64> {
    let mut bc = vec![0.0; n];
    let mut sc = Scratch::new(n);
    for &s in sources {
        brandes_from(adj, s, &mut sc, &mut bc);
    }
    bc
}

#[cfg(feature = "parallel")]
fn accumulate(adj: &SimpleAdj, n: usize, sources: &[u32]) -> Vec<f64> {
    use rayon::prelude::*;
    sources
        .par_iter()
        .fold(
            || (Scratch::new(n), vec![0.0; n]),
            |(mut sc, mut bc), &s| {
                brandes_from(adj, s, &mut sc, &mut bc);
                (sc, bc)
            },
        )
        .map(|(_, bc)| bc)
        .reduce(
            || vec![0.0; n],
            |mut a, b| {
                for (x, y) in a.iter_mut().zip(b) {
                    *x += y;
                }
                a
            },
        )
}

/// Betweenness centrality with Brandes' algorithm on the unweighted graph.
///
/// The projection is treated as a simple graph: parallel arcs collapse and self
/// loops are ignored. On undirected projections each pair is counted once.
/// With `sample_size = Some(k)` only `k` seeded random sources are used and the
/// result is scaled by `n / k`. With the `parallel` feature, sources run on rayon.
///
/// Errors if `sample_size == Some(0)`.
///
/// Complexity: `O(n * m)` time exact, `O(k * m)` sampled; `O(n + m)` memory.
pub fn betweenness_centrality(g: &Projection, cfg: &BetweennessConfig) -> Result<Vec<f64>> {
    if cfg.sample_size == Some(0) {
        return Err(Error::InvalidParameter(
            "sample_size must be at least 1".into(),
        ));
    }
    let n = g.node_count();
    let adj = SimpleAdj::new(g);
    let mut sources: Vec<u32> = (0..n as u32).collect();
    let mut scale = 1.0;
    if let Some(k) = cfg.sample_size {
        if k < n {
            XorShift64::new(cfg.seed).shuffle(&mut sources);
            sources.truncate(k);
            scale = n as f64 / k as f64;
        }
    }
    let mut bc = accumulate(&adj, n, &sources);
    if !g.is_directed() {
        scale *= 0.5;
    }
    if cfg.normalize && n > 2 {
        let pairs = ((n - 1) * (n - 2)) as f64;
        scale /= if g.is_directed() { pairs } else { pairs / 2.0 };
    }
    for x in &mut bc {
        *x *= scale;
    }
    Ok(bc)
}

/// Closeness centrality from out-arc BFS distances.
///
/// With `r` nodes reachable from `u` (including `u`) and distance sum `S`, the
/// score is `(r - 1) / S`; with `wf_improved` it is additionally multiplied by
/// `(r - 1) / (n - 1)` (Wasserman-Faust), which keeps disconnected graphs
/// comparable. Nodes that reach nothing score 0. Direction follows out-arcs.
///
/// Complexity: `O(n * (n + m))` time, `O(n)` memory.
pub fn closeness_centrality(g: &Projection, wf_improved: bool) -> Vec<f64> {
    let n = g.node_count();
    let mut dist = vec![u32::MAX; n];
    let mut queue: Vec<u32> = Vec::with_capacity(n);
    let mut out = vec![0.0; n];
    for s in 0..n as u32 {
        queue.clear();
        dist[s as usize] = 0;
        queue.push(s);
        let mut head = 0;
        let mut total: u64 = 0;
        while head < queue.len() {
            let u = queue[head];
            head += 1;
            for &v in g.out_neighbors(u) {
                if dist[v as usize] == u32::MAX {
                    dist[v as usize] = dist[u as usize] + 1;
                    total += dist[v as usize] as u64;
                    queue.push(v);
                }
            }
        }
        let reached = queue.len() - 1;
        if reached > 0 && total > 0 {
            let mut c = reached as f64 / total as f64;
            if wf_improved {
                c *= reached as f64 / (n - 1) as f64;
            }
            out[s as usize] = c;
        }
        for &v in &queue {
            dist[v as usize] = u32::MAX;
        }
    }
    out
}
