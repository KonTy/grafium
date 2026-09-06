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
    /// The GGUF `general.architecture` value (e.g. `"qwen3"`, `"llama"`),
    /// read directly from the file's metadata header without loading any
    /// tensor weights — see [`peek_gguf_info`]. `None` if the file isn't a
    /// GGUF, couldn't be parsed, or (for non-`Llm`/`Embedding` kinds, or
    /// when the `llm-local` feature is disabled) wasn't inspected at all.
    pub architecture: Option<String>,
    /// A short human-readable summary assembled from whatever descriptive
    /// GGUF metadata the file happens to carry (`general.name`,
    /// `general.basename`, `general.size_label`, `general.finetune`,
    /// `general.base_model.0.name`, ...) — GGUF has no single mandatory
    /// "description" field, so this is a best-effort composite rather than
    /// a direct read of one key. `None` if nothing useful was found.
    pub description: Option<String>,
    /// `true` if [`architecture`](Self::architecture) is one of
    /// [`KNOWN_UNSTABLE_ARCHITECTURES`] — the model picker UI uses this to
    /// show a warning instead of letting the user pick a model that will
    /// only fail once they try to use it (see
    /// `ai::providers::local_llm::check_architecture_compatibility`, which
    /// enforces the same list at load time as the actual safety net; this
    /// flag is purely advisory/informational).
    pub unstable_architecture: bool,
}

/// `general.architecture` values whose Gated-Delta-Net (hybrid recurrent
/// memory) layers were, in an older bundled llama.cpp version, known to
/// segfault rather than fail cleanly — see
/// `ai::providers::local_llm::check_architecture_compatibility` for the
/// enforcement side of this list. Defined here (rather than in
/// `ai::providers::local_llm`) so this module's model-picker metadata
/// (`ModelInfo::unstable_architecture`) and that load-time check always
/// agree on exactly the same set of architectures, with a single place to
/// update as llama.cpp's support matures.
///
/// **Re-tested and cleared 2026-08-05**: this used to list `"qwen3next"`,
/// `"qwen35"`, and `"qwen35moe"` after a real SIGSEGV observed with a
/// Qwen3.6 GGUF. That crash was an upstream llama.cpp graph-splitting bug
/// (`ggml-org/llama.cpp` issue #19864), fixed by PR #19866 (merged
/// 2026-02-24). We bumped our vendored `llama-cpp-2`/`llama-cpp-sys-2` from
/// 0.1.153 to 0.1.154 (which already carried that fix, released five
/// months after it landed upstream) and re-ran the exact `qwen35`-arch
/// GGUFs that crashed before (via `core/examples/debug_llm_repro.rs`,
/// bypassing this check with `GRAFIUM_SKIP_ARCH_CHECK=1` during the
/// re-test only): both the "fused Gated Delta Net (autoregressive)" and
/// "(chunked)" paths now report `enabled` (previously they were rejected
/// and fell back to the crash-prone path), and a full CPU-only decode +
/// generation round-trip completed cleanly with correct output — no
/// crash, in both a fully-CPU and a partial-GPU-offload configuration.
/// Left as an empty list (rather than deleted outright) so the mechanism
/// stays ready to reuse immediately if a *new* architecture turns out to
/// have the same problem in the future.
pub const KNOWN_UNSTABLE_ARCHITECTURES: &[&str] = &[];

