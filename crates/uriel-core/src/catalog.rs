//! Index and constraint definitions, and the schema introspection result.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Kind of a node-property constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// No two nodes with the label may hold an equal value for the key.
    Unique,
    /// Every node with the label must have a non-null value for the key.
    Exists,
}

impl fmt::Display for ConstraintKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConstraintKind::Unique => "unique",
            ConstraintKind::Exists => "exists",
        })
    }
}

/// A property index on `(label, key)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IndexDef {
    /// Indexed label.
    pub label: String,
    /// Indexed property key.
    pub key: String,
    /// `true` if the index was created implicitly to back a unique constraint.
    pub backs_constraint: bool,
}

/// A constraint on `(label, key)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ConstraintDef {
    /// Constraint kind.
    pub kind: ConstraintKind,
    /// Constrained label.
    pub label: String,
    /// Constrained property key.
    pub key: String,
}

/// Schema information for one label or one relationship type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntitySchema {
    /// Label or relationship type name.
    pub name: String,
    /// Exact number of nodes with the label / relationships of the type.
    pub count: u64,
    /// Property keys seen in the sample, each with the sorted set of value type names
    /// observed (`Boolean`, `Integer`, `Float`, `String`, `Bytes`, `List`, `Map`, `Vector`).
    pub properties: BTreeMap<String, Vec<String>>,
    /// Number of entities inspected to build `properties`.
    pub sampled: u64,
}

/// Result of [`crate::ReadTx::schema`]. Serializes to JSON with `serde_json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    /// Graph epoch the schema was read at.
    pub epoch: u64,
    /// Total node count.
    pub node_count: u64,
    /// Total relationship count.
    pub rel_count: u64,
    /// One entry per label in use, sorted by name.
    pub labels: Vec<EntitySchema>,
    /// One entry per relationship type in use, sorted by name.
    pub rel_types: Vec<EntitySchema>,
    /// All property indexes.
    pub indexes: Vec<IndexDef>,
    /// All constraints.
    pub constraints: Vec<ConstraintDef>,
}
