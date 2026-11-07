//! Streaming construction of CSR projections from a read snapshot.

use std::collections::BTreeSet;

use uriel_algo::{Orientation, Projection, ProjectionBuilder};
use uriel_core::{NodeId, ReadTx, Value};

/// Which part of the graph to project.
#[derive(Debug, Clone, Copy)]
pub struct Spec<'a> {
    /// Keep nodes carrying at least one of these labels; empty keeps every node.
    pub labels: &'a [String],
    /// Keep relationships of these types; empty keeps every type.
    pub rel_types: &'a [String],
    /// Relationship property read as the arc weight (missing or non-numeric counts as 1).
    pub weight: Option<&'a str>,
    /// Refuse to project more nodes than this.
    pub max_nodes: usize,
    /// Refuse to project more relationships than this.
    pub max_edges: usize,
}

impl Spec<'_> {
    /// The whole graph with no limits.
    pub fn everything() -> Spec<'static> {
        Spec {
            labels: &[],
            rel_types: &[],
            weight: None,
            max_nodes: usize::MAX,
            max_edges: usize::MAX,
        }
    }
}

/// Why a projection could not be built.
#[derive(Debug)]
pub enum ProjectError {
    /// The selection holds more nodes or relationships than the spec allows.
    TooLarge {
        /// `"nodes"` or `"edges"`.
        what: &'static str,
        /// The count found (a lower bound once the limit was passed while streaming).
        count: u64,
        /// The limit from the spec.
        limit: usize,
    },
    /// Storage failure.
    Core(uriel_core::Error),
    /// Projection failure (for example a non-finite weight).
    Algo(uriel_algo::Error),
}

impl From<uriel_core::Error> for ProjectError {
    fn from(e: uriel_core::Error) -> Self {
        ProjectError::Core(e)
    }
}

impl From<uriel_algo::Error> for ProjectError {
    fn from(e: uriel_algo::Error) -> Self {
        ProjectError::Algo(e)
    }
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::TooLarge { what, count, limit } => {
                write!(f, "{count} {what} exceed the limit of {limit}")
            }
            ProjectError::Core(e) => write!(f, "{e}"),
            ProjectError::Algo(e) => write!(f, "{e}"),
        }
    }
}

/// Node ids (sorted) and the arcs between them as dense indices into `ids`.
#[derive(Debug, Clone)]
pub struct Projected {
    /// Source node id of projection index `i`.
    pub ids: Vec<NodeId>,
    /// `(source, target, weight)` per relationship, in relationship id order.
    pub edges: Vec<(u32, u32, f64)>,
    /// Relationships whose weight property was absent or not a number.
    pub unweighted_edges: usize,
}

impl Projected {
    /// Index of a node id in the projection.
    pub fn index_of(&self, id: NodeId) -> Option<u32> {
        self.ids.binary_search(&id).ok().map(|i| i as u32)
    }

    /// Builds the CSR graph; with `weighted` the arcs carry the projected weights.
    pub fn projection(
        &self,
        orientation: Orientation,
        weighted: bool,
    ) -> Result<Projection, uriel_algo::Error> {
        let mut b = ProjectionBuilder::new(self.ids.len()).orientation(orientation);
        for &(u, v, w) in &self.edges {
            b = if weighted {
                b.add_weighted_edge(u, v, w)
            } else {
                b.add_edge(u, v)
            };
        }
        b.weighted(weighted).build()
    }
}

/// A numeric property value as `f64`.
pub fn weight_of(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Int(i)) => Some(*i as f64),
        Some(Value::Float(f)) => Some(*f),
        _ => None,
    }
}

/// Selects the node ids (sorted) the spec keeps, refusing past `max_nodes`.
pub fn select_nodes(read: &ReadTx, spec: &Spec) -> Result<Vec<NodeId>, ProjectError> {
    let too_large = |count: u64| ProjectError::TooLarge {
        what: "nodes",
        count,
        limit: spec.max_nodes,
    };
    if spec.labels.is_empty() {
        let total = read.node_count()?;
        if total > spec.max_nodes as u64 {
            return Err(too_large(total));
        }
        let mut ids: Vec<NodeId> = Vec::with_capacity(total as usize);
        for n in read.all_nodes()? {
            ids.push(n?.id);
        }
        ids.sort_unstable();
        return Ok(ids);
    }
    let mut sum = 0u64;
    for l in spec.labels {
        sum += read.count_nodes_with_label(l)?;
    }
    let mut seen: BTreeSet<NodeId> = BTreeSet::new();
    for l in spec.labels {
        for id in read.node_ids_by_label(l)? {
            seen.insert(id?);
            if seen.len() > spec.max_nodes {
                return Err(too_large(sum.max(seen.len() as u64)));
            }
        }
    }
    Ok(seen.into_iter().collect())
}

/// Streams the selected nodes and relationships out of `read`.
pub fn project(read: &ReadTx, spec: &Spec) -> Result<Projected, ProjectError> {
    let ids = select_nodes(read, spec)?;
    let mut edges: Vec<(u32, u32, f64)> = Vec::new();
    let mut unweighted_edges = 0usize;
    for r in read.all_rels()? {
        let r = r?;
        if !spec.rel_types.is_empty() && !spec.rel_types.contains(&r.rel_type) {
            continue;
        }
        let (Ok(a), Ok(b)) = (ids.binary_search(&r.src), ids.binary_search(&r.dst)) else {
            continue;
        };
        if edges.len() >= spec.max_edges {
            return Err(ProjectError::TooLarge {
                what: "edges",
                count: edges.len() as u64 + 1,
                limit: spec.max_edges,
            });
        }
        let w = match spec.weight {
            Some(key) => weight_of(r.props.get(key)).unwrap_or_else(|| {
                unweighted_edges += 1;
                1.0
            }),
            None => 1.0,
        };
        edges.push((a as u32, b as u32, w));
    }
    Ok(Projected {
        ids,
        edges,
        unweighted_edges,
    })
}
