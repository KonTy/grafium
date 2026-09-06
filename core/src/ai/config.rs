//! AI configuration — serializable settings for provider selection and tuning.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::model_library::LocalModelRef;

/// Top-level AI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    /// Whether AI features are enabled.
    pub enabled: bool,
    /// Which mode to use.
    pub mode: AiMode,
    /// Local provider config.
    pub local: Option<LocalConfig>,
    /// Cloud provider config.
    pub cloud: Option<CloudConfig>,
    /// Embedding settings.
    pub embedding: EmbeddingConfig,
    /// Reference generation settings.
    pub references: ReferenceConfig,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: AiMode::Local,
            local: Some(LocalConfig::default()),
            cloud: None,
            embedding: EmbeddingConfig::default(),
            references: ReferenceConfig::default(),
        }
    }
}

/// AI mode selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AiMode {
    /// Run everything locally (Ollama / HuggingFace models).
    Local,
    /// Use cloud APIs (OpenAI, Anthropic).
    Cloud,
    /// Use local for embeddings, cloud for complex reasoning.
    Hybrid,
}

/// Configuration for a specific provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: ProviderType,
    pub model: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Ollama,
    OpenAi,
    OpenAiCompatible,
    Anthropic,
    HuggingFace,
}

/// Local AI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    /// Local provider endpoint type.
    pub provider: ProviderType,
    /// Base URL for endpoint-based local providers (Ollama, vLLM/OpenAI-compatible).
    pub base_url: String,
    /// Optional API key for endpoint-based providers.
    pub api_key: Option<String>,
    /// LLM model name (used by endpoint-based providers: Ollama, vLLM/
    /// OpenAI-compatible). Ignored for `ProviderType::HuggingFace`, which
    /// uses `local_llm` instead.
    pub llm_model: String,
    /// Embedding model name.
    pub embedding_model: String,
    /// Where imported/downloaded model files live for the embedded local
    /// LLM (`local_llm`, used when `provider == ProviderType::HuggingFace`).
    /// `None` means the default (`<data_dir>/models` — see
    /// `model_library::default_models_dir`). Overridable so a user can
    /// point at a models folder they already keep elsewhere (e.g. a shared
    /// `~/Documents/models` used by several apps, or an external drive)
    /// instead of duplicating multi-gigabyte GGUF files into Grafium's own
    /// data directory. Mirrors `media::config::MediaConfig::models_dir`
    /// exactly — both are meant to be pointed at the very same shared
    /// folder once a Settings screen surfaces both.
    #[serde(default)]
    pub models_dir: Option<std::path::PathBuf>,
    /// Settings for the embedded local LLM runtime (llama.cpp), used only
    /// when `provider == ProviderType::HuggingFace`. Mirrors
    /// `media::config::WhisperSettings` — see `LocalLlmSettings`.
    pub local_llm: LocalLlmSettings,
    /// Settings for the embedded local embedding runtime (llama.cpp), used
    /// only when `provider == ProviderType::HuggingFace`. Lets the
    /// Embedded provider do semantic search / "Research this page" on its
    /// own instead of requiring the user to switch to Ollama or vLLM just
    /// to get an embedder. See `LocalEmbeddingSettings`.
    #[serde(default)]
    pub local_embedding: LocalEmbeddingSettings,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            provider: ProviderType::OpenAiCompatible,
            base_url: "http://localhost:8000/v1".to_string(),
            api_key: None,
            llm_model: "llama3.2".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            models_dir: None,
            local_llm: LocalLlmSettings::default(),
            local_embedding: LocalEmbeddingSettings::default(),
        }
    }
}

