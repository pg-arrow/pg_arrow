# AI Assistant Development Guidelines

## Project Overview

- **Project**: pg_arrow - PostgreSQL to Apache Arrow converter
- **Language**: Rust
- **Purpose**: Read PostgreSQL data files directly and convert to Arrow format
- **Key Focus**: Low-level PostgreSQL page format parsing

## File Organization

- `src/file/`: Page file reading and parsing
- `src/codec/`: Data encoding/decoding logic
- `src/arrow/`: Arrow format conversion
- `src/util/`: Helper utilities
- `docs/design/DESIGN.md`: Main design document (~6000+ lines, TOC at top, changelog at top)
- `RESEARCH/WAL_FORMAT.md`: WAL file physical format reference (~1900 lines, Rust struct definitions)

When adding new sub-crates or modules (e.g., `pg_arrow_core`, `pg_arrow_logical`, `pg_arrow_datafusion`), add a `.CLAUDE.md` file in that crate's root with crate-specific context: purpose, key types, internal conventions, and important invariants.

## Development Principles

### Code Quality

- Prefer explicit error handling over unwrap()
- Use meaningful variable names that reflect PostgreSQL terminology
- Add inline comments for complex bit manipulation and offset calculations
- Document PostgreSQL page format assumptions
- Run `cargo fmt` and `cargo clippy` before committing — treat clippy warnings as errors (`cargo clippy -- -D warnings`)
- All public API types and functions must have doc comments (`///`)

### Error Handling

- Use `thiserror` for library error types (structured, typed errors in `pg_arrow_core`)
- Use `anyhow` only in binary entry points and tests
- Never `unwrap()` or `expect()` on data from external sources (files, network, user input)
- `unwrap()` is acceptable only on invariants proven by prior logic — add a comment explaining why
- Propagate errors with `?` — let callers decide how to handle them
- Include context in errors: file path, offset, page number, expected vs actual values

### `unsafe` Code Policy

- Minimize `unsafe` — prefer safe abstractions even at slight performance cost
- Every `unsafe` block **must** have a `// SAFETY:` comment explaining why the invariants hold
- Candidate uses: memory-mapped I/O, SIMD intrinsics, zero-copy Arrow buffer construction
- Never use `unsafe` to bypass bounds checks on untrusted input — validate first, then use `unsafe` on the validated range if needed
- All `unsafe` code must be covered by Miri (`cargo miri test`) and AddressSanitizer in CI

### Dependency Policy

- Prefer stdlib solutions before adding crates
- Allowed crates by category:
  - Serialization: `serde`, `toml` (config), `serde_json` (output)
  - Arrow: `arrow`, `datafusion`, `parquet`
  - Error handling: `thiserror` (lib), `anyhow` (bin/tests)
  - Async: `tokio`
  - Testing: `proptest`, `insta`, `criterion`
- For anything else, justify the dependency — evaluate maintenance status, transitive deps, and compile time impact

## Rust Best Practices

### Security

- **Validate all external input before use**: Every offset, length, and index from an external source must be bounds-checked before accessing memory. Malformed input should produce an error, never a crash.
- **No unchecked indexing on untrusted data**: Use `.get()` instead of `[]` for buffers from I/O. Reserve `[]` for indices proven in-bounds by prior validation.
- **Integer overflow**: Use `checked_add()`, `checked_mul()`, `saturating_add()` for arithmetic on external values. Never assume offsets or sizes fit their type.
- **No secret material in memory longer than needed**: Zeroize passwords and auth tokens after use. Prefer `secrecy` crate for sensitive values.
- **Deny unsafe patterns in CI**: Use `#![deny(unsafe_op_in_unsafe_fn)]` to require explicit `unsafe` blocks even inside `unsafe fn` bodies.

### Reliability

- **Make invalid states unrepresentable**: Use enums and newtypes to enforce invariants at the type level (e.g., `PageNumber(u32)` instead of bare `u32`).
- **Fail fast on corruption**: Return `Err` immediately on invalid input. Do not attempt recovery or "best effort" parsing of corrupt data.
- **Exhaustive matching**: Always match all enum variants explicitly (`match` without `_ =>`). This ensures new variants cause compile errors, not silent bugs.
- **Resource cleanup with RAII**: File handles, mmap regions, and connections must be managed with `Drop` impls. Never rely on manual cleanup.
- **Test error paths, not just happy paths**: Every `Err` variant should have at least one test that triggers it.

### Performance

