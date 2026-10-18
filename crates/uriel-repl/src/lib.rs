#![forbid(unsafe_code)]

mod builder;
mod replica;

pub use builder::BatchBuilder;
#[cfg(feature = "mutations")]
pub use replica::Mutation;
pub use replica::{Ack, Fault, Replica, Staged, Ticket, WriteError};
