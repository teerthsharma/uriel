//! Immutable compressed-sparse-row (CSR) graph projections.
//!
//! A [`Projection`] is what every algorithm in this crate consumes. Node ids are
//! dense `u32` values `0..node_count`; the caller maps its own ids to and from
//! them. Adjacency lists are sorted by target id, so lookups are deterministic and
//! [`Projection::has_edge`] is a binary search.
//!
//! * **Directed** projections keep separate out- and in-adjacency.
//! * **Undirected** projections store each edge `{u, v}` as the two arcs `u -> v`
//!   and `v -> u`; a self loop is stored as a single arc. In- and out-adjacency
//!   are the same data.
//! * Parallel edges and self loops are kept unless an [`Aggregation`] other than
//!   [`Aggregation::None`] is chosen in the [`ProjectionBuilder`].
//!
//! Memory is `O(n + m)`; construction is `O(m log m)` (one stable sort).

use crate::error::{Error, Result};

/// How edges of the source graph become arcs of the projection (as in GDS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Edges keep their direction.
    Natural,
    /// Edges are flipped.
    Reverse,
    /// Edges are traversable both ways.
    Undirected,
}

/// How parallel arcs (same source and target) are merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aggregation {
    /// Keep every parallel arc.
    None,
    /// Keep the first arc inserted.
    Single,
    /// Merge into one arc whose weight is the sum.
    Sum,
    /// Merge into one arc with the smallest weight.
    Min,
    /// Merge into one arc with the largest weight.
    Max,
}

#[derive(Debug, Clone)]
struct Csr {
    offsets: Vec<usize>,
    targets: Vec<u32>,
    weights: Option<Vec<f64>>,
}

impl Csr {
    /// `arcs` must be sorted by source.
    fn from_sorted(node_count: usize, arcs: &[(u32, u32, f64)], weighted: bool) -> Self {
        let mut offsets = vec![0usize; node_count + 1];
        for &(s, _, _) in arcs {
            offsets[s as usize + 1] += 1;
        }
        for i in 0..node_count {
            offsets[i + 1] += offsets[i];
        }
        Self {
            offsets,
            targets: arcs.iter().map(|a| a.1).collect(),
            weights: weighted.then(|| arcs.iter().map(|a| a.2).collect()),
        }
    }

    fn range(&self, u: u32) -> std::ops::Range<usize> {
        self.offsets[u as usize]..self.offsets[u as usize + 1]
    }
}

/// An immutable CSR graph with out- and in-adjacency and optional `f64` weights.
#[derive(Debug, Clone)]
pub struct Projection {
    node_count: usize,
    directed: bool,
    edge_count: usize,
    out: Csr,
    /// `None` for undirected projections (in-adjacency equals out-adjacency).
    inc: Option<Csr>,
}

impl Projection {
    /// Builds an unweighted projection. `directed == false` symmetrises every edge.
    ///
    /// Errors with [`Error::NodeOutOfRange`] if an endpoint is `>= node_count`.
    /// Complexity: `O(m log m)`.
    pub fn from_edges(node_count: usize, edges: &[(u32, u32)], directed: bool) -> Result<Self> {
        let mut b = ProjectionBuilder::new(node_count).orientation(orientation_of(directed));
        b.edges.extend(edges.iter().map(|&(u, v)| (u, v, 1.0)));
        b.build()
    }

    /// Builds a weighted projection from `(source, target, weight)` triples.
    pub fn from_weighted_edges(
        node_count: usize,
        edges: &[(u32, u32, f64)],
        directed: bool,
    ) -> Result<Self> {
        let mut b = ProjectionBuilder::new(node_count)
            .orientation(orientation_of(directed))
            .weighted(true);
        b.edges.extend_from_slice(edges);
        b.build()
    }

    /// Starts a [`ProjectionBuilder`] for `node_count` nodes.
    pub fn builder(node_count: usize) -> ProjectionBuilder {
        ProjectionBuilder::new(node_count)
    }

    /// Number of nodes.
    pub fn node_count(&self) -> usize {
        self.node_count
    }

    /// Number of logical edges (an undirected edge counts once).
    pub fn edge_count(&self) -> usize {
        self.edge_count
    }

    /// Number of stored directed arcs (`2 * edges` for undirected, minus self loops).
    pub fn arc_count(&self) -> usize {
        self.out.targets.len()
    }

