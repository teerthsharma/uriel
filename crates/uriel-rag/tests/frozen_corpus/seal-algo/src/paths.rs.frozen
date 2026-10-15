//! Shortest-path algorithms: Dijkstra, A*, Bellman-Ford, Yen's k shortest
//! simple paths and all-pairs distances.
//!
//! Unweighted projections behave as if every arc had weight `1.0`. Unreachable
//! nodes have distance `f64::INFINITY`. Parallel arcs are handled by always
//! using the cheapest one; self loops never improve a path.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

use crate::error::{Error, Result};
use crate::projection::Projection;

/// Largest node count [`all_pairs_shortest_paths`] accepts (`n^2` `f64`s of output).
pub const MAX_ALL_PAIRS_NODES: usize = 4096;

/// Single-source shortest-path tree.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortestPaths {
    /// The source node.
    pub source: u32,
    /// Distance from the source, `f64::INFINITY` when unreachable.
    pub dist: Vec<f64>,
    /// Predecessor on a shortest path; `None` for the source and unreachable nodes.
    pub parent: Vec<Option<u32>>,
}

impl ShortestPaths {
    /// The node sequence `source ..= t`, or `None` if `t` is unreachable.
    /// `O(path length)`.
    pub fn path_to(&self, t: u32) -> Option<Vec<u32>> {
        if !self.dist.get(t as usize)?.is_finite() {
            return None;
        }
        let mut path = vec![t];
        let mut cur = t;
        while let Some(p) = self.parent[cur as usize] {
            path.push(p);
            cur = p;
        }
        path.reverse();
        Some(path)
    }

    /// Distance to `t`, or `None` if unreachable or out of range.
    pub fn distance_to(&self, t: u32) -> Option<f64> {
        self.dist.get(t as usize).copied().filter(|d| d.is_finite())
    }
}

/// A concrete path with its total cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Nodes from start to end, inclusive.
    pub nodes: Vec<u32>,
    /// Sum of the (cheapest parallel) arc weights along the path.
    pub cost: f64,
}

/// Min-heap entry ordered by `key` ascending (ties by node id for determinism).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HeapItem {
    pub key: f64,
    pub node: u32,
}

impl Eq for HeapItem {}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .key
            .total_cmp(&self.key)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn check_non_negative(g: &Projection) -> Result<()> {
    for u in 0..g.node_count() as u32 {
        for (v, w) in g.out_edges(u) {
            if w < 0.0 {
                return Err(Error::NegativeWeight {
                    from: u,
                    to: v,
                    weight: w,
                });
            }
        }
    }
    Ok(())
}

/// Dijkstra with optional early exit and optional node/arc exclusions (for Yen).
/// Weights must already be validated as non-negative.
fn dijkstra_core(
    g: &Projection,
    source: u32,
    target: Option<u32>,
    blocked_nodes: &[bool],
    blocked_arcs: &HashSet<(u32, u32)>,
) -> ShortestPaths {
    let n = g.node_count();
    let mut dist = vec![f64::INFINITY; n];
    let mut parent = vec![None; n];
    let mut heap = BinaryHeap::new();
    dist[source as usize] = 0.0;
    heap.push(HeapItem {
        key: 0.0,
        node: source,
    });
    while let Some(HeapItem { key, node: u }) = heap.pop() {
        if key > dist[u as usize] {
            continue;
        }
        if Some(u) == target {
            break;
        }
        for (v, w) in g.out_edges(u) {
            if (!blocked_nodes.is_empty() && blocked_nodes[v as usize])
                || (!blocked_arcs.is_empty() && blocked_arcs.contains(&(u, v)))
            {
                continue;
            }
            let nd = key + w;
            if nd < dist[v as usize] {
                dist[v as usize] = nd;
                parent[v as usize] = Some(u);
                heap.push(HeapItem { key: nd, node: v });
            }
        }
    }
    ShortestPaths {
        source,
        dist,
        parent,
    }
}

