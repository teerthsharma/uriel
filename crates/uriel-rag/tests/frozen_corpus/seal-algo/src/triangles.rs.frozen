//! Triangle counting, local clustering coefficient and k-core decomposition.
//!
//! All three treat the projection as a simple undirected graph: direction is
//! ignored, parallel arcs collapse to one edge and self loops are dropped.

use crate::projection::Projection;

/// Sorted, deduplicated, loop-free undirected adjacency in CSR form.
struct Simple {
    offsets: Vec<usize>,
    targets: Vec<u32>,
}

impl Simple {
    fn new(g: &Projection) -> Self {
        let n = g.node_count();
        let mut offsets = Vec::with_capacity(n + 1);
        let mut targets = Vec::new();
        offsets.push(0);
        let mut buf: Vec<u32> = Vec::new();
        for u in 0..n as u32 {
            buf.clear();
            buf.extend(g.neighbors_any(u).map(|(v, _)| v).filter(|&v| v != u));
            buf.sort_unstable();
            buf.dedup();
            targets.extend_from_slice(&buf);
            offsets.push(targets.len());
        }
        Self { offsets, targets }
    }

    fn of(&self, u: u32) -> &[u32] {
        &self.targets[self.offsets[u as usize]..self.offsets[u as usize + 1]]
    }

    fn degree(&self, u: u32) -> usize {
        self.offsets[u as usize + 1] - self.offsets[u as usize]
    }
}

/// Triangle counts.
#[derive(Debug, Clone, PartialEq)]
pub struct TriangleCount {
    /// Triangles each node belongs to.
    pub per_node: Vec<u64>,
    /// Distinct triangles in the graph (`sum(per_node) / 3`).
    pub total: u64,
}

/// Counts triangles with the degree-ordered forward algorithm: every edge points
/// from the lower to the higher `(degree, id)` rank and forward lists are merged.
///
/// Complexity: `O(m^{3/2})` time, `O(n + m)` memory.
pub fn triangle_count(g: &Projection) -> TriangleCount {
    let s = Simple::new(g);
    let n = g.node_count();
    let rank_key = |u: u32| (s.degree(u), u);
    let mut fwd: Vec<Vec<u32>> = vec![Vec::new(); n];
    for u in 0..n as u32 {
        for &v in s.of(u) {
            if rank_key(u) < rank_key(v) {
                fwd[u as usize].push(v);
            }
        }
        fwd[u as usize].sort_unstable();
    }
    let mut per_node = vec![0u64; n];
    let mut total = 0u64;
    for u in 0..n {
        for &v in &fwd[u] {
            let (a, b) = (&fwd[u], &fwd[v as usize]);
            let (mut i, mut j) = (0, 0);
            while i < a.len() && j < b.len() {
                match a[i].cmp(&b[j]) {
                    std::cmp::Ordering::Less => i += 1,
                    std::cmp::Ordering::Greater => j += 1,
                    std::cmp::Ordering::Equal => {
                        per_node[u] += 1;
                        per_node[v as usize] += 1;
                        per_node[a[i] as usize] += 1;
                        total += 1;
                        i += 1;
                        j += 1;
                    }
                }
            }
        }
    }
    TriangleCount { per_node, total }
}

/// Local clustering coefficients.
#[derive(Debug, Clone, PartialEq)]
pub struct Clustering {
    /// `2 * triangles / (d * (d - 1))` per node; 0 when `d < 2`.
    pub coefficients: Vec<f64>,
    /// Mean over all nodes (nodes with `d < 2` count as 0); 0 for an empty graph.
    pub average: f64,
}

/// Local clustering coefficient of every node, from [`triangle_count`].
///
/// Complexity: that of [`triangle_count`].
pub fn local_clustering_coefficient(g: &Projection) -> Clustering {
    let tri = triangle_count(g);
    let s = Simple::new(g);
    let coefficients: Vec<f64> = (0..g.node_count())
        .map(|u| {
            let d = s.degree(u as u32) as f64;
            if d < 2.0 {
                0.0
            } else {
                2.0 * tri.per_node[u] as f64 / (d * (d - 1.0))
            }
        })
        .collect();
    let average = if coefficients.is_empty() {
        0.0
    } else {
        coefficients.iter().sum::<f64>() / coefficients.len() as f64
    };
    Clustering {
        coefficients,
        average,
    }
}

/// k-core decomposition (Batagelj-Zaversnik bucket peeling). Returns each node's
/// core number: the largest `k` such that the node lies in a subgraph where every
/// node has degree at least `k`.
///
/// Complexity: `O(n + m)` time and memory.
pub fn k_core_decomposition(g: &Projection) -> Vec<u32> {
    let s = Simple::new(g);
    let n = g.node_count();
    if n == 0 {
        return Vec::new();
    }
    let mut deg: Vec<usize> = (0..n as u32).map(|u| s.degree(u)).collect();
    let max_deg = deg.iter().copied().max().unwrap_or(0);
    // bin[d] = start index in `vert` of nodes with current degree d
    let mut bin = vec![0usize; max_deg + 2];
    for &d in &deg {
        bin[d + 1] += 1;
    }
    for d in 0..=max_deg {
        bin[d + 1] += bin[d];
    }
    let mut pos = vec![0usize; n];
    let mut vert = vec![0u32; n];
    let mut fill = bin.clone();
    for v in 0..n {
        pos[v] = fill[deg[v]];
        vert[pos[v]] = v as u32;
        fill[deg[v]] += 1;
    }
    for i in 0..n {
        let v = vert[i];
        for &u in s.of(v) {
            let (u, v) = (u as usize, v as usize);
            if deg[u] > deg[v] {
                let du = deg[u];
                let pu = pos[u];
                let pw = bin[du];
                let w = vert[pw] as usize;
                if u != w {
                    pos[u] = pw;
                    pos[w] = pu;
                    vert[pu] = w as u32;
                    vert[pw] = u as u32;
                }
                bin[du] += 1;
                deg[u] -= 1;
            }
        }
    }
    deg.into_iter().map(|d| d as u32).collect()
}
