#!/usr/bin/env bash
# Install a locally-built Grafium into ~/.local for day-to-day use.
#
# Exists because deploying by hand kept drifting: the binary got copied to
# ~/.local/bin while the llama.cpp/ggml shared objects it dynamically links
# against were left behind from an older build. Same soname, different code —
# so it still *links* and starts, and the breakage only shows up later as
# subtly wrong inference behaviour. Copying both together, from one build
# directory into an immutable application bundle is the point of this script.
#
# Usage:  scripts/deploy-local.sh [build-dir]
#   build-dir defaults to the repo's own target/release.
#   GRAFIUM_KEEP_BUILDS sets how many installed builds are kept (default 3).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_dir="${1:-$repo_root/target/release}"
bin_dir="$HOME/.local/bin"
lib_dir="$HOME/.local/lib"
app_lib_dir="$lib_dir/grafium"

binary="$build_dir/grafium"
if [[ ! -x "$binary" ]]; then
  echo "error: no built binary at $binary" >&2
  echo "hint: cargo build --release -p grafium --features <...>" >&2
  exit 1
fi

# Refuse to install a binary older than the frontend it is supposed to contain.
#
# The UI is embedded at compile time, so a binary built before the last
# `npm run build` ships the *previous* interface. Nothing about that looks
# wrong: the build succeeds, the app starts, and it is simply the old UI —
# which reads as "my change did nothing" and sends you hunting a bug that
# isn't there.
dist_dir="$repo_root/ui/dist"
if [[ -d "$dist_dir" ]]; then
  newer="$(find "$dist_dir" -type f -newer "$binary" -print -quit 2>/dev/null || true)"
  if [[ -n "$newer" ]]; then
    echo "error: $dist_dir is newer than $binary" >&2
    echo "       the binary embeds the frontend, so this would install a stale UI" >&2
    echo "hint:  cargo build --release -p grafium   # after npm run build" >&2
    exit 1
  fi
fi

mkdir -p "$bin_dir" "$app_lib_dir"

# Refuse to install a binary compiled from a different revision than the
# checkout it is being deployed from.
#
# This is the counterpart to the frontend check above, for the same class of
# invisible failure. Cargo will happily leave an older `target/release/grafium`
# in place when a rebuild is skipped or fails after the previous one
# succeeded, and nothing about the resulting deploy looks wrong — you get a
# working app that is simply not the code you just wrote, which reads as "my
# change did nothing".
#
# The binary reports its own revision via `--version` (see
# `ui/src-tauri/build.rs`), so this compares what is actually inside it
# against HEAD rather than trusting timestamps.
head_sha=""
version_line=""
if git -C "$repo_root" rev-parse --git-dir >/dev/null 2>&1; then
  head_sha="$(git -C "$repo_root" rev-parse --short=7 HEAD)"

  # A binary built before stamping existed does not understand `--version`
  # and would try to open its GUI instead, so cap how long this can block.
  runner=(env "LD_LIBRARY_PATH=$build_dir${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}")
  command -v timeout >/dev/null 2>&1 && runner=(timeout 20s "${runner[@]}")
  version_line="$("${runner[@]}" "$binary" --version 2>/dev/null </dev/null || true)"
  built_sha="$(sed -n 's/.*commit \([0-9a-f]\{7,\}\).*/\1/p' <<<"$version_line")"

  if [[ -z "$built_sha" ]]; then
    echo "warning: $binary does not report a revision; cannot verify it is current" >&2
  elif [[ "$built_sha" != "$head_sha" ]]; then
    echo "error: binary was built from commit $built_sha, but this checkout is at $head_sha" >&2
    echo "       installing it would deploy code that is not what you have checked out" >&2
    echo "hint:  cd ui && npm run build && cd .. && cargo build --release -p grafium" >&2
    exit 1
  fi

  if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
    echo "warning: working tree has uncommitted changes; the build may not match commit $head_sha" >&2
  fi

  # Being in sync with the *checkout* still leaves you behind everyone else.
  # Note this reads the local remote-tracking ref, which a `git fetch` has to
  # have updated to mean anything.
  if upstream="$(git -C "$repo_root" rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null)"; then
    behind="$(git -C "$repo_root" rev-list --count 'HEAD..@{upstream}' 2>/dev/null || echo 0)"
    if [[ "$behind" -gt 0 ]]; then
      echo "warning: HEAD is $behind commit(s) behind $upstream — you are deploying old code" >&2
      echo "         (run 'git fetch' first; this compares against the last fetched state)" >&2
    fi
  fi
fi

# Each build is immutable. Never overwrite mapped native libraries beneath a
# running process, or pair its old executable with a new library generation.
stage="$(mktemp -d "$app_lib_dir/build.XXXXXXXX")"
cp "$binary" "$stage/grafium-bin"
chmod +x "$stage/grafium-bin"
shopt -s nullglob
copied=0
for so in "$build_dir"/lib{ggml,ggml-base,ggml-cpu,ggml-vulkan,ggml-cuda,llama,llama-common}.so*; do
  # Dereference build-cache links: an installed build must survive removal of
  # the source worktree or its target directory.
  cp -L "$so" "$stage/"
  copied=$((copied + 1))
done
shopt -u nullglob
if [[ $copied -eq 0 ]]; then
  echo "error: no native libraries found; existing installation is unchanged" >&2
  exit 1
fi

