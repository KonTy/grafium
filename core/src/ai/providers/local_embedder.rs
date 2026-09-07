//! Embedded local text embeddings via llama.cpp (through the `llama-cpp-2`
//! bindings) — the embedding-side counterpart to `local_llm::LocalLlm`.
//! Together they let `ProviderType::HuggingFace` ("Embedded") be fully
//! self-contained: chat completions *and* the embeddings that power
//! semantic search / "Research this page", with no separate Ollama/vLLM
//! endpoint required.
//!
//! Shares the same conventions as `LocalLlm`: resolves its model file
//! through `model_library` (`from_settings`/`from_config`), shares the
//! process-wide llama.cpp backend via `llama_shared::shared_backend`, and
//! runs inference inside `spawn_blocking` since llama.cpp is synchronous
//! CPU/GPU-bound work.
//!
//! Gated behind the `llm-local` Cargo feature, same as `local_llm`.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};

use super::llama_shared::{shared_backend, OFFLOAD_ALL_LAYERS};
use crate::ai::config::LocalEmbeddingSettings;
use crate::ai::traits::{BoxFuture, Embedder};
use crate::error::{CoreError, Result};
use crate::model_library::{self, ModelKind};

/// Context window used when the model's own trained context length isn't
/// usable. Embedding models are typically trained on short-to-medium
/// windows (512-8192 tokens); this is a conservative fallback, not a
/// commonly-hit case.
const DEFAULT_CTX_SIZE: u32 = 2048;

/// Upper bound applied to a model's own trained context length when
/// auto-deriving a default — same defensive cap `local_llm` applies, in
/// case a future embedding checkpoint advertises an unexpectedly huge
/// trained context (KV cache allocation scales directly with this).
const DEFAULT_AUTO_CTX_CAP: u32 = 8192;

/// Long enough to load a cold model from disk on a slow drive.
const EMBED_LOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// Indexing sends large batches, and the first one pays the load cost too.
const EMBED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Runs a GGUF embedding model fully in-process via llama.cpp. Stateless
/// per call beyond the loaded model, so one instance is reused across many
/// `embed()` calls — mirrors `LocalLlm` exactly.
pub struct LocalEmbedder {
    model_path: PathBuf,
    ctx_size: NonZeroU32,
    dimension: usize,
    name: String,
}

/// A loaded embedding model, resident in the worker child between requests.
///
/// Mirrors `local_llm::LlmSlot`: keeping the model loaded across calls is what
/// makes indexing bearable, since a re-load per batch would dominate the cost.
pub(crate) struct EmbedderSlot {
    model_path: PathBuf,
    backend: Arc<LlamaBackend>,
    model: Arc<LlamaModel>,
    ctx_size: NonZeroU32,
    dimension: usize,
}

impl EmbedderSlot {
    fn matches(&self, model_path: &Path) -> bool {
        self.model_path == model_path
    }
}

fn ensure_slot(slot: &mut Option<EmbedderSlot>, model_path: &Path) -> Result<()> {
    if slot.as_ref().is_some_and(|c| c.matches(model_path)) {
        return Ok(());
    }
    // Release the previous model's native memory before loading another.
    *slot = None;
    // Route llama.cpp's own logging through tracing before touching it. Without
    // this the load failed with nothing but "null result from llama cpp" and
    // the actual reason — an allocation refused, no Vulkan device — went
    // nowhere at all.
    crate::ai::providers::local_llm::install_llm_logging();
    let backend = shared_backend();
    let model_params = LlamaModelParams::default().with_n_gpu_layers(OFFLOAD_ALL_LAYERS);
    let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
        .map_err(|e| CoreError::Other(format!("failed to load embedding model: {e}")))?;
    let ctx_size = Some(model.n_ctx_train())
        .filter(|&n| n > 0)
        .map(|n| n.min(DEFAULT_AUTO_CTX_CAP))
        .and_then(NonZeroU32::new)
        .unwrap_or_else(|| NonZeroU32::new(DEFAULT_CTX_SIZE).expect("nonzero constant"));
    let dimension = model.n_embd() as usize;
    *slot = Some(EmbedderSlot {
        model_path: model_path.to_path_buf(),
        backend,
        model: Arc::new(model),
        ctx_size,
        dimension,
    });
    Ok(())
}

