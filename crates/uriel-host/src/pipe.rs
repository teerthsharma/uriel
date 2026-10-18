use std::io::ErrorKind;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use tokio::runtime::{Builder, Runtime};
use tokio::time::timeout;

use crate::codec::{body_len_from_prefix, LENGTH_PREFIX};
use crate::error::HostError;
use crate::host::HostInner;
use crate::transport::{map_io, Recv, Transport, FRAME_DEADLINE};

const ERROR_PIPE_BUSY: i32 = 231;
const ERROR_PIPE_NOT_CONNECTED: i32 = 233;
const CHUNK: usize = 8192;
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const CONNECT_RETRY: Duration = Duration::from_millis(10);
const ACCEPT_POLL: Duration = Duration::from_millis(20);

trait PipeIo: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> PipeIo for T {}

pub fn pipe_path(sock_name: &str) -> String {
    format!(r"\\.\pipe\{sock_name}")
}

fn map_pipe(e: std::io::Error) -> HostError {
    if e.raw_os_error() == Some(ERROR_PIPE_NOT_CONNECTED) {
        HostError::Closed
    } else {
        map_io(e)
    }
}

fn new_runtime() -> Result<Arc<Runtime>, HostError> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("uriel-host-pipe")
        .enable_io()
        .enable_time()
        .build()?;
    Ok(Arc::new(runtime))
}

pub struct PipeTransport {
    runtime: Arc<Runtime>,
    pipe: Box<dyn PipeIo>,
    pending: Vec<u8>,
    started: Option<Instant>,
    max_frame: usize,
}

impl PipeTransport {
    fn wrap(runtime: Arc<Runtime>, pipe: Box<dyn PipeIo>, max_frame: usize) -> PipeTransport {
        PipeTransport {
            runtime,
            pipe,
            pending: Vec::new(),
            started: None,
            max_frame,
        }
    }

    pub fn connect(path: &str, max_frame: usize) -> Result<PipeTransport, HostError> {
        let runtime = new_runtime()?;
        let deadline = Instant::now() + CONNECT_WAIT;
        let client = loop {
            let attempt = {
                let _entered = runtime.enter();
                ClientOptions::new().open(path)
            };
            match attempt {
                Ok(client) => break client,
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline =>
                {
                    thread::sleep(CONNECT_RETRY);
                }
                Err(e) => return Err(HostError::Io(e)),
            }
        };
        Ok(PipeTransport::wrap(runtime, Box::new(client), max_frame))
    }

    fn take_frame(&mut self, into: &mut Vec<u8>) -> Result<bool, HostError> {
        if self.pending.len() < LENGTH_PREFIX {
            return Ok(false);
        }
        let mut prefix = [0u8; LENGTH_PREFIX];
        prefix.copy_from_slice(&self.pending[..LENGTH_PREFIX]);
        let total = LENGTH_PREFIX + body_len_from_prefix(prefix, self.max_frame)?;
        if self.pending.len() < total {
            return Ok(false);
        }
        into.clear();
        into.extend_from_slice(&self.pending[..total]);
        self.pending.drain(..total);
        self.started = if self.pending.is_empty() {
            None
        } else {
            Some(Instant::now())
        };
        Ok(true)
    }
}

impl Transport for PipeTransport {
    fn send(&mut self, frame: &[u8]) -> Result<(), HostError> {
        let pipe = &mut self.pipe;
        let outcome = self.runtime.block_on(async {
            timeout(FRAME_DEADLINE, async {
                pipe.write_all(frame).await?;
                pipe.flush().await
            })
            .await
        });
        match outcome {
            Err(_) => Err(HostError::Io(ErrorKind::TimedOut.into())),
            Ok(written) => written.map_err(map_pipe),
        }
    }

    fn recv(&mut self, into: &mut Vec<u8>, wait: Duration) -> Result<Recv, HostError> {
        let mut chunk = [0u8; CHUNK];
        loop {
            if self.take_frame(into)? {
                return Ok(Recv::Frame);
            }
            let (budget, mid_frame) = match self.started {
                Some(since) => (FRAME_DEADLINE.saturating_sub(since.elapsed()), true),
                None => (wait.max(Duration::from_millis(1)), false),
            };
            let pipe = &mut self.pipe;
            let outcome = self
                .runtime
                .block_on(async { timeout(budget, pipe.read(&mut chunk)).await });
            match outcome {
                Err(_) if mid_frame => return Err(HostError::Io(ErrorKind::TimedOut.into())),
                Err(_) => return Ok(Recv::Timeout),
                Ok(Ok(0)) => return Err(HostError::Closed),
                Ok(Ok(n)) => {
                    if self.pending.is_empty() {
                        self.started = Some(Instant::now());
                    }
                    self.pending.extend_from_slice(&chunk[..n]);
                }
                Ok(Err(e)) => return Err(map_pipe(e)),
            }
        }
    }
}

fn create_instance(
    runtime: &Runtime,
    path: &str,
    first: bool,
) -> Result<NamedPipeServer, HostError> {
    let _entered = runtime.enter();
    Ok(ServerOptions::new()
        .first_pipe_instance(first)
        .reject_remote_clients(true)
        .create(path)?)
}

pub(crate) fn start(
    inner: &Arc<HostInner>,
    path: String,
    max_frame: usize,
) -> Result<JoinHandle<()>, HostError> {
    let runtime = new_runtime()?;
    let first = create_instance(&runtime, &path, true)?;
    let accept_inner = inner.clone();
    thread::Builder::new()
        .name("uriel-host-pipe-accept".into())
        .spawn(move || accept_loop(&accept_inner, &runtime, &path, first, max_frame))
        .map_err(HostError::Io)
}

fn accept_loop(
    inner: &Arc<HostInner>,
    runtime: &Arc<Runtime>,
    path: &str,
    first: NamedPipeServer,
    max_frame: usize,
) {
    let mut waiting = Some(first);
    while !inner.is_stopping() {
        let Some(server) = waiting.take() else {
            match create_instance(runtime, path, false) {
                Ok(next) => waiting = Some(next),
                Err(_) => thread::sleep(ACCEPT_POLL),
            }
            continue;
        };
        match runtime.block_on(async { timeout(ACCEPT_POLL, server.connect()).await }) {
            Err(_) => waiting = Some(server),
            Ok(Err(_)) => thread::sleep(ACCEPT_POLL),
            Ok(Ok(())) => {
                waiting = create_instance(runtime, path, false).ok();
                let transport = PipeTransport::wrap(runtime.clone(), Box::new(server), max_frame);
                let _ = inner.attach(Box::new(transport));
            }
        }
    }
}
