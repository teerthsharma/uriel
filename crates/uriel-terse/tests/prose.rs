mod common;

use common::{neg_count, SplitMix, HANDWRITTEN, LEVELS};
use uriel_terse::{compact, estimate_tokens, Level};

#[test]
fn lite_strips_preamble_and_postamble_but_keeps_articles() {
    let out = compact("Sure, the cache is stale. I hope this helps!", Level::Lite);
    assert_eq!(out, "The cache is stale.");
}

#[test]
fn full_drops_articles_and_copulas() {
    assert_eq!(compact("The cache is stale.", Level::Full), "Cache stale.");
}

#[test]
fn full_maps_long_words_to_short_ones() {
    assert_eq!(
        compact("We utilize it to identify faults.", Level::Full),
        "We use it to find faults."
    );
    assert_eq!(
        compact("It must happen in order to work.", Level::Full),
        "It must happen to work."
    );
}

#[test]
fn lite_leaves_long_words_alone() {
    assert_eq!(compact("We utilize it.", Level::Lite), "We utilize it.");
}

#[test]
fn uncertainty_is_mapped_not_deleted() {
    assert_eq!(
        compact("I'm not certain, but likely the cache is stale.", Level::Full),
        "Likely cache stale. Me not sure."
    );
    assert_eq!(
        compact("I'm not certain, but likely the cache is stale.", Level::Lite),
        "Likely the cache is stale. Me not sure."
    );
    assert_eq!(compact("I think it works.", Level::Full), "Likely it works.");
    assert_eq!(compact("Perhaps it works.", Level::Full), "Maybe it works.");
}

#[test]
fn filler_words_dropped_but_not_after_negation() {
    assert_eq!(compact("It is really slow.", Level::Lite), "It is slow.");
    assert_eq!(
        compact("It is not really slow.", Level::Lite),
        "It is not really slow."
    );
}

#[test]
fn negated_pleasantries_survive() {
    let out = compact("No problem. It works.", Level::Full);
    assert!(out.contains("No problem"), "{out:?}");
    let out = compact("Don't hesitate to ask. It works.", Level::Full);
    assert!(out.contains("Don't hesitate"), "{out:?}");
    assert!(neg_count(&out) >= 1);
}

#[test]
fn protected_spans_pass_through() {
    let input =
        "The function `rag_search` in src/lib.rs takes 12 ms and 3.5 GB, see https://x.org/a_b.";
    for level in LEVELS {
        let out = compact(input, level);
        for keep in [
            "`rag_search`",
            "src/lib.rs",
            "12 ms",
            "3.5 GB",
            "https://x.org/a_b",
        ] {
            assert!(out.contains(keep), "{keep} lost at {level:?}: {out:?}");
        }
    }
}

#[test]
fn fenced_blocks_are_untouched() {
    let input = "The code:\n\n```rust\nlet the_value = a + b;\nlet a = the(a); // is really\n```\n\nIt is the end.";
    for level in LEVELS {
        let out = compact(input, level);
        assert!(
            out.contains("```rust\nlet the_value = a + b;\nlet a = the(a); // is really\n```"),
            "{out:?}"
        );
    }
}

#[test]
fn indented_and_table_lines_are_verbatim() {
    let input = "    the a is\n| the | a |\nthe cat is here";
    let out = compact(input, Level::Full);
    assert!(out.starts_with("    the a is\n| the | a |\n"), "{out:?}");
}

#[test]
fn full_puts_one_idea_per_line() {
    assert_eq!(
        compact("Use the cache. Then restart.", Level::Full),
        "Use cache.\nThen restart."
    );
    assert_eq!(
        compact("Use the cache. Then restart.", Level::Lite),
        "Use the cache. Then restart."
    );
}

#[test]
fn abbreviations_do_not_split_sentences() {
    let out = compact("Dr. Smith said e.g. this works. Fine.", Level::Full);
    assert!(
        out.starts_with("Dr. Smith said e.g. this works.\n"),
        "{out:?}"
    );
}

#[test]
fn list_markers_survive() {
    let out = compact(
        "1. Use the cache. Then restart.\n- The item is done.",
        Level::Full,
    );
    assert!(out.starts_with("1. Use cache."), "{out:?}");
    assert!(out.contains("\n- Item done"), "{out:?}");
}

#[test]
fn ultra_abbreviates_and_full_does_not() {
    let input = "The configuration of the database and the implementation.";
    assert!(compact(input, Level::Full).contains("configuration"));
    let ultra = compact(input, Level::Ultra);
    assert!(
        ultra.contains("config") && ultra.contains("DB") && ultra.contains("impl"),
        "{ultra:?}"
    );
    assert!(ultra.contains('&'), "{ultra:?}");
}

#[test]
fn levels_never_get_longer() {
    for input in HANDWRITTEN {
        let lite = compact(input, Level::Lite).len();
        let full = compact(input, Level::Full).len();
        let ultra = compact(input, Level::Ultra).len();
        assert!(
            lite <= input.len() && full <= lite && ultra <= full,
            "{input:?}"
        );
    }
}

#[test]
fn handwritten_corpus_invariants() {
    for input in HANDWRITTEN {
        for level in LEVELS {
            common::assert_invariants(input, level);
        }
    }
}

#[test]
fn random_corpus_invariants() {
    let mut rng = SplitMix(2024);
    for _ in 0..4000 {
        let n = 1 + rng.below(4);
        let text: Vec<String> = (0..n).map(|_| common::random_sentence(&mut rng)).collect();
        let input = text.join(if rng.below(2) == 0 { " " } else { "\n" });
        for level in LEVELS {
            common::assert_invariants(&input, level);
        }
    }
}

#[test]
fn arbitrary_bytes_never_panic() {
    let mut rng = SplitMix(5);
    for _ in 0..2000 {
        let len = rng.below(300);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let s = String::from_utf8_lossy(&bytes).into_owned();
        for level in LEVELS {
            let out = compact(&s, level);
            assert!(out.len() <= s.len());
        }
    }
}

#[test]
fn real_repo_text_invariants() {
    let readme = include_str!("../../../README.md");
    let plan = include_str!("../../../PLAN.md");
    for doc in [readme, plan] {
        for para in doc.split("\n\n") {
            for level in LEVELS {
                common::assert_invariants(para, level);
            }
        }
    }
}

#[test]
fn savings_are_real_on_chatty_text() {
    let input = "Sure, I would be happy to help. Basically, the cache is really stale because we utilize a lot of background workers. I hope this helps!";
    let out = compact(input, Level::Full);
    assert!(
        estimate_tokens(&out) * 10 <= estimate_tokens(input) * 7,
        "{out:?}"
    );
}
