# Benchmarks

## Structure

```
benches/
  common/mod.rs        # Shared benchmark functions
  criterion_bench.rs   # Statistical wall-clock benchmarks (Criterion)
  iai_bench.rs         # Deterministic instruction-count benchmarks (iai)
```

## Running

```bash
cargo bench                          # Run all benchmarks
cargo bench --bench criterion_bench  # Criterion only
cargo bench --bench iai_bench        # iai only
```

## Criterion Configuration

### In-Code Options

```rust
Criterion::default()
    .sample_size(100)                          // Number of samples (default: 100, min: 10)
    .measurement_time(Duration::from_secs(5))  // Time spent measuring (default: 5s)
    .warm_up_time(Duration::from_secs(3))      // Warmup before measuring (default: 3s)
    .confidence_level(0.95)                    // Confidence for statistical tests (default: 0.95)
    .significance_level(0.05)                  // Threshold for "performance changed" (default: 0.05)
    .noise_threshold(0.01)                     // Ignore changes smaller than 1% (default: 0.01)
    .nresamples(100_000);                      // Bootstrap resamples (default: 100,000)
```

Per-benchmark group overrides:

```rust
let mut group = c.benchmark_group("page_reading");
group.sample_size(200);
group.measurement_time(Duration::from_secs(10));
group.throughput(Throughput::Elements(1));   // Report elements/sec
// or: group.throughput(Throughput::Bytes(8192));  // Report bytes/sec
group.finish();
```

Parameterized benchmarks:

```rust
for size in [1, 10, 100] {
    group.bench_with_input(BenchmarkId::new("get_page", size), &size, |b, &s| {
        b.iter(|| common::get_page_header(s))
    });
}
```

### CLI Flags

Pass after `--`: `cargo bench --bench criterion_bench -- <flags>`

| Flag | Purpose |
|---|---|
| `--sample-size N` | Override sample count |
| `--measurement-time N` | Seconds to measure |
| `--warm-up-time N` | Seconds to warm up |
| `--confidence-level N` | e.g. 0.99 |
| `--significance-level N` | e.g. 0.01 |
| `--noise-threshold N` | e.g. 0.02 |
| `--quick` | Minimal samples for fast iteration |
| `--baseline NAME` | Save results as named baseline |
| `--save-baseline NAME` | Save without comparing |
| `--load-baseline NAME` | Compare against saved baseline |
| `--output-format bencher` | Machine-readable output |
| `--plotting-backend disabled` | Skip HTML report generation |
| `FILTER` | Regex to select benchmarks by name |

Example:

```bash
cargo bench --bench criterion_bench -- --sample-size 200 --warm-up-time 5 "page header"
```

Reports are generated in `target/criterion/`.

## iai Configuration

iai uses Cachegrind for deterministic instruction-count measurement. There are no tuning parameters by design — every run produces the same result regardless of system load.

The only configuration is which functions are listed in `iai::main!()`.

### Upgrading to iai-callgrind

For more control with instruction-count benchmarking, `iai-callgrind` (the maintained successor to `iai`) adds:

- Multiple inputs per benchmark (`#[bench::name(args)]`)
- Event kind selection (instructions, cache misses, branch mispredictions)
- Regression detection with configurable thresholds
- Flamegraph generation

```rust
use iai_callgrind::{library_benchmark, library_benchmark_group, main};

#[library_benchmark]
#[bench::short(20)]
#[bench::long(2000)]
fn bench_get_page(hole: usize) {
    common::get_page_header(hole);
}

library_benchmark_group!(name = page_benches; benchmarks = bench_get_page);
main!(library_benchmark_groups = page_benches);
```

---

## Recorded Benchmark Results

### simd-utf8 experiment (`experiment/simd-utf8` branch)

**Platform**: aarch64, Apple M-series (native), macOS  
**Crate**: simdutf8 0.1.5, feature `aarch64_neon` (NEON intrinsics enabled)  
**Bench**: `cargo bench --bench simd_utf8 --features simd-utf8`  
**Criterion**: 100 samples, 3s warmup, 5s measurement per variant/size

#### Variants compared

| Variant | Description | Where used |
|---|---|---|
| `std` | `std::str::from_utf8` — scalar, returns `Utf8Error` with byte offset | default (no feature) |
| `simd_basic` | `simdutf8::basic::from_utf8` — fast NEON scan, **no error offset**; has false-negative rate for valid input requiring scalar re-check on some inputs | not used (tested only) |
| `simd_compat` | `simdutf8::compat::from_utf8` — precise NEON pass, returns `Utf8Error`-compatible error with offset; **no scalar fallback** for valid input | `codec.rs` + `arrow.rs` |
| `lossy_only` | `String::from_utf8_lossy` — scalar, replaces bad bytes with U+FFFD, allocates `String` | `arrow.rs` without feature |
| `simd_compat + lossy_fallback` | `simd_compat` for valid input (zero-copy), `from_utf8_lossy` only on invalid bytes | `arrow.rs` with feature |

