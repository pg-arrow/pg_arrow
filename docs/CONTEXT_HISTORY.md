# pg_arrow — Project Context History

> Compact reference of the full project history, design evolution, and current state.
> Written 2026-02-07.

## What pg_arrow Is

pg_arrow is a **read-only sidecar process** that reads PostgreSQL data files directly and
serves analytical queries via Apache Arrow + DataFusion. It is NOT a PostgreSQL extension,
fork, or replica — it's a separate Rust binary that shares the same `$PGDATA/` directory.

**Core value prop**: 10-100x faster analytical queries by reading PostgreSQL heap files,
converting to Arrow columnar format, and executing with DataFusion's vectorized engine —
all without touching PostgreSQL's write path.

## Timeline

| Date       | Milestone                                                                                                                                                                                              |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 2025-11-18 | Project initialized (empty repo)                                                                                                                                                                       |
| 2026-01-06 | Base project setup: Cargo workspace, test infrastructure, `setup-postgres.sh` script for multi-version PG testing                                                                                      |
| 2026-01-06 | Removed requirement for init arg to load test data                                                                                                                                                     |
| 2026-01-07 | First real code: basic page header parsing test — reads an 8KB PG heap page and extracts `pd_lsn`, `pd_checksum`, `pd_flags`, `pd_lower`, `pd_upper`, `pd_special`, page size, version, `pd_prune_xid` |
| 2026-01-22 | Research phase: Arrow format fundamentals, PostgreSQL-Arrow integration analysis                                                                                                                       |
| 2026-01-27 | Research: LSM trees (for Mode 3 logical replica design)                                                                                                                                                |
| 2026-01-28 | Research: WAL, torn pages, disk reliability                                                                                                                                                            |
| 2026-02-06 | Major design document written (DESIGN.md, ~5300 lines) covering full architecture                                                                                                                      |
| 2026-02-07 | Design docs committed, DESIGN renamed from DESIGN_HYBRID_DUAL_ENGINE.md                                                                                                                                |

## Architecture (Three Deployment Modes)

```
Mode 1: Sidecar + Primary       — pg_arrow reads $PGDATA/ on same server as PG primary
Mode 2: Sidecar + Replica        — pg_arrow reads $PGDATA/ on a PG streaming replica
Mode 3: Logical Replica           — pg_arrow receives pgoutput stream, NO $PGDATA/ access
```

**Two storage layers, one query engine:**

```
SHARED: pg_arrow_server (wire protocol, Flight SQL, auth, config, lifecycle, observability)
        pg_arrow_datafusion (CatalogProvider, optimizer rules, UDFs, SQL compat)
        pg_arrow_cli (psql-like for Flight SQL)

STORAGE A: pg_arrow_core (Modes 1-2)    STORAGE B: pg_arrow_logical (Mode 3)
  Page parsing, tuple decoding             pgoutput stream consumer
  MVCC visibility, CLOG reader             Arrow write buffer + deletion bitmap
  TOAST decompression, VM reader           PK index, LSM compaction
  Segment files, Arrow page cache          Parquet checkpoint + crash recovery
  HeapFileTableProvider                    LogicalReplicaTableProvider
  Reads: PostgreSQL $PGDATA/              Reads: logical replication stream
  Stores: cache (evictable)               Stores: Arrow batches + Parquet (authoritative)
```

Both storage layers implement DataFusion's `TableProvider` → the query engine doesn't care
where data comes from. Cross-provider JOINs work transparently.

## Client Access

```
Port 5432: PostgreSQL (OLTP writes + reads)
Port 5433: pg_arrow (PostgreSQL wire protocol — OLAP reads)
Port 5434: pg_arrow (Arrow Flight SQL — columnar, zero serialization overhead)
Embedded:  pg_arrow_core library (DuckDB, Polars, Python — no server needed)
```

## Current Code State

**Minimal — early Phase 0/1.** Codebase structure:

```
src/
  lib.rs          — pub mod arrow, codec, file, util
  file/mod.rs     — get_data_dir() + test_basic_page_header test (only real code)
  arrow/mod.rs    — empty
  codec/mod.rs    — empty
  util/mod.rs     — empty
  pg_include/     — untracked, for PG header reference
```

**What works today:**

- Read `pg-test-config.toml` to find PG data directory
- Read first 8KB page from a heap file
- Parse all page header fields (LSN, checksum, flags, lower, upper, special, pagesize, version, prune_xid)
- Parse first ItemIdData
- Assert page size = 8192

