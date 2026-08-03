//! Settings for the media pipeline — deliberately small and following the
//! same shape as `ai::config::AiConfig` (a serializable settings struct
//! persisted to a JSON file by the UI layer, loaded once at startup) so a
//! future Settings screen can wire it up exactly the same way the AI
//! settings screen already does.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model_library::LocalModelRef;

/// Media (video/audio ingestion + transcription) settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    /// Whether local Whisper transcription may run at all. When `false`,
    /// video/audio import only uses captions that already exist (creator or
    /// auto-generated) and reports a clear error otherwise, instead of
    /// silently kicking off a CPU-heavy transcription — some users may
    /// prefer that for very long recordings, or on low-power machines.
    /// Defaults to `true` since builds that compile in the `media` feature
    /// are meant to "just work" with no extra setup.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Where imported/downloaded model files live. `None` means the
    /// default (`<data_dir>/models` — see `model_library::default_models_dir`).
    /// Overridable in case a user wants to point at a models folder they
    /// already use for another tool.
    pub models_dir: Option<PathBuf>,
    pub whisper: WhisperSettings,
}

fn default_true() -> bool {
    true
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            models_dir: None,
            whisper: WhisperSettings::default(),
        }
    }
}

/// Whisper transcription settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhisperSettings {
    /// Which model file to use — see `model_library::LocalModelRef` for the
    /// exact resolution rules (bare file name, absolute path, or auto-pick).
    /// Flattened so the on-disk JSON stays a flat `{"model": ..., ...}`
    /// rather than nesting a `model_ref` object.
    #[serde(flatten)]
    pub model_ref: LocalModelRef,
    /// Force a specific language (e.g. `"en"`), or `None` to auto-detect.
    pub language: Option<String>,
}
