//! Reference generation engine — AI-powered cross-referencing.
//!
//! Given a page's content, this module:
//! 1. Extracts key concepts/entities from each paragraph
//! 2. Searches the vector store for related content
//! 3. Generates markdown footnote references
//! 4. Tracks staleness for incremental updates

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::ai::traits::{
    ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole, SearchResult,
    VectorStore,
};
use crate::ai::config::ReferenceConfig;
use crate::error::Result;

/// A generated reference for a specific location in a document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedReference {
    /// Reference number (1-based, unique per page).
    pub ref_number: usize,
    /// The paragraph/block index this reference is attached to.
    pub block_id: String,
    /// The text span that triggered the reference (entity/concept).
    pub anchor_text: String,
    /// Character offset within the block where the anchor starts.
    pub anchor_offset: usize,
    /// The reference content (what shows in the panel / footnote).
    pub reference_text: String,
    /// Links to related pages.
    pub related_pages: Vec<RelatedPage>,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f32,
    /// When this reference was generated.
    pub generated_at: i64,
}

/// A related page discovered by the reference engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedPage {
    pub page_id: String,
    pub page_title: String,
    pub graph_id: String,
    /// Relevance score.
    pub score: f32,
    /// Brief context snippet.
    pub snippet: String,
}

/// Metadata tracking reference freshness for a page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageReferencesMeta {
    pub page_id: String,
    pub generated_at: i64,
    pub content_hash: String,
    pub reference_count: usize,
    pub references: Vec<GeneratedReference>,
}

/// The reference engine — orchestrates AI analysis + vector search.
pub struct ReferenceEngine {
    config: ReferenceConfig,
}

impl ReferenceEngine {
    pub fn new(config: ReferenceConfig) -> Self {
        Self { config }
    }

    /// Generate references for a page's blocks.
    /// This is the main entry point — call on-demand when user triggers "Research".
    pub async fn generate_references(
        &self,
        page_id: &str,
        page_title: &str,
        blocks: &[(String, String)], // (block_id, content)
        graph_id: &str,
        llm: &dyn LlmProvider,
        embedder: &dyn Embedder,
        store: &dyn VectorStore,
    ) -> Result<PageReferencesMeta> {
        let mut all_refs: Vec<GeneratedReference> = Vec::new();
        let mut ref_counter = 1;

        for (block_id, content) in blocks {
            if content.trim().len() < 20 {
                continue;
            }

            // Step 1: Extract key concepts from this block.
            let concepts = self.extract_concepts(content, llm).await?;

            if concepts.is_empty() {
                continue;
            }

            // Step 2: For each concept, find related content via vector search.
            for concept in &concepts {
                let query_text = vec![format!("{} {}", page_title, concept.text)];
                let embeddings = embedder.embed(&query_text).await?;

                if embeddings.is_empty() {
                    continue;
                }

                let filter_graph = if self.config.cross_graph {
                    None
                } else {
                    Some(graph_id)
                };

                let results = store
                    .search(&embeddings[0], self.config.max_refs_per_paragraph, filter_graph)
                    .await?;

                // Filter out self-references and low-confidence results.
                let relevant: Vec<&SearchResult> = results
                    .iter()
                    .filter(|r| r.page_id != page_id)
                    .filter(|r| r.score >= self.config.min_similarity_score)
                    .collect();

                if relevant.is_empty() {
                    continue;
                }

                let related_pages: Vec<RelatedPage> = relevant
                    .iter()
                    .map(|r| RelatedPage {
                        page_id: r.page_id.clone(),
                        page_title: r.page_title.clone(),
                        graph_id: r.graph_id.clone(),
                        score: r.score,
                        snippet: truncate_snippet(&r.content, 150),
                    })
                    .collect();

                let avg_score =
                    related_pages.iter().map(|r| r.score).sum::<f32>() / related_pages.len() as f32;

                let reference_text = self
                    .format_reference(&concept.text, &related_pages)
                    .await;

                all_refs.push(GeneratedReference {
                    ref_number: ref_counter,
                    block_id: block_id.clone(),
                    anchor_text: concept.text.clone(),
                    anchor_offset: concept.offset,
                    reference_text,
                    related_pages,
                    confidence: avg_score,
                    generated_at: Utc::now().timestamp_millis(),
                });

                ref_counter += 1;
            }
        }

        let content_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            for (_, content) in blocks {
                hasher.update(content.as_bytes());
            }
            format!("{:x}", hasher.finalize())[..16].to_string()
        };

