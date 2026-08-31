//! Embedding pipeline — chunking, batching, and indexing page content.
//!
//! Performance considerations:
//! - Batch embedding requests (configurable batch_size)
//! - Skip unchanged blocks (hash-based dirty checking)
//! - Background-friendly: yields between batches

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

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

/// Planned page update derived from the current chunk set and cached hashes.
#[derive(Debug, Clone)]
pub struct PageChunkDiff {
    pub dirty_chunks: Vec<TextChunk>,
    pub removed_chunk_ids: Vec<String>,
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

    /// Compute which chunks need to be re-embedded and which stale chunk IDs
    /// should be removed after a successful update.
    pub fn diff_page_chunks(&self, page_id: &str, chunks: Vec<TextChunk>) -> PageChunkDiff {
        let current_chunk_ids: HashSet<String> =
            chunks.iter().map(|chunk| chunk.chunk_id.clone()).collect();

        let dirty_chunks = chunks
            .into_iter()
            .filter(|chunk| {
                self.hash_cache
                    .get(&chunk.chunk_id)
                    .map_or(true, |cached_hash| cached_hash != &chunk.content_hash)
            })
            .collect();

        let removed_chunk_ids = self
            .hash_cache
            .keys()
            .filter(|chunk_id| {
                Self::chunk_belongs_to_page(chunk_id, page_id)
                    && !current_chunk_ids.contains(*chunk_id)
            })
            .cloned()
            .collect();

        PageChunkDiff {
            dirty_chunks,
            removed_chunk_ids,
        }
    }

    /// Mark successfully written chunks as clean in the hash cache.
    pub fn mark_chunks_clean(&mut self, chunks: &[TextChunk]) {
        for chunk in chunks {
            self.hash_cache
                .insert(chunk.chunk_id.clone(), chunk.content_hash.clone());
        }
    }

    /// Remove stale chunk IDs from the hash cache.
    pub fn remove_chunks(&mut self, chunk_ids: &[String]) {
        for chunk_id in chunk_ids {
            self.hash_cache.remove(chunk_id);
        }
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
                    current = super::suffix_to_char_boundary(&current, overlap_chars).to_string();
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

    pub fn preload_hashes(&mut self, hashes: HashMap<String, String>) {
        self.hash_cache.extend(hashes);
    }

    /// Invalidate cache for a specific page (forces re-embedding on next run).
    pub fn invalidate_page(&mut self, page_id: &str) {
        self.hash_cache
            .retain(|chunk_id, _| !Self::chunk_belongs_to_page(chunk_id, page_id));
    }

    /// Clear entire hash cache.
    pub fn clear_cache(&mut self) {
        self.hash_cache.clear();
    }

    fn chunk_belongs_to_page(chunk_id: &str, page_id: &str) -> bool {
        matches!(chunk_id.strip_prefix(page_id), Some(rest) if rest.starts_with(':'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::config::EmbeddingConfig;

    #[test]
    fn split_block_handles_utf8_overlap_boundaries() {
        let pipeline = EmbeddingPipeline::new(EmbeddingConfig {
            chunk_max_tokens: 4,
            chunk_overlap_tokens: 1,
            batch_size: 1,
            vector_store_path: None,
        });

        let chunks = pipeline.split_block("aaaaaaaaaa🙂. B.");

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "aaaaaaaaaa🙂.");
        assert!(chunks[1].starts_with('🙂'));
        assert!(chunks
            .iter()
            .all(|chunk| std::str::from_utf8(chunk.as_bytes()).is_ok()));
    }
}