- **Minimize allocations in hot paths**: Pre-allocate buffers, reuse `Vec`s across iterations, avoid `format!()` in loops.
- **Prefer stack allocation for fixed-size structures**: Small, known-size structs should live on the stack, not behind `Box`.
- **Borrow instead of clone**: Use `&[u8]` slices over `Vec<u8>` copies when the source outlives the consumer. Only copy when ownership transfer is required.
- **Batch I/O operations**: Minimize syscall count — read multiple items per call (`pread` with large buffers, `mmap`). Per-item `read()` calls are syscall-overhead dominated.
- **Profile before optimizing**: Use `criterion` for micro-benchmarks, `perf`/`samply` for CPU profiling, `dhat` for allocation profiling. Don't optimize without data.
- **Cache-friendly access patterns**: Process data sequentially, not randomly. Keep hot data together for good L1/L2 cache utilization.
- **Avoid `Arc<Mutex<>>` in hot paths**: For shared read-only data, use `Arc<T>` without Mutex. For concurrent writes, prefer lock-free structures or sharded locks.

### Common Pitfalls

**Rust-Specific:**
- `as` casts truncate silently — use `u32::try_from(val)?` for fallible narrowing on external data
- Returning references from a function that owns the source (mmap, Vec) causes use-after-free — return owned data or ensure the source outlives borrows
- Always validate encoding from external sources with `String::from_utf8()`, never `from_utf8_unchecked`
- Use `#[cfg(target_os = "linux")]` for platform-specific features; ensure code compiles on all supported platforms
- Prefer `iter().map().collect()` over manual indexing with mutation

**Binary Parsing:**
- Know the byte order of the format you're reading — use the correct `from_*_bytes()` variant
- Many binary formats require aligned access — check the format spec for alignment rules
- Bit numbering conventions vary between formats (MSB-first vs LSB-first) — verify against the spec

**Domain-Specific (PostgreSQL):**
PostgreSQL internals have many subtle behaviors (overloaded header fields, multi-file storage, advisory metadata, schema evolution gaps). Always consult `docs/design/DESIGN.md` before implementing any PostgreSQL format parsing — do not rely on assumptions.

## PostgreSQL Testing

### Setup

```bash
# Quick setup with test data (uses latest/master)
./scripts/setup-postgres.sh -B -i -t

# Setup specific version using friendly names
./scripts/setup-postgres.sh -b pg18 -B -i -t
./scripts/setup-postgres.sh -b pg17 -B -i -t

# Setup with simple schema for basic testing
./scripts/setup-postgres.sh -b latest -B -i -t -s
```

All installations are local to the project directory. No root/postgres user needed.

### `pg-test-config.toml` Format

Generated by `setup-postgres.sh`. Each version is a `[postgres.<name>]` section:

```toml
[postgres.latest]
version = "master"                # Git branch
source_dir = "testdata/postgres-latest"
data_dir = "testdata/postgres-latest/data"       # $PGDATA
bin_dir = "testdata/postgres-latest/install/bin"  # psql, pg_ctl, etc.
initialized = false               # Has initdb been run?
test_db_created = false            # Has test data been loaded?
```

Read this file programmatically to locate binaries and data directories. Never hardcode paths.

### Running Tests

```bash
cargo test                    # Run all tests
cargo test test_name          # Run specific test
cargo test -- --nocapture     # Run with output
```

### Testing Guidelines

- Use real PostgreSQL data files when possible
- Test across different PostgreSQL versions (master, REL_18_STABLE, etc.)
- Include edge cases (NULLs, empty strings, max values, unicode)
- Validate both page-level and row-level parsing

## AI Assistant Best Practices

### Before Making Changes

1. Read existing code in the relevant module
2. Understand current patterns and conventions
3. Check test files for examples
4. Consult `docs/design/DESIGN.md` for PostgreSQL internals details

### When Suggesting Improvements

- Always suggest the simplest stdlib fix first before recommending external crates
- Don't over-engineer: if a one-line stdlib change solves the problem, say that before listing crate alternatives

### When Stuck

1. Check PostgreSQL source code comments
2. Review existing parser implementations
3. Look at test cases for similar features
4. Check `docs/design/DESIGN.md` — detailed analysis of every PostgreSQL subsystem pg_arrow interacts with

## Updating the Design Doc (`docs/design/DESIGN.md`)

The design doc is ~6000+ lines. Follow these rules when editing:

