use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use uriel_bench::store::{
    hit_rate, open_store, read_sequence, tier_options, warm, zipf_options, Tier,
};
use uriel_bench::zipf;
use uriel_core::Placement;

const NODES: usize = 64;
const ZIPF_NODES: usize = 2000;
const ZIPF_READS: usize = 4096;

fn tier_read_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("tier_read_latency");
    for (name, tier) in [("t0_hot", Tier::T0), ("t1_warm", Tier::T1), ("t2_disk", Tier::T2)] {
        let store = open_store(&tier_options(tier), NODES);
        warm(&store, 2);
        let tx = store.reader();
        let mut k = 0usize;
        group.bench_function(name, |b| {
            b.iter(|| {
                k = (k + 1) % NODES;
                black_box(tx.get_node(store.ids[k]).expect("read"))
            })
        });
    }
    group.finish();
}

fn zipf_hit_rate(c: &mut Criterion) {
    let mut group = c.benchmark_group("zipf_hit_rate");
    group.sample_size(20);
    let order = zipf::sequence(ZIPF_NODES, 1.0, ZIPF_READS, 7);
    for (name, placement) in [("local", Placement::Local), ("planned", Placement::Planned)] {
        let store = open_store(&zipf_options(placement), ZIPF_NODES);
        let tx = store.reader();
        read_sequence(&tx, &store.ids, &order);
        eprintln!("zipf_hit_rate/{name}: served_from_cache = {:.4}", hit_rate(&store.graph));
        group.bench_function(name, |b| {
            b.iter(|| black_box(read_sequence(&tx, &store.ids, &order)))
        });
    }
    group.finish();
}

criterion_group!(benches, tier_read_latency, zipf_hit_rate);
criterion_main!(benches);
