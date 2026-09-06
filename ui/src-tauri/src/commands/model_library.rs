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
    /// `"fits"`, `"tight"`, `"cpu_only"`, or `"unknown"` — mirrors
    /// `grafium_core::ai::gpu_fit::GpuFit`. Lets the picker warn *before* the
    /// user commits to a model that the loader will quietly demote to CPU.
    pub gpu_fit: String,
    /// One-line plain-English rationale for `gpu_fit`, safe to show verbatim.
    pub gpu_fit_detail: String,
}

impl ModelInfoPayload {
    /// Attaches a GPU-fit verdict to a scanned model.
    ///
    /// `free_vram_bytes` is passed in (rather than probed per model) because
    /// the probe shells out to `nvidia-smi` and deliberately samples several
    /// times — doing that once per file would turn opening Settings into a
    /// multi-second stall on a folder with a dozen models.
    fn from_info(
        info: grafium_core::model_library::ModelInfo,
        free_vram_bytes: Option<u64>,
    ) -> Self {
        use grafium_core::ai::gpu_fit;
        // Only chat models are actually offloaded to the GPU by this path, so
        // a fit verdict on a Whisper or embedding file would be noise at best
        // and misleading at worst.
        let is_llm = matches!(info.kind, grafium_core::model_library::ModelKind::Llm);
        let (fit, detail) = if is_llm {
            let fit = gpu_fit::assess_gpu_fit(info.size_bytes, free_vram_bytes);
            (
                fit.as_str().to_string(),
                gpu_fit::fit_detail(fit, info.size_bytes, free_vram_bytes),
            )
        } else {
            (gpu_fit::GpuFit::Unknown.as_str().to_string(), String::new())
        };
        let mut payload = Self::from(info);
        payload.gpu_fit = fit;
        payload.gpu_fit_detail = detail;
        payload
    }
}

impl From<grafium_core::model_library::ModelInfo> for ModelInfoPayload {
    fn from(info: grafium_core::model_library::ModelInfo) -> Self {
        let kind = match info.kind {
            grafium_core::model_library::ModelKind::Llm => "llm",
            grafium_core::model_library::ModelKind::Whisper => "whisper",
            grafium_core::model_library::ModelKind::Embedding => "embedding",
            grafium_core::model_library::ModelKind::Reranker => "reranker",
            grafium_core::model_library::ModelKind::Unknown => "unknown",
        };
        Self {
            file_name: info.file_name,
            size_bytes: info.size_bytes,
            kind: kind.to_string(),
            gpu_fit: grafium_core::ai::gpu_fit::GpuFit::Unknown
                .as_str()
                .to_string(),
            gpu_fit_detail: String::new(),
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
    let dir = match models_dir
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(d) => std::path::PathBuf::from(d),
        None => {
            let app_data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            grafium_core::model_library::default_models_dir(&app_data_dir)
        }
    };

    let models = grafium_core::model_library::scan_models_dir(&dir).map_err(|e| e.to_string())?;
    // Probed once for the whole listing — see `ModelInfoPayload::from_info`.
    let free_vram_bytes = grafium_core::ai::gpu_fit::detect_free_vram_bytes();
    Ok(models
        .into_iter()
        .map(|m| ModelInfoPayload::from_info(m, free_vram_bytes))
        .collect())
}
