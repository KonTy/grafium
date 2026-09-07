//! Process-isolated variant of [`super::local_llm::LocalLlm`] — same
//! model, same generation logic, but run inside a separate
//! `grafium-llm-worker` child process (see `src/bin/llm_worker.rs`)
//! instead of in this process directly.
//!
//! # Why
//! Local LLM inference via `llama-cpp-2` is a thin binding over llama.cpp,
//! genuine native C/C++ code. That code has a confirmed failure mode: an
//! upstream bug (new hybrid architectures like Qwen3.5/Qwen3-Next, see the
//! `KNOWN_UNSTABLE_ARCHITECTURES` check in `local_llm.rs`, and more
//! generally the class of bug it guards against) can segfault the whole
//! process partway through generation. A native SIGSEGV has no
//! `Result::Err` to catch and cannot be caught by `catch_unwind` — when
//! that happens in-process, it takes the entire Tauri app down with it,
//! along with whatever else the user was doing.
//!
//! `LocalLlmProcess` runs the *exact same* `LocalLlm` code
//! (model loading, RAM/VRAM budgeting, the architecture safety check,
//! automatic fallback-to-another-model — none of that logic is
//! duplicated or reimplemented here) inside a dedicated child process.
//! If that process dies unexpectedly (crash, killed, etc.), this struct
//! detects it (the pipe closes) and returns a clean [`CoreError`] instead
//! of the whole app disappearing, and transparently respawns a fresh
//! worker on the next request.
//!
//! This is the type `knowledge::engine` now constructs instead of
//! `LocalLlm` directly (behind the same `llm-local` feature) — the trait
//! object boundary (`Box<dyn LlmProvider>`) means no other call site needs
//! to change at all.
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::ai::config::LocalLlmSettings;
use crate::ai::traits::{BoxFuture, ChatMessage, CompletionOptions, LlmProvider};
use crate::error::{CoreError, Result};
use crate::model_library;

/// One request line sent to the worker's stdin — see `llm_worker.rs`'s
/// module docs for the full line-delimited-JSON protocol this is half of.
/// Deliberately the *single* definition of this shape: `llm_worker.rs`
/// imports this same type (both for serializing/deserializing) rather
/// than redefining it, so the two sides of this protocol can never drift
/// out of sync with each other.
#[derive(Serialize, Deserialize)]
pub struct WorkerRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub options: CompletionOptions,
}

/// One response line read from the worker's stdout — same "single
/// definition, shared by both sides" reasoning as [`WorkerRequest`].
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerResponse {
    /// Reported by the worker exactly once at startup, immediately after
    /// the model file finished loading and the process is ready to
    /// accept the first `WorkerRequest` on stdin.
    ///
    /// `backend` and `backend_device` are populated best-effort from the
    /// llama.cpp / GGML tracing lines emitted during model load — see
    /// `grafium_core::log_tap` and the worker's own subscriber wiring.
    /// They're both `None` on older workers that predate this field
    /// (serde treats a missing field as `None` for `Option<T>`), so the
    /// parent's UI code has to tolerate the absence without erroring.
    Ready {
        name: String,
        #[serde(default)]
        backend: Option<String>,
        #[serde(default)]
        backend_device: Option<String>,
        #[serde(default)]
        backend_reason: Option<String>,
        #[serde(default)]
        load_seconds: Option<f64>,
    },
    LoadError { message: String },
    Token { text: String },
    Done { text: String },
    Error { message: String },
}

