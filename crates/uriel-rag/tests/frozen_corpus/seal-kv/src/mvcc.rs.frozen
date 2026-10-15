
use crate::hlc::Timestamp;
use std::collections::BTreeMap;
use std::ops::RangeBounds;

pub type Key = Vec<u8>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MvccStore {
    data: BTreeMap<Key, BTreeMap<Timestamp, Option<Vec<u8>>>>,
}

impl MvccStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&mut self, key: &[u8], ts: Timestamp, value: Vec<u8>) {
        self.data.entry(key.to_vec()).or_default().insert(ts, Some(value));
    }

    pub fn delete(&mut self, key: &[u8], ts: Timestamp) {
        self.data.entry(key.to_vec()).or_default().insert(ts, None);
    }

    pub fn get(&self, key: &[u8], read_ts: Timestamp) -> Option<&[u8]> {
        let versions = self.data.get(key)?;
        versions.range(..=read_ts).next_back().and_then(|(_, v)| v.as_deref())
    }

    pub fn scan<R: RangeBounds<Key>>(&self, range: R, read_ts: Timestamp) -> Vec<(Key, Vec<u8>)> {
        self.data
            .range(range)
            .filter_map(|(k, versions)| {
                let (_, v) = versions.range(..=read_ts).next_back()?;
                v.as_ref().map(|v| (k.clone(), v.clone()))
            })
            .collect()
    }

    pub fn version_count(&self) -> usize {
        self.data.values().map(BTreeMap::len).sum()
    }

    pub fn key_count(&self) -> usize {
        self.data.len()
    }

    pub fn gc(&mut self, horizon: Timestamp) -> usize {
        let mut removed = 0;
        self.data.retain(|_, versions| {
            let keep_from = versions.range(..=horizon).next_back().map(|(ts, _)| *ts);
            if let Some(keep) = keep_from {
                let before = versions.len();
                *versions = versions.split_off(&keep);
                removed += before - versions.len();
                if versions.len() == 1 && matches!(versions.values().next(), Some(None)) {
                    removed += 1;
                    return false;
                }
            }
            true
        });
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    fn ts(w: u64) -> Timestamp {
        Timestamp::new(w, 0, 1)
    }

    #[test]
    fn get_returns_newest_at_or_before_read_ts() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(10), b"a".to_vec());
        s.put(b"k", ts(20), b"b".to_vec());
        assert_eq!(s.get(b"k", ts(5)), None);
        assert_eq!(s.get(b"k", ts(10)), Some(&b"a"[..]));
        assert_eq!(s.get(b"k", ts(19)), Some(&b"a"[..]));
        assert_eq!(s.get(b"k", ts(20)), Some(&b"b"[..]));
        assert_eq!(s.get(b"k", ts(99)), Some(&b"b"[..]));
        assert_eq!(s.get(b"missing", ts(99)), None);
    }

    #[test]
    fn tombstone_hides_key_but_not_older_snapshots() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(10), b"a".to_vec());
        s.delete(b"k", ts(20));
        assert_eq!(s.get(b"k", ts(15)), Some(&b"a"[..]));
        assert_eq!(s.get(b"k", ts(20)), None);
        s.put(b"k", ts(30), b"c".to_vec());
        assert_eq!(s.get(b"k", ts(25)), None);
        assert_eq!(s.get(b"k", ts(30)), Some(&b"c"[..]));
    }

    #[test]
    fn scan_is_ordered_ranged_and_snapshot_consistent() {
        let mut s = MvccStore::new();
        for (i, k) in ["a", "b", "c", "d"].iter().enumerate() {
            s.put(k.as_bytes(), ts(10 + i as u64), k.as_bytes().to_vec());
        }
        s.delete(b"b", ts(50));
        let r = s.scan(b"a".to_vec()..b"d".to_vec(), ts(100));
        assert_eq!(r.iter().map(|(k, _)| k.as_slice()).collect::<Vec<_>>(), vec![&b"a"[..], &b"c"[..]]);
        let r = s.scan(.., ts(11));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn snapshot_reads_repeatable_while_writes_continue() {
        let mut s = MvccStore::new();
        for i in 0..20u64 {
            s.put(format!("k{i}").as_bytes(), ts(i + 1), vec![i as u8]);
        }
        let snap = ts(20);
        let first = s.scan(.., snap);
        for i in 0..200u64 {
            s.put(format!("k{}", i % 25).as_bytes(), ts(21 + i), vec![99]);
            if i.is_multiple_of(7) {
                s.delete(format!("k{}", i % 20).as_bytes(), ts(21 + i));
            }
            assert_eq!(s.scan(.., snap), first);
        }
    }

    #[test]
    fn gc_keeps_newest_visible_at_horizon() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(10), b"a".to_vec());
        s.put(b"k", ts(20), b"b".to_vec());
        s.put(b"k", ts(30), b"c".to_vec());
        let removed = s.gc(ts(25));
        assert_eq!(removed, 1);
        assert_eq!(s.get(b"k", ts(25)), Some(&b"b"[..]));
        assert_eq!(s.get(b"k", ts(30)), Some(&b"c"[..]));
        assert_eq!(s.version_count(), 2);
    }

    #[test]
    fn gc_never_changes_reads_at_or_after_horizon() {
        let mut s = MvccStore::new();
        let mut seed = 7u64;
        for i in 0..400u64 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let k = format!("k{}", (seed >> 40) % 12);
            if (seed >> 20).is_multiple_of(5) {
                s.delete(k.as_bytes(), ts(i + 1));
            } else {
                s.put(k.as_bytes(), ts(i + 1), vec![(seed >> 8) as u8]);
            }
        }
        let horizon = ts(250);
        let before: Vec<_> = (250..=400).map(|t| s.scan(.., ts(t))).collect();
        let n0 = s.version_count();
        s.gc(horizon);
        assert!(s.version_count() < n0);
        let after: Vec<_> = (250..=400).map(|t| s.scan(.., ts(t))).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn gc_collapses_dead_keys() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(1), b"a".to_vec());
        s.delete(b"k", ts(2));
        assert_eq!(s.gc(ts(10)), 2);
        assert_eq!(s.key_count(), 0);
    }

    type Model = std::collections::BTreeMap<Key, Vec<(Timestamp, Option<Vec<u8>>)>>;
    type Snap = (Timestamp, Vec<(Key, Vec<u8>)>);

    fn model_get(m: &Model, k: &[u8], at: Timestamp) -> Option<Vec<u8>> {
        m.get(k)?.iter().filter(|(t, _)| *t <= at).max_by_key(|(t, _)| *t).and_then(|(_, v)| v.clone())
    }

    #[test]
    fn gc_keeps_exactly_the_newest_version_at_or_before_horizon_over_200_seeds() {
        for seed in 0..200u64 {
            let mut r = Rng::new(seed);
            let mut s = MvccStore::new();
            let mut model: Model = Default::default();
            let mut stamps: Vec<u64> = (1..=60).collect();
            for i in (1..stamps.len()).rev() {
                let j = r.below(i as u64 + 1) as usize;
                stamps.swap(i, j);
            }
            for w in stamps {
                let k = format!("k{}", r.below(6)).into_bytes();
                let t = ts(w);
                let v = if r.chance(1, 4) { None } else { Some(vec![r.below(256) as u8]) };
                match &v {
                    Some(b) => s.put(&k, t, b.clone()),
                    None => s.delete(&k, t),
                }
                model.entry(k).or_default().push((t, v));
            }
            let horizon = ts(r.between(1, 60));
            let mut expected = 0usize;
            for versions in model.values() {
                let keep = versions.iter().map(|(t, _)| *t).filter(|t| *t <= horizon).max();
                let survivors: Vec<_> = versions.iter().filter(|(t, _)| keep.is_none_or(|k| *t >= k)).collect();
                if keep.is_some() && survivors.len() == 1 && survivors[0].1.is_none() {
                    continue;
                }
                expected += survivors.len();
            }
            let before = s.version_count();
            let removed = s.gc(horizon);
            assert_eq!(s.version_count(), expected, "seed {seed}");
            assert_eq!(removed, before - expected, "seed {seed}");
            for k in model.keys() {
                for at in horizon.wall..=61 {
                    assert_eq!(s.get(k, ts(at)).map(<[u8]>::to_vec), model_get(&model, k, ts(at)), "seed {seed} key {k:?} at {at}");
                }
            }
            assert_eq!(s.gc(horizon), 0, "gc is idempotent at seed {seed}");
        }
    }

    #[test]
    fn gc_at_a_version_timestamp_keeps_that_version() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(10), b"a".to_vec());
        s.put(b"k", ts(20), b"b".to_vec());
        s.gc(ts(20));
        assert_eq!(s.get(b"k", ts(20)), Some(&b"b"[..]));
        assert_eq!(s.version_count(), 1);
    }

    #[test]
    fn gc_below_every_version_removes_nothing() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(10), b"a".to_vec());
        s.delete(b"j", ts(11));
        assert_eq!(s.gc(ts(5)), 0);
        assert_eq!(s.version_count(), 2);
    }

    #[test]
    fn snapshots_stay_repeatable_under_three_interleaved_writers_and_gc_below_the_oldest() {
        for seed in 0..40u64 {
            let mut r = Rng::new(seed);
            let mut s = MvccStore::new();
            let mut counter = 0u64;
            let mut snaps: Vec<Snap> = Vec::new();
            for step in 0..300 {
                counter += 1;
                let node = 1 + r.below(3) as u32;
                let t = Timestamp::new(counter, 0, node);
                let k = format!("k{}", r.below(10)).into_bytes();
                if r.chance(1, 5) {
                    s.delete(&k, t);
                } else {
                    s.put(&k, t, vec![r.below(256) as u8, step as u8]);
                }
                if r.chance(1, 15) {
                    let snap = Timestamp::new(counter, u32::MAX, u32::MAX);
                    snaps.push((snap, s.scan(.., snap)));
                }
                if r.chance(1, 40) {
                    if let Some((oldest, _)) = snaps.first() {
                        s.gc(*oldest);
                    }
                }
                for (snap, first) in &snaps {
                    assert_eq!(&s.scan(.., *snap), first, "seed {seed} step {step}");
                }
            }
            assert!(!snaps.is_empty());
        }
    }

    #[test]
    fn a_write_at_a_later_timestamp_never_changes_an_earlier_read() {
        let mut s = MvccStore::new();
        s.put(b"k", ts(10), b"a".to_vec());
        let snap = Timestamp::new(10, u32::MAX, u32::MAX);
        let first = s.get(b"k", snap).map(<[u8]>::to_vec);
        s.put(b"k", ts(11), b"b".to_vec());
        s.delete(b"k", ts(12));
        assert_eq!(s.get(b"k", snap).map(<[u8]>::to_vec), first);
    }
}