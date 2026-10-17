use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use uriel_core::{
    Command, ConstraintKind, Error, Graph, MemoryBudget, NodeId, Props, RelId, Value, WriteBatch,
    WriteTx,
};
use uriel_query::{AccessMode, Engine};

const SEEDS: u64 = 200;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    fn between(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi - lo + 1)
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const LABELS: [&str; 4] = ["U", "E", "P", "Q"];
const KEYS: [&str; 4] = ["id", "name", "w", "tags"];
const TYPES: [&str; 3] = ["KNOWS", "OWNS", "SEES"];

#[derive(Clone, Default)]
struct Model {
    nodes: Vec<u64>,
    rels: Vec<u64>,
}

#[derive(Default)]
struct Stats {
    violations: u64,
    rejected_ops: u64,
    rolled_back: u64,
    kinds: BTreeSet<&'static str>,
    batches: u64,
}

fn kind_name(c: &Command) -> &'static str {
    match c {
        Command::CreateNode { .. } => "CreateNode",
        Command::SetNodeProps { .. } => "SetNodeProps",
        Command::AddLabel { .. } => "AddLabel",
        Command::RemoveLabel { .. } => "RemoveLabel",
        Command::DeleteNode { .. } => "DeleteNode",
        Command::CreateRel { .. } => "CreateRel",
        Command::SetRelProps { .. } => "SetRelProps",
        Command::DeleteRel { .. } => "DeleteRel",
        Command::CreateIndex { .. } => "CreateIndex",
        Command::DropIndex { .. } => "DropIndex",
        Command::CreateConstraint { .. } => "CreateConstraint",
        Command::DropConstraint { .. } => "DropConstraint",
    }
}

fn value(rng: &mut Rng) -> Value {
    match rng.below(9) {
        0 => Value::Int(rng.below(6) as i64),
        1 => Value::Float(*rng.pick(&[0.5, -0.0, 2.0, f64::INFINITY, f64::NAN])),
        2 => Value::String(format!("s{}", rng.below(5))),
        3 => Value::Bool(rng.below(2) == 0),
        4 => Value::List(vec![Value::Int(rng.below(3) as i64), Value::Null]),
        5 => Value::Bytes(vec![rng.below(256) as u8; rng.below(4) as usize]),
        6 => Value::Vector(vec![rng.below(4) as f32, 0.25]),
        7 => Value::Null,
        _ => Value::Int(rng.below(6) as i64),
    }
}

fn props(rng: &mut Rng) -> Props {
    let mut out = Props::new();
    for _ in 0..rng.below(4) {
        out.insert((*rng.pick(&KEYS)).to_string(), value(rng));
    }
    out
}

fn labels(rng: &mut Rng) -> Vec<&'static str> {
    (0..rng.below(3)).map(|_| *rng.pick(&LABELS)).collect()
}

fn any_node(rng: &mut Rng, model: &Model) -> NodeId {
    if model.nodes.is_empty() || rng.below(12) == 0 {
        NodeId(rng.below(40))
    } else {
        NodeId(*rng.pick(&model.nodes))
    }
}

fn any_rel(rng: &mut Rng, model: &Model) -> RelId {
    if model.rels.is_empty() || rng.below(12) == 0 {
        RelId(rng.below(40))
    } else {
        RelId(*rng.pick(&model.rels))
    }
}

fn note<T>(r: Result<T, Error>, stats: &mut Stats) -> Option<T> {
    match r {
        Ok(v) => Some(v),
        Err(Error::ConstraintViolation { .. }) => {
            stats.violations += 1;
            None
        }
        Err(_) => {
            stats.rejected_ops += 1;
            None
        }
    }
}

