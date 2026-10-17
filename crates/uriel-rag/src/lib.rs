#![forbid(unsafe_code)]

pub mod bm25;
pub mod chunk;
mod compress;
mod embed;
mod error;
pub mod eval;
mod index;
mod ladder;
mod scan;
mod tokenize;

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use uriel_core::{Graph, Node, NodeId, Props, ReadTx, Value};
use sha2::{Digest, Sha256};

use crate::chunk::{chunk_text, ChunkOptions, Lang, RawChunk};
use crate::compress::{compress, Corpus};
use crate::embed::QVec;
use crate::index::{rust_defs, rust_mentions, Entry, Index, HOP_DECAY};
use crate::ladder::gist;
use crate::tokenize::token_estimate;

pub use crate::embed::{Embedder, HashEmbedder};
pub use crate::error::RagError;
pub use crate::ladder::Level;

const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules"];
const UTILITY_ALPHA: f32 = 0.3;
const MAX_HOPS: usize = 2;
const L1_RATIO: usize = 4;
const LADDER_L1_HITS: usize = 1;
const LADDER_L2_HITS: usize = 3;
const FILE_REC_BYTES: usize = 96;
const CAP_CHECK_EVERY: usize = 256;
const GIST_FLUSH_BYTES: usize = 1 << 20;

fn default_level(rank: usize) -> Level {
    if rank < LADDER_L1_HITS {
        Level::L1
    } else if rank < LADDER_L1_HITS + LADDER_L2_HITS {
        Level::L2
    } else {
        Level::L3
    }
}

#[derive(Clone)]
pub struct RagConfig {
    pub compression_ratio: usize,
    pub embed_dim: usize,
    pub embedder: Option<Arc<dyn Embedder>>,
    pub max_file_bytes: u64,
    pub chunking: ChunkOptions,
}

impl Default for RagConfig {
    fn default() -> Self {
        RagConfig {
            compression_ratio: 16,
            embed_dim: 256,
            embedder: None,
            max_file_bytes: 1 << 20,
            chunking: ChunkOptions::default(),
        }
    }
}

impl fmt::Debug for RagConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RagConfig")
            .field("compression_ratio", &self.compression_ratio)
            .field("embed_dim", &self.embed_dim)
            .field("embedder", &self.embedder.as_ref().map(|e| e.dim()))
            .field("max_file_bytes", &self.max_file_bytes)
            .field("chunking", &self.chunking)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub prune: bool,
    pub skip_dirs: Vec<String>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            prune: true,
            skip_dirs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub files_unchanged: usize,
    pub files_skipped: usize,
    pub files_removed: usize,
    pub chunks_added: usize,
    pub chunks_removed: usize,
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub k: usize,
    pub budget_tokens: usize,
    pub expand_hops: usize,
    pub use_utility: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        SearchOptions {
            k: 8,
            budget_tokens: 2000,
            expand_hops: 0,
            use_utility: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitVia {
    Direct,
    GraphNeighbour,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub chunk_id: u64,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub memory: String,
    pub score: f32,
    pub via: HitVia,
}

#[derive(Debug, Clone)]
pub struct ContextPack {
    pub hits: Vec<Hit>,
    pub text: String,
    pub tokens: usize,
    pub budget_tokens: usize,
    pub omitted: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LadderHit {
    pub chunk_id: u64,
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub via: HitVia,
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct LadderPack {
    pub hits: Vec<LadderHit>,
    pub text: String,
    pub tokens: usize,
    pub budget_tokens: usize,
    pub omitted: usize,
}

#[derive(Debug, Clone)]
pub struct RagStats {
    pub chunks: usize,
    pub files: usize,
    pub avg_compression_ratio: f64,
    pub text_bytes: u64,
    pub memory_bytes: u64,
    pub index_bytes: u64,
    pub vector_bytes: u64,
    pub spilled: bool,
}

#[derive(Debug)]
struct FileRec {
    node: NodeId,
    hash: String,
    dir: bool,
    chunks: Vec<u64>,
}

struct Piece {
    body: String,
    rendered: String,
    tokens: usize,
}

impl Piece {
    fn new(head: &str, body: &str, inline: bool) -> Piece {
        let rendered = if inline {
            format!("{head} {body}\n")
        } else {
            format!("{head}\n{body}\n")
        };
        Piece {
            tokens: token_estimate(&rendered),
            body: body.to_string(),
            rendered,
        }
    }
}

struct Rungs {
    l1: Option<Piece>,
    l2: Piece,
    l3: Piece,
}

impl Rungs {
    fn cheaper_than(&self, level: Level) -> Option<Level> {
        let here = self.at(level).tokens;
        let mut next = level.lower();
        while let Some(l) = next {
            if self.at(l).tokens < here {
                return Some(l);
            }
            next = l.lower();
        }
        None
    }

    fn at(&self, level: Level) -> &Piece {
        match level {
            Level::L0 | Level::L1 => self.l1.as_ref().unwrap_or(&self.l2),
            Level::L2 => &self.l2,
            Level::L3 => &self.l3,
        }
    }
}

#[derive(Default)]
struct State {
    index: Index,
    files: BTreeMap<String, FileRec>,
    spilled: bool,
}

impl State {
    fn files_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|(p, r)| (p.len() + r.hash.len() + r.chunks.len() * 8 + FILE_REC_BYTES) as u64)
            .sum()
    }

    fn over(&self, cap: u64) -> bool {
        self.index.resident_bytes() + self.files_bytes() > cap
    }

    fn spill(&mut self) {
        self.index = Index::default();
        self.spilled = true;
    }
}

struct PreparedChunk {
    start: u32,
    end: u32,
    text: String,
    mem: String,
    gist: String,
    l1: String,
    hash: String,
    vector: Vec<f32>,
    mentions: Vec<String>,
}

struct Prepared {
    path: String,
    hash: String,
    dir: bool,
    lang: Lang,
    chunks: Vec<PreparedChunk>,
}

struct Applied {
    path: String,
    hash: String,
    dir: bool,
    lang: Lang,
    node: NodeId,
    old_chunks: Vec<u64>,
    new_chunks: Vec<(u64, PreparedChunk)>,
}

pub struct Rag {
    graph: Graph,
    cfg: RagConfig,
    embedder: Arc<dyn Embedder>,
    state: RwLock<State>,
    gate: Mutex<()>,
    index_cap: u64,
}

impl fmt::Debug for Rag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rag")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

fn sha_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn corrupt(msg: impl Into<String>) -> RagError {
    RagError::Corrupt(msg.into())
}

fn prop_str<'a>(n: &'a Node, key: &str) -> Result<&'a str, RagError> {
    match n.props.get(key) {
        Some(Value::String(s)) => Ok(s),
        _ => Err(corrupt(format!(
            "node {} lacks string property `{key}`",
            n.id
        ))),
    }
}

