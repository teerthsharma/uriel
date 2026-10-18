use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use rayon::prelude::*;
use uriel_core::{Graph, MemoryBudget};
use uriel_rag::{IndexOptions, Rag, RagConfig, Soul, SoulOptions};

const WORDS: [&str; 12] = [
    "routes", "frames", "ledger", "window", "cursor", "digest", "packet", "socket", "buffer",
    "kernel", "schema", "stream",
];

fn doc_for(i: usize) -> String {
    let n = 2 + (i * 7) % 9;
    (0..n)
        .map(|k| WORDS[(i * 5 + k * 3) % WORDS.len()])
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_corpus(dir: &Path, files: usize) {
    for i in 0..files {
        let next = (i + 1) % files;
        let body = format!(
            "/// Handle {doc} for stage {i}.\npub fn handle_stage_{i}(input: &[u8]) -> usize {{\n    let mid = handle_stage_{next}(input);\n    input.len() + mid\n}}\n\npub fn helper_stage_{i}(x: usize) -> usize {{\n    x + {i}\n}}\n",
            doc = doc_for(i)
        );
        fs::write(dir.join(format!("stage_{i}.rs")), body).expect("write rs");
    }
    fs::write(
        dir.join("README.md"),
        "# Pipeline\n\nA staged byte pipeline where every stage hands its input to the next.\n\n## Stages\n\nEach stage reports how many bytes it saw.\n",
    )
    .expect("write md");
}

fn indexed(dir: &Path) -> Rag {
    let graph = Graph::open_in_memory(MemoryBudget::mib(128)).expect("graph");
    let rag = Rag::open(graph, RagConfig::default()).expect("rag");
    rag.index_dir(dir, &IndexOptions::default()).expect("index");
    rag
}

fn fixture(files: usize) -> (tempfile::TempDir, Rag) {
    let dir = tempfile::tempdir().expect("tmp");
    write_corpus(dir.path(), files);
    let rag = indexed(dir.path());
    (dir, rag)
}

fn soul_at(rag: &Rag, budget: usize) -> Soul {
    rag.soul(&SoulOptions {
        budget_tokens: budget,
        ..SoulOptions::default()
    })
    .expect("soul")
}

fn ids(soul: &Soul) -> BTreeSet<u64> {
    soul.sections.iter().map(|s| s.chunk_id).collect()
}

#[test]
fn soul_is_byte_identical_across_twenty_fresh_indexes() {
    let dir = tempfile::tempdir().expect("tmp");
    write_corpus(dir.path(), 40);
    let first = soul_at(&indexed(dir.path()), 400);
    assert!(first.sections.len() >= 5, "{}", first.sections.len());
    for run in 1..20 {
        let again = soul_at(&indexed(dir.path()), 400);
        assert_eq!(again.text, first.text, "run {run}");
        assert_eq!(again.digest, first.digest, "run {run}");
        assert_eq!(again.seed, first.seed, "run {run}");
        assert_eq!(again.rounds, first.rounds, "run {run}");
    }
}

#[test]
fn soul_is_identical_across_rayon_pools_of_one_two_four_and_eight_threads() {
    let (_dir, rag) = fixture(40);
    let want = soul_at(&rag, 400);
    assert!(!want.text.is_empty());
    for threads in [1usize, 2, 4, 8] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool");
        let got: Vec<String> = pool.install(|| {
            (0..16)
                .into_par_iter()
                .map(|_| soul_at(&rag, 400).text)
                .collect()
        });
        assert!(got.iter().all(|t| *t == want.text), "{threads} threads");
    }
}

#[test]
fn soul_never_exceeds_the_budget_across_two_hundred_budgets() {
    let (_dir, rag) = fixture(40);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut sizes = BTreeSet::new();
    for _ in 0..200 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let budget = 1 + (state >> 33) as usize % 3000;
        let soul = soul_at(&rag, budget);
        let counted = soul.text.chars().count().div_ceil(4);
        assert!(counted <= budget, "budget {budget}: {counted} tokens");
        assert_eq!(soul.tokens, counted);
        assert_eq!(soul.budget_tokens, budget);
        sizes.insert(soul.sections.len());
    }
    assert!(sizes.len() > 5, "budgets did not change the guide: {sizes:?}");
}

