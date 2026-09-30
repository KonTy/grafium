//! Local speech-to-text via whisper.cpp (through the `whisper-rs` bindings).
//!
//! Deliberately mirrors `ai::traits::LlmProvider`'s shape: a small trait
//! (`Transcriber`) so callers (summarization, fact-checking, a future CLI
//! command) depend on an abstraction rather than whisper.cpp directly. That
//! keeps the door open for a cloud STT backend later without touching any
//! call site — just a new `impl Transcriber`.
//!
//! Gated behind this crate's `media` Cargo feature since it
//! pulls in a C++ build of whisper.cpp; enable `media-vulkan` too to offload
//! inference to a GPU via Vulkan (works with just the Vulkan loader + GPU
//! driver already installed, no CUDA toolkit required).

use std::path::Path;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::{Result, RuntimeError};
use crate::transcription::{Transcript, TranscriptSegment};

pub use crate::transcription::{TranscribeProgress, Transcriber};

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
/// A loaded whisper model, resident in the worker child between requests.
///
/// Mirrors `local_llm::LlmSlot`. Loading reads a multi-hundred-megabyte file,
/// so keeping it across imports is what makes a second transcription quick.
pub(crate) struct WhisperSlot {
    model_path: std::path::PathBuf,
    language: Option<String>,
    transcriber: WhisperTranscriber,
}

impl WhisperSlot {
    fn matches(&self, model_path: &Path, language: Option<&str>) -> bool {
        self.model_path == model_path && self.language.as_deref() == language
    }
}

/// Transcribe in the child. Runs only in the worker process.
///
/// Native faults are isolated from the application. A GPU driver/kernel failure
/// can still affect the whole system, so GPU admission happens before loading.
pub(crate) fn transcribe_in_process(
    slot: &mut Option<WhisperSlot>,
    model_path: &Path,
    language: Option<&str>,
    wav_path: &Path,
    on_progress: &mut dyn FnMut(TranscribeProgress),
) -> Result<Transcript> {
    if !slot
        .as_ref()
        .is_some_and(|c| c.matches(model_path, language))
    {
        // Release the previous model before loading another.
        *slot = None;
        *slot = Some(WhisperSlot {
            model_path: model_path.to_path_buf(),
            language: language.map(str::to_string),
            transcriber: WhisperTranscriber::load(model_path, language)?,
        });
    }
    slot.as_ref()
        .expect("slot populated above")
        .transcriber
        .transcribe_with_progress(wav_path, on_progress)
}

/// Low-level in-process engine for worker dispatch and offline diagnostics.
/// Application hosts should use [`WorkerTranscriber`] to retain isolation.
pub struct WhisperTranscriber {
    ctx: WhisperContext,
    language: Option<String>,
    /// Which backend whisper.cpp *actually* ended up on — determined at
    /// load time by inspecting the GGML init log via `log_tap`. Exposed
    /// through [`Self::backend`] so callers can show "using GPU" /
    /// "fell back to CPU" up-front instead of leaving the user
    /// guessing why transcription is slow.
    backend: WhisperBackend,
    #[cfg(not(feature = "llm-local"))]
    gpu_context_budget: u64,
    #[cfg(feature = "llm-local")]
    device: Option<crate::native::native_gpu::Device>,
    _model_file: super::model_file::LockedModelFile,
}

impl WhisperTranscriber {
    /// Loads a `ggml-*.bin` model from `model_path`. Pass `language` (e.g.
    /// `"en"`) to force transcription in that language, or `None` to let
    /// whisper.cpp auto-detect it.
    pub fn load(model_path: &Path, language: Option<&str>) -> Result<Self> {
        let model_file = super::model_file::LockedModelFile::acquire(model_path)?;
        let canonical = model_file.path().to_path_buf();
        let model_path = canonical.as_path();
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
            return Err(RuntimeError::Other(format!(
                "whisper model file does not exist: {}",
                model_path.display()
            )));
        }

