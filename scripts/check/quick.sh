#!/usr/bin/env bash
# The inner developer loop: format, lint, and unit-test only the areas changed relative to a base.
#
#   scripts/check/quick.sh               uncommitted changes on main; on a branch, changes since it forked
#   scripts/check/quick.sh --base REF    changes since REF
#   scripts/check/quick.sh --all         every area, as if everything had changed
#
# The budget is one minute on a warm cache. Anything slower belongs in `make check` or an opt-in
# target, not here. Live model calls and containers never run from this script.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

base=""
all=0
while (($#)); do
    case "$1" in
        --base) base="$2"; shift 2 ;;
        --all) all=1; shift ;;
        -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "quick: unknown argument $1" >&2; exit 2 ;;
    esac
done

if [ -z "$base" ]; then
    branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
    if [ "$branch" = main ] || [ -z "$branch" ]; then
        base=HEAD
    else
        base="$(git merge-base HEAD main 2>/dev/null || echo HEAD)"
    fi
fi

if ((all)); then
    changed="$(git ls-files)"
else
    changed="$( { git diff --name-only "$base" --; git ls-files --others --exclude-standard; } | sort -u)"
fi
# Deleted paths still matter for selecting an area, but cannot be passed to a formatter.
existing() { while IFS= read -r f; do [ -e "$f" ] && printf '%s\n' "$f"; done; }
pick() { grep -E "$1" <<<"$changed" | existing || true; }

if [ -z "$changed" ]; then
    echo "[quick] Nothing changed since $(git rev-parse --short "$base")."
    exit 0
fi

started=$SECONDS
step() { printf '\n[quick] %s\n' "$*"; }

# Free space only, which `df` answers at once: sizing the target with `du` would cost seconds on every
# run. Below the reserve the stale-artifact sweep runs before anything can grow the target further.
free_mb="$(df -Pm "$ROOT" | awk 'NR == 2 {print $4}')"
if [ "${free_mb:-0}" -lt "${DISK_FREE_MIN_MB:-10000}" ]; then
    step "disk: ${free_mb} MB free, below the reserve; reclaiming stale build artifacts"
    bash "$ROOT/scripts/dev/clean-stale.sh"
fi

