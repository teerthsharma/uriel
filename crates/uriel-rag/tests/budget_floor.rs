use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use uriel_core::{Graph, MemoryBudget, Value};
use uriel_rag::{Hit, IndexOptions, Rag, RagConfig, SearchOptions};

const DEADLINE: Duration = Duration::from_secs(240);
const QUERIES: [&str; 6] = [
    "memory budget cache capacity",
    "transaction commit write batch",
    "tier promotion eviction policy",
    "hash embedding vector dimension",
    "cache admission frequency sketch",
    "nothing matches this phrase at all",
];

struct Outcome {
    digest: String,
    chunks: usize,
    spilled: bool,
    millis: u128,
    search_millis: u128,
    hits: Vec<Vec<Hit>>,
}

fn corpus() -> PathBuf {
    if let Some(dir) = std::env::var_os("URIEL_BUDGET_CORPUS") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("crates dir")
}

fn chunk_digest(graph: &Graph) -> (String, usize) {
    let read = graph.begin_read().expect("read");
    let mut rows: Vec<String> = Vec::new();
    for node in read.nodes_by_label("Chunk").expect("chunks") {
        let node = node.expect("node");
        let text = |k: &str| match node.props.get(k) {
            Some(Value::String(s)) => s.to_string(),
            _ => String::new(),
        };
        let line = |k: &str| match node.props.get(k) {
            Some(Value::Int(i)) => *i,
            _ => -1,
        };
        rows.push(format!(
            "{}:{}:{}:{}",
            text("file"),
            line("start"),
            line("end"),
            text("hash")
        ));
    }
    rows.sort();
    let mut h = Sha256::new();
    for r in &rows {
        h.update(r.as_bytes());
        h.update(b"\n");
    }
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    (hex, rows.len())
}

fn run(mib: usize) -> Outcome {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    let graph = Graph::open(&db, MemoryBudget::mib(mib)).expect("graph");
    let rag = Rag::open(graph.clone(), RagConfig::default()).expect("rag");
    let started = Instant::now();
    rag.index_dir(&corpus(), &IndexOptions::default())
        .expect("index");
    let millis = started.elapsed().as_millis();
    let (digest, chunks) = chunk_digest(&graph);
    let spilled = rag.stats().expect("stats").spilled;
    let searching = Instant::now();
    let hits = QUERIES
        .iter()
        .map(|q| rag.search(q, &SearchOptions::default()).expect("search"))
        .collect();
    let search_millis = searching.elapsed().as_millis();
    Outcome {
        digest,
        chunks,
        spilled,
        millis,
        search_millis,
        hits,
    }
}

fn bounded(mib: usize) -> Outcome {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(run(mib));
    });
    match rx.recv_timeout(DEADLINE) {
        Ok(o) => o,
        Err(_) => panic!("indexing at {mib} MiB did not finish within {DEADLINE:?}"),
    }
}

#[test]
fn indexing_the_workspace_at_the_minimum_budget_finishes_and_matches_the_roomy_budget() {
    let roomy = bounded(64);
    let tight = bounded(MemoryBudget::MIN_BYTES / (1024 * 1024));
    println!("DIGEST {} chunks={}", roomy.digest, roomy.chunks);
    println!(
        "BUDGET64 chunks={} spilled={} index_ms={} search_ms={}\nBUDGET16 chunks={} spilled={} index_ms={} search_ms={}",
        roomy.chunks,
        roomy.spilled,
        roomy.millis,
        roomy.search_millis,
        tight.chunks,
        tight.spilled,
        tight.millis,
        tight.search_millis
    );
    assert!(roomy.chunks > 1000);
    assert_eq!(tight.chunks, roomy.chunks);
    assert_eq!(tight.digest, roomy.digest);
    assert_eq!(tight.hits, roomy.hits);
}
