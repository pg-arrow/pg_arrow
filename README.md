# PG Arrow

## Development

### TODO

1. Write test to read from db datafile and parse and validate the postgres table data.
   - Page size
   - Page header
   - Page items
   - Row header
   - Row content
   - Column values
     - TOASTed data
     - Encoding/Compression
2. Table Scan
   - Sync Reader
   - Page iterator
   - Row iterator
   - Async Reader
3. Catalog Reader
   - pg_catalog finder and reader
4. PG Arrow extender type
   - Support user defined data parser

## PostgreSQL Setup for Testing

### Quick Setup Script

Use `scripts/setup-postgres.sh` to automate PostgreSQL setup for testing:

#### Basic Usage (Source Only)

```bash
# Setup latest/master branch (source code + data directory only)
./scripts/setup-postgres.sh

# Setup specific version using friendly names
./scripts/setup-postgres.sh --branch pg18
./scripts/setup-postgres.sh -b pg17
./scripts/setup-postgres.sh -b pg16

# Or use full branch names
./scripts/setup-postgres.sh -b REL_18_STABLE
```

#### Full Setup (Build + Initialize + Test Data)

```bash
# Complete setup with build, initialization, and full e-commerce test data
./scripts/setup-postgres.sh --build --init --test-data

# Setup PG 18 with everything (short form)
./scripts/setup-postgres.sh -b pg18 -B -i -t

# Setup with simple single-table schema
./scripts/setup-postgres.sh -b latest -B -i -t -s

# Setup latest with everything
./scripts/setup-postgres.sh -b latest -B -i -t
```

#### Options

- `-b, --branch VERSION`: Version/branch name (default: latest)
  - Friendly names: `pg18`, `pg17`, `pg16`, `latest`
  - Full branch names: `master`, `REL_18_STABLE`, `REL_17_STABLE`, etc.
- `-B, --build`: Build PostgreSQL locally using meson/ninja
- `-i, --init`: Initialize database cluster (requires --build)
- `-t, --test-data`: Create test database with sample tables (requires --init)
- `-s, --simple-schema`: Use simple single-table schema instead of full e-commerce schema
- `-h, --help`: Show usage information

#### What the Script Does

1. Clones PostgreSQL from <https://git.postgresql.org/git/postgresql.git> into `testdata/postgres/`
2. Maps version name to branch (pg18 → REL_18_STABLE, latest → master)
3. Creates git worktree in `testdata/postgres-{version}/` directory
4. Creates data directory at `testdata/postgres-{version}/data/`
5. Optionally builds PostgreSQL locally (installs to `testdata/postgres-{version}/install/`)
6. Optionally initializes database cluster using current user (no postgres user needed)
7. Optionally creates test database with schema:
   - Simple: Single table with all common datatypes (use `-s` flag)
   - Full: E-commerce schema with 5 tables and relationships (default)
8. Writes configuration to `pg-test-config.toml`
9. Prints setup summary to stdout

**Note**: Everything is installed locally within the project's `testdata/` directory - no system-wide installation or special user privileges required.

#### Directory Structure

After running the setup script, your project will have this structure:

```
pg_arrow/
├── testdata/
│   ├── postgres/              # Main PostgreSQL git repository
│   ├── postgres-latest/       # Worktree for master branch
│   │   ├── data/              # PostgreSQL data directory
│   │   ├── build/             # Meson build directory
│   │   └── install/
│   │       └── bin/           # PostgreSQL binaries
│   └── postgres-pg18/         # Worktree for REL_18_STABLE
│       ├── data/
│       ├── build/
│       └── install/
│           └── bin/
├── pg-test-config.toml        # Configuration file with paths
└── scripts/
    └── setup-postgres.sh      # Setup script
```

#### Configuration File

The script generates `pg-test-config.toml` with paths for use in Rust tests:

```toml
[postgres.latest]
version = "master"
source_dir = "/path/to/project/testdata/postgres-latest"
data_dir = "/path/to/project/testdata/postgres-latest/data"
install_dir = "/path/to/project/testdata/postgres-latest/install"
bin_dir = "/path/to/project/testdata/postgres-latest/install/bin"
initialized = true
test_db_created = true

[postgres.pg18]
version = "REL_18_STABLE"
source_dir = "/path/to/project/testdata/postgres-pg18"
data_dir = "/path/to/project/testdata/postgres-pg18/data"
install_dir = "/path/to/project/testdata/postgres-pg18/install"
bin_dir = "/path/to/project/testdata/postgres-pg18/install/bin"
initialized = true
test_db_created = true
```

#### Test Database Schema

When using `--test-data`, a test database is created with one of two schemas:

**Simple Schema** (use `-s` flag):

- Single table: `test_types`
- All common PostgreSQL datatypes: INTEGER, BIGINT, SMALLINT, NUMERIC, REAL, DOUBLE PRECISION, VARCHAR, TEXT, CHAR, BOOLEAN, DATE, TIMESTAMP, TIMESTAMPTZ, JSONB, BYTEA, UUID
- Basic indexes (B-tree and GIN for JSONB)
- 5 sample rows with edge cases (max values, negatives, zeros, NULLs, unicode)
- Ideal for basic datatype parsing tests

**Full E-commerce Schema** (default):

- 5 tables: `categories`, `products`, `customers`, `orders`, `order_items`
- Multiple foreign key relationships including self-referencing
- Various datatypes: SERIAL, BIGSERIAL, VARCHAR, TEXT, NUMERIC, REAL, BOOLEAN, DATE, TIMESTAMP, JSONB, BYTEA
- Comprehensive indexing: PRIMARY, UNIQUE, B-tree, GIN
- ~20+ rows of realistic sample data
- Ideal for testing complex relationships and real-world scenarios

### Manual Setup

If you prefer manual setup, here are step-by-step instructions for local installation:

```bash
# Clone PostgreSQL source
git clone https://git.postgresql.org/git/postgresql.git postgres
cd postgres

# Checkout specific branch if needed
git checkout REL_18_STABLE

# Build and install locally (no root/su needed)
meson setup build --prefix=$PWD/install
cd build
ninja
ninja install

# Initialize database cluster as current user
cd ..
mkdir -p data
./install/bin/initdb -D ./data

# Start PostgreSQL
./install/bin/pg_ctl -D ./data -l logfile start

# Create test database
./install/bin/createdb test

# Connect to database
./install/bin/psql test
```

**Note**: The script automates this process and supports multiple versions simultaneously using git worktrees.