/// Single-source shortest paths with Dijkstra's algorithm and a binary heap.
///
/// Errors with [`Error::NegativeWeight`] if any arc is negative (use
/// [`bellman_ford`] instead) and [`Error::NodeOutOfRange`] for a bad source.
///
/// Complexity: `O((n + m) log n)` time, `O(n)` memory.
pub fn dijkstra(g: &Projection, source: u32) -> Result<ShortestPaths> {
    g.check_node(source)?;
    check_non_negative(g)?;
    Ok(dijkstra_core(g, source, None, &[], &HashSet::new()))
}

/// Shortest path from `s` to `t`, stopping as soon as `t` is settled.
/// `Ok(None)` if `t` is unreachable. Same complexity and errors as [`dijkstra`].
pub fn dijkstra_path(g: &Projection, s: u32, t: u32) -> Result<Option<Path>> {
    g.check_node(s)?;
    g.check_node(t)?;
    check_non_negative(g)?;
    let sp = dijkstra_core(g, s, Some(t), &[], &HashSet::new());
    Ok(sp.path_to(t).map(|nodes| Path {
        nodes,
        cost: sp.dist[t as usize],
    }))
}

/// A* search from `s` to `t` with a caller-supplied heuristic `h(node)`.
///
/// `h` must never overestimate the remaining cost (admissible); then the result
/// is optimal. Re-expansion is allowed, so a merely admissible (inconsistent)
/// heuristic is still handled correctly. With `h = |_| 0.0` this is Dijkstra.
///
/// Complexity: `O((n + m) log n)` worst case; typically far fewer expansions.
pub fn astar(g: &Projection, s: u32, t: u32, h: impl Fn(u32) -> f64) -> Result<Option<Path>> {
    g.check_node(s)?;
    g.check_node(t)?;
    check_non_negative(g)?;
    let n = g.node_count();
    let mut dist = vec![f64::INFINITY; n];
    let mut parent: Vec<Option<u32>> = vec![None; n];
    let mut heap = BinaryHeap::new();
    dist[s as usize] = 0.0;
    heap.push(HeapItem { key: h(s), node: s });
    while let Some(HeapItem { key, node: u }) = heap.pop() {
        if u == t {
            break;
        }
        let du = dist[u as usize];
        if key > du + h(u) {
            continue; // stale entry
        }
        for (v, w) in g.out_edges(u) {
            let nd = du + w;
            if nd < dist[v as usize] {
                dist[v as usize] = nd;
                parent[v as usize] = Some(u);
                heap.push(HeapItem {
                    key: nd + h(v),
                    node: v,
                });
            }
        }
    }
    let sp = ShortestPaths {
        source: s,
        dist,
        parent,
    };
    Ok(sp.path_to(t).map(|nodes| Path {
        nodes,
        cost: sp.dist[t as usize],
    }))
}

/// Single-source shortest paths allowing negative weights.
///
/// Returns [`Error::NegativeCycle`] if a negative-weight cycle is reachable from
/// `source` (an undirected edge with negative weight is such a cycle). Cycles
/// unreachable from the source are ignored.
///
/// Complexity: `O(n * m)` time (early exit when a round changes nothing),
/// `O(n)` memory.
pub fn bellman_ford(g: &Projection, source: u32) -> Result<ShortestPaths> {
    g.check_node(source)?;
    let n = g.node_count();
    let mut dist = vec![f64::INFINITY; n];
    let mut parent = vec![None; n];
    dist[source as usize] = 0.0;
    let mut converged = false;
    for _ in 0..n.max(1) {
        let mut changed = false;
        for u in 0..n as u32 {
            let du = dist[u as usize];
            if !du.is_finite() {
                continue;
            }
            for (v, w) in g.out_edges(u) {
                if du + w < dist[v as usize] {
                    dist[v as usize] = du + w;
                    parent[v as usize] = Some(u);
                    changed = true;
                }
            }
        }
        if !changed {
            converged = true;
            break;
        }
    }
    if !converged {
        // n rounds still changed something: a shortest path would need > n-1 arcs.
        return Err(Error::NegativeCycle);
    }
    Ok(ShortestPaths {
        source,
        dist,
        parent,
    })
}

