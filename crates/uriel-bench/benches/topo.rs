use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use uriel_bench::topo::{blocks, cloud, graph, DIM};
use uriel_topo::layout::topology_order;
use uriel_topo::persistence::{persistence_f64, PersistenceConfig};
use uriel_topo::schedule::{schedule, ScheduleParams};

fn persistence(c: &mut Criterion) {
    let mut group = c.benchmark_group("topo_persistence");
    group.sample_size(20);
    let cfg = PersistenceConfig::default();
    for n in [500usize, 2_000] {
        let data = cloud(n, DIM, 1);
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| black_box(persistence_f64(&data, DIM, None, &cfg).expect("persistence")))
        });
    }
    group.finish();
}

fn schedule_blocks(c: &mut Criterion) {
    let mut group = c.benchmark_group("topo_schedule");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));
    let blocks = blocks(5_000, 2);
    let params = ScheduleParams::default();
    group.bench_function(BenchmarkId::from_parameter(5_000), |b| {
        b.iter(|| black_box(schedule(&blocks, &params, None).expect("schedule")))
    });
    group.finish();
}

fn layout(c: &mut Criterion) {
    let mut group = c.benchmark_group("topo_layout");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));
    let n = 50_000;
    let edges = graph(n, 3);
    group.bench_function(BenchmarkId::from_parameter(n), |b| {
        b.iter(|| black_box(topology_order(n, &edges, None).expect("layout")))
    });
    group.finish();
}

criterion_group!(benches, persistence, schedule_blocks, layout);
criterion_main!(benches);
