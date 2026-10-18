use std::collections::BTreeMap;

use uriel_core::Value;
use uriel_query::{AccessMode, QueryResult, QueryStats};
use uriel_rag::{ContextPack, Hit, HitVia, IndexReport, SearchOptions};

use crate::codec::{
    begin_frame, finish_frame, parse_frame, put_len, put_str, put_u32, put_u64, put_u8,
    put_usize, put_f32, put_map, put_value, Cursor,
};
use crate::error::{CodecError, ErrorCode};
use crate::stats::HostStats;

pub const MSG_QUERY: u8 = 0x01;
pub const MSG_SEARCH: u8 = 0x02;
pub const MSG_STATS: u8 = 0x03;
pub const MSG_PING: u8 = 0x04;
pub const MSG_INDEX: u8 = 0x05;
pub const MSG_INDEX_FILE: u8 = 0x06;
pub const MSG_QUERY_OK: u8 = 0x81;
pub const MSG_SEARCH_OK: u8 = 0x82;
pub const MSG_STATS_OK: u8 = 0x83;
pub const MSG_PONG: u8 = 0x84;
pub const MSG_INDEX_OK: u8 = 0x85;
pub const MSG_ERROR: u8 = 0xE0;
pub const MSG_BUSY: u8 = 0xE1;

#[derive(Debug, Clone)]
pub enum Request {
    Query {
        cypher: String,
        params: BTreeMap<String, Value>,
        mode: AccessMode,
    },
    Search {
        query: String,
        opts: SearchOptions,
    },
    Stats,
    Ping {
        nonce: u64,
    },
    Index {
        root: String,
    },
    IndexFile {
        path: String,
        content: String,
    },
}

fn same_options(a: &SearchOptions, b: &SearchOptions) -> bool {
    a.k == b.k
        && a.budget_tokens == b.budget_tokens
        && a.expand_hops == b.expand_hops
        && a.use_utility == b.use_utility
}

impl PartialEq for Request {
    fn eq(&self, other: &Request) -> bool {
        match (self, other) {
            (
                Request::Query {
                    cypher: c1,
                    params: p1,
                    mode: m1,
                },
                Request::Query {
                    cypher: c2,
                    params: p2,
                    mode: m2,
                },
            ) => c1 == c2 && p1 == p2 && m1 == m2,
            (
                Request::Search { query: q1, opts: o1 },
                Request::Search { query: q2, opts: o2 },
            ) => q1 == q2 && same_options(o1, o2),
            (Request::Stats, Request::Stats) => true,
            (Request::Ping { nonce: a }, Request::Ping { nonce: b }) => a == b,
            (Request::Index { root: a }, Request::Index { root: b }) => a == b,
            (
                Request::IndexFile {
                    path: p1,
                    content: c1,
                },
                Request::IndexFile {
                    path: p2,
                    content: c2,
                },
            ) => p1 == p2 && c1 == c2,
            _ => false,
        }
    }
}

pub fn encode_query(
    buf: &mut Vec<u8>,
    cypher: &str,
    params: &BTreeMap<String, Value>,
    mode: AccessMode,
    max_frame: usize,
) -> Result<(), CodecError> {
    begin_frame(buf, MSG_QUERY);
    put_str(buf, cypher);
    put_map(buf, params, 0)?;
    put_u8(
        buf,
        match mode {
            AccessMode::Read => 0,
            AccessMode::Write => 1,
        },
    );
    finish_frame(buf, max_frame)
}

pub fn encode_search(
    buf: &mut Vec<u8>,
    query: &str,
    opts: &SearchOptions,
    max_frame: usize,
) -> Result<(), CodecError> {
    begin_frame(buf, MSG_SEARCH);
    put_str(buf, query);
    put_usize(buf, opts.k);
    put_usize(buf, opts.budget_tokens);
    put_usize(buf, opts.expand_hops);
    put_u8(buf, u8::from(opts.use_utility));
    finish_frame(buf, max_frame)
}

pub fn encode_ping(buf: &mut Vec<u8>, nonce: u64, max_frame: usize) -> Result<(), CodecError> {
    begin_frame(buf, MSG_PING);
    put_u64(buf, nonce);
    finish_frame(buf, max_frame)
}

