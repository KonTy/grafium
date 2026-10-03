//! Embedded local text embeddings via llama.cpp (through the `llama-cpp-2`
//! bindings) — the embedding-side counterpart to `local_llm::LocalLlm`.
//! Together they let `ProviderType::HuggingFace` ("Embedded") be fully
//! self-contained: chat completions *and* the embeddings that power
//! semantic search / "Research this page", with no separate Ollama/vLLM
//! endpoint required.
//!
//! Shares the same conventions as `LocalLlm`: accepts a resolved model path, shares the
//! process-wide llama.cpp backend via `llama_shared::shared_backend`, and
//! runs inference inside `spawn_blocking` since llama.cpp is synchronous
//! CPU/GPU-bound work.
//!
//! Gated behind the `llm-local` Cargo feature, same as `local_llm`.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::{AddBos, LlamaModel};

use super::llama_shared::{shared_backend, OFFLOAD_ALL_LAYERS};
use super::native_gpu;
use crate::error::{Result, RuntimeError};
use crate::types::{BoxFuture, Embedder};

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

/// Indexing sends large batches, and the first one pays the load cost too.
const EMBED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Metadata-validated embedding provider. Weights are loaded on the first
/// nonempty embedding request, in the isolated worker rather than during setup.
pub struct LocalEmbedder {
    model_path: PathBuf,
    ctx_size: NonZeroU32,
    dimension: usize,
    name: String,
    gpu_layers: u32,
}

fn context_size(trained: u32) -> NonZeroU32 {
    NonZeroU32::new(trained.min(DEFAULT_AUTO_CTX_CAP))
        .unwrap_or_else(|| NonZeroU32::new(DEFAULT_CTX_SIZE).expect("nonzero constant"))
}

fn read_metadata(model_path: &Path) -> Result<(NonZeroU32, usize)> {
    let metadata = crate::gguf::read_embedding_metadata(model_path)?;
    Ok((
        context_size(metadata.context_length),
        metadata.embedding_length as usize,
    ))
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
    device: Option<native_gpu::Device>,
    requested_gpu_layers: u32,
    _model_file: super::model_file::LockedModelFile,
}

impl EmbedderSlot {
    fn matches(&self, model_path: &Path, context: Option<u32>, gpu_layers: u32) -> bool {
        self.model_path == model_path
            && context.is_none_or(|context| self.ctx_size.get() == context)
            && self.requested_gpu_layers == gpu_layers
    }
}

fn ensure_slot(
    slot: &mut Option<EmbedderSlot>,
    model_path: &Path,
    requested_context: Option<u32>,
    gpu_layers: u32,
) -> Result<()> {
    let canonical = super::model_file::canonical_model_path(model_path)?;
    let model_path = canonical.as_path();
    if slot
        .as_ref()
        .is_some_and(|c| c.matches(model_path, requested_context, gpu_layers))
    {
        return Ok(());
    }
    // Release the previous model's native memory before loading another.
    *slot = None;
    let model_file = super::model_file::LockedModelFile::acquire(model_path)?;
    let canonical = model_file.path().to_path_buf();
    let model_path = canonical.as_path();
    // Route llama.cpp's own logging through tracing before touching it. Without
    // this the load failed with nothing but "null result from llama cpp" and
    // the actual reason — an allocation refused, no Vulkan device — went
    // nowhere at all.
    crate::native::llm::install_llm_logging();
    let (context, _) = read_metadata(model_path)?;
    let context = match requested_context {
        Some(context) => NonZeroU32::new(crate::resources::safe_context_size(Some(context))?)
            .expect("validated nonzero context"),
        None => context,
    };
    crate::native::policy::validate_model_load(
        model_path,
        crate::native::policy::ModelWorkload::Llm {
            context_tokens: context.get(),
        },
    )?;
    let backend = shared_backend()?;
    let (model_params, mut device) = native_gpu::fitted_params(
        model_path,
        context,
        if cfg!(feature = "llm-local-vulkan") {
            gpu_layers
        } else {
            0
        },
        true,
    )?;
    let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
        .map_err(|e| RuntimeError::Other(format!("failed to load embedding model: {e}")))?;
    let ctx_size = context;
    let dimension = model.n_embd() as usize;
    if let Some(device) = &mut device {
        device.reserve_context()?;
    }
    super::worker::emit_resident(super::worker::ResidentModel {
        kind: super::worker::NativeModelKind::Embeddings,
        model_path: model_path.to_path_buf(),
        context_size: Some(ctx_size.get()),
        on_gpu: device.is_some(),
        gpu_layers: Some(if device.is_some() {
            (model_params.n_gpu_layers().max(0) as u32).min(model.n_layer().saturating_add(1))
        } else {
            0
        }),
    });
    *slot = Some(EmbedderSlot {
        model_path: model_path.to_path_buf(),
        backend,
        model: Arc::new(model),
        ctx_size,
        dimension,
        device,
        requested_gpu_layers: gpu_layers,
        _model_file: model_file,
    });
    Ok(())
}

