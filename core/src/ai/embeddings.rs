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

/// Extract `#hashtag` tokens from free text. A tag starts at `#` and runs
/// over alphanumeric / `_` / `-` / `/` characters (matching Grafium's tag
/// syntax, including nested `#a/b` tags). Returns tags *with* the leading
/// `#`, in order of appearance.
pub(crate) fn extract_hashtags(text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            // A tag must not be preceded by a non-whitespace char (avoids
            // matching things like `abc#def` or URL fragments).
            let preceded_ok = i == 0
                || bytes[i - 1].is_ascii_whitespace()
                || bytes[i - 1] == b'('
                || bytes[i - 1] == b'[';
            if preceded_ok {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() {
                    let c = bytes[j];
                    if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' || c == b'/' {
                        j += 1;
                    } else {
                        break;
                    }
                }
                if j > start {
                    tags.push(format!("#{}", &text[start..j]));
                    i = j;
                    continue;
                }
            }
        }
        i += 1;
    }
    tags
}

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
    /// Identifier for the embedding *scheme* — i.e. anything that changes the
    /// stored vector but isn't part of the hashed chunk text, chiefly the
    /// asymmetric document prefix a model family applies (`search_document: `
    /// for nomic, `passage: ` for e5, `` for none). Mixed into every content
    /// hash so a prefix/model-family change invalidates previously-embedded
    /// chunks and forces a rewrite, instead of leaving prefixed queries to run
    /// against unprefixed documents (which is worse than no prefixes at all).
    embedding_scheme: String,
}

impl EmbeddingPipeline {
    /// Max chars kept from any single ancestor's text in a breadcrumb, so a
    /// long parent can't dominate the chunk.
    const ANCESTOR_MAX_CHARS: usize = 80;
    /// Overall cap on the structural prefix length.
    const PREFIX_MAX_CHARS: usize = 300;
    /// Cycle/runaway guard for ancestor walks.
    const MAX_ANCESTOR_DEPTH: usize = 32;
    /// Version tag for the hash *format* itself (breadcrumb composition +
    /// scheme mixing). Bumping it invalidates every cached hash on purpose,
    /// e.g. if the composed-text layout ever changes.
    const HASH_FORMAT_VERSION: &'static str = "v2";

    pub fn new(config: EmbeddingConfig) -> Self {
        Self {
            config,
            hash_cache: HashMap::new(),
            embedding_scheme: String::new(),
        }
    }

    /// Set the embedding scheme identifier (the model family's document
    /// prefix). Changing it means previously-stored hashes no longer match,
    /// so the affected chunks are re-embedded on the next index run.
    pub fn set_embedding_scheme(&mut self, scheme: impl Into<String>) {
        self.embedding_scheme = scheme.into();
    }

    /// Chunk a page's blocks into embeddable text segments.
    /// Uses block-level granularity (natural chunk boundaries).
    /// Large blocks are split at sentence boundaries.
    ///
    /// Each chunk is embedded together with a deterministic structural
    /// *prefix* — a breadcrumb built from the page title, the block's
    /// ancestor chain (outermost first), and any tags inherited from those
    /// ancestors. This is the deterministic, zero-LLM-cost equivalent of
    /// Anthropic's "contextual retrieval": a block like "it was much better"
    /// only makes sense under its parent heading, so we bake that context
    /// into the embedded text (and, crucially, into the content hash — so
    /// *moving* a block re-embeds it under its new ancestors).
    pub fn chunk_page(&self, page: &Page, blocks: &[Block]) -> Vec<TextChunk> {
        let mut chunks = Vec::new();

        // Build the id→block map once so ancestor walks are O(depth), not
        // O(n) per block (which would be O(n²) across the page).
        let block_map: HashMap<&str, &Block> = blocks.iter().map(|b| (b.id.as_str(), b)).collect();

        for block in blocks {
            let content = block.content.trim();
            if content.is_empty() || content.len() < 10 {
                continue;
            }

            let prefix = Self::build_structural_prefix(page, block, &block_map);

            // If block is small enough, use as-is.
            if content.len() <= self.config.chunk_max_tokens * 4 {
                let embedded = Self::compose_embedded_text(&prefix, content);
                let hash = self.hash_content(&embedded);
                chunks.push(TextChunk {
                    chunk_id: format!("{}:{}", page.id, block.id),
                    page_id: page.id.clone(),
                    block_id: Some(block.id.clone()),
                    page_title: page.title.clone(),
                    content: embedded,
                    content_hash: hash,
                });
            } else {
                // Split large blocks at sentence boundaries.
                let sub_chunks = self.split_block(content);
                for (i, sub_content) in sub_chunks.into_iter().enumerate() {
                    let embedded = Self::compose_embedded_text(&prefix, &sub_content);
                    let sub_hash = self.hash_content(&embedded);
                    chunks.push(TextChunk {
                        chunk_id: format!("{}:{}:{}", page.id, block.id, i),
                        page_id: page.id.clone(),
                        block_id: Some(block.id.clone()),
                        page_title: page.title.clone(),
                        content: embedded,
                        content_hash: sub_hash,
                    });
                }
            }
        }

        chunks
    }