fn random_op(tx: &mut WriteTx, model: &mut Model, rng: &mut Rng, stats: &mut Stats) {
    match rng.below(20) {
        0..=4 => {
            let (l, p) = (labels(rng), props(rng));
            if let Some(id) = note(tx.create_node(&l, p), stats) {
                model.nodes.push(id.0);
            }
        }
        5 | 6 => {
            let (n, p) = (any_node(rng, model), props(rng));
            note(tx.set_node_props(n, p), stats);
        }
        7 => {
            let n = any_node(rng, model);
            note(tx.add_label(n, rng.pick(&LABELS)), stats);
        }
        8 => {
            let n = any_node(rng, model);
            note(tx.remove_label(n, rng.pick(&LABELS)), stats);
        }
        9 => {
            let n = any_node(rng, model);
            if note(tx.delete_node(n), stats).is_some() {
                model.nodes.retain(|x| *x != n.0);
            }
        }
        10 => {
            let n = any_node(rng, model);
            if note(tx.detach_delete_node(n), stats).is_some() {
                model.nodes.retain(|x| *x != n.0);
                model
                    .rels
                    .retain(|r| tx.get_rel(RelId(*r)).unwrap().is_some());
            }
        }
        11..=14 => {
            let (a, b, p) = (any_node(rng, model), any_node(rng, model), props(rng));
            if let Some(id) = note(tx.create_rel(a, b, rng.pick(&TYPES), p), stats) {
                model.rels.push(id.0);
            }
        }
        15 => {
            let (r, p) = (any_rel(rng, model), props(rng));
            note(tx.set_rel_props(r, p), stats);
        }
        16 => {
            let r = any_rel(rng, model);
            if note(tx.delete_rel(r), stats).is_some() {
                model.rels.retain(|x| *x != r.0);
            }
        }
        17 => {
            note(tx.create_index(rng.pick(&LABELS), rng.pick(&KEYS)), stats);
        }
        18 => {
            if rng.below(2) == 0 {
                note(tx.drop_index(rng.pick(&LABELS), rng.pick(&KEYS)), stats);
            } else {
                note(
                    tx.drop_constraint(
                        *rng.pick(&[ConstraintKind::Unique, ConstraintKind::Exists]),
                        rng.pick(&LABELS),
                        rng.pick(&KEYS),
                    ),
                    stats,
                );
            }
        }
        _ => {
            if rng.below(2) == 0 {
                note(
                    tx.create_unique_constraint(rng.pick(&LABELS), rng.pick(&KEYS)),
                    stats,
                );
            } else {
                note(
                    tx.create_exists_constraint(rng.pick(&LABELS), rng.pick(&KEYS)),
                    stats,
                );
            }
        }
    }
}

fn leader_run(graph: &Graph, seed: u64, stats: &mut Stats) -> Vec<WriteBatch> {
    let mut rng = Rng::new(seed);
    let mut model = Model::default();
    let mut batches = Vec::new();
    for _ in 0..30 {
        let snapshot = model.clone();
        let mut tx = graph.begin_write_recording().unwrap();
        for _ in 0..rng.between(1, 6) {
            random_op(&mut tx, &mut model, &mut rng, stats);
        }
        if rng.below(5) == 0 {
            drop(tx);
            model = snapshot;
            stats.rolled_back += 1;
            continue;
        }
        let (epoch, batch) = tx.commit_recorded().unwrap();
        if let Some(batch) = batch {
            assert_eq!(batch.epoch, epoch);
            for c in &batch.commands {
                stats.kinds.insert(kind_name(c));
            }
            stats.batches += 1;
            batches.push(batch);
        }
    }
    batches
}

