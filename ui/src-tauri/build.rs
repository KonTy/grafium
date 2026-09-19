use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    emit_build_stamp();

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

    // The frontend is embedded into the binary by `generate_context!` at
    // compile time, but a proc macro cannot tell Cargo what it read. Cargo
    // also stops watching the package as soon as a build script emits any
    // `rerun-if-changed`, which `bundle_native_libs` does — so without this,
    // changing only frontend files rebuilt nothing and the binary silently
    // kept serving the previous UI. That failure is invisible: the build
    // succeeds, the app runs, and it is simply the old interface.
    println!("cargo:rerun-if-changed=../dist");
    println!("cargo:rerun-if-changed=tauri.conf.json");

    tauri_build::build();
}

/// Records the revision the binary was built from, so a running Grafium can
/// say which code it actually contains.
///
/// `CARGO_PKG_VERSION` alone cannot answer that: it is hand-bumped, so it
/// stays identical across dozens of commits. Without a commit stamp the only
/// way to check whether a deployed binary is current is to compare file
/// hashes against a fresh build — and since that is tedious, the real-world
/// answer is to assume it is current and unknowingly test stale code.
fn emit_build_stamp() {
    let sha = git(&["rev-parse", "--short=7", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .map(|status| !status.is_empty())
        .unwrap_or(false);

    println!("cargo:rustc-env=GRAFIUM_GIT_SHA={sha}");
    println!(
        "cargo:rustc-env=GRAFIUM_GIT_DIRTY={}",
        if dirty { "1" } else { "0" }
    );
    println!("cargo:rustc-env=GRAFIUM_BUILD_TIME={}", build_timestamp());

    watch_git_head();
}

/// Declares the Git files whose contents decide the stamp above.
///
/// These are mandatory, not an optimisation. This build script emits other
/// `rerun-if-changed` lines, which switches Cargo out of its default "rerun
/// when anything in the package changed" mode and into "rerun only when a
/// declared path changed". Committing or switching branches modifies no file
/// inside this package, so without these the stamp would keep reporting
/// whichever revision was checked out the last time the script happened to
/// run. A version indicator that silently lies is worse than none at all,
/// because it actively justifies testing the wrong build.
fn watch_git_head() {
    // `--git-path` resolves these correctly inside a linked worktree, where
    // `.git` is a file and HEAD lives under `.git/worktrees/<name>/`.
    for path in ["HEAD", "index"] {
        if let Some(resolved) = git(&["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
    // HEAD only holds `ref: refs/heads/<branch>` on a branch, so committing
    // moves the ref file rather than HEAD itself.
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(resolved) = git(&["rev-parse", "--git-path", &head_ref]) {
            println!("cargo:rerun-if-changed={resolved}");
        }
    }
}

/// Runs `git` in the crate directory, returning trimmed stdout on success.
///
/// Every failure mode maps to `None` so that building from a source tarball,
/// or on a machine without Git, still succeeds with an `unknown` revision
/// rather than breaking the build.
fn git(args: &[&str]) -> Option<String> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").ok()?;
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.trim().to_string())
}

fn build_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or(0);
    format_utc(seconds)
}

/// Formats a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Hand-rolled to keep `[build-dependencies]` at just `tauri-build`: pulling
/// a date crate in here would add it to every clean build of the app for the
/// sake of one formatted string. The calendar conversion is Howard
/// Hinnant's `civil_from_days`, which is exact for all dates in range.
fn format_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let time_of_day = seconds.rem_euclid(86_400);

    // Shift the epoch to 0000-03-01 so leap days land at the end of the
    // cycle, which is what makes the era arithmetic below branch-free.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;

    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time_of_day / 3_600,
        (time_of_day % 3_600) / 60,
        time_of_day % 60
    )
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
    copy_native_libs();

    println!("cargo:rerun-if-changed=build.rs");

    // Point the linker at `bundled-libs/` (relative to the final
    // executable) so the app finds these at runtime without needing
    // `LD_LIBRARY_PATH` set. Tauri's `resource_dir()` resolves to
    // `<exe_dir>/../lib/<productName>` for both .deb and AppImage on
    // Linux, and to `<exe_dir>` itself on Windows (see
    // tauri-utils::platform::resource_dir_from) — matching where
    // `tauri.conf.json`'s `bundle.resources` places these files.
    //
    // Emitted unconditionally, and deliberately not inside `copy_native_libs`:
    // every early return in there is a "nothing to copy yet" case, and a build
    // that silently shipped a binary with no `RPATH` would only fail later, at
    // runtime, inside the packaged app.
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

/// Copies whatever llama.cpp/GGML shared libraries exist right now into
/// `bundled-libs/`.
///
/// Cargo only orders a build script against its crate's *build*-dependencies,
/// and `llama-cpp-sys-2` is a normal dependency of `grafium-core`, so on a cold
/// build this script can run while that CMake build is still emitting its
/// `.so`s — leaving `bundled-libs/` with a partial set. `cargo:rerun-if-changed`
/// on the shared build directory is what makes that self-correcting: the next
/// build sees the directory changed, re-runs this, and picks up the full set.
/// Without it the only declared input is `build.rs` itself, so a partial copy
/// would stay partial until something forced a rebuild.
fn copy_native_libs() {
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
    let resource_dest = profile_dir
        .parent()
        .map(|target_dir| target_dir.join("release").join("bundled-libs"))
        .unwrap_or_else(|| dest.clone());
    ensure_tauri_resource_glob_dir(&resource_dest);

    // Re-run whenever a crate's build-script output directory appears or
    // disappears here, which is what happens when the `llama-cpp-sys-2` build
    // finally lands its libraries.
    println!("cargo:rerun-if-changed={}", build_dir.display());

    let Ok(entries) = fs::read_dir(build_dir) else {
        return;
    };
    let sys_crate_dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("llama-cpp-sys-2-"))
                .unwrap_or(false)
        })
        .collect();

    let is_windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let mut copied_any = false;
    for sys_dir in &sys_crate_dirs {
        let search_root = sys_dir.join("out");
        // Also track the directory the libraries themselves land in, so a
        // rebuild of llama.cpp alone (same crate dir, new `.so`s) re-copies.
        println!("cargo:rerun-if-changed={}", search_root.display());
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
                    if resource_dest != dest {
                        let _ = fs::copy(&real_path, resource_dest.join(file_name));
                    }
                    copied_any = true;
                }
            }
        }
    }

    if !sys_crate_dirs.is_empty() && !copied_any {
        println!(
            "cargo:warning=llama-cpp-sys-2 is in the build but produced no shared libraries yet; \
             bundled-libs/ is incomplete. This build script will re-run and finish the copy on \
             the next build — repackage after that, or the installer will ship without libllama."
        );
    }
}

fn ensure_tauri_resource_glob_dir(dir: &Path) {
    if fs::create_dir_all(dir).is_err() {
        return;
    }

    let has_entries = fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    if !has_entries {
        let _ = fs::write(dir.join("copilot-dev-placeholder"), "");
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
