mod common;

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn criterion_benchmark(c: &mut Criterion) {
    // c.bench_function("Read page header", |b| {
    //     b.iter(|| common::get_page_header(black_box(20)))
    // });
    // c.bench_function("Iterate page header", |b| {
    //     b.iter(|| common::iterate_page_header(black_box(20)))
    // });
    //
    // c.bench_function("PgTableReader bootstrap", |b| {
    //     b.iter(|| common::bench_table_reader_bootstrap(black_box(20)))
    // });
    // c.bench_function("PgTableReader fetch_all pg_class", |b| {
    //     b.iter(|| common::bench_table_reader_fetch_all(black_box(20)))
    // });
    c.bench_function("PgTableReader fetch_by_limit", |b| {
        b.iter(|| common::bench_table_reader_fetch_with_limit(black_box(20)))
    });
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
