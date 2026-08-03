//! Tauri command for browsing locally-downloaded model files, so Settings
//! can offer a dropdown of what's actually on disk instead of asking the
//! user to type an exact file name — see `grafium_core::model_library` for
//! the underlying scan/classification logic this just exposes over IPC.

use serde::{Deserialize, Serialize};
use tauri::Manager;

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelInfoPayload {
    pub file_name: String,
    pub size_bytes: u64,
    /// `"llm"`, `"whisper"`, `"embedding"`, or `"unknown"` — mirrors
    /// `grafium_core::model_library::ModelKind`, stringified so the
    /// frontend doesn't need to duplicate the enum.
    pub kind: String,
}

impl From<grafium_core::model_library::ModelInfo> for ModelInfoPayload {
    fn from(info: grafium_core::model_library::ModelInfo) -> Self {
        let kind = match info.kind {
            grafium_core::model_library::ModelKind::Llm => "llm",
            grafium_core::model_library::ModelKind::Whisper => "whisper",
            grafium_core::model_library::ModelKind::Embedding => "embedding",
            grafium_core::model_library::ModelKind::Unknown => "unknown",
        };
        Self {
            file_name: info.file_name,
            size_bytes: info.size_bytes,
            kind: kind.to_string(),
        }
    }
}

/// Lists model files in `models_dir`, or in the shared default models
/// folder (`<app_data_dir>/models`) when `models_dir` is empty/unset —
/// the same default both the embedded LLM and Whisper transcription fall
/// back to (see `KnowledgeEngine::with_models_root` /
/// `commands::media::media_import_video`), so what this shows always
/// matches what actually gets used.
#[tauri::command]
pub async fn list_local_models(
    app: tauri::AppHandle,
    models_dir: Option<String>,
) -> Result<Vec<ModelInfoPayload>, String> {
    let dir = match models_dir.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(d) => std::path::PathBuf::from(d),
        None => {
            let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            grafium_core::model_library::default_models_dir(&app_data_dir)
        }
    };

    let models = grafium_core::model_library::scan_models_dir(&dir).map_err(|e| e.to_string())?;
    Ok(models.into_iter().map(ModelInfoPayload::from).collect())
}
