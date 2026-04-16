#!/usr/bin/env bash

# PostgreSQL Setup Script
# Sets up PostgreSQL source code for testing using git worktrees
# Supports multiple versions simultaneously with local installations

set -e # Exit on error

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TESTDATA_DIR="$PROJECT_ROOT/testdata"
PG_GIT_URL="https://git.postgresql.org/git/postgresql.git"
PG_MAIN_REPO="postgres-latest"
CONFIG_FILE="pg-test-config.toml"

# Default values
BRANCH="latest"
DO_BUILD=false
DO_INIT=false
DO_TEST_DATA=false
SIMPLE_SCHEMA=false

#=============================================================================
# Helper Functions
#=============================================================================

# Display usage information
usage() {
	cat <<EOF
Usage: $0 [OPTIONS]

Setup PostgreSQL source code for testing with local installation.

OPTIONS:
    -b, --branch VERSION    Version/branch name (default: latest)
                           Supported: pg18, pg17, pg16, latest, master, REL_XX_STABLE
    -B, --build            Build PostgreSQL after setup
    -i, --init             Initialize database (requires --build)
    -t, --test-data        Create test database with sample tables (requires --init)
    -s, --simple-schema    Use simple single-table schema instead of full e-commerce schema
    -h, --help             Show this help message

EXAMPLES:
    # Setup source code only for latest version
    $0

    # Setup PG 18 with build, init, and test data
    $0 -b pg18 -B -i -t

    # Setup latest with simple schema
    $0 -b latest -B -i -t -s

    # Setup specific branch
    $0 -b REL_17_STABLE -B -i -t

EOF
	exit 0
}

# Log informational message
log_info() {
	echo -e "${BLUE}[INFO]${NC} $*"
}

# Log success message
log_success() {
	echo -e "${GREEN}[SUCCESS]${NC} $*"
}

# Log warning message
log_warn() {
	echo -e "${YELLOW}[WARN]${NC} $*"
}

# Log error message and exit
log_error() {
	echo -e "${RED}[ERROR]${NC} $*" >&2
	exit 1
}

# Map user-friendly version names to PostgreSQL branch names
map_version_to_branch() {
	local version="$1"

	case "${version,,}" in # Convert to lowercase
	pg18 | postgres18)
		echo "REL_18_STABLE"
		;;
	pg17 | postgres17)
		echo "REL_17_STABLE"
		;;
	pg16 | postgres16)
		echo "REL_16_STABLE"
		;;
	pg15 | postgres15)
		echo "REL_15_STABLE"
		;;
	latest | master)
		echo "master"
		;;
	rel_*_stable | REL_*_STABLE)
		echo "$version"
		;;
	*)
		# Assume it's a valid branch name
		echo "$version"
		;;
	esac
}

# Normalize branch name for use in directory names
normalize_branch_name() {
	local branch="$1"
	local version_input="$2"

	# If user provided a friendly name (pg18, latest, etc), use that
	case "${version_input,,}" in
	pg18 | postgres18)
		echo "pg18"
		;;
	pg17 | postgres17)
		echo "pg17"
		;;
	pg16 | postgres16)
		echo "pg16"
		;;
	pg15 | postgres15)
		echo "pg15"
		;;
	latest | master)
		echo "latest"
		;;
	*)
		# Convert branch name to lowercase and replace underscores
		echo "${branch,,}" | tr '_' '-'
		;;
	esac
}

# Setup or update main PostgreSQL git repository (master branch)
setup_git_repo() {
	local repo_dir="$TESTDATA_DIR/$PG_MAIN_REPO"

	# Create testdata directory if it doesn't exist
	mkdir -p "$TESTDATA_DIR"

	if [ -d "$repo_dir/.git" ]; then
		log_info "PostgreSQL repository already exists, fetching latest changes..."
		cd "$repo_dir"
		git fetch origin || log_warn "Failed to fetch updates"
		cd "$PROJECT_ROOT"
	else
		log_info "Cloning PostgreSQL repository..."
		git clone "$PG_GIT_URL" "$repo_dir" || log_error "Failed to clone PostgreSQL repository"
		log_success "Repository cloned successfully"
	fi
}

