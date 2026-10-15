
use std::hash::Hash;

use crate::hash::stable_hash;
use crate::sketch::Sketch;

#[derive(Debug, Clone)]
pub struct Gate {
    sketch: Option<Sketch>,
}

impl Gate {
    pub fn new(capacity: usize) -> Gate {
        Gate {
            sketch: Some(Sketch::new(capacity)),
        }
    }

    pub fn with_sketch(sketch: Sketch) -> Gate {
        Gate {
            sketch: Some(sketch),
        }
    }

    pub fn disabled() -> Gate {
        Gate { sketch: None }
    }

    pub fn is_enabled(&self) -> bool {
        self.sketch.is_some()
    }

    pub fn record<K: Hash + ?Sized>(&mut self, key: &K) {
        self.record_hash(stable_hash(key));
    }

    pub fn record_hash(&mut self, hash: u64) {
        if let Some(s) = &mut self.sketch {
            s.increment(hash);
        }
    }

    pub fn estimate<K: Hash + ?Sized>(&self, key: &K) -> u8 {
        self.estimate_hash(stable_hash(key))
    }

    pub fn estimate_hash(&self, hash: u64) -> u8 {
        self.sketch.as_ref().map_or(0, |s| s.estimate(hash))
    }

    pub fn admit<K: Hash + ?Sized>(&self, candidate: &K, victim: &K) -> bool {
        self.admit_hash(stable_hash(candidate), stable_hash(victim))
    }

    pub fn admit_hash(&self, candidate: u64, victim: u64) -> bool {
        match &self.sketch {
            Some(s) => s.estimate(candidate) > s.estimate(victim),
            None => true,
        }
    }

    pub fn bytes(&self) -> usize {
        self.sketch.as_ref().map_or(0, Sketch::bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_one_hit_key_against_a_hot_victim() {
        let mut g = Gate::new(128);
        for _ in 0..8 {
            g.record(&"hot");
        }
        g.record(&"once");
        assert!(!g.admit(&"once", &"hot"));
        assert!(g.admit(&"hot", &"once"));
    }

    #[test]
    fn ties_are_rejected() {
        let mut g = Gate::new(128);
        g.record(&1u32);
        g.record(&2u32);
        assert!(!g.admit(&1u32, &2u32));
    }

    #[test]
    fn disabled_gate_admits_everything() {
        let g = Gate::disabled();
        assert!(g.admit(&"a", &"b"));
        assert!(!g.is_enabled());
    }
}
