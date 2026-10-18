use std::collections::{BTreeSet, HashMap, HashSet};

use crate::chunk::{is_doc_line, is_signature, Lang};
use crate::tokenize::terms;

const MIN_BUDGET: usize = 12;
const HEAD_MIN: usize = 16;
const MAX_LINE_CHARS: usize = 160;
const LEAD_WORDS: usize = 4;
const DEFINITION_KEYWORDS: &[&str] = &[
    "fn", "def", "class", "struct", "enum", "trait", "impl", "mod", "function", "interface", "type",
];

#[derive(Debug, Default)]
pub(crate) struct Corpus {
    df: HashMap<String, u32>,
    docs: u32,
}

impl Corpus {
    pub(crate) fn new() -> Corpus {
        Corpus::default()
    }

    pub(crate) fn add(&mut self, text: &str) {
        self.docs += 1;
        let unique: HashSet<String> = terms(text).into_iter().collect();
        for t in unique {
            *self.df.entry(t).or_insert(0) += 1;
        }
    }

    pub(crate) fn idf(&self, term: &str) -> f32 {
        let n = f64::from(self.docs.max(1));
        let df = f64::from(self.df.get(term).copied().unwrap_or(0));
        ((1.0 + (n - df + 0.5) / (df + 0.5)).ln()) as f32
    }
}

fn line_score(line_terms: Vec<String>, corpus: &Corpus) -> Option<f32> {
    let unique: BTreeSet<String> = line_terms.into_iter().collect();
    if unique.is_empty() {
        return None;
    }
    let sum: f64 = unique.iter().map(|t| f64::from(corpus.idf(t))).sum();
    Some((sum / (unique.len() as f64).sqrt()) as f32)
}

fn squeeze(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let hard: String = s.chars().take(max).collect();
    match hard.rfind(' ') {
        Some(i) if i > max / 2 => hard[..i].to_string(),
        _ => hard,
    }
}

fn token_weight(token: &str, corpus: &Corpus) -> f64 {
    let unique: BTreeSet<String> = terms(token).into_iter().collect();
    unique.iter().map(|t| f64::from(corpus.idf(t))).sum()
}

fn fit(line: &str, max: usize, corpus: &Corpus, signature: bool) -> String {
    let s = squeeze(line);
    if s.chars().count() <= max {
        return s;
    }
    let mut tokens: Vec<(usize, &str)> = Vec::new();
    for (i, tok) in s.split(' ').enumerate() {
        let marker = i == 0 && !tok.chars().any(char::is_alphanumeric);
        if marker || !terms(tok).is_empty() {
            tokens.push((i, tok));
        }
    }
    if tokens.is_empty() {
        return cut(&s, max);
    }
    let joined = |kept: &[(usize, &str)]| kept.iter().map(|t| t.1).collect::<Vec<_>>().join(" ");
    if joined(&tokens).chars().count() <= max {
        return joined(&tokens);
    }
    let weights: Vec<f64> = tokens.iter().map(|t| token_weight(t.1, corpus)).collect();
    let keyword = if signature {
        tokens
            .iter()
            .position(|t| t.0 < LEAD_WORDS && DEFINITION_KEYWORDS.contains(&t.1))
    } else {
        None
    };
    let marker = |k: usize| (tokens[k].0 == 0 && weights[k] == 0.0) || keyword == Some(k);
    let mut order: Vec<usize> = (0..tokens.len()).collect();
    order.sort_by(|&a, &b| {
        marker(b)
            .cmp(&marker(a))
            .then(weights[b].total_cmp(&weights[a]))
            .then(a.cmp(&b))
    });
    let mut used = 0usize;
    let mut kept: Vec<usize> = Vec::new();
    let mut anchored = false;
    for k in order {
        let cost = tokens[k].1.chars().count() + usize::from(!kept.is_empty());
        let anchor = !marker(k) && !anchored;
        if anchor || used + cost <= max {
            anchored |= anchor;
            used += cost;
            kept.push(k);
        }
    }
    kept.sort_unstable();
    let picked: Vec<(usize, &str)> = kept.into_iter().map(|k| tokens[k]).collect();
    joined(&picked)
}

