use std::path::Path;

use uriel_core::{Graph, MemoryBudget, NodeId, Props, Value};
use uriel_rag::{Embedder, HashEmbedder, Hit, Rag, RagConfig, SearchOptions};

const DISTRACTORS: usize = 10;
const FILLERS: usize = 9_000;
const FILLER_WORDS: [&str; 24] = [
    "ledger", "cursor", "bucket", "window", "digest", "policy", "budget", "metric", "sketch",
    "filter", "column", "record", "stage", "epoch", "cache", "state", "guard", "table", "frame",
    "block", "batch", "heap", "ring", "tree",
];

fn mix(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn filler(seed: u64) -> String {
    (0..14)
        .map(|i| FILLER_WORDS[(mix(seed * 31 + i) % FILLER_WORDS.len() as u64) as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

fn plant(path: &Path, fillers: usize) -> u64 {
    let graph = Graph::open(path, MemoryBudget::mib(256)).expect("graph");
    let embedder = HashEmbedder::new(256);
    let mut tx = graph.begin_write().expect("write");
    let mut fp = Props::new();
    fp.insert("path".into(), Value::from("planted.txt"));
    fp.insert("hash".into(), Value::from(format!("{:064x}", 1).as_str()));
    fp.insert("kind".into(), Value::from("dir"));
    let file = tx.create_node(&["File"], fp).expect("file");
    let mut rows: Vec<(String, String)> = (0..DISTRACTORS)
        .map(|_| ("quasar nebula".to_string(), "quasar nebula".to_string()))
        .collect();
    rows.push(("quasar".into(), "quasar nebula zorblax".into()));
    rows.extend((0..fillers as u64).map(|i| (filler(i), filler(i))));
    let mut target = 0u64;
    let mut prev: Option<NodeId> = None;
    for (n, (mem, text)) in rows.iter().enumerate() {
        let mut p = Props::new();
        p.insert("file".into(), Value::from("planted.txt"));
        p.insert("start".into(), Value::Int(n as i64 + 1));
        p.insert("end".into(), Value::Int(n as i64 + 1));
        p.insert("mem".into(), Value::from(mem.as_str()));
        p.insert("g3".into(), Value::from(mem.as_str()));
        p.insert("l1".into(), Value::from(mem.as_str()));
        p.insert("text".into(), Value::from(text.as_str()));
        p.insert("hash".into(), Value::from(format!("{n:064x}").as_str()));
        p.insert("utility".into(), Value::Float(0.0));
        p.insert("vec".into(), Value::from(embedder.embed(mem)));
        let id = tx.create_node(&["Chunk"], p).expect("chunk");
        tx.create_rel(id, file, "IN_FILE", Props::new()).expect("in_file");
        if let Some(prev) = prev {
            tx.create_rel(prev, id, "NEXT", Props::new()).expect("next");
        }
        prev = Some(id);
        if n == DISTRACTORS {
            target = id.0;
        }
    }
    tx.commit().expect("commit");
    target
}

fn open(path: &Path, mib: usize) -> Rag {
    Rag::open(
        Graph::open(path, MemoryBudget::mib(mib)).expect("graph"),
        RagConfig::default(),
    )
    .expect("rag")
}

fn opts(k: usize) -> SearchOptions {
    SearchOptions {
        k,
        expand_hops: 0,
        use_utility: false,
        ..SearchOptions::default()
    }
}

const QUERY: &str = "zorblax quasar nebula";

#[test]
fn a_term_the_compressed_memory_dropped_is_recovered_from_the_stored_full_text() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    let target = plant(&db, 0);
    let rag = open(&db, 256);
    let hits = rag.search(QUERY, &opts(3)).expect("search");
    assert_eq!(hits[0].chunk_id, target, "hits: {hits:?}");
}

#[test]
fn rerank_is_byte_identical_across_repeated_searches() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    plant(&db, 0);
    let rag = open(&db, 256);
    let first: Vec<Hit> = rag.search(QUERY, &opts(5)).expect("search");
    for _ in 0..50 {
        assert_eq!(rag.search(QUERY, &opts(5)).expect("search"), first);
    }
}

#[test]
fn resident_and_spilled_indexes_return_identical_reranked_hits() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    let target = plant(&db, FILLERS);
    let queries = [QUERY, "quasar nebula", "ledger cursor bucket", "zorblax"];
    let mut expected = Vec::new();
    {
        let resident = open(&db, 512);
        assert!(!resident.stats().expect("stats").spilled);
        for q in queries {
            expected.push(resident.search(q, &opts(5)).expect("search"));
        }
        assert_eq!(expected[0][0].chunk_id, target);
    }
    let spilled = open(&db, MemoryBudget::MIN_BYTES / (1024 * 1024));
    assert!(spilled.stats().expect("stats").spilled);
    for (q, want) in queries.iter().zip(expected) {
        assert_eq!(spilled.search(q, &opts(5)).expect("search"), want, "query {q:?}");
    }
}
