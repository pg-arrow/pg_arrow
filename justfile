# pg_arrow justfile
# Usage: just <recipe>   (run from pg_arrow/)
# Requires: https://github.com/casey/just

pg_version := env_var_or_default("PG_VERSION", "pg18")

# ── Default ───────────────────────────────────────────────────────────────────

default:
    @just --list

# ── Build ─────────────────────────────────────────────────────────────────────

# Debug build
build:
    cargo build

# Release build
release:
    cargo build --release

# ── Lint & Format ─────────────────────────────────────────────────────────────

fmt:
    cargo fmt

fmt-check:
    cargo fmt --check

clippy:
    cargo clippy -- -D warnings

# ── Tests ─────────────────────────────────────────────────────────────────────

test:
    cargo test

# ── Benchmarks ────────────────────────────────────────────────────────────────

# Criterion statistical benchmarks (optional filter regex)
bench filter="":
    cargo bench --bench criterion_bench -- {{filter}}

# iai instruction-count benchmarks
bench-iai:
    cargo bench --bench iai_bench

# File I/O latency benchmarks
bench-io:
    cargo bench --bench file_read_latency

# Run all benchmarks
bench-all: bench bench-iai bench-io

# ── Examples ──────────────────────────────────────────────────────────────────

# Run the table_reader example
# Usage: just example-table-reader /path/to/pgdata 16384
example-table-reader data_dir db_id="16384":
    cargo run --example table_reader -- {{data_dir}} {{db_id}}

# ── PostgreSQL Setup ──────────────────────────────────────────────────────────

# Full setup: build from source, init cluster, load test data
# Usage: just pg-setup pg18   (or pg17 / latest)
pg-setup pg=pg_version:
    bash scripts/setup-postgres.sh -b {{pg}} -B -i -t

# Full setup with simple schema (no pgbench tables)
pg-setup-simple pg=pg_version:
    bash scripts/setup-postgres.sh -b {{pg}} -B -i -t -s

# Build PostgreSQL source only
pg-build pg=pg_version:
    bash scripts/setup-postgres.sh -b {{pg}} -B

# Init cluster only (source must already be built)
pg-init pg=pg_version:
    bash scripts/setup-postgres.sh -b {{pg}} -i

# Load test data into an already-initialised cluster
pg-testdata pg=pg_version:
    bash scripts/setup-postgres.sh -b {{pg}} -t

# ── pgbackrest ────────────────────────────────────────────────────────────────

# Configure pgbackrest for WAL archiving
backup-setup:
    bash scripts/pgbackrest-backup.sh setup

# Full backup
backup-full:
    bash scripts/pgbackrest-backup.sh full

# Incremental backup
backup-incr:
    bash scripts/pgbackrest-backup.sh incr

# Differential backup
backup-diff:
    bash scripts/pgbackrest-backup.sh diff

# Show backup information
backup-info:
    bash scripts/pgbackrest-backup.sh info

# Restore from backup
# Usage: just backup-restore /path/to/restore/dir
backup-restore target_dir:
    bash scripts/pgbackrest-backup.sh restore -t {{target_dir}}

# ── Flamegraph & Profiling ────────────────────────────────────────────────────

# Flamegraph for criterion bench (requires cargo-flamegraph + perf/dtrace)
# Usage: just flamegraph-bench criterion_bench
flamegraph-bench bench="criterion_bench":
    cargo flamegraph --bench {{bench}} -o flamegraph.svg
    open flamegraph.svg

# Flamegraph for a specific test
# Usage: just flamegraph-test test_parse_large_table
flamegraph-test test_name:
    cargo flamegraph --test {{test_name}} -o flamegraph.svg
    open flamegraph.svg

# Flamegraph for the table_reader example
# Usage: just flamegraph-example /path/to/pgdata 16384
flamegraph-example data_dir db_id="16384":
    cargo flamegraph --example table_reader -o flamegraph.svg -- {{data_dir}} {{db_id}}
    open flamegraph.svg

# Samply CPU profile for a bench (macOS/Linux — opens in browser)
# Usage: just samply-bench criterion_bench
samply-bench bench="criterion_bench":
    samply record cargo bench --bench {{bench}}

# Samply CPU profile for a specific test
# Usage: just samply-test test_parse_large_table
samply-test test_name:
    samply record cargo test --release {{test_name}} -- --nocapture

# Open the last generated flamegraph
flamegraph-open:
    open flamegraph.svg

# ── Docs ──────────────────────────────────────────────────────────────────────

doc:
    cargo doc --open --no-deps
