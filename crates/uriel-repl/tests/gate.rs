mod common;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;
use std::time::Instant;

use common::{build_rep, digest, MemBackend, Node, Rep};
use uriel_core::{Command, ConstraintKind, Graph, MemoryBudget, NodeId, Props, Value, WriteBatch};
use uriel_kv::raft::{Body, Index, Message, Role, Storage, Term};
use uriel_kv::rng::Rng;
use uriel_kv::sim::{slot, SimDisk};
use uriel_query::{AccessMode, Engine};
use uriel_repl::{Ack, Ticket, WriteError};
#[cfg(feature = "mutations")]
use uriel_repl::Mutation;

const NODES: usize = 3;
const DROP_PER_MILLE: u64 = 80;
const DUP_PER_MILLE: u64 = 30;
const MAX_DELAY: u64 = 5;
const LONG_DELAY: u64 = 60;
const HEAL_ROUNDS: u64 = 300;
const PROOF_ROUNDS: u64 = 80;
const NO_EPOCH: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "mutations"), allow(dead_code))]
enum Mut {
    None,
    ApplyOnAppend,
    FatalRedelivery,
    AckBeforeCommit,
    SkipAppliedPersist,
    LocalRead,
    IgnoreSnapshot,
}

#[derive(Debug, Clone, Copy)]
struct Cfg {
    seed: u64,
    steps: u64,
    reads: bool,
    compaction: bool,
    mutation: Mut,
    applied_durable_check: bool,
}

