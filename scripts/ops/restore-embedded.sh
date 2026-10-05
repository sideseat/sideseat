#!/usr/bin/env bash
#
# Restore a backup created by backup-embedded.sh into an empty data directory.
# The restore marker is written first and intentionally remains until
# `sideseat system restore-repair` succeeds.

set -euo pipefail

usage() {
  echo "Usage: $0 BACKUP_DIR DATA_DIR" >&2
}

fail() {
  echo "restore-embedded: $*" >&2
  exit 1
}

if [[ $# -ne 2 ]]; then
  usage
  exit 2
fi

backup_dir=${1%/}
data_dir=${2%/}
payload="$backup_dir/payload"

[[ -n "$backup_dir" && "$backup_dir" != "/" ]] || fail "refusing unsafe backup directory: $backup_dir"
[[ -n "$data_dir" && "$data_dir" != "/" ]] || fail "refusing unsafe data directory: $data_dir"
[[ -f "$backup_dir/BACKUP-METADATA" ]] || fail "backup metadata not found"
[[ -f "$backup_dir/MANIFEST.sha256" ]] || fail "backup checksum manifest not found"
[[ -f "$payload/sqlite/sideseat.db" ]] || fail "backup SQLite database not found"
[[ -f "$payload/duckdb/sideseat.duckdb" ]] || fail "backup DuckDB database not found"

if command -v sha256sum >/dev/null; then
  (cd "$backup_dir" && sha256sum -c MANIFEST.sha256)
elif command -v shasum >/dev/null; then
  (cd "$backup_dir" && shasum -a 256 -c MANIFEST.sha256)
else
  fail "sha256sum or shasum is required"
fi

if [[ -d "$data_dir" ]] && [[ -n "$(find "$data_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
  fail "restore destination must be empty: $data_dir"
fi
mkdir -p "$data_dir"

# Create the interlock before exposing any restored store. A partial copy is
# therefore fail-closed if the operator accidentally attempts to start SideSeat.
printf '%s\n' "Restore repair is required before SideSeat may serve this data." \
  >"$data_dir/.restore-pending"
cp -R "$payload/." "$data_dir/"
sync

echo "Embedded files restored to: $data_dir"
echo "The restore marker is active. Run repair with the same SideSeat configuration:"
printf '  SIDESEAT_DATA_DIR=%q sideseat --config /path/to/sideseat.json system restore-repair --report /path/to/restore-report.json\n' "$data_dir"
