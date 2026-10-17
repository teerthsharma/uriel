use crate::durable::{Disk, FileStorage};
use crate::raft::{
    Body, ConfChange, ConfError, Entry, EntryKind, Index, MemStorage, Message, NodeId, NotLeader,
    Raft, ReadError, ReadState, Role, Storage, StorageError, Term,
};
use crate::rng::Rng;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::rc::Rc;

#[derive(Debug)]
enum Pend {
    Append(Vec<u8>),
    Truncate(u64),
}

fn apply_op(v: &mut Vec<u8>, op: &Pend) {
    match op {
        Pend::Append(b) => v.extend_from_slice(b),
        Pend::Truncate(n) => v.truncate(*n as usize),
    }
}

#[derive(Default)]
struct DiskInner {
    durable: BTreeMap<String, Vec<u8>>,
    pending: BTreeMap<String, Vec<Pend>>,
    ops: u64,
    fail_at: Option<u64>,
    dead: bool,
}

impl DiskInner {
    fn clean(&self, file: &str) -> bool {
        self.pending.get(file).is_none_or(|p| p.is_empty())
    }

    fn view(&self, file: &str) -> Vec<u8> {
        let mut v = self.durable.get(file).cloned().unwrap_or_default();
        if let Some(p) = self.pending.get(file) {
            for op in p {
                apply_op(&mut v, op);
            }
        }
        v
    }

    fn op(&mut self) -> Result<(), StorageError> {
        if self.dead {
            return Err(StorageError::Io("disk dead".into()));
        }
        if self.fail_at == Some(self.ops) {
            self.dead = true;
            return Err(StorageError::Io("injected failure".into()));
        }
        self.ops += 1;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct SimDisk(Rc<RefCell<DiskInner>>);

impl SimDisk {
    pub fn ops(&self) -> u64 {
        self.0.borrow().ops
    }

    pub fn fail_at(&self, at: Option<u64>) {
        let mut s = self.0.borrow_mut();
        s.fail_at = at;
        s.ops = 0;
        s.dead = false;
    }

    pub fn file(&self, file: &str) -> Vec<u8> {
        self.0.borrow().view(file)
    }

    pub fn set_file(&self, file: &str, bytes: Vec<u8>) {
        let mut s = self.0.borrow_mut();
        s.pending.remove(file);
        s.durable.insert(file.to_string(), bytes);
    }

    pub fn crash_drop(&self) {
        let mut s = self.0.borrow_mut();
        s.pending.clear();
        s.dead = false;
        s.fail_at = None;
    }

    pub fn crash(&self, rng: &mut Rng) {
        let mut s = self.0.borrow_mut();
        let files: Vec<String> = s.pending.keys().cloned().collect();
        for f in files {
            let ops = s.pending.remove(&f).unwrap_or_default();
            let keep = rng.between(0, ops.len() as u64) as usize;
            let mut v = s.durable.remove(&f).unwrap_or_default();
            for (i, op) in ops.into_iter().enumerate().take(keep) {
                match op {
                    Pend::Append(b) if i + 1 == keep && rng.chance(1, 2) => {
                        let cut = rng.below(b.len() as u64 + 1) as usize;
                        v.extend_from_slice(&b[..cut]);
                    }
                    other => apply_op(&mut v, &other),
                }
            }
            s.durable.insert(f, v);
        }
        s.dead = false;
        s.fail_at = None;
    }
}

impl Disk for SimDisk {
    fn size(&self, file: &str) -> Result<u64, StorageError> {
        let s = self.0.borrow();
        if s.clean(file) {
            return Ok(s.durable.get(file).map_or(0, |v| v.len() as u64));
        }
        Ok(s.view(file).len() as u64)
    }

    fn read_at(&self, file: &str, offset: u64, len: usize) -> Result<Vec<u8>, StorageError> {
        let s = self.0.borrow();
        let lo = offset as usize;
        let short = || StorageError::Io("short read".into());
        if s.clean(file) {
            let v = s.durable.get(file).map(Vec::as_slice).unwrap_or(&[]);
            return v.get(lo..lo + len).map(<[u8]>::to_vec).ok_or_else(short);
        }
        s.view(file).get(lo..lo + len).map(<[u8]>::to_vec).ok_or_else(short)
    }

    fn append(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let mut s = self.0.borrow_mut();
        s.op()?;
        s.pending.entry(file.to_string()).or_default().push(Pend::Append(bytes.to_vec()));
        Ok(())
    }

    fn truncate(&mut self, file: &str, len: u64) -> Result<(), StorageError> {
        let mut s = self.0.borrow_mut();
        s.op()?;
        s.pending.entry(file.to_string()).or_default().push(Pend::Truncate(len));
        Ok(())
    }

    fn sync(&mut self, file: &str) -> Result<(), StorageError> {
        let mut s = self.0.borrow_mut();
        s.op()?;
        let v = s.view(file);
        s.pending.remove(file);
        s.durable.insert(file.to_string(), v);
        Ok(())
    }

    fn replace(&mut self, file: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let mut s = self.0.borrow_mut();
        s.op()?;
        s.pending.remove(file);
        s.durable.insert(file.to_string(), bytes.to_vec());
        Ok(())
    }
}

pub fn ids(n: u64) -> Vec<NodeId> {
    (1..=n).map(NodeId).collect()
}

pub fn slot(id: NodeId) -> usize {
    id.0 as usize - 1
}

pub fn encode_state(entries: &[Entry]) -> Vec<u8> {
    let mut v = Vec::new();
    for e in entries {
        v.extend(e.term.0.to_le_bytes());
        v.extend(e.index.0.to_le_bytes());
        v.push(u8::from(e.kind == EntryKind::Conf));
        v.extend((e.data.len() as u32).to_le_bytes());
        v.extend(&e.data);
    }
    v
}

pub fn decode_state(b: &[u8]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let term = u64::from_le_bytes(b[at..at + 8].try_into().unwrap_or_default());
        let index = u64::from_le_bytes(b[at + 8..at + 16].try_into().unwrap_or_default());
        let kind = if b[at + 16] == 1 { EntryKind::Conf } else { EntryKind::Normal };
        let len = u32::from_le_bytes(b[at + 17..at + 21].try_into().unwrap_or_default()) as usize;
        let data = b[at + 21..at + 21 + len].to_vec();
        out.push(Entry { term: Term(term), index: Index(index), data, kind });
        at += 21 + len;
    }
    out
}

pub fn members_of<S: Storage>(r: &Raft<S>) -> Vec<u64> {
    r.members().iter().map(|m| m.0).collect()
}

pub struct Pump<S: Storage = MemStorage> {
    pub nodes: Vec<Raft<S>>,
    pub alive: Vec<bool>,
    pub restarted: Vec<bool>,
    pub boot: Vec<NodeId>,
    pub reads: Vec<Vec<ReadState>>,
    pub snapshots_sent: usize,
    pub snap_installs: Vec<u64>,
    pub isolated: BTreeSet<NodeId>,
    pub queue: VecDeque<Message>,
    pub leaders_by_term: BTreeMap<Term, NodeId>,
    pub applied: Vec<Vec<Entry>>,
    pub chaos: Option<Rng>,
}

impl Pump<MemStorage> {
    pub fn new(n: u64, seed: u64) -> Self {
        Pump::with_storages((0..n).map(|_| MemStorage::new()).collect(), seed)
    }

