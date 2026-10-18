mod engine;
mod lex;
mod rules;

use lex::{tokenize, Tok, K};
use rules::Index;

const MAX_PASSES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Lite,
    Full,
    Ultra,
}

pub fn compact(text: &str, level: Level) -> String {
    let idx = Index::new(level);
    let mut cur = pass(text, level, &idx);
    for _ in 0..MAX_PASSES {
        let next = pass(&cur, level, &idx);
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

fn split_lines(tokens: Vec<Tok>) -> Vec<(Vec<Tok>, String)> {
    let mut lines = Vec::new();
    let mut cur = Vec::new();
    for tok in tokens {
        if tok.k == K::Newline {
            lines.push((std::mem::take(&mut cur), tok.t));
        } else {
            cur.push(tok);
        }
    }
    lines.push((cur, String::new()));
    lines
}

fn pass(text: &str, level: Level, idx: &Index) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_blank = false;
    for (line, nl) in split_lines(tokenize(text)) {
        if line.iter().all(|t| t.k == K::Space) {
            if !prev_blank {
                out.push_str(&nl);
            }
            prev_blank = true;
            continue;
        }
        let toks = engine::run_line(line, level, idx);
        if toks.iter().all(|t| t.k == K::Space) {
            continue;
        }
        prev_blank = false;
        for t in &toks {
            out.push_str(&t.t);
        }
        out.push_str(&nl);
    }
    out
}