    /// `true` if the projection is directed.
    pub fn is_directed(&self) -> bool {
        self.directed
    }

    /// `true` if arcs carry weights.
    pub fn is_weighted(&self) -> bool {
        self.out.weights.is_some()
    }

    fn in_csr(&self) -> &Csr {
        self.inc.as_ref().unwrap_or(&self.out)
    }

    /// Out-neighbours of `u`, sorted ascending (parallel arcs repeat).
    /// Panics if `u >= node_count`.
    pub fn out_neighbors(&self, u: u32) -> &[u32] {
        &self.out.targets[self.out.range(u)]
    }

    /// Weights aligned with [`Self::out_neighbors`], or `None` if unweighted.
    pub fn out_weights(&self, u: u32) -> Option<&[f64]> {
        self.out.weights.as_ref().map(|w| &w[self.out.range(u)])
    }

    /// In-neighbours of `u`, sorted ascending.
    pub fn in_neighbors(&self, u: u32) -> &[u32] {
        let c = self.in_csr();
        &c.targets[c.range(u)]
    }

    /// Weights aligned with [`Self::in_neighbors`], or `None` if unweighted.
    pub fn in_weights(&self, u: u32) -> Option<&[f64]> {
        let c = self.in_csr();
        c.weights.as_ref().map(|w| &w[c.range(u)])
    }

    /// Number of out-arcs of `u`.
    pub fn out_degree(&self, u: u32) -> usize {
        self.out.range(u).len()
    }

    /// Number of in-arcs of `u`.
    pub fn in_degree(&self, u: u32) -> usize {
        self.in_csr().range(u).len()
    }

    /// Total degree: `out + in` when directed, the number of incident arcs when
    /// undirected (a self loop counts once).
    pub fn degree(&self, u: u32) -> usize {
        if self.directed {
            self.out_degree(u) + self.in_degree(u)
        } else {
            self.out_degree(u)
        }
    }

    /// Out-arcs of `u` as `(target, weight)`; the weight is `1.0` when unweighted.
    pub fn out_edges(&self, u: u32) -> impl Iterator<Item = (u32, f64)> + '_ {
        let r = self.out.range(u);
        let w = self.out.weights.as_deref();
        r.map(move |i| (self.out.targets[i], w.map_or(1.0, |w| w[i])))
    }

    /// In-arcs of `u` as `(source, weight)`; the weight is `1.0` when unweighted.
    pub fn in_edges(&self, u: u32) -> impl Iterator<Item = (u32, f64)> + '_ {
        let c = self.in_csr();
        let r = c.range(u);
        let w = c.weights.as_deref();
        r.map(move |i| (c.targets[i], w.map_or(1.0, |w| w[i])))
    }

    /// Neighbours ignoring direction: out-arcs, then (if directed) in-arcs.
    pub fn neighbors_any(&self, u: u32) -> impl Iterator<Item = (u32, f64)> + '_ {
        let ins = if self.directed {
            Some(self.in_edges(u))
        } else {
            None
        };
        self.out_edges(u).chain(ins.into_iter().flatten())
    }

    /// `true` if at least one arc `u -> v` exists. `O(log deg(u))`.
    pub fn has_edge(&self, u: u32, v: u32) -> bool {
        (u as usize) < self.node_count && self.out_neighbors(u).binary_search(&v).is_ok()
    }

    /// Every logical edge once, as `(source, target, weight)`. Undirected edges
    /// are reported with `source <= target`. `O(m)`.
    pub fn edges(&self) -> Vec<(u32, u32, f64)> {
        let mut v = Vec::with_capacity(self.edge_count);
        for u in 0..self.node_count as u32 {
            for (t, w) in self.out_edges(u) {
                if self.directed || u <= t {
                    v.push((u, t, w));
                }
            }
        }
        v
    }

    /// Returns [`Error::NodeOutOfRange`] if `u >= node_count`.
    pub fn check_node(&self, u: u32) -> Result<()> {
        if (u as usize) < self.node_count {
            Ok(())
        } else {
            Err(Error::NodeOutOfRange {
                node: u,
                node_count: self.node_count,
            })
        }
    }
}

fn orientation_of(directed: bool) -> Orientation {
    if directed {
        Orientation::Natural
    } else {
        Orientation::Undirected
    }
}