/// Filename substrings (matched case-insensitively) that identify chat
/// GGUFs known to be functionally broken as summarizers even though
/// their architecture loads cleanly — creative-writing fine-tunes whose
/// aggressive quantization damaged their instruction-following /
/// reasoning tokens, producing empty or one-word responses to
/// summarization prompts no matter which prompt shape we use.
///
/// These are marked with the same `unstable_architecture: true` flag
/// the arch-level list uses (name kept for backwards compatibility even
/// though the reason is different), so the model picker's ⚠️ badge and
/// the description-pane warning work without any UI plumbing changes.
/// The specific case that motivated adding this list was
/// `Qwen3.6-27B-Fable-Fusion-711-IQ2_M.gguf`: architecture `qwen3` (a
/// perfectly-supported arch — a plain Qwen3-4B loads and runs fine),
/// but this particular fine-tune at IQ2_M (2-bit quantization) responds
/// to the summarizer prompts with either an unclosed `<think>` or a
/// bare "Here" and then EOS, in every one of the three progressively-
/// looser prompt shapes we try.
///
/// Substrings, not exact filenames, so *variants* of these bad releases
/// (different quantizations of the same fine-tune, minor filename
/// tweaks) also get flagged — Q4_K_M of Fable-Fusion is probably fine
/// but at IQ2_M it's broken; substring matching catches both.
pub const KNOWN_UNSTABLE_MODEL_FILENAMES: &[&str] = &[
    "fable-fusion",
    "fable_fusion",
];

/// Case-insensitive substring match of `file_name` against
/// [`KNOWN_UNSTABLE_MODEL_FILENAMES`]. Broken out as a function (rather
/// than inlined at the two call sites in [`scan_models_dir`] and
/// [`import_model`]) so the same helper is reachable from tests and
/// from future callers.
pub fn is_known_unstable_filename(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    KNOWN_UNSTABLE_MODEL_FILENAMES
        .iter()
        .any(|needle| lower.contains(needle))
}

/// Grafium's default managed models directory: `<data_dir>/models`. Kept as
/// a single function so every caller agrees on the same location instead of
/// each hardcoding `"models"` separately.
pub fn default_models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

// Whisper's file-name convention pins its checkpoints to either the
// canonical `ggml-*.bin` naming from whisper.cpp itself or the
// `whisper-*` naming used by later GGUF conversions on Hugging Face.
// A more permissive size-based fallback (e.g. `medium.gguf`,
// `small.bin`) was tempting but caused misclassification of LLMs that
// use those exact words as *model variant* labels (Mistral-Small,
// Llama-Medium, TinyLlama) — see `classify` below.

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

/// Splits a lowercased filename into tokens delimited by `-`, `_`, `.`,
/// or whitespace. Used by [`classify`] to check for the whole `whisper`
/// token rather than a substring — many LLMs contain the letters
/// `whisper` inside a longer word (unlikely, but treating it as a
/// standalone token is more defensible than any substring match), and
/// this same helper leaves the door open for future stricter checks
/// (e.g. reintroducing `WHISPER_SIZE_TOKENS`-style word matching once
/// `classify` has more context to disambiguate them).
fn filename_tokens(lower: &str) -> impl Iterator<Item = &str> {
    lower.split(|c: char| matches!(c, '-' | '_' | '.' | ' '))
}

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
    // Whisper matching REQUIRES an unambiguous marker: either the
    // canonical `ggml-` prefix (whisper.cpp's own naming for its
    // checkpoints, no other GGML family reuses this) or a whole `whisper`
    // token somewhere in the file name. A bare size word like `small` or
    // `medium` is NOT enough on its own — many LLMs use those as variant
    // labels (`Mistral-Small`, `Llama-Medium`, ...) and were being
    // misclassified as Whisper checkpoints, then silently loaded as one,
    // which surfaced to the user as an opaque "Failed to create a new
    // whisper context" error even though the model file itself was fine.
    let tokens: Vec<&str> = filename_tokens(&lower).collect();
    let looks_like_whisper = lower.starts_with("ggml-") || tokens.contains(&"whisper");
    if looks_like_whisper {
        ModelKind::Whisper
    } else if lower.ends_with(".gguf") || lower.ends_with(".bin") {
        ModelKind::Llm
    } else {
        ModelKind::Unknown
    }
}

/// GGUF metadata peeked from a file's header without loading any tensor
/// data — see [`peek_gguf_info`].
struct GgufInfo {
    architecture: Option<String>,
    description: Option<String>,
}

