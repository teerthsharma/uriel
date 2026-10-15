use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::codec::MIN_MAX_FRAME;
use crate::error::{CodecError, ErrorCode, HostError};
use crate::host::{lock, Gauge, HostInner, ServeConfig};
use crate::message::{Request, Response};
use crate::transport::{Recv, TcpTransport, Transport};

const POLL: Duration = Duration::from_millis(20);
const ACCEPT_POLL: Duration = Duration::from_millis(5);

pub(crate) struct Slot {
    value: Mutex<Option<Response>>,
    ready: Condvar,
}

impl Slot {
    pub(crate) fn new() -> Arc<Slot> {
        Arc::new(Slot {
            value: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    fn fill(&self, response: Response) {
        *lock(&self.value) = Some(response);
        self.ready.notify_all();
    }

    fn wait(&self) -> Response {
        let mut guard = lock(&self.value);
        loop {
            if let Some(response) = guard.take() {
                return response;
            }
            guard = self
                .ready
                .wait(guard)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

struct Job {
    request: Request,
    slot: Arc<Slot>,
}

struct QueueState {
    jobs: VecDeque<Job>,
    closed: bool,
}

pub(crate) enum SubmitError {
    Busy,
    Closed,
}

pub(crate) struct Dispatcher {
    queue: Mutex<QueueState>,
    ready: Condvar,
    depth: usize,
    max_frame: usize,
    in_flight: AtomicU64,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl Dispatcher {
    pub(crate) fn validate(cfg: &ServeConfig) -> Result<(), HostError> {
        if cfg.queue_depth == 0 {
            return Err(HostError::InvalidConfig("queue_depth must be at least 1".into()));
        }
        if cfg.workers == 0 {
            return Err(HostError::InvalidConfig("workers must be at least 1".into()));
        }
        if cfg.max_frame < MIN_MAX_FRAME {
            return Err(HostError::InvalidConfig(format!(
                "max_frame must be at least {MIN_MAX_FRAME}"
            )));
        }
        Ok(())
    }

    pub(crate) fn start(inner: &Arc<HostInner>, cfg: &ServeConfig) -> Result<Arc<Dispatcher>, HostError> {
        Dispatcher::validate(cfg)?;
        let dispatcher = Arc::new(Dispatcher {
            queue: Mutex::new(QueueState {
                jobs: VecDeque::with_capacity(cfg.queue_depth),
                closed: false,
            }),
            ready: Condvar::new(),
            depth: cfg.queue_depth,
            max_frame: cfg.max_frame,
            in_flight: AtomicU64::new(0),
            workers: Mutex::new(Vec::with_capacity(cfg.workers)),
        });
        for index in 0..cfg.workers {
            let worker_inner = inner.clone();
            let worker_dispatcher = dispatcher.clone();
            let spawned = thread::Builder::new()
                .name(format!("uriel-host-worker-{index}"))
                .spawn(move || worker_loop(worker_inner, worker_dispatcher));
            match spawned {
                Ok(handle) => lock(&dispatcher.workers).push(handle),
                Err(e) => {
                    dispatcher.close_and_join();
                    return Err(HostError::Io(e));
                }
            }
        }
        Ok(dispatcher)
    }

    pub(crate) fn max_frame(&self) -> usize {
        self.max_frame
    }

    pub(crate) fn queue_len(&self) -> usize {
        lock(&self.queue).jobs.len()
    }

    pub(crate) fn in_flight(&self) -> u64 {
        self.in_flight.load(Ordering::SeqCst)
    }

    fn submit(&self, request: Request, slot: Arc<Slot>) -> Result<(), SubmitError> {
        let mut queue = lock(&self.queue);
        if queue.closed {
            return Err(SubmitError::Closed);
        }
        if queue.jobs.len() >= self.depth {
            return Err(SubmitError::Busy);
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        queue.jobs.push_back(Job { request, slot });
        drop(queue);
        self.ready.notify_one();
        Ok(())
    }

    fn next_job(&self) -> Option<Job> {
        let mut queue = lock(&self.queue);
        loop {
            if let Some(job) = queue.jobs.pop_front() {
                return Some(job);
            }
            if queue.closed {
                return None;
            }
            queue = self
                .ready
                .wait(queue)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(crate) fn close_and_join(&self) {
        lock(&self.queue).closed = true;
        self.ready.notify_all();
        let workers = std::mem::take(&mut *lock(&self.workers));
        for handle in workers {
            let _ = handle.join();
        }
    }
}

fn worker_loop(inner: Arc<HostInner>, dispatcher: Arc<Dispatcher>) {
    while let Some(job) = dispatcher.next_job() {
        let hook = inner.hook();
        let response = catch_unwind(AssertUnwindSafe(|| {
            if let Some(hook) = &hook {
                hook(&job.request);
            }
            inner.execute(&job.request)
        }))
        .unwrap_or_else(|_| Response::error(ErrorCode::Internal, "request handler panicked"));
        inner.record(&response);
        job.slot.fill(response);
        dispatcher.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn spawn_connection(
    inner: &Arc<HostInner>,
    dispatcher: Arc<Dispatcher>,
    transport: Box<dyn Transport>,
    id: u64,
    gauge: Arc<Gauge>,
) -> Result<JoinHandle<()>, HostError> {
    let connection_inner = inner.clone();
    thread::Builder::new()
        .name(format!("uriel-host-conn-{id}"))
        .spawn(move || {
            run_connection(&connection_inner, &dispatcher, transport, &gauge);
            connection_inner.connection_closed(id);
        })
        .map_err(HostError::Io)
}

fn error_code_for(error: &CodecError) -> ErrorCode {
    match error {
        CodecError::FrameTooLarge { .. } => ErrorCode::TooLarge,
        _ => ErrorCode::Malformed,
    }
}

fn run_connection(
    inner: &Arc<HostInner>,
    dispatcher: &Arc<Dispatcher>,
    mut transport: Box<dyn Transport>,
    gauge: &Gauge,
) {
    let max_frame = dispatcher.max_frame();
    let slot = Slot::new();
    let mut inbound: Vec<u8> = Vec::new();
    let mut outbound: Vec<u8> = Vec::new();
    loop {
        if inner.is_stopping() {
            break;
        }
        match transport.recv(&mut inbound, POLL) {
            Ok(Recv::Timeout) => continue,
            Ok(Recv::Frame) => {}
            Err(HostError::Codec(e)) => {
                Response::error(error_code_for(&e), e.to_string())
                    .encode_or_error(&mut outbound, max_frame);
                let _ = transport.send(&outbound);
                break;
            }
            Err(_) => break,
        }
        let response = match Request::decode(&inbound, max_frame) {
            Ok(Request::Ping { nonce }) => Response::Pong { nonce },
            Ok(request) => match dispatcher.submit(request, slot.clone()) {
                Ok(()) => slot.wait(),
                Err(SubmitError::Busy) => {
                    inner.record(&Response::Busy);
                    Response::Busy
                }
                Err(SubmitError::Closed) => {
                    Response::error(ErrorCode::ShuttingDown, "host is shutting down")
                }
            },
            Err(e) => Response::error(error_code_for(&e), e.to_string()),
        };
        response.encode_or_error(&mut outbound, max_frame);
        gauge.update(inbound.capacity(), outbound.capacity());
        if outbound.is_empty() || transport.send(&outbound).is_err() {
            break;
        }
    }
}

pub(crate) fn accept_loop(inner: Arc<HostInner>, dispatcher: Arc<Dispatcher>, listener: TcpListener) {
    while !inner.is_stopping() {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Ok(transport) = TcpTransport::new(stream, dispatcher.max_frame()) {
                    let _ = inner.attach(Box::new(transport));
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}
