use std::io;
use std::net::SocketAddr;

use thiserror::Error;

/// Errors surfaced by node setup and the peer RPC layer. `DistCache::get` never
/// returns these: peer failures degrade to a miss.
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("cannot bind {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        #[source]
        source: io::Error,
    },
    #[error("invalid configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("peer request timed out")]
    Timeout,
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("protocol error: {0}")]
    Protocol(String),
}
