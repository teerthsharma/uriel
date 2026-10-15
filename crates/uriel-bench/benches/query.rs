use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use uriel_bench::query::{engine, run, POINT_LOOKUP, TWO_HOP};

const NODES: usize = 5_000;

fn point_lookup(c: &mut Criterion) {
    let engine = engine(NODES);
    let mut k = 0usize;
    c.bench_function("query_point_lookup", |b| {
        b.iter(|| {
            k = (k + 37) % NODES;
            black_box(run(&engine, POINT_LOOKUP, k))
        })
    });
}

fn two_hop(c: &mut Criterion) {
    let engine = engine(NODES);
    let mut k = 0usize;
    c.bench_function("query_two_hop_traversal", |b| {
        b.iter(|| {
            k = (k + 37) % NODES;
            black_box(run(&engine, TWO_HOP, k))
        })
    });
}

criterion_group!(benches, point_lookup, two_hop);
criterion_main!(benches);
