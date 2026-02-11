# Hybrid Dual-Engine Architecture: PostgreSQL + pg_arrow DataFusion

> **Last updated**: 2026-02-11
>
> **Changelog**:
>
> - 2026-02-12: Added "TPC-H Benchmarking" section (8-table join benchmark with schema,
>   setup via tpchgen-rs, 22-query classification, partitioning strategy, performance profile).
>   Added "CH-benCHmark (HTAP Benchmarking)" section for concurrent OLTP+OLAP testing
>   (architecture diagram, BenchBase/go-tpc setup, freshness measurement, metrics).
>   Updated implementation plan with TPC-H and CH-benCHmark checklist items.
> - 2026-02-11: Added "Read Consistency for Direct Heap File Access" section. Documents the two
>   fundamental consistency problems for direct heap file reads (shared buffer lag — committed data
>   missing from disk, and cross-page inconsistency — pages at different LSNs) and the WAL replay
>   solution: read `pd_lsn` from each page header, replay WAL records from `pd_lsn..target_lsn` to
>   bring all pages to a single consistent point. Covers: single page atomicity (safe), target LSN
>   selection (`pg_current_wal_flush_lsn()` / `pg_last_wal_replay_lsn()`), WAL record types for heap
>   pages (RM_HEAP_ID, RM_HEAP2_ID), Full Page Image (FPI) optimization, complete read pipeline,
>   Mode 2 advantage (paused replay = zero WAL replay needed), practical cost analysis, WAL parsing
>   complexity (~2000-3000 lines), and consistency tiers (Tier 0: checkpoint-bound, Tier 1: MVCC-only,
>   Tier 2: full WAL replay, Tier 3: paused replica). Added "WAL File Physical Format" section
>   summarizing WAL file organization, LSN arithmetic, page/record headers, block references,
>   FPI restoration, heap record types, scanning algorithm, continuation records, and version
>   handling (PG14-master). Full 1900-line implementation reference at `RESEARCH/WAL_FORMAT.md`
>   with Rust struct definitions, all constants, complete decoding algorithm, and version-specific
>   code. Updated "File Access Patterns" section to reflect WAL replay requirement. Added
>   limitation #15 (shared buffer lag). Updated Phase 2 with step 2d (WAL replay for Tier 2
>   consistency, shared infrastructure with Phase 12b).
> - 2026-02-06: Architectural clarity — pg_arrow has two storage layers behind a shared query
>   engine. Storage Layer A (`pg_arrow_core`, Modes 1-2) reads PostgreSQL heap files. Storage
>   Layer B (`pg_arrow_logical`, Mode 3) is an entirely new Arrow-native columnar database: base
>   Parquet checkpoint + continuous logical replication apply with LSM-style compaction. Both
>   implement `TableProvider` — DataFusion, wire protocol, Flight SQL, observability are all shared.
>   Updated workspace layout to add `pg_arrow_logical` crate. Updated layered architecture diagram.
> - 2026-02-06: Merged content from DESIGN_ZERO_COPY_REPLICA.md (now deleted) into new "Deployment
>   Modes and WAL Synchronization" section. Three deployment modes: Sidecar+Primary (Mode 1),
>   Sidecar+Promotable Replica (Mode 2, PG handles promotion, pg_arrow keeps reading), Logical
>   Replica standalone or sidecar (Mode 3, no $PGDATA/ needed). Added 4-level WAL sync progression: Level 1
>   (recovery LSN sync — pause replay for torn-page-free reads), Level 2 (physical WAL stream
>   parsing for surgical per-page cache invalidation + full-page image extraction), Level 3
>   (logical replication for incremental Arrow maintenance — eliminates MVCC complexity entirely),
>   Level 4 (hybrid strategy per table based on query frequency). Added deployment topologies
>   (tiered standby with failback), promotion reality check (need ~70% of PostgreSQL), and
>   alternative approaches (PostgreSQL extension/fork like ParadeDB). Also added production gaps:
>   observability (Prometheus metrics, OpenTelemetry tracing, structured logging, health endpoints),
>   pg_arrow configuration management, graceful lifecycle (signals, drain, startup/shutdown),
>   error handling / resilience (degraded mode, circuit breakers), connection management (limits,
>   timeouts, backpressure), collation handling (sort order correctness), schema evolution
>   (ALTER TABLE ADD/DROP COLUMN, attisdropped), multi-database support, and replica/standby reading.
> - 2026-02-06: Added comprehensive "Testing and Validation Strategy" section covering 12 test types:
>   fuzz testing (page headers, tuple decoding, TOAST decompression, wire protocol), property-based
>   testing with proptest (round-trip invariants, page invariants, MVCC properties), differential
>   testing against PostgreSQL (pageinspect ground truth, full table scan diffing, regression suite
>   SELECT extraction), chaos/fault injection (torn pages, zero pages, concurrent VACUUM FULL,
>   corrupt item pointers, missing CLOG files, VM disagreements), concurrency/stress testing
>   (concurrent read+write, VACUUM FULL during scan, 100 parallel queries), memory safety (Miri,
>   AddressSanitizer, MemorySanitizer, ThreadSanitizer), snapshot testing with insta, mutation
>   testing with cargo-mutants, cross-version compatibility (pg17/pg18/latest), and code coverage.
>   Added "ClickBench Benchmarking" section with setup instructions, benchmark harness design,
>   expected performance profile, and comparison targets (PostgreSQL, DuckDB, ClickHouse). Added
>   Phase 11 (Testing Infrastructure) to implementation plan.
> - 2026-02-06: Major architectural evolution — pg_arrow split into reusable library (`pg_arrow_core`)
>   and engine-specific integrations. `pg_arrow_core` is engine-agnostic: any Arrow-compatible engine
>   (DataFusion, DuckDB, Polars, Python/PyArrow) can consume it via `Iterator<RecordBatch>` or Arrow C
>   Data Interface. Added sections: "Library Architecture and Crate Structure", "Arrow Flight and ADBC
>   Protocol", "Arrow-Native Optimizations", "PostgreSQL Index Reuse", "Incremental Arrow Page Cache",
>   "I/O Optimizations", "DataFusion Engine Integration". With DataFusion expertise, we can make upstream
>   changes for pg_arrow-specific optimizations (custom CatalogProvider, statistics from pg_statistic,
>   page-level predicate pushdown, adaptive scan strategy, memory pool integration). Updated architecture
>   diagram and implementation plan with Phase 9-10.
> - 2026-02-06: Added "Isolation Levels", "Security Model", "Configuration and Cluster Validation",
>   "Segment Files", "Torn Page Detection", "Database Encoding", "Concurrent DDL Safety",
>   "PostgreSQL Background Processes", "pg_arrow Background Jobs", and "PostgreSQL Features — Considered
>   and Excluded" sections. Major gaps identified: segment files (tables >1GB broken without it),
>   security model (pg_arrow bypasses ALL PostgreSQL permissions including RLS), isolation level
>   semantics (READ COMMITTED vs REPEATABLE READ snapshot behavior), pg_control validation (block_size,
>   checksums, version), and database encoding transcoding. Updated implementation plan with Phase 0
>   (cluster validation) and Phase 8 (security). Total revised to 21-31 weeks.
> - 2026-02-06: Added "Physical Storage Features", "Partitioning", and "PostgreSQL Protocol Compatibility"
>   sections. TOAST is critical — any table with text/JSONB columns is broken without it. Partitioning is
>   a major performance opportunity (parallel scan, partition pruning). The visibility map (`_vm` fork)
>   enables skipping per-tuple visibility checks for frozen pages. Wire protocol compatibility (especially
>   Extended Query Protocol) is required for any client beyond `psql`. Updated implementation plan with
>   Phases 6-7 and restructured Phase 1 to include TOAST.
> - 2026-02-06: Added "PostgreSQL SQL Compatibility via DataFusion Extensions" section. Initial analysis
>   overstated the gap between DataFusion and PostgreSQL SQL — DataFusion's extension APIs (ScalarUDF,
>   AggregateUDF, WindowUDF, TableFunction, OptimizerRule, PostgreSQL SQL dialect) cover the vast majority
>   of analytical query patterns. Only extension-dependent features (PostGIS, PL/pgSQL, custom types) truly
>   require fallback to PostgreSQL. Added Phase 5 to implementation plan.
> - 2026-02-06: Added "xmax Is Overloaded" section. `xmax != 0` does not mean "deleted" — it can mean
>   row-locked, aborted delete, or MultiXact. The original sketch (`if xmax != 0 { return false }`) would
>   incorrectly hide every row that was ever locked or had a failed delete. The infomask bits are what give
>   `xmax` its meaning; without them the field is ambiguous.
> - 2026-02-06: Added detailed MVCC visibility rules analysis (see "MVCC Visibility: The Real Complexity").
>   The original `is_tuple_visible` sketch was a massive oversimplification — checking only LSN and xmax.
>   A deeper review of PostgreSQL's `heapam_visibility.c` revealed the full scope: hint bits, CLOG lookups,
>   MultiXact resolution, subtransaction handling, snapshot `xip[]` arrays, and HOT chains.
>   Updated the implementation plan and limitations accordingly.

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Key Benefits ✅](#key-benefits-)
- [How It Works](#how-it-works)
  - [1. PostgreSQL: Write Path (Unchanged)](#1-postgresql-write-path-unchanged)
  - [2. pg_arrow: Read Path (DataFusion)](#2-pg_arrow-read-path-datafusion)
  - [3. Coordination: WAL Monitoring](#3-coordination-wal-monitoring)
- [Implementation Details](#implementation-details)
  - [PostgreSQL Table Provider (Zero-Copy)](#postgresql-table-provider-zero-copy)
- [Reading Strategy: When to Read Data Files](#reading-strategy-when-to-read-data-files)
- [Connection Routing: Smart Client](#connection-routing-smart-client)
- [Deployment Example](#deployment-example)
  - [Configuration](#configuration)
- [Failover Scenario](#failover-scenario)
- [File Access Patterns](#file-access-patterns)
  - [PostgreSQL (Read-Write)](#postgresql-read-write)
  - [pg_arrow (Read-Only)](#pg_arrow-read-only)
- [Read Consistency for Direct Heap File Access (Modes 1 & 2)](#read-consistency-for-direct-heap-file-access-modes-1--2)
  - [The Two Consistency Problems](#the-two-consistency-problems)
  - [Single Page Atomicity — Not the Real Problem](#single-page-atomicity--not-the-real-problem)
  - [Why MVCC Alone Doesn't Fully Solve This](#why-mvcc-alone-doesnt-fully-solve-this)
  - [The Solution: WAL Replay to Target LSN](#the-solution-wal-replay-to-target-lsn)
  - [Choosing target_lsn](#choosing-target_lsn)
  - [WAL Record Types for Heap Pages](#wal-record-types-for-heap-pages)
  - [Full Page Images (FPI) — A Major Optimization](#full-page-images-fpi--a-major-optimization)
  - [The Complete Read Pipeline](#the-complete-read-pipeline)
  - [Mode 2 Advantage: Zero WAL Replay](#mode-2-advantage-zero-wal-replay)
  - [Practical Cost of WAL Replay](#practical-cost-of-wal-replay)
  - [WAL Parsing Complexity](#wal-parsing-complexity)
  - [Consistency Tiers — Tradeoffs](#consistency-tiers--tradeoffs)
- [MVCC Consistency](#mvcc-consistency)
- [MVCC Visibility: The Real Complexity](#mvcc-visibility-the-real-complexity)
  - [Tuple Header MVCC Fields](#tuple-header-mvcc-fields)
  - [Infomask Bits Relevant to Visibility](#infomask-bits-relevant-to-visibility)
  - [xmax Is Overloaded — It Does NOT Mean "Deleted"](#xmax-is-overloaded--it-does-not-mean-deleted)
  - [What a Snapshot Really Is](#what-a-snapshot-really-is)
  - [The Real Visibility Algorithm (HeapTupleSatisfiesMVCC)](#the-real-visibility-algorithm-heaptuplesatisfiesmvcc)
  - [External File Dependencies](#external-file-dependencies)
  - [Implications for pg_arrow](#implications-for-pg_arrow)
  - [Practical Phased Approach for pg_arrow](#practical-phased-approach-for-pg_arrow)
- [Performance Comparison](#performance-comparison)
  - [Write Path (PostgreSQL Only)](#write-path-postgresql-only)
  - [Read Path (Analytical Queries)](#read-path-analytical-queries)
  - [Read Path (OLTP Queries)](#read-path-oltp-queries)
- [Caching Strategy (Optional Optimization)](#caching-strategy-optional-optimization)
- [Key Advantages of This Approach](#key-advantages-of-this-approach)
- [PostgreSQL SQL Compatibility via DataFusion Extensions](#postgresql-sql-compatibility-via-datafusion-extensions)
  - [DataFusion Extension Points](#datafusion-extension-points)
  - [Already Supported by DataFusion (No Work Needed)](#already-supported-by-datafusion-no-work-needed)
  - [Implementable via UDF/UDAF Registration (~50-200 lines each)](#implementable-via-udfudaf-registration-50-200-lines-each)
  - [JSON Support via datafusion-functions-json](#json-support-via-datafusion-functions-json)
  - [Capability-Based Query Routing](#capability-based-query-routing)
  - [What Actually Requires PostgreSQL Fallback](#what-actually-requires-postgresql-fallback)
- [Physical Storage Features](#physical-storage-features)
  - [TOAST (The Oversized-Attribute Storage Technique) — CRITICAL](#toast-the-oversized-attribute-storage-technique--critical)
  - [Visibility Map (`_vm` fork) — Major Optimization](#visibility-map-_vm-fork--major-optimization)
  - [Free Space Map (`_fsm` fork) — Not Needed](#free-space-map-_fsm-fork--not-needed)
  - [Tablespaces — Follow Symlinks](#tablespaces--follow-symlinks)
  - [Unlogged Tables — No WAL](#unlogged-tables--no-wal)
  - [Views and Materialized Views](#views-and-materialized-views)
  - [Large Objects — Ignore](#large-objects--ignore)
- [Partitioning — A Major Opportunity for pg_arrow](#partitioning--a-major-opportunity-for-pg_arrow)
  - [Physical Layout](#physical-layout)
  - [Partition Pruning](#partition-pruning)
  - [Parallel Scan Across Partitions](#parallel-scan-across-partitions)
  - [Table Inheritance (Legacy Partitioning)](#table-inheritance-legacy-partitioning)
  - [Sharding — Out of Scope](#sharding--out-of-scope)
- [PostgreSQL Protocol Compatibility](#postgresql-protocol-compatibility)
  - [Wire Protocol Layers](#wire-protocol-layers)
  - [Type OID Mapping](#type-oid-mapping)
  - [Catalog and Session Queries](#catalog-and-session-queries)
  - [Transaction Commands](#transaction-commands)
  - [Error Protocol](#error-protocol)
  - [Compatibility Tiers Summary](#compatibility-tiers-summary)
- [Isolation Levels](#isolation-levels)
  - [Why This Matters](#why-this-matters)
  - [Per-Connection Snapshot Tracking](#per-connection-snapshot-tracking)
  - [Snapshot Acquisition](#snapshot-acquisition)
- [Security Model](#security-model)
  - [What pg_arrow Bypasses](#what-pg_arrow-bypasses)
  - [The RLS Problem — Example](#the-rls-problem--example)
  - [Security Model Options](#security-model-options)
  - [Recommended Approach](#recommended-approach)
  - [Audit Logging](#audit-logging)
- [Configuration and Cluster Validation](#configuration-and-cluster-validation)
  - [pg_control — Read on Startup (CRITICAL)](#pg_control--read-on-startup-critical)
  - [PostgreSQL Settings — Read via Connection](#postgresql-settings--read-via-connection)
  - [Configuration File Structure](#configuration-file-structure)
- [Segment Files](#segment-files)
- [Torn Page Detection](#torn-page-detection)
  - [Detection via Data Checksums](#detection-via-data-checksums)
  - [Without Checksums](#without-checksums)
  - [PostgreSQL's Full-Page Writes](#postgresqls-full-page-writes)
- [WAL File Physical Format](#wal-file-physical-format)
  - [WAL File Organization](#wal-file-organization)
  - [LSN Arithmetic](#lsn-arithmetic)
  - [WAL Page Header](#wal-page-header)
  - [WAL Record Header (XLogRecord — 24 bytes)](#wal-record-header-xlogrecord--24-bytes)
  - [Record Structure — Two-Phase Layout](#record-structure--two-phase-layout)
  - [Block Reference Header](#block-reference-header)
  - [Heap WAL Record Types (RM_HEAP_ID = 10, RM_HEAP2_ID = 9)](#heap-wal-record-types-rm_heap_id--10-rm_heap2_id--9)
  - [Full Page Images (FPI)](#full-page-images-fpi)
  - [Scanning WAL for a Specific Page](#scanning-wal-for-a-specific-page)
  - [Continuation Records](#continuation-records)
  - [WAL Parsing Complexity and Version Handling](#wal-parsing-complexity-and-version-handling)
- [Database Encoding](#database-encoding)
  - [Encoding Handling](#encoding-handling)
  - [Collation](#collation)
- [Concurrent DDL Safety](#concurrent-ddl-safety)
  - [Detection Strategy](#detection-strategy)
  - [Unix File Descriptor Semantics](#unix-file-descriptor-semantics)
- [PostgreSQL Background Processes](#postgresql-background-processes)
  - [Processes That Modify Data Files](#processes-that-modify-data-files)
  - [Processes That Don't Modify Data Files](#processes-that-dont-modify-data-files)
  - [autovacuum Is pg_arrow's Best Friend](#autovacuum-is-pg_arrows-best-friend)
- [pg_arrow Background Jobs](#pg_arrow-background-jobs)
  - [1. Cluster Health Monitor](#1-cluster-health-monitor)
  - [2. WAL Position Monitor](#2-wal-position-monitor-already-in-design-doc-refined)
  - [3. Schema Cache Manager](#3-schema-cache-manager)
  - [4. Visibility Map Monitor](#4-visibility-map-monitor)
  - [5. pg_arrow Statistics Collector](#5-pg_arrow-statistics-collector)
  - [6. PostgreSQL Connection Pool](#6-postgresql-connection-pool)
  - [7. Warm-Up / Pre-Fetch (Optional)](#7-warm-up--pre-fetch-optional)
- [PostgreSQL Features — Considered and Excluded](#postgresql-features--considered-and-excluded)
  - [Excluded: Not Relevant to Read-Only Analytics](#excluded-not-relevant-to-read-only-analytics)
  - [Excluded: PostgreSQL Internal Subsystems](#excluded-postgresql-internal-subsystems)
  - [Excluded: Storage Features Not Needed for Analytics](#excluded-storage-features-not-needed-for-analytics)
  - [Partially Relevant: May Implement Later](#partially-relevant-may-implement-later)
- [Library Architecture and Crate Structure](#library-architecture-and-crate-structure)
  - [Layered Architecture](#layered-architecture)
  - [Workspace Layout](#workspace-layout)
  - [Core Library Public API](#core-library-public-api)
  - [Consumer Integration Examples](#consumer-integration-examples)
  - [Why This Separation Matters](#why-this-separation-matters)
- [Arrow Flight and ADBC Protocol](#arrow-flight-and-adbc-protocol)
  - [The Row Conversion Problem](#the-row-conversion-problem)
  - [Dual-Protocol Architecture](#dual-protocol-architecture)
  - [Arrow Flight SQL Server](#arrow-flight-sql-server)
  - [pg_arrow_cli — psql-like for Arrow Flight SQL](#pg_arrow_cli--psql-like-for-arrow-flight-sql)
  - [ADBC (Arrow Database Connectivity)](#adbc-arrow-database-connectivity)
- [Arrow-Native Optimizations](#arrow-native-optimizations)
  - [Late Materialization](#late-materialization)
  - [Dictionary Encoding for Low-Cardinality Columns](#dictionary-encoding-for-low-cardinality-columns)
  - [Vectorized SIMD Filtering](#vectorized-simd-filtering)
  - [RecordBatch Size Tuning](#recordbatch-size-tuning)
  - [Zero-Copy Slicing for LIMIT](#zero-copy-slicing-for-limit)
- [PostgreSQL Index Reuse](#postgresql-index-reuse)
  - [BRIN Index Reading — Best Bang for Buck](#brin-index-reading--best-bang-for-buck)
  - [B-tree Index Reading — Targeted Row Lookup](#b-tree-index-reading--targeted-row-lookup)
  - [Self-Built Zone Maps](#self-built-zone-maps)
- [Incremental Arrow Page Cache](#incremental-arrow-page-cache)
  - [Page-Level Arrow Cache](#page-level-arrow-cache)
  - [Three-Level Invalidation (Cheapest First)](#three-level-invalidation-cheapest-first)
  - [Column-Level Cache (Finer Granularity)](#column-level-cache-finer-granularity)
  - [Background Pre-Conversion](#background-pre-conversion)
  - [Persistent Cache (Survives Restarts)](#persistent-cache-survives-restarts)
- [I/O Optimizations](#io-optimizations)
  - [Memory-Mapped I/O](#memory-mapped-io)
  - [io_uring Async I/O (Linux)](#io_uring-async-io-linux)
  - [Readahead Hints](#readahead-hints)
  - [Batched CLOG Lookups](#batched-clog-lookups)
  - [Parallel Page Conversion](#parallel-page-conversion)
  - [Pipeline: Read → Parse → Convert → Execute](#pipeline-read--parse--convert--execute)
- [DataFusion Engine Integration](#datafusion-engine-integration)
  - [Custom CatalogProvider (Upstreamable)](#custom-catalogprovider-upstreamable)
  - [Statistics from pg_statistic (Upstreamable)](#statistics-from-pg_statistic-upstreamable)
  - [Custom ExecutionPlan with Multi-Level Filtering](#custom-executionplan-with-multi-level-filtering)
  - [Custom OptimizerRule: Adaptive Scan Strategy](#custom-optimizerrule-adaptive-scan-strategy)
  - [Custom OptimizerRule: PostgreSQL Fallback](#custom-optimizerrule-postgresql-fallback)
  - [Memory Pool Integration (DataFusion Core Change)](#memory-pool-integration-datafusion-core-change)
  - [Optimization Stack Summary](#optimization-stack-summary)
- [Deployment Modes and WAL Synchronization](#deployment-modes-and-wal-synchronization)
  - [Three Deployment Modes](#three-deployment-modes)
  - [Mode Comparison](#mode-comparison)
  - [Mode 1: Sidecar + Primary — Details](#mode-1-sidecar--primary--details)
  - [Mode 2: Sidecar + Promotable Replica — Details](#mode-2-sidecar--promotable-replica--details)
  - [Mode 3: Logical Replica — Details](#mode-3-logical-replica--details)
  - [WAL Synchronization Levels (Modes 1 & 2 only)](#wal-synchronization-levels-modes-1--2-only)
  - [Alternative Architectures (Considered)](#alternative-architectures-considered)
- [Production Readiness](#production-readiness)
  - [Observability](#observability)
  - [pg_arrow Configuration](#pg_arrow-configuration)
  - [Graceful Lifecycle Management](#graceful-lifecycle-management)
  - [Error Handling and Resilience](#error-handling-and-resilience)
  - [Connection Management](#connection-management)
  - [Collation Handling](#collation-handling)
  - [Schema Evolution (ALTER TABLE Handling)](#schema-evolution-alter-table-handling)
  - [Multi-Database Support](#multi-database-support)
  - [Extension Type Handling](#extension-type-handling)
  - [Numeric Precision Edge Cases](#numeric-precision-edge-cases)
- [Testing and Validation Strategy](#testing-and-validation-strategy)
  - [Fuzz Testing](#fuzz-testing)
  - [Property-Based Testing](#property-based-testing)
  - [Differential Testing Against PostgreSQL](#differential-testing-against-postgresql)
  - [MVCC Visibility Validation](#mvcc-visibility-validation)
  - [Test Data Generation](#test-data-generation)
  - [Chaos / Fault Injection Testing](#chaos--fault-injection-testing)
  - [Concurrency / Stress Testing](#concurrency--stress-testing)
  - [Memory Safety: Miri and Sanitizers](#memory-safety-miri-and-sanitizers)
  - [Snapshot Testing](#snapshot-testing)
  - [Mutation Testing](#mutation-testing)
  - [Cross-Version Compatibility Testing](#cross-version-compatibility-testing)
  - [Code Coverage](#code-coverage)
  - [Testing Matrix Summary](#testing-matrix-summary)
- [ClickBench Benchmarking](#clickbench-benchmarking)
  - [Setup](#setup)
  - [Benchmark Harness](#benchmark-harness)
  - [Benchmark Script](#benchmark-script)
  - [What ClickBench Measures](#what-clickbench-measures)
  - [Expected Performance Profile](#expected-performance-profile)
  - [Comparison Targets](#comparison-targets)
- [TPC-H Benchmarking](#tpc-h-benchmarking)
  - [Schema Overview](#schema-overview)
  - [Setup](#setup-1)
  - [Query Classification](#query-classification)
  - [What TPC-H Stresses in pg_arrow](#what-tpc-h-stresses-in-pg_arrow)
  - [Benchmark Harness](#benchmark-harness-1)
  - [Expected Performance Profile](#expected-performance-profile-1)
  - [Comparison Targets](#comparison-targets-1)
- [CH-benCHmark (HTAP Benchmarking)](#ch-benchmark-htap-benchmarking)
  - [Architecture](#architecture)
  - [Schema Mapping](#schema-mapping)
  - [Setup](#setup-2)
  - [Key Metrics](#key-metrics)
  - [Freshness Measurement](#freshness-measurement)
  - [Expected Performance Profile](#expected-performance-profile-2)
  - [Other HTAP Benchmarks](#other-htap-benchmarks)
- [Limitations](#limitations)
- [Recommended Implementation Plan](#recommended-implementation-plan)
  - [Phase 0: Cluster Validation and Foundation (1-2 weeks)](#phase-0-cluster-validation-and-foundation-1-2-weeks)
  - [Phase 1: pg_arrow_core Library (4-6 weeks)](#phase-1-pg_arrow_core-library-4-6-weeks)
  - [Phase 2: MVCC Consistency (4-6 weeks)](#phase-2-mvcc-consistency-4-6-weeks)
  - [Phase 3: Wire Protocol (3-4 weeks)](#phase-3-wire-protocol-3-4-weeks)
  - [Phase 4: Partitioning and Parallel Scan (2-3 weeks)](#phase-4-partitioning-and-parallel-scan-2-3-weeks)
  - [Phase 5: PostgreSQL SQL Compatibility (2-3 weeks)](#phase-5-postgresql-sql-compatibility-2-3-weeks)
  - [Phase 6: Production Features (3-4 weeks)](#phase-6-production-features-3-4-weeks)
  - [Phase 7: Advanced Features and Optimizations (ongoing)](#phase-7-advanced-features-and-optimizations-ongoing)
  - [Phase 8: Security (2-3 weeks)](#phase-8-security-2-3-weeks)
  - [Phase 9: Arrow Flight SQL and pg_arrow_cli (3-4 weeks)](#phase-9-arrow-flight-sql-and-pg_arrow_cli-3-4-weeks)
  - [Phase 10: Ecosystem Integrations (3-5 weeks)](#phase-10-ecosystem-integrations-3-5-weeks)
  - [Phase 11: Testing Infrastructure (ongoing, parallel with all phases)](#phase-11-testing-infrastructure-ongoing-parallel-with-all-phases)
  - [Phase 12: Deployment Modes and WAL Synchronization (5-8 weeks)](#phase-12-deployment-modes-and-wal-synchronization-5-8-weeks)
  - [Phase 13: Production Readiness (3-4 weeks)](#phase-13-production-readiness-3-4-weeks)

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────┐
│ Single Server                                                   │
│                                                                 │
│  ┌───────────────────────┐      ┌──────────────────────────┐  │
│  │  PostgreSQL Server    │      │  pg_arrow DataFusion     │  │
│  │  (Port 5432)          │      │  (Port 5433)             │  │
│  │                       │      │                          │  │
│  │  - Handles WRITES     │      │  - Handles READS         │  │
│  │  - Maintains WAL      │      │  - DataFusion executor   │  │
│  │  - MVCC/Transactions  │      │  - OLAP optimization     │  │
│  │  - Can handle reads   │      │  - Read-only             │  │
│  └───────────┬───────────┘      └──────────┬───────────────┘  │
│              │                             │                   │
│              │     SHARED DATA FILES       │                   │
│              └──────────────┬──────────────┘                   │
│                             ▼                                   │
│                    ┌─────────────────┐                         │
│                    │  Data Directory  │                         │
│                    │  /pg/data/       │                         │
│                    │  ├─ base/        │ ← Both read from here  │
│                    │  ├─ pg_wal/      │ ← PostgreSQL writes    │
│                    │  └─ global/      │   pg_arrow monitors    │
│                    └─────────────────┘                         │
└─────────────────────────────────────────────────────────────────┘

Client Applications:
  ├─ Writes          → Connect to PostgreSQL :5432
  ├─ Analytics (SQL)  → Connect to pg_arrow  :5433  (PostgreSQL wire protocol)
  ├─ Analytics (Fast) → Connect to pg_arrow  :5434  (Arrow Flight SQL — columnar)
  └─ Embedded         → pg_arrow_core library (DuckDB, Polars, Python — no server)
```

## Key Benefits ✅

1. ✅ **Zero data duplication** - Both engines read same files
2. ✅ **Promotable** - PostgreSQL is real, can become primary
3. ✅ **Fast analytics** - DataFusion for OLAP queries
4. ✅ **Write safety** - PostgreSQL handles all writes correctly
5. ✅ **Simple failover** - Just promote PostgreSQL instance
6. ✅ **Drop-in** - Add pg_arrow without changing PostgreSQL

## How It Works

### 1. PostgreSQL: Write Path (Unchanged)

```sql
-- Client connects to PostgreSQL (port 5432)
psql -h localhost -p 5432 -d mydb

-- All writes go through PostgreSQL
INSERT INTO orders (user_id, amount) VALUES (123, 99.99);
UPDATE users SET email = 'new@email.com' WHERE id = 123;
DELETE FROM logs WHERE created_at < NOW() - INTERVAL '7 days';

-- PostgreSQL:
-- 1. Writes to WAL
-- 2. Updates pages in buffer cache
-- 3. Background writer flushes to data files
-- 4. Everything works normally
```

**PostgreSQL does what it does best**: ACID transactions, writes, WAL management

### 2. pg_arrow: Read Path (DataFusion)

```sql
-- Client connects to pg_arrow (port 5433)
psql -h localhost -p 5433 -d mydb

-- Analytical queries go to pg_arrow
SELECT
    product_id,
    DATE_TRUNC('day', created_at) as day,
    SUM(amount) as daily_revenue
FROM orders
WHERE created_at > NOW() - INTERVAL '30 days'
GROUP BY product_id, day
ORDER BY daily_revenue DESC
LIMIT 100;

-- pg_arrow:
-- 1. Reads PostgreSQL heap files directly
-- 2. Converts to Arrow in-memory
-- 3. Executes with DataFusion (vectorized, columnar)
-- 4. Returns results (10-100x faster than PostgreSQL)
```

**pg_arrow does what it does best**: Fast analytical queries on columnar data

### 3. Coordination: WAL Monitoring

```rust
struct PgArrowEngine {
    // PostgreSQL data directory (shared, read-only access)
    pg_data_dir: PathBuf,

    // WAL position tracking
    last_read_lsn: LSN,

    // Optional: Cache for hot tables
    table_cache: HashMap<u32, CachedTableMetadata>,

    // DataFusion context
    datafusion_ctx: SessionContext,
}

impl PgArrowEngine {
    async fn run(&mut self) -> Result<()> {
        // Background task: Monitor WAL position
        tokio::spawn(async move {
            loop {
                // Check current WAL position (no write, just read)
                let current_lsn = self.read_current_wal_lsn()?;

                if current_lsn > self.last_read_lsn {
                    // New data written by PostgreSQL
                    // Invalidate relevant caches
                    self.invalidate_caches_for_lsn_range(
                        self.last_read_lsn,
                        current_lsn
                    )?;

                    self.last_read_lsn = current_lsn;
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });

        // Main task: Serve queries
        self.serve_queries().await
    }

    fn execute_query(&self, sql: &str) -> Result<DataFrame> {
        // 1. Get MVCC snapshot (current WAL LSN)
        let snapshot = self.get_current_snapshot()?;

        // 2. Register tables as DataFusion sources
        for table in self.get_tables()? {
            let provider = PostgreSQLTableProvider::new(
                self.pg_data_dir.clone(),
                table.oid,
                snapshot,
            );
            self.datafusion_ctx.register_table(&table.name, Arc::new(provider))?;
        }

        // 3. Execute with DataFusion
        self.datafusion_ctx.sql(sql).await
    }

    fn get_current_snapshot(&self) -> Result<Snapshot> {
        // Read current LSN from PostgreSQL's shared memory
        // Or from control file
        // This gives us MVCC snapshot for consistent reads

        let current_lsn = read_current_lsn(&self.pg_data_dir)?;

        Ok(Snapshot {
            lsn: current_lsn,
            timestamp: Utc::now(),
        })
    }
}
```

## Implementation Details

### PostgreSQL Table Provider (Zero-Copy)

```rust
use datafusion::datasource::TableProvider;
use datafusion::execution::context::SessionContext;

struct PostgreSQLTableProvider {
    data_dir: PathBuf,
    table_oid: u32,
    snapshot: Snapshot,  // MVCC snapshot for consistency
}

#[async_trait]
impl TableProvider for PostgreSQLTableProvider {
    async fn scan(
        &self,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        // Get heap file path
        let heap_file = self.data_dir
            .join(format!("base/{}/{}", self.database_oid(), self.table_oid));

        Ok(Arc::new(PostgreSQLScanExec {
            heap_file,
            snapshot: self.snapshot,
            projection: projection.cloned(),
            filters: filters.to_vec(),
            limit,
        }))
    }

    fn schema(&self) -> SchemaRef {
        // Read schema from PostgreSQL catalog (pg_class, pg_attribute)
        // Cache this in memory
        self.read_table_schema().unwrap()
    }
}

/// Execution plan: Stream Arrow batches from PostgreSQL heap file
struct PostgreSQLScanExec {
    heap_file: PathBuf,
    snapshot: Snapshot,
    projection: Option<Vec<usize>>,
    filters: Vec<Expr>,
    limit: Option<usize>,
}

impl ExecutionPlan for PostgreSQLScanExec {
    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let heap_file = self.heap_file.clone();
        let snapshot = self.snapshot;

        let stream = async_stream::stream! {
            // Open PostgreSQL heap file (read-only)
            let mut reader = HeapFileReader::open(&heap_file)?;

            let mut rows_yielded = 0;

            // Read pages one-by-one
            for page_num in 0..reader.num_pages() {
                // Read 8KB page from disk
                let page = reader.read_page(page_num)?;

                // Parse tuples and check MVCC visibility
                let visible_tuples = self.get_visible_tuples(page, snapshot)?;

                if visible_tuples.is_empty() {
                    continue;
                }

                // Convert to Arrow RecordBatch (in-memory, transient)
                let batch = self.tuples_to_arrow_batch(visible_tuples)?;

                // Apply projection and filters
                let filtered = self.apply_projection_and_filters(batch)?;

                if filtered.num_rows() > 0 {
                    rows_yielded += filtered.num_rows();
                    yield Ok(filtered);

                    if let Some(limit) = self.limit {
                        if rows_yielded >= limit {
                            break;
                        }
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }
}

impl PostgreSQLScanExec {
    fn get_visible_tuples(&self, page: Page, snapshot: Snapshot) -> Result<Vec<HeapTuple>> {
        let mut visible = Vec::new();

        for tuple in page.tuples() {
            // Apply MVCC visibility rules
            if self.is_tuple_visible(tuple, snapshot)? {
                visible.push(tuple);
            }
        }

        Ok(visible)
    }

    fn is_tuple_visible(&self, tuple: &HeapTuple, snapshot: Snapshot) -> Result<bool> {
        // WARNING: This is a simplified sketch. The real implementation is ~500-800 lines.
        // See "MVCC Visibility: The Real Complexity" section below for the full breakdown
        // including hint bits, CLOG lookups, MultiXact, subtransactions, and HOT chains.
        todo!("See full visibility rules section")
    }
}
```

## Reading Strategy: When to Read Data Files

```rust
// ANSWER: Read data files ON-DEMAND during query execution
// NOT on commits, NOT on flushes, ONLY when queries need data

// Example timeline:

// T0: PostgreSQL INSERT (port 5432)
INSERT INTO orders VALUES (1, 100.00);
// → PostgreSQL writes to WAL
// → PostgreSQL updates buffer cache
// → pg_arrow does NOTHING (not involved)

// T1: PostgreSQL commits
COMMIT;
// → PostgreSQL fsync WAL
// → pg_arrow does NOTHING

// T2: Background writer flushes page to disk
// → PostgreSQL writes to base/16384/12345
// → pg_arrow does NOTHING (just monitors LSN)

// T3: User queries pg_arrow (port 5433)
SELECT SUM(amount) FROM orders;
// → pg_arrow reads heap file NOW (first time)
// → Converts to Arrow in-memory
// → Executes with DataFusion
// → Returns result
// → Frees Arrow memory

// T4: Another query
SELECT AVG(amount) FROM orders;
// → pg_arrow reads heap file AGAIN (fresh read)
// → No cached data (or with caching, check if stale via LSN)
```

**Frequency**: Only during query execution
**Method**: On-demand page reads
**Storage**: In-memory only (transient Arrow batches)

## Connection Routing: Smart Client

```rust
// Client-side connection router

struct SmartPgArrowClient {
    pg_conn: postgres::Client,         // Write connection (port 5432)
    pgarrow_conn: postgres::Client,    // Read connection (port 5433)
}

impl SmartPgArrowClient {
    fn execute(&mut self, sql: &str) -> Result<QueryResult> {
        if is_write_query(sql) {
            // Route to PostgreSQL
            self.pg_conn.execute(sql, &[])
        } else if is_analytical_query(sql) {
            // Route to pg_arrow (DataFusion)
            self.pgarrow_conn.query(sql, &[])
        } else {
            // Simple reads can go to either (use PostgreSQL)
            self.pg_conn.query(sql, &[])
        }
    }
}

fn is_analytical_query(sql: &str) -> bool {
    // Heuristics:
    // - Contains GROUP BY, aggregations (SUM, AVG, COUNT)
    // - Scans large tables (> 100K rows)
    // - No ORDER BY with LIMIT (OLTP pattern)
    // - Read-only (SELECT)

    let upper = sql.to_uppercase();
    upper.contains("GROUP BY")
        || upper.contains("SUM(")
        || upper.contains("AVG(")
        || upper.contains("COUNT(")
}

// Usage:
let mut client = SmartPgArrowClient::connect()?;

// Write - goes to PostgreSQL
client.execute("INSERT INTO orders VALUES (1, 100.00)")?;

// Analytical read - goes to pg_arrow
let results = client.execute(
    "SELECT product_id, SUM(amount) FROM orders GROUP BY product_id"
)?;

// Simple read - goes to PostgreSQL
let user = client.execute("SELECT * FROM users WHERE id = 123")?;
```

## Deployment Example

```bash
# Server setup (single machine)

# 1. Start PostgreSQL (handles writes)
postgres -D /pg/data -p 5432

# 2. Start pg_arrow (handles analytical reads)
pg_arrow serve \
  --pg-data-dir /pg/data \
  --port 5433 \
  --read-only \
  --monitor-wal

# Both running, both reading from /pg/data
```

### Configuration

```toml
# pg_arrow.toml

[source]
# PostgreSQL data directory (read-only access)
pg_data_dir = "/var/lib/postgresql/data"

# Optional: Connect to PostgreSQL to read catalogs
pg_host = "localhost"
pg_port = 5432
pg_database = "mydb"
pg_user = "pgarrow_reader"

[server]
# pg_arrow listening port
port = 5433
protocol = "postgres"  # PostgreSQL wire protocol

# Read-only mode (CRITICAL)
read_only = true

[performance]
# WAL monitoring interval
wal_check_interval_ms = 100

# Table metadata cache TTL
metadata_cache_ttl_seconds = 300

# Optional: Cache hot table pages in memory
enable_page_cache = true
page_cache_size_mb = 1024

[query]
# Route these query patterns to DataFusion
analytical_patterns = [
    "GROUP BY",
    "SUM(",
    "AVG(",
    "COUNT(",
    "WINDOW FUNCTION"
]

# Minimum table size for DataFusion (smaller = use PostgreSQL)
min_table_size_kb = 1024
```

## Failover Scenario

```
Normal operation:
┌──────────────┐     ┌──────────────┐
│ PostgreSQL   │     │ pg_arrow     │
│ (Primary)    │     │ (Analytics)  │
│ Port 5432    │     │ Port 5433    │
└──────┬───────┘     └──────┬───────┘
       │                    │
       └────────┬───────────┘
                ▼
           /pg/data/

Primary fails:
┌──────────────┐     ┌──────────────┐
│ PostgreSQL   │     │ pg_arrow     │
│ (PROMOTE!)   │     │ (Continue)   │
│ Port 5432 ✓  │     │ Port 5433 ✓  │
└──────┬───────┘     └──────┬───────┘
       │                    │
       └────────┬───────────┘
                ▼
           /pg/data/

The PostgreSQL instance IS promotable!
pg_arrow continues reading (no downtime for analytics)
```

**Promotion steps**:

```bash
# 1. Promote PostgreSQL instance
pg_ctl promote -D /pg/data

# 2. pg_arrow automatically continues
# (no changes needed - still reads same files)

# 3. Application reconnects writes to promoted instance
# Analytics continue without interruption
```

## File Access Patterns

### PostgreSQL (Read-Write)

```rust
// PostgreSQL has exclusive write access
// Uses locking/MVCC to manage concurrent access

File operations:
  - Read: buffer cache → heap files
  - Write: buffer cache → heap files (via background writer)
  - WAL: append-only writes to pg_wal/
  - Locks: row-level, table-level (in-memory)
```

### pg_arrow (Read-Only)

```rust
// pg_arrow has read-only access
// No locks needed — but reads require WAL replay for consistency

File operations:
  - Read: heap files (on-demand, pages may be stale relative to shared buffers)
  - Read: pg_wal/ files (WAL replay to bring stale pages up to target_lsn)
  - Read: pg_xact/ files (CLOG for transaction commit/abort status)
  - NO writes to data files
  - Monitor: WAL position (for cache invalidation and consistency target)
```

**Important**: Naive heap file reads are NOT consistent — pages on disk lag behind shared
buffers, and different pages are at different LSNs. WAL replay is required to bring all
pages to a single consistent point in time. See "Read Consistency for Direct Heap File
Access" below for the full solution.

## Read Consistency for Direct Heap File Access (Modes 1 & 2)

> **Context**: When pg_arrow reads PostgreSQL heap files directly from `$PGDATA/`, it faces
> two fundamental consistency problems that naive file reads cannot solve. This section
> documents the problems and the WAL replay solution required for correctness.

### The Two Consistency Problems

**Problem 1: Shared Buffer Lag — Committed Data Missing From Disk**

PostgreSQL's write-ahead logging means data pages on disk are always behind the current
state. A transaction commits by flushing WAL, not by flushing data pages. The bgwriter
and checkpointer flush dirty pages asynchronously — potentially minutes after commit:

```
T0: INSERT INTO orders VALUES (1, 100.00);
T1: COMMIT;
    └─ WAL flushed to pg_wal/ (fsync)          ← committed, durable
    └─ Data page: STILL IN SHARED BUFFERS ONLY  ← not on disk yet
    └─ Heap file on disk: STALE

T2: pg_arrow reads heap file
    └─ Misses the committed row entirely

T3: bgwriter eventually flushes page (minutes later)
    └─ NOW the row is on disk
```

Reading heap files alone misses any committed data whose pages haven't been flushed by
bgwriter or checkpointer. On a busy system this can be a significant fraction of recent data.

**Problem 2: Cross-Page Inconsistency — Pages at Different Points in Time**

When reading multiple pages of a table, each page is at a different LSN. Every page header
contains `pd_lsn` — the LSN of the last WAL record applied to that page. Pages are flushed
independently by bgwriter:

```
Reading 20 pages of a table at time T:

Page 0:  pd_lsn = 0/5A000  ← flushed recently
Page 5:  pd_lsn = 0/5F000  ← flushed very recently
Page 10: pd_lsn = 0/52000  ← flushed a while ago
Page 15: pd_lsn = 0/48000  ← not flushed since last checkpoint

These pages represent 4 different points in time.
A tuple's HOT chain could cross page boundaries at inconsistent states.
VACUUM may have cleaned some pages but not others.
```

### Single Page Atomicity — Not the Real Problem

A single 8KB aligned `pread()` is practically atomic on modern systems. PostgreSQL's
bgwriter writes one aligned 8KB block at a time, and Linux's VFS page lock mechanism
ensures an aligned read won't see a partially-written block during normal operation.
Torn pages only occur during crashes (which is why `full_page_writes` exists for WAL
recovery). So single-page reads are safe — the real problems are the two above.

### Why MVCC Alone Doesn't Fully Solve This

MVCC visibility (xmin/xmax + snapshot) gives logical consistency — each tuple is
independently evaluated for visibility regardless of page state. This handles the
cross-page inconsistency problem for data that is physically present on disk.

But MVCC cannot make invisible data appear. If a committed INSERT's page hasn't been
flushed to disk, that tuple simply doesn't exist in the heap file. No amount of MVCC
checking can find a tuple that isn't physically there.

### The Solution: WAL Replay to Target LSN

WAL is guaranteed flushed to disk on commit (`synchronous_commit = on`, the default).
Even when data pages are stale, the WAL contains everything needed to bring them current.
Every page has `pd_lsn` in its header. The algorithm:

```
1. Choose target_lsn (a point where WAL is complete on disk)

2. For each page needed:
   a. Read page from heap file → get pd_lsn from header
   b. If pd_lsn >= target_lsn → page is already current, use as-is
   c. If pd_lsn < target_lsn → page is stale:
      - Scan WAL from pd_lsn to target_lsn
      - Find records tagged with this (relfilenode, fork, block_number)
      - Apply them to the in-memory page copy
      - Page is now at target_lsn

3. All pages are now at the same target_lsn → cross-page consistent

4. Apply MVCC visibility using snapshot → logically consistent result
```

This is the same mechanism as PostgreSQL's crash recovery and `pg_basebackup` — both
replay WAL over potentially-inconsistent page states to achieve consistency.

### Choosing target_lsn

**With PostgreSQL connection (recommended)**:

```sql
SELECT pg_current_wal_flush_lsn();
-- Returns the LSN up to which WAL is guaranteed flushed to pg_wal/ on disk
-- All committed transactions before this LSN have their WAL records on disk
```

**On a replica (Mode 2)**:

```sql
SELECT pg_last_wal_replay_lsn();
-- Returns the LSN up to which WAL has been replayed into data files
-- With paused replay, ALL pages on disk are at this LSN — no WAL replay needed
```

**Fully offline (no PostgreSQL connection)**:

Read the last complete WAL record from `pg_wal/` files directly. The end LSN of that
record is a safe target_lsn.

### WAL Record Types for Heap Pages

WAL records are tagged with `(relfilenode, fork_number, block_number)`, so pg_arrow can
find exactly which records affect which page:

```
Resource Manager: RM_HEAP_ID
  XLOG_HEAP_INSERT      → tuple data + offset to insert at
  XLOG_HEAP_DELETE      → item pointer offset to mark deleted
  XLOG_HEAP_UPDATE      → old offset + new tuple data
  XLOG_HEAP_HOT_UPDATE  → UPDATE within same page (no index update)
  XLOG_HEAP_LOCK        → row lock (FOR UPDATE/SHARE) — modifies xmax/infomask
  XLOG_HEAP_INIT_PAGE   → initialize a new page

Resource Manager: RM_HEAP2_ID
  XLOG_HEAP2_CLEAN         → vacuum page cleanup (remove dead line pointers)
  XLOG_HEAP2_FREEZE_PAGE   → freeze old tuples (set XMIN_FROZEN)
  XLOG_HEAP2_VISIBLE       → mark page all-visible in visibility map
  XLOG_HEAP2_MULTI_INSERT  → multi-row INSERT (COPY)
  XLOG_HEAP2_LOCK_UPDATED  → lock an already-updated tuple
```

### Full Page Images (FPI) — A Major Optimization

When `full_page_writes = on` (default), the first modification to any page after a
checkpoint writes the **entire 8KB page image** into the WAL record as a backup block:

1. If pg_arrow finds an FPI in the WAL stream for a page, it can use the FPI as the
   page content directly — no need to read the heap file for that page
2. After a checkpoint, every modified page's first WAL record contains its FPI
3. Subsequent WAL records for the same page (before next checkpoint) contain only deltas

For the page cache (WAL Synchronization Level 2), pg_arrow can extract FPIs from the WAL
stream and update its cache with zero file I/O.

### The Complete Read Pipeline

```
Query arrives at pg_arrow (Mode 1 or 2):

1. Acquire target_lsn
   ├─ Mode 1 (primary):  SELECT pg_current_wal_flush_lsn()
   └─ Mode 2 (replica):  SELECT pg_last_wal_replay_lsn()

2. Acquire MVCC snapshot
   └─ SELECT pg_current_snapshot()  →  'xmin:xmax:xip_list'

3. For each page in the table:
   a. pread() 8KB from heap file → raw page bytes
   b. Parse page header → extract pd_lsn
   c. If pd_lsn < target_lsn:
      ├─ Scan WAL files for records matching (relfilenode, block_num)
      │  in range pd_lsn..target_lsn
      ├─ If FPI found → use FPI as page base, apply subsequent deltas
      └─ Apply WAL records in LSN order to in-memory page copy
   d. Page is now at target_lsn
   e. For each tuple on page:
      └─ Check MVCC visibility against snapshot (Phase 2 logic)
   f. Visible tuples → convert to Arrow columnar format
   g. Yield RecordBatch to DataFusion

4. DataFusion executes query plan over RecordBatch stream

5. Return results to client
```

### Mode 2 Advantage: Zero WAL Replay

On a replica with paused WAL replay, all pages on disk are guaranteed consistent at the
replay LSN. No concurrent writes are modifying pages. This eliminates the WAL replay step
entirely:

```sql
-- On replica:
SELECT pg_wal_replay_pause();
-- No bgwriter, no recovery process modifying pages
-- All pages on disk are at pg_last_wal_replay_lsn()
-- pg_arrow reads heap files — perfectly consistent, no WAL replay needed
SELECT pg_wal_replay_resume();
```

This is why Mode 2 (sidecar + replica) is the recommended production deployment for heap
file reading. The tradeoff is briefly pausing replay, which increases replication lag by
the duration of the scan.

### Practical Cost of WAL Replay

| Scenario | WAL to replay per page | Notes |
|---|---|---|
| Right after `CHECKPOINT` | Minimal (0-few records) | Pages recently flushed, pd_lsn close to target |
| Long after `CHECKPOINT` (~5 min) | More records per page | Pages may be `checkpoint_timeout` behind |
| Mode 2 with paused replay | **Zero** | All pages already at replay LSN |
| Hot table with frequent updates | More records per page | But also more likely in OS page cache |

**Optimization**: Issue `CHECKPOINT` before a large scan to force-flush all dirty buffers.
After checkpoint completes, most pages have pd_lsn close to current, minimizing WAL replay.
This adds checkpoint overhead (~seconds) but makes the scan itself faster.

### WAL Parsing Complexity

WAL record format is PostgreSQL-version-specific and not a stable API. Implementing a
WAL parser requires:

| Component | Description | Approximate size |
|---|---|---|
| WAL page headers | `XLogPageHeaderData`, `XLogLongPageHeaderData` | ~100 lines |
| Record headers | `XLogRecord` (24 bytes: total_length, xid, rmgr_id) | ~200 lines |
| Resource manager dispatch | Route by rmgr_id to heap/heap2/etc. handlers | ~300 lines |
| Heap record parsing | `xl_heap_insert`, `xl_heap_delete`, `xl_heap_update` | ~500 lines |
| Backup block (FPI) extraction | `XLogRecordBlockHeader`, compressed/uncompressed FPI | ~400 lines |
| Version-aware handling | Layout differences between PG major versions | ~500 lines |
| **Total** | | **~2000-3000 lines** |

PostgreSQL's `pg_waldump` source (`src/bin/pg_waldump/`) is the reference implementation.

**Phase dependency**: WAL replay for read consistency (Phase 2) shares parsing
infrastructure with WAL stream parsing for cache invalidation (Phase 12b). The record
parser is the same code; the difference is whether records are applied to in-memory pages
(consistency) or used to invalidate/update cache entries (optimization).

### Consistency Tiers — Tradeoffs

pg_arrow can operate at different consistency levels depending on deployment needs:

| Tier | Consistency | WAL replay needed | Requirements |
|---|---|---|---|
| **Tier 0: Checkpoint-bound reads** | Pages consistent up to last checkpoint only | None | Read `pg_control` for checkpoint LSN |
| **Tier 1: MVCC-only (no WAL replay)** | Logically correct for data on disk, but misses unflushed committed data | None | Snapshot via `pg_current_snapshot()` |
| **Tier 2: WAL replay to flush LSN** | Fully consistent including unflushed data | Yes | WAL parser + `pg_current_wal_flush_lsn()` |
| **Tier 3: Paused replica reads** | Fully consistent, zero WAL replay cost | None | Mode 2 replica + `pg_wal_replay_pause()` |

Phase 1 can start with Tier 1 (MVCC-only) — this is correct for data on disk but may
miss very recently committed rows whose pages haven't been flushed. For most analytical
workloads (where data freshness of seconds-to-minutes is acceptable), this is sufficient.
Tier 2 (full WAL replay) is the complete solution, implemented alongside Phase 12b.

## MVCC Consistency

> **Note**: This section shows a simplified mental model. The real visibility logic requires
> checking infomask hint bits, CLOG, and snapshot `xip[]` — not just LSN comparison.
> See "MVCC Visibility: The Real Complexity" below for the full picture.

```rust
// Both engines respect PostgreSQL's MVCC

// PostgreSQL writes:
INSERT INTO orders VALUES (1, 100.00);  // xid = 1000
// Tuple: { xmin: 1000, xmax: 0, infomask: 0x0000, data: ... }
// Page LSN: 16/A000

COMMIT;
// WAL LSN advances to 16/A100
// CLOG entry for xid=1000 set to COMMITTED

// pg_arrow reads (before commit flushed):
// snapshot = { xmin: 999, xmax: 1001, xip: [1000] }
// Reads page, sees tuple with xmin=1000
// 1. No XMIN_COMMITTED hint bit → check CLOG
// 2. CLOG says IN_PROGRESS (or xid=1000 is in snapshot.xip[])
// → tuple not visible

// pg_arrow reads (after commit):
// snapshot = { xmin: 1001, xmax: 1002, xip: [] }
// Reads same page
// 1. No XMIN_COMMITTED hint bit → check CLOG
// 2. CLOG says COMMITTED, xid=1000 < snapshot.xmax, not in xip[]
// → tuple visible
// (pg_arrow cannot set hint bit since it's read-only — pays CLOG cost again next query)
```

## MVCC Visibility: The Real Complexity

> **Context**: The original `is_tuple_visible` sketch in this document checked only page LSN and
> `xmax != 0`, which would produce incorrect results in nearly every real-world scenario.
> A review of PostgreSQL's actual visibility logic (`src/backend/access/heap/heapam_visibility.c`,
> function `HeapTupleSatisfiesMVCC`) revealed the full scope documented below. This section captures
> what a correct pg_arrow implementation actually needs.

### Tuple Header MVCC Fields

Every heap tuple carries these MVCC-critical fields in its header:

```
t_xmin       - Transaction ID that inserted this tuple
t_xmax       - Overloaded field: delete/update xid, row lock xid, or MultiXactId (see below)
t_cid        - Command ID within the inserting/deleting transaction
t_ctid       - "Current" tuple ID — points to newer version if HOT-updated
t_infomask   - 16 status/hint bits
t_infomask2  - 16 more status bits
```

### Infomask Bits Relevant to Visibility

```
HEAP_XMIN_COMMITTED  (0x0100) - xmin is known committed (hint bit)
HEAP_XMIN_INVALID    (0x0200) - xmin is known aborted (hint bit)
HEAP_XMIN_FROZEN     (0x0300) - xmin is frozen, always visible (combined bits)
HEAP_XMAX_COMMITTED  (0x0400) - xmax is known committed (hint bit)
HEAP_XMAX_INVALID    (0x0800) - xmax is known aborted/unused (hint bit)
HEAP_XMAX_IS_MULTI   (0x1000) - xmax is a MultiXactId, not a plain xid
HEAP_XMAX_LOCK_ONLY  (0x0080) - xmax represents a row lock, not a delete
HEAP_UPDATED         (0x2000) - tuple resulted from an UPDATE (not INSERT)
HEAP_HOT_UPDATED     (0x4000) - tuple was HOT-updated (index not changed)
HEAP_COMBOCID        (0x0020) - t_cid is a "combo" command ID
```

### xmax Is Overloaded — It Does NOT Mean "Deleted"

A common mistake is to treat `xmax != 0` as "this tuple was deleted." In reality, `xmax` is a
multi-purpose field whose meaning depends entirely on the infomask bits. Without checking infomask,
`xmax` alone is ambiguous:

| `xmax` value | Infomask bits                      | What it means                                      | Tuple visible?          |
| ------------ | ---------------------------------- | -------------------------------------------------- | ----------------------- |
| `0`          | `XMAX_INVALID`                     | Never deleted or locked                            | Yes                     |
| non-zero     | `XMAX_INVALID`                     | Was set, but transaction aborted                   | Yes                     |
| non-zero     | `XMAX_LOCK_ONLY`                   | Row lock (`FOR UPDATE`/`FOR SHARE`) — not a delete | Yes                     |
| non-zero     | `XMAX_IS_MULTI` + `XMAX_LOCK_ONLY` | Multiple concurrent row locks — not a delete       | Yes                     |
| non-zero     | `XMAX_COMMITTED`                   | Committed delete/update                            | No (if before snapshot) |
| non-zero     | `XMAX_IS_MULTI` (no `LOCK_ONLY`)   | Mixed lock + delete — must resolve members         | Must resolve            |
| non-zero     | (no hint bits)                     | Unknown status — must check CLOG                   | Must check CLOG         |

**Why this matters for pg_arrow**: The original design sketch used `if xmax != 0 { return false }`,
which would incorrectly hide:

- Every row that was ever row-locked by `SELECT ... FOR UPDATE/SHARE`
- Every row where a DELETE/UPDATE was attempted but rolled back
- Every row involved in concurrent locking (MultiXact)

On a busy OLTP database, this could mean **hiding a large fraction of live rows**. The infomask
bits are not optional metadata — they are the only way to interpret what `xmax` means.

### What a Snapshot Really Is

A snapshot is NOT just an LSN. PostgreSQL snapshots contain:

```
xmin    - Oldest still-active transaction ID at snapshot time
xmax    - First not-yet-assigned transaction ID at snapshot time
xip[]   - Array of all transaction IDs that were in-progress at snapshot time
```

A transaction is "visible" if it committed before the snapshot was taken AND is not in the `xip[]` array (was not still running when the snapshot was created).

### The Real Visibility Algorithm (HeapTupleSatisfiesMVCC)

```
HeapTupleSatisfiesMVCC(tuple, snapshot):

  // ======== Phase 1: Check xmin (the inserting transaction) ========

  if HEAP_XMIN_INVALID is set:
      return NOT_VISIBLE                    // inserter aborted

  if HEAP_XMIN_FROZEN is set:
      // tuple was frozen by VACUUM — always visible
      goto check_xmax

  if HEAP_XMIN_COMMITTED is set:
      // hint bit says committed — skip CLOG lookup
      goto xmin_committed

  // No hint bits — must check CLOG (pg_xact/)
  status = read_clog(t_xmin)

  if status == IN_PROGRESS:
      if t_xmin == current_transaction:
          // Our own insert — check CID for command ordering
          if t_cid >= snapshot.curcid:
              return NOT_VISIBLE            // inserted after our scan started
          goto check_xmax
      return NOT_VISIBLE                    // someone else's uncommitted insert

  if status == ABORTED:
      set HEAP_XMIN_INVALID hint bit        // cache for future readers
      return NOT_VISIBLE

  if status == COMMITTED:
      set HEAP_XMIN_COMMITTED hint bit      // cache for future readers

  xmin_committed:
      // xmin committed — but was it visible at snapshot time?
      if t_xmin IN snapshot.xip[]:
          return NOT_VISIBLE                // was still in-progress at snapshot
      if t_xmin >= snapshot.xmax:
          return NOT_VISIBLE                // started after snapshot

  // ======== Phase 2: Check xmax (the deleting/updating transaction) ========

  check_xmax:
      if t_xmax == 0 OR HEAP_XMAX_INVALID is set:
          return VISIBLE                    // not deleted

      if HEAP_XMAX_LOCK_ONLY is set:
          return VISIBLE                    // xmax is a row lock, not a real delete

      if HEAP_XMAX_IS_MULTI is set:
          // MultiXactId — must resolve individual member transactions
          // Read pg_multixact/offsets/ and pg_multixact/members/
          // Check each member: is any a committed delete (not just a lock)?
          return resolve_multixact_visibility(t_xmax, snapshot)

      if HEAP_XMAX_COMMITTED is set:
          goto xmax_committed

      // No hint bits on xmax — check CLOG
      status = read_clog(t_xmax)

      if status == IN_PROGRESS:
          return VISIBLE                    // deleter hasn't committed yet
      if status == ABORTED:
          set HEAP_XMAX_INVALID hint bit
          return VISIBLE                    // deleter aborted
      if status == COMMITTED:
          set HEAP_XMAX_COMMITTED hint bit

  xmax_committed:
      // xmax committed — but was it visible at snapshot time?
      if t_xmax IN snapshot.xip[]:
          return VISIBLE                    // deleter was still running at snapshot
      if t_xmax >= snapshot.xmax:
          return VISIBLE                    // deleter started after snapshot
      return NOT_VISIBLE                    // deleted before our snapshot
```

### External File Dependencies

The visibility check cannot be performed using only the heap file. It requires reading:

| File/Directory          | Purpose                            | Format                                |
| ----------------------- | ---------------------------------- | ------------------------------------- |
| `pg_xact/` (CLOG)       | Transaction commit/abort status    | 2 bits per xid, packed into 8KB pages |
| `pg_multixact/offsets/` | Map MultiXactId to member offset   | 4 bytes per MultiXactId               |
| `pg_multixact/members/` | Individual xids within a MultiXact | (xid, status) pairs                   |
| `pg_subtrans/`          | Map sub-transaction to parent xid  | 4 bytes per xid                       |

### Implications for pg_arrow

**1. CLOG reader is mandatory**

Every tuple without hint bits requires a CLOG lookup. The `pg_xact/` files use a simple format
(2 bits per transaction: `IN_PROGRESS=0x00`, `COMMITTED=0x01`, `ABORTED=0x02`, `SUB_COMMITTED=0x03`),
packed 4 statuses per byte into 8KB pages. pg_arrow must implement a reader for these files.

**2. pg_arrow cannot write hint bits back**

PostgreSQL sets hint bits (`XMIN_COMMITTED`, `XMAX_INVALID`, etc.) as a side-effect of visibility
checks, avoiding repeated CLOG lookups. Since pg_arrow is read-only and must not write to heap
files, it will pay the CLOG lookup cost on every query for tuples that lack hint bits. This is a
meaningful performance penalty on recently-inserted data.

**3. Snapshot acquisition is non-trivial**

Getting a correct snapshot (with `xip[]` array) requires either:

- Connecting to PostgreSQL via SQL: `SELECT pg_current_snapshot()`
- Reading PostgreSQL's shared memory proc array (complex, version-dependent)
- Using a simplified approach that sacrifices some correctness (e.g., LSN-only)

**4. MultiXact and subtransaction resolution adds complexity**

Row-level locks (`SELECT ... FOR UPDATE`) create MultiXact entries. Savepoints create
subtransactions. Both require reading additional on-disk structures to resolve visibility.

**5. HOT update chains**

Heap-Only Tuples (HOT) form chains within a page via `t_ctid`. When a tuple is HOT-updated,
the old version's `t_ctid` points to the new version on the same page. pg_arrow must follow
these chains to find the currently-visible version.

### Practical Phased Approach for pg_arrow

Given the complexity, we should implement visibility in stages:

**Phase 2a — Frozen-only reads (simplest, always correct)**:

- Only return tuples with `HEAP_XMIN_FROZEN` set
- These are always visible, no CLOG needed
- Works for any data that VACUUM has frozen (older data)
- Limitation: misses recently-inserted data

**Phase 2b — Hint-bit reads + CLOG**:

- Read tuples with `XMIN_COMMITTED` hint bit (no CLOG needed)
- Implement CLOG reader for tuples without hint bits
- Acquire snapshot via `pg_current_snapshot()` over a PostgreSQL connection
- Skip MultiXact tuples (treat as not-visible, conservative)

**Phase 2c — Full visibility**:

- MultiXact resolution
- Subtransaction handling
- HOT chain traversal
- Combo CID support

## Performance Comparison

### Write Path (PostgreSQL Only)

```
INSERT: Same performance (pg_arrow not involved)
UPDATE: Same performance (pg_arrow not involved)
DELETE: Same performance (pg_arrow not involved)
```

### Read Path (Analytical Queries)

| Query Type          | PostgreSQL | pg_arrow (DataFusion) | Speedup |
| ------------------- | ---------- | --------------------- | ------- |
| GROUP BY + SUM      | 45 seconds | 1.2 seconds           | 37x     |
| Large scan + filter | 30 seconds | 800ms                 | 37x     |
| Window functions    | 60 seconds | 2 seconds             | 30x     |
| Complex joins       | 25 seconds | 3 seconds             | 8x      |

### Read Path (OLTP Queries)

| Query Type       | PostgreSQL | pg_arrow (DataFusion) | Winner       |
| ---------------- | ---------- | --------------------- | ------------ |
| Single row by PK | 0.5ms      | 2ms                   | PostgreSQL ✓ |
| Small range scan | 2ms        | 5ms                   | PostgreSQL ✓ |
| Index lookup     | 1ms        | N/A (no indexes)      | PostgreSQL ✓ |

**Strategy**: Route OLAP to pg_arrow, OLTP to PostgreSQL

## Caching Strategy (Optional Optimization)

```rust
// Optional: Cache hot table metadata and pages

struct PgArrowCache {
    // Schema cache (rarely changes)
    schemas: HashMap<u32, Arc<Schema>>,
    schema_lsn: HashMap<u32, LSN>,

    // Optional: Hot page cache
    pages: LruCache<(u32, u32), Page>,  // (table_oid, page_num) → Page
    page_lsn: HashMap<(u32, u32), LSN>,
}

impl PgArrowCache {
    fn get_schema(&mut self, table_oid: u32) -> Result<Arc<Schema>> {
        if let Some(schema) = self.schemas.get(&table_oid) {
            // Check if still valid (compare LSN)
            let cached_lsn = self.schema_lsn.get(&table_oid).unwrap();
            let current_lsn = get_current_lsn()?;

            if *cached_lsn == current_lsn {
                return Ok(schema.clone());
            }
        }

        // Cache miss or stale - read from PostgreSQL catalog
        let schema = self.read_schema_from_pg(table_oid)?;
        let current_lsn = get_current_lsn()?;

        self.schemas.insert(table_oid, schema.clone());
        self.schema_lsn.insert(table_oid, current_lsn);

        Ok(schema)
    }
}
```

## Key Advantages of This Approach

1. ✅ **True zero-copy**: Both engines read same files
2. ✅ **Promotable**: PostgreSQL is real and can be promoted
3. ✅ **Simple**: No complex replication setup
4. ✅ **Fast**: DataFusion for analytics (10-100x speedup)
5. ✅ **Safe**: PostgreSQL handles all writes correctly
6. ✅ **Flexible**: Can run both on same or different machines
7. ✅ **Drop-in**: Add pg_arrow without touching PostgreSQL

## PostgreSQL SQL Compatibility via DataFusion Extensions

> **Context**: Initial analysis assumed DataFusion couldn't handle many PostgreSQL SQL features.
> On closer review, DataFusion's extension APIs cover the vast majority of analytical query
> patterns. The gap is much smaller than initially thought.

### DataFusion Extension Points

```
ScalarUDF       → Register custom scalar functions (date_trunc, to_char, etc.)
AggregateUDF    → Register custom aggregates with accumulators (string_agg, percentile_cont)
WindowUDF       → Register custom window functions
TableFunction   → Table-valued functions (generate_series)
OptimizerRule   → Custom SQL rewriting (operator → function call translation)
ExecutionPlan   → Custom physical operators
SQL dialect     → sqlparser-rs supports PostgreSQL dialect natively
```

### Already Supported by DataFusion (No Work Needed)

These work out of the box with DataFusion's PostgreSQL dialect enabled:

| Feature                             | Example                                      | Notes                           |
| ----------------------------------- | -------------------------------------------- | ------------------------------- |
| `::` cast syntax                    | `x::text`, `'2024-01-01'::date`              | sqlparser-rs PostgreSQL dialect |
| `FILTER` clause                     | `COUNT(*) FILTER (WHERE x > 5)`              | Native support                  |
| `ILIKE`                             | `name ILIKE '%foo%'`                         | Native support                  |
| `GROUPING SETS` / `CUBE` / `ROLLUP` | `GROUP BY CUBE(a, b)`                        | Supported since ~v28            |
| Array functions                     | `array_agg()`, `unnest()`, `array_length()`  | Native `List` type              |
| Window functions                    | `ROW_NUMBER`, `RANK`, `LAG`, `LEAD`, `NTILE` | Native support                  |
| Recursive CTEs                      | `WITH RECURSIVE ...`                         | Supported since ~v38            |
| `COALESCE`, `NULLIF`, `CASE`        | Standard SQL                                 | Native support                  |
| Basic aggregates                    | `SUM`, `AVG`, `COUNT`, `MIN`, `MAX`          | Native support                  |
| `date_trunc`, `extract`             | `date_trunc('day', ts)`                      | Native support                  |

### Implementable via UDF/UDAF Registration (~50-200 lines each)

These require registering custom functions but are straightforward:

**Scalar functions (ScalarUDF)**:

```rust
// to_char — PostgreSQL date formatting
ctx.register_udf(create_udf(
    "to_char",
    vec![DataType::Timestamp, DataType::Utf8],
    Arc::new(DataType::Utf8),
    Volatility::Immutable,
    Arc::new(pg_to_char_impl),  // implement PG format patterns
));

// Additional: age(), date_part(), make_interval(), timezone()
```

**Aggregate functions (AggregateUDF)**:

```rust
// string_agg — concatenate strings with delimiter
impl Accumulator for StringAggAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let strings = as_string_array(&values[0]);
        let delim = as_string_array(&values[1]).value(0);
        for val in strings.iter().flatten() {
            if !self.result.is_empty() {
                self.result.push_str(delim);
            }
            self.result.push_str(val);
        }
        Ok(())
    }
    fn evaluate(&mut self) -> Result<ScalarValue> {
        Ok(ScalarValue::Utf8(Some(self.result.clone())))
    }
}

// percentile_cont — continuous percentile with WITHIN GROUP
// DataFusion's AggregateUDF supports ORDER BY within aggregates,
// so the accumulator receives pre-sorted input.

// Additional: mode(), percentile_disc(), bool_and(), bool_or()
```

**Table-valued functions (TableFunction)**:

```rust
// generate_series(start, stop[, step])
impl TableFunctionImpl for GenerateSeries {
    fn call(&self, args: &[Expr]) -> Result<Arc<dyn TableProvider>> {
        let start = extract_i64(&args[0])?;
        let stop = extract_i64(&args[1])?;
        let step = args.get(2).map(|a| extract_i64(a)).unwrap_or(Ok(1))?;
        Ok(Arc::new(GenerateSeriesTable { start, stop, step }))
    }
}
ctx.register_udtf("generate_series", Arc::new(GenerateSeries));
```

### JSON Support via datafusion-functions-json

The `datafusion-functions-json` crate adds JSON operators:

```rust
// Adds: ->, ->>, json_extract, json_extract_scalar, etc.
// Combined with an OptimizerRule, can rewrite PostgreSQL operator syntax:
//   data->>'key'  →  json_extract_scalar(data, 'key')
```

### Capability-Based Query Routing

For features that genuinely can't be handled, route to PostgreSQL:

```rust
fn route_query(sql: &str, features: &QueryFeatures) -> QueryTarget {
    // These MUST go to PostgreSQL — can't be implemented in DataFusion
    if features.uses_extension_functions    // PostGIS, pg_trgm, etc.
        || features.uses_plpgsql            // stored procedures
        || features.uses_custom_types       // domains, composite types
        || features.uses_custom_operators   // operator overloading
    {
        return QueryTarget::PostgreSQL;
    }

    // Everything else: DataFusion handles it (natively or via registered UDFs)
    QueryTarget::DataFusion
}
```

### What Actually Requires PostgreSQL Fallback

The set of features that genuinely can't be implemented in DataFusion is small:

| Feature                     | Why it can't be a UDF                               | Fallback cost           |
| --------------------------- | --------------------------------------------------- | ----------------------- |
| PostGIS functions           | Entire C geometry library (GEOS, PROJ)              | High — use PostgreSQL   |
| PL/pgSQL functions          | Requires a procedural language interpreter          | High — use PostgreSQL   |
| Custom types/domains        | PostgreSQL's extensible type system with custom I/O | Medium — use PostgreSQL |
| Custom operators            | Operator overloading with type resolution           | Medium — use PostgreSQL |
| `pg_trgm`, full-text search | GIN/GiST index-dependent functionality              | High — use PostgreSQL   |

For analytical workloads, these features are rarely in the hot path. The vast majority of
`GROUP BY` / `SUM` / `JOIN` / `WINDOW` queries will run entirely in DataFusion.

## Physical Storage Features

> **Context**: The heap file reader is only the starting point. PostgreSQL's physical storage
> includes several auxiliary structures that pg_arrow must handle for correctness and performance.
> TOAST in particular is critical — without it, any table with text or JSONB columns returns
> garbage (18-byte pointer structs instead of actual data).

### TOAST (The Oversized-Attribute Storage Technique) — CRITICAL

Any column value larger than ~2KB gets moved to a separate TOAST table. The main tuple stores
an 18-byte TOAST pointer instead of the actual data:

```
Physical layout:
  base/16384/24601          ← main heap file
  base/16384/24605          ← TOAST table (pg_toast.pg_toast_24601)

TOAST pointer structure (18 bytes):
  va_rawsize     u32   - Original uncompressed size
  va_extinfo     u32   - Compression method (pglz/lz4) + compressed size
  va_toastrelid  u32   - OID of the TOAST table
  va_valueid     u32   - chunk_id in the TOAST table
  va_toastrelid  (oid) - toast relation to read from

TOAST table internal structure:
  chunk_id    oid    - Matches va_valueid from the pointer
  chunk_seq   int4   - Ordering (0, 1, 2, ...)
  chunk_data  bytea  - Up to TOAST_MAX_CHUNK_SIZE (~2000 bytes)
```

pg_arrow's read path for a TOASTed value:

```
1. Read tuple from main heap file
2. Check varlena header (first byte of datum):
   - 1-byte header, size < 127      → inline short value (no TOAST)
   - 4-byte header, no external bit → inline long value (no TOAST)
   - 4-byte header, external bit    → TOAST pointer, must detoast
3. If TOAST pointer:
   a. Read va_toastrelid → find TOAST heap file
   b. Scan TOAST table for matching chunk_id (va_valueid)
   c. Read chunks in chunk_seq order
   d. Reassemble into contiguous buffer
   e. If va_extinfo indicates compression:
      - pglz → decompress with PostgreSQL's pglz algorithm
      - lz4  → decompress with LZ4 (PG14+)
4. Return the full decompressed value
```

Columns commonly TOASTed: `text`, `varchar(long)`, `jsonb`, `json`, `bytea`, `xml`, `tsvector`,
and any wide row where total tuple size exceeds ~2KB.

**Without TOAST support, pg_arrow is unusable for most real-world tables.**

### Visibility Map (`_vm` fork) — Major Optimization

Each table has a visibility map file:

```
base/16384/24601        ← main heap file
base/16384/24601_vm     ← visibility map (2 bits per page)
```

Two bits per heap page:

- **Bit 0: all-visible** — Every tuple on the page is visible to all current transactions
- **Bit 1: all-frozen** — Every tuple on the page has `XMIN_FROZEN` set

**Why this matters for pg_arrow**: Reading the VM first lets pg_arrow skip expensive per-tuple
visibility checks for the majority of pages in a mature table:

```
Page scan with VM optimization:
  1. Read visibility map (small — 1 bit per 4KB of heap data)
  2. For each page:
     if VM says all-frozen:
       → skip ALL visibility checks, every tuple is visible
     if VM says all-visible:
       → skip CLOG lookups, but still check xmax for recent deletes
     else:
       → full visibility check (hint bits, CLOG, etc.)
```

For tables that have been vacuumed (most production tables), the vast majority of pages will be
all-frozen. This makes the visibility map the single biggest performance optimization for pg_arrow.

### Free Space Map (`_fsm` fork) — Not Needed

```
base/16384/24601_fsm    ← free space map
```

Tracks free space per page for INSERT placement. Read-only pg_arrow can ignore this entirely.

### Tablespaces — Follow Symlinks

Tables in non-default tablespaces are accessed via symlinks:

```
$PGDATA/pg_tblspc/16385 → /ssd/pg_data/PG_18_202401011/16384/24601
```

pg_arrow must resolve these symlinks when locating heap files. Cannot assume all tables are
under `base/`.

### Unlogged Tables — No WAL

```sql
CREATE UNLOGGED TABLE temp_analytics (...);
```

Unlogged tables have heap files in the same `base/` directory (with an `_init` fork), but skip
WAL. Since pg_arrow's cache invalidation relies on WAL LSN monitoring, changes to unlogged tables
won't be detected. Fallback: check file modification time, or always re-read.

### Views and Materialized Views

**Regular views**: Stored as query text in `pg_rewrite`. pg_arrow reads the view definition and
expands it to a query on the underlying tables before passing to DataFusion. No heap file involved.

**Materialized views**: Have their own heap files, readable like any other table. No special
handling needed — pg_arrow sees them as regular tables with their own OID in `pg_class`.

### Large Objects — Ignore

PostgreSQL's large object facility (`lo_*` functions) stores data in `pg_largeobject`. Rarely
used in analytical workloads. Not a priority.

## Partitioning — A Major Opportunity for pg_arrow

> **Context**: PostgreSQL declarative partitioning (RANGE, LIST, HASH) creates separate heap files
> per partition. This maps perfectly to DataFusion's parallel execution model and is one of the
> areas where pg_arrow can significantly outperform PostgreSQL.

### Physical Layout

```
Table: orders (partitioned by RANGE on created_at)
├── orders_2024_q1  OID 24601  → base/16384/24601  (separate heap file)
├── orders_2024_q2  OID 24602  → base/16384/24602  (separate heap file)
├── orders_2024_q3  OID 24603  → base/16384/24603  (separate heap file)
└── orders_2024_q4  OID 24604  → base/16384/24604  (separate heap file)

Catalog relationships:
  pg_class          → parent table (relkind = 'p') and partitions (relkind = 'r')
  pg_inherits       → parent OID → child OIDs
  pg_partitioned_table → partition strategy (range/list/hash)
  pg_class.relpartbound → partition bounds per child
```

### Partition Pruning

If the query filters on the partition key, pg_arrow can skip irrelevant partitions entirely:

```sql
SELECT SUM(amount) FROM orders WHERE created_at > '2024-07-01';
-- Only scan orders_2024_q3 and orders_2024_q4
-- Skip q1 and q2 entirely (never read from disk)
```

pg_arrow reads partition bounds from `pg_class.relpartbound` and compares against query filters
before creating scan plans.

### Parallel Scan Across Partitions

Each partition is an independent heap file → DataFusion can scan them in parallel:

```rust
impl TableProvider for PartitionedPostgreSQLTable {
    async fn scan(&self, projection, filters, limit) -> Result<Arc<dyn ExecutionPlan>> {
        let partitions = self.get_partitions()?;  // from pg_inherits

        // Prune based on partition bounds vs query filters
        let relevant = partitions.iter()
            .filter(|p| p.bounds_overlap(&filters))
            .collect::<Vec<_>>();

        // One scan per partition — DataFusion parallelizes across threads
        let scans: Vec<Arc<dyn ExecutionPlan>> = relevant.iter()
            .map(|p| Arc::new(PostgreSQLScanExec::new(p.heap_file(), snapshot)))
            .collect();

        Ok(Arc::new(UnionExec::new(scans)))
    }

    fn output_partitioning(&self) -> Partitioning {
        Partitioning::UnknownPartitioning(self.partition_count())
    }
}
```

### Table Inheritance (Legacy Partitioning)

Pre-PG10 schemas used `INHERITS` for partitioning. The `pg_inherits` catalog tracks the
hierarchy. pg_arrow handles this the same way: union child table scans when querying the parent.
Check constraints on children can serve as partition bounds for pruning.

### Sharding — Out of Scope

PostgreSQL has no native sharding. Citus and `postgres_fdw` provide distributed query execution,
but these are extensions with their own protocols. pg_arrow serves a single PostgreSQL instance;
sharding is the application's or orchestrator's responsibility. A future multi-instance pg_arrow
coordinator could use DataFusion's distributed planning, but this is a separate project.

## PostgreSQL Protocol Compatibility

> **Context**: pg_arrow must speak the PostgreSQL wire protocol for clients to connect. The
> protocol has multiple layers, and getting only Simple Query working means only `psql` works.
> Real client libraries (JDBC, psycopg2, asyncpg, node-postgres, pgx) use the Extended Query
> Protocol and will not work without it.

### Wire Protocol Layers

**Layer 1 — Connection (must have)**:

| Message            | Direction       | Purpose                                                       |
| ------------------ | --------------- | ------------------------------------------------------------- |
| `StartupMessage`   | Client → Server | Protocol version, database, user                              |
| `SSLRequest`       | Client → Server | TLS negotiation (most clients try this first)                 |
| `AuthenticationOk` | Server → Client | Auth success (start with `trust`, add real auth later)        |
| `ParameterStatus`  | Server → Client | Report `server_version`, `client_encoding`, `DateStyle`, etc. |
| `BackendKeyData`   | Server → Client | PID + secret key for cancel requests                          |
| `ReadyForQuery`    | Server → Client | Transaction state: `I` (idle), `T` (in txn), `E` (error)      |
| `Terminate`        | Client → Server | Clean disconnect                                              |

**Layer 2 — Simple Query (psql works)**:

| Message           | Direction       | Purpose                                                     |
| ----------------- | --------------- | ----------------------------------------------------------- |
| `Query`           | Client → Server | SQL string                                                  |
| `RowDescription`  | Server → Client | Column names, type OIDs, format codes                       |
| `DataRow`         | Server → Client | Row data (one per row)                                      |
| `CommandComplete` | Server → Client | `SELECT 42`, `SET`, etc.                                    |
| `ErrorResponse`   | Server → Client | Structured error: severity, SQLSTATE, message, detail, hint |

**Layer 3 — Extended Query (real client libraries work)**:

| Message                          | Direction       | Purpose                                     |
| -------------------------------- | --------------- | ------------------------------------------- |
| `Parse`                          | Client → Server | Prepare a statement (SQL + parameter types) |
| `Bind`                           | Client → Server | Bind parameter values to prepared statement |
| `Describe`                       | Client → Server | Get column metadata without executing       |
| `Execute`                        | Client → Server | Run bound statement                         |
| `Sync`                           | Client → Server | End of request pipeline                     |
| `ParseComplete` / `BindComplete` | Server → Client | Acknowledgments                             |
| `CloseComplete`                  | Server → Client | Statement/portal closed                     |

**Layer 4 — Advanced (full compatibility)**:

| Message                                     | Purpose               | Who needs it                    |
| ------------------------------------------- | --------------------- | ------------------------------- |
| `CopyOutResponse` / `CopyData` / `CopyDone` | `COPY TO` bulk export | pg_dump, data tools             |
| `CancelRequest`                             | Cancel running query  | psql (Ctrl+C), connection pools |
| `NotificationResponse`                      | `LISTEN`/`NOTIFY`     | Reject — pg_arrow is read-only  |

### Type OID Mapping

Every column in `RowDescription` must include the correct PostgreSQL type OID. Clients use OIDs
to select decoders:

```
bool        = 16      int2        = 21      int4        = 23
int8        = 20      float4      = 700     float8      = 701
text        = 25      varchar     = 1043    char        = 18
bytea       = 17      date        = 1082    time        = 1083
timestamp   = 1114    timestamptz = 1184    interval    = 1186
numeric     = 1700    uuid        = 2950    jsonb       = 3802
json        = 114     oid         = 26      name        = 19

Arrays: int4[] = 1007, text[] = 1009, float8[] = 1022, etc.
```

pg_arrow needs a mapping from Arrow types back to PostgreSQL OIDs. Each type also needs text-format
and (optionally) binary-format encoders matching PostgreSQL's output.

### Catalog and Session Queries

Many clients auto-query on connection. pg_arrow must answer these or proxy to PostgreSQL:

```sql
-- Every ORM / client library on connect:
SELECT version();
SELECT current_database();
SELECT current_schema();
SELECT current_user;
SHOW server_version;
SHOW server_encoding;
SET client_encoding TO 'UTF8';

-- Schema discovery (DBeaver, pgAdmin, ORMs):
SELECT * FROM pg_catalog.pg_class WHERE relkind IN ('r', 'p', 'v', 'm');
SELECT * FROM pg_catalog.pg_attribute WHERE attrelid = $1;
SELECT * FROM information_schema.tables;
SELECT * FROM information_schema.columns WHERE table_name = $1;

-- Connection pools (PgBouncer, pgpool):
DISCARD ALL;
RESET ALL;

-- Monitoring tools:
SELECT pg_is_in_recovery();
SELECT pg_backend_pid();
SELECT pg_postmaster_start_time();
```

Options: proxy catalog queries to the real PostgreSQL instance, or build an in-memory catalog
from PostgreSQL's data files. Proxying is simpler and always correct.

### Transaction Commands

Even for read-only, clients expect these to work:

```sql
BEGIN;                            -- accept (start read-only snapshot)
BEGIN READ ONLY;                  -- accept
COMMIT;                           -- accept (no-op)
ROLLBACK;                         -- accept (no-op)
SET TRANSACTION ISOLATION LEVEL REPEATABLE READ;  -- accept
SAVEPOINT sp1;                    -- accept or reject gracefully
```

Any write command (`INSERT`, `UPDATE`, `DELETE`, `CREATE`, `DROP`) must return a clear error:

```
ERROR:  cannot execute INSERT in a read-only transaction
SQLSTATE: 25006
```

### Error Protocol

Clients parse structured error fields for retry logic and error display:

```
ErrorResponse fields:
  S (Severity):  ERROR, FATAL, WARNING, NOTICE, INFO
  V (Verbosity): ERROR (always same as S for non-localized)
  C (SQLSTATE):  42P01 (undefined_table), 25006 (read_only), 57014 (query_cancelled)
  M (Message):   "relation \"foo\" does not exist"
  D (Detail):    optional extra context
  H (Hint):      optional suggestion
  P (Position):  character offset in query string
```

Correct SQLSTATE codes matter — `40001` triggers retry in many clients, `57014` means cancelled, etc.

### Compatibility Tiers Summary

| Tier                      | What                                                    | Clients that work                |
| ------------------------- | ------------------------------------------------------- | -------------------------------- |
| 1: Simple Query + Connect | Startup, auth, Query, RowDescription, DataRow, errors   | `psql` only                      |
| 2: Extended Query         | Parse/Bind/Describe/Execute, type OIDs, ParameterStatus | JDBC, psycopg2, asyncpg, node-pg |
| 3: Catalog + Session      | `pg_class`/`pg_type` queries, `SET`/`SHOW`, `version()` | ORMs, DBeaver, pgAdmin           |
| 4: Advanced               | COPY TO, CancelRequest, binary format, SSL/TLS          | pg_dump, production clients      |

## Isolation Levels

PostgreSQL supports 4 isolation levels. For pg_arrow (read-only), only two distinct behaviors
matter — the difference is whether the snapshot refreshes per-statement or is held for the
entire transaction:

| Level                        | PostgreSQL behavior                         | pg_arrow snapshot strategy                                 |
| ---------------------------- | ------------------------------------------- | ---------------------------------------------------------- |
| READ UNCOMMITTED             | Same as READ COMMITTED in PG                | Same as READ COMMITTED                                     |
| **READ COMMITTED** (default) | Each **statement** gets a new snapshot      | Acquire snapshot via `pg_current_snapshot()` per statement |
| **REPEATABLE READ**          | All statements share the **first** snapshot | Acquire snapshot once, reuse for all statements in txn     |
| SERIALIZABLE                 | Same as REPEATABLE READ for read-only       | Same as REPEATABLE READ                                    |

### Why This Matters

In a multi-statement analytical transaction:

```sql
BEGIN ISOLATION LEVEL REPEATABLE READ;
SELECT SUM(amount) FROM orders;          -- snapshot taken at T1
-- PostgreSQL inserts 1000 rows between these two statements
SELECT COUNT(*) FROM orders;             -- must still see T1 snapshot, NOT new rows
COMMIT;
```

If pg_arrow ignores isolation level and always acquires a fresh snapshot (READ COMMITTED behavior),
these two queries could return inconsistent results under REPEATABLE READ.

### Per-Connection Snapshot Tracking

```rust
struct ConnectionState {
    isolation_level: IsolationLevel,
    active_snapshot: Option<Snapshot>,    // None until first statement in txn
    in_transaction: bool,
}

impl ConnectionState {
    fn get_snapshot(&mut self) -> Result<Snapshot> {
        match self.isolation_level {
            IsolationLevel::ReadCommitted => {
                // Always fresh snapshot per statement
                acquire_new_snapshot()
            }
            IsolationLevel::RepeatableRead | IsolationLevel::Serializable => {
                // Reuse snapshot within transaction
                if let Some(snap) = &self.active_snapshot {
                    Ok(snap.clone())
                } else {
                    let snap = acquire_new_snapshot()?;
                    self.active_snapshot = Some(snap.clone());
                    Ok(snap)
                }
            }
        }
    }

    fn on_commit_or_rollback(&mut self) {
        self.active_snapshot = None;
        self.in_transaction = false;
    }
}
```

### Snapshot Acquisition

Getting a correct snapshot (with `xip[]` array of in-progress transactions) requires a
PostgreSQL connection:

```sql
-- pg_arrow calls this on the PostgreSQL connection pool:
SELECT pg_current_snapshot();
-- Returns: '100:105:100,102,104'
--          xmin:xmax:xip_list
-- Meaning: xids 100,102,104 are in-progress; 101,103 committed; >=105 not yet started
```

This is a lightweight call but requires a live PostgreSQL connection. If PostgreSQL is down,
pg_arrow cannot acquire new snapshots and must either reject queries or serve with the last
known snapshot (degraded mode).

## Security Model

> **Context**: pg_arrow reads PostgreSQL heap files directly, which means it **completely bypasses
> PostgreSQL's entire security stack**. This is the most important architectural decision in the
> design — it determines who can see what data through pg_arrow.

### What pg_arrow Bypasses

| PostgreSQL security layer    | What it protects                             | pg_arrow status                                        |
| ---------------------------- | -------------------------------------------- | ------------------------------------------------------ |
| **pg_hba.conf**              | Host-based access control                    | **Bypassed** — pg_arrow has its own listener           |
| **Authentication**           | md5, scram-sha-256, cert, LDAP, etc.         | **Bypassed** — must implement or proxy                 |
| **GRANT/REVOKE**             | Table-level and column-level permissions     | **Bypassed** — heap file read has no permission checks |
| **Row-Level Security (RLS)** | Per-row access policies                      | **Bypassed** — pg_arrow sees ALL rows                  |
| **Column-level permissions** | Hide specific columns from users             | **Bypassed**                                           |
| **Schema permissions**       | Control access to schemas                    | **Bypassed**                                           |
| **`security_barrier` views** | Prevent predicate pushdown information leaks | **Bypassed**                                           |
| **`pgaudit`**                | Audit logging of data access                 | **Bypassed** — no audit trail in pg_arrow              |
| **`pg_read_all_data` role**  | Controlled broad read access                 | **Bypassed**                                           |

### The RLS Problem — Example

```sql
-- PostgreSQL has RLS on the users table:
ALTER TABLE users ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON users
    USING (tenant_id = current_setting('app.tenant_id'));

-- Through PostgreSQL (port 5432) — RLS enforced:
SET app.tenant_id = '42';
SELECT * FROM users;  -- only sees tenant 42's rows

-- Through pg_arrow (port 5433) — RLS bypassed:
SELECT * FROM users;  -- sees ALL tenants' data
```

This is a **data breach** in a multi-tenant application.

### Security Model Options

**Option A: Trusted internal service (recommended for Phase 1)**

pg_arrow is an internal service, not exposed to end users:

- Only accessible from the application backend via private network
- Application enforces its own authorization before sending queries
- pg_arrow trusts all connections (auth = `trust`)
- Firewall rules / network segmentation restrict access to port 5433
- Similar to how applications use internal Redis or Elasticsearch — trusted backend service

```
                    ┌─────────────────┐
Users ──── HTTPS ──→│  Application    │──── private ──→ pg_arrow :5433
                    │  (enforces      │──── private ──→ PostgreSQL :5432
                    │   auth + ACL)   │
                    └─────────────────┘
```

Pros: Simple, fast, no auth overhead in pg_arrow.
Cons: No defense-in-depth. If network segmentation fails, data is exposed.

**Option B: Proxy authentication to PostgreSQL**

pg_arrow forwards client credentials to PostgreSQL for validation:

```
1. Client connects to pg_arrow with username/password
2. pg_arrow attempts: SELECT 1 on PostgreSQL with same credentials
3. If PostgreSQL accepts → pg_arrow allows connection
4. pg_arrow stores the authenticated role for permission checks
```

**Option C: Full permission enforcement (most secure, most complex)**

pg_arrow authenticates users AND enforces PostgreSQL permissions:

```
1. Authenticate via scram-sha-256 (read pg_authid for password hashes)
2. On each query, check table permissions:
   SELECT has_table_privilege($user, $table, 'SELECT') via PG connection
3. Check column permissions:
   SELECT has_column_privilege($user, $table, $col, 'SELECT')
4. For RLS: read policy definitions from pg_policy catalog,
   translate WHERE clauses to DataFusion filters
5. For security_barrier views: ensure predicate ordering matches PG
```

This is correct but adds significant latency and complexity. RLS translation to DataFusion
filters is particularly challenging — policies can reference `current_user`, `current_setting()`,
and other session-dependent functions.

### Recommended Approach

Phase 1-6: **Option A** (trusted internal service). Document the security boundary clearly.
Phase 8: **Option B** (proxy auth) + table-level permission checks.
Future: **Option C** (RLS) only if multi-tenant direct access is needed.

### Audit Logging

Even as a trusted service, pg_arrow should log:

- All connections: who connected, from where, when
- All queries: query text, user, execution time, rows returned
- Access patterns: which tables were read, how many pages

This provides an audit trail independent of PostgreSQL's `pgaudit`.

## Configuration and Cluster Validation

### pg_control — Read on Startup (CRITICAL)

`global/pg_control` is a binary file containing cluster metadata. pg_arrow **must** read this
before reading any data files:

```
Key fields in pg_control:
  pg_control_version        - Must match expected version (PG 15 = 1300, PG 16 = 1300, etc.)
  catalog_version_no        - Catalog version (affects system table layouts)
  system_identifier         - Unique 64-bit cluster ID
  state                     - DB_IN_PRODUCTION, DB_SHUTDOWNED, DB_IN_RECOVERY, etc.
  checkPoint                - Latest checkpoint WAL location
  checkPointCopy.redo       - Redo start point
  blcksz                    - Block size (usually 8192, but compile-time configurable!)
  relseg_size               - Segment file size in blocks (usually 131072 = 1GB)
  xlog_blcksz               - WAL block size
  data_checksum_version     - 0 = disabled, 1 = enabled
  float8ByVal               - Float8 passed by value in tuples
  maxAlign                  - Maximum alignment requirement (4 or 8)
```

**Startup validation**:

```rust
fn validate_pg_control(pg_data_dir: &Path) -> Result<ClusterConfig> {
    let control = read_pg_control(pg_data_dir.join("global/pg_control"))?;

    // Validate state — don't read a crashed cluster
    if control.state != DB_IN_PRODUCTION && control.state != DB_SHUTDOWNED {
        return Err("Cluster is not in a valid state (crashed or in recovery?)");
    }

    // Validate block size — everything breaks if this is wrong
    if control.blcksz != 8192 {
        warn!("Non-standard block size: {}. Adjusting page reader.", control.blcksz);
    }

    // Check data checksums — enables torn page detection
    let checksums_enabled = control.data_checksum_version > 0;

    // Validate PG version is supported
    if control.pg_control_version < MIN_SUPPORTED_VERSION {
        return Err("PostgreSQL version too old");
    }

    Ok(ClusterConfig {
        block_size: control.blcksz as usize,
        segment_size: control.relseg_size as usize * control.blcksz as usize,
        checksums_enabled,
        system_identifier: control.system_identifier,
        float8_by_val: control.float8ByVal,
        max_align: control.maxAlign as usize,
    })
}
```

### PostgreSQL Settings — Read via Connection

Many settings affect data interpretation or output formatting. The most reliable way to read
them is via a PostgreSQL connection:

```sql
SELECT name, setting FROM pg_settings
WHERE name IN (
    'integer_datetimes',          -- timestamp storage format (always 'on' since PG 10)
    'server_encoding',            -- database encoding (affects text data interpretation)
    'lc_collate',                 -- collation (affects ORDER BY on text)
    'lc_ctype',                   -- character classification
    'timezone',                   -- session timezone for timestamptz display
    'DateStyle',                  -- date output format (ISO, SQL, Postgres, German)
    'IntervalStyle',              -- interval output format
    'bytea_output',               -- hex or escape
    'extra_float_digits',         -- float precision in text output
    'standard_conforming_strings' -- backslash handling in string literals
);
```

**Settings that affect binary data format** (misinterpret data if wrong):

| Setting                    | Effect                                                   | Risk                                                           |
| -------------------------- | -------------------------------------------------------- | -------------------------------------------------------------- |
| `integer_datetimes`        | Timestamps as int64 microseconds vs float8               | **Every timestamp wrong** if misread. Always `on` since PG 10. |
| `server_encoding`          | Encoding of text in heap files                           | **Garbled text** if not UTF-8 and not transcoded               |
| `float8ByVal` (pg_control) | Whether float8 is stored by value or reference in tuples | **Wrong float values** if misread                              |
| `maxAlign` (pg_control)    | Tuple data alignment                                     | **Wrong column offsets** if misread                            |

**Settings that affect text output** (wrong display format but data is correct):

| Setting              | Effect                                       | Default      |
| -------------------- | -------------------------------------------- | ------------ |
| `DateStyle`          | `2024-01-15` vs `01/15/2024` vs `15.01.2024` | `ISO, MDY`   |
| `IntervalStyle`      | `1 year 2 mons` vs `1-2` vs `P1Y2M`          | `postgres`   |
| `timezone`           | `timestamptz` display timezone               | System TZ    |
| `bytea_output`       | `\x48656c6c6f` vs `Hello`                    | `hex`        |
| `extra_float_digits` | Float text precision                         | `1` (PG 12+) |

### Configuration File Structure

```
$PGDATA/
├── postgresql.conf           ← Main config (text, parseable but complex — supports includes)
├── postgresql.auto.conf      ← ALTER SYSTEM overrides (same format, read second)
├── pg_hba.conf               ← Host-based authentication rules
├── pg_ident.conf             ← External username mapping
└── global/pg_control         ← Binary cluster metadata (most critical)
```

**Recommended reading order**:

```
1. pg_control         → binary, read directly, validate cluster basics
2. PostgreSQL conn    → SELECT from pg_settings (get ALL active settings correctly)
3. pg_arrow.toml      → pg_arrow's own operational config
```

Reading `pg_settings` via connection is preferred over parsing `postgresql.conf` because:

- `postgresql.conf` can include other files (`include`, `include_dir`)
- `postgresql.auto.conf` overrides `postgresql.conf`
- Some settings have complex defaults or depend on other settings
- `pg_settings` gives the actual active values after all resolution

## Segment Files

PostgreSQL splits tables larger than 1GB (configurable via `relseg_size` in `pg_control`) into
numbered segment files:

```
base/16384/24601          ← segment 0: pages 0 to 131071 (1GB)
base/16384/24601.1        ← segment 1: pages 131072 to 262143 (1GB)
base/16384/24601.2        ← segment 2: pages 262144 to 393215 (1GB)
...
```

Each segment contains contiguous 8KB pages. The segment number is `page_number / relseg_size`.
The default `relseg_size` is 131072 blocks (131072 \* 8192 = 1GB).

**pg_arrow must iterate across all segments**. The current page reader must be segment-aware:

```rust
fn read_page(&self, page_num: u32) -> Result<Page> {
    let pages_per_segment = self.cluster_config.segment_size / self.cluster_config.block_size;
    let segment_num = page_num / pages_per_segment as u32;
    let page_in_segment = page_num % pages_per_segment as u32;

    let segment_path = if segment_num == 0 {
        self.base_path.clone()                                    // base/16384/24601
    } else {
        PathBuf::from(format!("{}.{}", self.base_path.display(), segment_num))  // base/16384/24601.1
    };

    let offset = page_in_segment as u64 * self.cluster_config.block_size as u64;
    // read block_size bytes from segment_path at offset
}

fn total_pages(&self) -> Result<u32> {
    let mut total = 0u32;
    let mut seg = 0;
    loop {
        let path = if seg == 0 {
            self.base_path.clone()
        } else {
            PathBuf::from(format!("{}.{}", self.base_path.display(), seg))
        };
        match std::fs::metadata(&path) {
            Ok(meta) => {
                total += (meta.len() / self.cluster_config.block_size as u64) as u32;
                seg += 1;
            }
            Err(_) => break,  // No more segments
        }
    }
    Ok(total)
}
```

**Without segment file support, pg_arrow cannot read any table larger than 1GB.** This is the
most critical missing feature from the original design.

## Torn Page Detection

If pg_arrow reads a page while PostgreSQL's background writer is flushing it, the read could
return a partially-written page. While most modern filesystems provide atomic 8KB aligned writes
in practice, POSIX does not guarantee this.

### Detection via Data Checksums

If data checksums are enabled (`data_checksum_version > 0` in `pg_control`), each page header
contains a `pd_checksum` field (16-bit):

```rust
fn verify_page_checksum(page: &[u8], block_num: u32) -> Result<()> {
    let stored_checksum = u16::from_le_bytes([page[8], page[9]]);  // pd_checksum offset

    // Zero out pd_checksum in the data before computing
    let mut page_copy = page.to_vec();
    page_copy[8] = 0;
    page_copy[9] = 0;

    let computed = pg_checksum_page(&page_copy, block_num);

    if stored_checksum != computed {
        // Torn page detected — retry the read
        return Err(Error::TornPage { block_num, expected: stored_checksum, got: computed });
    }
    Ok(())
}

// pg_arrow page read with retry:
fn read_page_safe(&self, page_num: u32) -> Result<Page> {
    for attempt in 0..3 {
        let page = self.read_page_raw(page_num)?;
        if !self.cluster_config.checksums_enabled {
            return Ok(page);  // No checksums — can't detect torn pages
        }
        match verify_page_checksum(&page, page_num) {
            Ok(()) => return Ok(page),
            Err(e) if attempt < 2 => {
                // Retry — writer was probably mid-flush
                std::thread::sleep(Duration::from_micros(100));
                continue;
            }
            Err(e) => return Err(e),  // Persistent failure — real corruption
        }
    }
    unreachable!()
}
```

### Without Checksums

If checksums are disabled, pg_arrow has no reliable torn page detection. Options:

- Accept the (low) risk — torn pages are rare on modern hardware/filesystems
- Validate page header sanity: check `pd_lsn`, `pd_lower`, `pd_upper` are within valid ranges
- Read pages twice and compare (expensive, paranoid)

### PostgreSQL's Full-Page Writes

PostgreSQL's `full_page_writes = on` (default) writes complete page images to WAL after each
checkpoint. This protects PostgreSQL from torn pages during recovery, but does NOT protect
external readers like pg_arrow — it only helps if you replay WAL.

## WAL File Physical Format

> **Context**: pg_arrow needs a WAL parser for read consistency (replaying WAL records onto stale
> heap pages) and cache invalidation (knowing which pages changed). This section summarizes the
> key binary structures. Full implementation-ready reference with exact byte offsets, Rust struct
> definitions, and decoding algorithms is in `RESEARCH/WAL_FORMAT.md` (~1900 lines).

### WAL File Organization

WAL files live in `$PGDATA/pg_wal/`. Each file is a "segment" — default 16MB, containing
2048 pages of 8KB each. Segment filenames are 24 hex characters encoding
`(TimeLineID, log_id, seg_id)`.

```
$PGDATA/pg_wal/
  000000010000000000000001   ← segment 1, timeline 1 (16MB)
  000000010000000000000002   ← segment 2
  ...

Segment internals (16MB):
  Page 0:    [LongPageHeader 40B]  [record data...]
  Page 1:    [ShortPageHeader 24B] [record data...]
  ...
  Page 2047: [ShortPageHeader 24B] [record data...]
```

### LSN Arithmetic

An LSN (`XLogRecPtr = uint64`) is a byte offset into the abstract WAL stream:

```rust
// LSN → segment file + offset
let segment_number = lsn / wal_segment_size;   // default wal_segment_size = 16MB
let segment_offset = lsn & (wal_segment_size - 1);
let page_number    = segment_offset / 8192;    // XLOG_BLCKSZ
let page_offset    = segment_offset % 8192;

// LSN → segment filename
let segments_per_xlog_id = 0x100000000u64 / wal_segment_size;
let log_id  = (segment_number / segments_per_xlog_id) as u32;
let seg_id  = (segment_number % segments_per_xlog_id) as u32;
let filename = format!("{:08X}{:08X}{:08X}", timeline_id, log_id, seg_id);

// Advance past a record
let next_lsn = lsn + maxalign(xl_tot_len);  // MAXALIGN = round up to 8
```

### WAL Page Header

Every 8KB WAL page starts with a header. First page per segment = long header (40B),
all others = short header (24B):

```
XLogPageHeaderData (short — 24 bytes):
  Offset  Size  Field         Description
   0       2    xlp_magic     Version indicator (e.g., 0xD118 = PG18)
   2       2    xlp_info      Flags (XLP_LONG_HEADER, XLP_FIRST_IS_CONTRECORD, ...)
   4       4    xlp_tli       TimeLineID
   8       8    xlp_pageaddr  LSN of this page's start
  16       4    xlp_rem_len   Remaining bytes from previous page's record
  (MAXALIGN to 24 bytes)

XLogLongPageHeaderData (long — extends short with):
  20       8    xlp_sysid     System identifier (from pg_control)
  28       4    xlp_seg_size  WAL segment size (cross-check)
  32       4    xlp_xlog_blcksz  WAL block size (cross-check, = 8192)
  (MAXALIGN to 40 bytes)
```

**XLOG_PAGE_MAGIC version detection**:

| PG Version | Magic  | Key change |
|------------|--------|------------|
| PG 14      | 0xD110 | Baseline |
| PG 15      | 0xD113 | LZ4/ZSTD FPI compression |
| PG 16      | 0xD114 | RelFileNode → RelFileLocator (same binary layout) |
| PG 17      | 0xD116 | Unified prune/freeze WAL records |
| PG 18      | 0xD118 | xl_heap_prune: uint8 reason + uint8 flags |
| PG master  | 0xD11A | xl_heap_prune flags → uint16, VM bits in prune |

### WAL Record Header (XLogRecord — 24 bytes)

```
Offset  Size  Field       Description
 0       4    xl_tot_len  Total length of entire record (header + all data)
 4       4    xl_xid      Transaction ID
 8       8    xl_prev     LSN of previous record
16       1    xl_info     Low 4 bits: internal flags; High 4 bits: rmgr-specific opcode
17       1    xl_rmid     Resource manager ID (10 = RM_HEAP_ID, 9 = RM_HEAP2_ID)
18       2    (padding)
20       4    xl_crc      CRC-32C of entire record
```

### Record Structure — Two-Phase Layout

After the 24-byte header, a WAL record has **headers first, then data**:

```
[XLogRecord — 24 bytes]
[Block Reference Headers]     ← parsed first to learn sizes and targets
  [BlockHeader 0: 4B base + optional FPI header + RelFileLocator + BlockNumber]
  [BlockHeader 1: ...]
  ...
[Main Data Header]            ← XLogRecordDataHeaderShort (2B) or Long (5B)
[Block 0 FPI data]            ← full-page image bytes (if present)
[Block 0 block data]          ← rmgr-specific per-block payload
[Block 1 FPI data]
[Block 1 block data]
...
[Main data]                   ← rmgr-specific main record payload
```

**Key insight**: Headers and data are separated. Parse all headers first to learn sizes,
then read data payloads. Fields within headers are NOT aligned — use byte-slice parsing
(`from_ne_bytes`), not pointer casts.

### Block Reference Header

Each block reference identifies `(RelFileLocator, ForkNumber, BlockNumber)`:

```
Base header (4 bytes):
  0: uint8  block_id     (0-32)
  1: uint8  fork_flags   (low 4: fork, high 4: HAS_IMAGE|HAS_DATA|WILL_INIT|SAME_REL)
  2: uint16 data_length  (per-block payload size)

Conditional extensions:
  IF HAS_IMAGE:  +5B XLogRecordBlockImageHeader (bimg_len, hole_offset, bimg_info)
    IF compressed AND has_hole: +2B XLogRecordBlockCompressHeader (hole_length)
  IF NOT SAME_REL: +12B RelFileLocator (spcOid, dbOid, relNumber)
  ALWAYS: +4B BlockNumber
```

### Heap WAL Record Types (RM_HEAP_ID = 10, RM_HEAP2_ID = 9)

The opcode is `xl_info & 0x70`. Bit 7 (`0x80`) = page re-initialized.

| rmgr | Opcode | xl_info & 0x70 | Main data struct | Size | pg_arrow relevance |
|------|--------|---------------|-----------------|------|-------------------|
| HEAP | INSERT | 0x00 | `xl_heap_insert` | 3B | Critical — new rows |
| HEAP | DELETE | 0x10 | `xl_heap_delete` | 8B | Critical — row removal |
| HEAP | UPDATE | 0x20 | `xl_heap_update` | 14B | Critical — modified rows |
| HEAP | HOT_UPDATE | 0x40 | `xl_heap_update` | 14B | Critical — same-page update |
| HEAP | LOCK | 0x60 | `xl_heap_lock` | 8B | Needed — modifies xmax/infomask |
| HEAP | CONFIRM | 0x50 | `xl_heap_confirm` | 2B | Rare — speculative inserts |
| HEAP | INPLACE | 0x70 | variable | var | System catalogs only |
| HEAP2 | MULTI_INSERT | 0x50 | `xl_heap_multi_insert` | 3B+ | Critical — COPY operations |
| HEAP2 | PRUNE_* | 0x10/20/30 | `xl_heap_prune` | 2B+ | Important — vacuum cleanup |
| HEAP2 | VISIBLE | 0x40 | `xl_heap_visible` | 5B | VM optimization |

### Full Page Images (FPI)

When `full_page_writes = on` (default), the first modification to a page after a checkpoint
writes the entire 8KB page image into WAL. The image has the "hole" (zero-filled gap between
line pointers and tuple data) removed to save space. PG15+ supports LZ4/ZSTD compression.

**For pg_arrow**: When an FPI is found for a target page, use it directly as the page content.
All prior WAL records for that page become irrelevant — the FPI is a complete snapshot.

```
FPI restoration:
  1. Read bimg_len bytes from WAL data portion
  2. If compressed → decompress (pglz, lz4, or zstd based on bimg_info flags)
  3. Restore hole: copy bytes before hole_offset, leave hole_length zeros, copy bytes after
  4. Result: complete 8KB page
```

### Scanning WAL for a Specific Page

WAL has no index by `(relfilelocator, blkno)` — must scan linearly:

```
ScanWalForPage(target_rel, target_fork, target_blkno, start_lsn, end_lsn):
  FOR each record R where start_lsn <= R.lsn < end_lsn:
    Skip if R.xl_rmid not in {RM_HEAP_ID, RM_HEAP2_ID}  ← fast pre-filter
    Decode block reference headers
    FOR each block ref B:
      IF B.rlocator == target_rel AND B.forknum == target_fork AND B.blkno == target_blkno:
        IF B.has_image → DISCARD all prior records, COLLECT as FPI
        ELSE → COLLECT as delta record
  RETURN collected records in LSN order
```

**Performance mitigations**: Pre-filter by rmgr ID (skip non-heap records by jumping
`xl_tot_len`), batch-scan for multiple target pages in one pass, FPI short-circuit
(discard prior records when FPI found), checkpoint awareness (page `pd_lsn` tells you
exactly where to start scanning).

### Continuation Records

Records larger than remaining page space span multiple pages. The next page has
`XLP_FIRST_IS_CONTRECORD` set and `xlp_rem_len` indicates remaining bytes. Must
reassemble the record from multiple pages before decoding.

### WAL Parsing Complexity and Version Handling

WAL format changes between major PostgreSQL versions. Detect version from `xlp_magic` in
the first page header. There are only **two real breaking changes**:

**Breaking Change 1 — FPI Compression (PG15+)**: PG14 only supports `pglz`. PG15 added
LZ4 (`bimg_info & 0x08`) and ZSTD (`bimg_info & 0x10`). Only matters if cluster has
`wal_compression = lz4|zstd` (not default).

**Breaking Change 2 — HEAP2 Opcode Shift (PG17+)**: PG17 unified vacuum/freeze into
`PRUNE_*` records and **shifted all HEAP2 opcodes from VISIBLE onwards down by 0x10**:

```
Opcode    PG14-16           PG17+
0x30      CLEAN             PRUNE_VACUUM_CLEANUP
0x40      FREEZE_PAGE       VISIBLE              ← was 0x50
0x50      VISIBLE           MULTI_INSERT         ← was 0x60
0x60      MULTI_INSERT      LOCK_UPDATED         ← was 0x70
0x70      LOCK_UPDATED      NEW_CID              ← was 0x80
0x80      NEW_CID           (unused)
```

A parser hardcoding PG14-16 opcodes will misinterpret every VISIBLE, MULTI_INSERT,
LOCK_UPDATED, and NEW_CID record on PG17+.

**What's stable across ALL versions**: XLogRecord header (24B), page headers, block
reference headers, RelFileLocator (12B), INSERT/DELETE/UPDATE/HOT_UPDATE/LOCK opcodes
and payload structs, LSN arithmetic, CRC-32C, continuation records.

**Recommended strategy**: Start with PG17/18 support (simpler unified prune/freeze,
avoids PG14-16 vacuum/freeze structs). Use Neon project's `postgres_ffi` crate as
reference for version-specific struct definitions. Implement the minimal record set
(INSERT, DELETE, UPDATE, HOT_UPDATE, MULTI_INSERT, FPI) and add LOCK, PRUNE, VISIBLE
as needed. Validate output against `pg_waldump`.

> **Full reference**: See `RESEARCH/WAL_FORMAT.md` for complete Rust struct definitions,
> all constants, the full decoding algorithm, version-specific handling code, and the
> FPI restoration implementation.

## Database Encoding

PostgreSQL databases have a character encoding set at creation time:

```sql
SELECT datname, pg_encoding_to_char(encoding) FROM pg_database;
-- mydb | UTF8
-- legacy_db | LATIN1
-- jp_db | EUC_JP
```

Text data in heap files is stored in the database's encoding. Arrow strings are **always UTF-8**.

### Encoding Handling

```
If server_encoding == UTF8:
  → No transcoding needed. Pass text bytes directly to Arrow.
  → Validate UTF-8 (PostgreSQL allows some invalid sequences in certain configs).

If server_encoding == LATIN1 (ISO-8859-1):
  → Transcode every text value to UTF-8.
  → LATIN1 → UTF-8 is always valid (every byte maps to a Unicode codepoint).

If server_encoding == EUC_JP, EUC_KR, SJIS, etc.:
  → Transcode using iconv or encoding_rs crate.
  → Some characters may not have UTF-8 equivalents — handle errors.

If server_encoding == SQL_ASCII:
  → No encoding guarantee at all. Data could be anything.
  → Best effort: treat as UTF-8, replace invalid bytes with U+FFFD.
```

Most modern PostgreSQL deployments use UTF-8. But pg_arrow should detect the encoding on startup
and fail fast if it encounters an unsupported encoding rather than silently producing garbled data.

### Collation

`lc_collate` affects string comparison and `ORDER BY` on text columns. DataFusion uses Rust's
default Unicode ordering, which may differ from PostgreSQL's locale-aware collation. For
analytical queries this is usually acceptable, but results may differ from PostgreSQL for edge
cases (e.g., `ä` sorting relative to `a` in German locale).

## Concurrent DDL Safety

PostgreSQL can execute DDL while pg_arrow is reading data files. Most DDL operations modify
catalog tables but not heap files. However, some replace or remove heap files entirely:

| DDL Operation               | Effect on Heap Files                                                       | Risk for pg_arrow                                                                                              |
| --------------------------- | -------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| `ALTER TABLE ADD COLUMN`    | Existing tuples unchanged (new default stored in `pg_attrdef`, not tuples) | **Schema mismatch**: pg_arrow reads old tuples but schema now has new column. Must detect via catalog re-read. |
| `ALTER TABLE DROP COLUMN`   | Column marked `attisdropped` in `pg_attribute`, data stays in tuples       | **Low risk**: dropped columns still in tuples, just skip them. Read `pg_attribute.attisdropped`.               |
| `ALTER TABLE ALTER TYPE`    | May rewrite table (new `relfilenode`)                                      | **File replaced**: same risk as VACUUM FULL.                                                                   |
| `DROP TABLE`                | Heap file unlinked                                                         | **File disappears**: open fd stays valid (Unix semantics), but new scans fail.                                 |
| `TRUNCATE`                  | Old file unlinked, new empty file created                                  | **File replaced**: mid-read returns old data (ok), new scan sees empty table.                                  |
| `VACUUM FULL`               | Rewrites entire table to new `relfilenode`                                 | **File replaced**: old file unlinked, new file with different OID.                                             |
| `CLUSTER`                   | Rewrites table in index order, new `relfilenode`                           | **File replaced**: same as VACUUM FULL.                                                                        |
| `REINDEX`                   | Only modifies index files                                                  | **Safe**: pg_arrow doesn't read indexes.                                                                       |
| `CREATE INDEX`              | Creates new index files, may briefly lock table                            | **Safe**: pg_arrow doesn't read indexes.                                                                       |
| `CREATE INDEX CONCURRENTLY` | No table lock                                                              | **Safe**.                                                                                                      |

### Detection Strategy

pg_arrow should track `pg_class.relfilenode` for cached tables. If `relfilenode` changes, the
physical file was replaced (VACUUM FULL, CLUSTER, ALTER TYPE, TRUNCATE):

```rust
struct TableFileMapping {
    table_oid: u32,
    relfilenode: u32,         // from pg_class
    cached_at: Instant,
}

fn check_file_still_valid(&self, table: &TableFileMapping) -> Result<bool> {
    // Query PostgreSQL: SELECT relfilenode FROM pg_class WHERE oid = $1
    let current_relfilenode = self.query_relfilenode(table.table_oid)?;
    Ok(current_relfilenode == table.relfilenode)
}
```

### Unix File Descriptor Semantics

On Unix, if pg_arrow has an open file descriptor to a heap file and PostgreSQL unlinks it
(DROP TABLE, VACUUM FULL), the fd remains valid — pg_arrow can finish reading the old data.
The disk space is freed only when the last fd is closed. This means in-progress scans complete
correctly, but new scans must detect the change.

## PostgreSQL Background Processes

PostgreSQL runs several background processes that modify data files pg_arrow reads. Understanding
their behavior is important for correctness and performance.

### Processes That Modify Data Files

**Background Writer (`bgwriter`)**:

- Periodically flushes dirty pages from shared buffers to heap files
- This is how new/updated data becomes visible on disk
- Configuration: `bgwriter_delay` (default 200ms), `bgwriter_lru_maxpages`
- Risk for pg_arrow: reading a page mid-flush (torn page — see checksum section)
- pg_arrow does NOT need to interact with bgwriter. Just reads what's on disk.

**Checkpointer**:

- Flushes ALL dirty buffers at checkpoint intervals
- Updates `pg_control` with checkpoint LSN and timestamp
- Configuration: `checkpoint_timeout` (default 5min), `max_wal_size`
- **Useful for pg_arrow**: After a checkpoint, all data up to the checkpoint LSN is guaranteed
  on disk. The checkpoint LSN in `pg_control` serves as a conservative lower bound for
  "everything before this is safely flushed."
- pg_arrow could read `pg_control` to get the last checkpoint LSN as a safe snapshot bound
  without requiring a PostgreSQL connection (though `pg_current_snapshot()` is more precise).

**Autovacuum** (and manual VACUUM):

- Runs automatically based on table modification thresholds
- Configuration: `autovacuum_vacuum_threshold`, `autovacuum_vacuum_scale_factor`
- **What it does to files pg_arrow reads**:
  - Sets hint bits on tuples (`XMIN_COMMITTED`, `XMAX_INVALID`, etc.) — **good for pg_arrow**,
    reduces CLOG lookups
  - Updates visibility map (`_vm`) — marks pages as all-visible/all-frozen — **good for pg_arrow**,
    enables fast-path visibility skipping
  - Reclaims dead tuple space (updates line pointers, free space map) — neutral for pg_arrow
  - VACUUM FREEZE: sets `XMIN_FROZEN` on old tuples — **excellent for pg_arrow**, these tuples
    never need visibility checks
- **VACUUM FULL**: rewrites entire table — **dangerous**, see Concurrent DDL section
- Net effect: autovacuum progressively makes pg_arrow faster over time. More hint bits, more
  frozen pages, more all-visible VM entries.

**WAL Writer**:

- Flushes WAL buffers to `pg_wal/` files
- pg_arrow monitors WAL LSN for cache invalidation
- pg_arrow does NOT need to read WAL contents (unless doing WAL-based change detection)

### Processes That Don't Modify Data Files

These PostgreSQL processes exist but don't affect files pg_arrow reads:

| Process                         | Purpose                                           | pg_arrow relevance                                                                                              |
| ------------------------------- | ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| **WAL Archiver**                | Copies WAL segments to archive location           | None — pg_arrow doesn't use archived WAL                                                                        |
| **WAL Sender**                  | Streams WAL to replicas                           | None — unless pg_arrow uses streaming replication protocol in future                                            |
| **WAL Receiver**                | Receives WAL on replica                           | None — pg_arrow reads a primary's data dir                                                                      |
| **Logical Replication Workers** | Apply logical decoding changes                    | None                                                                                                            |
| **Parallel Query Workers**      | Forked workers for parallel queries               | None — PostgreSQL's parallel queries don't affect pg_arrow                                                      |
| **Stats Collector**             | Tracks table/index usage statistics (`pg_stat_*`) | pg_arrow could read `pg_statistic` for query optimization hints (row counts, distinct values, histogram bounds) |
| **Startup Process** (recovery)  | Replays WAL during recovery                       | Relevant only if pg_arrow reads a recovering cluster (should refuse — check `pg_control.state`)                 |
| **Archiver**                    | Archives WAL files for PITR                       | None                                                                                                            |

### autovacuum Is pg_arrow's Best Friend

It's worth emphasizing: the more aggressively autovacuum runs, the better pg_arrow performs.
Consider tuning these for pg_arrow workloads:

```
autovacuum_vacuum_scale_factor = 0.01    -- VACUUM at 1% dead tuples (default 20%)
autovacuum_freeze_max_age = 100000000    -- Freeze earlier
vacuum_freeze_min_age = 10000000         -- Freeze recent transactions sooner
```

More frozen tuples = more visibility map all-frozen pages = pg_arrow skips visibility checks
for more of the table.

## pg_arrow Background Jobs

Beyond reading PostgreSQL's data, pg_arrow needs its own background tasks:

### 1. Cluster Health Monitor

```
On startup:
  1. Read pg_control → validate version, block_size, checksums, state
  2. Verify state == DB_IN_PRODUCTION
  3. Connect to PostgreSQL → read pg_settings, cache configuration
  4. Read pg_database → validate target database, get encoding/locale

Periodically (every 30s):
  1. Ping PostgreSQL connection (SELECT 1)
  2. Re-read pg_control (check for unexpected state changes)
  3. If PostgreSQL is down:
     - Log warning, enter degraded mode
     - Continue serving queries with last known snapshot
     - Cannot acquire new snapshots — reject new transactions or use stale snapshot
  4. If PostgreSQL restarts (system_identifier matches, state == DB_IN_PRODUCTION):
     - Reconnect, invalidate all caches, re-read configuration
  5. If system_identifier changes:
     - FATAL: different cluster. Refuse to serve. Operator must reconfigure.
```

### 2. WAL Position Monitor (already in design doc, refined)

```
Periodically (every 100ms, configurable):
  1. Read current WAL LSN (via pg_current_wal_lsn() or pg_control)
  2. If LSN advanced beyond cached range:
     - Invalidate schema cache entries (DDL may have occurred)
     - Invalidate page cache entries for affected tables
     - Update visibility map cache (VACUUM may have run)
```

### 3. Schema Cache Manager

```
On startup:
  - Read pg_class, pg_attribute, pg_type, pg_namespace for all user tables
  - Cache: table OID → (relfilenode, column definitions, type info)
  - Read pg_inherits for partition relationships
  - Read pg_partitioned_table for partition bounds

On invalidation (WAL LSN advanced, or periodic refresh every 5 minutes):
  - Check pg_class.relfilenode for changes (DDL detected)
  - Re-read pg_attribute for tables with changed relfilenode
  - Refresh partition metadata if parent table changed

On explicit DISCARD/RESET from client:
  - Refresh session-level caches
```

### 4. Visibility Map Monitor

```
Periodically (every 5-10s):
  - Re-read _vm files for hot/cached tables
  - After autovacuum runs, more pages become all-frozen
  - Update in-memory VM bitmaps
  - Effect: queries progressively get faster as autovacuum freezes more pages
```

### 5. pg_arrow Statistics Collector

```
Track per-query:
  - Query text (truncated), execution time, rows returned
  - Pages read from disk vs served from cache
  - CLOG lookups performed, hint bit hit rate
  - TOAST detoasting count, bytes decompressed, decompress time
  - Partition pruning stats (scanned vs pruned)
  - Snapshot acquisition latency

Expose via:
  - Internal system tables (queryable via SQL on pg_arrow port):
      SELECT * FROM pg_arrow_stat_queries;
      SELECT * FROM pg_arrow_stat_tables;
  - Prometheus metrics endpoint (optional)
  - Log-based metrics for aggregation
```

### 6. PostgreSQL Connection Pool

pg_arrow needs persistent connections to PostgreSQL for:

- Snapshot acquisition (`pg_current_snapshot()`)
- Catalog queries (`pg_class`, `pg_type`, `pg_attribute`, etc.)
- Authentication proxying (Option B/C security model)
- Configuration reading (`pg_settings`)
- Permission checking (`has_table_privilege`, etc.)

```rust
struct PgConnectionPool {
    pool: deadpool_postgres::Pool,  // or bb8, or mobc
    max_connections: usize,          // Small: 5-10 connections
}
```

This pool must be separate from pg_arrow's client-facing connections and should be resilient
to PostgreSQL restarts (reconnect with backoff).

### 7. Warm-Up / Pre-Fetch (Optional)

```
On startup or config-driven:
  - Pre-read visibility maps for configured hot tables
  - Pre-read TOAST table metadata (locate TOAST heap files)
  - Pre-read pg_xact (CLOG) pages for recent transaction range
  - Optionally pre-scan and cache pages for small hot tables
  - Build initial schema cache

This reduces first-query latency after pg_arrow startup.
```

## PostgreSQL Features — Considered and Excluded

The following PostgreSQL features have been reviewed and explicitly excluded from pg_arrow's
scope. This section documents the reasoning so future contributors don't re-investigate them.

### Excluded: Not Relevant to Read-Only Analytics

| Feature                            | What it is                                      | Why excluded                                                                                                                      |
| ---------------------------------- | ----------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| **WAL writing**                    | Write-ahead log generation                      | pg_arrow is read-only, never writes WAL                                                                                           |
| **Buffer manager**                 | Shared memory page cache with LRU               | pg_arrow has its own caching strategy; doesn't share PG's shared_buffers                                                          |
| **Lock manager**                   | Row/table/advisory locks                        | pg_arrow is read-only, no locks needed for reads (reads committed data from disk)                                                 |
| **Deadlock detector**              | Detects lock cycles                             | No locks → no deadlocks                                                                                                           |
| **Two-phase commit (2PC)**         | `PREPARE TRANSACTION` / `COMMIT PREPARED`       | pg_arrow doesn't participate in distributed transactions                                                                          |
| **Savepoints**                     | `SAVEPOINT` / `ROLLBACK TO` within transactions | pg_arrow is read-only; accepts the command but it's a no-op                                                                       |
| **LISTEN/NOTIFY**                  | Asynchronous event notification                 | pg_arrow could potentially use this for cache invalidation, but WAL monitoring is simpler. Reject `LISTEN`/`NOTIFY` from clients. |
| **Sequences**                      | `nextval()`, `currval()`, `setval()`            | `nextval` is a write — reject it. `currval` could be proxied but low value for analytics.                                         |
| **Advisory locks**                 | `pg_advisory_lock()` / `pg_try_advisory_lock()` | Some ORMs use these for migrations — reject or proxy to PostgreSQL                                                                |
| **COPY FROM**                      | Bulk data import                                | Write operation — reject. (COPY TO is supported for export.)                                                                      |
| **Trigger execution**              | BEFORE/AFTER triggers                           | pg_arrow doesn't execute DML, so triggers never fire                                                                              |
| **Constraint checking**            | CHECK, UNIQUE, FK enforcement                   | Read-only — no data modification to validate                                                                                      |
| **Rule system**                    | `pg_rewrite` rules (non-view)                   | Complex, rarely used. Views are handled separately.                                                                               |
| **Event triggers**                 | DDL event hooks                                 | pg_arrow doesn't execute DDL                                                                                                      |
| **Foreign data wrappers**          | `postgres_fdw`, `file_fdw`, etc.                | Would require implementing the FDW protocol. Out of scope.                                                                        |
| **Logical decoding**               | Change data capture via replication slots       | pg_arrow reads heap files directly, doesn't need CDC                                                                              |
| **Streaming replication protocol** | Physical/logical replication                    | pg_arrow is not a replica. Reads data dir directly.                                                                               |

### Excluded: PostgreSQL Internal Subsystems

| Subsystem                        | What it does                                      | Why excluded                                                                                                                                                                                                 |
| -------------------------------- | ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Planner/Optimizer**            | PostgreSQL's query planner (cost-based optimizer) | pg_arrow uses DataFusion's optimizer instead                                                                                                                                                                 |
| **Executor**                     | PostgreSQL's Volcano-model row-at-a-time executor | pg_arrow uses DataFusion's vectorized columnar executor                                                                                                                                                      |
| **Index access methods**         | B-tree, Hash, GiST, GIN, BRIN, SP-GiST            | pg_arrow does full table scans via heap files. Could use BRIN for page-range filtering in future (BRIN stores min/max per page range).                                                                       |
| **Buffer pool / shared_buffers** | In-memory page cache shared across backends       | pg_arrow has its own page cache. Cannot share PG's because pg_arrow is a separate process.                                                                                                                   |
| **SLRU caches**                  | Simple LRU caches for CLOG, MultiXact, etc.       | pg_arrow reads the on-disk files directly, not the in-memory caches                                                                                                                                          |
| **Proc array**                   | Shared memory array of active backend processes   | Contains running transaction info. pg_arrow could read this for snapshot info, but it's complex, version-dependent, and requires shared memory attachment. Using `pg_current_snapshot()` via SQL is simpler. |
| **Catalog caches**               | In-memory caches of system catalog tuples         | pg_arrow reads catalogs via SQL connection, caches independently                                                                                                                                             |
| **Relation cache**               | Cached relation metadata (RelationData)           | pg_arrow builds its own table metadata from pg_class/pg_attribute                                                                                                                                            |
| **Type cache**                   | Cached type metadata (TypeCacheEntry)             | pg_arrow builds its own type mapping                                                                                                                                                                         |
| **PL/pgSQL interpreter**         | Procedural language for stored functions          | Cannot execute PL/pgSQL — queries calling PL/pgSQL functions must fall back to PostgreSQL                                                                                                                    |
| **PL/Python, PL/Perl, etc.**     | Other procedural languages                        | Same as PL/pgSQL — fallback to PostgreSQL                                                                                                                                                                    |
| **Extension infrastructure**     | `CREATE EXTENSION`, shared library loading        | pg_arrow doesn't load PostgreSQL extensions. Extension-dependent queries fall back to PostgreSQL.                                                                                                            |
| **Background worker framework**  | Custom background processes in PG                 | pg_arrow is external to PostgreSQL, has its own process model                                                                                                                                                |
| **Shared memory management**     | `shmget`/`mmap` for PG's shared memory            | pg_arrow does not attach to PostgreSQL's shared memory. All communication is via files and SQL connection.                                                                                                   |
| **Postmaster**                   | PostgreSQL's main process (fork model)            | pg_arrow has its own process/thread model (likely tokio async)                                                                                                                                               |

### Excluded: Storage Features Not Needed for Analytics

| Feature                      | What it is                                                  | Why excluded                                                                                                                                       |
| ---------------------------- | ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Free Space Map (`_fsm`)**  | Tracks free space per page for INSERT placement             | pg_arrow is read-only — never needs to find free space                                                                                             |
| **GIN/GiST/SP-GiST indexes** | Full-text search, geometric, pattern indexes                | pg_arrow doesn't use indexes. Full-text search queries should go to PostgreSQL.                                                                    |
| **BRIN indexes**             | Block Range INdex (min/max per page range)                  | _Partially relevant_: pg_arrow could read BRIN to skip page ranges. Low priority.                                                                  |
| **Hash indexes**             | Hash-based index access                                     | pg_arrow doesn't use indexes                                                                                                                       |
| **pg_filenode.map**          | Maps system catalog OIDs to relfilenode for shared catalogs | Only needed for `global/` shared catalogs. pg_arrow reads catalogs via SQL, not direct file access.                                                |
| **pg_internal.init**         | Relation cache init file                                    | PostgreSQL-internal optimization. pg_arrow doesn't use PG's relcache.                                                                              |
| **Temp tables**              | Session-local temporary tables                              | Created in `pg_temp_N` schemas. pg_arrow could technically read them but they're session-specific to the creating backend. No value for analytics. |
| **Temporary file storage**   | `pgsql_tmp/` for sorts, hash joins                          | PostgreSQL-internal. Not relevant.                                                                                                                 |

### Partially Relevant: May Implement Later

| Feature                      | What it is                                                 | Why partially relevant                                                                                                                                       | When to implement                          |
| ---------------------------- | ---------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------ |
| **BRIN index reading**       | Min/max per block range                                    | Could enable page-range pruning (skip pages where min > filter max). Cheap optimization for time-series data.                                                | Phase 7+                                   |
| **pg_statistic**             | Table statistics (row counts, distinct values, histograms) | DataFusion's optimizer could use these for better query plans. Read via SQL: `SELECT * FROM pg_statistic WHERE starelid = $1`.                               | Phase 6                                    |
| **Shared memory attachment** | Direct access to PG's shared_buffers, proc array           | Could read snapshot info without SQL connection. Faster but fragile, version-dependent.                                                                      | Probably never — SQL connection is simpler |
| **WAL parsing**              | Read WAL records to detect exactly which pages changed     | More precise than LSN-based cache invalidation. Could identify per-table, per-page changes.                                                                  | Phase 7+ if cache hit rate matters         |
| **Logical decoding**         | Stream logical changes for incremental updates             | Could maintain a cached Arrow representation and apply deltas instead of re-reading heap files. Major optimization for large tables with small change rates. | Future project                             |
| **pg_hba.conf parsing**      | Match pg_arrow's auth to PostgreSQL's auth config          | Consistent security posture. Complex (supports CIDR ranges, regex, multiple methods).                                                                        | Phase 8 with security model                |
| **LISTEN/NOTIFY**            | Receive DDL notifications from PostgreSQL                  | Alternative to polling pg_class for schema changes. Requires persistent connection.                                                                          | Phase 7+                                   |

## Library Architecture and Crate Structure

> **Context**: pg_arrow has two fundamentally different storage backends — heap file reading
> (Modes 1 & 2) and logical replication with Arrow-native storage (Mode 3). Everything above
> the storage layer (query engine, wire protocol, Flight SQL, CLI, observability) is shared.
> The storage layer is abstracted behind DataFusion's `TableProvider` trait — the query engine
> doesn't care where data comes from, only that it gets `RecordBatch` streams.
>
> Mode 3 is effectively an **entirely new columnar database** — it has its own storage format
> (Arrow/Parquet), write path (logical replication), crash recovery (checkpoint + replay),
> and compaction (LSM-style). The `pg_arrow_core` heap file crate is not used at all in Mode 3.

### Layered Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│                    SHARED ACROSS ALL MODES                         │
│                                                                    │
│  pg_arrow_server:   Wire protocol, Flight SQL, auth, config,      │
│                     lifecycle, observability, connection mgmt     │
│                                                                    │
│  pg_arrow_datafusion: CatalogProvider, optimizer rules, UDFs,     │
│                       statistics, SQL compatibility               │
│                                                                    │
│  pg_arrow_cli:      Interactive CLI (psql-like for Flight SQL)    │
│                                                                    │
│  All of this just needs: impl TableProvider → RecordBatch stream  │
│                                                                    │
├────────────────────────────┬──────────────────────────────────────┤
│  STORAGE LAYER A           │  STORAGE LAYER B                     │
│  pg_arrow_core             │  pg_arrow_logical                    │
│  (Modes 1 & 2)             │  (Mode 3)                            │
│                            │                                      │
│  Page parsing              │  pgoutput stream consumer            │
│  Tuple decoding            │  Arrow write buffer                  │
│  MVCC visibility           │  Deletion bitmap (RoaringBitmap)     │
│  CLOG reader               │  PK index                            │
│  TOAST decompression       │  LSM-style compaction                │
│  Visibility map            │  Parquet checkpoint + crash recovery │
│  Segment files             │  DDL tracking                        │
│  Arrow page cache          │                                      │
│  Zone maps / BRIN          │  Reads: logical replication stream   │
│  WAL cache invalidation    │  Stores: Arrow batches + Parquet     │
│                            │  Data IS the store (authoritative)   │
│  Reads: PostgreSQL $PGDATA/│  No $PGDATA/ access needed           │
│  Stores: cache (evictable) │  No MVCC, CLOG, TOAST, pages        │
│                            │                                      │
│  HeapFileTableProvider     │  LogicalReplicaTableProvider         │
│  (impl TableProvider)      │  (impl TableProvider)                │
└────────────────────────────┴──────────────────────────────────────┘
```

### Workspace Layout

```
pg_arrow/                          (workspace root)
├── Cargo.toml                     (workspace definition)
│
├── pg_arrow_core/                 STORAGE LAYER A — HEAP FILE READING (Modes 1 & 2)
│   ├── src/
│   │   ├── lib.rs                 Public API: PgCluster, PgTable, ScanOptions
│   │   ├── cluster.rs             pg_control reader, cluster validation
│   │   ├── heap/
│   │   │   ├── reader.rs          Segment-aware page reader
│   │   │   ├── page.rs            Page header, item pointers
│   │   │   ├── tuple.rs           Tuple header, null bitmap, column extraction
│   │   │   └── toast.rs           TOAST detoasting, pglz/lz4 decompression
│   │   ├── types/
│   │   │   ├── mod.rs             PG type OID → Arrow DataType mapping
│   │   │   ├── numeric.rs         PG numeric → Arrow Decimal128
│   │   │   ├── temporal.rs        timestamp, date, interval → Arrow
│   │   │   ├── text.rs            text, varchar, encoding transcoding
│   │   │   └── binary.rs          bytea → Arrow Binary
│   │   ├── mvcc/
│   │   │   ├── visibility.rs      HeapTupleSatisfiesMVCC equivalent
│   │   │   ├── clog.rs            pg_xact/ reader
│   │   │   ├── snapshot.rs        Snapshot types, acquisition
│   │   │   └── vm.rs              Visibility map reader
│   │   ├── index/
│   │   │   ├── brin.rs            BRIN index reader
│   │   │   └── btree.rs           B-tree index reader
│   │   ├── cache/
│   │   │   ├── arrow_cache.rs     Page-level Arrow RecordBatch cache
│   │   │   ├── zone_maps.rs       Per-page column min/max statistics
│   │   │   └── persistent.rs      Disk-backed Parquet cache
│   │   └── ffi/
│   │       ├── c_api.rs           Arrow C Data Interface exports
│   │       └── python.rs          PyO3 bindings
│   └── Cargo.toml                 Minimal deps: arrow-rs, lz4 (NO query engine dep)
│
├── pg_arrow_logical/              STORAGE LAYER B — LOGICAL REPLICA STORE (Mode 3)
│   ├── src/
│   │   ├── lib.rs                 Public API: LogicalReplicaStore
│   │   ├── store.rs               ArrowStore, TableState (batches + write buffer + deletes)
│   │   ├── consumer.rs            pgoutput stream consumer (BEGIN/INSERT/UPDATE/DELETE/COMMIT)
│   │   ├── compaction.rs          LSM-style compaction (merge write buffer, remove deletes)
│   │   ├── checkpoint.rs          Parquet checkpoint persistence + crash recovery
│   │   ├── pk_index.rs            Primary key → row_id index for UPDATE/DELETE lookups
│   │   └── ddl.rs                 Schema evolution from Relation messages + pg_class polling
│   └── Cargo.toml                 Deps: arrow-rs, parquet, roaring, tokio-postgres
│
├── pg_arrow_datafusion/           DATAFUSION INTEGRATION (shared, both storage layers)
│   ├── src/
│   │   ├── lib.rs
│   │   ├── catalog.rs             CatalogProvider → SchemaProvider → TableProvider
│   │   ├── table_provider.rs      HeapFileTableProvider + LogicalReplicaTableProvider
│   │   ├── scan.rs                PgArrowScanExec + ArrowStoreScanExec (custom ExecutionPlans)
│   │   ├── optimizer.rs           Custom OptimizerRules (PG fallback, adaptive scan)
│   │   ├── functions.rs           pg_compat UDFs/UDAFs
│   │   └── statistics.rs          pg_statistic → DataFusion Statistics
│   └── Cargo.toml                 Deps: pg_arrow_core, pg_arrow_logical, datafusion
│
├── pg_arrow_server/               SERVER BINARY (shared)
│   ├── src/
│   │   ├── main.rs                Mode selection from config → init correct storage layer
│   │   ├── pgwire.rs              PostgreSQL wire protocol (port 5433)
│   │   ├── flight.rs              Arrow Flight SQL server (port 5434)
│   │   ├── session.rs             Connection state, isolation levels, snapshots
│   │   ├── auth.rs                Authentication (trust, proxy, scram-sha-256)
│   │   ├── background.rs          Health monitor, WAL monitor, cache manager, VM monitor
│   │   ├── config.rs              pg_arrow.toml parsing
│   │   ├── lifecycle.rs           Startup/shutdown sequences, signal handling
│   │   └── observability.rs       Prometheus metrics, OpenTelemetry tracing, health endpoints
│   └── Cargo.toml                 Deps: pg_arrow_datafusion, pgwire, arrow-flight, tokio
│
├── pg_arrow_cli/                  INTERACTIVE CLI (psql-like for Flight SQL)
│   ├── src/
│   │   └── main.rs                Readline, table output, CSV/Parquet/Arrow export
│   └── Cargo.toml                 Deps: arrow-flight, rustyline, comfy-table
│
├── pg_arrow_python/               PYTHON BINDINGS (PyO3/maturin)
│   ├── src/
│   │   └── lib.rs                 pg_arrow.open(), cluster.scan() → PyArrow Table
│   ├── Cargo.toml
│   └── pyproject.toml
│
└── pg_arrow_duckdb/               DUCKDB EXTENSION (C FFI)
    ├── src/
    │   └── extension.cpp          pg_arrow_scan() table function for DuckDB
    └── CMakeLists.txt
```

### Core Library Public API

`pg_arrow_core` has **zero dependency on any query engine**. Its only dependencies are `arrow-rs`
(Arrow array types), `lz4`/custom pglz (TOAST decompression), and Rust standard library.

```rust
// pg_arrow_core/src/lib.rs

/// A PostgreSQL cluster (data directory)
pub struct PgCluster {
    config: ClusterConfig,       // from pg_control
    clog: ClogReader,            // pg_xact/ reader
    vm_cache: VmCache,           // visibility map cache
    arrow_cache: ArrowPageCache, // cached Arrow RecordBatches
    zone_maps: ZoneMapStore,     // per-page column min/max
}

impl PgCluster {
    /// Open a PostgreSQL data directory (read-only, no PG connection)
    pub fn open(data_dir: &Path) -> Result<Self>;

    /// Open with a PostgreSQL connection for catalogs and snapshots
    pub fn open_with_connection(data_dir: &Path, pg_conn: &str) -> Result<Self>;

    /// Get table by OID or by name
    pub fn table(&self, db_oid: u32, table_oid: u32) -> Result<PgTable>;
    pub fn table_by_name(&self, schema: &str, name: &str) -> Result<PgTable>;

    /// Acquire a snapshot (requires PG connection)
    pub fn snapshot(&self) -> Result<Snapshot>;
}

/// A single PostgreSQL table
pub struct PgTable { /* ... */ }

impl PgTable {
    /// Arrow schema for this table
    pub fn arrow_schema(&self) -> &arrow::datatypes::Schema;

    /// Table statistics from pg_statistic
    pub fn statistics(&self) -> Result<PgTableStatistics>;

    /// Synchronous scan → Iterator of Arrow RecordBatches
    pub fn scan(&self, options: ScanOptions) -> Result<PgScanIterator>;

    /// Async scan → Stream of Arrow RecordBatches
    pub async fn scan_async(&self, options: ScanOptions) -> Result<PgScanStream>;

    /// Export via Arrow C Data Interface (for C/C++/DuckDB/Python FFI)
    pub fn scan_to_ffi(&self, options: ScanOptions) -> Result<Vec<ArrowArrayFFI>>;
}

/// Engine-agnostic scan configuration
pub struct ScanOptions {
    pub snapshot: Snapshot,
    pub projection: Option<Vec<usize>>,     // column indices to read
    pub page_filter: Option<PageFilter>,     // zone map / BRIN predicates
    pub tuple_filter: Option<TupleFilter>,   // row-level predicates (late materialization)
    pub limit: Option<usize>,
    pub batch_size: usize,                   // target rows per RecordBatch (default 8192)
    pub use_cache: bool,                     // use Arrow page cache
    pub use_dictionary: bool,                // dictionary-encode low-cardinality columns
}
```

### Consumer Integration Examples

**DuckDB** (via C FFI + Arrow C Data Interface):

```sql
-- DuckDB extension using pg_arrow_core
INSTALL pg_arrow;
LOAD pg_arrow;

-- Read PostgreSQL data directory directly from DuckDB
SELECT * FROM pg_arrow_scan('/var/lib/postgresql/data', 'public', 'orders')
WHERE amount > 100
ORDER BY created_at DESC
LIMIT 1000;

-- Or register as a persistent view
CREATE VIEW orders AS SELECT * FROM pg_arrow_scan('/pg/data', 'public', 'orders');
```

**Python** (via PyO3 bindings):

```python
import pg_arrow

cluster = pg_arrow.open("/var/lib/postgresql/data",
                        connection="postgresql://localhost/mydb")
snapshot = cluster.snapshot()

# Returns PyArrow Table — zero-copy from Rust Arrow
table = cluster.scan("public.orders",
                     snapshot=snapshot,
                     columns=["order_id", "amount", "created_at"],
                     filter="amount > 100")

# Use with any Python tool — no server needed
df = table.to_pandas()              # pandas
pl_df = pl.from_arrow(table)        # Polars
duckdb.from_arrow(table)            # DuckDB in-process
```

**Polars** (direct Rust crate dependency):

```rust
use pg_arrow_core::{PgCluster, ScanOptions};

let cluster = PgCluster::open(Path::new("/pg/data"))?;
let table = cluster.table_by_name("public", "orders")?;
let batches: Vec<RecordBatch> = table.scan(ScanOptions::default())?.collect()?;
let df = polars::DataFrame::from_arrow_chunks(&table.arrow_schema(), batches)?;
```

### Why This Separation Matters

**Two storage layers, one query engine**: The `pg_arrow_datafusion` crate contains both
`HeapFileTableProvider` (backed by `pg_arrow_core`) and `LogicalReplicaTableProvider`
(backed by `pg_arrow_logical`). DataFusion doesn't know the difference — both produce
`RecordBatch` streams. This means all optimizer rules, UDFs, wire protocol encoding, and
Flight SQL work identically regardless of storage mode.

**Mode 3 is a new database**: `pg_arrow_logical` is a standalone columnar database engine
with its own storage format (Arrow/Parquet), write path (logical replication), crash recovery,
and compaction. It doesn't use `pg_arrow_core` at all. The shared layers (DataFusion, wire
protocol, Flight SQL, observability) make this feasible — only the storage layer changes.

**Hybrid mode spans both storage layers**: In the hybrid per-table strategy (Phase 12d),
hot tables use `LogicalReplicaTableProvider` (Arrow store, zero file I/O) while cold tables
use `HeapFileTableProvider` (heap files on demand). A single query can JOIN across both —
DataFusion handles cross-provider joins transparently. This is the key reason we build
`pg_arrow_logical` ourselves rather than using an external CDC-capable database (ClickHouse,
Materialize, etc.) — an external database would require a proxy for SQL translation, type
mapping, and session management, and could not transparently mix with heap file reads in a
single query plan. With both storage layers in-process behind `TableProvider`, the boundary
is invisible to clients and to the query optimizer.

**Embedded library consumers** use `pg_arrow_core` directly (Modes 1-2 only):

| Consumer                         | How it uses pg_arrow_core                         | Server needed?        |
| -------------------------------- | ------------------------------------------------- | --------------------- |
| **DataFusion** (pg_arrow_server) | Via `pg_arrow_datafusion` crate, full integration | Yes (serves clients)  |
| **DuckDB**                       | Via C FFI / Arrow C Data Interface                | No — embedded library |
| **Python/PyArrow**               | Via PyO3 bindings                                 | No — in-process       |
| **Polars**                       | Via Rust crate dependency                         | No — in-process       |
| **Spark**                        | Via Arrow Flight SQL client → pg_arrow_server     | Yes                   |
| **Custom Rust program**          | Via Rust crate dependency                         | No — library          |
| **Any Arrow-compatible engine**  | Via Arrow C Data Interface (C ABI)                | No                    |

The Arrow C Data Interface is the universal bridge — any language with Arrow support (C, C++,
Python, R, Julia, Go, Java via JNI) can consume `pg_arrow_core`'s output without copying data.

## Arrow Flight and ADBC Protocol

> **Context**: The PostgreSQL wire protocol forces a columnar → row conversion on every query
> response (`DataRow` messages are row-oriented). This is wasteful when pg_arrow already has
> data in Arrow columnar format internally. Arrow Flight SQL eliminates this conversion entirely.

### The Row Conversion Problem

```
PostgreSQL wire protocol path:
  Heap file → Arrow RecordBatch → serialize each row as DataRow message → client deserializes rows
  Cost: O(rows * columns) serialization + deserialization

Arrow Flight SQL path:
  Heap file → Arrow RecordBatch → send RecordBatch as Arrow IPC → client receives RecordBatch
  Cost: O(batches) metadata only — batch data is already in wire format (Arrow IPC)
```

For a 10M row × 20 column result set:

- PostgreSQL wire protocol: ~30 seconds (row-by-row serialize/deserialize)
- Arrow Flight SQL: ~2 seconds (stream Arrow IPC batches, near-zero serialization)

### Dual-Protocol Architecture

```
                              pg_arrow_server
                    ┌──────────────────────────────┐
                    │                              │
psql ──── PG wire ──→  Port 5433                   │
JDBC ──── PG wire ──→  (PostgreSQL wire protocol)  │
ORMs ──── PG wire ──→  Row-oriented output         │
                    │         │                    │
                    │         ▼                    │
                    │   ┌───────────────┐          │
                    │   │  DataFusion   │←── pg_arrow_core (heap reader)
                    │   │  Engine       │          │
                    │   └───────────────┘          │
                    │         ▲                    │
                    │         │                    │
pg_arrow_cli ─ gRPC ─→  Port 5434                  │
Python ADBC ── gRPC ─→  (Arrow Flight SQL)         │
DuckDB ─────── gRPC ─→  Columnar output            │
Spark ──────── gRPC ─→  Zero serialization          │
                    │                              │
                    └──────────────────────────────┘
```

Both ports share the same DataFusion engine. The only difference is output encoding.

### Arrow Flight SQL Server

```rust
// pg_arrow_server/src/flight.rs

use arrow_flight::sql::FlightSqlService;

struct PgArrowFlightService {
    datafusion_ctx: Arc<SessionContext>,  // shared with pgwire server
}

#[tonic::async_trait]
impl FlightSqlService for PgArrowFlightService {
    async fn do_get_statement(
        &self,
        ticket: TicketStatementQuery,
        request: Request<Ticket>,
    ) -> Result<Response<FlightDataStream>> {
        // Execute query with DataFusion
        let df = self.datafusion_ctx.sql(&ticket.statement_handle).await?;
        let batches = df.collect().await?;

        // Stream Arrow RecordBatches directly — no row conversion!
        let stream = FlightDataEncoderBuilder::new()
            .with_schema(df.schema())
            .build(futures::stream::iter(batches.into_iter().map(Ok)));

        Ok(Response::new(Box::pin(stream)))
    }

    // Prepared statements, schema discovery, etc.
    async fn do_get_prepared_statement(&self, ...) -> Result<...> { /* ... */ }
    async fn get_flight_info_statement(&self, ...) -> Result<...> { /* ... */ }
    async fn get_flight_info_catalogs(&self, ...) -> Result<...> { /* ... */ }
    async fn get_flight_info_schemas(&self, ...) -> Result<...> { /* ... */ }
    async fn get_flight_info_tables(&self, ...) -> Result<...> { /* ... */ }
}
```

### pg_arrow_cli — psql-like for Arrow Flight SQL

Since psql has no plugin system and is hardwired to libpq/PostgreSQL wire protocol, we build
a dedicated CLI that speaks Arrow Flight SQL:

```
pg_arrow_cli features:
  ├── Interactive SQL prompt (rustyline: readline, history, tab completion)
  ├── Meta-commands: \d, \dt, \l, \dn (via Flight SQL metadata RPCs)
  ├── Arrow Flight SQL connection (native Arrow RecordBatch results)
  │
  └── Output modes:
      ├── table     — render Arrow columns as aligned table (like psql default)
      ├── csv       — stream Arrow columns to CSV (zero row conversion)
      ├── json      — stream as JSON lines
      ├── parquet   — write Arrow batches to Parquet file (zero conversion!)
      ├── arrow     — dump raw Arrow IPC (pipe to other tools)
      └── expanded  — \x mode like psql
```

```bash
# Usage examples:
pg_arrow_cli --host localhost --port 5434

pg_arrow> SELECT product_id, SUM(amount) FROM orders GROUP BY product_id;
 product_id | sum
------------+-----------
        101 |   54892.50
        102 |   31204.00
(2 rows, 0.8s, streamed via Arrow Flight)

pg_arrow> \output /tmp/results.parquet
pg_arrow> SELECT * FROM orders WHERE created_at > '2024-01-01';
-- Written directly to Parquet — Arrow RecordBatches → Parquet with zero row conversion

pg_arrow> \output /tmp/results.csv
pg_arrow> SELECT * FROM orders;
-- Streamed to CSV from Arrow columnar arrays
```

### ADBC (Arrow Database Connectivity)

ADBC is a cross-language API built on Flight SQL. It's a drop-in replacement for ODBC/JDBC
but columnar-native. Client libraries exist for Python, Java, Go, R, C/C++.

```python
# Python — ADBC driver for pg_arrow
import adbc_driver_flightsql.dbapi

conn = adbc_driver_flightsql.dbapi.connect("grpc://localhost:5434")
cur = conn.cursor()
cur.execute("SELECT * FROM orders WHERE amount > 100")

# Native Arrow table — zero deserialization
table = cur.fetch_arrow_table()

# Zero-copy integrations
df = table.to_pandas()           # pandas (via Arrow-pandas bridge)
pl_df = pl.from_arrow(table)     # Polars
```

The performance difference vs PostgreSQL wire protocol:

| Result set         | PG wire (psycopg2) | ADBC (Flight SQL) | Speedup |
| ------------------ | ------------------ | ----------------- | ------- |
| 1M rows × 10 cols  | 8.2s               | 0.3s              | 27x     |
| 10M rows × 20 cols | 95s                | 2.1s              | 45x     |
| 100K rows × 5 cols | 0.6s               | 0.05s             | 12x     |

The speedup comes from eliminating per-row serialization/deserialization on both sides.

## Arrow-Native Optimizations

> **Context**: Since pg_arrow produces Arrow RecordBatches internally, we can apply columnar
> optimizations that PostgreSQL's row-oriented executor cannot.

### Late Materialization

Don't convert all columns from tuples to Arrow upfront. Only decode what the query needs:

```sql
SELECT name FROM users WHERE age > 30;
```

```
Naive approach:
  Parse tuple → decode ALL 10 columns → build full RecordBatch → filter → project
  Cost: decoded 10 columns, only needed 2

Late materialization:
  1. Parse tuple → decode ONLY age (column 3) → Arrow Int32Array
  2. Apply filter: age > 30 → selection vector [true, false, true, ...]
  3. Go back to tuples for matching rows → decode ONLY name (column 1)
  4. Build final RecordBatch with name column only
  Cost: decoded 2 columns, skipped 8
```

For wide tables (50+ columns) with selective filters, this is 10-25x less decoding work.
PostgreSQL tuples store a null bitmap with column offsets, so for fixed-width types we can
jump directly to a specific column without parsing preceding columns.

### Dictionary Encoding for Low-Cardinality Columns

Arrow's `DictionaryArray` stores repeated values once and uses integer indices:

```
Regular StringArray for "status" column (1M rows):
  ["active", "active", "inactive", "active", "pending", ...]
  Memory: ~7 bytes × 1M = 7MB

DictionaryArray:
  dictionary: ["active", "inactive", "pending"]    (3 unique strings)
  indices:    [0, 0, 1, 0, 2, 0, ...]              (1M × uint8 = 1MB)
  Memory: ~21 bytes + 1MB ≈ 1MB (7x smaller)
```

pg_arrow can detect low-cardinality columns by reading `pg_statistic.stadistinct`:

- `n_distinct < 256` → DictionaryArray with uint8 indices
- `n_distinct < 65536` → DictionaryArray with uint16 indices

GROUP BY on DictionaryArray is dramatically faster — compare integers instead of strings.

### Vectorized SIMD Filtering

Arrow-rs compute kernels use SIMD automatically for columnar data:

```rust
use arrow::compute::{filter, gt_scalar};

// Filter 1M rows with SIMD — ~2ms:
let age_array: Int32Array = /* from heap page conversion */;
let mask = gt_scalar(&age_array, 30)?;           // SIMD comparison
let filtered_names = filter(&name_array, &mask)?;  // SIMD gather
```

PostgreSQL's row-at-a-time executor cannot use SIMD. This is a fundamental advantage of the
Arrow format.

### RecordBatch Size Tuning

Converting one 8KB page at a time produces small batches (~200-400 rows). DataFusion and SIMD
work best with larger batches (8K-64K rows):

```
Per-page batches (suboptimal):
  Page 0 → RecordBatch(300 rows) → poor SIMD utilization
  Page 1 → RecordBatch(280 rows) → per-batch overhead

Multi-page batches (optimal):
  Pages 0-31 → RecordBatch(9600 rows) → full SIMD lanes utilized
  Pages 32-63 → RecordBatch(8800 rows) → less per-batch overhead
```

Accumulate tuples from multiple pages before building the RecordBatch. Target 8K-64K rows
per batch for optimal vectorization.

### Zero-Copy Slicing for LIMIT

Arrow arrays support zero-copy slicing — no data copied:

```rust
// SELECT * FROM orders LIMIT 100
let batch = /* 10000 rows from conversion */;
let limited = batch.slice(0, 100);  // Just adjusts offset/length pointers — zero copy
```

## PostgreSQL Index Reuse

> **Context**: pg_arrow doesn't need to build its own indexes. PostgreSQL already has B-tree,
> BRIN, and other indexes on disk. We can read these directly to skip pages during scans.

### BRIN Index Reading — Best Bang for Buck

BRIN (Block Range INdex) stores min/max per range of heap pages (default 128 pages = 1MB).
This is the simplest and most valuable index for pg_arrow to read:

```
BRIN for orders.created_at (128 pages per range):
  Pages 0-127:     min=2024-01-01, max=2024-01-15
  Pages 128-255:   min=2024-01-15, max=2024-02-01
  Pages 256-383:   min=2024-02-01, max=2024-02-15
  ...

Query: WHERE created_at > '2024-06-01'
  → Skip all ranges with max < 2024-06-01
  → For time-series data, this skips 90%+ of pages
```

BRIN is trivial to read — it's a regular heap table with `(page_range, min, max)` tuples:

```rust
struct BrinEntry {
    block_range_start: u32,
    block_range_end: u32,
    min_val: ScalarValue,
    max_val: ScalarValue,
}

fn prune_pages_with_brin(brin: &[BrinEntry], filter: &Expr) -> Vec<Range<u32>> {
    brin.iter()
        .filter(|entry| entry.range_may_match(filter))
        .map(|entry| entry.block_range_start..entry.block_range_end)
        .collect()
}
```

### B-tree Index Reading — Targeted Row Lookup

PostgreSQL's B-tree leaf pages contain `(key, TID)` pairs where TID = `(block_number, offset)`:

```
B-tree on orders.customer_id:
  Leaf page: [(100, (5,3)), (101, (5,7)), (102, (8,1)), ...]

Query: WHERE customer_id BETWEEN 100 AND 200
  1. Walk B-tree to find first leaf page with key >= 100
  2. Follow leaf chain, collecting TIDs until key > 200
  3. Group TIDs by block number: {5: [3,7], 8: [1,2], ...}
  4. Read ONLY those heap pages, extract ONLY those tuple offsets
  5. Skip all other pages entirely
```

For highly selective queries on indexed columns, this reduces I/O from full table scan to a
handful of pages. More complex to implement than BRIN (B-tree internal page format, leaf chain
traversal) but much more precise.

### Self-Built Zone Maps

As pg_arrow scans pages and converts to Arrow, it computes and caches per-page statistics:

```rust
struct PageZoneMap {
    page_num: u32,
    page_lsn: u64,
    columns: Vec<ColumnStats>,
}

struct ColumnStats {
    min: ScalarValue,
    max: ScalarValue,
    null_count: usize,
}
```

On subsequent queries, check the zone map before reading the page:

```
Query: WHERE amount > 1000
  Page 42 zone map: amount min=5.00, max=99.99 → SKIP (max < 1000)
  Page 43 zone map: amount min=500.00, max=5000.00 → READ (range overlaps)
```

This works like a self-built BRIN index, but:

- No dependency on PostgreSQL having created a BRIN index
- Works for any column, including non-indexed ones
- Built automatically as a side effect of scanning
- Invalidated by LSN change per page

## Incremental Arrow Page Cache

> **Context**: The most impactful single optimization. Instead of re-parsing PostgreSQL heap
> pages on every query, cache the Arrow-converted RecordBatches and reuse them.

### Page-Level Arrow Cache

```
Cache key:   (table_oid, page_number, page_lsn)
Cache value: Arrow RecordBatch + zone map stats for that page's visible tuples

First query:
  Page 0 (LSN=100) → parse tuples → build RecordBatch → CACHE → return
  Page 1 (LSN=100) → parse tuples → build RecordBatch → CACHE → return

Second query:
  Page 0: current LSN still 100 → CACHE HIT → return cached batch (no parsing!)
  Page 1: current LSN still 100 → CACHE HIT → return cached batch

After PostgreSQL modifies page 0:
  Page 0: LSN now 150 → INVALIDATE → re-parse → cache new batch → return
  Page 1: LSN still 100 → CACHE HIT → return cached (unchanged)
```

### Three-Level Invalidation (Cheapest First)

```
Level 1 — Visibility map (cheapest check):
  If page is all-frozen in VM → cache entry is PERMANENT (frozen data never changes)
  Don't even check LSN. This covers the majority of pages in mature tables.

Level 2 — Page LSN comparison (cheap check):
  Read only the page header (first 24 bytes) → compare pd_lsn with cached LSN
  If LSN unchanged → cache hit (skip reading the other 8168 bytes of page data)
  24 bytes vs 8192 bytes = 340x less I/O for cache validation

Level 3 — WAL-based table-level invalidation (cheapest for no-change case):
  WAL monitor tracks which tables had writes since last check
  If table had NO writes → ALL pages are still valid
  Skip per-page LSN checks entirely — return cached data for the whole table
```

### Column-Level Cache (Finer Granularity)

Cache individual Arrow arrays per column instead of full RecordBatches:

```
Cache key:   (table_oid, page_number, column_index, page_lsn)
Cache value: Arrow Array for that column on that page

Query 1: SELECT name, age FROM users WHERE age > 30
  → Cache: page0/name, page0/age, page0/id, ...

Query 2: SELECT email FROM users WHERE id = 5
  → Cache HIT for page0/id (already cached from query 1!)
  → Cache MISS for page0/email → decode only email column
```

Column-level cache has better hit rates for diverse workloads where different queries access
different column subsets.

### Background Pre-Conversion

Don't wait for queries to populate the cache:

```
WAL monitor detects: pages 42-45 of orders table changed

Background task (async, low priority):
  1. Read pages 42-45 from heap file
  2. Parse tuples, apply visibility checks
  3. Convert to Arrow RecordBatches
  4. Store in cache with current LSN
  5. Update zone maps

Next query on orders table:
  Pages 42-45 → CACHE HIT — Arrow data already ready
  Zero conversion latency for the user query
```

### Persistent Cache (Survives Restarts)

Write cached Arrow data to disk as Parquet files:

```
cache/
├── orders/
│   ├── page_0000_lsn_0000000100.parquet    ← compressed columnar
│   ├── page_0001_lsn_0000000100.parquet
│   ├── page_0042_lsn_0000000150.parquet    ← updated page
│   └── zone_maps.bin                        ← per-page min/max
└── users/
    ├── page_0000_lsn_0000000080.parquet
    └── zone_maps.bin
```

On startup, pg_arrow loads the persistent cache index. Only re-converts pages where LSN has
advanced since the cached version.

This is effectively an **incrementally-maintained columnar materialized view** of PostgreSQL's
data. With Parquet as the persistent format, you also get Zstd/Snappy compression for free.

## I/O Optimizations

### Memory-Mapped I/O

```rust
use memmap2::MmapOptions;

// mmap the heap file — OS manages page cache
let mmap = unsafe { MmapOptions::new().map(&file)? };
let page = &mmap[page_num * BLCKSZ .. (page_num + 1) * BLCKSZ];
```

Benefits: OS handles caching, no double-buffering, sequential scans trigger kernel readahead,
multiple queries share the same mapped pages.

### io_uring Async I/O (Linux)

Submit multiple page reads in one syscall:

```rust
// Submit 32 page reads in one syscall — hide disk latency
for page in 0..32 {
    ring.submit_read(fd, page * BLCKSZ, BLCKSZ)?;
}
let completions = ring.wait_completions(32)?;
```

### Readahead Hints

```rust
// Sequential scan hint — kernel prefetches ahead
posix_fadvise(fd, 0, file_len, POSIX_FADV_SEQUENTIAL);

// Index-driven targeted reads — prefetch specific pages
for page in pages_to_read {
    posix_fadvise(fd, page * BLCKSZ, BLCKSZ, POSIX_FADV_WILLNEED);
}
```

### Batched CLOG Lookups

Collect unique xids per page, batch-read the CLOG:

```
Naive: one CLOG read per tuple (N seeks)
Batched:
  1. Collect unique xids from page: {1000, 1001, 1005, 1008}
  2. All map to CLOG page 0 (32768 xids per CLOG page)
  3. Read CLOG page 0 ONCE → resolve all xids
  4. Apply results to all tuples
  Nearby xids almost always share a CLOG page → 1 read instead of N
```

### Parallel Page Conversion

Pages are independent — convert them across multiple threads:

```rust
let batches: Vec<RecordBatch> = (0..num_pages)
    .into_par_iter()                              // rayon parallel iterator
    .filter(|p| zone_map_matches(*p, &filters))   // skip non-matching pages
    .filter_map(|p| {
        cache.get(table_oid, p)                    // return cached if valid
            .or_else(|| convert_and_cache(p))      // else parse + convert + cache
    })
    .collect();
```

### Pipeline: Read → Parse → Convert → Execute

Overlap I/O with computation in a pipeline:

```
Thread 1 (I/O):       [read pg0] [read pg1] [read pg2] [read pg3] ...
Thread 2 (parse):                [parse pg0] [parse pg1] [parse pg2] ...
Thread 3 (Arrow):                            [arrow pg0] [arrow pg1] ...
Thread 4 (DataFusion):                                   [exec batch0] ...
```

Four pages processed simultaneously at different pipeline stages.

## DataFusion Engine Integration

> **Context**: We have DataFusion expertise and can contribute upstream changes or maintain
> pg_arrow-specific extensions. This section covers DataFusion integration points that go
> beyond standard `TableProvider` usage.

### Custom CatalogProvider (Upstreamable)

Map PostgreSQL's catalog hierarchy directly to DataFusion:

```rust
// pg_class (relnamespace) → pg_namespace → DataFusion schema
// pg_database → DataFusion catalog

struct PgCatalogProvider { cluster: Arc<PgCluster>, db_oid: u32 }

impl CatalogProvider for PgCatalogProvider {
    fn schema_names(&self) -> Vec<String> {
        // SELECT nspname FROM pg_namespace
    }
    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        Some(Arc::new(PgSchemaProvider { /* ... */ }))
    }
}

// DataFusion resolves: SELECT * FROM public.orders
// → catalog("pg") → schema("public") → table("orders") → PgTableProvider
```

### Statistics from pg_statistic (Upstreamable)

Feed PostgreSQL's ANALYZE statistics into DataFusion's optimizer for accurate cost estimates:

```rust
impl TableProvider for PgTableProvider {
    fn statistics(&self) -> Option<Statistics> {
        let pg_stats = self.pg_table.statistics().ok()?;
        Some(Statistics {
            num_rows: Precision::Exact(pg_stats.reltuples as usize),
            total_byte_size: Precision::Exact(pg_stats.relpages as usize * BLCKSZ),
            column_statistics: pg_stats.columns.iter().map(|col| {
                ColumnStatistics {
                    null_count: Precision::Exact(col.null_frac as usize),
                    distinct_count: Precision::Exact(col.n_distinct as usize),
                    min_value: Precision::Exact(col.min_value.clone()),
                    max_value: Precision::Exact(col.max_value.clone()),
                }
            }).collect(),
        })
    }
}
```

This gives DataFusion accurate row count estimates, distinct value counts, and min/max bounds —
dramatically improving join ordering and aggregation strategy selection.

### Custom ExecutionPlan with Multi-Level Filtering

A custom `ExecutionPlan` that implements the full pg_arrow scan pipeline:

```rust
impl ExecutionPlan for PgArrowScanExec {
    fn execute(&self, partition: usize, ctx: Arc<TaskContext>) -> Result<SendableRecordBatchStream> {
        let stream = async_stream::stream! {
            for page_range in self.assigned_page_ranges(partition) {

                // Level 1: BRIN pruning — skip 128 pages at once
                if !self.brin_matches(page_range, &self.filters) { continue; }

                for page_num in page_range {
                    // Level 2: Zone map pruning — skip individual pages
                    if !self.zone_map_matches(page_num, &self.filters) { continue; }

                    // Level 3: VM fast path — skip visibility checks for frozen pages
                    let skip_visibility = self.vm.is_all_frozen(page_num);

                    // Level 4: Arrow cache — skip re-parsing
                    if let Some(cached) = self.cache.get(self.table_oid, page_num) {
                        yield Ok(cached);
                        continue;
                    }

                    // Level 5: Parse page + late materialization + convert to Arrow
                    let page = self.reader.read_page(page_num)?;
                    let batch = self.convert_with_late_materialization(
                        page, &self.projection, &self.filters, &self.snapshot, skip_visibility,
                    )?;

                    // Update cache + zone maps for next query
                    self.cache.insert(self.table_oid, page_num, page.lsn(), &batch);
                    self.zone_maps.update(page_num, &batch);

                    if batch.num_rows() > 0 { yield Ok(batch); }
                }
            }
        };
        Ok(Box::pin(RecordBatchStreamAdapter::new(self.schema(), stream)))
    }
}
```

### Custom OptimizerRule: Adaptive Scan Strategy

Choose scan strategy based on selectivity and available indexes:

```rust
impl PhysicalOptimizerRule for AdaptiveScanRule {
    fn optimize(&self, plan: Arc<dyn ExecutionPlan>, config: &ConfigOptions)
        -> Result<Arc<dyn ExecutionPlan>>
    {
        if let Some(scan) = plan.as_any().downcast_ref::<PgArrowScanExec>() {
            let selectivity = estimate_selectivity(&scan.filters, &scan.statistics());

            if selectivity < 0.01 && scan.has_btree_index() {
                return Ok(Arc::new(PgBTreeIndexScan::from(scan)));  // Very selective + index
            }
            if selectivity < 0.1 && scan.has_brin_index() {
                return Ok(Arc::new(PgBrinScan::from(scan)));        // Moderate + BRIN
            }
            // Default: full scan with zone maps + cache
        }
        Ok(plan)
    }
}
```

### Custom OptimizerRule: PostgreSQL Fallback

Detect queries pg_arrow can't handle and rewrite to proxy to PostgreSQL:

```rust
impl OptimizerRule for PgFallbackRule {
    fn optimize(&self, plan: &LogicalPlan, _config: &dyn OptimizerConfig) -> Result<LogicalPlan> {
        if has_unsupported_functions(plan) || has_extension_types(plan) {
            let sql = logical_plan_to_sql(plan)?;
            return Ok(LogicalPlan::Extension(Arc::new(PgRemoteScan {
                sql,
                connection: self.pg_pool.clone(),
            })));
        }
        Ok(plan.clone())
    }
}
```

### Memory Pool Integration (DataFusion Core Change)

Tie pg_arrow's Arrow cache to DataFusion's memory management:

```rust
struct PgArrowMemoryPool {
    cache: Arc<ArrowPageCache>,
    inner: Arc<dyn MemoryPool>,
}

impl MemoryPool for PgArrowMemoryPool {
    fn grow(&self, reservation: &MemoryReservation, additional: usize) {
        if self.inner.try_grow(reservation, additional).is_err() {
            // DataFusion needs memory — evict from pg_arrow cache
            self.cache.evict_lru(additional);
            self.inner.grow(reservation, additional);
        }
    }
}
```

This ensures the Arrow page cache and DataFusion's query execution share a memory budget
intelligently — cache eviction happens automatically under memory pressure.

### Optimization Stack Summary

```
Scan execution flow with all optimizations:

  Query arrives
    │
    ▼
  DataFusion optimizer
    ├─ PgFallbackRule: can DataFusion handle this? If not → proxy to PostgreSQL
    ├─ AdaptiveScanRule: choose scan strategy (B-tree / BRIN / full scan)
    └─ Standard DataFusion optimization (join reorder, predicate pushdown, etc.)
    │
    ▼
  PgArrowScanExec (custom ExecutionPlan)
    │
    ├─ L1: WAL-based table invalidation — has table changed at all? If not → all cache valid
    ├─ L2: BRIN index pruning — skip page ranges by min/max
    │
    │  For each page:
    ├─ L3: Zone map pruning — skip page by per-page min/max
    ├─ L4: Visibility map — skip MVCC checks for all-frozen pages
    ├─ L5: Arrow page cache — skip re-parsing if LSN unchanged
    ├─ L6: Late materialization — decode only projected + filtered columns
    ├─ L7: Dictionary encoding — compress low-cardinality columns
    ├─ L8: Multi-page batching — accumulate 8K-64K rows per RecordBatch
    │
    ▼
  DataFusion vectorized execution (SIMD filter, aggregate, join, sort)
    │
    ▼
  Output: PostgreSQL wire protocol (row) OR Arrow Flight SQL (columnar)
```

## Deployment Modes and WAL Synchronization

> **Origin**: This section consolidates ideas from the earlier `DESIGN_ZERO_COPY_REPLICA.md`
> (now deleted) with a deeper analysis of WAL synchronization strategies and sidecar deployment.

### Three Deployment Modes

pg_arrow supports three deployment modes, each suited for different production requirements.
Modes 1 and 2 use the same codebase (`pg_arrow_core` with heap file parsing). Mode 3 is a
fundamentally different data path — pgoutput → Arrow, no heap files.

```
Mode 1: Sidecar + Primary
══════════════════════════
  ┌──────────────────────────────┐
  │ Server                        │
  │  PostgreSQL (primary)         │
  │  pg_arrow (sidecar)          │ ← reads $PGDATA/ directly
  │                               │   full pg_arrow_core (MVCC, CLOG, TOAST, pages)
  │  Single server, simplest      │
  │  Port 5432: OLTP (PostgreSQL) │
  │  Port 5433: OLAP (pg_arrow)  │
  └──────────────────────────────┘

Mode 2: Sidecar + Promotable Replica
═════════════════════════════════════
  ┌──────────────────────────────┐         ┌──────────────────────────────┐
  │ Server A                      │   WAL   │ Server B                      │
  │  PostgreSQL (primary)         │ ──────→ │  PostgreSQL (standby)         │
  │  pg_arrow (sidecar)          │         │  pg_arrow (sidecar)          │
  │                               │         │                               │
  │  Port 5432: OLTP writes      │         │  Port 5432: OLTP reads       │
  │  Port 5433: OLAP (pg_arrow) │         │  Port 5433: OLAP (pg_arrow) │
  └──────────────────────────────┘         └──────────────────────────────┘
                                             ↑ reads $PGDATA/ directly
                                               full pg_arrow_core
                                               PG handles promotion
                                               pg_arrow keeps reading after promote

Mode 3: Sidecar or Standalone — Non-Promotable Logical Replica
══════════════════════════════════════════════════════════════════
  ┌──────────────────────────────┐  logical  ┌─────────────────────────────┐
  │ Server A                      │  stream   │ Server C (any server)        │
  │  PostgreSQL (primary)         │ ────────→ │  pg_arrow (standalone)       │
  │                               │           │  No PostgreSQL process       │
  └──────────────────────────────┘           │  No $PGDATA/ access needed   │
                                              │  Checkpoint + logical apply  │
        OR: sidecar on a PG standby           │  Arrow/Parquet native store  │
            using logical instead              │  No MVCC, CLOG, TOAST, pages │
            of heap file reading               │  Port 5433: OLAP (pg_arrow) │
                                              │  Port 5434: Flight SQL       │
                                              └─────────────────────────────┘
```

### Mode Comparison

| | Mode 1: Sidecar + Primary | Mode 2: Sidecar + Promotable Replica | Mode 3: Logical Replica |
|---|---|---|---|
| **Promotable** | N/A (is the primary) | **Yes** — PG handles it | **No** |
| **Needs $PGDATA/** | Yes | Yes | **No** — network only |
| **Needs local PostgreSQL** | Yes (is the primary) | Yes (standby) | **No** |
| **Can run on separate server** | No | No | **Yes** |
| **MVCC complexity** | Full | Full | **None** — logical stream pre-resolved |
| **TOAST handling** | Must implement | Must implement | **Handled by PG** (pgoutput detoasts) |
| **Page parsing** | Must implement | Must implement | **Not needed** |
| **Impact on primary OLTP** | I/O competes with writes | **Zero** (reads replica) | **Minimal** (logical decoding CPU) |
| **Torn page risk** | Yes (concurrent backends) | Pause recovery → **zero risk** | **N/A** (no pages) |
| **Data freshness** | Real-time | Seconds (replication lag) | Seconds (replication lag) |
| **Memory model** | Cache (evictable) | Cache (evictable) | Full table state (required) |
| **Initial sync** | None (reads files) | None (reads files) | COPY snapshot (slow for large DBs) |
| **Recovery after crash** | Re-read files (instant) | Re-read files (instant) | Load Parquet checkpoint + replay |
| **PG version requirement** | Any | Any | 10+ (logical repl), 16+ on standby |
| **REPLICA IDENTITY needed** | No | No | **Yes** (on all published tables) |
| **pg_arrow_core crate** | Full (page parsing, MVCC, TOAST) | Full | **Not used** — pgoutput parser only |

### Mode 1: Sidecar + Primary — Details

The simplest deployment. pg_arrow runs alongside the primary PostgreSQL on the same server,
reading heap files from `$PGDATA/`. Both processes share the same filesystem.

pg_arrow connects to the local PostgreSQL via SQL for schema, snapshots, and CLOG-related
queries. All query execution goes through pg_arrow_core (page parsing, MVCC visibility,
TOAST detoasting, Arrow conversion) → DataFusion.

This mode has I/O contention with PostgreSQL's write path (both read/write the same disk),
but for SSDs and typical analytics workloads this is rarely a bottleneck.

### Mode 2: Sidecar + Promotable Replica — Details

pg_arrow runs alongside a PostgreSQL streaming replica. Both read the same `$PGDATA/`.
PostgreSQL owns the data directory — it receives WAL from the primary and applies it.
pg_arrow only reads.

**Promotion is entirely PostgreSQL's concern:**

```
Before promotion:
  PostgreSQL: standby mode, applying WAL from primary
  pg_arrow:   reading $PGDATA/, connected to local PG for metadata

  pg_ctl promote    ← standard PostgreSQL promotion

After promotion:
  PostgreSQL: primary mode, accepting writes, generating WAL
  pg_arrow:   reading $PGDATA/, connected to local PG for metadata
              ↑ NOTHING CHANGED for pg_arrow — same data dir, same PG connection
```

pg_arrow doesn't care whether its local PostgreSQL is primary or standby. It reads heap files
either way. On promotion, pg_arrow detects the mode change via `pg_is_in_recovery()` flipping
from `true` to `false`, invalidates caches (relfilenode may change), and reacquires schema.

**Full HA topology:**

```
                    ┌──────────────────────────────────┐
                    │ Server A (current primary)        │
                    │  PostgreSQL (primary) :5432       │
                    │  pg_arrow (sidecar)   :5433      │
                    └───────┬──────────┬───────────────┘
                   WAL stream│          │WAL stream
                            │          │
              ┌─────────────▼──┐  ┌────▼─────────────────┐
              │ Server B        │  │ Server C              │
              │ PostgreSQL      │  │ PostgreSQL (standby)  │
              │ (standby) :5432│  │              :5432    │
              │ pg_arrow  :5433│  │ pg_arrow     :5433   │
              └─────────────────┘  └───────────────────────┘

Failover (Server A dies):
  1. Promote Server B's PostgreSQL to primary
  2. Server B's pg_arrow keeps working (same $PGDATA/)
  3. Server C repoints WAL to Server B
  4. Server C's pg_arrow keeps working (same $PGDATA/)
  5. Analytics workload: brief stall, then resumes on B and C
```

**Replica-specific advantages:**

- **Zero torn page risk**: Recovery is single-threaded. Pause replay briefly for guaranteed
  consistent reads:

```sql
SELECT pg_wal_replay_pause();
-- pg_arrow reads heap files — no concurrent page writes
SELECT pg_last_wal_replay_lsn();  -- consistency point
-- ... pg_arrow reads ...
SELECT pg_wal_replay_resume();
```

- **Recovery LSN as consistency point**: `pg_last_wal_replay_lsn()` tells pg_arrow exactly
  how far the replica has recovered — a natural snapshot boundary.

- **No write contention**: Primary's I/O is completely unaffected by pg_arrow's scans.

**Replica-specific PostgreSQL configuration:**

```
hot_standby = on                 # required for SQL connection to replica
hot_standby_feedback = on        # prevent primary from vacuuming rows replica needs
max_standby_streaming_delay = -1 # never cancel pg_arrow queries due to WAL conflict
max_standby_archive_delay = -1   # same for archive recovery
```

### Mode 3: Logical Replica — Details

The most architecturally distinct mode. pg_arrow maintains its own **Arrow-native data store**
using logical replication — no heap files, no MVCC, no page parsing.

**Data path:**

```
Primary PostgreSQL                 pg_arrow Logical Replica
       │                                  │
       │  Phase 1: Base checkpoint         │
       │  CREATE_REPLICATION_SLOT          │
       │  pg_arrow_data LOGICAL pgoutput   │
       │                                   │
       │  COPY all tables using ───────→   │ Convert to Arrow RecordBatches
       │  slot's snapshot                  │ Persist as Parquet checkpoint
       │                                   │ checkpoint_lsn = slot consistent_point
       │                                   │
       │  Phase 2: Continuous apply        │
       │                                   │
       │── BEGIN txid=500 ────────────────→│
       │── INSERT users (5,'alice') ─────→│ append to Arrow write buffer
       │── DELETE orders (id=3) ─────────→│ mark in deletion bitmap
       │── COMMIT ───────────────────────→│ advance confirmed_lsn
       │                                   │
       │  (repeat forever)                 │ Periodic: compact + checkpoint
```

**What Mode 3 eliminates:**

```
Heap file mode (Modes 1-2):          Logical mode (Mode 3):
  Page header parsing        ──→       NOT NEEDED
  Item pointer parsing       ──→       NOT NEEDED
  Tuple header parsing       ──→       NOT NEEDED
  Null bitmap decoding       ──→       NOT NEEDED ¹
  MVCC visibility (500+ LOC) ──→       NOT NEEDED ²
  CLOG reader                ──→       NOT NEEDED ²
  Hint bits                  ──→       NOT NEEDED ²
  MultiXact resolution       ──→       NOT NEEDED ²
  Snapshot management        ──→       NOT NEEDED ²
  Visibility map             ──→       NOT NEEDED ²
  TOAST decompression        ──→       NOT NEEDED ³
  Segment file handling      ──→       NOT NEEDED
  Torn page detection        ──→       NOT NEEDED
  pg_control parsing         ──→       NOT NEEDED ⁴
  $PGDATA/ access            ──→       NOT NEEDED

¹ pgoutput sends decoded column values, not raw binary
² Logical stream only contains committed, visible changes
³ pgoutput sends detoasted values — TOAST is handled by PostgreSQL
⁴ Only needs replication protocol, not data directory access
```

**Arrow store state:**

```rust
struct ArrowStore {
    tables: HashMap<String, TableState>,
    confirmed_lsn: Lsn,
}

struct TableState {
    /// Immutable Arrow batches (from checkpoint + compacted inserts)
    frozen_batches: Vec<RecordBatch>,
    /// Recent inserts not yet compacted (append-only buffer)
    write_buffer: ArrowWriteBuffer,
    /// Deletion bitmap — marks rows deleted across all batches
    deleted: RoaringBitmap,
    /// Row ID counter
    next_row_id: u64,
    /// Primary key → row_id index (for UPDATE/DELETE lookups)
    pk_index: HashMap<Vec<ScalarValue>, u64>,
}

impl TableState {
    fn apply_insert(&mut self, values: Vec<ScalarValue>) {
        let row_id = self.next_row_id;
        self.next_row_id += 1;
        self.pk_index.insert(self.extract_pk(&values), row_id);
        self.write_buffer.append(row_id, values);
    }

    fn apply_delete(&mut self, old_key: &[ScalarValue]) {
        if let Some(row_id) = self.pk_index.remove(old_key) {
            self.deleted.insert(row_id as u32);
        }
    }

    fn apply_update(&mut self, old_key: &[ScalarValue], new_values: Vec<ScalarValue>) {
        self.apply_delete(old_key);
        self.apply_insert(new_values);
    }

    /// Read path: frozen batches + write buffer, filter out deleted
    fn scan(&self, projection: &[usize]) -> impl Iterator<Item = RecordBatch> {
        self.frozen_batches.iter()
            .chain(self.write_buffer.as_batches().iter())
            .map(|batch| filter_batch(batch, &self.deleted, projection))
            .filter(|batch| batch.num_rows() > 0)
    }
}
```

**Compaction** (LSM-style — merge write buffer into frozen batches, remove deleted rows):

```rust
impl TableState {
    fn compact(&mut self) {
        let all_batches: Vec<RecordBatch> = self.frozen_batches.drain(..)
            .chain(self.write_buffer.drain())
            .collect();

        let mut new_batches = Vec::new();
        let mut row_offset = 0u64;
        for batch in all_batches {
            let live_indices: Vec<u32> = (0..batch.num_rows() as u32)
                .filter(|i| !self.deleted.contains(row_offset as u32 + i))
                .collect();
            if !live_indices.is_empty() {
                new_batches.push(take_batch(&batch, &live_indices));
            }
            row_offset += batch.num_rows() as u64;
        }

        self.frozen_batches = new_batches;
        self.write_buffer.clear();
        self.deleted.clear();
        self.rebuild_pk_index();
    }
}
```

**Parquet checkpoint and crash recovery:**

```
$PG_ARROW_DATA/
  checkpoints/
    checkpoint_0x5A000120/        ← LSN of this checkpoint
      users.parquet
      orders.parquet
      manifest.json               ← table list, schemas, row counts, LSN
    checkpoint_0x5B000200/
      ...
  wal_position                    ← last confirmed LSN

Startup (crash recovery):
  1. Find latest checkpoint: checkpoint_0x5B000200
  2. Load Parquet files → frozen_batches
  3. Connect to primary, resume logical replication from 0x5B000200
  4. Apply buffered changes until caught up
  5. Start accepting queries

  Recovery time = Parquet load + replay since checkpoint
  With hourly checkpoints: seconds to minutes
```

**DDL handling**: Logical replication sends `Relation` messages when schema changes, but
does not replicate DDL itself. pg_arrow detects schema changes from `Relation` messages in
the stream and evolves Arrow schemas (add columns with nulls, handle type changes). Periodic
polling of `pg_class` detects new/dropped tables.

**Requirements:**

| Requirement | Details |
|---|---|
| PostgreSQL version | 10+ for logical replication; 16+ if consuming from standby |
| `REPLICA IDENTITY` | Required on all published tables (DEFAULT uses PK, FULL for no-PK tables) |
| Publication | `CREATE PUBLICATION pg_arrow_pub FOR ALL TABLES` on primary |
| Replication slot | `CREATE_REPLICATION_SLOT pg_arrow_data LOGICAL pgoutput` |
| `wal_level` | Must be `logical` on primary |
| `max_replication_slots` | At least 1 additional slot |
| Network | pg_arrow needs network access to primary (no filesystem access) |

### WAL Synchronization Levels (Modes 1 & 2 only)

Modes 1 and 2 (heap file reading) can integrate with the WAL stream at increasing levels
of sophistication for cache invalidation. Each level builds on the previous.

#### Level 1: Recovery LSN Synchronization

Use `pg_last_wal_replay_lsn()` (replica) or `pg_current_wal_lsn()` (primary) as the
consistency point. On replica, optionally pause recovery for torn-page-free reads.

#### Level 2: Physical WAL Stream for Cache Invalidation

pg_arrow receives the physical WAL stream and parses it to know exactly which pages changed:

```rust
struct WalMonitor {
    replication_conn: ReplicationConnection,
    dirty_pages: HashMap<Oid, HashSet<BlockNumber>>,
}

impl WalMonitor {
    fn process_wal_record(&mut self, record: &WalRecord) {
        match record.resource_manager {
            RmgrId::Heap => {
                self.dirty_pages
                    .entry(record.target_relation())
                    .or_default()
                    .insert(record.target_block());
            }
            RmgrId::Heap2 => { /* VACUUM, FREEZE — invalidate affected pages */ }
            _ => {}
        }
    }

    fn take_dirty_pages(&mut self, rel_oid: Oid) -> HashSet<BlockNumber> {
        self.dirty_pages.remove(&rel_oid).unwrap_or_default()
    }
}
```

**Performance**: 70GB table, 100 pages changed → read 800KB from disk, serve rest from cache.

**Full-page image bonus**: When `full_page_writes = on` (default), the first modification
after a checkpoint writes the complete 8KB page into WAL. pg_arrow extracts these and updates
its cache directly — zero file I/O for those pages.

#### Level 3: Per-Table Hybrid Strategy

Combine heap file reading (Levels 1-2) with logical replication for hot tables on the same
deployment. See "Level 4: Hybrid" in the Mode 3 section — this applies within Modes 1-2 as
well, using logical replication for frequently-queried tables while reading heap files for
cold tables.

### Alternative Architectures (Considered)

These alternatives from early design exploration were evaluated and set aside, documented
for reference.

**PostgreSQL Extension (ParadeDB/pg_analytics approach)**: Hook into PostgreSQL's executor
via `shared_preload_libraries`, redirect analytical queries to DataFusion. Pros: fully
promotable, drop-in, no data duplication. Cons: C/Rust interop complexity, tied to PostgreSQL
release cycle, maintenance burden.

**PostgreSQL Fork**: Fork PostgreSQL and replace the executor with DataFusion while keeping
the storage layer. Pros: full control, promotable. Cons: massive maintenance burden, diverges
from upstream PostgreSQL, community fragmentation.

**Logical Replication Subscriber with Full PostgreSQL**: A full PostgreSQL instance that
subscribes via logical replication but adds DataFusion for reads. Pros: promotable (it's
real PostgreSQL), standard replication. Cons: duplicates data (full PG instance), heavy
resource usage.

**Our choice — three deployment modes** — covers the full spectrum: sidecar for promotion
and heap-level access (Modes 1-2), standalone logical replica for simplicity and remote
deployment (Mode 3). No PostgreSQL fork/extension maintenance required.

## Production Readiness

Sections below cover operational concerns required for a production-grade deployment
that are not part of the core data path but are essential for reliability and operability.

### Observability

pg_arrow must expose comprehensive metrics, traces, and logs for production operation.

**Prometheus metrics** (exposed on HTTP `/metrics`):

| Metric | Type | What It Measures |
|---|---|---|
| `pg_arrow_queries_total` | Counter | Total queries executed (by status: success/error/timeout) |
| `pg_arrow_query_duration_seconds` | Histogram | Query latency distribution |
| `pg_arrow_pages_read_total` | Counter | Heap pages read from disk |
| `pg_arrow_pages_cache_hits_total` | Counter | Pages served from Arrow cache |
| `pg_arrow_cache_hit_ratio` | Gauge | Cache hit rate (pages_cache_hits / total_page_accesses) |
| `pg_arrow_clog_lookups_total` | Counter | CLOG page reads |
| `pg_arrow_toast_decompressions_total` | Counter | TOAST chunk reassemblies |
| `pg_arrow_active_connections` | Gauge | Current client connections |
| `pg_arrow_arrow_batches_converted_total` | Counter | RecordBatches produced |
| `pg_arrow_memory_usage_bytes` | Gauge | Current memory usage (cache + query) |
| `pg_arrow_replication_lag_bytes` | Gauge | WAL lag behind primary (replica mode) |
| `pg_arrow_pg_connection_pool_active` | Gauge | Active PostgreSQL backend connections |
| `pg_arrow_pg_connection_errors_total` | Counter | PostgreSQL connection failures |

**OpenTelemetry tracing** — distributed trace through the query lifecycle:

```
Span: query_execute (query_id=abc123, sql="SELECT ...")
  ├─ Span: datafusion_plan (logical_plan=..., physical_plan=...)
  ├─ Span: pg_arrow_scan (table=users, pages=1234)
  │    ├─ Span: page_read (page=0..100, cache_hits=80, disk_reads=20)
  │    ├─ Span: mvcc_visibility (tuples_checked=5000, visible=4800)
  │    ├─ Span: clog_lookup (xids=200, clog_pages_read=1)
  │    ├─ Span: toast_detoast (chunks=15, decompressed_bytes=45000)
  │    └─ Span: arrow_convert (batches=5, rows=4800)
  ├─ Span: datafusion_execute (partitions=4, output_rows=150)
  └─ Span: wire_protocol_send (rows=150, bytes=12400)
```

**Structured logging**: JSON-formatted logs with levels, query IDs, and durations. Use
`tracing` crate with `tracing-subscriber` for structured output.

**Health endpoints** (HTTP, separate from wire protocol port):

| Endpoint | Purpose |
|---|---|
| `GET /health` | Liveness: pg_arrow process is running |
| `GET /ready` | Readiness: PG connection up, schema loaded, accepting queries |
| `GET /metrics` | Prometheus metrics |
| `GET /status` | JSON: version, uptime, cache stats, connection count, replication lag |

### pg_arrow Configuration

pg_arrow needs its own configuration file for settings not derived from PostgreSQL.

```toml
# pg_arrow.toml

[server]
listen_address = "0.0.0.0"
wire_protocol_port = 5433         # PostgreSQL wire protocol
flight_sql_port = 5434            # Arrow Flight SQL
health_port = 9090                # HTTP health + metrics
max_connections = 200
connection_timeout_secs = 30
idle_connection_timeout_secs = 300
query_timeout_secs = 600          # 10 min default

[postgresql]
data_directory = "/var/lib/postgresql/17/main"
connection_string = "host=/var/run/postgresql dbname=mydb"
pool_size = 5                     # connections to PG for schema/snapshot/CLOG
pool_timeout_secs = 10

[cache]
max_memory_mb = 4096              # Arrow page cache memory limit
persistent_cache_dir = "/var/lib/pg_arrow/cache"
persistent_cache_max_gb = 50      # Parquet cache on disk
warm_up_tables = ["public.hits", "public.orders"]  # pre-load on startup

[wal]
sync_level = "wal_invalidation"   # heap_scan | recovery_lsn | wal_invalidation | logical
physical_slot = "pg_arrow_cache"
logical_slot = "pg_arrow_data"    # only if sync_level = logical

[logging]
level = "info"                    # trace, debug, info, warn, error
format = "json"                   # json or text
file = "/var/log/pg_arrow/pg_arrow.log"

[tracing]
otlp_endpoint = "http://localhost:4317"  # OpenTelemetry collector
sample_rate = 0.1                        # sample 10% of queries

[security]
mode = "trusted"                  # trusted | proxy_auth | full_permissions
tls_cert = "/etc/pg_arrow/server.crt"
tls_key = "/etc/pg_arrow/server.key"
```

**Runtime reconfigurable** (via `SIGHUP` or admin endpoint): `logging.level`,
`cache.max_memory_mb`, `query_timeout_secs`, `tracing.sample_rate`.

**Startup-only** (require restart): `listen_address`, ports, `data_directory`,
`connection_string`, `security.mode`.

### Graceful Lifecycle Management

```
Startup sequence:
  1. Parse config file + CLI args + environment variables
  2. Validate pg_control: block_size, PG version, cluster state
  3. Establish PostgreSQL connection pool
  4. Load schema cache (pg_class, pg_attribute, pg_type)
  5. Load persistent Arrow cache index (if exists)
  6. Start background jobs (WAL monitor, health check, VM monitor, etc.)
  7. Warm up tables listed in config (pre-convert to Arrow cache)
  8. Start accepting connections on wire protocol + Flight SQL ports
  9. Mark /ready endpoint as healthy

Shutdown (SIGTERM — graceful):
  1. Stop accepting new connections
  2. Wait for in-flight queries to complete (up to drain_timeout)
  3. Close all client connections
  4. Persist Arrow cache to Parquet (if persistent_cache_dir configured)
  5. Close PostgreSQL connection pool
  6. Flush metrics and logs
  7. Exit 0

Shutdown (SIGINT — fast):
  1. Cancel in-flight queries
  2. Close all connections immediately
  3. Best-effort cache persist
  4. Exit 0

Config reload (SIGHUP):
  1. Re-read pg_arrow.toml
  2. Apply runtime-reconfigurable settings
  3. Log changed settings
```

### Error Handling and Resilience

**Error taxonomy:**

| Error Type | Example | Response |
|---|---|---|
| Fatal | pg_control corrupt, wrong PG version, data_dir missing | Log, refuse to start |
| Connection loss | PG connection drops | Reconnect with exponential backoff + circuit breaker |
| Query error | Invalid SQL, type mismatch | Return ErrorResponse to client, continue serving |
| Page error | Torn page, checksum failure | Retry once; if persistent, skip page + warn |
| Timeout | Query exceeds timeout | Cancel query, return error to client |
| Memory pressure | Approaching memory limit | Evict cache, reject new queries if critical |

**Degraded mode** — when PostgreSQL connection is lost:

```
Normal mode:                    Degraded mode (PG down):
├─ Frozen pages → from cache    ├─ Frozen pages → from cache ✅
├─ Non-frozen → CLOG lookup     ├─ Non-frozen → CANNOT resolve ❌
├─ Schema → from PG             ├─ Schema → from cache ✅ (if cached)
├─ Snapshots → from PG          ├─ Snapshots → CANNOT acquire ❌
└─ All queries work              └─ Only frozen-data queries work
```

When PG connection is lost, pg_arrow can continue serving queries against all-frozen pages
(visibility map says all-frozen → no CLOG/snapshot needed). This covers VACUUM FREEZEd data,
which is typically the majority of historical data. New/recent data is unavailable until
PG reconnects.

**Circuit breaker for PG connection:**

```
States: Closed (healthy) → Open (broken) → Half-Open (testing)

Closed:  PG queries work normally
         After 3 consecutive failures → Open

Open:    All PG queries fail immediately (don't pile up)
         After 30 seconds → Half-Open

Half-Open: Try one PG query
           If success → Closed (recovered)
           If failure → Open (still broken)
```

### Connection Management

| Setting | Default | Description |
|---|---|---|
| `max_connections` | 200 | Maximum concurrent client connections |
| `connection_timeout_secs` | 30 | Max time to complete authentication handshake |
| `idle_connection_timeout_secs` | 300 | Close idle connections after 5 minutes |
| `query_timeout_secs` | 600 | Cancel queries running longer than 10 minutes |
| `max_concurrent_queries` | 50 | Admission control — queue excess queries |
| `per_query_memory_limit_mb` | 512 | DataFusion memory limit per query |

When `max_concurrent_queries` is reached, new queries are queued (up to queue depth limit)
with backpressure propagated to the client via delayed response.

### Collation Handling

PostgreSQL `ORDER BY` depends on database/column collation. DataFusion uses Rust's `Ord`
(byte-level comparison) by default. This produces **different sort orders** for non-ASCII text.

```sql
-- PostgreSQL with en_US.UTF-8 collation:
SELECT name FROM users ORDER BY name;
-- Returns: Ángela, Björk, Čedomir  (locale-aware sorting)

-- DataFusion (byte order):
-- Returns: Björk, Čedomir, Ángela  (different!)
```

**Approach:**

| Collation | Strategy | Effort |
|---|---|---|
| `C` / `POSIX` | Byte-order — DataFusion default is correct | None |
| `C.UTF-8` | Byte-order on UTF-8 — DataFusion correct | None |
| ICU collations | Use `icu` crate for locale-aware comparison | Medium |
| libc collations (`en_US.UTF-8`) | Use `icu` crate (libc collations are OS-dependent) | Medium |

For Phase 1, document the limitation: `ORDER BY` on text columns is only correct for `C`
and `POSIX` collations. ICU collation support is a Phase 7+ feature.

Read the database's default collation from `pg_database.datcollate` and warn on startup if
it's not `C`-based.

### Schema Evolution (ALTER TABLE Handling)

When `ALTER TABLE ADD COLUMN` is executed, existing tuples on disk **do not get rewritten**.
They have fewer attributes than the current schema says. PostgreSQL handles this by checking
`HeapTupleHeaderGetNatts(tuple)` vs the current `pg_attribute` count.

```
Before ALTER:  tuple has columns (id, name)        — natts = 2
After ALTER:   schema says (id, name, email TEXT DEFAULT 'none')  — natts = 3

Old tuple on disk: [id=1, name='alice']  ← only 2 attributes
New tuple on disk: [id=2, name='bob', email='bob@x.com']  ← 3 attributes
```

pg_arrow must handle this:

```rust
fn decode_tuple(tuple: &HeapTuple, schema: &[PgAttribute]) -> Result<Vec<Datum>> {
    let tuple_natts = tuple.header.t_infomask2 & HEAP_NATTS_MASK;
    let schema_natts = schema.len();

    let mut values = Vec::with_capacity(schema_natts);

    for (i, attr) in schema.iter().enumerate() {
        if attr.attisdropped {
            // Column was DROP COLUMN'd — skip, fill with NULL
            values.push(Datum::Null);
        } else if i >= tuple_natts as usize {
            // Tuple predates ALTER TABLE ADD COLUMN — use column default or NULL
            values.push(attr.default_value.clone().unwrap_or(Datum::Null));
        } else {
            values.push(decode_attribute(tuple, i, attr)?);
        }
    }
    Ok(values)
}
```

Key considerations:

- `pg_attribute.attisdropped = true` → column was dropped, slot is a placeholder (always NULL)
- `pg_attribute.atthasmissing = true` + `pg_attribute.attmissingval` → default value for
  tuples that predate `ADD COLUMN` (PostgreSQL 11+)
- `pg_attribute.attnum` → physical position in tuple (can differ from logical if columns dropped)
- Schema cache must be invalidated when `pg_class.relnatts` or `pg_attribute` changes

### Multi-Database Support

A PostgreSQL cluster contains multiple databases. Each database has its own `base/<db_oid>/`
directory with its own set of tables.

```
$PGDATA/
  base/
    1/          ← template1 (db_oid=1)
    16384/      ← mydb (db_oid=16384)
    16385/      ← analytics (db_oid=16385)
  global/       ← shared catalogs (pg_database, pg_authid, etc.)
```

pg_arrow handles this via the startup message's `database` parameter:

```
Client connects: StartupMessage { database: "analytics", user: "alice" }

pg_arrow:
  1. Look up db_oid: SELECT oid FROM pg_database WHERE datname = 'analytics'
     → 16385
  2. Set data path: base/16385/
  3. Load schema from this database's catalogs
  4. All subsequent queries operate within this database
```

Each client connection is scoped to one database (same as PostgreSQL). Different connections
can target different databases. The PostgreSQL connection pool should maintain per-database
connections.

### Extension Type Handling

PostgreSQL extensions add custom types that pg_arrow encounters in heap files.

| Extension | Type | Arrow Representation | Strategy |
|---|---|---|---|
| PostGIS | `geometry` | Binary (WKB) | Store as Arrow Binary; consumers use GeoArrow |
| PostGIS | `geography` | Binary (WKB) | Same as geometry |
| pgvector | `vector(N)` | `FixedSizeList<Float32, N>` | Parse binary format (dimension + float array) |
| hstore | `hstore` | `Map<Utf8, Utf8>` | Parse binary format (key-value pairs) |
| citext | `citext` | `Utf8` | Identical to text in storage |
| ltree | `ltree` | `Utf8` | Store as text representation |
| pg_trgm | trigrams | N/A | Index-only, no storage type |
| uuid-ossp | `uuid` | `FixedSizeBinary(16)` | Already a core PG type |

**Default strategy for unknown types**: Store as Arrow `Binary` with the raw PostgreSQL binary
representation. Consumers can interpret the bytes if they understand the type. This ensures
pg_arrow never fails on unknown extension types — it just stores them opaquely.

```rust
fn type_to_arrow(pg_type: &PgType) -> ArrowDataType {
    match pg_type.oid {
        // Core types
        INT4OID => ArrowDataType::Int32,
        TEXTOID => ArrowDataType::Utf8,
        // ... etc

        // Known extensions
        oid if is_postgis_geometry(oid) => ArrowDataType::Binary,
        oid if is_pgvector(oid) => {
            let dim = pg_type.typmod; // vector dimension
            ArrowDataType::FixedSizeList(Box::new(ArrowDataType::Float32), dim)
        }

        // Unknown: opaque binary fallback
        _ => {
            warn!("Unknown type OID {}, storing as Binary", pg_type.oid);
            ArrowDataType::Binary
        }
    }
}
```

### Numeric Precision Edge Cases

PostgreSQL `NUMERIC` is arbitrary precision (unlimited digits). Arrow `Decimal128` supports
up to 38 digits (precision 1-38).

| Scenario | PostgreSQL | Arrow | Strategy |
|---|---|---|---|
| `NUMERIC(10,2)` | Fits in Decimal128 | `Decimal128(10,2)` | Direct mapping |
| `NUMERIC(38,0)` | Fits in Decimal128 | `Decimal128(38,0)` | Direct mapping |
| `NUMERIC(39,0)` | 39 digits | Overflow! | Promote to `Decimal256` or store as `Utf8` |
| `NUMERIC` (no precision) | Arbitrary | Unknown at scan time | Scan first batch to detect; fallback to `Utf8` |
| `NaN` | Valid NUMERIC value | Not representable in Decimal | Map to `null` + warn, or `Utf8` |
| `Infinity` | Not valid for NUMERIC | N/A | N/A |

**Strategy**: Use `Decimal128` when typmod specifies precision ≤ 38. For unbound `NUMERIC` or
precision > 38, use `Decimal256` (Arrow supports it) or fall back to `Utf8` string
representation. `NaN` maps to null with a warning.

## Testing and Validation Strategy

pg_arrow is a binary format parser reading data from disk that PostgreSQL is actively writing.
Every parsing path is a potential crash surface, and every query result must exactly match what
PostgreSQL returns. This requires a multi-layered testing approach.

### Fuzz Testing

Every binary parsing path must be fuzz-tested. A malformed page, corrupt item pointer, or lying
TOAST header must never crash the process — only return `Err`.

**Fuzz surfaces:**

```
Inputs that MUST be fuzz-tested:
├── Page headers (24 bytes)          ← pd_lower > pd_upper? pd_special past page end?
├── Item pointers (4 bytes each)     ← lp_off points outside page? lp_len overflows?
├── Tuple headers (23+ bytes)        ← t_hoff > tuple length? null bitmap overflows?
├── Tuple data (variable)            ← varlena length > remaining bytes? numeric ndigits?
├── TOAST chunks                     ← chunk_seq out of order? decompressed size lie?
├── CLOG pages (8KB)                 ← xid maps to byte outside page?
├── Visibility map pages             ← bit index > page count?
├── BRIN index pages                 ← min > max? corrupt summary tuples?
├── pg_control (296 bytes)           ← block_size=0? segment_size=0?
└── Wire protocol input              ← malformed startup, oversized messages
```

**Tooling**: `cargo-fuzz` with libFuzzer backend, `arbitrary` crate for structured fuzzing.

**Fuzz targets:**

```rust
// fuzz/fuzz_targets/fuzz_page_header.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use pg_arrow_core::page::PageHeader;

fuzz_target!(|data: &[u8]| {
    // Should NEVER panic — only return Err
    let _ = PageHeader::parse(data);
});
```

```rust
// fuzz/fuzz_targets/fuzz_heap_page.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use pg_arrow_core::page::HeapPage;

fuzz_target!(|data: &[u8]| {
    if let Ok(page) = HeapPage::parse(data) {
        // If parsing succeeds, iteration must not panic
        for tuple_result in page.tuples() {
            let _ = tuple_result;
        }
    }
});
```

```rust
// fuzz/fuzz_targets/fuzz_tuple_decode.rs — structured fuzzing with Arbitrary
#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;
use pg_arrow_core::tuple::decode_datum;
use pg_arrow_core::types::PgType;

#[derive(Arbitrary, Debug)]
struct FuzzInput {
    type_id: u8,
    data: Vec<u8>,
    typlen: i16,
    typmod: i32,
}

fuzz_target!(|input: FuzzInput| {
    let pg_type = match input.type_id % 15 {
        0 => PgType::Bool,
        1 => PgType::Int16,
        2 => PgType::Int32,
        3 => PgType::Int64,
        4 => PgType::Float32,
        5 => PgType::Float64,
        6 => PgType::Text,
        7 => PgType::Bytea,
        8 => PgType::Timestamp,
        9 => PgType::Date,
        10 => PgType::Numeric,
        11 => PgType::Jsonb,
        12 => PgType::Uuid,
        13 => PgType::Varchar,
        _ => PgType::Text,
    };
    let _ = decode_datum(&input.data, pg_type, input.typlen, input.typmod);
});
```

```rust
// fuzz/fuzz_targets/fuzz_toast_decompress.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use pg_arrow_core::toast::{pglz_decompress, lz4_decompress};

fuzz_target!(|data: &[u8]| {
    if data.len() < 4 { return; }
    let claimed_size = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    // Claimed decompressed size could be a lie — must handle gracefully
    let _ = pglz_decompress(&data[4..], claimed_size as usize);
    let _ = lz4_decompress(&data[4..], claimed_size as usize);
});
```

```rust
// fuzz/fuzz_targets/fuzz_wire_protocol.rs
#![no_main]
use libfuzzer_sys::fuzz_target;
use pg_arrow_core::protocol::parse_message;

fuzz_target!(|data: &[u8]| {
    let _ = parse_message(data);
});
```

**Corpus seeding** — real PostgreSQL pages provide the best starting corpus:

```bash
# Extract individual pages as seed files from a real heap file
python3 -c "
import sys
data = open(sys.argv[1], 'rb').read()
block_size = 8192
for i in range(0, len(data), block_size):
    page = data[i:i+block_size]
    open(f'fuzz/corpus/fuzz_heap_page/page_{i//block_size:04d}', 'wb').write(page)
" testdata/postgres-latest/base/16384/16385
```

**Running:**

```bash
cargo fuzz run fuzz_page_header                         # runs indefinitely
cargo fuzz run fuzz_page_header -- -max_total_time=300  # 5 min (CI)
cargo fuzz tmin fuzz_page_header crash-abc123            # minimize crash
cargo fuzz coverage fuzz_page_header                     # coverage report
```

### Property-Based Testing

Fuzz testing finds crashes. Property-based testing finds **logic bugs** by verifying that
invariants hold across thousands of random inputs.

**Tooling**: `proptest` crate.

**Round-trip properties** — encode → decode must return the original value:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn roundtrip_int32(value: i32) {
        let pg_bytes = encode_pg_int32(value);
        let decoded = decode_datum(&pg_bytes, PgType::Int32, 4, -1)?;
        prop_assert_eq!(decoded, ScalarValue::Int32(Some(value)));
    }

    #[test]
    fn roundtrip_text(s in "\\PC{0,10000}") {
        let pg_bytes = encode_pg_text(&s);
        let decoded = decode_datum(&pg_bytes, PgType::Text, -1, -1)?;
        prop_assert_eq!(decoded, ScalarValue::Utf8(Some(s)));
    }

    #[test]
    fn roundtrip_numeric(
        sign in 0u16..2u16,
        weight in -100i16..100i16,
        dscale in 0u16..100u16,
        digits in prop::collection::vec(0u16..10000u16, 0..50),
    ) {
        let pg_numeric = encode_pg_numeric(sign, weight, dscale, &digits);
        let result = decode_datum(&pg_numeric, PgType::Numeric, -1, -1);
        // Must not panic — either valid decode or clean error
        prop_assert!(result.is_ok() || result.is_err());
    }
}
```

**Page invariant properties:**

```rust
proptest! {
    #[test]
    fn page_header_invariants(data in prop::collection::vec(any::<u8>(), 8192)) {
        if let Ok(header) = PageHeader::parse(&data) {
            prop_assert!(header.pd_lower as usize <= data.len());
            prop_assert!(header.pd_upper as usize <= data.len());
            prop_assert!(header.pd_lower <= header.pd_upper);
            for lp in header.item_ids() {
                if lp.is_used() {
                    prop_assert!(lp.offset() >= header.pd_upper);
                    prop_assert!(lp.offset() + lp.length() <= data.len() as u16);
                }
            }
        }
    }

    #[test]
    fn arrow_batch_schema_matches(
        num_rows in 1usize..1000,
        num_cols in 1usize..20,
    ) {
        let table = test_table_with_columns(num_cols);
        let schema = table.arrow_schema();
        let batches = table.scan_rows(num_rows);
        for batch in batches {
            prop_assert_eq!(batch.schema().as_ref(), schema);
            prop_assert!(batch.num_rows() <= num_rows);
        }
    }
}
```

**MVCC visibility properties:**

```rust
proptest! {
    #[test]
    fn frozen_tuples_always_visible(
        snapshot_xmin in 1u32..u32::MAX,
        snapshot_xmax in 1u32..u32::MAX,
    ) {
        let tuple = TupleHeader {
            t_xmin: 100,
            t_infomask: HEAP_XMIN_FROZEN,
            t_xmax: 0,
            ..Default::default()
        };
        let snapshot = Snapshot {
            xmin: snapshot_xmin,
            xmax: snapshot_xmax.max(snapshot_xmin),
            xip: vec![],
        };
        // Frozen tuples visible to ALL snapshots — no exceptions
        prop_assert!(is_tuple_visible(&tuple, &snapshot));
    }

    #[test]
    fn aborted_insert_never_visible(
        snapshot_xmin in 1u32..u32::MAX,
        snapshot_xmax in 1u32..u32::MAX,
    ) {
        let tuple = TupleHeader {
            t_xmin: 200,
            t_infomask: HEAP_XMIN_INVALID,
            ..Default::default()
        };
        let snapshot = Snapshot {
            xmin: snapshot_xmin,
            xmax: snapshot_xmax.max(snapshot_xmin),
            xip: vec![],
        };
        // Aborted inserts visible to NOBODY
        prop_assert!(!is_tuple_visible(&tuple, &snapshot));
    }
}
```

### Differential Testing Against PostgreSQL

The core correctness strategy: run the same operation through both PostgreSQL and pg_arrow,
diff the results. Any difference is a bug.

```
PostgreSQL (ground truth)          pg_arrow
        │                              │
  SELECT * FROM t WHERE x > 10    Same query
        │                              │
        ▼                              ▼
   Result set A                   Result set B
        │                              │
        └──────── DIFF ────────────────┘
                   │
            Must be identical
            (after ORDER BY + type normalization)
```

**Layer 1: Page-level — `pageinspect` as ground truth**

PostgreSQL's `pageinspect` extension lets us inspect raw page/tuple data field-by-field:

```sql
-- Install
CREATE EXTENSION pageinspect;

-- Page header fields
SELECT * FROM page_header(get_raw_page('users', 0));
-- lsn | checksum | flags | lower | upper | special | pagesize | version | prune_xid

-- Item pointers and tuple headers
SELECT * FROM heap_page_items(get_raw_page('users', 0));
-- lp | lp_off | t_xmin | t_xmax | t_infomask | t_infomask2 | t_ctid | t_data (hex)

-- Tuple data with attribute-level breakdow
SELECT * FROM heap_page_item_attrs(get_raw_page('users', 0), 'users'::regclass);
-- lp | lp_off | lp_flags | lp_len | t_xmin | t_xmax | t_infomask | t_infomask2 | t_attrs
```

```rust
#[test]
fn test_page_header_matches_pageinspect() {
    let pg_header = query_pg("SELECT * FROM page_header(get_raw_page('users', 0))");
    let our_header = read_page_header("base/16384/16385", 0);
    assert_eq!(pg_header.pd_lsn, our_header.pd_lsn);
    assert_eq!(pg_header.pd_lower, our_header.pd_lower);
    assert_eq!(pg_header.pd_upper, our_header.pd_upper);
    assert_eq!(pg_header.pd_checksum, our_header.pd_checksum);
    assert_eq!(pg_header.pd_flags, our_header.pd_flags);
}
```

**Layer 2: Tuple-level — full table scan diffing**

```rust
#[test]
fn test_full_table_scan_matches_pg() {
    let pg_rows = pg_query("SELECT id, name, value FROM test_table ORDER BY id");

    let cluster = PgCluster::open(&data_dir)?;
    let table = cluster.table(db_oid, table_oid)?;
    let batches: Vec<RecordBatch> = table.scan(ScanOptions::default())?.collect();
    let arrow_rows = sort_by_column(&batches, "id");

    assert_rows_equal(&pg_rows, &arrow_rows);
}
```

**Layer 3: Query-level — SQL regression suite extraction**

| PostgreSQL Source | What's Useful | Approach |
|---|---|---|
| `src/test/regress/sql/select.sql` | SELECT queries with joins, subqueries, CTEs | Extract read-only queries |
| `src/test/regress/sql/aggregates.sql` | Aggregate functions, GROUP BY, HAVING | Extract and run through both engines |
| `src/test/regress/sql/window.sql` | Window functions | Extract and diff results |
| `src/test/regress/sql/join.sql` | All join types | Extract and diff results |
| `src/test/regress/sql/subselect.sql` | Subqueries, EXISTS, IN | Extract and diff results |
| `src/test/regress/sql/groupingsets.sql` | GROUPING SETS, CUBE, ROLLUP | Extract and diff results |
| `src/test/regress/data/` | CSV test data loaded by COPY | Load same data into test PG instance |
| `src/test/regress/expected/` | Expected output | Compare pg_arrow output against same expected |

```rust
fn differential_test(query: &str) {
    let pg_result = pg_conn.query(query, &[]);
    let arrow_result = pg_arrow_conn.query(query, &[]);

    let pg_normalized = normalize_result(pg_result);     // sort rows, round floats, normalize NULLs
    let arrow_normalized = normalize_result(arrow_result);

    assert_eq!(pg_normalized, arrow_normalized, "Query mismatch: {}", query);
}

#[test]
fn test_regression_selects() {
    let queries = extract_selects_from("src/test/regress/sql/select.sql");
    for query in queries {
        differential_test(&query);
    }
}
```

### MVCC Visibility Validation

Visibility is the hardest part to validate. Strategy: create known visibility scenarios with
known transaction IDs, take a snapshot, and verify pg_arrow sees exactly what PostgreSQL sees.

```sql
-- Create known visibility scenarios
BEGIN;  -- txid = 100
INSERT INTO vis_test VALUES (1, 'visible');
COMMIT;

BEGIN;  -- txid = 101
INSERT INTO vis_test VALUES (2, 'deleted');
DELETE FROM vis_test WHERE id = 2;
COMMIT;

BEGIN;  -- txid = 102
INSERT INTO vis_test VALUES (3, 'uncommitted');
-- DON'T COMMIT — leave in-progress

-- Take snapshot
SELECT pg_current_snapshot();  -- e.g., '103:103:'
-- pg_arrow with this snapshot should see: id=1 only
-- id=2 deleted (committed delete), id=3 in-progress (not visible)
```

```rust
#[test]
fn test_mvcc_visibility_scenarios() {
    setup_visibility_scenarios(&pg_conn);
    let snapshot = pg_query_one("SELECT pg_current_snapshot()");

    let visible_rows = pg_arrow_scan_with_snapshot(&table, &snapshot);
    let pg_rows = pg_query("SELECT * FROM vis_test ORDER BY id");

    assert_rows_equal(&visible_rows, &pg_rows);
}
```

### Test Data Generation

Automated generation of tables that exercise every code path:

```sql
-- Type coverage: every supported PostgreSQL type
CREATE TABLE type_test (
    i16 smallint, i32 integer, i64 bigint,
    f32 real, f64 double precision,
    b boolean, t text, byt bytea,
    ts timestamp, tstz timestamptz, d date,
    n numeric(18,4), j jsonb, u uuid,
    arr_int integer[], arr_text text[]
);

-- Edge case: all NULLs
INSERT INTO type_test VALUES (
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL
);

-- Edge case: min/max/special values
INSERT INTO type_test VALUES (
    -32768, -2147483648, -9223372036854775808,
    'NaN'::real, 'Infinity'::float8,
    true, '', '\x',
    '0001-01-01', '0001-01-01 00:00:00+00', '0001-01-01',
    0.0000, '{}', '00000000-0000-0000-0000-000000000000',
    '{}', '{}'
);

-- Edge case: TOAST trigger (values >2KB get TOASTed)
INSERT INTO type_test (t, j) VALUES (
    repeat('x', 10000),
    ('{"key": "' || repeat('v', 10000) || '"}')::jsonb
);

-- Edge case: row locking (xmax overloading)
BEGIN;
SELECT * FROM type_test WHERE i32 = -2147483648 FOR UPDATE;
COMMIT;  -- xmax set but LOCK_ONLY flag

-- Edge case: aborted transaction
BEGIN;
INSERT INTO type_test (i32) VALUES (999);
ROLLBACK;  -- tuple exists on page but xmin is aborted
```

### Chaos / Fault Injection Testing

Simulate what happens when the PostgreSQL data directory is in a bad state — pg_arrow must
handle every case gracefully without panicking.

```rust
#[cfg(test)]
mod chaos_tests {
    // Torn page: valid header but truncated data
    #[test]
    fn torn_page_half_written() {
        let real_page = read_real_page("base/16384/16385", 0);
        let torn = &real_page[..4096];
        let result = HeapPage::parse(torn);
        assert!(result.is_err());
    }

    // Zero page: newly allocated but unwritten
    #[test]
    fn zero_page() {
        let zero_page = vec![0u8; 8192];
        let result = HeapPage::parse(&zero_page);
        match result {
            Ok(page) => assert_eq!(page.tuple_count(), 0),
            Err(_) => {}  // also acceptable
        }
    }

    // File truncated during read (simulates concurrent VACUUM)
    #[test]
    fn file_truncated_during_read() {
        let file = create_test_heap_file(100);  // 100 pages
        let reader = SegmentReader::open(&file)?;

        truncate_file(&file, 50 * 8192);  // simulate VACUUM truncation

        for page_result in reader.pages() {
            match page_result {
                Ok(_) => {},
                Err(e) => assert!(e.is_io_error()),  // expected for pages 50-99
            }
        }
    }

    // Corrupt item pointer pointing past page boundary
    #[test]
    fn corrupt_item_pointer_oob() {
        let mut page = read_real_page("base/16384/16385", 0);
        page[24] = 0x0F; page[25] = 0x27;  // lp_off = 9999 (past 8192)
        let result = HeapPage::parse(&page);
        if let Ok(p) = result {
            for tuple in p.tuples() {
                assert!(tuple.is_err());  // must catch OOB
            }
        }
    }

    // CLOG file missing
    #[test]
    fn clog_file_missing() {
        let cluster = PgCluster::open(&data_dir)?;
        std::fs::remove_file(data_dir.join("pg_xact/0000"))?;
        let result = cluster.clog().transaction_status(42);
        assert!(result.is_err());  // not panic, not "committed"
    }

    // Visibility map disagrees with page contents
    #[test]
    fn vm_says_frozen_but_page_has_unfrozen() {
        // VM optimization must never produce wrong results —
        // if VM is wrong, results must still be correct (just slower)
        let result = scan_with_vm_override(page_5_frozen: true);
        let result_no_vm = scan_without_vm(page_5);
        assert_eq!(result, result_no_vm);
    }
}
```

### Concurrency / Stress Testing

pg_arrow reads files that PostgreSQL is actively writing. These tests verify correctness
under concurrent access.

```rust
#[cfg(test)]
mod stress_tests {
    use std::thread;
    use std::time::Duration;

    // Concurrent read+write: PostgreSQL inserts while pg_arrow scans
    #[test]
    fn concurrent_read_write_stress() {
        let pg_conn = connect_pg();
        let arrow_cluster = PgCluster::open(&data_dir)?;

        let writer = thread::spawn(move || {
            for i in 0..10_000 {
                pg_conn.execute(
                    "INSERT INTO stress_test (val) VALUES ($1)", &[&i]
                ).unwrap();
                if i % 100 == 0 {
                    pg_conn.execute("CHECKPOINT", &[]).unwrap();
                }
            }
        });

        let reader = thread::spawn(move || {
            for _ in 0..100 {
                let table = arrow_cluster.table(db_oid, table_oid).unwrap();
                let result = table.scan(ScanOptions::default());
                assert!(result.is_ok());  // must not panic
                thread::sleep(Duration::from_millis(50));
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();
    }

    // VACUUM FULL during scan (replaces heap file via relfilenode change)
    #[test]
    fn vacuum_full_during_scan() {
        let scan = arrow_cluster.table(db_oid, oid)?.scan(slow_options);
        pg_conn.execute("VACUUM FULL stress_test", &[]).unwrap();

        // Must either complete with old data or error cleanly
        // Must NOT mix old and new file data
        let result: Result<Vec<_>> = scan.collect();
        // Either all-ok or clean error, never partial corruption
    }

    // 100 concurrent queries
    #[test]
    fn concurrent_queries_100() {
        let handles: Vec<_> = (0..100).map(|i| {
            thread::spawn(move || {
                let conn = connect_pg_arrow();
                conn.query(
                    &format!("SELECT COUNT(*) FROM hits WHERE CounterID = {}", i),
                    &[],
                )
            })
        }).collect();

        for h in handles {
            assert!(h.join().unwrap().is_ok());
        }
    }
}
```

### Memory Safety: Miri and Sanitizers

pg_arrow does raw byte manipulation and potentially `unsafe` for mmap/FFI. Memory safety
tools catch subtle bugs that normal tests miss.

```bash
# Miri — detects undefined behavior in safe + unsafe Rust (slow but thorough)
cargo +nightly miri test -- --test-threads=1

# AddressSanitizer — buffer overflows, use-after-free
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test

# MemorySanitizer — uninitialized memory reads
RUSTFLAGS="-Zsanitizer=memory" cargo +nightly test

# ThreadSanitizer — data races in concurrent code
RUSTFLAGS="-Zsanitizer=thread" cargo +nightly test
```

CI integration:

```yaml
# .github/workflows/ci.yml
jobs:
  miri:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: rustup toolchain install nightly --component miri
      - run: cargo +nightly miri test -p pg_arrow_core
        # Only core parsing — miri can't do I/O or FFI

  sanitizers:
    strategy:
      matrix:
        sanitizer: [address, memory, thread]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: |
          RUSTFLAGS="-Zsanitizer=${{ matrix.sanitizer }}" \
          cargo +nightly test -p pg_arrow_core --target x86_64-unknown-linux-gnu
```

### Snapshot Testing

Capture known-good outputs and detect unexpected regressions in output format.

**Tooling**: `insta` crate.

```rust
use insta::assert_snapshot;

#[test]
fn snapshot_page_header_display() {
    let page = read_real_page("base/16384/16385", 0);
    let header = PageHeader::parse(&page).unwrap();
    assert_snapshot!(format!("{:#?}", header));
    // Saved in snapshots/test__snapshot_page_header_display.snap
    // Any change to output format = test failure requiring `cargo insta review`
}

#[test]
fn snapshot_arrow_schema_users_table() {
    let table = open_test_table("users");
    assert_snapshot!(format!("{}", table.arrow_schema()));
}

#[test]
fn snapshot_query_plan() {
    let plan = datafusion_explain("SELECT COUNT(*) FROM users WHERE age > 30");
    assert_snapshot!(plan);
}
```

### Mutation Testing

Tests that never fail are useless. Mutation testing checks whether your tests actually
catch bugs by mutating your code and verifying tests fail.

```bash
cargo install cargo-mutants
cargo mutants -p pg_arrow_core

# Output:
# 142 mutants tested
# 128 caught (tests failed — good)
# 10 missed (tests passed with bug — BAD, need more tests)
# 4 timeout
```

Focus on **missed mutants** — these are real logic paths that your tests don't actually verify.

### Cross-Version Compatibility Testing

pg_arrow must work across all supported PostgreSQL versions. Page format, CLOG layout, and
catalog structure can differ between versions.

```rust
#[test_case("pg17")]
#[test_case("pg18")]
#[test_case("latest")]
fn test_full_scan_version(version: &str) {
    let data_dir = test_data_dir(version);
    if !data_dir.exists() {
        eprintln!("Skipping {} — not installed", version);
        return;
    }
    let cluster = PgCluster::open(&data_dir).unwrap();
    let table = cluster.table(db_oid, table_oid).unwrap();
    let batches: Vec<_> = table.scan(ScanOptions::default())
        .unwrap()
        .collect::<Result<Vec<_>>>()
        .unwrap();
    assert!(!batches.is_empty());
}
```

```bash
# CI: setup all versions and test
./scripts/setup-postgres.sh -b pg17 -B -i -t -s
./scripts/setup-postgres.sh -b pg18 -B -i -t -s
./scripts/setup-postgres.sh -b latest -B -i -t -s
cargo test
```

### Code Coverage

```bash
cargo install cargo-tarpaulin
cargo tarpaulin --out Html --output-dir coverage/
```

**Coverage targets:**

| Module | Target | Rationale |
|---|---|---|
| Page parsing (`pg_arrow_core::page`) | 95%+ | Binary format parsing — every branch matters |
| MVCC visibility (`pg_arrow_core::mvcc`) | 95%+ | Correctness-critical, subtle edge cases |
| TOAST detoasting (`pg_arrow_core::toast`) | 90%+ | Decompression + chunk reassembly |
| Type decoding (`pg_arrow_core::types`) | 90%+ | Every type code path |
| Wire protocol (`pg_arrow::protocol`) | 85%+ | Client-facing, many message types |
| DataFusion integration (`pg_arrow_datafusion`) | 80%+ | Integration glue |
| Overall | 80%+ | Minimum for a binary parser |

### Testing Matrix Summary

| Test Type | Tool | What It Catches | When to Run |
|---|---|---|---|
| Unit | `cargo test` | Logic bugs in parsing | Every commit |
| Differential | Custom harness vs PG | Wrong query results | Every commit |
| Fuzz | `cargo fuzz` | Panics/crashes on malformed input | CI (5 min) + nightly (hours) |
| Property | `proptest` | Invariant violations, edge cases | Every commit |
| Chaos | Custom harness | Bad state handling (torn pages, missing files) | Every commit |
| Stress | Concurrent threads | Race conditions, data corruption under load | Nightly |
| Miri | `cargo miri` | Undefined behavior in unsafe code | Nightly |
| Sanitizers | ASan/MSan/TSan | Memory bugs, data races | Nightly |
| Snapshot | `insta` | Unexpected output changes | Every commit |
| Mutation | `cargo-mutants` | Tests that don't actually test anything | Weekly |
| Cross-version | Multi-PG setup | Version-specific regressions | Every commit |
| Coverage | `tarpaulin` | Untested code paths | Weekly |
| ClickBench | Benchmark harness | Performance regressions | Pre-release |
| TPC-H | Benchmark harness | Join performance regressions, multi-table correctness | Pre-release |
| CH-benCHmark | BenchBase/go-tpc | HTAP throughput, freshness lag regressions | Pre-release |

## ClickBench Benchmarking

[ClickBench](https://benchmark.clickhouse.com/) is a single-table analytical benchmark with
43 queries against ~100M rows of real web analytics data (Yandex.Metrica `hits` table, ~100
columns). It's ideal for pg_arrow because it tests exactly the analytical query patterns we
optimize for.

### Setup

```bash
# 1. Download ClickBench data (TSV, ~15GB compressed)
wget https://datasets.clickhouse.com/hits_compatible/hits.tsv.gz

# 2. Create PostgreSQL table (schema from ClickBench repo)
wget https://raw.githubusercontent.com/ClickHouse/ClickBench/main/postgresql/create.sql
psql -d clickbench -f create.sql

# 3. Load data into PostgreSQL (~30-60 min, creates ~70GB heap files)
gunzip -c hits.tsv.gz | psql -d clickbench \
    -c "COPY hits FROM STDIN WITH (FORMAT text)"

# 4. VACUUM FREEZE to maximize all-frozen pages (critical for pg_arrow perf)
psql -d clickbench -c "VACUUM FREEZE hits"

# 5. Get the 43 benchmark queries
wget https://raw.githubusercontent.com/ClickHouse/ClickBench/main/postgresql/queries.sql
```

### Benchmark Harness

```rust
// benches/clickbench.rs
use std::time::Instant;

struct BenchResult {
    query_id: usize,
    query: String,
    pg_direct_ms: f64,      // PostgreSQL via libpq
    pg_arrow_ms: f64,       // pg_arrow + DataFusion
    speedup: f64,           // pg_direct / pg_arrow
    results_match: bool,    // correctness check (critical!)
}

fn run_clickbench() -> Vec<BenchResult> {
    let queries = parse_clickbench_queries("queries.sql");
    let mut results = Vec::new();

    for (i, query) in queries.iter().enumerate() {
        // Warm-up run (discard)
        let _ = pg_conn.query(query, &[]);
        let _ = arrow_conn.query(query, &[]);

        // 3 runs each, take median
        let pg_times: Vec<f64> = (0..3).map(|_| {
            let start = Instant::now();
            pg_conn.query(query, &[]).unwrap();
            start.elapsed().as_secs_f64() * 1000.0
        }).collect();

        let arrow_times: Vec<f64> = (0..3).map(|_| {
            let start = Instant::now();
            arrow_conn.query(query, &[]).unwrap();
            start.elapsed().as_secs_f64() * 1000.0
        }).collect();

        results.push(BenchResult {
            query_id: i + 1,
            query: query.clone(),
            pg_direct_ms: median(&pg_times),
            pg_arrow_ms: median(&arrow_times),
            speedup: median(&pg_times) / median(&arrow_times),
            results_match: diff_query_results(query, &pg_conn, &arrow_conn),
        });
    }
    results
}
```

### Benchmark Script

```bash
#!/bin/bash
# scripts/clickbench.sh — Run ClickBench against pg_arrow and PostgreSQL
QUERIES_FILE="clickbench/queries.sql"
RESULTS_FILE="clickbench/results.json"
PG_CONN="postgresql://localhost:5432/clickbench"
ARROW_CONN="postgresql://localhost:5433/clickbench"  # pg_arrow port
RUNS=3

echo "[]" > "$RESULTS_FILE"
query_num=0

while IFS= read -r query; do
    [ -z "$query" ] && continue
    query_num=$((query_num + 1))
    echo "Q${query_num}: ${query:0:80}..."

    # PostgreSQL direct
    pg_times=()
    for run in $(seq 1 $RUNS); do
        t=$(psql "$PG_CONN" -c "\\timing on" -c "$query" 2>&1 | grep "Time:" | awk '{print $2}')
        pg_times+=("$t")
    done

    # pg_arrow
    arrow_times=()
    for run in $(seq 1 $RUNS); do
        t=$(psql "$ARROW_CONN" -c "\\timing on" -c "$query" 2>&1 | grep "Time:" | awk '{print $2}')
        arrow_times+=("$t")
    done

    # Record
    jq --arg qn "$query_num" --arg q "$query" \
       --argjson pt "$(printf '%s\n' "${pg_times[@]}" | jq -s .)" \
       --argjson at "$(printf '%s\n' "${arrow_times[@]}" | jq -s .)" \
       '. += [{"query": ($qn|tonumber), "sql": $q, "pg_ms": $pt, "arrow_ms": $at}]' \
       "$RESULTS_FILE" > tmp.json && mv tmp.json "$RESULTS_FILE"
done < "$QUERIES_FILE"

echo "Results written to $RESULTS_FILE"
```

### What ClickBench Measures

| Query Pattern | ClickBench Queries | What It Tests in pg_arrow |
|---|---|---|
| Full scan + COUNT | Q0-Q3 | Raw page reading throughput, Arrow conversion speed |
| Filter + COUNT | Q4-Q12 | Predicate pushdown, late materialization |
| GROUP BY + aggregate | Q13-Q25 | DataFusion vectorized hash aggregation |
| GROUP BY + ORDER BY + LIMIT | Q26-Q35 | Top-N, sort performance |
| High-cardinality GROUP BY | Q36-Q40 | Memory pressure, dictionary encoding |
| Multiple columns | Q41-Q42 | Wide projection, column pruning |

### Expected Performance Profile

```
┌─────────────────────┬──────────┬───────────────────────────────────────┐
│ Query Type          │ Expected │ Why                                   │
├─────────────────────┼──────────┼───────────────────────────────────────┤
│ Full scan COUNT(*)  │ 2-5x     │ Arrow columnar + VM skip vs PG       │
│                     │ faster   │ row-by-row heap scan                  │
│ Filtered scan       │ 3-10x   │ Late materialization + SIMD filtering │
│                     │ faster   │ vs PG tuple deforming every column    │
│ GROUP BY agg        │ 2-8x    │ DataFusion vectorized hash agg vs     │
│                     │ faster   │ PG tuple-at-a-time                    │
│ High-cardinality    │ 1-3x    │ Both memory-bound, less advantage     │
│ GROUP BY            │ faster   │                                       │
│ String-heavy        │ 1-2x    │ TOAST overhead may negate gains       │
│                     │ faster   │ if many values are TOASTed            │
│ First run (cold)    │ 0.5-1x  │ Page parsing + Arrow conversion       │
│                     │ slower!  │ overhead, no cache yet                │
│ Subsequent (warm)   │ 3-10x   │ Arrow page cache skips re-parsing     │
│                     │ faster   │                                       │
└─────────────────────┴──────────┴───────────────────────────────────────┘
```

**Important**: Cold first-run on some queries may be **slower** than PostgreSQL because we have
parsing + Arrow conversion overhead without a shared buffer pool. The incremental Arrow page
cache is what makes subsequent runs fast. This is critical to track separately.

### Comparison Targets

For context, include these systems in benchmark reports:

| System | Role | Source |
|---|---|---|
| PostgreSQL 17 (direct) | Baseline — what pg_arrow must beat | Local |
| pg_arrow + DataFusion (cold) | Our worst case — no cache | Local |
| pg_arrow + DataFusion (warm) | Our target case — cached pages | Local |
| DuckDB (from Parquet) | Analytics engine reference | ClickBench public results |
| ClickHouse | Specialized OLAP reference | ClickBench public results |

## TPC-H Benchmarking

[TPC-H](http://www.tpc.org/tpch/) is an 8-table decision-support benchmark with 22 join-heavy
analytical queries. It complements ClickBench — where ClickBench tests single-table scan
throughput (wide table, simple filters, aggregations), TPC-H tests **multi-table join
performance** across a normalized snowflake schema. This is critical for pg_arrow because most
real-world analytical queries join multiple tables, and each table is a separate heap file that
pg_arrow must read, convert to Arrow, and feed into DataFusion's join operators. TPC-H becomes
relevant at **Phase 4+** (after basic query execution, joins, and partitioning support).

### Schema Overview

TPC-H uses a snowflake schema centered on LINEITEM and ORDERS:

```
                              ┌──────────┐
                              │  REGION  │
                              │  5 rows  │
                              └────┬─────┘
                                   │ r_regionkey
                              ┌────┴─────┐
                              │  NATION  │
                              │  25 rows │
                              └──┬───┬───┘
                  n_nationkey ┌──┘   └──┐ n_nationkey
                         ┌────┴────┐ ┌──┴──────┐
                         │ SUPPLIER│ │ CUSTOMER│
                         │ 10K(SF1)│ │ 150K    │
                         └────┬────┘ └────┬────┘
                   s_suppkey  │    c_custkey│
              ┌───────────────┤            │
              │          ┌────┴────┐  ┌────┴────┐
              │          │ PARTSUPP│  │  ORDERS │
              │          │  800K   │  │  1.5M   │
              │          └────┬────┘  └────┬────┘
              │     ps_partkey│   o_orderkey│
         ┌────┴────┐         │        ┌────┴─────┐
         │  PART   │         │        │ LINEITEM │
         │  200K   │         │        │  6M(SF1) │
         └─────────┘         │        └──────────┘
                             │             │
                             └─────────────┘
                           l_partkey, l_suppkey
```

| Table | Rows (SF1) | Rows (SF10) | Rows (SF100) | Role |
|---|---|---|---|---|
| LINEITEM | 6,001,215 | 59,986,052 | 600,037,902 | Fact table (~75% of total data) |
| ORDERS | 1,500,000 | 15,000,000 | 150,000,000 | Order headers |
| PARTSUPP | 800,000 | 8,000,000 | 80,000,000 | Part-supplier junction |
| CUSTOMER | 150,000 | 1,500,000 | 15,000,000 | Customer dimension |
| PART | 200,000 | 2,000,000 | 20,000,000 | Part dimension |
| SUPPLIER | 10,000 | 100,000 | 1,000,000 | Supplier dimension |
| NATION | 25 | 25 | 25 | Nation reference |
| REGION | 5 | 5 | 5 | Region reference |

At SF10 (~10GB raw data, ~25GB in PostgreSQL heap files), LINEITEM alone is ~18GB — large
enough to stress pg_arrow's page reading pipeline while fitting in memory for warm-cache tests.

### Setup

```bash
# === Option A: tpchgen-rs (recommended — pure Rust, 20x faster than dbgen) ===
# Built by the DataFusion community, outputs CSV/Parquet directly
cargo install tpchgen-rs
tpchgen -s 10 --format csv --output tpch-data/
# Generates: customer.csv, lineitem.csv, nation.csv, orders.csv,
#            part.csv, partsupp.csv, region.csv, supplier.csv

# === Option B: DuckDB tpch extension (quick, good for validation) ===
duckdb -c "INSTALL tpch; LOAD tpch; CALL dbgen(sf=10);"
duckdb -c "COPY lineitem TO 'tpch-data/lineitem.csv' (FORMAT CSV);"
# ... repeat for other tables

# === Option C: Classic dbgen (original TPC-H tool) ===
git clone https://github.com/electrum/tpch-dbgen.git && cd tpch-dbgen
make && ./dbgen -s 10

# --- Load into PostgreSQL ---
# 1. Create database and schema
createdb tpch
psql -d tpch -f tpch-schema.sql   # DDL from TPC-H spec (CREATE TABLE statements)

# 2. Load each table
for table in region nation part supplier partsupp customer orders lineitem; do
    psql -d tpch -c "\\COPY $table FROM 'tpch-data/${table}.csv' WITH (FORMAT csv, DELIMITER '|')"
done

# 3. Create indexes (for PostgreSQL baseline comparison)
psql -d tpch -f tpch-indexes.sql  # Primary keys + foreign keys from spec

# 4. VACUUM FREEZE — critical for pg_arrow all-frozen page optimization
psql -d tpch -c "VACUUM (FREEZE, ANALYZE)"

# 5. Optional: Partition LINEITEM by l_shipdate (7 yearly partitions, 1992-1998)
#    This tests pg_arrow's partition pruning on date-range queries
psql -d tpch <<'SQL'
CREATE TABLE lineitem_partitioned (LIKE lineitem INCLUDING ALL)
    PARTITION BY RANGE (l_shipdate);
CREATE TABLE lineitem_y1992 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1992-01-01') TO ('1993-01-01');
CREATE TABLE lineitem_y1993 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1993-01-01') TO ('1994-01-01');
CREATE TABLE lineitem_y1994 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1994-01-01') TO ('1995-01-01');
CREATE TABLE lineitem_y1995 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1995-01-01') TO ('1996-01-01');
CREATE TABLE lineitem_y1996 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1996-01-01') TO ('1997-01-01');
CREATE TABLE lineitem_y1997 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1997-01-01') TO ('1998-01-01');
CREATE TABLE lineitem_y1998 PARTITION OF lineitem_partitioned
    FOR VALUES FROM ('1998-01-01') TO ('1999-01-01');
INSERT INTO lineitem_partitioned SELECT * FROM lineitem;
VACUUM (FREEZE, ANALYZE) lineitem_partitioned;
SQL
```

### Query Classification

All 22 TPC-H queries, classified by SQL features and what they stress in pg_arrow:

| Q# | Name | Tables Joined | Key SQL Features | pg_arrow Stress Point |
|---|---|---|---|---|
| Q1 | Pricing Summary | 1 (LINEITEM) | Filtered agg, GROUP BY | LINEITEM scan throughput, date filter pushdown |
| Q2 | Minimum Cost Supplier | 5 | Correlated subquery, min agg | Multi-table join, subquery planning |
| Q3 | Shipping Priority | 3 | 3-way join, date filter, top-N | Join ordering, date pushdown, LIMIT |
| Q4 | Order Priority Checking | 2 | EXISTS subquery, date range | Semi-join optimization |
| Q5 | Local Supplier Volume | 6 | 6-way join, date filter | Join pipeline depth, filter pushdown |
| Q6 | Forecasting Revenue | 1 (LINEITEM) | Range predicates, agg | Pure scan + filter — best-case for pg_arrow |
| Q7 | Volume Shipping | 6 | CASE, 2 nation filters | Cross-join with selective filters |
| Q8 | National Market Share | 8 (all!) | 8-way join, CASE agg | Maximum join complexity, all heap files active |
| Q9 | Product Type Profit | 6 | LIKE, multi-agg | String predicate pushdown, TOAST potential |
| Q10 | Returned Item Reporting | 4 | Date range, top-N | Join + sort + limit pipeline |
| Q11 | Important Stock ID | 3 | HAVING, subquery for threshold | Aggregation with threshold filter |
| Q12 | Shipping Modes | 2 | IN list, CASE, date range | Predicate variety, CASE in aggregation |
| Q13 | Customer Distribution | 2 | LEFT OUTER JOIN, subquery | Outer join handling |
| Q14 | Promotion Effect | 2 | CASE in agg, date filter | Conditional aggregation |
| Q15 | Top Supplier | 3 | View/CTE, max subquery | CTE materialization |
| Q16 | Parts/Supplier Relationship | 3 | NOT IN, NOT LIKE, DISTINCT | Anti-join, string filtering |
| Q17 | Small-Quantity Order | 3 | Correlated subquery, avg | Correlated subquery execution |
| Q18 | Large Volume Customer | 3 | HAVING on subquery, top-N | Subquery in HAVING, large result sort |
| Q19 | Discounted Revenue | 3 | OR of AND predicates | Complex predicate composition |
| Q20 | Potential Part Promotion | 4 | Nested subqueries, IN | Multi-level subquery |
| Q21 | Suppliers Who Kept Orders | 4 | EXISTS + NOT EXISTS | Anti-semi-join combination |
| Q22 | Global Sales Opportunity | 2 | Substring, NOT EXISTS, avg | String functions, anti-join |

### What TPC-H Stresses in pg_arrow

**Cross-table joins (2-8 tables per query).** Every query except Q1 and Q6 joins multiple
tables. Q8 joins all 8 tables. Each table is a separate PostgreSQL heap file → separate
`TableProvider` in DataFusion. This tests pg_arrow's ability to maintain multiple concurrent
page readers and feed Arrow batches into DataFusion's hash join / merge join operators.

**LINEITEM scan throughput.** LINEITEM is ~75% of total data. At SF10, that's ~60M rows across
~18GB of heap pages. Q1 and Q6 are pure LINEITEM scans — they isolate pg_arrow's raw page
reading and Arrow conversion speed without join overhead.

**Date range filter pushdown.** 15 of 22 queries filter on `l_shipdate` or `o_orderdate`. These
are fixed-width `date` columns (4 bytes, no TOAST) — ideal candidates for predicate pushdown
into the page reader. With partitioned LINEITEM, this also tests partition pruning (skip entire
partition heap files when the date range doesn't overlap).

**TOAST on comment columns at high scale factors.** LINEITEM has `l_comment` (VARCHAR(44)),
ORDERS has `o_comment` (VARCHAR(79)), CUSTOMER has `c_comment` (VARCHAR(117)). At SF1 these
are inline, but at SF100+ longer comments may trigger TOAST. This validates that pg_arrow's
TOAST detoasting works correctly under join workloads.

**Cross-table MVCC snapshot consistency.** When reading 8 separate heap files for Q8, all must
reflect the same transaction snapshot. A row visible in LINEITEM but invisible in ORDERS (due
to snapshot mismatch) would produce wrong join results — a correctness-critical test.

**Partition pruning on date-partitioned LINEITEM.** With yearly partitions (1992-1998), a query
filtering `l_shipdate BETWEEN '1995-01-01' AND '1996-12-31'` should only read 2 of 7 partition
heap files. This tests pg_arrow's integration with DataFusion's partition pruning logic.

### Benchmark Harness

```rust
// benches/tpch.rs
use std::time::Instant;

struct TpchResult {
    query_id: usize,       // 1-22
    query_name: String,
    tables_joined: usize,
    pg_direct_ms: f64,
    pg_arrow_cold_ms: f64, // First run, no Arrow cache
    pg_arrow_warm_ms: f64, // Subsequent run, cached Arrow batches
    speedup_cold: f64,
    speedup_warm: f64,
    results_match: bool,   // Row-by-row diff vs PostgreSQL
}

fn run_tpch_benchmark(sf: u32) -> Vec<TpchResult> {
    let queries = load_tpch_queries("tpch/queries/"); // Q1.sql .. Q22.sql
    let mut results = Vec::new();

    for (i, query) in queries.iter().enumerate() {
        // Clear pg_arrow cache for cold measurement
        arrow_conn.query("SELECT pg_arrow_drop_cache()", &[]).ok();

        // Cold run (single)
        let cold_start = Instant::now();
        arrow_conn.query(query, &[]).unwrap();
        let cold_ms = cold_start.elapsed().as_secs_f64() * 1000.0;

        // Warm runs (3x median)
        let warm_times: Vec<f64> = (0..3).map(|_| {
            let start = Instant::now();
            arrow_conn.query(query, &[]).unwrap();
            start.elapsed().as_secs_f64() * 1000.0
        }).collect();

        // PostgreSQL baseline (3x median, already cached in shared buffers)
        let pg_times: Vec<f64> = (0..3).map(|_| {
            let start = Instant::now();
            pg_conn.query(query, &[]).unwrap();
            start.elapsed().as_secs_f64() * 1000.0
        }).collect();

        let pg_ms = median(&pg_times);
        results.push(TpchResult {
            query_id: i + 1,
            query_name: tpch_query_name(i + 1),
            tables_joined: tpch_table_count(i + 1),
            pg_direct_ms: pg_ms,
            pg_arrow_cold_ms: cold_ms,
            pg_arrow_warm_ms: median(&warm_times),
            speedup_cold: pg_ms / cold_ms,
            speedup_warm: pg_ms / median(&warm_times),
            results_match: diff_query_results(query, &pg_conn, &arrow_conn),
        });
    }
    results
}
```

### Expected Performance Profile

```
┌─────────────────────────┬──────────┬──────────┬──────────────────────────────────────┐
│ Query Category          │ Cold     │ Warm     │ Why                                  │
├─────────────────────────┼──────────┼──────────┼──────────────────────────────────────┤
│ Single-table scan       │ 1-2x    │ 3-8x    │ Q1/Q6: LINEITEM only — same as       │
│ (Q1, Q6)               │ faster   │ faster   │ ClickBench pattern                   │
│ 2-3 table join          │ 0.5-1.5x│ 2-5x    │ Q3/Q4/Q10/Q12: join overhead vs      │
│ (Q3, Q4, Q10, Q12)     │          │ faster   │ PG's optimized nested loop           │
│ 4-6 table join          │ 0.5-1x  │ 1.5-4x  │ Q5/Q7/Q9: DataFusion hash joins      │
│ (Q5, Q7, Q9)           │ even     │ faster   │ competitive but more tables to parse  │
│ 8-table join (Q8)       │ 0.5-1x  │ 1.5-3x  │ Maximum join depth — build side       │
│                         │ even     │ faster   │ fits memory, probe is LINEITEM scan  │
│ Subquery-heavy          │ 0.5-1x  │ 1-3x    │ Q2/Q17/Q20/Q21: DataFusion subquery  │
│ (Q2, Q17, Q20, Q21)    │ even     │ faster   │ decorrelation quality matters         │
│ With partitioned        │ 2-4x    │ 5-15x   │ Partition pruning skips entire heap   │
│ LINEITEM (date filter)  │ faster   │ faster   │ files — massive I/O reduction         │
└─────────────────────────┴──────────┴──────────┴──────────────────────────────────────┘
```

**Key insight**: TPC-H cold performance will be closer to PostgreSQL than ClickBench because
join overhead dominates scan time. The real advantage shows on warm runs (cached Arrow batches)
and with partitioned LINEITEM (partition pruning eliminates heap file I/O entirely).

### Comparison Targets

| System | Role | Source |
|---|---|---|
| PostgreSQL 17 (direct) | Baseline with indexes + shared buffers | Local |
| pg_arrow + DataFusion (cold) | Worst case — all tables parsed from heap | Local |
| pg_arrow + DataFusion (warm) | Target case — cached Arrow batches | Local |
| DuckDB (from Parquet) | Analytics engine on pre-converted data | Local / published |
| Hyper (Tableau) | Commercial hybrid engine reference | Published benchmarks |
| DataFusion (standalone, Parquet) | Same engine, native format — ceiling | Local |

## CH-benCHmark (HTAP Benchmarking)

The [CH-benCHmark](https://db.in.tum.de/research/projects/CHbenCHmark/) (Cole et al., DBTest
2011) is the standard benchmark for **hybrid transactional/analytical processing (HTAP)**
systems. It runs TPC-C (OLTP) and TPC-H (OLAP) queries **concurrently** against the same
dataset, measuring both transactional throughput and analytical latency under mixed workloads.
This is the ideal benchmark for pg_arrow's sidecar architecture, where PostgreSQL handles OLTP
writes and pg_arrow simultaneously serves OLAP reads. CH-benCHmark becomes relevant at
**Phase 6+** (after wire protocol, query execution, and basic sidecar deployment work).

### Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        CH-benCHmark Driver                              │
│                    (BenchBase or go-tpc)                                │
├──────────────────────────┬──────────────────────────────────────────────┤
│     OLTP Workers         │            OLAP Workers                      │
│   (TPC-C transactions)   │         (TPC-H queries)                      │
│                          │                                              │
│  NewOrder, Payment,      │  Q1-Q22 (adapted to                         │
│  OrderStatus, Delivery,  │   TPC-C schema names)                       │
│  StockLevel              │                                              │
└────────────┬─────────────┴──────────────┬───────────────────────────────┘
             │ port 5432                  │ port 5433
             ▼                            ▼
┌────────────────────────┐   ┌────────────────────────────────────────────┐
│     PostgreSQL          │   │              pg_arrow                      │
│   (OLTP engine)         │   │         (OLAP engine)                      │
│                         │   │                                            │
│  TPC-C tables:          │   │  Reads same heap files (Mode 1/2)         │
│  WAREHOUSE, DISTRICT,   │──▶│  OR receives logical stream (Mode 3)      │
│  CUSTOMER, OORDER,      │   │                                            │
│  ORDER_LINE, STOCK,     │   │  DataFusion executes TPC-H queries        │
│  ITEM, NEW_ORDER,       │   │  on Arrow-converted data                   │
│  HISTORY                │   │                                            │
└────────────────────────┘   └────────────────────────────────────────────┘
         │                              │
         │  $PGDATA/ (heap files)       │ reads / replicates
         └──────────────────────────────┘
```

### Schema Mapping

CH-benCHmark maps TPC-H tables to TPC-C equivalents:

| TPC-H Table | TPC-C Equivalent | Key Difference |
|---|---|---|
| LINEITEM | ORDER_LINE | ORDER_LINE has fewer columns, different names |
| ORDERS | OORDER | OORDER uses `o_id` + `o_d_id` + `o_w_id` composite key |
| CUSTOMER | CUSTOMER | Compatible — TPC-C CUSTOMER is superset |
| STOCK | PARTSUPP | STOCK is keyed by (warehouse, item) |
| ITEM | PART | ITEM is simpler (no PART attributes) |
| SUPPLIER | — | Synthesized from STOCK data |
| NATION | NATION | Added to TPC-C schema for CH-benCHmark |
| REGION | REGION | Added to TPC-C schema for CH-benCHmark |

### Setup

**Option A: BenchBase (CMU, Java) — most complete CH-benCHmark implementation**

```bash
# 1. Clone and build BenchBase
git clone https://github.com/cmu-db/benchbase.git && cd benchbase
./mvnw clean package -P postgres -DskipTests

# 2. Configure OLTP connection (PostgreSQL) and OLAP connection (pg_arrow)
# Edit config/postgres/sample_chbenchmark_config.xml:
#   <url>jdbc:postgresql://localhost:5432/chbench</url>    <!-- OLTP -->
#   <url>jdbc:postgresql://localhost:5433/chbench</url>    <!-- OLAP -->
#   <terminals>16</terminals>          <!-- OLTP concurrency -->
#   <olap_terminals>4</olap_terminals> <!-- OLAP concurrency -->

# 3. Create database and load TPC-C data
createdb chbench
java -jar benchbase.jar -b chbenchmark -c config/postgres/sample_chbenchmark_config.xml \
     --create=true --load=true

# 4. VACUUM FREEZE for pg_arrow
psql -d chbench -c "VACUUM (FREEZE, ANALYZE)"

# 5. Run mixed workload (OLTP + OLAP concurrent)
java -jar benchbase.jar -b chbenchmark -c config/postgres/sample_chbenchmark_config.xml \
     --execute=true
```

**Option B: go-tpc (PingCAP, Go) — lightweight alternative**

```bash
# 1. Install
go install github.com/pingcap/go-tpc@latest

# 2. Prepare TPC-C data (100 warehouses)
go-tpc tpcc prepare -H localhost -P 5432 -D chbench --warehouses 100

# 3. Run OLTP-only baseline (5 minutes)
go-tpc tpcc run -H localhost -P 5432 -D chbench --warehouses 100 \
    --threads 16 --time 5m

# 4. Run CH-benCHmark (OLTP + OLAP concurrent, 10 minutes)
go-tpc ch run -H localhost -P 5432 -D chbench --warehouses 100 \
    --oltp-threads 16 --olap-addr localhost:5433 --olap-threads 4 --time 10m
```

### Key Metrics

**1. OLTP Throughput (tpmC)**

Does pg_arrow's concurrent heap file reading degrade PostgreSQL's write performance?

| Scenario | Expected tpmC | Impact |
|---|---|---|
| PostgreSQL only (baseline) | X | — |
| PG + pg_arrow Mode 1 (shared primary) | 0.95-1.0x | Minimal — pg_arrow reads don't take locks |
| PG + pg_arrow Mode 2 (promotable replica) | 1.0x | Zero — separate data directory |
| PG + pg_arrow Mode 3 (logical replica) | 0.98-1.0x | Slight — logical decoding overhead |

**2. Analytical QPS (queries/hour)**

How many of the 22 TPC-H-style queries complete per hour under OLTP load?

| System | Expected QpH@SF10 | Notes |
|---|---|---|
| PostgreSQL (OLTP+OLAP same instance) | 50-200 | Lock contention, shared buffers thrashed |
| pg_arrow Mode 1 (sidecar + primary) | 500-2000 | Columnar scan, no lock contention |
| pg_arrow Mode 2 (sidecar + replica) | 500-2000 | Same perf, zero OLTP impact |
| pg_arrow Mode 3 (logical replica) | 1000-4000 | Arrow-native storage, no heap parsing |

**3. Freshness Lag**

Time between a PostgreSQL COMMIT and the data being visible in pg_arrow queries:

| Deployment Mode | Expected Lag | Mechanism |
|---|---|---|
| Mode 1 (sidecar + primary) | 0-10ms | Direct heap read — visible after fsync |
| Mode 2 (sidecar + replica) | 10-100ms | WAL replay delay on standby |
| Mode 3 (logical replica) | 50-500ms | Logical decoding → apply → visible |

### Freshness Measurement

Practical probe approach to measure data freshness lag:

```sql
-- On PostgreSQL (OLTP side): insert marker row with precise timestamp
INSERT INTO freshness_probe (probe_id, inserted_at)
VALUES (gen_random_uuid(), clock_timestamp());

-- On pg_arrow (OLAP side): poll for the marker row
SELECT clock_timestamp() - inserted_at AS freshness_lag
FROM freshness_probe
WHERE probe_id = '<uuid>';
```

Run this probe every second during the benchmark. Report: P50, P95, P99, max freshness lag.

### Expected Performance Profile

```
┌──────────────────────────┬─────────┬──────────┬──────────┬─────────────────┐
│ OLTP Concurrency         │  tpmC   │ OLAP QpH │ P50 Lag  │ P99 Lag         │
├──────────────────────────┼─────────┼──────────┼──────────┼─────────────────┤
│ 0 threads (OLAP only)    │    —    │  2000+   │   0ms    │   0ms           │
│ 8 threads                │  ~8K    │  1500+   │  <10ms   │  <100ms         │
│ 16 threads               │  ~15K   │  1200+   │  <10ms   │  <200ms         │
│ 32 threads               │  ~25K   │  800+    │  <20ms   │  <500ms         │
│ 64 threads (stress)      │  ~35K   │  500+    │  <50ms   │  <1s            │
└──────────────────────────┴─────────┴──────────┴──────────┴─────────────────┘
```

**Key insight**: pg_arrow's value is that OLAP QpH degrades gracefully under OLTP load (not
catastrophically like running TPC-H queries directly on the OLTP PostgreSQL instance), and
OLTP tpmC is minimally affected by concurrent OLAP reads.

### Other HTAP Benchmarks

For completeness, other HTAP benchmarks exist but CH-benCHmark is recommended as the starting
point:

- **HyBench** (VLDB 2024) — Financial-domain HTAP benchmark with realistic data distributions
  and temporal access patterns. More domain-specific than CH-benCHmark but valuable for
  demonstrating pg_arrow in financial analytics use cases.
- **TPC-DS** — 99 queries, 24 tables, more complex schema than TPC-H. A future goal after
  TPC-H correctness is validated. Stresses subquery decorrelation and complex joins more
  heavily than TPC-H.
- **HTAPBench** (Coelho et al., 2017) — Earlier HTAP benchmark, largely superseded by
  CH-benCHmark in academic use.

**Recommendation**: Start with CH-benCHmark (broadest tool support, most cited in HTAP
literature, directly reuses TPC-C/TPC-H infrastructure). Add HyBench or TPC-DS as stretch
goals once TPC-H correctness is proven.

## Limitations

1. ⚠️ **Single machine** (both on same host for true zero-copy)
   - Could extend with NFS/shared storage

2. ⚠️ **No indexes** in pg_arrow (full table scans)
   - DataFusion is fast enough for analytics
   - OLTP should use PostgreSQL (has indexes)

3. ⚠️ **MVCC visibility is the hardest part** (~500-800 lines of real implementation)
   - Requires CLOG reader (`pg_xact/`), MultiXact resolution, subtransaction handling
   - Cannot write hint bits back (performance penalty on recent data)
   - Snapshot acquisition needs PostgreSQL connection or shared memory parsing
   - Visibility map (`_vm`) is a critical optimization to mitigate this cost
   - See "MVCC Visibility: The Real Complexity" section for full analysis

4. ⚠️ **TOAST is mandatory for real-world tables**
   - Any table with text/JSONB columns will have TOASTed values
   - Requires reading separate TOAST heap files and decompressing (pglz/lz4)
   - Without TOAST support, pg_arrow returns garbage for large column values
   - See "Physical Storage Features → TOAST" section

5. ⚠️ **Wire protocol must support Extended Query**
   - Simple Query Protocol only supports `psql`
   - Real client libraries (JDBC, psycopg2, asyncpg) require Parse/Bind/Describe/Execute
   - Catalog queries (`pg_class`, `pg_type`, `information_schema`) needed for ORMs and tools
   - See "PostgreSQL Protocol Compatibility" section

6. ⚠️ **Security: pg_arrow bypasses ALL PostgreSQL permissions**
   - Table-level GRANT/REVOKE, column-level permissions, Row-Level Security all bypassed
   - Must be deployed as a trusted internal service (Phase 1) or implement permission checking (Phase 8)
   - See "Security Model" section for full analysis and options

7. ⚠️ **Segment files required for tables >1GB**
   - Tables are split into 1GB segments (`oid`, `oid.1`, `oid.2`, ...)
   - Without segment support, pg_arrow silently reads only the first 1GB of large tables

8. ⚠️ **pg_control validation required on startup**
   - Block size, segment size, data checksums, PG version, cluster state must be verified
   - Wrong block size = every page read is garbage
   - See "Configuration and Cluster Validation" section

9. ⚠️ **Database encoding must be handled**
   - Non-UTF8 databases (LATIN1, EUC_JP, SQL_ASCII) require transcoding to Arrow's UTF-8
   - Without transcoding, text data is garbled

10. ⚠️ **No sharding** — serves a single PostgreSQL instance
    - PostgreSQL itself has no native sharding
    - Multi-instance coordination is a separate future project

11. ⚠️ **Collation-dependent sort order**
    - `ORDER BY` on text columns only correct for `C` / `POSIX` collations in Phase 1
    - Locale-aware collations (`en_US.UTF-8`, ICU) produce different sort order than DataFusion's byte comparison
    - See "Collation Handling" section

12. ⚠️ **Schema evolution (ALTER TABLE)**
    - Old tuples on disk may have fewer columns than current schema (after `ADD COLUMN`)
    - Dropped columns leave holes in tuple layout (`attisdropped = true`)
    - Must check `tuple natts` vs `schema natts` and fill missing columns with defaults
    - See "Schema Evolution" section

13. ⚠️ **Cannot be promoted to primary**
    - pg_arrow is read-only — promotion would require reimplementing ~70% of PostgreSQL
    - Use tiered standby topology: PostgreSQL standby for HA, pg_arrow standby for analytics
    - See "Replica Deployment" section

14. ⚠️ **Extension types stored as opaque binary**
    - PostGIS geometry, pgvector embeddings, hstore etc. stored as Arrow Binary by default
    - Consumers must interpret the raw bytes; DataFusion cannot filter/sort on opaque binary
    - Known extensions (pgvector, hstore) get typed Arrow representations in Phase 7+
    - See "Extension Type Handling" section

15. ⚠️ **Shared buffer lag requires WAL replay for full consistency (Modes 1 & 2)**
    - Heap file pages on disk lag behind PostgreSQL's shared buffers — committed data may not be on disk
    - Different pages are at different LSNs (cross-page inconsistency)
    - Full consistency requires replaying WAL records onto stale pages to bring them to a common target_lsn
    - WAL parser is ~2000-3000 lines of version-specific code (PG WAL format is not a stable API)
    - Phase 1-2 can start with Tier 1 (MVCC-only, no WAL replay) — correct for data on disk but misses very recently committed unflushed rows
    - Mode 2 (replica with paused replay) sidesteps WAL replay entirely — all pages consistent at replay LSN
    - See "Read Consistency for Direct Heap File Access" section

## Recommended Implementation Plan

### Phase 0: Cluster Validation and Foundation (1-2 weeks)

- [ ] `pg_control` reader: parse binary format, extract block_size, segment_size, checksums, PG version, cluster state
- [ ] Startup validation: verify `state == DB_IN_PRODUCTION` or `DB_IN_ARCHIVE_RECOVERY` (replica), supported PG version, block_size
- [ ] PostgreSQL connection pool: establish connection, read `pg_settings` for encoding, timezone, DateStyle, etc.
- [ ] Database encoding detection: verify UTF-8 or implement transcoding for LATIN1/EUC_JP
- [ ] Segment file iteration: read `oid`, `oid.1`, `oid.2`, ... for multi-segment tables
- [ ] Page checksum verification (if `data_checksum_version > 0`): torn page detection with retry

### Phase 1: pg_arrow_core Library (4-6 weeks)

- [ ] **Crate structure**: Set up workspace with `pg_arrow_core` (engine-agnostic) and `pg_arrow_datafusion`
- [ ] Read PostgreSQL heap files (page iteration, tuple extraction) — segment-aware
- [ ] Parse page headers, item pointers, and tuple headers
- [ ] Decode inline column values for core types (int2/4/8, float4/8, bool, text, bytea, timestamp, date, numeric)
- [ ] **TOAST detoasting**: Read TOAST tables, reassemble chunks, decompress pglz/lz4
- [ ] **Visibility map reader**: Read `_vm` fork, use all-frozen bit to skip visibility checks
- [ ] **Schema evolution**: Handle `ALTER TABLE ADD/DROP COLUMN` — check `tuple natts` vs schema, fill missing columns with `attmissingval` or NULL, skip `attisdropped` columns
- [ ] **Unknown type fallback**: Store unrecognized extension types as Arrow `Binary`
- [ ] Convert to Arrow RecordBatch in-memory
- [ ] Public API: `PgCluster::open()`, `PgTable::scan()` → `Iterator<Result<RecordBatch>>`
- [ ] Basic DataFusion integration in `pg_arrow_datafusion` (TableProvider, CatalogProvider, simple query execution)
- [ ] Projection pushdown: only decode columns requested by the query
- [ ] Multi-page RecordBatch batching (target 8K-64K rows per batch)

### Phase 2: MVCC Consistency (4-6 weeks)

- [ ] **2a - Frozen-only reads** (1 week): Return only `HEAP_XMIN_FROZEN` tuples (always visible, no CLOG needed). Use visibility map to identify all-frozen pages for fast path.
- [ ] **2b - CLOG reader + Tier 1 consistency** (2 weeks): Implement `pg_xact/` reader, hint-bit-aware visibility, snapshot via `pg_current_snapshot()`, batched CLOG lookups (collect unique xids per page, read CLOG page once). This is Tier 1 consistency — correct for data on disk but may miss very recently committed unflushed rows (acceptable for analytics).
- [ ] **2c - Full visibility** (2-3 weeks): MultiXact resolution, subtransaction handling, HOT chain traversal
- [ ] **Isolation levels**: Per-connection snapshot tracking — READ COMMITTED (new snapshot per statement) vs REPEATABLE READ (snapshot held for transaction)
- [ ] **2d - WAL replay for Tier 2 consistency** (shared with Phase 12b): WAL page/record header parser, heap WAL record application (`xl_heap_insert/delete/update`), FPI extraction. Read `pd_lsn` from each page header, replay WAL records from `pd_lsn..target_lsn` to bring stale pages current. Target LSN via `pg_current_wal_flush_lsn()`. This ensures full consistency including unflushed committed data. See "Read Consistency for Direct Heap File Access" section.

### Phase 3: Wire Protocol (3-4 weeks)

- [ ] **3a - Simple Query Protocol** (1 week): Startup, auth (`trust` initially — real auth in Phase 8), Query/RowDescription/DataRow/CommandComplete, ErrorResponse with SQLSTATE codes. `psql` works.
- [ ] **3b - Extended Query Protocol** (2 weeks): Parse/Bind/Describe/Execute/Sync, prepared statements, type OID mapping (Arrow → PG OIDs), text format encoding for all supported types.
- [ ] **3c - Session and catalog** (1 week): `SET`/`SHOW` for core params, `BEGIN`/`COMMIT`/`ROLLBACK` (read-only no-ops), proxy catalog queries (`pg_class`, `pg_type`, `information_schema`) to PostgreSQL. System functions: `version()`, `current_database()`, `current_user`.
- [ ] Connection handling, `DISCARD ALL`/`RESET ALL` for connection pools.

### Phase 4: Partitioning and Parallel Scan (2-3 weeks)

- [ ] Read partition metadata from `pg_inherits`, `pg_partitioned_table`, `pg_class.relpartbound`
- [ ] Partition pruning: compare query filters against partition bounds (RANGE, LIST, HASH)
- [ ] Parallel scan: one DataFusion partition per physical heap file
- [ ] Legacy table inheritance support (pre-PG10 partitioning via `INHERITS`)
- [ ] Tablespace support: resolve `pg_tblspc/` symlinks

### Phase 5: PostgreSQL SQL Compatibility (2-3 weeks)

- [ ] Enable sqlparser-rs PostgreSQL dialect (`::` casts, `ILIKE`, etc.)
- [ ] Register `pg_compat` function library (~30-40 UDFs: `to_char`, `age`, `make_interval`, etc.)
- [ ] Register aggregate UDAFs: `string_agg`, `percentile_cont`, `percentile_disc`, `mode`, `bool_and`/`bool_or`
- [ ] Register `generate_series` as TableFunction
- [ ] Integrate `datafusion-functions-json` for `->` / `->>` operators
- [ ] Implement capability-based query routing with PostgreSQL fallback

### Phase 6: Production Features (3-4 weeks)

- [ ] Schema caching with LSN-based invalidation (schema cache manager background job)
- [ ] Visibility map monitor (background re-read of `_vm` files after autovacuum)
- [ ] Cluster health monitor (background PostgreSQL liveness check, pg_control re-read)
- [ ] pg_arrow statistics collector (query times, page reads, CLOG lookups, TOAST decompressions)
- [ ] Concurrent DDL safety: track `relfilenode` changes, detect DROP/TRUNCATE/VACUUM FULL
- [ ] Query optimization and performance tuning
- [ ] Binary format encoding for high-throughput clients
- [ ] COPY TO protocol for bulk data export
- [ ] SSL/TLS support
- [ ] CancelRequest handling
- [ ] Optional warm-up / pre-fetch for hot tables on startup

### Phase 7: Advanced Features and Optimizations (ongoing)

- [ ] EXPLAIN with DataFusion plan output
- [ ] Unlogged table support (file mtime-based invalidation)
- [ ] View expansion from `pg_rewrite`
- [ ] Additional type support (range types, composite types, enums, domains)
- [ ] Connection pool compatibility testing (PgBouncer, pgpool)
- [ ] **BRIN index reader**: Parse BRIN pages, extract min/max per block range, integrate with DataFusion pruning (see "PostgreSQL Index Reuse" section)
- [ ] **B-tree index reader**: Parse B-tree leaf pages for indexed lookups, range scans (see "PostgreSQL Index Reuse" section)
- [ ] **Self-built zone maps**: Per-page min/max/null_count maintained during scan, persisted across restarts (see "PostgreSQL Index Reuse" section)
- [ ] **`pg_statistic` reader**: Extract ndistinct, most_common_vals, histogram_bounds → feed to DataFusion optimizer (see "DataFusion Engine Integration" section)
- [ ] **Incremental Arrow page cache**: Page-level and column-level caching with LSN-based invalidation (see "Incremental Arrow Page Cache" section)
- [ ] **Persistent Parquet cache**: Spill cold cached pages to Parquet files on disk for fast restart (see "Incremental Arrow Page Cache" section)
- [ ] **WAL monitor**: Parse WAL for precise per-table/per-page cache invalidation (see "Incremental Arrow Page Cache" section)
- [ ] **Late materialization**: Only decode columns after filter evaluation (see "Arrow-Native Optimizations" section)
- [ ] **Dictionary encoding**: Auto-detect low-cardinality columns, use DictionaryArray (see "Arrow-Native Optimizations" section)
- [ ] **I/O optimizations**: mmap for read-only pages, io_uring on Linux, readahead hints (see "I/O Optimizations" section)
- [ ] **Adaptive scan strategy**: DataFusion optimizer rule to choose B-tree vs BRIN vs full scan based on selectivity (see "DataFusion Engine Integration" section)

### Phase 8: Security (2-3 weeks)

- [ ] Proxy authentication to PostgreSQL (forward client credentials for validation)
- [ ] Per-connection role tracking (store authenticated role after auth)
- [ ] Table-level permission checks via `has_table_privilege()` on PostgreSQL connection
- [ ] Column-level permission checks via `has_column_privilege()`
- [ ] Audit logging: all connections, queries, table accesses
- [ ] `pg_hba.conf` parsing for consistent host-based access control (optional)
- [ ] Row-Level Security: read `pg_policy`, translate policies to DataFusion filters (future)

### Phase 9: Arrow Flight SQL and pg_arrow_cli (3-4 weeks)

- [ ] **Arrow Flight SQL server** on separate port (default 5434):
  - `GetFlightInfo` → query planning, return schema + partition endpoints
  - `DoGet` → stream RecordBatches directly from DataFusion execution (zero row conversion)
  - `GetSqlInfo` → advertise capabilities (supported SQL, transaction support, etc.)
  - `GetCatalogs` / `GetSchemas` / `GetTables` → catalog discovery via PostgreSQL proxy
  - TLS support (reuse SSL certificates from Phase 6)
- [ ] **ADBC driver** (`pg_arrow_adbc`):
  - Implement ADBC 1.0 interface wrapping Flight SQL client
  - Python: `adbc_driver_pg_arrow` installable via pip
  - Go/Java: ADBC driver wrappers
- [ ] **pg_arrow_cli** — interactive query tool (psql alternative):
  - Arrow Flight SQL client with columnar display
  - `\timing` — show query execution time
  - `\format` — switch between table/csv/json/parquet output
  - `\export <query> <file.parquet>` — direct Parquet export (zero-copy from Arrow)
  - `\schema <table>` — show Arrow schema with types
  - `\explain <query>` — show DataFusion physical plan
  - Tab completion for table/column names via Flight SQL `GetTables`/`GetColumns`
  - History, multi-line editing (rustyline)

### Phase 10: Ecosystem Integrations (3-5 weeks)

- [ ] **pg_arrow_python** (PyO3 bindings):
  - `pip install pg_arrow` — Python package with native Rust core
  - `PgCluster.open(data_dir)` → `PgTable.scan()` → `pyarrow.RecordBatchReader`
  - Zero-copy via Arrow C Data Interface (PyArrow `import_from_c`)
  - Pandas integration: `table.to_pandas(columns=["id", "name"])`
  - Polars integration: `table.to_polars(columns=["id", "name"])`
  - Jupyter notebook friendly: `_repr_html_` for table previews
- [ ] **pg_arrow_duckdb** (DuckDB extension via C FFI):
  - Arrow C Data Interface: `pg_arrow_core` produces `ArrowArrayStream` → DuckDB consumes natively
  - DuckDB SQL: `SELECT * FROM pg_arrow_scan('/pgdata', 'public.users')`
  - Replacement scan registration for transparent access
  - Pushdown: DuckDB filter/projection → `ScanOptions` → pg_arrow_core
- [ ] **Arrow C Data Interface in pg_arrow_core**:
  - `PgTable::scan_to_ffi()` → `Vec<ArrowArrayFFI>` for any C/C++/Go/Java consumer
  - `ArrowArrayStream` producer for streaming consumers
  - Stable C ABI: `pg_arrow_core_open_cluster()`, `pg_arrow_core_scan_table()` etc.
  - Header file generation for C/C++ consumers (`pg_arrow.h`)

### Phase 11: Testing Infrastructure (ongoing, parallel with all phases)

- [ ] **Fuzz targets** (Phase 1+):
  - `fuzz_page_header`, `fuzz_heap_page`, `fuzz_tuple_decode`, `fuzz_toast_decompress`, `fuzz_wire_protocol`
  - Corpus seeding from real PostgreSQL pages
  - CI: 5-minute fuzz runs per target; Nightly: multi-hour sessions
- [ ] **Property-based tests** (Phase 1+):
  - Round-trip encode/decode for all supported types (proptest)
  - Page header invariants (pd_lower ≤ pd_upper, item pointers within bounds)
  - MVCC visibility invariants (frozen always visible, aborted never visible)
  - Arrow RecordBatch schema always matches declared schema
- [ ] **Differential testing harness** (Phase 1+):
  - `pageinspect`-based page/tuple field comparison
  - Full table scan diffing (pg_arrow vs `SELECT *` from PostgreSQL)
  - SQL query result diffing with normalization (sort, float rounding, NULL handling)
  - PostgreSQL regression suite SELECT extraction and replay
- [ ] **MVCC visibility test scenarios** (Phase 2):
  - Known-txid visibility scenarios (committed, aborted, in-progress, locked, deleted)
  - Snapshot-based differential testing via `pg_current_snapshot()`
- [ ] **Test data generation** (Phase 0):
  - Type coverage tables (every supported PG type)
  - Edge case data (NULLs, min/max values, empty strings, TOASTed values, row locks, aborted txns)
  - Multi-segment tables (>1GB)
- [ ] **Chaos / fault injection** (Phase 1+):
  - Torn pages, zero pages, truncated files, corrupt item pointers
  - Missing CLOG files, VM disagreements
- [ ] **Concurrency / stress tests** (Phase 2+):
  - Concurrent PostgreSQL writes during pg_arrow scan
  - VACUUM FULL during scan (relfilenode change)
  - 100 parallel query stress test
- [ ] **Memory safety** (Phase 1+):
  - Miri on `pg_arrow_core` (nightly CI)
  - AddressSanitizer, MemorySanitizer, ThreadSanitizer (nightly CI)
- [ ] **Snapshot tests** (Phase 1+):
  - `insta` snapshots for page header display, Arrow schema, query plans
- [ ] **Mutation testing** (Phase 1+):
  - `cargo-mutants` on `pg_arrow_core` (weekly CI)
  - Track and fix missed mutants
- [ ] **Cross-version compatibility** (Phase 0+):
  - Test against pg17, pg18, latest (master) via `setup-postgres.sh`
- [ ] **Code coverage** (ongoing):
  - `cargo-tarpaulin` with HTML reports (weekly CI)
  - Targets: 95% page/MVCC/TOAST, 90% types, 85% protocol, 80% overall
- [ ] **ClickBench benchmark** (Phase 4+):
  - Load 100M-row `hits` table into PostgreSQL
  - Run 43 queries: cold (no cache) and warm (cached)
  - Compare against PostgreSQL direct, record speedup ratios
  - Track regressions across releases
- [ ] **TPC-H benchmark** (Phase 4+):
  - Load SF10 into PostgreSQL (partitioned LINEITEM)
  - Run 22 queries: compare pg_arrow vs PostgreSQL baseline
  - Track per-query speedup, correctness validation
- [ ] **CH-benCHmark HTAP** (Phase 6+):
  - BenchBase or go-tpc setup with split OLTP/OLAP connections
  - Measure tpmC, analytical QPS, freshness lag
  - Baseline: OLTP-only vs OLTP+OLAP concurrent
- [ ] **CI pipeline** (``.github/workflows/ci.yml``):
  - Every commit: unit, differential, property, chaos, snapshot, cross-version
  - Nightly: fuzz (extended), stress, Miri, sanitizers
  - Weekly: mutation testing, coverage reports
  - Pre-release: ClickBench, TPC-H, CH-benCHmark full runs

### Phase 12: Deployment Modes and WAL Synchronization (5-8 weeks)

- [ ] **12a — Sidecar mode (Modes 1 & 2)** (1-2 weeks):
  - Sidecar + Primary: pg_arrow reads `$PGDATA/` alongside local PostgreSQL primary
  - Sidecar + Promotable Replica: pg_arrow reads `$PGDATA/` alongside local PostgreSQL standby
  - Accept `DB_IN_ARCHIVE_RECOVERY` in pg_control validation
  - Detect promotion via `pg_is_in_recovery()` flipping → invalidate caches, reacquire schema
  - Recovery LSN sync: use `pg_last_wal_replay_lsn()` as consistency point
  - Optional `pg_wal_replay_pause()` / `pg_wal_replay_resume()` for torn-page-free reads
- [ ] **12b — Physical WAL stream parsing (Modes 1 & 2 cache optimization)** (2-3 weeks):
  - Physical replication slot: `CREATE_REPLICATION_SLOT pg_arrow_cache PHYSICAL`
  - WAL record parser: heap_insert, heap_update, heap_delete, heap2 (vacuum/freeze)
  - Per-table, per-page dirty page tracking for surgical cache invalidation
  - Full-page image extraction — update Arrow cache directly from WAL, zero file I/O
- [ ] **12c — Logical replica mode (Mode 3)** (3-4 weeks):
  - Base checkpoint: `CREATE_REPLICATION_SLOT pg_arrow_data LOGICAL pgoutput` + initial COPY using slot snapshot
  - Convert initial COPY to Arrow RecordBatches, persist as Parquet checkpoint
  - pgoutput stream consumer: BEGIN, INSERT, UPDATE, DELETE, COMMIT, Relation
  - Arrow store: `TableState` with frozen_batches, write_buffer, deletion bitmap, PK index
  - Apply changes atomically per transaction (buffer until COMMIT)
  - LSM-style compaction: merge write buffer into frozen batches, remove deleted rows
  - Parquet checkpoint persistence: periodic snapshot of Arrow state for crash recovery
  - Crash recovery: load latest Parquet checkpoint → resume logical stream from checkpoint LSN
  - DDL handling: detect schema changes from Relation messages + periodic pg_class polling
  - Requires `REPLICA IDENTITY` on tables, `wal_level = logical`, PostgreSQL 10+ (16+ on standby)
- [ ] **12d — Hybrid per-table strategy**:
  - Auto-select HeapFileScan / WalInvalidatedCache / LogicalReplication per table
  - Track query frequency per table, auto-promote strategy as tables get hotter
  - Configurable per-table override in `pg_arrow.toml`

### Phase 13: Production Readiness (3-4 weeks)

- [ ] **Observability**:
  - Prometheus metrics endpoint (`/metrics`): query latency, page reads, cache hits, connections, memory, replication lag
  - OpenTelemetry tracing: per-query spans through plan → scan → page_read → mvcc → arrow_convert → execute → send
  - Structured JSON logging via `tracing` + `tracing-subscriber`
  - Health endpoints: `/health` (liveness), `/ready` (readiness), `/status` (JSON stats)
- [ ] **Configuration management**:
  - `pg_arrow.toml` config file with server, postgresql, cache, wal, logging, tracing, security sections
  - CLI argument parsing (clap)
  - Environment variable overrides (`PG_ARROW_DATA_DIR`, `PG_ARROW_PORT`, etc.)
  - Runtime-reconfigurable settings via SIGHUP (log level, cache size, query timeout, sample rate)
- [ ] **Graceful lifecycle**:
  - Signal handling: SIGTERM (drain + flush cache), SIGINT (fast shutdown), SIGHUP (config reload)
  - Startup sequence: validate → connect → load schema → load cache → warm up → accept connections
  - Shutdown sequence: stop accepting → drain in-flight → persist cache → close connections → exit
- [ ] **Error handling / resilience**:
  - PostgreSQL connection circuit breaker (Closed → Open → Half-Open)
  - Degraded mode: serve frozen-data queries when PG connection is lost
  - Query timeout enforcement with cancellation
  - Memory limit enforcement with cache eviction under pressure
- [ ] **Connection management**:
  - Max connections limit, connection queuing with backpressure
  - Idle connection timeout, authentication timeout
  - Per-query memory limit via DataFusion MemoryPool

**Total**: 33-50 weeks for full production-ready system with all deployment modes, ecosystem integrations, and testing infrastructure

---