/// Model metadata, read in the child. Runs only in the worker process.
pub(crate) fn info_in_process(
    slot: &mut Option<EmbedderSlot>,
    model_path: &Path,
) -> Result<(u32, usize)> {
    ensure_slot(slot, model_path)?;
    let slot = slot.as_ref().expect("slot populated by ensure_slot");
    Ok((slot.ctx_size.get(), slot.dimension))
}

/// Embed in the child. Runs only in the worker process.
pub(crate) fn embed_in_process(
    slot: &mut Option<EmbedderSlot>,
    model_path: &Path,
    context_size: u32,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    ensure_slot(slot, model_path)?;
    let slot = slot.as_ref().expect("slot populated by ensure_slot");
    let ctx_size = NonZeroU32::new(context_size).unwrap_or(slot.ctx_size);
    embed_all(&slot.model, &slot.backend, ctx_size, texts)
}

impl LocalEmbedder {
    /// Prepare a GGUF embedding model for use.
    ///
    /// Deliberately does not load anything here. The model lives in the worker
    /// child, so a fault inside llama.cpp — an allocation failure, a driver
    /// reset — kills the worker rather than the application. The context
    /// length and embedding width still have to be known up front to size the
    /// vector store, so those are read once, in the child, and returned.
    pub fn load(model_path: &Path) -> Result<Self> {
        let (context_size, dimension) = match crate::ai::worker::execute(
            crate::ai::worker::WorkerRequest::EmbedderInfo {
                model_path: model_path.to_path_buf(),
            },
            EMBED_LOAD_TIMEOUT,
        )? {
            crate::ai::worker::WorkerOutput::EmbedderInfo {
                context_size,
                dimension,
            } => (context_size, dimension),
            _ => {
                return Err(CoreError::Other(
                    "embedding worker returned an unexpected response".to_string(),
                ))
            }
        };

        let ctx_size = NonZeroU32::new(context_size)
            .unwrap_or_else(|| NonZeroU32::new(DEFAULT_CTX_SIZE).expect("nonzero constant"));

        let name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("local-embedder")
            .to_string();

        Ok(Self {
            model_path: model_path.to_path_buf(),
            ctx_size,
            dimension,
            name,
        })
    }

    /// Settings-driven alternative to [`Self::load`]: resolves which GGUF
    /// file to use via the shared [`model_library`] (through
    /// `settings.model_ref`, against `ModelKind::Embedding`) instead of
    /// requiring an exact path up front — mirrors
    /// `LocalLlm::from_settings` exactly.
    pub fn from_settings(models_dir: &Path, settings: &LocalEmbeddingSettings) -> Result<Self> {
        let model_path = settings
            .model_ref
            .resolve(models_dir, ModelKind::Embedding)?;
        Self::load(&model_path)
    }

    /// Same as [`Self::from_settings`], but takes the whole
    /// [`crate::ai::config::AiConfig`] + app data dir instead of
    /// pre-extracted fields — mirrors `LocalLlm::from_config` exactly.
    pub fn from_config(config: &crate::ai::config::AiConfig, data_dir: &Path) -> Result<Self> {
        let local = config
            .local
            .as_ref()
            .ok_or_else(|| CoreError::Other("No local AI provider configured".to_string()))?;
        let models_dir = local
            .models_dir
            .clone()
            .unwrap_or_else(|| model_library::default_models_dir(data_dir));
        Self::from_settings(&models_dir, &local.local_embedding)
    }
}

impl Embedder for LocalEmbedder {
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        self.embed_prefixed(texts.to_vec(), "")
    }

    /// Documents (the indexed side) get this model family's document prefix.
    fn embed_documents<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        let prefix = prefixes_for(&self.name).document;
        self.embed_prefixed(texts.to_vec(), prefix)
    }

    /// A search query gets this model family's (asymmetric) query prefix.
    fn embed_query<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        let prefix = prefixes_for(&self.name).query;
        let texts = vec![text.to_string()];
        let fut = self.embed_prefixed(texts, prefix);
        Box::pin(async move {
            let mut out = fut.await?;
            out.pop()
                .ok_or_else(|| CoreError::Other("embedder returned no vector for query".into()))
        })
    }

    /// A batch of search queries all get this model family's query prefix.
    fn embed_queries<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        let prefix = prefixes_for(&self.name).query;
        self.embed_prefixed(texts.to_vec(), prefix)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.name
    }

    /// The document prefix identifies this model family's embedding scheme:
    /// nomic → `search_document: `, e5 → `passage: `, others → empty. Folding
    /// it into the content hash makes a family/prefix change re-embed stale,
    /// unprefixed documents instead of leaving them mismatched with prefixed
    /// queries.
    fn embedding_scheme_id(&self) -> String {
        prefixes_for(&self.name).document.to_string()
    }
}