pub(crate) fn compress(text: &str, lang: Lang, ratio: usize, corpus: &Corpus) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    let orig = text.chars().count();
    let budget = (orig / ratio.max(1)).max(MIN_BUDGET);
    let mut chosen: Vec<Option<String>> = vec![None; lines.len()];
    let mut used = 0usize;

    let head = fit(
        lines[first],
        budget.clamp(HEAD_MIN, MAX_LINE_CHARS),
        corpus,
        is_signature(lines[first], lang),
    );
    used += head.chars().count() + 1;
    chosen[first] = Some(head);

    let mut sigs = Vec::new();
    let mut docs = Vec::new();
    let mut rest: Vec<(usize, f32)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i == first || line.trim().is_empty() {
            continue;
        }
        if is_signature(line, lang) {
            sigs.push(i);
        } else if is_doc_line(line, lang) {
            docs.push(i);
        } else {
            let Some(score) = line_score(terms(line), corpus) else {
                continue;
            };
            rest.push((i, score));
        }
    }
    rest.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));

    for &i in &sigs {
        let s = squeeze(lines[i]);
        let cost = s.chars().count().min(MAX_LINE_CHARS) + 1;
        let remaining = budget.saturating_sub(used);
        if cost <= remaining {
            used += cost;
            chosen[i] = Some(cut(&s, MAX_LINE_CHARS));
        } else if remaining >= 20 {
            let c = cut(&s, remaining - 1);
            used += c.chars().count() + 1;
            chosen[i] = Some(c);
        }
    }
    let plain = docs.into_iter().chain(rest.into_iter().map(|(i, _)| i));
    for i in plain {
        let s = cut(&squeeze(lines[i]), MAX_LINE_CHARS);
        let cost = s.chars().count() + 1;
        if used + cost <= budget {
            used += cost;
            chosen[i] = Some(s);
        }
    }
    chosen.into_iter().flatten().collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        let mut v = vec![
            "/// Computes the rolling checksum of a buffer.".to_string(),
            "pub fn compute_checksum(data: &[u8]) -> u32 {".to_string(),
        ];
        for i in 0..40 {
            v.push(format!("    let value_{i} = data[{i}] as u32 + {i};"));
        }
        v.push("    let rare_zeppelin_term = 1;".into());
        v.push("}".into());
        v.join("\n")
    }

    #[test]
    fn keeps_first_line_signature_and_order() {
        let text = sample();
        let mut corpus = Corpus::new();
        corpus.add(&text);
        let m = compress(&text, Lang::Rust, 16, &corpus);
        let lines: Vec<&str> = m.lines().collect();
        assert_eq!(lines[0], "/// Computes the rolling checksum of a buffer.");
        assert!(lines[1].starts_with("pub fn compute_checksum"));
        let positions: Vec<usize> = lines
            .iter()
            .map(|l| {
                text.lines()
                    .position(|o| squeeze(o) == *l)
                    .expect("line from original")
            })
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "order preserved");
    }

    #[test]
    fn size_tracks_the_ratio() {
        let text = sample();
        let mut corpus = Corpus::new();
        corpus.add(&text);
        let m16 = compress(&text, Lang::Rust, 16, &corpus);
        let m4 = compress(&text, Lang::Rust, 4, &corpus);
        assert!(m4.len() > m16.len() * 2, "{} vs {}", m4.len(), m16.len());
        assert!(m16.len() <= text.len() / 16 + 80);
    }

    #[test]
    fn salient_rare_line_beats_boilerplate() {
        let text = sample();
        let mut corpus = Corpus::new();
        for _ in 0..30 {
            corpus.add("let value_0 = data[0] as u32 + 0;");
        }
        corpus.add(&text);
        let m = compress(&text, Lang::Rust, 8, &corpus);
        assert!(m.contains("rare_zeppelin_term"), "{m}");
    }

    #[test]
    fn empty_and_tiny_inputs() {
        let c = Corpus::new();
        assert_eq!(compress("", Lang::Text, 16, &c), "");
        assert_eq!(compress("\n  \n", Lang::Text, 16, &c), "");
        assert_eq!(compress("hello world", Lang::Text, 16, &c), "hello world");
    }

    #[test]
    fn long_lines_are_cut() {
        let long = "word ".repeat(500);
        let m = compress(&long, Lang::Text, 16, &Corpus::new());
        assert!(m.chars().count() <= MAX_LINE_CHARS);
        assert!(!m.is_empty());
    }

    fn skewed_corpus_and_terms() -> (Corpus, Vec<String>) {
        let words: Vec<String> = (0..40u32)
            .map(|i| {
                let a = char::from(b'a' + (i / 26) as u8);
                let b = char::from(b'a' + (i % 26) as u8);
                format!("q{a}{b}x")
            })
            .collect();
        let mut corpus = Corpus::new();
        for round in 0..64usize {
            let doc: Vec<&str> = words
                .iter()
                .enumerate()
                .filter(|(i, _)| round % (i * 3 % 17 + 1) == 0)
                .map(|(_, w)| w.as_str())
                .collect();
            corpus.add(&doc.join(" "));
        }
        let unique: std::collections::BTreeSet<String> =
            terms(&words.join(" ")).into_iter().collect();
        assert_eq!(unique.len(), words.len());
        (corpus, unique.into_iter().collect())
    }

    #[test]
    fn line_score_is_independent_of_set_iteration_order() {
        let (corpus, base) = skewed_corpus_and_terms();
        let first = line_score(base.clone(), &corpus).expect("score").to_bits();
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..2000 {
            let mut shuffled = base.clone();
            for i in (1..shuffled.len()).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                shuffled.swap(i, (seed % (i as u64 + 1)) as usize);
            }
            let got = line_score(shuffled, &corpus).expect("score").to_bits();
            assert_eq!(got, first);
        }
    }

    #[test]
    fn compress_is_byte_identical_across_calls() {
        let (corpus, words) = skewed_corpus_and_terms();
        let mut text = String::from(
            "header line
",
        );
        for chunk in words.chunks(8) {
            text.push_str(&format!(
                "    {}
",
                chunk.join(" ")
            ));
        }
        for chunk in words.rchunks(7) {
            text.push_str(&format!(
                "    {} {}
",
                chunk.join(" "),
                chunk[0]
            ));
        }
        let first = compress(&text, Lang::Text, 3, &corpus);
        for _ in 0..500 {
            assert_eq!(compress(&text, Lang::Text, 3, &corpus), first);
        }
    }

    #[test]
    fn a_short_first_line_lets_a_small_chunk_reach_the_ratio() {
        let mut text = String::from("fn run() {\n");
        for i in 0..12 {
            text.push_str(&format!("    step_{i}();\n"));
        }
        let mut corpus = Corpus::new();
        corpus.add(&text);
        let m = compress(&text, Lang::Rust, 16, &corpus);
        assert!(m.starts_with("fn run() {"), "{m}");
        assert!(
            m.chars().count() <= 16,
            "{} chars from {}: {m}",
            m.chars().count(),
            text.len()
        );
    }

    #[test]
    fn an_overlong_first_line_keeps_its_rarest_word_not_its_prefix() {
        let first = "/// the quick brown fox jumps over a lazy zeppelinwordx";
        let mut text = String::from(first);
        text.push('\n');
        for i in 0..24 {
            text.push_str(&format!("    let pad_{i} = {i};\n"));
        }
        let mut corpus = Corpus::new();
        for _ in 0..20 {
            corpus.add("the quick brown fox jumps over a lazy dog");
        }
        corpus.add(&text);
        let m = compress(&text, Lang::Rust, 16, &corpus);
        let head = m.lines().next().expect("head");
        assert!(head.starts_with("///"), "{head}");
        assert!(head.contains("zeppelinwordx"), "{head}");
        assert!(head.chars().count() <= text.len() / 16, "{head}");
    }

    #[test]
    fn the_first_line_is_never_dropped_even_when_the_budget_is_tiny() {
        let text = "/// Computes a rolling checksum\nfn a() {}\nfn b() {}\nfn c() {}";
        let m = compress(text, Lang::Rust, 64, &Corpus::new());
        assert!(!m.is_empty());
        assert!(m.starts_with("///"), "{m}");
    }

    #[test]
    fn a_cut_signature_head_keeps_its_definition_keyword() {
        let text = "def parse_config(path):
    import json
    with open(path) as handle:
        return json.load(handle)
";
        let mut corpus = Corpus::new();
        corpus.add(text);
        let m = compress(text, Lang::Python, 16, &corpus);
        assert_eq!(m, "def parse_config(path):");
        let rust = "pub async fn handle_request(stream: Stream) -> Reply {
    todo!()
}
";
        let mut corpus = Corpus::new();
        corpus.add(rust);
        let m = compress(rust, Lang::Rust, 16, &corpus);
        assert!(m.starts_with("fn ") || m.contains(" fn "), "{m}");
    }
}