    pub fn with_members(total: u64, members: u64, seed: u64) -> Self {
        let storages = (0..total).map(|_| MemStorage::new()).collect();
        Pump::with_boot(storages, ids(members), seed)
    }
}

impl<S: Storage> Pump<S> {
    pub fn with_storages(storages: Vec<S>, seed: u64) -> Self {
        let boot = ids(storages.len() as u64);
        Pump::with_boot(storages, boot, seed)
    }

    pub fn try_make(
        id: NodeId,
        boot: &[NodeId],
        storage: S,
        seed: u64,
    ) -> Result<Raft<S>, StorageError> {
        if boot.contains(&id) {
            Raft::new(id, boot.to_vec(), storage, seed)
        } else {
            Raft::joining(id, boot.to_vec(), storage, seed)
        }
    }

    pub fn with_boot(storages: Vec<S>, boot: Vec<NodeId>, seed: u64) -> Self {
        let all = ids(storages.len() as u64);
        let nodes: Vec<_> = storages
            .into_iter()
            .zip(all.iter())
            .map(|(s, id)| Pump::try_make(*id, &boot, s, seed).expect("fresh storage opens"))
            .collect();
        let n = nodes.len();
        Pump {
            nodes,
            alive: vec![true; n],
            restarted: vec![false; n],
            boot,
            reads: vec![Vec::new(); n],
            snapshots_sent: 0,
            snap_installs: vec![0; n],
            isolated: BTreeSet::new(),
            queue: VecDeque::new(),
            leaders_by_term: BTreeMap::new(),
            applied: vec![Vec::new(); n],
            chaos: None,
        }
    }

    pub fn restart_checked(&mut self, i: usize, storage: S, seed: u64) -> Result<(), StorageError> {
        let boot = self.boot.clone();
        self.nodes[i] = Pump::try_make(NodeId(i as u64 + 1), &boot, storage, seed)?;
        self.alive[i] = true;
        self.restarted[i] = true;
        Ok(())
    }

    pub fn restart(&mut self, i: usize, storage: S, seed: u64) {
        self.restart_checked(i, storage, seed).expect("storage reopens");
    }

    pub fn reachable(&self, a: NodeId, b: NodeId) -> bool {
        self.alive[slot(a)]
            && self.alive[slot(b)]
            && !self.isolated.contains(&a)
            && !self.isolated.contains(&b)
    }

    pub fn observe(&mut self) {
        for (i, node) in self.nodes.iter().enumerate() {
            if self.alive[i] && node.role() == Role::Leader {
                let first = *self.leaders_by_term.entry(node.term()).or_insert(node.id());
                assert_eq!(first, node.id(), "two leaders in term {:?}", node.term());
            }
        }
        self.check_logs();
    }

