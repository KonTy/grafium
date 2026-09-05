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

    /// The model's context window in tokens, if the provider can report it.
    ///
    /// Used by the knowledge engine to size the retrieved-context budget so
    /// the assembled prompt cannot overflow the model's window (embedded
    /// llama.cpp hard-errors when the prompt token count reaches `n_ctx`).
    /// Remote providers that don't expose this cheaply return `None`, and
    /// callers fall back to a conservative default.
    fn context_window(&self) -> Option<usize> {
        None
    }

    /// Same as `complete`, but reports incremental output through `on_token`
    /// as it's produced, for callers that want to show the model "thinking"
    /// live instead of a silent wait. Still returns the full text at the
    /// end, same as `complete`.
    ///
    /// Providers that can't stream (e.g. a plain non-SSE HTTP JSON API) can
    /// rely on this default: wait for the full response, then invoke
    /// `on_token` once with it — callers should treat "one big chunk" and
    /// "many small chunks" as equally valid. `LocalLlm` overrides this with
    /// genuine token-by-token streaming, since that's the provider slow
    /// enough (CPU-bound local inference) for live feedback to matter.
    fn complete_stream<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        options: &'a CompletionOptions,
        on_token: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let text = self.complete(messages, options).await?;
            on_token(&text);
            Ok(text)
        })
    }
}

/// Embedding model trait — separate from LLM because embedding models are different.
pub trait Embedder: Send + Sync {
    /// Generate embeddings for a batch of texts.
    /// Returns one vector per input text.
    fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>>;

    /// Embed a batch of *documents* (the indexed side of retrieval).
    ///
    /// Asymmetric embedding models (e.g. Nomic's `search_document:` /
    /// `search_query:` convention, or E5's `passage:` / `query:`) need the
    /// document and query sides prefixed differently or retrieval quality
    /// drops materially. The default delegates to [`embed`] for models that
    /// don't distinguish the two; providers that do should override both this
    /// and [`embed_query`].
    fn embed_documents<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        self.embed(texts)
    }

    /// Embed a single search *query* (the lookup side of retrieval). See
    /// [`embed_documents`] for why query and document sides differ.
    fn embed_query<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>>> {
        Box::pin(async move {
            let texts = [text.to_string()];
            let mut out = self.embed_queries(&texts).await?;
            out.pop().ok_or_else(|| {
                crate::error::CoreError::Other("embedder returned no vector for query".into())
            })
        })
    }

    /// Embed a batch of search *queries* (the lookup side of retrieval).
    /// Same asymmetry rationale as [`embed_documents`]; the default delegates
    /// to [`embed`].
    fn embed_queries<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        self.embed(texts)
    }

    /// Embedding dimension (needed for vector store initialization).
    fn dimension(&self) -> usize;

    /// Model name for logging.
    fn model_name(&self) -> &str;

    /// Stable identifier for anything that affects the *stored vector* but is
    /// not part of the hashed chunk text — chiefly the asymmetric document
    /// prefix this model family applies. It is folded into the content hash so
    /// that changing the prefix scheme (or switching to a family with a
    /// different one) invalidates previously-embedded chunks and forces a
    /// rewrite, rather than silently running prefixed queries against
    /// unprefixed documents. Default: empty (no prefix).
    fn embedding_scheme_id(&self) -> String {
        String::new()
    }
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

    /// Number of stored vectors belonging to a specific graph.
    fn count_for_graph<'a>(&'a self, graph_id: &'a str) -> BoxFuture<'a, Result<usize>>;

    /// List `(chunk_id, content_hash)` for every stored chunk in a graph, so a
    /// fresh process can rebuild its in-memory hash cache from vectors that
    /// already exist on disk and avoid needlessly re-embedding unchanged
    /// content after a restart. Default: empty (no restore available).
    fn list_content_hashes<'a>(
        &'a self,
        graph_id: &'a str,
    ) -> BoxFuture<'a, Result<Vec<(String, String)>>> {
        let _ = graph_id;
        Box::pin(async move { Ok(Vec::new()) })
    }
}
