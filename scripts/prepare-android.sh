#!/usr/bin/env bash
# Apply Grafium's tracked Android customizations to Tauri's generated project.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
android_root="$repo_root/ui/src-tauri/gen/android/app/src/main"
manifest="$android_root/AndroidManifest.xml"
tracked="$repo_root/ui/src-tauri/android"

# Desktop-only checkouts do not have a generated Android project.
[[ -f "$manifest" ]] || exit 0

for source in "$tracked"/*.kt; do
  install -D -m 0644 "$source" "$android_root/java/com/grafium/app/$(basename "$source")"
done
# Retired JNI wrapper: canonical-CFI uploads now prepare the native narration queue.
rm -f "$android_root/java/com/grafium/app/ReaderEpubNative.kt"
while IFS= read -r -d '' source; do
  install -D -m 0644 "$source" "$android_root/res/${source#"$tracked/res/"}"
done < <(find "$tracked/res" -type f -print0)
while IFS= read -r -d '' source; do
  install -D -m 0644 "$source" "$android_root/assets/${source#"$tracked/assets/"}"
done < <(find "$tracked/assets" -type f -print0)
for source in "$tracked"/tests/*.kt; do
  install -D -m 0644 "$source" "$android_root/../test/java/com/grafium/app/$(basename "$source")"
done
install -D -m 0644 "$repo_root/ui/tests/fixtures/private-reader-position.json" \
  "$android_root/../test/resources/private-reader-position.json"
install -m 0644 "$tracked/AndroidManifest.xml" "$manifest"
install -m 0644 "$tracked/reader-rules.pro" "$repo_root/ui/src-tauri/gen/android/app/reader-rules.pro"

# sherpa-onnx is an in-process offline runtime, not Android's system/cloud TTS.
# Pin the upstream AAR digest; voice models remain separate explicit user imports/downloads.
speech_aar="$repo_root/ui/src-tauri/gen/android/app/libs/sherpa-onnx-1.13.8.aar"
speech_sha="633c24321e06b1fe79feafa03ea16cbc0f8a286641e2da3559bac91bdb13bd96"
if [[ ! -f "$speech_aar" ]] || [[ "$(sha256sum "$speech_aar" | cut -d ' ' -f 1)" != "$speech_sha" ]]; then
  mkdir -p "$(dirname "$speech_aar")"
  curl --fail --location --proto '=https' --tlsv1.2 --retry 2 \
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/sherpa-onnx-1.13.8.aar" \
    --output "$speech_aar.part"
  [[ "$(sha256sum "$speech_aar.part" | cut -d ' ' -f 1)" == "$speech_sha" ]] ||
    { echo "error: offline speech runtime SHA-256 mismatch" >&2; exit 1; }
  mv "$speech_aar.part" "$speech_aar"
fi

# Keep dependency versions tracked, idempotently injected into either generated DSL.
python3 - "$repo_root/ui/src-tauri/gen/android/app" <<'PY'
import pathlib
import sys
root = pathlib.Path(sys.argv[1])
gradle = root / "build.gradle.kts"
if not gradle.exists():
    gradle = root / "build.gradle"
if not gradle.exists():
    raise SystemExit("error: generated Android app Gradle build is missing")
start = "// BEGIN GRAFIUM PRIVATE READER"
end = "// END GRAFIUM PRIVATE READER"
text = gradle.read_text()
if start in text:
    before, rest = text.split(start, 1)
    _, after = rest.split(end, 1)
    text = before.rstrip() + after
text = text.rstrip() + """

// BEGIN GRAFIUM PRIVATE READER
dependencies {
    implementation("androidx.media3:media3-exoplayer:1.5.1")
    implementation("androidx.media3:media3-session:1.5.1")
    implementation("androidx.webkit:webkit:1.12.1")
    implementation(files("libs/sherpa-onnx-1.13.8.aar"))
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.robolectric:robolectric:4.14.1")
}
// END GRAFIUM PRIVATE READER
"""
gradle.write_text(text)
PY

grep -q 'android.permission.MANAGE_EXTERNAL_STORAGE' "$manifest" ||
  { echo "error: Android manifest is missing persistent storage access" >&2; exit 1; }
