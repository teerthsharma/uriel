#![forbid(unsafe_code)]

mod client;
pub mod codec;
mod error;
mod host;
mod message;
mod server;
mod stats;
mod transport;

pub use client::{Client, DEFAULT_CALL_TIMEOUT};
pub use codec::{DEFAULT_MAX_FRAME, MAX_DECODE_DEPTH};
pub use error::{CodecError, ErrorCode, HostError};
pub use host::{HandlerHook, Host, HostConfig, ServeConfig};
pub use message::{
    encode_ping, encode_query, encode_search, encode_stats_request, Request, Response, MSG_BUSY,
    MSG_ERROR, MSG_PING, MSG_PONG, MSG_QUERY, MSG_QUERY_OK, MSG_SEARCH, MSG_SEARCH_OK, MSG_STATS,
    MSG_STATS_OK,
};
pub use uriel_core::Value;
pub use uriel_query::{AccessMode, QueryResult, QueryStats};
pub use uriel_rag::{ContextPack, Hit, HitVia, SearchOptions};
pub use stats::{BufferGauge, HostStats};
pub use transport::{ChannelTransport, Recv, TcpTransport, Transport, FRAME_DEADLINE};