- **Read before editing**: Always read the target section first. Use `Grep` to find section headers (`^## `, `^### `) and line numbers.
- **Use unique context in `old_string`**: The file has repeated patterns. Include enough surrounding lines to make the match unique.
- **Maintain section order**: The TOC at the top reflects section order. New sections must be inserted correctly and the TOC updated.
- **Update the TOC**: Use GitHub-style anchors (lowercase, spaces→hyphens, strip special chars). Duplicate headings get `-1`, `-2` suffixes.
- **Update the changelog**: Add a new entry at the top of the changelog block (reverse chronological).
- **Update the implementation plan**: If adding features/benchmarks, add checklist items under the relevant Phase.
- **Update the Testing Matrix**: If adding test types/benchmarks, add rows to the Testing Matrix Summary table.
- **Check for orphaned content**: After large edits, verify no content was displaced or duplicated.
- **Match existing formatting**: Follow neighboring section patterns (header levels, tables, code blocks, ASCII diagrams).

## Maintaining CLAUDE.md Files

These instruction files are living documents — update them as the project evolves.

### When to Update

- **Always update the changelog**: Every change to any `CLAUDE.md` file must be recorded in the changelog at the **bottom** of that file (reverse chronological). This prevents re-introducing removed instructions.
- **New convention established**: Add it so future sessions follow it.
- **New pitfall discovered**: Document in Common Pitfalls so it isn't repeated.
- **New crate adopted**: Add to the Dependency Policy allowlist.
- **Module structure changes**: Update File Organization and create/update sub-crate `.CLAUDE.md`.
- **Stale information**: Remove or correct anything no longer true.

### Sub-Crate `.CLAUDE.md` Files

Each sub-crate should have its own `.CLAUDE.md` covering:
- **Purpose**: One-line description
- **Key types**: Main structs/enums and what they represent
- **Internal conventions**: Crate-specific patterns
- **Important invariants**: Things that must always be true
- **Error types**: The crate's error enum and when each variant is used
- **`unsafe` inventory**: All `unsafe` blocks and why they exist

Update when: new public types added, new `unsafe` blocks introduced, error variants changed, module boundaries shift.

### Self-Improvement Rule

After completing any non-trivial task, consider: *"Did I learn something that would help next time?"* If yes, update the appropriate `CLAUDE.md` (root or sub-crate).

## Resources

- [PostgreSQL Source](https://git.postgresql.org/gitweb/?p=postgresql.git)
- [PostgreSQL Page Format](https://www.postgresql.org/docs/current/storage-page-layout.html)
- [Apache Arrow Format](https://arrow.apache.org/docs/format/Columnar.html)
- [PostgreSQL Internals](https://www.postgresql.org/docs/current/internals.html)

---

> **Changelog** (newest first — this section is intentionally last for LLM context cache optimization):
>
> - 2026-02-12: Restructured for context cache optimization. Moved changelog to bottom (volatile
>   content last, stable prefix first). Consolidated redundant sections (Notes merged into
>   Development Principles, Testing Strategy + Running Tests + PostgreSQL Integration merged into
>   PostgreSQL Testing, Resources flattened). Reduced from 310 to ~260 lines.
> - 2026-02-12: Added changelog to CLAUDE.md to track instruction changes and prevent regressions.
> - 2026-02-12: Removed PostgreSQL-specific knowledge from Common Pitfalls (xmax overloading,
>   TOAST details, attisdropped, segment files, visibility map, null bitmap, pd_lower/pd_upper,
>   MAXALIGN formula, from_ne_bytes). Replaced with general guidelines pointing to DESIGN.md.
>   Removed "PostgreSQL Internals Reference" section entirely.
> - 2026-02-12: Added "Maintaining CLAUDE.md Files" section (when to update, sub-crate
>   .CLAUDE.md file template, self-improvement rule).
> - 2026-02-12: Added "Rust Best Practices" section (security, reliability, performance,
>   common pitfalls for binary parsing and Rust-specific issues).
> - 2026-02-12: Added Error Handling, unsafe Code Policy, Dependency Policy subsections.
>   Added pg-test-config.toml format docs. Split Resources into External and Project-Internal.
> - 2026-02-12: Added "Updating the Design Doc" subsection under Common Tasks.
> - 2026-02-12: Added note about .CLAUDE.md files for sub-crates under File Organization.
> - 2026-02-12: Initial version with project overview, development principles, common tasks,
>   PostgreSQL internals reference, AI assistant best practices, resources, and notes.