/// Opens `path` as a GGUF file and reads a handful of well-known
/// `general.*` metadata keys (never tensor weights, so this is cheap even
/// for a huge model) to populate [`ModelInfo::architecture`] and
/// [`ModelInfo::description`]. Returns an all-`None` [`GgufInfo`] if the
/// file isn't a valid GGUF or none of the keys this looks for are present
/// — deliberately silent about that rather than an error, since the model
/// picker should still show *something* (just the file name) for a file
/// it can't introspect.
#[cfg(feature = "llm-local")]
fn peek_gguf_info(path: &Path) -> GgufInfo {
    use llama_cpp_2::gguf::GgufContext;

    let Some(ctx) = GgufContext::from_file(path) else {
        return GgufInfo {
            architecture: None,
            description: None,
        };
    };

    let read_str = |key: &str| -> Option<String> {
        let idx = ctx.find_key(key);
        if idx < 0 {
            return None;
        }
        ctx.val_str(idx).map(|s| s.to_string())
    };

    let architecture = read_str("general.architecture");
    let name = read_str("general.name");
    let basename = read_str("general.basename");
    let size_label = read_str("general.size_label");
    let finetune = read_str("general.finetune");
    let base_model_name = read_str("general.base_model.0.name");
    let base_model_org = read_str("general.base_model.0.organization");

    // Assemble whatever pieces are actually present into one short
    // human-readable line, rather than requiring every field — most GGUF
    // conversions only fill in a handful of these.
    let mut parts: Vec<String> = Vec::new();
    if let Some(n) = name.or(basename) {
        parts.push(n);
    }
    if let Some(size) = &size_label {
        parts.push(size.clone());
    }
    if let Some(f) = &finetune {
        parts.push(f.clone());
    }
    if let Some(arch) = &architecture {
        parts.push(format!("({arch} architecture)"));
    }
    if let Some(base) = base_model_name {
        match base_model_org {
            Some(org) => parts.push(format!("based on {org}/{base}")),
            None => parts.push(format!("based on {base}")),
        }
    }

    let description = if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    };

    GgufInfo {
        architecture,
        description,
    }
}

