#!/usr/bin/env bash
# Delete a Cargo build cache once its build is installed.
#
# Keeping build outputs around after every deploy is what kept filling the
# disk, and we can always rebuild. The directory itself stays (it may be a
# symlink to another drive), so the next build lands in the same place.
# Nothing is removed while a Cargo build holds one of the cache's locks, or
# when the directory is not a Cargo cache (neither CACHEDIR.TAG nor
# .rustc_info.json).
#
# Usage: scripts/clear-build-cache.sh <target-dir>
set -euo pipefail

# Cargo's standard tag (https://bford.info/cachedir/).
CACHEDIR_TAG='Signature: 8a477f597d28d172789f06886806bc55
# This file is a cache directory tag created by cargo.
# For information about cache directory tags see https://bford.info/cachedir/'

target="${1:?usage: clear-build-cache.sh <target-dir>}"
if [[ ! -d "$target" ]]; then
  echo "no build cache at $target"
  exit 0
fi
dir="$(cd "$target" && pwd -P)"
if [[ ! -f "$dir/CACHEDIR.TAG" && ! -f "$dir/.rustc_info.json" ]]; then
  echo "warning: $dir is not a Cargo build cache; left alone" >&2
  exit 0
fi
while IFS= read -r -d '' lock; do
  if ! flock -n "$lock" true; then
    echo "warning: a Cargo build is using $dir; its cache was kept" >&2
    exit 0
  fi
done < <(find "$dir" -maxdepth 3 -name '.cargo*lock' -print0)

size="$(du -sh "$dir" 2>/dev/null | cut -f1)"
find "$dir" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
# Cargo writes its tag only when it creates the directory itself, and this
# one stays, so put the tag back for the next clear (and for backup tools).
printf '%s\n' "$CACHEDIR_TAG" > "$dir/CACHEDIR.TAG"
echo "cleared build cache $dir ($size); the next build starts fresh"
