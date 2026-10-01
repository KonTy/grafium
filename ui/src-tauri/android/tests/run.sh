#!/usr/bin/env bash
# Compile tracked Kotlin against Android/Media3 and run JVM tests without Rust or a device.
# The isolated fixture stubs only TauriActivity, not Android/Media3 APIs.
# Rust JNI is not linked here; preflight tests also verify that its absence fails closed.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
generated="$root/ui/src-tauri/gen/android"
[[ -x "$generated/gradlew" ]] || {
  echo "Run cargo tauri android init --ci --skip-targets-install from ui first." >&2
  exit 1
}
fixture="$generated/reader-check"
mkdir -p "$fixture/src/main/java/com/grafium/app"
cat > "$fixture/settings.gradle.kts" <<'EOF'
pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories { google(); mavenCentral() } }
rootProject.name = "grafium-reader-check"
EOF
cat > "$fixture/gradle.properties" <<'EOF'
android.useAndroidX=true
org.gradle.jvmargs=-Xmx2048m -XX:ActiveProcessorCount=2
org.gradle.workers.max=2
kotlin.compiler.execution.strategy=in-process
EOF
cat > "$fixture/src/main/java/com/grafium/app/TauriActivity.kt" <<'EOF'
package com.grafium.app
open class TauriActivity : androidx.appcompat.app.AppCompatActivity() {
  open fun onWebViewCreate(webView: android.webkit.WebView) {}
}
EOF
cat > "$fixture/build.gradle.kts" <<'EOF'
plugins {
  id("com.android.application") version "8.11.0"
  id("org.jetbrains.kotlin.android") version "1.9.25"
}
android {
  namespace = "com.grafium.app"
  compileSdk = 36
  defaultConfig {
    applicationId = "com.grafium.app"
    minSdk = 24
    targetSdk = 36
    manifestPlaceholders["usesCleartextTraffic"] = "false"
  }
  buildFeatures { buildConfig = true }
  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_1_8
    targetCompatibility = JavaVersion.VERSION_1_8
  }
  kotlinOptions { jvmTarget = "1.8" }
  testOptions { unitTests.isIncludeAndroidResources = true }
  sourceSets {
    getByName("main") {
      java.srcDir("../app/src/main/java")
      res.srcDir("../app/src/main/res")
      manifest.srcFile("../app/src/main/AndroidManifest.xml")
    }
    getByName("test").java.srcDir("../../../android/tests")
    getByName("test").resources.srcDir("../app/src/test/resources")
  }
}
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
  exclude("**/generated/**")
}
dependencies {
  implementation("androidx.appcompat:appcompat:1.7.1")
  implementation("androidx.activity:activity-ktx:1.10.1")
  implementation("androidx.webkit:webkit:1.12.1")
  implementation("com.google.android.material:material:1.12.0")
  implementation("androidx.media3:media3-exoplayer:1.5.1")
  implementation("androidx.media3:media3-session:1.5.1")
  implementation(files("../app/libs/sherpa-onnx-1.13.8.aar"))
  testImplementation("junit:junit:4.13.2")
  testImplementation("org.robolectric:robolectric:4.14.1")
}
EOF
"$root/scripts/prepare-android.sh"
runner=(nice -n 15 "$generated/gradlew" -p "$fixture" testDebugUnitTest --console=plain --max-workers=2 --no-daemon)
if command -v taskset >/dev/null 2>&1; then
  cpus="$(python3 -c 'import os; print(",".join(map(str, sorted(os.sched_getaffinity(0))[:2])))')"
  taskset --cpu-list "$cpus" "${runner[@]}" "$@"
else
  "${runner[@]}" "$@"
fi
