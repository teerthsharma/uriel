//! Breadth-first and depth-first traversal.

use std::collections::VecDeque;

use crate::error::Result;
use crate::projection::Projection;

/// Result of a single-source traversal. All vectors except `order` are indexed
/// by node id; unreached nodes hold `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct Traversal {
    /// Nodes in visit order (BFS: by level; DFS: preorder). Starts with the source.
    pub order: Vec<u32>,
    /// Tree parent of each reached node; `None` for the source and unreached nodes.
    pub parent: Vec<Option<u32>>,
    /// Hop distance (BFS) or tree depth (DFS) from the source.
    pub depth: Vec<Option<u32>>,
}

/// Breadth-first search along out-arcs from `source`.
///
/// Hop distances are shortest-path distances in the unweighted sense. Neighbours
/// are visited in ascending id order, so the result is deterministic.
///
/// Complexity: `O(n + m)` time, `O(n)` memory.
pub fn bfs(g: &Projection, source: u32) -> Result<Traversal> {
    g.check_node(source)?;
    let n = g.node_count();
    let mut t = Traversal {
        order: Vec::new(),
        parent: vec![None; n],
        depth: vec![None; n],
    };
    let mut queue = VecDeque::new();
    t.depth[source as usize] = Some(0);
    queue.push_back(source);
    while let Some(u) = queue.pop_front() {
        t.order.push(u);
        let du = t.depth[u as usize].unwrap_or(0);
        for &v in g.out_neighbors(u) {
            if t.depth[v as usize].is_none() {
                t.depth[v as usize] = Some(du + 1);
                t.parent[v as usize] = Some(u);
                queue.push_back(v);
            }
        }
    }
    Ok(t)
}

/// Depth-first search along out-arcs from `source`, iterative (no recursion, so
/// arbitrarily deep graphs are safe).
///
/// `order` is the preorder; neighbours are explored in ascending id order.
///
/// Complexity: `O(n + m)` time, `O(n)` memory.
pub fn dfs(g: &Projection, source: u32) -> Result<Traversal> {
    g.check_node(source)?;
    let n = g.node_count();
    let mut t = Traversal {
        order: Vec::new(),
        parent: vec![None; n],
        depth: vec![None; n],
    };
    // Frame = (node, index of the next out-neighbour to try).
    let mut stack: Vec<(u32, usize)> = vec![(source, 0)];
    t.depth[source as usize] = Some(0);
    t.order.push(source);
    while let Some(frame) = stack.last_mut() {
        let (u, next) = *frame;
        let adj = g.out_neighbors(u);
        if next >= adj.len() {
            stack.pop();
            continue;
        }
        frame.1 += 1;
        let v = adj[next];
        if t.depth[v as usize].is_none() {
            t.depth[v as usize] = Some(t.depth[u as usize].unwrap_or(0) + 1);
            t.parent[v as usize] = Some(u);
            t.order.push(v);
            stack.push((v, 0));
        }
    }
    Ok(t)
}