/// A live worker child process plus the pipe handles needed to talk to it.
/// `stdout` is a `BufReader` (not the raw handle) so `read_line` can be
/// used directly — line-buffering is exactly the protocol's framing.
struct WorkerHandle {
    child: Child,
    stdin: ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // Best-effort: if the worker is still alive when this handle is
        // dropped (e.g. `LocalLlmProcess` itself is being dropped, not the
        // more common "it crashed" case), don't leave an orphaned process
        // holding onto GBs of VRAM/RAM behind us.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The mutable/shared state behind a [`LocalLlmProcess`], split out and
/// wrapped in an `Arc` so `complete()`/`complete_stream()` can move a cheap
/// clone of it into `tokio::task::spawn_blocking` (which requires
/// `'static`) instead of capturing a borrow of `&self` — mirrors exactly
/// why `LocalLlm::complete` clones its `Arc<LlamaModel>`/`Arc<LlamaBackend>`
/// fields rather than capturing `self` there too.
struct Inner {
    worker_bin: PathBuf,
    models_dir: PathBuf,
    /// Base settings for the worker — kept as a structured value (not
    /// just a JSON string) so a respawn can apply runtime overrides like
    /// `respawn_use_mmap_override` before re-serializing.
    settings: Mutex<LocalLlmSettings>,
    /// Runtime override applied to `settings.use_mmap` on the next
    /// respawn. Set to `Some(false)` by `note_worker_crashed` when the
    /// previous exit looks like SIGBUS from a mmap page-fault, so a
    /// follow-up request auto-heals without the user having to touch
    /// Settings. Sticky for the process wrapper's lifetime; a full
    /// restart clears it (a persistent preference lives in
    /// `LocalLlmSettings::use_mmap` in the user's config instead).
    respawn_use_mmap_override: Mutex<Option<bool>>,
    worker: Mutex<Option<WorkerHandle>>,
}

impl Inner {
    /// Compose the settings JSON we'll hand to the next `spawn_worker`
    /// call — base settings plus any sticky runtime overrides
    /// (currently only mmap). Kept in one place so both the initial
    /// spawn in `from_settings` and the auto-respawn in `run_request`
    /// pick up the same overrides.
    fn compose_settings_json(&self) -> Result<String> {
        let mut settings = self
            .settings
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(force) = *self
            .respawn_use_mmap_override
            .lock()
            .unwrap_or_else(|p| p.into_inner())
        {
            settings.use_mmap = Some(force);
        }
        serde_json::to_string(&settings).map_err(CoreError::Json)
    }

    /// Records that the worker just exited under conditions matching a
    /// SIGBUS-flavored mmap crash, and arms `respawn_use_mmap_override`
    /// so the next spawn forces `use_mmap=false`. `status_str` is the
    /// `ExitStatus`'s Display form (Unix: `"signal: 7 (SIGBUS) ..."`).
    ///
    /// Returns whether the flag was newly armed — the caller uses this
    /// to tailor the error message (first hit vs. still-crashing).
    fn note_worker_crashed(&self, status_str: &str) -> bool {
        if !status_is_sigbus(status_str) {
            return false;
        }
        let mut guard = self
            .respawn_use_mmap_override
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let already_armed = *guard == Some(false);
        *guard = Some(false);
        if already_armed {
            tracing::warn!(
                "worker crashed with SIGBUS again even with use_mmap=false — \
                 storage or model file may be genuinely corrupted"
            );
        } else {
            tracing::warn!(
                "worker crashed with SIGBUS ({}), arming use_mmap=false for next respawn",
                status_str
            );
        }
        !already_armed
    }
}

/// Cheap textual check for whether the child's `ExitStatus`'s Display
/// form names signal 7 / SIGBUS. Both `signal: 7` (older `ExitStatus`
/// formatting) and `SIGBUS` (newer) are matched so we don't rely on
/// one specific stdlib format. Kept as a pure string function so it's
/// trivially unit-testable without spawning real children.
fn status_is_sigbus(status_str: &str) -> bool {
    let s = status_str.to_ascii_lowercase();
    s.contains("sigbus") || s.contains("signal: 7 ")
}

pub struct LocalLlmProcess {
    inner: Arc<Inner>,
    name: String,
    /// Which llama.cpp/GGML backend the worker reported at load time —
    /// "Vulkan", "CUDA", "Metal", "CPU", etc. `None` on older workers
    /// that predate the field. Exposed via [`Self::backend_summary`]
    /// so the UI can tell the user "your local LLM is on GPU (Vulkan)"
    /// vs "your local LLM fell back to CPU — expect slow responses"
    /// up front instead of leaving them staring at a spinner.
    backend: Option<String>,
    backend_device: Option<String>,
    backend_reason: Option<String>,
    load_seconds: Option<f64>,
}

impl LocalLlmProcess {
    /// Human-readable one-liner describing which backend the local LLM
    /// worker actually loaded onto, including load time and (on CPU
    /// fallback) the reason GPU wasn't used. Suitable for prepending
    /// verbatim to a progress event before the first inference call.
    fn describe_backend(&self) -> String {
        let backend = self.backend.as_deref().unwrap_or("unknown");
        let device = self.backend_device.as_deref();
        let load_seconds = self.load_seconds.unwrap_or(0.0);
        let base = match (backend, device) {
            ("cpu", _) => {
                let reason = self
                    .backend_reason
                    .as_deref()
                    .unwrap_or("GPU unavailable or not reported by llama.cpp");
                format!(
                    "⚠ Local LLM is running on CPU (loaded in {load_seconds:.1}s). \
                     Reason: {reason}. Inference will be significantly slower — \
                     consider lowering \"context_size\", picking a smaller/more \
                     quantized model, or enabling GPU offload in Settings."
                )
            }
            ("vulkan", Some(dev)) | ("cuda", Some(dev)) | ("metal", Some(dev)) => format!(
                "Local LLM loaded on {} ({}) in {:.1}s.",
                backend, dev, load_seconds
            ),
            (b, _) => format!(
                "Local LLM loaded on {b} backend in {load_seconds:.1}s."
            ),
        };
        base
    }
}

/// Locates the `grafium-llm-worker` binary that should sit right next to
/// whichever binary is currently running (`grafium`, `grafium-tui`, or a
/// test/example binary during development) — mirrors how Tauri "sidecar"
/// binaries are conventionally resolved, without requiring the
/// `tauri`-specific sidecar API (this needs to work from plain `core`,
/// which knows nothing about Tauri).
///
/// `GRAFIUM_LLM_WORKER_PATH` overrides this entirely — needed for
/// development/tests, where `current_exe()` is some `target/.../deps/...`
/// path that was never going to have the worker binary sitting next to it
/// (unlike the real packaged app, where both binaries are built into, and
/// shipped from, the same directory).
fn resolve_worker_binary() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("GRAFIUM_LLM_WORKER_PATH") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Other(format!("failed to locate own executable path: {e}")))?;
    let dir = exe.parent().ok_or_else(|| {
        CoreError::Other("own executable path has no parent directory".to_string())
    })?;
    let name = if cfg!(windows) {
        "grafium-llm-worker.exe"
    } else {
        "grafium-llm-worker"
    };
    let candidate = dir.join(name);
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(CoreError::Other(format!(
            "local LLM worker binary not found at {} (expected next to {}) — set \
             GRAFIUM_LLM_WORKER_PATH to override, or rebuild with the `llm-local`/\
             `llm-local-vulkan` feature enabled so it gets built",
            candidate.display(),
            exe.display()
        )))
    }
}