#[test]
fn refinement_stops_when_the_packing_settles_and_zero_rounds_is_the_initial_state() {
    let (_dir, rag) = fixture(40);
    let settled = rag
        .soul(&SoulOptions {
            budget_tokens: 400,
            max_rounds: 10_000,
            ..SoulOptions::default()
        })
        .expect("soul");
    assert!(settled.rounds >= 3, "{}", settled.rounds);
    assert!(settled.rounds < 200, "{}", settled.rounds);
    let capped = rag
        .soul(&SoulOptions {
            budget_tokens: 400,
            max_rounds: 2,
            ..SoulOptions::default()
        })
        .expect("soul");
    assert!(capped.rounds <= 2);
    let initial = rag
        .soul(&SoulOptions {
            budget_tokens: 400,
            max_rounds: 0,
            ..SoulOptions::default()
        })
        .expect("soul");
    assert_eq!(initial.rounds, 0);
    assert!(!initial.sections.is_empty());
}

#[test]
fn every_section_address_resolves_through_expand_to_the_exact_original_lines() {
    let (dir, rag) = fixture(40);
    let soul = soul_at(&rag, 600);
    assert!(soul.sections.len() >= 5);
    for s in &soul.sections {
        let original = fs::read_to_string(dir.path().join(&s.file)).expect("original");
        let lines: Vec<&str> = original.lines().collect();
        let want = lines[(s.start_line as usize - 1)..(s.end_line as usize)].join("\n");
        let got = rag.expand(s.chunk_id).expect("expand");
        assert_eq!(got.trim_end(), want.trim_end(), "{}:{}", s.file, s.start_line);
        assert!(
            soul.text
                .contains(&format!("// {}:{}-{} #{}", s.file, s.start_line, s.end_line, s.chunk_id)),
            "{} lacks its address line",
            s.file
        );
    }
}

#[test]
fn a_planted_hub_enters_a_tight_guide_only_through_propagation() {
    let dir = tempfile::tempdir().expect("tmp");
    fs::write(
        dir.path().join("hub.rs"),
        "pub fn quartzline_registry_lookup(key: u64) -> u64 {\n    key\n}\n",
    )
    .expect("hub");
    for i in 0..12 {
        let body = format!(
            "/// Caller number {i} validates {doc} against the registry before answering and reports every failure upward.\npub fn caller_{i}_entry(key: u64) -> u64 {{\n    quartzline_registry_lookup(key) + {i}\n}}\n",
            doc = doc_for(i)
        );
        fs::write(dir.path().join(format!("caller_{i}.rs")), body).expect("caller");
    }
    for i in 0..6 {
        let body = format!(
            "/// Standalone report {i} summarises {doc} for the nightly audit and nothing else here.\npub fn standalone_{i}_report(x: u64) -> u64 {{\n    x * {i}\n}}\n",
            doc = doc_for(i + 3)
        );
        fs::write(dir.path().join(format!("standalone_{i}.rs")), body).expect("standalone");
    }
    let rag = indexed(dir.path());
    let hub_id = {
        let hits = rag
            .search("quartzline_registry_lookup", &uriel_rag::SearchOptions::default())
            .expect("search");
        hits.iter().find(|h| h.file == "hub.rs").expect("hub chunk").chunk_id
    };
    let budget = 70;
    let diffused = soul_at(&rag, budget);
    let initial = rag
        .soul(&SoulOptions {
            budget_tokens: budget,
            max_rounds: 0,
            ..SoulOptions::default()
        })
        .expect("soul");
    assert!(!initial.sections.is_empty());
    assert!(!ids(&initial).contains(&hub_id), "hub must not be salient on its own");
    assert!(ids(&diffused).contains(&hub_id), "{}", diffused.text);
    assert_eq!(diffused.sections[0].chunk_id, hub_id);
}

fn chunk_total(rag: &Rag) -> usize {
    rag.stats().expect("stats").chunks
}

#[test]
fn a_second_identical_call_is_a_cache_hit_with_no_recomputation() {
    let (_dir, rag) = fixture(40);
    let first = soul_at(&rag, 400);
    assert!(!first.cache.hit);
    assert!(first.cache.rounds_run >= 1);
    assert_eq!(first.cache.features_computed, chunk_total(&rag));
    assert_eq!(first.cache.features_reused, 0);
    let second = soul_at(&rag, 400);
    assert!(second.cache.hit);
    assert_eq!(second.cache.rounds_run, 0);
    assert_eq!(second.cache.features_computed, 0);
    assert_eq!(second.text, first.text);
    assert_eq!(second.digest, first.digest);
    assert_eq!(second.sections, first.sections);
    assert_eq!(second.rounds, first.rounds);
    assert_eq!(second.seed, first.seed);
    assert_eq!(second.tokens, first.tokens);
    assert_eq!(second.omitted, first.omitted);
}