# Create or update git worktree for the specified branch
setup_worktree() {
	local branch="$1"
	local worktree_dir="$2"
	local repo_dir="$TESTDATA_DIR/$PG_MAIN_REPO"

	if [ -d "$worktree_dir" ]; then
		log_info "Worktree already exists at $worktree_dir"
		cd "$worktree_dir"
		git pull origin "$branch" || log_warn "Failed to pull latest changes"
		cd "$PROJECT_ROOT"
	else
		log_info "Creating worktree for branch '$branch'..."
		cd "$repo_dir"
		git worktree add -f "$worktree_dir" "$branch" || log_error "Failed to create worktree"
		cd "$PROJECT_ROOT"
		log_success "Worktree created at $worktree_dir"
	fi

	# Create data directory
	mkdir -p "$worktree_dir/data"
	log_info "Data directory created at $worktree_dir/data"
}

# Build PostgreSQL using meson and ninja
build_postgres() {
	local worktree_dir="$1"
	local build_dir="$worktree_dir/build"
	local install_dir="$worktree_dir/install"

	log_info "Building PostgreSQL in $worktree_dir..."

	# Check for required build tools
	if ! command -v meson &>/dev/null; then
		log_error "meson not found. Please install meson: pip install meson or brew install meson"
	fi

	if ! command -v ninja &>/dev/null; then
		log_error "ninja not found. Please install ninja: pip install ninja or brew install ninja"
	fi

	cd "$worktree_dir"

	# Setup meson build with local prefix
	if [ ! -d "$build_dir" ]; then
		log_info "Configuring build with meson..."
		meson setup build --prefix="$install_dir" || log_error "Meson setup failed"
	else
		log_info "Build directory exists, reconfiguring..."
		meson setup --reconfigure build --prefix="$install_dir" || log_error "Meson reconfigure failed"
	fi

	# Build
	log_info "Compiling PostgreSQL (this may take several minutes)..."
	cd build
	ninja || log_error "Build failed"

	# Install to local directory
	log_info "Installing to $install_dir..."
	ninja install || log_error "Installation failed"

	cd "$SCRIPT_DIR"
	log_success "PostgreSQL built and installed successfully"
}

# Initialize PostgreSQL database cluster
init_database() {
	local worktree_dir="$1"
	local data_dir="$worktree_dir/data"
	local initdb_bin="$worktree_dir/install/bin/initdb"

	# Check if already initialized
	if [ -f "$data_dir/PG_VERSION" ]; then
		log_info "Database already initialized at $data_dir"
		return 0
	fi

	if [ ! -x "$initdb_bin" ]; then
		log_error "initdb not found at $initdb_bin. Did the build succeed?"
	fi

	log_info "Initializing database cluster..."
	"$initdb_bin" -D "$data_dir" || log_error "Database initialization failed"

	log_success "Database initialized at $data_dir"
}

