# pg_arrow justfile
# Usage: just <recipe>   (run from pg_arrow/)
# Requires: https://github.com/casey/just

pg_version := env_var_or_default("PG_VERSION", "pg18")

# ── Default ───────────────────────────────────────────────────────────────────

[group('default')]
help:
    @just --list --unsorted

# ── Build ─────────────────────────────────────────────────────────────────────

# Debug build
[group('build')]
build:
    cargo build

# Release build
[group('build')]
release:
    cargo build --release

# Install sccache and configure it automatically in .cargo/config.toml
# Default cache: ~/Library/Caches/Mozilla.sccache (macOS) or ~/.cache/sccache (Linux), 10GB
# Override: SCCACHE_CACHE_SIZE=20G  or  SCCACHE_DIR=/custom/path
[group('build')]
sccache-setup:
    @if ! command -v sccache >/dev/null 2>&1; then \
        echo "Installing sccache..."; \
        cargo install sccache; \
    else \
        echo "sccache already installed: $(sccache --version)"; \
    fi
    @mkdir -p .cargo
    @if ! grep -q 'rustc-wrapper.*sccache' .cargo/config.toml 2>/dev/null; then \
        if grep -q '^\[build\]' .cargo/config.toml 2>/dev/null; then \
            awk '/^\[build\]/{print; print "rustc-wrapper = \"sccache\""; next} 1' \
                .cargo/config.toml > .cargo/config.toml.tmp && mv .cargo/config.toml.tmp .cargo/config.toml; \
        else \
            { [ -s .cargo/config.toml ] && printf '\n'; printf '[build]\nrustc-wrapper = "sccache"\n'; } >> .cargo/config.toml; \
        fi; \
        echo "sccache configured in .cargo/config.toml"; \
    else \
        echo "sccache already configured in .cargo/config.toml"; \
    fi

# Show sccache statistics (run after a build to verify cache hits)
[group('build')]
sccache-stats:
    sccache --show-stats

# ── Lint & Format ─────────────────────────────────────────────────────────────

[group('lint')]
fmt:
    cargo fmt

[group('lint')]
fmt-check:
    cargo fmt --check

[group('lint')]
clippy:
    cargo clippy -- -D warnings

# ── Tests ─────────────────────────────────────────────────────────────────────

[group('test')]
test:
    cargo test

# ── Benchmarks ────────────────────────────────────────────────────────────────

# Criterion statistical benchmarks (optional filter regex)
[group('bench')]
bench filter="":
    cargo bench --bench criterion_bench -- {{filter}}

# iai instruction-count benchmarks
[group('bench')]
bench-iai:
    cargo bench --bench iai_bench

# File I/O latency benchmarks
[group('bench')]
bench-io:
    cargo bench --bench file_read_latency

# Run all benchmarks
[group('bench')]
bench-all: bench bench-iai bench-io

# ── Examples ──────────────────────────────────────────────────────────────────

# Usage: just example-table-reader /path/to/pgdata 16384
[group('examples')]
example-table-reader data_dir db_id="16384":
    cargo run --example table_reader -- {{data_dir}} {{db_id}}

# ── PostgreSQL CLI ────────────────────────────────────────────────────────────

# Open a psql session for a given PostgreSQL version
# Usage: just psql pg18   or   just psql pg18 test
[group('postgres')]
psql pg=pg_version db="postgres":
    @bin=$(awk -v s="postgres.{{pg}}" '$0~"\\["s"\\]"{f=1} f&&$1=="bin_dir"{gsub(/.*= *"|"$/,""); print $0; exit}' pg-test-config.toml); \
     DYLD_LIBRARY_PATH="$bin/../lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" "$bin/psql" {{db}}

# ── PostgreSQL Setup ──────────────────────────────────────────────────────────

harness_setup := env_var_or_default("PG_HARNESS_DIR", "") + "/scripts/setup-postgres.sh"

[private]
check-harness:
    @[ -n "${PG_HARNESS_DIR:-}" ] || { echo "error: PG_HARNESS_DIR is not set\nSet it to your pg-test-harness clone: export PG_HARNESS_DIR=/path/to/pg-test-harness"; exit 1; }

# Full setup: build from source, init cluster, load test data
# Usage: just pg-setup pg18   (or pg17 / latest)
[group('postgres')]
pg-setup pg=pg_version: check-harness
    TARGET_DIR="$(pwd)" TESTDATA_DIR="$(pwd)/testdata" bash {{harness_setup}} -b {{pg}} -B -i -t