        let mut params = WhisperContextParameters::default();
        crate::native::policy::validate_model_load(
            model_path,
            crate::native::policy::ModelWorkload::Whisper,
        )?;
        #[cfg(feature = "llm-local")]
        let mut device = crate::native::native_gpu::speech_device(model_path)?;
        #[cfg(feature = "llm-local")]
        let gpu_layers = u32::from(device.is_some());
        #[cfg(feature = "llm-local")]
        if let Some(device) = &device {
            params.gpu_device(device.whisper_index);
        }
        #[cfg(not(feature = "llm-local"))]
        let gpu_layers = crate::native::policy::admitted_gpu_layers(
            model_path,
            crate::native::policy::ModelWorkload::Whisper,
            u32::from(cfg!(feature = "media-vulkan")),
        )?;
        params.use_gpu(gpu_layers > 0);
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

        let ctx = WhisperContext::new_with_params(&*model_path.to_string_lossy(), params).map_err(
            |e| {
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
                RuntimeError::Other(format!(
                    "failed to load whisper model at {}: {e} — check the log for the underlying \
                 whisper.cpp/GGML error message (e.g. Vulkan device init failure, invalid \
                 GGUF file, or out-of-memory){details}",
                    model_path.display()
                ))
            },
        )?;

        let backend = if gpu_layers == 0 {
            WhisperBackend::Cpu {
                reason:
                    "CPU selected by the runtime's GPU admission policy or build configuration."
                        .into(),
            }
        } else {
            detect_backend_from_log(&crate::log_tap::snapshot_since_targets(
                load_start,
                &["whisper", "ggml"],
            ))
        };
        #[cfg(feature = "llm-local")]
        if let Some(device) = &mut device {
            device.reserve_context()?;
        }

        super::worker::emit_resident(super::worker::ResidentModel {
            kind: super::worker::NativeModelKind::Transcription,
            model_path: model_path.to_path_buf(),
            context_size: None,
            on_gpu: matches!(&backend, WhisperBackend::Vulkan { .. }),
            gpu_layers: None,
        });

        Ok(Self {
            ctx,
            language: language.map(str::to_string),
            backend,
            #[cfg(not(feature = "llm-local"))]
            gpu_context_budget: std::fs::metadata(model_path)?.len(),
            #[cfg(feature = "llm-local")]
            device,
            _model_file: model_file,
        })
    }

    /// Which compute backend whisper.cpp actually ended up on. See
    /// [`WhisperBackend`] for how this is determined.
    pub fn backend(&self) -> &WhisperBackend {
        &self.backend
    }
}

