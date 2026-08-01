# Standalone Companion (DEPRECATED)

This directory contains the original **standalone** Android companion app
(`com.grafium.companion`) that received voice commands from SilentPulse and
wrote plain `.md` files into a shared Grafium graph directory, relying on the
**desktop** Grafium app's file watcher to re-index them.

## Status: superseded

The **Tauri Android build** in `ui/src-tauri/gen/android/` (`com.grafium.app`)
now handles voice-assistant commands and shares **the exact same Rust NLU**
as the desktop UI via JNI (`grafium_core::assistant::handle_command`,
exposed as `Java_com_grafium_app_AssistantReceiver_nativeHandleCommand`).

That means:
- No duplicated command grammar between platforms.
- New features (priority sorting, "todos due today", etc.) light up on both
  desktop and Android from a single change in `core/src/assistant/`.
- The Kotlin receiver is a ~200-line shim that only marshals intent extras
  in/out of the native call.

## What was changed

`android/app/src/main/AndroidManifest.xml` sets
`android:enabled="false"` on `VoiceCommandReceiver`, so this app no longer
registers itself with SilentPulse. If you *only* have the standalone
companion installed and *not* the Tauri build, flip that back to `"true"` —
but be aware the command grammar here is frozen and will not match what the
Tauri build does.

## To fully remove

Once you're satisfied the Tauri Android build is your daily driver:

```bash
rm -rf android/
# then delete `android` from settings.gradle.kts / workspace files if present
```
