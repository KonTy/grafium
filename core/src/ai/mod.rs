//! AI Engine module — modular, provider-agnostic AI infrastructure.
//!
//! Architecture:
//! - `traits.rs` — Core abstractions (LlmProvider, VectorStore, Embedder)
//! - `providers/` — Concrete implementations (Ollama, OpenAI, Anthropic)
//! - `embeddings.rs` — Chunking + embedding pipeline
//! - `references.rs` — Reference generation from AI analysis
//! - `config.rs` — AI configuration management

/// Appended to every prompt that produces an answer for the user.
///
/// Retrieved material is the user's own, and is routinely not in the language
/// they are asking in — a bilingual vocabulary note is the ordinary case.
/// With no explicit rule the model mirrors the language of its context instead
/// of the language of the question: an English question about setting
/// something up in a basement came back written entirely in Chinese, because
/// the top-scoring note was a Chinese-English glossary entry that happened to
/// gloss "basement" as "地下室".
///
/// Applies to the notes arm, the web arm, and the no-notes general arm alike,
/// since any of them can end up holding foreign-language context.
pub(crate) const ANSWER_LANGUAGE_RULE: &str =
    "Always write your answer in the language the user asked their question in, even when the \
notes or sources you are drawing on are in a different language. Quote foreign-language material \
verbatim where the exact wording matters, but translate or paraphrase it for the user instead of \
switching languages yourself.";

pub mod config;
pub mod embeddings;
pub mod gpu_fit;
pub mod providers;
pub mod reasoning;
pub mod text;
pub mod references;
pub mod resources;
pub mod traits;
pub mod web_research;
#[cfg(any(feature = "llm-local", feature = "media"))]
pub mod worker;

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