/// Points the dynamic loader at wherever the `libllama`/`libggml*` shared
/// libraries actually are, via `LD_LIBRARY_PATH` (`DYLD_LIBRARY_PATH` on
/// macOS; a no-op on Windows, where DLLs next to the `.exe` are found
/// automatically).
///
/// The *main* `grafium` binary doesn't need this: `ui/src-tauri/build.rs`
/// bakes a `$ORIGIN/../lib/Grafium`-relative `DT_RPATH` into it directly.
/// But `grafium-llm-worker` is a separate `core`-crate binary target,
/// built by plain `cargo`/`core`'s own build (no Tauri involvement, no
/// bundling knowledge), so it has no rpath of its own — without this, it
/// silently fails to start (`exit status 127`, "cannot open shared object
/// file") the moment the parent process wasn't *also* launched with
/// `LD_LIBRARY_PATH` already set (e.g. run via a plain desktop-launcher
/// script rather than a dev shell that happened to export it).
///
/// Tries, in order: `<worker_dir>/bundled-libs` (what a raw
/// `cargo build --release` always produces, see `bundle_native_libs` in
/// `ui/src-tauri/build.rs`) and `<worker_dir>/../lib/Grafium` (where a
/// packaged install / the main binary's own rpath expects them). Best
/// effort — if neither exists, leaves the environment untouched and lets
/// the exec fail with its own clear OS-level error rather than guessing.
fn set_native_lib_path_env(command: &mut Command, worker_bin: &Path) {
    if cfg!(windows) {
        return;
    }
    let Some(dir) = worker_bin.parent() else {
        return;
    };
    let candidates = [dir.join("bundled-libs"), dir.join("..").join("lib").join("Grafium")];
    let Some(lib_dir) = candidates.into_iter().find(|p| p.is_dir()) else {
        return;
    };
    let var = if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    let mut value = lib_dir.into_os_string();
    if let Some(existing) = std::env::var_os(var) {
        value.push(":");
        value.push(existing);
    }
    command.env(var, value);
}

