//! Shared discovery, bounded metadata inspection and owned-file management.
//!
//! [`LocalModelRef`] preserves filename, absolute-path and automatic selection
//! semantics. Discovery never confers ownership. [`ModelLibrary`] records only
//! imports/downloads it publishes, using checksums, stable IDs, bounded manifests
//! and short advisory-lock transactions. Owned-file mutations also require an
//! exclusive inode lock, cooperating with resident workers' shared file leases.
//! Imports retain originals; conflicting
//! destinations are not replaced. Downloads require an explicit URL, SHA256 and
//! byte limit; every bounded redirect hop is authorized separately. Transfers
//! retain disk headroom and have a separate, bounded download deadline. Failed
//! stages are hidden and cleaned up without sweeping others.
//!
//! Removal is available only to the shared manager after its user-intent and
//! model-lifecycle checks. It quarantines verified owned bytes instead of deleting
//! files. Preexisting models, external files and quarantined backups are never
//! automatically adopted, purged or recursively deleted.
//! Read-only discovery/resolution follows model links and returns canonical
//! inference targets. Libraries opened through linked roots are read-only; owned
//! mutations require an explicitly selected, non-symlinked canonical directory.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    error::{Result, RuntimeError},
    providers::NetworkConfig,
};

/// What kind of model a file appears to be, inferred from its name.
/// Used to filter [`scan_models_dir`] / [`resolve_model`] results so a
/// Whisper checkpoint never gets offered where an LLM is expected, and
/// vice versa.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelKind {
    /// A whisper.cpp speech-to-text checkpoint (`ggml-*.bin`, or a whisper
    /// `*.gguf`).
    Whisper,
    /// A general LLM checkpoint (llama.cpp-style `*.gguf` quantization).
    Llm,
    /// A text embedding model (e.g. `nomic-embed-text`, `bge-*`, `gte-*`,
    /// `e5-*`, `*-minilm-*`) — used for local semantic search, distinct from a
    /// general chat/completion LLM even though both ship as `.gguf` files.
    Embedding,
    /// A cross-encoder reranker (e.g. `bge-reranker-v2-m3`). Superficially
    /// looks like an embedding model (shares the `bge-` family prefix) but
    /// produces relevance *scores* for (query, document) pairs, NOT the
    /// sentence embedding vectors [`ModelKind::Embedding`] models produce. Feeding a
    /// reranker into an embedding backend silently yields garbage vectors, so
    /// it gets its own kind and is excluded from embedding auto-resolution.
    Reranker,
    /// Didn't match any recognized naming convention.
    Unknown,
}

/// A model file discovered in (or imported into) the managed models
/// directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelInfo {
    pub file_name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub kind: ModelKind,
    /// The GGUF `general.architecture` value (e.g. `"qwen3"`, `"llama"`),
    /// read directly from the file's metadata header without loading any
    /// tensor weights — see [`peek_gguf_info`]. `None` if the file isn't a
    /// GGUF, couldn't be parsed, or wasn't inspected for this kind. Inspection is bounded
    /// pure Rust and never loads the native inference backend.
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
    /// only fail once they try to use it. The native backend enforces the same
    /// shared blacklist at load time; this flag is purely advisory.
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
pub const KNOWN_UNSTABLE_MODEL_FILENAMES: &[&str] = &["fable-fusion", "fable_fusion"];

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

/// Default managed models directory: `<data_dir>/models`. Kept as
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
    lower.split(['-', '_', '.', ' '])
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
fn peek_gguf_info(path: &Path) -> GgufInfo {
    match crate::gguf::inspect_metadata(path) {
        Ok(metadata) => GgufInfo {
            architecture: Some(metadata.architecture),
            description: metadata.description,
        },
        Err(_) => GgufInfo {
            architecture: None,
            description: None,
        },
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
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip partial downloads / in-progress imports (see `import_model`).
        if hidden_or_partial(file_name) {
            continue;
        }
        let Some(target) = canonical_read_model(&path)? else {
            continue;
        };
        models.push(inspect_model(&path, file_name, fs::metadata(target)?.len()));
    }
    models.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(models)
}

/// Copies `source` (a model file the user downloaded from Hugging Face or
/// anywhere else) into `models_dir`, creating the directory if needed.
/// Copies rather than moves so the user's original download is left alone.
///
/// Copies via a unique staged file, records ownership and publishes without
/// overwriting existing models. A conflicting name requires a different name
/// or explicit removal of a managed model, never replacement of an external file.
pub fn import_model(source: &Path, models_dir: &Path) -> Result<ModelInfo> {
    Ok(ModelLibrary::new(models_dir)?.import(source)?.entry.info)
}

/// Resolves a configured model reference to an actual path on disk:
///   * If `configured` is an absolute path that exists, use its canonical path (lets
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
    resolve_model_with_vram(
        configured,
        models_dir,
        kind,
        if kind == ModelKind::Llm && configured.is_none() {
            crate::gpu_info::detect_primary_gpu().available_vram_bytes
        } else {
            None
        },
    )
}

