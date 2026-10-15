use std::fmt;

use crate::chunk::{is_doc_line, is_signature, Lang};

const SIG_MAX: usize = 64;
const GIST_MAX: usize = 96;
const DOC_MIN: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    L0,
    L1,
    L2,
    L3,
}

impl Level {
    pub fn detail(self) -> u8 {
        match self {
            Level::L0 => 3,
            Level::L1 => 2,
            Level::L2 => 1,
            Level::L3 => 0,
        }
    }

    pub fn lower(self) -> Option<Level> {
        match self {
            Level::L0 => Some(Level::L1),
            Level::L1 => Some(Level::L2),
            Level::L2 => Some(Level::L3),
            Level::L3 => None,
        }
    }

    pub fn higher(self) -> Option<Level> {
        match self {
            Level::L0 => None,
            Level::L1 => Some(Level::L0),
            Level::L2 => Some(Level::L1),
            Level::L3 => Some(Level::L2),
        }
    }

    pub fn from_number(n: u64) -> Option<Level> {
        match n {
            0 => Some(Level::L0),
            1 => Some(Level::L1),
            2 => Some(Level::L2),
            3 => Some(Level::L3),
            _ => None,
        }
    }

    pub fn number(self) -> u8 {
        match self {
            Level::L0 => 0,
            Level::L1 => 1,
            Level::L2 => 2,
            Level::L3 => 3,
        }
    }

    pub fn parse(s: &str) -> Option<Level> {
        let t = s.trim();
        let t = t.strip_prefix(['L', 'l']).unwrap_or(t);
        t.parse::<u64>().ok().and_then(Level::from_number)
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.number())
    }
}

fn squeeze(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cut_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let hard: String = s.chars().take(max).collect();
    match hard.rfind(' ') {
        Some(i) if i > max / 2 => hard[..i].to_string(),
        _ => hard,
    }
}

fn clean_signature(line: &str) -> String {
    let s = squeeze(line);
    let s = s.trim_end_matches(['{', ';', ',']).trim_end();
    s.to_string()
}

fn strip_doc(line: &str) -> String {
    let t = line.trim();
    let t = t
        .trim_start_matches("///")
        .trim_start_matches("//!")
        .trim_start_matches("/**")
        .trim_start_matches("\"\"\"")
        .trim_start_matches("'''");
    let t = t.trim_end_matches("*/").trim_end_matches("\"\"\"");
    let t = t.trim_start().trim_start_matches("* ").trim_start_matches('*');
    squeeze(t)
}

fn first_sentence(s: &str) -> String {
    match s.find(". ") {
        Some(i) => s[..i].to_string(),
        None => s.trim_end_matches('.').to_string(),
    }
}

fn doc_sentence(lines: &[&str], sig: usize, lang: Lang) -> String {
    match lang {
        Lang::Rust | Lang::Script | Lang::Python => {
            for line in lines.iter().filter(|l| is_doc_line(l, lang)) {
                let d = strip_doc(line);
                if d.chars().any(char::is_alphanumeric) {
                    return first_sentence(&d);
                }
            }
            String::new()
        }
        Lang::Markdown | Lang::Toml | Lang::Text | Lang::Json => {
            for line in lines.iter().skip(sig + 1) {
                if line.trim().is_empty() || is_signature(line, lang) {
                    continue;
                }
                let d = squeeze(line);
                if d.chars().any(char::is_alphanumeric) {
                    return first_sentence(&d);
                }
            }
            String::new()
        }
    }
}

pub(crate) fn gist(text: &str, lang: Lang) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(sig) = lines
        .iter()
        .position(|l| is_signature(l, lang))
        .or_else(|| lines.iter().position(|l| !l.trim().is_empty()))
    else {
        return String::new();
    };
    let head = cut_chars(&clean_signature(lines[sig]), SIG_MAX);
    let room = GIST_MAX.saturating_sub(head.chars().count() + 4);
    let doc = doc_sentence(&lines, sig, lang);
    if room >= DOC_MIN && !doc.is_empty() && doc != head {
        format!("{head} -- {}", cut_chars(&doc, room))
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_navigation_and_parse() {
        assert_eq!(Level::L0.lower(), Some(Level::L1));
        assert_eq!(Level::L3.lower(), None);
        assert_eq!(Level::L0.higher(), None);
        assert_eq!(Level::L3.higher(), Some(Level::L2));
        assert!(Level::L1.detail() > Level::L2.detail());
        assert_eq!(Level::parse("L2"), Some(Level::L2));
        assert_eq!(Level::parse("l3"), Some(Level::L3));
        assert_eq!(Level::parse("0"), Some(Level::L0));
        assert_eq!(Level::parse("4"), None);
        assert_eq!(Level::parse("x"), None);
        assert_eq!(Level::L1.to_string(), "L1");
    }

    #[test]
    fn rust_gist_is_signature_plus_first_doc_sentence() {
        let text = "/// Compute a rolling checksum. Second sentence.\n#[inline]\npub fn compute_checksum(data: &[u8]) -> u32 {\n    1\n}";
        assert_eq!(
            gist(text, Lang::Rust),
            "pub fn compute_checksum(data: &[u8]) -> u32 -- Compute a rolling checksum"
        );
    }

    #[test]
    fn gist_without_doc_is_just_the_signature() {
        let text = "pub struct Packet {\n    a: u8,\n}";
        assert_eq!(gist(text, Lang::Rust), "pub struct Packet");
    }

    #[test]
    fn markdown_gist_is_heading_plus_first_sentence() {
        let text = "## Pruning\n\nPrune roses in late winter. Remove dead wood.\n";
        assert_eq!(
            gist(text, Lang::Markdown),
            "## Pruning -- Prune roses in late winter"
        );
    }

    #[test]
    fn plain_text_gist_is_first_line_and_stays_one_line() {
        let g = gist("\n  hello   world\nsecond line", Lang::Text);
        assert!(g.starts_with("hello world"), "{g}");
        assert!(!g.contains('\n'));
        assert_eq!(gist("", Lang::Text), "");
        assert_eq!(gist("\n \n", Lang::Rust), "");
    }

    #[test]
    fn gist_is_short() {
        let long = format!("pub fn {}(x: u8) {{}}", "very_long_name_".repeat(20));
        let g = gist(&format!("/// {}\n{long}", "doc ".repeat(80)), Lang::Rust);
        assert!(g.chars().count() <= SIG_MAX + 4 + GIST_MAX, "{}", g.len());
        assert!(!g.contains('\n'));
    }
}