/// Everything a worker reports on its way to being ready, plus the
/// process handle itself. Returned by [`spawn_worker`] so
/// [`LocalLlmProcess::from_settings`] can store the backend/timing
/// diagnostics next to the process it belongs to.
struct WorkerStartup {
    handle: WorkerHandle,
    name: String,
    backend: Option<String>,
    backend_device: Option<String>,
    backend_reason: Option<String>,
    load_seconds: Option<f64>,
}

/// Spawns a fresh worker process and blocks (this is always called from a
/// context that's already synchronous/blocking — see the callers below)
/// until it reports either `Ready` or `LoadError`. All of the actual model
/// loading — RAM/VRAM budgeting, the architecture safety check, automatic
/// fallback to another already-downloaded model — happens *inside* the
/// worker via its own call to `LocalLlm::from_settings`; this function
/// only has to relay whichever outcome that produced.
fn spawn_worker(worker_bin: &Path, models_dir: &Path, settings_json: &str) -> Result<WorkerStartup> {
    let mut command = Command::new(worker_bin);
    command
        .arg(models_dir)
        .arg(settings_json)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Inherit stderr: the worker's own `tracing` output (llama.cpp's
        // internal logs, the same `vram_snapshot()`/decode-step logging
        // `local_llm.rs` always had) should keep landing wherever the
        // parent process's stderr already goes (e.g. `grafium.log`), not
        // vanish into a pipe nobody reads.
        .stderr(Stdio::inherit());
    set_native_lib_path_env(&mut command, worker_bin);
    let mut child = command
        .spawn()
        .map_err(|e| {
            CoreError::Other(format!(
                "failed to spawn local LLM worker process ({}): {e}",
                worker_bin.display()
            ))
        })?;

    let stdin = child.stdin.take().ok_or_else(|| {
        CoreError::Other("local LLM worker process has no stdin handle".to_string())
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        CoreError::Other("local LLM worker process has no stdout handle".to_string())
    })?;
    let mut stdout = std::io::BufReader::new(stdout);

    let mut line = String::new();
    let bytes_read = stdout.read_line(&mut line).map_err(CoreError::Io)?;
    if bytes_read == 0 {
        // Stdout closed before ever printing a line — the process died
        // (or exited) before it could even report a load error, e.g. it
        // was killed, failed to start up its runtime, or panicked in a way
        // that *did* unwind cleanly rather than segfaulting. Reap it so it
        // doesn't linger as a zombie, then surface a clear error either
        // way.
        let status = child.wait().map_err(CoreError::Io)?;
        return Err(CoreError::Other(format!(
            "local LLM worker process exited ({status}) before reporting whether the model \
             loaded successfully"
        )));
    }

    match serde_json::from_str::<WorkerResponse>(line.trim()) {
        Ok(WorkerResponse::Ready {
            name,
            backend,
            backend_device,
            backend_reason,
            load_seconds,
        }) => Ok(WorkerStartup {
            handle: WorkerHandle {
                child,
                stdin,
                stdout,
            },
            name,
            backend,
            backend_device,
            backend_reason,
            load_seconds,
        }),
        Ok(WorkerResponse::LoadError { message }) => {
            let _ = child.wait(); // Reap — it already exited after reporting this.
            Err(CoreError::Other(message))
        }
        Ok(_) => Err(CoreError::Other(
            "local LLM worker process sent an unexpected message before reporting readiness"
                .to_string(),
        )),
        Err(e) => Err(CoreError::Other(format!(
            "local LLM worker process sent an unparseable startup message ({}): {e}",
            line.trim()
        ))),
    }
}

