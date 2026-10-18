use std::collections::BTreeSet;

use serde_json::Value;
use uriel_core::{Graph, MemoryBudget};
use uriel_mcp::compact::{decode_pack, proxy_tokens};
use uriel_mcp::config::Format;
use uriel_mcp::view::{render_view, run_view, ViewRequest};
use uriel_rag::chunk::ChunkOptions;
use uriel_rag::{Hit, Rag, RagConfig, SearchOptions};

const QUESTION: &str = "ledger cursor orphan vault tally";

fn lines(word: &str, n: usize) -> String {
    (1..=n)
        .map(|i| format!("{word} entry number {i} records the {word} state for {word} audit"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn empty_rag() -> Rag {
    Rag::open(
        Graph::open_in_memory(MemoryBudget::mib(64)).expect("graph"),
        RagConfig {
            chunking: ChunkOptions {
                window_lines: 4,
                overlap_lines: 0,
                min_lines: 1,
                max_lines: 4,
            },
            ..RagConfig::default()
        },
    )
    .expect("rag")
}

fn fixture_of(files: &[(&str, &str, usize)]) -> Rag {
    let rag = empty_rag();
    for (name, word, n) in files {
        rag.index_text(name, &lines(word, *n)).expect("index");
    }
    rag
}

fn fixture() -> Rag {
    fixture_of(&[
        ("ledger.txt", "ledger", 12),
        ("cursor.txt", "cursor", 8),
        ("orphan.txt", "orphan", 4),
        ("vault.txt", "vault", 4),
        ("tally.txt", "tally", 4),
    ])
}

fn one_chain() -> Rag {
    fixture_of(&[("ledger.txt", "ledger", 12)])
}

fn only_singletons() -> Rag {
    fixture_of(&[
        ("orphan.txt", "orphan", 4),
        ("vault.txt", "vault", 4),
        ("tally.txt", "tally", 4),
    ])
}

fn request(format: Format, budget: usize, islands: bool) -> ViewRequest<'static> {
    ViewRequest {
        question: QUESTION,
        focus: None,
        k: 12,
        salient: 0,
        budget,
        format,
        islands,
    }
}

fn view(rag: &Rag, format: Format, budget: usize, islands: bool) -> String {
    run_view(rag, &request(format, budget, islands)).expect("view")
}

const GOLDEN_COMPACT_600: &str = "#5 omit=3\n>orphan.txt:1-4#8L1 orphan entry number 1 records the orphan state for orphan audit\n>cursor.txt:1-4#5L2 cursor entry number 1 records the cursor state\n>5-8#6L2 cursor entry number 5 records the cursor state\n>vault.txt:1-4#10L2 vault entry number 1 records the vault state\n>tally.txt:1-4#12 tally entry number 1 records the tally state for tally audit -- tally entry number 2 records\n";
const GOLDEN_COMPACT_40: &str = "#2 omit=6\n>cursor.txt:1-4#5L1 cursor entry number 1 records the cursor state for cursor audit\n>vault.txt:1-4#10L2 vault entry number 1 records the vault state\n";
const GOLDEN_JSON_600: &str = r##"{"budget_tokens":600,"candidates":8,"hits":[{"chunk_id":8,"end_line":4,"file":"orphan.txt","level":1,"score":0.032786883413791656,"start_line":1},{"chunk_id":5,"end_line":4,"file":"cursor.txt","level":2,"score":0.0317540317773819,"start_line":1},{"chunk_id":6,"end_line":8,"file":"cursor.txt","level":2,"score":0.03125763311982155,"start_line":5},{"chunk_id":10,"end_line":4,"file":"vault.txt","level":2,"score":0.03083491325378418,"start_line":1},{"chunk_id":12,"end_line":4,"file":"tally.txt","level":3,"score":0.03079839050769806,"start_line":1}],"omitted":3,"selected":5,"text":"// orphan.txt:1-4 #8\norphan entry number 1 records the orphan state for orphan audit\n// cursor.txt:1-4 #5\ncursor entry number 1 records the cursor state\n// cursor.txt:5-8 #6\ncursor entry number 5 records the cursor state\n// vault.txt:1-4 #10\nvault entry number 1 records the vault state\n// tally.txt:1-4 #12 tally entry number 1 records the tally state for tally audit -- tally entry number 2 records\n","tokens":101}"##;

fn sections(compact: &str) -> Vec<&str> {
    let bytes = compact.as_bytes();
    let mut starts: Vec<usize> = compact
        .match_indices('#')
        .filter(|(i, _)| *i == 0 || bytes[i - 1] == b'\n')
        .filter(|(i, _)| bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        .map(|(i, _)| i)
        .collect();
    starts.push(compact.len());
    starts.windows(2).map(|w| &compact[w[0]..w[1]]).collect()
}

fn files_of(section: &str) -> BTreeSet<String> {
    decode_pack(section)
        .unwrap_or_else(|e| panic!("{e}: {section}"))
        .hits
        .into_iter()
        .map(|h| h.file)
        .filter(|f| !f.is_empty())
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn view_without_islands_is_byte_identical_to_the_pre_islands_output() {
    let rag = fixture();
    assert_eq!(view(&rag, Format::Compact, 600, false), GOLDEN_COMPACT_600);
    assert_eq!(view(&rag, Format::Compact, 40, false), GOLDEN_COMPACT_40);
    assert_eq!(view(&rag, Format::Json, 600, false), GOLDEN_JSON_600);
}

#[test]
fn a_single_island_renders_the_same_bytes_with_islands_on_and_off() {
    for rag in [one_chain(), only_singletons()] {
        for format in [Format::Compact, Format::Json] {
            for budget in [10, 60, 600, 5000] {
                let off = view(&rag, format, budget, false);
                assert_eq!(view(&rag, format, budget, true), off, "{format:?} {budget}");
            }
        }
    }
}

#[test]
fn islands_render_one_section_each_ordered_by_the_island_of_the_best_hit() {
    let rag = fixture();
    let text = view(&rag, Format::Compact, 600, true);
    let parts = sections(&text);
    let files: Vec<BTreeSet<String>> = parts.iter().map(|p| files_of(p)).collect();
    assert_eq!(
        files,
        vec![
            set(&["orphan.txt", "tally.txt", "vault.txt"]),
            set(&["cursor.txt"]),
            set(&["ledger.txt"]),
        ],
        "{text}"
    );
    assert_eq!(parts.concat(), text);
    let first: Vec<String> = decode_pack(parts[0])
        .expect("decode")
        .hits
        .into_iter()
        .map(|h| h.file)
        .filter(|f| !f.is_empty())
        .collect();
    assert_eq!(first, ["orphan.txt", "vault.txt", "tally.txt"]);
}

#[test]
fn islands_json_lists_every_island_and_its_budget_share() {
    let rag = fixture();
    let v: Value = serde_json::from_str(&view(&rag, Format::Json, 600, true)).expect("json");
    let islands = v["islands"].as_array().expect("islands");
    let labels: Vec<u64> = islands
        .iter()
        .map(|i| i["label"].as_u64().expect("label"))
        .collect();
    let mut sorted = labels.clone();
    sorted.sort_unstable();
    assert_eq!(labels, sorted);
    assert_eq!(labels.len(), 3, "{v}");
    let shares: u64 = islands
        .iter()
        .map(|i| i["budget_tokens"].as_u64().expect("budget"))
        .sum();
    assert_eq!(shares, 600);
    let candidates: u64 = islands
        .iter()
        .map(|i| i["candidates"].as_u64().expect("candidates"))
        .sum();
    assert_eq!(Some(candidates), v["candidates"].as_u64());
    assert!(v["tokens"].as_u64() <= Some(600), "{v}");
    assert_eq!(
        v["hits"].as_array().expect("hits").len() as u64,
        islands
            .iter()
            .map(|i| i["kept"].as_u64().expect("kept"))
            .sum::<u64>()
    );
}

#[test]
fn the_sections_together_never_exceed_the_budget() {
    let rag = fixture();
    let mut nonempty = 0;
    for budget in 1..400 {
        let text = view(&rag, Format::Compact, budget, true);
        assert!(
            proxy_tokens(&text) <= budget,
            "{} > {budget}: {text}",
            proxy_tokens(&text)
        );
        nonempty += usize::from(text != "#0");
        let v: Value = serde_json::from_str(&view(&rag, Format::Json, budget, true)).expect("json");
        assert!(v["tokens"].as_u64() <= Some(budget as u64), "{v}");
        assert!(proxy_tokens(v["text"].as_str().expect("text")) <= budget, "{v}");
    }
    assert!(nonempty > 100, "{nonempty}");
}

fn candidates(rag: &Rag) -> Vec<Hit> {
    rag.search(
        QUESTION,
        &SearchOptions {
            k: 12,
            budget_tokens: 600,
            expand_hops: 0,
            use_utility: true,
        },
    )
    .expect("search")
}

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n
    }
}

fn shuffled(hits: &[Hit], rng: &mut Lcg) -> Vec<Hit> {
    let mut v = hits.to_vec();
    for i in (1..v.len()).rev() {
        v.swap(i, rng.below(i + 1));
    }
    v
}

#[test]
fn the_island_view_is_byte_identical_over_20_shuffled_insertions_of_the_same_hits() {
    let rag = fixture();
    let hits = candidates(&rag);
    let mut rng = Lcg(0x5ea1);
    let mut moved = 0;
    for format in [Format::Compact, Format::Json] {
        let req = request(format, 600, true);
        let want = render_view(&rag, &hits, &req).expect("view");
        for _ in 0..20 {
            let order = shuffled(&hits, &mut rng);
            moved += usize::from(order != hits);
            assert_eq!(
                render_view(&rag, &order, &req).expect("view"),
                want,
                "{format:?}"
            );
        }
    }
    assert!(moved >= 30, "{moved} of 40 shuffles changed the order");
}

#[test]
fn the_island_view_is_byte_identical_across_1_2_4_and_8_rayon_threads() {
    let rag = fixture();
    let hits = candidates(&rag);
    for format in [Format::Compact, Format::Json] {
        let req = request(format, 600, true);
        let want = render_view(&rag, &hits, &req).expect("view");
        for threads in [1, 2, 4, 8] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .expect("pool");
            let got = pool.install(|| render_view(&rag, &hits, &req).expect("view"));
            assert_eq!(got, want, "{format:?} on {threads} threads");
        }
    }
}

#[test]
fn budget_shares_are_proportional_to_the_summed_hit_scores_in_integer_arithmetic() {
    let rag = fixture();
    let v: Value = serde_json::from_str(&view(&rag, Format::Json, 600, true)).expect("json");
    let shares: Vec<u64> = v["islands"]
        .as_array()
        .expect("islands")
        .iter()
        .map(|i| i["budget_tokens"].as_u64().expect("budget"))
        .collect();
    assert_eq!(shares, [228, 152, 220]);
}
