//! Connected components (weak and strong) and topological ordering.
//!
//! Component labels are the smallest node id in the component, so labels are
//! stable and comparable across algorithms.

use std::collections::VecDeque;

use crate::error::{Error, Result};
use crate::projection::Projection;

pub(crate) fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let gp = parent[parent[x as usize] as usize];
        parent[x as usize] = gp; // path halving
        x = gp;
    }
    x
}

/// Weakly connected components via union-find (path halving + union by size).
/// Direction is ignored. Returns, for every node, the smallest node id in its
/// component.
///
/// Complexity: `O((n + m) α(n))` time, `O(n)` memory.
pub fn weakly_connected_components(g: &Projection) -> Vec<u32> {
    let n = g.node_count();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut size = vec![1u32; n];
    for u in 0..n as u32 {
        for &v in g.out_neighbors(u) {
            let (mut a, mut b) = (find(&mut parent, u), find(&mut parent, v));
            if a == b {
                continue;
            }
            if size[a as usize] < size[b as usize] {
                std::mem::swap(&mut a, &mut b);
            }
            parent[b as usize] = a;
            size[a as usize] += size[b as usize];
        }
    }
    let mut label_of_root = vec![u32::MAX; n];
    let mut labels = vec![0u32; n];
    for u in 0..n as u32 {
        let r = find(&mut parent, u) as usize;
        if label_of_root[r] == u32::MAX {
            label_of_root[r] = u; // ascending scan: first member is the minimum
        }
        labels[u as usize] = label_of_root[r];
    }
    labels
}

/// Strongly connected components with an iterative Tarjan (explicit stacks, so a
/// 200k-deep path cannot overflow the call stack). Returns the smallest node id
/// of each node's component. On an undirected projection this equals
/// [`weakly_connected_components`].
///
/// Complexity: `O(n + m)` time, `O(n)` memory.
pub fn strongly_connected_components(g: &Projection) -> Vec<u32> {
    if !g.is_directed() {
        return weakly_connected_components(g);
    }
    const UNSEEN: u32 = u32::MAX;
    let n = g.node_count();
    let mut index = vec![UNSEEN; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut labels = vec![0u32; n];
    let mut scc_stack: Vec<u32> = Vec::new();
    let mut call: Vec<(u32, usize)> = Vec::new();
    let mut counter = 0u32;

    for root in 0..n as u32 {
        if index[root as usize] != UNSEEN {
            continue;
        }
        index[root as usize] = counter;
        low[root as usize] = counter;
        counter += 1;
        scc_stack.push(root);
        on_stack[root as usize] = true;
        call.push((root, 0));
        while let Some(&(u, next)) = call.last() {
            let adj = g.out_neighbors(u);
            if next < adj.len() {
                if let Some(top) = call.last_mut() {
                    top.1 += 1;
                }
                let v = adj[next];
                if index[v as usize] == UNSEEN {
                    index[v as usize] = counter;
                    low[v as usize] = counter;
                    counter += 1;
                    scc_stack.push(v);
                    on_stack[v as usize] = true;
                    call.push((v, 0));
                } else if on_stack[v as usize] {
                    low[u as usize] = low[u as usize].min(index[v as usize]);
                }
            } else {
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent as usize] = low[parent as usize].min(low[u as usize]);
                }
                if low[u as usize] == index[u as usize] {
                    let start = scc_stack.iter().rposition(|&x| x == u).unwrap_or(0);
                    let min = scc_stack[start..].iter().copied().min().unwrap_or(u);
                    for &w in &scc_stack[start..] {
                        on_stack[w as usize] = false;
                        labels[w as usize] = min;
                    }
                    scc_stack.truncate(start);
                }
            }
        }
    }
    labels
}

/// Number of distinct labels. `O(n log n)`.
pub fn component_count(labels: &[u32]) -> usize {
    let mut v = labels.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len()
}

/// `(label, size)` for every component, sorted by label. `O(n log n)`.
pub fn component_sizes(labels: &[u32]) -> Vec<(u32, usize)> {
    let mut v = labels.to_vec();
    v.sort_unstable();
    let mut out: Vec<(u32, usize)> = Vec::new();
    for l in v {
        match out.last_mut() {
            Some(last) if last.0 == l => last.1 += 1,
            _ => out.push((l, 1)),
        }
    }
    out
}

/// Topological order of a directed graph (Kahn's algorithm, FIFO, so ties
/// resolve towards lower ids first among simultaneously ready nodes).
///
/// Errors: [`Error::RequiresDirected`] for undirected projections,
/// [`Error::CycleDetected`] if a cycle (including a self loop) exists.
///
/// Complexity: `O(n + m)` time, `O(n)` memory.
pub fn topological_sort(g: &Projection) -> Result<Vec<u32>> {
    if !g.is_directed() {
        return Err(Error::RequiresDirected);
    }
    let n = g.node_count();
    let mut indeg: Vec<usize> = (0..n as u32).map(|u| g.in_degree(u)).collect();
    let mut queue: VecDeque<u32> = (0..n as u32).filter(|&u| indeg[u as usize] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(u) = queue.pop_front() {
        order.push(u);
        for &v in g.out_neighbors(u) {
            indeg[v as usize] -= 1;
            if indeg[v as usize] == 0 {
                queue.push_back(v);
            }
        }
    }
    if order.len() == n {
        Ok(order)
    } else {
        Err(Error::CycleDetected)
    }
}
