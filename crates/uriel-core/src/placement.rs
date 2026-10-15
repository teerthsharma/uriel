use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use uriel_place::fixed::ONE;
use uriel_place::{AccessKind, Config, Event, Placer, Region, RegionId, Tier, Weights};

use crate::graph::Options;
use crate::tier::CacheKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    #[default]
    Local,
    Planned,
}

const MAX_REGION_IDS: usize = 64;
const WARM_SLOTS_PER_HOT_SLOT: u64 = 4;
const CLASSES: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Pend {
    rid: u32,
    kind: u8,
    bit: u8,
}

fn kind_rank(k: AccessKind) -> u8 {
    match k {
        AccessKind::Read => 0,
        AccessKind::Write => 1,
        AccessKind::Scan => 2,
    }
}

fn rank_kind(r: u8) -> AccessKind {
    match r {
        0 => AccessKind::Read,
        1 => AccessKind::Write,
        _ => AccessKind::Scan,
    }
}

pub(crate) struct PlanCore {
    placer: Placer,
    ids_per_region: u64,
    interval: usize,
    slot: u64,
    communities: Option<Arc<Vec<u32>>>,
    record: bool,
    pending: Vec<Pend>,
    new_edges: BTreeSet<(u32, u32)>,
    masks: BTreeMap<u32, u64>,
    sizes: BTreeMap<u32, u64>,
    tiers: BTreeMap<u32, Tier>,
    log: Vec<Event>,
    queue: Vec<u32>,
}

impl PlanCore {
    pub(crate) fn new(o: &Options) -> PlanCore {
        let interval = o.plan_every.max(1);
        let ids_per_region = o.region_ids.clamp(1, MAX_REGION_IDS) as u64;
        let slot = ((o.hot_bytes / o.hot_entries.max(1)).max(1)) as u64;
        let cap = if o.hot_entries == 0 {
            0
        } else {
            slot.saturating_mul(o.hot_entries as u64)
                .min(o.hot_bytes as u64)
        };
        let warm_cap = if o.warm_bytes == 0 {
            0
        } else {
            cap.max(slot).saturating_mul(WARM_SLOTS_PER_HOT_SLOT)
        };
        let config = Config {
            hot_bytes: cap,
            warm_bytes: warm_cap,
            prefetch_bytes: slot.saturating_mul(ids_per_region).saturating_mul(2),
            half_life: ((4 * o.hot_entries).max(4 * interval).max(16)) as u64,
            weights: Weights {
                heat: ONE,
                topo: if o.communities.is_some() { ONE / 4 } else { 0 },
                pin: 0,
                size: ONE / 64,
                life: 0,
            },
            size_unit: cap.max(1),
            gate_enabled: o.admission_gate,
            gate_capacity: o.hot_entries.max(16),
            ..Config::default()
        };
        PlanCore {
            placer: Placer::new(config, &[], &[]),
            ids_per_region,
            interval,
            slot,
            communities: o.communities.clone(),
            record: o.record_events,
            pending: Vec::new(),
            new_edges: BTreeSet::new(),
            masks: BTreeMap::new(),
            sizes: BTreeMap::new(),
            tiers: BTreeMap::new(),
            log: Vec::new(),
            queue: Vec::new(),
        }
    }

    fn locate(&self, key: CacheKey) -> (u32, u8) {
        let (class, id) = match key {
            CacheKey::Node(i) => (0, i),
            CacheKey::Out(i) => (1, i),
            CacheKey::In(i) => (2, i),
        };
        let block = (id / self.ids_per_region).min(u64::from(u32::MAX / CLASSES - 1)) as u32;
        (block * CLASSES + class, (id % self.ids_per_region) as u8)
    }

    pub(crate) fn keys_of(&self, rid: u32) -> Vec<CacheKey> {
        let (class, block) = (rid % CLASSES, u64::from(rid / CLASSES));
        (block * self.ids_per_region..(block + 1) * self.ids_per_region)
            .map(|i| match class {
                0 => CacheKey::Node(i),
                1 => CacheKey::Out(i),
                _ => CacheKey::In(i),
            })
            .collect()
    }

