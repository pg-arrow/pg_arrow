# Profiling and Debugging Guide

## Profiling Tools

Pick the right tool for the job:

| Tool | What It Measures | When to Use | Rust Integration |
|---|---|---|---|
| `criterion` | Micro-benchmarks (ns-level) | Comparing two implementations | `cargo bench` — built-in |
| `samply` | CPU sampling profiler | Finding hot functions | `samply record cargo test ...` |
| `perf` (Linux) | CPU counters, cache misses, branch mispredicts | Low-level perf analysis | `perf record/perf stat` |
| `Instruments` (macOS) | CPU, allocations, I/O, system calls | macOS-native profiling | Xcode Instruments |
| `dhat` | Heap allocation profiling | Finding excessive allocations | `dhat` crate, `#[global_allocator]` |
| `bytehound` | Memory profiler with timeline | Tracking memory over time | LD_PRELOAD-based |
| `Perfetto` | Trace visualization (chrome://tracing) | Visualizing execution timeline, async spans | Export via `tracing` + Perfetto SDK |
| `Tracy` | Real-time frame profiler | Live profiling with sub-μs overhead | `tracing-tracy` crate |

### Quick Start

```bash
# Micro-benchmark a specific function
cargo bench --bench page_parsing

# CPU profile a test with samply (macOS/Linux)
samply record cargo test --release test_parse_large_table -- --nocapture

# CPU profile with perf (Linux)
perf record -g cargo test --release test_parse_large_table -- --nocapture
perf report

# Heap allocation profiling
# Add to Cargo.toml: dhat = { version = "0.3", optional = true }
# Add #[cfg(feature = "dhat")] #[global_allocator] static ALLOC: dhat::Alloc = dhat::Alloc;
cargo test --features dhat test_parse_large_table
# Opens dhat-heap.json — view at https://nnethercote.github.io/dh_view/dh_view.html
```

## Tracing with `tracing` Crate

Use the `tracing` crate as the unified instrumentation layer. It supports structured logging, span-based timing, and can export to multiple backends simultaneously.

### Instrumenting Code

```rust
use tracing::{info, warn, instrument, span, Level};

#[instrument(skip(page_data), fields(page_num = %page_num))]
fn parse_page(page_data: &[u8], page_num: u32) -> Result<Page> {
    // Automatically creates a span with function name + fields
    info!(bytes = page_data.len(), "parsing page");
    // ...
}

// Manual span for finer control
fn scan_table(table_oid: u32) -> Result<Vec<RecordBatch>> {
    let span = span!(Level::INFO, "scan_table", oid = table_oid);
    let _guard = span.enter();

    for seg in segments {
        let seg_span = span!(Level::DEBUG, "segment", file = %seg.path());
        let _seg_guard = seg_span.enter();
        // ...
    }
}
```

### Backend Subscribers

Choose per use case — multiple can be active simultaneously via `tracing-subscriber` layers:

| Backend | Crate | Output | Use Case |
|---|---|---|---|
| Formatted logs | `tracing-subscriber` | stderr/stdout | Development, CI |
| Tracy | `tracing-tracy` | Tracy profiler (real-time) | Interactive profiling |
| Chrome trace | `tracing-chrome` | JSON (open in Perfetto UI) | Offline trace analysis |
| OpenTelemetry | `tracing-opentelemetry` | Jaeger/Zipkin | Distributed tracing |
| Log bridge | `tracing-log` | `log` crate compat | Integrating with log-based libraries |

### Example: Multi-Backend Setup

```rust
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn init_tracing() {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("pg_arrow=info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        // Uncomment for Tracy: .with(tracing_tracy::TracyLayer::default())
        // Uncomment for Chrome: .with(tracing_chrome::ChromeLayerBuilder::new().build())
        .init();
}
```

### What to Instrument

- **Always**: Table scan start/end, page parsing, MVCC visibility decisions, query execution
- **Debug level**: Per-page details, tuple-level parsing, cache hits/misses
- **Trace level**: Byte-level parsing, individual field decoding

## Interactive Debugging (GDB / LLDB)

### Setup

Rust ships with debugger wrappers that load pretty-printers for Rust types:

```bash
# Use rust-gdb or rust-lldb instead of raw gdb/lldb
rust-gdb target/debug/pg_arrow        # Linux (GDB)
rust-lldb target/debug/pg_arrow       # macOS (LLDB)

# Debug a specific test binary
cargo test --no-run test_parse_page    # Compile without running
rust-lldb target/debug/deps/pg_arrow-<hash>  # Debug the test binary
```

### Debugging Release Builds

For profiling-accurate debugging, build with debug info in release mode:

```toml
# Cargo.toml
[profile.release]
debug = true          # Full debug info in release builds
# OR
debug = "line-tables-only"  # Smaller — just file/line, no variable info
```

### Useful Breakpoint Patterns

```
# Break on a specific function
b pg_arrow::file::parse_page_header

# Break when a field has a specific value (conditional)
b pg_arrow::file::parse_page_header if page_num == 42

# Break on panic (catch all panics before unwinding)
b rust_panic                    # GDB
br set -n rust_panic            # LLDB

# Break on a specific offset being read (hardware watchpoint)
watch *(uint32_t*)(buf + 4)     # Break when bytes at buf+4 are read/written

# Print a Rust struct
p page_header                   # Pretty-printed via rust-gdb/rust-lldb
p/x page_header.pd_lsn          # Hex format
```

### Examining Binary Data in Debugger

```
# Dump raw memory as hex (GDB)
x/24bx buf              # 24 bytes in hex starting at buf (page header)
x/6wx buf               # Same as 6 x 32-bit words

# Dump raw memory (LLDB)
memory read buf buf+24 -f hex
memory read buf buf+8192 -o /tmp/page_dump.bin  # Dump to file
```

### Core Dumps

```bash
# Enable core dumps
ulimit -c unlimited

# Run and let it crash — produces core file
cargo test --release test_that_crashes

# Analyze
rust-gdb target/debug/deps/pg_arrow-<hash> core
bt                               # Backtrace
frame 3                          # Jump to frame
info locals                      # Show local variables
```

### Backtraces

```bash
# Full backtrace on panic
RUST_BACKTRACE=1 cargo test test_name

# Full backtrace with source locations
RUST_BACKTRACE=full cargo test test_name

# Backtrace for library code only (skip test harness noise)
RUST_LIB_BACKTRACE=1 cargo test test_name
```

## Syscall and I/O Tracing

Essential for an I/O-heavy project that reads heap files, mmaps pages, and does network I/O.

### strace (Linux)

```bash
# Trace all file I/O syscalls
strace -e trace=read,write,open,openat,mmap,pread64,close \
    cargo test --release test_parse_large_table 2>&1 | head -100

# Count syscalls by type (find excessive syscall patterns)
strace -c cargo test --release test_parse_large_table

# Trace only file operations on a specific path
strace -e trace=file -P testdata/postgres-latest/data/base/ \
    cargo test --release test_scan_table
```

### dtrace (macOS)

```bash
# Count syscalls by type
sudo dtrace -n 'syscall:::entry /execname == "pg_arrow"/ { @[probefunc] = count(); }'

# Trace file reads with size
sudo dtrace -n 'syscall::read:entry /execname == "pg_arrow"/ { printf("fd=%d size=%d", arg0, arg2); }'
```

### I/O Latency

```bash
# Per-file I/O latency histogram (Linux, requires bpftrace)
sudo bpftrace -e 'tracepoint:syscalls:sys_enter_read /comm == "pg_arrow"/ {
    @start[tid] = nsecs;
}
tracepoint:syscalls:sys_exit_read /comm == "pg_arrow" && @start[tid]/ {
    @usecs = hist((nsecs - @start[tid]) / 1000);
    delete(@start[tid]);
}'
```

## Assembly Inspection

Useful for verifying SIMD codegen, checking that bounds checks are elided, and understanding hot loop performance.

```bash
# View assembly for a specific function
cargo install cargo-show-asm
cargo asm pg_arrow::file::parse_page_header

# View with source interleaving
cargo asm pg_arrow::file::parse_page_header --rust

# Check if a function uses SIMD instructions
cargo asm pg_arrow::file::parse_page_header | grep -E 'vmov|vpadd|vpshuf|vpcmp'

# Full disassembly via objdump
cargo build --release
objdump -d -M intel target/release/pg_arrow | less

# Compiler Explorer style — see what optimizations applied
cargo install cargo-expand
cargo expand file::parse_page_header  # Expands macros, shows generated code
```

## Network Protocol Debugging

Relevant for wire protocol implementation and PostgreSQL connection handling.

### Capturing PostgreSQL Wire Protocol

```bash
# Capture traffic between pg_arrow and clients (or pg_arrow and PostgreSQL)
# On port 5433 (pg_arrow) or 5432 (PostgreSQL)
sudo tcpdump -i lo -w /tmp/pg_capture.pcap port 5433

# Open in Wireshark — it has a built-in PostgreSQL protocol dissector
wireshark /tmp/pg_capture.pcap
# Filter: pgsql

# Quick text dump without Wireshark
sudo tcpdump -i lo -X port 5433 | head -200
```

### pgbench for Load Testing

```bash
# Baseline PostgreSQL performance
pgbench -i -s 10 -d testdb -h localhost -p 5432
pgbench -c 16 -j 4 -T 60 -d testdb -h localhost -p 5432

# Same against pg_arrow
pgbench -c 16 -j 4 -T 60 -d testdb -h localhost -p 5433

# Custom query file
pgbench -f queries/analytical.sql -c 4 -T 30 -d testdb -h localhost -p 5433
```

## Debugging Binary Parsing

### Hex Inspection

```bash
# Dump first page (8192 bytes) of a heap file
xxd -l 8192 testdata/postgres-latest/data/base/16384/16385

# Dump specific offset range (e.g., page header: first 24 bytes)
xxd -s 0 -l 24 testdata/postgres-latest/data/base/16384/16385

# Compare two pages side by side
xxd -l 8192 file1 > /tmp/a.hex
xxd -l 8192 -s 8192 file1 > /tmp/b.hex
diff /tmp/a.hex /tmp/b.hex
```

### pageinspect (PostgreSQL Extension)

The gold standard for validating pg_arrow's parsing — compare against PostgreSQL's own interpretation:

```sql
CREATE EXTENSION pageinspect;

-- Page header
SELECT * FROM page_header(get_raw_page('test_table', 0));

-- Item pointers
SELECT * FROM heap_page_item_attrs(get_raw_page('test_table', 0), 'test_table');

-- Tuple headers
SELECT lp, t_xmin, t_xmax, t_infomask, t_infomask2, t_hoff, t_bits
FROM heap_page_items(get_raw_page('test_table', 0));

-- TOAST inspection
SELECT chunk_id, chunk_seq, length(chunk_data)
FROM pg_toast.pg_toast_<oid>;
```

### Snapshot Testing with `insta`

Capture parsed struct output as snapshots — any regression shows up as a diff:

```rust
#[test]
fn test_page_header_parsing() {
    let page = parse_page(&test_page_data).unwrap();
    insta::assert_debug_snapshot!(page.header);
}
// Creates/updates snapshots in src/snapshots/
// Review changes: cargo insta review
```

## Debugging Concurrency Issues

### Sanitizers

```bash
# ThreadSanitizer — detects data races
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu

# AddressSanitizer — buffer overflows, use-after-free
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu

# MemorySanitizer — uninitialized memory reads
RUSTFLAGS="-Zsanitizer=memory" cargo +nightly test -Zbuild-std --target x86_64-unknown-linux-gnu
```

### Miri

Detects undefined behavior in safe + unsafe Rust (slow but thorough):

```bash
cargo +nightly miri test -- test_parse_page_header
```

### tokio-console

Real-time async task debugger:

```bash
# Install
cargo install tokio-console

# Add to dependencies: console-subscriber = "0.4"
# In main: console_subscriber::init();

# Run your app, then in another terminal:
tokio-console
```

### Logging Concurrent Execution

Use `tracing` spans with thread/task IDs to reconstruct interleaving:

```rust
// In tracing subscriber setup:
tracing_subscriber::fmt()
    .with_thread_ids(true)
    .with_span_events(tracing_subscriber::fmt::format::FmtSpan::FULL)
    .init();
```

## Debugging Performance Regressions

### Comparative Benchmarking

```bash
# Save baseline
git stash  # or checkout main
cargo bench --bench page_parsing -- --save-baseline main

# Run against current changes
git stash pop  # or checkout feature branch
cargo bench --bench page_parsing -- --baseline main
```

### Flame Graphs

```bash
# Using samply (recommended — zero setup)
samply record cargo test --release test_name -- --nocapture
# Opens Firefox Profiler automatically with flame graph

# Using cargo-flamegraph
cargo install flamegraph
cargo flamegraph --test test_name -- --nocapture
# Opens flamegraph.svg
```

### Counting Allocations

```bash
# Quick allocation count with dhat
# After running with dhat feature enabled, check:
# - Total allocations
# - Peak memory
# - Allocation hotspots (by call stack)

# For tracking allocations over time:
# Use bytehound (Linux only)
LD_PRELOAD=libbytehound.so cargo test --release test_name
bytehound server bytehound-*.dat
# Opens web UI at localhost:8080
```

## References

### Profiling and Tracing

- [Tracing Methods for Performance — Thume](https://thume.ca/2023/12/02/tracing-methods/) — comprehensive survey of tracing/profiling approaches, overhead tradeoffs, and when to use each method
- [The Rust Performance Book](https://nnethercote.github.io/perf-book/) — Rust-specific optimization and profiling guidance
- [Perfetto UI](https://ui.perfetto.dev/) — open Chrome trace files for timeline visualization
- [Tracy Profiler](https://github.com/wolfpld/tracy) — real-time profiler with Rust support via `tracing-tracy`
- [samply](https://github.com/mstange/samply) — sampling profiler for macOS and Linux, outputs Firefox Profiler format
- [cargo-flamegraph](https://github.com/flamegraph-rs/flamegraph) — flame graph generation from cargo
- [dhat-rs](https://docs.rs/dhat/latest/dhat/) — heap profiling for Rust
- [cargo-show-asm](https://github.com/pacak/cargo-show-asm) — view generated assembly for Rust functions

### Debugging

- [Debugging Rust with GDB and LLDB](https://rustc-dev-guide.rust-lang.org/debugging-support-in-rustc.html) — Rust compiler dev guide on debugger integration
- [GDB to LLDB command map](https://lldb.llvm.org/use/map.html) — translating between GDB and LLDB commands
- [Miri](https://github.com/rust-lang/miri) — interpreter for detecting undefined behavior in Rust
- [tokio-console](https://github.com/tokio-rs/console) — real-time diagnostics for async Rust

### PostgreSQL-Specific

- [pageinspect](https://www.postgresql.org/docs/current/pageinspect.html) — PostgreSQL extension for low-level page inspection
- [pgbench](https://www.postgresql.org/docs/current/pgbench.html) — PostgreSQL built-in benchmarking tool
- [Wireshark PostgreSQL dissector](https://wiki.wireshark.org/PostgreSQL) — protocol-level analysis of PostgreSQL wire traffic
