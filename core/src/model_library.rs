//! Shared "model library" — where locally-downloaded model files (Whisper
//! ggml/gguf checkpoints, LLM GGUF quantizations, ...) live, and how
//! settings resolve a configured model name to an actual file on disk.
//!
//! This is intentionally the *only* place that knows about "a models
//! directory on disk". `media::transcribe` and any local LLM provider both
//! go through here instead of each inventing their own model-file
//! discovery/import logic — the exact "no repeated code" principle used
//! elsewhere in this crate (see `ai::traits::LlmProvider` for the same idea
//! applied to inference backends instead of model files).
//!
//! ## How it's meant to work end-to-end
//! 1. The user downloads a model file from Hugging Face (or anywhere) to
//!    wherever their browser/downloader puts it.
//! 2. They "import" it via [`import_model`], which copies it into Grafium's
//!    managed models directory so every future launch finds it without the
//!    user remembering (or Settings needing to store) where it originally
//!    landed. Power users can instead skip importing and point Settings at
//!    an absolute path directly — [`resolve_model`] supports both.
//! 3. Settings reference a model by *file name* (not a full path), which
//!    [`resolve_model`] turns into a real path, or auto-picks the only
//!    model of the right kind if nothing is configured yet — so a single
//!    downloaded model "just works" with zero settings changes.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// What kind of model a file appears to be, inferred from its name.
/// Used to filter [`scan_models_dir`] / [`resolve_model`] results so a
/// Whisper checkpoint never gets offered where an LLM is expected, and
/// vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// A whisper.cpp speech-to-text checkpoint (`ggml-*.bin`, or a whisper
    /// `*.gguf`).
    Whisper,
    /// A general LLM checkpoint (llama.cpp-style `*.gguf` quantization).
    Llm,
    /// A text embedding model (e.g. `nomic-embed-text`, `bge-*`, `gte-*`,
    /// `e5-*`, `*-minilm-*`) — used for local semantic search via
    /// `ai::providers::local_embedder::LocalEmbedder`, distinct from a
    /// general chat/completion LLM even though both ship as `.gguf` files.
    Embedding,
    /// A cross-encoder reranker (e.g. `bge-reranker-v2-m3`). Superficially
    /// looks like an embedding model (shares the `bge-` family prefix) but
    /// produces relevance *scores* for (query, document) pairs, NOT the
    /// sentence embedding vectors [`Embedding`] models produce. Feeding a
    /// reranker into [`LocalEmbedder`] silently yields garbage vectors, so
    /// it gets its own kind and is excluded from embedding auto-resolution.
    Reranker,
    /// Didn't match any recognized naming convention.
    Unknown,
}

/// A model file discovered in (or imported into) the managed models
/// directory.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub file_name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub kind: ModelKind,
}

/// Grafium's default managed models directory: `<data_dir>/models`. Kept as
/// a single function so every caller agrees on the same location instead of
/// each hardcoding `"models"` separately.
pub fn default_models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

/// Naming fragments the common local embedding model families always
/// include somewhere in the file name. Checked before the whisper heuristic
/// below since several of these families reuse whisper's generic size words
/// (e.g. `bge-base-en-v1.5.gguf`, `gte-small.gguf`).
const EMBEDDING_MARKERS: &[&str] = &[
    "embed",
    "bge-",
    "bge_",
    "gte-",
    "gte_",
    "e5-",
    "e5_",
    "minilm",
    "arctic-embed",
    "granite-embedding",
    "gist-embed",
    "sentence-transformer",
];