/// Collects edges and produces a [`Projection`].
#[derive(Debug, Clone)]
pub struct ProjectionBuilder {
    node_count: usize,
    orientation: Orientation,
    aggregation: Aggregation,
    weighted: bool,
    edges: Vec<(u32, u32, f64)>,
}

impl ProjectionBuilder {
    /// A builder for `node_count` nodes: natural orientation, no aggregation,
    /// unweighted until a weighted edge is added.
    pub fn new(node_count: usize) -> Self {
        Self {
            node_count,
            orientation: Orientation::Natural,
            aggregation: Aggregation::None,
            weighted: false,
            edges: Vec::new(),
        }
    }

    /// Sets the orientation (default [`Orientation::Natural`]).
    pub fn orientation(mut self, o: Orientation) -> Self {
        self.orientation = o;
        self
    }

    /// Sets how parallel arcs are merged (default [`Aggregation::None`]).
    pub fn aggregation(mut self, a: Aggregation) -> Self {
        self.aggregation = a;
        self
    }

    /// Forces the projection to carry weights (unit weight for plain edges).
    pub fn weighted(mut self, w: bool) -> Self {
        self.weighted = w;
        self
    }

    /// Adds an edge with weight `1.0`.
    pub fn add_edge(mut self, u: u32, v: u32) -> Self {
        self.edges.push((u, v, 1.0));
        self
    }

    /// Adds an edge and makes the projection weighted.
    pub fn add_weighted_edge(mut self, u: u32, v: u32, w: f64) -> Self {
        self.weighted = true;
        self.edges.push((u, v, w));
        self
    }

    /// Validates and builds. Complexity `O(m log m)`.
    pub fn build(self) -> Result<Projection> {
        let n = self.node_count;
        if n > u32::MAX as usize {
            return Err(Error::TooManyNodes(n));
        }
        let undirected = self.orientation == Orientation::Undirected;
        let mut arcs: Vec<(u32, u32, f64)> = Vec::with_capacity(self.edges.len() * 2);
        for &(u, v, w) in &self.edges {
            for x in [u, v] {
                if x as usize >= n {
                    return Err(Error::NodeOutOfRange {
                        node: x,
                        node_count: n,
                    });
                }
            }
            if !w.is_finite() {
                return Err(Error::InvalidWeight(w));
            }
            match self.orientation {
                Orientation::Natural => arcs.push((u, v, w)),
                Orientation::Reverse => arcs.push((v, u, w)),
                Orientation::Undirected => {
                    arcs.push((u, v, w));
                    if u != v {
                        arcs.push((v, u, w));
                    }
                }
            }
        }
        // Stable sort: ties keep insertion order, so `Single` is deterministic.
        arcs.sort_by_key(|a| (a.0, a.1));
        let arcs = aggregate(arcs, self.aggregation);
        let weighted = self.weighted;
        let out = Csr::from_sorted(n, &arcs, weighted);
        let edge_count = if undirected {
            arcs.iter().filter(|a| a.0 <= a.1).count()
        } else {
            arcs.len()
        };
        let inc = if undirected {
            None
        } else {
            let mut rev: Vec<_> = arcs.iter().map(|&(s, t, w)| (t, s, w)).collect();
            rev.sort_by_key(|a| (a.0, a.1));
            Some(Csr::from_sorted(n, &rev, weighted))
        };
        Ok(Projection {
            node_count: n,
            directed: !undirected,
            edge_count,
            out,
            inc,
        })
    }
}

/// Merges runs of arcs with equal `(source, target)`; input must be sorted.
fn aggregate(arcs: Vec<(u32, u32, f64)>, how: Aggregation) -> Vec<(u32, u32, f64)> {
    if how == Aggregation::None {
        return arcs;
    }
    let mut out: Vec<(u32, u32, f64)> = Vec::with_capacity(arcs.len());
    for a in arcs {
        match out.last_mut() {
            Some(last) if last.0 == a.0 && last.1 == a.1 => match how {
                Aggregation::Sum => last.2 += a.2,
                Aggregation::Min => last.2 = last.2.min(a.2),
                Aggregation::Max => last.2 = last.2.max(a.2),
                Aggregation::Single | Aggregation::None => {}
            },
            _ => out.push(a),
        }
    }
    out
}