/// Settings for the embedded local LLM runtime (llama.cpp via
/// `llama-cpp-2`). Mirrors `media::config::WhisperSettings`'s shape — both
/// wrap the same `LocalModelRef` because "which model file" is the same
/// question, just resolved against a different `ModelKind`
/// (`ModelKind::Llm` here, `ModelKind::Whisper` there).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalLlmSettings {
    /// Which GGUF model file to use — see `model_library::LocalModelRef`
    /// for the exact resolution rules (bare file name, absolute path, or
    /// auto-pick). Flattened so the on-disk JSON stays flat.
    #[serde(flatten)]
    pub model_ref: LocalModelRef,
    /// Context window size in tokens. `None` uses the model's own trained
    /// context length (see `n_ctx_train` on the loaded model).
    pub context_size: Option<u32>,
    /// Number of transformer layers to offload to the GPU when built with
    /// a GPU feature (`llm-local-vulkan`); ignored on CPU-only builds.
    /// `None` offloads every layer — the common "just use the GPU" case.
    pub gpu_layers: Option<u32>,
    /// Whether llama.cpp is allowed to memory-map the model file
    /// (`llama_model_params.use_mmap`).
    ///
    /// `None`: pick automatically — mmap OFF for CPU-only loads (so the
    ///   RAM budget check up front is meaningful), mmap ON for GPU loads
    ///   (avoids duplicating tensor bytes into RAM before they get copied
    ///   to VRAM). Matches the historical behavior.
    /// `Some(false)`: force mmap OFF regardless. Safer on unreliable
    ///   storage (removable drives, network mounts, systems under heavy
    ///   disk pressure) — an mmap page-fault that fails to fault in a
    ///   page raises SIGBUS mid-generation and crashes the worker;
    ///   disabling mmap makes the whole model resident up front so no
    ///   later fault can fail. Slower initial load, higher peak RAM.
    /// `Some(true)`: force mmap ON regardless. Fastest load; only pick
    ///   this if you know your storage is reliable.
    ///
    /// The process wrapper (`LocalLlmProcess`) may also flip this to
    /// `Some(false)` at runtime after a SIGBUS-flavored worker crash,
    /// so a follow-up request auto-heals — see `handle_worker_crash`.
    pub use_mmap: Option<bool>,
}

/// Settings for the embedded local embedding runtime (llama.cpp via
/// `llama-cpp-2`), resolved against `ModelKind::Embedding` rather than
/// `ModelKind::Llm`. Deliberately smaller than `LocalLlmSettings` — GGUF
/// embedding models are tiny and fast enough that a configurable context
/// size / GPU offload knob hasn't been needed so far; add fields here if
/// that changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalEmbeddingSettings {
    /// Which GGUF embedding model file to use — see
    /// `model_library::LocalModelRef` for the exact resolution rules.
    /// Flattened so the on-disk JSON stays flat.
    #[serde(flatten)]
    pub model_ref: LocalModelRef,
}

/// Cloud AI configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudConfig {
    /// Which cloud provider to use for LLM.
    pub llm_provider: ProviderType,
    /// LLM model name.
    pub llm_model: String,
    /// API key for LLM provider (optional for self-hosted OpenAI-compatible endpoints).
    pub llm_api_key: Option<String>,
    /// Optional custom base URL for LLM provider.
    pub llm_base_url: Option<String>,
    /// Which provider for embeddings (can differ from LLM).
    pub embedding_provider: ProviderType,
    /// Embedding model name.
    pub embedding_model: String,
    /// API key for embedding provider (if different).
    pub embedding_api_key: Option<String>,
    /// Optional custom base URL for embedding provider.
    pub embedding_base_url: Option<String>,
}

/// Embedding pipeline settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Maximum tokens per chunk (for splitting large blocks).
    pub chunk_max_tokens: usize,
    /// Overlap tokens between chunks.
    pub chunk_overlap_tokens: usize,
    /// Batch size for embedding requests.
    pub batch_size: usize,
    /// Vector store path (relative to app data dir).
    pub vector_store_path: Option<PathBuf>,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            chunk_max_tokens: 512,
            chunk_overlap_tokens: 50,
            batch_size: 32,
            vector_store_path: None,
        }
    }
}

/// Reference generation settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceConfig {
    /// Maximum references per paragraph.
    pub max_refs_per_paragraph: usize,
    /// Minimum similarity score to include a reference (0.0 - 1.0).
    pub min_similarity_score: f32,
    /// Days before references are considered stale.
    pub staleness_days: u32,
    /// Whether to include cross-graph references.
    pub cross_graph: bool,
}

impl Default for ReferenceConfig {
    fn default() -> Self {
        Self {
            max_refs_per_paragraph: 5,
            min_similarity_score: 0.6,
            staleness_days: 7,
            cross_graph: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_config_defaults_to_no_models_dir_override() {
        assert_eq!(LocalConfig::default().models_dir, None);
    }

    #[test]
    fn local_config_models_dir_round_trips_through_json() {
        let mut config = LocalConfig::default();
        config.models_dir = Some(std::path::PathBuf::from("/home/user/Documents/models"));

        let json = serde_json::to_string(&config).unwrap();
        let parsed: LocalConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed.models_dir,
            Some(std::path::PathBuf::from("/home/user/Documents/models"))
        );
    }

    #[test]
    fn local_config_missing_models_dir_field_deserializes_to_none() {
        // Old configs saved before this field existed must still load fine.
        let json = r#"{
            "provider": "openaicompatible",
            "base_url": "http://localhost:8000/v1",
            "api_key": null,
            "llm_model": "llama3.2",
            "embedding_model": "nomic-embed-text",
            "local_llm": {}
        }"#;
        let parsed: LocalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.models_dir, None);
    }
}
