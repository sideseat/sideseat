#!/usr/bin/env bash
# Push HEAD of a shared working tree, with the pre-push gate run on exactly the commit pushed.
#
#   scripts/dev/push-head.sh            push HEAD to origin's main
#
# The pre-push hook runs `make check` on the working tree it is invoked from, so in a tree that holds other
# agents' work in progress it checks something other than the commit. This checks HEAD out in verify-head's
# clean worktree, under verify-head's lock so neither run moves the checkout under the other, and pushes from
# there through `make push`, with the toolchains mise pins (a bare shell finds the system .NET and Node).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checkout="${SIDESEAT_VERIFY_HEAD_DIR:-$repo_root/../$(basename "$repo_root")-head}"
[ -e "$checkout/.git" ] || { echo "[push-head] run 'make verify-head' once to create $checkout" >&2; exit 1; }

lock="$checkout.lock"
until mkdir "$lock" 2>/dev/null; do
    holder="$(cat "$lock/pid" 2>/dev/null || true)"
    if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
        rm -rf "$lock"
        continue
    fi
    echo "[push-head] waiting for the run holding $lock"
    sleep 15
done
echo $$ >"$lock/pid"
trap 'rm -rf "$lock"' EXIT

head="$(git -C "$repo_root" rev-parse HEAD)"
branch="$(git -C "$repo_root" branch --show-current)"
git -C "$checkout" checkout -q --detach --force "$head"
git -C "$checkout" clean -q -fd -e target -e node_modules
cd "$checkout"
echo "[push-head] $(git log --oneline -1)"
export CARGO_TARGET_DIR="$checkout/target"
command -v mise >/dev/null 2>&1 && mise trust -q mise.toml 2>/dev/null || true
if command -v mise >/dev/null 2>&1; then
    mise exec -- make push ARGS="origin $head:refs/heads/${branch:-main}"
else
    make push ARGS="origin $head:refs/heads/${branch:-main}"
fi
