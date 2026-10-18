use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::thread::{self, JoinHandle};

use uriel_core::{MemoryBudget, Value};
use uriel_query::{AccessMode, Engine, QueryResult};
use uriel_rag::{ContextPack, IndexOptions, IndexReport, Rag, RagConfig, SearchOptions};
use uriel_sys::{Arena, Slab, Width};

use crate::codec::DEFAULT_MAX_FRAME;
use crate::error::{ErrorCode, HostError};
use crate::message::{Request, Response};
use crate::server::{accept_loop, spawn_connection, Dispatcher};
use crate::stats::{BufferGauge, HostStats};
use crate::transport::Transport;

const MAX_INDEX_PATH_BYTES: usize = 4096;
const BINARY_SNIFF_BYTES: usize = 8000;

pub type HandlerHook = Arc<dyn Fn(&Request) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct HostConfig {
    pub db_path: PathBuf,
    pub root: PathBuf,
    pub hot_cap_bytes: usize,
    pub memory: MemoryBudget,
    pub sock_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServeConfig {
    pub queue_depth: usize,
    pub workers: usize,
    pub max_frame: usize,
}

impl Default for ServeConfig {
    fn default() -> ServeConfig {
        ServeConfig {
            queue_depth: 64,
            workers: 4,
            max_frame: DEFAULT_MAX_FRAME,
        }
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
pub(crate) struct Gauge {
    inbound: AtomicUsize,
    outbound: AtomicUsize,
}

impl Gauge {
    pub(crate) fn update(&self, inbound: usize, outbound: usize) {
        self.inbound.store(inbound, Ordering::Relaxed);
        self.outbound.store(outbound, Ordering::Relaxed);
    }

    fn read(&self) -> BufferGauge {
        BufferGauge {
            inbound_capacity: self.inbound.load(Ordering::Relaxed),
            outbound_capacity: self.outbound.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
struct Counters {
    served: AtomicU64,
    failed: AtomicU64,
    busy: AtomicU64,
    connections: AtomicU64,
}

pub(crate) struct HostInner {
    engine: Engine,
    rag: Rag,
    root: PathBuf,
    max_file_bytes: u64,
    indexed: Mutex<bool>,
    index_passes: AtomicU64,
    arena: Mutex<Arena>,
    port_file: PathBuf,
    counters: Counters,
    stopping: AtomicBool,
    listening: AtomicBool,
    #[cfg(windows)]
    pipe_listening: AtomicBool,
    #[cfg(windows)]
    pipe_path: String,
    dispatcher: Mutex<Option<Arc<Dispatcher>>>,
    hook: RwLock<Option<HandlerHook>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    gauges: Mutex<BTreeMap<u64, Arc<Gauge>>>,
    next_connection: AtomicU64,
    shutdown_gate: Mutex<()>,
}

fn validate_config(cfg: &HostConfig) -> Result<(), HostError> {
    if cfg.hot_cap_bytes == 0 {
        return Err(HostError::InvalidConfig(
            "hot_cap_bytes must be set to a non-zero size".into(),
        ));
    }
    let name = &cfg.sock_name;
    let bad_char = |c: char| matches!(c, '/' | '\\' | ':' | '\0') || c.is_control();
    if name.is_empty() || name == "." || name == ".." || name.chars().any(bad_char) {
        return Err(HostError::InvalidConfig(format!(
            "sock_name {name:?} is not a plain name"
        )));
    }
    if !cfg.root.is_dir() {
        return Err(HostError::InvalidConfig(format!(
            "root {} is not a directory",
            cfg.root.display()
        )));
    }
    Ok(())
}

fn port_file_for(cfg: &HostConfig) -> PathBuf {
    let parent = cfg
        .db_path
        .parent()
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("."));
    parent.join(format!("{}.port", cfg.sock_name))
}

impl HostInner {
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    pub(crate) fn hook(&self) -> Option<HandlerHook> {
        self.hook
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn record(&self, response: &Response) {
        let counter = match response {
            Response::Busy => &self.counters.busy,
            Response::Error { .. } => &self.counters.failed,
            _ => &self.counters.served,
        };
        counter.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn connection_closed(&self, id: u64) {
        lock(&self.gauges).remove(&id);
        self.counters.connections.fetch_sub(1, Ordering::SeqCst);
    }

    pub(crate) fn execute(&self, request: &Request) -> Response {
        match request {
            Request::Query {
                cypher,
                params,
                mode,
            } => match self.engine.execute(cypher, params, *mode) {
                Ok(result) => Response::Query(result),
                Err(e) => Response::error(ErrorCode::Query, e.to_string()),
            },
            Request::Search { query, opts } => match self.search(query, opts) {
                Ok(pack) => Response::Search(pack),
                Err(e) => Response::error(ErrorCode::Search, e.to_string()),
            },
            Request::Stats => match self.stats() {
                Ok(stats) => Response::Stats(stats),
                Err(e) => Response::error(ErrorCode::Internal, e.to_string()),
            },
            Request::Ping { nonce } => Response::Pong { nonce: *nonce },
            Request::Index { root } => match self.index_root(root) {
                Ok(report) => Response::Indexed(report),
                Err(e) => Response::error(ErrorCode::Index, e.to_string()),
            },
            Request::IndexFile { path, content } => match self.index_file(path, content) {
                Ok(report) => Response::Indexed(report),
                Err(e) => Response::error(ErrorCode::Index, e.to_string()),
            },
        }
    }

    fn index_root(&self, root: &str) -> Result<IndexReport, HostError> {
        if !root.is_empty() {
            let asked = std::fs::canonicalize(root)
                .map_err(|e| HostError::InvalidIndex(format!("root {root:?}: {e}")))?;
            let own = std::fs::canonicalize(&self.root)?;
            if asked != own {
                return Err(HostError::InvalidIndex(format!(
                    "root {root:?} is not the host root"
                )));
            }
        }
        match self.index_pass(false)? {
            Some(report) => Ok(report),
            None => Err(HostError::Unexpected("index pass skipped")),
        }
    }

    fn index_file(&self, path: &str, content: &str) -> Result<IndexReport, HostError> {
        if path.is_empty()
            || path.len() > MAX_INDEX_PATH_BYTES
            || path.chars().any(char::is_control)
        {
            return Err(HostError::InvalidIndex(
                "path must be 1..=4096 bytes without control characters".into(),
            ));
        }
        let binary = content.bytes().take(BINARY_SNIFF_BYTES).any(|b| b == 0);
        if binary || content.len() as u64 > self.max_file_bytes {
            return Ok(IndexReport {
                files_scanned: 1,
                files_skipped: 1,
                ..IndexReport::default()
            });
        }
        Ok(self.rag.index_text(path, content)?)
    }

    fn search(&self, query: &str, opts: &SearchOptions) -> Result<ContextPack, HostError> {
        self.index_pass(true)?;
        Ok(self.rag.pack(query, opts)?)
    }

    fn index_pass(&self, only_if_pending: bool) -> Result<Option<IndexReport>, HostError> {
        let mut indexed = lock(&self.indexed);
        if only_if_pending && *indexed {
            return Ok(None);
        }
        let report = self.rag.index_dir(&self.root, &IndexOptions::default())?;
        self.index_passes.fetch_add(1, Ordering::SeqCst);
        *indexed = true;
        Ok(Some(report))
    }

    fn stats(&self) -> Result<HostStats, HostError> {
        let (arena_cap, arena_used) = {
            let arena = lock(&self.arena);
            (arena.cap() as u64, arena.used() as u64)
        };
        let rag = self.rag.stats()?;
        let (queue_len, in_flight) = match lock(&self.dispatcher).as_ref() {
            Some(d) => (d.queue_len() as u64, d.in_flight()),
            None => (0, 0),
        };
        Ok(HostStats {
            arena_cap,
            arena_used,
            epoch: self.engine.graph().epoch(),
            requests_served: self.counters.served.load(Ordering::SeqCst),
            requests_failed: self.counters.failed.load(Ordering::SeqCst),
            requests_busy: self.counters.busy.load(Ordering::SeqCst),
            in_flight,
            queue_len,
            connections: self.counters.connections.load(Ordering::SeqCst),
            chunks: rag.chunks as u64,
            files: rag.files as u64,
        })
    }

    fn dispatcher(self: &Arc<Self>) -> Result<Arc<Dispatcher>, HostError> {
        let mut slot = lock(&self.dispatcher);
        if self.is_stopping() {
            return Err(HostError::ShuttingDown);
        }
        if let Some(existing) = slot.as_ref() {
            return Ok(existing.clone());
        }
        let started = Dispatcher::start(self, &ServeConfig::default())?;
        *slot = Some(started.clone());
        Ok(started)
    }

    fn serve(self: &Arc<Self>, cfg: &ServeConfig) -> Result<(), HostError> {
        let mut slot = lock(&self.dispatcher);
        if self.is_stopping() {
            return Err(HostError::ShuttingDown);
        }
        if slot.is_some() {
            return Err(HostError::AlreadyServing);
        }
        *slot = Some(Dispatcher::start(self, cfg)?);
        Ok(())
    }

    pub(crate) fn attach(self: &Arc<Self>, transport: Box<dyn Transport>) -> Result<(), HostError> {
        let dispatcher = self.dispatcher()?;
        let mut threads = lock(&self.threads);
        if self.is_stopping() {
            return Err(HostError::ShuttingDown);
        }
        threads.retain(|handle| !handle.is_finished());
        let id = self.next_connection.fetch_add(1, Ordering::SeqCst);
        let gauge = Arc::new(Gauge::default());
        lock(&self.gauges).insert(id, gauge.clone());
        self.counters.connections.fetch_add(1, Ordering::SeqCst);
        match spawn_connection(self, dispatcher, transport, id, gauge) {
            Ok(handle) => {
                threads.push(handle);
                Ok(())
            }
            Err(e) => {
                self.connection_closed(id);
                Err(e)
            }
        }
    }

    fn listen_tcp(self: &Arc<Self>) -> Result<SocketAddr, HostError> {
        let dispatcher = self.dispatcher()?;
        if self.listening.swap(true, Ordering::SeqCst) {
            return Err(HostError::AlreadyServing);
        }
        let bound = (|| -> Result<(TcpListener, SocketAddr), HostError> {
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            listener.set_nonblocking(true)?;
            let addr = listener.local_addr()?;
            std::fs::write(&self.port_file, addr.port().to_string())?;
            Ok((listener, addr))
        })();
        let (listener, addr) = match bound {
            Ok(pair) => pair,
            Err(e) => {
                self.listening.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };
        let accept_inner = self.clone();
        let spawned = thread::Builder::new()
            .name("uriel-host-accept".into())
            .spawn(move || accept_loop(accept_inner, dispatcher, listener));
        match spawned {
            Ok(handle) => {
                lock(&self.threads).push(handle);
                Ok(addr)
            }
            Err(e) => {
                self.listening.store(false, Ordering::SeqCst);
                let _ = std::fs::remove_file(&self.port_file);
                Err(HostError::Io(e))
            }
        }
    }

    #[cfg(windows)]
    fn listen_pipe(self: &Arc<Self>) -> Result<String, HostError> {
        let dispatcher = self.dispatcher()?;
        if self.pipe_listening.swap(true, Ordering::SeqCst) {
            return Err(HostError::AlreadyServing);
        }
        match crate::pipe::start(self, self.pipe_path.clone(), dispatcher.max_frame()) {
            Ok(handle) => {
                lock(&self.threads).push(handle);
                Ok(self.pipe_path.clone())
            }
            Err(e) => {
                self.pipe_listening.store(false, Ordering::SeqCst);
                Err(e)
            }
        }
    }

    fn shutdown(&self) {
        let _gate = lock(&self.shutdown_gate);
        self.stopping.store(true, Ordering::SeqCst);
        let dispatcher = lock(&self.dispatcher).take();
        if let Some(dispatcher) = dispatcher {
            dispatcher.close_and_join();
        }
        let threads = std::mem::take(&mut *lock(&self.threads));
        for handle in threads {
            let _ = handle.join();
        }
        if self.listening.swap(false, Ordering::SeqCst) {
            let _ = std::fs::remove_file(&self.port_file);
        }
    }
}

pub struct Host {
    inner: Arc<HostInner>,
}

impl Host {
    pub fn open(cfg: HostConfig) -> Result<Host, HostError> {
        validate_config(&cfg)?;
        let arena = Arena::with_cap(cfg.hot_cap_bytes)?;
        let engine = Engine::open(&cfg.db_path, cfg.memory)?;
        let rag_config = RagConfig::default();
        let max_file_bytes = rag_config.max_file_bytes;
        let rag = Rag::open(engine.graph().clone(), rag_config)?;
        let port_file = port_file_for(&cfg);
        Ok(Host {
            inner: Arc::new(HostInner {
                engine,
                rag,
                root: cfg.root.clone(),
                max_file_bytes,
                indexed: Mutex::new(false),
                index_passes: AtomicU64::new(0),
                arena: Mutex::new(arena),
                port_file,
                counters: Counters::default(),
                stopping: AtomicBool::new(false),
                listening: AtomicBool::new(false),
                #[cfg(windows)]
                pipe_listening: AtomicBool::new(false),
                #[cfg(windows)]
                pipe_path: crate::pipe::pipe_path(&cfg.sock_name),
                dispatcher: Mutex::new(None),
                hook: RwLock::new(None),
                threads: Mutex::new(Vec::new()),
                gauges: Mutex::new(BTreeMap::new()),
                next_connection: AtomicU64::new(1),
                shutdown_gate: Mutex::new(()),
            }),
        })
    }

    pub fn query(
        &self,
        cypher: &str,
        params: &BTreeMap<String, Value>,
        mode: AccessMode,
    ) -> Result<QueryResult, HostError> {
        Ok(self.inner.engine.execute(cypher, params, mode)?)
    }

    pub fn search(&self, query: &str, opts: &SearchOptions) -> Result<ContextPack, HostError> {
        self.inner.search(query, opts)
    }

    pub fn stats(&self) -> Result<HostStats, HostError> {
        self.inner.stats()
    }

    pub fn index(&self) -> Result<IndexReport, HostError> {
        match self.inner.index_pass(false)? {
            Some(report) => Ok(report),
            None => Err(HostError::Unexpected("index pass skipped")),
        }
    }

    pub fn index_file(&self, path: &str, content: &str) -> Result<IndexReport, HostError> {
        self.inner.index_file(path, content)
    }

    pub fn index_passes(&self) -> u64 {
        self.inner.index_passes.load(Ordering::SeqCst)
    }

    pub fn shutdown(&self) {
        self.inner.shutdown();
    }

    pub fn arena_used(&self) -> usize {
        lock(&self.inner.arena).used()
    }

    pub fn arena_cap(&self) -> usize {
        lock(&self.inner.arena).cap()
    }

    pub fn reserve_hot(&self, bytes: usize) -> Result<Slab, HostError> {
        Ok(lock(&self.inner.arena).alloc(bytes, Width::W8)?)
    }

    pub fn serve(&self, cfg: ServeConfig) -> Result<(), HostError> {
        self.inner.serve(&cfg)
    }

    pub fn attach(&self, transport: impl Transport + 'static) -> Result<(), HostError> {
        self.inner.attach(Box::new(transport))
    }

    pub fn listen_tcp(&self) -> Result<SocketAddr, HostError> {
        self.inner.listen_tcp()
    }

    #[cfg(windows)]
    pub fn listen_pipe(&self) -> Result<String, HostError> {
        self.inner.listen_pipe()
    }

    #[cfg(windows)]
    pub fn pipe_path(&self) -> String {
        self.inner.pipe_path.clone()
    }

    pub fn port_file(&self) -> PathBuf {
        self.inner.port_file.clone()
    }

    pub fn set_handler_hook(&self, hook: impl Fn(&Request) + Send + Sync + 'static) {
        *self
            .inner
            .hook
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(hook));
    }

    pub fn clear_handler_hook(&self) {
        *self
            .inner
            .hook
            .write()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub fn connection_buffers(&self) -> Vec<BufferGauge> {
        lock(&self.inner.gauges)
            .values()
            .map(|gauge| gauge.read())
            .collect()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.inner.shutdown();
    }
}
