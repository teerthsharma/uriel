#![allow(dead_code)]

use uriel_terse::{compact, estimate_tokens, scan, Level};

pub struct SplitMix(pub u64);

impl SplitMix {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NEG_WORDS: [&str; 11] = [
    "not", "no", "never", "cannot", "nor", "neither", "none", "nothing", "nobody", "nowhere",
    "without",
];

pub fn neg_count(s: &str) -> usize {
    let lower = s.to_lowercase();
    let mut total = 0;
    let mut run = String::new();
    let mut flush = |run: &mut String, total: &mut usize| {
        if !run.is_empty() {
            let norm = run.replace('\u{2019}', "'");
            if NEG_WORDS.contains(&norm.as_str()) {
                *total += 1;
            }
            if norm.ends_with("n't") {
                *total += 1;
            }
            run.clear();
        }
    };
    for c in lower.chars() {
        if c.is_alphabetic() || c == '\'' || c == '\u{2019}' {
            run.push(c);
        } else {
            flush(&mut run, &mut total);
        }
    }
    flush(&mut run, &mut total);
    total
}

pub fn digit_runs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out.sort();
    out
}

pub fn span_texts(input: &str) -> Vec<&str> {
    scan(input).iter().map(|s| &input[s.start..s.end]).collect()
}

pub fn assert_invariants(input: &str, level: Level) {
    let out = compact(input, level);
    for text in span_texts(input) {
        let need = span_texts(input).iter().filter(|t| **t == text).count();
        assert!(
            out.matches(text).count() >= need,
            "protected span {text:?} lost at {level:?}\ninput: {input:?}\noutput: {out:?}"
        );
    }
    assert_eq!(
        digit_runs(input),
        digit_runs(&out),
        "numbers changed at {level:?}\ninput: {input:?}\noutput: {out:?}"
    );
    assert!(
        neg_count(&out) >= neg_count(input),
        "negation lost at {level:?}\ninput: {input:?}\noutput: {out:?}"
    );
    assert!(
        out.len() <= input.len(),
        "output longer at {level:?}\ninput: {input:?}\noutput: {out:?}"
    );
    assert!(estimate_tokens(&out) <= estimate_tokens(input));
    assert_eq!(
        compact(&out, level),
        out,
        "not idempotent at {level:?}\ninput: {input:?}"
    );
}

pub const LEVELS: [Level; 3] = [Level::Lite, Level::Full, Level::Ultra];

pub const HANDWRITTEN: &[&str] = &[
    "Sure, I would be happy to help. The cache is stale because we utilize a lot of background workers.",
    "I'm not certain, but likely the cache is stale. I hope this helps!",
    "No problem. It does not crash, and it never hangs.",
    "Great question! The function `rag_search` in src/lib.rs takes 12 ms and uses 3.5 GB of memory.",
    "Please note that the build cannot run on v1.2.3 without the flag --release.",
    "I think the std::collections::HashMap is used here, but I don't know for sure.",
    "Let me know if you have any questions. Feel free to reach out.",
    "The commit 20123e2 fixed it. Approximately 40% of the requests failed in order to retry.",
    "Basically, this is really just a very simple wrapper around the database configuration.",
    "Sorry about that. I apologize for the confusion. The result is not correct.",
    "Maybe the value of $x^2 + y_1$ is wrong, perhaps because of \"quoted text here\" and more.",
    "Here is the code:\n\n```rust\nlet the_value = a + b;\nlet a = the(a);\n```\n\nThe code is correct.",
    "1. Use the cache. Then restart the server.\n2. It is important to note that the server is not ready.",
    "- The first item is done.\n- The second item is not done.\n    The indented line is kept as is.",
    "| the | a |\n|---|---|\n| is | are |",
    "Dr. Smith said that e.g. the service is down. It is a problem. Is it fixed?",
    "It seems that the parser might fail on camelCaseNames and snake_case_names alike.",
    "As far as I know, the test passes. To the best of my knowledge, it never failed.",
    "We attempt to utilize the information, however the implementation is insufficient.",
    "The API is stable. The API is not stable. There is a bug. There are two bugs.",
    "Is the file src/main.rs present? Where is the log at C:\\logs\\run.txt located?",
    "See https://example.com/a_b?x=1, and www.example.org/path for details.",
    "It is not just a bug, it is really not very good, and not really fixable.",
    "Thanks for asking! Thank you. Certainly, absolutely, of course.",
    "",
    " ",
    "\n\n\n",
    "a",
    "the the the cat",
    "The",
    "Sure!",
    "Unicode: \u{e9}t\u{e9} is the best \u{1f600} and the \u{2019}tis not.",
];

pub fn random_sentence(rng: &mut SplitMix) -> String {
    const WORDS: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "cache", "server", "not", "no", "never", "cannot",
        "don't", "isn't", "really", "very", "just", "basically", "utilize", "identify",
        "approximately", "because", "however", "and", "with", "in", "order", "to", "of", "it",
        "that", "this", "there", "here", "what", "perhaps", "maybe", "probably", "I", "think",
        "believe", "I'm", "sure", "certain", "but", "likely", "fail", "works", "database",
        "configuration", "function", "implementation", "Sure,", "Certainly!", "Great", "question",
        "hope", "helps", "Sorry", "apologize", "confusion", "thanks", "please", "note",
        "important", "e.g.", "Dr.", "vs.", "etc.", "`code_span`", "src/lib.rs", "snake_case",
        "camelCase", "std::fmt", "https://x.org/a_b", "12", "3.5", "40%", "12ms", "v1.2.3",
        "20123e2", "\"quoted words\"", "$x_1$", "--flag", "C:\\a\\b.txt", "\u{e9}t\u{e9}",
        "\u{1f600}", "(", ")", ",", ".", "!", "?", ":", ";", "-", "*", "#", "&", "w/",
    ];
    let n = 1 + rng.below(14);
    let mut s = String::new();
    for i in 0..n {
        if i > 0 {
            let r = rng.below(20);
            s.push_str(match r {
                0 => "  ",
                1 => "\n",
                2 => "\t",
                3 => "",
                _ => " ",
            });
        }
        s.push_str(rng.pick(WORDS));
    }
    s
}