impl LocalEmbedder {
    /// Shared body for [`embed`], [`embed_documents`] and [`embed_query`]:
    /// applies `prefix` (possibly empty) to every input, then runs the
    /// blocking llama.cpp embedding loop off the async runtime.
    fn embed_prefixed(
        &self,
        texts: Vec<String>,
        prefix: &str,
    ) -> BoxFuture<'_, Result<Vec<Vec<f32>>>> {
        let model_path = self.model_path.clone();
        let ctx_size = self.ctx_size;
        let prefix = prefix.to_string();

        Box::pin(async move {
            if texts.is_empty() {
                return Ok(vec![]);
            }
            let prepared: Vec<String> = if prefix.is_empty() {
                texts
            } else {
                texts.into_iter().map(|t| format!("{prefix}{t}")).collect()
            };
            // Blocking IPC, so it goes on the blocking pool: the round trip
            // covers a whole batch and can take seconds on a cold model.
            tokio::task::spawn_blocking(move || {
                match crate::ai::worker::execute(
                    crate::ai::worker::WorkerRequest::Embed {
                        model_path,
                        context_size: ctx_size.get(),
                        texts: prepared,
                    },
                    EMBED_TIMEOUT,
                )? {
                    crate::ai::worker::WorkerOutput::Embed(vectors) => Ok(vectors),
                    _ => Err(CoreError::Other(
                        "embedding worker returned an unexpected response".to_string(),
                    )),
                }
            })
            .await
            .map_err(|e| CoreError::Other(format!("embedding task panicked: {e}")))?
        })
    }
}

/// Asymmetric query/document prefixes some embedding families require.
/// Matched by substring against the lowercased model file name. Kept as a
/// small table so new families can be added without touching the embed path.
///
/// Only families whose published usage clearly mandates prefixes are listed;
/// everything else gets no prefix (applying the wrong instruction to a model
/// that doesn't expect it hurts more than helps, e.g. bge-m3 needs none).
struct EmbeddingPrefixes {
    query: &'static str,
    document: &'static str,
}

fn prefixes_for(model_name: &str) -> EmbeddingPrefixes {
    let lower = model_name.to_lowercase();
    if lower.contains("nomic") {
        // https://huggingface.co/nomic-ai/nomic-embed-text-v1.5
        EmbeddingPrefixes {
            query: "search_query: ",
            document: "search_document: ",
        }
    } else if lower.contains("e5-") || lower.contains("e5_") || lower.contains("multilingual-e5") {
        // intfloat E5 family
        EmbeddingPrefixes {
            query: "query: ",
            document: "passage: ",
        }
    } else {
        EmbeddingPrefixes {
            query: "",
            document: "",
        }
    }
}