pub fn encode_stats_request(buf: &mut Vec<u8>, max_frame: usize) -> Result<(), CodecError> {
    begin_frame(buf, MSG_STATS);
    finish_frame(buf, max_frame)
}

pub fn encode_index(buf: &mut Vec<u8>, root: &str, max_frame: usize) -> Result<(), CodecError> {
    begin_frame(buf, MSG_INDEX);
    put_str(buf, root);
    finish_frame(buf, max_frame)
}

pub fn encode_index_file(
    buf: &mut Vec<u8>,
    path: &str,
    content: &str,
    max_frame: usize,
) -> Result<(), CodecError> {
    begin_frame(buf, MSG_INDEX_FILE);
    put_str(buf, path);
    put_str(buf, content);
    finish_frame(buf, max_frame)
}

impl Request {
    pub fn encode(&self, buf: &mut Vec<u8>, max_frame: usize) -> Result<(), CodecError> {
        match self {
            Request::Query {
                cypher,
                params,
                mode,
            } => encode_query(buf, cypher, params, *mode, max_frame),
            Request::Search { query, opts } => encode_search(buf, query, opts, max_frame),
            Request::Stats => encode_stats_request(buf, max_frame),
            Request::Ping { nonce } => encode_ping(buf, *nonce, max_frame),
            Request::Index { root } => encode_index(buf, root, max_frame),
            Request::IndexFile { path, content } => {
                encode_index_file(buf, path, content, max_frame)
            }
        }
    }

    pub fn decode(frame: &[u8], max_frame: usize) -> Result<Request, CodecError> {
        let (kind, payload) = parse_frame(frame, max_frame)?;
        let mut c = Cursor::new(payload);
        let request = match kind {
            MSG_QUERY => {
                let cypher = c.string()?;
                let params = c.map(0)?;
                let mode = match c.u8()? {
                    0 => AccessMode::Read,
                    1 => AccessMode::Write,
                    other => return Err(CodecError::UnknownTag(other)),
                };
                Request::Query {
                    cypher,
                    params,
                    mode,
                }
            }
            MSG_SEARCH => {
                let query = c.string()?;
                let k = c.usize()?;
                let budget_tokens = c.usize()?;
                let expand_hops = c.usize()?;
                let use_utility = c.bool()?;
                Request::Search {
                    query,
                    opts: SearchOptions {
                        k,
                        budget_tokens,
                        expand_hops,
                        use_utility,
                    },
                }
            }
            MSG_STATS => Request::Stats,
            MSG_PING => Request::Ping { nonce: c.u64()? },
            MSG_INDEX => Request::Index { root: c.string()? },
            MSG_INDEX_FILE => {
                let path = c.string()?;
                let content = c.string()?;
                Request::IndexFile { path, content }
            }
            other => return Err(CodecError::UnknownMessage(other)),
        };
        c.finish()?;
        Ok(request)
    }
}

#[derive(Debug, Clone)]
pub enum Response {
    Query(QueryResult),
    Search(ContextPack),
    Stats(HostStats),
    Pong { nonce: u64 },
    Indexed(IndexReport),
    Error { code: ErrorCode, message: String },
    Busy,
}

fn same_pack(a: &ContextPack, b: &ContextPack) -> bool {
    a.hits == b.hits
        && a.text == b.text
        && a.tokens == b.tokens
        && a.budget_tokens == b.budget_tokens
        && a.omitted == b.omitted
}

impl PartialEq for Response {
    fn eq(&self, other: &Response) -> bool {
        match (self, other) {
            (Response::Query(a), Response::Query(b)) => a == b,
            (Response::Search(a), Response::Search(b)) => same_pack(a, b),
            (Response::Stats(a), Response::Stats(b)) => a == b,
            (Response::Pong { nonce: a }, Response::Pong { nonce: b }) => a == b,
            (Response::Indexed(a), Response::Indexed(b)) => a == b,
            (
                Response::Error {
                    code: c1,
                    message: m1,
                },
                Response::Error {
                    code: c2,
                    message: m2,
                },
            ) => c1 == c2 && m1 == m2,
            (Response::Busy, Response::Busy) => true,
            _ => false,
        }
    }
}

