use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use redb::StorageBackend;

use crate::ids::{Node, NodeId, Rel, RelId};
use crate::{Graph, Options, Props, Result, Value};

pub(crate) const NO_CUT: u64 = u64::MAX;

#[derive(Debug)]
pub(crate) struct Shared {
    data: Mutex<Vec<u8>>,
    ops: AtomicU64,
    cut: AtomicU64,
    torn: AtomicBool,
    dead: AtomicBool,
    fail_writes: AtomicBool,
    fail_reads: AtomicBool,
}

#[derive(Debug, Clone)]
pub(crate) struct FaultBackend(pub(crate) Arc<Shared>);

fn lock(m: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FaultBackend {
    pub(crate) fn new() -> FaultBackend {
        FaultBackend(Arc::new(Shared {
            data: Mutex::new(Vec::new()),
            ops: AtomicU64::new(0),
            cut: AtomicU64::new(NO_CUT),
            torn: AtomicBool::new(false),
            dead: AtomicBool::new(false),
            fail_writes: AtomicBool::new(false),
            fail_reads: AtomicBool::new(false),
        }))
    }

    pub(crate) fn cut_after(&self, ops: u64, torn: bool) {
        self.0.torn.store(torn, Ordering::SeqCst);
        self.0.cut.store(ops, Ordering::SeqCst);
    }

    pub(crate) fn ops(&self) -> u64 {
        self.0.ops.load(Ordering::SeqCst)
    }

    pub(crate) fn revive(&self) {
        self.0.cut.store(NO_CUT, Ordering::SeqCst);
        self.0.dead.store(false, Ordering::SeqCst);
        self.0.fail_writes.store(false, Ordering::SeqCst);
        self.0.fail_reads.store(false, Ordering::SeqCst);
    }

    pub(crate) fn set_fail_writes(&self, on: bool) {
        self.0.fail_writes.store(on, Ordering::SeqCst);
    }

    pub(crate) fn set_fail_reads(&self, on: bool) {
        self.0.fail_reads.store(on, Ordering::SeqCst);
    }

    pub(crate) fn bytes(&self) -> Vec<u8> {
        lock(&self.0.data).clone()
    }

    pub(crate) fn open(&self, options: &Options) -> Result<Graph> {
        let me = self.clone();
        Graph::open_with_backend(move || me.clone(), options)
    }

    fn write_op(&self, offset: u64, data: &[u8], resize: Option<u64>) -> io::Result<()> {
        let s = &self.0;
        if s.dead.load(Ordering::SeqCst) {
            return Err(io::Error::other("power cut"));
        }
        if s.fail_writes.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        let n = s.ops.fetch_add(1, Ordering::SeqCst);
        let cut = s.cut.load(Ordering::SeqCst);
        let mut buf = lock(&s.data);
        if n >= cut {
            s.dead.store(true, Ordering::SeqCst);
            if n == cut && s.torn.load(Ordering::SeqCst) && resize.is_none() {
                let half = data.len() / 2;
                let end = offset as usize + half;
                if buf.len() < end {
                    buf.resize(end, 0);
                }
                buf[offset as usize..end].copy_from_slice(&data[..half]);
            }
            return Err(io::Error::other("power cut"));
        }
        match resize {
            Some(len) => buf.resize(len as usize, 0),
            None => {
                let end = offset as usize + data.len();
                if buf.len() < end {
                    buf.resize(end, 0);
                }
                buf[offset as usize..end].copy_from_slice(data);
            }
        }
        Ok(())
    }
}

impl StorageBackend for FaultBackend {
    fn len(&self) -> io::Result<u64> {
        Ok(lock(&self.0.data).len() as u64)
    }

    fn read(&self, offset: u64, out: &mut [u8]) -> io::Result<()> {
        if self.0.fail_reads.load(Ordering::SeqCst) || self.0.dead.load(Ordering::SeqCst) {
            return Err(io::Error::other("read fault"));
        }
        let buf = lock(&self.0.data);
        let end = offset as usize + out.len();
        if end > buf.len() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        out.copy_from_slice(&buf[offset as usize..end]);
        Ok(())
    }

    fn set_len(&self, len: u64) -> io::Result<()> {
        self.write_op(0, &[], Some(len))
    }

    fn sync_data(&self) -> io::Result<()> {
        if self.0.dead.load(Ordering::SeqCst) {
            return Err(io::Error::other("power cut"));
        }
        if self.0.fail_writes.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        self.write_op(offset, data, None)
    }
}

pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

pub(crate) type Export = (Vec<Node>, Vec<Rel>);

pub(crate) fn export(g: &Graph) -> Result<Export> {
    let r = g.begin_read()?;
    let nodes = r.all_nodes()?.collect::<Result<Vec<_>>>()?;
    let rels = r.all_rels()?.collect::<Result<Vec<_>>>()?;
    Ok((nodes, rels))
}

#[derive(Debug, Clone)]
pub(crate) enum Op {
    AddNode(u64),
    SetProp(usize, u64),
    AddRel(usize, usize),
    DelRel(usize),
    DelNode(usize),
}

pub(crate) fn script(seed: u64, ops: usize) -> Vec<Vec<Op>> {
    let mut rng = Rng(seed | 1);
    let mut out = Vec::new();
    let mut left = ops;
    while left > 0 {
        let take = (1 + rng.below(4)).min(left);
        let mut tx = Vec::new();
        for _ in 0..take {
            let pick = rng.below(10);
            tx.push(match pick {
                0..=3 => Op::AddNode(rng.next()),
                4..=5 => Op::SetProp(rng.below(1 << 16), rng.next()),
                6..=7 => Op::AddRel(rng.below(1 << 16), rng.below(1 << 16)),
                8 => Op::DelRel(rng.below(1 << 16)),
                _ => Op::DelNode(rng.below(1 << 16)),
            });
        }
        left -= take;
        out.push(tx);
    }
    out
}

fn body(v: u64) -> String {
    format!("record {v:016x} {}", "lorem ipsum ".repeat((v % 40) as usize))
}

#[derive(Default, Clone)]
pub(crate) struct Live {
    nodes: Vec<NodeId>,
    rels: Vec<RelId>,
}

fn apply(g: &Graph, txn: &[Op], live: &mut Live) -> Result<u64> {
    let mut tx = g.begin_write()?;
    let mut next = live.clone();
    for op in txn {
        match op {
            Op::AddNode(v) => {
                let mut p = Props::new();
                p.insert("body".into(), Value::from(body(*v)));
                p.insert("n".into(), Value::Int((*v % 1000) as i64));
                let id = tx.create_node(&["Doc"], p)?;
                next.nodes.push(id);
            }
            Op::SetProp(s, v) => {
                if !next.nodes.is_empty() {
                    let id = next.nodes[s % next.nodes.len()];
                    let mut p = Props::new();
                    p.insert("n".into(), Value::Int((*v % 1000) as i64));
                    tx.set_node_props(id, p)?;
                }
            }
            Op::AddRel(a, b) => {
                if !next.nodes.is_empty() {
                    let x = next.nodes[a % next.nodes.len()];
                    let y = next.nodes[b % next.nodes.len()];
                    let id = tx.create_rel(x, y, "K", Props::new())?;
                    next.rels.push(id);
                }
            }
            Op::DelRel(s) => {
                if !next.rels.is_empty() {
                    let at = s % next.rels.len();
                    let id = next.rels.remove(at);
                    tx.delete_rel(id)?;
                }
            }
            Op::DelNode(s) => {
                if !next.nodes.is_empty() {
                    let at = s % next.nodes.len();
                    let id = next.nodes.remove(at);
                    let edges = tx.out_edges(id, None)?.into_iter().chain(tx.in_edges(id, None)?);
                    let gone: Vec<RelId> = edges.map(|e| e.rel).collect();
                    next.rels.retain(|r| !gone.contains(r));
                    tx.detach_delete_node(id)?;
                }
            }
        }
    }
    let epoch = tx.commit()?;
    *live = next;
    Ok(epoch)
}

pub(crate) struct Run {
    pub(crate) acked: usize,
    pub(crate) failed: bool,
}

pub(crate) fn run(g: &Graph, script: &[Vec<Op>], mut after: impl FnMut(&Graph, u64)) -> Run {
    let mut live = Live::default();
    let mut acked = 0;
    for txn in script {
        match apply(g, txn, &mut live) {
            Ok(epoch) => {
                acked += 1;
                after(g, epoch);
            }
            Err(_) => {
                return Run {
                    acked,
                    failed: true,
                }
            }
        }
    }
    Run {
        acked,
        failed: false,
    }
}