    /// Compose the final embedded text: `<prefix>\n\n<content>`.
    fn compose_embedded_text(prefix: &str, content: &str) -> String {
        if prefix.is_empty() {
            content.to_string()
        } else {
            format!("{prefix}\n\n{content}")
        }
    }

    /// Build the deterministic structural prefix (breadcrumb) for a block:
    /// `Page Title > Grandparent text > Parent text [tags: #a #b]`, with each
    /// ancestor truncated so a long parent can't dominate the chunk, and the
    /// whole prefix capped.
    fn build_structural_prefix(
        page: &Page,
        block: &Block,
        block_map: &HashMap<&str, &Block>,
    ) -> String {
        // Walk parent_id upward (innermost → outermost), with a cycle guard.
        let mut ancestors: Vec<&Block> = Vec::new();
        let mut current = block.parent_id.as_deref();
        let mut guard = 0usize;
        while let Some(pid) = current {
            let Some(parent) = block_map.get(pid) else {
                break;
            };
            ancestors.push(parent);
            current = parent.parent_id.as_deref();
            guard += 1;
            if guard >= Self::MAX_ANCESTOR_DEPTH {
                break;
            }
        }
        ancestors.reverse(); // outermost first

        let mut parts: Vec<String> = Vec::with_capacity(ancestors.len() + 1);
        let title = page.title.trim();
        if !title.is_empty() {
            parts.push(title.to_string());
        }
        for ancestor in &ancestors {
            let text = ancestor.content.trim();
            if text.is_empty() {
                continue;
            }
            let truncated = super::truncate_to_char_boundary(text, Self::ANCESTOR_MAX_CHARS).trim();
            if !truncated.is_empty() {
                parts.push(truncated.to_string());
            }
        }

        let mut breadcrumb = parts.join(" > ");

        // Inherited tags: hashtags found on ancestors (scanned untruncated),
        // deduped in first-seen order. The block's own tags stay in its body.
        let tags = Self::collect_inherited_tags(&ancestors);
        if !tags.is_empty() {
            breadcrumb.push_str(" [tags: ");
            breadcrumb.push_str(&tags.join(" "));
            breadcrumb.push(']');
        }

        super::truncate_to_char_boundary(&breadcrumb, Self::PREFIX_MAX_CHARS)
            .trim_end()
            .to_string()
    }

    /// Collect `#hashtag` tokens from ancestor block contents, deduped in
    /// first-seen (outermost-first) order.
    fn collect_inherited_tags(ancestors: &[&Block]) -> Vec<String> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut tags: Vec<String> = Vec::new();
        for ancestor in ancestors {
            for tag in extract_hashtags(&ancestor.content) {
                if seen.insert(tag.clone()) {
                    tags.push(tag);
                }
            }
        }
        tags
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

    /// Whether the hash cache is empty — used to decide if it needs restoring
    /// from the vector store after a fresh process start.
    pub fn hash_cache_is_empty(&self) -> bool {
        self.hash_cache.is_empty()
    }

