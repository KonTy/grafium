#!/usr/bin/env bash
# Build Grafium's distributable packages: Linux bundles and/or an Android APK.
#
# Exists because both targets need environment fixes that are invisible until
# the build fails, and neither failure names its real cause.
#
# Linux/AppImage needs two:
#
#   * LD_LIBRARY_PATH must include the build directory. `llm-local` links
#     llama.cpp and GGML dynamically on purpose (see core/Cargo.toml: two
#     *static* vendored GGML copies collide at link time once `media` and
#     `llm-local` are both on). linuxdeploy resolves the binary's needed
#     libraries against the system loader path only, so it aborts with
#     "Could not find dependency: libggml-base.so.0" even though the .so is
#     sitting right next to the binary it just inspected. The .deb and .rpm
#     bundlers do not care — they copy from the build directory directly —
#     which is why AppImage can be the only target that fails.
#
#   * NO_STRIP=1, because linuxdeploy ships its own 2024-era binutils and
#     modern toolchains emit `.relr.dyn` relocation sections it cannot parse.
#     It then fails every system library it tries to strip, with
#     "unknown type [0x13] section `.relr.dyn'". Stripping is only a size
#     optimization, so skipping it costs disk and nothing else.
#
# Android needs the SDK/NDK exported, and — the subtle one — rustup's cargo
# rather than a distro-packaged /usr/bin/cargo. `rustup target add` installs
# the Android std into ~/.rustup, but a system Rust has its own sysroot under
# /usr and will never see it. The failure is a baffling "can't find crate for
# `core`" naming a target `rustup target list` swears is installed.
#
# Usage:  scripts/build-release.sh [linux|android|all]
#   Defaults to linux. Builds are serial on purpose: every target runs the
#   same `npm run build` into ui/dist, so two at once race and one reads a
#   hashed asset the other has already replaced.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-linux}"

case "$target" in
  linux | android | all) ;;
  *)
    echo "usage: scripts/build-release.sh [linux|android|all]" >&2
    exit 2
    ;;
esac

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: $1 is not installed" >&2
    exit 1
  }
}

build_linux() {
  need cargo-tauri
  echo "==> Building Linux bundles"
  cd "$repo_root"
  NO_STRIP=1 \
    LD_LIBRARY_PATH="$repo_root/target/release${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
    cargo tauri build
  echo "==> Linux bundles in $repo_root/target/release/bundle"
}

build_android() {
  # Prefer rustup's toolchain; a system cargo has no Android std.
  if [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    PATH="$HOME/.cargo/bin:$PATH"
    export PATH
  fi
  need cargo-tauri

  : "${ANDROID_HOME:=$HOME/Android/Sdk}"
  if [[ ! -d "$ANDROID_HOME" ]]; then
    echo "error: no Android SDK at $ANDROID_HOME (set ANDROID_HOME)" >&2
    exit 1
  fi
  export ANDROID_HOME

  if [[ -z "${NDK_HOME:-}" ]]; then
    # Pick the highest installed NDK rather than pinning a version that
    # quietly stops existing after an SDK manager update.
    # `|| true` matters: with `set -e -o pipefail` a missing ndk/ directory
    # makes find exit 1, which would kill the script here and never reach the
    # diagnostic below -- the exact case that message exists to explain.
    NDK_HOME="$(find "$ANDROID_HOME/ndk" -maxdepth 1 -mindepth 1 -type d 2>/dev/null |
      sort -V | tail -1 || true)"
  fi
  if [[ -z "$NDK_HOME" || ! -d "$NDK_HOME" ]]; then
    echo "error: no Android NDK found under $ANDROID_HOME/ndk (set NDK_HOME)" >&2
    exit 1
  fi
  export NDK_HOME

  if [[ -z "${JAVA_HOME:-}" ]] && [[ -d /usr/lib/jvm/java-17-openjdk ]]; then
    export JAVA_HOME=/usr/lib/jvm/java-17-openjdk
  fi

  echo "==> Building Android APK (NDK: $(basename "$NDK_HOME"))"
  cd "$repo_root/ui/src-tauri"
  # `android init` is idempotent and gen/ is gitignored, so regenerate rather
  # than assuming a previous run left a usable project behind.
  cargo tauri android init >/dev/null
  cargo tauri android build --apk --target aarch64
}

case "$target" in
  linux) build_linux ;;
  android) build_android ;;
  all)
    build_linux
    build_android
    ;;
esac
