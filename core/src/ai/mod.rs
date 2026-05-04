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
pub mod providers;
pub mod references;
pub mod traits;

pub use config::{AiConfig, AiMode, ProviderConfig};
pub use embeddings::EmbeddingPipeline;
pub use references::ReferenceEngine;
pub use traits::{CompletionOptions, Embedder, LlmProvider, SearchResult, VectorStore};
