//! Deterministic, dependency-free hashing and a tiny PRNG.
//!
//! The ring must place keys identically on every node and across restarts, so
//! `std`'s randomly seeded hasher cannot be used. `hash64` is FNV-1a followed by
//! a splitmix64 finalizer (FNV alone has weak high bits for short keys).

/// Deterministic 64-bit hash of `data` under `seed`.
pub fn hash64(data: &[u8], seed: u64) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET ^ mix(seed);
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    mix(h ^ data.len() as u64)
}

/// splitmix64 finalizer.
#[inline]
pub(crate) fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// splitmix64 generator: used for shuffles and the count-min row seeds.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0)
    }

    /// Uniform-ish value in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        debug_assert!(n > 0);
        // 128-bit multiply-shift: unbiased enough for scheduling and tests.
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// Fisher-Yates shuffle.
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_seed_sensitive() {
        assert_eq!(hash64(b"label:Person", 7), hash64(b"label:Person", 7));
        assert_ne!(hash64(b"label:Person", 7), hash64(b"label:Person", 8));
        assert_ne!(hash64(b"a", 0), hash64(b"b", 0));
        assert_ne!(hash64(b"", 0), hash64(b"\0", 0));
    }

    #[test]
    fn hash_avalanches_single_bit_flips() {
        let mut total = 0u32;
        let n = 2000u32;
        for i in 0..n {
            let a = i.to_le_bytes();
            let mut b = a;
            b[0] ^= 1;
            total += (hash64(&a, 0) ^ hash64(&b, 0)).count_ones();
        }
        let avg = total as f64 / n as f64;
        assert!((28.0..=36.0).contains(&avg), "avg flipped bits {avg}");
    }

    #[test]
    fn rng_is_deterministic_and_below_is_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut r = Rng::new(1);
        let mut seen = [false; 7];
        for _ in 0..1000 {
            let v = r.below(7);
            assert!(v < 7);
            seen[v] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = Rng::new(9);
        let mut v: Vec<u32> = (0..50).collect();
        r.shuffle(&mut v);
        assert_ne!(v, (0..50).collect::<Vec<_>>());
        v.sort_unstable();
        assert_eq!(v, (0..50).collect::<Vec<_>>());
    }
}
