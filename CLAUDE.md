# AI Assistant Development Guidelines

## Project Overview

- **Project**: pg_arrow - PostgreSQL to Apache Arrow converter
- **Language**: Rust
- **Purpose**: Read PostgreSQL data files directly and convert to Arrow format
- **Key Focus**: Low-level PostgreSQL page format parsing

## Development Principles

### Code Quality

- Prefer explicit error handling over unwrap()
- Use meaningful variable names that reflect PostgreSQL terminology
- Add inline comments for complex bit manipulation and offset calculations
- Document PostgreSQL page format assumptions

### PostgreSQL Integration

- Test data is managed via `scripts/setup-postgres.sh` script
- Configuration is in `pg-test-config.toml`
- All PostgreSQL source and data is in the `testdata/` directory (excluded from git)
- Never commit PostgreSQL source or data directories
- Use multiple PostgreSQL versions for compatibility testing
- All PostgreSQL installations are local to project (no system-wide installation)
- Version mapping: `pg18`→REL_18_STABLE, `pg17`→REL_17_STABLE, `latest`→master
- Directory structure: `testdata/postgres-{version}/` for each PostgreSQL version

### Testing Strategy

- Tests should use real PostgreSQL data files when possible
- Create minimal reproducible test cases
- Test across different PostgreSQL versions (master, REL_18_STABLE, etc.)
- Validate both page-level and row-level parsing

### File Organization

- `src/file/`: Page file reading and parsing
- `src/codec/`: Data encoding/decoding logic
- `src/arrow/`: Arrow format conversion
- `src/util/`: Helper utilities

## Common Tasks

### Setting Up PostgreSQL for Testing

```bash
# Quick setup with test data (uses latest/master)
./scripts/setup-postgres.sh -B -i -t

# Setup specific version using friendly names
./scripts/setup-postgres.sh -b pg18 -B -i -t
./scripts/setup-postgres.sh -b pg17 -B -i -t

# Setup with simple schema for basic testing
./scripts/setup-postgres.sh -b latest -B -i -t -s

# All installations are local to project directory
# No root/postgres user needed - runs as current user
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture
```

### Adding New Features

1. Check existing PostgreSQL data type handling
2. Review PostgreSQL source code for format details
3. Implement parser in appropriate module
4. Add integration tests with real data
5. Update documentation

## PostgreSQL Internals Reference

### Page Structure

- Page size: Typically 8KB (8192 bytes)
- Page header: 24 bytes (PageHeaderData)
- Item pointers: Array of (offset, length) tuples
- Items: Actual row data (heap tuples)

### Important Constants

- DEFAULT_PAGE_SIZE: 8192
- BLCKSZ: Block size (usually 8192)
- MAXALIGN: Alignment boundary

### Tuple Structure

- HeapTupleHeaderData: Tuple header
- Null bitmap: Variable length
- Tuple data: Column values

## AI Assistant Best Practices

### Before Making Changes

1. Read existing code in the relevant module
2. Understand current patterns and conventions
3. Check test files for examples
4. Review PostgreSQL documentation if needed

### When Adding PostgreSQL Integration

1. Use the setup script to create test databases
2. Verify compatibility across versions
3. Don't hardcode paths - use pg-test-config.toml
4. Add version-specific handling if needed

### When Writing Tests

1. Use the test database created by setup script
2. Test with both simple and complex schemas
3. Include edge cases (NULLs, empty strings, max values, unicode)
4. Clean up test data after tests complete

### When Stuck

1. Check PostgreSQL source code comments
2. Review existing parser implementations
3. Look at test cases for similar features
4. Consult PostgreSQL documentation

## Resources

- [PostgreSQL Source](https://git.postgresql.org/gitweb/?p=postgresql.git)
- [PostgreSQL Page Format](https://www.postgresql.org/docs/current/storage-page-layout.html)
- [Apache Arrow Format](https://arrow.apache.org/docs/format/Columnar.html)
- [PostgreSQL Internals](https://www.postgresql.org/docs/current/internals.html)

## Notes

- This is a low-level project working with binary formats
- Performance matters - avoid unnecessary allocations
- Safety matters - validate all offsets and lengths
- PostgreSQL version differences matter - test thoroughly
- Use `pg-test-config.toml` to locate test databases programmatically
