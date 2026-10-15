use uriel_place::{AccessKind, Config, Event, Placer, Region, RegionId};

use crate::zipf;

pub fn regions(n: usize) -> Vec<Region> {
    (0..n)
        .map(|i| Region {
            id: RegionId(i as u32),
            size: 4096 * (1 + (i % 7) as u64),
            community: (i / 64) as u32,
        })
        .collect()
}

pub fn edges(n: usize) -> Vec<(RegionId, RegionId)> {
    let mut out = Vec::with_capacity(2 * n);
    for i in 0..n {
        out.push((RegionId(i as u32), RegionId(((i + 1) % n) as u32)));
        out.push((RegionId(i as u32), RegionId(((i * 7 + 3) % n) as u32)));
    }
    out
}

pub fn events(n: usize, len: usize, seed: u64) -> Vec<Event> {
    zipf::sequence(n, 1.0, len, seed)
        .into_iter()
        .enumerate()
        .map(|(k, r)| Event::Access {
            region: RegionId(r as u32),
            kind: match k % 10 {
                0 => AccessKind::Write,
                1 => AccessKind::Scan,
                _ => AccessKind::Read,
            },
        })
        .collect()
}

pub fn placer(n: usize) -> Placer {
    Placer::new(Config::default(), &regions(n), &edges(n))
}

pub fn warmed_placer(n: usize) -> Placer {
    let mut p = placer(n);
    for ev in events(n, 4 * n, 11) {
        p.apply(ev);
    }
    p
}
