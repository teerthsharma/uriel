
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use uriel_place::{AccessKind, Event, Tier as PlanTier};
use uriel_tier::{
    stable_hash, Budget, Codec, Evicted, HotCache, Insert, KeyClass, Lz4, Shed, Tier,
    TierController,
};

use crate::codec::{decode_node, encode_node};
use crate::error::{Error, Result};
use crate::graph::Options;
use crate::ids::{Edge, Node, NodeId, Props, RelId};
use crate::placement::{PlanCore, Placement};
use crate::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum CacheKey {
    Node(u64),
    Out(u64),
    In(u64),
}

impl CacheKey {
    fn class(self) -> KeyClass {
        match self {
            CacheKey::Node(_) => KeyClass::Node,
            CacheKey::Out(_) | CacheKey::In(_) => KeyClass::Adjacency,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Entry {
    Node(Node),
    Adj(Arc<Vec<Edge>>),
}

pub(crate) const MAX_CACHED_EDGES: usize = 4096;
const OVERHEAD: usize = 64;
const WARM_THRESHOLD: u8 = 2;

fn value_bytes(v: &Value) -> usize {
    match v {
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => 16,
        Value::String(s) => 24 + s.len(),
        Value::Bytes(b) => 24 + b.len(),
        Value::Vector(x) => 24 + 4 * x.len(),
        Value::List(l) => 24 + l.iter().map(value_bytes).sum::<usize>(),
        Value::Map(m) => {
            24 + m
                .iter()
                .map(|(k, v)| 32 + k.len() + value_bytes(v))
                .sum::<usize>()
        }
    }
}

fn props_bytes(p: &Props) -> usize {
    p.iter().map(|(k, v)| 32 + k.len() + value_bytes(v)).sum()
}

fn weight(e: &Entry) -> usize {
    OVERHEAD
        + match e {
            Entry::Node(n) => {
                n.labels.iter().map(|l| 32 + l.len()).sum::<usize>() + props_bytes(&n.props)
            }
            Entry::Adj(a) => a.iter().map(|e| 48 + e.rel_type.len()).sum(),
        }
}

fn put_u32(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes());
}

fn entry_bytes(e: &Entry) -> Vec<u8> {
    match e {
        Entry::Node(n) => encode_node(&n.labels, &n.props),
        Entry::Adj(list) => {
            let mut out = Vec::with_capacity(4 + list.len() * 24);
            put_u32(&mut out, list.len());
            for e in list.iter() {
                out.extend_from_slice(&e.rel.0.to_le_bytes());
                out.extend_from_slice(&e.other.0.to_le_bytes());
                put_u32(&mut out, e.rel_type.len());
                out.extend_from_slice(e.rel_type.as_bytes());
            }
            out
        }
    }
}

fn corrupt(msg: &str) -> Error {
    Error::Corrupt(format!("warm tier: {msg}"))
}

fn take<'a>(buf: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    if n > buf.len() {
        return Err(corrupt("truncated adjacency blob"));
    }
    let (head, tail) = buf.split_at(n);
    *buf = tail;
    Ok(head)
}