    /// Seed the hash cache from `(chunk_id, content_hash)` pairs recovered from
    /// the vector store, so a restart doesn't re-embed content whose vectors
    /// already exist. Existing (fresher) entries are never overwritten.
    pub fn preload_hashes(&mut self, pairs: Vec<(String, String)>) {
        for (chunk_id, hash) in pairs {
            self.hash_cache.entry(chunk_id).or_insert(hash);
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

            let embeddings = embedder.embed_documents(&texts).await?;

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

    fn hash_content(&self, content: &str) -> String {
        let mut hasher = Sha256::new();
        // Mix the hash-format version and embedding scheme (document prefix)
        // into the digest. Use a unit separator so scheme and content can't be
        // confused for one another. A scheme change therefore changes the hash
        // and marks the chunk dirty, forcing re-embedding under the new prefix.
        hasher.update(Self::HASH_FORMAT_VERSION.as_bytes());
        hasher.update([0x1f]);
        hasher.update(self.embedding_scheme.as_bytes());
        hasher.update([0x1f]);
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())[..16].to_string()
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
    use crate::models::{Block, BlockType, Page};

    fn pipeline() -> EmbeddingPipeline {
        EmbeddingPipeline::new(EmbeddingConfig {
            chunk_max_tokens: 256,
            chunk_overlap_tokens: 16,
            batch_size: 1,
            vector_store_path: None,
        })
    }

    fn page(title: &str) -> Page {
        Page {
            id: "p1".to_string(),
            title: title.to_string(),
            file_path: None,
            created_at: 0,
            updated_at: 0,
            is_journal: false,
            properties: serde_json::Value::Null,
        }
    }

    fn block(id: &str, parent: Option<&str>, content: &str) -> Block {
        Block {
            id: id.to_string(),
            page_id: "p1".to_string(),
            parent_id: parent.map(|p| p.to_string()),
            order_index: 0,
            content: content.to_string(),
            block_type: BlockType::Text,
            properties: serde_json::Value::Null,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn embedded_text(chunks: &[TextChunk], block_id: &str) -> String {
        chunks
            .iter()
            .find(|c| c.block_id.as_deref() == Some(block_id))
            .expect("chunk exists")
            .content
            .clone()
    }

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

    #[test]
    fn root_block_prefix_is_just_page_title() {
        let pipeline = pipeline();
        let page = page("My Notes");
        let blocks = vec![block("b1", None, "A root level thought here")];

        let text = embedded_text(&pipeline.chunk_page(&page, &blocks), "b1");
        assert_eq!(text, "My Notes\n\nA root level thought here");
    }

    #[test]
    fn nested_block_prefix_has_full_ancestor_chain() {
        let pipeline = pipeline();
        let page = page("Journal");
        let blocks = vec![
            block("gp", None, "Grandparent heading"),
            block("p", Some("gp"), "Parent detail"),
            block("c", Some("p"), "The child block content that matters"),
        ];

        let text = embedded_text(&pipeline.chunk_page(&page, &blocks), "c");
        assert_eq!(
            text,
            "Journal > Grandparent heading > Parent detail\n\nThe child block content that matters"
        );
    }

    #[test]
    fn moving_a_block_changes_its_content_hash() {
        let pipeline = pipeline();
        let page = page("Notes");

        let before = vec![
            block("h1", None, "Section One"),
            block("h2", None, "Section Two"),
            block("leaf", Some("h1"), "A movable observation block"),
        ];
        let after = vec![
            block("h1", None, "Section One"),
            block("h2", None, "Section Two"),
            block("leaf", Some("h2"), "A movable observation block"),
        ];

        let hash_before = pipeline
            .chunk_page(&page, &before)
            .into_iter()
            .find(|c| c.block_id.as_deref() == Some("leaf"))
            .unwrap()
            .content_hash;
        let hash_after = pipeline
            .chunk_page(&page, &after)
            .into_iter()
            .find(|c| c.block_id.as_deref() == Some("leaf"))
            .unwrap()
            .content_hash;

        assert_ne!(
            hash_before, hash_after,
            "moving a block under a new parent must invalidate its embedding"
        );
    }

    #[test]
    fn changing_prefix_scheme_invalidates_previously_clean_chunks() {
        let page = page("Notes");
        let blocks = vec![block("root", None, "some content worth embedding here")];

        // Index once under the default (no-prefix) scheme and mark clean.
        let mut pipeline = pipeline();
        let chunks_v1 = pipeline.chunk_page(&page, &blocks);
        pipeline.mark_chunks_clean(&chunks_v1);
        assert!(
            pipeline
                .diff_page_chunks(&page.id, pipeline.chunk_page(&page, &blocks))
                .dirty_chunks
                .is_empty(),
            "unchanged content under the same scheme stays clean"
        );

        // Switch to a model family with a document prefix (e.g. nomic). The
        // same text must now be considered dirty so it gets re-embedded with
        // the prefix, instead of leaving unprefixed vectors matched against
        // prefixed queries.
        pipeline.set_embedding_scheme("search_document: ");
        let diff = pipeline.diff_page_chunks(&page.id, pipeline.chunk_page(&page, &blocks));
        assert_eq!(
            diff.dirty_chunks.len(),
            1,
            "a prefix-scheme change must mark the chunk dirty for re-embedding"
        );

        // And the hash genuinely differs between schemes.
        let hash_none = chunks_v1[0].content_hash.clone();
        let hash_prefixed = pipeline.chunk_page(&page, &blocks)[0].content_hash.clone();
        assert_ne!(hash_none, hash_prefixed);
    }

    #[test]
    fn long_ancestor_text_is_truncated() {
        let pipeline = pipeline();
        let page = page("P");
        let long_parent = "x".repeat(500);
        let blocks = vec![
            block("parent", None, &long_parent),
            block("child", Some("parent"), "child content goes here"),
        ];

        let text = embedded_text(&pipeline.chunk_page(&page, &blocks), "child");
        let breadcrumb = text.split("\n\n").next().unwrap();
        // Ancestor is capped at ANCESTOR_MAX_CHARS, far below 500.
        assert!(breadcrumb.len() < 200, "breadcrumb was {breadcrumb:?}");
        assert!(breadcrumb.starts_with("P > x"));
    }

    #[test]
    fn prefix_truncation_respects_utf8_boundaries() {
        let pipeline = pipeline();
        let page = page("P");
        // Multi-byte chars right around the truncation point.
        let long_parent = "🙂".repeat(200);
        let blocks = vec![
            block("parent", None, &long_parent),
            block("child", Some("parent"), "child content"),
        ];

        let text = embedded_text(&pipeline.chunk_page(&page, &blocks), "child");
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
    }

    #[test]
    fn inherited_tags_are_collected_from_ancestors() {
        let pipeline = pipeline();
        let page = page("P");
        let blocks = vec![
            block("root", None, "Health log #health #wellness"),
            block("child", Some("root"), "felt much better today after rest"),
        ];

        let text = embedded_text(&pipeline.chunk_page(&page, &blocks), "child");
        let breadcrumb = text.split("\n\n").next().unwrap();
        assert!(
            breadcrumb.contains("[tags: #health #wellness]"),
            "{breadcrumb}"
        );
    }

    #[test]
    fn extract_hashtags_finds_nested_and_dedups_order() {
        let tags = extract_hashtags("a #foo and #bar/baz plus not#atag end #foo");
        assert_eq!(tags, vec!["#foo", "#bar/baz", "#foo"]);
    }

    #[test]
    fn preload_hashes_seeds_cache_and_avoids_reembedding() {
        let mut pipeline = pipeline();
        assert!(pipeline.hash_cache_is_empty());

        let page = page("P");
        let blocks = vec![block("root", None, "some content to embed for indexing")];
        let chunks = pipeline.chunk_page(&page, &blocks);
        let expected_hash = chunks[0].content_hash.clone();
        let chunk_id = chunks[0].chunk_id.clone();

        // Simulate a fresh process: cache restored from stored vector metadata.
        pipeline.preload_hashes(vec![(chunk_id, expected_hash)]);
        assert!(!pipeline.hash_cache_is_empty());

        // Nothing dirty because the restored hash matches the current content.
        let diff = pipeline.diff_page_chunks(&page.id, pipeline.chunk_page(&page, &blocks));
        assert!(diff.dirty_chunks.is_empty());

        // preload never overwrites an existing (fresher) entry.
        pipeline.preload_hashes(vec![(
            pipeline.chunk_page(&page, &blocks)[0].chunk_id.clone(),
            "STALE".to_string(),
        )]);
        let diff = pipeline.diff_page_chunks(&page.id, pipeline.chunk_page(&page, &blocks));
        assert!(diff.dirty_chunks.is_empty());
    }
}