# Create simple schema with single table
create_simple_schema() {
	local psql_bin="$1"
	local dbname="$2"

	log_info "Creating simple schema with test_types table..."

	"$psql_bin" -d "$dbname" <<'EOF'
-- Simple test table with common datatypes
CREATE TABLE test_types (
    id BIGSERIAL PRIMARY KEY,
    int_col INTEGER,
    bigint_col BIGINT,
    smallint_col SMALLINT,
    numeric_col NUMERIC(10, 2),
    real_col REAL,
    double_col DOUBLE PRECISION,
    varchar_col VARCHAR(100),
    text_col TEXT,
    char_col CHAR(10),
    boolean_col BOOLEAN,
    date_col DATE,
    timestamp_col TIMESTAMP,
    timestamptz_col TIMESTAMPTZ,
    json_col JSONB,
    bytea_col BYTEA,
    uuid_col UUID,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_test_types_int_col ON test_types(int_col);
CREATE INDEX idx_test_types_varchar_col ON test_types(varchar_col);
CREATE INDEX idx_test_types_date_col ON test_types(date_col);
CREATE INDEX idx_test_types_json_col ON test_types USING GIN(json_col);

-- Sample data with various values including NULLs
INSERT INTO test_types (int_col, bigint_col, smallint_col, numeric_col, real_col, double_col, varchar_col, text_col, char_col, boolean_col, date_col, timestamp_col, timestamptz_col, json_col, uuid_col) VALUES
    (42, 9223372036854775807, 32767, 12345.67, 3.14159, 2.718281828, 'Hello World', 'This is a long text field that can contain multiple sentences and paragraphs.', 'CHAR_VAL', true, '2024-01-15', '2024-01-15 10:30:00', '2024-01-15 10:30:00-08:00', '{"key": "value", "number": 42}', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'),
    (-100, -123456789, -30000, -999.99, -1.23, -4.56, 'Test String', 'Short text', 'ABC', false, '2023-12-01', '2023-12-01 15:45:30', '2023-12-01 15:45:30+00:00', '{"array": [1, 2, 3], "nested": {"a": 1}}', 'b1eebc99-9c0b-4ef8-bb6d-6bb9bd380a22'),
    (0, 0, 0, 0.00, 0.0, 0.0, '', 'Empty string test', '', true, '2000-01-01', '2000-01-01 00:00:00', '2000-01-01 00:00:00+00:00', '{}', 'c2eebc99-9c0b-4ef8-bb6d-6bb9bd380a33'),
    (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL),
    (2147483647, 1234567890, 100, 99999.99, 123.456, 789.012, 'Special chars: !@#$%', 'Unicode: 你好世界 🌍', 'UNICODE', false, '2025-06-30', '2025-06-30 23:59:59', '2025-06-30 23:59:59-07:00', '{"unicode": "你好", "emoji": "🎉"}', 'd3eebc99-9c0b-4ef8-bb6d-6bb9bd380a44');
EOF

	log_success "Simple schema created successfully"
}

# Create e-commerce schema with multiple tables
create_ecommerce_schema() {
	local psql_bin="$1"
	local dbname="$2"

	log_info "Creating e-commerce schema with multiple tables..."

	"$psql_bin" -d "$dbname" <<'EOF'
-- Categories table - simple reference data
CREATE TABLE categories (
    id SERIAL PRIMARY KEY,
    name VARCHAR(100) NOT NULL,
    description TEXT,
    parent_id INTEGER REFERENCES categories(id),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_categories_name ON categories(name);
CREATE INDEX idx_categories_parent_id ON categories(parent_id);

-- Products table - various datatypes
CREATE TABLE products (
    id BIGSERIAL PRIMARY KEY,
    sku VARCHAR(50) NOT NULL UNIQUE,
    name VARCHAR(200) NOT NULL,
    description TEXT,
    price NUMERIC(10, 2) NOT NULL,
    cost NUMERIC(10, 2),
    weight REAL,
    is_active BOOLEAN DEFAULT true,
    stock_quantity INTEGER DEFAULT 0,
    category_id INTEGER REFERENCES categories(id),
    metadata JSONB,
    image_data BYTEA,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_products_sku ON products(sku);
CREATE INDEX idx_products_name ON products(name);
CREATE INDEX idx_products_category_id ON products(category_id);
CREATE INDEX idx_products_price ON products(price);
CREATE INDEX idx_products_is_active ON products(is_active);
CREATE INDEX idx_products_metadata ON products USING GIN(metadata);

-- Customers table - various text and date types
CREATE TABLE customers (
    id BIGSERIAL PRIMARY KEY,
    email VARCHAR(255) NOT NULL UNIQUE,
    first_name VARCHAR(100),
    last_name VARCHAR(100),
    phone VARCHAR(20),
    address TEXT,
    city VARCHAR(100),
    state VARCHAR(50),
    zip_code VARCHAR(10),
    country VARCHAR(50) DEFAULT 'USA',
    date_of_birth DATE,
    is_verified BOOLEAN DEFAULT false,
    loyalty_points INTEGER DEFAULT 0,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_customers_email ON customers(email);
CREATE INDEX idx_customers_last_name ON customers(last_name);
CREATE INDEX idx_customers_created_at ON customers(created_at);

-- Orders table - temporal and numeric data
CREATE TABLE orders (
    id BIGSERIAL PRIMARY KEY,
    order_number VARCHAR(50) NOT NULL UNIQUE,
    customer_id BIGINT NOT NULL REFERENCES customers(id),
    status VARCHAR(20) DEFAULT 'pending',
    subtotal NUMERIC(12, 2) NOT NULL,
    tax NUMERIC(10, 2) DEFAULT 0,
    shipping NUMERIC(10, 2) DEFAULT 0,
    total NUMERIC(12, 2) NOT NULL,
    notes TEXT,
    order_date TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    shipped_date TIMESTAMP,
    delivered_date TIMESTAMP
);
CREATE INDEX idx_orders_order_number ON orders(order_number);
CREATE INDEX idx_orders_customer_id ON orders(customer_id);
CREATE INDEX idx_orders_status ON orders(status);
CREATE INDEX idx_orders_order_date ON orders(order_date);

-- Order items table - join table with quantities
CREATE TABLE order_items (
    id BIGSERIAL PRIMARY KEY,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    product_id BIGINT NOT NULL REFERENCES products(id),
    quantity INTEGER NOT NULL DEFAULT 1,
    unit_price NUMERIC(10, 2) NOT NULL,
    discount NUMERIC(10, 2) DEFAULT 0,
    total NUMERIC(12, 2) NOT NULL
);
CREATE INDEX idx_order_items_order_id ON order_items(order_id);
CREATE INDEX idx_order_items_product_id ON order_items(product_id);

-- Sample data
INSERT INTO categories (name, description, parent_id) VALUES
    ('Electronics', 'Electronic devices and accessories', NULL),
    ('Computers', 'Desktop and laptop computers', 1),
    ('Phones', 'Mobile phones and tablets', 1),
    ('Clothing', 'Apparel and accessories', NULL);

INSERT INTO products (sku, name, description, price, cost, weight, category_id, stock_quantity, metadata) VALUES
    ('LAPTOP-001', 'Premium Laptop', 'High-performance laptop with 16GB RAM', 1299.99, 899.99, 2.5, 2, 15, '{"brand": "TechCo", "warranty": "2 years"}'),
    ('PHONE-001', 'Smartphone Pro', '5G smartphone with advanced camera', 899.99, 599.99, 0.4, 3, 50, '{"brand": "PhoneCo", "color": "black", "storage": "256GB"}'),
    ('SHIRT-001', 'Cotton T-Shirt', 'Comfortable cotton t-shirt', 24.99, 12.50, 0.2, 4, 100, '{"size": "M", "color": "blue"}'),
    ('LAPTOP-002', 'Budget Laptop', 'Affordable laptop for everyday use', 599.99, 399.99, 2.2, 2, 25, '{"brand": "ValueTech", "warranty": "1 year"}'),
    ('PHONE-002', 'Basic Phone', 'Simple smartphone for calls and texts', 299.99, 199.99, 0.3, 3, 75, '{"brand": "SimpleCo", "storage": "64GB"}');

INSERT INTO customers (email, first_name, last_name, phone, address, city, state, zip_code, date_of_birth, is_verified, loyalty_points) VALUES
    ('john.doe@example.com', 'John', 'Doe', '555-0101', '123 Main St', 'Springfield', 'IL', '62701', '1985-03-15', true, 150),
    ('jane.smith@example.com', 'Jane', 'Smith', '555-0102', '456 Oak Ave', 'Portland', 'OR', '97201', '1990-07-22', true, 300),
    ('bob.wilson@example.com', 'Bob', 'Wilson', '555-0103', '789 Pine Rd', 'Austin', 'TX', '78701', '1982-11-08', false, 0),
    ('alice.brown@example.com', 'Alice', 'Brown', '555-0104', '321 Elm St', 'Seattle', 'WA', '98101', '1995-01-30', true, 450);

INSERT INTO orders (order_number, customer_id, status, subtotal, tax, shipping, total, order_date, shipped_date) VALUES
    ('ORD-2024-001', 1, 'delivered', 1299.99, 104.00, 15.00, 1418.99, '2024-01-15 10:30:00', '2024-01-16 14:00:00'),
    ('ORD-2024-002', 2, 'shipped', 924.98, 74.00, 10.00, 1008.98, '2024-01-20 15:45:00', '2024-01-21 09:00:00'),
    ('ORD-2024-003', 1, 'pending', 24.99, 2.00, 5.00, 31.99, '2024-01-25 11:20:00', NULL),
    ('ORD-2024-004', 3, 'delivered', 599.99, 48.00, 12.00, 659.99, '2024-01-18 09:15:00', '2024-01-19 16:30:00');

INSERT INTO order_items (order_id, product_id, quantity, unit_price, discount, total) VALUES
    (1, 1, 1, 1299.99, 0, 1299.99),
    (2, 2, 1, 899.99, 0, 899.99),
    (2, 3, 1, 24.99, 0, 24.99),
    (3, 3, 1, 24.99, 0, 24.99),
    (4, 4, 1, 599.99, 0, 599.99);
EOF

	log_success "E-commerce schema created successfully"
}

# Create test database and populate with data
create_test_db() {
	local worktree_dir="$1"
	local pg_ctl_bin="$worktree_dir/install/bin/pg_ctl"
	local createdb_bin="$worktree_dir/install/bin/createdb"
	local psql_bin="$worktree_dir/install/bin/psql"
	local data_dir="$worktree_dir/data"
	local test_dbname="test"

	if [ ! -x "$pg_ctl_bin" ]; then
		log_error "pg_ctl not found. Database must be built and initialized first."
	fi

	# Check if server is already running
	local is_running=false
	if "$pg_ctl_bin" -D "$data_dir" status &>/dev/null; then
		is_running=true
		log_info "PostgreSQL server is already running"
	else
		# Start PostgreSQL server temporarily
		log_info "Starting PostgreSQL server..."
		"$pg_ctl_bin" -D "$data_dir" -l "$data_dir/logfile" start || log_error "Failed to start PostgreSQL server"
		sleep 2 # Give it time to start
		log_success "PostgreSQL server started"
	fi

	# Check if database already exists
	if "$psql_bin" -lqt | cut -d \| -f 1 | grep -qw "$test_dbname"; then
		log_info "Database '$test_dbname' already exists, using it..."
		# log_info "Database '$test_dbname' already exists, dropping and recreating..."
		# "$psql_bin" -c "DROP DATABASE IF EXISTS $test_dbname;" postgres
	else
		# Create database
		log_info "Creating database '$test_dbname'..."
		"$createdb_bin" "$test_dbname" || log_error "Failed to create database"
	fi

	# Create schema based on user choice
	if [ "$SIMPLE_SCHEMA" = true ]; then
		create_simple_schema "$psql_bin" "$test_dbname"
	else
		create_ecommerce_schema "$psql_bin" "$test_dbname"
	fi

	# Stop server if we started it
	if [ "$is_running" = false ]; then
		log_info "Stopping PostgreSQL server..."
		"$pg_ctl_bin" -D "$data_dir" stop
	fi

	log_success "Test database created successfully"
}

# Update configuration file with paths
update_config_file() {
	local version_name="$1"
	local branch="$2"
	local worktree_dir="$3"
	local initialized="$4"
	local test_db_created="$5"

	local config_path="$PROJECT_ROOT/$CONFIG_FILE"
	local data_dir="$worktree_dir/data"
	local build_dir="$worktree_dir/build"
	local install_dir="$worktree_dir/install"
	local bin_dir="$install_dir/bin"

	# Create or update config file
	log_info "Updating configuration file..."

	# Remove existing section if present
	if [ -f "$config_path" ]; then
		sed -i.bak "/^\[postgres\.$version_name\]/,/^$/d" "$config_path"
		rm -f "$config_path.bak"
	fi

	# Append new configuration
	cat >>"$config_path" <<EOF

[postgres.$version_name]
version = "$branch"
source_dir = "$worktree_dir"
data_dir = "$data_dir"
build_dir = "$build_dir"
install_dir = "$install_dir"
bin_dir = "$bin_dir"
initialized = $initialized
test_db_created = $test_db_created
EOF

	log_success "Configuration updated in $config_path"
}

# Print summary of setup
print_summary() {
	local version_name="$1"
	local branch="$2"
	local worktree_dir="$3"
	local data_dir="$worktree_dir/data"
	local install_dir="$worktree_dir/install"
	local bin_dir="$install_dir/bin"

	echo ""
	echo "======================================================================"
	log_success "PostgreSQL Setup Complete!"
	echo "======================================================================"
	echo ""
	echo "Version:        $version_name ($branch)"
	echo "Source:         $worktree_dir"
	echo "Data Directory: $data_dir"

	if [ "$DO_BUILD" = true ]; then
		echo "Install Directory: $install_dir"
		echo "Binaries:       $bin_dir"
	fi

	if [ "$DO_INIT" = true ]; then
		echo "Database:       Initialized"
	fi

	if [ "$DO_TEST_DATA" = true ]; then
		if [ "$SIMPLE_SCHEMA" = true ]; then
			echo "Test Data:      Simple schema (test_types table)"
		else
			echo "Test Data:      E-commerce schema (5 tables)"
		fi
	fi

	echo "Config File:    $PROJECT_ROOT/$CONFIG_FILE"
	echo ""

	if [ "$DO_BUILD" = true ] && [ "$DO_INIT" = true ]; then
		echo "To start PostgreSQL:"
		echo "  $bin_dir/pg_ctl -D $data_dir -l $data_dir/logfile start"
		echo ""
		echo "To connect to the test database:"
		echo "  $bin_dir/psql test"
		echo ""
		echo "To stop PostgreSQL:"
		echo "  $bin_dir/pg_ctl -D $data_dir stop"
		echo ""
	fi

	echo "======================================================================"
}

#=============================================================================
# Main Script Logic
#=============================================================================

main() {
	# Parse command-line arguments
	while [[ $# -gt 0 ]]; do
		case $1 in
		-b | --branch)
			BRANCH="$2"
			shift 2
			;;
		-B | --build)
			DO_BUILD=true
			shift
			;;
		-i | --init)
			DO_INIT=true
			shift
			;;
		-t | --test-data)
			DO_TEST_DATA=true
			shift
			;;
		-s | --simple-schema)
			SIMPLE_SCHEMA=true
			shift
			;;
		-h | --help)
			usage
			;;
		*)
			echo "Unknown option: $1"
			usage
			;;
		esac
	done

	# Validate options
	if [ "$DO_INIT" = true ] && [ "$DO_BUILD" != true ]; then
		log_error "Option --init requires --build"
	fi

	# Map version to branch
	local git_branch
	git_branch=$(map_version_to_branch "$BRANCH")

	# Normalize for directory name
	local version_name
	version_name=$(normalize_branch_name "$git_branch" "$BRANCH")

	local worktree_dir="$TESTDATA_DIR/postgres-$version_name"

	log_info "Setting up PostgreSQL $version_name ($git_branch)"
	echo ""

	# Execute setup steps
	setup_git_repo
	setup_worktree "$git_branch" "$worktree_dir"

	local initialized="false"
	local test_db_created="false"

	if [ "$DO_BUILD" = true ]; then
		build_postgres "$worktree_dir"
	fi

	if [ "$DO_INIT" = true ]; then
		init_database "$worktree_dir"
		initialized="true"
	fi

	if [ "$DO_TEST_DATA" = true ]; then
		create_test_db "$worktree_dir"
		test_db_created="true"
	fi

	# Update configuration file
	update_config_file "$version_name" "$git_branch" "$worktree_dir" "$initialized" "$test_db_created"

	# Print summary
	print_summary "$version_name" "$git_branch" "$worktree_dir"
}

# Run main function
main "$@"