    pub(crate) fn ingest(&mut self, key: CacheKey, kind: AccessKind) {
        let (rid, bit) = self.locate(key);
        self.pending.push(Pend {
            rid,
            kind: kind_rank(kind),
            bit,
        });
    }

    pub(crate) fn ingest_edge(&mut self, a: u64, b: u64) {
        let (ra, rb) = (
            self.locate(CacheKey::Node(a)).0,
            self.locate(CacheKey::Node(b)).0,
        );
        if ra != rb {
            self.new_edges.insert((ra.min(rb), ra.max(rb)));
        }
    }

    pub(crate) fn due(&self) -> bool {
        self.pending.len() >= self.interval
    }

    #[cfg(test)]
    pub(crate) fn pending_ops(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn tier_of(&self, key: CacheKey) -> Tier {
        let (rid, _) = self.locate(key);
        self.tiers.get(&rid).copied().unwrap_or(Tier::Cold)
    }

    pub(crate) fn hot_regions(&self) -> Vec<u32> {
        self.tiers
            .iter()
            .filter(|(_, t)| **t == Tier::Hot)
            .map(|(r, _)| *r)
            .collect()
    }

    pub(crate) fn log(&self) -> Vec<Event> {
        self.log.clone()
    }

    pub(crate) fn take_jobs(&mut self) -> Vec<u32> {
        let mut q = std::mem::take(&mut self.queue);
        q.sort_unstable();
        q.dedup();
        q
    }

    fn community(&self, rid: u32) -> u32 {
        self.communities
            .as_ref()
            .and_then(|c| c.get((rid / CLASSES) as usize))
            .copied()
            .unwrap_or(0)
    }

    fn emit(&mut self, ev: Event) {
        self.placer.apply(ev);
        if self.record {
            self.log.push(ev);
        }
    }

    pub(crate) fn advance(&mut self) -> Vec<(u32, Tier, Tier)> {
        let mut pend = std::mem::take(&mut self.pending);
        pend.sort_unstable();
        let edges = std::mem::take(&mut self.new_edges);
        let mut bits: BTreeMap<u32, u64> = BTreeMap::new();
        for p in &pend {
            *bits.entry(p.rid).or_default() |= 1u64 << p.bit;
        }
        let mut touched: BTreeSet<u32> = bits.keys().copied().collect();
        for &(a, b) in &edges {
            touched.insert(a);
            touched.insert(b);
        }
        let full = self.ids_per_region.saturating_mul(self.slot);
        for rid in touched {
            let mask = self.masks.entry(rid).or_insert(0);
            *mask |= bits.get(&rid).copied().unwrap_or(0);
            let size = if *mask == 0 {
                full
            } else {
                u64::from(mask.count_ones()) * self.slot
            };
            match self.sizes.insert(rid, size) {
                None => {
                    let community = self.community(rid);
                    self.placer.add_region(Region {
                        id: RegionId(rid),
                        size,
                        community,
                    });
                }
                Some(old) if old != size => self.emit(Event::Resize {
                    region: RegionId(rid),
                    size,
                }),
                Some(_) => {}
            }
        }
        for &(a, b) in &edges {
            self.placer.add_edge(RegionId(a), RegionId(b));
        }
        for p in pend {
            self.emit(Event::Access {
                region: RegionId(p.rid),
                kind: rank_kind(p.kind),
            });
        }
        let plan = self.placer.plan();
        let mut next: BTreeMap<u32, Tier> = BTreeMap::new();
        for r in &plan.warm {
            next.insert(r.0, Tier::Warm);
        }
        for r in &plan.hot {
            next.insert(r.0, Tier::Hot);
        }
        self.queue.extend(plan.prefetch.iter().map(|r| r.0));
        let mut changes = Vec::new();
        for (rid, old) in &self.tiers {
            if !next.contains_key(rid) {
                changes.push((*rid, *old, Tier::Cold));
            }
        }
        for (rid, new) in &next {
            let old = self.tiers.get(rid).copied().unwrap_or(Tier::Cold);
            if old != *new {
                changes.push((*rid, old, *new));
            }
        }
        changes.sort_unstable_by_key(|c| c.0);
        self.tiers = next;
        changes
    }
}
