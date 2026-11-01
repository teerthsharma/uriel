//! A bounded graph of tiny goals with dependencies.
//!
//! The order is a topological sort (Kahn's algorithm) with a lexicographic tie-break, so the same
//! goals give the same layers whatever order they were added in. A goal is `pending`, `issued`,
//! `done` or `failed`; `blocked` is derived (a pending goal with a failed goal among its
//! transitive dependencies). There is no I/O and no clock: the same calls give the same answer.
//!
//! The JSON form (`uriel.ledger.v1`) is read by `scripts/ledger_next.py`, which re-implements the
//! layering and the ready rule independently; `tests/python_agrees.rs` keeps the two in step.
#![forbid(unsafe_code)]

mod error;
mod ledger;

pub use error::Error;
pub use ledger::{
    AddReport, Ledger, NewGoal, State, MAX_BATCH, MAX_DETAIL_BYTES, MAX_GOALS, MAX_NEEDS,
    MAX_RESULT_BYTES, MAX_TITLE_BYTES, SCHEMA,
};