**Why `simd_compat` beats `simd_basic` at short-medium sizes**: `simd_basic` uses a
two-phase approach — a fast NEON scan that may produce false negatives (valid input
rejected), followed by a scalar re-validation pass. This extra branch dominates at
small sizes. `simd_compat` does a single, more precise NEON pass; no re-check needed
for valid UTF-8. At very large sizes (16KB+) `simd_basic` pulls ahead slightly.

#### `utf8_validate` group — pure validation, no allocation

| bytes | std (ns) | simd_basic (ns) | simd_compat (ns) | basic speedup | compat speedup |
|------:|---------:|----------------:|-----------------:|--------------:|---------------:|
|     8 |     4.73 |            9.65 |             5.32 |         0.49x |          0.89x |
|    16 |     2.65 |            2.65 |             3.23 |         1.00x |          0.82x |
|    32 |     3.53 |            7.09 |             4.11 |         0.50x |          0.86x |
|    64 |     4.47 |            2.37 |             2.54 |     **1.88x** |      **1.76x** |
|   128 |     5.29 |            8.34 |             3.06 |         0.63x |      **1.73x** |
|   256 |     7.69 |            5.33 |             4.35 |     **1.44x** |      **1.77x** |
|   512 |    12.51 |           13.34 |             6.21 |         0.94x |      **2.01x** |
|  1024 |    27.20 |           18.41 |            10.44 |     **1.48x** |      **2.61x** |
|  4096 |   119.85 |           43.72 |            35.76 |     **2.74x** |      **3.35x** |
| 16384 |   507.97 |          135.02 |           141.38 |     **3.76x** |      **3.59x** |
| 65536 |  2051.50 |          504.11 |           540.76 |     **4.07x** |      **3.79x** |

**Crossover**: `simd_compat` breaks even at **~64 bytes**; `simd_basic` at ~64 bytes too
but with erratic behaviour at 128 bytes. For sizes ≥ 512 bytes `simd_compat` is the
consistent winner. At 16KB+ `simd_basic` is marginally faster.

**Implication**: PostgreSQL inline varchar/bpchar (≤ ~60 bytes) sees no gain. De-TOASTed
text fields (typically 256 bytes–1 MB) will benefit significantly.

#### `utf8_arrow_hotpath` group — simulates `arrow.rs` ColumnBuilder::append_bytes

Measures the exact hot-path pattern: `simd_compat` validate → if valid use `&str`
directly (zero allocation); else fall back to `from_utf8_lossy` (allocates).

⚠️ **Note**: The `lossy_only` numbers include a `String` allocation on every call
(necessary since `from_utf8_lossy` returns `Cow` which must outlive the call in the
bench). The `simd+fallback` path avoids that allocation for valid UTF-8 (the common
case). The extreme speedup ratios at large sizes reflect allocation cost, not pure
validation.

| bytes | lossy_only (ns) | simd+fallback (ns) | speedup |
|------:|----------------:|-------------------:|--------:|
|     8 |            5.41 |               4.82 |   1.12x |
|    16 |            7.78 |               2.71 |   2.87x |
|    32 |           12.59 |               3.57 |   3.53x |
|    64 |           21.93 |               2.40 |   9.14x |
|   128 |           50.05 |               3.29 |  15.21x |
|   256 |           92.26 |               4.25 |  21.71x |
|   512 |          167.83 |               5.16 |  32.55x |
|  1024 |          320.58 |               9.01 |  35.58x |
|  4096 |         1229.46 |              32.49 |  37.84x |
| 16384 |         4877.83 |             130.04 |  37.51x |
| 65536 |        19299.75 |             499.95 |  38.60x |

The real-world gain in `arrow.rs` will be smaller than these ratios suggest because
`StringBuilder::append_value` copies the string into the Arrow buffer regardless —
the allocation savings only apply to the intermediate `Cow`. Still, eliminating
`from_utf8_lossy`'s internal allocation on every row is a genuine win for large strings.

#### End-to-end — TPC-H SF10, `lineitem` string columns (~60M rows)

Query: `SELECT l_shipinstruct, l_shipmode, l_comment FROM lineitem`  
Columns: `l_shipinstruct` bpchar(25), `l_shipmode` bpchar(10), `l_comment` varchar(44)  
Avg string length: **~10–44 bytes** — well below the ~64 byte crossover.

| build | best of 3 (ms) | notes |
|---|---:|---|
| without simd-utf8 (std) | 3244 | `from_utf8_lossy` scalar |
| simd-utf8 (compat+NEON) | 3254 | within noise |

No measurable end-to-end gain. Bottleneck is I/O + Arrow buffer building, not UTF-8
validation. Expected win once TOAST decompression is implemented — de-TOASTed strings
will be 256 bytes to several MB, squarely in the 3–4x gain region.

## When to Use Which

| | Criterion | iai |
|---|---|---|
| **What it measures** | Wall-clock time | CPU instruction count |
| **Deterministic** | No (noisy) | Yes |
| **Tunable** | Extensively | No |
| **Good for** | Real-world perf, throughput | CI regression detection |
| **Requires** | Nothing extra | Valgrind (cachegrind) |
