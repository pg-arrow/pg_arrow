mod common;

use std::hint::black_box;

fn iai_bench_get_page_header() {
    common::get_page_header(black_box(20));
}

fn iai_bench_iterate_page_header() {
    common::iterate_page_header(black_box(20));
}

iai::main!(iai_bench_get_page_header, iai_bench_iterate_page_header);
