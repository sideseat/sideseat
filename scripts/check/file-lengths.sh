#!/usr/bin/env bash
set -euo pipefail

readonly MAX_LINES=1000

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

mode="${1:---worktree}"
case "$mode" in
    --worktree | --cached) ;;
    *)
        echo "Usage: $0 [--worktree|--cached]" >&2
        exit 2
        ;;
esac

is_checked_source() {
    case "$1" in
        Makefile | \
        *.rs | *.py | \
        *.ts | *.tsx | *.js | *.jsx | *.mjs | *.cjs | \
        *.cs | \
        *.sh | *.awk | \
        *.css | *.scss | *.astro | \
        *.toml | *.yaml | *.yml | *.hcl | *.mk | \
        *.tla | *.html | *.tmpl | \
        server/assets/rules/*.json)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

line_count() {
    if [ "$mode" = "--cached" ]; then
        git show ":$1" | awk 'END { print NR + 0 }'
    else
        awk 'END { print NR + 0 }' "$1"
    fi
}

candidate_files() {
    if [ "$mode" = "--cached" ]; then
        git diff --cached --name-only --diff-filter=ACMR -z
    else
        git ls-files -z
    fi
}

violations=0
while IFS= read -r -d '' file; do
    is_checked_source "$file" || continue
    lines="$(line_count "$file")"
    if [ "$lines" -gt "$MAX_LINES" ]; then
        printf '%6d %s\n' "$lines" "$file" >&2
        violations=$((violations + 1))
    fi
done < <(candidate_files)

if [ "$violations" -ne 0 ]; then
    echo "Error: $violations tracked source file(s) exceed the hard ${MAX_LINES}-line limit." >&2
    exit 1
fi

if [ "$mode" = "--cached" ]; then
    echo "[file-lengths] All staged source files are at most ${MAX_LINES} lines"
else
    echo "[file-lengths] All tracked source files are at most ${MAX_LINES} lines"
fi
