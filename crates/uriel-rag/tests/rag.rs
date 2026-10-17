use std::fs;
use std::path::Path;

use uriel_core::Graph;
use uriel_rag::{ContextPack, Hit, HitVia, IndexOptions, Rag, RagConfig, SearchOptions};

fn write(dir: &Path, name: &str, lines: &[&str]) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir");
    }
    fs::write(path, lines.join("\n")).expect("write fixture");
}

const CHECKSUM: &[&str] = &[
    "/// Compute a rolling adler checksum over a byte buffer.",
    "pub fn compute_checksum(data: &[u8]) -> u32 {",
    "    let mut a: u32 = 1;",
    "    let mut b: u32 = 0;",
    "    for byte in data {",
    "        a = (a + u32::from(*byte)) % 65521;",
    "        b = (b + a) % 65521;",
    "    }",
    "    (b << 16) | a",
    "}",
    "",
    "/// Parse a duration string such as five minutes or two hours into seconds.",
    "pub fn parse_duration_text(input: &str) -> Option<u64> {",
    "    let trimmed = input.trim();",
    "    let (digits, unit) = trimmed.split_at(trimmed.len().checked_sub(1)?);",
    "    let value: u64 = digits.parse().ok()?;",
    "    match unit {",
    "        \"m\" => Some(value * 60),",
    "        \"h\" => Some(value * 3600),",
    "        _ => None,",
    "    }",
    "}",
    "",
    "/// Render a table of rows as aligned plain text columns.",
    "pub fn render_table(rows: &[Vec<String>]) -> String {",
    "    let mut out = String::new();",
    "    for row in rows {",
    "        out.push_str(&row.join(\"  \"));",
    "        out.push('\\n');",
    "    }",
    "    out",
    "}",
];

const PACKET: &[&str] = &[
    "/// Verify a packet body against the stored integrity value.",
    "pub fn verify_packet(body: &[u8], stored: u32) -> bool {",
    "    let actual = compute_checksum(body);",
    "    if actual != stored {",
    "        return false;",
    "    }",
    "    true",
    "}",
];

fn memory_rag() -> Rag {
    Rag::open(
        Graph::open_in_memory(uriel_core::MemoryBudget::mib(80)).expect("graph"),
        RagConfig::default(),
    )
    .expect("rag")
}

fn fixture_rag() -> (tempfile::TempDir, Rag) {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "a.rs", CHECKSUM);
    write(dir.path(), "b.rs", PACKET);
    write(
        dir.path(),
        "notes.md",
        &[
            "# Gardening notes",
            "",
            "Tomatoes need full sunlight and regular watering during the summer months.",
            "Compost enriches the soil with nutrients and improves drainage.",
            "",
            "## Pruning",
            "",
            "Prune roses in late winter before new growth begins to appear on the stems.",
            "Remove dead wood and crossing branches to keep the plant healthy.",
        ],
    );
    let rag = memory_rag();
    rag.index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    (dir, rag)
}

fn key(hits: &[Hit]) -> Vec<(u64, String, u32, u32, u32, bool)> {
    hits.iter()
        .map(|h| {
            (
                h.chunk_id,
                h.file.clone(),
                h.start_line,
                h.end_line,
                h.score.to_bits(),
                h.via == HitVia::Direct,
            )
        })
        .collect()
}

fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

#[test]
fn search_finds_semantically_related_chunk() {
    let (_dir, rag) = fixture_rag();
    let hits = rag
        .search("rolling sum over a byte buffer", &SearchOptions::default())
        .expect("search");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].file, "a.rs");
    assert!(hits[0].memory.contains("checksum"), "{:?}", hits[0].memory);
    let hits = rag
        .search("when to prune roses", &SearchOptions::default())
        .expect("search");
    assert_eq!(hits[0].file, "notes.md");
}