/// Without the `llm-local` feature, GGUF-peeking isn't compiled in at all
/// (its only dependency, `llama-cpp-2`, is feature-gated) — every model
/// file just reports no architecture/description rather than failing to
/// build.
#[cfg(not(feature = "llm-local"))]
fn peek_gguf_info(_path: &Path) -> GgufInfo {
    GgufInfo {
        architecture: None,
        description: None,
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
        let kind = classify(file_name);
        // Only worth introspecting GGUF-shaped chat/embedding models —
        // Whisper checkpoints and unknown files don't have (or don't need)
        // an architecture/description shown in the model picker.
        let (architecture, description) = if matches!(kind, ModelKind::Llm | ModelKind::Embedding)
        {
            let info = peek_gguf_info(&path);
            (info.architecture, info.description)
        } else {
            (None, None)
        };
        let unstable_architecture = architecture
            .as_deref()
            .is_some_and(|a| KNOWN_UNSTABLE_ARCHITECTURES.contains(&a))
            || is_known_unstable_filename(file_name);
        models.push(ModelInfo {
            file_name: file_name.to_string(),
            path: path.clone(),
            size_bytes,
            kind,
            architecture,
            description,
            unstable_architecture,
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
    let kind = classify(&file_name);
    let (architecture, description) = if matches!(kind, ModelKind::Llm | ModelKind::Embedding) {
        let info = peek_gguf_info(&dest);
        (info.architecture, info.description)
    } else {
        (None, None)
    };
    let unstable_architecture = architecture
        .as_deref()
        .is_some_and(|a| KNOWN_UNSTABLE_ARCHITECTURES.contains(&a))
        || is_known_unstable_filename(&file_name);
    Ok(ModelInfo {
        kind,
        file_name,
        path: dest,
        size_bytes,
        architecture,
        description,
        unstable_architecture,
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
fn pick_best_llm(
    mut candidates: Vec<ModelInfo>,
    free_vram_bytes: Option<u64>,
) -> Option<ModelInfo> {
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
                architecture: None,
                description: None,
                unstable_architecture: false,
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
        assert_ne!(
            picked.file_name,
            "GLM-4.6V-Flash-heretic-imatrix-Q4_K_M.gguf"
        );
    }

    #[test]
    fn auto_pick_falls_back_to_first_when_vram_is_unmeasurable() {
        // Must not degrade to "no model found" on non-NVIDIA machines.
        let picked = pick_best_llm(real_world_lineup(), None).expect("must still pick something");
        assert_eq!(
            picked.file_name,
            "GLM-4.6V-Flash-heretic-imatrix-Q4_K_M.gguf"
        );
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
    fn is_known_unstable_filename_matches_fable_fusion_variants_case_insensitively() {
        // The regression: user selected `Qwen3.6-27B-Fable-Fusion-711-IQ2_M.gguf`
        // (arch=qwen3, which is otherwise fine) and got literally "Here"
        // back as the summary. Filename-substring matching catches this
        // whole family regardless of arch, quantization, or casing.
        assert!(is_known_unstable_filename(
            "Qwen3.6-27B-Fable-Fusion-711-IQ2_M.gguf"
        ));
        assert!(is_known_unstable_filename(
            "qwen3.6-fable-fusion-14b-q4_k_m.gguf"
        ));
        assert!(is_known_unstable_filename("Fable_Fusion_Q3.gguf"));
        // But a plain, well-behaved fine-tune must NOT be flagged just
        // because it's from the same base — if we ever match too broadly
        // we'd disable perfectly good models.
        assert!(!is_known_unstable_filename(
            "Qwen3-4B-Instruct-Q4_K_M.gguf"
        ));
        assert!(!is_known_unstable_filename(
            "mistral-7b-instruct-v0.3.q4_k_m.gguf"
        ));
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

    /// Regression: an LLM file name that happens to contain a Whisper
    /// size word (`small`, `base`, `medium`, `tiny`, `large`) as a
    /// *variant* label must NOT be classified as a Whisper checkpoint —
    /// otherwise `resolve_model(None, ..., Whisper)` picks one of these
    /// LLM files first and the whisper.cpp loader chokes on it with an
    /// opaque "Failed to create a new whisper context" that took a full
    /// diagnostic round-trip to trace back to a misclassification.
    ///
    /// Real-world examples that were misclassified before the fix:
    /// * Mistral-Small-3.2-24B-Instruct-2506-...gguf   (contained "small")
    /// * Llama-3.2-Tiny.gguf                            (contained "tiny")
    /// * gemma-2-medium-q4_k_m.gguf                     (contained "medium")
    #[test]
    fn classify_does_not_misclassify_llms_using_whisper_size_words_as_variant_labels() {
        assert_eq!(
            classify("Mistral-Small-3.2-24B-Instruct-2506-Heretic-v1.2-2.i1-Q4_K_M.gguf"),
            ModelKind::Llm,
            "Mistral-Small must be an LLM, not a Whisper checkpoint"
        );
        assert_eq!(
            classify("Llama-3.2-Tiny.gguf"),
            ModelKind::Llm,
            "TinyLlama-style names must be an LLM, not a Whisper checkpoint"
        );
        assert_eq!(
            classify("gemma-2-medium-q4_k_m.gguf"),
            ModelKind::Llm,
            "gemma-2-medium must be an LLM, not a Whisper checkpoint"
        );
        assert_eq!(
            classify("qwen3-14b-base.gguf"),
            ModelKind::Llm,
            "a *-base LLM must be an LLM, not a Whisper checkpoint"
        );
        assert_eq!(
            classify("Llama-3.1-70B-Large.gguf"),
            ModelKind::Llm,
            "a *-large LLM must be an LLM, not a Whisper checkpoint"
        );
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
