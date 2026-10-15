//! Minimum spanning trees (Kruskal forest, Prim from a start node).
//!
//! Direction is ignored, self loops are skipped and, between parallel edges,
//! the lightest wins. Negative weights are allowed. Unweighted projections have
//! unit weights.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::components::find;
use crate::error::Result;
use crate::projection::Projection;

/// A minimum spanning tree or forest.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanningForest {
    /// Chosen edges as `(low id, high id, weight)`.
    pub edges: Vec<(u32, u32, f64)>,
    /// Sum of the chosen edge weights.
    pub total_weight: f64,
    /// Number of trees: connected components for [`kruskal_mst`], always 1 for
    /// [`prim_mst`].
    pub components: usize,
}

/// Kruskal's algorithm: sort edges by weight and add those that join two
/// different components (union-find). Yields a minimum spanning *forest* when the
/// graph is disconnected.
///
/// Complexity: `O(m log m)` time, `O(n + m)` memory.
pub fn kruskal_mst(g: &Projection) -> SpanningForest {
    let n = g.node_count();
    let mut edges: Vec<(u32, u32, f64)> = g
        .edges()
        .into_iter()
        .filter(|e| e.0 != e.1)
        .map(|(u, v, w)| (u.min(v), u.max(v), w))
        .collect();
    edges.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut size = vec![1u32; n];
    let mut chosen = Vec::new();
    let mut total = 0.0;
    for (u, v, w) in edges {
        let (mut a, mut b) = (find(&mut parent, u), find(&mut parent, v));
        if a == b {
            continue;
        }
        if size[a as usize] < size[b as usize] {
            std::mem::swap(&mut a, &mut b);
        }
        parent[b as usize] = a;
        size[a as usize] += size[b as usize];
        total += w;
        chosen.push((u, v, w));
    }
    SpanningForest {
        components: n - chosen.len(),
        edges: chosen,
        total_weight: total,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cand {
    w: f64,
    from: u32,
    to: u32,
}

impl Eq for Cand {}

impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        o.w.total_cmp(&self.w)
            .then_with(|| o.from.cmp(&self.from))
            .then_with(|| o.to.cmp(&self.to))
    }
}

impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Prim's algorithm (lazy, binary heap) grown from `start`: a minimum spanning
/// tree of `start`'s connected component only.
///
/// Errors with `NodeOutOfRange` for a bad `start`.
///
/// Complexity: `O(m log m)` time, `O(n + m)` memory.
pub fn prim_mst(g: &Projection, start: u32) -> Result<SpanningForest> {
    g.check_node(start)?;
    let mut visited = vec![false; g.node_count()];
    let mut heap = BinaryHeap::new();
    let mut edges = Vec::new();
    let mut total = 0.0;
    let push_from = |u: u32, heap: &mut BinaryHeap<Cand>, visited: &[bool]| {
        for (v, w) in g.neighbors_any(u) {
            if v != u && !visited[v as usize] {
                heap.push(Cand { w, from: u, to: v });
            }
        }
    };
    visited[start as usize] = true;
    push_from(start, &mut heap, &visited);
    while let Some(Cand { w, from, to }) = heap.pop() {
        if visited[to as usize] {
            continue;
        }
        visited[to as usize] = true;
        total += w;
        edges.push((from.min(to), from.max(to), w));
        push_from(to, &mut heap, &visited);
    }
    Ok(SpanningForest {
        edges,
        total_weight: total,
        components: 1,
    })
}