if command -v ldd >/dev/null 2>&1; then
  dependencies="$(LD_LIBRARY_PATH="$stage" ldd "$stage/grafium-bin")"
  if missing="$(grep "not found" <<<"$dependencies")"; then
    echo "error: unresolved staged libraries; existing installation is unchanged:" >&2
    echo "$missing" >&2
    exit 1
  fi
  while IFS= read -r dependency; do
    if [[ "$dependency" =~ ^[[:space:]]*lib(ggml|llama)[^[:space:]]*[[:space:]] ]]; then
      if [[ "$dependency" != *"=> $stage/"* ]]; then
        echo "error: native library resolved outside the staged build: $dependency" >&2
        exit 1
      fi
    fi
  done <<<"$dependencies"
fi

staged_version="$(timeout 20s env LD_LIBRARY_PATH="$stage" "$stage/grafium-bin" --version </dev/null)"
if [[ -n "$version_line" && "$staged_version" != "$version_line" ]]; then
  echo "error: staged binary identity differs; existing installation is unchanged" >&2
  exit 1
fi

# Preserve and verify the old launchers/binary before switching either entry
# point. Legacy flat libraries remain untouched; older build dirs are pruned
# only after the switch.
backup="$(mktemp -d "$app_lib_dir/backup.XXXXXXXX")"
for name in grafium grafium-bin; do
  if [[ -e "$bin_dir/$name" || -L "$bin_dir/$name" ]]; then
    cp -L "$bin_dir/$name" "$backup/$name"
    cmp -s "$bin_dir/$name" "$backup/$name"
  fi
done
(
  cd "$backup"
  shopt -s nullglob
  previous=(*)
  if [[ ${#previous[@]} -gt 0 ]]; then
    sha256sum "${previous[@]}" > SHA256SUMS
    sha256sum --check --quiet SHA256SUMS
  fi
)

bash "$repo_root/scripts/install-desktop.sh" "$backup/desktop"

launcher="$(mktemp "$bin_dir/.grafium-launcher.XXXXXXXX")"
{
  echo '#!/bin/bash'
  printf 'export LD_LIBRARY_PATH=%q\n' "$stage"
  printf 'exec %q "$@"\n' "$stage/grafium-bin"
} >"$launcher"
chmod +x "$launcher"
compat_launcher="$(mktemp "$bin_dir/.grafium-bin-launcher.XXXXXXXX")"
cp "$launcher" "$compat_launcher"
chmod +x "$compat_launcher"
sync -f "$stage"
sync -f "$backup"
mv -fT "$compat_launcher" "$bin_dir/grafium-bin"
mv -fT "$launcher" "$bin_dir/grafium"
sync -f "$bin_dir"

echo "installed: $stage ($copied native libraries)"
echo "verified previous entry points: $backup"
echo "ok: run 'grafium' (or use the desktop entry); running instances were not interrupted"

# Keep the newest few installations for rollback and remove older ones, with
# the entry-point backups that could only restore those. A build that a
# running Grafium still executes or maps is never removed.
keep_builds="${GRAFIUM_KEEP_BUILDS:-3}"
if [[ ! "$keep_builds" =~ ^[1-9][0-9]*$ ]]; then
  echo "warning: GRAFIUM_KEEP_BUILDS must be a positive integer; keeping 3 builds" >&2
  keep_builds=3
fi
build_in_use() {
  # A process's maps list its executable as well as its shared libraries.
  grep -qsF -- "$1/" /proc/[0-9]*/maps
}
generation=0
removed_builds=0
while IFS= read -r -d '' build; do
  generation=$((generation + 1))
  if (( generation <= keep_builds )) || [[ "$build" == "$stage" ]] || build_in_use "$build"; then
    continue
  fi
  rm -rf -- "$build"
  removed_builds=$((removed_builds + 1))
done < <(find "$app_lib_dir" -mindepth 1 -maxdepth 1 -type d -name 'build.*' -printf '%T@ %p\0' |
         sort -z -n -r | cut -z -d ' ' -f 2-)
for saved in "$app_lib_dir"/backup.*/grafium; do
  [[ -f "$saved" && "$(head -c 2 "$saved")" == '#!' ]] || continue
  restorable=0
  for reference in $(grep -oE 'build\.[A-Za-z0-9]+' "$saved" || true); do
    [[ -d "$app_lib_dir/$reference" ]] && restorable=1
  done
  (( restorable )) || rm -rf -- "$(dirname "$saved")"
done
if (( removed_builds > 0 )); then
  echo "removed $removed_builds older build(s); keeping the newest $keep_builds"
fi

# Retire Cargo artifacts that newer builds superseded in the checkout this
# build came from; everything its builds still reuse is kept.
if [[ "$(cd "$build_dir" && pwd -P)/" == "$(cd "$repo_root" && pwd -P)/target/"* ]]; then
  if ! python3 "$repo_root/scripts/prune-build-cache.py" "$repo_root"; then
    echo "warning: build cache pruning failed; the installation itself succeeded" >&2
  fi
fi

# Always end by stating exactly what is now installed, so the answer to
# "which build am I testing?" is visible at deploy time instead of needing to
# be reconstructed later.
if [[ -n "$version_line" ]]; then
  echo "installed build: $version_line"
elif [[ -n "$head_sha" ]]; then
  echo "installed build: commit $head_sha (binary predates --version support)"
fi