fn take_u32(buf: &mut &[u8]) -> Result<usize> {
    let b = take(buf, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
}

fn take_u64(buf: &mut &[u8]) -> Result<u64> {
    let b = take(buf, 8)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}

fn decode_adj(mut buf: &[u8]) -> Result<Vec<Edge>> {
    let n = take_u32(&mut buf)?;
    if n.saturating_mul(20) > buf.len() {
        return Err(corrupt("adjacency count exceeds blob"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let rel = RelId(take_u64(&mut buf)?);
        let other = NodeId(take_u64(&mut buf)?);
        let len = take_u32(&mut buf)?;
        let ty = std::str::from_utf8(take(&mut buf, len)?)
            .map_err(|_| corrupt("adjacency type is not UTF-8"))?;
        out.push(Edge {
            rel,
            rel_type: ty.to_string(),
            other,
        });
    }
    Ok(out)
}

fn compress_entry(e: &Entry) -> Vec<u8> {
    Lz4.compress(&entry_bytes(e))
}

fn decompress_entry(key: CacheKey, blob: &[u8]) -> Result<Entry> {
    let bytes = Lz4
        .decompress(blob)
        .map_err(|e| Error::Corrupt(format!("warm tier: {e}")))?;
    Ok(match key {
        CacheKey::Node(id) => Entry::Node(decode_node(NodeId(id), &bytes)?),
        CacheKey::Out(_) | CacheKey::In(_) => Entry::Adj(Arc::new(decode_adj(&bytes)?)),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierCounters {
    pub hits: u64,
    pub misses: u64,
    pub promotions: u64,
    pub evictions: u64,
    pub entries: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierStats {
    pub hot: TierCounters,
    pub warm: TierCounters,
    pub disk_reads: u64,
    pub bypassed: u64,
    pub gate_rejected: u64,
    pub invalidated: u64,
    pub budget_bytes: usize,
    pub stored_bytes: u64,
    pub logical_bytes: u64,
}

impl TierStats {
    pub fn compression_ratio(&self) -> f64 {
        if self.stored_bytes == 0 {
            1.0
        } else {
            self.logical_bytes as f64 / self.stored_bytes as f64
        }
    }
}

#[derive(Default)]
pub(crate) struct Probe {
    pub(crate) entry: Option<Entry>,
    pub(crate) jobs: Vec<u32>,
    pub(crate) tracked: bool,
}

struct Stashed {
    entry: Entry,
    from_warm: bool,
}

struct Planned {
    core: PlanCore,
    stash: BTreeMap<CacheKey, Stashed>,
    outstanding: usize,
    hot_hits: u64,
    hot_misses: u64,
    warm_hits: u64,
    warm_misses: u64,
    hot_evicted: u64,
    warm_evicted: u64,
}

struct Inner {
    hot: HotCache<CacheKey, Entry>,
    warm: HotCache<CacheKey, Vec<u8>>,
    controller: TierController,
    budget: Budget,
    current: u64,
    inflight: bool,
    warm_promotions: u64,
    bypassed: u64,
    invalidated: u64,
    plan: Option<Planned>,
}

impl Inner {
    fn maybe_advance(&mut self) -> bool {
        let due = self
            .plan
            .as_ref()
            .is_some_and(|p| p.outstanding == 0 && p.core.due());
        if due {
            self.advance();
        }
        due
    }

    fn take_jobs(&mut self) -> Vec<u32> {
        self.plan
            .as_mut()
            .map(|p| p.core.take_jobs())
            .unwrap_or_default()
    }

    fn advance(&mut self) {
        let Some(mut p) = self.plan.take() else {
            return;
        };
        for (rid, prev, next) in p.core.advance() {
            match (prev, next) {
                (PlanTier::Hot | PlanTier::Warm, PlanTier::Cold) => {
                    for k in p.core.keys_of(rid) {
                        p.hot_evicted += u64::from(self.hot.remove(&k).is_some());
                        p.warm_evicted += u64::from(self.warm.remove(&k).is_some());
                    }
                }
                (PlanTier::Hot, PlanTier::Warm) => {
                    for k in p.core.keys_of(rid) {
                        if let Some(v) = self.hot.remove(&k) {
                            p.hot_evicted += 1;
                            let blob = compress_entry(&v);
                            self.insert_warm(k, blob);
                        }
                    }
                }
                _ => {}
            }
        }
        for (key, st) in std::mem::take(&mut p.stash) {
            match p.core.tier_of(key) {
                PlanTier::Hot => {
                    let w = weight(&st.entry);
                    match self.hot.insert(key, st.entry, w) {
                        Insert::Admitted { evicted } => {
                            if st.from_warm {
                                self.warm.remove(&key);
                            }
                            self.demote(evicted);
                        }
                        Insert::Rejected(k, v) => {
                            if !st.from_warm {
                                let blob = compress_entry(&v);
                                self.insert_warm(k, blob);
                            }
                        }
                    }
                }
                PlanTier::Warm => {
                    if !st.from_warm {
                        let blob = compress_entry(&st.entry);
                        self.insert_warm(key, blob);
                    }
                }
                PlanTier::Cold => {}
            }
        }
        self.enforce_budget();
        self.plan = Some(p);
    }

    fn usable(&self, epoch: u64) -> bool {
        !self.inflight && epoch == self.current
    }

    fn insert_warm(&mut self, key: CacheKey, blob: Vec<u8>) {
        let w = blob.len() + OVERHEAD;
        if matches!(self.warm.insert(key, blob, w), Insert::Admitted { .. }) {
            self.warm_promotions += 1;
        }
    }

    fn demote(&mut self, evicted: Vec<Evicted<CacheKey, Entry>>) {
        for e in evicted {
            let blob = compress_entry(&e.value);
            self.insert_warm(e.key, blob);
        }
    }

    fn enforce_budget(&mut self) {
        loop {
            let shed = self.budget.shed(self.hot.bytes(), self.warm.bytes());
            let dropped = match shed {
                Shed::Nothing => break,
                Shed::Warm => self.warm.pop_lru().is_some(),
                Shed::Hot => self.hot.pop_lru().is_some(),
            };
            if !dropped {
                break;
            }
        }
    }

    fn remove_key(&mut self, key: &CacheKey) -> u64 {
        if let Some(p) = self.plan.as_mut() {
            p.stash.remove(key);
        }
        let in_hot = self.hot.remove(key).is_some();
        let in_warm = self.warm.remove(key).is_some();
        u64::from(in_hot) + u64::from(in_warm)
    }
}

pub(crate) struct TierState {
    planned: bool,
    compress_cold: bool,
    cold_threshold: usize,
    disk_reads: AtomicU64,
    inner: Mutex<Inner>,
}

impl TierState {
    pub(crate) fn new(o: &Options, epoch: u64) -> TierState {
        let budget = Budget::new(o.hot_bytes.saturating_add(o.warm_bytes));
        let planned = o.placement == Placement::Planned;
        TierState {
            planned,
            compress_cold: o.compress_cold,
            cold_threshold: o.cold_threshold_bytes,
            disk_reads: AtomicU64::new(0),
            inner: Mutex::new(Inner {
                hot: if o.admission_gate && !planned {
                    HotCache::new(o.hot_entries, o.hot_bytes)
                } else {
                    HotCache::with_gate(o.hot_entries, o.hot_bytes, uriel_tier::Gate::disabled())
                },
                warm: HotCache::with_gate(usize::MAX, o.warm_bytes, uriel_tier::Gate::disabled()),
                controller: TierController::new(
                    o.hot_entries.max(16),
                    WARM_THRESHOLD,
                    (o.hot_entries.max(16) as u64) * 10,
                ),
                budget,
                current: epoch,
                inflight: false,
                warm_promotions: 0,
                bypassed: 0,
                invalidated: 0,
                plan: planned.then(|| Planned {
                    core: PlanCore::new(o),
                    stash: BTreeMap::new(),
                    outstanding: 0,
                    hot_hits: 0,
                    hot_misses: 0,
                    warm_hits: 0,
                    warm_misses: 0,
                    hot_evicted: 0,
                    warm_evicted: 0,
                }),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn store(&self, record: &[u8]) -> Vec<u8> {
        let tier = if self.compress_cold {
            Tier::Cold
        } else {
            Tier::Hot
        };
        uriel_tier::encode_value(record, tier, self.cold_threshold)
    }

    #[cfg(test)]
    pub(crate) fn disk_reads_for_test(&self) -> u64 {
        self.disk_reads.load(Ordering::Relaxed)
    }

    pub(crate) fn note_disk_read(&self) {
        self.disk_reads.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn lookup(&self, epoch: u64, key: CacheKey, kind: AccessKind) -> Probe {
        let mut g = self.lock();
        if !g.usable(epoch) {
            g.bypassed += 1;
            return Probe::default();
        }
        if g.plan.is_some() {
            return Self::lookup_planned(&mut g, key, kind);
        }
        let h = stable_hash(&key);
        g.controller.on_access(key.class(), h);
        if let Some(e) = g.hot.get(&key) {
            return Probe {
                entry: Some(e.clone()),
                ..Probe::default()
            };
        }
        let Some(blob) = g.warm.get(&key).cloned() else {
            return Probe::default();
        };
        let Ok(entry) = decompress_entry(key, &blob) else {
            g.warm.remove(&key);
            return Probe::default();
        };
        let w = weight(&entry);
        if let Insert::Admitted { evicted } = g.hot.insert(key, entry.clone(), w) {
            g.warm.remove(&key);
            g.demote(evicted);
        }
        g.enforce_budget();
        Probe {
            entry: Some(entry),
            ..Probe::default()
        }
    }

    fn lookup_planned(g: &mut Inner, key: CacheKey, kind: AccessKind) -> Probe {
        let Some(mut p) = g.plan.take() else {
            return Probe::default();
        };
        p.core.ingest(key, kind);
        let mut probe = Probe::default();
        if let Some(e) = g.hot.peek(&key) {
            p.hot_hits += 1;
            probe.entry = Some(e.clone());
        } else {
            p.hot_misses += 1;
            let found = g
                .warm
                .peek(&key)
                .map(|blob| decompress_entry(key, blob));
            match found {
                Some(Ok(entry)) => {
                    p.warm_hits += 1;
                    p.stash.entry(key).or_insert(Stashed {
                        entry: entry.clone(),
                        from_warm: true,
                    });
                    probe.entry = Some(entry);
                }
                Some(Err(_)) => {
                    g.warm.remove(&key);
                    p.warm_misses += 1;
                }
                None => p.warm_misses += 1,
            }
        }
        if probe.entry.is_none() {
            p.outstanding += 1;
            probe.tracked = true;
        }
        g.plan = Some(p);
        if probe.entry.is_some() && g.maybe_advance() {
            probe.jobs = g.take_jobs();
        }
        probe
    }

    pub(crate) fn settle(
        &self,
        tracked: bool,
        epoch: u64,
        key: CacheKey,
        entry: Option<Entry>,
    ) -> Vec<u32> {
        if !self.planned {
            if let Some(e) = entry {
                self.fill(epoch, key, e);
            }
            return Vec::new();
        }
        let mut g = self.lock();
        let usable = g.usable(epoch);
        if let Some(p) = g.plan.as_mut() {
            if let (Some(e), true) = (entry, usable) {
                p.stash.entry(key).or_insert(Stashed {
                    entry: e,
                    from_warm: false,
                });
            }
            if tracked {
                p.outstanding = p.outstanding.saturating_sub(1);
            }
        }
        if g.maybe_advance() {
            g.take_jobs()
        } else {
            Vec::new()
        }
    }

    pub(crate) fn fill(&self, epoch: u64, key: CacheKey, entry: Entry) {
        let mut g = self.lock();
        if !g.usable(epoch) {
            return;
        }
        let w = weight(&entry);
        match g.hot.insert(key, entry, w) {
            Insert::Admitted { evicted } => g.demote(evicted),
            Insert::Rejected(k, v) => {
                if g.controller
                    .should_promote_to_warm(k.class(), stable_hash(&k))
                {
                    let blob = compress_entry(&v);
                    g.insert_warm(k, blob);
                }
            }
        }
        g.enforce_budget();
    }

    pub(crate) fn fill_warm(&self, epoch: u64, key: CacheKey, entry: &Entry) {
        let mut g = self.lock();
        if !g.usable(epoch) || g.hot.contains(&key) || g.warm.contains(&key) {
            return;
        }
        let blob = compress_entry(entry);
        g.insert_warm(key, blob);
        g.enforce_budget();
    }

    pub(crate) fn note_scan(&self, id: u64) {
        if !self.planned {
            return;
        }
        let mut g = self.lock();
        if let Some(p) = g.plan.as_mut() {
            p.core.ingest(CacheKey::Node(id), AccessKind::Scan);
        }
        g.maybe_advance();
    }

    pub(crate) fn region_keys(&self, rid: u32) -> Vec<CacheKey> {
        self.lock()
            .plan
            .as_ref()
            .map(|p| p.core.keys_of(rid))
            .unwrap_or_default()
    }

    pub(crate) fn resident(&self, key: &CacheKey) -> bool {
        let g = self.lock();
        g.hot.contains(key) || g.warm.contains(key)
    }

    pub(crate) fn placement_log(&self) -> Vec<Event> {
        self.lock()
            .plan
            .as_ref()
            .map(|p| p.core.log())
            .unwrap_or_default()
    }

    pub(crate) fn hot_regions(&self) -> Vec<u32> {
        self.lock()
            .plan
            .as_ref()
            .map(|p| p.core.hot_regions())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn hot_keys(&self) -> Vec<CacheKey> {
        let g = self.lock();
        let mut k = g.hot.keys_mru();
        k.sort();
        k
    }

    #[cfg(test)]
    pub(crate) fn warm_keys(&self) -> Vec<CacheKey> {
        let g = self.lock();
        let mut k = g.warm.keys_mru();
        k.sort();
        k
    }

    #[cfg(test)]
    pub(crate) fn tier_of(&self, key: CacheKey) -> Option<PlanTier> {
        self.lock().plan.as_ref().map(|p| p.core.tier_of(key))
    }

    #[cfg(test)]
    pub(crate) fn pending_ops(&self) -> usize {
        self.lock().plan.as_ref().map_or(0, |p| p.core.pending_ops())
    }

    pub(crate) fn begin_commit(&self, touched: &BTreeSet<CacheKey>) {
        let mut g = self.lock();
        g.inflight = true;
        let n: u64 = touched.iter().map(|k| g.remove_key(k)).sum();
        g.invalidated += n;
    }

    pub(crate) fn finish_commit(
        &self,
        touched: &BTreeSet<CacheKey>,
        edges: &[(u64, u64)],
        epoch: u64,
    ) {
        let mut g = self.lock();
        let n: u64 = touched.iter().map(|k| g.remove_key(k)).sum();
        g.invalidated += n;
        g.current = epoch;
        g.inflight = false;
        if let Some(p) = g.plan.as_mut() {
            for k in touched {
                p.core.ingest(*k, AccessKind::Write);
            }
            for &(a, b) in edges {
                p.core.ingest_edge(a, b);
            }
        }
        g.maybe_advance();
    }

    pub(crate) fn abort_commit(&self) {
        self.lock().inflight = false;
    }

    pub(crate) fn reset(&self, epoch: u64) {
        let mut g = self.lock();
        g.hot.clear();
        g.warm.clear();
        if let Some(p) = g.plan.as_mut() {
            p.stash.clear();
        }
        g.current = epoch;
        g.inflight = false;
    }

    pub(crate) fn clear(&self) {
        let mut g = self.lock();
        g.hot.clear();
        g.warm.clear();
        if let Some(p) = g.plan.as_mut() {
            p.stash.clear();
        }
    }

    pub(crate) fn counters(&self, stored: u64, logical: u64) -> TierStats {
        let g = self.lock();
        let (h, w) = (g.hot.stats(), g.warm.stats());
        let (he, we) = g
            .plan
            .as_ref()
            .map_or((0, 0), |p| (p.hot_evicted, p.warm_evicted));
        let own = g
            .plan
            .as_ref()
            .map(|p| (p.hot_hits, p.hot_misses, p.warm_hits, p.warm_misses));
        let (hh, hm, wh, wm) = own.unwrap_or((h.hits, h.misses, w.hits, w.misses));
        TierStats {
            hot: TierCounters {
                hits: hh,
                misses: hm,
                promotions: h.admitted,
                evictions: h.evicted + he,
                entries: g.hot.len(),
                bytes: g.hot.bytes(),
            },
            warm: TierCounters {
                hits: wh,
                misses: wm,
                promotions: g.warm_promotions,
                evictions: w.evicted + we,
                entries: g.warm.len(),
                bytes: g.warm.bytes(),
            },
            disk_reads: self.disk_reads.load(Ordering::Relaxed),
            bypassed: g.bypassed,
            gate_rejected: h.rejected,
            invalidated: g.invalidated,
            budget_bytes: g.budget.total(),
            stored_bytes: stored,
            logical_bytes: logical,
        }
    }
}