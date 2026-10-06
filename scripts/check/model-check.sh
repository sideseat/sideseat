#!/usr/bin/env bash
# Model-check one TLA+ specification with hard caps, so a model that outgrows its bounds fails instead of
# filling the disk or running for hours.
#
#   scripts/check/model-check.sh JAR SPEC_DIR SPEC
#
# TLC keeps its state queue and fingerprints on disk, and a model whose bounds are too loose can write tens of
# gigabytes in minutes - enough to fill a developer's disk and take every other build down with it. So each run
# gets its own state directory under SPEC_DIR/states, a watchdog kills TLC when that directory passes
# MODEL_CHECK_MAX_MB or the run passes MODEL_CHECK_MAX_SECONDS, and the directory is removed whatever the
# outcome. The defaults are well above what the committed models need (the largest writes a few hundred
# megabytes; OrderGraph is the slowest at about ten minutes, almost all of it enumerating initial states) and
# well below what hurts. MODEL_CHECK_HEAP caps the heap when set.
set -euo pipefail

jar=$1
dir=$2
spec=$3
max_mb=${MODEL_CHECK_MAX_MB:-2048}
max_seconds=${MODEL_CHECK_MAX_SECONDS:-1800}
# TLC's own default (a quarter of the machine's memory) unless asked otherwise: capping it low makes TLC
# spill to disk instead, which is slower and is what the disk cap is for.
heap=${MODEL_CHECK_HEAP:-}

states="$dir/states/$spec"
log=$(mktemp)
cleanup() {
	rm -rf "$states" "$log"
	rmdir "$dir/states" 2>/dev/null || true
	rm -f "$dir/${spec}"_TTrace_*.tla "$dir/${spec}"_TTrace_*.bin
}
trap cleanup EXIT
mkdir -p "$states"

(cd "$dir" && exec java -XX:+UseParallelGC ${heap:+-Xmx$heap} -cp "$jar" tlc2.TLC \
	-workers auto -metadir "states/$spec" -config "$spec.cfg" "$spec.tla") >"$log" 2>&1 &
tlc=$!

reason=""
start=$SECONDS
while kill -0 "$tlc" 2>/dev/null; do
	used=$(du -sm "$states" 2>/dev/null | cut -f1)
	if [ "${used:-0}" -gt "$max_mb" ]; then
		reason="its state passed ${max_mb} MB"
	elif [ $((SECONDS - start)) -gt "$max_seconds" ]; then
		reason="it ran longer than ${max_seconds} s"
	fi
	if [ -n "$reason" ]; then
		kill "$tlc" 2>/dev/null || true
		break
	fi
	sleep 1
done
wait "$tlc" 2>/dev/null || true

if [ -n "$reason" ]; then
	echo "FAILED: stopped because $reason; tighten the model's bounds"
	tail -5 "$log"
	exit 1
fi
if grep -q "Model checking completed. No error has been found" "$log"; then
	grep -oE "[0-9]+ distinct states found" "$log" | tail -1
	exit 0
fi
echo "FAILED"
tail -25 "$log"
exit 1
