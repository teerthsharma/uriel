//! Error type of the harness.

use crate::types::GoalId;

/// Everything that can go wrong outside of chart validation (which returns `Violation`s).
#[derive(Debug, thiserror::Error)]
pub enum HouseError {
    /// The seal database failed.
    #[error("graph: {0}")]
    Graph(#[from] uriel_core::Error),
    /// Stored JSON could not be read or written.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// Filesystem access failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// No Need exists yet.
    #[error("no need has been stated; call need() first")]
    NoNeed,
    /// A Need already exists in this database.
    #[error("a need already exists ({0})")]
    NeedExists(GoalId),
    /// The merged chart does not exist yet.
    #[error("no merged chart; call merge() first")]
    NotMerged,
    /// Unknown goal id.
    #[error("unknown goal {0}")]
    UnknownGoal(GoalId),
    /// Unknown test name.
    #[error("unknown test `{0}`")]
    UnknownTest(String),
    /// A stored record is malformed.
    #[error("corrupt house state: {0}")]
    Corrupt(String),
    /// The goal has already had its one Dr House leap.
    #[error("goal {0} has already used its leap")]
    LeapAlreadyUsed(GoalId),
    /// A caller supplied something unusable.
    #[error("invalid input: {0}")]
    Invalid(String),
}
