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
# splits on python_full_version, so one interpreter leaves the other branches unexamined. The branches are read
# from the lock rather than listed here, so a moved floor or a new split cannot leave one unaudited; a branch
# above every released interpreter (`>= 3.15`) has nothing to run yet. The scanner runs from
# scripts/tools/audit, which has its own lockfile; `--path` names the environment to examine, because a
# locked scanner run from its own project audits itself otherwise.
branches="$(python3 - <<'PY'
import re
lock = open("sdk/python/uv.lock").read()
markers = re.search(r"^resolution-markers = \[(.*?)^\]", lock, re.M | re.S).group(1)
versions = set()
for minor in re.findall(r"python_full_version == '3\.(\d+)\.\*'", markers):
    versions.add(int(minor))
for minor in re.findall(r"python_full_version < '3\.(\d+)'", markers):
    versions.add(int(minor) - 1)
print(" ".join(f"3.{minor}" for minor in sorted(versions)))
PY
)"
[ -n "$branches" ] || { echo "[audit] no resolution branches found in sdk/python/uv.lock" >&2; exit 1; }
for python in $branches; do
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