/// Classifies a file name by the naming conventions its ecosystem uses:
/// embedding models mention "embed" or one of a handful of well-known
/// embedding family prefixes; whisper.cpp checkpoints are `ggml-*` or
/// otherwise explicitly mention "whisper"; llama.cpp quantizations are
/// `*.gguf` (or the older `*.bin`) without those markers.
///
/// We intentionally do NOT use bare Whisper size words ("small", "base",
/// "medium", ...) as classification signals — LLM naming widely reuses
/// them (e.g. `Mistral-Small-3.2-24B`, `Qwen3-Small`, `phi-small`) and
/// misclassifying an LLM as Whisper causes the auto-picker to feed a
/// multi-GB LLM file into whisper.cpp and produce `Failed to create a new
/// whisper context`.
pub fn classify(file_name: &str) -> ModelKind {
    let lower = file_name.to_lowercase();
    // Rerankers must be checked BEFORE the embedding markers: they share the
    // `bge-` family prefix (e.g. `bge-reranker-v2-m3-Q8_0.gguf`) but are
    // cross-encoders, not sentence-embedding models. Classifying one as
    // `Embedding` would let the auto-picker feed it into `LocalEmbedder` and
    // silently produce garbage vectors.
    if lower.contains("rerank") {
        return ModelKind::Reranker;
    }
    if EMBEDDING_MARKERS.iter().any(|m| lower.contains(m)) {
        return ModelKind::Embedding;
    }
    // Whisper checkpoints come from ggerganov's naming: `ggml-<size>.bin`
    // or `ggml-<size>-q*.bin`. The upstream `whisper.cpp` repo distributes
    // exactly this shape. Third-party quantizations always include the
    // literal word "whisper".
    let looks_like_whisper = lower.starts_with("ggml-") || lower.contains("whisper");
    if looks_like_whisper {
        ModelKind::Whisper
    } else if lower.ends_with(".gguf") || lower.ends_with(".bin") {
        ModelKind::Llm
    } else {
        ModelKind::Unknown
    }
}

/// Lists every model file directly inside `models_dir` (non-recursive),
/// alphabetically. Returns an empty list (not an error) if the directory
/// doesn't exist yet — a fresh install simply has no models until the user
/// imports one.
pub fn scan_models_dir(models_dir: &Path) -> Result<Vec<ModelInfo>> {
    if !models_dir.exists() {
        return Ok(Vec::new());
    }
    let mut models = Vec::new();
    for entry in fs::read_dir(models_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip partial downloads / in-progress imports (see `import_model`).
        if file_name.ends_with(".part") || file_name.ends_with(".tmp") {
            continue;
        }
        let size_bytes = entry.metadata()?.len();
        models.push(ModelInfo {
            file_name: file_name.to_string(),
            path: path.clone(),
            size_bytes,
            kind: classify(file_name),
        });
    }
    models.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(models)
}

/// Copies `source` (a model file the user downloaded from Hugging Face or
/// anywhere else) into `models_dir`, creating the directory if needed.
/// Copies rather than moves so the user's original download is left alone.
///
/// Copies via a `.part` temp file + rename so a crash/interrupt mid-copy
/// never leaves a truncated file that `scan_models_dir` would treat as
/// real — the same trick `yt-dlp`/browsers use for downloads. Re-importing
/// a file with the same name overwrites the existing one, which is how a
/// user replaces a model with a newer quantization of the same name.
pub fn import_model(source: &Path, models_dir: &Path) -> Result<ModelInfo> {
    if !source.is_file() {
        return Err(CoreError::NotFound(format!(
            "Model file not found: {}",
            source.display()
        )));
    }
    fs::create_dir_all(models_dir)?;
    let file_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| CoreError::Other(format!("Invalid model file name: {}", source.display())))?
        .to_string();
    let dest = models_dir.join(&file_name);
    let tmp_dest = models_dir.join(format!("{file_name}.part"));
    fs::copy(source, &tmp_dest)?;
    fs::rename(&tmp_dest, &dest)?;

    let size_bytes = fs::metadata(&dest)?.len();
    Ok(ModelInfo {
        kind: classify(&file_name),
        file_name,
        path: dest,
        size_bytes,
    })
}

/// Resolves a configured model reference to an actual path on disk:
///   * If `configured` is an absolute path that exists, use it as-is (lets
///     power users point straight at a file anywhere, bypassing the
///     managed directory entirely).
///   * Otherwise treat it as a file name inside `models_dir`.
///   * If `configured` is `None`, auto-pick the first model of `kind` found
///     in `models_dir`, so a single downloaded/imported model "just works"
///     without the user having to type its exact file name into settings.
pub fn resolve_model(
    configured: Option<&str>,
    models_dir: &Path,
    kind: ModelKind,
) -> Result<PathBuf> {
    if let Some(configured) = configured {
        let as_path = PathBuf::from(configured);
        if as_path.is_absolute() && as_path.is_file() {
            return Ok(as_path);
        }
        let candidate = models_dir.join(configured);
        if candidate.is_file() {
            return Ok(candidate);
        }
        return Err(CoreError::NotFound(format!(
            "Configured model \"{configured}\" not found in {} (looked for that exact path, \
             and as a file name inside the managed models directory)",
            models_dir.display()
        )));
    }

    let candidates: Vec<ModelInfo> = scan_models_dir(models_dir)?
        .into_iter()
        .filter(|m| m.kind == kind)
        .collect();

    // Only chat models are GPU-offloaded by this path, and only they are
    // large enough for the choice to matter; anything else keeps the simple
    // first-match behaviour.
    let picked = if kind == ModelKind::Llm {
        pick_best_llm(candidates, crate::ai::gpu_fit::detect_free_vram_bytes())
    } else {
        candidates.into_iter().next()
    };

    picked
        .map(|m| m.path)
        .ok_or_else(|| CoreError::NotFound(model_not_found_hint(models_dir, kind)))
}

