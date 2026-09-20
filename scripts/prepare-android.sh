#!/usr/bin/env bash
# Apply Grafium's tracked Android customizations to Tauri's generated project.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
android_root="$repo_root/ui/src-tauri/gen/android/app/src/main"
manifest="$android_root/AndroidManifest.xml"
activity="$android_root/java/com/grafium/app/MainActivity.kt"
receiver="$android_root/java/com/grafium/app/AssistantReceiver.kt"

# Desktop-only checkouts do not have a generated Android project.
[[ -f "$manifest" ]] || exit 0

install -D -m 0644 "$repo_root/ui/src-tauri/android/MainActivity.kt" "$activity"
install -D -m 0644 "$repo_root/ui/src-tauri/android/AssistantReceiver.kt" "$receiver"
install -m 0644 "$repo_root/ui/src-tauri/android/AndroidManifest.xml" "$manifest"

grep -q 'android.permission.MANAGE_EXTERNAL_STORAGE' "$manifest" ||
  { echo "error: Android manifest is missing persistent storage access" >&2; exit 1; }
