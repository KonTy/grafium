//! Standalone worker process for local LLM inference (llama.cpp), spawned
//! by `ai::providers::local_llm_process::LocalLlmProcess`.
//!
//! # Why this exists
//! Local LLM inference via `llama-cpp-2` runs native, unmanaged C/C++ code
//! in-process. That code has a real, observed failure mode: a genuine
//! upstream bug (confirmed via llama.cpp's own issue tracker, affecting
//! new hybrid architectures like Qwen3.5/Qwen3-Next across every backend)
//! can segfault the whole process partway through generation, with no
//! `Result::Err` to catch — Rust's panic/catch_unwind machinery cannot
//! recover from a native SIGSEGV. When that code runs inside the same
//! process as the Tauri GUI, a single bad model or a not-yet-fixed
//! llama.cpp bug takes down the entire app, losing whatever else the user
//! was doing (editing a page, mid-sync, etc).
//!
//! Running the exact same inference code in this separate worker process
//! instead means a native crash only kills *this* process — the parent
//! (`LocalLlmProcess`) detects the dead child, surfaces a clean error to
//! the caller instead of vanishing, and can spawn a fresh worker for the
//! next request.
//!
//! # Protocol
//! Line-delimited JSON over stdin/stdout (never binary framing — keeps
//! this trivially debuggable by hand, e.g. `echo '{"messages":...}' | this
//! binary`). One line in, one or more lines out:
//!
//!  1. On startup: loads the model via the *exact* same
//!     [`grafium_core::ai::providers::local_llm::LocalLlm::from_settings`]
//!     used previously in-process (unchanged — same RAM/VRAM budgeting,
//!     same known-unstable-architecture rejection, same fallback-model
//!     search), then writes exactly one line to stdout: `{"type":"ready"}`
//!     on success, or `{"type":"load_error","message":"..."}` (then exits
//!     with a non-zero status) on failure.
//!  2. Then, once per line read from stdin (each a
//!     [`WorkerRequest`]), streams zero or more `{"type":"token",...}`
//!     lines followed by exactly one `{"type":"done",...}` or
//!     `{"type":"error",...}` line.
//!  3. Stdin EOF (parent dropped its handle, or exited) ends the loop and
//!     this process exits cleanly.
//!
//! Every line is flushed immediately — the parent relies on seeing partial
//! progress (and the process still being alive) even mid-generation, and
//! there being no ambiguity about what was actually sent before a crash
//! (see `local_llm.rs`'s own logging, which relies on the same flush
//! behavior of a non-TTY stdout).

use std::io::{BufRead, Write};

use grafium_core::ai::config::LocalLlmSettings;
use grafium_core::ai::providers::local_llm::LocalLlm;
use grafium_core::ai::providers::local_llm_process::{WorkerRequest, WorkerResponse};
use grafium_core::ai::traits::LlmProvider;

fn write_line(response: &WorkerResponse) {
    let mut stdout = std::io::stdout().lock();
    // A malformed response can't be allowed to silently vanish (that would
    // look identical to "the process crashed" from the parent's point of
    // view) — but serializing our own well-typed enum should never
    // actually fail, so this is just defensive.
    if let Ok(line) = serde_json::to_string(response) {
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
    }
}

/// Summary of which backend the LLM ended up on, per what llama.cpp /
/// GGML told us during load. Fed back to the parent via
/// [`WorkerResponse::Ready`] so the UI can display "your LLM is on
/// Vulkan GPU (AMD Radeon)" / "your LLM fell back to CPU" before the
/// first inference call.
struct LlmBackendInfo {
    backend: String,
    device: Option<String>,
    reason: Option<String>,
}

/// Inspects the [`grafium_core::log_tap`] events captured during model
/// load and decides which backend won. Same idea as the whisper.cpp
/// backend detector in `media::transcribe::detect_backend_from_log`:
/// llama.cpp doesn't return this via any Rust-side API — we have to
/// read what it printed to its own log.
fn detect_llm_backend(load_start: std::time::Instant) -> LlmBackendInfo {
    let events = grafium_core::log_tap::snapshot_since_targets(
        load_start,
        &["llama", "ggml"],
    );
    let mut saw_vulkan = false;
    let mut saw_cuda = false;
    let mut saw_metal = false;
    let mut device: Option<String> = None;
    let mut failure_reason: Option<String> = None;

    for ev in &events {
        let msg = &ev.message;
        let ml = msg.to_lowercase();

        // Positive markers — llama.cpp / GGML log these when a GPU
        // backend actually initialized. "found N Vulkan devices" and
        // per-device lines like "Vulkan0: <device>" are the two most
        // reliable signals.
        if ml.contains("vulkan") {
            if ml.contains("found") && ml.contains("device") {
                saw_vulkan = true;
            }
            if ml.starts_with("vulkan") && ml.contains(":") {
                saw_vulkan = true;
                if let Some(after) = msg.splitn(2, ':').nth(1) {
                    let trimmed = after.trim();
                    let name = trimmed.split('|').next().unwrap_or(trimmed).trim();
                    if !name.is_empty() {
                        device = Some(name.to_string());
                    }
                }
            }
            if ml.contains("no supported")
                || ml.contains("failed to init")
                || ml.contains("no vulkan")
                || ml.contains("device not found")
            {
                failure_reason = Some(msg.clone());
            }
        }
        if ml.contains("cuda") && (ml.contains("device") || ml.contains("found")) {
            saw_cuda = true;
            if device.is_none() {
                device = Some(msg.clone());
            }
        }
        if ml.contains("metal") && ml.contains("device") {
            saw_metal = true;
            if device.is_none() {
                device = Some(msg.clone());
            }
        }
        if ml.contains("cpu backend") || ml.contains("using cpu") {
            if failure_reason.is_none() {
                failure_reason = Some(msg.clone());
            }
        }
    }

    if let Some(reason) = failure_reason {
        return LlmBackendInfo {
            backend: "cpu".to_string(),
            device: None,
            reason: Some(reason),
        };
    }
    if saw_vulkan {
        return LlmBackendInfo {
            backend: "vulkan".to_string(),
            device,
            reason: None,
        };
    }
    if saw_cuda {
        return LlmBackendInfo {
            backend: "cuda".to_string(),
            device,
            reason: None,
        };
    }
    if saw_metal {
        return LlmBackendInfo {
            backend: "metal".to_string(),
            device,
            reason: None,
        };
    }
    LlmBackendInfo {
        backend: "cpu".to_string(),
        device: None,
        reason: Some(
            "no GPU backend initialization was reported by llama.cpp — this build \
             may not have a GPU backend compiled in, or the log filter dropped \
             the backend init lines (try RUST_LOG=llama=debug,ggml=debug to see \
             what llama.cpp actually said at load time)"
                .to_string(),
        ),
    }
}

