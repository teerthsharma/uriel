#![forbid(unsafe_code)]

pub mod afk;
pub mod config;
pub mod dept;
pub mod input;
pub mod policy;
pub mod router;
pub mod ticket;
pub mod types;

pub use config::RouterConfig;
pub use input::{Effect, Input};
pub use policy::{
    changes_for, gate, revalidate, AcceptMode, Admission, Admitted, Cell, Policy, Rule,
    ScopePolicy, SourcePolicy,
};
pub use router::{replay, AttentionItem, ObserveReport, Router};
pub use ticket::{Assignment, InputFail, Phase, Status, Ticket};
pub use types::*;
