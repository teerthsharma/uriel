use std::collections::BTreeMap;

use uriel_core::{Graph, GraphDump, WriteBatch};
use uriel_kv::raft::{
    Entry, EntryKind, Index, Message, NodeId, Raft, ReadError, Ready, Role, Snapshot, Storage,
    StorageError, Term,
};

use crate::builder::BatchBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ticket(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ack {
    Committed { epoch: u64 },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Fault {
    #[error("committed entry {index} does not decode: {reason}")]
    Decode { index: u64, reason: String },
    #[error("committed entry {index} (epoch {epoch}) failed to apply: {reason}")]
    Apply { index: u64, epoch: u64, reason: String },
    #[error("entry {index} was already applied and redelivery is fatal in this configuration")]
    AlreadyApplied { index: u64 },
    #[error("snapshot at {index} cannot be installed: {reason}")]
    Snapshot { index: u64, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("not the leader (hint: {0:?})")]
    NotLeader(Option<NodeId>),
    #[error("the previous write has not been applied yet")]
    Busy,
    #[error("the batch is empty")]
    Empty,
    #[error("the batch is invalid against the applied state: {0}")]
    Rejected(uriel_core::Error),
    #[error("the replica is stopped on a fault")]
    Faulted,
}

#[cfg(feature = "mutations")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutation {
    ApplyOnAppend,
    FatalRedelivery,
    AckBeforeCommit,
    LocalRead,
    IgnoreSnapshot,
}

struct Inflight {
    ticket: Ticket,
    index: Index,
    term: Term,
}

pub struct Staged {
    ready: Ready,
}

impl Staged {
    pub fn messages(&self) -> &[Message] {
        &self.ready.messages
    }

    pub fn entries(&self) -> usize {
        self.ready.committed_entries.len()
    }
}

type Fresh = Box<dyn FnMut() -> uriel_core::Result<Graph>>;

pub struct Replica<S: Storage> {
    raft: Raft<S>,
    graph: Graph,
    fresh: Fresh,
    inflight: Option<Inflight>,
    next_ticket: u64,
    acks: Vec<(Ticket, Ack)>,
    graph_index: Index,
    waiting: BTreeMap<u64, Index>,
    ready_reads: Vec<u64>,
    fault: Option<Fault>,
    redelivered: u64,
    snapshots_installed: u64,
    #[cfg(feature = "mutations")]
    mutation: Option<Mutation>,
}

impl<S: Storage> Replica<S> {
    pub fn new(raft: Raft<S>, graph: Graph, fresh: Fresh) -> Replica<S> {
        let graph_index = raft.applied_index();
        Replica {
            raft,
            graph,
            fresh,
            inflight: None,
            next_ticket: 1,
            acks: Vec::new(),
            graph_index,
            waiting: BTreeMap::new(),
            ready_reads: Vec::new(),
            fault: None,
            redelivered: 0,
            snapshots_installed: 0,
            #[cfg(feature = "mutations")]
            mutation: None,
        }
    }

    #[cfg(feature = "mutations")]
    pub fn set_mutation(&mut self, mutation: Option<Mutation>) {
        self.mutation = mutation;
    }

    pub fn raft(&self) -> &Raft<S> {
        &self.raft
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn fault(&self) -> Option<&Fault> {
        self.fault.as_ref()
    }

    pub fn redelivered(&self) -> u64 {
        self.redelivered
    }

    pub fn snapshots_installed(&self) -> u64 {
        self.snapshots_installed
    }

    pub fn tick(&mut self) {
        if self.fault.is_none() {
            self.raft.tick();
        }
    }

    pub fn step(&mut self, msg: Message) {
        if self.fault.is_none() {
            self.raft.step(msg);
        }
    }

    pub fn drain_acks(&mut self) -> Vec<(Ticket, Ack)> {
        std::mem::take(&mut self.acks)
    }

    pub fn drain_reads(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.ready_reads)
    }

    pub fn read_index(&mut self, ctx: u64) -> Result<(), ReadError> {
        if self.fault.is_some() {
            return Err(ReadError::NotLeader(None));
        }
        #[cfg(feature = "mutations")]
        if self.mutation == Some(Mutation::LocalRead) && self.raft.role() == Role::Leader {
            self.ready_reads.push(ctx);
            return Ok(());
        }
        self.raft.read_index(ctx)
    }

    pub fn write(&mut self, build: impl FnOnce(&mut BatchBuilder)) -> Result<Ticket, WriteError> {
        if self.fault.is_some() {
            return Err(WriteError::Faulted);
        }
        if self.raft.role() != Role::Leader {
            return Err(WriteError::NotLeader(self.raft.leader()));
        }
        let applied = self.raft.applied_index();
        if self.inflight.is_some()
            || applied != self.raft.storage().last_index()
            || self.graph_index != applied
        {
            return Err(WriteError::Busy);
        }
        let read = self.graph.begin_read().map_err(WriteError::Rejected)?;
        let (next_node, next_rel) = read.next_ids().map_err(WriteError::Rejected)?;
        let mut builder = BatchBuilder::new(read.epoch() + 1, next_node, next_rel);
        drop(read);
        build(&mut builder);
        let batch = builder.finish();
        if batch.commands.is_empty() {
            return Err(WriteError::Empty);
        }
        self.graph.validate_batch(&batch).map_err(WriteError::Rejected)?;
        let index = self
            .raft
            .propose(batch.encode())
            .map_err(|e| WriteError::NotLeader(e.hint))?;
        let ticket = Ticket(self.next_ticket);
        self.next_ticket += 1;
        self.inflight = Some(Inflight { ticket, index, term: self.raft.term() });
        #[cfg(feature = "mutations")]
        if self.mutation == Some(Mutation::AckBeforeCommit) {
            self.acks.push((ticket, Ack::Committed { epoch: batch.epoch }));
        }
        Ok(ticket)
    }

    pub fn stage(&mut self) -> Option<Staged> {
        if self.fault.is_some() {
            return None;
        }
        #[cfg(feature = "mutations")]
        if self.mutation == Some(Mutation::ApplyOnAppend) {
            if let Err(f) = self.apply_appended() {
                self.fault = Some(f);
                return None;
            }
        }
        let ready = self.raft.ready();
        if self.raft.fault().is_some() {
            return None;
        }
        if let Some(snap) = &ready.snapshot {
            if let Err(f) = self.install(snap) {
                self.fault = Some(f);
                return None;
            }
        }
        for entry in &ready.committed_entries {
            if let Err(f) = self.apply_entry(entry) {
                self.fault = Some(f);
                return None;
            }
        }
        for rs in &ready.read_states {
            self.waiting.insert(rs.ctx, rs.index);
        }
        let reached = self.graph_index;
        let done: Vec<u64> =
            self.waiting.iter().filter(|(_, i)| **i <= reached).map(|(c, _)| *c).collect();
        for ctx in done {
            self.waiting.remove(&ctx);
            self.ready_reads.push(ctx);
        }
        self.expire_inflight();
        Some(Staged { ready })
    }

    pub fn finish(&mut self, staged: Staged) {
        self.raft.advance(staged.ready);
    }

    pub fn compact(&mut self) -> Result<bool, StorageError> {
        let applied = self.raft.applied_index();
        if applied.0 == 0
            || self.graph_index != applied
            || applied <= self.raft.storage().snapshot_meta().index
        {
            return Ok(false);
        }
        let dump = self.graph.export_dump().map_err(|e| StorageError::Io(e.to_string()))?;
        self.raft.compact(applied, dump.encode())?;
        Ok(true)
    }

    fn expire_inflight(&mut self) {
        let lost = self
            .inflight
            .as_ref()
            .is_some_and(|f| self.raft.role() != Role::Leader || self.raft.term() != f.term);
        if lost {
            if let Some(f) = self.inflight.take() {
                self.acks.push((f.ticket, Ack::Unknown));
            }
        }
    }

    fn install(&mut self, snap: &Snapshot) -> Result<(), Fault> {
        #[cfg(feature = "mutations")]
        if self.mutation == Some(Mutation::IgnoreSnapshot) {
            return Ok(());
        }
        let index = snap.meta.index.0;
        let fail = |reason: String| Fault::Snapshot { index, reason };
        let dump = GraphDump::decode(&snap.data).map_err(|e| fail(e.to_string()))?;
        if dump.batch.epoch > self.graph.epoch() {
            let fresh = (self.fresh)().map_err(|e| fail(e.to_string()))?;
            fresh.restore_dump(&dump).map_err(|e| fail(e.to_string()))?;
            self.graph = fresh;
            self.snapshots_installed += 1;
        }
        self.graph_index = self.graph_index.max(snap.meta.index);
        Ok(())
    }

    fn apply_entry(&mut self, entry: &Entry) -> Result<(), Fault> {
        self.graph_index = self.graph_index.max(entry.index);
        if entry.kind != EntryKind::Normal || entry.data.is_empty() {
            return Ok(());
        }
        let index = entry.index.0;
        let batch = WriteBatch::decode(&entry.data)
            .map_err(|e| Fault::Decode { index, reason: e.to_string() })?;
        if batch.epoch <= self.graph.epoch() {
            #[cfg(feature = "mutations")]
            if self.mutation == Some(Mutation::FatalRedelivery) {
                return Err(Fault::AlreadyApplied { index });
            }
            self.redelivered += 1;
        } else {
            self.graph.apply_batch(&batch).map_err(|e| Fault::Apply {
                index,
                epoch: batch.epoch,
                reason: e.to_string(),
            })?;
        }
        self.resolve(entry, batch.epoch);
        Ok(())
    }

    fn resolve(&mut self, entry: &Entry, epoch: u64) {
        let matches = self.inflight.as_ref().is_some_and(|f| f.index == entry.index);
        if !matches {
            return;
        }
        if let Some(f) = self.inflight.take() {
            #[cfg(feature = "mutations")]
            if self.mutation == Some(Mutation::AckBeforeCommit) {
                return;
            }
            let ack = if f.term == entry.term { Ack::Committed { epoch } } else { Ack::Unknown };
            self.acks.push((f.ticket, ack));
        }
    }

    #[cfg(feature = "mutations")]
    fn apply_appended(&mut self) -> Result<(), Fault> {
        let first = self.raft.storage().first_index();
        let from = Index(self.graph_index.0 + 1).max(first);
        let to = Index(self.raft.storage().last_index().0 + 1);
        if from >= to {
            return Ok(());
        }
        let entries = self.raft.storage().entries(from, to).unwrap_or_default();
        let held = self.inflight.take();
        let applied = entries.iter().try_for_each(|entry| self.apply_entry(entry));
        self.inflight = held;
        applied
    }
}
