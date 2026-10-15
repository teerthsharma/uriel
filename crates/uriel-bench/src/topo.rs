use uriel_topo::measure::community_graph;
use uriel_topo::schedule::Blocks;

use crate::rng::Rng;

pub const DIM: usize = 16;
const CLUSTERS: usize = 12;

pub fn cloud(n: usize, dim: usize, seed: u64) -> Vec<f64> {
    let mut rng = Rng::new(seed);
    let centres: Vec<f64> = (0..CLUSTERS * dim).map(|_| 6.0 * rng.normal()).collect();
    let mut out = Vec::with_capacity(n * dim);
    for i in 0..n {
        let c = i % CLUSTERS;
        for d in 0..dim {
            out.push(centres[c * dim + d] + rng.normal());
        }
    }
    out
}

pub fn blocks(n: usize, seed: u64) -> Blocks {
    Blocks::new(
        (0..n as u64).collect(),
        cloud(n, DIM, seed),
        DIM,
        vec![128; n],
    )
    .expect("blocks")
}

pub fn graph(n: usize, seed: u64) -> Vec<(u32, u32)> {
    community_graph(n, 64, seed, false).1
}
