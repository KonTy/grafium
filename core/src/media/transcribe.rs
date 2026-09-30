//! Grafium settings/error adapters for shared transcription implementations.

use crate::error::Result;
use crate::media::Transcript;
use crate::model_library::{self, LocalModelRef, ModelKind};
use model_runtime::transcription::Transcriber as RuntimeTranscriber;
use std::path::Path;

pub use model_runtime::native::transcribe::WhisperBackend;
pub use model_runtime::transcription::TranscribeProgress;

/// Retains CoreError-based contracts for Grafium consumers and test doubles.
pub trait Transcriber {
    fn transcribe(&self, path: &Path) -> Result<Transcript> {
        self.transcribe_with_progress(path, &mut |_| {})
    }
    fn transcribe_with_progress(
        &self,
        path: &Path,
        _on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        self.transcribe(path)
    }
}

pub struct WhisperTranscriber(model_runtime::native::transcribe::WhisperTranscriber);

impl WhisperTranscriber {
    pub fn load(path: &Path, language: Option<&str>) -> Result<Self> {
        Ok(Self(
            model_runtime::native::transcribe::WhisperTranscriber::load(path, language)?,
        ))
    }
    pub fn backend(&self) -> &WhisperBackend {
        self.0.backend()
    }

    pub fn from_settings(
        models_dir: &Path,
        model_ref: &LocalModelRef,
        language: Option<&str>,
    ) -> Result<Self> {
        Self::load(
            &model_ref.resolve(models_dir, ModelKind::Whisper)?,
            language,
        )
    }
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
        path: &Path,
        on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        Ok(self.0.transcribe_with_progress(path, on_progress)?)
    }
}

pub struct WorkerTranscriber(model_runtime::native::transcribe::WorkerTranscriber);

impl WorkerTranscriber {
    pub fn new(path: &Path, language: Option<&str>) -> Self {
        Self(model_runtime::native::transcribe::WorkerTranscriber::new(
            path, language,
        ))
    }
}

impl Transcriber for WorkerTranscriber {
    fn transcribe_with_progress(
        &self,
        path: &Path,
        on_progress: &mut dyn FnMut(TranscribeProgress),
    ) -> Result<Transcript> {
        Ok(self.0.transcribe_with_progress(path, on_progress)?)
    }
}
