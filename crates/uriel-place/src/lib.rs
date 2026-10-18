#![deny(unsafe_code)]

pub mod arena;
pub mod calibrate;
pub mod config;
pub mod event;
pub mod fixed;
pub mod gate;
pub mod place;
pub mod replay;

pub use arena::{
    lower_bound, plan_append_only, plan_arena, plan_first_fit, plan_offsets, ArenaError, Buffer,
    Plan,
};
pub use config::{Config, Placement, Weights};
pub use event::{AccessKind, Event, Level, Lifetime, PlaceError, Region, RegionId, Tier};
pub use place::Placer;
