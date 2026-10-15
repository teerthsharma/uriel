
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone)]
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        StableHasher(0xcbf2_9ce4_8422_2325)
    }
}

pub(crate) fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Hasher for StableHasher {
    fn finish(&self) -> u64 {
        mix(self.0)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

pub fn stable_hash<K: Hash + ?Sized>(key: &K) -> u64 {
    let mut h = StableHasher::default();
    key.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_pinned_across_runs_and_platforms() {
        assert_eq!(stable_hash(&0u64), stable_hash(&0u64));
        assert_eq!(stable_hash(&"seal"), stable_hash(&"seal"));
        assert_ne!(stable_hash(&1u64), stable_hash(&2u64));
        assert_eq!(stable_hash(&0u64), 0x5ba3_14b8_cfda_3b6bu64);
        assert_eq!(stable_hash(&"seal"), 0xe7bf_f2c3_72c8_94fcu64);
    }
}