impl LocalLlmProcess {
    /// Settings-driven constructor — the process-isolated counterpart to
    /// `LocalLlm::from_settings`, with the identical signature/behavior
    /// from the caller's point of view (resolves the configured model,
    /// falls back to another already-downloaded one if it doesn't fit or
    /// is a known-unstable architecture, returns `Err` only if nothing
    /// usable is found at all) — all of that logic lives, unchanged, in
    /// `LocalLlm::from_settings` itself, now just running inside the
    /// worker process instead of this one.
    pub fn from_settings(models_dir: &Path, settings: &LocalLlmSettings) -> Result<Self> {
        let worker_bin = resolve_worker_binary()?;
        let settings_json = serde_json::to_string(settings).map_err(CoreError::Json)?;

        let startup = spawn_worker(&worker_bin, models_dir, &settings_json)?;
        let WorkerStartup {
            handle,
            name,
            backend,
            backend_device,
            backend_reason,
            load_seconds,
        } = startup;

        Ok(Self {
            inner: Arc::new(Inner {
                worker_bin,
                models_dir: models_dir.to_path_buf(),
                settings: Mutex::new(settings.clone()),
                respawn_use_mmap_override: Mutex::new(None),
                worker: Mutex::new(Some(handle)),
            }),
            // Fixed at construction time, deliberately not updated on a
            // later respawn (even if the worker's own fallback search
            // picks a different model the second time around) — this is
            // purely an informational/display value (provider name shown
            // in Settings/UI), not load-bearing, and keeping it stable
            // avoids the borrow-checker/lifetime complexity of returning
            // `&str` from `LlmProvider::name(&self)` for a value that can
            // change out from under a `&self` (non-`&mut self`) method.
            name,
            backend,
            backend_device,
            backend_reason,
            load_seconds,
        })
    }

    /// Same as [`Self::from_settings`], but takes the whole
    /// [`crate::ai::config::AiConfig`] + app data dir — mirrors
    /// `LocalLlm::from_config` exactly (same models-dir default
    /// resolution), for the same reason: callers loading settings
    /// straight from disk have this shape on hand, not pre-extracted
    /// fields.
    pub fn from_config(config: &crate::ai::config::AiConfig, data_dir: &Path) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".to_string()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        Self::from_settings(&models_dir, &local.local_llm)
    }
}

impl Inner {
    /// Runs one request against the (already-loaded) worker, transparently
    /// respawning it first if a previous request's crash (or the worker
    /// simply never having been spawned yet, which shouldn't happen post-
    /// construction) left `self.worker` empty. `on_token` is invoked for
    /// every `Token` line as it streams in — `complete()` just discards
    /// the incremental pieces and keeps the final `Done` text, exactly
    /// like `LlmProvider::complete_stream`'s own default/`LocalLlm`'s
    /// relationship to `complete`.
    ///
    /// This is written as a plain blocking function (not `async`)
    /// deliberately — every call site below runs it inside
    /// `tokio::task::spawn_blocking`, the same pattern `local_llm.rs`'s
    /// `generate()` already uses for the in-process engine, since the
    /// underlying work (writing to a pipe, blocking on a child process)
    /// is exactly the kind of blocking I/O that pattern exists for.
    fn run_request(
        &self,
        messages: &[ChatMessage],
        options: &CompletionOptions,
        mut on_token: impl FnMut(&str),
    ) -> Result<String> {
        let mut guard = self.worker.lock().unwrap_or_else(|p| p.into_inner());

        // Detect a worker that crashed (or was killed) since the last
        // request, before trying to use it — `try_wait` is non-blocking
        // and returns `Ok(Some(_))` once the child has actually exited.
        if let Some(handle) = guard.as_mut() {
            if matches!(handle.child.try_wait(), Ok(Some(_))) {
                *guard = None;
            }
        }

        if guard.is_none() {
            let settings_json = self.compose_settings_json()?;
            let startup =
                spawn_worker(&self.worker_bin, &self.models_dir, &settings_json)?;
            *guard = Some(startup.handle);
            // Note: we deliberately don't update the parent's cached
            // `backend`/`load_seconds` fields on a respawn — they
            // reflect the *first* load, which is what's already been
            // reported to the UI. A silent respawn (e.g. after a
            // crash) that happens to land on a different backend
            // isn't user-visible, and re-plumbing "hey the LLM's
            // backend changed" up through the whole progress-event
            // stack for a rare edge case isn't worth the complexity.
        }

        let handle = guard.as_mut().expect("just ensured Some above");

        let request = WorkerRequest {
            messages: messages.to_vec(),
            options: options.clone(),
        };
        let request_line = serde_json::to_string(&request).map_err(CoreError::Json)?;
        let write_result = (|| -> std::io::Result<()> {
            writeln!(handle.stdin, "{request_line}")?;
            handle.stdin.flush()
        })();
        if let Err(e) = write_result {
            // Broken pipe almost always means the worker died — drop it
            // so the *next* call respawns, rather than reusing a dead
            // handle and getting the same error forever. Reap the child
            // first so we can inspect its exit status for a SIGBUS
            // signature (same recovery path as the mid-response case
            // below), otherwise a worker that died between spawn and
            // the very first `writeln!` here would slip past the auto
            // mmap-fallback and keep crashing.
            let status = guard
                .take()
                .map(|mut h| h.child.wait().ok())
                .flatten()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "unknown exit status".to_string());
            let armed_mmap_off = self.note_worker_crashed(&status);
            if armed_mmap_off {
                return Err(CoreError::Other(format!(
                    "local LLM worker exited with SIGBUS before it could accept the request \
                     ({status}) — this is llama.cpp's memory-mapped read of the model file \
                     hitting a page it couldn't fault in (unreliable storage, corrupt GGUF, \
                     or not enough RAM+swap). Auto-retrying your next request with \
                     memory-mapping disabled. Set \"Disable memory-mapping\" in Settings → \
                     Local LLM to make this permanent, or re-download the GGUF file."
                )));
            }
            return Err(CoreError::Other(format!(
                "local LLM worker process is no longer accepting requests (it likely crashed, \
                 exit={status}): {e}"
            )));
        }

