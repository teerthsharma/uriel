use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use uriel_bench::table::table;
use uriel_mcp::compact::encode_table;

fn encode_200_rows(c: &mut Criterion) {
    let table = table(200);
    c.bench_function("compact_encode_table_200_rows", |b| {
        b.iter(|| black_box(encode_table(&table)))
    });
}

criterion_group!(benches, encode_200_rows);
criterion_main!(benches);