    pub fn check_logs(&self) {
        let n = self.nodes.len();
        for i in 0..n {
            let node = &self.nodes[i];
            assert!(
                node.commit_index() <= node.storage().last_index(),
                "node {} commit beyond its log",
                i + 1
            );
            for j in (i + 1)..n {
                let a = self.nodes[i].storage();
                let b = self.nodes[j].storage();
                let shared = a.last_index().min(b.last_index()).0;
                let floor = a.first_index().0.max(b.first_index().0) - 1;
                if let Some(k) = (floor..=shared)
                    .rev()
                    .find(|k| a.term_at(Index(*k)) == b.term_at(Index(*k)))
                {
                    for m in floor..=k {
                        assert_eq!(
                            a.term_at(Index(m)),
                            b.term_at(Index(m)),
                            "log matching broken between {} and {} at {}",
                            i + 1,
                            j + 1,
                            m
                        );
                    }
                }
                let c = self.applied[i].len().min(self.applied[j].len());
                assert_eq!(
                    self.applied[i][..c],
                    self.applied[j][..c],
                    "nodes {} and {} applied different data",
                    i + 1,
                    j + 1
                );
            }
        }
        let top = self.nodes.iter().map(|n| n.term()).max().unwrap_or_default();
        let longest = (0..n).max_by_key(|i| self.applied[*i].len()).unwrap_or(0);
        for (i, node) in self.nodes.iter().enumerate() {
            if self.alive[i] && node.role() == Role::Leader && node.term() == top {
                for e in self.applied[longest]
                    .iter()
                    .filter(|e| e.index.0 + 1 >= node.storage().first_index().0)
                {
                    assert_eq!(
                        node.storage().term_at(e.index),
                        Some(e.term),
                        "leader {} lost committed index {}",
                        i + 1,
                        e.index.0
                    );
                }
            }
        }
    }

    pub fn collect_checked(&mut self) -> Result<(), String> {
        for (i, node) in self.nodes.iter_mut().enumerate() {
            if !self.alive[i] {
                continue;
            }
            let ready = node.ready();
            if node.fault().is_some() {
                self.alive[i] = false;
                continue;
            }
            self.queue.extend(ready.messages.iter().cloned());
            self.snapshots_sent +=
                ready.messages.iter().filter(|m| matches!(m.body, Body::Snapshot { .. })).count();
            self.reads[i].extend(ready.read_states.iter().copied());
            if let Some(snap) = &ready.snapshot {
                self.applied[i] = decode_state(&snap.data);
                self.snap_installs[i] += 1;
                if self.applied[i].len() as u64 != snap.meta.index.0 {
                    return Err(format!("node {} snapshot state does not match its index", i + 1));
                }
            }
            for e in &ready.committed_entries {
                let have = self.applied[i].len();
                if self.restarted[i] && e.index.0 as usize <= have {
                    if self.applied[i][e.index.0 as usize - 1] != *e {
                        return Err(format!("node {} redelivered a different entry at {}", i + 1, e.index.0));
                    }
                } else if e.index.0 as usize != have + 1 {
                    return Err(format!(
                        "node {} delivery gap or repeat: got {} after {have}",
                        i + 1,
                        e.index.0
                    ));
                } else {
                    self.applied[i].push(e.clone());
                }
            }
            node.advance(ready);
        }
        Ok(())
    }

    pub fn collect(&mut self) {
        self.collect_checked().expect("delivery is consistent");
    }

    pub fn next_message(&mut self) -> Option<Message> {
        let len = self.queue.len();
        if len == 0 {
            return None;
        }
        let pick = match self.chaos.as_mut() {
            Some(r) => {
                if r.chance(1, 2) {
                    r.below(len as u64) as usize
                } else {
                    0
                }
            }
            None => 0,
        };
        let m = self.queue.remove(pick)?;
        if let Some(r) = self.chaos.as_mut() {
            if r.chance(1, 10) {
                self.queue.push_back(m.clone());
            }
        }
        Some(m)
    }

    pub fn compact_node(&mut self, i: usize) -> Result<(), StorageError> {
        let index = Index(self.applied[i].len() as u64);
        let data = encode_state(&self.applied[i]);
        self.nodes[i].compact(index, data)
    }

    pub fn read_index(&mut self, id: NodeId, ctx: u64) -> Result<(), ReadError> {
        let r = self.nodes[slot(id)].read_index(ctx);
        self.drain();
        r
    }

    pub fn propose_conf(&mut self, id: NodeId, change: ConfChange) -> Result<Index, ConfError> {
        let r = self.nodes[slot(id)].propose_conf_change(change);
        self.observe();
        self.drain();
        r
    }

    pub fn propose(&mut self, id: NodeId, data: Vec<u8>) -> Result<Index, NotLeader> {
        let r = self.nodes[slot(id)].propose(data);
        self.observe();
        self.drain();
        r
    }

    pub fn settle(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.tick();
        }
    }

    pub fn applied_data(&self, i: usize) -> Vec<Vec<u8>> {
        self.applied[i]
            .iter()
            .filter(|e| e.kind == EntryKind::Normal && !e.data.is_empty())
            .map(|e| e.data.clone())
            .collect()
    }

    pub fn drain(&mut self) {
        self.collect();
        let mut steps = 0;
        while let Some(m) = self.next_message() {
            steps += 1;
            assert!(steps < 100_000, "message storm");
            if self.reachable(m.from, m.to) {
                let i = slot(m.to);
                self.nodes[i].step(m);
                self.observe();
            }
            self.collect();
        }
    }

    pub fn tick(&mut self) {
        for i in 0..self.nodes.len() {
            if self.alive[i] {
                self.nodes[i].tick();
            }
        }
        self.observe();
        self.drain();
    }

    pub fn tick_only(&mut self, id: NodeId) {
        self.nodes[slot(id)].tick();
        self.observe();
        self.drain();
    }

