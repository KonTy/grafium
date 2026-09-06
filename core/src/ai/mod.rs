//! AI Engine module — modular, provider-agnostic AI infrastructure.
//!
//! Architecture:
//! - `traits.rs` — Core abstractions (LlmProvider, VectorStore, Embedder)
//! - `providers/` — Concrete implementations (Ollama, OpenAI, Anthropic)
//! - `embeddings.rs` — Chunking + embedding pipeline
//! - `references.rs` — Reference generation from AI analysis
//! - `config.rs` — AI configuration management

pub mod config;
pub mod embeddings;
pub mod gpu_fit;
pub mod providers;
pub mod reasoning;
pub mod references;
pub mod traits;
pub mod web_research;

pub use config::{AiConfig, AiMode, ProviderConfig};
pub use embeddings::EmbeddingPipeline;
pub use references::ReferenceEngine;
pub use traits::{CompletionOptions, Embedder, LlmProvider, SearchResult, VectorStore};
pub use web_research::{
    Citation, ResearchTopic, WebResearchConfig, WebResearchEngine, WebResearchResult,
};

pub(crate) fn truncate_to_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) fn suffix_to_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut start = text.len() - max_bytes;
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    &text[start..]
}
