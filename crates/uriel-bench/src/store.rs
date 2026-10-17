use uriel_core::{Graph, NodeId, Options, Placement, Props, ReadTx, Value};
use tempfile::TempDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    T0,
    T1,
    T2,
}

pub fn tier_options(tier: Tier) -> Options {
    match tier {
        Tier::T0 => uriel_core::Options::new(uriel_core::MemoryBudget::mib(80)),
        Tier::T1 => Options {
            hot_entries: 1,
            admission_gate: false,
            ..uriel_core::Options::new(uriel_core::MemoryBudget::mib(80))
        },
        Tier::T2 => Options {
            hot_entries: 0,
            hot_bytes: 0,
            warm_bytes: 0,
            admission_gate: false,
            ..uriel_core::Options::new(uriel_core::MemoryBudget::mib(80))
        },
    }
}

pub fn zipf_options(placement: Placement) -> Options {
    Options {
        placement,
        hot_entries: 256,
        hot_bytes: 256 * 1024,
        warm_bytes: 512 * 1024,
        ..uriel_core::Options::new(uriel_core::MemoryBudget::mib(80))
    }
}

pub fn body(i: usize) -> String {
    format!("node {i}: the quick brown fox jumps over the lazy dog; ").repeat(12)
}

pub struct Store {
    pub graph: Graph,
    pub ids: Vec<NodeId>,
    _dir: TempDir,
}

impl Store {
    pub fn reader(&self) -> ReadTx {
        self.graph.begin_read().expect("begin read")
    }
}

pub fn open_store(options: &Options, nodes: usize) -> Store {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bench.redb");
    let graph = Graph::open_with(&path, options).expect("open graph");
    let mut tx = graph.begin_write().expect("begin write");
    let ids = (0..nodes)
        .map(|i| {
            let mut props = Props::new();
            props.insert("i".to_string(), Value::Int(i as i64));
            props.insert("body".to_string(), Value::from(body(i)));
            tx.create_node(&["Doc"], props).expect("create node")
        })
        .collect();
    tx.commit().expect("commit");
    Store {
        graph,
        ids,
        _dir: dir,
    }
}

pub fn warm(store: &Store, laps: usize) {
    let tx = store.reader();
    for _ in 0..laps {
        for id in &store.ids {
            tx.get_node(*id).expect("warm read");
        }
    }
}

pub fn read_sequence(tx: &ReadTx, ids: &[NodeId], order: &[usize]) -> usize {
    let mut props = 0;
    for &i in order {
        if let Some(node) = tx.get_node(ids[i]).expect("read node") {
            props += node.props.len();
        }
    }
    props
}

pub fn hit_rate(graph: &Graph) -> f64 {
    let stats = graph.tier_stats().expect("tier stats");
    let served = stats.hot.hits + stats.warm.hits;
    let total = served + stats.disk_reads;
    served as f64 / total.max(1) as f64
}
