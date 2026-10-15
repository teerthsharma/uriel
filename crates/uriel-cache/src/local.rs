//! Byte-bounded W-TinyLFU cache with epoch + tag invalidation.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::hash::{hash64, mix};

/// Per-entry metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheMeta {
    /// Graph epoch the value was computed at.
    pub epoch: u64,
    /// Labels / relationship types the computation touched.
    pub tags: Vec<String>,
    /// Optional time-to-live.
    pub ttl: Option<Duration>,
}

impl CacheMeta {
    pub fn new(epoch: u64) -> Self {
        CacheMeta {
            epoch,
            tags: Vec::new(),
            ttl: None,
        }
    }

    pub fn with_tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tags = tags.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }
}
/// A cached value together with its metadata (`meta.ttl` is the *remaining* ttl).
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub value: Arc<[u8]>,
    pub meta: CacheMeta,
}

/// Counters snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    /// Entries removed to honour the byte bound (including rejected candidates).
    pub evictions: u64,
    pub expirations: u64,
    pub invalidations: u64,
    /// Inserts refused outright (oversize, or older than a tag epoch floor).
    pub rejected: u64,
    /// Charged bytes currently held.
    pub bytes: usize,
    pub entries: usize,
}

/// Fixed per-entry bookkeeping charge (map slot, list links, `Arc` headers).
const ENTRY_OVERHEAD: usize = 64;
const NIL: usize = usize::MAX;
const SKETCH_ROWS: usize = 4;
const MAX_COUNT: u8 = 15;

/// Count-min sketch of 4-bit-saturating counters with periodic halving.
struct Sketch {
    rows: [Vec<u8>; SKETCH_ROWS],
    mask: usize,
    additions: usize,
    sample: usize,
}

impl Sketch {
    fn new(width: usize) -> Self {
        let width = width.next_power_of_two();
        Sketch {
            rows: std::array::from_fn(|_| vec![0u8; width]),
            mask: width - 1,
            additions: 0,
            sample: width * 10,
        }
    }

    #[inline]
    fn slot(&self, h: u64, row: usize) -> usize {
        (mix(h.wrapping_add((row as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15))) as usize)
            & self.mask
    }

    fn estimate(&self, h: u64) -> u8 {
        (0..SKETCH_ROWS)
            .map(|r| self.rows[r][self.slot(h, r)])
            .min()
            .unwrap_or(0)
    }