/// Model metadata, read in the child. Runs only in the worker process.
pub(crate) fn info_in_process(
    slot: &mut Option<EmbedderSlot>,
    model_path: &Path,
) -> Result<(u32, usize)> {
    ensure_slot(slot, model_path, None, default_gpu_layers())?;
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
    embed_configured_in_process(slot, model_path, context_size, default_gpu_layers(), texts)
}

pub(crate) fn embed_configured_in_process(
    slot: &mut Option<EmbedderSlot>,
    model_path: &Path,
    context_size: u32,
    gpu_layers: u32,
    texts: &[String],
) -> Result<Vec<Vec<f32>>> {
    ensure_slot(slot, model_path, Some(context_size), gpu_layers)?;
    let slot = slot.as_ref().expect("slot populated by ensure_slot");
    let ctx_size = NonZeroU32::new(context_size).unwrap_or(slot.ctx_size);
    let additional = crate::resources::estimate_context_for_model(model_path, ctx_size.get())?;
    crate::native::policy::validate_inference_headroom("embedding context", additional)?;
    if let Some(device) = &slot.device {
        device.check_context()?;
    }
    embed_all(
        &slot.model,
        &slot.backend,
        ctx_size,
        texts,
        slot.device.clone(),
    )
}

fn default_gpu_layers() -> u32 {
    if cfg!(feature = "llm-local-vulkan") {
        OFFLOAD_ALL_LAYERS
    } else {
        0
    }
}

impl LocalEmbedder {
    /// Prepare a GGUF embedding model for use.
    ///
    /// Read only GGUF metadata, without loading tensor weights, initializing a
    /// GPU, spawning a worker, or waiting for the inference lock. This keeps
    /// engine construction/reconfiguration cheap even while another model runs.
    /// Native model validation and load errors are reported by the first
    /// embedding request, just as they are for the lazy local chat provider.
    pub fn load(model_path: &Path) -> Result<Self> {
        Self::load_with_options(model_path, None, Some(default_gpu_layers()))
    }

