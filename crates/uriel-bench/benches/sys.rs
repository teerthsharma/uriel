use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use uriel_bench::hot::{alloc_many, arena, edges};
use uriel_bench::rng::Rng;
use uriel_sys::HotCsr;

const NODES: usize = 100_000;
const DEGREE: usize = 5;

fn neighbours(c: &mut Criterion) {
    let mut arena = arena();
    let csr = HotCsr::build(&mut arena, NODES, &edges(NODES, DEGREE, 9)).expect("build csr");
    let mut rng = Rng::new(21);
    c.bench_function("hot_csr_neighbours_100k_nodes", |b| {
        b.iter(|| black_box(csr.neighbours(rng.below(NODES) as u32).len()))
    });
}

fn arena_alloc(c: &mut Criterion) {
    let mut arena = arena();
    c.bench_function("arena_alloc_1024_slabs_and_reset", |b| {
        b.iter(|| black_box(alloc_many(&mut arena, 1024, 64)))
    });
}

criterion_group!(benches, neighbours, arena_alloc);
criterion_main!(benches);