fn digest(graph: &Graph) -> String {
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

fn replica() -> Engine {
    Engine::open_in_memory(MemoryBudget::mib(80)).unwrap()
}

fn query(e: &Engine, q: &str) -> Vec<Vec<Value>> {
    e.execute(q, &BTreeMap::new(), AccessMode::Read)
        .unwrap_or_else(|err| panic!("{q}: {err}"))
        .rows
}

const QUERIES: [&str; 7] = [
    "MATCH (n) RETURN count(n)",
    "MATCH (n:U) RETURN id(n) ORDER BY id(n)",
    "MATCH (n:P) RETURN count(n)",
    "MATCH (n) OPTIONAL MATCH (n)-[r]->() RETURN id(n), count(r) ORDER BY id(n)",
    "MATCH (a)-[r:KNOWS]->(b) RETURN id(a), id(b), id(r) ORDER BY id(r)",
    "MATCH (n) RETURN id(n), labels(n) ORDER BY id(n)",
    "MATCH ()-[r]->() RETURN type(r), count(*) ORDER BY type(r)",
];

fn replay(batches: &[WriteBatch]) -> Engine {
    let b = replica();
    for batch in batches {
        let wire = WriteBatch::decode(&batch.encode()).unwrap();
        b.graph().apply_batch(&wire).unwrap();
    }
    b
}

#[test]
fn replicas_applying_recorded_batches_reach_identical_state_over_200_seeds() {
    let mut stats = Stats::default();
    for seed in 0..SEEDS {
        let a = replica();
        let batches = leader_run(a.graph(), seed, &mut stats);
        let b = replay(&batches);
        assert_eq!(digest(a.graph()), digest(b.graph()), "seed {seed}");
        assert_eq!(a.graph().epoch(), b.graph().epoch(), "seed {seed}");
        assert_eq!(a.graph().epoch(), batches.len() as u64, "seed {seed}");
        for q in QUERIES {
            assert_eq!(query(&a, q), query(&b, q), "seed {seed}: {q}");
        }
        assert!(
            b.graph()
                .begin_read()
                .unwrap()
                .check_integrity()
                .unwrap()
                .is_empty(),
            "seed {seed}: replica indexes inconsistent"
        );
    }
    let all: BTreeSet<&str> = [
        "CreateNode",
        "SetNodeProps",
        "AddLabel",
        "RemoveLabel",
        "DeleteNode",
        "CreateRel",
        "SetRelProps",
        "DeleteRel",
        "CreateIndex",
        "DropIndex",
        "CreateConstraint",
        "DropConstraint",
    ]
    .into_iter()
    .collect();
    assert_eq!(stats.kinds, all, "every command kind must be exercised");
    assert!(stats.violations >= 100, "violations {}", stats.violations);
    assert!(stats.rolled_back >= 500, "rollbacks {}", stats.rolled_back);
    assert!(stats.batches >= 3000, "batches {}", stats.batches);
}

#[test]
fn a_constraint_violating_or_rolled_back_transaction_records_no_batch() {
    let a = replica();
    a.graph().create_unique_constraint("U", "id").unwrap();
    let mut tx = a.graph().begin_write_recording().unwrap();
    tx.create_node(&["U"], BTreeMap::from([("id".to_string(), Value::Int(1))]))
        .unwrap();
    tx.commit_recorded().unwrap();
    let mut tx = a.graph().begin_write_recording().unwrap();
    let dup = tx.create_node(&["U"], BTreeMap::from([("id".to_string(), Value::Int(1))]));
    assert!(matches!(dup, Err(Error::ConstraintViolation { .. })));
    assert_eq!(tx.commit_recorded().unwrap().1, None);
    let mut tx = a.graph().begin_write_recording().unwrap();
    tx.create_node(&["U"], Props::new()).unwrap();
    drop(tx);
    assert_eq!(a.graph().epoch(), 2);
}

#[test]
fn applying_a_batch_twice_or_out_of_order_is_rejected_on_epoch_and_changes_nothing() {
    let mut stats = Stats::default();
    let a = replica();
    let batches = leader_run(a.graph(), 7, &mut stats);
    assert!(batches.len() >= 3);
    let b = replay(&batches);
    let before = digest(b.graph());
    for batch in [&batches[0], batches.last().unwrap()] {
        assert!(matches!(
            b.graph().apply_batch(batch),
            Err(Error::StaleBatch { .. })
        ));
    }
    assert_eq!(digest(b.graph()), before);
    let c = replica();
    assert!(matches!(
        c.graph().apply_batch(&batches[1]),
        Err(Error::StaleBatch { .. })
    ));
    assert_eq!(c.graph().epoch(), 0);
}