    fn increment(&mut self, h: u64) {
        for r in 0..SKETCH_ROWS {
            let s = self.slot(h, r);
            let c = &mut self.rows[r][s];
            if *c < MAX_COUNT {
                *c += 1;
            }
        }
        self.additions += 1;
        if self.additions >= self.sample {
            for row in &mut self.rows {
                for c in row.iter_mut() {
                    *c >>= 1;
                }
            }
            self.additions /= 2;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Seg {
    Window,
    Main,
}

struct Node {
    key: Arc<str>,
    hash: u64,
    value: Arc<[u8]>,
    epoch: u64,
    tags: Vec<String>,
    expires_at: Option<Instant>,
    size: usize,
    prev: usize,
    next: usize,
    seg: Seg,
}

/// Intrusive doubly linked list over slab indices; `head` is most recently used.
struct List {
    head: usize,
    tail: usize,
    bytes: usize,
}

impl List {
    fn new() -> Self {
        List {
            head: NIL,
            tail: NIL,
            bytes: 0,
        }
    }
}

struct Inner {
    max_bytes: usize,
    window_max: usize,
    main_max: usize,
    map: HashMap<Arc<str>, usize>,
    slab: Vec<Option<Node>>,
    free: Vec<usize>,
    window: List,
    main: List,
    tag_index: HashMap<String, HashSet<Arc<str>>>,
    floors: HashMap<String, u64>,
    sketch: Sketch,
    stats: CacheStats,
}

impl Inner {
    fn new(max_bytes: usize) -> Self {
        // Window: 1% of capacity, but at least min(10%, 8 KiB) so that recency has room to
        // work on small caches.
        let window_max = (max_bytes / 100).max((max_bytes / 10).min(8 * 1024));
        let est_entries = max_bytes / 128;
        Inner {
            max_bytes,
            window_max,
            main_max: max_bytes - window_max,
            map: HashMap::new(),
            slab: Vec::new(),
            free: Vec::new(),
            window: List::new(),
            main: List::new(),
            tag_index: HashMap::new(),
            floors: HashMap::new(),
            sketch: Sketch::new(est_entries.clamp(1024, 1 << 20)),
            stats: CacheStats::default(),
        }
    }

    fn node(&self, i: usize) -> &Node {
        self.slab[i].as_ref().expect("live slab index")
    }

    fn node_mut(&mut self, i: usize) -> &mut Node {
        self.slab[i].as_mut().expect("live slab index")
    }

    fn list_mut(&mut self, seg: Seg) -> &mut List {
        match seg {
            Seg::Window => &mut self.window,
            Seg::Main => &mut self.main,
        }
    }

    fn unlink(&mut self, i: usize) {
        let (prev, next, seg, size) = {
            let n = self.node(i);
            (n.prev, n.next, n.seg, n.size)
        };
        if prev != NIL {
            self.node_mut(prev).next = next;
        } else {
            self.list_mut(seg).head = next;
        }
        if next != NIL {
            self.node_mut(next).prev = prev;
        } else {
            self.list_mut(seg).tail = prev;
        }
        self.list_mut(seg).bytes -= size;
        let n = self.node_mut(i);
        n.prev = NIL;
        n.next = NIL;
    }

    fn push_front(&mut self, i: usize, seg: Seg) {
        let old_head = self.list_mut(seg).head;
        let size = {
            let n = self.node_mut(i);
            n.seg = seg;
            n.prev = NIL;
            n.next = old_head;
            n.size
        };
        if old_head != NIL {
            self.node_mut(old_head).prev = i;
        }
        let l = self.list_mut(seg);
        l.head = i;
        if l.tail == NIL {
            l.tail = i;
        }
        l.bytes += size;
    }

    /// Fully remove a node that is already unlinked from its list.
    fn drop_unlinked(&mut self, i: usize) -> Node {
        let node = self.slab[i].take().expect("live slab index");
        self.free.push(i);
        self.map.remove(&node.key);
        for t in &node.tags {
            if let Some(set) = self.tag_index.get_mut(t) {
                set.remove(&node.key);
                if set.is_empty() {
                    self.tag_index.remove(t);
                }
            }
        }
        node
    }

    fn remove_idx(&mut self, i: usize) -> Node {
        self.unlink(i);
        self.drop_unlinked(i)
    }

    fn bytes(&self) -> usize {
        self.window.bytes + self.main.bytes
    }

    fn expired(&self, i: usize, now: Instant) -> bool {
        self.node(i).expires_at.is_some_and(|t| now >= t)
    }

    fn get_entry(&mut self, key: &str, now: Instant) -> Option<CacheEntry> {
        let h = hash64(key.as_bytes(), SKETCH_SEED);
        self.sketch.increment(h);
        let Some(&i) = self.map.get(key) else {
            self.stats.misses += 1;
            return None;
        };
        if self.expired(i, now) {
            self.remove_idx(i);
            self.stats.expirations += 1;
            self.stats.misses += 1;
            return None;
        }
        self.stats.hits += 1;
        let seg = self.node(i).seg;
        self.unlink(i);
        self.push_front(i, seg);
        let n = self.node(i);
        Some(CacheEntry {
            value: Arc::clone(&n.value),
            meta: CacheMeta {
                epoch: n.epoch,
                tags: n.tags.clone(),
                ttl: n.expires_at.map(|t| t.saturating_duration_since(now)),
            },
        })
    }

    fn insert(&mut self, key: &str, value: Arc<[u8]>, meta: CacheMeta, now: Instant) -> bool {
        // A failed overwrite must not leave the previous (now superseded) value behind.
        if let Some(&old) = self.map.get(key) {
            self.remove_idx(old);
        }
        let mut tags = meta.tags;
        tags.sort_unstable();
        tags.dedup();
        let size =
            ENTRY_OVERHEAD + key.len() + value.len() + tags.iter().map(String::len).sum::<usize>();
        if size > self.max_bytes
            || tags
                .iter()
                .any(|t| self.floors.get(t).is_some_and(|f| *f > meta.epoch))
        {
            self.stats.rejected += 1;
            return false;
        }
        let h = hash64(key.as_bytes(), SKETCH_SEED);
        self.sketch.increment(h);

        let key: Arc<str> = Arc::from(key);
        for t in &tags {
            self.tag_index
                .entry(t.clone())
                .or_default()
                .insert(Arc::clone(&key));
        }
        let node = Node {
            key: Arc::clone(&key),
            hash: h,
            value,
            epoch: meta.epoch,
            tags,
            expires_at: meta.ttl.map(|d| now + d),
            size,
            prev: NIL,
            next: NIL,
            seg: Seg::Window,
        };
        let i = match self.free.pop() {
            Some(i) => {
                self.slab[i] = Some(node);
                i
            }
            None => {
                self.slab.push(Some(node));
                self.slab.len() - 1
            }
        };
        self.map.insert(Arc::clone(&key), i);
        self.push_front(i, Seg::Window);
        self.rebalance(now);
        self.map.contains_key(&*key)
    }

    /// Move window overflow into the main segment through the TinyLFU gate.
    fn rebalance(&mut self, now: Instant) {
        while self.window.bytes > self.window_max && self.window.tail != NIL {
            let cand = self.window.tail;
            self.unlink(cand);
            let (csize, cfreq) = {
                let n = self.node(cand);
                (n.size, self.sketch.estimate(n.hash))
            };
            let mut admitted = true;
            while self.main.bytes + csize > self.main_max {
                let victim = self.main.tail;
                if victim == NIL {
                    admitted = false;
                    break;
                }
                if self.expired(victim, now) {
                    self.remove_idx(victim);
                    self.stats.expirations += 1;
                    continue;
                }
                let vfreq = self.sketch.estimate(self.node(victim).hash);
                if cfreq > vfreq {
                    self.remove_idx(victim);
                    self.stats.evictions += 1;
                } else {
                    admitted = false;
                    break;
                }
            }
            if admitted {
                self.push_front(cand, Seg::Main);
            } else {
                self.drop_unlinked(cand);
                self.stats.evictions += 1;
            }
        }
    }

    fn remove_keys(&mut self, keys: Vec<Arc<str>>) -> usize {
        let mut n = 0;
        for k in keys {
            if let Some(&i) = self.map.get(&*k) {
                self.remove_idx(i);
                n += 1;
            }
        }
        self.stats.invalidations += n as u64;
        n
    }
}

const SKETCH_SEED: u64 = 0x5EA1_CAC4_E5ED;

/// Thread-safe byte-bounded W-TinyLFU cache.
///
/// New entries land in a small LRU window; on overflow the window's LRU victim
/// competes with the main segment's LRU victim and the one with the lower
/// count-min frequency estimate is evicted (ties keep the incumbent, which is what
/// makes one-hit scans harmless). All operations are O(1) amortised plus the cost
/// of the entry's tag count.
pub struct LocalCache {
    inner: Mutex<Inner>,
}

impl LocalCache {
    pub fn new(max_bytes: usize) -> Self {
        LocalCache {
            inner: Mutex::new(Inner::new(max_bytes)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn max_bytes(&self) -> usize {
        self.lock().max_bytes
    }

    pub fn len(&self) -> usize {
        self.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, key: &str) -> Option<Arc<[u8]>> {
        self.get_entry(key).map(|e| e.value)
    }

    pub fn get_entry(&self, key: &str) -> Option<CacheEntry> {
        self.get_entry_at(key, Instant::now())
    }

    /// Returns `true` if the entry is resident afterwards.
    pub fn insert(&self, key: &str, value: impl Into<Arc<[u8]>>, meta: CacheMeta) -> bool {
        self.insert_at(key, value, meta, Instant::now())
    }

    pub fn remove(&self, key: &str) -> bool {
        let mut g = self.lock();
        match g.map.get(key).copied() {
            Some(i) => {
                g.remove_idx(i);
                true
            }
            None => false,
        }
    }

    /// Drop every entry carrying `tag`. Returns the number removed.
    pub fn invalidate_tag(&self, tag: &str) -> usize {
        let mut g = self.lock();
        let keys: Vec<Arc<str>> = g
            .tag_index
            .get(tag)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        g.remove_keys(keys)
    }

    /// Drop entries carrying `tag` whose epoch is `< epoch`, and refuse later
    /// inserts for that tag older than `epoch`. Returns the number removed.
    pub fn invalidate_tag_before(&self, tag: &str, epoch: u64) -> usize {
        let mut g = self.lock();
        let floor = g.floors.entry(tag.to_owned()).or_insert(0);
        *floor = (*floor).max(epoch);
        let keys: Vec<Arc<str>> = match g.tag_index.get(tag) {
            Some(set) => set
                .iter()
                .filter(|k| g.map.get(&***k).is_some_and(|&i| g.node(i).epoch < epoch))
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        g.remove_keys(keys)
    }

    /// Would an entry computed at `epoch` touching `tags` be refused as stale?
    pub fn is_stale(&self, epoch: u64, tags: &[String]) -> bool {
        let g = self.lock();
        tags.iter()
            .any(|t| g.floors.get(t).is_some_and(|f| *f > epoch))
    }

    pub fn invalidate_all(&self) {
        let mut g = self.lock();
        let n = g.map.len() as u64;
        g.map.clear();
        g.slab.clear();
        g.free.clear();
        g.window = List::new();
        g.main = List::new();
        g.tag_index.clear();
        g.stats.invalidations += n;
    }

    pub fn stats(&self) -> CacheStats {
        let g = self.lock();
        CacheStats {
            bytes: g.bytes(),
            entries: g.map.len(),
            ..g.stats
        }
    }

    pub(crate) fn get_entry_at(&self, key: &str, now: Instant) -> Option<CacheEntry> {
        self.lock().get_entry(key, now)
    }

    pub(crate) fn insert_at(
        &self,
        key: &str,
        value: impl Into<Arc<[u8]>>,
        meta: CacheMeta,
        now: Instant,
    ) -> bool {
        self.lock().insert(key, value.into(), meta, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Rng;

    fn meta(epoch: u64, tags: &[&str]) -> CacheMeta {
        CacheMeta::new(epoch).with_tags(tags.iter().copied())
    }

    #[test]
    fn insert_get_roundtrip_and_stats() {
        let c = LocalCache::new(10_000);
        assert!(c.get("a").is_none());
        assert!(c.insert("a", vec![1, 2, 3], CacheMeta::new(1)));
        assert_eq!(&*c.get("a").unwrap(), &[1, 2, 3]);
        let s = c.stats();
        assert_eq!((s.hits, s.misses, s.entries), (1, 1, 1));
        assert!(s.bytes >= 3 && s.bytes <= 10_000);
    }

    #[test]
    fn replacing_a_key_updates_bytes_not_entry_count() {
        let c = LocalCache::new(100_000);
        c.insert("k", vec![0u8; 1000], CacheMeta::new(1));
        let big = c.stats().bytes;
        c.insert("k", vec![0u8; 10], CacheMeta::new(2));
        let small = c.stats().bytes;
        assert_eq!(c.len(), 1);
        assert_eq!(big - small, 990);
        assert_eq!(c.get_entry("k").unwrap().meta.epoch, 2);
    }

    #[test]
    fn oversize_value_is_rejected_without_disturbing_cache() {
        let c = LocalCache::new(1000);
        assert!(c.insert("small", vec![0u8; 10], CacheMeta::new(1)));
        assert!(!c.insert("huge", vec![0u8; 5000], CacheMeta::new(1)));
        assert!(c.get("small").is_some());
        assert!(c.get("huge").is_none());
        assert_eq!(c.stats().rejected, 1);
    }

    #[test]
    fn byte_bound_never_exceeded_under_100k_random_inserts() {
        let max = 256 * 1024;
        let c = LocalCache::new(max);
        let mut rng = Rng::new(0xC0FFEE);
        for i in 0..100_000u32 {
            let key = format!("k{}", rng.below(20_000));
            let len = 1 + rng.below(2_000);
            c.insert(&key, vec![(i % 251) as u8; len], meta(1, &["Person"]));
            if i % 7 == 0 {
                c.get(&format!("k{}", rng.below(20_000)));
            }
            let s = c.stats();
            assert!(
                s.bytes <= max,
                "bytes {} > {} at insert {}",
                s.bytes,
                max,
                i
            );
        }
        let s = c.stats();
        assert!(s.entries > 0 && s.evictions > 0);
        assert!(s.bytes <= max);
    }

    #[test]
    fn byte_bound_holds_across_threads() {
        let max = 64 * 1024;
        let c = Arc::new(LocalCache::new(max));
        let handles: Vec<_> = (0..4u64)
            .map(|t| {
                let c = Arc::clone(&c);
                std::thread::spawn(move || {
                    let mut rng = Rng::new(t + 1);
                    for _ in 0..10_000 {
                        let key = format!("k{}", rng.below(5_000));
                        c.insert(&key, vec![0u8; 1 + rng.below(1_000)], CacheMeta::new(1));
                        c.get(&format!("k{}", rng.below(5_000)));
                        assert!(c.stats().bytes <= max);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn tinylfu_keeps_hot_working_set_against_one_hit_scan() {
        // ~90 charged bytes per entry -> roughly 700 entries fit.
        let c = LocalCache::new(64 * 1024);
        let hot: Vec<String> = (0..50).map(|i| format!("hot{i:03}")).collect();
        for k in &hot {
            c.insert(k, vec![7u8; 16], CacheMeta::new(1));
            for _ in 0..8 {
                assert!(c.get(k).is_some());
            }
        }
        // 5000 distinct one-hit keys: far larger than capacity.
        for i in 0..5000 {
            c.insert(&format!("scan{i:05}"), vec![0u8; 16], CacheMeta::new(1));
        }
        let kept = hot.iter().filter(|k| c.get(k).is_some()).count();
        assert!(kept >= 48, "only {kept}/50 hot keys survived the scan");
        assert!(c.stats().evictions >= 4000);
    }

    #[test]
    fn ttl_expiry_is_exact_and_releases_bytes() {
        let c = LocalCache::new(10_000);
        let t0 = Instant::now();
        let ttl = Duration::from_millis(500);
        c.insert_at("t", vec![1u8; 100], CacheMeta::new(1).with_ttl(ttl), t0);
        c.insert_at("forever", vec![1u8; 100], CacheMeta::new(1), t0);
        let before = c.stats().bytes;

        let e = c
            .get_entry_at("t", t0 + Duration::from_millis(499))
            .unwrap();
        assert_eq!(e.meta.ttl, Some(Duration::from_millis(1)));

        assert!(c.get_entry_at("t", t0 + ttl).is_none());
        assert!(c
            .get_entry_at("forever", t0 + Duration::from_secs(86_400))
            .is_some());
        let s = c.stats();
        assert_eq!(s.expirations, 1);
        assert!(s.bytes < before);
        assert_eq!(s.entries, 1);
    }

    #[test]
    fn ttl_expiry_in_real_time() {
        let c = LocalCache::new(10_000);
        c.insert(
            "t",
            vec![1u8; 10],
            CacheMeta::new(1).with_ttl(Duration::from_millis(30)),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.get("t").is_some() {
            assert!(Instant::now() < deadline, "entry never expired");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn tag_invalidation_drops_older_epochs_and_keeps_newer() {
        let c = LocalCache::new(100_000);
        c.insert("old", vec![1], meta(1, &["Person", "KNOWS"]));
        c.insert("edge", vec![1], meta(2, &["Person"]));
        c.insert("new", vec![1], meta(5, &["Person"]));
        c.insert("other", vec![1], meta(1, &["City"]));

        let removed = c.invalidate_tag_before("Person", 3);
        assert_eq!(removed, 2, "epochs 1 and 2 are older than 3");
        assert!(c.get("old").is_none());
        assert!(c.get("edge").is_none());
        assert!(c.get("new").is_some(), "epoch 5 >= 3 must survive");
        assert!(c.get("other").is_some(), "different tag must survive");

        // Entry whose epoch equals the new epoch is current, not stale.
        c.insert("eq", vec![1], meta(3, &["Person"]));
        assert_eq!(c.invalidate_tag_before("Person", 3), 0);
        assert!(c.get("eq").is_some());
    }

    #[test]
    fn epoch_floor_refuses_late_stale_inserts() {
        let c = LocalCache::new(100_000);
        c.invalidate_tag_before("Person", 10);
        assert!(!c.insert("late", vec![1], meta(9, &["Person"])));
        assert!(c.get("late").is_none());
        assert!(c.insert("fresh", vec![1], meta(10, &["Person"])));
        assert!(c.insert("untagged", vec![1], meta(1, &["City"])));
        assert_eq!(c.stats().rejected, 1);
    }

    #[test]
    fn is_stale_reflects_the_epoch_floor() {
        let c = LocalCache::new(1000);
        let tags = vec!["Person".to_string()];
        assert!(!c.is_stale(1, &tags), "no floor yet");
        c.invalidate_tag_before("Person", 10);
        assert!(c.is_stale(9, &tags));
        assert!(!c.is_stale(10, &tags));
        assert!(!c.is_stale(1, &["City".to_string()]));
        assert!(!c.is_stale(1, &[]));
    }

    #[test]
    fn unconditional_tag_invalidation_and_invalidate_all() {
        let c = LocalCache::new(100_000);
        c.insert("a", vec![1], meta(9, &["X"]));
        c.insert("b", vec![1], meta(9, &["X", "Y"]));
        c.insert("c", vec![1], meta(9, &["Y"]));
        assert_eq!(c.invalidate_tag("X"), 2);
        assert!(c.get("c").is_some());
        // 'b' was indexed under Y too; its removal must not leave a dangling index.
        assert_eq!(c.invalidate_tag("Y"), 1);
        assert!(c.is_empty());
        assert_eq!(c.stats().bytes, 0);

        c.insert("d", vec![1], meta(1, &["Z"]));
        c.invalidate_all();
        assert!(c.is_empty());
        assert_eq!(c.stats().bytes, 0);
    }

    #[test]
    fn replaced_key_drops_its_old_tags() {
        let c = LocalCache::new(100_000);
        c.insert("k", vec![1], meta(1, &["Old"]));
        c.insert("k", vec![2], meta(2, &["New"]));
        assert_eq!(c.invalidate_tag("Old"), 0);
        assert!(c.get("k").is_some());
        assert_eq!(c.invalidate_tag("New"), 1);
    }

    #[test]
    fn remove_deletes_entry() {
        let c = LocalCache::new(10_000);
        c.insert("k", vec![1], CacheMeta::new(1));
        assert!(c.remove("k"));
        assert!(!c.remove("k"));
        assert_eq!(c.stats().bytes, 0);
    }
}
