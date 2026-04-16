use pg_arrow::file::reader::*;
use pg_arrow::table::PgTableReader;

pub fn get_page_header(_hole: usize) {
    let table_reader = TableFileReader::new(16384, 1259);

    let mut page_reader = table_reader.get_page_reader().unwrap();
    let pg_heap_page = page_reader.get_page_by_index(0).unwrap();
    assert!(pg_heap_page.lp_num == 51);
}

pub fn iterate_page_header(_hole: usize) {
    let table_reader = TableFileReader::new(16384, 16630);

    let page_reader = table_reader.get_page_reader().unwrap();
    for _row in page_reader.into_iter() {
        std::hint::black_box(1);
    }
}

pub fn bench_table_reader_bootstrap(_hole: usize) {
    let reader = PgTableReader::new(16384).unwrap();
    std::hint::black_box(reader);
}

pub fn bench_table_reader_fetch_all(_hole: usize) {
    let mut reader = PgTableReader::new(16384).unwrap();
    reader.set_table("pgbench_accounts").unwrap();
    let rows = reader.fetch_all().unwrap();
    std::hint::black_box(rows);
}

pub fn bench_table_reader_fetch_with_limit(_hole: usize) {
    let mut reader = PgTableReader::new(16384).unwrap();
    reader.set_table("pgbench_accounts").unwrap();
    let rows = reader.fetch_by_limit(5_000_000).unwrap();
    std::hint::black_box(rows);
}
