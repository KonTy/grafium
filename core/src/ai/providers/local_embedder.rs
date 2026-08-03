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
use std::path::Path;
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

/// Runs a GGUF embedding model fully in-process via llama.cpp. Stateless
/// per call beyond the loaded model, so one instance is reused across many
/// `embed()` calls — mirrors `LocalLlm` exactly.
pub struct LocalEmbedder {
    backend: Arc<LlamaBackend>,
    model: Arc<LlamaModel>,
    ctx_size: NonZeroU32,
    dimension: usize,
    name: String,
}

impl LocalEmbedder {
    /// Loads a GGUF embedding model from `model_path`.
    pub fn load(model_path: &Path) -> Result<Self> {
        let backend = shared_backend();

        // Embedding models are small; always offload every layer when a
        // GPU backend is compiled in (no separate "gpu_layers" setting
        // needed — see `LocalEmbeddingSettings`'s doc comment).
        let model_params = LlamaModelParams::default().with_n_gpu_layers(OFFLOAD_ALL_LAYERS);

        let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
            .map_err(|e| CoreError::Other(format!("failed to load embedding model: {e}")))?;

        let ctx_size = Some(model.n_ctx_train())
            .filter(|&n| n > 0)
            .map(|n| n.min(DEFAULT_AUTO_CTX_CAP))
            .and_then(NonZeroU32::new)
            .unwrap_or_else(|| NonZeroU32::new(DEFAULT_CTX_SIZE).expect("nonzero constant"));

        let dimension = model.n_embd() as usize;

        let name = model_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("local-embedder")
            .to_string();

        Ok(Self {
            backend,
            model: Arc::new(model),
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
        let model = self.model.clone();
        let backend = self.backend.clone();
        let ctx_size = self.ctx_size;
        let texts = texts.to_vec();

        Box::pin(async move {
            if texts.is_empty() {
                return Ok(vec![]);
            }
            tokio::task::spawn_blocking(move || embed_all(&model, &backend, ctx_size, &texts))
                .await
                .map_err(|e| CoreError::Other(format!("embedding task panicked: {e}")))?
        })
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.name
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

    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| CoreError::Other(format!("failed to create llama context: {e}")))?;

    let n_ctx = ctx.n_ctx() as i32;
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
}
