use std::collections::{BTreeMap, BTreeSet};

use uriel_tier::stable_hash;
use sha2::{Digest, Sha256};

use crate::config::{Config, Placement};
use crate::event::{AccessKind, Event, Level, Lifetime, PlaceError, Region, RegionId, Tier};
use crate::fixed::{self, Fx, ONE};
use crate::gate::FreqGate;

const SCAN_HEAT: Fx = ONE / 8;
const MAGIC: &[u8; 4] = b"SPLC";
const VERSION: u8 = 1;
const MIN_RECORD: usize = 62;

pub(crate) struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], PlaceError> {
        if n > self.b.len() {
            return Err(PlaceError::BadCheckpoint("truncated"));
        }
        let (h, t) = self.b.split_at(n);
        self.b = t;
        Ok(h)
    }
    pub(crate) fn u8(&mut self) -> Result<u8, PlaceError> {
        Ok(self.take(1)?[0])
    }
    pub(crate) fn u32(&mut self) -> Result<u32, PlaceError> {
        let mut a = [0u8; 4];
        a.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(a))
    }
    pub(crate) fn u64(&mut self) -> Result<u64, PlaceError> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(a))
    }
    fn i64(&mut self) -> Result<i64, PlaceError> {
        Ok(self.u64()? as i64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RegionState {
    size: u64,
    community: u32,
    heat: Fx,
    heat_tick: u64,
    pinned: bool,
    tier: Tier,
    acc_n: u32,
    first: u64,
    last_acc: u64,
    gap: u64,
}

#[derive(Debug, Clone)]
pub struct Placer {
    config: Config,
    fingerprint: [u8; 32],
    regions: BTreeMap<RegionId, RegionState>,
    adj: BTreeMap<RegionId, BTreeSet<RegionId>>,
    clock: u64,
    level: Level,
    gate: FreqGate,
    ignored: u64,
}

fn key(id: RegionId) -> u64 {
    stable_hash(&id.0)
}

fn fingerprint(config: &Config, edges: &[(RegionId, RegionId)]) -> [u8; 32] {
    let mut h = Sha256::new();
    let w = &config.weights;
    for v in [
        config.hot_bytes,
        config.warm_bytes,
        config.prefetch_bytes,
        config.half_life,
        w.heat as u64,
        w.topo as u64,
        w.pin as u64,
        w.size as u64,
        w.life as u64,
        config.size_unit,
        u64::from(config.gate_enabled),
        config.gate_capacity as u64,
        config.compact_threshold as u64,
        config.compact_min_regions as u64,
        config.durable_span,
    ] {
        h.update(v.to_le_bytes());
    }
    let norm: BTreeSet<(u32, u32)> = edges
        .iter()
        .filter(|(a, b)| a != b)
        .map(|&(a, b)| (a.0.min(b.0), a.0.max(b.0)))
        .collect();
    for (a, b) in norm {
        h.update(a.to_le_bytes());
        h.update(b.to_le_bytes());
    }
    h.finalize().into()
}

fn build_adj(
    regions: &BTreeMap<RegionId, RegionState>,
    edges: &[(RegionId, RegionId)],
) -> BTreeMap<RegionId, BTreeSet<RegionId>> {
    let mut adj: BTreeMap<RegionId, BTreeSet<RegionId>> = BTreeMap::new();
    for &(a, b) in edges {
        if a != b && regions.contains_key(&a) && regions.contains_key(&b) {
            adj.entry(a).or_default().insert(b);
            adj.entry(b).or_default().insert(a);
        }
    }
    adj
}

impl Placer {
    pub fn new(config: Config, regions: &[Region], edges: &[(RegionId, RegionId)]) -> Placer {
        let mut map = BTreeMap::new();
        for r in regions {
            map.entry(r.id).or_insert(RegionState {
                size: r.size,
                community: r.community,
                heat: 0,
                heat_tick: 0,
                pinned: false,
                tier: Tier::Cold,
                acc_n: 0,
                first: 0,
                last_acc: 0,
                gap: 0,
            });
        }
        let adj = build_adj(&map, edges);
        let gate = FreqGate::new(config.gate_capacity, config.gate_enabled);
        let fp = fingerprint(&config, edges);
        Placer {
            config,
            fingerprint: fp,
            regions: map,
            adj,
            clock: 0,
            level: Level::Normal,
            gate,
            ignored: 0,
        }
    }

    pub fn add_region(&mut self, r: Region) -> bool {
        if self.regions.contains_key(&r.id) {
            return false;
        }
        self.regions.insert(
            r.id,
            RegionState {
                size: r.size,
                community: r.community,
                heat: 0,
                heat_tick: 0,
                pinned: false,
                tier: Tier::Cold,
                acc_n: 0,
                first: 0,
                last_acc: 0,
                gap: 0,
            },
        );
        true
    }

    pub fn add_edge(&mut self, a: RegionId, b: RegionId) {
        if a != b && self.regions.contains_key(&a) && self.regions.contains_key(&b) {
            self.adj.entry(a).or_default().insert(b);
            self.adj.entry(b).or_default().insert(a);
        }
    }

    pub fn apply(&mut self, ev: Event) {
        let t = self.clock;
        let hl = self.config.half_life;
        match ev {
            Event::Access { region, kind } => match self.regions.get_mut(&region) {
                None => self.ignored += 1,
                Some(s) => {
                    let w = match kind {
                        AccessKind::Read => ONE,
                        AccessKind::Write => 2 * ONE,
                        AccessKind::Scan => SCAN_HEAT,
                    };
                    s.heat =
                        fixed::decay(s.heat, t.saturating_sub(s.heat_tick), hl).saturating_add(w);
                    s.heat_tick = t;
                    if kind != AccessKind::Scan {
                        if s.acc_n == 0 {
                            s.first = t;
                        } else {
                            let gap = t.saturating_sub(s.last_acc);
                            s.gap = if s.acc_n == 1 {
                                gap
                            } else {
                                (3 * s.gap + gap) / 4
                            };
                        }
                        s.acc_n = s.acc_n.saturating_add(1);
                        s.last_acc = t;
                    }
                    self.gate.increment(key(region));
                }
            },
            Event::Pin { region } => match self.regions.get_mut(&region) {
                Some(s) => s.pinned = true,
                None => self.ignored += 1,
            },
            Event::Unpin { region } => match self.regions.get_mut(&region) {
                Some(s) => s.pinned = false,
                None => self.ignored += 1,
            },
            Event::Resize { region, size } => match self.regions.get_mut(&region) {
                Some(s) => s.size = size,
                None => self.ignored += 1,
            },
            Event::Pressure { level } => self.level = level,
        }
        self.clock = self.clock.saturating_add(1);
    }

    pub fn clock(&self) -> u64 {
        self.clock
    }

    pub fn ignored(&self) -> u64 {
        self.ignored
    }

    pub fn level(&self) -> Level {
        self.level
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn heat(&self, r: RegionId) -> Option<Fx> {
        self.regions.get(&r).map(|s| {
            fixed::decay(
                s.heat,
                self.clock.saturating_sub(s.heat_tick),
                self.config.half_life,
            )
        })
    }

    pub fn tier_of(&self, r: RegionId) -> Option<Tier> {
        self.regions.get(&r).map(|s| s.tier)
    }

    fn lifetime_state(&self, s: &RegionState) -> Lifetime {
        if s.acc_n < 2 {
            return Lifetime::Ephemeral;
        }
        let idle = self.clock.saturating_sub(s.last_acc);
        let gap = s.gap.max(1);
        if idle > self.config.half_life && idle > gap.saturating_mul(8) {
            Lifetime::Ephemeral
        } else if s.acc_n >= 4 && s.last_acc.saturating_sub(s.first) >= self.config.durable_span {
            Lifetime::Durable
        } else {
            Lifetime::Session
        }
    }

    pub fn lifetime(&self, r: RegionId) -> Option<Lifetime> {
        self.regions.get(&r).map(|s| self.lifetime_state(s))
    }

    fn scores(&self) -> BTreeMap<RegionId, (Fx, Fx)> {
        let hl = self.config.half_life;
        let w = &self.config.weights;
        let unit = self.config.size_unit.max(1);
        let mut comm: BTreeMap<u32, (i128, i128)> = BTreeMap::new();
        let mut heats: BTreeMap<RegionId, Fx> = BTreeMap::new();
        for (id, s) in &self.regions {
            let h = fixed::decay(s.heat, self.clock.saturating_sub(s.heat_tick), hl);
            heats.insert(*id, h);
            let e = comm.entry(s.community).or_insert((0, 0));
            e.0 += i128::from(h);
            e.1 += 1;
        }
        let mut out = BTreeMap::new();
        for (id, s) in &self.regions {
            let h = heats.get(id).copied().unwrap_or(0);
            let mean =
                comm.get(&s.community)
                    .map_or(0, |&(sum, n)| if n > 0 { fixed::clamp(sum / n) } else { 0 });
            let life = match self.lifetime_state(s) {
                Lifetime::Ephemeral => -ONE,
                Lifetime::Session => 0,
                Lifetime::Durable => ONE,
            };
            let mut sc = fixed::mul(w.heat, h);
            sc = sc.saturating_add(fixed::mul(w.topo, mean));
            if s.pinned {
                sc = sc.saturating_add(w.pin);
            }
            sc = sc.saturating_add(fixed::mul(w.life, life));
            sc = sc.saturating_sub(fixed::mul(w.size, fixed::ratio(s.size, unit)));
            out.insert(*id, (sc, h));
        }
        out
    }

    pub fn score(&self, r: RegionId) -> Option<Fx> {
        self.scores().get(&r).map(|x| x.0)
    }

    fn size(&self, r: RegionId) -> u64 {
        self.regions.get(&r).map_or(0, |s| s.size)
    }

    pub fn plan(&mut self) -> Placement {
        let sc = self.scores();
        let score_of = |id: RegionId| sc.get(&id).map_or(0, |x| x.0);
        let heat_of = |id: RegionId| sc.get(&id).map_or(0, |x| x.1);
        let sort_best = |v: &mut Vec<RegionId>| {
            v.sort_by_cached_key(|id| (std::cmp::Reverse(score_of(*id)), *id));
        };
        let level = self.level;
        let critical = level == Level::Critical;
        let eff =
            u64::try_from(u128::from(self.config.hot_bytes) * u128::from(level.eighths()) / 8)
                .unwrap_or(u64::MAX);

        let mut hot: BTreeSet<RegionId> = BTreeSet::new();
        let mut used: u64 = 0;
        let mut overflow: Vec<RegionId> = Vec::new();

        if !critical {
            let mut pinned: Vec<RegionId> = self
                .regions
                .iter()
                .filter(|(_, s)| s.pinned)
                .map(|(i, _)| *i)
                .collect();
            sort_best(&mut pinned);
            for id in pinned {
                let sz = self.size(id);
                if used.saturating_add(sz) <= eff {
                    hot.insert(id);
                    used += sz;
                } else {
                    overflow.push(id);
                }
            }
        }
        let skip_pinned = |s: &RegionState| s.pinned && !critical;

        let mut inc: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(_, s)| s.tier == Tier::Hot && !skip_pinned(s))
            .map(|(i, _)| *i)
            .collect();
        sort_best(&mut inc);
        let mut incumbents: BTreeSet<RegionId> = BTreeSet::new();
        for id in inc {
            let sz = self.size(id);
            if used.saturating_add(sz) <= eff {
                hot.insert(id);
                incumbents.insert(id);
                used += sz;
            }
        }

        let mut cands: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(i, s)| {
                !hot.contains(i)
                    && !skip_pinned(s)
                    && (heat_of(**i) > 0 || s.pinned)
                    && score_of(**i) > 0
            })
            .map(|(i, _)| *i)
            .collect();
        sort_best(&mut cands);
        let mut victims_asc: Vec<RegionId> = incumbents
            .iter()
            .filter(|v| !self.regions.get(v).is_some_and(|s| s.pinned))
            .copied()
            .collect();
        victims_asc.sort_by_cached_key(|v| (score_of(*v), std::cmp::Reverse(*v)));
        let mut cursor = 0usize;
        for c in cands {
            let sz = self.size(c);
            if sz > eff {
                continue;
            }
            if used.saturating_add(sz) <= eff {
                hot.insert(c);
                used += sz;
                continue;
            }
            let need = used.saturating_add(sz) - eff;
            let ck = key(c);
            let mut freed = 0u64;
            let mut taken = 0usize;
            let mut beaten = true;
            for &v in &victims_asc[cursor..] {
                if !self.gate.admit(ck, key(v)) {
                    beaten = false;
                    break;
                }
                taken += 1;
                freed = freed.saturating_add(self.size(v));
                if freed >= need {
                    break;
                }
            }
            if beaten && freed >= need {
                for &v in &victims_asc[cursor..cursor + taken] {
                    hot.remove(&v);
                    used = used.saturating_sub(self.size(v));
                }
                cursor += taken;
                hot.insert(c);
                used += sz;
            }
        }

        let mut warm: BTreeSet<RegionId> = BTreeSet::new();
        let mut wused = 0u64;
        let mut wc: Vec<RegionId> = self
            .regions
            .iter()
            .filter(|(i, s)| {
                !hot.contains(i) && (heat_of(**i) > 0 || s.pinned) && score_of(**i) > 0
            })
            .map(|(i, _)| *i)
            .collect();
        sort_best(&mut wc);
        for id in wc {
            let sz = self.size(id);
            if wused.saturating_add(sz) <= self.config.warm_bytes {
                warm.insert(id);
                wused += sz;
            }
        }

        for (id, s) in &mut self.regions {
            s.tier = if hot.contains(id) {
                Tier::Hot
            } else if warm.contains(id) {
                Tier::Warm
            } else {
                Tier::Cold
            };
        }

        let mut order: Vec<RegionId> = hot.iter().copied().collect();
        sort_best(&mut order);
        let mut prefetch = Vec::new();
        let mut seen: BTreeSet<RegionId> = BTreeSet::new();
        let mut pf_used = 0u64;
        for h in order {
            if let Some(ns) = self.adj.get(&h) {
                for &n in ns {
                    if hot.contains(&n) || seen.contains(&n) {
                        continue;
                    }
                    let sz = self.size(n);
                    if pf_used.saturating_add(sz) <= self.config.prefetch_bytes {
                        prefetch.push(n);
                        seen.insert(n);
                        pf_used += sz;
                    }
                }
            }
        }

        let mut hist = [[0u64; 64]; 3];
        let mut bytes = [0u64; 3];
        for s in self.regions.values() {
            let t = usize::from(s.tier.tag());
            hist[t][(63 - s.size.max(1).leading_zeros()) as usize] += 1;
            bytes[t] = bytes[t].saturating_add(s.size);
        }
        let mut entropy = [0; 3];
        let mut compact = Vec::new();
        for (t, tier) in [Tier::Hot, Tier::Warm, Tier::Cold].into_iter().enumerate() {
            entropy[t] = fixed::entropy(&hist[t]);
            let n: u64 = hist[t].iter().sum();
            if n >= self.config.compact_min_regions as u64
                && entropy[t] >= self.config.compact_threshold
            {
                compact.push(tier);
            }
        }

        let list = |t: Tier| -> Vec<RegionId> {
            self.regions
                .iter()
                .filter(|(_, s)| s.tier == t)
                .map(|(i, _)| *i)
                .collect()
        };
        Placement {
            clock: self.clock,
            pressure: Some(level),
            hot: list(Tier::Hot),
            warm: list(Tier::Warm),
            cold: list(Tier::Cold),
            prefetch,
            pin_overflow: {
                overflow.sort();
                overflow
            },
            compact,
            entropy,
            hot_bytes: bytes[0],
            warm_bytes: bytes[1],
            hot_budget: eff,
        }
    }

    pub fn snapshot(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(128 + self.regions.len() * MIN_RECORD);
        o.extend_from_slice(MAGIC);
        o.push(VERSION);
        o.extend_from_slice(&self.fingerprint);
        o.extend_from_slice(&self.clock.to_le_bytes());
        o.extend_from_slice(&self.ignored.to_le_bytes());
        o.push(self.level.tag());
        self.gate.encode(&mut o);
        o.extend_from_slice(&(self.regions.len() as u64).to_le_bytes());
        for (id, s) in &self.regions {
            o.extend_from_slice(&id.0.to_le_bytes());
            o.extend_from_slice(&s.size.to_le_bytes());
            o.extend_from_slice(&s.community.to_le_bytes());
            o.extend_from_slice(&s.heat.to_le_bytes());
            o.extend_from_slice(&s.heat_tick.to_le_bytes());
            o.push(u8::from(s.pinned));
            o.push(s.tier.tag());
            o.extend_from_slice(&s.acc_n.to_le_bytes());
            o.extend_from_slice(&s.first.to_le_bytes());
            o.extend_from_slice(&s.last_acc.to_le_bytes());
            o.extend_from_slice(&s.gap.to_le_bytes());
        }
        o
    }

    pub fn restore(
        config: Config,
        edges: &[(RegionId, RegionId)],
        bytes: &[u8],
    ) -> Result<Placer, PlaceError> {
        let mut r = Reader { b: bytes };
        if r.take(4)? != MAGIC {
            return Err(PlaceError::BadCheckpoint("magic"));
        }
        if r.u8()? != VERSION {
            return Err(PlaceError::BadCheckpoint("version"));
        }
        let fp = fingerprint(&config, edges);
        if r.take(32)? != fp {
            return Err(PlaceError::ConfigMismatch);
        }
        let clock = r.u64()?;
        let ignored = r.u64()?;
        let level = Level::from_tag(r.u8()?).ok_or(PlaceError::BadCheckpoint("level"))?;
        let gate = FreqGate::decode(&mut r)?;
        if gate.is_enabled() != config.gate_enabled {
            return Err(PlaceError::ConfigMismatch);
        }
        let n = r.u64()?;
        if n > (r.b.len() / MIN_RECORD) as u64 {
            return Err(PlaceError::BadCheckpoint("region count"));
        }
        let mut regions = BTreeMap::new();
        let mut last: Option<u32> = None;
        for _ in 0..n {
            let id = r.u32()?;
            if last.is_some_and(|l| id <= l) {
                return Err(PlaceError::BadCheckpoint("region ids not ascending"));
            }
            last = Some(id);
            let size = r.u64()?;
            let community = r.u32()?;
            let heat = r.i64()?;
            let heat_tick = r.u64()?;
            let pinned = match r.u8()? {
                0 => false,
                1 => true,
                _ => return Err(PlaceError::BadCheckpoint("pin flag")),
            };
            let tier = Tier::from_tag(r.u8()?).ok_or(PlaceError::BadCheckpoint("tier"))?;
            let acc_n = r.u32()?;
            let first = r.u64()?;
            let last_acc = r.u64()?;
            let gap = r.u64()?;
            if heat < 0 || heat_tick > clock || last_acc > clock {
                return Err(PlaceError::BadCheckpoint("region field out of range"));
            }
            regions.insert(
                RegionId(id),
                RegionState {
                    size,
                    community,
                    heat,
                    heat_tick,
                    pinned,
                    tier,
                    acc_n,
                    first,
                    last_acc,
                    gap,
                },
            );
        }
        if !r.b.is_empty() {
            return Err(PlaceError::BadCheckpoint("trailing bytes"));
        }
        let adj = build_adj(&regions, edges);
        Ok(Placer {
            config,
            fingerprint: fp,
            regions,
            adj,
            clock,
            level,
            gate,
            ignored,
        })
    }
}