fn put_query_result(buf: &mut Vec<u8>, result: &QueryResult) -> Result<(), CodecError> {
    put_len(buf, result.columns.len());
    for column in &result.columns {
        put_str(buf, column);
    }
    put_len(buf, result.rows.len());
    for row in &result.rows {
        put_len(buf, row.len());
        for cell in row {
            put_value(buf, cell, 0)?;
        }
    }
    let s = &result.stats;
    for counter in [
        s.nodes_created,
        s.nodes_deleted,
        s.rels_created,
        s.rels_deleted,
        s.properties_set,
        s.labels_added,
        s.labels_removed,
        s.indexes_added,
        s.constraints_added,
        s.rows_returned,
    ] {
        put_u64(buf, counter);
    }
    put_u8(buf, u8::from(s.from_cache));
    Ok(())
}

fn get_query_result(c: &mut Cursor<'_>) -> Result<QueryResult, CodecError> {
    let column_count = c.count(4)?;
    let mut columns = Vec::with_capacity(column_count);
    for _ in 0..column_count {
        columns.push(c.string()?);
    }
    let row_count = c.count(4)?;
    let mut rows = Vec::with_capacity(row_count);
    for _ in 0..row_count {
        let cell_count = c.count(1)?;
        let mut row = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            row.push(c.value(0)?);
        }
        rows.push(row);
    }
    let nodes_created = c.u64()?;
    let nodes_deleted = c.u64()?;
    let rels_created = c.u64()?;
    let rels_deleted = c.u64()?;
    let properties_set = c.u64()?;
    let labels_added = c.u64()?;
    let labels_removed = c.u64()?;
    let indexes_added = c.u64()?;
    let constraints_added = c.u64()?;
    let rows_returned = c.u64()?;
    let from_cache = c.bool()?;
    Ok(QueryResult {
        columns,
        rows,
        stats: QueryStats {
            nodes_created,
            nodes_deleted,
            rels_created,
            rels_deleted,
            properties_set,
            labels_added,
            labels_removed,
            indexes_added,
            constraints_added,
            from_cache,
            rows_returned,
        },
    })
}

fn put_pack(buf: &mut Vec<u8>, pack: &ContextPack) {
    put_len(buf, pack.hits.len());
    for hit in &pack.hits {
        put_u64(buf, hit.chunk_id);
        put_str(buf, &hit.file);
        put_u32(buf, hit.start_line);
        put_u32(buf, hit.end_line);
        put_str(buf, &hit.memory);
        put_f32(buf, hit.score);
        put_u8(
            buf,
            match hit.via {
                HitVia::Direct => 0,
                HitVia::GraphNeighbour => 1,
            },
        );
    }
    put_str(buf, &pack.text);
    put_usize(buf, pack.tokens);
    put_usize(buf, pack.budget_tokens);
    put_usize(buf, pack.omitted);
}

fn get_pack(c: &mut Cursor<'_>) -> Result<ContextPack, CodecError> {
    let hit_count = c.count(29)?;
    let mut hits = Vec::with_capacity(hit_count);
    for _ in 0..hit_count {
        let chunk_id = c.u64()?;
        let file = c.string()?;
        let start_line = c.u32()?;
        let end_line = c.u32()?;
        let memory = c.string()?;
        let score = c.f32()?;
        let via = match c.u8()? {
            0 => HitVia::Direct,
            1 => HitVia::GraphNeighbour,
            other => return Err(CodecError::UnknownTag(other)),
        };
        hits.push(Hit {
            chunk_id,
            file,
            start_line,
            end_line,
            memory,
            score,
            via,
        });
    }
    let text = c.string()?;
    let tokens = c.usize()?;
    let budget_tokens = c.usize()?;
    let omitted = c.usize()?;
    Ok(ContextPack {
        hits,
        text,
        tokens,
        budget_tokens,
        omitted,
    })
}

fn put_report(buf: &mut Vec<u8>, r: &IndexReport) {
    for v in [
        r.files_scanned,
        r.files_indexed,
        r.files_unchanged,
        r.files_skipped,
        r.files_removed,
        r.chunks_added,
        r.chunks_removed,
    ] {
        put_usize(buf, v);
    }
}

