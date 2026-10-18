pub const CHARS_PER_TOKEN: usize = 4;

pub fn estimate_tokens(s: &str) -> usize {
    s.chars().count().div_ceil(CHARS_PER_TOKEN)
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
