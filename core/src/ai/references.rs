//! Reference generation engine — AI-powered cross-referencing.
//!
//! Given a page's content, this module:
//! 1. Extracts key concepts/entities from each paragraph
//! 2. Searches the vector store for related content
//! 3. Generates markdown footnote references
//! 4. Tracks staleness for incremental updates

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::ai::config::ReferenceConfig;
use crate::ai::traits::{
    ChatMessage, CompletionOptions, Embedder, LlmProvider, MessageRole, SearchResult, VectorStore,
};
use crate::error::{CoreError, Result};
use crate::parser::TagTerm;

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
    /// AI-generated summary of the page, produced alongside references.
    /// `None` if summarization failed or produced no output — summary
    /// generation is best-effort and never fails the whole "Research this
    /// page" operation.
    pub summary: Option<PageSummary>,
}

/// A short AI-generated digest of a page's content, broken out per topic
/// so multi-subject content (e.g. a long podcast transcript jumping
/// between many unrelated subjects) gets one paragraph per subject
/// instead of a single blended summary that risks dropping topics —
/// important since the eventual workflow is to delete the original
/// transcript/source text and keep only this summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    /// A single sentence directly answering/addressing the page title, when
    /// the title poses a question or makes a claim worth answering (e.g. a
    /// title like "Is Rust Faster Than C++?" gets a one-line answer).
    /// `None` for purely descriptive titles (e.g. "Meeting Notes").
    pub title_answer: Option<String>,
    /// One entry per distinct topic/subject discussed in the content, in
    /// the order they're covered. Content about a single subject still
    /// produces exactly one entry.
    pub topics: Vec<TopicSummary>,
}

impl PageSummary {
    /// All tags across every topic, in first-seen order and deduplicated
    /// case-insensitively by term — used when a caller just needs a flat
    /// display-label list to render (e.g. as hashtags) and topic
    /// boundaries don't matter. Uses each tag's `label()` (the qualified
    /// disambiguated phrase, if any, else the bare term).
    pub fn all_tags(&self) -> Vec<String> {
        self.all_tag_terms()
            .into_iter()
            .map(|tag| tag.label().to_string())
            .collect()
    }

    /// All tags across every topic as full [`TagTerm`] structs (preserving
    /// any `qualified` disambiguation), in first-seen order and
    /// deduplicated case-insensitively by `term` — used when a caller
    /// needs to find-and-wrap the terms in place (e.g.
    /// [`crate::parser::wrap_known_terms_as_links`]) rather than just
    /// display them.
    pub fn all_tag_terms(&self) -> Vec<TagTerm> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for topic in &self.topics {
            for tag in &topic.tags {
                if seen.insert(tag.term.to_lowercase()) {
                    out.push(tag.clone());
                }
            }
        }
        out
    }
}

