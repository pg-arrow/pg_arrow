//! Benchmark: file read latency for PostgreSQL page reads.
//!
//! Compares two approaches:
//!   1. Current logic: synchronous pread via BufReader (as used by PageReader)
//!   2. macOS true async: POSIX AIO (aio_read) for concurrent non-blocking reads
//!
//! Both read raw 8KB pages from the same PostgreSQL heap file. The benchmark
//! measures pure I/O latency — no tuple parsing or Arrow conversion.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use pg_arrow::heap::page::{HeapPageData, ItemIdData, PageHeaderData, read_line_pointer};
use std::fs::File;
use std::hint::black_box;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::io::AsRawFd;

const PAGE_SIZE: usize = 8192;

/// Resolve the heap file path for the test database's pgbench_accounts table.
/// Falls back to pg-test-config.toml like the rest of the test suite.
fn get_heap_file_path() -> String {
    let project_root = env!("CARGO_MANIFEST_DIR");
    let config_path = format!("{}/pg-test-config.toml", project_root);
    let config_str = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| panic!("Cannot read {config_path}: {e}"));
    let table: toml::Table = config_str.parse().unwrap();

    let data_dir = table
        .get("postgres")
        .and_then(|v| v.get("pg18"))
        .and_then(|v| v.get("data_dir"))
        .and_then(|v| v.as_str())
        .expect("pg-test-config.toml missing postgres.pg18.data_dir");

    // Use the hits table (relfilenode 16728, db 16727) — first segment is ~1GB
    // which gives ~131072 pages for meaningful I/O benchmarks.
    format!("{}/base/16727/16728", data_dir)
}

/// Get the number of pages in the file.
fn file_page_count(path: &str) -> usize {
    let meta = std::fs::metadata(path).unwrap();
    meta.len() as usize / PAGE_SIZE
}

// ─────────────────────────────────────────────────────────────────────────────
// Approach 1: Current logic — sequential BufReader reads (mirrors PageReader)
// ─────────────────────────────────────────────────────────────────────────────

fn read_pages_sequential(path: &str, num_pages: usize) -> Vec<[u8; PAGE_SIZE]> {
    let mut file = File::open(path).unwrap();
    let mut pages = Vec::with_capacity(num_pages);

    for i in 0..num_pages {
        let mut buf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start((i * PAGE_SIZE) as u64)).unwrap();
        file.read_exact(&mut buf).unwrap();
        pages.push(buf);
    }

    pages
}

// ─────────────────────────────────────────────────────────────────────────────
// Approach 1b: pread (no seek needed, mirrors what a proper impl would do)
// ─────────────────────────────────────────────────────────────────────────────

fn read_pages_pread(path: &str, num_pages: usize) -> Vec<[u8; PAGE_SIZE]> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    let mut pages = Vec::with_capacity(num_pages);

    for i in 0..num_pages {
        let mut buf = [0u8; PAGE_SIZE];
        let offset = (i * PAGE_SIZE) as i64;
        let n =
            unsafe { libc::pread(fd, buf.as_mut_ptr() as *mut libc::c_void, PAGE_SIZE, offset) };
        assert_eq!(n as usize, PAGE_SIZE, "short pread at page {i}");
        pages.push(buf);
    }

    pages
}

// ─────────────────────────────────────────────────────────────────────────────
// Approach 2: macOS POSIX AIO — submit all reads, then wait for completion
// ─────────────────────────────────────────────────────────────────────────────

/// macOS limits concurrent AIO operations (typically AIO_LISTIO_MAX = 16,
/// system-wide AIO_MAX = 90). We submit in batches to stay within limits.
const AIO_BATCH_SIZE: usize = 16;

