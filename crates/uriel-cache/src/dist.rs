//! Distributed layer: TCP server, pooled client, and `DistCache`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::error::CacheError;
use crate::local::{CacheMeta, LocalCache};
use crate::membership::{MemberState, Table};
use crate::protocol::{read_frame, ttl_to_ms, write_frame, Request, Response, WireEntry};
use crate::ring::{NodeId, Ring};

/// Behaviour of the distributed layer.
#[derive(Debug, Clone)]
pub struct DistConfig {
    /// Owners (including possibly this node) each key is stored on.
    pub replicas: usize,
    /// Hard bound on any single `get` / `put` / `invalidate` call.
    pub request_timeout: Duration,
    pub vnodes: usize,
    pub max_frame: usize,
}

impl Default for DistConfig {
    fn default() -> Self {
        DistConfig {
            replicas: 2,
            request_timeout: Duration::from_millis(500),
            vnodes: crate::ring::DEFAULT_VNODES,
            max_frame: 16 * 1024 * 1024,
        }
    }
}

/// Peer RPC counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DistStats {
    pub remote_hits: u64,
    pub remote_misses: u64,
    /// Peer calls that failed or timed out (each degraded to a miss).
    pub remote_errors: u64,
}

const MAX_IDLE_PER_PEER: usize = 4;

/// Pooled request/response client with a hard deadline per call.
///
/// Every request in the protocol is idempotent, so a request that fails on a
/// pooled (possibly stale) connection is retried once on a fresh connection.
pub(crate) struct Client {
    pool: Mutex<HashMap<SocketAddr, Vec<TcpStream>>>,
    max_frame: usize,
}

impl Client {
    pub(crate) fn new(max_frame: usize) -> Self {
        Client {
            pool: Mutex::new(HashMap::new()),
            max_frame,
        }
    }

    fn take(&self, addr: SocketAddr) -> Option<TcpStream> {
        let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        pool.get_mut(&addr).and_then(Vec::pop)
    }

    fn give_back(&self, addr: SocketAddr, stream: TcpStream) {
        let mut pool = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        let idle = pool.entry(addr).or_default();
        if idle.len() < MAX_IDLE_PER_PEER {
            idle.push(stream);
        }
    }

    async fn exchange(
        &self,
        stream: &mut TcpStream,
        req: &Request,
    ) -> Result<Response, CacheError> {
        write_frame(stream, req, self.max_frame).await?;
        read_frame(stream, self.max_frame).await
    }

    async fn call_inner(&self, addr: SocketAddr, req: &Request) -> Result<Response, CacheError> {
        if let Some(mut stream) = self.take(addr) {
            if let Ok(resp) = self.exchange(&mut stream, req).await {
                self.give_back(addr, stream);
                return Ok(resp);
            }
            // Stale connection: fall through to a fresh one.
        }
        let mut stream = TcpStream::connect(addr).await?;
        let _ = stream.set_nodelay(true);
        let resp = self.exchange(&mut stream, req).await?;
        self.give_back(addr, stream);
        Ok(resp)
    }

    /// One request/response round trip, connect included, bounded by `deadline`.
    pub(crate) async fn call(
        &self,
        addr: SocketAddr,
        req: &Request,
        deadline: tokio::time::Instant,
    ) -> Result<Response, CacheError> {
        match tokio::time::timeout_at(deadline, self.call_inner(addr, req)).await {
            Ok(r) => r,
            Err(_) => Err(CacheError::Timeout),
        }
    }
}

fn process(local: &LocalCache, req: Request) -> Response {
    match req {
        Request::Get { key } => {
            Response::Value(local.get_entry(&key).map(|e| WireEntry::from_entry(&e)))
        }
        Request::Put { key, entry } => {
            let (value, meta) = entry.into_parts();
            Response::Stored(local.insert(&key, value, meta))
        }
        Request::Invalidate { tags, epoch } => {
            let removed: usize = tags
                .iter()
                .map(|t| local.invalidate_tag_before(t, epoch))
                .sum();
            Response::Invalidated(removed as u64)
        }
    }
}