/// Chooses which chat model to auto-load when the user hasn't configured one.
///
/// Previously this was simply "first match in alphabetical order", which is
/// effectively random with respect to the only property that matters. On a
/// real 16 GB machine holding eight GGUFs it selected a 5.9 GB vision model
/// that emits its reasoning in Chinese, and the next alphabetical candidate
/// was a 13.6 GB model that runs on the CPU at ~1.5 tok/s. Neither is a
/// defensible zero-config default when a 2.4 GB model on the same box does
/// 60-74 tok/s.
///
/// The rule: never auto-pick a model that can't run on the GPU if one that
/// can is available, and among the models that do fit prefer the largest,
/// since parameter count is the best size-only proxy for answer quality.
/// `Tight` ranks below `Fits` but above CPU-bound, so a borderline model is
/// only chosen when nothing fits comfortably.
///
/// When free VRAM can't be measured, this deliberately falls back to the
/// historical alphabetical behaviour rather than guessing — an unmeasurable
/// GPU shouldn't silently change which model a user's machine loads.
fn pick_best_llm(mut candidates: Vec<ModelInfo>, free_vram_bytes: Option<u64>) -> Option<ModelInfo> {
    use crate::ai::gpu_fit::{assess_gpu_fit, GpuFit};

    // No GPU reading available (non-NVIDIA, no `nvidia-smi`, unparsable
    // output): keep the historical first-alphabetically pick. Returning
    // `None` here instead would turn "can't measure VRAM" into "no model
    // found", breaking auto-detect outright on those machines.
    let Some(free) = free_vram_bytes else {
        return candidates.into_iter().next();
    };

    // Ranks ascending so the best candidate sorts last: fit tier first, then
    // size as the quality proxy within a tier.
    candidates.sort_by_key(|m| {
        let tier = match assess_gpu_fit(m.size_bytes, Some(free)) {
            GpuFit::Fits => 3,
            GpuFit::Tight => 2,
            GpuFit::CpuOnly => 1,
            GpuFit::Unknown => 0,
        };
        (tier, m.size_bytes)
    });
    candidates.pop()
}

/// A reference to a locally-managed model file, exactly as it's meant to
/// be stored in settings. This is the single reusable shape for "which
/// model file should this feature use" across *every* local-inference
/// feature — `media::config::WhisperSettings` and
/// `ai::config::LocalLlmSettings` both embed one instead of each inventing
/// their own bare-name/absolute-path/auto-pick settings field, and both get
/// the resolution behaviour ([`Self::resolve`]) for free rather than each
/// calling [`resolve_model`] by hand. Any future local model need (e.g. a
/// local embedding model) should embed this same type rather than adding
/// another ad hoc `Option<String>` model field.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct LocalModelRef {
    /// A bare file name to resolve inside the managed models directory, an
    /// absolute path to use as-is, or `None` to auto-pick the only model
    /// of the expected kind. See [`resolve_model`] for the exact rules.
    pub model: Option<String>,
}

impl LocalModelRef {
    /// Convenience constructor for callers that already have a file name
    /// or path in hand (e.g. tests, or a CLI flag) rather than deserializing
    /// one from settings.
    pub fn named(model: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
        }
    }

    /// Resolves this reference to an actual file path, against `models_dir`
    /// and the expected `kind`. Thin wrapper around [`resolve_model`] kept
    /// as a method so callers read `settings.model_ref.resolve(...)` next
    /// to the data it resolves, instead of a free function call that reads
    /// like it could belong to any random model reference.
    pub fn resolve(&self, models_dir: &Path, kind: ModelKind) -> Result<PathBuf> {
        resolve_model(self.model.as_deref(), models_dir, kind)
    }
}