    pub fn leader(&self) -> Option<NodeId> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                self.alive[*i] && !self.isolated.contains(&n.id()) && n.role() == Role::Leader
            })
            .max_by_key(|(_, n)| n.term())
            .map(|(_, n)| n.id())
    }

    pub fn run_until_leader(&mut self, max_ticks: u32) -> Option<NodeId> {
        for _ in 0..max_ticks {
            self.tick();
            if let Some(l) = self.leader() {
                return Some(l);
            }
        }
        None
    }
}

pub trait Durability {
    type S: Storage;
    fn open(&mut self, slot: usize) -> Self::S;
    fn recover(
        &mut self,
        slot: usize,
        old: &Self::S,
        rng: &mut Rng,
    ) -> Result<Self::S, StorageError>;
    fn arm_fault(&mut self, slot: usize, after_ops: u64);
}

#[derive(Debug, Default)]
pub struct MemDurability;

impl Durability for MemDurability {
    type S = MemStorage;

    fn open(&mut self, _slot: usize) -> MemStorage {
        MemStorage::new()
    }

    fn recover(&mut self, _slot: usize, old: &MemStorage, _rng: &mut Rng) -> Result<MemStorage, StorageError> {
        Ok(old.clone())
    }

    fn arm_fault(&mut self, _slot: usize, _after_ops: u64) {}
}

pub struct DiskDurability {
    disks: Vec<SimDisk>,
    cache: usize,
}

impl DiskDurability {
    pub fn new(nodes: usize, cache: usize) -> Self {
        DiskDurability { disks: (0..nodes).map(|_| SimDisk::default()).collect(), cache }
    }
}

impl Durability for DiskDurability {
    type S = FileStorage<SimDisk>;

    fn open(&mut self, slot: usize) -> Self::S {
        FileStorage::open(self.disks[slot].clone(), self.cache).expect("a blank disk opens")
    }

    fn recover(
        &mut self,
        slot: usize,
        _old: &Self::S,
        rng: &mut Rng,
    ) -> Result<Self::S, StorageError> {
        self.disks[slot].crash(rng);
        FileStorage::open(self.disks[slot].clone(), self.cache)
    }

