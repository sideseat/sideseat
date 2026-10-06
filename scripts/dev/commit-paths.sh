#!/usr/bin/env bash
# Commit exactly the named paths, whatever else is staged or modified in the working tree.
#
#   scripts/dev/commit-paths.sh -m "fix(scope): subject" [-m "body"] -- path [path...]
#
# For a working tree several agents share. The commit is built in a private index read from HEAD, so it
# cannot sweep in anyone else's staged work, and the hooks run against exactly what is committed. Afterwards
# the shared index is updated for the committed paths only: left alone it would keep the pre-commit blobs,
# and a later `git checkout -- <path>`, which restores from the index, would silently revert the commit.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

messages=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    case "$1" in
        -m) messages+=(-m "$2"); shift 2 ;;
        *) echo "usage: $0 -m <message> [-m <body>] -- <path>..." >&2; exit 2 ;;
    esac
done
[ "${1:-}" = "--" ] && shift
[ ${#messages[@]} -gt 0 ] && [ $# -gt 0 ] || { echo "usage: $0 -m <message> [-m <body>] -- <path>..." >&2; exit 2; }

# Files only: a directory would take every change under it, including another agent's work in progress.
for path in "$@"; do
    if [ -d "$path" ]; then
        echo "[commit-paths] $path is a directory; name the files to commit" >&2
        exit 2
    fi
done

private_index="$(mktemp "${TMPDIR:-/tmp}/sideseat-index.XXXXXX")"
trap 'rm -f "$private_index"' EXIT

GIT_INDEX_FILE="$private_index" git read-tree HEAD
# `-A` so a deleted or renamed path is committed as such.
GIT_INDEX_FILE="$private_index" git add -A -- "$@"
if GIT_INDEX_FILE="$private_index" git diff --cached --quiet; then
    echo "[commit-paths] nothing to commit in the named paths" >&2
    exit 1
fi
GIT_INDEX_FILE="$private_index" git commit "${messages[@]}"

# The shared index now matches the new HEAD for these paths and is untouched everywhere else.
git reset -q -- "$@"
