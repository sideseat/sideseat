#!/usr/bin/env bash
#
# Create a checkpointed, checksummed backup of an embedded SideSeat data directory.
# SideSeat must be stopped: DuckDB permits only one writing process, and a stop gives
# SQLite, DuckDB, and the filesystem blob tree one operational boundary.

set -euo pipefail

usage() {
  echo "Usage: $0 DATA_DIR BACKUP_DIR" >&2
  echo "Requires: sqlite3, duckdb, and sha256sum or shasum." >&2
}

fail() {
  echo "backup-embedded: $*" >&2
  exit 1
}

if [[ $# -ne 2 ]]; then
  usage
  exit 2
fi

data_dir=${1%/}
backup_dir=${2%/}
sqlite_db="$data_dir/sqlite/sideseat.db"
duckdb_db="$data_dir/duckdb/sideseat.duckdb"

[[ -n "$data_dir" && "$data_dir" != "/" ]] || fail "refusing unsafe data directory: $data_dir"
[[ -n "$backup_dir" && "$backup_dir" != "/" ]] || fail "refusing unsafe backup directory: $backup_dir"
[[ -f "$sqlite_db" ]] || fail "SQLite database not found: $sqlite_db"
[[ -f "$duckdb_db" ]] || fail "DuckDB database not found: $duckdb_db"
[[ ! -e "$data_dir/.restore-pending" ]] ||
  fail "data directory is pending restore repair; repair it before taking a new backup"
[[ ! -e "$backup_dir" ]] || fail "backup destination already exists: $backup_dir"
command -v sqlite3 >/dev/null || fail "sqlite3 is required"
command -v duckdb >/dev/null || fail "duckdb is required"

if command -v sha256sum >/dev/null; then
  checksum_command=sha256sum
elif command -v shasum >/dev/null; then
  checksum_command="shasum -a 256"
else
  fail "sha256sum or shasum is required"
fi

backup_parent=$(dirname "$backup_dir")
backup_name=$(basename "$backup_dir")
mkdir -p "$backup_parent"
work_dir=$(mktemp -d "$backup_parent/.${backup_name}.incomplete.XXXXXX")
complete=0
on_exit() {
  if [[ $complete -eq 0 ]]; then
    echo "backup-embedded: incomplete backup retained for inspection: $work_dir" >&2
  fi
}
trap on_exit EXIT

# These commands also fail when a running SideSeat process owns DuckDB. SQLite's
# TRUNCATE checkpoint must report zero busy readers and zero remaining WAL frames.
sqlite_checkpoint=$(sqlite3 -batch "$sqlite_db" "PRAGMA wal_checkpoint(TRUNCATE);")
[[ "$sqlite_checkpoint" == "0|0|0" ]] ||
  fail "SQLite checkpoint was incomplete ($sqlite_checkpoint); stop all SideSeat processes"
[[ "$(sqlite3 -batch "$sqlite_db" "PRAGMA integrity_check;")" == "ok" ]] ||
  fail "SQLite integrity_check did not return ok"
duckdb "$duckdb_db" -c "CHECKPOINT;" >/dev/null

mkdir -p "$work_dir/payload/sqlite" "$work_dir/payload/duckdb"
cp -p "$sqlite_db" "$work_dir/payload/sqlite/sideseat.db"
cp -p "$duckdb_db" "$work_dir/payload/duckdb/sideseat.duckdb"

if [[ -d "$data_dir/files" ]]; then
  mkdir -p "$work_dir/payload/files"
  cp -R "$data_dir/files/." "$work_dir/payload/files/"
fi

# File-backed secrets and the cached price catalogue live beside the stores.
# files_temp and debug are intentionally excluded: neither is durable state.
for filename in secrets.json model_prices.json; do
  if [[ -f "$data_dir/$filename" ]]; then
    cp -p "$data_dir/$filename" "$work_dir/payload/$filename"
  fi
done

{
  echo "format=1"
  echo "created_at_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "source_data_dir=$data_dir"
} >"$work_dir/BACKUP-METADATA"

(
  cd "$work_dir"
  while IFS= read -r relative_path; do
    if [[ "$checksum_command" == "sha256sum" ]]; then
      digest=$(sha256sum "$relative_path" | awk '{print $1}')
    else
      digest=$(shasum -a 256 "$relative_path" | awk '{print $1}')
    fi
    printf '%s  %s\n' "$digest" "$relative_path"
  done < <(find payload -type f -print | LC_ALL=C sort)
) >"$work_dir/MANIFEST.sha256"

mv "$work_dir" "$backup_dir"
complete=1
trap - EXIT
echo "Embedded backup complete: $backup_dir"
