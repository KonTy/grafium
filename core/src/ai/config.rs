//! AI configuration — serializable settings for provider selection and tuning.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::error::{CoreError, Result};
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

impl AiConfig {
    /// Clamp legacy or hand-edited collection limits before initialization.
    /// Model/context limits remain strict because silently accepting those can
    /// hand unsafe allocations to native runtimes.
    pub fn sanitize_collection_limits(&mut self) {
        self.embedding.chunk_max_tokens = self.embedding.chunk_max_tokens.clamp(1, 4_096);
        self.embedding.chunk_overlap_tokens = self
            .embedding
            .chunk_overlap_tokens
            .min(self.embedding.chunk_max_tokens.saturating_sub(1));
        self.embedding.batch_size = self.embedding.batch_size.clamp(1, 64);
        self.references.max_refs_per_paragraph =
            self.references.max_refs_per_paragraph.clamp(1, 50);
        self.references.min_similarity_score = self.references.min_similarity_score.clamp(0.0, 1.0);
    }

    pub fn validate(&self) -> Result<()> {
        let embedding = &self.embedding;
        if !(1..=4_096).contains(&embedding.chunk_max_tokens) {
            return Err(CoreError::Other(
                "Embedding chunk size must be between 1 and 4096 tokens".to_string(),
            ));
        }
        if embedding.chunk_overlap_tokens >= embedding.chunk_max_tokens {
            return Err(CoreError::Other(
                "Embedding overlap must be smaller than the chunk size".to_string(),
            ));
        }
        if !(1..=64).contains(&embedding.batch_size) {
            return Err(CoreError::Other(
                "Embedding batch size must be between 1 and 64".to_string(),
            ));
        }
        if !(1..=50).contains(&self.references.max_refs_per_paragraph) {
            return Err(CoreError::Other(
                "References per paragraph must be between 1 and 50".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.references.min_similarity_score) {
            return Err(CoreError::Other(
                "Reference similarity score must be between 0 and 1".to_string(),
            ));
        }
        if let (AiMode::Local, Some(local)) = (&self.mode, &self.local) {
            if local.provider == ProviderType::HuggingFace {
                crate::ai::resources::safe_context_size(local.local_llm.context_size)?;
                crate::ai::resources::safe_gpu_layers(local.local_llm.gpu_layers)?;
            }
        }
        Ok(())
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
    /// Context window size in tokens. `None` uses Grafium's safe 4096-token
    /// default rather than an arbitrarily large model-trained context.
    pub context_size: Option<u32>,
    /// Number of transformer layers to offload to the GPU when built with
    /// a GPU feature (`llm-local-vulkan`); ignored on CPU-only builds.
    /// `None` stays CPU-only. GPU offload requires an explicit layer count and
    /// the `GRAFIUM_ALLOW_GPU_OFFLOAD=1` safety opt-in.
    pub gpu_layers: Option<u32>,
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

    #[test]
    fn sanitize_collection_limits_repairs_legacy_unsafe_values() {
        let mut config = AiConfig::default();
        config.embedding.chunk_max_tokens = usize::MAX;
        config.embedding.chunk_overlap_tokens = usize::MAX;
        config.embedding.batch_size = usize::MAX;
        config.references.max_refs_per_paragraph = usize::MAX;
        config.references.min_similarity_score = f32::INFINITY;

        config.sanitize_collection_limits();

        assert_eq!(config.embedding.chunk_max_tokens, 4_096);
        assert_eq!(config.embedding.chunk_overlap_tokens, 4_095);
        assert_eq!(config.embedding.batch_size, 64);
        assert_eq!(config.references.max_refs_per_paragraph, 50);
        assert_eq!(config.references.min_similarity_score, 1.0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_zero_embedding_batch_size() {
        let mut config = AiConfig::default();
        config.embedding.batch_size = 0;

        assert!(config.validate().is_err());
    }
}
