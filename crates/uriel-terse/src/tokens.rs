pub const CHARS_PER_TOKEN: usize = 4;

pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(CHARS_PER_TOKEN)
}

const WORD_CHARS: f64 = 5.5;
const DIGITS_PER_TOKEN: usize = 2;
const NEWLINE: f64 = 0.7;
const PUNCT: f64 = 0.6;

pub fn count_tokens(s: &str) -> usize {
    let mut total = 0.0f64;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c.is_ascii_alphabetic() {
            let mut n = 1usize;
            while it.next_if(char::is_ascii_alphabetic).is_some() {
                n += 1;
            }
            total += (n as f64 / WORD_CHARS).max(1.0);
        } else if c.is_ascii_digit() {
            let mut n = 1usize;
            while it.next_if(char::is_ascii_digit).is_some() {
                n += 1;
            }
            total += n.div_ceil(DIGITS_PER_TOKEN) as f64;
        } else if c.is_whitespace() {
            if c == '\n' {
                total += NEWLINE;
            }
            while let Some(w) = it.next_if(|w| w.is_whitespace()) {
                if w == '\n' {
                    total += NEWLINE;
                }
            }
        } else {
            total += PUNCT * if c.is_ascii() { 1.0 } else { 2.0 };
        }
    }
    total.round() as usize
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    pub before: usize,
    pub after: usize,
    pub saved_ratio: f64,
}

impl Report {
    pub fn from_counts(before: usize, after: usize) -> Self {
        let saved_ratio = if before == 0 {
            0.0
        } else {
            1.0 - after as f64 / before as f64
        };
        Report {
            before,
            after,
            saved_ratio,
        }
    }

    pub fn measure(before: &str, after: &str) -> Self {
        Self::from_counts(estimate_tokens(before), estimate_tokens(after))
    }
}
