use std::collections::{HashMap, HashSet};

use crate::chunk::{is_doc_line, is_signature, Lang};
use crate::tokenize::terms;

const MIN_BUDGET: usize = 32;
const MAX_LINE_CHARS: usize = 160;

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

pub(crate) fn compress(text: &str, lang: Lang, ratio: usize, corpus: &Corpus) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(first) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    let orig = text.chars().count();
    let budget = (orig / ratio.max(1)).max(MIN_BUDGET);
    let mut chosen: Vec<Option<String>> = vec![None; lines.len()];
    let mut used = 0usize;

    let head = cut(&squeeze(lines[first]), budget.clamp(48, MAX_LINE_CHARS));
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
            let unique: HashSet<String> = terms(line).into_iter().collect();
            if unique.is_empty() {
                continue;
            }
            let sum: f32 = unique.iter().map(|t| corpus.idf(t)).sum();
            rest.push((i, sum / (unique.len() as f32).sqrt()));
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
}