/// A friendly, actionable error message pointing the user at where to
/// download a model and where to put it — shown the first time a feature
/// needing a local model is used with nothing configured yet.
fn model_not_found_hint(models_dir: &Path, kind: ModelKind) -> String {
    match kind {
        ModelKind::Whisper => format!(
            "No Whisper model found in {}. Download one (e.g. ggml-base.en.bin or ggml-medium.bin) \
             from https://huggingface.co/ggerganov/whisper.cpp/tree/main and either place it in that \
             directory or import it from Settings.",
            models_dir.display()
        ),
        ModelKind::Llm => format!(
            "No LLM model found in {}. Download a GGUF quantization from Hugging Face (search \
             the model name + \"GGUF\") and either place it in that directory or import it from Settings.",
            models_dir.display()
        ),
        ModelKind::Embedding => format!(
            "No embedding model found in {}. Download a GGUF embedding model (e.g. \
             nomic-ai/nomic-embed-text-v1.5-GGUF or CompendiumLabs/bge-small-en-v1.5-gguf from \
             Hugging Face) and either place it in that directory or import it from Settings.",
            models_dir.display()
        ),
        ModelKind::Reranker => format!(
            "No reranker model found in {}. Download a GGUF reranker (e.g. \
             bge-reranker-v2-m3) and either place it in that directory or import it from Settings.",
            models_dir.display()
        ),
        ModelKind::Unknown => format!("No matching model found in {}.", models_dir.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a candidate list from `(file_name, size_mib)` pairs.
    fn llms(specs: &[(&str, u64)]) -> Vec<ModelInfo> {
        specs
            .iter()
            .map(|(name, mib)| ModelInfo {
                file_name: (*name).to_string(),
                path: PathBuf::from(*name),
                size_bytes: mib * 1024 * 1024,
                kind: ModelKind::Llm,
            })
            .collect()
    }

    /// The real models directory that produced the original complaint, in
    /// the alphabetical order `scan_models_dir` returns. The old
    /// "first match wins" rule picked GLM (a vision model that reasons in
    /// Chinese); the next candidates were CPU-bound multi-GB models. On a
    /// 16 GB card the only sensible auto-pick is the 4B.
    fn real_world_lineup() -> Vec<ModelInfo> {
        llms(&[
            ("GLM-4.6V-Flash-heretic-imatrix-Q4_K_M.gguf", 5881),
            ("Huihui-Qwen3-14B-abliterated-v2.Q4_K_M.gguf", 8585),
            ("Mistral-Small-3.2-24B-Instruct-2506.i1-Q4_K_M.gguf", 13670),
            ("Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf", 17698),
            ("Qwen3-4B-Instruct-2507-Q4_K_M.gguf", 2382),
            ("zen-pro-qwen3-8b.gguf", 4984),
        ])
    }

    #[test]
    fn auto_pick_prefers_the_largest_model_that_fits_in_vram() {
        // A 16 GB card with a couple of GB already in use by the desktop.
        let free = Some(14_000 * 1024 * 1024);
        let picked = pick_best_llm(real_world_lineup(), free).expect("a model should be picked");
        // 8585 MiB is the largest that clears the margin at this free size.
        assert_eq!(
            picked.file_name,
            "Huihui-Qwen3-14B-abliterated-v2.Q4_K_M.gguf"
        );
    }

    #[test]
    fn auto_pick_never_prefers_a_cpu_bound_model_over_one_that_fits() {
        // Only ~4 GB free: everything but the 4B is CPU-bound.
        let free = Some(4_000 * 1024 * 1024);
        let picked = pick_best_llm(real_world_lineup(), free).expect("a model should be picked");
        assert_eq!(picked.file_name, "Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
    }

    /// Regression: the previous rule returned whatever sorted first, which on
    /// this machine was a 5.9 GB vision model.
    #[test]
    fn auto_pick_is_not_merely_alphabetical() {
        let free = Some(14_000 * 1024 * 1024);
        let picked = pick_best_llm(real_world_lineup(), free).unwrap();
        assert_ne!(picked.file_name, "GLM-4.6V-Flash-heretic-imatrix-Q4_K_M.gguf");
    }

    #[test]
    fn auto_pick_falls_back_to_first_when_vram_is_unmeasurable() {
        // Must not degrade to "no model found" on non-NVIDIA machines.
        let picked = pick_best_llm(real_world_lineup(), None).expect("must still pick something");
        assert_eq!(picked.file_name, "GLM-4.6V-Flash-heretic-imatrix-Q4_K_M.gguf");
    }

    #[test]
    fn auto_pick_returns_none_only_when_there_are_no_candidates() {
        assert!(pick_best_llm(Vec::new(), Some(14_000 * 1024 * 1024)).is_none());
        assert!(pick_best_llm(Vec::new(), None).is_none());
    }

    /// When nothing fits, still pick *something* — the largest CPU-bound
    /// model is a defensible last resort, and erroring out would be worse.
    #[test]
    fn auto_pick_still_returns_a_model_when_nothing_fits() {
        let free = Some(1_000 * 1024 * 1024);
        let picked = pick_best_llm(real_world_lineup(), free).expect("must still pick something");
        assert_eq!(picked.file_name, "Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf");
    }


    #[test]
    fn classify_recognizes_whisper_and_llm_naming_conventions() {
        assert_eq!(classify("ggml-base.en.bin"), ModelKind::Whisper);
        assert_eq!(classify("ggml-medium.bin"), ModelKind::Whisper);
        assert_eq!(classify("ggml-large-v3.bin"), ModelKind::Whisper);
        assert_eq!(classify("ggml-large-v3-turbo.bin"), ModelKind::Whisper);
        assert_eq!(classify("whisper-large-v3-q5_0.gguf"), ModelKind::Whisper);
        assert_eq!(
            classify("llama-3.1-8b-instruct.Q4_K_M.gguf"),
            ModelKind::Llm
        );
        assert_eq!(classify("qwen2.5-14b-instruct-q4_k_m.gguf"), ModelKind::Llm);
        assert_eq!(classify("notes.txt"), ModelKind::Unknown);
    }

    #[test]
    fn classify_does_not_treat_llm_size_words_as_whisper() {
        // Real-world LLM names include Whisper's generic size words
        // ("small", "medium") as marketing labels. Misclassifying them
        // sends a multi-GB LLM into whisper.cpp and fails with
        // "Failed to create a new whisper context".
        assert_eq!(
            classify("Mistral-Small-3.2-24B-Instruct-2506-Heretic-v1.2-2.i1-Q4_K_M.gguf"),
            ModelKind::Llm
        );
        assert_eq!(classify("phi-medium-4k-Q4_K_M.gguf"), ModelKind::Llm);
        assert_eq!(classify("Qwen3-Small-Instruct-Q4.gguf"), ModelKind::Llm);
    }

    #[test]
    fn classify_recognizes_embedding_model_naming_conventions() {
        assert_eq!(
            classify("nomic-embed-text-v1.5.f16.gguf"),
            ModelKind::Embedding
        );
        assert_eq!(
            classify("mxbai-embed-large-v1-q4_k_m.gguf"),
            ModelKind::Embedding
        );
        assert_eq!(classify("gte-small.q8_0.gguf"), ModelKind::Embedding);
        assert_eq!(classify("e5-small-v2.Q4_K_M.gguf"), ModelKind::Embedding);
        // Several embedding families reuse whisper's generic size words
        // ("base", "small") in their own names — the embedding check must
        // win so these aren't misclassified as whisper checkpoints.
        assert_eq!(
            classify("bge-base-en-v1.5-q4_k_m.gguf"),
            ModelKind::Embedding
        );
    }

    #[test]
    fn classify_recognizes_rerankers_as_their_own_kind() {
        // Rerankers share the `bge-` family prefix with embedding models but
        // are cross-encoders: feeding one into `LocalEmbedder` produces
        // garbage vectors. They must be classified distinctly and excluded
        // from embedding auto-resolution.
        assert_eq!(
            classify("bge-reranker-v2-m3-Q8_0.gguf"),
            ModelKind::Reranker
        );
        assert_eq!(
            classify("bge-reranker-large.Q4_K_M.gguf"),
            ModelKind::Reranker
        );
        assert_eq!(classify("jina-reranker-v2-base.gguf"), ModelKind::Reranker);
    }

    #[test]
    fn resolve_model_does_not_auto_pick_a_reranker_for_embeddings() {
        // Regression: with only a reranker on disk, embedding auto-resolution
        // must fail loudly rather than silently hand the reranker to
        // `LocalEmbedder`.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("bge-reranker-v2-m3-Q8_0.gguf"),
            b"fake reranker bytes",
        )
        .unwrap();
        let resolved = resolve_model(None, dir.path(), ModelKind::Embedding);
        assert!(
            resolved.is_err(),
            "a reranker must not be auto-resolved as an embedding model"
        );

        // But a real embedding model alongside it still resolves.
        fs::write(
            dir.path().join("nomic-embed-text-v1.5.f16.gguf"),
            b"fake embed bytes",
        )
        .unwrap();
        let resolved = resolve_model(None, dir.path(), ModelKind::Embedding).unwrap();
        assert_eq!(
            resolved.file_name().and_then(|n| n.to_str()),
            Some("nomic-embed-text-v1.5.f16.gguf")
        );
    }

    #[test]
    fn scan_models_dir_returns_empty_for_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(scan_models_dir(&missing).unwrap().is_empty());
    }

    #[test]
    fn scan_models_dir_skips_partial_downloads() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ggml-base.en.bin"), b"fake model bytes").unwrap();
        fs::write(
            dir.path().join("ggml-medium.bin.part"),
            b"still downloading",
        )
        .unwrap();
        let models = scan_models_dir(dir.path()).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].file_name, "ggml-base.en.bin");
    }

    #[test]
    fn import_model_copies_into_managed_dir_without_deleting_source() {
        let source_dir = tempfile::tempdir().unwrap();
        let models_dir = tempfile::tempdir().unwrap();
        let source_path = source_dir.path().join("ggml-tiny.en.bin");
        fs::write(&source_path, b"pretend model weights").unwrap();

        let info = import_model(&source_path, models_dir.path()).unwrap();

        assert!(
            source_path.exists(),
            "import_model must not delete the original file"
        );
        assert_eq!(info.path, models_dir.path().join("ggml-tiny.en.bin"));
        assert_eq!(info.kind, ModelKind::Whisper);
        assert_eq!(fs::read(&info.path).unwrap(), b"pretend model weights");
    }

    #[test]
    fn resolve_model_auto_picks_the_only_match_when_nothing_configured() {
        let models_dir = tempfile::tempdir().unwrap();
        fs::write(models_dir.path().join("ggml-base.en.bin"), b"x").unwrap();
        fs::write(models_dir.path().join("llama-3.1-8b.Q4_K_M.gguf"), b"y").unwrap();

        let whisper_path = resolve_model(None, models_dir.path(), ModelKind::Whisper).unwrap();
        assert_eq!(whisper_path, models_dir.path().join("ggml-base.en.bin"));

        let llm_path = resolve_model(None, models_dir.path(), ModelKind::Llm).unwrap();
        assert_eq!(llm_path, models_dir.path().join("llama-3.1-8b.Q4_K_M.gguf"));
    }

    #[test]
    fn resolve_model_reports_a_helpful_error_when_nothing_found() {
        let models_dir = tempfile::tempdir().unwrap();
        let err = resolve_model(None, models_dir.path(), ModelKind::Whisper).unwrap_err();
        assert!(err.to_string().contains("huggingface.co"));
    }

    #[test]
    fn local_model_ref_resolve_matches_the_free_function() {
        let models_dir = tempfile::tempdir().unwrap();
        fs::write(models_dir.path().join("ggml-base.en.bin"), b"x").unwrap();

        let by_ref = LocalModelRef::default()
            .resolve(models_dir.path(), ModelKind::Whisper)
            .unwrap();
        let by_function = resolve_model(None, models_dir.path(), ModelKind::Whisper).unwrap();
        assert_eq!(by_ref, by_function);

        let named = LocalModelRef::named("ggml-base.en.bin")
            .resolve(models_dir.path(), ModelKind::Whisper)
            .unwrap();
        assert_eq!(named, models_dir.path().join("ggml-base.en.bin"));
    }

    #[test]
    fn resolve_model_honors_an_explicit_absolute_path_override() {
        let elsewhere = tempfile::tempdir().unwrap();
        let models_dir = tempfile::tempdir().unwrap();
        let explicit_path = elsewhere.path().join("my-custom-model.gguf");
        fs::write(&explicit_path, b"z").unwrap();

        let resolved = resolve_model(
            Some(explicit_path.to_str().unwrap()),
            models_dir.path(),
            ModelKind::Llm,
        )
        .unwrap();
        assert_eq!(resolved, explicit_path);
    }
}