# Full setup with simple schema (no pgbench tables)
[group('postgres')]
pg-setup-simple pg=pg_version: check-harness
    TARGET_DIR="$(pwd)" TESTDATA_DIR="$(pwd)/testdata" bash {{harness_setup}} -b {{pg}} -B -i -t -s

# Build PostgreSQL source only
[group('postgres')]
pg-build pg=pg_version: check-harness
    TARGET_DIR="$(pwd)" TESTDATA_DIR="$(pwd)/testdata" bash {{harness_setup}} -b {{pg}} -B

# Init cluster only (source must already be built)
[group('postgres')]
pg-init pg=pg_version: check-harness
    TARGET_DIR="$(pwd)" TESTDATA_DIR="$(pwd)/testdata" bash {{harness_setup}} -b {{pg}} -i

# Load test data into an already-initialised cluster
[group('postgres')]
pg-testdata pg=pg_version: check-harness
    TARGET_DIR="$(pwd)" TESTDATA_DIR="$(pwd)/testdata" bash {{harness_setup}} -b {{pg}} -t

# Create pgbench_test db with pgbench data (SF=1 by default; override with PGBENCH_SCALE=N or PGBENCH_DBNAME=name)
[group('postgres')]
pg-setup-pgbench pg=pg_version: check-harness
    TARGET_DIR="$(pwd)" TESTDATA_DIR="$(pwd)/testdata" bash {{harness_setup}} -b {{pg}} -p

# ── pgbackrest ────────────────────────────────────────────────────────────────

harness_backup := env_var_or_default("PG_HARNESS_DIR", "") + "/scripts/pgbackrest-backup.sh"

# Configure pgbackrest for WAL archiving
[group('backup')]
backup-setup pg=pg_version: check-harness
    TESTDATA_DIR="$(pwd)/testdata" PG_VERSION={{pg}} bash {{harness_backup}} setup

# Full backup
[group('backup')]
backup-full pg=pg_version: check-harness
    TESTDATA_DIR="$(pwd)/testdata" PG_VERSION={{pg}} bash {{harness_backup}} full

# Incremental backup
[group('backup')]
backup-incr pg=pg_version: check-harness
    TESTDATA_DIR="$(pwd)/testdata" PG_VERSION={{pg}} bash {{harness_backup}} incr

# Differential backup
[group('backup')]
backup-diff pg=pg_version: check-harness
    TESTDATA_DIR="$(pwd)/testdata" PG_VERSION={{pg}} bash {{harness_backup}} diff

# Show backup information
[group('backup')]
backup-info pg=pg_version: check-harness
    TESTDATA_DIR="$(pwd)/testdata" PG_VERSION={{pg}} bash {{harness_backup}} info

# Usage: just backup-restore /path/to/restore/dir
[group('backup')]
backup-restore target_dir pg=pg_version: check-harness
    TESTDATA_DIR="$(pwd)/testdata" PG_VERSION={{pg}} bash {{harness_backup}} restore -t {{target_dir}}

# ── Flamegraph & Profiling ────────────────────────────────────────────────────

# Flamegraph for criterion bench (requires cargo-flamegraph + perf/dtrace)
# Usage: just flamegraph-bench criterion_bench
[group('profiling')]
flamegraph-bench bench="criterion_bench":
    cargo flamegraph --bench {{bench}} -o flamegraph.svg
    open flamegraph.svg

# Flamegraph for a specific test
# Usage: just flamegraph-test test_parse_large_table
[group('profiling')]
flamegraph-test test_name:
    cargo flamegraph --test {{test_name}} -o flamegraph.svg
    open flamegraph.svg

# Flamegraph for the table_reader example
# Usage: just flamegraph-example /path/to/pgdata 16384
[group('profiling')]
flamegraph-example data_dir db_id="16384":
    cargo flamegraph --example table_reader -o flamegraph.svg -- {{data_dir}} {{db_id}}
    open flamegraph.svg

# Samply CPU profile for a bench (macOS/Linux — opens in browser)
# Usage: just samply-bench criterion_bench
[group('profiling')]
samply-bench bench="criterion_bench":
    samply record cargo bench --bench {{bench}}

# Samply CPU profile for a specific test
# Usage: just samply-test test_parse_large_table
[group('profiling')]
samply-test test_name:
    samply record cargo test --release {{test_name}} -- --nocapture

# Open the last generated flamegraph
[group('profiling')]
flamegraph-open:
    open flamegraph.svg

# ── Docs ──────────────────────────────────────────────────────────────────────

[group('docs')]
doc:
    cargo doc --open --no-deps
