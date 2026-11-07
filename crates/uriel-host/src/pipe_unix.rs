use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::codec::{body_len_from_prefix, LENGTH_PREFIX};
use crate::error::HostError;
use crate::host::HostInner;
use crate::transport::{map_io, Recv, Transport, FRAME_DEADLINE};

const CHUNK: usize = 8192;
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const CONNECT_RETRY: Duration = Duration::from_millis(10);
const ACCEPT_POLL: Duration = Duration::from_millis(20);
const OWNER_ONLY: u32 = 0o600;

pub fn pipe_path(sock_name: &str) -> String {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{sock_name}.sock")).to_string_lossy().into_owned()
}

pub struct PipeTransport {
    stream: UnixStream,
    pending: Vec<u8>,
    started: Option<Instant>,
    max_frame: usize,
}

impl PipeTransport {
    fn wrap(stream: UnixStream, max_frame: usize) -> PipeTransport {
        PipeTransport {
            stream,
            pending: Vec::new(),
            started: None,
            max_frame,
        }
    }

    pub fn connect(path: &str, max_frame: usize) -> Result<PipeTransport, HostError> {
        let deadline = Instant::now() + CONNECT_WAIT;
        let stream = loop {
            match UnixStream::connect(path) {
                Ok(stream) => break stream,
                Err(e)
                    if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused)
                        && Instant::now() < deadline =>
                {
                    thread::sleep(CONNECT_RETRY);
                }
                Err(e) => return Err(HostError::Io(e)),
            }
        };
        Ok(PipeTransport::wrap(stream, max_frame))
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
        self.stream.set_write_timeout(Some(FRAME_DEADLINE))?;
        self.stream.write_all(frame).map_err(map_io)?;
        self.stream.flush().map_err(map_io)
    }

    fn recv(&mut self, into: &mut Vec<u8>, wait: Duration) -> Result<Recv, HostError> {
        let mut chunk = [0u8; CHUNK];
        loop {
            if self.take_frame(into)? {
                return Ok(Recv::Frame);
            }
            let (budget, mid_frame) = match self.started {
                Some(since) => (FRAME_DEADLINE.saturating_sub(since.elapsed()), true),
                None => (wait, false),
            };
            self.stream
                .set_read_timeout(Some(budget.max(Duration::from_millis(1))))?;
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(HostError::Closed),
                Ok(n) => {
                    if self.pending.is_empty() {
                        self.started = Some(Instant::now());
                    }
                    self.pending.extend_from_slice(&chunk[..n]);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if mid_frame {
                        return Err(HostError::Io(ErrorKind::TimedOut.into()));
                    }
                    return Ok(Recv::Timeout);
                }
                Err(e) => return Err(map_io(e)),
            }
        }
    }
}

fn bind_owner_only(path: &str) -> Result<UnixListener, HostError> {
    if UnixStream::connect(path).is_ok() {
        return Err(HostError::Io(ErrorKind::AddrInUse.into()));
    }
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(OWNER_ONLY))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub(crate) fn start(
    inner: &Arc<HostInner>,
    path: String,
    max_frame: usize,
) -> Result<JoinHandle<()>, HostError> {
    let listener = bind_owner_only(&path)?;
    let accept_inner = inner.clone();
    thread::Builder::new()
        .name("uriel-host-socket-accept".into())
        .spawn(move || accept_loop(&accept_inner, &listener, &path, max_frame))
        .map_err(HostError::Io)
}

fn accept_loop(inner: &Arc<HostInner>, listener: &UnixListener, path: &str, max_frame: usize) {
    while !inner.is_stopping() {
        match listener.accept() {
            Ok((stream, _)) => {
                if stream.set_nonblocking(false).is_ok() {
                    let _ = inner.attach(Box::new(PipeTransport::wrap(stream, max_frame)));
                }
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
    let _ = std::fs::remove_file(path);
}