#[test]
fn a_different_budget_or_focus_is_a_different_cache_entry() {
    let (_dir, rag) = fixture(40);
    let a = soul_at(&rag, 400);
    let b = soul_at(&rag, 800);
    assert!(!b.cache.hit);
    assert_ne!(a.text, b.text);
    let focused = rag
        .soul(&SoulOptions {
            budget_tokens: 400,
            focus: Some("stage_1".into()),
            ..SoulOptions::default()
        })
        .expect("soul");
    assert!(!focused.cache.hit);
    assert!(focused.sections.iter().all(|s| s.file.starts_with("stage_1")));
    assert!(soul_at(&rag, 400).cache.hit);
    assert!(soul_at(&rag, 800).cache.hit);
    assert_eq!(rag.soul_cache_stats().expect("stats").guides, 3);
}

#[test]
fn editing_one_file_recomputes_only_its_chunks_and_supersedes_the_old_guide() {
    let (dir, rag) = fixture(40);
    let before = soul_at(&rag, 400);
    assert!(!before.cache.hit);
    fs::write(
        dir.path().join("stage_3.rs"),
        "/// Rewritten stage three now audits ledger digests and emits a summary per frame.\npub fn handle_stage_3(input: &[u8]) -> usize {\n    input.len()\n}\n",
    )
    .expect("edit");
    let report = rag.index_dir(dir.path(), &IndexOptions::default()).expect("reindex");
    assert_eq!(report.files_indexed, 1);
    let after = soul_at(&rag, 400);
    assert!(!after.cache.hit);
    assert!(after.cache.rounds_run >= 1);
    assert_eq!(after.cache.features_computed, report.chunks_added);
    assert_eq!(after.cache.features_reused, chunk_total(&rag) - report.chunks_added);
    let stats = rag.soul_cache_stats().expect("stats");
    assert_eq!(stats.guides, 1);
    assert_eq!(stats.features, chunk_total(&rag));
    assert_eq!(stats.sections, after.sections.len());
    assert!(soul_at(&rag, 400).cache.hit);
}

#[test]
fn cached_sections_are_persisted_one_node_per_section() {
    let (_dir, rag) = fixture(40);
    let soul = soul_at(&rag, 400);
    let stats = rag.soul_cache_stats().expect("stats");
    assert_eq!(stats.guides, 1);
    assert_eq!(stats.sections, soul.sections.len());
    assert!(stats.sections >= 5);
    assert!(stats.bytes > 0 && stats.bytes <= stats.cap, "{stats:?}");
}

fn sha256(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(fs::read(path).expect("read db"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn a_read_only_graph_computes_in_memory_and_leaves_the_database_file_unchanged() {
    let dir = tempfile::tempdir().expect("tmp");
    let corpus = dir.path().join("corpus");
    fs::create_dir(&corpus).expect("mkdir");
    write_corpus(&corpus, 40);
    let db = dir.path().join("g.uriel");
    {
        let graph = Graph::open(&db, MemoryBudget::mib(128)).expect("graph");
        let rag = Rag::open(graph, RagConfig::default()).expect("rag");
        rag.index_dir(&corpus, &IndexOptions::default()).expect("index");
    }
    let copy = dir.path().join("w.uriel");
    fs::copy(&db, &copy).expect("copy");
    let before = sha256(&db);
    let read_only = {
        let graph = Graph::open_read_only(&db, MemoryBudget::mib(128)).expect("ro");
        let rag = Rag::open(graph, RagConfig::default()).expect("ro rag");
        let first = soul_at(&rag, 400);
        let second = soul_at(&rag, 400);
        assert!(!first.cache.hit && !second.cache.hit);
        assert!(!first.sections.is_empty());
        assert_eq!(first.text, second.text);
        assert_eq!(rag.soul_cache_stats().expect("stats").guides, 0);
        first
    };
    assert_eq!(sha256(&db), before);
    let writable = {
        let graph = Graph::open(&copy, MemoryBudget::mib(128)).expect("graph");
        let rag = Rag::open(graph, RagConfig::default()).expect("rag");
        soul_at(&rag, 400)
    };
    assert_ne!(sha256(&copy), before, "a writable run must populate the cache");
    let reopened = Graph::open_read_only(&copy, MemoryBudget::mib(128)).expect("ro");
    let rag = Rag::open(reopened, RagConfig::default()).expect("ro rag");
    assert_eq!(rag.soul_cache_stats().expect("stats").guides, 1);
    assert!(soul_at(&rag, 400).cache.hit, "a read-only graph reads a cache a writer left");
    assert_eq!(writable.text, read_only.text);
}

#[test]
fn rendered_sections_concatenate_to_the_guide_text() {
    let (_dir, rag) = fixture(40);
    for budget in [120, 400, 1200] {
        let soul = soul_at(&rag, budget);
        let joined: String = soul.sections.iter().map(|s| s.render()).collect();
        assert_eq!(joined, soul.text, "budget {budget}");
    }
}
