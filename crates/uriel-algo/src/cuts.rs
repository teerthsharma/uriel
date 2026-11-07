//! Cut vertices (articulation points) and bridges.
//!
//! The projection is read as a simple undirected graph: direction is ignored,
//! parallel arcs collapse to one edge and self loops are dropped.

use crate::projection::Projection;

const NONE: u32 = u32::MAX;

fn simple_adjacency(g: &Projection) -> (Vec<usize>, Vec<u32>) {
    let n = g.node_count();
    let mut offsets = Vec::with_capacity(n + 1);
    let mut targets = Vec::new();
    let mut buf: Vec<u32> = Vec::new();
    offsets.push(0);
    for u in 0..n as u32 {
        buf.clear();
        buf.extend(g.neighbors_any(u).map(|(v, _)| v).filter(|&v| v != u));
        buf.sort_unstable();
        buf.dedup();
        targets.extend_from_slice(&buf);
        offsets.push(targets.len());
    }
    (offsets, targets)
}

fn lowlink(g: &Projection) -> (Vec<u32>, Vec<(u32, u32)>) {
    let n = g.node_count();
    let (offsets, targets) = simple_adjacency(g);
    let mut disc = vec![NONE; n];
    let mut low = vec![0u32; n];
    let mut parent = vec![NONE; n];
    let mut is_cut = vec![false; n];
    let mut bridges: Vec<(u32, u32)> = Vec::new();
    let mut next = vec![0usize; n];
    let mut clock = 0u32;
    let mut stack: Vec<u32> = Vec::new();
    for root in 0..n as u32 {
        if disc[root as usize] != NONE {
            continue;
        }
        let mut root_children = 0usize;
        disc[root as usize] = clock;
        low[root as usize] = clock;
        clock += 1;
        stack.push(root);
        while let Some(&u) = stack.last() {
            let ui = u as usize;
            let at = offsets[ui] + next[ui];
            if at < offsets[ui + 1] {
                next[ui] += 1;
                let v = targets[at];
                if v == parent[ui] {
                    continue;
                }
                if disc[v as usize] == NONE {
                    parent[v as usize] = u;
                    disc[v as usize] = clock;
                    low[v as usize] = clock;
                    clock += 1;
                    stack.push(v);
                } else {
                    low[ui] = low[ui].min(disc[v as usize]);
                }
            } else {
                stack.pop();
                let p = parent[ui];
                if p == NONE {
                    continue;
                }
                let pi = p as usize;
                low[pi] = low[pi].min(low[ui]);
                if p == root {
                    root_children += 1;
                } else if low[ui] >= disc[pi] {
                    is_cut[pi] = true;
                }
                if low[ui] > disc[pi] {
                    bridges.push((p.min(u), p.max(u)));
                }
            }
        }
        if root_children > 1 {
            is_cut[root as usize] = true;
        }
    }
    bridges.sort_unstable();
    let cuts = (0..n as u32).filter(|&u| is_cut[u as usize]).collect();
    (cuts, bridges)
}

/// Articulation points: nodes whose removal increases the number of connected
/// components. Sorted ascending.
///
/// Complexity: `O(n + m log m)` time, `O(n + m)` memory; iterative, so deep
/// graphs cannot overflow the stack.
pub fn articulation_points(g: &Projection) -> Vec<u32> {
    lowlink(g).0
}

/// Bridges: edges whose removal increases the number of connected components,
/// as `(low, high)` node pairs sorted ascending.
///
/// Complexity as [`articulation_points`].
pub fn bridges(g: &Projection) -> Vec<(u32, u32)> {
    lowlink(g).1
}
