//! Tauri command for browsing locally-downloaded model files, so Settings
//! can offer a dropdown of what's actually on disk instead of asking the
//! user to type an exact file name — see `grafium_core::model_library` for
//! the underlying scan/classification logic this just exposes over IPC.

use serde::{Deserialize, Serialize};
use tauri::Manager;

/// How comfortably a specific chat GGUF is expected to run on this
/// machine's GPU — computed by comparing
/// [`grafium_core::gpu_info::estimated_vram_needed_bytes`] to the
/// detected total VRAM. The model picker uses this to put a ⚠️ next to
/// models that will spill to CPU/RAM and be slow, so the user can pick
/// something appropriate without discovering the mismatch by watching
/// the first summary crawl at 0.5 tok/s.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VramFit {
    /// Weights + estimated overhead comfortably fit in VRAM (with the
    /// usual 1.5 GiB safety margin baked in). No warning shown.
    Fits,
    /// Weights fit but the estimated overhead pushes it over the top —
    /// will *probably* run on GPU but with almost no headroom for a
    /// larger context, background apps, etc. Amber warning.
    Tight,
    /// Weights alone won't fit — llama.cpp will offload only some layers
    /// to GPU (or none at all if we're far over budget), streaming the
    /// rest from host RAM and grinding at CPU-inference speeds. Red ⚠️.
    WontFit,
    /// GPU detection failed, so we have no basis for a warning. The UI
    /// falls back to a plain size display.
    Unknown,
}

fn classify_fit(model_bytes: u64, total_vram_bytes: Option<u64>) -> VramFit {
    let Some(total) = total_vram_bytes else {
        return VramFit::Unknown;
    };
    let needed = grafium_core::gpu_info::estimated_vram_needed_bytes(model_bytes);
    // Split into three bands: comfortably-fits (< 85% of VRAM),
    // tight-but-plausible (85-105%), won't-fit (> 105%). Percentages
    // chosen empirically: at 85% of the card's total, there's still
    // room for the desktop's own use of the GPU (compositor, browser
    // GPU-accel, etc.); past 105% we're already asking llama.cpp to
    // partial-offload, which is what triggers the slow path.
    let tight_ceiling = total.saturating_mul(85) / 100;
    let wont_fit_ceiling = total.saturating_mul(105) / 100;
    if needed <= tight_ceiling {
        VramFit::Fits
    } else if needed <= wont_fit_ceiling {
        VramFit::Tight
    } else {
        VramFit::WontFit
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModelInfoPayload {
    pub file_name: String,
    pub size_bytes: u64,
    /// `"llm"`, `"whisper"`, `"embedding"`, or `"unknown"` — mirrors
    /// `grafium_core::model_library::ModelKind`, stringified so the
    /// frontend doesn't need to duplicate the enum.
    pub kind: String,
    /// The GGUF `general.architecture` value, if this file could be
    /// peeked (see `grafium_core::model_library::ModelInfo::architecture`
    /// for how/when this is populated).
    pub architecture: Option<String>,
    /// Best-effort human-readable summary assembled from whatever GGUF
    /// metadata the file carries, for the model picker's description pane.
    pub description: Option<String>,
    /// `true` if this model's architecture is known to crash the bundled
    /// llama.cpp — the model picker shows a warning instead of letting the
    /// user pick it blind.
    pub unstable_architecture: bool,
    /// How well this model is expected to fit in the detected GPU's
    /// VRAM, or `Unknown` if we couldn't detect the GPU at all. Set on
    /// `Llm` kind only — non-chat models don't get this annotation.
    pub vram_fit: VramFit,
    /// The estimated VRAM (in bytes) needed to run this model with a
    /// typical context — same number the fit classification uses. Sent
    /// down so the UI can put "needs ~15 GB" in the description without
    /// re-deriving it in TypeScript.
    pub vram_needed_bytes: Option<u64>,
}

impl ModelInfoPayload {
    fn from_with_vram(
        info: grafium_core::model_library::ModelInfo,
        total_vram_bytes: Option<u64>,
    ) -> Self {
        let kind = match info.kind {
            grafium_core::model_library::ModelKind::Llm => "llm",
            grafium_core::model_library::ModelKind::Whisper => "whisper",
            grafium_core::model_library::ModelKind::Embedding => "embedding",
            grafium_core::model_library::ModelKind::Unknown => "unknown",
        };
        // Only chat models get a VRAM-fit annotation — whisper/embedding
        // are tiny compared to any modern GPU, and unknown-kind files
        // shouldn't be surfaced as loadable chat models anyway.
        let is_llm = matches!(info.kind, grafium_core::model_library::ModelKind::Llm);
        let (vram_fit, vram_needed_bytes) = if is_llm {
            (
                classify_fit(info.size_bytes, total_vram_bytes),
                Some(grafium_core::gpu_info::estimated_vram_needed_bytes(
                    info.size_bytes,
                )),
            )
        } else {
            (VramFit::Unknown, None)
        };
        Self {
            file_name: info.file_name,
            size_bytes: info.size_bytes,
            kind: kind.to_string(),
            architecture: info.architecture,
            description: info.description,
            unstable_architecture: info.unstable_architecture,
            vram_fit,
            vram_needed_bytes,
        }
    }
}

impl From<grafium_core::model_library::ModelInfo> for ModelInfoPayload {
    fn from(info: grafium_core::model_library::ModelInfo) -> Self {
        // Fallback conversion used by callers that don't have a
        // GpuInfo handy — pretends we don't know the VRAM so the UI
        // shows no fit annotation, which is the safe default.
        Self::from_with_vram(info, None)
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
    // Detect once, off the scan loop, so we only shell out to
    // nvidia-smi/vulkaninfo a single time per Settings-tab open even
    // when there are dozens of GGUFs on disk.
    let gpu = grafium_core::gpu_info::detect_primary_gpu();
    let total = gpu.total_vram_bytes;
    Ok(models
        .into_iter()
        .map(|m| ModelInfoPayload::from_with_vram(m, total))
        .collect())
}

/// Best-effort report of the primary discrete GPU on this machine so
/// the Settings UI can (a) display which card we detected and how much
/// VRAM it has, and (b) explain *why* a specific model is or isn't
/// flagged as slow. Never errors — returns `GpuInfo::default()` (source
/// = `None`) when no detection path succeeded, and the UI treats that
/// as "we don't know, don't annotate anything".
#[tauri::command]
pub async fn detect_gpu_info() -> Result<grafium_core::gpu_info::GpuInfo, String> {
    Ok(grafium_core::gpu_info::detect_primary_gpu())
}