async fn handle_conn(
    mut stream: TcpStream,
    local: Arc<LocalCache>,
    max_frame: usize,
    mut shutdown: watch::Receiver<bool>,
) {
    let _ = stream.set_nodelay(true);
    loop {
        let req: Request = tokio::select! {
            _ = shutdown.changed() => return,
            r = read_frame(&mut stream, max_frame) => match r {
                Ok(req) => req,
                Err(_) => return,
            },
        };
        let resp = process(&local, req);
        if write_frame(&mut stream, &resp, max_frame).await.is_err() {
            return;
        }
    }
}

/// Serve the cache protocol on `listener` until `shutdown` fires. Dropping the
/// returned future aborts every connection handler.
pub(crate) async fn serve(
    listener: TcpListener,
    local: Arc<LocalCache>,
    max_frame: usize,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    conns.spawn(handle_conn(stream, Arc::clone(&local), max_frame, shutdown.clone()));
                    while conns.try_join_next().is_some() {}
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            },
        }
    }
}

struct Inner {
    local: Arc<LocalCache>,
    table: Arc<Table>,
    ring: Mutex<(u64, Arc<Ring>)>,
    client: Client,
    cfg: DistConfig,
    remote_hits: AtomicU64,
    remote_misses: AtomicU64,
    remote_errors: AtomicU64,
}

enum Fetched {
    Hit(Arc<[u8]>),
    Miss,
    Failed,
}

impl Inner {
    fn ring(&self) -> Arc<Ring> {
        let version = self.table.version();
        let mut g = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        if g.0 != version {
            let mut ring = Ring::new(self.cfg.vnodes);
            for m in self.table.snapshot() {
                if m.state != MemberState::Dead {
                    ring.add_node(&m.id);
                }
            }
            *g = (version, Arc::new(ring));
        }
        Arc::clone(&g.1)
    }

    /// Remote owners of `key` other than this node, with their addresses.
    fn remote_owners(&self, key: &str) -> Vec<SocketAddr> {
        let me = self.table.me().id;
        self.ring()
            .owners(key, self.cfg.replicas)
            .into_iter()
            .filter(|o| *o != me)
            .filter_map(|o| self.table.get(&o).map(|m| m.addr))
            .collect()
    }

    async fn fetch(
        self: Arc<Self>,
        addr: SocketAddr,
        key: String,
        deadline: tokio::time::Instant,
    ) -> Fetched {
        match self
            .client
            .call(addr, &Request::Get { key: key.clone() }, deadline)
            .await
        {
            Ok(Response::Value(Some(entry))) => {
                let (value, meta) = entry.into_parts();
                if self.local.is_stale(meta.epoch, &meta.tags) {
                    self.remote_misses.fetch_add(1, Ordering::Relaxed);
                    return Fetched::Miss;
                }
                let value: Arc<[u8]> = Arc::from(value);
                self.local.insert(&key, Arc::clone(&value), meta);
                self.remote_hits.fetch_add(1, Ordering::Relaxed);
                Fetched::Hit(value)
            }
            Ok(_) => {
                self.remote_misses.fetch_add(1, Ordering::Relaxed);
                Fetched::Miss
            }
            Err(_) => {
                self.remote_errors.fetch_add(1, Ordering::Relaxed);
                Fetched::Failed
            }
        }
    }

    /// Send `req` to every address concurrently; all calls share one deadline.
    async fn broadcast(self: &Arc<Self>, targets: Vec<SocketAddr>, req: Request) -> Vec<Response> {
        if targets.is_empty() {
            return Vec::new();
        }
        let deadline = tokio::time::Instant::now() + self.cfg.request_timeout;
        let req = Arc::new(req);
        let mut set = JoinSet::new();
        for addr in targets {
            let inner = Arc::clone(self);
            let req = Arc::clone(&req);
            set.spawn(async move { inner.client.call(addr, &req, deadline).await });
        }
        let mut out = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(resp)) => out.push(resp),
                _ => {
                    self.remote_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        out
    }
}

/// Local cache + ring + peer protocol. Cheap to clone.
#[derive(Clone)]
pub struct DistCache {
    inner: Arc<Inner>,
}

