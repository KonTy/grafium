//! Local speech-to-text via whisper.cpp (through the `whisper-rs` bindings).
//!
//! Deliberately mirrors `ai::traits::LlmProvider`'s shape: a small trait
//! (`Transcriber`) so callers (summarization, fact-checking, a future CLI
//! command) depend on an abstraction rather than whisper.cpp directly. That
//! keeps the door open for a cloud STT backend later without touching any
//! call site — just a new `impl Transcriber`.
//!
//! Gated behind the `media` Cargo feature (see `core/Cargo.toml`) since it
//! pulls in a C++ build of whisper.cpp; enable `media-vulkan` too to offload
//! inference to a GPU via Vulkan (works with just the Vulkan loader + GPU
//! driver already installed, no CUDA toolkit required).

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::{CoreError, Result};
use crate::media::types::{Transcript, TranscriptSegment};
use crate::model_library::{self, LocalModelRef, ModelKind};

/// Anything that can turn a 16kHz mono WAV file (exactly what
/// `media::ingest::fetch_audio` produces) into a [`Transcript`].
pub trait Transcriber {
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript> {
        self.transcribe_with_progress(wav_path, &mut |_| {})
    }

    /// Same as [`Self::transcribe`], but reports periodic progress
    /// (0-100 percent + a human-readable message) so a caller can show
    /// live status during what can be a multi-minute inference pass.
    ///
    /// The default implementation forwards to [`Self::transcribe`] and
    /// ignores the callback; a real implementation (like
    /// [`WhisperTranscriber`]) should override this so the user isn't
    /// staring at a silent progress dialog.
    fn transcribe_with_progress(
        &self,
        wav_path: &Path,
        _on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        self.transcribe(wav_path)
    }
}

/// Progress update from a running transcription pass. `percent` is a
/// best-effort 0-100 completion estimate, and `message` is a
/// human-readable label the UI can show verbatim.
#[derive(Debug, Clone)]
pub struct TranscribeProgress {
    pub percent: u8,
    pub message: String,
}

/// Which compute backend whisper.cpp actually ended up using once its
/// context was created — determined by inspecting the tracing/log output
/// GGML emitted during init (`log_tap::snapshot_since_targets`), not by
/// asking the caller. Surfaced to the user via the media-import progress
/// UI so a "GPU didn't init, falling back to CPU" state is *visible*
/// rather than an unexplained slowdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhisperBackend {
    /// GPU-accelerated via the Vulkan backend of GGML.
    Vulkan {
        /// The device name whisper.cpp reported, if we could parse one
        /// out of the init log (e.g. "AMD Radeon RX 7900 XTX"). `None`
        /// just means we couldn't parse a specific name from the log
        /// line — GPU is still on.
        device: Option<String>,
    },
    /// GGML fell back to the CPU backend. Either the user's build doesn't
    /// have a working GPU backend, `use_gpu(true)` failed to find a
    /// device, or Vulkan init errored out; whisper.cpp handles all three
    /// by silently using CPU, so the only signal to the user is this
    /// enum plus the accompanying `reason` string.
    Cpu { reason: String },
}

impl WhisperBackend {
    /// Short user-facing label suitable for a status line.
    pub fn label(&self) -> String {
        match self {
            WhisperBackend::Vulkan { device: Some(d) } => format!("Vulkan GPU ({d})"),
            WhisperBackend::Vulkan { device: None } => "Vulkan GPU".to_string(),
            WhisperBackend::Cpu { .. } => "CPU".to_string(),
        }
    }