#[test]
fn deterministic_and_identical_after_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    write(&src, "a.rs", CHECKSUM);
    write(&src, "b.rs", PACKET);
    let db = dir.path().join("rag.redb");
    let queries = [
        "adler checksum bytes",
        "duration minutes hours",
        "table columns",
        "verify packet",
    ];
    let opts = SearchOptions {
        k: 5,
        ..SearchOptions::default()
    };

    let before: Vec<_> = {
        let rag = Rag::open(Graph::open(&db, uriel_core::MemoryBudget::mib(80)).expect("open"), RagConfig::default()).expect("rag");
        rag.index_dir(&src, &IndexOptions::default())
            .expect("index");
        let first: Vec<_> = queries
            .iter()
            .map(|q| key(&rag.search(q, &opts).expect("s")))
            .collect();
        let again: Vec<_> = queries
            .iter()
            .map(|q| key(&rag.search(q, &opts).expect("s")))
            .collect();
        assert_eq!(first, again, "same process, same query, same order");
        first
    };
    let rag = Rag::open(Graph::open(&db, uriel_core::MemoryBudget::mib(80)).expect("reopen"), RagConfig::default()).expect("rag");
    let after: Vec<_> = queries
        .iter()
        .map(|q| key(&rag.search(q, &opts).expect("s")))
        .collect();
    assert_eq!(before, after, "a reopened Rag answers identically");
    assert!(before.iter().all(|h| !h.is_empty()));
}

#[test]
fn incremental_reindex_skips_unchanged_and_replaces_edited() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "a.rs", CHECKSUM);
    write(
        dir.path(),
        "zoo.txt",
        &[
            "zebra giraffe savanna enclosure feeding schedule",
            "zebra giraffe keepers",
        ],
    );
    let graph = Graph::open_in_memory(uriel_core::MemoryBudget::mib(80)).expect("graph");
    let rag = Rag::open(graph.clone(), RagConfig::default()).expect("rag");
    let first = rag
        .index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    assert_eq!(first.files_indexed, 2);
    assert!(first.chunks_added >= 2);

    let second = rag
        .index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    assert_eq!(second.files_unchanged, 2);
    assert_eq!(second.files_indexed, 0);
    assert_eq!(second.chunks_added, 0);

    write(
        dir.path(),
        "zoo.txt",
        &[
            "walrus penguin arctic aquarium tank",
            "walrus penguin divers",
        ],
    );
    let third = rag
        .index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    assert_eq!(third.files_indexed, 1);
    assert_eq!(third.files_unchanged, 1);
    assert!(third.chunks_removed >= 1);

    let count = |label: &str| {
        graph
            .begin_read()
            .expect("r")
            .count_nodes_with_label(label)
            .expect("c")
    };
    let stats = rag.stats().expect("stats");
    assert_eq!(count("Chunk") as usize, stats.chunks, "db and index agree");
    assert_eq!(count("File") as usize, 2);
    let stale = rag
        .search("zebra giraffe", &SearchOptions::default())
        .expect("s");
    for h in &stale {
        assert!(!h.memory.contains("zebra"), "stale memory in index: {h:?}");
        assert!(!rag.expand(h.chunk_id).expect("expand").contains("zebra"));
    }
    let fresh = rag
        .search("walrus penguin", &SearchOptions::default())
        .expect("s");
    assert_eq!(fresh[0].file, "zoo.txt");

    fs::remove_file(dir.path().join("zoo.txt")).expect("rm");
    let fourth = rag
        .index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    assert_eq!(fourth.files_removed, 1);
    assert_eq!(count("File"), 1);
    assert_eq!(count("Chunk") as usize, rag.stats().expect("stats").chunks);
    assert!(rag
        .search("walrus penguin", &SearchOptions::default())
        .expect("s")
        .iter()
        .all(|h| h.file != "zoo.txt"));
}

#[test]
fn stale_chunks_are_not_resurrected_on_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    write(&src, "zoo.txt", &["zebra giraffe savanna enclosure"]);
    let db = dir.path().join("rag.redb");
    {
        let rag = Rag::open(Graph::open(&db, uriel_core::MemoryBudget::mib(80)).expect("g"), RagConfig::default()).expect("rag");
        rag.index_dir(&src, &IndexOptions::default())
            .expect("index");
        write(&src, "zoo.txt", &["walrus penguin arctic aquarium"]);
        rag.index_dir(&src, &IndexOptions::default())
            .expect("index");
    }
    let rag = Rag::open(Graph::open(&db, uriel_core::MemoryBudget::mib(80)).expect("g"), RagConfig::default()).expect("rag");
    assert_eq!(rag.stats().expect("stats").chunks, 1);
    let hits = rag
        .search("zebra giraffe walrus", &SearchOptions::default())
        .expect("s");
    assert!(hits.iter().all(|h| !h.memory.contains("zebra")));
}