        loop {
            let mut line = String::new();
            let bytes_read = match handle.stdout.read_line(&mut line) {
                Ok(n) => n,
                Err(e) => {
                    // Same reap-and-classify pattern as the "broken pipe
                    // on stdin" branch above: if the child died with
                    // SIGBUS, arm the sticky mmap-off flag so the next
                    // request auto-heals.
                    let status = guard
                        .take()
                        .map(|mut h| h.child.wait().ok())
                        .flatten()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "unknown exit status".to_string());
                    let armed_mmap_off = self.note_worker_crashed(&status);
                    if armed_mmap_off {
                        return Err(CoreError::Other(format!(
                            "lost contact with the local LLM worker mid-response — it \
                             exited with SIGBUS ({status}), which is llama.cpp's mmap of \
                             the model file hitting a page it couldn't fault in. \
                             Auto-retrying your next request with memory-mapping disabled."
                        )));
                    }
                    return Err(CoreError::Other(format!(
                        "lost contact with the local LLM worker process mid-response (it likely \
                         crashed, exit={status}): {e}"
                    )));
                }
            };
            if bytes_read == 0 {
                // EOF: the worker's stdout closed, i.e. it exited, without
                // ever sending a `done`/`error` line for this request —
                // exactly the signature of a native crash (SIGSEGV) mid-
                // generation, the whole reason this module exists. Reap it
                // and surface a clean, actionable error instead of the
                // silent process-wide crash this used to be.
                let status = guard
                    .take()
                    .map(|mut h| h.child.wait().ok())
                    .flatten()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "unknown exit status".to_string());
                // SIGBUS (signal 7) has a distinct root cause from the
                // more common SIGSEGV: it's almost always the kernel
                // failing to fault in a page of the model file that
                // llama.cpp had `mmap`ed, i.e. a storage/RAM problem,
                // not a "bad model architecture" problem. Detect it and
                // (a) arm the sticky "force use_mmap=false on next
                // respawn" flag so a follow-up request auto-heals, and
                // (b) surface the specific cause and remedy in the
                // error message instead of the generic crash text.
                let armed_mmap_off = self.note_worker_crashed(&status);
                if armed_mmap_off {
                    return Err(CoreError::Other(format!(
                        "local LLM worker process crashed with SIGBUS ({status}) — this is \
                         llama.cpp's memory-mapped read of the model file hitting a page \
                         it couldn't fault in. Almost always caused by: (a) the model file \
                         living on unreliable storage (removable drive that disconnected, \
                         network mount, snap/flatpak sandbox, filesystem under extreme \
                         pressure), (b) a partial/corrupt GGUF download, or (c) not enough \
                         combined RAM + swap to back the mmap. \
                         Auto-retrying your next request with memory-mapping disabled — \
                         this is slower to load but doesn't depend on lazy page-in and \
                         will surface a real \"out of memory\" error instead of crashing \
                         if the model still doesn't fit. To make this permanent (recommended \
                         if this keeps happening) set \"Disable memory-mapping\" in \
                         Settings → Local LLM. To rule out a corrupt model file, re-download \
                         the GGUF."
                    )));
                }
                if status_is_sigbus(&status) {
                    // Second SIGBUS in a row even with use_mmap=false —
                    // now we're looking at a genuinely broken model
                    // file or filesystem, not a mmap-vs-eager tradeoff.
                    return Err(CoreError::Other(format!(
                        "local LLM worker crashed with SIGBUS again ({status}) even with \
                         memory-mapping already disabled — this points to a corrupt model \
                         file or an unreliable storage medium. Try: (a) re-downloading the \
                         GGUF file (partial downloads are a common cause), (b) moving the \
                         model to a stable local disk if it currently lives on a network \
                         mount / removable drive / snap or flatpak sandbox, or (c) picking \
                         a different model in Settings."
                    )));
                }
                return Err(CoreError::Other(format!(
                    "local LLM worker process crashed while generating a response ({status}) — \
                     this is almost always a native llama.cpp bug triggered by a specific model/\
                     prompt combination, not a Grafium bug as such. It has been isolated to a \
                     separate process, so the rest of the app is unaffected; a fresh worker will \
                     be started automatically for your next request."
                )));
            }

            match serde_json::from_str::<WorkerResponse>(line.trim()) {
                Ok(WorkerResponse::Token { text }) => on_token(&text),
                Ok(WorkerResponse::Done { text }) => return Ok(text),
                Ok(WorkerResponse::Error { message }) => return Err(CoreError::Other(message)),
                Ok(_) => {
                    // `Ready`/`LoadError` after startup would be a protocol
                    // violation on the worker's part — ignore rather than
                    // killing an otherwise-healthy worker over it.
                    continue;
                }
                Err(e) => {
                    return Err(CoreError::Other(format!(
                        "local LLM worker process sent an unparseable response ({}): {e}",
                        line.trim()
                    )));
                }
            }
        }
    }
}

