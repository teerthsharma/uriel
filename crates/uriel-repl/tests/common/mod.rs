#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use redb::StorageBackend;
use sha2::{Digest, Sha256};
use uriel_core::{Graph, MemoryBudget, Options, Props, Value};
use uriel_kv::durable::FileStorage;
use uriel_kv::raft::{
    Entry, HardState, Index, Message, NodeId, Raft, Role, Snapshot, SnapshotMeta, Storage,
    StorageError, Term,
};
use uriel_kv::rng::Rng;
use uriel_kv::sim::{ids, slot, SimDisk};
use uriel_repl::Replica;

#[derive(Debug, Clone, Default)]
pub struct MemBackend(Arc<Mutex<Vec<u8>>>);

fn lock(m: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StorageBackend for MemBackend {
    fn len(&self) -> io::Result<u64> {
        Ok(lock(&self.0).len() as u64)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
        let buf = lock(&self.0);
        let end = offset as usize + out.len();
        if end > buf.len() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        out.copy_from_slice(&buf[offset as usize..end]);
        Ok(())
    }

    fn set_len(&self, len: u64) -> io::Result<()> {
        lock(&self.0).resize(len as usize, 0);
        Ok(())
    }

    fn sync_data(&self) -> io::Result<()> {
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let mut buf = lock(&self.0);
        let end = offset as usize + data.len();
        if buf.len() < end {
            buf.resize(end, 0);
        }
        buf[offset as usize..end].copy_from_slice(data);
        Ok(())
    }
}

pub fn open_graph(backend: &MemBackend) -> Graph {
    let me = backend.clone();
    Graph::open_with_backend(move || me.clone(), &Options::new(MemoryBudget::mib(16)))
        .expect("graph opens on a memory backend")
}

pub struct NodeStorage {
    pub inner: FileStorage<SimDisk>,
    pub drop_applied: bool,
}

impl Storage for NodeStorage {
    fn hard_state(&self) -> HardState {
        self.inner.hard_state()
    }
    fn set_hard_state(&mut self, hs: HardState) -> Result<(), StorageError> {
        self.inner.set_hard_state(hs)
    }
    fn applied(&self) -> Index {
        self.inner.applied()
    }
    fn set_applied(&mut self, applied: Index) -> Result<(), StorageError> {
        if self.drop_applied {
            return Ok(());
        }
        self.inner.set_applied(applied)
    }
    fn first_index(&self) -> Index {
        self.inner.first_index()
    }
    fn last_index(&self) -> Index {
        self.inner.last_index()
    }
    fn term_at(&self, index: Index) -> Option<Term> {
        self.inner.term_at(index)
    }
    fn entries(&self, from: Index, to: Index) -> Result<Vec<Entry>, StorageError> {
        self.inner.entries(from, to)
    }
    fn append(&mut self, entries: &[Entry]) -> Result<(), StorageError> {
        self.inner.append(entries)
    }
    fn truncate_from(&mut self, index: Index) -> Result<(), StorageError> {
        self.inner.truncate_from(index)
    }
    fn snapshot_meta(&self) -> SnapshotMeta {
        self.inner.snapshot_meta()
    }
    fn snapshot_data(&self) -> Result<Vec<u8>, StorageError> {
        self.inner.snapshot_data()
    }
    fn install_snapshot(&mut self, snap: Snapshot) -> Result<(), StorageError> {
        self.inner.install_snapshot(snap)
    }
    fn sync(&mut self) -> Result<(), StorageError> {
        self.inner.sync()
    }
}

pub type Rep = Replica<NodeStorage>;

pub struct Node {
    pub disk: SimDisk,
    pub backend: Rc<RefCell<MemBackend>>,
    pub rep: Rep,
    pub up: bool,
}

pub fn build_rep(
    id: u64,
    members: u64,
    disk: &SimDisk,
    backend: &Rc<RefCell<MemBackend>>,
    seed: u64,
    drop_applied: bool,
) -> Result<Rep, StorageError> {
    let inner = FileStorage::open(disk.clone(), 8)?;
    let storage = NodeStorage { inner, drop_applied };
    let raft = Raft::new(NodeId(id), ids(members), storage, seed)?;
    let graph = open_graph(&backend.borrow());
    let slot = Rc::clone(backend);
    let fresh = Box::new(move || {
        let b = MemBackend::default();
        *slot.borrow_mut() = b.clone();
        Ok(open_graph(&b))
    });
    Ok(Replica::new(raft, graph, fresh))
}

pub struct Mini {
    pub nodes: Vec<Node>,
    pub isolated: BTreeSet<usize>,
    pub queue: VecDeque<Message>,
    pub seed: u64,
}

impl Mini {
    pub fn new(n: u64, seed: u64) -> Mini {
        let nodes = (1..=n)
            .map(|id| {
                let disk = SimDisk::default();
                let backend = Rc::new(RefCell::new(MemBackend::default()));
                let rep = build_rep(id, n, &disk, &backend, seed, false).expect("fresh node opens");
                Node { disk, backend, rep, up: true }
            })
            .collect();
        Mini { nodes, isolated: BTreeSet::new(), queue: VecDeque::new(), seed }
    }

    pub fn route(&mut self, msgs: impl IntoIterator<Item = Message>) {
        self.queue.extend(msgs);
    }

    pub fn deliver_queue(&mut self) {
        while let Some(m) = self.queue.pop_front() {
            let (from, to) = (slot(m.from), slot(m.to));
            if !self.nodes[to].up || self.isolated.contains(&from) || self.isolated.contains(&to) {
                continue;
            }
            self.nodes[to].rep.step(m);
        }
    }

    pub fn stage_and_finish(&mut self, i: usize) {
        if !self.nodes[i].up {
            return;
        }
        if let Some(staged) = self.nodes[i].rep.stage() {
            let msgs = staged.messages().to_vec();
            self.nodes[i].rep.finish(staged);
            self.route(msgs);
        }
    }

    pub fn pump(&mut self) {
        for _ in 0..40 {
            for i in 0..self.nodes.len() {
                self.stage_and_finish(i);
            }
            if self.queue.is_empty() {
                break;
            }
            self.deliver_queue();
        }
    }

    pub fn tick(&mut self, rounds: u32) {
        for _ in 0..rounds {
            for n in self.nodes.iter_mut().filter(|n| n.up) {
                n.rep.tick();
            }
            self.pump();
        }
    }

    pub fn leader(&self) -> Option<usize> {
        (0..self.nodes.len()).find(|i| {
            self.nodes[*i].up
                && !self.isolated.contains(i)
                && self.nodes[*i].rep.raft().role() == Role::Leader
        })
    }

    pub fn elect(&mut self) -> usize {
        for _ in 0..200 {
            self.tick(1);
            if let Some(l) = self.leader() {
                self.tick(3);
                return l;
            }
        }
        panic!("no leader elected");
    }

    pub fn restart(&mut self, i: usize) {
        let mut rng = Rng::new(self.seed ^ i as u64);
        self.nodes[i].disk.crash(&mut rng);
        let n = self.nodes.len() as u64;
        let rep = build_rep(i as u64 + 1, n, &self.nodes[i].disk, &self.nodes[i].backend, self.seed + 77, false)
            .expect("node reopens");
        self.nodes[i].rep = rep;
        self.nodes[i].up = true;
    }
}

pub fn digest(graph: &Graph) -> String {
    let read = graph.begin_read().unwrap();
    let mut h = Sha256::new();
    for n in read.all_nodes().unwrap() {
        h.update(format!("{:?}\n", n.unwrap()));
    }
    for r in read.all_rels().unwrap() {
        h.update(format!("{:?}\n", r.unwrap()));
    }
    h.update(format!("{:?}\n", graph.list_indexes().unwrap()));
    h.update(format!("{:?}\n", graph.list_constraints().unwrap()));
    h.update(format!("epoch={}\n", graph.epoch()));
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn props(pairs: &[(&str, Value)]) -> Props {
    pairs.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect()
}
