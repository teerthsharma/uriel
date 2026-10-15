
#![forbid(unsafe_code)]

mod cache;
mod codec;
mod controller;
mod error;
mod gate;
mod hash;
mod sketch;

pub use cache::{CacheStats, Evicted, HotCache, Insert};
pub use codec::{
    decode_value, decode_value_cow, encode_value, is_tagged, logical_len, Codec, Cold, Lz4, Raw,
    Tier, MAX_DECODED_LEN, TAG_COLD, TAG_LZ4, TAG_RAW,
};
pub use controller::{Budget, KeyClass, Shed, TierController};
pub use error::{Result, TierError};
pub use gate::Gate;
pub use hash::{stable_hash, StableHasher};
pub use sketch::Sketch;