# --- Rust -------------------------------------------------------------------------------------
# Changed files map to the crate that owns them, test fixtures included; workspace-level inputs
# select every crate.
rust_files="$(grep -E '\.(rs|toml)$|^server/assets/|^server/tests/|^Cargo\.lock$' <<<"$changed" || true)"
if [ -n "$rust_files" ]; then
    crates=()
    workspace=0
    while IFS= read -r f; do
        case "$f" in
            Cargo.toml|Cargo.lock|rust-toolchain.toml|clippy.toml|rustfmt.toml|.cargo/*) workspace=1 ;;
            server/assets/*) crates+=(sideseat-rule-assets sideseat-domain sideseat-server) ;;
            server/crates/*|server/*|sdk/rust/*)
                dir="$(dirname "$f")"
                while [ "$dir" != "." ] && [ ! -f "$dir/Cargo.toml" ]; do dir="$(dirname "$dir")"; done
                [ -f "$dir/Cargo.toml" ] || continue
                name="$(sed -n 's/^name = "\(.*\)"/\1/p' "$dir/Cargo.toml" | head -1)"
                [ -n "$name" ] && crates+=("$name")
                ;;
        esac
    done <<<"$rust_files"

    step "rustfmt"
    # The changed files only: their formatting is all this change can have altered.
    rs_existing="$(grep -E '\.rs$' <<<"$rust_files" | existing || true)"
    if [ -n "$rs_existing" ]; then
        # shellcheck disable=SC2086
        rustfmt --check --edition 2024 $rs_existing
    fi
    if ((workspace)); then
        step "clippy (workspace: a workspace manifest changed)"
        cargo clippy --locked --workspace --all-targets -- -D warnings
        step "tests skipped for workspace-wide changes; run make test-rust"
    elif ((${#crates[@]})); then
        # Word splitting is safe: crate names contain no whitespace. `mapfile` needs bash 4.
        # shellcheck disable=SC2207
        crates=($(printf '%s\n' "${crates[@]}" | sort -u))
        packages=()
        for c in "${crates[@]}"; do packages+=(-p "$c"); done
        step "clippy ${crates[*]}"
        cargo clippy --locked "${packages[@]}" --all-targets -- -D warnings
        # The server's container parity, backup-restore and footprint suites are opt-in targets, so they
        # are neither built nor run here: its offline gates are the message goldens and the repository
        # invariants. Of the goldens, the loop runs the comparison itself - every fixture's expected views
        # and per-fixture invariants; the corpus-wide property tests each re-read every fixture in a
        # process of their own and run in `make test`. One invocation, so nextest runs every selected
        # crate's tests in parallel and cargo builds them in one pass.
        # Unit tests run under `cargo test`: one process per binary instead of nextest's one per test,
        # which on macOS made process start-up most of this step (about 20 s against 3 s for the
        # domain and ingestion crates). The Rust SDK keeps nextest, because its tests each own the
        # process's global providers. Of the server, the loop runs its unit tests and the golden
        # comparison itself; the corpus-wide golden properties and the container suites run in
        # `make test`, and the repository invariants - docs, scripts, layout - only when something
        # other than Rust source changed.
        step "tests ${crates[*]}"
        unit=()
        sdk=0
        server=0
        for c in "${crates[@]}"; do
            case "$c" in
                sideseat) sdk=1 ;;
                sideseat-server) server=1 ;;
                *) unit+=(-p "$c") ;;
            esac
        done
        # One build of every target the runs below need, so cargo compiles them in parallel rather
        # than one invocation after another.
        build=()
        if ((${#unit[@]})); then build+=("${unit[@]}"); fi
        if ((server)); then
            build+=(-p sideseat-server --test message_goldens)
            if grep -qvE '\.rs$' <<<"$changed"; then build+=(--test repository); fi
        fi
        if ((${#build[@]})); then
            cargo test --locked -q --no-run --lib --bins "${build[@]}"
        fi
        # The unit tests run in one invocation over the same packages as the build: cargo unifies
        # features across the packages it is given, so testing a crate apart from the server would
        # compile it a second time with a different feature set.
        units=()
        if ((${#unit[@]})); then units+=("${unit[@]}"); fi
        if ((server)); then units+=(-p sideseat-server); fi
        # The golden comparison and the unit tests each keep about one core busy, so with every
        # target built they run side by side; the comparison's output is held back until it ends.
        golden=""
        if ((server)); then
            golden_log="$(mktemp)"
            cargo test --locked -q -p sideseat-server --test message_goldens -- --exact message_goldens \
                >"$golden_log" 2>&1 &
            golden=$!
        fi
        if ((${#units[@]})); then
            cargo test --locked -q --lib --bins "${units[@]}" || {
                status=$?
                if [ -n "$golden" ]; then kill "$golden" 2>/dev/null || true; rm -f "$golden_log"; fi
                exit "$status"
            }
        fi
        if [ -n "$golden" ]; then
            golden_status=0
            wait "$golden" || golden_status=$?
            if ((golden_status)); then cat "$golden_log" >&2; fi
            rm -f "$golden_log"
            ((golden_status == 0)) || exit "$golden_status"
            if grep -qvE '\.rs$' <<<"$changed"; then
                cargo test --locked -q -p sideseat-server --test repository
            fi
        fi
        if ((sdk)); then
            command -v cargo-nextest >/dev/null 2>&1 || {
                echo "[quick] cargo-nextest is required: mise install (or cargo install cargo-nextest --locked)" >&2
                exit 1
            }
            cargo nextest run --locked --no-tests=pass -p sideseat
        fi
    fi
fi

# --- TypeScript -------------------------------------------------------------------------------
node_project() {
    local dir="$1" pattern="$2"
    local files
    files="$(pick "^$dir/.*\.(ts|tsx|js|mjs|css|json)$")"
    [ -n "$files" ] || return 0
    [ -d "$dir/node_modules" ] || { echo "[quick] $dir/node_modules missing; run make setup" >&2; exit 1; }
    local rel
    rel="$(sed "s#^$dir/##" <<<"$files")"
    step "$dir: prettier, eslint, typecheck, tests"
    (
        cd "$dir"
        # shellcheck disable=SC2086
        npx --no-install prettier --check $rel
        local lintable
        lintable="$(grep -E "$pattern" <<<"$rel" || true)"
        # shellcheck disable=SC2086
        [ -z "$lintable" ] || npx --no-install eslint --max-warnings 0 $lintable
        npx --no-install tsc -b --noEmit 2>/dev/null || npx --no-install tsc --noEmit
        if grep -q '"vitest"' package.json; then
            # shellcheck disable=SC2086
            npx --no-install vitest related --run --passWithNoTests $rel
        fi
    )
}
node_project web '\.(ts|tsx)$'
node_project sdk/js '\.(ts|tsx)$'
node_project examples/javascript '\.(ts|tsx)$'

# --- Python -----------------------------------------------------------------------------------
py_files="$(pick '\.py$')"
if [ -n "$py_files" ]; then
    step "ruff"
    # shellcheck disable=SC2086
    uv run --locked ruff format --check $py_files
    # shellcheck disable=SC2086
    uv run --locked ruff check $py_files
fi
if grep -qE '^sdk/python/' <<<"$changed"; then
    step "sdk/python: mypy, pytest"
    (cd sdk/python && uv run --locked mypy src && uv run --locked pytest -q -x)
fi

# --- .NET -------------------------------------------------------------------------------------
if grep -qE '^sdk/dotnet/' <<<"$changed"; then
    step "sdk/dotnet: tests"
    DOTNET_COMMAND="${DOTNET:-dotnet}" ./scripts/test/dotnet-sdk.sh
fi

# --- Shell ------------------------------------------------------------------------------------
sh_files="$(pick '\.sh$|^\.githooks/')"
if [ -n "$sh_files" ]; then
    step "shell syntax"
    while IFS= read -r f; do bash -n "$f"; done <<<"$sh_files"
    if command -v shellcheck >/dev/null 2>&1; then
        # shellcheck disable=SC2086
        shellcheck -S warning $sh_files
    fi
fi

printf '\n[quick] Passed in %ss.\n' "$((SECONDS - started))"
