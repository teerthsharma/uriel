//! SWIM-lite membership over UDP.
//!
//! * Failure detection: every `probe_interval` a node pings one peer; if no ack
//!   arrives within `probe_timeout` it asks `indirect_probes` other peers to ping
//!   the target for it; if the round still ends without an ack the target is
//!   marked `Suspect`, and `Dead` after `suspect_timeout`.
//! * Suspicion is refuted by the suspected node bumping its incarnation number.
//! * Dissemination: every datagram piggybacks the members that have been
//!   transmitted the fewest times, so fresh updates spread first and the rest of
//!   the table keeps circulating as anti-entropy.
//! * Join: a node pings its seed addresses until it learns of another member.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::hash::Rng;
use crate::ring::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemberState {
    Alive,
    Suspect,
    Dead,
}

impl MemberState {
    fn rank(self) -> u8 {
        match self {
            MemberState::Alive => 0,
            MemberState::Suspect => 1,
            MemberState::Dead => 2,
        }
    }
}

/// One row of the membership table. `addr` serves both UDP gossip and the TCP
/// cache protocol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub id: NodeId,
    pub addr: SocketAddr,
    pub state: MemberState,
    pub incarnation: u64,
}

/// Gossip timing and sizing knobs.
#[derive(Debug, Clone)]
pub struct GossipConfig {
    pub probe_interval: Duration,
    /// Wait this long for a direct ack before probing indirectly.
    pub probe_timeout: Duration,
    /// Suspect -> Dead delay.
    pub suspect_timeout: Duration,
    pub indirect_probes: usize,
    /// Members piggybacked per datagram.
    pub max_piggyback: usize,
    /// How long Dead tombstones are kept before being reaped.
    pub dead_retention: Duration,
}

impl Default for GossipConfig {
    fn default() -> Self {
        GossipConfig {
            probe_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_millis(500),
            suspect_timeout: Duration::from_secs(5),
            indirect_probes: 3,
            max_piggyback: 12,
            dead_retention: Duration::from_secs(60),
        }
    }
}

impl GossipConfig {
    /// Millisecond-scale timings for tests and local clusters.
    pub fn fast() -> Self {
        GossipConfig {
            probe_interval: Duration::from_millis(50),
            probe_timeout: Duration::from_millis(25),
            suspect_timeout: Duration::from_millis(400),
            indirect_probes: 3,
            max_piggyback: 12,
            dead_retention: Duration::from_secs(30),
        }
    }
}

/// Result of merging one record into the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    Ignored,
    Updated,
    /// The record accused *this* node; its incarnation was bumped.
    Refuted,
}

struct Entry {
    member: Member,
    /// Times this record has been piggybacked since it last changed.
    tx: u32,
    /// When the record last changed (drives suspect expiry and tombstone reaping).
    since: Instant,
}

struct TableInner {
    me: NodeId,
    left: bool,
    members: HashMap<NodeId, Entry>,
}

/// Membership table: a pure state machine, no I/O.
///
/// A record replaces the stored one iff `(incarnation, severity)` is strictly
/// greater, with severity `Alive < Suspect < Dead`. A node refutes accusations
/// by raising its own incarnation above the accusation.
pub(crate) struct Table {
    inner: Mutex<TableInner>,
    version: AtomicU64,
}

