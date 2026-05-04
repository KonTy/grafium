//! Embedding pipeline — chunking, batching, and indexing page content.
//!
//! Performance considerations:
//! - Batch embedding requests (configurable batch_size)
//! - Skip unchanged blocks (hash-based dirty checking)
//! - Background-friendly: yields between batches

use sha2::{Digest, Sha256};
use std::collections::HashMap;

use crate::ai::config::EmbeddingConfig;
use crate::ai::traits::{ChunkEmbedding, Embedder, VectorStore};
use crate::error::Result;
use crate::models::{Block, Page};

/// A chunk of text ready for embedding.
#[derive(Debug, Clone)]
pub struct TextChunk {
    pub chunk_id: String,
    pub page_id: String,
    pub block_id: Option<String>,
    pub page_title: String,
    pub content: String,
    pub content_hash: String,
}

/// Pipeline state for embedding operations.
pub struct EmbeddingPipeline {
    config: EmbeddingConfig,
    /// Cache of content hashes to avoid re-embedding unchanged content.
    hash_cache: HashMap<String, String>,
}

impl EmbeddingPipeline {
    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            hash_cache: HashMap::new(),
        }
    }

    /// Chunk a page's blocks into embeddable text segments.
    /// Uses block-level granularity (natural chunk boundaries).
    /// Large blocks are split at sentence boundaries.
    pub fn chunk_page(&self, page: &Page, blocks: &[Block]) -> Vec<TextChunk> {
        let mut chunks = Vec::new();

        for block in blocks {
            let content = block.content.trim();
            if content.is_empty() || content.len() < 10 {
                continue;
            }

            let hash = Self::hash_content(content);

            // If block is small enough, use as-is.
            if content.len() <= self.config.chunk_max_tokens * 4 {
                chunks.push(TextChunk {
                    chunk_id: format!("{}:{}", page.id, block.id),
                    page_id: page.id.clone(),
                    block_id: Some(block.id.clone()),
                    page_title: page.title.clone(),
                    content: format!("{}\n\n{}", page.title, content),
                    content_hash: hash,
                });
            } else {
                // Split large blocks at sentence boundaries.
                let sub_chunks = self.split_block(content);
                for (i, sub_content) in sub_chunks.into_iter().enumerate() {
                    let sub_hash = Self::hash_content(&sub_content);
                    chunks.push(TextChunk {
                        chunk_id: format!("{}:{}:{}", page.id, block.id, i),
                        page_id: page.id.clone(),
                        block_id: Some(block.id.clone()),
                        page_title: page.title.clone(),
                        content: format!("{}\n\n{}", page.title, sub_content),
                        content_hash: sub_hash,
                    });
                }
            }
        }

        chunks
    }

    /// Filter out chunks that haven't changed (hash match).
    pub fn filter_unchanged(&mut self, chunks: Vec<TextChunk>) -> Vec<TextChunk> {
        chunks
            .into_iter()
            .filter(|chunk| {
                let cached = self.hash_cache.get(&chunk.chunk_id);
                let is_new = cached.map_or(true, |h| h != &chunk.content_hash);
                if !is_new {
                    return false;
                }
                // Update cache.
                self.hash_cache
                    .insert(chunk.chunk_id.clone(), chunk.content_hash.clone());
                true
            })
            .collect()
    }

    /// Embed and store chunks in batches.
    /// Returns the number of chunks successfully embedded.
    pub async fn embed_and_store(
        &self,
        chunks: &[TextChunk],
        graph_id: &str,
        embedder: &dyn Embedder,
        store: &dyn VectorStore,
    ) -> Result<usize> {
        if chunks.is_empty() {
            return Ok(0);
        }

        let mut total = 0;
        let batch_size = self.config.batch_size;

        for batch in chunks.chunks(batch_size) {
            let texts: Vec<String> = batch.iter().map(|c| c.content.clone()).collect();

            let embeddings = embedder.embed(&texts).await?;

            let chunk_embeddings: Vec<ChunkEmbedding> = batch
                .iter()
                .zip(embeddings.into_iter())
                .map(|(chunk, embedding)| ChunkEmbedding {
                    chunk_id: chunk.chunk_id.clone(),
                    graph_id: graph_id.to_string(),
                    page_id: chunk.page_id.clone(),
                    block_id: chunk.block_id.clone(),
                    page_title: chunk.page_title.clone(),
                    content: chunk.content.clone(),
                    embedding,
                    metadata: serde_json::json!({
                        "content_hash": chunk.content_hash,
                    }),
                })
                .collect();

            store.upsert(&chunk_embeddings).await?;
            total += chunk_embeddings.len();
        }

        Ok(total)
    }

    /// Split a large block into smaller chunks at sentence boundaries.
    fn split_block(&self, content: &str) -> Vec<String> {
        let max_chars = self.config.chunk_max_tokens * 4; // rough char estimate
        let overlap_chars = self.config.chunk_overlap_tokens * 4;

        let sentences: Vec<&str> = content
            .split_inclusive(|c| c == '.' || c == '!' || c == '?' || c == '\n')
            .collect();

        let mut chunks = Vec::new();
        let mut current = String::new();

        for sentence in sentences {
            if current.len() + sentence.len() > max_chars && !current.is_empty() {
                chunks.push(current.clone());
                // Overlap: keep the last portion.
                if current.len() > overlap_chars {
                    current = current[current.len() - overlap_chars..].to_string();
                }
                // Don't clear completely — overlap preserved above.
            }
            current.push_str(sentence);
        }

        if !current.is_empty() {
            chunks.push(current);
        }

        chunks
    }

    fn hash_content(content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())[..16].to_string()
    }

    /// Invalidate cache for a specific page (forces re-embedding on next run).
    pub fn invalidate_page(&mut self, page_id: &str) {
        self.hash_cache.retain(|k, _| !k.starts_with(page_id));
    }

    /// Clear entire hash cache.
    pub fn clear_cache(&mut self) {
        self.hash_cache.clear();
    }
}