fn main() {
    // Wire the log tap alongside the fmt layer, exactly like
    // `ui/src-tauri/src/lib.rs::run` does — so the worker's own
    // `LocalLlm::from_settings` (which runs llama.cpp / GGML init
    // in-process) captures each tracing event into
    // [`grafium_core::log_tap`]'s ring buffer, letting us detect
    // which backend the loader ended up on and report it back to
    // the parent as part of the Ready message.
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    struct LogTapLayer;
    impl<S> tracing_subscriber::Layer<S> for LogTapLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            use grafium_core::log_tap::{record, TapLevel};
            struct V(String);
            impl tracing::field::Visit for V {
                fn record_debug(
                    &mut self,
                    f: &tracing::field::Field,
                    v: &dyn std::fmt::Debug,
                ) {
                    if f.name() == "message" {
                        self.0 = format!("{v:?}");
                    }
                }
                fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
                    if f.name() == "message" {
                        self.0 = v.to_string();
                    }
                }
            }
            let mut visitor = V(String::new());
            event.record(&mut visitor);
            let level = match *event.metadata().level() {
                tracing::Level::TRACE => TapLevel::Trace,
                tracing::Level::DEBUG => TapLevel::Debug,
                tracing::Level::INFO => TapLevel::Info,
                tracing::Level::WARN => TapLevel::Warn,
                tracing::Level::ERROR => TapLevel::Error,
            };
            record(level, event.metadata().target(), &visitor.0);
        }
    }
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(LogTapLayer)
        .init();

    let mut args = std::env::args().skip(1);
    let models_dir = match args.next() {
        Some(dir) => std::path::PathBuf::from(dir),
        None => {
            eprintln!("usage: grafium-llm-worker <models_dir> <settings_json>");
            std::process::exit(2);
        }
    };
    let settings_json = match args.next() {
        Some(json) => json,
        None => {
            eprintln!("usage: grafium-llm-worker <models_dir> <settings_json>");
            std::process::exit(2);
        }
    };
    let settings: LocalLlmSettings = match serde_json::from_str(&settings_json) {
        Ok(s) => s,
        Err(e) => {
            write_line(&WorkerResponse::LoadError {
                message: format!("invalid settings JSON passed to worker: {e}"),
            });
            std::process::exit(1);
        }
    };

    let load_start = std::time::Instant::now();
    let llm = match LocalLlm::from_settings(&models_dir, &settings) {
        Ok(llm) => llm,
        Err(e) => {
            write_line(&WorkerResponse::LoadError {
                message: e.to_string(),
            });
            std::process::exit(1);
        }
    };
    let load_seconds = load_start.elapsed().as_secs_f64();
    // Inspect the tap for llama.cpp / GGML log lines emitted during
    // load and decide which backend the model actually landed on.
    // llama.cpp doesn't return that information via any API — it
    // prints it during init and silently falls back to CPU on GPU
    // failure — so parsing the log is the *only* way to tell.
    let backend_info = detect_llm_backend(load_start);
    write_line(&WorkerResponse::Ready {
        name: llm.name().to_string(),
        backend: Some(backend_info.backend),
        backend_device: backend_info.device,
        backend_reason: backend_info.reason,
        load_seconds: Some(load_seconds),
    });

    // A single-threaded runtime is enough: `LlmProvider::complete_stream`
    // itself hands the actual (CPU-bound) inference off to
    // `tokio::task::spawn_blocking`, which uses tokio's separate blocking
    // thread pool regardless of the runtime's own worker-thread count —
    // this worker only ever handles one request at a time anyway (stdin is
    // read one line at a time, synchronously), so there's nothing for
    // extra async worker threads to do.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            write_line(&WorkerResponse::LoadError {
                message: format!("failed to start async runtime: {e}"),
            });
            std::process::exit(1);
        }
    };

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break }; // stdin closed/errored — exit.
        if line.trim().is_empty() {
            continue;
        }
        let request: WorkerRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_line(&WorkerResponse::Error {
                    message: format!("invalid request JSON: {e}"),
                });
                continue;
            }
        };

        let result = rt.block_on(async {
            let mut on_token = |piece: &str| {
                write_line(&WorkerResponse::Token {
                    text: piece.to_string(),
                });
            };
            llm.complete_stream(&request.messages, &request.options, &mut on_token)
                .await
        });

        match result {
            Ok(text) => write_line(&WorkerResponse::Done { text }),
            Err(e) => write_line(&WorkerResponse::Error {
                message: e.to_string(),
            }),
        }
    }
}
