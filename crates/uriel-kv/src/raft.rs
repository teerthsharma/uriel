use crate::rng::Rng;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Term(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Index(pub u64);

impl Index {
    fn plus(self, n: u64) -> Index {
        Index(self.0 + n)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not leader (hint: {hint:?})")]
pub struct NotLeader {
    pub hint: Option<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub term: Term,
    pub index: Index,
    pub data: Vec<u8>,
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
    Heartbeat { commit: Index },
    HeartbeatResp,
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
    fn set_hard_state(&mut self, hs: HardState);
    fn last_index(&self) -> Index;
    fn term_at(&self, index: Index) -> Option<Term>;
    fn entries(&self, from: Index, to: Index) -> Vec<Entry>;
    fn append(&mut self, entries: &[Entry]);
    fn truncate_from(&mut self, index: Index);
}

#[derive(Debug, Clone, Default)]
pub struct MemStorage {
    hs: HardState,
    log: Vec<Entry>,
}

impl MemStorage {
    pub fn new() -> Self {
        MemStorage::default()
    }
}

impl Storage for MemStorage {
    fn hard_state(&self) -> HardState {
        self.hs
    }

    fn set_hard_state(&mut self, hs: HardState) {
        self.hs = hs;
    }

    fn last_index(&self) -> Index {
        Index(self.log.len() as u64)
    }

    fn term_at(&self, index: Index) -> Option<Term> {
        if index.0 == 0 {
            return Some(Term(0));
        }
        self.log.get(index.0 as usize - 1).map(|e| e.term)
    }

    fn entries(&self, from: Index, to: Index) -> Vec<Entry> {
        let lo = (from.0.max(1) - 1) as usize;
        let hi = (to.0.saturating_sub(1) as usize).min(self.log.len());
        if lo >= hi {
            return Vec::new();
        }
        self.log[lo..hi].to_vec()
    }

    fn append(&mut self, entries: &[Entry]) {
        self.log.extend(entries.iter().cloned());
    }

    fn truncate_from(&mut self, index: Index) {
        let keep = index.0.saturating_sub(1) as usize;
        self.log.truncate(keep);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub messages: Vec<Message>,
    pub hard_state_changed: bool,
    pub committed_entries: Vec<Entry>,
}

const ELECTION_MIN: u64 = 10;
const ELECTION_MAX: u64 = 19;
const HEARTBEAT_INTERVAL: u64 = 2;
const MAX_BATCH: u64 = 100;

pub struct Raft<S: Storage> {
    id: NodeId,
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
}

impl<S: Storage> Raft<S> {
    pub fn new(id: NodeId, peers: Vec<NodeId>, storage: S, seed: u64) -> Self {
        let hs = storage.hard_state();
        let mut rng = Rng::new(seed.wrapping_add(id.0));
        let election_timeout = rng.between(ELECTION_MIN, ELECTION_MAX);
        Raft {
            id,
            peers: peers.into_iter().filter(|p| *p != id).collect(),
            storage,
            role: Role::Follower,
            term: hs.term,
            vote: hs.vote,
            commit: hs.commit,
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
            applied: Index(0),
            check_quorum: false,
            quorum_elapsed: 0,
            active: BTreeSet::new(),
        }
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

    pub fn propose(&mut self, data: Vec<u8>) -> Result<Index, NotLeader> {
        if self.role != Role::Leader {
            return Err(NotLeader { hint: self.leader });
        }
        let index = self.storage.last_index().plus(1);
        let entry = Entry { term: self.term, index, data };
        self.storage.append(std::slice::from_ref(&entry));
        for i in 0..self.peers.len() {
            let peer = self.peers[i];
            self.send_append(peer);
        }
        self.maybe_commit();
        Ok(index)
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

    fn quorum(&self) -> usize {
        self.peers.len().div_ceil(2) + 1
    }

    fn persist(&mut self) {
        self.storage.set_hard_state(HardState {
            term: self.term,
            vote: self.vote,
            commit: self.commit,
        });
        self.dirty = true;
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

    fn become_follower(&mut self, term: Term, leader: Option<NodeId>) {
        if term > self.term {
            self.term = term;
            self.vote = None;
            self.persist();
        }
        self.role = Role::Follower;
        self.leader = leader;
        self.granted.clear();
        self.heartbeat_elapsed = 0;
        self.reset_timer();
    }

    fn accept_leader(&mut self, leader: NodeId) {
        self.role = Role::Follower;
        self.leader = Some(leader);
        self.granted.clear();
        self.election_elapsed = 0;
    }

    fn start_prevote(&mut self) {
        self.role = Role::PreCandidate;
        self.leader = None;
        self.granted.clear();
        self.granted.insert(self.id);
        self.reset_timer();
        if self.granted.len() >= self.quorum() {
            self.start_election();
            return;
        }
        let body = Body::PreVote {
            last_index: self.storage.last_index(),
            last_term: self.last_term(),
        };
        self.broadcast(Term(self.term.0 + 1), body);
    }

    fn start_election(&mut self) {
        self.term = Term(self.term.0 + 1);
        self.vote = Some(self.id);
        self.persist();
        self.role = Role::Candidate;
        self.leader = None;
        self.granted.clear();
        self.granted.insert(self.id);
        self.reset_timer();
        if self.granted.len() >= self.quorum() {
            self.become_leader();
            return;
        }
        let body = Body::Vote {
            last_index: self.storage.last_index(),
            last_term: self.last_term(),
        };
        self.broadcast(self.term, body);
    }

    fn become_leader(&mut self) {
        self.role = Role::Leader;
        self.leader = Some(self.id);
        self.granted.clear();
        self.heartbeat_elapsed = 0;
        self.active.clear();
        self.quorum_elapsed = 0;
        let prev_index = self.storage.last_index();
        self.next_index.clear();
        self.match_index.clear();
        for i in 0..self.peers.len() {
            let peer = self.peers[i];
            self.next_index.insert(peer, prev_index.plus(1));
            self.match_index.insert(peer, Index(0));
        }
        let noop = Entry { term: self.term, index: prev_index.plus(1), data: Vec::new() };
        self.storage.append(std::slice::from_ref(&noop));
        for i in 0..self.peers.len() {
            let peer = self.peers[i];
            self.send_append(peer);
        }
        self.maybe_commit();
    }

    fn send_append(&mut self, peer: NodeId) {
        let last = self.storage.last_index();
        let next = self.next_index.get(&peer).copied().unwrap_or(last.plus(1)).max(Index(1));
        if next > last {
            return;
        }
        let prev_index = Index(next.0 - 1);
        let prev_term = self.storage.term_at(prev_index).unwrap_or_default();
        let upto = Index(last.0.min(next.0 + MAX_BATCH - 1));
        let entries = self.storage.entries(next, upto.plus(1));
        self.next_index.insert(peer, upto.plus(1));
        let commit = self.commit;
        self.send(peer, self.term, Body::Append { prev_index, prev_term, entries, commit });
    }

    fn maybe_commit(&mut self) {
        if self.role != Role::Leader {
            return;
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
            self.persist();
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
    ) {
        self.accept_leader(from);
        let last = self.storage.last_index();
        if prev_index > last || self.storage.term_at(prev_index) != Some(prev_term) {
            let hint = self.conflict_hint(prev_index);
            let body = Body::AppendResp { success: false, match_index: Index(0), hint };
            self.send(from, self.term, body);
            return;
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
                self.storage.truncate_from(first);
            }
            self.storage.append(&entries[skip..]);
        }
        let target = leader_commit.min(last_new);
        if target > self.commit {
            self.commit = target;
            self.persist();
        }
        let body = Body::AppendResp { success: true, match_index: last_new, hint: last_new };
        self.send(from, self.term, body);
    }

    fn handle_append_resp(&mut self, from: NodeId, success: bool, match_index: Index, hint: Index) {
        if self.role != Role::Leader || !self.next_index.contains_key(&from) {
            return;
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
            self.maybe_commit();
        } else {
            let next = self.next_index.get(&from).copied().unwrap_or(last.plus(1));
            let lowered = next.min(hint.plus(1)).max(matched.plus(1));
            self.next_index.insert(from, lowered);
        }
        self.send_append(from);
    }

    fn handle_heartbeat_resp(&mut self, from: NodeId) {
        if self.role != Role::Leader || !self.next_index.contains_key(&from) {
            return;
        }
        self.active.insert(from);
        let matched = self.match_index.get(&from).copied().unwrap_or_default();
        if matched < self.storage.last_index() {
            self.next_index.insert(from, matched.plus(1));
            self.send_append(from);
        }
    }

    fn reject_stale(&mut self, msg: &Message) {
        let body = match msg.body {
            Body::PreVote { .. } => Body::PreVoteResp { granted: false },
            Body::Vote { .. } => Body::VoteResp { granted: false },
            Body::Append { .. } => Body::AppendResp {
                success: false,
                match_index: Index(0),
                hint: self.storage.last_index(),
            },
            Body::Heartbeat { .. } => Body::HeartbeatResp,
            _ => return,
        };
        self.send(msg.from, self.term, body);
    }

    pub fn tick(&mut self) {
        if self.role == Role::Leader {
            if self.check_quorum {
                self.quorum_elapsed += 1;
                if self.quorum_elapsed >= 2 * self.election_timeout {
                    let heard = self.active.len() + 1;
                    self.active.clear();
                    self.quorum_elapsed = 0;
                    if heard < self.quorum() {
                        self.become_follower(self.term, None);
                        return;
                    }
                }
            }
            self.heartbeat_elapsed += 1;
            if self.heartbeat_elapsed >= HEARTBEAT_INTERVAL {
                self.heartbeat_elapsed = 0;
                for i in 0..self.peers.len() {
                    let peer = self.peers[i];
                    let matched = self.match_index.get(&peer).copied().unwrap_or_default();
                    let commit = self.commit.min(matched);
                    self.send(peer, self.term, Body::Heartbeat { commit });
                }
            }
        } else {
            self.election_elapsed += 1;
            if self.election_elapsed >= self.election_timeout {
                self.start_prevote();
            }
        }
    }

    pub fn step(&mut self, msg: Message) {
        if msg.term > self.term {
            match msg.body {
                Body::PreVote { .. } | Body::PreVoteResp { granted: true } => {}
                Body::Append { .. } | Body::Heartbeat { .. } => {
                    self.become_follower(msg.term, Some(msg.from))
                }
                _ => self.become_follower(msg.term, None),
            }
        } else if msg.term < self.term {
            self.reject_stale(&msg);
            return;
        }
        match msg.body {
            Body::PreVote { last_index, last_term } => {
                let grant = msg.term > self.term
                    && !self.leader_alive()
                    && self.log_is_up_to_date(last_index, last_term);
                let term = if grant { msg.term } else { self.term };
                self.send(msg.from, term, Body::PreVoteResp { granted: grant });
            }
            Body::PreVoteResp { granted } => {
                if self.role == Role::PreCandidate
                    && granted
                    && msg.term == Term(self.term.0 + 1)
                {
                    self.granted.insert(msg.from);
                    if self.granted.len() >= self.quorum() {
                        self.start_election();
                    }
                }
            }
            Body::Vote { last_index, last_term } => {
                let free = self.vote.is_none() || self.vote == Some(msg.from);
                let grant = free && self.log_is_up_to_date(last_index, last_term);
                if grant {
                    self.vote = Some(msg.from);
                    self.persist();
                    self.election_elapsed = 0;
                }
                self.send(msg.from, self.term, Body::VoteResp { granted: grant });
            }
            Body::VoteResp { granted } => {
                if self.role == Role::Candidate && granted {
                    self.granted.insert(msg.from);
                    if self.granted.len() >= self.quorum() {
                        self.become_leader();
                    }
                }
            }
            Body::Append { prev_index, prev_term, entries, commit } => {
                self.handle_append(msg.from, prev_index, prev_term, entries, commit);
            }
            Body::AppendResp { success, match_index, hint } => {
                self.handle_append_resp(msg.from, success, match_index, hint);
            }
            Body::Heartbeat { commit } => {
                self.accept_leader(msg.from);
                let target = commit.min(self.storage.last_index());
                if target > self.commit {
                    self.commit = target;
                    self.persist();
                }
                self.send(msg.from, self.term, Body::HeartbeatResp);
            }
            Body::HeartbeatResp => self.handle_heartbeat_resp(msg.from),
        }
    }

    pub fn ready(&self) -> Ready {
        let committed_entries = self
            .storage
            .entries(self.applied.plus(1), self.commit.plus(1));
        Ready {
            messages: self.outbox.clone(),
            hard_state_changed: self.dirty,
            committed_entries,
        }
    }

    pub fn advance(&mut self, ready: Ready) {
        let n = ready.messages.len().min(self.outbox.len());
        self.outbox.drain(..n);
        self.dirty = false;
        if let Some(last) = ready.committed_entries.last() {
            self.applied = self.applied.max(last.index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};

    fn ids(n: u64) -> Vec<NodeId> {
        (1..=n).map(NodeId).collect()
    }

    struct Pump {
        nodes: Vec<Raft<MemStorage>>,
        alive: Vec<bool>,
        isolated: BTreeSet<NodeId>,
        queue: VecDeque<Message>,
        leaders_by_term: BTreeMap<Term, NodeId>,
        applied: Vec<Vec<Entry>>,
        chaos: Option<Rng>,
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
        Entry { term: Term(term), index: Index(index), data: vec![index as u8] }
    }

    impl Pump {
        fn new(n: u64, seed: u64) -> Self {
            Pump::with_storages((0..n).map(|_| MemStorage::new()).collect(), seed)
        }

        fn with_storages(storages: Vec<MemStorage>, seed: u64) -> Self {
            let all = ids(storages.len() as u64);
            let nodes: Vec<_> = storages
                .into_iter()
                .zip(all.iter())
                .map(|(s, id)| Raft::new(*id, all.clone(), s, seed))
                .collect();
            let n = nodes.len();
            Pump {
                nodes,
                alive: vec![true; n],
                isolated: BTreeSet::new(),
                queue: VecDeque::new(),
                leaders_by_term: BTreeMap::new(),
                applied: vec![Vec::new(); n],
                chaos: None,
            }
        }

        fn slot(id: NodeId) -> usize {
            id.0 as usize - 1
        }

        fn reachable(&self, a: NodeId, b: NodeId) -> bool {
            self.alive[Pump::slot(a)]
                && self.alive[Pump::slot(b)]
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
                    let a = &self.nodes[i].storage.log;
                    let b = &self.nodes[j].storage.log;
                    let shared = a.len().min(b.len());
                    if let Some(k) = (0..shared).rev().find(|k| a[*k].term == b[*k].term) {
                        assert_eq!(a[..=k], b[..=k], "log matching broken between {} and {}", i + 1, j + 1);
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
                    let log = &node.storage.log;
                    for e in &self.applied[longest] {
                        assert_eq!(
                            log.get(e.index.0 as usize - 1),
                            Some(e),
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
                self.queue.extend(ready.messages.iter().cloned());
                for e in &ready.committed_entries {
                    assert_eq!(e.index.0 as usize, self.applied[i].len() + 1, "gap or repeat in delivery");
                    self.applied[i].push(e.clone());
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

        fn propose(&mut self, id: NodeId, data: Vec<u8>) -> Result<Index, NotLeader> {
            let r = self.nodes[Pump::slot(id)].propose(data);
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
            self.applied[i].iter().filter(|e| !e.data.is_empty()).map(|e| e.data.clone()).collect()
        }

        fn drain(&mut self) {
            self.collect();
            let mut steps = 0;
            while let Some(m) = self.next_message() {
                steps += 1;
                assert!(steps < 100_000, "message storm");
                if self.reachable(m.from, m.to) {
                    let i = Pump::slot(m.to);
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
            self.nodes[Pump::slot(id)].tick();
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
        Raft::new(NodeId(1), ids(3), MemStorage::new(), 1)
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
        s.set_hard_state(HardState { term: Term(term), vote: None, commit: Index(0) });
        let entries: Vec<Entry> = (1..=len)
            .map(|i| Entry { term: Term(term), index: Index(i), data: vec![i as u8] })
            .collect();
        s.append(&entries);
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
                        p.alive[Pump::slot(l)] = false;
                        killed = Some(l);
                    }
                }
                if t == 70 {
                    if let Some(l) = killed {
                        p.alive[Pump::slot(l)] = true;
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
        let term = p.nodes[Pump::slot(leader)].term();
        let loner = ids(3).into_iter().find(|id| *id != leader).unwrap();
        p.isolated.insert(loner);
        for _ in 0..100 {
            p.tick();
        }
        assert_eq!(p.nodes[Pump::slot(loner)].term(), term);
        p.isolated.clear();
        for _ in 0..25 {
            p.tick_only(loner);
        }
        assert_eq!(p.nodes[Pump::slot(loner)].term(), term);
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
        let old_term = p.nodes[Pump::slot(old)].term();
        p.alive[Pump::slot(old)] = false;
        let new = p.run_until_leader(60).expect("new leader");
        assert_ne!(new, old);
        assert!(p.nodes[Pump::slot(new)].term() > old_term);
    }

    #[test]
    fn a_restarted_node_remembers_its_vote_and_term() {
        let mut r = lone();
        assert_eq!(vote_answer(&mut r, vote(2, 5)), Some(true));
        let mut again = Raft::new(NodeId(1), ids(3), r.storage().clone(), 99);
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
            body: Body::Heartbeat { commit: Index(0) },
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
        s.set_hard_state(HardState { term: Term(3), vote: None, commit: Index(1) });
        s.append(&[entry(1, 1), entry(2, 2)]);
        let mut r = Raft::new(NodeId(1), ids(3), s, 1);
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
        let mut r = Raft::new(NodeId(1), ids(3), log_of(1, 3), 1);
        let answer = append_answer(&mut r, append_msg(2, 1, 1, vec![entry(2, 2)], 0));
        assert_eq!(answer, Some((true, Index(2), Index(2))));
        assert_eq!(r.storage().last_index(), Index(2));
        assert_eq!(r.storage().term_at(Index(1)), Some(Term(1)));
        assert_eq!(r.storage().term_at(Index(2)), Some(Term(2)));
    }

    #[test]
    fn a_duplicate_shorter_append_does_not_truncate_a_matching_suffix() {
        let mut r = Raft::new(NodeId(1), ids(3), log_of(1, 3), 1);
        let answer = append_answer(&mut r, append_msg(1, 0, 0, vec![entry(1, 1)], 0));
        assert_eq!(answer, Some((true, Index(1), Index(1))));
        assert_eq!(r.storage().last_index(), Index(3));
    }

    #[test]
    fn a_follower_commits_min_of_leader_commit_and_its_last_new_entry_and_never_lowers_it() {
        let mut r = Raft::new(NodeId(1), ids(3), log_of(1, 2), 1);
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
        s.set_hard_state(HardState { term: Term(2), vote: None, commit: Index(0) });
        s.append(&[entry(1, 1), entry(1, 2), entry(2, 3), entry(2, 4), entry(2, 5)]);
        let mut r = Raft::new(NodeId(1), ids(3), s, 1);
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
            body: Body::Heartbeat { commit: Index(0) },
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
        let mut r = Raft::new(NodeId(1), ids(1), MemStorage::new(), 1);
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
        let ahead = p.applied_data(Pump::slot(leader));
        assert_eq!(ahead.len(), 200);
        assert!(p.applied_data(Pump::slot(lag)).is_empty());
        p.isolated.clear();
        p.settle(40);
        assert_eq!(p.applied_data(Pump::slot(lag)), ahead);
        assert_eq!(p.nodes[Pump::slot(lag)].storage.log, p.nodes[Pump::slot(leader)].storage.log);
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
        p.alive[Pump::slot(old)] = false;
        let new = p.run_until_leader(80).expect("new leader");
        assert_ne!(new, old);
        let second: Vec<Vec<u8>> = (0..50u32).map(|i| tagged(2, i)).collect();
        for data in &second {
            p.propose(new, data.clone()).expect("new leader accepts");
        }
        p.settle(10);
        let both: Vec<Vec<u8>> = first.iter().chain(second.iter()).cloned().collect();
        for i in (0..3).filter(|i| *i != Pump::slot(old)) {
            assert_eq!(p.applied_data(i), both);
        }
        p.alive[Pump::slot(old)] = true;
        p.settle(40);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), both, "replica {}", i + 1);
        }
        assert_eq!(p.nodes[Pump::slot(old)].role(), Role::Follower);
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
        assert_eq!(p.applied_data(Pump::slot(old)), committed);
        let commit_before = p.nodes[Pump::slot(old)].commit_index();
        p.isolated.insert(old);
        for i in 0..10u32 {
            p.propose(old, tagged(2, i)).expect("stale leader still accepts");
        }
        p.settle(5);
        assert_eq!(p.nodes[Pump::slot(old)].commit_index(), commit_before);
        assert_eq!(p.applied_data(Pump::slot(old)), committed);
        let new = p.run_until_leader(80).expect("majority elects a leader");
        assert_ne!(new, old);
        let fresh: Vec<Vec<u8>> = (0..5u32).map(|i| tagged(3, i)).collect();
        for data in &fresh {
            p.propose(new, data.clone()).expect("new leader accepts");
        }
        p.settle(10);
        let expected: Vec<Vec<u8>> = committed.iter().chain(fresh.iter()).cloned().collect();
        assert!(p.nodes[Pump::slot(old)].storage.log.iter().any(|e| e.data.first() == Some(&2)));
        p.isolated.clear();
        p.settle(40);
        for i in 0..3 {
            assert_eq!(p.applied_data(i), expected, "replica {}", i + 1);
        }
        assert!(p.nodes[Pump::slot(old)].storage.log.iter().all(|e| e.data.first() != Some(&2)));
        assert_eq!(p.nodes[Pump::slot(old)].role(), Role::Follower);
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
                        p.alive[Pump::slot(l)] = false;
                        killed = Some(l);
                    }
                }
                if t == 130 {
                    if let Some(l) = killed {
                        p.alive[Pump::slot(l)] = true;
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
        assert_ne!(p.nodes[Pump::slot(leader)].role(), Role::Leader);
        assert!(p.run_until_leader(80).is_some());
    }

    #[test]
    fn a_healthy_leader_keeps_leading_for_200_ticks_with_check_quorum_on() {
        let mut p = Pump::new(3, 52);
        for n in p.nodes.iter_mut() {
            n.set_check_quorum(true);
        }
        let leader = p.run_until_leader(60).expect("leader");
        let term = p.nodes[Pump::slot(leader)].term();
        p.settle(200);
        assert_eq!(p.leader(), Some(leader));
        assert_eq!(p.nodes[Pump::slot(leader)].term(), term);
    }
}