    /// The reason CPU fallback occurred, if any. Empty for the GPU path.
    pub fn fallback_reason(&self) -> Option<&str> {
        match self {
            WhisperBackend::Cpu { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}

/// Runs whisper.cpp locally against a `ggml`/`gguf` Whisper model file.
/// Stateless per call beyond the loaded model, so one instance can be reused
/// across many `transcribe()` calls (avoids re-loading the model each time).
pub struct WhisperTranscriber {
    ctx: WhisperContext,
    language: Option<String>,
    /// Which backend whisper.cpp *actually* ended up on — determined at
    /// load time by inspecting the GGML init log via `log_tap`. Exposed
    /// through [`Self::backend`] so callers can show "using GPU" /
    /// "fell back to CPU" up-front instead of leaving the user
    /// guessing why transcription is slow.
    backend: WhisperBackend,
}

impl WhisperTranscriber {
    /// Loads a `ggml-*.bin` model from `model_path`. Pass `language` (e.g.
    /// `"en"`) to force transcription in that language, or `None` to let
    /// whisper.cpp auto-detect it.
    pub fn load(model_path: &Path, language: Option<&str>) -> Result<Self> {
        // whisper.cpp/GGML log verbosely to stderr by default, which would
        // corrupt a raw-mode terminal (e.g. the TUI). We enable
        // whisper-rs's `tracing_backend` in Cargo.toml so those messages
        // route through `tracing` instead — this keeps stdout/stderr
        // clean for the TUI *and* keeps whisper.cpp's actual error
        // messages (e.g. "unable to init vulkan device", "invalid model
        // file") available in the log so a bare "failed to load whisper
        // model: Failed to create a new whisper context" is diagnosable
        // instead of a dead end.
        static INSTALL_LOGGING_HOOKS: std::sync::Once = std::sync::Once::new();
        INSTALL_LOGGING_HOOKS.call_once(whisper_rs::install_logging_hooks);

        // Verify the model file up front — `whisper_init_from_file` just
        // returns a generic "context is null" through the bindings when
        // the file doesn't exist or is unreadable, which surfaces to the
        // user as the same opaque "Failed to create a new whisper
        // context" as an actual whisper.cpp init failure. Distinguishing
        // them at this layer saves a debugging round-trip.
        if !model_path.exists() {
            return Err(CoreError::Other(format!(
                "whisper model file does not exist: {}",
                model_path.display()
            )));
        }

        let mut params = WhisperContextParameters::default();
        // Explicitly opt into GPU offload — the `whisper-rs/vulkan`
        // Cargo feature already sets `use_gpu = true` in the default,
        // but stating it here means "GPU on" doesn't silently regress
        // if that default ever changes upstream, and makes the intent
        // grep-able from a debug log ("why isn't my GPU being used?").
        params.use_gpu(true);
        // flash-attn shrinks whisper's KV cache activations and gives a
        // measurable speedup with negligible accuracy loss on Vulkan/CUDA
        // (this is the same knob whisper.cpp's own examples default to
        // when a GPU is available). No-op on the CPU backend so it's
        // safe to leave on unconditionally.
        params.flash_attn(true);

        // Snapshot the tap so we can attribute *only* the events GGML
        // emits during this specific `new_with_params` call to *this*
        // load — even if another whisper/llama context is loading
        // concurrently (which it is, in the LLM subprocess), events
        // from before this instant aren't ours to interpret.
        let load_start = std::time::Instant::now();

        let ctx = WhisperContext::new_with_params(
            &*model_path.to_string_lossy(),
            params,
        )
        .map_err(|e| {
            // Include any whisper/GGML log lines from *this* load so
            // the surface error carries the actual cause — e.g.
            // "ggml_vulkan: no supported devices found" — instead of
            // just a generic "Failed to create a new whisper context".
            let init_log = format_tap_events(&crate::log_tap::snapshot_since_targets(
                load_start,
                &["whisper", "ggml"],
            ));
            let details = if init_log.is_empty() {
                String::new()
            } else {
                format!("\n\nWhisper/GGML log:\n{init_log}")
            };
            CoreError::Other(format!(
                "failed to load whisper model at {}: {e} — check the log for the underlying \
                 whisper.cpp/GGML error message (e.g. Vulkan device init failure, invalid \
                 GGUF file, or out-of-memory){details}",
                model_path.display()
            ))
        })?;

        let backend = detect_backend_from_log(&crate::log_tap::snapshot_since_targets(
            load_start,
            &["whisper", "ggml"],
        ));

        Ok(Self {
            ctx,
            language: language.map(str::to_string),
            backend,
        })
    }

    /// Which compute backend whisper.cpp actually ended up on. See
    /// [`WhisperBackend`] for how this is determined.
    pub fn backend(&self) -> &WhisperBackend {
        &self.backend
    }

    /// Settings-driven alternative to [`Self::load`]: resolves which model
    /// file to use via the shared [`model_library`] (through `model_ref`)
    /// instead of requiring an exact path up front. This is what makes
    /// "download a model from Hugging Face, put it in the models folder,
    /// it just works" actually work: nothing has to be typed into Settings
    /// at all unless there's more than one Whisper model to choose between.
    pub fn from_settings(
        models_dir: &Path,
        model_ref: &LocalModelRef,
        language: Option<&str>,
    ) -> Result<Self> {
        let model_path = model_ref.resolve(models_dir, ModelKind::Whisper)?;
        Self::load(&model_path, language)
    }

    /// Same as [`Self::from_settings`], but takes the whole [`MediaConfig`]
    /// + app data dir instead of pre-extracted fields — the shape a caller
    /// loading settings straight from disk (e.g. the Tauri command layer,
    /// mirroring `ai_get_config`/`ai_set_config`) will actually have on hand.
    pub fn from_config(config: &crate::media::MediaConfig, data_dir: &Path) -> Result<Self> {
        let models_dir = config
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        Self::from_settings(
            &models_dir,
            &config.whisper.model_ref,
            config.whisper.language.as_deref(),
        )
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe_with_progress(
        &self,
        wav_path: &Path,
        on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        let samples = read_wav_as_f32_mono(wav_path)?;
        // 16kHz mono samples → seconds. Reported up front so the UI
        // can show "3:24 of audio" rather than a completely opaque
        // "Transcribing…" that could mean 10 seconds or 10 minutes.
        let duration_secs = samples.len() as f64 / 16_000.0;
        // Backend prefix that shows up in every progress message so
        // the user can see at a glance whether whisper is on the GPU
        // (fast) or fell back to CPU (much slower) — and, if it fell
        // back, *why* (the reason string comes from the actual GGML
        // log line via `detect_backend_from_log`).
        let backend_label = self.backend.label();
        if let Some(reason) = self.backend.fallback_reason() {
            on_progress(TranscribeProgress {
                percent: 0,
                message: format!(
                    "⚠ Whisper is running on {backend_label}: {reason}\n\
                     Transcription will be significantly slower than on a GPU. \
                     Install/enable a Vulkan-capable GPU driver, or pick a smaller \
                     Whisper model (e.g. base/small) in Settings, if this is too slow."
                ),
            });
        }
        on_progress(TranscribeProgress {
            percent: 0,
            message: format!(
                "Transcribing {} of audio with Whisper on {}…",
                format_duration(duration_secs),
                backend_label,
            ),
        });

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| CoreError::Other(format!("failed to create whisper state: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        if let Some(lang) = &self.language {
            params.set_language(Some(lang.as_str()));
        }

        // whisper.cpp calls this callback with an integer percent
        // (0-100) at regular intervals during inference. Without it,
        // `state.full(...)` blocks for the *entire* transcription with
        // no indication of progress — which is what makes a long video
        // feel indistinguishable from a hung process.
        //
        // The `set_progress_callback_safe` API takes an `FnMut(i32) +
        // 'static`, but our caller-supplied `on_progress` is only borrowed
        // for the duration of this call. We widen its lifetime to
        // `'static` via `mem::transmute` and rely on the fact that
        // whisper.cpp only ever invokes the callback synchronously from
        // inside `state.full(...)` on this same thread — so the
        // reference is live for every invocation. whisper-rs's
        // `set_progress_callback_safe` leaks the closure box internally
        // (see its source), so there's no cross-thread aliasing after
        // this function returns either.
        let duration_label = format_duration(duration_secs);
        let duration_label_cb = duration_label.clone();
        let backend_label_cb = backend_label.clone();
        // SAFETY: see the paragraph above — the transmuted reference is
        // only dereferenced during `state.full(...)`, still on this
        // thread, and never after this function returns.
        let on_progress_static: &'static mut dyn FnMut(TranscribeProgress) =
            unsafe { std::mem::transmute(on_progress) };
        params.set_progress_callback_safe(move |percent: i32| {
            let clamped = percent.clamp(0, 100) as u8;
            on_progress_static(TranscribeProgress {
                percent: clamped,
                message: format!(
                    "Transcribing {} of audio with Whisper on {}… {}%",
                    duration_label_cb, backend_label_cb, clamped
                ),
            });
        });

        let run_start = std::time::Instant::now();
        state.full(params, &samples).map_err(|e| {
            // Include any whisper/GGML log lines from *this* run so the
            // surface error carries the actual cause verbatim (e.g. a
            // GPU device-lost/hang message, or a KV-cache OOM), instead
            // of being reduced to a generic "whisper transcription
            // failed".
            let run_log = format_tap_events(&crate::log_tap::snapshot_since_targets(
                run_start,
                &["whisper", "ggml"],
            ));
            let details = if run_log.is_empty() {
                String::new()
            } else {
                format!("\n\nWhisper/GGML log:\n{run_log}")
            };
            CoreError::Other(format!("whisper transcription failed: {e}{details}"))
        })?;

        let num_segments = state.full_n_segments();

        let mut segments = Vec::with_capacity(num_segments as usize);
        let mut full_text = String::new();
        for i in 0..num_segments {
            let Some(segment) = state.get_segment(i) else {
                continue;
            };
            let text = segment
                .to_str()
                .map_err(|e| CoreError::Other(format!("failed to get segment text: {e}")))?
                .trim()
                .to_string();
            // whisper.cpp timestamps are in centiseconds.
            let start_ms = segment.start_timestamp() * 10;
            let end_ms = segment.end_timestamp() * 10;

            if !full_text.is_empty() {
                full_text.push(' ');
            }
            full_text.push_str(&text);
            segments.push(TranscriptSegment {
                start_ms,
                end_ms,
                text,
            });
        }

        Ok(Transcript {
            segments,
            full_text,
        })
    }
}

/// Interprets whisper.cpp/GGML init log lines to figure out which backend
/// actually ended up running. GGML doesn't return this via any API — it
/// prints it to stderr and silently falls back to CPU on GPU init failure
/// — so parsing the log is the *only* way to distinguish "GPU is on"
/// from "GPU asked for, GPU not found, CPU quietly used" post-hoc.
///
/// Heuristic:
/// - "found N Vulkan devices" / "Vulkan0: <name>" / "using Vulkan backend" → Vulkan
/// - "no supported ... devices" / "no ... found" / "failed to init" → CPU with reason
/// - No Vulkan-related line at all → CPU with a generic reason (probably
///   a CPU-only build, or logs are below the current filter level).
fn detect_backend_from_log(events: &[crate::log_tap::TapEvent]) -> WhisperBackend {
    let mut saw_vulkan_success = false;
    let mut vulkan_device: Option<String> = None;
    let mut failure_reason: Option<String> = None;
    let mut saw_any_backend_line = false;

    for ev in events {
        let msg_lower = ev.message.to_lowercase();

        if msg_lower.contains("vulkan") {
            saw_any_backend_line = true;

            // Positive markers — whisper.cpp / GGML logs a mix of
            // "found N Vulkan devices" (during device enumeration) and
            // "Vulkan0: <device name> ..." (per-device). We flag success
            // when either appears, and try to grab the device name if
            // we can, but a missing device name isn't a failure.
            if msg_lower.contains("found") && msg_lower.contains("device") {
                saw_vulkan_success = true;
            }
            if msg_lower.starts_with("vulkan") && msg_lower.contains(":") {
                saw_vulkan_success = true;
                // Try to extract a device name from lines like
                // "Vulkan0: AMD Radeon RX 7900 XTX (RADV NAVI31) | uma: 0 | ..."
                if let Some(after_colon) = ev.message.splitn(2, ':').nth(1) {
                    let trimmed = after_colon.trim();
                    let name = trimmed.split('|').next().unwrap_or(trimmed).trim();
                    if !name.is_empty() {
                        vulkan_device = Some(name.to_string());
                    }
                }
            }

            // Negative markers — whisper.cpp / GGML log any of these on
            // failure. Anything matching these overrides an earlier
            // "success" (a partial init followed by a failure means
            // CPU fallback in whisper.cpp).
            if msg_lower.contains("no supported")
                || msg_lower.contains("no vulkan")
                || msg_lower.contains("failed to init")
                || msg_lower.contains("failed to create")
                || msg_lower.contains("device not found")
                || msg_lower.contains("no gpu")
            {
                failure_reason = Some(ev.message.clone());
            }
        }

        // Even a plain "using CPU backend" from GGML/whisper is a signal.
        if msg_lower.contains("cpu backend") || msg_lower.contains("using cpu") {
            saw_any_backend_line = true;
            if failure_reason.is_none() {
                failure_reason = Some(ev.message.clone());
            }
        }
    }

    if let Some(reason) = failure_reason {
        return WhisperBackend::Cpu { reason };
    }
    if saw_vulkan_success {
        return WhisperBackend::Vulkan {
            device: vulkan_device,
        };
    }
    if !saw_any_backend_line {
        // No GGML/whisper backend lines at all. Most likely the log
        // filter is above `info` (so we didn't see them) *or* this
        // build has no GPU backend compiled in. Either way, whisper
        // is on CPU as far as the user is concerned.
        return WhisperBackend::Cpu {
            reason: "no GPU backend initialization was reported by whisper.cpp — this build \
                     may not have a GPU backend compiled in, or the log level is filtering \
                     out backend messages (try setting RUST_LOG=whisper=debug,ggml=debug)"
                .to_string(),
        };
    }
    // Backend lines were seen but none matched a success pattern — treat as CPU.
    WhisperBackend::Cpu {
        reason: "whisper.cpp did not report a working GPU backend during init".to_string(),
    }
}

/// Joins tap events into a compact multi-line block suitable for appending
/// to an error message — one line per event, prefixed with the level so
/// the reader can eyeball severity.
fn format_tap_events(events: &[crate::log_tap::TapEvent]) -> String {
    events
        .iter()
        .map(|ev| format!("  [{:?} {}] {}", ev.level, ev.target, ev.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Turns a duration in seconds into a compact `m:ss`/`h:mm:ss` label the
/// UI can show verbatim (e.g. "0:42", "3:24", "1:05:17"). Used only for
/// progress reporting — precision beyond the second isn't useful when
/// the user is watching a status line update.
fn format_duration(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Decodes a 16-bit PCM mono WAV file into the `f32` samples whisper.cpp
/// expects, using `whisper-rs`'s own conversion helper rather than
/// reimplementing PCM normalization.
fn read_wav_as_f32_mono(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| CoreError::Other(format!("failed to open WAV {}: {e}", path.display())))?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(CoreError::Other(format!(
            "expected mono WAV, got {} channels — was it normalized via ingest::fetch_audio?",
            spec.channels
        )));
    }

    let samples: std::result::Result<Vec<i16>, _> = reader.samples::<i16>().collect();
    let samples =
        samples.map_err(|e| CoreError::Other(format!("failed reading WAV samples: {e}")))?;

    let mut floats = vec![0.0f32; samples.len()];
    whisper_rs::convert_integer_to_float_audio(&samples, &mut floats)
        .map_err(|e| CoreError::Other(format!("failed to convert audio samples: {e}")))?;
    Ok(floats)
}

#[cfg(test)]
mod backend_detection_tests {
    //! `detect_backend_from_log` is what tells the user whether Whisper
    //! is going to be fast (GPU) or slow (CPU) — a bug here would silently
    //! mislabel the two, so cover the actual whisper.cpp/GGML log
    //! phrasings we've observed in practice (see `TODO.md`'s notes on
    //! Vulkan init + the traces gathered while debugging the null-context
    //! crash).
    use super::{detect_backend_from_log, WhisperBackend};
    use crate::log_tap::{TapEvent, TapLevel};
    use std::time::Instant;

    fn ev(level: TapLevel, target: &str, message: &str) -> TapEvent {
        TapEvent {
            at: Instant::now(),
            level,
            target: target.to_string(),
            message: message.to_string(),
        }
    }

    #[test]
    fn detects_vulkan_from_found_devices_line() {
        let events = vec![
            ev(TapLevel::Info, "ggml_vulkan", "found 1 Vulkan devices:"),
            ev(
                TapLevel::Info,
                "ggml_vulkan",
                "Vulkan0: AMD Radeon RX 7900 XTX (RADV NAVI31) | uma: 0 | fp16: 1",
            ),
        ];
        let backend = detect_backend_from_log(&events);
        match backend {
            WhisperBackend::Vulkan { device } => {
                assert_eq!(device.as_deref(), Some("AMD Radeon RX 7900 XTX (RADV NAVI31)"));
            }
            other => panic!("expected Vulkan backend, got {other:?}"),
        }
    }

    #[test]
    fn detects_cpu_fallback_from_no_supported_devices() {
        // The "success line was seen but then a failure came after" case:
        // the failure marker must override so we don't mis-label as GPU.
        let events = vec![
            ev(TapLevel::Info, "ggml_vulkan", "found 0 Vulkan devices:"),
            ev(
                TapLevel::Warn,
                "ggml_vulkan",
                "ggml_vulkan: no supported devices found",
            ),
        ];
        let backend = detect_backend_from_log(&events);
        match backend {
            WhisperBackend::Cpu { reason } => {
                assert!(
                    reason.contains("no supported devices"),
                    "expected raw log line in reason, got: {reason}"
                );
            }
            other => panic!("expected CPU fallback, got {other:?}"),
        }
    }

    #[test]
    fn labels_cpu_when_no_backend_lines_at_all() {
        // No whisper/ggml lines in the log — we should still fall back
        // to CPU with a useful reason, not silently claim GPU.
        let events: Vec<TapEvent> = Vec::new();
        let backend = detect_backend_from_log(&events);
        match backend {
            WhisperBackend::Cpu { reason } => {
                assert!(!reason.is_empty(), "reason should not be empty");
            }
            other => panic!("expected CPU fallback, got {other:?}"),
        }
    }
}