impl DistCache {
    pub(crate) fn new(table: Arc<Table>, local: Arc<LocalCache>, cfg: DistConfig) -> Self {
        let client = Client::new(cfg.max_frame);
        DistCache {
            inner: Arc::new(Inner {
                local,
                table,
                ring: Mutex::new((0, Arc::new(Ring::new(cfg.vnodes)))),
                client,
                cfg,
                remote_hits: AtomicU64::new(0),
                remote_misses: AtomicU64::new(0),
                remote_errors: AtomicU64::new(0),
            }),
        }
    }

    pub fn local(&self) -> &Arc<LocalCache> {
        &self.inner.local
    }

    pub fn id(&self) -> NodeId {
        self.inner.table.me().id
    }

    pub fn stats(&self) -> DistStats {
        DistStats {
            remote_hits: self.inner.remote_hits.load(Ordering::Relaxed),
            remote_misses: self.inner.remote_misses.load(Ordering::Relaxed),
            remote_errors: self.inner.remote_errors.load(Ordering::Relaxed),
        }
    }

    /// Owners of `key` in the current ring (alive and suspect members).
    pub fn owners(&self, key: &str) -> Vec<NodeId> {
        self.inner.ring().owners(key, self.inner.cfg.replicas)
    }

    /// Local hit, else ask the ring owners. Never errors and never outlives
    /// `request_timeout`: an unreachable peer is a miss.
    pub async fn get(&self, key: &str) -> Option<Arc<[u8]>> {
        if let Some(v) = self.inner.local.get(key) {
            return Some(v);
        }
        let targets = self.inner.remote_owners(key);
        let deadline = tokio::time::Instant::now() + self.inner.cfg.request_timeout;
        match targets.as_slice() {
            [] => None,
            [addr] => match Arc::clone(&self.inner)
                .fetch(*addr, key.to_owned(), deadline)
                .await
            {
                Fetched::Hit(v) => Some(v),
                _ => None,
            },
            _ => {
                // Ask every owner at once; the first hit wins and dropping the set
                // aborts the rest, so one silent replica cannot hide a healthy one.
                let mut set = JoinSet::new();
                for addr in targets {
                    set.spawn(Arc::clone(&self.inner).fetch(addr, key.to_owned(), deadline));
                }
                while let Some(joined) = set.join_next().await {
                    if let Ok(Fetched::Hit(v)) = joined {
                        return Some(v);
                    }
                }
                None
            }
        }
    }

    /// Store locally and on the owner replicas. Returns remote replicas that acked.
    pub async fn put(&self, key: &str, value: impl Into<Arc<[u8]>>, meta: CacheMeta) -> usize {
        let value: Arc<[u8]> = value.into();
        let wire = WireEntry {
            value: value.to_vec(),
            epoch: meta.epoch,
            tags: meta.tags.clone(),
            ttl_ms: meta.ttl.map(ttl_to_ms),
        };
        self.inner.local.insert(key, value, meta);
        let targets = self.inner.remote_owners(key);
        let req = Request::Put {
            key: key.to_owned(),
            entry: wire,
        };
        self.inner
            .broadcast(targets, req)
            .await
            .into_iter()
            .filter(|r| matches!(r, Response::Stored(true)))
            .count()
    }

    /// Drop entries tagged with any of `tags` and older than `new_epoch` on this node
    /// and (best effort) on all live peers. Returns peers that acked.
    pub async fn invalidate(&self, tags: &[String], new_epoch: u64) -> usize {
        for t in tags {
            self.inner.local.invalidate_tag_before(t, new_epoch);
        }
        let me = self.inner.table.me().id;
        let targets: Vec<SocketAddr> = self
            .inner
            .table
            .peers()
            .into_iter()
            .filter(|m| m.id != me)
            .map(|m| m.addr)
            .collect();
        let req = Request::Invalidate {
            tags: tags.to_vec(),
            epoch: new_epoch,
        };
        self.inner
            .broadcast(targets, req)
            .await
            .into_iter()
            .filter(|r| matches!(r, Response::Invalidated(_)))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::{Member, MemberState};
    use std::time::Instant;
    use tokio::net::TcpListener;

    const T: Duration = Duration::from_millis(300);

    fn member(id: &str, addr: SocketAddr, state: MemberState) -> Member {
        Member {
            id: NodeId::new(id),
            addr,
            state,
            incarnation: 0,
        }
    }

    fn table(me: &str, peers: &[(&str, SocketAddr)]) -> Arc<Table> {
        let t = Table::new(member(
            me,
            "127.0.0.1:1".parse().unwrap(),
            MemberState::Alive,
        ));
        for (id, addr) in peers {
            t.apply(
                &member(id, *addr, MemberState::Alive),
                std::time::Instant::now(),
            );
        }
        Arc::new(t)
    }

    fn cfg(replicas: usize) -> DistConfig {
        DistConfig {
            replicas,
            request_timeout: T,
            ..DistConfig::default()
        }
    }

    /// A peer that accepts TCP connections and never answers.
    async fn silent_peer() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
        addr
    }

