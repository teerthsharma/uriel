use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use uriel_bench::placement::{events, placer, warmed_placer};

const SIZES: [usize; 2] = [1_000, 20_000];
const EVENTS: usize = 1 << 16;

fn apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("place_apply");
    for n in SIZES {
        let evs = events(n, EVENTS, 5);
        let mut p = placer(n);
        let mut k = 0usize;
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| {
                k = (k + 1) & (EVENTS - 1);
                p.apply(black_box(evs[k]));
            })
        });
    }
    group.finish();
}

fn plan(c: &mut Criterion) {
    let mut group = c.benchmark_group("place_plan");
    group.sample_size(20);
    for n in SIZES {
        let mut p = warmed_placer(n);
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| black_box(p.plan()))
        });
    }
    group.finish();
}

criterion_group!(benches, apply, plan);
criterion_main!(benches);
