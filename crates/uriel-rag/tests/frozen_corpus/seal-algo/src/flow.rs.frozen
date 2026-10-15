//! Maximum flow / minimum cut with Dinic's algorithm.

use std::collections::VecDeque;

use crate::error::{Error, Result};
use crate::projection::Projection;

/// Residual capacities at or below this are treated as saturated.
const EPS: f64 = 1e-12;

/// A maximum flow and the matching minimum cut.
#[derive(Debug, Clone, PartialEq)]
pub struct MaxFlow {
    /// Flow value from source to sink (equals the min-cut capacity).
    pub value: f64,
    /// Flow on every projection arc, in CSR order: the out-arcs of node 0 in
    /// order, then those of node 1, and so on. Self loops carry 0.
    pub arc_flows: Vec<f64>,
    /// `true` for nodes on the source side of a minimum cut (reachable from the
    /// source in the final residual graph).
    pub source_side: Vec<bool>,
}

/// Dinic's maximum-flow algorithm. Arc weights are capacities (`1` on
/// unweighted projections); every arc is an independent directed arc, so an
/// undirected edge allows flow in either direction. Parallel arcs add up and
/// self loops are ignored.
///
/// Errors: `NodeOutOfRange`, `InvalidParameter` if `source == sink`, and
/// `NegativeWeight` for a negative capacity.
///
/// Complexity: `O(V^2 E)` worst case (`O(E sqrt V)` on unit-capacity bipartite
/// networks); `O(V + E)` memory. The blocking-flow search is iterative.
pub fn max_flow(g: &Projection, source: u32, sink: u32) -> Result<MaxFlow> {
    g.check_node(source)?;
    g.check_node(sink)?;
    if source == sink {
        return Err(Error::InvalidParameter(
            "source and sink must differ".into(),
        ));
    }
    let n = g.node_count();
    let arcs = g.arc_count();
    // edge 2a = forward arc a, edge 2a+1 = its residual twin.
    let mut to = vec![0u32; 2 * arcs];
    let mut cap = vec![0.0f64; 2 * arcs];
    let mut count = vec![0usize; n + 1];
    let mut a = 0;
    for u in 0..n as u32 {
        for (v, c) in g.out_edges(u) {
            if c < 0.0 {
                return Err(Error::NegativeWeight {
                    from: u,
                    to: v,
                    weight: c,
                });
            }
            to[2 * a] = v;
            to[2 * a + 1] = u;
            if u != v {
                cap[2 * a] = c;
                count[u as usize + 1] += 1;
                count[v as usize + 1] += 1;
            }
            a += 1;
        }
    }
    for i in 0..n {
        count[i + 1] += count[i];
    }
    let start = count.clone();
    let mut fill = count;
    let mut adj = vec![0u32; start[n]];
    let mut a = 0;
    for u in 0..n as u32 {
        for &v in g.out_neighbors(u) {
            if u != v {
                adj[fill[u as usize]] = (2 * a) as u32;
                fill[u as usize] += 1;
                adj[fill[v as usize]] = (2 * a + 1) as u32;
                fill[v as usize] += 1;
            }
            a += 1;
        }
    }

    let (s, t) = (source as usize, sink as usize);
    let mut level = vec![-1i32; n];
    let mut it = vec![0usize; n];
    let mut path: Vec<usize> = Vec::new();
    let mut value = 0.0;
    loop {
        level.fill(-1);
        level[s] = 0;
        let mut queue = VecDeque::from([s]);
        while let Some(u) = queue.pop_front() {
            for &e in &adj[start[u]..start[u + 1]] {
                let e = e as usize;
                let v = to[e] as usize;
                if cap[e] > EPS && level[v] < 0 {
                    level[v] = level[u] + 1;
                    queue.push_back(v);
                }
            }
        }
        if level[t] < 0 {
            break; // `level >= 0` now marks the source side of a minimum cut
        }
        it.copy_from_slice(&start[..n]);
        'blocking: loop {
            path.clear();
            let mut u = s;
            while u != t {
                let mut advanced = false;
                while it[u] < start[u + 1] {
                    let e = adj[it[u]] as usize;
                    let v = to[e] as usize;
                    if cap[e] > EPS && level[v] == level[u] + 1 {
                        path.push(e);
                        u = v;
                        advanced = true;
                        break;
                    }
                    it[u] += 1;
                }
                if !advanced {
                    match path.pop() {
                        None => break 'blocking,
                        Some(e) => {
                            u = to[e ^ 1] as usize;
                            it[u] += 1;
                        }
                    }
                }
            }
            let bottleneck = path.iter().map(|&e| cap[e]).fold(f64::INFINITY, f64::min);
            for &e in &path {
                cap[e] -= bottleneck;
                cap[e ^ 1] += bottleneck;
            }
            value += bottleneck;
        }
    }
    let arc_flows = (0..arcs).map(|a| cap[2 * a + 1]).collect();
    let source_side = level.iter().map(|&l| l >= 0).collect();
    Ok(MaxFlow {
        value,
        arc_flows,
        source_side,
    })
}
