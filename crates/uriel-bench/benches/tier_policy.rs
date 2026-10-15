use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use uriel_bench::zipf;
use uriel_tier::{stable_hash, Gate, Sketch};

const CAPACITY: usize = 4096;
const HASHES: usize = 1 << 16;

fn hashes() -> Vec<u64> {
    zipf::sequence(20_000, 1.0, HASHES, 3)
        .into_iter()
        .map(|k| stable_hash(&(k as u64)))
        .collect()
}

fn sketch_increment(c: &mut Criterion) {
    let hashes = hashes();
    let mut sketch = Sketch::new(CAPACITY);
    let mut k = 0usize;
    c.bench_function("sketch_increment", |b| {
        b.iter(|| {
            k = (k + 1) & (HASHES - 1);
            sketch.increment(black_box(hashes[k]));
        })
    });
}

fn gate_admit(c: &mut Criterion) {
    let hashes = hashes();
    let mut gate = Gate::new(CAPACITY);
    for h in &hashes {
        gate.record_hash(*h);
    }
    let mut k = 0usize;
    c.bench_function("gate_admit", |b| {
        b.iter(|| {
            k = (k + 1) & (HASHES - 1);
            black_box(gate.admit_hash(hashes[k], hashes[(k + 1) & (HASHES - 1)]))
        })
    });
}

criterion_group!(benches, sketch_increment, gate_admit);
criterion_main!(benches);