#[test]
fn pack_never_exceeds_budget_across_200_budgets() {
    let (_dir, rag) = fixture_rag();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut included_some = 0;
    for _ in 0..200 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let budget = ((state >> 33) % 400) as usize;
        let opts = SearchOptions {
            k: 10,
            budget_tokens: budget,
            expand_hops: 1,
            use_utility: true,
        };
        let pack: ContextPack = rag
            .pack("checksum duration table roses packet", &opts)
            .expect("pack");
        assert!(pack.tokens <= budget, "{} > {budget}", pack.tokens);
        assert!(
            tokens(&pack.text) <= budget,
            "rendered text over budget {budget}"
        );
        assert_eq!(pack.budget_tokens, budget);
        if !pack.hits.is_empty() {
            included_some += 1;
        }
    }
    assert!(
        included_some > 100,
        "packs should usually contain something, got {included_some}"
    );
    let big = SearchOptions {
        budget_tokens: 4000,
        ..SearchOptions::default()
    };
    assert!(!rag.pack("checksum", &big).expect("pack").hits.is_empty());
    let zero = SearchOptions {
        budget_tokens: 0,
        ..SearchOptions::default()
    };
    assert!(rag.pack("checksum", &zero).expect("pack").hits.is_empty());
}

#[test]
fn compression_ratio_is_near_target_on_real_corpus() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../uriel-core/src");
    let rag = memory_rag();
    rag.index_dir(&corpus, &IndexOptions::default())
        .expect("index");
    let stats = rag.stats().expect("stats");
    assert!(stats.chunks > 50, "corpus too small: {}", stats.chunks);
    let target = 16.0;
    assert!(
        stats.avg_compression_ratio >= 0.6 * target && stats.avg_compression_ratio <= 1.6 * target,
        "ratio {} outside [{}, {}]",
        stats.avg_compression_ratio,
        0.6 * target,
        1.6 * target
    );
}

#[test]
fn feedback_moves_ranking_in_expected_direction() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, func) in [("one.rs", "clean_alpha"), ("two.rs", "clean_beta")] {
        let sig = format!("pub fn {func}(text: &str) -> String {{");
        write(
            dir.path(),
            name,
            &[
                "/// Normalize whitespace in user supplied text before storing it.",
                &sig,
                "    let mut out = String::new();",
                "    for word in text.split_whitespace() {",
                "        if !out.is_empty() {",
                "            out.push(' ');",
                "        }",
                "        out.push_str(word);",
                "    }",
                "    out",
                "}",
            ],
        );
    }
    let rag = memory_rag();
    rag.index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    let q = "normalize whitespace in user supplied text";
    let plain = SearchOptions {
        k: 2,
        use_utility: false,
        ..SearchOptions::default()
    };
    let with = SearchOptions {
        use_utility: true,
        ..plain.clone()
    };
    let base = rag.search(q, &plain).expect("s");
    assert_eq!(base.len(), 2);
    let (first, second) = (base[0].chunk_id, base[1].chunk_id);
    assert_eq!(
        key(&rag.search(q, &with).expect("s")),
        key(&base),
        "no feedback yet, no change"
    );

    for _ in 0..3 {
        rag.feedback(&[second], &[first]).expect("feedback");
    }
    let after = rag.search(q, &with).expect("s");
    assert_eq!(after[0].chunk_id, second, "used chunk must rise");
    assert_eq!(
        rag.search(q, &plain).expect("s")[0].chunk_id,
        first,
        "utility off ignores the prior"
    );
}

#[test]
fn feedback_persists_across_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    write(&src, "a.rs", CHECKSUM);
    let db = dir.path().join("rag.redb");
    let id = {
        let rag = Rag::open(Graph::open(&db, uriel_core::MemoryBudget::mib(80)).expect("g"), RagConfig::default()).expect("rag");
        rag.index_dir(&src, &IndexOptions::default())
            .expect("index");
        let id = rag
            .search("checksum", &SearchOptions::default())
            .expect("s")[0]
            .chunk_id;
        for _ in 0..4 {
            rag.feedback(&[id], &[]).expect("fb");
        }
        let hits = rag
            .search("checksum", &SearchOptions::default())
            .expect("s");
        assert!(hits[0].score > 0.0);
        id
    };
    let graph = Graph::open(&db, uriel_core::MemoryBudget::mib(80)).expect("g");
    let node = graph
        .begin_read()
        .expect("r")
        .get_node(uriel_core::NodeId(id))
        .expect("get")
        .expect("exists");
    match node.props.get("utility") {
        Some(uriel_core::Value::Float(u)) => assert!(*u > 0.5 && *u <= 1.0, "utility {u}"),
        other => panic!("utility not stored as float: {other:?}"),
    }
}

