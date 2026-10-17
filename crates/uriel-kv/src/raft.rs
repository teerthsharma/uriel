use crate::rng::Rng;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Term(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Index(pub u64);

impl Index {
    pub(crate) fn plus(self, n: u64) -> Index {
        Index(self.0 + n)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not leader (hint: {hint:?})")]
pub struct NotLeader {
    pub hint: Option<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageError {
    #[error("io: {0}")]
    Io(String),
    #[error("corrupt: {0}")]
    Corrupt(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfError {
    #[error("not leader (hint: {0:?})")]
    NotLeader(Option<NodeId>),
    #[error("a configuration change is already in flight")]
    Pending,
    #[error("the leader has not committed an entry of its own term")]
    NotReady,
    #[error("the change is a no-op or removes the leader")]
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error("not leader (hint: {0:?})")]
    NotLeader(Option<NodeId>),
    #[error("too many reads in flight")]
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Normal,
    Conf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub term: Term,
    pub index: Index,
    pub data: Vec<u8>,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfChange {
    Add(NodeId),
    Remove(NodeId),
}

impl ConfChange {
    pub fn encode(&self) -> Vec<u8> {
        let (op, id) = match self {
            ConfChange::Add(n) => (1u8, n.0),
            ConfChange::Remove(n) => (2u8, n.0),
        };
        let mut v = vec![op];
        v.extend_from_slice(&id.to_le_bytes());
        v
    }

    pub fn decode(b: &[u8]) -> Option<ConfChange> {
        let id = NodeId(u64::from_le_bytes(b.get(1..9)?.try_into().ok()?));
        match (b.len(), b[0]) {
            (9, 1) => Some(ConfChange::Add(id)),
            (9, 2) => Some(ConfChange::Remove(id)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotMeta {
    pub index: Index,
    pub term: Term,
    pub members: Vec<NodeId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub meta: SnapshotMeta,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadState {
    pub ctx: u64,
    pub index: Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Follower,
    PreCandidate,
    Candidate,
    Leader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HardState {
    pub term: Term,
    pub vote: Option<NodeId>,
    pub commit: Index,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    PreVote { last_index: Index, last_term: Term },
    PreVoteResp { granted: bool },
    Vote { last_index: Index, last_term: Term },
    VoteResp { granted: bool },
    Append { prev_index: Index, prev_term: Term, entries: Vec<Entry>, commit: Index },
    AppendResp { success: bool, match_index: Index, hint: Index },
    Heartbeat { commit: Index, seq: u64 },
    HeartbeatResp { seq: u64 },
    Snapshot { meta: SnapshotMeta, data: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub from: NodeId,
    pub to: NodeId,
    pub term: Term,
    pub body: Body,
}

pub trait Storage {
    fn hard_state(&self) -> HardState;
    fn set_hard_state(&mut self, hs: HardState) -> Result<(), StorageError>;
    fn applied(&self) -> Index;
    fn set_applied(&mut self, applied: Index) -> Result<(), StorageError>;
    fn first_index(&self) -> Index;
    fn last_index(&self) -> Index;
    fn term_at(&self, index: Index) -> Option<Term>;
    fn entries(&self, from: Index, to: Index) -> Result<Vec<Entry>, StorageError>;
    fn append(&mut self, entries: &[Entry]) -> Result<(), StorageError>;
    fn truncate_from(&mut self, index: Index) -> Result<(), StorageError>;
    fn snapshot_meta(&self) -> SnapshotMeta;
    fn snapshot_data(&self) -> Result<Vec<u8>, StorageError>;
    fn install_snapshot(&mut self, snap: Snapshot) -> Result<(), StorageError>;
    fn sync(&mut self) -> Result<(), StorageError>;
}

#[derive(Debug, Clone, Default)]
pub struct MemStorage {
    hs: HardState,
    applied: Index,
    base: Index,
    base_term: Term,
    snap: Snapshot,
    log: Vec<Entry>,
}

impl MemStorage {
    pub fn new() -> Self {
        MemStorage::default()
    }

    fn slot(&self, index: Index) -> Option<usize> {
        index.0.checked_sub(self.base.0 + 1).map(|p| p as usize)
    }
}

impl Storage for MemStorage {
    fn hard_state(&self) -> HardState {
        self.hs
    }

    fn set_hard_state(&mut self, hs: HardState) -> Result<(), StorageError> {
        self.hs = hs;
        Ok(())
    }

    fn applied(&self) -> Index {
        self.applied
    }

    fn set_applied(&mut self, applied: Index) -> Result<(), StorageError> {
        self.applied = applied;
        Ok(())
    }

    fn first_index(&self) -> Index {
        self.base.plus(1)
    }

    fn last_index(&self) -> Index {
        self.base.plus(self.log.len() as u64)
    }

    fn term_at(&self, index: Index) -> Option<Term> {
        if index == self.base {
            return Some(self.base_term);
        }
        self.log.get(self.slot(index)?).map(|e| e.term)
    }

    fn entries(&self, from: Index, to: Index) -> Result<Vec<Entry>, StorageError> {
        let lo = from.0.max(self.base.0 + 1);
        let hi = to.0.min(self.last_index().0 + 1);
        if lo >= hi {
            return Ok(Vec::new());
        }
        let off = self.base.0 + 1;
        Ok(self.log[(lo - off) as usize..(hi - off) as usize].to_vec())
    }

    fn append(&mut self, entries: &[Entry]) -> Result<(), StorageError> {
        self.log.extend(entries.iter().cloned());
        Ok(())
    }

    fn truncate_from(&mut self, index: Index) -> Result<(), StorageError> {
        let keep = index.0.saturating_sub(self.base.0 + 1) as usize;
        self.log.truncate(keep);
        Ok(())
    }

    fn snapshot_meta(&self) -> SnapshotMeta {
        self.snap.meta.clone()
    }

    fn snapshot_data(&self) -> Result<Vec<u8>, StorageError> {
        Ok(self.snap.data.clone())
    }

    fn install_snapshot(&mut self, snap: Snapshot) -> Result<(), StorageError> {
        if snap.meta.index <= self.base {
            return Ok(());
        }
        let keep = snap.meta.index <= self.last_index()
            && self.term_at(snap.meta.index) == Some(snap.meta.term);
        if keep {
            let drop = (snap.meta.index.0 - self.base.0) as usize;
            self.log.drain(..drop);
        } else {
            self.log.clear();
        }
        self.base = snap.meta.index;
        self.base_term = snap.meta.term;
        self.snap = snap;
        Ok(())
    }

    fn sync(&mut self) -> Result<(), StorageError> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub messages: Vec<Message>,
    pub hard_state_changed: bool,
    pub committed_entries: Vec<Entry>,
    pub snapshot: Option<Snapshot>,
    pub read_states: Vec<ReadState>,
}

const ELECTION_MIN: u64 = 10;
const ELECTION_MAX: u64 = 19;
const HEARTBEAT_INTERVAL: u64 = 2;
const MAX_BATCH: u64 = 100;
const MAX_APPLY_BATCH: u64 = 1024;
const MAX_PENDING_READS: usize = 1024;
const REPLAY_BATCH: u64 = 256;

type Res = Result<(), StorageError>;

fn empty_ready() -> Ready {
    Ready {
        messages: Vec::new(),
        hard_state_changed: false,
        committed_entries: Vec::new(),
        snapshot: None,
        read_states: Vec::new(),
    }
}

fn quorum_of(members: usize) -> usize {
    members / 2 + 1
}

struct PendingRead {
    ctx: u64,
    index: Index,
    seq: u64,
    acks: BTreeSet<NodeId>,
}

pub struct Raft<S: Storage> {
    id: NodeId,
    members: BTreeSet<NodeId>,
    conf_hist: Vec<(Index, BTreeSet<NodeId>)>,
    voter: bool,
    peers: Vec<NodeId>,
    storage: S,
    role: Role,
    term: Term,
    vote: Option<NodeId>,
    commit: Index,
    leader: Option<NodeId>,
    election_elapsed: u64,
    election_timeout: u64,
    heartbeat_elapsed: u64,
    rng: Rng,
    granted: BTreeSet<NodeId>,
    outbox: Vec<Message>,
    dirty: bool,
    next_index: BTreeMap<NodeId, Index>,
    match_index: BTreeMap<NodeId, Index>,
    applied: Index,
    check_quorum: bool,
    quorum_elapsed: u64,
    active: BTreeSet<NodeId>,
    fault: Option<StorageError>,
    term_start: Index,
    pending_snapshot: Option<Snapshot>,
    pending_reads: VecDeque<PendingRead>,
    deferred_reads: Vec<u64>,
    read_seq: u64,
    read_states: Vec<ReadState>,
}

impl<S: Storage> Raft<S> {
    pub fn new(id: NodeId, peers: Vec<NodeId>, storage: S, seed: u64) -> Result<Self, StorageError> {
        let mut members: BTreeSet<NodeId> = peers.into_iter().collect();
        members.insert(id);
        Raft::build(id, members, storage, seed)
    }

    pub fn joining(
        id: NodeId,
        members: Vec<NodeId>,
        storage: S,
        seed: u64,
    ) -> Result<Self, StorageError> {
        let members: BTreeSet<NodeId> = members.into_iter().filter(|m| *m != id).collect();
        Raft::build(id, members, storage, seed)
    }

    fn build(
        id: NodeId,
        boot: BTreeSet<NodeId>,
        storage: S,
        seed: u64,
    ) -> Result<Self, StorageError> {
        let hs = storage.hard_state();
        let applied = storage.applied();
        let meta = storage.snapshot_meta();
        if hs.commit > storage.last_index() {
            return Err(StorageError::Corrupt("commit index beyond the log".into()));
        }
        if applied > hs.commit.max(meta.index) {
            return Err(StorageError::Corrupt("applied index beyond commit".into()));
        }
        let base_members: BTreeSet<NodeId> =
            if meta.index.0 > 0 { meta.members.iter().copied().collect() } else { boot };
        let mut rng = Rng::new(seed.wrapping_add(id.0));
        let election_timeout = rng.between(ELECTION_MIN, ELECTION_MAX);
        let mut r = Raft {
            id,
            members: BTreeSet::new(),
            conf_hist: vec![(meta.index, base_members)],
            voter: false,
            peers: Vec::new(),
            storage,
            role: Role::Follower,
            term: hs.term,
            vote: hs.vote,
            commit: hs.commit.max(meta.index),
            leader: None,
            election_elapsed: 0,
            election_timeout,
            heartbeat_elapsed: 0,
            rng,
            granted: BTreeSet::new(),
            outbox: Vec::new(),
            dirty: false,
            next_index: BTreeMap::new(),
            match_index: BTreeMap::new(),
            applied,
            check_quorum: false,
            quorum_elapsed: 0,
            active: BTreeSet::new(),
            fault: None,
            term_start: Index(0),
            pending_snapshot: None,
            pending_reads: VecDeque::new(),
            deferred_reads: Vec::new(),
            read_seq: 0,
            read_states: Vec::new(),
        };
        r.refresh_members();
        r.replay_conf()?;
        if applied < meta.index {
            let data = r.storage.snapshot_data()?;
            r.pending_snapshot = Some(Snapshot { meta, data });
        }
        Ok(r)
    }

    pub fn id(&self) -> NodeId {
        self.id
    }

    pub fn set_check_quorum(&mut self, on: bool) {
        self.check_quorum = on;
    }

    pub fn commit_index(&self) -> Index {
        self.commit
    }

    pub fn applied_index(&self) -> Index {
        self.applied
    }

    pub fn fault(&self) -> Option<&StorageError> {
        self.fault.as_ref()
    }

    pub fn members(&self) -> &BTreeSet<NodeId> {
        &self.members
    }

    pub fn is_voter(&self) -> bool {
        self.voter
    }

    pub fn propose(&mut self, data: Vec<u8>) -> Result<Index, NotLeader> {
        if self.fault.is_some() || self.role != Role::Leader {
            let hint = if self.fault.is_some() { None } else { self.leader };
            return Err(NotLeader { hint });
        }
        let mut index = Index(0);
        self.run(|r| {
            index = r.storage.last_index().plus(1);
            let entry = Entry { term: r.term, index, data, kind: EntryKind::Normal };
            r.storage.append(std::slice::from_ref(&entry))?;
            r.replicate()?;
            r.maybe_commit()
        });
        if self.fault.is_some() {
            return Err(NotLeader { hint: None });
        }
        Ok(index)
    }

    pub fn propose_conf_change(&mut self, change: ConfChange) -> Result<Index, ConfError> {
        if self.fault.is_some() || self.role != Role::Leader {
            let hint = if self.fault.is_some() { None } else { self.leader };
            return Err(ConfError::NotLeader(hint));
        }
        if self.commit < self.term_start {
            return Err(ConfError::NotReady);
        }
        if self.conf_hist.last().is_some_and(|(i, _)| *i > self.commit) {
            return Err(ConfError::Pending);
        }
        let valid = match change {
            ConfChange::Add(n) => !self.members.contains(&n),
            ConfChange::Remove(n) => n != self.id && self.members.contains(&n),
        };
        if !valid {
            return Err(ConfError::Invalid);
        }
        let mut index = Index(0);
        self.run(|r| {
            index = r.storage.last_index().plus(1);
            let entry = Entry { term: r.term, index, data: change.encode(), kind: EntryKind::Conf };
            r.storage.append(std::slice::from_ref(&entry))?;
            r.push_conf(index, change);
            r.replicate()?;
            r.maybe_commit()
        });
        if self.fault.is_some() {
            return Err(ConfError::NotLeader(None));
        }
        Ok(index)
    }

    pub fn read_index(&mut self, ctx: u64) -> Result<(), ReadError> {
        if self.fault.is_some() || self.role != Role::Leader {
            let hint = if self.fault.is_some() { None } else { self.leader };
            return Err(ReadError::NotLeader(hint));
        }
        if self.pending_reads.len() + self.deferred_reads.len() >= MAX_PENDING_READS {
            return Err(ReadError::Busy);
        }
        if self.commit < self.term_start {
            self.deferred_reads.push(ctx);
        } else {
            self.start_read(ctx);
        }
        Ok(())
    }

    pub fn compact(&mut self, index: Index, data: Vec<u8>) -> Result<(), StorageError> {
        if let Some(e) = &self.fault {
            return Err(e.clone());
        }
        let base = self.storage.first_index().0 - 1;
        if index > self.applied || index.0 <= base {
            return Err(StorageError::Invalid("compaction point outside (snapshot, applied]".into()));
        }
        let term = self
            .storage
            .term_at(index)
            .ok_or_else(|| StorageError::Invalid("no entry at the compaction point".into()))?;
        let members = self.members_at(index).into_iter().collect();
        let snap = Snapshot { meta: SnapshotMeta { index, term, members }, data };
        self.run(|r| {
            r.storage.install_snapshot(snap)?;
            r.rebase_conf(index);
            Ok(())
        });
        match &self.fault {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn term(&self) -> Term {
        self.term
    }

    pub fn leader(&self) -> Option<NodeId> {
        self.leader
    }

    pub fn storage(&self) -> &S {
        &self.storage
    }

    fn run(&mut self, f: impl FnOnce(&mut Self) -> Res) {
        if self.fault.is_some() {
            return;
        }
        let r = f(self).and_then(|()| self.storage.sync());
        if let Err(e) = r {
            self.fault = Some(e);
            self.outbox.clear();
        }
    }

    fn quorum(&self) -> usize {
        quorum_of(self.members.len())
    }

    fn members_at(&self, index: Index) -> BTreeSet<NodeId> {
        self.conf_hist
            .iter()
            .rev()
            .find(|(i, _)| *i <= index)
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    }

    fn refresh_members(&mut self) {
        self.members = self.conf_hist.last().map(|(_, m)| m.clone()).unwrap_or_default();
        self.voter = self.members.contains(&self.id);
        self.peers = self.members.iter().copied().filter(|m| *m != self.id).collect();
        if self.role == Role::Leader {
            if !self.voter {
                self.role = Role::Follower;
                self.leader = None;
                self.pending_reads.clear();
                self.deferred_reads.clear();
                return;
            }
            let first = self.storage.first_index();
            let peers: BTreeSet<NodeId> = self.peers.iter().copied().collect();
            self.next_index.retain(|p, _| peers.contains(p));
            self.match_index.retain(|p, _| peers.contains(p));
            for p in &peers {
                self.next_index.entry(*p).or_insert(first);
                self.match_index.entry(*p).or_insert(Index(0));
            }
        }
    }

    fn push_conf(&mut self, index: Index, change: ConfChange) {
        let mut m = self.members.clone();
        match change {
            ConfChange::Add(n) => {
                m.insert(n);
            }
            ConfChange::Remove(n) => {
                m.remove(&n);
            }
        }
        self.conf_hist.push((index, m));
        self.refresh_members();
    }

    fn apply_entry_conf(&mut self, e: &Entry) -> Res {
        if e.kind == EntryKind::Conf {
            let change = ConfChange::decode(&e.data)
                .ok_or_else(|| StorageError::Corrupt("malformed configuration change".into()))?;
            self.push_conf(e.index, change);
        }
        Ok(())
    }

    fn truncate_conf(&mut self, from: Index) {
        while self.conf_hist.len() > 1 && self.conf_hist.last().is_some_and(|(i, _)| *i >= from) {
            self.conf_hist.pop();
        }
        self.refresh_members();
    }

    fn rebase_conf(&mut self, index: Index) {
        let m = self.members_at(index);
        self.conf_hist.retain(|(i, _)| *i > index);
        self.conf_hist.insert(0, (index, m));
    }

    fn replay_conf(&mut self) -> Res {
        let last = self.storage.last_index();
        let mut from = self.storage.first_index();
        while from <= last {
            let to = Index(last.0.min(from.0 + REPLAY_BATCH - 1)).plus(1);
            for e in self.storage.entries(from, to)? {
                self.apply_entry_conf(&e)?;
            }
            from = to;
        }
        Ok(())
    }

    fn persist(&mut self) -> Res {
        self.storage.set_hard_state(HardState {
            term: self.term,
            vote: self.vote,
            commit: self.commit,
        })?;
        self.dirty = true;
        Ok(())
    }

    fn reset_timer(&mut self) {
        self.election_elapsed = 0;
        self.election_timeout = self.rng.between(ELECTION_MIN, ELECTION_MAX);
    }

    fn send(&mut self, to: NodeId, term: Term, body: Body) {
        self.outbox.push(Message { from: self.id, to, term, body });
    }

    fn broadcast(&mut self, term: Term, body: Body) {
        for i in 0..self.peers.len() {
            let to = self.peers[i];
            self.send(to, term, body.clone());
        }
    }

    fn replicate(&mut self) -> Res {
        for i in 0..self.peers.len() {
            let peer = self.peers[i];
            self.send_append(peer)?;
        }
        Ok(())
    }

    fn last_term(&self) -> Term {
        self.storage.term_at(self.storage.last_index()).unwrap_or_default()
    }

    fn log_is_up_to_date(&self, last_index: Index, last_term: Term) -> bool {
        let mine = self.last_term();
        last_term > mine || (last_term == mine && last_index >= self.storage.last_index())
    }

    fn leader_alive(&self) -> bool {
        self.role == Role::Leader
            || (self.leader.is_some() && self.election_elapsed < self.election_timeout)
    }

    fn become_follower(&mut self, term: Term, leader: Option<NodeId>) -> Res {
        if term > self.term {
            self.term = term;
            self.vote = None;
            self.persist()?;
        }
        self.role = Role::Follower;
        self.leader = leader;
        self.granted.clear();
        self.pending_reads.clear();
        self.deferred_reads.clear();
        self.heartbeat_elapsed = 0;
        self.reset_timer();
        Ok(())
    }

    fn accept_leader(&mut self, leader: NodeId) {
        self.role = Role::Follower;
        self.leader = Some(leader);
        self.granted.clear();
        self.election_elapsed = 0;
    }

    fn start_prevote(&mut self) -> Res {
        self.role = Role::PreCandidate;
        self.leader = None;
        self.granted.clear();
        self.granted.insert(self.id);
        self.reset_timer();
        if self.granted.len() >= self.quorum() {
            return self.start_election();
        }
        let body = Body::PreVote {
            last_index: self.storage.last_index(),
            last_term: self.last_term(),
        };
        self.broadcast(Term(self.term.0 + 1), body);
        Ok(())
    }

    fn start_election(&mut self) -> Res {
        self.term = Term(self.term.0 + 1);
        self.vote = Some(self.id);
        self.persist()?;
        self.role = Role::Candidate;
        self.leader = None;
        self.granted.clear();
        self.granted.insert(self.id);
        self.reset_timer();
        if self.granted.len() >= self.quorum() {
            return self.become_leader();
        }
        let body = Body::Vote {
            last_index: self.storage.last_index(),
            last_term: self.last_term(),
        };
        self.broadcast(self.term, body);
        Ok(())
    }

    fn become_leader(&mut self) -> Res {
        self.role = Role::Leader;
        self.leader = Some(self.id);
        self.granted.clear();
        self.heartbeat_elapsed = 0;
        self.active.clear();
        self.quorum_elapsed = 0;
        self.pending_reads.clear();
        self.deferred_reads.clear();
        let prev_index = self.storage.last_index();
        self.term_start = prev_index.plus(1);
        self.next_index.clear();
        self.match_index.clear();
        for i in 0..self.peers.len() {
            let peer = self.peers[i];
            self.next_index.insert(peer, prev_index.plus(1));
            self.match_index.insert(peer, Index(0));
        }
        let noop = Entry {
            term: self.term,
            index: prev_index.plus(1),
            data: Vec::new(),
            kind: EntryKind::Normal,
        };
        self.storage.append(std::slice::from_ref(&noop))?;
        self.replicate()?;
        self.maybe_commit()
    }

    fn send_append(&mut self, peer: NodeId) -> Res {
        let last = self.storage.last_index();
        let first = self.storage.first_index();
        let next = self.next_index.get(&peer).copied().unwrap_or(last.plus(1)).max(Index(1));
        if next < first {
            let meta = self.storage.snapshot_meta();
            let data = self.storage.snapshot_data()?;
            self.next_index.insert(peer, first);
            self.send(peer, self.term, Body::Snapshot { meta, data });
            return Ok(());
        }
        if next > last {
            return Ok(());
        }
        let prev_index = Index(next.0 - 1);
        let prev_term = self.storage.term_at(prev_index).unwrap_or_default();
        let upto = Index(last.0.min(next.0 + MAX_BATCH - 1));
        let entries = self.storage.entries(next, upto.plus(1))?;
        self.next_index.insert(peer, upto.plus(1));
        let commit = self.commit;
        self.send(peer, self.term, Body::Append { prev_index, prev_term, entries, commit });
        Ok(())
    }

    fn maybe_commit(&mut self) -> Res {
        if self.role != Role::Leader {
            return Ok(());
        }
        let mut matched: Vec<Index> = self
            .peers
            .iter()
            .map(|p| self.match_index.get(p).copied().unwrap_or_default())
            .collect();
        matched.push(self.storage.last_index());
        matched.sort_unstable_by(|a, b| b.cmp(a));
        let candidate = matched[self.quorum() - 1];
        if candidate > self.commit && self.storage.term_at(candidate) == Some(self.term) {
            self.commit = candidate;
            self.persist()?;
        }
        if self.commit >= self.term_start && !self.deferred_reads.is_empty() {
            for ctx in std::mem::take(&mut self.deferred_reads) {
                self.start_read(ctx);
            }
        }
        Ok(())
    }

    fn start_read(&mut self, ctx: u64) {
        if self.quorum() <= 1 {
            self.read_states.push(ReadState { ctx, index: self.commit });
            return;
        }
        self.read_seq += 1;
        let seq = self.read_seq;
        let acks = BTreeSet::from([self.id]);
        self.pending_reads.push_back(PendingRead { ctx, index: self.commit, seq, acks });
        self.heartbeat(seq);
    }

    fn heartbeat(&mut self, seq: u64) {
        for i in 0..self.peers.len() {
            let peer = self.peers[i];
            let matched = self.match_index.get(&peer).copied().unwrap_or_default();
            let commit = self.commit.min(matched);
            self.send(peer, self.term, Body::Heartbeat { commit, seq });
        }
    }

    fn conflict_hint(&self, prev_index: Index) -> Index {
        let last = self.storage.last_index();
        if prev_index > last {
            return last;
        }
        let conflict_term = self.storage.term_at(prev_index).unwrap_or_default();
        let mut first = prev_index.0;
        while first > 1 && self.storage.term_at(Index(first - 1)) == Some(conflict_term) {
            first -= 1;
        }
        Index(first - 1)
    }

    fn handle_append(
        &mut self,
        from: NodeId,
        prev_index: Index,
        prev_term: Term,
        entries: Vec<Entry>,
        leader_commit: Index,
    ) -> Res {
        self.accept_leader(from);
        let base = Index(self.storage.first_index().0 - 1);
        let (prev_index, prev_term, entries) = if prev_index < base {
            let kept: Vec<Entry> = entries.into_iter().filter(|e| e.index > base).collect();
            (base, self.storage.term_at(base).unwrap_or_default(), kept)
        } else {
            (prev_index, prev_term, entries)
        };
        let last = self.storage.last_index();
        if prev_index > last || self.storage.term_at(prev_index) != Some(prev_term) {
            let hint = self.conflict_hint(prev_index);
            let body = Body::AppendResp { success: false, match_index: Index(0), hint };
            self.send(from, self.term, body);
            return Ok(());
        }
        let last_new = prev_index.plus(entries.len() as u64);
        let mut skip = 0;
        for e in &entries {
            if self.storage.term_at(e.index) == Some(e.term) {
                skip += 1;
            } else {
                break;
            }
        }
        if skip < entries.len() {
            let first = entries[skip].index;
            if first <= self.storage.last_index() {
                self.storage.truncate_from(first)?;
                self.truncate_conf(first);
            }
            self.storage.append(&entries[skip..])?;
            for e in &entries[skip..] {
                self.apply_entry_conf(e)?;
            }
        }
        let target = leader_commit.min(last_new);
        if target > self.commit {
            self.commit = target;
            self.persist()?;
        }
        let body = Body::AppendResp { success: true, match_index: last_new, hint: last_new };
        self.send(from, self.term, body);
        Ok(())
    }

    fn handle_snapshot(&mut self, from: NodeId, meta: SnapshotMeta, data: Vec<u8>) -> Res {
        self.accept_leader(from);
        if meta.index <= self.commit {
            let commit = self.commit;
            let body = Body::AppendResp { success: true, match_index: commit, hint: commit };
            self.send(from, self.term, body);
            return Ok(());
        }
        let index = meta.index;
        let members: BTreeSet<NodeId> = meta.members.iter().copied().collect();
        let snap = Snapshot { meta, data };
        self.storage.install_snapshot(snap.clone())?;
        self.conf_hist = vec![(index, members)];
        self.refresh_members();
        self.replay_conf()?;
        self.commit = index;
        self.persist()?;
        self.pending_snapshot = Some(snap);
        let body = Body::AppendResp { success: true, match_index: index, hint: index };
        self.send(from, self.term, body);
        Ok(())
    }

    fn handle_append_resp(
        &mut self,
        from: NodeId,
        success: bool,
        match_index: Index,
        hint: Index,
    ) -> Res {
        if self.role != Role::Leader || !self.next_index.contains_key(&from) {
            return Ok(());
        }
        self.active.insert(from);
        let last = self.storage.last_index();
        let matched = self.match_index.get(&from).copied().unwrap_or_default();
        if success {
            let acked = match_index.min(last);
            if acked > matched {
                self.match_index.insert(from, acked);
            }
            let floor = self.match_index.get(&from).copied().unwrap_or_default().plus(1);
            let next = self.next_index.get(&from).copied().unwrap_or(floor);
            if next < floor {
                self.next_index.insert(from, floor);
            }
            self.maybe_commit()?;
        } else {
            let next = self.next_index.get(&from).copied().unwrap_or(last.plus(1));
            let lowered = next.min(hint.plus(1)).max(matched.plus(1));
            self.next_index.insert(from, lowered);
        }
        self.send_append(from)
    }

    fn handle_heartbeat_resp(&mut self, from: NodeId, seq: u64) -> Res {
        if self.role != Role::Leader || !self.next_index.contains_key(&from) {
            return Ok(());
        }
        self.active.insert(from);
        if seq > 0 {
            for pr in self.pending_reads.iter_mut().filter(|pr| pr.seq <= seq) {
                pr.acks.insert(from);
            }
            let quorum = self.quorum();
            let (done, waiting): (Vec<PendingRead>, Vec<PendingRead>) =
                std::mem::take(&mut self.pending_reads)
                    .into_iter()
                    .partition(|pr| pr.acks.len() >= quorum);
            self.pending_reads = waiting.into();
            for pr in done {
                self.read_states.push(ReadState { ctx: pr.ctx, index: pr.index });
            }
        }
        let matched = self.match_index.get(&from).copied().unwrap_or_default();
        if matched < self.storage.last_index() {
            self.next_index.insert(from, matched.plus(1));
            self.send_append(from)?;
        }
        Ok(())
    }

    fn reject_stale(&mut self, msg: &Message) {
        let body = match msg.body {
            Body::PreVote { .. } => Body::PreVoteResp { granted: false },
            Body::Vote { .. } => Body::VoteResp { granted: false },
            Body::Append { .. } | Body::Snapshot { .. } => Body::AppendResp {
                success: false,
                match_index: Index(0),
                hint: self.storage.last_index(),
            },
            Body::Heartbeat { seq, .. } => Body::HeartbeatResp { seq },
            _ => return,
        };
        self.send(msg.from, self.term, body);
    }

    pub fn tick(&mut self) {
        self.run(Self::tick_inner);
    }

    fn tick_inner(&mut self) -> Res {
        if self.role == Role::Leader {
            if self.check_quorum {
                self.quorum_elapsed += 1;
                if self.quorum_elapsed >= 2 * self.election_timeout {
                    let heard = self.active.len() + 1;
                    self.active.clear();
                    self.quorum_elapsed = 0;
                    if heard < self.quorum() {
                        return self.become_follower(self.term, None);
                    }
                }
            }
            self.heartbeat_elapsed += 1;
            if self.heartbeat_elapsed >= HEARTBEAT_INTERVAL {
                self.heartbeat_elapsed = 0;
                let seq = self.pending_reads.back().map_or(0, |pr| pr.seq);
                self.heartbeat(seq);
            }
        } else {
            self.election_elapsed += 1;
            if self.election_elapsed >= self.election_timeout && self.voter {
                return self.start_prevote();
            }
        }
        Ok(())
    }

    pub fn step(&mut self, msg: Message) {
        self.run(|r| r.step_inner(msg));
    }

    fn step_inner(&mut self, msg: Message) -> Res {
        if msg.term > self.term {
            match msg.body {
                Body::PreVote { .. } | Body::PreVoteResp { granted: true } => {}
                Body::Append { .. } | Body::Heartbeat { .. } | Body::Snapshot { .. } => {
                    self.become_follower(msg.term, Some(msg.from))?
                }
                _ => self.become_follower(msg.term, None)?,
            }
        } else if msg.term < self.term {
            self.reject_stale(&msg);
            return Ok(());
        }
        let member = self.members.contains(&msg.from);
        match msg.body {
            Body::PreVote { last_index, last_term } => {
                let grant = msg.term > self.term
                    && self.voter
                    && member
                    && !self.leader_alive()
                    && self.log_is_up_to_date(last_index, last_term);
                let term = if grant { msg.term } else { self.term };
                self.send(msg.from, term, Body::PreVoteResp { granted: grant });
            }
            Body::PreVoteResp { granted } => {
                if self.role == Role::PreCandidate
                    && granted
                    && member
                    && msg.term == Term(self.term.0 + 1)
                {
                    self.granted.insert(msg.from);
                    if self.granted.len() >= self.quorum() {
                        self.start_election()?;
                    }
                }
            }
            Body::Vote { last_index, last_term } => {
                let free = self.vote.is_none() || self.vote == Some(msg.from);
                let grant = free && self.voter && member && self.log_is_up_to_date(last_index, last_term);
                if grant {
                    self.vote = Some(msg.from);
                    self.persist()?;
                    self.election_elapsed = 0;
                }
                self.send(msg.from, self.term, Body::VoteResp { granted: grant });
            }
            Body::VoteResp { granted } => {
                if self.role == Role::Candidate && granted && member {
                    self.granted.insert(msg.from);
                    if self.granted.len() >= self.quorum() {
                        self.become_leader()?;
                    }
                }
            }
            Body::Append { prev_index, prev_term, entries, commit } => {
                self.handle_append(msg.from, prev_index, prev_term, entries, commit)?;
            }
            Body::AppendResp { success, match_index, hint } => {
                self.handle_append_resp(msg.from, success, match_index, hint)?;
            }
            Body::Heartbeat { commit, seq } => {
                self.accept_leader(msg.from);
                let target = commit.min(self.storage.last_index());
                if target > self.commit {
                    self.commit = target;
                    self.persist()?;
                }
                self.send(msg.from, self.term, Body::HeartbeatResp { seq });
            }
            Body::HeartbeatResp { seq } => self.handle_heartbeat_resp(msg.from, seq)?,
            Body::Snapshot { meta, data } => self.handle_snapshot(msg.from, meta, data)?,
        }
        Ok(())
    }

    pub fn ready(&mut self) -> Ready {
        if self.fault.is_some() {
            return empty_ready();
        }
        let floor = self.applied.max(Index(self.storage.first_index().0 - 1));
        let to = self.commit.min(floor.plus(MAX_APPLY_BATCH)).plus(1);
        let committed_entries = match self.storage.entries(floor.plus(1), to) {
            Ok(e) => e,
            Err(e) => {
                self.fault = Some(e);
                self.outbox.clear();
                return empty_ready();
            }
        };
        Ready {
            messages: self.outbox.clone(),
            hard_state_changed: self.dirty,
            committed_entries,
            snapshot: self.pending_snapshot.clone(),
            read_states: self.read_states.clone(),
        }
    }

    pub fn advance(&mut self, ready: Ready) {
        let n = ready.messages.len().min(self.outbox.len());
        self.outbox.drain(..n);
        let r = ready.read_states.len().min(self.read_states.len());
        self.read_states.drain(..r);
        self.dirty = false;
        let mut applied = self.applied;
        if let Some(snap) = &ready.snapshot {
            applied = applied.max(snap.meta.index);
            self.pending_snapshot = None;
        }
        if let Some(last) = ready.committed_entries.last() {
            applied = applied.max(last.index);
        }
        if applied > self.applied {
            self.run(|r| {
                r.storage.set_applied(applied)?;
                r.applied = applied;
                Ok(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::{FileStorage, SimDisk};
    use std::collections::{BTreeMap, VecDeque};

    fn ids(n: u64) -> Vec<NodeId> {
        (1..=n).map(NodeId).collect()
    }

    struct Pump<S: Storage = MemStorage> {
        nodes: Vec<Raft<S>>,
        alive: Vec<bool>,
        restarted: Vec<bool>,
        boot: Vec<NodeId>,
        reads: Vec<Vec<ReadState>>,
        snapshots_sent: usize,
        isolated: BTreeSet<NodeId>,
        queue: VecDeque<Message>,
        leaders_by_term: BTreeMap<Term, NodeId>,
        applied: Vec<Vec<Entry>>,
        chaos: Option<Rng>,
    }

    fn slot(id: NodeId) -> usize {
        id.0 as usize - 1
    }

    fn payload(i: u32) -> Vec<u8> {
        i.to_be_bytes().to_vec()
    }

    fn tagged(tag: u8, i: u32) -> Vec<u8> {
        let mut v = vec![tag];
        v.extend(i.to_be_bytes());
        v
    }

    fn entry(term: u64, index: u64) -> Entry {
        Entry { term: Term(term), index: Index(index), data: vec![index as u8], kind: EntryKind::Normal }
    }

    impl Pump<MemStorage> {
        fn new(n: u64, seed: u64) -> Self {
            Pump::with_storages((0..n).map(|_| MemStorage::new()).collect(), seed)
        }

        fn with_members(total: u64, members: u64, seed: u64) -> Self {
            let storages = (0..total).map(|_| MemStorage::new()).collect();
            Pump::with_boot(storages, ids(members), seed)
        }
    }

    fn encode_state(entries: &[Entry]) -> Vec<u8> {
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

    fn decode_state(b: &[u8]) -> Vec<Entry> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < b.len() {
            let term = u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
            let index = u64::from_le_bytes(b[at + 8..at + 16].try_into().unwrap());
            let kind = if b[at + 16] == 1 { EntryKind::Conf } else { EntryKind::Normal };
            let len = u32::from_le_bytes(b[at + 17..at + 21].try_into().unwrap()) as usize;
            let data = b[at + 21..at + 21 + len].to_vec();
            out.push(Entry { term: Term(term), index: Index(index), data, kind });
            at += 21 + len;
        }
        out
    }

    fn members_of<S: Storage>(r: &Raft<S>) -> Vec<u64> {
        r.members().iter().map(|m| m.0).collect()
    }

    impl<S: Storage> Pump<S> {
        fn with_storages(storages: Vec<S>, seed: u64) -> Self {
            let boot = ids(storages.len() as u64);
            Pump::with_boot(storages, boot, seed)
        }

        fn make(id: NodeId, boot: &[NodeId], storage: S, seed: u64) -> Raft<S> {
            if boot.contains(&id) {
                Raft::new(id, boot.to_vec(), storage, seed).unwrap()
            } else {
                Raft::joining(id, boot.to_vec(), storage, seed).unwrap()
            }
        }

        fn with_boot(storages: Vec<S>, boot: Vec<NodeId>, seed: u64) -> Self {
            let all = ids(storages.len() as u64);
            let nodes: Vec<_> = storages
                .into_iter()
                .zip(all.iter())
                .map(|(s, id)| Pump::make(*id, &boot, s, seed))
                .collect();
            let n = nodes.len();
            Pump {
                nodes,
                alive: vec![true; n],
                restarted: vec![false; n],
                boot,
                reads: vec![Vec::new(); n],
                snapshots_sent: 0,
                isolated: BTreeSet::new(),
                queue: VecDeque::new(),
                leaders_by_term: BTreeMap::new(),
                applied: vec![Vec::new(); n],
                chaos: None,
            }
        }

        fn restart(&mut self, i: usize, storage: S, seed: u64) {
            self.nodes[i] = Pump::make(NodeId(i as u64 + 1), &self.boot.clone(), storage, seed);
            self.alive[i] = true;
            self.restarted[i] = true;
        }

        fn reachable(&self, a: NodeId, b: NodeId) -> bool {
            self.alive[slot(a)]
                && self.alive[slot(b)]
                && !self.isolated.contains(&a)
                && !self.isolated.contains(&b)
        }

        fn observe(&mut self) {
            for (i, node) in self.nodes.iter().enumerate() {
                if self.alive[i] && node.role() == Role::Leader {
                    let first = *self.leaders_by_term.entry(node.term()).or_insert(node.id());
                    assert_eq!(first, node.id(), "two leaders in term {:?}", node.term());
                }
            }
            self.check_logs();
        }

        fn check_logs(&self) {
            let n = self.nodes.len();
            for i in 0..n {
                let node = &self.nodes[i];
                assert!(
                    node.commit <= node.storage.last_index(),
                    "node {} commit beyond its log",
                    i + 1
                );
                for j in (i + 1)..n {
                    let a = &self.nodes[i].storage;
                    let b = &self.nodes[j].storage;
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
                        .filter(|e| e.index.0 + 1 >= node.storage.first_index().0)
                    {
                        assert_eq!(
                            node.storage.term_at(e.index),
                            Some(e.term),
                            "leader {} lost committed index {}",
                            i + 1,
                            e.index.0
                        );
                    }
                }
            }
        }

        fn collect(&mut self) {
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
                self.snapshots_sent += ready
                    .messages
                    .iter()
                    .filter(|m| matches!(m.body, Body::Snapshot { .. }))
                    .count();
                self.reads[i].extend(ready.read_states.iter().copied());
                if let Some(snap) = &ready.snapshot {
                    self.applied[i] = decode_state(&snap.data);
                    assert_eq!(self.applied[i].len() as u64, snap.meta.index.0, "snapshot state");
                }
                for e in &ready.committed_entries {
                    let have = self.applied[i].len();
                    if self.restarted[i] && e.index.0 as usize <= have {
                        assert_eq!(self.applied[i][e.index.0 as usize - 1], *e, "redelivery differs");
                    } else {
                        assert_eq!(e.index.0 as usize, have + 1, "gap or repeat in delivery");
                        self.applied[i].push(e.clone());
                    }
                }
                node.advance(ready);
            }
        }

        fn next_message(&mut self) -> Option<Message> {
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

        fn compact_node(&mut self, i: usize) {
            let index = Index(self.applied[i].len() as u64);
            let data = encode_state(&self.applied[i]);
            self.nodes[i].compact(index, data).unwrap();
        }

        fn read_index(&mut self, id: NodeId, ctx: u64) -> Result<(), ReadError> {
            let r = self.nodes[slot(id)].read_index(ctx);
            self.drain();
            r
        }

        fn propose_conf(&mut self, id: NodeId, change: ConfChange) -> Result<Index, ConfError> {
            let r = self.nodes[slot(id)].propose_conf_change(change);
            self.observe();
            self.drain();
            r
        }

        fn propose(&mut self, id: NodeId, data: Vec<u8>) -> Result<Index, NotLeader> {
            let r = self.nodes[slot(id)].propose(data);
            self.observe();
            self.drain();
            r
        }

        fn settle(&mut self, ticks: u32) {
            for _ in 0..ticks {
                self.tick();
            }
        }

        fn applied_data(&self, i: usize) -> Vec<Vec<u8>> {
            self.applied[i].iter().filter(|e| e.kind == EntryKind::Normal && !e.data.is_empty()).map(|e| e.data.clone()).collect()
        }

        fn drain(&mut self) {
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

        fn tick(&mut self) {
            for i in 0..self.nodes.len() {
                if self.alive[i] {
                    self.nodes[i].tick();
                }
            }
            self.observe();
            self.drain();
        }

        fn tick_only(&mut self, id: NodeId) {
            self.nodes[slot(id)].tick();
            self.observe();
            self.drain();
        }

        fn leader(&self) -> Option<NodeId> {
            self.nodes
                .iter()
                .enumerate()
                .filter(|(i, n)| {
                    self.alive[*i] && !self.isolated.contains(&n.id()) && n.role() == Role::Leader
                })
                .max_by_key(|(_, n)| n.term())
                .map(|(_, n)| n.id())
        }

        fn run_until_leader(&mut self, max_ticks: u32) -> Option<NodeId> {
            for _ in 0..max_ticks {
                self.tick();
                if let Some(l) = self.leader() {
                    return Some(l);
                }
            }
            None
        }
    }

    fn lone() -> Raft<MemStorage> {
        Raft::new(NodeId(1), ids(3), MemStorage::new(), 1).unwrap()
    }

    fn take(r: &mut Raft<MemStorage>) -> Vec<Message> {
        let ready = r.ready();
        let msgs = ready.messages.clone();
        r.advance(ready);
        msgs
    }

    fn vote(from: u64, term: u64) -> Message {
        Message {
            from: NodeId(from),
            to: NodeId(1),
            term: Term(term),
            body: Body::Vote { last_index: Index(0), last_term: Term(0) },
        }
    }

    fn prevote(from: u64, term: u64) -> Message {
        Message {
            from: NodeId(from),
            to: NodeId(1),
            term: Term(term),
            body: Body::PreVote { last_index: Index(0), last_term: Term(0) },
        }
    }

    fn vote_answer(r: &mut Raft<MemStorage>, m: Message) -> Option<bool> {
        r.step(m);
        take(r).into_iter().find_map(|x| match x.body {
            Body::VoteResp { granted } => Some(granted),
            _ => None,
        })
    }

    fn prevote_answer(r: &mut Raft<MemStorage>, m: Message) -> Option<bool> {
        r.step(m);
        take(r).into_iter().find_map(|x| match x.body {
            Body::PreVoteResp { granted } => Some(granted),
            _ => None,
        })
    }

    fn log_of(term: u64, len: u64) -> MemStorage {
        let mut s = MemStorage::new();
        s.set_hard_state(HardState { term: Term(term), vote: None, commit: Index(0) }).unwrap();
        let entries: Vec<Entry> = (1..=len)
            .map(|i| Entry { term: Term(term), index: Index(i), data: vec![i as u8], kind: EntryKind::Normal })
            .collect();
        s.append(&entries).unwrap();
        s
    }

    #[test]
    fn a_leader_is_elected_on_three_nodes_within_60_ticks() {
        let mut p = Pump::new(3, 1);
        assert!(p.run_until_leader(60).is_some());
    }

    #[test]
    fn a_leader_is_elected_on_five_nodes_within_60_ticks() {
        let mut p = Pump::new(5, 1);
        assert!(p.run_until_leader(60).is_some());
    }

    #[test]
    fn at_most_one_leader_per_term_across_200_seeds() {
        for seed in 0..200u64 {
            let n = if seed % 2 == 0 { 3 } else { 5 };
            let mut p = Pump::new(n, seed);
            let mut killed = None;
            for t in 0..120 {
                if t == 40 {
                    if let Some(l) = p.leader() {
                        p.alive[slot(l)] = false;
                        killed = Some(l);
                    }
                }
                if t == 70 {
                    if let Some(l) = killed {
                        p.alive[slot(l)] = true;
                    }
                }
                p.tick();
            }
            assert!(!p.leaders_by_term.is_empty(), "seed {seed} elected nobody");
        }
    }

    #[test]
    fn a_partitioned_node_that_keeps_ticking_never_raises_the_cluster_term_on_rejoin() {
        let mut p = Pump::new(3, 7);
        let leader = p.run_until_leader(60).expect("leader");
        for _ in 0..5 {
            p.tick();
        }
        let term = p.nodes[slot(leader)].term();
        let loner = ids(3).into_iter().find(|id| *id != leader).unwrap();
        p.isolated.insert(loner);
        for _ in 0..100 {
            p.tick();
        }
        assert_eq!(p.nodes[slot(loner)].term(), term);
        p.isolated.clear();
        for _ in 0..25 {
            p.tick_only(loner);
        }
        assert_eq!(p.nodes[slot(loner)].term(), term);
        for _ in 0..60 {
            p.tick();
        }
        assert!(p.nodes.iter().all(|n| n.term() == term));
        assert_eq!(p.leader(), Some(leader));
    }

    #[test]
    fn a_node_that_voted_in_a_term_refuses_a_different_candidate_in_that_term() {
        let mut r = lone();
        assert_eq!(vote_answer(&mut r, vote(2, 1)), Some(true));
        assert_eq!(vote_answer(&mut r, vote(3, 1)), Some(false));
        assert_eq!(vote_answer(&mut r, vote(2, 1)), Some(true));
        assert_eq!(vote_answer(&mut r, vote(3, 2)), Some(true));
    }

    #[test]
    fn a_node_whose_log_is_behind_never_wins() {
        for seed in 0..50u64 {
            let storages = vec![MemStorage::new(), log_of(1, 2), log_of(1, 2)];
            let mut p = Pump::with_storages(storages, seed);
            assert!(p.run_until_leader(80).is_some(), "seed {seed} elected nobody");
            assert!(
                p.leaders_by_term.values().all(|id| *id != NodeId(1)),
                "seed {seed} behind node won"
            );
        }
    }

    #[test]
    fn killing_the_leader_elects_a_new_one_with_a_higher_term() {
        let mut p = Pump::new(3, 3);
        let old = p.run_until_leader(60).expect("leader");
        let old_term = p.nodes[slot(old)].term();
        p.alive[slot(old)] = false;
        let new = p.run_until_leader(60).expect("new leader");
        assert_ne!(new, old);
        assert!(p.nodes[slot(new)].term() > old_term);
    }

    #[test]
    fn a_restarted_node_remembers_its_vote_and_term() {
        let mut r = lone();
        assert_eq!(vote_answer(&mut r, vote(2, 5)), Some(true));
        let mut again = Raft::new(NodeId(1), ids(3), r.storage().clone(), 99).unwrap();
        assert_eq!(again.term(), Term(5));
        assert_eq!(again.role(), Role::Follower);
        assert_eq!(again.storage().hard_state().vote, Some(NodeId(2)));
        assert_eq!(vote_answer(&mut again, vote(3, 5)), Some(false));
        assert_eq!(vote_answer(&mut again, vote(2, 5)), Some(true));
    }

    #[test]
    fn a_node_that_heard_a_live_leader_refuses_a_prevote() {
        let mut r = lone();
        r.step(Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(3),
            body: Body::Heartbeat { commit: Index(0), seq: 0 },
        });
        assert_eq!(r.leader(), Some(NodeId(2)));
        assert_eq!(prevote_answer(&mut r, prevote(3, 4)), Some(false));
        assert_eq!(r.term(), Term(3));
    }

    #[test]
    fn a_node_with_no_live_leader_grants_a_prevote_without_changing_term_or_vote() {
        let mut r = lone();
        assert_eq!(prevote_answer(&mut r, prevote(3, 1)), Some(true));
        assert_eq!(r.term(), Term(0));
        assert_eq!(r.storage().hard_state().vote, None);
    }

    fn ack(from: u64, term: u64, success: bool, matched: u64, hint: u64) -> Message {
        Message {
            from: NodeId(from),
            to: NodeId(1),
            term: Term(term),
            body: Body::AppendResp { success, match_index: Index(matched), hint: Index(hint) },
        }
    }

    fn append_msg(term: u64, prev_index: u64, prev_term: u64, entries: Vec<Entry>, commit: u64) -> Message {
        Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(term),
            body: Body::Append {
                prev_index: Index(prev_index),
                prev_term: Term(prev_term),
                entries,
                commit: Index(commit),
            },
        }
    }

    fn append_answer(r: &mut Raft<MemStorage>, m: Message) -> Option<(bool, Index, Index)> {
        r.step(m);
        take(r).into_iter().find_map(|x| match x.body {
            Body::AppendResp { success, match_index, hint } => Some((success, match_index, hint)),
            _ => None,
        })
    }

    fn indices(entries: &[Entry]) -> Vec<u64> {
        entries.iter().map(|e| e.index.0).collect()
    }

    fn old_entry_leader() -> Raft<MemStorage> {
        let mut s = MemStorage::new();
        s.set_hard_state(HardState { term: Term(3), vote: None, commit: Index(1) }).unwrap();
        s.append(&[entry(1, 1), entry(2, 2)]).unwrap();
        let mut r = Raft::new(NodeId(1), ids(3), s, 1).unwrap();
        while r.role() == Role::Follower {
            r.tick();
        }
        r.step(Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(4),
            body: Body::PreVoteResp { granted: true },
        });
        r.step(Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(4),
            body: Body::VoteResp { granted: true },
        });
        assert_eq!(r.role(), Role::Leader);
        assert_eq!(r.term(), Term(4));
        r
    }

    #[test]
    fn a_follower_truncates_a_conflicting_suffix_then_appends() {
        let mut r = Raft::new(NodeId(1), ids(3), log_of(1, 3), 1).unwrap();
        let answer = append_answer(&mut r, append_msg(2, 1, 1, vec![entry(2, 2)], 0));
        assert_eq!(answer, Some((true, Index(2), Index(2))));
        assert_eq!(r.storage().last_index(), Index(2));
        assert_eq!(r.storage().term_at(Index(1)), Some(Term(1)));
        assert_eq!(r.storage().term_at(Index(2)), Some(Term(2)));
    }

    #[test]
    fn a_duplicate_shorter_append_does_not_truncate_a_matching_suffix() {
        let mut r = Raft::new(NodeId(1), ids(3), log_of(1, 3), 1).unwrap();
        let answer = append_answer(&mut r, append_msg(1, 0, 0, vec![entry(1, 1)], 0));
        assert_eq!(answer, Some((true, Index(1), Index(1))));
        assert_eq!(r.storage().last_index(), Index(3));
    }

    #[test]
    fn a_follower_commits_min_of_leader_commit_and_its_last_new_entry_and_never_lowers_it() {
        let mut r = Raft::new(NodeId(1), ids(3), log_of(1, 2), 1).unwrap();
        let answer = append_answer(&mut r, append_msg(1, 2, 1, vec![entry(1, 3)], 9));
        assert_eq!(answer, Some((true, Index(3), Index(3))));
        assert_eq!(r.commit_index(), Index(3));
        let answer = append_answer(&mut r, append_msg(1, 3, 1, Vec::new(), 1));
        assert_eq!(answer, Some((true, Index(3), Index(3))));
        assert_eq!(r.commit_index(), Index(3));
        assert_eq!(r.storage().hard_state().commit, Index(3));
    }

    #[test]
    fn a_rejected_append_hints_the_start_of_the_conflicting_term_or_the_log_end() {
        let mut s = MemStorage::new();
        s.set_hard_state(HardState { term: Term(2), vote: None, commit: Index(0) }).unwrap();
        s.append(&[entry(1, 1), entry(1, 2), entry(2, 3), entry(2, 4), entry(2, 5)]).unwrap();
        let mut r = Raft::new(NodeId(1), ids(3), s, 1).unwrap();
        let answer = append_answer(&mut r, append_msg(3, 5, 3, Vec::new(), 0));
        assert_eq!(answer, Some((false, Index(0), Index(2))));
        let answer = append_answer(&mut r, append_msg(3, 9, 2, Vec::new(), 0));
        assert_eq!(answer, Some((false, Index(0), Index(5))));
    }

    #[test]
    fn a_leader_backs_up_to_the_hinted_index_and_resends_from_there() {
        let mut r = old_entry_leader();
        let _ = take(&mut r);
        r.step(ack(2, 4, false, 0, 0));
        let to_two: Vec<_> = take(&mut r).into_iter().filter(|m| m.to == NodeId(2)).collect();
        assert_eq!(to_two.len(), 1);
        match &to_two[0].body {
            Body::Append { prev_index, entries, .. } => {
                assert_eq!(*prev_index, Index(0));
                assert_eq!(indices(entries), vec![1, 2, 3]);
            }
            other => panic!("expected append, got {other:?}"),
        }
    }

    #[test]
    fn a_leader_steps_down_on_a_higher_term_response() {
        let mut r = old_entry_leader();
        r.step(ack(3, 9, false, 0, 0));
        assert_eq!(r.role(), Role::Follower);
        assert_eq!(r.term(), Term(9));
        assert_eq!(r.propose(vec![1]), Err(NotLeader { hint: None }));
    }

    #[test]
    fn propose_on_a_follower_returns_the_known_leader_as_hint() {
        let mut r = lone();
        assert_eq!(r.propose(vec![1]), Err(NotLeader { hint: None }));
        r.step(Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(3),
            body: Body::Heartbeat { commit: Index(0), seq: 0 },
        });
        assert_eq!(r.propose(vec![1]), Err(NotLeader { hint: Some(NodeId(2)) }));
    }

    #[test]
    fn propose_on_the_leader_appends_and_sends_only_the_new_entry_to_every_peer() {
        let mut r = old_entry_leader();
        let _ = take(&mut r);
        assert_eq!(r.propose(vec![9]), Ok(Index(4)));
        let sent = take(&mut r);
        assert_eq!(sent.len(), 2);
        for m in sent {
            match m.body {
                Body::Append { prev_index, prev_term, entries, .. } => {
                    assert_eq!(prev_index, Index(3));
                    assert_eq!(prev_term, Term(4));
                    assert_eq!(indices(&entries), vec![4]);
                }
                other => panic!("expected append, got {other:?}"),
            }
        }
    }

    #[test]
    fn committed_entries_are_delivered_once_and_only_after_advance() {
        let mut r = Raft::new(NodeId(1), ids(1), MemStorage::new(), 1).unwrap();
        while r.role() != Role::Leader {
            r.tick();
        }
        let first = r.ready();
        assert_eq!(indices(&first.committed_entries), vec![1]);
        assert_eq!(r.ready().committed_entries, first.committed_entries);
        r.advance(first);
        assert!(r.ready().committed_entries.is_empty());
        assert_eq!(r.propose(vec![5]), Ok(Index(2)));
        let next = r.ready();
        assert_eq!(indices(&next.committed_entries), vec![2]);
        assert_eq!(next.committed_entries[0].data, vec![5]);
        r.advance(next);
        assert!(r.ready().committed_entries.is_empty());
    }

    #[test]
    fn figure_8_an_old_term_entry_on_a_majority_is_not_committed_until_a_current_term_entry_is() {
        let mut r = old_entry_leader();
        let first = r.ready();
        assert_eq!(indices(&first.committed_entries), vec![1]);
        r.advance(first);
        r.step(ack(2, 4, true, 2, 2));
        assert_eq!(r.commit_index(), Index(1));
        assert!(r.ready().committed_entries.is_empty());
        assert_eq!(r.storage().term_at(Index(2)), Some(Term(2)));
        r.step(ack(2, 4, true, 3, 3));
        assert_eq!(r.commit_index(), Index(3));
        let ready = r.ready();
        assert_eq!(indices(&ready.committed_entries), vec![2, 3]);
        assert_eq!(ready.committed_entries[0].term, Term(2));
        assert_eq!(ready.committed_entries[1].term, Term(4));
    }

    #[test]
    fn one_thousand_proposals_commit_on_all_three_replicas_in_identical_order() {
        let mut p = Pump::new(3, 11);
        let leader = p.run_until_leader(60).expect("leader");
        let expected: Vec<Vec<u8>> = (0..1000u32).map(payload).collect();
        for data in &expected {
            p.propose(leader, data.clone()).expect("leader accepts");
        }
        p.settle(10);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), expected, "replica {}", i + 1);
        }
    }

    #[test]
    fn a_lagging_follower_catches_up_after_missing_200_entries() {
        let mut p = Pump::new(3, 21);
        let leader = p.run_until_leader(60).expect("leader");
        let lag = ids(3).into_iter().find(|id| *id != leader).unwrap();
        p.isolated.insert(lag);
        for i in 0..200 {
            p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.settle(30);
        let ahead = p.applied_data(slot(leader));
        assert_eq!(ahead.len(), 200);
        assert!(p.applied_data(slot(lag)).is_empty());
        p.isolated.clear();
        p.settle(40);
        assert_eq!(p.applied_data(slot(lag)), ahead);
        assert_eq!(p.nodes[slot(lag)].storage.log, p.nodes[slot(leader)].storage.log);
    }

    #[test]
    fn killing_the_leader_loses_no_committed_entry_and_the_new_leader_accepts_proposals() {
        let mut p = Pump::new(3, 41);
        let old = p.run_until_leader(60).expect("leader");
        let first: Vec<Vec<u8>> = (0..50u32).map(|i| tagged(1, i)).collect();
        for data in &first {
            p.propose(old, data.clone()).expect("leader accepts");
        }
        p.settle(10);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), first);
        }
        p.alive[slot(old)] = false;
        let new = p.run_until_leader(80).expect("new leader");
        assert_ne!(new, old);
        let second: Vec<Vec<u8>> = (0..50u32).map(|i| tagged(2, i)).collect();
        for data in &second {
            p.propose(new, data.clone()).expect("new leader accepts");
        }
        p.settle(10);
        let both: Vec<Vec<u8>> = first.iter().chain(second.iter()).cloned().collect();
        for i in (0..3).filter(|i| *i != slot(old)) {
            assert_eq!(p.applied_data(i), both);
        }
        p.alive[slot(old)] = true;
        p.settle(40);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), both, "replica {}", i + 1);
        }
        assert_eq!(p.nodes[slot(old)].role(), Role::Follower);
    }

    #[test]
    fn a_minority_partition_cannot_commit_and_its_uncommitted_entries_are_overwritten_after_heal() {
        let mut p = Pump::new(3, 31);
        let old = p.run_until_leader(60).expect("leader");
        let committed: Vec<Vec<u8>> = (0..5u32).map(|i| tagged(1, i)).collect();
        for data in &committed {
            p.propose(old, data.clone()).expect("leader accepts");
        }
        p.settle(10);
        assert_eq!(p.applied_data(slot(old)), committed);
        let commit_before = p.nodes[slot(old)].commit_index();
        p.isolated.insert(old);
        for i in 0..10u32 {
            p.propose(old, tagged(2, i)).expect("stale leader still accepts");
        }
        p.settle(5);
        assert_eq!(p.nodes[slot(old)].commit_index(), commit_before);
        assert_eq!(p.applied_data(slot(old)), committed);
        let new = p.run_until_leader(80).expect("majority elects a leader");
        assert_ne!(new, old);
        let fresh: Vec<Vec<u8>> = (0..5u32).map(|i| tagged(3, i)).collect();
        for data in &fresh {
            p.propose(new, data.clone()).expect("new leader accepts");
        }
        p.settle(10);
        let expected: Vec<Vec<u8>> = committed.iter().chain(fresh.iter()).cloned().collect();
        assert!(p.nodes[slot(old)].storage.log.iter().any(|e| e.data.first() == Some(&2)));
        p.isolated.clear();
        p.settle(40);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), expected, "replica {}", i + 1);
        }
        assert!(p.nodes[slot(old)].storage.log.iter().all(|e| e.data.first() != Some(&2)));
        assert_eq!(p.nodes[slot(old)].role(), Role::Follower);
    }

    #[test]
    fn duplicated_and_reordered_delivery_keeps_log_matching_across_20_seeds() {
        for seed in 0..20u64 {
            let mut p = Pump::new(3, seed);
            p.chaos = Some(Rng::new(seed ^ 0xC0FFEE));
            let mut killed = None;
            for t in 0..200u32 {
                if t == 80 {
                    if let Some(l) = p.leader() {
                        p.alive[slot(l)] = false;
                        killed = Some(l);
                    }
                }
                if t == 130 {
                    if let Some(l) = killed {
                        p.alive[slot(l)] = true;
                    }
                }
                if t % 2 == 0 {
                    if let Some(l) = p.leader() {
                        let _ = p.propose(l, tagged(7, t));
                    }
                }
                p.tick();
            }
            p.chaos = None;
            p.settle(80);
            let a = p.applied_data(0);
            assert!(!a.is_empty(), "seed {seed} committed nothing");
            assert_eq!(a, p.applied_data(1), "seed {seed}");
            assert_eq!(a, p.applied_data(2), "seed {seed}");
        }
    }

    #[test]
    fn a_leader_cut_off_from_a_majority_steps_down_when_check_quorum_is_on() {
        let mut p = Pump::new(3, 51);
        for n in p.nodes.iter_mut() {
            n.set_check_quorum(true);
        }
        let leader = p.run_until_leader(60).expect("leader");
        p.isolated.insert(leader);
        p.settle(100);
        assert_ne!(p.nodes[slot(leader)].role(), Role::Leader);
        assert!(p.run_until_leader(80).is_some());
    }

    #[test]
    fn a_healthy_leader_keeps_leading_for_200_ticks_with_check_quorum_on() {
        let mut p = Pump::new(3, 52);
        for n in p.nodes.iter_mut() {
            n.set_check_quorum(true);
        }
        let leader = p.run_until_leader(60).expect("leader");
        let term = p.nodes[slot(leader)].term();
        p.settle(200);
        assert_eq!(p.leader(), Some(leader));
        assert_eq!(p.nodes[slot(leader)].term(), term);
    }

    fn crash_cluster(
        target: usize,
        fail_at: Option<u64>,
        seed: u64,
    ) -> (u64, Pump<FileStorage<SimDisk>>) {
        let disks: Vec<SimDisk> = (0..3).map(|_| SimDisk::default()).collect();
        let storages =
            disks.iter().map(|d| FileStorage::open(d.clone(), 4).unwrap()).collect::<Vec<_>>();
        let mut p = Pump::with_storages(storages, seed);
        disks[target].fail_at(fail_at);
        p.run_until_leader(80);
        for i in 0..15u32 {
            if let Some(l) = p.leader() {
                let _ = p.propose(l, tagged(1, i));
            }
            p.tick();
        }
        p.settle(10);
        for i in 0..10u32 {
            if let Some(l) = p.leader() {
                let _ = p.propose(l, tagged(2, i));
            }
            p.tick();
        }
        let before = (0..3).map(|i| p.applied_data(i)).max_by_key(|a| a.len()).unwrap();
        let ops = disks[target].ops();
        disks[target].crash(&mut Rng::new(seed ^ fail_at.unwrap_or(0xFFFF)));
        let reopened = FileStorage::open(disks[target].clone(), 4).unwrap();
        p.restart(target, reopened, seed + 1000);
        p.settle(150);
        let after = p.applied_data(0);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), after, "replica {} after restart, fail_at {fail_at:?}", i + 1);
        }
        assert!(
            after.starts_with(&before),
            "a committed entry was lost: fail_at {fail_at:?} target {target}"
        );
        assert!(p.leader().is_some(), "no leader after heal, fail_at {fail_at:?}");
        (ops, p)
    }

    #[test]
    fn a_crash_at_every_write_boundary_keeps_election_safety_and_loses_no_committed_entry() {
        for seed in [3u64, 8] {
            for target in 0..3usize {
                let (ops, clean) = crash_cluster(target, None, seed);
                assert!(ops > 20, "scenario wrote too little to be a test: {ops}");
                assert!(!clean.applied_data(0).is_empty());
                for k in 0..=ops {
                    crash_cluster(target, Some(k), seed);
                }
            }
        }
    }

    #[test]
    fn consecutive_configurations_always_have_overlapping_quorums_and_two_step_jumps_do_not() {
        for n in 1..64usize {
            for m in [n + 1, n.saturating_sub(1).max(1)] {
                assert!(quorum_of(n) + quorum_of(m) > n.max(m), "{n} -> {m}");
            }
        }
        assert!(quorum_of(3) + quorum_of(5) <= 5);
    }

    #[test]
    fn a_lagging_follower_catches_up_from_a_snapshot_after_the_leader_compacts_its_log() {
        let mut p = Pump::new(3, 61);
        let leader = p.run_until_leader(60).expect("leader");
        let lag = ids(3).into_iter().find(|id| *id != leader).unwrap();
        let other = ids(3).into_iter().find(|id| *id != leader && *id != lag).unwrap();
        p.isolated.insert(lag);
        for i in 0..300 {
            p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.settle(10);
        p.compact_node(slot(leader));
        p.compact_node(slot(other));
        assert!(p.nodes[slot(leader)].storage.first_index() > Index(250));
        assert_eq!(p.snapshots_sent, 0);
        p.isolated.clear();
        p.settle(60);
        assert!(p.snapshots_sent >= 1);
        let expected = p.applied_data(slot(leader));
        assert_eq!(expected.len(), 300);
        assert_eq!(p.applied_data(slot(lag)), expected);
        assert!(p.nodes[slot(lag)].storage.first_index() > Index(250));
        for i in 300..320 {
            p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.settle(20);
        for n in 0..3 {
            assert_eq!(p.applied_data(n).len(), 320, "replica {}", n + 1);
        }
    }

    #[test]
    fn compaction_is_refused_beyond_applied_or_at_or_below_the_previous_snapshot() {
        let mut r = Raft::new(NodeId(1), ids(1), MemStorage::new(), 1).unwrap();
        while r.role() != Role::Leader {
            r.tick();
        }
        for i in 0..5u8 {
            r.propose(vec![i + 1]).unwrap();
        }
        let ready = r.ready();
        r.advance(ready);
        assert_eq!(r.applied_index(), Index(6));
        assert!(r.compact(Index(7), vec![]).is_err());
        assert!(r.compact(Index(4), vec![7, 7]).is_ok());
        assert!(r.compact(Index(4), vec![7, 7]).is_err());
        assert!(r.compact(Index(3), vec![7]).is_err());
        assert!(r.fault().is_none());
        assert_eq!(r.storage().first_index(), Index(5));
        assert_eq!(r.storage().snapshot_meta().index, Index(4));
        assert_eq!(r.storage().snapshot_meta().members, vec![NodeId(1)]);
        assert_eq!(r.storage().snapshot_data().unwrap(), vec![7, 7]);
        assert_eq!(r.storage().term_at(Index(4)), Some(r.term()));
        assert_eq!(r.storage().term_at(Index(3)), None);
        assert_eq!(r.propose(vec![9]), Ok(Index(7)));
    }

    #[test]
    fn a_second_membership_change_is_refused_until_the_first_has_committed() {
        let mut p = Pump::with_members(5, 3, 71);
        let leader = p.run_until_leader(60).expect("leader");
        p.settle(5);
        let l = slot(leader);
        assert!(p.nodes[l].propose_conf_change(ConfChange::Add(NodeId(4))).is_ok());
        assert_eq!(
            p.nodes[l].propose_conf_change(ConfChange::Add(NodeId(5))),
            Err(ConfError::Pending)
        );
        p.drain();
        p.settle(10);
        assert_eq!(p.propose_conf(leader, ConfChange::Add(NodeId(5))).map(|_| ()), Ok(()));
        p.settle(20);
        for i in 0..5 {
            assert_eq!(members_of(&p.nodes[i]), vec![1, 2, 3, 4, 5], "node {}", i + 1);
        }
    }

    #[test]
    fn a_leader_refuses_a_membership_change_until_it_has_committed_in_its_own_term() {
        let mut r = old_entry_leader();
        assert_eq!(r.propose_conf_change(ConfChange::Add(NodeId(4))), Err(ConfError::NotReady));
        r.step(ack(2, 4, true, 3, 3));
        assert_eq!(r.commit_index(), Index(3));
        assert_eq!(r.propose_conf_change(ConfChange::Add(NodeId(1))), Err(ConfError::Invalid));
        assert_eq!(r.propose_conf_change(ConfChange::Remove(NodeId(1))), Err(ConfError::Invalid));
        assert_eq!(r.propose_conf_change(ConfChange::Remove(NodeId(9))), Err(ConfError::Invalid));
        assert_eq!(r.propose_conf_change(ConfChange::Add(NodeId(4))), Ok(Index(4)));
        assert_eq!(
            r.propose_conf_change(ConfChange::Remove(NodeId(2))),
            Err(ConfError::Pending)
        );
        assert_eq!(members_of(&r), vec![1, 2, 3, 4]);
    }

    #[test]
    fn adding_a_node_raises_the_quorum_to_three_of_four_and_the_new_node_converges() {
        let mut p = Pump::with_members(4, 3, 72);
        let leader = p.run_until_leader(60).expect("leader");
        p.settle(5);
        for i in 0..10 {
            p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.propose_conf(leader, ConfChange::Add(NodeId(4))).expect("change accepted");
        p.settle(20);
        for i in 10..20 {
            p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.settle(20);
        let expected = p.applied_data(slot(leader));
        assert_eq!(expected.len(), 20);
        for i in 0..4 {
            assert_eq!(p.applied_data(i), expected, "replica {}", i + 1);
            assert_eq!(members_of(&p.nodes[i]), vec![1, 2, 3, 4]);
        }
        assert!(p.nodes[3].is_voter());
        let spare = ids(3).into_iter().find(|id| *id != leader).unwrap();
        p.alive[slot(leader)] = false;
        p.alive[slot(spare)] = false;
        assert!(p.run_until_leader(150).is_none());
        p.alive[slot(spare)] = true;
        assert!(p.run_until_leader(150).is_some());
    }

    #[test]
    fn removing_a_node_shrinks_the_quorum_to_two_of_two() {
        let mut p = Pump::new(3, 73);
        let leader = p.run_until_leader(60).expect("leader");
        p.settle(5);
        let gone = ids(3).into_iter().find(|id| *id != leader).unwrap();
        let stay = ids(3).into_iter().find(|id| *id != leader && *id != gone).unwrap();
        p.propose_conf(leader, ConfChange::Remove(gone)).expect("change accepted");
        p.settle(20);
        assert_eq!(members_of(&p.nodes[slot(leader)]), vec![leader.0.min(stay.0), leader.0.max(stay.0)]);
        let term = p.nodes[slot(leader)].term();
        p.settle(150);
        assert_eq!(p.leader(), Some(leader));
        assert_eq!(p.nodes[slot(leader)].term(), term);
        p.alive[slot(gone)] = false;
        p.propose(leader, payload(1)).expect("leader accepts");
        p.settle(10);
        let before = p.nodes[slot(leader)].commit_index();
        p.alive[slot(stay)] = false;
        p.propose(leader, payload(2)).expect("leader accepts");
        p.settle(30);
        assert_eq!(p.nodes[slot(leader)].commit_index(), before);
    }

    #[test]
    fn a_removed_node_is_refused_a_vote_by_the_members() {
        let mut p = Pump::new(3, 74);
        let leader = p.run_until_leader(60).expect("leader");
        p.settle(5);
        let gone = ids(3).into_iter().find(|id| *id != leader).unwrap();
        p.propose_conf(leader, ConfChange::Remove(gone)).expect("change accepted");
        p.settle(20);
        let term = p.nodes[slot(leader)].term();
        let node = &mut p.nodes[slot(leader)];
        node.step(Message {
            from: gone,
            to: leader,
            term: Term(term.0 + 1),
            body: Body::Vote { last_index: Index(1 << 20), last_term: Term(1 << 20) },
        });
        let ready = node.ready();
        let answer = ready.messages.iter().find_map(|m| match m.body {
            Body::VoteResp { granted } => Some(granted),
            _ => None,
        });
        assert_eq!(answer, Some(false));
    }

    #[test]
    fn an_uncommitted_membership_change_on_a_cut_off_leader_cannot_form_a_second_majority_and_is_reverted() {
        for seed in 0..30u64 {
            let mut p = Pump::with_members(4, 3, seed);
            let old = p.run_until_leader(60).expect("leader");
            p.settle(5);
            for i in 0..5 {
                p.propose(old, tagged(1, i)).expect("leader accepts");
            }
            p.settle(5);
            let before = p.nodes[slot(old)].commit_index();
            assert!(p.nodes[slot(old)].propose_conf_change(ConfChange::Add(NodeId(4))).is_ok());
            p.isolated.insert(old);
            p.drain();
            assert_eq!(members_of(&p.nodes[slot(old)]), vec![1, 2, 3, 4]);
            let new = p.run_until_leader(120).expect("old majority elects a leader");
            assert_ne!(new, old);
            for i in 0..5 {
                p.propose(new, tagged(2, i)).expect("new leader accepts");
            }
            p.settle(10);
            assert_eq!(p.nodes[slot(old)].commit_index(), before, "seed {seed}");
            assert!(p.nodes[slot(new)].commit_index() > before, "seed {seed}");
            p.isolated.clear();
            p.settle(80);
            assert_eq!(members_of(&p.nodes[slot(old)]), vec![1, 2, 3], "seed {seed}");
            assert_ne!(p.nodes[slot(old)].role(), Role::Leader);
            let a = p.applied_data(0);
            assert_eq!(a.len(), 10, "seed {seed}");
            assert_eq!(p.applied_data(1), a);
            assert_eq!(p.applied_data(2), a);
            assert!(p.applied_data(3).is_empty());
        }
    }

    #[test]
    fn a_node_added_after_compaction_learns_the_configuration_from_the_snapshot() {
        let mut p = Pump::with_members(4, 3, 75);
        p.isolated.insert(NodeId(4));
        let leader = p.run_until_leader(60).expect("leader");
        p.settle(5);
        p.propose_conf(leader, ConfChange::Add(NodeId(4))).expect("change accepted");
        p.settle(10);
        for i in 0..40 {
            p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.settle(10);
        for i in 0..3 {
            p.compact_node(i);
        }
        assert!(p.nodes[slot(leader)].storage.first_index() > Index(40));
        assert!(!p.nodes[3].is_voter());
        p.isolated.clear();
        p.settle(60);
        assert!(p.snapshots_sent >= 1);
        assert_eq!(members_of(&p.nodes[3]), vec![1, 2, 3, 4]);
        assert!(p.nodes[3].is_voter());
        assert_eq!(p.applied_data(3), p.applied_data(slot(leader)));
        assert_eq!(p.applied_data(3).len(), 40);
    }

    #[test]
    fn a_read_index_returns_a_position_no_older_than_the_last_committed_write() {
        let mut p = Pump::new(3, 81);
        let leader = p.run_until_leader(60).expect("leader");
        let mut last = Index(0);
        for i in 0..10 {
            last = p.propose(leader, payload(i)).expect("leader accepts");
        }
        p.settle(5);
        assert!(p.nodes[slot(leader)].commit_index() >= last);
        p.read_index(leader, 77).expect("leader accepts reads");
        let reads = &p.reads[slot(leader)];
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].ctx, 77);
        assert!(reads[0].index >= last);
        assert!(p.applied[slot(leader)].len() as u64 >= reads[0].index.0);
        let follower = ids(3).into_iter().find(|id| *id != leader).unwrap();
        assert_eq!(p.read_index(follower, 1), Err(ReadError::NotLeader(Some(leader))));
    }

    #[test]
    fn a_cut_off_leader_cannot_confirm_a_read_while_the_majority_has_moved_on() {
        let mut p = Pump::new(3, 82);
        let old = p.run_until_leader(60).expect("leader");
        for i in 0..3 {
            p.propose(old, tagged(1, i)).expect("leader accepts");
        }
        p.settle(10);
        p.isolated.insert(old);
        p.read_index(old, 5).expect("leader accepts reads");
        let new = p.run_until_leader(80).expect("majority elects a leader");
        assert_ne!(new, old);
        for i in 0..3 {
            p.propose(new, tagged(2, i)).expect("new leader accepts");
        }
        p.settle(10);
        assert_eq!(p.nodes[slot(old)].role(), Role::Leader);
        assert!(p.reads[slot(old)].is_empty());
        p.isolated.clear();
        p.settle(40);
        assert!(p.reads[slot(old)].is_empty());
        assert_ne!(p.nodes[slot(old)].role(), Role::Leader);
    }

    #[test]
    fn a_read_waits_for_a_commit_in_the_leaders_term_and_then_for_a_heartbeat_quorum() {
        let mut r = old_entry_leader();
        r.read_index(5).expect("accepted");
        assert!(r.ready().read_states.is_empty());
        r.step(ack(2, 4, true, 3, 3));
        let ready = r.ready();
        assert!(ready.read_states.is_empty());
        let seq = ready.messages.iter().find_map(|m| match m.body {
            Body::Heartbeat { seq, .. } => Some(seq),
            _ => None,
        });
        assert_eq!(seq, Some(1));
        r.step(Message {
            from: NodeId(2),
            to: NodeId(1),
            term: Term(4),
            body: Body::HeartbeatResp { seq: 1 },
        });
        let ready = r.ready();
        assert_eq!(ready.read_states, vec![ReadState { ctx: 5, index: Index(3) }]);
        r.advance(ready);
        assert!(r.ready().read_states.is_empty());
    }

    #[test]
    fn a_single_node_confirms_a_read_without_any_heartbeat() {
        let mut r = Raft::new(NodeId(1), ids(1), MemStorage::new(), 1).unwrap();
        while r.role() != Role::Leader {
            r.tick();
        }
        r.read_index(9).expect("accepted");
        let commit = r.commit_index();
        assert_eq!(r.ready().read_states, vec![ReadState { ctx: 9, index: commit }]);
    }
}
