#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dotnet_command="${DOTNET_COMMAND:-dotnet}"
project="$repo_root/sdk/dotnet/tests/SideSeat.Tests.csproj"
results_dir="$(mktemp -d)"
trx_file="$results_dir/sideseat.trx"

cleanup() {
  rm -rf -- "$results_dir"
}
trap cleanup EXIT

if ! command -v "$dotnet_command" >/dev/null 2>&1; then
  echo "[test-sdk-dotnet] dotnet was not found: $dotnet_command" >&2
  exit 1
fi

"$dotnet_command" restore "$project" --locked-mode
"$dotnet_command" test "$project" \
  --configuration Release \
  --no-restore \
  --logger "trx;LogFileName=sideseat.trx" \
  --results-directory "$results_dir"

python3 - "$trx_file" <<'PY'
import sys
import xml.etree.ElementTree as ET

trx_file = sys.argv[1]
root = ET.parse(trx_file).getroot()
counters = next(
    (element for element in root.iter() if element.tag.rsplit("}", 1)[-1] == "Counters"),
    None,
)
if counters is None:
    raise SystemExit("[test-sdk-dotnet] TRX has no test counters")

executed = int(counters.attrib.get("executed", "0"))
failed = int(counters.attrib.get("failed", "0"))
minimum = 5
if failed:
    raise SystemExit(f"[test-sdk-dotnet] TRX reports {failed} failed test(s)")
if executed < minimum:
    raise SystemExit(
        f"[test-sdk-dotnet] expected at least {minimum} executed tests, saw {executed}; "
        "the test run was vacuous or coverage regressed"
    )
print(f"[test-sdk-dotnet] verified {executed} executed test(s) from TRX")
PY
