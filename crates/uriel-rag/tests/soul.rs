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