fn get_report(c: &mut Cursor<'_>) -> Result<IndexReport, CodecError> {
    Ok(IndexReport {
        files_scanned: c.usize()?,
        files_indexed: c.usize()?,
        files_unchanged: c.usize()?,
        files_skipped: c.usize()?,
        files_removed: c.usize()?,
        chunks_added: c.usize()?,
        chunks_removed: c.usize()?,
    })
}

fn put_host_stats(buf: &mut Vec<u8>, s: &HostStats) {
    for v in [
        s.arena_cap,
        s.arena_used,
        s.epoch,
        s.requests_served,
        s.requests_failed,
        s.requests_busy,
        s.in_flight,
        s.queue_len,
        s.connections,
        s.chunks,
        s.files,
        s.embed_dim,
    ] {
        put_u64(buf, v);
    }
}

fn get_host_stats(c: &mut Cursor<'_>) -> Result<HostStats, CodecError> {
    Ok(HostStats {
        arena_cap: c.u64()?,
        arena_used: c.u64()?,
        epoch: c.u64()?,
        requests_served: c.u64()?,
        requests_failed: c.u64()?,
        requests_busy: c.u64()?,
        in_flight: c.u64()?,
        queue_len: c.u64()?,
        connections: c.u64()?,
        chunks: c.u64()?,
        files: c.u64()?,
        embed_dim: c.u64()?,
    })
}

impl Response {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Response {
        Response::Error {
            code,
            message: message.into(),
        }
    }

    pub fn encode(&self, buf: &mut Vec<u8>, max_frame: usize) -> Result<(), CodecError> {
        match self {
            Response::Query(result) => {
                begin_frame(buf, MSG_QUERY_OK);
                put_query_result(buf, result)?;
            }
            Response::Search(pack) => {
                begin_frame(buf, MSG_SEARCH_OK);
                put_pack(buf, pack);
            }
            Response::Stats(stats) => {
                begin_frame(buf, MSG_STATS_OK);
                put_host_stats(buf, stats);
            }
            Response::Pong { nonce } => {
                begin_frame(buf, MSG_PONG);
                put_u64(buf, *nonce);
            }
            Response::Indexed(report) => {
                begin_frame(buf, MSG_INDEX_OK);
                put_report(buf, report);
            }
            Response::Error { code, message } => {
                begin_frame(buf, MSG_ERROR);
                put_u8(buf, *code as u8);
                put_str(buf, message);
            }
            Response::Busy => begin_frame(buf, MSG_BUSY),
        }
        finish_frame(buf, max_frame)
    }

    pub fn encode_or_error(&self, buf: &mut Vec<u8>, max_frame: usize) {
        let failure = match self.encode(buf, max_frame) {
            Ok(()) => return,
            Err(e) => e,
        };
        let code = match failure {
            CodecError::FrameTooLarge { .. } => ErrorCode::TooLarge,
            _ => ErrorCode::Internal,
        };
        let fallback = Response::error(code, failure.to_string());
        if fallback.encode(buf, max_frame).is_err() {
            Response::Busy
                .encode(buf, max_frame)
                .unwrap_or_else(|_| buf.clear());
        }
    }

    pub fn decode(frame: &[u8], max_frame: usize) -> Result<Response, CodecError> {
        let (kind, payload) = parse_frame(frame, max_frame)?;
        let mut c = Cursor::new(payload);
        let response = match kind {
            MSG_QUERY_OK => Response::Query(get_query_result(&mut c)?),
            MSG_SEARCH_OK => Response::Search(get_pack(&mut c)?),
            MSG_STATS_OK => Response::Stats(get_host_stats(&mut c)?),
            MSG_PONG => Response::Pong { nonce: c.u64()? },
            MSG_INDEX_OK => Response::Indexed(get_report(&mut c)?),
            MSG_ERROR => {
                let code = ErrorCode::from_wire(c.u8()?)?;
                let message = c.string()?;
                Response::Error { code, message }
            }
            MSG_BUSY => Response::Busy,
            other => return Err(CodecError::UnknownMessage(other)),
        };
        c.finish()?;
        Ok(response)
    }
}
