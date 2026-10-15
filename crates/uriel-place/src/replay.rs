use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::config::{Config, Placement};
use crate::event::{AccessKind, Event, Level, Region, RegionId};
use crate::place::Placer;

#[derive(Debug, Clone)]
pub struct SplitMix(pub u64);

impl SplitMix {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub config: Config,
    pub regions: Vec<Region>,
    pub edges: Vec<(RegionId, RegionId)>,
    pub events: Vec<Event>,
    pub plan_every: usize,
}

pub fn scenario(seed: u64) -> Scenario {
    let mut rng = SplitMix(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5ea1);
    let n = 24 + rng.below(40) as u32;
    let regions: Vec<Region> = (0..n)
        .map(|i| Region {
            id: RegionId(i),
            size: 1024 << rng.below(5),
            community: i / 8,
        })
        .collect();
    let total: u64 = regions.iter().map(|r| r.size).sum();
    let mut edges: Vec<(RegionId, RegionId)> = (0..n)
        .map(|i| (RegionId(i), RegionId((i + 1) % n)))
        .collect();
    for _ in 0..n {
        edges.push((
            RegionId(rng.below(u64::from(n)) as u32),
            RegionId(rng.below(u64::from(n)) as u32),
        ));
    }
    let config = Config {
        hot_bytes: total / 4,
        warm_bytes: total / 4,
        prefetch_bytes: total / 16,
        half_life: 48 + rng.below(100),
        gate_capacity: 32,
        durable_span: 150,
        ..Config::default()
    };
    let want = 300 + rng.below(200) as usize;
    let pick = |rng: &mut SplitMix| {
        let (a, b) = (rng.below(u64::from(n)), rng.below(u64::from(n)));
        RegionId((a * b / u64::from(n)) as u32)
    };
    let mut events = Vec::with_capacity(want + 8);
    while events.len() < want {
        match rng.below(100) {
            0..=51 => events.push(Event::Access {
                region: pick(&mut rng),
                kind: AccessKind::Read,
            }),
            52..=63 => events.push(Event::Access {
                region: pick(&mut rng),
                kind: AccessKind::Write,
            }),
            64..=77 => {
                let start = rng.below(u64::from(n)) as u32;
                for j in 0..(2 + rng.below(7)) as u32 {
                    events.push(Event::Access {
                        region: RegionId((start + j) % n),
                        kind: AccessKind::Scan,
                    });
                }
            }
            78..=83 => events.push(Event::Pin {
                region: pick(&mut rng),
            }),
            84..=87 => events.push(Event::Unpin {
                region: pick(&mut rng),
            }),
            88..=90 => events.push(Event::Resize {
                region: pick(&mut rng),
                size: 1024 << rng.below(6),
            }),
            _ => {
                let level = match rng.below(6) {
                    0..=2 => Level::Normal,
                    3 => Level::Elevated,
                    4 => Level::High,
                    _ => Level::Critical,
                };
                events.push(Event::Pressure { level });
            }
        }
    }
    events.truncate(want);
    Scenario {
        config,
        regions,
        edges,
        events,
        plan_every: 4 + rng.below(9) as usize,
    }
}

#[derive(Debug, Default)]
pub struct Sequencer {
    next: u64,
    pending: BTreeMap<u64, Vec<Event>>,
}

impl Sequencer {
    pub fn new() -> Sequencer {
        Sequencer::default()
    }
    pub fn push(&mut self, seq: u64, batch: Vec<Event>) -> Vec<Event> {
        if seq >= self.next {
            self.pending.entry(seq).or_insert(batch);
        }
        let mut out = Vec::new();
        while let Some(b) = self.pending.remove(&self.next) {
            out.extend(b);
            self.next += 1;
        }
        out
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    pub fn next_seq(&self) -> u64 {
        self.next
    }
}

#[derive(Debug)]
pub struct Player {
    placer: Placer,
    hasher: Sha256,
    index: usize,
    plan_every: usize,
    placements: Vec<Placement>,
}

impl Player {
    pub fn new(sc: &Scenario) -> Player {
        Player::resume(
            Placer::new(sc.config.clone(), &sc.regions, &sc.edges),
            0,
            sc.plan_every,
        )
    }
    pub fn resume(placer: Placer, index: usize, plan_every: usize) -> Player {
        Player {
            placer,
            hasher: Sha256::new(),
            index,
            plan_every: plan_every.max(1),
            placements: Vec::new(),
        }
    }
    fn decide(&mut self) {
        let p = self.placer.plan();
        self.hasher.update(p.encode());
        self.placements.push(p);
    }
    pub fn feed(&mut self, events: &[Event]) {
        for &e in events {
            self.placer.apply(e);
            self.index += 1;
            if self.index.is_multiple_of(self.plan_every) {
                self.decide();
            }
        }
    }
    pub fn placer(&self) -> &Placer {
        &self.placer
    }
    pub fn finish(mut self) -> ([u8; 32], Vec<Placement>) {
        self.decide();
        (self.hasher.finalize().into(), self.placements)
    }
}
pub fn replay_digest(sc: &Scenario) -> [u8; 32] {
    let mut p = Player::new(sc);
    p.feed(&sc.events);
    p.finish().0
}

pub fn replay_placements(sc: &Scenario) -> Vec<Placement> {
    let mut p = Player::new(sc);
    p.feed(&sc.events);
    p.finish().1
}
