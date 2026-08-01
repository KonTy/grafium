//! Core AI traits — all providers implement these.
//! Designed for zero-cost abstraction: use `Box<dyn LlmProvider>` for runtime dispatch,
//! or generics for compile-time monomorphization in hot paths.

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Options for LLM completion requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionOptions {
    /// Maximum tokens to generate.
    pub max_tokens: Option<u32>,
    /// Sampling temperature (0.0 = deterministic, 1.0 = creative).
    pub temperature: Option<f32>,
    /// System prompt / context.
    pub system_prompt: Option<String>,
    /// Stop sequences.
    pub stop: Option<Vec<String>>,
}

impl Default for CompletionOptions {
    fn default() -> Self {
        Self {
            max_tokens: Some(2048),
            temperature: Some(0.3),
            system_prompt: None,
            stop: None,
        }
    }
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

/// Result from a vector similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Unique identifier of the stored chunk.
    pub chunk_id: String,
    /// The graph this chunk belongs to.
    pub graph_id: String,
    /// Page ID within the graph.
    pub page_id: String,
    /// Block ID (if block-level granularity).
    pub block_id: Option<String>,
    /// Page title for display.
    pub page_title: String,
    /// The actual text content of the chunk.
    pub content: String,
    /// Cosine similarity score (0.0 - 1.0).
    pub score: f32,
    /// Additional metadata.
    pub metadata: serde_json::Value,
}

/// A chunk with its embedding, ready for storage.
#[derive(Debug, Clone)]
pub struct ChunkEmbedding {
    pub chunk_id: String,
    pub graph_id: String,
    pub page_id: String,
    pub block_id: Option<String>,
    pub page_title: String,
    pub content: String,
    pub embedding: Vec<f32>,
    pub metadata: serde_json::Value,
}

// Type alias for async trait returns (avoids the `async_trait` macro overhead).
// Defined once in `async_util` and re-exported here so every AI provider
// (and, elsewhere, `scraping::browser::BrowserDriver`) shares the exact same
// type instead of each module declaring its own `Pin<Box<dyn Future<...>>>`.
pub use crate::async_util::BoxFuture;

/// LLM provider trait — abstracts over Ollama, OpenAI, Anthropic, etc.
pub trait LlmProvider: Send + Sync {
    /// Generate a completion from a prompt.
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
    ) -> BoxFuture<'a, Result<String>>;

    /// Provider name for logging/config.
    fn name(&self) -> &str;

    /// Check if the provider is available (connection test).
    fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>>;
}

/// Embedding model trait — separate from LLM because embedding models are different.
pub trait Embedder: Send + Sync {
    /// Generate embeddings for a batch of texts.
    /// Returns one vector per input text.
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>>;

    /// Embedding dimension (needed for vector store initialization).
    fn dimension(&self) -> usize;

    /// Model name for logging.
    fn model_name(&self) -> &str;
}

/// Vector store trait — abstracts over LanceDB, Qdrant, SQLite-vec, etc.
pub trait VectorStore: Send + Sync {
    /// Insert or update embeddings. Upsert semantics (same chunk_id = overwrite).
    fn upsert<'a>(&'a self, chunks: &'a [ChunkEmbedding]) -> BoxFuture<'a, Result<()>>;

    /// Find the top-k most similar chunks to the query embedding.
    /// `filter_graph_id`: optionally restrict to a specific graph.
    fn search<'a>(
        &'a self,
        query_embedding: &'a [f32],
        top_k: usize,
        filter_graph_id: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<SearchResult>>>;

    /// Delete all chunks belonging to a specific page.
    fn delete_by_page<'a>(
        &'a self,
        graph_id: &'a str,
        page_id: &'a str,
    ) -> BoxFuture<'a, Result<()>>;

    /// Delete a specific set of chunks within a graph.
    fn delete_chunks<'a>(
        &'a self,
        graph_id: &'a str,
        chunk_ids: &'a [String],
    ) -> BoxFuture<'a, Result<()>>;

    /// Delete all chunks belonging to a specific graph.
    fn delete_by_graph<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<()>>;

    /// Total number of stored vectors.
    fn count<'a>(&'a self) -> BoxFuture<'a, Result<usize>>;
}
