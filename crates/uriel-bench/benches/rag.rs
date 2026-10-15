use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use uriel_bench::retrieval::{corpus, search_options, QUERY};

const FILES: usize = 40;

fn search(c: &mut Criterion) {
    let corpus = corpus(FILES);
    let opts = search_options(8, 4000);
    c.bench_function("rag_search", |b| {
        b.iter(|| black_box(corpus.rag.search(QUERY, &opts).expect("search")))
    });
}

fn pack_ladder(c: &mut Criterion) {
    let corpus = corpus(FILES);
    let opts = search_options(8, 4000);
    c.bench_function("rag_pack_ladder", |b| {
        b.iter(|| black_box(corpus.rag.pack_ladder(QUERY, &opts).expect("pack")))
    });
}

criterion_group!(benches, search, pack_ladder);
criterion_main!(benches);