/// A single topic's summary paragraph and key terms, one of potentially
/// many that make up a [`PageSummary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicSummary {
    /// Short label for this specific topic (e.g. "Magnesium and sleep
    /// quality"), not the overall content's title.
    pub topic: String,
    /// A concise paragraph summarizing what the content says about this
    /// topic specifically (not the whole piece).
    pub summary: String,
    /// Key terms identifying this topic, each an already-verbatim (or
    /// underscore/hyphen-joined) phrase found in the source content, with
    /// an optional `qualified` disambiguation for terms that would be
    /// ambiguous out of context (e.g. `term: "absorption"`,
    /// `qualified: Some("body absorption")`). Callers find-and-wrap these
    /// in place as real `[[wiki-link]]`s in the original text rather than
    /// only showing them in a separate summary panel.
    #[serde(default)]
    pub tags: Vec<TagTerm>,
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
        on_progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<PageReferencesMeta> {
        let mut all_refs: Vec<GeneratedReference> = Vec::new();
        let mut ref_counter = 1;
        let eligible_blocks = blocks
            .iter()
            .filter(|(_, content)| content.trim().len() >= 20)
            .map(|(block_id, content)| (block_id.as_str(), content.as_str()))
            .collect::<Vec<_>>();

        on_progress(&format!(
            "Analyzing {} block{} for key concepts (this can take a while on a local CPU \
             model)...",
            eligible_blocks.len(),
            if eligible_blocks.len() == 1 { "" } else { "s" }
        ));

        let mut pending_references = Vec::new();
        for ((block_id, _), concepts) in eligible_blocks.iter().copied().zip(
            self.extract_concepts_batch(&eligible_blocks, llm, on_progress)
                .await?
                .into_iter(),
        ) {
            for concept in concepts {
                pending_references.push(PendingReference {
                    block_id: block_id.to_string(),
                    concept,
                });
            }
        }

        let query_texts = pending_references
            .iter()
            .map(|pending| format!("{} {}", page_title, pending.concept.text))
            .collect::<Vec<_>>();
        let embeddings = if query_texts.is_empty() {
            Vec::new()
        } else {
            on_progress(&format!(
                "Generating embeddings for {} concept{}...",
                query_texts.len(),
                if query_texts.len() == 1 { "" } else { "s" }
            ));
            embedder.embed_queries(&query_texts).await?
        };
        if embeddings.len() != query_texts.len() {
            return Err(CoreError::Other(format!(
                "Embedder returned {} embeddings for {} texts",
                embeddings.len(),
                query_texts.len()
            )));
        }

        let filter_graph = if self.config.cross_graph {
            None
        } else {
            Some(graph_id)
        };

        if !pending_references.is_empty() {
            on_progress("Searching related pages...");
        }

        for (pending, embedding) in pending_references.into_iter().zip(embeddings.into_iter()) {
            let results = store
                .search(&embedding, self.config.max_refs_per_paragraph, filter_graph)
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
                .format_reference(&pending.concept.text, &related_pages)
                .await;

            all_refs.push(GeneratedReference {
                ref_number: ref_counter,
                block_id: pending.block_id,
                anchor_text: pending.concept.text,
                anchor_offset: pending.concept.offset,
                reference_text,
                related_pages,
                confidence: avg_score,
                generated_at: Utc::now().timestamp_millis(),
            });

            ref_counter += 1;
        }

        let content_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            for (_, content) in blocks {
                hasher.update(content.as_bytes());
            }
            format!("{:x}", hasher.finalize())[..16].to_string()
        };

        // Summarization is best-effort: a failure here (bad JSON from a
        // small/quantized model, provider hiccup, etc.) shouldn't discard
        // the references work already done above.
        let summary = if eligible_blocks.is_empty() {
            None
        } else {
            on_progress("Summarizing article...");
            let full_text = eligible_blocks
                .iter()
                .map(|(_, content)| *content)
                .collect::<Vec<_>>()
                .join("\n\n");
            match generate_page_summary(page_title, &full_text, llm, on_progress).await {
                Ok(summary) => Some(summary),
                Err(error) => {
                    on_progress(&format!("Could not generate a summary: {error}"));
                    None
                }
            }
        };

        Ok(PageReferencesMeta {
            page_id: page_id.to_string(),
            generated_at: Utc::now().timestamp_millis(),
            content_hash,
            reference_count: all_refs.len(),
            references: all_refs,
            summary,
        })
    }

    /// Use LLM to extract key concepts/entities from a text block.
    async fn extract_concepts(
        &self,
        content: &str,
        llm: &dyn LlmProvider,
        on_progress: &mut (dyn FnMut(&str) + Send),
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

        let response = stream_completion(
            llm,
            &messages,
            &concept_extraction_options(),
            "Extracting concepts: ",
            on_progress,
        )
        .await?;

        // Parse the structured response.
        parse_concept_response(&response, content)
    }

    async fn extract_concepts_batch(
        &self,
        blocks: &[(&str, &str)],
        llm: &dyn LlmProvider,
        on_progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Vec<Vec<ConceptExtraction>>> {
        if blocks.is_empty() {
            return Ok(Vec::new());
        }

        if blocks.len() == 1 {
            return Ok(vec![
                self.extract_concepts(blocks[0].1, llm, on_progress).await?,
            ]);
        }

        let request = blocks
            .iter()
            .map(|(block_id, content)| ConceptExtractionBlockInput { block_id, content })
            .collect::<Vec<_>>();
        let request_body = serde_json::to_string(&request).map_err(|error| {
            CoreError::Parse(format!(
                "Failed to serialize batched concept extraction request: {error}"
            ))
        })?;
        let messages = vec![
            ChatMessage {
                role: MessageRole::System,
                content: BATCH_CONCEPT_EXTRACTION_PROMPT.to_string(),
            },
            ChatMessage {
                role: MessageRole::User,
                content: request_body,
            },
        ];

        let response = stream_completion(
            llm,
            &messages,
            &concept_extraction_options(),
            "Extracting concepts: ",
            on_progress,
        )
        .await?;
        match parse_batched_concept_response(&response, blocks) {
            Ok(parsed) => Ok(parsed),
            Err(_) => {
                let mut extracted = Vec::with_capacity(blocks.len());
                for (_, content) in blocks {
                    extracted.push(self.extract_concepts(content, llm, on_progress).await?);
                }
                Ok(extracted)
            }
        }
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

/// Runs a completion through `LlmProvider::complete_stream`, forwarding
/// accumulated output to `on_progress` in small chunks (mirroring the
/// `ai_ask_stream` Tauri command's own chunking) rather than firing one
/// progress event per token — that would flood the UI for no benefit, since
/// all a caller needs is visibility that the model is actively producing
/// output, not a token-perfect replay.
async fn stream_completion(
    llm: &dyn LlmProvider,
    messages: &[ChatMessage],
    options: &CompletionOptions,
    label: &str,
    on_progress: &mut (dyn FnMut(&str) + Send),
) -> Result<String> {
    const CHUNK_CHARS: usize = 40;
    let mut buffer = String::new();
    let mut on_token = |piece: &str| {
        buffer.push_str(piece);
        if buffer.chars().count() >= CHUNK_CHARS {
            on_progress(&format!("{label}{buffer}"));
            buffer.clear();
        }
    };
    llm.complete_stream(messages, options, &mut on_token).await
}

/// Use the LLM to produce a one-line answer to a title (when it poses a
/// question/claim), a short prose summary, and topic hashtags for a piece
/// of content. Shared by `ReferenceEngine::generate_references` ("Research
/// this page") and the media-import pipeline (video/audio transcripts) —
/// both want the exact same "answer the title, summarize, tag" shape.
pub async fn generate_page_summary(
    title: &str,
    full_text: &str,
    llm: &dyn LlmProvider,
    on_progress: &mut (dyn FnMut(&str) + Send),
) -> Result<PageSummary> {
    // Keep the summarization prompt well within the context window even
    // for very long pages — this is a summary, not a full re-read, so a
    // generous prefix is enough context without risking the same
    // `n_ctx`-sized costs the concept-extraction pass already has to
    // manage for the full page.
    const MAX_SUMMARY_INPUT_CHARS: usize = 8000;
    let truncated_text = if full_text.len() > MAX_SUMMARY_INPUT_CHARS {
        super::truncate_to_char_boundary(full_text, MAX_SUMMARY_INPUT_CHARS)
    } else {
        full_text
    };

    let messages = vec![
        ChatMessage {
            role: MessageRole::System,
            content: PAGE_SUMMARY_PROMPT.to_string(),
        },
        ChatMessage {
            role: MessageRole::User,
            content: format!("Title: {title}\n\nContent:\n{truncated_text}"),
        },
    ];

    let response = stream_completion(
        llm,
        &messages,
        &summary_options(),
        "Summarizing: ",
        on_progress,
    )
    .await?;

    parse_summary_response(&response)
}

/// An extracted concept from text.
#[derive(Debug, Clone)]
struct ConceptExtraction {
    text: String,
    offset: usize,
}

#[derive(Deserialize)]
struct ConceptJson {
    text: String,
    #[serde(rename = "type")]
    _type: Option<String>,
}

#[derive(Serialize)]
struct ConceptExtractionBlockInput<'a> {
    block_id: &'a str,
    content: &'a str,
}

struct PendingReference {
    block_id: String,
    concept: ConceptExtraction,
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

const BATCH_CONCEPT_EXTRACTION_PROMPT: &str = r#"You are a knowledge extraction system. Given a JSON array of text blocks, identify the key concepts, entities, and claims in each block that would benefit from cross-referencing.

Input format:
[{"block_id": "block-1", "content": "..."}, {"block_id": "block-2", "content": "..."}]

Return a JSON array of objects with:
- "block_id": the matching block id from the input
- "concepts": an array of objects with:
  - "text": the exact phrase from that block's content (must be a substring)
  - "type": one of "concept", "entity", "claim", "term"

Rules:
- Return one object per input block
- Extract 0-5 items maximum per block
- Only extract meaningful, referenceable items (not common words)
- Each "text" must appear verbatim in that block's content
- Prefer noun phrases and technical terms
- If a block has no useful concepts, return an empty "concepts" array for it

Example output:
[{"block_id": "block-1", "concepts": [{"text": "machine learning", "type": "concept"}]}, {"block_id": "block-2", "concepts": []}]

Return ONLY the JSON array, no other text."#;

fn concept_extraction_options() -> CompletionOptions {
    CompletionOptions {
        max_tokens: Some(512),
        temperature: Some(0.1),
        ..Default::default()
    }
}

const PAGE_SUMMARY_PROMPT: &str = r##"You are a careful research assistant. You are given a title and content (an article, video/podcast transcript, or similar).

The content may cover a single subject or many distinct, unrelated subjects — for example a long podcast episode that jumps between many topics over its runtime. Identify EVERY distinct topic discussed. Do not skip minor topics and do not blend separate subjects into one paragraph: this summary may later fully replace the original content, so nothing meaningfully discussed should be lost.

Return a JSON object with:
- "title_answer": if the title poses a question or makes a claim that the content answers or supports/refutes, one sentence directly answering it using the content. If the title is purely descriptive (e.g. a name, a date, "Meeting Notes"), use null.
- "topics": an array with one object per distinct topic/subject discussed, in the order they're covered (or most-to-least important if a topic recurs throughout). If the content only covers one subject, return a single-element array. Each object has:
  - "topic": a short label for this specific subject (e.g. "Magnesium and sleep quality"), not the overall title.
  - "summary": a 2-5 sentence paragraph, in your own words, covering everything meaningful said about THIS topic specifically (not the whole piece).
  - "tags": an array of 1-4 key term objects for this topic. Each object has:
    - "term": a phrase taken VERBATIM from the content (lowercase is fine; use underscores instead of spaces for multi-word terms, no "#" prefix, e.g. "magnesium", "insulin_resistance"). Only use a term whose underlying words actually appear in the content — these are used to highlight/link the matching text in place, not just to label the summary. PREFER the longest already-verbatim phrase that is unambiguous on its own (e.g. use "soil_absorption" as the term if the content literally says "soil absorption"), rather than a short generic word.
    - "qualified": OPTIONAL. Only set this if "term" is a short, generic word that would be ambiguous or confusing out of context when linked on its own (e.g. bare "absorption" could mean bodily absorption or soil/chemical absorption) AND no longer verbatim phrase already disambiguates it. Give a short 2-3 word disambiguated phrase (e.g. "body absorption"). Omit or use null otherwise — most tags should NOT set this.

Example output (a two-topic segment, with one disambiguated tag):
{"title_answer": null, "topics": [{"topic": "Magnesium and sleep", "summary": "Magnesium glycinate was discussed as a supplement that can improve sleep onset and quality when taken before bed. The speaker noted most people are mildly deficient due to modern soil depletion and processed diets, and that the body's absorption of magnesium from food has declined.", "tags": [{"term": "magnesium"}, {"term": "sleep"}, {"term": "absorption", "qualified": "body absorption"}]}, {"topic": "Insulin resistance and diet", "summary": "The conversation shifted to insulin resistance, describing it as reduced cellular sensitivity to insulin that drives fat storage and fatigue. Cutting refined carbohydrates and adding resistance training were recommended as the most effective interventions.", "tags": [{"term": "insulin_resistance"}, {"term": "refined_carbohydrates"}]}]}

Return ONLY the JSON object, no other text."##;

fn summary_options() -> CompletionOptions {
    CompletionOptions {
        max_tokens: Some(1200),
        temperature: Some(0.3),
        ..Default::default()
    }
}

/// Parse LLM response into ConceptExtraction structs.
fn parse_concept_response(
    response: &str,
    original_content: &str,
) -> Result<Vec<ConceptExtraction>> {
    let trimmed = response.trim();
    let json_str = extract_json_array(trimmed)?;
    let parsed: Vec<ConceptJson> = serde_json::from_str(json_str).map_err(|error| {
        concept_parse_error(
            &format!("invalid concept extraction JSON: {}", error),
            trimmed,
        )
    })?;

    Ok(parse_concepts_for_content(parsed, original_content))
}

fn parse_batched_concept_response(
    response: &str,
    original_blocks: &[(&str, &str)],
) -> Result<Vec<Vec<ConceptExtraction>>> {
    #[derive(Deserialize)]
    struct BatchedConceptJson {
        block_id: String,
        #[serde(default)]
        concepts: Vec<ConceptJson>,
    }

    let trimmed = response.trim();
    let json_str = extract_json_array(trimmed)?;
    let parsed: Vec<BatchedConceptJson> = serde_json::from_str(json_str).map_err(|error| {
        concept_parse_error(
            &format!("invalid batched concept extraction JSON: {}", error),
            trimmed,
        )
    })?;

    let original_content_by_block = original_blocks
        .iter()
        .copied()
        .collect::<HashMap<&str, &str>>();
    let mut concepts_by_block = HashMap::with_capacity(parsed.len());
    for block in parsed {
        let Some(content) = original_content_by_block
            .get(block.block_id.as_str())
            .copied()
        else {
            continue;
        };
        concepts_by_block.insert(
            block.block_id,
            parse_concepts_for_content(block.concepts, content),
        );
    }

    Ok(original_blocks
        .iter()
        .map(|(block_id, _)| concepts_by_block.remove(*block_id).unwrap_or_default())
        .collect())
}

fn extract_json_array(response: &str) -> Result<&str> {
    if response.starts_with('[') {
        return Ok(response);
    }

    response
        .find('[')
        .and_then(|start| {
            response[start..]
                .rfind(']')
                .map(|end| &response[start..=start + end])
        })
        .ok_or_else(|| concept_parse_error("missing JSON array in response", response))
}

pub(crate) fn extract_json_object(response: &str) -> Result<&str> {
    if response.starts_with('{') {
        return Ok(response);
    }

    response
        .find('{')
        .and_then(|start| {
            response[start..]
                .rfind('}')
                .map(|end| &response[start..=start + end])
        })
        .ok_or_else(|| concept_parse_error("missing JSON object in response", response))
}

/// Accepts either a plain string tag (the old shape, or a model that
/// doesn't follow the newer `{term, qualified}` schema) or a full
/// `{term, qualified}` object, so tag-array parsing stays robust to LLM
/// output that doesn't perfectly match the documented format. Shared by
/// [`parse_summary_response`] and [`crate::ai::web_research`]'s synthesis
/// parsing so both AI-tagging call sites decode the same JSON shape
/// identically instead of each re-implementing this leniency.
#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum TagJson {
    Plain(String),
    Qualified {
        term: String,
        #[serde(default)]
        qualified: Option<String>,
    },
}

/// Converts raw parsed `tags` into cleaned [`TagTerm`]s: trims whitespace
/// and a leading `#` off `term`, trims `qualified` and drops it if empty,
/// and drops any tag whose `term` ends up empty.
pub(crate) fn clean_tag_terms(tags: Vec<TagJson>) -> Vec<TagTerm> {
    tags.into_iter()
        .map(|tag| match tag {
            TagJson::Plain(term) => TagTerm {
                term,
                qualified: None,
            },
            TagJson::Qualified { term, qualified } => TagTerm { term, qualified },
        })
        .map(|tag| TagTerm {
            term: tag.term.trim().trim_start_matches('#').to_string(),
            qualified: tag
                .qualified
                .map(|q| q.trim().to_string())
                .filter(|q| !q.is_empty()),
        })
        .filter(|tag| !tag.term.is_empty())
        .collect()
}

fn parse_summary_response(response: &str) -> Result<PageSummary> {
    #[derive(Deserialize)]
    struct TopicJson {
        topic: String,
        summary: String,
        #[serde(default)]
        tags: Vec<TagJson>,
    }

    #[derive(Deserialize)]
    struct SummaryJson {
        #[serde(default)]
        title_answer: Option<String>,
        #[serde(default)]
        topics: Vec<TopicJson>,
    }

    let trimmed = response.trim();
    let json_str = extract_json_object(trimmed)?;
    let parsed: SummaryJson = serde_json::from_str(json_str).map_err(|error| {
        concept_parse_error(&format!("invalid page summary JSON: {}", error), trimmed)
    })?;

    let topics = parsed
        .topics
        .into_iter()
        .filter(|topic| !topic.summary.trim().is_empty())
        .map(|topic| TopicSummary {
            topic: topic.topic.trim().to_string(),
            summary: topic.summary.trim().to_string(),
            tags: clean_tag_terms(topic.tags),
        })
        .collect();

    Ok(PageSummary {
        title_answer: parsed
            .title_answer
            .filter(|answer| !answer.trim().is_empty()),
        topics,
    })
}

fn parse_concepts_for_content(
    parsed: Vec<ConceptJson>,
    original_content: &str,
) -> Vec<ConceptExtraction> {
    parsed
        .into_iter()
        .filter_map(|concept| {
            let offset = original_content.find(&concept.text)?;
            Some(ConceptExtraction {
                text: concept.text,
                offset,
            })
        })
        .collect()
}

pub(crate) fn truncate_snippet(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        format!("{}…", super::truncate_to_char_boundary(text, max_len))
    }
}

