use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::codec::{body_len_from_prefix, LENGTH_PREFIX};
use crate::error::HostError;

pub const FRAME_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recv {
    Frame,
    Timeout,
}

pub trait Transport: Send {
    fn send(&mut self, frame: &[u8]) -> Result<(), HostError>;
    fn recv(&mut self, into: &mut Vec<u8>, timeout: Duration) -> Result<Recv, HostError>;
}

pub struct ChannelTransport {
    outgoing: Sender<Vec<u8>>,
    incoming: Receiver<Vec<u8>>,
    spare_for_sending: Receiver<Vec<u8>>,
    spare_returned: Sender<Vec<u8>>,
}

impl ChannelTransport {
    pub fn pair() -> (ChannelTransport, ChannelTransport) {
        let (a_to_b, b_from_a) = channel();
        let (b_to_a, a_from_b) = channel();
        let (pool_for_a, a_pool) = channel();
        let (pool_for_b, b_pool) = channel();
        let a = ChannelTransport {
            outgoing: a_to_b,
            incoming: a_from_b,
            spare_for_sending: a_pool,
            spare_returned: pool_for_b,
        };
        let b = ChannelTransport {
            outgoing: b_to_a,
            incoming: b_from_a,
            spare_for_sending: b_pool,
            spare_returned: pool_for_a,
        };
        (a, b)
    }
}

impl Transport for ChannelTransport {
    fn send(&mut self, frame: &[u8]) -> Result<(), HostError> {
        let mut carrier = self.spare_for_sending.try_recv().unwrap_or_default();
        carrier.clear();
        carrier.extend_from_slice(frame);
        self.outgoing.send(carrier).map_err(|_| HostError::Closed)
    }

    fn recv(&mut self, into: &mut Vec<u8>, timeout: Duration) -> Result<Recv, HostError> {
        match self.incoming.recv_timeout(timeout) {
            Ok(carrier) => {
                into.clear();
                into.extend_from_slice(&carrier);
                let _ = self.spare_returned.send(carrier);
                Ok(Recv::Frame)
            }
            Err(RecvTimeoutError::Timeout) => Ok(Recv::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(HostError::Closed),
        }
    }
}

pub struct TcpTransport {
    stream: TcpStream,
    max_frame: usize,
}

fn is_disconnect(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::UnexpectedEof
            | ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::NotConnected
    )
}

fn map_io(e: std::io::Error) -> HostError {
    if is_disconnect(e.kind()) {
        HostError::Closed
    } else {
        HostError::Io(e)
    }
}

impl TcpTransport {
    pub fn new(stream: TcpStream, max_frame: usize) -> Result<TcpTransport, HostError> {
        stream.set_nodelay(true)?;
        stream.set_nonblocking(false)?;
        Ok(TcpTransport { stream, max_frame })
    }

    pub fn connect(addr: impl ToSocketAddrs, max_frame: usize) -> Result<TcpTransport, HostError> {
        TcpTransport::new(TcpStream::connect(addr)?, max_frame)
    }

    pub fn connect_port_file(
        port_file: &Path,
        max_frame: usize,
    ) -> Result<TcpTransport, HostError> {
        let text = std::fs::read_to_string(port_file)?;
        let port: u16 = text.trim().parse().map_err(|_| {
            HostError::InvalidConfig(format!("port file {} is not a port", port_file.display()))
        })?;
        TcpTransport::connect(SocketAddr::from(([127, 0, 0, 1], port)), max_frame)
    }

    pub fn peer_addr(&self) -> Result<SocketAddr, HostError> {
        Ok(self.stream.peer_addr()?)
    }
}

impl Transport for TcpTransport {
    fn send(&mut self, frame: &[u8]) -> Result<(), HostError> {
        self.stream.write_all(frame).map_err(map_io)?;
        self.stream.flush().map_err(map_io)
    }

    fn recv(&mut self, into: &mut Vec<u8>, timeout: Duration) -> Result<Recv, HostError> {
        self.stream
            .set_read_timeout(Some(timeout.max(Duration::from_millis(1))))?;
        let mut probe = [0u8; 1];
        match self.stream.peek(&mut probe) {
            Ok(0) => return Err(HostError::Closed),
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Ok(Recv::Timeout)
            }
            Err(e) => return Err(map_io(e)),
        }
        self.stream.set_read_timeout(Some(FRAME_DEADLINE))?;
        let mut prefix = [0u8; LENGTH_PREFIX];
        self.stream.read_exact(&mut prefix).map_err(map_io)?;
        let body = body_len_from_prefix(prefix, self.max_frame)?;
        into.clear();
        into.extend_from_slice(&prefix);
        into.resize(LENGTH_PREFIX + body, 0);
        self.stream
            .read_exact(&mut into[LENGTH_PREFIX..])
            .map_err(map_io)?;
        Ok(Recv::Frame)
    }
}
