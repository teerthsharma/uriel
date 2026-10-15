//! Identifiers and the plain data records returned by reads.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::value::Value;

/// Property map of a node or relationship. Ordered by key so iteration is deterministic.
pub type Props = BTreeMap<String, Value>;

/// Identifier of a node. Allocated monotonically, persisted, and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

/// Identifier of a relationship. Allocated monotonically, persisted, and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for RelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A node as stored: its labels and properties.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Node identifier.
    pub id: NodeId,
    /// Labels, deduplicated and sorted.
    pub labels: BTreeSet<String>,
    /// Properties. A property is never [`Value::Null`]; setting `Null` removes the key.
    pub props: Props,
}

/// A relationship as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Rel {
    /// Relationship identifier.
    pub id: RelId,
    /// Start node.
    pub src: NodeId,
    /// End node.
    pub dst: NodeId,
    /// Relationship type.
    pub rel_type: String,
    /// Properties (never `Null`).
    pub props: Props,
}

/// Cheap view of one adjacency entry, produced by [`crate::ReadTx::out_edges`] and
/// [`crate::ReadTx::in_edges`] without touching the relationship record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The relationship.
    pub rel: RelId,
    /// Its type.
    pub rel_type: String,
    /// The node at the far end: the destination for an outgoing edge, the source for an
    /// incoming one.
    pub other: NodeId,
}
