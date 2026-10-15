
use std::collections::HashMap;
use std::hash::Hash;

use crate::gate::Gate;
use crate::hash::stable_hash;

const NIL: usize = usize::MAX;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub admitted: u64,
    pub rejected: u64,
    pub evicted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evicted<K, V> {
    pub key: K,
    pub value: V,
    pub weight: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Insert<K, V> {
    Admitted {
        evicted: Vec<Evicted<K, V>>,
    },
    Rejected(K, V),
}

struct Slot<K, V> {
    key: K,
    value: V,
    weight: usize,
    prev: usize,
    next: usize,
}

pub struct HotCache<K, V> {
    map: HashMap<K, usize>,
    slots: Vec<Option<Slot<K, V>>>,
    free: Vec<usize>,
    head: usize,
    tail: usize,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
    gate: Gate,
    stats: CacheStats,
}

impl<K: Hash + Eq + Clone, V> HotCache<K, V> {
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        let sized = max_entries.min(1 << 20);
        Self::with_gate(max_entries, max_bytes, Gate::new(sized))
    }

    pub fn with_gate(max_entries: usize, max_bytes: usize, gate: Gate) -> Self {
        HotCache {
            map: HashMap::new(),
            slots: Vec::new(),
            free: Vec::new(),
            head: NIL,
            tail: NIL,
            max_entries,
            max_bytes,
            bytes: 0,
            gate,
            stats: CacheStats::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    pub fn stats(&self) -> CacheStats {
        self.stats
    }

    pub fn contains(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    fn slot(&self, i: usize) -> Option<&Slot<K, V>> {
        self.slots.get(i).and_then(Option::as_ref)
    }

    fn slot_mut(&mut self, i: usize) -> Option<&mut Slot<K, V>> {
        self.slots.get_mut(i).and_then(Option::as_mut)
    }

    fn unlink(&mut self, i: usize) {
        let (prev, next) = match self.slot(i) {
            Some(s) => (s.prev, s.next),
            None => return,
        };
        match self.slot_mut(prev) {
            Some(p) => p.next = next,
            None => self.head = next,
        }
        match self.slot_mut(next) {
            Some(n) => n.prev = prev,
            None => self.tail = prev,
        }
    }

    fn push_front(&mut self, i: usize) {
        let old = self.head;
        if let Some(s) = self.slot_mut(i) {
            s.prev = NIL;
            s.next = old;
        }
        match self.slot_mut(old) {
            Some(o) => o.prev = i,
            None => self.tail = i,
        }
        self.head = i;
    }

    fn take(&mut self, i: usize) -> Option<Slot<K, V>> {
        self.unlink(i);
        let s = self.slots.get_mut(i)?.take()?;
        self.free.push(i);
        self.map.remove(&s.key);
        self.bytes -= s.weight;
        Some(s)
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.gate.record(key);
        match self.map.get(key).copied() {
            Some(i) => {
                self.stats.hits += 1;
                self.unlink(i);
                self.push_front(i);
                self.slot(i).map(|s| &s.value)
            }
            None => {
                self.stats.misses += 1;
                None
            }
        }
    }

    pub fn peek(&self, key: &K) -> Option<&V> {
        self.map
            .get(key)
            .and_then(|&i| self.slot(i))
            .map(|s| &s.value)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let i = self.map.get(key).copied()?;
        self.take(i).map(|s| s.value)
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.slots.clear();
        self.free.clear();
        self.head = NIL;
        self.tail = NIL;
        self.bytes = 0;
    }

    pub fn insert(&mut self, key: K, value: V, weight: usize) -> Insert<K, V> {
        if weight > self.max_bytes || self.max_entries == 0 {
            self.remove(&key);
            self.stats.rejected += 1;
            return Insert::Rejected(key, value);
        }
        if let Some(i) = self.map.get(&key).copied() {
            let old = self.slot(i).map_or(0, |s| s.weight);
            if let Some(s) = self.slot_mut(i) {
                s.value = value;
                s.weight = weight;
            }
            self.bytes = self.bytes - old + weight;
            self.unlink(i);
            self.push_front(i);
            let evicted = self.evict_overflow(Some(i));
            return Insert::Admitted { evicted };
        }

        let mut need_entries = (self.map.len() + 1).saturating_sub(self.max_entries);
        let mut need_bytes = (self.bytes + weight).saturating_sub(self.max_bytes);
        let cand = stable_hash(&key);
        let mut cursor = self.tail;
        let mut victims = 0usize;
        while need_entries > 0 || need_bytes > 0 {
            let Some(s) = self.slot(cursor) else {
                break;
            };
            if !self.gate.admit_hash(cand, stable_hash(&s.key)) {
                self.stats.rejected += 1;
                return Insert::Rejected(key, value);
            }
            need_entries = need_entries.saturating_sub(1);
            need_bytes = need_bytes.saturating_sub(s.weight);
            cursor = s.prev;
            victims += 1;
        }
        let mut evicted = Vec::with_capacity(victims);
        for _ in 0..victims {
            if let Some(s) = self.take(self.tail) {
                self.stats.evicted += 1;
                evicted.push(Evicted {
                    key: s.key,
                    value: s.value,
                    weight: s.weight,
                });
            }
        }
        let slot = Slot {
            key: key.clone(),
            value,
            weight,
            prev: NIL,
            next: NIL,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.slots[i] = Some(slot);
                i
            }
            None => {
                self.slots.push(Some(slot));
                self.slots.len() - 1
            }
        };
        self.map.insert(key, i);
        self.bytes += weight;
        self.push_front(i);
        self.stats.admitted += 1;
        Insert::Admitted { evicted }
    }

    fn evict_overflow(&mut self, keep: Option<usize>) -> Vec<Evicted<K, V>> {
        let mut out = Vec::new();
        while self.bytes > self.max_bytes || self.map.len() > self.max_entries {
            let t = self.tail;
            if Some(t) == keep || t == NIL {
                break;
            }
            match self.take(t) {
                Some(s) => {
                    self.stats.evicted += 1;
                    out.push(Evicted {
                        key: s.key,
                        value: s.value,
                        weight: s.weight,
                    });
                }
                None => break,
            }
        }
        out
    }

    pub fn keys_mru(&self) -> Vec<K> {
        let mut out = Vec::with_capacity(self.map.len());
        let mut i = self.head;
        while let Some(s) = self.slot(i) {
            out.push(s.key.clone());
            i = s.next;
        }
        out
    }

    pub fn pop_lru(&mut self) -> Option<Evicted<K, V>> {
        let t = self.tail;
        self.take(t).map(|s| Evicted {
            key: s.key,
            value: s.value,
            weight: s.weight,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    fn lru(n: usize) -> HotCache<u64, u64> {
        HotCache::with_gate(n, usize::MAX, Gate::disabled())
    }

    #[test]
    fn lru_order_and_eviction() {
        let mut c = lru(2);
        c.insert(1, 10, 1);
        c.insert(2, 20, 1);
        assert_eq!(c.get(&1), Some(&10));
        let Insert::Admitted { evicted } = c.insert(3, 30, 1) else {
            panic!("plain LRU admits")
        };
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].key, 2);
        assert_eq!(c.keys_mru(), vec![3, 1]);
        assert_eq!(c.stats().evicted, 1);
    }

    #[test]
    fn replace_updates_weight_and_value() {
        let mut c = HotCache::with_gate(10, 100, Gate::disabled());
        c.insert(1u64, "a", 10);
        c.insert(1u64, "b", 30);
        assert_eq!(c.bytes(), 30);
        assert_eq!(c.peek(&1), Some(&"b"));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn oversize_entry_is_rejected_and_returned() {
        let mut c: HotCache<u64, u64> = HotCache::new(10, 50);
        assert_eq!(c.insert(1, 7, 51), Insert::Rejected(1, 7));
        assert!(c.is_empty());
    }

    #[test]
    fn rejection_changes_nothing() {
        let mut c: HotCache<u64, u64> = HotCache::new(2, usize::MAX);
        for _ in 0..5 {
            c.insert(1, 1, 1);
            c.get(&1);
            c.insert(2, 2, 1);
            c.get(&2);
        }
        let before = c.keys_mru();
        assert!(matches!(c.insert(99, 0, 1), Insert::Rejected(99, 0)));
        assert_eq!(c.keys_mru(), before);
        assert_eq!(c.stats().rejected, 1);
    }

    #[test]
    fn never_exceeds_byte_bound_under_random_ops() {
        for gate in [Gate::new(64), Gate::disabled()] {
            let mut c: HotCache<u64, Vec<u8>> = HotCache::with_gate(64, 2048, gate);
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
            for _ in 0..100_000 {
                let k = rng.next() % 300;
                match rng.next() % 4 {
                    0 | 1 => {
                        let w = (rng.next() % 700) as usize;
                        c.insert(k, vec![0u8; w], w);
                    }
                    2 => {
                        c.get(&k);
                    }
                    _ => {
                        c.remove(&k);
                    }
                }
                assert!(c.bytes() <= 2048, "bytes {} over bound", c.bytes());
                assert!(c.len() <= 64);
            }
            let sum: usize = c
                .keys_mru()
                .iter()
                .map(|k| c.peek(k).map_or(0, Vec::len))
                .sum();
            assert_eq!(sum, c.bytes(), "weight bookkeeping drifted");
        }
    }

    fn scan_run(gate: Gate, scan: u64, every: u64) -> (usize, u64, u64) {
        let mut c: HotCache<u64, u64> = HotCache::with_gate(128, usize::MAX, gate);
        for _ in 0..10 {
            for k in 0..100u64 {
                if c.get(&k).is_none() {
                    c.insert(k, k, 1);
                }
            }
        }
        let (mut hot_cursor, mut hits, mut reads) = (0u64, 0u64, 0u64);
        for s in 0..scan {
            let k = 1_000 + s;
            if c.get(&k).is_none() {
                c.insert(k, k, 1);
            }
            if every > 0 && s % every == 0 {
                let h = hot_cursor % 100;
                hot_cursor += 1;
                reads += 1;
                if c.get(&h).is_some() {
                    hits += 1;
                } else {
                    c.insert(h, h, 1);
                }
            }
        }
        ((0..100u64).filter(|k| c.contains(k)).count(), hits, reads)
    }

    #[test]
    fn scan_does_not_evict_hot_set_with_gate_but_does_without() {
        let (_, g_hits, reads) = scan_run(Gate::new(128), 100_000, 2);
        let (_, p_hits, _) = scan_run(Gate::disabled(), 100_000, 2);
        assert!(g_hits * 100 >= reads * 95, "gate on hit {g_hits}/{reads}");
        assert!(
            p_hits * 100 <= reads * 5,
            "gate off still hit {p_hits}/{reads}"
        );
    }

    #[test]
    fn pure_scan_with_a_long_memory_gate_does_not_evict_hot_set() {
        let gate = Gate::with_sketch(crate::Sketch::with_sample(1 << 17, 10_000_000));
        let (gated, _, _) = scan_run(gate, 100_000, 0);
        let (plain, _, _) = scan_run(Gate::disabled(), 100_000, 0);
        assert!(gated >= 95, "gate on kept only {gated}/100 hot keys");
        assert_eq!(plain, 0, "gate off should be flushed completely");
    }

    fn drive(seed: u64) -> (Vec<u64>, CacheStats) {
        let mut c: HotCache<u64, u64> = HotCache::new(24, 4096);
        let mut rng = Rng(seed);
        for _ in 0..50_000 {
            let k = rng.next() % 200;
            match rng.next() % 5 {
                0 | 1 => {
                    c.insert(k, k, (rng.next() % 300) as usize);
                }
                2 | 3 => {
                    c.get(&k);
                }
                _ => {
                    c.remove(&k);
                }
            }
        }
        (c.keys_mru(), c.stats())
    }

    #[test]
    fn identical_operation_sequences_give_identical_cache_state() {
        for seed in [1u64, 0xabcdef, 0x9e37_79b9_7f4a_7c15] {
            let a = drive(seed);
            let b = drive(seed);
            assert_eq!(a, b, "seed {seed:#x}");
            assert!(a.1.admitted > 0 && a.1.rejected > 0 && a.1.evicted > 0);
        }
        assert_ne!(drive(1).0, drive(2).0);
    }
}