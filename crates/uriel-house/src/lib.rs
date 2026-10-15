//! # uriel-house
//!
//! The deterministic harness of House mode. The fellows (Foreman, Chase, Cameron) chart a
//! Need; this crate validates and merges the charts, stores the merged goal tree in a seal
//! database, schedules leaves under a resource governor, and accepts `Reached` only for a
//! goal whose bound test was RED on its first recorded run. It spawns no agents: the
//! House session dispatches the [`Batch`]es it emits.
//!
//! See `docs/HOUSE-SEAL.md` for the spec.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod governor;
pub mod house;
mod merge;
pub mod probe;
pub mod replay;
mod sched;
pub mod sink;
mod smart;
mod state;
mod store;
pub mod types;
pub mod validate;

pub use config::{HouseConfig, ModelSlots};
pub use error::HouseError;
pub use house::House;
pub use probe::{FakeProbe, RealProbe, SystemProbe, SystemSnapshot};
pub use replay::{decision_digest, DecisionRecord, DispatchRecord, HouseCall};
pub use sink::{EventSink, JsonlSink, VecSink};
pub use types::*;