/// Resolution with host-supplied telemetry. This is a picker hint only, not
/// permission to load a model: runtime admission must check resources again.
pub fn resolve_model_with_vram(
    configured: Option<&str>,
    models_dir: &Path,
    kind: ModelKind,
    free_vram_bytes: Option<u64>,
) -> Result<PathBuf> {
    if let Some(configured) = configured {
        let as_path = PathBuf::from(configured);
        let candidate = if as_path.is_absolute() {
            as_path
        } else {
            validate_basename(configured)?;
            models_dir.join(configured)
        };
        if let Some(target) = canonical_read_model(&candidate)? {
            return Ok(target);
        }
        return Err(RuntimeError::Other(format!(
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
        pick_best_llm(candidates, free_vram_bytes)
    } else {
        candidates.into_iter().next()
    };

    let picked =
        picked.ok_or_else(|| RuntimeError::Other(model_not_found_hint(models_dir, kind)))?;
    canonical_read_model(&picked.path)?
        .ok_or_else(|| RuntimeError::Other(model_not_found_hint(models_dir, kind)))
}

fn canonical_read_model(path: &Path) -> Result<Option<PathBuf>> {
    let hidden = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(hidden_or_partial)
    };
    if hidden(path) || !path.is_file() {
        return Ok(None);
    }
    let target = match path.canonicalize() {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok((target.is_file() && !hidden(&target)).then_some(target))
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
        let margin = (m.size_bytes / 5).clamp(512 * 1024 * 1024, 2 * 1024 * 1024 * 1024);
        let required = m.size_bytes.saturating_add(margin);
        let tier = if required > free {
            1
        } else if free - required < 1024 * 1024 * 1024 {
            2
        } else {
            3
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

const INTERNAL_PREFIX: &str = ".model-library-";
const MANIFEST_NAME: &str = ".model-library-manifest.json";
const LOCK_NAME: &str = ".model-library-lock";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MANIFEST_ENTRIES: usize = 4096;
const MAX_MODEL_BYTES: u64 = 256 * 1024 * 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(3);

fn failure(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Other(message.into())
}

fn hidden_or_partial(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with(INTERNAL_PREFIX) || lower.ends_with(".part") || lower.ends_with(".tmp")
}

fn validate_basename(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved_device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if name.is_empty()
        || name.len() > 240
        || name.contains(['/', '\\', ':', '<', '>', '"', '|', '?', '*'])
        || reserved_device
        || name.chars().any(char::is_control)
        || name.ends_with(['.', ' '])
        || hidden_or_partial(name)
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(failure(
            "Model name must be a plain file name, not a path, reserved name or partial download",
        ));
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(failure(
            "Expected SHA256 must contain exactly 64 hexadecimal characters",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

/// A discovered file is not owned merely because it resides in the library.
/// An owned ID is an ownership claim, not proof that the file is unchanged;
/// removal always rechecks identity, size and checksum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CatalogEntry {
    pub info: ModelInfo,
    pub owned_id: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportResult {
    pub entry: CatalogEntry,
    pub already_present: bool,
}

/// Every download requires an explicitly supplied URL, digest and byte limit.
/// URL authorization does not constrain DNS resolution, proxies or IP locality:
/// the host's transport/network policy remains authoritative.
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub file_name: String,
    pub expected_sha256: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

/// Transfer limits independent of inference request timeouts. Disk checks are
/// conservative observations, not reservations against unrelated OS writers.
#[derive(Debug, Clone, Copy)]
pub struct TransferPolicy {
    /// At most ten; zero disables redirects. Every hop still needs host approval.
    pub max_redirects: usize,
    /// Total time across all redirects and response bytes; at most 24 hours.
    pub total_timeout: Duration,
    /// Free bytes to retain on the library's volume in addition to each write.
    pub disk_headroom_bytes: u64,
}

impl Default for TransferPolicy {
    fn default() -> Self {
        Self {
            max_redirects: 5,
            total_timeout: Duration::from_secs(6 * 60 * 60),
            disk_headroom_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

/// Hosts/tests may supply quota-aware or synthetic free-space observations.
/// The default uses `fs2::available_space` on the canonical library root.
pub trait DiskSpaceProbe: Send + Sync {
    fn available_bytes(&self, root: &Path) -> Result<u64>;
}

impl<F: Fn(&Path) -> Result<u64> + Send + Sync> DiskSpaceProbe for F {
    fn available_bytes(&self, root: &Path) -> Result<u64> {
        self(root)
    }
}

/// The manager must obtain explicit user intent, reject configured/in-use
/// models and drain/evict an idle native model before passing this approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemovalApproval {
    ExplicitUserRequest,
}

/// Removal never destroys bytes. Quarantined files are hidden from discovery;
/// hosts may offer manual recovery using this path, but must not purge them
/// without a separate, explicitly approved retention policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovalResult {
    pub id: String,
    pub original_path: PathBuf,
    pub quarantine_path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileStamp {
    device: Option<u64>,
    inode: Option<u64>,
    created_ns: Option<u64>,
    modified_ns: Option<u64>,
}

impl FileStamp {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        let nanos = |time: std::io::Result<std::time::SystemTime>| {
            time.ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_nanos()
                .try_into()
                .ok()
        };
        #[cfg(unix)]
        let (device, inode) = {
            use std::os::unix::fs::MetadataExt;
            (Some(metadata.dev()), Some(metadata.ino()))
        };
        #[cfg(not(unix))]
        let (device, inode) = (None, None);
        Self {
            device,
            inode,
            created_ns: nanos(metadata.created()),
            modified_ns: nanos(metadata.modified()),
        }
    }

    fn same_identity(&self, other: &Self) -> bool {
        match (self.device, self.inode) {
            (Some(device), Some(inode)) => {
                other.device == Some(device) && other.inode == Some(inode)
            }
            _ => self.created_ns.is_some() && self.created_ns == other.created_ns,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum OwnedState {
    Active,
    Quarantined { file_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedModel {
    id: String,
    file_name: String,
    kind: ModelKind,
    size_bytes: u64,
    sha256: String,
    stamp: FileStamp,
    state: OwnedState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    models: Vec<OwnedModel>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: 1,
            models: Vec::new(),
        }
    }
}

/// Shared local-file management. The canonical root is pinned at creation;
/// imports copy sources, never replace destinations, and only successfully
/// published imports/downloads are entered in the durable ownership manifest.
/// Advisory locks cover short manifest transactions, not network transfers.
#[derive(Clone)]
pub struct ModelLibrary {
    root: PathBuf,
    requested_root: PathBuf,
    directory: Arc<File>,
    root_stamp: FileStamp,
    mutations_allowed: bool,
    transfer_policy: TransferPolicy,
    disk_space: Arc<dyn DiskSpaceProbe>,
}

impl std::fmt::Debug for ModelLibrary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ModelLibrary")
            .field("root", &self.root)
            .field("mutations_allowed", &self.mutations_allowed)
            .field("transfer_policy", &self.transfer_policy)
            .finish_non_exhaustive()
    }
}

struct LibraryLock {
    _file: File,
}

struct Stage<'a> {
    library: &'a ModelLibrary,
    name: String,
    file: File,
}

impl Drop for Stage<'_> {
    fn drop(&mut self) {
        if let Ok(metadata) = self.file.metadata() {
            // Only the uniquely created file belonging to this operation is
            // cleaned up. Never sweep other sessions' stages or old backups.
            let _ = self
                .library
                .unlink_matching(&self.name, &FileStamp::from_metadata(&metadata));
        }
    }
}

#[derive(Clone, Copy)]
enum OpenMode {
    Read,
    Create,
    Lock,
}

impl ModelLibrary {
    /// Opens a pinned canonical root. An existing root reached through a symlink
    /// supports read-only discovery/resolution without creating library files;
    /// mutation requires reopening its canonical, non-symlinked path explicitly.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let requested_root = absolute_path(root.as_ref())?;
        let strict_root = absolute_without_symlinks(&requested_root);
        let mutations_allowed = strict_root.is_ok();
        if !requested_root.exists() {
            fs::create_dir_all(strict_root?)?;
        }
        let root = requested_root.canonicalize()?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x02000000 | 0x00200000);
        }
        let directory = options.open(&root)?;
        let metadata = directory.metadata()?;
        if !metadata.is_dir() {
            return Err(failure("Model library root must be a real directory"));
        }
        let library = Self {
            root,
            requested_root,
            directory: Arc::new(directory),
            root_stamp: FileStamp::from_metadata(&metadata),
            mutations_allowed,
            transfer_policy: TransferPolicy::default(),
            disk_space: Arc::new(|root: &Path| Ok(fs2::available_space(root)?)),
        };
        let _lock = library.read_lock()?;
        library.read_manifest()?;
        Ok(library)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_transfer_policy(mut self, policy: TransferPolicy) -> Result<Self> {
        if policy.max_redirects > 10
            || policy.total_timeout.is_zero()
            || policy.total_timeout > Duration::from_secs(24 * 60 * 60)
            || policy.disk_headroom_bytes == 0
        {
            return Err(failure(
                "Transfer policy requires at most ten redirects, a positive timeout of at most 24 hours, and positive disk headroom",
            ));
        }
        self.transfer_policy = policy;
        Ok(self)
    }

    pub fn with_disk_space_probe(mut self, probe: Arc<dyn DiskSpaceProbe>) -> Self {
        self.disk_space = probe;
        self
    }

    pub fn list(&self) -> Result<Vec<CatalogEntry>> {
        let _lock = self.read_lock()?;
        let manifest = self.read_manifest()?;
        Ok(scan_models_dir(&self.root)?
            .into_iter()
            .map(|info| {
                if self.mutations_allowed {
                    catalog_entry(info, &manifest)
                } else {
                    CatalogEntry {
                        info,
                        owned_id: None,
                        sha256: None,
                    }
                }
            })
            .collect())
    }

    pub fn scan(&self) -> Result<Vec<CatalogEntry>> {
        self.list()
    }

    pub fn resolve(&self, reference: &LocalModelRef, kind: ModelKind) -> Result<PathBuf> {
        let _lock = self.read_lock()?;
        self.read_manifest()?;
        reference.resolve(&self.root, kind)
    }

    pub fn resolve_with_vram(
        &self,
        reference: &LocalModelRef,
        kind: ModelKind,
        free_vram_bytes: Option<u64>,
    ) -> Result<PathBuf> {
        let _lock = self.read_lock()?;
        self.read_manifest()?;
        resolve_model_with_vram(
            reference.model.as_deref(),
            &self.root,
            kind,
            free_vram_bytes,
        )
    }

    pub fn import(&self, source: &Path) -> Result<ImportResult> {
        self.ensure_mutable_root()?;
        let source = absolute_without_symlinks(source)?;
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| failure("Model source requires a UTF-8 file name"))?;
        validate_basename(name)?;
        let mut input = open_regular_source(&source)?;
        let metadata = input.metadata()?;
        if metadata.len() > MAX_MODEL_BYTES {
            return Err(failure("Model import exceeds the 256 GiB file limit"));
        }
        {
            let _lock = self.lock()?;
            let manifest = self.read_manifest()?;
            if source == self.root.join(name) {
                let info = inspect_model(&source, name, metadata.len());
                return Ok(ImportResult {
                    entry: catalog_entry(info, &manifest),
                    already_present: true,
                });
            }
        }
        self.ensure_disk_space(metadata.len())?;
        let mut stage = self.stage()?;
        let (size, digest) =
            copy_and_hash_checked(&mut input, &mut stage.file, MAX_MODEL_BYTES, |count| {
                self.ensure_disk_space(count as u64)
            })?;
        if size != metadata.len()
            || FileStamp::from_metadata(&input.metadata()?) != FileStamp::from_metadata(&metadata)
        {
            return Err(failure(
                "Model source changed during import; retry after the write finishes",
            ));
        }
        stage.file.sync_all()?;
        self.ensure_disk_space(0)?;
        self.publish(&stage, name, size, digest)
    }

    /// Streams into a unique stage and verifies SHA256 before atomic publish.
    /// Redirects are bounded and individually authorized; fresh GET requests
    /// never forward authentication or referrer headers from a previous hop.
    /// HTTPS downgrades, URL credentials and opaque custom clients are refused.
    pub async fn download(
        &self,
        request: DownloadRequest,
        network: &NetworkConfig,
        cancel: Option<Arc<AtomicBool>>,
        progress: Option<&(dyn Fn(DownloadProgress) + Send + Sync)>,
    ) -> Result<ImportResult> {
        self.ensure_mutable_root()?;
        let deadline = Instant::now() + self.transfer_policy.total_timeout;
        validate_basename(&request.file_name)?;
        let expected = validate_digest(&request.expected_sha256)?;
        if request.max_bytes == 0 || request.max_bytes > MAX_MODEL_BYTES {
            return Err(failure(
                "Download byte limit must be positive and at most 256 GiB",
            ));
        }
        check_download_cancel(&cancel)?;
        let url =
            url::Url::parse(&request.url).map_err(|_| failure("Invalid model download URL"))?;
        {
            let _lock = self.lock()?;
            self.read_manifest()?;
            if self.child_exists(&request.file_name)? {
                return Err(failure(
                    "Model destination already exists; choose another file name",
                ));
            }
        }
        self.ensure_disk_space(0)?;
        let mut stage = self.stage()?;
        let mut response = self
            .download_response(url, network, &cancel, deadline)
            .await?;
        if !response.status().is_success() {
            return Err(failure(format!(
                "Model download returned HTTP {}",
                response.status()
            )));
        }
        let total = response.content_length();
        if total.is_some_and(|bytes| bytes > request.max_bytes) {
            return Err(failure("Model download exceeds the requested byte limit"));
        }
        self.ensure_disk_space(total.unwrap_or(0))?;
        let mut size = 0u64;
        let mut hash = Sha256::new();
        let notify = |downloaded_bytes| {
            if let Some(progress) = progress {
                progress(DownloadProgress {
                    downloaded_bytes,
                    total_bytes: total,
                });
            }
        };
        notify(0);
        while let Some(chunk) = download_wait(response.chunk(), &cancel, deadline).await? {
            if chunk.len() as u64 > request.max_bytes.saturating_sub(size) {
                return Err(failure("Model download exceeds the requested byte limit"));
            }
            self.ensure_disk_space(chunk.len() as u64)?;
            stage.file.write_all(&chunk)?;
            hash.update(&chunk);
            size += chunk.len() as u64;
            notify(size);
        }
        check_download_cancel(&cancel)?;
        let digest = format!("{:x}", hash.finalize());
        if digest != expected {
            return Err(failure(
                "Model download SHA256 mismatch; no model was published",
            ));
        }
        stage.file.sync_all()?;
        check_download_cancel(&cancel)?;
        remaining_transfer_time(deadline)?;
        self.ensure_disk_space(0)?;
        self.publish_checked(
            &stage,
            &request.file_name,
            size,
            digest,
            &cancel,
            Some(deadline),
        )
    }

    async fn download_response(
        &self,
        mut url: url::Url,
        network: &NetworkConfig,
        cancel: &Option<Arc<AtomicBool>>,
        deadline: Instant,
    ) -> Result<reqwest::Response> {
        let mut visited = HashSet::new();
        for redirects in 0..=self.transfer_policy.max_redirects {
            url.set_fragment(None);
            if !visited.insert(url.clone()) {
                return Err(failure(
                    "Model download redirect loop; no model was published",
                ));
            }
            let client = network.model_download_client(&url)?;
            // Request-level timeouts override the inference client's short total
            // timeout. The remaining deadline is shared by all hops and reads.
            let response = download_wait(
                client
                    .get(url.clone())
                    .timeout(remaining_transfer_time(deadline)?)
                    .send(),
                cancel,
                deadline,
            )
            .await?;
            if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                return Ok(response);
            }
            if redirects == self.transfer_policy.max_redirects {
                return Err(failure("Model download exceeded its redirect limit"));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| failure("Model download redirect has no valid Location header"))?;
            url = redirect_target(&url, location)?;
        }
        unreachable!("bounded redirect loop always returns")
    }

    fn ensure_disk_space(&self, additional_bytes: u64) -> Result<()> {
        self.ensure_root()?;
        let available = self.disk_space.available_bytes(&self.root)?;
        let required = additional_bytes
            .checked_add(self.transfer_policy.disk_headroom_bytes)
            .ok_or_else(|| {
                failure("Model transfer disk requirement exceeds the supported range")
            })?;
        if available < required {
            return Err(failure(format!(
                "Insufficient free disk space for model transfer: {available} bytes available, \
                 {required} required including {} bytes of reserved headroom; free space or choose another library",
                self.transfer_policy.disk_headroom_bytes
            )));
        }
        Ok(())
    }

    /// Only the shared manager may invoke this after lifecycle and approval
    /// checks. Applications cannot bypass those checks through a public unlink.
    pub(crate) fn quarantine_owned(
        &self,
        id: &str,
        _approval: RemovalApproval,
    ) -> Result<RemovalResult> {
        let _lock = self.lock()?;
        let mut manifest = self.read_manifest()?;
        let index = manifest
            .models
            .iter()
            .position(|model| model.id == id && matches!(model.state, OwnedState::Active))
            .ok_or_else(|| failure("Only an active library-owned model ID can be removed"))?;
        let model = manifest.models[index].clone();
        let mut file = self.open_child(&model.file_name, OpenMode::Read)?;
        // Keep this actual-inode lease until quarantine and its manifest commit
        // finish. Worker hosts hold shared leases, including for CPU residents.
        lock_owned_file_exclusively(&file)?;
        let metadata = file.metadata()?;
        if metadata.len() != model.size_bytes || FileStamp::from_metadata(&metadata) != model.stamp
        {
            return Err(failure(
                "Owned model changed; refusing to remove a replaced or modified file",
            ));
        }
        let (size, checksum) = copy_and_hash(&mut file, &mut std::io::sink(), model.size_bytes)?;
        if size != model.size_bytes || checksum != model.sha256 {
            return Err(failure("Owned model checksum changed; refusing removal"));
        }
        self.verify_child(&model.file_name, &model.stamp)?;
        let quarantine = format!("{INTERNAL_PREFIX}trash-{}.quarantine", Uuid::new_v4());
        self.link_child(&model.file_name, &quarantine)?;
        self.verify_child(&quarantine, &model.stamp)?;
        if let Err(error) = self.unlink_matching(&model.file_name, &model.stamp) {
            // Preserve the quarantine if cleanup cannot verify its identity.
            let _ = self.unlink_matching(&quarantine, &model.stamp);
            return Err(error);
        }
        manifest.models[index].state = OwnedState::Quarantined {
            file_name: quarantine.clone(),
        };
        if let Err(error) = self.save_manifest(&manifest) {
            // Best-effort rollback is no-clobber. If another file appeared, keep
            // the quarantined bytes rather than destroy either copy.
            if self.link_child(&quarantine, &model.file_name).is_ok() {
                let _ = self.unlink_matching(&quarantine, &model.stamp);
            }
            return Err(failure(format!(
                "Could not record model quarantine: {error}. Preserved bytes at {} or {}",
                self.root.join(&quarantine).display(),
                self.root.join(&model.file_name).display()
            )));
        }
        self.sync_directory()?;
        Ok(RemovalResult {
            id: model.id,
            original_path: self.root.join(model.file_name),
            quarantine_path: self.root.join(quarantine),
            sha256: model.sha256,
            size_bytes: model.size_bytes,
        })
    }

    fn ensure_root(&self) -> Result<()> {
        absolute_without_symlinks(&self.root)?;
        let metadata = fs::symlink_metadata(&self.root)?;
        if !metadata.is_dir()
            || !self
                .root_stamp
                .same_identity(&FileStamp::from_metadata(&metadata))
        {
            return Err(failure(
                "Model library root changed; reopen the library before continuing",
            ));
        }
        Ok(())
    }

    fn lock(&self) -> Result<LibraryLock> {
        self.ensure_mutable_root()?;
        let file = self.open_child(LOCK_NAME, OpenMode::Lock)?;
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(LibraryLock { _file: file }),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(failure(
                            "Model library is busy; retry after the current file operation",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn read_lock(&self) -> Result<Option<LibraryLock>> {
        if self.mutations_allowed {
            self.lock().map(Some)
        } else {
            // Atomic manifest replacement permits a read-only snapshot without
            // creating a lock file in a user-owned, linked directory.
            self.ensure_root()?;
            Ok(None)
        }
    }

    fn ensure_mutable_root(&self) -> Result<()> {
        if !self.mutations_allowed {
            return Err(failure(
                "Model library was opened through a symlink and is read-only; select its canonical directory explicitly for owned file operations",
            ));
        }
        absolute_without_symlinks(&self.requested_root)?;
        if self.requested_root.canonicalize()? != self.root {
            return Err(failure(
                "Model library root was retargeted; reopen it before managing files",
            ));
        }
        self.ensure_root()
    }

    fn stage(&self) -> Result<Stage<'_>> {
        self.ensure_mutable_root()?;
        let name = format!("{INTERNAL_PREFIX}stage-{}.part", Uuid::new_v4());
        let file = self.open_child(&name, OpenMode::Create)?;
        Ok(Stage {
            library: self,
            name,
            file,
        })
    }

    fn read_manifest(&self) -> Result<Manifest> {
        let mut file = match self.open_child(MANIFEST_NAME, OpenMode::Read) {
            Ok(file) => file,
            Err(RuntimeError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Manifest::default());
            }
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > MAX_MANIFEST_BYTES {
            return Err(failure(
                "Model ownership manifest exceeds its size limit; no changes made",
            ));
        }
        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(failure(
                "Model ownership manifest exceeds its size limit; no changes made",
            ));
        }
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| {
            failure("Model ownership manifest is corrupt; restore its backup before managing files")
        })?;
        if manifest.version != 1 || manifest.models.len() > MAX_MANIFEST_ENTRIES {
            return Err(failure(
                "Unsupported or oversized model ownership manifest; no changes made",
            ));
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for model in &manifest.models {
            validate_basename(&model.file_name)?;
            validate_digest(&model.sha256)?;
            if Uuid::parse_str(&model.id).is_err()
                || !ids.insert(&model.id)
                || model.kind != classify(&model.file_name)
                || model.size_bytes > MAX_MODEL_BYTES
            {
                return Err(failure(
                    "Invalid model ownership manifest entry; no changes made",
                ));
            }
            let stored_name = match &model.state {
                OwnedState::Active => &model.file_name,
                OwnedState::Quarantined { file_name } => {
                    if !file_name.starts_with(&format!("{INTERNAL_PREFIX}trash-"))
                        || !file_name.ends_with(".quarantine")
                        || file_name.contains(['/', '\\', ':'])
                        || file_name.len() > 120
                    {
                        return Err(failure("Invalid quarantine path in model manifest"));
                    }
                    file_name
                }
            };
            if !names.insert(stored_name) {
                return Err(failure("Duplicate model ownership path; no changes made"));
            }
        }
        Ok(manifest)
    }

    fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        if manifest.models.len() > MAX_MANIFEST_ENTRIES {
            return Err(failure(
                "Model ownership manifest is full; no model was published",
            ));
        }
        let bytes = serde_json::to_vec(manifest)?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(failure("Model ownership manifest exceeds its size limit"));
        }
        let mut stage = self.stage()?;
        stage.file.write_all(&bytes)?;
        stage.file.sync_all()?;
        self.ensure_root()?;
        // The manifest is the only replaceable library file, and callers hold
        // the advisory lock and have already parsed/validated its prior state.
        self.rename_child(&stage.name, MANIFEST_NAME)?;
        self.sync_directory()
    }

    fn publish(
        &self,
        stage: &Stage<'_>,
        name: &str,
        size: u64,
        digest: String,
    ) -> Result<ImportResult> {
        self.publish_checked(stage, name, size, digest, &None, None)
    }

    fn publish_checked(
        &self,
        stage: &Stage<'_>,
        name: &str,
        size: u64,
        digest: String,
        cancel: &Option<Arc<AtomicBool>>,
        deadline: Option<Instant>,
    ) -> Result<ImportResult> {
        // Publication rollback may unlink the new model. Prevent admission of
        // this inode until the entire ownership transaction has committed.
        lock_owned_file_exclusively(&stage.file)?;
        let _lock = self.lock()?;
        check_download_cancel(cancel)?;
        if let Some(deadline) = deadline {
            remaining_transfer_time(deadline)?;
        }
        self.ensure_disk_space(0)?;
        let mut manifest = self.read_manifest()?;
        if self.child_exists(name)? {
            if let Some(owned) = manifest
                .models
                .iter()
                .find(|model| model.file_name == name && matches!(model.state, OwnedState::Active))
            {
                let mut file = self.open_child(name, OpenMode::Read)?;
                if FileStamp::from_metadata(&file.metadata()?) == owned.stamp {
                    let (existing_size, existing_digest) = copy_and_hash_checked(
                        &mut file,
                        &mut std::io::sink(),
                        MAX_MODEL_BYTES,
                        |_| {
                            check_download_cancel(cancel)?;
                            if let Some(deadline) = deadline {
                                remaining_transfer_time(deadline)?;
                            }
                            Ok(())
                        },
                    )?;
                    if existing_size == size && existing_digest == digest && digest == owned.sha256
                    {
                        return Ok(ImportResult {
                            entry: catalog_entry(
                                inspect_model(&self.root.join(name), name, size),
                                &manifest,
                            ),
                            already_present: true,
                        });
                    }
                }
            }
            return Err(failure(
                "Model destination already exists; unmanaged or different files are never overwritten",
            ));
        }
        if manifest.models.len() >= MAX_MANIFEST_ENTRIES {
            return Err(failure(
                "Model ownership manifest is full; no model was published",
            ));
        }
        if manifest
            .models
            .iter()
            .any(|model| model.file_name == name && matches!(model.state, OwnedState::Active))
        {
            return Err(failure(
                "The manifest owns a missing file with this name; restore it or choose another name",
            ));
        }
        let stamp = FileStamp::from_metadata(&stage.file.metadata()?);
        let model = OwnedModel {
            id: Uuid::new_v4().to_string(),
            file_name: name.into(),
            kind: classify(name),
            size_bytes: size,
            sha256: digest,
            stamp: stamp.clone(),
            state: OwnedState::Active,
        };
        manifest.models.push(model);
        // Hard-link publication is atomic and fails if anything already exists
        // at the destination. Unlike rename, it can never clobber an unmanaged file.
        self.verify_child(&stage.name, &stamp)?;
        self.link_child(&stage.name, name)?;
        self.verify_child(name, &stamp)?;
        if let Err(error) = self.save_manifest(&manifest) {
            let _ = self.unlink_matching(name, &stamp);
            return Err(error);
        }
        Ok(ImportResult {
            entry: catalog_entry(inspect_model(&self.root.join(name), name, size), &manifest),
            already_present: false,
        })
    }

    fn child_exists(&self, name: &str) -> Result<bool> {
        self.ensure_root()?;
        match fs::symlink_metadata(self.root.join(name)) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    fn open_child(&self, name: &str, mode: OpenMode) -> Result<File> {
        match mode {
            OpenMode::Read => self.ensure_root()?,
            OpenMode::Create | OpenMode::Lock => self.ensure_mutable_root()?,
        }
        #[cfg(unix)]
        let file = {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name =
                std::ffi::CString::new(name).map_err(|_| failure("Invalid model file name"))?;
            let access = match mode {
                OpenMode::Read => libc::O_RDONLY,
                OpenMode::Create => libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
                OpenMode::Lock => libc::O_RDWR | libc::O_CREAT,
            };
            // The directory handle pins all mutation paths inside the root even
            // if a different process renames an ancestor during an operation.
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    access | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            unsafe { File::from_raw_fd(fd) }
        };
        #[cfg(not(unix))]
        let file = {
            let path = self.root.join(name);
            if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(failure("Symlinks are not allowed for managed model files"));
            }
            let mut options = OpenOptions::new();
            options.read(true);
            match mode {
                OpenMode::Read => {}
                OpenMode::Create => {
                    options.write(true).create_new(true);
                }
                OpenMode::Lock => {
                    options.write(true).create(true);
                }
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(0x00200000);
            }
            options.open(path)?
        };
        if !file.metadata()?.is_file() {
            return Err(failure(
                "Managed model path must be a regular file, not a symlink or device",
            ));
        }
        Ok(file)
    }

    fn verify_child(&self, name: &str, stamp: &FileStamp) -> Result<()> {
        let file = self.open_child(name, OpenMode::Read)?;
        if FileStamp::from_metadata(&file.metadata()?) != *stamp {
            return Err(failure(
                "Model file changed during the operation; no unknown files removed",
            ));
        }
        Ok(())
    }

    fn unlink_matching(&self, name: &str, stamp: &FileStamp) -> Result<()> {
        self.ensure_mutable_root()?;
        let file = self.open_child(name, OpenMode::Read)?;
        if *stamp != FileStamp::from_metadata(&file.metadata()?) {
            return Err(failure(
                "Refusing to clean up a file owned by another operation",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name =
                std::ffi::CString::new(name).map_err(|_| failure("Invalid model file name"))?;
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(unix))]
        fs::remove_file(self.root.join(name))?;
        Ok(())
    }

    fn link_child(&self, source: &str, destination: &str) -> Result<()> {
        self.ensure_mutable_root()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let source =
                std::ffi::CString::new(source).map_err(|_| failure("Invalid model file name"))?;
            let destination = std::ffi::CString::new(destination)
                .map_err(|_| failure("Invalid model file name"))?;
            let fd = self.directory.as_raw_fd();
            if unsafe { libc::linkat(fd, source.as_ptr(), fd, destination.as_ptr(), 0) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(unix))]
        fs::hard_link(self.root.join(source), self.root.join(destination))?;
        Ok(())
    }

    fn rename_child(&self, source: &str, destination: &str) -> Result<()> {
        self.ensure_mutable_root()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let source = std::ffi::CString::new(source)
                .map_err(|_| failure("Invalid manifest file name"))?;
            let destination = std::ffi::CString::new(destination)
                .map_err(|_| failure("Invalid manifest file name"))?;
            let fd = self.directory.as_raw_fd();
            if unsafe { libc::renameat(fd, source.as_ptr(), fd, destination.as_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(unix))]
        fs::rename(self.root.join(source), self.root.join(destination))?;
        Ok(())
    }

    fn sync_directory(&self) -> Result<()> {
        #[cfg(unix)]
        self.directory.sync_all()?;
        Ok(())
    }
}

fn lock_owned_file_exclusively(file: &File) -> Result<()> {
    match FileExt::try_lock_exclusive(file) {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
        {
            Err(failure(
                "Model file is in use by another worker or application; unload it before removing or replacing it",
            ))
        }
        Err(error) => Err(failure(format!("Cannot lock the owned model file: {error}"))),
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(failure(
            "Parent traversal is not allowed in model library paths",
        ));
    }
    Ok(absolute)
}

fn absolute_without_symlinks(path: &Path) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(failure("Symlinks are not allowed in managed model paths"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(absolute)
}

fn open_regular_source(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    let file = options.open(path).map_err(|error| {
        failure(format!(
            "Cannot open model source {}: {error}. Choose an existing regular file",
            path.display()
        ))
    })?;
    if !file.metadata()?.is_file() {
        return Err(failure("Model source must be an existing regular file"));
    }
    Ok(file)
}

fn copy_and_hash(
    input: &mut impl Read,
    output: &mut impl Write,
    limit: u64,
) -> Result<(u64, String)> {
    copy_and_hash_checked(input, output, limit, |_| Ok(()))
}

fn copy_and_hash_checked(
    input: &mut impl Read,
    output: &mut impl Write,
    limit: u64,
    mut before_write: impl FnMut(usize) -> Result<()>,
) -> Result<(u64, String)> {
    let mut buffer = [0; 64 * 1024];
    let mut size = 0u64;
    let mut hash = Sha256::new();
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if count as u64 > limit.saturating_sub(size) {
            return Err(failure("Model file exceeds the allowed byte limit"));
        }
        before_write(count)?;
        output.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
        size += count as u64;
    }
    Ok((size, format!("{:x}", hash.finalize())))
}

fn inspect_model(path: &Path, name: &str, size_bytes: u64) -> ModelInfo {
    let kind = classify(name);
    let GgufInfo {
        architecture,
        description,
    } = if matches!(
        kind,
        ModelKind::Llm | ModelKind::Embedding | ModelKind::Reranker
    ) {
        peek_gguf_info(path)
    } else {
        GgufInfo {
            architecture: None,
            description: None,
        }
    };
    let unstable_architecture = architecture
        .as_deref()
        .is_some_and(|a| KNOWN_UNSTABLE_ARCHITECTURES.contains(&a))
        || is_known_unstable_filename(name);
    ModelInfo {
        file_name: name.into(),
        path: path.into(),
        size_bytes,
        kind,
        architecture,
        description,
        unstable_architecture,
    }
}

fn catalog_entry(info: ModelInfo, manifest: &Manifest) -> CatalogEntry {
    let stamp = fs::symlink_metadata(&info.path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| FileStamp::from_metadata(&metadata));
    let owned = manifest.models.iter().find(|model| {
        model.file_name == info.file_name
            && matches!(model.state, OwnedState::Active)
            && stamp.as_ref() == Some(&model.stamp)
    });
    CatalogEntry {
        info,
        owned_id: owned.map(|model| model.id.clone()),
        sha256: owned.map(|model| model.sha256.clone()),
    }
}

fn check_download_cancel(cancel: &Option<Arc<AtomicBool>>) -> Result<()> {
    if cancel
        .as_ref()
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
    {
        return Err(RuntimeError::Cancelled);
    }
    Ok(())
}

async fn download_wait<T>(
    future: impl std::future::Future<Output = reqwest::Result<T>>,
    cancel: &Option<Arc<AtomicBool>>,
    deadline: Instant,
) -> Result<T> {
    check_download_cancel(cancel)?;
    let timeout = tokio::time::sleep(remaining_transfer_time(deadline)?);
    tokio::pin!(timeout);
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(20)), if cancel.is_some() => check_download_cancel(cancel)?,
            _ = &mut timeout => return Err(failure("Model download exceeded its total transfer deadline")),
            result = &mut future => {
                check_download_cancel(cancel)?;
                remaining_transfer_time(deadline)?;
                return result.map_err(|error| {
                    if error.is_timeout() {
                        failure("Model download exceeded its total transfer deadline")
                    } else {
                        failure(format!("Model download failed: {}", error.without_url()))
                    }
                });
            }
        }
    }
}