fn read_pages_aio(path: &str, num_pages: usize) -> Vec<[u8; PAGE_SIZE]> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();

    let mut pages: Vec<[u8; PAGE_SIZE]> = vec![[0u8; PAGE_SIZE]; num_pages];

    // Process in batches to respect macOS AIO limits.
    for batch_start in (0..num_pages).step_by(AIO_BATCH_SIZE) {
        let batch_end = (batch_start + AIO_BATCH_SIZE).min(num_pages);
        let batch_len = batch_end - batch_start;

        let mut aiocbs: Vec<libc::aiocb> = Vec::with_capacity(batch_len);

        for i in batch_start..batch_end {
            let mut cb: libc::aiocb = unsafe { std::mem::zeroed() };
            cb.aio_fildes = fd;
            cb.aio_offset = (i * PAGE_SIZE) as i64;
            cb.aio_buf = pages[i].as_mut_ptr() as *mut libc::c_void;
            cb.aio_nbytes = PAGE_SIZE;
            cb.aio_sigevent.sigev_notify = libc::SIGEV_NONE;
            aiocbs.push(cb);
        }

        // Submit batch.
        for cb in &mut aiocbs {
            let ret = unsafe { libc::aio_read(cb as *mut libc::aiocb) };
            assert_eq!(
                ret,
                0,
                "aio_read submit failed: {}",
                std::io::Error::last_os_error()
            );
        }

        // Wait for batch to complete.
        for cb in &mut aiocbs {
            loop {
                let err = unsafe { libc::aio_error(cb as *const libc::aiocb) };
                if err == libc::EINPROGRESS {
                    std::hint::spin_loop();
                    continue;
                }
                assert_eq!(err, 0, "aio_error returned {err}");
                let n = unsafe { libc::aio_return(cb as *mut libc::aiocb) };
                assert_eq!(n as usize, PAGE_SIZE, "short aio read");
                break;
            }
        }
    }

    pages
}

// ─────────────────────────────────────────────────────────────────────────────
// Approach 2b: Batched AIO — submit in batches with lio_listio
// ─────────────────────────────────────────────────────────────────────────────

fn read_pages_lio_listio(path: &str, num_pages: usize) -> Vec<[u8; PAGE_SIZE]> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();

    let mut pages: Vec<[u8; PAGE_SIZE]> = vec![[0u8; PAGE_SIZE]; num_pages];

    for batch_start in (0..num_pages).step_by(AIO_BATCH_SIZE) {
        let batch_end = (batch_start + AIO_BATCH_SIZE).min(num_pages);
        let batch_len = batch_end - batch_start;

        let mut aiocbs: Vec<libc::aiocb> = Vec::with_capacity(batch_len);

        for i in batch_start..batch_end {
            let mut cb: libc::aiocb = unsafe { std::mem::zeroed() };
            cb.aio_fildes = fd;
            cb.aio_offset = (i * PAGE_SIZE) as i64;
            cb.aio_buf = pages[i].as_mut_ptr() as *mut libc::c_void;
            cb.aio_nbytes = PAGE_SIZE;
            cb.aio_lio_opcode = libc::LIO_READ;
            cb.aio_sigevent.sigev_notify = libc::SIGEV_NONE;
            aiocbs.push(cb);
        }

        let ptrs: Vec<*mut libc::aiocb> =
            aiocbs.iter_mut().map(|cb| cb as *mut libc::aiocb).collect();
        let ret = unsafe {
            libc::lio_listio(
                libc::LIO_WAIT,
                ptrs.as_ptr() as *const *mut libc::aiocb,
                ptrs.len() as i32,
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            ret,
            0,
            "lio_listio failed: {}",
            std::io::Error::last_os_error()
        );

        for cb in &mut aiocbs {
            let n = unsafe { libc::aio_return(cb as *mut libc::aiocb) };
            assert_eq!(n as usize, PAGE_SIZE, "short lio read");
        }
    }

    pages
}

// ─────────────────────────────────────────────────────────────────────────────
// Approach 3: Bulk pread — read PAGES_PER_IO consecutive pages per syscall
// ─────────────────────────────────────────────────────────────────────────────

const PAGES_PER_IO: usize = 5;
const BULK_READ_SIZE: usize = PAGES_PER_IO * PAGE_SIZE; // 40KB per syscall

fn read_pages_pread_bulk(path: &str, num_pages: usize) -> Vec<[u8; PAGE_SIZE]> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    let mut pages = Vec::with_capacity(num_pages);

    let mut page_idx = 0;
    while page_idx < num_pages {
        let remaining = num_pages - page_idx;
        let chunk = remaining.min(PAGES_PER_IO);
        let read_bytes = chunk * PAGE_SIZE;

        let mut buf = vec![0u8; read_bytes];
        let offset = (page_idx * PAGE_SIZE) as i64;
        let n = unsafe {
            libc::pread(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                read_bytes,
                offset,
            )
        };
        assert_eq!(
            n as usize, read_bytes,
            "short bulk pread at page {page_idx}"
        );

        // Split the bulk buffer into individual page-sized arrays.
        for i in 0..chunk {
            let mut page = [0u8; PAGE_SIZE];
            page.copy_from_slice(&buf[i * PAGE_SIZE..(i + 1) * PAGE_SIZE]);
            pages.push(page);
        }

        page_idx += chunk;
    }

    pages
}