fn prop_line(n: &Node, key: &str) -> Result<u32, RagError> {
    match n.props.get(key) {
        Some(Value::Int(i)) => u32::try_from(*i)
            .map_err(|_| corrupt(format!("node {} has out-of-range `{key}`", n.id))),
        _ => Err(corrupt(format!(
            "node {} lacks integer property `{key}`",
            n.id
        ))),
    }
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> RagError + '_ {
    move |source| RagError::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl Rag {
    pub fn open(graph: Graph, cfg: RagConfig) -> Result<Rag, RagError> {
        let cap = graph.budget().rag_index() as u64;
        Rag::open_capped(graph, cfg, cap)
    }

    fn open_capped(graph: Graph, cfg: RagConfig, index_cap: u64) -> Result<Rag, RagError> {
        if cfg.compression_ratio == 0 {
            return Err(RagError::InvalidOptions(
                "compression_ratio must be at least 1".into(),
            ));
        }
        let embedder: Arc<dyn Embedder> = match &cfg.embedder {
            Some(e) => e.clone(),
            None => {
                if cfg.embed_dim == 0 {
                    return Err(RagError::InvalidOptions(
                        "embed_dim must be at least 1".into(),
                    ));
                }
                Arc::new(HashEmbedder::new(cfg.embed_dim))
            }
        };
        if !graph.is_read_only() && !graph.begin_read()?.has_index("File", "path")? {
            graph.create_index("File", "path")?;
        }
        let state = Self::load(&graph, embedder.as_ref(), index_cap)?;
        Ok(Rag {
            graph,
            cfg,
            embedder,
            state: RwLock::new(state),
            gate: Mutex::new(()),
            index_cap,
        })
    }

    fn flush_gists(graph: &Graph, pending: &mut Vec<(NodeId, String)>) {
        if pending.is_empty() || graph.is_read_only() {
            pending.clear();
            return;
        }
        if let Ok(mut tx) = graph.begin_write() {
            let done = pending.iter().all(|(id, g)| {
                let mut props = Props::new();
                props.insert("g3".into(), Value::from(g.as_str()));
                tx.set_node_props(*id, props).is_ok()
            });
            if done {
                let _ = tx.commit();
            }
        }
        pending.clear();
    }

    fn load(graph: &Graph, embedder: &dyn Embedder, cap: u64) -> Result<State, RagError> {
        let read = graph.begin_read()?;
        let mut state = State::default();
        let mut pending: Vec<(NodeId, String)> = Vec::new();
        let mut pending_bytes = 0usize;
        for node in read.nodes_by_label("File")? {
            let node = node?;
            let path = prop_str(&node, "path")?.to_string();
            let rec = FileRec {
                node: node.id,
                hash: prop_str(&node, "hash")?.to_string(),
                dir: prop_str(&node, "kind")? == "dir",
                chunks: Vec::new(),
            };
            state.files.insert(path, rec);
        }
        let mut seen = 0usize;
        for node in read.nodes_by_label("Chunk")? {
            let node = node?;
            let file = prop_str(&node, "file")?;
            let mem = prop_str(&node, "mem")?;
            let text = prop_str(&node, "text")?;
            let lang = Lang::from_path(file);
            let rec = state.files.get_mut(file).ok_or_else(|| {
                corrupt(format!(
                    "chunk {} belongs to unknown file `{file}`",
                    node.id
                ))
            })?;
            rec.chunks.push(node.id.0);
            prop_line(&node, "start")?;
            prop_line(&node, "end")?;
            if !matches!(node.props.get("g3"), Some(Value::String(_))) {
                let g = gist(text, lang);
                pending_bytes += g.len();
                pending.push((node.id, g));
                if pending_bytes >= GIST_FLUSH_BYTES {
                    Self::flush_gists(graph, &mut pending);
                    pending_bytes = 0;
                }
            }
            seen += 1;
            if state.spilled {
                continue;
            }
            let vector = scan::vector_of(&node, mem, embedder);
            state.index.insert(
                Entry {
                    id: node.id.0,
                    vector: QVec::from_f32(&vector),
                    utility: scan::utility_of(&node),
                    lang,
                    text_bytes: u32::try_from(text.len()).unwrap_or(u32::MAX),
                    mem_len: u32::try_from(mem.len()).unwrap_or(u32::MAX),
                },
                mem,
                if lang == Lang::Rust {
                    rust_mentions(text)
                } else {
                    Vec::new()
                },
            );
            if seen % CAP_CHECK_EVERY == 0 && state.over(cap) {
                state.spill();
            }
        }
        if !state.spilled && state.over(cap) {
            state.spill();
        }
        Self::flush_gists(graph, &mut pending);
        Ok(state)
    }

    fn read_state(&self) -> RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_state(&self) -> RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn index_dir(&self, root: &Path, opts: &IndexOptions) -> Result<IndexReport, RagError> {
        let _gate = lock(&self.gate);
        let mut files = Vec::new();
        let mut skip: Vec<&str> = SKIP_DIRS.to_vec();
        skip.extend(opts.skip_dirs.iter().map(String::as_str));
        walk(root, root, &skip, &mut files)?;

        let mut report = IndexReport::default();
        let mut corpus = Corpus::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut pending: Vec<(String, String, Lang, Vec<RawChunk>)> = Vec::new();
        for (rel, abs) in files {
            report.files_scanned += 1;
            let Ok(meta) = fs::metadata(&abs) else {
                report.files_skipped += 1;
                continue;
            };
            if meta.len() > self.cfg.max_file_bytes {
                report.files_skipped += 1;
                continue;
            }
            let Ok(bytes) = fs::read(&abs) else {
                report.files_skipped += 1;
                continue;
            };
            if bytes.iter().take(8000).any(|b| *b == 0) {
                report.files_skipped += 1;
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                report.files_skipped += 1;
                continue;
            };
            let lang = Lang::from_path(&rel);
            let chunks = chunk_text(&text, lang, &self.cfg.chunking);
            for c in &chunks {
                corpus.add(&c.text);
            }
            let hash = sha_hex(text.as_bytes());
            seen.insert(rel.clone());
            let unchanged = self
                .read_state()
                .files
                .get(&rel)
                .is_some_and(|r| r.dir && r.hash == hash);
            if unchanged {
                report.files_unchanged += 1;
            } else {
                pending.push((rel, hash, lang, chunks));
            }
        }
        let prepared: Vec<Prepared> = pending
            .into_iter()
            .map(|(path, hash, lang, chunks)| self.prepare(path, hash, true, lang, chunks, &corpus))
            .collect();
        let removals: Vec<String> = if opts.prune {
            self.read_state()
                .files
                .iter()
                .filter(|(p, r)| r.dir && !seen.contains(*p))
                .map(|(p, _)| p.clone())
                .collect()
        } else {
            Vec::new()
        };
        self.apply(prepared, removals, &mut report)?;
        Ok(report)
    }

    pub fn index_text(&self, source_id: &str, text: &str) -> Result<IndexReport, RagError> {
        let _gate = lock(&self.gate);
        let mut report = IndexReport {
            files_scanned: 1,
            ..IndexReport::default()
        };
        let hash = sha_hex(text.as_bytes());
        let unchanged = self
            .read_state()
            .files
            .get(source_id)
            .is_some_and(|r| !r.dir && r.hash == hash);
        if unchanged {
            report.files_unchanged = 1;
            return Ok(report);
        }
        let lang = Lang::from_path(source_id);
        let chunks = chunk_text(text, lang, &self.cfg.chunking);
        let mut corpus = Corpus::new();
        for c in &chunks {
            corpus.add(&c.text);
        }
        let prepared =
            vec![self.prepare(source_id.to_string(), hash, false, lang, chunks, &corpus)];
        self.apply(prepared, Vec::new(), &mut report)?;
        Ok(report)
    }

    fn prepare(
        &self,
        path: String,
        hash: String,
        dir: bool,
        lang: Lang,
        chunks: Vec<RawChunk>,
        corpus: &Corpus,
    ) -> Prepared {
        let chunks = chunks
            .into_iter()
            .map(|c| {
                let mem = compress(&c.text, lang, self.cfg.compression_ratio, corpus);
                let l1 = compress(&c.text, lang, self.l1_ratio(), corpus);
                let mut vector = self.embedder.embed(&mem);
                vector.resize(self.embedder.dim(), 0.0);
                PreparedChunk {
                    gist: gist(&c.text, lang),
                    l1,
                    start: c.start_line,
                    end: c.end_line,
                    hash: sha_hex(c.text.as_bytes()),
                    mentions: if lang == Lang::Rust {
                        rust_mentions(&c.text)
                    } else {
                        Vec::new()
                    },
                    text: c.text,
                    mem,
                    vector,
                }
            })
            .collect();
        Prepared {
            path,
            hash,
            dir,
            lang,
            chunks,
        }
    }

    fn apply(
        &self,
        prepared: Vec<Prepared>,
        removals: Vec<String>,
        report: &mut IndexReport,
    ) -> Result<(), RagError> {
        if prepared.is_empty() && removals.is_empty() {
            return Ok(());
        }
        let mut applied: Vec<Applied> = Vec::new();
        let mut removed: Vec<(String, Vec<u64>)> = Vec::new();
        {
            let state = self.read_state();
            let mut tx = self.graph.begin_write()?;
            for path in &removals {
                if let Some(rec) = state.files.get(path) {
                    for id in &rec.chunks {
                        tx.detach_delete_node(NodeId(*id))?;
                    }
                    tx.detach_delete_node(rec.node)?;
                    removed.push((path.clone(), rec.chunks.clone()));
                }
            }
            for p in prepared {
                let old = state.files.get(&p.path);
                let old_chunks = old.map(|r| r.chunks.clone()).unwrap_or_default();
                for id in &old_chunks {
                    tx.detach_delete_node(NodeId(*id))?;
                }
                let kind = if p.dir { "dir" } else { "text" };
                let node = match old {
                    Some(rec) => {
                        let mut props = Props::new();
                        props.insert("hash".into(), Value::from(p.hash.as_str()));
                        props.insert("kind".into(), Value::from(kind));
                        tx.set_node_props(rec.node, props)?;
                        rec.node
                    }
                    None => {
                        let mut props = Props::new();
                        props.insert("path".into(), Value::from(p.path.as_str()));
                        props.insert("hash".into(), Value::from(p.hash.as_str()));
                        props.insert("kind".into(), Value::from(kind));
                        tx.create_node(&["File"], props)?
                    }
                };
                let mut new_chunks: Vec<(u64, PreparedChunk)> = Vec::new();
                let mut prev: Option<NodeId> = None;
                for c in p.chunks {
                    let mut props = Props::new();
                    props.insert("file".into(), Value::from(p.path.as_str()));
                    props.insert("start".into(), Value::Int(i64::from(c.start)));
                    props.insert("end".into(), Value::Int(i64::from(c.end)));
                    props.insert("mem".into(), Value::from(c.mem.as_str()));
                    props.insert("g3".into(), Value::from(c.gist.as_str()));
                    props.insert("l1".into(), Value::from(c.l1.as_str()));
                    props.insert("text".into(), Value::from(c.text.as_str()));
                    props.insert("hash".into(), Value::from(c.hash.as_str()));
                    props.insert("utility".into(), Value::Float(0.0));
                    props.insert("vec".into(), Value::from(c.vector.clone()));
                    let id = tx.create_node(&["Chunk"], props)?;
                    tx.create_rel(id, node, "IN_FILE", Props::new())?;
                    if let Some(prev) = prev {
                        tx.create_rel(prev, id, "NEXT", Props::new())?;
                    }
                    prev = Some(id);
                    new_chunks.push((id.0, c));
                }
                applied.push(Applied {
                    path: p.path,
                    hash: p.hash,
                    dir: p.dir,
                    lang: p.lang,
                    node,
                    old_chunks,
                    new_chunks,
                });
            }
            tx.commit()?;
        }

        let mut state = self.write_state();
        for (path, chunks) in removed {
            for id in &chunks {
                state.index.remove(*id);
            }
            state.files.remove(&path);
            report.files_removed += 1;
            report.chunks_removed += chunks.len();
        }
        for a in applied {
            for id in &a.old_chunks {
                state.index.remove(*id);
            }
            report.chunks_removed += a.old_chunks.len();
            report.files_indexed += 1;
            report.chunks_added += a.new_chunks.len();
            let ids: Vec<u64> = a.new_chunks.iter().map(|(id, _)| *id).collect();
            if !state.spilled {
                for (id, c) in a.new_chunks {
                    state.index.insert(
                        Entry {
                            id,
                            vector: QVec::from_f32(&c.vector),
                            utility: 0.0,
                            lang: a.lang,
                            text_bytes: u32::try_from(c.text.len()).unwrap_or(u32::MAX),
                            mem_len: u32::try_from(c.mem.len()).unwrap_or(u32::MAX),
                        },
                        &c.mem,
                        c.mentions,
                    );
                }
            }
            state.files.insert(
                a.path,
                FileRec {
                    node: a.node,
                    hash: a.hash,
                    dir: a.dir,
                    chunks: ids,
                },
            );
        }
        if !state.spilled && state.over(self.index_cap) {
            state.spill();
        }
        Ok(())
    }

    fn hit(read: &ReadTx, id: u64, score: f32, via: HitVia) -> Result<Option<Hit>, RagError> {
        let Some(node) = read
            .get_node(NodeId(id))?
            .filter(|n| n.labels.contains("Chunk"))
        else {
            return Ok(None);
        };
        Ok(Some(Hit {
            chunk_id: id,
            file: prop_str(&node, "file")?.to_string(),
            start_line: prop_line(&node, "start")?,
            end_line: prop_line(&node, "end")?,
            memory: prop_str(&node, "mem")?.to_string(),
            score,
            via,
        }))
    }

    fn adjacent(read: &ReadTx, id: u64) -> Result<Vec<u64>, RagError> {
        let mut out = Vec::with_capacity(2);
        if let Some(e) = read.in_edges(NodeId(id), Some("NEXT"))?.next().transpose()? {
            out.push(e.other.0);
        }
        if let Some(e) = read.out_edges(NodeId(id), Some("NEXT"))?.next().transpose()? {
            out.push(e.other.0);
        }
        Ok(out)
    }

    fn expand_ids(
        state: &State,
        read: &ReadTx,
        direct: &[(u64, f32)],
        hops: usize,
    ) -> Result<Vec<(u64, f32)>, RagError> {
        let mut seen: HashSet<u64> = direct.iter().map(|(id, _)| *id).collect();
        let mut frontier: Vec<(u64, f32)> = direct.to_vec();
        let mut out = Vec::new();
        for _ in 0..hops {
            let mut next = Vec::new();
            for &(id, score) in &frontier {
                let decayed = score * HOP_DECAY;
                let mut cands: Vec<u64> = Self::adjacent(read, id)?;
                if !state.spilled {
                    cands.retain(|c| state.index.contains(*c));
                    if state.index.get(id).is_some_and(|e| e.lang == Lang::Rust) {
                        if let Some(node) = read.get_node(NodeId(id))? {
                            let defs = rust_defs(prop_str(&node, "text")?);
                            cands.extend(state.index.symbol_neighbours(id, &defs));
                        }
                    }
                }
                for c in cands {
                    if seen.insert(c) {
                        next.push((c, decayed));
                    }
                }
            }
            out.extend(next.iter().copied());
            frontier = next;
        }
        Ok(out)
    }

    pub fn search(&self, query: &str, opts: &SearchOptions) -> Result<Vec<Hit>, RagError> {
        if opts.expand_hops > MAX_HOPS {
            return Err(RagError::InvalidOptions(format!(
                "expand_hops must be in 0..={MAX_HOPS}, got {}",
                opts.expand_hops
            )));
        }
        let qvec = self.embedder.embed(query);
        let state = self.read_state();
        let read = self.graph.begin_read()?;
        let direct = if state.spilled {
            scan::direct(
                &read,
                self.embedder.as_ref(),
                query,
                &qvec,
                opts.k,
                opts.use_utility,
            )?
        } else {
            state.index.direct(query, &qvec, opts.k, opts.use_utility)
        };
        let mut hits: Vec<Hit> = Vec::with_capacity(direct.len());
        for (id, score) in &direct {
            hits.extend(Self::hit(&read, *id, *score, HitVia::Direct)?);
        }
        if opts.expand_hops > 0 {
            let mut extra = Self::expand_ids(&state, &read, &direct, opts.expand_hops)?;
            extra.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            for (id, score) in &extra {
                hits.extend(Self::hit(&read, *id, *score, HitVia::GraphNeighbour)?);
            }
        }
        Ok(hits)
    }

    pub fn pack(&self, query: &str, opts: &SearchOptions) -> Result<ContextPack, RagError> {
        let hits = self.search(query, opts)?;
        let total = hits.len();
        let mut used = 0usize;
        let mut text = String::new();
        let mut kept = Vec::new();
        for hit in hits {
            let piece = format!(
                "// {}:{}-{}\n{}\n",
                hit.file, hit.start_line, hit.end_line, hit.memory
            );
            let cost = token_estimate(&piece);
            if used + cost <= opts.budget_tokens {
                used += cost;
                text.push_str(&piece);
                kept.push(hit);
            }
        }
        Ok(ContextPack {
            tokens: token_estimate(&text),
            omitted: total - kept.len(),
            hits: kept,
            text,
            budget_tokens: opts.budget_tokens,
        })
    }

    pub fn expand(&self, chunk_id: u64) -> Result<String, RagError> {
        self.expand_level(chunk_id, Level::L0)
    }

    fn chunk_node(&self, chunk_id: u64) -> Result<Node, RagError> {
        self.graph
            .begin_read()?
            .get_node(NodeId(chunk_id))?
            .filter(|n| n.labels.contains("Chunk"))
            .ok_or(RagError::NotFound(chunk_id))
    }

    pub fn expand_level(&self, chunk_id: u64, level: Level) -> Result<String, RagError> {
        let node = self.chunk_node(chunk_id)?;
        match level {
            Level::L0 => Ok(prop_str(&node, "text")?.to_string()),
            Level::L2 => Ok(prop_str(&node, "mem")?.to_string()),
            Level::L3 => self.derived_prop(&node, "g3", gist),
            Level::L1 => {
                let ratio = self.l1_ratio();
                self.derived_prop(&node, "l1", |text, lang| {
                    compress(text, lang, ratio, &Corpus::new())
                })
            }
        }
    }

    fn l1_ratio(&self) -> usize {
        (self.cfg.compression_ratio / 4).clamp(1, L1_RATIO)
    }

    fn derived_prop(
        &self,
        node: &Node,
        key: &str,
        derive: impl FnOnce(&str, Lang) -> String,
    ) -> Result<String, RagError> {
        if let Some(Value::String(s)) = node.props.get(key) {
            return Ok(s.clone());
        }
        let text = prop_str(node, "text")?;
        let lang = Lang::from_path(prop_str(node, "file")?);
        let derived = derive(text, lang);
        if self.graph.is_read_only() {
            return Ok(derived);
        }
        let _gate = lock(&self.gate);
        if let Ok(mut tx) = self.graph.begin_write() {
            let mut props = Props::new();
            props.insert(key.into(), Value::from(derived.as_str()));
            if tx.set_node_props(node.id, props).is_ok() {
                let _ = tx.commit();
            }
        }
        Ok(derived)
    }

    pub fn chunk_fingerprint(&self, chunk_id: u64) -> Result<Option<String>, RagError> {
        match self.chunk_node(chunk_id) {
            Ok(node) => Ok(Some(sha_hex(prop_str(&node, "text")?.as_bytes()))),
            Err(RagError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn gist_of(read: &ReadTx, id: u64) -> Result<String, RagError> {
        let Some(node) = read.get_node(NodeId(id))? else {
            return Ok(String::new());
        };
        if let Some(Value::String(g)) = node.props.get("g3") {
            return Ok(g.clone());
        }
        let lang = Lang::from_path(prop_str(&node, "file")?);
        Ok(gist(prop_str(&node, "text")?, lang))
    }

    pub fn pack_ladder(&self, query: &str, opts: &SearchOptions) -> Result<LadderPack, RagError> {
        let hits = self.search(query, opts)?;
        self.ladder_pack(hits, opts.budget_tokens)
    }

    pub fn ladder_pack(
        &self,
        hits: Vec<Hit>,
        budget_tokens: usize,
    ) -> Result<LadderPack, RagError> {
        let total = hits.len();
        let gists: Vec<String> = {
            let read = self.graph.begin_read()?;
            hits.iter()
                .map(|h| Self::gist_of(&read, h.chunk_id))
                .collect::<Result<_, _>>()?
        };
        let rungs: Vec<Rungs> = hits
            .iter()
            .zip(&gists)
            .enumerate()
            .map(|(rank, (hit, g))| {
                let head = format!(
                    "// {}:{}-{} #{}",
                    hit.file, hit.start_line, hit.end_line, hit.chunk_id
                );
                let l1 = if default_level(rank) == Level::L1 {
                    self.expand_level(hit.chunk_id, Level::L1).ok()
                } else {
                    None
                };
                Rungs {
                    l1: l1.map(|t| Piece::new(&head, &t, false)),
                    l2: Piece::new(&head, &hit.memory, false),
                    l3: Piece::new(&head, g, true),
                }
            })
            .collect();
        let mut levels: Vec<Level> = rungs
            .iter()
            .enumerate()
            .map(|(rank, r)| match default_level(rank) {
                Level::L1 if r.l1.is_none() => Level::L2,
                other => other,
            })
            .collect();
        let mut kept = total;
        loop {
            let cost: usize = (0..kept).map(|i| rungs[i].at(levels[i]).tokens).sum();
            if cost <= budget_tokens || kept == 0 {
                break;
            }
            match (0..kept)
                .rev()
                .find_map(|i| rungs[i].cheaper_than(levels[i]).map(|l| (i, l)))
            {
                Some((i, l)) => levels[i] = l,
                None => kept -= 1,
            }
        }
        let mut text = String::new();
        let mut out = Vec::with_capacity(kept);
        for (i, hit) in hits.into_iter().take(kept).enumerate() {
            let level = levels[i];
            let piece = rungs[i].at(level);
            text.push_str(&piece.rendered);
            out.push(LadderHit {
                chunk_id: hit.chunk_id,
                file: hit.file,
                start_line: hit.start_line,
                end_line: hit.end_line,
                score: hit.score,
                via: hit.via,
                level,
                text: piece.body.clone(),
            });
        }
        Ok(LadderPack {
            tokens: token_estimate(&text),
            omitted: total - out.len(),
            hits: out,
            text,
            budget_tokens,
        })
    }

    pub fn feedback(&self, used: &[u64], unused: &[u64]) -> Result<(), RagError> {
        let _gate = lock(&self.gate);
        let mut updated: BTreeMap<u64, f32> = BTreeMap::new();
        {
            let state = self.read_state();
            let read = self.graph.begin_read()?;
            for (ids, target) in [(used, 1.0f32), (unused, -1.0f32)] {
                for id in ids {
                    let current = match updated.get(id) {
                        Some(u) => *u,
                        None if state.spilled => match read.get_node(NodeId(*id))? {
                            Some(n) if n.labels.contains("Chunk") => scan::utility_of(&n),
                            _ => continue,
                        },
                        None => match state.index.get(*id) {
                            Some(e) => e.utility,
                            None => continue,
                        },
                    };
                    let next =
                        ((1.0 - UTILITY_ALPHA) * current + UTILITY_ALPHA * target).clamp(-1.0, 1.0);
                    updated.insert(*id, next);
                }
            }
        }
        if updated.is_empty() {
            return Ok(());
        }
        let mut tx = self.graph.begin_write()?;
        for (id, u) in &updated {
            let mut props = Props::new();
            props.insert("utility".into(), Value::Float(f64::from(*u)));
            tx.set_node_props(NodeId(*id), props)?;
        }
        tx.commit()?;
        let mut state = self.write_state();
        for (id, u) in updated {
            if let Some(e) = state.index.get_mut(id) {
                e.utility = u;
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> Result<RagStats, RagError> {
        let state = self.read_state();
        let (chunks, text, mem, index_bytes, vector_bytes) = if state.spilled {
            let (chunks, text, mem) = scan::totals(&self.graph.begin_read()?)?;
            (chunks, text, mem, 0, 0)
        } else {
            let (text, mem) = state.index.byte_totals();
            (
                state.index.len(),
                text,
                mem,
                state.index.resident_bytes(),
                state.index.vector_bytes(),
            )
        };
        Ok(RagStats {
            chunks,
            files: state.files.len(),
            avg_compression_ratio: if mem == 0 {
                0.0
            } else {
                text as f64 / mem as f64
            },
            text_bytes: text,
            memory_bytes: mem,
            index_bytes,
            vector_bytes,
            spilled: state.spilled,
        })
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    skip: &[&str],
    out: &mut Vec<(String, PathBuf)>,
) -> Result<(), RagError> {
    let mut entries: Vec<fs::DirEntry> = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .collect::<Result<_, _>>()
        .map_err(io_err(dir))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for e in entries {
        let Ok(ft) = e.file_type() else {
            continue;
        };
        let path = e.path();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let name = e.file_name();
            if skip.iter().any(|s| name.to_string_lossy() == *s) {
                continue;
            }
            walk(root, &path, skip, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_ids_match_database_after_edit() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("a.txt"), "alpha beta gamma").expect("write");
        let graph = Graph::open_in_memory(uriel_core::MemoryBudget::mib(80)).expect("graph");
        let rag = Rag::open(graph.clone(), RagConfig::default()).expect("rag");
        rag.index_dir(dir.path(), &IndexOptions::default())
            .expect("index");
        fs::write(dir.path().join("a.txt"), "delta epsilon zeta").expect("write");
        rag.index_dir(dir.path(), &IndexOptions::default())
            .expect("index");
        let state = rag.read_state();
        let ids = state.index.ids();
        let read = graph.begin_read().expect("read");
        let db: Vec<u64> = read
            .nodes_by_label("Chunk")
            .expect("chunks")
            .map(|n| n.expect("node").id.0)
            .collect();
        assert_eq!(ids, db);
    }

    fn f32_top(vectors: &[(u64, Vec<f32>)], q: &[f32], n: usize) -> Vec<u64> {
        let mut scored: Vec<(u64, f32)> = vectors
            .iter()
            .map(|(id, v)| (*id, v.iter().zip(q).map(|(a, b)| a * b).sum::<f32>()))
            .filter(|(_, s)| *s > 0.0)
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.truncate(n);
        scored.into_iter().map(|(id, _)| id).collect()
    }

    #[test]
    fn int8_vector_leg_keeps_the_f32_top_10_on_the_frozen_queries() {
        let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let corpus = eval::materialize(&tests.join("frozen_corpus")).expect("materialize");
        let graph = Graph::open_in_memory(uriel_core::MemoryBudget::mib(512)).expect("graph");
        let rag = Rag::open(graph.clone(), RagConfig::default()).expect("rag");
        rag.index_dir(corpus.root(), &IndexOptions::default())
            .expect("index");
        let set = eval::parse_queries(
            &fs::read_to_string(tests.join("eval_queries.json")).expect("queries"),
        )
        .expect("parse");
        let vectors: Vec<(u64, Vec<f32>)> = graph
            .begin_read()
            .expect("read")
            .nodes_by_label("Chunk")
            .expect("chunks")
            .map(|n| {
                let n = n.expect("node");
                match n.props.get("vec") {
                    Some(Value::Vector(v)) => (n.id.0, v.clone()),
                    _ => panic!("chunk {} lacks a vector", n.id),
                }
            })
            .collect();
        let state = rag.read_state();
        assert!(!state.spilled);
        let (mut same_order, mut same_set, mut top1) = (0usize, 0usize, 0usize);
        for q in &set.queries {
            let qv = rag.embedder.embed(&q.query);
            let exact = f32_top(&vectors, &qv, 10);
            let quant = state.index.vector_ids(&qv, 10);
            same_order += usize::from(exact == quant);
            let (mut a, mut b) = (exact.clone(), quant.clone());
            a.sort_unstable();
            b.sort_unstable();
            same_set += usize::from(a == b);
            top1 += usize::from(exact.first() == quant.first());
        }
        println!(
            "INT8 vs F32 vector leg over {} queries on {} chunks: identical top-10 order {same_order}, identical top-10 set {same_set}, identical top-1 {top1}",
            set.queries.len(),
            vectors.len()
        );
        assert_eq!(set.queries.len(), 100);
        assert!(top1 >= 99, "top-1 agreement {top1}/100");
        assert!(same_set >= 90, "top-10 set agreement {same_set}/100");
    }

    #[test]
    fn sha_hex_is_64_hex_chars() {
        let h = sha_hex(b"abc");
        assert_eq!(h.len(), 64);
        assert!(h.starts_with("ba7816bf"));
    }
}