impl LlmProvider for LocalLlmProcess {
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>> {
        let messages = messages.to_vec();
        let options = options.clone();
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || inner.run_request(&messages, &options, |_| {}))
                .await
                .map_err(|e| CoreError::Other(format!("LLM inference task panicked: {e}")))?
        })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        let messages = messages.to_vec();
        let options = options.clone();
        let inner = self.inner.clone();

        Box::pin(async move {
            // Same rationale as `LocalLlm::complete_stream`: `on_token` is
            // an arbitrary `&mut` closure the caller owns, so incremental
            // pieces are handed back through a channel from the blocking
            // thread that actually talks to the worker, rather than
            // calling `on_token` directly from there.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

            let generation = tokio::task::spawn_blocking(move || {
                inner.run_request(&messages, &options, |piece| {
                    let _ = tx.send(piece.to_string());
                })
            });

            let forward_tokens = async {
                while let Some(piece) = rx.recv().await {
                    on_token(&piece);
                }
            };

            let (result, ()) = tokio::join!(generation, forward_tokens);
            result.map_err(|e| CoreError::Other(format!("LLM inference task panicked: {e}")))?
        })
    }

    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let alive = self
                .inner
                .worker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_mut()
                .is_some_and(|h| h.child.try_wait().is_ok_and(|s| s.is_none()));
            Ok(alive)
        })
    }

    fn backend_summary(&self) -> Option<String> {
        // Only return `Some` when we actually got a backend field from
        // the worker's Ready message — an older worker binary (predating
        // this field) would yield `backend = None` here, and we'd rather
        // stay quiet in that case than show a misleading "backend:
        // unknown" line.
        self.backend.as_ref().map(|_| self.describe_backend())
    }

    fn abort_in_flight(&self) {
        // Hard-kill the worker child. This is the *only* way to
        // interrupt a llama.cpp generation: the C++ inference loop
        // checks nothing between tokens (llama-cpp-2's binding exposes
        // no abort callback we can hook into from Rust), so a
        // co-operative "please stop" signal wouldn't get read until
        // generation finished on its own — exactly the case we're
        // trying to escape.
        //
        // The Drop impl on `WorkerHandle` already does the same kill,
        // but we can't rely on drop timing here: the reader thread in
        // `run_request` is currently blocked on `read_line`, and
        // dropping the handle from *this* thread would deadlock trying
        // to acquire the same mutex. Instead we grab the mutex briefly
        // and reach directly into the child — killing it makes the
        // reader's `read_line` return `Ok(0)` or an EOF error, which
        // `run_request` already handles by dropping the (now-dead)
        // handle from its guard and surfacing an error. That error
        // gets swallowed by our own cancellation branch in
        // `stream_completion`, replaced with `CoreError::Cancelled` so
        // the caller sees "you cancelled this" rather than "the
        // worker crashed mid-generation".
        let mut guard = self
            .inner
            .worker
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(handle) = guard.as_mut() {
            let _ = handle.child.kill();
            // Deliberately don't `wait()` here: the reader thread will
            // observe stdout EOF and clean up its own guard slot,
            // spawning a fresh worker on the next request. Waiting
            // synchronously here would block the caller (which is
            // typically the async cancel branch) on an unbounded I/O
            // — the child might take a moment to actually exit after
            // SIGKILL under memory pressure.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_sigbus_matches_common_display_forms() {
        // Rust's `ExitStatus::to_string()` on Unix has drifted over
        // versions between "signal: 7 (SIGBUS) (core dumped)" and
        // shorter variants — both must match, and neither of the more
        // common SIGSEGV / non-crash exit forms should.
        assert!(status_is_sigbus("signal: 7 (SIGBUS) (core dumped)"));
        assert!(status_is_sigbus("signal: 7 (SIGBUS)"));
        assert!(status_is_sigbus("exited with signal SIGBUS"));
        assert!(!status_is_sigbus("signal: 11 (SIGSEGV) (core dumped)"));
        assert!(!status_is_sigbus("exit status: 1"));
        assert!(!status_is_sigbus("unknown exit status"));
    }

    #[test]
    fn note_worker_crashed_arms_flag_and_returns_true_first_time_only() {
        // Not exercising a real worker here — just the pure state
        // machine of `note_worker_crashed` + `respawn_use_mmap_override`,
        // so the auto-heal path can be trusted without spinning up a
        // full llama.cpp process.
        let inner = Inner {
            worker_bin: PathBuf::from("/tmp/unused"),
            models_dir: PathBuf::from("/tmp/unused"),
            settings: Mutex::new(LocalLlmSettings::default()),
            respawn_use_mmap_override: Mutex::new(None),
            worker: Mutex::new(None),
        };

        // Non-SIGBUS status: no arming, no "we just armed" signal.
        assert!(!inner.note_worker_crashed("signal: 11 (SIGSEGV) (core dumped)"));
        assert_eq!(
            *inner
                .respawn_use_mmap_override
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            None
        );

        // First SIGBUS: arms Some(false), returns true.
        assert!(inner.note_worker_crashed("signal: 7 (SIGBUS) (core dumped)"));
        assert_eq!(
            *inner
                .respawn_use_mmap_override
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
            Some(false)
        );

        // Second SIGBUS: still SIGBUS but already armed, returns false
        // so the caller can produce the "still crashing" error text
        // instead of "auto-retrying".
        assert!(!inner.note_worker_crashed("signal: 7 (SIGBUS)"));
    }

    #[test]
    fn compose_settings_json_applies_override() {
        let inner = Inner {
            worker_bin: PathBuf::from("/tmp/unused"),
            models_dir: PathBuf::from("/tmp/unused"),
            settings: Mutex::new(LocalLlmSettings {
                use_mmap: Some(true),
                ..LocalLlmSettings::default()
            }),
            respawn_use_mmap_override: Mutex::new(None),
            worker: Mutex::new(None),
        };
        // No override yet: original use_mmap=true is preserved.
        let baseline: serde_json::Value =
            serde_json::from_str(&inner.compose_settings_json().unwrap()).unwrap();
        assert_eq!(baseline["use_mmap"], serde_json::json!(true));

        // Arm override — compose should now flip it to false.
        *inner
            .respawn_use_mmap_override
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(false);
        let overridden: serde_json::Value =
            serde_json::from_str(&inner.compose_settings_json().unwrap()).unwrap();
        assert_eq!(overridden["use_mmap"], serde_json::json!(false));
    }
}