impl Table {
    pub(crate) fn new(me: Member) -> Self {
        let id = me.id.clone();
        let mut members = HashMap::new();
        members.insert(
            id.clone(),
            Entry {
                member: me,
                tx: 0,
                since: Instant::now(),
            },
        );
        Table {
            inner: Mutex::new(TableInner {
                me: id,
                left: false,
                members,
            }),
            version: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TableInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn me(&self) -> Member {
        let g = self.lock();
        g.members[&g.me].member.clone()
    }

    /// Increases on every accepted change to the table.
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub(crate) fn get(&self, id: &NodeId) -> Option<Member> {
        self.lock().members.get(id).map(|e| e.member.clone())
    }

    pub(crate) fn snapshot(&self) -> Vec<Member> {
        let mut v: Vec<Member> = self
            .lock()
            .members
            .values()
            .map(|e| e.member.clone())
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Members that may be probed: everyone except self and the dead.
    pub(crate) fn peers(&self) -> Vec<Member> {
        let g = self.lock();
        let mut v: Vec<Member> = g
            .members
            .values()
            .filter(|e| e.member.id != g.me && e.member.state != MemberState::Dead)
            .map(|e| e.member.clone())
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub(crate) fn apply(&self, m: &Member, now: Instant) -> Applied {
        let mut g = self.lock();
        if m.id == g.me {
            if g.left {
                return Applied::Ignored;
            }
            let me = g.me.clone();
            let e = g.members.get_mut(&me).expect("self row exists");
            if m.state != MemberState::Alive && m.incarnation >= e.member.incarnation {
                e.member.incarnation = m.incarnation + 1;
                e.member.state = MemberState::Alive;
                e.tx = 0;
                e.since = now;
                drop(g);
                self.bump();
                return Applied::Refuted;
            }
            return Applied::Ignored;
        }
        let applied = match g.members.get_mut(&m.id) {
            None => {
                g.members.insert(
                    m.id.clone(),
                    Entry {
                        member: m.clone(),
                        tx: 0,
                        since: now,
                    },
                );
                true
            }
            Some(e) => {
                let newer =
                    (m.incarnation, m.state.rank()) > (e.member.incarnation, e.member.state.rank());
                if newer {
                    e.member = m.clone();
                    e.tx = 0;
                    e.since = now;
                }
                newer
            }
        };
        drop(g);
        if applied {
            self.bump();
            Applied::Updated
        } else {
            Applied::Ignored
        }
    }

    /// Alive -> Suspect at the current incarnation.
    pub(crate) fn mark_suspect(&self, id: &NodeId, now: Instant) -> bool {
        let mut g = self.lock();
        if *id == g.me {
            return false;
        }
        let changed = match g.members.get_mut(id) {
            Some(e) if e.member.state == MemberState::Alive => {
                e.member.state = MemberState::Suspect;
                e.tx = 0;
                e.since = now;
                true
            }
            _ => false,
        };
        drop(g);
        if changed {
            self.bump();
        }
        changed
    }

    /// Promote overdue suspects to dead, reap old tombstones; returns new deaths.
    pub(crate) fn expire(
        &self,
        now: Instant,
        suspect_timeout: Duration,
        dead_retention: Duration,
    ) -> Vec<NodeId> {
        let mut g = self.lock();
        let me = g.me.clone();
        let mut died = Vec::new();
        let mut reap = Vec::new();
        for (id, e) in g.members.iter_mut() {
            if *id == me {
                continue;
            }
            let age = now.saturating_duration_since(e.since);
            match e.member.state {
                MemberState::Suspect if age >= suspect_timeout => {
                    e.member.state = MemberState::Dead;
                    e.tx = 0;
                    e.since = now;
                    died.push(id.clone());
                }
                MemberState::Dead if age >= dead_retention => reap.push(id.clone()),
                _ => {}
            }
        }
        for id in &reap {
            g.members.remove(id);
        }
        drop(g);
        if !died.is_empty() || !reap.is_empty() {
            self.bump();
        }
        died
    }

    /// Up to `k` records, least-transmitted first.
    pub(crate) fn piggyback(&self, k: usize) -> Vec<Member> {
        let mut g = self.lock();
        let mut order: Vec<(u32, NodeId)> =
            g.members.iter().map(|(id, e)| (e.tx, id.clone())).collect();
        order.sort_unstable();
        order.truncate(k);
        order
            .into_iter()
            .filter_map(|(_, id)| {
                g.members.get_mut(&id).map(|e| {
                    e.tx = e.tx.saturating_add(1);
                    e.member.clone()
                })
            })
            .collect()
    }

    /// Declare this node dead at a higher incarnation (voluntary leave).
    pub(crate) fn leave(&self) -> Member {
        let mut g = self.lock();
        g.left = true;
        let me = g.me.clone();
        let e = g.members.get_mut(&me).expect("self row exists");
        e.member.incarnation += 1;
        e.member.state = MemberState::Dead;
        let m = e.member.clone();
        drop(g);
        self.bump();
        m
    }
}

#[derive(Debug, Default)]
pub(crate) struct GossipStats {
    pub suspicions: AtomicU64,
    pub indirect_probes: AtomicU64,
}

/// Datagram budget: stay under a typical path MTU.
const MAX_DATAGRAM: usize = 1400;

#[derive(Serialize, Deserialize)]
enum Msg {
    Ping {
        seq: u64,
        from: Member,
        updates: Vec<Member>,
    },
    PingReq {
        seq: u64,
        from: Member,
        target: SocketAddr,
        updates: Vec<Member>,
    },
    Ack {
        seq: u64,
        from: Member,
        updates: Vec<Member>,
    },
}

impl Msg {
    fn updates_mut(&mut self) -> &mut Vec<Member> {
        match self {
            Msg::Ping { updates, .. } | Msg::PingReq { updates, .. } | Msg::Ack { updates, .. } => {
                updates
            }
        }
    }

    /// Serialize, dropping piggybacked records until the datagram fits.
    fn encode(mut self) -> Option<Vec<u8>> {
        loop {
            let bytes = bincode::serialize(&self).ok()?;
            if bytes.len() <= MAX_DATAGRAM || self.updates_mut().is_empty() {
                return Some(bytes);
            }
            self.updates_mut().pop();
        }
    }
}

async fn send_raw(socket: &UdpSocket, addr: SocketAddr, msg: Msg) {
    if let Some(bytes) = msg.encode() {
        // Errors (unreachable peer, Windows ICMP resets) are indistinguishable from loss.
        let _ = socket.send_to(&bytes, addr).await;
    }
}

struct Probe {
    seq: u64,
    target: Member,
    started: Instant,
    acked: bool,
    indirect_sent: bool,
}

struct Actor {
    socket: Arc<UdpSocket>,
    table: Arc<Table>,
    cfg: GossipConfig,
    seeds: Vec<SocketAddr>,
    stats: Arc<GossipStats>,
    blocked: Arc<Mutex<HashSet<SocketAddr>>>,
    seq: u64,
    rng: Rng,
    order: Vec<NodeId>,
    pending: Option<Probe>,
    next_probe_at: Instant,
    next_join_at: Instant,
    /// relay seq -> (origin addr, origin seq, deadline)
    relays: HashMap<u64, (SocketAddr, u64, Instant)>,
}

impl Actor {
    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    async fn send(&self, addr: SocketAddr, msg: Msg) {
        let blocked = self
            .blocked
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&addr);
        if !blocked {
            send_raw(&self.socket, addr, msg).await;
        }
    }

    fn merge(&self, from: &Member, updates: &[Member], now: Instant) {
        self.table.apply(from, now);
        for u in updates {
            self.table.apply(u, now);
        }
    }

    async fn on_datagram(&mut self, buf: &[u8], src: SocketAddr, now: Instant) {
        let Ok(msg) = bincode::deserialize::<Msg>(buf) else {
            return;
        };
        match msg {
            Msg::Ping { seq, from, updates } => {
                self.merge(&from, &updates, now);
                let ack = Msg::Ack {
                    seq,
                    from: self.table.me(),
                    updates: self.table.piggyback(self.cfg.max_piggyback),
                };
                self.send(src, ack).await;
            }
            Msg::PingReq {
                seq,
                from,
                target,
                updates,
            } => {
                self.merge(&from, &updates, now);
                let relay_seq = self.next_seq();
                self.relays
                    .insert(relay_seq, (src, seq, now + self.cfg.probe_interval));
                let ping = Msg::Ping {
                    seq: relay_seq,
                    from: self.table.me(),
                    updates: self.table.piggyback(self.cfg.max_piggyback),
                };
                self.send(target, ping).await;
            }
            Msg::Ack { seq, from, updates } => {
                self.merge(&from, &updates, now);
                if let Some(p) = self.pending.as_mut().filter(|p| p.seq == seq) {
                    p.acked = true;
                } else if let Some((origin, origin_seq, _)) = self.relays.remove(&seq) {
                    let fwd = Msg::Ack {
                        seq: origin_seq,
                        from,
                        updates: self.table.piggyback(self.cfg.max_piggyback),
                    };
                    self.send(origin, fwd).await;
                }
            }
        }
    }

    fn pick_target(&mut self) -> Option<Member> {
        loop {
            if self.order.is_empty() {
                let mut ids: Vec<NodeId> = self.table.peers().into_iter().map(|m| m.id).collect();
                if ids.is_empty() {
                    return None;
                }
                self.rng.shuffle(&mut ids);
                self.order = ids;
            }
            let id = self.order.pop()?;
            if let Some(m) = self.table.get(&id) {
                if m.state != MemberState::Dead && m.id != self.table.me().id {
                    return Some(m);
                }
            }
        }
    }

    async fn on_tick(&mut self, now: Instant) {
        self.table
            .expire(now, self.cfg.suspect_timeout, self.cfg.dead_retention);
        self.relays.retain(|_, (_, _, deadline)| *deadline > now);

        // Indirect probing once the direct ack is overdue.
        let mut indirect: Option<(u64, SocketAddr, NodeId)> = None;
        if let Some(p) = self.pending.as_mut() {
            if !p.acked
                && !p.indirect_sent
                && now.saturating_duration_since(p.started) >= self.cfg.probe_timeout
            {
                p.indirect_sent = true;
                indirect = Some((p.seq, p.target.addr, p.target.id.clone()));
            }
        }
        if let Some((seq, target_addr, target_id)) = indirect {
            let mut helpers: Vec<Member> = self
                .table
                .peers()
                .into_iter()
                .filter(|m| m.id != target_id)
                .collect();
            self.rng.shuffle(&mut helpers);
            helpers.truncate(self.cfg.indirect_probes);
            self.stats
                .indirect_probes
                .fetch_add(helpers.len() as u64, Ordering::Relaxed);
            for h in helpers {
                let req = Msg::PingReq {
                    seq,
                    from: self.table.me(),
                    target: target_addr,
                    updates: self.table.piggyback(self.cfg.max_piggyback),
                };
                self.send(h.addr, req).await;
            }
        }

        if now >= self.next_probe_at {
            if let Some(p) = self.pending.take() {
                if !p.acked && self.table.mark_suspect(&p.target.id, now) {
                    self.stats.suspicions.fetch_add(1, Ordering::Relaxed);
                }
            }
            if let Some(target) = self.pick_target() {
                let seq = self.next_seq();
                let ping = Msg::Ping {
                    seq,
                    from: self.table.me(),
                    updates: self.table.piggyback(self.cfg.max_piggyback),
                };
                self.send(target.addr, ping).await;
                self.pending = Some(Probe {
                    seq,
                    target,
                    started: now,
                    acked: false,
                    indirect_sent: false,
                });
            }
            self.next_probe_at = now + self.cfg.probe_interval;
        }

        // Join / rejoin: nobody left to probe, so knock on the seeds.
        if now >= self.next_join_at && !self.seeds.is_empty() && self.table.peers().is_empty() {
            let me_addr = self.table.me().addr;
            for seed in self.seeds.clone() {
                if seed == me_addr {
                    continue;
                }
                let seq = self.next_seq();
                let ping = Msg::Ping {
                    seq,
                    from: self.table.me(),
                    updates: self.table.piggyback(self.cfg.max_piggyback),
                };
                self.send(seed, ping).await;
            }
            self.next_join_at = now + self.cfg.probe_interval;
        }
    }

    async fn run(mut self, mut shutdown: watch::Receiver<bool>) {
        let sock = Arc::clone(&self.socket);
        let mut buf = vec![0u8; 65_536];
        let tick_every = (self.cfg.probe_timeout / 2).max(Duration::from_millis(1));
        let mut tick = tokio::time::interval(tick_every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = shutdown.changed() => break,
                res = sock.recv_from(&mut buf) => match res {
                    Ok((n, src)) => self.on_datagram(&buf[..n], src, Instant::now()).await,
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
                    Err(_) => tokio::time::sleep(Duration::from_millis(5)).await,
                },
                _ = tick.tick() => self.on_tick(Instant::now()).await,
            }
        }
    }
}

/// Running gossip actor.
pub(crate) struct GossipHandle {
    pub table: Arc<Table>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub stats: Arc<GossipStats>,
    /// Test hook: datagrams to these addresses are silently dropped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub blocked: Arc<Mutex<HashSet<SocketAddr>>>,
    socket: Arc<UdpSocket>,
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

pub(crate) fn spawn_gossip(
    socket: UdpSocket,
    me: Member,
    cfg: GossipConfig,
    seeds: Vec<SocketAddr>,
) -> GossipHandle {
    let socket = Arc::new(socket);
    let seed_mix = crate::hash::hash64(me.id.as_str().as_bytes(), me.addr.port().into());
    let table = Arc::new(Table::new(me));
    let stats = Arc::new(GossipStats::default());
    let blocked = Arc::new(Mutex::new(HashSet::new()));
    let now = Instant::now();
    let actor = Actor {
        socket: Arc::clone(&socket),
        table: Arc::clone(&table),
        cfg,
        seeds,
        stats: Arc::clone(&stats),
        blocked: Arc::clone(&blocked),
        seq: 0,
        rng: Rng::new(seed_mix),
        order: Vec::new(),
        pending: None,
        next_probe_at: now,
        next_join_at: now,
        relays: HashMap::new(),
    };
    let (shutdown, rx) = watch::channel(false);
    let task = tokio::spawn(actor.run(rx));
    GossipHandle {
        table,
        stats,
        blocked,
        socket,
        shutdown,
        task,
    }
}

impl GossipHandle {
    /// Announce a voluntary leave to all live peers, then stop.
    pub(crate) async fn leave_and_stop(self) {
        let left = self.table.leave();
        for peer in self.table.peers() {
            let msg = Msg::Ping {
                seq: 0,
                from: left.clone(),
                updates: Vec::new(),
            };
            send_raw(&self.socket, peer.addr, msg).await;
        }
        let _ = self.shutdown.send(true);
        let _ = self.task.await;
    }

    /// Stop without telling anyone (simulates a crash).
    pub(crate) async fn crash(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn member(id: &str, state: MemberState, inc: u64) -> Member {
        Member {
            id: NodeId::new(id),
            addr: addr(9000),
            state,
            incarnation: inc,
        }
    }

    fn table() -> Table {
        Table::new(member("me", MemberState::Alive, 0))
    }

    fn state_of(t: &Table, id: &str) -> (MemberState, u64) {
        let m = t.get(&NodeId::new(id)).unwrap();
        (m.state, m.incarnation)
    }

    // ---------- Table: pure state machine ----------

    #[test]
    fn new_member_is_added_once() {
        let t = table();
        let now = Instant::now();
        let v0 = t.version();
        assert_eq!(
            t.apply(&member("a", MemberState::Alive, 0), now),
            Applied::Updated
        );
        assert!(t.version() > v0);
        let v1 = t.version();
        assert_eq!(
            t.apply(&member("a", MemberState::Alive, 0), now),
            Applied::Ignored
        );
        assert_eq!(t.version(), v1, "no-op merges must not bump the version");
        assert_eq!(t.snapshot().len(), 2);
        assert_eq!(t.peers().len(), 1);
    }

    #[test]
    fn precedence_follows_incarnation_then_severity() {
        use MemberState::*;
        let now = Instant::now();
        // (current, incoming, accepted?)
        let cases = [
            ((Alive, 3), (Alive, 3), false),
            ((Alive, 3), (Alive, 4), true),
            ((Alive, 3), (Suspect, 3), true),
            ((Alive, 3), (Suspect, 2), false),
            ((Alive, 3), (Dead, 3), true),
            ((Suspect, 3), (Alive, 3), false),
            ((Suspect, 3), (Alive, 4), true),
            ((Suspect, 3), (Suspect, 3), false),
            ((Suspect, 3), (Suspect, 4), true),
            ((Suspect, 3), (Dead, 3), true),
            ((Dead, 3), (Alive, 3), false),
            ((Dead, 3), (Suspect, 3), false),
            ((Dead, 3), (Alive, 4), true),
            ((Dead, 3), (Dead, 3), false),
        ];
        for ((cs, ci), (is, ii), accepted) in cases {
            let t = table();
            t.apply(&member("x", Alive, 0), now);
            // Drive the entry to the starting state.
            assert_eq!(t.apply(&member("x", cs, ci), now), Applied::Updated);
            let got = t.apply(&member("x", is, ii), now);
            assert_eq!(
                got == Applied::Updated,
                accepted,
                "{cs:?}@{ci} <- {is:?}@{ii}"
            );
            let expect = if accepted { (is, ii) } else { (cs, ci) };
            assert_eq!(state_of(&t, "x"), expect, "{cs:?}@{ci} <- {is:?}@{ii}");
        }
    }

    #[test]
    fn accusation_about_self_is_refuted_with_higher_incarnation() {
        let t = table();
        let now = Instant::now();
        assert_eq!(
            t.apply(&member("me", MemberState::Suspect, 0), now),
            Applied::Refuted
        );
        assert_eq!(t.me().incarnation, 1);
        assert_eq!(t.me().state, MemberState::Alive);
        // Stale accusation: already refuted.
        assert_eq!(
            t.apply(&member("me", MemberState::Suspect, 0), now),
            Applied::Ignored
        );
        assert_eq!(t.me().incarnation, 1);
        // Dead rumour at the current incarnation also forces a bump.
        assert_eq!(
            t.apply(&member("me", MemberState::Dead, 1), now),
            Applied::Refuted
        );
        assert_eq!(t.me().incarnation, 2);
        // Alive claims about self never change anything.
        assert_eq!(
            t.apply(&member("me", MemberState::Alive, 5), now),
            Applied::Ignored
        );
        assert_eq!(t.me().incarnation, 2);
    }

    #[test]
    fn mark_suspect_only_demotes_alive_peers() {
        let t = table();
        let now = Instant::now();
        t.apply(&member("a", MemberState::Alive, 2), now);
        assert!(t.mark_suspect(&NodeId::new("a"), now));
        assert_eq!(state_of(&t, "a"), (MemberState::Suspect, 2));
        assert!(!t.mark_suspect(&NodeId::new("a"), now), "already suspect");
        assert!(!t.mark_suspect(&NodeId::new("ghost"), now));
        assert!(
            !t.mark_suspect(&NodeId::new("me"), now),
            "never suspect self"
        );
    }

    #[test]
    fn suspect_becomes_dead_after_timeout_and_tombstone_is_reaped() {
        let t = table();
        let t0 = Instant::now();
        let st = Duration::from_millis(100);
        let keep = Duration::from_secs(10);
        t.apply(&member("a", MemberState::Alive, 0), t0);
        t.apply(&member("b", MemberState::Alive, 0), t0);
        t.mark_suspect(&NodeId::new("a"), t0);

        assert!(t
            .expire(t0 + Duration::from_millis(99), st, keep)
            .is_empty());
        assert_eq!(state_of(&t, "a").0, MemberState::Suspect);

        let died = t.expire(t0 + st, st, keep);
        assert_eq!(died, vec![NodeId::new("a")]);
        assert_eq!(state_of(&t, "a"), (MemberState::Dead, 0));
        assert_eq!(
            state_of(&t, "b").0,
            MemberState::Alive,
            "alive peers untouched"
        );
        assert!(t.peers().iter().all(|m| m.id != NodeId::new("a")));

        // Not reported twice.
        assert!(t.expire(t0 + st * 2, st, keep).is_empty());
        // Tombstone is reaped after retention; self is never reaped.
        t.expire(t0 + st + keep, st, keep);
        assert!(t.get(&NodeId::new("a")).is_none());
        assert!(t.get(&NodeId::new("me")).is_some());
    }

    #[test]
    fn piggyback_sends_fresh_updates_first_and_cycles_through_the_table() {
        let t = table();
        let now = Instant::now();
        for i in 0..10 {
            t.apply(&member(&format!("old{i}"), MemberState::Alive, 0), now);
        }
        // Exhaust the "freshness" of everything so far.
        for _ in 0..20 {
            t.piggyback(4);
        }
        t.apply(&member("fresh", MemberState::Alive, 0), now);
        let first = t.piggyback(3);
        assert_eq!(first.len(), 3);
        assert!(
            first.iter().any(|m| m.id == NodeId::new("fresh")),
            "fresh update goes first"
        );

        // Anti-entropy: repeated small piggybacks eventually cover every member.
        let mut seen = HashSet::new();
        for _ in 0..30 {
            for m in t.piggyback(3) {
                seen.insert(m.id);
            }
        }
        assert_eq!(seen.len(), 12, "all rows incl. self must circulate");
        assert!(t.piggyback(0).is_empty());
        assert_eq!(t.piggyback(100).len(), 12);
    }

    #[test]
    fn leave_declares_self_dead_at_a_higher_incarnation() {
        let t = table();
        let now = Instant::now();
        let m = t.leave();
        assert_eq!(m.state, MemberState::Dead);
        assert_eq!(m.incarnation, 1);
        // Another table, which knew us Alive@0, accepts it.
        let other = Table::new(member("other", MemberState::Alive, 0));
        other.apply(&member("me", MemberState::Alive, 0), now);
        assert_eq!(other.apply(&m, now), Applied::Updated);
        assert_eq!(state_of(&other, "me"), (MemberState::Dead, 1));
        // After leaving, we do not refute our own departure.
        assert_eq!(
            t.apply(&member("me", MemberState::Dead, 1), now),
            Applied::Ignored
        );
    }

    // ---------- Gossip actor over real UDP on loopback ----------

    fn fast() -> GossipConfig {
        GossipConfig::fast()
    }

    async fn spawn_node(name: &str, seeds: Vec<SocketAddr>, cfg: GossipConfig) -> GossipHandle {
        let sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let me = Member {
            id: NodeId::new(name),
            addr: sock.local_addr().unwrap(),
            state: MemberState::Alive,
            incarnation: 0,
        };
        spawn_gossip(sock, me, cfg, seeds)
    }

    async fn eventually(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) -> Duration {
        let start = Instant::now();
        while !f() {
            assert!(start.elapsed() < timeout, "timed out waiting for: {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        start.elapsed()
    }

    fn alive_count(h: &GossipHandle) -> usize {
        h.table
            .snapshot()
            .iter()
            .filter(|m| m.state == MemberState::Alive)
            .count()
    }

    fn sees(h: &GossipHandle, id: &str, state: MemberState) -> bool {
        h.table
            .get(&NodeId::new(id))
            .is_some_and(|m| m.state == state)
    }

    async fn trio(cfg: GossipConfig) -> (GossipHandle, GossipHandle, GossipHandle) {
        let a = spawn_node("a", vec![], cfg.clone()).await;
        let seed = a.table.me().addr;
        let b = spawn_node("b", vec![seed], cfg.clone()).await;
        let c = spawn_node("c", vec![seed], cfg).await;
        eventually("3-node convergence", Duration::from_secs(10), || {
            alive_count(&a) == 3 && alive_count(&b) == 3 && alive_count(&c) == 3
        })
        .await;
        (a, b, c)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn three_nodes_converge_through_a_single_seed() {
        let (a, b, c) = trio(fast()).await;
        for h in [&a, &b, &c] {
            let mut ids: Vec<String> = h.table.snapshot().into_iter().map(|m| m.id.0).collect();
            ids.sort();
            assert_eq!(ids, ["a", "b", "c"]);
        }
        a.crash().await;
        b.crash().await;
        c.crash().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crashed_node_is_marked_dead_by_all_survivors_within_bound() {
        let (a, b, c) = trio(fast()).await;
        let start = Instant::now();
        c.crash().await;
        eventually("c dead on a and b", Duration::from_secs(15), || {
            sees(&a, "c", MemberState::Dead) && sees(&b, "c", MemberState::Dead)
        })
        .await;
        let took = start.elapsed();
        eprintln!("crash detected by all survivors in {took:?}");
        assert!(sees(&a, "b", MemberState::Alive) && sees(&b, "a", MemberState::Alive));
        a.crash().await;
        b.crash().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn graceful_leave_propagates_without_waiting_for_failure_detection() {
        let mut cfg = fast();
        cfg.suspect_timeout = Duration::from_secs(60); // detection could not finish in time
        let (a, b, c) = trio(cfg).await;
        c.leave_and_stop().await;
        eventually("leave seen", Duration::from_secs(5), || {
            sees(&a, "c", MemberState::Dead) && sees(&b, "c", MemberState::Dead)
        })
        .await;
        a.crash().await;
        b.crash().await;
    }

    /// a cannot reach c directly (both directions), b can reach both.
    async fn partially_partitioned(mut cfg: GossipConfig) -> u64 {
        // Roomier timings than `fast()`: a spurious scheduler-induced suspicion must not
        // be mistaken for a missing indirect probe.
        cfg.probe_interval = Duration::from_millis(100);
        cfg.probe_timeout = Duration::from_millis(60);
        let (a, b, c) = trio(cfg).await;
        let (aa, ca) = (a.table.me().addr, c.table.me().addr);
        a.blocked.lock().unwrap().insert(ca);
        c.blocked.lock().unwrap().insert(aa);
        tokio::time::sleep(Duration::from_millis(2000)).await;
        let sus: u64 = [&a, &b, &c]
            .iter()
            .map(|h| h.stats.suspicions.load(Ordering::Relaxed))
            .sum();
        if sus == 0 {
            assert_eq!(alive_count(&a), 3);
            assert_eq!(alive_count(&c), 3);
        }
        a.crash().await;
        b.crash().await;
        c.crash().await;
        sus
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn indirect_probes_keep_a_partially_partitioned_peer_unsuspected() {
        assert_eq!(partially_partitioned(fast()).await, 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_indirect_probes_the_same_partition_causes_suspicion() {
        let mut cfg = fast();
        cfg.indirect_probes = 0;
        assert!(
            partially_partitioned(cfg).await > 0,
            "control: partition must be observable"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wrongly_declared_dead_node_refutes_after_the_partition_heals() {
        let (a, b, c) = trio(fast()).await;
        let (aa, ba, ca) = (a.table.me().addr, b.table.me().addr, c.table.me().addr);
        for h in [&a, &b] {
            h.blocked.lock().unwrap().insert(ca);
        }
        c.blocked.lock().unwrap().extend([aa, ba]);
        eventually("c declared dead", Duration::from_secs(15), || {
            sees(&a, "c", MemberState::Dead) && sees(&b, "c", MemberState::Dead)
        })
        .await;

        for h in [&a, &b] {
            h.blocked.lock().unwrap().clear();
        }
        c.blocked.lock().unwrap().clear();
        eventually(
            "c refutes and is alive again",
            Duration::from_secs(15),
            || {
                [&a, &b].iter().all(|h| {
                    h.table
                        .get(&NodeId::new("c"))
                        .is_some_and(|m| m.state == MemberState::Alive && m.incarnation >= 1)
                })
            },
        )
        .await;
        a.crash().await;
        b.crash().await;
        c.crash().await;
    }
}