pub(crate) fn concept_parse_error(reason: &str, response: &str) -> CoreError {
    CoreError::Parse(format!(
        "Failed to parse concept extraction response ({reason}). Response snippet: {}",
        truncate_snippet(response, 200)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::traits::BoxFuture;
    use serde_json::json;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockLlmState {
        calls: usize,
        user_messages: Vec<String>,
    }

    struct MockLlm {
        responses: Arc<Mutex<VecDeque<String>>>,
        state: Arc<Mutex<MockLlmState>>,
    }

    impl MockLlm {
        fn new<I>(responses: I) -> (Self, Arc<Mutex<MockLlmState>>)
        where
            I: IntoIterator<Item = String>,
        {
            let state = Arc::new(Mutex::new(MockLlmState::default()));
            (
                Self {
                    responses: Arc::new(Mutex::new(responses.into_iter().collect())),
                    state: state.clone(),
                },
                state,
            )
        }
    }

    impl LlmProvider for MockLlm {
        fn complete<'a>(
            &'a self,
            messages: &'a [ChatMessage],
            _options: &'a CompletionOptions,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                let user_message = messages
                    .iter()
                    .find(|message| message.role == MessageRole::User)
                    .map(|message| message.content.clone())
                    .unwrap_or_default();

                let mut state = self.state.lock().unwrap();
                state.calls += 1;
                state.user_messages.push(user_message);
                drop(state);

                self.responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| CoreError::Other("No mock LLM response queued".to_string()))
            })
        }

        fn name(&self) -> &str {
            "mock-llm"
        }

        fn health_check<'a>(&'a self) -> BoxFuture<'a, Result<bool>> {
            Box::pin(async move { Ok(true) })
        }
    }

    #[derive(Default)]
    struct MockEmbedderState {
        calls: usize,
        batches: Vec<Vec<String>>,
    }

    struct MockEmbedder {
        state: Arc<Mutex<MockEmbedderState>>,
        embeddings: HashMap<String, Vec<f32>>,
        dimension: usize,
    }

    impl MockEmbedder {
        fn new(embeddings: HashMap<String, Vec<f32>>) -> (Self, Arc<Mutex<MockEmbedderState>>) {
            let dimension = embeddings.values().next().map(Vec::len).unwrap_or(0);
            let state = Arc::new(Mutex::new(MockEmbedderState::default()));
            (
                Self {
                    state: state.clone(),
                    embeddings,
                    dimension,
                },
                state,
            )
        }
    }

    impl Embedder for MockEmbedder {
        fn embed<'a>(&'a self, texts: &'a [String]) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
            Box::pin(async move {
                let mut state = self.state.lock().unwrap();
                state.calls += 1;
                state.batches.push(texts.to_vec());
                drop(state);

                texts
                    .iter()
                    .map(|text| {
                        self.embeddings.get(text).cloned().ok_or_else(|| {
                            CoreError::Other(format!("Missing mock embedding for '{text}'"))
                        })
                    })
                    .collect()
            })
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        fn model_name(&self) -> &str {
            "mock-embedder"
        }
    }

    struct MockVectorStore {
        results: HashMap<String, Vec<SearchResult>>,
    }

    impl MockVectorStore {
        fn new(results: HashMap<String, Vec<SearchResult>>) -> Self {
            Self { results }
        }
    }

    impl VectorStore for MockVectorStore {
        fn upsert<'a>(
            &'a self,
            _chunks: &'a [crate::ai::traits::ChunkEmbedding],
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move { Ok(()) })
        }

        fn search<'a>(
            &'a self,
            query_embedding: &'a [f32],
            _top_k: usize,
            _filter_graph_id: Option<&'a str>,
        ) -> BoxFuture<'a, Result<Vec<SearchResult>>> {
            Box::pin(async move {
                Ok(self
                    .results
                    .get(&embedding_key(query_embedding))
                    .cloned()
                    .unwrap_or_default())
            })
        }

        fn delete_by_page<'a>(
            &'a self,
            _graph_id: &'a str,
            _page_id: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move { Ok(()) })
        }

        fn delete_chunks<'a>(
            &'a self,
            _graph_id: &'a str,
            _chunk_ids: &'a [String],
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move { Ok(()) })
        }

        fn delete_by_graph<'a>(&'a self, _graph_id: &'a str) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move { Ok(()) })
        }

        fn count<'a>(&'a self) -> BoxFuture<'a, Result<usize>> {
            Box::pin(async move { Ok(0) })
        }

        fn count_for_graph<'a>(&'a self, _graph_id: &'a str) -> BoxFuture<'a, Result<usize>> {
            Box::pin(async move { Ok(0) })
        }
    }

    fn embedding_key(embedding: &[f32]) -> String {
        embedding
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    fn search_result(page_id: &str, page_title: &str, score: f32, content: &str) -> SearchResult {
        SearchResult {
            chunk_id: format!("{page_id}-chunk"),
            graph_id: "graph-2".to_string(),
            page_id: page_id.to_string(),
            block_id: Some(format!("{page_id}-block")),
            page_title: page_title.to_string(),
            content: content.to_string(),
            score,
            metadata: json!({}),
        }
    }

    #[test]
    fn truncate_snippet_handles_utf8_boundaries() {
        let snippet = truncate_snippet("abcdefghij🙂klm", 11);

        assert_eq!(snippet, "abcdefghij…");
        assert!(std::str::from_utf8(snippet.as_bytes()).is_ok());
    }

    #[test]
    fn parse_concept_response_returns_error_for_malformed_json() {
        let error = match parse_concept_response(r#"[{"text":"Rust""#, "Rust makes parsing strict")
        {
            Ok(_) => panic!("malformed concept JSON should fail"),
            Err(error) => error,
        };

        assert!(matches!(error, CoreError::Parse(_)));
        assert!(error.to_string().contains("Response snippet"));
    }

    #[tokio::test]
    async fn generate_references_batches_page_concepts_and_embeddings() -> Result<()> {
        let page_title = "Systems Page";
        let blocks = vec![
            (
                "block-1".to_string(),
                "Rust ownership enables memory safety in concurrent systems.".to_string(),
            ),
            (
                "block-2".to_string(),
                "Tokio provides async scheduling for Rust services.".to_string(),
            ),
        ];
        let (llm, llm_state) = MockLlm::new([
            r#"
            [
              {"block_id":"block-1","concepts":[
                {"text":"Rust ownership","type":"concept"},
                {"text":"memory safety","type":"claim"}
              ]},
              {"block_id":"block-2","concepts":[
                {"text":"Tokio","type":"entity"}
              ]}
            ]
        "#
            .to_string(),
            r#"{"title_answer": null, "topics": [{"topic": "Rust and Tokio", "summary": "Rust's ownership model enables safe concurrency; Tokio adds async scheduling.", "tags": [{"term": "rust"}, {"term": "tokio"}]}]}"#
                .to_string(),
        ]);
        let (embedder, embedder_state) = MockEmbedder::new(HashMap::from([
            (format!("{page_title} Rust ownership"), vec![1.0]),
            (format!("{page_title} memory safety"), vec![2.0]),
            (format!("{page_title} Tokio"), vec![3.0]),
        ]));
        let store = MockVectorStore::new(HashMap::from([
            (
                embedding_key(&[1.0]),
                vec![
                    search_result("page-self", "Systems Page", 0.99, "self result"),
                    search_result("page-rust", "Rust Book", 0.9, "rust ownership overview"),
                    search_result("page-low", "Low Score", 0.4, "too weak"),
                ],
            ),
            (
                embedding_key(&[2.0]),
                vec![
                    search_result(
                        "page-borrow",
                        "Borrow Checker",
                        0.8,
                        "borrow rules explained",
                    ),
                    search_result("page-own", "Ownership Guide", 0.7, "ownership patterns"),
                ],
            ),
            (
                embedding_key(&[3.0]),
                vec![search_result(
                    "page-tokio",
                    "Tokio Runtime",
                    0.95,
                    "async runtime overview",
                )],
            ),
        ]));
        let engine = ReferenceEngine::new(ReferenceConfig {
            max_refs_per_paragraph: 3,
            min_similarity_score: 0.6,
            ..Default::default()
        });

        let meta = engine
            .generate_references(
                "page-self",
                page_title,
                &blocks,
                "graph-1",
                &llm,
                &embedder,
                &store,
                &mut |_| {},
            )
            .await?;

        let llm_state = llm_state.lock().unwrap();
        assert_eq!(llm_state.calls, 2);
        assert!(llm_state.user_messages[0].contains("\"block_id\":\"block-1\""));
        assert!(llm_state.user_messages[0].contains("\"block_id\":\"block-2\""));
        drop(llm_state);

        let embedder_state = embedder_state.lock().unwrap();
        assert_eq!(embedder_state.calls, 1);
        assert_eq!(
            embedder_state.batches,
            vec![vec![
                format!("{page_title} Rust ownership"),
                format!("{page_title} memory safety"),
                format!("{page_title} Tokio"),
            ]]
        );
        drop(embedder_state);

        assert_eq!(meta.reference_count, 3);
        assert_eq!(
            meta.references
                .iter()
                .map(|reference| (reference.block_id.as_str(), reference.anchor_text.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("block-1", "Rust ownership"),
                ("block-1", "memory safety"),
                ("block-2", "Tokio"),
            ]
        );
        assert!((meta.references[0].confidence - 0.9).abs() < 1e-6);
        assert!((meta.references[1].confidence - 0.75).abs() < 1e-6);
        assert!((meta.references[2].confidence - 0.95).abs() < 1e-6);
        assert_eq!(
            meta.references[0]
                .related_pages
                .iter()
                .map(|page| page.page_title.as_str())
                .collect::<Vec<_>>(),
            vec!["Rust Book"]
        );
        assert_eq!(
            meta.references[1]
                .related_pages
                .iter()
                .map(|page| page.page_title.as_str())
                .collect::<Vec<_>>(),
            vec!["Borrow Checker", "Ownership Guide"]
        );
        assert_eq!(
            meta.references[1].reference_text,
            "**memory safety**\n→ [[Borrow Checker]] (score: 80%): borrow rules explained\n→ [[Ownership Guide]] (score: 70%): ownership patterns"
        );

        let summary = meta.summary.expect("summary should be generated");
        assert_eq!(summary.title_answer, None);
        assert_eq!(summary.topics.len(), 1);
        assert_eq!(
            summary.topics[0].summary,
            "Rust's ownership model enables safe concurrency; Tokio adds async scheduling."
        );
        assert_eq!(
            summary.topics[0]
                .tags
                .iter()
                .map(|t| t.term.as_str())
                .collect::<Vec<_>>(),
            vec!["rust", "tokio"]
        );
        assert_eq!(
            summary.all_tags(),
            vec!["rust".to_string(), "tokio".to_string()]
        );

        Ok(())
    }
}
