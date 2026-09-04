//! Process-isolated speech-to-text via whisper.cpp (`whisper-rs` bindings).
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

use std::path::{Path, PathBuf};
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::ai::resources::{self, ModelWorkload};
use crate::error::{CoreError, Result};
use crate::media::types::{Transcript, TranscriptSegment};
use crate::model_library::{self, LocalModelRef, ModelKind};

/// Anything that can turn a 16kHz mono WAV file (exactly what
/// `media::ingest::fetch_audio` produces) into a [`Transcript`].
pub trait Transcriber {
    fn transcribe(&self, wav_path: &Path) -> Result<Transcript>;
}

/// Resolves a local Whisper model and dispatches each transcription to a
/// disposable resource-limited Grafium worker process.
pub struct WhisperTranscriber {
    model_path: PathBuf,
    language: Option<String>,
}

impl WhisperTranscriber {
    /// Loads a `ggml-*.bin` model from `model_path`. Pass `language` (e.g.
    /// `"en"`) to force transcription in that language, or `None` to let
    /// whisper.cpp auto-detect it.
    pub fn load(model_path: &Path, language: Option<&str>) -> Result<Self> {
        resources::validate_model_load(model_path, ModelWorkload::Whisper)?;
        Ok(Self {
            model_path: model_path.to_path_buf(),
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
        resources::validate_audio_buffer(wav_path)?;
        match crate::ai::worker::execute(
            crate::ai::worker::WorkerRequest::Whisper {
                model_path: self.model_path.clone(),
                language: self.language.clone(),
                wav_path: wav_path.to_path_buf(),
            },
            Duration::from_secs(2 * 60 * 60),
        )? {
            crate::ai::worker::WorkerOutput::Whisper(transcript) => Ok(transcript),
            #[cfg(feature = "llm-local")]
            crate::ai::worker::WorkerOutput::Ready => Err(CoreError::Other(
                "native AI worker returned a health result for a transcription request".to_string(),
            )),
            #[cfg(feature = "llm-local")]
            crate::ai::worker::WorkerOutput::Llm(_) => Err(CoreError::Other(
                "native AI worker returned an LLM response for a transcription request".to_string(),
            )),
        }
    }
}

pub(crate) struct WhisperSlot {
    model_path: PathBuf,
    language: Option<String>,
    ctx: WhisperContext,
}

impl WhisperSlot {
    fn matches(&self, model_path: &Path, language: Option<&str>) -> bool {
        self.model_path == model_path && self.language.as_deref() == language
    }
}

pub(crate) fn transcribe_in_process(
    slot: &mut Option<WhisperSlot>,
    model_path: &Path,
    language: Option<&str>,
    wav_path: &Path,
) -> Result<Transcript> {
    static INSTALL_LOGGING_HOOKS: std::sync::Once = std::sync::Once::new();
    INSTALL_LOGGING_HOOKS.call_once(whisper_rs::install_logging_hooks);

    // Only re-check model-load admission when we actually have to load. A cached
    // model already occupies its RAM footprint; running `validate_model_load`
    // again would subtract it from `available` and falsely reject reuse.
    let matches = slot
        .as_ref()
        .is_some_and(|cached| cached.matches(model_path, language));
    if !matches {
        resources::validate_model_load(model_path, ModelWorkload::Whisper)?;
        // Drop the previous model before loading a replacement.
        *slot = None;
        let ctx = WhisperContext::new_with_params(
            &*model_path.to_string_lossy(),
            WhisperContextParameters::default(),
        )
        .map_err(|e| CoreError::Other(format!("failed to load whisper model: {e}")))?;
        *slot = Some(WhisperSlot {
            model_path: model_path.to_path_buf(),
            language: language.map(str::to_string),
            ctx,
        });
    }
    let slot = slot.as_ref().expect("slot populated for transcription");

    // Validate audio buffer admission per-request (the wav varies each call);
    // model admission was already handled above only if we had to load.
    resources::validate_audio_buffer(wav_path)?;

    let samples = read_wav_as_f32_mono(wav_path)?;
    let mut state = slot
        .ctx
        .create_state()
        .map_err(|e| CoreError::Other(format!("failed to create whisper state: {e}")))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_n_threads(resources::inference_thread_count());
    if let Some(lang) = language {
        params.set_language(Some(lang));
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

    reader
        .samples::<i16>()
        .map(|sample| {
            sample
                .map(|value| value as f32 / 32_768.0)
                .map_err(|e| CoreError::Other(format!("failed reading WAV samples: {e}")))
        })
        .collect()
}
