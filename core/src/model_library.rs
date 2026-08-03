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

/// Naming fragments whisper.cpp model releases always include somewhere in
/// the file name (`ggml-base.en.bin`, `ggml-large-v3.bin`,
/// `whisper-large-v3-q5_0.gguf`, ...).
const WHISPER_SIZE_NAMES: &[&str] = &["tiny", "base", "small", "medium", "large-v"];

/// Naming fragments the common local embedding model families always
/// include somewhere in the file name. Checked *before* the whisper
/// heuristic below since several of these families reuse whisper's generic
/// size words (e.g. `bge-base-en-v1.5.gguf`, `gte-small.gguf`) — without
/// this ordering those would be misclassified as whisper checkpoints.
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
/// otherwise mention "whisper" or a whisper size name; llama.cpp
/// quantizations are `*.gguf` (or the older `*.bin`) without those markers.
pub fn classify(file_name: &str) -> ModelKind {
    let lower = file_name.to_lowercase();
    if EMBEDDING_MARKERS.iter().any(|m| lower.contains(m)) {
        return ModelKind::Embedding;
    }
    let looks_like_whisper = lower.starts_with("ggml-")
        || lower.contains("whisper")
        || WHISPER_SIZE_NAMES.iter().any(|s| lower.contains(s));
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

    scan_models_dir(models_dir)?
        .into_iter()
        .find(|m| m.kind == kind)
        .map(|m| m.path)
        .ok_or_else(|| CoreError::NotFound(model_not_found_hint(models_dir, kind)))
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
        ModelKind::Unknown => format!("No matching model found in {}.", models_dir.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognizes_whisper_and_llm_naming_conventions() {
        assert_eq!(classify("ggml-base.en.bin"), ModelKind::Whisper);
        assert_eq!(classify("ggml-medium.bin"), ModelKind::Whisper);
        assert_eq!(classify("ggml-large-v3.bin"), ModelKind::Whisper);
        assert_eq!(classify("whisper-large-v3-q5_0.gguf"), ModelKind::Whisper);
        assert_eq!(
            classify("llama-3.1-8b-instruct.Q4_K_M.gguf"),
            ModelKind::Llm
        );
        assert_eq!(classify("qwen2.5-14b-instruct-q4_k_m.gguf"), ModelKind::Llm);
        assert_eq!(classify("notes.txt"), ModelKind::Unknown);
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