        Ok(PageReferencesMeta {
            page_id: page_id.to_string(),
            generated_at: Utc::now().timestamp_millis(),
            content_hash,
            reference_count: all_refs.len(),
            references: all_refs,
        })
    }

    /// Use LLM to extract key concepts/entities from a text block.
    async fn extract_concepts(
        &self,
        content: &str,
        llm: &dyn LlmProvider,
    ) -> Result<Vec<ConceptExtraction>> {
        let messages = vec![
            ChatMessage {
                role: MessageRole::System,
                content: CONCEPT_EXTRACTION_PROMPT.to_string(),
            },
            ChatMessage {
                role: MessageRole::User,
                content: content.to_string(),
            },
        ];

        let options = CompletionOptions {
            max_tokens: Some(512),
            temperature: Some(0.1),
            ..Default::default()
        };

        let response = llm.complete(&messages, &options).await?;

        // Parse the structured response.
        parse_concept_response(&response, content)
    }

    /// Format a reference entry for display.
    async fn format_reference(&self, concept: &str, related: &[RelatedPage]) -> String {
        let mut parts = vec![format!("**{}**", concept)];

        for page in related.iter().take(3) {
            parts.push(format!(
                "→ [[{}]] (score: {:.0}%): {}",
                page.page_title,
                page.score * 100.0,
                page.snippet
            ));
        }

        parts.join("\n")
    }

    /// Check if a page's references are stale.
    pub fn is_stale(&self, meta: &PageReferencesMeta) -> bool {
        let now = Utc::now().timestamp_millis();
        let age_ms = now - meta.generated_at;
        let staleness_ms = self.config.staleness_days as i64 * 24 * 60 * 60 * 1000;
        age_ms > staleness_ms
    }
}

/// An extracted concept from text.
#[derive(Debug, Clone)]
struct ConceptExtraction {
    text: String,
    offset: usize,
}

const CONCEPT_EXTRACTION_PROMPT: &str = r#"You are a knowledge extraction system. Given a text block, identify the key concepts, entities, and claims that would benefit from cross-referencing.

Return a JSON array of objects with:
- "text": the exact phrase from the input (must be a substring)
- "type": one of "concept", "entity", "claim", "term"

Rules:
- Extract 1-5 items maximum
- Only extract meaningful, referenceable items (not common words)
- The "text" must appear verbatim in the input
- Prefer noun phrases and technical terms

Example output:
[{"text": "machine learning", "type": "concept"}, {"text": "transformer architecture", "type": "term"}]

Return ONLY the JSON array, no other text."#;

/// Parse LLM response into ConceptExtraction structs.
fn parse_concept_response(response: &str, original_content: &str) -> Result<Vec<ConceptExtraction>> {
    // Try to parse as JSON array.
    let trimmed = response.trim();
    let json_str = if trimmed.starts_with('[') {
        trimmed
    } else {
        // Try to find JSON array within the response.
        trimmed
            .find('[')
            .and_then(|start| {
                trimmed[start..]
                    .rfind(']')
                    .map(|end| &trimmed[start..=start + end])
            })
            .unwrap_or("[]")
    };

    #[derive(Deserialize)]
    struct ConceptJson {
        text: String,
        #[serde(rename = "type")]
        _type: Option<String>,
    }

    let parsed: Vec<ConceptJson> = serde_json::from_str(json_str).unwrap_or_default();

    Ok(parsed
        .into_iter()
        .filter_map(|c| {
            let offset = original_content.find(&c.text)?;
            Some(ConceptExtraction {
                text: c.text,
                offset,
            })
        })
        .collect())
}

fn truncate_snippet(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}…", &text[..max_len])
    }
}
