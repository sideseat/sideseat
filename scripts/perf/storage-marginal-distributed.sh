#!/usr/bin/env bash
# What one more item costs on ClickHouse: the corpus posted to a live server three times and ten
# (`storage-footprint.py --replay-passes`, storage_passes.py), each table merged in its production parts, each
# signal's stored bytes per item between the two runs; and the metric figure on one run of the derived load, a
# million points. Each figure is held to scripts/perf/storage-marginal-distributed-ceiling.json: its regression
# ceiling, about 2 % above the measured figure since ClickHouse merges are not byte-identical between runs, and
# the product target. About an hour and a half on a laptop; opt-in, before a release.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
work="$(mktemp -d "${TMPDIR:-/tmp}/sideseat-marginal-distributed.XXXXXX")"
trap 'rm -rf "$work"' EXIT
# One instant every run is rebased to, so each puts the corpus in the same month's partitions.
anchor="$(python3 -c 'import time; print(time.time_ns())')"
footprint=("uv" "run" "--locked" "--script" "$root/scripts/perf/storage-footprint.py" "distributed" "--production-parts"
  "--rebase-at" "$anchor")

for passes in 3 10; do
  echo "[storage-marginal] $passes passes"
  "${footprint[@]}" --replay-passes "$passes" --json "$work/x$passes.json" | tail -n 12
done
echo "[storage-marginal] metrics, the derived load"
"${footprint[@]}" --signal metrics --metrics-load --json "$work/metrics.json" | tail -n 6

python3 - "$work" "$root/scripts/perf" <<'PY'
import importlib.util, json, sys
from pathlib import Path

work, perf = Path(sys.argv[1]), Path(sys.argv[2])
sys.path.insert(0, str(perf))
import storage_growth  # noqa: E402

spec = importlib.util.spec_from_file_location("footprint", perf / "storage-footprint.py")
footprint = importlib.util.module_from_spec(spec)
spec.loader.exec_module(footprint)
ceilings = json.loads((perf / "storage-marginal-distributed-ceiling.json").read_text())
whole = {name: json.loads((work / f"{name}.json").read_text()) for name in ("x3", "x10", "metrics")}
runs = {name: run["signals"] for name, run in whole.items()}
# Each run's bytes are attributed afresh, by the owner maps as they are now, not as the run recorded them.
layers = {name: footprint.attribute(run["measured"]) for name, run in whole.items()}
kept = lambda name, s: sum(v for k, v in layers[name][s].items() if k not in footprint.MEDIA_LAYERS)  # noqa: E731
figures = {}
for signal in ("traces", "logs"):
    items = runs["x10"][signal]["items"] - runs["x3"][signal]["items"]
    figures[signal] = (kept("x10", signal) - kept("x3", signal)) / items
figures["metrics"] = runs["metrics"]["metrics"]["stored_excluding_media_per_item"]
failures = []
partitions = [whole[name]["measured"]["analytics_partitions"] for name in ("x3", "x10")]
# Each store apart: a table name both hold would otherwise hide one's growth behind the other's.
grown = {s: runs["x10"][s]["items"] - runs["x3"][s]["items"] for s in ("traces", "logs", "metrics")}
for store, owners in (("analytics_parts", footprint.ANALYTICS_OWNER), ("transactional", footprint.TRANSACTIONAL_OWNER)):
    lines, unowned = storage_growth.growth(
        whole["x3"]["measured"][store], whole["x10"]["measured"][store], grown, owners
    )
    for line in lines:
        print(f"[storage-marginal] growth: {store} {line}")
    failures += unowned
if partitions[0] != partitions[1]:
    failures.append(f"three passes and ten hold different partitions, so the rate carries a partition's fixed cost: {partitions}")
for signal, figure in figures.items():
    print(f"[storage-marginal] {signal}: {figure:.1f} B/item excluding media")
    for kind, limit in ceilings[signal].items():
        if figure > limit:
            failures.append(f"{signal}: {figure:.1f} B/item is above the {kind} ceiling of {limit}")
for failure in failures:
    print(f"[storage-marginal] FAIL: {failure}")
sys.exit(1 if failures else 0)
PY
