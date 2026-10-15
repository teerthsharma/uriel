//! Error type shared by every fallible operation in `uriel-core`.

use crate::catalog::ConstraintKind;
use crate::ids::{NodeId, RelId};

/// Result alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong in `uriel-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The underlying `redb` storage engine reported a failure (I/O, corruption,
    /// transaction or table error).
    #[error("storage error: {0}")]
    Storage(Box<redb::Error>),

    /// Stored bytes could not be decoded, or an internal invariant was found broken.
    #[error("corrupt data: {0}")]
    Corrupt(String),

    /// The database file was written by an incompatible `uriel-core` format version.
    #[error("unsupported storage format version {found} (this build reads version {expected})")]
    UnsupportedFormat {
        /// Version found in the file.
        found: u64,
        /// Version this build understands.
        expected: u64,
    },

    /// No node with this id exists.
    #[error("node {0} not found")]
    NodeNotFound(NodeId),

    /// No relationship with this id exists.
    #[error("relationship {0} not found")]
    RelNotFound(RelId),

    /// `delete_node` was called on a node that still has relationships.
    #[error("node {0} still has relationships; use detach_delete_node")]
    NodeHasRelationships(NodeId),

    /// A write would violate a uniqueness or existence constraint, or a constraint
    /// cannot be created because existing data violates it.
    #[error("{kind} constraint on (:{label}) property `{key}` violated: {detail}")]
    ConstraintViolation {
        /// Which kind of constraint was violated.
        kind: ConstraintKind,
        /// Label the constraint is declared on.
        label: String,
        /// Property key the constraint is declared on.
        key: String,
        /// Human readable explanation (offending node, value).
        detail: String,
    },

    /// An index on `(label, key)` already exists.
    #[error("an index on (:{label}) `{key}` already exists")]
    IndexExists {
        /// Indexed label.
        label: String,
        /// Indexed property key.
        key: String,
    },

    /// No index on `(label, key)` exists.
    #[error("no index on (:{label}) `{key}`")]
    IndexNotFound {
        /// Indexed label.
        label: String,
        /// Indexed property key.
        key: String,
    },

    /// The index is the backing index of a uniqueness constraint; drop the constraint instead.
    #[error("index on (:{label}) `{key}` backs a unique constraint; drop the constraint instead")]
    IndexBacksConstraint {
        /// Indexed label.
        label: String,
        /// Indexed property key.
        key: String,
    },

    /// A constraint of this kind on `(label, key)` already exists.
    #[error("a {kind} constraint on (:{label}) `{key}` already exists")]
    ConstraintExists {
        /// Constraint kind.
        kind: ConstraintKind,
        /// Constrained label.
        label: String,
        /// Constrained property key.
        key: String,
    },

    /// No constraint of this kind on `(label, key)` exists.
    #[error("no {kind} constraint on (:{label}) `{key}`")]
    ConstraintNotFound {
        /// Constraint kind.
        kind: ConstraintKind,
        /// Constrained label.
        label: String,
        /// Constrained property key.
        key: String,
    },

    /// A label, relationship type or property key is empty or contains a NUL byte.
    #[error("invalid name {0:?}: names must be non-empty and must not contain NUL")]
    InvalidName(String),

    /// A property value cannot be stored (for example it is nested too deeply).
    #[error("invalid value: {0}")]
    InvalidValue(String),

    /// An earlier operation in this transaction failed half way through; the transaction
    /// can only be dropped (rolled back).
    #[error("transaction is poisoned by an earlier storage failure and cannot commit")]
    TransactionPoisoned,

    /// A JSON document is not a valid encoding of a [`crate::Value`].
    #[error("invalid JSON value encoding: {0}")]
    Json(String),

    #[error("record {id} in table `{table}` failed its checksum")]
    CorruptRecord { table: &'static str, id: u64 },

    #[error("the graph is read-only")]
    ReadOnly,

    #[error("cold data is unavailable")]
    Unavailable,
}

impl Error {
    pub(crate) fn is_io(&self) -> bool {
        match self {
            Error::Storage(e) => matches!(**e, redb::Error::Io(_) | redb::Error::PreviousIo),
            _ => false,
        }
    }

    pub(crate) fn is_already_open(&self) -> bool {
        matches!(self, Error::Storage(e) if matches!(**e, redb::Error::DatabaseAlreadyOpen))
    }
}

macro_rules! storage_from {
    ($($t:ty),* $(,)?) => {$(
        impl From<$t> for Error {
            fn from(e: $t) -> Self {
                Error::Storage(Box::new(redb::Error::from(e)))
            }
        }
    )*};
}

storage_from!(
    redb::DatabaseError,
    redb::TransactionError,
    redb::TableError,
    redb::StorageError,
    redb::CommitError,
);
