//! Distributed cache for seal.
//!
//! * [`LocalCache`]: byte-bounded W-TinyLFU cache with epoch and tag invalidation.
//! * [`Ring`]: consistent-hash ring with virtual nodes.
//! * [`membership`]: SWIM-lite gossip over UDP (failure detection + anti-entropy).
//! * [`DistCache`]: local cache + ring + a length-prefixed TCP protocol to peers.
//! * [`CacheNode`]: spawns all of the above on one address.

pub mod dist;
pub mod error;
pub mod hash;
pub mod local;
pub mod membership;
pub mod node;
mod protocol;
pub mod ring;

pub use dist::{DistCache, DistConfig, DistStats};
pub use error::CacheError;
pub use local::{CacheEntry, CacheMeta, CacheStats, LocalCache};
pub use membership::{GossipConfig, Member, MemberState};
pub use node::{CacheNode, NodeConfig};
pub use ring::{NodeId, Ring, DEFAULT_VNODES};