#[test]
fn graph_expansion_adds_neighbours_direct_search_missed() {
    let (_dir, rag) = fixture_rag();
    let q = "rolling adler checksum over a byte buffer";
    let direct = SearchOptions {
        k: 1,
        expand_hops: 0,
        ..SearchOptions::default()
    };
    let hits0 = rag.search(q, &direct).expect("s");
    assert_eq!(hits0.len(), 1);
    assert_eq!(hits0[0].via, HitVia::Direct);
    assert!(hits0.iter().all(|h| h.file != "b.rs"));

    let hops1 = SearchOptions {
        expand_hops: 1,
        ..direct.clone()
    };
    let hits1 = rag.search(q, &hops1).expect("s");
    assert_eq!(hits1[0].chunk_id, hits0[0].chunk_id);
    assert!(hits1.len() > 1);
    assert!(hits1[1..].iter().all(|h| h.via == HitVia::GraphNeighbour));
    assert!(hits1
        .iter()
        .any(|h| h.file == "a.rs" && h.chunk_id != hits0[0].chunk_id));
    let sym = hits1
        .iter()
        .find(|h| h.file == "b.rs")
        .expect("symbol neighbour from b.rs");
    assert_eq!(sym.via, HitVia::GraphNeighbour);
    assert!(
        sym.score < hits1[0].score && sym.score > 0.0,
        "decayed score"
    );
    for h in &hits1[1..] {
        assert!(h.score < hits1[0].score);
    }
}

#[test]
fn skips_binary_oversize_and_ignored_directories() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "keep.rs", CHECKSUM);
    fs::write(dir.path().join("blob.bin"), b"abc\0def\0ghi").expect("bin");
    let big = "word ".repeat(300_000);
    assert!(big.len() > 1 << 20);
    fs::write(dir.path().join("huge.txt"), big).expect("big");
    write(dir.path(), "target/debug/gen.rs", CHECKSUM);
    write(
        dir.path(),
        "node_modules/pkg/index.js",
        &["function leftpad() { return 1; }"],
    );
    write(dir.path(), ".git/config.txt", &["secret config text"]);
    let rag = memory_rag();
    let report = rag
        .index_dir(dir.path(), &IndexOptions::default())
        .expect("index");
    assert_eq!(report.files_indexed, 1);
    assert_eq!(report.files_skipped, 2, "binary + oversize: {report:?}");
    assert_eq!(rag.stats().expect("stats").files, 1);
    let hits = rag
        .search("checksum leftpad secret", &SearchOptions::default())
        .expect("s");
    assert!(hits.iter().all(|h| h.file == "keep.rs"));
}

#[test]
fn index_text_is_searchable_and_incremental() {
    let rag = memory_rag();
    let first = rag
        .index_text(
            "decision:7",
            "We chose redb for storage because it is embedded and transactional.",
        )
        .expect("index");
    assert_eq!(first.files_indexed, 1);
    let again = rag
        .index_text(
            "decision:7",
            "We chose redb for storage because it is embedded and transactional.",
        )
        .expect("index");
    assert_eq!(again.files_unchanged, 1);
    assert_eq!(again.chunks_added, 0);
    let hits = rag
        .search(
            "which embedded transactional storage",
            &SearchOptions::default(),
        )
        .expect("s");
    assert_eq!(hits[0].file, "decision:7");
    let full = rag.expand(hits[0].chunk_id).expect("expand");
    assert!(full.contains("redb"));
    rag.index_text("decision:7", "Switched to a different engine entirely.")
        .expect("index");
    assert_eq!(rag.stats().expect("stats").chunks, 1);
    assert!(
        rag.expand(hits[0].chunk_id).is_err(),
        "old chunk id is gone"
    );
}

#[test]
fn invalid_options_are_errors() {
    let (_dir, rag) = fixture_rag();
    let bad = SearchOptions {
        expand_hops: 3,
        ..SearchOptions::default()
    };
    assert!(rag.search("x", &bad).is_err());
    let cfg = RagConfig {
        compression_ratio: 0,
        ..RagConfig::default()
    };
    assert!(Rag::open(Graph::open_in_memory(uriel_core::MemoryBudget::mib(80)).expect("g"), cfg).is_err());
}
