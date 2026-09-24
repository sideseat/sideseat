#!/usr/bin/env bash
set -euo pipefail

release_type="${1:-}"
case "$release_type" in
  patch | minor | major) ;;
  *)
    echo "Usage: $0 {patch|minor|major}" >&2
    exit 2
    ;;
esac

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Error: release requires a clean working tree" >&2
  exit 1
fi

branch="$(git symbolic-ref --quiet --short HEAD)" || {
  echo "Error: release requires a checked-out branch" >&2
  exit 1
}
git remote get-url origin >/dev/null
git fetch --quiet --tags origin

remote_branch="refs/remotes/origin/$branch"
if ! git rev-parse --verify "$remote_branch" >/dev/null 2>&1; then
  echo "Error: origin has no branch named $branch" >&2
  exit 1
fi
if ! git merge-base --is-ancestor "$remote_branch" HEAD; then
  echo "Error: $branch is behind or diverged from origin/$branch; update it before releasing" >&2
  exit 1
fi

echo "[release] Running pre-release checks..."
make --no-print-directory check

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Error: pre-release checks modified the working tree" >&2
  exit 1
fi

echo "[release] Bumping $release_type version..."
make --no-print-directory bump TYPE="$release_type"

new_version="$(node -p "require('./cli/package.json').version")"
tag="v$new_version"
if git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
  echo "Error: tag $tag already exists" >&2
  exit 1
fi

echo "[release] Verifying generated version changes..."
make --no-print-directory version-check
make --no-print-directory check

version_files=(
  Cargo.toml
  Cargo.lock
  cli/package.json
  cli/platforms/platform-*/package.json
)
git add -- "${version_files[@]}"

if ! git diff --quiet || [[ -n "$(git ls-files --others --exclude-standard)" ]]; then
  echo "Error: release generated changes outside the version files" >&2
  git status --short >&2
  exit 1
fi
if git diff --cached --quiet; then
  echo "Error: version bump produced no changes" >&2
  exit 1
fi
git diff --cached --check

echo "[release] Committing $tag..."
git commit -m "Release $tag"
git tag --annotate "$tag" --message "Release $tag"

echo "[release] Pushing $branch and $tag atomically..."
git push --atomic origin \
  "HEAD:refs/heads/$branch" \
  "refs/tags/$tag:refs/tags/$tag"

echo "[release] Published $tag. Build with 'make build-cli', sign with 'make sign-release SIGN_IDENTITY=\"Developer ID Application: Name (TEAMID)\"', then run 'make build-release && make publish-release'."
