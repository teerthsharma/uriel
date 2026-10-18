use std::collections::BTreeMap;
use std::time::Duration;

use uriel_core::Value;
use uriel_query::{AccessMode, QueryResult};
use uriel_rag::{ContextPack, IndexReport, SearchOptions};

use crate::codec::DEFAULT_MAX_FRAME;
use crate::error::HostError;
use crate::message::{
    encode_index, encode_index_file, encode_ping, encode_query, encode_search,
    encode_stats_request, Response,
};
use crate::stats::HostStats;
use crate::transport::{Recv, Transport};

pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Client {
    transport: Box<dyn Transport>,
    outbound: Vec<u8>,
    inbound: Vec<u8>,
    max_frame: usize,
    timeout: Duration,
    next_nonce: u64,
}

impl Client {
    pub fn connect(transport: impl Transport + 'static) -> Result<Client, HostError> {
        Client::connect_with(transport, DEFAULT_MAX_FRAME, DEFAULT_CALL_TIMEOUT)
    }

    pub fn connect_with(
        transport: impl Transport + 'static,
        max_frame: usize,
        timeout: Duration,
    ) -> Result<Client, HostError> {
        let mut client = Client {
            transport: Box::new(transport),
            outbound: Vec::new(),
            inbound: Vec::new(),
            max_frame,
            timeout,
            next_nonce: 1,
        };
        client.ping()?;
        Ok(client)
    }

    pub fn outbound_capacity(&self) -> usize {
        self.outbound.capacity()
    }

    pub fn inbound_capacity(&self) -> usize {
        self.inbound.capacity()
    }

    pub fn query(
        &mut self,
        cypher: &str,
        params: &BTreeMap<String, Value>,
        mode: AccessMode,
    ) -> Result<QueryResult, HostError> {
        encode_query(&mut self.outbound, cypher, params, mode, self.max_frame)?;
        match self.round_trip()? {
            Response::Query(result) => Ok(result),
            other => Err(unexpected(other, "query")),
        }
    }

    pub fn search(&mut self, query: &str, opts: &SearchOptions) -> Result<ContextPack, HostError> {
        encode_search(&mut self.outbound, query, opts, self.max_frame)?;
        match self.round_trip()? {
            Response::Search(pack) => Ok(pack),
            other => Err(unexpected(other, "search")),
        }
    }

    pub fn index(&mut self, root: &str) -> Result<IndexReport, HostError> {
        encode_index(&mut self.outbound, root, self.max_frame)?;
        match self.round_trip()? {
            Response::Indexed(report) => Ok(report),
            other => Err(unexpected(other, "index")),
        }
    }

    pub fn index_file(&mut self, path: &str, content: &str) -> Result<IndexReport, HostError> {
        encode_index_file(&mut self.outbound, path, content, self.max_frame)?;
        match self.round_trip()? {
            Response::Indexed(report) => Ok(report),
            other => Err(unexpected(other, "index_file")),
        }
    }

    pub fn stats(&mut self) -> Result<HostStats, HostError> {
        encode_stats_request(&mut self.outbound, self.max_frame)?;
        match self.round_trip()? {
            Response::Stats(stats) => Ok(stats),
            other => Err(unexpected(other, "stats")),
        }
    }

    pub fn ping(&mut self) -> Result<(), HostError> {
        let nonce = self.next_nonce;
        self.next_nonce = self.next_nonce.wrapping_add(1);
        encode_ping(&mut self.outbound, nonce, self.max_frame)?;
        match self.round_trip()? {
            Response::Pong { nonce: echoed } if echoed == nonce => Ok(()),
            Response::Pong { .. } => Err(HostError::Unexpected("pong carried a different nonce")),
            other => Err(unexpected(other, "ping")),
        }
    }

    fn round_trip(&mut self) -> Result<Response, HostError> {
        self.transport.send(&self.outbound)?;
        match self.transport.recv(&mut self.inbound, self.timeout)? {
            Recv::Timeout => Err(HostError::Timeout),
            Recv::Frame => Ok(Response::decode(&self.inbound, self.max_frame)?),
        }
    }
}

fn unexpected(response: Response, operation: &'static str) -> HostError {
    match response {
        Response::Busy => HostError::Busy,
        Response::Error { code, message } => HostError::Remote { code, message },
        _ => HostError::Unexpected(operation),
    }
}