    fn arm_fault(&mut self, slot: usize, after_ops: u64) {
        self.disks[slot].fail_at(Some(after_ops));
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SimConfig {
    pub seed: u64,
    pub steps: u64,
    pub nodes: u64,
    pub members: u64,
    pub membership: bool,
    pub calm: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub seed: u64,
    pub step: u64,
    pub what: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "violation at seed {} step {}: {}", self.seed, self.step, self.what)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub trace: u64,
    pub committed: u64,
    pub reads: u64,
    pub restarts: u64,
    pub conf_changes: u64,
    pub snapshots: u64,
    pub partitions: u64,
    pub disk_faults: u64,
    pub terms_led: u64,
}

const DROP_PER_MILLE: u64 = 80;
const DUP_PER_MILLE: u64 = 30;
const MAX_DELAY: u64 = 5;
const LONG_DELAY: u64 = 60;
const STALE_LEADER_TICKS: u64 = 110;
const HEAL_STEPS: u64 = 1500;
const HEAL_PROOF_STEPS: u64 = 400;
const LOG_WINDOW: u64 = 96;
const FULL_CHECK_EVERY: u64 = 128;

fn quorum_size(members: usize) -> usize {
    members / 2 + 1
}

fn digest(e: &Entry) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in e.data.iter().chain(&[u8::from(e.kind == EntryKind::Conf)]) {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

fn msg_key(m: &Message) -> u64 {
    let (tag, a, b) = match &m.body {
        Body::PreVote { last_index, last_term } => (1, last_index.0, last_term.0),
        Body::PreVoteResp { granted } => (2, u64::from(*granted), 0),
        Body::Vote { last_index, last_term } => (3, last_index.0, last_term.0),
        Body::VoteResp { granted } => (4, u64::from(*granted), 0),
        Body::Append { prev_index, prev_term, entries, commit } => {
            (5, prev_index.0 ^ ((entries.len() as u64) << 40), commit.0 ^ (prev_term.0 << 32))
        }
        Body::AppendResp { success, match_index, hint } => {
            (6, match_index.0 | (u64::from(*success) << 63), hint.0)
        }
        Body::Heartbeat { commit, seq } => (7, commit.0, *seq),
        Body::HeartbeatResp { seq } => (8, *seq, 0),
        Body::Snapshot { meta, .. } => (9, meta.index.0, meta.term.0),
    };
    let mut h = m.from.0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for x in [m.to.0, m.term.0, tag, a, b] {
        h = (h ^ x).wrapping_mul(0x100_0000_01b3).rotate_left(13);
    }
    h
}

struct Flight {
    due: u64,
    msg: Message,
}

struct Sim<D: Durability> {
    cfg: SimConfig,
    rng: Rng,
    dur: D,
    pump: Pump<D::S>,
    net: Vec<Flight>,
    step: u64,
    group: Vec<u8>,
    down_until: Vec<Option<u64>>,
    committed: BTreeMap<u64, (Term, u64)>,
    verified: Vec<usize>,
    verified_gen: Vec<u64>,
    pending_reads: BTreeMap<u64, u64>,
    iso: Vec<u64>,
    lead_mark: Vec<(Term, u64)>,
    promised: Vec<(Term, Option<NodeId>)>,
    lead_base: Vec<(Term, u64)>,
    crash_after: Vec<usize>,
    trace: u64,
    ctx: u64,
    counter: u64,
    quiet: bool,
    report: Report,
}

type Check = Result<(), Violation>;

pub fn run<D: Durability>(cfg: SimConfig, mut dur: D) -> Result<Report, Violation> {
    let n = cfg.nodes as usize;
    let storages: Vec<D::S> = (0..n).map(|i| dur.open(i)).collect();
    for s in 0..n {
        dur.arm_fault(s, u64::MAX);
    }
    let mut pump = Pump::with_boot(storages, ids(cfg.members), cfg.seed);
    pump.nodes.iter_mut().for_each(|n| n.set_check_quorum(true));
    let mut sim = Sim {
        cfg,
        rng: Rng::new(cfg.seed ^ 0x51D_0000_0000_0000),
        dur,
        pump,
        net: Vec::new(),
        step: 0,
        group: vec![0; n],
        down_until: vec![None; n],
        committed: BTreeMap::new(),
        verified: vec![0; n],
        verified_gen: vec![0; n],
        pending_reads: BTreeMap::new(),
        iso: vec![0; n],
        lead_mark: vec![(Term(u64::MAX), 0); n],
        promised: vec![(Term(0), None); n],
        lead_base: vec![(Term(u64::MAX), 0); n],
        crash_after: Vec::new(),
        trace: 0xcbf2_9ce4_8422_2325,
        ctx: 0,
        counter: 0,
        quiet: false,
        report: Report::default(),
    };
    for _ in 0..cfg.steps {
        sim.one_step()?;
    }
    sim.heal()?;
    sim.report.trace = sim.trace;
    sim.report.committed = sim.committed.len() as u64;
    sim.report.snapshots = sim.pump.snapshots_sent as u64;
    sim.report.terms_led = sim.pump.leaders_by_term.len() as u64;
    Ok(sim.report)
}

impl<D: Durability> Sim<D> {
    fn bad<T>(&self, what: String) -> Result<T, Violation> {
        Err(Violation { seed: self.cfg.seed, step: self.step, what })
    }

    fn mix(&mut self, x: u64) {
        self.trace = (self.trace ^ x).wrapping_mul(0x100_0000_01b3).rotate_left(7);
    }

    fn nodes(&self) -> usize {
        self.cfg.nodes as usize
    }

    fn pick_leader(&mut self) -> Option<usize> {
        let leaders: Vec<usize> = (0..self.nodes())
            .filter(|i| self.pump.alive[*i] && self.pump.nodes[*i].role() == Role::Leader)
            .collect();
        if leaders.is_empty() {
            return None;
        }
        Some(leaders[self.rng.below(leaders.len() as u64) as usize])
    }

    fn one_step(&mut self) -> Check {
        self.step += 1;
        let n = self.nodes();
        let split = self.group.iter().any(|g| *g != self.group[0]);
        if !self.quiet {
            if !split && self.rng.chance(1, 250) {
                for g in self.group.iter_mut() {
                    *g = self.rng.below(2) as u8;
                }
                self.report.partitions += 1;
            } else if split && self.rng.chance(1, 450) {
                self.group.iter_mut().for_each(|g| *g = 0);
            }
            if self.rng.chance(1, 400 * self.cfg.calm) {
                let i = self.rng.below(n as u64) as usize;
                self.crash(i);
            }
            if self.rng.chance(1, 300) {
                let i = self.rng.below(n as u64) as usize;
                let after = self.rng.below(4);
                if self.pump.alive[i] {
                    self.dur.arm_fault(i, after);
                }
            }
            if self.rng.chance(1, 120) {
                let i = self.rng.below(n as u64) as usize;
                if self.pump.alive[i] {
                    let _ = self.pump.compact_node(i);
                }
            }
        }
        for i in 0..n {
            if self.down_until[i].is_some_and(|t| t <= self.step) {
                self.restart(i)?;
            }
        }
        let roll = self.rng.below(1000);
        self.mix(roll);
        match roll {
            0..=199 => self.tick_nodes(),
            200..=789 => self.deliver(),
            790..=869 => self.propose(),
            870..=909 => self.read(),
            910..=929 if self.cfg.membership => self.change_membership(),
            _ => self.deliver(),
        }
        self.collect()?;
        for i in std::mem::take(&mut self.crash_after) {
            self.crash(i);
        }
        self.check()
    }

    fn crash(&mut self, i: usize) {
        if !self.pump.alive[i] {
            return;
        }
        self.pump.alive[i] = false;
        let wait = self.rng.between(3, 80);
        self.down_until[i] = Some(self.step + wait);
    }

    fn restart(&mut self, i: usize) -> Check {
        let seed = self.rng.next_u64();
        let new = match self.dur.recover(i, self.pump.nodes[i].storage(), &mut self.rng) {
            Ok(s) => s,
            Err(e) => return self.bad(format!("node {} cannot reopen its storage: {e}", i + 1)),
        };
        if let Err(e) = self.pump.restart_checked(i, new, seed) {
            return self.bad(format!("node {} rejects its recovered state: {e}", i + 1));
        }
        self.pump.nodes[i].set_check_quorum(true);
        let (rt, rv) = self.promised[i];
        let (nt, nv) = (self.pump.nodes[i].term(), self.pump.nodes[i].vote());
        if nt < rt || (nt == rt && rv.is_some() && nv != rv) {
            return self.bad(format!(
                "node {} forgot a released promise: had term {} vote {:?}, recovered term {} vote {:?}",
                i + 1,
                rt.0,
                rv.map(|v| v.0),
                nt.0,
                nv.map(|v| v.0)
            ));
        }
        self.down_until[i] = None;
        self.iso[i] = 0;
        self.lead_mark[i] = (Term(u64::MAX), 0);
        self.lead_base[i] = (Term(u64::MAX), 0);
        self.report.restarts += 1;
        let st = self.pump.nodes[i].storage();
        let lo = st.first_index().0.saturating_sub(1).max(1);
        let hi = self.pump.nodes[i].commit_index().0;
        if lo <= hi {
            for (idx, (term, _)) in self.committed.range(lo..=hi) {
                if st.term_at(Index(*idx)) != Some(*term) {
                    return self.bad(format!(
                        "node {} restarted without committed entry {idx} (term {})",
                        i + 1,
                        term.0
                    ));
                }
            }
        }
        Ok(())
    }

    fn tick_nodes(&mut self) {
        for i in 0..self.nodes() {
            if !self.pump.alive[i] || (!self.quiet && !self.rng.chance(7, 8)) {
                continue;
            }
            self.pump.nodes[i].tick();
            let node = &self.pump.nodes[i];
            if node.role() == Role::Leader {
                let reach = node
                    .members()
                    .iter()
                    .filter(|m| {
                        let s = slot(**m);
                        s < self.group.len() && self.pump.alive[s] && self.group[s] == self.group[i]
                    })
                    .count();
                if reach < quorum_size(node.members().len()) {
                    self.iso[i] += 1;
                } else {
                    self.iso[i] = 0;
                }
            } else {
                self.iso[i] = 0;
            }
        }
    }

    fn deliver(&mut self) {
        let burst = 1 + self.rng.below(8);
        for _ in 0..burst {
            self.deliver_next();
        }
    }

    fn deliver_next(&mut self) {
        let due: Vec<usize> = (0..self.net.len()).filter(|k| self.net[*k].due <= self.step).collect();
        if due.is_empty() {
            return;
        }
        let pick = if !self.quiet && self.rng.chance(1, 2) {
            due[self.rng.below(due.len() as u64) as usize]
        } else {
            due[0]
        };
        let f = self.net.remove(pick);
        self.deliver_one(f.msg);
    }

    fn deliver_one(&mut self, msg: Message) {
        let (from, to) = (slot(msg.from), slot(msg.to));
        if to >= self.nodes() || from >= self.nodes() {
            return;
        }
        if !self.pump.alive[to] || self.group[from] != self.group[to] {
            return;
        }
        self.mix(msg_key(&msg));
        let voting = matches!(msg.body, Body::Vote { .. });
        self.pump.nodes[to].step(msg);
        let odds = if voting { 3 + self.cfg.calm } else { 80 * self.cfg.calm };
        if !self.quiet && self.rng.chance(1, odds) {
            self.crash_after.push(to);
        }
    }

    fn propose(&mut self) {
        let Some(i) = self.pick_leader() else { return };
        self.counter += 1;
        let data = self.counter.to_be_bytes().to_vec();
        let _ = self.pump.nodes[i].propose(data);
        if !self.quiet && self.rng.chance(1, 20 * self.cfg.calm) {
            self.crash_after.push(i);
        }
    }

    fn change_membership(&mut self) {
        let Some(i) = self.pick_leader() else { return };
        let node = &self.pump.nodes[i];
        let members: Vec<NodeId> = node.members().iter().copied().collect();
        let outside: Vec<NodeId> =
            ids(self.cfg.nodes).into_iter().filter(|m| !node.members().contains(m)).collect();
        let me = node.id();
        let removable: Vec<NodeId> = members.iter().copied().filter(|m| *m != me).collect();
        let remove = !removable.is_empty()
            && members.len() > 2
            && (outside.is_empty() || self.rng.chance(1, 2));
        let change = if remove {
            ConfChange::Remove(removable[self.rng.below(removable.len() as u64) as usize])
        } else if !outside.is_empty() {
            ConfChange::Add(outside[self.rng.below(outside.len() as u64) as usize])
        } else {
            return;
        };
        if self.pump.nodes[i].propose_conf_change(change).is_ok() {
            self.report.conf_changes += 1;
        }
    }

    fn commit_hi(&self) -> u64 {
        let applied = self.committed.keys().next_back().copied().unwrap_or(0);
        let live = (0..self.nodes())
            .filter(|i| self.pump.alive[*i])
            .map(|i| self.pump.nodes[i].commit_index().0)
            .max()
            .unwrap_or(0);
        applied.max(live)
    }

    fn read(&mut self) {
        let Some(i) = self.pick_leader() else { return };
        self.ctx += 1;
        let hi = self.commit_hi();
        if self.pump.nodes[i].read_index(self.ctx).is_ok() {
            self.pending_reads.insert(self.ctx, hi);
        }
    }

    fn collect(&mut self) -> Check {
        if let Err(e) = self.pump.collect_checked() {
            return self.bad(e);
        }
        for i in 0..self.nodes() {
            if self.pump.alive[i] {
                let node = &self.pump.nodes[i];
                self.promised[i] = (node.term(), node.vote());
            }
        }
        let sent: Vec<Message> = self.pump.queue.drain(..).collect();
        for m in sent {
            if !self.quiet {
                if self.rng.chance(DROP_PER_MILLE, 1000) {
                    continue;
                }
                let copies = if self.rng.chance(DUP_PER_MILLE, 1000) { 2 } else { 1 };
                for _ in 0..copies {
                    let mut delay = self.rng.below(MAX_DELAY + 1);
                    if self.rng.chance(2, 100) {
                        delay += self.rng.below(LONG_DELAY);
                    }
                    self.net.push(Flight { due: self.step + delay, msg: m.clone() });
                }
            } else {
                self.net.push(Flight { due: self.step, msg: m });
            }
        }
        for i in 0..self.nodes() {
            if !self.pump.alive[i] && self.down_until[i].is_none() {
                self.report.disk_faults += 1;
                let wait = self.rng.between(3, 60);
                self.down_until[i] = Some(self.step + wait);
            }
            let reads: Vec<ReadState> = self.pump.reads[i].drain(..).collect();
            for rs in reads {
                if let Some(hi) = self.pending_reads.remove(&rs.ctx) {
                    if rs.index.0 < hi {
                        return self.bad(format!(
                            "stale read: node {} confirmed index {} below committed {hi}",
                            i + 1,
                            rs.index.0
                        ));
                    }
                    self.report.reads += 1;
                }
            }
        }
        Ok(())
    }

    fn check(&mut self) -> Check {
        for i in 0..self.nodes() {
            let (t, c, r, id) = {
                let node = &self.pump.nodes[i];
                (node.term(), node.commit_index().0, node.role(), node.id())
            };
            self.mix(t.0 ^ (c << 20) ^ ((r as u64) << 60));
            if !self.pump.alive[i] {
                continue;
            }
            if r == Role::Leader {
                if self.lead_base[i].0 != t {
                    self.lead_base[i] = (t, c);
                } else if c > self.lead_base[i].1 {
                    let at = self.pump.nodes[i].storage().term_at(Index(c));
                    if at != Some(t) {
                        return self.bad(format!(
                            "leader {} of term {} advanced commit to {c} on an entry of term {:?}",
                            id.0,
                            t.0,
                            at.map(|x| x.0)
                        ));
                    }
                }
                let first = *self.pump.leaders_by_term.entry(t).or_insert(id);
                if first != id {
                    return self.bad(format!("two leaders in term {}: {} and {}", t.0, first.0, id.0));
                }
            }
            if self.iso[i] > STALE_LEADER_TICKS {
                return self.bad(format!(
                    "node {} stayed leader for {} ticks without a reachable quorum",
                    i + 1,
                    self.iso[i]
                ));
            }
        }
        self.check_logs()?;
        self.check_applied()?;
        self.check_leaders()
    }

    fn check_logs(&self) -> Check {
        let full = self.step.is_multiple_of(FULL_CHECK_EVERY);
        let n = self.nodes();
        for i in 0..n {
            if !self.pump.alive[i] {
                continue;
            }
            let a = self.pump.nodes[i].storage();
            if self.pump.nodes[i].commit_index() > a.last_index() {
                return self.bad(format!("node {} commit is beyond its log", i + 1));
            }
            for j in (i + 1)..n {
                if !self.pump.alive[j] {
                    continue;
                }
                let b = self.pump.nodes[j].storage();
                let shared = a.last_index().min(b.last_index()).0;
                let floor = a.first_index().0.max(b.first_index().0) - 1;
                let lo = if full { floor } else { floor.max(shared.saturating_sub(LOG_WINDOW)) };
                let top = (lo..=shared)
                    .rev()
                    .find(|k| a.term_at(Index(*k)).is_some() && a.term_at(Index(*k)) == b.term_at(Index(*k)));
                let Some(top) = top else { continue };
                for m in lo..=top {
                    if a.term_at(Index(m)) != b.term_at(Index(m)) {
                        return self.bad(format!(
                            "log matching broken between {} and {} at index {m}",
                            i + 1,
                            j + 1
                        ));
                    }
                }
                if top > floor {
                    let ea = a.entries(Index(top), Index(top + 1));
                    let eb = b.entries(Index(top), Index(top + 1));
                    if let (Ok(ea), Ok(eb)) = (ea, eb) {
                        if ea != eb {
                            return self.bad(format!(
                                "nodes {} and {} hold different entries at index {top}",
                                i + 1,
                                j + 1
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn check_applied(&mut self) -> Check {
        for i in 0..self.nodes() {
            if self.pump.snap_installs[i] != self.verified_gen[i] {
                self.verified_gen[i] = self.pump.snap_installs[i];
                self.verified[i] = 0;
            }
            for k in self.verified[i]..self.pump.applied[i].len() {
                let e = &self.pump.applied[i][k];
                let key = (e.term, digest(e));
                if e.index.0 != k as u64 + 1 {
                    return self.bad(format!("node {} applied index {} at position {k}", i + 1, e.index.0));
                }
                match self.committed.get(&e.index.0) {
                    Some(prev) if *prev != key => {
                        return self.bad(format!(
                            "state machine safety: index {} applied with two different entries",
                            e.index.0
                        ));
                    }
                    Some(_) => {}
                    None => {
                        self.committed.insert(e.index.0, key);
                    }
                }
            }
            self.verified[i] = self.pump.applied[i].len();
        }
        Ok(())
    }

    fn check_leaders(&mut self) -> Check {
        let top = self.pump.nodes.iter().map(|n| n.term()).max().unwrap_or_default();
        for i in 0..self.nodes() {
            let node = &self.pump.nodes[i];
            if !self.pump.alive[i] || node.role() != Role::Leader || node.term() != top {
                continue;
            }
            let start = if self.lead_mark[i].0 == top { self.lead_mark[i].1 } else { 0 };
            let st = node.storage();
            let first = st.first_index().0;
            let mut last = start;
            for (idx, (term, _)) in self.committed.range(start + 1..) {
                if idx + 1 >= first && st.term_at(Index(*idx)) != Some(*term) {
                    let what = format!(
                        "leader {} of term {} lacks committed index {idx} (term {})",
                        i + 1,
                        top.0,
                        term.0
                    );
                    return self.bad(what);
                }
                last = *idx;
            }
            self.lead_mark[i] = (top, last);
        }
        Ok(())
    }

    fn flush_net(&mut self) {
        let mut guard = 0;
        while !self.net.is_empty() && guard < 20_000 {
            guard += 1;
            let f = self.net.remove(0);
            self.deliver_one(f.msg);
            if self.pump.collect_checked().is_err() {
                return;
            }
            let sent: Vec<Message> = self.pump.queue.drain(..).collect();
            for m in sent {
                self.net.push(Flight { due: self.step, msg: m });
            }
        }
    }

    fn quiet_round(&mut self) -> Check {
        self.step += 1;
        self.tick_nodes();
        self.collect()?;
        self.flush_net();
        self.collect()?;
        self.check()
    }

    fn heal(&mut self) -> Check {
        self.quiet = true;
        self.group.iter_mut().for_each(|g| *g = 0);
        for i in 0..self.nodes() {
            if self.down_until[i].is_some() {
                self.restart(i)?;
            }
        }
        for i in 0..self.nodes() {
            self.dur.arm_fault(i, u64::MAX);
        }
        for _ in 0..HEAL_STEPS {
            self.quiet_round()?;
        }
        let Some(l) = self.pick_leader() else {
            return self.bad("no leader after the cluster healed".into());
        };
        let Ok(proof) = self.pump.nodes[l].propose(vec![0xFE; 4]) else {
            return self.bad("the healed leader refused a proposal".into());
        };
        for _ in 0..HEAL_PROOF_STEPS {
            self.quiet_round()?;
        }
        let leader = &self.pump.nodes[l];
        if leader.role() == Role::Leader && leader.commit_index() < proof {
            return self.bad("the healed cluster does not commit".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn three(seed: u64) -> SimConfig {
        SimConfig { seed, steps: 3_000, nodes: 3, members: 3, membership: false, calm: 1 }
    }

    fn five(seed: u64) -> SimConfig {
        SimConfig { seed, steps: 3_000, nodes: 5, members: 5, membership: true, calm: 3 }
    }

    fn run_backend(cfg: SimConfig) -> Result<Report, Violation> {
        if cfg.seed.is_multiple_of(2) {
            run(cfg, MemDurability)
        } else {
            run(cfg, DiskDurability::new(cfg.nodes as usize, 4))
        }
    }

    fn gate(seeds: std::ops::Range<u64>, make: fn(u64) -> SimConfig) -> Report {
        let mut total = Report::default();
        for seed in seeds {
            match run_backend(make(seed)) {
                Ok(r) => {
                    total.committed += r.committed;
                    total.reads += r.reads;
                    total.restarts += r.restarts;
                    total.conf_changes += r.conf_changes;
                    total.snapshots += r.snapshots;
                    total.partitions += r.partitions;
                    total.disk_faults += r.disk_faults;
                    total.terms_led += r.terms_led;
                }
                Err(v) => panic!("{v}"),
            }
        }
        total
    }

    #[test]
    fn fifty_seeds_of_three_thousand_steps_on_three_nodes_hold_every_invariant() {
        let t = Instant::now();
        let total = gate(0..50, three);
        eprintln!("gate 50x3000 on 3 nodes: {:?} {total:?}", t.elapsed());
        assert!(total.committed > 2_000, "{total:?}");
        assert!(total.reads > 100, "{total:?}");
        assert!(total.restarts > 200, "{total:?}");
        assert!(total.partitions > 50, "{total:?}");
        assert!(total.snapshots > 0, "{total:?}");
        assert!(total.disk_faults > 0, "{total:?}");
    }

    #[test]
    fn twenty_seeds_of_three_thousand_steps_on_five_nodes_with_membership_changes_hold_every_invariant() {
        let t = Instant::now();
        let total = gate(0..20, five);
        eprintln!("gate 20x3000 on 5 nodes: {:?} {total:?}", t.elapsed());
        assert!(total.committed > 500, "{total:?}");
        assert!(total.conf_changes > 40, "{total:?}");
        assert!(total.restarts > 80, "{total:?}");
        assert!(total.reads > 40, "{total:?}");
    }

    #[test]
    fn the_same_seed_gives_the_same_trace_and_a_different_seed_gives_another() {
        for seed in [3u64, 4] {
            let a = run_backend(five(seed)).expect("clean run");
            let b = run_backend(five(seed)).expect("clean run");
            assert_eq!(a, b, "seed {seed} did not replay identically");
        }
        let a = run_backend(five(3)).expect("clean run");
        let c = run_backend(five(5)).expect("clean run");
        assert_ne!(a.trace, c.trace);
    }
}
