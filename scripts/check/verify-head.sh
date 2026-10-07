#!/usr/bin/env bash
# Check that HEAD itself builds and passes, independent of whatever is uncommitted in this working tree.
#
#   scripts/check/verify-head.sh            cargo check of every target at HEAD
#   scripts/check/verify-head.sh --test     plus the repository invariants and the tracked message goldens
#
# A shared working tree holds other people's work in progress, so "it passes here" says nothing about the
# commit. This builds HEAD in a persistent detached worktree beside the repository, with its own target
# directory kept warm between runs, and links the working tree's node_modules so nothing is reinstalled.
set -euo pipefail

# Bash reads a script as it runs, so an edit to this file in the shared tree mid-run would break the run.
# Execute from a private copy instead.
if [ -z "${SIDESEAT_VERIFY_HEAD_COPY:-}" ]; then
    copy="$(mktemp "${TMPDIR:-/tmp}/verify-head.XXXXXX")"
    cp "${BASH_SOURCE[0]}" "$copy"
    SIDESEAT_VERIFY_HEAD_COPY="$copy" SIDESEAT_VERIFY_HEAD_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" \
        exec bash "$copy" "$@"
fi
trap 'rm -f "$SIDESEAT_VERIFY_HEAD_COPY"' EXIT

repo_root="${SIDESEAT_VERIFY_HEAD_ROOT}"
cd "$repo_root"

checkout="${SIDESEAT_VERIFY_HEAD_DIR:-$repo_root/../$(basename "$repo_root")-head}"

# One run at a time: a second run would check out a newer HEAD under the first one's tests. mkdir is atomic,
# so whoever creates the directory holds the lock; a lock whose holder has died is taken over.
lock="$checkout.lock"
until mkdir "$lock" 2>/dev/null; do
    holder="$(cat "$lock/pid" 2>/dev/null || true)"
    if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
        rm -rf "$lock"
        continue
    fi
    echo "[verify-head] waiting for the run holding $lock"
    sleep 15
done
echo $$ >"$lock/pid"
trap 'rm -rf "$lock"; rm -f "$SIDESEAT_VERIFY_HEAD_COPY"' EXIT

head="$(git rev-parse HEAD)"

if [ ! -d "$checkout/.git" ] && [ ! -f "$checkout/.git" ]; then
    git worktree add -q --detach "$checkout" "$head"
    command -v mise >/dev/null 2>&1 && mise trust -q "$checkout/mise.toml" 2>/dev/null || true
fi
git -C "$checkout" checkout -q --detach --force "$head"
git -C "$checkout" clean -q -fd -e target -e node_modules

while IFS= read -r manifest; do
    project="$(dirname "$manifest")"
    if [ -d "$project/node_modules" ] && [ ! -e "$checkout/$project/node_modules" ]; then
        ln -s "$repo_root/$project/node_modules" "$checkout/$project/node_modules"
    fi
done < <(git ls-files '*package.json' | grep -v node_modules)

export CARGO_TARGET_DIR="$checkout/target"
cd "$checkout"
echo "[verify-head] $(git log --oneline -1)"
cargo check --locked --workspace --all-targets
if [ "${1:-}" = "--test" ]; then
    cargo test --locked -q -p sideseat-server --test repository
    MESSAGE_FIXTURES=tracked cargo test --locked -q -p sideseat-server --test message_goldens
fi
echo "[verify-head] HEAD is green"
