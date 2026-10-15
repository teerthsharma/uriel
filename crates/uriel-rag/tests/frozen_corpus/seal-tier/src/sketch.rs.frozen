
use crate::hash::mix;

const ROWS: usize = 4;
const SEEDS: [u64; ROWS] = [
    0x9e37_79b9_7f4a_7c15,
    0xc2b2_ae3d_27d4_eb4f,
    0x1656_67b1_9e37_79f9,
    0x27d4_eb2f_1656_67c5,
];
pub const MAX_COUNT: u8 = 15;

#[derive(Debug, Clone)]
pub struct Sketch {
    table: Vec<u8>,
    width: usize,
    additions: usize,
    sample_size: usize,
    resets: u64,
}

impl Sketch {
    pub fn new(capacity: usize) -> Sketch {
        let capacity = capacity.max(4);
        Sketch::with_sample(capacity.saturating_mul(4), capacity.saturating_mul(10))
    }

    pub fn with_sample(width: usize, sample_size: usize) -> Sketch {
        let width = width.max(16).next_power_of_two();
        Sketch {
            table: vec![0; ROWS * width / 2],
            width,
            additions: 0,
            sample_size: sample_size.max(1),
            resets: 0,
        }
    }

    fn slot(&self, row: usize, hash: u64) -> (usize, u32) {
        let col = (mix(hash ^ SEEDS[row]) as usize) & (self.width - 1);
        let idx = row * self.width + col;
        (idx >> 1, ((idx & 1) * 4) as u32)
    }

    fn get(&self, byte: usize, shift: u32) -> u8 {
        (self.table[byte] >> shift) & 0x0f
    }

    pub fn estimate(&self, hash: u64) -> u8 {
        (0..ROWS)
            .map(|r| {
                let (b, s) = self.slot(r, hash);
                self.get(b, s)
            })
            .min()
            .unwrap_or(0)
    }

    pub fn increment(&mut self, hash: u64) {
        let slots: [(usize, u32); ROWS] = std::array::from_fn(|r| self.slot(r, hash));
        let min = slots
            .iter()
            .map(|&(b, s)| self.get(b, s))
            .min()
            .unwrap_or(0);
        if min < MAX_COUNT {
            for &(b, s) in &slots {
                if self.get(b, s) == min {
                    self.table[b] += 1 << s;
                }
            }
        }
        self.additions += 1;
        if self.additions >= self.sample_size {
            self.age();
        }
    }

    pub fn age(&mut self) {
        for b in &mut self.table {
            *b = (*b >> 1) & 0x77;
        }
        self.additions /= 2;
        self.resets += 1;
    }

    pub fn resets(&self) -> u64 {
        self.resets
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn bytes(&self) -> usize {
        self.table.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_is_monotone_and_saturates() {
        let mut s = Sketch::with_sample(1024, usize::MAX);
        let mut last = 0;
        for _ in 0..40 {
            s.increment(7);
            let e = s.estimate(7);
            assert!(e >= last, "estimate went down: {last} -> {e}");
            last = e;
        }
        assert_eq!(last, MAX_COUNT);
    }

    #[test]
    fn unseen_key_estimates_zero_in_a_sparse_sketch() {
        let mut s = Sketch::with_sample(4096, usize::MAX);
        s.increment(1);
        assert_eq!(s.estimate(2), 0);
        assert_eq!(s.estimate(1), 1);
    }

    #[test]
    fn counts_are_aged_by_halving() {
        let mut s = Sketch::with_sample(1024, 100);
        for _ in 0..12 {
            s.increment(42);
        }
        assert_eq!(s.estimate(42), 12);
        for k in 1000..1088 {
            s.increment(k);
        }
        assert_eq!(s.resets(), 1);
        assert_eq!(s.estimate(42), 6);
        s.age();
        assert_eq!(s.estimate(42), 3);
    }

    #[test]
    fn hot_key_beats_the_cold_crowd() {
        let mut s = Sketch::new(256);
        for _ in 0..10 {
            s.increment(1);
        }
        for k in 100..600 {
            s.increment(k);
        }
        assert!(s.estimate(1) > s.estimate(100));
    }

    #[test]
    fn saturated_counters_do_not_carry_into_their_neighbour() {
        let mut s = Sketch::with_sample(4096, usize::MAX);
        for _ in 0..3 {
            s.increment(4);
        }
        for _ in 0..40 {
            s.increment(3);
        }
        assert_eq!(s.estimate(3), MAX_COUNT);
        assert_eq!(s.estimate(4), 3);
    }
}
