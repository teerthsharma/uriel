use uriel_sys::{Arena, Width};

use crate::rng::Rng;

pub const ARENA_BYTES: usize = 16 << 20;

pub fn edges(nodes: usize, degree: usize, seed: u64) -> Vec<(u32, u32)> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(nodes * degree);
    for u in 0..nodes {
        for _ in 0..degree {
            out.push((u as u32, rng.below(nodes) as u32));
        }
    }
    out
}

pub fn arena() -> Arena {
    Arena::with_cap(ARENA_BYTES).expect("arena")
}

pub fn alloc_many(arena: &mut Arena, count: usize, bytes: usize) -> usize {
    let mut total = 0;
    for _ in 0..count {
        total += arena.alloc(bytes, Width::W32).expect("alloc").padded_len();
    }
    arena.reset().expect("reset");
    total
}
