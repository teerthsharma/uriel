//! A tiny seeded PRNG (xorshift64*), so the crate needs no `rand` dependency.

/// Deterministic xorshift64* generator. Not cryptographic.
///
/// The same seed always yields the same stream on every platform.
#[derive(Debug, Clone)]
pub struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    /// Creates a generator; any seed (including 0) is valid.
    ///
    /// The seed is scrambled with one splitmix64 round so that nearby seeds give
    /// unrelated streams and the xorshift state is never zero.
    pub fn new(seed: u64) -> Self {
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self {
            state: if z == 0 { 0x9E37_79B9_7F4A_7C15 } else { z },
        }
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform float in `[0, 1)` with 53 bits of precision.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform integer in `[0, n)`. Panics if `n == 0`.
    ///
    /// Uses Lemire's multiply-shift with rejection, so it is unbiased.
    pub fn gen_range(&mut self, n: u64) -> u64 {
        assert!(n > 0, "gen_range requires n > 0");
        let mut m = (self.next_u64() as u128) * (n as u128);
        let mut low = m as u64;
        if low < n {
            let threshold = n.wrapping_neg() % n;
            while low < threshold {
                m = (self.next_u64() as u128) * (n as u128);
                low = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// Fisher-Yates shuffle, O(n).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.gen_range(i as u64 + 1) as usize;
            items.swap(i, j);
        }
    }
}
