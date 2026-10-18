mod common;

use common::{span_texts, SplitMix};
use uriel_terse::{scan, Kind};

fn kinds(input: &str) -> Vec<(Kind, &str)> {
    scan(input)
        .iter()
        .map(|s| (s.kind, &input[s.start..s.end]))
        .collect()
}

fn assert_span(input: &str, expected: &str) {
    assert!(
        span_texts(input).contains(&expected),
        "expected span {expected:?} in {input:?}, got {:?}",
        kinds(input)
    );
}

fn assert_no_span_touching(input: &str, needle: &str) {
    let at = input.find(needle).unwrap();
    let hit = scan(input)
        .iter()
        .any(|s| s.start < at + needle.len() && at < s.end);
    assert!(
        !hit,
        "unexpected span over {needle:?} in {input:?}: {:?}",
        kinds(input)
    );
}

#[test]
fn fenced_block_is_one_span() {
    let input = "a\n```rust\nlet x_y = 1;\nthe a\n```\nb";
    assert_eq!(
        kinds(input),
        vec![(Kind::Fence, "```rust\nlet x_y = 1;\nthe a\n```")]
    );
}

#[test]
fn tilde_fence_and_longer_closer() {
    let input = "~~~\nx\n~~~~\nafter";
    assert_eq!(kinds(input), vec![(Kind::Fence, "~~~\nx\n~~~~")]);
}

#[test]
fn unclosed_fence_runs_to_end() {
    let input = "intro\n```\ncode the a\nmore";
    assert_eq!(kinds(input), vec![(Kind::Fence, "```\ncode the a\nmore")]);
}

#[test]
fn inline_code_matches_run_length() {
    assert_span("use `Vec<u8>` now", "`Vec<u8>`");
    assert_span("use ``a ` b`` now", "``a ` b``");
}

#[test]
fn unmatched_backtick_is_plain() {
    assert!(scan("a ` b the c").is_empty());
}

#[test]
fn urls_drop_trailing_punctuation() {
    assert_span(
        "see https://example.com/a_b?x=1, ok",
        "https://example.com/a_b?x=1",
    );
    assert_span("go to www.example.org/path.", "www.example.org/path");
    assert_span("(http://localhost:8080/x)", "http://localhost:8080/x");
}

#[test]
fn paths() {
    assert_span("edit src/lib.rs.", "src/lib.rs");
    assert_span("in crates/uriel-rag/src now", "crates/uriel-rag/src");
    assert_span(
        "open C:\\Users\\seal\\x.txt please",
        "C:\\Users\\seal\\x.txt",
    );
    assert_span("see README.md for more", "README.md");
    assert_span("at ~/notes/todo and ./a/b", "~/notes/todo");
    assert_span("at ~/notes/todo and ./a/b", "./a/b");
    assert_no_span_touching("use and/or here", "and/or");
}

#[test]
fn identifiers() {
    assert_span(
        "call std::collections::HashMap please",
        "std::collections::HashMap",
    );
    assert_span("the snake_case_name var", "snake_case_name");
    assert_span("the camelCase var", "camelCase");
    assert_span("the HashMap type", "HashMap");
    assert_span("run with --release now", "--release");
    assert_span("call do_thing() now", "do_thing()");
    assert_no_span_touching("the plain words here", "plain");
}

#[test]
fn hashes() {
    assert_span("commit 20123e2 fixed", "20123e2");
    assert_span("sha 124b89d5b7bb0000 here", "124b89d5b7bb0000");
    assert_no_span_touching("the defaced wall", "defaced");
}

#[test]
fn versions_and_numbers() {
    assert_span("on v1.2.3 now", "v1.2.3");
    assert_span("msrv 1.90 set", "1.90");
    assert_span("tag 1.2.3-rc.1 out", "1.2.3-rc.1");
    assert_span("took 12 ms total", "12 ms");
    assert_span("uses 3.5GB total", "3.5GB");
    assert_span("about 10% done", "10%");
    assert_span("got 1,234.56 back", "1,234.56");
    assert_span("fail 2 of 40", "40");
    assert_span("an x86 box", "x86");
}

#[test]
fn quoted_text() {
    assert_span("say \"hello world\" now", "\"hello world\"");
    assert_span("say 'hello world' now", "'hello world'");
    assert_span(
        "say \u{201c}hello world\u{201d} now",
        "\u{201c}hello world\u{201d}",
    );
    assert_no_span_touching("it's fine and that's ok", "fine");
}

#[test]
fn latex() {
    assert_span("so $x^2 + y_1$ holds", "$x^2 + y_1$");
    assert_span("so $$a=b$$ holds", "$$a=b$$");
    assert_span("so \\(a+b\\) holds", "\\(a+b\\)");
    assert_span("so \\frac{a}{b} holds", "\\frac{a}{b}");
    assert_span("costs $5 or maybe $6 total", "5");
}

#[test]
fn spans_are_sorted_disjoint_and_on_boundaries() {
    let mut rng = SplitMix(7);
    for _ in 0..3000 {
        let s = common::random_sentence(&mut rng);
        let spans = scan(&s);
        let mut prev = 0;
        for sp in &spans {
            assert!(sp.start >= prev && sp.start < sp.end && sp.end <= s.len());
            assert!(s.is_char_boundary(sp.start) && s.is_char_boundary(sp.end));
            prev = sp.end;
        }
    }
}

#[test]
fn arbitrary_bytes_never_panic() {
    let mut rng = SplitMix(99);
    for _ in 0..2000 {
        let len = rng.below(200);
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let s = String::from_utf8_lossy(&bytes);
        let spans = scan(&s);
        for sp in &spans {
            assert!(s.is_char_boundary(sp.start) && s.is_char_boundary(sp.end));
        }
    }
}

#[test]
fn hostile_inputs_are_bounded() {
    let started = std::time::Instant::now();
    for pat in [
        "`",
        "\"",
        "'",
        "$",
        "`` ",
        "\u{201c}",
        "a`b\"c'd$e",
        "```\n",
    ] {
        let s = pat.repeat(100_000 / pat.len());
        let _ = scan(&s);
    }
    let ramp: String = (1..400).map(|n| format!("{} ", "`".repeat(n))).collect();
    let _ = scan(&ramp);
    assert!(
        started.elapsed().as_secs() < 20,
        "scan too slow: {:?}",
        started.elapsed()
    );
}