fn remaining_transfer_time(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| failure("Model download exceeded its total transfer deadline"))
}

fn redirect_target(current: &url::Url, location: &str) -> Result<url::Url> {
    let next = current
        .join(location)
        .map_err(|_| failure("Invalid model download redirect URL"))?;
    if !matches!(next.scheme(), "http" | "https")
        || next.host_str().is_none()
        || !next.username().is_empty()
        || next.password().is_some()
    {
        return Err(failure(
            "Model redirects require HTTP(S) URLs without embedded credentials",
        ));
    }
    if current.scheme() == "https" && next.scheme() != "https" {
        return Err(failure(
            "Model download refused an HTTPS-to-HTTP redirect downgrade",
        ));
    }
    Ok(next)
}

#[cfg(test)]
#[path = "models/managed_tests.rs"]
mod managed_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap()
    }

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
        assert!(!is_known_unstable_filename("Qwen3-4B-Instruct-Q4_K_M.gguf"));
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
        let dir = test_dir();
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
        let dir = test_dir();
        let missing = dir.path().join("does-not-exist");
        assert!(scan_models_dir(&missing).unwrap().is_empty());
    }

    #[test]
    fn scan_models_dir_skips_partial_downloads() {
        let dir = test_dir();
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
        let source_dir = test_dir();
        let models_dir = test_dir();
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
        let models_dir = test_dir();
        fs::write(models_dir.path().join("ggml-base.en.bin"), b"x").unwrap();
        fs::write(models_dir.path().join("llama-3.1-8b.Q4_K_M.gguf"), b"y").unwrap();

        let whisper_path = resolve_model(None, models_dir.path(), ModelKind::Whisper).unwrap();
        assert_eq!(whisper_path, models_dir.path().join("ggml-base.en.bin"));

        let llm_path = resolve_model(None, models_dir.path(), ModelKind::Llm).unwrap();
        assert_eq!(llm_path, models_dir.path().join("llama-3.1-8b.Q4_K_M.gguf"));
    }

    #[test]
    fn resolve_model_reports_a_helpful_error_when_nothing_found() {
        let models_dir = test_dir();
        let err = resolve_model(None, models_dir.path(), ModelKind::Whisper).unwrap_err();
        assert!(err.to_string().contains("huggingface.co"));
    }

    #[test]
    fn local_model_ref_resolve_matches_the_free_function() {
        let models_dir = test_dir();
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
        let elsewhere = test_dir();
        let models_dir = test_dir();
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
