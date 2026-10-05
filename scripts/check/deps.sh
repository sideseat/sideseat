#!/usr/bin/env bash
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root" || exit 1

dry_run="${SIDESEAT_DEPS_CHECK_DRY_RUN:-0}"
failures=0
skipped=0
node_report_index=0
temp_dir="$(mktemp -d)"
trap 'rm -rf -- "$temp_dir"' EXIT

print_command() {
  printf '  '
  printf '%q ' "$@"
  printf '\n'
}

skip_tool() {
  local tool="$1"
  local install_hint="$2"
  echo "[deps-check] SKIPPED: $tool is not installed ($install_hint)" >&2
  skipped=$((skipped + 1))
}

record_failure() {
  local subject="$1"
  echo "[deps-check] FAILED: $subject" >&2
  failures=$((failures + 1))
}

echo "=== Rust workspace (Cargo.toml) ==="
rust_command=(cargo outdated --workspace --root-deps-only --manifest-path Cargo.toml)
if [[ "$dry_run" == "1" ]]; then
  print_command "${rust_command[@]}"
elif ! command -v cargo-outdated >/dev/null 2>&1; then
  skip_tool cargo-outdated "cargo install cargo-outdated"
elif ! "${rust_command[@]}"; then
  record_failure "Rust dependency query"
fi

echo
echo "=== Node.js projects ==="
if [[ "$dry_run" != "1" ]] && ! command -v npm >/dev/null 2>&1; then
  skip_tool npm "install Node.js"
else
  while IFS= read -r -d '' manifest; do
    project="${manifest%/package.json}"
    [[ "$project" == "$manifest" ]] && project="."
    echo "--- $manifest"
    node_command=(npm --prefix "$project" outdated --json)
    if [[ "$dry_run" == "1" ]]; then
      print_command "${node_command[@]}"
      continue
    fi

    node_report_index=$((node_report_index + 1))
    report="$temp_dir/npm-$node_report_index.json"
    npm_status=0
    "${node_command[@]}" >"$report" || npm_status=$?

    render_status=0
    # Keep the JavaScript literal; shell expansion would corrupt its template strings.
    # shellcheck disable=SC2016
    node -e '
const fs = require("node:fs");
const path = process.argv[1];
let report;
try {
  report = JSON.parse(fs.readFileSync(path, "utf8") || "{}");
} catch (error) {
  console.error(`invalid npm report: ${error.message}`);
  process.exit(2);
}
if (report.error) {
  console.error(report.error.summary || report.error.message || JSON.stringify(report.error));
  process.exit(2);
}
const entries = Object.entries(report);
if (entries.length === 0) {
  console.log("up to date");
  process.exit(0);
}
for (const [name, versions] of entries) {
  const current = versions.current ?? "not installed";
  const wanted = versions.wanted ?? "?";
  const latest = versions.latest ?? "?";
  console.log(`${name}: ${current} -> ${wanted} (latest ${latest})`);
}
process.exit(10);
' "$report" || render_status=$?

    if [[ "$render_status" == "10" ]]; then
      continue
    fi
    if [[ "$npm_status" != "0" || "$render_status" != "0" ]]; then
      record_failure "$manifest dependency query"
    fi
  done < <(git ls-files -z -- 'package.json' '**/package.json')
fi

echo
echo "=== Python projects ==="
if [[ "$dry_run" != "1" ]] && ! command -v uv >/dev/null 2>&1; then
  skip_tool uv "install uv"
else
  while IFS= read -r -d '' manifest; do
    project="${manifest%/pyproject.toml}"
    [[ "$project" == "$manifest" ]] && project="."
    echo "--- $manifest"
    python_command=(uv tree --project "$project" --locked --outdated --depth 1)
    if [[ "$dry_run" == "1" ]]; then
      print_command "${python_command[@]}"
    elif ! "${python_command[@]}"; then
      record_failure "$manifest dependency query"
    fi
  done < <(git ls-files -z -- 'pyproject.toml' '**/pyproject.toml')
fi

echo
if ((failures > 0)); then
  echo "[deps-check] $failures dependency query or queries failed; $skipped tool group(s) skipped." >&2
  exit 1
fi
echo "[deps-check] Completed; $skipped tool group(s) skipped."
