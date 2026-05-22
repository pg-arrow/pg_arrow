/// Benchmark: simd-utf8 vs std::str::from_utf8 across string lengths.
///
/// Motivation: simdutf8 NEON on aarch64 has per-call setup overhead that
/// dominates for short strings but amortises over large buffers.
/// This bench finds the crossover point — expected around 128-256 bytes.
///
/// Run with:
///   cargo bench --bench simd_utf8                          # std baseline
///   cargo bench --bench simd_utf8 --features simd-utf8    # NEON path
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;

/// Sizes to sweep: from typical short PG varchar to de-TOASTed large text.
const SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 4096, 16384, 65536];

fn make_payload(size: usize) -> Vec<u8> {
    // Valid ASCII-UTF8, repeated pattern.
    let pattern = b"The quick brown fox jumps over the lazy dog. ";
    let mut v = Vec::with_capacity(size);
    while v.len() < size {
        let take = (size - v.len()).min(pattern.len());
        v.extend_from_slice(&pattern[..take]);
    }
    v
}

fn bench_validate_utf8(c: &mut Criterion) {
    let mut group = c.benchmark_group("utf8_validate");

    for &size in SIZES {
        let payload = make_payload(size);
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("std", size), &payload, |b, p| {
            b.iter(|| {
                let _ = black_box(std::str::from_utf8(black_box(p)));
            });
        });

        #[cfg(feature = "simd-utf8")]
        group.bench_with_input(BenchmarkId::new("simd_basic", size), &payload, |b, p| {
            b.iter(|| {
                let _ = black_box(simdutf8::basic::from_utf8(black_box(p)));
            });
        });

        #[cfg(feature = "simd-utf8")]
        group.bench_with_input(BenchmarkId::new("simd_compat", size), &payload, |b, p| {
            b.iter(|| {
                let _ = black_box(simdutf8::compat::from_utf8(black_box(p)));
            });
        });
    }

    group.finish();
}

/// Simulates the actual hot-path pattern in arrow.rs:
/// simd validate → if ok use directly, else fall back to from_utf8_lossy.
fn bench_arrow_hotpath(c: &mut Criterion) {
    let mut group = c.benchmark_group("utf8_arrow_hotpath");

    for &size in SIZES {
        let payload = make_payload(size);
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("lossy_only", size), &payload, |b, p| {
            b.iter(|| {
                let s = black_box(String::from_utf8_lossy(black_box(p)));
                black_box(s.as_ref().len())
            });
        });

        #[cfg(feature = "simd-utf8")]
        group.bench_with_input(
            BenchmarkId::new("simd_then_lossy_fallback", size),
            &payload,
            |b, p| {
                b.iter(|| {
                    let s = match simdutf8::basic::from_utf8(black_box(p)) {
                        Ok(s) => std::borrow::Cow::Borrowed(s),
                        Err(_) => String::from_utf8_lossy(p),
                    };
                    black_box(s.as_ref().len())
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_validate_utf8, bench_arrow_hotpath);
criterion_main!(benches);