    /// A healthy peer: real server over its own LocalCache.
    async fn healthy_peer() -> (SocketAddr, Arc<LocalCache>, watch::Sender<bool>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let local = Arc::new(LocalCache::new(1 << 20));
        let (tx, rx) = watch::channel(false);
        tokio::spawn(serve(l, Arc::clone(&local), 1 << 20, rx));
        (addr, local, tx)
    }

    fn dist(me: &str, peers: &[(&str, SocketAddr)], replicas: usize) -> DistCache {
        DistCache::new(
            table(me, peers),
            Arc::new(LocalCache::new(1 << 20)),
            cfg(replicas),
        )
    }

    fn deadline(d: Duration) -> tokio::time::Instant {
        tokio::time::Instant::now() + d
    }

    // ---------- Client ----------

    #[tokio::test]
    async fn client_gets_a_value_from_a_real_server() {
        let (addr, local, _tx) = healthy_peer().await;
        local.insert("k", vec![5, 6], CacheMeta::new(3).with_tags(["A"]));
        let c = Client::new(1 << 20);
        let r = c
            .call(addr, &Request::Get { key: "k".into() }, deadline(T))
            .await
            .unwrap();
        match r {
            Response::Value(Some(e)) => {
                assert_eq!(e.value, vec![5, 6]);
                assert_eq!(e.epoch, 3);
                assert_eq!(e.tags, vec!["A".to_string()]);
            }
            other => panic!("{other:?}"),
        }
        let r = c
            .call(
                addr,
                &Request::Get {
                    key: "absent".into(),
                },
                deadline(T),
            )
            .await
            .unwrap();
        assert_eq!(r, Response::Value(None));
    }

    #[tokio::test]
    async fn client_times_out_against_a_silent_peer() {
        let addr = silent_peer().await;
        let c = Client::new(1 << 20);
        let start = Instant::now();
        let err = c
            .call(
                addr,
                &Request::Get { key: "k".into() },
                deadline(Duration::from_millis(150)),
            )
            .await
            .unwrap_err();
        let took = start.elapsed();
        assert!(matches!(err, CacheError::Timeout), "{err:?}");
        assert!(
            took >= Duration::from_millis(140) && took < Duration::from_secs(2),
            "{took:?}"
        );
    }

