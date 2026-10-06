#!/usr/bin/env bash
# Supply-chain audit: advisories, licences, bans and sources for every dependency graph the repository ships.
#
# Not part of `make check`: its answer changes without the code changing (an advisory disclosed today was
# invisible yesterday) and it needs the network. Run it with `make audit` before a release and periodically.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

for tool in cargo-deny cargo-audit; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "[audit] $tool is required: mise install" >&2
        exit 1
    }
done

step() { printf '\n[audit] %s\n' "$*"; }

# Blocking policy for advisories, licences, bans and dependency sources. An accepted advisory belongs in
# deny.toml with its rationale.
step "cargo deny"
cargo deny check

# Advisory-only: `cargo audit` has no per-advisory rationale file, so a finding it reports and `cargo deny`
# does not is information rather than a verdict.
step "cargo audit (advisory)"
cargo audit || echo "[audit] cargo audit reported findings; cargo deny above is the verdict"

for project in web sdk/js examples/javascript docs; do
    step "npm audit ($project)"
    (cd "$project" && npm audit --audit-level=high)
done

# The Python SDK is published. `--all-extras`, because pip-audit examines the installed environment and would
# otherwise never see the optional framework instrumentation. Once per lockfile resolution branch: the lock
# splits on python_full_version, so one interpreter leaves the other branches unexamined. The scanner runs
# from scripts/tools/audit, which has its own lockfile; `--path` names the environment to examine, because a
# locked scanner run from its own project audits itself otherwise.
for python in 3.10 3.12 3.13; do
    step "pip-audit (sdk/python, python $python)"
    (cd sdk/python && uv sync --locked --all-extras --python "$python")
    uv run --locked --project scripts/tools/audit pip-audit \
        --path "$(echo sdk/python/.venv/lib/python*/site-packages)"
done

for tool in scripts/tools/otel-replay scripts/tools/mcp-calculator; do
    step "pip-audit ($tool)"
    (cd "$tool" && uv sync --locked)
    uv run --locked --project scripts/tools/audit pip-audit \
        --path "$(echo "$tool"/.venv/lib/python*/site-packages)"
done

printf '\n[audit] Passed\n'
