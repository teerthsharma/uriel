use std::path::Path;

use uriel_core::{Graph, MemoryBudget, NodeId, Props, Value};
use uriel_rag::{Embedder, HashEmbedder, HitVia, IndexOptions, Rag, RagConfig, SearchOptions};

const CHUNKS_PER_FILE: usize = 40;
const WORDS: [&str; 24] = [
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

fn sentence(seed: u64) -> String {
    (0..14)
        .map(|i| WORDS[(mix(seed * 31 + i) % WORDS.len() as u64) as usize])
        .collect::<Vec<_>>()
        .join(" ")
}

fn build(path: &Path, chunks: usize) {
    let graph = Graph::open(path, MemoryBudget::mib(256)).expect("graph");
    let embedder = HashEmbedder::new(256);
    let mut tx = graph.begin_write().expect("write");
    let mut made = 0usize;
    for f in 0..chunks.div_ceil(CHUNKS_PER_FILE) {
        let rel = format!("notes/part_{f}.txt");
        let mut fp = Props::new();
        fp.insert("path".into(), Value::from(rel.as_str()));
        fp.insert("hash".into(), Value::from(format!("{f:064x}").as_str()));
        fp.insert("kind".into(), Value::from("dir"));
        let file = tx.create_node(&["File"], fp).expect("file");
        let mut prev: Option<NodeId> = None;
        for c in 0..CHUNKS_PER_FILE {
            if made == chunks {
                break;
            }
            let text = sentence(made as u64);
            let mut p = Props::new();
            p.insert("file".into(), Value::from(rel.as_str()));
            p.insert("start".into(), Value::Int(c as i64 + 1));
            p.insert("end".into(), Value::Int(c as i64 + 1));
            p.insert("mem".into(), Value::from(text.as_str()));
            p.insert("g3".into(), Value::from(text.as_str()));
            p.insert("l1".into(), Value::from(text.as_str()));
            p.insert("text".into(), Value::from(text.as_str()));
            p.insert("hash".into(), Value::from(format!("{made:064x}").as_str()));
            p.insert("utility".into(), Value::Float(0.0));
            p.insert("vec".into(), Value::from(embedder.embed(&text)));
            let id = tx.create_node(&["Chunk"], p).expect("chunk");
            tx.create_rel(id, file, "IN_FILE", Props::new()).expect("in_file");
            if let Some(prev) = prev {
                tx.create_rel(prev, id, "NEXT", Props::new()).expect("next");
            }
            prev = Some(id);
            made += 1;
        }
    }
    tx.commit().expect("commit");
}

fn open(path: &Path, mib: usize) -> Rag {
    Rag::open(
        Graph::open(path, MemoryBudget::mib(mib)).expect("graph"),
        RagConfig::default(),
    )
    .expect("rag")
}

#[test]
fn a_resident_index_holds_one_int8_vector_per_chunk_plus_one_scale() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    build(&db, 300);
    let rag = open(&db, 256);
    let stats = rag.stats().expect("stats");
    assert!(!stats.spilled);
    assert_eq!(stats.chunks, 300);
    assert_eq!(stats.vector_bytes, 300 * (256 + 4));
}

#[test]
fn an_index_over_its_budget_spills_and_answers_like_a_resident_one() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    build(&db, 9_000);
    let queries = [
        "ledger cursor bucket",
        "window digest policy budget",
        "metric sketch filter",
        "heap ring tree frame",
        "stage epoch cache state guard",
        "nothing matches this phrase at all",
    ];
    let mut expected = Vec::new();
    {
        let resident = open(&db, 512);
        assert!(!resident.stats().expect("stats").spilled);
        for hops in [0usize, 1] {
            for q in queries {
                let opts = SearchOptions {
                    k: 10,
                    expand_hops: hops,
                    ..SearchOptions::default()
                };
                expected.push((hops, q, resident.search(q, &opts).expect("search")));
            }
        }
    }
    let spilled = open(&db, MemoryBudget::MIN_BYTES / (1024 * 1024));
    let stats = spilled.stats().expect("stats");
    assert!(stats.spilled, "9000 chunks must not fit the 16 MiB budget's index share");
    assert_eq!(stats.chunks, 9_000);
    assert_eq!(stats.vector_bytes, 0);
    assert_eq!(stats.index_bytes, 0);
    let mut direct_seen = 0;
    for (hops, q, want) in expected {
        let opts = SearchOptions {
            k: 10,
            expand_hops: hops,
            ..SearchOptions::default()
        };
        let got = spilled.search(q, &opts).expect("search");
        assert_eq!(got, want, "hops {hops} query {q:?}");
        direct_seen += got.iter().filter(|h| h.via == HitVia::Direct).count();
    }
    assert!(direct_seen > 0);
}

#[test]
fn a_spilled_index_still_indexes_new_files_and_serves_them() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    build(&db, 9_000);
    let spilled = open(&db, MemoryBudget::MIN_BYTES / (1024 * 1024));
    assert!(spilled.stats().expect("stats").spilled);
    let docs = tempfile::tempdir().expect("docs");
    std::fs::write(
        docs.path().join("fresh.txt"),
        "zebra quokka narwhal pangolin axolotl vocabulary",
    )
    .expect("write");
    let report = spilled
        .index_dir(
            docs.path(),
            &IndexOptions {
                prune: false,
                ..IndexOptions::default()
            },
        )
        .expect("index");
    assert_eq!(report.chunks_added, 1);
    let hits = spilled
        .search("zebra quokka narwhal", &SearchOptions::default())
        .expect("search");
    assert_eq!(hits[0].file, "fresh.txt");
    assert_eq!(spilled.stats().expect("stats").chunks, 9_001);
}

#[test]
fn feedback_moves_the_utility_prior_of_a_spilled_chunk() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("g.uriel");
    build(&db, 9_000);
    let spilled = open(&db, MemoryBudget::MIN_BYTES / (1024 * 1024));
    assert!(spilled.stats().expect("stats").spilled);
    let opts = SearchOptions {
        k: 5,
        use_utility: true,
        ..SearchOptions::default()
    };
    let before = spilled.search("ledger cursor bucket", &opts).expect("search");
    let last = before.last().expect("hits");
    let (last_id, last_score) = (last.chunk_id, last.score);
    for _ in 0..6 {
        spilled.feedback(&[last_id], &[]).expect("feedback");
    }
    let after = spilled.search("ledger cursor bucket", &opts).expect("search");
    let moved = after.iter().find(|h| h.chunk_id == last_id).expect("still a hit");
    assert!(moved.score > last_score, "{} !> {last_score}", moved.score);
}
