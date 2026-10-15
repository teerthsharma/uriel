//! `CacheNode`: one process-local participant in the cluster.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::watch;

use crate::dist::{serve, DistCache, DistConfig};
use crate::error::CacheError;
use crate::hash::hash64;
use crate::local::LocalCache;
use crate::membership::{spawn_gossip, GossipConfig, GossipHandle, Member, MemberState};
use crate::ring::NodeId;

/// Everything needed to start a node.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Address for both the TCP cache protocol and UDP gossip. Port 0 picks a free
    /// port that is available for both protocols.
    pub bind: SocketAddr,
    /// Address advertised to peers; required when `bind` is an unspecified address.
    pub advertise: Option<SocketAddr>,
    /// Defaults to a random id.
    pub node_id: Option<NodeId>,
    /// Gossip addresses of existing members to join through.
    pub seeds: Vec<SocketAddr>,
    /// Local cache capacity in bytes.
    pub max_bytes: usize,
    pub vnodes: usize,
    /// Owners per key.
    pub replicas: usize,
    /// Hard bound on each peer-facing operation.
    pub request_timeout: Duration,
    pub max_frame: usize,
    pub gossip: GossipConfig,
}

impl Default for NodeConfig {
    fn default() -> Self {
        let d = DistConfig::default();
        NodeConfig {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            advertise: None,
            node_id: None,
            seeds: Vec::new(),
            max_bytes: 64 * 1024 * 1024,
            vnodes: d.vnodes,
            replicas: d.replicas,
            request_timeout: d.request_timeout,
            max_frame: d.max_frame,
            gossip: GossipConfig::default(),
        }
    }
}

/// Handle to a running node. Dropping it without `shutdown`/`abort` leaves the
/// background tasks running until the runtime stops.
pub struct CacheNode {
    id: NodeId,
    addr: SocketAddr,
    cache: DistCache,
    gossip: GossipHandle,
    stop_server: tokio::sync::watch::Sender<bool>,
    server: tokio::task::JoinHandle<()>,
}

/// Bind TCP and UDP on the same port. With port 0, retry until the OS-chosen TCP
/// port is also free for UDP.
async fn bind_pair(bind: SocketAddr) -> Result<(TcpListener, UdpSocket), CacheError> {
    let bind_err = |source| CacheError::Bind { addr: bind, source };
    if bind.port() != 0 {
        let tcp = TcpListener::bind(bind).await.map_err(bind_err)?;
        let udp = UdpSocket::bind(bind).await.map_err(bind_err)?;
        return Ok((tcp, udp));
    }
    let mut last = None;
    for _ in 0..64 {
        let tcp = TcpListener::bind(bind).await.map_err(bind_err)?;
        let port = tcp.local_addr().map_err(bind_err)?.port();
        match UdpSocket::bind(SocketAddr::new(bind.ip(), port)).await {
            Ok(udp) => return Ok((tcp, udp)),
            Err(e) => last = Some(e),
        }
    }
    Err(bind_err(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            "no port free for both TCP and UDP",
        )
    })))
}

fn generated_id(addr: SocketAddr) -> NodeId {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut material = nanos.to_le_bytes().to_vec();
    material.extend_from_slice(&u64::from(std::process::id()).to_le_bytes());
    material.extend_from_slice(&COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    NodeId::new(format!(
        "node-{:016x}",
        hash64(&material, u64::from(addr.port()))
    ))
}

impl CacheNode {
    pub async fn spawn(cfg: NodeConfig) -> Result<CacheNode, CacheError> {
        if cfg.replicas == 0 {
            return Err(CacheError::InvalidConfig("replicas must be at least 1"));
        }
        if cfg.vnodes == 0 {
            return Err(CacheError::InvalidConfig("vnodes must be at least 1"));
        }
        if cfg.max_bytes == 0 {
            return Err(CacheError::InvalidConfig("max_bytes must be non-zero"));
        }
        if cfg.request_timeout.is_zero() {
            return Err(CacheError::InvalidConfig(
                "request_timeout must be non-zero",
            ));
        }
        if cfg.bind.ip().is_unspecified() && cfg.advertise.is_none() {
            return Err(CacheError::InvalidConfig(
                "binding an unspecified address requires `advertise`",
            ));
        }

        let (listener, udp) = bind_pair(cfg.bind).await?;
        let bound = listener.local_addr()?;
        let addr = cfg.advertise.unwrap_or(bound);
        let id = cfg.node_id.clone().unwrap_or_else(|| generated_id(addr));

        let local = Arc::new(LocalCache::new(cfg.max_bytes));
        let (stop_server, stop_rx) = watch::channel(false);
        let server = tokio::spawn(serve(listener, Arc::clone(&local), cfg.max_frame, stop_rx));

        let me = Member {
            id: id.clone(),
            addr,
            state: MemberState::Alive,
            incarnation: 0,
        };
        let gossip = spawn_gossip(udp, me, cfg.gossip, cfg.seeds);
        let cache = DistCache::new(
            Arc::clone(&gossip.table),
            local,
            DistConfig {
                replicas: cfg.replicas,
                request_timeout: cfg.request_timeout,
                vnodes: cfg.vnodes,
                max_frame: cfg.max_frame,
            },
        );
        Ok(CacheNode {
            id,
            addr,
            cache,
            gossip,
            stop_server,
            server,
        })
    }

    /// The address peers use for both gossip (UDP) and cache requests (TCP).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn id(&self) -> &NodeId {
        &self.id
    }

    pub fn cache(&self) -> &DistCache {
        &self.cache
    }

    /// Current membership table, sorted by id.
    pub fn members(&self) -> Vec<Member> {
        self.gossip.table.snapshot()
    }

    /// Announce departure to live peers, then stop serving.
    pub async fn shutdown(self) {
        self.gossip.leave_and_stop().await;
        let _ = self.stop_server.send(true);
        let _ = self.server.await;
    }

    /// Stop everything immediately without announcing (simulates a crash).
    pub async fn abort(self) {
        self.gossip.crash().await;
        self.server.abort();
        let _ = self.server.await;
    }
}