    #[tokio::test]
    async fn client_reports_refused_connections_quickly() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let c = Client::new(1 << 20);
        let start = Instant::now();
        let err = c
            .call(
                addr,
                &Request::Get { key: "k".into() },
                deadline(Duration::from_secs(5)),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, CacheError::Io(_) | CacheError::Timeout),
            "{err:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(4));
    }

    #[tokio::test]
    async fn client_retries_on_a_fresh_connection_when_the_pooled_one_is_dead() {
        let (addr, local, tx) = healthy_peer().await;
        local.insert("k", vec![1], CacheMeta::new(1));
        let c = Client::new(1 << 20);
        let r = c
            .call(addr, &Request::Get { key: "k".into() }, deadline(T))
            .await;
        assert!(matches!(r, Ok(Response::Value(Some(_)))));
        assert_eq!(
            c.pool.lock().unwrap().get(&addr).map_or(0, Vec::len),
            1,
            "connection pooled"
        );

        // Kill the server (closing its connections) and bring up another on the same port.
        tx.send(true).unwrap();
        let start = Instant::now();
        let l2 = loop {
            match TcpListener::bind(addr).await {
                Ok(l) => break l,
                Err(e) => {
                    assert!(
                        start.elapsed() < Duration::from_secs(5),
                        "rebind failed: {e}"
                    );
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        };
        let local2 = Arc::new(LocalCache::new(1 << 20));
        local2.insert("k", vec![2], CacheMeta::new(1));
        let (_tx2, rx2) = watch::channel(false);
        tokio::spawn(serve(l2, local2, 1 << 20, rx2));

        let r = c
            .call(
                addr,
                &Request::Get { key: "k".into() },
                deadline(Duration::from_secs(2)),
            )
            .await
            .unwrap();
        match r {
            Response::Value(Some(e)) => assert_eq!(e.value, vec![2]),
            other => panic!("{other:?}"),
        }
    }

    // ---------- DistCache ----------

    #[tokio::test]
    async fn local_hit_never_touches_the_network() {
        let silent = silent_peer().await;
        let d = dist("me", &[("s", silent)], 2);
        d.local().insert("k", vec![9], CacheMeta::new(1));
        let start = Instant::now();
        assert_eq!(&*d.get("k").await.unwrap(), &[9]);
        assert!(start.elapsed() < T / 2);
        assert_eq!(d.stats(), DistStats::default());
    }

    #[tokio::test]
    async fn get_with_silent_owner_is_a_miss_within_the_timeout() {
        let silent = silent_peer().await;
        let d = dist("me", &[("s", silent)], 2);
        let start = Instant::now();
        assert!(d.get("anything").await.is_none());
        let took = start.elapsed();
        assert!(took < T + Duration::from_millis(500), "{took:?}");
        assert!(d.stats().remote_errors >= 1 || d.owners("anything") == vec![NodeId::new("me")]);
    }

    #[tokio::test]
    async fn get_deadline_is_shared_across_many_silent_replicas() {
        let peers: Vec<(String, SocketAddr)> = {
            let mut v = Vec::new();
            for i in 0..4 {
                v.push((format!("s{i}"), silent_peer().await));
            }
            v
        };
        let refs: Vec<(&str, SocketAddr)> = peers.iter().map(|(n, a)| (n.as_str(), *a)).collect();
        let d = dist("me", &refs, 5);
        let start = Instant::now();
        assert!(d.get("k").await.is_none());
        let took = start.elapsed();
        assert!(
            took < T + Duration::from_millis(400),
            "sequential timeouts would take ~4x: {took:?}"
        );
        assert!(
            took >= T - Duration::from_millis(50),
            "must wait out the deadline: {took:?}"
        );
    }

    #[tokio::test]
    async fn silent_replica_does_not_hide_a_healthy_replica() {
        let silent = silent_peer().await;
        let (healthy, hlocal, _tx) = healthy_peer().await;
        hlocal.insert("k", vec![42], CacheMeta::new(1));
        let d = dist("me", &[("s", silent), ("h", healthy)], 3);
        let start = Instant::now();
        assert_eq!(&*d.get("k").await.unwrap(), &[42]);
        assert!(
            start.elapsed() < T / 2,
            "healthy replica must not wait for the silent one"
        );
    }

    #[tokio::test]
    async fn remote_hit_populates_local_with_epoch_tags_and_remaining_ttl() {
        let (peer, plocal, _tx) = healthy_peer().await;
        plocal.insert(
            "k",
            vec![1, 2],
            CacheMeta::new(4)
                .with_tags(["T"])
                .with_ttl(Duration::from_secs(10)),
        );
        let d = dist("me", &[("p", peer)], 2);
        assert_eq!(&*d.get("k").await.unwrap(), &[1, 2]);
        let e = d.local().get_entry("k").expect("populated locally");
        assert_eq!(e.meta.epoch, 4);
        assert_eq!(e.meta.tags, vec!["T".to_string()]);
        let ttl = e.meta.ttl.unwrap();
        assert!(
            ttl <= Duration::from_secs(10) && ttl > Duration::from_secs(5),
            "{ttl:?}"
        );
        let s = d.stats();
        assert_eq!((s.remote_hits, s.remote_errors), (1, 0));
    }

    #[tokio::test]
    async fn remote_hit_older_than_a_local_epoch_floor_is_a_miss() {
        let (peer, plocal, _tx) = healthy_peer().await;
        plocal.insert("k", vec![1], CacheMeta::new(4).with_tags(["T"]));
        let d = dist("me", &[("p", peer)], 2);
        d.local().invalidate_tag_before("T", 5);
        assert!(
            d.get("k").await.is_none(),
            "stale remote value must not be served"
        );
        assert!(d.local().get("k").is_none());
    }

    #[tokio::test]
    async fn remote_miss_is_counted_and_returns_none() {
        let (peer, _plocal, _tx) = healthy_peer().await;
        let d = dist("me", &[("p", peer)], 2);
        // pick a key that the healthy peer owns, so it is actually asked
        let key = (0..200)
            .map(|i| format!("k{i}"))
            .find(|k| d.owners(k).contains(&NodeId::new("p")))
            .unwrap();
        assert!(d.get(&key).await.is_none());
        assert_eq!(d.stats().remote_misses, 1);
    }

    #[tokio::test]
    async fn put_stores_locally_and_on_owner_replicas() {
        let (peer, plocal, _tx) = healthy_peer().await;
        let d = dist("me", &[("p", peer)], 2);
        let acks = d
            .put("k", vec![7], CacheMeta::new(2).with_tags(["T"]))
            .await;
        assert_eq!(acks, 1);
        assert!(d.local().get("k").is_some());
        let e = plocal.get_entry("k").expect("replicated");
        assert_eq!((e.meta.epoch, e.meta.tags), (2, vec!["T".to_string()]));
    }

    #[tokio::test]
    async fn put_with_silent_peer_still_succeeds_locally_within_the_timeout() {
        let silent = silent_peer().await;
        let d = dist("me", &[("s", silent)], 2);
        let start = Instant::now();
        let acks = d.put("k", vec![7], CacheMeta::new(1)).await;
        assert_eq!(acks, 0);
        assert!(start.elapsed() < T + Duration::from_millis(500));
        assert!(d.local().get("k").is_some());
    }

    #[tokio::test]
    async fn invalidate_applies_locally_and_broadcasts_to_every_live_peer() {
        let (p1, l1, _t1) = healthy_peer().await;
        let (p2, l2, _t2) = healthy_peer().await;
        let silent = silent_peer().await;
        let d = dist("me", &[("p1", p1), ("p2", p2), ("s", silent)], 2);
        for l in [d.local().as_ref(), l1.as_ref(), l2.as_ref()] {
            l.insert("old", vec![1], CacheMeta::new(1).with_tags(["T"]));
            l.insert("new", vec![1], CacheMeta::new(9).with_tags(["T"]));
            l.insert("other", vec![1], CacheMeta::new(1).with_tags(["U"]));
        }
        let start = Instant::now();
        let acks = d.invalidate(&["T".to_string()], 5).await;
        assert_eq!(acks, 2, "two healthy peers acked, the silent one did not");
        assert!(start.elapsed() < T + Duration::from_millis(500));
        for l in [d.local().as_ref(), l1.as_ref(), l2.as_ref()] {
            assert!(l.get("old").is_none());
            assert!(l.get("new").is_some(), "epoch 9 >= 5 survives");
            assert!(l.get("other").is_some());
        }
        // The floor blocks late stale writes everywhere that acked.
        assert!(!l1.insert("late", vec![1], CacheMeta::new(4).with_tags(["T"])));
    }

    #[tokio::test]
    async fn ring_follows_membership_changes() {
        let (peer, _l, _tx) = healthy_peer().await;
        let t = table("me", &[("p", peer)]);
        let d = DistCache::new(Arc::clone(&t), Arc::new(LocalCache::new(1 << 20)), cfg(1));
        let owned_by_p: Vec<String> = (0..500)
            .map(|i| format!("k{i}"))
            .filter(|k| d.owners(k) == vec![NodeId::new("p")])
            .collect();
        assert!(!owned_by_p.is_empty());

        t.apply(
            &member("p", peer, MemberState::Dead),
            std::time::Instant::now(),
        );
        for k in &owned_by_p {
            assert_eq!(
                d.owners(k),
                vec![NodeId::new("me")],
                "dead node leaves the ring"
            );
        }
        t.apply(
            &Member {
                incarnation: 1,
                ..member("p", peer, MemberState::Alive)
            },
            std::time::Instant::now(),
        );
        for k in &owned_by_p {
            assert_eq!(
                d.owners(k),
                vec![NodeId::new("p")],
                "rejoined node returns to the ring"
            );
        }
        assert_eq!(d.stats().remote_errors, 0);
    }
}