/// Embeds each text one at a time: tokenize -> fresh batch -> decode ->
/// read the pooled sequence embedding, clearing the KV cache between texts
/// so they don't bleed into each other's context. Mirrors the pattern from
/// llama-cpp-rs's own `examples/embeddings` (the crate has no higher-level
/// "just embed this" helper, same situation `local_llm::generate` is in for
/// completions).
///
/// Embeddings are returned as-is (not L2-normalized): `vector_store`'s
/// cosine similarity already normalizes by magnitude internally, and no
/// other `Embedder` impl in this crate pre-normalizes either.
fn embed_all(
    model: &LlamaModel,
    backend: &LlamaBackend,
    ctx_size: NonZeroU32,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    let n_threads = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4);
    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(Some(ctx_size))
        // See the matching comment in `local_llm::generate` — without this,
        // a text longer than n_batch's default (2048) crashes the whole
        // process instead of returning an error.
        .with_n_batch(ctx_size.get())
        .with_n_ubatch(ctx_size.get())
        .with_n_threads(n_threads)
        .with_n_threads_batch(n_threads)
        .with_embeddings(true);

    let mut ctx = model.new_context(backend, ctx_params).map_err(|e| {
        // Longest text about to be sent through this context — the batch
        // size and prompt-tokens hints in the shared error shaper are
        // most useful when they reflect the biggest thing we were about
        // to try to embed. This is a rough estimate (we haven't
        // tokenized the texts yet); good enough for the hint.
        let biggest_char_count = texts.iter().map(|t| t.chars().count()).max().unwrap_or(0);
        CoreError::Other(super::local_llm::context_creation_error_message(
            &e.to_string(),
            ctx_size.get(),
            ctx_size.get(),
            biggest_char_count,
        ))
    })?;

    // llama.cpp pads n_ctx up to a multiple of 256 *after* fixing n_batch, so
    // a requested ctx_size that isn't a multiple of 256 comes back with
    // n_ctx > n_batch. Truncating to n_ctx would then overrun the batch, and
    // for a non-causal embedding model the ubatch has to hold the whole
    // sequence too. Take the smallest of the three limits llama.cpp actually
    // applied — going over any of them is an abort(), not a catchable error.
    let n_ctx = ctx
        .n_ctx()
        .min(ctx.n_batch())
        .min(ctx.n_ubatch())
        .max(1) as i32;
    let mut embeddings = Vec::with_capacity(texts.len());

    for text in texts {
        let tokens = model
            .str_to_token(text, AddBos::Always)
            .map_err(|e| CoreError::Other(format!("failed to tokenize text for embedding: {e}")))?;

        if tokens.is_empty() {
            embeddings.push(vec![0.0; model.n_embd() as usize]);
            continue;
        }

        let tokens = if tokens.len() as i32 >= n_ctx {
            &tokens[..(n_ctx as usize).saturating_sub(1).max(1)]
        } else {
            &tokens[..]
        };

        let mut batch = LlamaBatch::new(tokens.len().max(1), 1);
        batch
            .add_sequence(tokens, 0, false)
            .map_err(|e| CoreError::Other(format!("failed to queue text for embedding: {e}")))?;

        ctx.clear_kv_cache();
        ctx.decode(&mut batch)
            .map_err(|e| CoreError::Other(format!("llama.cpp embedding decode failed: {e}")))?;

        let embedding = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| CoreError::Other(format!("failed to read embedding output: {e}")))?;
        embeddings.push(embedding.to_vec());
    }

    Ok(embeddings)
}

#[cfg(test)]
mod config_tests {
    use super::*;
    use crate::ai::config::{AiConfig, LocalConfig, ProviderType};

    /// Same reasoning as `local_llm`'s equivalent test: proves
    /// `from_config` resolves against `LocalConfig::models_dir` when set,
    /// rather than always falling back to `<data_dir>/models`, without
    /// needing a real model file on disk (the "no model found" error names
    /// the directory that was actually searched).
    #[test]
    fn from_config_honors_models_dir_override_over_default_data_dir() {
        let data_dir = tempfile::tempdir().unwrap();
        let custom_models_dir = tempfile::tempdir().unwrap();

        let mut ai_config = AiConfig::default();
        ai_config.local = Some(LocalConfig {
            provider: ProviderType::HuggingFace,
            models_dir: Some(custom_models_dir.path().to_path_buf()),
            ..LocalConfig::default()
        });

        let err = LocalEmbedder::from_config(&ai_config, data_dir.path())
            .map(|_| ())
            .unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains(&custom_models_dir.path().display().to_string()),
            "expected error to name the configured models_dir ({}), got: {message}",
            custom_models_dir.path().display()
        );
    }

    #[test]
    fn prefixes_for_applies_asymmetric_nomic_and_e5_conventions() {
        let nomic = prefixes_for("nomic-embed-text-v1.5.f16.gguf");
        assert_eq!(nomic.query, "search_query: ");
        assert_eq!(nomic.document, "search_document: ");

        let e5 = prefixes_for("multilingual-e5-large.Q4_K_M.gguf");
        assert_eq!(e5.query, "query: ");
        assert_eq!(e5.document, "passage: ");

        // Families without a mandated convention get no prefix — applying the
        // wrong instruction hurts more than helps.
        let bge = prefixes_for("bge-m3-Q8_0.gguf");
        assert_eq!(bge.query, "");
        assert_eq!(bge.document, "");
    }
}