fn clean(seed: u64) -> Cfg {
    Cfg {
        seed,
        steps: 2_000,
        reads: true,
        compaction: true,
        mutation: Mut::None,
        applied_durable_check: true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Violation {
    seed: u64,
    step: u64,
    what: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "violation at seed {} step {}: {}", self.seed, self.step, self.what)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Report {
    trace: u64,
    acked: u64,
    unknown: u64,
    rejected: u64,
    refused: u64,
    committed_epochs: u64,
    restarts: u64,
    partitions: u64,
    disk_faults: u64,
    crashes_before_finish: u64,
    redelivered: u64,
    snapshots_installed: u64,
    compactions: u64,
    reads: u64,
}

impl Report {
    fn add(&mut self, o: &Report) {
        self.acked += o.acked;
        self.unknown += o.unknown;
        self.rejected += o.rejected;
        self.refused += o.refused;
        self.committed_epochs += o.committed_epochs;
        self.restarts += o.restarts;
        self.partitions += o.partitions;
        self.disk_faults += o.disk_faults;
        self.crashes_before_finish += o.crashes_before_finish;
        self.redelivered += o.redelivered;
        self.snapshots_installed += o.snapshots_installed;
        self.compactions += o.compactions;
        self.reads += o.reads;
    }
}

type Check = Result<(), Violation>;

struct Flight {
    due: u64,
    msg: Message,
}

struct Oracle {
    graph: Graph,
    hi: u64,
    log: BTreeMap<u64, (Term, u64)>,
    digests: Vec<String>,
    markers: Vec<u64>,
}

impl Oracle {
    fn new() -> Oracle {
        let graph = Graph::open_in_memory(MemoryBudget::mib(16)).expect("oracle opens");
        let first = digest(&graph);
        Oracle { graph, hi: 0, log: BTreeMap::new(), digests: vec![first], markers: Vec::new() }
    }

    fn epoch(&self) -> u64 {
        self.graph.epoch()
    }
}

struct PendingRead {
    node: usize,
    min_epoch: u64,
    min_markers: usize,
}

struct Sim {
    cfg: Cfg,
    rng: Rng,
    nodes: Vec<Node>,
    net: Vec<Flight>,
    step: u64,
    group: Vec<u8>,
    down_until: Vec<Option<u64>>,
    quiet: bool,
    trace: u64,
    report: Report,
    oracle: Oracle,
    markers: Vec<BTreeMap<Ticket, u64>>,
    acked: BTreeMap<u64, u64>,
    max_acked_epoch: u64,
    finished_applied: Vec<Index>,
    checked: Vec<u64>,
    pending_reads: BTreeMap<u64, PendingRead>,
    ctx: u64,
    marker_counter: u64,
    crash_after: Vec<usize>,
    leaders_by_term: BTreeMap<Term, usize>,
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
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
        Body::Snapshot { meta, data } => (9, meta.index.0, fnv(data)),
    };
    let mut h = m.from.0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for x in [m.to.0, m.term.0, tag, a, b] {
        h = (h ^ x).wrapping_mul(0x100_0000_01b3).rotate_left(13);
    }
    h
}

fn marker_of(batch: &WriteBatch) -> u64 {
    for c in &batch.commands {
        if let Command::CreateNode { labels, props, .. } = c {
            if labels.contains("W") {
                if let Some(Value::Int(k)) = props.get("k") {
                    return *k as u64;
                }
            }
        }
    }
    NO_EPOCH
}

fn props1(key: &str, v: Value) -> Props {
    let mut p = Props::new();
    p.insert(key.to_string(), v);
    p
}

fn plan(b: &mut uriel_repl::BatchBuilder, graph: &Graph, rng: &mut Rng, k: u64) {
    if graph.epoch() == 0 {
        b.create_index("W", "k");
        b.create_index("P", "name");
        b.create_constraint(ConstraintKind::Unique, "U", "id");
    }
    let read = graph.begin_read().expect("read");
    let mut plain: Vec<NodeId> = Vec::new();
    let mut every: Vec<NodeId> = Vec::new();
    for n in read.all_nodes().expect("nodes").take(400) {
        let n = n.expect("node");
        every.push(n.id);
        if !n.labels.contains("W") {
            plain.push(n.id);
        }
    }
    let mut rels = Vec::new();
    let mut touched = BTreeSet::new();
    for r in read.all_rels().expect("rels").take(400) {
        let r = r.expect("rel");
        touched.insert(r.src);
        touched.insert(r.dst);
        rels.push(r.id);
    }
    drop(read);
    let marker = b.create_node(&["W"], props1("k", Value::Int(k as i64)));
    every.push(marker);
    let pick = |rng: &mut Rng, items: &[NodeId]| -> Option<NodeId> {
        (!items.is_empty()).then(|| items[rng.below(items.len() as u64) as usize])
    };
    for _ in 0..rng.between(0, 3) {
        match rng.below(8) {
            0 => {
                let id = Value::Int(rng.below(40) as i64);
                b.create_node(&["U"], props1("id", id));
            }
            1 => {
                let name = Value::String(format!("n{}", rng.below(20)));
                b.create_node(&["P"], props1("name", name));
            }
            2 => {
                if let (Some(s), Some(d)) = (pick(rng, &every), pick(rng, &every)) {
                    b.create_rel(s, d, "R", props1("w", Value::Int(rng.below(9) as i64)));
                }
            }
            3 => {
                if let Some(n) = pick(rng, &every) {
                    b.set_node_props(n, props1("p", Value::Int(rng.below(100) as i64)));
                }
            }
            4 => {
                if let Some(n) = pick(rng, &plain) {
                    if rng.chance(1, 2) {
                        b.add_label(n, "Q");
                    } else {
                        b.remove_label(n, "Q");
                    }
                }
            }
            5 => {
                if !rels.is_empty() {
                    b.delete_rel(uriel_core::RelId(rels[rng.below(rels.len() as u64) as usize].0));
                }
            }
            6 => {
                let free: Vec<NodeId> =
                    plain.iter().copied().filter(|n| !touched.contains(n)).collect();
                if let Some(n) = pick(rng, &free) {
                    b.delete_node(n);
                }
            }
            _ => {
                b.set_node_props(NodeId(1_000_000), props1("p", Value::Int(1)));
            }
        }
    }
}

impl Sim {
    fn new(cfg: Cfg) -> Sim {
        let nodes = (0..NODES)
            .map(|i| {
                let disk = SimDisk::default();
                let backend = Rc::new(RefCell::new(MemBackend::default()));
                let rep = Self::make_rep(cfg, i, &disk, &backend, cfg.seed).expect("fresh node");
                Node { disk, backend, rep, up: true }
            })
            .collect();
        Sim {
            cfg,
            rng: Rng::new(cfg.seed ^ 0x8E_0000_0000_0000),
            nodes,
            net: Vec::new(),
            step: 0,
            group: vec![0; NODES],
            down_until: vec![None; NODES],
            quiet: false,
            trace: 0xcbf2_9ce4_8422_2325,
            report: Report::default(),
            oracle: Oracle::new(),
            markers: vec![BTreeMap::new(); NODES],
            acked: BTreeMap::new(),
            max_acked_epoch: 0,
            finished_applied: vec![Index(0); NODES],
            checked: vec![NO_EPOCH; NODES],
            pending_reads: BTreeMap::new(),
            ctx: 0,
            marker_counter: 0,
            crash_after: Vec::new(),
            leaders_by_term: BTreeMap::new(),
        }
    }

    fn make_rep(
        cfg: Cfg,
        i: usize,
        disk: &SimDisk,
        backend: &Rc<RefCell<MemBackend>>,
        seed: u64,
    ) -> Result<Rep, uriel_kv::raft::StorageError> {
        let drop_applied = cfg.mutation == Mut::SkipAppliedPersist;
        #[cfg_attr(not(feature = "mutations"), allow(unused_mut))]
        let mut rep = build_rep(i as u64 + 1, NODES as u64, disk, backend, seed, drop_applied)?;
        #[cfg(feature = "mutations")]
        match cfg.mutation {
            Mut::ApplyOnAppend => rep.set_mutation(Some(Mutation::ApplyOnAppend)),
            Mut::FatalRedelivery => rep.set_mutation(Some(Mutation::FatalRedelivery)),
            Mut::AckBeforeCommit => rep.set_mutation(Some(Mutation::AckBeforeCommit)),
            Mut::LocalRead => rep.set_mutation(Some(Mutation::LocalRead)),
            Mut::IgnoreSnapshot => rep.set_mutation(Some(Mutation::IgnoreSnapshot)),
            Mut::None | Mut::SkipAppliedPersist => {}
        }
        Ok(rep)
    }

    fn bad<T>(&self, what: String) -> Result<T, Violation> {
        Err(Violation { seed: self.cfg.seed, step: self.step, what })
    }

    fn mix(&mut self, x: u64) {
        self.trace = (self.trace ^ x).wrapping_mul(0x100_0000_01b3).rotate_left(7);
    }

    fn pick_leader(&mut self) -> Option<usize> {
        let leaders: Vec<usize> = (0..NODES)
            .filter(|i| self.nodes[*i].up && self.nodes[*i].rep.raft().role() == Role::Leader)
            .collect();
        if leaders.is_empty() {
            return None;
        }
        Some(leaders[self.rng.below(leaders.len() as u64) as usize])
    }

    fn take_down(&mut self, i: usize) {
        if !self.nodes[i].up {
            return;
        }
        self.nodes[i].up = false;
        let wait = self.rng.between(3, 80);
        self.down_until[i] = Some(self.step + wait);
    }

    fn restart(&mut self, i: usize) -> Check {
        let seed = self.rng.next_u64();
        let mut r = self.rng.fork();
        self.nodes[i].disk.crash(&mut r);
        let (disk, backend) = (self.nodes[i].disk.clone(), Rc::clone(&self.nodes[i].backend));
        let rep = match Self::make_rep(self.cfg, i, &disk, &backend, seed) {
            Ok(rep) => rep,
            Err(e) => return self.bad(format!("node {} cannot reopen: {e}", i + 1)),
        };
        let was = self.finished_applied[i];
        if self.cfg.applied_durable_check && rep.raft().applied_index() < was {
            return self.bad(format!(
                "node {} lost its applied index across a restart: persisted {} but had advanced to {}",
                i + 1,
                rep.raft().applied_index().0,
                was.0
            ));
        }
        self.nodes[i].rep = rep;
        self.nodes[i].up = true;
        self.down_until[i] = None;
        self.markers[i].clear();
        self.checked[i] = NO_EPOCH;
        self.report.restarts += 1;
        self.mix(0x7e57_0000 ^ i as u64);
        Ok(())
    }

    fn refresh_oracle(&mut self) -> Check {
        let top = (0..NODES)
            .filter(|i| self.nodes[*i].up)
            .map(|i| self.nodes[i].rep.raft().commit_index().0)
            .max()
            .unwrap_or(0);
        while self.oracle.hi < top {
            let idx = self.oracle.hi + 1;
            let mut found = None;
            for i in 0..NODES {
                let n = &self.nodes[i];
                let st = n.rep.raft().storage();
                if !n.up || n.rep.raft().commit_index().0 < idx {
                    continue;
                }
                if st.first_index().0 > idx || st.last_index().0 < idx {
                    continue;
                }
                let Ok(es) = st.entries(Index(idx), Index(idx + 1)) else { continue };
                let Some(e) = es.into_iter().next() else { continue };
                match &found {
                    None => found = Some(e),
                    Some(f) if f.term != e.term || f.data != e.data => {
                        return self.bad(format!("committed entry {idx} differs between replicas"));
                    }
                    Some(_) => {}
                }
            }
            let Some(e) = found else {
                return self.bad(format!("no replica holds committed entry {idx}"));
            };
            self.oracle.log.insert(idx, (e.term, fnv(&e.data)));
            self.oracle.hi = idx;
            if e.kind == uriel_kv::raft::EntryKind::Normal && !e.data.is_empty() {
                let batch = match WriteBatch::decode(&e.data) {
                    Ok(b) => b,
                    Err(err) => return self.bad(format!("committed entry {idx} undecodable: {err}")),
                };
                if let Err(err) = self.oracle.graph.apply_batch(&batch) {
                    return self.bad(format!(
                        "committed batch at index {idx} (epoch {}) does not apply to the oracle: {err}",
                        batch.epoch
                    ));
                }
                self.oracle.markers.push(marker_of(&batch));
                let d = digest(&self.oracle.graph);
                self.oracle.digests.push(d);
            }
        }
        Ok(())
    }

    fn ack(&mut self, node: usize, ticket: Ticket, ack: Ack) -> Check {
        match ack {
            Ack::Unknown => {
                self.report.unknown += 1;
                self.mix(0xa0);
            }
            Ack::Committed { epoch } => {
                let Some(marker) = self.markers[node].get(&ticket).copied() else {
                    return self.bad(format!("node {} acked an unknown ticket {}", node + 1, ticket.0));
                };
                if epoch == 0 || epoch > self.oracle.epoch() {
                    return self.bad(format!(
                        "write {marker} acknowledged at epoch {epoch} but only {} epochs are committed",
                        self.oracle.epoch()
                    ));
                }
                let committed = self.oracle.markers[epoch as usize - 1];
                if committed != marker {
                    return self.bad(format!(
                        "write {marker} acknowledged at epoch {epoch} but the committed batch there is {committed}"
                    ));
                }
                self.acked.insert(marker, epoch);
                self.max_acked_epoch = self.max_acked_epoch.max(epoch);
                self.report.acked += 1;
                self.mix(0xac00 ^ marker ^ (epoch << 20));
            }
        }
        Ok(())
    }

    fn serve_read(&mut self, node: usize, ctx: u64) -> Check {
        let Some(pr) = self.pending_reads.remove(&ctx) else { return Ok(()) };
        if pr.node != node {
            return Ok(());
        }
        let graph = self.nodes[node].rep.graph().clone();
        if graph.epoch() < pr.min_epoch {
            return self.bad(format!(
                "stale read on node {}: epoch {} below acknowledged {}",
                node + 1,
                graph.epoch(),
                pr.min_epoch
            ));
        }
        let engine = Engine::from_graph(graph.clone());
        let rows = match engine.execute("MATCH (n:W) RETURN count(n)", &BTreeMap::new(), AccessMode::Read) {
            Ok(r) => r.rows,
            Err(e) => return self.bad(format!("read query failed on node {}: {e}", node + 1)),
        };
        let count = match rows.first().and_then(|r| r.first()) {
            Some(Value::Int(n)) => *n as usize,
            other => return self.bad(format!("unexpected read result {other:?}")),
        };
        if count < pr.min_markers {
            return self.bad(format!(
                "stale read on node {}: {count} marker nodes but {} writes were acknowledged",
                node + 1,
                pr.min_markers
            ));
        }
        self.report.reads += 1;
        self.mix(0x4ead ^ count as u64);
        Ok(())
    }

    fn check_nodes(&mut self) -> Check {
        for i in 0..NODES {
            if !self.nodes[i].up {
                continue;
            }
            if let Some(f) = self.nodes[i].rep.fault() {
                return self.bad(format!("node {} latched a fault: {f}", i + 1));
            }
            let e = self.nodes[i].rep.graph().epoch();
            if e == self.checked[i] {
                continue;
            }
            if e > self.oracle.epoch() {
                return self.bad(format!(
                    "node {} graph is at epoch {e} but only {} epochs are committed",
                    i + 1,
                    self.oracle.epoch()
                ));
            }
            let d = digest(self.nodes[i].rep.graph());
            if d != self.oracle.digests[e as usize] {
                return self.bad(format!(
                    "node {} graph at epoch {e} differs from the committed log's state at that epoch",
                    i + 1
                ));
            }
            self.checked[i] = e;
        }
        for i in 0..NODES {
            let node = &self.nodes[i].rep;
            if self.nodes[i].up && node.raft().role() == Role::Leader {
                let first = *self.leaders_by_term.entry(node.raft().term()).or_insert(i);
                if first != i {
                    return self.bad(format!("two leaders in term {}", node.raft().term().0));
                }
            }
        }
        Ok(())
    }

    fn send(&mut self, msgs: Vec<Message>) {
        for m in msgs {
            if self.quiet {
                self.net.push(Flight { due: self.step, msg: m });
                continue;
            }
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
        }
    }

    fn collect(&mut self) -> Check {
        self.refresh_oracle()?;
        for i in 0..NODES {
            if !self.nodes[i].up {
                continue;
            }
            let staged = self.nodes[i].rep.stage();
            if let Some(f) = self.nodes[i].rep.fault() {
                return self.bad(format!("node {} latched a fault: {f}", i + 1));
            }
            if self.nodes[i].rep.raft().fault().is_some() {
                self.report.disk_faults += 1;
                self.take_down(i);
                continue;
            }
            let acks = self.nodes[i].rep.drain_acks();
            for (t, a) in acks {
                self.ack(i, t, a)?;
            }
            for ctx in self.nodes[i].rep.drain_reads() {
                self.serve_read(i, ctx)?;
            }
            let Some(staged) = staged else { continue };
            if !self.quiet && staged.entries() > 0 && self.rng.chance(1, 25) {
                self.report.crashes_before_finish += 1;
                self.take_down(i);
                continue;
            }
            let msgs = staged.messages().to_vec();
            self.nodes[i].rep.finish(staged);
            if self.nodes[i].rep.raft().fault().is_some() {
                self.report.disk_faults += 1;
                self.take_down(i);
            } else {
                self.finished_applied[i] = self.nodes[i].rep.raft().applied_index();
            }
            self.send(msgs);
        }
        Ok(())
    }

    fn deliver_one(&mut self, msg: Message) {
        let (from, to) = (slot(msg.from), slot(msg.to));
        if !self.nodes[to].up || self.group[from] != self.group[to] {
            return;
        }
        self.mix(msg_key(&msg));
        self.nodes[to].rep.step(msg);
        if !self.quiet && self.rng.chance(1, 80) {
            self.crash_after.push(to);
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

    fn tick_nodes(&mut self) {
        for i in 0..NODES {
            if self.nodes[i].up && (self.quiet || self.rng.chance(7, 8)) {
                self.nodes[i].rep.tick();
            }
        }
    }

    fn client_write(&mut self) {
        let leader = self.pick_leader();
        let i = match leader {
            Some(l) if self.rng.chance(9, 10) => l,
            _ => self.rng.below(NODES as u64) as usize,
        };
        if !self.nodes[i].up {
            return;
        }
        self.marker_counter += 1;
        let k = self.marker_counter;
        let mut r = Rng::new(self.rng.next_u64());
        let graph = self.nodes[i].rep.graph().clone();
        match self.nodes[i].rep.write(|b| plan(b, &graph, &mut r, k)) {
            Ok(t) => {
                self.markers[i].insert(t, k);
                self.mix(0xb0 ^ k);
            }
            Err(WriteError::Rejected(_)) => self.report.rejected += 1,
            Err(_) => self.report.refused += 1,
        }
    }

    fn client_read(&mut self) {
        let Some(i) = self.pick_leader() else { return };
        self.ctx += 1;
        if self.nodes[i].rep.read_index(self.ctx).is_ok() {
            self.pending_reads.insert(
                self.ctx,
                PendingRead { node: i, min_epoch: self.max_acked_epoch, min_markers: self.acked.len() },
            );
        }
    }

    fn one_step(&mut self) -> Check {
        self.step += 1;
        let split = self.group.iter().any(|g| *g != self.group[0]);
        if split {
            if self.rng.chance(1, 450) {
                self.group.iter_mut().for_each(|g| *g = 0);
            }
        } else if self.rng.chance(1, 250) {
            for g in self.group.iter_mut() {
                *g = self.rng.below(2) as u8;
            }
            self.report.partitions += 1;
        }
        if self.rng.chance(1, 400) {
            let i = self.rng.below(NODES as u64) as usize;
            self.take_down(i);
        }
        if self.rng.chance(1, 300) {
            let i = self.rng.below(NODES as u64) as usize;
            let after = self.rng.below(4);
            if self.nodes[i].up {
                self.nodes[i].disk.fail_at(Some(after));
            }
        }
        if self.cfg.compaction && self.rng.chance(1, 40) {
            let i = self.rng.below(NODES as u64) as usize;
            if self.nodes[i].up && matches!(self.nodes[i].rep.compact(), Ok(true)) {
                self.report.compactions += 1;
            }
        }
        for i in 0..NODES {
            if self.down_until[i].is_some_and(|t| t <= self.step) {
                self.restart(i)?;
            }
        }
        let roll = self.rng.below(1000);
        self.mix(roll);
        match roll {
            0..=199 => self.tick_nodes(),
            200..=699 => {
                for _ in 0..1 + self.rng.below(8) {
                    self.deliver_next();
                }
            }
            700..=839 => self.client_write(),
            840..=879 if self.cfg.reads => self.client_read(),
            _ => self.deliver_next(),
        }
        self.collect()?;
        for i in std::mem::take(&mut self.crash_after) {
            self.take_down(i);
        }
        self.check_nodes()
    }

    fn flush_net(&mut self) -> Check {
        let mut guard = 0;
        while !self.net.is_empty() && guard < 20_000 {
            guard += 1;
            let f = self.net.remove(0);
            self.deliver_one(f.msg);
            self.collect()?;
        }
        Ok(())
    }

    fn quiet_round(&mut self) -> Check {
        self.step += 1;
        self.tick_nodes();
        self.collect()?;
        self.flush_net()?;
        self.check_nodes()
    }

    fn heal(&mut self) -> Check {
        self.quiet = true;
        self.group.iter_mut().for_each(|g| *g = 0);
        for i in 0..NODES {
            if !self.nodes[i].up {
                self.restart(i)?;
            }
            self.nodes[i].disk.fail_at(None);
        }
        self.crash_after.clear();
        for _ in 0..HEAL_ROUNDS {
            self.quiet_round()?;
        }
        for _ in 0..2 {
            let Some(l) = self.pick_leader() else {
                return self.bad("no leader after the cluster healed".into());
            };
            let before = self.oracle.epoch();
            let mut waited = 0;
            loop {
                let ok = self.nodes[l].rep.raft().role() == Role::Leader;
                self.marker_counter += 1;
                let k = self.marker_counter;
                let mut r = Rng::new(self.rng.next_u64());
                let graph = self.nodes[l].rep.graph().clone();
                match self.nodes[l].rep.write(|b| plan(b, &graph, &mut r, k)) {
                    Ok(t) => {
                        self.markers[l].insert(t, k);
                        break;
                    }
                    Err(WriteError::Busy | WriteError::Rejected(_)) if ok && waited < 30 => {
                        waited += 1;
                        self.quiet_round()?;
                    }
                    Err(e) => return self.bad(format!("the healed leader refused a write: {e}")),
                }
            }
            for _ in 0..PROOF_ROUNDS {
                self.quiet_round()?;
            }
            if self.oracle.epoch() <= before {
                return self.bad("the healed cluster does not commit".into());
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Check {
        let want = self.oracle.digests[self.oracle.epoch() as usize].clone();
        for i in 0..NODES {
            let g = self.nodes[i].rep.graph();
            if g.epoch() != self.oracle.epoch() || digest(g) != want {
                return self.bad(format!(
                    "node {} ended at epoch {} with a different state than the committed log at epoch {}",
                    i + 1,
                    g.epoch(),
                    self.oracle.epoch()
                ));
            }
            let read = g.begin_read().expect("read");
            for (k, epoch) in &self.acked {
                let found = read
                    .nodes_by_prop("W", "k", &Value::Int(*k as i64))
                    .expect("index lookup")
                    .count();
                if found != 1 {
                    return self.bad(format!(
                        "acknowledged write {k} (epoch {epoch}) is missing on node {}",
                        i + 1
                    ));
                }
            }
        }
        for (k, epoch) in &self.acked {
            if self.oracle.markers[*epoch as usize - 1] != *k {
                return self.bad(format!("acknowledged write {k} is not at epoch {epoch} in the committed log"));
            }
        }
        Ok(())
    }
}

fn run(cfg: Cfg) -> Result<Report, Violation> {
    let mut sim = Sim::new(cfg);
    for _ in 0..cfg.steps {
        sim.one_step()?;
    }
    sim.heal()?;
    sim.finish()?;
    for (i, n) in sim.nodes.iter().enumerate() {
        sim.report.redelivered += n.rep.redelivered();
        sim.report.snapshots_installed += n.rep.snapshots_installed();
        let _ = i;
    }
    let digest_bytes = sim.oracle.digests.last().map(|d| fnv(d.as_bytes())).unwrap_or(0);
    sim.mix(digest_bytes);
    sim.report.trace = sim.trace;
    sim.report.committed_epochs = sim.oracle.epoch();
    Ok(sim.report)
}

fn gate(seeds: std::ops::Range<u64>, make: impl Fn(u64) -> Cfg) -> Report {
    let mut total = Report::default();
    for seed in seeds {
        match run(make(seed)) {
            Ok(r) => total.add(&r),
            Err(v) => panic!("{v}"),
        }
    }
    total
}

#[test]
fn thirty_seeds_of_two_thousand_steps_keep_every_replica_equal_and_lose_no_acknowledged_write() {
    let t = Instant::now();
    let total = gate(0..30, clean);
    eprintln!("gate 30x2000 on 3 replicas: {:?} {total:?}", t.elapsed());
    assert!(total.acked > 500, "{total:?}");
    assert!(total.committed_epochs > 500, "{total:?}");
    assert!(total.restarts > 100, "{total:?}");
    assert!(total.partitions > 50, "{total:?}");
    assert!(total.redelivered > 0, "{total:?}");
    assert!(total.crashes_before_finish > 20, "{total:?}");
    assert!(total.rejected > 20, "{total:?}");
    assert!(total.reads > 50, "{total:?}");
    assert!(total.snapshots_installed > 0, "{total:?}");
}

#[test]
fn the_same_seed_replays_to_the_same_trace_and_another_seed_does_not() {
    let a = run(clean(11)).expect("clean run");
    let b = run(clean(11)).expect("clean run");
    assert_eq!(a, b, "seed 11 did not replay identically");
    let c = run(clean(12)).expect("clean run");
    assert_ne!(a.trace, c.trace);
}

#[cfg(feature = "mutations")]
mod mutations {
    use super::*;

    fn first_failure(mutation: Mut) -> Violation {
        for seed in 0..30 {
            let cfg = Cfg { mutation, ..clean(seed) };
            if let Err(v) = run(cfg) {
                eprintln!("mutation {mutation:?} caught: {v}");
                return v;
            }
        }
        panic!("mutation {mutation:?} survived 30 seeds");
    }

    #[test]
    fn applying_on_append_instead_of_on_commit_is_caught() {
        first_failure(Mut::ApplyOnAppend);
    }

    #[test]
    fn treating_an_already_applied_redelivery_as_fatal_is_caught() {
        first_failure(Mut::FatalRedelivery);
    }

    #[test]
    fn acknowledging_before_commit_is_caught() {
        first_failure(Mut::AckBeforeCommit);
    }

    #[test]
    fn not_persisting_the_applied_index_is_caught() {
        first_failure(Mut::SkipAppliedPersist);
    }

    #[test]
    fn serving_a_read_without_the_read_index_round_is_caught() {
        first_failure(Mut::LocalRead);
    }

    #[test]
    fn ignoring_an_installed_snapshot_is_caught() {
        first_failure(Mut::IgnoreSnapshot);
    }

    #[test]
    fn without_the_durability_check_a_lost_applied_index_costs_replay_but_not_correctness() {
        let mut clean_total = Report::default();
        let mut lossy_total = Report::default();
        for seed in 0..10 {
            clean_total.add(&run(clean(seed)).expect("clean"));
            let cfg = Cfg { mutation: Mut::SkipAppliedPersist, applied_durable_check: false, ..clean(seed) };
            lossy_total.add(&run(cfg).expect("idempotent apply absorbs the replay"));
        }
        eprintln!("redelivered clean {} lossy {}", clean_total.redelivered, lossy_total.redelivered);
        assert!(lossy_total.redelivered > 5 * clean_total.redelivered.max(1));
    }
}
