#!/usr/bin/env bash
# Model-check a specification's mutants: each line of SPEC_DIR/SPEC.mutants overrides some of the .cfg's
# constants and names the invariant TLC must then report violated.
#
#   scripts/check/model-mutants.sh JAR SPEC_DIR SPEC
#
#   # comment
#   Drain = FALSE -> AckedReadable
#   Lease = FALSE, CollectorsStopped = FALSE -> AckedReadable
#
# A model that only passes says nothing about whether its guards are needed; these runs say it of each guard,
# every time, so a guard weakened in the model - or an invariant that stopped seeing what it claims - fails the
# gate rather than going unnoticed. Each mutant runs under `model-check.sh`'s caps.
set -euo pipefail
jar=$1
dir=$2
spec=$3
mutants="$dir/$spec.mutants"
[ -f "$mutants" ] || exit 0
failed=0
n=0
config=""
# A run interrupted, or failing before its own cleanup, must not leave a configuration that pairs with no
# specification behind: the next `harden-spec` would refuse it.
cleanup() {
	[ -z "$config" ] || rm -f "$dir/$config" "$dir/$config.bak"
}
trap cleanup EXIT
while IFS= read -r line || [ -n "$line" ]; do
	case "$line" in '' | '#'*) continue ;; esac
	n=$((n + 1))
	overrides=${line%%->*}
	expected=$(echo "${line##*->}" | tr -d ' ')
	config="${spec}_mutant_$n.cfg"
	cp "$dir/$spec.cfg" "$dir/$config"
	IFS=',' read -ra pairs <<<"$overrides"
	for pair in "${pairs[@]}"; do
		name=$(echo "${pair%%=*}" | tr -d ' ')
		value=$(echo "${pair#*=}" | sed 's/^ *//; s/ *$//')
		if ! grep -qE "^[[:space:]]+$name = " "$dir/$config"; then
			echo "FAILED: $spec.mutants line $n overrides $name, which $spec.cfg does not set"
			failed=1
		fi
		sed -i.bak -E "s|^([[:space:]]+)$name = .*$|\\1$name = $value|" "$dir/$config"
	done
	rm -f "$dir/$config.bak"
	printf "[harden-spec]   %-40s " "$(echo "$overrides" | sed 's/ *$//')"
	./scripts/check/model-check.sh "$jar" "$dir" "$spec" "$config" "$expected" || failed=1
	cleanup
	config=""
done <"$mutants"
exit $failed