fn cheapest_arc(g: &Projection, u: u32, v: u32) -> f64 {
    g.out_edges(u)
        .filter(|&(t, _)| t == v)
        .map(|(_, w)| w)
        .fold(f64::INFINITY, f64::min)
}

fn path_cost(g: &Projection, nodes: &[u32]) -> f64 {
    nodes.windows(2).map(|w| cheapest_arc(g, w[0], w[1])).sum()
}

/// Yen's algorithm: the `k` shortest *simple* (loop-free) paths from `s` to `t`,
/// cheapest first. Fewer than `k` are returned if fewer exist. Paths are
/// distinct as node sequences; between parallel arcs the cheapest is used.
///
/// Errors as [`dijkstra`] (non-negative weights required).
///
/// Complexity: `O(k * n * (n + m) log n)` time.
pub fn yen_k_shortest_paths(g: &Projection, s: u32, t: u32, k: usize) -> Result<Vec<Path>> {
    g.check_node(s)?;
    g.check_node(t)?;
    check_non_negative(g)?;
    if k == 0 {
        return Ok(Vec::new());
    }
    let n = g.node_count();
    let first = dijkstra_core(g, s, Some(t), &[], &HashSet::new());
    let Some(nodes) = first.path_to(t) else {
        return Ok(Vec::new());
    };
    let mut accepted = vec![Path {
        cost: first.dist[t as usize],
        nodes,
    }];
    let mut candidates: Vec<Path> = Vec::new();
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    seen.insert(accepted[0].nodes.clone());

    while accepted.len() < k {
        let prev = accepted[accepted.len() - 1].nodes.clone();
        for i in 0..prev.len().saturating_sub(1) {
            let spur = prev[i];
            let root = &prev[..=i];
            let mut blocked_arcs = HashSet::new();
            for p in &accepted {
                if p.nodes.len() > i + 1 && p.nodes[..=i] == *root {
                    blocked_arcs.insert((p.nodes[i], p.nodes[i + 1]));
                }
            }
            let mut blocked_nodes = vec![false; n];
            for &r in &root[..i] {
                blocked_nodes[r as usize] = true;
            }
            let sp = dijkstra_core(g, spur, Some(t), &blocked_nodes, &blocked_arcs);
            if let Some(spur_path) = sp.path_to(t) {
                let mut total = root[..i].to_vec();
                total.extend(spur_path);
                if seen.insert(total.clone()) {
                    let cost = path_cost(g, &total);
                    candidates.push(Path { nodes: total, cost });
                }
            }
        }
        let best = candidates
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.cost
                    .total_cmp(&b.cost)
                    .then_with(|| a.nodes.cmp(&b.nodes))
            })
            .map(|(i, _)| i);
        match best {
            Some(i) => accepted.push(candidates.swap_remove(i)),
            None => break,
        }
    }
    Ok(accepted)
}

/// All-pairs shortest distances by running Dijkstra from every node.
///
/// `result[s][t]` is the distance, `f64::INFINITY` if unreachable. Only for small
/// graphs: errors with [`Error::TooLarge`] above [`MAX_ALL_PAIRS_NODES`] nodes.
///
/// Complexity: `O(n (n + m) log n)` time, `O(n^2)` memory.
pub fn all_pairs_shortest_paths(g: &Projection) -> Result<Vec<Vec<f64>>> {
    let n = g.node_count();
    if n > MAX_ALL_PAIRS_NODES {
        return Err(Error::TooLarge {
            node_count: n,
            limit: MAX_ALL_PAIRS_NODES,
        });
    }
    check_non_negative(g)?;
    let none = HashSet::new();
    Ok((0..n as u32)
        .map(|s| dijkstra_core(g, s, None, &[], &none).dist)
        .collect())
}