    /// Explicit shared-settings preparation. An absent GPU setting means CPU,
    /// unlike the legacy `load` convenience method's automatic GPU intent.
    /// Context and GPU limits remain subject to host policy and live admission.
    pub fn load_with_options(
        model_path: &Path,
        context_size: Option<u32>,
        gpu_layers: Option<u32>,
    ) -> Result<Self> {
        let model_path = super::model_file::canonical_model_path(model_path)?;
        let (ctx_size, dimension) = read_metadata(&model_path)?;
        let ctx_size = match context_size {
            Some(context) => NonZeroU32::new(crate::resources::safe_context_size(Some(context))?)
                .expect("validated nonzero context"),
            None => ctx_size,
        };
        let requested_gpu_layers = crate::resources::safe_gpu_layers(gpu_layers)?;
        let gpu_layers = crate::native::policy::safe_gpu_layers(Some(requested_gpu_layers))?;

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
            gpu_layers,
        })
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
                .ok_or_else(|| RuntimeError::Other("embedder returned no vector for query".into()))
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
        let gpu_layers = self.gpu_layers;
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
                match crate::native::worker::execute(
                    crate::native::worker::WorkerRequest::EmbedConfigured {
                        model_path,
                        context_size: ctx_size.get(),
                        gpu_layers,
                        texts: prepared,
                    },
                    EMBED_TIMEOUT,
                )? {
                    crate::native::worker::WorkerOutput::Embed(vectors) => Ok(vectors),
                    _ => Err(RuntimeError::Other(
                        "embedding worker returned an unexpected response".to_string(),
                    )),
                }
            })
            .await
            .map_err(|e| RuntimeError::Other(format!("embedding task panicked: {e}")))?
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
    device: Option<native_gpu::Device>,
) -> Result<Vec<Vec<f32>>> {
    let ctx_params = native_gpu::context_params(ctx_size, true);
    let mut pressure = native_gpu::PressureWatch::new(device);
    pressure.check()?;

    let mut ctx = model.new_context(backend, ctx_params).map_err(|e| {
        // Longest text about to be sent through this context — the batch
        // size and prompt-tokens hints in the shared error shaper are
        // most useful when they reflect the biggest thing we were about
        // to try to embed. This is a rough estimate (we haven't
        // tokenized the texts yet); good enough for the hint.
        let biggest_char_count = texts.iter().map(|t| t.chars().count()).max().unwrap_or(0);
        RuntimeError::Other(super::llm::context_creation_error_message(
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
    let n_ctx = ctx.n_ctx().min(ctx.n_batch()).min(ctx.n_ubatch()).max(1) as i32;
    let mut embeddings = Vec::with_capacity(texts.len());

    for text in texts {
        pressure.check()?;
        let tokens = model.str_to_token(text, AddBos::Always).map_err(|e| {
            RuntimeError::Other(format!("failed to tokenize text for embedding: {e}"))
        })?;

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
            .map_err(|e| RuntimeError::Other(format!("failed to queue text for embedding: {e}")))?;

        ctx.clear_kv_cache();
        ctx.decode(&mut batch)
            .map_err(|e| RuntimeError::Other(format!("llama.cpp embedding decode failed: {e}")))?;

        let embedding = ctx
            .embeddings_seq_ith(0)
            .map_err(|e| RuntimeError::Other(format!("failed to read embedding output: {e}")))?;
        embeddings.push(embedding.to_vec());
    }

    Ok(embeddings)
}

#[cfg(test)]
mod config_tests {
    use super::*;

    fn gguf_string(value: &str) -> Vec<u8> {
        let mut bytes = (value.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(value.as_bytes());
        bytes
    }

    fn metadata_fixture(entries: &[(&str, u32, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        // No tensors: this fixture cannot be loaded for inference. Successful
        // preparation therefore proves that it did not request a native load.
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u64).to_le_bytes());
        for (key, kind, value) in entries {
            bytes.extend(gguf_string(key));
            bytes.extend_from_slice(&kind.to_le_bytes());
            bytes.extend_from_slice(value);
        }
        bytes.resize(bytes.len().div_ceil(32) * 32, 0);
        bytes
    }

    fn model_fixture(architecture: &str, context: u32, dimension: u32) -> Vec<u8> {
        metadata_fixture(&[
            ("general.architecture", 8, gguf_string(architecture)),
            (
                &format!("{architecture}.context_length"),
                4,
                context.to_le_bytes().to_vec(),
            ),
            (
                &format!("{architecture}.embedding_length"),
                4,
                dimension.to_le_bytes().to_vec(),
            ),
        ])
    }

    #[test]
    fn prepares_metadata_without_loading_a_worker_or_tensor_weights() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nomic-embed.gguf");
        std::fs::write(&path, model_fixture("nomic-bert", 2048, 768)).unwrap();

        let provider = LocalEmbedder::load(&path).unwrap();
        assert_eq!(provider.dimension(), 768);
        assert_eq!(provider.ctx_size.get(), 2048);
        assert_eq!(provider.embedding_scheme_id(), "search_document: ");
    }

    #[test]
    fn explicit_context_and_cpu_settings_prepare_without_loading_a_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embed.gguf");
        std::fs::write(&path, model_fixture("bert", 2048, 384)).unwrap();
        let provider = LocalEmbedder::load_with_options(&path, Some(512), Some(0)).unwrap();
        assert_eq!(provider.ctx_size.get(), 512);
        assert_eq!(provider.gpu_layers, 0);
        assert_eq!(provider.dimension(), 384);
    }

    #[test]
    fn explicit_null_gpu_is_cpu_while_legacy_loading_preserves_automatic_intent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embed.gguf");
        std::fs::write(&path, model_fixture("bert", 2048, 384)).unwrap();
        let explicit = LocalEmbedder::load_with_options(&path, None, None).unwrap();
        assert_eq!(explicit.gpu_layers, 0);
        assert_eq!(explicit.ctx_size.get(), 2048);
        let legacy = LocalEmbedder::load(&path).unwrap();
        assert_eq!(
            legacy.gpu_layers,
            crate::native::policy::safe_gpu_layers(Some(default_gpu_layers())).unwrap()
        );
        assert!(LocalEmbedder::load_with_options(&path, Some(0), None).is_err());
        assert!(LocalEmbedder::load_with_options(&path, None, Some(u32::MAX)).is_err());
    }

    #[test]
    fn metadata_context_uses_the_same_cap_and_fallback_as_native_loading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embed.gguf");
        for (trained, expected) in [(0, 2048), (512, 512), (32768, 8192)] {
            std::fs::write(&path, model_fixture("bert", trained, 384)).unwrap();
            assert_eq!(LocalEmbedder::load(&path).unwrap().ctx_size.get(), expected);
        }
    }

    #[test]
    fn rejects_missing_or_malformed_metadata_instead_of_guessing_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embed.gguf");
        for bytes in [
            b"not a GGUF model".to_vec(),
            metadata_fixture(&[]),
            metadata_fixture(&[("general.architecture", 4, 1u32.to_le_bytes().to_vec())]),
            metadata_fixture(&[
                ("general.architecture", 8, gguf_string("bert")),
                ("bert.embedding_length", 8, gguf_string("768")),
                ("bert.context_length", 4, 512u32.to_le_bytes().to_vec()),
            ]),
            model_fixture("bert", 512, 0),
            model_fixture("bert", 512, u32::MAX),
        ] {
            std::fs::write(&path, bytes).unwrap();
            let error = LocalEmbedder::load(&path).err().expect("invalid metadata");
            assert!(error
                .to_string()
                .contains("Invalid embedding model metadata"));
        }
        assert!(LocalEmbedder::load(&dir.path().join("missing.gguf")).is_err());
    }

    #[tokio::test]
    async fn empty_embedding_request_does_not_load_the_lazy_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embed.gguf");
        std::fs::write(&path, model_fixture("bert", 512, 384)).unwrap();
        let provider = LocalEmbedder::load(&path).unwrap();
        assert!(provider.embed(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn deferred_load_errors_are_returned_instead_of_successful_embeddings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("embed.gguf");
        std::fs::write(&path, model_fixture("bert", 512, 384)).unwrap();
        let provider = LocalEmbedder::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        // No native worker is configured in this test process; even if it
        // were, admission cannot start one for the now-missing model.
        let result = provider.embed(&["test input".to_string()]).await;
        assert!(
            result.is_err(),
            "deferred load failures must reach the caller"
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