**Dependencies:** serde, toml (for config parsing). No arrow-rs yet.

## Test Infrastructure

- `scripts/setup-postgres.sh`: Automates PG clone, build, initdb, test data
  - Supports multiple versions: `pg17`, `pg18`, `latest` (master)
  - Uses git worktrees for parallel versions
  - Two test schemas: simple (single type-coverage table) or full (e-commerce, 5 tables)
  - All local to `testdata/` — no system PG or root needed
- `pg-test-config.toml`: Generated config with paths to each PG version's data_dir, bin_dir, etc.
- Currently testing against pg18 (REL_18_STABLE) with test_db_created

## Key Design Decisions Made

1. **Separate process, not extension/fork** — avoids PG maintenance burden, C interop complexity
2. **`pg_arrow_core` is engine-agnostic** — any Arrow consumer (DataFusion, DuckDB, Polars, Python) can use it
3. **Mode 3 eliminates all PG storage complexity** — pgoutput detoasts, resolves MVCC, sends decoded values
4. **Three-phase MVCC implementation** — frozen-only → hint bits + CLOG → full visibility (MultiXact, subtxns, HOT)
5. **Visibility map is the #1 optimization** — skip per-tuple visibility checks for all-frozen pages
6. **Arrow page cache with LSN invalidation** — most impactful performance optimization after VM
7. **Dual protocol** — PG wire for compatibility, Flight SQL for performance (27-45x faster result transfer)
8. **Security: trusted internal service first** — pg_arrow bypasses ALL PG permissions (GRANT, RLS, column ACLs)
9. **DataFusion SQL compat covers 95%+ of analytical queries** — only PostGIS/PL/pgSQL/custom types need PG fallback

## Critical PostgreSQL Internals (Lessons Learned)

### xmax Is Overloaded

`xmax != 0` does NOT mean deleted. It can mean:

- Row locked (FOR UPDATE/SHARE) — `XMAX_LOCK_ONLY` flag
- Aborted delete — `XMAX_INVALID` flag
- MultiXact (concurrent locks) — `XMAX_IS_MULTI` flag
- Committed delete — `XMAX_COMMITTED` flag
- Unknown (no hint bits) — must check CLOG

The infomask bits determine xmax's meaning. Without them, the field is ambiguous.

### MVCC Visibility Is ~500-800 Lines

Not a simple LSN check. Requires: hint bits → CLOG lookup → snapshot `xip[]` check →
MultiXact resolution → subtransaction handling → HOT chain traversal.

### TOAST Is Mandatory

Any table with text/JSONB columns will have TOASTed values (18-byte pointers instead of data).
Without TOAST support, pg_arrow returns garbage. pgoutput (Mode 3) detoasts automatically.

### Segment Files

Tables >1GB split into `oid`, `oid.1`, `oid.2`, ... Without segment support, pg_arrow silently
reads only the first 1GB.

### Schema Evolution

After `ALTER TABLE ADD COLUMN`, old tuples have fewer columns than the schema. Must check
`tuple natts` vs `schema natts`, fill missing with `attmissingval` or NULL. `attisdropped`
columns leave holes in tuple layout.

### pg_arrow Can't Write Hint Bits

Since pg_arrow is read-only, it pays the CLOG lookup cost repeatedly for tuples without hint bits.
VACUUM/autovacuum setting hint bits progressively improves pg_arrow performance.

### Snapshot ≠ LSN

A real PG snapshot contains `xmin` (oldest active txid), `xmax` (next txid), and `xip[]`
(array of in-progress txids). Acquired via `pg_current_snapshot()`.

## Implementation Plan (14 Phases, 33-50 weeks total)

| Phase | Scope                                                                                     | Weeks   |
| ----- | ----------------------------------------------------------------------------------------- | ------- |
| 0     | Cluster validation (pg_control, segments, checksums, encoding)                            | 1-2     |
| 1     | pg_arrow_core library (page/tuple parsing, types, TOAST, VM, schema evolution)            | 4-6     |
| 2     | MVCC consistency (frozen → CLOG → full visibility, isolation levels)                      | 4-6     |
| 3     | Wire protocol (Simple Query → Extended Query → session/catalog)                           | 3-4     |
| 4     | Partitioning and parallel scan                                                            | 2-3     |
| 5     | PostgreSQL SQL compatibility (UDFs, UDAFs, generate_series, JSON)                         | 2-3     |
| 6     | Production features (caching, health monitor, DDL safety, SSL, COPY TO)                   | 3-4     |
| 7     | Advanced optimizations (BRIN, B-tree, zone maps, pg_statistic, late materialization, I/O) | ongoing |
| 8     | Security (proxy auth, permissions, audit logging)                                         | 2-3     |
| 9     | Arrow Flight SQL + pg_arrow_cli                                                           | 3-4     |
| 10    | Ecosystem integrations (Python/PyO3, DuckDB extension, Arrow C Data Interface)            | 3-5     |
| 11    | Testing infrastructure (fuzz, property, differential, chaos, stress, Miri, ClickBench)    | ongoing |
| 12    | Deployment modes + WAL synchronization (sidecar, WAL parsing, logical replica, hybrid)    | 5-8     |
| 13    | Production readiness (observability, config, lifecycle, resilience, connection mgmt)      | 3-4     |

