#!/usr/bin/env bash
# Install a locally-built Grafium into ~/.local for day-to-day use.
#
# Exists because deploying by hand kept drifting: the binary got copied to
# ~/.local/bin while the llama.cpp/ggml shared objects it dynamically links
# against were left behind from an older build. Same soname, different code —
# so it still *links* and starts, and the breakage only shows up later as
# subtly wrong inference behaviour. Copying both together, from one build
# directory, is the whole point of this script.
#
# Usage:  scripts/deploy-local.sh [build-dir]
#   build-dir defaults to the repo's own target/release.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_dir="${1:-$repo_root/target/release}"
bin_dir="$HOME/.local/bin"
lib_dir="$HOME/.local/lib"

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

mkdir -p "$bin_dir" "$lib_dir"

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

# The launcher sets LD_LIBRARY_PATH rather than relying on the binary's
# RPATH, so the libraries can live in a plain ~/.local/lib alongside
# everything else instead of a Grafium-specific directory.
cat >"$bin_dir/grafium" <<EOF
#!/bin/bash
export LD_LIBRARY_PATH="$lib_dir:\$LD_LIBRARY_PATH"
exec $bin_dir/grafium-bin "\$@"
EOF
chmod +x "$bin_dir/grafium"

# Install to a temp name then rename: replacing a running executable in
# place gets ETXTBSY, and a partial copy would leave an unstartable app.
cp "$binary" "$bin_dir/grafium-bin.new"
chmod +x "$bin_dir/grafium-bin.new"
mv -f "$bin_dir/grafium-bin.new" "$bin_dir/grafium-bin"
echo "installed: $bin_dir/grafium-bin"

shopt -s nullglob
copied=0
for so in "$build_dir"/lib{ggml,ggml-base,ggml-cpu,ggml-vulkan,ggml-cuda,llama,llama-common}.so*; do
  cp -P "$so" "$lib_dir/"
  copied=$((copied + 1))
done
shopt -u nullglob
echo "installed: $copied shared objects -> $lib_dir"

if [[ $copied -eq 0 ]]; then
  echo "warning: no ggml/llama shared objects found in $build_dir." >&2
  echo "         A local-LLM build should produce them; the app will fail to start" >&2
  echo "         if it was built with a dynamically-linked llama.cpp." >&2
fi

# Fail loudly on the exact drift this script exists to prevent, rather than
# letting a missing/oversized library surface as a runtime crash.
if command -v ldd >/dev/null 2>&1; then
  if missing=$(LD_LIBRARY_PATH="$lib_dir" ldd "$bin_dir/grafium-bin" 2>/dev/null | grep "not found"); then
    echo "error: unresolved shared libraries after install:" >&2
    echo "$missing" >&2
    exit 1
  fi
fi

echo "ok: run 'grafium' (or use the desktop entry)"

# Always end by stating exactly what is now installed, so the answer to
# "which build am I testing?" is visible at deploy time instead of needing to
# be reconstructed later.
if [[ -n "$version_line" ]]; then
  echo "installed build: $version_line"
elif [[ -n "$head_sha" ]]; then
  echo "installed build: commit $head_sha (binary predates --version support)"
fi
