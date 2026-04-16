#!/usr/bin/env bash

# pgbackrest Backup Script for PostgreSQL 18
# Supports full, incremental, and differential backups to a local folder
# All backups are plain (uncompressed)

set -e

# Color codes
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PG_DIR="$PROJECT_ROOT/testdata/postgres-pg18"
PG_DATA="$PG_DIR/data"
PG_BIN="$PG_DIR/install/bin"
PGBACKREST_CONF="$PG_DIR/pgbackrest.conf"
BACKUP_DIR="$PG_DIR/backups"
STANZA="pg18"

PGBACKREST="/opt/homebrew/bin/pgbackrest"

log_info()    { echo -e "${BLUE}[INFO]${NC} $*"; }
log_success() { echo -e "${GREEN}[OK]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $*"; }
log_error()   { echo -e "${RED}[ERROR]${NC} $*" >&2; exit 1; }

pgbr() {
	"$PGBACKREST" --config="$PGBACKREST_CONF" --stanza="$STANZA" "$@"
}

usage() {
	cat <<EOF
Usage: $0 <command>

Commands:
  setup    Configure PostgreSQL and pgbackrest, create stanza
  full     Run a full backup (plain, uncompressed)
  incr     Run an incremental backup (changes since last backup)
  diff     Run a differential backup (changes since last full)
  info     Show backup information
  restore  Restore from backup (use -t <dir> for target directory)

Examples:
  $0 setup          # First-time setup
  $0 full           # Take a full backup
  $0 incr           # Take an incremental backup
  $0 info           # List all backups
  $0 restore -t /tmp/pg-restore   # Restore to a directory
EOF
	exit 0
}

# Check prerequisites
check_prereqs() {
	if [ ! -x "$PGBACKREST" ]; then
		log_error "pgbackrest not found at $PGBACKREST. Install with: brew install pgbackrest"
	fi
	if [ ! -d "$PG_DATA" ]; then
		log_error "PostgreSQL data directory not found at $PG_DATA"
	fi
	if [ ! -x "$PG_BIN/pg_ctl" ]; then
		log_error "pg_ctl not found at $PG_BIN/pg_ctl"
	fi
}

# Run pg_ctl from the PG directory (handles relative -D paths in postmaster.opts)
pg_ctl_cmd() {
	(cd "$PG_DIR" && "$PG_BIN/pg_ctl" -D "$PG_DATA" "$@")
}

# Check if PostgreSQL is running
pg_is_running() {
	pg_ctl_cmd status &>/dev/null
}

# Setup: configure PG, create dirs, create stanza
cmd_setup() {
	check_prereqs
	log_info "Setting up pgbackrest for PostgreSQL 18..."

	# Create backup directories
	mkdir -p "$BACKUP_DIR/log" "$BACKUP_DIR/lock"
	log_success "Backup directories created at $BACKUP_DIR"

	# Configure PostgreSQL for WAL archiving
	log_info "Configuring PostgreSQL for WAL archiving..."

	local pg_conf="$PG_DATA/postgresql.conf"
	local archive_cmd="$PGBACKREST --config=$PGBACKREST_CONF --stanza=$STANZA archive-push %p"

	# Check if already configured
	if grep -q "^archive_mode = on" "$pg_conf" 2>/dev/null; then
		log_info "archive_mode already configured"
	else
		# Append archive settings
		cat >>"$pg_conf" <<EOF

# pgbackrest WAL archiving (added by pgbackrest-backup.sh)
wal_level = replica
archive_mode = on
archive_command = '$archive_cmd'
EOF
		log_success "PostgreSQL archive settings added to postgresql.conf"
	fi

	# Restart PostgreSQL to apply changes
	if pg_is_running; then
		log_info "Restarting PostgreSQL to apply config changes..."
		pg_ctl_cmd restart -l "$PG_DATA/logfile"
		sleep 2
		log_success "PostgreSQL restarted"
	else
		log_info "Starting PostgreSQL..."
		pg_ctl_cmd start -l "$PG_DATA/logfile"
		sleep 2
		log_success "PostgreSQL started"
	fi

	# Verify archive settings are active
	local current_archive_mode
	current_archive_mode=$("$PG_BIN/psql" -tAc "SHOW archive_mode;" postgres 2>/dev/null || echo "unknown")
	if [ "$current_archive_mode" = "on" ]; then
		log_success "archive_mode is active"
	else
		log_warn "archive_mode is '$current_archive_mode' — may need a restart"
	fi

	# Create stanza
	log_info "Creating pgbackrest stanza '$STANZA'..."
	pgbr stanza-create
	log_success "Stanza '$STANZA' created"

	# Verify
	log_info "Running stanza check..."
	pgbr check
	log_success "Setup complete! You can now run: $0 full"
}

# Full backup
cmd_full() {
	check_prereqs
	if [ ! -f "$PGBACKREST_CONF" ]; then
		log_error "pgbackrest not configured. Run '$0 setup' first."
	fi

	log_info "Starting full backup..."
	pgbr backup --type=full
	log_success "Full backup completed"
	echo ""
	pgbr info
}

# Incremental backup
cmd_incr() {
	check_prereqs
	if [ ! -f "$PGBACKREST_CONF" ]; then
		log_error "pgbackrest not configured. Run '$0 setup' first."
	fi

	log_info "Starting incremental backup..."
	pgbr backup --type=incr
	log_success "Incremental backup completed"
	echo ""
	pgbr info
}

# Differential backup
cmd_diff() {
	check_prereqs
	if [ ! -f "$PGBACKREST_CONF" ]; then
		log_error "pgbackrest not configured. Run '$0 setup' first."
	fi

	log_info "Starting differential backup..."
	pgbr backup --type=diff
	log_success "Differential backup completed"
	echo ""
	pgbr info
}

# Show backup info
cmd_info() {
	check_prereqs
	pgbr info
}

# Restore from backup
cmd_restore() {
	check_prereqs
	local target_dir=""

	# Parse restore-specific args
	while [[ $# -gt 0 ]]; do
		case $1 in
		-t | --target)
			target_dir="$2"
			shift 2
			;;
		*)
			log_error "Unknown restore option: $1"
			;;
		esac
	done

	if [ -z "$target_dir" ]; then
		log_error "Restore requires a target directory. Usage: $0 restore -t /path/to/restore"
	fi

	if [ -d "$target_dir" ] && [ "$(ls -A "$target_dir" 2>/dev/null)" ]; then
		log_error "Target directory '$target_dir' is not empty. Use an empty or non-existent directory."
	fi

	mkdir -p "$target_dir"

	log_info "Restoring to $target_dir..."
	pgbr restore --pg1-path="$target_dir"
	log_success "Restore completed to $target_dir"
}

# Main
case "${1:-}" in
setup)
	cmd_setup
	;;
full)
	cmd_full
	;;
incr)
	cmd_incr
	;;
diff)
	cmd_diff
	;;
info)
	cmd_info
	;;
restore)
	shift
	cmd_restore "$@"
	;;
-h | --help | "")
	usage
	;;
*)
	echo "Unknown command: $1"
	usage
	;;
esac
