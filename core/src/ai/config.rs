//! AI configuration — serializable settings for provider selection and tuning.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// LLM model name.
    pub llm_model: String,
    /// Embedding model name.
    pub embedding_model: String,
    /// Optional local model path for embedded local runtimes.
    pub model_path: Option<PathBuf>,
}

impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            provider: ProviderType::OpenAiCompatible,
            base_url: "http://localhost:8000/v1".to_string(),
            api_key: None,
            llm_model: "llama3.2".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            model_path: None,
        }
    }
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
