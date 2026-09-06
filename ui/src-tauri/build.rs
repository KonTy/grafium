use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Must run *before* `tauri_build::build()`: that call validates
    // `bundle.resources` globs from `tauri.conf.json` immediately and fails
    // the build if they don't match anything yet, so the native libraries
    // need to already be in place first.
    //
    // Only relevant on the desktop targets that actually enable `media`/
    // `llm-local` (see the target-conditional `grafium_core` dependency in
    // Cargo.toml) — a no-op everywhere else, including Android (which also
    // doesn't reference `bundled-libs` from its own platform config).
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        bundle_native_libs();
    }

    tauri_build::build();
}

/// `llama-cpp-2`'s `dynamic-link` feature builds llama.cpp/GGML as shared
/// libraries (`libllama.so.0`, `libggml*.so.0` / `llama.dll`, `ggml*.dll`)
/// rather than statically linking them, so the Tauri bundler needs to know
/// where to find them to package them into the installer (see
/// `tauri.conf.json`'s `bundle.resources`).
///
/// Cargo puts them under `llama-cpp-sys-2`'s build script `OUT_DIR`, whose
/// exact path includes a content hash that changes across rebuilds — not
/// something `tauri.conf.json` can reference directly. This copies whatever
/// shared libraries that build produced (dereferencing symlinks, so the
/// real SONAME-versioned files end up as plain regular files) into a fixed,
/// predictable `bundled-libs/` directory next to the build output that
/// `tauri.conf.json` *can* reference.
///
/// Best-effort: if `llm-local`/`media` aren't enabled (e.g. this crate was
/// built without them) there's simply nothing to find, and this silently
/// does nothing.
fn bundle_native_libs() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is always set for build.rs"));
    // OUT_DIR looks like `<target-dir>/<profile>/build/grafium-<hash>/out`;
    // `build_dir` is `<target-dir>/<profile>/build`, where every crate's
    // build-script output for this profile lives, including
    // `llama-cpp-sys-2-<hash>/out/...`.
    let Some(build_dir) = out_dir.ancestors().nth(2) else {
        return;
    };
    let Some(profile_dir) = out_dir.ancestors().nth(3) else {
        return;
    };
    let dest = profile_dir.join("bundled-libs");

    let Ok(entries) = fs::read_dir(build_dir) else {
        return;
    };
    let sys_crate_dirs = entries.flatten().map(|e| e.path()).filter(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with("llama-cpp-sys-2-"))
            .unwrap_or(false)
    });

    let is_windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let mut copied_any = false;
    for sys_dir in sys_crate_dirs {
        let search_root = sys_dir.join("out");
        if !search_root.is_dir() {
            continue;
        }
        for path in find_native_libs(&search_root, is_windows) {
            let Some(file_name) = path.file_name() else {
                continue;
            };
            if !copied_any {
                let _ = fs::create_dir_all(&dest);
            }
            // Dereference symlinks (Linux ships `libggml.so.0` as a symlink
            // to `libggml.so.0.13.1`) so the bundler copies a real file.
            if let Ok(real_path) = path.canonicalize() {
                if fs::copy(&real_path, dest.join(file_name)).is_ok() {
                    copied_any = true;
                }
            }
        }
    }

    println!("cargo:rerun-if-changed=build.rs");

    // Point the linker at `bundled-libs/` (relative to the final
    // executable) so the app finds these at runtime without needing
    // `LD_LIBRARY_PATH` set. Tauri's `resource_dir()` resolves to
    // `<exe_dir>/../lib/<productName>` for both .deb and AppImage on
    // Linux, and to `<exe_dir>` itself on Windows (see
    // tauri-utils::platform::resource_dir_from) — matching where
    // `tauri.conf.json`'s `bundle.resources` places these files.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // `--disable-new-dtags` forces the linker to emit the legacy
        // `DT_RPATH` tag instead of `DT_RUNPATH`. This matters: `DT_RUNPATH`
        // on the main executable only applies to resolving *its own* direct
        // `NEEDED` entries (e.g. `libllama.so.0`) — it is NOT consulted when
        // resolving `libllama.so.0`'s own transitive dependencies
        // (`libggml*.so.0`), since those libraries have no rpath of their
        // own. The older `DT_RPATH`, when set on the main executable, is
        // used by the dynamic loader as a process-wide fallback search path
        // for *all* dependency resolution, transitively — exactly what's
        // needed here. Verified against an actual built `.deb` with `ldd`.
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags,-rpath,$ORIGIN/../lib/Grafium");
    }
}

/// Finds the native shared libraries a llama.cpp CMake build produced under
/// `root`, matching by extension only (`.so`-with-version-suffix on Linux,
/// `.dll` on Windows) since the exact set of libraries CMake emits can
/// change between llama.cpp versions.
fn find_native_libs(root: &Path, is_windows: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let is_native_lib = if is_windows {
                name.ends_with(".dll")
            } else {
                // Matches the real SONAME files Linux/macOS actually load at
                // runtime (e.g. `libggml-base.so.0`), not the unversioned
                // `.so` dev symlinks used only at link time.
                name.contains(".so.") || name.ends_with(".dylib")
            };
            if is_native_lib {
                found.push(path);
            }
        }
    }
    found
}
