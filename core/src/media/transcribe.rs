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
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript>;
}

/// Runs whisper.cpp locally against a `ggml`/`gguf` Whisper model file.
/// Stateless per call beyond the loaded model, so one instance can be reused
/// across many `transcribe()` calls (avoids re-loading the model each time).
pub struct WhisperTranscriber {
    ctx: WhisperContext,
    language: Option<String>,
}

impl WhisperTranscriber {
    /// Loads a `ggml-*.bin` model from `model_path`. Pass `language` (e.g.
    /// `"en"`) to force transcription in that language, or `None` to let
    /// whisper.cpp auto-detect it.
    pub fn load(model_path: &Path, language: Option<&str>) -> Result<Self> {
        // whisper.cpp/GGML log verbosely to stderr by default, which would
        // corrupt a raw-mode terminal (e.g. the TUI). Route logs through
        // `whisper-rs`'s hooks instead — since we don't enable the
        // `log_backend`/`tracing_backend` features, this silences them.
        static INSTALL_LOGGING_HOOKS: std::sync::Once = std::sync::Once::new();
        INSTALL_LOGGING_HOOKS.call_once(whisper_rs::install_logging_hooks);

        let ctx = WhisperContext::new_with_params(
            &*model_path.to_string_lossy(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| CoreError::Other(format!("failed to load whisper model: {e}")))?;
        Ok(Self {
            ctx,
            language: language.map(str::to_string),
        })
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
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript> {
        let samples = read_wav_as_f32_mono(wav_path)?;

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

        state
            .full(params, &samples)
            .map_err(|e| CoreError::Other(format!("whisper transcription failed: {e}")))?;

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