// ─────────────────────────────────────────────────────────────────────────────
// Approach 3b: Bulk pread, zero-copy — return slices into the read buffer
// ─────────────────────────────────────────────────────────────────────────────

fn read_pages_pread_bulk_zerocopy(path: &str, num_pages: usize, pages_per_io: usize) -> Vec<u8> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    let total_bytes = num_pages * PAGE_SIZE;
    let chunk_bytes = pages_per_io * PAGE_SIZE;
    let mut buf = vec![0u8; total_bytes];

    let mut offset = 0usize;
    while offset < total_bytes {
        let chunk = (total_bytes - offset).min(chunk_bytes);
        let n = unsafe {
            libc::pread(
                fd,
                buf[offset..].as_mut_ptr() as *mut libc::c_void,
                chunk,
                offset as i64,
            )
        };
        assert_eq!(n as usize, chunk, "short bulk pread at offset {offset}");
        offset += chunk;
    }

    // Caller accesses pages as: buf[i*PAGE_SIZE..(i+1)*PAGE_SIZE]
    buf
}

// ─────────────────────────────────────────────────────────────────────────────
// Benchmark groups
// ─────────────────────────────────────────────────────────────────────────────

fn bench_file_read_latency(c: &mut Criterion) {
    let path = get_heap_file_path();
    let total_pages = file_page_count(&path);
    println!("Benchmarking against {path} ({total_pages} pages)");

    // Test with different page counts to see scaling behavior and AIO crossover.
    let page_counts: Vec<usize> = vec![1, 16, 128, 1024]
        .into_iter()
        .filter(|&n| n <= total_pages)
        .collect();

    let mut group = c.benchmark_group("file_read_latency");

    for &num_pages in &page_counts {
        group.bench_with_input(
            BenchmarkId::new("sequential_bufread", num_pages),
            &num_pages,
            |b, &n| b.iter(|| black_box(read_pages_sequential(&path, n))),
        );

        group.bench_with_input(BenchmarkId::new("pread", num_pages), &num_pages, |b, &n| {
            b.iter(|| black_box(read_pages_pread(&path, n)))
        });

        group.bench_with_input(
            BenchmarkId::new("pread_bulk_5x", num_pages),
            &num_pages,
            |b, &n| b.iter(|| black_box(read_pages_pread_bulk(&path, n))),
        );

        group.bench_with_input(
            BenchmarkId::new("aio_read", num_pages),
            &num_pages,
            |b, &n| b.iter(|| black_box(read_pages_aio(&path, n))),
        );

        group.bench_with_input(
            BenchmarkId::new("lio_listio", num_pages),
            &num_pages,
            |b, &n| b.iter(|| black_box(read_pages_lio_listio(&path, n))),
        );
    }

    group.finish();

    // ── Sweep pages-per-io for zerocopy bulk pread at fixed 1024 pages ───
    let num_pages = 1024.min(total_pages);
    let mut sweep = c.benchmark_group("bulk_zerocopy_pages_per_io");
    let pages_per_io_values = [1, 2, 4, 8, 16, 32, 64, 128];

    for &ppi in &pages_per_io_values {
        sweep.bench_with_input(BenchmarkId::from_parameter(ppi), &ppi, |b, &ppi| {
            b.iter(|| black_box(read_pages_pread_bulk_zerocopy(&path, num_pages, ppi)))
        });
    }

    sweep.finish();
}

// ─────────────────────────────────────────────────────────────────────────────
// Benchmark: read + parse — cache line effects
//
// Compares two strategies at fixed 1024 pages:
//   "bulk_then_parse": read ALL pages in one shot, then parse all
//   "interleaved_{N}": read N pages, parse them immediately, repeat
//
// The interleaved approach keeps the working set in L1/L2 cache during parse.
// Apple Silicon: 128-byte cache lines, 64KB L1d, ~16MB shared L2.
// ─────────────────────────────────────────────────────────────────────────────

/// Read all pages into one buffer, then parse each page.
fn bulk_then_parse(path: &str, num_pages: usize) -> Vec<HeapPageData> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    let total_bytes = num_pages * PAGE_SIZE;
    let mut buf = vec![0u8; total_bytes];

    // One big read.
    let mut offset = 0usize;
    while offset < total_bytes {
        let chunk = (total_bytes - offset).min(128 * PAGE_SIZE);
        let n = unsafe {
            libc::pread(
                fd,
                buf[offset..].as_mut_ptr() as *mut libc::c_void,
                chunk,
                offset as i64,
            )
        };
        assert_eq!(n as usize, chunk);
        offset += chunk;
    }

    // Then parse all.
    let mut pages = Vec::with_capacity(num_pages);
    for i in 0..num_pages {
        let start = i * PAGE_SIZE;
        let mut page_buf = [0u8; PAGE_SIZE];
        page_buf.copy_from_slice(&buf[start..start + PAGE_SIZE]);
        pages.push(HeapPageData::parse(page_buf).unwrap());
    }
    pages
}

