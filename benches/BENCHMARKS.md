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

## When to Use Which

| | Criterion | iai |
|---|---|---|
| **What it measures** | Wall-clock time | CPU instruction count |
| **Deterministic** | No (noisy) | Yes |
| **Tunable** | Extensively | No |
| **Good for** | Real-world perf, throughput | CI regression detection |
| **Requires** | Nothing extra | Valgrind (cachegrind) |
