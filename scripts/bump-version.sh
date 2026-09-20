#!/usr/bin/env bash
# Keep every checked-in Grafium version source synchronized.
#
# Usage:
#   scripts/bump-version.sh          # increment the patch version
#   scripts/bump-version.sh 1.2.3    # set an explicit version
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
current="$(sed -n 's/^version = "\([0-9]\+\.[0-9]\+\.[0-9]\+\)"/\1/p' "$repo_root/Cargo.toml" | head -1)"

if [[ -z "$current" ]]; then
  echo "error: could not read the workspace version from Cargo.toml" >&2
  exit 1
fi

if [[ $# -gt 1 ]]; then
  echo "usage: scripts/bump-version.sh [MAJOR.MINOR.PATCH]" >&2
  exit 2
fi

if [[ $# -eq 1 ]]; then
  next="$1"
else
  IFS=. read -r major minor patch <<<"$current"
  next="$major.$minor.$((patch + 1))"
fi

if [[ ! "$next" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: version must use MAJOR.MINOR.PATCH format" >&2
  exit 2
fi

if [[ "$next" == "$current" ]]; then
  echo "error: Grafium is already version $next" >&2
  exit 2
fi

sed -i -E "0,/^version = \"[0-9]+\\.[0-9]+\\.[0-9]+\"/s//version = \"$next\"/" \
  "$repo_root/Cargo.toml"
sed -i -E "0,/^version = \"[0-9]+\\.[0-9]+\\.[0-9]+\"/s//version = \"$next\"/" \
  "$repo_root/ui/src-tauri/Cargo.toml"
sed -i -E "0,/\"version\": \"[0-9]+\\.[0-9]+\\.[0-9]+\"/s//\"version\": \"$next\"/" \
  "$repo_root/ui/src-tauri/tauri.conf.json"

npm --prefix "$repo_root/ui" version "$next" --no-git-tag-version --allow-same-version >/dev/null
sed -i -E "/^name = \"(grafium|grafium-core|grafium-tui)\"$/ {
  n
  s/^version = \"[0-9]+\\.[0-9]+\\.[0-9]+\"/version = \"$next\"/
}" "$repo_root/Cargo.lock"

echo "Grafium version: $current -> $next"