impl Transcriber for WhisperTranscriber {
    fn transcribe_with_progress(
        &self,
        wav_path: &Path,
        on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        crate::native::policy::validate_audio_buffer(wav_path)?;
        #[cfg(feature = "llm-local")]
        if let Some(device) = &self.device {
            device.check_context()?;
        }
        #[cfg(not(feature = "llm-local"))]
        if matches!(self.backend, WhisperBackend::Vulkan { .. }) {
            crate::native::policy::validate_gpu_context_headroom(self.gpu_context_budget)?;
        }
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
            .map_err(|e| RuntimeError::Other(format!("failed to create whisper state: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        #[cfg(feature = "llm-local")]
        let mut pressure = crate::native::native_gpu::PressureWatch::new(self.device.clone());
        let stopped = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let stop_reason = stopped.clone();
        let mut last_check = std::time::Instant::now() - std::time::Duration::from_secs(1);
        params.set_abort_callback_safe(move || {
            if last_check.elapsed() < std::time::Duration::from_millis(500) {
                return false;
            }
            last_check = std::time::Instant::now();
            #[cfg(feature = "llm-local")]
            let error = pressure.check().err().map(|e| e.to_string());
            #[cfg(not(feature = "llm-local"))]
            let error = crate::resources::critical_memory_pressure();
            if let Some(error) = error {
                crate::native::worker::emit_runtime_warning(&error);
                *stop_reason
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                return true;
            }
            false
        });
        params.set_n_threads(crate::native::policy::inference_thread_count());
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
        let inference = state.full(params, &samples);
        check_abort_reason(&stopped)?;
        inference.map_err(|e| {
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
            RuntimeError::Other(format!("whisper transcription failed: {e}{details}"))
        })?;

        let num_segments = state.full_n_segments();

        let mut segments = Vec::with_capacity(num_segments as usize);
        for i in 0..num_segments {
            let Some(segment) = state.get_segment(i) else {
                continue;
            };
            let text = segment
                .to_str()
                .map_err(|e| RuntimeError::Other(format!("failed to get segment text: {e}")))?
                .trim()
                .to_string();
            // whisper.cpp timestamps are in centiseconds.
            let start_ms = segment.start_timestamp() * 10;
            let end_ms = segment.end_timestamp() * 10;

            segments.push(TranscriptSegment {
                start_ms,
                end_ms,
                text,
            });
        }

        Ok(Transcript::from_segments(segments))
    }
}

fn check_abort_reason(stopped: &std::sync::Mutex<Option<String>>) -> Result<()> {
    if let Some(reason) = stopped
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    {
        return Err(RuntimeError::Other(reason));
    }
    Ok(())
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
                if let Some((_, after_colon)) = ev.message.split_once(':') {
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
        .map_err(|e| RuntimeError::Other(format!("failed to open WAV {}: {e}", path.display())))?;
    let spec = reader.spec();
    if spec.channels != 1 {
        return Err(RuntimeError::Other(format!(
            "expected mono WAV, got {} channels — was it normalized via ingest::fetch_audio?",
            spec.channels
        )));
    }

    let samples: std::result::Result<Vec<i16>, _> = reader.samples::<i16>().collect();
    let samples =
        samples.map_err(|e| RuntimeError::Other(format!("failed reading WAV samples: {e}")))?;

    let mut floats = vec![0.0f32; samples.len()];
    whisper_rs::convert_integer_to_float_audio(&samples, &mut floats)
        .map_err(|e| RuntimeError::Other(format!("failed to convert audio samples: {e}")))?;
    Ok(floats)
}

/// A [`Transcriber`] that runs whisper in the isolated worker process.
///
/// Holds only a path and a language: the model itself is never loaded in this
/// process, so a fault inside whisper.cpp ends the worker rather than the
/// application. Progress reports stream back from the child as they happen, so
/// the live percentage during a long transcription is unchanged — including
/// the up-front "running on CPU, here is why" warning, which the child emits
/// as its first progress message.
pub struct WorkerTranscriber {
    model_path: std::path::PathBuf,
    language: Option<String>,
}

impl WorkerTranscriber {
    pub fn new(model_path: &Path, language: Option<&str>) -> Self {
        Self {
            model_path: model_path.to_path_buf(),
            language: language.map(str::to_string),
        }
    }
}

impl Transcriber for WorkerTranscriber {
    fn transcribe_with_progress(
        &self,
        wav_path: &Path,
        on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        // Generous, because it bounds silence rather than the whole job: the
        // child reports progress as it goes and each report restarts the wait,
        // so a long file that is visibly advancing is never cut off.
        const WHISPER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1800);

        match crate::native::worker::execute_with_progress(
            crate::native::worker::WorkerRequest::Whisper {
                model_path: self.model_path.clone(),
                language: self.language.clone(),
                wav_path: wav_path.to_path_buf(),
            },
            WHISPER_TIMEOUT,
            on_progress,
        )? {
            crate::native::worker::WorkerOutput::Whisper(transcript) => Ok(transcript),
            #[cfg(feature = "llm-local")]
            _ => Err(RuntimeError::Other(
                "transcription worker returned an unexpected response".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod backend_detection_tests {
    //! `detect_backend_from_log` is what tells the user whether Whisper
    //! is going to be fast (GPU) or slow (CPU) — a bug here would silently
    //! mislabel the two, so cover the actual whisper.cpp/GGML log
    //! phrasings we've observed in practice (see `TODO.md`'s notes on
    //! Vulkan init + the traces gathered while debugging the null-context
    //! crash).
    use super::{check_abort_reason, detect_backend_from_log, WhisperBackend};
    use crate::log_tap::{TapEvent, TapLevel};
    use std::time::Instant;

    #[test]
    fn recorded_pressure_abort_cannot_be_returned_as_successful_partial_text() {
        let reason = std::sync::Mutex::new(Some("synthetic RAM pressure".into()));
        assert!(check_abort_reason(&reason)
            .unwrap_err()
            .to_string()
            .contains("synthetic RAM pressure"));
        assert!(check_abort_reason(&std::sync::Mutex::new(None)).is_ok());
    }

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
                assert_eq!(
                    device.as_deref(),
                    Some("AMD Radeon RX 7900 XTX (RADV NAVI31)")
                );
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