/// Read `chunk_size` pages, parse them immediately, repeat.
/// Keeps the working set cache-hot during parsing.
fn interleaved_read_parse(path: &str, num_pages: usize, chunk_size: usize) -> Vec<HeapPageData> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    let chunk_bytes = chunk_size * PAGE_SIZE;
    let mut pages = Vec::with_capacity(num_pages);

    let mut page_idx = 0;
    while page_idx < num_pages {
        let remaining = num_pages - page_idx;
        let this_chunk = remaining.min(chunk_size);
        let read_bytes = this_chunk * PAGE_SIZE;

        let mut buf = vec![0u8; read_bytes];
        let offset = (page_idx * PAGE_SIZE) as i64;
        let n = unsafe {
            libc::pread(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                read_bytes,
                offset,
            )
        };
        assert_eq!(n as usize, read_bytes);

        // Parse immediately while data is cache-hot.
        for i in 0..this_chunk {
            let start = i * PAGE_SIZE;
            let mut page_buf = [0u8; PAGE_SIZE];
            page_buf.copy_from_slice(&buf[start..start + PAGE_SIZE]);
            pages.push(HeapPageData::parse(page_buf).unwrap());
        }

        page_idx += this_chunk;
    }

    pages
}

/// Same as interleaved but parses header + line pointers directly from the
/// bulk buffer slice — no copy_from_slice into an owned [u8; 8192].
/// This isolates the cost of the copy.
fn interleaved_read_parse_nocopy(
    path: &str,
    num_pages: usize,
    chunk_size: usize,
) -> Vec<(PageHeaderData, Vec<ItemIdData>)> {
    let file = File::open(path).unwrap();
    let fd = file.as_raw_fd();
    let mut results = Vec::with_capacity(num_pages);

    let mut page_idx = 0;
    while page_idx < num_pages {
        let remaining = num_pages - page_idx;
        let this_chunk = remaining.min(chunk_size);
        let read_bytes = this_chunk * PAGE_SIZE;

        let mut buf = vec![0u8; read_bytes];
        let offset = (page_idx * PAGE_SIZE) as i64;
        let n = unsafe {
            libc::pread(
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                read_bytes,
                offset,
            )
        };
        assert_eq!(n as usize, read_bytes);

        // Parse directly from the slice — no copy into [u8; 8192].
        for i in 0..this_chunk {
            let page_slice = &buf[i * PAGE_SIZE..(i + 1) * PAGE_SIZE];
            let header = PageHeaderData::parse(page_slice).unwrap();
            let lp_num = header.num_line_pointers();
            let mut lp_items = Vec::with_capacity(lp_num);
            for lp_index in 0..lp_num {
                lp_items.push(read_line_pointer(page_slice, lp_index));
            }
            results.push((header, lp_items));
        }

        page_idx += this_chunk;
    }

    results
}

fn bench_read_parse_cache(c: &mut Criterion) {
    let path = get_heap_file_path();
    let total_pages = file_page_count(&path);
    let num_pages = total_pages;

    let mut group = c.benchmark_group("read_parse_cache");

    group.bench_function("bulk_then_parse", |b| {
        b.iter(|| black_box(bulk_then_parse(&path, num_pages)))
    });

    // Interleaved at various chunk sizes (pages read per IO before parsing).
    // Smaller chunks = warmer cache for parse, more syscalls.
    // L1d is 64KB = 8 pages, L2 chunk ~2048 pages on Apple Silicon.
    // let chunk_sizes = [4, 8, 16, 32, 64, 128];
    let chunk_sizes = [64, 96, 128];
    for &cs in &chunk_sizes {
        group.bench_with_input(BenchmarkId::new("interleaved", cs), &cs, |b, &cs| {
            b.iter(|| black_box(interleaved_read_parse(&path, num_pages, cs)))
        });
    }

    for &cs in &chunk_sizes {
        group.bench_with_input(BenchmarkId::new("interleaved_nocopy", cs), &cs, |b, &cs| {
            b.iter(|| black_box(interleaved_read_parse_nocopy(&path, num_pages, cs)))
        });
    }

    group.finish();
}

criterion_group!(benches, bench_file_read_latency, bench_read_parse_cache);
criterion_main!(benches);