## Next Implementation Steps (from current state)

The immediate work is Phase 0-1:

1. **ItemIdData parsing** — 4 bytes each, extract offset + length + flags
2. **HeapTupleHeaderData parsing** — t_xmin, t_xmax, t_cid, t_ctid, t_infomask, t_infomask2, t_hoff
3. **Null bitmap extraction** — variable length, 1 bit per column
4. **Fixed-width type decoders** — int2/4/8, float4/8, bool, date, timestamp
5. **Varlena parsing** — 1-byte and 4-byte headers, inline short/long values
6. **Arrow RecordBatch construction** — wire up arrow-rs dependency
7. **pg_control reader** — cluster validation (block_size, checksums, PG version, state)
8. **Segment file iteration** — support tables >1GB
9. **TOAST resolution** — read TOAST table, reassemble chunks, decompress pglz/lz4
10. **Visibility map reader** — 2 bits per page (all-visible, all-frozen)

## Research Completed

| Document                                                | Topics                                                                                            |
| ------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `RESEARCH/arrow_format.md`                              | Arrow format design, type layouts, PG→Arrow mapping, IPC format                                   |
| `RESEARCH/advanced_arrow_and_postgresql_integration.md` | 10 architectural decisions, TOAST/MVCC/HOT challenges, code examples, DuckDB/ParadeDB comparison  |
| `RESEARCH/lsm_trees_comprehensive_guide.md`             | LSM architecture, compaction strategies, RUM conjecture, RocksDB/LevelDB/WiredTiger, cloud-native |
| `RESEARCH/wal_torn_pages_disk_reliability.md`           | WAL/ARIES, torn pages, FPW, MySQL doublewrite, TigerBeetle, NVMe atomics                          |
| `RESEARCH/hyper_umbra_cedardb_systems.md`               | HyPer/Umbra/CedarDB query compilation                                                             |
| `RESEARCH/kafka_internals.md`                           | Kafka log-structured storage (inspiration for Mode 3)                                             |
| `RESEARCH/mongodb_wiredtiger_internals.md`              | WiredTiger storage engine internals                                                               |
| `RESEARCH/filesystem_design_expert_guide.md`            | Filesystem design for database storage                                                            |

## Other Design Documents

| Document                                    | Purpose                                                                                                                                                                                                                             |
| ------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `docs/design/DESIGN.md`                     | Main design doc (~5300 lines) — the single source of truth                                                                                                                                                                          |
| `docs/design/DESIGN_CONSISTENCY_PROBLEM.md` | Analysis of read consistency when PG is writing (buffer cache vs disk mismatch, torn pages, MVCC, WAL-ahead). 4 solutions: snapshot isolation, WAL replay, physical standby, shared memory. Recommends snapshot isolation (Phase 1) |
| `docs/design/DESIGN_REALTIME_ANALYTICS.md`  | Real-time analytics architecture: bootstrap from data files → continuous WAL sync → Arrow tables. DataFusion integration via IPC files, Arrow Flight, or embedded in-process. LSN-based replication lag tracking                    |

## Comparable Systems

| System                            | Approach                                | How pg_arrow differs                                                 |
| --------------------------------- | --------------------------------------- | -------------------------------------------------------------------- |
| DuckDB `postgres_scanner`         | Protocol-based (SQL connection to PG)   | pg_arrow reads heap files directly — no PG query overhead            |
| ParadeDB `pg_analytics`           | PG extension (shared_preload_libraries) | pg_arrow is external — no C interop, no PG release coupling          |
| Citus                             | PG extension for distributed queries    | pg_arrow is single-instance, read-only, columnar-optimized           |
| ClickHouse MaterializedPostgreSQL | Logical replication to ClickHouse       | pg_arrow Mode 3 is similar but Arrow-native, DataFusion query engine |
